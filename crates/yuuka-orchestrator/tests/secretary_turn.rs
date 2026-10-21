//! P1-2 会話オーケストレーションの end-to-end 統合テスト（fake Gemini backend・実 SQLite）。
//!
//! 検証: ユーザー発言の永続化 → 直近履歴ロード → システムプロンプト組立 → 暗号化済みユーザー鍵の
//! 復号 → FC ループ（tools 無し）→ アシスタント応答の永続化 → [`TurnReply`]。ネットワーク不要。
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use secrecy::SecretString;
use serde_json::json;
use yuuka_core::{
    BotId, GeminiError, Tool, ToolContext, ToolError, ToolExposure, ToolName, ToolOutcome, UserId,
};
use yuuka_crypto::SystemCrypto;
use yuuka_discord::{BotStatus, IncomingChat, StatusSink};
use yuuka_gemini::{
    Content, FunctionDeclaration, GenerateBackend, GenerateContentResponse, ToolConfig,
};
use yuuka_orchestrator::{message_log, ChatEngine, GeminiFactory};
use yuuka_tools::{NativeProvider, ToolRegistry};
use yuuka_web::Db;

static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 事前計画（R06）の呼び出しか。計画は本編の応答・記録の対象にしない。
fn is_planner_call(system_instruction: Option<&str>) -> bool {
    system_instruction
        .is_some_and(|s| s.starts_with(yuuka_orchestrator::turn_plan::PLANNER_INSTRUCTION))
}

/// 計画に失敗した応答（計画なし＝能力で絞ったツールのまま本編を進める）。
fn no_plan() -> GenerateContentResponse {
    serde_json::from_value(json!({
        "candidates": [ { "content": { "role": "model", "parts": [ { "text": "{}" } ] } } ]
    }))
    .unwrap()
}

/// migrations 適用済みの一時 DB（本番 open は CREATE しないので先にファイルを作る）。
fn fresh_db() -> (Db, std::path::PathBuf) {
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("yuuka_orch_it_{}_{n}.sqlite", std::process::id()));
    {
        rusqlite::Connection::open(&path).expect("seed file");
    }
    let db = Db::open(&path).expect("open db");
    (db, path)
}

/// 固定テキストを 1 度だけ返す fake backend（tools 無しの単発ターン）。
struct FakeBackend {
    responses: Mutex<std::collections::VecDeque<GenerateContentResponse>>,
}

#[async_trait]
impl GenerateBackend for FakeBackend {
    async fn generate(
        &self,
        system_instruction: Option<&str>,
        _declarations: &[FunctionDeclaration],
        _contents: &[Content],
        _tool_config: Option<ToolConfig>,
    ) -> Result<GenerateContentResponse, GeminiError> {
        if is_planner_call(system_instruction) {
            return Ok(no_plan());
        }
        Ok(self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("fake backend ran out of responses"))
    }
}

/// 固定テキストを返す backend を生成するファクトリ。
struct FakeFactory {
    text: String,
}

impl GeminiFactory for FakeFactory {
    fn build(
        &self,
        _model: &str,
        _api_key: SecretString,
    ) -> Result<Arc<dyn GenerateBackend>, GeminiError> {
        let resp: GenerateContentResponse = serde_json::from_value(json!({
            "candidates": [ { "content": { "role": "model", "parts": [ { "text": self.text } ] } } ]
        }))
        .expect("valid response");
        let mut q = std::collections::VecDeque::new();
        q.push_back(resp);
        Ok(Arc::new(FakeBackend {
            responses: Mutex::new(q),
        }))
    }
}

/// システム鍵 crypto を作り、ユーザーの Gemini キーを暗号化して users 行を seed する。
fn seed_user_with_key(path: &std::path::Path, crypto: &SystemCrypto, discord_id: &str) {
    let enc = crypto.encrypt_text("fake-gemini-key").expect("encrypt");
    let conn = rusqlite::Connection::open(path).expect("open seed");
    conn.execute(
        "INSERT INTO users (discord_id, username, salt, role, \
         gemini_api_key_encrypted, gemini_api_key_iv, gemini_api_key_tag, gemini_model) \
         VALUES (?1, 'yuu', '00', 'user', ?2, ?3, ?4, 'gemini-3.1-flash-lite')",
        rusqlite::params![discord_id, enc.encrypted, enc.iv, enc.auth_tag],
    )
    .expect("seed user");
}

/// users 行だけ seed（Gemini キー無し）。
fn seed_user_no_key(path: &std::path::Path, discord_id: &str) {
    let conn = rusqlite::Connection::open(path).expect("open seed");
    conn.execute(
        "INSERT INTO users (discord_id, username, salt, role) \
         VALUES (?1, 'yuu', '00', 'user')",
        rusqlite::params![discord_id],
    )
    .expect("seed user");
}

fn count_logs(path: &std::path::Path, user_id: &str, role: &str) -> i64 {
    let conn = rusqlite::Connection::open(path).expect("open");
    conn.query_row(
        "SELECT COUNT(*) FROM message_logs WHERE user_id = ?1 AND role = ?2",
        rusqlite::params![user_id, role],
        |r| r.get(0),
    )
    .expect("count")
}

/// 保存済みログ本文を古い順で返す（#36: 空文字保存の回帰検証に使う）。
fn log_contents(path: &std::path::Path, user_id: &str, role: &str) -> Vec<String> {
    let conn = rusqlite::Connection::open(path).expect("open");
    let mut stmt = conn
        .prepare("SELECT content FROM message_logs WHERE user_id = ?1 AND role = ?2 ORDER BY id")
        .expect("prepare");
    stmt.query_map(rusqlite::params![user_id, role], |r| r.get(0))
        .expect("query")
        .collect::<Result<Vec<String>, _>>()
        .expect("rows")
}

