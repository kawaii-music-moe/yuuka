//! yuuka-playbook — playbook ドメイン（T1 fan-out・**参照実装は yuuka-todo**）:
//! Repo + wire DTO + route を縦に持つ。
//!
//! 凍結契約（core/types）は変更しない。DAG: `playbook → web, db, types, core`。
//! ルータは supervisor が共通レイヤ配下にマージする。
//!
//! Phase 1 参照スコープ = コア CRUD（list/save/delete、name キーの upsert）。
//! schedules/runs（cron・定期実行・履歴）は deferred。

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
    <dto::Playbook as TS>::export_all(&cfg)?;
    <dto::NewPlaybook as TS>::export_all(&cfg)?;
    <dto::PlaybookListData as TS>::export_all(&cfg)?;
    <dto::PlaybookData as TS>::export_all(&cfg)?;
    <dto::PlaybookDeletedData as TS>::export_all(&cfg)?;
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

    use crate::dto::NewPlaybook;
    use crate::repo::PlaybookRepo;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    const PLAYBOOKS_DDL: &str = "CREATE TABLE playbooks (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        name TEXT NOT NULL,
        title TEXT NOT NULL,
        keywords TEXT DEFAULT '[]',
        description TEXT DEFAULT '',
        steps TEXT NOT NULL DEFAULT '',
        created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        UNIQUE(user_id, bot_id, name)
    );";

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir()
            .join(format!("yuuka_playbook_test_{}_{seq}.sqlite", std::process::id()));
        {
            let conn = rusqlite::Connection::open(&path).expect("seed db");
            conn.execute_batch(PLAYBOOKS_DDL).expect("create playbooks");
        }
        Db::open(&path).expect("open db")
    }

    fn scope(user: &str) -> UserScope {
        UserScope::new(UserId::new(user), BotId::system_default())
    }

    fn new_playbook(name: &str, title: &str, keywords: Vec<String>) -> NewPlaybook {
        NewPlaybook {
            name: name.to_owned(),
            title: title.to_owned(),
            keywords,
            description: String::new(),
            steps: "step one".to_owned(),
        }
    }

    #[tokio::test]
    async fn save_list_and_scope_isolation() {
        let db = seed_db();
        let repo = PlaybookRepo::new(&db);
        repo.save(
            &scope("userA"),
            new_playbook("Morning Routine", "朝の準備", vec!["朝".to_owned()]),
        )
        .await
        .unwrap();

        let listed = repo.list(&scope("userA"), None).await.unwrap();
        assert_eq!(listed.len(), 1);
        // Node parity: name は正規化される（空白→_・小文字化）。
        assert_eq!(listed[0].name, "morning_routine");
        assert_eq!(listed[0].title, "朝の準備");
        assert_eq!(listed[0].keywords, vec!["朝".to_owned()]);
        assert_eq!(listed[0].steps, "step one");

        // 別ユーザーには見えない（分離キーを型で強制）。
        assert!(repo.list(&scope("userB"), None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn save_is_upsert() {
        let db = seed_db();
        let repo = PlaybookRepo::new(&db);
        repo.save(&scope("u"), new_playbook("pb", "v1", vec![]))
            .await
            .unwrap();
        let updated = repo
            .save(
                &scope("u"),
                NewPlaybook {
                    name: "pb".to_owned(),
                    title: "v2".to_owned(),
                    keywords: vec!["k".to_owned()],
                    description: "d".to_owned(),
                    steps: "s2".to_owned(),
                },
            )
            .await
            .unwrap();
        assert_eq!(updated.title, "v2");
        // upsert なので重複せず1件。
        let listed = repo.list(&scope("u"), None).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].steps, "s2");
    }

    #[tokio::test]
    async fn get_and_delete() {
        let db = seed_db();
        let repo = PlaybookRepo::new(&db);
        repo.save(&scope("u"), new_playbook("pb", "t", vec![]))
            .await
            .unwrap();

        let got = repo
            .get(&scope("u"), "pb".to_owned())
            .await
            .unwrap()
            .expect("found");
        assert_eq!(got.title, "t");

        assert!(repo.delete(&scope("u"), "pb".to_owned()).await.unwrap());
        assert!(repo.list(&scope("u"), None).await.unwrap().is_empty());
        // 二重削除は false。
        assert!(!repo.delete(&scope("u"), "pb".to_owned()).await.unwrap());
    }

    #[tokio::test]
    async fn list_query_filters() {
        let db = seed_db();
        let repo = PlaybookRepo::new(&db);
        repo.save(&scope("u"), new_playbook("alpha", "First", vec![]))
            .await
            .unwrap();
        repo.save(&scope("u"), new_playbook("beta", "Second", vec![]))
            .await
            .unwrap();

        let hits = repo
            .list(&scope("u"), Some("alpha".to_owned()))
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "alpha");
        // 空 query は全件（Node の falsy 扱いと一致）。
        assert_eq!(
            repo.list(&scope("u"), Some(String::new())).await.unwrap().len(),
            2
        );
    }

    #[tokio::test]
    async fn delete_is_scope_isolated() {
        let db = seed_db();
        let repo = PlaybookRepo::new(&db);
        repo.save(&scope("userA"), new_playbook("shared", "t", vec![]))
            .await
            .unwrap();
        // 別ユーザーは同名を削除できない（クロススコープ削除を防ぐ）。
        assert!(!repo
            .delete(&scope("userB"), "shared".to_owned())
            .await
            .unwrap());
        assert_eq!(repo.list(&scope("userA"), None).await.unwrap().len(), 1);
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
                    .uri("/api/playbooks/save")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"hello","title":"Hi","keywords":["home"],"steps":"do it"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(save.status(), StatusCode::OK);

        let list = app
            .oneshot(
                Request::builder()
                    .uri("/api/playbooks")
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
        assert_eq!(j["playbooks"][0]["name"], serde_json::json!("hello"));
        assert_eq!(j["playbooks"][0]["keywords"][0], serde_json::json!("home"));
        // 内部列は露出しない（構造的フェイルクローズ）。
        assert!(j["playbooks"][0]["user_id"].is_null());
        assert!(j["playbooks"][0]["bot_id"].is_null());
        assert!(j["playbooks"][0]["id"].is_null());
        assert!(j["playbooks"][0]["created_at"].is_null());
    }

    #[tokio::test]
    async fn route_requires_auth() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/playbooks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
