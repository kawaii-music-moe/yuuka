//! `users` テーブルの薄いアクセサ（PWA 設定画面用・issue #33/#39）。
//!
//! `yuuka-settings`（`/api/settings/gemini` 等）とは別の最小限のクエリを直接持つ: PWA 設定
//! GET は暗号化キーの有無に関わらず `gemini_model` を返す必要があり（`yuuka_orchestrator::user::
//! user_gemini` は暗号 3 列が揃っていないと `None` を返すため流用できない）、書き込みも
//! `gemini_model` 列のみを触ればよく（PWA はモデル名しか変更しない・APIキー列は不変）、
//! `yuuka-settings::repo`（private module）の `get_gemini_enc`/`set_gemini` を経由する理由がない。

use rusqlite::{params, OptionalExtension};
use yuuka_core::DbError;
use yuuka_db::map_sqlite;
use yuuka_web::Db;

/// PWA 設定画面に必要な `users` の部分ビュー。
#[derive(Debug, Clone)]
pub struct UserAgentSettings {
    /// 未設定・空文字は [`yuuka_gemini::DEFAULT_MODEL`] に呼び出し側で倒す。
    pub model: Option<String>,
    /// `user_google_accounts` に当該利用者の行があるか（issue #272）。
    pub google_connected: bool,
    /// 連携先の表示名（primary アカウントの `email`）。列が NULL の行では `None`。
    pub google_account: Option<String>,
}

