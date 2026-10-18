//! 他クレートの HTTP テスト用ヘルパ（feature `test-support` のときだけ公開）。
//!
//! 個人データのルートは `botId` 必須（[`crate::resolve_scope`]）なので、テストでは
//! 利用者の Bot 行を用意し、リクエストに `botId` が無ければ補ってから送る。

use axum::extract::Request;
use axum::http::Uri;
use axum::middleware::{self, Next};
use axum::Router;
use rusqlite::params;
use yuuka_db::map_sqlite;

use crate::state::Db;

/// テスト用 Bot の既定 ID。
pub const TEST_BOT_ID: &str = "test_bot";

/// `owner` が持つ Bot 行を `bots` に入れる（既にあれば何もしない）。
///
/// # Panics
/// 書き込みに失敗したとき（テスト専用）。
#[allow(clippy::expect_used)]
pub async fn seed_bot(db: &Db, bot_id: &str, owner: &str) {
    let bot_id = bot_id.to_owned();
    let owner = owner.to_owned();
    db.writer
        .transaction(move |tx| {
            // 最小 DDL で DB を作るテストもあるため、認可クエリが引く 2 表を無ければ作る。
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS bots (\
                   id TEXT PRIMARY KEY, user_id TEXT NOT NULL, name TEXT NOT NULL);\
                 CREATE TABLE IF NOT EXISTS bot_shares (\
                   id INTEGER PRIMARY KEY AUTOINCREMENT, bot_id TEXT NOT NULL,\
                   shared_user_id TEXT NOT NULL, status TEXT NOT NULL);",
            )
            .map_err(map_sqlite)?;
            // マイグレーション済みの DB では bots.user_id → users の外部キーがあるので持ち主も入れる。
            let has_users: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'users')",
                    [],
                    |row| row.get(0),
                )
                .map_err(map_sqlite)?;
            if has_users {
                tx.execute(
                    "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt) \
                     VALUES (?1, ?1, '', '')",
                    params![owner],
                )
                .map_err(map_sqlite)?;
            }
            tx.execute(
                "INSERT OR IGNORE INTO bots (id, user_id, name) VALUES (?1, ?2, ?1)",
                params![bot_id, owner],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
        .expect("seed bot");
}

/// クエリに `botId` が無いリクエストへ `botId=<bot_id>` を足すレイヤを掛ける。
///
/// JSON body の `botId` は [`crate::ScopedJson`] が body → query の順に読むため、body に
/// `botId` を書いたテストはそちらが優先される。
pub fn with_bot_query(router: Router, bot_id: &'static str) -> Router {
    router.layer(middleware::from_fn(
        move |mut req: Request, next: Next| async move {
            add_bot_query(&mut req, bot_id);
            next.run(req).await
        },
    ))
}

fn add_bot_query(req: &mut Request, bot_id: &str) {
    let uri = req.uri();
    let query = uri.query().unwrap_or_default();
    if query.split('&').any(|kv| kv.starts_with("botId=")) {
        return;
    }
    let sep = if query.is_empty() { "" } else { "&" };
    let path_and_query = format!("{}?{query}{sep}botId={bot_id}", uri.path());
    let mut parts = uri.clone().into_parts();
    parts.path_and_query = path_and_query.parse().ok();
    if let Ok(new_uri) = Uri::from_parts(parts) {
        *req.uri_mut() = new_uri;
    }
}
