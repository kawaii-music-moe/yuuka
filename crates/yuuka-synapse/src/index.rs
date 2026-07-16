// ─────────────────────────────────────────────────────────────────────────────
//  RAM ベクトル索引（スコープ別バケット・総当たりコサイン KNN）
// ─────────────────────────────────────────────────────────────────────────────
//
//  この索引は記憶の重い処理（埋め込み保持・近傍探索）を担う RAM 構造。
//  現フェーズは「スコープ単位・総当たりコサイン KNN」。1スコープあたり数千件しか
//  ないため総当たりでもサブミリ秒で完了する（設計書が総当たりを是としている）。
//  将来 USearch 等の近似最近傍へ差し替える際も、本構造体の公開 API
//  （insert / remove / knn / load 系）を保てば呼び出し側は不変。
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::HashMap;

use crate::embedder::cosine;

/// スコープ（user_id / bot_id / guild_id の複合）。
/// guild_id 不在は None として扱い、None 同士・値同士が一致したときのみ同一スコープ。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Scope {
    pub user_id: String,
    pub bot_id: String,
    pub guild_id: Option<String>,
}

/// RAM 上の1シナプス・エントリ。
#[derive(Clone, Debug)]
pub struct Entry {
    pub id: i64,
    pub topic_id: Option<String>,
    pub content: String,
    pub vector: Vec<f32>,
    /// 形成時の時間帯（0-23）。再ランキング専用。None=文脈未知（中立扱い）。
    pub ctx_tod: Option<i64>,
    /// 形成時の曜日（0=日〜6=土）。再ランキング専用。None=文脈未知（中立扱い）。
    pub ctx_dow: Option<i64>,
    /// 形成時刻（Unix エポック秒）。recency 再ランキング専用。None=不明（ブーストなし＝中立）。
    pub created_at: Option<i64>,
}

/// 想起時の時刻文脈（再ランキング指定）。意味KNN後にスコアへ補正をかける。
/// weight=0 のときは補正しない（純粋な意味KNN）。
#[derive(Clone, Copy, Debug)]
pub struct TimeContext {
    pub now_tod: i64,
    pub now_dow: i64,
    pub weight: f32,
}

/// 想起時の recency 文脈（再ランキング指定）。意味KNN後に「形成からの経過時間」で
/// 最近のシナプスを加算ブーストする。weight=0 / halflife<=0 のときは補正しない。
#[derive(Clone, Copy, Debug)]
pub struct RecencyContext {
    /// 現在時刻（Unix エポック秒）。
    pub now_epoch: i64,
    /// 加算ブーストの重み。
    pub weight: f32,
    /// 半減期（秒）。この時間で recency 係数が半分になる。
    pub halflife_secs: f32,
}

/// 時刻近接度 [0,1]。1=完全一致、0=最遠。環状距離（24h/7d の周期）で算出。
/// tod/dow の双方が None なら 0.5（中立）。片方のみ既知ならその次元のみで判定。
fn time_affinity(entry: &Entry, ctx: &TimeContext) -> f32 {
    let mut sum = 0.0f32;
    let mut n = 0u32;
    if let Some(tod) = entry.ctx_tod {
        let d = circular_distance(tod, ctx.now_tod, 24);
        sum += 1.0 - (d as f32) / 12.0; // 最大環状距離 12
        n += 1;
    }
    if let Some(dow) = entry.ctx_dow {
        let d = circular_distance(dow, ctx.now_dow, 7);
        sum += 1.0 - (d as f32) / 3.5; // 最大環状距離 3.5
        n += 1;
    }
    if n == 0 {
        0.5 // 文脈未知＝中立（補正ゼロになる）
    } else {
        sum / n as f32
    }
}

/// 周期 `period` 上での環状距離（0〜period/2）。
fn circular_distance(a: i64, b: i64, period: i64) -> i64 {
    let raw = (a - b).abs() % period;
    raw.min(period - raw)
}

/// recency 係数 [0,1]。1=たった今、経過時間とともに半減期で指数減衰する。
/// created_at=None（形成時刻不明）は 0（ブーストなし＝中立）。
/// 呼び出し側は halflife_secs > 0 を保証すること。
fn recency_factor(entry: &Entry, ctx: &RecencyContext) -> f32 {
    match entry.created_at {
        Some(created) => {
            let age = (ctx.now_epoch - created).max(0) as f32;
            0.5f32.powf(age / ctx.halflife_secs)
        }
        None => 0.0,
    }
}

/// 1スコープあたりの保持上限。超過時は最小 id を退避（FIFO 近似）し、
/// メモリが無限に増えないよう上限を固定する（§5.4）。
pub const MAX_PER_SCOPE: usize = 5000;

