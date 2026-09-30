//! yuuka-supervisor（lib+bin）— アプリ組立とライフサイクル。
//!
//! フレームワークルート（yuuka-web）と各ドメイン（T1）のルータを merge し、任意で SPA 静的
//! 配信を載せ、共通レイヤ（CSRF/body 上限/セキュリティヘッダ）を被せて完成ルータを返す。
//! bin（main.rs）が config 読込・実 AuthBackend（yuuka-auth）・`axum::serve`・graceful shutdown を
//! 配線する。JoinSet による全サービス監督（bot/gemini/services）は後続増分。
//! DAG: `supervisor → web, auth, todo, …`（ドメインを束ねる最下流）。

use std::path::Path;

use axum::Router;
use yuuka_web::{apply_common_layers, framework_routes, mount_pwa, mount_static, AppState};

pub mod desktop_dist;
pub mod discord;
pub mod services;
pub mod supervisor;
pub mod tenants;
pub mod tool_registry;
pub mod ws;

pub use discord::{DiscordTenantService, MessengerRegistrationDm};
pub use services::{build_supervised_services, CronSupervised};
pub use supervisor::{RestartPolicy, ServiceError, ShutdownToken, SupervisedService, Supervisor};
pub use tenants::{
    RegistryBotRuntime, RegistryBotViewRuntime, RegistryDiscordLive, RegistryLifecycle,
    TenantRegistry,
};
pub use tool_registry::{build_native_provider, build_tool_registry};
pub use ws::ws_routes;

