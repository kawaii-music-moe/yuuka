//! settings データアクセス（Node `userRepo` の設定系セッターのパリティ）。
//!
//! ユーザー本人の行のみを更新する（`WHERE discord_id = ?`）。bcrypt ハッシュ化は yuuka-auth の
//! [`hash_password`](yuuka_auth::users::hash_password)（cost 12）を再利用し、パスワード変更でも
//! `salt` 列は**絶対に変更しない**（ユーザー鍵導出＝保存済み資格情報の復号に使うため）。

use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, ErrorCode, OptionalExtension};
use yuuka_core::DbError;
use yuuka_db::map_sqlite;
use yuuka_web::Db;

/// ユーザーが Admin ロールか（Node `isAdmin`）。行が無ければ `false`。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn is_admin(db: &Db, discord_id: &str) -> Result<bool, DbError> {
    let id = discord_id.to_owned();
    db.read
        .read(move |conn| {
            let role: Option<String> = conn
                .query_row(
                    "SELECT role FROM users WHERE discord_id = ?1",
                    params![id],
                    |r| r.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite)?;
            Ok(role.as_deref() == Some("admin"))
        })
        .await
}

/// Bot の Discord トークン設定状態（オーナー識別 + suspend + トークン 3 列）。
#[derive(Debug, Clone)]
pub struct BotDiscordRow {
    /// Bot 作成者（オーナー）の discord_id。
    pub user_id: String,
    /// 管理者による停止処分中か。
    pub suspended: bool,
    /// トークン暗号文（未設定は `None`）。
    pub token_encrypted: Option<String>,
    /// トークン IV。
    pub token_iv: Option<String>,
    /// トークン auth tag。
    pub token_tag: Option<String>,
}

impl BotDiscordRow {
    /// 3 列が揃っていればトークン設定済み（Node `hasToken`）。
    #[must_use]
    pub fn has_token(&self) -> bool {
        self.token_encrypted.as_deref().is_some_and(|s| !s.is_empty())
            && self.token_iv.is_some()
            && self.token_tag.is_some()
    }
}

