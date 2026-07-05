//! `TodoRepo` — `UserScope` 束縛のデータアクセス（分離キーを型で強制・§12.2 契約5）。
//!
//! 全クエリは `WHERE user_id = ? AND bot_id = ?` を必須にし、`&UserScope` を取ることで
//! 「user_id/bot_id 無しクエリ」を型で不能化する。読みは [`ReadPool`]、書きは
//! [`WriterHandle`]（BEGIN IMMEDIATE）へ送る。

use rusqlite::{params, Row};
use yuuka_core::scope::ScopedRepo;
use yuuka_core::{DbError, UserScope};
use yuuka_db::{map_sqlite, ReadPool, WriterHandle};
use yuuka_web::Db;

use crate::dto::{NewTodo, Todo};

/// 返却列（クリーンビュー・内部列 user_id/bot_id 等は含めない）。
const TODO_COLUMNS: &str = "id, title, description, due_date, start_date, priority, tags, status, \
     progress, parent_id, repeat_rule, repeat_until, repeat_count, created_at, updated_at";

/// todo リポジトリ（DB ハンドルを借用する軽量ラッパ・per-request 構築）。
pub struct TodoRepo<'a> {
    read: &'a ReadPool,
    writer: &'a WriterHandle,
}

impl ScopedRepo for TodoRepo<'_> {}

impl<'a> TodoRepo<'a> {
    /// 共有 DB ハンドルから構築する。
    #[must_use]
    pub fn new(db: &'a Db) -> Self {
        Self {
            read: &db.read,
            writer: &db.writer,
        }
    }

    /// スコープ内の全 todo を作成日時降順で返す。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list(&self, scope: &UserScope) -> Result<Vec<Todo>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {TODO_COLUMNS} FROM todos \
                     WHERE user_id = ?1 AND bot_id = ?2 ORDER BY created_at DESC, id DESC"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![uid, bid], row_to_todo)
                    .map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                Ok(out)
            })
            .await
    }

    /// スコープ内の単一 todo を取得する（無ければ `None`）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn get(&self, scope: &UserScope, id: i64) -> Result<Option<Todo>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {TODO_COLUMNS} FROM todos \
                     WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let mut rows = stmt
                    .query_map(params![id, uid, bid], row_to_todo)
                    .map_err(map_sqlite)?;
                match rows.next() {
                    Some(row) => Ok(Some(row.map_err(map_sqlite)?)),
                    None => Ok(None),
                }
            })
            .await
    }

    /// todo を作成し、作成後の行を返す。
    ///
    /// # Errors
    /// 挿入失敗・作成後の取得失敗時 [`DbError`]。
    pub async fn add(&self, scope: &UserScope, input: NewTodo) -> Result<Todo, DbError> {
        let (uid, bid) = scope_keys(scope);
        let id = self
            .writer
            .transaction(move |tx| {
                let tags_json =
                    serde_json::to_string(&input.tags).unwrap_or_else(|_| "[]".to_owned());
                tx.execute(
                    "INSERT INTO todos \
                       (user_id, bot_id, title, description, due_date, start_date, priority, tags, \
                        parent_id, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, datetime('now'), datetime('now'))",
                    params![
                        uid,
                        bid,
                        input.title,
                        input.description,
                        input.due_date,
                        input.start_date,
                        input.priority,
                        tags_json,
                        input.parent_id,
                    ],
                )
                .map_err(map_sqlite)?;
                Ok(tx.last_insert_rowid())
            })
            .await?;
        self.get(scope, id)
            .await?
            .ok_or_else(|| DbError::Operation("inserted todo not found".to_owned()))
    }

    /// todo を完了（status=done, progress=100）にし、更新後の行を返す（無ければ `None`）。
    ///
    /// # Errors
    /// 更新・取得失敗時 [`DbError`]。
    pub async fn complete(&self, scope: &UserScope, id: i64) -> Result<Option<Todo>, DbError> {
        let (uid, bid) = scope_keys(scope);
        let changed = self
            .writer
            .transaction(move |tx| {
                let n = tx
                    .execute(
                        "UPDATE todos SET status = 'done', progress = 100, \
                         updated_at = datetime('now') \
                         WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3",
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

    /// todo を削除する（削除できたら `true`）。
    ///
    /// # Errors
    /// 削除失敗時 [`DbError`]。
    pub async fn delete(&self, scope: &UserScope, id: i64) -> Result<bool, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.writer
            .transaction(move |tx| {
                let n = tx
                    .execute(
                        "DELETE FROM todos WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3",
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

/// SQLite 行を [`Todo`] へ変換する（`tags` は JSON 文字列 → `Vec<String>`）。
fn row_to_todo(row: &Row) -> rusqlite::Result<Todo> {
    let tags_json: String = row.get("tags")?;
    let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
    Ok(Todo {
        id: row.get("id")?,
        title: row.get("title")?,
        description: row.get("description")?,
        due_date: row.get("due_date")?,
        start_date: row.get("start_date")?,
        priority: row.get("priority")?,
        tags,
        status: row.get("status")?,
        progress: row.get("progress")?,
        parent_id: row.get("parent_id")?,
        repeat_rule: row.get("repeat_rule")?,
        repeat_until: row.get("repeat_until")?,
        repeat_count: row.get("repeat_count")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}
