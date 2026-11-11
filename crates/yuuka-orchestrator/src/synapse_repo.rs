//! シナプス（記憶の断片）リポジトリ — Node `src/db/synapseRepo.ts` の書き込み経路パリティ＋
//! 評価・選別（docs/design/memory_retention.md）のための読み書き。
//!
//! `synapses` テーブル（スキーマ定義元は `yuuka-db` migrations V17・V37）への書き込みを、workspace の
//! **単一 writer actor**（[`yuuka_web::Db::writer`]）経由で直列化する（並行 writer を作らない・R-2）。
//! 想起時の鮮度更新（`touch`）と有用の記録（`mark_useful`）、候補（`status='candidate'`）の一覧と
//! 確定、減衰と削除の対象選定もここに置く。想起 KNN の読み取りは RAM 索引（`yuuka-synapse`）が担う。

use rusqlite::params;
use yuuka_core::DbError;
use yuuka_db::map_sqlite;
use yuuka_web::Db;

/// 候補として入れるときの `status`。
pub const STATUS_CANDIDATE: &str = "candidate";
/// 評価を通って想起の対象になった `status`。
pub const STATUS_KEPT: &str = "kept";

/// シナプス挿入の引数（Node `insertSynapse` パリティ＋選別用の列）。`ctx_tod`/`ctx_dow` は再ランキング専用文脈。
pub struct InsertArgs {
    pub user_id: String,
    pub bot_id: String,
    pub guild_id: Option<String>,
    pub content: String,
    pub topic_id: Option<String>,
    pub source_msg_id: Option<i64>,
    pub ctx_tod: Option<i64>,
    pub ctx_dow: Option<i64>,
    /// `candidate`（評価待ち・想起に出さない）か `kept`（想起の対象）。
    pub status: &'static str,
    /// 正規化した内容のハッシュ（同じ内容を二度入れないため）。
    pub content_hash: Option<String>,
}

/// スコープ（利用者 × Bot × ギルド）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScopeKey {
    pub user_id: String,
    pub bot_id: String,
    pub guild_id: Option<String>,
}

/// 評価待ちの候補 1 件。
#[derive(Debug, Clone)]
pub struct Candidate {
    pub id: i64,
    pub content: String,
    pub topic_id: Option<String>,
    /// 候補になってからの経過秒。
    pub age_secs: i64,
}

/// 候補を持つスコープと、その件数・最古の経過秒。
#[derive(Debug, Clone)]
pub struct CandidateScope {
    pub scope: ScopeKey,
    pub count: i64,
    pub oldest_age_secs: i64,
}

/// 評価器が候補に与えた結果。
#[derive(Debug, Clone)]
pub struct Verdict {
    /// 書き直した短い事実文。
    pub statement: String,
    pub kind: String,
    /// `long` / `medium` / `short`。
    pub durability: String,
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
                 (user_id, bot_id, guild_id, content, topic_id, source_msg_id, ctx_tod, ctx_dow, \
                  status, content_hash) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    args.user_id,
                    args.bot_id,
                    args.guild_id,
                    args.content,
                    args.topic_id,
                    args.source_msg_id,
                    args.ctx_tod,
                    args.ctx_dow,
                    args.status,
                    args.content_hash,
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
                params![embedding, model_version, id],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// 想起されたシナプスの鮮度を更新する（`use_count++` / `last_used_at=now` / `strength += 0.1`）。
/// 想起は「使われた」証拠ではないので強度の加算は小さい（有用は [`mark_useful`]）。
/// id 群はスコープ済み想起（RAM 索引の KNN 結果）から得られたものを前提に主キー id でのみ更新する。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn touch_synapses(db: &Db, ids: Vec<i64>) -> Result<(), DbError> {
    update_by_ids(
        db,
        ids,
        "UPDATE synapses SET \
           use_count = use_count + 1, \
           last_used_at = datetime('now', 'localtime'), \
           decay_score = decay_score + 1.0, \
           strength = strength + 0.1 \
         WHERE id IN ({ids})",
    )
    .await
}

