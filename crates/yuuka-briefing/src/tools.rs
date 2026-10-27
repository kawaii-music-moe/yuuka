//! 定期報告ツール（秘書経路）。
//!
//! - listPeriodicReports: 本人の定期報告と設定内容を読む
//! - savePeriodicReport: 新規登録・部分更新（Web-API と同じ検証）
//! - deletePeriodicReport: 削除
//! - runPeriodicReportNow: 今すぐ生成して会話に表示する／設定した配信先へテスト配信する

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Local;
use serde_json::{json, Map, Value};
use yuuka_core::aux_text::{AuxTextGenerator, NullAuxTextGenerator};
use yuuka_core::tool::FunctionDeclaration;
use yuuka_core::{ResponsePart, Tool, ToolContext, ToolError, ToolName, ToolOutcome};
use yuuka_web::Db;

use crate::compose;
use crate::manage::{self, SaveError};
use crate::model::{report_view, PeriodicReport, ReportSection};
use crate::repo;
use crate::routes::{DeliveryRunner, NullDeliveryRunner};

/// このクレートが公開するツール（配信 runner 未注入＝`runPeriodicReportNow` の `deliver: true` は常に未配信）。
///
/// # Errors
/// ツール名が Gemini 制約に反する場合 [`ToolError`]。
pub fn tools(db: Db) -> Result<Vec<Arc<dyn Tool>>, ToolError> {
    tools_with(db, Arc::new(NullDeliveryRunner))
}

/// 配信 runner を注入してツールを組む。`runPeriodicReportNow` の `deliver: true` は `runner` で
/// 設定した配信先へテスト配信する（`/api/periodic-reports/test` と同じ経路）。
///
/// # Errors
/// ツール名が Gemini 制約に反する場合 [`ToolError`]。
pub fn tools_with(
    db: Db,
    runner: Arc<dyn DeliveryRunner>,
) -> Result<Vec<Arc<dyn Tool>>, ToolError> {
    tools_with_aux_text(db, runner, Arc::new(NullAuxTextGenerator))
}

