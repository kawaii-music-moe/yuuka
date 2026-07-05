//! yuuka-persona — persona ドメイン（T1 fan-out）: Repo + wire DTO + route を縦に持つ。
//!
//! **参照実装は yuuka-todo**。persona は `owner_id`（ユーザー単位）でスコープされ、
//! bot 単位ではない（適用中ペルソナ `bot_active_personas` は別スコープ）。凍結契約
//! （core/types）は変更しない。ルータは supervisor が共通レイヤ配下にマージする。
//!
//! Phase 1 参照スコープ = コア CRUD（list/save=create+update/delete）。
//! activate/publish/marketplace/import/recommended-persona/admin unpublish+delete は
//! `bot_active_personas`・`bots`・`audit_logs` 連携が必要なため **deferred**。

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
    <dto::Persona as TS>::export_all(&cfg)?;
    <dto::SavePersona as TS>::export_all(&cfg)?;
    <dto::PersonaListData as TS>::export_all(&cfg)?;
    <dto::PersonaData as TS>::export_all(&cfg)?;
    <dto::PersonaDeletedData as TS>::export_all(&cfg)?;
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

    use crate::dto::SavePersona;
    use crate::repo::PersonaRepo;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    // Node migrations の personas テーブル定義（owner_id スコープ・bot_id なし）。
    const PERSONAS_DDL: &str = "CREATE TABLE personas (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        owner_id TEXT NOT NULL,
        name TEXT NOT NULL,
        prompt TEXT NOT NULL DEFAULT '',
        is_public INTEGER NOT NULL DEFAULT 0,
        created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
    );";

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir()
            .join(format!("yuuka_persona_test_{}_{seq}.sqlite", std::process::id()));
        {
            let conn = rusqlite::Connection::open(&path).expect("seed db");
            conn.execute_batch(PERSONAS_DDL).expect("create personas");
        }
        Db::open(&path).expect("open db")
    }

    fn scope(user: &str) -> UserScope {
        UserScope::new(UserId::new(user), BotId::system_default())
    }

    fn new_persona(name: &str, prompt: &str) -> SavePersona {
        SavePersona {
            id: None,
            name: name.to_owned(),
            prompt: prompt.to_owned(),
        }
    }

    #[tokio::test]
    async fn add_list_and_scope_isolation() {
        let db = seed_db();
        let repo = PersonaRepo::new(&db);
        let created = repo
            .add(&scope("userA"), new_persona("  ゆうか  ", "prompt-body"))
            .await
            .unwrap();
        // name は trim される（Node parity）。
        assert_eq!(created.name, "ゆうか");
        assert_eq!(created.prompt, "prompt-body");
        assert!(!created.is_public);

        let listed = repo.list(&scope("userA")).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "ゆうか");

        // 別ユーザーには見えない（owner_id で分離）。
        assert!(repo.list(&scope("userB")).await.unwrap().is_empty());
        // 別ユーザーからは get もできない。
        assert!(repo.get(&scope("userB"), created.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn update_scoped_and_cross_user_denied() {
        let db = seed_db();
        let repo = PersonaRepo::new(&db);
        let created = repo
            .add(&scope("owner"), new_persona("orig", "p0"))
            .await
            .unwrap();

        let updated = repo
            .update(
                &scope("owner"),
                created.id,
                new_persona("renamed", "p1"),
            )
            .await
            .unwrap()
            .expect("updated row");
        assert_eq!(updated.name, "renamed");
        assert_eq!(updated.prompt, "p1");

        // 他ユーザーは更新できない（None＝404 相当）。値も変わらない。
        let cross = repo
            .update(&scope("intruder"), created.id, new_persona("hax", "evil"))
            .await
            .unwrap();
        assert!(cross.is_none());
        let after = repo.get(&scope("owner"), created.id).await.unwrap().unwrap();
        assert_eq!(after.name, "renamed");
    }

    #[tokio::test]
    async fn delete_scoped() {
        let db = seed_db();
        let repo = PersonaRepo::new(&db);
        let created = repo.add(&scope("u"), new_persona("t", "x")).await.unwrap();

        // 他ユーザーは削除できない。
        assert!(!repo.delete(&scope("other"), created.id).await.unwrap());
        assert!(repo.delete(&scope("u"), created.id).await.unwrap());
        assert!(repo.list(&scope("u")).await.unwrap().is_empty());
        // 二重削除は false。
        assert!(!repo.delete(&scope("u"), created.id).await.unwrap());
    }

    #[tokio::test]
    async fn empty_name_rejected() {
        let db = seed_db();
        let repo = PersonaRepo::new(&db);
        assert!(repo.add(&scope("u"), new_persona("   ", "x")).await.is_err());
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
                    .uri("/api/personas/save")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"hello","prompt":"be nice"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(save.status(), StatusCode::OK);

        let list = app
            .oneshot(
                Request::builder()
                    .uri("/api/personas")
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
        assert_eq!(j["personas"][0]["name"], serde_json::json!("hello"));
        assert_eq!(j["personas"][0]["prompt"], serde_json::json!("be nice"));
        assert_eq!(j["personas"][0]["is_public"], serde_json::json!(false));
        assert_eq!(j["max_length"], serde_json::json!(20000));
        // 内部列 owner_id は露出しない（構造的フェイルクローズ）。
        assert!(j["personas"][0]["owner_id"].is_null());
    }

    #[tokio::test]
    async fn route_requires_auth() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/personas")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