/// Bot 1 件の Discord トークン設定を引く（Node `getBotById` + `getBotDiscordConfig` 相当）。不在は `None`。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_bot_discord(db: &Db, bot_id: &str) -> Result<Option<BotDiscordRow>, DbError> {
    let bot_id = bot_id.to_owned();
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT user_id, suspended, discord_token_encrypted, discord_token_iv, discord_token_tag \
                 FROM bots WHERE id = ?1",
                params![bot_id],
                |r| {
                    Ok(BotDiscordRow {
                        user_id: r.get(0)?,
                        suspended: r.get::<_, i64>(1)? != 0,
                        token_encrypted: r.get(2)?,
                        token_iv: r.get(3)?,
                        token_tag: r.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// Bot の Discord トークン 3 列を更新する（Node `updateBotDiscordToken`・全て NULL でクリア）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn update_bot_discord_token(
    db: &Db,
    bot_id: &str,
    encrypted: Option<String>,
    iv: Option<String>,
    tag: Option<String>,
) -> Result<(), DbError> {
    let bot_id = bot_id.to_owned();
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "UPDATE bots SET discord_token_encrypted = ?1, discord_token_iv = ?2, \
                 discord_token_tag = ?3, updated_at = datetime('now', 'localtime') WHERE id = ?4",
                params![encrypted, iv, tag, bot_id],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// `/api/status` ダッシュボード集計のスナップショット（`(user_id, bot_id)` スコープ・単一読み取り）。
#[derive(Debug, Clone)]
pub struct StatusSnapshot {
    pub tasks: i64,
    pub pending_tasks: i64,
    /// `[low/その他, medium, high]`（応答キー `0/1/2` に対応）。
    pub priorities: [i64; 3],
    pub schedules: i64,
    pub schedule_trend: Vec<i64>,
    pub expenses: i64,
    pub expense_trend: Vec<i64>,
    pub username: Option<String>,
    pub gemini_model: Option<String>,
    pub gemini_key_present: bool,
    pub backup_enabled: bool,
    pub backup_folder_id: Option<String>,
    pub backup_interval_hours: i64,
    pub backup_generations: i64,
    pub backup_last_run_at: Option<String>,
    pub rich_reply_enabled: bool,
    pub remind_default_minutes: i64,
    pub notify_target_type: String,
    pub notify_target_id: Option<String>,
    pub active_persona_id: Option<i64>,
}

/// `/api/status` の集計を単一接続で行う（Node の複数 prepare を 1 read closure に集約）。
///
/// 日付トレンドは SQLite `date('now', ?)`（UTC・Node の `toISOString().slice(0,10)` と一致）で生成する。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
#[allow(clippy::too_many_lines)]
pub async fn status_snapshot(
    db: &Db,
    user_id: &str,
    bot_id: &str,
) -> Result<StatusSnapshot, DbError> {
    let user_id = user_id.to_owned();
    let bot_id = bot_id.to_owned();
    db.read
        .read(move |conn| {
            let scoped = |sql: &str| -> Result<i64, DbError> {
                conn.query_row(sql, params![user_id, bot_id], |r| r.get::<_, i64>(0))
                    .map_err(map_sqlite)
            };
            let tasks = scoped(
                "SELECT COUNT(*) FROM todos WHERE user_id = ?1 AND bot_id = ?2 AND parent_id IS NULL",
            )?;
            let pending_tasks = scoped(
                "SELECT COUNT(*) FROM todos WHERE user_id = ?1 AND bot_id = ?2 AND status = 'open' AND parent_id IS NULL",
            )?;
            let schedules =
                scoped("SELECT COUNT(*) FROM schedules WHERE user_id = ?1 AND bot_id = ?2")?;
            let expenses =
                scoped("SELECT COUNT(*) FROM expenses WHERE user_id = ?1 AND bot_id = ?2")?;

            // 優先度別の未完了 ToDo（high→[2] / medium→[1] / それ以外→[0]）。
            let mut priorities = [0i64; 3];
            {
                let mut stmt = conn
                    .prepare(
                        "SELECT priority, COUNT(*) FROM todos \
                         WHERE user_id = ?1 AND bot_id = ?2 AND status = 'open' AND parent_id IS NULL \
                         GROUP BY priority",
                    )
                    .map_err(map_sqlite)?;
                let rows = stmt
                    .query_map(params![user_id, bot_id], |r| {
                        Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?))
                    })
                    .map_err(map_sqlite)?;
                for row in rows {
                    let (pri, cnt) = row.map_err(map_sqlite)?;
                    match pri.as_deref() {
                        Some("high") => priorities[2] += cnt,
                        Some("medium") => priorities[1] += cnt,
                        _ => priorities[0] += cnt,
                    }
                }
            }

            // スケジュール: 今日〜+4 日のイベント数（UTC 日付）。
            let mut schedule_trend = Vec::with_capacity(5);
            for i in 0..5 {
                let modifier = format!("+{i} day");
                let c: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM schedules \
                         WHERE user_id = ?1 AND bot_id = ?2 AND date(start_at) = date('now', ?3)",
                        params![user_id, bot_id, modifier],
                        |r| r.get(0),
                    )
                    .map_err(map_sqlite)?;
                schedule_trend.push(c);
            }

            // 経費: 4 日前〜今日の支出額（UTC 日付・SUM NULL は 0）。
            let mut expense_trend = Vec::with_capacity(5);
            for i in (0..5).rev() {
                let modifier = format!("-{i} day");
                let total: Option<i64> = conn
                    .query_row(
                        "SELECT SUM(amount) FROM expenses \
                         WHERE user_id = ?1 AND bot_id = ?2 AND type = 'expense' AND date = date('now', ?3)",
                        params![user_id, bot_id, modifier],
                        |r| r.get(0),
                    )
                    .map_err(map_sqlite)?;
                expense_trend.push(total.unwrap_or(0));
            }

            // users 行（存在すれば設定を読む・不在は既定へ）。
            type UserRow = (
                Option<String>,
                Option<String>,
                bool,
                bool,
                Option<String>,
                i64,
                i64,
                Option<String>,
                bool,
                i64,
                String,
                Option<String>,
            );
            let urow: Option<UserRow> = conn
                .query_row(
                    "SELECT username, gemini_model, gemini_api_key_encrypted, backup_enabled, \
                     backup_folder_id, backup_interval_hours, backup_generations, backup_last_run_at, \
                     rich_reply_enabled, remind_default_minutes, notify_target_type, notify_target_id \
                     FROM users WHERE discord_id = ?1",
                    params![user_id],
                    |r| {
                        Ok((
                            r.get::<_, Option<String>>(0)?,
                            r.get::<_, Option<String>>(1)?,
                            r.get::<_, Option<String>>(2)?.is_some(),
                            r.get::<_, i64>(3)? != 0,
                            r.get::<_, Option<String>>(4)?,
                            r.get::<_, i64>(5)?,
                            r.get::<_, i64>(6)?,
                            r.get::<_, Option<String>>(7)?,
                            r.get::<_, i64>(8)? != 0,
                            r.get::<_, i64>(9)?,
                            r.get::<_, String>(10)?,
                            r.get::<_, Option<String>>(11)?,
                        ))
                    },
                )
                .optional()
                .map_err(map_sqlite)?;

            let active_persona_id: Option<i64> = conn
                .query_row(
                    "SELECT persona_id FROM bot_active_personas WHERE user_id = ?1 AND bot_id = ?2",
                    params![user_id, bot_id],
                    |r| r.get::<_, i64>(0),
                )
                .optional()
                .map_err(map_sqlite)?;

            let (
                username,
                gemini_model,
                gemini_key_present,
                backup_enabled,
                backup_folder_id,
                backup_interval_hours,
                backup_generations,
                backup_last_run_at,
                rich_reply_enabled,
                remind_default_minutes,
                notify_target_type,
                notify_target_id,
            ) = urow.unwrap_or((
                None, None, false, false, None, 24, 7, None, true, 10, "dm".to_owned(), None,
            ));

            Ok(StatusSnapshot {
                tasks,
                pending_tasks,
                priorities,
                schedules,
                schedule_trend,
                expenses,
                expense_trend,
                username,
                gemini_model,
                gemini_key_present,
                backup_enabled,
                backup_folder_id,
                backup_interval_hours,
                backup_generations,
                backup_last_run_at,
                rich_reply_enabled,
                remind_default_minutes,
                notify_target_type,
                notify_target_id,
                active_persona_id,
            })
        })
        .await
}

