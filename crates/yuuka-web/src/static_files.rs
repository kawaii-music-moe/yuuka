//! SPA（`dist/public`）静的配信 — Node `serveStaticFile`（`src/server.ts`）と挙動を一致させる。
//!
//! - **キャッシュ**（M-3 (a)(b)）: `assets/` 配下のハッシュ資産（`-{8文字以上}.{js|css|…}`）は
//!   `immutable` 長期キャッシュ、それ以外（index.html/manifest.json/sw.js/theme-init.js/404.html 等）
//!   は `no-cache, no-store, must-revalidate`。未存在（404）にも immutable を付けない。
//! - **未存在の分岐**（M-3 (c)）: **拡張子なし → index.html(200)**（SPA クライアントルーティング）、
//!   **拡張子あり → 404.html(404)**。`/sw.js` 等の欠落・削除を index.html で覆い隠さない
//!   （SW 更新・欠落検知を壊さない）。
//! - `.br`/`.gz` サイドカーがあれば `ServeDir` が precompressed を優先配信。
//! - パストラバーサルは `ServeDir` が正規化してルート外を弾く。
//!
//! セキュリティヘッダ（CSP/Referrer/HSTS/nosniff/X-Frame）は `apply_common_layers` が
//! 全応答へ付与する（本モジュールは `Cache-Control` のみ担当）。
//! index.html への google-site-verification meta 注入は後続増分（deferred・機能非依存）。

use std::convert::Infallible;
use std::path::Path;

use axum::body::Body;
use axum::extract::Request;
use axum::http::header::{HeaderValue, CACHE_CONTROL, CONTENT_TYPE};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Router;
use tower::{service_fn, ServiceExt};
use tower_http::services::ServeDir;

/// ハッシュ付き資産の Cache-Control（1 年・immutable）。
const IMMUTABLE: &str = "public, max-age=31536000, immutable";
/// 非ハッシュ資産・index.html・404・SPA フォールバックの Cache-Control（Node 一致）。
const NO_CACHE: &str = "no-cache, no-store, must-revalidate";

/// `dist_dir`（例 `dist/public`）を SPA として `router` の `fallback_service` に載せる。
///
/// API ルートは明示登録されており本サービスは最後に評価されるため `/api/*` を食い潰さない。
pub fn mount_static<S>(router: Router<S>, dist_dir: &Path) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let dist = dist_dir.to_path_buf();
    router.fallback_service(service_fn(move |req: Request| {
        let dist = dist.clone();
        async move { Ok::<Response, Infallible>(serve_static(&dist, req).await) }
    }))
}

/// Node `serveStaticFile` の写像: `ServeDir` で配信し、未存在は拡張子で分岐、最後に Cache-Control。
async fn serve_static(dist: &Path, req: Request) -> Response {
    let path = req.uri().path().to_owned();

    // ServeDir が実ファイル配信（precompress/MIME/range/traversal 防御）。エラー型は Infallible。
    let serve = ServeDir::new(dist).precompressed_br().precompressed_gzip();
    let served = match serve.oneshot(req).await {
        Ok(resp) => resp.into_response(),
        Err(never) => match never {},
    };

    // 未存在（404）の分岐: 拡張子なし → index.html(200・SPA)、拡張子あり → 404.html(404)。
    let mut resp = if served.status() == StatusCode::NOT_FOUND {
        if Path::new(&path).extension().is_some() {
            file_response(
                &dist.join("404.html"),
                StatusCode::NOT_FOUND,
                "404 Not Found",
            )
            .await
        } else {
            file_response(&dist.join("index.html"), StatusCode::OK, "").await
        }
    } else {
        served
    };

    // Cache-Control: 200 かつハッシュ資産のみ immutable、他（非ハッシュ 200・404 等）は no-cache。
    let cache = if resp.status() == StatusCode::OK && is_hashed_asset(&path) {
        IMMUTABLE
    } else {
        NO_CACHE
    };
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static(cache));
    resp
}

/// ファイルを読んで `text/html` 応答を作る（読めなければ `fallback` テキストを本文にする）。
async fn file_response(path: &Path, status: StatusCode, fallback: &str) -> Response {
    let body = match tokio::fs::read(path).await {
        Ok(bytes) => Body::from(bytes),
        Err(_) => Body::from(fallback.to_owned()),
    };
    let mut resp = Response::new(body);
    *resp.status_mut() = status;
    resp.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    resp
}