/// 補助 LLM を注入してツール群を構築する。本番 assembly はこの経路を使い、未配線時は [`tools`] が
/// ニュースのタイトル列挙・活動データの生データ本文へ安全に縮退する。
///
/// # Errors
/// ツール名が Gemini 制約に反する場合 [`ToolError`]。
pub fn tools_with_aux_text(
    db: Db,
    runner: Arc<dyn DeliveryRunner>,
    aux_text: Arc<dyn AuxTextGenerator>,
) -> Result<Vec<Arc<dyn Tool>>, ToolError> {
    Ok(vec![
        Arc::new(ListPeriodicReportsTool {
            name: ToolName::checked("listPeriodicReports".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(SavePeriodicReportTool {
            name: ToolName::checked("savePeriodicReport".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(DeletePeriodicReportTool {
            name: ToolName::checked("deletePeriodicReport".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(RunPeriodicReportNowTool {
            name: ToolName::checked("runPeriodicReportNow".to_owned())?,
            db,
            runner,
            aux_text,
        }),
    ])
}

fn fail(message: impl Into<String>) -> ToolOutcome {
    ToolOutcome::from_payload(json!({ "success": false, "message": message.into() }))
}

fn exec_err(e: yuuka_core::DbError) -> ToolError {
    ToolError::Execution(e.to_string())
}

/// 対象の指定（`id` か `name`）の JSON Schema プロパティ。
fn target_properties() -> Map<String, Value> {
    let mut properties = Map::new();
    properties.insert(
        "id".to_owned(),
        json!({ "type": "integer", "description": "定期報告の ID（listPeriodicReports で確認）" }),
    );
    properties.insert(
        "name".to_owned(),
        json!({ "type": "string", "description": "定期報告の名前（id の代わりに指定できる）" }),
    );
    properties
}

/// `id`（優先）か `name` で本人の定期報告を探す。
async fn find_target(
    db: &Db,
    ctx: &ToolContext,
    args: &Value,
) -> Result<Result<PeriodicReport, ToolOutcome>, ToolError> {
    let (user, bot) = (ctx.user_id.as_str(), ctx.bot_id.as_str());
    if let Some(id) = args.get("id").and_then(Value::as_i64) {
        let found = repo::find_report(db, user, bot, id)
            .await
            .map_err(exec_err)?;
        return Ok(found.ok_or_else(|| fail(format!("ID {id} の定期報告は見つかりません。"))));
    }
    let Some(name) = args
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|n| !n.is_empty())
    else {
        return Ok(Err(fail("id か name で定期報告を指定してください。")));
    };
    let found = repo::list_reports(db, user, bot)
        .await
        .map_err(exec_err)?
        .into_iter()
        .find(|r| r.settings.name == name);
    Ok(found.ok_or_else(|| fail(format!("「{name}」という定期報告は見つかりません。"))))
}

// ─── listPeriodicReports ─────────────────────────────────────────────────────

struct ListPeriodicReportsTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for ListPeriodicReportsTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "登録済みの定期報告（朝報・日報・週報など、天気・ニュース・タスク等を\
                組み合わせて定時に送るもの）の一覧と設定内容を取得する。"
                .to_owned(),
            parameters_json_schema: json!({ "type": "object", "properties": {} }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, _args: Value) -> Result<ToolOutcome, ToolError> {
        let reports = repo::list_reports(&self.db, ctx.user_id.as_str(), ctx.bot_id.as_str())
            .await
            .map_err(exec_err)?;
        Ok(ToolOutcome::from_payload(json!({
            "success": true,
            "reports": reports.iter().map(report_view).collect::<Vec<_>>(),
        })))
    }
}

// ─── savePeriodicReport ──────────────────────────────────────────────────────

struct SavePeriodicReportTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for SavePeriodicReportTool {
    fn declaration(&self) -> FunctionDeclaration {
        let section_keys: Vec<&str> = ReportSection::ALL.iter().map(|s| s.key()).collect();
        FunctionDeclaration {
            name: self.name.clone(),
            description: "定期報告を新しく登録する、または既存のものを変更する（id を指定すると変更・\
                指定した項目だけ更新）。新規登録には name・schedule_cron・sections が必要。\n\
                ・載せる内容（sections）: weather=天気 / news=RSSニュース / tasks=完了・持ち越しタスク / \
                schedules=これからの予定 / payments=支払い予定 / finance=家計（収入・支出）/ topics=会話のトピック。\n\
                ・tasks・schedules・payments・finance を含むものは DM にしか送れない（target_type は dm）。\n\
                ・weather には地点（location_name か latitude+longitude）、news には RSS フィードが1件以上必要。\n\
                ・フィードは http(s) の公開URLのみ。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "変更する定期報告の ID（省略で新規登録）" },
                    "name": { "type": "string", "description": "名前（例 '朝報'・40文字以内）" },
                    "enabled": { "type": "boolean", "description": "有効にするか（新規は既定で有効）" },
                    "schedule_cron": { "type": "string", "description": "配信時刻の cron 式（例 '0 7 * * *'）" },
                    "target_type": { "type": "string", "description": "'dm' | 'channel'" },
                    "target_id": { "type": "string", "description": "channel 指定時のチャンネルID" },
                    "sections": {
                        "type": "array",
                        "items": { "type": "string", "enum": section_keys },
                        "description": "載せる内容（全置換・1つ以上）"
                    },
                    "location_name": { "type": "string", "description": "天気の地点名（例 '東京'）" },
                    "latitude": { "type": "number", "description": "天気の緯度（地点名より優先）" },
                    "longitude": { "type": "number", "description": "天気の経度" },
                    "add_news_feed": { "type": "string", "description": "追加する RSS フィードURL" },
                    "remove_news_feed": { "type": "string", "description": "削除するフィード（URL の部分一致）" },
                    "news_keywords": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "ニュース絞り込みキーワード（全置換）"
                    }
                }
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let (user, bot) = (ctx.user_id.as_str(), ctx.bot_id.as_str());
        let id = args.get("id").and_then(Value::as_i64);

        // ツールの引数を API と同じキーの変更内容へ写す。
        let mut changes = Map::new();
        for key in [
            "name",
            "enabled",
            "schedule_cron",
            "target_type",
            "target_id",
            "sections",
            "location_name",
            "news_keywords",
        ] {
            if let Some(value) = args.get(key) {
                changes.insert(key.to_owned(), value.clone());
            }
        }
        for (arg, key) in [("latitude", "weather_lat"), ("longitude", "weather_lng")] {
            if let Some(value) = args.get(arg) {
                changes.insert(key.to_owned(), value.clone());
            }
        }
        let add_feed = args.get("add_news_feed").and_then(Value::as_str);
        let remove_feed = args.get("remove_news_feed").and_then(Value::as_str);
        if add_feed.is_some() || remove_feed.is_some() {
            let mut feeds = match id {
                Some(id) => repo::find_report(&self.db, user, bot, id)
                    .await
                    .map_err(exec_err)?
                    .map(|r| r.settings.news_feeds)
                    .unwrap_or_default(),
                None => Vec::new(),
            };
            if let Some(url) = add_feed.map(str::trim).filter(|u| !u.is_empty()) {
                if !feeds.iter().any(|f| f == url) {
                    feeds.push(url.to_owned());
                }
            }
            if let Some(part) = remove_feed.filter(|p| !p.is_empty()) {
                feeds.retain(|f| !f.contains(part));
            }
            changes.insert("news_feeds".to_owned(), json!(feeds));
        }

        match manage::save_report(&self.db, user, bot, id, &Value::Object(changes)).await {
            Ok(report) => {
                let verb = if id.is_some() { "更新" } else { "登録" };
                let state = if report.settings.enabled {
                    "有効"
                } else {
                    "無効"
                };
                Ok(ToolOutcome::from_payload(json!({
                    "success": true,
                    "message": format!(
                        "定期報告「{}」を{verb}しました📋（{state} / {}）",
                        report.settings.name, report.settings.schedule_cron
                    ),
                    "report": report_view(&report),
                })))
            }
            Err(SaveError::Invalid(message)) => Ok(fail(message)),
            Err(SaveError::NotFound) => Ok(fail("指定の定期報告は見つかりません。")),
            Err(SaveError::Db(e)) => Err(exec_err(e)),
        }
    }
}

// ─── deletePeriodicReport ────────────────────────────────────────────────────

struct DeletePeriodicReportTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for DeletePeriodicReportTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description:
                "定期報告を削除する（id か name で指定）。止めるだけなら savePeriodicReport で \
                enabled=false にする。"
                    .to_owned(),
            parameters_json_schema: json!({ "type": "object", "properties": target_properties() }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let report = match find_target(&self.db, ctx, &args).await? {
            Ok(report) => report,
            Err(outcome) => return Ok(outcome),
        };
        repo::delete_report(
            &self.db,
            ctx.user_id.as_str(),
            ctx.bot_id.as_str(),
            report.id,
        )
        .await
        .map_err(exec_err)?;
        Ok(ToolOutcome::from_payload(json!({
            "success": true,
            "message": format!("定期報告「{}」を削除しました。", report.settings.name),
        })))
    }
}

// ─── runPeriodicReportNow ────────────────────────────────────────────────────

/// 定期報告を今すぐ生成する。配信先は `deliver` 引数で選ぶ（issue #136）。
///
/// - 既定（`deliver` 省略/false）: 生成した報告を**この返信にインライン Embed で返す**。ただし非公開の
///   内容（タスク等）を含む報告は、ギルドの会話では表示せず DM への配信を案内する。
/// - `deliver: true`: 設定した配信先へテスト配信し、送信結果を返す。送信は [`DeliveryRunner`]
///   （`/api/periodic-reports/test` と同じ・定時配信と同じ生成・送信経路）へ委譲する。
struct RunPeriodicReportNowTool {
    name: ToolName,
    db: Db,
    runner: Arc<dyn DeliveryRunner>,
    aux_text: Arc<dyn AuxTextGenerator>,
}

#[async_trait]
impl Tool for RunPeriodicReportNowTool {
    fn declaration(&self) -> FunctionDeclaration {
        let mut properties = target_properties();
        properties.insert(
            "deliver".to_owned(),
            json!({
                "type": "boolean",
                "description": "true なら設定した配信先へテスト配信する。省略/false ならこの会話に表示する。"
            }),
        );
        FunctionDeclaration {
            name: self.name.clone(),
            description: "登録済みの定期報告を今すぐ生成する（id か name で指定）。\n\
                ・「朝報を出して」「今日のまとめを見せて」等は deliver を省略し、この会話に表示する。\n\
                ・「テスト配信して」「設定した送り先に届くか確かめたい」等は deliver=true にし、\
                設定した配信先（DM/チャンネル）へ送る。\n\
                ・どれがあるか分からなければ先に listPeriodicReports で確認する。"
                .to_owned(),
            parameters_json_schema: json!({ "type": "object", "properties": properties }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let report = match find_target(&self.db, ctx, &args).await? {
            Ok(report) => report,
            Err(outcome) => return Ok(outcome),
        };
        let name = report.settings.name.clone();

        if args.get("deliver").and_then(Value::as_bool) == Some(true) {
            let sent = self
                .runner
                .run_report(ctx.user_id.as_str(), ctx.bot_id.as_str(), report.id)
                .await;
            let message = if sent {
                format!("定期報告「{name}」を設定した配信先へテスト配信しました📋")
            } else {
                "定期報告の配信に失敗しました。配信先（DM/チャンネル）の設定と Bot の送信権限を確認してください。"
                    .to_owned()
            };
            let target = match report.settings.channel_target() {
                Some(id) => json!({ "type": "channel", "id": id }),
                None => json!({ "type": "dm" }),
            };
            return Ok(ToolOutcome::from_payload(json!({
                "success": sent,
                "message": message,
                "target": target,
            })));
        }

        // 非公開の内容は、ほかの人も見るギルドの会話には出さない。
        if ctx.guild_id.is_some() && report.settings.has_private() {
            return Ok(fail(format!(
                "定期報告「{name}」にはタスク・予定などの非公開の内容が含まれるため、この会話には表示できません。\
                 deliver=true で DM へ送ってください。"
            )));
        }
        let embed =
            compose::build_report_embed(&self.db, self.aux_text.as_ref(), &report, Local::now())
                .await
                .map_err(exec_err)?;
        Ok(ToolOutcome {
            payload: json!({ "success": true, "message": format!("定期報告「{name}」を生成しました📋") }),
            parts: vec![ResponsePart::Embed(embed)],
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use rusqlite::Connection;
    use yuuka_core::{BotId, GuildId, UserId};

    use super::*;
    use crate::model::is_likely_public_http_url;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    /// 実スキーマへ移行し、ツールの呼び出し元ユーザー（userA）を入れた DB。
    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_periodic_report_tools_{}_{seq}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let mut conn = Connection::open(&path).unwrap();
            yuuka_db::run_migrations(&mut conn).expect("migrate");
            conn.execute(
                "INSERT INTO users (discord_id, username, salt) VALUES ('userA', 'userA', 'x')",
                [],
            )
            .unwrap();
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
    async fn save_list_and_delete_reports() {
        let db = seed_db();
        let tools = tools(db).unwrap();
        let save = find(&tools, "savePeriodicReport");
        let list = find(&tools, "listPeriodicReports");
        let delete = find(&tools, "deletePeriodicReport");

        // 必須項目が足りない・非公開の内容をチャンネルへ → fail。
        for args in [
            json!({"name": "日報", "sections": ["tasks"]}),
            json!({"name": "日報", "schedule_cron": "0 22 * * *", "sections": ["tasks"],
                   "target_type": "channel", "target_id": "chan-1"}),
        ] {
            let out = save.call(&ctx(), args.clone()).await.unwrap();
            assert_eq!(out.payload["success"], false, "{args}");
        }

        let out = save
            .call(
                &ctx(),
                json!({"name": "朝報", "schedule_cron": "0 7 * * *", "sections": ["weather", "news"],
                       "latitude": 35.68, "longitude": 139.76,
                       "add_news_feed": "https://example.com/a.xml"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true, "{}", out.payload);
        let id = out.payload["report"]["id"].as_i64().unwrap();
        assert_eq!(out.payload["report"]["weather_lat"], 35.68);

        // フィード追加・部分一致削除（他の項目は保たれる）。
        let out = save
            .call(
                &ctx(),
                json!({"id": id, "add_news_feed": "https://example.com/b.xml"}),
            )
            .await
            .unwrap();
        assert_eq!(
            out.payload["report"]["news_feeds"],
            json!(["https://example.com/a.xml", "https://example.com/b.xml"])
        );
        let out = save
            .call(&ctx(), json!({"id": id, "remove_news_feed": "a.xml"}))
            .await
            .unwrap();
        assert_eq!(
            out.payload["report"]["news_feeds"],
            json!(["https://example.com/b.xml"])
        );
        assert_eq!(out.payload["report"]["schedule_cron"], "0 7 * * *");
        // 内部アドレスのフィードは保存前に拒否する。
        for feed in [
            "http://10.0.0.1/rss",
            "http://127.1/rss",
            "http://[::ffff:127.0.0.1]/rss",
        ] {
            let out = save
                .call(&ctx(), json!({"id": id, "add_news_feed": feed}))
                .await
                .unwrap();
            assert_eq!(out.payload["success"], false, "{feed}");
        }

        // 一覧は本人分だけ。
        let out = list.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["reports"].as_array().unwrap().len(), 1);
        let other = ToolContext::new(BotId::system_default(), UserId::new("userB"));
        let out = list.call(&other, json!({})).await.unwrap();
        assert_eq!(out.payload["reports"].as_array().unwrap().len(), 0);
        let out = delete.call(&other, json!({"id": id})).await.unwrap();
        assert_eq!(out.payload["success"], false);

        let out = delete.call(&ctx(), json!({"name": "朝報"})).await.unwrap();
        assert_eq!(out.payload["success"], true);
        let out = list.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["reports"].as_array().unwrap().len(), 0);
    }

    /// 呼ばれた (user, bot, id) を記録し、指定の送信結果を返す runner。
    struct RecordingRunner {
        result: bool,
        calls: std::sync::Mutex<Vec<(String, String, i64)>>,
    }

    impl RecordingRunner {
        fn new(result: bool) -> Arc<Self> {
            Arc::new(Self {
                result,
                calls: std::sync::Mutex::new(Vec::new()),
            })
        }

        fn calls(&self) -> Vec<(String, String, i64)> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl DeliveryRunner for RecordingRunner {
        async fn run_report(&self, user_id: &str, bot_id: &str, report_id: i64) -> bool {
            self.calls
                .lock()
                .unwrap()
                .push((user_id.to_owned(), bot_id.to_owned(), report_id));
            self.result
        }
    }

    /// 外部 HTTP を発生させない内容（タスク・会話トピック）の定期報告を入れる。
    async fn seed_report(tools: &[Arc<dyn Tool>]) -> i64 {
        let out = find(tools, "savePeriodicReport")
            .call(
                &ctx(),
                json!({"name": "日報", "schedule_cron": "0 22 * * *", "sections": ["tasks", "topics"]}),
            )
            .await
            .unwrap();
        out.payload["report"]["id"].as_i64().unwrap()
    }

    #[tokio::test]
    async fn run_now_deliver_sends_through_runner() {
        for result in [true, false] {
            let db = seed_db();
            let runner = RecordingRunner::new(result);
            let tools = tools_with(db, runner.clone()).unwrap();
            let id = seed_report(&tools).await;
            let run = find(&tools, "runPeriodicReportNow");

            let out = run
                .call(&ctx(), json!({"name": "日報", "deliver": true}))
                .await
                .unwrap();

            // 本人・呼び出し中の Bot・対象 id で runner（/test と同じ配信経路）へ委譲する。
            assert_eq!(
                runner.calls(),
                vec![("userA".to_owned(), "system_default".to_owned(), id)]
            );
            assert_eq!(out.payload["success"], result);
            assert_eq!(out.payload["target"]["type"], "dm");
            let message = out.payload["message"].as_str().unwrap();
            assert!(
                message.contains(if result {
                    "テスト配信しました"
                } else {
                    "配信に失敗しました"
                }),
                "{message}"
            );
            // 配信先へ送るので、会話にはインライン Embed を出さない。
            assert!(out.parts.is_empty());
        }
    }

    #[tokio::test]
    async fn run_now_inline_embed_and_guild_privacy() {
        let db = seed_db();
        let runner = RecordingRunner::new(true);
        let tools = tools_with(db, runner.clone()).unwrap();
        let id = seed_report(&tools).await;
        let run = find(&tools, "runPeriodicReportNow");

        // 既定はこの会話にインライン Embed で返す（配信しない）。
        let out = run.call(&ctx(), json!({"id": id})).await.unwrap();
        assert_eq!(out.payload["success"], true);
        let embed = match out.parts.as_slice() {
            [ResponsePart::Embed(embed)] => Some(embed),
            _ => None,
        }
        .expect("embed expected");
        assert!(embed
            .title
            .as_deref()
            .is_some_and(|t| t.starts_with("📋 日報 ")));
        assert!(embed
            .description
            .as_deref()
            .is_some_and(|d| d.contains("✅ 完了タスク: 0件")));
        assert!(runner.calls().is_empty());

        // ギルドの会話では非公開の内容を出さない。
        let mut guild = ctx();
        guild.guild_id = Some(GuildId::new("guild-1".to_owned()));
        let out = run.call(&guild, json!({"id": id})).await.unwrap();
        assert_eq!(out.payload["success"], false);
        assert!(out.parts.is_empty());

        // 無い id・未指定は fail。
        let out = run.call(&ctx(), json!({"id": id + 1})).await.unwrap();
        assert_eq!(out.payload["success"], false);
        let out = run.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["success"], false);
    }
}
