//! `GET/POST /api/settings/discord` — 独自 Bot の Discord トークン設定（Node `settingsRoutes.ts` パリティ）。
//!
//! トークン設定状態の閲覧/変更はオーナー（`system_default` は Admin）のみ。トークンは
//! [`SystemCrypto`](yuuka_crypto::SystemCrypto) で暗号化保存し、変更時は runtime シーム経由で Bot を
//! （再）起動する（gateway 未配線時は no-op・DB 効果は常に働く）。

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use yuuka_core::WebError;
use yuuka_web::{ApiError, AppState, AuthenticatedUser};

use crate::repo::{self, BotDiscordRow};
use crate::SettingsRuntime;

#[derive(Debug, Deserialize)]
pub(crate) struct BotIdQuery {
    #[serde(rename = "botId")]
    bot_id: Option<String>,
}

fn forbidden(message: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({ "success": false, "message": message })),
    )
        .into_response()
}

/// `?botId=` を解決する（非空なら採用・空/欠落は `system_default`・Node と一致）。
fn resolve_bot_id(raw: Option<&str>) -> String {
    match raw {
        Some(b) if !b.is_empty() => b.to_owned(),
        _ => "system_default".to_owned(),
    }
}

// ─── GET: トークン設定状態の閲覧 ─────────────────────────────────────────────

pub(crate) async fn get_discord(
    user: AuthenticatedUser,
    State(state): State<AppState>,
    Query(q): Query<BotIdQuery>,
) -> Result<Response, ApiError> {
    let user_id = &user.0.discord_id;
    let bot_id = resolve_bot_id(q.bot_id.as_deref());
    let row = repo::get_bot_discord(&state.db, &bot_id).await?;

    // トークン設定状態はオーナー（system_default は Admin）のみ閲覧可能（POST と同一権限）。
    if bot_id == "system_default" {
        if !repo::is_admin(&state.db, user_id).await? {
            return Ok(forbidden(
                "システムBotのDiscord設定は管理者のみ閲覧できます。",
            ));
        }
    } else {
        match &row {
            Some(r) if &r.user_id == user_id => {}
            _ => return Ok(forbidden("Botのトークン設定はオーナーのみが閲覧できます。")),
        }
    }

    let has_token = row.as_ref().is_some_and(BotDiscordRow::has_token);
    Ok(Json(json!({
        "success": true,
        "hasToken": has_token,
        "tokenMasked": if has_token { "••••••••••••" } else { "" },
    }))
    .into_response())
}

// ─── POST: トークンの保存/クリア + Bot 再起動 ────────────────────────────────

#[derive(Debug, Deserialize)]
pub(crate) struct DiscordInput {
    #[serde(rename = "botId")]
    bot_id: Option<Value>,
    token: Option<Value>,
}

pub(crate) async fn post_discord(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<SettingsRuntime>>,
    State(state): State<AppState>,
    Json(body): Json<DiscordInput>,
) -> Result<Response, ApiError> {
    let user_id = &user.0.discord_id;
    // Node: `typeof botId === "string" && botId ? botId : "system_default"`。
    let bot_id = match body.bot_id.as_ref().and_then(Value::as_str) {
        Some(b) if !b.is_empty() => b.to_owned(),
        _ => "system_default".to_owned(),
    };
    // Node: `typeof token === "string" ? token : ""`。
    let token = body
        .token
        .as_ref()
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();

    let current = repo::get_bot_discord(&state.db, &bot_id).await?;

    if bot_id == "system_default" {
        if !repo::is_admin(&state.db, user_id).await? {
            return Ok(forbidden(
                "システムBotのDiscord設定は管理者のみ変更可能です。",
            ));
        }
    } else {
        match &current {
            Some(r) if &r.user_id == user_id => {}
            _ => return Ok(forbidden("Botのトークンはオーナーのみが変更できます。")),
        }
    }

    let has_current_token = current
        .as_ref()
        .and_then(|r| r.token_encrypted.as_deref())
        .is_some_and(|s| !s.is_empty());

    let mut encrypted: Option<String> = None;
    let mut iv: Option<String> = None;
    let mut tag: Option<String> = None;
    let mut token_changed = false;
    let mut token_cleared = false;

    if token.trim().is_empty() {
        // 空欄 → クリア（既存トークンがあるときのみ変更扱い）。
        if has_current_token {
            token_changed = true;
            token_cleared = true;
        }
    } else if token.starts_with("••••") {
        // マスク → 変更なし（既存 3 列を書き戻す）。
        if let Some(r) = &current {
            encrypted = r.token_encrypted.clone();
            iv = r.token_iv.clone();
            tag = r.token_tag.clone();
        }
    } else {
        // 新しいトークン → 暗号化して保存。
        let Some(crypto) = rt.crypto.as_ref() else {
            return Err(ApiError(WebError::Internal));
        };
        let enc = crypto
            .encrypt_text(token.trim())
            .map_err(|_| ApiError(WebError::Internal))?;
        encrypted = Some(enc.encrypted);
        iv = Some(enc.iv);
        tag = Some(enc.auth_tag);
        token_changed = true;
    }

    repo::update_bot_discord_token(
        &state.db,
        &bot_id,
        encrypted.clone(),
        iv.clone(),
        tag.clone(),
    )
    .await?;
    let detail = if token_cleared {
        "cleared"
    } else if token_changed {
        "updated"
    } else {
        "unchanged"
    };
    yuuka_auth::audit::add_audit_log(
        &state.db,
        user_id,
        "bot.token_change",
        Some(&bot_id),
        Some(detail),
    )
    .await;

    // クリア時は稼働中 Bot を停止する。
    if token_cleared {
        rt.bots.stop(&bot_id).await;
    }

    // 変更時は Bot を（再）起動する。suspend は DB 観測可能なので明示メッセージ、runtime 効果は
    // fire-and-forget（Null シーム時は no-op・admin default-bot/token と同方針）。
    let mut startup_message = "";
    if token_changed && encrypted.is_some() {
        let suspended = current.as_ref().is_some_and(|r| r.suspended);
        if suspended {
            startup_message = " このBotは管理者により停止処分中のため、起動できません。";
        } else if bot_id == "system_default" {
            if let (Some(crypto), Some(e), Some(i), Some(t)) =
                (rt.crypto.as_ref(), &encrypted, &iv, &tag)
            {
                if let Ok(plain) = crypto.decrypt_text(e, i, t) {
                    rt.bots.restart_default(&plain).await;
                }
            }
        } else {
            rt.bots.start_custom(&bot_id).await;
        }
    }

    let message = format!("設定を保存しました。{startup_message}")
        .trim()
        .to_owned();
    Ok(Json(json!({ "success": true, "message": message })).into_response())
}
