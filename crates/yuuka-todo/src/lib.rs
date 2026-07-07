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
    <dto::Todo as TS>::export_all(&cfg)?;
    <dto::TodoWithSubtasks as TS>::export_all(&cfg)?;
    <dto::NewTodo as TS>::export_all(&cfg)?;
    <dto::TaskListData as TS>::export_all(&cfg)?;
    <dto::TaskData as TS>::export_all(&cfg)?;
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
        let path = std::env::temp_dir().join(format!(
            "yuuka_todo_test_{}_{seq}.sqlite",
            std::process::id()
        ));
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
            tags,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn add_list_and_scope_isolation() {
        let db = seed_db();
        let repo = TodoRepo::new(&db);
        repo.add(&scope("userA"), new_todo("a-task", vec!["x".to_owned()]))
            .await
            .unwrap();

        let listed = repo.list_tree(&scope("userA"), None, None).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "a-task");
        assert_eq!(listed[0].tags, vec!["x".to_owned()]);
        assert_eq!(listed[0].status, "open");

        // 別ユーザーには見えない（分離キーを型で強制）。
        assert!(repo
            .list_tree(&scope("userB"), None, None)
            .await
            .unwrap()
            .is_empty());
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
        assert!(repo
            .list_tree(&scope("u"), None, None)
            .await
            .unwrap()
            .is_empty());
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
                    parent_id: Some(parent.id),
                    ..Default::default()
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
                    parent_id: Some(parent.id),
                    ..Default::default()
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
        let bytes = axum::body::to_bytes(list.into_body(), usize::MAX)
            .await
            .unwrap();
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
        let bytes = axum::body::to_bytes(add.into_body(), usize::MAX)
            .await
            .unwrap();
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
        assert_eq!(
            priority(r#"{"title":"t","priority":2}"#).as_deref(),
            Some("high")
        );
        assert_eq!(
            priority(r#"{"title":"t","priority":1}"#).as_deref(),
            Some("medium")
        );
        assert_eq!(
            priority(r#"{"title":"t","priority":0}"#).as_deref(),
            Some("low")
        );
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

    /// フィールド指定用の最小 `NewTodo`（テスト側で必要な列だけ後から差し替える）。
    fn nt(title: &str) -> NewTodo {
        NewTodo {
            title: title.to_owned(),
            ..Default::default()
        }
    }

    /// H-3: 親のみをルートに、サブタスクを `subtasks` へネストし、`effective_progress` を
    /// 葉の完了率で算出する（子なしは done→100 / 未完→progress）。
    #[tokio::test]
    async fn list_tree_nests_subtasks_and_computes_progress() {
        let db = seed_db();
        let repo = TodoRepo::new(&db);
        let sc = scope("u");
        let parent = repo.add(&sc, nt("parent")).await.unwrap();
        let mut ca = nt("child-a");
        ca.parent_id = Some(parent.id);
        let child_a = repo.add(&sc, ca).await.unwrap();
        let mut cb = nt("child-b");
        cb.parent_id = Some(parent.id);
        repo.add(&sc, cb).await.unwrap();
        // 葉 2 件のうち 1 件を完了 → 50%。
        repo.complete(&sc, child_a.id).await.unwrap();

        let tree = repo.list_tree(&sc, None, None).await.unwrap();
        assert_eq!(tree.len(), 1, "親のみルート");
        let root = &tree[0];
        assert_eq!(root.id, parent.id);
        assert_eq!(root.subtasks.len(), 2);
        assert_eq!(root.effective_progress, 50);

        let done_leaf = root
            .subtasks
            .iter()
            .find(|t| t.id == child_a.id)
            .expect("done leaf");
        assert_eq!(done_leaf.status, "done");
        assert_eq!(done_leaf.effective_progress, 100);
        let open_leaf = root
            .subtasks
            .iter()
            .find(|t| t.id != child_a.id)
            .expect("open leaf");
        assert_eq!(open_leaf.effective_progress, 0);
    }

    /// H-3: `status` フィルタは親に適用（pending→open / done / all）。
    #[tokio::test]
    async fn list_tree_status_filter() {
        let db = seed_db();
        let repo = TodoRepo::new(&db);
        let sc = scope("u");
        let open_root = repo.add(&sc, nt("open-root")).await.unwrap();
        let done_root = repo.add(&sc, nt("done-root")).await.unwrap();
        repo.complete(&sc, done_root.id).await.unwrap();

        let open_only = repo
            .list_tree(&sc, Some("open".to_owned()), None)
            .await
            .unwrap();
        assert_eq!(open_only.len(), 1);
        assert_eq!(open_only[0].id, open_root.id);

        let done_only = repo
            .list_tree(&sc, Some("done".to_owned()), None)
            .await
            .unwrap();
        assert_eq!(done_only.len(), 1);
        assert_eq!(done_only[0].id, done_root.id);

        // None（=all）は両方。
        assert_eq!(repo.list_tree(&sc, None, None).await.unwrap().len(), 2);
    }

    /// H-3: `tag` フィルタは json_each で照合し、該当タグを持つ親のみ返す。
    #[tokio::test]
    async fn list_tree_tag_filter() {
        let db = seed_db();
        let repo = TodoRepo::new(&db);
        let sc = scope("u");
        let mut work = nt("work-task");
        work.tags = vec!["work".to_owned()];
        let mut home = nt("home-task");
        home.tags = vec!["home".to_owned()];
        let w = repo.add(&sc, work).await.unwrap();
        repo.add(&sc, home).await.unwrap();

        let filtered = repo
            .list_tree(&sc, None, Some("work".to_owned()))
            .await
            .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, w.id);
    }

    /// H-3: ORDER_CLAUSE = 優先度（high→medium→low→未設定）→ 期限 → 作成日時降順。
    #[tokio::test]
    async fn list_tree_sorts_by_priority() {
        let db = seed_db();
        let repo = TodoRepo::new(&db);
        let sc = scope("u");
        for (title, prio) in [
            ("low-t", Some("low")),
            ("high-t", Some("high")),
            ("med-t", Some("medium")),
            ("none-t", None),
        ] {
            let mut t = nt(title);
            t.priority = prio.map(str::to_owned);
            repo.add(&sc, t).await.unwrap();
        }
        let tree = repo.list_tree(&sc, None, None).await.unwrap();
        let titles: Vec<&str> = tree.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["high-t", "med-t", "low-t", "none-t"]);
    }

    /// H-3 E2E: `GET /api/tasks` がネストしたツリーを返し、内部列を露出しない。
    #[tokio::test]
    async fn route_tasks_returns_nested_tree_without_internal_columns() {
        let app = app();
        // 親を追加し id を取得。
        let add_parent = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks/add")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"title":"P"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(add_parent.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let pid = j["task"]["id"].as_i64().expect("parent id");

        // サブタスクを親の下に追加。
        let add_child = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks/add")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"title":"C","parentId":{pid}}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(add_child.status(), StatusCode::OK);

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
        let bytes = axum::body::to_bytes(list.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        // 親のみルート、サブタスクはネスト。
        assert_eq!(j["tasks"][0]["title"], serde_json::json!("P"));
        assert_eq!(
            j["tasks"][0]["subtasks"][0]["title"],
            serde_json::json!("C")
        );
        assert!(j["tasks"][0]["effective_progress"].is_number());
        // 内部列は露出しない（clean view の凍結）。
        assert!(j["tasks"][0]["user_id"].is_null());
        assert!(j["tasks"][0]["bot_id"].is_null());
        assert!(j["tasks"][0]["linked_payment_id"].is_null());
        assert!(j["tasks"][0]["subtasks"][0]["user_id"].is_null());
    }

    /// M-12 golden: `POST /api/tasks/delete` は `200 {success:<bool>}` を返す。
    /// 実在削除は `success:true`、該当無（二重削除）は **404 ではなく** `200 {success:false}`。
    /// いずれも `deletedId`/`deleted_id` を返さない（`*DeletedData` 幽霊型の除去）。
    #[tokio::test]
    async fn route_delete_returns_bare_success_without_deleted_id() {
        let app = app();
        let add = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks/add")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"title":"to-delete"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(add.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let id = j["task"]["id"].as_i64().expect("id");

        let del = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks/delete")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"id":{id}}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(del.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(del.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["success"], serde_json::json!(true));
        // deletedId は返さない（Node parity）。
        assert!(j["deletedId"].is_null());
        assert!(j["deleted_id"].is_null());

        // 二重削除（該当無）は 404 ではなく 200 {success:false}。
        let del2 = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks/delete")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"id":{id}}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(del2.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(del2.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["success"], serde_json::json!(false));
    }

    /// M-12 golden: `POST /api/tasks/complete` の該当無は **404 ではなく** `200 {success:false}`
    /// （`task` キーは付かない・Node `{success:!!todo, task}` パリティ）。
    #[tokio::test]
    async fn route_complete_missing_returns_success_false() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks/complete")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"id":999999}"#))
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
        assert!(j["task"].is_null());
    }
}
