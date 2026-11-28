//! Discord の後付け連携（docs/design/optional_discord.md §5）。
//!
//! ログイン済みの利用者を Discord OAuth2（scope `identify`）へ送り、戻ってきた code を交換して
//! Discord のユーザー ID を確かめ、`users.discord_id` に記録する。ログインそのものには使わない。
//!
//! - `GET /api/me/discord`：連携の状態（`available` は連携ボタンを出せるか）。
//! - `GET /api/me/discord/link/start`：Discord の認可画面へ送る。
//! - `GET /api/me/discord/link/callback`：code を交換して連携し、アカウント画面へ戻す。
//! - `POST /api/me/discord/unlink`：連携を解除する（OIDC の紐付けがあるときだけ）。
//!
//! `state` は発行した利用者に結び付けてメモリに持ち、callback では同じ利用者のセッションで
//! 戻ってきたときだけ受け付ける（他人の Discord を連携させる CSRF を防ぐ）。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;
use yuuka_core::secrets::{ExposeSecret, SecretString};

/// 認可画面から戻るまでの猶予。
pub const LINK_TTL: Duration = Duration::from_secs(10 * 60);
/// 保留できる連携手続きの上限（溢れたら古いものから捨てる）。
const MAX_PENDING: usize = 1024;
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const DISCORD_AUTHORIZE: &str = "https://discord.com/oauth2/authorize";
const DISCORD_API: &str = "https://discord.com/api";

/// Discord OAuth2 のクライアント。プロセスに 1 つ作り、[`crate::AuthRuntime`] に持たせる。
pub struct DiscordLinkClient {
    client_id: String,
    client_secret: SecretString,
    redirect_uri: String,
    api_base: String,
    http: reqwest::Client,
    /// `state` → （利用者 ID・発行時刻）。
    pending: Mutex<HashMap<String, (String, Instant)>>,
}

/// 連携手続きの失敗。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
    /// `state` が無い・期限切れ・別の利用者のもの。
    BadState,
    /// Discord が code を受け付けなかった、または応答が読めなかった。
    Rejected(String),
    /// Discord へ届かなかった。
    Unavailable(String),
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct DiscordUser {
    id: String,
}

impl DiscordLinkClient {
    /// クライアント ID とシークレットがそろっていれば作る。戻り先は `{public_base}/api/me/discord/link/callback`。
    #[must_use]
    pub fn new(
        client_id: Option<&str>,
        client_secret: Option<&SecretString>,
        public_base: &str,
    ) -> Option<Self> {
        let client_id = client_id.filter(|id| !id.is_empty())?.to_owned();
        let client_secret = client_secret?.clone();
        let http = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .ok()?;
        Some(Self {
            client_id,
            client_secret,
            redirect_uri: format!(
                "{}/api/me/discord/link/callback",
                public_base.trim_end_matches('/')
            ),
            api_base: DISCORD_API.to_owned(),
            http,
            pending: Mutex::new(HashMap::new()),
        })
    }

    /// Discord API の基点を差し替える（テスト用）。
    #[must_use]
    pub fn with_api_base(mut self, api_base: &str) -> Self {
        self.api_base = api_base.trim_end_matches('/').to_owned();
        self
    }

    /// 認可画面の URL を作り、`state` を `user_id` に結び付けて覚える。
    ///
    /// # Errors
    /// 乱数を得られないとき [`LinkError::Unavailable`]。
    pub fn start(&self, user_id: &str) -> Result<String, LinkError> {
        let state =
            crate::token::generate_token().map_err(|e| LinkError::Unavailable(e.to_string()))?;
        {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            pending.retain(|_, (_, at)| now.duration_since(*at) < LINK_TTL);
            if pending.len() >= MAX_PENDING {
                if let Some(oldest) = pending
                    .iter()
                    .min_by_key(|(_, (_, at))| *at)
                    .map(|(k, _)| k.clone())
                {
                    pending.remove(&oldest);
                }
            }
            pending.insert(state.clone(), (user_id.to_owned(), now));
        }
        let mut url = reqwest::Url::parse(DISCORD_AUTHORIZE)
            .map_err(|e| LinkError::Unavailable(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &self.client_id)
            .append_pair("scope", "identify")
            .append_pair("redirect_uri", &self.redirect_uri)
            .append_pair("state", &state)
            .append_pair("prompt", "consent");
        Ok(url.into())
    }

    /// 戻ってきた `state` と `code` を確かめ、Discord のユーザー ID を返す。
    ///
    /// # Errors
    /// [`LinkError`]。
    pub async fn finish(
        &self,
        user_id: &str,
        state: &str,
        code: &str,
    ) -> Result<String, LinkError> {
        let owner = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(state)
            .filter(|(_, at)| at.elapsed() < LINK_TTL)
            .map(|(owner, _)| owner);
        if owner.as_deref() != Some(user_id) {
            return Err(LinkError::BadState);
        }
        let response = self
            .http
            .post(format!("{}/oauth2/token", self.api_base))
            .basic_auth(&self.client_id, Some(self.client_secret.expose_secret()))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", self.redirect_uri.as_str()),
            ])
            .send()
            .await
            .map_err(|e| LinkError::Unavailable(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(LinkError::Rejected(format!("token: {status}")));
        }
        let token: TokenResponse = response
            .json()
            .await
            .map_err(|e| LinkError::Rejected(format!("token: {e}")))?;
        let response = self
            .http
            .get(format!("{}/users/@me", self.api_base))
            .bearer_auth(&token.access_token)
            .send()
            .await
            .map_err(|e| LinkError::Unavailable(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(LinkError::Rejected(format!("users/@me: {status}")));
        }
        let user: DiscordUser = response
            .json()
            .await
            .map_err(|e| LinkError::Rejected(format!("users/@me: {e}")))?;
        if user.id.is_empty() || !user.id.bytes().all(|b| b.is_ascii_digit()) {
            return Err(LinkError::Rejected("Discord ID が数字ではない".to_owned()));
        }
        Ok(user.id)
    }
}
