//! テキストユーティリティ（現行 `src/utils/discordMarkdown.ts` / `src/bot.ts` の純粋ロジック移植）。
//!
//! - [`to_discord_markdown`] — Discord 非対応 Markdown の機械的変換（1:1 移植）。
//! - [`split_message`]       — 2000 字・改行境界での分割（現行 `splitMessage`）。
//! - [`strip_self_mention`] / [`strip_all_mentions`] — メンション除去（秘書/汎用で挙動が異なる）。
//! - [`is_supported_audio`] — 音声添付判定（現行 `SUPPORTED_AUDIO_TYPES`）。
//!
//! いずれもネットワーク非依存の純関数でユニットテストする。

use std::sync::OnceLock;

use regex::{Captures, Regex};

/// Discord 1 メッセージの最大文字数（現行 `2000`）。
pub const DISCORD_MAX_MESSAGE_LEN: usize = 2000;

/// 対応音声フォーマット（現行 `SUPPORTED_AUDIO_TYPES` [`src/bot.ts:61-72`]）。
pub const SUPPORTED_AUDIO_TYPES: &[&str] = &[
    "audio/ogg",
    "audio/mpeg",
    "audio/mp3",
    "audio/wav",
    "audio/x-wav",
    "audio/mp4",
    "audio/x-m4a",
    "audio/m4a",
    "audio/aac",
    "audio/flac",
];

/// メンバー外のメンションへ返す利用案内（現行 `sendNonMemberGuidance` [`src/bot.ts:636-639`]）。
pub const NON_MEMBER_GUIDANCE: &str =
    "👋 このBotは利用メンバー制です。下のボタンから利用申請するとBot作成者へ承認依頼が届きます。";

/// 添付 content-type が音声か（現行 `SUPPORTED_AUDIO_TYPES.includes(ct) || ct.startsWith("audio/")`）。
///
/// `;` 以降を落として小文字化してから判定する（現行と一致）。
#[must_use]
pub fn is_supported_audio(content_type: &str) -> bool {
    let ct = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    ct.starts_with("audio/") || SUPPORTED_AUDIO_TYPES.contains(&ct.as_str())
}

/// 自 Bot 宛メンション（`<@id>` / `<@!id>`）のみ除去する（現行の汎用モード [`src/bot.ts:751-753`]）。
///
/// 他ユーザーへのメンションはメンバー追加依頼の対象解決に必要なため残す（要件 §4.3.3）。
#[must_use]
pub fn strip_self_mention(content: &str, bot_user_id: &str) -> String {
    // bot_user_id は Discord snowflake（数字）想定だが、念のため正規表現メタ文字を無害化する。
    let escaped = regex::escape(bot_user_id);
    // 実行時構築（escape 済みなので失敗し得ないが）失敗時は never-match へ縮退。
    let re = Regex::new(&format!("<@!?{escaped}>")).unwrap_or_else(|_| never_match().clone());
    re.replace_all(content, "").into_owned()
}

/// 全メンション（`<@id>` / `<@!id>`）を除去する（現行の秘書経路 [`src/bot.ts:1048`]）。
#[must_use]
pub fn strip_all_mentions(content: &str) -> String {
    mention_regex().replace_all(content, "").into_owned()
}

fn mention_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile(r"<@!?\d+>"))
}

/// 決してマッチしない正規表現（実行時 regex 構築失敗時の安全フォールバック）。`\z\A` は
/// 「末尾の直後→先頭」で決して真にならないが構文的には妥当。
fn never_match() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile(r"\z\A"))
}

/// コンパイル時定数パターンをコンパイルする。全パターンは本モジュールのユニットテストで実行され、
/// 無効ならテストが失敗するため実行時にはエラー経路へ到達しない。restriction lint の緩和は
/// この 1 箇所（定数パターン・テスト済み）に限定する。
#[allow(clippy::unwrap_used)]
fn compile(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap()
}

