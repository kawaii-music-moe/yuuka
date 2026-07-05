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
pub mod routes;
pub mod state;

pub use auth::{AdminUser, AuthBackend, AuthenticatedUser, OptionalUser};
pub use config::WebConfig;
pub use error::ApiError;
pub use state::AppState;

use axum::extract::DefaultBodyLimit;
use axum::http::header::{HeaderValue, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS};
use axum::routing::get;
use axum::Router;
use tower_http::set_header::SetResponseHeaderLayer;

/// リクエストボディ上限（Node `MAX_BODY_BYTES` = 10MB・超過は 413）。
pub const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

/// 全ルートに共通のミドルウェア（CSRF・body 上限・セキュリティヘッダ）を積む。
///
/// ドメインルート（T1）はこのレイヤ群の下にマージされる。全 API 応答に Node 互換の
/// `X-Content-Type-Options: nosniff` / `X-Frame-Options: SAMEORIGIN` を付与し、
/// 状態変更 × Cookie 認証には CSRF ガードを適用する。
pub fn apply_common_layers<S>(router: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router
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
}

/// アプリのルータを構築する（Phase 1 増分2 時点は `/api/me` + 共通レイヤ）。
pub fn build_router(state: AppState) -> Router {
    apply_common_layers(Router::new().route("/api/me", get(routes::me))).with_state(state)
}

#[cfg(test)]
mod tests {
    use super::{AdminUser, AppState, AuthBackend, WebConfig};
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::response::Response;
    use axum::Router;
    use tower::ServiceExt;
    use yuuka_core::AuthError;
    use yuuka_types::{Role, SessionUser};

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
        super::build_router(AppState::new(auth, config))
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
        let app = super::build_router(AppState::new(auth, config));

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
        let state = AppState::new(auth, WebConfig::default());
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
}
