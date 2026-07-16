//! MCP サーバー拡張リポジトリ（Node `src/db/mcpRepo.ts` パリティ・自己完結）。
//!
//! `user_id = NULL` の行はシステムレベル登録（Admin のみ管理・全ユーザー利用可）。owner（`user_id`）
//! 所有のまま「使わせる Bot を許可リスト（`bot_mcp_access`）で選ぶ」共有モデル。`bot_id` 列は退役
//! （NOT NULL DEFAULT `'system_default'` が入るのみ・参照しない）。

use rusqlite::{params, OptionalExtension};
use serde_json::Value;
use yuuka_core::DbError;
use yuuka_db::map_sqlite;
use yuuka_web::Db;

/// `mcp_servers` の 1 行（Node `McpServerRecord`）。
#[derive(Debug, Clone)]
pub struct McpServerRecord {
    /// PK。
    pub id: i64,
    /// 所有者 discord_id（`None` = システムレベル）。
    pub user_id: Option<String>,
    /// 表示名。
    pub name: String,
    /// MCP エンドポイント URL。
    pub endpoint_url: String,
    /// 認証情報 暗号文（`None` = 認証なし）。
    pub auth_credential_encrypted: Option<String>,
    /// 認証情報 IV。
    pub auth_credential_iv: Option<String>,
    /// 認証情報 authTag。
    pub auth_credential_tag: Option<String>,
    /// `tools/list` キャッシュ（JSON 文字列）。
    pub tools_cache: String,
    /// キャッシュ更新時刻（`None` = 未取得）。
    pub tools_cache_updated: Option<String>,
    /// tools/call 前に確認を要求するか（1/0）。
    pub requires_confirmation: i64,
    /// 有効フラグ（1/0）。
    pub enabled: i64,
    /// 作成時刻。
    pub created_at: String,
}

impl McpServerRecord {
    /// 全カラムを 1 行から読む共通マッパ（`SELECT *` の列順に依存しないよう名前で引く）。
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            user_id: row.get("user_id")?,
            name: row.get("name")?,
            endpoint_url: row.get("endpoint_url")?,
            auth_credential_encrypted: row.get("auth_credential_encrypted")?,
            auth_credential_iv: row.get("auth_credential_iv")?,
            auth_credential_tag: row.get("auth_credential_tag")?,
            tools_cache: row
                .get::<_, Option<String>>("tools_cache")?
                .unwrap_or_else(|| "[]".to_owned()),
            tools_cache_updated: row.get("tools_cache_updated")?,
            requires_confirmation: row.get("requires_confirmation")?,
            enabled: row.get("enabled")?,
            created_at: row.get("created_at")?,
        })
    }

    /// 認証情報が設定されているか（Node `has_auth = !!auth_credential_encrypted`）。
    #[must_use]
    pub fn has_auth(&self) -> bool {
        self.auth_credential_encrypted
            .as_deref()
            .is_some_and(|s| !s.is_empty())
    }

    /// スコープ文字列（`user_id IS NULL ? "system" : "user"`・Node `toSafeView` の scope）。
    #[must_use]
    pub fn scope(&self) -> &'static str {
        if self.user_id.is_none() {
            "system"
        } else {
            "user"
        }
    }
}

/// 単一列（`name`/`description`）に絞った tools_cache のツール（Node `toSafeView` の tools 要素）。
#[derive(Debug, Clone)]
pub struct SafeTool {
    /// ツール名。
    pub name: String,
    /// 説明（欠落は空文字・Node `t.description ?? ""`）。
    pub description: String,
}

/// tools_cache（JSON）を `{name, description}` の配列へパースする（Node `parseToolsCache` + `toSafeView`）。
/// 非配列/パース失敗は空 vec。各要素は object のみ・`name` は文字列化・`description` は `??""`。
#[must_use]
pub fn parse_tools_cache(server: &McpServerRecord) -> Vec<SafeTool> {
    let raw = if server.tools_cache.is_empty() {
        "[]"
    } else {
        server.tools_cache.as_str()
    };
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|t| {
            let obj = t.as_object()?;
            let name = obj.get("name").and_then(Value::as_str).unwrap_or_default();
            let description = obj
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default();
            Some(SafeTool {
                name: name.to_owned(),
                description: description.to_owned(),
            })
        })
        .collect()
}

