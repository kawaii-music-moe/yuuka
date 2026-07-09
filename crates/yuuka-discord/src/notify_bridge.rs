//! `services::Notifier` → [`DiscordMessenger`] ブリッジ（P1-4）。
//!
//! 常駐サービス（reminder/birthday/payment …）は `yuuka_services::Notifier` ポート越しに配信する。
//! 実配信は本ブリッジが Discord の [`crate::ports::Notifier`]（`send_to_user`）へ委譲する。
//! main が `Arc<DiscordMessenger>` を `Arc<dyn yuuka_services::Notifier>` として ServiceContext へ
//! 注入すると、`NullNotifier`（未配線縮退）から実 Discord 配信へ切り替わる（Discord live = P1-3）。
//!
//! 孤児規則: `services::Notifier`（外来トレイト）を本クレートのローカル型 [`DiscordMessenger`] に
//! 実装するため、本 impl は yuuka-discord に置く（services は discord に依存しない＝非循環）。

use async_trait::async_trait;
use yuuka_services::{Notification, NotifyTarget, Notifier as ServicesNotifier};

use crate::manager::DiscordMessenger;
use crate::ports::{DeliverTarget, Notifier as DiscordNotifierPort, TurnReply};

/// services の [`NotifyTarget`] を discord の [`DeliverTarget`] へ写す。
///
/// `Default`（ユーザー既定送信先）は DM 配信に落とす。チャンネル ID 指定は透過。
/// 注: ユーザーの `notify_target_type`/`notify_target_id`（DB）に基づく既定の解決は services 側の
/// 責務（`Notification` 生成時に `Channel(..)` を積む）。本ブリッジは Target の型写像のみを担う。
fn to_deliver_target(target: NotifyTarget) -> DeliverTarget {
    match target {
        NotifyTarget::Default => DeliverTarget::Dm,
        NotifyTarget::Channel(id) => DeliverTarget::Channel(id),
    }
}

#[async_trait]
impl ServicesNotifier for DiscordMessenger {
    async fn send(&self, notification: Notification) -> bool {
        // 空本文は配信しない（Node `sendToUser` / `Notifier` 契約・呼び出し側は false で次 tick 再試行）。
        if notification.content.trim().is_empty() {
            return false;
        }
        let target = to_deliver_target(notification.target);
        let reply = TurnReply::text(notification.content);
        // discord ポートへ委譲: クライアント解決（bot_id 本人→default フォールバック）・
        // チャンネル露出ガード（第三者チャンネルへの流入防止）・分割送信は委譲先が担う。
        DiscordNotifierPort::send_to_user(
            self,
            &notification.user_id,
            reply,
            target,
            &notification.bot_id,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_maps_to_dm() {
        assert!(matches!(
            to_deliver_target(NotifyTarget::Default),
            DeliverTarget::Dm
        ));
    }

    #[test]
    fn channel_id_passes_through() {
        assert!(matches!(
            to_deliver_target(NotifyTarget::Channel("123456".to_owned())),
            DeliverTarget::Channel(id) if id == "123456"
        ));
    }
}
