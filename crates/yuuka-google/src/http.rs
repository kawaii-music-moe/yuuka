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

use crate::ports::{
    CalendarEventsPort, CalendarPort, CalendarSummary, GoogleError, GoogleEvent, GoogleEventInput,
    GoogleOAuthPort, GoogleTokens, LinkedGoogleAccount,
};
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
/// カレンダー API の基点（`{base}/{calendarId}/events`）。
const CALENDARS_ENDPOINT: &str = "https://www.googleapis.com/calendar/v3/calendars";
/// アクセストークンの使い回し期間（Google の有効期限 1 時間より短く取る）。
const ACCESS_TOKEN_TTL: Duration = Duration::from_secs(50 * 60);
/// 予定一覧のページ数の上限（1 ページ 250 件・取り込み範囲は 1 か月程度なので十分）。
const MAX_EVENT_PAGES: usize = 8;
/// アカウントに既定カレンダーが無いときの登録先（Google の主カレンダーの別名）。
const PRIMARY_CALENDAR_ALIAS: &str = "primary";

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
    /// アカウント別のアクセストークン（[`ACCESS_TOKEN_TTL`] の間は使い回す）。
    access_tokens: Mutex<HashMap<i64, (String, Instant)>>,
    /// アカウント別の書き込み可能カレンダー一覧（[`CACHE_TTL`]）。
    account_calendars: Mutex<HashMap<i64, (Vec<CalendarSummary>, Instant)>>,
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
            access_tokens: Mutex::new(HashMap::new()),
            account_calendars: Mutex::new(HashMap::new()),
        }
    }

    /// アカウントのアクセストークン（キャッシュが無効ならリフレッシュトークンから取り直す）。
    ///
    /// # Errors
    /// アカウント不在 / 復号失敗 / 上流失敗時 [`GoogleError`]。
    async fn access_token_for_account(&self, account_id: i64) -> Result<String, GoogleError> {
        {
            let guard = self.access_tokens.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((token, fetched)) = guard.get(&account_id) {
                if fetched.elapsed() < ACCESS_TOKEN_TTL {
                    return Ok(token.clone());
                }
            }
        }
        let (_owner, enc, iv, tag) = repo::get_account_tokens(&self.db, account_id)
            .await
            .map_err(|e| GoogleError::Upstream(e.to_string()))?
            .ok_or(GoogleError::NotConfigured)?;
        let refresh = self
            .crypto
            .decrypt_text(&enc, &iv, &tag)
            .map_err(|e| GoogleError::Upstream(e.to_string()))?;
        let access = self.refresh_access_token(&refresh).await?;
        let mut guard = self.access_tokens.lock().unwrap_or_else(|e| e.into_inner());
        guard.insert(account_id, (access.clone(), Instant::now()));
        Ok(access)
    }

    /// 上流の失敗をログ用の文言にする（本文の先頭だけ添える）。
    async fn upstream_error(what: &str, resp: reqwest::Response) -> GoogleError {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let snippet: String = body.chars().take(200).collect();
        GoogleError::Upstream(format!("{what} status {status}: {snippet}"))
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

/// ローカル暦（`'YYYY-MM-DD HH:MM:SS'`・JST）を Google API の RFC 3339（`+09:00`）にする。
fn local_to_rfc3339(local: &str) -> Option<String> {
    let naive = chrono::NaiveDateTime::parse_from_str(local, "%Y-%m-%d %H:%M:%S").ok()?;
    Some(naive.format("%Y-%m-%dT%H:%M:%S+09:00").to_string())
}

/// 開始から 1 時間後のローカル暦（終了が省略された予定用・Node `createCalendarEvent` と同じ）。
fn one_hour_after(local: &str) -> Option<String> {
    let naive = chrono::NaiveDateTime::parse_from_str(local, "%Y-%m-%d %H:%M:%S").ok()?;
    Some(
        (naive + chrono::Duration::hours(1))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
    )
}

/// Google の `start`/`end`（`dateTime` か終日の `date`）をローカル暦にする。終日予定の `end.date` は
/// 翌日（排他的）なので、`exclusive_end` のときは前日の 23:59:59 にする。
fn google_time_to_local(value: &serde_json::Value, exclusive_end: bool) -> Option<String> {
    let offset = chrono::FixedOffset::east_opt(9 * 3600)?;
    if let Some(dt) = value.get("dateTime").and_then(serde_json::Value::as_str) {
        let parsed = chrono::DateTime::parse_from_rfc3339(dt).ok()?;
        return Some(
            parsed
                .with_timezone(&offset)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
        );
    }
    let date = value.get("date").and_then(serde_json::Value::as_str)?;
    let day = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    if exclusive_end {
        let last = day.pred_opt()?;
        return Some(format!("{} 23:59:59", last.format("%Y-%m-%d")));
    }
    Some(format!("{} 00:00:00", day.format("%Y-%m-%d")))
}

/// events.list 応答の `items` を [`GoogleEvent`] にする（キャンセル済み・開始の無いものは除く）。
fn parse_events(value: &serde_json::Value) -> Vec<GoogleEvent> {
    let Some(items) = value.get("items").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter(|item| item.get("status").and_then(serde_json::Value::as_str) != Some("cancelled"))
        .filter_map(|item| {
            let id = item.get("id").and_then(serde_json::Value::as_str)?;
            let start_local = google_time_to_local(item.get("start")?, false)?;
            let end_local = item
                .get("end")
                .and_then(|end| google_time_to_local(end, true));
            let text = |key: &str| {
                item.get(key)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            let title = Some(text("summary"))
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "無題の予定".to_owned());
            Some(GoogleEvent {
                id: id.to_owned(),
                title,
                description: text("description"),
                start_local,
                end_local,
            })
        })
        .collect()
}

#[async_trait]
impl CalendarEventsPort for GoogleHttpClient {
    async fn account_for(&self, user_id: &str, bot_id: &str) -> Option<LinkedGoogleAccount> {
        if !self.is_configured() {
            return None;
        }
        let account = repo::resolve_account_for_bot(&self.db, user_id, bot_id)
            .await
            .ok()
            .flatten()?;
        Some(LinkedGoogleAccount {
            account_id: account.id,
            default_calendar_id: account
                .calendar_id
                .filter(|id| !id.is_empty())
                .unwrap_or_else(|| PRIMARY_CALENDAR_ALIAS.to_owned()),
        })
    }

    async fn import_account_for(&self, user_id: &str, bot_id: &str) -> Option<i64> {
        if !self.is_configured() {
            return None;
        }
        repo::explicit_account_for_bot(&self.db, user_id, bot_id)
            .await
            .ok()
            .flatten()
    }

    async fn calendars(&self, account_id: i64) -> Result<Vec<CalendarSummary>, GoogleError> {
        {
            let guard = self
                .account_calendars
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some((calendars, fetched)) = guard.get(&account_id) {
                if cache_entry_is_fresh(calendars, fetched.elapsed()) {
                    return Ok(calendars.clone());
                }
            }
        }
        let access = self.access_token_for_account(account_id).await?;
        let calendars = self.fetch_calendar_list(&access).await?;
        let mut guard = self
            .account_calendars
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        guard.insert(account_id, (calendars.clone(), Instant::now()));
        Ok(calendars)
    }

    async fn insert_event(
        &self,
        account_id: i64,
        calendar_id: &str,
        event: &GoogleEventInput,
    ) -> Result<String, GoogleError> {
        let invalid = || GoogleError::Upstream("invalid event datetime".to_owned());
        let start = local_to_rfc3339(&event.start_local).ok_or_else(invalid)?;
        let end_local = match &event.end_local {
            Some(end) => end.clone(),
            None => one_hour_after(&event.start_local).ok_or_else(invalid)?,
        };
        let end = local_to_rfc3339(&end_local).ok_or_else(invalid)?;
        let body = serde_json::json!({
            "summary": event.title,
            "description": event.description.clone().unwrap_or_default(),
            "start": { "dateTime": start },
            "end": { "dateTime": end },
        });
        let access = self.access_token_for_account(account_id).await?;
        let resp = self
            .http
            .post(format!(
                "{CALENDARS_ENDPOINT}/{}/events",
                encode_component(calendar_id)
            ))
            .bearer_auth(access)
            .json(&body)
            .timeout(HTTP_TIMEOUT)
            .send()
            .await
            .map_err(|e| GoogleError::Upstream(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(Self::upstream_error("events.insert", resp).await);
        }
        let value: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| GoogleError::Upstream(e.to_string()))?;
        value
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| GoogleError::Upstream("events.insert returned no id".to_owned()))
    }

    async fn delete_event(
        &self,
        account_id: i64,
        calendar_id: &str,
        event_id: &str,
    ) -> Result<(), GoogleError> {
        let access = self.access_token_for_account(account_id).await?;
        let resp = self
            .http
            .delete(format!(
                "{CALENDARS_ENDPOINT}/{}/events/{}",
                encode_component(calendar_id),
                encode_component(event_id)
            ))
            .bearer_auth(access)
            .timeout(HTTP_TIMEOUT)
            .send()
            .await
            .map_err(|e| GoogleError::Upstream(e.to_string()))?;
        // 既に消えている（404/410）なら目的は達成済み。
        let status = resp.status().as_u16();
        if resp.status().is_success() || status == 404 || status == 410 {
            return Ok(());
        }
        Err(Self::upstream_error("events.delete", resp).await)
    }

    async fn list_events(
        &self,
        account_id: i64,
        calendar_id: &str,
        from_local: &str,
        to_local: &str,
    ) -> Result<Vec<GoogleEvent>, GoogleError> {
        let invalid = || GoogleError::Upstream("invalid range datetime".to_owned());
        let time_min = local_to_rfc3339(from_local).ok_or_else(invalid)?;
        let time_max = local_to_rfc3339(to_local).ok_or_else(invalid)?;
        let access = self.access_token_for_account(account_id).await?;
        let url = format!(
            "{CALENDARS_ENDPOINT}/{}/events",
            encode_component(calendar_id)
        );
        let mut events = Vec::new();
        let mut page_token: Option<String> = None;
        for _ in 0..MAX_EVENT_PAGES {
            let mut query = vec![
                ("timeMin", time_min.clone()),
                ("timeMax", time_max.clone()),
                ("singleEvents", "true".to_owned()),
                ("orderBy", "startTime".to_owned()),
                ("maxResults", "250".to_owned()),
            ];
            if let Some(token) = &page_token {
                query.push(("pageToken", token.clone()));
            }
            let resp = self
                .http
                .get(&url)
                .query(&query)
                .bearer_auth(&access)
                .timeout(HTTP_TIMEOUT)
                .send()
                .await
                .map_err(|e| GoogleError::Upstream(e.to_string()))?;
            if !resp.status().is_success() {
                return Err(Self::upstream_error("events.list", resp).await);
            }
            let value: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| GoogleError::Upstream(e.to_string()))?;
            events.extend(parse_events(&value));
            page_token = value
                .get("nextPageToken")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            if page_token.is_none() {
                break;
            }
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        cache_entry_is_fresh, encode_component, google_time_to_local, local_to_rfc3339,
        one_hour_after, parse_access_token, parse_calendar_list, parse_email, parse_events,
        parse_tokens, CalendarSummary, CACHE_TTL, OAUTH_SCOPES,
    };

    #[test]
    fn converts_between_local_time_and_google_time() {
        assert_eq!(
            local_to_rfc3339("2026-10-06 09:30:00").as_deref(),
            Some("2026-10-06T09:30:00+09:00")
        );
        assert_eq!(local_to_rfc3339("2026-10-06T09:30:00"), None);
        assert_eq!(
            one_hour_after("2026-10-06 23:30:00").as_deref(),
            Some("2026-10-07 00:30:00")
        );
        let dt = serde_json::json!({ "dateTime": "2026-10-06T01:00:00Z" });
        assert_eq!(
            google_time_to_local(&dt, false).as_deref(),
            Some("2026-10-06 10:00:00")
        );
        // 終日予定: 開始はその日の 0 時、終了（翌日・排他的）は前日の 23:59:59。
        let day = serde_json::json!({ "date": "2026-10-07" });
        assert_eq!(
            google_time_to_local(&day, false).as_deref(),
            Some("2026-10-07 00:00:00")
        );
        assert_eq!(
            google_time_to_local(&day, true).as_deref(),
            Some("2026-10-06 23:59:59")
        );
    }

    #[test]
    fn parses_events_skipping_cancelled_ones() {
        let body = serde_json::json!({ "items": [
            { "id": "a", "summary": "会議", "description": "メモ",
              "start": { "dateTime": "2026-10-06T10:00:00+09:00" },
              "end": { "dateTime": "2026-10-06T11:00:00+09:00" } },
            { "id": "b", "status": "cancelled", "start": { "dateTime": "2026-10-06T12:00:00+09:00" } },
            { "id": "c", "start": { "date": "2026-10-08" }, "end": { "date": "2026-10-09" } }
        ] });
        let events = parse_events(&body);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, "a");
        assert_eq!(events[0].title, "会議");
        assert_eq!(events[0].start_local, "2026-10-06 10:00:00");
        assert_eq!(events[0].end_local.as_deref(), Some("2026-10-06 11:00:00"));
        assert_eq!(events[1].title, "無題の予定");
        assert_eq!(events[1].start_local, "2026-10-08 00:00:00");
        assert_eq!(events[1].end_local.as_deref(), Some("2026-10-08 23:59:59"));
    }

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
