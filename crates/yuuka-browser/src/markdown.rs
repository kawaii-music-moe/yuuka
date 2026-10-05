//! HTML → markdown 変換（Node `src/rust_crawler` の `traverse`/`clean_markdown` 流用・非対話版）。
//!
//! `fetchDynamicPage` 用に本文だけを軽い markdown へ整形する。script/style/nav/footer/img 等の不要
//! 要素・ノイズ class/id・非表示要素を落とし、見出し・リンク・リスト・表・コードを markdown 化する。

use scraper::node::Node;
use scraper::Html;

/// HTML 文字列を markdown へ変換する（title を先頭 `# ` 見出しに付す）。
#[must_use]
pub fn html_to_markdown(html: &str) -> String {
    let document = Html::parse_document(html);
    let title = extract_title(&document);
    let raw = traverse(document.tree.root(), false);
    let body = clean_markdown(&raw);
    if !title.is_empty() && title != "無題のページ" {
        format!("# {title}\n\n{body}")
    } else {
        body
    }
}

/// `<title>` テキストを取り出す（無ければ "無題のページ"）。
#[must_use]
pub fn extract_title(document: &Html) -> String {
    let Ok(selector) = scraper::Selector::parse("title") else {
        return "無題のページ".to_owned();
    };
    match document.select(&selector).next() {
        Some(el) => {
            let t = el.text().collect::<Vec<_>>().join(" ").trim().to_owned();
            if t.is_empty() {
                "無題のページ".to_owned()
            } else {
                t
            }
        }
        None => "無題のページ".to_owned(),
    }
}

/// 空白を 1 個へ圧縮する（Node `compressWhitespace`）。
fn compress_whitespace(s: &str) -> String {
    let mut result = String::new();
    let mut last_was_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !last_was_space {
                result.push(' ');
                last_was_space = true;
            }
        } else {
            result.push(c);
            last_was_space = false;
        }
    }
    result
}

const UNWANTED_TAGS: &[&str] = &[
    "script", "style", "noscript", "iframe", "svg", "img", "header", "footer", "nav", "link",
    "meta", "select", "button", "input", "textarea", "aside",
];

const NOISE_CLASS_KEYWORDS: &[&str] = &[
    "footer",
    "nav",
    "sidebar",
    "menu",
    "ads",
    "advertisement",
    "cookie",
    "popup",
    "modal",
    "overlay",
    "banner",
    "promo",
];

const NOISE_ID_KEYWORDS: &[&str] = &[
    "footer",
    "nav",
    "sidebar",
    "menu",
    "ads",
    "advertisement",
    "cookie",
    "popup",
    "modal",
    "overlay",
];

