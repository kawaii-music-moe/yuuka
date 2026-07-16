//! L2 連想想起（read 経路）— Node `src/gemini.ts` `buildRecallSection`/`buildRecallQuery` パリティ。
//!
//! シナプスエンジン（[`yuuka_synapse::SynapseEngine`]）へ問い合わせ、入力に意味的に近い過去の記憶
//! （1st Hop KNN）を「思考のカンペ（極小コンテキスト）」として整形し、systemInstruction 末尾へ追記する。
//! エンジン想起が空/失敗のときは `""` を返し、現行挙動（直近履歴のみ）へデグレードする（best-effort）。
//! 想起したシナプスは鮮度（`use_count`/`last_used_at`/`decay_score`）を更新する（失敗は無視）。

use std::sync::Arc;

use tokio::sync::Mutex;
use yuuka_synapse::{RecencyContext, Scope, SynapseEngine, TimeContext};

use crate::synapse_repo;
use yuuka_web::Db;

// ─── Node config 既定値（src/config.ts）を逐語移植 ────────────────────────────────

/// L2 想起（1st Hop KNN）の取得件数（Node `SYNAPSE_RECALL_K` 既定 5）。
const RECALL_K: usize = 5;

/// recency 加算ブースト重み（Node `SYNAPSE_RECENCY_WEIGHT` 既定 0.15）。0 で無効。
const RECENCY_WEIGHT: f32 = 0.15;

/// recency ブーストの半減期（時間・Node `SYNAPSE_RECENCY_HALFLIFE_HOURS` 既定 18）。
const RECENCY_HALFLIFE_HOURS: f32 = 18.0;

/// 時刻文脈の再ランキング重み（Node `SYNAPSE_TIME_BIAS_WEIGHT` = 0.1）。意味KNN後にコサインへ補正。
const TIME_BIAS_WEIGHT: f32 = 0.1;

/// 返信スレッド化クエリの最大長（字面 n-gram 埋め込みの希釈を避ける上限・Node `RECALL_QUERY_MAX_LEN`）。
const RECALL_QUERY_MAX_LEN: usize = 500;

/// L2 想起のクエリを組み立てる（Node `buildRecallQuery`）。
///
/// 返信時（`reply_chain` 非空）は直近 2 件の返信元文脈を現発話へ前置して「数個前をさかのぼる」想起を
/// 効かせ、上限でキャップする。返信でない場合は現発話そのまま。現状 Rust の秘書経路は返信チェーンを
/// 縮退シームとして持たない（空を渡す）ため、実質は現発話がそのままクエリになる。
#[must_use]
pub fn build_recall_query(message_text: &str, reply_chain: &[String]) -> String {
    let text = message_text.trim();
    if reply_chain.is_empty() {
        return text.to_owned();
    }
    // 返信元は古い順に並ぶため、直近 2 件を採用する。現発話は必ず末尾に残す。
    let start = reply_chain.len().saturating_sub(2);
    let mut parts: Vec<&str> = reply_chain
        .get(start..)
        .unwrap_or(&[])
        .iter()
        .map(String::as_str)
        .filter(|s| !s.trim().is_empty())
        .collect();
    if !text.is_empty() {
        parts.push(text);
    }
    let combined = parts.join("\n");
    if combined.chars().count() > RECALL_QUERY_MAX_LEN {
        // Node は末尾 RECALL_QUERY_MAX_LEN 文字を残す（現発話を保持するため）。
        let skip = combined.chars().count() - RECALL_QUERY_MAX_LEN;
        combined.chars().skip(skip).collect()
    } else {
        combined
    }
}

/// 想起セクションを組み立てて systemInstruction 末尾へ追記する文字列を返す（Node `buildRecallSection`）。
///
/// エンジン想起が空・クエリが空・失敗のときは `""`（呼び出し側は systemInstruction をそのまま使う）。
/// 想起できた場合は鮮度更新（`touch_synapses`）を best-effort で行う（失敗は無視）。
pub async fn build_recall_section(
    db: &Db,
    engine: &Arc<Mutex<SynapseEngine>>,
    scope: &Scope,
    query: &str,
) -> String {
    let q = query.trim();
    if q.is_empty() {
        return String::new();
    }

    // 現在の時間帯・曜日・エポックで時刻補正 + recency 加算ブーストをかける。
    let (now_tod, now_dow, now_epoch) = local_now();
    let time_ctx = Some(TimeContext {
        now_tod,
        now_dow,
        weight: TIME_BIAS_WEIGHT,
    });
    let recency_ctx = if RECENCY_WEIGHT > 0.0 && RECENCY_HALFLIFE_HOURS > 0.0 {
        Some(RecencyContext {
            now_epoch,
            weight: RECENCY_WEIGHT,
            halflife_secs: RECENCY_HALFLIFE_HOURS * 3600.0,
        })
    } else {
        None
    };

    let neighbors = {
        let eng = engine.lock().await;
        eng.assemble(scope, q, time_ctx, recency_ctx, RECALL_K)
    };
    if neighbors.is_empty() {
        return String::new();
    }

    // 想起されたシナプスの鮮度を更新（id 群はスコープ済み assemble の結果）。失敗は無視。
    let ids: Vec<i64> = neighbors.iter().map(|n| n.id).collect();
    if let Err(e) = synapse_repo::touch_synapses(db, ids).await {
        tracing::warn!(error = %e, "[Synapse] touch（鮮度更新）に失敗（無視）");
    }

    let lines = neighbors
        .iter()
        .map(|n| format!("- {}", n.content))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "\n# 関連する過去の記憶（連想想起／参考情報）\n\
         以下はあなたが過去の会話から記憶した、現在の発話に関連し得る事項です。\
         関連する場合のみ参考にし、無関係なら無視してください（古い情報の可能性があります）。\n{lines}"
    )
}

