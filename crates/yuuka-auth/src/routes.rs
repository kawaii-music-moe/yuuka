//! sk-account を唯一の本人認証元として使う認証ルート。
//!
//! Yuuka と sk-account は親ドメインが異なるため Cookie は共有しない。ブラウザを
//! sk-account の認可入口へ送り、一回限り・短命の code をバックチャネルで交換した後、
//! Yuuka 自身のホスト専用 Cookie セッションを発行する。

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{Extension, FromRequestParts, Query, State};
use axum::http::header::{LOCATION, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use yuuka_types::SessionUser;
use yuuka_web::{ApiError, AppState, AuthenticatedUser};

use crate::{users, SessionStore};

const COOKIE_HOST: &str = "__Host-yuuka-session";
const COOKIE_DEV: &str = "yuuka-session";

pub struct AuthRuntime {
    sessions: SessionStore,
    session_ttl_secs: u64,
    sk_account_public_url: String,
    sk_account_internal_url: String,
    admin_discord_ids: Vec<String>,
    http: reqwest::Client,
}

impl AuthRuntime {
    #[must_use]
    pub fn new(
        sessions: SessionStore,
        session_ttl_days: u32,
        sk_account_public_url: String,
        sk_account_internal_url: String,
        admin_discord_ids: Vec<String>,
    ) -> Self {
        Self {
            sessions,
            session_ttl_secs: u64::from(session_ttl_days) * 24 * 60 * 60,
            sk_account_public_url: sk_account_public_url.trim_end_matches('/').to_owned(),
            sk_account_internal_url: sk_account_internal_url.trim_end_matches('/').to_owned(),
            admin_discord_ids,
            http: reqwest::Client::new(),
        }
    }
}

pub fn routes(runtime: Arc<AuthRuntime>) -> Router<AppState> {
    Router::new()
        .route("/api/auth/sk/start", get(start))
        .route("/api/auth/sk/callback", get(callback))
        .route("/api/logout", post(logout))
        .route("/api/users", get(users_route))
        .layer(Extension(runtime))
}

async fn start(Extension(rt): Extension<Arc<AuthRuntime>>) -> Response {
    redirect(&format!(
        "{}/api/services/yuuka/authorize",
        rt.sk_account_public_url
    ))
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkAccount {
    id: String,
    discord_user_id: String,
    display_name: String,
}

#[derive(Deserialize)]
struct ExchangeResponse {
    account: SkAccount,
}

async fn callback(
    State(state): State<AppState>,
    Extension(rt): Extension<Arc<AuthRuntime>>,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let Some(code) = query.code.filter(|v| !v.is_empty()) else {
        return error(StatusCode::BAD_REQUEST, "認証コードがありません。");
    };
    let response = match rt
        .http
        .post(format!(
            "{}/api/services/yuuka/exchange",
            rt.sk_account_internal_url
        ))
        .json(&json!({ "code": code }))
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => response,
        Ok(_) => {
            return error(
                StatusCode::UNAUTHORIZED,
                "認証コードが無効または期限切れです。",
            )
        }
        Err(e) => {
            tracing::error!(error = %e, "sk-account code exchange failed");
            return error(StatusCode::BAD_GATEWAY, "sk-accountに接続できません。");
        }
    };
    let exchange = match response.json::<ExchangeResponse>().await {
        Ok(value) => value,
        Err(e) => {
            tracing::error!(error = %e, "invalid sk-account exchange response");
            return error(StatusCode::BAD_GATEWAY, "sk-accountの応答が不正です。");
        }
    };

    let role = match users::upsert_from_sk_account(
        &state.db,
        &exchange.account.id,
        &exchange.account.discord_user_id,
        &exchange.account.display_name,
        &rt.admin_discord_ids,
    )
    .await
    {
        Ok(role) => role,
        Err(e) => return ApiError::from(e).into_response(),
    };
    let user = SessionUser {
        discord_id: exchange.account.discord_user_id,
        username: exchange.account.display_name,
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
    let cookie = build_session_cookie(state.config.https, &token, ttl_max_age(rt.session_ttl_secs));
    let mut response = redirect("/");
    response.headers_mut().insert(SET_COOKIE, cookie);
    response
}

async fn logout(
    State(state): State<AppState>,
    Extension(rt): Extension<Arc<AuthRuntime>>,
    token: SessionCookieToken,
) -> Response {
    if let Some(token) = token.0 {
        rt.sessions.destroy(&token).await;
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

fn extract_cookie_token(parts: &Parts, https: bool) -> Option<String> {
    let raw = parts
        .headers
        .get(axum::http::header::COOKIE)?
        .to_str()
        .ok()?;
    let preferred = if https { COOKIE_HOST } else { COOKIE_DEV };
    raw.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == preferred && !value.is_empty()).then(|| value.to_owned())
    })
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
