//! 招待コード（`REGISTRATION_MODE=invite`・docs/design/optional_discord.md §4.1）。
//!
//! 管理者が発行し、初めてログインする人がログイン開始時（`/api/auth/oidc/start?invite=...`）に渡す。
//! コードは 1 回だけ使え、期限を付けられる。使うときは利用者の作成と同じトランザクションで
//! 予約・記録するので、同じコードで 2 人が登録することはない。

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rusqlite::{params, OptionalExtension, Transaction};
use serde::Serialize;
use yuuka_core::DbError;
use yuuka_db::map_sqlite;
use yuuka_web::Db;

/// 受け付けるコードの長さ（発行するのは 16 文字）。
const MIN_LEN: usize = 8;
const MAX_LEN: usize = 64;
/// 期限の上限（日）。
pub const MAX_EXPIRES_DAYS: u32 = 365;

/// 招待コード 1 件（管理画面の一覧）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteCode {
    pub code: String,
    pub note: Option<String>,
    pub created_by: Option<String>,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub used_by: Option<String>,
    pub used_at: Option<String>,
}

/// 利用者から受け取ったコードを整える。形が合わなければ `None`（URL セーフ base64 の文字だけ）。
#[must_use]
pub fn normalize(raw: &str) -> Option<String> {
    let code = raw.trim();
    let ok_len = (MIN_LEN..=MAX_LEN).contains(&code.len());
    let ok_chars = code
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    (ok_len && ok_chars).then(|| code.to_owned())
}