/// 完成アプリのルータを組み立てる（フレームワーク + 認証発行 + 会話 WS + 全ドメイン + 任意の静的配信 + 共通レイヤ）。
///
/// 新ドメイン（finance/schedule/…）は `.merge(yuuka_xxx::routes())` を足す。`auth_routes` は
/// [`yuuka_auth::routes`] が返す認証発行ルータ、`ws_routes` は [`ws::ws_routes`] が返す `/ws/chat`
/// ルータ（それぞれ `Extension` で依存を内包済み）。web の再起動毎に呼ばれるので、呼び出し側で 1 度
/// 組んだものを clone して渡す（`Router` は安価に clone 可能）。`dist_dir` を渡すと SPA
/// （`dist/public`）を `fallback_service` として載せる（未指定は API のみ）。
///
/// 引数は state + dist_dir/pwa_dist_dir と、`Extension` で依存を内包済みの pre-built ルータ群
/// （auth/admin/settings/webhook/bot-attribute/ws/client-api）。crypto/runtime を State へ入れると全
/// `AppState` 構築へ波及するため、これらは main 側で組んで渡す配線関数（引数数の lint は本質的な
/// 配線都合として許容）。
///
/// `client_api_routes` は [`yuuka_client_api::routes_with`] が返す `/api/client/*` ルータ（`ChatEngine`/
/// `RateLimiter` を `Extension` で内包済み・チャット送信 issue #41）。
///
/// `pwa_dist_dir` を渡すと PWA（issue #33・`client/pwa`）を [`mount_pwa`] で明示ルート群として
/// 載せる。`dist_dir`（admin SPA・`fallback_service`）とは独立に効くため、両方 `Some` でも
/// 互いを横取りしない（[`yuuka_web::mount_pwa`] のドキュメント参照）。
#[allow(clippy::too_many_arguments)]
pub fn build_app(
    state: AppState,
    auth_routes: Router<AppState>,
    admin_routes: Router<AppState>,
    settings_routes: Router<AppState>,
    webhook_routes: Router<AppState>,
    bot_attribute_routes: Router<AppState>,
    credential_routes: Router<AppState>,
    device_auth_routes: Router<AppState>,
    ws_routes: Router<AppState>,
    mcp_routes: Router<AppState>,
    integrated_routes: Router<AppState>,
    finance_routes: Router<AppState>,
    bot_management_routes: Router<AppState>,
    client_api_routes: Router<AppState>,
    dist_dir: Option<&Path>,
    pwa_dist_dir: Option<&Path>,
) -> Router {
    let routes = framework_routes()
        .merge(auth_routes)
        .merge(admin_routes)
        .merge(settings_routes)
        .merge(webhook_routes)
        .merge(bot_attribute_routes)
        .merge(credential_routes)
        .merge(device_auth_routes)
        .merge(ws_routes)
        .merge(mcp_routes)
        .merge(integrated_routes)
        .merge(finance_routes)
        .merge(yuuka_todo::routes())
        .merge(yuuka_schedule::routes())
        .merge(yuuka_timeline::routes())
        .merge(yuuka_reminder::routes())
        .merge(yuuka_personal::routes())
        .merge(yuuka_playbook::routes())
        .merge(yuuka_persona::routes())
        .merge(yuuka_briefing::routes())
        .merge(yuuka_auth::device_routes())
        .merge(yuuka_orchestrator::member_request_routes())
        .merge(yuuka_orchestrator::bot_share_routes())
        .merge(client_api_routes)
        .merge(bot_management_routes)
        .merge(desktop_dist::routes());
    let routes = match pwa_dist_dir {
        Some(dir) => mount_pwa(routes, dir),
        None => routes,
    };
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
        build_test_app(None, None)
    }

    /// [`app`] のパラメータ化版。`dist_dir`/`pwa_dist_dir` を渡すと [`super::build_app`] が実サーバ
    /// （`main.rs`）と全く同じ組み立て（PWA → admin static の順で mount）を行う。組み合わせルータの
    /// 検証（[`combined_production_router_wires_pwa_admin_and_api_without_route_conflicts`]）に使う。
    fn build_test_app(
        dist_dir: Option<&std::path::Path>,
        pwa_dist_dir: Option<&std::path::Path>,
    ) -> Router {
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
        // 設定ルータ（in-memory セッション・NullBotRuntime・暗号なし・Google は Null シーム）を組んで
        // merge を検証する。
        let settings_runtime = std::sync::Arc::new(yuuka_settings::SettingsRuntime::new(
            yuuka_auth::SessionStore::in_memory(),
            7,
            None,
            std::sync::Arc::new(yuuka_admin::NullBotRuntime),
            std::sync::Arc::new(yuuka_google::NullGoogleOAuth),
            std::sync::Arc::new(yuuka_google::NullCalendar),
            std::sync::Arc::new(yuuka_google::NullBackup),
            std::sync::Arc::new(yuuka_google::OAuthStateStore::new()),
        ));
        // client-api ルータ（`/api/client/*`・issue #41 のチャット送信を含む）は `Extension` で
        // `ChatEngine`/`RateLimiter` を内包するため、WS と違って完全に空のルータでは代替できない
        // （route 自体を登録しないと `combined_production_router_wires_pwa_admin_and_api_without_route_conflicts`
        // が検証したい「認証切れ→401」を「未登録→404」に取り違えてしまう）。`RealGeminiFactory` は
        // ここでは `.build()` まで到達しない（`AuthenticatedUser` 抽出が先に 401 で短絡する）ため、
        // 実 API キーなしで安全に使える。
        let db = test_db();
        let client_api_engine = std::sync::Arc::new(yuuka_orchestrator::ChatEngine::new(
            db.clone(),
            None,
            yuuka_tools::ToolRegistry::new(),
            std::sync::Arc::new(yuuka_orchestrator::RealGeminiFactory),
            None,
            std::sync::Arc::new(yuuka_mcp::NullMcpClient),
            None,
        ));
        let client_api_rate_limiter: std::sync::Arc<dyn yuuka_discord::RateLimiter> =
            std::sync::Arc::new(yuuka_orchestrator::InMemoryRateLimiter::new(db.clone()));
        super::build_app(
            AppState::new(Arc::new(FakeAuth), WebConfig::default(), db),
            yuuka_auth::routes(runtime),
            yuuka_admin::routes(admin_runtime),
            yuuka_settings::routes(settings_runtime),
            // Webhook ルータ（Null プロセッサ・暗号なし）を merge して検証する。
            yuuka_webhook::routes(),
            // Bot 属性ルータ（暗号なし）を merge して検証する。
            yuuka_orchestrator::bot_attribute_routes(),
            // credential ルータ（暗号なし）を merge して検証する。
            yuuka_credential::routes(),
            // デバイスフロー ルータ（空 store）を merge して検証する。
            yuuka_auth::device_auth_routes(yuuka_auth::DeviceAuthStore::new(String::new())),
            // WS ルータは merge 検証には不要（ChatEngine 構築を避け空ルータを渡す）。
            axum::Router::new(),
            // MCP ルータ（NullMcpClient・暗号なし・in-memory token）を merge して検証する。
            yuuka_mcp::routes(std::sync::Arc::new(yuuka_mcp::McpRuntime::new(
                None,
                std::sync::Arc::new(yuuka_mcp::NullMcpClient),
                std::sync::Arc::new(yuuka_mcp::ProxyTokenManager::new()),
            ))),
            // 統合設定ルータ（NullBotLifecycle・NullCalendar）を merge して検証する。
            yuuka_integrated::routes(std::sync::Arc::new(
                yuuka_integrated::IntegratedRuntime::new(
                    std::sync::Arc::new(yuuka_integrated::NullBotLifecycle),
                    std::sync::Arc::new(yuuka_google::NullCalendar),
                ),
            )),
            // finance ルータ（既定 NullReceiptParser）を merge して検証する。
            yuuka_finance::routes(),
            // Bot 管理ルータ（既定 NullBotViewRuntime・crypto なし）を merge して検証する。
            yuuka_orchestrator::bot_management_routes(),
            // client-api ルータ（fake ChatEngine・InMemoryRateLimiter）。上のコメント参照。
            yuuka_client_api::routes_with(client_api_engine, client_api_rate_limiter),
            dist_dir,
            pwa_dist_dir,
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

    /// PWA dist（index.html + manifest + sw.js + icon + ハッシュ資産）。
    /// `crates/yuuka-web/src/static_files.rs` の `pwa_tests::pwa_dist` と同じ形。
    fn pwa_test_dist() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::write(dir.path().join("index.html"), "<html>PWA-SHELL</html>").expect("index");
        std::fs::write(dir.path().join("manifest.webmanifest"), "{}").expect("manifest");
        std::fs::write(dir.path().join("sw.js"), "// sw").expect("sw");
        std::fs::create_dir_all(dir.path().join("icons")).expect("icons dir");
        std::fs::write(dir.path().join("icons/app-icon.svg"), "<svg/>").expect("icon");
        std::fs::create_dir_all(dir.path().join("assets")).expect("assets dir");
        std::fs::write(dir.path().join("assets/app-BIDAdKj3.js"), "console.log(1)")
            .expect("hashed");
        dir
    }

    /// 管理画面 SPA dist（index.html + 404.html + ハッシュ資産）。
    fn admin_test_dist() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::write(dir.path().join("index.html"), "<html>ADMIN-SHELL</html>").expect("index");
        std::fs::write(dir.path().join("404.html"), "<html>NOT-FOUND</html>").expect("404");
        std::fs::create_dir_all(dir.path().join("assets")).expect("assets dir");
        std::fs::write(
            dir.path().join("assets/admin-CAFEBEEF.js"),
            "console.log(2)",
        )
        .expect("hashed");
        dir
    }

    async fn combined_get(
        admin_dir: &std::path::Path,
        pwa_dir: &std::path::Path,
        uri: &str,
    ) -> axum::response::Response {
        // 実サーバ（main.rs）と全く同じ [`super::build_app`] 呼び出し（PWA → admin static の順で
        // mount）でルータを組み立てる。axum は `Router::route` で GET が重複登録された時点で panic
        // するため、本ヘルパー呼び出し自体が「重複ルートでビルドが壊れていないか」を毎回検証する。
        build_test_app(Some(admin_dir), Some(pwa_dir))
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    /// [`combined_get`] の POST 版（issue #41・`POST /api/client/chat/messages` の無認証 401 検証用）。
    async fn combined_post(
        admin_dir: &std::path::Path,
        pwa_dir: &std::path::Path,
        uri: &str,
        body: &str,
    ) -> axum::response::Response {
        build_test_app(Some(admin_dir), Some(pwa_dir))
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn combined_production_router_wires_pwa_admin_and_api_without_route_conflicts() {
        // PR #74 レビュー対応: #73 の暫定 `GET /` → `/admin/` リダイレクトが `mount_pwa` の `"/"`
        // シェルルートと衝突し、実サーバ（`build_app` が PWA→admin static の順で mount）は
        // 起動時に panic していた。本テストは `build_app` をそのまま呼び出すため、同種の重複ルートが
        // 再発すればここで（起動を待たずに）検知できる。
        let pwa_dir = pwa_test_dist();
        let admin_dir = admin_test_dist();

        // GET / → PWA シェル（200・no-cache）。
        let root = combined_get(admin_dir.path(), pwa_dir.path(), "/").await;
        assert_eq!(root.status(), StatusCode::OK);
        assert_eq!(
            root.headers()
                .get("cache-control")
                .and_then(|v| v.to_str().ok()),
            Some("no-cache, no-store, must-revalidate")
        );
        let root_body = axum::body::to_bytes(root.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&root_body).contains("PWA-SHELL"));

        // /admin/<deep-link> → 管理画面 SPA index（200・拡張子なし deep link フォールバック）。
        let admin_deep = combined_get(admin_dir.path(), pwa_dir.path(), "/admin/bots/123").await;
        assert_eq!(admin_deep.status(), StatusCode::OK);
        let admin_body = axum::body::to_bytes(admin_deep.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&admin_body).contains("ADMIN-SHELL"));

        // PWA のハッシュ資産 → immutable。
        let pwa_asset =
            combined_get(admin_dir.path(), pwa_dir.path(), "/assets/app-BIDAdKj3.js").await;
        assert_eq!(pwa_asset.status(), StatusCode::OK);
        assert_eq!(
            pwa_asset
                .headers()
                .get("cache-control")
                .and_then(|v| v.to_str().ok()),
            Some("public, max-age=31536000, immutable")
        );

        // 管理画面のハッシュ資産 → immutable。
        let admin_asset = combined_get(
            admin_dir.path(),
            pwa_dir.path(),
            "/admin/assets/admin-CAFEBEEF.js",
        )
        .await;
        assert_eq!(admin_asset.status(), StatusCode::OK);
        assert_eq!(
            admin_asset
                .headers()
                .get("cache-control")
                .and_then(|v| v.to_str().ok()),
            Some("public, max-age=31536000, immutable")
        );

        // /api/client/* は無認証 → 401（PWA の API・yuuka-client-api）。
        let client_status =
            combined_get(admin_dir.path(), pwa_dir.path(), "/api/client/status").await;
        assert_eq!(client_status.status(), StatusCode::UNAUTHORIZED);

        // PR #75 レビュー対応: POST /api/client/chat/messages（issue #41・チャット送信）も
        // client_api_routes として組み合わせルータへ登録されていることを検証する。無認証 → 401
        // （`ChatEngine`/`RateLimiter` の `Extension` 配線がここに来るまでに一切呼ばれないことも
        // 同時に確認できる ── 呼ばれていれば `RealGeminiFactory` が実 API キー無しで失敗する）。
        let chat_send = combined_post(
            admin_dir.path(),
            pwa_dir.path(),
            "/api/client/chat/messages",
            r#"{"content":"hello"}"#,
        )
        .await;
        assert_eq!(chat_send.status(), StatusCode::UNAUTHORIZED);

        // 未登録 /api/* は JSON 404（index.html に握り潰されない・B6 parity）。
        let unknown_api =
            combined_get(admin_dir.path(), pwa_dir.path(), "/api/does-not-exist").await;
        assert_eq!(unknown_api.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            unknown_api
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some("application/json")
        );
    }
}
