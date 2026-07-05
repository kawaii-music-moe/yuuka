//! `ContactRepo` — `UserScope` 束縛のデータアクセス（分離キーを型で強制・§12.2 契約5）。
//!
//! 全クエリは `WHERE user_id = ? AND bot_id = ?` を必須にし、`&UserScope` を取ることで
//! 「user_id/bot_id 無しクエリ」を型で不能化する。読みは [`ReadPool`]、書きは
//! [`WriterHandle`]（BEGIN IMMEDIATE）へ送る。

use rusqlite::{params, Row};
use yuuka_core::scope::ScopedRepo;
use yuuka_core::{DbError, UserScope};
use yuuka_db::{map_sqlite, ReadPool, WriterHandle};
use yuuka_web::Db;

use crate::dto::{Contact, NewContact};

/// 返却列（クリーンビュー・内部列 user_id/bot_id/birthday_reminded_year 等は含めない）。
const CONTACT_COLUMNS: &str = "id, name, birthday, relationship, contact_info, notes, tags, \
     created_at, updated_at";

/// 連絡先リポジトリ（DB ハンドルを借用する軽量ラッパ・per-request 構築）。
pub struct ContactRepo<'a> {
    read: &'a ReadPool,
    writer: &'a WriterHandle,
}

impl ScopedRepo for ContactRepo<'_> {}

impl<'a> ContactRepo<'a> {
    /// 共有 DB ハンドルから構築する。
    #[must_use]
    pub fn new(db: &'a Db) -> Self {
        Self {
            read: &db.read,
            writer: &db.writer,
        }
    }

    /// スコープ内の全連絡先を氏名昇順で返す（Node `listContacts`）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list(&self, scope: &UserScope) -> Result<Vec<Contact>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {CONTACT_COLUMNS} FROM contacts \
                     WHERE user_id = ?1 AND bot_id = ?2 ORDER BY name ASC, id ASC"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![uid, bid], row_to_contact)
                    .map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                Ok(out)
            })
            .await
    }

    /// スコープ内の単一連絡先を取得する（無ければ `None`）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn get(&self, scope: &UserScope, id: i64) -> Result<Option<Contact>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {CONTACT_COLUMNS} FROM contacts \
                     WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let mut rows = stmt
                    .query_map(params![id, uid, bid], row_to_contact)
                    .map_err(map_sqlite)?;
                match rows.next() {
                    Some(row) => Ok(Some(row.map_err(map_sqlite)?)),
                    None => Ok(None),
                }
            })
            .await
    }

    /// 連絡先を作成し、作成後の行を返す（Node `addContact`）。
    ///
    /// # Errors
    /// 挿入失敗・作成後の取得失敗時 [`DbError`]。
    pub async fn add(&self, scope: &UserScope, input: NewContact) -> Result<Contact, DbError> {
        let (uid, bid) = scope_keys(scope);
        let id = self
            .writer
            .transaction(move |tx| {
                let tags_json =
                    serde_json::to_string(&input.tags).unwrap_or_else(|_| "[]".to_owned());
                tx.execute(
                    "INSERT INTO contacts \
                       (user_id, bot_id, name, birthday, relationship, contact_info, notes, tags, \
                        created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, \
                             datetime('now', 'localtime'), datetime('now', 'localtime'))",
                    params![
                        uid,
                        bid,
                        input.name,
                        input.birthday,
                        input.relationship,
                        input.contact_info,
                        input.notes,
                        tags_json,
                    ],
                )
                .map_err(map_sqlite)?;
                Ok(tx.last_insert_rowid())
            })
            .await?;
        self.get(scope, id)
            .await?
            .ok_or_else(|| DbError::Operation("inserted contact not found".to_owned()))
    }

    /// 連絡先を更新し、更新後の行を返す（無ければ `None`）。Node `updateContact` 相当。
    ///
    /// route 側で全フィールドを構築済みのため、ここでは全列を上書きする。
    ///
    /// # Errors
    /// 更新・取得失敗時 [`DbError`]。
    pub async fn update(
        &self,
        scope: &UserScope,
        id: i64,
        input: NewContact,
    ) -> Result<Option<Contact>, DbError> {
        let (uid, bid) = scope_keys(scope);
        let changed = self
            .writer
            .transaction(move |tx| {
                let tags_json =
                    serde_json::to_string(&input.tags).unwrap_or_else(|_| "[]".to_owned());
                let n = tx
                    .execute(
                        "UPDATE contacts SET \
                           name = ?1, birthday = ?2, relationship = ?3, contact_info = ?4, \
                           notes = ?5, tags = ?6, updated_at = datetime('now', 'localtime') \
                         WHERE id = ?7 AND user_id = ?8 AND bot_id = ?9",
                        params![
                            input.name,
                            input.birthday,
                            input.relationship,
                            input.contact_info,
                            input.notes,
                            tags_json,
                            id,
                            uid,
                            bid,
                        ],
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

    /// 連絡先を削除する（削除できたら `true`）。Node `deleteContact` 相当。
    ///
    /// # Errors
    /// 削除失敗時 [`DbError`]。
    pub async fn delete(&self, scope: &UserScope, id: i64) -> Result<bool, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.writer
            .transaction(move |tx| {
                let n = tx
                    .execute(
                        "DELETE FROM contacts WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3",
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

/// SQLite 行を [`Contact`] へ変換する（`tags` は JSON 文字列 → `Vec<String>`）。
fn row_to_contact(row: &Row) -> rusqlite::Result<Contact> {
    let tags_json: String = row.get("tags")?;
    let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
    Ok(Contact {
        id: row.get("id")?,
        name: row.get("name")?,
        birthday: row.get("birthday")?,
        relationship: row.get("relationship")?,
        contact_info: row.get("contact_info")?,
        notes: row.get("notes")?,
        tags,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}
