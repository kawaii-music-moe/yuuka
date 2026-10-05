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
//! ## Node パリティの意図的 divergence
//! - **LLM 要約（generateAuxText）は未配線**。Node はニュースを Gemini で 3〜5 件に要約するが、Rust は
//!   `build_briefing` のヘッドライン列挙で代替する（手動 `runBriefingNow` と同じ意図的 divergence）。
//! - **配信は Embed でなく text**。通知ポート（[`crate::notifier::Notifier`]）は現状 text 経路のみのため、
//!   [`yuuka_briefing::render_briefing_text`] で Embed 相当の本文を text 化して配信する。

use async_trait::async_trait;
use chrono::Local;
use yuuka_briefing::{
    build_briefing, list_enabled_briefings, render_briefing_text, EnabledBriefing,
};
use yuuka_core::{BotId, DbError, UserId};

use crate::context::ServiceContext;
use crate::cron_util;
use crate::notifier::{Notification, NotifyTarget};
use crate::schedule::{CronService, Schedule};

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
        if let Err(e) = deliver(ctx, &config).await {
            // 1 件の DB エラーで tick 全体を止めない（次設定・次 tick で復帰・Node の per-user try/catch）。
            tracing::error!(user = %config.user_id, error = %e, "❌ 朝報配信に失敗");
        }
    }
    Ok(())
}

/// 1 件の朝報を生成して配信先へ送る（Node `runBriefingForUser`）。
async fn deliver(ctx: &ServiceContext, config: &EnabledBriefing) -> Result<(), DbError> {
    // 本人設定を再読込して天気/RSS を生成（走査行の直後に設定が消えた等は None → 何もしない）。
    let Some(content) = build_briefing(&ctx.db, &config.user_id, &config.bot_id).await? else {
        return Ok(());
    };

    let body = render_briefing_text(&content);
    let notification = Notification::text(
        UserId::new(config.user_id.clone()),
        BotId::new(config.bot_id.clone()),
        body,
    )
    .with_target(resolve_target(config));

    // 配信不可（Discord 未配線 = NullNotifier / クライアント無し）は false。朝報は last_run を持たず
    // 「この分だけ配信」のため、失敗しても再スケジュールはしない（Node も送信結果を保存しない）。
    if !ctx.notifier.send(notification).await {
        tracing::warn!(user = %config.user_id, "⚠️ 朝報の通知に失敗（配信先未配線などで未送信）");
    }
    Ok(())
}

/// 配信先を解決する（Node `{ type: target_type, id: target_id }`）。
/// `channel` かつ ID 有り → チャンネル指定・それ以外 → 既定送信先（→DM）。reminder と同規律。
fn resolve_target(config: &EnabledBriefing) -> NotifyTarget {
    if config.target_type == "channel" {
        match config.target_id.as_deref() {
            Some(id) if !id.is_empty() => NotifyTarget::Channel(id.to_owned()),
            _ => NotifyTarget::Default,
        }
    } else {
        NotifyTarget::Default
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
    const DDL: &str = "CREATE TABLE briefing_configs (\
        user_id TEXT NOT NULL, bot_id TEXT NOT NULL DEFAULT 'system_default', \
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
            let conn = rusqlite::Connection::open(&path).expect("empty file");
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
        // 既定送信先（→DM）で、本文に朝報タイトルが入る。
        assert_eq!(sent[0].target, NotifyTarget::Default);
        assert!(sent[0].content.contains("今日の朝報です"));
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