fn engine_with(db: Db, crypto: Option<Arc<SystemCrypto>>, text: &str) -> ChatEngine {
    ChatEngine::new(
        db,
        crypto,
        ToolRegistry::new(),
        Arc::new(FakeFactory {
            text: text.to_owned(),
        }),
        None,
        Arc::new(yuuka_mcp::NullMcpClient),
        None,
    )
}

fn null_sink() -> StatusSink {
    Arc::new(|_: BotStatus| {})
}

/// OCR と秘書ターンで同じユーザー鍵・モデルを使い、OCR はツールを実行しない。
struct ReceiptBackend {
    calls: Mutex<Vec<(Vec<String>, Vec<Content>)>>,
    readable: bool,
    is_receipt: bool,
}

#[async_trait]
impl GenerateBackend for ReceiptBackend {
    async fn generate(
        &self,
        _system_instruction: Option<&str>,
        declarations: &[FunctionDeclaration],
        contents: &[Content],
        _tool_config: Option<ToolConfig>,
    ) -> Result<GenerateContentResponse, GeminiError> {
        let names = declarations
            .iter()
            .map(|d| d.name.clone())
            .collect::<Vec<_>>();
        let is_ocr = names == ["readReceipt"];
        self.calls.lock().unwrap().push((names, contents.to_vec()));
        let parts = if is_ocr {
            json!([{"functionCall": {"name": "readReceipt", "args": {
                "isReceipt": self.is_receipt, "date": "2026-10-20", "kind": "expense",
                "category": "食費", "description": if self.readable { "店: 卵" } else { "" },
                "amount": if self.readable { 600 } else { 0 },
                "lines": if self.readable { json!([{"description": "卵", "category": "食費", "amount": 600}]) } else { json!([]) }
            }}}])
        } else {
            json!([{"text": "読み取り内容を確認しました。"}])
        };
        Ok(serde_json::from_value(
            json!({"candidates": [{"content": {"role": "model", "parts": parts}}]}),
        )
        .unwrap())
    }
}

struct ReceiptFactory(Arc<ReceiptBackend>);
impl GeminiFactory for ReceiptFactory {
    fn build(
        &self,
        model: &str,
        key: SecretString,
    ) -> Result<Arc<dyn GenerateBackend>, GeminiError> {
        use secrecy::ExposeSecret;
        if model != "gemini-3.1-flash-lite" || key.expose_secret() != "fake-gemini-key" {
            return Err(GeminiError::KeyUnavailable {
                scope: "test: incorrect receipt user key/model".to_owned(),
            });
        }
        Ok(self.0.clone())
    }
}

#[tokio::test]
async fn receipt_draft_has_no_turn_side_effects_and_recording_uses_same_ocr() {
    for (is_receipt, readable) in [(true, true), (true, false), (false, false)] {
        let (db, path) = fresh_db();
        let crypto =
            Arc::new(SystemCrypto::new(SecretString::from("receipt-test-secret")).unwrap());
        seed_user_with_key(&path, &crypto, "receipt-user");
        let backend = Arc::new(ReceiptBackend {
            calls: Mutex::new(Vec::new()),
            readable,
            is_receipt,
        });
        let engine = ChatEngine::new(
            db,
            Some(crypto),
            ToolRegistry::new(),
            Arc::new(ReceiptFactory(backend.clone())),
            None,
            Arc::new(yuuka_mcp::NullMcpClient),
            None,
        );
        let uid = UserId::new("receipt-user");
        let draft = engine
            .extract_receipt(
                engine
                    .acquire_turn(
                        &BotId::new("receipt-bot"),
                        &uid,
                        yuuka_orchestrator::Channel::Pwa,
                    )
                    .await
                    .expect("permit"),
                "QUJD",
                "IMAGE/PNG; x=y",
                "2026-10-20",
            )
            .await;
        assert_eq!(draft.is_ok(), readable);
        if let Ok(draft) = draft {
            assert_eq!(draft.amount, Some(600));
        }
        assert_eq!(count_logs(&path, uid.as_str(), "user"), 0);
        assert_eq!(count_logs(&path, uid.as_str(), "assistant"), 0);
        let reply = engine
            .receipt_turn(
                engine
                    .acquire_turn(
                        &BotId::new("receipt-bot"),
                        &uid,
                        yuuka_orchestrator::Channel::Web,
                    )
                    .await
                    .expect("permit"),
                yuuka_discord::InlineMedia {
                    data_base64: "QUJD".to_owned(),
                    mime_type: "image/png".to_owned(),
                },
                Some("保存せず内容だけ教えて".to_owned()),
                Vec::new(),
                &null_sink(),
            )
            .await
            .unwrap();
        let calls = backend.calls.lock().unwrap();
        assert_eq!(calls.len(), if readable || !is_receipt { 3 } else { 2 });
        assert_eq!(calls[0].0, ["readReceipt"]);
        assert_eq!(calls[1].0, ["readReceipt"]);
        if readable {
            let turn = serde_json::to_string(&calls[2].1).unwrap();
            assert!(turn.contains("receipt_ocr"));
            assert!(turn.contains("保存せず内容だけ教えて"));
            assert!(turn.contains("卵"));
            assert!(
                !turn.contains("inlineData"),
                "秘書ターンで画像を再解析しない"
            );
            let logs = log_contents(&path, uid.as_str(), "user");
            assert_eq!(logs.len(), 1);
            assert!(
                !logs[0].contains("卵"),
                "OCR 明細をユーザー発言として保存しない"
            );
        } else if !is_receipt {
            let turn = serde_json::to_string(&calls[2].1).unwrap();
            assert!(
                turn.contains("inlineData"),
                "普通の写真は従来の画像会話へ戻す"
            );
            assert!(!turn.contains("receipt_ocr"));
        } else {
            assert!(reply.text.contains("読み取れませんでした"));
            assert_eq!(count_logs(&path, uid.as_str(), "user"), 0);
        }
        let conn = rusqlite::Connection::open(path).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM expenses", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "OCR 自体は保存しない");
    }
}

