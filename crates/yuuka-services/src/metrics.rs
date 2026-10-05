//! 軽量ベースラインメトリクス（architecture §10・現行 `src/services/metrics.ts`）。
//!
//! 依存ゼロ・インメモリのみ（プロセス再起動でリセット＝恒久ストアは目的外）。カウンタと
//! レイテンシ標本（kind ごと最大 1000・古いものから捨てる）を保持し、[`MetricsLogService`] が
//! 5 分ごとに snapshot をログに出す。カウンタ更新は将来 gemini/tools 層が [`ServiceContext`]
//! 経由でこのレジストリへ書き込む（現状は配線待ちのため 0 が出る）。

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;

use crate::context::ServiceContext;
use crate::schedule::{CronService, Schedule};

/// 各レイテンシ kind の最大標本数（現行 `MAX_SAMPLES`）。
const MAX_SAMPLES: usize = 1000;
/// 定期ログ間隔（現行 `LOG_INTERVAL_MS = 5 分`）。
const LOG_INTERVAL_SECS: u64 = 5 * 60;

/// カウンタ＋レイテンシ標本のインメモリレジストリ（スレッド安全）。
#[derive(Default)]
pub struct MetricsRegistry {
    counters: Mutex<HashMap<String, u64>>,
    latencies: Mutex<HashMap<String, VecDeque<u64>>>,
}

impl MetricsRegistry {
    /// 新規レジストリ。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// カウンタを増やす（例: `"gemini_calls"`/`"recall_hits"`/`"tool_calls"`・現行 `incrMetric`）。
    pub fn incr(&self, name: &str, by: u64) {
        let mut c = self.counters.lock().unwrap_or_else(PoisonError::into_inner);
        *c.entry(name.to_owned()).or_insert(0) += by;
    }

    /// レイテンシ標本を記録する（ms・現行 `recordLatency`）。上限超で最古を捨てる。
    pub fn record_latency(&self, kind: &str, ms: u64) {
        let mut l = self
            .latencies
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let q = l.entry(kind.to_owned()).or_default();
        q.push_back(ms);
        while q.len() > MAX_SAMPLES {
            q.pop_front();
        }
    }

    /// 定期ログ用の 1 行スナップショット（現行 `metricsSnapshot` の要約）。
    #[must_use]
    pub fn snapshot_line(&self) -> String {
        let counters = self.counters.lock().unwrap_or_else(PoisonError::into_inner);
        let latencies = self
            .latencies
            .lock()
            .unwrap_or_else(PoisonError::into_inner);

        let mut counter_parts: Vec<String> =
            counters.iter().map(|(k, v)| format!("{k}={v}")).collect();
        counter_parts.sort();

        let mut latency_parts: Vec<String> = latencies
            .iter()
            .map(|(kind, arr)| {
                let mut sorted: Vec<u64> = arr.iter().copied().collect();
                sorted.sort_unstable();
                let count = sorted.len();
                let sum: u64 = sorted.iter().sum();
                let avg = if count == 0 {
                    0
                } else {
                    (sum + (count as u64) / 2) / count as u64
                };
                format!(
                    "{kind}(count={count},p50={},p95={},avg={avg})",
                    percentile(&sorted, 50),
                    percentile(&sorted, 95),
                )
            })
            .collect();
        latency_parts.sort();

        format!(
            "counters{{{}}} latency{{{}}}",
            counter_parts.join(","),
            latency_parts.join(",")
        )
    }
}

/// ソート済み標本から最近傍パーセンタイル（現行 `percentile`・`ceil(p/100*len)-1` を整数で）。
fn percentile(sorted: &[u64], p: usize) -> u64 {
    let len = sorted.len();
    if len == 0 {
        return 0;
    }
    let ceil = (p * len).div_ceil(100); // ceil(p*len/100)
    let idx = ceil.saturating_sub(1).min(len - 1);
    sorted.get(idx).copied().unwrap_or(0)
}

/// 5 分ごとにメトリクス snapshot をログへ出すサービス（現行 `startMetricsLogging`）。
pub struct MetricsLogService;

#[async_trait]
impl CronService for MetricsLogService {
    fn name(&self) -> &'static str {
        "metrics"
    }

    fn schedule(&self) -> Schedule {
        Schedule::FixedSecs(LOG_INTERVAL_SECS)
    }

    fn run_on_start(&self) -> bool {
        // 現行は setInterval のみ（起動直後には出さない）。
        false
    }

    async fn tick(&self, ctx: &ServiceContext) {
        tracing::info!("📊 [Metrics] {}", ctx.metrics.snapshot_line());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_matches_node_semantics() {
        // 1..=100 の p50=50, p95=95（現行と同じ最近傍）。
        let v: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile(&v, 50), 50);
        assert_eq!(percentile(&v, 95), 95);
        assert_eq!(percentile(&[], 50), 0);
        assert_eq!(percentile(&[7], 95), 7);
    }

    #[test]
    fn registry_counts_and_caps_samples() {
        let m = MetricsRegistry::new();
        m.incr("gemini_calls", 1);
        m.incr("gemini_calls", 2);
        for i in 0..1500 {
            m.record_latency("response", i);
        }
        let line = m.snapshot_line();
        assert!(line.contains("gemini_calls=3"), "line: {line}");
        // 上限 1000 に丸められている（最古 500 を捨てた）→ count=1000。
        assert!(line.contains("count=1000"), "line: {line}");
    }
}
