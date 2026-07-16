//! P1-3 汎用モード（ギルド常駐 / owner DM）の end-to-end 統合テスト（fake Gemini・実 SQLite）。
//!
//! 検証: Bot 専用キー復号 → ギルド/DM 分離コンテキストへの永続化（`[名前]:` プレフィックス）→
//! FC ループ → アシスタント応答の永続化 → [`TurnReply`]。キー未設定時の縮退（guild=黙殺 / DM=⚠️）。
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use secrecy::SecretString;
use serde_json::json;
use yuuka_core::{BotId, GeminiError, GuildId, UserId};
use yuuka_crypto::SystemCrypto;
use yuuka_discord::{
    BotStatus, IncomingChat, Speaker, StatusSink, TurnDelivery, TurnProcessor, TurnReply,
};
use yuuka_gemini::{
    Content, FunctionDeclaration, GenerateBackend, GenerateContentResponse, ToolConfig,
};
use yuuka_orchestrator::{ChatEngine, GeminiFactory};
use yuuka_tools::ToolRegistry;
use yuuka_web::Db;

static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn fresh_db() -> (Db, std::path::PathBuf) {
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "yuuka_generic_it_{}_{n}.sqlite",
        std::process::id()
    ));
    {
        rusqlite::Connection::open(&path).expect("seed file");
    }
    let db = Db::open(&path).expect("open db");
    (db, path)
}

fn conn(path: &std::path::Path) -> rusqlite::Connection {
    rusqlite::Connection::open(path).expect("open")
}

/// 汎用モード Bot（capabilities から secretary を外す）を seed。`with_key` で Bot 専用キーを付ける。
fn seed_guild_bot(
    path: &std::path::Path,
    crypto: &SystemCrypto,
    id: &str,
    owner: &str,
    with_key: bool,
) {
    // bots.user_id は users(discord_id) を FK 参照する → オーナーを先に seed。
    conn(path)
        .execute(
            "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt, role) \
             VALUES (?1, ?1, 'x', '00', 'user')",
            rusqlite::params![owner],
        )
        .expect("seed owner");
    conn(path)
        .execute(
            "INSERT INTO bots (id, user_id, name, capabilities) \
             VALUES (?1, ?2, ?1, '[\"persona\",\"memory\",\"mcp\"]')",
            rusqlite::params![id, owner],
        )
        .expect("seed bot");
    if with_key {
        let enc = crypto.encrypt_text("fake-bot-gemini-key").expect("encrypt");
        conn(path)
            .execute(
                "UPDATE bots SET gemini_api_key_encrypted = ?1, gemini_api_key_iv = ?2, \
                 gemini_api_key_tag = ?3 WHERE id = ?4",
                rusqlite::params![enc.encrypted, enc.iv, enc.auth_tag, id],
            )
            .expect("set key");
    }
}

fn count_guild_logs(path: &std::path::Path, bot_id: &str, guild_id: &str, role: &str) -> i64 {
    conn(path)
        .query_row(
            "SELECT COUNT(*) FROM message_logs WHERE bot_id = ?1 AND guild_id = ?2 AND role = ?3",
            rusqlite::params![bot_id, guild_id, role],
            |r| r.get(0),
        )
        .expect("count")
}

fn count_dm_logs(path: &std::path::Path, bot_id: &str, user_id: &str, role: &str) -> i64 {
    conn(path)
        .query_row(
            "SELECT COUNT(*) FROM message_logs WHERE bot_id = ?1 AND user_id = ?2 \
             AND guild_id IS NULL AND role = ?3",
            rusqlite::params![bot_id, user_id, role],
            |r| r.get(0),
        )
        .expect("count")
}

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
            .expect("response"))
    }
}

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

struct NoopDelivery;

#[async_trait]
impl TurnDelivery for NoopDelivery {
    async fn on_interim(&self, _text: String) {}
    async fn deliver_final(&self, _reply: TurnReply) {}
}

