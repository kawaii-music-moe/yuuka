//! `YUUKA_ENCRYPTION_SECRET` ローテーション（Node `rotateSecretKey` パリティ・§6.2.1）。
//!
//! `YUUKA_ENCRYPTION_SECRET_NEW` が設定された起動時に、全暗号化列を **旧鍵で復号 → 新鍵で再暗号化**
//! する。マイグレーション直後・サービス起動前に単一スレッドで 1 回だけ回す前提（Node と同じ）。
//! DB は生 `rusqlite::Connection` を 1 トランザクションで更新する（writer を立てる前の起動フェーズ）。

use std::collections::HashMap;

use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension};

use crate::{decrypt_with_key, derive_system_key, derive_user_key, encrypt_with_key, CryptoError};

/// 1 つの暗号化カラム群の仕様（Node `EncryptedColumnSpec`）。
#[derive(Debug, Clone, Copy)]
pub struct EncryptedColumnSpec {
    /// テーブル名。
    pub table: &'static str,
    /// 行を一意に特定する列（`WHERE` に使う）。
    pub key_columns: &'static [&'static str],
    /// `[暗号文, IV, authTag]` の列名トリプレット。
    pub columns: [&'static str; 3],
    /// ユーザー鍵で暗号化されている場合、salt 解決のための `user_id` 列名。
    pub user_scoped_by: Option<&'static str>,
}

/// システム内の全暗号化カラムのレジストリ（Node `ENCRYPTED_COLUMNS` と同一・ローテーション対象）。
pub const ENCRYPTED_COLUMNS: &[EncryptedColumnSpec] = &[
    EncryptedColumnSpec {
        table: "users",
        key_columns: &["discord_id"],
        columns: [
            "gemini_api_key_encrypted",
            "gemini_api_key_iv",
            "gemini_api_key_tag",
        ],
        user_scoped_by: None,
    },
    EncryptedColumnSpec {
        table: "users",
        key_columns: &["discord_id"],
        columns: [
            "google_refresh_token_encrypted",
            "google_refresh_token_iv",
            "google_refresh_token_tag",
        ],
        user_scoped_by: None,
    },
    EncryptedColumnSpec {
        table: "bots",
        key_columns: &["id"],
        columns: [
            "discord_token_encrypted",
            "discord_token_iv",
            "discord_token_tag",
        ],
        user_scoped_by: None,
    },
    EncryptedColumnSpec {
        table: "webhook_endpoints",
        key_columns: &["id"],
        columns: ["secret_encrypted", "secret_iv", "secret_tag"],
        user_scoped_by: None,
    },
    EncryptedColumnSpec {
        table: "mcp_servers",
        key_columns: &["id"],
        columns: [
            "auth_credential_encrypted",
            "auth_credential_iv",
            "auth_credential_tag",
        ],
        user_scoped_by: None,
    },
    EncryptedColumnSpec {
        table: "credentials",
        key_columns: &["user_id", "service_name"],
        columns: ["encrypted_password", "iv", "auth_tag"],
        user_scoped_by: Some("user_id"),
    },
];