/// tools_cache（JSON）を FC ループ用の完全な [`crate::McpTool`]（name/description/inputSchema）へパースする。
///
/// [`parse_tools_cache`]（安全ビュー・name/description のみ）と異なり `inputSchema` を保持する。
/// Node `parseToolsCache`（`McpToolDef[]` をそのまま返す）+ [`crate::McpProvider`] の消費に対応。非配列/
/// パース失敗は空 vec・各要素は object のみ・`name` 空は落とす（動的ツール名を作れないため）。
#[must_use]
pub fn parse_tools_cache_full(server: &McpServerRecord) -> Vec<crate::McpTool> {
    let raw = if server.tools_cache.is_empty() {
        "[]"
    } else {
        server.tools_cache.as_str()
    };
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|t| {
            let obj = t.as_object()?;
            let name = obj.get("name").and_then(Value::as_str).unwrap_or_default();
            if name.is_empty() {
                return None;
            }
            let description = obj
                .get("description")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            let input_schema = obj.get("inputSchema").filter(|s| s.is_object()).cloned();
            Some(crate::McpTool {
                name: name.to_owned(),
                description,
                input_schema,
            })
        })
        .collect()
}

/// 対象サーバーの操作権限を検証する（Node `canManage`）。本人所有、または（システム登録かつ Admin）。
#[must_use]
pub fn can_manage(server: &McpServerRecord, user_id: &str, is_admin: bool) -> bool {
    match server.user_id.as_deref() {
        None => is_admin,
        Some(owner) => owner == user_id,
    }
}

/// ユーザーが Admin ロールか（Node `getUserByDiscordId(...).role === "admin"`）。行が無ければ `false`。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn is_admin(db: &Db, discord_id: &str) -> Result<bool, DbError> {
    let id = discord_id.to_owned();
    db.read
        .read(move |conn| {
            let role: Option<String> = conn
                .query_row(
                    "SELECT role FROM users WHERE discord_id = ?1",
                    params![id],
                    |r| r.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite)?;
            Ok(role.as_deref() == Some("admin"))
        })
        .await
}

/// トークン発行ユーザーの実在確認 + Admin 判定を 1 read で行う（プロキシ再検証用・Node
/// `getUserByDiscordId` + `role === "admin"`）。返り値は `Some(is_admin)`（不在は `None`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn lookup_user_admin(db: &Db, discord_id: &str) -> Result<Option<bool>, DbError> {
    let id = discord_id.to_owned();
    db.read
        .read(move |conn| {
            let role: Option<String> = conn
                .query_row(
                    "SELECT role FROM users WHERE discord_id = ?1",
                    params![id],
                    |r| r.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite)?;
            Ok(role.map(|r| r == "admin"))
        })
        .await
}

/// 1 件を id で引く（Node `getServerById`）。不在は `None`。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_server_by_id(db: &Db, id: i64) -> Result<Option<McpServerRecord>, DbError> {
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT * FROM mcp_servers WHERE id = ?1",
                params![id],
                McpServerRecord::from_row,
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// 全カラムを取り出す共通クエリ（`from_row` と対で使う）。owner 一覧・system 一覧で SQL のみ差し替える。
async fn query_servers(db: &Db, sql: &'static str, arg: Option<String>) -> Result<Vec<McpServerRecord>, DbError> {
    db.read
        .read(move |conn| {
            let mut stmt = conn.prepare(sql).map_err(map_sqlite)?;
            let mapper = McpServerRecord::from_row;
            let rows = match arg {
                Some(ref a) => stmt.query_map(params![a], mapper),
                None => stmt.query_map([], mapper),
            }
            .map_err(map_sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(map_sqlite)?);
            }
            Ok(out)
        })
        .await
}

/// owner 本人が登録したサーバー一覧（Node `listServersForOwner`・`created_at ASC`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_servers_for_owner(db: &Db, user_id: &str) -> Result<Vec<McpServerRecord>, DbError> {
    query_servers(
        db,
        "SELECT * FROM mcp_servers WHERE user_id = ?1 ORDER BY created_at ASC",
        Some(user_id.to_owned()),
    )
    .await
}

/// システムレベル登録（`user_id IS NULL`）の一覧（Node `listSystemServers`・`created_at ASC`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_system_servers(db: &Db) -> Result<Vec<McpServerRecord>, DbError> {
    query_servers(
        db,
        "SELECT * FROM mcp_servers WHERE user_id IS NULL ORDER BY created_at ASC",
        None,
    )
    .await
}