/// `(model, google連携有無, 連携先 email)` を返す（行が無ければ `None`＝未登録ユーザー）。
///
/// Google 連携の有無は `user_google_accounts` に当該 `user_id`（= `users.discord_id`）の行が
/// あるかで判定する（issue #272）。旧列 `users.google_refresh_token_encrypted` は現在の OAuth
/// フロー（`yuuka-settings::google` → `yuuka_google::repo::add_or_update_account`）が一切書かず、
/// 鍵ローテーションと salvage だけが触るため、連携済みでも常に未接続と表示されていた。
/// 表示名は primary アカウント（`is_primary DESC, id ASC` 先頭・`yuuka_google::repo` と同順）の
/// `email`。内部 `calendar_id` は返さない（issue #47）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_agent_settings(
    db: &Db,
    discord_id: &str,
) -> Result<Option<UserAgentSettings>, DbError> {
    let id = discord_id.to_owned();
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT u.gemini_model, g.id, g.email \
                 FROM users u \
                 LEFT JOIN user_google_accounts g ON g.id = ( \
                     SELECT id FROM user_google_accounts \
                     WHERE user_id = u.discord_id \
                     ORDER BY is_primary DESC, id ASC LIMIT 1 \
                 ) \
                 WHERE u.discord_id = ?1",
                params![id],
                |r| {
                    let model: Option<String> = r.get(0)?;
                    let account_id: Option<i64> = r.get(1)?;
                    let email: Option<String> = r.get(2)?;
                    Ok(UserAgentSettings {
                        model,
                        google_connected: account_id.is_some(),
                        google_account: email,
                    })
                },
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// `gemini_model` 列のみを更新する（issue #39: 呼び出し側で [`yuuka_gemini::ALLOWED_MODELS`] に
/// 対して事前検証済みの値を渡すこと）。
///
/// `users` 行が存在せず何も更新されなかった場合は `false` を返す（呼び出し側は保存されていない値を
/// 成功として報告しないこと）。行はここでは作らない: `users` は認証（`yuuka-auth`）が
/// `salt`・sk-account ID 付きで作る唯一の正で、PWA 設定の書き込みが仮の認証情報で行を作ると
/// 未登録ユーザーを生み得るため。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn set_gemini_model(db: &Db, discord_id: &str, model: &str) -> Result<bool, DbError> {
    let (id, model) = (discord_id.to_owned(), model.to_owned());
    db.writer
        .transaction(move |tx| {
            let updated = tx
                .execute(
                    "UPDATE users SET gemini_model = ?1, updated_at = datetime('now', 'localtime') \
                     WHERE discord_id = ?2",
                    params![model, id],
                )
                .map_err(map_sqlite)?;
            Ok(updated > 0)
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    async fn open_db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("t.db");
        drop(rusqlite::Connection::open(&path).expect("create empty"));
        let db = Db::open(Path::new(&path)).expect("open db");
        (db, dir)
    }

    async fn seed_user(db: &Db, id: &str, model: Option<&str>) {
        let (id, model) = (id.to_owned(), model.map(str::to_owned));
        db.writer
            .transaction(move |tx| {
                tx.execute(
                    "INSERT INTO users (discord_id, username, salt, gemini_model) \
                     VALUES (?1, ?1, 'x', ?2)",
                    params![id, model],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
            .expect("seed user");
    }

    /// 現在の OAuth フローと同じく `user_google_accounts` 側に連携行を作る（issue #272）。
    async fn seed_google_account(db: &Db, id: &str, email: Option<&str>, is_primary: bool) {
        let (id, email) = (id.to_owned(), email.map(str::to_owned));
        db.writer
            .transaction(move |tx| {
                tx.execute(
                    "INSERT INTO user_google_accounts \
                       (user_id, email, refresh_token_encrypted, refresh_token_iv, \
                        refresh_token_tag, is_primary) \
                     VALUES (?1, ?2, 'enc', 'iv', 'tag', ?3)",
                    params![id, email, i64::from(is_primary)],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
            .expect("seed google account");
    }

    /// 旧列 `users.google_refresh_token_encrypted` だけを埋める（鍵ローテーション・salvage 相当）。
    async fn seed_legacy_google_column(db: &Db, id: &str) {
        let id = id.to_owned();
        db.writer
            .transaction(move |tx| {
                tx.execute(
                    "UPDATE users SET google_refresh_token_encrypted = 'enc' \
                     WHERE discord_id = ?1",
                    params![id],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
            .expect("seed legacy column");
    }

    #[tokio::test]
    async fn unregistered_user_yields_none() {
        let (db, _dir) = open_db().await;
        assert!(get_agent_settings(&db, "ghost").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn reads_model_and_google_connected_flag() {
        let (db, _dir) = open_db().await;
        seed_user(&db, "u1", Some("gemini-2.5-pro")).await;
        seed_google_account(&db, "u1", Some("u1@example.com"), true).await;
        seed_user(&db, "u2", None).await;

        let s1 = get_agent_settings(&db, "u1").await.unwrap().unwrap();
        assert_eq!(s1.model.as_deref(), Some("gemini-2.5-pro"));
        assert!(s1.google_connected);
        assert_eq!(s1.google_account.as_deref(), Some("u1@example.com"));

        let s2 = get_agent_settings(&db, "u2").await.unwrap().unwrap();
        assert_eq!(s2.model, None);
        assert!(!s2.google_connected);
        assert_eq!(s2.google_account, None);
    }

    #[tokio::test]
    async fn google_connected_follows_accounts_table_not_legacy_column() {
        let (db, _dir) = open_db().await;
        // 旧列だけがある利用者は連携済みとみなさない（現在の OAuth フローは旧列を書かない）。
        seed_user(&db, "legacy", None).await;
        seed_legacy_google_column(&db, "legacy").await;
        let legacy = get_agent_settings(&db, "legacy").await.unwrap().unwrap();
        assert!(!legacy.google_connected, "旧列は連携の判定材料にしない");

        // 旧列が空でも `user_google_accounts` に行があれば連携済み（issue #272 の症状）。
        seed_user(&db, "oauth", None).await;
        seed_google_account(&db, "oauth", Some("oauth@example.com"), true).await;
        let oauth = get_agent_settings(&db, "oauth").await.unwrap().unwrap();
        assert!(oauth.google_connected);
        assert_eq!(oauth.google_account.as_deref(), Some("oauth@example.com"));
    }

    #[tokio::test]
    async fn google_account_is_none_when_email_is_null() {
        let (db, _dir) = open_db().await;
        seed_user(&db, "u1", None).await;
        seed_google_account(&db, "u1", None, true).await;

        let s = get_agent_settings(&db, "u1").await.unwrap().unwrap();
        assert!(s.google_connected, "email が NULL でも連携済み");
        assert_eq!(s.google_account, None);
    }

    #[tokio::test]
    async fn google_account_prefers_primary_account() {
        let (db, _dir) = open_db().await;
        seed_user(&db, "u1", None).await;
        seed_google_account(&db, "u1", Some("sub@example.com"), false).await;
        seed_google_account(&db, "u1", Some("primary@example.com"), true).await;

        let s = get_agent_settings(&db, "u1").await.unwrap().unwrap();
        assert!(s.google_connected);
        assert_eq!(s.google_account.as_deref(), Some("primary@example.com"));
    }

    #[tokio::test]
    async fn set_gemini_model_updates_only_that_column() {
        let (db, _dir) = open_db().await;
        seed_user(&db, "u1", Some("gemini-2.5-pro")).await;
        seed_google_account(&db, "u1", Some("u1@example.com"), true).await;
        assert!(set_gemini_model(&db, "u1", "gemini-3.5-flash")
            .await
            .unwrap());
        let s = get_agent_settings(&db, "u1").await.unwrap().unwrap();
        assert_eq!(s.model.as_deref(), Some("gemini-3.5-flash"));
        assert!(s.google_connected, "google 連携フラグは不変");
    }

    #[tokio::test]
    async fn set_gemini_model_reports_missing_row_without_creating_one() {
        let (db, _dir) = open_db().await;
        assert!(!set_gemini_model(&db, "ghost", "gemini-3.5-flash")
            .await
            .unwrap());
        assert!(
            get_agent_settings(&db, "ghost").await.unwrap().is_none(),
            "行は作られない"
        );
    }
}
