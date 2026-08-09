//! `PersonaRepo` — `UserScope` 束縛のデータアクセス（分離キーを型で強制）。
//!
//! persona は `owner_id`（= ユーザーの Discord ID）でスコープされる。**設定オーナーキー**は
//! [`UserScope::config_owner_id`]＝共有 bot ではその**オーナー**、`system_default`／user 単位の
//! 経路では発話ユーザー自身。これにより A が B へ共有した bot では、ペルソナ（一覧・作成・編集・
//! 削除・適用中）が**オーナーの名前空間へ正規化**され、A・B 双方で同一の共有ペルソナ集合を
//! 共同編集できる（`system_default` 共有秘書は従来どおりユーザー単位で独立）。全クエリは
//! `WHERE owner_id = ?` を必須にし、`&UserScope` を取ることで「owner 無しクエリ」を型で不能化する。
//! 読みは [`ReadPool`]、書きは [`WriterHandle`]（BEGIN IMMEDIATE）へ送る。

use rusqlite::{params, OptionalExtension, Row};
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

    /// (user, bot) の適用中ペルソナを設定する（Node `setActivePersonaForBot`）。`persona_id = None` は
    /// **適用解除**（DELETE＝既定ペルソナへ）、`Some(id)` は upsert（`ON CONFLICT(user_id, bot_id)`）。
    /// bot 単位（`scope.bot_id`）でスコープする（秘書ペルソナは Bot 単位で独立・v8）。呼び出し側で
    /// 所有権（owner=user）を検証済み前提（FK: persona_id→personas・user_id→users）。
    ///
    /// # Errors
    /// 書き込み失敗時 [`DbError`]。
    pub async fn set_active(
        &self,
        scope: &UserScope,
        persona_id: Option<i64>,
    ) -> Result<(), DbError> {
        // 共有 bot ではオーナーの行（`config_owner_id`）へ upsert する＝適用中ペルソナを
        // 全ユーザーで 1 つに同期する。`system_default` は発話ユーザー単位（従来どおり独立）。
        let user = owner_id(scope);
        let bot = scope.bot_id().as_str().to_owned();
        self.writer
            .transaction(move |tx| {
                match persona_id {
                    None => {
                        tx.execute(
                            "DELETE FROM bot_active_personas WHERE user_id = ?1 AND bot_id = ?2",
                            params![user, bot],
                        )
                        .map_err(map_sqlite)?;
                    }
                    Some(pid) => {
                        tx.execute(
                            "INSERT INTO bot_active_personas (user_id, bot_id, persona_id, updated_at) \
                             VALUES (?1, ?2, ?3, datetime('now', 'localtime')) \
                             ON CONFLICT(user_id, bot_id) DO UPDATE SET \
                               persona_id = excluded.persona_id, \
                               updated_at = datetime('now', 'localtime')",
                            params![user, bot, pid],
                        )
                        .map_err(map_sqlite)?;
                    }
                }
                Ok(())
            })
            .await
    }

    /// (user, bot) の適用中ペルソナ id を読む（テスト/検証用・無ければ `None`）。
    ///
    /// # Errors
    /// 読み取り失敗時 [`DbError`]。
    pub async fn active_persona_id(&self, scope: &UserScope) -> Result<Option<i64>, DbError> {
        // 共有 bot はオーナーの適用中ペルソナ（`config_owner_id`）を読む＝全ユーザーで同期。
        let user = owner_id(scope);
        let bot = scope.bot_id().as_str().to_owned();
        self.read
            .read(move |conn| {
                conn.query_row(
                    "SELECT persona_id FROM bot_active_personas \
                     WHERE user_id = ?1 AND bot_id = ?2",
                    params![user, bot],
                    |r| r.get::<_, i64>(0),
                )
                .optional()
                .map_err(map_sqlite)
            })
            .await
    }

    /// 所有者本人のペルソナの公開フラグ（`is_public`）を設定する（Node `updatePersona({isPublic})`）。
    /// スコープ内に無ければ `false`。**非公開化（1→0）時は当該ペルソナを推奨に設定している Bot から
    /// 解除**する（`bots.recommended_persona_id = NULL`・§5.2.1）。読み取り・更新・解除を単一 writer Tx で。
    ///
    /// # Errors
    /// 書き込み失敗時 [`DbError`]。
    pub async fn set_public(
        &self,
        scope: &UserScope,
        id: i64,
        is_public: bool,
    ) -> Result<bool, DbError> {
        let owner = owner_id(scope);
        self.writer
            .transaction(move |tx| {
                // 現在の is_public（owner-scoped）。無ければ他人／不在で false。
                let current: Option<i64> = tx
                    .query_row(
                        "SELECT is_public FROM personas WHERE id = ?1 AND owner_id = ?2",
                        params![id, owner],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(map_sqlite)?;
                let Some(current) = current else {
                    return Ok(false);
                };
                let new_public = i64::from(is_public);
                let n = tx
                    .execute(
                        "UPDATE personas SET is_public = ?1, \
                         updated_at = datetime('now', 'localtime') \
                         WHERE id = ?2 AND owner_id = ?3",
                        params![new_public, id, owner],
                    )
                    .map_err(map_sqlite)?;
                // 非公開化（1→0）時は推奨 Bot から解除（Node §5.2.1）。
                if current == 1 && new_public == 0 {
                    tx.execute(
                        "UPDATE bots SET recommended_persona_id = NULL \
                         WHERE recommended_persona_id = ?1",
                        params![id],
                    )
                    .map_err(map_sqlite)?;
                }
                Ok(n > 0)
            })
            .await
    }

    /// 公開ペルソナ（`is_public = 1`）を呼び出し元の所有として**独立コピー**する（Node `importPersona`）。
    /// コピーは `is_public = 0`（既定）。ソースが非公開/不在なら `None`。読み取り + 挿入を単一 writer Tx で
    /// 原子的に行う（read→insert 間にソースが非公開化される競合を排除・Node の別クエリ実装より堅牢）。
    ///
    /// # Errors
    /// 書き込み・作成後取得失敗時 [`DbError`]。
    pub async fn import_public(
        &self,
        scope: &UserScope,
        source_id: i64,
    ) -> Result<Option<Persona>, DbError> {
        let owner = owner_id(scope);
        let new_id = self
            .writer
            .transaction(move |tx| {
                let src: Option<(String, String)> = tx
                    .query_row(
                        "SELECT name, prompt FROM personas WHERE id = ?1 AND is_public = 1",
                        params![source_id],
                        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                    )
                    .optional()
                    .map_err(map_sqlite)?;
                let Some((name, prompt)) = src else {
                    return Ok(None);
                };
                tx.execute(
                    "INSERT INTO personas (owner_id, name, prompt) VALUES (?1, ?2, ?3)",
                    params![owner, name, prompt],
                )
                .map_err(map_sqlite)?;
                Ok(Some(tx.last_insert_rowid()))
            })
            .await?;
        match new_id {
            Some(id) => self.get(scope, id).await,
            None => Ok(None),
        }
    }

    /// 所有者本人のペルソナを削除し、**適用中・推奨設定も掃除する**（Node `deletePersona`）。
    ///
    /// `bot_active_personas`（適用中）は FK `ON DELETE CASCADE` で自動掃除されるが、
    /// `bots.recommended_persona_id` は **FK が無い**ため明示的に `NULL` へ解除する。削除・解除を
    /// 単一 writer Tx で原子的に行う（削除できたら `true`・該当無は `false`）。
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
                if n == 0 {
                    return Ok(false);
                }
                // bot_active_personas は FK ON DELETE CASCADE で自動掃除（foreign_keys=ON）。
                // bots.recommended_persona_id は FK 無しのため明示的に解除する（Node §）。
                tx.execute(
                    "UPDATE bots SET recommended_persona_id = NULL \
                     WHERE recommended_persona_id = ?1",
                    params![id],
                )
                .map_err(map_sqlite)?;
                Ok(true)
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

/// スコープから**設定オーナーキー**（owner_id）を取り出す（`spawn_blocking` の `'static`
/// クロージャ用）。共有 bot ではオーナー、`system_default`／user 単位では発話ユーザー自身
/// （[`UserScope::config_owner_id`]）。これにより共有 bot のペルソナが全ユーザーで同期される。
fn owner_id(scope: &UserScope) -> String {
    scope.config_owner_id().as_str().to_owned()
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
