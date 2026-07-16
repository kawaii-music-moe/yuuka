//! Google 連携データアクセス（Node `googleAccountRepo` / `userRepo` の Google 系パリティ）。
//!
//! `user_google_accounts`（owner 単位の複数アカウント）と `bot_google_account`（Bot ごとの使用
//! アカウント）を扱う。リフレッシュトークンは呼び出し側（ルート）が [`SystemCrypto`] で暗号化した
//! 3 列（暗号文/iv/tag）を渡す（Node は repo 内で暗号化するが、本移植は crypto をルート層へ集約する）。
//!
//! [`SystemCrypto`]: yuuka_crypto::SystemCrypto

use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use yuuka_core::DbError;
use yuuka_db::map_sqlite;
use yuuka_web::Db;

/// UI に返す安全なアカウントビュー（トークン列を除く・Node `UserGoogleAccountSafe`）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GoogleAccountSafe {
    pub id: i64,
    pub email: Option<String>,
    pub calendar_id: Option<String>,
    pub calendars: Vec<String>,
    pub is_primary: bool,
}

/// primary アカウントの最小ビュー（`/api/status` の連携判定 + カレンダー ID 表示用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrimaryAccount {
    pub id: i64,
    pub calendar_id: Option<String>,
}

/// アカウント所有者の識別（越権チェック用の最小ビュー）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountOwner {
    pub id: i64,
    pub user_id: String,
    pub email: Option<String>,
    pub calendar_id: Option<String>,
    pub is_primary: bool,
}

/// Bot が使う Google アカウントの設定モード（Node `getBotGoogleMode`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BotGoogleMode {
    /// 未設定（発話者の primary へフォールバック）。
    Primary,
    /// 連携なし（明示的に無効）。
    None,
    /// 特定アカウント ID を使用。
    Account(i64),
}

impl BotGoogleMode {
    /// Node の `"primary" | "none" | number` と一致する JSON 値へ写像する。
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Primary => serde_json::Value::String("primary".to_owned()),
            Self::None => serde_json::Value::String("none".to_owned()),
            Self::Account(id) => serde_json::Value::Number((*id).into()),
        }
    }
}

fn parse_calendars(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

/// owner の primary アカウントを引く（Node `getPrimaryGoogleAccount`・`is_primary DESC, id ASC` 先頭）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_primary_account(
    db: &Db,
    user_id: &str,
) -> Result<Option<PrimaryAccount>, DbError> {
    let user_id = user_id.to_owned();
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT id, calendar_id FROM user_google_accounts WHERE user_id = ?1 \
                 ORDER BY is_primary DESC, id ASC LIMIT 1",
                params![user_id],
                |r| {
                    Ok(PrimaryAccount {
                        id: r.get(0)?,
                        calendar_id: r.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// owner の primary アカウントの暗号化リフレッシュトークン 3 列を引く（`GoogleHttpClient` の
/// キャッシュ済み一覧解決用）。`(account_id, encrypted, iv, tag)` を返す。Node の
/// `getPrimaryGoogleAccount` + repo 内復号に相当（本移植は復号をクライアント層へ集約する）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_primary_account_tokens(
    db: &Db,
    user_id: &str,
) -> Result<Option<(i64, String, String, String)>, DbError> {
    let user_id = user_id.to_owned();
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT id, refresh_token_encrypted, refresh_token_iv, refresh_token_tag \
                 FROM user_google_accounts WHERE user_id = ?1 \
                 ORDER BY is_primary DESC, id ASC LIMIT 1",
                params![user_id],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// 特定アカウントの暗号化リフレッシュトークン 3 列 + 所有者を引く（`GoogleHttpClient` の
/// アカウント別一覧解決用）。`(user_id, encrypted, iv, tag)` を返す。越権チェックのため owner を含む。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_account_tokens(
    db: &Db,
    account_id: i64,
) -> Result<Option<(String, String, String, String)>, DbError> {
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT user_id, refresh_token_encrypted, refresh_token_iv, refresh_token_tag \
                 FROM user_google_accounts WHERE id = ?1",
                params![account_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// owner の連携アカウント数（`/api/status` の `googleAccountCount`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn count_accounts(db: &Db, user_id: &str) -> Result<i64, DbError> {
    let user_id = user_id.to_owned();
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM user_google_accounts WHERE user_id = ?1",
                params![user_id],
                |r| r.get::<_, i64>(0),
            )
            .map_err(map_sqlite)
        })
        .await
}

/// owner の連携アカウント一覧（UI 用・トークン無し・Node `listGoogleAccountsSafe`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_accounts_safe(db: &Db, user_id: &str) -> Result<Vec<GoogleAccountSafe>, DbError> {
    let user_id = user_id.to_owned();
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, email, calendar_id, calendars, is_primary \
                     FROM user_google_accounts WHERE user_id = ?1 ORDER BY is_primary DESC, id ASC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![user_id], |r| {
                    let calendars: String = r.get(3)?;
                    Ok(GoogleAccountSafe {
                        id: r.get(0)?,
                        email: r.get(1)?,
                        calendar_id: r.get(2)?,
                        calendars: parse_calendars(&calendars),
                        is_primary: r.get::<_, i64>(4)? == 1,
                    })
                })
                .map_err(map_sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(map_sqlite)?);
            }
            Ok(out)
        })
        .await
}

