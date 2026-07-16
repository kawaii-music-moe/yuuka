//! chromium CLI 連携（Node `src/rust_crawler` の fetch-js/screenshot 流用）。
//!
//! CDP を使わず chromium バイナリを `--dump-dom`（JS レンダリング済み DOM 取得）/`--screenshot`
//! （全画面キャプチャ）で 1 発起動する。interactive セッションは別途（後続増分・要 CDP）。

use std::path::{Path, PathBuf};

/// chromium/chrome 実行ファイルを探す（Node `findChromeExecutable`）。
///
/// `CHROME_EXECUTABLE_PATH` → 既知パス → puppeteer キャッシュ の順。
///
/// # Errors
/// どこにも見つからない場合。
pub fn find_chrome() -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var("CHROME_EXECUTABLE_PATH") {
        let p = PathBuf::from(&path);
        if p.exists() {
            return Ok(p);
        }
    }
    const COMMON: &[&str] = &[
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium-browser",
        "/usr/bin/chromium",
        "/usr/local/bin/google-chrome",
        "/snap/bin/chromium",
        "/usr/bin/google-chrome-beta",
    ];
    for path in COMMON {
        let p = Path::new(path);
        if p.exists() {
            return Ok(p.to_path_buf());
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let base = Path::new(&home).join(".cache/puppeteer/chrome");
        if let Ok(entries) = std::fs::read_dir(&base) {
            for entry in entries.flatten() {
                for suffix in ["chrome-linux64/chrome", "chrome-linux/chrome"] {
                    let candidate = entry.path().join(suffix);
                    if candidate.exists() {
                        return Ok(candidate);
                    }
                }
            }
        }
    }
    Err("Chrome実行ファイルが見つかりません。CHROME_EXECUTABLE_PATH環境変数を設定するか、Chromeをインストールしてください。".to_owned())
}

/// `--dump-dom` で JS レンダリング済み HTML を取得する（Node crawler `fetchJsToString`）。
///
/// # Errors
/// chrome 不在・起動失敗・空応答時。
pub async fn dump_dom(url: &str) -> Result<String, String> {
    let chrome = find_chrome()?;
    let output = tokio::process::Command::new(&chrome)
        .args([
            "--headless",
            "--disable-gpu",
            "--no-sandbox",
            "--disable-dev-shm-usage",
            "--dump-dom",
            "--timeout=12000",
            url,
        ])
        .output()
        .await
        .map_err(|e| format!("Chrome 起動に失敗しました: {e}"))?;

    let html = String::from_utf8_lossy(&output.stdout).into_owned();
    if html.trim().is_empty() {
        return Err("Chrome dump-dom が空のコンテンツを返しました。".to_owned());
    }
    Ok(html)
}

/// `--screenshot` で全画面キャプチャを撮り `out_path` へ保存する（Node crawler `runScreenshot`）。
///
/// # Errors
/// chrome 不在・起動失敗・画像未生成時。
pub async fn screenshot(url: &str, out_path: &Path) -> Result<(), String> {
    let chrome = find_chrome()?;

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let temp_dir = std::env::temp_dir().join(format!("yuuka-screenshot-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;

    let output = tokio::process::Command::new(&chrome)
        .args([
            "--headless",
            "--disable-gpu",
            "--no-sandbox",
            "--disable-dev-shm-usage",
            "--disable-software-rasterizer",
            "--hide-scrollbars",
            "--window-size=1280,800",
            "--screenshot",
            url,
        ])
        .current_dir(&temp_dir)
        .output()
        .await
        .map_err(|e| format!("Chrome 起動に失敗しました: {e}"))?;

    let produced = temp_dir.join("screenshot.png");
    if produced.exists() {
        let copy = std::fs::copy(&produced, out_path).map(|_| ());
        std::fs::remove_dir_all(&temp_dir).ok();
        copy.map_err(|e| e.to_string())
    } else {
        std::fs::remove_dir_all(&temp_dir).ok();
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "スクリーンショットファイルが生成されませんでした。Chrome出力:\n{stderr}"
        ))
    }
}
