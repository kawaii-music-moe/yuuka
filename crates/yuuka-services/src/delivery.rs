//! 朝報・日報/週報の**テスト配信**ランナー（Node `deliveryRoutes` の `/api/briefing/test`・
//! `/api/report-configs/test` が呼ぶ `runBriefingForUser` / `runReportForUser`）。
//!
//! `yuuka-briefing` の [`DeliveryRunner`] ポートを、定時配信（[`crate::briefing`] / [`crate::report`]）と
//! **同じ生成・送信関数**（`deliver`）で実装する。生成ロジックは複製しない。supervisor がこれを
//! `yuuka_briefing::routes_with` へ注入する（briefing は services に依存しない＝非循環）。
//!
//! ## スコープ
//! 渡された `user_id`/`bot_id`（ルートが [`yuuka_web::resolve_scope`] で解決した、リクエストした本人と
//! 選択中 Bot）の設定行だけを読み、その本人宛てに送る。全ユーザー横断の走査はしないため、cron 用の
//! 横断アクセス証憑（`CrossUserAccess`）も持たない。
//!
//! ## Node パリティ
//! - 朝報: 設定が無ければ未配信（`false`）。`enabled` は見ない（テストは無効化中でも送れる）。
//!   送信先は設定の `target_type`/`target_id`。
//! - 日報/週報: 設定が無くてもユーザー既定の通知先（`users.notify_target_*`）へ送る（Node は
//!   `config ? {...} : undefined` で送信先を省略し、`sendToUser` が既定の通知先へ解決する）。
//!   こちらも `enabled` は見ない。
//! - 戻り値は送信結果そのもの（Node `sendToUser` の boolean）。DB 失敗も未配信（`false`）として返す。

use std::sync::Arc;

use async_trait::async_trait;
use yuuka_briefing::{find_briefing, find_report, DeliveryRunner, EnabledBriefing, EnabledReport};
use yuuka_web::Db;

use crate::notifier::{DefaultTargetNotifier, Notifier, NotifyTarget};
use crate::{briefing, report};

/// 通知ポート（[`Notifier`]）経由でテスト配信する [`DeliveryRunner`] 実装。
pub struct NotifierDeliveryRunner {
    db: Db,
    notifier: Arc<dyn Notifier>,
}

impl NotifierDeliveryRunner {
    /// DB と通知ポート（main では Discord Messenger）を束ねて作る。
    ///
    /// 定時配信（[`crate::context::ServiceContext::new`]）と同じく `DefaultTargetNotifier` で包み、
    /// `NotifyTarget::Default` をユーザー既定の通知先へ解決してから送る。
    #[must_use]
    pub fn new(db: Db, notifier: Arc<dyn Notifier>) -> Self {
        let notifier = Arc::new(DefaultTargetNotifier::new(db.clone(), notifier));
        Self { db, notifier }
    }
}

#[async_trait]
impl DeliveryRunner for NotifierDeliveryRunner {
    async fn run_briefing(&self, user_id: &str, bot_id: &str) -> bool {
        let config = match find_briefing(&self.db, user_id, bot_id).await {
            Ok(Some(config)) => config,
            Ok(None) => return false,
            Err(e) => {
                tracing::error!(user = %user_id, error = %e, "❌ 朝報のテスト配信で設定の読み取りに失敗");
                return false;
            }
        };
        let target = EnabledBriefing {
            user_id: user_id.to_owned(),
            bot_id: bot_id.to_owned(),
            schedule_cron: config.schedule_cron,
            target_type: config.target_type,
            target_id: config.target_id,
        };
        match briefing::deliver(&self.db, self.notifier.as_ref(), &target).await {
            Ok(sent) => sent,
            Err(e) => {
                tracing::error!(user = %user_id, error = %e, "❌ 朝報のテスト配信に失敗");
                false
            }
        }
    }

