//! 会話記憶の評価器と忘却（docs/design/memory_retention.md §4-B/C/D）。
//!
//! - 評価器 [`evaluate_scope`]: 候補（`status='candidate'`）をまとめて 1 回の LLM 呼び出しで判定し、
//!   残すものは三人称の短い事実文に書き直して kept（索引へ登録）にし、残さないものは消す。
//!   LLM が使えないスコープは、24 時間を過ぎた候補を意味マーカーの有無で振り分ける（縮退）。
//! - 忘却 [`decay_and_prune`]: kept の強度を毎日減衰させ、弱くなった古い記憶と上限超過分を消す。
//! - 有用性 [`useful_ids`]: 想起した記憶の内容語が返答に現れたものを「有用」とみなして強める。
//!
//! この module は DB と RAM 索引だけに触り、LLM は [`MemoryLlm`] ポート越しに呼ぶ（本人キー・Bot キーの
//! 取り出しは supervisor が実装する）。cron の周期と起動条件も supervisor 側（`memory.rs`）。

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::sync::Mutex;
use yuuka_core::{DbError, MetricsSink};
use yuuka_synapse::{FormationContext, Scope, SynapseEngine};
use yuuka_web::Db;

use crate::synapse_extract;
use crate::synapse_repo::{self, Candidate, ScopeKey, Verdict};

/// 1 回の評価で LLM に渡す候補の上限。
pub const EVAL_BATCH: usize = 30;
/// LLM が使えないとき、候補をこの時間（秒）待ってから縮退の振り分けにかける。
pub const FALLBACK_AFTER_SECS: i64 = 24 * 3600;
/// 事実文の最大長（文字）。
pub const STATEMENT_MAX_CHARS: usize = 120;
/// 1 日の減衰率（半減期 約 23 日）。
pub const DAILY_DECAY: f64 = 0.97;
/// `durability=long` の 1 日の減衰率（半減期 約 46 日）。
pub const LONG_DECAY: f64 = 0.985;
/// この強度を下回り、かつ作成から [`FORGET_MIN_AGE_DAYS`] 日を過ぎ、有用期限も過ぎた記憶を消す。
pub const FORGET_THRESHOLD: f64 = 0.2;
/// 作成からこの日数は、弱くても消さない（評価直後の記憶に使われる機会を与える）。
pub const FORGET_MIN_AGE_DAYS: i64 = 14;
/// 秘書（guild なし）スコープの kept の上限。
pub const CAP_SECRETARY: usize = 500;
/// ギルド（汎用モード）スコープの kept の上限。
pub const CAP_GUILD: usize = 300;

/// 評価に使う LLM の呼び出し口。`None` はこのスコープでは LLM を使えない（キー未設定・失敗）。
#[async_trait]
pub trait MemoryLlm: Send + Sync {
    async fn generate(&self, scope: &ScopeKey, prompt: &str) -> Option<String>;
}

/// LLM を使わない実装（テスト・未配線）。常に `None`。
pub struct NullMemoryLlm;

#[async_trait]
impl MemoryLlm for NullMemoryLlm {
    async fn generate(&self, _scope: &ScopeKey, _prompt: &str) -> Option<String> {
        None
    }
}

/// 評価の結果の件数。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EvalReport {
    pub kept: usize,
    pub dropped: usize,
    /// LLM が使えず、まだ待っている候補の数。
    pub deferred: usize,
    /// LLM を呼んだか。
    pub used_llm: bool,
}

/// 忘却の結果の件数。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PruneReport {
    pub decayed: usize,
    pub forgotten: usize,
    pub over_cap: usize,
}

const KINDS: &[&str] = &[
    "preference",
    "fact",
    "constraint",
    "relationship",
    "event",
    "instruction",
];

/// LLM の出力 1 件。
#[derive(Debug, Deserialize)]
struct RawVerdict {
    n: usize,
    #[serde(default)]
    keep: bool,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    statement: Option<String>,
    #[serde(default)]
    durability: Option<String>,
}

