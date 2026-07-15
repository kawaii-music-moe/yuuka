//! briefing/report 配信設定ツール（秘書経路・Node `briefingFunctions` パリティ）。
//!
//! **本crateでカバー**: configureReport（日報/週報の配信設定を部分更新）・getBriefingConfig
//! （朝ブリーフィング + レポートの現在設定を読む）。**deferred**: configureBriefing（SSRF ガード +
//! ニュースフィード配列の部分更新が必要）・runBriefingNow（briefing サービス本体＝天気/RSS 依存）。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use yuuka_core::tool::FunctionDeclaration;
use yuuka_core::{
    EmbedFieldPart, EmbedPart, ResponsePart, Tool, ToolContext, ToolError, ToolName, ToolOutcome,
};
use yuuka_web::Db;

use crate::repo::{self, BriefingConfig, ReportConfig};
use crate::service;

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
            db: db.clone(),
        }),
        Arc::new(ConfigureBriefingTool {
            name: ToolName::checked("configureBriefing".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(RunBriefingNowTool {
            name: ToolName::checked("runBriefingNow".to_owned())?,
            db,
        }),
    ])
}

/// SSRF ガード: 公開 http(s) URL らしいか（Node `isLikelyPublicHttpUrl`）。内部/ローカル/非 http(s) を拒否。
pub(crate) fn is_likely_public_http_url(raw: &str) -> bool {
    let Some(rest) = raw
        .strip_prefix("http://")
        .or_else(|| raw.strip_prefix("https://"))
    else {
        return false;
    };
    // authority = 最初の '/' '?' '#' まで。
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return false; // userinfo は拒否。
    }
    // ホスト（ポート除去・IPv6 は角括弧内）。
    let host = if let Some(stripped) = authority.strip_prefix('[') {
        stripped.split(']').next().unwrap_or("")
    } else {
        authority.rsplit_once(':').map_or(authority, |(h, _)| h)
    }
    .to_ascii_lowercase();
    if host.is_empty() || host == "localhost" || host.ends_with(".localhost") {
        return false;
    }
    if let Ok(v4) = host.parse::<std::net::Ipv4Addr>() {
        return !is_blocked_ipv4(v4);
    }
    if let Ok(v6) = host.parse::<std::net::Ipv6Addr>() {
        return !is_blocked_ipv6(v6);
    }
    // ドメイン名は公開扱い（DNS リバインドは実行時の fetch 側で別途対処）。
    true
}

fn is_blocked_ipv4(ip: std::net::Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || o[0] == 0 // 0.0.0.0/8 "this host"
        || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0.0/24
}

fn is_blocked_ipv6(ip: std::net::Ipv6Addr) -> bool {
    let seg0 = ip.segments()[0];
    ip.is_loopback()
        || ip.is_unspecified()
        || (seg0 & 0xfe00) == 0xfc00 // fc00::/7 ULA
        || (seg0 & 0xffc0) == 0xfe80 // fe80::/10 link-local
}

fn fail(message: impl Into<String>) -> ToolOutcome {
    ToolOutcome::from_payload(json!({ "success": false, "message": message.into() }))
}

fn exec_err(e: yuuka_core::DbError) -> ToolError {
    ToolError::Execution(e.to_string())
}

