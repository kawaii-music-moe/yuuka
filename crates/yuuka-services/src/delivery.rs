//! 定期報告の**テスト配信**ランナー（`/api/periodic-reports/test` と `runPeriodicReportNow`
//! の `deliver: true` が呼ぶ）。
//!
//! `yuuka-briefing` の [`DeliveryRunner`] ポートを、定時配信（[`crate::periodic_report`]）と
//! **同じ生成・送信関数**（`deliver`）で実装する。生成ロジックは複製しない。supervisor がこれを
//! `yuuka_briefing::routes_with` へ注入する（briefing は services に依存しない＝非循環）。
//!
//! ## スコープ
//! 渡された `user_id`/`bot_id`（ルートが [`yuuka_web::resolve_scope`] で解決した、リクエストした本人と
//! 選択中 Bot）の定期報告だけを読み、その本人（または設定のチャンネル）へ送る。他人の定期報告の id は
//! 無いものとして扱う。全ユーザー横断の走査はしないため、cron 用の横断アクセス証憑も持たない。
//! `enabled` は見ない（無効にしている報告もテストできる）。戻り値は送信結果で、DB 失敗も未配信（`false`）。

use std::sync::Arc;

use async_trait::async_trait;
use yuuka_briefing::{find_report, DeliveryRunner};
use yuuka_core::aux_text::{AuxTextGenerator, NullAuxTextGenerator};
use yuuka_web::Db;

use crate::notifier::{DefaultTargetNotifier, Notifier};
use crate::periodic_report;

/// 通知ポート（[`Notifier`]）経由でテスト配信する [`DeliveryRunner`] 実装。
pub struct NotifierDeliveryRunner {
    db: Db,
    notifier: Arc<dyn Notifier>,
    aux_text: Arc<dyn AuxTextGenerator>,
}

impl NotifierDeliveryRunner {
    /// DB と通知ポート（main では Discord Messenger）を束ねて作る。
    ///
    /// 定時配信（[`crate::context::ServiceContext::new`]）と同じく `DefaultTargetNotifier` で包む。
    #[must_use]
    pub fn new(db: Db, notifier: Arc<dyn Notifier>) -> Self {
        let notifier = Arc::new(DefaultTargetNotifier::new(db.clone(), notifier));
        Self {
            db,
            notifier,
            aux_text: Arc::new(NullAuxTextGenerator),
        }
    }

    /// 定時配信と同じ補助テキスト生成ポート（ニュース要約・活動データのまとめ）を注入する。
    #[must_use]
    pub fn with_aux_text(mut self, aux_text: Arc<dyn AuxTextGenerator>) -> Self {
        self.aux_text = aux_text;
        self
    }
}

