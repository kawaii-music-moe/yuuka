//! `GET /api/status` — ユーザー個別ダッシュボードの集計（Node `settingsRoutes.ts` `/api/status` パリティ）。
//!
//! 秘書業務データは `(user_id, bot_id)` スコープで集計する。`?botId=` はアクセス検証を通ったときのみ採用し、
//! それ以外は `system_default` へフォールバックする（`resolveBotId` と一致）。Google カレンダー一覧は連携済み
//! （primary アカウントあり）のときのみ [`CalendarPort`](yuuka_google::CalendarPort) 経由で取得する。

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::json;
use yuuka_web::{has_bot_access, ApiError, AppState, AuthenticatedUser};

use crate::repo;
use crate::SettingsRuntime;

#[derive(Debug, Deserialize)]
pub(crate) struct StatusQuery {
    #[serde(rename = "botId")]
    bot_id: Option<String>,
}

/// クレデンシャルの安全なマスキング（Node `mask`）。空/NULL は「未設定」、8 文字以下は「****」、
/// それ以外は先頭 4 + `...` + 末尾 4。
fn mask(value: Option<&str>) -> String {
    match value {
        None => "未設定".to_owned(),
        Some("") => "未設定".to_owned(),
        Some(s) if s.chars().count() <= 8 => "****".to_owned(),
        Some(s) => {
            let chars: Vec<char> = s.chars().collect();
            let head: String = chars.iter().take(4).collect();
            let tail: String = chars.iter().skip(chars.len() - 4).collect();
            format!("{head}...{tail}")
        }
    }
}

pub(crate) async fn status(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<SettingsRuntime>>,
    State(state): State<AppState>,
    Query(q): Query<StatusQuery>,
) -> Result<Response, ApiError> {
    let user_id = &user.0.discord_id;

    // ダッシュボードは選択中の Bot スコープで集計する（アクセス不可は system_default）。
    let bot_id = match q.bot_id.as_deref() {
        Some(b) if !b.is_empty() && has_bot_access(&state.db, user_id, b).await? => b.to_owned(),
        _ => "system_default".to_owned(),
    };

    let snap = repo::status_snapshot(&state.db, user_id, &bot_id).await?;

    // Google 連携状況（primary アカウント基準）。
    let primary = yuuka_google::repo::get_primary_account(&state.db, user_id).await?;
    let google_linked = primary.is_some();
    let google_calendar_id = primary
        .as_ref()
        .and_then(|p| p.calendar_id.clone())
        .unwrap_or_else(|| "未設定".to_owned());
    let account_count = yuuka_google::repo::count_accounts(&state.db, user_id).await?;
    let calendars = if google_linked {
        rt.calendar.cached_calendars(user_id).await
    } else {
        Vec::new()
    };

    let notify_target = if snap.notify_target_type == "channel" && snap.notify_target_id.is_some() {
        json!({ "type": "channel", "id": snap.notify_target_id })
    } else {
        json!({ "type": "dm" })
    };

    let gemini_model = snap
        .gemini_model
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "gemini-3.1-flash-lite".to_owned());
    let gemini_api_key = mask(snap.gemini_key_present.then_some("configured"));

    Ok(Json(json!({
        "success": true,
        "user": {
            "discordId": user_id,
            "username": snap.username.clone().unwrap_or_else(|| user_id.clone()),
        },
        "stats": {
            "tasks": snap.tasks,
            "pendingTasks": snap.pending_tasks,
            "pendingPriorities": {
                "0": snap.priorities[0],
                "1": snap.priorities[1],
                "2": snap.priorities[2],
            },
            "schedules": snap.schedules,
            "scheduleTrend": snap.schedule_trend,
            "expenses": snap.expenses,
            "expenseTrend": snap.expense_trend,
        },
        "config": {
            "dbPath": state.config.db_path,
            "reminderCron": state.config.reminder_cron,
            "googleCalendarId": google_calendar_id,
            "googleCalendars": calendars,
            "googleLinked": google_linked,
            "googleAccountCount": account_count,
            "geminiModel": gemini_model,
            "geminiApiKey": gemini_api_key,
            "backupEnabled": snap.backup_enabled,
            "backupFolderId": mask(snap.backup_folder_id.as_deref()),
            "backupIntervalHours": snap.backup_interval_hours,
            "backupGenerations": snap.backup_generations,
            "backupLastRunAt": snap.backup_last_run_at,
            "richReplyEnabled": snap.rich_reply_enabled,
            "remindDefaultMinutes": snap.remind_default_minutes,
            "notifyTarget": notify_target,
            "activePersonaId": snap.active_persona_id,
        },
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::mask;

    #[test]
    fn mask_parity() {
        // 空/NULL → 未設定。
        assert_eq!(mask(None), "未設定");
        assert_eq!(mask(Some("")), "未設定");
        // 8 文字以下 → ****。
        assert_eq!(mask(Some("12345678")), "****");
        // 8 文字超 → 先頭 4 + ... + 末尾 4（Node `mask("configured")`）。
        assert_eq!(mask(Some("configured")), "conf...ured");
        assert_eq!(mask(Some("FOLDER_ABCDEFG")), "FOLD...DEFG");
    }
}
