//! OIDC プロバイダーを唯一の本人認証元として使う認証ルート（標準は sk-account）。
//!
//! ブラウザをプロバイダーの認可エンドポイントへ送り、戻ってきた一回限りの code をバックチャネルで
//! 交換して ID トークンを検証した後、Yuuka 自身のホスト専用 Cookie セッションを発行する。
//! 流れと検証は [`crate::oidc`] に置き、ここは HTTP の入口と Cookie だけを扱う。
//!
//! ログイン CSRF（攻撃者自身の code を持つ callback へ被害者を送り込み、攻撃者の
//! アカウントでログインさせる）を防ぐため、`start` で発行した `state` を短命 Cookie と
//! 認可 URL の両方に載せ、`callback` で一致を確かめてから code を交換する。
//!
//! プロバイダーでログアウトすると `POST /api/auth/oidc/backchannel-logout` に logout_token が届くので、
//! 同じプロバイダーのセッション（`sid`）から作った Yuuka のセッションを消す。

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{Extension, Form, FromRequestParts, Query, State};
use axum::http::header::{CACHE_CONTROL, LOCATION, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use yuuka_types::SessionUser;
use yuuka_web::{ApiError, AppState, AuthenticatedUser};

use crate::oidc::{LoginError, LogoutTarget, OidcClient};
use crate::{users, SessionStore};

const COOKIE_HOST: &str = "__Host-yuuka-session";
const COOKIE_DEV: &str = "yuuka-session";
const STATE_COOKIE_HOST: &str = "__Host-yuuka-oidc-state";
const STATE_COOKIE_DEV: &str = "yuuka-oidc-state";

pub struct AuthRuntime {
    sessions: SessionStore,
    session_ttl_secs: u64,
    admin_discord_ids: Vec<String>,
    oidc: Option<Arc<OidcClient>>,
}

impl AuthRuntime {
    /// `oidc` が `None`（`OIDC_CLIENT_SECRET` 未設定）ならログインできない。
    #[must_use]
    pub fn new(
        sessions: SessionStore,
        session_ttl_days: u32,
        admin_discord_ids: Vec<String>,
        oidc: Option<Arc<OidcClient>>,
    ) -> Self {
        Self {
            sessions,
            session_ttl_secs: u64::from(session_ttl_days) * 24 * 60 * 60,
            admin_discord_ids,
            oidc,
        }
    }
}

pub fn routes(runtime: Arc<AuthRuntime>) -> Router<AppState> {
    Router::new()
        .route("/api/auth/oidc/provider", get(provider))
        .route("/api/auth/oidc/start", get(start))
        .route("/api/auth/oidc/callback", get(callback))
        .route(
            "/api/auth/oidc/backchannel-logout",
            post(backchannel_logout),
        )
        .route("/api/logout", post(logout))
        .route("/api/users", get(users_route))
        .layer(Extension(runtime))
}

/// ログイン画面向け: ログイン先の表示名と、ログインできる状態か。
async fn provider(Extension(rt): Extension<Arc<AuthRuntime>>) -> Response {
    Json(json!({
        "success": true,
        "enabled": rt.oidc.is_some(),
        "label": rt.oidc.as_ref().map(|oidc| oidc.label()),
    }))
    .into_response()
}

async fn start(
    State(state): State<AppState>,
    Extension(rt): Extension<Arc<AuthRuntime>>,
) -> Response {
    let Some(oidc) = &rt.oidc else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "ログインが設定されていません。",
        );
    };
    let (authorize_url, login_state) = match oidc.start_login().await {
        Ok(started) => started,
        Err(e) => {
            tracing::error!(error = %e.0, "OIDC のログインを開始できない");
            return error(StatusCode::BAD_GATEWAY, "ログイン先に接続できません。");
        }
    };
    let mut response = redirect(&authorize_url);
    let max_age = i64::try_from(crate::oidc::LOGIN_TTL.as_secs()).unwrap_or(i64::MAX);
    response.headers_mut().insert(
        SET_COOKIE,
        build_state_cookie(state.config.https, &login_state, max_age),
    );
    response
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    iss: Option<String>,
    error: Option<String>,
}