/// 想起した記憶が返答に使われたと判定したものを強める（`useful_count++` / `last_useful_at=now` /
/// `strength += 1.0`）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn mark_useful(db: &Db, ids: Vec<i64>) -> Result<(), DbError> {
    update_by_ids(
        db,
        ids,
        "UPDATE synapses SET \
           useful_count = useful_count + 1, \
           last_useful_at = datetime('now', 'localtime'), \
           strength = strength + 1.0 \
         WHERE id IN ({ids})",
    )
    .await
}

/// 候補を kept にし、内容を評価器の事実文に置き換える。埋め込みは呼び出し側が索引化してから
/// [`update_synapse_embedding`] で入れる。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn keep_candidate(db: &Db, id: i64, verdict: Verdict) -> Result<(), DbError> {
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "UPDATE synapses SET status = 'kept', content = ?1, kind = ?2, durability = ?3, \
                   strength = 1.0, evaluated_at = datetime('now', 'localtime') \
                 WHERE id = ?4 AND status = 'candidate'",
                params![verdict.statement, verdict.kind, verdict.durability, id],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// 行を消す（候補の不採用・忘却）。RAM 索引からの除去は呼び出し側が行う。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn delete_synapses(db: &Db, ids: Vec<i64>) -> Result<(), DbError> {
    update_by_ids(db, ids, "DELETE FROM synapses WHERE id IN ({ids})").await
}

/// 同じ内容（ハッシュ）が同スコープに既にあるか（候補・kept を問わない）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn has_content_hash(db: &Db, scope: &ScopeKey, hash: &str) -> Result<bool, DbError> {
    let (uid, bid, gid, hash) = (
        scope.user_id.clone(),
        scope.bot_id.clone(),
        scope.guild_id.clone(),
        hash.to_owned(),
    );
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT count(*) FROM synapses \
                 WHERE user_id = ?1 AND bot_id = ?2 AND guild_id IS ?3 AND content_hash = ?4",
                params![uid, bid, gid, hash],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .map_err(map_sqlite)
        })
        .await
}

/// 今日（ローカル暦日）このスコープに入れた件数（候補・kept を問わない）。1 日の上限に使う。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn count_created_today(db: &Db, scope: &ScopeKey) -> Result<i64, DbError> {
    let (uid, bid, gid) = (
        scope.user_id.clone(),
        scope.bot_id.clone(),
        scope.guild_id.clone(),
    );
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT count(*) FROM synapses \
                 WHERE user_id = ?1 AND bot_id = ?2 AND guild_id IS ?3 \
                   AND date(created_at) = date('now', 'localtime')",
                params![uid, bid, gid],
                |r| r.get::<_, i64>(0),
            )
            .map_err(map_sqlite)
        })
        .await
}

/// 候補を持つスコープの一覧（件数と最古の経過秒付き）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_candidate_scopes(db: &Db) -> Result<Vec<CandidateScope>, DbError> {
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT user_id, bot_id, guild_id, count(*), \
                       max(CAST(strftime('%s','now') AS INTEGER) \
                           - CAST(strftime('%s', created_at, 'utc') AS INTEGER)) \
                     FROM synapses WHERE status = 'candidate' \
                     GROUP BY user_id, bot_id, guild_id",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(CandidateScope {
                        scope: ScopeKey {
                            user_id: r.get(0)?,
                            bot_id: r.get(1)?,
                            guild_id: r.get(2)?,
                        },
                        count: r.get(3)?,
                        oldest_age_secs: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
                    })
                })
                .map_err(map_sqlite)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(map_sqlite)
        })
        .await
}

/// スコープの候補を古い順に最大 `limit` 件。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_candidates(
    db: &Db,
    scope: &ScopeKey,
    limit: usize,
) -> Result<Vec<Candidate>, DbError> {
    let (uid, bid, gid) = (
        scope.user_id.clone(),
        scope.bot_id.clone(),
        scope.guild_id.clone(),
    );
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, content, topic_id, \
                       CAST(strftime('%s','now') AS INTEGER) \
                         - CAST(strftime('%s', created_at, 'utc') AS INTEGER) \
                     FROM synapses \
                     WHERE user_id = ?1 AND bot_id = ?2 AND guild_id IS ?3 AND status = 'candidate' \
                     ORDER BY id ASC LIMIT ?4",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![uid, bid, gid, limit], |r| {
                    Ok(Candidate {
                        id: r.get(0)?,
                        content: r.get(1)?,
                        topic_id: r.get(2)?,
                        age_secs: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    })
                })
                .map_err(map_sqlite)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(map_sqlite)
        })
        .await
}

