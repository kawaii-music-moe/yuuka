//! 会話ログ（`message_logs`）— Node `src/db/messageLogRepo.ts` の秘書コンテキスト部パリティ。
//!
//! SQLite を正の履歴とする（Redis キャッシュは**意図的に非移植の縮退シーム**＝SQLite 直読み。Node も
//! Redis ミス時は SQLite から再構築するため挙動は「キャッシュ常時ミス」に等しく整合）。コンテキスト
//! リセット境界（floor）は `system_settings` の `context_floor:{botId}:{userId}` に文字列 int で保持する。

use rusqlite::{params, OptionalExtension};
use yuuka_core::DbError;
use yuuka_db::map_sqlite;
use yuuka_web::Db;

/// 直近コンテキストの既定件数（Node `CONTEXT_LIMIT = 15`）。
pub const CONTEXT_LIMIT: i64 = 15;

/// 汎用モード・ギルドコンテキストの既定件数（Node `GUILD_CONTEXT_LIMIT = 30`）。
pub const GUILD_CONTEXT_LIMIT: i64 = 30;

/// 会話 1 発言（LLM へ渡す履歴要素・Node `ContextEntry`）。`role` は `"user"`/`"assistant"`。
#[derive(Debug, Clone)]
pub struct ContextEntry {
    pub role: String,
    pub content: String,
}

/// 秘書コンテキストのリセット境界キー（Node `contextFloorKey`）。
fn context_floor_key(user_id: &str, bot_id: &str) -> String {
    format!("context_floor:{bot_id}:{user_id}")
}

/// owner DM（汎用モード）のリセット境界キー（Node `botDmContextFloorKey`）。秘書と floor を分けて
/// 互いのリセットが干渉しないようにする（SQLite の行自体は `guild_id IS NULL` で秘書と共有）。
fn bot_dm_context_floor_key(user_id: &str, bot_id: &str) -> String {
    format!("context_floor:{bot_id}:dm:{user_id}")
}

/// 送受信メッセージを `message_logs` へ記録する（Node `addMessageLog`・`guild_id = NULL` = 秘書/DM）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn add_message_log(
    db: &Db,
    user_id: &str,
    bot_id: &str,
    role: &str,
    content: &str,
    discord_msg_id: Option<&str>,
    reply_to_msg_id: Option<&str>,
) -> Result<(), DbError> {
    let (user_id, bot_id, role, content) = (
        user_id.to_owned(),
        bot_id.to_owned(),
        role.to_owned(),
        content.to_owned(),
    );
    let discord_msg_id = discord_msg_id.map(str::to_owned);
    let reply_to_msg_id = reply_to_msg_id.map(str::to_owned);
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT INTO message_logs (user_id, bot_id, discord_msg_id, role, content, reply_to_msg_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![user_id, bot_id, discord_msg_id, role, content, reply_to_msg_id],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// 秘書コンテキスト（`guild_id IS NULL`・秘書 floor）を古い順に取得する（Node `getRecentContext`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn recent_context(
    db: &Db,
    user_id: &str,
    bot_id: &str,
    limit: i64,
) -> Result<Vec<ContextEntry>, DbError> {
    recent_context_with_floor(db, user_id, bot_id, context_floor_key(user_id, bot_id), limit).await
}

/// owner DM（汎用モード）コンテキストを古い順に取得する（Node `getBotDmContext`）。SQLite の行は秘書と
/// 同じ（`bot_id × user_id × guild_id IS NULL`）だが、リセット境界だけ DM 専用 floor で分離する。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn recent_bot_dm_context(
    db: &Db,
    bot_id: &str,
    user_id: &str,
    limit: i64,
) -> Result<Vec<ContextEntry>, DbError> {
    recent_context_with_floor(db, user_id, bot_id, bot_dm_context_floor_key(user_id, bot_id), limit)
        .await
}

