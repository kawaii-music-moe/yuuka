//! 朝報コンテンツ生成（Node `briefingService.runBriefingForUser` の天気/RSS 部分パリティ）。
//!
//! 天気は **Open-Meteo（api.open-meteo.com・API キー不要）**、ニュースは登録 RSS フィード（SSRF
//! ガード付き・個別失敗は無視）。生成した [`BriefingContent`] を `runBriefingNow` ツールがインライン
//! Embed として返す（手動確認用）。**LLM 要約（generateAuxText）は Gemini 依存のため未配線**＝
//! フォールバックのヘッドライン列挙で代替する。定時配信（cron→DM/channel）は DeliveryRunner シーム
//! （Discord live 配線時）が担い、本サービスと同じ生成ロジックを再利用できる。

use std::time::Duration;

use crate::repo::find_briefing;
use yuuka_core::DbError;
use yuuka_web::Db;

/// 生成した朝報の中身（Embed の各フィールド）。
pub struct BriefingContent {
    pub fields: Vec<(String, String)>,
    /// 天気/ニュースいずれかの実コンテンツがあるか。
    pub has_content: bool,
}

/// 設定が存在すれば朝報コンテンツを生成する（無ければ `None`）。
///
/// # Errors
/// 設定読み取り失敗時 [`DbError`]。
pub async fn build_briefing(
    db: &Db,
    user_id: &str,
    bot_id: &str,
) -> Result<Option<BriefingContent>, DbError> {
    let Some(config) = find_briefing(db, user_id, bot_id).await? else {
        return Ok(None);
    };

    let mut fields: Vec<(String, String)> = Vec::new();
    let mut has_content = false;

    // ── 天気（Open-Meteo） ──
    if let (Some(lat), Some(lng)) = (config.weather_lat, config.weather_lng) {
        match fetch_weather(lat, lng).await {
            Some(days) if !days.is_empty() => {
                let location = config
                    .location_name
                    .clone()
                    .unwrap_or_else(|| format!("{lat}, {lng}"));
                let lines: Vec<String> = days
                    .iter()
                    .enumerate()
                    .map(|(i, w)| {
                        let day = if i == 0 { "今日" } else { "明日" };
                        let rain = w
                            .precipitation
                            .map(|p| format!(" / 降水確率 {p}%"))
                            .unwrap_or_default();
                        format!(
                            "**{day}**: {}　{}℃〜{}℃{rain}",
                            w.label,
                            w.temp_min.round() as i64,
                            w.temp_max.round() as i64
                        )
                    })
                    .collect();
                fields.push((format!("🌤️ {location} の天気"), lines.join("\n")));
                has_content = true;
            }
            _ => fields.push((
                "🌤️ 天気".to_owned(),
                "天気情報の取得に失敗しました。".to_owned(),
            )),
        }
    }

    // ── ニュース（RSS・LLM 要約は未配線のためヘッドライン列挙で代替） ──
    if !config.news_feeds.is_empty() {
        let items = fetch_news(&config.news_feeds, &config.news_keywords).await;
        if items.is_empty() {
            fields.push((
                "📰 ニュース".to_owned(),
                "フィードから記事を取得できませんでした。".to_owned(),
            ));
        } else {
            let text: String = items
                .iter()
                .take(5)
                .map(|i| format!("・{}", i.title))
                .collect::<Vec<_>>()
                .join("\n");
            let clipped: String = text.chars().take(1024).collect();
            fields.push(("📰 今朝のニュース".to_owned(), clipped));
            has_content = true;
        }
    }

    Ok(Some(BriefingContent {
        fields,
        has_content,
    }))
}

// ─── 天気（Open-Meteo） ────────────────────────────────────────────────────────

struct WeatherDay {
    label: String,
    temp_max: f64,
    temp_min: f64,
    precipitation: Option<i64>,
}

