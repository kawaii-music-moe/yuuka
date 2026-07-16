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
        if matches!(
            inner.code,
            ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked
        ) {
            return DbError::Busy;
        }
    }
    DbError::Operation(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::{map_sqlite, open_conn, run_migrations, ReadPool, WriterHandle};
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

        let jm: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert!(jm.eq_ignore_ascii_case("wal"), "journal_mode={jm}");
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
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

    #[test]
    fn baseline_reruns_on_fully_populated_db_and_stamps_schema_version() {
        // 移行期の現実シナリオ: Node が作成した DB（= baseline 相当のオブジェクトのみ・refinery
        // 履歴なし）へ refinery が全マイグレーションを流す。baseline の CREATE 群が IF NOT EXISTS で
        // 冪等でないと「object already exists」で失敗し Rust writer が起動不能になる（#2 のガード）。
        //
        // 注: post-baseline のマイグレーション（V19 の ALTER TABLE ADD COLUMN 等）は SQLite に
        // IF NOT EXISTS が無く再走冪等にできないため、「全オブジェクト既存 + 履歴なし」ではなく
        // 「Node 実スキーマ（baseline のみ既存）+ 履歴なし」を正確に再現する: 履歴を消した上で
        // post-baseline の産物（bot_channels / message_logs.channel_id）も除去してから再走する。
        let dir = tempdir().unwrap();
        let path = dir.path().join("populated.sqlite");
        seed_db(&path, "");
        let mut conn = open_conn(&path, false).unwrap();

        // 1) 全オブジェクトを作成（+ refinery 履歴を刻む）。
        run_migrations(&mut conn).unwrap();

        // 2) refinery 履歴を消し、post-baseline 産物を落として「Node 作成 DB」状態を作る。
        conn.execute_batch(
            "DROP TABLE refinery_schema_history; \
             DROP TABLE bot_channels; \
             DROP INDEX idx_message_logs_guild_channel; \
             ALTER TABLE message_logs DROP COLUMN channel_id;",
        )
        .unwrap();

        // 3) 全体を再走 — baseline は冪等 CREATE、post-baseline は対象不在なので成功する。
        run_migrations(&mut conn).unwrap();

        // #1: schema_version が '17' で刻まれている（Node がこの DB を開いても再 DROP しない）。
        let v: String = conn
            .query_row(
                "SELECT value FROM system_settings WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, "17", "baseline は schema_version='17' を刻印する");

        // post-baseline の産物が再作成されている（V18 / V19）。
        let bot_channels: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='bot_channels'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bot_channels, 1, "V18: bot_channels 再作成");
        let channel_col: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('message_logs') WHERE name='channel_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(channel_col, 1, "V19: message_logs.channel_id 再追加");
    }

    #[tokio::test]
    async fn writer_serializes_and_reader_reads_back() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("t.sqlite");
        // テスト専用 DDL（本番では Rust は DDL 不発行・schema.rs 参照）。DB と表を先に用意。
        seed_db(
            &path,
            "CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT);",
        );

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

    #[tokio::test]
    // panic-isolation を検証するテストは意図的に panic! を使う（clippy::panic はテスト例外
    // 設定が無いため関数単位で許可する。supervisor.rs のテストと同方針）。
    #[allow(clippy::panic)]
    async fn writer_survives_job_panic_m5() {
        // M-5: ジョブ内 panic で writer スレッドが死なず、後続の書き込みが継続できること。
        // catch_unwind が無いと 1 発の panic 以後、全書き込みが恒久 WriterGone になる。
        let dir = tempdir().unwrap();
        let path = dir.path().join("panic.sqlite");
        seed_db(
            &path,
            "CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT);",
        );
        let writer = WriterHandle::spawn(path.clone()).unwrap();

        // 1) panic するジョブ。当該呼び出しは WriterGone を受け取る（sender が unwind で drop）。
        let panicked = writer
            .execute(|_conn| -> Result<(), DbError> { panic!("boom in job") })
            .await;
        assert!(
            matches!(panicked, Err(DbError::WriterGone)),
            "panic した呼び出しは WriterGone を返す: {panicked:?}"
        );

        // 2) writer は生存: 後続の通常書き込みが成功する。
        writer
            .transaction(|tx| {
                tx.execute("INSERT INTO items(name) VALUES(?1)", ["after-panic"])
                    .map_err(map_sqlite)?;
                Ok(())
            })
            .await
            .expect("writer は panic 後も生存して書き込みを処理する");

        let pool = ReadPool::open(&path).unwrap();
        let name: String = pool
            .read(|c| {
                c.query_row(
                    "SELECT name FROM items WHERE name = 'after-panic'",
                    [],
                    |r| r.get(0),
                )
                .map_err(map_sqlite)
            })
            .await
            .unwrap();
        assert_eq!(name, "after-panic");
    }
}
