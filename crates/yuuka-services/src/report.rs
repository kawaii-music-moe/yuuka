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
//! ## Node パリティ
//! 直近24時間/7日の活動・カテゴリ別支出・本人のDM会話サンプルから、ユーザーキーで
//! 補助生成する。キー無し・生成失敗・空応答時は既存の生データ本文へ戻す。
//! 定時配信とテスト配信は同じ生成経路・Embed構造を使う。

use async_trait::async_trait;
use chrono::Local;
use rusqlite::params;
use yuuka_briefing::{list_enabled_reports, EnabledReport};
use yuuka_core::aux_text::AuxTextGenerator;
use yuuka_core::{BotId, DbError, EmbedFieldPart, EmbedPart, UserId};
use yuuka_db::map_sqlite;
use yuuka_web::Db;

use crate::context::ServiceContext;
use crate::notifier::{Notification, Notifier, NotifyTarget};
use crate::schedule::{CronService, Schedule};
use yuuka_core::{cron_util, datetime};

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
        if let Err(e) = deliver(
            &ctx.db,
            ctx.notifier.as_ref(),
            ctx.aux_text.as_ref(),
            &config,
        )
        .await
        {
            // 1 件の DB エラーで tick 全体を止めない（次設定・次 tick で復帰・Node の per-user try/catch）。
            tracing::error!(user = %config.user_id, error = %e, "❌ レポート配信に失敗");
        }
    }
    Ok(())
}

/// 1 件のレポートを生成して配信先へ送る（Node `runReportForUser`）。
/// 送信できたら `true`。
///
/// 定時配信（[`process_due_reports`]）とテスト配信（[`crate::delivery::NotifierDeliveryRunner`]）の
/// 共通経路。`config` の `user_id`/`bot_id` のデータだけを集約し、その本人へ送る。
/// 送信先は配信設定（`target_type`/`target_id`）から解決する。
pub(crate) async fn deliver(
    db: &Db,
    notifier: &dyn Notifier,
    generator: &dyn AuxTextGenerator,
    config: &EnabledReport,
) -> Result<bool, DbError> {
    deliver_to(db, notifier, generator, config, resolve_target(config)).await
}

/// [`deliver`] の送信先指定版。テスト配信で設定行が無いとき、[`NotifyTarget::Default`]
/// （ユーザー既定送信先）で送るために使う（Node は `config ? {...} : undefined` で送信先を省略）。
pub(crate) async fn deliver_to(
    db: &Db,
    notifier: &dyn Notifier,
    generator: &dyn AuxTextGenerator,
    config: &EnabledReport,
    target: NotifyTarget,
) -> Result<bool, DbError> {
    let is_weekly = config.r#type == "weekly";
    let data = collect_report_data(db, &config.user_id, &config.bot_id, is_weekly).await?;

    // 補助素材の読み取りも含め、要約経路の失敗で生データ配信を失わない。
    let summary = match build_summary_prompt(db, config, &data).await {
        Ok(prompt) => generator.generate_aux_text(&config.user_id, &prompt).await,
        Err(e) => {
            tracing::warn!(user = %config.user_id, error = %e, "レポート要約素材の取得に失敗（生データへ縮退）");
            None
        }
    };
    let notification = Notification::embed(
        UserId::new(config.user_id.clone()),
        BotId::new(config.bot_id.clone()),
        report_embed(&data, summary.as_deref()),
    )
    .with_target(target);

    // 配信不可（Discord 未配線 = NullNotifier / クライアント無し）は false。レポートは last_run を持たず
    // 「この分だけ配信」のため、失敗しても再スケジュールはしない（Node も送信結果を保存しない）。
    let sent = notifier.send(notification).await;
    if !sent {
        tracing::warn!(user = %config.user_id, "⚠️ レポートの通知に失敗（配信先未配線などで未送信）");
    }
    Ok(sent)
}

/// 配信先を解決する（Node `{ type: target_type, id: target_id }`）。
/// `channel` かつ ID 有り → チャンネル指定・それ以外 → DM。briefing と同規律（配信設定の送信先を
/// 明示して渡すため、ユーザー既定送信先は見ない）。
fn resolve_target(config: &EnabledReport) -> NotifyTarget {
    if config.target_type == "channel" {
        match config.target_id.as_deref() {
            Some(id) if !id.is_empty() => NotifyTarget::Channel(id.to_owned()),
            _ => NotifyTarget::Dm,
        }
    } else {
        NotifyTarget::Dm
    }
}

