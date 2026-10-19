//! ツール実行実績リポジトリ — Node `src/db/toolOutcomeRepo.ts` の書き込み経路（`recordToolOutcome`）パリティ。
//!
//! `tool_outcomes` テーブル（スキーマ定義元は `yuuka-db` migrations V17）への INSERT を、workspace の
//! **単一 writer actor**（[`yuuka_web::Db::writer`]）経由で直列化する。Node は `topic_id` があるときだけ
//! `topic_tool_stats` を同一トランザクションで加算していたが、Node の呼び出し元は常に `topic_id=null`
//! （2nd Hop のトピック紐付けは未着手）で到達しない経路のため移植しない。読み出し
//! （`getTopicToolStats` / `getToolOutcomeStats`）も Node に呼び出し元が無く未移植。
//!
//! `args_digest` は呼び出し側で秘匿マスク済みのものを渡す（[`crate::tool_outcome`] 参照）。

use yuuka_core::DbError;
use yuuka_db::map_sqlite;
use yuuka_web::Db;

/// 実行実績 1 件の挿入引数（Node `ToolOutcomeInput`）。データ分離キーは `user_id` + `bot_id`。
pub struct InsertArgs {
    pub user_id: String,
    pub bot_id: String,
    /// `None` = DM/秘書、`Some` = 汎用モードのギルド。
    pub guild_id: Option<String>,
    pub topic_id: Option<String>,
    pub synapse_id: Option<i64>,
    pub tool_name: String,
    /// 引数ダイジェスト（秘匿マスク済み）。生の引数は渡さないこと。
    pub args_digest: Option<String>,
    /// `"success"` / `"error"`。
    pub status: String,
    pub latency_ms: Option<i64>,
}

/// ツール実行実績を 1 件挿入し、採番された `id` を返す（Node `recordToolOutcome` の INSERT 部）。
/// 空文字の `topic_id` は NULL に正規化する（Node と同じ）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn insert_tool_outcome(db: &Db, args: InsertArgs) -> Result<i64, DbError> {
    let topic_id = args.topic_id.filter(|t| !t.is_empty());
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT INTO tool_outcomes \
                 (user_id, bot_id, guild_id, topic_id, synapse_id, tool_name, args_digest, status, latency_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    args.user_id,
                    args.bot_id,
                    args.guild_id,
                    topic_id,
                    args.synapse_id,
                    args.tool_name,
                    args.args_digest,
                    args.status,
                    args.latency_ms,
                ],
            )
            .map_err(map_sqlite)?;
            Ok(tx.last_insert_rowid())
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    async fn open_db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("t.db");
        // Rust は存在しない DB を作らない（P0-4）。空ファイルを先に作り、`Db::open` の migrations で
        // tool_outcomes テーブルを作らせる（synapse_repo.rs と同型）。
        drop(rusqlite::Connection::open(&path).expect("create empty"));
        let db = Db::open(Path::new(&path)).expect("open db");
        (db, dir)
    }

    type Row = (
        String,
        String,
        Option<String>,
        Option<String>,
        Option<i64>,
        String,
        Option<String>,
        String,
        Option<i64>,
    );

    async fn read_row(db: &Db, id: i64) -> Row {
        db.read
            .read(move |conn| {
                conn.query_row(
                    "SELECT user_id, bot_id, guild_id, topic_id, synapse_id, tool_name, args_digest, \
                     status, latency_ms FROM tool_outcomes WHERE id = ?1",
                    rusqlite::params![id],
                    |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                            r.get(5)?,
                            r.get(6)?,
                            r.get(7)?,
                            r.get(8)?,
                        ))
                    },
                )
                .map_err(map_sqlite)
            })
            .await
            .expect("read back")
    }

    /// 全列が指定どおりに保存され、`created_at` は既定値で埋まる。
    #[tokio::test]
    async fn insert_persists_all_columns() {
        let (db, _dir) = open_db().await;
        let id = insert_tool_outcome(
            &db,
            InsertArgs {
                user_id: "u1".into(),
                bot_id: "b1".into(),
                guild_id: Some("g1".into()),
                topic_id: None,
                synapse_id: None,
                tool_name: "addTodo".into(),
                args_digest: Some(r#"{"title":"牛乳"}"#.into()),
                status: "error".into(),
                latency_ms: Some(42),
            },
        )
        .await
        .expect("insert");
        assert!(id > 0, "採番された id を返す");

        let row = read_row(&db, id).await;
        assert_eq!(
            row,
            (
                "u1".to_owned(),
                "b1".to_owned(),
                Some("g1".to_owned()),
                None,
                None,
                "addTodo".to_owned(),
                Some(r#"{"title":"牛乳"}"#.to_owned()),
                "error".to_owned(),
                Some(42),
            )
        );

        let created_at: String = db
            .read
            .read(move |conn| {
                conn.query_row(
                    "SELECT created_at FROM tool_outcomes WHERE id = ?1",
                    rusqlite::params![id],
                    |r| r.get(0),
                )
                .map_err(map_sqlite)
            })
            .await
            .expect("created_at");
        assert!(!created_at.is_empty());
    }

    /// 空文字の topic_id は NULL へ正規化される（Node と同じ）。
    #[tokio::test]
    async fn empty_topic_id_is_stored_as_null() {
        let (db, _dir) = open_db().await;
        let id = insert_tool_outcome(
            &db,
            InsertArgs {
                user_id: "u1".into(),
                bot_id: "b1".into(),
                guild_id: None,
                topic_id: Some(String::new()),
                synapse_id: None,
                tool_name: "addTodo".into(),
                args_digest: None,
                status: "success".into(),
                latency_ms: None,
            },
        )
        .await
        .expect("insert");
        assert_eq!(read_row(&db, id).await.3, None);
    }
}