/// `/api/settings/user` の部分更新パッチ（present なフィールドのみ UPDATE する・Node `key in body` 意味論）。
#[derive(Debug, Default)]
pub(crate) struct UserSettingsPatch {
    /// リッチ返信の有効/無効。
    pub rich_reply: Option<bool>,
    /// リマインド既定（分・0 下限は呼び出し側で処理済み）。
    pub remind_minutes: Option<i64>,
    /// 通知先種別（`"dm"`/`"channel"`・呼び出し側で正規化済み）。
    pub notify_type: Option<&'static str>,
    /// 通知先 ID（`Some(None)` は明示 NULL・`Some(Some(v))` は値・`None` は列を触らない）。
    pub notify_id: Option<Option<String>>,
    /// タイムゾーン（非空文字列のみ・呼び出し側で処理済み）。
    pub timezone: Option<String>,
}

/// ユーザー名を更新する（Node `updateUsername`）。変更行があれば `true`。
///
/// username は UNIQUE。衝突時は `Ok(false)`（ハンドラの「既に使われている」400 に写像する。Node は
/// UNIQUE 違反で 500 に落ちるが、ハンドラの false 分岐メッセージの意図に合わせる＝安全側）。
///
/// # Errors
/// UNIQUE 以外の書き込み失敗時 [`DbError`]。
pub async fn update_username(db: &Db, discord_id: &str, username: &str) -> Result<bool, DbError> {
    let discord_id = discord_id.to_owned();
    let username = username.to_owned();
    db.writer
        .transaction(move |tx| {
            match tx.execute(
                "UPDATE users SET username = ?1, updated_at = datetime('now', 'localtime') \
                 WHERE discord_id = ?2",
                params![username, discord_id],
            ) {
                Ok(n) => Ok(n > 0),
                Err(rusqlite::Error::SqliteFailure(e, _))
                    if e.code == ErrorCode::ConstraintViolation =>
                {
                    Ok(false)
                }
                Err(e) => Err(map_sqlite(e)),
            }
        })
        .await
}

