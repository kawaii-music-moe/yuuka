//! 会話エンジン（Node `gemini.ts` `processMessage` 秘書経路の上位層）。
//!
//! 1 ターンの流れ: リッチ返信フラグ → ユーザー発言を永続化 → 直近履歴ロード → `contents` 組立 →
//! システムプロンプト組立 → ユーザーの Gemini キー復号 → **FC ループ実行**（tools 往復は
//! [`yuuka_gemini::run_function_calling_loop`] が担う）→ アシスタント応答を永続化 → [`TurnReply`]。
//!
//! **縮退シーム（Node で第一級サポート・空移植）**: ターンプランナー（plan=null）・シナプス想起（""）・
//! 非同期配信（同期のみ）・Redis キャッシュ（SQLite 直）・返信チェーン・能力ゲート（全ツール露出＝
//! 現行 P2-B 既知ギャップ）。汎用モード（guild/owner DM）は未実装（P1-3）。

use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use secrecy::SecretString;
use yuuka_core::{BotId, DbError, GeminiError, GuildId, ResponsePart, ToolContext, UserId};
use yuuka_crypto::SystemCrypto;
use yuuka_discord::{
    BotStatus, FileAttachment, IncomingChat, InlineMedia, Speaker, StatusSink, TurnDelivery,
    TurnError, TurnProcessor, TurnReply,
};
use yuuka_gemini::{
    run_function_calling_loop, Content, GenerateBackend, GeminiClient, LoopOptions, Part, Role,
    Status, StatusCb,
};
use yuuka_tools::ToolRegistry;
use yuuka_web::Db;

use crate::message_log::{self, ContextEntry};
use crate::{persona, system_prompt, user};

/// 空応答時の既定文（Node `runPlannedTurn` の `fallbackText` 既定）。
const FALLBACK_TEXT: &str = "処理が完了しました。";
/// Gemini キー未設定時の応答（Node `processMessage` の catch が返す ⚠️ 文言）。
const NO_KEY_MESSAGE: &str =
    "⚠️ Gemini API Keyが設定されていません。管理画面からあなた専用のAPIキーを設定してください。";

/// 復号済み Gemini キー + モデルから [`GenerateBackend`] を作るファクトリ。
///
/// 本番は [`RealGeminiFactory`]（`GeminiClient`）。テストは fake backend を返す実装を注入し、
/// ネットワーク無しでオーケストレーションの意味論を検証する（gemini/auth と同じ規律）。
pub trait GeminiFactory: Send + Sync {
    /// モデル名と API キーから backend を構築する。
    ///
    /// # Errors
    /// クライアント構築失敗時 [`GeminiError`]。
    fn build(&self, model: &str, api_key: SecretString)
        -> Result<Arc<dyn GenerateBackend>, GeminiError>;
}

/// 本番ファクトリ: `GeminiClient` を構築する。
pub struct RealGeminiFactory;

impl GeminiFactory for RealGeminiFactory {
    fn build(
        &self,
        model: &str,
        api_key: SecretString,
    ) -> Result<Arc<dyn GenerateBackend>, GeminiError> {
        Ok(Arc::new(GeminiClient::new(model, api_key)?))
    }
}

/// 会話オーケストレーションエンジン。`/ws/chat`（デスクトップ）と Discord の [`TurnProcessor`] が共有する。
pub struct ChatEngine {
    db: Db,
    /// 保存済み Gemini キーの復号に使う（`YUUKA_ENCRYPTION_SECRET` 未設定なら `None`）。
    crypto: Option<Arc<SystemCrypto>>,
    /// ドメインツールのレジストリ（毎ターン snapshot する）。
    registry: ToolRegistry,
    /// backend ファクトリ（本番 `GeminiClient` / テスト fake）。
    factory: Arc<dyn GeminiFactory>,
}

impl ChatEngine {
    /// 依存を注入して構築する。
    #[must_use]
    pub fn new(
        db: Db,
        crypto: Option<Arc<SystemCrypto>>,
        registry: ToolRegistry,
        factory: Arc<dyn GeminiFactory>,
    ) -> Self {
        Self {
            db,
            crypto,
            registry,
            factory,
        }
    }

    /// 本番 `GeminiClient` ファクトリで構築する糖衣。
    #[must_use]
    pub fn with_real_gemini(
        db: Db,
        crypto: Option<Arc<SystemCrypto>>,
        registry: ToolRegistry,
    ) -> Self {
        Self::new(db, crypto, registry, Arc::new(RealGeminiFactory))
    }

    /// 発話ユーザーが Gemini キーを設定済みか（WS の事前チェック `error/no_gemini_key` 用）。
    pub async fn user_has_gemini_key(&self, user_id: &str) -> bool {
        matches!(user::user_gemini(&self.db, user_id).await, Ok(Some(_)))
    }