/// 評価プロンプトを組み立てる（候補は 1 始まりの番号付き）。
#[must_use]
pub fn eval_prompt(candidates: &[Candidate]) -> String {
    let mut p = String::from(
        "あなたは個人秘書の長期記憶を整理する係です。以下は利用者の発話の断片です。\
         それぞれについて、来月以降も役に立つ「利用者本人についての事実」が含まれているかを判定してください。\n\
         残す基準: 好み・習慣・制約（アレルギー・予定の制約）・家族や友人との関係・本人の目標や予定・\
         秘書への恒久的な指示。残さないもの: その場限りの感想や愚痴、質問、作業の依頼、計算や文章の内容、\
         他人の発言の引用、歌詞やコピペ、秘密情報。\n\
         残す場合は、第三者が読んでも分かる三人称の短い事実文（80 文字以内・「利用者は〜」で始める）に\
         書き直してください。1 つの断片から複数の事実が読み取れるときは最も重要な 1 つだけにします。\n\
         出力は JSON 配列だけを返してください。各要素: {\"n\": 番号, \"keep\": true/false, \
         \"kind\": \"preference|fact|constraint|relationship|event|instruction\", \
         \"statement\": \"事実文\", \"durability\": \"long|medium|short\"}。\
         keep が false の要素は n と keep だけでかまいません。説明文や ``` は付けないでください。\n\n",
    );
    for (i, c) in candidates.iter().enumerate() {
        let one_line: String = c.content.split_whitespace().collect::<Vec<_>>().join(" ");
        p.push_str(&format!("{}. {one_line}\n", i + 1));
    }
    p
}

/// LLM の応答から判定を取り出す（前後の説明文や ``` を許す）。壊れていれば `None`。
#[must_use]
pub fn parse_verdicts(raw: &str, count: usize) -> Option<Vec<(usize, Option<Verdict>)>> {
    let start = raw.find('[')?;
    let end = raw.rfind(']')?;
    if end <= start {
        return None;
    }
    let slice = raw.get(start..=end)?;
    let items: Vec<RawVerdict> = serde_json::from_str(slice).ok()?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        if item.n == 0 || item.n > count {
            continue;
        }
        let verdict = if item.keep {
            let statement = item
                .statement
                .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|s| !s.is_empty())
                .map(|s| s.chars().take(STATEMENT_MAX_CHARS).collect::<String>());
            statement.map(|statement| Verdict {
                statement,
                kind: item
                    .kind
                    .filter(|k| KINDS.contains(&k.as_str()))
                    .unwrap_or_else(|| "fact".to_owned()),
                durability: match item.durability.as_deref() {
                    Some("long") => "long".to_owned(),
                    Some("short") => "short".to_owned(),
                    _ => "medium".to_owned(),
                },
            })
        } else {
            None
        };
        out.push((item.n - 1, verdict));
    }
    Some(out)
}

/// 1 スコープの候補を評価する。LLM が使えなければ縮退の振り分け（24 時間を過ぎた候補だけ）。
///
/// # Errors
/// DB の読み書きに失敗したとき。
pub async fn evaluate_scope(
    db: &Db,
    engine: &Arc<Mutex<SynapseEngine>>,
    llm: &dyn MemoryLlm,
    scope: &ScopeKey,
    metrics: Option<&dyn MetricsSink>,
) -> Result<EvalReport, DbError> {
    let candidates = synapse_repo::list_candidates(db, scope, EVAL_BATCH).await?;
    if candidates.is_empty() {
        return Ok(EvalReport::default());
    }
    let mut report = EvalReport::default();

    let prompt = eval_prompt(&candidates);
    let verdicts = match llm.generate(scope, &prompt).await {
        Some(raw) => match parse_verdicts(&raw, candidates.len()) {
            Some(v) => {
                report.used_llm = true;
                Some(v)
            }
            None => {
                tracing::warn!(
                    user = %scope.user_id,
                    bot = %scope.bot_id,
                    "[Synapse] 評価の応答を読めませんでした（次回にやり直します）"
                );
                return Ok(report);
            }
        },
        None => None,
    };

    let decisions: Vec<(Candidate, Option<Verdict>)> = match verdicts {
        Some(v) => {
            // 応答に無い番号は「残さない」扱い（LLM が省いた＝重要でない）。
            let mut by_index: std::collections::HashMap<usize, Option<Verdict>> =
                v.into_iter().collect();
            candidates
                .into_iter()
                .enumerate()
                .map(|(i, c)| {
                    let verdict = by_index.remove(&i).flatten();
                    (c, verdict)
                })
                .collect()
        }
        None => {
            // 縮退: 24 時間待っても LLM が使えない候補だけ、意味マーカーの有無で振り分ける。
            let mut out = Vec::new();
            for c in candidates {
                if c.age_secs < FALLBACK_AFTER_SECS {
                    report.deferred += 1;
                    continue;
                }
                let verdict = synapse_extract::is_memorable_marker(&c.content).then(|| Verdict {
                    statement: c.content.chars().take(STATEMENT_MAX_CHARS).collect(),
                    kind: "fact".to_owned(),
                    durability: "medium".to_owned(),
                });
                out.push((c, verdict));
            }
            out
        }
    };

    let mut to_delete = Vec::new();
    for (candidate, verdict) in decisions {
        match verdict {
            Some(verdict) => {
                keep(db, engine, scope, &candidate, verdict).await?;
                report.kept += 1;
            }
            None => to_delete.push(candidate.id),
        }
    }
    report.dropped = to_delete.len();
    synapse_repo::delete_synapses(db, to_delete).await?;

    if let Some(m) = metrics {
        m.incr("synapse_kept", report.kept as u64);
        m.incr("synapse_dropped", report.dropped as u64);
    }
    tracing::info!(
        user = %scope.user_id,
        bot = %scope.bot_id,
        guild = ?scope.guild_id,
        kept = report.kept,
        dropped = report.dropped,
        deferred = report.deferred,
        llm = report.used_llm,
        "[Synapse] 記憶の候補を評価しました"
    );
    Ok(report)
}

