//! スキーママイグレーションランナー。
//!
//! Phase 5 (最終カットオーバー) で導入され、以降の DDL 権限を Rust 側が持つ。
//! refinery による前方専用・非破壊のマイグレーションを実行する。
//!
//! V17 までのスキーマは `V17__baseline.sql` で冪等作成されるため、
//! 既存データベース（Node が作成）でも新規作成でも正しく適用され、
//! 以後 `refinery_schema_history` で追跡される。

use rusqlite::Connection;
use yuuka_core::DbError;

mod embedded {
    refinery::embed_migrations!("migrations");
}

/// DB スキーマのマイグレーションを実行する。
///
/// 起動時に `WriterHandle` の単一コネクションから呼ばれ、必要な DDL を適用する。
///
/// # Errors
/// マイグレーション実行に失敗した場合 [`DbError::Migration`]。
pub fn run_migrations(conn: &mut Connection) -> Result<(), DbError> {
    if let Err(e) = embedded::migrations::runner().run(conn) {
        let msg = e.to_string();
        // 起動時の一過性ロック（別コネクションが一時的に write ロックを保持）を
        // **恒久 Migration 失敗（Fatal＝プロセス即終了）に誤判定しない**。SQLITE_BUSY/LOCKED は
        // 回復可能なので Busy（Transient＝再試行対象）へ分類し、スキーマ非互換等の本当の
        // 恒久障害のみ Migration に落とす（絶対制約2・自己復帰）。
        let lower = msg.to_ascii_lowercase();
        if lower.contains("database is locked") || lower.contains("is busy") {
            tracing::warn!(error = %msg, "db migrations: 一過性ロックを検出。再試行可能な Busy として返します");
            return Err(DbError::Busy);
        }
        return Err(DbError::Migration {
            expected: "success".to_owned(),
            found: format!("refinery error: {msg}"),
        });
    }

    tracing::info!("db migrations applied successfully");
    Ok(())
}
