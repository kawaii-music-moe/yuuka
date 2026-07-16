//! yuuka シナプス認知エンジン（in-process ライブラリ）。
//!
//! 旧 `src/rust_synapse` の常駐デーモン（改行 JSON over stdio）を **プロセス内 API** として
//! 吸収したもの（Phase H）。子プロセス/IPC を廃し、[`SynapseEngine`] を直接メソッド呼び出しする。
//!
//! 全体は **完全自己完結・LLM 非依存**: ヒューリスティック抽出 + [`HashNgramEmbedder`]
//! （FNV-1a 文字 n-gram）+ 総当たりコサイン KNN 想起。埋め込みバイト契約（f32 リトルエンディアン
//! 連続 BLOB）は Node と不変で相互運用する。
//!
//! # 使い方
//! - 起動時に [`SynapseEngine::boot`] で DB から RAM 索引を一度ロードする。
//! - 想起（read）: [`SynapseEngine::assemble`]。
//! - 索引化（write）: [`SynapseEngine::index`]（埋め込みバイト列を返し、呼び出し側が SQLite へ永続化）。
//! - 削除 / 再構築 / ヘルス: [`SynapseEngine::forget`] / [`SynapseEngine::reindex`] / [`SynapseEngine::health`]。
//!
//! 共有可変アクセスが要る場合は `Arc<Mutex<SynapseEngine>>` で包む（これは唯一の RAM 索引）。

mod embedder;
mod index;
mod storage;

use std::path::{Path, PathBuf};

pub use embedder::{cosine, Embedder, HashNgramEmbedder, MODEL_VERSION};
pub use index::{
    le_bytes_to_vector, vector_to_le_bytes, Entry, Neighbor, RecencyContext, Scope, SynapseIndex,
    TimeContext, MAX_PER_SCOPE,
};

/// 既定の埋め込み次元（旧デーモン `--dim` 既定・Node と一致させること）。
pub const DEFAULT_DIM: usize = 256;

/// シナプス形成時の再ランキング専用文脈（意味埋め込みには混ぜない）。全て未知なら [`Default`]。
#[derive(Clone, Copy, Debug, Default)]
pub struct FormationContext {
    /// 形成時の時間帯（0-23）。`None`=文脈未知（中立）。
    pub ctx_tod: Option<i64>,
    /// 形成時の曜日（0=日〜6=土）。`None`=文脈未知（中立）。
    pub ctx_dow: Option<i64>,
    /// 形成時刻（Unix エポック秒）。`None`=不明（recency ブーストなし＝中立）。
    pub created_at: Option<i64>,
}

/// エンジンの健全性サマリ（旧 `health` コマンドの結果）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Health {
    pub model_version: &'static str,
    pub dim: usize,
    pub total: usize,
}

/// 索引化（write）の結果。埋め込みバイト列（f32 LE・SQLite BLOB 契約）とモデル世代を返す。
/// 呼び出し側はこの `embedding` を `synapses.embedding` へ、`model_version` を
/// `embedding_model_version` へ書き込む。
#[derive(Clone, Debug)]
pub struct Indexed {
    /// 埋め込みの生バイト列（リトルエンディアン f32 連続・dim*4 バイト）。
    pub embedding: Vec<u8>,
    /// 埋め込みモデル世代（不一致は再埋め込み対象）。
    pub model_version: &'static str,
    /// 出力次元数。
    pub dim: usize,
}

/// in-process シナプスエンジン（埋め込み器 + RAM 索引 + DB パスを 1 つの所有構造に束ねる）。
///
/// 旧デーモンの `Engine` と `handle(req)` ディスパッチをメソッド API へ展開したもの。
/// スレッド安全な共有は呼び出し側が `Arc<Mutex<..>>` で行う（本体は内部可変性を持たない）。
pub struct SynapseEngine {
    embedder: Box<dyn Embedder>,
    index: SynapseIndex,
    db_path: PathBuf,
}

impl SynapseEngine {
    /// 起動時: read-only で DB を読み索引を構築する。失敗しても**空索引で続行**する（panic 厳禁）。
    ///
    /// ロード件数を返す第2要素（Node の「DB から N 件のシナプスを RAM 索引へロード」ログに相当）。
    #[must_use]
    pub fn boot(db_path: impl Into<PathBuf>, dim: usize) -> (Self, LoadOutcome) {
        let db_path = db_path.into();
        let embedder: Box<dyn Embedder> = make_embedder(dim);

        let (index, outcome) = match storage::load_index(&db_path, embedder.as_ref()) {
            Ok((idx, loaded)) => (idx, LoadOutcome::Loaded(loaded)),
            Err(e) => (SynapseIndex::new(dim), LoadOutcome::Empty(e)),
        };

        (
            Self {
                embedder,
                index,
                db_path,
            },
            outcome,
        )
    }

