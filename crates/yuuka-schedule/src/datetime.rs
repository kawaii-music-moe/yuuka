//! `schedules.start_at` / `end_at` の保存形式の正規化。
//!
//! 一覧・リマインド・PWA カレンダーはどれも `'YYYY-MM-DD HH:MM:SS'`（ローカル暦・本番は JST 固定）
//! 前提で文字列比較／パースする。ところが `addSchedule` ツール（Discord のエージェント）は Gemini が
//! 返す ISO 8601（`2026-05-28T10:00:00`・オフセット付きのことも）を渡してくるので、保存前にここで揃える。

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime};

/// 保存形式（`'YYYY-MM-DD HH:MM:SS'`）。
const STORAGE_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

/// オフセット無しで受け付ける書式（`%.f` は小数秒があれば読む）。
const NAIVE_FORMATS: [&str; 4] = [
    "%Y-%m-%dT%H:%M:%S%.f",
    "%Y-%m-%dT%H:%M",
    "%Y-%m-%d %H:%M:%S%.f",
    "%Y-%m-%d %H:%M",
];

/// ローカル暦のオフセット（本番デプロイは JST 固定・`TZ=Asia/Tokyo`）。
fn local_offset() -> Option<FixedOffset> {
    FixedOffset::east_opt(9 * 3600)
}

/// 日時文字列を保存形式（ローカル暦の `'YYYY-MM-DD HH:MM:SS'`）へ正規化する。
///
/// - オフセット付き ISO 8601（`Z` / `+09:00` 等）はローカル暦（JST）へ換算する。
/// - オフセット無し（`T` 区切り・空白区切り、秒・小数秒の有無を問わず）はそのままローカル暦とみなす。
/// - 日付のみ（`YYYY-MM-DD`）はその日の 00:00:00。
///
/// 解釈できなければ `None`。
#[must_use]
pub fn normalize_local_datetime(raw: &str) -> Option<String> {
    let s = raw.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        let local = dt.with_timezone(&local_offset()?);
        return Some(local.naive_local().format(STORAGE_FORMAT).to_string());
    }
    if let Some(naive) = NAIVE_FORMATS
        .iter()
        .find_map(|fmt| NaiveDateTime::parse_from_str(s, fmt).ok())
    {
        return Some(naive.format(STORAGE_FORMAT).to_string());
    }
    let date = NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    Some(
        date.and_hms_opt(0, 0, 0)?
            .format(STORAGE_FORMAT)
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_iso_and_storage_formats() {
        for (input, expected) in [
            ("2026-05-28T10:00:00", "2026-05-28 10:00:00"),
            ("2026-05-28T10:00", "2026-05-28 10:00:00"),
            ("2026-05-28T10:00:00.250", "2026-05-28 10:00:00"),
            ("2026-05-28 10:00:00", "2026-05-28 10:00:00"),
            ("2026-05-28 10:00", "2026-05-28 10:00:00"),
            (" 2026-05-28T10:00:00 ", "2026-05-28 10:00:00"),
            ("2026-05-28", "2026-05-28 00:00:00"),
            // オフセット付きはローカル暦（JST）へ換算する。
            ("2026-05-28T10:00:00+09:00", "2026-05-28 10:00:00"),
            ("2026-05-28T01:00:00Z", "2026-05-28 10:00:00"),
            ("2026-05-28T20:00:00Z", "2026-05-29 05:00:00"),
        ] {
            assert_eq!(
                normalize_local_datetime(input).as_deref(),
                Some(expected),
                "{input}"
            );
        }
    }

    #[test]
    fn rejects_unparsable_values() {
        for input in ["", "明日の10時", "2026-02-30T10:00:00", "2026-05-28T25:00"] {
            assert!(normalize_local_datetime(input).is_none(), "{input}");
        }
    }
}
