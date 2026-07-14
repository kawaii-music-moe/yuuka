//! yuuka-credential — credential ドメイン（T1 fan-out・**参照実装は yuuka-todo**）:
//! Repo + wire DTO + route を縦に持つ。凍結契約（core/types）は変更しない。
//! DAG: `credential → web, db, types, core`。ルータは supervisor が共通レイヤ配下にマージする。
//!
//! **機密ドメイン**: `credentials` テーブルの暗号化列（`encrypted_password`/`iv`/`auth_tag`）と
//! `user_id` は DTO のフィールドに存在させない（構造的フェイルクローズ・§6.4・R-13）。
//! テーブルは `(user_id, service_name)` 複合 PK で `bot_id` を持たないため、分離キーは
//! `user_id` のみを `WHERE` に必須化する（bot 利用許可は別表・下記 deferred）。
//!
//! Phase 1 参照スコープ = コア CRUD（list / get / delete）。以下は **deferred**（後回し）:
//! - `POST /api/credentials/register`（`services/secretService.ts` のユーザー鍵暗号化
//!   Argon2id + AES-256-GCM が必要・本クレート外の暗号層に依存）。
//! - GET 一覧の `bot_credential_access` 許可フィルタ（`listCredentialNamesForBot` で
//!   応対 Bot に許可済みの service だけへ絞り込む）。
//! - grant/revoke（`grantCredentialToOwnerBots` / `deleteAllGrantsForCredential`）連携。
//! - 復号を伴う record 取得（`getCredentialRecord`・secretService 経由のみ）。

pub mod access;
pub mod dto;
pub mod repo;
pub mod routes;
pub mod tools;

