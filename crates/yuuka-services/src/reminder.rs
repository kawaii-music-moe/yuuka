//! リマインドエンジン（§3.3.2・現行 `reminderEngine.ts`）。
//!
//! 毎分＋起動直後（取りこぼし復帰・§10）に 3 種を処理する:
//!   1. 時刻指定リマインド（`reminders` の期限到来 pending・繰り返しは cron 式で再スケジュール）
//!   2. ToDo 期限リマインド（24 時間以内に迫った未通知 ToDo）
//!   3. 予定リマインド（Google カレンダー同期予定のイベント前通知）
//!
//! 送信失敗はマークせず pending/未通知のまま残し、次 tick で再試行する（§10 復帰と同経路）。
//! 逐次 await ループのため Node の `ticking` 多重起動防止は不要（[`crate::schedule::run_cron`]）。

use async_trait::async_trait;
use chrono::Local;
use yuuka_core::{BotId, UserId};
use yuuka_reminder::cron::DueReminder;
use yuuka_reminder::repo::ReminderRepo;
use yuuka_schedule::repo::ScheduleRepo;
use yuuka_todo::repo::TodoRepo;

use crate::context::ServiceContext;
use crate::cron_util;
use crate::notifier::{Notification, NotifyTarget};
use crate::schedule::{CronService, Schedule};

/// リマインドエンジンサービス。
pub struct ReminderService;

#[async_trait]
impl CronService for ReminderService {
    fn name(&self) -> &'static str {
        "reminder"
    }

    fn schedule(&self) -> Schedule {
        Schedule::EveryMinute // config.reminderCron 既定 "* * * * *"
    }

    async fn tick(&self, ctx: &ServiceContext) {
        if let Err(e) = process_due_reminders(ctx).await {
            tracing::error!(error = %e, "❌ リマインド（時刻指定）処理でエラー");
        }
        if let Err(e) = process_todo_due(ctx).await {
            tracing::error!(error = %e, "❌ ToDo 期限リマインド処理でエラー");
        }
        if let Err(e) = process_schedule_reminders(ctx).await {
            tracing::error!(error = %e, "❌ 予定リマインド処理でエラー");
        }
    }
}

/// 1. 時刻指定・繰り返しリマインド（§3.3.2）。
async fn process_due_reminders(ctx: &ServiceContext) -> Result<(), yuuka_core::DbError> {
    let repo = ReminderRepo::new(&ctx.db);
    let due = repo.list_due_pending(ctx.cross).await?;
    for reminder in due {
        let target = resolve_target(&reminder);
        let notification = Notification::text(
            UserId::new(reminder.user_id.clone()),
            BotId::new(reminder.bot_id.clone()),
            format!("⏰ リマインド: {}", reminder.message),
        )
        .with_target(target);

        if !ctx.notifier.send(notification).await {
            // 送信失敗は pending のまま残し次 tick で再試行（§10）。
            tracing::warn!(id = reminder.id, user = %reminder.user_id, "⚠️ リマインド送信失敗のため再試行します");
            continue;
        }

        if let Err(e) = advance_after_send(ctx, &repo, &reminder).await {
            tracing::error!(id = reminder.id, error = %e, "❌ リマインド状態更新に失敗");
        }
    }
    Ok(())
}

/// 送信後の状態更新（繰り返しは次回へ・単発/壊れ規則は送信済み）。
async fn advance_after_send(
    ctx: &ServiceContext,
    repo: &ReminderRepo<'_>,
    reminder: &DueReminder,
) -> Result<(), yuuka_core::DbError> {
    match reminder.repeat_rule.as_deref() {
        Some(rule) => match cron_util::next_reminder_trigger(rule, Local::now()) {
            Some(next) => repo.reschedule_repeat(ctx.cross, reminder.id, next).await,
            // cron 式が壊れている場合は無限再送を防ぐため単発扱い（送信済み）。
            None => {
                tracing::error!(id = reminder.id, rule = rule, "❌ repeat_rule 解釈失敗のため単発扱い");
                repo.mark_sent(ctx.cross, reminder.id).await
            }
        },
        None => repo.mark_sent(ctx.cross, reminder.id).await,
    }
}

