//! integrated データアクセス（self-contained・Node `botRepo` / `mcpRepo` / `credentialAccessRepo` /
//! `credentialRepo` の該当セッターのパリティ）。
//!
//! yuuka-orchestrator へは依存せず直接 SQL を書く（DAG を平坦に保つ）。Google 系は yuuka-google の
//! repo を再利用する（本ファイルは扱わない）。会話履歴クリアは Redis 非依存で `system_settings` の
//! floor を進める（yuuka-orchestrator `message_log.rs::clear_context` と同一挙動）。

use rusqlite::{params, OptionalExtension};
use serde_json::Value;
use yuuka_core::DbError;
use yuuka_db::map_sqlite;
use yuuka_web::Db;

/// 既知ケーパビリティ（Node `KNOWN_CAPABILITIES`・core は暗黙付与のため書かない）。
const KNOWN_CAPABILITIES: [&str; 4] = ["persona", "memory", "mcp", "secretary"];

/// Bot 1 件の統合管理ビュー（Node `BotRecord` のうち本ページで使う列のみ）。
#[derive(Debug, Clone)]
pub struct BotRow {
    /// Bot ID（`system_default` を含む）。
    pub id: String,
    /// Bot 作成者（オーナー）の discord_id。
    pub user_id: String,
    /// 表示名。
    pub name: String,
    /// 管理者による停止処分中か（`suspended == 1`）。
    pub suspended: bool,
    /// オーナーによる手動停止希望（`stopped == 1`）。
    pub stopped: bool,
    /// Discord トークン暗号文が設定済みか（非 NULL かつ非空）。
    pub has_token: bool,
    /// Discord から同期された表示名（未設定は `None`）。
    pub discord_username: Option<String>,
    /// Discord から同期されたアバター URL（未設定は `None`）。
    pub discord_avatar_url: Option<String>,
    /// ケーパビリティ JSON 文字列（`null` 可）。
    pub capabilities: Option<String>,
}

/// MCP サーバー 1 件の統合管理ビュー（Node `McpServerRecord` のうち本ページで使う列のみ）。
#[derive(Debug, Clone)]
pub struct McpServerRow {
    /// サーバー ID。
    pub id: i64,
    /// 表示名。
    pub name: String,
    /// エンドポイント URL。
    pub endpoint_url: String,
    /// 有効フラグ（`enabled == 1`）。
    pub enabled: bool,
    /// 認証資格の暗号文が設定済みか（非 NULL かつ非空）。
    pub has_auth: bool,
    /// tools_cache（JSON 配列文字列・`None` 可）。
    pub tools_cache: Option<String>,
}

/// 認証情報 1 件の一覧ビュー（Node `CredentialIndexEntry`・秘密列は含まない）。
#[derive(Debug, Clone)]
pub struct CredentialRow {
    /// サービス名（正規化済み）。
    pub service_name: String,
    /// ユーザー名。
    pub username: String,
    /// URL（`None` 可）。
    pub url: Option<String>,
    /// 更新時刻。
    pub updated_at: Option<String>,
}

/// ケーパビリティ JSON をパースする（Node `parseCapabilities`）。`null`/非配列は秘書相当のフル集合へ
/// フォールバックし、既知ケーパビリティのみを残す（順序は入力順を保つ・重複除去はしない＝
/// 集合判定 `contains` にのみ使うため十分）。
#[must_use]
pub fn parse_capabilities(raw: Option<&str>) -> Vec<String> {
    let parsed: Option<Value> = raw
        .filter(|s| !s.is_empty())
        .and_then(|s| serde_json::from_str(s).ok());
    match parsed {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str())
            .filter(|c| KNOWN_CAPABILITIES.contains(c))
            .map(str::to_owned)
            .collect(),
        // `null`・非配列・パース失敗 → 秘書相当のフル集合。
        _ => KNOWN_CAPABILITIES.iter().map(|s| (*s).to_owned()).collect(),
    }
}

/// `tools_cache`（JSON 配列文字列）の要素数（Node `parseToolsCache(s).length`）。非配列/壊れ/NULL は 0。
#[must_use]
pub fn parse_tools_len(raw: Option<&str>) -> usize {
    raw.filter(|s| !s.is_empty())
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| v.as_array().map(Vec::len))
        .unwrap_or(0)
}

