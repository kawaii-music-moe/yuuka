//! yuuka-web — axum Web 層（ルート・型付き認可 extractor・静的配信・WS）。
//!
//! Phase 1 増分1: **認証フレームワーク + `/api/me`**。
//! - 認可レベル（none/user/admin）を型（[`AuthenticatedUser`]/[`AdminUser`]/[`OptionalUser`]）で強制。
//! - 認証バックエンドは [`AuthBackend`] トレイト越し（Cookie=Redis / Bearer=SQLite の実装は後続増分）。
//! - エラーは [`ApiError`]（`WebError` を Node 互換 `{success:false,message}` へ写像）。
//!
//! 後続増分: `/api/setup/status`・`/api/status`・静的配信（SPA/immutable）・Redis/SQLite 認証実装・
//! CSRF/ボディ上限レイヤ・WS。DAG: `web → core, types`。

pub mod auth;
pub mod config;
pub mod csrf;
pub mod error;
pub mod extract;
pub mod routes;
pub mod scope;
pub mod state;
pub mod static_files;

pub use auth::{AdminUser, AuthBackend, AuthenticatedUser, OptionalUser};
pub use config::WebConfig;
pub use error::ApiError;
pub use extract::ScopedJson;
pub use scope::{has_bot_access, resolve_scope};
pub use state::{AppState, Db};
pub use static_files::mount_static;

use axum::extract::DefaultBodyLimit;
use axum::http::header::{
    HeaderValue, CONTENT_SECURITY_POLICY, REFERRER_POLICY, STRICT_TRANSPORT_SECURITY,
    X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use axum::routing::get;
use axum::Router;
use tower_http::set_header::SetResponseHeaderLayer;

/// リクエストボディ上限（Node `MAX_BODY_BYTES` = 10MB・超過は 413）。
pub const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

/// Content-Security-Policy（Node `server.ts` の `CSP` と一字一句一致・PLAN §6.6）。
///
/// `script-src` から `'unsafe-inline'` を除外した実効的 XSS 多層防御。`style-src` の
/// `'unsafe-inline'` はテンプレート内 `style=` のため維持（Node 踏襲）。インライン JS を
/// 足す場合は nonce/hash 方式へ移行すること。
const CSP: &str = "default-src 'self'; script-src 'self' https://static.cloudflareinsights.com; \
style-src 'self' 'unsafe-inline' https://fonts.googleapis.com https://fonts.gstatic.com; \
font-src 'self' https://fonts.gstatic.com https://fonts.googleapis.com; \
img-src 'self' data: https://assets-global.website-files.com https://cdn.discordapp.com; \
connect-src 'self' https://cloudflareinsights.com; worker-src 'self'; frame-src 'self'; \
frame-ancestors 'self';";

/// 全ルートに共通のミドルウェア（CSRF・body 上限・セキュリティヘッダ）を積む。
///
/// ドメインルート（T1）はこのレイヤ群の下にマージされる。全 API 応答に Node 互換の
/// `X-Content-Type-Options: nosniff` / `X-Frame-Options: SAMEORIGIN` を付与し、
/// 状態変更 × Cookie 認証には CSRF ガードを適用する。
/// `https` が真（HTTPS 本番＝`config.baseUrl` が `https://`）の時のみ HSTS を付与する
/// （Node `server.ts` の SSL ストリッピング対策・全応答へ `Strict-Transport-Security`）。
pub fn apply_common_layers<S>(router: Router<S>, https: bool) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let router = router
        .layer(axum::middleware::from_fn(csrf::csrf_guard))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(SetResponseHeaderLayer::overriding(
            X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            X_FRAME_OPTIONS,
            // Node の API 応答は SAMEORIGIN（同一オリジンの iframe は許可）。parity 維持。
            HeaderValue::from_static("SAMEORIGIN"),
        ))
        // Node `SECURITY_HEADERS` の残り 2 件（H-2）。SPA を Rust が配信し始める前に必須。
        .layer(SetResponseHeaderLayer::overriding(
            REFERRER_POLICY,
            HeaderValue::from_static("strict-origin-when-cross-origin"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CSP),
        ));
    // HSTS: HTTPS 本番のみ。2 年・サブドメイン込み（preload は運用判断で含めない）。
    if https {
        router.layer(SetResponseHeaderLayer::overriding(
            STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=63072000; includeSubDomains"),
        ))
    } else {
        router
    }
}

/// フレームワーク自身のルート（認証・ヘルス等。ドメインルートは含まない）。
///
/// supervisor はこれに各ドメインの `routes()` を `merge` し、[`apply_common_layers`] を
/// 被せてから `with_state` する。
pub fn framework_routes() -> Router<AppState> {
    Router::new().route("/api/me", get(routes::me))
}

