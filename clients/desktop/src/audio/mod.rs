//! 音声入力（Phase 4）。`audio` feature の裏に隔離する。
//!
//! feature OFF（既定）でもクレートはコンパイルできること（要件 §10）。
//! そのため、feature 無効時も最小の型/関数シグネチャだけは提供する。

pub mod record;

/// 録音結果（送信用の base64 添付）。WS `msg.audio` にそのまま載せられる形。
#[derive(Debug, Clone)]
pub struct RecordedAudio {
    /// MIME（`audio/ogg`（Opus）推奨。フォールバックで `audio/wav`）。
    pub mime: String,
    /// base64 エンコード済みデータ。
    pub data_base64: String,
}