/// 現地時刻の (時間帯 0-23, 曜日 0=日〜6=土, Unix エポック秒)。
fn local_now() -> (i64, i64, i64) {
    use chrono::{Datelike, Local, Timelike};
    let now = Local::now();
    (
        i64::from(now.hour()),
        i64::from(now.weekday().num_days_from_sunday()),
        now.timestamp(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recall_query_without_reply_chain_is_message_text() {
        assert_eq!(
            build_recall_query("  好きな食べ物はカレー  ", &[]),
            "好きな食べ物はカレー"
        );
    }

    #[test]
    fn recall_query_prepends_last_two_reply_context() {
        let chain = vec![
            "古い1".to_owned(),
            "古い2".to_owned(),
            "直近1".to_owned(),
            "直近2".to_owned(),
        ];
        // 直近 2 件（直近1・直近2）+ 現発話が改行結合される。
        assert_eq!(
            build_recall_query("現発話", &chain),
            "直近1\n直近2\n現発話"
        );
    }

    #[test]
    fn recall_query_caps_to_max_len_keeping_tail() {
        let chain = vec!["あ".repeat(600)];
        let q = build_recall_query("末尾", &chain);
        assert_eq!(q.chars().count(), RECALL_QUERY_MAX_LEN);
        assert!(q.ends_with("末尾"), "現発話（末尾）は保持される");
    }

    /// 想起 0 件のときはセクションが空（現行挙動へデグレード）。
    #[tokio::test]
    async fn empty_recall_yields_empty_section() {
        use std::path::Path;
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("t.db");
        // P0-4: Rust は存在しない DB を作らないため空ファイルを先に作る（Db::open が migrations 実行）。
        drop(rusqlite::Connection::open(&path).expect("create empty"));
        let db = Db::open(Path::new(&path)).expect("db");
        let (engine, _) = SynapseEngine::boot(&path, yuuka_synapse::DEFAULT_DIM);
        let engine = Arc::new(Mutex::new(engine));
        let scope = Scope {
            user_id: "u1".into(),
            bot_id: "b1".into(),
            guild_id: None,
        };
        let section = build_recall_section(&db, &engine, &scope, "何か").await;
        assert!(section.is_empty(), "索引が空なら想起セクションは空");
        // 空クエリも空。
        assert!(build_recall_section(&db, &engine, &scope, "   ")
            .await
            .is_empty());
    }

    /// 索引に記憶を入れると想起され、フォーマット済みセクションが返り鮮度も更新される。
    #[tokio::test]
    async fn recall_formats_section_and_touches() {
        use std::path::Path;
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("t.db");
        // P0-4: Rust は存在しない DB を作らないため空ファイルを先に作る（Db::open が migrations 実行）。
        drop(rusqlite::Connection::open(&path).expect("create empty"));
        let db = Db::open(Path::new(&path)).expect("db");
        let (mut engine, _) = SynapseEngine::boot(&path, yuuka_synapse::DEFAULT_DIM);
        let scope = Scope {
            user_id: "u1".into(),
            bot_id: "b1".into(),
            guild_id: None,
        };
        // DB へ 1 行 insert（id=1）してから索引へ登録（recall→touch が id=1 を更新できるように）。
        synapse_repo::insert_synapse(
            &db,
            synapse_repo::InsertArgs {
                user_id: "u1".into(),
                bot_id: "b1".into(),
                guild_id: None,
                content: "好きな食べ物はカレーです".into(),
                topic_id: Some("カレー".into()),
                source_msg_id: None,
                ctx_tod: None,
                ctx_dow: None,
            },
        )
        .await
        .expect("insert");
        engine.index(
            scope.clone(),
            1,
            Some("カレー".into()),
            "好きな食べ物はカレーです".into(),
            yuuka_synapse::FormationContext::default(),
        );
        let engine = Arc::new(Mutex::new(engine));

        let section = build_recall_section(&db, &engine, &scope, "好きな食べ物はカレーです").await;
        assert!(
            section.contains("# 関連する過去の記憶（連想想起／参考情報）"),
            "見出しを含む"
        );
        assert!(section.contains("- 好きな食べ物はカレーです"), "content を箇条書きに含む");

        // 鮮度更新（use_count）が反映されている。
        let use_count: i64 = db
            .read
            .read(|conn| {
                conn.query_row("SELECT use_count FROM synapses WHERE id = 1", [], |r| r.get(0))
                    .map_err(yuuka_db::map_sqlite)
            })
            .await
            .expect("read");
        assert_eq!(use_count, 1, "recall で touch されている");
    }
}
