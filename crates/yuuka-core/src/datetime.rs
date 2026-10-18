//! 日時の解釈・保存形式変換の共通層（D05: 以前は reminder / schedule / client-api / cron_util に
//! ほぼ同じ変換が並存し、受理形式とオフセットの扱いが少しずつ違っていた）。
//!
//! DB の日時列（`reminders.trigger_at`・`schedules.start_at` 等）は **ローカル暦**の
//! `'YYYY-MM-DD HH:MM:SS'` で保存し、`datetime('now','localtime')` と字句比較する。入力側の用途差は
//! 関数名で区別し、ここで一元管理する。
//!
//! | 関数 | 用途 | 受理形式 | オフセット付き入力 |
//! |---|---|---|---|
//! | [`input_to_db_local`] | リマインド `trigger_at`（LLM・Web） | 日付のみ / `T`・空白区切り（秒・小数秒は任意） / RFC 3339 | サーバーのローカル TZ へ換算（Node `toDbDateTime` パリティ） |
//! | [`input_to_db_jst`] | 予定 `start_at` / `end_at`（LLM） | 同上 | JST 固定へ換算 |
//! | [`browser_local_to_db`] | PWA の `datetime-local` | `YYYY-MM-DDTHH:MM[:SS]` のみ | 受理しない |
//! | [`db_to_utc_iso`] | PWA 向けの出力 | DB 形式のみ | （出力は UTC `...Z`） |
//! | [`local_to_db`] | 実行時に求めた時刻（cron 次回発火など） | `DateTime<Local>` | — |
//!
//! 本番は `TZ=Asia/Tokyo` 固定なので「ローカル TZ」と「JST 固定」は同じ結果になる。JST 固定側は
//! テスト・開発機の TZ に左右されないことを優先した既存の選択で、挙動を変えないためそのまま残す。

use chrono::{
    DateTime, FixedOffset, Local, NaiveDate, NaiveDateTime, SecondsFormat, TimeZone, Utc,
};

/// DB 保存形式（ローカル暦・`datetime('now','localtime')` と同形）。
pub const DB_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

/// オフセット無しの入力で受け付ける書式（`%.f` は小数秒があれば読む・切り捨て）。
const NAIVE_INPUT_FORMATS: [&str; 4] = [
    "%Y-%m-%dT%H:%M:%S%.f",
    "%Y-%m-%dT%H:%M",
    "%Y-%m-%d %H:%M:%S%.f",
    "%Y-%m-%d %H:%M",
];

/// 本番デプロイのローカル暦（JST = UTC+9）。
#[must_use]
pub fn jst() -> Option<FixedOffset> {
    FixedOffset::east_opt(9 * 3600)
}

/// 実行時の時刻を DB 保存形式へ整形する。
#[must_use]
pub fn local_to_db(dt: DateTime<Local>) -> String {
    dt.format(DB_FORMAT).to_string()
}

/// 入力の日時をローカル壁時計の [`NaiveDateTime`] に解く（オフセット付きは `tz` へ換算）。
///
/// 日付のみ（`YYYY-MM-DD`）はその日の 0 時。実在しない日時・解釈できない入力は `None`。
fn parse_input<Tz: TimeZone>(input: &str, tz: &Tz) -> Option<NaiveDateTime> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(tz).naive_local());
    }
    if let Some(naive) = NAIVE_INPUT_FORMATS
        .iter()
        .find_map(|fmt| NaiveDateTime::parse_from_str(s, fmt).ok())
    {
        return Some(naive);
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .ok()?
        .and_hms_opt(0, 0, 0)
}

/// 入力の日時を DB 保存形式へ正規化する（オフセット付きは**サーバーのローカル TZ**へ換算）。
///
/// リマインドの `trigger_at` 用（Node `toDbDateTime` が `new Date(...)` をローカル時刻で再整形するのと
/// 同じ）。`T` 区切りのまま保存すると `trigger_at <= datetime('now','localtime')` の字句比較が
/// 崩れて発火が遅れるため（B4）、区切りを空白へ揃える。既に DB 形式なら冪等。
#[must_use]
pub fn input_to_db_local(input: &str) -> Option<String> {
    Some(parse_input(input, &Local)?.format(DB_FORMAT).to_string())
}

/// 入力の日時を DB 保存形式へ正規化する（オフセット付きは **JST 固定**へ換算）。
///
/// 予定の `start_at` / `end_at` 用。受理形式は [`input_to_db_local`] と同じ。
#[must_use]
pub fn input_to_db_jst(input: &str) -> Option<String> {
    Some(parse_input(input, &jst()?)?.format(DB_FORMAT).to_string())
}

/// ブラウザの `datetime-local`（`YYYY-MM-DDTHH:MM`、秒付きも可）を DB 保存形式へ変換する。
///
/// PWA の API 契約どおり `T` 区切りだけを受け付ける（日付のみ・空白区切り・オフセット付きは `None`
/// → 呼び出し側で 400）。
#[must_use]
pub fn browser_local_to_db(input: &str) -> Option<String> {
    let s = input.trim();
    let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S"))
        .ok()?;
    Some(naive.format(DB_FORMAT).to_string())
}

