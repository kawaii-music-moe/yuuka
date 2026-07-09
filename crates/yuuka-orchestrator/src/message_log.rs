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

/// 会話 1 発言（LLM へ渡す履歴要素・Node `ContextEntry`）。`role` は `"user"`/`"assistant"`。
#[derive(Debug, Clone)]
pub struct ContextEntry {
    pub role: String,
    pub content: String,
}

/// `context_floor:{botId}:{userId}` キー（Node `contextFloorKey`）。
fn context_floor_key(user_id: &str, bot_id: &str) -> String {
    format!("context_floor:{bot_id}:{user_id}")
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

/// LLM へ渡す直近コンテキストを**古い順**で取得する（Node `getRecentContext` の SQLite 再構築部）。
///
/// リセット境界（floor）より後・`guild_id IS NULL`（秘書/DM）の直近 `limit` 件を古い順に返す。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn recent_context(
    db: &Db,
    user_id: &str,
    bot_id: &str,
    limit: i64,
) -> Result<Vec<ContextEntry>, DbError> {
    let floor_key = context_floor_key(user_id, bot_id);
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
