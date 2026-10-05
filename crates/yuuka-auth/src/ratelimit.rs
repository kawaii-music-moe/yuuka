//! ログイン試行・登録コード送信のレート制限（Node `authRoutes.ts` の in-memory Map パリティ）。
//!
//! いずれも鍵は `"{clientIp}|{account}"`（IP 単独で全アカウントを巻き込まない）。プロセスローカルで
//! 十分（Node も in-memory）。失効エントリはアクセス時に遅延掃除する（Node の定期 sweep 相当）。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// ログイン試行の上限（Node `MAX_LOGIN_ATTEMPTS = 5`）。超過でロックアウト。
const MAX_LOGIN_ATTEMPTS: u32 = 5;
/// ログインロックアウト時間（Node `LOGIN_LOCKOUT_MS = 15 分`）。
const LOGIN_LOCKOUT: Duration = Duration::from_secs(15 * 60);
/// 確認コード送信の上限（Node `MAX_REGISTER_SENDS = 5`）。
const MAX_REGISTER_SENDS: u32 = 5;
/// 確認コード送信のウィンドウ（Node `REGISTER_WINDOW_MS = 15 分`）。
const REGISTER_WINDOW: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Copy)]
struct Attempt {
    count: u32,
    reset_at: Instant,
}

/// ログイン試行と登録送信の 2 系統のレート制限を束ねる（プロセス内共有・`Arc` で保持）。
#[derive(Default)]
pub struct RateLimiter {
    login: Mutex<HashMap<String, Attempt>>,
    register_send: Mutex<HashMap<String, Attempt>>,
}

impl RateLimiter {
    /// 空のレートリミッタ。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// ログインがロックアウト中かを返す。ロック中なら `Some(残り秒)`、許可なら `None`
    /// （Node: `count >= MAX && now < resetAt` のとき 429、残り秒 = `ceil((resetAt-now)/1000)`）。
    pub fn login_locked_secs(&self, key: &str) -> Option<u64> {
        let map = lock(&self.login);
        let a = map.get(key)?;
        if a.count >= MAX_LOGIN_ATTEMPTS {
            let now = Instant::now();
            if now < a.reset_at {
                return Some(ceil_secs(a.reset_at - now));
            }
        }
        None
    }

    /// ログイン失敗を記録する（Node: 窓が無い/失効済みなら新規窓 `{1, now+15分}`、窓内なら `count+=1`）。
    /// ロック中に `resetAt` を延長しない（毎回延ばさない）点まで一致させる。
    pub fn record_login_failure(&self, key: &str) {
        let mut map = lock(&self.login);
        let now = Instant::now();
        match map.get_mut(key) {
            Some(a) if a.reset_at > now => a.count += 1,
            _ => {
                map.insert(
                    key.to_owned(),
                    Attempt {
                        count: 1,
                        reset_at: now + LOGIN_LOCKOUT,
                    },
                );
            }
        }
    }

    /// ログイン成功時に試行カウントを消す（Node `loginAttempts.delete(rlKey)`）。
    pub fn clear_login(&self, key: &str) {
        lock(&self.login).remove(key);
    }

    /// 登録コード送信のレート制限を消費する（Node `allowRegisterSend`）。上限超過なら `false`。
    pub fn allow_register_send(&self, key: &str) -> bool {
        let mut map = lock(&self.register_send);
        let now = Instant::now();
        match map.get_mut(key) {
            Some(a) if a.reset_at > now => {
                if a.count >= MAX_REGISTER_SENDS {
                    false
                } else {
                    a.count += 1;
                    true
                }
            }
            _ => {
                map.insert(
                    key.to_owned(),
                    Attempt {
                        count: 1,
                        reset_at: now + REGISTER_WINDOW,
                    },
                );
                true
            }
        }
    }
}

/// ロック毒化しても中身を回収して継続（panic 非伝播・絶対制約1）。
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `Duration` を切り上げ秒へ（Node `Math.ceil(ms/1000)` 相当）。
fn ceil_secs(d: Duration) -> u64 {
    d.as_secs() + u64::from(d.subsec_nanos() > 0)
}

#[cfg(test)]
mod tests {
    use super::RateLimiter;

    #[test]
    fn login_locks_after_five_failures() {
        let rl = RateLimiter::new();
        let key = "1.2.3.4|acct";
        // 5 回失敗までは許可（count 1..=5）。
        for _ in 0..5 {
            assert_eq!(rl.login_locked_secs(key), None);
            rl.record_login_failure(key);
        }
        // 6 回目のチェックでロック（count==5 && now<reset）。残り秒 > 0。
        let remain = rl.login_locked_secs(key).expect("locked");
        assert!(remain > 0 && remain <= 15 * 60);
        // 成功でクリアされる。
        rl.clear_login(key);
        assert_eq!(rl.login_locked_secs(key), None);
    }

    #[test]
    fn register_send_allows_five_then_blocks() {
        let rl = RateLimiter::new();
        let key = "1.2.3.4|123";
        for _ in 0..5 {
            assert!(rl.allow_register_send(key));
        }
        // 6 回目は上限超過。
        assert!(!rl.allow_register_send(key));
    }
}
