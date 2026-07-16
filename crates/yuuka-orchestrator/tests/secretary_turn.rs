//! P1-2 会話オーケストレーションの end-to-end 統合テスト（fake Gemini backend・実 SQLite）。
//!
//! 検証: ユーザー発言の永続化 → 直近履歴ロード → システムプロンプト組立 → 暗号化済みユーザー鍵の
//! 復号 → FC ループ（tools 無し）→ アシスタント応答の永続化 → [`TurnReply`]。ネットワーク不要。
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use secrecy::SecretString;
use serde_json::json;
use yuuka_core::{BotId, GeminiError, UserId};
use yuuka_crypto::SystemCrypto;
use yuuka_discord::{BotStatus, IncomingChat, StatusSink};
use yuuka_gemini::{
    Content, FunctionDeclaration, GenerateBackend, GenerateContentResponse, ToolConfig,
};
use yuuka_orchestrator::{ChatEngine, GeminiFactory};
use yuuka_tools::ToolRegistry;
use yuuka_web::Db;

static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
        _system_instruction: Option<&str>,
        _declarations: &[FunctionDeclaration],
        _contents: &[Content],
        _tool_config: Option<ToolConfig>,
    ) -> Result<GenerateContentResponse, GeminiError> {
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
        "INSERT INTO users (discord_id, username, password_hash, salt, role, \
         gemini_api_key_encrypted, gemini_api_key_iv, gemini_api_key_tag, gemini_model) \
         VALUES (?1, 'yuu', 'x', '00', 'user', ?2, ?3, ?4, 'gemini-3.1-flash-lite')",
        rusqlite::params![discord_id, enc.encrypted, enc.iv, enc.auth_tag],
    )
    .expect("seed user");
}

/// users 行だけ seed（Gemini キー無し）。
fn seed_user_no_key(path: &std::path::Path, discord_id: &str) {
    let conn = rusqlite::Connection::open(path).expect("open seed");
    conn.execute(
        "INSERT INTO users (discord_id, username, password_hash, salt, role) \
         VALUES (?1, 'yuu', 'x', '00', 'user')",
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

#[tokio::test]
async fn secretary_turn_persists_and_replies() {
    let (db, path) = fresh_db();
    let crypto =
        Arc::new(SystemCrypto::new(SecretString::from("orch-test-secret".to_owned())).unwrap());
    seed_user_with_key(&path, &crypto, "u1");

    let engine = engine_with(db, Some(crypto), "こんにちは！ご用件をどうぞ。");
    let reply = engine
        .secretary_turn(
            &BotId::system_default(),
            &UserId::new("u1"),
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
                &BotId::system_default(),
                &UserId::new("u2"),
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
            &BotId::system_default(),
            &UserId::new("u3"),
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
            &BotId::system_default(),
            &UserId::new("u4"),
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
            &BotId::system_default(),
            &UserId::new("u5"),
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
