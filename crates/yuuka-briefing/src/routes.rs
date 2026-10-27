//! 定期報告の Web-API（全て auth:user・bot スコープ）。
//!
//! - `GET  /api/periodic-reports`        本人の定期報告一覧
//! - `POST /api/periodic-reports/create` 登録（[`crate::model::ReportSettings::apply`] と同じキー）
//! - `POST /api/periodic-reports/update` 部分更新（`id` + 変えるキーだけ・有効/無効の切替もこれ）
//! - `POST /api/periodic-reports/delete` 削除（`id`）
//! - `POST /api/periodic-reports/test`   テスト配信（`id`）
//!
//! 認可は [`AuthenticatedUser`] extractor で型強制、DB は `State<Db>`、bot スコープは共通の
//! [`resolve_scope`]（`botId`＝body → クエリの順・未指定は 403・未アクセスは 404）で解決する。
//! 他人の行は存在しない扱い（404）。
//!
//! **配信サービスのシーム**: `/test` の実配信は [`DeliveryRunner`] ポート越しに委譲する。実 runner
//! （`yuuka_services::NotifierDeliveryRunner`＝定時配信と同じ生成・送信経路）は supervisor が
//! [`routes_with`] で注入する。[`routes`] の既定は [`NullDeliveryRunner`]（常に `false`＝未配信）。

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::{Extension, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use yuuka_core::DbError;
use yuuka_web::{resolve_scope, AppState, AuthenticatedUser, Db};

use crate::manage::{self, SaveError};
use crate::model::report_view;
use crate::repo;

/// テスト配信の実行ポート。
#[async_trait]
pub trait DeliveryRunner: Send + Sync {
    /// 本人（`user_id` + `bot_id`）の定期報告 `report_id` を、設定した配信先へテスト配信する
    /// （成功なら `true`）。無効にしている定期報告も送る。
    async fn run_report(&self, user_id: &str, bot_id: &str, report_id: i64) -> bool;
}

/// 配信サービス未配線時の既定 runner（常に未配信＝`false`）。
pub struct NullDeliveryRunner;

#[async_trait]
impl DeliveryRunner for NullDeliveryRunner {
    async fn run_report(&self, _user_id: &str, _bot_id: &str, _report_id: i64) -> bool {
        false
    }
}

/// 後から実 runner を差し込める runner（issue #136）。
///
/// ツールレジストリ（`runPeriodicReportNow`）は会話エンジンより先に組まれ、実 runner が使う Messenger は
/// 会話エンジンを受け取る Discord マネージャから作られる（循環）。そのためレジストリにはこれを渡し、
/// Messenger ができた時点で [`LateDeliveryRunner::set`] する。未設定の間は [`NullDeliveryRunner`]
/// と同じく常に未配信（`false`）。
#[derive(Default)]
pub struct LateDeliveryRunner {
    inner: std::sync::OnceLock<Arc<dyn DeliveryRunner>>,
}

impl LateDeliveryRunner {
    /// 未設定（常に未配信）の runner を作る。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 実 runner を設定する。設定は 1 度だけで、2 度目以降は無視して `false` を返す。
    pub fn set(&self, runner: Arc<dyn DeliveryRunner>) -> bool {
        self.inner.set(runner).is_ok()
    }
}

#[async_trait]
impl DeliveryRunner for LateDeliveryRunner {
    async fn run_report(&self, user_id: &str, bot_id: &str, report_id: i64) -> bool {
        match self.inner.get() {
            Some(runner) => runner.run_report(user_id, bot_id, report_id).await,
            None => false,
        }
    }
}

#[derive(Debug, Deserialize)]
struct BotQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
}

/// 定期報告ルータ（既定 [`NullDeliveryRunner`]・`AppState` 上でマージされる）。
pub fn routes() -> Router<AppState> {
    routes_with(Arc::new(NullDeliveryRunner))
}