/// アカウントを 1 件引く（所有権チェック用・Node `getGoogleAccountById`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_account(db: &Db, account_id: i64) -> Result<Option<AccountOwner>, DbError> {
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT id, user_id, email, calendar_id, is_primary \
                 FROM user_google_accounts WHERE id = ?1",
                params![account_id],
                |r| {
                    Ok(AccountOwner {
                        id: r.get(0)?,
                        user_id: r.get(1)?,
                        email: r.get(2)?,
                        calendar_id: r.get(3)?,
                        is_primary: r.get::<_, i64>(4)? == 1,
                    })
                },
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// Google アカウントを追加/更新する（Node `addGoogleAccount`）。同一 `(user_id, email)` は更新（トークン
/// 差し替え + `calendar_id` は `COALESCE`）、無ければ新規。owner の最初のアカウントは自動 primary。返り値は
/// アカウント ID。**リフレッシュトークンは暗号化済み 3 列を渡す**（呼び出し側で `SystemCrypto` 暗号化）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn add_or_update_account(
    db: &Db,
    user_id: &str,
    email: Option<String>,
    refresh_enc: String,
    refresh_iv: String,
    refresh_tag: String,
    calendar_id: Option<String>,
) -> Result<i64, DbError> {
    let user_id = user_id.to_owned();
    db.writer
        .transaction(move |tx| {
            let has_any: bool = tx
                .query_row(
                    "SELECT 1 FROM user_google_accounts WHERE user_id = ?1 LIMIT 1",
                    params![user_id],
                    |_| Ok(()),
                )
                .optional()
                .map_err(map_sqlite)?
                .is_some();
            let is_primary = i64::from(!has_any);

            // email が非 NULL のときのみ既存行を探す（Node: email NULL は常に新規）。
            let existing_id: Option<i64> = match &email {
                Some(e) => tx
                    .query_row(
                        "SELECT id FROM user_google_accounts WHERE user_id = ?1 AND email = ?2",
                        params![user_id, e],
                        |r| r.get::<_, i64>(0),
                    )
                    .optional()
                    .map_err(map_sqlite)?,
                None => None,
            };

            if let Some(id) = existing_id {
                tx.execute(
                    "UPDATE user_google_accounts \
                     SET refresh_token_encrypted = ?1, refresh_token_iv = ?2, refresh_token_tag = ?3, \
                         calendar_id = COALESCE(?4, calendar_id), updated_at = datetime('now','localtime') \
                     WHERE id = ?5",
                    params![refresh_enc, refresh_iv, refresh_tag, calendar_id, id],
                )
                .map_err(map_sqlite)?;
                return Ok(id);
            }

            tx.execute(
                "INSERT INTO user_google_accounts \
                 (user_id, email, refresh_token_encrypted, refresh_token_iv, refresh_token_tag, calendar_id, is_primary) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![user_id, email, refresh_enc, refresh_iv, refresh_tag, calendar_id, is_primary],
            )
            .map_err(map_sqlite)?;
            Ok(tx.last_insert_rowid())
        })
        .await
}

/// primary を付け替える（Node `setPrimaryGoogleAccount`・owner 本人のアカウントのみ・他を解除）。
/// 対象が本人所有でなければ `Ok(false)`。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn set_primary(db: &Db, user_id: &str, account_id: i64) -> Result<bool, DbError> {
    let user_id = user_id.to_owned();
    db.writer
        .transaction(move |tx| {
            let owner: Option<String> = tx
                .query_row(
                    "SELECT user_id FROM user_google_accounts WHERE id = ?1",
                    params![account_id],
                    |r| r.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite)?;
            if owner.as_deref() != Some(user_id.as_str()) {
                return Ok(false);
            }
            tx.execute(
                "UPDATE user_google_accounts SET is_primary = 0 WHERE user_id = ?1",
                params![user_id],
            )
            .map_err(map_sqlite)?;
            tx.execute(
                "UPDATE user_google_accounts SET is_primary = 1, updated_at = datetime('now','localtime') WHERE id = ?1",
                params![account_id],
            )
            .map_err(map_sqlite)?;
            Ok(true)
        })
        .await
}

