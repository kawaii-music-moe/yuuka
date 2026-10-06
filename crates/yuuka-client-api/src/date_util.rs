//! JST 日境界のヘルパ（issue #43: PWA のカレンダー・家計は UTC 基準で 1 日ずれていた）。
//!
//! PWA クライアント（PR #63・`localDateKey`/`localMonthKey`）はブラウザのローカル日付
//! （`YYYY-MM-DD`/`YYYY-MM`）を送る。サーバー（本番デプロイは JST 固定・`Dockerfile` の
//! `TZ=Asia/Tokyo`）はそれを**そのままローカル暦日**として扱えばよく、UTC 変換は不要。
//! SQLite 側の列（`expenses.date`・`schedules.start_at` 等）も同じくローカル暦文字列で保存されて
//! いるため（他ドメインの `date('now','localtime')` 等と同じ規約）、文字列比較がそのまま JST の
//! 日境界と一致する。

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, SecondsFormat, TimeZone, Utc};

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

/// ブラウザのローカル日時（`YYYY-MM-DDTHH:MM`、秒付きも可）を SQLite の `'YYYY-MM-DD HH:MM:SS'`
/// （ローカル暦）へ変換する。書式違反・存在しない日時は `None`（呼び出し側で 400 に倒す）。
pub fn parse_local_datetime(s: &str) -> Option<String> {
    let s = s.trim();
    let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S"))
        .ok()?;
    Some(naive.format("%Y-%m-%d %H:%M:%S").to_string())
}

/// SQLite の `'YYYY-MM-DD HH:MM:SS'`（ローカル・本番は JST）を UTC の ISO8601（`Z` 終端・
/// ミリ秒 3 桁）へ変換する（Node `new Date(value.replace(" ","T")).toISOString()` パリティ:
/// Node はタイムゾーン無し日時文字列をサーバーのローカル TZ で解釈してから UTC の
/// `toISOString()` を返す）。
///
/// パース不能（壊れた行）は `None`（呼び出し側は行をスキップし 500 にしない・issue #47）。
pub fn local_to_utc_iso(value: &str) -> Option<String> {
    let naive = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S").ok()?;
    let offset = FixedOffset::east_opt(9 * 3600)?; // JST = UTC+9（本番デプロイの固定 TZ）。
    let local: DateTime<FixedOffset> = offset.from_local_datetime(&naive).single()?;
    let utc: DateTime<Utc> = local.with_timezone(&Utc);
    Some(utc.to_rfc3339_opts(SecondsFormat::Millis, true))
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
    fn parse_local_datetime_converts_to_sqlite_format() {
        assert_eq!(
            parse_local_datetime("2026-10-06T09:30").as_deref(),
            Some("2026-10-06 09:30:00")
        );
        assert_eq!(
            parse_local_datetime("2026-10-06T09:30:15").as_deref(),
            Some("2026-10-06 09:30:15")
        );
        assert!(
            parse_local_datetime("2026-02-30T09:00").is_none(),
            "存在しない日付"
        );
        assert!(parse_local_datetime("2026-10-06").is_none(), "時刻なし");
        assert!(parse_local_datetime("").is_none());
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

    #[test]
    fn local_to_utc_iso_shifts_by_jst_offset() {
        // 9/24 10:00 JST → 9/24 01:00Z（issue #43 再現ケース: 旧バグは 9/25 枠に表示されていた）。
        assert_eq!(
            local_to_utc_iso("2026-09-24 10:00:00").as_deref(),
            Some("2026-09-24T01:00:00.000Z")
        );
        // 0時台の JST は前日 UTC へ繰り下がる。
        assert_eq!(
            local_to_utc_iso("2026-09-24 05:00:00").as_deref(),
            Some("2026-09-23T20:00:00.000Z")
        );
    }

    #[test]
    fn local_to_utc_iso_is_none_for_garbage() {
        assert_eq!(local_to_utc_iso("not-a-datetime"), None);
        assert_eq!(local_to_utc_iso(""), None);
    }
}
