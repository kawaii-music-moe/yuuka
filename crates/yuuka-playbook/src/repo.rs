//! `PlaybookRepo` — `UserScope` 束縛のデータアクセス（分離キーを型で強制・§12.2 契約5）。
//!
//! 全クエリは `WHERE user_id = ? AND bot_id = ?` を必須にし、`&UserScope` を取ることで
//! 「user_id/bot_id 無しクエリ」を型で不能化する。読みは [`ReadPool`]、書きは
//! [`WriterHandle`]（BEGIN IMMEDIATE）へ送る。playbook はスコープ内で `name` が一意。

use rusqlite::{params, Row};
use yuuka_core::scope::ScopedRepo;
use yuuka_core::{DbError, UserScope};
use yuuka_db::{map_sqlite, ReadPool, WriterHandle};
use yuuka_web::Db;

use crate::dto::{NewPlaybook, Playbook};

/// 返却列（クリーンビュー・内部列 id/user_id/bot_id/created_at/updated_at は含めない）。
const PLAYBOOK_COLUMNS: &str = "name, title, keywords, description, steps";

/// playbook リポジトリ（DB ハンドルを借用する軽量ラッパ・per-request 構築）。
pub struct PlaybookRepo<'a> {
    read: &'a ReadPool,
    writer: &'a WriterHandle,
}

impl ScopedRepo for PlaybookRepo<'_> {}

impl<'a> PlaybookRepo<'a> {
    /// 共有 DB ハンドルから構築する。
    #[must_use]
    pub fn new(db: &'a Db) -> Self {
        Self {
            read: &db.read,
            writer: &db.writer,
        }
    }

    /// スコープ内の playbook を更新日時降順で返す（`query` 指定時は部分一致で絞る）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list(
        &self,
        scope: &UserScope,
        query: Option<String>,
    ) -> Result<Vec<Playbook>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let mut out = Vec::new();
                match query {
                    Some(q) if !q.is_empty() => {
                        let like = format!("%{q}%");
                        let sql = format!(
                            "SELECT {PLAYBOOK_COLUMNS} FROM playbooks \
                             WHERE user_id = ?1 AND bot_id = ?2 AND ( \
                               name LIKE ?3 OR title LIKE ?3 OR description LIKE ?3 \
                               OR steps LIKE ?3 OR keywords LIKE ?3 ) \
                             ORDER BY updated_at DESC, name ASC"
                        );
                        let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                        let rows = stmt
                            .query_map(params![uid, bid, like], row_to_playbook)
                            .map_err(map_sqlite)?;
                        for row in rows {
                            out.push(row.map_err(map_sqlite)?);
                        }
                    }
                    _ => {
                        let sql = format!(
                            "SELECT {PLAYBOOK_COLUMNS} FROM playbooks \
                             WHERE user_id = ?1 AND bot_id = ?2 \
                             ORDER BY updated_at DESC, name ASC"
                        );
                        let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                        let rows = stmt
                            .query_map(params![uid, bid], row_to_playbook)
                            .map_err(map_sqlite)?;
                        for row in rows {
                            out.push(row.map_err(map_sqlite)?);
                        }
                    }
                }
                Ok(out)
            })
            .await
    }

    /// スコープ内の単一 playbook を名前で取得する（無ければ `None`）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn get(&self, scope: &UserScope, name: String) -> Result<Option<Playbook>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {PLAYBOOK_COLUMNS} FROM playbooks \
                     WHERE user_id = ?1 AND bot_id = ?2 AND name = ?3"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let mut rows = stmt
                    .query_map(params![uid, bid, name], row_to_playbook)
                    .map_err(map_sqlite)?;
                match rows.next() {
                    Some(row) => Ok(Some(row.map_err(map_sqlite)?)),
                    None => Ok(None),
                }
            })
            .await
    }

    /// playbook を upsert し、保存後の行を返す（Node `savePlaybook`）。
    ///
    /// `name` は Node 同様 `[^a-zA-Z0-9\-_]` を `_` に置換し小文字化する。正規化後が空なら
    /// [`DbError::Operation`]（route 層で 400 に変換）。
    ///
    /// # Errors
    /// 正規化後 name が空、または挿入／取得失敗時 [`DbError`]。
    pub async fn save(&self, scope: &UserScope, input: NewPlaybook) -> Result<Playbook, DbError> {
        let (uid, bid) = scope_keys(scope);
        let safe_name = normalize_name(&input.name);
        if safe_name.is_empty() {
            return Err(DbError::Operation("playbook name is invalid".to_owned()));
        }
        let name_for_get = safe_name.clone();
        self.writer
            .transaction(move |tx| {
                let keywords_json =
                    serde_json::to_string(&input.keywords).unwrap_or_else(|_| "[]".to_owned());
                tx.execute(
                    "INSERT INTO playbooks \
                       (user_id, bot_id, name, title, keywords, description, steps, \
                        created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, \
                             datetime('now', 'localtime'), datetime('now', 'localtime')) \
                     ON CONFLICT(user_id, bot_id, name) DO UPDATE SET \
                       title = excluded.title, \
                       keywords = excluded.keywords, \
                       description = excluded.description, \
                       steps = excluded.steps, \
                       updated_at = datetime('now', 'localtime')",
                    params![
                        uid,
                        bid,
                        safe_name,
                        input.title,
                        keywords_json,
                        input.description,
                        input.steps,
                    ],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await?;
        self.get(scope, name_for_get)
            .await?
            .ok_or_else(|| DbError::Operation("saved playbook not found".to_owned()))
    }

    /// playbook を名前で削除する（削除できたら `true`）。
    ///
    /// # Errors
    /// 削除失敗時 [`DbError`]。
    pub async fn delete(&self, scope: &UserScope, name: String) -> Result<bool, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.writer
            .transaction(move |tx| {
                let n = tx
                    .execute(
                        "DELETE FROM playbooks WHERE user_id = ?1 AND bot_id = ?2 AND name = ?3",
                        params![uid, bid, name],
                    )
                    .map_err(map_sqlite)?;
                Ok(n > 0)
            })
            .await
    }
}

/// Node `savePlaybook` の name 正規化（`[^a-zA-Z0-9\-_]` を `_` に、小文字化）。
fn normalize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// スコープから所有 String キーを取り出す（`spawn_blocking` の `'static` クロージャ用）。
fn scope_keys(scope: &UserScope) -> (String, String) {
    (
        scope.user_id().as_str().to_owned(),
        scope.bot_id().as_str().to_owned(),
    )
}

/// SQLite 行を [`Playbook`] へ変換する（`keywords` は JSON 文字列 → `Vec<String>`）。
fn row_to_playbook(row: &Row) -> rusqlite::Result<Playbook> {
    let keywords_json: Option<String> = row.get("keywords")?;
    let keywords: Vec<String> = keywords_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    Ok(Playbook {
        name: row.get("name")?,
        title: row.get("title")?,
        keywords,
        description: row.get::<_, Option<String>>("description")?.unwrap_or_default(),
        steps: row.get::<_, Option<String>>("steps")?.unwrap_or_default(),
    })
}