/// Node `isHashedAsset`（`server.ts`）の写像: `/assets/` 配下 かつ ファイル名末尾が
/// `-{8文字以上の [A-Za-z0-9_-]}.{js|css|woff|woff2|png|jpg|jpeg|svg|webp}`。
///
/// 「最初の `-`」起点で判定するのは Node 正規表現 `-[A-Za-z0-9_-]{8,}\.ext$` の貪欲マッチ
/// （charset に `-` を含む）と一致させるため。
fn is_hashed_asset(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/assets/") else {
        return false;
    };
    // 末尾セグメント（ファイル名）で判定（Node 正規表現は `$` アンカー）。
    let name = rest.rsplit('/').next().unwrap_or(rest);
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    if !matches!(
        ext,
        "js" | "css" | "woff" | "woff2" | "png" | "jpg" | "jpeg" | "svg" | "webp"
    ) {
        return false;
    }
    let Some(dash) = stem.find('-') else {
        return false;
    };
    let Some(hash) = stem.get(dash + 1..) else {
        return false;
    };
    hash.len() >= 8
        && hash
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
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

    /// index.html / 404.html / manifest.json ＋ ハッシュ資産 2 種（8 文字/6 文字）を持つ dist を作る。
    fn dist() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tmp");
        fs::write(dir.path().join("index.html"), "<html>SPA-SHELL</html>").expect("index");
        fs::write(dir.path().join("404.html"), "<html>NOT-FOUND</html>").expect("404");
        fs::write(dir.path().join("manifest.json"), "{}").expect("manifest");
        fs::create_dir_all(dir.path().join("assets")).expect("assets dir");
        // 実 Vite 相当の 8 文字ハッシュ。
        fs::write(dir.path().join("assets/app-BIDAdKj3.js"), "console.log(1)").expect("hashed");
        // 6 文字（Node 正規表現 {8,} に一致しない境界ケース）。
        fs::write(dir.path().join("assets/app-abc123.js"), "console.log(2)").expect("short");
        dir
    }

    fn app(dir: &std::path::Path) -> Router {
        mount_static(Router::new().route("/api/me", get(|| async { "me" })), dir)
    }

    async fn get_resp(dir: &std::path::Path, uri: &str) -> axum::response::Response {
        app(dir)
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    fn cache_control(resp: &axum::response::Response) -> Option<String> {
        resp.headers()
            .get("cache-control")
            .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
    }

    #[tokio::test]
    async fn hashed_asset_gets_immutable_cache() {
        let dir = dist();
        let resp = get_resp(dir.path(), "/assets/app-BIDAdKj3.js").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            cache_control(&resp).as_deref(),
            Some("public, max-age=31536000, immutable")
        );
    }

    #[tokio::test]
    async fn short_hash_asset_is_not_immutable() {
        // 6 文字ハッシュは Node 正規表現 {8,} に一致しない → no-cache（境界の凍結）。
        let dir = dist();
        let resp = get_resp(dir.path(), "/assets/app-abc123.js").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            cache_control(&resp).as_deref(),
            Some("no-cache, no-store, must-revalidate")
        );
    }

    #[tokio::test]
    async fn missing_asset_is_404_not_immutable() {
        // M-3(a): /assets の未存在は 404 で、immutable を付けない。
        let dir = dist();
        let resp = get_resp(dir.path(), "/assets/missing-BIDAdKj3.js").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            cache_control(&resp).as_deref(),
            Some("no-cache, no-store, must-revalidate")
        );
    }

    #[tokio::test]
    async fn index_html_and_manifest_are_no_cache() {
        // M-3(b): 非ハッシュ資産は no-cache（古い SPA シェル残留＝白画面の防止）。
        let dir = dist();
        for uri in ["/index.html", "/manifest.json", "/"] {
            let resp = get_resp(dir.path(), uri).await;
            assert_eq!(resp.status(), StatusCode::OK, "uri={uri}");
            assert_eq!(
                cache_control(&resp).as_deref(),
                Some("no-cache, no-store, must-revalidate"),
                "uri={uri}"
            );
        }
    }

    #[tokio::test]
    async fn extensionless_unknown_falls_back_to_index_html() {
        // 拡張子なしの未知パスは index.html を 200 で返す（SPA ルーティング）。
        let dir = dist();
        let resp = get_resp(dir.path(), "/dashboard/tasks").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            cache_control(&resp).as_deref(),
            Some("no-cache, no-store, must-revalidate")
        );
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("SPA-SHELL"));
    }

    #[tokio::test]
    async fn extension_unknown_returns_404_not_index() {
        // M-3(c): 拡張子付きの未存在は index.html を返さず 404（SW 更新・欠落検知を壊さない）。
        let dir = dist();
        for uri in ["/old-removed.js", "/sw-missing.js", "/style.css"] {
            let resp = get_resp(dir.path(), uri).await;
            assert_eq!(resp.status(), StatusCode::NOT_FOUND, "uri={uri}");
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let text = String::from_utf8_lossy(&bytes);
            assert!(text.contains("NOT-FOUND"), "uri={uri} body={text}");
            assert!(
                !text.contains("SPA-SHELL"),
                "uri={uri} 404 が index を返している"
            );
        }
    }

    #[tokio::test]
    async fn api_route_is_not_shadowed_by_static() {
        let dir = dist();
        let resp = get_resp(dir.path(), "/api/me").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"me");
    }
}
