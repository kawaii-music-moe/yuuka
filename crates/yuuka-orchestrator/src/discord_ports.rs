//! [`yuuka_discord`] 注入ポートの **本番 DB 実装**（Node `db/*`・`services/botRateLimit` パリティ）。
//!
//! - [`DbBotDirectory`] — Bot メタデータ・アクセス判定・トークン復号・プロフィール同期。
//! - [`DbMembership`]   — 利用申請・共有招待・ペルソナインポート（ボタンフロー）。
//! - [`InMemoryRateLimiter`] — 汎用モードの利用量制御（固定窓カウンタ）。
//!
//! 起動時に `main` が構築して [`yuuka_discord::ManagerPorts`] へ渡す。DB エラーは trait が値を返す
//! 契約のため**安全側の既定**（不在/不許可/false）へ畳んで `tracing` へ記録する（Discord 経路を
//! 起動不能にしない）。トレイト定義は `yuuka-discord`、実装は本クレート（orphan 規則 OK）。
//!
//! **意図的 divergence**: Node は membership 判定中の DB 例外を外側 catch へ伝播し「処理中にエラーが
//! 発生しました」を返信するが、本実装は安全側 deny（一過性 SQLITE_BUSY でもその 1 発言を黙殺/非メンバー
//! 誘導）。WAL の読み取りは BUSY をほぼ返さないため実害は稀で、Discord 経路の頑健性を優先した。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::Local;
use rusqlite::{params, OptionalExtension};
use secrecy::SecretString;
use yuuka_core::{BotId, DbError, GuildId, UserId};
use yuuka_crypto::SystemCrypto;
use yuuka_db::map_sqlite;
use yuuka_discord::{
    BotDirectory, BotRecord, DecisionOutcome, MemberDecision, MembershipService, PersonaRecord,
    RateDecision, RateExceeded, RateLimiter, ShareRecord, SubmitOutcome,
};
use yuuka_web::Db;

use crate::bot_repo;

// ─── BotDirectory ────────────────────────────────────────────────────────────

/// Bot メタデータ・アクセス判定・トークン復号（Node `botRepo`/`botAttributesRepo`/`userRepo`）。
pub struct DbBotDirectory {
    db: Db,
    /// Discord トークン復号に使う（`YUUKA_ENCRYPTION_SECRET` 未設定なら `None`＝起動対象 0 に縮退）。
    crypto: Option<Arc<SystemCrypto>>,
}

impl DbBotDirectory {
    /// 依存を注入して構築する。
    #[must_use]
    pub fn new(db: Db, crypto: Option<Arc<SystemCrypto>>) -> Self {
        Self { db, crypto }
    }
}

/// `Result` を安全側の既定へ畳んで警告ログする（trait が値契約のため）。
fn or_default<T: Default>(r: Result<T, DbError>, what: &str) -> T {
    r.unwrap_or_else(|e| {
        tracing::warn!(error = %e, what, "BotDirectory の DB アクセスに失敗（安全側の既定で継続）");
        T::default()
    })
}

#[async_trait]
impl BotDirectory for DbBotDirectory {
    async fn get_bot(&self, bot_id: &BotId) -> Option<BotRecord> {
        or_default(bot_repo::get_bot(&self.db, bot_id.as_str()).await, "get_bot")
            .map(|r| r.to_record())
    }

    async fn list_all_bots(&self) -> Vec<BotRecord> {
        or_default(bot_repo::list_all_bots(&self.db).await, "list_all_bots")
            .iter()
            .map(bot_repo::BotRow::to_record)
            .collect()
    }

    async fn list_bots_for_user(&self, user_id: &UserId) -> Vec<BotId> {
        or_default(
            bot_repo::list_bot_ids_for_user(&self.db, user_id.as_str()).await,
            "list_bots_for_user",
        )
        .into_iter()
        .map(BotId::new)
        .collect()
    }

    async fn decrypt_token(&self, bot_id: &BotId) -> Option<SecretString> {
        let crypto = self.crypto.as_ref()?;
        let triplet =
            or_default(bot_repo::bot_discord_token(&self.db, bot_id.as_str()).await, "decrypt_token")?;
        match crypto.decrypt_text(&triplet.encrypted, &triplet.iv, &triplet.tag) {
            Ok(plain) => Some(SecretString::from(plain)),
            Err(e) => {
                tracing::warn!(error = %e, bot_id = %bot_id, "Discord トークンの復号に失敗");
                None
            }
        }
    }

    async fn update_profile(
        &self,
        bot_id: &BotId,
        username: &str,
        avatar_url: &str,
        discord_user_id: &str,
    ) {
        if let Err(e) = bot_repo::update_discord_profile(
            &self.db,
            bot_id.as_str(),
            username,
            avatar_url,
            discord_user_id,
        )
        .await
        {
            tracing::warn!(error = %e, bot_id = %bot_id, "Discord プロフィール同期に失敗");
        }
    }

