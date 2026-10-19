//! ツール実行実績の記録（Node `gemini.ts` FC ループの `recordToolOutcome` 呼び出し + `utils/toolArgRedaction.ts`）。
//!
//! FC ループ（[`yuuka_gemini::run_function_calling_loop`]）は dispatch 毎に [`ToolOutcomeSink`] へ
//! 実績を渡す。本モジュールの [`DbToolOutcomeSink`] がそれを `tool_outcomes` へ 1 行保存する。
//! 引数は**そのまま保存しない**: [`build_outcome_args_digest`]（Node `buildOutcomeArgsDigest`）で
//! キー名マスク・認証系ツールの全伏せ・自由入力値の伏せ・値の形状による伏字化を施したダイジェストのみ残す
//! （§6.3.2 / §9.3 の多層防御）。`topic_id` は Node と同じく未付与（2nd Hop のトピック紐付けは未着手）。

use async_trait::async_trait;
use serde_json::Value;
use yuuka_core::{DbError, ToolContext};
use yuuka_gemini::{ToolOutcomeRecord, ToolOutcomeSink};
use yuuka_web::Db;

use crate::synapse_extract::looks_like_secret_value;
use crate::tool_outcome_repo::{self, InsertArgs};

/// 引数に秘匿値を含み得るツール（引数全体を伏せる・Node `SECRET_ARG_FUNCTIONS`）。
const SECRET_ARG_FUNCTIONS: &[&str] = &["addCredential", "updateCredential"];

/// 自由入力テキストを引数に持つツール（値そのものに認証情報がタイプされ得る・Node `FREE_TEXT_INPUT_TOOLS`）。
const FREE_TEXT_INPUT_TOOLS: &[&str] = &["browserInteractiveType"];

/// ダイジェストの最大文字数（Node `JSON.stringify(masked).slice(0, 500)`）。
const MAX_DIGEST_CHARS: usize = 500;

/// 秘匿すべき引数キーか（Node `SECRET_KEY_PATTERN = /password|secret|token|api_?key|credential/i`・部分一致）。
///
/// 操作履歴（[`yuuka_core::ActionRecorder`]）は `credential_value` のみだが、こちらは永続化用に
/// `credential` 全般を伏せる（Node の差をそのまま保つ）。
fn is_secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.contains("password")
        || k.contains("secret")
        || k.contains("token")
        || k.contains("apikey")
        || k.contains("api_key")
        || k.contains("credential")
}

/// 引数を秘匿マスクした JSON 文字列にする（Node `sanitizeArgsForLog`）。
fn sanitize_args(name: &str, args: &Value) -> String {
    if SECRET_ARG_FUNCTIONS.contains(&name) {
        let service = match args.get("service_name") {
            None | Some(Value::Null) => "?".to_owned(),
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
        };
        let mut obj = serde_json::Map::new();
        obj.insert(
            "(引数は秘匿情報を含むため非表示)".to_owned(),
            Value::String(format!("service={service}")),
        );
        return Value::Object(obj).to_string();
    }
    let mut masked = serde_json::Map::new();
    if let Some(obj) = args.as_object() {
        for (key, value) in obj {
            let v = if is_secret_key(key) {
                Value::String("(秘匿)".to_owned())
            } else {
                value.clone()
            };
            masked.insert(key.clone(), v);
        }
    }
    Value::Object(masked)
        .to_string()
        .chars()
        .take(MAX_DIGEST_CHARS)
        .collect()
}

/// 区切りを含まない塊の構成文字か（Node `TOKEN_RUN_RE = [A-Za-z0-9_\-+/=.]{12,}` の文字集合）。
fn is_run_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '/' | '=' | '.')
}

/// テキスト中の秘匿値らしい塊（12 文字以上）を `(秘匿値)` へ置換する（Node `redactSecretsInText`）。
fn redact_secrets_in_text(text: &str) -> String {
    fn flush(out: &mut String, run: &mut String) {
        if run.chars().count() >= 12 && looks_like_secret_value(run) {
            out.push_str("(秘匿値)");
        } else {
            out.push_str(run);
        }
        run.clear();
    }
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    for c in text.chars() {
        if is_run_char(c) {
            run.push(c);
        } else {
            flush(&mut out, &mut run);
            out.push(c);
        }
    }
    flush(&mut out, &mut run);
    out
}

/// `tool_outcomes.args_digest` 用の引数ダイジェスト（Node `buildOutcomeArgsDigest`）。
///
/// キー名マスク＋認証系ツール全伏せ（[`sanitize_args`]）に加え、
/// 1) 自由入力系ツールの `text` 値を `(入力値:N文字)` へ構造的に伏せ、
/// 2) 値の形状から秘匿らしいトークン（JWT/API キー/鍵等）を伏字化する。
#[must_use]
pub fn build_outcome_args_digest(name: &str, args: &Value) -> String {
    let replaced;
    let mut safe_args = args;
    if FREE_TEXT_INPUT_TOOLS.contains(&name) {
        if let Some(text) = args.get("text").and_then(Value::as_str) {
            let mut copy = args.clone();
            if let Some(obj) = copy.as_object_mut() {
                obj.insert(
                    "text".to_owned(),
                    Value::String(format!("(入力値:{}文字)", text.chars().count())),
                );
            }
            replaced = copy;
            safe_args = &replaced;
        }
    }
    redact_secrets_in_text(&sanitize_args(name, safe_args))
}

