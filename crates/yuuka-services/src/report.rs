//! 日報・週報の自動生成配信サービス（§3.8・現行 `reportService.ts`）。
//!
//! Node は `node-cron("* * * * *")` で毎分発火し、有効な全ユーザーの `report_configs` を走査して
//! `cronMatchesNow(schedule_cron)`（現在分に一致）なら `runReportForUser` で当該期間の活動データを
//! 集約・生成し `sendToUser(target)` で配信する。**`report_configs` には last_run 列が無い**ため、
//! Node は last_run を使わず「毎分 cron 一致判定」だけで冪等性を担保する（briefing と同モデル）。
//! Rust も同モデルを踏襲する。
//!
//! ## 二重発火ガード（last_run 不要な理由）
//! [`crate::briefing`] と同一。[`Schedule::EveryMinute`] は [`crate::schedule::run_cron`] で分境界に
//! 整列して sleep し、逐次 await ループで 1 分内 tick は 1 回のみ。かつ `run_on_start=false` で起動時
//! 即時 tick を行わない。よって [`cron_util::cron_matches_now`] 単体で「その分に 1 回だけ配信」が成立し、
//! 追加の last_run マーカーは不要。
//!
//! ## Node パリティの意図的 divergence
//! - **LLM 要約（generateAuxText）は未配線**（Gemini オーケストレーション上位層が未整備）。Node 自身も
//!   要約失敗時は `buildFallbackText(data)`（生データ）へフォールバックし本文を
//!   「Yuuka レポート（生データ）」と表示する。Rust は**この Node 自前のフォールバック経路だけを実装**
//!   する（briefing/runBriefingNow と同系統の意図的 divergence・生データ集約は Node と完全に等価）。
//! - **配信は Embed でなく text**。通知ポート（[`crate::notifier::Notifier`]）は現状 text 経路のみのため、
//!   Node の Embed（タイトル + 本文 + 集計フィールド + フッタ）を Discord マークダウンの text 本文へ
//!   整形して配信する（briefing の `render_briefing_text` と同系統・情報は等価）。
//!
//! ## 集約フィールド（Node `collectReportData` → `buildFallbackText` が使う範囲のみ）
//! `buildFallbackText` が参照するのは completedTodos / carryOverTodos / schedules / payments /
//! incomeTotal / expenseTotal の 6 項目のみ（categoryBreakdown・conversationSamples は Gemini プロンプト
//! 専用＝deferred のため fallback では未使用）。よってここでもその 6 項目だけを集約する。

use async_trait::async_trait;
use chrono::Local;
use rusqlite::params;
use yuuka_briefing::{list_enabled_reports, EnabledReport};
use yuuka_core::{BotId, DbError, UserId};
use yuuka_db::map_sqlite;
use yuuka_web::Db;

use crate::context::ServiceContext;
use crate::cron_util;
use crate::notifier::{Notification, NotifyTarget};
use crate::schedule::{CronService, Schedule};

/// 日報・週報定時配信サービス。
pub struct ReportService;

#[async_trait]
impl CronService for ReportService {
    fn name(&self) -> &'static str {
        "report"
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
        if let Err(e) = process_due_reports(ctx).await {
            tracing::error!(error = %e, "❌ レポート配信の走査でエラー");
        }
    }
}