/// kept の強度を 1 日分減衰させる。`long` は半分の速さで減る。更新件数を返す。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn decay_strength(db: &Db, daily_rate: f64, long_rate: f64) -> Result<usize, DbError> {
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "UPDATE synapses SET strength = strength * \
                   (CASE durability WHEN 'long' THEN ?1 ELSE ?2 END) \
                 WHERE status = 'kept'",
                params![long_rate, daily_rate],
            )
            .map_err(map_sqlite)
        })
        .await
}

/// 忘れてよい kept の id: 強度が `threshold` 未満で、作成から `min_age_days` 日を過ぎ、最後に有用
/// だった日から `durability` の期間（long 365 / medium 90 / short 14 日）も過ぎているもの。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn select_forgettable(
    db: &Db,
    threshold: f64,
    min_age_days: i64,
) -> Result<Vec<i64>, DbError> {
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id FROM synapses \
                     WHERE status = 'kept' AND strength < ?1 \
                       AND created_at < datetime('now', 'localtime', '-' || ?2 || ' days') \
                       AND (last_useful_at IS NULL OR last_useful_at < datetime('now', 'localtime', \
                            '-' || (CASE durability WHEN 'long' THEN 365 WHEN 'short' THEN 14 ELSE 90 END) \
                            || ' days'))",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![threshold, min_age_days], |r| r.get::<_, i64>(0))
                .map_err(map_sqlite)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(map_sqlite)
        })
        .await
}

/// kept の件数をスコープごとに返す。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn kept_scope_counts(db: &Db) -> Result<Vec<(ScopeKey, i64)>, DbError> {
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT user_id, bot_id, guild_id, count(*) FROM synapses \
                     WHERE status = 'kept' GROUP BY user_id, bot_id, guild_id",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        ScopeKey {
                            user_id: r.get(0)?,
                            bot_id: r.get(1)?,
                            guild_id: r.get(2)?,
                        },
                        r.get::<_, i64>(3)?,
                    ))
                })
                .map_err(map_sqlite)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(map_sqlite)
        })
        .await
}

/// スコープの kept のうち、強度の低い順に `cap` 件を超える分の id。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn select_over_cap(db: &Db, scope: &ScopeKey, cap: usize) -> Result<Vec<i64>, DbError> {
    let (uid, bid, gid) = (
        scope.user_id.clone(),
        scope.bot_id.clone(),
        scope.guild_id.clone(),
    );
    let cap = i64::try_from(cap).unwrap_or(i64::MAX);
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id FROM synapses \
                     WHERE user_id = ?1 AND bot_id = ?2 AND guild_id IS ?3 AND status = 'kept' \
                     ORDER BY strength DESC, id DESC LIMIT -1 OFFSET ?4",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![uid, bid, gid, cap], |r| r.get::<_, i64>(0))
                .map_err(map_sqlite)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(map_sqlite)
        })
        .await
}

/// 利用者に見せる kept の 1 件（docs/design/memory_retention.md §4-E）。内部スコープ列は持たない。
#[derive(Debug, Clone, serde::Serialize)]
pub struct MemoryRow {
    pub id: i64,
    /// 評価器が書いた事実文（評価前からある行は発話の生テキスト）。
    pub content: String,
    pub kind: Option<String>,
    pub durability: String,
    pub strength: f64,
    pub use_count: i64,
    pub useful_count: i64,
    pub last_used_at: Option<String>,
    pub created_at: String,
}

const MEMORY_ROW_COLUMNS: &str =
    "id, content, kind, durability, strength, use_count, useful_count, \
     last_used_at, created_at";

fn map_memory_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryRow> {
    Ok(MemoryRow {
        id: r.get(0)?,
        content: r.get(1)?,
        kind: r.get(2)?,
        durability: r.get(3)?,
        strength: r.get(4)?,
        use_count: r.get(5)?,
        useful_count: r.get(6)?,
        last_used_at: r.get(7)?,
        created_at: r.get(8)?,
    })
}

