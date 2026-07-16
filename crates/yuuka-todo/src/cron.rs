//! cron 用の**全ユーザー横断走査**（§3.2/§3.3.1・現行 todoRepo の
//! `listOpenTodosDueWithinAcrossUsers` / `markDueReminded` / `listOverdueRoutinesAcrossUsers` /
//! `advanceRoutine` / `endRoutineById`）。
//!
//! 通常の `UserScope` 経路から隔離するため [`CrossUserAccess`] 証憑を要求する。返す型は
//! 通知・繰り返し計算に必要な内部列（`user_id`/`bot_id`/`repeat_*`）を含む（クリーンビュー `Todo`
//! とは別型）。リマインドエンジン（期限通知）とルーチンサービス（繰り返し生成）の双方が使う。

use rusqlite::{params, Row};
use yuuka_core::{CrossUserAccess, DbError};
use yuuka_db::map_sqlite;

use crate::repo::TodoRepo;

/// 期限が迫った未通知 ToDo（cron 期限リマインド用・内部列を持つ）。
#[derive(Debug, Clone)]
pub struct DueTodo {
    pub id: i64,
    pub user_id: String,
    pub bot_id: String,
    pub title: String,
    pub due_date: Option<String>,
}

/// 期日を過ぎた繰り返し（ルーチン）タスク（cron ルーチン生成用・内部列を持つ）。
#[derive(Debug, Clone)]
pub struct OverdueRoutine {
    pub id: i64,
    pub user_id: String,
    pub title: String,
    pub due_date: Option<String>,
    pub repeat_rule: Option<String>,
    pub repeat_until: Option<String>,
    pub repeat_count: Option<i64>,
}

impl TodoRepo<'_> {
    /// 期限が `hours` 時間以内に迫った未通知の open ToDo を**全ユーザー横断**で返す
    /// （現行 `listOpenTodosDueWithinAcrossUsers`）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list_open_due_within(
        &self,
        _cron: CrossUserAccess,
        hours: i64,
    ) -> Result<Vec<DueTodo>, DbError> {
        let window = format!("+{} hours", hours.max(0));
        self.read
            .read(move |conn| {
                let sql = "SELECT id, user_id, bot_id, title, due_date FROM todos \
                     WHERE status = 'open' AND due_reminded = 0 AND due_date IS NOT NULL \
                     AND datetime(due_date) <= datetime('now', 'localtime', ?1) \
                     ORDER BY datetime(due_date) ASC";
                let mut stmt = conn.prepare(sql).map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![window], row_to_due)
                    .map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                Ok(out)
            })
            .await
    }

    /// ToDo を期限通知済みにする（現行 `markDueReminded`）。id は横断走査済み。
    ///
    /// # Errors
    /// 更新失敗時 [`DbError`]。
    pub async fn mark_due_reminded(&self, _cron: CrossUserAccess, id: i64) -> Result<(), DbError> {
        self.writer
            .execute(move |conn| {
                conn.execute(
                    "UPDATE todos SET due_reminded = 1, updated_at = datetime('now', 'localtime') \
                     WHERE id = ?1",
                    params![id],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
    }

    /// 期日を過ぎた繰り返し（ルーチン・親）タスクを**全ユーザー横断**で返す
    /// （現行 `listOverdueRoutinesAcrossUsers`）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list_overdue_routines(
        &self,
        _cron: CrossUserAccess,
    ) -> Result<Vec<OverdueRoutine>, DbError> {
        self.read
            .read(move |conn| {
                let sql = "SELECT id, user_id, title, due_date, repeat_rule, repeat_until, \
                     repeat_count FROM todos \
                     WHERE repeat_rule IS NOT NULL AND parent_id IS NULL AND due_date IS NOT NULL \
                     AND datetime(due_date) <= datetime('now', 'localtime') \
                     ORDER BY datetime(due_date) ASC";
                let mut stmt = conn.prepare(sql).map_err(map_sqlite)?;
                let rows = stmt.query_map([], row_to_routine).map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                Ok(out)
            })
            .await
    }

    /// ルーチンを次回期日へ進める（同一行の due_date を更新し状態/進捗/通知フラグをリセット・
    /// 現行 `advanceRoutine`）。更新できたら `true`。
    ///
    /// # Errors
    /// 更新失敗時 [`DbError`]。
    pub async fn advance_routine(
        &self,
        _cron: CrossUserAccess,
        id: i64,
        next_due: String,
        next_count: Option<i64>,
    ) -> Result<bool, DbError> {
        self.writer
            .execute(move |conn| {
                let n = conn
                    .execute(
                        "UPDATE todos SET due_date = ?1, status = 'open', progress = 0, \
                         due_reminded = 0, repeat_count = ?2, \
                         updated_at = datetime('now', 'localtime') WHERE id = ?3",
                        params![next_due, next_count, id],
                    )
                    .map_err(map_sqlite)?;
                Ok(n > 0)
            })
            .await
    }

    /// ルーチンを終了して単発タスクへ戻す（`repeat_*` をクリア・現行 `endRoutineById`）。
    ///
    /// # Errors
    /// 更新失敗時 [`DbError`]。
    pub async fn end_routine(&self, _cron: CrossUserAccess, id: i64) -> Result<(), DbError> {
        self.writer
            .execute(move |conn| {
                conn.execute(
                    "UPDATE todos SET repeat_rule = NULL, repeat_until = NULL, repeat_count = NULL, \
                     updated_at = datetime('now', 'localtime') WHERE id = ?1",
                    params![id],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
    }
}

fn row_to_due(row: &Row) -> rusqlite::Result<DueTodo> {
    Ok(DueTodo {
        id: row.get("id")?,
        user_id: row.get("user_id")?,
        bot_id: row.get("bot_id")?,
        title: row.get("title")?,
        due_date: row.get("due_date")?,
    })
}

fn row_to_routine(row: &Row) -> rusqlite::Result<OverdueRoutine> {
    Ok(OverdueRoutine {
        id: row.get("id")?,
        user_id: row.get("user_id")?,
        title: row.get("title")?,
        due_date: row.get("due_date")?,
        repeat_rule: row.get("repeat_rule")?,
        repeat_until: row.get("repeat_until")?,
        repeat_count: row.get("repeat_count")?,
    })
}
