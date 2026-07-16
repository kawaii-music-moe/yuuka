//! 配信設定 Web-API（Node `deliveryRoutes`・全て auth:user）。
//!
//! 朝報（briefing）・日報/週報（report）の設定 CRUD と、テスト配信 2 本を提供する。
//! 認可は [`AuthenticatedUser`] extractor で型強制、DB は `State<Db>`、bot スコープは共通の
//! [`resolve_scope`]（`botId`＝body → クエリの順・未アクセスは `system_default` へフォールバック）で解決。
//! レスポンス形は Node `sendJson` にバイト単位で合わせる（フラット `{success, ...}`・日本語 verbatim）。
//!
//! **配信サービスのシーム**: `/api/briefing/test`・`/api/report-configs/test` の実配信は
//! [`DeliveryRunner`] ポート越しに委譲する。サービス本体（天気/RSS 取得・Discord 送信）は未移植のため、
//! 既定は [`NullDeliveryRunner`]（常に `false`＝未配信）へ縮退する（route surface・検証・設定存在チェックは
//! 完全に働く）。サービス配線時に実 runner を [`routes_with`] へ注入すれば live 化する。
//! これは admin/settings の `BotRuntime`/`NullBotRuntime` と同じ縮退方針。

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

use crate::repo;
use crate::tools::{is_likely_public_http_url, is_valid_cron_basic};

/// テスト配信の実行ポート（Node `runBriefingForUser` / `runReportForUser` のシーム）。
#[async_trait]
pub trait DeliveryRunner: Send + Sync {
    /// 朝報をテスト配信する（成功なら `true`）。
    async fn run_briefing(&self, user_id: &str, bot_id: &str) -> bool;
    /// 日報/週報（`report_type`＝"daily"/"weekly"）をテスト配信する（成功なら `true`）。
    async fn run_report(&self, user_id: &str, bot_id: &str, report_type: &str) -> bool;
}

/// 配信サービス未配線時の既定 runner（常に未配信＝`false`）。
pub struct NullDeliveryRunner;

#[async_trait]
impl DeliveryRunner for NullDeliveryRunner {
    async fn run_briefing(&self, _user_id: &str, _bot_id: &str) -> bool {
        false
    }
    async fn run_report(&self, _user_id: &str, _bot_id: &str, _report_type: &str) -> bool {
        false
    }
}

#[derive(Debug, Deserialize)]
struct BotQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
}

/// 配信設定ルータ（既定 [`NullDeliveryRunner`]・`AppState` 上でマージされる）。
pub fn routes() -> Router<AppState> {
    routes_with(Arc::new(NullDeliveryRunner))
}

/// 実配信 runner を注入して配信設定ルータを組む（サービス配線時に使う）。
pub fn routes_with(runner: Arc<dyn DeliveryRunner>) -> Router<AppState> {
    Router::new()
        .route(
            "/api/briefing-config",
            get(get_briefing_config).post(post_briefing_config),
        )
        .route("/api/briefing/test", post(briefing_test))
        .route(
            "/api/report-configs",
            get(get_report_configs).post(post_report_configs),
        )
        .route("/api/report-configs/test", post(report_test))
        .layer(Extension(runner))
}

// ─── 朝報設定 ────────────────────────────────────────────────────────────────

async fn get_briefing_config(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotQuery>,
) -> Response {
    let scope = match resolve_scope(&user.0, &db, q.bot_id.as_deref()).await {
        Ok(s) => s,
        Err(e) => return server_error(&e),
    };
    let config =
        match repo::find_briefing(&db, scope.user_id().as_str(), scope.bot_id().as_str()).await {
            Ok(c) => c,
            Err(e) => return server_error(&e),
        };
    let config_json = config.map_or(Value::Null, |c| {
        json!({
            "enabled": c.enabled,
            "schedule_cron": c.schedule_cron,
            "target_type": c.target_type,
            "target_id": c.target_id,
            "weather_lat": c.weather_lat,
            "weather_lng": c.weather_lng,
            "location_name": c.location_name,
            "news_feeds": c.news_feeds,
            "news_keywords": c.news_keywords,
        })
    });
    json_ok(json!({ "success": true, "config": config_json }))
}