/// Discord では OCR の入力検証（10 MiB 超・許可外形式）に通らない写真も、エラーにせず
/// 従来の画像会話へ戻す（develop では Discord の画像はサイズ・形式の制限なしで秘書ターンに渡る）。
#[tokio::test]
async fn receipt_turn_falls_back_to_image_chat_when_ocr_rejects_image() {
    let oversized = "A".repeat(10 * 1024 * 1024 + 4);
    for (data, mime) in [("QUJD", "image/avif"), (oversized.as_str(), "image/jpeg")] {
        let (db, path) = fresh_db();
        let crypto =
            Arc::new(SystemCrypto::new(SecretString::from("receipt-test-secret")).unwrap());
        seed_user_with_key(&path, &crypto, "receipt-user");
        let backend = Arc::new(ReceiptBackend {
            calls: Mutex::new(Vec::new()),
            readable: true,
            is_receipt: true,
        });
        let engine = ChatEngine::new(
            db,
            Some(crypto),
            ToolRegistry::new(),
            Arc::new(ReceiptFactory(backend.clone())),
            None,
            Arc::new(yuuka_mcp::NullMcpClient),
            None,
        );
        let uid = UserId::new("receipt-user");
        let reply = engine
            .receipt_turn(
                engine
                    .acquire_turn(
                        &BotId::new("receipt-bot"),
                        &uid,
                        yuuka_orchestrator::Channel::Web,
                    )
                    .await
                    .expect("permit"),
                yuuka_discord::InlineMedia {
                    data_base64: data.to_owned(),
                    mime_type: mime.to_owned(),
                },
                Some("この写真どう？".to_owned()),
                Vec::new(),
                &null_sink(),
            )
            .await
            .unwrap();
        assert_eq!(reply.text, "読み取り内容を確認しました。", "{mime}");
        let calls = backend.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "OCR は LLM を呼ばずに検証で弾く: {mime}");
        assert_ne!(calls[0].0, ["readReceipt"]);
        let turn = serde_json::to_string(&calls[0].1).unwrap();
        assert!(
            turn.contains("inlineData"),
            "画像はそのまま会話へ渡す: {mime}"
        );
        assert!(turn.contains("この写真どう？"));
        assert!(!turn.contains("receipt_ocr"));
        assert_eq!(count_logs(&path, uid.as_str(), "user"), 1);
    }
}

#[tokio::test]
async fn secretary_turn_persists_and_replies() {
    let (db, path) = fresh_db();
    let crypto =
        Arc::new(SystemCrypto::new(SecretString::from("orch-test-secret".to_owned())).unwrap());
    seed_user_with_key(&path, &crypto, "u1");

    let engine = engine_with(db, Some(crypto), "こんにちは！ご用件をどうぞ。");
    let reply = engine
        .secretary_turn(
            engine
                .acquire_turn(
                    &BotId::system_default(),
                    &UserId::new("u1"),
                    yuuka_orchestrator::Channel::Web,
                )
                .await
                .expect("permit"),
            IncomingChat {
                text: "こんにちは".to_owned(),
                ..IncomingChat::default()
            },
            &null_sink(),
        )
        .await
        .expect("turn ok");

    assert_eq!(reply.text, "こんにちは！ご用件をどうぞ。");
    // ユーザー発言 + アシスタント応答が両方永続化される。
    assert_eq!(count_logs(&path, "u1", "user"), 1, "user 発言が保存される");
    assert_eq!(
        count_logs(&path, "u1", "assistant"),
        1,
        "assistant 応答が保存される"
    );
}

/// #36: モデルの返信が Embed 形 JSON ブロックだけのとき、復元後の本文が空文字のまま
/// 会話ログに保存されてはいけない（「空でも fallback を保存」不変条件が復元後にも効く）。
#[tokio::test]
async fn embed_only_reply_does_not_persist_empty_string() {
    let (db, path) = fresh_db();
    let crypto =
        Arc::new(SystemCrypto::new(SecretString::from("orch-test-secret".to_owned())).unwrap());
    seed_user_with_key(&path, &crypto, "u6");

    let engine = engine_with(
        db,
        Some(crypto),
        "```json\n{\"title\":\"天気\",\"description\":\"晴れ\"}\n```",
    );
    let reply = engine
        .secretary_turn(
            engine
                .acquire_turn(
                    &BotId::system_default(),
                    &UserId::new("u6"),
                    yuuka_orchestrator::Channel::Web,
                )
                .await
                .expect("permit"),
            IncomingChat {
                text: "天気教えて".to_owned(),
                ..IncomingChat::default()
            },
            &null_sink(),
        )
        .await
        .expect("turn ok");

    assert_eq!(reply.embeds.len(), 1, "Embed は復元される");
    assert!(
        !reply.text.trim().is_empty(),
        "reply.text が空文字: {:?}",
        reply.text
    );

    let saved = log_contents(&path, "u6", "assistant");
    assert_eq!(saved.len(), 1);
    assert!(
        !saved[0].trim().is_empty(),
        "会話ログに空文字が保存されてはいけない: {saved:?}"
    );
}

#[tokio::test]
async fn second_turn_sees_prior_history() {
    let (db, path) = fresh_db();
    let crypto =
        Arc::new(SystemCrypto::new(SecretString::from("orch-test-secret".to_owned())).unwrap());
    seed_user_with_key(&path, &crypto, "u2");
    let engine = engine_with(db, Some(crypto), "了解しました。");

    for _ in 0..2 {
        engine
            .secretary_turn(
                engine
                    .acquire_turn(
                        &BotId::system_default(),
                        &UserId::new("u2"),
                        yuuka_orchestrator::Channel::Web,
                    )
                    .await
                    .expect("permit"),
                IncomingChat {
                    text: "メモして".to_owned(),
                    ..IncomingChat::default()
                },
                &null_sink(),
            )
            .await
            .expect("turn");
    }
    // 2 ターンで user/assistant 各 2 行（履歴ロードが落ちていないこと＝クエリ健全）。
    assert_eq!(count_logs(&path, "u2", "user"), 2);
    assert_eq!(count_logs(&path, "u2", "assistant"), 2);
}