/// 長いメッセージを `max_len` 文字で分割する（現行 `splitMessage` [`src/bot.ts:1268-1288`]）。
///
/// 改行境界を優先し、`max_len` の半分より前にしか改行が無ければ `max_len` で機械分割する。
/// JS の UTF-16 長さに対し Rust は char 単位で扱う（マルチバイト境界での panic を避けつつ意味は同一）。
#[must_use]
pub fn split_message(text: &str, max_len: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut chunks: Vec<String> = Vec::new();
    if max_len == 0 {
        return chunks;
    }
    let mut start = 0usize;
    while start < chars.len() {
        let remaining = chars.len() - start;
        if remaining <= max_len {
            chunks.push(chars.get(start..).unwrap_or(&[]).iter().collect());
            break;
        }
        // remaining 内の index [0, max_len] で最後の '\n' を探す（JS lastIndexOf("\n", maxLength) 相当）。
        let mut split_point = max_len;
        for i in (0..=max_len).rev() {
            if chars.get(start + i) == Some(&'\n') {
                // maxLength/2 より前の改行は使わない（機械分割にフォールバック）。
                if i >= max_len / 2 {
                    split_point = i;
                }
                break;
            }
        }
        chunks.push(
            chars
                .get(start..start + split_point)
                .unwrap_or(&[])
                .iter()
                .collect(),
        );
        start += split_point;
        // remaining.substring(splitPoint).trimStart() 相当（境界の空白・改行を落とす）。
        while chars.get(start).is_some_and(|c| c.is_whitespace()) {
            start += 1;
        }
    }
    chunks
}

// ─── toDiscordMarkdown（Discord 非対応 Markdown の機械的変換） ─────────────────

/// 非対応 Markdown を Discord 互換の代替表現へ変換する（現行 `toDiscordMarkdown` 1:1 移植）。
///
/// フェンスドコードブロック（```` ``` ````）とインラインコード（`` ` ``）の内部は保護し、
/// 変換対象から除外する。**分割（[`split_message`]）前の全文**に適用すること（コードフェンスが
/// チャンク境界で割れると保護判定が壊れるため）。
#[must_use]
pub fn to_discord_markdown(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }
    // フェンスドコードブロックを保護しつつ、それ以外の区間のみ変換。
    split_keeping_matches(input, fenced_code_regex())
        .into_iter()
        .map(|seg| {
            if seg.is_match {
                seg.text.to_owned()
            } else {
                transform_plain(seg.text)
            }
        })
        .collect()
}

struct Segment<'a> {
    text: &'a str,
    is_match: bool,
}

/// 正規表現マッチ区間を「保護対象」として非マッチ区間と区別して返す（現行 `splitKeepingDelimiter`）。
fn split_keeping_matches<'a>(input: &'a str, re: &Regex) -> Vec<Segment<'a>> {
    let mut segments = Vec::new();
    let mut last = 0usize;
    for m in re.find_iter(input) {
        if m.start() > last {
            if let Some(text) = input.get(last..m.start()) {
                segments.push(Segment { text, is_match: false });
            }
        }
        segments.push(Segment { text: m.as_str(), is_match: true });
        last = m.end();
    }
    if last < input.len() {
        if let Some(text) = input.get(last..) {
            segments.push(Segment { text, is_match: false });
        }
    }
    segments
}

/// コードブロック外区間の変換（現行 `transformPlain`）。
fn transform_plain(text: &str) -> String {
    // 行頭パターン（タスクリスト・脚注定義）を行ごとに変換。
    let mut work = text
        .split('\n')
        .map(|line| {
            let l = convert_task_list_line(line);
            convert_footnote_def_line(&l)
        })
        .collect::<Vec<_>>()
        .join("\n");

    // テーブルは複数行ブロック（まだコードフェンス化されていない段階で変換）。
    work = convert_tables(&work);

    // インライン系はインラインコードを保護して適用。
    apply_inline(&work)
}

/// インラインコード（`` ` ``）を保護しつつインライン変換を適用（現行 `applyInline`）。
fn apply_inline(text: &str) -> String {
    split_keeping_matches(text, inline_code_regex())
        .into_iter()
        .map(|seg| {
            if seg.is_match {
                seg.text.to_owned()
            } else {
                let s = convert_math(seg.text);
                let s = convert_highlight(&s);
                let s = convert_inline_html(&s);
                convert_footnote_refs(&s)
            }
        })
        .collect()
}

