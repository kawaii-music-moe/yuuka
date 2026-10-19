//! yuuka-finance — finance ドメイン（T1 fan-out・参照実装は yuuka-todo）: Repo + wire DTO + route。
//!
//! DAG: `finance → web, db, types, core`。ルータは supervisor が共通レイヤ配下にマージする。
//! 参照スコープ = コア CRUD（expense の list/add/get/delete）・予算上限 (budget_limits)・
//! 支払い予定 (planned_payments)・月次集計 (total/incomeTotal/breakdown/trend)・receipt OCR
//! (upload-receipt)。receipt は [`ReceiptParser`] シーム越しで、既定は [`NullReceiptParser`]（503）、
//! supervisor が実 `ChatEngine`（Gemini vision 秘書ターン）へ配線する（A1・`routes_with`）。

pub mod dto;
pub mod receipt;
pub mod repo;
pub mod routes;
pub mod tools;

pub use receipt::{NullReceiptParser, ReceiptError, ReceiptParser};
pub use routes::{routes, routes_with};
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
    <dto::BudgetLimit as TS>::export_all(&cfg)?;
    <dto::BudgetLimitListData as TS>::export_all(&cfg)?;
    <dto::SetBudgetLimit as TS>::export_all(&cfg)?;
    <dto::DeleteBudgetLimit as TS>::export_all(&cfg)?;
    <dto::PlannedPayment as TS>::export_all(&cfg)?;
    <dto::PlannedPaymentListData as TS>::export_all(&cfg)?;
    <dto::PlannedPaymentData as TS>::export_all(&cfg)?;
    <dto::NewPlannedPayment as TS>::export_all(&cfg)?;
    <dto::PlannedPaymentId as TS>::export_all(&cfg)?;
    <dto::SettlePlanData as TS>::export_all(&cfg)?;
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
    use yuuka_web::test_support::{with_bot_query, TEST_BOT_ID};
    use yuuka_web::{AppState, AuthBackend, Db, WebConfig};

    use crate::dto::NewExpense;
    use crate::repo::ExpenseRepo;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    // 実効スキーマは `Db::open`（= WriterHandle::spawn 内 run_migrations）が
    // V17__baseline.sql を丸ごと適用して作る（expenses/budget_limits/planned_payments/todos
    // すべて含む）。テスト側で部分 DDL を手書きすると後続 INDEX（例 idx_todos_parent）と
    // 衝突するため、空ファイルからマイグレーションに一任する。
    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_finance_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        // 前回の残骸を除去して毎回まっさらなマイグレーション適用にする。
        let _ = std::fs::remove_file(&path);
        {
            // ReadPool は READ_ONLY で開くため、writer がマイグレーションを流す前に
            // 空の SQLite ファイルを実体として作っておく（DDL は書かず migration に一任）。
            let conn = rusqlite::Connection::open(&path).expect("create empty db file");
            drop(conn);
        }
        let db = Db::open(&path).expect("open db");
        // 実マイグレーションは planned_payments/expenses/todos に users への FK を張るため、
        // テスト対象ユーザーを先に作る（本番は Node が users を作成済みの前提）。
        {
            let conn = rusqlite::Connection::open(&path).expect("seed users conn");
            for uid in ["u", "userA", "userB", "other"] {
                conn.execute(
                    "INSERT OR IGNORE INTO users (discord_id, username, salt) \
                     VALUES (?1, ?1, 'x')",
                    rusqlite::params![uid],
                )
                .expect("seed user");
            }
        }
        db
    }

    fn scope(user: &str) -> UserScope {
        UserScope::new(UserId::new(user), BotId::new(TEST_BOT_ID))
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
        let created = repo
            .add(&scope("u"), new_expense(500, "娯楽"))
            .await
            .unwrap();

        let got = repo
            .get(&scope("u"), created.id)
            .await
            .unwrap()
            .expect("row");
        assert_eq!(got.id, created.id);

        // 別スコープからは取得も削除もできない。
        assert!(repo
            .get(&scope("other"), created.id)
            .await
            .unwrap()
            .is_none());
        assert!(!repo.delete(&scope("other"), created.id).await.unwrap());

        assert!(repo.delete(&scope("u"), created.id).await.unwrap());
        assert!(repo.list(&scope("u")).await.unwrap().is_empty());
        // 二重削除は false。
        assert!(!repo.delete(&scope("u"), created.id).await.unwrap());
    }

    /// 明示日付を与えて `date` を指定月に固定した収支を作る（集計テスト用）。
    fn dated_expense(amount: i64, category: &str, date: &str, etype: &str) -> NewExpense {
        NewExpense {
            amount,
            category: category.to_owned(),
            description: None,
            date: Some(date.to_owned()),
            time: Some("12:00:00".to_owned()),
            r#type: Some(etype.to_owned()),
        }
    }

    #[tokio::test]
    async fn monthly_total_breakdown_and_trend() {
        let db = seed_db();
        let repo = ExpenseRepo::new(&db);
        let s = scope("u");
        // 2026-03: 食費 1000 + 食費 500 + 交通費 200（支出）、給与 40000（収入）。
        repo.add(&s, dated_expense(1000, "食費", "2026-03-05", "expense"))
            .await
            .unwrap();
        repo.add(&s, dated_expense(500, "食費", "2026-03-20", "expense"))
            .await
            .unwrap();
        repo.add(&s, dated_expense(200, "交通費", "2026-03-21", "expense"))
            .await
            .unwrap();
        repo.add(&s, dated_expense(40_000, "給与", "2026-03-25", "income"))
            .await
            .unwrap();
        // 別月 2026-02: 食費 999（集計対象外の確認用）。
        repo.add(&s, dated_expense(999, "食費", "2026-02-10", "expense"))
            .await
            .unwrap();

        // 月次合計（支出/収入）は指定月のみ集計。
        assert_eq!(
            repo.monthly_total(&s, "expense", 2026, 3).await.unwrap(),
            1700
        );
        assert_eq!(
            repo.monthly_total(&s, "income", 2026, 3).await.unwrap(),
            40_000
        );
        // 空月は 0（COALESCE）。
        assert_eq!(repo.monthly_total(&s, "expense", 2026, 4).await.unwrap(), 0);

        // カテゴリ別内訳は金額降順・件数付き（食費 1500/2件 → 交通費 200/1件）。
        let bd = repo
            .monthly_category_breakdown(&s, 2026, 3, "expense")
            .await
            .unwrap();
        assert_eq!(bd.len(), 2);
        assert_eq!(bd[0].category, "食費");
        assert_eq!(bd[0].total, 1500);
        assert_eq!(bd[0].count, 2);
        assert_eq!(bd[1].category, "交通費");
        assert_eq!(bd[1].total, 200);

        // trend は常に指定件数（古い順）で、記録の無い月は 0 埋め。
        let trend = repo.monthly_trend(&s, 6).await.unwrap();
        assert_eq!(trend.len(), 6);
        // 別ユーザーには金銭データが漏れない（スコープ分離）。
        assert_eq!(
            repo.monthly_total(&scope("userB"), "expense", 2026, 3)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn list_by_month_honors_month_filter_and_scope() {
        // issue #47: Node の GET /api/client/finance/transactions は month を無視し常に直近
        // 100 件を返すバグだった。Rust 版の list_by_month は月で絞り込む。
        let db = seed_db();
        let repo = ExpenseRepo::new(&db);
        let s = scope("u");
        repo.add(&s, dated_expense(1000, "食費", "2026-03-05", "expense"))
            .await
            .unwrap();
        repo.add(&s, dated_expense(500, "食費", "2026-03-20", "expense"))
            .await
            .unwrap();
        repo.add(&s, dated_expense(999, "食費", "2026-02-10", "expense"))
            .await
            .unwrap();

        let march = repo.list_by_month(&s, 2026, 3).await.unwrap();
        assert_eq!(march.len(), 2, "2026-02 の記録は含まれない");
        assert!(march.iter().all(|e| e.date.starts_with("2026-03")));
        // 新しい順（date DESC）。
        assert_eq!(march[0].date, "2026-03-20");

        // 記録の無い月は空。
        assert!(repo.list_by_month(&s, 2026, 4).await.unwrap().is_empty());

        // 別ユーザーには漏れない。
        assert!(repo
            .list_by_month(&scope("userB"), 2026, 3)
            .await
            .unwrap()
            .is_empty());
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

    async fn app() -> axum::Router {
        let auth = Arc::new(FakeAuth {
            user: SessionUser {
                discord_id: "u".to_owned(),
                username: "u".to_owned(),
                role: Role::User,
            },
        });
        let db = seed_db();
        yuuka_web::test_support::seed_bot(&db, TEST_BOT_ID, "u").await;
        let state = AppState::new(auth, WebConfig::default(), db);
        with_bot_query(super::routes().with_state(state), TEST_BOT_ID)
    }

    #[tokio::test]
    async fn route_add_then_list() {
        let app = app().await;
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
        let bytes = axum::body::to_bytes(list.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["success"], serde_json::json!(true));
        assert_eq!(j["expenses"][0]["amount"], serde_json::json!(1500));
        assert_eq!(j["expenses"][0]["category"], serde_json::json!("食費"));
        assert_eq!(j["expenses"][0]["memo"], serde_json::json!("lunch"));
        // 内部列は露出しない（金銭データの構造的フェイルクローズ）。
        assert!(j["expenses"][0]["user_id"].is_null());
        assert!(j["expenses"][0]["bot_id"].is_null());
        // 月次集計フィールドが同梱される（silent 縮退の解消・当月の支出 1500）。
        assert_eq!(j["total"], serde_json::json!(1500));
        assert_eq!(j["incomeTotal"], serde_json::json!(0));
        assert_eq!(j["breakdown"][0]["category"], serde_json::json!("食費"));
        assert_eq!(j["breakdown"][0]["total"], serde_json::json!(1500));
        assert_eq!(j["breakdown"][0]["count"], serde_json::json!(1));
        // trend は 6 件・最新（末尾）は当月で支出 1500。
        assert_eq!(j["trend"].as_array().unwrap().len(), 6);
        assert_eq!(j["trend"][5]["expense"], serde_json::json!(1500));
    }

    #[tokio::test]
    async fn route_add_rejects_missing_fields() {
        let resp = app()
            .await
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
            .await
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

    // ── ヘルパ（消込テスト用の直挿し／検査）──────────────────────────────────

    /// 支払い予定を1件挿入し（user=u/test_bot）、任意で紐付き未完了 ToDo を作る。
    async fn insert_plan_with_todo(db: &Db, with_open_todo: bool) -> i64 {
        db.writer
            .transaction(move |tx| {
                tx.execute(
                    "INSERT INTO planned_payments \
                       (user_id, bot_id, title, amount, category, due_date) \
                     VALUES ('u', 'test_bot', '電気代', 5000, '光熱費', '2026-07-01')",
                    [],
                )
                .map_err(yuuka_db::map_sqlite)?;
                let plan_id = tx.last_insert_rowid();
                if with_open_todo {
                    tx.execute(
                        "INSERT INTO todos (user_id, bot_id, title, status, linked_payment_id) \
                         VALUES ('u', 'test_bot', '電気代を払う', 'open', ?1)",
                        rusqlite::params![plan_id],
                    )
                    .map_err(yuuka_db::map_sqlite)?;
                }
                Ok(plan_id)
            })
            .await
            .expect("seed plan")
    }

    /// 予定に対する紐付き ToDo の status を1件だけ読む（テスト検査用）。
    async fn todo_status(db: &Db, plan_id: i64) -> String {
        db.read
            .read(move |conn| {
                conn.query_row(
                    "SELECT status FROM todos WHERE linked_payment_id = ?1",
                    rusqlite::params![plan_id],
                    |r| r.get::<_, String>(0),
                )
                .map_err(yuuka_db::map_sqlite)
            })
            .await
            .expect("todo status")
    }

    /// 予定に期日前リマインドを紐付ける（`link_reminder` 相当の行を直接作る・テスト用）。
    /// `status` で送信済み等の状態も作れる。`bot_id` は別 Bot の行を作るときに変える。
    async fn insert_linked_reminder(db: &Db, plan_id: i64, bot_id: &str, status: &str) -> i64 {
        let bot_id = bot_id.to_owned();
        let status = status.to_owned();
        db.writer
            .transaction(move |tx| {
                tx.execute(
                    "INSERT INTO reminders \
                       (user_id, bot_id, message, trigger_at, status, source, source_id) \
                     VALUES ('u', ?1, '電気代の期日です', '2099-06-30 09:00:00', ?2, 'payment', ?3)",
                    rusqlite::params![bot_id, status, plan_id.to_string()],
                )
                .map_err(yuuka_db::map_sqlite)?;
                let reminder_id = tx.last_insert_rowid();
                tx.execute(
                    "UPDATE planned_payments SET linked_reminder_id = ?1 WHERE id = ?2",
                    rusqlite::params![reminder_id, plan_id],
                )
                .map_err(yuuka_db::map_sqlite)?;
                Ok(reminder_id)
            })
            .await
            .expect("seed reminder")
    }

    /// リマインドの status を読む（テスト検査用）。
    async fn reminder_status(db: &Db, reminder_id: i64) -> String {
        db.read
            .read(move |conn| {
                conn.query_row(
                    "SELECT status FROM reminders WHERE id = ?1",
                    rusqlite::params![reminder_id],
                    |r| r.get::<_, String>(0),
                )
                .map_err(yuuka_db::map_sqlite)
            })
            .await
            .expect("reminder status")
    }

    // ── 予算上限（budget_limits） ────────────────────────────────────────────

    #[tokio::test]
    async fn budget_limit_upsert_list_delete() {
        let db = seed_db();
        let repo = ExpenseRepo::new(&db);
        let s = scope("u");

        assert!(repo.list_budget_limits(&s).await.unwrap().is_empty());

        repo.upsert_budget_limit(&s, "食費".to_owned(), 50_000)
            .await
            .unwrap();
        repo.upsert_budget_limit(&s, "娯楽".to_owned(), 0)
            .await
            .unwrap();
        let limits = repo.list_budget_limits(&s).await.unwrap();
        assert_eq!(limits.len(), 2);
        // category 昇順。
        assert_eq!(limits[0].category, "娯楽");
        assert_eq!(limits[0].limit_amount, 0);
        assert_eq!(limits[1].category, "食費");
        assert_eq!(limits[1].limit_amount, 50_000);

        // 再設定は UPSERT（重複行を作らない）。
        repo.upsert_budget_limit(&s, "食費".to_owned(), 60_000)
            .await
            .unwrap();
        let limits = repo.list_budget_limits(&s).await.unwrap();
        assert_eq!(limits.len(), 2);
        assert_eq!(limits[1].limit_amount, 60_000);

        // 別スコープには見えない（金銭データの分離）。
        assert!(repo
            .list_budget_limits(&scope("other"))
            .await
            .unwrap()
            .is_empty());

        // 削除（2回目は false）。
        assert!(repo
            .delete_budget_limit(&s, "食費".to_owned())
            .await
            .unwrap());
        assert!(!repo
            .delete_budget_limit(&s, "食費".to_owned())
            .await
            .unwrap());
        assert_eq!(repo.list_budget_limits(&s).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn route_budget_limits_set_get_and_message() {
        let app = app().await;
        let set = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/expenses/budget-limits")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"category":"食費","limit_amount":50000}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(set.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(set.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["success"], serde_json::json!(true));
        assert_eq!(
            j["message"],
            serde_json::json!("食費 の予算上限を ¥50,000 に設定しました。")
        );

        let get = app
            .oneshot(
                Request::builder()
                    .uri("/api/expenses/budget-limits")
                    .header("cookie", "__Host-yuuka-session=good")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(get.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["limits"][0]["category"], serde_json::json!("食費"));
        assert_eq!(j["limits"][0]["limit_amount"], serde_json::json!(50000));
        assert!(j["limits"][0]["user_id"].is_null());
        assert!(j["limits"][0]["bot_id"].is_null());
    }

    #[tokio::test]
    async fn route_budget_limit_rejects_negative() {
        let resp = app()
            .await
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/expenses/budget-limits")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"category":"食費","limit_amount":-1}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── 支払い予定（planned_payments）・消込 ─────────────────────────────────

    #[tokio::test]
    async fn plan_add_list_and_cancel() {
        use crate::dto::NewPlannedPayment;

        let db = seed_db();
        let repo = ExpenseRepo::new(&db);
        let s = scope("u");

        let created = repo
            .add_plan(
                &s,
                NewPlannedPayment {
                    title: "家賃".to_owned(),
                    amount: 80_000,
                    category: "住居".to_owned(),
                    due_date: Some("2026-08-01".to_owned()),
                    planned_date: None,
                    description: Some("8月分".to_owned()),
                    repeat_rule: None,
                },
                "2026-08-01".to_owned(),
            )
            .await
            .unwrap();
        assert_eq!(created.status, "pending");
        assert_eq!(created.amount, 80_000);
        assert_eq!(created.memo.as_deref(), Some("8月分"));
        assert_eq!(created.due_date, "2026-08-01");

        // pending のみ既定で見える。
        assert_eq!(repo.list_plans(&s, false).await.unwrap().len(), 1);

        // cancel → pending から消え、include_paid で cancelled として見える。
        assert!(repo.cancel_plan(&s, created.id).await.unwrap().is_some());
        assert!(repo.list_plans(&s, false).await.unwrap().is_empty());
        let all = repo.list_plans(&s, true).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].status, "cancelled");
        // 二重キャンセル（pending でない）は None。
        assert!(repo.cancel_plan(&s, created.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn route_plan_add_falls_back_to_planned_date() {
        // 旧 UI 互換: dueDate 非存在時は plannedDate を期日として採用。
        let app = app().await;
        let add = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/expenses/plans/add")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"title":"サブスク","amount":980,"category":"通信費","plannedDate":"2026-09-10"}"#,
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
        assert_eq!(j["plan"]["due_date"], serde_json::json!("2026-09-10"));
        assert_eq!(j["plan"]["status"], serde_json::json!("pending"));
        assert!(j["plan"]["user_id"].is_null());
    }

    #[tokio::test]
    async fn route_plan_add_validates_repeat_rule_with_runtime_parser() {
        // R19: 5 フィールドでも実行時 parser が解釈できない式は保存しない（ツールと同じ判定）。
        let app = app().await;
        let post = |body: &'static str| {
            Request::builder()
                .method("POST")
                .uri("/api/expenses/plans/add")
                .header("cookie", "__Host-yuuka-session=good")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap()
        };
        let bad = app
            .clone()
            .oneshot(post(
                r#"{"title":"家賃","amount":80000,"category":"その他","dueDate":"2026-08-27","repeatRule":"0 0 32 * *"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);

        let ok = app
            .oneshot(post(
                r#"{"title":"家賃","amount":80000,"category":"その他","dueDate":"2026-08-27","repeatRule":"0 0 27 * *"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(ok.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["plan"]["repeat_rule"], serde_json::json!("0 0 27 * *"));
    }

    #[tokio::test]
    async fn route_plan_add_rejects_missing_due() {
        let resp = app()
            .await
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/expenses/plans/add")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"title":"x","amount":100,"category":"食費"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn settle_records_expense_and_completes_todo() {
        let db = seed_db();
        let plan_id = insert_plan_with_todo(&db, true).await;
        let repo = ExpenseRepo::new(&db);
        let s = scope("u");

        let plan = repo.get_plan(&s, plan_id).await.unwrap().expect("plan");
        let settled = repo.settle_plan(&s, &plan).await.unwrap();
        let expense_id = settled.expense_id;
        assert_eq!(settled.completed_todos, 1);
        assert_eq!(settled.cancelled_reminder_id, None); // 紐付きリマインド無し

        // (1) Expense（amount/category=予定・memo=title・expense/manual）。
        let expense = repo.get(&s, expense_id).await.unwrap().expect("expense");
        assert_eq!(expense.amount, 5000);
        assert_eq!(expense.category, "光熱費");
        assert_eq!(expense.memo.as_deref(), Some("電気代"));
        assert_eq!(expense.r#type, "expense");
        assert_eq!(expense.source, "manual");

        // (2) 予定は settled + settled_expense_id、pending から消える。
        let settled = repo.get_plan(&s, plan_id).await.unwrap().expect("plan");
        assert_eq!(settled.status, "settled");
        assert_eq!(settled.settled_expense_id, Some(expense_id));
        assert!(repo.list_plans(&s, false).await.unwrap().is_empty());

        // (3) 紐付き ToDo は done。
        assert_eq!(todo_status(&db, plan_id).await, "done");
    }

    /// R14: 記帳済みの支出で消込すると新しい支出を作らず、その支出を settled_expense_id に記録する。
    /// 他ユーザーの支出・収入・存在しない ID では何も変更しない。
    #[tokio::test]
    async fn settle_with_existing_expense_links_without_new_record() {
        let db = seed_db();
        let plan_id = insert_plan_with_todo(&db, true).await;
        let repo = ExpenseRepo::new(&db);
        let s = scope("u");

        let existing = repo.add(&s, new_expense(5000, "光熱費")).await.unwrap();
        let income = repo
            .add(
                &s,
                NewExpense {
                    r#type: Some("income".to_owned()),
                    ..new_expense(5000, "その他")
                },
            )
            .await
            .unwrap();
        let others = repo
            .add(&scope("other"), new_expense(5000, "光熱費"))
            .await
            .unwrap();

        // 条件を満たさない支出では None・予定は pending のまま・ToDo も未完了。
        for bad in [others.id, income.id, 99_999] {
            assert_eq!(
                repo.settle_plan_with_expense(&s, plan_id, bad)
                    .await
                    .unwrap(),
                None
            );
        }
        let plan = repo.get_plan(&s, plan_id).await.unwrap().expect("plan");
        assert_eq!(plan.status, "pending");
        assert_eq!(plan.settled_expense_id, None);
        assert_eq!(todo_status(&db, plan_id).await, "open");

        // 記帳済みの支出で消込: 紐付き ToDo を完了し、支出は増えない。
        let settled = repo
            .settle_plan_with_expense(&s, plan_id, existing.id)
            .await
            .unwrap()
            .expect("settled");
        assert_eq!(settled.expense_id, existing.id);
        assert_eq!(settled.completed_todos, 1);
        let settled = repo.get_plan(&s, plan_id).await.unwrap().expect("plan");
        assert_eq!(settled.status, "settled");
        assert_eq!(settled.settled_expense_id, Some(existing.id));
        assert_eq!(todo_status(&db, plan_id).await, "done");
        assert_eq!(repo.list(&s).await.unwrap().len(), 2); // existing + income のみ

        // 消込済みの予定はもう一度消込できない。
        assert_eq!(
            repo.settle_plan_with_expense(&s, plan_id, existing.id)
                .await
                .unwrap(),
            None
        );
    }

    /// R15: 消込（新規記帳・記帳済みの支出）とキャンセルで、紐付く pending の期日前リマインドを
    /// 止める。送信済みのリマインドと、紐付きの無いリマインドは変えない。
    #[tokio::test]
    async fn settle_and_cancel_stop_linked_reminder() {
        let db = seed_db();
        let repo = ExpenseRepo::new(&db);
        let s = scope("u");
        // 別の予定に紐付く pending のリマインド（巻き込まれないことの確認用）。
        let unrelated_plan = insert_plan_with_todo(&db, false).await;
        let unrelated = insert_linked_reminder(&db, unrelated_plan, TEST_BOT_ID, "pending").await;

        // 新規記帳で消込。
        let plan_id = insert_plan_with_todo(&db, false).await;
        let reminder = insert_linked_reminder(&db, plan_id, TEST_BOT_ID, "pending").await;
        let plan = repo.get_plan(&s, plan_id).await.unwrap().expect("plan");
        let settled = repo.settle_plan(&s, &plan).await.unwrap();
        assert_eq!(settled.cancelled_reminder_id, Some(reminder));
        assert_eq!(reminder_status(&db, reminder).await, "cancelled");

        // 記帳済みの支出で消込。
        let plan_id = insert_plan_with_todo(&db, false).await;
        let reminder = insert_linked_reminder(&db, plan_id, TEST_BOT_ID, "pending").await;
        let existing = repo.add(&s, new_expense(5000, "光熱費")).await.unwrap();
        let settled = repo
            .settle_plan_with_expense(&s, plan_id, existing.id)
            .await
            .unwrap()
            .expect("settled");
        assert_eq!(settled.cancelled_reminder_id, Some(reminder));
        assert_eq!(reminder_status(&db, reminder).await, "cancelled");

        // キャンセル。
        let plan_id = insert_plan_with_todo(&db, false).await;
        let reminder = insert_linked_reminder(&db, plan_id, TEST_BOT_ID, "pending").await;
        let cancelled = repo
            .cancel_plan(&s, plan_id)
            .await
            .unwrap()
            .expect("cancelled");
        assert_eq!(cancelled.cancelled_reminder_id, Some(reminder));
        assert_eq!(reminder_status(&db, reminder).await, "cancelled");

        // 送信済みのリマインドは sent のまま（止めたことにしない）。
        let plan_id = insert_plan_with_todo(&db, false).await;
        let reminder = insert_linked_reminder(&db, plan_id, TEST_BOT_ID, "sent").await;
        let cancelled = repo
            .cancel_plan(&s, plan_id)
            .await
            .unwrap()
            .expect("cancelled");
        assert_eq!(cancelled.cancelled_reminder_id, None);
        assert_eq!(reminder_status(&db, reminder).await, "sent");

        // 別 Bot のリマインド id が紐付いていても、スコープ外なので変えない。
        let plan_id = insert_plan_with_todo(&db, false).await;
        let reminder = insert_linked_reminder(&db, plan_id, "other_bot", "pending").await;
        let cancelled = repo
            .cancel_plan(&s, plan_id)
            .await
            .unwrap()
            .expect("cancelled");
        assert_eq!(cancelled.cancelled_reminder_id, None);
        assert_eq!(reminder_status(&db, reminder).await, "pending");

        assert_eq!(reminder_status(&db, unrelated).await, "pending");
    }

    /// R15: HTTP の消込（plans/pay）・キャンセル（plans/delete）でも紐付きリマインドを止める。
    #[tokio::test]
    async fn route_pay_and_delete_stop_linked_reminder() {
        let db = seed_db();
        let paid_plan = insert_plan_with_todo(&db, false).await;
        let paid_reminder = insert_linked_reminder(&db, paid_plan, TEST_BOT_ID, "pending").await;
        let cancelled_plan = insert_plan_with_todo(&db, false).await;
        let cancelled_reminder =
            insert_linked_reminder(&db, cancelled_plan, TEST_BOT_ID, "pending").await;
        let auth = Arc::new(FakeAuth {
            user: SessionUser {
                discord_id: "u".to_owned(),
                username: "u".to_owned(),
                role: Role::User,
            },
        });
        let state = AppState::new(auth, WebConfig::default(), db.clone());
        yuuka_web::test_support::seed_bot(&db, TEST_BOT_ID, "u").await;
        let app = with_bot_query(super::routes().with_state(state), TEST_BOT_ID);

        for (uri, plan_id) in [
            ("/api/expenses/plans/pay", paid_plan),
            ("/api/expenses/plans/delete", cancelled_plan),
        ] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(uri)
                        .header("cookie", "__Host-yuuka-session=good")
                        .header("content-type", "application/json")
                        .body(Body::from(format!(r#"{{"id":{plan_id}}}"#)))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{uri}");
        }
        assert_eq!(reminder_status(&db, paid_reminder).await, "cancelled");
        assert_eq!(reminder_status(&db, cancelled_reminder).await, "cancelled");
    }

    #[tokio::test]
    async fn route_pay_settles_and_reports_todo_count() {
        let db = seed_db();
        let plan_id = insert_plan_with_todo(&db, true).await;
        let auth = Arc::new(FakeAuth {
            user: SessionUser {
                discord_id: "u".to_owned(),
                username: "u".to_owned(),
                role: Role::User,
            },
        });
        yuuka_web::test_support::seed_bot(&db, TEST_BOT_ID, "u").await;
        let state = AppState::new(auth, WebConfig::default(), db);
        let app = with_bot_query(super::routes().with_state(state), TEST_BOT_ID);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/expenses/plans/pay")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"id":{plan_id}}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["success"], serde_json::json!(true));
        assert_eq!(j["completedTodos"], serde_json::json!(1));
        assert_eq!(j["expense"]["amount"], serde_json::json!(5000));
        assert_eq!(j["expense"]["memo"], serde_json::json!("電気代"));
        assert_eq!(
            j["message"],
            serde_json::json!("「電気代」の支払いを消込しました。（紐付くToDo 1件を自動完了）")
        );
    }

    #[tokio::test]
    async fn route_pay_404_and_400_paths() {
        // 存在しない id → 404。
        let resp = app()
            .await
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/expenses/plans/pay")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"id":9999}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // id=0（Node の `!id`）→ 400。
        let resp = app()
            .await
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/expenses/plans/pay")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"id":0}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn route_pay_non_pending_is_blocked() {
        // キャンセル済みの予定は 400（既に消込済/キャンセル済）。
        let db = seed_db();
        let plan_id = insert_plan_with_todo(&db, false).await;
        db.writer
            .transaction(move |tx| {
                tx.execute(
                    "UPDATE planned_payments SET status = 'cancelled' WHERE id = ?1",
                    rusqlite::params![plan_id],
                )
                .map_err(yuuka_db::map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();
        let auth = Arc::new(FakeAuth {
            user: SessionUser {
                discord_id: "u".to_owned(),
                username: "u".to_owned(),
                role: Role::User,
            },
        });
        yuuka_web::test_support::seed_bot(&db, TEST_BOT_ID, "u").await;
        let state = AppState::new(auth, WebConfig::default(), db);
        let app = with_bot_query(super::routes().with_state(state), TEST_BOT_ID);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/expenses/plans/pay")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"id":{plan_id}}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn route_plan_delete_bare_success() {
        let db = seed_db();
        let plan_id = insert_plan_with_todo(&db, false).await;
        let auth = Arc::new(FakeAuth {
            user: SessionUser {
                discord_id: "u".to_owned(),
                username: "u".to_owned(),
                role: Role::User,
            },
        });
        yuuka_web::test_support::seed_bot(&db, TEST_BOT_ID, "u").await;
        let state = AppState::new(auth, WebConfig::default(), db);
        let app = with_bot_query(super::routes().with_state(state), TEST_BOT_ID);

        // pending → true。
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/expenses/plans/delete")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"id":{plan_id}}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["success"], serde_json::json!(true));

        // 2回目（既に cancelled）→ false（Node の cancelPlannedPayment 成否 bool）。
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/expenses/plans/delete")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"id":{plan_id}}}"#)))
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
    }

    /// 呼ばれた bot_id を記録するだけのレシート解析（ルートの Bot 解決を確かめる）。
    struct RecordingParser(std::sync::Mutex<Vec<String>>);

    #[async_trait]
    impl crate::ReceiptParser for RecordingParser {
        async fn parse_receipt(
            &self,
            bot_id: &str,
            _user_id: &str,
            _image_base64: &str,
            _mime_type: &str,
            _additional_text: Option<&str>,
        ) -> Result<serde_json::Value, crate::ReceiptError> {
            self.0.lock().unwrap().push(bot_id.to_owned());
            Ok(serde_json::json!({ "ok": true }))
        }
    }

    /// issue #141: レシート読み取りは Bot 未指定・システム Bot を 403、アクセス不可を 404 にし、
    /// システム Bot として解析しない。
    #[tokio::test]
    async fn route_upload_receipt_requires_own_bot() {
        let auth = Arc::new(FakeAuth {
            user: SessionUser {
                discord_id: "u".to_owned(),
                username: "u".to_owned(),
                role: Role::User,
            },
        });
        let db = seed_db();
        yuuka_web::test_support::seed_bot(&db, TEST_BOT_ID, "u").await;
        yuuka_web::test_support::seed_bot(&db, "foreign", "someone").await;
        let parser = Arc::new(RecordingParser(std::sync::Mutex::new(Vec::new())));
        let app = super::routes_with(parser.clone()).with_state(AppState::new(
            auth,
            WebConfig::default(),
            db,
        ));

        let post = |bot: Option<&str>| {
            let mut body =
                serde_json::json!({ "imageBase64": "aGVsbG8=", "mimeType": "image/png" });
            if let Some(b) = bot {
                body["botId"] = serde_json::json!(b);
            }
            Request::builder()
                .method("POST")
                .uri("/api/expenses/upload-receipt")
                .header("cookie", "__Host-yuuka-session=good")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        for (bot, want) in [
            (None, StatusCode::FORBIDDEN),
            (Some("system_default"), StatusCode::FORBIDDEN),
            (Some("foreign"), StatusCode::NOT_FOUND),
            (Some(TEST_BOT_ID), StatusCode::OK),
        ] {
            let resp = app.clone().oneshot(post(bot)).await.unwrap();
            assert_eq!(resp.status(), want, "{bot:?}");
        }
        assert_eq!(*parser.0.lock().unwrap(), vec![TEST_BOT_ID.to_owned()]);
    }
}
