//! browser 系 Native ツール（Node `browserModule`/`browserFunctions` パリティ・cap=secretary）。
//!
//! 本増分は非対話の 3 本＝`searchWeb`（reqwest+scraper・chromium 不要）/`fetchDynamicPage`
//! （静的 fetch → 失敗/短小なら chromium `--dump-dom` → markdown）/`takePageScreenshot`
//! （chromium `--screenshot`）。対話セッション 6 本（要 CDP）は後続増分。

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use yuuka_core::tool::FunctionDeclaration;
use yuuka_core::{Tool, ToolContext, ToolError, ToolName, ToolOutcome};

use crate::interactive::BrowserManager;
use crate::{chromium, markdown, search, ssrf};

/// markdown 上限（Node の 30000 文字 slice）。
const MARKDOWN_LIMIT: usize = 30_000;
/// この文字数未満なら静的 fetch を失敗扱いにし chromium へフォールバック（Node の閾値）。
const MIN_STATIC_LEN: usize = 100;

/// このドメインが公開する Native ツール一式（chromium CLI + reqwest/scraper 経路の 3 本）。
///
/// # Errors
/// ツール名が Gemini 制約に反する場合（コンパイル時定数のため通常発生しない）。
pub fn tools(manager: Arc<BrowserManager>) -> Result<Vec<Arc<dyn Tool>>, ToolError> {
    // 対話ブラウザは per-user chromium を管理する共有マネージャ（対話 6 ツール + browserFillCredential
    //〔yuuka-credential〕が同一インスタンスを共有＝supervisor が 1 つ生成し注入する）。
    Ok(vec![
        Arc::new(SearchWebTool {
            name: ToolName::checked("searchWeb".to_owned())?,
        }),
        Arc::new(FetchDynamicPageTool {
            name: ToolName::checked("fetchDynamicPage".to_owned())?,
        }),
        Arc::new(TakePageScreenshotTool {
            name: ToolName::checked("takePageScreenshot".to_owned())?,
        }),
        Arc::new(InteractiveOpenTool {
            name: ToolName::checked("browserInteractiveOpen".to_owned())?,
            manager: Arc::clone(&manager),
        }),
        Arc::new(InteractiveClickTool {
            name: ToolName::checked("browserInteractiveClick".to_owned())?,
            manager: Arc::clone(&manager),
        }),
        Arc::new(InteractiveTypeTool {
            name: ToolName::checked("browserInteractiveType".to_owned())?,
            manager: Arc::clone(&manager),
        }),
        Arc::new(InteractiveWaitTool {
            name: ToolName::checked("browserInteractiveWait".to_owned())?,
            manager: Arc::clone(&manager),
        }),
        Arc::new(InteractiveStatusTool {
            name: ToolName::checked("browserInteractiveStatus".to_owned())?,
            manager: Arc::clone(&manager),
        }),
        Arc::new(InteractiveCloseTool {
            name: ToolName::checked("browserInteractiveClose".to_owned())?,
            manager,
        }),
    ])
}

/// `{success:false, message}` を返す。
fn fail(message: impl Into<String>) -> ToolOutcome {
    ToolOutcome::from_payload(json!({ "success": false, "message": message.into() }))
}

/// 文字列引数を trim して取り出す（空は None）。
fn arg_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// 文字数で truncate する（Node `slice(0, n)` 相当・超過時はそのまま切る）。
fn truncate_chars(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        s.chars().take(n).collect()
    } else {
        s.to_owned()
    }
}

// ─── searchWeb ────────────────────────────────────────────────────────────────

struct SearchWebTool {
    name: ToolName,
}

#[async_trait]
impl Tool for SearchWebTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "インターネットでキーワード検索し、関連ページのタイトル・URL・説明文の一覧を取り出す。\n\
                ・例: 今の天気、最新ニュース、事実確認など、その時々の新しい情報を調べたい時の最初の一歩に使う。\n\
                ・もっと詳しく知りたい時は、得られたURLを fetchDynamicPage に渡してページ本文を読む。\n\
                ・検索とページ閲覧を何度か繰り返し、複数の情報を見比べて確かめるとよい。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "検索キーワード（例: '東京 明日の天気'）" }
                },
                "required": ["query"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, _ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(query) = arg_str(&args, "query") else {
            return Ok(fail("検索キーワード（query）を指定してください。"));
        };
        match search::search_web(&query).await {
            Ok(results) => Ok(ToolOutcome::from_payload(json!({
                "success": true,
                "query": query,
                "results": results,
            }))),
            Err(message) => Ok(fail(message)),
        }
    }
}