async fn fetch_weather(lat: f64, lng: f64) -> Option<Vec<WeatherDay>> {
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lng}\
         &daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max\
         &timezone=Asia%2FTokyo&forecast_days=2"
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?;
    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    let daily = json.get("daily")?;
    let times = daily.get("time")?.as_array()?;
    let codes = daily.get("weather_code").and_then(|v| v.as_array());
    let maxs = daily.get("temperature_2m_max").and_then(|v| v.as_array());
    let mins = daily.get("temperature_2m_min").and_then(|v| v.as_array());
    let precs = daily
        .get("precipitation_probability_max")
        .and_then(|v| v.as_array());

    let get_f = |arr: Option<&Vec<serde_json::Value>>, i: usize| -> f64 {
        arr.and_then(|a| a.get(i))
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(f64::NAN)
    };
    let out: Vec<WeatherDay> = (0..times.len())
        .map(|i| {
            let code = codes
                .and_then(|a| a.get(i))
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(-1);
            WeatherDay {
                label: weather_code_label(code),
                temp_max: get_f(maxs, i),
                temp_min: get_f(mins, i),
                precipitation: precs
                    .and_then(|a| a.get(i))
                    .and_then(serde_json::Value::as_i64),
            }
        })
        .collect();
    Some(out)
}

/// Open-Meteo weather_code → 日本語ラベル（Node `WEATHER_CODE_MAP`）。
fn weather_code_label(code: i64) -> String {
    let label = match code {
        0 => "快晴",
        1 => "晴れ",
        2 => "一部曇り",
        3 => "曇り",
        45 => "霧",
        48 => "着氷性の霧",
        51 => "弱い霧雨",
        53 => "霧雨",
        55 => "強い霧雨",
        56 => "弱い着氷性霧雨",
        57 => "着氷性霧雨",
        61 => "小雨",
        63 => "雨",
        65 => "大雨",
        66 => "弱い着氷性の雨",
        67 => "着氷性の雨",
        71 => "小雪",
        73 => "雪",
        75 => "大雪",
        77 => "霧雪",
        80 => "にわか雨（弱）",
        81 => "にわか雨",
        82 => "激しいにわか雨",
        85 => "にわか雪（弱）",
        86 => "にわか雪",
        95 => "雷雨",
        96 => "雷雨（弱い雹）",
        99 => "雷雨（強い雹）",
        _ => return format!("不明({code})"),
    };
    label.to_owned()
}

// ─── ニュース（RSS） ───────────────────────────────────────────────────────────

struct NewsItem {
    title: String,
}

async fn fetch_news(feeds: &[String], keywords: &[String]) -> Vec<NewsItem> {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        // The destination check below only covers the original URL. Following redirects here
        // would let an otherwise public feed redirect the request to a loopback/link-local host.
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut items: Vec<NewsItem> = Vec::new();
    for url in feeds.iter().take(10) {
        // SSRF: 取得直前に DNS 解決先まで検査し、内部/予約レンジ・非 http(s) は取得しない
        // （個別失敗は無視・Node パリティ）。登録時の簡易検査はドメインを公開扱いにするため、ここで再検査する。
        if yuuka_browser::assert_safe_outbound_url(url).await.is_err() {
            continue;
        }
        let Ok(resp) = client.get(url).send().await else {
            continue;
        };
        if !resp.status().is_success() {
            continue;
        }
        let Ok(body) = resp.text().await else {
            continue;
        };
        for title in parse_feed_titles(&body).into_iter().take(10) {
            items.push(NewsItem { title });
        }
    }

    // キーワードフィルタ（指定時のみ・全滅時は先頭 5 件・Node パリティ）。
    if keywords.is_empty() {
        return items;
    }
    let filtered: Vec<NewsItem> = items
        .iter()
        .filter(|it| keywords.iter().any(|k| it.title.contains(k)))
        .map(|it| NewsItem {
            title: it.title.clone(),
        })
        .collect();
    if filtered.is_empty() {
        items.into_iter().take(5).collect()
    } else {
        filtered
    }
}

/// RSS/Atom から記事タイトル群を最小抽出する（`<item>`/`<entry>` 内の最初の `<title>`）。
/// チャンネルタイトル（先頭の `<title>`）は除く。CDATA・エンティティを軽く解す。
fn parse_feed_titles(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    // <item> または <entry> 単位で分割し、各チャンクの最初の <title> を取る。
    for chunk in split_items(xml) {
        if let Some(title) = extract_first_tag(chunk, "title") {
            let t = decode_min(&title);
            if !t.is_empty() {
                out.push(t);
            }
        }
    }
    out
}

