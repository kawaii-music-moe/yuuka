//! 対話ブラウザ（Node `browserService.ts` の `browserInteractive*` パリティ・要 CDP）。
//!
//! ユーザーごとに 1 つの chromium を chromiumoxide で起動し、永続ページ上で open/click/type/wait/
//! status/close を行う。DOM 注釈（`data-yuuka-id`）・markdown 抽出は Node の `page.evaluate` する
//! JS を流用し chromiumoxide の evaluate で実行する。
//!
//! **ランタイム検証は実機（chrome 稼働環境）で必要**＝本 sandbox に chrome が無いため CDP 経路は
//! コンパイル検証 + 純ロジックの単体テストのみ（Discord live と同じ扱い）。
//!
//! 差分（Node と意図的に異なる点）:
//! - idle auto-close は「次の対話操作時に他ユーザーの idle セッションも掃除」する opportunistic 方式
//!   （Node は per-session タイマ）。完全無活動時のみ掃除が遅れるが、暴走 chrome の蓄積は防ぐ。

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chromiumoxide::{Browser, BrowserConfig, Page};
use futures::StreamExt;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::chromium::find_chrome;
use crate::markdown::clean_markdown;

const VIEWPORT_W: u32 = 1280;
const VIEWPORT_H: u32 = 800;
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
    (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
const AUTO_CLOSE: Duration = Duration::from_secs(5 * 60);
const MARKDOWN_LIMIT: usize = 30_000;

/// ユーザー 1 人ぶんの対話セッション（chromium プロセス + ページ）。
struct Session {
    browser: Browser,
    page: Page,
    last_interaction: Instant,
}

/// 対話ブラウザの per-user セッション管理（プロセス内共有 `Arc<BrowserManager>`）。
#[derive(Default)]
pub struct BrowserManager {
    sessions: Mutex<HashMap<String, Arc<tokio::sync::Mutex<Session>>>>,
}

impl BrowserManager {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// std Mutex を poison 耐性で取る（.await を跨がない・短時間のみ保持）。
    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<tokio::sync::Mutex<Session>>>> {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 既存セッション（生存確認済み）を返すか、新規起動する。idle セッションは掃除する。
    async fn get_or_launch(&self, user: &str) -> Result<Arc<tokio::sync::Mutex<Session>>, String> {
        self.sweep_idle().await;

        // 既存を取り出す（Arc を clone してロックは即解放）。
        let existing = self.map().get(user).map(Arc::clone);
        if let Some(arc) = existing {
            // 生存確認（Node のダミー evaluate）。死んでいれば閉じて作り直す。
            let alive = {
                let mut sess = arc.lock().await;
                sess.last_interaction = Instant::now();
                sess.page.evaluate("1").await.is_ok()
            };
            if alive {
                return Ok(arc);
            }
            self.close(user).await.ok();
        }

        let session = launch_session(user).await?;
        let arc = Arc::new(tokio::sync::Mutex::new(session));
        self.map().insert(user.to_owned(), Arc::clone(&arc));
        Ok(arc)
    }

    /// idle（AUTO_CLOSE 超過）のセッションを閉じる（対話操作時に呼ぶ opportunistic 掃除）。
    async fn sweep_idle(&self) {
        let now = Instant::now();
        let mut stale = Vec::new();
        {
            let map = self.map();
            for (user, arc) in map.iter() {
                if let Ok(sess) = arc.try_lock() {
                    if now.duration_since(sess.last_interaction) >= AUTO_CLOSE {
                        stale.push(user.clone());
                    }
                }
            }
        }
        for user in stale {
            self.close(&user).await.ok();
        }
    }

    /// browserInteractiveOpen（Node）。SSRF は tool 側で検証済み前提でここは遷移のみ。
    pub(crate) async fn open(&self, user: &str, url: &str) -> Result<Value, String> {
        let arc = self.get_or_launch(user).await?;
        let sess = arc.lock().await;
        // 遷移エラー/タイムアウトは握って続行（Node: navigation warn + timeout 20s, continuing）。
        // goto は load を待つため wait_for_navigation は付けない（付けると次遷移待ちで無限待機し得る）。
        let _ = tokio::time::timeout(Duration::from_secs(20), sess.page.goto(url)).await;
        let title = page_title(&sess.page).await;
        let current = sess.page.url().await.ok().flatten().unwrap_or_default();
        Ok(json!({
            "success": true,
            "title": title,
            "url": current,
            "message": format!("URL: {url} を開きました。"),
        }))
    }

    /// browserInteractiveClose（Node）。
    pub(crate) async fn close(&self, user: &str) -> Result<Value, String> {
        let arc = self.map().remove(user);
        if let Some(arc) = arc {
            let mut sess = arc.lock().await;
            let _ = sess.browser.close().await;
        }
        Ok(json!({ "success": true, "message": "ブラウザセッションを正常に終了しました。" }))
    }

    /// browserInteractiveWait（Node）。
    pub(crate) async fn wait(
        &self,
        user: &str,
        selector: Option<&str>,
        timeout_ms: u64,
    ) -> Result<Value, String> {
        let arc = self.get_or_launch(user).await?;
        let sess = arc.lock().await;
        match selector {
            Some(sel) => {
                let actual = resolve_selector(sel);
                wait_for_selector(&sess.page, &actual, timeout_ms).await?;
                Ok(json!({
                    "success": true,
                    "message": format!("要素 \"{actual}\" が出現するまで待機しました。"),
                }))
            }
            None => {
                tokio::time::sleep(Duration::from_millis(timeout_ms)).await;
                Ok(json!({
                    "success": true,
                    "message": format!("{timeout_ms}ms 待機しました。"),
                }))
            }
        }
    }

    /// browserInteractiveClick（Node）。数値ID/`:contains()`/CSS + text フォールバック。
    pub(crate) async fn click(&self, user: &str, selector: &str) -> Result<Value, String> {
        let arc = self.get_or_launch(user).await?;
        let sess = arc.lock().await;
        let actual = resolve_selector(selector);

        // :contains()/:has-text() のパース。
        if let Some((tag, text)) = parse_contains(&actual) {
            let clicked = eval_bool(&sess.page, &click_by_text_js(tag.as_deref(), &text)).await;
            if clicked {
                tokio::time::sleep(Duration::from_millis(1000)).await;
                return Ok(json!({
                    "success": true,
                    "message": format!("テキスト \"{text}\" に合致する要素を見つけ出し、クリックしました。"),
                }));
            }
            return Err(format!(
                "テキスト \"{text}\" に合致する要素 \"{actual}\" のクリックに失敗しました。"
            ));
        }

        // 通常 CSS セレクタ。
        match wait_for_selector(&sess.page, &actual, 5000).await {
            Ok(()) => {
                if let Ok(el) = sess.page.find_element(&actual).await {
                    if el.click().await.is_ok() {
                        tokio::time::sleep(Duration::from_millis(1000)).await;
                        return Ok(json!({
                            "success": true,
                            "message": format!("要素 \"{actual}\" をクリックしました。"),
                        }));
                    }
                }
                // スマートフォールバック（テキストマッチ）。
                self.smart_click_fallback(&sess.page, selector, &actual)
                    .await
            }
            Err(_) => {
                self.smart_click_fallback(&sess.page, selector, &actual)
                    .await
            }
        }
    }

    async fn smart_click_fallback(
        &self,
        page: &Page,
        selector: &str,
        actual: &str,
    ) -> Result<Value, String> {
        let clicked = eval_bool(page, &click_by_text_js(None, actual)).await;
        if clicked {
            tokio::time::sleep(Duration::from_millis(1000)).await;
            Ok(json!({
                "success": true,
                "message": format!("テキスト \"{actual}\" に合致する要素を見つけ出し、クリックしました。"),
            }))
        } else {
            Err(format!(
                "要素またはテキスト \"{selector}\" のクリックに失敗しました。"
            ))
        }
    }

    /// browserInteractiveType（Node）。CSS 入力 + 属性部分一致フォールバック。
    /// `browserFillCredential`（別クレート）も復号値の入力にこれを使うため pub。
    pub async fn type_text(&self, user: &str, selector: &str, text: &str) -> Result<Value, String> {
        let arc = self.get_or_launch(user).await?;
        let sess = arc.lock().await;
        let actual = resolve_selector(selector);

        if wait_for_selector(&sess.page, &actual, 5000).await.is_ok() {
            if let Ok(el) = sess.page.find_element(&actual).await {
                // 既存値をクリアしてから入力（Node: focus→Ctrl+A→Backspace→type delay 50）。
                let _ = el
                    .call_js_fn("function(){ this.focus(); this.value=''; }", false)
                    .await;
                if el.type_str(text).await.is_ok() {
                    return Ok(json!({
                        "success": true,
                        "message": format!("要素 \"{actual}\" にテキストを入力しました。"),
                    }));
                }
            }
        }

        // スマートフォールバック（placeholder/name/id/aria-label 部分一致）。
        let typed = eval_bool(&sess.page, &type_fallback_js(&actual, text)).await;
        if typed {
            Ok(json!({
                "success": true,
                "message": format!("検索キー \"{selector}\" に合致する入力フィールドを見つけ出し、テキストを入力しました。"),
            }))
        } else {
            Err(format!(
                "要素 \"{selector}\" へのテキスト入力に失敗しました。"
            ))
        }
    }

    /// browserInteractiveStatus（Node）。注釈→スクショ→対話 markdown。
    pub(crate) async fn status(&self, user: &str, screenshot_path: &Path) -> Result<Value, String> {
        let arc = self.get_or_launch(user).await?;
        let sess = arc.lock().await;

        // DOM 注釈（失敗は握る＝Node の try/catch warn）。
        let _ = sess.page.evaluate(ANNOTATE_JS).await;

        let title = page_title(&sess.page).await;
        let url = sess.page.url().await.ok().flatten().unwrap_or_default();

        // viewport のみのスクショを保存。
        let shot = sess
            .page
            .screenshot(chromiumoxide::page::ScreenshotParams::builder().build())
            .await
            .map_err(|e| format!("スクリーンショットに失敗しました: {e}"))?;
        std::fs::write(screenshot_path, shot).map_err(|e| e.to_string())?;

        // 対話要素入り markdown（JS で raw を作り Rust で整形）。
        let raw = sess
            .page
            .evaluate(EXTRACT_INTERACTIVE_JS)
            .await
            .map_err(|e| e.to_string())?
            .into_value::<String>()
            .unwrap_or_default();
        let md = clean_markdown(&raw);
        let content: String = md.chars().take(MARKDOWN_LIMIT).collect();

        Ok(json!({
            "success": true,
            "url": url,
            "title": title,
            "imagePath": screenshot_path.to_string_lossy(),
            "markdownContent": content,
        }))
    }
}

/// 新規 chromium を起動してページを整える（viewport/UA/Accept-Language）。
async fn launch_session(user: &str) -> Result<Session, String> {
    let chrome = find_chrome()?;
    let profile = std::path::Path::new("data")
        .join("browser_profiles")
        .join(user);
    let config = BrowserConfig::builder()
        .chrome_executable(chrome)
        .arg("--no-sandbox")
        .arg("--disable-setuid-sandbox")
        .arg("--disable-dev-shm-usage")
        .arg(format!("--window-size={VIEWPORT_W},{VIEWPORT_H}"))
        .user_data_dir(profile)
        .build()?;

    let (browser, mut handler) = Browser::launch(config)
        .await
        .map_err(|e| format!("ブラウザ起動に失敗しました: {e}"))?;
    // Handler ストリームを駆動する（切断/クローズで終了）。
    tokio::spawn(async move { while handler.next().await.is_some() {} });

    let page = browser
        .new_page("about:blank")
        .await
        .map_err(|e| format!("ページ生成に失敗しました: {e}"))?;
    let _ = page.set_user_agent(USER_AGENT).await;

    Ok(Session {
        browser,
        page,
        last_interaction: Instant::now(),
    })
}

/// タイトルを取得する（空/失敗は "無題のページ"）。
async fn page_title(page: &Page) -> String {
    match page.get_title().await {
        Ok(Some(t)) if !t.is_empty() => t,
        _ => "無題のページ".to_owned(),
    }
}

/// 数値 ID / `id:NNN` を `[data-yuuka-id="NNN"]` に変換する（Node のセレクタ正規化）。
fn resolve_selector(selector: &str) -> String {
    let s = selector.trim();
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        return format!("[data-yuuka-id=\"{s}\"]");
    }
    if let Some(rest) = s
        .strip_prefix("id:")
        .or_else(|| s.strip_prefix("ID:").or_else(|| s.strip_prefix("Id:")))
    {
        if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) {
            return format!("[data-yuuka-id=\"{rest}\"]");
        }
    }
    s.to_owned()
}