/// KNN の結果1件。
#[derive(Clone, Debug)]
pub struct Neighbor {
    pub id: i64,
    pub content: String,
    pub topic_id: Option<String>,
    pub score: f32,
}

/// スコープ別バケットを束ねる RAM 索引。
#[derive(Default)]
pub struct SynapseIndex {
    /// スコープ → エントリ群。
    buckets: HashMap<Scope, Vec<Entry>>,
}

impl SynapseIndex {
    /// 期待次元は現状ベクトル長検証に使わず（呼び出し側が embedder.dim で検証）、
    /// 将来の近似最近傍実装への差し替え互換のためシグネチャだけ受ける。
    #[must_use]
    pub fn new(_dim: usize) -> Self {
        SynapseIndex {
            buckets: HashMap::new(),
        }
    }

    /// 全スコープ合計の保持件数。
    #[must_use]
    pub fn total(&self) -> usize {
        self.buckets.values().map(Vec::len).sum()
    }

    /// エントリを挿入／置換（id をキーに同一スコープ内で重複排除）。
    /// スコープ上限を超える場合は最小 id を退避してから挿入する。
    pub fn insert(&mut self, scope: Scope, entry: Entry) {
        // 念のため：他スコープに同 id が残っていると forget(id) の意味が曖昧になるため、
        // 異なるスコープに同一 id があれば取り除いてから入れ直す。
        self.remove_from_other_scopes(&scope, entry.id);

        let bucket = self.buckets.entry(scope).or_default();

        // 既存（同 id）を置換。
        if let Some(slot) = bucket.iter_mut().find(|e| e.id == entry.id) {
            *slot = entry;
            return;
        }

        // 上限超過なら最小 id を退避（FIFO 近似）。
        if bucket.len() >= MAX_PER_SCOPE {
            if let Some((min_pos, _)) = bucket.iter().enumerate().min_by_key(|(_, e)| e.id) {
                let evicted = bucket.swap_remove(min_pos);
                // 退避ログは過剰に出さない（上限到達時のみ）。
                eprintln!(
                    "[synapse] スコープ上限({})到達: id={} を退避しました",
                    MAX_PER_SCOPE, evicted.id
                );
            }
        }

        bucket.push(entry);
    }

    /// 指定スコープ以外のバケットから同一 id を取り除く（内部ヘルパ）。
    fn remove_from_other_scopes(&mut self, keep: &Scope, id: i64) {
        for (scope, bucket) in &mut self.buckets {
            if scope == keep {
                continue;
            }
            bucket.retain(|e| e.id != id);
        }
    }

    /// id でエントリを除去（どのスコープにあっても）。除去できたら true。
    pub fn remove(&mut self, id: i64) -> bool {
        let mut removed = false;
        for bucket in self.buckets.values_mut() {
            let before = bucket.len();
            bucket.retain(|e| e.id != id);
            if bucket.len() != before {
                removed = true;
            }
        }
        // 空になったバケットは掃除する。
        self.buckets.retain(|_, v| !v.is_empty());
        removed
    }

    /// 指定スコープに対する総当たりコサイン KNN（スコア降順、最大 k 件）。
    /// `time_ctx` 指定かつ weight>0 のとき、コサインスコアへ時刻近接（環状）の補正をかける。
    /// `recency` 指定かつ weight>0 のとき、形成からの経過時間で最近のシナプスを加算ブーストする。
    /// いずれも意味埋め込みは変えず、最終スコアのみ再ランキングする。
    #[must_use]
    pub fn knn(
        &self,
        scope: &Scope,
        query: &[f32],
        k: usize,
        time_ctx: Option<TimeContext>,
        recency: Option<RecencyContext>,
    ) -> Vec<Neighbor> {
        let Some(bucket) = self.buckets.get(scope) else {
            return Vec::new();
        };

        let mut scored: Vec<(f32, &Entry)> = bucket
            .iter()
            .map(|e| {
                let mut score = cosine(query, &e.vector);
                // 時刻補正: weight*(affinity-0.5)。affinity=0.5（中立/文脈未知）で補正ゼロ。
                if let Some(ctx) = time_ctx {
                    if ctx.weight > 0.0 {
                        score += ctx.weight * (time_affinity(e, &ctx) - 0.5);
                    }
                }
                // recency 加算ブースト: weight*recency_factor。最近ほど大きく持ち上げ、古い項は ~0。
                if let Some(rc) = recency {
                    if rc.weight > 0.0 && rc.halflife_secs > 0.0 {
                        score += rc.weight * recency_factor(e, &rc);
                    }
                }
                (score, e)
            })
            .collect();

        // スコア降順（NaN は最後尾相当に倒す）。
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        scored
            .into_iter()
            .take(k)
            .map(|(score, e)| Neighbor {
                id: e.id,
                content: e.content.clone(),
                topic_id: e.topic_id.clone(),
                score,
            })
            .collect()
    }
}

