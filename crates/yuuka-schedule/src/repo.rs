//! `ScheduleRepo` — `UserScope` 束縛のデータアクセス（分離キーを型で強制・§12.2 契約5）。
//!
//! 全クエリは `WHERE user_id = ? AND bot_id = ?` を必須にし、`&UserScope` を取ることで
//! 「user_id/bot_id 無しクエリ」を型で不能化する。読みは [`ReadPool`]、書きは
//! [`WriterHandle`]（BEGIN IMMEDIATE）へ送る。参照実装は yuuka-todo。

use rusqlite::{params, Row};
use yuuka_core::scope::ScopedRepo;
use yuuka_core::{DbError, UserScope};
use yuuka_db::{map_sqlite, ReadPool, WriterHandle};
use yuuka_web::Db;

use crate::dto::{NewSchedule, Schedule};

/// 返却列（クリーンビュー・内部列 user_id/bot_id/reminded/google_* は含めない）。
const SCHEDULE_COLUMNS: &str =
    "id, title, description, start_at, end_at, remind_before_minutes, created_at";

/// Node parity: `remind_before_minutes` 未指定時の既定値。
const DEFAULT_REMIND_BEFORE_MINUTES: i64 = 10;

/// schedule リポジトリ（DB ハンドルを借用する軽量ラッパ・per-request 構築）。
pub struct ScheduleRepo<'a> {
    pub(crate) read: &'a ReadPool,
    pub(crate) writer: &'a WriterHandle,
}

impl ScopedRepo for ScheduleRepo<'_> {}

impl<'a> ScheduleRepo<'a> {
    /// 共有 DB ハンドルから構築する。
    #[must_use]
    pub fn new(db: &'a Db) -> Self {
        Self {
            read: &db.read,
            writer: &db.writer,
        }
    }

    /// スコープ内の直近 `days` 日以内に開始する予定を開始時刻昇順で返す（Node listUpcomingSchedules）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list_upcoming(&self, scope: &UserScope, days: i64) -> Result<Vec<Schedule>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {SCHEDULE_COLUMNS} FROM schedules \
                     WHERE user_id = ?1 AND bot_id = ?2 \
                     AND start_at >= datetime('now', 'localtime') \
                     AND start_at <= datetime('now', 'localtime', '+' || ?3 || ' days') \
                     ORDER BY start_at ASC"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![uid, bid, days], row_to_schedule)
                    .map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                Ok(out)
            })
            .await
    }

    /// スコープ内の単一予定を取得する（無ければ `None`）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn get(&self, scope: &UserScope, id: i64) -> Result<Option<Schedule>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {SCHEDULE_COLUMNS} FROM schedules \
                     WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let mut rows = stmt
                    .query_map(params![id, uid, bid], row_to_schedule)
                    .map_err(map_sqlite)?;
                match rows.next() {
                    Some(row) => Ok(Some(row.map_err(map_sqlite)?)),
                    None => Ok(None),
                }
            })
            .await
    }

    /// 予定を作成し、作成後の行を返す。
    ///
    /// # Errors
    /// 挿入失敗・作成後の取得失敗時 [`DbError`]。
    pub async fn add(&self, scope: &UserScope, input: NewSchedule) -> Result<Schedule, DbError> {
        let (uid, bid) = scope_keys(scope);
        let remind = input
            .remind_before_minutes
            .unwrap_or(DEFAULT_REMIND_BEFORE_MINUTES);
        let id = self
            .writer
            .transaction(move |tx| {
                tx.execute(
                    "INSERT INTO schedules \
                       (user_id, bot_id, title, description, start_at, end_at, \
                        remind_before_minutes) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        uid,
                        bid,
                        input.title,
                        input.description,
                        input.start_at,
                        input.end_at,
                        remind,
                    ],
                )
                .map_err(map_sqlite)?;
                Ok(tx.last_insert_rowid())
            })
            .await?;
        self.get(scope, id)
            .await?
            .ok_or_else(|| DbError::Operation("inserted schedule not found".to_owned()))
    }

    /// 予定を削除する（削除できたら `true`）。
    ///
    /// # Errors
    /// 削除失敗時 [`DbError`]。
    pub async fn delete(&self, scope: &UserScope, id: i64) -> Result<bool, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.writer
            .transaction(move |tx| {
                let n = tx
                    .execute(
                        "DELETE FROM schedules WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3",
                        params![id, uid, bid],
                    )
                    .map_err(map_sqlite)?;
                Ok(n > 0)
            })
            .await
    }
}

/// スコープから所有 String キーを取り出す（`spawn_blocking` の `'static` クロージャ用）。
fn scope_keys(scope: &UserScope) -> (String, String) {
    (
        scope.user_id().as_str().to_owned(),
        scope.bot_id().as_str().to_owned(),
    )
}

/// SQLite 行を [`Schedule`] へ変換する。
fn row_to_schedule(row: &Row) -> rusqlite::Result<Schedule> {
    Ok(Schedule {
        id: row.get("id")?,
        title: row.get("title")?,
        description: row.get("description")?,
        start_at: row.get("start_at")?,
        end_at: row.get("end_at")?,
        remind_before_minutes: row.get("remind_before_minutes")?,
        created_at: row.get("created_at")?,
    })
}
