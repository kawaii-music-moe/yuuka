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
    // PRAGMA foreign_keys cannot change inside a transaction. Refuse callers that
    // would silently leave CASCADE enabled during the V27 table rebuild.
    if !conn.is_autocommit() {
        return Err(DbError::Migration {
            expected: "migration connection outside a transaction".to_owned(),
            found: "active transaction".to_owned(),
        });
    }
    let result = (|| {
        // V25 relies on ON DELETE CASCADE: keep the original FK mode through V26.
        embedded::migrations::runner()
            .set_target(refinery::Target::Version(26))
            .run(conn)
            .map_err(migration_error)?;
        let last = embedded::migrations::runner()
            .get_last_applied_migration(conn)
            .map_err(migration_error)?;
        if last.is_some_and(|m| m.version() < 27) {
            let foreign_keys: bool = conn
                .pragma_query_value(None, "foreign_keys", |r| r.get(0))
                .map_err(crate::map_sqlite)?;
            conn.pragma_update(None, "foreign_keys", false)
                .map_err(crate::map_sqlite)?;
            let rebuild = embedded::migrations::runner()
                .set_target(refinery::Target::Version(27))
                .run(conn)
                .map_err(migration_error);
            // Restore on both success and failure; V27 checks FK integrity before
            // refinery commits its DDL, data and schema history transaction.
            conn.pragma_update(None, "foreign_keys", foreign_keys)
                .map_err(crate::map_sqlite)?;
            rebuild?;
        }
        // Future migrations must run with normal FK enforcement again.
        embedded::migrations::runner()
            .run(conn)
            .map_err(migration_error)?;
        Ok(())
    })();
    result?;
    tracing::info!("db migrations applied successfully");
    Ok(())
}

