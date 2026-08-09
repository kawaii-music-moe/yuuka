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
        let path = std::env::temp_dir().join(format!(
            "yuuka_persona_test_{}_{seq}.sqlite",
            std::process::id()
        ));
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

    // ─── 共有 bot の owner-canonical 同期（A が B と bot を共有した場合） ───────────────

    /// 共有 bot スコープ（発話ユーザー `user`・bot `bot`・オーナー `owner`）＝`resolve_scope` が
    /// 実 bot を解決したときの `UserScope`。`config_owner_id` はオーナー。
    fn scope_shared(user: &str, bot: &str, owner: &str) -> UserScope {
        UserScope::with_owner(UserId::new(user), BotId::new(bot), UserId::new(owner))
    }

    const ACTIVE_DDL: &str = "CREATE TABLE bot_active_personas (
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        persona_id INTEGER NOT NULL,
        updated_at TEXT NOT NULL DEFAULT (datetime('now','localtime')),
        PRIMARY KEY (user_id, bot_id)
    );";

    fn seed_db_full() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_persona_sync_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        {
            let conn = rusqlite::Connection::open(&path).expect("seed db");
            conn.execute_batch(PERSONAS_DDL).expect("create personas");
            conn.execute_batch(ACTIVE_DDL).expect("create bot_active_personas");
        }
        Db::open(&path).expect("open db")
    }

    async fn count_active_rows(db: &Db, bot: &str) -> i64 {
        let bot = bot.to_owned();
        db.read
            .read(move |conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM bot_active_personas WHERE bot_id = ?1",
                    rusqlite::params![bot],
                    |r| r.get::<_, i64>(0),
                )
                .map_err(yuuka_db::map_sqlite)
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn shared_bot_personas_are_owner_canonical_and_collaborative() {
        let db = seed_db_full();
        let repo = PersonaRepo::new(&db);
        // オーナー A が自分の bot(botX) にペルソナ作成（owner-canonical キー = A）。
        let owner_scope = scope_shared("A", "botX", "A");
        let p = repo
            .add(&owner_scope, new_persona("shared-p", "body"))
            .await
            .unwrap();

        // 共有相手 B（同じ botX・オーナー A）は A のペルソナを閲覧できる（＝同期）。
        let b_scope = scope_shared("B", "botX", "A");
        let listed = repo.list(&b_scope).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, p.id);
        assert!(repo.get(&b_scope, p.id).await.unwrap().is_some());

        // B は共同編集できる（オーナー名前空間へ書き込み）→ A から見ても反映される。
        let updated = repo
            .update(&b_scope, p.id, new_persona("edited-by-B", "b2"))
            .await
            .unwrap();
        assert_eq!(updated.expect("updated").name, "edited-by-B");
        assert_eq!(
            repo.get(&owner_scope, p.id).await.unwrap().unwrap().name,
            "edited-by-B"
        );

        // B が新規作成 → オーナー A の名前空間に入る（A から見える）。
        let q = repo.add(&b_scope, new_persona("by-B", "q")).await.unwrap();
        assert!(repo.get(&owner_scope, q.id).await.unwrap().is_some());

        // 無関係ユーザー C（system_default）は A の共有ペルソナを見えない（テナント分離維持）。
        assert!(repo.list(&scope("C")).await.unwrap().is_empty());

        // B は削除も可能（共同編集）。
        assert!(repo.delete(&b_scope, p.id).await.unwrap());
        assert!(repo.get(&owner_scope, p.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn shared_bot_active_persona_is_single_synced_row() {
        let db = seed_db_full();
        let repo = PersonaRepo::new(&db);
        let owner_scope = scope_shared("A", "botX", "A");
        let b_scope = scope_shared("B", "botX", "A");
        let p = repo.add(&owner_scope, new_persona("P", "p")).await.unwrap();
        let q = repo.add(&owner_scope, new_persona("Q", "q")).await.unwrap();

        // A が P を適用 → B も同じ適用中ペルソナを読む（同期）。
        repo.set_active(&owner_scope, Some(p.id)).await.unwrap();
        assert_eq!(repo.active_persona_id(&b_scope).await.unwrap(), Some(p.id));

        // B が Q に切替 → A から見ても Q（共同編集・単一の共有行）。
        repo.set_active(&b_scope, Some(q.id)).await.unwrap();
        assert_eq!(
            repo.active_persona_id(&owner_scope).await.unwrap(),
            Some(q.id)
        );

        // bot_active_personas は botX につき 1 行のみ（オーナー A キー）。
        assert_eq!(count_active_rows(&db, "botX").await, 1);
    }

    #[tokio::test]
    async fn system_default_active_persona_stays_per_user() {
        let db = seed_db_full();
        let repo = PersonaRepo::new(&db);
        // system_default はオーナー無し＝発話ユーザー単位（共有秘書は各自独立）。
        let a = scope("A");
        let b = scope("B");
        let pa = repo.add(&a, new_persona("A-p", "a")).await.unwrap();
        let pb = repo.add(&b, new_persona("B-p", "b")).await.unwrap();
        repo.set_active(&a, Some(pa.id)).await.unwrap();
        repo.set_active(&b, Some(pb.id)).await.unwrap();
        // 各自の適用中は独立。
        assert_eq!(repo.active_persona_id(&a).await.unwrap(), Some(pa.id));
        assert_eq!(repo.active_persona_id(&b).await.unwrap(), Some(pb.id));
        // 相互に相手のペルソナは見えない。
        assert!(repo.get(&a, pb.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn shared_bot_import_lands_in_owner_namespace() {
        let db = seed_db_full();
        let repo = PersonaRepo::new(&db);
        // 第三者 C が公開ペルソナを持つ。
        let c = scope("C");
        let src = repo
            .add(&c, new_persona("public-src", "shared-body"))
            .await
            .unwrap();
        repo.set_public(&c, src.id, true).await.unwrap();

        // 共有相手 B が botX（オーナー A）上でインポート → オーナー A の名前空間へ独立コピーされる。
        let b_scope = scope_shared("B", "botX", "A");
        let imported = repo
            .import_public(&b_scope, src.id)
            .await
            .unwrap()
            .expect("imported");

        // オーナー A から見える（＝共有集合に追加され、A/B 双方で同期）。
        let owner_scope = scope_shared("A", "botX", "A");
        assert!(repo.get(&owner_scope, imported.id).await.unwrap().is_some());
        // 発話者 B 自身の user 単位（system_default）名前空間には入らない。
        assert!(repo.get(&scope("B"), imported.id).await.unwrap().is_none());
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
        assert!(repo
            .get(&scope("userB"), created.id)
            .await
            .unwrap()
            .is_none());
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
            .update(&scope("owner"), created.id, new_persona("renamed", "p1"))
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
        let after = repo
            .get(&scope("owner"), created.id)
            .await
            .unwrap()
            .unwrap();
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
        assert!(repo
            .add(&scope("u"), new_persona("   ", "x"))
            .await
            .is_err());
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
        app_with(seed_db())
    }

    fn app_with(db: Db) -> axum::Router {
        let auth = Arc::new(FakeAuth {
            user: SessionUser {
                discord_id: "u".to_owned(),
                username: "u".to_owned(),
                role: Role::User,
            },
        });
        let state = AppState::new(auth, WebConfig::default(), db);
        super::routes().with_state(state)
    }

    /// personas を手動作成した DB を開き、パスも返す（marketplace の公開行直挿し用）。
    fn seed_db_at() -> (Db, std::path::PathBuf) {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_persona_mkt_{}_{seq}.sqlite",
            std::process::id()
        ));
        {
            let conn = rusqlite::Connection::open(&path).expect("seed db");
            conn.execute_batch(PERSONAS_DDL).expect("create personas");
        }
        (Db::open(&path).expect("open db"), path)
    }

    /// 公開/非公開ペルソナを直挿しする（publish 未移植のため raw で is_public を立てる）。作成 id を返す。
    fn insert_persona(
        path: &std::path::Path,
        owner: &str,
        name: &str,
        prompt: &str,
        is_public: i64,
    ) -> i64 {
        let conn = rusqlite::Connection::open(path).expect("open");
        conn.execute(
            "INSERT INTO personas (owner_id, name, prompt, is_public) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![owner, name, prompt, is_public],
        )
        .expect("insert persona");
        conn.last_insert_rowid()
    }

    /// owner_username JOIN 用に users 行を入れる。
    fn insert_user(path: &std::path::Path, discord_id: &str, username: &str) {
        let conn = rusqlite::Connection::open(path).expect("open");
        conn.execute(
            "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt) \
             VALUES (?1, ?2, 'x', 'x')",
            rusqlite::params![discord_id, username],
        )
        .expect("insert user");
    }

    #[tokio::test]
    async fn marketplace_list_and_get_public() {
        let (db, path) = seed_db_at();
        insert_user(&path, "alice", "Alice");
        // 公開 2 件（alice）+ 非公開 1 件（bob）。
        let p_pub = insert_persona(&path, "alice", "Public One", "pub-prompt", 1);
        insert_persona(&path, "alice", "Public Two", "pub2", 1);
        let p_priv = insert_persona(&path, "bob", "Secret", "priv-prompt", 0);
        let repo = PersonaRepo::new(&db);

        // list_public は公開 2 件のみ・owner_username を JOIN（owner を跨ぐ公開読み取り）。
        let listed = repo.list_public().await.unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().all(|p| p.owner_username == "Alice"));
        assert!(listed
            .iter()
            .any(|p| p.name == "Public One" && p.prompt == "pub-prompt"));

        // get_public: 公開は取れる・非公開/不在は None（非公開を決して返さない）。
        let got = repo.get_public(p_pub).await.unwrap().expect("public");
        assert_eq!(got.name, "Public One");
        assert_eq!(got.prompt, "pub-prompt");
        assert!(repo.get_public(p_priv).await.unwrap().is_none());
        assert!(repo.get_public(9999).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn route_marketplace_list_get_and_guards() {
        let (db, path) = seed_db_at();
        insert_user(&path, "alice", "Alice");
        let pub_id = insert_persona(&path, "alice", "Shared", "shared-prompt", 1);
        let priv_id = insert_persona(&path, "bob", "Secret", "secret", 0);
        let app = app_with(db);

        // 一覧: 200・公開 1 件・owner_username 出力・owner_id 非露出。
        let list = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/personas/marketplace")
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
        assert_eq!(j["personas"].as_array().unwrap().len(), 1);
        assert_eq!(j["personas"][0]["name"], serde_json::json!("Shared"));
        assert_eq!(
            j["personas"][0]["owner_username"],
            serde_json::json!("Alice")
        );
        assert!(j["personas"][0]["owner_id"].is_null());

        // プレビュー: 公開 → 200 {persona:{id,name,prompt}}。
        let get = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/personas/marketplace/{pub_id}"))
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
        assert_eq!(j["persona"]["prompt"], serde_json::json!("shared-prompt"));

        // 非公開 → 404 + Node 文言。
        let priv_get = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/personas/marketplace/{priv_id}"))
                    .header("cookie", "__Host-yuuka-session=good")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(priv_get.status(), StatusCode::NOT_FOUND);
        let bytes = axum::body::to_bytes(priv_get.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            j["message"],
            serde_json::json!("公開ペルソナが見つかりません。")
        );

        // 非整数 id → 404（Node `Number.isInteger` 不成立）。
        let bad = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/personas/marketplace/abc")
                    .header("cookie", "__Host-yuuka-session=good")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bad.status(), StatusCode::NOT_FOUND);

        // 認証必須。
        let unauth = app
            .oneshot(
                Request::builder()
                    .uri("/api/personas/marketplace")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);
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
        let bytes = axum::body::to_bytes(list.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["success"], serde_json::json!(true));
        assert_eq!(j["personas"][0]["name"], serde_json::json!("hello"));
        assert_eq!(j["personas"][0]["prompt"], serde_json::json!("be nice"));
        assert_eq!(j["personas"][0]["is_public"], serde_json::json!(false));
        assert_eq!(j["max_length"], serde_json::json!(20000));
        // M-11: 未適用時は active_persona_id が null で存在する（キー欠落でない）。
        assert!(j.get("active_persona_id").is_some());
        assert!(j["active_persona_id"].is_null());
        // 内部列 owner_id は露出しない（構造的フェイルクローズ）。
        assert!(j["personas"][0]["owner_id"].is_null());
    }

    #[tokio::test]
    async fn import_public_copies_and_rejects_non_public() {
        let (db, path) = seed_db_at();
        let pub_id = insert_persona(&path, "alice", "Shared", "shared-prompt", 1);
        let priv_id = insert_persona(&path, "bob", "Secret", "secret", 0);
        let repo = PersonaRepo::new(&db);

        // 公開ペルソナを userX の所有として独立コピー（is_public=0）。
        let copied = repo
            .import_public(&scope("userX"), pub_id)
            .await
            .unwrap()
            .expect("imported");
        assert_eq!(copied.name, "Shared");
        assert_eq!(copied.prompt, "shared-prompt");
        assert!(!copied.is_public);
        assert_ne!(copied.id, pub_id, "独立コピー＝別 id");
        // userX の一覧に出る（自分の所有になった）。
        assert_eq!(repo.list(&scope("userX")).await.unwrap().len(), 1);
        // 元ソースは残る（コピーであって移動でない）。
        assert!(repo.get_public(pub_id).await.unwrap().is_some());

        // 非公開/不在はコピーできない（None）。
        assert!(repo
            .import_public(&scope("userX"), priv_id)
            .await
            .unwrap()
            .is_none());
        assert!(repo
            .import_public(&scope("userX"), 9999)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn route_import_persona() {
        let (db, path) = seed_db_at();
        let pub_id = insert_persona(&path, "alice", "Shared", "shared-prompt", 1);
        let priv_id = insert_persona(&path, "bob", "Secret", "secret", 0);
        let app = app_with(db);

        // 成功: 200 {persona, message}。応答 persona は自分の所有コピー（is_public=false）。
        let ok = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/personas/import")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"id":{pub_id}}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(ok.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["success"], serde_json::json!(true));
        assert_eq!(j["persona"]["name"], serde_json::json!("Shared"));
        assert_eq!(j["persona"]["is_public"], serde_json::json!(false));
        assert!(j["message"]
            .as_str()
            .unwrap()
            .contains("インポートしました"));
        assert!(j["persona"]["owner_id"].is_null());

        // id 欠落 → 400「id は必須です。」。
        let bad = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/personas/import")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(bad.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["message"], serde_json::json!("id は必須です。"));

        // 非公開 id → 404。
        let priv_resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/personas/import")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"id":{priv_id}}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(priv_resp.status(), StatusCode::NOT_FOUND);
    }

    /// bots.recommended_persona_id を読む（unpublish 解除の検証用）。
    fn recommended(path: &std::path::Path, bot_id: &str) -> Option<i64> {
        let conn = rusqlite::Connection::open(path).expect("open");
        conn.query_row(
            "SELECT recommended_persona_id FROM bots WHERE id = ?1",
            rusqlite::params![bot_id],
            |r| r.get::<_, Option<i64>>(0),
        )
        .expect("query")
    }

    async fn send_post(
        app: &axum::Router,
        uri: &str,
        body: &str,
    ) -> (StatusCode, serde_json::Value) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let j = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, j)
    }

    #[tokio::test]
    async fn set_public_toggles_and_detaches_recommending_bots() {
        let (db, path) = seed_db_at();
        insert_user(&path, "owner", "Owner");
        let pid = insert_persona(&path, "owner", "P", "prompt", 0);
        // このペルソナを推奨に設定した Bot（unpublish で解除されるはず）。
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute(
                "INSERT INTO bots (id, user_id, name, recommended_persona_id) \
                 VALUES ('b1', 'owner', 'B', ?1)",
                rusqlite::params![pid],
            )
            .unwrap();
        }
        let repo = PersonaRepo::new(&db);

        // 公開（0→1）: 是公開・Bot 推奨は維持（公開化では解除しない）。
        assert!(repo.set_public(&scope("owner"), pid, true).await.unwrap());
        assert!(repo.get_public(pid).await.unwrap().is_some());
        assert_eq!(recommended(&path, "b1"), Some(pid), "公開化では推奨維持");

        // 非公開化（1→0）: 非公開・**推奨 Bot から解除**。
        assert!(repo.set_public(&scope("owner"), pid, false).await.unwrap());
        assert!(repo.get_public(pid).await.unwrap().is_none());
        assert_eq!(recommended(&path, "b1"), None, "非公開化で推奨解除");

        // 他人は変更できない・不在も false。
        assert!(!repo
            .set_public(&scope("intruder"), pid, true)
            .await
            .unwrap());
        assert!(!repo.set_public(&scope("owner"), 9999, true).await.unwrap());
    }

    #[tokio::test]
    async fn route_publish_persona() {
        let (db, path) = seed_db_at();
        insert_user(&path, "u", "U"); // FakeAuth の user は "u"
        let mine = insert_persona(&path, "u", "Mine", "p", 0);
        let others = insert_persona(&path, "someone", "Theirs", "p2", 0);
        let app = app_with(db);

        // 公開: 200 success:true・Node 文言。
        let (st, j) = send_post(
            &app,
            "/api/personas/publish",
            &format!(r#"{{"id":{mine},"isPublic":true}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["success"], serde_json::json!(true));
        assert!(j["message"].as_str().unwrap().contains("公開しました"));

        // 非公開に戻す: success:true・非公開文言。
        let (_, j) = send_post(
            &app,
            "/api/personas/publish",
            &format!(r#"{{"id":{mine},"isPublic":false}}"#),
        )
        .await;
        assert!(j["message"].as_str().unwrap().contains("非公開にしました"));

        // 非 bool isPublic は非公開扱い（Node 厳密 ===true）。他人のペルソナ → success:false。
        let (st, j) = send_post(
            &app,
            "/api/personas/publish",
            &format!(r#"{{"id":{others},"isPublic":true}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["success"], serde_json::json!(false));
        assert!(j["message"]
            .as_str()
            .unwrap()
            .contains("所有者ではありません"));

        // id 欠落 → 400。
        let (st, j) = send_post(&app, "/api/personas/publish", r#"{"isPublic":true}"#).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert_eq!(j["message"], serde_json::json!("id は必須です。"));
    }

    #[tokio::test]
    async fn delete_cascades_active_and_recommended() {
        let (db, path) = seed_db_at();
        insert_user(&path, "u", "U");
        let pid = insert_persona(&path, "u", "P", "prompt", 1);
        // このペルソナを推奨した Bot ＋ 適用中状態を作る。
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute(
                "INSERT INTO bots (id, user_id, name, recommended_persona_id) \
                 VALUES ('b1', 'u', 'B', ?1)",
                rusqlite::params![pid],
            )
            .unwrap();
        }
        let repo = PersonaRepo::new(&db);
        repo.set_active(&scope("u"), Some(pid)).await.unwrap();
        assert_eq!(
            repo.active_persona_id(&scope("u")).await.unwrap(),
            Some(pid)
        );
        assert_eq!(recommended(&path, "b1"), Some(pid));

        // 削除で: personas 行削除 + bot_active_personas（FK cascade）+ bots.recommended（明示解除）。
        assert!(repo.delete(&scope("u"), pid).await.unwrap());
        assert!(
            repo.active_persona_id(&scope("u")).await.unwrap().is_none(),
            "適用中は FK ON DELETE CASCADE で消える"
        );
        assert_eq!(recommended(&path, "b1"), None, "推奨は明示解除される");
    }

    #[tokio::test]
    async fn set_active_upsert_and_clear() {
        let (db, path) = seed_db_at();
        insert_user(&path, "u", "U");
        let p1 = insert_persona(&path, "u", "One", "p1", 0);
        let p2 = insert_persona(&path, "u", "Two", "p2", 0);
        let repo = PersonaRepo::new(&db);

        // 初期は未適用。
        assert!(repo.active_persona_id(&scope("u")).await.unwrap().is_none());
        // 適用 → p1。
        repo.set_active(&scope("u"), Some(p1)).await.unwrap();
        assert_eq!(repo.active_persona_id(&scope("u")).await.unwrap(), Some(p1));
        // upsert → p2（PK(user,bot) なので 1 行が差し替わる）。
        repo.set_active(&scope("u"), Some(p2)).await.unwrap();
        assert_eq!(repo.active_persona_id(&scope("u")).await.unwrap(), Some(p2));
        // 解除 → None。
        repo.set_active(&scope("u"), None).await.unwrap();
        assert!(repo.active_persona_id(&scope("u")).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn route_activate_persona() {
        let (db, path) = seed_db_at();
        insert_user(&path, "u", "U");
        let mine = insert_persona(&path, "u", "Mine", "p", 0);
        let others = insert_persona(&path, "other", "Theirs", "p2", 0);
        let app = app_with(db.clone());

        // 適用: 200「…を適用しました。」・active が mine になる。
        let (st, j) = send_post(
            &app,
            "/api/personas/activate",
            &format!(r#"{{"id":{mine}}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(j["success"], serde_json::json!(true));
        assert!(j["message"].as_str().unwrap().contains("適用しました"));
        assert_eq!(
            PersonaRepo::new(&db)
                .active_persona_id(&scope("u"))
                .await
                .unwrap(),
            Some(mine)
        );

        // 他人のペルソナ → 403。
        let (st, j) = send_post(
            &app,
            "/api/personas/activate",
            &format!(r#"{{"id":{others}}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        assert!(j["message"]
            .as_str()
            .unwrap()
            .contains("自分のペルソナのみ"));

        // 解除（id null）→ 200「デフォルト…」・active None。
        let (st, j) = send_post(&app, "/api/personas/activate", r#"{"id":null}"#).await;
        assert_eq!(st, StatusCode::OK);
        assert!(j["message"].as_str().unwrap().contains("デフォルト"));
        assert!(PersonaRepo::new(&db)
            .active_persona_id(&scope("u"))
            .await
            .unwrap()
            .is_none());

        // 非整数 id → 400「id が不正です。」。
        let (st, j) = send_post(&app, "/api/personas/activate", r#"{"id":"abc"}"#).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert_eq!(j["message"], serde_json::json!("id が不正です。"));
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