// ─── 期間活動データの集約（Node `collectReportData`） ───────────────────

/// 期間中の活動データ（生データと要約で共有）。
struct ReportData {
    period_label: String,
    from: String,
    to: String,
    completed_todos: Vec<(String, Vec<String>)>, // (title, tags)
    carry_over_todos: Vec<(String, Option<String>, Option<String>)>, // (title, due_date, priority)
    schedules: Vec<(String, String)>,            // (title, start_at)
    payments: Vec<PaymentRow>,
    income_total: i64,
    expense_total: i64,
}

/// 支払い予定 1 件（Node `payments` 要素）。
struct PaymentRow {
    title: String,
    amount: i64,
    status: String,
    due_date: String,
}

/// 当該期間の活動データを集約する（Node `collectReportData`）。
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
    let period_seconds = if is_weekly {
        7 * 24 * 60 * 60
    } else {
        24 * 60 * 60
    };
    let from_dt = now - chrono::Duration::seconds(period_seconds);
    let from = datetime::local_to_db(from_dt);
    let to = datetime::local_to_db(now);
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
                    "SELECT title, tags FROM todos \
                     WHERE user_id = ?1 AND bot_id = ?2 AND status = 'done' \
                       AND updated_at >= ?3 AND updated_at <= ?4 \
                     ORDER BY updated_at DESC LIMIT 30",
                )
                .map_err(map_sqlite)?;
            let completed_todos = stmt
                .query_map(params![u, b, from_c, to_c], |r| {
                    let tags: String = r.get(1)?;
                    Ok((
                        r.get(0)?,
                        serde_json::from_str::<Vec<String>>(&tags).unwrap_or_default(),
                    ))
                })
                .map_err(map_sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite)?;

            // 持ち越しタスク: open かつ期限が当該期間内（Node `carryOverTodos`・date(due_date) 範囲・上限30）。
            let mut stmt = conn
                .prepare(
                    "SELECT title, due_date, priority FROM todos \
                     WHERE user_id = ?1 AND bot_id = ?2 AND status = 'open' \
                       AND due_date IS NOT NULL \
                       AND date(due_date) >= date(?3) AND date(due_date) <= date(?4) \
                     ORDER BY due_date ASC LIMIT 30",
                )
                .map_err(map_sqlite)?;
            let carry_over_todos = stmt
                .query_map(params![u, b, from_day_c, to_day_c], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
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
            // planned_payments は V17 で bot_id 列を持つ（finance の登録も bot_id 付き）ため、Node と
            // 同じく user_id + bot_id で絞り、同じユーザーの別 Bot の支払い予定を混ぜない（R24）。
            let mut stmt = conn
                .prepare(
                    "SELECT title, amount, status, due_date FROM planned_payments \
                     WHERE user_id = ?1 AND bot_id = ?2 \
                       AND date(due_date) >= date(?3) AND date(due_date) <= date(?4) \
                     ORDER BY due_date ASC LIMIT 20",
                )
                .map_err(map_sqlite)?;
            let payments = stmt
                .query_map(params![u, b, from_day_c, to_day_c], |r| {
                    Ok(PaymentRow {
                        title: r.get::<_, String>(0)?,
                        amount: r.get::<_, i64>(1)?,
                        status: r.get::<_, String>(2)?,
                        due_date: r.get(3)?,
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
                from,
                to,
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

/// Node のプロンプト専用素材。生データとは分け、取得失敗時も配信可能にする。
async fn build_summary_prompt(
    db: &Db,
    config: &EnabledReport,
    data: &ReportData,
) -> Result<String, DbError> {
    let (u, b) = (config.user_id.clone(), config.bot_id.clone());
    let (from_day, to_day) = (data.from[..10].to_owned(), data.to[..10].to_owned());
    let categories = db
        .read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT category, SUM(amount) FROM expenses \
                     WHERE user_id = ?1 AND bot_id = ?2 AND type = 'expense' \
                       AND date >= ?3 AND date <= ?4 \
                     GROUP BY category ORDER BY SUM(amount) DESC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![u, b, from_day, to_day], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                })
                .map_err(map_sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite)?;
            Ok(rows)
        })
        .await?;
    // 最新40件（assistantを含む）からユーザー発言を20件、各100文字。guild会話は除外する。
    let messages = yuuka_conversation::search_messages(
        db,
        &config.user_id,
        &config.bot_id,
        None,
        Some(&data.from),
        Some(&data.to),
        40,
    )
    .await?;
    let mut source = vec![format!("# 完了タスク（{}件）", data.completed_todos.len())];
    source.extend(
        data.completed_todos
            .iter()
            .map(|(title, tags)| format!("- {title} [{}]", tags.join("/"))),
    );
    source.push(format!(
        "# 持ち越しタスク（{}件）",
        data.carry_over_todos.len()
    ));
    source.extend(data.carry_over_todos.iter().map(|(title, due, priority)| {
        format!(
            "- {title}（期限: {}、優先度: {}）",
            due.as_deref().unwrap_or(""),
            priority.as_deref().unwrap_or("未設定")
        )
    }));
    source.push(format!("# 予定（{}件）", data.schedules.len()));
    source.extend(
        data.schedules
            .iter()
            .map(|(title, start)| format!("- {title} ({start})")),
    );
    source.push("# 支払い予定".to_owned());
    source.extend(data.payments.iter().map(|p| {
        format!(
            "- {} {}円 期日{} 状態:{}",
            p.title, p.amount, p.due_date, p.status
        )
    }));
    source.push(format!(
        "# 家計: 収入{}円 / 支出{}円",
        data.income_total, data.expense_total
    ));
    source.extend(
        categories
            .iter()
            .map(|(category, total)| format!("- {category}: {total}円")),
    );
    source.push(
        "# 会話トピックの素材（ユーザー発言の抜粋。詳細は書かずトピック傾向のみ要約すること）"
            .to_owned(),
    );
    source.extend(
        messages
            .iter()
            .filter(|m| m.role == "user")
            .take(20)
            .map(|m| format!("- {}", m.content.chars().take(100).collect::<String>())),
    );
    let type_label = if config.r#type == "weekly" {
        "週報"
    } else {
        "日報"
    };
    Ok(format!(
        "以下は{type_label}（対象期間: {}）の活動データです。\
         Discord通知用の{type_label}サマリを日本語で簡潔に生成してください（800文字以内）。\
         構成: 1)タスクの進捗 2)予定・支払いのトピック 3)家計サマリ 4)主な会話トピック（プライバシーに配慮し詳細は省く） 5)ひとことコメント。\
         サマリ本文のみを出力してください。\n\n{}", data.period_label, source.join("\n")
    ))
}

