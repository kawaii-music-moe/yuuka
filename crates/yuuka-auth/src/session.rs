//! Cookie セッションの**発行 + 検証**（共有 Redis の不透明トークン・Node `sessionService.ts` パリティ）。
//!
//! - トークン: CSPRNG（[`crate::token::generate_token`]）。保存はハッシュ化キー `session:{sha256(token)}`
//!   のみ（Redis ダンプが漏れても生トークンは復元不能）。
//! - TTL: `session_ttl_days` 日。アクセス毎に自動延長（スライディングウィンドウ）。
//! - ユーザー毎の発行済みセット `user_sessions:{discordId}` を保持（将来の一括失効用）。
//! - **Redis 不通時は in-memory Map フォールバック**（Node と同一・プロセス再起動で消えるのは許容）。
//!
//! キー書式・ハッシュ・シリアライズ（camelCase `{discordId,username,role}`）は既存 Node と一致させ、
//! 移行期は Node/Rust が同一 Redis を共有できる（strangler）。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use yuuka_core::AuthError;
use yuuka_types::SessionUser;

/// 起動時の初期 Redis 接続に許す上限時間（Redis 障害で起動をハングさせない）。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// in-memory セッション（Redis 縮退時のフォールバック）。
struct MemSession {
    user: SessionUser,
    expires_at: Instant,
}

/// Redis 縮退時のフォールバックストア（Node の `memorySessions`/`memoryUserSessions` 相当）。
#[derive(Default)]
struct MemoryStore {
    /// `token_hash → セッション`。
    sessions: HashMap<String, MemSession>,
    /// `discord_id → 発行済み token_hash 集合`（将来の一括失効用）。
    user_sessions: HashMap<String, HashSet<String>>,
}

/// セッションの発行・検証を担う共有ストア（Redis + in-memory フォールバック）。
///
/// `ConnectionManager` は背後で自動再接続する。全クローンが同一 Redis と同一 in-memory を共有する
/// （`Arc<Mutex<..>>`）。
#[derive(Clone)]
pub struct SessionStore {
    /// 接続済み Redis（未接続時は `None` ＝ in-memory のみ）。
    redis: Option<ConnectionManager>,
    /// Redis 縮退時のフォールバック（全クローン共有）。
    memory: Arc<Mutex<MemoryStore>>,
}

