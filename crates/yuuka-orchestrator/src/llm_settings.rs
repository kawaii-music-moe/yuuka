//! Bot-specific generation settings; deployment defaults apply until explicitly saved.
use rusqlite::{params, OptionalExtension};
use yuuka_core::DbError;
use yuuka_db::map_sqlite;
use yuuka_gemini::LlmSettings;
use yuuka_web::Db;

pub async fn get(db: &Db, bot_id: &str) -> Result<LlmSettings, DbError> {
    let id = bot_id.to_owned();
    let row = db
        .read
        .read(move |conn| {
            conn.query_row(
                "SELECT llm_provider, llm_model FROM bots WHERE id = ?1",
                params![id],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await?;
    let defaults = LlmSettings::default();
    let (provider, model) = row.unwrap_or((None, None));
    LlmSettings::validate(
        provider.as_deref().unwrap_or(&defaults.provider),
        model.as_deref().unwrap_or(&defaults.model),
    )
    .map_err(|e| DbError::Operation(e.to_string()))
}

pub async fn save(db: &Db, bot_id: &str, settings: Option<LlmSettings>) -> Result<(), DbError> {
    let id = bot_id.to_owned();
    let (provider, model) = settings
        .map(|s| (Some(s.provider), Some(s.model)))
        .unwrap_or((None, None));
    db.writer.transaction(move |tx| {
        tx.execute("UPDATE bots SET llm_provider = ?1, llm_model = ?2, updated_at = datetime('now','localtime') WHERE id = ?3", params![provider, model, id]).map_err(map_sqlite)?;
        Ok(())
    }).await
}
