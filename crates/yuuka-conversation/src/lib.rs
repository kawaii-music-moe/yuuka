//! yuuka-conversation — 会話ログ要約ツール（秘書経路・Node `conversationFunctions` パリティ）。
//!
//! `summarizeConversationTopic`: SQLite に永続化された会話履歴（`message_logs`）から、キーワード/期間で
//! 過去のやり取りを時系列で取り出して LLM に要約させる。取得対象は **本人（`ctx.user_id`）の DM/秘書会話
//! のみ**（`guild_id IS NULL`・§3.12.3 プライバシー）。3 文字以上は FTS5（`message_logs_fts`）MATCH、
//! 1–2 文字は LIKE、キーワード無しは期間のみで検索する（Node `searchMessages` パリティ）。

use std::sync::Arc;

use async_trait::async_trait;
use rusqlite::types::Value as SqlValue;
use serde_json::{json, Value};
use yuuka_core::tool::FunctionDeclaration;
use yuuka_core::{DbError, Tool, ToolContext, ToolError, ToolExposure, ToolName, ToolOutcome};
use yuuka_db::map_sqlite;
use yuuka_web::Db;

/// 検索ヒット 1 件（LLM へ返す最小フィールド）。
#[derive(Debug, Clone)]
pub struct MessageHit {
    pub role: String,
    pub content: String,
    pub created_at: String,
}

/// このクレートが公開するツール（summarizeConversationTopic）。
///
/// # Errors
/// ツール名が Gemini 制約に反する場合 [`ToolError`]。
pub fn tools(db: Db) -> Result<Vec<Arc<dyn Tool>>, ToolError> {
    Ok(vec![Arc::new(SummarizeConversationTopicTool {
        name: ToolName::checked("summarizeConversationTopic".to_owned())?,
        db,
    })])
}

struct SummarizeConversationTopicTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for SummarizeConversationTopicTool {
    fn exposure(&self) -> ToolExposure {
        // 秘書経路・memory 能力（Node `moduleCatalog` の conversation.cap = "memory"）。
        ToolExposure {
            capability: Some("memory"),
            secretary: true,
            guild_assistant: false,
            requires_guild: false,
        }
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "昔の会話を話題やキーワード・期間でさがし、まとめるための会話ログを時系列順で取り出す。\n\
                ・例:「先週の旅行計画の話をまとめて」「〇〇について前に何を話したか要約して」。\n\
                ・このツール自体は要約しないので、返ってきた会話ログを読んで、あなたがユーザーの頼みに沿ってまとめて伝える。\n\
                ・該当が多い時は新しい順の上位10件だけに絞って返る。\n\
                ・取り出せるのはユーザー本人の会話だけ。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "keyword": { "type": "string", "description": "まとめたい話題を表す言葉（例: '旅行'、'引っ越し'）。省略すると期間だけでさがす。" },
                    "from": { "type": "string", "description": "さがす期間の始まりの日。形式: YYYY-MM-DD。省略可。" },
                    "to": { "type": "string", "description": "さがす期間の終わりの日。形式: YYYY-MM-DD。その日の終わりまで含む。省略可。" }
                }
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let keyword = opt_str(&args, "keyword");
        let from = opt_str(&args, "from");
        let to = opt_str(&args, "to");

        if keyword.is_none() && from.is_none() && to.is_none() {
            return Ok(ToolOutcome::from_payload(json!({
                "success": false,
                "message": "要約対象を特定するため、キーワードまたは期間（from/to）のいずれかを指定してください。",
            })));
        }

        // 11 件取得して 10 件に切り詰め、「絞り込みが発生したか」を判定（Node §3.12.2）。
        let found = search_messages(
            &self.db,
            ctx.user_id.as_str(),
            ctx.bot_id.as_str(),
            keyword.as_deref(),
            from.as_deref(),
            to.as_deref(),
            11,
        )
        .await
        .map_err(|e| ToolError::Execution(e.to_string()))?;
        let narrowed = found.len() > 10;
        let mut records: Vec<MessageHit> = found.into_iter().take(10).collect();

        if records.is_empty() {
            return Ok(ToolOutcome::from_payload(json!({
                "success": true,
                "message": "条件に一致する過去の会話は見つかりませんでした。その旨をユーザーへ伝えてください。",
                "logs": [],
            })));
        }