#[tokio::test]
async fn missing_gemini_key_returns_warning_reply() {
    let (db, path) = fresh_db();
    seed_user_no_key(&path, "u3");
    // crypto は Some でもキー行が無ければ ⚠️ 応答（Node processMessage の catch パリティ）。
    let crypto =
        Arc::new(SystemCrypto::new(SecretString::from("orch-test-secret".to_owned())).unwrap());
    let engine = engine_with(db, Some(crypto), "unused");

    let reply = engine
        .secretary_turn(
            engine
                .acquire_turn(
                    &BotId::system_default(),
                    &UserId::new("u3"),
                    yuuka_orchestrator::Channel::Web,
                )
                .await
                .expect("permit"),
            IncomingChat {
                text: "やあ".to_owned(),
                ..IncomingChat::default()
            },
            &null_sink(),
        )
        .await
        .expect("turn ok");
    assert!(
        reply.text.contains("Gemini API Keyが設定されていません"),
        "reply={}",
        reply.text
    );
    // ⚠️ 定型応答は履歴に保存しない（Node processMessage catch パリティ）。ユーザー発言は保存済み。
    assert_eq!(
        count_logs(&path, "u3", "user"),
        1,
        "ユーザー発言は保存される"
    );
    assert_eq!(
        count_logs(&path, "u3", "assistant"),
        0,
        "⚠️ 応答は履歴を汚染しない"
    );
}

/// `message_logs` を破壊して非 LLM（DB）エラーを誘発する。
fn break_message_logs(path: &std::path::Path) {
    rusqlite::Connection::open(path)
        .expect("open")
        .execute_batch("ALTER TABLE message_logs RENAME TO message_logs_broken")
        .expect("break table");
}

#[tokio::test]
async fn non_llm_error_returns_persona_styled_reply() {
    let (db, path) = fresh_db();
    let crypto =
        Arc::new(SystemCrypto::new(SecretString::from("orch-test-secret".to_owned())).unwrap());
    seed_user_with_key(&path, &crypto, "u4");
    break_message_logs(&path);
    let engine = engine_with(
        db,
        Some(crypto),
        "ごめんなさい、うまく処理できませんでした…！",
    );

    let reply = engine
        .secretary_turn(
            engine
                .acquire_turn(
                    &BotId::system_default(),
                    &UserId::new("u4"),
                    yuuka_orchestrator::Channel::Web,
                )
                .await
                .expect("permit"),
            IncomingChat {
                text: "やあ".to_owned(),
                ..IncomingChat::default()
            },
            &null_sink(),
        )
        .await
        .expect("persona error reply");
    // 固定の GENERIC_ERROR ではなく、LLM（fake）が生成したペルソナ入りエラー報告が返る。
    assert_eq!(reply.text, "ごめんなさい、うまく処理できませんでした…！");
}

#[tokio::test]
async fn non_llm_error_without_key_falls_back_to_fixed_error() {
    let (db, path) = fresh_db();
    seed_user_no_key(&path, "u5");
    break_message_logs(&path);
    let crypto =
        Arc::new(SystemCrypto::new(SecretString::from("orch-test-secret".to_owned())).unwrap());
    let engine = engine_with(db, Some(crypto), "unused");

    let result = engine
        .secretary_turn(
            engine
                .acquire_turn(
                    &BotId::system_default(),
                    &UserId::new("u5"),
                    yuuka_orchestrator::Channel::Web,
                )
                .await
                .expect("permit"),
            IncomingChat {
                text: "やあ".to_owned(),
                ..IncomingChat::default()
            },
            &null_sink(),
        )
        .await;
    // キー無し＝ペルソナ応答も生成不能 → Err（呼び出し側の固定文フォールバックへ）。
    assert!(
        result.is_err(),
        "生成不能時は Err で固定文経路へ: {result:?}"
    );
}

// ─── issue #41: PWA チャット送信（`secretary_turn_pwa`）の source 分離 ───

fn count_logs_by_source(path: &std::path::Path, user_id: &str, role: &str, source: &str) -> i64 {
    let conn = rusqlite::Connection::open(path).expect("open");
    conn.query_row(
        "SELECT COUNT(*) FROM message_logs WHERE user_id = ?1 AND role = ?2 AND source = ?3",
        rusqlite::params![user_id, role, source],
        |r| r.get(0),
    )
    .expect("count")
}

/// `secretary_turn_pwa` は `secretary_turn`（Discord）と同じ応答生成経路を通りつつ、
/// 会話ログを `source='pwa'` として永続化する（issue #41）。
#[tokio::test]
async fn secretary_turn_pwa_persists_under_pwa_source() {
    let (db, path) = fresh_db();
    let crypto =
        Arc::new(SystemCrypto::new(SecretString::from("orch-test-secret".to_owned())).unwrap());
    seed_user_with_key(&path, &crypto, "u6");
    let engine = engine_with(
        db.clone(),
        Some(crypto),
        "PWAからのご用件ですね、承知しました。",
    );

    // #77: PWA のユーザー発言は呼び出し側（`chat_send`）が `202` の前に保存する。ターンは保存しない。
    message_log::accept_pwa_user_message(&db, "u6", "system_default", "PWAから送信")
        .await
        .expect("accept");

    let reply = engine
        .secretary_turn_pwa(
            engine
                .acquire_turn(
                    &BotId::system_default(),
                    &UserId::new("u6"),
                    yuuka_orchestrator::Channel::Pwa,
                )
                .await
                .expect("permit"),
            IncomingChat {
                text: "PWAから送信".to_owned(),
                ..IncomingChat::default()
            },
            &null_sink(),
        )
        .await
        .expect("pwa turn ok");

    assert_eq!(reply.text, "PWAからのご用件ですね、承知しました。");
    assert_eq!(
        count_logs_by_source(&path, "u6", "user", "pwa"),
        1,
        "PWA のユーザー発言が source='pwa' で（受理時に 1 度だけ）保存される・ターンは再保存しない"
    );
    assert_eq!(
        count_logs_by_source(&path, "u6", "assistant", "pwa"),
        1,
        "PWA のアシスタント応答が source='pwa' で保存される"
    );
    assert_eq!(
        count_logs(&path, "u6", "user"),
        1,
        "Discord 側（既定 source）には漏れない"
    );
}

