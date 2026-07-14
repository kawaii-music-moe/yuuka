//! Bot メタデータ・メンバー制・ノート・Bot 専用鍵の DB アクセス（Node `db/botRepo`・
//! `db/botAttributesRepo`・`db/botMemberRequestRepo`・`db/personaRepo`・`db/userRepo` パリティ）。
//!
//! `yuuka-discord` の注入ポート（[`yuuka_discord::BotDirectory`]/[`yuuka_discord::MembershipService`]）を
//! [`crate::discord_ports`] が実装する際の生 SQL 層。読み取りは read pool、書き込み・読み書き混在（申請の
//! 承認等）は writer actor 上で単一 Tx として実行する（R-1: 第二 writer 経路を作らない）。

use rusqlite::{params, OptionalExtension};
use yuuka_core::DbError;
use yuuka_core::{BotId, CapabilitySet, UserId};
use yuuka_db::map_sqlite;
use yuuka_discord::{BotRecord, MemberDecision, PersonaRecord, ShareRecord};
use yuuka_web::Db;

/// Bot 専用モデル（Node `BOT_DEFAULT_MODEL`・汎用モードは常にこれ）。
pub const BOT_DEFAULT_MODEL: &str = "gemini-3.1-flash-lite";

/// 既知の能力（Node `KNOWN_CAPABILITIES`）。JSON 内の未知エントリは除外する。`core` は暗黙付与のため含めない。
const KNOWN_CAPABILITIES: [&str; 4] = ["persona", "memory", "mcp", "secretary"];

/// 秘書相当のフル能力（Node `BOT_PRESETS.secretary.capabilities`）。DB 未登録 Bot・不正 JSON のフォールバック。
#[must_use]
pub fn secretary_full_capabilities() -> CapabilitySet {
    CapabilitySet::from_granted(KNOWN_CAPABILITIES.iter().map(|c| (*c).to_owned()).collect())
}

/// capabilities JSON を能力集合へ解決する（Node `parseCapabilities`）。null/空/非配列/パース失敗は
/// 秘書相当フルセットへフォールバックし、有効配列は既知能力のみ保持する（未知は破棄）。
#[must_use]
pub fn parse_capabilities(raw: &str) -> CapabilitySet {
    // Node `JSON.parse(json || DEFAULT)`: null/空文字は DEFAULT（秘書相当）へ。
    if raw.trim().is_empty() {
        return secretary_full_capabilities();
    }
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(serde_json::Value::Array(arr)) => {
            let granted: Vec<String> = arr
                .iter()
                .filter_map(serde_json::Value::as_str)
                .filter(|c| KNOWN_CAPABILITIES.contains(c))
                .map(str::to_owned)
                .collect();
            CapabilitySet::from_granted(granted)
        }
        // 非配列 or パース失敗は秘書相当（Node parseCapabilities の catch / 非 Array フォールスルー）。
        _ => secretary_full_capabilities(),
    }
}

/// `bots` 1 行の必要フィールド射影（Node `BotRecord` の一部）。
#[derive(Debug, Clone)]
pub struct BotRow {
    pub id: String,
    pub owner_id: String,
    pub name: String,
    pub suspended: bool,
    pub stopped: bool,
    pub recommended_persona_id: Option<i64>,
    pub persona_id: Option<i64>,
    pub capabilities: String,
    pub has_gemini_key: bool,
}

impl BotRow {
    /// この Bot の能力集合（Node `resolveBotCapabilities` の bot 存在時経路 = `parseCapabilities`）。
    #[must_use]
    pub fn capability_set(&self) -> CapabilitySet {
        parse_capabilities(&self.capabilities)
    }

    /// 汎用モード（ギルド常駐アシスタント）か（Node `isGuildAssistantBot = !capabilities.includes("secretary")`）。
    #[must_use]
    pub fn is_guild_assistant(&self) -> bool {
        match serde_json::from_str::<Vec<String>>(&self.capabilities) {
            Ok(caps) => !caps.iter().any(|c| c == "secretary"),
            // パース不能は安全側（秘書扱い＝汎用モードに落とさない）。
            Err(_) => false,
        }
    }

    /// provider 中立 [`BotRecord`] へ写像する。
    #[must_use]
    pub fn to_record(&self) -> BotRecord {
        BotRecord {
            id: BotId::new(self.id.clone()),
            owner_id: UserId::new(self.owner_id.clone()),
            name: self.name.clone(),
            suspended: self.suspended,
            stopped: self.stopped,
            recommended_persona_id: self.recommended_persona_id,
            is_guild_assistant: self.is_guild_assistant(),
            has_gemini_key: self.has_gemini_key,
        }
    }
}

/// SELECT 句（`BotRow` の列順に一致・全 select で共有）。
const BOT_SELECT: &str = "SELECT id, user_id, name, suspended, stopped, recommended_persona_id, \
     persona_id, capabilities, \
     (gemini_api_key_encrypted IS NOT NULL AND gemini_api_key_encrypted <> '') AS has_key \
     FROM bots";

/// `bots` 1 行を [`BotRow`] へ読む（`query_row`/`query_map` 共通）。
fn map_bot_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<BotRow> {
    Ok(BotRow {
        id: r.get(0)?,
        owner_id: r.get(1)?,
        name: r.get(2)?,
        suspended: r.get::<_, i64>(3)? != 0,
        stopped: r.get::<_, i64>(4)? != 0,
        recommended_persona_id: r.get(5)?,
        persona_id: r.get(6)?,
        // NULL 耐性: 手編集/レガシー DB で capabilities が NULL でもクエリを落とさず空文字へ。
        // parse_capabilities が空文字を秘書相当へ畳む（Node `parseCapabilities(null)` パリティ）。
        capabilities: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
        has_gemini_key: r.get::<_, i64>(8)? != 0,
    })
}

/// Bot を 1 件引く（Node `getBotById`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_bot(db: &Db, bot_id: &str) -> Result<Option<BotRow>, DbError> {
    let bot_id = bot_id.to_owned();
    db.read
        .read(move |conn| {
            conn.query_row(
                &format!("{BOT_SELECT} WHERE id = ?1"),
                params![bot_id],
                map_bot_row,
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// 全 Bot（起動時の一括起動・Node `listAllBots`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_all_bots(db: &Db) -> Result<Vec<BotRow>, DbError> {
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(&format!("{BOT_SELECT} ORDER BY created_at ASC"))
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map([], map_bot_row)
                .map_err(map_sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite)?;
            Ok(rows)
        })
        .await
}

/// ユーザーがアクセス可能な Bot ID（オーナー + system_default + 共有 active・Node `listBotsForUser`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_bot_ids_for_user(db: &Db, user_id: &str) -> Result<Vec<String>, DbError> {
    let user_id = user_id.to_owned();
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT DISTINCT b.id FROM bots b \
                     LEFT JOIN bot_shares s ON s.bot_id = b.id AND s.shared_user_id = ?1 AND s.status = 'active' \
                     WHERE b.user_id = ?1 OR b.id = 'system_default' OR s.id IS NOT NULL \
                     ORDER BY b.created_at ASC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![user_id], |r| r.get::<_, String>(0))
                .map_err(map_sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite)?;
            Ok(rows)
        })
        .await
}