/// リマインドの送信先を解決する（現行 `resolveTarget`）。
fn resolve_target(reminder: &DueReminder) -> NotifyTarget {
    if reminder.target_type.as_deref() == Some("channel") {
        match reminder.target_id.as_deref() {
            Some(id) if !id.is_empty() => NotifyTarget::Channel(id.to_owned()),
            // チャンネル指定だが ID 不明 → 既定送信先（→DM）へ委ねる。
            _ => NotifyTarget::Default,
        }
    } else {
        NotifyTarget::Default
    }
}

/// 2. ToDo 期限リマインド（§3.3.1）。
async fn process_todo_due(ctx: &ServiceContext) -> Result<(), yuuka_core::DbError> {
    let repo = TodoRepo::new(&ctx.db);
    let todos = repo.list_open_due_within(ctx.cross, 24).await?;
    for todo in todos {
        let content = format!(
            "⏰ ToDoの期限が近づいています: 「{}」 (#{})\n期限: {}",
            todo.title,
            todo.id,
            format_display_datetime(todo.due_date.as_deref())
        );
        let notification = Notification::text(
            UserId::new(todo.user_id.clone()),
            BotId::new(todo.bot_id.clone()),
            content,
        );
        if ctx.notifier.send(notification).await {
            if let Err(e) = repo.mark_due_reminded(ctx.cross, todo.id).await {
                tracing::error!(id = todo.id, error = %e, "❌ ToDo 期限通知フラグ更新に失敗");
            }
        } else {
            tracing::warn!(id = todo.id, user = %todo.user_id, "⚠️ ToDo 期限リマインド送信失敗のため再試行します");
        }
    }
    Ok(())
}

/// 3. 予定リマインド（§3.3.1）。
async fn process_schedule_reminders(ctx: &ServiceContext) -> Result<(), yuuka_core::DbError> {
    let repo = ScheduleRepo::new(&ctx.db);
    let schedules = repo.get_unreminded(ctx.cross).await?;
    for schedule in schedules {
        let content = format!(
            "⏰ まもなく予定の時間です: 「{}」\n開始: {}",
            schedule.title,
            format_display_datetime(Some(&schedule.start_at))
        );
        let notification = Notification::text(
            UserId::new(schedule.user_id.clone()),
            BotId::new(schedule.bot_id.clone()),
            content,
        );
        if ctx.notifier.send(notification).await {
            if let Err(e) = repo.mark_reminded(ctx.cross, schedule.id).await {
                tracing::error!(id = schedule.id, error = %e, "❌ 予定通知フラグ更新に失敗");
            }
        } else {
            tracing::warn!(id = schedule.id, user = %schedule.user_id, "⚠️ 予定リマインド送信失敗のため再試行します");
        }
    }
    Ok(())
}

/// DB 形式（`'YYYY-MM-DD HH:MM:SS'`/ISO）の日時を通知向けに整形（現行 `formatDisplayDateTime`）。
/// 秒は落として `'YYYY-MM-DD HH:MM'`、日付のみはそのまま、日付で始まらなければ原文。
fn format_display_datetime(value: Option<&str>) -> String {
    let raw = match value {
        Some(s) if !s.trim().is_empty() => s.trim().replace('T', " "),
        _ => return "未設定".to_owned(),
    };
    let Some(date) = raw.get(0..10).filter(|d| is_ymd(d)) else {
        return raw;
    };
    if raw.as_bytes().get(10) == Some(&b' ') {
        if let Some(hm) = raw.get(11..16).filter(|t| is_hm(t)) {
            return format!("{date} {hm}");
        }
    }
    date.to_owned()
}

/// `YYYY-MM-DD` 形か。
fn is_ymd(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, &c)| match i {
            4 | 7 => c == b'-',
            _ => c.is_ascii_digit(),
        })
}