/// `secretary_turn`（Discord）と `secretary_turn_pwa` は同じ `(bot_id, user_id)` でも
/// 互いのコンテキストを読まない（#38 と同種のキー不一致の再発防止・issue #41）。
#[tokio::test]
async fn secretary_turn_pwa_context_is_isolated_from_discord() {
    let (db, path) = fresh_db();
    let crypto =
        Arc::new(SystemCrypto::new(SecretString::from("orch-test-secret".to_owned())).unwrap());
    seed_user_with_key(&path, &crypto, "u7");
    let engine = engine_with(db.clone(), Some(crypto), "了解しました。");

    // Discord 経路で 1 ターン。
    engine
        .secretary_turn(
            engine
                .acquire_turn(
                    &BotId::system_default(),
                    &UserId::new("u7"),
                    yuuka_orchestrator::Channel::Web,
                )
                .await
                .expect("permit"),
            IncomingChat {
                text: "discord発言".to_owned(),
                ..IncomingChat::default()
            },
            &null_sink(),
        )
        .await
        .expect("discord turn");

    // PWA 経路で 1 ターン（同じ user_id/bot_id）。発言は受理時に保存済み（#77）。
    message_log::accept_pwa_user_message(&db, "u7", "system_default", "pwa発言")
        .await
        .expect("accept");
    engine
        .secretary_turn_pwa(
            engine
                .acquire_turn(
                    &BotId::system_default(),
                    &UserId::new("u7"),
                    yuuka_orchestrator::Channel::Pwa,
                )
                .await
                .expect("permit"),
            IncomingChat {
                text: "pwa発言".to_owned(),
                ..IncomingChat::default()
            },
            &null_sink(),
        )
        .await
        .expect("pwa turn");

    // 双方 1 件ずつ、互いの source には漏れない。
    assert_eq!(count_logs_by_source(&path, "u7", "user", "discord"), 1);
    assert_eq!(count_logs_by_source(&path, "u7", "user", "pwa"), 1);
    assert_eq!(count_logs(&path, "u7", "user"), 2, "合計は両方の和");
}

/// `generate()` 呼び出しごとの `contents`（role と結合テキストの列）の記録。
type SeenContents = Arc<Mutex<Vec<Vec<(String, String)>>>>;

/// `generate()` へ渡された `contents`（role と結合テキスト）を記録し、固定テキストを返す backend。
struct RecordingBackend {
    seen: SeenContents,
    text: String,
}

#[async_trait]
impl GenerateBackend for RecordingBackend {
    async fn generate(
        &self,
        system_instruction: Option<&str>,
        _declarations: &[FunctionDeclaration],
        contents: &[Content],
        _tool_config: Option<ToolConfig>,
    ) -> Result<GenerateContentResponse, GeminiError> {
        if is_planner_call(system_instruction) {
            return Ok(no_plan());
        }
        let rendered = contents
            .iter()
            .map(|c| {
                let role = serde_json::to_value(c.role)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                let text = c
                    .parts
                    .iter()
                    .filter_map(|p| p.text.clone())
                    .collect::<Vec<_>>()
                    .join("\n");
                (role, text)
            })
            .collect();
        self.seen.lock().unwrap().push(rendered);
        Ok(serde_json::from_value(json!({
            "candidates": [ { "content": { "role": "model", "parts": [ { "text": self.text } ] } } ]
        }))
        .expect("valid response"))
    }
}

struct RecordingFactory {
    seen: SeenContents,
    text: String,
}

impl GeminiFactory for RecordingFactory {
    fn build(
        &self,
        _model: &str,
        _api_key: SecretString,
    ) -> Result<Arc<dyn GenerateBackend>, GeminiError> {
        Ok(Arc::new(RecordingBackend {
            seen: self.seen.clone(),
            text: self.text.clone(),
        }))
    }
}

/// issue #77: PWA のユーザー発言は受理時に永続化され、ターンは再保存しない（履歴に二重に入らない）。
/// 保存済みの発言は LLM への `contents` にちょうど 1 回だけ現れる（persist-before-load は維持）。
#[tokio::test]
async fn secretary_turn_pwa_does_not_resave_the_accepted_user_message() {
    let (db, path) = fresh_db();
    let crypto =
        Arc::new(SystemCrypto::new(SecretString::from("orch-test-secret".to_owned())).unwrap());
    seed_user_with_key(&path, &crypto, "u10");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let engine = ChatEngine::new(
        db.clone(),
        Some(crypto),
        ToolRegistry::new(),
        Arc::new(RecordingFactory {
            seen: seen.clone(),
            text: "承知しました。".to_owned(),
        }),
        None,
        Arc::new(yuuka_mcp::NullMcpClient),
        None,
    );

    let accepted =
        message_log::accept_pwa_user_message(&db, "u10", "system_default", "受理済みの発言")
            .await
            .expect("accept");
    assert_eq!(count_logs_by_source(&path, "u10", "user", "pwa"), 1);

    engine
        .secretary_turn_pwa(
            engine
                .acquire_turn(
                    &BotId::system_default(),
                    &UserId::new("u10"),
                    yuuka_orchestrator::Channel::Pwa,
                )
                .await
                .expect("permit"),
            IncomingChat {
                text: "受理済みの発言".to_owned(),
                ..IncomingChat::default()
            },
            &null_sink(),
        )
        .await
        .expect("pwa turn ok");

    assert_eq!(
        count_logs_by_source(&path, "u10", "user", "pwa"),
        1,
        "ターン完了後もユーザー発言は 1 件のまま（二重保存しない）"
    );
    assert_eq!(count_logs_by_source(&path, "u10", "assistant", "pwa"), 1);
    // 履歴上の順序は user → assistant（受理時の id が先頭のまま）。
    let history = message_log::list_pwa_messages(&db, "u10", "system_default", 50)
        .await
        .unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].id, accepted.message_id);
    assert_eq!(history[0].role, "user");
    assert_eq!(history[1].role, "assistant");

    // LLM へは、受理時に保存した発言が user ターンとして 1 回だけ渡る。
    let calls = seen.lock().unwrap();
    assert_eq!(calls.len(), 1);
    let user_turns: Vec<&(String, String)> =
        calls[0].iter().filter(|(role, _)| role == "user").collect();
    assert_eq!(user_turns.len(), 1, "contents={:?}", calls[0]);
    assert_eq!(user_turns[0].1, "受理済みの発言");
}

