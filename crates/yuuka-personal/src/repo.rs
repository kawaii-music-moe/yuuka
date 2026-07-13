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

use crate::dto::{ClipboardEntry, Contact, NewContact};

/// 返却列（クリーンビュー・内部列 user_id/bot_id/birthday_reminded_year 等は含めない）。
const CONTACT_COLUMNS: &str = "id, name, birthday, relationship, contact_info, notes, tags, \
     created_at, updated_at";

/// クリップボードの返却列（クリーンビュー・内部列 user_id/bot_id は含めない）。
const CLIPBOARD_COLUMNS: &str = "id, content, expires_at, created_at";

/// 連絡先リポジトリ（DB ハンドルを借用する軽量ラッパ・per-request 構築）。
pub struct ContactRepo<'a> {
    pub(crate) read: &'a ReadPool,
    pub(crate) writer: &'a WriterHandle,
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

/// クリップボード（一時メモ・TTL 付き）リポジトリ（`UserScope` 束縛）。Node `clipboardRepo`。
///
/// TTL 一括削除（`deleteExpired`・全ユーザー横断）は cron 専用の別経路（本 CRUD には無い）。
pub struct ClipboardRepo<'a> {
    pub(crate) read: &'a ReadPool,
    pub(crate) writer: &'a WriterHandle,
}

impl ScopedRepo for ClipboardRepo<'_> {}

impl<'a> ClipboardRepo<'a> {
    /// 共有 DB ハンドルから構築する。
    #[must_use]
    pub fn new(db: &'a Db) -> Self {
        Self {
            read: &db.read,
            writer: &db.writer,
        }
    }

    /// TTL 付きで一時メモを追加し、作成後の行を返す（Node `addEntry`）。
    ///
    /// `ttl_hours` は `None`＝無期限（`expires_at` NULL）、`Some(h)`＝`now + h 時間`（ローカル暦・
    /// 秒精度）。Node は JS 側で `toDbDateTime(new Date(Date.now()+h*3600_000))` を計算するが、
    /// ここでは SQLite の `datetime('now','localtime','+N seconds')` で同じローカル秒境界に合わせる
    /// （chrono 非依存・秒精度は Node の `toDbDateTime` の切り捨てと一致）。
    ///
    /// # Errors
    /// 挿入失敗・作成後の取得失敗時 [`DbError`]。
    pub async fn add(
        &self,
        scope: &UserScope,
        content: String,
        ttl_hours: Option<f64>,
    ) -> Result<ClipboardEntry, DbError> {
        let (uid, bid) = scope_keys(scope);
        // ttl_hours=Some → 秒へ変換して datetime 修飾子に（切り捨て＝Node の秒フォーマットと一致）。
        let modifier = ttl_hours.map(|h| format!("+{} seconds", (h * 3600.0) as i64));
        let id = self
            .writer
            .transaction(move |tx| {
                match &modifier {
                    Some(m) => tx.execute(
                        "INSERT INTO clipboard_entries (user_id, bot_id, content, expires_at) \
                         VALUES (?1, ?2, ?3, datetime('now', 'localtime', ?4))",
                        params![uid, bid, content, m],
                    ),
                    None => tx.execute(
                        "INSERT INTO clipboard_entries (user_id, bot_id, content, expires_at) \
                         VALUES (?1, ?2, ?3, NULL)",
                        params![uid, bid, content],
                    ),
                }
                .map_err(map_sqlite)?;
                Ok(tx.last_insert_rowid())
            })
            .await?;
        self.get(scope, id)
            .await?
            .ok_or_else(|| DbError::Operation("inserted clipboard entry not found".to_owned()))
    }