    async fn run_report(&self, user_id: &str, bot_id: &str, report_type: &str) -> bool {
        let config = match find_report(&self.db, user_id, bot_id, report_type).await {
            Ok(config) => config,
            Err(e) => {
                tracing::error!(user = %user_id, error = %e, "❌ レポートのテスト配信で設定の読み取りに失敗");
                return false;
            }
        };
        // 設定が無ければ送信先を省略（Default）し、ユーザー既定の通知先へ解決させる。
        // schedule_cron は配信では使わない（cron 判定は定時配信側のみ）。
        let has_config = config.is_some();
        let (schedule_cron, target_type, target_id) = match config {
            Some(c) => (c.schedule_cron, c.target_type, c.target_id),
            None => (String::new(), String::new(), None),
        };
        let target = EnabledReport {
            user_id: user_id.to_owned(),
            bot_id: bot_id.to_owned(),
            r#type: report_type.to_owned(),
            schedule_cron,
            target_type,
            target_id,
        };
        let result = if has_config {
            report::deliver(&self.db, self.notifier.as_ref(), &target).await
        } else {
            report::deliver_to(
                &self.db,
                self.notifier.as_ref(),
                &target,
                NotifyTarget::Default,
            )
            .await
        };
        match result {
            Ok(sent) => sent,
            Err(e) => {
                tracing::error!(user = %user_id, error = %e, "❌ レポートのテスト配信に失敗");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use yuuka_briefing::DeliveryRunner;
    use yuuka_web::Db;

    use super::NotifierDeliveryRunner;
    use crate::notifier::NotifyTarget;
    use crate::test_support::RecordingNotifier;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    /// 空ファイル → `Db::open` が実スキーマ（V17 baseline）を流した DB に、2 ユーザー × 2 Bot の
    /// 配信設定を入れる（users/bots 行は作らないので FK は切って直接入れる）。
    ///
    /// - a/bot1: 朝報（無効・チャンネル chan-a1）・日報（無効・チャンネル chan-a1）
    /// - a/bot2: 設定なし
    /// - b/bot2: 朝報（有効・チャンネル chan-b2）・週報（有効・チャンネル chan-b2）
    /// - ユーザー既定の通知先: a はチャンネル chan-a-default・b は DM・c は users 行なし
    ///
    /// 天気・RSS は未設定にして外部 HTTP を発生させない（朝報本文は空案内文になる）。
    fn seed() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = dir.path().join(format!("delivery_{seq}.sqlite"));
        drop(rusqlite::Connection::open(&path).expect("empty file"));
        let db = Db::open(&path).expect("open db");
        let conn = rusqlite::Connection::open(&path).expect("seed conn");
        conn.execute_batch(
            "PRAGMA foreign_keys = OFF;
             INSERT INTO briefing_configs (user_id, bot_id, enabled, target_type, target_id)
               VALUES ('a', 'bot1', 0, 'channel', 'chan-a1'),
                      ('b', 'bot2', 1, 'channel', 'chan-b2');
             INSERT INTO report_configs (user_id, bot_id, type, enabled, target_type, target_id)
               VALUES ('a', 'bot1', 'daily', 0, 'channel', 'chan-a1'),
                      ('b', 'bot2', 'weekly', 1, 'channel', 'chan-b2');
             INSERT INTO users (discord_id, username, salt, notify_target_type, notify_target_id)
               VALUES ('a', 'a', '', 'channel', 'chan-a-default'),
                      ('b', 'b', '', 'dm', NULL);",
        )
        .expect("seed configs");
        (db, dir)
    }

    fn runner(db: Db, notifier: &Arc<RecordingNotifier>) -> NotifierDeliveryRunner {
        NotifierDeliveryRunner::new(db, notifier.clone())
    }

    #[tokio::test]
    async fn briefing_test_sends_own_config_even_if_disabled() {
        let (db, _dir) = seed();
        let notifier = Arc::new(RecordingNotifier::new(true));

        assert!(runner(db, &notifier).run_briefing("a", "bot1").await);

        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        // 本人・選択中 Bot 宛てに、その設定の送信先（チャンネル）へ。
        assert_eq!(sent[0].user_id.as_str(), "a");
        assert_eq!(sent[0].bot_id.as_str(), "bot1");
        assert_eq!(sent[0].target, NotifyTarget::Channel("chan-a1".to_owned()));
        assert!(sent[0].embeds[0]
            .title
            .as_deref()
            .is_some_and(|title| title.contains("今日の朝報です")));
    }

    #[tokio::test]
    async fn briefing_test_does_not_use_other_users_config() {
        let (db, _dir) = seed();
        let notifier = Arc::new(RecordingNotifier::new(true));

        // a/bot2 は未設定（同じ bot2 に b の設定はあるが使わない）→ 未配信。
        assert!(!runner(db, &notifier).run_briefing("a", "bot2").await);
        assert_eq!(notifier.count(), 0);
    }

    #[tokio::test]
    async fn briefing_test_reports_send_failure() {
        let (db, _dir) = seed();
        // 配信先（Discord）が送れない → false をそのまま返す。
        let notifier = Arc::new(RecordingNotifier::new(false));

        assert!(!runner(db, &notifier).run_briefing("a", "bot1").await);
        assert_eq!(notifier.count(), 1);
    }

    #[tokio::test]
    async fn report_test_sends_own_config() {
        let (db, _dir) = seed();
        let notifier = Arc::new(RecordingNotifier::new(true));

        assert!(runner(db, &notifier).run_report("a", "bot1", "daily").await);

        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].user_id.as_str(), "a");
        assert_eq!(sent[0].bot_id.as_str(), "bot1");
        assert_eq!(sent[0].target, NotifyTarget::Channel("chan-a1".to_owned()));
        assert!(sent[0].embeds[0]
            .title
            .as_deref()
            .is_some_and(|title| title.contains("📋 日報")));
    }