/// DB 保存形式（JST のローカル暦）を UTC の ISO 8601（`Z` 終端・ミリ秒 3 桁）へ変換する。
///
/// Node `new Date(value.replace(" ","T")).toISOString()` パリティ。パース不能（壊れた行）は `None`
/// （呼び出し側は原文のまま返すか行をスキップする・issue #47）。
#[must_use]
pub fn db_to_utc_iso(value: &str) -> Option<String> {
    let naive = NaiveDateTime::parse_from_str(value, DB_FORMAT).ok()?;
    let local = jst()?.from_local_datetime(&naive).single()?;
    Some(
        local
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_formats_are_shared_between_local_and_jst() {
        for (input, expected) in [
            ("2026-05-28T10:00:00", "2026-05-28 10:00:00"),
            ("2026-05-28T10:00", "2026-05-28 10:00:00"),
            ("2026-05-28T10:00:00.250", "2026-05-28 10:00:00"),
            ("2026-05-28 10:00:00", "2026-05-28 10:00:00"),
            ("2026-05-28 10:00", "2026-05-28 10:00:00"),
            ("2026-05-28 10:00:15.5", "2026-05-28 10:00:15"),
            (" 2026-05-28T10:00:00 ", "2026-05-28 10:00:00"),
            ("2026-05-28", "2026-05-28 00:00:00"),
        ] {
            assert_eq!(
                input_to_db_local(input).as_deref(),
                Some(expected),
                "{input}"
            );
            assert_eq!(input_to_db_jst(input).as_deref(), Some(expected), "{input}");
        }
    }

    #[test]
    fn input_rejects_unparsable_values() {
        for input in [
            "",
            "  ",
            "later",
            "明日の10時",
            "2026-02-30T10:00:00",
            "2026-05-28T25:00",
        ] {
            assert!(input_to_db_local(input).is_none(), "{input}");
            assert!(input_to_db_jst(input).is_none(), "{input}");
        }
    }

    #[test]
    fn jst_input_converts_offsets_to_jst() {
        for (input, expected) in [
            ("2026-05-28T10:00:00+09:00", "2026-05-28 10:00:00"),
            ("2026-05-28T01:00:00Z", "2026-05-28 10:00:00"),
            ("2026-05-28T20:00:00Z", "2026-05-29 05:00:00"),
        ] {
            assert_eq!(input_to_db_jst(input).as_deref(), Some(expected), "{input}");
        }
    }

    #[test]
    fn local_input_converts_offsets_to_server_local() {
        // 結果はサーバーの TZ 次第なので、Local で換算した値と一致することと形だけを確かめる。
        for input in [
            "2026-07-12T09:00:00Z",
            "2026-07-12T09:00:00+09:00",
            "2026-07-12T09:00:00.500Z",
        ] {
            let out = input_to_db_local(input).expect("offset input should normalize");
            let expected = DateTime::parse_from_rfc3339(input)
                .unwrap()
                .with_timezone(&Local)
                .format(DB_FORMAT)
                .to_string();
            assert_eq!(out, expected, "{input}");
            assert_eq!(out.len(), 19, "{out}");
            assert_eq!(out.as_bytes().get(10), Some(&b' '), "{out}");
        }
    }

    #[test]
    fn normalized_input_orders_lexically() {
        // B4: 正規化後は `<= datetime('now','localtime')` の字句比較が時系列と一致する。
        let trigger = input_to_db_local("2026-07-12T09:00:00").unwrap();
        assert!(trigger.as_str() <= "2026-07-12 15:00:00");
    }

    #[test]
    fn browser_local_accepts_only_t_separated() {
        assert_eq!(
            browser_local_to_db("2026-10-06T09:30").as_deref(),
            Some("2026-10-06 09:30:00")
        );
        assert_eq!(
            browser_local_to_db("2026-10-06T09:30:15").as_deref(),
            Some("2026-10-06 09:30:15")
        );
        for input in [
            "2026-02-30T09:00",
            "2026-10-06",
            "2026-10-06 09:30",
            "2026-10-06T09:30:00Z",
            "",
        ] {
            assert!(browser_local_to_db(input).is_none(), "{input}");
        }
    }

    #[test]
    fn db_to_utc_iso_shifts_by_jst_offset() {
        // issue #43 再現ケース: 9/24 10:00 JST → 9/24 01:00Z。
        assert_eq!(
            db_to_utc_iso("2026-09-24 10:00:00").as_deref(),
            Some("2026-09-24T01:00:00.000Z")
        );
        assert_eq!(
            db_to_utc_iso("2026-09-24 05:00:00").as_deref(),
            Some("2026-09-23T20:00:00.000Z")
        );
        assert_eq!(db_to_utc_iso("not-a-datetime"), None);
        assert_eq!(db_to_utc_iso("2026-09-24T10:00:00"), None);
        assert_eq!(db_to_utc_iso(""), None);
    }

    #[test]
    fn local_to_db_formats_storage_string() {
        let dt = Local.with_ymd_and_hms(2026, 7, 9, 8, 0, 0).unwrap();
        assert_eq!(local_to_db(dt), "2026-07-09 08:00:00");
    }
}