// ─── Embed 整形（Node `buildFallbackText` + `EmbedBuilder`） ────────────────────────────────

/// レポートのタイトルを作る（Node `📋 ${typeLabel} ${periodLabel}`）。
fn title_line(is_weekly: bool, period_label: &str) -> String {
    let type_label = if is_weekly { "週報" } else { "日報" };
    format!("📋 {type_label} {period_label}")
}

/// Node `buildFallbackText` と同じ生データ本文を作る。
fn render_report_body(data: &ReportData) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("✅ 完了タスク: {}件", data.completed_todos.len()));
    for (t, _) in data.completed_todos.iter().take(8) {
        lines.push(format!("  ・{t}"));
    }
    lines.push(format!(
        "📌 持ち越しタスク: {}件",
        data.carry_over_todos.len()
    ));
    for (title, due, _) in data.carry_over_todos.iter().take(8) {
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
    lines.join("\n")
}

/// 生成本文または生データを旧 Node と同じ構造の Embed にする。
fn report_embed(data: &ReportData, summary: Option<&str>) -> EmbedPart {
    let summary = summary.map(str::trim).filter(|s| !s.is_empty());
    let body = summary
        .map(str::to_owned)
        .unwrap_or_else(|| render_report_body(data));
    let is_weekly = data.period_label.contains('〜');
    EmbedPart {
        title: Some(title_line(is_weekly, &data.period_label)),
        description: Some(body.chars().take(4000).collect()),
        color: 0x58_65_F2,
        fields: vec![
            EmbedFieldPart {
                name: "✅ 完了".to_owned(),
                value: format!("{}件", data.completed_todos.len()),
                inline: true,
            },
            EmbedFieldPart {
                name: "📌 持ち越し".to_owned(),
                value: format!("{}件", data.carry_over_todos.len()),
                inline: true,
            },
            EmbedFieldPart {
                name: "💰 収支".to_owned(),
                value: format!(
                    "+{} / -{}",
                    format_currency(data.income_total),
                    format_currency(data.expense_total)
                ),
                inline: true,
            },
        ],
        footer: Some(
            if summary.is_some() {
                "Yuuka 自動生成レポート"
            } else {
                "Yuuka レポート（生データ）"
            }
            .to_owned(),
        ),
    }
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
        DROP TABLE IF EXISTS report_configs; CREATE TABLE report_configs (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL, \
            bot_id TEXT NOT NULL, type TEXT NOT NULL, \
            enabled INTEGER NOT NULL DEFAULT 0, schedule_cron TEXT NOT NULL DEFAULT '0 21 * * *', \
            target_type TEXT NOT NULL DEFAULT 'dm', target_id TEXT, \
            updated_at TEXT NOT NULL DEFAULT (datetime('now','localtime')), \
            UNIQUE (user_id, bot_id, type));\
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
            let mut conn = rusqlite::Connection::open(&path).expect("empty file");
            // Install the isolated unit schema after migrating the real schema (SD07).
            yuuka_db::run_migrations(&mut conn).expect("migrate before unit fixture");
            conn.execute_batch(DDL).expect("ddl");
            conn.execute(
                "INSERT INTO report_configs (user_id, bot_id, type, enabled, schedule_cron, target_type, target_id) \
                 VALUES ('u', 'system_default', ?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![r#type, i64::from(enabled), cron, target_type, target_id],
            )
            .expect("seed row");
        }
        // 会話サンプル（message_logs）と通知先（users）は実マイグレーションのスキーマをそのまま使う。
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
        // 設定の dm は DM（ユーザー既定送信先は見ない）で、日報は Embed として届く。
        assert_eq!(sent[0].target, NotifyTarget::Dm);
        assert!(sent[0].content.is_empty());
        assert_eq!(sent[0].embeds.len(), 1);
        let embed = &sent[0].embeds[0];
        assert!(embed
            .title
            .as_deref()
            .is_some_and(|title| title.contains("📋 日報")));
        assert_eq!(embed.color, 0x58_65_F2);
        assert_eq!(embed.footer.as_deref(), Some("Yuuka レポート（生データ）"));
        // 集約元が空でも fallback 本体（完了/持ち越し/収支）は出る。
        let body = embed.description.as_deref().unwrap_or_default();
        assert!(body.contains("✅ 完了タスク: 0件"));
        assert!(body.contains("💰 収支: 収入 ¥0 / 支出 ¥0"));
        assert_eq!(embed.fields.len(), 3);
        assert!(embed.fields.iter().all(|field| field.inline));
        assert_eq!(embed.fields[0].name, "✅ 完了");
        assert_eq!(embed.fields[0].value, "0件");
        assert_eq!(embed.fields[1].name, "📌 持ち越し");
        assert_eq!(embed.fields[1].value, "0件");
        assert_eq!(embed.fields[2].name, "💰 収支");
        assert_eq!(embed.fields[2].value, "+¥0 / -¥0");
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
        assert!(sent[0].embeds[0]
            .title
            .as_deref()
            .is_some_and(|title| title.contains("📋 週報")));
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
                     INSERT INTO planned_payments (user_id, bot_id, title, amount, due_date, status) \
                       VALUES ('u','system_default','家賃',80000, date('now','localtime'),'pending');\
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
        let body = sent[0].embeds[0].description.as_deref().unwrap_or_default();
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

    #[tokio::test]
    async fn payments_are_scoped_to_report_bot() {
        // 同じユーザーの別 Bot の支払い予定はレポートに混ぜない（R24・Node の user_id + bot_id 条件）。
        let (db, _dir) = seed(true, "* * * * *", "daily", "dm", None);
        db.writer
            .execute(|conn| {
                conn.execute_batch(
                    "INSERT INTO planned_payments (user_id, bot_id, title, amount, due_date, status) \
                       VALUES ('u','system_default','家賃',80000, date('now','localtime'),'pending');\
                     INSERT INTO planned_payments (user_id, bot_id, title, amount, due_date, status) \
                       VALUES ('u','other_bot','別Botのサブスク',980, date('now','localtime'),'pending');",
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
        let body = sent[0].embeds[0].description.as_deref().unwrap_or_default();
        assert!(body.contains("  ・家賃 ¥80,000 [未払い]"), "body: {body}");
        assert!(!body.contains("別Botのサブスク"), "body: {body}");
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
    async fn summary_is_used_by_scheduled_and_manual_reports() {
        use yuuka_briefing::DeliveryRunner;
        let (db, _dir) = seed(true, "* * * * *", "weekly", "channel", Some("chan-1"));
        db.writer.execute(|conn| {
            conn.execute_batch(r#"
                INSERT INTO todos (user_id, bot_id, title, status, tags, updated_at)
                    VALUES ('u','system_default','完了資料','done','["仕事"]',datetime('now','localtime'));
                INSERT INTO todos (user_id, bot_id, title, status, due_date, priority)
                    VALUES ('u','system_default','提出資料','open',date('now','localtime'),'high');
                INSERT INTO schedules (user_id, bot_id, title, start_at)
                    VALUES ('u','system_default','面談',datetime('now','localtime'));
                INSERT INTO planned_payments (user_id, bot_id, title, amount, due_date)
                    VALUES ('u','system_default','家賃',80000,date('now','localtime'));
                INSERT INTO expenses (user_id, bot_id, amount, category, date)
                    VALUES ('u','system_default',1500,'食費',date('now','localtime'));
                INSERT INTO expenses (user_id, bot_id, amount, category, date)
                    VALUES ('other','system_default',9999,'別ユーザー家計',date('now','localtime'));
                INSERT INTO expenses (user_id, bot_id, amount, category, date)
                    VALUES ('u','other',9999,'別Bot家計',date('now','localtime'));
                INSERT INTO message_logs (user_id, bot_id, role, content, created_at)
                    VALUES ('u','system_default','user','旅行の相談',datetime('now','localtime'));
            "#).map_err(map_sqlite)?;
            Ok(())
        }).await.unwrap();
        let generator = Arc::new(RecordingGenerator {
            response: Some("  今週のまとめ。\n".to_owned()),
            ..Default::default()
        });
        let notifier = Arc::new(RecordingNotifier::new(true));
        let ctx = ctx_with(db.clone(), notifier.clone()).with_aux_text(generator.clone());
        ReportService.tick(&ctx).await;
        let runner = crate::NotifierDeliveryRunner::new(db, notifier.clone())
            .with_aux_text(generator.clone());
        assert!(runner.run_report("u", "system_default", "weekly").await);
        let calls = generator.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        for (user, prompt) in calls.iter() {
            assert_eq!(user, "u");
            for expected in [
                "以下は週報",
                "800文字以内",
                "プライバシーに配慮し詳細は省く",
                "完了資料 [仕事]",
                "提出資料（期限:",
                "優先度: high",
                "面談",
                "家賃 80000円 期日",
                "食費: 1500円",
                "旅行の相談",
            ] {
                assert!(prompt.contains(expected), "missing {expected}: {prompt}");
            }
            assert!(!prompt.contains("別ユーザー家計"));
            assert!(!prompt.contains("別Bot家計"));
        }
        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        for notification in sent.iter() {
            assert_eq!(
                notification.target,
                NotifyTarget::Channel("chan-1".to_owned())
            );
            let embed = &notification.embeds[0];
            assert_eq!(embed.description.as_deref(), Some("今週のまとめ。"));
            assert_eq!(embed.footer.as_deref(), Some("Yuuka 自動生成レポート"));
            assert_eq!(embed.fields[0].value, "1件");
            assert_eq!(embed.fields[2].value, "+¥0 / -¥1,500");
        }
    }

    #[tokio::test]
    async fn manual_report_without_config_uses_summary_and_default_target() {
        use yuuka_briefing::DeliveryRunner;
        let (db, _dir) = seed(false, "* * * * *", "weekly", "dm", None);
        db.writer.execute(|conn| {
            conn.execute_batch("INSERT INTO users (discord_id, username, salt, notify_target_type, notify_target_id)
                VALUES ('u','u','','channel','default-channel');").map_err(map_sqlite)?;
            Ok(())
        }).await.unwrap();
        let notifier = Arc::new(RecordingNotifier::new(true));
        let generator = Arc::new(RecordingGenerator {
            response: Some("今日のまとめ。".to_owned()),
            ..Default::default()
        });
        let runner = crate::NotifierDeliveryRunner::new(db, notifier.clone())
            .with_aux_text(generator.clone());
        assert!(runner.run_report("u", "system_default", "daily").await);
        let calls = generator.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "u");
        assert!(calls[0].1.contains("以下は日報"));
        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0].target,
            NotifyTarget::Channel("default-channel".to_owned())
        );
        assert_eq!(
            sent[0].embeds[0].description.as_deref(),
            Some("今日のまとめ。")
        );
    }

    #[tokio::test]
    async fn missing_or_empty_generation_keeps_raw_report() {
        for response in [None, Some(String::new()), Some(" \n\t ".to_owned())] {
            let (db, _dir) = seed(true, "* * * * *", "daily", "dm", None);
            let notifier = Arc::new(RecordingNotifier::new(true));
            let generator = Arc::new(RecordingGenerator {
                response,
                ..Default::default()
            });
            let ctx = ctx_with(db, notifier.clone()).with_aux_text(generator.clone());
            ReportService.tick(&ctx).await;
            assert_eq!(generator.calls.lock().unwrap().len(), 1);
            let sent = notifier.sent.lock().unwrap();
            assert_eq!(sent.len(), 1);
            assert_eq!(
                sent[0].embeds[0].footer.as_deref(),
                Some("Yuuka レポート（生データ）")
            );
            assert!(sent[0].embeds[0]
                .description
                .as_ref()
                .unwrap()
                .contains("✅ 完了タスク: 0件"));
        }
    }

    #[tokio::test]
    async fn missing_summary_material_keeps_raw_delivery() {
        let (db, _dir) = seed(true, "* * * * *", "daily", "dm", None);
        db.writer
            .execute(|conn| {
                conn.execute_batch("DROP TABLE message_logs")
                    .map_err(map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();
        let notifier = Arc::new(RecordingNotifier::new(true));
        let generator = Arc::new(RecordingGenerator::default());
        let ctx = ctx_with(db, notifier.clone()).with_aux_text(generator.clone());
        ReportService.tick(&ctx).await;
        assert!(generator.calls.lock().unwrap().is_empty());
        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0].embeds[0].footer.as_deref(),
            Some("Yuuka レポート（生データ）")
        );
    }

    #[tokio::test]
    async fn prompt_samples_are_scoped_and_bounded_like_node() {
        let (db, _dir) = seed(true, "* * * * *", "daily", "dm", None);
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
        let data = collect_report_data(&db, "u", "system_default", false)
            .await
            .unwrap();
        let config = list_enabled_reports(&db, yuuka_core::CrossUserAccess::for_scheduled_task())
            .await
            .unwrap()
            .remove(0);
        let prompt = build_summary_prompt(&db, &config, &data).await.unwrap();
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
            assert!(!prompt.contains(excluded));
        }
        // ユーザー発言だけでも20件まで（40件窓と別の上限）。
        db.writer
            .execute(|conn| {
                conn.execute(
                    "UPDATE message_logs SET role = 'user' WHERE role = 'assistant'",
                    [],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();
        let prompt = build_summary_prompt(&db, &config, &data).await.unwrap();
        assert_eq!(
            prompt
                .lines()
                .filter(|s| s.starts_with("- 抜粋") || s.starts_with("- アシスタント発言"))
                .count(),
            20
        );
    }

    #[tokio::test]
    async fn daily_and_weekly_use_rolling_periods_and_bound_description() {
        let (db, _dir) = seed(true, "* * * * *", "daily", "dm", None);
        db.writer
            .execute(|conn| {
                conn.execute_batch(
                    "INSERT INTO todos (user_id, bot_id, title, status, updated_at) VALUES
                ('u','system_default','2日前','done',datetime('now','localtime','-2 days')),
                ('u','system_default','8日前','done',datetime('now','localtime','-8 days'));",
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();
        for (weekly, days, count) in [(false, 1, 0), (true, 7, 1)] {
            let data = collect_report_data(&db, "u", "system_default", weekly)
                .await
                .unwrap();
            let parse =
                |s: &str| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap();
            assert_eq!(
                parse(&data.to) - parse(&data.from),
                chrono::Duration::days(days)
            );
            assert_eq!(data.completed_todos.len(), count);
            let long = "あ😀".repeat(3000);
            let embed = report_embed(&data, Some(&long));
            assert_eq!(embed.description.unwrap().chars().count(), 4000);
            assert_eq!(embed.footer.as_deref(), Some("Yuuka 自動生成レポート"));
        }
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