#[async_trait]
impl DeliveryRunner for NotifierDeliveryRunner {
    async fn run_report(&self, user_id: &str, bot_id: &str, report_id: i64) -> bool {
        let report = match find_report(&self.db, user_id, bot_id, report_id).await {
            Ok(Some(report)) => report,
            Ok(None) => return false,
            Err(e) => {
                tracing::error!(user = %user_id, error = %e, "❌ 定期報告のテスト配信で設定の読み取りに失敗");
                return false;
            }
        };
        match periodic_report::deliver(
            &self.db,
            self.notifier.as_ref(),
            self.aux_text.as_ref(),
            &report,
        )
        .await
        {
            Ok(sent) => sent,
            Err(e) => {
                tracing::error!(user = %user_id, error = %e, "❌ 定期報告のテスト配信に失敗");
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

    /// 空ファイル → `Db::open` が実スキーマを流した DB に、2 ユーザーの定期報告を入れる
    /// （bots 行は作らないので FK は切って直接入れる）。
    ///
    /// - id 1: a/bot1「お知らせ」（無効・会話トピックだけ・チャンネル chan-a1）
    /// - id 2: b/bot2「日報」（有効・タスク・DM）
    ///
    /// 天気・RSS は選ばず外部 HTTP を発生させない。
    fn seed() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = dir.path().join(format!("delivery_{seq}.sqlite"));
        drop(rusqlite::Connection::open(&path).expect("empty file"));
        let db = Db::open(&path).expect("open db");
        let conn = rusqlite::Connection::open(&path).expect("seed conn");
        conn.execute_batch(
            r#"PRAGMA foreign_keys = OFF;
             INSERT INTO users (discord_id, username, salt) VALUES ('a', 'a', ''), ('b', 'b', '');
             INSERT INTO periodic_reports
               (id, user_id, bot_id, name, enabled, schedule_cron, target_type, target_id, sections)
               VALUES (1, 'a', 'bot1', 'お知らせ', 0, '0 7 * * *', 'channel', 'chan-a1', '["topics"]'),
                      (2, 'b', 'bot2', '日報', 1, '0 22 * * *', 'dm', NULL, '["tasks"]');"#,
        )
        .expect("seed reports");
        (db, dir)
    }

    fn runner(db: Db, notifier: &Arc<RecordingNotifier>) -> NotifierDeliveryRunner {
        NotifierDeliveryRunner::new(db, notifier.clone())
    }

    #[tokio::test]
    async fn sends_own_report_to_its_target_even_if_disabled() {
        let (db, _dir) = seed();
        let notifier = Arc::new(RecordingNotifier::new(true));

        assert!(
            runner(db.clone(), &notifier)
                .run_report("a", "bot1", 1)
                .await
        );
        assert!(runner(db, &notifier).run_report("b", "bot2", 2).await);

        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].user_id.as_str(), "a");
        assert_eq!(sent[0].bot_id.as_str(), "bot1");
        assert_eq!(sent[0].target, NotifyTarget::Channel("chan-a1".to_owned()));
        assert!(sent[0].embeds[0]
            .title
            .as_deref()
            .is_some_and(|t| t.starts_with("📋 お知らせ ")));
        assert_eq!(sent[1].user_id.as_str(), "b");
        assert_eq!(sent[1].target, NotifyTarget::Dm);
    }

    #[tokio::test]
    async fn does_not_send_other_users_report() {
        let (db, _dir) = seed();
        let notifier = Arc::new(RecordingNotifier::new(true));

        // 他人の id・別 Bot・無い id は送らない。
        assert!(
            !runner(db.clone(), &notifier)
                .run_report("a", "bot1", 2)
                .await
        );
        assert!(
            !runner(db.clone(), &notifier)
                .run_report("a", "bot2", 1)
                .await
        );
        assert!(!runner(db, &notifier).run_report("a", "bot1", 99).await);
        assert_eq!(notifier.count(), 0);
    }

    #[tokio::test]
    async fn reports_send_failure() {
        let (db, _dir) = seed();
        let notifier = Arc::new(RecordingNotifier::new(false));
        assert!(!runner(db, &notifier).run_report("a", "bot1", 1).await);
        assert_eq!(notifier.count(), 1);
    }

    #[tokio::test]
    async fn run_now_deliver_uses_same_route_as_test_delivery() {
        use yuuka_core::{BotId, ToolContext, UserId};

        // issue #136: runPeriodicReportNow（deliver: true）は /api/periodic-reports/test と同じ runner で、
        // 呼び出した本人・Bot の設定した配信先へ送る（会話へのインライン表示ではない）。
        let (db, _dir) = seed();
        let notifier = Arc::new(RecordingNotifier::new(true));
        let tools =
            yuuka_briefing::tools_with(db.clone(), Arc::new(runner(db, &notifier))).expect("tools");
        let run = tools
            .iter()
            .find(|t| t.declaration().name.as_str() == "runPeriodicReportNow")
            .expect("runPeriodicReportNow");
        let ctx = ToolContext::new(BotId::new("bot1"), UserId::new("a"));

        let out = run
            .call(
                &ctx,
                serde_json::json!({"name": "お知らせ", "deliver": true}),
            )
            .await
            .expect("call");

        assert_eq!(out.payload["success"], true);
        assert!(out.parts.is_empty());
        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].user_id.as_str(), "a");
        assert_eq!(sent[0].target, NotifyTarget::Channel("chan-a1".to_owned()));
    }
}
