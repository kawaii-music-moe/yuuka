//! モデルが `showRichContent` を呼ばず、Embed を ```json コードブロックとして本文に埋めてしまう
//! 事象（Discord 上で色付きカードではなく生の JSON コードブロックが表示される）を救済する。
//!
//! 最終テキストから **Embed 形の** フェンス JSON ブロックだけを抽出して実 [`RichEmbed`] に変換し、
//! 該当ブロックを本文から取り除く。Embed 形でない JSON（ユーザーが提示を頼んだデータ等）は温存する。

use yuuka_discord::{EmbedField, RichEmbed};

/// 本文から Embed 形の JSON コードブロックを取り除き、抽出した [`RichEmbed`] 群を返す。
///
/// 戻り値 `.0` は掃除後テキスト、`.1` は復元した Embed（本文出現順）。Embed 形ブロックが無ければ
/// 入力テキストをそのまま返し、Embed は空。
#[must_use]
pub fn recover_embeds_from_text(text: &str) -> (String, Vec<RichEmbed>) {
    let mut embeds = Vec::new();
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some((before, lang, body, after)) = next_fenced_block(rest) {
        // ```json（言語指定 json / なし）ブロックのみ対象。他言語はそのまま残す。
        let is_jsonish = lang.is_empty() || lang.eq_ignore_ascii_case("json");
        let parsed = if is_jsonish {
            serde_json::from_str::<serde_json::Value>(body.trim())
                .ok()
                .and_then(|v| embeds_from_value(&v))
        } else {
            None
        };
        match parsed {
            Some(found) if !found.is_empty() => {
                // Embed 形なので本文からは除去（before の末尾空白を残しすぎないよう trim_end）。
                out.push_str(before.trim_end_matches([' ', '\t']));
                embeds.extend(found);
            }
            _ => {
                // Embed 形でない → ブロックを原文のまま温存。
                out.push_str(before);
                out.push_str("```");
                out.push_str(lang);
                out.push_str(body);
                out.push_str("```");
            }
        }
        rest = after;
    }
    out.push_str(rest);

    if embeds.is_empty() {
        // 変換対象が無ければ元テキストを保つ（掃除で空白が変わるのを避ける）。
        (text.to_owned(), embeds)
    } else {
        (collapse_blank_lines(out.trim()), embeds)
    }
}

/// 先頭のフェンスドコードブロックを 1 個切り出す。戻り値は (ブロック前, 言語トークン, 本文, ブロック後)。
/// 本文は前後の改行を含む生文字列。閉じ ``` が無ければ `None`。
fn next_fenced_block(text: &str) -> Option<(&str, &str, &str, &str)> {
    let open = text.find("```")?;
    let after_open = &text[open + 3..];
    // 言語トークン（同一行の非改行部分）。
    let line_end = after_open.find('\n')?;
    let lang = &after_open[..line_end];
    // 言語トークンに ``` が含まれる（= 空ブロック ```` ``` ````）場合は言語なし扱いにしない簡略化: 対象外。
    if lang.contains('`') {
        return None;
    }
    let body_start = line_end + 1;
    let body_region = &after_open[body_start..];
    let close = body_region.find("```")?;
    let body = &body_region[..close];
    let before = &text[..open];
    let after = &body_region[close + 3..];
    Some((before, lang, body, after))
}

/// JSON 値が Embed 形なら [`RichEmbed`] 群へ変換する。単一 Embed / `{embeds:[...]}` / 配列に対応。
fn embeds_from_value(v: &serde_json::Value) -> Option<Vec<RichEmbed>> {
    // `{ "embed": {...} }` / `{ "embeds": [...] }` ラッパ。
    if let Some(inner) = v.get("embed") {
        return embeds_from_value(inner);
    }
    if let Some(arr) = v.get("embeds").and_then(|x| x.as_array()) {
        let out: Vec<RichEmbed> = arr.iter().filter_map(embed_from_object).collect();
        return (!out.is_empty()).then_some(out);
    }
    if let Some(arr) = v.as_array() {
        let out: Vec<RichEmbed> = arr.iter().filter_map(embed_from_object).collect();
        return (!out.is_empty()).then_some(out);
    }
    embed_from_object(v).map(|e| vec![e])
}

/// 単一オブジェクトが Embed 形（title があり、かつ Embed 固有キーを持つ）なら変換する。
fn embed_from_object(v: &serde_json::Value) -> Option<RichEmbed> {
    let obj = v.as_object()?;
    // Embed と断定できる最低条件: title があること（無ければ Embed 扱いしない）。
    let title = obj.get("title").and_then(|x| x.as_str())?;
    let description = obj
        .get("description")
        .and_then(|x| x.as_str())
        .or_else(|| obj.get("text").and_then(|x| x.as_str()));
    let has_fields = obj.get("fields").and_then(|x| x.as_array()).is_some();
    let has_footer = obj.contains_key("footer");
    let has_color = obj.contains_key("color");

    // さらに Embed 固有キー（description/fields/footer/color）を最低 1 つ伴うことを要求し、
    // 「単なる {title: ...} のデータ」を巻き込みにくくする。
    if description.is_none() && !has_fields && !has_footer && !has_color {
        return None;
    }

    let fields = obj
        .get("fields")
        .and_then(|x| x.as_array())
        .map(|arr| arr.iter().filter_map(field_from_object).collect())
        .unwrap_or_default();

    Some(RichEmbed {
        title: Some(title.to_owned()),
        description: description.map(str::to_owned),
        color: obj.get("color").and_then(parse_color),
        fields,
        footer: footer_text(obj.get("footer")),
    })
}

