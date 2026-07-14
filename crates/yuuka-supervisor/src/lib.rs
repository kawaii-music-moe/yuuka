//! yuuka-supervisor（lib+bin）— アプリ組立とライフサイクル。
//!
//! フレームワークルート（yuuka-web）と各ドメイン（T1）のルータを merge し、任意で SPA 静的
//! 配信を載せ、共通レイヤ（CSRF/body 上限/セキュリティヘッダ）を被せて完成ルータを返す。
//! bin（main.rs）が config 読込・実 AuthBackend（yuuka-auth）・`axum::serve`・graceful shutdown を
//! 配線する。JoinSet による全サービス監督（bot/gemini/services）は後続増分。
//! DAG: `supervisor → web, auth, todo, …`（ドメインを束ねる最下流）。

use std::path::Path;

use axum::Router;
use yuuka_web::{apply_common_layers, framework_routes, mount_static, AppState};

pub mod desktop_dist;
pub mod discord;
pub mod services;
pub mod supervisor;
pub mod tool_registry;
pub mod ws;

pub use discord::{DiscordTenantService, MessengerRegistrationDm};
pub use services::{build_supervised_services, CronSupervised};
pub use supervisor::{RestartPolicy, ServiceError, ShutdownToken, SupervisedService, Supervisor};
pub use tool_registry::{build_native_provider, build_tool_registry};
pub use ws::ws_routes;

