//! `CredentialRepo` — `UserScope` 束縛のデータアクセス（分離キーを型で強制・§12.2 契約5）。
//!
//! `credentials` テーブルは `(user_id, service_name)` 複合 PK で **`bot_id` を持たない**
//! （bot への利用許可は別表 `bot_credential_access`。本コア CRUD では扱わず deferred）。
//! よって分離キーは **`user_id` のみ**を `WHERE` に必須化する（Node `credentialRepo.ts` と一致）。
//! `&UserScope` を取ることで「user_id 無しクエリ」を型で不能化する。読みは [`ReadPool`]、
//! 書きは [`WriterHandle`]（BEGIN IMMEDIATE）へ送る。
//!
//! **機密フェイルクローズ**: 一覧は Node `listCredentials` と同じく暗号化列
//! （`encrypted_password` / `iv` / `auth_tag`）を**決して SELECT しない**（§6.4）。

use rusqlite::{params, Row};
use yuuka_core::scope::ScopedRepo;
use yuuka_core::{DbError, UserScope};
use yuuka_db::{map_sqlite, ReadPool, WriterHandle};
use yuuka_web::Db;

use crate::dto::Credential;

/// 一覧の返却列（クリーンビュー・暗号化列と user_id は含めない）。
const CREDENTIAL_COLUMNS: &str = "service_name, username, url, updated_at";

/// credential リポジトリ（DB ハンドルを借用する軽量ラッパ・per-request 構築）。
pub struct CredentialRepo<'a> {
    read: &'a ReadPool,
    writer: &'a WriterHandle,
}

impl ScopedRepo for CredentialRepo<'_> {}

impl<'a> CredentialRepo<'a> {
    /// 共有 DB ハンドルから構築する。
    #[must_use]
    pub fn new(db: &'a Db) -> Self {
        Self {
            read: &db.read,
            writer: &db.writer,
        }
    }

    /// スコープ内の全認証情報を service_name 昇順で返す（暗号化列は含めない）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn list(&self, scope: &UserScope) -> Result<Vec<Credential>, DbError> {
        let uid = scope_user(scope);
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {CREDENTIAL_COLUMNS} FROM credentials \
                     WHERE user_id = ?1 ORDER BY service_name ASC"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![uid], row_to_credential)
                    .map_err(map_sqlite)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(map_sqlite)?);
                }
                Ok(out)
            })
            .await
    }

    /// スコープ内の単一認証情報インデックスを取得する（暗号化列は含めない・無ければ `None`）。
    ///
    /// service_name は正規化（trim + 小文字化）して照合する（Node と一致）。
    ///
    /// # Errors
    /// クエリ失敗時 [`DbError`]。
    pub async fn get(
        &self,
        scope: &UserScope,
        service_name: &str,
    ) -> Result<Option<Credential>, DbError> {
        let uid = scope_user(scope);
        let svc = normalize_service_name(service_name);
        self.read
            .read(move |conn| {
                let sql = format!(
                    "SELECT {CREDENTIAL_COLUMNS} FROM credentials \
                     WHERE user_id = ?1 AND service_name = ?2"
                );
                let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
                let mut rows = stmt
                    .query_map(params![uid, svc], row_to_credential)
                    .map_err(map_sqlite)?;
                match rows.next() {
                    Some(row) => Ok(Some(row.map_err(map_sqlite)?)),
                    None => Ok(None),
                }
            })
            .await
    }

    /// 認証情報を完全削除する（削除できたら `true`）。
    ///
    /// service_name は正規化（trim + 小文字化）して照合する（Node `deleteCredential` と一致）。
    ///
    /// # Errors
    /// 削除失敗時 [`DbError`]。
    pub async fn delete(&self, scope: &UserScope, service_name: &str) -> Result<bool, DbError> {
        let uid = scope_user(scope);
        let svc = normalize_service_name(service_name);
        self.writer
            .transaction(move |tx| {
                let n = tx
                    .execute(
                        "DELETE FROM credentials WHERE user_id = ?1 AND service_name = ?2",
                        params![uid, svc],
                    )
                    .map_err(map_sqlite)?;
                Ok(n > 0)
            })
            .await
    }

    /// ユーザーの暗号化 salt（`users.salt` hex）を返す（無ければ `None`・ユーザー鍵導出に使う）。
    ///
    /// # Errors
    /// 読み取り失敗時 [`DbError`]。
    pub async fn user_salt(&self, user_id: &str) -> Result<Option<String>, DbError> {
        let uid = user_id.to_owned();
        self.read
            .read(move |conn| {
                let mut stmt = conn
                    .prepare("SELECT salt FROM users WHERE discord_id = ?1")
                    .map_err(map_sqlite)?;
                let mut rows = stmt
                    .query_map(params![uid], |row| row.get::<_, String>(0))
                    .map_err(map_sqlite)?;
                match rows.next() {
                    Some(v) => Ok(Some(v.map_err(map_sqlite)?)),
                    None => Ok(None),
                }
            })
            .await
    }

    /// 認証情報を保存する（Node `saveCredential`＝upsert・暗号文/iv/tag は呼び出し側で暗号化済み）。
    /// service_name は正規化して保存する。
    ///
    /// # Errors
    /// 書き込み失敗時 [`DbError`]。
    pub async fn save(
        &self,
        scope: &UserScope,
        service_name: &str,
        username: String,
        url: Option<String>,
        enc: yuuka_crypto::Encrypted,
    ) -> Result<(), DbError> {
        let uid = scope_user(scope);
        let svc = normalize_service_name(service_name);
        let yuuka_crypto::Encrypted {
            encrypted,
            iv,
            auth_tag,
        } = enc;
        self.writer
            .transaction(move |tx| {
                tx.execute(
                    "INSERT INTO credentials \
                       (user_id, service_name, url, username, encrypted_password, iv, auth_tag, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now', 'localtime')) \
                     ON CONFLICT(user_id, service_name) DO UPDATE SET \
                       url = excluded.url, username = excluded.username, \
                       encrypted_password = excluded.encrypted_password, iv = excluded.iv, \
                       auth_tag = excluded.auth_tag, updated_at = datetime('now', 'localtime')",
                    params![uid, svc, url, username, encrypted, iv, auth_tag],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
    }

    /// 更新の部分マージ用に、暗号化列を含む現在値を取得する（無ければ `None`）。
    ///
    /// # Errors
    /// 読み取り失敗時 [`DbError`]。
    #[allow(clippy::type_complexity)]
    pub async fn get_full(
        &self,
        scope: &UserScope,
        service_name: &str,
    ) -> Result<Option<(String, Option<String>, String, String, String)>, DbError> {
        let uid = scope_user(scope);
        let svc = normalize_service_name(service_name);
        self.read
            .read(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT username, url, encrypted_password, iv, auth_tag FROM credentials \
                         WHERE user_id = ?1 AND service_name = ?2",
                    )
                    .map_err(map_sqlite)?;
                let mut rows = stmt
                    .query_map(params![uid, svc], |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    })
                    .map_err(map_sqlite)?;
                match rows.next() {
                    Some(v) => Ok(Some(v.map_err(map_sqlite)?)),
                    None => Ok(None),
                }
            })
            .await
    }
}

