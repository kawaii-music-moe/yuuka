//! 定期報告の定時配信サービス（旧 朝報 `briefingService.ts`・日報/週報 `reportService.ts` の統合）。
//!
//! 毎分発火し、有効な全ユーザーの定期報告（`periodic_reports`）を走査して、`schedule_cron` が現在分に
//! 一致するものを [`yuuka_briefing::build_report_embed`] で生成して配信する。
//!
//! ## 二重発火ガード（last_run 不要な理由）
//! [`Schedule::EveryMinute`] は [`crate::schedule::run_cron`] で **分境界に整列**して sleep し、逐次
//! await ループのため 1 分内に tick はちょうど 1 回しか走らない。かつ `run_on_start=false` で起動直後の
//! 即時 tick を行わない（プロセス再起動が同一分内に起きても二重配信しない）。よって
//! [`cron_util::cron_matches_now`] 単体で「その分に 1 回だけ配信」が成立する（旧朝報・日報と同じ規律）。
//!
//! ## 配信先
//! 設定のチャンネル指定は、非公開の内容（タスク・予定・支払い予定・家計）を含まないときだけ使う。
//! それ以外は登録した本人への DM（[`yuuka_briefing::ReportSettings::channel_target`]）。

use async_trait::async_trait;
use chrono::Local;
use yuuka_briefing::{build_report_embed, list_enabled_reports, PeriodicReport};
use yuuka_core::aux_text::AuxTextGenerator;
use yuuka_core::{cron_util, BotId, DbError, UserId};
use yuuka_web::Db;

use crate::context::ServiceContext;
use crate::notifier::{Notification, Notifier, NotifyTarget};
use crate::schedule::{CronService, Schedule};

/// 定期報告の定時配信サービス。
pub struct PeriodicReportService;

#[async_trait]
impl CronService for PeriodicReportService {
    fn name(&self) -> &'static str {
        "periodic_report"
    }

    fn schedule(&self) -> Schedule {
        Schedule::EveryMinute
    }

    fn run_on_start(&self) -> bool {
        // last_run を持たず「その分に一致したら配信」判定のため、起動時即実行は同一分内の
        // 二重配信を招きうる。
        false
    }

    async fn tick(&self, ctx: &ServiceContext) {
        if let Err(e) = process_due_reports(ctx).await {
            tracing::error!(error = %e, "❌ 定期報告の走査でエラー");
        }
    }
}

/// 有効な定期報告を走査し、現在分に一致するものを配信する。
async fn process_due_reports(ctx: &ServiceContext) -> Result<(), DbError> {
    let reports = list_enabled_reports(&ctx.db, ctx.cross).await?;
    let now = Local::now();
    for report in reports {
        if !cron_util::cron_matches_now(&report.settings.schedule_cron, now) {
            continue;
        }
        tracing::info!(user = %report.user_id, report = report.id, name = %report.settings.name,
            "📋 定期報告を配信します");
        if let Err(e) = deliver(
            &ctx.db,
            ctx.notifier.as_ref(),
            ctx.aux_text.as_ref(),
            &report,
        )
        .await
        {
            // 1 件の DB エラーで tick 全体を止めない（次の報告・次 tick で復帰）。
            tracing::error!(user = %report.user_id, report = report.id, error = %e,
                "❌ 定期報告の配信に失敗");
        }
    }
    Ok(())
}

