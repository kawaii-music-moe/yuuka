//! シナプス抽出（write 経路）— Node `src/services/synapseExtractor.ts` `maybeExtractSynapse` パリティ。
//!
//! **完全ヒューリスティック・LLM コストゼロ**。会話ターン確定後にユーザー発話から「記憶に値する断片」を
//! 選別し、`synapses` へ挿入 + [`yuuka_synapse`] で埋め込み + embedding 列を更新する（fire-and-forget）。
//!
//! 秘匿値ガード（Node `src/utils/secretGuard.ts` `containsSecretValue`）は Rust に既存実装が無いため
//! ここへ移植する（キーワード `SECRET_GUARD_RE` + 値の形状 `contains_secret_value` の二層防御・§9.3）。

use std::sync::Arc;

use tokio::sync::Mutex;
use yuuka_synapse::{FormationContext, Scope, SynapseEngine};

use crate::synapse_repo::{self, InsertArgs};
use yuuka_web::Db;

// ─── Node 定数（synapseExtractor.ts）を逐語移植 ──────────────────────────────────

/// シナプス content の最大長（トークン肥大を避ける・Node `MAX_CONTENT_LEN`）。
const MAX_CONTENT_LEN: usize = 300;

/// 抽出対象から除外する最小文字数（trim 後・Node `MIN_USER_TEXT_LEN`）。**文字数**で数える
/// （Node `string.length` は UTF-16 code unit 長だが、CJK/BMP 中心の入力では char 数と一致し、
/// 短文の足切りという用途では十分に等価）。
const MIN_USER_TEXT_LEN: usize = 8;

/// マーカー語を含まない発話を「長さだけ」で記憶する下限（trim 後・Node `MEMORABLE_MIN_LEN`）。
const MEMORABLE_MIN_LEN: usize = 80;

/// 「記憶に値する」意味マーカー（Node `MEMORABLE_RE`・逐語）。嗜好・習慣・事実・制約・記憶依頼。
const MEMORABLE_MARKERS: &[&str] = &[
    "好き",
    "嫌い",
    "苦手",
    "お気に入り",
    "推し",
    "いつも",
    "毎日",
    "毎週",
    "毎朝",
    "毎晩",
    "習慣",
    "誕生日",
    "記念日",
    "アレルギー",
    "出身",
    "在住",
    "住ん",
    "勤め",
    "所属",
    "締め切り",
    "期限",
    "目標",
    "設定",
    "覚え",
    "記憶",
    "忘れない",
];

/// 秘匿ガードのキーワード（Node `SECRET_GUARD_RE`・逐語・大文字小文字無視）。
const SECRET_GUARD_KEYWORDS: &[&str] = &[
    "password",
    "passwd",
    "パスワード",
    "secret",
    "シークレット",
    "token",
    "api_key",
    "api-key",
    "apikey",
    "credential",
    "暗証",
    "ワンタイム",
    "otp",
];

/// Node `SECRET_GUARD_RE.test()` パリティ: キーワードのいずれかを（大文字小文字無視で）含むか。
/// `api[_-]?key` は `apikey`/`api_key`/`api-key` の 3 形を候補列挙してカバーする。
fn secret_guard_hit(text: &str) -> bool {
    let lower = text.to_lowercase();
    SECRET_GUARD_KEYWORDS.iter().any(|kw| lower.contains(kw))
}

/// Node `MEMORABLE_RE.test()` パリティ: 意味マーカーのいずれかを含むか。
fn is_memorable_marker(text: &str) -> bool {
    MEMORABLE_MARKERS.iter().any(|m| text.contains(m))
}

/// 純粋なコマンドっぽい入力（先頭が記号トリガ）を雑に判定（Node `looksLikeCommand`）。
/// 先頭 1 文字が `! / . \ $ # ＠ @` のいずれか。
fn looks_like_command(text: &str) -> bool {
    matches!(
        text.trim().chars().next(),
        Some('!' | '/' | '.' | '\\' | '$' | '#' | '＠' | '@')
    )
}

/// content の正規化（前後空白除去 + 連続空白を 1 つへ + 長さ上限・Node `capContent`）。
fn cap_content(text: &str) -> String {
    // Node: text.trim().replace(/\s+/g, " ")。連続する空白類（半角/改行/タブ）を単一半角空白へ。
    let mut normalized = String::with_capacity(text.len());
    let mut prev_ws = false;
    for c in text.trim().chars() {
        if c.is_whitespace() {
            if !prev_ws {
                normalized.push(' ');
            }
            prev_ws = true;
        } else {
            normalized.push(c);
            prev_ws = false;
        }
    }
    // 長さ上限（Node は UTF-16 slice だが char 数で切る＝境界安全・実務上等価）。
    if normalized.chars().count() > MAX_CONTENT_LEN {
        normalized.chars().take(MAX_CONTENT_LEN).collect()
    } else {
        normalized
    }
}