// §1 タスクリスト（現行 `convertTaskListLine`）。
fn convert_task_list_line(line: &str) -> String {
    task_list_regex()
        .replace(line, |caps: &Captures| {
            let indent = caps.get(1).map_or("", |m| m.as_str());
            let mark = caps.get(2).map_or("", |m| m.as_str());
            let boxch = match mark {
                " " => "⬜",
                "-" | "/" => "🔄", // 進行中（拡張記法）
                _ => "✅",
            };
            format!("{indent}{boxch} ")
        })
        .into_owned()
}

// §4 脚注定義行（現行 `convertFootnoteDefLine`）。`[^id]: text` → `[※id] text`。
fn convert_footnote_def_line(line: &str) -> String {
    footnote_def_regex()
        .replace(line, |caps: &Captures| {
            let indent = caps.get(1).map_or("", |m| m.as_str());
            let id = caps.get(2).map_or("", |m| m.as_str());
            format!("{indent}[※{id}] ")
        })
        .into_owned()
}

// §4 脚注参照（現行 `convertFootnoteRefs`）。`[^id]` → `[※id]`。
fn convert_footnote_refs(text: &str) -> String {
    footnote_ref_regex()
        .replace_all(text, |caps: &Captures| {
            format!("[※{}]", caps.get(1).map_or("", |m| m.as_str()))
        })
        .into_owned()
}

// §2 ハイライト（現行 `convertHighlight`）。`==text==` → `**text**`。
fn convert_highlight(text: &str) -> String {
    highlight_regex()
        .replace_all(text, |caps: &Captures| {
            format!("**{}**", caps.get(1).map_or("", |m| m.as_str()))
        })
        .into_owned()
}

// §2 簡易インライン HTML（現行 `convertInlineHtml`）。
// Rust regex は後方参照非対応のため、開閉タグをタグ別に明示（機能は同一）。
fn convert_inline_html(text: &str) -> String {
    let mut s = text.to_owned();
    for (re, wrap) in inline_html_rules() {
        s = re
            .replace_all(&s, |caps: &Captures| {
                let inner = caps.get(1).map_or("", |m| m.as_str());
                format!("{wrap}{inner}{wrap}")
            })
            .into_owned();
    }
    s
}

// §3 数式（現行 `convertMath`）。
fn convert_math(text: &str) -> String {
    // ブロック数式 $$...$$ → コードブロック（先に処理）。
    let out = math_block_regex()
        .replace_all(text, |caps: &Captures| {
            let body = caps.get(1).map_or("", |m| m.as_str()).trim();
            format!("\n```\n{body}\n```\n")
        })
        .into_owned();
    // インライン数式 $...$ → インラインコード（数式記号を含むものだけ。通貨 `$100` は対象外）。
    math_inline_regex()
        .replace_all(&out, |caps: &Captures| {
            let whole = caps.get(0).map_or("", |m| m.as_str());
            let body = caps.get(1).map_or("", |m| m.as_str());
            if math_hint_regex().is_match(body) {
                format!("`{}`", body.trim())
            } else {
                whole.to_owned()
            }
        })
        .into_owned()
}

// §6 テーブル（現行 `convertTables`）。
fn convert_tables(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut result: Vec<String> = Vec::new();
    let mut i = 0usize;
    while let Some(&header) = lines.get(i) {
        let sep = lines.get(i + 1).copied();
        if header.contains('|') && is_table_separator(sep) {
            let header_cells = split_table_row(header);
            i += 2; // ヘッダ行と区切り行をスキップ。
            let mut rows: Vec<Vec<String>> = Vec::new();
            while let Some(&line) = lines.get(i) {
                if !line.contains('|') || line.trim().is_empty() {
                    break;
                }
                rows.push(split_table_row(line));
                i += 1;
            }
            result.push(render_ascii_table(&header_cells, &rows));
            continue;
        }
        result.push(header.to_owned());
        i += 1;
    }
    result.join("\n")
}

fn is_table_separator(line: Option<&str>) -> bool {
    let Some(line) = line else { return false };
    let cells = split_table_row(line);
    if cells.is_empty() || (cells.len() == 1 && cells.first().is_some_and(String::is_empty)) {
        return false;
    }
    cells.iter().all(|c| table_sep_cell_regex().is_match(c.trim()))
}