/// 有効なレポート設定を走査し、現在分に一致するものを配信する（Node `tick`）。
async fn process_due_reports(ctx: &ServiceContext) -> Result<(), DbError> {
    let configs = list_enabled_reports(&ctx.db, ctx.cross).await?;
    let now = Local::now();
    for config in configs {
        if !cron_util::cron_matches_now(&config.schedule_cron, now) {
            continue;
        }
        tracing::info!(user = %config.user_id, r#type = %config.r#type, "📋 レポートを生成します");
        if let Err(e) = deliver(ctx, &config).await {
            // 1 件の DB エラーで tick 全体を止めない（次設定・次 tick で復帰・Node の per-user try/catch）。
            tracing::error!(user = %config.user_id, error = %e, "❌ レポート配信に失敗");
        }
    }
    Ok(())
}

/// 1 件のレポートを生成して配信先へ送る（Node `runReportForUser` の生データフォールバック経路）。
async fn deliver(ctx: &ServiceContext, config: &EnabledReport) -> Result<(), DbError> {
    let is_weekly = config.r#type == "weekly";
    let data = collect_report_data(&ctx.db, &config.user_id, &config.bot_id, is_weekly).await?;

    // LLM 要約は未配線＝Node の `buildFallbackText(data)` 経路（フッタ「Yuuka レポート（生データ）」）。
    let body = render_report_text(&data);
    let notification = Notification::text(
        UserId::new(config.user_id.clone()),
        BotId::new(config.bot_id.clone()),
        body,
    )
    .with_target(resolve_target(config));

    // 配信不可（Discord 未配線 = NullNotifier / クライアント無し）は false。レポートは last_run を持たず
    // 「この分だけ配信」のため、失敗しても再スケジュールはしない（Node も送信結果を保存しない）。
    if !ctx.notifier.send(notification).await {
        tracing::warn!(user = %config.user_id, "⚠️ レポートの通知に失敗（配信先未配線などで未送信）");
    }
    Ok(())
}

/// 配信先を解決する（Node `{ type: target_type, id: target_id }`）。
/// `channel` かつ ID 有り → チャンネル指定・それ以外 → 既定送信先（→DM）。briefing / reminder と同規律。
fn resolve_target(config: &EnabledReport) -> NotifyTarget {
    if config.target_type == "channel" {
        match config.target_id.as_deref() {
            Some(id) if !id.is_empty() => NotifyTarget::Channel(id.to_owned()),
            _ => NotifyTarget::Default,
        }
    } else {
        NotifyTarget::Default
    }
}

// ─── 期間活動データの集約（Node `collectReportData` の fallback 使用範囲） ───────────────────

/// 期間中の活動データ（Node `ReportData` の `buildFallbackText` が使う範囲のみ）。
struct ReportData {
    period_label: String,
    completed_todos: Vec<String>,
    carry_over_todos: Vec<(String, Option<String>)>, // (title, due_date)
    schedules: Vec<(String, String)>,                // (title, start_at)
    payments: Vec<PaymentRow>,
    income_total: i64,
    expense_total: i64,
}

/// 支払い予定 1 件（Node `payments` 要素の `buildFallbackText` 使用フィールド）。
struct PaymentRow {
    title: String,
    amount: i64,
    status: String,
}

/// 当該期間の活動データを集約する（Node `collectReportData` の生データフォールバック使用範囲）。
///
/// 期間ウィンドウは Node と同一: daily = 直近 24h、weekly = 直近 7 日（`now - periodMs` 〜 `now`）。
/// datetime 比較列（todos.updated_at）は `'YYYY-MM-DD HH:MM:SS'` の `from`/`to` で、date 比較列
/// （due_date/planned_payments.due_date/expenses.date）は先頭 10 文字の `from_day`/`to_day` で絞る
/// （Node の `date(...)` 比較・`slice(0,10)` パリティ）。全て単一ユーザー・単一 Bot スコープの
/// 読み取り専用クエリ（横断走査は上位の走査ループが担い、ここは本人分だけを集める）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
async fn collect_report_data(
    db: &Db,
    user_id: &str,
    bot_id: &str,
    is_weekly: bool,
) -> Result<ReportData, DbError> {
    let now = Local::now();
    let period_ms = if is_weekly {
        7 * 24 * 60 * 60
    } else {
        24 * 60 * 60
    };
    let from_dt = now - chrono::Duration::seconds(period_ms);
    let from = cron_util::to_db_datetime(from_dt);
    let to = cron_util::to_db_datetime(now);
    let from_day = from[..10].to_owned();
    let to_day = to[..10].to_owned();

    // 期間ラベル（Node `periodLabel`。daily = 当日日付・weekly = 開始日〜今日）。
    let period_label = if is_weekly {
        format!(
            "{}/{} 〜 {}/{}",
            from_dt.format("%-m"),
            from_dt.format("%-d"),
            now.format("%-m"),
            now.format("%-d"),
        )
    } else {
        format!(
            "{}/{}/{}",
            now.format("%Y"),
            now.format("%-m"),
            now.format("%-d"),
        )
    };

    let (u, b) = (user_id.to_owned(), bot_id.to_owned());
    let (from_c, to_c, from_day_c, to_day_c) =
        (from.clone(), to.clone(), from_day.clone(), to_day.clone());

    db.read
        .read(move |conn| {
            // 完了タスク: 当該期間内に更新された done（Node `completedTodos`・updated_at 範囲・上限30）。
            let mut stmt = conn
                .prepare(
                    "SELECT title FROM todos \
                     WHERE user_id = ?1 AND bot_id = ?2 AND status = 'done' \
                       AND updated_at >= ?3 AND updated_at <= ?4 \
                     ORDER BY updated_at DESC LIMIT 30",
                )
                .map_err(map_sqlite)?;
            let completed_todos = stmt
                .query_map(params![u, b, from_c, to_c], |r| r.get::<_, String>(0))
                .map_err(map_sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite)?;

            // 持ち越しタスク: open かつ期限が当該期間内（Node `carryOverTodos`・date(due_date) 範囲・上限30）。
            let mut stmt = conn
                .prepare(
                    "SELECT title, due_date FROM todos \
                     WHERE user_id = ?1 AND bot_id = ?2 AND status = 'open' \
                       AND due_date IS NOT NULL \
                       AND date(due_date) >= date(?3) AND date(due_date) <= date(?4) \
                     ORDER BY due_date ASC LIMIT 30",
                )
                .map_err(map_sqlite)?;
            let carry_over_todos = stmt
                .query_map(params![u, b, from_day_c, to_day_c], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
                })
                .map_err(map_sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite)?;

            // 予定（Node `listSchedulesInRange`・start_at 範囲・datetime 文字列比較）。
            let mut stmt = conn
                .prepare(
                    "SELECT title, start_at FROM schedules \
                     WHERE user_id = ?1 AND bot_id = ?2 AND start_at >= ?3 AND start_at <= ?4 \
                     ORDER BY start_at ASC",
                )
                .map_err(map_sqlite)?;
            let schedules = stmt
                .query_map(params![u, b, from_c, to_c], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(map_sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite)?;

            // 支払い予定: 当該期間に期日を迎えるもの（Node `payments`・date(due_date) 範囲・上限20）。
            // planned_payments は user_id のみで bot_id 列を持たない（Node の SQL は bot_id を
            // 条件に含めるが、テーブルに列が無いため Node 側でも該当行は 0 件になる方が実態に近い）。
            // ここは Node の SQL 条件（bot_id = ?）を落とし、実テーブルのスキーマ（user_id スコープ）に
            // 合わせる＝Node パリティ上の必然的差分（planned_payments に bot_id が無い）。
            let mut stmt = conn
                .prepare(
                    "SELECT title, amount, status FROM planned_payments \
                     WHERE user_id = ?1 \
                       AND date(due_date) >= date(?2) AND date(due_date) <= date(?3) \
                     ORDER BY due_date ASC LIMIT 20",
                )
                .map_err(map_sqlite)?;
            let payments = stmt
                .query_map(params![u, from_day_c, to_day_c], |r| {
                    Ok(PaymentRow {
                        title: r.get::<_, String>(0)?,
                        amount: r.get::<_, i64>(1)?,
                        status: r.get::<_, String>(2)?,
                    })
                })
                .map_err(map_sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite)?;

            // 家計サマリ: 当該期間の収支合計（Node `incomeRow`/`expenseRow`・date 範囲・COALESCE(SUM,0)）。
            let income_total: i64 = conn
                .query_row(
                    "SELECT COALESCE(SUM(amount), 0) FROM expenses \
                     WHERE user_id = ?1 AND bot_id = ?2 AND type = 'income' \
                       AND date >= ?3 AND date <= ?4",
                    params![u, b, from_day_c, to_day_c],
                    |r| r.get::<_, i64>(0),
                )
                .map_err(map_sqlite)?;
            let expense_total: i64 = conn
                .query_row(
                    "SELECT COALESCE(SUM(amount), 0) FROM expenses \
                     WHERE user_id = ?1 AND bot_id = ?2 AND type = 'expense' \
                       AND date >= ?3 AND date <= ?4",
                    params![u, b, from_day_c, to_day_c],
                    |r| r.get::<_, i64>(0),
                )
                .map_err(map_sqlite)?;

            Ok(ReportData {
                period_label,
                completed_todos,
                carry_over_todos,
                schedules,
                payments,
                income_total,
                expense_total,
            })
        })
        .await
}

// ─── テキスト整形（Node `buildFallbackText` + Embed 相当の text 化） ─────────────────────────

/// レポートのタイトルを作る（Node `📋 ${typeLabel} ${periodLabel}`）。
fn title_line(is_weekly: bool, period_label: &str) -> String {
    let type_label = if is_weekly { "週報" } else { "日報" };
    format!("📋 {type_label} {period_label}")
}

/// 生データレポートを **text** へ整形する（Node `buildFallbackText` + Embed タイトル/集計/フッタ相当）。
///
/// Node は Embed（タイトル + description=`buildFallbackText(data)` + 集計フィールド + フッタ
/// 「Yuuka レポート（生データ）」）で配信する。通知ポートは text 経路のみのため、それらを Discord
/// マークダウンで縦に積んだ text 本文にする（briefing の `render_briefing_text` と同系統・情報は等価）。
fn render_report_text(data: &ReportData) -> String {
    let is_weekly = data.period_label.contains('〜');
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("**{}**", title_line(is_weekly, &data.period_label)));
    lines.push(String::new());

    // ── buildFallbackText 本体（Node と同一の行構成） ──
    lines.push(format!("✅ 完了タスク: {}件", data.completed_todos.len()));
    for t in data.completed_todos.iter().take(8) {
        lines.push(format!("  ・{t}"));
    }
    lines.push(format!(
        "📌 持ち越しタスク: {}件",
        data.carry_over_todos.len()
    ));
    for (title, due) in data.carry_over_todos.iter().take(8) {
        let due = due.as_deref().unwrap_or("");
        lines.push(format!("  ・{title}（期限: {due}）"));
    }
    if !data.schedules.is_empty() {
        lines.push(format!("📅 予定: {}件", data.schedules.len()));
        for (title, start) in data.schedules.iter().take(8) {
            lines.push(format!("  ・{title} ({start})"));
        }
    }
    if !data.payments.is_empty() {
        lines.push("💳 支払い予定:".to_owned());
        for p in &data.payments {
            let status_label = match p.status.as_str() {
                "settled" => "消込済",
                "cancelled" => "取消",
                _ => "未払い",
            };
            lines.push(format!(
                "  ・{} {} [{status_label}]",
                p.title,
                format_currency(p.amount)
            ));
        }
    }
    lines.push(format!(
        "💰 収支: 収入 {} / 支出 {}",
        format_currency(data.income_total),
        format_currency(data.expense_total)
    ));

    // ── Embed の集計フィールド相当（Node `.addFields(...)`）＋フッタ ──
    lines.push(String::new());
    lines.push(format!(
        "✅ 完了 {}件 / 📌 持ち越し {}件 / 💰 収支 +{} / -{}",
        data.completed_todos.len(),
        data.carry_over_todos.len(),
        format_currency(data.income_total),
        format_currency(data.expense_total),
    ));
    lines.push("Yuuka レポート（生データ）".to_owned());

    lines.join("\n")
}