async fn callback(
    State(state): State<AppState>,
    Extension(rt): Extension<Arc<AuthRuntime>>,
    Query(query): Query<CallbackQuery>,
    headers: HeaderMap,
) -> Response {
    let https = state.config.https;
    let mut response = exchange_and_login(&state, &rt, query, &headers).await;
    // 成否に関わらず state は一回限りにする。
    response
        .headers_mut()
        .append(SET_COOKIE, build_state_cookie(https, "", 0));
    response
}

async fn exchange_and_login(
    state: &AppState,
    rt: &AuthRuntime,
    query: CallbackQuery,
    headers: &HeaderMap,
) -> Response {
    let Some(oidc) = &rt.oidc else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "ログインが設定されていません。",
        );
    };
    let expected = cookie_value(headers, state_cookie_name(state.config.https));
    let login_state = match (expected, query.state.as_deref()) {
        (Some(expected), Some(actual))
            if constant_time_eq(expected.as_bytes(), actual.as_bytes()) =>
        {
            expected
        }
        _ => {
            return error(
                StatusCode::BAD_REQUEST,
                "ログインの確認に失敗しました。もう一度ログインしてください。",
            )
        }
    };
    let Some(code) = query.code.filter(|v| !v.is_empty()) else {
        if let Some(e) = query.error {
            tracing::warn!(error = %e, "OIDC プロバイダーがログインを拒否した");
        }
        return error(StatusCode::BAD_REQUEST, "認証コードがありません。");
    };
    let outcome = match oidc
        .finish_login(&login_state, &code, query.iss.as_deref())
        .await
    {
        Ok(outcome) => outcome,
        Err(LoginError::Rejected(message)) => return error(StatusCode::UNAUTHORIZED, message),
        Err(LoginError::Unavailable(e)) => {
            tracing::error!(error = %e.0, "OIDC のログインを完了できない");
            return error(StatusCode::BAD_GATEWAY, "ログイン先に接続できません。");
        }
    };
    let identity = outcome.identity;

    let role = match users::upsert_from_sk_account(
        &state.db,
        &identity.subject,
        &identity.discord_id,
        &identity.display_name,
        &rt.admin_discord_ids,
    )
    .await
    {
        Ok(role) => role,
        Err(e) => return ApiError::from(e).into_response(),
    };
    let user = SessionUser {
        discord_id: identity.discord_id,
        username: identity.display_name,
        role,
    };
    let token = match rt.sessions.create(&user, rt.session_ttl_secs).await {
        Ok(token) => token,
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "セッションを作成できません。",
            )
        }
    };
    rt.sessions
        .put_oidc_link(
            &crate::sha256_hex(&token),
            &outcome.link,
            rt.session_ttl_secs,
        )
        .await;
    let cookie = build_session_cookie(state.config.https, &token, ttl_max_age(rt.session_ttl_secs));
    let mut response = redirect("/");
    response.headers_mut().insert(SET_COOKIE, cookie);
    response
}

#[derive(Deserialize)]
struct BackchannelLogoutForm {
    logout_token: Option<String>,
}