/// `tag:contains("text")` / `:has-text('text')` をパースする（Node の containsRegex）。
fn parse_contains(selector: &str) -> Option<(Option<String>, String)> {
    let s = selector.trim();
    for marker in [":contains(", ":has-text("] {
        if let Some(idx) = s.find(marker) {
            let tag = &s[..idx];
            let after = &s[idx + marker.len()..];
            let inner = after.strip_suffix(')')?;
            let text = inner.trim().trim_matches(|c| c == '"' || c == '\'');
            if text.is_empty() {
                return None;
            }
            let tag_opt = if tag.is_empty() {
                None
            } else if tag
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                Some(tag.to_owned())
            } else {
                None
            };
            return Some((tag_opt, text.to_owned()));
        }
    }
    None
}

/// JS を評価して bool を得る（失敗は false）。
async fn eval_bool(page: &Page, js: &str) -> bool {
    match page.evaluate(js).await {
        Ok(res) => res.into_value::<bool>().unwrap_or(false),
        Err(_) => false,
    }
}

/// セレクタ出現をポーリング待ちする（chromiumoxide に waitForSelector 相当が無いため自前）。
async fn wait_for_selector(page: &Page, selector: &str, timeout_ms: u64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if page.find_element(selector).await.is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!("要素 \"{selector}\" が見つかりませんでした。"));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// テキスト一致クリックの JS（Node の smart click・引数は JSON 埋め込み）。
fn click_by_text_js(tag: Option<&str>, text: &str) -> String {
    let tag_js = tag.map_or_else(|| "null".to_owned(), |t| json!(t).to_string());
    let txt_js = json!(text).to_string();
    format!(
        "(() => {{ const tag = {tag_js}; const txt = {txt_js}; \
         const query = tag ? tag : \"a, button, input[type='button'], input[type='submit'], [role='button'], span, div, h1, h2, h3, h4\"; \
         const elements = Array.from(document.querySelectorAll(query)); \
         const target = elements.find((el) => {{ const elText = (el.textContent||'').trim(); const valText = (el.getAttribute('value')||'').trim(); \
         return elText === txt || elText.includes(txt) || valText === txt || valText.includes(txt); }}); \
         if (target) {{ target.click(); return true; }} return false; }})()"
    )
}

