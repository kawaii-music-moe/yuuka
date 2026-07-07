//! yuuka-finance — finance ドメイン（T1 fan-out・参照実装は yuuka-todo）: Repo + wire DTO + route。
//!
//! DAG: `finance → web, db, types, core`。ルータは supervisor が共通レイヤ配下にマージする。
//! Phase 1 参照スコープ = コア CRUD（expense の list/add/get/delete）。receipt OCR・予算上限
//! (budget_limits)・支払い予定 (planned_payments)・月次集計 (total/breakdown/trend) は deferred。

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
    <dto::Expense as TS>::export_all(&cfg)?;
    <dto::NewExpense as TS>::export_all(&cfg)?;
    <dto::ExpenseListData as TS>::export_all(&cfg)?;
    <dto::ExpenseData as TS>::export_all(&cfg)?;
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

    use crate::dto::NewExpense;
    use crate::repo::ExpenseRepo;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    // 実効スキーマ = migrations.ts の CREATE TABLE expenses + migrateToBotScopedData の
    // ALTER TABLE ADD COLUMN bot_id（新規 DB では bot_id 付き）を反映。
    const EXPENSES_DDL: &str = "CREATE TABLE expenses (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        type TEXT NOT NULL DEFAULT 'expense',
        amount INTEGER NOT NULL,
        category TEXT NOT NULL,
        memo TEXT,
        date TEXT NOT NULL,
        time TEXT,
        source TEXT NOT NULL DEFAULT 'manual',
        created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
    );";

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir()
            .join(format!("yuuka_finance_test_{}_{seq}.sqlite", std::process::id()));
        {
            let conn = rusqlite::Connection::open(&path).expect("seed db");
            conn.execute_batch(EXPENSES_DDL).expect("create expenses");
        }
        Db::open(&path).expect("open db")
    }

    fn scope(user: &str) -> UserScope {
        UserScope::new(UserId::new(user), BotId::system_default())
    }

    fn new_expense(amount: i64, category: &str) -> NewExpense {
        NewExpense {
            amount,
            category: category.to_owned(),
            description: None,
            date: None,
            time: None,
            r#type: None,
        }
    }

    #[tokio::test]
    async fn add_list_and_scope_isolation() {
        let db = seed_db();
        let repo = ExpenseRepo::new(&db);
        repo.add(&scope("userA"), new_expense(1200, "食費"))
            .await
            .unwrap();

        let listed = repo.list(&scope("userA")).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].amount, 1200);
        assert_eq!(listed[0].category, "食費");
        // 既定は expense / manual、date/time は localtime 補完で非空。
        assert_eq!(listed[0].r#type, "expense");
        assert_eq!(listed[0].source, "manual");
        assert!(!listed[0].date.is_empty());
        assert!(listed[0].time.is_some());

        // 別ユーザーには見えない（分離キーを型で強制）。金銭データのスコープ厳守。
        assert!(repo.list(&scope("userB")).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn income_type_and_explicit_fields() {
        let db = seed_db();
        let repo = ExpenseRepo::new(&db);
        let created = repo
            .add(
                &scope("u"),
                NewExpense {
                    amount: 300_000,
                    category: "給与".to_owned(),
                    description: Some("memo-text".to_owned()),
                    date: Some("2026-01-15".to_owned()),
                    time: Some("09:30:00".to_owned()),
                    r#type: Some("income".to_owned()),
                },
            )
            .await
            .unwrap();
        assert_eq!(created.r#type, "income");
        assert_eq!(created.memo.as_deref(), Some("memo-text"));
        assert_eq!(created.date, "2026-01-15");
        assert_eq!(created.time.as_deref(), Some("09:30:00"));

        // unknown type は expense に正規化（Node financeRoutes parity）。
        let e = repo
            .add(
                &scope("u"),
                NewExpense {
                    amount: 10,
                    category: "x".to_owned(),
                    description: None,
                    date: None,
                    time: None,
                    r#type: Some("bogus".to_owned()),
                },
            )
            .await
            .unwrap();
        assert_eq!(e.r#type, "expense");
    }

    #[tokio::test]
    async fn get_and_delete() {
        let db = seed_db();
        let repo = ExpenseRepo::new(&db);
        let created = repo.add(&scope("u"), new_expense(500, "娯楽")).await.unwrap();

        let got = repo
            .get(&scope("u"), created.id)
            .await
            .unwrap()
            .expect("row");
        assert_eq!(got.id, created.id);

        // 別スコープからは取得も削除もできない。
        assert!(repo.get(&scope("other"), created.id).await.unwrap().is_none());
        assert!(!repo.delete(&scope("other"), created.id).await.unwrap());

        assert!(repo.delete(&scope("u"), created.id).await.unwrap());
        assert!(repo.list(&scope("u")).await.unwrap().is_empty());
        // 二重削除は false。
        assert!(!repo.delete(&scope("u"), created.id).await.unwrap());
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
                    .uri("/api/expenses/add")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"amount":1500,"category":"食費","description":"lunch"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(add.status(), StatusCode::OK);

        let list = app
            .oneshot(
                Request::builder()
                    .uri("/api/expenses")
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
        assert_eq!(j["expenses"][0]["amount"], serde_json::json!(1500));
        assert_eq!(j["expenses"][0]["category"], serde_json::json!("食費"));
        assert_eq!(j["expenses"][0]["memo"], serde_json::json!("lunch"));
        // 内部列は露出しない（金銭データの構造的フェイルクローズ）。
        assert!(j["expenses"][0]["user_id"].is_null());
        assert!(j["expenses"][0]["bot_id"].is_null());
    }

    #[tokio::test]
    async fn route_add_rejects_missing_fields() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/expenses/add")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"amount":0,"category":"食費"}"#))
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
                    .uri("/api/expenses")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
