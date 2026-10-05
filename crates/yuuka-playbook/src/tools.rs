//! playbook ドメインの Native ツール（現行 `src/functions/playbookFunctions.ts` の移植・§9.4）。
//!
//! 雛形は yuuka-todo/src/tools.rs（構造・命名・ok/fail JSON・bare ツール名・UserScope・
//! DbError→ToolError::Execution・`tools(db)->Result<Vec<Arc<dyn Tool>>>` を踏襲）。
//!
//! **wire 契約**: HTTP route の body は snake/camel だが、**tool 引数は Node 宣言と同名**
//! （playbook は元から単語キー name/title/keywords/description/steps/query＝snake 相当）。
//! ツール名は Node system prompt が参照する **bare 名**（`savePlaybook` 等・namespace 無し）。
//!
//! 移植済み: savePlaybook / findPlaybooks / runPlaybook / deletePlaybook（repo の save/list/get/delete）。
//! 未移植（後続・repo 外の依存要）: getRecentActionHistory（actionRecorder サービス依存・repo メソッド無し）。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use yuuka_core::tool::FunctionDeclaration;
use yuuka_core::{
    ActionRecorder, DbError, Tool, ToolContext, ToolError, ToolName, ToolOutcome, UserScope,
};
use yuuka_web::Db;

use crate::dto::NewPlaybook;
use crate::repo::PlaybookRepo;

