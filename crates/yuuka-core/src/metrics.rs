//! 実利用メトリクスの記録先ポート（Node `src/services/metrics.ts` の `incrMetric`/`recordLatency`）。
//!
//! 実体（インメモリレジストリ + 5 分毎の定期ログ）は `yuuka-services::MetricsRegistry`。会話の実処理
//! （orchestrator の engine・gemini の FC ループ・シナプス想起）は services に依存しないため、
//! 本トレイト越しに注入されたレジストリへ書き込む（依存の向きは `* → core` のまま）。

/// カウンタとレイテンシ標本の記録先。記録は失敗しない（ベストエフォート・応答を妨げない）。
pub trait MetricsSink: Send + Sync {
    /// カウンタを `by` だけ増やす（例: `"messages"`/`"tool_calls"`/`"recall_hit"`）。
    fn incr(&self, name: &str, by: u64);

    /// レイテンシ標本（ミリ秒）を記録する（例: `"response"`/`"assemble"`）。
    fn record_latency(&self, kind: &str, ms: u64);
}