    /// 秘書経路の 1 ターンを処理する（Node `processMessage`）。
    ///
    /// # Errors
    /// DB 障害・鍵復号失敗・上流（Gemini）障害時 [`TurnError`]。キー未設定は ⚠️ テキスト応答
    /// （Node `processMessage` の catch と同じく `Ok` で返す）。
    pub async fn secretary_turn(
        &self,
        bot_id: &BotId,
        user_id: &UserId,
        msg: IncomingChat,
        status: &StatusSink,
    ) -> Result<TurnReply, TurnError> {
        let uid = user_id.as_str();
        let bid = bot_id.as_str();

        let rich = user::rich_reply_enabled(&self.db, uid).await;

        // 1. ユーザー発言を先に永続化（空表現はスキップ・Node describeIncomingMessage）。
        let log_text = describe_incoming(&msg);
        if !log_text.is_empty() {
            message_log::add_message_log(
                &self.db,
                uid,
                bid,
                "user",
                &log_text,
                msg.discord_msg_id.as_deref(),
                msg.reply_to_msg_id.as_deref(),
            )
            .await
            .map_err(to_turn_err)?;
        }

        // 2. 直近 15 件（古い順）をロードして contents を組む。
        let history = message_log::recent_context(&self.db, uid, bid, message_log::CONTEXT_LIMIT)
            .await
            .map_err(to_turn_err)?;
        let mut contents = build_contents(&history, &msg);

        // 3. システムプロンプト（ペルソナ + 固定ルール + 現在日時）。
        let persona_prompt = persona::active_persona_prompt(&self.db, uid, bid)
            .await
            .map_err(to_turn_err)?;
        let sys = system_prompt::build_system_instruction(
            persona_prompt.as_deref(),
            rich,
            &system_prompt::now_date_time_ja(),
        );

        // 4. ユーザーの Gemini キー（未設定は ⚠️ 応答・アシスタント側も保存）。
        let Some(cfg) = user::user_gemini(&self.db, uid).await.map_err(to_turn_err)? else {
            let _ = message_log::add_message_log(&self.db, uid, bid, "assistant", NO_KEY_MESSAGE, None, None).await;
            return Ok(TurnReply::text(NO_KEY_MESSAGE));
        };
        let crypto = self
            .crypto
            .as_ref()
            .ok_or_else(|| TurnError::Failed("encryption unavailable (YUUKA_ENCRYPTION_SECRET)".to_owned()))?;
        let plaintext = crypto
            .decrypt_text(&cfg.encrypted, &cfg.iv, &cfg.tag)
            .map_err(|e| TurnError::Failed(format!("gemini key decrypt: {e}")))?;
        let backend = self
            .factory
            .build(&cfg.model, SecretString::from(plaintext))
            .map_err(|e| TurnError::Failed(e.to_string()))?;

        // 5. FC ループ（tools 往復・完了是正・max iterations は crate 内で処理）。
        let mut ctx = ToolContext::new(bot_id.clone(), user_id.clone());
        ctx.rich_reply_enabled = rich;
        let snapshot = self.registry.snapshot(&ctx);
        let opts = LoopOptions {
            on_status: Some(status_bridge(status)),
            ..LoopOptions::default()
        };
        let result = run_function_calling_loop(
            backend.as_ref(),
            &snapshot,
            &sys,
            &mut contents,
            &ctx,
            &opts,
        )
        .await
        .map_err(|e| TurnError::Failed(e.to_string()))?;

        // 6. 応答テキスト（空なら fallback）。7. アシスタント応答を必ず保存（空でも fallback を保存）。
        let reply_text = if result.text.trim().is_empty() {
            FALLBACK_TEXT.to_owned()
        } else {
            result.text.clone()
        };
        message_log::add_message_log(&self.db, uid, bid, "assistant", &reply_text, None, None)
            .await
            .map_err(to_turn_err)?;

        Ok(TurnReply {
            text: reply_text,
            files: rich_parts_to_files(&result.rich_parts),
            ..TurnReply::default()
        })
    }
}

#[async_trait]
impl TurnProcessor for ChatEngine {
    async fn process_secretary(
        &self,
        bot_id: &BotId,
        user_id: &UserId,
        msg: IncomingChat,
        status: StatusSink,
        _delivery: Arc<dyn TurnDelivery>,
    ) -> Result<TurnReply, TurnError> {
        // 非同期配信（deferred）は縮退シーム＝同期実行のみ（delivery は未使用）。
        self.secretary_turn(bot_id, user_id, msg, &status).await
    }

    async fn parse_receipt(
        &self,
        bot_id: &BotId,
        user_id: &UserId,
        image: InlineMedia,
        caption: Option<String>,
        status: StatusSink,
        _delivery: Arc<dyn TurnDelivery>,
    ) -> Result<TurnReply, TurnError> {
        // レシート経路は画像 + キャプションを 1 ターンとして秘書処理へ流す（system prompt の OCR ルール
        // が分類・記録を誘導する）。専用の receipt プロンプト差し込みは後続で拡張。
        let msg = IncomingChat {
            text: caption.unwrap_or_default(),
            image: Some(image),
            ..IncomingChat::default()
        };
        self.secretary_turn(bot_id, user_id, msg, &status).await
    }

