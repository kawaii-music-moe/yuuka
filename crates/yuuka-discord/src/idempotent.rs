//! `(a, b)` キーの TTL ワンショットガード。用途は 2 つ:
//!
//! - **二重応答冪等ガード**（現行 `claimMessageOnce` [`src/bot.ts:939-956`]）: キー
//!   `(bot_user_id, message_id)`・TTL 60s。同一 Discord identity の接続が万一複数生じても 1
//!   メッセージへの処理を 1 回に保つ多重防御。
//! - **利用案内スロットル**（現行 `guidanceThrottle` [`src/bot.ts:609-620`]）: キー
//!   `(bot_id, user_id)`・TTL 5min。メンバー外への案内を連投スパムで送りすぎない。
//!
//! いずれも「TTL 内の初回だけ `true`、以降は `false`」という同一意味論なので 1 型で表す。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 冪等ガードの既定 TTL（現行 `HANDLED_MESSAGE_TTL_MS = 60_000`）。
pub const DEDUP_TTL: Duration = Duration::from_secs(60);
/// 利用案内スロットルの TTL（現行 `GUIDANCE_THROTTLE_MS = 5 * 60 * 1000`）。
pub const GUIDANCE_TTL: Duration = Duration::from_secs(5 * 60);
/// 遅延掃除のしきい値（現行 `size > 2000`）。
const SWEEP_THRESHOLD: usize = 2000;

/// `(a, b)` キーの TTL ワンショットガード。
pub struct MessageDedup {
    // 期限（挿入時刻 + TTL）。`moka` を使わず現行同様の手書き Map + 遅延掃除で移植する。
    seen: Mutex<HashMap<(String, String), Instant>>,
    ttl: Duration,
}

impl Default for MessageDedup {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageDedup {
    /// 冪等ガード（TTL 60s）。
    #[must_use]
    pub fn new() -> Self {
        Self::with_ttl(DEDUP_TTL)
    }

    /// 任意 TTL のガード（スロットル用途など）。
    #[must_use]
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            seen: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    /// TTL 内の初回なら `true`（続行してよい）。以降は `false`（スキップすべき）。
    /// 冪等ガードでは `claimMessageOnce`、スロットルでは「今回送ってよいか」に対応する。
    pub fn claim(&self, key_a: &str, key_b: &str) -> bool {
        self.claim_at(key_a, key_b, Instant::now())
    }

    /// 時刻注入版（テスト用に TTL 満了を決定的に検証する）。
    fn claim_at(&self, key_a: &str, key_b: &str, now: Instant) -> bool {
        let Ok(mut map) = self.seen.lock() else {
            // 毒された Mutex（別スレッド panic）でも応答は落とさない。安全側（処理続行）で返す。
            return true;
        };

        // 期限切れエントリの遅延掃除（メモリ肥大防止・低頻度で十分）。
        if map.len() > SWEEP_THRESHOLD {
            map.retain(|_, &mut exp| exp > now);
        }

        let key = (key_a.to_owned(), key_b.to_owned());
        if let Some(&exp) = map.get(&key) {
            if exp > now {
                return false; // TTL 内で既に消費済み。
            }
        }
        map.insert(key, now + self.ttl);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_claim_succeeds_duplicate_fails() {
        let dedup = MessageDedup::new();
        assert!(dedup.claim("botA", "m1"));
        assert!(!dedup.claim("botA", "m1"), "同一メッセージ2回目はスキップ");
    }

    #[test]
    fn different_bot_identity_claims_independently() {
        let dedup = MessageDedup::new();
        assert!(dedup.claim("botA", "m1"));
        // 別 Bot（別 user id）は同じ message_id でも独立に応答する（取りこぼさない）。
        assert!(dedup.claim("botB", "m1"));
    }

    #[test]
    fn claim_reopens_after_ttl_expires() {
        let dedup = MessageDedup::new();
        let t0 = Instant::now();
        assert!(dedup.claim_at("botA", "m1", t0));
        assert!(!dedup.claim_at("botA", "m1", t0 + Duration::from_secs(30)));
        // TTL 経過後は再取得できる。
        assert!(dedup.claim_at("botA", "m1", t0 + Duration::from_secs(61)));
    }
}
