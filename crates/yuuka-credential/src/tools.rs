//! credential ドメインの Native ツール（現行 `src/functions/credentialFunctions.ts` の移植・§6.4）。
//!
//! **参照実装**: `yuuka-todo::tools`（構造・命名・ok/fail JSON・bare ツール名・`UserScope`・
//! `DbError`→`ToolError::Execution` を踏襲）。core の凍結 [`Tool`] を実装し、`tools(db)` が
//! `Vec<Arc<dyn Tool>>` を返す（assembly 層が `NativeProvider` へ登録する）。
//!
//! **wire 契約の非対称に注意**: HTTP route の body は camelCase（`serviceName`）だが、**tool 引数は
//! snake_case**（`service_name`・Node の Gemini 宣言と一致）。ツール名は Node の system prompt が
//! 参照する **bare 名**（`listCredentialServices` / `deleteCredential`）を使う。
//!
//! **機密フェイルクローズ**: 一覧は暗号化列（`encrypted_password`/`iv`/`auth_tag`）を **決して
//! 露出しない**（repo が SELECT しない・§6.4）。パスワード値はツールの応答に一切含めない。
//!
//! 移植済み: listCredentialServices（→ `CredentialRepo::list`）/ deleteCredential
//! （→ `CredentialRepo::delete`）。**未移植（deferred）**:
//! - `addCredential` / `updateCredential`: repo に add/update 相当が無く、`services/secretService.ts`
//!   のユーザー鍵暗号化（Argon2id + AES-256-GCM）に依存するため本クレート外。
//! - `browserFillCredential`: 平文パスワードの復号（`getDecryptedCredential`）と browserService への
//!   直接入力が必要で、repo `get`（クリーンビュー・パスワード非含有）では代替不能。
//! - listCredentialServices / deleteCredential の `bot_credential_access` 許可フィルタ・grant 掃除も
//!   deferred（HTTP route と同じく本コア CRUD では扱わない）。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use yuuka_core::tool::FunctionDeclaration;
use yuuka_core::{DbError, Tool, ToolContext, ToolError, ToolName, ToolOutcome, UserScope};
use yuuka_web::Db;

use crate::repo::CredentialRepo;

/// このドメインが公開する Native ツール一式を作る。
///
/// assembly 層（bot/WS）が `NativeProvider::register` で束ねる。
///
/// # Errors
/// ツール名が Gemini 制約に反する場合 [`ToolError`]（コンパイル時定数なので通常発生しない）。
pub fn tools(db: Db) -> Result<Vec<Arc<dyn Tool>>, ToolError> {
    Ok(vec![
        Arc::new(ListCredentialServicesTool {
            name: ToolName::checked("listCredentialServices".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(DeleteCredentialTool {
            name: ToolName::checked("deleteCredential".to_owned())?,
            db,
        }),
    ])
}

// ─── 共通ヘルパ（todo/tools.rs と同一規約） ────────────────────────────────────

/// `{success:true, message, ...extra}`（Node `ok(msg, extra)` 相当）。
fn ok_payload(message: impl Into<String>, extra: Value) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("success".to_owned(), Value::Bool(true));
    obj.insert("message".to_owned(), Value::String(message.into()));
    if let Value::Object(map) = extra {
        for (k, v) in map {
            obj.insert(k, v);
        }
    }
    Value::Object(obj)
}

/// `{success:false, message}`（Node `fail(msg)`）。実行エラーではなく「妥当だが失敗」な結果。
fn fail_payload(message: impl Into<String>) -> ToolOutcome {
    ToolOutcome::from_payload(json!({ "success": false, "message": message.into() }))
}

/// ctx からデータ分離スコープを組む（credential 表は user_id のみで束ねる・§12.2 契約5）。
fn scope_of(ctx: &ToolContext) -> UserScope {
    UserScope::new(ctx.user_id.clone(), ctx.bot_id.clone())
}

/// `asOptionalString`（trim 後空なら None）。
fn arg_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// DbError をツール実行エラーへ（握り潰さず Gemini へ `{success:false}` として返る・§8.4）。
fn exec_err(e: DbError) -> ToolError {
    ToolError::Execution(e.to_string())
}

// ─── listCredentialServices ───────────────────────────────────────────────────

struct ListCredentialServicesTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for ListCredentialServicesTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "パスワード保管庫に登録済みのサービス名・ユーザー名・URLの一覧を見る（パスワードは出ない）。\n\
                ・例:「どのサービスのアカウントを登録してる？」と聞かれた時。\n\
                ・ブラウザ自動ログイン（browserFillCredential）の前に、正しいサービス名を確かめたい時にも使う。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {}
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, _args: Value) -> Result<ToolOutcome, ToolError> {
        let services = CredentialRepo::new(&self.db)
            .list(&scope_of(ctx))
            .await
            .map_err(exec_err)?;

        if services.is_empty() {
            return Ok(ToolOutcome::from_payload(ok_payload(
                "このBotが利用を許可された認証情報はありません。addCredential で登録すると自動的にこのBotへ許可されます。既存の認証情報は統合管理ページ（Bot統合管理）でこのBotへ利用を許可してください。",
                json!({ "services": [] }),
            )));
        }

        let lines: Vec<String> = services
            .iter()
            .map(|s| {
                let url_part = match s.url.as_deref().filter(|u| !u.is_empty()) {
                    Some(u) => format!("、URL: {u}"),
                    None => String::new(),
                };
                format!(
                    "🔑 {}（ユーザー名: {}{}）",
                    s.service_name, s.username, url_part
                )
            })
            .collect();
        let message = format!(
            "登録済みサービス一覧 ({}件):\n{}\n※パスワードは表示できません。",
            services.len(),
            lines.join("\n"),
        );
        Ok(ToolOutcome::from_payload(ok_payload(
            message,
            json!({ "services": services }),
        )))
    }
}

