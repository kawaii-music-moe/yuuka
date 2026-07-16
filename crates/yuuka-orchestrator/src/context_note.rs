//! コンテキストノート（`context_notes`）を systemInstruction へ注入するシーム（Node `gemini.ts:170-175`・§3.7.3）。
//!
//! Node `buildSystemInstruction`（秘書モード）は persona/固定ルールの後に、ユーザーが登録した背景情報
//! （`context_notes.content`）を「# コンテキストノート」セクションとして注入する。保存系（tool
//! `getContextNote`/`setContextNote`/`appendContextNote` 3 本 + Web `/api/context-note` GET/POST 2 本）は
//! 既存だが**読み戻しが無い silent 後退**（例:「乳製品アレルギー」を登録しても会話で考慮されない）を解消する。
//! guild/DM の `buildGuildSystemInstruction` は Node でも非注入のため、本注入は**秘書ターンのみ**。
//!
//! 注: 読み取りは orchestrator の慣習（`persona`/`message_log`/`synapse_repo` と同様に DB を直接引く）に
//! 従い、`yuuka-personal::ContextNoteRepo` へは依存しない（domain CRUD クレートへの DAG エッジを増やさない）。

use rusqlite::{params, OptionalExtension};
use yuuka_db::map_sqlite;
use yuuka_web::Db;

/// `(user_id, bot_id)` のコンテキストノートを systemInstruction 末尾セクションとして返す。
///
/// 未登録 / 空文字（trim 後）/ 読み取り失敗のいずれも `""` を返す（Node は `getContextNote` を
/// try/catch で包み、失敗時は `contextNoteSection = ""` へデグレードしてターンを落とさない＝それと一致）。
pub async fn build_context_note_section(db: &Db, user_id: &str, bot_id: &str) -> String {
    let (uid, bid) = (user_id.to_owned(), bot_id.to_owned());
    let note = db
        .read
        .read(move |conn| {
            conn.query_row(
                "SELECT content FROM context_notes WHERE user_id = ?1 AND bot_id = ?2",
                params![uid, bid],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await;
    let content = match note {
        Ok(Some(c)) => c,
        Ok(None) => return String::new(),
        Err(e) => {
            tracing::warn!(error = %e, "コンテキストノートの取得に失敗（空セクションへデグレード）");
            return String::new();
        }
    };
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // Node `gemini.ts:175` 逐語（先頭 \n・見出し・案内文・本文は trim 済み）。
    format!(
        "\n# コンテキストノート（ユーザーが「覚えておいてほしい」と登録した背景情報）\n\
         以下はユーザー固有の考慮事項・背景知識です。会話・判断の際に常に考慮してください。\n{trimmed}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    async fn open_db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("t.db");
        // P0-4: Rust は存在しない DB を作らないため空ファイルを先に作る（Db::open が migrations 実行）。
        drop(rusqlite::Connection::open(&path).expect("create empty"));
        let db = Db::open(Path::new(&path)).expect("open db");
        (db, dir)
    }

    async fn set_note(db: &Db, uid: &str, bid: &str, content: &str) {
        let (uid, bid, content) = (uid.to_owned(), bid.to_owned(), content.to_owned());
        db.writer
            .transaction(move |tx| {
                // context_notes.user_id は users(discord_id) への FK（ON DELETE CASCADE）。先に user を用意する。
                tx.execute(
                    "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt) \
                     VALUES (?1, ?1, 'x', 'x')",
                    params![uid],
                )
                .map_err(map_sqlite)?;
                tx.execute(
                    "INSERT INTO context_notes (user_id, bot_id, content, updated_at) \
                     VALUES (?1, ?2, ?3, datetime('now','localtime'))",
                    params![uid, bid, content],
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
            .expect("insert note");
    }

    #[tokio::test]
    async fn no_note_yields_empty_section() {
        let (db, _dir) = open_db().await;
        assert_eq!(build_context_note_section(&db, "u1", "b1").await, "");
    }

    #[tokio::test]
    async fn whitespace_only_note_yields_empty_section() {
        let (db, _dir) = open_db().await;
        set_note(&db, "u1", "b1", "   \n  ").await;
        assert_eq!(build_context_note_section(&db, "u1", "b1").await, "");
    }

    #[tokio::test]
    async fn note_is_formatted_as_section_node_parity() {
        let (db, _dir) = open_db().await;
        // 前後空白は trim される（Node `note.trim()`）。
        set_note(&db, "u1", "b1", "  乳製品アレルギー\n仕事はエンジニア  ").await;
        let section = build_context_note_section(&db, "u1", "b1").await;
        assert_eq!(
            section,
            "\n# コンテキストノート（ユーザーが「覚えておいてほしい」と登録した背景情報）\n\
             以下はユーザー固有の考慮事項・背景知識です。会話・判断の際に常に考慮してください。\n\
             乳製品アレルギー\n仕事はエンジニア"
        );
    }

    #[tokio::test]
    async fn scope_is_isolated_per_user_and_bot() {
        let (db, _dir) = open_db().await;
        set_note(&db, "u1", "b1", "u1のノート").await;
        // 別ユーザー・別 Bot は空（PK(user_id, bot_id) スコープ）。
        assert_eq!(build_context_note_section(&db, "u2", "b1").await, "");
        assert_eq!(build_context_note_section(&db, "u1", "b2").await, "");
    }
}
