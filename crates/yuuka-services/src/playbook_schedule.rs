//! マクロ（Playbook）定期実行サービス（§3.6・現行 `playbookScheduleService`）。
//!
//! Node は `node-cron` で schedule ごとにジョブを登録し発火時に `executePlaybook` を呼ぶ。Rust は
//! 他 cron サービス（reminder 等）と同じ **EveryMinute tick モデル**で、毎分「有効な全スケジュール」を
//! 走査し `cron_expression` の次回発火が `last_run_at`（無ければ `created_at`）より後かつ現在以前なら
//! due とみなして実行する。実行は本人の Gemini キー/データで（[`ServiceContext::playbook_runner`]・
//! `ChatEngine` へのブリッジは supervisor が注入）。
//!
//! **意図的な tick-model 差分**: (1) Node は実行後（成功/失敗いずれも `updateLastRun`）だが、マクロ
//! 未検出の早期 return では `updateLastRun` しない。tick モデルでこれを踏襲すると `last_run_at` が進まず
//! 毎分再発火して run が溢れるため、**未検出でも `touch_last_run` する**（無限再実行の回避）。
//! (2) `run_on_start=false` でも、`is_due` が `last_run_at`（無ければ `created_at`）基準で次回発火を
//! 評価するため、**ダウンタイム後の復帰直後 最初の EveryMinute tick で取りこぼしを最大 1 回 catch-up
//! 発火する**（`is_due` は単一 occurrence のみ見るので有界・暴走しない）。Node は node-cron の live tick
//! のみで catch-up しないため、これは Node との意図的挙動差（復帰時に 1 回だけ多く走りうる）。

use async_trait::async_trait;
use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use yuuka_core::{BotId, DbError, UserId, UserScope};
use yuuka_playbook::cron::DueSchedule;
use yuuka_playbook::repo::PlaybookRepo;

use crate::context::ServiceContext;
use crate::cron_util;
use crate::notifier::Notification;
use crate::schedule::{CronService, Schedule};

/// マクロ定期実行サービス。
pub struct PlaybookScheduleService;

#[async_trait]
impl CronService for PlaybookScheduleService {
    fn name(&self) -> &'static str {
        "playbook"
    }

    fn schedule(&self) -> Schedule {
        Schedule::EveryMinute
    }

    fn run_on_start(&self) -> bool {
        // ダウンタイム中の取りこぼしを一括発火させない（Node は node-cron 登録のみで catch-up 無し）。
        false
    }

    async fn tick(&self, ctx: &ServiceContext) {
        if let Err(e) = process_due_schedules(ctx).await {
            tracing::error!(error = %e, "❌ マクロ定期実行の走査でエラー");
        }
    }
}

/// due な有効スケジュールを走査して実行する。
async fn process_due_schedules(ctx: &ServiceContext) -> Result<(), DbError> {
    let repo = PlaybookRepo::new(&ctx.db);
    let schedules = repo.list_enabled_schedules(ctx.cross).await?;
    let now = Local::now();
    for schedule in schedules {
        if !is_due(&schedule, now) {
            continue;
        }
        if let Err(e) = execute(ctx, &repo, &schedule).await {
            // 1 件の DB エラーで tick 全体を止めない（次スケジュール・次 tick で復帰）。
            tracing::error!(
                schedule_id = schedule.id,
                user = %schedule.user_id,
                error = %e,
                "❌ マクロ定期実行に失敗"
            );
        }
    }
    Ok(())
}

/// `cron_expression` の次回発火が「基準時刻（last_run_at→created_at）より後 かつ 現在以前」か。
///
/// 壊れた cron 式（`next_after` が `None`）は due としない（Node `cron.validate` が弾くのと同じ）。
fn is_due(schedule: &DueSchedule, now: DateTime<Local>) -> bool {
    let reference = schedule
        .last_run_at
        .as_deref()
        .and_then(parse_local)
        .or_else(|| parse_local(&schedule.created_at));
    let Some(reference) = reference else {
        tracing::warn!(
            schedule_id = schedule.id,
            "⚠️ マクロスケジュールの基準時刻を解釈できずスキップ"
        );
        return false;
    };
    cron_util::next_after(&schedule.cron_expression, reference).is_some_and(|next| next <= now)
}

