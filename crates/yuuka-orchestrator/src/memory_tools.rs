//! 会話記憶を利用者の頼みで忘れる Native ツール `forgetMemory`（docs/design/memory_retention.md §4-E）。
//!
//! 消せるのは **本人 × Bot の kept** だけ（他人の記憶・評価待ちの候補は対象外）。指定は `id`
//! （`GET /api/memories` や前回の結果で分かっている番号）か `query`（記憶の内容の一部）。`query` で
//! 複数が当たれば消さずに候補を返し、どれかを `id` で指し直してもらう（曖昧なまま消さない）。
//! DB から消えたときだけ RAM 索引（[`SynapseEngine::forget`]）からも除く。
//!
//! 露出: `memory` 能力・秘書と汎用モードの両経路（汎用モードのギルド記憶も発話者本人の分だけ）。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use yuuka_core::tool::{FunctionDeclaration, ToolExposure};
use yuuka_core::{Tool, ToolContext, ToolError, ToolName, ToolOutcome};
use yuuka_synapse::SynapseEngine;
use yuuka_web::Db;

use crate::synapse_repo::{self, MemoryRow};

/// `query` で複数が当たったときに返す候補の上限。
const MATCH_LIMIT: usize = 5;

/// このモジュールが公開するツール（forgetMemory）。
///
/// # Errors
/// ツール名が Gemini 制約に反する場合 [`ToolError`]（定数名なので通常発生しない）。
pub fn tools(db: Db, engine: Arc<Mutex<SynapseEngine>>) -> Result<Vec<Arc<dyn Tool>>, ToolError> {
    Ok(vec![Arc::new(ForgetMemoryTool {
        name: ToolName::checked("forgetMemory".to_owned())?,
        db,
        engine,
    })])
}

struct ForgetMemoryTool {
    name: ToolName,
    db: Db,
    engine: Arc<Mutex<SynapseEngine>>,
}

fn arg_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// 数値または数値文字列を i64 へ（Gemini は number を f64 で送りうる）。
fn arg_i64(args: &Value, key: &str) -> Option<i64> {
    let v = args.get(key)?;
    if let Some(i) = v.as_i64() {
        return Some(i);
    }
    if let Some(f) = v.as_f64().filter(|f| f.is_finite()) {
        return Some(f.trunc() as i64);
    }
    v.as_str().and_then(|s| s.trim().parse::<i64>().ok())
}

fn fail(message: impl Into<String>) -> ToolOutcome {
    ToolOutcome::from_payload(json!({ "success": false, "message": message.into() }))
}

fn exec_err(e: yuuka_core::DbError) -> ToolError {
    ToolError::Execution(e.to_string())
}

fn brief(row: &MemoryRow) -> Value {
    json!({ "id": row.id, "content": row.content, "kind": row.kind })
}

#[async_trait]
impl Tool for ForgetMemoryTool {
    fn exposure(&self) -> ToolExposure {
        ToolExposure {
            capability: Some("memory"),
            secretary: true,
            guild_assistant: true,
            requires_guild: false,
        }
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "ユーザーについて覚えている記憶（長期記憶）を 1 件忘れる。\n\
                ・ユーザーが「〇〇のことは忘れて」「それは覚えないで」と頼んだときに使う。\n\
                ・query に記憶の内容の一部（例: 'カレー'、'金曜'）を渡す。複数当たったら候補が返るので、\
                ユーザーに確認してから id で指定し直す。\n\
                ・消せるのはユーザー本人の記憶だけ。会話ログは消えない。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "number", "description": "忘れる記憶の番号（候補一覧で分かっているとき）" },
                    "query": { "type": "string", "description": "忘れる記憶の内容の一部（部分一致）" }
                }
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let uid = ctx.user_id.as_str();
        let bid = ctx.bot_id.as_str();

        let target: MemoryRow = if let Some(id) = arg_i64(&args, "id") {
            // id 指定でも本人 × Bot の kept であることを先に確かめる（他人の番号を渡されても消さない）。
            let rows = synapse_repo::list_kept(&self.db, uid, bid)
                .await
                .map_err(exec_err)?;
            match rows.into_iter().find(|r| r.id == id) {
                Some(row) => row,
                None => return Ok(fail(format!("番号 {id} の記憶は見つかりませんでした。"))),
            }
        } else if let Some(query) = arg_str(&args, "query") {
            let mut found =
                synapse_repo::find_kept_by_content(&self.db, uid, bid, &query, MATCH_LIMIT + 1)
                    .await
                    .map_err(exec_err)?;
            match found.len() {
                0 => {
                    return Ok(fail(format!(
                        "「{query}」に当たる記憶は見つかりませんでした。"
                    )))
                }
                1 => found.remove(0),
                n => {
                    let truncated = n > MATCH_LIMIT;
                    found.truncate(MATCH_LIMIT);
                    return Ok(ToolOutcome::from_payload(json!({
                        "success": false,
                        "message": format!(
                            "「{query}」に当たる記憶が複数あります。どれを忘れるかユーザーに確かめ、id で指定し直してください。{}",
                            if truncated { "（多すぎるため一部だけ示しています。query をもっと具体的に）" } else { "" }
                        ),
                        "matches": found.iter().map(brief).collect::<Vec<_>>(),
                    })));
                }
            }
        } else {
            return Ok(fail("忘れる記憶を id か query で指定してください。"));
        };

        let deleted = synapse_repo::delete_kept_owned(&self.db, uid, bid, target.id)
            .await
            .map_err(exec_err)?;
        if !deleted {
            return Ok(fail(format!(
                "番号 {} の記憶は見つかりませんでした。",
                target.id
            )));
        }
        self.engine.lock().await.forget(target.id);
        Ok(ToolOutcome::from_payload(json!({
            "success": true,
            "message": format!("「{}」を忘れました。", target.content),
            "forgotten": brief(&target),
        })))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use yuuka_core::{BotId, UserId};
    use yuuka_web::test_support::TEST_BOT_ID;

