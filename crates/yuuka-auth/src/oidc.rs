//! OIDC の RP（Relying Party）。ログインは OIDC プロバイダーだけに任せる。
//!
//! - 標準のプロバイダーは sk-account（`OIDC_ISSUER` 省略時）。任意の OIDC プロバイダーに替えられる。
//! - Authorization Code フロー＋ PKCE（S256）＋ `state`・`nonce`。エンドポイントは discovery
//!   （`/.well-known/openid-configuration`）から取る。サーバー向けの呼び出し（token・JWKS・revoke）は
//!   `OIDC_INTERNAL_URL` があればそちらへ向ける。
//! - JWT（ID トークン・logout_token）の署名と claim の検証は `jsonwebtoken`（ring）に任せ、自前で暗号処理を
//!   書かない。受け付ける署名方式は非対称鍵のものだけ（`none`・HS* は拒否）。
//! - ログイン後は Yuuka 自身のセッション（[`crate::SessionStore`]）を発行し、プロバイダー側の状態を
//!   [`OidcLink`] として並べて持つ。アクセストークンの期限が切れたらリフレッシュして、プロバイダーで
//!   ログアウト済みならセッションを終える（Back-Channel Logout を取りこぼしたときの保険）。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use yuuka_core::config::OidcSettings;
use yuuka_core::secrets::ExposeSecret;

use crate::session::OidcLink;

/// プロバイダーでのログイン・登録に掛かる時間を見込んだ、ログイン手続きの有効期間。
pub(crate) const LOGIN_TTL: Duration = Duration::from_secs(10 * 60);
/// 進行中のログイン手続きを覚えておく上限（認証なしの `start` でメモリを増やし続けさせない）。
const MAX_PENDING_LOGINS: usize = 10_000;
/// discovery と JWKS を使い回す時間。
const METADATA_TTL: Duration = Duration::from_secs(60 * 60);
/// 知らない `kid` が来たときに JWKS を取り直す間隔の下限（偽の `kid` でプロバイダーを叩かせない）。
const JWKS_REFETCH_INTERVAL: Duration = Duration::from_secs(60);
/// 期限ぎりぎりのトークンを使わないよう、少し早めにリフレッシュする。
const REFRESH_MARGIN_SECS: u64 = 30;
/// プロバイダーが `expires_in` を返さないときに次に確かめ直すまでの時間。
const DEFAULT_ACCESS_TTL_SECS: u64 = 5 * 60;
/// プロバイダーに届かなかったときに次に確かめ直すまでの時間。
const RETRY_AFTER_SECS: u64 = 30;
/// プロバイダーとの 1 回の通信に許す時間。
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
/// logout_token の `iat` として受け付けるずれ。
const LOGOUT_TOKEN_MAX_AGE_SECS: u64 = 5 * 60;
const BACKCHANNEL_LOGOUT_EVENT: &str = "http://schemas.openid.net/event/backchannel-logout";
/// 受け付ける JWT の署名方式（非対称鍵のみ）。
const ALLOWED_ALGORITHMS: [Algorithm; 9] = [
    Algorithm::ES256,
    Algorithm::ES384,
    Algorithm::RS256,
    Algorithm::RS384,
    Algorithm::RS512,
    Algorithm::PS256,
    Algorithm::PS384,
    Algorithm::PS512,
    Algorithm::EdDSA,
];

/// プロバイダーに届かない・想定外の応答だったとき（利用者の操作ではなく、こちら側の問題）。
#[derive(Debug)]
pub struct Unavailable(pub String);

/// ログインを完了できなかった理由。
#[derive(Debug)]
pub enum LoginError {
    /// 手続きが見つからない・`state` 不一致など（もう一度ログインしてもらう）。
    Rejected(&'static str),
    /// プロバイダーに届かない・応答が不正。
    Unavailable(Unavailable),
}

impl From<Unavailable> for LoginError {
    fn from(e: Unavailable) -> Self {
        Self::Unavailable(e)
    }
}

/// ID トークンから取り出した利用者。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcIdentity {
    /// `sub`（`users.sk_account_id` に入れる）。
    pub subject: String,
    /// Discord ID（`OIDC_DISCORD_ID_CLAIM`）。Yuuka の利用者を特定する。
    pub discord_id: String,
    /// 表示名（`name` → `preferred_username` → Discord ID）。
    pub display_name: String,
}

