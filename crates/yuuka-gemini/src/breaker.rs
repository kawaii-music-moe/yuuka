//! 極小サーキットブレーカ（§8.2.3・§5.5 / 00-decisions #8 の「自前 `AtomicU*` 状態機械」）。
//!
//! 外部依存（Gemini エンドポイント）の連続失敗を検知して open にし、cool-down 中の
//! 呼び出しを即 fail させて無駄打ち＋雪崩を防ぐ（劣化縮退＝現行「混み合っています」定型応答）。
//! `recloser` を採らず self-contained にしたのは、offline ビルド互換と依存最小化のため
//! （00-decisions #8 が「自前も対等な選択肢」と明記）。
//!
//! 状態遷移: Closed → (連続失敗 >= threshold) → Open → (cool-down 経過) → HalfOpen
//! → (成功) Closed / (失敗) Open。half-open は 1 本だけ試行を通す。

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::time::{Duration, Instant};

const STATE_CLOSED: u8 = 0;
const STATE_OPEN: u8 = 1;
const STATE_HALF_OPEN: u8 = 2;

/// スレッド安全な最小ブレーカ。`Instant` は生成起点からの経過 ms を `AtomicU64` に格納する
/// （`Instant::now()` はワークフロースクリプトではなく通常ランタイムで使うため問題ない）。
#[derive(Debug)]
pub struct CircuitBreaker {
    state: AtomicU8,
    consecutive_failures: AtomicU64,
    /// open に入った時刻（`origin` からの経過ミリ秒）。
    opened_at_ms: AtomicU64,
    origin: Instant,
    failure_threshold: u64,
    cooldown: Duration,
}

impl CircuitBreaker {
    /// `failure_threshold` 連続失敗で open、`cooldown` 経過後に half-open。
    #[must_use]
    pub fn new(failure_threshold: u64, cooldown: Duration) -> Self {
        Self {
            state: AtomicU8::new(STATE_CLOSED),
            consecutive_failures: AtomicU64::new(0),
            opened_at_ms: AtomicU64::new(0),
            origin: Instant::now(),
            failure_threshold: failure_threshold.max(1),
            cooldown,
        }
    }

    fn now_ms(&self) -> u64 {
        // saturating: 単調増加なので実質溢れないが、 millis の u128→u64 を安全に丸める。
        u64::try_from(self.origin.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// 呼び出し前ゲート。`true` なら試行してよい。open かつ cool-down 中は `false`
    /// （呼び出し側は劣化縮退へ落とす）。cool-down 経過後は half-open へ 1 本だけ通す。
    #[must_use]
    pub fn acquire(&self) -> bool {
        match self.state.load(Ordering::Acquire) {
            STATE_CLOSED | STATE_HALF_OPEN => true,
            _ => {
                // open: cool-down 経過を確認。経過していれば half-open へ 1 本だけ通す。
                let opened = self.opened_at_ms.load(Ordering::Acquire);
                let elapsed_ms = self.now_ms().saturating_sub(opened);
                if elapsed_ms >= u64::try_from(self.cooldown.as_millis()).unwrap_or(u64::MAX) {
                    // Open → HalfOpen（1 本試行を許可）。他スレッドと競合しても実害は無い。
                    self.state.store(STATE_HALF_OPEN, Ordering::Release);
                    true
                } else {
                    false
                }
            }
        }
    }

    /// 成功を記録。half-open からの成功で Closed へ復帰、失敗カウンタをリセット。
    pub fn on_success(&self) {
        self.consecutive_failures.store(0, Ordering::Release);
        self.state.store(STATE_CLOSED, Ordering::Release);
    }

    /// 失敗を記録。連続失敗が閾値到達で open にする。
    pub fn on_failure(&self) {
        let failures = self.consecutive_failures.fetch_add(1, Ordering::AcqRel) + 1;
        if failures >= self.failure_threshold
            || self.state.load(Ordering::Acquire) == STATE_HALF_OPEN
        {
            self.opened_at_ms.store(self.now_ms(), Ordering::Release);
            self.state.store(STATE_OPEN, Ordering::Release);
        }
    }

    /// 現在 open か（テスト・観測用）。
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.state.load(Ordering::Acquire) == STATE_OPEN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_after_threshold_and_recovers_after_cooldown() {
        let cb = CircuitBreaker::new(2, Duration::from_millis(20));
        assert!(cb.acquire());

        cb.on_failure();
        assert!(cb.acquire(), "1 失敗ではまだ closed");
        cb.on_failure();
        assert!(cb.is_open(), "閾値到達で open");
        assert!(!cb.acquire(), "cool-down 中は即 fail");

        std::thread::sleep(Duration::from_millis(25));
        assert!(cb.acquire(), "cool-down 経過で half-open 試行を許可");
        cb.on_success();
        assert!(!cb.is_open(), "half-open 成功で closed 復帰");
        assert!(cb.acquire());
    }

    #[test]
    fn half_open_failure_reopens() {
        let cb = CircuitBreaker::new(1, Duration::from_millis(10));
        cb.on_failure();
        assert!(cb.is_open());
        std::thread::sleep(Duration::from_millis(15));
        assert!(cb.acquire()); // half-open
        cb.on_failure(); // half-open で失敗 → 再 open
        assert!(cb.is_open());
    }
}
