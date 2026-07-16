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

/// 利用量サマリの 1 日ぶん（Node `getBotUsageSeries` の `series` 要素・`requests`=user 発話数・
/// `responses`=assistant 応答数）。
#[derive(Debug, Clone)]
pub struct UsagePoint {
    pub date: String,
    pub requests: i64,
    pub responses: i64,
}

/// Bot 利用量の時系列（連続日付・欠損 0 補完済み）＋合計（Node `getBotUsageSeries` の戻り）。
#[derive(Debug, Clone)]
pub struct BotUsageSeries {
    pub series: Vec<UsagePoint>,
    pub total_requests: i64,
    pub total_responses: i64,
}

/// Bot 単位の利用量を直近 `days` 日ぶん集計する（Node `getBotUsageSeries`・**bot_id 単位の
/// 読み取り専用クエリ**＝コスト可視化用に user_id 全件走査の明示的例外）。`days` は `[1, 90]` に
/// クランプする。連続日付列は SQLite の `date('now','localtime')` から生成し（WHERE と同一基準で
/// TZ ドリフトを避ける）、ログの無い日は 0 補完する。role が `user`/`assistant` 以外は数えない。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_bot_usage_series(
    db: &Db,
    bot_id: &str,
    days: i64,
) -> Result<BotUsageSeries, DbError> {
    let bot_id = bot_id.to_owned();
    let clamped = days.clamp(1, 90);
    db.read
        .read(move |conn| {
            // 再帰 CTE で「今日〜今日-(clamped-1)」の連続 local 日付を作り、LEFT JOIN で 0 補完集計する。
            // 予約語（offset 等）を避けた別名を使う。
            let mut stmt = conn
                .prepare(
                    "WITH RECURSIVE offsets(k) AS ( \
                       SELECT 0 UNION ALL SELECT k + 1 FROM offsets WHERE k < ?1 - 1 \
                     ), \
                     days_list(day) AS ( \
                       SELECT date('now', 'localtime', '-' || k || ' days') FROM offsets \
                     ) \
                     SELECT days_list.day AS day, \
                       COALESCE(SUM(CASE WHEN ml.role = 'user' THEN 1 ELSE 0 END), 0) AS requests, \
                       COALESCE(SUM(CASE WHEN ml.role = 'assistant' THEN 1 ELSE 0 END), 0) AS responses \
                     FROM days_list \
                     LEFT JOIN message_logs ml \
                       ON date(ml.created_at) = days_list.day AND ml.bot_id = ?2 \
                     GROUP BY days_list.day \
                     ORDER BY days_list.day ASC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![clamped, bot_id], |r| {
                    Ok(UsagePoint {
                        date: r.get::<_, String>(0)?,
                        requests: r.get::<_, i64>(1)?,
                        responses: r.get::<_, i64>(2)?,
                    })
                })
                .map_err(map_sqlite)?;
            let mut series = Vec::new();
            let (mut total_requests, mut total_responses) = (0_i64, 0_i64);
            for row in rows {
                let p = row.map_err(map_sqlite)?;
                total_requests += p.requests;
                total_responses += p.responses;
                series.push(p);
            }
            Ok(BotUsageSeries {
                series,
                total_requests,
                total_responses,
            })
        })
        .await
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
    recent_context_with_floor(
        db,
        user_id,
        bot_id,
        context_floor_key(user_id, bot_id),
        limit,
    )
    .await
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
    recent_context_with_floor(
        db,
        user_id,
        bot_id,
        bot_dm_context_floor_key(user_id, bot_id),
        limit,
    )
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

/// Bot 単位の日次利用件数（Node `countBotDailyUsage`・`role='user'` を日付集計・降順）。
///
/// `days` は `[1,90]` に floor + clamp する。`created_at >= date('now','localtime','-N days')` で範囲を
/// 絞り、`date(created_at)`（localtime 修飾子なし＝列 DEFAULT が既に localtime のため二重変換を避ける）で
/// グルーピングする。`assistant-config` のコスト可視化用（`user_id` 全件走査の明示的例外）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn count_bot_daily_usage(
    db: &Db,
    bot_id: &str,
    days: i64,
) -> Result<Vec<(String, i64)>, DbError> {
    let clamped = days.clamp(1, 90);
    let modifier = format!("-{clamped} days");
    let bot_id = bot_id.to_owned();
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT date(created_at) AS date, COUNT(*) AS count FROM message_logs \
                     WHERE bot_id = ?1 AND role = 'user' AND created_at >= date('now', 'localtime', ?2) \
                     GROUP BY date(created_at) ORDER BY date DESC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![bot_id, modifier], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::get_bot_usage_series;
    use yuuka_web::Db;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn seed_db() -> (Db, std::path::PathBuf) {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_msglog_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let conn = rusqlite::Connection::open(&path).expect("create empty");
            drop(conn);
        }
        let db = Db::open(&path).expect("open");
        (db, path)
    }

    /// `created_at` を `day_offset` 日前に明示して message_logs へ 1 行入れる。
    fn insert_log(path: &std::path::Path, bot_id: &str, role: &str, day_offset: i64) {
        let conn = rusqlite::Connection::open(path).expect("open");
        conn.execute(
            "INSERT INTO message_logs (user_id, bot_id, role, content, created_at) \
             VALUES ('u', ?1, ?2, 'c', datetime('now', 'localtime', ?3))",
            rusqlite::params![bot_id, role, format!("-{day_offset} days")],
        )
        .expect("insert log");
    }

    #[tokio::test]
    async fn usage_series_gap_fills_totals_and_scopes_to_bot() {
        let (db, path) = seed_db();
        // 今日: user×2, assistant×1。昨日: user×1。3日前: 範囲外(days=3=今日含む3日)。
        insert_log(&path, "b1", "user", 0);
        insert_log(&path, "b1", "user", 0);
        insert_log(&path, "b1", "assistant", 0);
        insert_log(&path, "b1", "user", 1);
        insert_log(&path, "b1", "user", 3);
        // 別 Bot のログは混ざらない。
        insert_log(&path, "other", "user", 0);
        // user/assistant 以外の role は数えない。
        insert_log(&path, "b1", "system", 0);

        let s = get_bot_usage_series(&db, "b1", 3).await.unwrap();
        assert_eq!(s.series.len(), 3, "連続3日ぶん(欠損0補完)");
        // 昇順: [今日-2, 今日-1(昨日), 今日]。
        assert_eq!(s.series[0].requests, 0);
        assert_eq!(s.series[0].responses, 0);
        assert_eq!(s.series[1].requests, 1); // 昨日: user×1
        assert_eq!(s.series[2].requests, 2); // 今日: user×2
        assert_eq!(s.series[2].responses, 1); // 今日: assistant×1
        assert_eq!(s.total_requests, 3); // 範囲外(3日前)・別Bot・system は除外
        assert_eq!(s.total_responses, 1);

        // ログ無し Bot は全 0・長さは days。
        let z = get_bot_usage_series(&db, "system_default", 5)
            .await
            .unwrap();
        assert_eq!(z.series.len(), 5);
        assert_eq!(z.total_requests, 0);
        assert_eq!(z.total_responses, 0);

        // days クランプ: 0→1、200→90。
        assert_eq!(
            get_bot_usage_series(&db, "b1", 0)
                .await
                .unwrap()
                .series
                .len(),
            1
        );
        assert_eq!(
            get_bot_usage_series(&db, "b1", 200)
                .await
                .unwrap()
                .series
                .len(),
            90
        );
    }
}
