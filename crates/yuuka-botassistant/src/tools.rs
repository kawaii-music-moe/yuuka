//! guild-assistant ノートツール（汎用モードでのみ露出・Node `botPersonalNoteFunctions` /
//! `botGuildMemoryFunctions` パリティ）。
//!
//! **露出**: [`Tool::exposure`] を上書きして `guild_assistant` カタログ・能力 `memory` に分類する
//! （秘書経路では露出しない）。共有ノート（Guild）は guild スコープ必須（`requires_guild`）。
//! [`ToolExposure::is_visible`] が `process_guild` の `ctx.mode = GuildAssistant` で自動選別する。
//!
//! My=個人ノート（bot × ユーザー）/ Guild=共有ノート（bot × ギルド）。各 get/set/append の 6 本。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use yuuka_core::tool::{FunctionDeclaration, ToolExposure};
use yuuka_core::{Tool, ToolContext, ToolError, ToolName, ToolOutcome};
use yuuka_web::Db;

use crate::repo::{self, BOT_NOTE_MAX_LENGTH};

/// ノート対象（個人＝bot×ユーザー / 共有＝bot×ギルド）。
#[derive(Clone, Copy)]
enum Target {
    My,
    Guild,
}

/// ノート操作。
#[derive(Clone, Copy)]
enum Op {
    Get,
    Set,
    Append,
}

impl Target {
    fn label(self) -> &'static str {
        match self {
            Target::My => "個人ノート",
            Target::Guild => "共有ノート",
        }
    }
}

/// guild-assistant ノートツール（対象 × 操作でパラメタ化）。
struct NoteTool {
    name: ToolName,
    db: Db,
    target: Target,
    op: Op,
}

impl NoteTool {
    fn make(
        name: &'static str,
        db: Db,
        target: Target,
        op: Op,
    ) -> Result<Arc<dyn Tool>, ToolError> {
        Ok(Arc::new(Self {
            name: ToolName::checked(name.to_owned())?,
            db,
            target,
            op,
        }))
    }

    /// 対象キーを解決する（My=user_id・Guild=guild_id）。Guild で guild スコープが無ければ Node
    /// `requireGuild` 相当のエラー文言を返す（露出ゲートで通常は到達しないが防御的に確認）。
    fn key<'a>(&self, ctx: &'a ToolContext) -> Result<&'a str, &'static str> {
        match self.target {
            Target::My => Ok(ctx.user_id.as_str()),
            Target::Guild => ctx
                .guild_id
                .as_ref()
                .map(yuuka_core::GuildId::as_str)
                .ok_or("この機能はサーバー（ギルド）内の会話でのみ利用できます。"),
        }
    }

    async fn get(&self, bot: &str, key: &str) -> Result<String, ToolError> {
        match self.target {
            Target::My => repo::get_my(&self.db, bot, key).await,
            Target::Guild => repo::get_guild(&self.db, bot, key).await,
        }
        .map_err(exec_err)
    }

    async fn set(&self, bot: &str, key: &str, content: String) -> Result<(), ToolError> {
        match self.target {
            Target::My => repo::set_my(&self.db, bot, key, content).await,
            Target::Guild => repo::set_guild(&self.db, bot, key, content).await,
        }
        .map_err(exec_err)
    }
}

#[async_trait]
impl Tool for NoteTool {
    fn exposure(&self) -> ToolExposure {
        ToolExposure {
            capability: Some("memory"),
            secretary: false,
            guild_assistant: true,
            requires_guild: matches!(self.target, Target::Guild),
        }
    }