async fn post_briefing_config(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotQuery>,
    Json(body): Json<Value>,
) -> Response {
    // cron が文字列で与えられたら妥当性検証（Node `cron.validate`）。
    if let Some(cron) = body.get("schedule_cron").and_then(Value::as_str) {
        if !is_valid_cron_basic(cron) {
            return bad_request("cron式が不正です。");
        }
    }

    // SSRF: news_feeds 配列は http(s) 公開URLのみ（非空 trim 後に検査・Node 同順）。
    let feeds_present = body.get("news_feeds").and_then(Value::as_array);
    if let Some(arr) = feeds_present {
        let bad = arr
            .iter()
            .map(js_string)
            .find(|s| !s.trim().is_empty() && !is_likely_public_http_url(s));
        if let Some(bad) = bad {
            return bad_request(&format!(
                "不正なフィードURLです（http(s)の公開URLのみ指定できます）: {bad}"
            ));
        }
    }

    let raw_bot = body
        .get("botId")
        .and_then(Value::as_str)
        .or(q.bot_id.as_deref());
    let scope = match resolve_scope(&user.0, &db, raw_bot).await {
        Ok(s) => s,
        Err(e) => return server_error(&e),
    };

    // Node `key in body` 意味論: present なフィールドだけ patch へ載せる。
    let patch = repo::BriefingPatch {
        enabled: body.get("enabled").map(|v| v.as_bool() == Some(true)),
        schedule_cron: body
            .get("schedule_cron")
            .and_then(Value::as_str)
            .map(str::to_owned),
        target_type: body
            .get("target_type")
            .map(|v| target_type_of(v).to_owned()),
        target_id: body.get("target_id").map(opt_trimmed),
        weather_lat: body.get("weather_lat").map(weather_value),
        weather_lng: body.get("weather_lng").map(weather_value),
        location_name: body.get("location_name").map(opt_trimmed),
        // Node は `.map(String)` で空要素も残す（非フィルタ）。
        news_feeds: body
            .get("news_feeds")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(js_string).collect()),
        news_keywords: body
            .get("news_keywords")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(js_string).collect()),
    };

    if let Err(e) = repo::upsert_briefing(
        &db,
        scope.user_id().as_str(),
        scope.bot_id().as_str(),
        patch,
    )
    .await
    {
        return server_error(&e);
    }
    ok_message("朝報の設定を保存しました。")
}

async fn briefing_test(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Extension(runner): Extension<Arc<dyn DeliveryRunner>>,
    Query(q): Query<BotQuery>,
    Json(body): Json<Value>,
) -> Response {
    let raw_bot = body
        .get("botId")
        .and_then(Value::as_str)
        .or(q.bot_id.as_deref());
    let scope = match resolve_scope(&user.0, &db, raw_bot).await {
        Ok(s) => s,
        Err(e) => return server_error(&e),
    };
    let (uid, bid) = (
        scope.user_id().as_str().to_owned(),
        scope.bot_id().as_str().to_owned(),
    );
    // 設定が無ければ配信前に 400（Node parity）。
    match repo::find_briefing(&db, &uid, &bid).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return bad_request("朝報がまだ設定されていません。先に設定を保存してください。")
        }
        Err(e) => return server_error(&e),
    }
    let sent = runner.run_briefing(&uid, &bid).await;
    json_ok(json!({
        "success": sent,
        "message": if sent {
            "朝報をテスト配信しました。Discordを確認してください。"
        } else {
            "配信に失敗しました。Botが起動しているか確認してください。"
        },
    }))
}

// ─── 日報・週報設定 ──────────────────────────────────────────────────────────

async fn get_report_configs(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotQuery>,
) -> Response {
    let scope = match resolve_scope(&user.0, &db, q.bot_id.as_deref()).await {
        Ok(s) => s,
        Err(e) => return server_error(&e),
    };
    let configs =
        match repo::get_reports(&db, scope.user_id().as_str(), scope.bot_id().as_str()).await {
            Ok(list) => list,
            Err(e) => return server_error(&e),
        };
    let configs: Vec<Value> = configs
        .into_iter()
        .map(|c| {
            json!({
                "type": c.r#type,
                "enabled": c.enabled,
                "schedule_cron": c.schedule_cron,
                "target_type": c.target_type,
                "target_id": c.target_id,
            })
        })
        .collect();
    json_ok(json!({ "success": true, "configs": configs }))
}

