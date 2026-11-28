//! Web の受信箱（`web_notifications`）の読み出しと既読（docs/design/optional_discord.md §6.1）。
//!
//! 保存は yuuka-services の `DefaultTargetNotifier`（通知先が Web の利用者）が行う。

use axum::extract::{Query, State};
use axum::response::Response;
use axum::Json;
use rusqlite::{params, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::json;
use yuuka_db::map_sqlite;
use yuuka_web::{ApiError, AppState, AuthenticatedUser};

use crate::routes::json_ok;

/// 一度に返す件数の上限。
const MAX_LIMIT: u32 = 200;
/// 一度に既読にできる ID の上限。
const MAX_IDS: usize = 500;

/// 受信箱の 1 件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WebNotification {
    pub id: i64,
    pub bot_id: Option<String>,
    pub content: String,
    pub created_at: String,
    pub read_at: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct ListQuery {
    limit: Option<u32>,
}

/// `GET /api/notifications?limit=`：新しい順の一覧と未読数。
pub(crate) async fn list(
    user: AuthenticatedUser,
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Response, ApiError> {
    let uid = user.0.user_id.clone();
    let limit = query.limit.unwrap_or(50).clamp(1, MAX_LIMIT);
    let (items, unread) = state
        .db
        .read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, bot_id, content, created_at, read_at FROM web_notifications \
                     WHERE user_id = ?1 ORDER BY id DESC LIMIT ?2",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![uid, limit], |r| {
                    Ok(WebNotification {
                        id: r.get(0)?,
                        bot_id: r.get(1)?,
                        content: r.get(2)?,
                        created_at: r.get(3)?,
                        read_at: r.get(4)?,
                    })
                })
                .map_err(map_sqlite)?;
            let items = rows.collect::<Result<Vec<_>, _>>().map_err(map_sqlite)?;
            let unread: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM web_notifications WHERE user_id = ?1 AND read_at IS NULL",
                    params![uid],
                    |r| r.get(0),
                )
                .map_err(map_sqlite)?;
            Ok((items, unread))
        })
        .await?;
    Ok(json_ok(
        json!({ "success": true, "notifications": items, "unread": unread }),
    ))
}

#[derive(Deserialize)]
pub(crate) struct ReadBody {
    /// 既読にする ID。省略するとすべて既読にする。
    ids: Option<Vec<i64>>,
}

/// `POST /api/notifications/read`：既読にする（本人の通知だけ）。
pub(crate) async fn mark_read(
    user: AuthenticatedUser,
    State(state): State<AppState>,
    Json(body): Json<ReadBody>,
) -> Result<Response, ApiError> {
    let uid = user.0.user_id.clone();
    let ids: Option<Vec<i64>> = body.ids.map(|ids| ids.into_iter().take(MAX_IDS).collect());
    let updated = state
        .db
        .writer
        .transaction(move |tx| {
            let n = match ids {
                None => tx
                    .execute(
                        "UPDATE web_notifications SET read_at = datetime('now', 'localtime') \
                         WHERE user_id = ?1 AND read_at IS NULL",
                        params![uid],
                    )
                    .map_err(map_sqlite)?,
                Some(ids) if ids.is_empty() => 0,
                Some(ids) => {
                    let marks = vec!["?"; ids.len()].join(", ");
                    let sql = format!(
                        "UPDATE web_notifications SET read_at = datetime('now', 'localtime') \
                         WHERE user_id = ? AND read_at IS NULL AND id IN ({marks})"
                    );
                    let mut values: Vec<rusqlite::types::Value> =
                        vec![rusqlite::types::Value::Text(uid)];
                    values.extend(ids.into_iter().map(rusqlite::types::Value::Integer));
                    tx.execute(&sql, params_from_iter(values))
                        .map_err(map_sqlite)?
                }
            };
            Ok(n)
        })
        .await?;
    Ok(json_ok(json!({ "success": true, "updated": updated })))
}
