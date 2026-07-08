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
    embedded::migrations::runner()
        .run(conn)
        .map_err(|e| DbError::Migration {
            expected: "success".to_owned(),
            found: format!("refinery error: {e}"),
        })?;

    tracing::info!("db migrations applied successfully");
    Ok(())
}