    /// エンジンのヘルス（モデル世代・次元・総件数）。
    #[must_use]
    pub fn health(&self) -> Health {
        Health {
            model_version: self.embedder.model_version(),
            dim: self.embedder.dim(),
            total: self.index.total(),
        }
    }

    /// 全スコープ合計の保持件数。
    #[must_use]
    pub fn total(&self) -> usize {
        self.index.total()
    }

    /// 埋め込みモデル世代。
    #[must_use]
    pub fn model_version(&self) -> &'static str {
        self.embedder.model_version()
    }

    /// 出力次元数。
    #[must_use]
    pub fn dim(&self) -> usize {
        self.embedder.dim()
    }

    /// L2 連想想起（1st Hop KNN）。`query_text` を埋め込み、`scope` バケットへ総当たりコサインで
    /// 近傍探索する。`time_ctx` / `recency_ctx` は任意の再ランキング（意味埋め込みは変えない）。
    ///
    /// 旧 `assemble` コマンド相当。近傍が無ければ空 Vec を返す。
    #[must_use]
    pub fn assemble(
        &self,
        scope: &Scope,
        query_text: &str,
        time_ctx: Option<TimeContext>,
        recency_ctx: Option<RecencyContext>,
        limit: usize,
    ) -> Vec<Neighbor> {
        let qvec = self.embedder.embed(query_text);
        self.index.knn(scope, &qvec, limit, time_ctx, recency_ctx)
    }

    /// シナプスを埋め込み、RAM 索引へ登録し、永続化用の埋め込みバイト列を返す（旧 `index` コマンド）。
    ///
    /// `id` は SQLite で採番済みのシナプス ID。`ctx` は再ランキング専用の任意の形成時文脈
    /// （未知なら [`FormationContext::default`]）。返した [`Indexed::embedding`] を呼び出し側が
    /// SQLite へ書き戻す。
    pub fn index(
        &mut self,
        scope: Scope,
        id: i64,
        topic_id: Option<String>,
        content: String,
        ctx: FormationContext,
    ) -> Indexed {
        let vector = self.embedder.embed(&content);
        let embedding = vector_to_le_bytes(&vector);
        let model_version = self.embedder.model_version();
        let dim = self.embedder.dim();

        self.index.insert(
            scope,
            Entry {
                id,
                topic_id,
                content,
                vector,
                ctx_tod: ctx.ctx_tod,
                ctx_dow: ctx.ctx_dow,
                created_at: ctx.created_at,
            },
        );

        Indexed {
            embedding,
            model_version,
            dim,
        }
    }

    /// RAM 索引から id を除去する（旧 `forget` コマンド）。除去できたら `true`。
    pub fn forget(&mut self, id: i64) -> bool {
        self.index.remove(id)
    }

    /// RAM 索引を SQLite から再構築する（旧 `reindex` コマンド）。成功時は総件数を返し、失敗時は
    /// **現索引を維持**して `Err`（日本語メッセージ）を返す（クラッシュ厳禁）。
    ///
    /// # Errors
    /// DB の再読込に失敗した場合、日本語メッセージ付きの `Err`。
    pub fn reindex(&mut self) -> Result<usize, String> {
        let (idx, _loaded) = storage::load_index(&self.db_path, self.embedder.as_ref())?;
        self.index = idx;
        Ok(self.index.total())
    }

    /// 構築時に読んだ DB パス（ログ用）。
    #[must_use]
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }
}

/// [`SynapseEngine::boot`] のロード結果（ログ用に件数/エラーを持ち回す）。
#[derive(Clone, Debug)]
pub enum LoadOutcome {
    /// DB から N 件をロードできた。
    Loaded(usize),
    /// DB を読めず空索引で起動した（付随エラーメッセージ）。
    Empty(String),
}