    async fn is_registered_user(&self, user_id: &UserId) -> bool {
        or_default(
            bot_repo::is_registered_user(&self.db, user_id.as_str()).await,
            "is_registered_user",
        )
    }

    async fn is_bot_member(&self, bot_id: &BotId, guild_id: &GuildId, user_id: &UserId) -> bool {
        or_default(
            bot_repo::is_bot_member(&self.db, bot_id.as_str(), guild_id.as_str(), user_id.as_str())
                .await,
            "is_bot_member",
        )
    }

    async fn is_guild_allowed(&self, bot_id: &BotId, guild_id: &GuildId) -> bool {
        or_default(
            bot_repo::is_guild_allowed(&self.db, bot_id.as_str(), guild_id.as_str()).await,
            "is_guild_allowed",
        )
    }

    async fn is_any_role_allowed(
        &self,
        bot_id: &BotId,
        guild_id: &GuildId,
        role_ids: &[String],
    ) -> bool {
        or_default(
            bot_repo::is_any_role_allowed(&self.db, bot_id.as_str(), guild_id.as_str(), role_ids)
                .await,
            "is_any_role_allowed",
        )
    }
}

// ─── MembershipService ───────────────────────────────────────────────────────

/// 利用申請・共有招待・ペルソナインポートの DB 実体（Node `memberRequest`/`botRepo`/`personaRepo`）。
pub struct DbMembership {
    db: Db,
}

impl DbMembership {
    /// 依存を注入して構築する。
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait]
impl MembershipService for DbMembership {
    async fn submit_member_request(
        &self,
        bot_id: &BotId,
        guild_id: &str,
        applicant: &UserId,
        applicant_label: Option<String>,
        _guild_label: Option<String>,
    ) -> SubmitOutcome {
        // applicant_label/guild_label は owner 宛 DM 用途で interaction ハンドラが直接使う（本ポートは DB のみ）。
        let _ = (applicant_label, _guild_label);
        let r = bot_repo::submit_member_request(
            &self.db,
            bot_id.as_str(),
            guild_id,
            applicant.as_str(),
            None,
        )
        .await;
        match r {
            Ok(res) => SubmitOutcome {
                ok: res.ok,
                message: res.message,
                owner_id: res.owner_id,
                bot_name: res.bot_name,
                request_id: res.request_id,
            },
            Err(e) => {
                tracing::warn!(error = %e, "利用申請の作成に失敗");
                SubmitOutcome {
                    ok: false,
                    message: "申請の作成に失敗しました。".to_owned(),
                    ..SubmitOutcome::default()
                }
            }
        }
    }

    async fn decide_member_request(
        &self,
        request_id: i64,
        decision: MemberDecision,
        actor: &UserId,
    ) -> DecisionOutcome {
        match bot_repo::decide_member_request(&self.db, request_id, decision, actor.as_str()).await {
            Ok(res) => DecisionOutcome {
                ok: res.ok,
                message: res.message,
                status: res.status,
                bot_name: res.bot_name,
                applicant_id: res.applicant_id,
            },
            Err(e) => {
                tracing::warn!(error = %e, "利用申請の承認/却下に失敗");
                DecisionOutcome {
                    ok: false,
                    message: "処理に失敗しました。".to_owned(),
                    ..DecisionOutcome::default()
                }
            }
        }
    }

    async fn get_share(&self, share_id: i64) -> Option<ShareRecord> {
        or_default(bot_repo::get_share(&self.db, share_id).await, "get_share")
    }

    async fn accept_share(&self, bot_id: &BotId, shared_user: &UserId) {
        if let Err(e) =
            bot_repo::accept_share(&self.db, bot_id.as_str(), shared_user.as_str()).await
        {
            tracing::warn!(error = %e, "共有招待の承認に失敗");
        }
    }

    async fn revoke_share(&self, bot_id: &BotId, shared_user: &UserId) {
        if let Err(e) =
            bot_repo::revoke_share(&self.db, bot_id.as_str(), shared_user.as_str()).await
        {
            tracing::warn!(error = %e, "共有招待の辞退/取消に失敗");
        }
    }

    async fn import_persona(&self, user_id: &UserId, persona_id: i64) -> bool {
        or_default(
            bot_repo::import_persona(&self.db, user_id.as_str(), persona_id).await,
            "import_persona",
        )
    }

    async fn get_public_persona(&self, persona_id: i64) -> Option<PersonaRecord> {
        or_default(
            bot_repo::get_public_persona(&self.db, persona_id).await,
            "get_public_persona",
        )
    }
}

// ─── RateLimiter（固定窓・in-memory） ─────────────────────────────────────────

