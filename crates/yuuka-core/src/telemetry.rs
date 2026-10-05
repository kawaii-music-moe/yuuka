//! tracing/tracing-subscriber の初期化（§2.1 telemetry）。
//!
//! `init_telemetry()` を起動シーケンスで 1 度だけ呼ぶ。`RUST_LOG` 相当の
//! `EnvFilter` を尊重し、未指定時は `info` 既定。

use std::io::IsTerminal;

use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// telemetry を初期化する。二重初期化は無害に無視される（`try_init`）。
pub fn init_telemetry() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    // ANSI カラーコードは `docker logs` 経由の grep（deploy/instance.sh のヘルスチェック・#53）を
    // 不安定にするため、標準出力が TTY でない場合（コンテナ実行時は常にこれに該当）や
    // `NO_COLOR`（https://no-color.org/）が設定されている場合は無効化する。対話的な
    // `cargo run` 等ではそのまま色付き出力を維持する。
    let ansi = std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal();
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_ansi(ansi))
        .try_init();
}