// ─── deleteCredential ─────────────────────────────────────────────────────────

struct DeleteCredentialTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for DeleteCredentialTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "登録済みのログイン情報を保管庫から完全に消す（取り消せない）。\n\
                ・ユーザーが「消して」とはっきり頼んだ時だけ呼ぶ。\n\
                ・呼ぶ前に、消すサービス名をユーザーに読み上げて確認をもらう。\n\
                ・確認なしに勝手に消さない。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "service_name": {
                        "type": "string",
                        "description": "消したいサービス名。listCredentialServices で確認できる。"
                    }
                },
                "required": ["service_name"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(service_name) = arg_str(&args, "service_name") else {
            return Ok(fail_payload("service_name を指定してください。"));
        };

        // repo.delete が内部で正規化（trim + 小文字化）して照合する（Node と一致）。
        let deleted = CredentialRepo::new(&self.db)
            .delete(&scope_of(ctx), &service_name)
            .await
            .map_err(exec_err)?;

        if deleted {
            // 成功メッセージは正規化名（Node `serviceName.trim().toLowerCase()` と一致）。
            Ok(ToolOutcome::from_payload(ok_payload(
                format!(
                    "「{}」の認証情報を削除しました🗑️",
                    service_name.to_lowercase()
                ),
                json!({}),
            )))
        } else {
            // not-found は Node と同じく入力名（trim 済み・原ケース）で示す。
            Ok(fail_payload(format!(
                "サービス「{service_name}」の認証情報が見つかりません。listCredentialServices で登録済みサービス名を確認してください。"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use rusqlite::{params, Connection};
    use yuuka_core::{BotId, UserId};

    use super::*;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    /// Node migrations.ts の credentials 表と一致（暗号化列を含む・ツールは SELECT しない）。
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

    fn seed_db_at() -> (Db, std::path::PathBuf) {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_credential_tools_{}_{seq}.sqlite",
            std::process::id()
        ));
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(CREDENTIALS_DDL).unwrap();
        }
        (Db::open(&path).unwrap(), path)
    }

    fn insert_at(path: &std::path::Path, user_id: &str, service_name: &str, username: &str, url: &str) {
        let conn = Connection::open(path).unwrap();
        conn.execute(
            "INSERT INTO credentials \
               (user_id, service_name, url, username, encrypted_password, iv, auth_tag, updated_at) \
             VALUES (?1, ?2, ?3, ?4, 'ENC', 'IV', 'TAG', datetime('now','localtime'))",
            params![user_id, service_name, url, username],
        )
        .unwrap();
    }

    fn ctx(user: &str) -> ToolContext {
        ToolContext::new(BotId::system_default(), UserId::new(user))
    }

    fn find<'a>(tools: &'a [Arc<dyn Tool>], name: &str) -> &'a Arc<dyn Tool> {
        tools
            .iter()
            .find(|t| t.declaration().name.as_str() == name)
            .unwrap()
    }

    #[tokio::test]
    async fn list_and_delete_roundtrip() {
        let (db, path) = seed_db_at();
        insert_at(&path, "userA", "github", "alice", "https://gh.test");
        insert_at(&path, "userA", "aws", "alice2", "");
        let tools = tools(db).unwrap();

        // 宣言名は bare（Node system prompt と一致）。
        let names: Vec<String> = tools.iter().map(|t| t.declaration().name.to_string()).collect();
        assert!(names.contains(&"listCredentialServices".to_owned()));
        assert!(names.contains(&"deleteCredential".to_owned()));
        assert!(!names.iter().any(|n| n.contains(':')), "native は bare 名");

        // list: service_name 昇順で 2 件、暗号化列は payload に出ない。
        let list = find(&tools, "listCredentialServices");
        let out = list.call(&ctx("userA"), json!({})).await.unwrap();
        assert_eq!(out.payload["success"], true);
        let arr = out.payload["services"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["service_name"], "aws");
        assert_eq!(arr[1]["service_name"], "github");
        assert_eq!(arr[1]["username"], "alice");
        // 機密フェイルクローズ: 暗号化列・user_id は絶対に露出しない。
        assert!(arr[0]["encrypted_password"].is_null());
        assert!(arr[0]["iv"].is_null());
        assert!(arr[0]["auth_tag"].is_null());
        assert!(arr[0]["user_id"].is_null());

        // delete: 入力は正規化（"  GitHub " → "github"）して照合、成功。
        let delete = find(&tools, "deleteCredential");
        let out = delete
            .call(&ctx("userA"), json!({"service_name": "  GitHub "}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);

        // list で 1 件（aws）に減る。
        let out = list.call(&ctx("userA"), json!({})).await.unwrap();
        assert_eq!(out.payload["services"].as_array().unwrap().len(), 1);

        // 二重削除は not-found（success:false）。
        let out = delete
            .call(&ctx("userA"), json!({"service_name": "github"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);
    }

    #[tokio::test]
    async fn delete_validates_and_list_empty_message() {
        let (db, _path) = seed_db_at();
        let tools = tools(db).unwrap();

        // service_name 欠落 → fail。
        let delete = find(&tools, "deleteCredential");
        let out = delete.call(&ctx("u"), json!({})).await.unwrap();
        assert_eq!(out.payload["success"], false);

        // 空一覧でも success:true・services:[]（形状 parity）。
        let list = find(&tools, "listCredentialServices");
        let out = list.call(&ctx("u"), json!({})).await.unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["services"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn scope_isolation_across_users() {
        let (db, path) = seed_db_at();
        insert_at(&path, "userA", "github", "alice", "");
        insert_at(&path, "userB", "aws", "bob", "");
        let tools = tools(db).unwrap();
        let list = find(&tools, "listCredentialServices");

        // 別ユーザーには自分の分だけ。
        let out = list.call(&ctx("userB"), json!({})).await.unwrap();
        let arr = out.payload["services"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["service_name"], "aws");

        // 別ユーザーは他人の認証情報を削除できない（分離キーを型で強制）。
        let delete = find(&tools, "deleteCredential");
        let out = delete
            .call(&ctx("userB"), json!({"service_name": "github"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);
        // userA の github は残る。
        let out = list.call(&ctx("userA"), json!({})).await.unwrap();
        assert_eq!(out.payload["services"].as_array().unwrap().len(), 1);
    }
}