/// 全暗号化エントリを旧鍵で復号→新鍵で再暗号化する。再暗号化した件数を返す。
///
/// `old_secret == new_secret` のときは何もせず `Ok(0)`（Node のスキップ挙動）。
/// 1 件でも失敗すればトランザクションごとロールバックし [`CryptoError`] を返す（部分適用を残さない）。
///
/// # Errors
/// DB 操作失敗・復号失敗・鍵導出失敗で [`CryptoError`]。
pub fn rotate_secret_key(
    conn: &mut Connection,
    old_secret: &str,
    new_secret: &str,
) -> Result<usize, CryptoError> {
    if old_secret == new_secret {
        tracing::warn!(
            "YUUKA_ENCRYPTION_SECRET_NEW が現行鍵と同一のためローテーションをスキップします"
        );
        return Ok(0);
    }
    tracing::info!("YUUKA_ENCRYPTION_SECRET ローテーションを開始します");

    let old_system_key = derive_system_key(old_secret)?;
    let new_system_key = derive_system_key(new_secret)?;

    let tx = conn.transaction()?;

    // ユーザーソルト一覧（discord_id → salt）。ユーザー鍵スコープの再暗号化に使う。
    let mut user_salts: HashMap<String, String> = HashMap::new();
    if table_exists(&tx, "users")? {
        let mut stmt = tx.prepare("SELECT discord_id, salt FROM users")?;
        let mapped = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })?;
        for row in mapped {
            let (id, salt) = row?;
            if let Some(salt) = salt {
                user_salts.insert(id, salt);
            }
        }
    }

    let mut rotated = 0usize;
    for spec in ENCRYPTED_COLUMNS {
        if !table_exists(&tx, spec.table)? {
            continue; // テーブル未作成（初回起動）はスキップ。
        }
        let [enc_col, iv_col, tag_col] = spec.columns;

        // SELECT 対象列（key_columns + enc/iv/tag + user_scoped_by・重複排除）。
        let mut cols: Vec<&str> = spec.key_columns.to_vec();
        for c in [enc_col, iv_col, tag_col] {
            if !cols.contains(&c) {
                cols.push(c);
            }
        }
        if let Some(u) = spec.user_scoped_by {
            if !cols.contains(&u) {
                cols.push(u);
            }
        }
        let position = |name: &str| cols.iter().position(|c| *c == name);

        let select_sql = format!(
            "SELECT {} FROM {} WHERE {} IS NOT NULL AND {} != ''",
            cols.join(", "),
            spec.table,
            enc_col,
            enc_col
        );

        // 行を所有 Vec<Value> に確定してから UPDATE する（SELECT の借用を解放するため）。
        let col_count = cols.len();
        let mut stmt = tx.prepare(&select_sql)?;
        let raw_rows: Vec<Vec<Value>> = stmt
            .query_map([], |r| {
                let mut vals = Vec::with_capacity(col_count);
                for i in 0..col_count {
                    vals.push(r.get::<usize, Value>(i)?);
                }
                Ok(vals)
            })?
            .collect::<Result<_, _>>()?;
        drop(stmt);

        let where_sql = spec
            .key_columns
            .iter()
            .map(|c| format!("{c} = ?"))
            .collect::<Vec<_>>()
            .join(" AND ");
        let update_sql = format!(
            "UPDATE {} SET {} = ?, {} = ?, {} = ? WHERE {}",
            spec.table, enc_col, iv_col, tag_col, where_sql
        );

        for row in &raw_rows {
            let enc = value_str(row.get(position(enc_col).unwrap_or(usize::MAX)));
            let iv = value_str(row.get(position(iv_col).unwrap_or(usize::MAX)));
            let tag = value_str(row.get(position(tag_col).unwrap_or(usize::MAX)));
            let (Some(enc), Some(iv), Some(tag)) = (enc, iv, tag) else {
                continue; // NULL/非文字列は WHERE で除外済みだが防御的に。
            };

            let new_enc = if let Some(u) = spec.user_scoped_by {
                let uid = value_str(row.get(position(u).unwrap_or(usize::MAX)));
                let Some(uid) = uid else { continue };
                let Some(salt) = user_salts.get(&uid) else {
                    continue; // ソルト不明のユーザーはスキップ（Node と同一）。
                };
                let old_key = derive_user_key(old_secret, salt)?;
                let new_key = derive_user_key(new_secret, salt)?;
                let plaintext = decrypt_with_key(&old_key, &enc, &iv, &tag)?;
                encrypt_with_key(&new_key, &plaintext)?
            } else {
                let plaintext = decrypt_with_key(&old_system_key, &enc, &iv, &tag)?;
                encrypt_with_key(&new_system_key, &plaintext)?
            };

            // バインド: [新 enc, 新 iv, 新 tag, key_columns...]。
            let mut binds: Vec<Value> = vec![
                Value::Text(new_enc.encrypted),
                Value::Text(new_enc.iv),
                Value::Text(new_enc.auth_tag),
            ];
            for kc in spec.key_columns {
                let v = row
                    .get(position(kc).unwrap_or(usize::MAX))
                    .cloned()
                    .unwrap_or(Value::Null);
                binds.push(v);
            }
            tx.execute(&update_sql, rusqlite::params_from_iter(binds))?;
            rotated += 1;
        }
    }

    tx.commit()?;
    tracing::info!(rotated, "YUUKA_ENCRYPTION_SECRET ローテーション完了");
    Ok(rotated)
}

