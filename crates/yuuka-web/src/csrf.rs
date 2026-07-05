//! CSRF 防御ミドルウェア（既存 Node `isCrossSiteStateChange`・routeRegistry.ts:37-60 と一致）。
//!
//! 状態変更（POST/PUT/PATCH/DELETE）× **Cookie 認証（ambient credential）** のリクエストに
//! same-site を強制する。`Authorization: Bearer` を持つ非 ambient リクエストは対象外。
//! 判定は `Sec-Fetch-Site` を優先し、無ければ `Origin`/`Referer` の hostname を `Host` と照合。
//! 両者欠落時は `SameSite=Lax` cookie に委ねて許可する（Node と一致）。
//!
//! 注: Node の `stripProtoKeys`（`__proto__`/`constructor`/`prototype` 除去）は **Rust では不要**。
//! serde は typed struct / `Value` にデシリアライズし prototype チェーンが存在しないため、
//! prototype 汚染は構造的に発生しない（移植不要な JS 固有防御）。

use axum::extract::Request;
use axum::http::Method;
use axum::middleware::Next;
use axum::response::Response;
use yuuka_core::WebError;

use crate::error::ApiError;

const COOKIE_HOST: &str = "__Host-yuuka-session";
const COOKIE_DEV: &str = "yuuka-session";

/// CSRF ガード（`axum::middleware::from_fn` で使う）。
///
/// # Errors
/// クロスサイトな状態変更（Cookie 認証）を検出したら 403（[`WebError::Forbidden`]）。
pub async fn csrf_guard(req: Request, next: Next) -> Result<Response, ApiError> {
    if is_state_changing(req.method())
        && has_session_cookie(&req)
        && !has_bearer(&req)
        && is_cross_site(&req)
    {
        return Err(ApiError(WebError::Forbidden));
    }
    Ok(next.run(req).await)
}

fn is_state_changing(m: &Method) -> bool {
    matches!(*m, Method::POST | Method::DELETE | Method::PUT | Method::PATCH)
}

fn header_str<'a>(req: &'a Request, name: &str) -> Option<&'a str> {
    req.headers().get(name)?.to_str().ok()
}

/// `Authorization: Bearer ...`（非 ambient 認証）を持つか。
fn has_bearer(req: &Request) -> bool {
    header_str(req, "authorization")
        .and_then(|v| v.get(..7))
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("bearer "))
}

/// セッション Cookie（ambient credential）を持つか。
fn has_session_cookie(req: &Request) -> bool {
    header_str(req, "cookie").is_some_and(|raw| {
        raw.split(';')
            .filter_map(|kv| kv.split_once('='))
            .any(|(k, _)| {
                let k = k.trim();
                k == COOKIE_HOST || k == COOKIE_DEV
            })
    })
}

/// クロスサイトな状態変更か（Sec-Fetch-Site 優先、Origin/Referer フォールバック）。
fn is_cross_site(req: &Request) -> bool {
    if let Some(sfs) = header_str(req, "sec-fetch-site") {
        // same-origin / same-site / none は許可、cross-site のみ拒否。
        return sfs.eq_ignore_ascii_case("cross-site");
    }
    let host = header_str(req, "host").map(host_only);
    let source = header_str(req, "origin").or_else(|| header_str(req, "referer"));
    match source {
        // Origin/Referer 両欠 → SameSite=Lax に委ねて許可（Node と一致）。
        None => false,
        Some(url) => match (url_host(url), host) {
            (Some(candidate), Some(h)) => candidate != h,
            // hostname か Host が解釈不能なら安全側で cross-site 扱い。
            _ => true,
        },
    }
}

/// `hostname:port` から hostname を取り出す（port が数値のときのみ剥がす）。
fn host_only(h: &str) -> &str {
    h.rsplit_once(':').map_or(h, |(host, port)| {
        if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) {
            host
        } else {
            h
        }
    })
}

/// URL（Origin/Referer）から hostname を取り出す。
fn url_host(url: &str) -> Option<&str> {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = after_scheme.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    Some(host_only(authority))
}