/// このドメインが公開する Native ツール一式を作る。
///
/// assembly 層（bot/WS）が `NativeProvider::register` で束ねる。`recorder` は操作履歴
/// レコーダー（`getRecentActionHistory` が読む・FC ループが書く）。`None` の場合は履歴が常に空
/// （記録が無効の環境）として振る舞う。
///
/// # Errors
/// ツール名が Gemini 制約に反する場合 [`ToolError`]（コンパイル時定数なので通常発生しない）。
pub fn tools(
    db: Db,
    recorder: Option<Arc<ActionRecorder>>,
) -> Result<Vec<Arc<dyn Tool>>, ToolError> {
    Ok(vec![
        Arc::new(SavePlaybookTool {
            name: ToolName::checked("savePlaybook".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(FindPlaybooksTool {
            name: ToolName::checked("findPlaybooks".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(RunPlaybookTool {
            name: ToolName::checked("runPlaybook".to_owned())?,
            db: db.clone(),
        }),
        Arc::new(DeletePlaybookTool {
            name: ToolName::checked("deletePlaybook".to_owned())?,
            db,
        }),
        Arc::new(GetRecentActionHistoryTool {
            name: ToolName::checked("getRecentActionHistory".to_owned())?,
            recorder,
        }),
    ])
}

// ─── 共通ヘルパ ───────────────────────────────────────────────────────────────

/// `{success:true, message, ...extra}`（Node `ok(msg, extra)`）。
fn ok_payload(message: impl Into<String>, extra: Value) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("success".to_owned(), Value::Bool(true));
    obj.insert("message".to_owned(), Value::String(message.into()));
    if let Value::Object(map) = extra {
        for (k, v) in map {
            obj.insert(k, v);
        }
    }
    Value::Object(obj)
}

/// `{success:false, message}`（Node `fail(msg)`）。実行エラーではなく「妥当だが失敗」な結果。
fn fail_payload(message: impl Into<String>) -> ToolOutcome {
    ToolOutcome::from_payload(json!({ "success": false, "message": message.into() }))
}

/// ctx からデータ分離スコープを組む。
fn scope_of(ctx: &ToolContext) -> UserScope {
    UserScope::new(ctx.user_id.clone(), ctx.bot_id.clone())
}

/// `String(args[key] ?? "").trim()`（trim 後空なら None）。
fn arg_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// `Array.isArray(x) ? x.map(String) : []`（文字列要素はそのまま、他は JSON 表現へ）。
fn arg_string_array(args: &Value, key: &str) -> Vec<String> {
    match args.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| v.to_string())
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// DbError をツール実行エラーへ（握り潰さず Gemini へ `{success:false}` として返る・§8.4）。
fn exec_err(e: DbError) -> ToolError {
    ToolError::Execution(e.to_string())
}

// ─── savePlaybook ────────────────────────────────────────────────────────────

struct SavePlaybookTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for SavePlaybookTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "操作の手順をマクロ（Playbook）として保存し、あとで呼び出せるようにする。\n\
                ・例:「この手順を覚えておいて」「『〜〜』という名前で保存して」。\n\
                ・保存する前に、呼び出し名・説明・手順の内容をユーザーに見せて承認を得てから呼ぶ。\n\
                ・ユーザーが手順を言葉で説明した時は、その内容を整理して保存する。\n\
                ・直前にBotがやった操作を覚える時は、先に getRecentActionHistory で操作履歴を取得し、それを手順にまとめてから保存する。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "マクロの英数字のファイル名。例: 'example_login', 'morning_check'。" },
                    "title": { "type": "string", "description": "マクロの分かりやすい日本語タイトル（呼び出し名）。例: '朝の確認', 'サンプルサイトのログインと請求書取得'。" },
                    "keywords": { "type": "array", "items": { "type": "string" }, "description": "次に呼び出す時に見つけやすくする関連キーワードのリスト。例: ['朝', '確認', 'ニュース']。" },
                    "description": { "type": "string", "description": "このマクロが何をするものかの簡単な説明。" },
                    "steps": { "type": "string", "description": "Markdown形式の具体的な操作手順。使うツール名（browserInteractiveOpen, browserFillCredential など）や判断の条件を書いておくと、後で再実行する時の正確さが上がる。" }
                },
                "required": ["name", "title", "keywords", "description", "steps"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let name = arg_str(&args, "name");
        let title = arg_str(&args, "title");
        let steps = arg_str(&args, "steps");
        let (Some(name), Some(title), Some(steps)) = (name, title, steps) else {
            return Ok(fail_payload("name・title・steps は必須です。"));
        };

        let new = NewPlaybook {
            name,
            title,
            keywords: arg_string_array(&args, "keywords"),
            description: arg_str(&args, "description").unwrap_or_default(),
            steps,
        };

        let saved = PlaybookRepo::new(&self.db)
            .save(&scope_of(ctx), new)
            .await
            .map_err(exec_err)?;

        let message = format!(
            "マクロ「{}」を {} として正常に保存しました。",
            saved.title, saved.name
        );
        Ok(ToolOutcome::from_payload(ok_payload(message, json!({}))))
    }
}

// ─── findPlaybooks ───────────────────────────────────────────────────────────

struct FindPlaybooksTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for FindPlaybooksTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "登録済みマクロ（Playbook）を一覧、またはキーワードで検索する（手順の中身も返る）。\n\
                ・ブラウザ操作や作業の自動化を頼まれた時、使えるマクロが既にないか最初に確認する目的で使う。\n\
                ・ユーザーが呼び出し名っぽい短い言葉（例:「朝の確認」）を送った時も、まずこれで探す。\n\
                ・見つかったら実行内容を要約してユーザーに確認し、承認を得てから runPlaybook で実行する。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "検索するキーワードや一部の文字列。例: 'ログイン', 'でんき'。省略=全マクロの一覧を返す。" }
                }
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let query = arg_str(&args, "query");
        let playbooks = PlaybookRepo::new(&self.db)
            .list(&scope_of(ctx), query.clone())
            .await
            .map_err(exec_err)?;

        if playbooks.is_empty() {
            let message = match &query {
                Some(q) => format!("「{q}」に合致するマクロは見つかりませんでした。"),
                None => "登録済みのマクロはありません。".to_owned(),
            };
            return Ok(ToolOutcome::from_payload(json!({
                "success": true,
                "count": 0,
                "message": message,
                "playbooks": [],
            })));
        }

        Ok(ToolOutcome::from_payload(json!({
            "success": true,
            "count": playbooks.len(),
            "playbooks": playbooks,
        })))
    }
}

// ─── runPlaybook ─────────────────────────────────────────────────────────────

struct RunPlaybookTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for RunPlaybookTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "指定した名前のマクロ（Playbook）の手順を取り出して実行できるようにする。\n\
                ・ユーザーが実行を承認した後で呼ぶ。\n\
                ・返ってきた手順（steps）の通りに、各ツールを順番どおり実行する。\n\
                ・途中のステップが失敗したら止めて、どのステップで何が起きたかをユーザーに正直に報告する。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "実行するマクロの英数字名。findPlaybooks の結果に入っている name を使う。" }
                },
                "required": ["name"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(name) = arg_str(&args, "name") else {
            return Ok(fail_payload("name は必須です。"));
        };
        match PlaybookRepo::new(&self.db)
            .get(&scope_of(ctx), name.clone())
            .await
            .map_err(exec_err)?
        {
            Some(playbook) => Ok(ToolOutcome::from_payload(ok_payload(
                "以下の手順に厳密に従って、各ツールを順番に実行してください。失敗した場合は中断して正直に報告すること。",
                json!({ "playbook": playbook }),
            ))),
            None => Ok(fail_payload(format!(
                "マクロ「{name}」が見つかりません。findPlaybooks で正しい name を確認してください。"
            ))),
        }
    }
}

