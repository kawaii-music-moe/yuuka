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
    pub google_connected: bool,
}

/// `(model, google連携有無)` を返す（行が無ければ `None`＝未登録ユーザー）。
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
                "SELECT gemini_model, google_refresh_token_encrypted \
                 FROM users WHERE discord_id = ?1",
                params![id],
                |r| {
                    let model: Option<String> = r.get(0)?;
                    let google_refresh: Option<String> = r.get(1)?;
                    Ok(UserAgentSettings {
                        model,
                        google_connected: google_refresh.is_some(),
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
/// `password_hash`/`salt` 付きで作る唯一の正で、PWA 設定の書き込みが仮の認証情報で行を作ると
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

    async fn seed_user(db: &Db, id: &str, model: Option<&str>, google_refresh: Option<&str>) {
        let (id, model, google_refresh) = (
            id.to_owned(),
            model.map(str::to_owned),
            google_refresh.map(str::to_owned),
        );
        db.writer
            .transaction(move |tx| {
                tx.execute(
                    "INSERT INTO users \
                       (discord_id, username, password_hash, salt, gemini_model, \
                        google_refresh_token_encrypted) \
                     VALUES (?1, ?1, 'x', 'x', ?2, ?3)",
                    params![id, model, google_refresh],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
            .expect("seed user");
    }

    #[tokio::test]
    async fn unregistered_user_yields_none() {
        let (db, _dir) = open_db().await;
        assert!(get_agent_settings(&db, "ghost").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn reads_model_and_google_connected_flag() {
        let (db, _dir) = open_db().await;
        seed_user(&db, "u1", Some("gemini-2.5-pro"), Some("enc")).await;
        seed_user(&db, "u2", None, None).await;

        let s1 = get_agent_settings(&db, "u1").await.unwrap().unwrap();
        assert_eq!(s1.model.as_deref(), Some("gemini-2.5-pro"));
        assert!(s1.google_connected);

        let s2 = get_agent_settings(&db, "u2").await.unwrap().unwrap();
        assert_eq!(s2.model, None);
        assert!(!s2.google_connected);
    }

    #[tokio::test]
    async fn set_gemini_model_updates_only_that_column() {
        let (db, _dir) = open_db().await;
        seed_user(&db, "u1", Some("gemini-2.5-pro"), Some("enc")).await;
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
