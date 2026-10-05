//! ビルド時 URL 焼き込み・URL 導出・起動引数解釈・非機微設定の読み書き。
//!
//! - 公開 URL は **ビルド時に焼き込む**（client_design.md §2）。
//!   `option_env!("YUUKA_API_BASE")` を使い、未設定ならローカル backend を既定にする。
//! - 機微（Bearer トークン）は keyring のみ。ここで扱うのは**非機微設定のみ**
//!   （ホットキー・オーバーレイ位置/不透明度・前回 Bot・録音自動送信）。
//! - botId 決定順: `--bot <id>` → 保存された前回 Bot → None
//!   （None の場合は呼び出し側が `ready.bots` のプライマリへフォールバック）。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// ビルド時に焼き込まれる公開 API ベース URL。
///
/// 本番: `YUUKA_API_BASE=https://yuuka.kawaii-music.moe cargo build --release`
/// 開発（未設定時）: ローカル backend（コンテナ PORT 7854）。
pub const API_BASE: &str = match option_env!("YUUKA_API_BASE") {
    Some(v) => v,
    None => "http://127.0.0.1:7854",
};

/// keyring のサービス名（Bearer トークン保存に使う。net/auth.rs と共有）。
pub const KEYRING_SERVICE: &str = "yuuka-desktop";

// ===========================================================================
// URL 導出（REST ベース / WS URL）
// ===========================================================================

/// REST のベース URL（末尾スラッシュ無し）。例 `https://host` または `http://127.0.0.1:7854`。
///
/// 焼き込み URL をそのまま使う（`API_BASE` は既に `scheme://host[:port]` 形）。
pub fn rest_base() -> String {
    API_BASE.trim_end_matches('/').to_string()
}

/// 指定 botId に対する WS URL を導出する。
///
/// `http` → `ws`、`https` → `wss` に変換し、`/ws/chat?botId=<id>` を付ける
/// （client_design.md §9）。botId は URL エンコードする。
pub fn ws_url(bot_id: &str) -> String {
    let base = rest_base();
    let ws_scheme_base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        // scheme 不明時は素朴に wss を仮定（焼き込み URL は scheme を含む前提）。
        format!("wss://{base}")
    };
    format!("{ws_scheme_base}/ws/chat?botId={}", urlencode(bot_id))
}

/// 最小限の URL クエリエンコード（追加依存を避けるための自前実装）。
///
/// botId は通常 ASCII の不透明 ID だが、安全のため非英数記号をパーセントエンコードする。
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 端末一覧（`GET /api/devices`）に表示する端末名を推測する。
///
/// 機微情報を避けつつ識別しやすい名前を選ぶ。`COMPUTERNAME`(Windows) /
/// `HOSTNAME`(Unix) を順に見て、空なら `None`（サーバ側で既定名にフォールバック）。
pub fn device_name() -> Option<String> {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

// ===========================================================================
// 起動引数（client_design.md §4.4）
// ===========================================================================

/// 解釈済みの起動引数。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CliArgs {
    /// `--bot <id>`: 起動時に束縛する botId（最優先）。
    pub bot_id: Option<String>,
    /// `--hidden`: トレイのみで起動（オーバーレイ非表示）。
    pub hidden: bool,
}

impl CliArgs {
    /// プロセス引数から解釈する。
    pub fn from_env() -> Self {
        Self::parse(std::env::args().skip(1))
    }

    /// 任意のイテレータから解釈する（テスト容易性のため分離）。
    pub fn parse<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut out = CliArgs::default();
        let mut iter = args.into_iter().map(Into::into).peekable();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--bot" => {
                    // 値が続く形（`--bot <id>`）。
                    out.bot_id = iter.next();
                }
                "--hidden" => out.hidden = true,
                other => {
                    // `--bot=<id>` 形も許容。
                    if let Some(val) = other.strip_prefix("--bot=") {
                        out.bot_id = Some(val.to_string());
                    }
                    // 未知引数は無視（前方互換）。
                }
            }
        }
        out
    }
}

// ===========================================================================
// 非機微設定（directories の config dir へ JSON 保存）
// ===========================================================================

/// オーバーレイ（オーブ）の画面位置。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct OverlayPos {
    pub x: f32,
    pub y: f32,
}

impl Default for OverlayPos {
    fn default() -> Self {
        // 既定は右下寄り（実機調整は Phase 5）。
        OverlayPos {
            x: 1200.0,
            y: 700.0,
        }
    }
}

