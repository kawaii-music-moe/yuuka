//! Cookie セッション検証（共有 Redis の不透明トークン・Node `getSession` パリティ）。
//!
//! Rust は**検証専用**（セッション発行は Node のログインが担う）。`session:{token_hash}` の
//! JSON `{discordId,username,role}` を引き、取得成功時に `session:` と `user_sessions:{discordId}`
//! の TTL をスライディング更新する。Redis 到達不能時は縮退（`Ok(None)`）＝Cookie 認証のみ無効化。

use std::time::Duration;

use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use yuuka_core::AuthError;
use yuuka_types::SessionUser;

/// 起動時の初期 Redis 接続に許す上限時間（Redis 障害で起動をハングさせない）。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// セッション検証に使う Redis ストア（自動再接続つき `ConnectionManager`）。
///
/// 起動時に Redis へ到達できなければ [`SessionStore::Disabled`]（起動は継続・Cookie 縮退）。
#[derive(Clone)]
pub enum SessionStore {
    /// 接続済み Redis（`ConnectionManager` が背後で自動再接続する）。
    ///
    /// `ConnectionManager` は大きいので `Box` に入れる（`Disabled` との variant サイズ差回避）。
    Redis(Box<ConnectionManager>),
    /// Redis 未接続（起動時到達不能／URL 不正）。Cookie セッションは検証不可。
    Disabled,
}

impl SessionStore {
    /// `redis_url` へ接続を試みる。到達不能でも `Disabled` を返して**起動は継続**する。
    #[must_use]
    pub async fn connect(redis_url: &str) -> Self {
        let client = match redis::Client::open(redis_url) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "REDIS_URL が不正。Cookie セッションは縮退");
                return Self::Disabled;
            }
        };
        // 初期接続に上限時間を設ける（Redis 障害時に起動がハングしない・制約2）。
        match tokio::time::timeout(CONNECT_TIMEOUT, ConnectionManager::new(client)).await {
            Ok(Ok(cm)) => {
                tracing::info!("Redis 接続確立（Cookie セッション検証 有効）");
                Self::Redis(Box::new(cm))
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "Redis 接続不可。Cookie セッションは縮退（Bearer は動作）");
                Self::Disabled
            }
            Err(_) => {
                tracing::warn!("Redis 接続タイムアウト。Cookie セッションは縮退（Bearer は動作）");
                Self::Disabled
            }
        }
    }

    /// `session:{token_hash}` を引き、TTL をスライディング更新して [`SessionUser`] を返す。
    ///
    /// 未ヒット・破損値・Redis 縮退はいずれも `Ok(None)`（＝未認証扱い）。
    ///
    /// # Errors
    /// Redis コマンド失敗（接続断など）は [`AuthError::Backend`]（502・監視向け。401 と区別）。
    pub async fn get(
        &self,
        token_hash: &str,
        ttl_secs: u64,
    ) -> Result<Option<SessionUser>, AuthError> {
        let Self::Redis(cm) = self else {
            return Ok(None);
        };
        let mut cm = (**cm).clone();
        let session_key = format!("session:{token_hash}");

        let json: Option<String> = cm.get(&session_key).await.map_err(|_| AuthError::Backend)?;
        let Some(json) = json else {
            return Ok(None);
        };
        // 破損値は無効扱い（Node は破損レコードを削除するが、検証専用の Rust は None で足りる）。
        let Ok(user) = serde_json::from_str::<SessionUser>(&json) else {
            return Ok(None);
        };

        // スライディング更新（session + user_sessions 両キー）。失敗しても検証結果は返す。
        let ttl = i64::try_from(ttl_secs).unwrap_or(i64::MAX);
        let user_key = format!("user_sessions:{}", user.discord_id);
        let _: Result<i64, _> = cm.expire(&session_key, ttl).await;
        let _: Result<i64, _> = cm.expire(&user_key, ttl).await;

        Ok(Some(user))
    }
}
