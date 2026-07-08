//! yuuka-db — rusqlite **単一 writer actor** + **read pool** + **schema 互換ガード**。
//!
//! 並行性ハザードの構造的排除（R-1/R-2・§11.4）:
//! - 書き込みは [`WriterHandle`]（1 タスク・1 コネクションに直列化）へ一本化し、
//!   プロセス内の並行 writer 競合（即-BUSY）を型で排除する。
//! - 読み取りは [`ReadPool`]（deadpool-sqlite、複数リーダー並行）。同期呼び出しは
//!   deadpool の `interact`（内部 `spawn_blocking`）でブロッキングプールへ逃がす。
//! - PRAGMA（WAL / foreign_keys / busy_timeout=5000 / synchronous=NORMAL）は
//!   [`pool::open_conn`] で明示。全書込 Tx は BEGIN IMMEDIATE（[`WriterHandle::transaction`]）。
//! - DDL 所有権は Rust 側に移行済み。`refinery` による前方専用マイグレーションを
//!   適用する（[`schema::run_migrations`]）。

pub mod pool;
pub mod schema;
pub mod writer;

pub use pool::{open_conn, ReadPool};
pub use schema::run_migrations;
pub use writer::WriterHandle;

use yuuka_core::DbError;

/// rusqlite エラーを層別 [`DbError`] へ写像する。
///
/// `SQLITE_BUSY`/`SQLITE_LOCKED` は [`DbError::Busy`]（アプリ層 backon リトライ対象・
/// Phase 1）へ、その他は [`DbError::Operation`] へ。driver 型を core に持ち込まない
/// ため、`#[from]` ではなくここで明示変換する（§4.3）。
///
/// 下流のドメイン repo（yuuka-todo 等）が read/write クロージャ内で使えるよう `pub`。
pub fn map_sqlite(e: rusqlite::Error) -> DbError {
    use rusqlite::ErrorCode;
    if let rusqlite::Error::SqliteFailure(inner, _) = &e {
        if matches!(inner.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) {
            return DbError::Busy;
        }
    }
    DbError::Operation(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        map_sqlite, open_conn, run_migrations, ReadPool, WriterHandle,
    };
    use tempfile::tempdir;
    use yuuka_core::DbError;

    /// 本番では Node が DB を作成する。テストでは書込可能な生コネクションで用意する
    /// （yuuka-db の open_conn は CREATE しないため、対象 DB が既存であることを前提とする）。
    fn seed_db(path: &std::path::Path, ddl: &str) {
        let c = rusqlite::Connection::open(path).unwrap();
        if !ddl.is_empty() {
            c.execute_batch(ddl).unwrap();
        }
    }

    #[test]
    fn open_conn_does_not_create_missing_db() {
        // CREATE を外したので、存在しない DB への open は即エラーになり空 DB を作らない（C-2）。
        let dir = tempdir().unwrap();
        let path = dir.path().join("absent.sqlite");
        assert!(open_conn(&path, false).is_err());
        assert!(!path.exists(), "must not create an empty db file");
    }

    #[test]
    fn read_only_conn_rejects_writes() {
        // READ_ONLY + query_only により read 接続の書込を SQLite 層で拒否する
        // （writer actor を通らない第二 writer 経路を機械排除・C-1）。
        let dir = tempdir().unwrap();
        let path = dir.path().join("ro.sqlite");
        seed_db(&path, "CREATE TABLE t(id INTEGER PRIMARY KEY);");
        let ro = open_conn(&path, true).unwrap();
        assert!(
            ro.execute("INSERT INTO t(id) VALUES(1)", []).is_err(),
            "read-only connection must reject writes"
        );
    }

    #[test]
    fn pragmas_applied_on_open() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("p.sqlite");
        seed_db(&path, "");
        let conn = open_conn(&path, false).unwrap();

        let jm: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
        assert!(jm.eq_ignore_ascii_case("wal"), "journal_mode={jm}");
        let fk: i64 = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0)).unwrap();
        assert_eq!(fk, 1, "foreign_keys must be ON");
    }

    #[test]
    fn run_migrations_succeeds_on_empty_and_existing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("m.sqlite");
        seed_db(&path, "");
        let mut conn = open_conn(&path, false).unwrap();

        // 初回（空 DB からのスキーマ構築）
        run_migrations(&mut conn).unwrap();

        // 2 回目（既存 DB での冪等実行）
        run_migrations(&mut conn).unwrap();
    }

    #[tokio::test]
    async fn writer_serializes_and_reader_reads_back() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("t.sqlite");
        // テスト専用 DDL（本番では Rust は DDL 不発行・schema.rs 参照）。DB と表を先に用意。
        seed_db(&path, "CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT);");

        let writer = WriterHandle::spawn(path.clone()).unwrap();

        // BEGIN IMMEDIATE 経路（R-1 回避）。
        writer
            .transaction(|tx| {
                tx.execute("INSERT INTO items(name) VALUES(?1)", ["hello"])
                    .map_err(map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();

        let pool = ReadPool::open(&path).unwrap();
        let name: String = pool
            .read(|c| {
                c.query_row("SELECT name FROM items WHERE id = 1", [], |r| r.get(0))
                    .map_err(map_sqlite)
            })
            .await
            .unwrap();
        assert_eq!(name, "hello");
    }
}
