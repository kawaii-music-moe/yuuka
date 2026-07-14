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
