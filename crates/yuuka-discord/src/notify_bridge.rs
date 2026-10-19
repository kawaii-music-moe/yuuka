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
use yuuka_services::{Notification, Notifier as ServicesNotifier, NotifyTarget};

use crate::manager::DiscordMessenger;
use crate::ports::{
    DeliverTarget, EmbedField, Notifier as DiscordNotifierPort, RichEmbed, TurnReply,
};

/// services の [`NotifyTarget`] を discord の [`DeliverTarget`] へ写す。
///
/// `Dm` は DM、チャンネル ID 指定は透過。
/// 注: ユーザーの `notify_target_type`/`notify_target_id`（DB）に基づく `Default` の解決は services 側の
/// 責務（`ServiceContext` の notifier が `Dm` / `Channel(..)` へ解決してから渡す）。本ブリッジは Target の
/// 型写像のみを担い、未解決の `Default` が届いた場合は DM に落とす。
fn to_deliver_target(target: NotifyTarget) -> DeliverTarget {
    match target {
        NotifyTarget::Default | NotifyTarget::Dm => DeliverTarget::Dm,
        NotifyTarget::Channel(id) => DeliverTarget::Channel(id),
    }
}

fn to_rich_embed(embed: yuuka_core::EmbedPart) -> RichEmbed {
    RichEmbed {
        title: embed.title,
        description: embed.description,
        color: Some(embed.color),
        fields: embed
            .fields
            .into_iter()
            .map(|field| EmbedField {
                name: field.name,
                value: field.value,
                inline: field.inline,
            })
            .collect(),
        footer: embed.footer,
    }
}

#[async_trait]
impl ServicesNotifier for DiscordMessenger {
    async fn send(&self, notification: Notification) -> bool {
        // 本文も Embed も無い通知は配信しない（Node `sendToUser` / `Notifier` 契約）。
        if notification.content.trim().is_empty() && notification.embeds.is_empty() {
            return false;
        }
        let target = to_deliver_target(notification.target);
        let reply = TurnReply {
            text: notification.content,
            embeds: notification.embeds.into_iter().map(to_rich_embed).collect(),
            ..TurnReply::default()
        };
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
    fn dm_maps_to_dm() {
        assert!(matches!(
            to_deliver_target(NotifyTarget::Dm),
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

    #[test]
    fn service_embed_maps_to_discord_embed() {
        let embed = to_rich_embed(yuuka_core::EmbedPart {
            title: Some("朝報".to_owned()),
            description: Some("本文".to_owned()),
            color: 0x00_B0_F4,
            fields: vec![yuuka_core::EmbedFieldPart {
                name: "天気".to_owned(),
                value: "晴れ".to_owned(),
                inline: false,
            }],
            footer: Some("提供元".to_owned()),
        });

        assert_eq!(embed.title.as_deref(), Some("朝報"));
        assert_eq!(embed.description.as_deref(), Some("本文"));
        assert_eq!(embed.color, Some(0x00_B0_F4));
        assert_eq!(embed.fields.len(), 1);
        assert_eq!(embed.fields[0].name, "天気");
        assert!(!embed.fields[0].inline);
        assert_eq!(embed.footer.as_deref(), Some("提供元"));
    }
}
