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
    #[test]
    fn v28_preserves_legacy_tags_without_retrospective_tagging() {
        let mut conn = Connection::open_in_memory().unwrap();
        embedded::migrations::runner()
            .set_target(refinery::Target::Version(26))
            .run(&mut conn)
            .unwrap();
        conn.execute_batch(r#"INSERT INTO users (discord_id, username, salt, sk_account_id) VALUES ('u','u','s','acc-u');
            INSERT INTO todos (user_id, title, tags) VALUES ('u','legacy','["業務","独自タグ"]'), ('u','empty','[]'), ('u','bad','broken');"#).unwrap();
        run_migrations(&mut conn).unwrap();
        let rows: Vec<(String, String, i64, i64)> = conn.prepare("SELECT t.tags, s.explicit_tags, s.pending, s.disabled FROM todos t JOIN todo_tagging s ON s.todo_id=t.id ORDER BY t.id").unwrap()
            .query_map([], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap().collect::<Result<_,_>>().unwrap();
        assert_eq!(rows.len(), 3);
        for (tags, explicit, pending, disabled) in rows {
            assert_eq!(tags, explicit);
            assert_eq!(pending, 0);
            assert_eq!(disabled, 0);
        }
        run_migrations(&mut conn).unwrap();
        conn.execute(
            "UPDATE todos SET title = 'edited' WHERE title = 'legacy'",
            [],
        )
        .unwrap();
        let pending: i64 = conn
            .query_row("SELECT sum(pending) FROM todo_tagging", [], |r| r.get(0))
            .unwrap();
        assert_eq!(pending, 1);
    }

    #[test]
    fn v35_moves_briefing_and_report_configs_into_periodic_reports() {
        // 本番と同じく V34 まで進めた DB（V27 は外部キーを切って流す）に旧設定を入れてから V35 を流す。
        let mut conn = v26();
        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        embedded::migrations::runner()
            .set_target(refinery::Target::Version(27))
            .run(&mut conn)
            .unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        embedded::migrations::runner()
            .set_target(refinery::Target::Version(34))
            .run(&mut conn)
            .unwrap();
        conn.execute_batch(
            r#"INSERT INTO users (discord_id, username, salt) VALUES ('u1','u1','s'), ('u2','u2','s'), ('u3','u3','s');
            -- 朝報: 天気+ニュース（チャンネル）/ 地点名だけ（DM）/ 何も無い / フィード JSON 破損だけ。
            INSERT INTO briefing_configs (user_id, bot_id, enabled, schedule_cron, target_type, target_id,
                weather_lat, weather_lng, location_name, news_feeds, news_keywords)
              VALUES ('u1','b1',1,'0 7 * * *','channel','chan-1',35.6,139.7,'東京','["https://e.com/rss"]','["AI"]'),
                     ('u2','b1',0,'0 6 * * *','dm',NULL,NULL,NULL,'大阪','[]','[]'),
                     ('u3','b1',1,'0 7 * * *','dm',NULL,NULL,NULL,NULL,'[]','[]'),
                     ('u1','b2',1,'0 7 * * *','dm',NULL,NULL,NULL,'  ','oops','[]');
            -- 日報・週報: 未選択（全部・チャンネル指定は DM へ）/ 会話トピックだけ（チャンネルのまま）。
            INSERT INTO report_configs (user_id, bot_id, type, enabled, schedule_cron, target_type, target_id, sections)
              VALUES ('u1','b1','daily',1,'0 22 * * *','channel','chan-1',NULL),
                     ('u2','b1','weekly',0,'0 21 * * 0','channel','chan-2','["topics"]'),
                     ('u3','b1','daily',1,'0 21 * * *','channel','',NULL);"#,
        )
        .unwrap();
        run_migrations(&mut conn).unwrap();

        type Row = (
            String,
            String,
            String,
            i64,
            String,
            String,
            Option<String>,
            String,
        );
        let rows: Vec<Row> = conn
            .prepare(
                "SELECT user_id, bot_id, name, enabled, schedule_cron, target_type, target_id, sections \
                 FROM periodic_reports ORDER BY user_id, bot_id, name",
            )
            .unwrap()
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let all = r#"["tasks","schedules","payments","finance","topics"]"#;
        let s = |v: &str| v.to_owned();
        assert_eq!(
            rows,
            vec![
                (
                    s("u1"),
                    s("b1"),
                    s("日報"),
                    1,
                    s("0 22 * * *"),
                    s("dm"),
                    None,
                    s(all)
                ),
                (
                    s("u1"),
                    s("b1"),
                    s("朝報"),
                    1,
                    s("0 7 * * *"),
                    s("channel"),
                    Some(s("chan-1")),
                    s(r#"["weather","news"]"#)
                ),
                (
                    s("u2"),
                    s("b1"),
                    s("朝報"),
                    0,
                    s("0 6 * * *"),
                    s("dm"),
                    None,
                    s(r#"["weather"]"#)
                ),
                (
                    s("u2"),
                    s("b1"),
                    s("週報"),
                    0,
                    s("0 21 * * 0"),
                    s("channel"),
                    Some(s("chan-2")),
                    s(r#"["topics"]"#)
                ),
                (
                    s("u3"),
                    s("b1"),
                    s("日報"),
                    1,
                    s("0 21 * * *"),
                    s("dm"),
                    None,
                    s(all)
                ),
            ]
        );
        let (location, lat, feeds, keywords): (String, f64, String, String) = conn
            .query_row(
                "SELECT location_name, weather_lat, news_feeds, news_keywords FROM periodic_reports \
                 WHERE user_id = 'u1' AND name = '朝報'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(location, "東京");
        assert!((lat - 35.6).abs() < f64::EPSILON);
        assert_eq!(feeds, r#"["https://e.com/rss"]"#);
        assert_eq!(keywords, r#"["AI"]"#);
        // 旧テーブルは残す。
        let old: i64 = conn
            .query_row("SELECT COUNT(*) FROM briefing_configs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(old, 4);
    }
}