/// issue #77 の回帰防止: Discord/WS の秘書ターン（`secretary_turn`）は従来どおり、ユーザー発言を
/// ターン内で **ちょうど 1 度**保存する（PWA 側の保存移動の影響を受けない）。
#[tokio::test]
async fn secretary_turn_discord_saves_the_user_message_exactly_once() {
    let (db, path) = fresh_db();
    let crypto =
        Arc::new(SystemCrypto::new(SecretString::from("orch-test-secret".to_owned())).unwrap());
    seed_user_with_key(&path, &crypto, "u11");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let engine = ChatEngine::new(
        db,
        Some(crypto),
        ToolRegistry::new(),
        Arc::new(RecordingFactory {
            seen: seen.clone(),
            text: "了解です。".to_owned(),
        }),
        None,
        Arc::new(yuuka_mcp::NullMcpClient),
        None,
    );

    engine
        .secretary_turn(
            engine
                .acquire_turn(
                    &BotId::system_default(),
                    &UserId::new("u11"),
                    yuuka_orchestrator::Channel::Web,
                )
                .await
                .expect("permit"),
            IncomingChat {
                text: "discordからの発言".to_owned(),
                ..IncomingChat::default()
            },
            &null_sink(),
        )
        .await
        .expect("discord turn ok");

    assert_eq!(
        count_logs_by_source(&path, "u11", "user", "discord"),
        1,
        "Discord/WS のユーザー発言はターン内で 1 度だけ保存される"
    );
    assert_eq!(count_logs_by_source(&path, "u11", "user", "pwa"), 0);
    assert_eq!(
        count_logs_by_source(&path, "u11", "assistant", "discord"),
        1
    );
    assert_eq!(
        log_contents(&path, "u11", "user"),
        vec!["discordからの発言".to_owned()]
    );
    // LLM へも当該発言が user ターンとして 1 回だけ渡る。
    let calls = seen.lock().unwrap();
    let user_turns: Vec<&(String, String)> =
        calls[0].iter().filter(|(role, _)| role == "user").collect();
    assert_eq!(user_turns.len(), 1, "contents={:?}", calls[0]);
    assert_eq!(user_turns[0].1, "discordからの発言");
}

// ─── システム Bot（system_default）は案内役: 個人向けのツール・プロンプトを使わない ────────────

/// 記録した（systemInstruction, 宣言済みツール名）。
type Captured = Arc<Mutex<Option<(String, Vec<String>)>>>;

/// 受け取った systemInstruction と宣言済みツール名を記録して固定テキストを返す backend。
struct CapturingBackend {
    seen: Captured,
}

#[async_trait]
impl GenerateBackend for CapturingBackend {
    async fn generate(
        &self,
        system_instruction: Option<&str>,
        declarations: &[FunctionDeclaration],
        _contents: &[Content],
        _tool_config: Option<ToolConfig>,
    ) -> Result<GenerateContentResponse, GeminiError> {
        if is_planner_call(system_instruction) {
            return Ok(no_plan());
        }
        *self.seen.lock().unwrap() = Some((
            system_instruction.unwrap_or_default().to_owned(),
            declarations.iter().map(|d| d.name.clone()).collect(),
        ));
        Ok(serde_json::from_value(json!({
            "candidates": [ { "content": { "role": "model", "parts": [ { "text": "ok" } ] } } ]
        }))
        .expect("valid response"))
    }
}

struct CapturingFactory {
    seen: Captured,
}

impl GeminiFactory for CapturingFactory {
    fn build(
        &self,
        _model: &str,
        _api_key: SecretString,
    ) -> Result<Arc<dyn GenerateBackend>, GeminiError> {
        Ok(Arc::new(CapturingBackend {
            seen: self.seen.clone(),
        }))
    }
}

/// 宣言だけを持つダミーツール（露出分類の選別を見るため・実行はしない）。
struct DummyTool {
    name: &'static str,
    exposure: ToolExposure,
}

#[async_trait]
impl Tool for DummyTool {
    fn declaration(&self) -> yuuka_core::tool::FunctionDeclaration {
        yuuka_core::tool::FunctionDeclaration {
            name: ToolName::checked(self.name.to_owned()).expect("valid name"),
            description: String::new(),
            parameters_json_schema: json!({ "type": "object", "properties": {} }),
            requires_confirmation: false,
        }
    }

    fn exposure(&self) -> ToolExposure {
        self.exposure.clone()
    }

    async fn call(
        &self,
        _ctx: &ToolContext,
        _args: serde_json::Value,
    ) -> Result<ToolOutcome, ToolError> {
        Err(ToolError::Execution("unused".to_owned()))
    }
}

