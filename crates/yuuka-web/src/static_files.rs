//! SPA（`dist/public`）静的配信（§decisions: `ServeDir` precompressed + SPA フォールバック）。
//!
//! - `/assets/*`（Vite のハッシュ付き資産）は **immutable 長期キャッシュ**。
//! - それ以外の未知 GET は **index.html** にフォールバック（クライアントルーティング）。
//! - `.br`/`.gz` サイドカーがあれば precompressed を優先配信する。
//!
//! API ルートは明示登録されており本サービスは `fallback_service`＝最後に評価されるため、
//! `/api/*` を食い潰さない。共通レイヤ（nosniff/frame-options 等）は静的応答にも被さる。
//!
//! index.html への google-site-verification meta 注入は後続増分（SEO 用途・機能非依存）。

use std::path::Path;

use axum::http::header::{HeaderValue, CACHE_CONTROL};
use axum::Router;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

/// ハッシュ付き資産の Cache-Control（1 年・immutable）。
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// `dist_dir`（例 `dist/public`）を SPA として `router` に載せる。
///
/// `/assets` はハッシュ付きなので immutable、その他は SPA フォールバックで index.html。
pub fn mount_static<S>(router: Router<S>, dist_dir: &Path) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let index = dist_dir.join("index.html");

    // ハッシュ付き資産（/assets）: immutable 長期キャッシュ。
    let assets = ServeDir::new(dist_dir.join("assets"))
        .precompressed_br()
        .precompressed_gzip();

    // それ以外: dist ルートを配信し、未知パスは index.html（SPA ルーティング）。
    let spa = ServeDir::new(dist_dir)
        .precompressed_br()
        .precompressed_gzip()
        .fallback(ServeFile::new(index));

    router
        .nest_service(
            "/assets",
            tower::ServiceBuilder::new()
                .layer(SetResponseHeaderLayer::overriding(
                    CACHE_CONTROL,
                    HeaderValue::from_static(IMMUTABLE),
                ))
                .service(assets),
        )
        .fallback_service(spa)
}

#[cfg(test)]
mod tests {
    use super::mount_static;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use std::fs;
    use tower::ServiceExt;

    fn dist_with(index: &str, asset: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tmp");
        fs::write(dir.path().join("index.html"), index).expect("index");
        if let Some(a) = asset {
            fs::create_dir_all(dir.path().join("assets")).expect("assets dir");
            fs::write(dir.path().join("assets/app-abc123.js"), a).expect("asset");
        }
        dir
    }

    fn app(dir: &std::path::Path) -> Router {
        mount_static(
            Router::new().route("/api/me", get(|| async { "me" })),
            dir,
        )
    }

    #[tokio::test]
    async fn serves_hashed_asset_with_immutable_cache() {
        let dir = dist_with("<html>root</html>", Some("console.log(1)"));
        let resp = app(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/assets/app-abc123.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("cache-control").map(|v| v.as_bytes()),
            Some(&b"public, max-age=31536000, immutable"[..])
        );
    }

    #[tokio::test]
    async fn unknown_path_falls_back_to_index_html() {
        // クライアントルーティング用パスは index.html を返す（SPA）。
        let dir = dist_with("<html>SPA-SHELL</html>", None);
        let resp = app(dir.path())
            .oneshot(Request::builder().uri("/dashboard/tasks").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("SPA-SHELL"));
    }

    #[tokio::test]
    async fn api_route_is_not_shadowed_by_static() {
        let dir = dist_with("<html>root</html>", None);
        let resp = app(dir.path())
            .oneshot(Request::builder().uri("/api/me").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&bytes[..], b"me");
    }
}
