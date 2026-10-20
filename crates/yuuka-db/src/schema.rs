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

#[cfg(test)]
mod tests {
    use super::{embedded, run_migrations};
    use rusqlite::Connection;

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
    fn v27_preserves_legacy_tags_without_retrospective_tagging() {
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
}