/// 属性部分一致で入力する JS（Node の smart type fallback・引数は JSON 埋め込み）。
fn type_fallback_js(selector: &str, text: &str) -> String {
    let sel_js = json!(selector).to_string();
    let txt_js = json!(text).to_string();
    format!(
        "(() => {{ const sel = {sel_js}; const txt = {txt_js}; \
         const inputs = Array.from(document.querySelectorAll('input, textarea')); \
         const lowerSel = sel.toLowerCase(); \
         const target = inputs.find((el) => {{ \
           const placeholder = (el.getAttribute('placeholder')||'').toLowerCase(); \
           const name = (el.getAttribute('name')||'').toLowerCase(); \
           const id = (el.id||'').toLowerCase(); \
           const label = (el.getAttribute('aria-label')||'').toLowerCase(); \
           return placeholder.includes(lowerSel) || name.includes(lowerSel) || id.includes(lowerSel) || label.includes(lowerSel); }}); \
         if (target) {{ target.focus(); target.value = txt; \
           target.dispatchEvent(new Event('input', {{ bubbles: true }})); \
           target.dispatchEvent(new Event('change', {{ bubbles: true }})); return true; }} return false; }})()"
    )
}

/// DOM に `data-yuuka-id` を付与する JS（Node `annotateInteractiveElements` 逐語）。
const ANNOTATE_JS: &str = "(() => { \
  const oldElements = document.querySelectorAll('[data-yuuka-id]'); \
  oldElements.forEach((el) => el.removeAttribute('data-yuuka-id')); \
  const selectors = [\"input:not([type='hidden'])\",'button','select','textarea','a',\"[role='button']\",'[onclick]'].join(','); \
  const elements = Array.from(document.querySelectorAll(selectors)); \
  let idCounter = 1; \
  elements.forEach((el) => { \
    const rect = el.getBoundingClientRect(); \
    const style = window.getComputedStyle(el); \
    const isVisible = rect.width > 0 && rect.height > 0 && style.display !== 'none' && style.visibility !== 'hidden' && style.opacity !== '0'; \
    if (isVisible) { el.setAttribute('data-yuuka-id', String(idCounter++)); } \
  }); })()";