/// 金額を `¥1,234` 形式へ整形する（Node `formatCurrency` = `¥{amount.toLocaleString("ja-JP")}`）。
fn format_currency(amount: i64) -> String {
    let negative = amount < 0;
    let digits = amount.unsigned_abs().to_string();
    let len = digits.len();
    let mut grouped = String::with_capacity(len + len / 3);
    for (i, ch) in digits.chars().enumerate() {
        let remaining = len - i;
        if i != 0 && remaining.is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    if negative {
        format!("¥-{grouped}")
    } else {
        format!("¥{grouped}")
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

    /// report_configs + 集約元テーブル（todos/schedules/planned_payments/expenses）を FK 無しで作る
    /// （各ドメイン crate のテスト DDL と同形・集約と cron 判定に必要な列を持つ）。
    const DDL: &str = "\
        CREATE TABLE report_configs (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL, \
            bot_id TEXT NOT NULL DEFAULT 'system_default', type TEXT NOT NULL, \
            enabled INTEGER NOT NULL DEFAULT 0, schedule_cron TEXT NOT NULL DEFAULT '0 21 * * *', \
            target_type TEXT NOT NULL DEFAULT 'dm', target_id TEXT, \
            updated_at TEXT NOT NULL DEFAULT (datetime('now','localtime')), \
            UNIQUE (user_id, bot_id, type));\
        CREATE TABLE todos (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL, \
            bot_id TEXT NOT NULL DEFAULT 'system_default', title TEXT NOT NULL, description TEXT, \
            due_date TEXT, start_date TEXT, priority TEXT, tags TEXT NOT NULL DEFAULT '[]', \
            status TEXT NOT NULL DEFAULT 'open', progress INTEGER NOT NULL DEFAULT 0, parent_id INTEGER, \
            linked_payment_id INTEGER, due_reminded INTEGER NOT NULL DEFAULT 0, repeat_rule TEXT, \
            repeat_until TEXT, repeat_count INTEGER, \
            created_at TEXT NOT NULL DEFAULT (datetime('now','localtime')), \
            updated_at TEXT NOT NULL DEFAULT (datetime('now','localtime')));\
        CREATE TABLE schedules (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL, \
            bot_id TEXT NOT NULL DEFAULT 'system_default', title TEXT NOT NULL, description TEXT, \
            start_at TEXT NOT NULL, end_at TEXT, remind_before_minutes INTEGER NOT NULL DEFAULT 10, \
            reminded INTEGER NOT NULL DEFAULT 0, google_event_id TEXT, google_calendar_id TEXT, \
            created_at TEXT NOT NULL DEFAULT (datetime('now','localtime')));\
        CREATE TABLE planned_payments (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL, title TEXT NOT NULL, \
            amount INTEGER NOT NULL, category TEXT, memo TEXT, due_date TEXT NOT NULL, repeat_rule TEXT, \
            status TEXT NOT NULL DEFAULT 'pending', \
            created_at TEXT NOT NULL DEFAULT (datetime('now','localtime')), \
            updated_at TEXT NOT NULL DEFAULT (datetime('now','localtime')));\
        CREATE TABLE expenses (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL, \
            bot_id TEXT NOT NULL DEFAULT 'system_default', type TEXT NOT NULL DEFAULT 'expense', \
            amount INTEGER NOT NULL, category TEXT NOT NULL, memo TEXT, date TEXT NOT NULL, time TEXT, \
            source TEXT NOT NULL DEFAULT 'manual', \
            created_at TEXT NOT NULL DEFAULT (datetime('now','localtime')));";

    /// enabled/cron/type/target を指定した report 設定を 1 件仕込んだ DB を作る（集約元は空）。
    fn seed(
        enabled: bool,
        cron: &str,
        r#type: &str,
        target_type: &str,
        target_id: Option<&str>,
    ) -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = dir.path().join(format!("report_svc_{seq}.sqlite"));
        {
            let conn = rusqlite::Connection::open(&path).expect("empty file");
            conn.execute_batch(DDL).expect("ddl");
            conn.execute(
                "INSERT INTO report_configs (user_id, bot_id, type, enabled, schedule_cron, target_type, target_id) \
                 VALUES ('u', 'system_default', ?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![r#type, i64::from(enabled), cron, target_type, target_id],
            )
            .expect("seed row");
        }
        let db = Db::open(&path).expect("open db");
        (db, dir)
    }

    #[tokio::test]
    async fn due_config_delivers_to_dm() {
        // "* * * * *" は常に現在分にマッチ → 配信される。
        let (db, _dir) = seed(true, "* * * * *", "daily", "dm", None);
        let notifier = Arc::new(RecordingNotifier::new(true));
        let ctx = ctx_with(db, notifier.clone());

        ReportService.tick(&ctx).await;

        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        // 既定送信先（→DM）で、本文に日報タイトルと生データフッタが入る。
        assert_eq!(sent[0].target, NotifyTarget::Default);
        assert!(sent[0].content.contains("📋 日報"));
        assert!(sent[0].content.contains("Yuuka レポート（生データ）"));
        // 集約元が空でも fallback 本体（完了/持ち越し/収支）は出る。
        assert!(sent[0].content.contains("✅ 完了タスク: 0件"));
        assert!(sent[0].content.contains("💰 収支: 収入 ¥0 / 支出 ¥0"));
    }

    #[tokio::test]
    async fn due_config_with_channel_target_delivers_to_channel() {
        let (db, _dir) = seed(true, "* * * * *", "weekly", "channel", Some("chan-1"));
        let notifier = Arc::new(RecordingNotifier::new(true));
        let ctx = ctx_with(db, notifier.clone());

        ReportService.tick(&ctx).await;

        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].target, NotifyTarget::Channel("chan-1".to_owned()));
        // 週報タイトルは type ラベルが「週報」。
        assert!(sent[0].content.contains("📋 週報"));
    }

    #[tokio::test]
    async fn non_due_config_is_not_delivered() {
        // 現在分に一致しない cron（元日 0:00 のみ）は今この分にはまず一致しない → 配信されない。
        let (db, _dir) = seed(true, "0 0 1 1 *", "daily", "dm", None);
        let notifier = Arc::new(RecordingNotifier::new(true));
        let ctx = ctx_with(db, notifier.clone());

        ReportService.tick(&ctx).await;

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
        let (db, _dir) = seed(false, "* * * * *", "daily", "dm", None);
        let notifier = Arc::new(RecordingNotifier::new(true));
        let ctx = ctx_with(db, notifier.clone());

        ReportService.tick(&ctx).await;
        assert_eq!(notifier.count(), 0);
    }

    #[tokio::test]
    async fn aggregates_period_activity_into_fallback_body() {
        // 集約 4 テーブルへ「期間内」の行を入れ、fallback 本文へ反映されることを検証する。
        let (db, _dir) = seed(true, "* * * * *", "daily", "dm", None);
        db.writer
            .execute(|conn| {
                // 完了タスク（今この瞬間に更新＝daily 窓内）。
                conn.execute_batch(
                    "INSERT INTO todos (user_id, bot_id, title, status, updated_at) \
                       VALUES ('u','system_default','買い物','done', datetime('now','localtime'));\
                     INSERT INTO todos (user_id, bot_id, title, status, due_date) \
                       VALUES ('u','system_default','レポート提出','open', date('now','localtime'));\
                     INSERT INTO schedules (user_id, bot_id, title, start_at) \
                       VALUES ('u','system_default','会議', datetime('now','localtime'));\
                     INSERT INTO planned_payments (user_id, title, amount, due_date, status) \
                       VALUES ('u','家賃',80000, date('now','localtime'),'pending');\
                     INSERT INTO expenses (user_id, bot_id, type, amount, category, date) \
                       VALUES ('u','system_default','income',300000,'salary', date('now','localtime'));\
                     INSERT INTO expenses (user_id, bot_id, type, amount, category, date) \
                       VALUES ('u','system_default','expense',1500,'food', date('now','localtime'));",
                )
                .map_err(yuuka_db::map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();

        let notifier = Arc::new(RecordingNotifier::new(true));
        let ctx = ctx_with(db, notifier.clone());
        ReportService.tick(&ctx).await;

        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        let body = &sent[0].content;
        assert!(body.contains("✅ 完了タスク: 1件"), "body: {body}");
        assert!(body.contains("  ・買い物"), "body: {body}");
        assert!(body.contains("📌 持ち越しタスク: 1件"), "body: {body}");
        assert!(body.contains("  ・レポート提出"), "body: {body}");
        assert!(body.contains("📅 予定: 1件"), "body: {body}");
        assert!(body.contains("  ・会議"), "body: {body}");
        assert!(body.contains("💳 支払い予定:"), "body: {body}");
        assert!(body.contains("  ・家賃 ¥80,000 [未払い]"), "body: {body}");
        assert!(
            body.contains("💰 収支: 収入 ¥300,000 / 支出 ¥1,500"),
            "body: {body}"
        );
    }

    #[test]
    fn format_currency_matches_node_locale() {
        assert_eq!(format_currency(0), "¥0");
        assert_eq!(format_currency(1500), "¥1,500");
        assert_eq!(format_currency(80000), "¥80,000");
        assert_eq!(format_currency(1234567), "¥1,234,567");
    }

    #[tokio::test]
    async fn does_not_run_on_start() {
        assert!(!ReportService.run_on_start());
    }
}
