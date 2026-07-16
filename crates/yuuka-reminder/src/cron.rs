//! cron（リマインドエンジン）用の**全ユーザー横断走査**（§3.3.2・現行 reminderRepo の
//! `listDuePending` / `markSent` / `rescheduleRepeat`）。
//!
//! 通常の `UserScope` 経路から**隔離**するため、各メソッドは [`CrossUserAccess`] 証憑を要求する
//! （横断アクセスは cron/バッチ起点でしか作れない・§7.3）。返す [`DueReminder`] は通知に必要な
//! 内部列（`user_id`/`bot_id`/送信先/繰り返し規則）を含む（クリーンビュー `Reminder` とは別型）。

use rusqlite::{params, Row};
use yuuka_core::{CrossUserAccess, DbError};
use yuuka_db::map_sqlite;

use crate::repo::ReminderRepo;

/// 送信期限に達した pending リマインド（cron 専用・内部列を持つ）。
#[derive(Debug, Clone)]
pub struct DueReminder {
    pub id: i64,
    pub user_id: String,
    pub bot_id: String,
    pub message: String,
    pub trigger_at: String,
    pub repeat_rule: Option<String>,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub source: Option<String>,
}

const DUE_COLUMNS: &str =
    "id, user_id, bot_id, message, trigger_at, repeat_rule, target_type, target_id, source";

impl ReminderRepo<'_> {
    /// 送信期限に達した pending リマインドを**全ユーザー横断**で返す（現行 `listDuePending`）。
    ///
    /// `status='pending' AND trigger_at <= datetime('now','localtime')` を `trigger_at` 昇順。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list_due_pending(
        &self,
        _cron: CrossUserAccess,
    ) -> Result<Vec<DueReminder>, DbError> {
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {DUE_COLUMNS} FROM reminders \
                     WHERE status = 'pending' AND trigger_at <= datetime('now', 'localtime') \
                     ORDER BY trigger_at ASC"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let rows = stmt.query_map([], row_to_due).map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                Ok(out)
            })
            .await
    }

    /// リマインドを送信済みにする（現行 `markSent`）。id は `list_due_pending` で横断走査済み。
    ///
    /// # Errors
    /// 更新失敗時 [`DbError`]。
    pub async fn mark_sent(&self, _cron: CrossUserAccess, id: i64) -> Result<(), DbError> {
        self.writer
            .execute(move |conn| {
                conn.execute(
                    "UPDATE reminders SET status = 'sent' WHERE id = ?1",
                    params![id],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
    }

    /// 繰り返しリマインドを次回時刻へ再スケジュール（`status='pending'` へ戻す・現行 `rescheduleRepeat`）。
    ///
    /// `next_trigger_at` は DB 形式 `'YYYY-MM-DD HH:MM:SS'`（呼び出し側で cron 式から算出済み）。
    ///
    /// # Errors
    /// 更新失敗時 [`DbError`]。
    pub async fn reschedule_repeat(
        &self,
        _cron: CrossUserAccess,
        id: i64,
        next_trigger_at: String,
    ) -> Result<(), DbError> {
        self.writer
            .execute(move |conn| {
                conn.execute(
                    "UPDATE reminders SET trigger_at = ?1, status = 'pending' WHERE id = ?2",
                    params![next_trigger_at, id],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
    }
}

fn row_to_due(row: &Row) -> rusqlite::Result<DueReminder> {
    Ok(DueReminder {
        id: row.get("id")?,
        user_id: row.get("user_id")?,
        bot_id: row.get("bot_id")?,
        message: row.get("message")?,
        trigger_at: row.get("trigger_at")?,
        repeat_rule: row.get("repeat_rule")?,
        target_type: row.get("target_type")?,
        target_id: row.get("target_id")?,
        source: row.get("source")?,
    })
}