/// 秘書ツール（`addTodo`）と core ツール（`showRichContent`）を登録したレジストリ。
fn registry_with_secretary_and_core_tools() -> ToolRegistry {
    let core = ToolExposure {
        capability: None,
        secretary: true,
        guild_assistant: true,
        requires_guild: false,
    };
    let provider = NativeProvider::new()
        .with_tool(Arc::new(DummyTool {
            name: "addTodo",
            exposure: ToolExposure::secretary(),
        }))
        .and_then(|p| {
            p.with_tool(Arc::new(DummyTool {
                name: "showRichContent",
                exposure: core,
            }))
        })
        .expect("register");
    ToolRegistry::new().with_provider(Arc::new(provider))
}

async fn captured_turn(bot_id: &BotId, seed_bot: bool) -> (String, Vec<String>) {
    let (db, path) = fresh_db();
    let crypto =
        Arc::new(SystemCrypto::new(SecretString::from("orch-test-secret".to_owned())).unwrap());
    seed_user_with_key(&path, &crypto, "u20");
    if seed_bot {
        let conn = rusqlite::Connection::open(&path).expect("open seed");
        conn.execute(
            "INSERT INTO bots (id, user_id, name) VALUES (?1, 'u20', 'Mine')",
            rusqlite::params![bot_id.as_str()],
        )
        .expect("seed bot");
    }
    let seen = Arc::new(Mutex::new(None));
    let engine = ChatEngine::new(
        db,
        Some(crypto),
        registry_with_secretary_and_core_tools(),
        Arc::new(CapturingFactory { seen: seen.clone() }),
        None,
        Arc::new(yuuka_mcp::NullMcpClient),
        None,
    );
    engine
        .secretary_turn(
            engine
                .acquire_turn(
                    bot_id,
                    &UserId::new("u20"),
                    yuuka_orchestrator::Channel::Web,
                )
                .await
                .expect("permit"),
            IncomingChat {
                text: "タスクを追加して".to_owned(),
                ..IncomingChat::default()
            },
            &null_sink(),
        )
        .await
        .expect("turn ok");
    let captured = seen.lock().unwrap().take();
    captured.expect("backend called")
}

#[tokio::test]
async fn system_bot_turn_uses_the_guide_prompt_and_no_personal_tools() {
    let (sys, tools) = captured_turn(&BotId::system_default(), false).await;
    assert!(sys.contains("システム案内役"), "sys={sys}");
    assert!(!sys.contains("addTodo"), "秘書の機能ルールを含めない");
    assert_eq!(tools, ["showRichContent"], "core ツールだけ");
}

#[tokio::test]
async fn personal_bot_turn_keeps_the_secretary_prompt_and_tools() {
    let (sys, mut tools) = captured_turn(&BotId::new("bot_mine"), true).await;
    assert!(!sys.contains("システム案内役"));
    assert!(sys.contains("addTodo"));
    tools.sort();
    assert_eq!(tools, ["addTodo", "showRichContent"]);
}