    use super::*;
    use crate::synapse_repo::{InsertArgs, STATUS_CANDIDATE, STATUS_KEPT};

    async fn setup() -> (Db, Arc<Mutex<SynapseEngine>>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        drop(rusqlite::Connection::open(&path).unwrap());
        let db = Db::open(Path::new(&path)).unwrap();
        let (engine, _) = SynapseEngine::boot(&path, yuuka_synapse::DEFAULT_DIM);
        (db, Arc::new(Mutex::new(engine)), dir)
    }

    async fn insert(db: &Db, user: &str, content: &str, status: &'static str) -> i64 {
        synapse_repo::insert_synapse(
            db,
            InsertArgs {
                user_id: user.into(),
                bot_id: TEST_BOT_ID.into(),
                guild_id: None,
                content: content.into(),
                topic_id: None,
                source_msg_id: None,
                ctx_tod: None,
                ctx_dow: None,
                status,
                content_hash: None,
            },
        )
        .await
        .unwrap()
    }

    fn ctx(user: &str) -> ToolContext {
        let mut c = ToolContext::new(BotId::new(TEST_BOT_ID), UserId::new(user));
        c.capabilities = yuuka_core::CapabilitySet::from_granted(vec!["memory".to_owned()]);
        c
    }

    fn tool(db: &Db, engine: &Arc<Mutex<SynapseEngine>>) -> Arc<dyn Tool> {
        tools(db.clone(), engine.clone()).unwrap().remove(0)
    }

    async fn count_rows(db: &Db) -> i64 {
        db.read
            .read(|conn| {
                conn.query_row("SELECT count(*) FROM synapses", [], |r| r.get(0))
                    .map_err(yuuka_db::map_sqlite)
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn exposure_is_memory_capability_on_both_routes() {
        let (db, engine, _dir) = setup().await;
        let t = tool(&db, &engine);
        let exp = t.exposure();
        assert_eq!(exp.capability, Some("memory"));
        assert!(exp.secretary && exp.guild_assistant && !exp.requires_guild);
        assert!(exp.is_visible(&ctx("u1")));
        assert_eq!(t.declaration().name.to_string(), "forgetMemory");
    }

    /// query が 1 件に当たれば消え、索引からも除かれる。
    #[tokio::test]
    async fn forgets_single_match_by_query_and_removes_from_index() {
        let (db, engine, _dir) = setup().await;
        let id = insert(&db, "u1", "利用者はカレーが好き", STATUS_KEPT).await;
        insert(&db, "u1", "利用者は金曜は在宅", STATUS_KEPT).await;
        {
            let mut eng = engine.lock().await;
            eng.index(
                yuuka_synapse::Scope {
                    user_id: "u1".into(),
                    bot_id: TEST_BOT_ID.into(),
                    guild_id: None,
                },
                id,
                None,
                "利用者はカレーが好き".to_owned(),
                yuuka_synapse::FormationContext::default(),
            );
        }
        let t = tool(&db, &engine);
        let out = t
            .call(&ctx("u1"), json!({ "query": "カレー" }))
            .await
            .unwrap();
        let payload = out.payload;
        assert_eq!(payload["success"], true);
        assert_eq!(payload["forgotten"]["id"], id);
        assert_eq!(count_rows(&db).await, 1);
        assert_eq!(engine.lock().await.total(), 0);
    }

    /// 複数当たりは消さずに候補を返す。0 件・引数無しも消さない。
    #[tokio::test]
    async fn ambiguous_or_missing_queries_do_not_delete() {
        let (db, engine, _dir) = setup().await;
        insert(&db, "u1", "利用者はカレーが好き", STATUS_KEPT).await;
        insert(&db, "u1", "利用者はカレー屋で働いている", STATUS_KEPT).await;
        let t = tool(&db, &engine);

        let out = t
            .call(&ctx("u1"), json!({ "query": "カレー" }))
            .await
            .unwrap();
        let payload = out.payload;
        assert_eq!(payload["success"], false);
        assert_eq!(payload["matches"].as_array().unwrap().len(), 2);

        let out = t
            .call(&ctx("u1"), json!({ "query": "寿司" }))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);

        let out = t.call(&ctx("u1"), json!({})).await.unwrap();
        assert_eq!(out.payload["success"], false);
        assert_eq!(count_rows(&db).await, 2);
    }

    /// id 指定でも他人の記憶・候補は消えない。本人の kept は消える。
    #[tokio::test]
    async fn id_is_scoped_to_caller_and_kept_only() {
        let (db, engine, _dir) = setup().await;
        let mine = insert(&db, "u1", "利用者はカレーが好き", STATUS_KEPT).await;
        let pending = insert(&db, "u1", "今日は疲れた", STATUS_CANDIDATE).await;
        let theirs = insert(&db, "u2", "他人の記憶", STATUS_KEPT).await;
        let t = tool(&db, &engine);

        for id in [theirs, pending] {
            let out = t.call(&ctx("u1"), json!({ "id": id })).await.unwrap();
            assert_eq!(out.payload["success"], false, "id={id} は消えない");
        }
        assert_eq!(count_rows(&db).await, 3);

        // Gemini が number を f64 で送っても受ける。
        #[allow(clippy::cast_precision_loss)]
        let as_f64 = mine as f64;
        let out = t.call(&ctx("u1"), json!({ "id": as_f64 })).await.unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(count_rows(&db).await, 2);
    }
}