/// ログインの完了結果。
pub struct LoginOutcome {
    /// 利用者。
    pub identity: OidcIdentity,
    /// Yuuka のセッションに並べて保存するプロバイダー側の状態。
    pub link: OidcLink,
}

/// リフレッシュの結果。
pub enum RefreshOutcome {
    /// まだログイン中。新しい状態を保存する。
    Active(OidcLink),
    /// プロバイダーでログアウト済み・失効済み。セッションを終える。
    Ended,
}

/// logout_token が示すログアウトの対象。
#[derive(Debug, PartialEq, Eq)]
pub enum LogoutTarget {
    /// プロバイダーのセッション（`sid`）。
    Session(String),
    /// 利用者（`sub`）の全セッション。
    Subject(String),
}

#[derive(Debug, Clone, Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    #[serde(default)]
    revocation_endpoint: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    id_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

struct PendingLogin {
    nonce: String,
    verifier: String,
    expires_at: Instant,
}

/// OIDC の RP。プロセスに 1 つ作り、ログインのルートと [`crate::CompositeAuth`] で共有する。
pub struct OidcClient {
    issuer: String,
    internal_url: Option<String>,
    client_id: String,
    client_secret: String,
    scope: String,
    discord_id_claim: String,
    label: String,
    redirect_uri: String,
    http: reqwest::Client,
    discovery: tokio::sync::Mutex<Option<(Instant, Discovery)>>,
    jwks: tokio::sync::Mutex<Option<(Instant, JwkSet)>>,
    pending: Mutex<HashMap<String, PendingLogin>>,
    /// 同じセッションのリフレッシュを 1 つずつ行うためのロック（セッションのハッシュで振り分ける）。
    /// 重ねると古いリフレッシュトークンの再使用と見なされ、grant ごと失効させられるため。
    refresh_locks: [tokio::sync::Mutex<()>; 32],
}

impl OidcClient {
    /// 設定から作る。`OIDC_CLIENT_SECRET` が無ければ `None`（ログインできない）。
    ///
    /// `public_base` は Yuuka の公開 URL（`BASE_URL`）。戻り先は `{public_base}/api/auth/oidc/callback`。
    #[must_use]
    pub fn from_settings(settings: &OidcSettings, public_base: &str) -> Option<Self> {
        let client_secret = settings.client_secret.as_ref()?.expose_secret().to_owned();
        let http = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .ok()?;
        Some(Self {
            issuer: settings.issuer.clone(),
            internal_url: settings.internal_url.clone(),
            client_id: settings.client_id.clone(),
            client_secret,
            scope: settings.scope.clone(),
            discord_id_claim: settings.discord_id_claim.clone(),
            label: settings.label.clone(),
            redirect_uri: format!(
                "{}/api/auth/oidc/callback",
                public_base.trim_end_matches('/')
            ),
            http,
            discovery: tokio::sync::Mutex::new(None),
            jwks: tokio::sync::Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            refresh_locks: std::array::from_fn(|_| tokio::sync::Mutex::new(())),
        })
    }

