//! JST 日境界のヘルパ（issue #43: PWA のカレンダー・家計は UTC 基準で 1 日ずれていた）。
//!
//! PWA クライアント（PR #63・`localDateKey`/`localMonthKey`）はブラウザのローカル日付
//! （`YYYY-MM-DD`/`YYYY-MM`）を送る。サーバー（本番デプロイは JST 固定・`Dockerfile` の
//! `TZ=Asia/Tokyo`）はそれを**そのままローカル暦日**として扱えばよく、UTC 変換は不要。
//! SQLite 側の列（`expenses.date`・`schedules.start_at` 等）も同じくローカル暦文字列で保存されて
//! いるため（他ドメインの `date('now','localtime')` 等と同じ規約）、文字列比較がそのまま JST の
//! 日境界と一致する。
//!
//! 日時（時刻つき）の入出力変換は `yuuka_core::datetime`（`browser_local_to_db` / `db_to_utc_iso`）に
//! 共通化した（D05）。ここには PWA の日付・月キーと範囲境界だけを置く。

use chrono::NaiveDate;

/// サーバーのローカル暦（本番は JST 固定）で `YYYY-MM-DD` を厳密パースする。
///
/// 存在しない日付（例: `2026-02-30`）・書式違反は `None`（呼び出し側で 400 に倒す）。
pub fn parse_local_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}

/// `YYYY-MM` を厳密パースする（`1` 日を補って [`NaiveDate`] にする・月境界計算の起点用）。
pub fn parse_local_month(s: &str) -> Option<(i32, u32)> {
    let s = s.trim();
    let (y, m) = s.split_once('-')?;
    if y.len() != 4 || m.len() != 2 {
        return None;
    }
    let year: i32 = y.parse().ok()?;
    let month: u32 = m.parse().ok()?;
    if !(1..=12).contains(&month) {
        return None;
    }
    Some((year, month))
}

/// `date` の「翌日 0 時」を `'YYYY-MM-DD HH:MM:SS'` で返す（範囲クエリの排他的上限・issue #43:
/// 文字列 `<=` 比較だと最終日の予定が抜け落ちる旧 Node バグを避ける）。
pub fn day_start(date: NaiveDate) -> String {
    format!("{} 00:00:00", date.format("%Y-%m-%d"))
}

/// [`day_start`] の翌日版（範囲の排他的上限）。
pub fn next_day_start(date: NaiveDate) -> String {
    let next = date.succ_opt().unwrap_or(date);
    day_start(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_local_date_rejects_invalid() {
        assert!(parse_local_date("2026-09-24").is_some());
        assert!(parse_local_date("2026-02-30").is_none(), "存在しない日付");
        assert!(parse_local_date("not-a-date").is_none());
        assert!(parse_local_date("2026-13-01").is_none(), "月が範囲外");
        assert!(parse_local_date("").is_none());
    }

    #[test]
    fn parse_local_month_rejects_invalid() {
        assert_eq!(parse_local_month("2026-09"), Some((2026, 9)));
        assert!(parse_local_month("2026-13").is_none());
        assert!(parse_local_month("2026").is_none());
    }

    #[test]
    fn next_day_start_rolls_over_month_and_year() {
        assert_eq!(
            next_day_start(parse_local_date("2026-09-30").unwrap()),
            "2026-10-01 00:00:00"
        );
        assert_eq!(
            next_day_start(parse_local_date("2026-12-31").unwrap()),
            "2027-01-01 00:00:00"
        );
    }
}