/// ヒューリスティックなトピック語抽出（粗くてよい / None 許容・Node `deriveTopicId`）。
/// 区切り文字で分割し、2 文字以上かつ「全部ひらがな」でない最長トークンを 32 文字にキャップして返す。
fn derive_topic_id(text: &str) -> Option<String> {
    // Node の分割文字クラス: 空白・、。．,.!?！？「」『』()（）[]【】
    let is_sep = |c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '、' | '。'
                    | '．'
                    | ','
                    | '.'
                    | '!'
                    | '?'
                    | '！'
                    | '？'
                    | '「'
                    | '」'
                    | '『'
                    | '』'
                    | '('
                    | ')'
                    | '（'
                    | '）'
                    | '['
                    | ']'
                    | '【'
                    | '】'
            )
    };
    let mut longest: Option<&str> = None;
    for tok in text.split(is_sep) {
        let tok = tok.trim();
        let len = tok.chars().count();
        if len < 2 {
            continue;
        }
        // 全部ひらがな（U+3041..=U+3093 相当）のトークンは除外（Node `/^[ぁ-ん]+$/`）。
        if tok.chars().all(|c| ('ぁ'..='ん').contains(&c)) {
            continue;
        }
        if longest.is_none_or(|l| len > l.chars().count()) {
            longest = Some(tok);
        }
    }
    let longest = longest?;
    let topic: String = longest.chars().take(32).collect();
    if topic.chars().count() >= 2 {
        Some(topic)
    } else {
        None
    }
}

// ─── 秘匿値ガード（Node `src/utils/secretGuard.ts` `containsSecretValue`）逐語移植 ────

/// 既知の秘匿トークン接頭辞（部分一致・Node `SECRET_PREFIXES`）。
const SECRET_PREFIXES: &[&str] = &[
    "sk-",
    "rk_",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xoxr-",
    "xoxs-",
    "AKIA",
    "ASIA",
    "AIza",
    "ya29.",
    "-----BEGIN",
    "AccountKey=",
];

/// 文字あたりのシャノンエントロピー（bit・Node `shannonEntropyBits`）。
fn shannon_entropy_bits(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let mut freq = std::collections::HashMap::new();
    let mut total = 0usize;
    for ch in s.chars() {
        *freq.entry(ch).or_insert(0usize) += 1;
        total += 1;
    }
    let total = total as f64;
    let mut bits = 0.0f64;
    for &count in freq.values() {
        let p = count as f64 / total;
        bits -= p * p.log2();
    }
    bits
}

/// JWT（`eyJ...` の base64url 3 セグメント・Node `JWT_RE`）を含むか。手書きスキャンで再現する。
fn contains_jwt(t: &str) -> bool {
    let bytes = t.as_bytes();
    // base64url 文字集合 [A-Za-z0-9_-]。
    let is_b64u = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    // `from` からの base64url 連長（indexing/slicing を避け iterator で数える）。
    let run = |from: usize| bytes.iter().skip(from).take_while(|&&b| is_b64u(b)).count();
    for i in 0..bytes.len() {
        // 先頭 "eyJ"。
        if !bytes.get(i..).is_some_and(|s| s.starts_with(b"eyJ")) {
            continue;
        }
        // "eyJ" + 続き。Node: eyJ[..]{8,} → 本体 8 文字以上（＝seg1 全長 11 以上）。
        let seg1 = 3 + run(i + 3);
        let j = i + seg1;
        if seg1 < 11 || bytes.get(j) != Some(&b'.') {
            continue;
        }
        let len2 = run(j + 1);
        let k = j + 1 + len2;
        if len2 < 6 || bytes.get(k) != Some(&b'.') {
            continue;
        }
        if run(k + 1) >= 4 {
            return true;
        }
    }
    false
}

