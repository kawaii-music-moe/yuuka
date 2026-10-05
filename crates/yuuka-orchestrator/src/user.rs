//! ユーザー個別設定の読み取り（Gemini キー・リッチ返信フラグ）— Node `userRepo`/`llmClient` パリティ。
//!
//! 秘書経路の LLM 呼び出しは**発話ユーザー本人の Gemini キー**を使う（Node `getUserGenAI` → `users`
//! テーブル）。キーは `system_settings` 由来のシステム鍵で AES-256-GCM 暗号化されている（P1-5）。

use rusqlite::{params, OptionalExtension};
use yuuka_core::DbError;
use yuuka_db::map_sqlite;
use yuuka_web::Db;

/// ユーザーの暗号化 Gemini 設定（Node `getUserGeminiConfig`）。3 列が揃っていれば復号可能。
#[derive(Debug, Clone)]
pub struct UserGemini {
    pub encrypted: String,
    pub iv: String,
    pub tag: String,
    /// モデル名（未設定は既定 `gemini-3.1-flash-lite`）。
    pub model: String,
}

/// `users` の Gemini 4 列（enc/iv/tag/model・いずれも nullable）を読み出す生タプル。
type RawGeminiRow = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// ユーザーの Gemini 設定を引く。暗号 3 列のいずれかが欠ければ `None`（＝キー未設定）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn user_gemini(db: &Db, user_id: &str) -> Result<Option<UserGemini>, DbError> {
    let user_id = user_id.to_owned();
    db.read
        .read(move |conn| {
            let row: Option<RawGeminiRow> = conn
                .query_row(
                    "SELECT gemini_api_key_encrypted, gemini_api_key_iv, gemini_api_key_tag, gemini_model \
                     FROM users WHERE discord_id = ?1",
                    params![user_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()
                .map_err(map_sqlite)?;
            let Some((Some(encrypted), Some(iv), Some(tag), model)) = row else {
                return Ok(None);
            };
            Ok(Some(UserGemini {
                encrypted,
                iv,
                tag,
                model: model.filter(|m| !m.is_empty()).unwrap_or_else(|| "gemini-3.1-flash-lite".to_owned()),
            }))
        })
        .await
}

/// リッチ返信が有効か（Node `getUserRichReplyEnabled`・既定 true・行なし/エラーは true）。
pub async fn rich_reply_enabled(db: &Db, user_id: &str) -> bool {
    let user_id = user_id.to_owned();
    let result = db
        .read
        .read(move |conn| {
            conn.query_row(
                "SELECT rich_reply_enabled FROM users WHERE discord_id = ?1",
                params![user_id],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await;
    match result {
        Ok(Some(v)) => v != 0,
        // 行なし・読み取り失敗は Node 既定に合わせ true（リッチ返信オン）。
        Ok(None) | Err(_) => true,
    }
}
