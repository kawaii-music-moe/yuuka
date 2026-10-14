//! サービスからユーザーへの通知配信ポート（現行 `src/services/notifier.ts` `sendToUser`）。
//!
//! **本 crate が所有するポート**にすることで services は discord に依存しない（DAG: `services →
//! core`）。実配信は supervisor が Discord の Messenger をこの trait へアダプトして注入する
//! （gemini `GenerateBackend` / web `AuthBackend` と同じ規律）。discord 未起動時は [`NullNotifier`]
//! が「配信不可＝ `false`」を返し、呼び出し側は現行同様に未送信として次 tick で再試行する
//! （リマインドは pending のまま・誕生日は未マークのまま）。

use std::sync::Arc;

use async_trait::async_trait;
use yuuka_core::{BotId, UserId};
use yuuka_reminder::notify_target::{user_notify_target, UserNotifyTarget};
use yuuka_web::Db;

/// 送信先（現行 `NotifyTarget`）。`Default` は `ServiceContext` の notifier
/// （`DefaultTargetNotifier`）がユーザー既定送信先へ解決する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotifyTarget {
    /// ユーザー設定の既定送信先（無ければ DM。現行 `sendToUser` の target 省略）。
    Default,
    /// DM（明示指定。ユーザー既定送信先は見ない・現行 `{ type: "dm" }`）。
    Dm,
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

/// [`NotifyTarget::Default`] をユーザー設定の既定送信先（`users.notify_target_*`）へ解決してから
/// 下位の notifier へ渡す（現行 notifier.ts `sendToUser` の送信先解決）。
///
/// [`crate::context::ServiceContext::new`] が注入された notifier をこれで包むため、各サービスは送信先を
/// 省略（[`Notification::text`]）するだけでユーザー設定に従う。設定の読み出しに失敗した場合は現行同様
/// DM へ倒し、通知自体は止めない。
pub(crate) struct DefaultTargetNotifier {
    db: Db,
    inner: Arc<dyn Notifier>,
}

impl DefaultTargetNotifier {
    /// `inner` を包む。
    #[must_use]
    pub(crate) fn new(db: Db, inner: Arc<dyn Notifier>) -> Self {
        Self { db, inner }
    }
}

#[async_trait]
impl Notifier for DefaultTargetNotifier {
    async fn send(&self, mut notification: Notification) -> bool {
        if notification.target == NotifyTarget::Default {
            notification.target =
                match user_notify_target(&self.db, notification.user_id.as_str()).await {
                    Ok(UserNotifyTarget::Channel(id)) => NotifyTarget::Channel(id),
                    Ok(UserNotifyTarget::Dm) => NotifyTarget::Dm,
                    Err(e) => {
                        tracing::warn!(
                            user_id = %notification.user_id,
                            error = %e,
                            "既定送信先を取得できないため DM へフォールバック"
                        );
                        NotifyTarget::Dm
                    }
                };
        }
        self.inner.send(notification).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{seeded_db, RecordingNotifier};
    use yuuka_db::map_sqlite;

    /// `seeded_db` のマイグレーションが作る users 表へ既定送信先を書く。
    async fn insert_user(db: &Db, user: &str, kind: &str, id: Option<&str>) {
        let (user, kind, id) = (user.to_owned(), kind.to_owned(), id.map(str::to_owned));
        db.writer
            .execute(move |conn| {
                conn.execute(
                    "INSERT INTO users \
                       (discord_id, username, password_hash, salt, notify_target_type, notify_target_id) \
                     VALUES (?1, ?1, 'x', 'x', ?2, ?3)",
                    rusqlite::params![user, kind, id],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();
    }

    async fn resolved_target(db: Db, target: NotifyTarget, user: &str) -> NotifyTarget {
        let rec = Arc::new(RecordingNotifier::new(true));
        let notifier = DefaultTargetNotifier::new(db, rec.clone());
        let ok = notifier
            .send(
                Notification::text(UserId::new(user), BotId::new("system_default"), "x")
                    .with_target(target),
            )
            .await;
        assert!(ok);
        let sent = rec.sent.lock().expect("lock");
        sent[0].target.clone()
    }

    #[tokio::test]
    async fn default_target_follows_user_setting() {
        let (db, _dir) = seeded_db("");
        insert_user(&db, "chan", "channel", Some("c-default")).await;
        insert_user(&db, "dm", "dm", None).await;

        assert_eq!(
            resolved_target(db.clone(), NotifyTarget::Default, "chan").await,
            NotifyTarget::Channel("c-default".to_owned())
        );
        assert_eq!(
            resolved_target(db.clone(), NotifyTarget::Default, "dm").await,
            NotifyTarget::Dm
        );
        // users 行が無いユーザーは DM。
        assert_eq!(
            resolved_target(db, NotifyTarget::Default, "nobody").await,
            NotifyTarget::Dm
        );
    }

    #[tokio::test]
    async fn explicit_targets_ignore_user_setting() {
        let (db, _dir) = seeded_db("");
        insert_user(&db, "chan", "channel", Some("c-default")).await;

        assert_eq!(
            resolved_target(db.clone(), NotifyTarget::Dm, "chan").await,
            NotifyTarget::Dm
        );
        assert_eq!(
            resolved_target(db, NotifyTarget::Channel("c1".to_owned()), "chan").await,
            NotifyTarget::Channel("c1".to_owned())
        );
    }

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
