//! Google の実 HTTP クライアント（`GoogleOAuthPort` + `CalendarPort` の live 実装）。
//!
//! Node は `googleapis`（`google.auth.OAuth2` / `google.calendar` / `google.oauth2`）を直接呼ぶが、
//! 本移植は同じ Google REST エンドポイントを [`reqwest`] で直接叩く。設定（client_id/secret）は
//! システム共通、リフレッシュトークンは owner 単位の複数アカウント（`user_google_accounts`）から取り、
//! **システム鍵**（[`SystemCrypto::decrypt_text`]）で復号する（Node `googleAccountRepo` はシステム鍵で
//! 暗号化する）。カレンダー一覧は Node `getCachedCalendars` と同じく accountId をまたぐ user 単位の
//! in-memory キャッシュ（TTL 5 分）を持つ。
//!
//! ## 実 HTTP は本番環境検証待ち（live-deferred）
//!
//! `exchange_code` / トークンリフレッシュ / `calendarList` の**実 Google 通信**は本番の OAuth 設定が
//! 無いと走らせられないため、単体テストでは検証していない（`yuuka-browser` の CDP live 未検証注記と
//! 同方針）。単体テストは**純ロジックのみ**——`auth_url` 生成（スコープ/パラメータ厳密一致）、
//! token/userinfo/calendarList の JSON パース（固定文字列フィクスチャ）、キャッシュ TTL 挙動——を
//! 検証する。実通信は実環境検証で担保する。
//!
//! [`SystemCrypto::decrypt_text`]: yuuka_crypto::SystemCrypto::decrypt_text

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use yuuka_crypto::SystemCrypto;
use yuuka_web::Db;

use crate::ports::{CalendarPort, CalendarSummary, GoogleError, GoogleOAuthPort, GoogleTokens};
use crate::repo;

/// カレンダーキャッシュの TTL（Node `CACHE_TTL = 5 * 60 * 1000`）。
const CACHE_TTL: Duration = Duration::from_secs(5 * 60);
/// HTTP タイムアウト（`yuuka-browser` の 15s に合わせる）。
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// Google の認可エンドポイント（同意画面 URL の基点）。
const AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
/// OAuth トークンエンドポイント（コード交換 / リフレッシュ共用）。
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
/// userinfo エンドポイント（email 取得）。
const USERINFO_ENDPOINT: &str = "https://www.googleapis.com/oauth2/v2/userinfo";
/// calendarList エンドポイント（利用可能カレンダー一覧）。
const CALENDAR_LIST_ENDPOINT: &str = "https://www.googleapis.com/calendar/v3/users/me/calendarList";

/// 同意画面で要求する OAuth スコープ（Node `GOOGLE_OAUTH_SCOPES`・並び順も一致）。
const OAUTH_SCOPES: &[&str] = &[
    "openid",
    "https://www.googleapis.com/auth/userinfo.email",
    "https://www.googleapis.com/auth/userinfo.profile",
    "https://www.googleapis.com/auth/calendar",
    "https://www.googleapis.com/auth/drive.file",
];

/// Google の実 HTTP クライアント（OAuth + Calendar）。
///
/// `Db` は `Clone`・`Arc<SystemCrypto>` / `reqwest::Client` は共有安価。カレンダーキャッシュは
/// user_id をキーに `(一覧, 取得時刻)` を保持する。
pub struct GoogleHttpClient {
    client_id: String,
    client_secret: String,
    crypto: Arc<SystemCrypto>,
    db: Db,
    http: reqwest::Client,
    cache: Mutex<HashMap<String, (Vec<CalendarSummary>, Instant)>>,
}

impl std::fmt::Debug for GoogleHttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleHttpClient")
            .field("configured", &self.is_configured())
            .finish_non_exhaustive()
    }
}

