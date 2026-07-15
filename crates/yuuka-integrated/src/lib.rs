//! yuuka-integrated — Bot 統合管理 API（`/api/integrated/*`・全ルート auth:"user"）。Node
//! `integratedRoutes.ts` パリティ。owner が「自分の Bot」のヘルス/起動停止と「自分のリソース
//! （認証情報/MCP/Google）」の Bot 別利用許可を一括管理する。全アクセスは auth:"user" かつ owner
//! 本人スコープ（[`AuthenticatedUser`](yuuka_web::AuthenticatedUser) extractor で型強制）。
//!
//! DB 効果は [`repo`] の直接 SQL（yuuka-orchestrator 非依存）と yuuka-google の repo 再利用で常に完全に
//! 働く。ルート固有の実行時依存（Bot ライフサイクル・Google Calendar 取得）は [`IntegratedRuntime`] に
//! まとめ `Extension` レイヤで注入する（`SettingsRuntime`/`AdminRuntime` と同方式）。
//!
//! **Bot ライフサイクルシーム**（[`BotLifecycle`]）: `run_status`（Node `botRunStatus`）・`start`（Node
//! `startCustomBot`）・`stop`（Node `stopCustomBot`）は稼働中 Discord クライアントに触れる。Discord
//! gateway は `YUUKA_RUST_DISCORD` ゲート既定 off のため既定は [`NullBotLifecycle`] に縮退する
//! （`run_status`→`{false,false}`・`start`→`false`・`stop`→no-op）。この縮退は正直である：gateway 無しでは
//! Discord Bot が本当に起動できないため、start/restart は `set_bot_stopped(false)` を DB へ反映してから
//! `start`→`false` → **502**（running:false, connected:false）を返す。**DB 効果（stopped フラグ・許可付与・
//! Google 割当・floor 進行）は常に完全に働く**。gateway 配線後に実 `BotLifecycle` を注入すれば live 化する。
//!
//! Google Calendar 取得は [`yuuka_google::CalendarPort`] シーム越し（未配線時は
//! [`NullCalendar`](yuuka_google::NullCalendar)＝空一覧・キャッシュ無効化 no-op）。

use std::sync::Arc;

use async_trait::async_trait;
use axum::routing::{get, post};
use axum::{Extension, Router};
use yuuka_google::CalendarPort;
use yuuka_web::AppState;

pub mod repo;
mod routes;

/// Bot のランタイム稼働状態（Node `botRunStatus` の戻り `{running, connected}`）。
#[derive(Debug, Clone, Copy)]
pub struct BotRunStatus {
    /// クライアントが生成済みか（Node `!!client.readyAt` / `!!customClients.get(botId)`）。
    pub running: bool,
    /// Discord gateway と接続済みか（Node `client.isReady()` / `!!c?.readyAt`）。
    pub connected: bool,
}

/// 稼働中 Discord クライアントへの起動/停止/状態照会ポート（Node `startCustomBot`/`stopCustomBot`/
/// `botRunStatus`）。gateway 未配線時は [`NullBotLifecycle`] へ縮退する。
#[async_trait]
pub trait BotLifecycle: Send + Sync {
    /// Bot の稼働状態を返す（Node `botRunStatus`）。
    fn run_status(&self, bot_id: &str) -> BotRunStatus;
    /// 保存済みトークンで Bot を（再）起動する（Node `startCustomBot`・成否 bool）。
    async fn start(&self, bot_id: &str) -> bool;
    /// 稼働中の Bot クライアントを停止する（Node `stopCustomBot`）。冪等・未稼働は no-op。
    async fn stop(&self, bot_id: &str);
}

/// Discord gateway 未配線時の縮退実装（正直な縮退：起動不能・非稼働）。
pub struct NullBotLifecycle;

#[async_trait]
impl BotLifecycle for NullBotLifecycle {
    fn run_status(&self, _bot_id: &str) -> BotRunStatus {
        BotRunStatus {
            running: false,
            connected: false,
        }
    }
    async fn start(&self, _bot_id: &str) -> bool {
        false
    }
    async fn stop(&self, _bot_id: &str) {}
}

/// 統合ルートが使う実行時依存（`Extension` で各ハンドラへ注入）。
pub struct IntegratedRuntime {
    /// Bot 起動/停止/状態照会のシーム（未配線時は [`NullBotLifecycle`]）。
    lifecycle: Arc<dyn BotLifecycle>,
    /// Google Calendar 取得/キャッシュ無効化のシーム（未配線時は
    /// [`NullCalendar`](yuuka_google::NullCalendar)）。
    calendar: Arc<dyn CalendarPort>,
}