    /// ログインボタンに出す名前。
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// ログインを始める。プロバイダーの認可エンドポイントの URL と、ブラウザに持たせる `state` を返す。
    ///
    /// # Errors
    /// discovery を取得できなければ [`Unavailable`]。
    pub async fn start_login(&self) -> Result<(String, String), Unavailable> {
        let discovery = self.discovery().await?;
        let random = || {
            crate::token::generate_token()
                .map_err(|_| Unavailable("乱数を生成できませんでした".to_owned()))
        };
        let (state, nonce, verifier) = (random()?, random()?, random()?);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut url = reqwest::Url::parse(&discovery.authorization_endpoint)
            .map_err(|e| Unavailable(format!("認可エンドポイントが不正です: {e}")))?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &self.client_id)
            .append_pair("redirect_uri", &self.redirect_uri)
            .append_pair("scope", &self.scope)
            .append_pair("state", &state)
            .append_pair("nonce", &nonce)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256");
        let now = Instant::now();
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.retain(|_, login| login.expires_at > now);
        if pending.len() >= MAX_PENDING_LOGINS {
            // 上限に達したら一番古い手続きを捨てる（その人はもう一度ログインし直す）。
            if let Some(oldest) = pending
                .iter()
                .min_by_key(|(_, login)| login.expires_at)
                .map(|(state, _)| state.clone())
            {
                pending.remove(&oldest);
            }
        }
        pending.insert(
            state.clone(),
            PendingLogin {
                nonce,
                verifier,
                expires_at: now + LOGIN_TTL,
            },
        );
        Ok((url.into(), state))
    }

    /// プロバイダーから戻ってきた `code` を交換し、ID トークンを検証して利用者を返す。
    ///
    /// 呼び出し側は先に、ブラウザの `state` Cookie とクエリの `state` が一致することを確かめる。
    ///
    /// # Errors
    /// 手続きが見つからない・ID トークンが不正なら [`LoginError::Rejected`]、プロバイダーに届かなければ
    /// [`LoginError::Unavailable`]。
    pub async fn finish_login(
        &self,
        state: &str,
        code: &str,
        iss: Option<&str>,
    ) -> Result<LoginOutcome, LoginError> {
        const RETRY: &str = "ログインの確認に失敗しました。もう一度ログインしてください。";
        let login = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(state)
            .filter(|login| login.expires_at > Instant::now())
            .ok_or(LoginError::Rejected(RETRY))?;
        // RFC 9207: 応答に iss が付いていれば、送った先のプロバイダーであること。
        if iss.is_some_and(|iss| iss.trim_end_matches('/') != self.issuer) {
            return Err(LoginError::Rejected(RETRY));
        }
        let (status, body) = self
            .token_request(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", &self.redirect_uri),
                ("code_verifier", &login.verifier),
            ])
            .await?;
        if status == 400 || status == 401 {
            tracing::warn!(status, error = %error_code(&body), "OIDC の code 交換を拒否された");
            return Err(LoginError::Rejected("認証コードが無効または期限切れです。"));
        }
        let tokens = parse_tokens(status, &body)?;
        let id_token = tokens
            .id_token
            .as_deref()
            .ok_or_else(|| Unavailable("ID トークンがありません".to_owned()))?;
        let claims = self.verify_jwt(id_token).await.map_err(|e| {
            tracing::warn!(error = %e, "ID トークンを確認できなかった");
            LoginError::Rejected(RETRY)
        })?;
        if claims.get("nonce").and_then(Value::as_str) != Some(login.nonce.as_str()) {
            tracing::warn!("ID トークンの nonce が一致しない");
            return Err(LoginError::Rejected(RETRY));
        }
        if !authorized_party_ok(&claims, &self.client_id) {
            tracing::warn!("ID トークンの azp が一致しない");
            return Err(LoginError::Rejected(RETRY));
        }
        let identity = self.identity(&claims).ok_or_else(|| {
            tracing::warn!(claim = %self.discord_id_claim, "ID トークンに Discord ID がない");
            LoginError::Rejected("このアカウントには Discord ID がありません。")
        })?;
        let link = OidcLink {
            sub: identity.subject.clone(),
            sid: claims.get("sid").and_then(Value::as_str).map(str::to_owned),
            refresh_token: tokens.refresh_token,
            access_expires_at: unix_now() + tokens.expires_in.unwrap_or(DEFAULT_ACCESS_TTL_SECS),
            retry_after: 0,
        };
        Ok(LoginOutcome { identity, link })
    }

    /// プロバイダーへの再確認が必要か（アクセストークンの期限切れ）。
    #[must_use]
    pub fn needs_refresh(link: &OidcLink) -> bool {
        let now = unix_now();
        link.refresh_token.is_some()
            && now + REFRESH_MARGIN_SECS >= link.access_expires_at
            && now >= link.retry_after
    }

    /// 同じセッションのリフレッシュを直列にするロックを取る（`hash` はセッションのハッシュ）。
    pub async fn lock_refresh(&self, hash: &str) -> tokio::sync::MutexGuard<'_, ()> {
        let index = hash.bytes().fold(0usize, |acc, b| {
            acc.wrapping_mul(31).wrapping_add(usize::from(b))
        });
        let [first, ..] = &self.refresh_locks;
        self.refresh_locks
            .get(index % self.refresh_locks.len())
            .unwrap_or(first)
            .lock()
            .await
    }

    /// リフレッシュしてプロバイダー側のログイン状態を確かめ直す。
    ///
    /// # Errors
    /// プロバイダーに届かなければ [`Unavailable`]（呼び出し側はセッションを残して後で確かめ直す）。
    pub async fn refresh(&self, link: &OidcLink) -> Result<RefreshOutcome, Unavailable> {
        let Some(refresh_token) = &link.refresh_token else {
            return Ok(RefreshOutcome::Active(link.clone()));
        };
        let (status, body) = self
            .token_request(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ])
            .await?;
        if status == 400 || status == 401 {
            return Ok(RefreshOutcome::Ended);
        }
        let tokens = parse_tokens(status, &body)?;
        if let Some(id_token) = &tokens.id_token {
            match self.verify_jwt(id_token).await {
                Ok(claims) if claims.get("sub").and_then(Value::as_str) == Some(&link.sub) => {}
                Ok(_) => return Ok(RefreshOutcome::Ended),
                Err(e) => return Err(Unavailable(format!("ID トークンを確認できません: {e}"))),
            }
        }
        Ok(RefreshOutcome::Active(OidcLink {
            sub: link.sub.clone(),
            sid: link.sid.clone(),
            // 入れ替え式でないプロバイダーは新しい値を返さないので、そのまま使い続ける。
            refresh_token: tokens.refresh_token.or_else(|| link.refresh_token.clone()),
            access_expires_at: unix_now() + tokens.expires_in.unwrap_or(DEFAULT_ACCESS_TTL_SECS),
            retry_after: 0,
        }))
    }

    /// プロバイダーに届かなかったときの状態（少し待ってから確かめ直す）。
    #[must_use]
    pub fn postpone(link: &OidcLink) -> OidcLink {
        OidcLink {
            retry_after: unix_now() + RETRY_AFTER_SECS,
            ..link.clone()
        }
    }

    /// ログアウトした利用者のリフレッシュトークンを失効させる（RFC 7009・届かなくても気にしない）。
    pub async fn revoke(&self, link: &OidcLink) {
        let Some(refresh_token) = &link.refresh_token else {
            return;
        };
        let endpoint = match self.discovery().await {
            Ok(Discovery {
                revocation_endpoint: Some(endpoint),
                ..
            }) => self.server_url(&endpoint),
            Ok(_) => return,
            Err(e) => {
                tracing::warn!(error = %e.0, "リフレッシュトークンを失効させられなかった");
                return;
            }
        };
        let result = self
            .http
            .post(endpoint)
            .basic_auth(
                form_encode(&self.client_id),
                Some(form_encode(&self.client_secret)),
            )
            .form(&[
                ("token", refresh_token.as_str()),
                ("token_type_hint", "refresh_token"),
            ])
            .send()
            .await;
        if let Err(e) = result {
            tracing::warn!(error = %e, "リフレッシュトークンを失効させられなかった");
        }
    }

    /// Back-Channel Logout の logout_token を検証し、ログアウトの対象を返す。不正なら `None`。
    pub async fn verify_logout_token(&self, logout_token: &str) -> Option<LogoutTarget> {
        if let Ok(header) = jsonwebtoken::decode_header(logout_token) {
            // 型が付いていれば logout+jwt であること（ID トークンを logout_token として使わせない）。
            if header
                .typ
                .as_deref()
                .is_some_and(|typ| !typ.eq_ignore_ascii_case("logout+jwt"))
            {
                return None;
            }
        }
        let claims = match self.verify_jwt(logout_token).await {
            Ok(claims) => claims,
            Err(e) => {
                tracing::warn!(error = %e, "logout_token を確認できなかった");
                return None;
            }
        };
        let issued_recently = claims
            .get("iat")
            .and_then(Value::as_u64)
            .is_some_and(|iat| unix_now().abs_diff(iat) <= LOGOUT_TOKEN_MAX_AGE_SECS);
        let has_event = claims
            .get("events")
            .and_then(|events| events.get(BACKCHANNEL_LOGOUT_EVENT))
            .is_some_and(Value::is_object);
        if !issued_recently || !has_event || claims.get("nonce").is_some() {
            return None;
        }
        let text = |key: &str| {
            claims
                .get(key)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
        };
        text("sid")
            .map(LogoutTarget::Session)
            .or_else(|| text("sub").map(LogoutTarget::Subject))
    }

    fn identity(&self, claims: &Value) -> Option<OidcIdentity> {
        let text = |key: &str| {
            claims
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
        };
        let subject = text("sub")?;
        // Discord ID は数字の文字列（数値で返すプロバイダーもあるので受け付ける）。
        let discord_id = match claims.get(&self.discord_id_claim)? {
            Value::String(s) => s.trim().to_owned(),
            Value::Number(n) => n.to_string(),
            _ => return None,
        };
        if discord_id.is_empty() || !discord_id.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let display_name = text("name")
            .or_else(|| text("preferred_username"))
            .unwrap_or_else(|| discord_id.clone());
        Some(OidcIdentity {
            subject,
            discord_id,
            display_name,
        })
    }

    /// サーバーから呼ぶ URL。issuer で始まるものは `OIDC_INTERNAL_URL` へ置き換える。
    fn server_url(&self, url: &str) -> String {
        match (&self.internal_url, url.strip_prefix(&self.issuer)) {
            (Some(internal), Some(rest)) if rest.is_empty() || rest.starts_with('/') => {
                format!("{internal}{rest}")
            }
            _ => url.to_owned(),
        }
    }

    async fn discovery(&self) -> Result<Discovery, Unavailable> {
        let mut cache = self.discovery.lock().await;
        if let Some((fetched, discovery)) = cache.as_ref() {
            if fetched.elapsed() < METADATA_TTL {
                return Ok(discovery.clone());
            }
        }
        let url = self.server_url(&format!("{}/.well-known/openid-configuration", self.issuer));
        let fetched = self
            .fetch_json::<Discovery>(&url)
            .await
            .and_then(|discovery| {
                if discovery.issuer.trim_end_matches('/') == self.issuer {
                    Ok(discovery)
                } else {
                    Err(Unavailable(format!(
                        "discovery の issuer が一致しません: {}",
                        discovery.issuer
                    )))
                }
            });
        match fetched {
            Ok(discovery) => {
                *cache = Some((Instant::now(), discovery.clone()));
                Ok(discovery)
            }
            // 取り直しに失敗しても、前に取れた内容があれば使い続ける。
            Err(e) => match cache.as_ref() {
                Some((_, discovery)) => {
                    tracing::warn!(error = %e.0, "discovery を取り直せなかった（前の内容を使う）");
                    Ok(discovery.clone())
                }
                None => Err(e),
            },
        }
    }

    async fn fetch_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
    ) -> Result<T, Unavailable> {
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| Unavailable(format!("{url} に接続できません: {e}")))?;
        if !response.status().is_success() {
            return Err(Unavailable(format!(
                "{url} が {} を返しました",
                response.status()
            )));
        }
        response
            .json::<T>()
            .await
            .map_err(|e| Unavailable(format!("{url} の応答が不正です: {e}")))
    }

    async fn token_request(&self, pairs: &[(&str, &str)]) -> Result<(u16, Value), Unavailable> {
        let discovery = self.discovery().await?;
        let response = self
            .http
            .post(self.server_url(&discovery.token_endpoint))
            .basic_auth(
                form_encode(&self.client_id),
                Some(form_encode(&self.client_secret)),
            )
            .form(pairs)
            .send()
            .await
            .map_err(|e| Unavailable(format!("トークンエンドポイントに接続できません: {e}")))?;
        let status = response.status().as_u16();
        let body = response.json::<Value>().await.unwrap_or(Value::Null);
        Ok((status, body))
    }

    async fn decoding_key(&self, kid: Option<&str>, alg: Algorithm) -> Result<DecodingKey, String> {
        let mut cache = self.jwks.lock().await;
        if let Some((fetched, set)) = cache.as_ref() {
            if fetched.elapsed() < METADATA_TTL {
                if let Some(key) = find_key(set, kid, alg) {
                    return key;
                }
            }
            // 鍵の入れ替え直後なら取り直すと見つかる。ただし取り直したばかりなら待つ。
            if fetched.elapsed() < JWKS_REFETCH_INTERVAL {
                return Err("署名鍵が見つかりません".to_owned());
            }
        }
        let discovery = self.discovery().await.map_err(|e| e.0)?;
        let set: JwkSet = self
            .fetch_json(&self.server_url(&discovery.jwks_uri))
            .await
            .map_err(|e| e.0)?;
        let key = find_key(&set, kid, alg);
        *cache = Some((Instant::now(), set));
        key.unwrap_or_else(|| Err("署名鍵が見つかりません".to_owned()))
    }

    /// 署名・`iss`・`aud`・`exp` を確かめた JWT の claim。
    async fn verify_jwt(&self, token: &str) -> Result<Value, String> {
        let header = jsonwebtoken::decode_header(token).map_err(|e| e.to_string())?;
        if !ALLOWED_ALGORITHMS.contains(&header.alg) {
            return Err(format!("受け付けない署名方式です: {:?}", header.alg));
        }
        let key = self.decoding_key(header.kid.as_deref(), header.alg).await?;
        // iss は discovery が返した表記のまま照合する（末尾の `/` の有無はプロバイダーによる）。
        let issuer = self.discovery().await.map_err(|e| e.0)?.issuer;
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[&issuer]);
        validation.set_audience(&[&self.client_id]);
        jsonwebtoken::decode::<Value>(token, &key, &validation)
            .map(|data| data.claims)
            .map_err(|e| e.to_string())
    }
}

