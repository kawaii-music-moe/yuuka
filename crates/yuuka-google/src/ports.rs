//! Google 外部サブシステムのシーム（OAuth / Calendar / Drive バックアップ）と OAuth state ストア。
//!
//! Node は `googleapis` / `googleCalendarService` / `backupService` を直接呼ぶが、本移植ではこれらの
//! HTTP サブシステムを**ポート（trait）越し**にし、gateway 未配線時は [`NullGoogleOAuth`] /
//! [`NullCalendar`] / [`NullBackup`] へ縮退する（admin の `BotRuntime` シームと同方針）。**DB 効果
//! （アカウント行の作成/削除・primary 付替え・カレンダー列更新）は常に完全に働く**。実 HTTP 実装を
//! 注入すれば live 化する。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Serialize;

/// カレンダーの最小サマリ（`/api/status` と integrated のカレンダー一覧で返す形）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CalendarSummary {
    pub id: String,
    pub summary: String,
}

/// OAuth トークン交換の結果（refresh_token は再同意が無いと来ないため `Option`）。
#[derive(Debug, Clone, Default)]
pub struct GoogleTokens {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
}

/// Google サブシステム呼び出しの失敗（内部詳細は握り潰し、ルートは固定メッセージへ写像）。
#[derive(Debug, Clone)]
pub enum GoogleError {
    /// システムに Google OAuth2 クライアント設定が無い（`isGoogleOAuthConfigured=false`）。
    NotConfigured,
    /// 上流 HTTP / トークン交換 / ファイル I/O の失敗。
    Upstream(String),
}

/// Google OAuth2 認可コードフローのポート（認可 URL 生成・トークン交換・email 取得）。
#[async_trait]
pub trait GoogleOAuthPort: Send + Sync {
    /// システムに OAuth2 クライアント設定（ID/Secret）が登録されているか。
    fn is_configured(&self) -> bool;
    /// 同意画面の認可 URL を生成する（`access_type=offline` / `prompt=consent` / スコープ + state）。
    fn auth_url(&self, redirect_uri: &str, state: &str) -> String;
    /// 認可コードをトークンへ交換する。
    async fn exchange_code(
        &self,
        redirect_uri: &str,
        code: &str,
    ) -> Result<GoogleTokens, GoogleError>;
    /// アクセストークンからユーザーの email を取得する（失敗は `None`・Node は握り潰す）。
    async fn fetch_email(&self, access_token: &str) -> Option<String>;
}

/// gateway 未配線時の OAuth 縮退（未設定扱い）。
pub struct NullGoogleOAuth;

#[async_trait]
impl GoogleOAuthPort for NullGoogleOAuth {
    fn is_configured(&self) -> bool {
        false
    }
    fn auth_url(&self, _redirect_uri: &str, _state: &str) -> String {
        String::new()
    }
    async fn exchange_code(
        &self,
        _redirect_uri: &str,
        _code: &str,
    ) -> Result<GoogleTokens, GoogleError> {
        Err(GoogleError::NotConfigured)
    }
    async fn fetch_email(&self, _access_token: &str) -> Option<String> {
        None
    }
}

/// Google Calendar のポート（キャッシュ済み一覧の取得 + アカウント別一覧 + キャッシュ無効化）。
#[async_trait]
pub trait CalendarPort: Send + Sync {
    /// ユーザーの primary アカウント基準のキャッシュ済みカレンダー一覧（`/api/status`）。
    async fn cached_calendars(&self, user_id: &str) -> Vec<CalendarSummary>;
    /// 特定アカウントのカレンダー一覧を取得する（integrated `GET :id/calendars`）。
    async fn list_for_account(
        &self,
        user_id: &str,
        account_id: i64,
    ) -> Result<Vec<CalendarSummary>, GoogleError>;
    /// ユーザー単位のカレンダーキャッシュを無効化する。
    fn invalidate_user(&self, user_id: &str);
    /// アカウント単位のカレンダーキャッシュを無効化する。
    fn invalidate_account(&self, account_id: i64);
}