fn field_from_object(v: &serde_json::Value) -> Option<EmbedField> {
    let obj = v.as_object()?;
    let name = obj.get("name").and_then(|x| x.as_str())?;
    let value = obj
        .get("value")
        .and_then(|x| x.as_str())
        .or_else(|| obj.get("text").and_then(|x| x.as_str()))?;
    Some(EmbedField {
        name: name.to_owned(),
        value: value.to_owned(),
        inline: obj.get("inline").and_then(serde_json::Value::as_bool).unwrap_or(false),
    })
}

/// footer は文字列 or `{ text: "..." }` の両対応。
fn footer_text(v: Option<&serde_json::Value>) -> Option<String> {
    match v {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Object(o)) => o
            .get("text")
            .and_then(|x| x.as_str())
            .map(str::to_owned),
        _ => None,
    }
}

/// color を `0xRRGGBB`（u32）へ。数値・`"#RRGGBB"`・`"RRGGBB"` に対応。範囲外/未知は None。
fn parse_color(v: &serde_json::Value) -> Option<u32> {
    match v {
        serde_json::Value::Number(n) => n.as_u64().and_then(|u| u32::try_from(u).ok()),
        serde_json::Value::String(s) => {
            let hex = s.trim().trim_start_matches('#');
            u32::from_str_radix(hex, 16).ok().filter(|c| *c <= 0xFF_FFFF)
        }
        _ => None,
    }
}

/// 3 連以上の空行を 2 連（＝1 空行）へ圧縮する（ブロック除去後の間延びを整える）。
fn collapse_blank_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank_run = 0usize;
    for line in s.split_inclusive('\n') {
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                out.push_str(line);
            }
        } else {
            blank_run = 0;
            out.push_str(line);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_json_embed_block() {
        let text = "気温をまとめました。\n\n```json\n{\"title\":\"天気\",\"description\":\"晴れ\",\"color\":\"#00ff00\",\"fields\":[{\"name\":\"最高\",\"value\":\"30℃\",\"inline\":true}],\"footer\":\"気象庁\"}\n```\n以上です。";
        let (out, embeds) = recover_embeds_from_text(text);
        assert_eq!(embeds.len(), 1);
        let e = &embeds[0];
        assert_eq!(e.title.as_deref(), Some("天気"));
        assert_eq!(e.description.as_deref(), Some("晴れ"));
        assert_eq!(e.color, Some(0x00ff00));
        assert_eq!(e.fields.len(), 1);
        assert_eq!(e.fields[0].name, "最高");
        assert!(e.fields[0].inline);
        assert_eq!(e.footer.as_deref(), Some("気象庁"));
        assert!(!out.contains("```"), "コードブロックは除去: {out}");
        assert!(out.contains("気温をまとめました"));
        assert!(out.contains("以上です"));
    }

    #[test]
    fn keeps_non_embed_json_block() {
        let text = "設定はこちら:\n```json\n{\"port\": 8080, \"host\": \"localhost\"}\n```";
        let (out, embeds) = recover_embeds_from_text(text);
        assert!(embeds.is_empty());
        assert_eq!(out, text, "Embed 形でない JSON は温存");
    }

    #[test]
    fn keeps_bare_title_only_object() {
        // title だけ（Embed 固有キー無し）は巻き込まない。
        let text = "```json\n{\"title\": \"just data\"}\n```";
        let (out, embeds) = recover_embeds_from_text(text);
        assert!(embeds.is_empty());
        assert_eq!(out, text);
    }

    #[test]
    fn recovers_embeds_wrapper() {
        let text = "```json\n{\"embeds\":[{\"title\":\"A\",\"description\":\"x\"},{\"title\":\"B\",\"description\":\"y\"}]}\n```";
        let (_out, embeds) = recover_embeds_from_text(text);
        assert_eq!(embeds.len(), 2);
        assert_eq!(embeds[1].title.as_deref(), Some("B"));
    }

    #[test]
    fn keeps_non_json_code_block() {
        let text = "```rust\nfn main() {}\n```";
        let (out, embeds) = recover_embeds_from_text(text);
        assert!(embeds.is_empty());
        assert_eq!(out, text);
    }

    #[test]
    fn no_block_is_unchanged() {
        let text = "ただのテキストです。";
        let (out, embeds) = recover_embeds_from_text(text);
        assert!(embeds.is_empty());
        assert_eq!(out, text);
    }
}