/// アカウント単位の同期対象カレンダーを更新する（Node `updateGoogleCalendars`）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn update_account_calendars(
    db: &Db,
    account_id: i64,
    calendars: &[String],
) -> Result<(), DbError> {
    let json = serde_json::to_string(calendars).unwrap_or_else(|_| "[]".to_owned());
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "UPDATE user_google_accounts SET calendars = ?1, updated_at = datetime('now','localtime') WHERE id = ?2",
                params![json, account_id],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// ユーザー個人（`users.google_calendars`）の同期対象カレンダーを更新する（Node
/// `updateUserGoogleSettings({calendars})`・`/api/settings/calendars` 用）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn set_user_calendars(
    db: &Db,
    user_id: &str,
    calendars: &[String],
) -> Result<(), DbError> {
    let json = serde_json::to_string(calendars).unwrap_or_else(|_| "[]".to_owned());
    let user_id = user_id.to_owned();
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "UPDATE users SET google_calendars = ?1, updated_at = datetime('now','localtime') WHERE discord_id = ?2",
                params![json, user_id],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// アカウントを削除する（Node `deleteGoogleAccount`・owner 本人のみ）。削除対象が primary だった場合は
/// 残りの最古（id 昇順）を primary へ昇格する。本人所有でなければ `Ok(false)`。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn delete_account(db: &Db, user_id: &str, account_id: i64) -> Result<bool, DbError> {
    let user_id = user_id.to_owned();
    db.writer
        .transaction(move |tx| {
            let acct: Option<(String, i64)> = tx
                .query_row(
                    "SELECT user_id, is_primary FROM user_google_accounts WHERE id = ?1",
                    params![account_id],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
                )
                .optional()
                .map_err(map_sqlite)?;
            let Some((owner, is_primary)) = acct else {
                return Ok(false);
            };
            if owner != user_id {
                return Ok(false);
            }
            tx.execute(
                "DELETE FROM user_google_accounts WHERE id = ?1",
                params![account_id],
            )
            .map_err(map_sqlite)?;
            if is_primary == 1 {
                let next: Option<i64> = tx
                    .query_row(
                        "SELECT id FROM user_google_accounts WHERE user_id = ?1 ORDER BY id ASC LIMIT 1",
                        params![user_id],
                        |r| r.get::<_, i64>(0),
                    )
                    .optional()
                    .map_err(map_sqlite)?;
                if let Some(next_id) = next {
                    tx.execute(
                        "UPDATE user_google_accounts SET is_primary = 1 WHERE id = ?1",
                        params![next_id],
                    )
                    .map_err(map_sqlite)?;
                }
            }
            Ok(true)
        })
        .await
}

/// Bot の Google 使用モードを引く（Node `getBotGoogleMode`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_bot_google_mode(db: &Db, bot_id: &str) -> Result<BotGoogleMode, DbError> {
    let bot_id = bot_id.to_owned();
    db.read
        .read(move |conn| {
            let row: Option<Option<i64>> = conn
                .query_row(
                    "SELECT google_account_id FROM bot_google_account WHERE bot_id = ?1",
                    params![bot_id],
                    |r| r.get::<_, Option<i64>>(0),
                )
                .optional()
                .map_err(map_sqlite)?;
            Ok(match row {
                None => BotGoogleMode::Primary,
                Some(None) => BotGoogleMode::None,
                Some(Some(id)) => BotGoogleMode::Account(id),
            })
        })
        .await
}

/// Bot の使用アカウントを設定する（Node `setBotGoogleAccount`・`None` は「連携なし」で upsert）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn set_bot_google_account(
    db: &Db,
    bot_id: &str,
    account_id: Option<i64>,
) -> Result<(), DbError> {
    let bot_id = bot_id.to_owned();
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT INTO bot_google_account (bot_id, google_account_id) VALUES (?1, ?2) \
                 ON CONFLICT(bot_id) DO UPDATE SET google_account_id = excluded.google_account_id, \
                 created_at = datetime('now','localtime')",
                params![bot_id, account_id],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// Bot の使用アカウント割当を解除する（Node `clearBotGoogleAccount`・行削除で primary フォールバックへ）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn clear_bot_google_account(db: &Db, bot_id: &str) -> Result<(), DbError> {
    let bot_id = bot_id.to_owned();
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "DELETE FROM bot_google_account WHERE bot_id = ?1",
                params![bot_id],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}
