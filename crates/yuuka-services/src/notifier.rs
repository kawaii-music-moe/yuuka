//! サービスからユーザーへの通知配信ポート（現行 `src/services/notifier.ts` `sendToUser`）。
//!
//! **本 crate が所有するポート**にすることで services は discord に依存しない（DAG: `services →
//! core`）。実配信は supervisor が Discord の Messenger をこの trait へアダプトして注入する
//! （gemini `GenerateBackend` / web `AuthBackend` と同じ規律）。discord 未起動時は [`NullNotifier`]
//! が「配信不可＝ `false`」を返し、呼び出し側は現行同様に未送信として次 tick で再試行する
//! （リマインドは pending のまま・誕生日は未マークのまま）。

use async_trait::async_trait;
use yuuka_core::{BotId, UserId};

/// 送信先（現行 `NotifyTarget`）。`Default` は「ユーザー既定送信先 → DM」を配信側で解決する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotifyTarget {
    /// ユーザー設定の既定送信先（無ければ DM）。
    Default,
    /// 明示チャンネル ID。
    Channel(String),
}

/// 1 通の通知（現行 `NotifyPayload` の text 経路）。埋め込み/ファイルは report/briefing 実装時に拡張する。
#[derive(Debug, Clone)]
pub struct Notification {
    /// 宛先ユーザー。
    pub user_id: UserId,
    /// 配信元 Bot（秘書業務データの Bot 別分離・本人クライアントで届ける）。
    pub bot_id: BotId,
    /// 本文（空なら配信しない）。
    pub content: String,
    /// 送信先の解決方針。
    pub target: NotifyTarget,
}

impl Notification {
    /// DM/既定送信先への text 通知を作る。
    #[must_use]
    pub fn text(user_id: UserId, bot_id: BotId, content: impl Into<String>) -> Self {
        Self {
            user_id,
            bot_id,
            content: content.into(),
            target: NotifyTarget::Default,
        }
    }

    /// 送信先を差し替える（reminder のチャンネル指定用）。
    #[must_use]
    pub fn with_target(mut self, target: NotifyTarget) -> Self {
        self.target = target;
        self
    }
}

/// ユーザー通知配信（現行 `sendToUser`）。送信成功なら `true`。
#[async_trait]
pub trait Notifier: Send + Sync {
    /// 1 通配信する。空本文・利用可能クライアント無し・送信失敗は `false`（呼び出し側で再試行）。
    async fn send(&self, notification: Notification) -> bool;
}

/// discord 未配線時の縮退実装。配信せず `false` を返し、その旨を debug ログに残す。
///
/// 現行 notifier が「利用可能な Bot クライアントが無い → `false`」を返すのと同じ意味論。
/// これにより reminder/birthday は未送信として次 tick で再試行され、データを失わない。
pub struct NullNotifier;

#[async_trait]
impl Notifier for NullNotifier {
    async fn send(&self, notification: Notification) -> bool {
        tracing::debug!(
            user_id = %notification.user_id,
            bot_id = %notification.bot_id,
            "通知配信先（Discord）未配線のため送信をスキップ（次 tick で再試行）"
        );
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::RecordingNotifier;

    #[tokio::test]
    async fn null_notifier_reports_undelivered() {
        let n = NullNotifier;
        let ok = n
            .send(Notification::text(
                UserId::new("u"),
                BotId::new("system_default"),
                "hi",
            ))
            .await;
        assert!(!ok);
    }

    #[tokio::test]
    async fn recording_notifier_captures_content_and_target() {
        let rec = RecordingNotifier::new(true);
        let ok = rec
            .send(
                Notification::text(UserId::new("u"), BotId::new("b"), "x")
                    .with_target(NotifyTarget::Channel("c1".to_owned())),
            )
            .await;
        assert!(ok);
        let sent = rec.sent.lock().expect("lock");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].content, "x");
        assert_eq!(sent[0].target, NotifyTarget::Channel("c1".to_owned()));
    }
}