    async fn process_guild(
        &self,
        _bot_id: &BotId,
        _guild_id: &GuildId,
        _speaker: Speaker,
        _msg: IncomingChat,
        _status: StatusSink,
        _delivery: Arc<dyn TurnDelivery>,
    ) -> Result<TurnReply, TurnError> {
        // 汎用モード（ギルド常駐・Bot 専用キー）は P1-3/汎用モードのスコープ。未実装を明示。
        Err(TurnError::Failed(
            "guild generic mode は未実装（P1-3/汎用モード）".to_owned(),
        ))
    }

    async fn process_bot_dm(
        &self,
        _bot_id: &BotId,
        _speaker: Speaker,
        _msg: IncomingChat,
        _status: StatusSink,
        _delivery: Arc<dyn TurnDelivery>,
    ) -> Result<TurnReply, TurnError> {
        Err(TurnError::Failed(
            "owner DM generic mode は未実装（P1-3/汎用モード）".to_owned(),
        ))
    }
}

/// [`DbError`] を [`TurnError`] へ写像する。
fn to_turn_err(e: DbError) -> TurnError {
    TurnError::Failed(e.to_string())
}

/// ログへ残す発言表現（Node `describeIncomingMessage`＝`text || "[音声メッセージ]" || "[画像]" || ""`）。
fn describe_incoming(msg: &IncomingChat) -> String {
    if !msg.text.is_empty() {
        msg.text.clone()
    } else if msg.audio.is_some() {
        "[音声メッセージ]".to_owned()
    } else if msg.image.is_some() {
        "[画像]".to_owned()
    } else {
        String::new()
    }
}

/// 履歴 + 新規ユーザー発言から Gemini `contents` を組む（Node `buildContentsFromHistory` の中核）。
///
/// 連続する同一 role は `\n` で結合し、Gemini が要求する user/model 交互列にする。新規発言は末尾の
/// user ターンとして（テキスト + 添付画像/音声を inline data で）付ける。
fn build_contents(history: &[ContextEntry], msg: &IncomingChat) -> Vec<Content> {
    let mut out: Vec<Content> = Vec::new();
    let mut last_role: Option<&str> = None;
    for e in history {
        if last_role == Some(e.role.as_str()) {
            if let Some(last) = out.last_mut() {
                if let Some(first) = last.parts.first_mut() {
                    match &mut first.text {
                        Some(t) => {
                            t.push('\n');
                            t.push_str(&e.content);
                        }
                        None => first.text = Some(e.content.clone()),
                    }
                }
            }
        } else {
            let role = if e.role == "assistant" {
                Role::Model
            } else {
                Role::User
            };
            out.push(Content {
                role,
                parts: vec![Part::text(e.content.clone())],
            });
            last_role = Some(e.role.as_str());
        }
    }

    let mut parts = Vec::new();
    if !msg.text.is_empty() {
        parts.push(Part::text(msg.text.clone()));
    }
    if let Some(img) = &msg.image {
        parts.push(Part::inline_data(img.mime_type.clone(), img.data_base64.clone()));
    }
    if let Some(aud) = &msg.audio {
        parts.push(Part::inline_data(aud.mime_type.clone(), aud.data_base64.clone()));
    }
    if parts.is_empty() {
        parts.push(Part::text(String::new()));
    }
    out.push(Content {
        role: Role::User,
        parts,
    });
    out
}

/// FC ループの status コールバックを [`StatusSink`] へ橋渡しする（Gemini `Status` → `BotStatus`）。
fn status_bridge(sink: &StatusSink) -> StatusCb {
    let sink = sink.clone();
    Arc::new(move |s: Status| {
        let bs = match s {
            Status::Thinking => BotStatus::Thinking,
            Status::Writing => BotStatus::Writing,
        };
        sink(bs);
    })
}

/// ツール生成メディア（`rich_parts`）を [`FileAttachment`] へ写像する（base64 デコード）。
fn rich_parts_to_files(parts: &[ResponsePart]) -> Vec<FileAttachment> {
    parts
        .iter()
        .filter_map(|p| match p {
            ResponsePart::InlineData { mime_type, data } => {
                let bytes = base64::engine::general_purpose::STANDARD.decode(data).ok()?;
                let ext = if mime_type.contains("png") {
                    "png"
                } else if mime_type.contains("jpeg") || mime_type.contains("jpg") {
                    "jpg"
                } else {
                    "bin"
                };
                Some(FileAttachment {
                    name: format!("attachment.{ext}"),
                    bytes,
                })
            }
            _ => None,
        })
        .collect()
}
