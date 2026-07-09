//! `users` テーブルの認証向け CRUD（Node `src/db/userRepo.ts` パリティ）。
//!
//! パスワードは **bcrypt cost 12（`$2b$`・Node bcryptjs v3 と相互運用）**。`users.salt` は bcrypt の
//! salt とは別物で、Argon2id ユーザー鍵導出用の **16 バイト CSPRNG hex**（[`yuuka_crypto::generate_user_salt`]）
//! を生成時に一度だけ設定し、以後変更しない。bcrypt は CPU 集約的（cost 12 ≒ 数百 ms）なので
//! `spawn_blocking` で回し、async ランタイムをブロックしない。

use rusqlite::{params, OptionalExtension};
use yuuka_crypto::{generate_user_salt, Encrypted};
use yuuka_db::map_sqlite;
use yuuka_core::DbError;
use yuuka_types::Role;
use yuuka_web::Db;

/// bcrypt コスト係数（Node `BCRYPT_COST = 12` と一致）。変更時は [`DUMMY_PASSWORD_HASH`] も再生成する。
const BCRYPT_COST: u32 = 12;

/// ユーザー不在時に**同等の bcrypt 比較時間**を消費するためのダミーハッシュ（cost 12・`$2b$`）。
/// Node `userRepo.ts` の `DUMMY_PASSWORD_HASH` と同一値（アカウント列挙のタイミングオラクル対策）。
const DUMMY_PASSWORD_HASH: &str = "$2b$12$tvcUPxX5xmqpVZCS6aSDQe7WKkrXvMd8batVtwbJFI1uJ42EzpGlG";

/// 認証に必要な最小のユーザー行（秘密は `password_hash` のみ・DTO には絶対露出しない）。
#[derive(Debug, Clone)]
pub struct AuthUser {
    /// Discord ID（PK）。
    pub discord_id: String,
    /// 表示名。
    pub username: String,
    /// bcrypt ハッシュ（`$2b$…`）。
    pub password_hash: String,
    /// 権限ロール（生文字列 `'user'`/`'admin'`）。
    pub role: String,
}

impl AuthUser {
    /// `users.role` 生文字列を [`Role`] へ写像する（未知は `User`・Node `role || "user"` 相当）。
    #[must_use]
    pub fn role_enum(&self) -> Role {
        role_from_str(&self.role)
    }
}

/// `users.role`（`'user'`/`'admin'`）を [`Role`] へ（未知は `User`）。
#[must_use]
pub fn role_from_str(raw: &str) -> Role {
    if raw.eq_ignore_ascii_case("admin") {
        Role::Admin
    } else {
        Role::User
    }
}

/// [`Role`] を `users.role` 文字列へ（`create_user` の INSERT 用）。
fn role_text(role: Role) -> &'static str {
    match role {
        Role::Admin => "admin",
        Role::User => "user",
    }
}

