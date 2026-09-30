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

/// 秘書コンテキストのリセット境界キー（Node `contextFloorKey`）。**Discord 専用**（後方互換のため
/// 無変更で維持・`source` 導入前からの既存境界をそのまま引き継ぐ）。
fn context_floor_key(user_id: &str, bot_id: &str) -> String {
    format!("context_floor:{bot_id}:{user_id}")
}

/// owner DM（汎用モード）のリセット境界キー（Node `botDmContextFloorKey`）。秘書と floor を分けて
/// 互いのリセットが干渉しないようにする（SQLite の行自体は `guild_id IS NULL` で秘書と共有）。
fn bot_dm_context_floor_key(user_id: &str, bot_id: &str) -> String {
    format!("context_floor:{bot_id}:dm:{user_id}")
}

/// PWA コンテキストのリセット境界キー（issue #33/#38: Discord とは別軸で管理する）。
///
/// Node の `source` 導入（8/14 merge bb97bae）は `contextKey`/`contextFloorKey` を
/// `` `${botId}:${source}` `` で束ねたのに対し `clearContext` だけ据え置きのままで、書き込み/読み出しと
/// リセットのキーが食い違って「リセットしても消えない」バグ（issue #38）を生んだ。Rust では
/// **読み出し（[`recent_pwa_context`]）とリセット（[`clear_pwa_context`]）が必ずこの同じ関数を経由する**
/// ようにして、同種のキー不一致を構造的に起こせなくする。
fn pwa_context_floor_key(user_id: &str, bot_id: &str) -> String {
    format!("context_floor:{bot_id}:pwa:{user_id}")
}

/// 送受信メッセージを `message_logs` へ記録する（Node `addMessageLog`・`guild_id = NULL` = 秘書/DM・
/// `source` は既定 `"discord"`）。
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
    add_message_log_for(
        db,
        user_id,
        bot_id,
        role,
        content,
        discord_msg_id,
        reply_to_msg_id,
        "discord",
    )
    .await
}

/// PWA 発の送受信メッセージを `message_logs` へ記録する（`source = "pwa"`・discord_msg_id/reply_to_msg_id
/// は常に `NULL`）。issue #33 の PWA チャット送信用。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn add_pwa_message_log(
    db: &Db,
    user_id: &str,
    bot_id: &str,
    role: &str,
    content: &str,
) -> Result<(), DbError> {
    add_message_log_for(db, user_id, bot_id, role, content, None, None, "pwa").await
}

/// [`add_message_log`]/[`add_pwa_message_log`] の共通実装。`source` を明示的に列へ書く
/// （マイグレーション V21 の `DEFAULT 'discord'` に頼らず、Discord 経路も含めて全 INSERT が
/// 自分の由来を明示する）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
#[allow(clippy::too_many_arguments)]
async fn add_message_log_for(
    db: &Db,
    user_id: &str,
    bot_id: &str,
    role: &str,
    content: &str,
    discord_msg_id: Option<&str>,
    reply_to_msg_id: Option<&str>,
    source: &str,
) -> Result<(), DbError> {
    let (user_id, bot_id, role, content, source) = (
        user_id.to_owned(),
        bot_id.to_owned(),
        role.to_owned(),
        content.to_owned(),
        source.to_owned(),
    );
    let discord_msg_id = discord_msg_id.map(str::to_owned);
    let reply_to_msg_id = reply_to_msg_id.map(str::to_owned);
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT INTO message_logs \
                   (user_id, bot_id, discord_msg_id, role, content, reply_to_msg_id, source) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    user_id,
                    bot_id,
                    discord_msg_id,
                    role,
                    content,
                    reply_to_msg_id,
                    source
                ],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// 秘書コンテキスト（`guild_id IS NULL`・`source='discord'`・秘書 floor）を古い順に取得する
/// （Node `getRecentContext`）。
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
        "discord",
        limit,
    )
    .await
}