/// 最小 cron 妥当性（5 フィールド・空でない・Node `cron.validate` の簡約）。
pub(crate) fn is_valid_cron_basic(rule: &str) -> bool {
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

// ─── configureBriefing ───────────────────────────────────────────────────────

struct ConfigureBriefingTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for ConfigureBriefingTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "朝の自動ブリーフィング（天気・ニュース）を設定する。有効/無効・配信時刻\
                （cron）・地点（緯度経度/地名）・ニュースフィード（RSS の追加/削除）・キーワードを変える。\
                指定した項目だけ更新する。フィードは http(s) の公開URLのみ。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "enabled": { "type": "boolean", "description": "有効にするか（任意）" },
                    "schedule_cron": { "type": "string", "description": "配信時刻の cron 式（例 '0 7 * * *'・任意）" },
                    "latitude": { "type": "number", "description": "天気の緯度（任意）" },
                    "longitude": { "type": "number", "description": "天気の経度（任意）" },
                    "location_name": { "type": "string", "description": "地名（任意）" },
                    "add_news_feed": { "type": "string", "description": "追加する RSS フィードURL（http(s) 公開URLのみ・任意）" },
                    "remove_news_feed": { "type": "string", "description": "削除するフィード（URL の部分一致・任意）" },
                    "news_keywords": { "type": "array", "items": { "type": "string" }, "description": "ニュース絞り込みキーワード（任意・全置換）" }
                }
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        if let Some(cron) = arg_str(&args, "schedule_cron") {
            if !is_valid_cron_basic(&cron) {
                return Ok(fail("schedule_cron のcron式が不正です。"));
            }
        }
        // SSRF: 追加フィードは公開 http(s) URL のみ。
        let add_feed = arg_str(&args, "add_news_feed");
        if let Some(url) = &add_feed {
            if !is_likely_public_http_url(url) {
                return Ok(fail(
                    "add_news_feed は http(s) の公開URLのみ指定できます（内部アドレスは不可）。",
                ));
            }
        }
        let remove_feed = arg_str(&args, "remove_news_feed");

        // フィードの add/remove を現在値へ適用。
        let feeds = if add_feed.is_some() || remove_feed.is_some() {
            let mut feeds = repo::get_briefing(&self.db, ctx.user_id.as_str(), ctx.bot_id.as_str())
                .await
                .map_err(exec_err)?
                .news_feeds;
            if let Some(url) = add_feed {
                if !feeds.contains(&url) {
                    feeds.push(url);
                }
            }
            if let Some(target) = remove_feed {
                feeds.retain(|f| !f.contains(&target));
            }
            Some(feeds)
        } else {
            None
        };

        let patch = repo::BriefingPatch {
            enabled: args.get("enabled").map(|v| v.as_bool() == Some(true)),
            schedule_cron: arg_str(&args, "schedule_cron"),
            // tool は配信先を触らない（Web-API 側のみ設定可）。
            target_type: None,
            target_id: None,
            // 緯度経度/地名は present のときのみ設定（tool ではクリアは未対応）。
            weather_lat: args.get("latitude").and_then(Value::as_f64).map(Some),
            weather_lng: args.get("longitude").and_then(Value::as_f64).map(Some),
            location_name: arg_str(&args, "location_name").map(Some),
            news_feeds: feeds,
            news_keywords: args
                .get("news_keywords")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                }),
        };
        let updated =
            repo::upsert_briefing(&self.db, ctx.user_id.as_str(), ctx.bot_id.as_str(), patch)
                .await
                .map_err(exec_err)?;
        Ok(ToolOutcome::from_payload(json!({
            "success": true,
            "message": "朝報の設定を更新しました🌅",
            "config": briefing_view(&updated),
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

// ─── runBriefingNow ──────────────────────────────────────────────────────────

/// 朝報を今すぐ生成して**この返信にインライン表示**する（Node `runBriefingNow`）。
///
/// **意図的 divergence**: Node は設定した配信先（DM/channel）へ Discord 送信し「テスト配信しました」
/// と返す。Rust は Discord 配信（Messenger）が Discord live 配線時のシームのため、手動 runBriefingNow
/// は**生成した朝報をインライン Embed で返す**（今すぐ確認できて有用）。定時配信（cron→配信先）は
/// DeliveryRunner シームが担い、同じ [`service::build_briefing`] 生成ロジックを再利用する。
struct RunBriefingNowTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for RunBriefingNowTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "設定済みの朝報（天気・ニュース）を今すぐ生成して見せる。\n\
                ・「朝報を出して」「今日の天気とニュースまとめて」等で呼ぶ。\n\
                ・先に configureBriefing で天気の地点やニュースのRSSフィードを設定しておく必要がある。"
                .to_owned(),
            parameters_json_schema: json!({ "type": "object", "properties": {} }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, _args: Value) -> Result<ToolOutcome, ToolError> {
        let content = service::build_briefing(&self.db, ctx.user_id.as_str(), ctx.bot_id.as_str())
            .await
            .map_err(exec_err)?;
        let Some(content) = content else {
            return Ok(fail(
                "朝報がまだ設定されていません。先に configureBriefing で天気の地点やニュースフィードを設定してください。",
            ));
        };

        let fields: Vec<EmbedFieldPart> = content
            .fields
            .into_iter()
            .map(|(name, value)| EmbedFieldPart {
                name,
                value,
                inline: false,
            })
            .collect();
        let description = if content.has_content {
            None
        } else {
            Some(service::BRIEFING_EMPTY_DESC.to_owned())
        };

        let embed = EmbedPart {
            title: Some(service::BRIEFING_TITLE.to_owned()),
            description,
            color: service::BRIEFING_COLOR,
            fields,
            footer: Some(service::BRIEFING_FOOTER.to_owned()),
        };

        Ok(ToolOutcome {
            payload: json!({ "success": true, "message": "朝報を生成しました🌅" }),
            parts: vec![ResponsePart::Embed(embed)],
        })
    }
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

    #[test]
    fn ssrf_guard_rejects_internal_urls() {
        assert!(is_likely_public_http_url("https://example.com/feed.xml"));
        assert!(is_likely_public_http_url("http://news.example.co.jp/rss"));
        // 非 http(s)・内部・ローカル・メタデータは拒否。
        assert!(!is_likely_public_http_url("ftp://example.com"));
        assert!(!is_likely_public_http_url("http://localhost/rss"));
        assert!(!is_likely_public_http_url("http://127.0.0.1/rss"));
        assert!(!is_likely_public_http_url("http://10.0.0.5/rss"));
        assert!(!is_likely_public_http_url("http://192.168.1.1/rss"));
        assert!(!is_likely_public_http_url(
            "http://169.254.169.254/latest/meta-data"
        ));
        assert!(!is_likely_public_http_url("http://user:pw@example.com/rss"));
        assert!(!is_likely_public_http_url("http://[::1]/rss"));
    }

    #[tokio::test]
    async fn configure_briefing_feeds_and_partial() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let configure = find(&tools, "configureBriefing");
        let get = find(&tools, "getBriefingConfig");

        // 不正 cron → fail。
        assert_eq!(
            configure
                .call(&ctx(), json!({"schedule_cron": "bad"}))
                .await
                .unwrap()
                .payload["success"],
            false
        );
        // 内部フィード → SSRF で fail。
        assert_eq!(
            configure
                .call(&ctx(), json!({"add_news_feed": "http://10.0.0.1/rss"}))
                .await
                .unwrap()
                .payload["success"],
            false
        );

        // 有効化 + 地名 + 公開フィード追加。
        let out = configure
            .call(
                &ctx(),
                json!({"enabled": true, "location_name": "東京", "add_news_feed": "https://example.com/a.xml"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["config"]["enabled"], true);
        assert_eq!(out.payload["config"]["location"], "東京");
        assert_eq!(
            out.payload["config"]["news_feeds"],
            json!(["https://example.com/a.xml"])
        );

        // フィード追加（2件目）+ enabled は保持（部分更新）。
        let out = configure
            .call(
                &ctx(),
                json!({"add_news_feed": "https://example.com/b.xml"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["config"]["enabled"], true);
        assert_eq!(
            out.payload["config"]["news_feeds"]
                .as_array()
                .unwrap()
                .len(),
            2
        );

        // 部分一致で削除。
        let out = configure
            .call(&ctx(), json!({"remove_news_feed": "a.xml"}))
            .await
            .unwrap();
        assert_eq!(
            out.payload["config"]["news_feeds"],
            json!(["https://example.com/b.xml"])
        );

        // getBriefingConfig に反映。
        let out = get.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["briefing"]["enabled"], true);
        assert_eq!(
            out.payload["briefing"]["news_feeds"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn run_briefing_now_fails_when_unconfigured() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let run = find(&tools, "runBriefingNow");
        // 設定行が無い → 「まだ設定されていません」で fail（build_briefing が None）。
        let out = run.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["success"], false);
        assert!(out.payload["message"]
            .as_str()
            .unwrap()
            .contains("朝報がまだ設定されていません"));
        assert!(out.parts.is_empty());
    }
}