/// 1 件の due スケジュールを実行する（run 記録 → マクロ取得 → 実行 → 確定 → 通知 → last_run 更新）。
async fn execute(
    ctx: &ServiceContext,
    repo: &PlaybookRepo<'_>,
    schedule: &DueSchedule,
) -> Result<(), DbError> {
    let run_id = repo
        .record_run_start(
            ctx.cross,
            schedule.id,
            schedule.user_id.clone(),
            schedule.bot_id.clone(),
            schedule.playbook_name.clone(),
        )
        .await?;
    tracing::info!(
        playbook = %schedule.playbook_name,
        user = %schedule.user_id,
        "▶️ マクロ定期実行開始"
    );

    let user_id = UserId::new(schedule.user_id.clone());
    let bot_id = BotId::new(schedule.bot_id.clone());
    let scope = UserScope::new(user_id.clone(), bot_id.clone());

    // 対象マクロを取得（Node `findPlaybooks(...).find(name)`）。未検出は failed 記録＋last_run 更新。
    let Some(playbook) = repo.get(&scope, schedule.playbook_name.clone()).await? else {
        repo.record_run_finish(
            ctx.cross,
            run_id,
            "failed",
            format!("マクロ「{}」が見つかりませんでした。", schedule.playbook_name),
        )
        .await?;
        // tick モデルの無限再発火を避けるため未検出でも last_run を進める（Node 差分・上記モジュール注記）。
        repo.touch_last_run(ctx.cross, schedule.id).await?;
        return Ok(());
    };

    // Node prompt と厳密一致（マクロ名にはタイトル・本文に steps）。
    let prompt = format!(
        "【定期実行】以下のマクロ（手順書）を実行してください。\n\nマクロ名: {}\n---\n{}",
        playbook.title, playbook.steps
    );

    match ctx.playbook_runner.run_secretary(&bot_id, &user_id, prompt).await {
        Ok(text) => {
            repo.record_run_finish(ctx.cross, run_id, "success", text.clone()).await?;
            repo.touch_last_run(ctx.cross, schedule.id).await?;
            // Node: `📋 マクロ「**{title}**」の定期実行が完了しました。\n\n{text.slice(0,1700)}`。
            let content = format!(
                "📋 マクロ「**{}**」の定期実行が完了しました。\n\n{}",
                playbook.title,
                truncate_chars(&text, 1700)
            );
            notify(ctx, &user_id, &bot_id, content).await;
            tracing::info!(playbook = %schedule.playbook_name, "✅ マクロ定期実行完了");
        }
        Err(err) => {
            repo.record_run_finish(ctx.cross, run_id, "failed", err.clone()).await?;
            repo.touch_last_run(ctx.cross, schedule.id).await?;
            // Node: `⚠️ マクロ「{playbook_name}」の定期実行に失敗しました: {err.slice(0,500)}`。
            let content = format!(
                "⚠️ マクロ「{}」の定期実行に失敗しました: {}",
                schedule.playbook_name,
                truncate_chars(&err, 500)
            );
            notify(ctx, &user_id, &bot_id, content).await;
            tracing::error!(playbook = %schedule.playbook_name, error = %err, "❌ マクロ定期実行失敗");
        }
    }
    Ok(())
}

/// 実行結果をユーザーへ通知する（既定送信先＝DM・Node `sendToUser(userId, {content}, undefined, botId)`）。
async fn notify(ctx: &ServiceContext, user_id: &UserId, bot_id: &BotId, content: String) {
    let notification = Notification::text(user_id.clone(), bot_id.clone(), content);
    if !ctx.notifier.send(notification).await {
        tracing::warn!(user = %user_id, "⚠️ マクロ定期実行結果の通知に失敗（run は記録済み）");
    }
}

/// DB 形式 `'YYYY-MM-DD HH:MM:SS'`（ローカル）を `DateTime<Local>` へ。
fn parse_local(s: &str) -> Option<DateTime<Local>> {
    let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok()?;
    Local.from_local_datetime(&naive).single()
}