/// Bot 管理ビュー用の詳細行（Node `BotRecord` の Web 応答に必要な列）。
///
/// `token_enc`（暗号 3 列）は application_id 導出（トークン先頭セグメントの base64url）にのみ使い、
/// **応答 JSON には決して出さない**（view 層で復号→app-id を出すだけ・§6.4 機密フェイルクローズ）。
#[derive(Debug, Clone)]
pub struct BotDetail {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub recommended_persona_id: Option<i64>,
    pub persona_id: Option<i64>,
    pub capabilities: String,
    pub discord_username: Option<String>,
    pub discord_avatar_url: Option<String>,
    pub discord_application_id: Option<String>,
    pub suspended: bool,
    pub created_at: String,
    pub updated_at: String,
    pub has_token: bool,
    pub has_gemini_key: bool,
    /// 復号用の Discord トークン暗号 3 列（app-id 導出専用・非公開）。
    pub token_enc: Option<EncryptedTriplet>,
}

/// `BotDetail` の SELECT 列（列順は [`map_bot_detail`] と一致）。
const BOT_DETAIL_SELECT: &str = "SELECT id, user_id, name, recommended_persona_id, persona_id, \
     capabilities, discord_username, discord_avatar_url, discord_application_id, suspended, \
     created_at, updated_at, discord_token_encrypted, discord_token_iv, discord_token_tag, \
     gemini_api_key_encrypted FROM bots";

fn map_bot_detail(r: &rusqlite::Row<'_>) -> rusqlite::Result<BotDetail> {
    let token_encrypted: Option<String> = r.get(12)?;
    let token_iv: Option<String> = r.get(13)?;
    let token_tag: Option<String> = r.get(14)?;
    let gemini_encrypted: Option<String> = r.get(15)?;
    let has_token = token_encrypted.as_deref().is_some_and(|s| !s.is_empty());
    let has_gemini_key = gemini_encrypted.as_deref().is_some_and(|s| !s.is_empty());
    let token_enc = match (token_encrypted, token_iv, token_tag) {
        (Some(encrypted), Some(iv), Some(tag)) if !encrypted.is_empty() => {
            Some(EncryptedTriplet { encrypted, iv, tag })
        }
        _ => None,
    };
    Ok(BotDetail {
        id: r.get(0)?,
        user_id: r.get(1)?,
        name: r.get(2)?,
        recommended_persona_id: r.get(3)?,
        persona_id: r.get(4)?,
        capabilities: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
        discord_username: r.get(6)?,
        discord_avatar_url: r.get(7)?,
        discord_application_id: r.get(8)?,
        suspended: r.get::<_, i64>(9)? != 0,
        created_at: r.get(10)?,
        updated_at: r.get(11)?,
        has_token,
        has_gemini_key,
        token_enc,
    })
}

/// 詳細行を 1 件引く（作成直後の返却・Node `getBotById` の Web 応答用）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_bot_detail(db: &Db, bot_id: &str) -> Result<Option<BotDetail>, DbError> {
    let bot_id = bot_id.to_owned();
    db.read
        .read(move |conn| {
            conn.query_row(
                &format!("{BOT_DETAIL_SELECT} WHERE id = ?1"),
                params![bot_id],
                map_bot_detail,
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// ユーザーがアクセス可能な Bot の詳細一覧（owner + system_default + 共有 active・Node `listBotsForUser`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_bots_for_user(db: &Db, user_id: &str) -> Result<Vec<BotDetail>, DbError> {
    let user_id = user_id.to_owned();
    db.read
        .read(move |conn| {
            // 列順は BOT_DETAIL_SELECT / map_bot_detail と一致（b.* 別名版）。
            let sql = "SELECT DISTINCT b.id, b.user_id, b.name, b.recommended_persona_id, \
                        b.persona_id, b.capabilities, b.discord_username, b.discord_avatar_url, \
                        b.discord_application_id, b.suspended, b.created_at, b.updated_at, \
                        b.discord_token_encrypted, b.discord_token_iv, b.discord_token_tag, \
                        b.gemini_api_key_encrypted \
                 FROM bots b \
                 LEFT JOIN bot_shares s ON s.bot_id = b.id AND s.shared_user_id = ?1 \
                    AND s.status = 'active' \
                 WHERE b.user_id = ?1 OR b.id = 'system_default' OR s.id IS NOT NULL \
                 ORDER BY b.created_at ASC";
            let mut stmt = conn.prepare(sql).map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![user_id], map_bot_detail)
                .map_err(map_sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(map_sqlite)?);
            }
            Ok(out)
        })
        .await
}

/// Bot を作成する（Node `createBot`＝最小 INSERT・残りは既定）。作成後の詳細行を返す。
///
/// # Errors
/// 書き込み・取得失敗時 [`DbError`]。
pub async fn create_bot(
    db: &Db,
    bot_id: &str,
    user_id: &str,
    name: &str,
) -> Result<BotDetail, DbError> {
    let (bid, uid, nm) = (bot_id.to_owned(), user_id.to_owned(), name.to_owned());
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT INTO bots (id, user_id, name) VALUES (?1, ?2, ?3)",
                params![bid, uid, nm],
            )
            .map_err(map_sqlite)?;
            tx.query_row(
                &format!("{BOT_DETAIL_SELECT} WHERE id = ?1"),
                params![bid],
                map_bot_detail,
            )
            .map_err(map_sqlite)
        })
        .await
}

/// Bot を削除する（Node `deleteBot`）。行が消えたら `true`。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn delete_bot(db: &Db, bot_id: &str) -> Result<bool, DbError> {
    let bot_id = bot_id.to_owned();
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute("DELETE FROM bots WHERE id = ?1", params![bot_id])
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