/// 1 件の定期報告を生成して配信先へ送る。送信できたら `true`。
///
/// 定時配信（[`process_due_reports`]）とテスト配信（[`crate::delivery::NotifierDeliveryRunner`]）の
/// 共通経路。`report` の `user_id`/`bot_id` のデータだけを集め、その本人（または設定のチャンネル）へ送る。
pub(crate) async fn deliver(
    db: &Db,
    notifier: &dyn Notifier,
    aux_text: &dyn AuxTextGenerator,
    report: &PeriodicReport,
) -> Result<bool, DbError> {
    let embed = build_report_embed(db, aux_text, report, Local::now()).await?;
    let target = match report.settings.channel_target() {
        Some(id) => NotifyTarget::Channel(id.to_owned()),
        None => NotifyTarget::Dm,
    };
    let notification = Notification::embed(
        UserId::new(report.user_id.clone()),
        BotId::new(report.bot_id.clone()),
        embed,
    )
    .with_target(target);

    // 配信不可（Discord 未配線 = NullNotifier / クライアント無し）は false。last_run を持たず
    // 「この分だけ配信」のため、失敗しても再スケジュールはしない。
    let sent = notifier.send(notification).await;
    if !sent {
        tracing::warn!(user = %report.user_id, report = report.id,
            "⚠️ 定期報告の通知に失敗（配信先未配線などで未送信）");
    }
    Ok(sent)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::{Datelike, Timelike};
    use rusqlite::params;
    use yuuka_core::datetime;
    use yuuka_db::map_sqlite;
    use yuuka_web::Db;

    use super::*;
    use crate::test_support::{ctx_with, seeded_db, RecordingNotifier};

    /// 集計元テーブル（todos/schedules/planned_payments/expenses）を FK 無しで作る（各ドメイン crate の
    /// テスト DDL と同形）。定期報告・利用者・会話ログは実マイグレーションのスキーマをそのまま使う。
    const DDL: &str = "\
        DROP TABLE IF EXISTS todos; CREATE TABLE todos (list TEXT NOT NULL DEFAULT '個人', \
            id INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL, \
            bot_id TEXT NOT NULL, title TEXT NOT NULL, description TEXT, \
            due_date TEXT, start_date TEXT, priority TEXT, tags TEXT NOT NULL DEFAULT '[]', \
            status TEXT NOT NULL DEFAULT 'open', progress INTEGER NOT NULL DEFAULT 0, parent_id INTEGER, \
            linked_payment_id INTEGER, due_reminded INTEGER NOT NULL DEFAULT 0, repeat_rule TEXT, \
            repeat_until TEXT, repeat_count INTEGER, \
            created_at TEXT NOT NULL DEFAULT (datetime('now','localtime')), \
            updated_at TEXT NOT NULL DEFAULT (datetime('now','localtime')));\
        DROP TABLE IF EXISTS schedules; CREATE TABLE schedules (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL, \
            bot_id TEXT NOT NULL, title TEXT NOT NULL, description TEXT, \
            start_at TEXT NOT NULL, end_at TEXT, remind_before_minutes INTEGER NOT NULL DEFAULT 10, \
            reminded INTEGER NOT NULL DEFAULT 0, google_event_id TEXT, google_calendar_id TEXT, \
            created_at TEXT NOT NULL DEFAULT (datetime('now','localtime')));\
        DROP TABLE IF EXISTS planned_payments; CREATE TABLE planned_payments (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL, title TEXT NOT NULL, \
            amount INTEGER NOT NULL, category TEXT, memo TEXT, due_date TEXT NOT NULL, repeat_rule TEXT, \
            status TEXT NOT NULL DEFAULT 'pending', \
            created_at TEXT NOT NULL DEFAULT (datetime('now','localtime')), \
            updated_at TEXT NOT NULL DEFAULT (datetime('now','localtime')), \
            bot_id TEXT NOT NULL);\
        DROP TABLE IF EXISTS expenses; CREATE TABLE expenses (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL, \
            bot_id TEXT NOT NULL, type TEXT NOT NULL DEFAULT 'expense', \
            amount INTEGER NOT NULL, category TEXT NOT NULL, memo TEXT, date TEXT NOT NULL, time TEXT, \
            source TEXT NOT NULL DEFAULT 'manual', \
            created_at TEXT NOT NULL DEFAULT (datetime('now','localtime')));";

    /// 毎年 1/1 の式。前回〜次回が 1 年にまたがるため、「今」の前後に入れた行は必ず期間内になる。
    const YEARLY: &str = "0 0 1 1 *";

    /// 利用者 u と、指定の定期報告 1 件を入れた DB を作る（チャンネル指定は保存時の検証を通さず直接入れる）。
    async fn seed(
        enabled: bool,
        cron: &'static str,
        sections: &'static str,
        target: Option<&'static str>,
    ) -> (Db, tempfile::TempDir) {
        let (db, dir) = seeded_db(DDL);
        db.writer
            .execute(move |conn| {
                conn.execute(
                    "INSERT INTO users (discord_id, username, salt) VALUES ('u', 'u', 'x')",
                    [],
                )
                .map_err(map_sqlite)?;
                conn.execute(
                    "INSERT INTO periodic_reports \
                       (user_id, bot_id, name, enabled, schedule_cron, target_type, target_id, sections) \
                     VALUES ('u', 'system_default', '日報', ?1, ?2, ?3, ?4, ?5)",
                    params![
                        i64::from(enabled),
                        cron,
                        if target.is_some() { "channel" } else { "dm" },
                        target,
                        sections,
                    ],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();
        (db, dir)
    }

    async fn only_report(db: &Db) -> PeriodicReport {
        list_enabled_reports(db, yuuka_core::CrossUserAccess::for_scheduled_task())
            .await
            .unwrap()
            .remove(0)
    }

    async fn exec(db: &Db, sql: &'static str) {
        db.writer
            .execute(move |conn| {
                conn.execute_batch(sql).map_err(map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();
    }

    const ALL_ACTIVITY: &str = r#"["tasks","schedules","payments","finance","topics"]"#;

    #[tokio::test]
    async fn due_report_delivers_embed_to_dm() {
        // "* * * * *" は常に現在分にマッチ → 配信される。
        let (db, _dir) = seed(true, "* * * * *", ALL_ACTIVITY, None).await;
        let notifier = Arc::new(RecordingNotifier::new(true));
        let ctx = ctx_with(db, notifier.clone());

        PeriodicReportService.tick(&ctx).await;

        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].target, NotifyTarget::Dm);
        assert!(sent[0].content.is_empty());
        let embed = &sent[0].embeds[0];
        assert!(embed
            .title
            .as_deref()
            .is_some_and(|title| title.starts_with("📋 日報 ")));
        assert_eq!(embed.color, yuuka_briefing::REPORT_COLOR);
        assert_eq!(embed.footer.as_deref(), Some("Yuuka レポート（生データ）"));
        // 集計元が空でも生データ本文（完了/持ち越し/収支）は出る。
        let body = embed.description.as_deref().unwrap_or_default();
        assert!(body.contains("✅ 完了タスク: 0件"), "{body}");
        assert!(body.contains("💰 収支: 収入 ¥0 / 支出 ¥0"), "{body}");
        let fields: Vec<(&str, &str)> = embed
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.value.as_str()))
            .collect();
        assert_eq!(
            fields,
            [
                ("✅ 完了", "0件"),
                ("📌 持ち越し", "0件"),
                ("💰 収支", "+¥0 / -¥0")
            ]
        );
        assert!(embed.fields.iter().all(|field| field.inline));
    }

    #[tokio::test]
    async fn channel_is_used_only_without_private_sections() {
        // 会話トピックだけならチャンネルへ。
        let (db, _dir) = seed(true, "* * * * *", r#"["topics"]"#, Some("chan-1")).await;
        let notifier = Arc::new(RecordingNotifier::new(true));
        PeriodicReportService
            .tick(&ctx_with(db, notifier.clone()))
            .await;
        assert_eq!(
            notifier.sent.lock().unwrap()[0].target,
            NotifyTarget::Channel("chan-1".to_owned())
        );

        // 非公開の内容を含む行がチャンネル指定で残っていても DM に送る。
        let (db, _dir) = seed(true, "* * * * *", r#"["tasks"]"#, Some("chan-1")).await;
        let notifier = Arc::new(RecordingNotifier::new(true));
        PeriodicReportService
            .tick(&ctx_with(db, notifier.clone()))
            .await;
        assert_eq!(notifier.sent.lock().unwrap()[0].target, NotifyTarget::Dm);
    }

    #[tokio::test]
    async fn non_due_and_disabled_reports_are_not_delivered() {
        let now = Local::now();
        let is_new_year_midnight =
            now.month() == 1 && now.day() == 1 && now.hour() == 0 && now.minute() == 0;
        if !is_new_year_midnight {
            // 現在分に一致しない cron（元日 0:00 のみ）は配信されない。
            let (db, _dir) = seed(true, YEARLY, ALL_ACTIVITY, None).await;
            let notifier = Arc::new(RecordingNotifier::new(true));
            PeriodicReportService
                .tick(&ctx_with(db, notifier.clone()))
                .await;
            assert_eq!(notifier.count(), 0);
        }
        let (db, _dir) = seed(false, "* * * * *", ALL_ACTIVITY, None).await;
        let notifier = Arc::new(RecordingNotifier::new(true));
        PeriodicReportService
            .tick(&ctx_with(db, notifier.clone()))
            .await;
        assert_eq!(notifier.count(), 0);
    }

    #[tokio::test]
    async fn aggregates_period_activity_into_raw_body() {
        let (db, _dir) = seed(true, YEARLY, ALL_ACTIVITY, None).await;
        exec(
            &db,
            "INSERT INTO todos (user_id, bot_id, title, status, updated_at) \
               VALUES ('u','system_default','買い物','done', datetime('now','localtime'));\
             INSERT INTO todos (user_id, bot_id, title, status, due_date) \
               VALUES ('u','system_default','レポート提出','open', date('now','localtime'));\
             INSERT INTO schedules (user_id, bot_id, title, start_at) \
               VALUES ('u','system_default','会議', datetime('now','localtime','+1 hour'));\
             INSERT INTO schedules (user_id, bot_id, title, start_at) \
               VALUES ('u','system_default','終わった会議', datetime('now','localtime','-1 hour'));\
             INSERT INTO planned_payments (user_id, bot_id, title, amount, due_date, status) \
               VALUES ('u','system_default','家賃',80000, date('now','localtime'),'pending');\
             INSERT INTO planned_payments (user_id, bot_id, title, amount, due_date, status) \
               VALUES ('u','other_bot','別Botのサブスク',980, date('now','localtime'),'pending');\
             INSERT INTO expenses (user_id, bot_id, type, amount, category, date) \
               VALUES ('u','system_default','income',300000,'salary', date('now','localtime'));\
             INSERT INTO expenses (user_id, bot_id, type, amount, category, date) \
               VALUES ('u','system_default','expense',1500,'food', date('now','localtime'));",
        )
        .await;
        let report = only_report(&db).await;
        let notifier = RecordingNotifier::new(true);
        assert!(deliver(
            &db,
            &notifier,
            &yuuka_core::aux_text::NullAuxTextGenerator,
            &report
        )
        .await
        .unwrap());

        let sent = notifier.sent.lock().unwrap();
        let body = sent[0].embeds[0].description.as_deref().unwrap_or_default();
        for expected in [
            "✅ 完了タスク: 1件",
            "  ・買い物",
            "📌 持ち越しタスク: 1件",
            "  ・レポート提出",
            "📅 これからの予定: 1件",
            "  ・会議",
            "💳 支払い予定:",
            "  ・家賃 ¥80,000 [未払い]",
            "💰 収支: 収入 ¥300,000 / 支出 ¥1,500",
        ] {
            assert!(body.contains(expected), "missing {expected}: {body}");
        }
        // 過ぎた予定・別 Bot の支払い予定は載せない。
        assert!(!body.contains("終わった会議"), "{body}");
        assert!(!body.contains("別Botのサブスク"), "{body}");
    }

    #[derive(Default)]
    struct RecordingGenerator {
        response: Option<String>,
        calls: std::sync::Mutex<Vec<(String, String)>>,
    }

    #[async_trait]
    impl AuxTextGenerator for RecordingGenerator {
        async fn generate_aux_text(&self, user_id: &str, prompt: &str) -> Option<String> {
            self.calls
                .lock()
                .unwrap()
                .push((user_id.to_owned(), prompt.to_owned()));
            self.response.clone()
        }
    }

    #[tokio::test]
    async fn summary_is_used_by_scheduled_and_manual_delivery() {
        use yuuka_briefing::DeliveryRunner;
        let (db, _dir) = seed(true, "* * * * *", ALL_ACTIVITY, None).await;
        let generator = Arc::new(RecordingGenerator {
            response: Some("  今週のまとめ。\n".to_owned()),
            ..Default::default()
        });
        let notifier = Arc::new(RecordingNotifier::new(true));
        let ctx = ctx_with(db.clone(), notifier.clone()).with_aux_text(generator.clone());
        PeriodicReportService.tick(&ctx).await;
        let report = only_report(&db).await;
        let runner = crate::NotifierDeliveryRunner::new(db, notifier.clone())
            .with_aux_text(generator.clone());
        assert!(runner.run_report("u", "system_default", report.id).await);

        let calls = generator.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        for (user, prompt) in calls.iter() {
            assert_eq!(user, "u");
            for expected in [
                "以下は定期報告「日報」",
                "800文字以内",
                "構成: 1)タスクの進捗 2)予定・支払いのトピック 3)家計サマリ \
                 4)主な会話トピック（プライバシーに配慮し詳細は省く） 5)ひとことコメント。サマリ本文のみ",
            ] {
                assert!(prompt.contains(expected), "missing {expected}: {prompt}");
            }
        }
        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        for notification in sent.iter() {
            let embed = &notification.embeds[0];
            assert_eq!(embed.description.as_deref(), Some("今週のまとめ。"));
            assert_eq!(embed.footer.as_deref(), Some("Yuuka 自動生成レポート"));
        }
    }

    #[tokio::test]
    async fn prompt_contains_only_selected_and_scoped_material() {
        // タスクと予定だけを選ぶと、家計・支払い・会話は本文・フィールド・要約素材のどれにも載らない。
        let (db, _dir) = seed(true, YEARLY, r#"["schedules","tasks"]"#, None).await;
        exec(
            &db,
            r#"INSERT INTO todos (user_id, bot_id, title, status, tags, updated_at)
                VALUES ('u','system_default','完了資料','done','["仕事"]',datetime('now','localtime'));
               INSERT INTO todos (user_id, bot_id, title, status, due_date, priority)
                VALUES ('u','system_default','提出資料','open',date('now','localtime'),'high');
               INSERT INTO todos (user_id, bot_id, title, status, updated_at)
                VALUES ('other','system_default','別ユーザーのタスク','done',datetime('now','localtime'));
               INSERT INTO schedules (user_id, bot_id, title, start_at)
                VALUES ('u','system_default','面談',datetime('now','localtime','+1 hour'));
               INSERT INTO planned_payments (user_id, bot_id, title, amount, due_date)
                VALUES ('u','system_default','家賃',80000,date('now','localtime'));
               INSERT INTO expenses (user_id, bot_id, amount, category, date)
                VALUES ('u','system_default',1500,'食費',date('now','localtime'));
               INSERT INTO message_logs (user_id, bot_id, role, content, created_at)
                VALUES ('u','system_default','user','旅行の相談',datetime('now','localtime'));"#,
        )
        .await;
        let report = only_report(&db).await;
        let notifier = RecordingNotifier::new(true);
        let generator = RecordingGenerator::default();
        deliver(&db, &notifier, &generator, &report).await.unwrap();

        let sent = notifier.sent.lock().unwrap();
        let embed = &sent[0].embeds[0];
        let body = embed.description.as_deref().unwrap_or_default();
        assert!(body.contains("  ・完了資料"), "{body}");
        assert!(body.contains("  ・面談"), "{body}");
        assert!(
            !body.contains("家賃") && !body.contains("💰 収支"),
            "{body}"
        );
        let names: Vec<&str> = embed.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["✅ 完了", "📌 持ち越し"]);

        let calls = generator.calls.lock().unwrap();
        let prompt = &calls[0].1;
        for expected in [
            "構成: 1)タスクの進捗 2)予定のトピック 3)ひとことコメント。",
            "構成に無い内容には触れないでください。",
            "完了資料 [仕事]",
            "提出資料（期限:",
            "優先度: high",
            "面談",
        ] {
            assert!(prompt.contains(expected), "missing {expected}: {prompt}");
        }
        for excluded in [
            "家賃",
            "食費",
            "家計",
            "旅行の相談",
            "会話トピック",
            "別ユーザーのタスク",
        ] {
            assert!(!prompt.contains(excluded), "{excluded}: {prompt}");
        }
    }

    #[tokio::test]
    async fn conversation_samples_are_scoped_and_bounded() {
        let (db, _dir) = seed(true, YEARLY, r#"["topics"]"#, None).await;
        db.writer.execute(|conn| {
            let insert = |user: &str, bot: &str, guild: Option<&str>, role: &str, content: &str, date: &str| {
                conn.execute("INSERT INTO message_logs (user_id, bot_id, guild_id, role, content, created_at) VALUES (?1,?2,?3,?4,?5,?6)", params![user, bot, guild, role, content, date]).map_err(map_sqlite)
            };
            let now = datetime::local_to_db(Local::now());
            // 40件窓から落ちる発言。
            insert("u", "system_default", None, "user", "古い発言", &now)?;
            for i in 0..40 {
                let role = if i % 2 == 0 { "user" } else { "assistant" };
                let content = if role == "user" { format!("抜粋{i:02}{}末尾", "あ".repeat(110)) } else { "アシスタント発言".to_owned() };
                insert("u", "system_default", None, role, &content, &now)?;
            }
            for (user, bot, guild, date, content) in [
                ("other", "system_default", None, now.as_str(), "別ユーザー発言"),
                ("u", "other", None, now.as_str(), "別Bot発言"),
                ("u", "system_default", Some("guild"), now.as_str(), "ギルド発言"),
                ("u", "system_default", None, "2000-01-01 00:00:00", "期間外発言"),
                ("u", "system_default", None, "2999-01-01 00:00:00", "未来発言"),
            ] { insert(user, bot, guild, "user", content, date)?; }
            Ok(())
        }).await.unwrap();
        let report = only_report(&db).await;
        let notifier = RecordingNotifier::new(true);
        let generator = RecordingGenerator::default();
        deliver(&db, &notifier, &generator, &report).await.unwrap();

        let calls = generator.calls.lock().unwrap();
        let prompt = &calls[0].1;
        let samples: Vec<_> = prompt.lines().filter(|s| s.starts_with("- 抜粋")).collect();
        assert_eq!(samples.len(), 20);
        assert!(samples[0].starts_with("- 抜粋38"));
        assert!(samples.iter().all(|s| s.chars().count() == 102));
        for excluded in [
            "別ユーザー発言",
            "別Bot発言",
            "ギルド発言",
            "期間外発言",
            "未来発言",
            "古い発言",
            "アシスタント発言",
            "末尾",
        ] {
            assert!(!prompt.contains(excluded), "{excluded}");
        }
        // 会話トピックだけで要約も無いときは、空でない案内文を送る。
        let embed = &notifier.sent.lock().unwrap()[0].embeds[0];
        assert_eq!(
            embed.description.as_deref(),
            Some("この期間に報告する内容はありません。")
        );
        assert!(embed.fields.is_empty());
    }

    #[tokio::test]
    async fn failed_or_empty_summary_keeps_raw_report() {
        for response in [None, Some(String::new()), Some(" \n\t ".to_owned())] {
            let (db, _dir) = seed(true, "* * * * *", ALL_ACTIVITY, None).await;
            let notifier = Arc::new(RecordingNotifier::new(true));
            let generator = Arc::new(RecordingGenerator {
                response,
                ..Default::default()
            });
            let ctx = ctx_with(db, notifier.clone()).with_aux_text(generator.clone());
            PeriodicReportService.tick(&ctx).await;
            assert_eq!(generator.calls.lock().unwrap().len(), 1);
            let sent = notifier.sent.lock().unwrap();
            assert_eq!(
                sent[0].embeds[0].footer.as_deref(),
                Some("Yuuka レポート（生データ）")
            );
        }

        // 要約素材（会話ログ）が読めなくても、生データで配信する。
        let (db, _dir) = seed(true, "* * * * *", ALL_ACTIVITY, None).await;
        exec(&db, "DROP TABLE message_logs").await;
        let notifier = Arc::new(RecordingNotifier::new(true));
        let generator = Arc::new(RecordingGenerator::default());
        let ctx = ctx_with(db, notifier.clone()).with_aux_text(generator.clone());
        PeriodicReportService.tick(&ctx).await;
        assert!(generator.calls.lock().unwrap().is_empty());
        assert_eq!(notifier.count(), 1);
    }

    #[tokio::test]
    async fn does_not_run_on_start() {
        assert!(!PeriodicReportService.run_on_start());
    }
}