/// 利用者 × Bot の kept を強度の高い順に返す（ギルドの別は問わない＝本人の分は全部）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_kept(db: &Db, user_id: &str, bot_id: &str) -> Result<Vec<MemoryRow>, DbError> {
    let (uid, bid) = (user_id.to_owned(), bot_id.to_owned());
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {MEMORY_ROW_COLUMNS} FROM synapses \
                     WHERE user_id = ?1 AND bot_id = ?2 AND status = 'kept' \
                     ORDER BY strength DESC, id DESC"
                ))
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![uid, bid], map_memory_row)
                .map_err(map_sqlite)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(map_sqlite)
        })
        .await
}

/// 利用者 × Bot の kept のうち、内容に `needle` を含むもの（最大 `limit` 件・強い順）。
/// `%` と `_` はリテラルとして扱う。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn find_kept_by_content(
    db: &Db,
    user_id: &str,
    bot_id: &str,
    needle: &str,
    limit: usize,
) -> Result<Vec<MemoryRow>, DbError> {
    let (uid, bid) = (user_id.to_owned(), bot_id.to_owned());
    let escaped = needle
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let pattern = format!("%{escaped}%");
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {MEMORY_ROW_COLUMNS} FROM synapses \
                     WHERE user_id = ?1 AND bot_id = ?2 AND status = 'kept' \
                       AND content LIKE ?3 ESCAPE '\\' \
                     ORDER BY strength DESC, id DESC LIMIT ?4"
                ))
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![uid, bid, pattern, limit], map_memory_row)
                .map_err(map_sqlite)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(map_sqlite)
        })
        .await
}

/// 利用者 × Bot の評価待ち（`status='candidate'`）の件数。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn count_candidates(db: &Db, user_id: &str, bot_id: &str) -> Result<i64, DbError> {
    let (uid, bid) = (user_id.to_owned(), bot_id.to_owned());
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT count(*) FROM synapses \
                 WHERE user_id = ?1 AND bot_id = ?2 AND status = 'candidate'",
                params![uid, bid],
                |r| r.get::<_, i64>(0),
            )
            .map_err(map_sqlite)
        })
        .await
}