// ─── fetchDynamicPage ─────────────────────────────────────────────────────────

struct FetchDynamicPageTool {
    name: ToolName,
}

#[async_trait]
impl Tool for FetchDynamicPageTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "指定したURLのページを開いて、本文だけを軽くまとめたHTMLを取り出す。\n\
                ・JavaScriptで作られるページ（SPAなど）にも対応する。\n\
                ・スクリプト・スタイル・ナビ・フッター・画像・メタ情報などの不要部分を取り除くので、中身を正確に読みやすい。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "開きたいウェブページのURL" }
                },
                "required": ["url"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, _ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(url) = arg_str(&args, "url") else {
            return Ok(fail("URL を指定してください。"));
        };
        if let Err(msg) = ssrf::assert_safe_outbound_url(&url).await {
            return Ok(fail(msg));
        }

        // 静的 fetch → markdown。短小/失敗なら chromium --dump-dom へフォールバック（Node の多段）。
        let markdown_text = match static_markdown(&url).await {
            Some(md) => md,
            None => match chromium::dump_dom(&url).await {
                Ok(html) => markdown::html_to_markdown(&html),
                Err(e) => return Ok(fail(format!("ページの取得に失敗しました: {e}"))),
            },
        };

        let title = first_heading(&markdown_text);
        let content = truncate_chars(&markdown_text, MARKDOWN_LIMIT);
        Ok(ToolOutcome::from_payload(json!({
            "success": true,
            "url": url,
            "title": title,
            "markdownContent": content,
            // 後方互換: 旧クライアントは htmlContent を読む（Node と同一・同値）。
            "htmlContent": content,
        })))
    }
}

/// 静的 fetch + markdown 化。取得失敗、または本文が極端に短い場合は `None`（chromium 経路へ）。
async fn static_markdown(url: &str) -> Option<String> {
    let html = search::fetch_html(url).await.ok()?;
    let md = markdown::html_to_markdown(&html);
    if md.trim().chars().count() < MIN_STATIC_LEN {
        None
    } else {
        Some(md)
    }
}

/// markdown 先頭の `# 見出し` を title として取り出す（無ければ "無題のページ"）。
fn first_heading(markdown_text: &str) -> String {
    markdown_text
        .lines()
        .find_map(|l| l.strip_prefix("# ").map(str::trim).filter(|s| !s.is_empty()))
        .map_or_else(|| "無題のページ".to_owned(), str::to_owned)
}

// ─── takePageScreenshot ───────────────────────────────────────────────────────

struct TakePageScreenshotTool {
    name: ToolName,
}

#[async_trait]
impl Tool for TakePageScreenshotTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "指定したURLのページ全体のスクリーンショットを撮り、画像としてサーバーに保存する。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "スクリーンショットを撮るウェブページのURL" }
                },
                "required": ["url"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, _ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(url) = arg_str(&args, "url") else {
            return Ok(fail("URL を指定してください。"));
        };
        if let Err(msg) = ssrf::assert_safe_outbound_url(&url).await {
            return Ok(fail(msg));
        }

        // 保存先 data/screenshots/screenshot_{ts}.png（Node と同じ・cwd 相対で返す）。
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let rel: PathBuf = ["data", "screenshots"].iter().collect();
        if let Err(e) = std::fs::create_dir_all(&rel) {
            return Ok(fail(format!("保存先ディレクトリを作成できません: {e}")));
        }
        let file = rel.join(format!("screenshot_{stamp}.png"));

        match chromium::screenshot(&url, &file).await {
            Ok(()) => Ok(ToolOutcome::from_payload(json!({
                "success": true,
                "url": url,
                "message": "スクリーンショットの撮影に成功しました。",
                "imagePath": file.to_string_lossy(),
            }))),
            Err(e) => Ok(fail(format!("スクリーンショットの撮影に失敗しました: {e}"))),
        }
    }
}

// ─── browserInteractive*（対話セッション・要 CDP） ─────────────────────────────

