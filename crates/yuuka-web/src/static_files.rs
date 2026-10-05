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
//! **配信パス（#34）**: フロント（`frontend/`・管理画面 SPA）は `vite.config.ts` の
//! `base: "/admin/"` に合わせて [`ADMIN_PREFIX`] 配下にのみ載せる（`nest_service`）。ルート
//! `"/"` は PWA（[`mount_pwa`]・issue #33）が明示ルートとして配信する。`/admin` 以外の
//! 未登録パスは本サービスの対象外＝素の 404（以前はどんな未知パスも index.html(200) を返していたが、
//! それは `/admin` 移設前の名残で、PWA 領域を誤って呑み込む状態だった）。
//!
//! **旧 `/login`（互換）**: `/admin` 移設前、共有ログインは `/login`（ルート直下）にあった。現在は
//! `/admin/login`（管理画面 SPA 内のルート）にしか存在しないため、ブックマークや旧ビルドの PWA
//! （`/login?returnTo=…` へ送っていた）が 404 にならないよう、`GET /login`・`/login/` を
//! クエリ（`returnTo` 等）ごと `/admin/login` へリダイレクトする（[`legacy_login_redirect`]）。
//!
//! セキュリティヘッダ（CSP/Referrer/HSTS/nosniff/X-Frame）は `apply_common_layers` が
//! 全応答へ付与する（本モジュールは `Cache-Control` のみ担当）。
//! index.html への google-site-verification meta 注入は後続増分（deferred・機能非依存）。

use std::convert::Infallible;
use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::extract::Request;
use axum::http::header::{HeaderValue, CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use tower::{service_fn, ServiceExt};
use tower_http::services::ServeDir;

/// ハッシュ付き資産の Cache-Control（1 年・immutable）。
const IMMUTABLE: &str = "public, max-age=31536000, immutable";
/// 非ハッシュ資産・index.html・404・SPA フォールバックの Cache-Control（Node 一致）。
const NO_CACHE: &str = "no-cache, no-store, must-revalidate";

/// 管理画面 SPA の配信プレフィックス。`frontend/vite.config.ts` の `base` と一字一句一致させる
/// こと（ズレると資産 404・deep link 崩壊に直結する）。OAuth コールバック等サーバ側リダイレクト
/// 先の構築にも使う（[`crate::static_files`] 外からは `yuuka_web::ADMIN_PREFIX` で参照）。
pub const ADMIN_PREFIX: &str = "/admin";

/// `dist_dir`（例 `dist/public`）を管理画面 SPA として [`ADMIN_PREFIX`] 配下に `nest_service` で載せる。
///
/// API ルートは明示登録されており `/admin` はそれと衝突しないため（`yuuka-admin` の API は全て
/// `/api/admin/*`）、この呼び出し順は問わない。ルート `"/"` は本関数では触らない ── PWA
/// （[`mount_pwa`]）が明示ルートとして配信する面であり、[`mount_pwa`] と組み合わせない呼び出し
/// （PWA 未配線）では単に [`top_level_fallback`] の 404 に落ちる。
///
/// 旧共有ログインのパス `GET /login`・`/login/` だけは例外的に `/admin/login` へ互換リダイレクトする
/// （[`legacy_login_redirect`]）。
pub fn mount_static<S>(router: Router<S>, dist_dir: &Path) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let dist = dist_dir.to_path_buf();
    let admin_service = service_fn(move |req: Request| {
        let dist = dist.clone();
        async move { Ok::<Response, Infallible>(serve_static(&dist, req).await) }
    });
    let dist_for_fallback = dist_dir.to_path_buf();
    router
        .route("/login", get(legacy_login_redirect))
        .route("/login/", get(legacy_login_redirect))
        .nest_service(ADMIN_PREFIX, admin_service)
        .fallback_service(service_fn(move |req: Request| {
            let dist = dist_for_fallback.clone();
            async move { Ok::<Response, Infallible>(top_level_fallback(&dist, req).await) }
        }))
}

