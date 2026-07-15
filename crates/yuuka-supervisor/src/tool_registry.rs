//! 全ドメインの Native ツールを 1 つの [`ToolRegistry`] へ組み上げる assembly 点（§9.3/§9.4）。
//!
//! Phase 2 の合流点: 各ドメインクレートが公開する `tools(db)` を集約し、`NativeProvider` に
//! 登録して `ToolRegistry` を作る。bot（Phase 3）/WS はこの registry を `snapshot(ctx)` して
//! gemini の function-calling ループへ `&dyn ToolProvider` として渡す。MCP/WASM provider は
//! 後続で `registry.register(Arc<dyn ToolProvider>)` を足すだけで透過的に合流する。

use std::sync::Arc;

use yuuka_core::{ActionRecorder, Tool, ToolError};
use yuuka_crypto::SystemCrypto;
use yuuka_tools::{NativeProvider, ToolRegistry};
use yuuka_web::Db;

/// 全ドメインの Native ツールを登録した [`NativeProvider`] を作る。
///
/// `recorder` は操作履歴レコーダー（`getRecentActionHistory` が読む）。FC ループにも同じ Arc を渡す。
///
/// # Errors
/// ドメインの `tools(db)` 構築失敗、または名前重複時に [`ToolError`]。
pub fn build_native_provider(
    db: &Db,
    crypto: Option<Arc<SystemCrypto>>,
    recorder: Option<Arc<ActionRecorder>>,
) -> Result<NativeProvider, ToolError> {
    let mut provider = NativeProvider::new();
    for tool in all_domain_tools(db, crypto, recorder)? {
        provider.register(tool)?;
    }
    Ok(provider)
}

/// 全ドメインの Native ツールを登録した [`ToolRegistry`]（gemini ループへ snapshot して渡す）。
///
/// # Errors
/// [`build_native_provider`] と同じ。
pub fn build_tool_registry(
    db: &Db,
    crypto: Option<Arc<SystemCrypto>>,
    recorder: Option<Arc<ActionRecorder>>,
) -> Result<ToolRegistry, ToolError> {
    let provider = build_native_provider(db, crypto, recorder)?;
    Ok(ToolRegistry::new().with_provider(Arc::new(provider)))
}