/// 新しいコード（12 バイトの CSPRNG を URL セーフ base64 にした 16 文字）。
///
/// # Errors
/// 乱数を得られないとき [`DbError`]。
pub fn new_code() -> Result<String, DbError> {
    let mut bytes = [0u8; 12];
    getrandom::getrandom(&mut bytes)
        .map_err(|e| DbError::Operation(format!("招待コードの乱数: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// 招待コードを発行する。`expires_in_days` は 1〜[`MAX_EXPIRES_DAYS`] に丸める。
///
/// # Errors
/// DB 失敗時 [`DbError`]。
pub async fn create(
    db: &Db,
    created_by: &str,
    note: Option<String>,
    expires_in_days: Option<u32>,
) -> Result<InviteCode, DbError> {
    let code = new_code()?;
    let created_by = created_by.to_owned();
    let note = note
        .map(|n| n.trim().chars().take(200).collect::<String>())
        .filter(|n| !n.is_empty());
    let days = expires_in_days.map(|d| d.clamp(1, MAX_EXPIRES_DAYS));
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT INTO invite_codes (code, note, created_by, expires_at) VALUES \
                 (?1, ?2, ?3, CASE WHEN ?4 IS NULL THEN NULL \
                  ELSE datetime('now', 'localtime', '+' || ?4 || ' days') END)",
                params![code, note, created_by, days],
            )
            .map_err(map_sqlite)?;
            get_in(tx, &code)?.ok_or_else(|| DbError::Operation("招待コードの作成".to_owned()))
        })
        .await
}

/// 招待コードの一覧（新しい順）。
///
/// # Errors
/// DB 失敗時 [`DbError`]。
pub async fn list(db: &Db) -> Result<Vec<InviteCode>, DbError> {
    db.read
        .read(|conn| {
            let mut stmt = conn
                .prepare(&format!("{SELECT} ORDER BY created_at DESC, code"))
                .map_err(map_sqlite)?;
            let rows = stmt.query_map([], row).map_err(map_sqlite)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(map_sqlite)
        })
        .await
}

/// 招待コードを削除する。無ければ `false`。
///
/// # Errors
/// DB 失敗時 [`DbError`]。
pub async fn delete(db: &Db, code: &str) -> Result<bool, DbError> {
    let code = code.to_owned();
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute("DELETE FROM invite_codes WHERE code = ?1", params![code])
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

/// 未使用・期限内のコードを予約する（利用者の作成と同じトランザクションで呼ぶ）。使えなければ `false`。
pub(crate) fn reserve(tx: &Transaction<'_>, code: &str) -> Result<bool, DbError> {
    let n = tx
        .execute(
            "UPDATE invite_codes SET used_at = datetime('now', 'localtime') \
             WHERE code = ?1 AND used_at IS NULL \
               AND (expires_at IS NULL OR expires_at > datetime('now', 'localtime'))",
            params![code],
        )
        .map_err(map_sqlite)?;
    Ok(n == 1)
}

/// 予約したコードに利用者を記録する（利用者の行を作った後に呼ぶ）。
pub(crate) fn mark_used(tx: &Transaction<'_>, code: &str, user_id: &str) -> Result<(), DbError> {
    tx.execute(
        "UPDATE invite_codes SET used_by = ?1 WHERE code = ?2",
        params![user_id, code],
    )
    .map_err(map_sqlite)?;
    Ok(())
}

const SELECT: &str =
    "SELECT code, note, created_by, created_at, expires_at, used_by, used_at FROM invite_codes";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<InviteCode> {
    Ok(InviteCode {
        code: r.get(0)?,
        note: r.get(1)?,
        created_by: r.get(2)?,
        created_at: r.get(3)?,
        expires_at: r.get(4)?,
        used_by: r.get(5)?,
        used_at: r.get(6)?,
    })
}

fn get_in(tx: &Transaction<'_>, code: &str) -> Result<Option<InviteCode>, DbError> {
    tx.query_row(&format!("{SELECT} WHERE code = ?1"), params![code], row)
        .optional()
        .map_err(map_sqlite)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_url_safe_and_normalized() {
        let code = new_code().unwrap();
        assert_eq!(code.len(), 16, "{code}");
        assert_eq!(normalize(&format!("  {code} ")), Some(code.clone()));
        assert_ne!(new_code().unwrap(), code);
        assert_eq!(normalize("short"), None);
        assert_eq!(normalize("has space inside"), None);
        assert_eq!(normalize("semi;colon-123456"), None);
        assert_eq!(normalize(&"a".repeat(65)), None);
    }

    fn db() -> (tempfile::NamedTempFile, Db) {
        let file = tempfile::NamedTempFile::new().unwrap();
        let db = Db::open(file.path()).unwrap();
        (file, db)
    }

    #[tokio::test]
    async fn invite_is_used_once_and_respects_expiry() {
        let (_file, db) = db();
        db.writer
            .transaction(|tx| {
                tx.execute(
                    "INSERT INTO users (id, username, salt, role) VALUES ('admin', 'a', 's', 'admin')",
                    [],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();
        let invite = create(&db, "admin", Some(" 友人用 ".to_owned()), Some(7))
            .await
            .unwrap();
        assert_eq!(invite.note.as_deref(), Some("友人用"));
        assert!(invite.expires_at.is_some());
        assert_eq!(list(&db).await.unwrap().len(), 1);

        let code = invite.code.clone();
        let (first, second) = db
            .writer
            .transaction(move |tx| Ok((reserve(tx, &code)?, reserve(tx, &code)?)))
            .await
            .unwrap();
        assert!(first);
        assert!(!second, "2 回目は使えない");

        // 期限切れは使えない。
        let expired = create(&db, "admin", None, None).await.unwrap().code;
        let probe = expired.clone();
        let usable = db
            .writer
            .transaction(move |tx| {
                tx.execute(
                    "UPDATE invite_codes SET expires_at = datetime('now', 'localtime', '-1 days') \
                     WHERE code = ?1",
                    params![probe],
                )
                .map_err(map_sqlite)?;
                reserve(tx, &probe)
            })
            .await
            .unwrap();
        assert!(!usable);

        assert!(delete(&db, &expired).await.unwrap());
        assert!(!delete(&db, &expired).await.unwrap());
    }
}