/// OIDC Back-Channel Logout: プロバイダーでログアウトしたセッションを消す。
async fn backchannel_logout(
    State(state): State<AppState>,
    Extension(rt): Extension<Arc<AuthRuntime>>,
    Form(form): Form<BackchannelLogoutForm>,
) -> Response {
    let status = match (&rt.oidc, form.logout_token) {
        (None, _) => StatusCode::NOT_IMPLEMENTED,
        (Some(_), None) => StatusCode::BAD_REQUEST,
        (Some(oidc), Some(logout_token)) => match oidc.verify_logout_token(&logout_token).await {
            None => StatusCode::BAD_REQUEST,
            Some(LogoutTarget::Session(sid)) => {
                rt.sessions.destroy_by_sid(&sid).await;
                StatusCode::OK
            }
            Some(LogoutTarget::Subject(sub)) => {
                match users::discord_id_by_sk_account(&state.db, &sub).await {
                    Ok(Some(discord_id)) => {
                        rt.sessions.destroy_all_for_user(&discord_id).await;
                        StatusCode::OK
                    }
                    Ok(None) => StatusCode::OK,
                    Err(e) => return ApiError::from(e).into_response(),
                }
            }
        },
    };
    let mut response = status.into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn logout(
    State(state): State<AppState>,
    Extension(rt): Extension<Arc<AuthRuntime>>,
    token: SessionCookieToken,
) -> Response {
    if let Some(token) = token.0 {
        let hash = crate::sha256_hex(&token);
        let link = rt.sessions.oidc_link(&hash, rt.session_ttl_secs).await;
        rt.sessions.destroy_hash(&hash).await;
        // プロバイダー側のリフレッシュトークンも失効させる（応答は待たない）。
        if let (Some(oidc), Some(link)) = (rt.oidc.clone(), link) {
            tokio::spawn(async move { oidc.revoke(&link).await });
        }
    }
    let mut response = Json(json!({ "success": true })).into_response();
    response
        .headers_mut()
        .insert(SET_COOKIE, build_session_cookie(state.config.https, "", 0));
    response
}

async fn users_route(user: AuthenticatedUser, State(state): State<AppState>) -> Response {
    match users::get_user_by_discord_id(&state.db, &user.0.discord_id).await {
        Ok(Some(found)) => Json(json!({
            "success": true,
            "user": { "discordId": found.discord_id, "username": found.username, "role": found.role }
        }))
        .into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "ユーザーが見つかりません。"),
        Err(e) => ApiError::from(e).into_response(),
    }
}

fn redirect(location: &str) -> Response {
    let mut response = StatusCode::FOUND.into_response();
    if let Ok(value) = HeaderValue::from_str(location) {
        response.headers_mut().insert(LOCATION, value);
    }
    response
}

fn error(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(json!({ "success": false, "message": message })),
    )
        .into_response()
}

fn ttl_max_age(ttl_secs: u64) -> i64 {
    i64::try_from(ttl_secs).unwrap_or(i64::MAX)
}

#[must_use]
pub fn build_session_cookie(https: bool, token: &str, max_age: i64) -> HeaderValue {
    let name = if https { COOKIE_HOST } else { COOKIE_DEV };
    let secure = if https { "; Secure" } else { "" };
    let cookie =
        format!("{name}={token}; Path=/; HttpOnly; SameSite=Lax{secure}; Max-Age={max_age}");
    HeaderValue::from_str(&cookie).unwrap_or_else(|_| HeaderValue::from_static(""))
}

fn state_cookie_name(https: bool) -> &'static str {
    if https {
        STATE_COOKIE_HOST
    } else {
        STATE_COOKIE_DEV
    }
}

/// `__Host-` 接頭辞は `Path=/` を要求するため、state Cookie もサイト全体に付ける。
/// 兄弟サブドメインから上書きされない（`__Host-`）ことを経路の絞り込みより優先する。
fn build_state_cookie(https: bool, value: &str, max_age: i64) -> HeaderValue {
    let name = state_cookie_name(https);
    let secure = if https { "; Secure" } else { "" };
    let cookie =
        format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax{secure}; Max-Age={max_age}");
    HeaderValue::from_str(&cookie).unwrap_or_else(|_| HeaderValue::from_static(""))
}

/// 長さは秘密ではないので先に比べ、中身は早期終了せずに全バイトを比べる。
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff = a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y));
    std::hint::black_box(diff) == 0
}

fn cookie_value(headers: &HeaderMap, wanted: &str) -> Option<String> {
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == wanted && !value.is_empty()).then(|| value.to_owned())
    })
}

fn extract_cookie_token(parts: &Parts, https: bool) -> Option<String> {
    cookie_value(&parts.headers, if https { COOKIE_HOST } else { COOKIE_DEV })
}

pub struct SessionCookieToken(pub Option<String>);

impl FromRequestParts<AppState> for SessionCookieToken {
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(extract_cookie_token(parts, state.config.https)))
    }
}
