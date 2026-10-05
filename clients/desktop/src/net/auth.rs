//! OAuth デバイスフロー（RFC 8628 型）クライアント + keyring トークン保存。
//!
//! 仕様: backend_api.md §1 / client_design.md §8。
//!
//! ```text
//! 1. POST /api/auth/device/code         → user_code / verification_uri_complete / interval
//! 2. open(verification_uri_complete)    → 既定ブラウザでログイン&承認
//! 3. loop POST /api/auth/device/token   → authorization_pending / slow_down / approved
//! 4. keyring へ access_token を保存       → 以後 WS/REST で Bearer
//! ```
//!
//! 機微なトークンは keyring（OS 資格情報ストア）のみに保存し、平文ファイルへは
//! 書かない（NFR-4）。

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::{rest_base, KEYRING_SERVICE};

/// keyring エントリのユーザー名（サービスは [`KEYRING_SERVICE`]）。
const KEYRING_TOKEN_USER: &str = "bearer-token";

/// デバイスフロー進捗を UI へ報告するコールバック。
///
/// `ui/login.rs` がこれを実装し、user_code・verification_uri・ポーリング状況を
/// 画面に出す（client_design.md §8「ログイン中 UI」）。同期コールバックにして
/// 呼び出し側（tokio タスク）から手軽に叩けるようにする。
pub trait LoginProgress: Send {
    /// `device/code` 取得直後。ユーザーに見せる情報。
    fn on_code(&mut self, user_code: &str, verification_uri_complete: &str);
    /// ポーリング中（承認待ち）。UI のスピナー継続に使う。
    fn on_pending(&mut self) {}
}

/// 何もしない `LoginProgress`（テスト/ヘッドレス用）。
pub struct NoopProgress;
impl LoginProgress for NoopProgress {
    fn on_code(&mut self, _user_code: &str, _uri: &str) {}
}

/// デバイスフロー / トークン保存のエラー。
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("device code expired before approval")]
    Expired,
    #[error("server returned an error: {0}")]
    Server(String),
    #[error("keyring error: {0}")]
    Keyring(#[from] keyring::Error),
}

// ===========================================================================
// ワイヤ型（backend_api.md §1）
// ===========================================================================

#[derive(Debug, Serialize)]
struct DeviceCodeReq<'a> {
    client: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_name: Option<String>,
}

/// `POST /api/auth/device/code` のレスポンス（backend_api.md §1.1）。
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCodeResp {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    /// ポーリング最小間隔（秒）。
    pub interval: u64,
    /// 失効までの秒数。
    pub expires_in: u64,
}

#[derive(Debug, Serialize)]
struct TokenReq<'a> {
    device_code: &'a str,
}

/// `POST /api/auth/device/token` の成功/保留レスポンス（backend_api.md §1.3）。
///
/// 承認済みは `access_token` を、保留/失効は `error` を返すユニオン形なので、
/// 両方を任意フィールドとして受ける。
#[derive(Debug, Clone, Deserialize)]
struct TokenResp {
    /// `authorization_pending` / `slow_down` / `expired_token` 等。
    error: Option<String>,
    /// 承認済み時のみ。
    access_token: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    token_type: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    user: Option<crate::model::UserInfo>,
}

/// URL から `scheme://host[:port]`（オリジン）を取り出す。簡易パーサ
/// （`https://h/p?x` → `https://h`）。解釈できなければ `None`。
fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    if host.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{host}"))
}

// ===========================================================================
// デバイスフロー本体
// ===========================================================================

/// デバイスフローを最後まで実行し、access_token を返す。
///
/// `progress` に user_code 等を通知しつつ、`interval` を尊重してポーリングする。
/// `slow_down` で間隔を増やし、`authorization_pending` は継続、`expired_token` で
/// [`AuthError::Expired`]。承認されたらトークンを keyring に保存して返す。
pub async fn run_device_flow(
    client: &reqwest::Client,
    device_name: Option<String>,
    progress: &mut dyn LoginProgress,
) -> Result<String, AuthError> {
    let base = rest_base();

    // 1) device/code
    let code: DeviceCodeResp = client
        .post(format!("{base}/api/auth/device/code"))
        .json(&DeviceCodeReq {
            client: "desktop",
            device_name,
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    progress.on_code(&code.user_code, &code.verification_uri_complete);

    // 承認先（verification_uri = サーバの BASE_URL 由来）とポーリング先（api_base）の
    // オリジンが食い違うと、ユーザーは別バックエンドで承認し、こちらは永遠に
    // authorization_pending を引き続ける（=「承認しても進まない」）。設定不整合として警告する。
    if let (Some(v), Some(b)) = (origin_of(&code.verification_uri), origin_of(&base)) {
        if !v.eq_ignore_ascii_case(&b) {
            log::warn!(
                "verification origin ({v}) != api_base origin ({b}); \
                 ブラウザの承認先とアプリのポーリング先が別バックエンドです。\
                 配布ビルドの YUUKA_API_BASE とサーバの BASE_URL を一致させてください。"
            );
        }
    }

    // 2) 既定ブラウザで承認ページを開く（失敗しても致命ではない — UI に URL を出す）。
    if let Err(e) = open::that_detached(&code.verification_uri_complete) {
        log::warn!("failed to open browser for verification: {e}");
    }

    // 3) ポーリング（interval 尊重・slow_down で増加）。
    let mut interval = Duration::from_secs(code.interval.max(1));
    loop {
        tokio::time::sleep(interval).await;

        let resp: TokenResp = client
            .post(format!("{base}/api/auth/device/token"))
            .json(&TokenReq {
                device_code: &code.device_code,
            })
            // 410/202 等の非 2xx も JSON 本体を読むため error_for_status は使わない。
            .send()
            .await?
            .json()
            .await?;

        if let Some(token) = resp.access_token {
            // 4) 承認済み → keyring 保存して返す。
            save_token(&token)?;
            return Ok(token);
        }

        match resp.error.as_deref() {
            Some("authorization_pending") => {
                progress.on_pending();
                // 継続。
            }
            Some("slow_down") => {
                // interval を増やす（RFC 8628 推奨: +5s）。
                interval += Duration::from_secs(5);
                progress.on_pending();
            }
            Some("expired_token") => return Err(AuthError::Expired),
            Some(other) => return Err(AuthError::Server(other.to_string())),
            None => {
                // access_token も error も無い不正応答。
                return Err(AuthError::Server("malformed token response".into()));
            }
        }
    }
}

// ===========================================================================
// keyring（トークンの保存/読込/削除）
// ===========================================================================

fn entry() -> Result<keyring::Entry, keyring::Error> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_TOKEN_USER)
}

/// Bearer トークンを OS 資格情報ストアへ保存する。
pub fn save_token(token: &str) -> Result<(), AuthError> {
    entry()?.set_password(token)?;
    Ok(())
}

/// 保存済み Bearer トークンを読み込む。未保存なら `Ok(None)`。
pub fn load_token() -> Result<Option<String>, AuthError> {
    match entry()?.get_password() {
        Ok(t) => Ok(Some(t)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// 保存済みトークンを破棄する（ログアウト / 401 時）。未保存でもエラーにしない。
pub fn delete_token() -> Result<(), AuthError> {
    match entry()?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}