fn split_table_row(line: &str) -> Vec<String> {
    let mut s = line.trim();
    s = s.strip_prefix('|').unwrap_or(s);
    s = s.strip_suffix('|').unwrap_or(s);
    s.split('|').map(|c| c.trim().to_owned()).collect()
}

fn render_ascii_table(header: &[String], rows: &[Vec<String>]) -> String {
    let col_count = header
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    let mut widths = vec![1usize; col_count];
    for (c, w) in widths.iter_mut().enumerate() {
        let mut max_w = 1usize;
        if let Some(cell) = header.get(c) {
            max_w = max_w.max(display_width(cell));
        }
        for row in rows {
            if let Some(cell) = row.get(c) {
                max_w = max_w.max(display_width(cell));
            }
        }
        *w = max_w;
    }

    let render_row = |cells: &[String]| -> String {
        (0..col_count)
            .map(|c| {
                pad_cell(
                    cells.get(c).map_or("", |s| s.as_str()),
                    widths.get(c).copied().unwrap_or(1),
                )
            })
            .collect::<Vec<_>>()
            .join(" | ")
    };
    let separator = widths
        .iter()
        .map(|w| "-".repeat(*w))
        .collect::<Vec<_>>()
        .join("-|-");

    let mut body = vec![render_row(header), separator];
    for row in rows {
        body.push(render_row(row));
    }
    format!("```\n{}\n```", body.join("\n"))
}

/// 全角文字を 2 幅として簡易計算（等幅表示の桁ずれ軽減・現行 `displayWidth`/`FULLWIDTH`）。
fn display_width(s: &str) -> usize {
    s.chars().map(|c| if is_fullwidth(c) { 2 } else { 1 }).sum()
}

/// East Asian Wide 相当の範囲（現行 `FULLWIDTH` 正規表現のコードポイント移植）。
fn is_fullwidth(c: char) -> bool {
    let u = c as u32;
    matches!(u,
        0x1100..=0x115F   // Hangul Jamo
        | 0x2E80..=0x303E // CJK Radicals..CJK Symbols
        | 0x3041..=0x33FF // Hiragana..CJK Compatibility
        | 0x3400..=0x4DBF // CJK Ext A
        | 0x4E00..=0x9FFF // CJK Unified
        | 0xA000..=0xA4CF // Yi
        | 0xAC00..=0xD7A3 // Hangul Syllables
        | 0xF900..=0xFAFF // CJK Compatibility Ideographs
        | 0xFE30..=0xFE4F // CJK Compatibility Forms
        | 0xFF00..=0xFF60 // Fullwidth Forms
        | 0xFFE0..=0xFFE6 // Fullwidth Signs
    )
}

fn pad_cell(s: &str, width: usize) -> String {
    let pad = width.saturating_sub(display_width(s));
    if pad > 0 {
        format!("{s}{}", " ".repeat(pad))
    } else {
        s.to_owned()
    }
}

// ─── コンパイル済み正規表現（OnceLock でプロセス内 1 回だけコンパイル） ─────────

macro_rules! lazy_regex {
    ($name:ident, $pat:expr) => {
        fn $name() -> &'static Regex {
            static RE: OnceLock<Regex> = OnceLock::new();
            RE.get_or_init(|| compile($pat))
        }
    };
}

lazy_regex!(fenced_code_regex, r"(?s)```.*?```");
lazy_regex!(inline_code_regex, r"`[^`\n]+`");
lazy_regex!(task_list_regex, r"^(\s*)[-*+]\s+\[([ xX/-])\]\s+");
lazy_regex!(footnote_def_regex, r"^(\s*)\[\^([^\]]+)\]:\s*");
lazy_regex!(footnote_ref_regex, r"\[\^([^\]]+)\]");
lazy_regex!(highlight_regex, r"==([^=\n]+)==");
lazy_regex!(math_block_regex, r"(?s)\$\$(.+?)\$\$");
lazy_regex!(math_inline_regex, r"\$([^$\n]+?)\$");
lazy_regex!(
    math_hint_regex,
    r"[\\^_{}=∑∫√≈≠≤≥×÷±∞αβγδθλμπσφω]|\\[a-zA-Z]+"
);
lazy_regex!(table_sep_cell_regex, r"^:?-{1,}:?$");

