//! 適用中ペルソナの解決（Node `getActivePersonaPrompt`）。
//!
//! `bot_active_personas(user_id, bot_id) → persona_id → personas.prompt`。行が無ければ `None`
//! （呼び出し側は既定ペルソナ [`crate::system_prompt::DEFAULT_PERSONA`] へフォールバックする）。

use rusqlite::{params, OptionalExtension};
use yuuka_core::DbError;
use yuuka_db::map_sqlite;
use yuuka_web::Db;

/// `(user_id, bot_id)` に適用中のペルソナ prompt を返す（無ければ `None`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn active_persona_prompt(
    db: &Db,
    user_id: &str,
    bot_id: &str,
) -> Result<Option<String>, DbError> {
    let (user_id, bot_id) = (user_id.to_owned(), bot_id.to_owned());
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT p.prompt FROM bot_active_personas a \
                 JOIN personas p ON p.id = a.persona_id \
                 WHERE a.user_id = ?1 AND a.bot_id = ?2",
                params![user_id, bot_id],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}
