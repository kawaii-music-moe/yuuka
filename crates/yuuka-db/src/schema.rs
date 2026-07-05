//! スキーマ互換ガード（移行期の破壊経路封じ込め・§11.4 / R-4）。
//!
//! **移行期間中、スキーマ移行の権限は Node に一元化**する。Rust の yuuka-db は
//! 起動時に `system_settings` の `schema_version` を読み、期待固定値 `"17"` と
//! 一致しなければ [`DbError::Migration`] で fail-fast する。**Rust は DDL を一切
//! 発行しない**（レガシー全 DROP 分岐も移植しない）。これで「Rust が system_settings
//! を引き継がず初期化 → 本番テーブル全 DROP」の事故が構造的に不可能になる。

use rusqlite::Connection;
use yuuka_core::DbError;

/// 現行 Node スキーマの固定バージョン（`src/db/migrations.ts` の `SCHEMA_VERSION`）。
pub const EXPECTED_SCHEMA_VERSION: &str = "17";

/// DB スキーマが Rust の期待版と互換かを確認する（起動時のみ・fail-fast）。
///
/// `system_settings` の欠落・読み取り失敗・版数不一致はいずれも [`DbError::Migration`]
/// として起動を止める（回復不能＝起動時の致命に該当）。DDL は一切発行しない。
///
/// # Errors
/// `schema_version` が読めない、または `"17"` と一致しない場合 [`DbError::Migration`]。
pub fn assert_schema_compatible(conn: &Connection) -> Result<(), DbError> {
    let found: String = conn
        .query_row(
            "SELECT value FROM system_settings WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => DbError::Migration {
                expected: EXPECTED_SCHEMA_VERSION.to_owned(),
                found: "<absent>".to_owned(),
            },
            // system_settings 不在（no such table）等も含め、確認不能は起動を止める。
            other => DbError::Migration {
                expected: EXPECTED_SCHEMA_VERSION.to_owned(),
                found: format!("<unreadable: {other}>"),
            },
        })?;

    if found == EXPECTED_SCHEMA_VERSION {
        tracing::info!(schema_version = %found, "db schema compatible");
        Ok(())
    } else {
        Err(DbError::Migration {
            expected: EXPECTED_SCHEMA_VERSION.to_owned(),
            found,
        })
    }
}