/// パスワードを bcrypt で再ハッシュして更新する（Node `updatePassword`）。**`salt` は変更しない**。
/// 全セッション/デスクトップトークンの失効は呼び出し側で行う。
///
/// # Errors
/// bcrypt 失敗・書き込み失敗時 [`DbError`]。
pub async fn update_password(
    db: &Db,
    discord_id: &str,
    new_password: &str,
) -> Result<bool, DbError> {
    // async な bcrypt はクロージャ外で先に済ませる（yuuka-auth の cost 12 実装を再利用）。
    let hash = yuuka_auth::users::hash_password(new_password.to_owned()).await?;
    let discord_id = discord_id.to_owned();
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute(
                    "UPDATE users SET password_hash = ?1, updated_at = datetime('now', 'localtime') \
                     WHERE discord_id = ?2",
                    params![hash, discord_id],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

/// Admin ロールのユーザー数（Node `countAdmins`・唯一の管理者の自己削除ガード用）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn count_admins(db: &Db) -> Result<i64, DbError> {
    db.read
        .read(move |conn| {
            conn.query_row("SELECT COUNT(*) FROM users WHERE role = 'admin'", [], |r| {
                r.get::<_, i64>(0)
            })
            .map_err(map_sqlite)
        })
        .await
}

/// ユーザーを削除する（Node `deleteUser`・関連データは FK ON DELETE CASCADE）。削除できれば `true`。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn delete_user(db: &Db, discord_id: &str) -> Result<bool, DbError> {
    let discord_id = discord_id.to_owned();
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute(
                    "DELETE FROM users WHERE discord_id = ?1",
                    params![discord_id],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

/// `system_default` を除く、指定オーナーの Bot ID 一覧（アカウント削除時の Bot 停止ループ用）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn bot_ids_owned_by(db: &Db, user_id: &str) -> Result<Vec<String>, DbError> {
    let user_id = user_id.to_owned();
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare("SELECT id FROM bots WHERE user_id = ?1 AND id != 'system_default'")
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![user_id], |row| row.get::<_, String>(0))
                .map_err(map_sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(map_sqlite)?);
            }
            Ok(out)
        })
        .await
}

/// 現在の Gemini キー 3 列（暗号文/iv/tag）を返す（keep-current 分岐用・行が無ければ `None`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
#[allow(clippy::type_complexity)]
pub async fn get_gemini_enc(
    db: &Db,
    discord_id: &str,
) -> Result<Option<(Option<String>, Option<String>, Option<String>)>, DbError> {
    let id = discord_id.to_owned();
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT gemini_api_key_encrypted, gemini_api_key_iv, gemini_api_key_tag \
                     FROM users WHERE discord_id = ?1",
                )
                .map_err(map_sqlite)?;
            let mut rows = stmt
                .query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .map_err(map_sqlite)?;
            match rows.next() {
                Some(row) => Ok(Some(row.map_err(map_sqlite)?)),
                None => Ok(None),
            }
        })
        .await
}