/// 完成アプリのルータを組み立てる（フレームワーク + 認証発行 + 会話 WS + 全ドメイン + 任意の静的配信 + 共通レイヤ）。
///
/// 新ドメイン（finance/schedule/…）は `.merge(yuuka_xxx::routes())` を足す。`auth_routes` は
/// [`yuuka_auth::routes`] が返す認証発行ルータ、`ws_routes` は [`ws::ws_routes`] が返す `/ws/chat`
/// ルータ（それぞれ `Extension` で依存を内包済み）。web の再起動毎に呼ばれるので、呼び出し側で 1 度
/// 組んだものを clone して渡す（`Router` は安価に clone 可能）。`dist_dir` を渡すと SPA
/// （`dist/public`）を `fallback_service` として載せる（未指定は API のみ）。
pub fn build_app(
    state: AppState,
    auth_routes: Router<AppState>,
    admin_routes: Router<AppState>,
    settings_routes: Router<AppState>,
    webhook_routes: Router<AppState>,
    ws_routes: Router<AppState>,
    dist_dir: Option<&Path>,
) -> Router {
    let routes = framework_routes()
        .merge(auth_routes)
        .merge(admin_routes)
        .merge(settings_routes)
        .merge(webhook_routes)
        .merge(ws_routes)
        .merge(yuuka_todo::routes())
        .merge(yuuka_finance::routes())
        .merge(yuuka_schedule::routes())
        .merge(yuuka_timeline::routes())
        .merge(yuuka_reminder::routes())
        .merge(yuuka_personal::routes())
        .merge(yuuka_credential::routes())
        .merge(yuuka_playbook::routes())
        .merge(yuuka_persona::routes())
        .merge(yuuka_briefing::routes())
        .merge(yuuka_auth::device_routes())
        .merge(yuuka_orchestrator::member_request_routes())
        .merge(yuuka_orchestrator::bot_share_routes())
        .merge(yuuka_orchestrator::bot_management_routes())
        .merge(yuuka_orchestrator::bot_attribute_routes())
        .merge(desktop_dist::routes());
    let routes = match dist_dir {
        Some(dir) => mount_static(routes, dir),
        None => routes,
    };
    let https = state.config.https;
    let allowed_host = state.config.allowed_host.clone();
    apply_common_layers(routes, https, allowed_host).with_state(state)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::Router;
    use tower::ServiceExt;
    use yuuka_core::AuthError;
    use yuuka_types::{Role, SessionUser};
    use yuuka_web::{AppState, AuthBackend, Db, WebConfig};

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

    static TEST_DB_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn test_db() -> Db {
        let seq = TEST_DB_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_sup_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        {
            let conn = rusqlite::Connection::open(&path).expect("seed");
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS todos (id INTEGER PRIMARY KEY AUTOINCREMENT, \
                 user_id TEXT NOT NULL, bot_id TEXT NOT NULL DEFAULT 'system_default', \
                 title TEXT NOT NULL, description TEXT, due_date TEXT, start_date TEXT, \
                 priority TEXT, tags TEXT NOT NULL DEFAULT '[]', status TEXT NOT NULL DEFAULT 'open', \
                 progress INTEGER NOT NULL DEFAULT 0, parent_id INTEGER, linked_payment_id INTEGER, \
                 due_reminded INTEGER NOT NULL DEFAULT 0, repeat_rule TEXT, repeat_until TEXT, \
                 repeat_count INTEGER, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);",
            )
            .expect("ddl");
        }
        let db = Db::open(&path).expect("open"); // migrations で users 表を作成。
                                                 // /api/me は DB からユーザーを再取得する（P3-5）。FakeAuth のユーザー "u" を seed する
                                                 // （V17 実スキーマの NOT NULL: username/password_hash/salt を充足）。
        {
            let conn = rusqlite::Connection::open(&path).expect("open users");
            conn.execute(
                "INSERT OR REPLACE INTO users (discord_id, username, password_hash, salt, role) \
                 VALUES ('u', 'u', 'x', '00', 'user')",
                [],
            )
            .expect("seed user");
        }
        db
    }

    fn app() -> Router {
        // 認証発行ルータ（in-memory セッション・DM 未配線・暗号なし）を組んで merge を検証する。
        let runtime = std::sync::Arc::new(yuuka_auth::AuthRuntime::new(
            yuuka_auth::SessionStore::in_memory(),
            7,
            None,
            std::sync::Arc::new(yuuka_auth::NullRegistrationDm),
            Vec::new(),
        ));
        // 管理ルータ（in-memory セッション・NullBotRuntime・暗号なし）を組んで merge を検証する。
        let admin_runtime = std::sync::Arc::new(yuuka_admin::AdminRuntime::new(
            yuuka_auth::SessionStore::in_memory(),
            None,
            std::sync::Arc::new(yuuka_admin::NullBotRuntime),
            String::new(),
            String::new(),
        ));
        // 設定ルータ（in-memory セッション・NullBotRuntime・暗号なし）を組んで merge を検証する。
        let settings_runtime = std::sync::Arc::new(yuuka_settings::SettingsRuntime::new(
            yuuka_auth::SessionStore::in_memory(),
            7,
            None,
            std::sync::Arc::new(yuuka_admin::NullBotRuntime),
        ));
        super::build_app(
            AppState::new(Arc::new(FakeAuth), WebConfig::default(), test_db()),
            yuuka_auth::routes(runtime),
            yuuka_admin::routes(admin_runtime),
            yuuka_settings::routes(settings_runtime),
            // Webhook ルータ（Null プロセッサ・暗号なし）を merge して検証する。
            yuuka_webhook::routes(),
            // WS ルータは merge 検証には不要（ChatEngine 構築を避け空ルータを渡す）。
            axum::Router::new(),
            None,
        )
    }

    #[tokio::test]
    async fn merged_app_serves_framework_and_domain_routes() {
        // framework route /api/me（web）。
        let me = app()
            .oneshot(
                Request::builder()
                    .uri("/api/me")
                    .header("cookie", "__Host-yuuka-session=good")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(me.status(), StatusCode::OK);

        // domain route /api/tasks（todo）— 認証必須が効く。
        let tasks_unauth = app()
            .oneshot(
                Request::builder()
                    .uri("/api/tasks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(tasks_unauth.status(), StatusCode::UNAUTHORIZED);

        let tasks = app()
            .oneshot(
                Request::builder()
                    .uri("/api/tasks")
                    .header("cookie", "__Host-yuuka-session=good")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(tasks.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn csrf_layer_applies_to_domain_post() {
        // 共通レイヤ（CSRF）がドメイン POST にも被さる: Cookie×cross-site → 403。
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks/add")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("sec-fetch-site", "cross-site")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"title":"x"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