/// `kid` に当たる鍵（`kid` が無ければ、署名方式に合う鍵が 1 つだけのときにそれを使う）。
fn find_key(
    set: &JwkSet,
    kid: Option<&str>,
    alg: Algorithm,
) -> Option<Result<DecodingKey, String>> {
    let jwk = match kid {
        Some(kid) => set.find(kid)?,
        None => {
            let mut candidates = set.keys.iter().filter(|jwk| {
                jwk.common
                    .key_algorithm
                    .is_none_or(|key_alg| same_alg(key_alg, alg))
            });
            let only = candidates.next()?;
            if candidates.next().is_some() {
                return None;
            }
            only
        }
    };
    // 鍵に署名方式が書かれていれば、JWT の alg と一致すること。
    if jwk
        .common
        .key_algorithm
        .is_some_and(|key_alg| !same_alg(key_alg, alg))
    {
        return Some(Err("署名鍵と署名方式が一致しません".to_owned()));
    }
    Some(DecodingKey::from_jwk(jwk).map_err(|e| e.to_string()))
}

/// 鍵に書かれた署名方式と JWT の alg が同じか（どちらも RFC 7518 の名前で比べる）。
fn same_alg(key_alg: jsonwebtoken::jwk::KeyAlgorithm, alg: Algorithm) -> bool {
    format!("{key_alg:?}") == format!("{alg:?}")
}