/// ケーパビリティからプリセット ID を逆引きする（Node `presetIdForCapabilities`）。
#[must_use]
pub fn preset_id_for(caps: &[String]) -> &'static str {
    if caps.iter().any(|c| c == "secretary") {
        "secretary"
    } else {
        "mcp_assistant"
    }
}

/// 汎用モード（ギルド常駐）Bot か（Node `isGuildAssistantBot`＝`secretary` を持たない）。
#[must_use]
pub fn is_guild_assistant(caps: &[String]) -> bool {
    !caps.iter().any(|c| c == "secretary")
}

/// Bot 1 件を引く（Node `getBotById`）。不在は `None`。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_bot(db: &Db, bot_id: &str) -> Result<Option<BotRow>, DbError> {
    let bot_id = bot_id.to_owned();
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT id, user_id, name, suspended, stopped, discord_token_encrypted, \
                 discord_username, discord_avatar_url, capabilities FROM bots WHERE id = ?1",
                params![bot_id],
                map_bot_row,
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// オーナー所有 Bot 一覧（Node `listBotsOwnedBy`・`created_at ASC`）。共有・system_default 由来は含めない。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_bots_owned_by(db: &Db, user_id: &str) -> Result<Vec<BotRow>, DbError> {
    let user_id = user_id.to_owned();
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, user_id, name, suspended, stopped, discord_token_encrypted, \
                     discord_username, discord_avatar_url, capabilities FROM bots \
                     WHERE user_id = ?1 ORDER BY created_at ASC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![user_id], map_bot_row)
                .map_err(map_sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(map_sqlite)?);
            }
            Ok(out)
        })
        .await
}

fn map_bot_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<BotRow> {
    let token: Option<String> = r.get(5)?;
    Ok(BotRow {
        id: r.get(0)?,
        user_id: r.get(1)?,
        name: r.get(2)?,
        suspended: r.get::<_, i64>(3)? == 1,
        stopped: r.get::<_, i64>(4)? == 1,
        has_token: token.is_some_and(|s| !s.is_empty()),
        discord_username: r.get(6)?,
        discord_avatar_url: r.get(7)?,
        capabilities: r.get(8)?,
    })
}

/// ユーザーが指定 Bot へアクセス可能か（Node `hasBotAccess`）。`system_default` は常に `true`。
/// それ以外は所有 or `bot_shares` の active 共有を持つ場合に `true`。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn has_bot_access(db: &Db, user_id: &str, bot_id: &str) -> Result<bool, DbError> {
    if bot_id == "system_default" {
        return Ok(true);
    }
    let user_id = user_id.to_owned();
    let bot_id = bot_id.to_owned();
    db.read
        .read(move |conn| {
            let row: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM bots b \
                     LEFT JOIN bot_shares s ON s.bot_id = b.id AND s.shared_user_id = ?1 AND s.status = 'active' \
                     WHERE b.id = ?2 AND (b.user_id = ?1 OR s.id IS NOT NULL)",
                    params![user_id, bot_id],
                    |r| r.get::<_, i64>(0),
                )
                .optional()
                .map_err(map_sqlite)?;
            Ok(row.is_some())
        })
        .await
}

/// Bot が管理者停止処分中か（Node `isBotSuspended`）。不在は `false`。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn is_bot_suspended(db: &Db, bot_id: &str) -> Result<bool, DbError> {
    let bot_id = bot_id.to_owned();
    db.read
        .read(move |conn| {
            let s: Option<i64> = conn
                .query_row(
                    "SELECT suspended FROM bots WHERE id = ?1",
                    params![bot_id],
                    |r| r.get::<_, i64>(0),
                )
                .optional()
                .map_err(map_sqlite)?;
            Ok(s == Some(1))
        })
        .await
}