/// f32 ベクトルを「リトルエンディアン・連続バイト列」へ。
/// これは Node 側 SQLite BLOB との **ハード契約**（dim*4 バイト）。
#[must_use]
pub fn vector_to_le_bytes(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for &x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

/// リトルエンディアン f32 バイト列を Vec<f32> へ復号。
/// 長さが4の倍数でなければ None（呼び出し側でスキップ＋警告）。
#[must_use]
pub fn le_bytes_to_vector(bytes: &[u8]) -> Option<Vec<f32>> {
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        let arr = [
            *chunk.first()?,
            *chunk.get(1)?,
            *chunk.get(2)?,
            *chunk.get(3)?,
        ];
        out.push(f32::from_le_bytes(arr));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_entry(id: i64, created_at: Option<i64>) -> Entry {
        Entry {
            id,
            topic_id: None,
            content: format!("c{id}"),
            // 全件同一ベクトル → cosine は同点になり、recency 補正だけが順位を決める。
            vector: vec![1.0, 0.0, 0.0],
            ctx_tod: None,
            ctx_dow: None,
            created_at,
        }
    }

    fn scope() -> Scope {
        Scope {
            user_id: "u".into(),
            bot_id: "b".into(),
            guild_id: None,
        }
    }

    #[test]
    fn recency_boosts_newer_when_weight_positive() {
        let mut idx = SynapseIndex::new(3);
        idx.insert(scope(), mk_entry(1, Some(1_000))); // 古い
        idx.insert(scope(), mk_entry(2, Some(100_000))); // 新しい
        let q = vec![1.0, 0.0, 0.0];
        let rc = RecencyContext {
            now_epoch: 100_000,
            weight: 0.5,
            halflife_secs: 3600.0,
        };
        let res = idx.knn(&scope(), &q, 2, None, Some(rc));
        let (first, second) = (res.first().expect("1件目"), res.get(1).expect("2件目"));
        assert_eq!(first.id, 2, "recency 有効なら新しい方が上位に来るべき");
        assert!(first.score > second.score, "新しい方のスコアが高いべき");
    }

    #[test]
    fn recency_disabled_keeps_scores_tied() {
        // recency 無効（None）では cosine 同点 → スコアは等しい（順位は不変）。
        let mut idx = SynapseIndex::new(3);
        idx.insert(scope(), mk_entry(1, Some(1_000)));
        idx.insert(scope(), mk_entry(2, Some(100_000)));
        let q = vec![1.0, 0.0, 0.0];
        let res = idx.knn(&scope(), &q, 2, None, None);
        let (first, second) = (res.first().expect("1件目"), res.get(1).expect("2件目"));
        assert!(
            (first.score - second.score).abs() < 1e-6,
            "recency 無効なら cosine 同点でスコアも同点であるべき"
        );
    }

    #[test]
    fn recency_none_created_at_is_neutral() {
        // created_at 不明（None）はブーストされず、既知で最近の方が上位になる。
        let mut idx = SynapseIndex::new(3);
        idx.insert(scope(), mk_entry(1, None)); // 形成時刻不明
        idx.insert(scope(), mk_entry(2, Some(100_000))); // 最近
        let q = vec![1.0, 0.0, 0.0];
        let rc = RecencyContext {
            now_epoch: 100_000,
            weight: 0.5,
            halflife_secs: 3600.0,
        };
        let res = idx.knn(&scope(), &q, 2, None, Some(rc));
        assert_eq!(
            res.first().expect("1件目").id,
            2,
            "created_at 既知で最近の方がブーストされ上位"
        );
    }

    #[test]
    fn le_bytes_roundtrip() {
        let v = vec![1.0f32, -0.5, 0.25];
        let bytes = vector_to_le_bytes(&v);
        assert_eq!(bytes.len(), 12);
        let back = le_bytes_to_vector(&bytes).expect("復号できる");
        assert_eq!(v, back);
        assert!(le_bytes_to_vector(&[0u8; 3]).is_none(), "4の倍数でないと None");
    }

    #[test]
    fn remove_deletes_across_scopes() {
        let mut idx = SynapseIndex::new(3);
        idx.insert(scope(), mk_entry(1, None));
        assert_eq!(idx.total(), 1);
        assert!(idx.remove(1));
        assert_eq!(idx.total(), 0);
        assert!(!idx.remove(1), "存在しない id の除去は false");
    }
}
