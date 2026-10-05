//! guild-assistant ノートのデータアクセス（Node `botNoteRepo`）。
//!
//! - **個人ノート**: `bot_context_notes`（PK `(bot_id, user_id)`）— Bot × 発話ユーザー単位。
//! - **共有ノート**: `bot_guild_notes`（PK `(bot_id, guild_id)`）— Bot × ギルド単位。
//!
//! いずれも単純な key→content。無ければ空文字を返す（Node と同一）。上限（10,000 文字）検証と
//! 追記の改行連結は tool 層で行う（context-note tool と同方針）。

use rusqlite::params;
use yuuka_core::DbError;
use yuuka_db::map_sqlite;
use yuuka_web::Db;

/// ノート上限文字数（Node `BOT_NOTE_MAX_LENGTH`）。
pub const BOT_NOTE_MAX_LENGTH: usize = 10_000;

/// 個人ノート（`bot_context_notes`）の本文を返す（無ければ空文字・Node `getBotUserNote`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_my(db: &Db, bot_id: &str, user_id: &str) -> Result<String, DbError> {
    read_note(
        db,
        "SELECT content FROM bot_context_notes WHERE bot_id = ?1 AND user_id = ?2",
        bot_id.to_owned(),
        user_id.to_owned(),
    )
    .await
}

/// 個人ノートを全置換で upsert する（Node `setBotUserNote`）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn set_my(db: &Db, bot_id: &str, user_id: &str, content: String) -> Result<(), DbError> {
    write_note(
        db,
        "INSERT INTO bot_context_notes (bot_id, user_id, content, updated_at) \
         VALUES (?1, ?2, ?3, datetime('now', 'localtime')) \
         ON CONFLICT(bot_id, user_id) DO UPDATE SET \
           content = excluded.content, updated_at = datetime('now', 'localtime')",
        bot_id.to_owned(),
        user_id.to_owned(),
        content,
    )
    .await
}

/// 共有ノート（`bot_guild_notes`）の本文を返す（無ければ空文字・Node `getBotGuildNote`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_guild(db: &Db, bot_id: &str, guild_id: &str) -> Result<String, DbError> {
    read_note(
        db,
        "SELECT content FROM bot_guild_notes WHERE bot_id = ?1 AND guild_id = ?2",
        bot_id.to_owned(),
        guild_id.to_owned(),
    )
    .await
}

/// 共有ノートを全置換で upsert する（Node `setBotGuildNote`）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn set_guild(
    db: &Db,
    bot_id: &str,
    guild_id: &str,
    content: String,
) -> Result<(), DbError> {
    write_note(
        db,
        "INSERT INTO bot_guild_notes (bot_id, guild_id, content, updated_at) \
         VALUES (?1, ?2, ?3, datetime('now', 'localtime')) \
         ON CONFLICT(bot_id, guild_id) DO UPDATE SET \
           content = excluded.content, updated_at = datetime('now', 'localtime')",
        bot_id.to_owned(),
        guild_id.to_owned(),
        content,
    )
    .await
}

async fn read_note(db: &Db, sql: &'static str, k1: String, k2: String) -> Result<String, DbError> {
    db.read
        .read(move |conn| {
            let mut stmt = conn.prepare(sql).map_err(map_sqlite)?;
            let mut rows = stmt
                .query_map(params![k1, k2], |row| row.get::<_, String>(0))
                .map_err(map_sqlite)?;
            match rows.next() {
                Some(v) => Ok(v.map_err(map_sqlite)?),
                None => Ok(String::new()),
            }
        })
        .await
}

async fn write_note(
    db: &Db,
    sql: &'static str,
    k1: String,
    k2: String,
    content: String,
) -> Result<(), DbError> {
    db.writer
        .transaction(move |tx| {
            tx.execute(sql, params![k1, k2, content])
                .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

// ─── 利用メンバー管理（bot_members・Node `botMemberFunctions`） ────────────────

/// Bot の所有者（作成者）の Discord ID を返す（無ければ `None`・Node `getBotById(botId).user_id`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn bot_owner(db: &Db, bot_id: &str) -> Result<Option<String>, DbError> {
    let bot_id = bot_id.to_owned();
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare("SELECT user_id FROM bots WHERE id = ?1")
                .map_err(map_sqlite)?;
            let mut rows = stmt
                .query_map(params![bot_id], |row| row.get::<_, String>(0))
                .map_err(map_sqlite)?;
            match rows.next() {
                Some(v) => Ok(Some(v.map_err(map_sqlite)?)),
                None => Ok(None),
            }
        })
        .await
}

/// 指定ユーザーが当該ギルドの利用メンバーか（Node `isBotMember`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn is_member(
    db: &Db,
    bot_id: &str,
    guild_id: &str,
    user_id: &str,
) -> Result<bool, DbError> {
    let (b, g, u) = (bot_id.to_owned(), guild_id.to_owned(), user_id.to_owned());
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM bot_members \
                 WHERE bot_id = ?1 AND guild_id = ?2 AND user_id = ?3)",
                params![b, g, u],
                |row| row.get::<_, bool>(0),
            )
            .map_err(map_sqlite)
        })
        .await
}

/// 利用メンバーを追加する（Node `addBotMember`・`INSERT OR IGNORE`）。追加できたら `true`。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn add_member(
    db: &Db,
    bot_id: &str,
    guild_id: &str,
    user_id: &str,
    added_by: &str,
) -> Result<bool, DbError> {
    let (b, g, u, by) = (
        bot_id.to_owned(),
        guild_id.to_owned(),
        user_id.to_owned(),
        added_by.to_owned(),
    );
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute(
                    "INSERT OR IGNORE INTO bot_members (bot_id, guild_id, user_id, added_by) \
                     VALUES (?1, ?2, ?3, ?4)",
                    params![b, g, u, by],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

/// 利用メンバーを削除する（Node `removeBotMember`）。削除できたら `true`。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn remove_member(
    db: &Db,
    bot_id: &str,
    guild_id: &str,
    user_id: &str,
) -> Result<bool, DbError> {
    let (b, g, u) = (bot_id.to_owned(), guild_id.to_owned(), user_id.to_owned());
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute(
                    "DELETE FROM bot_members \
                     WHERE bot_id = ?1 AND guild_id = ?2 AND user_id = ?3",
                    params![b, g, u],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

/// 当該ギルドの利用メンバー（`(user_id, created_at)`）を追加日時昇順で返す（Node `listBotMembers`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_members(
    db: &Db,
    bot_id: &str,
    guild_id: &str,
) -> Result<Vec<(String, String)>, DbError> {
    let (b, g) = (bot_id.to_owned(), guild_id.to_owned());
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT user_id, created_at FROM bot_members \
                     WHERE bot_id = ?1 AND guild_id = ?2 ORDER BY created_at ASC, user_id ASC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![b, g], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(map_sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(map_sqlite)?);
            }
            Ok(out)
        })
        .await
}