/// 簡易インライン HTML の変換規則（タグ別・後方参照を使わない）。現行 `convertInlineHtml` と等価。
fn inline_html_rules() -> &'static [(Regex, &'static str)] {
    static RULES: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    RULES.get_or_init(|| {
        let build = compile;
        vec![
            (build(r"(?is)<\s*b\s*>(.*?)<\s*/\s*b\s*>"), "**"),
            (build(r"(?is)<\s*strong\s*>(.*?)<\s*/\s*strong\s*>"), "**"),
            (build(r"(?is)<\s*i\s*>(.*?)<\s*/\s*i\s*>"), "*"),
            (build(r"(?is)<\s*em\s*>(.*?)<\s*/\s*em\s*>"), "*"),
            (build(r"(?is)<\s*u\s*>(.*?)<\s*/\s*u\s*>"), "__"),
            (build(r"(?is)<\s*mark[^>]*>(.*?)<\s*/\s*mark\s*>"), "**"),
            (build(r"(?is)<\s*span[^>]*>(.*?)<\s*/\s*span\s*>"), "**"),
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_detection_matches_supported_and_prefix() {
        assert!(is_supported_audio("audio/ogg"));
        assert!(is_supported_audio("audio/ogg; codecs=opus"));
        assert!(is_supported_audio("AUDIO/FLAC"));
        assert!(is_supported_audio("audio/anything")); // startsWith audio/
        assert!(!is_supported_audio("image/png"));
        assert!(!is_supported_audio(""));
    }

    #[test]
    fn strip_self_mention_only_removes_bot() {
        let s = strip_self_mention("<@123> hi <@!123> <@999>", "123");
        assert_eq!(s.trim(), "hi  <@999>");
    }

    #[test]
    fn strip_all_mentions_removes_every_mention() {
        assert_eq!(strip_all_mentions("<@1> a <@!2> b").trim(), "a  b");
    }

    #[test]
    fn split_message_prefers_newline_boundary() {
        let text = format!("{}\n{}", "a".repeat(1200), "b".repeat(1500));
        let chunks = split_message(&text, 2000);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], "a".repeat(1200)); // 改行境界で分割（末尾 \n は次チャンクへ→trim）。
        assert_eq!(chunks[1], "b".repeat(1500));
    }

    #[test]
    fn split_message_hard_splits_without_newline() {
        let text = "x".repeat(4500);
        let chunks = split_message(&text, 2000);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].chars().count(), 2000);
        assert_eq!(chunks[2].chars().count(), 500);
    }

    #[test]
    fn split_message_short_text_single_chunk() {
        assert_eq!(split_message("hello", 2000), vec!["hello".to_owned()]);
    }

    #[test]
    fn markdown_task_list_and_highlight() {
        assert_eq!(to_discord_markdown("- [ ] todo"), "⬜ todo");
        assert_eq!(to_discord_markdown("- [x] done"), "✅ done");
        assert_eq!(to_discord_markdown("- [/] wip"), "🔄 wip");
        assert_eq!(to_discord_markdown("==hi=="), "**hi**");
    }

    #[test]
    fn markdown_protects_code_blocks() {
        let input = "```\n- [ ] not converted\n```\n- [ ] converted";
        let out = to_discord_markdown(input);
        assert!(out.contains("- [ ] not converted"), "コード内は変換しない: {out}");
        assert!(out.contains("⬜ converted"), "コード外は変換: {out}");
    }

    #[test]
    fn markdown_inline_html_and_footnotes() {
        assert_eq!(to_discord_markdown("<b>bold</b>"), "**bold**");
        assert_eq!(to_discord_markdown("<i>it</i>"), "*it*");
        assert_eq!(to_discord_markdown("ref[^1]"), "ref[※1]");
        assert_eq!(to_discord_markdown("[^1]: note"), "[※1] note");
    }

    #[test]
    fn markdown_math_currency_not_converted() {
        // 通貨は数式記号を含まないため変換されない。
        assert_eq!(to_discord_markdown("$100 と $200"), "$100 と $200");
        // 数式記号を含むインラインは変換。
        assert_eq!(to_discord_markdown("$a = b$"), "`a = b`");
    }

    #[test]
    fn markdown_empty_input() {
        assert_eq!(to_discord_markdown(""), "");
    }
}