impl IntegratedRuntime {
    /// 実行時依存を束ねる。
    #[must_use]
    pub fn new(lifecycle: Arc<dyn BotLifecycle>, calendar: Arc<dyn CalendarPort>) -> Self {
        Self {
            lifecycle,
            calendar,
        }
    }
}

/// 統合ルータ（`AppState` 上でマージされる）。ルート固有依存を `Extension` で載せる。
pub fn routes(runtime: Arc<IntegratedRuntime>) -> Router<AppState> {
    Router::new()
        .route("/api/integrated/overview", get(routes::overview))
        .route("/api/integrated/bots/start", post(routes::bots_start))
        .route("/api/integrated/bots/stop", post(routes::bots_stop))
        .route("/api/integrated/bots/restart", post(routes::bots_restart))
        .route(
            "/api/integrated/bots/clear-history",
            post(routes::bots_clear_history),
        )
        .route("/api/integrated/grants/mcp", post(routes::grants_mcp))
        .route(
            "/api/integrated/grants/credential",
            post(routes::grants_credential),
        )
        .route("/api/integrated/grants/google", post(routes::grants_google))
        .route(
            "/api/integrated/google/accounts/primary",
            post(routes::google_accounts_primary),
        )
        .route(
            "/api/integrated/google/accounts/delete",
            post(routes::google_accounts_delete),
        )
        .route(
            "/api/integrated/google/accounts/calendars",
            post(routes::google_accounts_calendars),
        )
        .route(
            "/api/integrated/google/accounts/{id}/calendars",
            get(routes::google_account_calendars_list),
        )
        .layer(Extension(runtime))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use rusqlite::{params, Connection};
    use serde_json::Value;
    use tower::ServiceExt;
    use yuuka_core::AuthError;
    use yuuka_google::NullCalendar;
    use yuuka_types::{Role, SessionUser};
    use yuuka_web::{AppState, AuthBackend, Db, WebConfig};

    use crate::{IntegratedRuntime, NullBotLifecycle};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    struct FakeAuth {
        user: SessionUser,
    }

    #[async_trait]
    impl AuthBackend for FakeAuth {
        async fn session_user(&self, token: &str) -> Result<Option<SessionUser>, AuthError> {
            Ok((token == "good").then(|| self.user.clone()))
        }
        async fn desktop_user(&self, _token: &str) -> Result<Option<SessionUser>, AuthError> {
            Ok(None)
        }
    }

    fn fresh_db() -> (Db, std::path::PathBuf) {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_integrated_route_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        drop(Connection::open(&path).expect("create db file"));
        let db = Db::open(&path).expect("open db");
        (db, path)
    }

    fn app_for(db: Db, user: &str) -> axum::Router {
        let auth = Arc::new(FakeAuth {
            user: SessionUser {
                discord_id: user.to_owned(),
                username: user.to_owned(),
                role: Role::User,
            },
        });
        let state = AppState::new(auth, WebConfig::default(), db);
        let rt = Arc::new(IntegratedRuntime::new(
            Arc::new(NullBotLifecycle),
            Arc::new(NullCalendar),
        ));
        super::routes(rt).with_state(state)
    }

    fn seed_user(path: &std::path::Path, user: &str) {
        Connection::open(path)
            .expect("open")
            .execute(
                "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt) \
                 VALUES (?1, ?1, 'h', 's')",
                params![user],
            )
            .expect("seed user");
    }

    fn seed_bot(path: &std::path::Path, bot: &str, user: &str, token: Option<&str>) {
        Connection::open(path)
            .expect("open")
            .execute(
                "INSERT INTO bots (id, user_id, name, discord_token_encrypted) \
                 VALUES (?1, ?2, ?1, ?3)",
                params![bot, user, token],
            )
            .expect("seed bot");
    }

    async fn body_json(resp: axum::response::Response) -> Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn overview_lists_system_default_and_owned() {
        let (db, path) = fresh_db();
        seed_user(&path, "owner");
        seed_user(&path, "admin");
        seed_bot(&path, "system_default", "admin", None);
        seed_bot(&path, "b1", "owner", Some("ENC"));
        let app = app_for(db, "owner");

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/integrated/overview")
                    .header("cookie", "__Host-yuuka-session=good")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp).await;
        assert_eq!(j["success"], Value::Bool(true));
        let bots = j["bots"].as_array().unwrap();
        // system_default（health のみ・is_system_default:true）＋所有 b1。
        assert_eq!(bots.len(), 2);
        assert_eq!(bots[0]["id"], "system_default");
        assert_eq!(bots[0]["is_system_default"], Value::Bool(true));
        assert_eq!(bots[1]["id"], "b1");
        assert_eq!(bots[1]["is_system_default"], Value::Bool(false));
        // Null シーム → running/connected は false。
        assert_eq!(bots[1]["running"], Value::Bool(false));
        assert_eq!(bots[1]["connected"], Value::Bool(false));
        // b1 は has_token:true、preset は既定 capabilities（secretary）。
        assert_eq!(bots[1]["has_token"], Value::Bool(true));
        assert_eq!(bots[1]["preset"], "secretary");
        assert_eq!(bots[1]["google_setting"], "primary");
    }

    #[tokio::test]
    async fn start_forbidden_for_system_default() {
        let (db, path) = fresh_db();
        seed_user(&path, "owner");
        seed_user(&path, "admin");
        seed_bot(&path, "system_default", "admin", None);
        let app = app_for(db, "owner");

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/integrated/bots/start")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from("{\"botId\":\"system_default\"}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let j = body_json(resp).await;
        assert_eq!(j["message"], "このBotを操作する権限がありません。");
    }

    #[tokio::test]
    async fn start_degraded_502_but_clears_stopped() {
        let (db, path) = fresh_db();
        seed_user(&path, "owner");
        seed_bot(&path, "b1", "owner", Some("ENC"));
        // 事前に stopped=1 にしておく（DB 効果で解除されることを確認）。
        Connection::open(&path)
            .unwrap()
            .execute("UPDATE bots SET stopped = 1 WHERE id = 'b1'", [])
            .unwrap();
        let app = app_for(db, "owner");

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/integrated/bots/start")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from("{\"botId\":\"b1\"}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        // Null シーム start→false ⇒ 502・running/connected false（正直な縮退）。
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        let j = body_json(resp).await;
        assert_eq!(j["success"], Value::Bool(false));
        assert_eq!(
            j["message"],
            "起動に失敗しました（トークンを確認してください）。"
        );
        assert_eq!(j["running"], Value::Bool(false));
        // DB 効果は完全に働く: stopped は 0 に解除されている。
        let stopped: i64 = Connection::open(&path)
            .unwrap()
            .query_row("SELECT stopped FROM bots WHERE id = 'b1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stopped, 0);
    }

    #[tokio::test]
    async fn stop_persists_stopped_flag() {
        let (db, path) = fresh_db();
        seed_user(&path, "owner");
        seed_bot(&path, "b1", "owner", Some("ENC"));
        let app = app_for(db, "owner");

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/integrated/bots/stop")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from("{\"botId\":\"b1\"}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp).await;
        assert_eq!(j["message"], "停止しました。");
        let stopped: i64 = Connection::open(&path)
            .unwrap()
            .query_row("SELECT stopped FROM bots WHERE id = 'b1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stopped, 1);
    }

    #[tokio::test]
    async fn clear_history_and_grant_credential_404() {
        let (db, path) = fresh_db();
        seed_user(&path, "owner");
        seed_bot(&path, "b1", "owner", Some("ENC"));
        Connection::open(&path)
            .unwrap()
            .execute(
                "INSERT INTO message_logs (user_id, bot_id, role, content) \
                 VALUES ('owner','b1','user','hi')",
                [],
            )
            .unwrap();
        let app = app_for(db, "owner");

        // clear-history 200。
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/integrated/bots/clear-history")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from("{\"botId\":\"b1\"}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp).await;
        assert_eq!(
            j["message"],
            "会話履歴をクリアしました（次のメッセージから新しい会話になります。永続ログは保持されます）。"
        );

        // 秘書 floor が書かれている（b1 は既定 capabilities=secretary → 秘書キー）。
        let floor: Option<String> = Connection::open(&path)
            .unwrap()
            .query_row(
                "SELECT value FROM system_settings WHERE key = 'context_floor:b1:owner'",
                [],
                |r| r.get(0),
            )
            .ok();
        assert!(floor.is_some());

        // grants/credential: 存在しない service → 404。
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/integrated/grants/credential")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        "{\"botId\":\"b1\",\"serviceName\":\"ghost\",\"granted\":true}",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let j = body_json(resp).await;
        assert_eq!(j["message"], "対象の認証情報が見つかりません。");
    }
}
