//! 時刻ユーティリティ（jiff・Node `Date` パリティ）。
//!
//! chrono（非メンテ・RUSTSEC-2026-0543）からの移行に伴い、各 crate で重複していた
//! 「UTC・ミリ秒・`Z` の ISO 8601」整形をここへ集約する。

use jiff::Timestamp;

/// ISO 8601（UTC・ミリ秒・`Z`）に整形する（Node `Date#toISOString()` と同形）。
#[must_use]
pub fn iso_millis(ts: Timestamp) -> String {
    ts.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// 現在時刻の ISO 8601（UTC・ミリ秒・`Z`）（Node `new Date().toISOString()`）。
#[must_use]
pub fn now_iso_millis() -> String {
    iso_millis(Timestamp::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_node_to_iso_string() {
        let ts = Timestamp::from_millisecond(1_700_000_000_123).expect("ts");
        assert_eq!(iso_millis(ts), "2023-11-14T22:13:20.123Z");
        let ts = Timestamp::from_second(1_700_000_000).expect("ts");
        assert_eq!(iso_millis(ts), "2023-11-14T22:13:20.000Z");
    }
}
