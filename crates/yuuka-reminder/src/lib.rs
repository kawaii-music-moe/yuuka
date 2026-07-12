//! yuuka-reminder — reminder ドメイン（T1 fan-out）: Repo + wire DTO + route を縦に持つ。
//!
//! **参照実装は yuuka-todo**。凍結契約（core/types）は変更しない。
//! DAG: `reminder → web, db, types, core`。ルータは supervisor が共通レイヤ配下にマージする。
//!
//! Phase 1 スコープ = コア CRUD（list/add/cancel/delete）。cron 用の全件走査
//! （listDuePending / markSent / rescheduleRepeat）は Phase 4 で [`cron`] に追加済み。deferred（後回し）:
//! - repeat_rule の cron 式**厳密**検証・過去日時の次回時刻補正（cron 依存・cron_util は上位 crate）
//! - 既定送信先解決（users.notify_target_*）・source/source_id の各機能連携
//!
//! trigger_at の DB 形式正規化（`T`/空白/日付のみ）は [`datetime::to_db_datetime`] で実装済み（B4 修正）。
//!
//! cancel 失敗の **404（不在）/ 409（実在するが pending でない）** 区別は M-12 で実装済み。

pub mod cron;
pub mod datetime;
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
    <dto::Reminder as TS>::export_all(&cfg)?;
    <dto::NewReminder as TS>::export_all(&cfg)?;
    <dto::ReminderListData as TS>::export_all(&cfg)?;
    <dto::ReminderData as TS>::export_all(&cfg)?;
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

    use crate::dto::NewReminder;
    use crate::repo::ReminderRepo;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    // Node migrations.ts の reminders 表 + v3 で後付けされる bot_id 列（DEFAULT 'system_default'）。
    const REMINDERS_DDL: &str = "CREATE TABLE reminders (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        message TEXT NOT NULL,
        trigger_at TEXT NOT NULL,
        repeat_rule TEXT,
        target_type TEXT NOT NULL DEFAULT 'dm',
        target_id TEXT,
        status TEXT NOT NULL DEFAULT 'pending',
        source TEXT NOT NULL DEFAULT 'manual',
        source_id TEXT,
        created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
    );";

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_reminder_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        {
            let conn = rusqlite::Connection::open(&path).expect("seed db");
            conn.execute_batch(REMINDERS_DDL).expect("create reminders");
        }
        Db::open(&path).expect("open db")
    }

    fn scope(user: &str) -> UserScope {
        UserScope::new(UserId::new(user), BotId::system_default())
    }

    fn new_reminder(message: &str) -> NewReminder {
        NewReminder {
            message: message.to_owned(),
            trigger_at: "2999-01-01 09:00:00".to_owned(),
            repeat_rule: None,
            target_type: None,
            target_id: None,
        }
    }

    #[tokio::test]
    async fn add_list_and_scope_isolation() {
        let db = seed_db();
        let repo = ReminderRepo::new(&db);
        let created = repo
            .add(&scope("userA"), new_reminder("drink water"))
            .await
            .unwrap();
        assert_eq!(created.message, "drink water");
        assert_eq!(created.status, "pending");
        assert_eq!(created.target_type, "dm");
        assert_eq!(created.source, "manual");

        let listed = repo.list(&scope("userA"), false).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].message, "drink water");

        // 別ユーザーには見えない（分離キーを型で強制）。
        assert!(repo.list(&scope("userB"), false).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn cancel_hides_from_pending_but_shows_in_all() {
        let db = seed_db();
        let repo = ReminderRepo::new(&db);
        let created = repo.add(&scope("u"), new_reminder("t")).await.unwrap();

        let cancelled = repo
            .cancel(&scope("u"), created.id)
            .await
            .unwrap()
            .expect("cancelled row");
        assert_eq!(cancelled.status, "cancelled");

        // pending のみの一覧からは消える。
        assert!(repo.list(&scope("u"), false).await.unwrap().is_empty());
        // include_all では残る。
        let all = repo.list(&scope("u"), true).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].status, "cancelled");

        // 既に pending でないので二重キャンセルは None。
        assert!(repo.cancel(&scope("u"), created.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_and_cross_scope_isolation() {
        let db = seed_db();
        let repo = ReminderRepo::new(&db);
        let created = repo
            .add(&scope("userA"), new_reminder("secret"))
            .await
            .unwrap();

        // 別ユーザーは削除できない（スコープ外）。
        assert!(!repo.delete(&scope("userB"), created.id).await.unwrap());
        // 別ユーザーからは取得もできない。
        assert!(repo
            .get(&scope("userB"), created.id)
            .await
            .unwrap()
            .is_none());

        // 本人は削除できる。
        assert!(repo.delete(&scope("userA"), created.id).await.unwrap());
        assert!(repo.list(&scope("userA"), true).await.unwrap().is_empty());
        // 二重削除は false。
        assert!(!repo.delete(&scope("userA"), created.id).await.unwrap());
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
                    .uri("/api/reminders/add")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"message":"hello","trigger_at":"2999-01-01 09:00:00"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(add.status(), StatusCode::OK);

        let list = app
            .oneshot(
                Request::builder()
                    .uri("/api/reminders")
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
        assert_eq!(j["reminders"][0]["message"], serde_json::json!("hello"));
        assert_eq!(j["reminders"][0]["status"], serde_json::json!("pending"));
        // 内部列は露出しない（構造的フェイルクローズ）。
        assert!(j["reminders"][0]["user_id"].is_null());
        assert!(j["reminders"][0]["bot_id"].is_null());
    }

    #[tokio::test]
    async fn route_requires_auth() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/reminders")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// wire 契約ロック（**非対称の例外**）: reminder の入力は例外的に **snake_case**。
    /// Node は `ctx.body.trigger_at` / `ctx.body.repeat_rule` を読み、フロントも snake_case を送る。
    /// `NewReminder` に `#[serde(rename_all = "camelCase")]` を付けると契約が壊れるため、誤って
    /// 付けた場合にこのテストが落ちて検知する（H-1 の一律 camelCase 化に対する回帰ガード）。
    #[test]
    fn new_reminder_wire_contract_is_snake_case() {
        let snake: NewReminder = serde_json::from_str(
            r#"{"message":"m","trigger_at":"2026-07-06 12:00:00","repeat_rule":"0 9 * * 1","target_type":"dm","target_id":"123"}"#,
        )
        .unwrap();
        assert_eq!(snake.trigger_at, "2026-07-06 12:00:00");
        assert_eq!(snake.repeat_rule.as_deref(), Some("0 9 * * 1"));
        assert_eq!(snake.target_type.as_deref(), Some("dm"));
        assert_eq!(snake.target_id.as_deref(), Some("123"));

        // camelCase を付けてしまうと snake_case の任意フィールドが拾われなくなる。
        // 現契約（snake_case）では camelCase キーは無視される。
        let camel: NewReminder =
            serde_json::from_str(r#"{"message":"m","trigger_at":"t","repeatRule":"x"}"#).unwrap();
        assert_eq!(camel.repeat_rule, None);
    }

    /// M-12 golden: `POST /api/reminders/cancel` は Node パリティで失敗理由を区別する。
    /// 不在 → **404**、実在するが pending でない（キャンセル済み）→ **409**。
    #[tokio::test]
    async fn route_cancel_missing_is_404_and_nonpending_is_409() {
        let app = app();

        // 不在 ID → 404。
        let missing = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/reminders/cancel")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"reminder_id":999999}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);

        // 追加 → id を取得。
        let add = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/reminders/add")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"message":"m","trigger_at":"2999-01-01 09:00:00"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(add.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let id = j["reminder"]["id"].as_i64().expect("id");

        // 1 回目キャンセル → 200 {success:true, reminder(status=cancelled)}。
        let c1 = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/reminders/cancel")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"reminder_id":{id}}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(c1.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(c1.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["success"], serde_json::json!(true));
        assert_eq!(j["reminder"]["status"], serde_json::json!("cancelled"));

        // 2 回目（実在するが pending でない）→ 409。
        let c2 = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/reminders/cancel")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"reminder_id":{id}}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(c2.status(), StatusCode::CONFLICT);
    }
}