/// `<item ...>` / `<entry ...>` の開始位置以降を各チャンクにする（先頭のチャンネル部は捨てる）。
fn split_items(xml: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    let bytes = xml;
    let mut rest = bytes;
    loop {
        let lower_pos = find_ci(rest, "<item").or_else(|| find_ci(rest, "<entry"));
        let Some(pos) = lower_pos else { break };
        // 次の item/entry の手前までを 1 チャンクに。
        let after = rest.get(pos + 1..).unwrap_or("");
        let next = find_ci(after, "<item")
            .or_else(|| find_ci(after, "<entry"))
            .map_or(rest.len(), |n| pos + 1 + n);
        if let Some(chunk) = rest.get(pos..next) {
            chunks.push(chunk);
        }
        rest = rest.get(next..).unwrap_or("");
    }
    chunks
}

/// 大文字小文字を無視した部分文字列検索（バイト位置）。
fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let h = haystack.to_ascii_lowercase();
    let n = needle.to_ascii_lowercase();
    h.find(&n)
}

/// チャンク内の最初の `<tag ...>CONTENT</tag>` の CONTENT を取り出す（CDATA 対応）。
fn extract_first_tag(chunk: &str, tag: &str) -> Option<String> {
    let open_pat = format!("<{tag}");
    let start = find_ci(chunk, &open_pat)?;
    let after_open = chunk.get(start..)?;
    let gt = after_open.find('>')?;
    let content_start = start + gt + 1;
    let content_region = chunk.get(content_start..)?;
    let close_pat = format!("</{tag}");
    let end = find_ci(content_region, &close_pat)?;
    let raw = content_region.get(..end)?.trim();
    // CDATA を剥がす。
    let inner = raw
        .strip_prefix("<![CDATA[")
        .and_then(|s| s.strip_suffix("]]>"))
        .unwrap_or(raw);
    Some(inner.trim().to_owned())
}

