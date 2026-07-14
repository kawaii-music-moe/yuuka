//! DM チャレンジ登録の状態機械（Node `src/services/pendingRegistration.ts` パリティ）と
//! 確認コード DM 配信ポート（Node `bot.ts::sendRegistrationCodeDM`）。
//!
//! 登録は「主張された Discord ID 宛にワンタイムコードを DM し、本人確認後にのみユーザー作成」する
//! （G1 なりすまし対策）。保留は**プロセスローカルの in-memory**（`Mutex<HashMap>`・Node の `Map` 等価）。
//! コードは 6 桁 CSPRNG（000000–999999 一様）・TTL 10 分・最大 5 回試行。照合は定数時間。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;

/// 保留の有効期限（Node `TTL_MS = 10 * 60 * 1000`）。
const TTL: Duration = Duration::from_secs(10 * 60);
/// コード誤入力の許容回数（Node `MAX_ATTEMPTS = 5`）。6 回目の検証は `TooManyAttempts` で破棄。
const MAX_ATTEMPTS: u32 = 5;

/// 本人確認前に保持する登録データ（Node `PendingRegistration`）。`password` は平文・**ログ厳禁**。
#[derive(Debug, Clone)]
pub struct PendingRegistration {
    /// 表示名。
    pub username: String,
    /// 平文パスワード（確認完了後に bcrypt 化して破棄）。
    pub password: String,
    /// Gemini API キー（確認完了後に暗号化保存）。
    pub gemini_api_key: String,
    /// 事前検証済みの招待コード（確認完了後にアトミック消費）。
    pub invite_code: String,
}

/// 検証結果（Node `VerifyResult`）。
#[derive(Debug)]
pub enum VerifyResult {
    /// コード一致（保留は消費済み）。
    Ok(Box<PendingRegistration>),
    /// 該当する保留が無い。
    NotFound,
    /// 有効期限切れ（保留は破棄済み）。
    Expired,
    /// 試行回数超過（保留は破棄済み）。
    TooManyAttempts,
    /// コード不一致（保留は保持・試行回数 +1）。
    CodeMismatch,
}

struct PendingEntry {
    reg: PendingRegistration,
    code: String,
    expires_at: Instant,
    attempts: u32,
}

/// `discordId → 保留登録` の in-memory ストア（プロセスローカル・Node の `Map` 等価）。
#[derive(Default)]
pub struct PendingStore {
    inner: Mutex<HashMap<String, PendingEntry>>,
}

impl PendingStore {
    /// 空のストアを作る。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 6 桁ワンタイムコード（CSPRNG・000000–999999 一様）を生成する（Node `crypto.randomInt(0,1_000_000)`）。
    fn generate_code() -> Result<String, getrandom::Error> {
        Ok(format!("{:06}", uniform_below_million()?))
    }

    /// 保留を作成（同一 ID は上書き）し、DM 用コードを返す。**返り値はログ/レスポンスに出さない**。
    ///
    /// # Errors
    /// CSPRNG（コード生成）失敗時 [`getrandom::Error`]（実質起こらない）。
    pub fn create(
        &self,
        discord_id: &str,
        reg: PendingRegistration,
    ) -> Result<String, getrandom::Error> {
        let code = Self::generate_code()?;
        let entry = PendingEntry {
            reg,
            code: code.clone(),
            expires_at: Instant::now() + TTL,
            attempts: 0,
        };
        let mut map = self.lock();
        // 肥大防止に期限切れを掃除してから挿入（Node の 5 分 sweep 相当の軽量版）。
        let now = Instant::now();
        map.retain(|_, e| e.expires_at > now);
        map.insert(discord_id.to_owned(), entry);
        Ok(code)
    }

