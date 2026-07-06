//! `TimelineRepo` — `UserScope` 束縛のデータアクセス（分離キーを型で強制・§12.2 契約5）。
//!
//! 全クエリは `WHERE user_id = ? AND bot_id = ?` を必須にし、`&UserScope` を取ることで
//! 「user_id/bot_id 無しクエリ」を型で不能化する。読みは [`ReadPool`]、書きは
//! [`WriterHandle`]（BEGIN IMMEDIATE）へ送る。対象表は `timeline_records`。

use rusqlite::{params, Row};
use yuuka_core::scope::ScopedRepo;
use yuuka_core::{DbError, UserScope};
use yuuka_db::{map_sqlite, ReadPool, WriterHandle};
use yuuka_web::Db;

use crate::dto::{NewTimelineRecord, TimelineRecord};

/// 返却列（クリーンビュー・内部列 user_id/bot_id は含めない）。
const RECORD_COLUMNS: &str = "id, date, recorded_at, type, title, content, todo_id, expense_id, \
     amount, expense_category, media_path, media_type, location, created_at";

/// timeline リポジトリ（DB ハンドルを借用する軽量ラッパ・per-request 構築）。
pub struct TimelineRepo<'a> {
    read: &'a ReadPool,
    writer: &'a WriterHandle,
}

impl ScopedRepo for TimelineRepo<'_> {}

impl<'a> TimelineRepo<'a> {
    /// 共有 DB ハンドルから構築する。
    #[must_use]
    pub fn new(db: &'a Db) -> Self {
        Self {
            read: &db.read,
            writer: &db.writer,
        }
    }

    /// スコープ内・指定日の全記録を `recorded_at` 昇順で返す（Node `listTimelineRecords`）。
    ///
    /// `date` が `None`（未指定）なら **本日（UTC）** に畳む（Node の
    /// `date ?? new Date().toISOString().slice(0,10)` と一致。SQLite `date('now')` は UTC）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list(
        &self,
        scope: &UserScope,
        date: Option<String>,
    ) -> Result<Vec<TimelineRecord>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {RECORD_COLUMNS} FROM timeline_records \
                     WHERE user_id = ?1 AND bot_id = ?2 AND date = COALESCE(?3, date('now')) \
                     ORDER BY recorded_at ASC, id ASC"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![uid, bid, date], row_to_record)
                    .map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                Ok(out)
            })
            .await
    }

    /// スコープ内の単一記録を取得する（無ければ `None`）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn get(&self, scope: &UserScope, id: i64) -> Result<Option<TimelineRecord>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {RECORD_COLUMNS} FROM timeline_records \
                     WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let mut rows = stmt
                    .query_map(params![id, uid, bid], row_to_record)
                    .map_err(map_sqlite)?;
                match rows.next() {
                    Some(row) => Ok(Some(row.map_err(map_sqlite)?)),
                    None => Ok(None),
                }
            })
            .await
    }

    /// 記録を作成し、作成後の行を返す（Node `addTimelineRecord`）。
    ///
    /// `recorded_at` は未指定なら `datetime('now')`（**UTC**。Node は `new Date().toISOString()`
    /// ＝ UTC で入れるため localtime にすると +9h ずれる。`created_at` は Node 同様 localtime）。
    ///
    /// # Errors
    /// 挿入失敗・作成後の取得失敗時 [`DbError`]。
    pub async fn add(
        &self,
        scope: &UserScope,
        input: NewTimelineRecord,
    ) -> Result<TimelineRecord, DbError> {
        let (uid, bid) = scope_keys(scope);
        let id = self
            .writer
            .transaction(move |tx| {
                tx.execute(
                    "INSERT INTO timeline_records \
                       (user_id, bot_id, date, recorded_at, type, title, content, \
                        todo_id, expense_id, amount, expense_category, media_path, media_type, \
                        location, created_at) \
                     VALUES (?1, ?2, ?3, \
                             COALESCE(?4, datetime('now')), \
                             ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, \
                             datetime('now', 'localtime'))",
                    params![
                        uid,
                        bid,
                        input.date,
                        input.recorded_at,
                        input.r#type,
                        input.title,
                        input.content,
                        input.todo_id,
                        input.expense_id,
                        input.amount,
                        input.expense_category,
                        input.media_path,
                        input.media_type,
                        input.location,
                    ],
                )
                .map_err(map_sqlite)?;
                Ok(tx.last_insert_rowid())
            })
            .await?;
        self.get(scope, id)
            .await?
            .ok_or_else(|| DbError::Operation("inserted timeline record not found".to_owned()))
    }

    /// 記録を削除する（削除できたら `true`。Node `deleteTimelineRecord`）。
    ///
    /// # Errors
    /// 削除失敗時 [`DbError`]。
    pub async fn delete(&self, scope: &UserScope, id: i64) -> Result<bool, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.writer
            .transaction(move |tx| {
                let n = tx
                    .execute(
                        "DELETE FROM timeline_records WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3",
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

/// SQLite 行を [`TimelineRecord`] へ変換する。
fn row_to_record(row: &Row) -> rusqlite::Result<TimelineRecord> {
    Ok(TimelineRecord {
        id: row.get("id")?,
        date: row.get("date")?,
        recorded_at: row.get("recorded_at")?,
        r#type: row.get("type")?,
        title: row.get("title")?,
        content: row.get("content")?,
        todo_id: row.get("todo_id")?,
        expense_id: row.get("expense_id")?,
        amount: row.get("amount")?,
        expense_category: row.get("expense_category")?,
        media_path: row.get("media_path")?,
        media_type: row.get("media_type")?,
        location: row.get("location")?,
        created_at: row.get("created_at")?,
    })
}
