//! `PersonaRepo` — `UserScope` 束縛のデータアクセス（分離キーを型で強制）。
//!
//! persona は `owner_id`（= ユーザーの Discord ID）でスコープされる（bot 単位ではなく
//! **ユーザー単位**で独立）。従って全クエリは `WHERE owner_id = ?` を必須にし、
//! `&UserScope` を取ることで「owner 無しクエリ」を型で不能化する。`bot_id` は persona の
//! 所有権には関与しない（適用中ペルソナ `bot_active_personas` はコア CRUD 外＝deferred）。
//! 読みは [`ReadPool`]、書きは [`WriterHandle`]（BEGIN IMMEDIATE）へ送る。

use rusqlite::{params, Row};
use serde::Serialize;
use yuuka_core::scope::ScopedRepo;
use yuuka_core::{DbError, UserScope};
use yuuka_db::{map_sqlite, ReadPool, WriterHandle};
use yuuka_web::Db;

use crate::dto::{Persona, SavePersona, PERSONA_MAX_LENGTH};

/// マーケットプレイス一覧の 1 件（Node `PublicPersonaView`）。**公開ペルソナは owner を跨いで見せる**
/// ため owner_id ではなく **owner_username**（`users` JOIN・不在は「不明」）を出す。prompt は公開扱い。
#[derive(Debug, Clone, Serialize)]
pub struct MarketplacePersona {
    pub id: i64,
    pub name: String,
    pub prompt: String,
    pub updated_at: String,
    pub owner_username: String,
}

/// 公開ペルソナの全文プレビュー（Node marketplace/:id の `{id, name, prompt}`）。
#[derive(Debug, Clone, Serialize)]
pub struct MarketplacePreview {
    pub id: i64,
    pub name: String,
    pub prompt: String,
}

/// 返却列（クリーンビュー・内部列 owner_id は含めない）。
const PERSONA_COLUMNS: &str = "id, name, prompt, is_public, created_at, updated_at";

/// persona リポジトリ（DB ハンドルを借用する軽量ラッパ・per-request 構築）。
pub struct PersonaRepo<'a> {
    read: &'a ReadPool,
    writer: &'a WriterHandle,
}

impl ScopedRepo for PersonaRepo<'_> {}

impl<'a> PersonaRepo<'a> {
    /// 共有 DB ハンドルから構築する。
    #[must_use]
    pub fn new(db: &'a Db) -> Self {
        Self {
            read: &db.read,
            writer: &db.writer,
        }
    }

