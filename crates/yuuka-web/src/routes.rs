//! ルートハンドラ（Phase 1 増分1: `/api/me` のみ。以降の増分で拡張）。

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use rusqlite::{params, OptionalExtension};
use serde_json::json;
use yuuka_db::map_sqlite;
use yuuka_types::{Envelope, MeData, Role, SessionUser};

use crate::auth::AuthenticatedUser;
use crate::error::ApiError;
use crate::state::AppState;

/// `GET /api/me`（auth: user）。認証済みユーザーと規約 URL を返す。
///
/// セッション解決後に **SQLite を再取得**して最新の username/role を返し、ユーザーが消失して
/// いれば **404**（Node `authRoutes.ts` `/api/me` `getUserByDiscordId` parity）。成功時形状は
/// Node の `{ success:true, user, privacyPolicyUrl, termsUrl }` に一致（`Envelope<MeData>`）。
/// 未認証は extractor が 401、DB 障害は `ApiError`（BUSY=502 / その他 500）へ写像する。
pub async fn me(user: AuthenticatedUser, State(state): State<AppState>) -> Response {
    let discord_id = user.0.discord_id.clone();

    // Node は毎回 users を引き直す（セッション JSON のスナップショットを信頼しない）。
    let lookup_id = discord_id.clone();
    let row: Option<(String, String)> = match state
        .db
        .read
        .read(move |conn| {
            conn.query_row(
                "SELECT username, role FROM users WHERE discord_id = ?1",
                params![lookup_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
    {
        Ok(row) => row,
        Err(e) => return ApiError::from(e).into_response(),
    };

    let Some((username, role)) = row else {
        // ユーザー消失 → 404（Node と同一メッセージ・`{success:false,message}`）。
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "success": false, "message": "ユーザーが見つかりません。" })),
        )
            .into_response();
    };

    // legal URL は system_settings（admin 保存値）を優先し、無ければ config へ（Node publicLegalUrls）。
    let (privacy_policy_url, terms_url) = crate::settings::public_legal_urls(&state).await;
    Json(Envelope::ok(MeData {
        // DB 最新値で返す（role は DB 権威・Node の `user.role || "user"` 相当）。
        user: SessionUser {
            discord_id,
            username,
            role: parse_role(&role),
        },
        privacy_policy_url,
        terms_url,
    }))
    .into_response()
}

/// `users.role`（'user'|'admin'）を [`Role`] へ写像（未知は user・Node の `|| "user"` 相当）。
fn parse_role(raw: &str) -> Role {
    if raw.eq_ignore_ascii_case("admin") {
        Role::Admin
    } else {
        Role::User
    }
}