/// サービス名の正規化（trim + 小文字化）。Node `normalizeServiceName` と同一。
pub(crate) fn normalize_service_name(service_name: &str) -> String {
    service_name.trim().to_lowercase()
}

/// 認証情報フィールド長の上限（DoS・肥大化対策・Node `secretService` `assertFieldLengths`）。
pub(crate) const MAX_SERVICE_NAME: usize = 128;
pub(crate) const MAX_USERNAME: usize = 256;
pub(crate) const MAX_PASSWORD: usize = 1024;
pub(crate) const MAX_URL: usize = 2048;

/// フィールド長を検証する（超過時はユーザー向けエラー文言・Node `assertFieldLengths` パリティ）。
/// route（register）と tool（add/update）で共有する。文字数（`chars().count()`）で判定する。
pub(crate) fn check_field_lengths(
    service: &str,
    username: Option<&str>,
    password: Option<&str>,
    url: Option<&str>,
) -> Option<String> {
    if service.chars().count() > MAX_SERVICE_NAME {
        return Some(format!(
            "サービス名が長すぎます（最大{MAX_SERVICE_NAME}文字）。"
        ));
    }
    if username.is_some_and(|u| u.chars().count() > MAX_USERNAME) {
        return Some(format!(
            "ユーザー名が長すぎます（最大{MAX_USERNAME}文字）。"
        ));
    }
    if password.is_some_and(|p| p.chars().count() > MAX_PASSWORD) {
        return Some(format!(
            "パスワードが長すぎます（最大{MAX_PASSWORD}文字）。"
        ));
    }
    if url.is_some_and(|u| u.chars().count() > MAX_URL) {
        return Some(format!("URLが長すぎます（最大{MAX_URL}文字）。"));
    }
    None
}

/// スコープから所有 String の user_id を取り出す（`spawn_blocking` の `'static` クロージャ用）。
fn scope_user(scope: &UserScope) -> String {
    scope.user_id().as_str().to_owned()
}

/// SQLite 行を [`Credential`] へ変換する（暗号化列は SELECT しないため触れない）。
fn row_to_credential(row: &Row) -> rusqlite::Result<Credential> {
    Ok(Credential {
        service_name: row.get("service_name")?,
        username: row.get("username")?,
        url: row.get("url")?,
        updated_at: row.get("updated_at")?,
    })
}