/// LLM へ渡す直近コンテキストを**古い順**で取得する（Node `getRecentContext`/`getBotDmContext` の SQLite
/// 再構築部）。指定 `floor_key` より後・`guild_id IS NULL`（秘書/DM）の直近 `limit` 件を古い順に返す。
async fn recent_context_with_floor(
    db: &Db,
    user_id: &str,
    bot_id: &str,
    floor_key: String,
    limit: i64,
) -> Result<Vec<ContextEntry>, DbError> {
    let (user_id, bot_id) = (user_id.to_owned(), bot_id.to_owned());
    db.read
        .read(move |conn| {
            // floor（無ければ 0・非数値も 0）。
            let floor: i64 = conn
                .query_row(
                    "SELECT value FROM system_settings WHERE key = ?1",
                    params![floor_key],
                    |r| r.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite)?
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(0);

            let mut stmt = conn
                .prepare(
                    "SELECT role, content FROM ( \
                       SELECT id, role, content FROM message_logs \
                       WHERE user_id = ?1 AND bot_id = ?2 AND id > ?3 AND guild_id IS NULL \
                       ORDER BY id DESC LIMIT ?4 \
                     ) ORDER BY id ASC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![user_id, bot_id, floor, limit], |r| {
                    Ok(ContextEntry {
                        role: r.get::<_, String>(0)?,
                        content: r.get::<_, String>(1)?,
                    })
                })
                .map_err(map_sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite)?;
            Ok(rows)
        })
        .await
}

/// 汎用モードのギルド会話を記録する（Node `addGuildMessageLog`・`guild_id` 非 NULL）。
///
/// 発話者は `[名前]: 本文` プレフィックス済みで渡す（呼び出し側で組む・§4.6.1）。`user_id` には
/// Web 未登録の Discord ユーザー ID も入る（メンバー制 §4.3.3）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
// `message_logs` の各列（user/bot/guild/role/content/msg-id 群）に 1:1 対応するフラット引数
// （秘書版 `add_message_log` と同形・列を struct 化するとかえって読みにくい）。
#[allow(clippy::too_many_arguments)]
pub async fn add_guild_message_log(
    db: &Db,
    bot_id: &str,
    guild_id: &str,
    user_id: &str,
    role: &str,
    content: &str,
    discord_msg_id: Option<&str>,
    reply_to_msg_id: Option<&str>,
) -> Result<(), DbError> {
    let (bot_id, guild_id, user_id, role, content) = (
        bot_id.to_owned(),
        guild_id.to_owned(),
        user_id.to_owned(),
        role.to_owned(),
        content.to_owned(),
    );
    let discord_msg_id = discord_msg_id.map(str::to_owned);
    let reply_to_msg_id = reply_to_msg_id.map(str::to_owned);
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT INTO message_logs (user_id, bot_id, discord_msg_id, role, content, reply_to_msg_id, guild_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![user_id, bot_id, discord_msg_id, role, content, reply_to_msg_id, guild_id],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// 汎用モードのギルドコンテキスト（直近 `limit` 件・古い順）を取得する（Node `getGuildContext` の
/// SQLite 再構築部）。`bot_id × guild_id` スコープで floor は使わない（Node パリティ）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn recent_guild_context(
    db: &Db,
    bot_id: &str,
    guild_id: &str,
    limit: i64,
) -> Result<Vec<ContextEntry>, DbError> {
    let (bot_id, guild_id) = (bot_id.to_owned(), guild_id.to_owned());
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT role, content FROM ( \
                       SELECT id, role, content FROM message_logs \
                       WHERE bot_id = ?1 AND guild_id = ?2 \
                       ORDER BY id DESC LIMIT ?3 \
                     ) ORDER BY id ASC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![bot_id, guild_id, limit], |r| {
                    Ok(ContextEntry {
                        role: r.get::<_, String>(0)?,
                        content: r.get::<_, String>(1)?,
                    })
                })
                .map_err(map_sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite)?;
            Ok(rows)
        })
        .await
}

/// 秘書コンテキストをリセットする（Node `clearContext`）。永続ログは消さず floor を現在の最大 id に
/// 進めることで、以降の再構築で過去メッセージを復元しないようにする。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn clear_context(db: &Db, user_id: &str, bot_id: &str) -> Result<(), DbError> {
    let floor_key = context_floor_key(user_id, bot_id);
    let (user_id, bot_id) = (user_id.to_owned(), bot_id.to_owned());
    db.writer
        .transaction(move |tx| {
            let max_id: Option<i64> = tx
                .query_row(
                    "SELECT MAX(id) FROM message_logs WHERE user_id = ?1 AND bot_id = ?2",
                    params![user_id, bot_id],
                    |r| r.get::<_, Option<i64>>(0),
                )
                .map_err(map_sqlite)?;
            if let Some(max_id) = max_id {
                tx.execute(
                    "INSERT INTO system_settings (key, value) VALUES (?1, ?2) \
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value, \
                     updated_at = datetime('now', 'localtime')",
                    params![floor_key, max_id.to_string()],
                )
                .map_err(map_sqlite)?;
            }
            Ok(())
        })
        .await
}