impl SessionStore {
    /// `redis_url` へ接続を試みる。到達不能でも **起動は継続**（in-memory フォールバックのみ有効）。
    #[must_use]
    pub async fn connect(redis_url: &str) -> Self {
        let memory = Arc::new(Mutex::new(MemoryStore::default()));
        let client = match redis::Client::open(redis_url) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "REDIS_URL が不正。Cookie セッションは in-memory 縮退");
                return Self {
                    redis: None,
                    memory,
                };
            }
        };
        match tokio::time::timeout(CONNECT_TIMEOUT, ConnectionManager::new(client)).await {
            Ok(Ok(cm)) => {
                tracing::info!("Redis 接続確立（Cookie セッション 発行/検証 有効）");
                Self {
                    redis: Some(cm),
                    memory,
                }
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "Redis 接続不可。Cookie セッションは in-memory 縮退");
                Self {
                    redis: None,
                    memory,
                }
            }
            Err(_) => {
                tracing::warn!("Redis 接続タイムアウト。Cookie セッションは in-memory 縮退");
                Self {
                    redis: None,
                    memory,
                }
            }
        }
    }

    /// Redis を使わない in-memory 専用ストアを作る（単一インスタンス構成/テスト向け）。
    ///
    /// [`connect`](Self::connect) が Redis 到達不能で返すのと同じ縮退状態を明示的に作る。
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            redis: None,
            memory: Arc::new(Mutex::new(MemoryStore::default())),
        }
    }

    /// セッションを**発行**し、生トークンを返す（Node `createSession`）。呼び出し側は Cookie で渡す。
    ///
    /// Redis 保存に失敗（または未接続）なら in-memory へフォールバックする（トークンは常に返る）。
    ///
    /// # Errors
    /// CSPRNG（トークン生成）失敗時のみ [`AuthError::Backend`]（弱いトークンを発行しない）。実質起こらない。
    pub async fn create(&self, user: &SessionUser, ttl_secs: u64) -> Result<String, AuthError> {
        let token = crate::token::generate_token().map_err(|_| AuthError::Backend)?;
        let hash = crate::sha256_hex(&token);
        // SessionUser は camelCase の固定形状で serialize は失敗し得ないが、lint 準拠で伝播する。
        let payload = serde_json::to_string(user).map_err(|_| AuthError::Backend)?;

        if let Some(cm) = &self.redis {
            let mut cm = cm.clone();
            match redis_create(&mut cm, &hash, &payload, &user.discord_id, ttl_secs).await {
                Ok(()) => return Ok(token),
                Err(e) => {
                    tracing::warn!(error = %e, "Redis へのセッション保存に失敗。in-memory へフォールバック");
                }
            }
        }
        self.memory_create(&hash, user, ttl_secs);
        Ok(token)
    }

    /// トークン（ハッシュ済み）からユーザーを解決し、TTL をスライディング更新する（Node `getSession`）。
    ///
    /// 引数 `token_hash` は生トークンの sha256hex。未ヒット・破損値・Redis 縮退は `Ok(None)`。
    ///
    /// # Errors
    /// Redis コマンド失敗**かつ** in-memory にも無い場合のみ [`AuthError::Backend`]（502・監視向け・
    /// 401 と区別）。in-memory に救済がある場合は `Ok(Some)` を返す（Node の in-memory 救済と一致）。
    pub async fn get(
        &self,
        token_hash: &str,
        ttl_secs: u64,
    ) -> Result<Option<SessionUser>, AuthError> {
        let ttl = i64::try_from(ttl_secs).unwrap_or(i64::MAX);
        let session_key = format!("session:{token_hash}");

        if let Some(cm) = &self.redis {
            let mut cm = cm.clone();
            match cm.get::<_, Option<String>>(&session_key).await {
                Ok(Some(json)) => {
                    // 破損値は無効扱い（検証専用の Rust は None で足りる）。
                    let Ok(user) = serde_json::from_str::<SessionUser>(&json) else {
                        return Ok(None);
                    };
                    let user_key = format!("user_sessions:{}", user.discord_id);
                    let _: Result<i64, _> = cm.expire(&session_key, ttl).await;
                    let _: Result<i64, _> = cm.expire(&user_key, ttl).await;
                    return Ok(Some(user));
                }
                // Redis 稼働中にキーが無い → in-memory を確認（Redis 一時停止中発行分の救済）。
                Ok(None) => {}
                Err(_) => {
                    // Redis 障害: in-memory に救済があれば返し、無ければ Backend（502 シグナル）。
                    return match self.memory_get(token_hash, ttl_secs) {
                        Some(user) => Ok(Some(user)),
                        None => Err(AuthError::Backend),
                    };
                }
            }
        }
        Ok(self.memory_get(token_hash, ttl_secs))
    }

    /// セッションを失効させる（Node `destroySession`）。Redis と in-memory の両方から削除する。
    pub async fn destroy(&self, token: &str) {
        let hash = crate::sha256_hex(token);
        let session_key = format!("session:{hash}");

        if let Some(cm) = &self.redis {
            let mut cm = cm.clone();
            // 発行済みセットからも除くため先にユーザーを特定する。
            let raw: Option<String> = cm.get(&session_key).await.unwrap_or(None);
            let _: Result<i64, _> = cm.del(&session_key).await;
            if let Some(json) = raw {
                if let Ok(user) = serde_json::from_str::<SessionUser>(&json) {
                    let user_key = format!("user_sessions:{}", user.discord_id);
                    let _: Result<i64, _> = cm.srem(&user_key, &hash).await;
                }
            }
        }
        self.memory_remove(&hash);
    }

    /// あるユーザーの発行済み全セッションを失効させる（Node `destroyAllSessionsForUser`）。
    ///
    /// ロール変更・ユーザー削除の即時反映に使う（陳腐化したセッション内ロールを断つ）。Redis と
    /// in-memory の双方から、`user_sessions:{id}` セットの全 token_hash に対応する `session:{hash}`
    /// を削除し、セット自体も除く。Redis 失敗は best-effort（`warn` のみ・in-memory は常に処理）。
    pub async fn destroy_all_for_user(&self, discord_id: &str) {
        let user_key = format!("user_sessions:{discord_id}");
        if let Some(cm) = &self.redis {
            let mut cm = cm.clone();
            match cm.smembers::<_, Vec<String>>(&user_key).await {
                Ok(hashes) => {
                    if !hashes.is_empty() {
                        let keys: Vec<String> =
                            hashes.iter().map(|h| format!("session:{h}")).collect();
                        let _: Result<i64, _> = cm.del(keys).await;
                    }
                    let _: Result<i64, _> = cm.del(&user_key).await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Redis の全セッション失効に失敗");
                }
            }
        }
        // in-memory 側も常に失効（Redis 縮退中に発行された分の救済）。
        let mut mem = self.memory();
        if let Some(set) = mem.user_sessions.remove(discord_id) {
            for hash in set {
                mem.sessions.remove(&hash);
            }
        }
    }

    fn memory(&self) -> MutexGuard<'_, MemoryStore> {
        self.memory.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn memory_create(&self, hash: &str, user: &SessionUser, ttl_secs: u64) {
        let mut mem = self.memory();
        mem.sessions.insert(
            hash.to_owned(),
            MemSession {
                user: user.clone(),
                expires_at: Instant::now() + Duration::from_secs(ttl_secs),
            },
        );
        mem.user_sessions
            .entry(user.discord_id.clone())
            .or_default()
            .insert(hash.to_owned());
    }

    /// in-memory から解決し、ヒット時は TTL をスライディング更新する。期限切れは削除して `None`。
    fn memory_get(&self, token_hash: &str, ttl_secs: u64) -> Option<SessionUser> {
        let now = Instant::now();
        let mut mem = self.memory();
        // 期限切れ判定（不変借用）→ 削除、を分けて借用衝突を避ける。
        let expired = match mem.sessions.get(token_hash) {
            None => return None,
            Some(e) => e.expires_at <= now,
        };
        if expired {
            self.remove_locked(&mut mem, token_hash);
            return None;
        }
        // 生存: スライディング更新して user を複製して返す（expect を使わず if let で束ねる）。
        if let Some(e) = mem.sessions.get_mut(token_hash) {
            e.expires_at = now + Duration::from_secs(ttl_secs);
            return Some(e.user.clone());
        }
        None
    }

    fn memory_remove(&self, hash: &str) {
        let mut mem = self.memory();
        self.remove_locked(&mut mem, hash);
    }

    /// ロック保持中に session + user_sessions 両方から token_hash を除く。
    fn remove_locked(&self, mem: &mut MemoryStore, hash: &str) {
        if let Some(e) = mem.sessions.remove(hash) {
            let uid = e.user.discord_id;
            if let Some(set) = mem.user_sessions.get_mut(&uid) {
                set.remove(hash);
                if set.is_empty() {
                    mem.user_sessions.remove(&uid);
                }
            }
        }
    }
}

