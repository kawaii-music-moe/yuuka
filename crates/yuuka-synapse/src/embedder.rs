// ─────────────────────────────────────────────────────────────────────────────
//  埋め込み生成モジュール（Embedder）
// ─────────────────────────────────────────────────────────────────────────────
//
//  【現状＝レキシカル・フォールバック】
//  本モジュールの既定実装 `HashNgramEmbedder` は、外部依存ゼロ・決定論的な
//  「ハッシュ文字 n-gram」埋め込みである。意味的な近さではなく字面（語彙）の
//  重なりを捉えるに過ぎないため、あくまで本物のニューラル埋め込みが入るまでの
//  繋ぎ（フォールバック）と位置づける。CJK 混在・短文でも破綻しないよう
//  unigram も併用する。
//
//  【将来＝ONNX 実埋め込みモデルの差し込みポイント】
//  architecture_renewal_v3.md §記憶コア／表(埋め込み)に従い、ここが
//  `bge-micro` 級（INT8 量子化）を `ort`（ONNX Runtime）もしくは `candle`
//  で駆動する実埋め込みへ差し替える「唯一の場所」である。
//  トレイト境界（`embed(&str) -> Vec<f32>` と `dim()` / `model_version()`）と
//  「f32・リトルエンディアン・連続」のバイト契約は不変に保つこと。これにより
//  Node 側の SQLite BLOB 永続化フォーマットを壊さずに中身だけ差し替えられる。
// ─────────────────────────────────────────────────────────────────────────────

/// 埋め込み器の抽象。実装を差し替えても呼び出し側は不変。
pub trait Embedder: Send + Sync {
    /// 入力文字列を `dim()` 次元の L2 正規化済みベクトルへ。
    fn embed(&self, text: &str) -> Vec<f32>;
    /// 出力次元数。
    fn dim(&self) -> usize;
    /// モデル識別子（埋め込みの互換性キー）。
    fn model_version(&self) -> &'static str;
}

/// 既定（レキシカル・フォールバック）埋め込みのモデル識別子。
pub const MODEL_VERSION: &str = "hash-ngram-v1";

/// ハッシュ文字 n-gram 埋め込み器（依存ゼロ・決定論的）。
pub struct HashNgramEmbedder {
    dim: usize,
}

impl HashNgramEmbedder {
    #[must_use]
    pub fn new(dim: usize) -> Self {
        // 次元0は無意味なので最低1に丸める（健全性のため）。
        let dim = if dim == 0 { 1 } else { dim };
        HashNgramEmbedder { dim }
    }
}

/// FNV-1a（64bit）ハッシュ。安定・高速・依存ゼロ。
/// 同一文字列は常に同一ハッシュ → 埋め込みの決定論性を担保する。
fn fnv1a_64(bytes: &[u8]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

impl Embedder for HashNgramEmbedder {
    fn embed(&self, text: &str) -> Vec<f32> {
        let mut vec = vec![0.0f32; self.dim];

        // 小文字化して Unicode スカラ単位で扱う（CJK もそのまま1文字として）。
        let lowered = text.to_lowercase();
        let chars: Vec<char> = lowered.chars().collect();

        // 1つの n-gram をベクトルへ加算するヘルパ。
        // ハッシュの最下位ビットを符号として使い、+1.0 / -1.0 を蓄積する
        // （キャンセル可能にして衝突由来の偏りを緩和する）。
        let dim = self.dim as u64;
        let accumulate = |gram: &str, vec: &mut [f32]| {
            let h = fnv1a_64(gram.as_bytes());
            let idx = (h % dim) as usize;
            let sign = if (h >> 1) & 1 == 0 { 1.0 } else { -1.0 };
            if let Some(slot) = vec.get_mut(idx) {
                *slot += sign;
            }
        };

        // unigram（短文・CJK 混在を頑健に扱うため）。
        let mut buf = String::new();
        for &c in &chars {
            buf.clear();
            buf.push(c);
            accumulate(&buf, &mut vec);
        }

        // 文字 3-gram。
        const N: usize = 3;
        if chars.len() >= N {
            for window in chars.windows(N) {
                buf.clear();
                for &c in window {
                    buf.push(c);
                }
                accumulate(&buf, &mut vec);
            }
        }

        // L2 正規化（ゼロベクトルはそのまま＝NaN を避ける）。
        let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut vec {
                *x /= norm;
            }
        }

        vec
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn model_version(&self) -> &'static str {
        MODEL_VERSION
    }
}

/// コサイン類似度。両ベクトルとも L2 正規化済みなら内積に一致するが、
/// 念のため防御的に norm で割る（保存側・クエリ側の双方で一貫した結果を保証）。
#[must_use]
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na <= 0.0 || nb <= 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embed_is_l2_normalized_and_deterministic() {
        let e = HashNgramEmbedder::new(64);
        let v1 = e.embed("好きな食べ物はカレーです");
        let v2 = e.embed("好きな食べ物はカレーです");
        assert_eq!(v1, v2, "同一入力は決定論的に同一ベクトル");
        let norm: f32 = v1.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4, "L2 正規化されている norm={norm}");
    }

    #[test]
    fn cosine_of_identical_is_one_and_disjoint_is_low() {
        let e = HashNgramEmbedder::new(256);
        let a = e.embed("毎朝コーヒーを飲む習慣があります");
        let b = e.embed("毎朝コーヒーを飲む習慣があります");
        let c = e.embed("量子力学の観測問題について");
        assert!((cosine(&a, &b) - 1.0).abs() < 1e-4, "同文はコサイン≈1");
        assert!(cosine(&a, &c) < cosine(&a, &b), "無関係な文の方がコサインは低い");
    }

    #[test]
    fn dim_zero_is_clamped_to_one() {
        let e = HashNgramEmbedder::new(0);
        assert_eq!(e.dim(), 1);
        assert_eq!(e.embed("x").len(), 1);
    }
}
