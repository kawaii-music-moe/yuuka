//! `ReminderRepo` — `UserScope` 束縛のデータアクセス（分離キーを型で強制）。
//!
//! 全クエリは `WHERE user_id = ? AND bot_id = ?` を必須にし、`&UserScope` を取ることで
//! 「user_id/bot_id 無しクエリ」を型で不能化する。読みは [`ReadPool`]、書きは
//! [`WriterHandle`]（BEGIN IMMEDIATE）へ送る。cron 用の全件走査（listDuePending /
//! markSent / rescheduleRepeat）は [`crate::cron`]（`CrossUserAccess` 証憑必須）へ隔離した。

use rusqlite::{params, Row};
use yuuka_core::scope::ScopedRepo;
use yuuka_core::{DbError, UserScope};
use yuuka_db::{map_sqlite, ReadPool, WriterHandle};
use yuuka_web::Db;

use crate::dto::{NewReminder, Reminder};

/// 返却列（クリーンビュー・内部列 user_id/bot_id は含めない）。
const REMINDER_COLUMNS: &str = "id, message, trigger_at, repeat_rule, target_type, target_id, \
     status, source, source_id, created_at";

/// reminder リポジトリ（DB ハンドルを借用する軽量ラッパ・per-request 構築）。
pub struct ReminderRepo<'a> {
    pub(crate) read: &'a ReadPool,
    pub(crate) writer: &'a WriterHandle,
}

impl ScopedRepo for ReminderRepo<'_> {}

impl<'a> ReminderRepo<'a> {
    /// 共有 DB ハンドルから構築する。
    #[must_use]
    pub fn new(db: &'a Db) -> Self {
        Self {
            read: &db.read,
            writer: &db.writer,
        }
    }

    /// スコープ内のリマインドを返す。
    ///
    /// `include_all=false` は `status='pending'` のみを `trigger_at` 昇順。
    /// `include_all=true` は全 status を pending 優先・`trigger_at` 昇順（Node parity）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list(
        &self,
        scope: &UserScope,
        include_all: bool,
    ) -> Result<Vec<Reminder>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let sql = if include_all {
                    format!(
                        "SELECT {REMINDER_COLUMNS} FROM reminders \
                         WHERE user_id = ?1 AND bot_id = ?2 \
                         ORDER BY CASE status WHEN 'pending' THEN 0 ELSE 1 END, \
                         trigger_at ASC, id ASC"
                    )
                } else {
                    format!(
                        "SELECT {REMINDER_COLUMNS} FROM reminders \
                         WHERE user_id = ?1 AND bot_id = ?2 AND status = 'pending' \
                         ORDER BY trigger_at ASC, id ASC"
                    )
                };
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![uid, bid], row_to_reminder)
                    .map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                Ok(out)
            })
            .await
    }

    /// スコープ内の単一リマインドを取得する（無ければ `None`）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn get(&self, scope: &UserScope, id: i64) -> Result<Option<Reminder>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {REMINDER_COLUMNS} FROM reminders \
                     WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let mut rows = stmt
                    .query_map(params![id, uid, bid], row_to_reminder)
                    .map_err(map_sqlite)?;
                match rows.next() {
                    Some(row) => Ok(Some(row.map_err(map_sqlite)?)),
                    None => Ok(None),
                }
            })
            .await
    }

    /// リマインドを作成し、作成後の行を返す。
    ///
    /// `target_type` 未指定は `"dm"`、`source` は `"manual"` 固定（Web UI 由来）。
    /// trigger_at の正規化・cron 検証・過去日時補正・既定送信先解決は deferred。
    ///
    /// # Errors
    /// 挿入失敗・作成後の取得失敗時 [`DbError`]。
    pub async fn add(&self, scope: &UserScope, input: NewReminder) -> Result<Reminder, DbError> {
        let (uid, bid) = scope_keys(scope);
        let id = self
            .writer
            .transaction(move |tx| {
                let target_type = match input.target_type.as_deref() {
                    Some("channel") => "channel",
                    _ => "dm",
                };
                tx.execute(
                    "INSERT INTO reminders \
                       (user_id, bot_id, message, trigger_at, repeat_rule, target_type, \
                        target_id, status, source, source_id, created_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', 'manual', NULL, \
                             datetime('now', 'localtime'))",
                    params![
                        uid,
                        bid,
                        input.message,
                        input.trigger_at,
                        input.repeat_rule,
                        target_type,
                        input.target_id,
                    ],
                )
                .map_err(map_sqlite)?;
                Ok(tx.last_insert_rowid())
            })
            .await?;
        self.get(scope, id)
            .await?
            .ok_or_else(|| DbError::Operation("inserted reminder not found".to_owned()))
    }

    /// リマインドをキャンセル（status=cancelled、`pending` のみ対象）し更新後の行を返す。
    ///
    /// pending でない／存在しない場合は `None`。route 側は `None` のとき [`Self::get`] で
    /// 実在を確かめ、Node パリティで **不在→404／実在するが pending でない→409** を区別する。
    ///
    /// # Errors
    /// 更新・取得失敗時 [`DbError`]。
    pub async fn cancel(&self, scope: &UserScope, id: i64) -> Result<Option<Reminder>, DbError> {
        let (uid, bid) = scope_keys(scope);
        let changed = self
            .writer
            .transaction(move |tx| {
                let n = tx
                    .execute(
                        "UPDATE reminders SET status = 'cancelled' \
                         WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3 AND status = 'pending'",
                        params![id, uid, bid],
                    )
                    .map_err(map_sqlite)?;
                Ok(n > 0)
            })
            .await?;
        if changed {
            self.get(scope, id).await
        } else {
            Ok(None)
        }
    }

    /// リマインドを削除する（削除できたら `true`）。
    ///
    /// # Errors
    /// 削除失敗時 [`DbError`]。
    pub async fn delete(&self, scope: &UserScope, id: i64) -> Result<bool, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.writer
            .transaction(move |tx| {
                let n = tx
                    .execute(
                        "DELETE FROM reminders WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3",
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

/// SQLite 行を [`Reminder`] へ変換する。
fn row_to_reminder(row: &Row) -> rusqlite::Result<Reminder> {
    Ok(Reminder {
        id: row.get("id")?,
        message: row.get("message")?,
        trigger_at: row.get("trigger_at")?,
        repeat_rule: row.get("repeat_rule")?,
        target_type: row.get("target_type")?,
        target_id: row.get("target_id")?,
        status: row.get("status")?,
        source: row.get("source")?,
        source_id: row.get("source_id")?,
        created_at: row.get("created_at")?,
    })
}
