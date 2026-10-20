//! 朝報（天気・ニュース）定時配信サービス（§3.9・現行 `briefingService.ts`）。
//!
//! Node は `node-cron("* * * * *")` で毎分発火し、有効な全ユーザーの `briefing_configs` を走査して
//! `cronMatchesNow(schedule_cron)`（現在分に一致）なら `runBriefingForUser` で天気/RSS を生成し
//! `sendToUser(target)` で配信する。**`briefing_configs` には last_run 列が無い**ため、Node は
//! last_run を使わず「毎分 cron 一致判定」だけで冪等性を担保する。Rust も同モデルを踏襲する。
//!
//! ## 二重発火ガード（last_run 不要な理由）
//! [`Schedule::EveryMinute`] は [`crate::schedule::run_cron`] で **分境界に整列**して sleep する
//! （`next_after("* * * * *", now)` = 厳密に次の :00）。逐次 await ループのため 1 分内に tick は
//! ちょうど 1 回しか走らない（前 tick 完了まで次 sleep に入らない）。かつ `run_on_start=false` で
//! 起動直後の即時 tick を行わない（プロセス再起動が同一分内に起きても二重配信しない）。よって
//! [`cron_util::cron_matches_now`] 単体で「その分に 1 回だけ配信」が成立し、追加の last_run マーカーは
//! 不要（reminder/playbook が「分境界整列 + 逐次ループ」で構造的に二重発火を避けるのと同規律）。
//!
//! ニュースは補助 LLM で要約し、利用不可時はヘッドライン列挙へ縮退する。
//! 定期配信は Node と同じく、タイトル・天気/ニュースフィールド・フッタを持つスカイブルーの Embed とする。

use async_trait::async_trait;
use chrono::Local;
use yuuka_briefing::{build_briefing, list_enabled_briefings, BriefingContent, EnabledBriefing};
use yuuka_core::{BotId, DbError, EmbedFieldPart, EmbedPart, UserId};
use yuuka_web::Db;

use crate::context::ServiceContext;
use crate::notifier::{Notification, Notifier, NotifyTarget};
use crate::schedule::{CronService, Schedule};
use yuuka_core::cron_util;

/// 朝報定時配信サービス。
pub struct BriefingService;

#[async_trait]
impl CronService for BriefingService {
    fn name(&self) -> &'static str {
        "briefing"
    }

    fn schedule(&self) -> Schedule {
        Schedule::EveryMinute // Node `cron.schedule("* * * * *")`。
    }

    fn run_on_start(&self) -> bool {
        // last_run を持たず「その分に一致したら配信」判定のため、起動時即実行は同一分内の
        // 二重配信を招きうる。Node も node-cron 登録のみで catch-up しない＝起動時実行しない。
        false
    }

    async fn tick(&self, ctx: &ServiceContext) {
        if let Err(e) = process_due_briefings(ctx).await {
            tracing::error!(error = %e, "❌ 朝報配信の走査でエラー");
        }
    }
}

/// 有効な朝報設定を走査し、現在分に一致するものを配信する（Node `tick`）。
async fn process_due_briefings(ctx: &ServiceContext) -> Result<(), DbError> {
    let configs = list_enabled_briefings(&ctx.db, ctx.cross).await?;
    let now = Local::now();
    for config in configs {
        if !cron_util::cron_matches_now(&config.schedule_cron, now) {
            continue;
        }
        tracing::info!(user = %config.user_id, "🌅 朝報を配信します");
        if let Err(e) = deliver(
            &ctx.db,
            ctx.notifier.as_ref(),
            ctx.news_summarizer.as_ref(),
            &config,
        )
        .await
        {
            // 1 件の DB エラーで tick 全体を止めない（次設定・次 tick で復帰・Node の per-user try/catch）。
            tracing::error!(user = %config.user_id, error = %e, "❌ 朝報配信に失敗");
        }
    }
    Ok(())
}