/// 1 トークンが秘匿値らしい形状か（Node `looksLikeSecretValue`）。接頭辞 / JWT / 高エントロピー長塊。
fn looks_like_secret_value(token: &str) -> bool {
    let t = token.trim();
    if t.chars().count() < 8 {
        return false;
    }
    for prefix in SECRET_PREFIXES {
        if t.contains(prefix) {
            return true;
        }
    }
    if contains_jwt(t) {
        return true;
    }
    // 区切りを含まない長い塊（Node `TOKEN_RUN_RE = [A-Za-z0-9_\-+/=.]{12,}`）を走査。
    let is_run_char =
        |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '/' | '=' | '.');
    let mut run = String::new();
    let check = |run: &str| -> bool {
        if run.chars().count() < 12 {
            return false;
        }
        let has_lower = run.chars().any(|c| c.is_ascii_lowercase());
        let has_upper = run.chars().any(|c| c.is_ascii_uppercase());
        let has_digit = run.chars().any(|c| c.is_ascii_digit());
        let classes = u32::from(has_lower) + u32::from(has_upper) + u32::from(has_digit);
        let entropy = shannon_entropy_bits(run);
        let len = run.chars().count();
        // 24 文字以上・2 クラス以上・高エントロピー → 機械生成トークン。
        if len >= 24 && classes >= 2 && entropy >= 3.2 {
            return true;
        }
        // 40 文字以上の非常に長い塊は 1 クラスでも秘匿扱い（hex ダンプ等）。
        len >= 40 && entropy >= 3.0
    };
    for c in t.chars() {
        if is_run_char(c) {
            run.push(c);
        } else {
            if check(&run) {
                return true;
            }
            run.clear();
        }
    }
    check(&run)
}

/// テキスト中に秘匿値らしいトークンが 1 つでも含まれるか（Node `containsSecretValue`）。
/// Node の分割文字クラス `[\s"'\`,;<>(){}[\]]+` で素朴に分割して各トークンを判定する。
fn contains_secret_value(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    let is_split = |c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '"' | '\'' | '`' | ',' | ';' | '<' | '>' | '(' | ')' | '{' | '}' | '[' | ']'
            )
    };
    text.split(is_split).any(looks_like_secret_value)
}

// ─── 抽出本体（Node `maybeExtractSynapse`）─────────────────────────────────────

/// 秘書経路のスコープ引数（guild_id は None＝DM/秘書・非 None＝汎用モードのギルド）。
#[derive(Clone)]
pub struct ExtractScope {
    pub user_id: String,
    pub bot_id: String,
    pub guild_id: Option<String>,
}

/// 会話ターンからシナプスを抽出し SQLite + RAM 索引へ永続化する（Node `maybeExtractSynapse`）。
///
/// 抽出はヒューリスティック（LLM コストゼロ）。呼び出し側は **fire-and-forget**（await せず・失敗を
/// 投げない）で呼ぶ。`user_text` が非 memorable / 秘匿 / 短すぎ / コマンドなら何もしない。
///
/// 失敗（DB/索引）は握って `warn` ログのみ（Node の catch と同じく呼び出し側へ伝播しない）。
pub async fn maybe_extract_synapse(
    db: &Db,
    engine: &Arc<Mutex<SynapseEngine>>,
    scope: ExtractScope,
    user_text: &str,
    source_msg_id: Option<i64>,
) {
    if let Err(e) = extract_inner(db, engine, scope, user_text, source_msg_id).await {
        tracing::warn!(error = %e, "[Synapse] シナプス抽出に失敗しました（無視）");
    }
}

/// [`maybe_extract_synapse`] の本体（Result を返し、呼び出し側が warn へ落とす）。
async fn extract_inner(
    db: &Db,
    engine: &Arc<Mutex<SynapseEngine>>,
    scope: ExtractScope,
    user_text: &str,
    source_msg_id: Option<i64>,
) -> Result<(), yuuka_core::DbError> {
    let trimmed = user_text.trim();

    // 秘匿除外不変条件（§9.3）: (1) キーワード / (2) 値形状。いずれかで記憶しない。
    if secret_guard_hit(user_text) || contains_secret_value(user_text) {
        return Ok(());
    }
    // 短すぎ / コマンドっぽい入力はスキップ。
    if trimmed.chars().count() < MIN_USER_TEXT_LEN || looks_like_command(trimmed) {
        return Ok(());
    }

    // memorable 判定: 意味マーカーを含むか、十分に長い実質発話のみ。
    let is_memorable = is_memorable_marker(trimmed) || trimmed.chars().count() >= MEMORABLE_MIN_LEN;
    if !is_memorable {
        return Ok(());
    }

    let content = cap_content(user_text);
    if content.is_empty() {
        return Ok(());
    }
    // content 側も両ガードを通す（念のため）。
    if secret_guard_hit(&content) || contains_secret_value(&content) {
        return Ok(());
    }
    let topic_id = derive_topic_id(trimmed);

    // 形成時の時刻文脈（再ランキング専用・現地時刻）。意味埋め込みには混ぜない。
    let (ctx_tod, ctx_dow, now_epoch) = local_time_context();

    // 永続化（Rust が唯一の書き手＝単一 writer actor）。
    let id = synapse_repo::insert_synapse(
        db,
        InsertArgs {
            user_id: scope.user_id.clone(),
            bot_id: scope.bot_id.clone(),
            guild_id: scope.guild_id.clone(),
            content: content.clone(),
            topic_id: topic_id.clone(),
            source_msg_id,
            ctx_tod: Some(ctx_tod),
            ctx_dow: Some(ctx_dow),
        },
    )
    .await?;

    // RAM 索引へ登録し、埋め込みバイト列を受け取って永続化する（in-process・エンジンは Mutex 越し）。
    let indexed = {
        let mut eng = engine.lock().await;
        eng.index(
            Scope {
                user_id: scope.user_id,
                bot_id: scope.bot_id,
                guild_id: scope.guild_id,
            },
            id,
            topic_id,
            content,
            FormationContext {
                ctx_tod: Some(ctx_tod),
                ctx_dow: Some(ctx_dow),
                created_at: Some(now_epoch),
            },
        )
    };
    synapse_repo::update_synapse_embedding(db, id, indexed.embedding, indexed.model_version)
        .await?;
    Ok(())
}