/// 最小の XML エンティティ復元。
fn decode_min(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

/// 手動 briefing のフッタ + タイトル・色（Node parity・スカイブルー）。
pub(crate) const BRIEFING_TITLE: &str = "🌅 おはようございます！今日の朝報です";
pub(crate) const BRIEFING_COLOR: u32 = 0x0000_b0f4;
pub(crate) const BRIEFING_FOOTER: &str = "データ提供: Open-Meteo / 登録RSSフィード";
pub(crate) const BRIEFING_EMPTY_DESC: &str =
    "朝報に表示する内容がまだ設定されていません。天気の地点（緯度経度）やニュースのRSSフィードを設定してください。";

/// 生成した朝報を**プレーンテキスト**へ整形する（定時配信の [`crate::cron::run_due_briefings`] 用）。
///
/// **意図的 divergence**: Node は Embed（`{ embeds: [embed] }`）で配信するが、Rust の通知ポート
/// （`yuuka_services::Notifier`）は現状 text 経路のみを持つ（Embed は後続拡張）。ここでは Embed の
/// タイトル・各フィールド（`名: 値`）・フッタを Discord のマークダウンで縦に積んだ text 本文にする
/// （runBriefingNow の「インライン Embed で返す」divergence と同系統・情報は等価）。空コンテンツ時は
/// [`BRIEFING_EMPTY_DESC`] を本文に入れる（Node の `setDescription` と等価）。
#[must_use]
pub fn render_briefing_text(content: &BriefingContent) -> String {
    let mut out = String::new();
    out.push_str("**");
    out.push_str(BRIEFING_TITLE);
    out.push_str("**\n");
    if content.has_content {
        for (name, value) in &content.fields {
            out.push('\n');
            out.push_str("**");
            out.push_str(name);
            out.push_str("**\n");
            out.push_str(value);
            out.push('\n');
        }
    } else {
        out.push('\n');
        out.push_str(BRIEFING_EMPTY_DESC);
        out.push('\n');
    }
    out.push('\n');
    out.push_str(BRIEFING_FOOTER);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weather_labels_match_node() {
        assert_eq!(weather_code_label(0), "快晴");
        assert_eq!(weather_code_label(63), "雨");
        assert_eq!(weather_code_label(95), "雷雨");
        assert_eq!(weather_code_label(1234), "不明(1234)");
    }

    #[test]
    fn parse_rss_extracts_item_titles() {
        let xml = r#"<rss><channel><title>My Feed</title>
            <item><title>記事A</title><link>https://e.com/a</link></item>
            <item><title><![CDATA[記事B & C]]></title><link>https://e.com/b</link></item>
            </channel></rss>"#;
        let titles = parse_feed_titles(xml);
        assert_eq!(titles, vec!["記事A".to_owned(), "記事B & C".to_owned()]);
    }

    #[test]
    fn parse_atom_entries() {
        let xml = r#"<feed><title>Atom Feed</title>
            <entry><title>エントリ1</title><link href="https://e.com/1"/></entry></feed>"#;
        let titles = parse_feed_titles(xml);
        assert_eq!(titles, vec!["エントリ1".to_owned()]);
    }

    /// R25: 文字列の簡易検査ではドメイン扱いになるが、実際は内部アドレスへ届くフィードを取得しない。
    /// `127.1` / `2130706433` は URL 仕様で 127.0.0.1 に正規化され、`::ffff:127.0.0.1` は IPv4 の
    /// ループバックへ届く。取得直前の検査で止め、ローカルのサーバーへ 1 度も接続しないことを確かめる。
    #[tokio::test]
    async fn fetch_news_skips_feeds_reaching_internal_address() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let server_hits = Arc::clone(&hits);
        let server = tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                server_hits.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let body = "<rss><channel><item><title>内部の記事</title></item></channel></rss>";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/rss+xml\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });

        let feeds = vec![
            format!("http://127.1:{port}/feed.xml"),
            format!("http://2130706433:{port}/feed.xml"),
            format!("http://[::ffff:127.0.0.1]:{port}/feed.xml"),
        ];
        let items = fetch_news(&feeds, &[]).await;
        server.abort();

        assert!(items.is_empty());
        assert_eq!(hits.load(Ordering::SeqCst), 0, "内部アドレスへ接続した");
    }

    /// R25: a feed URL which passed the initial safety check must not be allowed to redirect the
    /// HTTP client to an internal address. The production client intentionally returns the 302
    /// response to `fetch_news`, which then ignores it as a non-success response.
    #[tokio::test]
    async fn news_client_does_not_follow_redirects_to_internal_address() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let internal = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let internal_addr = internal.local_addr().unwrap();
        let internal_hits = Arc::new(AtomicUsize::new(0));
        let server_hits = Arc::clone(&internal_hits);
        let internal_server = tokio::spawn(async move {
            if let Ok((mut sock, _)) = internal.accept().await {
                server_hits.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let _ = sock
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                    .await;
            }
        });

        let redirector = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redirector_addr = redirector.local_addr().unwrap();
        let redirect_server = tokio::spawn(async move {
            let (mut sock, _) = redirector.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await;
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://{internal_addr}/feed.xml\r\nContent-Length: 0\r\n\r\n"
            );
            sock.write_all(response.as_bytes()).await.unwrap();
        });

        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let response = client
            .get(format!("http://{redirector_addr}/feed.xml"))
            .send()
            .await
            .unwrap();

        redirect_server.await.unwrap();
        tokio::task::yield_now().await;
        internal_server.abort();
        assert_eq!(response.status(), reqwest::StatusCode::FOUND);
        assert_eq!(internal_hits.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn decode_min_unescapes_entities() {
        assert_eq!(decode_min("A &amp; B &lt;x&gt;"), "A & B <x>");
    }

    #[test]
    fn render_text_includes_title_fields_and_footer() {
        let content = BriefingContent {
            fields: vec![
                (
                    "🌤️ 東京 の天気".to_owned(),
                    "**今日**: 晴れ　20℃〜28℃".to_owned(),
                ),
                (
                    "📰 今朝のニュース".to_owned(),
                    "・記事A\n・記事B".to_owned(),
                ),
            ],
            has_content: true,
        };
        let text = render_briefing_text(&content);
        assert!(text.contains(BRIEFING_TITLE));
        assert!(text.contains("🌤️ 東京 の天気"));
        assert!(text.contains("**今日**: 晴れ"));
        assert!(text.contains("・記事A"));
        assert!(text.contains(BRIEFING_FOOTER));
    }

    #[test]
    fn render_text_uses_empty_desc_when_no_content() {
        let content = BriefingContent {
            fields: vec![],
            has_content: false,
        };
        let text = render_briefing_text(&content);
        assert!(text.contains(BRIEFING_TITLE));
        assert!(text.contains(BRIEFING_EMPTY_DESC));
        // 本文は非空（NullNotifier 以外なら配信される）。
        assert!(!text.trim().is_empty());
    }
}