/// timeoutMs を取り出す（数値・省略は 5000＝Node 既定）。
fn arg_timeout(args: &Value) -> u64 {
    args.get("timeoutMs")
        .and_then(Value::as_u64)
        .filter(|n| *n > 0)
        .unwrap_or(5000)
}

/// status 用スクショ保存先（data/screenshots/interactive_screenshot_{ts}.png・cwd 相対）。
fn interactive_screenshot_path() -> Result<PathBuf, String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let dir: PathBuf = ["data", "screenshots"].iter().collect();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(format!("interactive_screenshot_{stamp}.png")))
}

struct InteractiveOpenTool {
    name: ToolName,
    manager: Arc<BrowserManager>,
}

#[async_trait]
impl Tool for InteractiveOpenTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "操作用ブラウザのセッションを開始（または再利用）して、指定したURLを開く。\n\
                ・ログインやページ操作を代行したい時の、いちばん最初の手順として呼ぶ。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "操作ブラウザで開きたいウェブページのURL" }
                },
                "required": ["url"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(url) = arg_str(&args, "url") else {
            return Ok(fail("URL を指定してください。"));
        };
        if let Err(msg) = ssrf::assert_safe_outbound_url(&url).await {
            return Ok(fail(msg));
        }
        match self.manager.open(ctx.user_id.as_str(), &url).await {
            Ok(v) => Ok(ToolOutcome::from_payload(v)),
            Err(e) => Ok(fail(e)),
        }
    }
}

struct InteractiveClickTool {
    name: ToolName,
    manager: Arc<BrowserManager>,
}

#[async_trait]
impl Tool for InteractiveClickTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "操作用ブラウザで今開いているページ上の、指定した要素をクリックする。\n\
                ・操作できる要素には [ID: 数値] や [Button ID: 数値] のように番号が振ってある。\n\
                ・selector には、まずその数値ID（例: '3'）をそのまま入れるのが一番確実。\n\
                ・CSSセレクタや要素内のテキストでも指定できるが、数値IDを優先する。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "selector": { "type": "string", "description": "クリックする要素の数値ID（最優先、例: '3'）。またはCSSセレクタ／要素内のテキストでも可" }
                },
                "required": ["selector"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(selector) = arg_str(&args, "selector") else {
            return Ok(fail("selector を指定してください。"));
        };
        match self.manager.click(ctx.user_id.as_str(), &selector).await {
            Ok(v) => Ok(ToolOutcome::from_payload(v)),
            Err(e) => Ok(fail(e)),
        }
    }
}

struct InteractiveTypeTool {
    name: ToolName,
    manager: Arc<BrowserManager>,
}

#[async_trait]
impl Tool for InteractiveTypeTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "操作用ブラウザで今開いているページの、指定した入力欄に文字を打ち込む。\n\
                ・入力欄には [Input (text) ID: 数値] のように番号が振ってある。\n\
                ・selector には、まずその数値ID（例: '2'）をそのまま入れるのが一番確実。\n\
                ・CSSセレクタやプレースホルダー名でも指定できるが、数値IDを優先する。\n\
                ・パスワードの入力にはこれを使わず、必ず browserFillCredential を使う。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "selector": { "type": "string", "description": "文字を入れる入力欄の数値ID（最優先、例: '2'）。またはCSSセレクタ／プレースホルダー名／name属性の一部でも可" },
                    "text": { "type": "string", "description": "打ち込む文字の内容" }
                },
                "required": ["selector", "text"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(selector) = arg_str(&args, "selector") else {
            return Ok(fail("selector を指定してください。"));
        };
        // text は空文字も許容（Node は String(args.text)）。
        let text = args.get("text").and_then(Value::as_str).unwrap_or("");
        match self
            .manager
            .type_text(ctx.user_id.as_str(), &selector, text)
            .await
        {
            Ok(v) => Ok(ToolOutcome::from_payload(v)),
            Err(e) => Ok(fail(e)),
        }
    }
}

struct InteractiveWaitTool {
    name: ToolName,
    manager: Arc<BrowserManager>,
}

