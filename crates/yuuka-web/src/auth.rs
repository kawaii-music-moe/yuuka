//! 認証バックエンド契約と、型付き認可 extractor（`FromRequestParts`）。
//!
//! 認可レベル（none/user/admin）を **型**で表現し、ハンドラ署名で強制する（§6・00-decisions）。
//! 解決順は **Cookie 優先 → Bearer**（§11.3・既存 `resolveRequestUser` と一致）。
//! axum 0.8 は `FromRequestParts` に `async fn`（RPITIT）を使い `#[async_trait]` 不要。

use async_trait::async_trait;
use axum::extract::FromRequestParts;
use axum::http::header::{AUTHORIZATION, COOKIE};
use axum::http::request::Parts;
use yuuka_core::{AuthError, WebError};
use yuuka_types::{Role, SessionUser};

use crate::error::ApiError;
use crate::state::AppState;

/// 認証バックエンド。Cookie セッション（Redis）と Bearer デスクトップトークン（SQLite）を
/// 解決する。実装が §11.3 のハッシュ・ストア参照を担う（Phase 1 は in-memory で骨組み検証、
/// Redis/SQLite 実装は後続増分）。`Arc<dyn AuthBackend>` で持つため `#[async_trait]`。
#[async_trait]
pub trait AuthBackend: Send + Sync {
    /// Cookie 由来トークンからセッションユーザーを解決する（未一致は `Ok(None)`）。
    ///
    /// # Errors
    /// ストア（Redis 等）への到達失敗時 [`AuthError`]。
    async fn session_user(&self, cookie_token: &str) -> Result<Option<SessionUser>, AuthError>;

    /// `Authorization: Bearer` のデスクトップトークンからユーザーを解決する。
    ///
    /// # Errors
    /// ストア（SQLite 等）への到達失敗時 [`AuthError`]。
    async fn desktop_user(&self, bearer_token: &str) -> Result<Option<SessionUser>, AuthError>;
}

const COOKIE_HOST: &str = "__Host-yuuka-session";
const COOKIE_DEV: &str = "yuuka-session";

/// `Cookie` ヘッダから指定名の値を取り出す。
fn cookie_value<'a>(parts: &'a Parts, name: &str) -> Option<&'a str> {
    let raw = parts.headers.get(COOKIE)?.to_str().ok()?;
    raw.split(';')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.trim(), v.trim()))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

/// `Authorization: Bearer <token>` を取り出す（`Bearer` は大小無視・既存正規表現に一致）。
fn bearer_token(parts: &Parts) -> Option<&str> {
    let raw = parts.headers.get(AUTHORIZATION)?.to_str().ok()?;
    let rest = raw
        .strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))?;
    let token = rest.trim();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

/// Cookie 優先 → Bearer の順でユーザーを解決する（未認証は `Ok(None)`）。
async fn resolve_user(parts: &Parts, state: &AppState) -> Result<Option<SessionUser>, AuthError> {
    // HTTPS 本番は `__Host-` を優先し、無ければ開発名も試す。
    let cookie_tok = cookie_value(parts, COOKIE_HOST).or_else(|| cookie_value(parts, COOKIE_DEV));
    if let Some(tok) = cookie_tok {
        if let Some(user) = state.auth.session_user(tok).await? {
            return Ok(Some(user));
        }
    }
    if let Some(tok) = bearer_token(parts) {
        if let Some(user) = state.auth.desktop_user(tok).await? {
            return Ok(Some(user));
        }
    }
    Ok(None)
}

/// 認証必須ユーザー（`auth: "user"` 相当）。未認証は 401。
#[derive(Debug, Clone)]
pub struct AuthenticatedUser(pub SessionUser);

impl FromRequestParts<AppState> for AuthenticatedUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match resolve_user(parts, state).await? {
            Some(user) => Ok(Self(user)),
            None => Err(ApiError(WebError::Unauthorized)),
        }
    }
}

/// 管理者必須（`auth: "admin"` 相当）。未認証は 401、非 admin は 403。
#[derive(Debug, Clone)]
pub struct AdminUser(pub SessionUser);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match resolve_user(parts, state).await? {
            Some(user) if user.role == Role::Admin => Ok(Self(user)),
            Some(_) => Err(ApiError(WebError::Forbidden)),
            None => Err(ApiError(WebError::Unauthorized)),
        }
    }
}

/// 任意認証（`auth: "none"` だがユーザーが居れば使う）。常に成功し、居なければ `None`。
#[derive(Debug, Clone)]
pub struct OptionalUser(pub Option<SessionUser>);

impl FromRequestParts<AppState> for OptionalUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(resolve_user(parts, state).await?))
    }
}
