//! Webhook 受信処理（[`yuuka_webhook::RealWebhookProcessor`]）のポートを Discord 配信・補助 LLM へ繋ぐ
//! 合成ルートアダプタ。
//!
//! `yuuka-webhook` は discord/orchestrator/gemini に依存しないため、双方を知る supervisor で橋渡しする
//! （[`crate::MessengerRegistrationDm`] と同じ規律）。

use std::sync::Arc;

use async_trait::async_trait;
use yuuka_core::secrets::wrap_secret;
use yuuka_core::{BotId, UserId};
use yuuka_crypto::SystemCrypto;
use yuuka_discord::{DeliverTarget, DiscordMessenger, Notifier, RichEmbed, TurnReply};
use yuuka_gemini::{Content, GeminiClient};
use yuuka_web::Db;
use yuuka_webhook::{AuxTextGenerator, WebhookNotice, WebhookNotifier, WebhookTarget};

/// Webhook 通知を [`DiscordMessenger`] で Embed として届ける（Node `sendToUser`）。
///
/// Webhook は全 Bot 共有の受信口なので、生成する ToDo/リマインドと同じく既定 Bot（`system_default`）から
/// 送る。チャンネル宛が解決できない・本人が閲覧できない場合の DM フォールバックは messenger が担う。
pub struct MessengerWebhookNotifier {
    messenger: Arc<DiscordMessenger>,
}

impl MessengerWebhookNotifier {
    /// messenger を注入して構築する。
    #[must_use]
    pub fn new(messenger: Arc<DiscordMessenger>) -> Self {
        Self { messenger }
    }
}

#[async_trait]
impl WebhookNotifier for MessengerWebhookNotifier {
    async fn notify(&self, user_id: &str, notice: WebhookNotice) -> bool {
        let target = match notice.target {
            WebhookTarget::Dm => DeliverTarget::Dm,
            WebhookTarget::Channel(id) => DeliverTarget::Channel(id),
        };
        let reply = TurnReply {
            embeds: vec![RichEmbed {
                title: Some(notice.title),
                description: Some(notice.description),
                color: Some(notice.color),
                fields: Vec::new(),
                footer: Some(notice.footer),
            }],
            ..TurnReply::default()
        };
        Notifier::send_to_user(
            self.messenger.as_ref(),
            &UserId::new(user_id),
            reply,
            target,
            &BotId::system_default(),
        )
        .await
    }
}

/// ユーザー本人の Gemini キーで補助生成する（Node `generateAuxText`・ツール無し）。
///
/// キー未設定・暗号鍵未設定・復号失敗・生成失敗はいずれも `None`（受信処理はフォールバック整形へ縮退）。
pub struct GeminiAuxText {
    db: Db,
    crypto: Option<Arc<SystemCrypto>>,
}

impl GeminiAuxText {
    /// DB（キーの読み出し）と crypto（復号）を注入して構築する。
    #[must_use]
    pub fn new(db: Db, crypto: Option<Arc<SystemCrypto>>) -> Self {
        Self { db, crypto }
    }
}

#[async_trait]
impl AuxTextGenerator for GeminiAuxText {
    async fn generate_aux_text(&self, user_id: &str, prompt: &str) -> Option<String> {
        let cfg = match yuuka_orchestrator::user::user_gemini(&self.db, user_id).await {
            Ok(Some(cfg)) => cfg,
            Ok(None) => return None,
            Err(e) => {
                tracing::warn!(error = %e, "補助生成: Gemini 設定の読み出しに失敗しました");
                return None;
            }
        };
        let crypto = self.crypto.as_ref()?;
        let key = match crypto.decrypt_text(&cfg.encrypted, &cfg.iv, &cfg.tag) {
            Ok(key) => key,
            Err(e) => {
                tracing::warn!(error = %e, "補助生成: Gemini キーの復号に失敗しました");
                return None;
            }
        };
        let client = match GeminiClient::new(cfg.model, wrap_secret(key)) {
            Ok(client) => client,
            Err(e) => {
                tracing::warn!(error = %e, "補助生成: Gemini クライアントを構築できません");
                return None;
            }
        };
        let resp = client
            .generate_aux(None, vec![Content::user_text(prompt)], None)
            .await?;
        Some(resp.text())
    }
}
