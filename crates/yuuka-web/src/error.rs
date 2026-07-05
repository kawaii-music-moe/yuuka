//! HTTP エラー応答型（`WebError` → axum レスポンスの写像）。
//!
//! **孤児規則**により core の [`WebError`] へ直接 axum の `IntoResponse` を実装できないため、
//! 本 crate 所有の [`ApiError`] でラップして写像する。本文は Node 互換の
//! `{ success: false, message }` で、内部 Display（`DbError` 等の機微）は
//! [`WebError::client_message`] により漏らさない。

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use yuuka_core::{AuthError, WebError};

/// yuuka-web が所有する HTTP エラー応答型。
#[derive(Debug)]
pub struct ApiError(pub WebError);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = Json(json!({ "success": false, "message": self.0.client_message() }));
        (status, body).into_response()
    }
}

impl From<WebError> for ApiError {
    fn from(e: WebError) -> Self {
        Self(e)
    }
}

impl From<AuthError> for ApiError {
    fn from(e: AuthError) -> Self {
        // 認証層エラーを WebError へ写像。AuthError は #[non_exhaustive] のため別クレートの
        // match には `_` が要る。認証系の未知バリアントは安全側（401）へ倒す。
        let web = match e {
            AuthError::SessionInvalid | AuthError::TokenMalformed => WebError::Unauthorized,
            AuthError::Forbidden => WebError::Forbidden,
            _ => WebError::Unauthorized,
        };
        Self(web)
    }
}