/// `aud` が複数あるときは `azp` が自分であること（OIDC Core 3.1.3.7）。
fn authorized_party_ok(claims: &Value, client_id: &str) -> bool {
    let multiple_audiences = claims
        .get("aud")
        .and_then(Value::as_array)
        .is_some_and(|aud| aud.len() > 1);
    match claims.get("azp").and_then(Value::as_str) {
        Some(azp) => azp == client_id,
        None => !multiple_audiences,
    }
}

fn parse_tokens(status: u16, body: &Value) -> Result<TokenResponse, Unavailable> {
    if status != 200 {
        return Err(Unavailable(format!(
            "トークンエンドポイントが {status} を返しました ({})",
            error_code(body)
        )));
    }
    serde_json::from_value(body.clone())
        .map_err(|e| Unavailable(format!("トークンの応答が不正です: {e}")))
}

fn error_code(body: &Value) -> &str {
    body.get("error")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// `client_secret_basic` はクライアント ID とシークレットを form エンコードしてから Basic にする（RFC 6749 2.3.1）。
fn form_encode(value: &str) -> String {
    reqwest::Url::parse_with_params("x:", &[("v", value)])
        .ok()
        .and_then(|url| url.query().map(|q| q.trim_start_matches("v=").to_owned()))
        .unwrap_or_else(|| value.to_owned())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn client(internal_url: Option<&str>) -> OidcClient {
        let settings = OidcSettings {
            issuer: "https://account.example.com".to_owned(),
            internal_url: internal_url.map(str::to_owned),
            client_id: "yuuka".to_owned(),
            client_secret: Some("secret".to_owned().into()),
            scope: "openid profile discord".to_owned(),
            discord_id_claim: "discord_id".to_owned(),
            label: "Example".to_owned(),
        };
        OidcClient::from_settings(&settings, "https://yuuka.example.com/").expect("client")
    }

    #[test]
    fn no_secret_means_no_client() {
        let settings = OidcSettings {
            issuer: "https://account.example.com".to_owned(),
            internal_url: None,
            client_id: "yuuka".to_owned(),
            client_secret: None,
            scope: "openid".to_owned(),
            discord_id_claim: "discord_id".to_owned(),
            label: "Example".to_owned(),
        };
        assert!(OidcClient::from_settings(&settings, "https://yuuka.example.com").is_none());
    }

    #[test]
    fn redirect_uri_is_built_from_base_url() {
        assert_eq!(
            client(None).redirect_uri,
            "https://yuuka.example.com/api/auth/oidc/callback"
        );
    }

    #[test]
    fn server_urls_under_the_issuer_go_to_the_internal_url() {
        let c = client(Some("http://127.0.0.1:3100"));
        assert_eq!(
            c.server_url("https://account.example.com/oauth2/token"),
            "http://127.0.0.1:3100/oauth2/token"
        );
        // 別ホストや、issuer を接頭辞に持つだけの別ホストは置き換えない。
        assert_eq!(
            c.server_url("https://keys.example.net/jwks"),
            "https://keys.example.net/jwks"
        );
        assert_eq!(
            c.server_url("https://account.example.com.evil.test/token"),
            "https://account.example.com.evil.test/token"
        );
        assert_eq!(
            client(None).server_url("https://account.example.com/oauth2/token"),
            "https://account.example.com/oauth2/token"
        );
    }

    #[test]
    fn identity_requires_a_numeric_discord_id() {
        let c = client(None);
        let ok = c
            .identity(&json!({ "sub": "acc-1", "discord_id": "100", "name": "Alice" }))
            .expect("identity");
        assert_eq!(
            ok,
            OidcIdentity {
                subject: "acc-1".to_owned(),
                discord_id: "100".to_owned(),
                display_name: "Alice".to_owned(),
            }
        );
        let numeric = c
            .identity(&json!({ "sub": "acc-2", "discord_id": 200, "preferred_username": "bob" }))
            .expect("numeric id");
        assert_eq!(numeric.discord_id, "200");
        assert_eq!(numeric.display_name, "bob");
        assert!(c.identity(&json!({ "sub": "acc-3" })).is_none());
        assert!(c
            .identity(&json!({ "sub": "acc-4", "discord_id": "12a" }))
            .is_none());
        assert!(c.identity(&json!({ "discord_id": "100" })).is_none());
    }

    #[test]
    fn azp_must_match_when_there_are_several_audiences() {
        assert!(authorized_party_ok(&json!({ "aud": "yuuka" }), "yuuka"));
        assert!(authorized_party_ok(
            &json!({ "aud": ["yuuka", "other"], "azp": "yuuka" }),
            "yuuka"
        ));
        assert!(!authorized_party_ok(
            &json!({ "aud": ["yuuka", "other"] }),
            "yuuka"
        ));
        assert!(!authorized_party_ok(
            &json!({ "aud": "yuuka", "azp": "other" }),
            "yuuka"
        ));
    }

    #[test]
    fn refresh_is_needed_only_after_the_access_token_expires() {
        let now = unix_now();
        let link = |expires: u64, retry: u64, refresh: bool| OidcLink {
            sub: "acc-1".to_owned(),
            sid: None,
            refresh_token: refresh.then(|| "rt".to_owned()),
            access_expires_at: expires,
            retry_after: retry,
        };
        assert!(!OidcClient::needs_refresh(&link(now + 600, 0, true)));
        assert!(OidcClient::needs_refresh(&link(now + 10, 0, true)));
        assert!(
            !OidcClient::needs_refresh(&link(now, now + 30, true)),
            "届かなかった直後は待つ"
        );
        assert!(
            !OidcClient::needs_refresh(&link(now, 0, false)),
            "リフレッシュトークンが無ければ確かめない"
        );
    }

    #[test]
    fn client_secret_basic_form_encodes_credentials() {
        assert_eq!(form_encode("a b+c:d"), "a+b%2Bc%3Ad");
        assert_eq!(form_encode("plain"), "plain");
    }
}
