//! Web 検索（Node `src/rust_crawler` の search 流用）。Google と DuckDuckGo(HTML) を並行取得し、
//! RRF（Reciprocal Rank Fusion）+ 権威スコア + キーワード一致で統合ランキングして上位 8 件を返す。
//! chromium 不要（reqwest + scraper）。

use std::collections::HashMap;
use std::time::Duration;

use scraper::{Html, Selector};
use serde::Serialize;

/// 検索結果 1 件（tool 応答の `results[]` 要素）。
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// 統合ランキング用の中間スコア。
struct Scored {
    result: SearchResult,
    rrf: f64,
}

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
    (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// クエリで検索し、統合ランキング済みの上位 8 件を返す。
///
/// # Errors
/// Google/DDG 双方が失敗、または結果が全て空/除外された場合。
pub async fn search_web(query: &str) -> Result<Vec<SearchResult>, String> {
    let google_url = format!("https://www.google.com/search?q={}", encode_query(query));
    let ddg_url = format!(
        "https://html.duckduckgo.com/html/?q={}",
        encode_query(query)
    );

    let (google_res, ddg_res) = tokio::join!(fetch_html(&google_url), fetch_html(&ddg_url));

    let google = google_res.map(|h| parse_google(&h)).unwrap_or_default();
    let ddg = ddg_res.map(|h| parse_ddg(&h)).unwrap_or_default();

    if google.is_empty() && ddg.is_empty() {
        return Err("検索結果が得られませんでした。".to_owned());
    }

    let merged = merge_and_rank(query, google, ddg);
    if merged.is_empty() {
        return Err("検索結果が全て除外されました。".to_owned());
    }
    Ok(merged)
}

/// `encodeURIComponent` 相当（クエリ用の最小パーセントエンコード）。
fn encode_query(q: &str) -> String {
    let mut out = String::new();
    for byte in q.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// RRF + 権威 + キーワードで統合し上位 8 件へ（Node `mergeAndRankResults`）。
fn merge_and_rank(
    query: &str,
    google: Vec<SearchResult>,
    ddg: Vec<SearchResult>,
) -> Vec<SearchResult> {
    let k = 60.0;
    let mut map: HashMap<String, Scored> = HashMap::new();

    for (i, res) in google.into_iter().enumerate() {
        let rrf = 1.0 / (k + (i + 1) as f64);
        map.insert(res.url.clone(), Scored { result: res, rrf });
    }
    for (i, res) in ddg.into_iter().enumerate() {
        let rrf = 1.0 / (k + (i + 1) as f64);
        match map.get_mut(&res.url) {
            Some(existing) => existing.rrf += rrf,
            None => {
                map.insert(res.url.clone(), Scored { result: res, rrf });
            }
        }
    }

    let mut scored: Vec<(f64, f64, SearchResult)> = map
        .into_values()
        .map(|s| {
            let authority = evaluate_authority(&s.result.url);
            let keyword = evaluate_keyword_relevance(query, &s.result.title, &s.result.snippet);
            let final_score = s.rrf + authority * 0.02 + keyword * 0.01;
            (authority, final_score, s.result)
        })
        .filter(|(authority, _, _)| *authority > -0.9)
        .collect();

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(8).map(|(_, _, r)| r).collect()
}

/// URL の権威スコア（Node `evaluateAuthority`）。
fn evaluate_authority(url: &str) -> f64 {
    let u = url.to_ascii_lowercase();
    let mut score = 0.0;
    if u.contains(".go.jp") || u.contains("jma.go.jp") {
        score += 1.0;
    } else if u.contains(".ac.jp") || u.contains(".edu") {
        score += 0.5;
    } else if u.contains(".or.jp") || u.contains(".org") {
        score += 0.3;
    }
    const TRUSTED: &[&str] = &[
        "itmedia.co.jp",
        "impress.co.jp",
        "nikkei.com",
        "asahi.com",
        "yomiuri.co.jp",
        "mainichi.jp",
        "nhk.or.jp",
        "wikipedia.org",
        "github.com",
        "microsoft.com",
        "transit.yahoo.co.jp",
        "weather.yahoo.co.jp",
        "jma.go.jp",
    ];
    for s in TRUSTED {
        if u.contains(s) {
            score += 0.6;
        }
    }
    const SPAM: &[&str] = &[
        "matome",
        "blog.jp",
        "livedoor.biz",
        "2ch",
        "5ch",
        "geha",
        "affiliate",
        "hachima",
        "jin115",
        "matomedane",
        "togetter",
    ];
    for s in SPAM {
        if u.contains(s) {
            score -= 1.0;
        }
    }
    score
}

/// タイトル/スニペットとクエリ語のキーワード一致スコア（Node `evaluateKeywordRelevance`）。
fn evaluate_keyword_relevance(query: &str, title: &str, snippet: &str) -> f64 {
    let title_l = title.to_ascii_lowercase();
    let snippet_l = snippet.to_ascii_lowercase();
    let query_l = query.to_ascii_lowercase();
    let words: Vec<&str> = query_l.split_whitespace().collect();
    if words.is_empty() {
        return 0.0;
    }
    let mut m = 0.0;
    for w in &words {
        if title_l.contains(w) {
            m += 2.0;
        }
        if snippet_l.contains(w) {
            m += 1.0;
        }
    }
    m / words.len() as f64
}

/// Google 検索結果 HTML をパースする（`div.g` → h3/a/snippet・最大 8 件）。
fn parse_google(html: &str) -> Vec<SearchResult> {
    let document = Html::parse_document(html);
    let (Ok(container), Ok(title_sel), Ok(anchor_sel)) = (
        Selector::parse("div.g"),
        Selector::parse("h3"),
        Selector::parse("a"),
    ) else {
        return Vec::new();
    };
    let snippet_sels: Vec<Selector> = ["div.VwiC3b", "span.aCOpbc", "div.yD3zGc"]
        .iter()
        .filter_map(|s| Selector::parse(s).ok())
        .collect();

    let mut results = Vec::new();
    for c in document.select(&container) {
        let (Some(title_el), Some(anchor)) =
            (c.select(&title_sel).next(), c.select(&anchor_sel).next())
        else {
            continue;
        };
        let title = title_el
            .text()
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_owned();
        let url = anchor.attr("href").unwrap_or("").to_owned();
        let mut snippet = String::new();
        for sel in &snippet_sels {
            if let Some(el) = c.select(sel).next() {
                snippet = el.text().collect::<Vec<_>>().join(" ").trim().to_owned();
                if !snippet.is_empty() {
                    break;
                }
            }
        }
        if !title.is_empty() && !url.is_empty() {
            results.push(SearchResult {
                title,
                url,
                snippet,
            });
        }
        if results.len() >= 8 {
            break;
        }
    }
    results
}

/// DuckDuckGo(HTML) 検索結果をパースする（`.result` → title link/snippet・最大 8 件）。
fn parse_ddg(html: &str) -> Vec<SearchResult> {
    let document = Html::parse_document(html);
    let (Ok(container), Ok(link_sel), Ok(snippet_sel)) = (
        Selector::parse(".result"),
        Selector::parse(".result__title a"),
        Selector::parse(".result__snippet"),
    ) else {
        return Vec::new();
    };
    let mut results = Vec::new();
    for c in document.select(&container) {
        let Some(link) = c.select(&link_sel).next() else {
            continue;
        };
        let title = link.text().collect::<Vec<_>>().join(" ").trim().to_owned();
        let url = link.attr("href").unwrap_or("").to_owned();
        let snippet = c
            .select(&snippet_sel)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_owned())
            .unwrap_or_default();
        if !title.is_empty() && !url.is_empty() {
            results.push(SearchResult {
                title,
                url,
                snippet,
            });
        }
        if results.len() >= 8 {
            break;
        }
    }
    results
}

/// ブラウザ相当ヘッダ + 指数バックオフで HTML を取得する（Node `fetchHtmlWithRetry`）。
/// `fetchDynamicPage` の静的取得経路でも共有する。
///
/// # Errors
/// 全リトライ失敗時に最後のエラー文言を返す。
pub async fn fetch_html(url: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| e.to_string())?;

    let mut delay = Duration::from_secs(1);
    let max_retries = 3;
    let mut last_err = "ページ取得に失敗しました。".to_owned();

    for i in 0..max_retries {
        let req = client
            .get(url)
            .header("User-Agent", UA)
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8",
            )
            .header("Accept-Language", "ja,en-US;q=0.9,en;q=0.8")
            .header("Cache-Control", "no-cache")
            .header("Sec-Fetch-Dest", "document")
            .header("Sec-Fetch-Mode", "navigate")
            .header("Sec-Fetch-Site", "none")
            .header("Upgrade-Insecure-Requests", "1")
            .timeout(Duration::from_secs(15));

        match req.send().await {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    return response.text().await.map_err(|e| e.to_string());
                }
                last_err = format!("サーバーがステータス {status} を返しました。");
                if status == reqwest::StatusCode::FORBIDDEN
                    || status == reqwest::StatusCode::UNAUTHORIZED
                {
                    return Err(last_err);
                }
            }
            Err(e) => last_err = format!("リクエストエラー: {e}"),
        }

        if i < max_retries - 1 {
            tokio::time::sleep(delay).await;
            delay *= 2;
        }
    }
    Err(last_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_query_percent_encodes_spaces_and_kanji() {
        assert_eq!(encode_query("a b"), "a%20b");
        assert_eq!(encode_query("東京"), "%E6%9D%B1%E4%BA%AC");
    }

    #[test]
    fn authority_boosts_gov_and_penalizes_spam() {
        assert!(evaluate_authority("https://www.jma.go.jp/x") > 0.9);
        assert!(evaluate_authority("https://matome.example.com/x") < 0.0);
    }

    #[test]
    fn keyword_relevance_weights_title_double() {
        // title 一致 = 2.0, snippet 一致 = 1.0, / 語数。
        let s = evaluate_keyword_relevance("東京 天気", "東京の天気", "今日");
        assert!(s > 0.0);
    }

    #[test]
    fn merge_dedupes_by_url_and_ranks() {
        let g = vec![SearchResult {
            title: "A".into(),
            url: "https://example.com/a".into(),
            snippet: String::new(),
        }];
        let d = vec![SearchResult {
            title: "A".into(),
            url: "https://example.com/a".into(),
            snippet: String::new(),
        }];
        let merged = merge_and_rank("q", g, d);
        assert_eq!(merged.len(), 1, "同一 URL は 1 件へ統合");
    }

    #[test]
    fn parse_ddg_extracts_results() {
        let html = r#"<div class="result"><a class="result__a" href="x"></a>
            <h2 class="result__title"><a href="https://e.com">タイトル</a></h2>
            <a class="result__snippet">説明文</a></div>"#;
        let r = parse_ddg(html);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title, "タイトル");
        assert_eq!(r[0].url, "https://e.com");
    }
}
