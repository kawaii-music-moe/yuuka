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
use yuuka_core::{AuthError, DbError, WebError};

/// Node 互換のエラー本文を返す。内部エラーの詳細を message へ渡さないこと。
pub fn status_json(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(json!({ "success": false, "message": message })),
    )
        .into_response()
}

/// 入力不正（400）。
pub fn bad_request(message: &str) -> Response {
    status_json(StatusCode::BAD_REQUEST, message)
}

/// 対象が見つからない（404）。
pub fn not_found(message: &str) -> Response {
    status_json(StatusCode::NOT_FOUND, message)
}

/// 競合（409）。
pub fn conflict(message: &str) -> Response {
    status_json(StatusCode::CONFLICT, message)
}

/// 操作する権限がない（403）。
pub fn forbidden(message: &str) -> Response {
    status_json(StatusCode::FORBIDDEN, message)
}

/// 詳細を利用者へ漏らさず、ログを残して内部エラー（500）を返す。
#[track_caller]
pub fn server_error() -> Response {
    let caller = std::panic::Location::caller();
    tracing::error!(
        file = caller.file(),
        line = caller.line(),
        "API の内部エラー"
    );
    status_json(
        StatusCode::INTERNAL_SERVER_ERROR,
        "内部エラーが発生しました。",
    )
}

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

impl From<DbError> for ApiError {
    fn from(e: DbError) -> Self {
        // 内部詳細（SQL/パス等）は client_message で丸める。BUSY は一時的な上流障害
        // 相当（502）、それ以外は 500。ドメインハンドラは `?` で DbError を写像できる。
        let web = if e.is_busy() {
            WebError::Upstream
        } else {
            WebError::Internal
        };
        Self(web)
    }
}

/// 業務サービス（ToDo・家計・ノート・予定の各 `service.rs`）の失敗（D02 #126）。
///
/// 管理画面の `/api/*`・PWA の `/api/client/*`・LLM ツールの 3 入口が同じサービスを呼び、
/// それぞれの応答形式（`{success:false, message}`・`{message}`・ツールの `{success:false}`）へ
/// 変換する。入口ごとに検証やエラー分類を持たないための共通の型。
#[derive(Debug, thiserror::Error)]
pub enum DomainError {
    /// 入力が業務ルールに合わない（HTTP は 400）。メッセージは利用者へそのまま返してよい。
    #[error("{0}")]
    Invalid(String),
    /// DB 失敗（HTTP は 500、BUSY は 502。詳細はクライアントへ出さない）。
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<DomainError> for ApiError {
    fn from(e: DomainError) -> Self {
        match e {
            DomainError::Invalid(message) => Self(WebError::Validation(message)),
            DomainError::Db(e) => e.into(),
        }
    }
}

impl From<AuthError> for ApiError {
    fn from(e: AuthError) -> Self {
        // 認証層エラーを WebError へ写像。AuthError は #[non_exhaustive] のため別クレートの
        // match には `_` が要る。認証系の未知バリアントは安全側（401）へ倒す。
        let web = match e {
            AuthError::SessionInvalid | AuthError::TokenMalformed => WebError::Unauthorized,
            AuthError::Forbidden => WebError::Forbidden,
            // ストア到達不能は 502（未認証 401 と区別し、劣化縮退/監視に載せる）。
            AuthError::Backend => WebError::Upstream,
            _ => WebError::Unauthorized,
        };
        Self(web)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shared_errors_preserve_status_headers_and_body() {
        for (response, status) in [
            (bad_request("入力が不正です。"), StatusCode::BAD_REQUEST),
            (not_found("入力が不正です。"), StatusCode::NOT_FOUND),
            (conflict("入力が不正です。"), StatusCode::CONFLICT),
            (forbidden("入力が不正です。"), StatusCode::FORBIDDEN),
        ] {
            assert_eq!(response.status(), status);
            assert_eq!(response.headers()["content-type"], "application/json");
            let body = axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap();
            let expected = serde_json::to_vec(&json!({
                "success": false,
                "message": "入力が不正です。",
            }))
            .unwrap();
            assert_eq!(body.as_ref(), expected.as_slice());
        }
    }

    #[tokio::test]
    async fn internal_error_has_only_generic_message() {
        let response = server_error();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            body,
            json!({ "success": false, "message": "内部エラーが発生しました。" })
        );
    }
}