/// 埋め込み器の生成（将来の ONNX 差し替えはここを feature ゲートで分岐させる）。
fn make_embedder(dim: usize) -> Box<dyn Embedder> {
    Box::new(HashNgramEmbedder::new(dim))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::TempDir;

    /// synapses テーブルだけを持つ最小 DB を作る（V17 の関連列に一致）。
    fn make_db(dir: &TempDir) -> PathBuf {
        let path = dir.path().join("t.db");
        let conn = Connection::open(&path).expect("open");
        conn.execute_batch(
            "CREATE TABLE synapses (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               user_id TEXT NOT NULL, bot_id TEXT NOT NULL, guild_id TEXT,
               content TEXT NOT NULL, topic_id TEXT, source_msg_id INTEGER,
               embedding BLOB, embedding_model_version TEXT,
               created_at TEXT NOT NULL DEFAULT (datetime('now','localtime')),
               last_used_at TEXT, use_count INTEGER NOT NULL DEFAULT 0,
               decay_score REAL NOT NULL DEFAULT 1.0,
               ctx_tod INTEGER, ctx_dow INTEGER
             );",
        )
        .expect("create");
        path
    }

    fn scope() -> Scope {
        Scope {
            user_id: "u1".into(),
            bot_id: "b1".into(),
            guild_id: None,
        }
    }

    #[test]
    fn boot_empty_db_yields_zero_total() {
        let dir = TempDir::new().expect("tmp");
        let path = make_db(&dir);
        let (engine, outcome) = SynapseEngine::boot(path, DEFAULT_DIM);
        assert!(matches!(outcome, LoadOutcome::Loaded(0)));
        assert_eq!(engine.total(), 0);
        assert_eq!(engine.model_version(), MODEL_VERSION);
        assert_eq!(engine.dim(), DEFAULT_DIM);
    }

    #[test]
    fn boot_missing_db_continues_empty() {
        // 存在しない DB は Err→空索引で起動継続（panic しない）。
        let (engine, outcome) = SynapseEngine::boot("/nonexistent/path/x.db", DEFAULT_DIM);
        assert!(matches!(outcome, LoadOutcome::Empty(_)));
        assert_eq!(engine.total(), 0);
    }

    #[test]
    fn index_assemble_forget_roundtrip() {
        let dir = TempDir::new().expect("tmp");
        let path = make_db(&dir);
        let (mut engine, _) = SynapseEngine::boot(path, DEFAULT_DIM);

        let indexed = engine.index(
            scope(),
            1,
            Some("カレー".into()),
            "好きな食べ物はカレーです".into(),
            FormationContext {
                ctx_tod: Some(12),
                ctx_dow: Some(3),
                created_at: Some(1_000_000),
            },
        );
        assert_eq!(indexed.model_version, MODEL_VERSION);
        assert_eq!(indexed.embedding.len(), DEFAULT_DIM * 4, "dim*4 バイトの BLOB 契約");
        assert_eq!(engine.total(), 1);

        // 意味的に近いクエリで想起できる（同文なので上位ヒット）。
        let hits = engine.assemble(&scope(), "好きな食べ物はカレーです", None, None, 5);
        assert_eq!(hits.first().expect("1件目").id, 1);
        assert!(hits.first().expect("1件目").score > 0.9, "同文はコサイン高");

        // 別スコープからは想起されない（データ分離）。
        let other = Scope {
            user_id: "other".into(),
            bot_id: "b1".into(),
            guild_id: None,
        };
        assert!(engine.assemble(&other, "カレー", None, None, 5).is_empty());

        // forget で消える。
        assert!(engine.forget(1));
        assert_eq!(engine.total(), 0);
        assert!(engine.assemble(&scope(), "カレー", None, None, 5).is_empty());
    }

    #[test]
    fn reindex_rebuilds_from_db() {
        let dir = TempDir::new().expect("tmp");
        let path = make_db(&dir);
        // DB に埋め込み付きの行を直接挿入してから reindex で拾えることを確認する。
        {
            let conn = Connection::open(&path).expect("open");
            let embedder = HashNgramEmbedder::new(DEFAULT_DIM);
            let bytes = vector_to_le_bytes(&embedder.embed("毎朝コーヒーを飲む"));
            conn.execute(
                "INSERT INTO synapses (id, user_id, bot_id, content, embedding, embedding_model_version) \
                 VALUES (1, 'u1', 'b1', '毎朝コーヒーを飲む', ?1, ?2)",
                rusqlite::params![bytes, MODEL_VERSION],
            )
            .expect("insert");
        }
        let (mut engine, _) = SynapseEngine::boot(&path, DEFAULT_DIM);
        // boot でも拾える（embedding 非 NULL）。
        assert_eq!(engine.total(), 1);
        let total = engine.reindex().expect("reindex ok");
        assert_eq!(total, 1);
        let hits = engine.assemble(&scope(), "毎朝コーヒーを飲む", None, None, 5);
        assert_eq!(hits.first().expect("1件目").id, 1);
    }

    #[test]
    fn health_reports_model_and_total() {
        let dir = TempDir::new().expect("tmp");
        let path = make_db(&dir);
        let (mut engine, _) = SynapseEngine::boot(path, 128);
        engine.index(
            scope(),
            1,
            None,
            "テスト内容です".into(),
            FormationContext::default(),
        );
        let h = engine.health();
        assert_eq!(h.model_version, MODEL_VERSION);
        assert_eq!(h.dim, 128);
        assert_eq!(h.total, 1);
    }
}
