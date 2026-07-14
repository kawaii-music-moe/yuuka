//! cron 式ユーティリティ（現行 `cron-parser` / `node-cron` とのパリティ）。
//!
//! yuuka の cron スケジュールとユーザー由来の `repeat_rule` は **5-field**（分 時 日 月 曜日）で、
//! ローカル壁時計で評価される（node-cron はサーバローカル時刻で発火）。croner の
//! `find_next_occurrence(_, inclusive=false)`（= 厳密に後）を使い、`cron-parser` の `.next()`
//! （currentDate を含まない次回）と意味を合わせる。日時は他モジュールと同じ DB 文字列
//! `'YYYY-MM-DD HH:MM:SS'`（ローカル）／`'YYYY-MM-DD'` で入出力する。

use chrono::{DateTime, Local, NaiveDate, TimeZone};
use croner::Cron;

/// 5-field cron 式を parse する（失敗は `None`＝壊れた式は呼び出し側でスキップ）。
fn parse(expr: &str) -> Option<Cron> {
    Cron::new(expr).parse().ok()
}

/// `after` より **厳密に後**の次回発火ローカル時刻（`cron-parser` `.next()` パリティ）。
#[must_use]
pub fn next_after(expr: &str, after: DateTime<Local>) -> Option<DateTime<Local>> {
    parse(expr)?.find_next_occurrence(&after, false).ok()
}

/// DB 保存形式 `'YYYY-MM-DD HH:MM:SS'`（ローカル）へ整形する（Node `toDbDateTime` と同形）。
#[must_use]
pub fn to_db_datetime(dt: DateTime<Local>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// リマインド `repeat_rule` の次回 `trigger_at`（`now` 起点・DB 文字列）。
///
/// 現行 reminderEngine: `CronExpressionParser.parse(rule, {currentDate: now}).next()` → `toDbDateTime`。
/// 壊れた式は `None`（呼び出し側は単発扱い＝ `markSent` で打ち切る）。
#[must_use]
pub fn next_reminder_trigger(expr: &str, now: DateTime<Local>) -> Option<String> {
    Some(to_db_datetime(next_after(expr, now)?))
}

/// ローカル 0 時へ丸める。
fn local_midnight(dt: DateTime<Local>) -> Option<DateTime<Local>> {
    let naive = dt.date_naive().and_hms_opt(0, 0, 0)?;
    Local.from_local_datetime(&naive).single()
}

/// `from`（`'YYYY-MM-DD'` またはその接頭辞を持つ datetime）を起点カーソル（ローカル 0 時）に解く。
/// 解釈不能なら `today` の 0 時（Node は `NaN→now`。ここは安全側で当日 0 時に寄せる）。
fn parse_due_cursor(from: &str, today: DateTime<Local>) -> Option<DateTime<Local>> {
    let date_part = from.get(0..10).unwrap_or(from);
    match NaiveDate::parse_from_str(date_part, "%Y-%m-%d") {
        Ok(d) => Local.from_local_datetime(&d.and_hms_opt(0, 0, 0)?).single(),
        Err(_) => local_midnight(today),
    }
}

/// 繰り返し（todo ルーチン / 支払い予定）の次回期日 `'YYYY-MM-DD'`。
///
/// 現行 `calcNextRecurringDueDate`: `from_due_date` を起点に次回を求め、長期停止で複数周期を
/// 跨いだ場合は **今日以降**の直近まで最大 1000 回読み飛ばす（過去期日の量産を防ぐ）。
/// 壊れた式・収束不能は `None`。
#[must_use]
pub fn next_recurring_due_date(
    expr: &str,
    from_due_date: &str,
    today: DateTime<Local>,
) -> Option<String> {
    let cron = parse(expr)?;
    let today0 = local_midnight(today)?;
    let mut cursor = parse_due_cursor(from_due_date, today)?;
    for _ in 0..1000 {
        let next = cron.find_next_occurrence(&cursor, false).ok()?;
        if next >= today0 {
            return Some(next.format("%Y-%m-%d").to_string());
        }
        cursor = next;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .expect("valid local time")
    }

    #[test]
    fn daily_next_is_strictly_after() {
        // "0 8 * * *"（毎朝8時）。8:00 ちょうどからの次回は翌日 8:00（inclusive=false）。
        let base = at(2026, 7, 8, 8, 0);
        let next = next_after("0 8 * * *", base).expect("has next");
        assert_eq!(to_db_datetime(next), "2026-07-09 08:00:00");
    }

    #[test]
    fn every_minute_advances_one_minute() {
        let base = at(2026, 7, 8, 8, 30);
        let next = next_after("* * * * *", base).expect("has next");
        assert_eq!(to_db_datetime(next), "2026-07-08 08:31:00");
    }

    #[test]
    fn reminder_trigger_formats_db_string() {
        // 現在 10:00、毎時 15 分 → 10:15。
        let now = at(2026, 7, 8, 10, 0);
        assert_eq!(
            next_reminder_trigger("15 * * * *", now).as_deref(),
            Some("2026-07-08 10:15:00")
        );
    }

    #[test]
    fn broken_cron_is_none() {
        assert!(next_after("not a cron", at(2026, 7, 8, 8, 0)).is_none());
        assert!(next_reminder_trigger("99 99 * * *", at(2026, 7, 8, 8, 0)).is_none());
    }

    #[test]
    fn recurring_skips_past_periods_to_today() {
        // 月次「毎月1日」。起点 2026-01-01、今日 2026-07-08 → 過去周期を飛ばし 2026-08-01。
        let today = at(2026, 7, 8, 9, 0);
        let next = next_recurring_due_date("0 0 1 * *", "2026-01-01", today).expect("has next");
        assert_eq!(next, "2026-08-01");
    }

    #[test]
    fn recurring_from_recent_due_returns_next_period() {
        // 毎日繰り返し・起点は今日、今日基準 → 翌日。
        let today = at(2026, 7, 8, 9, 0);
        let next = next_recurring_due_date("0 0 * * *", "2026-07-08", today).expect("has next");
        assert_eq!(next, "2026-07-09");
    }

    #[test]
    fn recurring_broken_rule_is_none() {
        let today = at(2026, 7, 8, 9, 0);
        assert!(next_recurring_due_date("garbage", "2026-07-08", today).is_none());
    }
}