/// レート制限の既定値（Node `RATE_LIMIT_DEFAULTS`・`system_settings` で上書き可）。
const USER_PER_MINUTE_KEY: &str = "mcp_rate_user_per_minute";
const USER_PER_DAY_KEY: &str = "mcp_rate_user_per_day";
const GUILD_PER_DAY_KEY: &str = "mcp_rate_guild_per_day";
const DEFAULT_USER_PER_MINUTE: u32 = 5;
const DEFAULT_USER_PER_DAY: u32 = 100;
const DEFAULT_GUILD_PER_DAY: u32 = 1000;
/// 分窓 TTL（Node と同一 60s）。
const MINUTE_TTL: Duration = Duration::from_secs(60);
/// 日窓 TTL（Node と同一 25h・日跨ぎ吸収）。
const DAY_TTL: Duration = Duration::from_secs(25 * 60 * 60);
/// カウンタ肥大の上限（超過時に失効エントリを掃除する）。
const COUNTER_SOFT_CAP: usize = 10_000;

/// 汎用モードのレート制限（Node `consumeRateLimit` の in-memory フォールバックと同一意味論）。
///
/// Discord ゲートウェイ所有は単一プロセス（`YUUKA_RUST_DISCORD` ゲートで排他）なので、プロセス内
/// 固定窓カウンタで正確。Redis 共有カウンタ（マルチプロセス整合）は後続（Node も in-memory を
/// 「防衛線として十分」と明記）。上限は毎回 `system_settings` から読む（Node `getRateLimitSettings`）。
pub struct InMemoryRateLimiter {
    db: Db,
    counters: Mutex<HashMap<String, Window>>,
}

/// 1 カウンタ窓（現在値と失効時刻）。
struct Window {
    count: u32,
    expires_at: Instant,
}

impl InMemoryRateLimiter {
    /// 依存を注入して構築する。
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self {
            db,
            counters: Mutex::new(HashMap::new()),
        }
    }

    /// 1 リクエストを窓へ消費して現在値を返す（失効/新規は 1 にリセット）。
    fn increment(&self, key: String, ttl: Duration) -> u32 {
        let now = Instant::now();
        // 毒された Mutex でも継続する（into_inner）。
        let mut map = self.counters.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if map.len() > COUNTER_SOFT_CAP {
            map.retain(|_, w| w.expires_at > now);
        }
        let window = map.entry(key).or_insert(Window {
            count: 0,
            expires_at: now,
        });
        if window.expires_at <= now {
            window.count = 1;
            window.expires_at = now + ttl;
        } else {
            window.count = window.count.saturating_add(1);
        }
        window.count
    }

    /// `system_settings` から上限を読む（未設定/不正/0 以下は既定）。
    async fn limit(&self, key: &'static str, default: u32) -> u32 {
        let r = self
            .db
            .read
            .read(move |conn| {
                conn.query_row(
                    "SELECT value FROM system_settings WHERE key = ?1",
                    params![key],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite)
            })
            .await;
        match r {
            Ok(Some(v)) => parse_positive_int(&v).unwrap_or(default),
            Ok(None) => default,
            Err(e) => {
                tracing::warn!(error = %e, key, "レート上限設定の読み取りに失敗（既定を使用）");
                default
            }
        }
    }
}

/// Node `parseInt(raw, 10)` パリティ: 先頭空白を飛ばし、先頭の連続 10 進数字だけを解釈する
/// （末尾ゴミは無視・数字が無ければ `None`）。0 以下は既定へ畳むため `None` を返す。
fn parse_positive_int(raw: &str) -> Option<u32> {
    let digits: String = raw
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse::<u32>().ok().filter(|n| *n > 0)
}

/// ローカル暦日の `YYYYMMDD`（Node `todaySuffix`＝`new Date()` のローカル年月日）。日窓カウンタを暦日
/// 境界でリセットするためのキーサフィックス（TTL 25h は日跨ぎの掃除用）。
fn today_suffix() -> String {
    Local::now().format("%Y%m%d").to_string()
}

#[async_trait]
impl RateLimiter for InMemoryRateLimiter {
    async fn consume(&self, bot_id: &BotId, guild_id: &GuildId, user_id: &UserId) -> RateDecision {
        let (per_min, per_day, guild_day) = (
            self.limit(USER_PER_MINUTE_KEY, DEFAULT_USER_PER_MINUTE).await,
            self.limit(USER_PER_DAY_KEY, DEFAULT_USER_PER_DAY).await,
            self.limit(GUILD_PER_DAY_KEY, DEFAULT_GUILD_PER_DAY).await,
        );
        let (bid, gid, uid) = (bot_id.as_str(), guild_id.as_str(), user_id.as_str());
        // 日窓は暦日サフィックスでキーを切り替え、深夜にクォータをリセットする（Node `consumeRateLimit`）。
        let day = today_suffix();

        if self.increment(format!("mcp_rate:{bid}:{uid}:m"), MINUTE_TTL) > per_min {
            return RateDecision::deny(RateExceeded::UserMinute);
        }
        if self.increment(format!("mcp_rate:{bid}:{uid}:d:{day}"), DAY_TTL) > per_day {
            return RateDecision::deny(RateExceeded::UserDay);
        }
        if self.increment(format!("mcp_rate_guild:{bid}:{gid}:d:{day}"), DAY_TTL) > guild_day {
            return RateDecision::deny(RateExceeded::GuildDay);
        }
        RateDecision::allow()
    }
}