/// `tool_outcomes` へ保存する [`ToolOutcomeSink`] 本番実装（main.rs で [`crate::ChatEngine::with_tool_outcomes`] 注入）。
pub struct DbToolOutcomeSink {
    db: Db,
}

impl DbToolOutcomeSink {
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ToolOutcomeSink for DbToolOutcomeSink {
    async fn record(
        &self,
        ctx: &ToolContext,
        record: ToolOutcomeRecord<'_>,
    ) -> Result<(), DbError> {
        let args = InsertArgs {
            user_id: ctx.user_id.as_str().to_owned(),
            bot_id: ctx.bot_id.as_str().to_owned(),
            guild_id: ctx.guild_id.as_ref().map(|g| g.as_str().to_owned()),
            topic_id: None,
            synapse_id: None,
            tool_name: record.tool_name.to_owned(),
            args_digest: Some(build_outcome_args_digest(record.tool_name, record.args)),
            status: record.status.as_str().to_owned(),
            latency_ms: Some(i64::try_from(record.latency_ms).unwrap_or(i64::MAX)),
        };
        tool_outcome_repo::insert_tool_outcome(&self.db, args)
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;
    use yuuka_core::{BotId, GuildId, UserId};
    use yuuka_db::map_sqlite;
    use yuuka_gemini::ToolOutcomeStatus;

    /// 読み戻し 1 行（user, bot, guild, tool, digest, status, latency）。
    type StoredRow = (String, String, Option<String>, String, String, String, i64);

    #[test]
    fn digest_masks_secret_keys() {
        let d = build_outcome_args_digest(
            "loginSite",
            &json!({"url": "https://example.com", "password": "hunter2", "api_key": "x", "credentialId": 3}),
        );
        assert!(!d.contains("hunter2"));
        assert!(d.contains(r#""password":"(秘匿)""#));
        assert!(d.contains(r#""api_key":"(秘匿)""#));
        assert!(d.contains(r#""credentialId":"(秘匿)""#));
        assert!(d.contains("https://example.com"));
    }

    #[test]
    fn digest_hides_credential_tool_args_entirely() {
        let d = build_outcome_args_digest(
            "addCredential",
            &json!({"service_name": "github", "username": "me", "password": "p"}),
        );
        assert_eq!(
            d,
            r#"{"(引数は秘匿情報を含むため非表示)":"service=github"}"#
        );
        let d = build_outcome_args_digest("updateCredential", &json!({}));
        assert_eq!(d, r#"{"(引数は秘匿情報を含むため非表示)":"service=?"}"#);
    }

    #[test]
    fn digest_hides_free_text_input_value() {
        let d = build_outcome_args_digest(
            "browserInteractiveType",
            &json!({"selector": "#pw", "text": "わたしのひみつ"}),
        );
        assert!(!d.contains("ひみつ"));
        assert!(d.contains("(入力値:7文字)"));
        assert!(d.contains("#pw"));
    }

    #[test]
    fn digest_redacts_secret_shaped_values() {
        let d = build_outcome_args_digest(
            "webSearch",
            &json!({"query": "token は ghp_abcdefghijklmnopqrstuvwxyz0123 です"}),
        );
        assert!(!d.contains("ghp_abcdef"));
        assert!(d.contains("(秘匿値)"));
        // 普通の値は残る。
        let d = build_outcome_args_digest("addTodo", &json!({"title": "牛乳を買う"}));
        assert_eq!(d, r#"{"title":"牛乳を買う"}"#);
    }

    #[test]
    fn digest_is_truncated_to_500_chars() {
        let long = "あ".repeat(1000);
        let d = build_outcome_args_digest("addTodo", &json!({"title": long}));
        assert_eq!(d.chars().count(), 500);
    }

    /// DB 記録器は ctx の user/bot/guild とダイジェストで 1 行保存し、生の引数は残さない。
    #[tokio::test]
    async fn db_sink_inserts_digest_row() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("t.db");
        drop(rusqlite::Connection::open(&path).expect("create empty"));
        let db = Db::open(Path::new(&path)).expect("open db");

        let sink = DbToolOutcomeSink::new(db.clone());
        let mut ctx = ToolContext::new(BotId::new("b1"), UserId::new("u1"));
        ctx.guild_id = Some(GuildId::new("g1"));
        let args = json!({"title": "牛乳", "password": "hunter2"});
        sink.record(
            &ctx,
            ToolOutcomeRecord {
                tool_name: "addTodo",
                args: &args,
                status: ToolOutcomeStatus::Error,
                latency_ms: 12,
            },
        )
        .await
        .expect("record");

        let rows: Vec<StoredRow> = db
            .read
            .read(|conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT user_id, bot_id, guild_id, tool_name, args_digest, status, latency_ms \
                         FROM tool_outcomes",
                    )
                    .map_err(map_sqlite)?;
                let rows = stmt
                    .query_map([], |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                            r.get(5)?,
                            r.get(6)?,
                        ))
                    })
                    .map_err(map_sqlite)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(map_sqlite)?;
                Ok(rows)
            })
            .await
            .expect("read back");
        assert_eq!(rows.len(), 1);
        let (user, bot, guild, tool, digest, status, latency) = rows[0].clone();
        assert_eq!((user.as_str(), bot.as_str()), ("u1", "b1"));
        assert_eq!(guild.as_deref(), Some("g1"));
        assert_eq!(tool, "addTodo");
        assert!(!digest.contains("hunter2"), "生の秘匿値は保存しない");
        assert_eq!(status, "error");
        assert_eq!(latency, 12);
    }
}