/// 実配信 runner を注入して定期報告ルータを組む（サービス配線時に使う）。
pub fn routes_with(runner: Arc<dyn DeliveryRunner>) -> Router<AppState> {
    Router::new()
        .route("/api/periodic-reports", get(list))
        .route("/api/periodic-reports/create", post(create))
        .route("/api/periodic-reports/update", post(update))
        .route("/api/periodic-reports/delete", post(delete))
        .route("/api/periodic-reports/test", post(test))
        .layer(Extension(runner))
}

async fn list(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotQuery>,
) -> Response {
    let scope = match resolve_scope(&user.0, &db, q.bot_id.as_deref()).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match repo::list_reports(&db, scope.user_id().as_str(), scope.bot_id().as_str()).await {
        Ok(reports) => json_ok(json!({
            "success": true,
            "reports": reports.iter().map(report_view).collect::<Vec<_>>(),
        })),
        Err(e) => server_error(&e),
    }
}

async fn create(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotQuery>,
    Json(body): Json<Value>,
) -> Response {
    save(user, db, q, body, false).await
}

async fn update(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotQuery>,
    Json(body): Json<Value>,
) -> Response {
    save(user, db, q, body, true).await
}

/// 登録・更新の共通処理（`is_update` なら body の `id` の行を部分更新）。
async fn save(
    user: AuthenticatedUser,
    db: Db,
    q: BotQuery,
    body: Value,
    is_update: bool,
) -> Response {
    let id = if is_update {
        match report_id(&body) {
            Some(id) => Some(id),
            None => return bad_request("id を指定してください。"),
        }
    } else {
        None
    };
    let raw_bot = body
        .get("botId")
        .and_then(Value::as_str)
        .or(q.bot_id.as_deref());
    let scope = match resolve_scope(&user.0, &db, raw_bot).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match manage::save_report(
        &db,
        scope.user_id().as_str(),
        scope.bot_id().as_str(),
        id,
        &body,
    )
    .await
    {
        Ok(report) => {
            let verb = if is_update { "保存" } else { "登録" };
            json_ok(json!({
                "success": true,
                "message": format!("定期報告「{}」を{verb}しました。", report.settings.name),
                "report": report_view(&report),
            }))
        }
        Err(SaveError::Invalid(message)) => bad_request(&message),
        Err(SaveError::NotFound) => not_found(),
        Err(SaveError::Db(e)) => server_error(&e),
    }
}

async fn delete(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotQuery>,
    Json(body): Json<Value>,
) -> Response {
    let Some(id) = report_id(&body) else {
        return bad_request("id を指定してください。");
    };
    let raw_bot = body
        .get("botId")
        .and_then(Value::as_str)
        .or(q.bot_id.as_deref());
    let scope = match resolve_scope(&user.0, &db, raw_bot).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match repo::delete_report(&db, scope.user_id().as_str(), scope.bot_id().as_str(), id).await {
        Ok(true) => json_ok(json!({ "success": true, "message": "定期報告を削除しました。" })),
        Ok(false) => not_found(),
        Err(e) => server_error(&e),
    }
}

async fn test(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Extension(runner): Extension<Arc<dyn DeliveryRunner>>,
    Query(q): Query<BotQuery>,
    Json(body): Json<Value>,
) -> Response {
    let Some(id) = report_id(&body) else {
        return bad_request("id を指定してください。");
    };
    let raw_bot = body
        .get("botId")
        .and_then(Value::as_str)
        .or(q.bot_id.as_deref());
    let scope = match resolve_scope(&user.0, &db, raw_bot).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    let (user_id, bot_id) = (scope.user_id().as_str(), scope.bot_id().as_str());
    let report = match repo::find_report(&db, user_id, bot_id, id).await {
        Ok(Some(report)) => report,
        Ok(None) => return not_found(),
        Err(e) => return server_error(&e),
    };
    let sent = runner.run_report(user_id, bot_id, id).await;
    json_ok(json!({
        "success": sent,
        "message": if sent {
            format!(
                "定期報告「{}」をテスト配信しました。Discordを確認してください。",
                report.settings.name
            )
        } else {
            "配信に失敗しました。Botが起動しているか、配信先の設定を確認してください。".to_owned()
        },
    }))
}

