//! コンテキストノートの読み書きの共通経路（D02 #126）。
//!
//! 管理画面の `/api/context-note`・PWA の `/api/client/shared-note`・エージェントのツール
//! （`getContextNote`/`setContextNote`/`appendContextNote`）はすべてここを通す。以前は PWA の読み書きが
//! `yuuka-orchestrator` に別の SQL として実装され、上限の数え方（UTF-16 と `chars()`）も入口ごとに
//! 違っていた。
//!
//! 1 つのノート（ユーザー × エージェントに 1 行）を共有し、本文は全入口で同じものを読み書きする。
//! タイトルは PWA だけが扱う。管理画面とツールの保存はタイトルを変えない。
//!
//! 秘書ターンの systemInstruction への注入（`yuuka-orchestrator/src/context_note.rs`）は読み取り専用で、
//! 失敗時に空へ縮退する別の用途なので、ここを経由しない。

use yuuka_core::{DbError, UserScope};
use yuuka_web::{Db, DomainError};

use crate::dto::ContextNote;
pub use crate::dto::CONTEXT_NOTE_MAX_LENGTH;
use crate::repo::ContextNoteRepo;

/// タイトル未指定（空白のみ）のときに保存・表示するタイトル（Node の既定表示と同じ）。
pub const DEFAULT_NOTE_TITLE: &str = "Shared note";

/// 上限判定に使う文字数（Node の `content.length` と同じ UTF-16 code unit 数）。
#[must_use]
pub fn content_length(content: &str) -> usize {
    content.encode_utf16().count()
}

/// コンテキストノートの読み書き（全入口共通）。
pub struct ContextNoteService<'a> {
    db: &'a Db,
}

impl<'a> ContextNoteService<'a> {
    #[must_use]
    pub fn new(db: &'a Db) -> Self {
        Self { db }
    }

    /// 本文と更新時刻を返す（未登録なら `("", None)`）。
    ///
    /// # Errors
    /// DB 失敗時 [`DomainError::Db`]。
    pub async fn get(&self, scope: &UserScope) -> Result<(String, Option<String>), DomainError> {
        Ok(ContextNoteRepo::new(self.db).get(scope).await?)
    }

    /// タイトル付きで返す（PWA の共有ノート・未登録なら `None`）。
    ///
    /// # Errors
    /// DB 失敗時 [`DomainError::Db`]。
    pub async fn get_note(&self, scope: &UserScope) -> Result<Option<ContextNote>, DomainError> {
        Ok(ContextNoteRepo::new(self.db).get_note(scope).await?)
    }

    /// 本文を全文置換する（管理画面・ツール）。タイトルは変えない。保存した本文の文字数を返す。
    ///
    /// # Errors
    /// 上限（[`CONTEXT_NOTE_MAX_LENGTH`]）超過は [`DomainError::Invalid`]、DB 失敗は
    /// [`DomainError::Db`]。
    pub async fn set_content(
        &self,
        scope: &UserScope,
        content: String,
    ) -> Result<usize, DomainError> {
        let length = check_length(&content)?;
        ContextNoteRepo::new(self.db).set(scope, content).await?;
        Ok(length)
    }

    /// タイトルと本文をまとめて置換し、保存後の行を返す（PWA）。タイトルが空白のみなら
    /// [`DEFAULT_NOTE_TITLE`] を保存する。
    ///
    /// # Errors
    /// 上限（[`CONTEXT_NOTE_MAX_LENGTH`]）超過は [`DomainError::Invalid`]、DB 失敗は
    /// [`DomainError::Db`]。
    pub async fn set_note(
        &self,
        scope: &UserScope,
        title: &str,
        body: String,
    ) -> Result<ContextNote, DomainError> {
        check_length(&body)?;
        let title = if title.trim().is_empty() {
            DEFAULT_NOTE_TITLE.to_owned()
        } else {
            title.to_owned()
        };
        let repo = ContextNoteRepo::new(self.db);
        repo.set_with_title(scope, title, body).await?;
        repo.get_note(scope).await?.ok_or_else(|| {
            DomainError::Db(DbError::Operation(
                "saved context note not found".to_owned(),
            ))
        })
    }
}

/// 上限内なら文字数を返す。超えていれば管理画面・ツールの従来の案内文で [`DomainError::Invalid`]。
fn check_length(content: &str) -> Result<usize, DomainError> {
    let length = content_length(content);
    if length > CONTEXT_NOTE_MAX_LENGTH {
        return Err(DomainError::Invalid(format!(
            "コンテキストノートは{}文字以内です（現在: {}文字）",
            format_thousands(CONTEXT_NOTE_MAX_LENGTH),
            format_thousands(length),
        )));
    }
    Ok(length)
}

