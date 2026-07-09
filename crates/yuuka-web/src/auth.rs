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
///
/// Node `parseCookies`(httpHelpers.ts:26-40) と一致させる: 同名が複数あれば**最後が勝つ**、
/// 値は最初の `=` 以降すべて（`split_once` が担保）。トークンは base64url で予約文字を
/// 含まないため percent-decode は省略する（§11.3。将来 % を含む cookie を扱うなら要追加）。
fn cookie_value<'a>(parts: &'a Parts, name: &str) -> Option<&'a str> {
    let raw = parts.headers.get(COOKIE)?.to_str().ok()?;
    raw.split(';')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.trim(), v.trim()))
        .rfind(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

/// `Authorization: Bearer <token>` を取り出す。
///
/// 既存 Node の `/^Bearer\s+(.+)$/i` と一致させる: スキーム語 `Bearer` は**大小無視**、
/// スキームとトークンの区切りは**1 個以上の空白**（スペース/タブ）。
fn bearer_token(parts: &Parts) -> Option<&str> {
    let raw = parts.headers.get(AUTHORIZATION)?.to_str().ok()?;
    // 先頭 6 バイト = "Bearer"（ASCII 境界）。非 ASCII 先頭や 6 バイト未満は None。
    let (scheme, rest) = raw.split_at_checked(6)?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    // スキーム直後に空白が最低 1 個必要（`\s+`）。
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let token = rest.trim();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

/// Cookie 優先 → Bearer の順でユーザーを解決する（未認証・縮退時とも `Ok(None)`）。
///
/// **M-1（認証縮退）**: Node の `getSessionUser`/`getBearerUser` は各ストア呼び出しを
/// try/catch で包み、実行時障害（Redis 断・SQLite 障害）を **null に潰して次経路へ継続**する
/// （[`src/server/httpHelpers.ts`] `getSessionUser`/`getBearerUser`/`resolveRequestUser`）。
/// これを移植し、`AuthError::Backend`（ストア到達不能）を `?` で 502 伝播させない。
/// もし伝播させると、有効な Bearer を持つデスクトップクライアントや `OptionalUser` 任意認証
/// ルートまで Redis 断で 502 に巻き込まれる。縮退は無音にせず `tracing::warn!` で必ず残す
/// （「全員 Cookie 認証が静かに効かなくなる」障害を追跡可能にする）。
async fn resolve_user(parts: &Parts, state: &AppState) -> Result<Option<SessionUser>, AuthError> {
    // 既存 Node(httpHelpers.ts:52-57) と一致: **HTTPS 本番は `__Host-` のみ受理**し、
    // 開発時のみ非 prefix 名も許す（HTTPS で非 `__Host-` を受理すると `__Host-` の
    // cookie 上書き防御が無効化＝セキュリティ後退になる）。
    let cookie_tok = if state.config.https {
        cookie_value(parts, COOKIE_HOST)
    } else {
        cookie_value(parts, COOKIE_HOST).or_else(|| cookie_value(parts, COOKIE_DEV))
    };
    if let Some(tok) = cookie_tok {
        match state.auth.session_user(tok).await {
            Ok(Some(user)) => return Ok(Some(user)),
            Ok(None) => {} // セッション無効 → Bearer を試す（Node の null 継続）。
            Err(e) => {
                // 認証ストア到達不能（Redis 断等）。502 化せず縮退して Bearer 経路へ継続。
                tracing::warn!(error = %e, "session store 到達不能: Cookie 認証を縮退し Bearer にフォールバック");
            }
        }
    }
    if let Some(tok) = bearer_token(parts) {
        match state.auth.desktop_user(tok).await {
            Ok(Some(user)) => return Ok(Some(user)),
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(error = %e, "desktop token store 到達不能: Bearer 認証を縮退");
            }
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
