//! briefing/report 配信設定ツール（秘書経路・Node `briefingFunctions` パリティ）。
//!
//! **本crateでカバー**: configureReport（日報/週報の配信設定を部分更新）・getBriefingConfig
//! （朝ブリーフィング + レポートの現在設定を読む）。**deferred**: configureBriefing（SSRF ガード +
//! ニュースフィード配列の部分更新が必要）・runBriefingNow（briefing サービス本体＝天気/RSS 依存）。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use yuuka_core::tool::FunctionDeclaration;
use yuuka_core::{Tool, ToolContext, ToolError, ToolName, ToolOutcome};
use yuuka_web::Db;

use crate::repo::{self, BriefingConfig, ReportConfig};

/// このクレートが公開するツール（configureReport / getBriefingConfig）。
///
/// # Errors
/// ツール名が Gemini 制約に反する場合 [`ToolError`]。
pub fn tools(db: Db) -> Result<Vec<Arc<dyn Tool>>, ToolError> {
    Ok(vec![
        Arc::new(ConfigureReportTool {
            name: ToolName::checked("configureReport".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(GetBriefingConfigTool {
            name: ToolName::checked("getBriefingConfig".to_owned())?,
            db,
        }),
    ])
}

fn fail(message: impl Into<String>) -> ToolOutcome {
    ToolOutcome::from_payload(json!({ "success": false, "message": message.into() }))
}

fn exec_err(e: yuuka_core::DbError) -> ToolError {
    ToolError::Execution(e.to_string())
}

/// 最小 cron 妥当性（5 フィールド・空でない・Node `cron.validate` の簡約）。
fn is_valid_cron_basic(rule: &str) -> bool {
    let fields: Vec<&str> = rule.split_whitespace().collect();
    fields.len() == 5 && fields.iter().all(|f| !f.is_empty())
}

/// trim 後空なら `None`。
fn arg_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

// ─── configureReport ─────────────────────────────────────────────────────────

struct ConfigureReportTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for ConfigureReportTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "日報（daily）／週報（weekly）の自動配信を設定する。有効/無効・配信時刻\
                （cron 式）・配信先を変える。指定した項目だけ更新する。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "type": { "type": "string", "description": "'daily'（日報）| 'weekly'（週報）" },
                    "enabled": { "type": "boolean", "description": "有効にするか（任意）" },
                    "schedule_cron": { "type": "string", "description": "配信時刻の cron 式（例 '0 21 * * *'・任意）" },
                    "target_type": { "type": "string", "description": "'dm' | 'channel'（任意）" },
                    "target_id": { "type": "string", "description": "channel 指定時のチャンネルID（任意）" }
                },
                "required": ["type"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let r#type = args.get("type").and_then(Value::as_str).unwrap_or("");
        if r#type != "daily" && r#type != "weekly" {
            return Ok(fail("type は 'daily' または 'weekly' を指定してください。"));
        }
        let schedule_cron = arg_str(&args, "schedule_cron");
        if let Some(cron) = &schedule_cron {
            if !is_valid_cron_basic(cron) {
                return Ok(fail("schedule_cron のcron式が不正です。"));
            }
        }
        // 部分更新: present なフィールドのみ上書き。
        let enabled = args.get("enabled").map(|v| v.as_bool() == Some(true));
        let target_type = args
            .get("target_type")
            .and_then(Value::as_str)
            .map(|t| if t == "channel" { "channel" } else { "dm" }.to_owned());
        let target_id = args.get("target_id").map(|v| {
            v.as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        });

        let updated = repo::upsert_report(
            &self.db,
            ctx.user_id.as_str(),
            ctx.bot_id.as_str(),
            r#type,
            repo::ReportPatch {
                enabled,
                schedule_cron,
                target_type,
                target_id,
            },
        )
        .await
        .map_err(exec_err)?;

        let type_label = if r#type == "daily" {
            "日報"
        } else {
            "週報"
        };
        let state = if updated.enabled { "有効" } else { "無効" };
        Ok(ToolOutcome::from_payload(json!({
            "success": true,
            "message": format!(
                "{type_label}の配信設定を更新しました📋（{state} / {}）",
                updated.schedule_cron
            ),
            "config": report_view(&updated),
        })))
    }
}

// ─── getBriefingConfig ───────────────────────────────────────────────────────