    /// コードを検証する。判定順は Node と同一（not_found → expired → too_many → mismatch → 成功）。
    pub fn verify(&self, discord_id: &str, code: &str) -> VerifyResult {
        let mut map = self.lock();
        let Some(entry) = map.get_mut(discord_id) else {
            return VerifyResult::NotFound;
        };
        if entry.expires_at <= Instant::now() {
            map.remove(discord_id);
            return VerifyResult::Expired;
        }
        if entry.attempts >= MAX_ATTEMPTS {
            map.remove(discord_id);
            return VerifyResult::TooManyAttempts;
        }
        if ct_eq(entry.code.as_bytes(), code.as_bytes()) {
            // 一致 → 消費（削除）。直前に get_mut で存在確認済みだが expect を避け match で束ねる。
            match map.remove(discord_id) {
                Some(e) => VerifyResult::Ok(Box::new(e.reg)),
                None => VerifyResult::NotFound,
            }
        } else {
            entry.attempts += 1;
            VerifyResult::CodeMismatch
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, PendingEntry>> {
        // ロック毒化しても中身を回収して継続（panic を伝播させない・絶対制約1）。
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// 0..1_000_000 の一様乱数（棄却サンプリングでモジュロバイアスを排除・Node `crypto.randomInt` 等価）。
///
/// # Errors
/// CSPRNG 失敗時 [`getrandom::Error`]。
fn uniform_below_million() -> Result<u32, getrandom::Error> {
    const BOUND: u32 = 1_000_000;
    // u32 空間を BOUND の倍数で切った上限。これ以上は棄却して偏りを無くす。
    const LIMIT: u32 = u32::MAX - (u32::MAX % BOUND);
    loop {
        let mut buf = [0u8; 4];
        getrandom::getrandom(&mut buf)?;
        let v = u32::from_le_bytes(buf);
        if v < LIMIT {
            return Ok(v % BOUND);
        }
    }
}

/// 定数時間バイト等価（Node `timingSafeEqual` 相当・長さ差は先に弾く）。
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// 確認コード DM 配信ポート（Node `sendRegistrationCodeDM: Promise<boolean>`）。
///
/// 実配信は Discord Messenger をアダプトして注入する（Discord live = P1-3）。Bot 未起動・DM 不可・
/// 送信失敗はすべて `false`（route は 502 を返す＝Node パリティ）。[`Notifier`] と同じ
/// `Arc<dyn Trait>` 注入規律に従う。
///
/// [`Notifier`]: yuuka_services（`crates/yuuka-services/src/notifier.rs`）
#[async_trait]
pub trait RegistrationDm: Send + Sync {
    /// 確認コードを `discord_id` 宛に DM する。送信成功なら `true`。**コードはログ厳禁**。
    async fn send_registration_code(&self, discord_id: &str, code: &str) -> bool;
}

/// Discord 未配線時の縮退実装。配信せず `false` を返す（Node `!client.isReady()` 相当）。
pub struct NullRegistrationDm;

#[async_trait]
impl RegistrationDm for NullRegistrationDm {
    async fn send_registration_code(&self, discord_id: &str, _code: &str) -> bool {
        tracing::debug!(
            discord_id = %discord_id,
            "登録確認コード DM 配信先（Discord）未配線のためスキップ（route は 502）"
        );
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{ct_eq, uniform_below_million, PendingRegistration, PendingStore, VerifyResult};

    fn reg() -> PendingRegistration {
        PendingRegistration {
            username: "yuu".to_owned(),
            password: "Secret-pass-1".to_owned(),
            gemini_api_key: "k".to_owned(),
            invite_code: "INV".to_owned(),
        }
    }

    #[test]
    fn code_is_six_digits() {
        let store = PendingStore::new();
        let code = store.create("123", reg()).expect("code");
        assert_eq!(code.len(), 6);
        assert!(code.bytes().all(|b| b.is_ascii_digit()));
    }

    #[test]
    fn verify_success_consumes_entry() {
        let store = PendingStore::new();
        let code = store.create("123", reg()).expect("code");
        // 一致で Ok（保留データを取り出せる）。panic! を避け matches! ガードで検証する。
        assert!(matches!(store.verify("123", &code), VerifyResult::Ok(r) if r.username == "yuu"));
        // 消費済み → 2 回目は NotFound。
        assert!(matches!(store.verify("123", &code), VerifyResult::NotFound));
    }

    #[test]
    fn mismatch_increments_then_too_many() {
        let store = PendingStore::new();
        let _code = store.create("123", reg()).expect("code");
        // 5 回まで CodeMismatch（保持）。6 回目は TooManyAttempts（破棄）。
        for _ in 0..5 {
            assert!(matches!(
                store.verify("123", "000000"),
                VerifyResult::CodeMismatch
            ));
        }
        assert!(matches!(
            store.verify("123", "000000"),
            VerifyResult::TooManyAttempts
        ));
        // 破棄後は NotFound。
        assert!(matches!(
            store.verify("123", "000000"),
            VerifyResult::NotFound
        ));
    }

    #[test]
    fn not_found_for_unknown_id() {
        let store = PendingStore::new();
        assert!(matches!(
            store.verify("nope", "000000"),
            VerifyResult::NotFound
        ));
    }

    #[test]
    fn ct_eq_matches_semantics() {
        assert!(ct_eq(b"123456", b"123456"));
        assert!(!ct_eq(b"123456", b"123457"));
        assert!(!ct_eq(b"12345", b"123456")); // 長さ差
    }

    #[test]
    fn uniform_is_in_range() {
        for _ in 0..1000 {
            assert!(uniform_below_million().expect("rng") < 1_000_000);
        }
    }
}