/// 整数を 3 桁区切りにする（Node `Number.prototype.toLocaleString()` の桁区切り相当）。
///
/// コンテキストノート上限メッセージの `10,000` 等を Node と一致させるため。
pub(crate) fn format_thousands(n: usize) -> String {
    let digits = n.to_string();
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, ch) in digits.chars().enumerate() {
        // 残り桁数が 3 の倍数になる境界（先頭を除く）でカンマを置く。
        if i != 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use yuuka_core::{BotId, UserId};

    use super::*;

    async fn open_db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        // Rust は存在しない DB を作らないため空ファイルを先に作る（Db::open が migrations を実行）。
        drop(rusqlite::Connection::open(&path).unwrap());
        let db = Db::open(Path::new(&path)).unwrap();
        // context_notes.user_id は users(discord_id) への FK。
        db.writer
            .execute(|conn| {
                conn.execute(
                    "INSERT INTO users (discord_id, username, salt) VALUES ('u1', 'u1', 'x')",
                    [],
                )
                .map_err(yuuka_db::map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();
        (db, dir)
    }

    fn scope() -> UserScope {
        UserScope::new(UserId::new("u1"), BotId::new("b1"))
    }

    #[test]
    fn length_counts_utf16_code_units_like_node() {
        assert_eq!(content_length("あいう"), 3);
        // 絵文字（サロゲートペア）は 2。
        assert_eq!(content_length("😀"), 2);
    }

    #[test]
    fn over_limit_uses_the_existing_message() {
        let body = "😀".repeat(5_001);
        assert!(matches!(
            check_length(&body),
            Err(DomainError::Invalid(ref m))
                if m == "コンテキストノートは10,000文字以内です（現在: 10,002文字）"
        ));
        assert_eq!(check_length(&"a".repeat(10_000)).unwrap(), 10_000);
    }

    #[test]
    fn thousands_separator() {
        assert_eq!(format_thousands(0), "0");
        assert_eq!(format_thousands(999), "999");
        assert_eq!(format_thousands(10_000), "10,000");
        assert_eq!(format_thousands(1_234_567), "1,234,567");
    }

    #[tokio::test]
    async fn pwa_save_persists_title_and_body() {
        let (db, _dir) = open_db().await;
        let service = ContextNoteService::new(&db);
        assert!(service.get_note(&scope()).await.unwrap().is_none());
        let note = service
            .set_note(&scope(), "My Title", "本文です".to_owned())
            .await
            .unwrap();
        assert_eq!(note.title, "My Title");
        assert_eq!(note.body, "本文です");
        assert!(!note.updated_at.is_empty());
    }

    #[tokio::test]
    async fn blank_pwa_title_falls_back_to_default() {
        let (db, _dir) = open_db().await;
        let note = ContextNoteService::new(&db)
            .set_note(&scope(), "   ", "本文".to_owned())
            .await
            .unwrap();
        assert_eq!(note.title, DEFAULT_NOTE_TITLE);
    }

    #[tokio::test]
    async fn body_only_save_keeps_the_pwa_title() {
        let (db, _dir) = open_db().await;
        let service = ContextNoteService::new(&db);
        service
            .set_note(&scope(), "私のノート", "PWA".to_owned())
            .await
            .unwrap();
        assert_eq!(
            service
                .set_content(&scope(), "管理画面".to_owned())
                .await
                .unwrap(),
            4
        );
        let note = service.get_note(&scope()).await.unwrap().unwrap();
        assert_eq!(note.title, "私のノート");
        assert_eq!(note.body, "管理画面");
        // 本文だけの新規保存は列の既定タイトル。
        let other = UserScope::new(UserId::new("u1"), BotId::new("b2"));
        service.set_content(&other, "x".to_owned()).await.unwrap();
        let note = service.get_note(&other).await.unwrap().unwrap();
        assert_eq!(note.title, DEFAULT_NOTE_TITLE);
    }

    #[tokio::test]
    async fn rejected_writes_do_not_touch_the_note() {
        let (db, _dir) = open_db().await;
        let service = ContextNoteService::new(&db);
        service
            .set_content(&scope(), "元の本文".to_owned())
            .await
            .unwrap();
        let over = "😀".repeat(5_001);
        assert!(matches!(
            service.set_content(&scope(), over.clone()).await,
            Err(DomainError::Invalid(_))
        ));
        assert!(matches!(
            service.set_note(&scope(), "t", over).await,
            Err(DomainError::Invalid(_))
        ));
        let (content, _) = service.get(&scope()).await.unwrap();
        assert_eq!(content, "元の本文");
    }
}