/// 当該 Bot に許可された MCP サーバー一覧（Node `listServersGrantedToBot`）。
///
/// = `bot_mcp_access` で許可されたサーバー（owner 所有）+ システムレベル登録（`user_id IS NULL`・全 Bot 利用可）。
/// 単一所有 Bot のランタイム解決に使う（共有秘書 `system_default` では
/// [`list_servers_granted_to_bot_scoped`] を使いクロステナント露出を防ぐ）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_servers_granted_to_bot(db: &Db, bot_id: &str) -> Result<Vec<McpServerRecord>, DbError> {
    let bot_id = bot_id.to_owned();
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT s.* FROM mcp_servers s \
                     JOIN bot_mcp_access a ON a.mcp_server_id = s.id AND a.bot_id = ?1 \
                     UNION \
                     SELECT s.* FROM mcp_servers s WHERE s.user_id IS NULL \
                     ORDER BY created_at ASC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![bot_id], McpServerRecord::from_row)
                .map_err(map_sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(map_sqlite)?);
            }
            Ok(out)
        })
        .await
}

/// 発話者スコープの MCP サーバー一覧（Node `listServersGrantedToBotScoped`・ランタイムのセキュリティゲート）。
///
/// 当該 Bot に「発話者（`speaker_user_id`）が付与した」許可分（`owner_id = speaker_user_id`）と、
/// システムレベル登録（`user_id IS NULL`・全 Bot 利用可）の和。共有秘書（`system_default`）は全ユーザーが
/// 会話するため、他人が付与した許可（他人の認証情報を抱えた MCP サーバー）が発話者の会話へ注入されない
/// ように発話者所有分のみへ限定する。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_servers_granted_to_bot_scoped(
    db: &Db,
    bot_id: &str,
    speaker_user_id: &str,
) -> Result<Vec<McpServerRecord>, DbError> {
    let (bot_id, speaker) = (bot_id.to_owned(), speaker_user_id.to_owned());
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT s.* FROM mcp_servers s \
                     JOIN bot_mcp_access a ON a.mcp_server_id = s.id AND a.bot_id = ?1 AND a.owner_id = ?2 \
                     UNION \
                     SELECT s.* FROM mcp_servers s WHERE s.user_id IS NULL \
                     ORDER BY created_at ASC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![bot_id, speaker], McpServerRecord::from_row)
                .map_err(map_sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(map_sqlite)?);
            }
            Ok(out)
        })
        .await
}

/// 当該サーバーの利用を許可されている Bot ID 一覧（重複排除・Node `listBotIdsForServer`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_bot_ids_for_server(db: &Db, server_id: i64) -> Result<Vec<String>, DbError> {
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare("SELECT DISTINCT bot_id FROM bot_mcp_access WHERE mcp_server_id = ?1")
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![server_id], |r| r.get::<_, String>(0))
                .map_err(map_sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(map_sqlite)?);
            }
            Ok(out)
        })
        .await
}

/// サーバーを 1 件追加する（Node `addServer`）。`auth_credential` は暗号化済み 3 列で渡す
/// （呼び出し側が [`yuuka_crypto`] で暗号化・空/未指定は全 `None`）。挿入した行を返す。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn add_server(
    db: &Db,
    user_id: Option<String>,
    name: &str,
    endpoint_url: &str,
    enc: Option<(String, String, String)>,
    requires_confirmation: bool,
) -> Result<McpServerRecord, DbError> {
    let name = name.to_owned();
    let endpoint_url = endpoint_url.to_owned();
    let (encrypted, iv, tag) = match enc {
        Some((e, i, t)) => (Some(e), Some(i), Some(t)),
        None => (None, None, None),
    };
    let confirm = i64::from(requires_confirmation);
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT INTO mcp_servers \
                 (user_id, name, endpoint_url, auth_credential_encrypted, auth_credential_iv, \
                  auth_credential_tag, requires_confirmation) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![user_id, name, endpoint_url, encrypted, iv, tag, confirm],
            )
            .map_err(map_sqlite)?;
            let id = tx.last_insert_rowid();
            tx.query_row(
                "SELECT * FROM mcp_servers WHERE id = ?1",
                params![id],
                McpServerRecord::from_row,
            )
            .map_err(map_sqlite)
        })
        .await
}

/// tools_cache を JSON 文字列で更新する（Node `updateToolsCache`・`tools_cache_updated` も更新）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn update_tools_cache(db: &Db, id: i64, tools_json: &str) -> Result<(), DbError> {
    let tools_json = tools_json.to_owned();
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "UPDATE mcp_servers SET tools_cache = ?1, \
                 tools_cache_updated = datetime('now', 'localtime') WHERE id = ?2",
                params![tools_json, id],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// 有効/無効を設定する（Node `setEnabled`）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn set_enabled(db: &Db, id: i64, enabled: bool) -> Result<(), DbError> {
    let flag = i64::from(enabled);
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "UPDATE mcp_servers SET enabled = ?1 WHERE id = ?2",
                params![flag, id],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

