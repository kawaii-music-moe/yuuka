//! `trigger_at` の DB 格納形式正規化（Node `src/utils/datetime.ts` `toDbDateTime` パリティ）。
//!
//! **B4（確定バグ）**: LLM は `addReminder` の `trigger_at` を Gemini schema 指定の
//! `YYYY-MM-DDTHH:MM:SS`（**T 区切り**）で生成し、Web フロントの `datetime-local` も `T` 区切りを送る。
//! これを生のまま `reminders.trigger_at` に INSERT すると、cron の期限判定
//! `trigger_at <= datetime('now','localtime')` が **空白区切り**（`YYYY-MM-DD HH:MM:SS`）と
//! **字句比較**される。`'T'`(0x54) > `' '`(0x20) のため、当日の過ぎた時刻でも `<=` が成立せず、
//! 翌日の日付境界までリマインドが**発火しない/遅延**する（sqlite 実測・LLM/Web 両経路）。
//!
//! 対策として、Node `reminderRepo.add` と同じく **DB 境界で正規化**する
//! （区切りを空白へ揃え `'YYYY-MM-DD HH:MM:SS'` に統一）。既に DB 形式の入力に対しては冪等。

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime};

/// DB 出力形式（他モジュール・`datetime('now','localtime')` と一致）。
const DB_FMT: &str = "%Y-%m-%d %H:%M:%S";

/// `YYYY-MM-DD` / `YYYY-MM-DD[ T]HH:MM[:SS][.f]` / タイムゾーン付き ISO を DB 格納形式
/// `'YYYY-MM-DD HH:MM:SS'`（ローカル壁時計）へ正規化する。
///
/// Node `toDbDateTime`（`new Date(...)` → local `getHours()` 等で再整形）パリティ:
/// - 日付のみ（`YYYY-MM-DD`）は**ローカル 0 時**（`... 00:00:00`）として解釈する。
/// - **タイムゾーン付き**（`...Z` / `...+09:00`）は tz を解釈して**ローカル壁時計へ変換**する
///   （Node と同じ実時刻シフト）。
/// - オフセット無しの日時は区切り（`T`/空白）を空白へ揃え、秒（省略時 `00`）・小数秒を許容する。
///
/// 解釈できない入力は `None`（呼び出し側で検証エラー＝400 に倒す）。実在しない日付（`2026-02-30` 等）も
/// chrono の厳密パースにより `None`（Node の `NaN→throw` に相当）。
#[must_use]
pub fn to_db_datetime(input: &str) -> Option<String> {
    let v = input.trim();
    if v.is_empty() {
        return None;
    }
    // 日付のみ → ローカル 0 時（Node: `new Date('YYYY-MM-DDT00:00:00')`）。
    if let Ok(date) = NaiveDate::parse_from_str(v, "%Y-%m-%d") {
        return Some(date.and_hms_opt(0, 0, 0)?.format(DB_FMT).to_string());
    }
    // タイムゾーン付き（`Z` / `±HH:MM`）→ ローカル壁時計へ変換（Node の tz シフトと一致）。
    if let Ok(dt) = DateTime::parse_from_rfc3339(v) {
        return Some(dt.with_timezone(&Local).format(DB_FMT).to_string());
    }
    // オフセット無し → 区切りを空白へ揃え（先頭 `T` 1 個のみ）、秒/小数秒の有無を許容して解釈。
    let normalized = v.replacen('T', " ", 1);
    for fmt in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(&normalized, fmt) {
            return Some(dt.format(DB_FMT).to_string());
        }
    }
    None
}

/// [`to_db_datetime`] を試み、解釈できなければ原文（trim なし）をそのまま返す（非破壊フォールバック）。
///
/// repo 境界で使う: 正規化できる入力（LLM/フロントの通常入力）は字句比較バグを解消し、
/// 想定外の入力は**既存挙動を維持**して回帰を作らない（Node は例外で弾くが、DB 層では原文保存に倒す）。
#[must_use]
pub fn normalize_or_keep(input: &str) -> String {
    to_db_datetime(input).unwrap_or_else(|| input.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{normalize_or_keep, to_db_datetime};

    #[test]
    fn iso_t_separator_is_converted_to_space() {
        // 確定バグの核心: LLM の T 区切り ISO を空白区切り DB 形式へ。
        assert_eq!(
            to_db_datetime("2026-07-12T09:00:00").as_deref(),
            Some("2026-07-12 09:00:00")
        );
    }

    #[test]
    fn already_db_format_is_idempotent() {
        assert_eq!(
            to_db_datetime("2999-01-01 09:00:00").as_deref(),
            Some("2999-01-01 09:00:00")
        );
    }

    #[test]
    fn date_only_becomes_local_midnight() {
        assert_eq!(
            to_db_datetime("2026-07-12").as_deref(),
            Some("2026-07-12 00:00:00")
        );
    }

    #[test]
    fn seconds_are_optional() {
        // datetime-local（秒なし）→ 秒を 00 補完。
        assert_eq!(
            to_db_datetime("2026-07-12T09:30").as_deref(),
            Some("2026-07-12 09:30:00")
        );
    }

    #[test]
    fn lexical_ordering_after_normalization_is_correct() {
        // 正規化後は cron の `<= datetime('now','localtime')` 字句比較が正しく効く。
        let trigger = to_db_datetime("2026-07-12T09:00:00").unwrap();
        let now = "2026-07-12 15:00:00"; // 実時刻は trigger より後。
        assert!(trigger.as_str() <= now, "正規化後は字句比較が時系列と一致する");
    }

    #[test]
    fn unparseable_returns_none_but_keep_keeps_raw() {
        assert_eq!(to_db_datetime("later"), None);
        assert_eq!(to_db_datetime("2026-02-30T00:00:00"), None); // 実在しない日付。
        assert_eq!(normalize_or_keep("later"), "later"); // 非破壊フォールバック。
    }

    #[test]
    fn timezone_suffix_is_accepted_and_space_formatted() {
        // #6/B4: Z / オフセット付きも受理し、空白区切り DB 形式（T/Z を含まない）へ整形する。
        // 実時刻はローカル tz 依存のため、形（19 桁・10 桁目が空白・T/Z 無し）だけ検証する。
        for input in [
            "2026-07-12T09:00:00Z",
            "2026-07-12T09:00:00+09:00",
            "2026-07-12T09:00:00.500Z",
        ] {
            let out = to_db_datetime(input).expect("tz-suffixed input should normalize");
            assert_eq!(out.len(), 19, "DB 形式は 19 桁: {out}");
            assert_eq!(out.as_bytes().get(10), Some(&b' '), "10 桁目は空白: {out}");
            assert!(!out.contains('T') && !out.contains('Z'), "T/Z を含まない: {out}");
        }
    }

    #[test]
    fn fractional_seconds_without_offset_are_truncated() {
        assert_eq!(
            to_db_datetime("2026-07-12T09:30:15.250").as_deref(),
            Some("2026-07-12 09:30:15")
        );
    }
}