/// 旧共有ログイン `/login` → `/admin/login` の互換リダイレクト先 URL。
///
/// クエリは**そのまま**引き継ぐ（PWA が付ける `returnTo`・`/device` 由来の値等。値の検証は移動先の
/// 管理画面 SPA が `returnTo` ごとに行う）。移動先は常に固定の `/admin/login` で始まり、`Host` や
/// リクエスト由来の値をパス側へ持ち込まないため、オープンリダイレクトにならない。
fn legacy_login_target(uri: &Uri) -> String {
    match uri.query() {
        Some(query) => format!("{ADMIN_PREFIX}/login?{query}"),
        None => format!("{ADMIN_PREFIX}/login"),
    }
}

/// `GET /login`・`/login/` → `/admin/login`（クエリ保持・307）。恒久化しない（`308`/`301` は
/// ブラウザにキャッシュされ、将来ログインの居場所が変わったときに戻せない）。
async fn legacy_login_redirect(uri: Uri) -> Redirect {
    Redirect::temporary(&legacy_login_target(&uri))
}

/// `/admin` にマッチしなかった全リクエストの最終フォールバック（`"/"` を含む ── PWA が未配線
/// （[`mount_pwa`] 未呼び出し）の場合はここへ落ちる）。
///
/// `/api/*` は（`/admin` 配下同様）明示 JSON 404 を維持する（B6 parity・`/admin` 移設の前後で
/// API 未実装 404 の挙動が変わらないように）。それ以外は真の 404（`dist/404.html`）を返し、
/// 管理画面 SPA を呑み込まない（旧実装は未知パスを何でも index.html(200) にフォールバックしていたが、
/// それは `/admin` 分離前の名残であり、PWA 領域（issue #33）を誤って管理画面が横取りしてしまう）。
async fn top_level_fallback(dist: &Path, req: Request) -> Response {
    let path = req.uri().path().to_owned();
    if path.starts_with("/api/") {
        return api_not_found();
    }
    let mut resp = file_response(
        &dist.join("404.html"),
        StatusCode::NOT_FOUND,
        "404 Not Found",
    )
    .await;
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static(NO_CACHE));
    resp
}

