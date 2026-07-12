//! CSRF 防御ミドルウェア（既存 Node `isCrossSiteStateChange`・routeRegistry.ts:37-60 と一致）。
//!
//! 状態変更（POST/PUT/PATCH/DELETE）× **Cookie 認証（ambient credential）** のリクエストに
//! same-site を強制する。`Authorization: Bearer <非空>` を持つ非 ambient リクエストは対象外。
//! 判定は `Sec-Fetch-Site: cross-site` のみを即拒否し、それ以外は `Origin`/`Referer` の hostname を
//! **設定済み許可ホスト（`config.base_url` 由来・未設定は localhost 群）** と照合する（Node `isAllowedHost`）。
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

/// CSRF ガード（`axum::middleware::from_fn` で使う）。`allowed_host` は許可オリジンのホスト名
/// （`config.base_url` 由来・未設定 `None` は localhost/127.0.0.1/[::1] を許可する開発既定）。
///
/// # Errors
/// クロスサイトな状態変更（Cookie 認証）を検出したら 403（[`WebError::Forbidden`]）。
pub async fn csrf_guard(
    allowed_host: Option<String>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    if is_state_changing(req.method())
        && has_session_cookie(&req)
        && !has_bearer(&req)
        && is_cross_site(&req, allowed_host.as_deref())
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

/// `Authorization: Bearer <非空トークン>`（非 ambient 認証）を持つか。
///
/// **#3**: スキーム語のみ（`Bearer ` で末尾がトークン空）の場合は Bearer 扱いしない — さもないと
/// 空 Bearer で CSRF を回避しつつ auth extractor は Cookie にフォールバックする潜在バイパスになる。
/// 判定は [`crate::auth`] の `bearer_token`（非空トークン必須）と一致させる。
fn has_bearer(req: &Request) -> bool {
    let Some(raw) = header_str(req, "authorization") else {
        return false;
    };
    let Some((scheme, rest)) = raw.split_at_checked(6) else {
        return false;
    };
    scheme.eq_ignore_ascii_case("bearer")
        && rest.starts_with(char::is_whitespace)
        && !rest.trim().is_empty()
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

/// クロスサイトな状態変更か（Node `isCrossSiteStateChange` パリティ）。
///
/// **#2**: `Sec-Fetch-Site` は `cross-site` のみを即拒否する。他値（`same-site`/`same-origin`/`none`）は
/// **Origin/Referer の allowlist 検証へ委ねる** — `same-site` は攻撃者が握った兄弟サブドメイン
/// （`SameSite=Lax` cookie が付く）を含むため、ブラウザラベルだけで許可してはならない。
fn is_cross_site(req: &Request, allowed_host: Option<&str>) -> bool {
    if header_str(req, "sec-fetch-site").is_some_and(|s| s.eq_ignore_ascii_case("cross-site")) {
        return true;
    }
    // Origin 優先（`null` は無視）→ Referer。両欠は判定不能で許可（SameSite=Lax 委任・Node と一致）。
    let source = header_str(req, "origin")
        .filter(|o| !o.eq_ignore_ascii_case("null"))
        .or_else(|| header_str(req, "referer"));
    match source {
        None => false,
        Some(url) => match url_host(url) {
            Some(host) => !is_allowed_host(host, allowed_host),
            // URL 解釈不能なら安全側で cross-site 扱い（Node の `catch → return true`）。
            None => true,
        },
    }
}

/// Origin/Referer の hostname が自サイトか（Node `isAllowedHost`・**#1**）。
///
/// `allowed_host`（`config.base_url` のホスト名）が設定済ならそれと**大小無視で一致**、未設定なら
/// localhost 群のみ許可する。クライアント供給の `Host` ヘッダには依存しない（Host 注入耐性）。
fn is_allowed_host(hostname: &str, allowed_host: Option<&str>) -> bool {
    match allowed_host {
        Some(h) => hostname.eq_ignore_ascii_case(h),
        None => matches!(hostname, "localhost" | "127.0.0.1" | "[::1]" | "::1"),
    }
}

/// `config.base_url` から許可ホスト名を取り出す（[`crate::WebConfig`] 構築時に 1 回）。
pub(crate) fn allowed_host_from_base_url(base_url: Option<&str>) -> Option<String> {
    url_host(base_url?).map(str::to_owned)
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