    /// スコープ内の単一エントリを id で取得する（期限フィルタなし・作成直後の取得用）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn get(
        &self,
        scope: &UserScope,
        id: i64,
    ) -> Result<Option<ClipboardEntry>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {CLIPBOARD_COLUMNS} FROM clipboard_entries \
                     WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let mut rows = stmt
                    .query_map(params![id, uid, bid], row_to_clipboard)
                    .map_err(map_sqlite)?;
                match rows.next() {
                    Some(row) => Ok(Some(row.map_err(map_sqlite)?)),
                    None => Ok(None),
                }
            })
            .await
    }

    /// スコープ内の有効な（期限切れでない）エントリを新しい順で返す（Node `listEntries`）。
    ///
    /// `expires_at IS NULL OR expires_at > datetime('now','localtime')`（無期限＋未期限）を
    /// `created_at DESC` で返す（Node と同一）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list(&self, scope: &UserScope) -> Result<Vec<ClipboardEntry>, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {CLIPBOARD_COLUMNS} FROM clipboard_entries \
                     WHERE user_id = ?1 AND bot_id = ?2 \
                       AND (expires_at IS NULL OR expires_at > datetime('now', 'localtime')) \
                     ORDER BY created_at DESC"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![uid, bid], row_to_clipboard)
                    .map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                Ok(out)
            })
            .await
    }

    /// スコープ内のエントリを削除する（削除できたら `true`）。Node `deleteEntry` 相当。
    ///
    /// # Errors
    /// 削除失敗時 [`DbError`]。
    pub async fn delete(&self, scope: &UserScope, id: i64) -> Result<bool, DbError> {
        let (uid, bid) = scope_keys(scope);
        self.writer
            .transaction(move |tx| {
                let n = tx
                    .execute(
                        "DELETE FROM clipboard_entries WHERE user_id = ?1 AND bot_id = ?2 AND id = ?3",
                        params![uid, bid, id],
                    )
                    .map_err(map_sqlite)?;
                Ok(n > 0)
            })
            .await
    }
}

/// コンテキストノート（永続メモ・ユーザー×bot に 1 ドキュメント）リポジトリ。Node `contextNoteRepo`。
pub struct ContextNoteRepo<'a> {
    pub(crate) read: &'a ReadPool,
    pub(crate) writer: &'a WriterHandle,
}

impl ScopedRepo for ContextNoteRepo<'_> {}

impl<'a> ContextNoteRepo<'a> {
    /// 共有 DB ハンドルから構築する。
    #[must_use]
    pub fn new(db: &'a Db) -> Self {
        Self {
            read: &db.read,
            writer: &db.writer,
        }
    }

    /// 全文と更新時刻を返す（未登録なら `("", None)`）。Node `getContextNote` +
    /// `getContextNoteUpdatedAt` を 1 クエリに束ねる（両者は同一行を引くため）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn get(&self, scope: &UserScope) -> Result<(String, Option<String>), DbError> {
        let (uid, bid) = scope_keys(scope);
        self.read
            .read(move |conn| {
                let sql = "SELECT content, updated_at FROM context_notes \
                     WHERE user_id = ?1 AND bot_id = ?2";
                let mut stmt = conn.prepare(sql).map_err(map_sqlite)?;
                let mut rows = stmt
                    .query_map(params![uid, bid], |row| {
                        Ok((row.get::<_, String>("content")?, row.get::<_, Option<String>>("updated_at")?))
                    })
                    .map_err(map_sqlite)?;
                match rows.next() {
                    Some(row) => row.map_err(map_sqlite),
                    None => Ok((String::new(), None)),
                }
            })
            .await
    }

    /// 全文を置換する（upsert）。Node `setContextNote`（`INSERT ... ON CONFLICT DO UPDATE`）。
    ///
    /// 文字数上限の検証は route 側で行う（Node は route と repo の双方で確認するが、Rust では
    /// route で 400 を返すため repo 到達時は上限内が保証される）。
    ///
    /// # Errors
    /// 書き込み失敗時 [`DbError`]。
    pub async fn set(&self, scope: &UserScope, content: String) -> Result<(), DbError> {
        let (uid, bid) = scope_keys(scope);
        self.writer
            .execute(move |conn| {
                conn.execute(
                    "INSERT INTO context_notes (user_id, bot_id, content, updated_at) \
                     VALUES (?1, ?2, ?3, datetime('now', 'localtime')) \
                     ON CONFLICT(user_id, bot_id) DO UPDATE SET \
                       content = excluded.content, updated_at = datetime('now', 'localtime')",
                    params![uid, bid, content],
                )
                .map_err(map_sqlite)?;
                Ok(())
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

/// SQLite 行を [`ClipboardEntry`] へ変換する（クリーンビュー列のみ）。
fn row_to_clipboard(row: &Row) -> rusqlite::Result<ClipboardEntry> {
    Ok(ClipboardEntry {
        id: row.get("id")?,
        content: row.get("content")?,
        expires_at: row.get("expires_at")?,
        created_at: row.get("created_at")?,
    })
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
