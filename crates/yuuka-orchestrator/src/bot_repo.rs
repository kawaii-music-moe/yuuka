//! Bot メタデータ・メンバー制・ノート・Bot 専用鍵の DB アクセス（Node `db/botRepo`・
//! `db/botAttributesRepo`・`db/botMemberRequestRepo`・`db/personaRepo`・`db/userRepo` パリティ）。
//!
//! `yuuka-discord` の注入ポート（[`yuuka_discord::BotDirectory`]/[`yuuka_discord::MembershipService`]）を
//! [`crate::discord_ports`] が実装する際の生 SQL 層。読み取りは read pool、書き込み・読み書き混在（申請の
//! 承認等）は writer actor 上で単一 Tx として実行する（R-1: 第二 writer 経路を作らない）。

use rusqlite::{params, OptionalExtension};
use yuuka_core::DbError;
use yuuka_db::map_sqlite;
use yuuka_discord::{BotRecord, MemberDecision, PersonaRecord, ShareRecord};
use yuuka_core::{BotId, CapabilitySet, UserId};
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
    exists(db, "SELECT 1 FROM users WHERE discord_id = ?1 LIMIT 1", user_id.to_owned()).await
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
    /// 承認 DM の宛先（Bot オーナー・Node `sendMemberRequestDM` の ownerId）。ok 時のみ。
    pub owner_id: Option<String>,
    /// Bot 表示名（DM 本文）。ok 時のみ。
    pub bot_name: Option<String>,
    /// 申請 ID（承認/却下ボタンの custom_id）。ok 時のみ。
    pub request_id: Option<i64>,
}

/// 失敗結果（DM 情報なし）。
fn deny_submit(message: &str) -> SubmitResult {
    SubmitResult {
        ok: false,
        message: message.to_owned(),
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
                return Ok(deny_submit("対象のBotが見つかりません。"));
            };
            if owner == applicant {
                return Ok(deny_submit("あなたはこのBotのオーナーです（申請は不要です）。"));
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
                return Ok(deny_submit("あなたは既にこのギルドの利用メンバーです。"));
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
                Some((_, status)) if status == "pending" => {
                    Ok(deny_submit("既に申請済みです。オーナーの承認をお待ちください。"))
                }
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
    pub status: Option<MemberDecision>,
    pub bot_name: Option<String>,
    /// 申請者（結果 DM の宛先・Node `sendMemberDecisionDM` の applicantId）。ok 時のみ。
    pub applicant_id: Option<String>,
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
                return Ok(deny_decide("申請が見つかりません。"));
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
                return Ok(deny_decide("Botが見つかりません。"));
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
                return Ok(deny_decide("このBotのオーナーのみが承認/却下できます。"));
            }
            if status != "pending" {
                return Ok(deny_decide("この申請は既に処理済みです。"));
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
                return Ok(deny_decide("この申請は既に処理済みです。"));
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
                status: Some(decision),
                bot_name: Some(bot_name),
                applicant_id: Some(applicant),
            })
        })
        .await
}

/// 承認/却下の否定結果を組む短縮ヘルパ。
fn deny_decide(message: &str) -> DecideResult {
    DecideResult {
        ok: false,
        message: message.to_owned(),
        status: None,
        bot_name: None,
        applicant_id: None,
    }
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

/// 共有招待を辞退・取消（→revoked・Node `revokeShare`）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn revoke_share(db: &Db, bot_id: &str, shared_user: &str) -> Result<(), DbError> {
    let (bot_id, shared_user) = (bot_id.to_owned(), shared_user.to_owned());
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "UPDATE bot_shares SET status = 'revoked', updated_at = datetime('now','localtime') \
                 WHERE bot_id = ?1 AND shared_user_id = ?2",
                params![bot_id, shared_user],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// 公開ペルソナ（`is_public=1`）を引く（Node `getPersonaById` フィルタ済み）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_public_persona(db: &Db, persona_id: i64) -> Result<Option<PersonaRecord>, DbError> {
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
