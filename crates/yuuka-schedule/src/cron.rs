//! cron（リマインドエンジン）用の**全ユーザー横断走査**（§3.3.1・現行 scheduleRepo の
//! `getUnremindedSchedules` / `markReminded`）。
//!
//! 通常の `UserScope` 経路から隔離するため [`CrossUserAccess`] 証憑を要求する。返す
//! [`DueSchedule`] は通知に必要な内部列（`user_id`/`bot_id`）を含む。

use rusqlite::{Row, params};
use yuuka_core::{CrossUserAccess, DbError};
use yuuka_db::map_sqlite;

use crate::repo::ScheduleRepo;

/// 通知前時間（`remind_before_minutes`）に達した未通知予定（cron 専用・内部列を持つ）。
#[derive(Debug, Clone)]
pub struct DueSchedule {
    pub id: i64,
    pub user_id: String,
    pub bot_id: String,
    pub title: String,
    pub start_at: String,
}

impl ScheduleRepo<'_> {
    /// 通知時刻に達した未通知予定を**全ユーザー横断**で返す（現行 `getUnremindedSchedules`）。
    ///
    /// `reminded=0` かつ `start_at - remind_before <= now` かつ `start_at >= now-2分`
    /// （開始から時間が経ちすぎた予定は通知しない・Node parity）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn get_unreminded(
        &self,
        _cron: CrossUserAccess,
    ) -> Result<Vec<DueSchedule>, DbError> {
        self.read
            .read(move |conn| {
                let sql = "SELECT id, user_id, bot_id, title, start_at FROM schedules \
                     WHERE reminded = 0 \
                     AND datetime(start_at, '-' || remind_before_minutes || ' minutes') \
                         <= datetime('now', 'localtime') \
                     AND start_at >= datetime('now', 'localtime', '-2 minutes')";
                let mut stmt = conn.prepare(sql).map_err(map_sqlite)?;
                let rows = stmt.query_map([], row_to_due).map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                Ok(out)
            })
            .await
    }

    /// 予定を通知済みにする（現行 `markReminded`）。id は `get_unreminded` で横断走査済み。
    ///
    /// # Errors
    /// 更新失敗時 [`DbError`]。
    pub async fn mark_reminded(&self, _cron: CrossUserAccess, id: i64) -> Result<(), DbError> {
        self.writer
            .execute(move |conn| {
                conn.execute("UPDATE schedules SET reminded = 1 WHERE id = ?1", params![id])
                    .map_err(map_sqlite)?;
                Ok(())
            })
            .await
    }
}

fn row_to_due(row: &Row) -> rusqlite::Result<DueSchedule> {
    Ok(DueSchedule {
        id: row.get("id")?,
        user_id: row.get("user_id")?,
        bot_id: row.get("bot_id")?,
        title: row.get("title")?,
        start_at: row.get("start_at")?,
    })
}