fn migration_error(e: refinery::Error) -> DbError {
    let msg = e.to_string();
    // 起動時の一過性ロック（別コネクションが一時的に write ロックを保持）を
    // **恒久 Migration 失敗（Fatal＝プロセス即終了）に誤判定しない**。SQLITE_BUSY/LOCKED は
    // 回復可能なので Busy（Transient＝再試行対象）へ分類し、スキーマ非互換等の本当の
    // 恒久障害のみ Migration に落とす（絶対制約2・自己復帰）。
    let lower = msg.to_ascii_lowercase();
    if lower.contains("database is locked") || lower.contains("is busy") {
        tracing::warn!(error = %msg, "db migrations: 一過性ロックを検出。再試行可能な Busy として返します");
        return DbError::Busy;
    }
    DbError::Migration {
        expected: "success".to_owned(),
        found: format!("refinery error: {msg}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{embedded, run_migrations};
    use rusqlite::Connection;

    fn v26() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        embedded::migrations::runner()
            .set_target(refinery::Target::Version(26))
            .run(&mut conn)
            .unwrap();
        conn
    }

    fn assert_foreign_keys(conn: &Connection, expected: bool) {
        let actual: bool = conn
            .pragma_query_value(None, "foreign_keys", |r| r.get(0))
            .unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn v27_preserves_children_and_restores_foreign_keys_on_repeat() {
        let mut conn = v26();
        conn.execute_batch(
            "INSERT INTO users(discord_id,username,salt) VALUES('u','u','salt');
             INSERT INTO bots(id,user_id,name) VALUES('b','u','bot');
             INSERT INTO todos(id,user_id,bot_id,title) VALUES(1,'u','b','todo');
             INSERT INTO task_progress_logs(user_id,bot_id,todo_id,progress) VALUES('u','b',1,20);
             INSERT INTO mcp_servers(id,user_id,name,endpoint_url) VALUES(1,'u','mcp','https://mcp');
             INSERT INTO bot_mcp_access(bot_id,owner_id,mcp_server_id) VALUES('b','u',1);
             INSERT INTO message_logs(id,user_id,bot_id,role,content) VALUES(1,'u','b','user','needle');
             INSERT INTO message_attachments(message_log_id,name,mime_type,bytes)
               VALUES(1,'a','application/octet-stream',X'00FF');",
        )
        .unwrap();
        for _ in 0..2 {
            run_migrations(&mut conn).unwrap();
            assert_foreign_keys(&conn, true);
            for table in [
                "task_progress_logs",
                "bot_mcp_access",
                "message_attachments",
            ] {
                let count: i64 = conn
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                    .unwrap();
                assert_eq!(count, 1, "{table}");
            }
        }
        assert!(conn
            .execute(
                "INSERT INTO todos(user_id,title) VALUES('u','missing bot')",
                []
            )
            .is_err());
        // Both system-wide and owner-scoped MCP registrations need no Bot column.
        conn.execute_batch(
            "INSERT INTO mcp_servers(user_id,name,endpoint_url) VALUES(NULL,'system','https://system');
             INSERT INTO mcp_servers(user_id,name,endpoint_url) VALUES('u','user','https://user');
             DELETE FROM todos WHERE id=1;
             DELETE FROM mcp_servers WHERE id=1;
             DELETE FROM message_logs WHERE id=1;",
        )
        .unwrap();
        for table in [
            "task_progress_logs",
            "bot_mcp_access",
            "message_attachments",
        ] {
            let count: i64 = conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "CASCADE: {table}");
        }
    }

    #[test]
    fn v27_failure_rolls_back_history_and_restores_fk_then_retries() {
        let mut conn = v26();
        conn.execute_batch(
            "PRAGMA foreign_keys=OFF;
             INSERT INTO task_progress_logs(user_id,bot_id,todo_id,progress)
               VALUES('missing','b',999,0);
             PRAGMA foreign_keys=ON;",
        )
        .unwrap();
        assert!(run_migrations(&mut conn).is_err());
        assert_foreign_keys(&conn, true);
        let version = embedded::migrations::runner()
            .get_last_applied_migration(&mut conn)
            .unwrap()
            .unwrap()
            .version();
        assert_eq!(version, 26);
        let default: String = conn
            .query_row(
                "SELECT dflt_value FROM pragma_table_info('todos') WHERE name='bot_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(default, "'system_default'");
        conn.execute("DELETE FROM task_progress_logs", []).unwrap();
        run_migrations(&mut conn).unwrap();
        assert_foreign_keys(&conn, true);
    }

    #[test]
    fn v27_preserves_disabled_fk_mode_and_rejects_active_transaction() {
        let mut conn = v26();
        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        run_migrations(&mut conn).unwrap();
        assert_foreign_keys(&conn, false);
        conn.execute_batch("BEGIN;").unwrap();
        assert!(run_migrations(&mut conn).is_err());
        assert!(!conn.is_autocommit());
        conn.execute_batch("ROLLBACK;").unwrap();
    }

    #[test]
    fn v25_drops_accounts_not_linked_to_sk_account() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        embedded::migrations::runner()
            .set_target(refinery::Target::Version(24))
            .run(&mut conn)
            .unwrap();
        conn.execute_batch(
            "INSERT INTO users (discord_id, username, password_hash, salt, sk_account_id) VALUES
               ('legacy', 'legacy', '$2b$12$hash', '00', NULL),
               ('linked', 'linked', '', '00', 'acc-1');
             INSERT INTO bots (id, user_id, name) VALUES ('b-legacy', 'legacy', 'b'), ('b-linked', 'linked', 'b');
             INSERT INTO invite_codes (code) VALUES ('code');",
        )
        .unwrap();

        run_migrations(&mut conn).unwrap();

        let users: Vec<String> = conn
            .prepare("SELECT discord_id FROM users ORDER BY discord_id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(users, vec!["linked".to_owned()]);
        let bots: Vec<String> = conn
            .prepare("SELECT id FROM bots ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            bots,
            vec!["b-linked".to_owned()],
            "旧アカウントの所有データも消える"
        );
        let password_col: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('users') WHERE name='password_hash'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(password_col, 0);
    }
}