/// 候補を kept にして索引へ登録する。
async fn keep(
    db: &Db,
    engine: &Arc<Mutex<SynapseEngine>>,
    scope: &ScopeKey,
    candidate: &Candidate,
    verdict: Verdict,
) -> Result<(), DbError> {
    let statement = verdict.statement.clone();
    synapse_repo::keep_candidate(db, candidate.id, verdict).await?;
    let indexed = {
        let mut eng = engine.lock().await;
        eng.index(
            Scope {
                user_id: scope.user_id.clone(),
                bot_id: scope.bot_id.clone(),
                guild_id: scope.guild_id.clone(),
            },
            candidate.id,
            candidate.topic_id.clone(),
            statement,
            FormationContext {
                ctx_tod: None,
                ctx_dow: None,
                created_at: Some(chrono::Local::now().timestamp()),
            },
        )
    };
    synapse_repo::update_synapse_embedding(
        db,
        candidate.id,
        indexed.embedding,
        indexed.model_version,
    )
    .await
}

/// 1 日 1 回の減衰と忘却。
///
/// # Errors
/// DB の読み書きに失敗したとき。
pub async fn decay_and_prune(
    db: &Db,
    engine: &Arc<Mutex<SynapseEngine>>,
    metrics: Option<&dyn MetricsSink>,
) -> Result<PruneReport, DbError> {
    let mut report = PruneReport {
        decayed: synapse_repo::decay_strength(db, DAILY_DECAY, LONG_DECAY).await?,
        ..PruneReport::default()
    };

    let forget =
        synapse_repo::select_forgettable(db, FORGET_THRESHOLD, FORGET_MIN_AGE_DAYS).await?;
    report.forgotten = forget.len();
    remove(db, engine, forget).await?;

    for (scope, count) in synapse_repo::kept_scope_counts(db).await? {
        let cap = if scope.guild_id.is_some() {
            CAP_GUILD
        } else {
            CAP_SECRETARY
        };
        if usize::try_from(count).unwrap_or(usize::MAX) <= cap {
            continue;
        }
        let over = synapse_repo::select_over_cap(db, &scope, cap).await?;
        report.over_cap += over.len();
        remove(db, engine, over).await?;
    }

    if let Some(m) = metrics {
        m.incr(
            "synapse_forgotten",
            (report.forgotten + report.over_cap) as u64,
        );
    }
    tracing::info!(
        decayed = report.decayed,
        forgotten = report.forgotten,
        over_cap = report.over_cap,
        "[Synapse] 記憶の減衰と忘却を実行しました"
    );
    Ok(report)
}

async fn remove(db: &Db, engine: &Arc<Mutex<SynapseEngine>>, ids: Vec<i64>) -> Result<(), DbError> {
    if ids.is_empty() {
        return Ok(());
    }
    {
        let mut eng = engine.lock().await;
        for id in &ids {
            eng.forget(*id);
        }
    }
    synapse_repo::delete_synapses(db, ids).await
}

/// 記憶の内容語（2 文字以上・全部ひらがなでない・助詞や定型語でないトークン）。
fn content_words(statement: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "利用者",
        "です",
        "ます",
        "こと",
        "もの",
        "ため",
        "よう",
        "する",
        "いる",
        "ある",
    ];
    let is_sep = |c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '、' | '。'
                    | '，'
                    | '．'
                    | ','
                    | '.'
                    | '!'
                    | '?'
                    | '！'
                    | '？'
                    | '「'
                    | '」'
                    | '（'
                    | '）'
                    | '('
                    | ')'
                    | '・'
                    | ':'
                    | '：'
                    | 'は'
                    | 'が'
                    | 'を'
                    | 'に'
                    | 'の'
                    | 'と'
                    | 'で'
                    | 'も'
            )
    };
    statement
        .split(is_sep)
        .map(str::trim)
        .filter(|t| t.chars().count() >= 2)
        .filter(|t| !t.chars().all(|c| ('ぁ'..='ん').contains(&c)))
        .filter(|t| !STOP.contains(t))
        .map(str::to_owned)
        .collect()
}