/// 現地時刻の (時間帯 0-23, 曜日 0=日〜6=土, Unix エポック秒)。Node `new Date()` の
/// `getHours()`/`getDay()`/`getTime()/1000` に対応する。
fn local_time_context() -> (i64, i64, i64) {
    use chrono::{Datelike, Local, Timelike};
    let now = Local::now();
    let tod = i64::from(now.hour());
    let dow = i64::from(now.weekday().num_days_from_sunday());
    let epoch = now.timestamp();
    (tod, dow, epoch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memorable_marker_input_is_memorable() {
        // 意味マーカー「好き」を含む短文は memorable。
        assert!(is_memorable_marker("好きな食べ物はカレーです"));
        assert!(!is_memorable_marker("今日はいい天気ですね"));
    }

    #[test]
    fn long_input_is_memorable_by_length() {
        let long = "あ".repeat(MEMORABLE_MIN_LEN);
        assert!(long.chars().count() >= MEMORABLE_MIN_LEN);
        // マーカー無しでも長さ >= 80 で memorable（extract_inner の分岐と同条件）。
        assert!(!is_memorable_marker(&long));
    }

    #[test]
    fn secret_keyword_and_value_are_guarded() {
        // (1) キーワード。
        assert!(secret_guard_hit("私のpasswordは覚えておいて"));
        assert!(secret_guard_hit("APIキーの token を保存"));
        assert!(secret_guard_hit("api_key を教える"));
        assert!(!secret_guard_hit("好きな食べ物はカレーです"));
        // (2) 値の形状（JWT・高エントロピー塊・既知接頭辞）。
        assert!(contains_secret_value(
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcd1234"
        ));
        assert!(contains_secret_value("sk-abcdef0123456789ABCDEF")); // 既知接頭辞 sk-
        assert!(!contains_secret_value("好きな食べ物はカレーです")); // 通常文は非秘匿
    }

    #[test]
    fn command_like_is_skipped() {
        assert!(looks_like_command("!help"));
        assert!(looks_like_command("/todo add x"));
        assert!(looks_like_command("＠mention"));
        assert!(!looks_like_command("好きな食べ物はカレーです"));
    }

    #[test]
    fn derive_topic_picks_longest_non_hiragana_token() {
        // 区切り（、）で分割し最長の非ひらがなトークンを選ぶ（Node deriveTopicId は字面分割のみ・
        // スクリプト境界では切らない）。パスタ(3)/カレーライス(6)は非ひらがな、うどんは全ひらがなで除外。
        assert_eq!(
            derive_topic_id("パスタ、カレーライス、うどん"),
            Some("カレーライス".to_owned())
        );
        // 全部ひらがな・短トークンのみなら None。
        assert_eq!(derive_topic_id("あ い う"), None);
    }

    #[test]
    fn cap_content_collapses_whitespace_and_caps_length() {
        assert_eq!(cap_content("  好き   な  食べ物 "), "好き な 食べ物");
        let long = "あ".repeat(MAX_CONTENT_LEN + 50);
        assert_eq!(cap_content(&long).chars().count(), MAX_CONTENT_LEN);
    }

    #[test]
    fn short_input_below_min_len_skipped() {
        // MIN_USER_TEXT_LEN=8 未満は足切り（trimmed.chars().count()）。
        assert!("好き".chars().count() < MIN_USER_TEXT_LEN);
    }
}
