//! 会話ターン実行ポート（cron が秘書ターンを起動するための境界・DAG: `services → core`）。
//!
//! playbook 定期実行は Node `playbookScheduleService.executePlaybook` が `processMessage`
//! （gemini オーケストレーション本体）を呼ぶ。Rust では `services → orchestrator` の逆依存を避け、
//! [`Notifier`](crate::notifier::Notifier) と同じ「services 所有ポート + 外部で impl + `ServiceContext`
//! 注入」方式にする。実体（`ChatEngine`）へのブリッジは supervisor（main.rs のアダプタ）が担う。

use async_trait::async_trait;
use yuuka_core::{BotId, UserId};

/// 秘書経路の 1 ターンを起動するポート（cron 由来・本人の Gemini キー/データで実行）。
///
/// 返り値は最終テキスト（Node `ProcessResult.text` 相当）。`embeds`/`files` は当面ドロップする
/// （通知は text のみ・既知の縮退）。失敗はエラー文言（そのまま run 記録・通知に使う）。
#[async_trait]
pub trait PlaybookRunner: Send + Sync {
    /// 指定 Bot/ユーザーのコンテキストで `prompt` を 1 ターン実行し、最終テキストを返す。
    ///
    /// # Errors
    /// ターン処理に失敗した場合、人間可読なエラー文言を `Err` で返す（run=failed・通知に転用）。
    async fn run_secretary(
        &self,
        bot_id: &BotId,
        user_id: &UserId,
        prompt: String,
    ) -> Result<String, String>;
}

/// 未配線時の縮退ランナー（常に失敗を返す＝run は failed 記録・実行はされない）。
///
/// `YUUKA_RUST_CRON` 無効時や、会話エンジン未構築のフォールバックに使う。
pub struct NullPlaybookRunner;

#[async_trait]
impl PlaybookRunner for NullPlaybookRunner {
    async fn run_secretary(
        &self,
        _bot_id: &BotId,
        _user_id: &UserId,
        _prompt: String,
    ) -> Result<String, String> {
        Err("会話エンジンが未配線のためマクロを実行できません。".to_owned())
    }
}