pub use access::CredentialAccessRepo;
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
    <dto::Credential as TS>::export_all(&cfg)?;
    <dto::CredentialListData as TS>::export_all(&cfg)?;
    <dto::DeleteCredential as TS>::export_all(&cfg)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use rusqlite::params;
    use tower::ServiceExt;
    use yuuka_core::{AuthError, BotId, UserId, UserScope};
    use yuuka_types::{Role, SessionUser};
    use yuuka_web::{AppState, AuthBackend, Db, WebConfig};

    use crate::repo::CredentialRepo;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    /// Node migrations.ts の credentials 表と一致（暗号化列を含む・SELECT はしない）。
    const CREDENTIALS_DDL: &str = "CREATE TABLE credentials (
        user_id TEXT NOT NULL,
        service_name TEXT NOT NULL,
        url TEXT,
        username TEXT NOT NULL,
        encrypted_password TEXT NOT NULL,
        iv TEXT NOT NULL,
        auth_tag TEXT NOT NULL,
        updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        PRIMARY KEY (user_id, service_name)
    );";

    fn scope(user: &str) -> UserScope {
        UserScope::new(UserId::new(user), BotId::system_default())
    }

    #[tokio::test]
    async fn list_get_and_scope_isolation() {
        let (db, path) = seed_db_at();
        insert_at(&path, "userA", "github", "alice");
        insert_at(&path, "userA", "aws", "alice2");
        insert_at(&path, "userB", "github", "bob");

        let repo = CredentialRepo::new(&db);

        // 一覧は service_name 昇順・自スコープのみ。
        let listed = repo.list(&scope("userA")).await.unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].service_name, "aws");
        assert_eq!(listed[1].service_name, "github");
        assert_eq!(listed[1].username, "alice");

        // 別ユーザーには自分の分だけ。
        let b = repo.list(&scope("userB")).await.unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].username, "bob");

        // get は正規化（trim + 小文字）して照合する。
        let got = repo
            .get(&scope("userA"), "  GitHub  ")
            .await
            .unwrap()
            .expect("found");
        assert_eq!(got.service_name, "github");
        // クロススコープの service は取れない。
        assert!(repo.get(&scope("userB"), "aws").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_normalizes_and_is_scoped() {
        let (db, path) = seed_db_at();
        insert_at(&path, "u", "github", "alice");
        let repo = CredentialRepo::new(&db);

        // 正規化して削除（"  GITHUB " → "github"）。
        assert!(repo.delete(&scope("u"), "  GITHUB ").await.unwrap());
        assert!(repo.list(&scope("u")).await.unwrap().is_empty());
        // 二重削除は false。
        assert!(!repo.delete(&scope("u"), "github").await.unwrap());
    }

    #[tokio::test]
    async fn delete_does_not_cross_scope() {
        let (db, path) = seed_db_at();
        insert_at(&path, "owner", "github", "alice");
        let repo = CredentialRepo::new(&db);

        // 別ユーザーは削除できない（分離キーを型で強制）。
        assert!(!repo.delete(&scope("attacker"), "github").await.unwrap());
        assert_eq!(repo.list(&scope("owner")).await.unwrap().len(), 1);
    }

    // ── route テスト ────────────────────────────────────────────────────────

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

    fn app_with(db: Db) -> axum::Router {
        app_with_crypto(db, None)
    }

    fn app_with_crypto(
        db: Db,
        crypto: Option<Arc<yuuka_crypto::SystemCrypto>>,
    ) -> axum::Router {
        let auth = Arc::new(FakeAuth {
            user: SessionUser {
                discord_id: "u".to_owned(),
                username: "u".to_owned(),
                role: Role::User,
            },
        });
        let state = AppState::new(auth, WebConfig::default(), db);
        super::routes_with(crypto).with_state(state)
    }

    #[tokio::test]
    async fn route_list_returns_clean_view() {
        let (db, path) = seed_db_at();
        insert_at(&path, "u", "github", "alice");
        // v5: 応対 Bot（未指定 → system_default）へ許可済みの service だけ返る。
        grant_at(&path, "system_default", "u", "github");
        let app = app_with(db);

        let list = app
            .oneshot(
                Request::builder()
                    .uri("/api/credentials")
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
        assert_eq!(
            j["credentials"][0]["service_name"],
            serde_json::json!("github")
        );
        assert_eq!(j["credentials"][0]["username"], serde_json::json!("alice"));
        // 機密フェイルクローズ: 暗号化列・user_id は絶対に露出しない。
        assert!(j["credentials"][0]["encrypted_password"].is_null());
        assert!(j["credentials"][0]["iv"].is_null());
        assert!(j["credentials"][0]["auth_tag"].is_null());
        assert!(j["credentials"][0]["user_id"].is_null());
    }

    /// GET 一覧は応対 Bot へ許可済み（bot_credential_access）の service だけ返す（未許可は除外）。
    #[tokio::test]
    async fn route_list_filters_ungranted() {
        let (db, path) = seed_db_at();
        insert_at(&path, "u", "github", "alice");
        insert_at(&path, "u", "aws", "alice2");
        // github だけ許可（aws は未許可 → 一覧に出ない）。
        grant_at(&path, "system_default", "u", "github");
        let app = app_with(db);

        let list = app
            .oneshot(
                Request::builder()
                    .uri("/api/credentials")
                    .header("cookie", "__Host-yuuka-session=good")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(list.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let arr = j["credentials"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["service_name"], serde_json::json!("github"));
    }

    #[tokio::test]
    async fn route_delete_then_list_empty() {
        let (db, path) = seed_db_at();
        insert_at(&path, "u", "github", "alice");
        grant_at(&path, "system_default", "u", "github");
        let app = app_with(db);

        let del = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/credentials/delete")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"serviceName":"GitHub"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(del.status(), StatusCode::OK);
        // 削除に伴い利用許可も掃除される（同名再登録時の許可復活を防ぐ）。
        assert!(granted_bots(&path, "u", "github").is_empty());

        let list = app
            .oneshot(
                Request::builder()
                    .uri("/api/credentials")
                    .header("cookie", "__Host-yuuka-session=good")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(list.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(j["credentials"].as_array().unwrap().is_empty());
    }

    /// M-12 golden: 該当無の delete は **404 ではなく** `200 {success:false}`（Node parity・
    /// `deletedServiceName` は返さない）。
    #[tokio::test]
    async fn route_delete_missing_is_bare_success_false() {
        let (db, _path) = seed_db_at();
        let app = app_with(db);
        let del = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/credentials/delete")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"serviceName":"nope"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(del.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(del.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["success"], serde_json::json!(false));
        assert!(j["deletedServiceName"].is_null());
        assert!(j["deleted_service_name"].is_null());
    }

    /// delete のガードは Node `!serviceName`（trim せず）パリティ: 空文字は 400「サービス名は必須です。」、
    /// 空白のみは通って正規化後不一致で 200 {success:false} の no-op（400 にしない）。
    #[tokio::test]
    async fn route_delete_guard_matches_node() {
        let (db, _path) = seed_db_at();
        let app = app_with(db);

        // 空文字 → 400 + 日本語メッセージ。
        let empty = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/credentials/delete")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"serviceName":""}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(empty.status(), StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(empty.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["message"], serde_json::json!("サービス名は必須です。"));

        // 空白のみ → 200 {success:false}（no-op・400 にしない）。
        let ws = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/credentials/delete")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"serviceName":"   "}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ws.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(ws.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["success"], serde_json::json!(false));
    }

    #[tokio::test]
    async fn route_requires_auth() {
        let (db, _path) = seed_db_at();
        let resp = app_with(db)
            .oneshot(
                Request::builder()
                    .uri("/api/credentials")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// register: 暗号化保存 + owner 本人の全 Bot(system_default,b1) へ付与 + GET 一覧へ反映。
    /// 必須欠落は Node と同じ 400 `{success:false}`。
    #[tokio::test]
    async fn route_register_encrypts_and_grants_to_owner_bots() {
        let (db, path) = seed_db_at();
        // register は writer 経由で保存/付与するため FK 有効。users("u", salt) + 所有 Bot を seed。
        let salt = yuuka_crypto::generate_user_salt().unwrap();
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt) \
                 VALUES ('u','u','x',?1)",
                params![salt],
            )
            .unwrap();
            for bot in ["system_default", "b1"] {
                conn.execute(
                    "INSERT OR IGNORE INTO bots (id, user_id, name) VALUES (?1, 'u', 'n')",
                    params![bot],
                )
                .unwrap();
            }
        }
        let crypto =
            Arc::new(yuuka_crypto::SystemCrypto::new(secrecy::SecretString::from("master-secret")).unwrap());
        let app = app_with_crypto(db, Some(crypto));

        // 必須欠落 → 400 {success:false, message}。
        let bad = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/credentials/register")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"serviceName":"GitHub"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(bad.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(j["success"], serde_json::json!(false));

        // 正常登録 → 200 success:true・正規化(github)保存・owner 全 Bot へ付与。
        let ok = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/credentials/register")
                    .header("cookie", "__Host-yuuka-session=good")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"serviceName":"  GitHub ","username":"me","password":"pw","url":"https://gh.test"}"#,
                    ))
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
        assert_eq!(
            granted_bots(&path, "u", "github"),
            vec!["b1".to_owned(), "system_default".to_owned()]
        );

        // GET 一覧へ反映（system_default へ許可済み）。暗号化列は出ない。
        let list = app
            .oneshot(
                Request::builder()
                    .uri("/api/credentials")
                    .header("cookie", "__Host-yuuka-session=good")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(list.into_body(), usize::MAX)
            .await
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let arr = j["credentials"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["service_name"], serde_json::json!("github"));
        assert!(arr[0]["encrypted_password"].is_null());
    }

    // ── テスト用ヘルパ（seed パスを明示保持して直挿しする） ─────────────────

    fn seed_db_at() -> (Db, std::path::PathBuf) {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_credential_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        {
            let conn = rusqlite::Connection::open(&path).expect("seed db");
            conn.execute_batch(CREDENTIALS_DDL)
                .expect("create credentials");
        }
        let db = Db::open(&path).expect("open db");
        (db, path)
    }

    fn insert_at(path: &std::path::Path, user_id: &str, service_name: &str, username: &str) {
        let conn = rusqlite::Connection::open(path).expect("open");
        conn.execute(
            "INSERT INTO credentials \
               (user_id, service_name, url, username, encrypted_password, iv, auth_tag, updated_at) \
             VALUES (?1, ?2, ?3, ?4, 'ENC', 'IV', 'TAG', datetime('now','localtime'))",
            params![user_id, service_name, "https://ex.test", username],
        )
        .expect("insert credential");
    }

    /// 利用許可を直挿しする（FK を切って users/bots seed を省く・GET 一覧フィルタ検証用）。
    fn grant_at(path: &std::path::Path, bot_id: &str, owner_id: &str, service_name: &str) {
        let conn = rusqlite::Connection::open(path).expect("open");
        conn.execute_batch("PRAGMA foreign_keys=OFF")
            .expect("fk off");
        conn.execute(
            "INSERT OR IGNORE INTO bot_credential_access (bot_id, owner_id, service_name) \
             VALUES (?1, ?2, ?3)",
            params![bot_id, owner_id, service_name],
        )
        .expect("insert grant");
    }

    /// owner×service に紐づく許可 Bot 一覧を直読みする（delete の掃除・register の付与を検証する）。
    fn granted_bots(path: &std::path::Path, owner_id: &str, service_name: &str) -> Vec<String> {
        let conn = rusqlite::Connection::open(path).expect("open");
        let mut stmt = conn
            .prepare(
                "SELECT bot_id FROM bot_credential_access \
                 WHERE owner_id = ?1 AND service_name = ?2 ORDER BY bot_id",
            )
            .expect("prepare");
        let rows = stmt
            .query_map(params![owner_id, service_name], |r| r.get::<_, String>(0))
            .expect("query");
        rows.map(|r| r.expect("row")).collect()
    }
}
