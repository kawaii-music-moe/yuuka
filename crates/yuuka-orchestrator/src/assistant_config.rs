//! `GET /api/bots/assistant-config` の集計リポジトリ（Node `personaRepo`/`mcpRepo` の読み取り系パリティ）。
//!
//! 汎用モード設定タブの一括取得で使うペルソナ一覧（own/public）と Bot へ許可された MCP サーバー一覧を引く。
//! いずれも読み取り専用。ハンドラ本体（認可 + 応答組立）は [`crate::bot_attr_routes`] にある。

use rusqlite::params;
use yuuka_core::DbError;
use yuuka_db::map_sqlite;
use yuuka_web::Db;

/// owner 本人のペルソナ（Node `listPersonasForUser`・`ORDER BY updated_at DESC`）。`(id, name)` を返す。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_own_personas(db: &Db, owner_id: &str) -> Result<Vec<(i64, String)>, DbError> {
    let owner_id = owner_id.to_owned();
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, name FROM personas WHERE owner_id = ?1 ORDER BY updated_at DESC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![owner_id], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(map_sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(map_sqlite)?);
            }
            Ok(out)
        })
        .await
}

/// 公開ペルソナ一覧（Node `listPublicPersonas`・owner をまたぐ公開読み取り・`owner_username` は users JOIN・
/// 不在は「不明」）。`(id, name, owner_username)` を `updated_at DESC` で返す。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_public_personas(db: &Db) -> Result<Vec<(i64, String, String)>, DbError> {
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT p.id, p.name, COALESCE(u.username, '不明') AS owner_username \
                     FROM personas p LEFT JOIN users u ON u.discord_id = p.owner_id \
                     WHERE p.is_public = 1 ORDER BY p.updated_at DESC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })
                .map_err(map_sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(map_sqlite)?);
            }
            Ok(out)
        })
        .await
}

/// Bot へ許可された MCP サーバー（Node `listServersGrantedToBot`＝`bot_mcp_access` 経由 + システムレベル
/// `user_id IS NULL`・`created_at ASC`）。表示用の最小ビュー。
#[derive(Debug, Clone)]
pub struct GrantedServer {
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    pub system: bool,
}

/// Bot に利用許可された MCP サーバー一覧（許可分 UNION システムレベル）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_servers_granted_to_bot(
    db: &Db,
    bot_id: &str,
) -> Result<Vec<GrantedServer>, DbError> {
    let bot_id = bot_id.to_owned();
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT s.id, s.name, s.enabled, s.user_id FROM mcp_servers s \
                     JOIN bot_mcp_access a ON a.mcp_server_id = s.id AND a.bot_id = ?1 \
                     UNION SELECT s.id, s.name, s.enabled, s.user_id FROM mcp_servers s \
                     WHERE s.user_id IS NULL ORDER BY id ASC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![bot_id], |r| {
                    let user_id: Option<String> = r.get(3)?;
                    Ok(GrantedServer {
                        id: r.get::<_, i64>(0)?,
                        name: r.get::<_, String>(1)?,
                        enabled: r.get::<_, i64>(2)? == 1,
                        system: user_id.is_none(),
                    })
                })
                .map_err(map_sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(map_sqlite)?);
            }
            Ok(out)
        })
        .await
}