/// 1 件の朝報を生成して配信先へ送る（Node `runBriefingForUser`）。送信できたら `true`。
///
/// 定時配信（[`process_due_briefings`]）とテスト配信（[`crate::delivery::NotifierDeliveryRunner`]）の
/// 共通経路。`config` の `user_id`/`bot_id` の設定だけを読み、その本人へ送る。
pub(crate) async fn deliver(
    db: &Db,
    notifier: &dyn Notifier,
    news_summarizer: &dyn yuuka_briefing::NewsSummarizer,
    config: &EnabledBriefing,
) -> Result<bool, DbError> {
    // 本人設定を再読込して天気/RSS を生成（走査行の直後に設定が消えた等は None → 何もしない）。
    let Some(content) =
        build_briefing(db, news_summarizer, &config.user_id, &config.bot_id).await?
    else {
        return Ok(false);
    };

    let notification = Notification::embed(
        UserId::new(config.user_id.clone()),
        BotId::new(config.bot_id.clone()),
        briefing_embed(content),
    )
    .with_target(resolve_target(config));

    // 配信不可（Discord 未配線 = NullNotifier / クライアント無し）は false。朝報は last_run を持たず
    // 「この分だけ配信」のため、失敗しても再スケジュールはしない（Node も送信結果を保存しない）。
    let sent = notifier.send(notification).await;
    if !sent {
        tracing::warn!(user = %config.user_id, "⚠️ 朝報の通知に失敗（配信先未配線などで未送信）");
    }
    Ok(sent)
}

/// 生成済み朝報を旧 Node と同じ構造の Embed にする。
fn briefing_embed(content: BriefingContent) -> EmbedPart {
    EmbedPart {
        title: Some("🌅 おはようございます！今日の朝報です".to_owned()),
        description: (!content.has_content).then(|| {
            "朝報に表示する内容がまだ設定されていません。天気の地点（緯度経度）やニュースのRSSフィードを設定してください。"
                .to_owned()
        }),
        color: 0x00_B0_F4,
        fields: content
            .fields
            .into_iter()
            .map(|(name, value)| EmbedFieldPart {
                name,
                value,
                inline: false,
            })
            .collect(),
        footer: Some("データ提供: Open-Meteo / 登録RSSフィード".to_owned()),
    }
}