    #[tokio::test]
    async fn report_test_without_own_config_goes_to_default_target() {
        let (db, _dir) = seed();
        let notifier = Arc::new(RecordingNotifier::new(true));

        // a/bot2 の週報は未設定（b/bot2 の週報はチャンネル指定だが使わない）→ a の既定の通知先
        // （チャンネル chan-a-default）。明示 DM にはしない。
        assert!(
            runner(db, &notifier)
                .run_report("a", "bot2", "weekly")
                .await
        );

        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].user_id.as_str(), "a");
        assert_eq!(sent[0].bot_id.as_str(), "bot2");
        assert_eq!(
            sent[0].target,
            NotifyTarget::Channel("chan-a-default".to_owned())
        );
        assert!(sent[0].embeds[0]
            .title
            .as_deref()
            .is_some_and(|title| title.contains("📋 週報")));
    }

    #[tokio::test]
    async fn report_test_without_config_falls_back_to_dm_by_user_setting() {
        let (db, _dir) = seed();
        let notifier = Arc::new(RecordingNotifier::new(true));
        let runner = runner(db, &notifier);

        // b は既定の通知先が DM・c は users 行が無い → どちらも DM。
        assert!(runner.run_report("b", "bot1", "daily").await);
        assert!(runner.run_report("c", "bot1", "daily").await);

        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].user_id.as_str(), "b");
        assert_eq!(sent[0].target, NotifyTarget::Dm);
        assert_eq!(sent[1].user_id.as_str(), "c");
        assert_eq!(sent[1].target, NotifyTarget::Dm);
    }

    #[tokio::test]
    async fn run_briefing_now_deliver_uses_same_route_as_test_delivery() {
        use yuuka_core::{BotId, ToolContext, UserId};

        // issue #136: runBriefingNow（deliver: true）は /api/briefing/test と同じ runner で、
        // 呼び出した本人・Bot の設定した配信先へ送る（会話へのインライン表示ではない）。
        let (db, _dir) = seed();
        let notifier = Arc::new(RecordingNotifier::new(true));
        let tools =
            yuuka_briefing::tools_with(db.clone(), Arc::new(runner(db, &notifier))).expect("tools");
        let run = tools
            .iter()
            .find(|t| t.declaration().name.as_str() == "runBriefingNow")
            .expect("runBriefingNow");
        let ctx = ToolContext::new(BotId::new("bot1"), UserId::new("a"));

        let out = run
            .call(&ctx, serde_json::json!({"deliver": true}))
            .await
            .expect("call");

        assert_eq!(out.payload["success"], true);
        assert!(out.parts.is_empty());
        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].user_id.as_str(), "a");
        assert_eq!(sent[0].bot_id.as_str(), "bot1");
        assert_eq!(sent[0].target, NotifyTarget::Channel("chan-a1".to_owned()));
        assert!(sent[0].embeds[0]
            .title
            .as_deref()
            .is_some_and(|title| title.contains("今日の朝報です")));
    }
}
