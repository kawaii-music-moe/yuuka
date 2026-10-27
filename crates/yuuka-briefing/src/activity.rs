//! 定期報告の活動データ（タスク・予定・支払い予定・家計・会話トピック）の集計と要約素材。
//!
//! 旧日報・週報（Node `reportService.collectReportData`）の集計を、定期報告の期間（[`ReportWindow`]）で
//! 行う。振り返り（完了・持ち越しタスク、家計、会話）は「前回の配信予定時刻 〜 今」、これからの予定は
//! 「今 〜 次回の配信予定時刻」、支払い予定は両方にまたがる日付範囲で見る。全て単一ユーザー・単一 Bot
//! スコープの読み取り専用クエリ。

use chrono::{DateTime, Duration, Local, Timelike};
use rusqlite::params;
use yuuka_core::{cron_util, datetime, DbError, EmbedFieldPart};
use yuuka_db::map_sqlite;
use yuuka_web::Db;

use crate::model::{PeriodicReport, ReportSection, ReportSettings};

/// 定期報告の対象期間。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ReportWindow {
    /// 振り返りの始まり（前回の配信予定時刻）。
    pub from: DateTime<Local>,
    /// 生成した時刻。
    pub now: DateTime<Local>,
    /// これからの予定を載せる終わり（次回の配信予定時刻）。
    pub until: DateTime<Local>,
}

impl ReportWindow {
    /// 配信スケジュールから期間を決める。前回・次回が求まらない式は前後 24 時間にする。
    ///
    /// 前回は「この分」の発火を含めずに探す（定時配信の発火中に、今まさに配信している回を前回と
    /// 数えないため）。テスト配信では「直近の配信予定時刻 〜 今」になる。
    pub(crate) fn for_schedule(schedule_cron: &str, now: DateTime<Local>) -> Self {
        let this_minute = now
            .with_second(0)
            .and_then(|t| t.with_nanosecond(0))
            .unwrap_or(now);
        let from = cron_util::previous_before(schedule_cron, this_minute)
            .unwrap_or_else(|| now - Duration::days(1));
        let until =
            cron_util::next_after(schedule_cron, now).unwrap_or_else(|| now + Duration::days(1));
        Self { from, now, until }
    }

    /// タイトルに付ける期間の表記。1 日程度までは今日の日付、それより長ければ開始日〜今日。
    pub(crate) fn label(&self) -> String {
        if self.now - self.from <= Duration::hours(25) {
            format!(
                "{}/{}/{}",
                self.now.format("%Y"),
                self.now.format("%-m"),
                self.now.format("%-d")
            )
        } else {
            format!(
                "{}/{} 〜 {}/{}",
                self.from.format("%-m"),
                self.from.format("%-d"),
                self.now.format("%-m"),
                self.now.format("%-d"),
            )
        }
    }
}

/// 期間中の活動データ（生データ本文と要約素材で共有）。
pub(crate) struct ActivityData {
    pub completed_todos: Vec<(String, Vec<String>)>, // (title, tags)
    pub carry_over_todos: Vec<(String, Option<String>, Option<String>)>, // (title, due_date, priority)
    pub schedules: Vec<(String, String)>,                                // (title, start_at)
    pub payments: Vec<PaymentRow>,
    pub income_total: i64,
    pub expense_total: i64,
}

/// 支払い予定 1 件。
pub(crate) struct PaymentRow {
    pub title: String,
    pub amount: i64,
    pub status: String,
    pub due_date: String,
}

/// DB 比較用の期間文字列（`'YYYY-MM-DD HH:MM:SS'` と先頭 10 文字の日付）。
struct WindowStrings {
    from: String,
    now: String,
    until: String,
    from_day: String,
    now_day: String,
    until_day: String,
}

impl WindowStrings {
    fn new(window: &ReportWindow) -> Self {
        let from = datetime::local_to_db(window.from);
        let now = datetime::local_to_db(window.now);
        let until = datetime::local_to_db(window.until);
        Self {
            from_day: from[..10].to_owned(),
            now_day: now[..10].to_owned(),
            until_day: until[..10].to_owned(),
            from,
            now,
            until,
        }
    }
}