/// Bot プロフィール（名前 + 任意のアバター）を更新する（Node `updateBotProfile`）。行が動けば `true`。
/// `avatar_url` が `None` のときは既存アバターを保持（`COALESCE`）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn update_bot_profile(
    db: &Db,
    bot_id: &str,
    name: &str,
    avatar_url: Option<&str>,
) -> Result<bool, DbError> {
    let (bid, nm, avatar) = (
        bot_id.to_owned(),
        name.to_owned(),
        avatar_url.map(str::to_owned),
    );
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute(
                    "UPDATE bots SET name = ?1, \
                        discord_avatar_url = COALESCE(?2, discord_avatar_url), \
                        updated_at = datetime('now','localtime') WHERE id = ?3",
                    params![nm, avatar, bid],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

/// 暗号化された 3 列（enc/iv/tag）を持つ設定（Discord トークン / Bot 専用 Gemini キー共通）。
#[derive(Debug, Clone)]
pub struct EncryptedTriplet {
    pub encrypted: String,
    pub iv: String,
    pub tag: String,
}

/// nullable な暗号 3 列をまとめて読む（いずれか欠ければ `None`）。
fn read_triplet(row: (Option<String>, Option<String>, Option<String>)) -> Option<EncryptedTriplet> {
    match row {
        (Some(encrypted), Some(iv), Some(tag)) if !encrypted.is_empty() => {
            Some(EncryptedTriplet { encrypted, iv, tag })
        }
        _ => None,
    }
}

/// Bot の Discord トークン暗号 3 列（Node `getBotDiscordConfig`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn bot_discord_token(db: &Db, bot_id: &str) -> Result<Option<EncryptedTriplet>, DbError> {
    let bot_id = bot_id.to_owned();
    db.read
        .read(move |conn| {
            let row = conn
                .query_row(
                    "SELECT discord_token_encrypted, discord_token_iv, discord_token_tag \
                     FROM bots WHERE id = ?1",
                    params![bot_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()
                .map_err(map_sqlite)?;
            Ok(row.and_then(read_triplet))
        })
        .await
}

/// Bot 専用 Gemini キー暗号 3 列（Node `getBotGenAI` の復号対象）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn bot_gemini(db: &Db, bot_id: &str) -> Result<Option<EncryptedTriplet>, DbError> {
    let bot_id = bot_id.to_owned();
    db.read
        .read(move |conn| {
            let row = conn
                .query_row(
                    "SELECT gemini_api_key_encrypted, gemini_api_key_iv, gemini_api_key_tag \
                     FROM bots WHERE id = ?1",
                    params![bot_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()
                .map_err(map_sqlite)?;
            Ok(row.and_then(read_triplet))
        })
        .await
}

/// Bot 専用 Gemini キーの暗号 3 列を更新する（`None` で解除＝NULL・Node `updateBotGeminiKey`）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn update_bot_gemini_key(
    db: &Db,
    bot_id: &str,
    enc: Option<EncryptedTriplet>,
) -> Result<(), DbError> {
    let bot_id = bot_id.to_owned();
    let (e, iv, tag) = match enc {
        Some(t) => (Some(t.encrypted), Some(t.iv), Some(t.tag)),
        None => (None, None, None),
    };
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "UPDATE bots SET gemini_api_key_encrypted = ?1, gemini_api_key_iv = ?2, \
                    gemini_api_key_tag = ?3, updated_at = datetime('now','localtime') \
                 WHERE id = ?4",
                params![e, iv, tag, bot_id],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// Bot 既定の有効モジュール JSON（`bots.enabled_modules`・NULL は `None`＝全有効）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn bot_enabled_modules(db: &Db, bot_id: &str) -> Result<Option<String>, DbError> {
    let bot_id = bot_id.to_owned();
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT enabled_modules FROM bots WHERE id = ?1",
                params![bot_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()
            .map(Option::flatten)
            .map_err(map_sqlite)
        })
        .await
}

/// ユーザー個別の有効モジュール上書き JSON（無ければ `None`・Node `getUserModulesJson`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_user_modules(
    db: &Db,
    bot_id: &str,
    user_id: &str,
) -> Result<Option<String>, DbError> {
    let (b, u) = (bot_id.to_owned(), user_id.to_owned());
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT enabled_modules FROM bot_user_modules WHERE bot_id = ?1 AND user_id = ?2",
                params![b, u],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// ユーザー個別の有効モジュール上書きを保存する（Node `setUserModules`）。
/// `Some(ids)` は JSON で upsert・`None` は行削除（Bot 既定へフォールバック）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn set_user_modules(
    db: &Db,
    bot_id: &str,
    user_id: &str,
    modules: Option<Vec<String>>,
) -> Result<(), DbError> {
    let (b, u) = (bot_id.to_owned(), user_id.to_owned());
    db.writer
        .transaction(move |tx| {
            match modules {
                None => {
                    tx.execute(
                        "DELETE FROM bot_user_modules WHERE bot_id = ?1 AND user_id = ?2",
                        params![b, u],
                    )
                    .map_err(map_sqlite)?;
                }
                Some(ids) => {
                    let json = serde_json::to_string(&ids).unwrap_or_else(|_| "[]".to_owned());
                    tx.execute(
                        "INSERT INTO bot_user_modules (bot_id, user_id, enabled_modules) \
                         VALUES (?1, ?2, ?3) \
                         ON CONFLICT(bot_id, user_id) DO UPDATE SET \
                           enabled_modules = excluded.enabled_modules, \
                           updated_at = datetime('now','localtime')",
                        params![b, u, json],
                    )
                    .map_err(map_sqlite)?;
                }
            }
            Ok(())
        })
        .await
}

/// Discord プロフィール（名前・アバター・application id）を DB へ同期（Node `updateBotDiscordProfile`・
/// `COALESCE` で未指定は据え置き）。書き込みは writer actor 上。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn update_discord_profile(
    db: &Db,
    bot_id: &str,
    username: &str,
    avatar_url: &str,
    application_id: &str,
) -> Result<(), DbError> {
    let (bot_id, username, avatar_url, application_id) = (
        bot_id.to_owned(),
        opt(username),
        opt(avatar_url),
        opt(application_id),
    );
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "UPDATE bots SET discord_username = COALESCE(?1, discord_username), \
                 discord_avatar_url = COALESCE(?2, discord_avatar_url), \
                 discord_application_id = COALESCE(?3, discord_application_id), \
                 updated_at = datetime('now', 'localtime') WHERE id = ?4",
                params![username, avatar_url, application_id, bot_id],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// 空文字を `None` に畳む（`COALESCE` 据え置き用）。
fn opt(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_owned())
}

// ─── メンバー制・許可判定（Node `botAttributesRepo`/`userRepo`） ────────────────

/// Web 登録済みユーザーか（Node `isRegisteredUser`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn is_registered_user(db: &Db, user_id: &str) -> Result<bool, DbError> {
    exists(
        db,
        "SELECT 1 FROM users WHERE discord_id = ?1 LIMIT 1",
        user_id.to_owned(),
    )
    .await
}

/// 管理者ロールか（Node `isAdmin`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn is_admin(db: &Db, user_id: &str) -> Result<bool, DbError> {
    exists(
        db,
        "SELECT 1 FROM users WHERE discord_id = ?1 AND role = 'admin' LIMIT 1",
        user_id.to_owned(),
    )
    .await
}

/// 許可ギルドか（Node `isGuildAllowed`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn is_guild_allowed(db: &Db, bot_id: &str, guild_id: &str) -> Result<bool, DbError> {
    exists2(
        db,
        "SELECT 1 FROM bot_guilds WHERE bot_id = ?1 AND guild_id = ?2",
        bot_id.to_owned(),
        guild_id.to_owned(),
    )
    .await
}

/// 利用メンバーか（Node `isBotMember`。owner は呼び出し側で暗黙メンバー）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn is_bot_member(
    db: &Db,
    bot_id: &str,
    guild_id: &str,
    user_id: &str,
) -> Result<bool, DbError> {
    let (bot_id, guild_id, user_id) = (bot_id.to_owned(), guild_id.to_owned(), user_id.to_owned());
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT 1 FROM bot_members WHERE bot_id = ?1 AND guild_id = ?2 AND user_id = ?3",
                params![bot_id, guild_id, user_id],
                |_| Ok(()),
            )
            .optional()
            .map(|o| o.is_some())
            .map_err(map_sqlite)
        })
        .await
}