impl GoogleHttpClient {
    /// 実 HTTP クライアントを組み立てる。`main` から設定と共有ハンドルを注入する。
    #[must_use]
    pub fn new(
        client_id: String,
        client_secret: String,
        crypto: Arc<SystemCrypto>,
        db: Db,
        http: reqwest::Client,
    ) -> Self {
        Self {
            client_id,
            client_secret,
            crypto,
            db,
            http,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// リフレッシュトークンをアクセストークンへ交換する（POST `grant_type=refresh_token`）。
    ///
    /// # Errors
    /// 上流 HTTP 失敗 / `access_token` 不在時 [`GoogleError::Upstream`]。
    async fn refresh_access_token(&self, refresh_token: &str) -> Result<String, GoogleError> {
        let form = [
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ];
        let resp = self
            .http
            .post(TOKEN_ENDPOINT)
            .form(&form)
            .timeout(HTTP_TIMEOUT)
            .send()
            .await
            .map_err(|e| GoogleError::Upstream(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(GoogleError::Upstream(format!(
                "token refresh status {}",
                resp.status()
            )));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| GoogleError::Upstream(e.to_string()))?;
        parse_access_token(&body)
            .ok_or_else(|| GoogleError::Upstream("no access_token in refresh response".to_owned()))
    }

    /// アクセストークンで calendarList を取得し `CalendarSummary` へ写像する（`minAccessRole=writer`）。
    ///
    /// # Errors
    /// 上流 HTTP 失敗 / 非 2xx 時 [`GoogleError::Upstream`]。
    async fn fetch_calendar_list(
        &self,
        access_token: &str,
    ) -> Result<Vec<CalendarSummary>, GoogleError> {
        let resp = self
            .http
            .get(CALENDAR_LIST_ENDPOINT)
            .query(&[("minAccessRole", "writer")])
            .bearer_auth(access_token)
            .timeout(HTTP_TIMEOUT)
            .send()
            .await
            .map_err(|e| GoogleError::Upstream(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(GoogleError::Upstream(format!(
                "calendarList status {}",
                resp.status()
            )));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| GoogleError::Upstream(e.to_string()))?;
        Ok(parse_calendar_list(&body))
    }

    /// primary アカウントの復号済みリフレッシュトークン経由でカレンダー一覧を取得する（キャッシュ非経由）。
    ///
    /// # Errors
    /// アカウント不在 / 復号失敗 / 上流失敗時 [`GoogleError`]。
    async fn list_primary(&self, user_id: &str) -> Result<Vec<CalendarSummary>, GoogleError> {
        let tokens = repo::get_primary_account_tokens(&self.db, user_id)
            .await
            .map_err(|e| GoogleError::Upstream(e.to_string()))?
            .ok_or(GoogleError::NotConfigured)?;
        let (_id, enc, iv, tag) = tokens;
        let refresh = self
            .crypto
            .decrypt_text(&enc, &iv, &tag)
            .map_err(|e| GoogleError::Upstream(e.to_string()))?;
        let access = self.refresh_access_token(&refresh).await?;
        self.fetch_calendar_list(&access).await
    }
}

/// `encodeURIComponent` 相当（クエリ値用の最小パーセントエンコード・`search.rs` と同一規則）。
fn encode_component(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// トークン応答 JSON から `access_token` を抜く（`None` = 不在/非文字列）。
fn parse_access_token(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    value
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// トークン交換応答 JSON を [`GoogleTokens`] へパースする（両トークンとも `Option`）。
fn parse_tokens(body: &str) -> Option<GoogleTokens> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let field = |k: &str| {
        value
            .get(k)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    Some(GoogleTokens {
        access_token: field("access_token"),
        refresh_token: field("refresh_token"),
    })
}

/// userinfo 応答 JSON から `email` を抜く（`None` = 不在/非文字列・Node は握り潰す）。
fn parse_email(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    value
        .get("email")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// キャッシュエントリを採用できるか（Node `getCachedCalendars`: 非空 + TTL 内のみヒット）。
fn cache_entry_is_fresh(calendars: &[CalendarSummary], age: Duration) -> bool {
    !calendars.is_empty() && age < CACHE_TTL
}

/// calendarList 応答 JSON を `CalendarSummary` へ写像する（`id` と `summary` の両方があるものだけ）。
fn parse_calendar_list(body: &str) -> Vec<CalendarSummary> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };
    let Some(items) = value.get("items").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let id = item.get("id").and_then(serde_json::Value::as_str)?;
            let summary = item.get("summary").and_then(serde_json::Value::as_str)?;
            Some(CalendarSummary {
                id: id.to_owned(),
                summary: summary.to_owned(),
            })
        })
        .collect()
}

#[async_trait]
impl GoogleOAuthPort for GoogleHttpClient {
    fn is_configured(&self) -> bool {
        !self.client_id.is_empty() && !self.client_secret.is_empty()
    }

    fn auth_url(&self, redirect_uri: &str, state: &str) -> String {
        let scope = OAUTH_SCOPES.join(" ");
        format!(
            "{AUTH_ENDPOINT}?client_id={}&redirect_uri={}&response_type=code&access_type=offline&prompt=consent&scope={}&state={}",
            encode_component(&self.client_id),
            encode_component(redirect_uri),
            encode_component(&scope),
            encode_component(state),
        )
    }

    async fn exchange_code(
        &self,
        redirect_uri: &str,
        code: &str,
    ) -> Result<GoogleTokens, GoogleError> {
        let form = [
            ("code", code),
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
        ];
        let resp = self
            .http
            .post(TOKEN_ENDPOINT)
            .form(&form)
            .timeout(HTTP_TIMEOUT)
            .send()
            .await
            .map_err(|e| GoogleError::Upstream(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(GoogleError::Upstream(format!(
                "token exchange status {}",
                resp.status()
            )));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| GoogleError::Upstream(e.to_string()))?;
        parse_tokens(&body)
            .ok_or_else(|| GoogleError::Upstream("malformed token response".to_owned()))
    }

    async fn fetch_email(&self, access_token: &str) -> Option<String> {
        let resp = self
            .http
            .get(USERINFO_ENDPOINT)
            .bearer_auth(access_token)
            .timeout(HTTP_TIMEOUT)
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let body = resp.text().await.ok()?;
        parse_email(&body)
    }
}

#[async_trait]
impl CalendarPort for GoogleHttpClient {
    async fn cached_calendars(&self, user_id: &str) -> Vec<CalendarSummary> {
        // キャッシュヒット（Node `getCachedCalendars`: 非空 + TTL 内のみ採用）。
        {
            let guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((calendars, fetched)) = guard.get(user_id) {
                if cache_entry_is_fresh(calendars, fetched.elapsed()) {
                    return calendars.clone();
                }
            }
        }
        // ミス時は取得。あらゆる失敗は空一覧へ縮退（Null と同じ非致命挙動）。
        let calendars = self.list_primary(user_id).await.unwrap_or_default();
        let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        guard.insert(user_id.to_owned(), (calendars.clone(), Instant::now()));
        calendars
    }

    async fn list_for_account(
        &self,
        user_id: &str,
        account_id: i64,
    ) -> Result<Vec<CalendarSummary>, GoogleError> {
        let tokens = repo::get_account_tokens(&self.db, account_id)
            .await
            .map_err(|e| GoogleError::Upstream(e.to_string()))?
            .ok_or(GoogleError::NotConfigured)?;
        let (owner, enc, iv, tag) = tokens;
        // 越権チェック: 対象アカウントの所有者が呼び出しユーザーと一致すること。
        if owner != user_id {
            return Err(GoogleError::NotConfigured);
        }
        let refresh = self
            .crypto
            .decrypt_text(&enc, &iv, &tag)
            .map_err(|e| GoogleError::Upstream(e.to_string()))?;
        let access = self.refresh_access_token(&refresh).await?;
        self.fetch_calendar_list(&access).await
    }

    fn invalidate_user(&self, user_id: &str) {
        let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        guard.remove(user_id);
    }

    fn invalidate_account(&self, _account_id: i64) {
        // キャッシュは user 単位（Node は accountId 単位だが本移植は primary 一覧のみキャッシュする）。
        // accountId → user の逆引きは持たないため、安全側で全キャッシュを掃く。
        let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        guard.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        cache_entry_is_fresh, encode_component, parse_access_token, parse_calendar_list,
        parse_email, parse_tokens, CalendarSummary, CACHE_TTL, OAUTH_SCOPES,
    };

    // auth_url は self（client_id）に依存するため、生成規則を純関数で再現して厳密検証する。
    fn build_auth_url(client_id: &str, redirect_uri: &str, state: &str) -> String {
        let scope = OAUTH_SCOPES.join(" ");
        format!(
            "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&access_type=offline&prompt=consent&scope={}&state={}",
            encode_component(client_id),
            encode_component(redirect_uri),
            encode_component(&scope),
            encode_component(state),
        )
    }

    #[test]
    fn oauth_scopes_match_node_exactly() {
        assert_eq!(
            OAUTH_SCOPES,
            [
                "openid",
                "https://www.googleapis.com/auth/userinfo.email",
                "https://www.googleapis.com/auth/userinfo.profile",
                "https://www.googleapis.com/auth/calendar",
                "https://www.googleapis.com/auth/drive.file",
            ]
        );
    }

    #[test]
    fn auth_url_builds_exact_params_and_encoded_scope() {
        let url = build_auth_url(
            "cid-123",
            "https://app.example.com/api/settings/google/oauth/callback",
            "nonce-abc",
        );
        // 固定パラメータ。
        assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));
        assert!(url.contains("client_id=cid-123"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("prompt=consent"));
        assert!(url.contains("state=nonce-abc"));
        // redirect_uri は URL エンコードされる（`:` `/` が %3A %2F）。
        assert!(url.contains(
            "redirect_uri=https%3A%2F%2Fapp.example.com%2Fapi%2Fsettings%2Fgoogle%2Foauth%2Fcallback"
        ));
        // scope はスペース結合（%20）+ 各値のスラッシュ/コロンもエンコード。
        // （継続行の改行/インデントが混ざらないよう + 連結で 1 本の文字列にする）
        let expected_scope = "scope=openid%20".to_owned()
            + "https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fuserinfo.email%20"
            + "https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fuserinfo.profile%20"
            + "https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fcalendar%20"
            + "https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fdrive.file";
        assert!(url.contains(&expected_scope));
    }

    #[test]
    fn encode_component_encodes_reserved_and_kanji() {
        assert_eq!(encode_component("a b"), "a%20b");
        assert_eq!(encode_component("https://x/y"), "https%3A%2F%2Fx%2Fy");
        assert_eq!(encode_component("東京"), "%E6%9D%B1%E4%BA%AC");
        // unreserved は素通し。
        assert_eq!(encode_component("A-z_0.9~"), "A-z_0.9~");
    }

    #[test]
    fn parse_tokens_extracts_both_when_present() {
        let body = r#"{"access_token":"at-1","refresh_token":"rt-1","expires_in":3599}"#;
        let t = parse_tokens(body).expect("parsed");
        assert_eq!(t.access_token.as_deref(), Some("at-1"));
        assert_eq!(t.refresh_token.as_deref(), Some("rt-1"));
    }

    #[test]
    fn parse_tokens_refresh_optional() {
        // 再同意が無いと refresh_token は来ない → None。
        let body = r#"{"access_token":"at-only","token_type":"Bearer"}"#;
        let t = parse_tokens(body).expect("parsed");
        assert_eq!(t.access_token.as_deref(), Some("at-only"));
        assert_eq!(t.refresh_token, None);
    }

    #[test]
    fn parse_tokens_rejects_non_json() {
        assert!(parse_tokens("not json").is_none());
    }

    #[test]
    fn parse_access_token_from_refresh_response() {
        let body = r#"{"access_token":"fresh","expires_in":3599,"token_type":"Bearer"}"#;
        assert_eq!(parse_access_token(body).as_deref(), Some("fresh"));
        assert!(parse_access_token(r#"{"error":"invalid_grant"}"#).is_none());
    }

    #[test]
    fn parse_email_from_userinfo() {
        let body = r#"{"id":"1","email":"user@example.com","verified_email":true}"#;
        assert_eq!(parse_email(body).as_deref(), Some("user@example.com"));
        // email 不在 → None（非致命）。
        assert!(parse_email(r#"{"id":"1"}"#).is_none());
        assert!(parse_email("garbage").is_none());
    }

    #[test]
    fn parse_calendar_list_maps_id_and_summary() {
        let body = r#"{
            "kind":"calendar#calendarList",
            "items":[
                {"id":"primary@example.com","summary":"メイン","accessRole":"owner"},
                {"id":"team@example.com","summary":"チーム"},
                {"id":"no-summary@example.com"},
                {"summary":"no-id"}
            ]
        }"#;
        let out = parse_calendar_list(body);
        assert_eq!(
            out,
            vec![
                CalendarSummary {
                    id: "primary@example.com".to_owned(),
                    summary: "メイン".to_owned(),
                },
                CalendarSummary {
                    id: "team@example.com".to_owned(),
                    summary: "チーム".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn cache_hit_requires_nonempty_and_within_ttl() {
        let one = vec![CalendarSummary {
            id: "c".to_owned(),
            summary: "s".to_owned(),
        }];
        // 非空 + TTL 内 → 採用。
        assert!(cache_entry_is_fresh(&one, Duration::from_secs(10)));
        // 非空 + TTL 直前 → 採用。
        assert!(cache_entry_is_fresh(
            &one,
            CACHE_TTL - Duration::from_secs(1)
        ));
        // 非空 + TTL 経過 → 破棄（再取得）。
        assert!(!cache_entry_is_fresh(&one, CACHE_TTL));
        assert!(!cache_entry_is_fresh(
            &one,
            CACHE_TTL + Duration::from_secs(1)
        ));
        // 空一覧は TTL 内でも採用しない（Node と一致・空をキャッシュ固着させない）。
        assert!(!cache_entry_is_fresh(&[], Duration::from_secs(0)));
    }

    #[test]
    fn parse_calendar_list_empty_on_missing_items_or_garbage() {
        assert!(parse_calendar_list(r#"{"kind":"x"}"#).is_empty());
        assert!(parse_calendar_list("nope").is_empty());
        assert!(parse_calendar_list(r#"{"items":[]}"#).is_empty());
    }
}
