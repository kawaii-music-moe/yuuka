//! yuuka-personal — personal（連絡先）ドメイン（T1 fan-out）。**参照実装は yuuka-todo**。
//!
//! Repo + wire DTO + route を縦に持つ。凍結契約（core/types）は変更しない。
//! DAG: `personal → web, db, types, core`。ルータは supervisor が共通レイヤ配下にマージする。
//!
//! 参照スコープ = 連絡先のコア CRUD（list / save〔add|update〕 / delete）＋コンテキストノート
//! （`/api/context-note` GET/POST・`context_notes` 表）＋クリップボード（`/api/clipboard` GET・
//! `/api/clipboard/delete` POST・`clipboard_entries` 表）＋クリップボードの追加ツール
//! （`addClipboardEntry`・[`tools`]・TTL 付き `ClipboardRepo::add`）。cron は誕生日リマインド
//! （`listBirthdayContactsForDate` / `markBirthdayReminded`）と TTL 一括削除（`deleteExpired`・
//! `yuuka-services` の `ClipboardCleanupService`）が全ユーザー横断で稼働。
//! 以下は **deferred**（後続増分で追加）:
//! - 連絡先の部分一致検索（`searchContacts`・`GET` 検索パラメータ）

pub mod cron;
pub mod dto;
pub mod repo;
pub mod routes;
pub mod tools;

pub use routes::routes;
pub use tools::tools;

use std::path::Path;

use ts_rs::TS;