    fn declaration(&self) -> FunctionDeclaration {
        let label = self.target.label();
        let (description, params) = match self.op {
            Op::Get => (
                format!("{label}（自由記述のメモ帳）の現在の全文を読む。書き換え前の確認に使う。"),
                json!({ "type": "object", "properties": {} }),
            ),
            Op::Set => (
                format!(
                    "{label}の全文を書き換える（全置換）。{}文字まで。誤消し防止のため先に get で現在の中身を確認する。",
                    thousands(BOT_NOTE_MAX_LENGTH)
                ),
                json!({
                    "type": "object",
                    "properties": {
                        "content": { "type": "string", "description": format!("書き換え後の{label}の全文") }
                    },
                    "required": ["content"]
                }),
            ),
            Op::Append => (
                format!("{label}に1行追記する。覚えておくべき事実を残す時に呼ぶ。整理し直す時は set を使う。"),
                json!({
                    "type": "object",
                    "properties": {
                        "content": { "type": "string", "description": "追記する内容（1行）" }
                    },
                    "required": ["content"]
                }),
            ),
        };
        FunctionDeclaration {
            name: self.name.clone(),
            description,
            parameters_json_schema: params,
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let bot = ctx.bot_id.as_str();
        let key = match self.key(ctx) {
            Ok(k) => k,
            Err(msg) => return Ok(fail(msg)),
        };
        let label = self.target.label();

        match self.op {
            Op::Get => {
                let content = self.get(bot, key).await?;
                let length = content.chars().count();
                Ok(ToolOutcome::from_payload(json!({
                    "success": true,
                    "content": content,
                    "length": length,
                    "max_length": BOT_NOTE_MAX_LENGTH,
                })))
            }
            Op::Set => {
                // Node: String(args.content ?? "")（trim せず・空許容）。
                let content = args.get("content").and_then(Value::as_str).unwrap_or("");
                let length = content.chars().count();
                if length > BOT_NOTE_MAX_LENGTH {
                    return Ok(fail(over_limit(label, length)));
                }
                self.set(bot, key, content.to_owned()).await?;
                Ok(ToolOutcome::from_payload(json!({
                    "success": true,
                    "message": format!("{label}を更新しました📝"),
                    "total_length": length,
                })))
            }
            Op::Append => {
                let trimmed = args
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_owned();
                if trimmed.is_empty() {
                    return Ok(fail("追記する内容が空です。"));
                }
                let current = self.get(bot, key).await?;
                let next = if current.is_empty() {
                    trimmed
                } else {
                    format!("{current}\n{trimmed}")
                };
                let next_len = next.chars().count();
                if next_len > BOT_NOTE_MAX_LENGTH {
                    return Ok(fail(over_limit(label, next_len)));
                }
                self.set(bot, key, next).await?;
                Ok(ToolOutcome::from_payload(json!({
                    "success": true,
                    "message": format!("{label}に追記しました📝"),
                    "total_length": next_len,
                    "max_length": BOT_NOTE_MAX_LENGTH,
                })))
            }
        }
    }
}

/// このクレートが公開する guild-assistant ツール一式（6 本）。
///
/// # Errors
/// ツール名が Gemini 制約に反する場合 [`ToolError`]（コンパイル時定数なので通常発生しない）。
pub fn tools(db: Db) -> Result<Vec<Arc<dyn Tool>>, ToolError> {
    Ok(vec![
        NoteTool::make("getMyNote", db.clone(), Target::My, Op::Get)?,
        NoteTool::make("setMyNote", db.clone(), Target::My, Op::Set)?,
        NoteTool::make("appendMyNote", db.clone(), Target::My, Op::Append)?,
        NoteTool::make("getGuildNote", db.clone(), Target::Guild, Op::Get)?,
        NoteTool::make("setGuildNote", db.clone(), Target::Guild, Op::Set)?,
        NoteTool::make("appendGuildNote", db, Target::Guild, Op::Append)?,
    ])
}

fn fail(message: impl Into<String>) -> ToolOutcome {
    ToolOutcome::from_payload(json!({ "success": false, "message": message.into() }))
}

fn exec_err(e: yuuka_core::DbError) -> ToolError {
    ToolError::Execution(e.to_string())
}

/// 上限超過メッセージ（Node `validateLength`）。
fn over_limit(label: &str, length: usize) -> String {
    format!(
        "{label}は{}文字以内です（現在: {}文字）",
        thousands(BOT_NOTE_MAX_LENGTH),
        thousands(length)
    )
}

/// 3 桁区切り（Node `Number.toLocaleString()`）。
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i != 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use rusqlite::Connection;
    use yuuka_core::tool::TurnMode;
    use yuuka_core::{BotId, CapabilitySet, GuildId, UserId};

