//! 他クレートの単体テスト用ヘルパ（feature `test-support` のときだけ公開）。

use rusqlite::Connection;

/// 単体テスト用の DDL を実行し、その DDL が作り直した `todos` に実スキーマのトリガーを張り直す。
///
/// fixture は実マイグレーションの後に、FK 無しの単体用 DDL（`DROP TABLE IF EXISTS todos;
/// CREATE TABLE todos ...`）でテーブルを置き換える（SD07）。SQLite の `DROP TABLE` はそのテーブルの
/// トリガーも削除するため、そのままでは V28 の自動タグ予約（`todo_tagging_*`）が動かず、本番と挙動が
/// ずれる。DDL を実行する前に実スキーマのトリガー定義を控えておき、消えたものだけを作り直す。
///
/// # Errors
/// SQL の実行に失敗した場合。
pub fn execute_unit_ddl(conn: &Connection, ddl: &str) -> rusqlite::Result<()> {
    let triggers = {
        let mut stmt = conn.prepare(
            "SELECT name, sql FROM sqlite_master \
             WHERE type = 'trigger' AND tbl_name = 'todos' AND sql IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    conn.execute_batch(ddl)?;
    for (name, sql) in triggers {
        let exists: bool = conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'trigger' AND name = ?1)",
            [&name],
            |r| r.get(0),
        )?;
        if !exists {
            conn.execute_batch(&sql)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaced_todos_keeps_auto_tag_triggers() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::run_migrations(&mut conn).unwrap();
        execute_unit_ddl(
            &conn,
            "DROP TABLE IF EXISTS todos; CREATE TABLE todos (id INTEGER PRIMARY KEY AUTOINCREMENT,
             user_id TEXT NOT NULL, bot_id TEXT NOT NULL, title TEXT NOT NULL, description TEXT,
             tags TEXT NOT NULL DEFAULT '[]');",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO todos (user_id, bot_id, title, tags) VALUES ('u', 'b', 't', '[\"x\"]')",
            [],
        )
        .unwrap();
        let (explicit, pending): (String, i64) = conn
            .query_row(
                "SELECT explicit_tags, pending FROM todo_tagging WHERE todo_id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(explicit, r#"["x"]"#);
        assert_eq!(pending, 1);
        // 置き換えない DDL ではトリガーが残っているので、作り直さない（重複作成で失敗しない）。
        execute_unit_ddl(&conn, "CREATE TABLE unrelated (id INTEGER PRIMARY KEY);").unwrap();
    }
}
