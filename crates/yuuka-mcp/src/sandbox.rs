//! MCP Apps のサンドボックスプロキシ（ext-apps 仕様 2026-01-26「Sandbox proxy」）。
//!
//! MCP サーバーが返す画面（`ui://` リソースの HTML）は第三者のコードなので、yuuka 本体のオリジンでは
//! 動かさない。管理画面はこのページを iframe で開き、このページがさらに内側の iframe に画面を読み込んで
//! メッセージを中継する（`sandbox.html`）。
//!
//! **オリジンの分け方**（`MCP_SANDBOX_ORIGIN`）:
//! - 設定あり: 管理画面は `{MCP_SANDBOX_ORIGIN}/mcp-apps/sandbox` を `allow-scripts allow-same-origin` で
//!   開く。画面は本体と別の実オリジンで動く（仕様どおり）。そのホスト名では、このページ以外を 404 にする
//!   （[`sandbox_host_guard`]。Cookie が送られても API に触れさせない）。
//! - 設定なし: 本体の `/mcp-apps/sandbox` を `allow-scripts`（`allow-same-origin` なし）で開く。画面は
//!   不透明オリジンで動く。本体からは隔離されるが、localStorage 等は使えず、外部への fetch は
//!   `Origin: null` になる。
//!
//! このページの CSP は、内側の画面が読み込めるものの上限を決める（srcdoc の iframe は親の CSP を
//! 引き継ぐ）。個々の画面の制限は、サーバーが `_meta.ui.csp` で申告した内容をプロキシが meta で
//! 差し込んで掛ける（申告の無いドメインは許さない）。

use axum::extract::{Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, HOST};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use yuuka_web::AppState;

/// サンドボックスプロキシのパス。
pub const SANDBOX_PATH: &str = "/mcp-apps/sandbox";

const SANDBOX_HTML: &str = include_str!("sandbox.html");

/// `BASE_URL` のオリジン（未設定時はリクエストの Host・`http://`）。
pub(crate) fn self_origin(state: &AppState, host: Option<&str>) -> String {
    if let Some(origin) = state.config.base_url.as_deref().and_then(url_origin) {
        return origin;
    }
    format!("http://{}", host.unwrap_or("localhost"))
}

/// URL のオリジン（`scheme://host[:port]`）。既定ポートは省く。
pub(crate) fn url_origin(url: &str) -> Option<String> {
    let url = url::Url::parse(url).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let origin = url.origin().ascii_serialization();
    (origin != "null").then_some(origin)
}

/// プロキシページの CSP。内側の画面が使えるものの上限（https のみ）と、埋め込める親を決める。
fn proxy_csp(host_origin: &str) -> String {
    [
        "default-src 'none'".to_owned(),
        "script-src 'self' 'unsafe-inline' https:".to_owned(),
        "style-src 'self' 'unsafe-inline' https:".to_owned(),
        "img-src 'self' data: blob: https:".to_owned(),
        "font-src 'self' data: https:".to_owned(),
        "media-src 'self' data: blob: https:".to_owned(),
        "connect-src https: wss:".to_owned(),
        "frame-src 'self' https:".to_owned(),
        "base-uri 'self' https:".to_owned(),
        "object-src 'none'".to_owned(),
        "form-action 'none'".to_owned(),
        format!("frame-ancestors {host_origin}"),
    ]
    .join("; ")
}

/// `GET /mcp-apps/sandbox`（認証なし・静的ページ）。ホスト（管理画面）のオリジンを埋め込んで返す。
pub(crate) async fn sandbox_page(State(state): State<AppState>, req: Request) -> Response {
    let host = req.headers().get(HOST).and_then(|v| v.to_str().ok());
    let host_origin = self_origin(&state, host);
    let html = SANDBOX_HTML.replace("__HOST_ORIGIN__", &html_attr_escape(&host_origin));
    let mut resp = (StatusCode::OK, html).into_response();
    let h = resp.headers_mut();
    h.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(v) = HeaderValue::from_str(&proxy_csp(&host_origin)) {
        h.insert(CONTENT_SECURITY_POLICY, v);
    }
    resp
}

fn html_attr_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// `MCP_SANDBOX_ORIGIN` のホスト名（`host[:port]`）。未設定・不正は `None`。
#[must_use]
pub fn sandbox_host(sandbox_origin: Option<&str>) -> Option<String> {
    let url = url::Url::parse(sandbox_origin?).ok()?;
    let host = url.host_str()?;
    Some(match url.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_owned(),
    })
}

/// サンドボックス用ホスト名へのリクエストは、サンドボックスプロキシの GET 以外を 404 にする。
///
/// 同じプロセスが本体とサンドボックスの両方のホスト名で応答するため、サンドボックス側のオリジンで動く
/// 画面から本体の API（Cookie 付き）に触れられないようにする。`sandbox_host` が `None`（未設定）なら
/// 何もしない。
pub async fn sandbox_host_guard(
    sandbox_host: Option<std::sync::Arc<str>>,
    req: Request,
    next: Next,
) -> Response {
    if let Some(sandbox) = sandbox_host.as_deref() {
        let host = req
            .headers()
            .get(HOST)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        if host.eq_ignore_ascii_case(sandbox) {
            let allowed =
                req.method() == axum::http::Method::GET && req.uri().path() == SANDBOX_PATH;
            if !allowed {
                return StatusCode::NOT_FOUND.into_response();
            }
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_and_host_extraction() {
        assert_eq!(
            url_origin("https://yuuka.example/app?x=1").as_deref(),
            Some("https://yuuka.example")
        );
        assert_eq!(
            url_origin("http://localhost:3000/").as_deref(),
            Some("http://localhost:3000")
        );
        assert_eq!(url_origin("ftp://x"), None);
        assert_eq!(
            sandbox_host(Some("https://mcp-sandbox.example")).as_deref(),
            Some("mcp-sandbox.example")
        );
        assert_eq!(
            sandbox_host(Some("http://localhost:3001")).as_deref(),
            Some("localhost:3001")
        );
        assert_eq!(sandbox_host(None), None);
    }

    #[test]
    fn proxy_csp_limits_parent_and_plugins() {
        let csp = proxy_csp("https://yuuka.example");
        assert!(csp.contains("frame-ancestors https://yuuka.example"));
        assert!(csp.contains("object-src 'none'"));
        assert!(!csp.contains("http:"), "平文 http の読み込みは許さない");
    }

    #[test]
    fn sandbox_html_has_host_origin_placeholder() {
        assert!(SANDBOX_HTML.contains("__HOST_ORIGIN__"));
        assert!(SANDBOX_HTML.contains("ui/notifications/sandbox-proxy-ready"));
    }
}