    use super::*;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    // bot_context_notes / bot_guild_notes を FK 無しで先に作る（V17 の bots FK を避ける）。
    const NOTES_DDL: &str = "CREATE TABLE bot_context_notes (\
        bot_id TEXT NOT NULL, user_id TEXT NOT NULL, content TEXT NOT NULL DEFAULT '', \
        updated_at TEXT NOT NULL DEFAULT (datetime('now','localtime')), PRIMARY KEY (bot_id, user_id));\
        CREATE TABLE bot_guild_notes (\
        bot_id TEXT NOT NULL, guild_id TEXT NOT NULL, content TEXT NOT NULL DEFAULT '', \
        updated_at TEXT NOT NULL DEFAULT (datetime('now','localtime')), PRIMARY KEY (bot_id, guild_id));";

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_botassist_{}_{seq}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(NOTES_DDL).unwrap();
        }
        Db::open(&path).unwrap()
    }

    fn guild_ctx() -> ToolContext {
        let mut c = ToolContext::new(BotId::system_default(), UserId::new("u1"));
        c.mode = TurnMode::GuildAssistant;
        c.capabilities = CapabilitySet::from_granted(vec!["memory".to_owned()]);
        c.guild_id = Some(GuildId::new("g1"));
        c
    }

    fn find<'a>(tools: &'a [Arc<dyn Tool>], name: &str) -> &'a Arc<dyn Tool> {
        tools
            .iter()
            .find(|t| t.declaration().name.as_str() == name)
            .unwrap()
    }

    #[test]
    fn exposure_is_guild_assistant_memory_only() {
        let tools = tools(seed_db()).unwrap();
        assert_eq!(tools.len(), 6);
        let get_my = find(&tools, "getMyNote");
        let get_guild = find(&tools, "getGuildNote");

        // 秘書経路では一切露出しない。
        let mut sec = ToolContext::new(BotId::system_default(), UserId::new("u1"));
        sec.capabilities = CapabilitySet::from_granted(vec!["secretary".to_owned()]);
        assert!(!get_my.exposure().is_visible(&sec));
        assert!(!get_guild.exposure().is_visible(&sec));

        // 汎用モード + memory 能力で個人ノートは可視。共有ノートは guild 必須。
        let g = guild_ctx();
        assert!(get_my.exposure().is_visible(&g));
        assert!(get_guild.exposure().is_visible(&g));
        let mut no_guild = guild_ctx();
        no_guild.guild_id = None;
        assert!(get_my.exposure().is_visible(&no_guild)); // 個人は guild 不要
        assert!(!get_guild.exposure().is_visible(&no_guild)); // 共有は guild 必須

        // memory 能力が無ければ不可視。
        let mut no_mem = guild_ctx();
        no_mem.capabilities = CapabilitySet::default();
        assert!(!get_my.exposure().is_visible(&no_mem));
    }

    #[tokio::test]
    async fn my_note_get_set_append() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let get = find(&tools, "getMyNote");
        let set = find(&tools, "setMyNote");
        let append = find(&tools, "appendMyNote");
        let ctx = guild_ctx();

        let out = get.call(&ctx, json!({})).await.unwrap();
        assert_eq!(out.payload["content"], "");

        set.call(&ctx, json!({"content": "好きな色は青"}))
            .await
            .unwrap();
        let out = get.call(&ctx, json!({})).await.unwrap();
        assert_eq!(out.payload["content"], "好きな色は青");

        append.call(&ctx, json!({"content": "犬派"})).await.unwrap();
        let out = get.call(&ctx, json!({})).await.unwrap();
        assert_eq!(out.payload["content"], "好きな色は青\n犬派");

        // 空追記 → fail。
        let out = append.call(&ctx, json!({"content": "  "})).await.unwrap();
        assert_eq!(out.payload["success"], false);
        // 別ユーザーには漏れない。
        let mut other = guild_ctx();
        other.user_id = UserId::new("u2");
        let out = get.call(&other, json!({})).await.unwrap();
        assert_eq!(out.payload["content"], "");
    }

    #[tokio::test]
    async fn guild_note_requires_guild_and_is_shared() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let set = find(&tools, "setGuildNote");
        let get = find(&tools, "getGuildNote");

        // guild スコープ無しは fail（防御的チェック）。
        let mut no_guild = guild_ctx();
        no_guild.guild_id = None;
        let out = set.call(&no_guild, json!({"content": "x"})).await.unwrap();
        assert_eq!(out.payload["success"], false);

        // guild 内で設定 → 同ギルドの別ユーザーからも見える（共有）。
        set.call(&guild_ctx(), json!({"content": "定例は毎週月曜"}))
            .await
            .unwrap();
        let mut other_user = guild_ctx();
        other_user.user_id = UserId::new("u2");
        let out = get.call(&other_user, json!({})).await.unwrap();
        assert_eq!(out.payload["content"], "定例は毎週月曜");
        // 別ギルドには漏れない。
        let mut other_guild = guild_ctx();
        other_guild.guild_id = Some(GuildId::new("g2"));
        let out = get.call(&other_guild, json!({})).await.unwrap();
        assert_eq!(out.payload["content"], "");
    }
}