/// オーナーの手動停止希望フラグを永続化する（Node `setBotStopped`）。変更行があれば `true`。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn set_bot_stopped(db: &Db, bot_id: &str, stopped: bool) -> Result<bool, DbError> {
    let bot_id = bot_id.to_owned();
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute(
                    "UPDATE bots SET stopped = ?1, updated_at = datetime('now', 'localtime') \
                     WHERE id = ?2",
                    params![i64::from(stopped), bot_id],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

/// Bot が許可された MCP サーバー ID 一覧（owner 本人付与分のみ・Node `listServerIdsForBot(botId, ownerId)`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_server_ids_for_bot(
    db: &Db,
    bot_id: &str,
    owner_id: &str,
) -> Result<Vec<i64>, DbError> {
    let bot_id = bot_id.to_owned();
    let owner_id = owner_id.to_owned();
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT mcp_server_id FROM bot_mcp_access WHERE bot_id = ?1 AND owner_id = ?2",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![bot_id, owner_id], |r| r.get::<_, i64>(0))
                .map_err(map_sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(map_sqlite)?);
            }
            Ok(out)
        })
        .await
}

/// オーナー本人が登録した MCP サーバー一覧（許可付与対象・Node `listServersForOwner`・`created_at ASC`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_servers_for_owner(db: &Db, user_id: &str) -> Result<Vec<McpServerRow>, DbError> {
    let user_id = user_id.to_owned();
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, name, endpoint_url, enabled, auth_credential_encrypted, tools_cache \
                     FROM mcp_servers WHERE user_id = ?1 ORDER BY created_at ASC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![user_id], |r| {
                    let auth: Option<String> = r.get(4)?;
                    Ok(McpServerRow {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        endpoint_url: r.get(2)?,
                        enabled: r.get::<_, i64>(3)? == 1,
                        has_auth: auth.is_some_and(|s| !s.is_empty()),
                        tools_cache: r.get(5)?,
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

/// MCP サーバー 1 件の所有者を引く（越権チェック用・Node `getServerById` の `user_id` のみ）。不在は `None`。
/// 返り値 `Some(None)` はシステムレベル登録（`user_id IS NULL`）を表す。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_server_owner(
    db: &Db,
    server_id: i64,
) -> Result<Option<Option<String>>, DbError> {
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT user_id FROM mcp_servers WHERE id = ?1",
                params![server_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// Bot に MCP サーバー利用を許可する（冪等・Node `grantMcpToBot`）。`owner_id` は付与した呼び出し元。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn grant_mcp_to_bot(
    db: &Db,
    bot_id: &str,
    owner_id: &str,
    server_id: i64,
) -> Result<(), DbError> {
    let bot_id = bot_id.to_owned();
    let owner_id = owner_id.to_owned();
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT OR IGNORE INTO bot_mcp_access (bot_id, owner_id, mcp_server_id) \
                 VALUES (?1, ?2, ?3)",
                params![bot_id, owner_id, server_id],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// Bot から MCP サーバー利用許可を取り消す（Node `revokeMcpFromBot`）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn revoke_mcp_from_bot(
    db: &Db,
    bot_id: &str,
    owner_id: &str,
    server_id: i64,
) -> Result<(), DbError> {
    let bot_id = bot_id.to_owned();
    let owner_id = owner_id.to_owned();
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "DELETE FROM bot_mcp_access WHERE bot_id = ?1 AND owner_id = ?2 AND mcp_server_id = ?3",
                params![bot_id, owner_id, server_id],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// Bot が利用を許可された認証情報名一覧（owner 本人付与分・Node `listCredentialNamesForBot`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_credential_names_for_bot(
    db: &Db,
    bot_id: &str,
    owner_id: &str,
) -> Result<Vec<String>, DbError> {
    let bot_id = bot_id.to_owned();
    let owner_id = owner_id.to_owned();
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT service_name FROM bot_credential_access WHERE bot_id = ?1 AND owner_id = ?2",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![bot_id, owner_id], |r| r.get::<_, String>(0))
                .map_err(map_sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(map_sqlite)?);
            }
            Ok(out)
        })
        .await
}

/// 指定サービスの認証情報が存在するか（Node の grant 側 `getDecryptedCredential` 存在判定に相当・復号不要
/// のため `credentials` 表を直接引く。`service_name` は呼び出し側で正規化済み）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn credential_exists(
    db: &Db,
    user_id: &str,
    service_name: &str,
) -> Result<bool, DbError> {
    let user_id = user_id.to_owned();
    let service_name = service_name.to_owned();
    db.read
        .read(move |conn| {
            let row: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM credentials WHERE user_id = ?1 AND service_name = ?2 LIMIT 1",
                    params![user_id, service_name],
                    |r| r.get::<_, i64>(0),
                )
                .optional()
                .map_err(map_sqlite)?;
            Ok(row.is_some())
        })
        .await
}