/// 本ドメインの wire DTO を `base_dir/generated/` へ生成する（xtask gen-types が呼ぶ）。
///
/// # Errors
/// ts-rs のシリアライズ／書き込み失敗時 [`ts_rs::ExportError`]。
pub fn export_bindings(base_dir: &Path) -> Result<(), ts_rs::ExportError> {
    let cfg = ts_rs::Config::new().with_out_dir(base_dir.to_path_buf());
    <dto::Contact as TS>::export_all(&cfg)?;
    <dto::NewContact as TS>::export_all(&cfg)?;
    <dto::ContactListData as TS>::export_all(&cfg)?;
    <dto::ContactData as TS>::export_all(&cfg)?;
    <dto::ClipboardEntry as TS>::export_all(&cfg)?;
    <dto::ClipboardListData as TS>::export_all(&cfg)?;
    <dto::ContextNoteData as TS>::export_all(&cfg)?;
    <dto::SetContextNote as TS>::export_all(&cfg)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
    use yuuka_core::{AuthError, BotId, UserId, UserScope};
    use yuuka_types::{Role, SessionUser};
    use yuuka_web::{AppState, AuthBackend, Db, WebConfig};

    use crate::dto::NewContact;
    use crate::repo::{ClipboardRepo, ContactRepo, ContextNoteRepo};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    // Node migrations.ts の contacts DDL + v3 で後付けされる bot_id 列を反映。
    const CONTACTS_DDL: &str = "CREATE TABLE contacts (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        name TEXT NOT NULL,
        birthday TEXT,
        relationship TEXT,
        contact_info TEXT,
        notes TEXT,
        tags TEXT NOT NULL DEFAULT '[]',
        birthday_reminded_year INTEGER,
        created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
    );";

    // V17__baseline.sql の clipboard_entries を反映（bot_id は後付け列・default system_default）。
    const CLIPBOARD_DDL: &str = "CREATE TABLE clipboard_entries (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        content TEXT NOT NULL,
        expires_at TEXT,
        created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        bot_id TEXT NOT NULL DEFAULT 'system_default'
    );";

    // V17__baseline.sql の context_notes を反映（PK(user_id, bot_id)・upsert 対象）。
    const CONTEXT_NOTES_DDL: &str = "CREATE TABLE context_notes (
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        content TEXT NOT NULL DEFAULT '',
        updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        PRIMARY KEY (user_id, bot_id)
    );";

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_personal_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        {
            let conn = rusqlite::Connection::open(&path).expect("seed db");
            conn.execute_batch(CONTACTS_DDL).expect("create contacts");
            conn.execute_batch(CLIPBOARD_DDL).expect("create clipboard");
            conn.execute_batch(CONTEXT_NOTES_DDL)
                .expect("create context_notes");
        }
        Db::open(&path).expect("open db")
    }

    fn scope(user: &str) -> UserScope {
        UserScope::new(UserId::new(user), BotId::system_default())
    }

    fn new_contact(name: &str, tags: Vec<String>) -> NewContact {
        NewContact {
            id: None,
            name: name.to_owned(),
            birthday: None,
            relationship: None,
            contact_info: None,
            notes: None,
            tags,
        }
    }

    #[tokio::test]
    async fn add_list_and_scope_isolation() {
        let db = seed_db();
        let repo = ContactRepo::new(&db);
        repo.add(&scope("userA"), new_contact("Alice", vec!["friend".to_owned()]))
            .await
            .unwrap();

        let listed = repo.list(&scope("userA")).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "Alice");
        assert_eq!(listed[0].tags, vec!["friend".to_owned()]);

        // 別ユーザーには見えない（分離キーを型で強制）。
        assert!(repo.list(&scope("userB")).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn update_and_delete() {
        let db = seed_db();
        let repo = ContactRepo::new(&db);
        let created = repo
            .add(&scope("u"), new_contact("Bob", vec![]))
            .await
            .unwrap();

        let updated = repo
            .update(
                &scope("u"),
                created.id,
                NewContact {
                    id: Some(created.id),
                    name: "Bob R.".to_owned(),
                    birthday: Some("--12-25".to_owned()),
                    relationship: Some("colleague".to_owned()),
                    contact_info: None,
                    notes: None,
                    tags: vec!["work".to_owned()],
                },
            )
            .await
            .unwrap()
            .expect("updated row");
        assert_eq!(updated.name, "Bob R.");
        assert_eq!(updated.birthday.as_deref(), Some("--12-25"));
        assert_eq!(updated.relationship.as_deref(), Some("colleague"));
        assert_eq!(updated.tags, vec!["work".to_owned()]);

        assert!(repo.delete(&scope("u"), created.id).await.unwrap());
        assert!(repo.list(&scope("u")).await.unwrap().is_empty());
        // 二重削除は false。
        assert!(!repo.delete(&scope("u"), created.id).await.unwrap());
    }

    #[tokio::test]
    async fn update_outside_scope_is_noop() {
        let db = seed_db();
        let repo = ContactRepo::new(&db);
        let created = repo
            .add(&scope("userA"), new_contact("Carol", vec![]))
            .await
            .unwrap();

        // 別ユーザーは他人の連絡先を更新できない（該当行 0 → None）。
        let res = repo
            .update(
                &scope("userB"),
                created.id,
                new_contact("Hijack", vec![]),
            )
            .await
            .unwrap();
        assert!(res.is_none());

        // 元の行は不変。
        let orig = repo
            .get(&scope("userA"), created.id)
            .await
            .unwrap()
            .expect("still there");
        assert_eq!(orig.name, "Carol");
    }

    struct FakeAuth {
        user: SessionUser,
    }

    #[async_trait]
    impl AuthBackend for FakeAuth {
        async fn session_user(&self, token: &str) -> Result<Option<SessionUser>, AuthError> {
            Ok((token == "good").then(|| self.user.clone()))
        }
        async fn desktop_user(&self, _token: &str) -> Result<Option<SessionUser>, AuthError> {
            Ok(None)
        }
    }

    fn app() -> axum::Router {
        let auth = Arc::new(FakeAuth {
            user: SessionUser {
                discord_id: "u".to_owned(),
                username: "u".to_owned(),
                role: Role::User,
            },
        });
        let state = AppState::new(auth, WebConfig::default(), seed_db());
        super::routes().with_state(state)
    }

    #[tokio::test]
    async fn route_save_then_list() {
        let app = app();
        let save = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/contacts/save")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"Dave","tags":["gym"],"birthday":"1990-01-02"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(save.status(), StatusCode::OK);

        let list = app
            .oneshot(
                Request::builder()
                    .uri("/api/contacts")
                    .header("cookie", "__Host-yuuka-session=good")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(list.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["success"], serde_json::json!(true));
        assert_eq!(j["contacts"][0]["name"], serde_json::json!("Dave"));
        assert_eq!(j["contacts"][0]["tags"][0], serde_json::json!("gym"));
        assert_eq!(j["contacts"][0]["birthday"], serde_json::json!("1990-01-02"));
        // 内部列は露出しない（構造的フェイルクローズ）。
        assert!(j["contacts"][0]["user_id"].is_null());
        assert!(j["contacts"][0]["bot_id"].is_null());
        assert!(j["contacts"][0]["birthday_reminded_year"].is_null());
    }

    #[tokio::test]
    async fn route_save_rejects_bad_birthday() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/contacts/save")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"Eve","birthday":"not-a-date"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn route_requires_auth() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/contacts")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// wire 契約の直接凍結: `contactInfo` は camelCase で受理し、snake_case は拾わない。
    #[test]
    fn new_contact_wire_contract_camelcase() {
        let camel: NewContact =
            serde_json::from_str(r#"{"name":"Zoe","contactInfo":"zoe@example.com"}"#).unwrap();
        assert_eq!(camel.contact_info.as_deref(), Some("zoe@example.com"));

        // snake_case は拾われない（これが update 時の NULL 消去の原因だった）。
        let snake: NewContact =
            serde_json::from_str(r#"{"name":"Zoe","contact_info":"zoe@example.com"}"#).unwrap();
        assert_eq!(snake.contact_info, None);
    }

    /// H-1 回帰: update は全列上書きのため、camelCase 欠落だと `contactInfo` が NULL で消去された。
    /// camelCase 受理により、同値を送る update で連絡先情報が保持されることを凍結する。
    #[tokio::test]
    async fn route_save_update_preserves_contact_info() {
        let app = app();
        // 新規作成（contactInfo つき）。
        let create = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/contacts/save")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"Frank","contactInfo":"frank@example.com"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(create.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            j["contact"]["contact_info"],
            serde_json::json!("frank@example.com")
        );
        let id = j["contact"]["id"].as_i64().expect("contact id");

        // 同じ contactInfo を送って update → 保持される（NULL 消去しない）。
        let update = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/contacts/save")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"id":{id},"name":"Frank R.","contactInfo":"frank@example.com"}}"#
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(update.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(update.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            j["contact"]["contact_info"],
            serde_json::json!("frank@example.com")
        );
        assert_eq!(j["contact"]["name"], serde_json::json!("Frank R."));
    }

    // ─── クリップボード（repo） ──────────────────────────────────────────────

    /// 期限付き（`expires_at`）でエントリを直接挿入する（add は deferred のためテスト用ヘルパ）。
    async fn seed_clipboard(db: &Db, scope: &UserScope, content: &str, expires_at: Option<&str>) {
        let uid = scope.user_id().as_str().to_owned();
        let bid = scope.bot_id().as_str().to_owned();
        let content = content.to_owned();
        let expires_at = expires_at.map(str::to_owned);
        db.writer
            .execute(move |conn| {
                conn.execute(
                    "INSERT INTO clipboard_entries (user_id, bot_id, content, expires_at) \
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![uid, bid, content, expires_at],
                )
                .map_err(yuuka_db::map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn clipboard_list_filters_expired_and_scopes() {
        let db = seed_db();
        // 無期限・未来期限は見える。過去期限は除外。別ユーザーは不可視。
        seed_clipboard(&db, &scope("cbA"), "forever", None).await;
        seed_clipboard(&db, &scope("cbA"), "future", Some("2999-12-31 00:00:00")).await;
        seed_clipboard(&db, &scope("cbA"), "past", Some("2000-01-01 00:00:00")).await;
        seed_clipboard(&db, &scope("cbB"), "otheruser", None).await;

        let repo = ClipboardRepo::new(&db);
        let entries = repo.list(&scope("cbA")).await.unwrap();
        let contents: Vec<&str> = entries.iter().map(|e| e.content.as_str()).collect();
        assert!(contents.contains(&"forever"));
        assert!(contents.contains(&"future"));
        assert!(!contents.contains(&"past"), "expired entry must be filtered");
        assert!(
            !contents.contains(&"otheruser"),
            "other user's entry must not leak"
        );
        assert_eq!(entries.len(), 2);
    }

    #[tokio::test]
    async fn clipboard_delete_respects_scope() {
        let db = seed_db();
        seed_clipboard(&db, &scope("cbUser"), "note", None).await;
        let repo = ClipboardRepo::new(&db);
        let id = repo.list(&scope("cbUser")).await.unwrap()[0].id;

        // 別ユーザーは削除できない（該当行 0 → false）。
        assert!(!repo.delete(&scope("intruder"), id).await.unwrap());
        // 本人は削除できる。二重削除は false。
        assert!(repo.delete(&scope("cbUser"), id).await.unwrap());
        assert!(!repo.delete(&scope("cbUser"), id).await.unwrap());
        assert!(repo.list(&scope("cbUser")).await.unwrap().is_empty());
    }

    // ─── コンテキストノート（repo） ──────────────────────────────────────────

    #[tokio::test]
    async fn context_note_get_default_is_empty() {
        let db = seed_db();
        let repo = ContextNoteRepo::new(&db);
        let (content, updated_at) = repo.get(&scope("cn")).await.unwrap();
        assert_eq!(content, "");
        assert_eq!(updated_at, None);
    }

    #[tokio::test]
    async fn context_note_set_upserts_and_scopes() {
        let db = seed_db();
        let repo = ContextNoteRepo::new(&db);
        repo.set(&scope("cnA"), "first".to_owned()).await.unwrap();
        let (content, updated_at) = repo.get(&scope("cnA")).await.unwrap();
        assert_eq!(content, "first");
        assert!(updated_at.is_some(), "updated_at set on write");

        // 同一 scope への 2 回目は upsert（全体置換）。
        repo.set(&scope("cnA"), "second".to_owned()).await.unwrap();
        assert_eq!(repo.get(&scope("cnA")).await.unwrap().0, "second");

        // 別ユーザーは独立（分離キーを型で強制）。
        assert_eq!(repo.get(&scope("cnB")).await.unwrap().0, "");
    }

    // ─── クリップボード・コンテキストノート（route） ─────────────────────────

    #[tokio::test]
    async fn route_context_note_roundtrip() {
        let app = app();
        // 初期は空・max_length 同梱。
        let get0 = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/context-note")
                    .header("cookie", "__Host-yuuka-session=good")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get0.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(get0.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["success"], serde_json::json!(true));
        assert_eq!(j["content"], serde_json::json!(""));
        assert_eq!(j["updated_at"], serde_json::Value::Null);
        assert_eq!(j["max_length"], serde_json::json!(10_000));

        // 保存。
        let post = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/context-note")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"content":"覚えておいてね"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(post.status(), StatusCode::OK);

        // 再取得で保存内容が返る。
        let get1 = app
            .oneshot(
                Request::builder()
                    .uri("/api/context-note")
                    .header("cookie", "__Host-yuuka-session=good")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(get1.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["content"], serde_json::json!("覚えておいてね"));
        assert!(j["updated_at"].is_string());
    }

    #[tokio::test]
    async fn route_context_note_rejects_oversize() {
        // 上限（10,000 文字）超で 400 + Node 同一メッセージ（3 桁区切り）。
        let over = "あ".repeat(10_001);
        let body = serde_json::json!({ "content": over }).to_string();
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/context-note")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["success"], serde_json::json!(false));
        assert_eq!(
            j["message"],
            serde_json::json!("コンテキストノートは10,000文字以内です（現在: 10,001文字）")
        );
    }

    #[tokio::test]
    async fn route_clipboard_delete_missing_is_200_false() {
        // Node parity: 該当無でも 200 + {success:false, message}。
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/clipboard/delete")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"id":99999}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["success"], serde_json::json!(false));
        assert_eq!(j["message"], serde_json::json!("メモが見つかりません。"));
    }

    #[tokio::test]
    async fn route_clipboard_requires_auth() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/clipboard")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