// ─── deletePlaybook ──────────────────────────────────────────────────────────

struct DeletePlaybookTool {
    name: ToolName,
    db: Db,
}

#[async_trait]
impl Tool for DeletePlaybookTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "マクロ（Playbook）を削除する（元に戻せない）。\n\
                ・削除する前に、消す対象のタイトルをユーザーに確認してから呼ぶ。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "削除するマクロの英数字名。" }
                },
                "required": ["name"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        let Some(name) = arg_str(&args, "name") else {
            return Ok(fail_payload("name は必須です。"));
        };
        let deleted = PlaybookRepo::new(&self.db)
            .delete(&scope_of(ctx), name.clone())
            .await
            .map_err(exec_err)?;
        if deleted {
            Ok(ToolOutcome::from_payload(ok_payload(
                format!("マクロ「{name}」を削除しました🗑️"),
                json!({}),
            )))
        } else {
            Ok(fail_payload(format!(
                "マクロ「{name}」が見つかりませんでした。"
            )))
        }
    }
}

// ─── getRecentActionHistory ──────────────────────────────────────────────────

/// 直近の FC 操作履歴を取り出す（Node `getRecentActionHistory`）。実行ベースのマクロ登録準備。
///
/// 履歴は [`ActionRecorder`]（プロセス内・TTL 2h）から読む。`recorder` が `None`、または記録が無い
/// 場合は空扱い（Node の「履歴なし」分岐と同じ案内文）。
struct GetRecentActionHistoryTool {
    name: ToolName,
    recorder: Option<Arc<ActionRecorder>>,
}