/// Redis へセッションを書く（`SET session:{hash}` EX + `SADD user_sessions:{id}` + EXPIRE）。
async fn redis_create(
    cm: &mut ConnectionManager,
    hash: &str,
    payload: &str,
    discord_id: &str,
    ttl_secs: u64,
) -> redis::RedisResult<()> {
    let session_key = format!("session:{hash}");
    let user_key = format!("user_sessions:{discord_id}");
    let ttl_i64 = i64::try_from(ttl_secs).unwrap_or(i64::MAX);
    let _: () = cm.set_ex(&session_key, payload, ttl_secs).await?;
    let _: () = cm.sadd(&user_key, hash).await?;
    // 発行済みセットもセッションと同じだけ生存させる（アクセス毎に延長）。
    let _: () = cm.expire(&user_key, ttl_i64).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::SessionStore;
    use yuuka_types::{Role, SessionUser};

    /// Redis 未接続（in-memory のみ）のストアを作る。
    fn memory_only() -> SessionStore {
        SessionStore::in_memory()
    }

    fn user() -> SessionUser {
        SessionUser {
            discord_id: "123".to_owned(),
            username: "yuu".to_owned(),
            role: Role::User,
        }
    }

    #[tokio::test]
    async fn issue_then_verify_roundtrips_in_memory() {
        let store = memory_only();
        let token = store.create(&user(), 3600).await.expect("issued");
        // 発行トークンを sha256hex 化して get で解決できる（発行↔検証のキー整合）。
        let hash = crate::sha256_hex(&token);
        let got = store.get(&hash, 3600).await.expect("no backend err");
        assert_eq!(got.map(|u| u.discord_id), Some("123".to_owned()));
    }

    #[tokio::test]
    async fn destroy_removes_session() {
        let store = memory_only();
        let token = store.create(&user(), 3600).await.expect("issued");
        let hash = crate::sha256_hex(&token);
        store.destroy(&token).await;
        let got = store.get(&hash, 3600).await.expect("no backend err");
        assert!(got.is_none(), "失効後は解決できない");
    }

    #[tokio::test]
    async fn destroy_all_for_user_revokes_every_session() {
        let store = memory_only();
        // 同一ユーザーで 2 セッション発行。
        let t1 = store.create(&user(), 3600).await.expect("issued t1");
        let t2 = store.create(&user(), 3600).await.expect("issued t2");
        store.destroy_all_for_user("123").await;
        for t in [t1, t2] {
            let hash = crate::sha256_hex(&t);
            let got = store.get(&hash, 3600).await.expect("no backend err");
            assert!(got.is_none(), "一括失効後は解決できない");
        }
        // 別ユーザーのセッションは残る。
        let other = SessionUser {
            discord_id: "999".to_owned(),
            username: "z".to_owned(),
            role: Role::User,
        };
        let t3 = store.create(&other, 3600).await.expect("issued t3");
        store.destroy_all_for_user("123").await;
        let hash = crate::sha256_hex(&t3);
        assert!(store.get(&hash, 3600).await.expect("no err").is_some());
    }

    #[tokio::test]
    async fn expired_memory_session_is_none() {
        let store = memory_only();
        // TTL 0 秒で発行 → 即時失効扱い。
        let token = store.create(&user(), 0).await.expect("issued");
        let hash = crate::sha256_hex(&token);
        // わずかに待つ必要なく、expires_at <= now で None（0 秒なので now と同時）。
        let got = store.get(&hash, 0).await.expect("no backend err");
        assert!(got.is_none());
    }
}