/// `HH:MM` 形か。
fn is_hm(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 5
        && b.iter().enumerate().all(|(i, &c)| match i {
            2 => c == b':',
            _ => c.is_ascii_digit(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ctx_with, seeded_db, RecordingNotifier};
    use std::sync::Arc;
    use yuuka_db::map_sqlite;

    const REMINDERS_DDL: &str = "CREATE TABLE reminders (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL, bot_id TEXT NOT NULL DEFAULT 'system_default',
        message TEXT NOT NULL, trigger_at TEXT NOT NULL, repeat_rule TEXT,
        target_type TEXT NOT NULL DEFAULT 'dm', target_id TEXT,
        status TEXT NOT NULL DEFAULT 'pending', source TEXT NOT NULL DEFAULT 'manual',
        source_id TEXT, created_at TEXT NOT NULL DEFAULT (datetime('now','localtime'))
    );";

    #[test]
    fn display_datetime_drops_seconds() {
        assert_eq!(format_display_datetime(Some("2026-07-08 09:30:15")), "2026-07-08 09:30");
        assert_eq!(format_display_datetime(Some("2026-07-08T09:30:15")), "2026-07-08 09:30");
        assert_eq!(format_display_datetime(Some("2026-07-08")), "2026-07-08");
        assert_eq!(format_display_datetime(None), "未設定");
        assert_eq!(format_display_datetime(Some("  ")), "未設定");
        assert_eq!(format_display_datetime(Some("later")), "later");
    }

    #[tokio::test]
    async fn due_reminder_sent_and_marked() {
        let (db, _dir) = seeded_db(REMINDERS_DDL);
        db.writer
            .execute(|conn| {
                conn.execute(
                    "INSERT INTO reminders (user_id, bot_id, message, trigger_at, status) \
                     VALUES ('u','system_default','飲み物','2000-01-01 00:00:00','pending')",
                    [],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();

        let rec = Arc::new(RecordingNotifier::new(true));
        let ctx = ctx_with(db, rec.clone());
        process_due_reminders(&ctx).await.unwrap();

        assert_eq!(rec.count(), 1);
        {
            let sent = rec.sent.lock().unwrap();
            assert_eq!(sent[0].content, "⏰ リマインド: 飲み物");
        }

        // 送信成功 → status='sent'。
        let status: String = ctx
            .db
            .read
            .read(|conn| {
                conn.query_row("SELECT status FROM reminders WHERE id=1", [], |r| r.get(0))
                    .map_err(map_sqlite)
            })
            .await
            .unwrap();
        assert_eq!(status, "sent");
    }

    #[tokio::test]
    async fn failed_send_leaves_pending() {
        let (db, _dir) = seeded_db(REMINDERS_DDL);
        db.writer
            .execute(|conn| {
                conn.execute(
                    "INSERT INTO reminders (user_id, bot_id, message, trigger_at, status) \
                     VALUES ('u','system_default','x','2000-01-01 00:00:00','pending')",
                    [],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();

        // succeed=false（配信不可）→ pending のまま。
        let rec = Arc::new(RecordingNotifier::new(false));
        let ctx = ctx_with(db, rec.clone());
        process_due_reminders(&ctx).await.unwrap();

        let status: String = ctx
            .db
            .read
            .read(|conn| {
                conn.query_row("SELECT status FROM reminders WHERE id=1", [], |r| r.get(0))
                    .map_err(map_sqlite)
            })
            .await
            .unwrap();
        assert_eq!(status, "pending");
    }

    #[tokio::test]
    async fn repeat_rule_reschedules_instead_of_sent() {
        let (db, _dir) = seeded_db(REMINDERS_DDL);
        db.writer
            .execute(|conn| {
                conn.execute(
                    "INSERT INTO reminders (user_id, bot_id, message, trigger_at, repeat_rule, status) \
                     VALUES ('u','system_default','daily','2000-01-01 00:00:00','0 9 * * *','pending')",
                    [],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();

        let rec = Arc::new(RecordingNotifier::new(true));
        let ctx = ctx_with(db, rec.clone());
        process_due_reminders(&ctx).await.unwrap();

        // 繰り返しは pending へ戻り、trigger_at が未来へ進む。
        let (status, trigger): (String, String) = ctx
            .db
            .read
            .read(|conn| {
                conn.query_row("SELECT status, trigger_at FROM reminders WHERE id=1", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .map_err(map_sqlite)
            })
            .await
            .unwrap();
        assert_eq!(status, "pending");
        assert_ne!(trigger, "2000-01-01 00:00:00");
    }
}