struct GetBriefingConfigTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for GetBriefingConfigTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "朝ブリーフィングと日報/週報の現在の配信設定を取得する。".to_owned(),
            parameters_json_schema: json!({ "type": "object", "properties": {} }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, _args: Value) -> Result<ToolOutcome, ToolError> {
        let user = ctx.user_id.as_str();
        let bot = ctx.bot_id.as_str();
        let briefing = repo::get_briefing(&self.db, user, bot)
            .await
            .map_err(exec_err)?;
        let reports = repo::get_reports(&self.db, user, bot)
            .await
            .map_err(exec_err)?;
        Ok(ToolOutcome::from_payload(json!({
            "success": true,
            "briefing": briefing_view(&briefing),
            "reports": reports.iter().map(report_view).collect::<Vec<_>>(),
        })))
    }
}

fn briefing_view(c: &BriefingConfig) -> Value {
    json!({
        "enabled": c.enabled,
        "schedule_cron": c.schedule_cron,
        "target_type": c.target_type,
        "target_id": c.target_id,
        "location": c.location_name,
        "news_feeds": c.news_feeds,
        "news_keywords": c.news_keywords,
    })
}

fn report_view(c: &ReportConfig) -> Value {
    json!({
        "type": c.r#type,
        "enabled": c.enabled,
        "schedule_cron": c.schedule_cron,
        "target_type": c.target_type,
        "target_id": c.target_id,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use rusqlite::Connection;
    use yuuka_core::{BotId, UserId};

    use super::*;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    // briefing_configs / report_configs を FK 無しで先に作る（V17 の users FK を避ける）。
    const DDL: &str = "CREATE TABLE briefing_configs (\
        user_id TEXT NOT NULL, bot_id TEXT NOT NULL DEFAULT 'system_default', \
        enabled INTEGER NOT NULL DEFAULT 0, schedule_cron TEXT NOT NULL DEFAULT '0 7 * * *', \
        target_type TEXT NOT NULL DEFAULT 'dm', target_id TEXT, weather_lat REAL, weather_lng REAL, \
        location_name TEXT, news_feeds TEXT NOT NULL DEFAULT '[]', \
        news_keywords TEXT NOT NULL DEFAULT '[]', \
        updated_at TEXT NOT NULL DEFAULT (datetime('now','localtime')), PRIMARY KEY (user_id, bot_id));\
        CREATE TABLE report_configs (\
        id INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL, \
        bot_id TEXT NOT NULL DEFAULT 'system_default', type TEXT NOT NULL, \
        enabled INTEGER NOT NULL DEFAULT 0, schedule_cron TEXT NOT NULL DEFAULT '0 21 * * *', \
        target_type TEXT NOT NULL DEFAULT 'dm', target_id TEXT, \
        updated_at TEXT NOT NULL DEFAULT (datetime('now','localtime')), UNIQUE(user_id, bot_id, type));";

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_briefing_{}_{seq}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(DDL).unwrap();
        }
        Db::open(&path).unwrap()
    }

    fn ctx() -> ToolContext {
        ToolContext::new(BotId::system_default(), UserId::new("userA"))
    }

    fn find<'a>(tools: &'a [Arc<dyn Tool>], name: &str) -> &'a Arc<dyn Tool> {
        tools
            .iter()
            .find(|t| t.declaration().name.as_str() == name)
            .unwrap()
    }

    #[tokio::test]
    async fn configure_report_and_get_config() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let configure = find(&tools, "configureReport");
        let get = find(&tools, "getBriefingConfig");

        // 不正 type / cron → fail。
        assert_eq!(
            configure
                .call(&ctx(), json!({"type": "monthly"}))
                .await
                .unwrap()
                .payload["success"],
            false
        );
        assert_eq!(
            configure
                .call(&ctx(), json!({"type": "daily", "schedule_cron": "bad"}))
                .await
                .unwrap()
                .payload["success"],
            false
        );

        // 日報を有効化。
        let out = configure
            .call(
                &ctx(),
                json!({"type": "daily", "enabled": true, "schedule_cron": "0 8 * * *"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["config"]["enabled"], true);
        assert_eq!(out.payload["config"]["schedule_cron"], "0 8 * * *");

        // 部分更新: enabled のみ false に（cron は保持）。
        let out = configure
            .call(&ctx(), json!({"type": "daily", "enabled": false}))
            .await
            .unwrap();
        assert_eq!(out.payload["config"]["enabled"], false);
        assert_eq!(out.payload["config"]["schedule_cron"], "0 8 * * *");

        // getBriefingConfig: briefing 既定 + reports に日報 1 件。
        let out = get.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["briefing"]["enabled"], false);
        assert_eq!(out.payload["briefing"]["schedule_cron"], "0 7 * * *");
        let reports = out.payload["reports"].as_array().unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0]["type"], "daily");

        // 別ユーザーには漏れない。
        let out = get
            .call(
                &ToolContext::new(BotId::system_default(), UserId::new("userB")),
                json!({}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["reports"].as_array().unwrap().len(), 0);
    }
}