/// アプリのルータを構築する（Phase 1 増分2 時点は `/api/me` + 共通レイヤ）。
pub fn build_router(state: AppState) -> Router {
    let https = state.config.https;
    apply_common_layers(framework_routes(), https).with_state(state)
}

#[cfg(test)]
mod tests {
    use super::{AdminUser, AppState, AuthBackend, Db, WebConfig};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::response::Response;
    use axum::Router;
    use tower::ServiceExt;
    use yuuka_core::AuthError;
    use yuuka_types::{Role, SessionUser};

    static TEST_DB_SEQ: AtomicU64 = AtomicU64::new(0);

    /// テスト用の一時 DB を用意する（本番の open は CREATE しないため先に seed する）。
    fn test_db() -> Db {
        let seq = TEST_DB_SEQ.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("yuuka_web_test_{}_{seq}.sqlite", std::process::id()));
        {
            rusqlite::Connection::open(&path).expect("seed db");
        }
        Db::open(&path).expect("open db")
    }

    /// テスト用のインメモリ認証バックエンド（Cookie トークン一致で固定ユーザーを返す）。
    struct FakeAuth {
        cookie_token: String,
        user: SessionUser,
    }

    #[async_trait]
    impl AuthBackend for FakeAuth {
        async fn session_user(&self, token: &str) -> Result<Option<SessionUser>, AuthError> {
            Ok((token == self.cookie_token).then(|| self.user.clone()))
        }
        async fn desktop_user(&self, _token: &str) -> Result<Option<SessionUser>, AuthError> {
            Ok(None)
        }
    }

    fn session_user(role: Role) -> SessionUser {
        SessionUser {
            discord_id: "123".to_owned(),
            username: "yuu".to_owned(),
            role,
        }
    }

    fn app_with(role: Role) -> Router {
        let auth = Arc::new(FakeAuth {
            cookie_token: "good-token".to_owned(),
            user: session_user(role),
        });
        let config = WebConfig {
            terms_url: "https://example.test/terms".to_owned(),
            ..WebConfig::default()
        };
        super::build_router(AppState::new(auth, config, test_db()))
    }

    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("read body");
        serde_json::from_slice(&bytes).expect("parse json")
    }

    #[tokio::test]
    async fn me_requires_authentication() {
        let resp = app_with(Role::User)
            .oneshot(
                Request::builder()
                    .uri("/api/me")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.headers().get("x-content-type-options").map(|v| v.as_bytes()),
            Some(&b"nosniff"[..])
        );
        let j = body_json(resp).await;
        assert_eq!(j["success"], serde_json::json!(false));
        assert!(j["message"].is_string());
    }

    #[tokio::test]
    async fn me_returns_user_with_valid_cookie() {
        let resp = app_with(Role::User)
            .oneshot(
                Request::builder()
                    .uri("/api/me")
                    .header("cookie", "__Host-yuuka-session=good-token")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp).await;
        assert_eq!(j["success"], serde_json::json!(true));
        assert_eq!(j["user"]["discordId"], serde_json::json!("123"));
        assert_eq!(j["user"]["role"], serde_json::json!("user"));
        assert_eq!(j["termsUrl"], serde_json::json!("https://example.test/terms"));
    }

    #[tokio::test]
    async fn https_prod_requires_host_prefixed_cookie() {
        // HTTPS 本番（config.https=true）は `__Host-` のみ受理し、非 prefix cookie は拒否する。
        let auth = Arc::new(FakeAuth {
            cookie_token: "good-token".to_owned(),
            user: session_user(Role::User),
        });
        let config = WebConfig {
            https: true,
            ..WebConfig::default()
        };
        let app = super::build_router(AppState::new(auth, config, test_db()));

        // 非 __Host- cookie → 拒否（401）。
        let rejected = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/me")
                    .header("cookie", "yuuka-session=good-token")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);

        // __Host- cookie → 受理（200）。
        let accepted = app
            .oneshot(
                Request::builder()
                    .uri("/api/me")
                    .header("cookie", "__Host-yuuka-session=good-token")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(accepted.status(), StatusCode::OK);
    }

    fn csrf_app() -> Router {
        // state 不要（csrf_guard は状態を使わない）。POST ルートで CSRF を検証する。
        super::apply_common_layers(
            Router::new().route("/x", axum::routing::post(|| async { "ok" })),
            false,
        )
    }

    async fn csrf_status(headers: &[(&str, &str)]) -> StatusCode {
        let mut builder = Request::builder().method("POST").uri("/x");
        for (k, v) in headers {
            builder = builder.header(*k, *v);
        }
        csrf_app()
            .oneshot(builder.body(Body::empty()).expect("request"))
            .await
            .expect("response")
            .status()
    }

    #[tokio::test]
    async fn csrf_blocks_cross_site_cookie_post() {
        // Cookie 認証 × POST × cross-site → 403。
        assert_eq!(
            csrf_status(&[
                ("cookie", "__Host-yuuka-session=t"),
                ("sec-fetch-site", "cross-site"),
            ])
            .await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn csrf_allows_same_origin_and_missing_and_bearer() {
        // same-origin は許可。
        assert_eq!(
            csrf_status(&[
                ("cookie", "__Host-yuuka-session=t"),
                ("sec-fetch-site", "same-origin"),
            ])
            .await,
            StatusCode::OK
        );
        // Origin/Referer/Sec-Fetch-Site 欠落は SameSite=Lax に委ね許可。
        assert_eq!(
            csrf_status(&[("cookie", "__Host-yuuka-session=t")]).await,
            StatusCode::OK
        );
        // Origin hostname 不一致 → 403。
        assert_eq!(
            csrf_status(&[
                ("cookie", "__Host-yuuka-session=t"),
                ("host", "yuuka.example"),
                ("origin", "https://evil.example"),
            ])
            .await,
            StatusCode::FORBIDDEN
        );
        // Origin hostname 一致 → 許可。
        assert_eq!(
            csrf_status(&[
                ("cookie", "__Host-yuuka-session=t"),
                ("host", "yuuka.example"),
                ("origin", "https://yuuka.example"),
            ])
            .await,
            StatusCode::OK
        );
        // Bearer のみ（非 ambient）は cross-site でも対象外 → 許可。
        assert_eq!(
            csrf_status(&[
                ("authorization", "Bearer tok"),
                ("sec-fetch-site", "cross-site"),
            ])
            .await,
            StatusCode::OK
        );
        // Cookie 無し（webhook 等）は対象外 → 許可。
        assert_eq!(
            csrf_status(&[("sec-fetch-site", "cross-site")]).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn admin_extractor_forbids_non_admin() {
        // AdminUser を使う一時ルートで、user ロールが 403 になることを確認。
        let auth = Arc::new(FakeAuth {
            cookie_token: "good-token".to_owned(),
            user: session_user(Role::User),
        });
        let state = AppState::new(auth, WebConfig::default(), test_db());
        let app = Router::new()
            .route("/admin", axum::routing::get(|_: AdminUser| async { "ok" }))
            .with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/admin")
                    .header("cookie", "__Host-yuuka-session=good-token")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    fn header_app(https: bool) -> Router {
        super::apply_common_layers(
            Router::new().route("/x", axum::routing::get(|| async { "ok" })),
            https,
        )
    }

    async fn headers_of(app: Router) -> axum::http::HeaderMap {
        app.oneshot(
            Request::builder()
                .uri("/x")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response")
        .headers()
        .clone()
    }

    #[tokio::test]
    async fn security_headers_present_on_all_responses() {
        // H-2: CSP / Referrer-Policy を nosniff / X-Frame と併せて全応答へ付与する。
        let h = headers_of(header_app(false)).await;
        assert_eq!(
            h.get("content-security-policy").map(|v| v.as_bytes()),
            Some(super::CSP.as_bytes())
        );
        // CSP は script-src から unsafe-inline を除外している（XSS 多層防御の核）。
        assert!(super::CSP.contains("script-src 'self' https://static.cloudflareinsights.com;"));
        assert!(!super::CSP.contains("script-src 'self' 'unsafe-inline'"));
        assert_eq!(
            h.get("referrer-policy").map(|v| v.as_bytes()),
            Some(&b"strict-origin-when-cross-origin"[..])
        );
        assert_eq!(
            h.get("x-content-type-options").map(|v| v.as_bytes()),
            Some(&b"nosniff"[..])
        );
        assert_eq!(
            h.get("x-frame-options").map(|v| v.as_bytes()),
            Some(&b"SAMEORIGIN"[..])
        );
        // https=false（開発/移行期の非 TLS）では HSTS を付けない。
        assert!(h.get("strict-transport-security").is_none());
    }

    #[tokio::test]
    async fn hsts_only_on_https_deployment() {
        // HSTS は HTTPS 本番（config.https=true）でのみ付与する（Node parity）。
        let h = headers_of(header_app(true)).await;
        assert_eq!(
            h.get("strict-transport-security").map(|v| v.as_bytes()),
            Some(&b"max-age=63072000; includeSubDomains"[..])
        );
    }
}