/// 想起した記憶のうち、内容語が返答本文に 1 つでも現れたものの id（＝返答に使われたとみなす）。
#[must_use]
pub fn useful_ids(recalled: &[(i64, String)], reply: &str) -> Vec<i64> {
    if reply.trim().is_empty() {
        return Vec::new();
    }
    recalled
        .iter()
        .filter(|(_, statement)| {
            content_words(statement)
                .iter()
                .any(|w| reply.contains(w.as_str()))
        })
        .map(|(id, _)| *id)
        .collect()
}

/// 返答確定後に有用な記憶を強める（best-effort・失敗は warn）。
pub async fn record_usefulness(db: &Db, recalled: &[(i64, String)], reply: &str) {
    let ids = useful_ids(recalled, reply);
    if ids.is_empty() {
        return;
    }
    if let Err(e) = synapse_repo::mark_useful(db, ids).await {
        tracing::warn!(error = %e, "[Synapse] 有用の記録に失敗（無視）");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::synapse_repo::{InsertArgs, STATUS_CANDIDATE, STATUS_KEPT};
    use std::path::Path;

    struct FixedLlm(Option<String>);

    #[async_trait]
    impl MemoryLlm for FixedLlm {
        async fn generate(&self, _scope: &ScopeKey, _prompt: &str) -> Option<String> {
            self.0.clone()
        }
    }

    fn scope() -> ScopeKey {
        ScopeKey {
            user_id: "u1".into(),
            bot_id: "b1".into(),
            guild_id: None,
        }
    }

    async fn setup() -> (Db, Arc<Mutex<SynapseEngine>>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        drop(rusqlite::Connection::open(&path).unwrap());
        let db = Db::open(Path::new(&path)).unwrap();
        let (engine, _) = SynapseEngine::boot(&path, yuuka_synapse::DEFAULT_DIM);
        (db, Arc::new(Mutex::new(engine)), dir)
    }

    async fn candidate(db: &Db, content: &str) -> i64 {
        synapse_repo::insert_synapse(
            db,
            InsertArgs {
                user_id: "u1".into(),
                bot_id: "b1".into(),
                guild_id: None,
                content: content.into(),
                topic_id: None,
                source_msg_id: None,
                ctx_tod: None,
                ctx_dow: None,
                status: STATUS_CANDIDATE,
                content_hash: None,
            },
        )
        .await
        .unwrap()
    }

    #[test]
    fn parse_accepts_fenced_json_and_normalizes_fields() {
        let raw = "```json\n[{\"n\":1,\"keep\":true,\"kind\":\"preference\",\"statement\":\"利用者は  カレーが好き\",\"durability\":\"long\"},{\"n\":2,\"keep\":false},{\"n\":9,\"keep\":true,\"statement\":\"範囲外\"}]\n```";
        let v = parse_verdicts(raw, 2).unwrap();
        assert_eq!(v.len(), 2);
        let first = v[0].1.as_ref().unwrap();
        assert_eq!(first.statement, "利用者は カレーが好き");
        assert_eq!(first.durability, "long");
        assert!(v[1].1.is_none());
        assert!(parse_verdicts("わかりません", 2).is_none());
        // keep=true でも statement が無ければ残さない。
        let v = parse_verdicts("[{\"n\":1,\"keep\":true}]", 1).unwrap();
        assert!(v[0].1.is_none());
    }

    #[tokio::test]
    async fn llm_verdicts_keep_rewrite_and_index_or_delete() {
        let (db, engine, _dir) = setup().await;
        let a = candidate(&db, "私はカレーが好きなんだよね").await;
        let _b = candidate(&db, "今日は疲れた、もう寝る").await;
        let llm = FixedLlm(Some(
            "[{\"n\":1,\"keep\":true,\"kind\":\"preference\",\"statement\":\"利用者はカレーが好き\",\"durability\":\"long\"},{\"n\":2,\"keep\":false}]".into(),
        ));
        let report = evaluate_scope(&db, &engine, &llm, &scope(), None)
            .await
            .unwrap();
        assert_eq!(
            report,
            EvalReport {
                kept: 1,
                dropped: 1,
                deferred: 0,
                used_llm: true
            }
        );
        assert_eq!(engine.lock().await.total(), 1, "kept だけ索引に入る");
        let counts = synapse_repo::kept_scope_counts(&db).await.unwrap();
        assert_eq!(counts[0].1, 1);
        assert!(synapse_repo::list_candidates(&db, &scope(), 10)
            .await
            .unwrap()
            .is_empty());
        // 想起で事実文が返る。
        let hits = engine.lock().await.assemble(
            &Scope {
                user_id: "u1".into(),
                bot_id: "b1".into(),
                guild_id: None,
            },
            "カレー",
            None,
            None,
            5,
        );
        assert_eq!(
            hits.first().map(|n| (n.id, n.content.as_str())),
            Some((a, "利用者はカレーが好き"))
        );
    }

    #[tokio::test]
    async fn without_llm_fresh_candidates_wait_and_old_ones_fall_back_to_markers() {
        let (db, engine, _dir) = setup().await;
        let fresh = candidate(&db, "私はカレーが好き").await;
        let old_marked = candidate(&db, "毎朝コーヒーを飲む習慣がある").await;
        let old_plain = candidate(&db, "私は明日忙しい").await;
        db.writer
            .transaction(move |tx| {
                tx.execute(
                    "UPDATE synapses SET created_at = datetime('now','localtime','-2 days') WHERE id IN (?1, ?2)",
                    rusqlite::params![old_marked, old_plain],
                )
                .map_err(yuuka_db::map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();
        let report = evaluate_scope(&db, &engine, &NullMemoryLlm, &scope(), None)
            .await
            .unwrap();
        assert_eq!(
            report,
            EvalReport {
                kept: 1,
                dropped: 1,
                deferred: 1,
                used_llm: false
            }
        );
        let left = synapse_repo::list_candidates(&db, &scope(), 10)
            .await
            .unwrap();
        assert_eq!(left.iter().map(|c| c.id).collect::<Vec<_>>(), vec![fresh]);
    }

    #[tokio::test]
    async fn unreadable_llm_output_leaves_candidates_untouched() {
        let (db, engine, _dir) = setup().await;
        candidate(&db, "私はカレーが好き").await;
        let llm = FixedLlm(Some("すみません、分かりません".into()));
        let report = evaluate_scope(&db, &engine, &llm, &scope(), None)
            .await
            .unwrap();
        assert_eq!(report, EvalReport::default());
        assert_eq!(
            synapse_repo::list_candidates(&db, &scope(), 10)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn prune_removes_weak_old_memories_from_db_and_index() {
        let (db, engine, _dir) = setup().await;
        let id = synapse_repo::insert_synapse(
            &db,
            InsertArgs {
                user_id: "u1".into(),
                bot_id: "b1".into(),
                guild_id: None,
                content: "古くて弱い記憶".into(),
                topic_id: None,
                source_msg_id: None,
                ctx_tod: None,
                ctx_dow: None,
                status: STATUS_KEPT,
                content_hash: None,
            },
        )
        .await
        .unwrap();
        db.writer
            .transaction(move |tx| {
                tx.execute(
                    "UPDATE synapses SET created_at = datetime('now','localtime','-40 days'), strength = 0.1 WHERE id = ?1",
                    rusqlite::params![id],
                )
                .map_err(yuuka_db::map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();
        engine.lock().await.index(
            Scope {
                user_id: "u1".into(),
                bot_id: "b1".into(),
                guild_id: None,
            },
            id,
            None,
            "古くて弱い記憶".into(),
            FormationContext::default(),
        );
        let report = decay_and_prune(&db, &engine, None).await.unwrap();
        assert_eq!(report.decayed, 1);
        assert_eq!(report.forgotten, 1);
        assert_eq!(engine.lock().await.total(), 0);
        assert!(synapse_repo::kept_scope_counts(&db)
            .await
            .unwrap()
            .is_empty());
    }

    #[test]
    fn useful_when_content_word_appears_in_reply() {
        let recalled = vec![
            (1, "利用者はカレーが好き".to_owned()),
            (2, "利用者は金曜日は在宅で働く".to_owned()),
        ];
        assert_eq!(
            useful_ids(&recalled, "今夜はカレーにしましょうか？"),
            vec![1]
        );
        assert!(useful_ids(&recalled, "了解です").is_empty());
        assert!(useful_ids(&recalled, "").is_empty());
    }
}