/// Node `serveStaticFile` の写像: `ServeDir` で配信し、未存在は拡張子で分岐、最後に Cache-Control。
async fn serve_static(dist: &Path, req: Request) -> Response {
    let path = req.uri().path().to_owned();

    // **B6**: 未登録の `/api/*` は SPA フォールバック（index.html 200）に落とさず、明示 JSON 404 を返す
    // （Node `server.ts:339` パリティ）。登録済み API ルートは axum が先にマッチするため、ここへ到達する
    // `/api/*` は未実装/未登録パスのみ。これを index.html(200) で握り潰すと、フロント `client.ts` が
    // JSON パース失敗 → `res.ok=true` で「空の成功」に誤変換し、未実装機能が沈黙して壊れる。全メソッド対象。
    if path.starts_with("/api/") {
        return api_not_found();
    }

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

/// 未登録 `/api/*` 向けの JSON 404（Node `server.ts` の `{success:false,message:…}` と一致）。
/// 共通セキュリティヘッダは [`crate::apply_common_layers`] が全応答へ付与する。
fn api_not_found() -> Response {
    let mut resp = Response::new(Body::from(
        r#"{"success":false,"message":"APIエンドポイントが見つかりません。"}"#,
    ));
    *resp.status_mut() = StatusCode::NOT_FOUND;
    resp.headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
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

/// PWA（`client/pwa`）のクライアントサイドルート（Node `CLIENT_ROUTES`・`src/server.ts`）。
const PWA_SHELL_ROUTES: [&str; 7] = [
    "/",
    "/chat",
    "/todo",
    "/calendar",
    "/finance",
    "/notes",
    "/settings",
];

/// PWA（`client/pwa`）の SPA を、Node `serveStaticFile` の PWA 分岐と同じ URL 面に登録する
/// （issue #33: Rust に `/api/client/*` は移植したが、PWA 自体を配信するルートが無いと
/// ブラウザから開けない）。
///
/// Node が明示許可する面だけを再現し、それ以外（`/admin`・`/login`・未登録の `/api/*` 等）には
/// 一切踏み込まない:
/// - **シェルルート**（[`PWA_SHELL_ROUTES`]・Vue Router のクライアントサイドルート）→
///   `index.html`（`no-cache, no-store, must-revalidate`）
/// - **固定静的パス**: `/manifest.webmanifest`・`/sw.js`・`/icons/app-icon.svg`（いずれも
///   `no-cache`。`sw.js` は Service Worker の更新検知のため特に必須・PR #67 の
///   ネットワーク優先 SW と整合）
/// - **ハッシュ資産**: `/assets/*` → [`is_hashed_asset`] 判定で `immutable` 長期キャッシュ、
///   それ以外・未存在は `no-cache`/404
///
/// 本関数は**明示 `route`/`nest_service` のみ**を積み、[`mount_static`]（admin SPA・
/// `fallback_service` ベース）とは独立に同一ルータへ組み合わせられる: axum は明示ルートを
/// `fallback_service` より必ず優先するため、登録順に関わらず PWA が上記の固定パス群だけを
/// 横取りし、`/admin` を含む残りは admin 側の fallback へ従来どおり落ちる。
pub fn mount_pwa<S>(router: Router<S>, pwa_dist: &Path) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let dist = pwa_dist.to_path_buf();

    let mut router = router;
    for path in PWA_SHELL_ROUTES {
        let shell_dist = dist.clone();
        router = router.route(path, get(move || pwa_index(shell_dist.clone())));
    }

    let manifest_dist = dist.clone();
    let sw_dist = dist.clone();
    let icon_dist = dist.clone();
    // `nest_service("/assets", ..)` はプレフィックスを剥がして渡す（例: `/assets/app-x.js` →
    // `/app-x.js`）ため、配信ルートは `dist` 直下ではなく `dist/assets` にする必要がある。
    let assets_dist = dist.join("assets");
    router
        .route(
            "/manifest.webmanifest",
            get(move || {
                pwa_file(
                    manifest_dist.clone(),
                    "manifest.webmanifest",
                    "application/manifest+json; charset=utf-8",
                )
            }),
        )
        .route(
            "/sw.js",
            get(move || {
                pwa_file(
                    sw_dist.clone(),
                    "sw.js",
                    "application/javascript; charset=utf-8",
                )
            }),
        )
        .route(
            "/icons/app-icon.svg",
            get(move || pwa_file(icon_dist.clone(), "icons/app-icon.svg", "image/svg+xml")),
        )
        .nest_service(
            "/assets",
            service_fn(move |req: Request| {
                let dist = assets_dist.clone();
                async move { Ok::<Response, Infallible>(serve_pwa_assets(&dist, req).await) }
            }),
        )
}

/// PWA シェル（`index.html`）を返す。SPA シェルは常に最新を取得させるため `no-cache` 固定。
async fn pwa_index(dist: PathBuf) -> Response {
    let mut resp = file_response(&dist.join("index.html"), StatusCode::OK, "").await;
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static(NO_CACHE));
    resp
}

/// PWA の固定静的ファイル（manifest/sw.js/icon）を `no-cache` で返す。読めなければ 404。
async fn pwa_file(dist: PathBuf, rel: &'static str, content_type: &'static str) -> Response {
    match tokio::fs::read(dist.join(rel)).await {
        Ok(bytes) => {
            let mut resp = Response::new(Body::from(bytes));
            *resp.status_mut() = StatusCode::OK;
            resp.headers_mut()
                .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
            resp.headers_mut()
                .insert(CACHE_CONTROL, HeaderValue::from_static(NO_CACHE));
            resp
        }
        Err(_) => {
            let mut resp = Response::new(Body::from("404 Not Found"));
            *resp.status_mut() = StatusCode::NOT_FOUND;
            resp
        }
    }
}

