//! yuuka-personal — personal（連絡先）ドメイン（T1 fan-out）。**参照実装は yuuka-todo**。
//!
//! Repo + wire DTO + route を縦に持つ。凍結契約（core/types）は変更しない。
//! DAG: `personal → web, db, types, core`。ルータは supervisor が共通レイヤ配下にマージする。
//!
//! Phase 1 参照スコープ = 連絡先のコア CRUD（list / save〔add|update〕 / delete）。以下は
//! **deferred**（後続増分で追加）:
//! - 誕生日リマインド cron（`listBirthdayContactsForDate` / `markBirthdayReminded`・
//!   `birthday_reminded_year` 更新、全ユーザー横断クエリ）
//! - 連絡先の部分一致検索（`searchContacts`・`GET` 検索パラメータ）
//! - コンテキストノート（`/api/context-note` GET/POST・`context_notes` 表）
//! - クリップボード（`/api/clipboard`・`/api/clipboard/delete`・`clipboard_entries` 表）

pub mod dto;
pub mod repo;
pub mod routes;

pub use routes::routes;

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
    use crate::repo::ContactRepo;

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

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_personal_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        {
            let conn = rusqlite::Connection::open(&path).expect("seed db");
            conn.execute_batch(CONTACTS_DDL).expect("create contacts");
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
}
