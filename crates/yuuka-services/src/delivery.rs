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
//! - 日報/週報: 設定が無くても既定送信先（→DM）へ送る（Node は `config ? {...} : undefined`）。
//!   こちらも `enabled` は見ない。
//! - 戻り値は送信結果そのもの（Node `sendToUser` の boolean）。DB 失敗も未配信（`false`）として返す。

use std::sync::Arc;

use async_trait::async_trait;
use yuuka_briefing::{find_briefing, find_report, DeliveryRunner, EnabledBriefing, EnabledReport};
use yuuka_web::Db;

use crate::notifier::Notifier;
use crate::{briefing, report};

/// 通知ポート（[`Notifier`]）経由でテスト配信する [`DeliveryRunner`] 実装。
pub struct NotifierDeliveryRunner {
    db: Db,
    notifier: Arc<dyn Notifier>,
}

impl NotifierDeliveryRunner {
    /// DB と通知ポート（main では Discord Messenger）を束ねて作る。
    #[must_use]
    pub fn new(db: Db, notifier: Arc<dyn Notifier>) -> Self {
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
        // 設定が無ければ既定送信先（→DM）。schedule_cron は配信では使わない（cron 判定は定時配信側のみ）。
        let (schedule_cron, target_type, target_id) = match config {
            Some(c) => (c.schedule_cron, c.target_type, c.target_id),
            None => (String::new(), "dm".to_owned(), None),
        };
        let target = EnabledReport {
            user_id: user_id.to_owned(),
            bot_id: bot_id.to_owned(),
            r#type: report_type.to_owned(),
            schedule_cron,
            target_type,
            target_id,
        };
        match report::deliver(&self.db, self.notifier.as_ref(), &target).await {
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
                      ('b', 'bot2', 'weekly', 1, 'channel', 'chan-b2');",
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
        assert!(sent[0].content.contains("今日の朝報です"));
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
        assert!(sent[0].content.contains("📋 日報"));
    }

    #[tokio::test]
    async fn report_test_without_own_config_goes_to_default_target() {
        let (db, _dir) = seed();
        let notifier = Arc::new(RecordingNotifier::new(true));

        // a/bot2 の週報は未設定（b/bot2 の週報はチャンネル指定だが使わない）→ 本人の既定送信先（→DM）。
        assert!(
            runner(db, &notifier)
                .run_report("a", "bot2", "weekly")
                .await
        );

        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].user_id.as_str(), "a");
        assert_eq!(sent[0].bot_id.as_str(), "bot2");
        assert_eq!(sent[0].target, NotifyTarget::Default);
        assert!(sent[0].content.contains("📋 週報"));
    }
}