/// Gemini 設定（暗号化キー 3 列 + モデル）を更新する（Node `updateUserGeminiSettings`・3 列は nullable）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn set_gemini(
    db: &Db,
    discord_id: &str,
    encrypted: Option<String>,
    iv: Option<String>,
    tag: Option<String>,
    model: &str,
) -> Result<(), DbError> {
    let discord_id = discord_id.to_owned();
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

/// ユーザー設定を部分更新する（Node `updateUserSettings`・present な列のみ SET）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn update_user_settings(
    db: &Db,
    discord_id: &str,
    patch: UserSettingsPatch,
) -> Result<(), DbError> {
    let mut sets: Vec<&str> = Vec::new();
    let mut vals: Vec<Value> = Vec::new();
    if let Some(b) = patch.rich_reply {
        sets.push("rich_reply_enabled = ?");
        vals.push(Value::Integer(i64::from(b)));
    }
    if let Some(m) = patch.remind_minutes {
        sets.push("remind_default_minutes = ?");
        vals.push(Value::Integer(m));
    }
    if let Some(t) = patch.notify_type {
        sets.push("notify_target_type = ?");
        vals.push(Value::Text(t.to_owned()));
    }
    if let Some(id) = patch.notify_id {
        sets.push("notify_target_id = ?");
        vals.push(id.map_or(Value::Null, Value::Text));
    }
    if let Some(tz) = patch.timezone {
        sets.push("timezone = ?");
        vals.push(Value::Text(tz));
    }
    // 更新対象が無ければ no-op（Node は false を返すが、ハンドラは常に 200 を返すため副作用のみ省く）。
    if sets.is_empty() {
        return Ok(());
    }
    let sql = format!(
        "UPDATE users SET {}, updated_at = datetime('now', 'localtime') WHERE discord_id = ?",
        sets.join(", ")
    );
    vals.push(Value::Text(discord_id.to_owned()));
    db.writer
        .transaction(move |tx| {
            tx.execute(&sql, params_from_iter(vals))
                .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// バックアップ設定を更新する（Node `updateUserBackupSettings`）。interval は [1,720] に、generations は
/// [1,∞) に floor + clamp する。`folder`＝`Some` のときのみ `backup_folder_id` 列を更新する。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn update_backup(
    db: &Db,
    discord_id: &str,
    enabled: bool,
    interval_hours: f64,
    generations: f64,
    folder: Option<String>,
) -> Result<(), DbError> {
    // Node: Math.min(720, Math.max(1, Math.floor(intervalHours))) / Math.max(1, Math.floor(generations))。
    let interval = (interval_hours.floor() as i64).clamp(1, 720);
    let gens = (generations.floor() as i64).max(1);

    let mut sets: Vec<&str> = vec![
        "backup_enabled = ?",
        "backup_interval_hours = ?",
        "backup_generations = ?",
    ];
    let mut vals: Vec<Value> = vec![
        Value::Integer(i64::from(enabled)),
        Value::Integer(interval),
        Value::Integer(gens),
    ];
    if let Some(f) = folder {
        sets.push("backup_folder_id = ?");
        vals.push(Value::Text(f));
    }
    let sql = format!(
        "UPDATE users SET {}, updated_at = datetime('now', 'localtime') WHERE discord_id = ?",
        sets.join(", ")
    );
    vals.push(Value::Text(discord_id.to_owned()));
    db.writer
        .transaction(move |tx| {
            tx.execute(&sql, params_from_iter(vals))
                .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use rusqlite::Connection;

    use super::*;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn fresh_db() -> (Db, PathBuf) {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_settings_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        // Rust は不在 DB を作らない（P0-4）。raw 接続で空ファイルを作ってから migrate する。
        drop(Connection::open(&path).expect("create db file"));
        let db = Db::open(&path).expect("open db");
        (db, path)
    }

    fn raw(path: &PathBuf) -> Connection {
        Connection::open(path).expect("raw conn")
    }

    fn seed_user(path: &PathBuf, discord_id: &str, role: &str) {
        raw(path)
            .execute(
                "INSERT INTO users (discord_id, username, password_hash, salt, role) \
                 VALUES (?1, ?1, 'orig-hash', 'deadbeef', ?2)",
                params![discord_id, role],
            )
            .expect("seed user");
    }

    fn col(path: &PathBuf, id: &str, column: &str) -> Option<String> {
        let sql = format!("SELECT {column} FROM users WHERE discord_id = ?1");
        raw(path)
            .query_row(&sql, params![id], |r| r.get::<_, Option<String>>(0))
            .expect("read col")
    }

    fn col_i(path: &PathBuf, id: &str, column: &str) -> Option<i64> {
        let sql = format!("SELECT {column} FROM users WHERE discord_id = ?1");
        raw(path)
            .query_row(&sql, params![id], |r| r.get::<_, Option<i64>>(0))
            .expect("read col_i")
    }

    #[tokio::test]
    async fn username_update_and_unique_conflict() {
        let (db, path) = fresh_db();
        seed_user(&path, "alice", "user");
        seed_user(&path, "bob", "user");
        assert!(update_username(&db, "alice", "alice2").await.unwrap());
        assert_eq!(col(&path, "alice", "username").as_deref(), Some("alice2"));
        // 既存 username への変更は UNIQUE 衝突 → Ok(false)。
        assert!(!update_username(&db, "alice", "bob").await.unwrap());
        // 不在ユーザーは false。
        assert!(!update_username(&db, "ghost", "x").await.unwrap());
    }

    #[tokio::test]
    async fn password_rehash_keeps_salt() {
        let (db, path) = fresh_db();
        seed_user(&path, "alice", "user");
        let salt_before = col(&path, "alice", "salt");
        assert!(update_password(&db, "alice", "NewPassw0rd!").await.unwrap());
        let hash_after = col(&path, "alice", "password_hash").unwrap();
        // bcrypt ハッシュに置き換わる（$2b$ 始まり・元の 'orig-hash' でない）。
        assert!(hash_after.starts_with("$2b$"), "bcrypt でリハッシュ");
        assert_ne!(hash_after, "orig-hash");
        // salt は絶対に変更しない（資格情報復号のため）。
        assert_eq!(col(&path, "alice", "salt"), salt_before);
    }

    #[tokio::test]
    async fn count_admins_and_delete() {
        let (db, path) = fresh_db();
        seed_user(&path, "a", "admin");
        seed_user(&path, "b", "user");
        seed_user(&path, "c", "admin");
        assert_eq!(count_admins(&db).await.unwrap(), 2);
        assert!(delete_user(&db, "b").await.unwrap());
        assert!(!delete_user(&db, "b").await.unwrap());
        assert_eq!(count_admins(&db).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn owned_bots_excludes_system_default() {
        let (db, path) = fresh_db();
        seed_user(&path, "owner", "user");
        raw(&path)
            .execute(
                "INSERT INTO bots (id, user_id, name) VALUES ('system_default','owner','sd'),('b1','owner','b1')",
                [],
            )
            .unwrap();
        assert_eq!(
            bot_ids_owned_by(&db, "owner").await.unwrap(),
            vec!["b1".to_owned()]
        );
    }

    #[tokio::test]
    async fn gemini_set_and_keep_current() {
        let (db, path) = fresh_db();
        seed_user(&path, "u", "user");
        // 新規キー保存。
        set_gemini(
            &db,
            "u",
            Some("enc".to_owned()),
            Some("iv".to_owned()),
            Some("tag".to_owned()),
            "gemini-3.1-flash-lite",
        )
        .await
        .unwrap();
        assert_eq!(
            get_gemini_enc(&db, "u").await.unwrap(),
            Some((
                Some("enc".to_owned()),
                Some("iv".to_owned()),
                Some("tag".to_owned())
            ))
        );
        assert_eq!(
            col(&path, "u", "gemini_model").as_deref(),
            Some("gemini-3.1-flash-lite")
        );
        // keep-current: 既存 3 列を書き戻し、モデルのみ変更。
        let (e, i, t) = get_gemini_enc(&db, "u").await.unwrap().unwrap();
        set_gemini(&db, "u", e, i, t, "gemini-3.1-pro")
            .await
            .unwrap();
        assert_eq!(
            get_gemini_enc(&db, "u").await.unwrap(),
            Some((
                Some("enc".to_owned()),
                Some("iv".to_owned()),
                Some("tag".to_owned())
            ))
        );
        assert_eq!(
            col(&path, "u", "gemini_model").as_deref(),
            Some("gemini-3.1-pro")
        );
    }

    #[tokio::test]
    async fn user_settings_partial_update() {
        let (db, path) = fresh_db();
        seed_user(&path, "u", "user");
        // rich_reply と remind のみ更新（notify/timezone は既定のまま）。
        update_user_settings(
            &db,
            "u",
            UserSettingsPatch {
                rich_reply: Some(false),
                remind_minutes: Some(30),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(col_i(&path, "u", "rich_reply_enabled"), Some(0));
        assert_eq!(col_i(&path, "u", "remind_default_minutes"), Some(30));
        // 既定値のまま（触れていない）。
        assert_eq!(col(&path, "u", "notify_target_type").as_deref(), Some("dm"));

        // notify_id を明示 NULL・notify_type channel・timezone を設定。
        update_user_settings(
            &db,
            "u",
            UserSettingsPatch {
                notify_id: Some(None),
                notify_type: Some("channel"),
                timezone: Some("UTC".to_owned()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            col(&path, "u", "notify_target_type").as_deref(),
            Some("channel")
        );
        assert_eq!(col(&path, "u", "notify_target_id"), None);
        assert_eq!(col(&path, "u", "timezone").as_deref(), Some("UTC"));

        // 空パッチは no-op（エラーにならない）。
        update_user_settings(&db, "u", UserSettingsPatch::default())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn backup_clamps_and_optional_folder() {
        let (db, path) = fresh_db();
        seed_user(&path, "u", "user");
        // interval 1000 → 720 に、generations 0.5 → floor 0 → max(1)=1 に。folder は未指定で不変。
        update_backup(&db, "u", true, 1000.0, 0.5, None)
            .await
            .unwrap();
        assert_eq!(col_i(&path, "u", "backup_enabled"), Some(1));
        assert_eq!(col_i(&path, "u", "backup_interval_hours"), Some(720));
        assert_eq!(col_i(&path, "u", "backup_generations"), Some(1));
        assert_eq!(col(&path, "u", "backup_folder_id"), None);

        // interval 0.5 → floor 0 → max(1)=1、folder 指定で列更新。
        update_backup(&db, "u", false, 0.5, 3.0, Some("FOLDER123".to_owned()))
            .await
            .unwrap();
        assert_eq!(col_i(&path, "u", "backup_enabled"), Some(0));
        assert_eq!(col_i(&path, "u", "backup_interval_hours"), Some(1));
        assert_eq!(col_i(&path, "u", "backup_generations"), Some(3));
        assert_eq!(
            col(&path, "u", "backup_folder_id").as_deref(),
            Some("FOLDER123")
        );
    }
}
