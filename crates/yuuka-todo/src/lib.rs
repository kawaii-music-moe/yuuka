//! yuuka-todo — todo ドメイン（T1 **参照実装**）: Repo + wire DTO + route を縦に持つ。
//!
//! 残り8ドメイン（finance/schedule/timeline/reminder/personal/credential/playbook/persona）は
//! 本クレートを雛形に per-domain クレートで並行実装する。凍結契約（core/types）は変更しない。
//! DAG: `todo → web, db, types, core`。ルータは supervisor が共通レイヤ配下にマージする。
//!
//! Phase 1 参照スコープ = コア CRUD（list/add/complete/delete）。recurrence・finance 連携・
//! gantt/someday・progress log・優先度2段・15 tool function・subtree 削除は todo 完成パスで追加。

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
    <dto::Todo as TS>::export_all(&cfg)?;
    <dto::NewTodo as TS>::export_all(&cfg)?;
    <dto::TaskListData as TS>::export_all(&cfg)?;
    <dto::TaskData as TS>::export_all(&cfg)?;
    <dto::DeletedData as TS>::export_all(&cfg)?;
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

    use crate::dto::NewTodo;
    use crate::repo::TodoRepo;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    const TODOS_DDL: &str = "CREATE TABLE todos (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        title TEXT NOT NULL,
        description TEXT,
        due_date TEXT,
        start_date TEXT,
        priority TEXT,
        tags TEXT NOT NULL DEFAULT '[]',
        status TEXT NOT NULL DEFAULT 'open',
        progress INTEGER NOT NULL DEFAULT 0,
        parent_id INTEGER,
        linked_payment_id INTEGER,
        due_reminded INTEGER NOT NULL DEFAULT 0,
        repeat_rule TEXT,
        repeat_until TEXT,
        repeat_count INTEGER,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );";

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir()
            .join(format!("yuuka_todo_test_{}_{seq}.sqlite", std::process::id()));
        {
            let conn = rusqlite::Connection::open(&path).expect("seed db");
            conn.execute_batch(TODOS_DDL).expect("create todos");
        }
        Db::open(&path).expect("open db")
    }

    fn scope(user: &str) -> UserScope {
        UserScope::new(UserId::new(user), BotId::system_default())
    }

    fn new_todo(title: &str, tags: Vec<String>) -> NewTodo {
        NewTodo {
            title: title.to_owned(),
            description: None,
            due_date: None,
            start_date: None,
            priority: None,
            tags,
            parent_id: None,
        }
    }

    #[tokio::test]
    async fn add_list_and_scope_isolation() {
        let db = seed_db();
        let repo = TodoRepo::new(&db);
        repo.add(&scope("userA"), new_todo("a-task", vec!["x".to_owned()]))
            .await
            .unwrap();

        let listed = repo.list(&scope("userA")).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "a-task");
        assert_eq!(listed[0].tags, vec!["x".to_owned()]);
        assert_eq!(listed[0].status, "open");

        // 別ユーザーには見えない（分離キーを型で強制）。
        assert!(repo.list(&scope("userB")).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn complete_and_delete() {
        let db = seed_db();
        let repo = TodoRepo::new(&db);
        let created = repo.add(&scope("u"), new_todo("t", vec![])).await.unwrap();

        let done = repo
            .complete(&scope("u"), created.id)
            .await
            .unwrap()
            .expect("completed row");
        assert_eq!(done.status, "done");
        // Node parity: complete は progress を変更しない（作成時の 0 のまま）。
        assert_eq!(done.progress, 0);

        assert!(repo.delete(&scope("u"), created.id).await.unwrap());
        assert!(repo.list(&scope("u")).await.unwrap().is_empty());
        // 二重削除は false。
        assert!(!repo.delete(&scope("u"), created.id).await.unwrap());
    }

    #[tokio::test]
    async fn parent_id_outside_scope_is_demoted() {
        let db = seed_db();
        let repo = TodoRepo::new(&db);
        let parent = repo
            .add(&scope("userA"), new_todo("parent", vec![]))
            .await
            .unwrap();

        // 別ユーザーが userA の todo を親に指定 → NULL に降格（クロススコープ参照を防ぐ）。
        let cross = repo
            .add(
                &scope("userB"),
                NewTodo {
                    title: "child".to_owned(),
                    description: None,
                    due_date: None,
                    start_date: None,
                    priority: None,
                    tags: vec![],
                    parent_id: Some(parent.id),
                },
            )
            .await
            .unwrap();
        assert_eq!(cross.parent_id, None);

        // 同一スコープの親は保持。
        let sibling = repo
            .add(
                &scope("userA"),
                NewTodo {
                    title: "sibling".to_owned(),
                    description: None,
                    due_date: None,
                    start_date: None,
                    priority: None,
                    tags: vec![],
                    parent_id: Some(parent.id),
                },
            )
            .await
            .unwrap();
        assert_eq!(sibling.parent_id, Some(parent.id));
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
    async fn route_add_then_list() {
        let app = app();
        let add = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks/add")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"title":"hello","tags":["home"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(add.status(), StatusCode::OK);

        let list = app
            .oneshot(
                Request::builder()
                    .uri("/api/tasks")
                    .header("cookie", "__Host-yuuka-session=good")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(list.into_body(), usize::MAX).await.unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["success"], serde_json::json!(true));
        assert_eq!(j["tasks"][0]["title"], serde_json::json!("hello"));
        assert_eq!(j["tasks"][0]["tags"][0], serde_json::json!("home"));
        // 内部列は露出しない（構造的フェイルクローズ）。
        assert!(j["tasks"][0]["user_id"].is_null());
        assert!(j["tasks"][0]["bot_id"].is_null());
    }

    #[tokio::test]
    async fn route_requires_auth() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/tasks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// H-1 回帰: フロント／Node は **camelCase**（`dueDate`/`startDate`）を送り、旧 UI は
    /// **数値優先度**（`2`）を送る。どちらも無音で落とさず永続化することを凍結する。
    #[tokio::test]
    async fn route_add_accepts_camelcase_and_numeric_priority() {
        let app = app();
        let add = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks/add")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"title":"camel","dueDate":"2026-07-08","startDate":"2026-07-07","priority":2}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(add.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(add.into_body(), usize::MAX).await.unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        // camelCase が `#[serde(default)]` で None に落ちず永続化される。
        assert_eq!(j["task"]["due_date"], serde_json::json!("2026-07-08"));
        assert_eq!(j["task"]["start_date"], serde_json::json!("2026-07-07"));
        // 数値 2 → "high"（Node normalizePriority と一致）。
        assert_eq!(j["task"]["priority"], serde_json::json!("high"));
    }

    /// wire 契約の直接凍結: 入力 DTO は camelCase を受理し、snake_case は拾わない。
    /// `priority` は数値 0/1/2・文字列・不正値を Node `normalizePriority` と同一規則で正規化する。
    #[test]
    fn newtodo_wire_contract_is_camelcase_in() {
        // camelCase を受理する。
        let camel: NewTodo = serde_json::from_str(
            r#"{"title":"t","dueDate":"2026-07-08","startDate":"2026-07-07","parentId":42}"#,
        )
        .unwrap();
        assert_eq!(camel.due_date.as_deref(), Some("2026-07-08"));
        assert_eq!(camel.start_date.as_deref(), Some("2026-07-07"));
        assert_eq!(camel.parent_id, Some(42));

        // snake_case は camelCase 契約では拾われない（フロントは camelCase のみ送る）。
        let snake: NewTodo =
            serde_json::from_str(r#"{"title":"t","due_date":"2026-07-08"}"#).unwrap();
        assert_eq!(snake.due_date, None);

        // priority の正規化（Node normalizePriority 厳密一致・不正値は None で 400 にしない）。
        let priority = |body: &str| serde_json::from_str::<NewTodo>(body).unwrap().priority;
        assert_eq!(priority(r#"{"title":"t","priority":2}"#).as_deref(), Some("high"));
        assert_eq!(priority(r#"{"title":"t","priority":1}"#).as_deref(), Some("medium"));
        assert_eq!(priority(r#"{"title":"t","priority":0}"#).as_deref(), Some("low"));
        assert_eq!(
            priority(r#"{"title":"t","priority":"high"}"#).as_deref(),
            Some("high")
        );
        assert_eq!(priority(r#"{"title":"t","priority":""}"#), None);
        assert_eq!(priority(r#"{"title":"t","priority":"bogus"}"#), None);
        assert_eq!(priority(r#"{"title":"t","priority":null}"#), None);
        assert_eq!(priority(r#"{"title":"t","priority":9}"#), None);
        assert_eq!(priority(r#"{"title":"t"}"#), None);
    }
}