/// Discord ID でユーザーを引く（Node `getUserByDiscordId`）。不在は `Ok(None)`。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_user_by_discord_id(db: &Db, discord_id: &str) -> Result<Option<AuthUser>, DbError> {
    let id = discord_id.to_owned();
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT discord_id, username, password_hash, role FROM users WHERE discord_id = ?1",
                params![id],
                |r| {
                    Ok(AuthUser {
                        discord_id: r.get(0)?,
                        username: r.get(1)?,
                        password_hash: r.get(2)?,
                        role: r.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// 登録済みユーザー数（Node `listAllUsers().length` の needSetup 判定に使う）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn count_users(db: &Db) -> Result<i64, DbError> {
    db.read
        .read(|conn| {
            conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get::<_, i64>(0))
                .map_err(map_sqlite)
        })
        .await
}

/// パスワードを bcrypt（cost 12）でハッシュする（`spawn_blocking` 上・`$2b$`）。
///
/// # Errors
/// bcrypt 失敗・join 失敗時 [`DbError::Operation`]。
async fn hash_password(password: String) -> Result<String, DbError> {
    tokio::task::spawn_blocking(move || bcrypt::hash(password, BCRYPT_COST))
        .await
        .map_err(|e| DbError::Operation(format!("bcrypt join: {e}")))?
        .map_err(|e| DbError::Operation(format!("bcrypt hash: {e}")))
}

/// パスワードを**一定時間**で検証する（Node `verifyPasswordConstantTime`）。
///
/// `stored_hash` が `None`（ユーザー不在）でもダミーハッシュとの bcrypt 比較を実行して
/// 応答時間差を消し、`false` を返す（アカウント列挙のタイミングオラクル対策）。bcrypt は
/// `spawn_blocking` で回す。破損ハッシュ（旧 scrypt 等）は `false`（Node の try/catch 相当）。
pub async fn verify_password_constant_time(password: String, stored_hash: Option<String>) -> bool {
    tokio::task::spawn_blocking(move || match stored_hash {
        Some(h) => bcrypt::verify(&password, &h).unwrap_or(false),
        None => {
            // 結果は捨てるが、不在ユーザーでも同等の比較コストを必ず消費する。
            let _ = bcrypt::verify(&password, DUMMY_PASSWORD_HASH);
            false
        }
    })
    .await
    .unwrap_or(false)
}

/// ユーザーを作成する（Node `createUser`）。最初のユーザー、または `admin_ids` に含まれる ID は
/// **admin** ロールになる。`salt`（Argon2id 用 16B hex）を生成し、bcrypt ハッシュと共に 5 列を INSERT する。
///
/// COUNT→INSERT は単一 writer トランザクション内で行い、初回 admin 判定の競合を閉じる（Node の
/// `db.transaction` 相当）。返り値は付与されたロール（発行するセッションに使う）。
///
/// # Errors
/// 一意制約違反（discord_id 重複・username 重複）は [`DbError::Operation`]、bcrypt/salt/DB 失敗も同様。
pub async fn create_user(
    db: &Db,
    discord_id: &str,
    username: &str,
    password: &str,
    admin_ids: &[String],
) -> Result<Role, DbError> {
    // async な導出（bcrypt/CSPRNG）は writer クロージャの外で先に済ませる（クロージャは同期）。
    let hash = hash_password(password.to_owned()).await?;
    let salt = generate_user_salt().map_err(|e| DbError::Operation(format!("salt 生成: {e}")))?;

    let discord_id = discord_id.to_owned();
    let username = username.to_owned();
    let is_admin_id = admin_ids.iter().any(|id| id == &discord_id);

    db.writer
        .transaction(move |tx| {
            let count: i64 = tx
                .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
                .map_err(map_sqlite)?;
            let role = if count == 0 || is_admin_id {
                Role::Admin
            } else {
                Role::User
            };
            tx.execute(
                "INSERT INTO users (discord_id, username, password_hash, salt, role) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![discord_id, username, hash, salt, role_text(role)],
            )
            .map_err(map_sqlite)?;
            Ok(role)
        })
        .await
}

/// Gemini API 設定（暗号化キー + モデル）を更新する（Node `updateUserGeminiSettings`）。
///
/// `enc` は [`yuuka_crypto::SystemCrypto::encrypt_text`] の出力（`encrypted`/`iv`/`auth_tag` の hex）。
/// DB カラムは `gemini_api_key_encrypted`/`_iv`/`_tag`（`auth_tag` → `_tag` 列に対応）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn update_user_gemini_settings(
    db: &Db,
    discord_id: &str,
    enc: &Encrypted,
    model: &str,
) -> Result<(), DbError> {
    let discord_id = discord_id.to_owned();
    let encrypted = enc.encrypted.clone();
    let iv = enc.iv.clone();
    let tag = enc.auth_tag.clone();
    let model = model.to_owned();
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "UPDATE users SET gemini_api_key_encrypted = ?1, gemini_api_key_iv = ?2, \
                 gemini_api_key_tag = ?3, gemini_model = ?4, updated_at = datetime('now', 'localtime') \
                 WHERE discord_id = ?5",
                params![encrypted, iv, tag, model, discord_id],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::{verify_password_constant_time, BCRYPT_COST, DUMMY_PASSWORD_HASH};

    #[test]
    fn dummy_hash_is_valid_bcrypt() {
        // ダミーハッシュが cost 12 の妥当な bcrypt であること（検証で panic せず false を返す）。
        assert!(DUMMY_PASSWORD_HASH.starts_with("$2b$12$"));
        assert_eq!(BCRYPT_COST, 12);
        // 任意パスワードとは一致しない（が検証自体は成立する）。
        assert!(!bcrypt::verify("anything", DUMMY_PASSWORD_HASH).unwrap());
    }

    #[tokio::test]
    async fn verify_round_trips_and_rejects_absent_user() {
        // Rust が作った $2b$ ハッシュを Rust が検証できる（＝Node とも相互運用可能な形式）。
        let hash = bcrypt::hash("Correct-horse-1", BCRYPT_COST).unwrap();
        assert!(hash.starts_with("$2b$"));
        assert!(verify_password_constant_time("Correct-horse-1".to_owned(), Some(hash.clone())).await);
        assert!(!verify_password_constant_time("wrong".to_owned(), Some(hash)).await);
        // 不在ユーザー（None）は常に false（例外を投げずダミー比較を消費）。
        assert!(!verify_password_constant_time("anything".to_owned(), None).await);
        // 破損ハッシュ（旧 scrypt 形式）は false（Node の try/catch 相当）。
        assert!(
            !verify_password_constant_time("x".to_owned(), Some("not-a-bcrypt-hash".to_owned())).await
        );
    }
}