/// DOM ツリーを再帰的に markdown へ（Node crawler `traverse`・interactive=false 相当）。
fn traverse(node: ego_tree::NodeRef<'_, Node>, is_pre: bool) -> String {
    let node_data = node.value();

    // 要素でもテキストでもない（Document/Fragment 等）は子だけ辿る。
    let Some(el) = node_data.as_element() else {
        if let Some(text) = node_data.as_text() {
            return if is_pre {
                text.to_string()
            } else {
                compress_whitespace(text)
            };
        }
        let mut children_text = String::new();
        for child in node.children() {
            children_text.push_str(&traverse(child, is_pre));
        }
        return children_text;
    };

    let tag_name = el.name().to_ascii_lowercase();
    if UNWANTED_TAGS.contains(&tag_name.as_str()) {
        return String::new();
    }

    // ノイズ class / id / aria-hidden / display:none を落とす。
    if let Some(class_val) = el.attr("class") {
        let lower = class_val.to_ascii_lowercase();
        if NOISE_CLASS_KEYWORDS.iter().any(|k| lower.contains(k)) {
            return String::new();
        }
    }
    if let Some(id_val) = el.attr("id") {
        let lower = id_val.to_ascii_lowercase();
        if NOISE_ID_KEYWORDS.iter().any(|k| lower.contains(k)) {
            return String::new();
        }
    }
    if el.attr("aria-hidden") == Some("true") {
        return String::new();
    }
    if let Some(style) = el.attr("style") {
        let s = style.to_ascii_lowercase();
        if s.contains("display:none")
            || s.contains("display: none")
            || s.contains("visibility:hidden")
            || s.contains("visibility: hidden")
        {
            return String::new();
        }
    }

    let is_next_pre = is_pre || tag_name == "pre" || tag_name == "code";
    let mut children_text = String::new();
    for child in node.children() {
        children_text.push_str(&traverse(child, is_next_pre));
    }

    match tag_name.as_str() {
        "h1" => format!("\n\n# {}\n\n", children_text.trim()),
        "h2" => format!("\n\n## {}\n\n", children_text.trim()),
        "h3" => format!("\n\n### {}\n\n", children_text.trim()),
        "h4" | "h5" | "h6" => format!("\n\n#### {}\n\n", children_text.trim()),
        "p" => format!("\n\n{}\n\n", children_text.trim()),
        "br" => "\n".to_owned(),
        "hr" => "\n\n---\n\n".to_owned(),
        "a" => {
            let href = el.attr("href").unwrap_or("").trim();
            let text = children_text.trim();
            if !href.is_empty()
                && !text.is_empty()
                && !href.starts_with("javascript:")
                && !href.starts_with("mailto:")
            {
                format!(" [{text}]({href}) ")
            } else {
                children_text
            }
        }
        "li" => format!("\n- {}", children_text.trim()),
        "ul" | "ol" => format!("\n{children_text}\n"),
        "th" | "td" => {
            let cell = children_text.replace(['\n', '\r'], " ");
            format!(" {} |", compress_whitespace(cell.trim()))
        }
        "tr" => format!("\n|{children_text}"),
        "thead" | "tbody" | "table" => format!("\n\n{children_text}\n\n"),
        "pre" => format!("\n```\n{}\n```\n", children_text.trim()),
        "code" => {
            if is_pre {
                children_text
            } else {
                format!(" `{}` ", children_text.trim())
            }
        }
        "div" | "section" | "article" | "main" | "body" | "blockquote" | "form" => {
            format!("\n{children_text}\n")
        }
        _ => children_text,
    }
}

/// 連続空行を 1 行に潰す等の整形（Node crawler `cleanMarkdown`）。コードブロック内は保全する。
/// 対話ページの `extractPageMarkdown` 後処理でも共有する。
pub(crate) fn clean_markdown(s: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut in_code_block = false;
    let mut consecutive_empty = 0;

    for line in s.lines() {
        let trimmed_line = line.trim();
        if trimmed_line.starts_with("```") {
            in_code_block = !in_code_block;
            consecutive_empty = 0;
            lines.push(trimmed_line.to_owned());
            continue;
        }
        if in_code_block {
            lines.push(line.to_owned());
            consecutive_empty = 0;
        } else {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                consecutive_empty += 1;
                if consecutive_empty <= 1 {
                    lines.push(String::new());
                }
            } else {
                consecutive_empty = 0;
                lines.push(trimmed.to_owned());
            }
        }
    }

    lines.join("\n").trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_noise_and_renders_headings_links() {
        let html = r#"<html><head><title>テスト</title></head><body>
            <nav class="menu">ナビ</nav>
            <script>evil()</script>
            <h1>見出し</h1>
            <p>本文です。<a href="https://example.com">リンク</a></p>
            <footer id="footer">フッタ</footer>
        </body></html>"#;
        let md = html_to_markdown(html);
        assert!(md.starts_with("# テスト"));
        assert!(md.contains("# 見出し"));
        assert!(md.contains("[リンク](https://example.com)"));
        assert!(!md.contains("evil"));
        assert!(!md.contains("ナビ"));
        assert!(!md.contains("フッタ"));
    }

    #[test]
    fn drops_javascript_links_keeps_text() {
        let html = r#"<body><p><a href="javascript:void(0)">クリック</a></p></body>"#;
        let md = html_to_markdown(html);
        assert!(md.contains("クリック"));
        assert!(!md.contains("javascript:"));
    }
}