/// gateway 未配線時の Calendar 縮退（空一覧・キャッシュ無効化は no-op）。
pub struct NullCalendar;

#[async_trait]
impl CalendarPort for NullCalendar {
    async fn cached_calendars(&self, _user_id: &str) -> Vec<CalendarSummary> {
        Vec::new()
    }
    async fn list_for_account(
        &self,
        _user_id: &str,
        _account_id: i64,
    ) -> Result<Vec<CalendarSummary>, GoogleError> {
        Ok(Vec::new())
    }
    fn invalidate_user(&self, _user_id: &str) {}
    fn invalidate_account(&self, _account_id: i64) {}
}

/// 手動バックアップ（Google Drive アップロード）のポート。成功時は Drive のファイル URL を返す。
#[async_trait]
pub trait BackupPort: Send + Sync {
    /// ユーザーのデータを Drive へバックアップし、ファイル URL を返す（Node `runBackup`）。
    async fn run_backup(&self, user_id: &str) -> Result<String, GoogleError>;
}

/// gateway 未配線時のバックアップ縮退（常に失敗＝Node の Google 未連携時 500 と一致）。
pub struct NullBackup;

#[async_trait]
impl BackupPort for NullBackup {
    async fn run_backup(&self, _user_id: &str) -> Result<String, GoogleError> {
        Err(GoogleError::NotConfigured)
    }
}

/// OAuth の CSRF 対策 state nonce ストア（Node `oauthStateStore` の in-memory フォールバック相当）。
///
/// 一回限りの nonce をセッションユーザーへ束縛して発行し、コールバックで消費して照合する。TTL 経過で
/// 掃除される。web 再起動を跨ぐよう main で 1 度生成し `Extension` 注入する（`DeviceAuthStore` 同方式）。
pub struct OAuthStateStore {
    state: Mutex<HashMap<String, (String, Instant)>>,
    ttl: Duration,
}

impl Default for OAuthStateStore {
    fn default() -> Self {
        Self::new()
    }
}

impl OAuthStateStore {
    /// 既定 TTL 10 分の空ストアを作る。
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(HashMap::new()),
            ttl: Duration::from_secs(600),
        }
    }

    /// セッションユーザーに束縛した一回限りの state nonce を発行する。
    ///
    /// # Errors
    /// CSPRNG 失敗時 [`getrandom::Error`]。
    pub fn create(&self, user_id: &str) -> Result<String, getrandom::Error> {
        let mut bytes = [0u8; 32];
        getrandom::getrandom(&mut bytes)?;
        let nonce = hex::encode(bytes);
        let now = Instant::now();
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.retain(|_, (_, exp)| *exp > now);
        guard.insert(nonce.clone(), (user_id.to_owned(), now + self.ttl));
        Ok(nonce)
    }

    /// state を消費して束縛ユーザー ID を返す（一回限り・不在/期限切れは `None`）。
    #[must_use]
    pub fn consume(&self, nonce: &str) -> Option<String> {
        if nonce.is_empty() {
            return None;
        }
        let now = Instant::now();
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.retain(|_, (_, exp)| *exp > now);
        guard.remove(nonce).map(|(user_id, _)| user_id)
    }
}

#[cfg(test)]
mod tests {
    use super::OAuthStateStore;

    #[test]
    fn state_is_single_use_and_user_bound() {
        let store = OAuthStateStore::new();
        let nonce = store.create("alice").expect("nonce");
        // 消費で束縛ユーザーが返る。
        assert_eq!(store.consume(&nonce).as_deref(), Some("alice"));
        // 二度目は不在（一回限り）。
        assert_eq!(store.consume(&nonce), None);
        // 空文字は常に None。
        assert_eq!(store.consume(""), None);
    }
}