/// サーバーを削除する（Node `deleteServer`）。本人登録分は本人のみ、システムレベル登録は Admin のみ削除可。
/// 削除できれば `true`（不在・権限なしは `false`）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn delete_server(
    db: &Db,
    id: i64,
    requesting_user_id: &str,
    is_admin_user: bool,
) -> Result<bool, DbError> {
    let requesting_user_id = requesting_user_id.to_owned();
    db.writer
        .transaction(move |tx| {
            let owner: Option<Option<String>> = tx
                .query_row(
                    "SELECT user_id FROM mcp_servers WHERE id = ?1",
                    params![id],
                    |r| r.get::<_, Option<String>>(0),
                )
                .optional()
                .map_err(map_sqlite)?;
            // 行が無ければ false（Node: getServerById → undefined → false）。
            let Some(owner) = owner else {
                return Ok(false);
            };
            match owner {
                None => {
                    if !is_admin_user {
                        return Ok(false);
                    }
                }
                Some(o) if o != requesting_user_id => return Ok(false),
                Some(_) => {}
            }
            let changed = tx
                .execute("DELETE FROM mcp_servers WHERE id = ?1", params![id])
                .map_err(map_sqlite)?;
            Ok(changed > 0)
        })
        .await
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use rusqlite::Connection;

    use super::*;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn fresh_db() -> (Db, PathBuf) {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_mcp_repo_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        drop(Connection::open(&path).expect("create db file"));
        let db = Db::open(&path).expect("open db");
        (db, path)
    }

    fn raw(path: &PathBuf) -> Connection {
        Connection::open(path).expect("raw conn")
    }

    fn seed_user(path: &PathBuf, discord_id: &str, role: &str) {
        raw(path)
            .execute(
                "INSERT INTO users (discord_id, username, password_hash, salt, role) \
                 VALUES (?1, ?1, 'h', 'deadbeef', ?2)",
                params![discord_id, role],
            )
            .expect("seed user");
    }

    #[tokio::test]
    async fn add_get_and_safe_fields() {
        let (db, path) = fresh_db();
        seed_user(&path, "alice", "user");
        let rec = add_server(
            &db,
            Some("alice".to_owned()),
            "  MyMcp  ",
            "https://mcp.example/mcp",
            Some(("enc".to_owned(), "iv".to_owned(), "tag".to_owned())),
            true,
        )
        .await
        .unwrap();
        assert_eq!(rec.name, "  MyMcp  "); // repo は trim しない（呼び出し側 trim 済み想定）。
        assert!(rec.has_auth());
        assert_eq!(rec.scope(), "user");
        assert_eq!(rec.requires_confirmation, 1);
        assert_eq!(rec.enabled, 1);

        let fetched = get_server_by_id(&db, rec.id).await.unwrap().unwrap();
        assert_eq!(fetched.endpoint_url, "https://mcp.example/mcp");
        assert!(get_server_by_id(&db, 9999).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn owner_and_system_listing() {
        let (db, path) = fresh_db();
        seed_user(&path, "alice", "user");
        add_server(&db, Some("alice".to_owned()), "a", "https://x/mcp", None, true)
            .await
            .unwrap();
        add_server(&db, None, "sys", "https://y/mcp", None, false)
            .await
            .unwrap();
        let own = list_servers_for_owner(&db, "alice").await.unwrap();
        assert_eq!(own.len(), 1);
        assert_eq!(own.first().unwrap().name, "a");
        let sys = list_system_servers(&db).await.unwrap();
        assert_eq!(sys.len(), 1);
        assert_eq!(sys.first().unwrap().scope(), "system");
        assert_eq!(sys.first().unwrap().requires_confirmation, 0);
    }

    #[tokio::test]
    async fn tools_cache_roundtrip_and_parse() {
        let (db, path) = fresh_db();
        seed_user(&path, "alice", "user");
        let rec = add_server(&db, Some("alice".to_owned()), "a", "https://x/mcp", None, true)
            .await
            .unwrap();
        // 既定は空配列。
        assert!(parse_tools_cache(&rec).is_empty());
        update_tools_cache(
            &db,
            rec.id,
            r#"[{"name":"echo","description":"repeat"},{"name":"","description":"skip-empty-ok"},{"noname":true}]"#,
        )
        .await
        .unwrap();
        let fresh = get_server_by_id(&db, rec.id).await.unwrap().unwrap();
        assert!(fresh.tools_cache_updated.is_some());
        let tools = parse_tools_cache(&fresh);
        // 3 要素とも object なので保持（name 空も description 欠落も toSafeView は落とさない）。
        assert_eq!(tools.len(), 3);
        assert_eq!(tools.first().unwrap().name, "echo");
        assert_eq!(tools.first().unwrap().description, "repeat");
        // description 欠落 → 空文字。
        assert_eq!(tools.get(2).unwrap().description, "");
        // 非配列 JSON は空。
        let mut bad = fresh;
        bad.tools_cache = "{\"not\":\"array\"}".to_owned();
        assert!(parse_tools_cache(&bad).is_empty());
    }

    #[tokio::test]
    async fn set_enabled_and_delete_permissions() {
        let (db, path) = fresh_db();
        seed_user(&path, "alice", "user");
        seed_user(&path, "bob", "user");
        let a = add_server(&db, Some("alice".to_owned()), "a", "https://x/mcp", None, true)
            .await
            .unwrap();
        let sys = add_server(&db, None, "sys", "https://y/mcp", None, true)
            .await
            .unwrap();

        set_enabled(&db, a.id, false).await.unwrap();
        assert_eq!(get_server_by_id(&db, a.id).await.unwrap().unwrap().enabled, 0);

        // 他人は本人所有を削除できない。
        assert!(!delete_server(&db, a.id, "bob", false).await.unwrap());
        // 非 admin はシステム登録を削除できない。
        assert!(!delete_server(&db, sys.id, "alice", false).await.unwrap());
        // admin はシステム登録を削除できる。
        assert!(delete_server(&db, sys.id, "carol", true).await.unwrap());
        // 本人は本人所有を削除できる。
        assert!(delete_server(&db, a.id, "alice", false).await.unwrap());
        // 不在 id は false。
        assert!(!delete_server(&db, a.id, "alice", false).await.unwrap());
    }

    #[tokio::test]
    async fn is_admin_and_lookup() {
        let (db, path) = fresh_db();
        seed_user(&path, "alice", "user");
        seed_user(&path, "root", "admin");
        assert!(!is_admin(&db, "alice").await.unwrap());
        assert!(is_admin(&db, "root").await.unwrap());
        assert!(!is_admin(&db, "ghost").await.unwrap());
        assert_eq!(lookup_user_admin(&db, "alice").await.unwrap(), Some(false));
        assert_eq!(lookup_user_admin(&db, "root").await.unwrap(), Some(true));
        assert_eq!(lookup_user_admin(&db, "ghost").await.unwrap(), None);
    }

    #[test]
    fn can_manage_logic() {
        let owned = McpServerRecord {
            id: 1,
            user_id: Some("alice".to_owned()),
            name: "n".to_owned(),
            endpoint_url: "u".to_owned(),
            auth_credential_encrypted: None,
            auth_credential_iv: None,
            auth_credential_tag: None,
            tools_cache: "[]".to_owned(),
            tools_cache_updated: None,
            requires_confirmation: 1,
            enabled: 1,
            created_at: "now".to_owned(),
        };
        let mut system = owned.clone();
        system.user_id = None;
        // 本人所有: 本人のみ true（admin かどうか無関係）。
        assert!(can_manage(&owned, "alice", false));
        assert!(!can_manage(&owned, "bob", true));
        // システム登録: admin のみ true。
        assert!(can_manage(&system, "anyone", true));
        assert!(!can_manage(&system, "anyone", false));
    }

    #[tokio::test]
    async fn bot_ids_for_server() {
        let (db, path) = fresh_db();
        seed_user(&path, "alice", "user");
        seed_user(&path, "carol", "user");
        let a = add_server(&db, Some("alice".to_owned()), "a", "https://x/mcp", None, true)
            .await
            .unwrap();
        raw(&path)
            .execute(
                "INSERT INTO bots (id, user_id, name) VALUES ('b1','alice','b1'),('b2','alice','b2')",
                [],
            )
            .unwrap();
        // DISTINCT を検証: b1 が 2 owner から許可されても bot_id は 1 回だけ返す。
        raw(&path)
            .execute(
                "INSERT INTO bot_mcp_access (bot_id, owner_id, mcp_server_id) \
                 VALUES ('b1','alice',?1),('b2','alice',?1),('b1','carol',?1)",
                params![a.id],
            )
            .unwrap();
        let mut ids = list_bot_ids_for_server(&db, a.id).await.unwrap();
        ids.sort();
        assert_eq!(ids, vec!["b1".to_owned(), "b2".to_owned()]);
    }
}