struct AttachmentProbe;
#[async_trait]
impl Tool for AttachmentProbe {
    fn declaration(&self) -> yuuka_core::tool::FunctionDeclaration {
        yuuka_core::tool::FunctionDeclaration {
            name: ToolName::checked("addTimelineRecord".to_owned()).unwrap(),
            description: "probe".to_owned(),
            parameters_json_schema: json!({"type":"object"}),
            requires_confirmation: false,
        }
    }
    async fn call(
        &self,
        ctx: &ToolContext,
        _: serde_json::Value,
    ) -> Result<ToolOutcome, ToolError> {
        assert_eq!(ctx.user_id.as_str(), "u13");
        assert_eq!(ctx.bot_id.as_str(), "media-bot");
        let attachments = &ctx.conversation_attachments;
        assert_eq!(attachments.len(), 2);
        assert_eq!(attachments.first().unwrap().position, 0);
        assert_eq!(attachments.get(1).unwrap().position, 2);
        assert_eq!(attachments.first().unwrap().source_message_id, "discord:13");
        assert_eq!(attachments.get(1).unwrap().mime_type, "video/mp4");
        assert_eq!(attachments.get(1).unwrap().data_base64, "BAUG");
        Ok(ToolOutcome::from_payload(json!({"success":true})))
    }
}
struct AttachmentFactory;
impl GeminiFactory for AttachmentFactory {
    fn build(&self, _: &str, _: SecretString) -> Result<Arc<dyn GenerateBackend>, GeminiError> {
        let responses=[
            json!({"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"addTimelineRecord","args":{"date":"2026-10-20","type":"media","attachment_indexes":[0,2]}}}]}}]}),
            json!({"candidates":[{"content":{"role":"model","parts":[{"text":"保存しました。"}]}}]}),
        ].into_iter().map(|v| serde_json::from_value(v).unwrap()).collect();
        Ok(Arc::new(FakeBackend {
            responses: Mutex::new(responses),
        }))
    }
}
fn attachment_engine() -> (ChatEngine, Vec<yuuka_core::tool::ConversationAttachment>) {
    attachment_engine_with(Arc::new(AttachmentFactory))
}
fn attachment_engine_with(
    factory: Arc<dyn GeminiFactory>,
) -> (ChatEngine, Vec<yuuka_core::tool::ConversationAttachment>) {
    let (db, path) = fresh_db();
    let crypto =
        Arc::new(SystemCrypto::new(SecretString::from("attachment-test".to_owned())).unwrap());
    seed_user_with_key(&path, &crypto, "u13");
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "INSERT INTO bots(id,user_id,name) VALUES('media-bot','u13','Media')",
            [],
        )
        .unwrap();
    let provider = NativeProvider::new()
        .with_tool(Arc::new(AttachmentProbe))
        .unwrap();
    let engine = ChatEngine::new(
        db,
        Some(crypto),
        ToolRegistry::new().with_provider(Arc::new(provider)),
        factory,
        None,
        Arc::new(yuuka_mcp::NullMcpClient),
        None,
    );
    let attachments = [(0, "image/png", "AQID"), (2, "video/mp4", "BAUG")]
        .into_iter()
        .map(
            |(position, mime, bytes)| yuuka_core::tool::ConversationAttachment {
                source_message_id: "discord:13".to_owned(),
                attachment_id: position.to_string(),
                position,
                mime_type: mime.to_owned(),
                data_base64: bytes.to_owned(),
            },
        )
        .collect();
    (engine, attachments)
}

struct NoopDelivery;
#[async_trait]
impl yuuka_discord::TurnDelivery for NoopDelivery {
    async fn on_interim(&self, _text: String) {}
    async fn deliver_final(&self, _reply: yuuka_discord::TurnReply) {}
}

/// 共通 OCR（`readReceipt`）に応答したあと、秘書ターンでは保存候補を `addTimelineRecord` で保存する。
struct ReceiptAttachmentBackend {
    is_receipt: bool,
    calls: Mutex<Vec<(Vec<String>, Vec<Content>)>>,
}
#[async_trait]
impl GenerateBackend for ReceiptAttachmentBackend {
    async fn generate(
        &self,
        system_instruction: Option<&str>,
        declarations: &[FunctionDeclaration],
        contents: &[Content],
        _tool_config: Option<ToolConfig>,
    ) -> Result<GenerateContentResponse, GeminiError> {
        if is_planner_call(system_instruction) {
            return Ok(no_plan());
        }
        let names = declarations
            .iter()
            .map(|d| d.name.clone())
            .collect::<Vec<_>>();
        let is_ocr = names == ["readReceipt"];
        let mut calls = self.calls.lock().unwrap();
        let turn_calls = calls.iter().filter(|(n, _)| n != &["readReceipt"]).count();
        calls.push((names, contents.to_vec()));
        let parts = if is_ocr {
            json!([{"functionCall": {"name": "readReceipt", "args": {
                "isReceipt": self.is_receipt, "date": "2026-10-20", "kind": "expense",
                "category": "食費", "description": "店: 卵", "amount": 600,
                "lines": [{"description": "卵", "category": "食費", "amount": 600}]
            }}}])
        } else if turn_calls == 0 {
            json!([{"functionCall": {"name": "addTimelineRecord", "args": {
                "date": "2026-10-20", "type": "media", "attachment_indexes": [0, 2]
            }}}])
        } else {
            json!([{"text": "保存しました。"}])
        };
        Ok(serde_json::from_value(
            json!({"candidates": [{"content": {"role": "model", "parts": parts}}]}),
        )
        .unwrap())
    }
}
struct ReceiptAttachmentFactory(Arc<ReceiptAttachmentBackend>);
impl GeminiFactory for ReceiptAttachmentFactory {
    fn build(&self, _: &str, _: SecretString) -> Result<Arc<dyn GenerateBackend>, GeminiError> {
        Ok(self.0.clone())
    }
}

/// Discord の写真はレシート経路（`parse_receipt` → 共通 OCR の `receipt_turn`）を通る。
/// OCR 後の秘書ターン・レシート以外のフォールバック・OCR が弾く画像のフォールバックの
/// いずれでも、保存候補を秘書ターン（カタログ通知と `ToolContext`）へ渡す。
#[tokio::test]
async fn receipt_path_keeps_timeline_attachment_catalog() {
    use yuuka_discord::TurnProcessor;
    for (is_receipt, mime) in [
        (true, "image/png"),
        (false, "image/png"),
        (true, "image/avif"),
    ] {
        let backend = Arc::new(ReceiptAttachmentBackend {
            is_receipt,
            calls: Mutex::new(Vec::new()),
        });
        let (engine, attachments) =
            attachment_engine_with(Arc::new(ReceiptAttachmentFactory(backend.clone())));
        let reply = engine
            .parse_receipt(
                &BotId::new("media-bot"),
                &UserId::new("u13"),
                yuuka_discord::InlineMedia {
                    data_base64: "AQID".to_owned(),
                    mime_type: mime.to_owned(),
                },
                IncomingChat {
                    text: "このレシートと動画を保存して".to_owned(),
                    timeline_attachments: attachments,
                    ..IncomingChat::default()
                },
                null_sink(),
                Arc::new(NoopDelivery),
            )
            .await
            .unwrap();
        assert_eq!(reply.text, "保存しました。", "{is_receipt} {mime}");
        let calls = backend.calls.lock().unwrap();
        let ocr_path = is_receipt && mime == "image/png";
        let ocr_calls = calls.iter().filter(|(n, _)| n == &["readReceipt"]).count();
        assert_eq!(ocr_calls, usize::from(mime == "image/png"), "{mime}");
        let turn = calls
            .iter()
            .filter(|(n, _)| n != &["readReceipt"])
            .map(|(_, c)| serde_json::to_string(c).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(turn.len(), 2, "{is_receipt} {mime}");
        assert!(
            turn[0].contains("現在の発話者本人の保存可能な添付"),
            "保存候補のカタログを秘書ターンへ通知する: {is_receipt} {mime}"
        );
        assert_eq!(turn[0].contains("receipt_ocr"), ocr_path, "{mime}");
        assert_eq!(
            turn[0].contains("inlineData"),
            !ocr_path,
            "OCR 済みのレシートだけ画像を外す: {is_receipt} {mime}"
        );
        // AttachmentProbe が ToolContext の保存候補を検証して成功を返している。
        assert!(turn[1].contains("functionResponse"), "{is_receipt} {mime}");
        assert!(turn[1].contains("\"success\":true"), "{is_receipt} {mime}");
    }
}

#[tokio::test]
async fn secretary_fc_receives_trusted_attachment_catalog_in_order() {
    let (engine, attachments) = attachment_engine();
    let reply = engine
        .secretary_turn(
            engine
                .acquire_turn(
                    &BotId::new("media-bot"),
                    &UserId::new("u13"),
                    yuuka_orchestrator::Channel::Web,
                )
                .await
                .expect("permit"),
            IncomingChat {
                text: "この写真と動画を保存して".to_owned(),
                timeline_attachments: attachments,
                ..IncomingChat::default()
            },
            &null_sink(),
        )
        .await
        .unwrap();
    assert_eq!(reply.text, "保存しました。");
}