/// 対話要素入り markdown を返す JS（Node `extractPageMarkdown(page, true)` の traverse 逐語・
/// interactive=true 固定）。raw を返し Rust 側で clean_markdown する。
const EXTRACT_INTERACTIVE_JS: &str = "(() => { \
  function isVisible(el){ if(!el || el.nodeType !== Node.ELEMENT_NODE) return true; const style=window.getComputedStyle(el); if(style.display==='none'||style.visibility==='hidden') return false; const rect=el.getBoundingClientRect(); if(rect.width===0&&rect.height===0) return false; return true; } \
  function traverse(node, isPre){ isPre=isPre||false; if(!node) return ''; \
    if(node.nodeType===Node.ELEMENT_NODE && !isVisible(node)) return ''; \
    if(node.nodeType===Node.TEXT_NODE){ const text=node.textContent||''; return isPre?text:text.replace(/\\s+/g,' '); } \
    if(node.nodeType!==Node.ELEMENT_NODE) return ''; \
    const tagName=node.tagName.toLowerCase(); \
    const baseUnwanted=['script','style','noscript','iframe','svg','img','link','meta']; \
    if(baseUnwanted.includes(tagName)) return ''; \
    if(tagName==='pre'||tagName==='code'){ let codeText=''; for(const child of Array.from(node.childNodes)){ codeText+=traverse(child,true); } return tagName==='pre'?('\\n```\\n'+codeText.trim()+'\\n```\\n'):(' `'+codeText.trim()+'` '); } \
    const yuukaId=node.getAttribute('data-yuuka-id')||''; const idStr=yuukaId?(' ID: '+yuukaId):''; \
    if(tagName==='input'){ const type=node.getAttribute('type')||'text'; const name=node.getAttribute('name')||''; const id=node.id||''; const placeholder=node.getAttribute('placeholder')||''; const val=node.value||''; const displayVal=type==='password'?(val?'********':''):val; return ' [Input ('+type+')'+idStr+' id=\"'+id+'\" name=\"'+name+'\" placeholder=\"'+placeholder+'\" value=\"'+displayVal+'\"] '; } \
    if(tagName==='textarea'){ const name=node.getAttribute('name')||''; const id=node.id||''; const placeholder=node.getAttribute('placeholder')||''; const val=node.value||''; return ' [Textarea'+idStr+' id=\"'+id+'\" name=\"'+name+'\" placeholder=\"'+placeholder+'\" value=\"'+val+'\"] '; } \
    if(tagName==='button'){ let btnText=''; for(const child of Array.from(node.childNodes)){ btnText+=traverse(child,isPre); } const id=node.id||''; const name=node.getAttribute('name')||''; return ' [Button'+idStr+': \"'+btnText.trim()+'\" id=\"'+id+'\" name=\"'+name+'\"] '; } \
    if(tagName==='select'){ const name=node.getAttribute('name')||''; const id=node.id||''; const options=Array.from(node.querySelectorAll('option')).map((opt)=>opt.value+':'+((opt.textContent||'').trim())).join(', '); return ' [Select'+idStr+' id=\"'+id+'\" name=\"'+name+'\" Options: {'+options+'}] '; } \
    let childrenText=''; for(const child of Array.from(node.childNodes)){ childrenText+=traverse(child,isPre); } \
    switch(tagName){ \
      case 'h1': return '\\n\\n# '+childrenText.trim()+'\\n\\n'; \
      case 'h2': return '\\n\\n## '+childrenText.trim()+'\\n\\n'; \
      case 'h3': return '\\n\\n### '+childrenText.trim()+'\\n\\n'; \
      case 'h4': case 'h5': case 'h6': return '\\n\\n#### '+childrenText.trim()+'\\n\\n'; \
      case 'p': return '\\n\\n'+childrenText.trim()+'\\n\\n'; \
      case 'br': return '\\n'; \
      case 'hr': return '\\n\\n---\\n\\n'; \
      case 'a': { const href=node.href; const text=childrenText.trim(); const yid=node.getAttribute('data-yuuka-id')||''; const idPrefix=yid?('[ID: '+yid+'] '):''; if(href&&text&&!href.startsWith('javascript:')&&!href.startsWith('mailto:')){ return ' '+idPrefix+'['+text+']('+href+') '; } if(yid&&text){ return ' '+idPrefix+text+' '; } return childrenText; } \
      case 'li': return '\\n- '+childrenText.trim(); \
      case 'ul': case 'ol': return '\\n'+childrenText+'\\n'; \
      case 'th': case 'td': { const cellText=childrenText.replace(/[\\r\\n]+/g,' ').trim(); const compressed=cellText.replace(/\\s+/g,' '); return ' '+compressed+' |'; } \
      case 'tr': return '\\n|'+childrenText; \
      case 'thead': case 'tbody': return childrenText; \
      case 'table': return '\\n\\n'+childrenText+'\\n\\n'; \
      default: { const isBlock=['div','section','article','aside','main','body','blockquote','form'].includes(tagName); return isBlock?('\\n'+childrenText+'\\n'):childrenText; } \
    } } \
  return traverse(document.body); })()";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_selector_maps_ids() {
        assert_eq!(resolve_selector("3"), "[data-yuuka-id=\"3\"]");
        assert_eq!(resolve_selector(" 12 "), "[data-yuuka-id=\"12\"]");
        assert_eq!(resolve_selector("id:7"), "[data-yuuka-id=\"7\"]");
        assert_eq!(resolve_selector("ID:9"), "[data-yuuka-id=\"9\"]");
        assert_eq!(resolve_selector("div.foo"), "div.foo");
        assert_eq!(resolve_selector("#login"), "#login");
    }

    #[test]
    fn parse_contains_extracts_tag_and_text() {
        assert_eq!(
            parse_contains("button:contains(\"送信\")"),
            Some((Some("button".to_owned()), "送信".to_owned()))
        );
        assert_eq!(
            parse_contains(":has-text('ログイン')"),
            Some((None, "ログイン".to_owned()))
        );
        assert_eq!(parse_contains("div.foo"), None);
    }

    #[test]
    fn click_by_text_js_embeds_args_safely() {
        let js = click_by_text_js(Some("button"), "\"; alert(1); //");
        // JSON 埋め込みで壊れない（引用符がエスケープされる）。
        assert!(js.contains("const tag = \"button\""));
        assert!(js.contains(r#"\"; alert(1); //"#));
    }

    #[test]
    fn extract_js_is_an_expression() {
        // IIFE 式（先頭 ( 末尾 )）であること。
        assert!(EXTRACT_INTERACTIVE_JS.trim_start().starts_with("(()"));
        assert!(EXTRACT_INTERACTIVE_JS.trim_end().ends_with(")()"));
        assert!(ANNOTATE_JS.trim_start().starts_with("(()"));
    }
}