/// 保有ロールのいずれかが許可ロールか（Node `isAnyRoleAllowed`。空配列は false）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn is_any_role_allowed(
    db: &Db,
    bot_id: &str,
    guild_id: &str,
    role_ids: &[String],
) -> Result<bool, DbError> {
    if role_ids.is_empty() {
        return Ok(false);
    }
    let (bot_id, guild_id) = (bot_id.to_owned(), guild_id.to_owned());
    let role_ids = role_ids.to_vec();
    db.read
        .read(move |conn| {
            // `role_id IN (?, ?, …)` を bot_id/guild_id の後ろに動的展開する。
            let placeholders = (3..3 + role_ids.len())
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT 1 FROM bot_roles WHERE bot_id = ?1 AND guild_id = ?2 AND role_id IN ({placeholders}) LIMIT 1"
            );
            let mut binds: Vec<&dyn rusqlite::ToSql> = vec![&bot_id, &guild_id];
            for r in &role_ids {
                binds.push(r);
            }
            conn.query_row(&sql, binds.as_slice(), |_| Ok(()))
                .optional()
                .map(|o| o.is_some())
                .map_err(map_sqlite)
        })
        .await
}

// ─── ギルド常駐アシスタントの許可リスト管理（Node `botAttributesRepo`・Web 管理 UI） ──

/// `bot_guilds` 1 行（応答許可ギルド）。
#[derive(Debug, Clone)]
pub struct BotGuildRow {
    pub bot_id: String,
    pub guild_id: String,
    pub created_at: String,
}

/// `bot_members` 1 行（利用メンバー）。
#[derive(Debug, Clone)]
pub struct BotMemberRow {
    pub bot_id: String,
    pub guild_id: String,
    pub user_id: String,
    pub added_by: String,
    pub created_at: String,
}

/// `bot_roles` 1 行（利用可能ロール）。
#[derive(Debug, Clone)]
pub struct BotRoleRow {
    pub bot_id: String,
    pub guild_id: String,
    pub role_id: String,
    pub role_name: Option<String>,
    pub added_by: String,
    pub created_at: String,
}