/// 配信先を解決する（Node `{ type: target_type, id: target_id }`）。
/// `channel` かつ ID 有り → チャンネル指定・それ以外 → DM。Node は配信設定の送信先を常に明示して
/// 渡すため、ユーザー既定送信先は見ない（ID 無しのチャンネル指定も DM に落ちる）。
fn resolve_target(config: &EnabledBriefing) -> NotifyTarget {
    if config.target_type == "channel" {
        match config.target_id.as_deref() {
            Some(id) if !id.is_empty() => NotifyTarget::Channel(id.to_owned()),
            _ => NotifyTarget::Dm,
        }
    } else {
        NotifyTarget::Dm
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use chrono::{Datelike, Timelike};
    use yuuka_web::Db;

    use super::*;
    use crate::test_support::{ctx_with, RecordingNotifier};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    /// briefing_configs だけを FK 無しで作る（tools.rs の DDL と同形・cron 判定に必要な列を持つ）。
    const DDL: &str = "DROP TABLE IF EXISTS briefing_configs; CREATE TABLE briefing_configs (\
        user_id TEXT NOT NULL, bot_id TEXT NOT NULL, \
        enabled INTEGER NOT NULL DEFAULT 0, schedule_cron TEXT NOT NULL DEFAULT '0 7 * * *', \
        target_type TEXT NOT NULL DEFAULT 'dm', target_id TEXT, weather_lat REAL, weather_lng REAL, \
        location_name TEXT, news_feeds TEXT NOT NULL DEFAULT '[]', \
        news_keywords TEXT NOT NULL DEFAULT '[]', \
        updated_at TEXT NOT NULL DEFAULT (datetime('now','localtime')), PRIMARY KEY (user_id, bot_id));";

    /// enabled/cron/target を指定した briefing 設定を 1 件仕込んだ DB を作る。
    ///
    /// 天気・RSS は未設定（`weather_lat/lng` NULL・`news_feeds` 空）にして外部 HTTP を発生させない
    /// ＝`build_briefing` は「コンテンツ無し」の [`yuuka_briefing::BriefingContent`] を返し、
    /// `render_briefing_text` が空案内文（非空 body）を作るので配信判定だけを検証できる。
    fn seed(
        enabled: bool,
        cron: &str,
        target_type: &str,
        target_id: Option<&str>,
    ) -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = dir.path().join(format!("briefing_svc_{seq}.sqlite"));
        {
            let mut conn = rusqlite::Connection::open(&path).expect("empty file");
            // Install the isolated unit schema after migrating the real schema (SD07).
            yuuka_db::run_migrations(&mut conn).expect("migrate before unit fixture");
            conn.execute_batch(DDL).expect("ddl");
            conn.execute(
                "INSERT INTO briefing_configs (user_id, bot_id, enabled, schedule_cron, target_type, target_id) \
                 VALUES ('u', 'system_default', ?1, ?2, ?3, ?4)",
                rusqlite::params![i64::from(enabled), cron, target_type, target_id],
            )
            .expect("seed row");
        }
        let db = Db::open(&path).expect("open db");
        (db, dir)
    }

    #[tokio::test]
    async fn due_config_delivers_to_dm() {
        // "* * * * *" は常に現在分にマッチ → 配信される。
        let (db, _dir) = seed(true, "* * * * *", "dm", None);
        let notifier = Arc::new(RecordingNotifier::new(true));
        let ctx = ctx_with(db, notifier.clone());

        BriefingService.tick(&ctx).await;

        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        // 設定の dm は DM（ユーザー既定送信先は見ない）で、朝報は Embed として届く。
        assert_eq!(sent[0].target, NotifyTarget::Dm);
        assert!(sent[0].content.is_empty());
        assert_eq!(sent[0].embeds.len(), 1);
        let embed = &sent[0].embeds[0];
        assert_eq!(
            embed.title.as_deref(),
            Some("🌅 おはようございます！今日の朝報です")
        );
        assert_eq!(embed.color, 0x00_B0_F4);
        assert!(embed.description.as_deref().is_some_and(
            |description| description.contains("朝報に表示する内容がまだ設定されていません")
        ));
        assert_eq!(
            embed.footer.as_deref(),
            Some("データ提供: Open-Meteo / 登録RSSフィード")
        );
    }

    #[tokio::test]
    async fn due_config_with_channel_target_delivers_to_channel() {
        let (db, _dir) = seed(true, "* * * * *", "channel", Some("chan-1"));
        let notifier = Arc::new(RecordingNotifier::new(true));
        let ctx = ctx_with(db, notifier.clone());

        BriefingService.tick(&ctx).await;

        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].target, NotifyTarget::Channel("chan-1".to_owned()));
    }

    #[test]
    fn generated_content_keeps_weather_and_news_as_embed_fields() {
        let embed = briefing_embed(BriefingContent {
            fields: vec![
                ("🌤️ 東京 の天気".to_owned(), "**今日**: 晴れ".to_owned()),
                ("📰 今朝のニュース".to_owned(), "・見出し".to_owned()),
            ],
            has_content: true,
        });

        assert!(embed.description.is_none());
        assert_eq!(embed.fields.len(), 2);
        assert_eq!(embed.fields[0].name, "🌤️ 東京 の天気");
        assert_eq!(embed.fields[0].value, "**今日**: 晴れ");
        assert_eq!(embed.fields[1].name, "📰 今朝のニュース");
        assert!(embed.fields.iter().all(|field| !field.inline));
    }

    #[tokio::test]
    async fn non_due_config_is_not_delivered() {
        // 現在分に一致しない cron（1970-01-01 に固定される "0 0 1 1 *" = 元日 0:00 のみ）は
        // 今この分にはまず一致しない → 配信されない。
        let (db, _dir) = seed(true, "0 0 1 1 *", "dm", None);
        let notifier = Arc::new(RecordingNotifier::new(true));
        let ctx = ctx_with(db, notifier.clone());

        BriefingService.tick(&ctx).await;

        // 実行時刻が元日 0:00 でない限り配信されない（テストは通年で安定）。
        let now = Local::now();
        let is_new_year_midnight =
            now.month() == 1 && now.day() == 1 && now.hour() == 0 && now.minute() == 0;
        if is_new_year_midnight {
            return; // 極稀な境界（元日 0:00）はスキップ。
        }
        assert_eq!(notifier.count(), 0);
    }

    #[tokio::test]
    async fn disabled_config_is_not_delivered() {
        let (db, _dir) = seed(false, "* * * * *", "dm", None);
        let notifier = Arc::new(RecordingNotifier::new(true));
        let ctx = ctx_with(db, notifier.clone());

        BriefingService.tick(&ctx).await;
        assert_eq!(notifier.count(), 0);
    }

    #[tokio::test]
    async fn does_not_run_on_start() {
        assert!(!BriefingService.run_on_start());
    }
}