/// PWA 側の `/assets/*` を配信する（[`is_hashed_asset`] 判定でハッシュ資産のみ immutable）。
///
/// `nest_service("/assets", ..)` はプレフィックスを剥がして渡すため、判定用に `/assets` を
/// 付け直してから [`is_hashed_asset`] へ渡す。
async fn serve_pwa_assets(dist: &Path, req: Request) -> Response {
    let full_path = format!("/assets{}", req.uri().path());
    let serve = ServeDir::new(dist).precompressed_br().precompressed_gzip();
    let served = match serve.oneshot(req).await {
        Ok(resp) => resp.into_response(),
        Err(never) => match never {},
    };
    let mut resp = served;
    let cache = if resp.status() == StatusCode::OK && is_hashed_asset(&full_path) {
        IMMUTABLE
    } else {
        NO_CACHE
    };
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static(cache));
    resp
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
        let resp = get_resp(dir.path(), "/admin/assets/app-BIDAdKj3.js").await;
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
        let resp = get_resp(dir.path(), "/admin/assets/app-abc123.js").await;
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
        let resp = get_resp(dir.path(), "/admin/assets/missing-BIDAdKj3.js").await;
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
        for uri in [
            "/admin/index.html",
            "/admin/manifest.json",
            "/admin",
            "/admin/",
        ] {
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
        // /admin 配下の拡張子なし未知パスは index.html を 200 で返す（SPA ルーティング・deep link）。
        let dir = dist();
        let resp = get_resp(dir.path(), "/admin/dashboard/tasks").await;
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
        for uri in [
            "/admin/old-removed.js",
            "/admin/sw-missing.js",
            "/admin/style.css",
        ] {
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
    async fn path_outside_admin_is_404_not_admin_shell() {
        // #34/#74: `/admin` 移設後は "/admin" 以外の未知パスを管理画面 index.html で呑み込まない
        // （旧実装はどんな未知パスも SPA にフォールバックし、PWA 領域〔issue #33〕を横取りしていた）。
        // "/" もこの対象 ── `mount_static` 単独（PWA 未配線）では [`super::mount_pwa`] が無いため
        // 暫定リダイレクトはもう存在せず、他の未登録パスと同じく 404 に落ちる
        // （PWA を配線した本番構成では [`super::mount_pwa`] が先に "/" を明示ルートとして横取りする・
        // `pwa_tests::shell_routes_serve_pwa_index_no_cache` 参照）。
        let dir = dist();
        for uri in ["/", "/dashboard/tasks"] {
            let resp = get_resp(dir.path(), uri).await;
            assert_eq!(resp.status(), StatusCode::NOT_FOUND, "uri={uri}");
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            assert!(
                !String::from_utf8_lossy(&bytes).contains("SPA-SHELL"),
                "uri={uri}"
            );
        }
    }

    fn location(resp: &axum::response::Response) -> Option<String> {
        resp.headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    }

    #[tokio::test]
    async fn legacy_login_redirects_to_admin_login_preserving_query() {
        // 旧共有ログイン `/login`（PWA・ブックマーク）は `/admin/login` へ。`returnTo` 等のクエリは
        // 一字一句そのまま引き継ぐ（PWA は `returnTo=%2Ftodo` を付けてくる）。
        let dir = dist();
        for (uri, expected) in [
            ("/login", "/admin/login"),
            ("/login/", "/admin/login"),
            ("/login?", "/admin/login?"),
            ("/login?returnTo=%2Ftodo", "/admin/login?returnTo=%2Ftodo"),
            (
                "/login?returnTo=%2Fchat%3Ffrom%3Dchat%23latest&x=1",
                "/admin/login?returnTo=%2Fchat%3Ffrom%3Dchat%23latest&x=1",
            ),
        ] {
            let resp = get_resp(dir.path(), uri).await;
            assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT, "uri={uri}");
            assert_eq!(location(&resp).as_deref(), Some(expected), "uri={uri}");
        }
    }

    #[tokio::test]
    async fn legacy_login_redirect_never_leaves_the_admin_login_path() {
        // オープンリダイレクトにならない: 移動先は常に `/admin/login` 固定始まり。ホスト差し替え風の
        // クエリ（`//evil.example`・`https://…`）もクエリ内に留まりパス側へは出ない。
        let dir = dist();
        for uri in [
            "/login?returnTo=//evil.example",
            "/login?returnTo=https://evil.example/",
            "/login?//evil.example",
            "/login?@evil.example",
        ] {
            let resp = get_resp(dir.path(), uri).await;
            assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT, "uri={uri}");
            let loc = location(&resp).expect("location");
            assert!(loc.starts_with("/admin/login"), "uri={uri} loc={loc}");
            assert!(!loc.starts_with("//"), "uri={uri} loc={loc}");
            let path = loc.split('?').next().unwrap_or_default();
            assert_eq!(path, "/admin/login", "uri={uri} loc={loc}");
        }
    }

    #[tokio::test]
    async fn legacy_login_redirect_is_get_only_and_leaves_other_unknown_paths_404() {
        let dir = dist();
        // HEAD は GET と同じ（axum の `get()` が自動で処理する）。
        let head = app(dir.path())
            .oneshot(
                Request::builder()
                    .method("HEAD")
                    .uri("/login?returnTo=%2Ftodo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(head.status(), StatusCode::TEMPORARY_REDIRECT);
        // POST は 405（`/api/login` はこの面ではない）。
        let post = app(dir.path())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/login")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);
        // `/login` 配下の他パス・類似パスは互換対象外（従来どおり 404）。
        for uri in ["/login/extra", "/loginx", "/logout"] {
            let resp = get_resp(dir.path(), uri).await;
            assert_eq!(resp.status(), StatusCode::NOT_FOUND, "uri={uri}");
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

    #[tokio::test]
    async fn unregistered_api_path_returns_json_404_not_index_html() {
        // B6: 未登録 /api/* は index.html(200) ではなく明示 JSON 404 を返す（SPA 握り潰し防止）。
        let dir = dist();
        let resp = get_resp(dir.path(), "/api/does-not-exist").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            resp.headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some("application/json")
        );
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("\"success\":false"), "body={text}");
        assert!(
            text.contains("APIエンドポイントが見つかりません"),
            "body={text}"
        );
        assert!(
            !text.contains("SPA-SHELL"),
            "API 404 が index を返している: {text}"
        );
    }
}

#[cfg(test)]
mod pwa_tests {
    use super::{mount_pwa, mount_static};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use std::fs;
    use tower::ServiceExt;

    /// PWA dist（index.html + manifest + sw.js + icon + assets）。
    fn pwa_dist() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tmp");
        fs::write(dir.path().join("index.html"), "<html>PWA-SHELL</html>").expect("index");
        fs::write(dir.path().join("manifest.webmanifest"), "{}").expect("manifest");
        fs::write(dir.path().join("sw.js"), "// sw").expect("sw");
        fs::create_dir_all(dir.path().join("icons")).expect("icons dir");
        fs::write(dir.path().join("icons/app-icon.svg"), "<svg/>").expect("icon");
        fs::create_dir_all(dir.path().join("assets")).expect("assets dir");
        fs::write(dir.path().join("assets/app-BIDAdKj3.js"), "console.log(1)").expect("hashed");
        dir
    }

    /// admin dist（`mount_static` の fallback 用・PWA と共存できることを確認するため）。
    fn admin_dist() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tmp");
        fs::write(dir.path().join("index.html"), "<html>ADMIN-SHELL</html>").expect("index");
        fs::write(dir.path().join("404.html"), "<html>NOT-FOUND</html>").expect("404");
        dir
    }

    fn cache_control(resp: &axum::response::Response) -> Option<String> {
        resp.headers()
            .get("cache-control")
            .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
    }

    async fn body_text(resp: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// PWA + admin fallback + 明示 API ルートを同一ルータへ組み合わせたアプリ（実運用の配線を模す）。
    fn app(pwa_dir: &std::path::Path, admin_dir: &std::path::Path) -> Router {
        let router = Router::new().route("/api/me", get(|| async { "me" }));
        let router = mount_pwa(router, pwa_dir);
        mount_static(router, admin_dir)
    }

    async fn get_resp(
        pwa_dir: &std::path::Path,
        admin_dir: &std::path::Path,
        uri: &str,
    ) -> axum::response::Response {
        app(pwa_dir, admin_dir)
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn shell_routes_serve_pwa_index_no_cache() {
        let pwa = pwa_dist();
        let admin = admin_dist();
        for uri in [
            "/",
            "/chat",
            "/todo",
            "/calendar",
            "/finance",
            "/notes",
            "/settings",
        ] {
            let resp = get_resp(pwa.path(), admin.path(), uri).await;
            assert_eq!(resp.status(), StatusCode::OK, "uri={uri}");
            assert_eq!(
                cache_control(&resp).as_deref(),
                Some("no-cache, no-store, must-revalidate"),
                "uri={uri}"
            );
            let text = body_text(resp).await;
            assert!(text.contains("PWA-SHELL"), "uri={uri} body={text}");
        }
    }

    #[tokio::test]
    async fn manifest_and_sw_are_no_cache_with_correct_content_type() {
        let pwa = pwa_dist();
        let admin = admin_dist();

        let manifest = get_resp(pwa.path(), admin.path(), "/manifest.webmanifest").await;
        assert_eq!(manifest.status(), StatusCode::OK);
        assert_eq!(
            manifest
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some("application/manifest+json; charset=utf-8")
        );
        assert_eq!(
            cache_control(&manifest).as_deref(),
            Some("no-cache, no-store, must-revalidate")
        );

        let sw = get_resp(pwa.path(), admin.path(), "/sw.js").await;
        assert_eq!(sw.status(), StatusCode::OK);
        assert_eq!(
            cache_control(&sw).as_deref(),
            Some("no-cache, no-store, must-revalidate"),
            "sw.js は必ず no-cache（更新検知を壊さない）"
        );
    }

    #[tokio::test]
    async fn pwa_assets_hashed_file_is_immutable() {
        let pwa = pwa_dist();
        let admin = admin_dist();
        let resp = get_resp(pwa.path(), admin.path(), "/assets/app-BIDAdKj3.js").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            cache_control(&resp).as_deref(),
            Some("public, max-age=31536000, immutable")
        );
    }

    #[tokio::test]
    async fn admin_and_api_routes_are_not_shadowed_by_pwa() {
        // /admin* は PWA のシェルルートに含まれないため、従来どおり admin 側の fallback（index.html）
        // へ落ちる。/api/me も明示ルートとして生き続ける（PWA が横取りしない）。
        let pwa = pwa_dist();
        let admin = admin_dist();

        let admin_resp = get_resp(pwa.path(), admin.path(), "/admin").await;
        assert_eq!(admin_resp.status(), StatusCode::OK);
        let admin_text = body_text(admin_resp).await;
        assert!(admin_text.contains("ADMIN-SHELL"), "body={admin_text}");

        let api_resp = get_resp(pwa.path(), admin.path(), "/api/me").await;
        assert_eq!(api_resp.status(), StatusCode::OK);
        assert_eq!(body_text(api_resp).await, "me");
    }

    #[tokio::test]
    async fn missing_pwa_file_is_404() {
        let pwa = tempfile::tempdir().expect("tmp"); // 空dist（未ビルド想定）。
        let admin = admin_dist();
        let resp = get_resp(pwa.path(), admin.path(), "/manifest.webmanifest").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