/// 先頭 `n` 文字に切り詰める（JS `String.prototype.slice(0, n)` 近似・BMP は一致）。
fn truncate_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use yuuka_core::{BotId, UserId};
    use yuuka_web::Db;

    use super::*;
    use crate::test_support::{ctx_full, RecordingNotifier};
    use crate::turn::PlaybookRunner;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    /// 呼び出しプロンプトを記録し、指定の結果を返す fake ランナー。
    struct FakeRunner {
        reply: Result<String, String>,
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl PlaybookRunner for FakeRunner {
        async fn run_secretary(
            &self,
            _bot_id: &BotId,
            _user_id: &UserId,
            prompt: String,
        ) -> Result<String, String> {
            self.calls.lock().unwrap().push(prompt);
            self.reply.clone()
        }
    }

    /// 空ファイル→マイグレーションで全表を作り、user + playbook + 有効スケジュールを仕込む。
    ///
    /// `enabled`/`with_playbook` で「無効スケジュール」「マクロ未検出」ケースを作れる。
    fn seed(enabled: bool, with_playbook: bool) -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = dir.path().join(format!("pb_sched_{seq}.sqlite"));
        {
            let conn = rusqlite::Connection::open(&path).expect("empty file");
            drop(conn);
        }
        let db = Db::open(&path).expect("open db");
        let conn = rusqlite::Connection::open(&path).expect("seed conn");
        conn.execute(
            "INSERT INTO users (discord_id, username, password_hash, salt) VALUES ('u','u','x','x')",
            [],
        )
        .unwrap();
        if with_playbook {
            conn.execute(
                "INSERT INTO playbooks \
                   (user_id, bot_id, name, title, keywords, description, steps, created_at, updated_at) \
                 VALUES ('u','system_default','morning','朝のルーティン','[]','','手順1\n手順2', \
                         datetime('now','localtime'), datetime('now','localtime'))",
                [],
            )
            .unwrap();
        }
        // created_at を過去にし、last_run_at NULL＝初回 tick で due になるようにする。
        conn.execute(
            "INSERT INTO playbook_schedules \
               (user_id, bot_id, playbook_name, cron_expression, description, enabled, created_at, updated_at) \
             VALUES ('u','system_default','morning','* * * * *','', ?1, '2020-01-01 00:00:00', \
                     datetime('now','localtime'))",
            rusqlite::params![i64::from(enabled)],
        )
        .unwrap();
        (db, dir)
    }

    async fn run_status_and_output(db: &Db) -> Option<(String, String)> {
        db.read
            .read(|conn| {
                conn.query_row(
                    "SELECT status, output FROM playbook_runs WHERE user_id = 'u'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(yuuka_db::map_sqlite)
            })
            .await
            .ok()
    }

    async fn run_count(db: &Db) -> i64 {
        db.read
            .read(|conn| {
                conn.query_row("SELECT COUNT(*) FROM playbook_runs", [], |r| r.get(0))
                    .map_err(yuuka_db::map_sqlite)
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn due_schedule_runs_records_notifies_and_is_idempotent() {
        let (db, _dir) = seed(true, true);
        let notifier = Arc::new(RecordingNotifier::new(true));
        let runner = Arc::new(FakeRunner {
            reply: Ok("実行結果です".to_owned()),
            calls: Mutex::new(Vec::new()),
        });
        let ctx = ctx_full(db.clone(), notifier.clone(), runner.clone());

        PlaybookScheduleService.tick(&ctx).await;

        // 成功 run が 1 件・output は runner の返した本文。
        let (status, output) = run_status_and_output(&db).await.expect("run row");
        assert_eq!(status, "success");
        assert_eq!(output, "実行結果です");

        // last_run_at が更新されている。
        let last: Option<String> = db
            .read
            .read(|conn| {
                conn.query_row(
                    "SELECT last_run_at FROM playbook_schedules WHERE user_id = 'u'",
                    [],
                    |r| r.get(0),
                )
                .map_err(yuuka_db::map_sqlite)
            })
            .await
            .unwrap();
        assert!(last.is_some());

        // 通知 1 件・📋 完了文（タイトル入り）。
        {
            let sent = notifier.sent.lock().unwrap();
            assert_eq!(sent.len(), 1);
            assert!(sent[0].content.contains("📋 マクロ「**朝のルーティン**」の定期実行が完了しました"));
            assert!(sent[0].content.contains("実行結果です"));
        }

        // プロンプトが Node と同形（【定期実行】+ タイトル + steps）。
        {
            let calls = runner.calls.lock().unwrap();
            assert_eq!(calls.len(), 1);
            assert!(calls[0].starts_with("【定期実行】以下のマクロ（手順書）を実行してください。"));
            assert!(calls[0].contains("マクロ名: 朝のルーティン"));
            assert!(calls[0].contains("手順1\n手順2"));
        }

        // 冪等性: 同一分内の 2 回目 tick では last_run 前進により due にならず再実行しない。
        PlaybookScheduleService.tick(&ctx).await;
        assert_eq!(run_count(&db).await, 1);
    }

    #[tokio::test]
    async fn runner_error_records_failed_and_warns() {
        let (db, _dir) = seed(true, true);
        let notifier = Arc::new(RecordingNotifier::new(true));
        let runner = Arc::new(FakeRunner {
            reply: Err("Gemini 障害".to_owned()),
            calls: Mutex::new(Vec::new()),
        });
        let ctx = ctx_full(db.clone(), notifier.clone(), runner);

        PlaybookScheduleService.tick(&ctx).await;

        let (status, output) = run_status_and_output(&db).await.expect("run row");
        assert_eq!(status, "failed");
        assert_eq!(output, "Gemini 障害");
        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].content.contains("⚠️ マクロ「morning」の定期実行に失敗しました"));
    }

    #[tokio::test]
    async fn missing_playbook_records_failed_without_running() {
        // マクロ未検出でも last_run を進める（tick モデルの無限再発火回避）。
        let (db, _dir) = seed(true, false);
        let notifier = Arc::new(RecordingNotifier::new(true));
        let runner = Arc::new(FakeRunner {
            reply: Ok("should not run".to_owned()),
            calls: Mutex::new(Vec::new()),
        });
        let ctx = ctx_full(db.clone(), notifier, runner.clone());

        PlaybookScheduleService.tick(&ctx).await;

        let (status, output) = run_status_and_output(&db).await.expect("run row");
        assert_eq!(status, "failed");
        assert!(output.contains("マクロ「morning」が見つかりませんでした。"));
        // ランナーは呼ばれない。
        assert_eq!(runner.calls.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn disabled_schedule_is_not_run() {
        let (db, _dir) = seed(false, true);
        let notifier = Arc::new(RecordingNotifier::new(true));
        let runner = Arc::new(FakeRunner {
            reply: Ok("x".to_owned()),
            calls: Mutex::new(Vec::new()),
        });
        let ctx = ctx_full(db.clone(), notifier, runner);

        PlaybookScheduleService.tick(&ctx).await;
        assert_eq!(run_count(&db).await, 0);
    }
}
