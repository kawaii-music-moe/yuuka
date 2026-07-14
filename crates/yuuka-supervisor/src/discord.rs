//! Discord テナントを [`SupervisedService`] 化するアダプタ（§8.3・絶対制約2）。
//!
//! `yuuka-discord` は supervisor に依存しない（DAG: `supervisor → discord`）。本モジュールが
//! [`TenantRunner`] を [`SupervisedService`] でラップし、Shard の恒久クローズ（無効トークン・
//! DisallowedIntents 等・`DiscordError::is_fatal()`）を [`ServiceError::Permanent`]（再起動しない）、
//! それ以外の一過性障害を [`ServiceError::Transient`]（指数バックオフ再起動）へ分類する。
//!
//! **Phase 4 での有効化**: real ports（`BotDirectory`/`TurnProcessor`/`MembershipService`/
//! `RateLimiter`）が揃ったら `main` が [`DiscordManager::prepare`] → 各 `runner` を本アダプタで
//! [`Supervisor::service`] へ登録する。現段階（Phase 3）では seam のみを凍結し、live 起動へは繋がない。

use std::sync::Arc;

use yuuka_auth::RegistrationDm;
use yuuka_core::DiscordError;
use yuuka_discord::{DiscordMessenger, TenantRunner};

use crate::supervisor::{ServiceError, ShutdownToken, SupervisedService};

/// 1 Discord テナント（Shard poll loop）を監督下タスク化するアダプタ。
pub struct DiscordTenantService {
    runner: TenantRunner,
}

impl DiscordTenantService {
    #[must_use]
    pub fn new(runner: TenantRunner) -> Self {
        Self { runner }
    }
}

#[async_trait::async_trait]
impl SupervisedService for DiscordTenantService {
    fn name(&self) -> String {
        format!("discord:{}", self.runner.bot_id())
    }

    async fn run(&self, mut shutdown: ShutdownToken) -> Result<(), ServiceError> {
        // 停止協調は cancel future として tenant ループの `select!` へ配線する。
        let result = self
            .runner
            .run(async move { shutdown.cancelled().await })
            .await;
        map_discord_result(result)
    }
}

/// 認証発行の登録コード DM ポート（[`yuuka_auth::RegistrationDm`]）を [`DiscordMessenger`] で満たす
/// 合成ルートアダプタ。
///
/// `yuuka-auth`（トレイト）と `yuuka-discord`（`send_registration_code_dm`）は互いに依存しないため、
/// 双方に依存する supervisor で newtype 越しに橋渡しする（notify_bridge の Notifier と同じ規律）。
/// デフォルト Bot 未起動なら `send_registration_code_dm` が `false` を返し、`/api/register` は 502 に縮退する。
pub struct MessengerRegistrationDm {
    messenger: Arc<DiscordMessenger>,
}

impl MessengerRegistrationDm {
    /// messenger を注入して構築する。
    #[must_use]
    pub fn new(messenger: Arc<DiscordMessenger>) -> Self {
        Self { messenger }
    }
}

#[async_trait::async_trait]
impl RegistrationDm for MessengerRegistrationDm {
    async fn send_registration_code(&self, discord_id: &str, code: &str) -> bool {
        self.messenger
            .send_registration_code_dm(discord_id, code)
            .await
    }
}

/// `DiscordError` を supervisor 分類へ写像する（恒久クローズのみ再起動しない）。
fn map_discord_result(result: Result<(), DiscordError>) -> Result<(), ServiceError> {
    match result {
        Ok(()) => Ok(()),
        // 無効トークン・インテント拒否等（再接続不能）は恒久障害＝再起動しない。
        Err(e) if e.is_fatal() => Err(ServiceError::permanent(e.to_string())),
        // ストリーム終了・その他は一過性＝バックオフ再起動でテナントを起こし直す。
        Err(e) => Err(ServiceError::transient(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use yuuka_core::{DiscordError, Fatality};

    use super::map_discord_result;
    use crate::supervisor::ServiceError;

    fn fatality_of(e: DiscordError) -> Fatality {
        match map_discord_result(Err(e)) {
            Err(se) => se.fatality(),
            Ok(()) => Fatality::Transient, // 到達しない（Err を渡す）。
        }
    }

    #[test]
    fn fatal_close_maps_to_permanent() {
        // 恒久クローズ（無効トークン等）は再起動しない。
        assert_eq!(
            fatality_of(DiscordError::ShardClosedFatal { code: Some(4004) }),
            Fatality::Permanent
        );
    }

    #[test]
    fn transient_errors_map_to_transient() {
        // 一過性のゲートウェイ断・トランスポート障害はバックオフ再起動対象。
        assert_eq!(
            fatality_of(DiscordError::GatewayClosed),
            Fatality::Transient
        );
        assert_eq!(
            fatality_of(DiscordError::Transport("boom".to_owned())),
            Fatality::Transient
        );
    }

    #[test]
    fn intentional_stop_is_ok() {
        // 停止協調での正常終了は Ok（再起動しない）。
        assert!(matches!(map_discord_result(Ok(())), Ok(())));
        // 型の健全性: ServiceError の判定に使う名前を参照（未使用 import 回避）。
        let _ = ServiceError::transient("x");
    }
}
