//! tracing/tracing-subscriber の初期化（§2.1 telemetry）。
//!
//! `init_telemetry()` を起動シーケンスで 1 度だけ呼ぶ。`RUST_LOG` 相当の
//! `EnvFilter` を尊重し、未指定時は `info` 既定。

use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// telemetry を初期化する。二重初期化は無害に無視される（`try_init`）。
pub fn init_telemetry() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer())
        .try_init();
}