async fn post_report_configs(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Query(q): Query<BotQuery>,
    Json(body): Json<Value>,
) -> Response {
    let report_type = body.get("type").and_then(Value::as_str).unwrap_or("");
    if report_type != "daily" && report_type != "weekly" {
        return bad_request("type は 'daily' または 'weekly' を指定してください。");
    }
    if let Some(cron) = body.get("schedule_cron").and_then(Value::as_str) {
        if !is_valid_cron_basic(cron) {
            return bad_request("cron式が不正です。");
        }
    }
    let raw_bot = body
        .get("botId")
        .and_then(Value::as_str)
        .or(q.bot_id.as_deref());
    let scope = match resolve_scope(&user.0, &db, raw_bot).await {
        Ok(s) => s,
        Err(e) => return server_error(&e),
    };

    let patch = repo::ReportPatch {
        enabled: body.get("enabled").map(|v| v.as_bool() == Some(true)),
        schedule_cron: body
            .get("schedule_cron")
            .and_then(Value::as_str)
            .map(str::to_owned),
        target_type: body
            .get("target_type")
            .map(|v| target_type_of(v).to_owned()),
        target_id: body.get("target_id").map(opt_trimmed),
    };
    if let Err(e) = repo::upsert_report(
        &db,
        scope.user_id().as_str(),
        scope.bot_id().as_str(),
        report_type,
        patch,
    )
    .await
    {
        return server_error(&e);
    }
    let label = if report_type == "daily" {
        "日報"
    } else {
        "週報"
    };
    ok_message(&format!("{label}の設定を保存しました。"))
}

async fn report_test(
    user: AuthenticatedUser,
    State(db): State<Db>,
    Extension(runner): Extension<Arc<dyn DeliveryRunner>>,
    Query(q): Query<BotQuery>,
    Json(body): Json<Value>,
) -> Response {
    // Node: `type === "weekly" ? "weekly" : "daily"`（既定 daily）。
    let report_type = if body.get("type").and_then(Value::as_str) == Some("weekly") {
        "weekly"
    } else {
        "daily"
    };
    let raw_bot = body
        .get("botId")
        .and_then(Value::as_str)
        .or(q.bot_id.as_deref());
    let scope = match resolve_scope(&user.0, &db, raw_bot).await {
        Ok(s) => s,
        Err(e) => return server_error(&e),
    };
    let sent = runner
        .run_report(
            scope.user_id().as_str(),
            scope.bot_id().as_str(),
            report_type,
        )
        .await;
    json_ok(json!({
        "success": sent,
        "message": if sent { "レポートをテスト配信しました。" } else { "配信に失敗しました。" },
    }))
}

// ─── ヘルパ ──────────────────────────────────────────────────────────────────

/// `target_type` は "channel" のときだけ channel・それ以外は dm（Node parity）。
fn target_type_of(v: &Value) -> &'static str {
    if v.as_str() == Some("channel") {
        "channel"
    } else {
        "dm"
    }
}

/// `typeof x === "string" && x.trim() ? x.trim() : null` 相当（present 前提の内側値）。
fn opt_trimmed(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// weather 列の値（null/空文字は NULL へ・それ以外は数値化・Node `Number(...)`）。
fn weather_value(v: &Value) -> Option<f64> {
    if v.is_null() {
        return None;
    }
    if let Some(s) = v.as_str() {
        let t = s.trim();
        return if t.is_empty() {
            None
        } else {
            t.parse::<f64>().ok()
        };
    }
    v.as_f64()
}

/// JS `String(x)` 相当（文字列はそのまま・null は "null"・他は JSON 表現）。フィード/キーワード用。
fn js_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".to_owned(),
        other => other.to_string(),
    }
}

fn json_ok(body: Value) -> Response {
    (StatusCode::OK, Json(body)).into_response()
}

fn ok_message(message: &str) -> Response {
    json_ok(json!({ "success": true, "message": message }))
}

fn bad_request(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "success": false, "message": message })),
    )
        .into_response()
}