#[async_trait]
impl Tool for GetRecentActionHistoryTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "このユーザーとの会話で直近にやったツール操作の履歴を取得する（操作をマクロ化する準備に使う）。\n\
                ・例:「今の操作を覚えておいて」「これを記憶して」。\n\
                ・取得した履歴を手順としてMarkdownにまとめ、マクロ候補（呼び出し名・説明・手順）をユーザーに見せ、承認を得てから savePlaybook で保存する。"
                .to_owned(),
            parameters_json_schema: json!({ "type": "object", "properties": {} }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, _args: Value) -> Result<ToolOutcome, ToolError> {
        let actions = self
            .recorder
            .as_ref()
            .map(|r| r.recent(ctx.user_id.as_str()))
            .unwrap_or_default();

        if actions.is_empty() {
            return Ok(ToolOutcome::from_payload(ok_payload(
                "直近の操作履歴がありません（履歴は2時間で揮発します）。ユーザーに手順を説明してもらい、説明ベースで登録してください。",
                json!({ "count": 0, "actions": [] }),
            )));
        }

        let count = actions.len();
        // RecordedAction は Serialize 導出済み（失敗し得ないが lint 準拠で空配列フォールバック）。
        let actions_json =
            serde_json::to_value(&actions).unwrap_or_else(|_| Value::Array(Vec::new()));
        Ok(ToolOutcome::from_payload(ok_payload(
            "直近の操作履歴です。この履歴から再実行可能な手順をMarkdownに要約し、マクロ候補（呼び出し名・説明・手順）をユーザーに提示して、承認を得てから savePlaybook で保存してください。",
            json!({ "count": count, "actions": actions_json }),
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use rusqlite::Connection;
    use yuuka_core::{BotId, UserId};

    use super::*;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    const PLAYBOOKS_DDL: &str = "CREATE TABLE playbooks (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        name TEXT NOT NULL,
        title TEXT NOT NULL,
        keywords TEXT DEFAULT '[]',
        description TEXT DEFAULT '',
        steps TEXT NOT NULL DEFAULT '',
        created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        UNIQUE(user_id, bot_id, name)
    );";

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_playbook_tools_{}_{seq}.sqlite",
            std::process::id()
        ));
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(PLAYBOOKS_DDL).unwrap();
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
    async fn save_find_run_delete_roundtrip() {
        let db = seed_db();
        let tools = tools(db, None).unwrap();

        // 宣言名は bare（Node system prompt と一致）。
        let names: Vec<String> = tools
            .iter()
            .map(|t| t.declaration().name.to_string())
            .collect();
        assert!(names.contains(&"savePlaybook".to_owned()));
        assert!(!names.iter().any(|n| n.contains(':')), "native は bare 名");

        // save（name は正規化・空白→_・小文字化）。
        let save = find(&tools, "savePlaybook");
        let out = save
            .call(
                &ctx(),
                json!({
                    "name": "Morning Routine",
                    "title": "朝の準備",
                    "keywords": ["朝"],
                    "description": "d",
                    "steps": "step one"
                }),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert!(out.payload["message"]
            .as_str()
            .unwrap()
            .contains("morning_routine"));

        // find（query 無し）で 1 件見える。
        let list = find(&tools, "findPlaybooks");
        let out = list.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["count"], 1);
        assert_eq!(out.payload["playbooks"][0]["name"], "morning_routine");
        assert_eq!(out.payload["playbooks"][0]["keywords"][0], "朝");

        // find（query マッチ）。
        let out = list
            .call(&ctx(), json!({"query": "morning"}))
            .await
            .unwrap();
        assert_eq!(out.payload["count"], 1);

        // run（get）で手順が返る。
        let run = find(&tools, "runPlaybook");
        let out = run
            .call(&ctx(), json!({"name": "morning_routine"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["playbook"]["steps"], "step one");

        // run（不在）は success:false。
        let out = run.call(&ctx(), json!({"name": "nope"})).await.unwrap();
        assert_eq!(out.payload["success"], false);

        // delete。
        let delete = find(&tools, "deletePlaybook");
        let out = delete
            .call(&ctx(), json!({"name": "morning_routine"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        // 二重削除は not-found（success:false）。
        let out = delete
            .call(&ctx(), json!({"name": "morning_routine"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);
    }

    #[tokio::test]
    async fn save_validates_required_fields() {
        let db = seed_db();
        let tools = tools(db, None).unwrap();
        let save = find(&tools, "savePlaybook");

        // title 欠落 → fail。
        let out = save
            .call(&ctx(), json!({"name": "pb", "steps": "s"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);

        // steps 欠落 → fail。
        let out = save
            .call(&ctx(), json!({"name": "pb", "title": "t"}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);
    }

    #[tokio::test]
    async fn find_empty_reports_message() {
        let db = seed_db();
        let tools = tools(db, None).unwrap();
        let list = find(&tools, "findPlaybooks");

        // 無し・query 無し。
        let out = list.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["count"], 0);
        assert_eq!(out.payload["message"], "登録済みのマクロはありません。");

        // 無し・query 有り。
        let out = list.call(&ctx(), json!({"query": "x"})).await.unwrap();
        assert_eq!(out.payload["count"], 0);
        assert!(out.payload["message"]
            .as_str()
            .unwrap()
            .contains("合致するマクロは見つかりませんでした"));
    }

    #[tokio::test]
    async fn scope_isolation_across_users() {
        let db = seed_db();
        let tools = tools(db, None).unwrap();
        let save = find(&tools, "savePlaybook");
        let list = find(&tools, "findPlaybooks");

        save.call(&ctx(), json!({"name": "pb", "title": "t", "steps": "s"}))
            .await
            .unwrap();

        // 別ユーザーには見えない。
        let ctx_b = ToolContext::new(BotId::system_default(), UserId::new("userB"));
        let out = list.call(&ctx_b, json!({})).await.unwrap();
        assert_eq!(out.payload["count"], 0);
    }

    #[tokio::test]
    async fn recent_action_history_reads_recorder() {
        let db = seed_db();
        let recorder = Arc::new(ActionRecorder::new());
        let tools = tools(db, Some(Arc::clone(&recorder))).unwrap();
        let hist = find(&tools, "getRecentActionHistory");

        // 記録が無ければ「履歴なし」分岐（count 0・案内文）。
        let out = hist.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["count"], 0);
        assert!(out.payload["message"]
            .as_str()
            .unwrap()
            .contains("直近の操作履歴がありません"));

        // レコーダへ 2 件記録（getRecentActionHistory 自身は除外されるので数に入らない）。
        recorder.record("userA", "addTodo", &json!({"title": "牛乳"}));
        recorder.record("userA", "getRecentActionHistory", &json!({}));
        recorder.record("userA", "addExpense", &json!({"amount": 300}));

        let out = hist.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["count"], 2);
        assert_eq!(out.payload["actions"][0]["name"], "addTodo");
        assert_eq!(out.payload["actions"][1]["name"], "addExpense");
        assert_eq!(out.payload["actions"][0]["argsSummary"], "title=牛乳");

        // 別ユーザーには見えない（recorder は user 単位）。
        let ctx_b = ToolContext::new(BotId::system_default(), UserId::new("userB"));
        let out = hist.call(&ctx_b, json!({})).await.unwrap();
        assert_eq!(out.payload["count"], 0);
    }

    #[tokio::test]
    async fn recent_action_history_empty_without_recorder() {
        let db = seed_db();
        let tools = tools(db, None).unwrap();
        let hist = find(&tools, "getRecentActionHistory");
        let out = hist.call(&ctx(), json!({})).await.unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.payload["count"], 0);
    }
}