/// 各ドメインクレートの `tools(db)` を 1 本のリストへ集約する。
///
/// 新ドメイン/新ツールはここに 1 行足す（依存の向きは supervisor → domains のまま）。
///
/// # Errors
/// いずれかのドメインの `tools(db)` が失敗した場合 [`ToolError`]。
fn all_domain_tools(
    db: &Db,
    crypto: Option<Arc<SystemCrypto>>,
    recorder: Option<Arc<ActionRecorder>>,
) -> Result<Vec<Arc<dyn Tool>>, ToolError> {
    let mut all: Vec<Arc<dyn Tool>> = Vec::new();
    all.extend(yuuka_todo::tools(db.clone())?);
    all.extend(yuuka_finance::tools(db.clone())?);
    all.extend(yuuka_schedule::tools(db.clone())?);
    all.extend(yuuka_timeline::tools(db.clone())?);
    all.extend(yuuka_reminder::tools(db.clone())?);
    all.extend(yuuka_personal::tools(db.clone())?);
    all.extend(yuuka_credential::tools(db.clone(), crypto)?);
    all.extend(yuuka_playbook::tools(db.clone(), recorder)?);
    // guild-assistant（汎用モード）ツール。露出は Tool::exposure（guild_assistant + memory 能力）で
    // 選別されるため、秘書経路のスナップショットには現れない。
    all.extend(yuuka_botassistant::tools(db.clone())?);
    all.extend(yuuka_briefing::tools(db.clone())?);
    // 会話ログ要約（秘書経路・memory 能力）。
    all.extend(yuuka_conversation::tools(db.clone())?);
    // リッチ返信 Embed（core・秘書/汎用モード両経路で常時露出・db 非依存）。
    all.extend(yuuka_richcontent::tools()?);
    // browser 系（secretary・chromium CLI + reqwest/scraper・db 非依存）。searchWeb/fetchDynamicPage/
    // takePageScreenshot。対話セッション 6 本は後続増分。
    all.extend(yuuka_browser::tools()?);
    // persona はツール関数を持たない（web 管理）。
    Ok(all)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use rusqlite::Connection;
    use serde_json::json;
    use yuuka_core::{BotId, ToolContext, ToolName, ToolProvider, UserId};
    use yuuka_gemini::{run_function_calling_loop, Content, GenerateContentResponse, LoopOptions};

    use super::*;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    // 参照系ツールの dispatch に必要な最小スキーマ（addTodo の縦スライス検証用に todos のみ）。
    const DDL: &str = "CREATE TABLE todos (
        id INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default', title TEXT NOT NULL, description TEXT,
        due_date TEXT, start_date TEXT, priority TEXT, tags TEXT NOT NULL DEFAULT '[]',
        status TEXT NOT NULL DEFAULT 'open', progress INTEGER NOT NULL DEFAULT 0,
        parent_id INTEGER, linked_payment_id INTEGER, due_reminded INTEGER NOT NULL DEFAULT 0,
        repeat_rule TEXT, repeat_until TEXT, repeat_count INTEGER,
        created_at TEXT NOT NULL, updated_at TEXT NOT NULL);";

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_registry_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(DDL).unwrap();
        }
        Db::open(&path).unwrap()
    }

    fn ctx() -> ToolContext {
        // 秘書 Bot 相当（能力ゲートで secretary ツールが露出するように）。
        let mut c = ToolContext::new(BotId::system_default(), UserId::new("userA"));
        c.capabilities = yuuka_core::CapabilitySet::from_granted(vec!["secretary".to_owned()]);
        c
    }

    #[test]
    fn registry_aggregates_all_domains_without_name_collision() {
        let db = seed_db();
        // build_native_provider が Ok = 全ドメイン横断で重複ツール名が無いことの保証。
        let provider = build_native_provider(&db, None, None).unwrap();
        let decls = provider.list(&ctx());
        let names: Vec<String> = decls.iter().map(|d| d.name.to_string()).collect();

        // 代表的な各ドメインのツールが揃っている。
        for expected in [
            "addTodo",
            "addExpense",
            "addSchedule",
            "addTimelineRecord",
            "addReminder",
            "addContact",
            "listCredentialServices",
        ] {
            assert!(
                names.contains(&expected.to_owned()),
                "missing tool: {expected}"
            );
        }
        // native は bare 名（namespace 無し）。
        assert!(!names.iter().any(|n| n.contains(':')));
        assert!(
            decls.len() >= 14,
            "8 ドメイン分のツールが集約されている: {}",
            decls.len()
        );
    }

    /// canned レスポンスを返す fake backend（GenerateBackend は pub なので外部クレートで実装可）。
    struct FakeBackend {
        scripted: std::sync::Mutex<std::collections::VecDeque<GenerateContentResponse>>,
    }

    #[async_trait::async_trait]
    impl yuuka_gemini::GenerateBackend for FakeBackend {
        async fn generate(
            &self,
            _sys: Option<&str>,
            _decls: &[yuuka_gemini::FunctionDeclaration],
            _contents: &[Content],
            _tc: Option<yuuka_gemini::ToolConfig>,
        ) -> Result<GenerateContentResponse, yuuka_core::GeminiError> {
            self.scripted
                .lock()
                .unwrap()
                .pop_front()
                .ok_or(yuuka_core::GeminiError::MaxIterations)
        }
    }

    #[tokio::test]
    async fn end_to_end_gemini_loop_dispatches_real_domain_tool() {
        // ★Phase 2 全鎖の統合検証: gemini FC ループ → RegistrySnapshot → NativeProvider →
        //   yuuka-todo の addTodo ツール → TodoRepo → 実 SQLite。
        let db = seed_db();
        let registry = build_tool_registry(&db, None, None).unwrap();
        let snapshot = registry.snapshot(&ctx());

        let resp = |v: serde_json::Value| -> GenerateContentResponse {
            serde_json::from_value(v).unwrap()
        };
        let backend = FakeBackend {
            scripted: std::sync::Mutex::new(
                vec![
                    // モデルが addTodo を呼ぶ。
                    resp(json!({"candidates":[{"content":{"role":"model","parts":[
                        {"functionCall":{"name":"addTodo","args":{"title":"牛乳を買う"},"id":"c1"}}
                    ]}}]})),
                    // ツール結果を受けて最終テキスト。
                    resp(json!({"candidates":[{"content":{"role":"model","parts":[
                        {"text":"ToDo を追加しました。"}
                    ]}}]})),
                ]
                .into_iter()
                .collect(),
            ),
        };

        let mut contents = vec![Content::user_text("牛乳を買うのを忘れないで")];
        let result = run_function_calling_loop(
            &backend,
            &snapshot,
            "sys",
            &mut contents,
            &ctx(),
            &LoopOptions::default(),
        )
        .await
        .unwrap();

        assert_eq!(result.tool_calls, vec!["addTodo".to_owned()]);
        assert_eq!(result.text, "ToDo を追加しました。");

        // ★実 DB に todo が作られたことを直接確認（ツールが本当に repo を叩いた）。
        let scope = yuuka_core::UserScope::new(UserId::new("userA"), BotId::system_default());
        let todos = yuuka_todo::repo::TodoRepo::new(&db)
            .list_tree(&scope, None, None)
            .await
            .unwrap();
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].title, "牛乳を買う");

        // snapshot は addTodo を解決できる。
        assert!(snapshot.has(&ToolName::checked("addTodo".to_owned()).unwrap()));
    }
}