/// DB 失敗は 500（内部エラー文字列は出さない・Node の 500 経路に対応）。
fn server_error(err: &DbError) -> Response {
    // ログ相当（内部詳細）は将来 tracing で。応答は定型のみ。
    let _ = err;
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
    use serde_json::Value;
    use tower::ServiceExt;
    use yuuka_core::AuthError;
    use yuuka_types::{Role, SessionUser};
    use yuuka_web::{AppState, AuthBackend, Db, WebConfig};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    struct FakeAuth;

    #[async_trait]
    impl AuthBackend for FakeAuth {
        async fn session_user(&self, token: &str) -> Result<Option<SessionUser>, AuthError> {
            Ok((token == "good").then(|| SessionUser {
                discord_id: "u".to_owned(),
                username: "u".to_owned(),
                role: Role::User,
            }))
        }
        async fn desktop_user(&self, _token: &str) -> Result<Option<SessionUser>, AuthError> {
            Ok(None)
        }
    }

    // 空ファイル → Db::open が V17 baseline を流す（briefing_configs/report_configs/users を含む）。
    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_delivery_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let conn = rusqlite::Connection::open(&path).expect("create empty db file");
            drop(conn);
        }
        let db = Db::open(&path).expect("open db");
        {
            let conn = rusqlite::Connection::open(&path).expect("seed user conn");
            conn.execute(
                "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt) \
                 VALUES ('u', 'u', 'x', 'x')",
                [],
            )
            .expect("seed user");
        }
        db
    }

    fn app() -> axum::Router {
        let state = AppState::new(Arc::new(FakeAuth), WebConfig::default(), seed_db());
        super::routes().with_state(state)
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

    #[tokio::test]
    async fn briefing_get_unset_is_null_then_roundtrips() {
        let app = app();

        // 未設定は config: null。
        let (st, j) = send(&app, "GET", "/api/briefing-config", "").await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["success"], true);
        assert!(j["config"].is_null());

        // 保存（enabled + cron + 地名 + 緯度 + 公開フィード）。
        let (st, j) = send(
            &app,
            "POST",
            "/api/briefing-config",
            r#"{"enabled":true,"schedule_cron":"0 8 * * *","location_name":"東京","weather_lat":35.6,"news_feeds":["https://example.com/a.xml"]}"#,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["message"], "朝報の設定を保存しました。");

        // GET に反映。
        let (_st, j) = send(&app, "GET", "/api/briefing-config", "").await;
        let c = &j["config"];
        assert_eq!(c["enabled"], true);
        assert_eq!(c["schedule_cron"], "0 8 * * *");
        assert_eq!(c["location_name"], "東京");
        assert_eq!(c["weather_lat"], 35.6);
        assert_eq!(
            c["news_feeds"],
            serde_json::json!(["https://example.com/a.xml"])
        );

        // 部分更新: location_name を null で明示クリア・他は保持。
        let (_st, _j) = send(
            &app,
            "POST",
            "/api/briefing-config",
            r#"{"location_name":null}"#,
        )
        .await;
        let (_st, j) = send(&app, "GET", "/api/briefing-config", "").await;
        assert!(j["config"]["location_name"].is_null());
        assert_eq!(j["config"]["enabled"], true); // 保持。
    }

    #[tokio::test]
    async fn briefing_post_rejects_bad_cron_and_internal_feed() {
        let app = app();
        let (st, j) = send(
            &app,
            "POST",
            "/api/briefing-config",
            r#"{"schedule_cron":"nope"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert_eq!(j["message"], "cron式が不正です。");

        let (st, j) = send(
            &app,
            "POST",
            "/api/briefing-config",
            r#"{"news_feeds":["http://169.254.169.254/latest"]}"#,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(j["message"].as_str().unwrap().contains("不正なフィードURL"));
    }

    #[tokio::test]
    async fn briefing_test_requires_config_then_null_runner_reports_failure() {
        let app = app();
        // 未設定 → 400。
        let (st, j) = send(&app, "POST", "/api/briefing/test", "{}").await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert_eq!(
            j["message"],
            "朝報がまだ設定されていません。先に設定を保存してください。"
        );

        // 設定後は 200 だが NullDeliveryRunner は未配信（success:false）。
        send(
            &app,
            "POST",
            "/api/briefing-config",
            r#"{"enabled":true,"schedule_cron":"0 8 * * *"}"#,
        )
        .await;
        let (st, j) = send(&app, "POST", "/api/briefing/test", "{}").await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["success"], false);
        assert!(j["message"].as_str().unwrap().contains("配信に失敗"));
    }

    #[tokio::test]
    async fn report_configs_roundtrip_and_validation() {
        let app = app();

        // 不正 type → 400。
        let (st, j) = send(&app, "POST", "/api/report-configs", r#"{"type":"monthly"}"#).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert_eq!(
            j["message"],
            "type は 'daily' または 'weekly' を指定してください。"
        );

        // daily 保存。
        let (st, j) = send(
            &app,
            "POST",
            "/api/report-configs",
            r#"{"type":"daily","enabled":true,"schedule_cron":"0 21 * * *"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["message"], "日報の設定を保存しました。");

        // GET に反映。
        let (st, j) = send(&app, "GET", "/api/report-configs", "").await;
        assert_eq!(st, StatusCode::OK);
        let configs = j["configs"].as_array().unwrap();
        assert!(configs
            .iter()
            .any(|c| c["type"] == "daily" && c["enabled"] == true));

        // report/test は NullDeliveryRunner で success:false（200）。
        let (st, j) = send(
            &app,
            "POST",
            "/api/report-configs/test",
            r#"{"type":"weekly"}"#,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["success"], false);
    }
}