/// PWA コンテキスト（`guild_id IS NULL`・`source='pwa'`・PWA floor）を古い順に取得する（issue #33）。
///
/// [`pwa_context_floor_key`] を [`clear_pwa_context`] と共有するため、書き込み/読み出しとリセットの
/// キーが食い違う心配がない（issue #38 の Node バグの再発防止）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn recent_pwa_context(
    db: &Db,
    user_id: &str,
    bot_id: &str,
    limit: i64,
) -> Result<Vec<ContextEntry>, DbError> {
    recent_context_with_floor(
        db,
        user_id,
        bot_id,
        pwa_context_floor_key(user_id, bot_id),
        "pwa",
        limit,
    )
    .await
}

/// owner DM（汎用モード）コンテキストを古い順に取得する（Node `getBotDmContext`）。SQLite の行は秘書と
/// 同じ（`bot_id × user_id × guild_id IS NULL`・`source='discord'`）だが、リセット境界だけ DM 専用
/// floor で分離する。
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
        "discord",
        limit,
    )
    .await
}

/// LLM へ渡す直近コンテキストを**古い順**で取得する（Node `getRecentContext`/`getBotDmContext` の SQLite
/// 再構築部）。指定 `floor_key` より後・`guild_id IS NULL`（秘書/DM）・指定 `source` の直近 `limit` 件を
/// 古い順に返す。
async fn recent_context_with_floor(
    db: &Db,
    user_id: &str,
    bot_id: &str,
    floor_key: String,
    source: &str,
    limit: i64,
) -> Result<Vec<ContextEntry>, DbError> {
    let (user_id, bot_id, source) = (user_id.to_owned(), bot_id.to_owned(), source.to_owned());
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
                         AND source = ?5 \
                       ORDER BY id DESC LIMIT ?4 \
                     ) ORDER BY id ASC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![user_id, bot_id, floor, limit, source], |r| {
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

/// PWA チャット画面の履歴一覧 1 件（Node `MessageLogRecord` の PWA 表示に使う部分集合）。
#[derive(Debug, Clone)]
pub struct PwaMessageRow {
    pub id: i64,
    /// `"user" | "assistant"`。
    pub role: String,
    pub content: String,
    /// `'YYYY-MM-DD HH:MM:SS'`（SQLite localtime）。
    pub created_at: String,
}

/// PWA チャット履歴を古い順に返す（Node `listPwaMessages`・`source='pwa'` のみ・floor 非依存＝
/// リセット後も永続ログ自体は表示する、Node と同じ「履歴一覧は全件、LLM コンテキストだけ floor で
/// 絞る」設計）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_pwa_messages(
    db: &Db,
    user_id: &str,
    bot_id: &str,
    limit: i64,
) -> Result<Vec<PwaMessageRow>, DbError> {
    let (user_id, bot_id) = (user_id.to_owned(), bot_id.to_owned());
    let limit = limit.clamp(1, 200);
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, role, content, created_at FROM ( \
                       SELECT id, role, content, created_at FROM message_logs \
                       WHERE user_id = ?1 AND bot_id = ?2 AND source = 'pwa' \
                       ORDER BY id DESC LIMIT ?3 \
                     ) ORDER BY id ASC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![user_id, bot_id, limit], |r| {
                    Ok(PwaMessageRow {
                        id: r.get::<_, i64>(0)?,
                        role: r.get::<_, String>(1)?,
                        content: r.get::<_, String>(2)?,
                        created_at: r.get::<_, String>(3)?,
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
    channel_id: Option<&str>,
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
    let channel_id = channel_id.map(str::to_owned);
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT INTO message_logs (user_id, bot_id, discord_msg_id, role, content, reply_to_msg_id, guild_id, channel_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![user_id, bot_id, discord_msg_id, role, content, reply_to_msg_id, guild_id, channel_id],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// 汎用モードのギルドコンテキスト（直近 `limit` 件・古い順）を取得する（Node `getGuildContext` の
/// SQLite 再構築部）。floor は使わない（Node パリティ）。
///
/// `channel_id` があれば `bot_id × guild_id × channel_id` の厳密一致で引き、チャンネル間で会話が
/// 混ざらないようにする（V19）。チャンネル不明の経路（Web 等）は従来どおりギルド全体で引く。
/// V19 以前の行（channel_id = NULL）は厳密一致に含まれない＝チャンネル別履歴は移行後に育つ。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn recent_guild_context(
    db: &Db,
    bot_id: &str,
    guild_id: &str,
    channel_id: Option<&str>,
    limit: i64,
) -> Result<Vec<ContextEntry>, DbError> {
    let (bot_id, guild_id) = (bot_id.to_owned(), guild_id.to_owned());
    let channel_id = channel_id.map(str::to_owned);
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT role, content FROM ( \
                       SELECT id, role, content FROM message_logs \
                       WHERE bot_id = ?1 AND guild_id = ?2 \
                         AND (?4 IS NULL OR channel_id = ?4) \
                       ORDER BY id DESC LIMIT ?3 \
                     ) ORDER BY id ASC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![bot_id, guild_id, limit, channel_id], |r| {
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

/// PWA コンテキストをリセットする（issue #33/#38）。[`clear_context`] の PWA 版で、境界キーは
/// 必ず [`pwa_context_floor_key`]（[`recent_pwa_context`] と共有）を使う。
///
/// Node の bug（issue #38）は、コンテキストを `source` 別キーへ分けた際に `clearContext` だけ
/// 旧キーのまま据え置かれ、リセットが何も消さなくなった。Rust では読み出し・リセットが同じ
/// `pwa_context_floor_key` 関数を経由するため、この種のキー不一致は構造的に起こらない。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn clear_pwa_context(db: &Db, user_id: &str, bot_id: &str) -> Result<(), DbError> {
    let floor_key = pwa_context_floor_key(user_id, bot_id);
    let (user_id, bot_id) = (user_id.to_owned(), bot_id.to_owned());
    db.writer
        .transaction(move |tx| {
            // PWA (`source='pwa'`) の最大 id のみを境界にする（Discord 側の id が pwa より新しくても
            // 巻き込まない・[`recent_pwa_context`] が `source='pwa'` で絞るのと対称）。
            let max_id: Option<i64> = tx
                .query_row(
                    "SELECT MAX(id) FROM message_logs \
                     WHERE user_id = ?1 AND bot_id = ?2 AND source = 'pwa'",
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

    // ─── issue #33/#38: source 分離コンテキスト（discord/pwa）のテスト ───

    use super::{
        add_message_log, add_pwa_message_log, clear_context, clear_pwa_context, list_pwa_messages,
        recent_context, recent_pwa_context, CONTEXT_LIMIT,
    };

    /// `users` 行を用意する（`message_logs`/`context_notes` の FK 対象）。
    fn seed_user(path: &std::path::Path, user_id: &str) {
        let conn = rusqlite::Connection::open(path).expect("open");
        conn.execute(
            "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt) \
             VALUES (?1, ?1, 'x', 'x')",
            rusqlite::params![user_id],
        )
        .expect("seed user");
    }

    #[tokio::test]
    async fn pwa_and_discord_contexts_are_isolated() {
        let (db, path) = seed_db();
        seed_user(&path, "u");

        add_message_log(&db, "u", "b1", "user", "discord-1", None, None)
            .await
            .unwrap();
        add_pwa_message_log(&db, "u", "b1", "user", "pwa-1")
            .await
            .unwrap();

        // Discord 側の再構築には PWA の発話が混ざらない。
        let discord_ctx = recent_context(&db, "u", "b1", CONTEXT_LIMIT).await.unwrap();
        assert_eq!(discord_ctx.len(), 1);
        assert_eq!(discord_ctx[0].content, "discord-1");

        // PWA 側の再構築には Discord の発話が混ざらない。
        let pwa_ctx = recent_pwa_context(&db, "u", "b1", CONTEXT_LIMIT)
            .await
            .unwrap();
        assert_eq!(pwa_ctx.len(), 1);
        assert_eq!(pwa_ctx[0].content, "pwa-1");
    }

    #[tokio::test]
    async fn clear_pwa_context_resets_pwa_only_and_uses_same_key_as_read() {
        let (db, path) = seed_db();
        seed_user(&path, "u");

        add_pwa_message_log(&db, "u", "b1", "user", "pwa-before")
            .await
            .unwrap();
        add_message_log(&db, "u", "b1", "user", "discord-before", None, None)
            .await
            .unwrap();

        clear_pwa_context(&db, "u", "b1").await.unwrap();

        // PWA コンテキストはリセット後、境界以前の発話を含まない（issue #38 の再発防止）。
        let pwa_ctx = recent_pwa_context(&db, "u", "b1", CONTEXT_LIMIT)
            .await
            .unwrap();
        assert!(
            pwa_ctx.is_empty(),
            "pwa clear は pwa floor を境界まで進める"
        );

        // Discord コンテキストは pwa のリセットに巻き込まれない（境界キーが別）。
        let discord_ctx = recent_context(&db, "u", "b1", CONTEXT_LIMIT).await.unwrap();
        assert_eq!(discord_ctx.len(), 1);
        assert_eq!(discord_ctx[0].content, "discord-before");

        // 新しい PWA 発話は境界より後なので見える。
        add_pwa_message_log(&db, "u", "b1", "user", "pwa-after")
            .await
            .unwrap();
        let pwa_ctx_after = recent_pwa_context(&db, "u", "b1", CONTEXT_LIMIT)
            .await
            .unwrap();
        assert_eq!(pwa_ctx_after.len(), 1);
        assert_eq!(pwa_ctx_after[0].content, "pwa-after");
    }

    #[tokio::test]
    async fn clear_context_discord_does_not_affect_pwa_context() {
        // 対称性の確認: 既存の (discord 用) clear_context も pwa 側を巻き込まない。
        let (db, path) = seed_db();
        seed_user(&path, "u");

        add_message_log(&db, "u", "b1", "user", "discord-before", None, None)
            .await
            .unwrap();
        add_pwa_message_log(&db, "u", "b1", "user", "pwa-before")
            .await
            .unwrap();

        clear_context(&db, "u", "b1").await.unwrap();

        assert!(recent_context(&db, "u", "b1", CONTEXT_LIMIT)
            .await
            .unwrap()
            .is_empty());
        let pwa_ctx = recent_pwa_context(&db, "u", "b1", CONTEXT_LIMIT)
            .await
            .unwrap();
        assert_eq!(pwa_ctx.len(), 1, "discord の clear は pwa floor を進めない");
    }

    #[tokio::test]
    async fn list_pwa_messages_returns_pwa_only_oldest_first_and_ignores_floor() {
        let (db, path) = seed_db();
        seed_user(&path, "u");

        add_pwa_message_log(&db, "u", "b1", "user", "pwa-1")
            .await
            .unwrap();
        add_message_log(&db, "u", "b1", "user", "discord-1", None, None)
            .await
            .unwrap();
        add_pwa_message_log(&db, "u", "b1", "assistant", "pwa-2")
            .await
            .unwrap();

        // 履歴一覧は floor 非依存（リセット後も全件表示）: 先に clear しても件数は変わらない。
        clear_pwa_context(&db, "u", "b1").await.unwrap();

        let history = list_pwa_messages(&db, "u", "b1", 50).await.unwrap();
        assert_eq!(history.len(), 2, "discord のログは含まず pwa のみ");
        assert_eq!(history[0].content, "pwa-1", "古い順");
        assert_eq!(history[1].content, "pwa-2");
    }
}