/// 本人 × Bot の kept を 1 件消す。他人の行・候補の行は消さず `false`。RAM 索引からの除去は
/// 呼び出し側が行う（`true` のときだけ）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn delete_kept_owned(
    db: &Db,
    user_id: &str,
    bot_id: &str,
    id: i64,
) -> Result<bool, DbError> {
    let (uid, bid) = (user_id.to_owned(), bot_id.to_owned());
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute(
                    "DELETE FROM synapses \
                     WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3 AND status = 'kept'",
                    params![id, uid, bid],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

/// `{ids}` を id の数だけのプレースホルダに展開して実行する。空配列は no-op。
async fn update_by_ids(db: &Db, ids: Vec<i64>, sql_template: &str) -> Result<(), DbError> {
    if ids.is_empty() {
        return Ok(());
    }
    let template = sql_template.to_owned();
    db.writer
        .transaction(move |tx| {
            let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
            let sql = template.replace("{ids}", &placeholders);
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

    fn scope() -> ScopeKey {
        ScopeKey {
            user_id: "u1".into(),
            bot_id: "b1".into(),
            guild_id: None,
        }
    }

    fn args(content: &str, status: &'static str, hash: Option<&str>) -> InsertArgs {
        InsertArgs {
            user_id: "u1".into(),
            bot_id: "b1".into(),
            guild_id: None,
            content: content.into(),
            topic_id: Some("カレー".into()),
            source_msg_id: None,
            ctx_tod: Some(12),
            ctx_dow: Some(3),
            status,
            content_hash: hash.map(str::to_owned),
        }
    }

    async fn read_i64(db: &Db, sql: &'static str, id: i64) -> i64 {
        db.read
            .read(move |conn| {
                conn.query_row(sql, params![id], |r| r.get(0))
                    .map_err(map_sqlite)
            })
            .await
            .expect("read")
    }

    async fn read_f64(db: &Db, sql: &'static str, id: i64) -> f64 {
        db.read
            .read(move |conn| {
                conn.query_row(sql, params![id], |r| r.get(0))
                    .map_err(map_sqlite)
            })
            .await
            .expect("read")
    }

    /// insert→embedding 更新→touch の書き込み経路が単一 writer 上で成功する（Node パリティ）。
    #[tokio::test]
    async fn insert_update_touch_roundtrip() {
        let (db, _dir) = open_db().await;
        let id = insert_synapse(&db, args("好きな食べ物はカレーです", STATUS_KEPT, None))
            .await
            .expect("insert");
        assert!(id > 0, "採番された id を返す");

        update_synapse_embedding(&db, id, vec![0u8, 1, 2, 3], "hash-ngram-v1")
            .await
            .expect("update embedding");

        let (blob_len, ver): (i64, String) = db
            .read
            .read(move |conn| {
                conn.query_row(
                    "SELECT length(embedding), embedding_model_version FROM synapses WHERE id = ?1",
                    params![id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(map_sqlite)
            })
            .await
            .expect("read back");
        assert_eq!(blob_len, 4);
        assert_eq!(ver, "hash-ngram-v1");

        touch_synapses(&db, vec![id]).await.expect("touch");
        assert_eq!(
            read_i64(&db, "SELECT use_count FROM synapses WHERE id = ?1", id).await,
            1
        );
        let strength = read_f64(&db, "SELECT strength FROM synapses WHERE id = ?1", id).await;
        assert!((strength - 1.1).abs() < 1e-9, "想起は +0.1: {strength}");

        mark_useful(&db, vec![id]).await.expect("useful");
        assert_eq!(
            read_i64(&db, "SELECT useful_count FROM synapses WHERE id = ?1", id).await,
            1
        );
        let strength = read_f64(&db, "SELECT strength FROM synapses WHERE id = ?1", id).await;
        assert!((strength - 2.1).abs() < 1e-9, "有用は +1.0: {strength}");
    }

    #[tokio::test]
    async fn touch_empty_is_noop() {
        let (db, _dir) = open_db().await;
        touch_synapses(&db, vec![]).await.expect("no-op ok");
        delete_synapses(&db, vec![]).await.expect("no-op ok");
    }

    /// 候補は一覧に出て、採用で kept に変わり、不採用で消える。重複ハッシュと当日件数も数えられる。
    #[tokio::test]
    async fn candidates_are_listed_kept_or_deleted() {
        let (db, _dir) = open_db().await;
        let a = insert_synapse(
            &db,
            args("私はカレーが好きだ", STATUS_CANDIDATE, Some("h1")),
        )
        .await
        .expect("a");
        let b = insert_synapse(&db, args("今日は疲れた", STATUS_CANDIDATE, Some("h2")))
            .await
            .expect("b");
        assert!(has_content_hash(&db, &scope(), "h1").await.expect("hash"));
        assert!(!has_content_hash(&db, &scope(), "h9").await.expect("hash"));
        assert_eq!(count_created_today(&db, &scope()).await.expect("today"), 2);

        let scopes = list_candidate_scopes(&db).await.expect("scopes");
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes.first().map(|s| s.count), Some(2));
        let list = list_candidates(&db, &scope(), 10).await.expect("list");
        assert_eq!(list.iter().map(|c| c.id).collect::<Vec<_>>(), vec![a, b]);

        keep_candidate(
            &db,
            a,
            Verdict {
                statement: "利用者はカレーが好き".into(),
                kind: "preference".into(),
                durability: "long".into(),
            },
        )
        .await
        .expect("keep");
        delete_synapses(&db, vec![b]).await.expect("delete");

        assert!(list_candidates(&db, &scope(), 10)
            .await
            .expect("list")
            .is_empty());
        let counts = kept_scope_counts(&db).await.expect("counts");
        assert_eq!(counts.first().map(|(_, n)| *n), Some(1));
        let content: String = db
            .read
            .read(move |conn| {
                conn.query_row(
                    "SELECT content FROM synapses WHERE id = ?1 AND status = 'kept' AND kind = 'preference'",
                    params![a],
                    |r| r.get(0),
                )
                .map_err(map_sqlite)
            })
            .await
            .expect("kept row");
        assert_eq!(content, "利用者はカレーが好き");
    }

    /// 減衰で強度が下がり、閾値を割った古い記憶だけが忘却の対象になる。上限超過は弱い順に選ばれる。
    #[tokio::test]
    async fn decay_forget_and_cap_select_expected_rows() {
        let (db, _dir) = open_db().await;
        let old = insert_synapse(&db, args("古い記憶", STATUS_KEPT, None))
            .await
            .expect("old");
        let fresh = insert_synapse(&db, args("新しい記憶", STATUS_KEPT, None))
            .await
            .expect("fresh");
        let strong = insert_synapse(&db, args("強い記憶", STATUS_KEPT, None))
            .await
            .expect("strong");
        // old は 30 日前に作られ強度 0.1、strong は 5.0 にしておく。
        db.writer
            .transaction(move |tx| {
                tx.execute(
                    "UPDATE synapses SET created_at = datetime('now','localtime','-30 days'), strength = 0.1 WHERE id = ?1",
                    params![old],
                )
                .map_err(map_sqlite)?;
                tx.execute(
                    "UPDATE synapses SET strength = 5.0 WHERE id = ?1",
                    params![strong],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
            .expect("seed");

        let n = decay_strength(&db, 0.5, 0.75).await.expect("decay");
        assert_eq!(n, 3);
        let s = read_f64(&db, "SELECT strength FROM synapses WHERE id = ?1", fresh).await;
        assert!((s - 0.5).abs() < 1e-9, "medium は daily_rate: {s}");

        let forget = select_forgettable(&db, 0.2, 14).await.expect("forget");
        assert_eq!(forget, vec![old], "fresh は作成 14 日未満なので対象外");

        let over = select_over_cap(&db, &scope(), 1).await.expect("cap");
        assert_eq!(over.len(), 2);
        assert!(!over.contains(&strong), "一番強いものは残る");
    }

    /// 利用者向けの一覧は本人 × Bot の kept だけを強い順に返し、削除は本人の kept にしか効かない。
    #[tokio::test]
    async fn user_facing_list_find_count_and_scoped_delete() {
        let (db, _dir) = open_db().await;
        let weak = insert_synapse(&db, args("利用者は紅茶が好き", STATUS_KEPT, None))
            .await
            .expect("weak");
        let strong = insert_synapse(&db, args("利用者は金曜は在宅", STATUS_KEPT, None))
            .await
            .expect("strong");
        let pending = insert_synapse(&db, args("今日は疲れた", STATUS_CANDIDATE, None))
            .await
            .expect("pending");
        let other = insert_synapse(
            &db,
            InsertArgs {
                user_id: "u2".into(),
                ..args("他人の記憶 紅茶", STATUS_KEPT, None)
            },
        )
        .await
        .expect("other");
        db.writer
            .transaction(move |tx| {
                tx.execute(
                    "UPDATE synapses SET strength = 5.0 WHERE id = ?1",
                    params![strong],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
            .expect("seed");

        let rows = list_kept(&db, "u1", "b1").await.expect("list");
        assert_eq!(
            rows.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![strong, weak],
            "候補と他人の行は出ず、強い順"
        );
        assert_eq!(count_candidates(&db, "u1", "b1").await.expect("count"), 1);

        let found = find_kept_by_content(&db, "u1", "b1", "紅茶", 10)
            .await
            .expect("find");
        assert_eq!(found.iter().map(|r| r.id).collect::<Vec<_>>(), vec![weak]);
        assert!(
            find_kept_by_content(&db, "u1", "b1", "%", 10)
                .await
                .expect("find")
                .is_empty(),
            "% はワイルドカードではなくリテラル"
        );

        // 他人の行・候補の行は本人として消せない。
        assert!(!delete_kept_owned(&db, "u1", "b1", other)
            .await
            .expect("del"));
        assert!(!delete_kept_owned(&db, "u1", "b1", pending)
            .await
            .expect("del"));
        // 本人の kept は消える（二度目は false）。
        assert!(delete_kept_owned(&db, "u1", "b1", weak).await.expect("del"));
        assert!(!delete_kept_owned(&db, "u1", "b1", weak).await.expect("del"));
        assert_eq!(list_kept(&db, "u1", "b1").await.expect("list").len(), 1);
        assert_eq!(list_kept(&db, "u2", "b1").await.expect("list").len(), 1);
    }
}