/// Bot に認証情報利用を許可する（冪等・Node `grantCredentialToBot`）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn grant_credential_to_bot(
    db: &Db,
    bot_id: &str,
    owner_id: &str,
    service_name: &str,
) -> Result<(), DbError> {
    let bot_id = bot_id.to_owned();
    let owner_id = owner_id.to_owned();
    let service_name = service_name.to_owned();
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT OR IGNORE INTO bot_credential_access (bot_id, owner_id, service_name) \
                 VALUES (?1, ?2, ?3)",
                params![bot_id, owner_id, service_name],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// Bot から認証情報利用許可を取り消す（Node `revokeCredentialFromBot`）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn revoke_credential_from_bot(
    db: &Db,
    bot_id: &str,
    owner_id: &str,
    service_name: &str,
) -> Result<(), DbError> {
    let bot_id = bot_id.to_owned();
    let owner_id = owner_id.to_owned();
    let service_name = service_name.to_owned();
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "DELETE FROM bot_credential_access WHERE bot_id = ?1 AND owner_id = ?2 AND service_name = ?3",
                params![bot_id, owner_id, service_name],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// オーナー本人が登録した認証情報の一覧（秘密列を含まない・Node `secretService.listCredentialServices`＝
/// `credentialRepo.listCredentials`・`service_name ASC`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_credential_services(
    db: &Db,
    user_id: &str,
) -> Result<Vec<CredentialRow>, DbError> {
    let user_id = user_id.to_owned();
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT service_name, username, url, updated_at FROM credentials \
                     WHERE user_id = ?1 ORDER BY service_name ASC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![user_id], |r| {
                    Ok(CredentialRow {
                        service_name: r.get(0)?,
                        username: r.get(1)?,
                        url: r.get(2)?,
                        updated_at: r.get(3)?,
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

/// 秘書コンテキストのリセット境界キー（Node `contextFloorKey`）。
#[must_use]
pub fn context_floor_key(user_id: &str, bot_id: &str) -> String {
    format!("context_floor:{bot_id}:{user_id}")
}

/// owner DM（汎用モード）のリセット境界キー（Node `botDmContextFloorKey`）。
#[must_use]
pub fn bot_dm_context_floor_key(user_id: &str, bot_id: &str) -> String {
    format!("context_floor:{bot_id}:dm:{user_id}")
}

/// 会話コンテキストをリセットする（Node `clearContext` / `clearBotDmContext` の SQLite 部・Redis 非依存）。
/// 永続ログは消さず floor を現在の最大 id に進めることで、以降の再構築で過去メッセージを復元しない。
/// `floor_key` は秘書 or DM のどちらかを呼び出し側が渡す（yuuka-orchestrator `clear_context` と同一挙動）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn clear_context_floor(
    db: &Db,
    user_id: &str,
    bot_id: &str,
    floor_key: String,
) -> Result<(), DbError> {
    let user_id = user_id.to_owned();
    let bot_id = bot_id.to_owned();
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

/// JS `Number(x)` + `Number.isInteger` 相当のコアース（整数のみ受理・それ以外は `None`）。
///
/// Node の各ルートは `Number(ctx.body.x)` → `Number.isInteger(...)` で判定するため、整数以外
/// （小数・NaN・非数）はガードを外れて 403/undefined 分岐へ落ちる。ここでは整数値のときだけ `Some(i64)`。
#[must_use]
pub fn js_int(value: &Value) -> Option<i64> {
    match value {
        // number: 整数値のみ受理（`Number.isInteger`）。i64 に収まらない大整数は範囲外＝非受理。
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(i)
            } else if let Some(f) = n.as_f64() {
                (f.fract() == 0.0 && f.is_finite() && f >= i64::MIN as f64 && f <= i64::MAX as f64)
                    .then_some(f as i64)
            } else {
                None
            }
        }
        // string: JS `Number("  12 ")` は trim して数値化。空文字は 0（整数）。整数表現のみ受理。
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                return Some(0);
            }
            if let Ok(i) = t.parse::<i64>() {
                return Some(i);
            }
            // "12.0" 等は f64 経由で整数判定（`Number("12.0")` = 12・`Number.isInteger` = true）。
            t.parse::<f64>().ok().and_then(|f| {
                (f.fract() == 0.0 && f.is_finite() && f >= i64::MIN as f64 && f <= i64::MAX as f64)
                    .then_some(f as i64)
            })
        }
        // Node `Number(true)` = 1 は整数だが本ルート群の accountId/serverId には来ない。JS `Number(null)`
        // = 0（整数）だが本移植では未指定は非受理側に倒す（bool/null/array/object は非整数扱い）。
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use rusqlite::{Connection, OptionalExtension};
    use serde_json::{json, Value};

    use super::{
        bot_dm_context_floor_key, clear_context_floor, context_floor_key, credential_exists,
        grant_credential_to_bot, grant_mcp_to_bot, is_guild_assistant, js_int,
        list_credential_names_for_bot, list_server_ids_for_bot, parse_capabilities, preset_id_for,
        revoke_credential_from_bot, revoke_mcp_from_bot, set_bot_stopped,
    };
    use yuuka_web::Db;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn fresh_db() -> (Db, std::path::PathBuf) {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_integrated_repo_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        drop(Connection::open(&path).expect("create db file"));
        let db = Db::open(&path).expect("open db");
        (db, path)
    }

    fn raw(path: &std::path::Path) -> Connection {
        Connection::open(path).expect("raw conn")
    }

    fn seed_owner_bot(path: &std::path::Path, user: &str, bot: &str) {
        let conn = raw(path);
        conn.execute(
            "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt) \
             VALUES (?1, ?1, 'h', 's')",
            rusqlite::params![user],
        )
        .expect("seed user");
        conn.execute(
            "INSERT INTO bots (id, user_id, name) VALUES (?1, ?2, ?1)",
            rusqlite::params![bot, user],
        )
        .expect("seed bot");
    }

    #[test]
    fn capabilities_parse_and_preset() {
        // null/空 → 秘書相当フル集合。
        let full = parse_capabilities(None);
        assert!(full.iter().any(|c| c == "secretary"));
        assert_eq!(preset_id_for(&full), "secretary");
        assert!(!is_guild_assistant(&full));

        // 非配列（オブジェクト）→ フル集合フォールバック。
        assert!(parse_capabilities(Some("{\"a\":1}"))
            .iter()
            .any(|c| c == "secretary"));

        // secretary 無し配列 → 汎用モード・未知値は捨てる。
        let caps = parse_capabilities(Some("[\"persona\",\"mcp\",\"bogus\"]"));
        assert_eq!(caps, vec!["persona".to_owned(), "mcp".to_owned()]);
        assert_eq!(preset_id_for(&caps), "mcp_assistant");
        assert!(is_guild_assistant(&caps));

        // 壊れた JSON → フル集合フォールバック。
        assert!(parse_capabilities(Some("["))
            .iter()
            .any(|c| c == "secretary"));
    }

    #[test]
    fn js_int_matches_number_is_integer() {
        assert_eq!(js_int(&json!(5)), Some(5));
        assert_eq!(js_int(&json!(-3)), Some(-3));
        assert_eq!(js_int(&json!("12")), Some(12));
        assert_eq!(js_int(&json!("  7 ")), Some(7));
        assert_eq!(js_int(&json!("")), Some(0)); // Number("") = 0（整数）
        assert_eq!(js_int(&json!("12.0")), Some(12));
        // 小数・非数・bool・null・配列は非整数（ガード外れ）。
        assert_eq!(js_int(&json!(1.5)), None);
        assert_eq!(js_int(&json!("abc")), None);
        assert_eq!(js_int(&json!(true)), None);
        assert_eq!(js_int(&Value::Null), None);
        assert_eq!(js_int(&json!([1])), None);
    }

    #[tokio::test]
    async fn set_stopped_roundtrip() {
        let (db, path) = fresh_db();
        seed_owner_bot(&path, "owner", "b1");
        assert!(set_bot_stopped(&db, "b1", true).await.unwrap());
        let stopped: i64 = raw(&path)
            .query_row("SELECT stopped FROM bots WHERE id = 'b1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stopped, 1);
        assert!(set_bot_stopped(&db, "b1", false).await.unwrap());
        // 不在 Bot は false。
        assert!(!set_bot_stopped(&db, "ghost", true).await.unwrap());
    }

    #[tokio::test]
    async fn mcp_grant_revoke_roundtrip() {
        let (db, path) = fresh_db();
        seed_owner_bot(&path, "owner", "b1");
        // MCP サーバーを直挿し（owner 所有）。
        raw(&path)
            .execute(
                "INSERT INTO mcp_servers (id, user_id, name, endpoint_url) \
                 VALUES (7, 'owner', 'srv', 'https://ex.test')",
                [],
            )
            .unwrap();
        grant_mcp_to_bot(&db, "b1", "owner", 7).await.unwrap();
        // 冪等（二重付与でも 1 件）。
        grant_mcp_to_bot(&db, "b1", "owner", 7).await.unwrap();
        assert_eq!(
            list_server_ids_for_bot(&db, "b1", "owner").await.unwrap(),
            vec![7]
        );
        // 別 owner の許可は混ざらない。
        assert!(list_server_ids_for_bot(&db, "b1", "other")
            .await
            .unwrap()
            .is_empty());
        revoke_mcp_from_bot(&db, "b1", "owner", 7).await.unwrap();
        assert!(list_server_ids_for_bot(&db, "b1", "owner")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn credential_grant_revoke_and_exists() {
        let (db, path) = fresh_db();
        seed_owner_bot(&path, "owner", "b1");
        raw(&path)
            .execute(
                "INSERT INTO credentials (user_id, service_name, username, encrypted_password, iv, auth_tag) \
                 VALUES ('owner', 'github', 'alice', 'E', 'I', 'T')",
                [],
            )
            .unwrap();
        assert!(credential_exists(&db, "owner", "github").await.unwrap());
        assert!(!credential_exists(&db, "owner", "missing").await.unwrap());

        grant_credential_to_bot(&db, "b1", "owner", "github")
            .await
            .unwrap();
        grant_credential_to_bot(&db, "b1", "owner", "github")
            .await
            .unwrap(); // 冪等
        assert_eq!(
            list_credential_names_for_bot(&db, "b1", "owner")
                .await
                .unwrap(),
            vec!["github".to_owned()]
        );
        revoke_credential_from_bot(&db, "b1", "owner", "github")
            .await
            .unwrap();
        assert!(list_credential_names_for_bot(&db, "b1", "owner")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn clear_history_advances_floor() {
        let (db, path) = fresh_db();
        seed_owner_bot(&path, "owner", "b1");
        // message_logs へ 2 件。最大 id を floor に進める。
        let conn = raw(&path);
        conn.execute(
            "INSERT INTO message_logs (user_id, bot_id, role, content) VALUES ('owner','b1','user','a')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message_logs (user_id, bot_id, role, content) VALUES ('owner','b1','assistant','b')",
            [],
        )
        .unwrap();
        let max_id: i64 = conn
            .query_row(
                "SELECT MAX(id) FROM message_logs WHERE user_id='owner' AND bot_id='b1'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        let key = context_floor_key("owner", "b1");
        clear_context_floor(&db, "owner", "b1", key.clone())
            .await
            .unwrap();
        let stored: String = raw(&path)
            .query_row(
                "SELECT value FROM system_settings WHERE key = ?1",
                rusqlite::params![key],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, max_id.to_string());

        // DM キーは別レコード。
        let dm_key = bot_dm_context_floor_key("owner", "b1");
        assert_ne!(dm_key, context_floor_key("owner", "b1"));
        clear_context_floor(&db, "owner", "b1", dm_key.clone())
            .await
            .unwrap();
        let dm_stored: String = raw(&path)
            .query_row(
                "SELECT value FROM system_settings WHERE key = ?1",
                rusqlite::params![dm_key],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dm_stored, max_id.to_string());

        // ログ無し Bot は floor を書かない（no-op）。
        seed_owner_bot(&path, "owner", "b2");
        let empty_key = context_floor_key("owner", "b2");
        clear_context_floor(&db, "owner", "b2", empty_key.clone())
            .await
            .unwrap();
        let none: Option<String> = raw(&path)
            .query_row(
                "SELECT value FROM system_settings WHERE key = ?1",
                rusqlite::params![empty_key],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        assert_eq!(none, None);
    }
}