/// 応答許可ギルドを追加（INSERT OR IGNORE・Node `addAllowedGuild`）。追加できたら `true`。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn add_allowed_guild(db: &Db, bot_id: &str, guild_id: &str) -> Result<bool, DbError> {
    let (b, g) = (bot_id.to_owned(), guild_id.to_owned());
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute(
                    "INSERT OR IGNORE INTO bot_guilds (bot_id, guild_id) VALUES (?1, ?2)",
                    params![b, g],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

/// 応答許可ギルドを削除（Node `removeAllowedGuild`）。削除できたら `true`。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn remove_allowed_guild(db: &Db, bot_id: &str, guild_id: &str) -> Result<bool, DbError> {
    let (b, g) = (bot_id.to_owned(), guild_id.to_owned());
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute(
                    "DELETE FROM bot_guilds WHERE bot_id = ?1 AND guild_id = ?2",
                    params![b, g],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

/// 応答許可ギルド一覧（created_at ASC・Node `listAllowedGuilds`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_allowed_guilds(db: &Db, bot_id: &str) -> Result<Vec<BotGuildRow>, DbError> {
    let b = bot_id.to_owned();
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT bot_id, guild_id, created_at FROM bot_guilds \
                     WHERE bot_id = ?1 ORDER BY created_at ASC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![b], |r| {
                    Ok(BotGuildRow {
                        bot_id: r.get(0)?,
                        guild_id: r.get(1)?,
                        created_at: r.get(2)?,
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

/// 利用メンバーを追加（INSERT OR IGNORE・Node `addBotMember`）。追加できたら `true`。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn add_bot_member(
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

/// 利用メンバーを削除（Node `removeBotMember`）。削除できたら `true`。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn remove_bot_member(
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
                    "DELETE FROM bot_members WHERE bot_id = ?1 AND guild_id = ?2 AND user_id = ?3",
                    params![b, g, u],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

/// 利用メンバー一覧（bot 全体・created_at ASC・Node `listBotMembers(botId)`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_bot_members(db: &Db, bot_id: &str) -> Result<Vec<BotMemberRow>, DbError> {
    let b = bot_id.to_owned();
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT bot_id, guild_id, user_id, added_by, created_at FROM bot_members \
                     WHERE bot_id = ?1 ORDER BY created_at ASC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![b], |r| {
                    Ok(BotMemberRow {
                        bot_id: r.get(0)?,
                        guild_id: r.get(1)?,
                        user_id: r.get(2)?,
                        added_by: r.get(3)?,
                        created_at: r.get(4)?,
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

/// 利用可能ロールを追加（INSERT OR IGNORE・Node `addAllowedRole`）。追加できたら `true`。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn add_allowed_role(
    db: &Db,
    bot_id: &str,
    guild_id: &str,
    role_id: &str,
    added_by: &str,
    role_name: Option<&str>,
) -> Result<bool, DbError> {
    let (b, g, r, by, name) = (
        bot_id.to_owned(),
        guild_id.to_owned(),
        role_id.to_owned(),
        added_by.to_owned(),
        role_name.map(str::to_owned),
    );
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute(
                    "INSERT OR IGNORE INTO bot_roles (bot_id, guild_id, role_id, role_name, added_by) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![b, g, r, name, by],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

/// 利用可能ロールを削除（Node `removeAllowedRole`）。削除できたら `true`。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn remove_allowed_role(
    db: &Db,
    bot_id: &str,
    guild_id: &str,
    role_id: &str,
) -> Result<bool, DbError> {
    let (b, g, r) = (bot_id.to_owned(), guild_id.to_owned(), role_id.to_owned());
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute(
                    "DELETE FROM bot_roles WHERE bot_id = ?1 AND guild_id = ?2 AND role_id = ?3",
                    params![b, g, r],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

/// 利用可能ロール一覧（bot 全体・created_at ASC・Node `listAllowedRoles(botId)`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_allowed_roles(db: &Db, bot_id: &str) -> Result<Vec<BotRoleRow>, DbError> {
    let b = bot_id.to_owned();
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT bot_id, guild_id, role_id, role_name, added_by, created_at \
                     FROM bot_roles WHERE bot_id = ?1 ORDER BY created_at ASC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![b], |r| {
                    Ok(BotRoleRow {
                        bot_id: r.get(0)?,
                        guild_id: r.get(1)?,
                        role_id: r.get(2)?,
                        role_name: r.get(3)?,
                        added_by: r.get(4)?,
                        created_at: r.get(5)?,
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

// ─── ノート（汎用モードのシステムプロンプト注入・Node `botGuildNote`/`botContextNote`） ──

/// ギルド共有ノート本文（無ければ空文字・Node `getBotGuildNote`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn bot_guild_note(db: &Db, bot_id: &str, guild_id: &str) -> Result<String, DbError> {
    let (bot_id, guild_id) = (bot_id.to_owned(), guild_id.to_owned());
    db.read
        .read(move |conn| {
            Ok(conn
                .query_row(
                    "SELECT content FROM bot_guild_notes WHERE bot_id = ?1 AND guild_id = ?2",
                    params![bot_id, guild_id],
                    |r| r.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite)?
                .unwrap_or_default())
        })
        .await
}

/// ギルド共有ノートを upsert する（長さ検証は呼び出し側・Node `setBotGuildNote`）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn set_bot_guild_note(
    db: &Db,
    bot_id: &str,
    guild_id: &str,
    content: &str,
) -> Result<(), DbError> {
    let (bot_id, guild_id, content) = (bot_id.to_owned(), guild_id.to_owned(), content.to_owned());
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT INTO bot_guild_notes (bot_id, guild_id, content, updated_at) \
                 VALUES (?1, ?2, ?3, datetime('now','localtime')) \
                 ON CONFLICT(bot_id, guild_id) DO UPDATE SET \
                   content = excluded.content, updated_at = datetime('now','localtime')",
                params![bot_id, guild_id, content],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// 発話者の個人ノート本文（無ければ空文字・Node `getBotUserNote` = `bot_context_notes`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn bot_user_note(db: &Db, bot_id: &str, user_id: &str) -> Result<String, DbError> {
    let (bot_id, user_id) = (bot_id.to_owned(), user_id.to_owned());
    db.read
        .read(move |conn| {
            Ok(conn
                .query_row(
                    "SELECT content FROM bot_context_notes WHERE bot_id = ?1 AND user_id = ?2",
                    params![bot_id, user_id],
                    |r| r.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite)?
                .unwrap_or_default())
        })
        .await
}

/// ペルソナ prompt を id で引く（汎用モードの Bot 単位ペルソナ・Node `getPersonaById().prompt`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn persona_prompt_by_id(db: &Db, persona_id: i64) -> Result<Option<String>, DbError> {
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT prompt FROM personas WHERE id = ?1",
                params![persona_id],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

// ─── MembershipService の DB 実体（ボタンフロー・Node `memberRequest`/`botRepo`/`personaRepo`） ──

/// 利用申請 submit の結果（`ok=false` は理由文言付き・`ok=true` は owner 宛 DM 用の id を伴う）。
pub struct SubmitResult {
    pub ok: bool,
    pub message: String,
    /// 失敗理由（Web-API の HTTP status 分岐用・Node `SubmitResult.code`）。ok 時は `None`。
    pub code: Option<SubmitDeny>,
    /// 承認 DM の宛先（Bot オーナー・Node `sendMemberRequestDM` の ownerId）。ok 時のみ。
    pub owner_id: Option<String>,
    /// Bot 表示名（DM 本文）。ok 時のみ。
    pub bot_name: Option<String>,
    /// 申請 ID（承認/却下ボタンの custom_id）。ok 時のみ。
    pub request_id: Option<i64>,
}

/// 利用申請 submit の失敗理由（Node `SubmitResult.code`）。`BotNotFound` のみ 404・他は 409。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitDeny {
    BotNotFound,
    IsOwner,
    AlreadyMember,
    AlreadyPending,
}

/// 失敗結果（DM 情報なし）。
fn deny_submit(code: SubmitDeny, message: &str) -> SubmitResult {
    SubmitResult {
        ok: false,
        message: message.to_owned(),
        code: Some(code),
        owner_id: None,
        bot_name: None,
        request_id: None,
    }
}

/// 成功結果（owner 宛 DM 用の id を伴う）。
fn ok_submit(owner_id: String, bot_name: String, request_id: i64) -> SubmitResult {
    SubmitResult {
        ok: true,
        message: String::new(),
        code: None,
        owner_id: Some(owner_id),
        bot_name: Some(bot_name),
        request_id: Some(request_id),
    }
}

/// メンバー外ユーザーの利用申請（Node `submitMemberRequest` + `createMemberRequest`）。
///
/// 本関数は DB のみ確定し、owner 宛の受付 DM 用に `owner_id`/`bot_name`/`request_id` を返す
/// （interaction ハンドラが `sendMemberRequestDM` を送る・Node `memberRequest.ts:64-71` と同分業）。
/// DM 送信失敗でも DB 上の申請は有効で Web 管理画面の「利用申請」から拾える（Node も DM 失敗を許容）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn submit_member_request(
    db: &Db,
    bot_id: &str,
    guild_id: &str,
    applicant: &str,
    note: Option<String>,
) -> Result<SubmitResult, DbError> {
    let (bot_id, guild_id, applicant) =
        (bot_id.to_owned(), guild_id.to_owned(), applicant.to_owned());
    db.writer
        .transaction(move |tx| {
            // Bot 存在 + system_default 除外 + owner 判定。owner/name は承認 DM（Node sendMemberRequestDM）にも使う。
            let bot: Option<(String, String)> = tx
                .query_row(
                    "SELECT user_id, name FROM bots WHERE id = ?1",
                    params![bot_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(map_sqlite)?;
            let Some((owner, bot_name)) = bot.filter(|_| bot_id != "system_default") else {
                return Ok(deny_submit(
                    SubmitDeny::BotNotFound,
                    "対象のBotが見つかりません。",
                ));
            };
            if owner == applicant {
                return Ok(deny_submit(
                    SubmitDeny::IsOwner,
                    "あなたはこのBotのオーナーです（申請は不要です）。",
                ));
            }
            // 既にメンバー。
            let is_member = tx
                .query_row(
                    "SELECT 1 FROM bot_members WHERE bot_id = ?1 AND guild_id = ?2 AND user_id = ?3",
                    params![bot_id, guild_id, applicant],
                    |_| Ok(()),
                )
                .optional()
                .map_err(map_sqlite)?
                .is_some();
            if is_member {
                return Ok(deny_submit(
                    SubmitDeny::AlreadyMember,
                    "あなたは既にこのギルドの利用メンバーです。",
                ));
            }
            // 既存申請（pending は二重申請不可・却下/承認済みは pending へ戻す）。
            let existing: Option<(i64, String)> = tx
                .query_row(
                    "SELECT id, status FROM bot_member_requests \
                     WHERE bot_id = ?1 AND guild_id = ?2 AND user_id = ?3",
                    params![bot_id, guild_id, applicant],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(map_sqlite)?;
            let trimmed_note = note
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(|n| n.chars().take(500).collect::<String>());
            match existing {
                Some((_, status)) if status == "pending" => Ok(deny_submit(
                    SubmitDeny::AlreadyPending,
                    "既に申請済みです。オーナーの承認をお待ちください。",
                )),
                Some((id, _)) => {
                    tx.execute(
                        "UPDATE bot_member_requests SET status = 'pending', note = ?1, \
                         decided_by = NULL, updated_at = datetime('now','localtime') WHERE id = ?2",
                        params![trimmed_note, id],
                    )
                    .map_err(map_sqlite)?;
                    Ok(ok_submit(owner, bot_name, id))
                }
                None => {
                    tx.execute(
                        "INSERT INTO bot_member_requests (bot_id, guild_id, user_id, note) \
                         VALUES (?1, ?2, ?3, ?4)",
                        params![bot_id, guild_id, applicant, trimmed_note],
                    )
                    .map_err(map_sqlite)?;
                    Ok(ok_submit(owner, bot_name, tx.last_insert_rowid()))
                }
            }
        })
        .await
}

/// 承認/却下の結果（Node `decideMemberRequestById` の `{ ok, message, status?, botName? }`）。
pub struct DecideResult {
    pub ok: bool,
    pub message: String,
    /// 失敗理由（Web-API の HTTP status 分岐用・Node `DecisionResult.code`）。ok 時は `None`。
    pub code: Option<DecideDeny>,
    pub status: Option<MemberDecision>,
    pub bot_name: Option<String>,
    /// 申請者（結果 DM の宛先・Node `sendMemberDecisionDM` の applicantId）。ok 時のみ。
    pub applicant_id: Option<String>,
}

/// 承認/却下の失敗理由（Node `DecisionResult.code`）。`NotFound`=404・`Forbidden`=403・`AlreadyDecided`=409。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecideDeny {
    NotFound,
    Forbidden,
    AlreadyDecided,
}

/// オーナー/Admin による申請の承認・却下（Node `decideMemberRequestById` + `decideMemberRequest`）。
///
/// 本関数は DB のみ確定し、申請者宛の結果 DM 用に `applicant_id`（+ `bot_name`/`status`）を返す
/// （interaction ハンドラが `sendMemberDecisionDM` を送る・Node `memberRequest.ts:131-135` と同分業）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn decide_member_request(
    db: &Db,
    request_id: i64,
    decision: MemberDecision,
    actor: &str,
) -> Result<DecideResult, DbError> {
    let actor = actor.to_owned();
    db.writer
        .transaction(move |tx| {
            let req: Option<(String, String, String, String)> = tx
                .query_row(
                    "SELECT bot_id, guild_id, user_id, status FROM bot_member_requests WHERE id = ?1",
                    params![request_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()
                .map_err(map_sqlite)?;
            let Some((bot_id, guild_id, applicant, status)) = req else {
                return Ok(deny_decide(DecideDeny::NotFound, "申請が見つかりません。"));
            };
            // Bot と owner。
            let bot: Option<(String, String)> = tx
                .query_row(
                    "SELECT user_id, name FROM bots WHERE id = ?1",
                    params![bot_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(map_sqlite)?;
            let Some((owner, bot_name)) = bot else {
                return Ok(deny_decide(DecideDeny::NotFound, "Botが見つかりません。"));
            };
            // owner or admin のみ。
            let is_admin = tx
                .query_row(
                    "SELECT 1 FROM users WHERE discord_id = ?1 AND role = 'admin' LIMIT 1",
                    params![actor],
                    |_| Ok(()),
                )
                .optional()
                .map_err(map_sqlite)?
                .is_some();
            if owner != actor && !is_admin {
                return Ok(deny_decide(
                    DecideDeny::Forbidden,
                    "このBotのオーナーのみが承認/却下できます。",
                ));
            }
            if status != "pending" {
                return Ok(deny_decide(
                    DecideDeny::AlreadyDecided,
                    "この申請は既に処理済みです。",
                ));
            }
            let status_str = match decision {
                MemberDecision::Approved => "approved",
                MemberDecision::Rejected => "rejected",
            };
            let moved = tx
                .execute(
                    "UPDATE bot_member_requests SET status = ?1, decided_by = ?2, \
                     updated_at = datetime('now','localtime') WHERE id = ?3 AND status = 'pending'",
                    params![status_str, actor, request_id],
                )
                .map_err(map_sqlite)?;
            if moved == 0 {
                return Ok(deny_decide(
                    DecideDeny::AlreadyDecided,
                    "この申請は既に処理済みです。",
                ));
            }
            if decision == MemberDecision::Approved {
                tx.execute(
                    "INSERT OR IGNORE INTO bot_members (bot_id, guild_id, user_id, added_by) \
                     VALUES (?1, ?2, ?3, ?4)",
                    params![bot_id, guild_id, applicant, actor],
                )
                .map_err(map_sqlite)?;
            }
            Ok(DecideResult {
                ok: true,
                message: String::new(),
                code: None,
                status: Some(decision),
                bot_name: Some(bot_name),
                applicant_id: Some(applicant),
            })
        })
        .await
}

/// 承認/却下の否定結果を組む短縮ヘルパ。
fn deny_decide(code: DecideDeny, message: &str) -> DecideResult {
    DecideResult {
        ok: false,
        message: message.to_owned(),
        code: Some(code),
        status: None,
        bot_name: None,
        applicant_id: None,
    }
}

/// `bot_member_requests` 1 行（Web 管理画面の一覧・Node `BotMemberRequestRecord` 全列）。
#[derive(Debug, Clone)]
pub struct MemberRequestRow {
    pub id: i64,
    pub bot_id: String,
    pub guild_id: String,
    pub user_id: String,
    pub status: String,
    pub note: Option<String>,
    pub decided_by: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

fn row_to_member_request(row: &rusqlite::Row) -> rusqlite::Result<MemberRequestRow> {
    Ok(MemberRequestRow {
        id: row.get("id")?,
        bot_id: row.get("bot_id")?,
        guild_id: row.get("guild_id")?,
        user_id: row.get("user_id")?,
        status: row.get("status")?,
        note: row.get("note")?,
        decided_by: row.get("decided_by")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

/// 申請 1 件を取得する（監査 target 生成用・Node `getMemberRequestById`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_member_request(db: &Db, id: i64) -> Result<Option<MemberRequestRow>, DbError> {
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT * FROM bot_member_requests WHERE id = ?1",
                params![id],
                row_to_member_request,
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// 申請者本人の申請状況一覧（Node `listMemberRequestsByUser`・created_at DESC）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_member_requests_by_user(
    db: &Db,
    user_id: &str,
) -> Result<Vec<MemberRequestRow>, DbError> {
    let uid = user_id.to_owned();
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT * FROM bot_member_requests WHERE user_id = ?1 ORDER BY created_at DESC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![uid], row_to_member_request)
                .map_err(map_sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(map_sqlite)?);
            }
            Ok(out)
        })
        .await
}

/// 指定 Bot 宛の申請一覧（owner 用・`status` 指定で絞り込み・Node `listMemberRequestsForBot`・created_at DESC）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_member_requests_for_bot(
    db: &Db,
    bot_id: &str,
    status: Option<&str>,
) -> Result<Vec<MemberRequestRow>, DbError> {
    let (bid, st) = (bot_id.to_owned(), status.map(str::to_owned));
    db.read
        .read(move |conn| {
            let mut out = Vec::new();
            if let Some(st) = st {
                let mut stmt = conn
                    .prepare(
                        "SELECT * FROM bot_member_requests WHERE bot_id = ?1 AND status = ?2 \
                         ORDER BY created_at DESC",
                    )
                    .map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![bid, st], row_to_member_request)
                    .map_err(map_sqlite)?;
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
            } else {
                let mut stmt = conn
                    .prepare(
                        "SELECT * FROM bot_member_requests WHERE bot_id = ?1 ORDER BY created_at DESC",
                    )
                    .map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![bid], row_to_member_request)
                    .map_err(map_sqlite)?;
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
            }
            Ok(out)
        })
        .await
}

/// ユーザーが所有する Bot の `(id, name)` 一覧（Node `listBotsOwnedBy`・created_at ASC）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_bots_owned_by(db: &Db, user_id: &str) -> Result<Vec<(String, String)>, DbError> {
    let uid = user_id.to_owned();
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare("SELECT id, name FROM bots WHERE user_id = ?1 ORDER BY created_at ASC")
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![uid], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(map_sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(map_sqlite)?);
            }
            Ok(out)
        })
        .await
}

/// 共有招待を引く（Node `getShareById`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_share(db: &Db, share_id: i64) -> Result<Option<ShareRecord>, DbError> {
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT bot_id, shared_user_id, status FROM bot_shares WHERE id = ?1",
                params![share_id],
                |r| {
                    Ok(ShareRecord {
                        bot_id: BotId::new(r.get::<_, String>(0)?),
                        shared_user_id: UserId::new(r.get::<_, String>(1)?),
                        status: r.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// 共有招待を承認（pending→active・Node `acceptShareInvite`）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn accept_share(db: &Db, bot_id: &str, shared_user: &str) -> Result<(), DbError> {
    let (bot_id, shared_user) = (bot_id.to_owned(), shared_user.to_owned());
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "UPDATE bot_shares SET status = 'active', updated_at = datetime('now','localtime') \
                 WHERE bot_id = ?1 AND shared_user_id = ?2 AND status = 'pending'",
                params![bot_id, shared_user],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// 共有招待を辞退・取消（→revoked・Node `revokeShare`）。行が動いたら `true`（Web の `{success}` 用）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn revoke_share(db: &Db, bot_id: &str, shared_user: &str) -> Result<bool, DbError> {
    let (bot_id, shared_user) = (bot_id.to_owned(), shared_user.to_owned());
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute(
                    "UPDATE bot_shares SET status = 'revoked', updated_at = datetime('now','localtime') \
                     WHERE bot_id = ?1 AND shared_user_id = ?2",
                    params![bot_id, shared_user],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

/// `bot_shares` 1 行（Web 共有設定一覧・Node `BotShareRecord` 全列）。
#[derive(Debug, Clone)]
pub struct BotShareRow {
    pub id: i64,
    pub bot_id: String,
    pub owner_id: String,
    pub shared_user_id: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

fn row_to_bot_share(row: &rusqlite::Row) -> rusqlite::Result<BotShareRow> {
    Ok(BotShareRow {
        id: row.get("id")?,
        bot_id: row.get("bot_id")?,
        owner_id: row.get("owner_id")?,
        shared_user_id: row.get("shared_user_id")?,
        status: row.get("status")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

/// 共有招待を作成する（pending・既存は pending へ戻す・Node `createShareInvite`）。作成後の行を返す。
///
/// # Errors
/// 書き込み・取得失敗時 [`DbError`]。
pub async fn create_share_invite(
    db: &Db,
    bot_id: &str,
    owner_id: &str,
    shared_user_id: &str,
) -> Result<BotShareRow, DbError> {
    let (bot_id, owner_id, shared_user_id) = (
        bot_id.to_owned(),
        owner_id.to_owned(),
        shared_user_id.to_owned(),
    );
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT INTO bot_shares (bot_id, owner_id, shared_user_id, status) \
                 VALUES (?1, ?2, ?3, 'pending') \
                 ON CONFLICT(bot_id, shared_user_id) \
                 DO UPDATE SET status = 'pending', updated_at = datetime('now','localtime')",
                params![bot_id, owner_id, shared_user_id],
            )
            .map_err(map_sqlite)?;
            tx.query_row(
                "SELECT * FROM bot_shares WHERE bot_id = ?1 AND shared_user_id = ?2",
                params![bot_id, shared_user_id],
                row_to_bot_share,
            )
            .map_err(map_sqlite)
        })
        .await
}

/// 指定 Bot の共有一覧（Node `listSharesForBot`・created_at ASC）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_shares_for_bot(db: &Db, bot_id: &str) -> Result<Vec<BotShareRow>, DbError> {
    let bot_id = bot_id.to_owned();
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare("SELECT * FROM bot_shares WHERE bot_id = ?1 ORDER BY created_at ASC")
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![bot_id], row_to_bot_share)
                .map_err(map_sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(map_sqlite)?);
            }
            Ok(out)
        })
        .await
}

/// ユーザーの表示名を引く（無ければ `None`・Node `getUserByDiscordId(...)?.username`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_username(db: &Db, discord_id: &str) -> Result<Option<String>, DbError> {
    let uid = discord_id.to_owned();
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT username FROM users WHERE discord_id = ?1",
                params![uid],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// 公開ペルソナ（`is_public=1`）を引く（Node `getPersonaById` フィルタ済み）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_public_persona(
    db: &Db,
    persona_id: i64,
) -> Result<Option<PersonaRecord>, DbError> {
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT id, name FROM personas WHERE id = ?1 AND is_public = 1",
                params![persona_id],
                |r| {
                    Ok(PersonaRecord {
                        id: r.get(0)?,
                        name: r.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// ペルソナの `(owner_id, is_public)` を引く（設定可否判定用・Node `getPersonaById` の必要列）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_persona_owner_public(
    db: &Db,
    persona_id: i64,
) -> Result<Option<(String, bool)>, DbError> {
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT owner_id, is_public FROM personas WHERE id = ?1",
                params![persona_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0)),
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// Bot 単位ペルソナを設定/解除する（Node `setBotPersona`・`persona_id` を更新）。行が動けば `true`。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn set_bot_persona(
    db: &Db,
    bot_id: &str,
    persona_id: Option<i64>,
) -> Result<bool, DbError> {
    let bot_id = bot_id.to_owned();
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute(
                    "UPDATE bots SET persona_id = ?1, updated_at = datetime('now','localtime') \
                     WHERE id = ?2",
                    params![persona_id, bot_id],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

/// 公開ペルソナ（`is_public = 1`）の `name` を返す（推奨ペルソナ設定の可否判定 + 成功文言用）。
/// 非公開／不在は `None`（Node は getPersonaById + `is_public !== 1` を判定・ここは 1 クエリに畳む）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_public_persona_name(
    db: &Db,
    persona_id: i64,
) -> Result<Option<String>, DbError> {
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT name FROM personas WHERE id = ?1 AND is_public = 1",
                params![persona_id],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// Bot の推奨ペルソナ（`recommended_persona_id`）を設定/解除する（Node `setRecommendedPersona`）。
/// `Some(id)` で設定・`None` で解除。行が動けば `true`（`updated_at` も更新・Node と一致）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn set_bot_recommended_persona(
    db: &Db,
    bot_id: &str,
    persona_id: Option<i64>,
) -> Result<bool, DbError> {
    let bot_id = bot_id.to_owned();
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute(
                    "UPDATE bots SET recommended_persona_id = ?1, \
                     updated_at = datetime('now','localtime') WHERE id = ?2",
                    params![persona_id, bot_id],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

/// Admin: 任意のペルソナを**非公開化**する（Node `adminUnpublishPersona`・§5.3.2・owner 非依存）。
/// `is_public = 0` にし、非公開化できたら推奨 Bot からも解除する。行が動けば `true`（不在は `false`）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn admin_unpublish_persona(db: &Db, id: i64) -> Result<bool, DbError> {
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute(
                    "UPDATE personas SET is_public = 0, \
                     updated_at = datetime('now','localtime') WHERE id = ?1",
                    params![id],
                )
                .map_err(map_sqlite)?;
            if n == 0 {
                return Ok(false);
            }
            tx.execute(
                "UPDATE bots SET recommended_persona_id = NULL WHERE recommended_persona_id = ?1",
                params![id],
            )
            .map_err(map_sqlite)?;
            Ok(true)
        })
        .await
}

/// Admin: 任意のペルソナを**削除**する（Node `adminDeletePersona`・§5.3.2・owner 非依存）。
/// 適用中（`bot_active_personas`）は FK `ON DELETE CASCADE` で自動掃除・推奨（`bots.recommended_persona_id`）
/// は FK 無のため明示解除する。行が動けば `true`（不在は `false`）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn admin_delete_persona(db: &Db, id: i64) -> Result<bool, DbError> {
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute("DELETE FROM personas WHERE id = ?1", params![id])
                .map_err(map_sqlite)?;
            if n == 0 {
                return Ok(false);
            }
            tx.execute(
                "UPDATE bots SET recommended_persona_id = NULL WHERE recommended_persona_id = ?1",
                params![id],
            )
            .map_err(map_sqlite)?;
            Ok(true)
        })
        .await
}

/// 公開ペルソナをユーザーのコピーとしてインポート（Node `importPersona`・独立コピー）。成功なら true。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn import_persona(db: &Db, user_id: &str, persona_id: i64) -> Result<bool, DbError> {
    let user_id = user_id.to_owned();
    db.writer
        .transaction(move |tx| {
            let source: Option<(String, String)> = tx
                .query_row(
                    "SELECT name, prompt FROM personas WHERE id = ?1 AND is_public = 1",
                    params![persona_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(map_sqlite)?;
            let Some((name, prompt)) = source else {
                return Ok(false);
            };
            tx.execute(
                "INSERT INTO personas (owner_id, name, prompt) VALUES (?1, ?2, ?3)",
                params![user_id, name, prompt],
            )
            .map_err(map_sqlite)?;
            Ok(true)
        })
        .await
}

// ─── 内部ヘルパ ────────────────────────────────────────────────────────────────

/// 1 引数 EXISTS 判定。
async fn exists(db: &Db, sql: &'static str, arg: String) -> Result<bool, DbError> {
    db.read
        .read(move |conn| {
            conn.query_row(sql, params![arg], |_| Ok(()))
                .optional()
                .map(|o| o.is_some())
                .map_err(map_sqlite)
        })
        .await
}

/// 2 引数 EXISTS 判定。
async fn exists2(db: &Db, sql: &'static str, a: String, b: String) -> Result<bool, DbError> {
    db.read
        .read(move |conn| {
            conn.query_row(sql, params![a, b], |_| Ok(()))
                .optional()
                .map(|o| o.is_some())
                .map_err(map_sqlite)
        })
        .await
}

#[cfg(test)]
mod crud_tests {
    use super::*;

    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_botcrud_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let conn = rusqlite::Connection::open(&path).expect("create empty");
            drop(conn);
        }
        let db = Db::open(&path).expect("open");
        {
            let conn = rusqlite::Connection::open(&path).expect("seed conn");
            for uid in ["owner", "sharee", "stranger"] {
                conn.execute(
                    "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt) \
                     VALUES (?1, ?1, 'x', 'x')",
                    params![uid],
                )
                .expect("seed user");
            }
        }
        db
    }

    #[tokio::test]
    async fn create_update_delete_roundtrip() {
        let db = seed_db();
        let created = create_bot(&db, "b1", "owner", "MyBot").await.unwrap();
        assert_eq!(created.name, "MyBot");
        assert_eq!(created.user_id, "owner");
        assert!(!created.has_token);
        assert!(!created.has_gemini_key);
        assert!(created.token_enc.is_none());
        // 既定 capabilities は秘書相当。
        assert!(created.capabilities.contains("secretary"));

        // プロフィール更新（アバターは None で保持）。
        assert!(update_bot_profile(&db, "b1", "Renamed", None)
            .await
            .unwrap());
        let d = get_bot_detail(&db, "b1").await.unwrap().unwrap();
        assert_eq!(d.name, "Renamed");
        assert!(d.discord_avatar_url.is_none());
        // アバター設定。
        assert!(
            update_bot_profile(&db, "b1", "Renamed", Some("https://cdn/x.png"))
                .await
                .unwrap()
        );
        let d = get_bot_detail(&db, "b1").await.unwrap().unwrap();
        assert_eq!(d.discord_avatar_url.as_deref(), Some("https://cdn/x.png"));

        // 削除。
        assert!(delete_bot(&db, "b1").await.unwrap());
        assert!(get_bot_detail(&db, "b1").await.unwrap().is_none());
        // 二重削除は false。
        assert!(!delete_bot(&db, "b1").await.unwrap());
    }

    #[tokio::test]
    async fn list_for_user_includes_owned_and_active_shares() {
        let db = seed_db();
        create_bot(&db, "b_owned", "owner", "Owned").await.unwrap();
        create_bot(&db, "b_other", "stranger", "Other")
            .await
            .unwrap();
        // b_other を sharee へ active 共有（公開 API 経由: 招待→承認）。
        create_share_invite(&db, "b_other", "stranger", "sharee")
            .await
            .unwrap();
        accept_share(&db, "b_other", "sharee").await.unwrap();

        // owner: 自分の b_owned のみ（b_other は非共有）。
        let owner_bots = list_bots_for_user(&db, "owner").await.unwrap();
        assert!(owner_bots.iter().any(|b| b.id == "b_owned"));
        assert!(!owner_bots.iter().any(|b| b.id == "b_other"));

        // sharee: active 共有の b_other が見える。
        let sharee_bots = list_bots_for_user(&db, "sharee").await.unwrap();
        assert!(sharee_bots.iter().any(|b| b.id == "b_other"));
        assert!(!sharee_bots.iter().any(|b| b.id == "b_owned"));
    }
}