        // 時系列（古い順）へ並べ替えて返す。本文は 1000 文字で切り詰め。
        records.reverse();
        let logs: Vec<Value> = records
            .iter()
            .map(|r| {
                json!({
                    "role": r.role,
                    "created_at": r.created_at,
                    "content": truncate_chars(&r.content, 1000),
                })
            })
            .collect();
        let suffix = if narrowed {
            "。該当が多いため新しい順の上位10件に絞っています"
        } else {
            ""
        };
        Ok(ToolOutcome::from_payload(json!({
            "success": true,
            "message": format!(
                "{}件の会話ログを取得しました（時系列順）{}。この内容をユーザーの依頼に沿って要約して提示してください。",
                logs.len(),
                suffix
            ),
            "logs": logs,
        })))
    }
}

/// 会話履歴検索（Node `searchMessages`・本人の DM/秘書会話＝`guild_id IS NULL` のみ）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn search_messages(
    db: &Db,
    user_id: &str,
    bot_id: &str,
    keyword: Option<&str>,
    from: Option<&str>,
    to: Option<&str>,
    limit: i64,
) -> Result<Vec<MessageHit>, DbError> {
    let limit = limit.clamp(1, 100);
    let (uid, bid) = (user_id.to_owned(), bot_id.to_owned());
    let keyword = keyword
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let (from, to) = (from.map(str::to_owned), to.map(str::to_owned));
    db.read
        .read(move |conn| {
            // 期間条件を組み立てる。
            let mut period_sql = String::new();
            let mut period: Vec<SqlValue> = Vec::new();
            if let Some(f) = &from {
                period_sql.push_str(" AND m.created_at >= ?");
                period.push(SqlValue::Text(normalize_period(f, false)));
            }
            if let Some(t) = &to {
                period_sql.push_str(" AND m.created_at <= ?");
                period.push(SqlValue::Text(normalize_period(t, true)));
            }

            let (sql, binds): (String, Vec<SqlValue>) = match &keyword {
                // 3 文字以上: FTS5 MATCH。
                Some(kw) if kw.chars().count() >= 3 => {
                    let match_expr = format!("\"{}\"", kw.replace('"', "\"\""));
                    let sql = format!(
                        "SELECT m.role, m.content, m.created_at FROM message_logs_fts \
                         JOIN message_logs m ON m.id = message_logs_fts.rowid \
                         WHERE message_logs_fts MATCH ? AND m.user_id = ? AND m.bot_id = ? \
                           AND m.guild_id IS NULL{period_sql} ORDER BY m.id DESC LIMIT ?"
                    );
                    let mut binds = vec![
                        SqlValue::Text(match_expr),
                        SqlValue::Text(uid.clone()),
                        SqlValue::Text(bid.clone()),
                    ];
                    binds.extend(period.iter().cloned());
                    binds.push(SqlValue::Integer(limit));
                    (sql, binds)
                }
                // 1–2 文字: LIKE（trigram では短い部分一致が引けないため）。
                Some(kw) => {
                    let like = format!("%{}%", escape_like(kw));
                    let sql = format!(
                        "SELECT m.role, m.content, m.created_at FROM message_logs m \
                         WHERE m.user_id = ? AND m.bot_id = ? AND m.guild_id IS NULL \
                           AND m.content LIKE ? ESCAPE '\\'{period_sql} ORDER BY m.id DESC LIMIT ?"
                    );
                    let mut binds = vec![
                        SqlValue::Text(uid.clone()),
                        SqlValue::Text(bid.clone()),
                        SqlValue::Text(like),
                    ];
                    binds.extend(period.iter().cloned());
                    binds.push(SqlValue::Integer(limit));
                    (sql, binds)
                }
                // キーワード無し: 期間のみ。
                None => {
                    let sql = format!(
                        "SELECT m.role, m.content, m.created_at FROM message_logs m \
                         WHERE m.user_id = ? AND m.bot_id = ? AND m.guild_id IS NULL{period_sql} \
                         ORDER BY m.id DESC LIMIT ?"
                    );
                    let mut binds = vec![SqlValue::Text(uid.clone()), SqlValue::Text(bid.clone())];
                    binds.extend(period.iter().cloned());
                    binds.push(SqlValue::Integer(limit));
                    (sql, binds)
                }
            };

            let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(binds.iter()), |r| {
                    Ok(MessageHit {
                        role: r.get(0)?,
                        content: r.get(1)?,
                        created_at: r.get(2)?,
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

/// 期間文字列を境界へ正規化する（Node `normalizePeriod`）。`YYYY-MM-DD` は日の開始/終了へ。
fn normalize_period(value: &str, is_end: bool) -> String {
    // Node `.replace("T", " ")` は最初の 1 つだけ置換する（JS String 引数の意味論）。
    let v = value.trim().replacen('T', " ", 1);
    if is_ymd(&v) {
        if is_end {
            format!("{v} 23:59:59")
        } else {
            format!("{v} 00:00:00")
        }
    } else {
        v
    }
}

/// `^\d{4}-\d{2}-\d{2}$`。
fn is_ymd(v: &str) -> bool {
    let b = v.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, &c)| match i {
            4 | 7 => c == b'-',
            _ => c.is_ascii_digit(),
        })
}

/// LIKE のメタ文字（`\` `%` `_`）をエスケープする。
fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// 文字数で切り詰め、超過時は末尾に `…`（Node `truncateContent`）。
fn truncate_chars(content: &str, max: usize) -> String {
    if content.chars().count() <= max {
        content.to_owned()
    } else {
        let head: String = content.chars().take(max).collect();
        format!("{head}…")
    }
}

/// trim 後空でない文字列（無ければ `None`）。
fn opt_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_conv_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let conn = rusqlite::Connection::open(&path).expect("create empty");
            drop(conn);
        }
        let db = Db::open(&path).expect("open");
        {
            let conn = rusqlite::Connection::open(&path).expect("seed conn");
            conn.execute(
                "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt) \
                 VALUES ('u', 'u', 'x', 'x')",
                [],
            )
            .expect("seed user");
            // FTS トリガが張られているため message_logs への INSERT で fts も更新される。
            let rows = [
                (
                    "user",
                    "京都への旅行計画を立てたい",
                    "2026-07-01 10:00:00",
                    None,
                ),
                (
                    "assistant",
                    "旅行の日程はいつ頃ですか？",
                    "2026-07-01 10:00:05",
                    None,
                ),
                (
                    "user",
                    "引っ越しの見積もりを取った",
                    "2026-07-05 09:00:00",
                    None,
                ),
                // 他人/ギルドは対象外。
                ("user", "旅行の写真", "2026-07-02 10:00:00", Some("999")),
            ];
            for (role, content, created, guild) in rows {
                conn.execute(
                    "INSERT INTO message_logs (user_id, bot_id, role, content, created_at, guild_id) \
                     VALUES ('u', 'system_default', ?1, ?2, ?3, ?4)",
                    rusqlite::params![role, content, created, guild],
                )
                .expect("seed msg");
            }
        }
        db
    }

    #[tokio::test]
    async fn fts_keyword_search_scopes_to_user_dm() {
        let db = seed_db();
        // 「旅行」で FTS 検索 → DM の 2 件（ギルドの写真は除外）。DESC。
        let hits = search_messages(&db, "u", "system_default", Some("旅行"), None, None, 11)
            .await
            .unwrap();
        assert_eq!(hits.len(), 2);
        // guild_id IS NULL のみ（旅行の写真=ギルドは含まれない）。
        assert!(hits.iter().all(|h| h.content.contains("旅行")));
        assert!(!hits.iter().any(|h| h.content == "旅行の写真"));

        // 期間で絞り込み（7/5 のみ）。
        let hits = search_messages(
            &db,
            "u",
            "system_default",
            None,
            Some("2026-07-05"),
            None,
            11,
        )
        .await
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].content, "引っ越しの見積もりを取った");
    }

    #[tokio::test]
    async fn tool_validates_and_returns_chronological() {
        let db = seed_db();
        let tool_set = tools(db).unwrap();
        let tool = &tool_set[0];
        assert_eq!(tool.exposure().capability, Some("memory"));

        let ctx = ToolContext {
            bot_id: yuuka_core::BotId::system_default(),
            user_id: yuuka_core::UserId::new("u".to_owned()),
            guild_id: None,
            capabilities: yuuka_core::CapabilitySet::from_granted(vec!["memory".to_owned()]),
            mode: yuuka_core::TurnMode::Secretary,
            rich_reply_enabled: true,
        };

        // 条件なし → fail。
        let out = tool.call(&ctx, json!({})).await.unwrap();
        assert_eq!(out.payload["success"], false);

        // 旅行 → 2 件・時系列（古い順）。
        let out = tool.call(&ctx, json!({ "keyword": "旅行" })).await.unwrap();
        assert_eq!(out.payload["success"], true);
        let logs = out.payload["logs"].as_array().unwrap();
        assert_eq!(logs.len(), 2);
        // 古い順: 最初が 10:00:00。
        assert_eq!(logs[0]["created_at"], "2026-07-01 10:00:00");
        assert_eq!(logs[1]["created_at"], "2026-07-01 10:00:05");
    }
}
