//! `/api/client/*` 用エラー応答（Node `sendJson(res, code, { message })` パリティ・`success` 包みなし）。
//!
//! 一般ダッシュボード（`yuuka_web::ApiError`）は `{success:false, message}` を返すが、Node
//! `clientRoutes.ts` は素の `{ message }` を返しており、PWA はこの形状を前提にしていないものの
//! （現状 `!res.ok` のみで判定）、参照実装（Node）と揃えておく。

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use yuuka_core::DbError;

use crate::dto::MessageBody;

/// `/api/client/*` ハンドラの戻り値エラー型。
#[derive(Debug)]
pub struct ClientApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ClientApiError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    /// 同一ユーザーのチャット送信ターンが既に進行中（issue #41 PR #75 レビュー・P1: 非同期配信の
    /// 重複ターン防止）。
    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
        }
    }

    /// 想定外の内部エラー。詳細はログにだけ残し、クライアントには `internal` だけ返す。
    pub fn internal(detail: impl std::fmt::Display) -> Self {
        tracing::warn!(error = %detail, "client-api: internal error");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "internal".to_owned(),
        }
    }
}

impl IntoResponse for ClientApiError {
    fn into_response(self) -> Response {
        (self.status, Json(MessageBody::new(self.message))).into_response()
    }
}

impl From<DbError> for ClientApiError {
    fn from(e: DbError) -> Self {
        // 内部詳細（SQL/パス等）はクライアントへ漏らさない。BUSY は一時的な上流障害相当（502）、
        // それ以外は 500（`yuuka_web::ApiError` の `DbError` 写像と同方針）。バリデーション由来の
        // `DbError::Operation`（例: persona prompt 上限超過）はここでは判別せず 500 に丸める
        // （呼び出し側で事前に長さ検証し、到達しないようにする）。
        let status = if e.is_busy() {
            StatusCode::BAD_GATEWAY
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        tracing::warn!(error = %e, "client-api: db error");
        Self {
            status,
            message: if status == StatusCode::BAD_GATEWAY {
                "upstream unavailable".to_owned()
            } else {
                "internal".to_owned()
            },
        }
    }
}
