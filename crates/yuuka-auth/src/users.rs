//! `users` テーブルの認証向け CRUD（Node `src/db/userRepo.ts` パリティ）。
//!
//! 本人認証は OIDC プロバイダー（標準は sk-account）だけが担い、Yuuka は認証情報（パスワード等）を一切持たない。
//! `users.salt` は Argon2id ユーザー鍵導出用の **16 バイト CSPRNG hex**
//! （[`yuuka_crypto::generate_user_salt`]）を生成時に一度だけ設定し、以後変更しない。

use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, OptionalExtension};
use yuuka_core::{DbError, RegistrationMode};
use yuuka_crypto::generate_user_salt;
use yuuka_db::map_sqlite;
use yuuka_types::Role;
use yuuka_web::Db;

/// 認証に必要な最小のユーザー行。
#[derive(Debug, Clone)]
pub struct AuthUser {
    /// Discord ID（PK）。
    pub discord_id: String,
    /// 表示名。
    pub username: String,
    /// 権限ロール（生文字列 `'user'`/`'admin'`）。
    pub role: String,
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

/// [`Role`] を `users.role` 文字列へ（新規作成の INSERT 用）。
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
pub async fn get_user_by_discord_id(
    db: &Db,
    discord_id: &str,
) -> Result<Option<AuthUser>, DbError> {
    let id = discord_id.to_owned();
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT id, username, role FROM users WHERE id = ?1",
                params![id],
                |r| {
                    Ok(AuthUser {
                        discord_id: r.get(0)?,
                        username: r.get(1)?,
                        role: r.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// OIDC の `sub`（`users.sk_account_id`）から利用者 ID（`users.id`）を引く。不在は `Ok(None)`。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn user_id_by_subject(db: &Db, sk_account_id: &str) -> Result<Option<String>, DbError> {
    let id = sk_account_id.to_owned();
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT id FROM users WHERE sk_account_id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// ログイン時に初期 admin を決める指定と、新規登録の扱い。
///
/// 管理者は `ADMIN_DISCORD_IDS`（Discord ID）・`ADMIN_OIDC_SUBJECTS`（OIDC の `sub`）・`ADMIN_EMAILS`
/// （確認済みのメール）のどれかで明示した人だけにする。既存利用者のロールはログインで変えない
/// （docs/design/optional_discord.md §4.2）。
#[derive(Debug, Clone, Default)]
pub struct LoginPolicy {
    /// `ADMIN_DISCORD_IDS`。
    pub admin_discord_ids: Vec<String>,
    /// `ADMIN_OIDC_SUBJECTS`。
    pub admin_subjects: Vec<String>,
    /// `ADMIN_EMAILS`（大文字小文字は区別しない）。
    pub admin_emails: Vec<String>,
    /// `REGISTRATION_MODE`。
    pub registration: RegistrationMode,
}

impl LoginPolicy {
    /// 初期 admin の指定が 1 つでもあるか。
    #[must_use]
    pub fn has_admin(&self) -> bool {
        !(self.admin_discord_ids.is_empty()
            && self.admin_subjects.is_empty()
            && self.admin_emails.is_empty())
    }

    fn is_admin(&self, identity: &LoginIdentity) -> bool {
        self.admin_subjects.iter().any(|s| s == &identity.subject)
            || identity
                .discord_id
                .as_ref()
                .is_some_and(|d| self.admin_discord_ids.iter().any(|a| a == d))
            || identity
                .email
                .as_ref()
                .is_some_and(|e| self.admin_emails.iter().any(|a| a.eq_ignore_ascii_case(e)))
    }
}

/// OIDC プロバイダーが確かめた本人の情報。
#[derive(Debug, Clone)]
pub struct LoginIdentity {
    /// OIDC の `sub`（`users.sk_account_id`）。
    pub subject: String,
    /// Discord ID（ID トークンの claim にあれば）。
    pub discord_id: Option<String>,
    /// 確認済みのメールアドレス。
    pub email: Option<String>,
    /// 表示名。
    pub username: String,
}

/// ログインで確定した利用者。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginUser {
    /// 利用者 ID（`users.id`）。
    pub user_id: String,
    /// ロール。
    pub role: Role,
}

/// ログインを断る理由（利用者へそのまま見せる文言を持つ）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginRejection {
    /// Discord アカウントや OIDC アカウントが、別の利用者に連携済み。統合はしない（§9）。
    Conflict(&'static str),
    /// `REGISTRATION_MODE=closed` で、初めてログインした人。
    RegistrationClosed,
    /// `REGISTRATION_MODE=invite` で、招待コードを持たずに初めてログインした人。
    InviteRequired,
    /// 招待コードが見つからない・使用済み・期限切れ。
    InviteInvalid,
}

impl LoginRejection {
    /// 利用者へ見せる文言。
    #[must_use]
    pub fn message(&self) -> &'static str {
        match self {
            Self::Conflict(message) => *message,
            Self::RegistrationClosed => {
                "新規登録は受け付けていません。管理者に問い合わせてください。"
            }
            Self::InviteRequired => {
                "新規登録には招待コードが必要です。管理者から届いた招待リンクを開いてログインしてください。"
            }
            Self::InviteInvalid => {
                "招待コードが無効か、使用済み・期限切れです。管理者に新しい招待を依頼してください。"
            }
        }
    }
}

const DISCORD_TAKEN: &str =
    "この Discord アカウントは別のユーザーに連携されています。管理者に問い合わせてください。";
const DISCORD_MISMATCH: &str =
    "このアカウントは別の Discord アカウントに連携されています。管理者に問い合わせてください。";

/// OIDC プロバイダーが確定した利用者（`sub`）を Yuuka の利用者へ結び付ける（§4.1）。
///
/// 1. `sub`（列名は sk-account 時代のまま `sk_account_id`）で利用者を引く。claim に Discord ID が
///    あり、まだ連携していなければ連携する。
/// 2. 見つからなければ、claim の Discord ID で、まだ `sub` に結び付いていない利用者（旧環境から移した
///    行など）を一度だけ引き受ける。
/// 3. それでも無ければ、`REGISTRATION_MODE` が許すときだけ新しく作る（`invite` は招待コードが要る。
///    `ADMIN_*` で明示した管理者は、どの方式でも登録できる）。利用者 ID は、Discord ID が
///    あればそれと同じ値、無ければ UUIDv7。Discord 側の境界（yuuka-discord の `identity`）が
///    `users.discord_id` を引くようになるまでは、連携済みの利用者は `id = discord_id` に保つ（§8-6）。
///
/// Discord アカウントの衝突は統合せずに断る。表示名は毎回プロバイダーの値で更新する。
///
/// # Errors
/// salt・ID の生成失敗、DB 失敗時 [`DbError`]。断るときは `Ok(Err(LoginRejection))`。
pub async fn upsert_from_oidc(
    db: &Db,
    identity: &LoginIdentity,
    policy: &LoginPolicy,
    invite: Option<&str>,
) -> Result<Result<LoginUser, LoginRejection>, DbError> {
    let salt = generate_user_salt().map_err(|e| DbError::Operation(format!("salt 生成: {e}")))?;
    let new_id = new_user_id()?;
    let is_admin = policy.is_admin(identity);
    let registration = policy.registration;
    let identity = identity.clone();
    let invite = invite.map(str::to_owned);
    db.writer
        .transaction(move |tx| {
            let LoginIdentity {
                subject,
                discord_id,
                username,
                ..
            } = identity;

            // 1) sub で引く。
            let linked: Option<(String, String, Option<String>)> = tx
                .query_row(
                    "SELECT id, role, discord_id FROM users WHERE sk_account_id = ?1",
                    params![subject],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()
                .map_err(map_sqlite)?;
            if let Some((user_id, role, linked_discord)) = linked {
                match (linked_discord.as_deref(), discord_id.as_deref()) {
                    (Some(have), Some(claim)) if have != claim => {
                        return Ok(Err(LoginRejection::Conflict(DISCORD_MISMATCH)));
                    }
                    (None, Some(claim)) => {
                        let taken: bool = tx
                            .query_row(
                                "SELECT EXISTS(SELECT 1 FROM users WHERE discord_id = ?1 AND id != ?2)",
                                params![claim, user_id],
                                |r| r.get(0),
                            )
                            .map_err(map_sqlite)?;
                        if taken {
                            return Ok(Err(LoginRejection::Conflict(DISCORD_TAKEN)));
                        }
                        tx.execute(
                            "UPDATE users SET discord_id = ?1, discord_linked_at = datetime('now','localtime') \
                             WHERE id = ?2",
                            params![claim, user_id],
                        )
                        .map_err(map_sqlite)?;
                    }
                    _ => {}
                }
                tx.execute(
                    "UPDATE users SET username = ?1, updated_at = datetime('now','localtime') WHERE id = ?2",
                    params![username, user_id],
                )
                .map_err(map_sqlite)?;
                return Ok(Ok(LoginUser {
                    user_id,
                    role: role_from_str(&role),
                }));
            }

            // 2) claim の Discord ID で、sub に未紐付けの利用者を引き受ける。
            if let Some(claim) = discord_id.as_deref() {
                let existing: Option<(String, String, Option<String>)> = tx
                    .query_row(
                        "SELECT id, role, sk_account_id FROM users WHERE discord_id = ?1",
                        params![claim],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()
                    .map_err(map_sqlite)?;
                if let Some((user_id, role, other_link)) = existing {
                    // 同じ Discord ID が別の OIDC アカウントへ結び付いていれば、乗っ取りとして断る。
                    if other_link.is_some_and(|other| other != subject) {
                        return Ok(Err(LoginRejection::Conflict(DISCORD_TAKEN)));
                    }
                    tx.execute(
                        "UPDATE users SET sk_account_id = ?1, username = ?2, updated_at = datetime('now','localtime') \
                         WHERE id = ?3",
                        params![subject, username, user_id],
                    )
                    .map_err(map_sqlite)?;
                    return Ok(Ok(LoginUser {
                        user_id,
                        role: role_from_str(&role),
                    }));
                }
            }

            // 3) 新規登録。招待制なら、利用者の作成と同じトランザクションでコードを予約する。
            // ADMIN_* で明示した管理者は、招待コードを発行する人がまだいない初回でも入れるよう、どの方式でも登録できる。
            let reserved = match registration {
                _ if is_admin => None,
                RegistrationMode::Closed => return Ok(Err(LoginRejection::RegistrationClosed)),
                RegistrationMode::Open => None,
                RegistrationMode::Invite => {
                    let Some(code) = invite else {
                        return Ok(Err(LoginRejection::InviteRequired));
                    };
                    if !crate::invite::reserve(tx, &code)? {
                        return Ok(Err(LoginRejection::InviteInvalid));
                    }
                    Some(code)
                }
            };
            let role = if is_admin { Role::Admin } else { Role::User };
            let user_id = discord_id.clone().unwrap_or(new_id);
            tx.execute(
                "INSERT INTO users (id, discord_id, discord_linked_at, username, salt, role, sk_account_id) \
                 VALUES (?1, ?2, CASE WHEN ?2 IS NULL THEN NULL ELSE datetime('now','localtime') END, ?3, ?4, ?5, ?6)",
                params![user_id, discord_id, username, salt, role_text(role), subject],
            )
            .map_err(map_sqlite)?;
            if let Some(code) = reserved {
                crate::invite::mark_used(tx, &code, &user_id)?;
            }
            Ok(Ok(LoginUser { user_id, role }))
        })
        .await
}

/// 新しい利用者 ID（UUIDv7・小文字の 8-4-4-4-12 形式）。
///
/// # Errors
/// 乱数を得られないとき [`DbError`]。
pub fn new_user_id() -> Result<String, DbError> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes)
        .map_err(|e| DbError::Operation(format!("利用者 ID の乱数: {e}")))?;
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
    Ok(format_uuid_v7(bytes, millis))
}

/// 乱数 16 バイトの先頭 48 ビットへ UNIX ミリ秒を入れ、版（7）と変種（RFC 9562）を立てる。
fn format_uuid_v7(mut bytes: [u8; 16], millis: u64) -> String {
    for (dst, src) in bytes.iter_mut().zip(millis.to_be_bytes().iter().skip(2)) {
        *dst = *src;
    }
    if let Some(b) = bytes.get_mut(6) {
        *b = (*b & 0x0f) | 0x70;
    }
    if let Some(b) = bytes.get_mut(8) {
        *b = (*b & 0x3f) | 0x80;
    }
    let mut out = String::with_capacity(36);
    for (i, b) in bytes.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            out.push('-');
        }
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_v7_has_time_prefix_version_and_variant() {
        let id = format_uuid_v7([0xff; 16], 0x0123_4567_89ab);
        assert_eq!(id, "01234567-89ab-7fff-bfff-ffffffffffff");
        let fresh = new_user_id().unwrap();
        assert_eq!(fresh.len(), 36);
        assert_eq!(fresh.chars().nth(14), Some('7'));
        assert_ne!(fresh, new_user_id().unwrap());
    }

    #[test]
    fn admin_is_matched_by_subject_discord_or_verified_email() {
        let policy = LoginPolicy {
            admin_discord_ids: vec!["100".to_owned()],
            admin_subjects: vec!["sub-admin".to_owned()],
            admin_emails: vec!["Owner@Example.com".to_owned()],
            registration: RegistrationMode::Open,
        };
        let who = |subject: &str, discord: Option<&str>, email: Option<&str>| LoginIdentity {
            subject: subject.to_owned(),
            discord_id: discord.map(str::to_owned),
            email: email.map(str::to_owned),
            username: "u".to_owned(),
        };
        assert!(policy.is_admin(&who("sub-admin", None, None)));
        assert!(policy.is_admin(&who("x", Some("100"), None)));
        assert!(policy.is_admin(&who("x", None, Some("owner@example.com"))));
        assert!(!policy.is_admin(&who("x", Some("200"), Some("other@example.com"))));
        assert!(policy.has_admin());
        assert!(!LoginPolicy::default().has_admin());
    }
}