/// 期間の活動データを集める。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub(crate) async fn collect(
    db: &Db,
    user_id: &str,
    bot_id: &str,
    window: &ReportWindow,
) -> Result<ActivityData, DbError> {
    let (u, b) = (user_id.to_owned(), bot_id.to_owned());
    let w = WindowStrings::new(window);
    db.read
        .read(move |conn| {
            // 完了タスク: 期間内に更新された done（上限30）。
            let mut stmt = conn
                .prepare(
                    "SELECT title, tags FROM todos \
                     WHERE user_id = ?1 AND bot_id = ?2 AND status = 'done' \
                       AND updated_at >= ?3 AND updated_at <= ?4 \
                     ORDER BY updated_at DESC LIMIT 30",
                )
                .map_err(map_sqlite)?;
            let completed_todos = stmt
                .query_map(params![u, b, w.from, w.now], |r| {
                    let tags: String = r.get(1)?;
                    Ok((
                        r.get(0)?,
                        serde_json::from_str::<Vec<String>>(&tags).unwrap_or_default(),
                    ))
                })
                .map_err(map_sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite)?;

            // 持ち越しタスク: open かつ期限が期間内（上限30）。
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
                .query_map(params![u, b, w.from_day, w.now_day], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                })
                .map_err(map_sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite)?;

            // これからの予定: 今〜次回の配信予定時刻に始まるもの（上限30）。
            let mut stmt = conn
                .prepare(
                    "SELECT title, start_at FROM schedules \
                     WHERE user_id = ?1 AND bot_id = ?2 AND start_at >= ?3 AND start_at <= ?4 \
                     ORDER BY start_at ASC LIMIT 30",
                )
                .map_err(map_sqlite)?;
            let schedules = stmt
                .query_map(params![u, b, w.now, w.until], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(map_sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite)?;

            // 支払い予定: 前回の配信日〜次回の配信日に期日を迎えるもの（済み・未払いの両方・上限20）。
            let mut stmt = conn
                .prepare(
                    "SELECT title, amount, status, due_date FROM planned_payments \
                     WHERE user_id = ?1 AND bot_id = ?2 \
                       AND date(due_date) >= date(?3) AND date(due_date) <= date(?4) \
                     ORDER BY due_date ASC LIMIT 20",
                )
                .map_err(map_sqlite)?;
            let payments = stmt
                .query_map(params![u, b, w.from_day, w.until_day], |r| {
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

            // 家計: 期間の収支合計。
            let total = |kind: &str| -> Result<i64, DbError> {
                conn.query_row(
                    "SELECT COALESCE(SUM(amount), 0) FROM expenses \
                     WHERE user_id = ?1 AND bot_id = ?2 AND type = ?3 \
                       AND date >= ?4 AND date <= ?5",
                    params![u, b, kind, w.from_day, w.now_day],
                    |r| r.get::<_, i64>(0),
                )
                .map_err(map_sqlite)
            };
            let income_total = total("income")?;
            let expense_total = total("expense")?;

            Ok(ActivityData {
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

/// 補助生成に渡す要約プロンプト。選ばれた内容だけを素材と構成に入れる。
/// 会話トピックを載せないときは会話ログを読まない。
///
/// # Errors
/// 素材の読み取り失敗時 [`DbError`]（呼び出し側は生データ本文へ縮退する）。
pub(crate) async fn summary_prompt(
    db: &Db,
    report: &PeriodicReport,
    window: &ReportWindow,
    data: &ActivityData,
) -> Result<String, DbError> {
    let settings = &report.settings;
    let has = |section| settings.has(section);
    let w = WindowStrings::new(window);
    let categories = if has(ReportSection::Finance) {
        expense_categories(db, &report.user_id, &report.bot_id, &w).await?
    } else {
        Vec::new()
    };
    // 最新40件（assistantを含む）からユーザー発言を20件、各100文字。guild会話は除外する。
    let messages = if has(ReportSection::Topics) {
        yuuka_conversation::search_messages(
            db,
            &report.user_id,
            &report.bot_id,
            None,
            Some(&w.from),
            Some(&w.now),
            40,
        )
        .await?
    } else {
        Vec::new()
    };

    let mut source = Vec::new();
    // 活動データを全部選んだときの構成は旧日報・週報と同じ「1)タスクの進捗 2)予定・支払いのトピック
    // 3)家計サマリ 4)主な会話トピック 5)ひとことコメント」になる。
    let mut outline = Vec::new();
    if has(ReportSection::Tasks) {
        outline.push("タスクの進捗");
        source.push(format!("# 完了タスク（{}件）", data.completed_todos.len()));
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
    }
    match (has(ReportSection::Schedules), has(ReportSection::Payments)) {
        (true, true) => outline.push("予定・支払いのトピック"),
        (true, false) => outline.push("予定のトピック"),
        (false, true) => outline.push("支払いのトピック"),
        (false, false) => {}
    }
    if has(ReportSection::Schedules) {
        source.push(format!(
            "# これからの予定（次回の報告まで・{}件）",
            data.schedules.len()
        ));
        source.extend(
            data.schedules
                .iter()
                .map(|(title, start)| format!("- {title} ({start})")),
        );
    }
    if has(ReportSection::Payments) {
        source.push("# 支払い予定".to_owned());
        source.extend(data.payments.iter().map(|p| {
            format!(
                "- {} {}円 期日{} 状態:{}",
                p.title, p.amount, p.due_date, p.status
            )
        }));
    }
    if has(ReportSection::Finance) {
        outline.push("家計サマリ");
        source.push(format!(
            "# 家計: 収入{}円 / 支出{}円",
            data.income_total, data.expense_total
        ));
        source.extend(
            categories
                .iter()
                .map(|(category, total)| format!("- {category}: {total}円")),
        );
    }
    if has(ReportSection::Topics) {
        outline.push("主な会話トピック（プライバシーに配慮し詳細は省く）");
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
    }
    outline.push("ひとことコメント");
    let outline = outline
        .iter()
        .enumerate()
        .map(|(i, item)| format!("{}){item}", i + 1))
        .collect::<Vec<_>>()
        .join(" ");
    // 活動データの一部だけを選んだときは、構成に無い話題（例: 家計）へ触れさせない。
    let all_activity = ReportSection::ALL
        .into_iter()
        .filter(|s| s.is_activity())
        .all(has);
    let scope_note = if all_activity {
        ""
    } else {
        "構成に無い内容には触れないでください。"
    };
    Ok(format!(
        "以下は定期報告「{}」（対象期間: {}）の活動データです。\
         Discord通知用のサマリを日本語で簡潔に生成してください（800文字以内）。\
         構成: {outline}。{scope_note}\
         サマリ本文のみを出力してください。\n\n{}",
        settings.name,
        window.label(),
        source.join("\n")
    ))
}

/// 期間のカテゴリ別支出（多い順）。要約素材の家計内訳に使う。
async fn expense_categories(
    db: &Db,
    user_id: &str,
    bot_id: &str,
    w: &WindowStrings,
) -> Result<Vec<(String, i64)>, DbError> {
    let (u, b) = (user_id.to_owned(), bot_id.to_owned());
    let (from_day, now_day) = (w.from_day.clone(), w.now_day.clone());
    db.read
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
                .query_map(params![u, b, from_day, now_day], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                })
                .map_err(map_sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite)?;
            Ok(rows)
        })
        .await
}

/// 補助生成が使えないときの生データ本文（Node `buildFallbackText` 相当）。選ばれた内容だけを載せる。
/// 会話トピックは生データには載せない（補助生成のまとめだけが扱う）。
pub(crate) fn raw_body(data: &ActivityData, settings: &ReportSettings) -> String {
    let mut lines: Vec<String> = Vec::new();
    if settings.has(ReportSection::Tasks) {
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
    }
    if settings.has(ReportSection::Schedules) {
        lines.push(format!("📅 これからの予定: {}件", data.schedules.len()));
        for (title, start) in data.schedules.iter().take(8) {
            lines.push(format!("  ・{title} ({start})"));
        }
    }
    if settings.has(ReportSection::Payments) && !data.payments.is_empty() {
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
    if settings.has(ReportSection::Finance) {
        lines.push(format!(
            "💰 収支: 収入 {} / 支出 {}",
            format_currency(data.income_total),
            format_currency(data.expense_total)
        ));
    }
    if lines.is_empty() {
        // 会話トピックだけを選び、まとめも作れなかったとき。
        return "この期間に報告する内容はありません。".to_owned();
    }
    lines.join("\n")
}

/// 件数のインラインフィールド（タスク・家計を選んだときだけ）。
pub(crate) fn count_fields(data: &ActivityData, settings: &ReportSettings) -> Vec<EmbedFieldPart> {
    let mut fields = Vec::new();
    if settings.has(ReportSection::Tasks) {
        fields.push(EmbedFieldPart {
            name: "✅ 完了".to_owned(),
            value: format!("{}件", data.completed_todos.len()),
            inline: true,
        });
        fields.push(EmbedFieldPart {
            name: "📌 持ち越し".to_owned(),
            value: format!("{}件", data.carry_over_todos.len()),
            inline: true,
        });
    }
    if settings.has(ReportSection::Finance) {
        fields.push(EmbedFieldPart {
            name: "💰 収支".to_owned(),
            value: format!(
                "+{} / -{}",
                format_currency(data.income_total),
                format_currency(data.expense_total)
            ),
            inline: true,
        });
    }
    fields
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
    use chrono::TimeZone;

    use super::*;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .expect("valid local time")
    }

    #[test]
    fn window_follows_schedule() {
        // 毎朝 7 時の定時配信: 前日 7 時〜今、これからは翌朝 7 時まで。
        let w = ReportWindow::for_schedule("0 7 * * *", at(2026, 10, 27, 7, 0));
        assert_eq!(w.from, at(2026, 10, 26, 7, 0));
        assert_eq!(w.until, at(2026, 10, 28, 7, 0));
        assert_eq!(w.label(), "2026/10/27");
        // 毎週日曜 21 時: 1 週間分の範囲表記。
        let w = ReportWindow::for_schedule("0 21 * * 0", at(2026, 10, 25, 21, 0));
        assert_eq!(w.from, at(2026, 10, 18, 21, 0));
        assert_eq!(w.label(), "10/18 〜 10/25");
        // 壊れた式は前後 24 時間。
        let now = at(2026, 10, 27, 12, 0);
        let w = ReportWindow::for_schedule("bad", now);
        assert_eq!(w.now - w.from, Duration::days(1));
        assert_eq!(w.until - w.now, Duration::days(1));
    }

    #[test]
    fn format_currency_matches_node_locale() {
        assert_eq!(format_currency(0), "¥0");
        assert_eq!(format_currency(1500), "¥1,500");
        assert_eq!(format_currency(80000), "¥80,000");
        assert_eq!(format_currency(1234567), "¥1,234,567");
        assert_eq!(format_currency(-500), "¥-500");
    }
}
