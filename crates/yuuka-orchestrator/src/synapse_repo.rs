//! シナプス（記憶の断片）リポジトリ — Node `src/db/synapseRepo.ts` の書き込み経路パリティ。
//!
//! `synapses` テーブル（スキーマ定義元は `yuuka-db` migrations V17）への書き込みを、workspace の
//! **単一 writer actor**（[`yuuka_web::Db::writer`]）経由で直列化する（並行 writer を作らない・R-2）。
//! 想起時の鮮度更新（`touch`）もここに置く。読み取り（想起 KNN）は RAM 索引（`yuuka-synapse`）が担うため
//! この repo には SELECT を置かない（Node の `getSynapsesByIds` 等は Rust 経路では RAM 索引で完結する）。

use yuuka_core::DbError;
use yuuka_db::map_sqlite;
use yuuka_web::Db;

/// シナプス挿入の引数（Node `insertSynapse` パリティ）。`ctx_tod`/`ctx_dow` は再ランキング専用文脈。
pub struct InsertArgs {
    pub user_id: String,
    pub bot_id: String,
    pub guild_id: Option<String>,
    pub content: String,
    pub topic_id: Option<String>,
    pub source_msg_id: Option<i64>,
    pub ctx_tod: Option<i64>,
    pub ctx_dow: Option<i64>,
}

/// シナプスを 1 件挿入し、採番された `id` を返す（Node `insertSynapse`）。`embedding` は後追いで
/// [`update_synapse_embedding`] が埋める（挿入直後は NULL＝未埋め込み）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn insert_synapse(db: &Db, args: InsertArgs) -> Result<i64, DbError> {
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT INTO synapses \
                 (user_id, bot_id, guild_id, content, topic_id, source_msg_id, ctx_tod, ctx_dow) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    args.user_id,
                    args.bot_id,
                    args.guild_id,
                    args.content,
                    args.topic_id,
                    args.source_msg_id,
                    args.ctx_tod,
                    args.ctx_dow,
                ],
            )
            .map_err(map_sqlite)?;
            Ok(tx.last_insert_rowid())
        })
        .await
}

/// 埋め込みベクトル（生 BLOB バイト列・f32 LE）とモデル世代を保存する（Node `updateSynapseEmbedding`）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn update_synapse_embedding(
    db: &Db,
    id: i64,
    embedding: Vec<u8>,
    model_version: &str,
) -> Result<(), DbError> {
    let model_version = model_version.to_owned();
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "UPDATE synapses SET embedding = ?1, embedding_model_version = ?2 WHERE id = ?3",
                rusqlite::params![embedding, model_version, id],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// 想起されたシナプスの鮮度を更新する（`use_count++` / `last_used_at=now` / `decay_score += 1.0`）。
/// Node `touchSynapses` パリティ。id 群はスコープ済み想起（RAM 索引の KNN 結果）から得られたものを前提に
/// 主キー id でのみ更新する。空配列は no-op。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn touch_synapses(db: &Db, ids: Vec<i64>) -> Result<(), DbError> {
    if ids.is_empty() {
        return Ok(());
    }
    db.writer
        .transaction(move |tx| {
            // プレースホルダを id 数だけ生成（IN 句・Node と同形）。
            let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
            let sql = format!(
                "UPDATE synapses SET \
                   use_count = use_count + 1, \
                   last_used_at = datetime('now', 'localtime'), \
                   decay_score = decay_score + 1.0 \
                 WHERE id IN ({placeholders})",
            );
            let params: Vec<&dyn rusqlite::ToSql> =
                ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
            tx.execute(&sql, params.as_slice()).map_err(map_sqlite)?;
            Ok(())
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
        // synapses テーブルを作らせる（message_log.rs の seed_db と同型）。
        drop(rusqlite::Connection::open(&path).expect("create empty"));
        let db = Db::open(Path::new(&path)).expect("open db");
        (db, dir)
    }

    /// insert→embedding 更新→touch の書き込み経路が単一 writer 上で成功する（Node パリティ）。
    #[tokio::test]
    async fn insert_update_touch_roundtrip() {
        let (db, _dir) = open_db().await;
        let id = insert_synapse(
            &db,
            InsertArgs {
                user_id: "u1".into(),
                bot_id: "b1".into(),
                guild_id: None,
                content: "好きな食べ物はカレーです".into(),
                topic_id: Some("カレー".into()),
                source_msg_id: None,
                ctx_tod: Some(12),
                ctx_dow: Some(3),
            },
        )
        .await
        .expect("insert");
        assert!(id > 0, "採番された id を返す");

        update_synapse_embedding(&db, id, vec![0u8, 1, 2, 3], "hash-ngram-v1")
            .await
            .expect("update embedding");

        // 読み戻して embedding とモデル世代が入っていること（read pool 経由）。
        let (blob_len, ver): (i64, String) = db
            .read
            .read(move |conn| {
                conn.query_row(
                    "SELECT length(embedding), embedding_model_version FROM synapses WHERE id = ?1",
                    rusqlite::params![id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(map_sqlite)
            })
            .await
            .expect("read back");
        assert_eq!(blob_len, 4);
        assert_eq!(ver, "hash-ngram-v1");

        touch_synapses(&db, vec![id]).await.expect("touch");
        let use_count: i64 = db
            .read
            .read(move |conn| {
                conn.query_row(
                    "SELECT use_count FROM synapses WHERE id = ?1",
                    rusqlite::params![id],
                    |r| r.get(0),
                )
                .map_err(map_sqlite)
            })
            .await
            .expect("read use_count");
        assert_eq!(use_count, 1, "touch で use_count が加算される");
    }

    #[tokio::test]
    async fn touch_empty_is_noop() {
        let (db, _dir) = open_db().await;
        touch_synapses(&db, vec![]).await.expect("no-op ok");
    }
}