fn engine_with(db: Db, crypto: Arc<SystemCrypto>, text: &str) -> ChatEngine {
    ChatEngine::new(
        db,
        Some(crypto),
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

fn speaker(user: &str, name: &str) -> Speaker {
    Speaker {
        user_id: UserId::new(user),
        display_name: name.to_owned(),
    }
}

#[tokio::test]
async fn process_guild_persists_prefixed_and_replies() {
    let (db, path) = fresh_db();
    let crypto = Arc::new(SystemCrypto::new(SecretString::from("gen-secret".to_owned())).unwrap());
    seed_guild_bot(&path, &crypto, "botG", "owner1", true);
    let engine = engine_with(db, crypto, "こんにちは、メンバーさん！");

    let reply = engine
        .process_guild(
            &BotId::new("botG"),
            &GuildId::new("g1"),
            speaker("mem1", "たろう"),
            IncomingChat {
                text: "こんにちは".to_owned(),
                ..IncomingChat::default()
            },
            null_sink(),
            Arc::new(NoopDelivery),
        )
        .await
        .expect("guild turn");

    assert_eq!(reply.text, "こんにちは、メンバーさん！");
    assert_eq!(count_guild_logs(&path, "botG", "g1", "user"), 1);
    assert_eq!(count_guild_logs(&path, "botG", "g1", "assistant"), 1);
    // 発話者名プレフィックスが付く（§4.6.1）。
    let logged: String = conn(&path)
        .query_row(
            "SELECT content FROM message_logs WHERE bot_id = 'botG' AND guild_id = 'g1' AND role = 'user'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(logged.starts_with("[たろう]: "), "content={logged}");
}

#[tokio::test]
async fn process_bot_dm_uses_separate_context() {
    let (db, path) = fresh_db();
    let crypto = Arc::new(SystemCrypto::new(SecretString::from("gen-secret".to_owned())).unwrap());
    seed_guild_bot(&path, &crypto, "botG", "owner1", true);
    let engine = engine_with(db, crypto, "動作確認 OK です。");

    let reply = engine
        .process_bot_dm(
            &BotId::new("botG"),
            speaker("owner1", "オーナー"),
            IncomingChat {
                text: "調子どう？".to_owned(),
                ..IncomingChat::default()
            },
            null_sink(),
            Arc::new(NoopDelivery),
        )
        .await
        .expect("dm turn");

    assert_eq!(reply.text, "動作確認 OK です。");
    // DM は guild_id NULL の bot×owner スコープに記録される。
    assert_eq!(count_dm_logs(&path, "botG", "owner1", "user"), 1);
    assert_eq!(count_dm_logs(&path, "botG", "owner1", "assistant"), 1);
    // ギルドログには入らない。
    assert_eq!(count_guild_logs(&path, "botG", "g1", "user"), 0);
}

#[tokio::test]
async fn missing_bot_key_degrades_by_scope() {
    let (db, path) = fresh_db();
    let crypto = Arc::new(SystemCrypto::new(SecretString::from("gen-secret".to_owned())).unwrap());
    seed_guild_bot(&path, &crypto, "botNoKey", "owner1", false);
    let engine = engine_with(db, crypto, "使われない");

    // owner DM: キー未設定は ⚠️ テキストで案内。
    let dm = engine
        .process_bot_dm(
            &BotId::new("botNoKey"),
            speaker("owner1", "オーナー"),
            IncomingChat {
                text: "やあ".to_owned(),
                ..IncomingChat::default()
            },
            null_sink(),
            Arc::new(NoopDelivery),
        )
        .await
        .expect("dm ok");
    assert!(
        dm.text.contains("Bot専用のGemini APIキー"),
        "dm={}",
        dm.text
    );

    // ギルド: キー未設定は黙殺（空応答）。
    let guild = engine
        .process_guild(
            &BotId::new("botNoKey"),
            &GuildId::new("g1"),
            speaker("mem1", "たろう"),
            IncomingChat {
                text: "こんにちは".to_owned(),
                ..IncomingChat::default()
            },
            null_sink(),
            Arc::new(NoopDelivery),
        )
        .await
        .expect("guild ok");
    assert!(guild.is_silent(), "guild は黙殺（空応答）");
}

#[tokio::test]
async fn non_llm_error_returns_bot_persona_reply() {
    let (db, path) = fresh_db();
    let crypto = Arc::new(SystemCrypto::new(SecretString::from("gen-secret".to_owned())).unwrap());
    seed_guild_bot(&path, &crypto, "botG", "owner1", true);
    // message_logs を破壊して非 LLM（DB）エラーを誘発。
    conn(&path)
        .execute_batch("ALTER TABLE message_logs RENAME TO message_logs_broken")
        .expect("break table");
    let engine = engine_with(db, crypto, "すみません、ちょっと調子が悪いみたいです…");

    let reply = engine
        .process_guild(
            &BotId::new("botG"),
            &GuildId::new("g1"),
            speaker("mem1", "たろう"),
            IncomingChat {
                text: "こんにちは".to_owned(),
                ..IncomingChat::default()
            },
            null_sink(),
            Arc::new(NoopDelivery),
        )
        .await
        .expect("persona error reply");
    // 固定の GENERIC_ERROR ではなく、Bot 専用キーで生成したペルソナ入りエラー報告が返る。
    assert_eq!(reply.text, "すみません、ちょっと調子が悪いみたいです…");
}