// ─── ヘルパ ──────────────────────────────────────────────────────────────────

fn report_id(body: &Value) -> Option<i64> {
    body.get("id").and_then(Value::as_i64)
}

fn json_ok(body: Value) -> Response {
    (StatusCode::OK, Json(body)).into_response()
}

fn bad_request(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "success": false, "message": message })),
    )
        .into_response()
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "success": false, "message": "定期報告が見つかりません。" })),
    )
        .into_response()
}

/// DB 失敗は 500（内部エラー文字列は応答に出さない）。
fn server_error(err: &DbError) -> Response {
    tracing::error!(error = %err, "定期報告 API の DB 操作に失敗");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "success": false, "message": "内部エラーが発生しました。" })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use serde_json::{json, Value};
    use tower::ServiceExt;
    use yuuka_core::AuthError;
    use yuuka_types::{Role, SessionUser};
    use yuuka_web::test_support::{with_bot_query, TEST_BOT_ID};
    use yuuka_web::{AppState, AuthBackend, Db, WebConfig};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    struct FakeAuth;

    #[async_trait]
    impl AuthBackend for FakeAuth {
        async fn session_user(&self, token: &str) -> Result<Option<SessionUser>, AuthError> {
            Ok(match token {
                "good" => Some(SessionUser {
                    discord_id: "u".to_owned(),
                    username: "u".to_owned(),
                    role: Role::User,
                }),
                _ => None,
            })
        }
        async fn desktop_user(&self, _token: &str) -> Result<Option<SessionUser>, AuthError> {
            Ok(None)
        }
    }

    // 空ファイル → Db::open が実スキーマを流す（periodic_reports/users を含む）。
    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_periodic_report_routes_{}_{seq}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        drop(rusqlite::Connection::open(&path).expect("create empty db file"));
        let db = Db::open(&path).expect("open db");
        let conn = rusqlite::Connection::open(&path).expect("seed user conn");
        conn.execute(
            "INSERT OR IGNORE INTO users (discord_id, username, salt) VALUES ('u', 'u', 'x')",
            [],
        )
        .expect("seed user");
        db
    }

    async fn app_with(runner: Arc<dyn super::DeliveryRunner>) -> axum::Router {
        let db = seed_db();
        yuuka_web::test_support::seed_bot(&db, TEST_BOT_ID, "u").await;
        let state = AppState::new(Arc::new(FakeAuth), WebConfig::default(), db);
        with_bot_query(super::routes_with(runner).with_state(state), TEST_BOT_ID)
    }

    async fn app() -> axum::Router {
        app_with(Arc::new(super::NullDeliveryRunner)).await
    }

    async fn send(app: &axum::Router, method: &str, uri: &str, body: &str) -> (StatusCode, Value) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, json)
    }

    async fn post(app: &axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
        send(app, "POST", uri, &body.to_string()).await
    }

    fn morning() -> Value {
        json!({
            "name": "朝報",
            "schedule_cron": "0 7 * * *",
            "sections": ["weather", "news"],
            "location_name": "東京",
            "news_feeds": ["https://example.com/rss"],
            "target_type": "channel",
            "target_id": "chan-1",
        })
    }

    #[tokio::test]
    async fn create_list_update_delete_roundtrip() {
        let app = app().await;

        let (st, j) = post(&app, "/api/periodic-reports/create", morning()).await;
        assert_eq!(st, StatusCode::OK, "{j}");
        assert_eq!(j["message"], "定期報告「朝報」を登録しました。");
        let id = j["report"]["id"].as_i64().unwrap();
        assert_eq!(j["report"]["enabled"], true);
        assert_eq!(j["report"]["private"], false);

        // 同じ名前は登録できない。
        let (st, j) = post(&app, "/api/periodic-reports/create", morning()).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(j["message"].as_str().unwrap().contains("既にあります"));

        // 有効/無効だけの部分更新（他の項目は保たれる）。
        let (st, _) = post(
            &app,
            "/api/periodic-reports/update",
            json!({"id": id, "enabled": false}),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let (st, j) = send(&app, "GET", "/api/periodic-reports", "").await;
        assert_eq!(st, StatusCode::OK);
        let reports = j["reports"].as_array().unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0]["enabled"], false);
        assert_eq!(reports[0]["target_id"], "chan-1");
        assert_eq!(reports[0]["sections"], json!(["weather", "news"]));

        // 非公開の内容を足すならチャンネルには送れない。DM に変えれば保存できる。
        let (st, j) = post(
            &app,
            "/api/periodic-reports/update",
            json!({"id": id, "sections": ["weather", "tasks"]}),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(j["message"]
            .as_str()
            .unwrap()
            .contains("DM にしか送れません"));
        let (st, j) = post(
            &app,
            "/api/periodic-reports/update",
            json!({"id": id, "sections": ["weather", "tasks"], "target_type": "dm"}),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{j}");
        assert_eq!(j["report"]["target_id"], Value::Null);
        assert_eq!(j["report"]["private"], true);

        let (st, _) = post(&app, "/api/periodic-reports/delete", json!({"id": id})).await;
        assert_eq!(st, StatusCode::OK);
        let (st, _) = post(&app, "/api/periodic-reports/delete", json!({"id": id})).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let (_, j) = send(&app, "GET", "/api/periodic-reports", "").await;
        assert_eq!(j["reports"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn invalid_requests_are_rejected() {
        let app = app().await;
        let (st, _) = post(
            &app,
            "/api/periodic-reports/update",
            json!({"enabled": true}),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let (st, _) = post(
            &app,
            "/api/periodic-reports/update",
            json!({"id": 999, "enabled": true}),
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let mut body = morning();
        body["schedule_cron"] = json!("bad");
        let (st, j) = post(&app, "/api/periodic-reports/create", body).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(j["message"].as_str().unwrap().contains("cron"));
    }

    /// 呼ばれた引数を記録して指定の結果を返す runner。
    struct FixedRunner(bool, std::sync::Mutex<Vec<(String, String, i64)>>);

    #[async_trait]
    impl super::DeliveryRunner for FixedRunner {
        async fn run_report(&self, user_id: &str, bot_id: &str, report_id: i64) -> bool {
            self.1
                .lock()
                .unwrap()
                .push((user_id.to_owned(), bot_id.to_owned(), report_id));
            self.0
        }
    }

    #[tokio::test]
    async fn test_delivery_uses_runner_for_own_report() {
        let runner = Arc::new(FixedRunner(true, std::sync::Mutex::default()));
        let app = app_with(runner.clone()).await;
        let (_, j) = post(&app, "/api/periodic-reports/create", morning()).await;
        let id = j["report"]["id"].as_i64().unwrap();

        let (st, j) = post(&app, "/api/periodic-reports/test", json!({"id": id})).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["success"], true);
        assert!(j["message"]
            .as_str()
            .unwrap()
            .contains("「朝報」をテスト配信"));
        assert_eq!(
            runner.1.lock().unwrap().as_slice(),
            [("u".to_owned(), TEST_BOT_ID.to_owned(), id)]
        );

        // 無い id は runner を呼ばずに 404。
        let (st, _) = post(&app, "/api/periodic-reports/test", json!({"id": id + 1})).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        assert_eq!(runner.1.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn late_runner_is_null_until_set_then_delegates() {
        use super::{DeliveryRunner, LateDeliveryRunner};

        let late = LateDeliveryRunner::new();
        // 未設定の間は未配信。
        assert!(!late.run_report("u", "b", 1).await);

        // 設定後は実 runner へ委譲する。
        assert!(late.set(Arc::new(FixedRunner(true, std::sync::Mutex::default()))));
        assert!(late.run_report("u", "b", 1).await);

        // 2 度目の設定は無視（最初の runner のまま）。
        assert!(!late.set(Arc::new(FixedRunner(false, std::sync::Mutex::default()))));
        assert!(late.run_report("u", "b", 1).await);
    }
}