    /// 所有者本人のペルソナを更新日時降順で返す。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list(&self, scope: &UserScope) -> Result<Vec<Persona>, DbError> {
        let owner = owner_id(scope);
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {PERSONA_COLUMNS} FROM personas \
                     WHERE owner_id = ?1 ORDER BY updated_at DESC, id DESC"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![owner], row_to_persona)
                    .map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                Ok(out)
            })
            .await
    }

    /// 所有者本人の単一ペルソナを取得する（無ければ `None`）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn get(&self, scope: &UserScope, id: i64) -> Result<Option<Persona>, DbError> {
        let owner = owner_id(scope);
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {PERSONA_COLUMNS} FROM personas \
                     WHERE id = ?1 AND owner_id = ?2"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let mut rows = stmt
                    .query_map(params![id, owner], row_to_persona)
                    .map_err(map_sqlite)?;
                match rows.next() {
                    Some(row) => Ok(Some(row.map_err(map_sqlite)?)),
                    None => Ok(None),
                }
            })
            .await
    }

    /// ペルソナを作成し、作成後の行を返す（`name` は trim・`is_public` は 0）。
    ///
    /// # Errors
    /// 挿入失敗・作成後の取得失敗時 [`DbError`]。バリデーション違反時 [`DbError::Operation`]。
    pub async fn add(&self, scope: &UserScope, input: SavePersona) -> Result<Persona, DbError> {
        let owner = owner_id(scope);
        let name = input.name.trim().to_owned();
        let prompt = input.prompt;
        validate(&name, &prompt)?;
        let id = self
            .writer
            .transaction(move |tx| {
                tx.execute(
                    "INSERT INTO personas (owner_id, name, prompt) VALUES (?1, ?2, ?3)",
                    params![owner, name, prompt],
                )
                .map_err(map_sqlite)?;
                Ok(tx.last_insert_rowid())
            })
            .await?;
        self.get(scope, id)
            .await?
            .ok_or_else(|| DbError::Operation("inserted persona not found".to_owned()))
    }

    /// 所有者本人のペルソナの `name`/`prompt` を更新し、更新後の行を返す（無ければ `None`）。
    ///
    /// # Errors
    /// 更新・取得失敗時 [`DbError`]。バリデーション違反時 [`DbError::Operation`]。
    pub async fn update(
        &self,
        scope: &UserScope,
        id: i64,
        input: SavePersona,
    ) -> Result<Option<Persona>, DbError> {
        // スコープ内に実在しなければ 404 相当（他人／不在は None）。
        if self.get(scope, id).await?.is_none() {
            return Ok(None);
        }
        let name = input.name.trim().to_owned();
        let prompt = input.prompt;
        validate(&name, &prompt)?;
        let owner = owner_id(scope);
        let changed = self
            .writer
            .transaction(move |tx| {
                let n = tx
                    .execute(
                        "UPDATE personas SET name = ?1, prompt = ?2, \
                         updated_at = datetime('now', 'localtime') \
                         WHERE id = ?3 AND owner_id = ?4",
                        params![name, prompt, id, owner],
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

    /// 公開ペルソナ（`is_public = 1`）を更新日時降順で全件返す（Node `listPublicPersonas`・
    /// **owner を跨ぐ公開読み取り**なのでスコープを取らない）。owner_username は `users` JOIN
    /// （不在は「不明」）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list_public(&self) -> Result<Vec<MarketplacePersona>, DbError> {
        self.read
            .read(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT p.id, p.name, p.prompt, p.updated_at, \
                            COALESCE(u.username, '不明') AS owner_username \
                         FROM personas p \
                         LEFT JOIN users u ON u.discord_id = p.owner_id \
                         WHERE p.is_public = 1 \
                         ORDER BY p.updated_at DESC",
                    )
                    .map_err(map_sqlite)?;
                let rows = stmt
                    .query_map([], |r| {
                        Ok(MarketplacePersona {
                            id: r.get(0)?,
                            name: r.get(1)?,
                            prompt: r.get(2)?,
                            updated_at: r.get(3)?,
                            owner_username: r.get(4)?,
                        })
                    })
                    .map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                Ok(out)
            })
            .await
    }

    /// 公開ペルソナ 1 件のプレビューを id で取る（`is_public = 1` のみ・無ければ `None`）。
    /// Node `getPersonaById` + `is_public !== 1 → 404` を 1 クエリに畳み、非公開ペルソナは決して返さない。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn get_public(&self, id: i64) -> Result<Option<MarketplacePreview>, DbError> {
        self.read
            .read(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, name, prompt FROM personas \
                         WHERE id = ?1 AND is_public = 1",
                    )
                    .map_err(map_sqlite)?;
                let mut rows = stmt
                    .query_map(params![id], |r| {
                        Ok(MarketplacePreview {
                            id: r.get(0)?,
                            name: r.get(1)?,
                            prompt: r.get(2)?,
                        })
                    })
                    .map_err(map_sqlite)?;
                match rows.next() {
                    Some(row) => Ok(Some(row.map_err(map_sqlite)?)),
                    None => Ok(None),
                }
            })
            .await
    }

    /// 所有者本人のペルソナを削除する（削除できたら `true`）。
    ///
    /// Node は `bot_active_personas` の適用解除・Bot 推奨設定解除も同時に行うが、
    /// それらはコア CRUD 外のため本スライスでは personas 行の削除のみを行う（deferred）。
    ///
    /// # Errors
    /// 削除失敗時 [`DbError`]。
    pub async fn delete(&self, scope: &UserScope, id: i64) -> Result<bool, DbError> {
        let owner = owner_id(scope);
        self.writer
            .transaction(move |tx| {
                let n = tx
                    .execute(
                        "DELETE FROM personas WHERE id = ?1 AND owner_id = ?2",
                        params![id, owner],
                    )
                    .map_err(map_sqlite)?;
                Ok(n > 0)
            })
            .await
    }
}

/// `name` 必須・`prompt` 長上限（Node `validatePersonaInput` 準拠）。
fn validate(name: &str, prompt: &str) -> Result<(), DbError> {
    if name.trim().is_empty() {
        return Err(DbError::Operation("persona name is required".to_owned()));
    }
    // Node `validatePersonaInput` は `prompt.length`（UTF-16 code unit 数）で判定するため、
    // `encode_utf16().count()` で数える（`chars().count()` は非BMP文字を過小評価し過剰許容）。
    if prompt.encode_utf16().count() > PERSONA_MAX_LENGTH {
        return Err(DbError::Operation(format!(
            "persona prompt exceeds {PERSONA_MAX_LENGTH} chars"
        )));
    }
    Ok(())
}

/// スコープから所有者キー（owner_id）を取り出す（`spawn_blocking` の `'static` クロージャ用）。
fn owner_id(scope: &UserScope) -> String {
    scope.user_id().as_str().to_owned()
}

/// SQLite 行を [`Persona`] へ変換する（`is_public` は 0/1 → bool）。
fn row_to_persona(row: &Row) -> rusqlite::Result<Persona> {
    let is_public: i64 = row.get("is_public")?;
    Ok(Persona {
        id: row.get("id")?,
        name: row.get("name")?,
        prompt: row.get("prompt")?,
        is_public: is_public != 0,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}