#[async_trait]
impl Tool for InteractiveWaitTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "操作用ブラウザで今開いているページの読み込みや表示を待つ。\n\
                ・指定したミリ秒だけ待つか、指定したCSSセレクタの要素が画面に出るまで待つ。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "selector": { "type": "string", "description": "出現を待ちたい要素のCSSセレクタ（省略可）" },
                    "timeoutMs": { "type": "number", "description": "待つ時間（ミリ秒）。省略=5000ミリ秒（5秒）" }
                }
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let selector = arg_str(&args, "selector");
        let timeout = arg_timeout(&args);
        match self
            .manager
            .wait(ctx.user_id.as_str(), selector.as_deref(), timeout)
            .await
        {
            Ok(v) => Ok(ToolOutcome::from_payload(v)),
            Err(e) => Ok(fail(e)),
        }
    }
}

struct InteractiveStatusTool {
    name: ToolName,
    manager: Arc<BrowserManager>,
}

#[async_trait]
impl Tool for InteractiveStatusTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "操作用ブラウザの今の状態を取り出す（今のURL・タイトル・最新スクショ画像のパス・読みやすく整えた本文）。\n\
                ・クリックや文字入力をした後は、画面がどう変わったか確認するために必ずこれを呼ぶ。"
                .to_owned(),
            parameters_json_schema: json!({ "type": "object", "properties": {} }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, _args: Value) -> Result<ToolOutcome, ToolError> {
        let path = match interactive_screenshot_path() {
            Ok(p) => p,
            Err(e) => return Ok(fail(format!("保存先を用意できません: {e}"))),
        };
        match self.manager.status(ctx.user_id.as_str(), &path).await {
            Ok(v) => Ok(ToolOutcome::from_payload(v)),
            Err(e) => Ok(fail(e)),
        }
    }
}

struct InteractiveCloseTool {
    name: ToolName,
    manager: Arc<BrowserManager>,
}

#[async_trait]
impl Tool for InteractiveCloseTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "操作用ブラウザのセッションを終了し、ブラウザを完全に閉じてリソースを解放する。\n\
                ・一連の操作の代行がすべて終わったら、最後にこれを呼ぶ。"
                .to_owned(),
            parameters_json_schema: json!({ "type": "object", "properties": {} }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, _args: Value) -> Result<ToolOutcome, ToolError> {
        match self.manager.close(ctx.user_id.as_str()).await {
            Ok(v) => Ok(ToolOutcome::from_payload(v)),
            Err(e) => Ok(fail(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_expose_nine_bare_names() {
        let t = tools(Arc::new(BrowserManager::new())).unwrap();
        let names: Vec<String> = t.iter().map(|x| x.declaration().name.to_string()).collect();
        for expected in [
            "searchWeb",
            "fetchDynamicPage",
            "takePageScreenshot",
            "browserInteractiveOpen",
            "browserInteractiveClick",
            "browserInteractiveType",
            "browserInteractiveWait",
            "browserInteractiveStatus",
            "browserInteractiveClose",
        ] {
            assert!(names.contains(&expected.to_owned()), "missing: {expected}");
        }
        assert_eq!(t.len(), 9);
        assert!(!names.iter().any(|n| n.contains(':')), "native は bare 名");
    }

    #[test]
    fn browser_tools_are_secretary_exposure() {
        // 既定露出（secretary）= Node browserModule cap:"secretary"。
        let t = tools(Arc::new(BrowserManager::new())).unwrap();
        for tool in &t {
            let e = tool.exposure();
            assert!(e.secretary, "secretary 経路で露出する");
            assert!(!e.guild_assistant, "汎用モードには露出しない");
        }
    }

    #[test]
    fn first_heading_extracts_title() {
        assert_eq!(first_heading("# タイトル\n\n本文"), "タイトル");
        assert_eq!(first_heading("本文だけ"), "無題のページ");
    }

    #[test]
    fn truncate_chars_caps_length() {
        let s: String = "あ".repeat(40_000);
        assert_eq!(truncate_chars(&s, MARKDOWN_LIMIT).chars().count(), MARKDOWN_LIMIT);
    }

    #[tokio::test]
    async fn fetch_rejects_ssrf_url() {
        let t = tools(Arc::new(BrowserManager::new())).unwrap();
        let fetch = t
            .iter()
            .find(|x| x.declaration().name.as_str() == "fetchDynamicPage")
            .unwrap();
        let ctx = ToolContext::new(
            yuuka_core::BotId::system_default(),
            yuuka_core::UserId::new("u"),
        );
        let out = fetch
            .call(&ctx, json!({ "url": "http://169.254.169.254/" }))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);
        assert!(out.payload["message"].as_str().unwrap().contains("内部/予約済み"));
    }
}