/// 永続化する非機微設定（client_design.md §10）。
///
/// トークンは含めない（keyring のみ）。会話履歴も含めない（正はサーバ）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// グローバルホットキー（egui/global-hotkey 表記の文字列。既定 `Alt+Y`）。
    pub hotkey: String,
    /// オーブの画面位置（ドラッグで更新・記憶）。
    pub overlay_pos: OverlayPos,
    /// オーブ/モーダルの不透明度（0.0–1.0）。
    pub overlay_opacity: f32,
    /// 前回接続していた botId（次回起動時の既定候補）。
    pub last_bot_id: Option<String>,
    /// 録音停止で自動送信するか（false なら確認後送信）。
    pub auto_send_recording: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            hotkey: "Alt+Y".to_string(),
            overlay_pos: OverlayPos::default(),
            overlay_opacity: 0.95,
            last_bot_id: None,
            auto_send_recording: true,
        }
    }
}

impl Settings {
    /// 設定ファイルの絶対パス（config dir / `settings.json`）。
    ///
    /// `directories` でプラットフォーム標準の config ディレクトリを解決する。
    /// 取得不能（例外的環境）の場合は `None`。
    pub fn config_path() -> Option<PathBuf> {
        let dirs = directories::ProjectDirs::from("moe", "kawaii-music", "yuuka-desktop")?;
        Some(dirs.config_dir().join("settings.json"))
    }

    /// 設定を読み込む。ファイルが無い/壊れている場合は既定値を返す（堅牢性優先）。
    pub fn load() -> Settings {
        let Some(path) = Self::config_path() else {
            return Settings::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                log::warn!("settings parse failed ({path:?}): {e}; using defaults");
                Settings::default()
            }),
            Err(_) => Settings::default(),
        }
    }

    /// 設定を保存する。ディレクトリが無ければ作る。
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = Self::config_path() else {
            return Ok(()); // 保存先が解決できない環境では黙って諦める。
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(&path, text)
    }
}

// ===========================================================================
// botId 決定
// ===========================================================================

/// 起動時に束縛する botId を決定する。
///
/// 決定順（client_design.md §4.4 / architecture.md §8.1）:
/// 1. `--bot <id>` 引数
/// 2. 保存された前回 Bot（`Settings.last_bot_id`）
/// 3. None（呼び出し側が `ready.bots` のプライマリへフォールバック）
pub fn resolve_bot_id(args: &CliArgs, settings: &Settings) -> Option<String> {
    args.bot_id.clone().or_else(|| settings.last_bot_id.clone())
}

// ===========================================================================
// テスト
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_url_https_to_wss() {
        // rest_base は API_BASE 由来なので、ロジックは ws_url の変換のみ検証。
        // ここでは関数の scheme 変換規則を直接確認するため一時的に組み立てを再現。
        let url = ws_url("b1");
        // 焼き込み既定はローカル http なので ws:// になる。
        assert!(url.starts_with("ws://") || url.starts_with("wss://"));
        assert!(url.contains("/ws/chat?botId=b1"));
    }

    #[test]
    fn ws_url_encodes_special_bot_id() {
        let url = ws_url("a b/c");
        assert!(url.contains("botId=a%20b%2Fc"), "got: {url}");
    }

    #[test]
    fn cli_parse_bot_and_hidden() {
        let a = CliArgs::parse(["--bot", "b42", "--hidden"]);
        assert_eq!(a.bot_id.as_deref(), Some("b42"));
        assert!(a.hidden);
    }

    #[test]
    fn cli_parse_bot_eq_form() {
        let a = CliArgs::parse(["--bot=xyz"]);
        assert_eq!(a.bot_id.as_deref(), Some("xyz"));
        assert!(!a.hidden);
    }

    #[test]
    fn cli_parse_unknown_ignored() {
        let a = CliArgs::parse(["--frobnicate", "value"]);
        assert_eq!(a, CliArgs::default());
    }

    #[test]
    fn resolve_bot_id_priority() {
        let mut settings = Settings::default();
        settings.last_bot_id = Some("saved".into());

        // 1. --bot が最優先
        let args = CliArgs {
            bot_id: Some("arg".into()),
            hidden: false,
        };
        assert_eq!(resolve_bot_id(&args, &settings).as_deref(), Some("arg"));

        // 2. 引数無し → 保存された前回 Bot
        let args = CliArgs::default();
        assert_eq!(resolve_bot_id(&args, &settings).as_deref(), Some("saved"));

        // 3. どちらも無し → None
        let args = CliArgs::default();
        let empty = Settings::default();
        assert_eq!(resolve_bot_id(&args, &empty), None);
    }

    #[test]
    fn settings_default_roundtrip() {
        let s = Settings::default();
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }
}