/// テーブルが存在するか（`sqlite_master` 照会）。
fn table_exists(conn: &Connection, name: &str) -> Result<bool, CryptoError> {
    let found = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name = ?1",
            [name],
            |_| Ok(()),
        )
        .optional()?;
    Ok(found.is_some())
}

/// `Option<&Value>` を TEXT として取り出す（非 TEXT / None は `None`）。
fn value_str(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::Text(s)) => Some(s.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{encrypt_with_key, generate_user_salt, SystemCrypto};
    use secrecy::SecretString;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE users (discord_id TEXT PRIMARY KEY, salt TEXT,
                gemini_api_key_encrypted TEXT, gemini_api_key_iv TEXT, gemini_api_key_tag TEXT,
                google_refresh_token_encrypted TEXT, google_refresh_token_iv TEXT, google_refresh_token_tag TEXT);
             CREATE TABLE credentials (user_id TEXT, service_name TEXT,
                encrypted_password TEXT, iv TEXT, auth_tag TEXT,
                PRIMARY KEY(user_id, service_name));",
        )
        .unwrap();
        conn
    }

    #[test]
    fn rotates_system_and_user_scoped_columns() {
        let old = "old-secret";
        let new = "new-secret";
        let salt = generate_user_salt().unwrap();

        // 旧鍵で 2 種のデータを暗号化して投入。
        let old_sys = derive_system_key(old).unwrap();
        let gem = encrypt_with_key(&old_sys, "gemini-KEY").unwrap();
        let old_user = derive_user_key(old, &salt).unwrap();
        let pw = encrypt_with_key(&old_user, "s3cret-pw").unwrap();

        let mut conn = setup_db();
        conn.execute(
            "INSERT INTO users (discord_id, salt, gemini_api_key_encrypted, gemini_api_key_iv, gemini_api_key_tag)
             VALUES ('u1', ?1, ?2, ?3, ?4)",
            rusqlite::params![salt, gem.encrypted, gem.iv, gem.auth_tag],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO credentials (user_id, service_name, encrypted_password, iv, auth_tag)
             VALUES ('u1', 'github', ?1, ?2, ?3)",
            rusqlite::params![pw.encrypted, pw.iv, pw.auth_tag],
        )
        .unwrap();

        // ローテーション実行（users×2 spec のうち gemini のみ在・google は NULL でスキップ、credentials×1）。
        let n = rotate_secret_key(&mut conn, old, new).unwrap();
        assert_eq!(n, 2, "gemini(system) + credentials(user) の 2 件");

        // 旧鍵ではもう復号できない。
        let (e, i, t): (String, String, String) = conn
            .query_row(
                "SELECT gemini_api_key_encrypted, gemini_api_key_iv, gemini_api_key_tag FROM users WHERE discord_id='u1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert!(
            decrypt_with_key(&old_sys, &e, &i, &t).is_err(),
            "旧システム鍵では復号不可"
        );

        // 新鍵で構築した SystemCrypto で正しく復号できる（システム鍵）。
        let new_crypto = SystemCrypto::new(SecretString::from(new)).unwrap();
        assert_eq!(new_crypto.decrypt_text(&e, &i, &t).unwrap(), "gemini-KEY");

        // credentials（ユーザー鍵）も新鍵で復号できる。
        let (ce, ci, ct): (String, String, String) = conn
            .query_row(
                "SELECT encrypted_password, iv, auth_tag FROM credentials WHERE user_id='u1' AND service_name='github'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            new_crypto.decrypt_for_user(&salt, &ce, &ci, &ct).unwrap(),
            "s3cret-pw"
        );
    }

    #[test]
    fn same_secret_is_noop() {
        let mut conn = setup_db();
        assert_eq!(rotate_secret_key(&mut conn, "same", "same").unwrap(), 0);
    }

    #[test]
    fn missing_tables_are_skipped() {
        // credentials/users しか無い DB でも bots/webhook/mcp の欠落で落ちない。
        let mut conn = setup_db();
        assert_eq!(rotate_secret_key(&mut conn, "a", "b").unwrap(), 0);
    }
}
