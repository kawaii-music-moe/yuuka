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
use yuuka_core::{
    ActionRecorder, BotId, DbError, GeminiError, GuildId, ResponsePart, ToolContext, TurnMode,
    UserId,
};
use yuuka_crypto::SystemCrypto;
use yuuka_discord::{
    BotStatus, EmbedField, FileAttachment, IncomingChat, InlineMedia, RichEmbed, Speaker,
    StatusSink, TurnDelivery, TurnError, TurnProcessor, TurnReply,
};
use yuuka_gemini::{
    run_function_calling_loop, Content, GeminiClient, GenerateBackend, LoopOptions, Part, Role,
    Status, StatusCb,
};
use yuuka_google::CalendarPort;
use yuuka_mcp::{McpClient, McpProvider};
use yuuka_synapse::{Scope as SynapseScope, SynapseEngine};
use yuuka_tools::ToolRegistry;
use yuuka_web::Db;

use crate::calendar_prompt;
use crate::context_note;
use crate::guild_prompt::{self, GuildScope};
use crate::message_log::{self, ContextEntry};
use crate::synapse_extract::{self, ExtractScope};
use crate::synapse_recall;
use crate::{bot_repo, persona, system_prompt, user};

/// 空応答時の既定文（Node `runPlannedTurn` の `fallbackText` 既定）。
const FALLBACK_TEXT: &str = "処理が完了しました。";
/// Gemini キー未設定時の応答（Node `processMessage` の catch が返す ⚠️ 文言）。
const NO_KEY_MESSAGE: &str =
    "⚠️ Gemini API Keyが設定されていません。管理画面からあなた専用のAPIキーを設定してください。";
/// 汎用モードで Bot 専用キー未設定時の owner DM 応答（Node `processBotDmMessage` の ⚠️ 文言）。
const BOT_NO_KEY_MESSAGE: &str =
    "⚠️ このBotにはBot専用のGemini APIキーが設定されていません。管理画面の「Bot設定」→「汎用モード設定」から設定してください。";
/// レート制限応答（秘書経路・Node `processMessage` catch・gemini.ts:1111・「（トークン枯渇など）」付き）。
const SECRETARY_RATE_LIMIT_MESSAGE: &str =
    "⚠️ 現在APIの利用制限（トークン枯渇など）に達しています。しばらく待ってからもう一度お試しください。";
/// サーバーエラー応答（秘書経路・Node `processMessage` catch・gemini.ts:1119・「（503等）」付き）。
const SECRETARY_SERVER_ERROR_MESSAGE: &str =
    "⚠️ AIサーバーが現在混み合っているか、一時的なエラーが発生しています（503等）。しばらく待ってからもう一度お試しください。";
/// レート制限応答（汎用モード・Node `guildErrorResult`・gemini.ts:1325）。
const GENERIC_RATE_LIMIT_MESSAGE: &str =
    "⚠️ 現在APIの利用制限に達しています。しばらく待ってからもう一度お試しください。";
/// サーバーエラー応答（汎用モード・Node `guildErrorResult`・gemini.ts:1333）。
const GENERIC_SERVER_ERROR_MESSAGE: &str =
    "⚠️ AIサーバーが現在混み合っているか、一時的なエラーが発生しています。しばらく待ってからもう一度お試しください。";
/// 非 LLM エラー時にペルソナ口調のエラー報告を生成させる指示（システムプロンプト末尾へ連結）。
const PERSONA_ERROR_INSTRUCTION: &str = "# エラー報告\n\
いまシステム内部でエラーが発生し、ユーザーの直前のメッセージを処理できませんでした。\
あなたのペルソナ・口調のまま、処理に失敗したことを短く（1〜3文で）謝り、\
少し時間をおいてもう一度試すようユーザーへ伝えてください。\
技術的な詳細・エラー内容・システム用語は出さないでください。";
/// ペルソナ入りエラー報告の生成トリガー（Gemini は contents 必須のための合成ユーザーターン）。
const PERSONA_ERROR_NOTICE: &str =
    "（システム通知）内部エラーが発生しました。ユーザーへ知らせてください。";

/// ターン失敗の内部分類（ペルソナ入りエラー応答を試みてよいかの判定）。
///
/// - [`TurnFailure::Llm`]: LLM 関連（鍵復号不能・backend 構築失敗・上流の未分類エラー）。LLM を
///   呼べない/信頼できないため従来の固定文へ（レート/サーバーは分類済み ⚠️ で先に返すためここに来ない）。
/// - [`TurnFailure::NonLlm`]: LLM 以外（DB 障害等）。LLM は使える見込みがあるため、ペルソナ口調の
///   エラー報告を生成して返す（生成失敗時は固定文へフォールバック）。
enum TurnFailure {
    Llm(TurnError),
    NonLlm(TurnError),
}

/// ペルソナ入りエラー応答に使うペルソナ/キーの出所。
#[derive(Clone, Copy)]
enum PersonaSource {
    /// 秘書経路: 発話ユーザーのキー + アクティブペルソナ（無ければ `DEFAULT_PERSONA`）。
    Secretary,
    /// 汎用モード: Bot 専用キー + Bot ペルソナ（無ければ指示のみ）。
    Bot,
}

/// 復号済み Gemini キー + モデルから [`GenerateBackend`] を作るファクトリ。
///
/// 本番は [`RealGeminiFactory`]（`GeminiClient`）。テストは fake backend を返す実装を注入し、
/// ネットワーク無しでオーケストレーションの意味論を検証する（gemini/auth と同じ規律）。
pub trait GeminiFactory: Send + Sync {
    /// モデル名と API キーから backend を構築する。
    ///
    /// # Errors
    /// クライアント構築失敗時 [`GeminiError`]。
    fn build(
        &self,
        model: &str,
        api_key: SecretString,
    ) -> Result<Arc<dyn GenerateBackend>, GeminiError>;
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
    /// 操作履歴レコーダー（秘書経路の FC ループへ渡して dispatch を記録・`getRecentActionHistory` が
    /// 読む）。`None` なら記録しない（ツール側は「履歴なし」として振る舞う）。
    recorder: Option<Arc<ActionRecorder>>,
    /// MCP 動的ツール用の外部 MCP クライアント（秘書ターンで [`McpProvider`] を探索するのに使う）。
    /// main.rs で MCP ルートと同一の [`yuuka_mcp::HttpMcpClient`] を共有注入する（セッションキャッシュ統一）。
    /// 未配線時は [`yuuka_mcp::NullMcpClient`]（探索は空・tools/call は失敗）へ縮退する。
    mcp_client: Arc<dyn McpClient>,
    /// シナプス認知エンジン（in-process・唯一の RAM 索引）。想起（read）と索引化（write）で共有する。
    /// `None` なら現行挙動（直近履歴のみ）へデグレード（想起は `""`・抽出は no-op）。main.rs で
    /// [`SynapseEngine::boot`] して 1 つだけ注入する。`Mutex` は唯一の可変索引を直列化する。
    synapse: Option<Arc<tokio::sync::Mutex<SynapseEngine>>>,
    /// 連携中 Google カレンダー一覧を systemInstruction へ注入するための CalendarPort（A3・5min キャッシュ）。
    /// `None` なら非注入（Node `isCalendarEnabled` 相当のデグレード）。main.rs で [`ChatEngine::with_calendar`] 注入。
    calendar: Option<Arc<dyn CalendarPort>>,
}

impl ChatEngine {
    /// 依存を注入して構築する。`mcp_client` は MCP 動的ツール探索に使う（未配線は
    /// [`yuuka_mcp::NullMcpClient`] を渡す）。
    #[must_use]
    pub fn new(
        db: Db,
        crypto: Option<Arc<SystemCrypto>>,
        registry: ToolRegistry,
        factory: Arc<dyn GeminiFactory>,
        recorder: Option<Arc<ActionRecorder>>,
        mcp_client: Arc<dyn McpClient>,
        synapse: Option<Arc<tokio::sync::Mutex<SynapseEngine>>>,
    ) -> Self {
        Self {
            db,
            crypto,
            registry,
            factory,
            recorder,
            mcp_client,
            synapse,
            calendar: None,
        }
    }

    /// 連携カレンダー一覧の注入に使う `CalendarPort` を後付けする（main.rs で A3 の `google_calendar` を注入・
    /// P2-E-2）。未注入なら systemInstruction にカレンダー一覧を組み込まない。
    #[must_use]
    pub fn with_calendar(mut self, calendar: Arc<dyn CalendarPort>) -> Self {
        self.calendar = Some(calendar);
        self
    }

    /// 本番 `GeminiClient` ファクトリで構築する糖衣。
    #[must_use]
    pub fn with_real_gemini(
        db: Db,
        crypto: Option<Arc<SystemCrypto>>,
        registry: ToolRegistry,
        recorder: Option<Arc<ActionRecorder>>,
        mcp_client: Arc<dyn McpClient>,
        synapse: Option<Arc<tokio::sync::Mutex<SynapseEngine>>>,
    ) -> Self {
        Self::new(
            db,
            crypto,
            registry,
            Arc::new(RealGeminiFactory),
            recorder,
            mcp_client,
            synapse,
        )
    }

    /// 発話ユーザーが Gemini キーを設定済みか（WS の事前チェック `error/no_gemini_key` 用）。
    pub async fn user_has_gemini_key(&self, user_id: &str) -> bool {
        matches!(user::user_gemini(&self.db, user_id).await, Ok(Some(_)))
    }

    /// 会話コンテキストをリセットする（WS `reset` フレーム・Node `clearContext`）。
    ///
    /// # Errors
    /// 書き込み失敗時 [`DbError`]。
    pub async fn reset_context(&self, user_id: &str, bot_id: &str) -> Result<(), DbError> {
        message_log::clear_context(&self.db, user_id, bot_id).await
    }

    /// 秘書経路の 1 ターンを処理する（Node `processMessage`）。
    ///
    /// 非 LLM エラー（DB 障害等）は固定文ではなく**ペルソナ口調のエラー報告**を LLM 生成して返す
    /// （生成不能時は従来の固定文へフォールバック）。LLM 関連エラー（レート/サーバー/鍵）は従来どおり
    /// 分類済み ⚠️ または固定文。
    ///
    /// # Errors
    /// 鍵復号失敗・上流（Gemini）障害・ペルソナ応答も生成不能な DB 障害時 [`TurnError`]。
    /// キー未設定は ⚠️ テキスト応答（Node `processMessage` の catch と同じく `Ok` で返す）。
    pub async fn secretary_turn(
        &self,
        bot_id: &BotId,
        user_id: &UserId,
        msg: IncomingChat,
        status: &StatusSink,
    ) -> Result<TurnReply, TurnError> {
        match self.secretary_turn_impl(bot_id, user_id, msg, status).await {
            Ok(r) => Ok(r),
            Err(TurnFailure::Llm(e)) => Err(e),
            Err(TurnFailure::NonLlm(e)) => {
                tracing::warn!(error = %e, "非 LLM エラー: ペルソナ入りエラー応答の生成を試みます");
                match self
                    .persona_error_reply(bot_id, user_id, PersonaSource::Secretary)
                    .await
                {
                    Some(text) => Ok(TurnReply::text(text)),
                    None => Err(e),
                }
            }
        }
    }

    /// [`Self::secretary_turn`] の本体（失敗を LLM/非 LLM に分類して返す）。
    async fn secretary_turn_impl(
        &self,
        bot_id: &BotId,
        user_id: &UserId,
        msg: IncomingChat,
        status: &StatusSink,
    ) -> Result<TurnReply, TurnFailure> {
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
            .map_err(db_fail)?;
        }

        // 2. 直近 15 件（古い順）をロードして contents を組む。
        let history = message_log::recent_context(&self.db, uid, bid, message_log::CONTEXT_LIMIT)
            .await
            .map_err(db_fail)?;
        let mut contents = build_contents(&history, &msg);

        // 3. システムプロンプト（ペルソナ + 固定ルール + 現在日時）。
        let persona_prompt = persona::active_persona_prompt(&self.db, uid, bid)
            .await
            .map_err(db_fail)?;
        let mut sys = system_prompt::build_system_instruction(
            persona_prompt.as_deref(),
            rich,
            &system_prompt::now_date_time_ja(),
        );

        // 3.35. 連携中 Google カレンダー一覧を systemInstruction へ注入（Node `gemini.ts:149-166`・P2-E-2・
        //       秘書モードのみ）。Node 構成順＝ペルソナ→カレンダー→コンテキストノートに合わせ、コンテキスト
        //       ノートより前へ。未注入/キャッシュ空は "" で不変。
        if let Some(cal) = &self.calendar {
            sys.push_str(&calendar_prompt::build_calendar_section(&self.db, cal, uid).await);
        }

        // 3.4. コンテキストノート（ユーザー登録の背景情報）を systemInstruction 末尾へ注入
        //      （Node `gemini.ts:170-175`・§3.7.3・構成順=ペルソナ→ルール→コンテキストノート）。
        //      秘書モードのみ（Node の guild/DM `buildGuildSystemInstruction` は非注入）。未登録/空/失敗は "" で不変。
        sys.push_str(&context_note::build_context_note_section(&self.db, uid, bid).await);

        // 3.5. L2 連想想起（シナプス）を systemInstruction 末尾へ追記（Node `buildRecallSection`）。
        //      秘書モードのスコープは (userId, botId)・guild_id なし（DM/秘書利用）。返信チェーンは
        //      Rust では縮退シーム（空）＝クエリは現発話。エンジン未配線/想起 0 件なら "" で不変。
        if let Some(engine) = &self.synapse {
            let scope = SynapseScope {
                user_id: uid.to_owned(),
                bot_id: bid.to_owned(),
                guild_id: None,
            };
            let query = synapse_recall::build_recall_query(&msg.text, &[]);
            let section =
                synapse_recall::build_recall_section(&self.db, engine, &scope, &query).await;
            sys.push_str(&section);
        }

        // 4. ユーザーの Gemini キー（未設定は ⚠️ 応答）。⚠️ 定型応答は履歴に**保存しない**
        //    （Node `processMessage` の catch は `saveAssistant` を通らず返すだけ＝キー未設定/レート/
        //    サーバーエラーの警告文が以後の文脈ウィンドウを汚染しない）。
        let Some(cfg) = user::user_gemini(&self.db, uid).await.map_err(db_fail)? else {
            return Ok(TurnReply::text(NO_KEY_MESSAGE));
        };
        let crypto = self.crypto.as_ref().ok_or_else(|| {
            llm_fail("encryption unavailable (YUUKA_ENCRYPTION_SECRET)".to_owned())
        })?;
        let plaintext = crypto
            .decrypt_text(&cfg.encrypted, &cfg.iv, &cfg.tag)
            .map_err(|e| llm_fail(format!("gemini key decrypt: {e}")))?;
        let backend = self
            .factory
            .build(&cfg.model, SecretString::from(plaintext))
            .map_err(|e| llm_fail(e.to_string()))?;

        // 5. 能力ゲート: この Bot の能力集合を解決（Node `resolveBotCapabilities`・DB 未登録は秘書相当）。
        let caps = match bot_repo::get_bot(&self.db, bid).await.map_err(db_fail)? {
            Some(bot) => bot.capability_set(),
            None => bot_repo::secretary_full_capabilities(),
        };

        // 6. FC ループ（tools 往復・完了是正・max iterations は crate 内で処理）。秘書経路のツールカタログ。
        let mut ctx = ToolContext::new(bot_id.clone(), user_id.clone());
        ctx.rich_reply_enabled = rich;
        ctx.capabilities = caps;
        ctx.mode = TurnMode::Secretary;
        // MCP 動的ツール: `mcp` 能力があるときだけ発話者スコープで探索し、そのターン限りの provider として
        // snapshot へ層状に足す（Node `resolveMcpModule`: `caps.has("mcp")` ガード → getMcpFunctionModuleForBot）。
        // 探索は DB + tools/list（ネットワーク）を伴うため FC ループ直前に一度だけ行う。取得失敗でも空 provider。
        let mcp_provider: Option<Arc<dyn yuuka_core::ToolProvider>> = if ctx.capabilities.has("mcp")
        {
            Some(Arc::new(
                McpProvider::discover(self.db.clone(), Arc::clone(&self.mcp_client), bid, uid)
                    .await,
            ))
        } else {
            None
        };
        let snapshot = self.registry.snapshot_with(&ctx, mcp_provider.as_ref());
        // 秘書経路のみ操作履歴を記録する（Node `processMessage` は recordActions:true・guild/DM は false）。
        let opts = LoopOptions {
            on_status: Some(status_bridge(status)),
            action_recorder: self.recorder.clone(),
            ..LoopOptions::default()
        };
        let result = match run_function_calling_loop(
            backend.as_ref(),
            &snapshot,
            &sys,
            &mut contents,
            &ctx,
            &opts,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                // Node `processMessage` catch: レート/サーバーエラーは分類済み ⚠️ を通常応答で返す
                // （履歴には保存しない）。その他の上流エラーは LLM 関連＝固定文フォールバックへ。
                if let Some(msg) = classify_gemini_error(
                    &e,
                    SECRETARY_RATE_LIMIT_MESSAGE,
                    SECRETARY_SERVER_ERROR_MESSAGE,
                ) {
                    return Ok(TurnReply::text(msg));
                }
                return Err(llm_fail(e.to_string()));
            }
        };

        // 6. 応答テキスト（空なら fallback）。7. アシスタント応答を必ず保存（空でも fallback を保存）。
        let reply_text = if result.text.trim().is_empty() {
            FALLBACK_TEXT.to_owned()
        } else {
            result.text.clone()
        };
        message_log::add_message_log(&self.db, uid, bid, "assistant", &reply_text, None, None)
            .await
            .map_err(db_fail)?;

        // R1: 確定した最終ターンからシナプス（記憶の断片）を抽出して長期記憶へ蓄積する（Node `onFinal`）。
        //     ユーザー発話が空（添付のみ等 log_text="" ）なら抽出しない（Node は logText 有りで呼ぶ）。
        //     fire-and-forget＝ターンの戻りをブロックせず、失敗も呼び出し側へ投げない（Node パリティ）。
        self.spawn_extraction(
            ExtractScope {
                user_id: uid.to_owned(),
                bot_id: bid.to_owned(),
                guild_id: None,
            },
            &log_text,
        );

        Ok(TurnReply {
            text: reply_text,
            embeds: rich_parts_to_embeds(&result.rich_parts),
            files: rich_parts_to_files(&result.rich_parts),
            ..TurnReply::default()
        })
    }

    /// シナプス抽出をバックグラウンド（fire-and-forget）で起動する（Node `maybeExtractSynapse` の
    /// post-turn 呼び出し）。エンジン未配線・`user_text` 空なら何もしない。抽出関数は内部で失敗を握って
    /// warn ログのみ出す（ターンの結果には一切影響しない）。
    fn spawn_extraction(&self, scope: ExtractScope, user_text: &str) {
        let Some(engine) = self.synapse.clone() else {
            return;
        };
        if user_text.trim().is_empty() {
            return;
        }
        let db = self.db.clone();
        let user_text = user_text.to_owned();
        tokio::spawn(async move {
            synapse_extract::maybe_extract_synapse(&db, &engine, scope, &user_text, None).await;
        });
    }

    /// 汎用モード（ギルド常駐 / owner DM）の 1 ターンを処理する（Node `processGuildMessage` /
    /// `processBotDmMessage`）。秘書経路との差分: **Bot 専用 Gemini キー**（発話者本人キーは使わない）・
    /// ギルド/DM 分離コンテキスト・Bot 単位ペルソナ + 共有/個人ノートのシステムプロンプト。
    ///
    /// **縮退シーム（秘書経路と同様）**: 返信チェーン・非同期配信・能力ゲート（ツール絞り込みは
    /// P2-B・現状は秘書経路と同じく全ツール露出）。
    ///
    /// # Errors
    /// 鍵復号失敗・上流（Gemini）障害・ペルソナ応答も生成不能な DB 障害時 [`TurnError`]。
    /// Bot レコード不在は空応答（黙殺）、DM でのキー未設定は ⚠️ テキストを `Ok` で返す。
    /// 非 LLM エラー（DB 障害等）は Bot ペルソナ口調のエラー報告を生成して返す。
    async fn generic_turn(
        &self,
        bot_id: &BotId,
        scope: GuildScope,
        guild_id: Option<&GuildId>,
        speaker: &Speaker,
        msg: IncomingChat,
        status: &StatusSink,
    ) -> Result<TurnReply, TurnError> {
        match self
            .generic_turn_impl(bot_id, scope, guild_id, speaker, msg, status)
            .await
        {
            Ok(r) => Ok(r),
            Err(TurnFailure::Llm(e)) => Err(e),
            Err(TurnFailure::NonLlm(e)) => {
                tracing::warn!(error = %e, "非 LLM エラー: ペルソナ入りエラー応答の生成を試みます");
                match self
                    .persona_error_reply(bot_id, &speaker.user_id, PersonaSource::Bot)
                    .await
                {
                    Some(text) => Ok(TurnReply::text(text)),
                    None => Err(e),
                }
            }
        }
    }

    /// [`Self::generic_turn`] の本体（失敗を LLM/非 LLM に分類して返す）。
    async fn generic_turn_impl(
        &self,
        bot_id: &BotId,
        scope: GuildScope,
        guild_id: Option<&GuildId>,
        speaker: &Speaker,
        msg: IncomingChat,
        status: &StatusSink,
    ) -> Result<TurnReply, TurnFailure> {
        let bid = bot_id.as_str();
        let uid = speaker.user_id.as_str();
        let gid = guild_id.map(GuildId::as_str);

        // Bot レコード（無ければ黙殺・Node は `getBotById` 不在で空応答）。
        let Some(bot) = bot_repo::get_bot(&self.db, bid).await.map_err(db_fail)? else {
            return Ok(TurnReply::default());
        };

        // 1. 発話を記録（guild は `[名前]: ` プレフィックス + ギルドログ、DM は plain + DM ログ）。
        let log_text = describe_incoming(&msg);
        if !log_text.is_empty() {
            match (scope, gid) {
                (GuildScope::Guild, Some(gid)) => {
                    let prefixed = format!("[{}]: {}", speaker.display_name, log_text);
                    message_log::add_guild_message_log(
                        &self.db,
                        bid,
                        gid,
                        uid,
                        "user",
                        &prefixed,
                        msg.discord_msg_id.as_deref(),
                        msg.reply_to_msg_id.as_deref(),
                    )
                    .await
                    .map_err(db_fail)?;
                }
                _ => {
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
                    .map_err(db_fail)?;
                }
            }
        }

        // 2. コンテキスト（guild: 直近 30・bot×guild / DM: 直近 15・bot×owner）。
        //    DM は owner 専用 floor（`context_floor:{botId}:dm:{userId}`）で秘書コンテキストと分離する
        //    （Node `getBotDmContext`・秘書 `getRecentContext` と floor キーを分ける）。
        let history = match (scope, gid) {
            (GuildScope::Guild, Some(gid)) => {
                message_log::recent_guild_context(
                    &self.db,
                    bid,
                    gid,
                    message_log::GUILD_CONTEXT_LIMIT,
                )
                .await
            }
            _ => {
                message_log::recent_bot_dm_context(&self.db, bid, uid, message_log::CONTEXT_LIMIT)
                    .await
            }
        }
        .map_err(db_fail)?;
        let mut contents = build_contents(&history, &msg);

        // 3. システムプロンプト（Bot 単位ペルソナ + 共有/個人ノート）。
        let persona_prompt = match bot.persona_id {
            Some(pid) => bot_repo::persona_prompt_by_id(&self.db, pid)
                .await
                .map_err(db_fail)?,
            None => None,
        };
        let guild_note = match (scope, gid) {
            (GuildScope::Guild, Some(gid)) => bot_repo::bot_guild_note(&self.db, bid, gid)
                .await
                .map_err(db_fail)?,
            _ => String::new(),
        };
        let personal_note = bot_repo::bot_user_note(&self.db, bid, uid)
            .await
            .map_err(db_fail)?;
        let mut sys = guild_prompt::build_guild_system_instruction(
            persona_prompt.as_deref(),
            scope,
            &speaker.display_name,
            uid,
            &guild_note,
            &personal_note,
            &system_prompt::now_date_time_ja(),
        );

        // 3.5. L2 連想想起（シナプス）を汎用モードでも systemInstruction 末尾へ追記する。
        //      **Node からの意図的拡張**（Node の guild/DM 経路は想起を組まないが、本タスク指示で
        //      経路×能力の 2 軸スコープ（guild_id 付き）を活かして記憶を共有する）。スコープは
        //      guild 経路は (uid, bid, guildId)・DM 経路は (uid, bid, None)。想起 0 件/未配線は不変。
        if let Some(engine) = &self.synapse {
            let sscope = SynapseScope {
                user_id: uid.to_owned(),
                bot_id: bid.to_owned(),
                guild_id: gid.map(str::to_owned),
            };
            let query = synapse_recall::build_recall_query(&msg.text, &[]);
            let section =
                synapse_recall::build_recall_section(&self.db, engine, &sscope, &query).await;
            sys.push_str(&section);
        }

        // 4. Bot 専用 Gemini キー（未設定: guild は黙殺・DM は ⚠️ 応答）。
        let Some(triplet) = bot_repo::bot_gemini(&self.db, bid).await.map_err(db_fail)? else {
            return Ok(match scope {
                GuildScope::Guild => TurnReply::default(),
                GuildScope::Dm => TurnReply::text(BOT_NO_KEY_MESSAGE),
            });
        };
        let crypto = self.crypto.as_ref().ok_or_else(|| {
            llm_fail("encryption unavailable (YUUKA_ENCRYPTION_SECRET)".to_owned())
        })?;
        let plaintext = crypto
            .decrypt_text(&triplet.encrypted, &triplet.iv, &triplet.tag)
            .map_err(|e| llm_fail(format!("bot gemini key decrypt: {e}")))?;
        let backend = self
            .factory
            .build(bot_repo::BOT_DEFAULT_MODEL, SecretString::from(plaintext))
            .map_err(|e| llm_fail(e.to_string()))?;

        // 5. FC ループ（ギルドは ctx.guild_id を通す・リッチ返信は常時有効・汎用モードのツールカタログ）。
        //    能力ゲート: Bot の能力集合 + GuildAssistant 経路 ⇒ 秘書ツールは露出せず guild-assistant のみ。
        let mut ctx = ToolContext::new(bot_id.clone(), speaker.user_id.clone());
        ctx.rich_reply_enabled = true;
        ctx.capabilities = bot.capability_set();
        ctx.mode = TurnMode::GuildAssistant;
        if let (GuildScope::Guild, Some(gid)) = (scope, guild_id) {
            ctx.guild_id = Some(gid.clone());
        }
        let snapshot = self.registry.snapshot(&ctx);
        let opts = LoopOptions {
            on_status: Some(status_bridge(status)),
            ..LoopOptions::default()
        };
        let result = match run_function_calling_loop(
            backend.as_ref(),
            &snapshot,
            &sys,
            &mut contents,
            &ctx,
            &opts,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                // Node `guildErrorResult`: レート/サーバーエラーは分類済み ⚠️ を通常応答で返す（履歴非保存）。
                // その他の上流エラーは LLM 関連＝固定文フォールバックへ。
                if let Some(msg) = classify_gemini_error(
                    &e,
                    GENERIC_RATE_LIMIT_MESSAGE,
                    GENERIC_SERVER_ERROR_MESSAGE,
                ) {
                    return Ok(TurnReply::text(msg));
                }
                return Err(llm_fail(e.to_string()));
            }
        };

        // 6. 応答テキスト（空は fallback）+ アシスタント応答を永続化。
        let reply_text = if result.text.trim().is_empty() {
            FALLBACK_TEXT.to_owned()
        } else {
            result.text.clone()
        };
        match (scope, gid) {
            (GuildScope::Guild, Some(gid)) => {
                message_log::add_guild_message_log(
                    &self.db,
                    bid,
                    gid,
                    uid,
                    "assistant",
                    &reply_text,
                    None,
                    None,
                )
                .await
            }
            _ => {
                message_log::add_message_log(
                    &self.db,
                    uid,
                    bid,
                    "assistant",
                    &reply_text,
                    None,
                    None,
                )
                .await
            }
        }
        .map_err(db_fail)?;

        // R1: 汎用モードのターンからもシナプス抽出（**Node からの意図的拡張**・本タスク指示）。
        //     スコープは guild 経路は (uid, bid, guildId)・DM 経路は (uid, bid, None)。fire-and-forget。
        self.spawn_extraction(
            ExtractScope {
                user_id: uid.to_owned(),
                bot_id: bid.to_owned(),
                guild_id: gid.map(str::to_owned),
            },
            &log_text,
        );

        Ok(TurnReply {
            text: reply_text,
            embeds: rich_parts_to_embeds(&result.rich_parts),
            files: rich_parts_to_files(&result.rich_parts),
            ..TurnReply::default()
        })
    }

    /// 非 LLM エラー時のペルソナ入りエラー報告を生成する（best-effort・失敗は `None`＝固定文へ）。
    ///
    /// 秘書経路は発話ユーザーのキー + アクティブペルソナ（無ければ `DEFAULT_PERSONA`）、汎用モードは
    /// Bot 専用キー + Bot ペルソナを使う。ツールは渡さない（謝罪文の生成のみ）。⚠️ 定型と同じく
    /// **履歴には保存しない**（エラー通知で文脈を汚染しない）。
    async fn persona_error_reply(
        &self,
        bot_id: &BotId,
        user_id: &UserId,
        source: PersonaSource,
    ) -> Option<String> {
        let crypto = self.crypto.as_ref()?;
        let (persona, model, enc, iv, tag) = match source {
            PersonaSource::Secretary => {
                let cfg = user::user_gemini(&self.db, user_id.as_str()).await.ok()??;
                let persona =
                    persona::active_persona_prompt(&self.db, user_id.as_str(), bot_id.as_str())
                        .await
                        .ok()
                        .flatten()
                        .unwrap_or_else(|| system_prompt::DEFAULT_PERSONA.to_owned());
                (Some(persona), cfg.model, cfg.encrypted, cfg.iv, cfg.tag)
            }
            PersonaSource::Bot => {
                let t = bot_repo::bot_gemini(&self.db, bot_id.as_str())
                    .await
                    .ok()??;
                let persona = match bot_repo::get_bot(&self.db, bot_id.as_str())
                    .await
                    .ok()
                    .flatten()
                    .and_then(|b| b.persona_id)
                {
                    Some(pid) => bot_repo::persona_prompt_by_id(&self.db, pid)
                        .await
                        .ok()
                        .flatten(),
                    None => None,
                };
                (
                    persona,
                    bot_repo::BOT_DEFAULT_MODEL.to_owned(),
                    t.encrypted,
                    t.iv,
                    t.tag,
                )
            }
        };
        let plaintext = crypto.decrypt_text(&enc, &iv, &tag).ok()?;
        let backend = self
            .factory
            .build(&model, SecretString::from(plaintext))
            .ok()?;

        let sys = match persona {
            Some(p) => format!("{p}\n\n{PERSONA_ERROR_INSTRUCTION}"),
            None => PERSONA_ERROR_INSTRUCTION.to_owned(),
        };
        let mut contents = vec![Content {
            role: Role::User,
            parts: vec![Part::text(PERSONA_ERROR_NOTICE.to_owned())],
        }];
        // ツール無しの単発生成（謝罪文にツール実行は不要）。
        let ctx = ToolContext::new(bot_id.clone(), user_id.clone());
        let empty = ToolRegistry::new().snapshot(&ctx);
        let result = run_function_calling_loop(
            backend.as_ref(),
            &empty,
            &sys,
            &mut contents,
            &ctx,
            &LoopOptions::default(),
        )
        .await
        .ok()?;
        let text = result.text.trim();
        (!text.is_empty()).then(|| text.to_owned())
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
        bot_id: &BotId,
        guild_id: &GuildId,
        speaker: Speaker,
        msg: IncomingChat,
        status: StatusSink,
        _delivery: Arc<dyn TurnDelivery>,
    ) -> Result<TurnReply, TurnError> {
        // 非同期配信（deferred）は縮退シーム＝同期実行のみ（delivery は未使用）。
        self.generic_turn(
            bot_id,
            GuildScope::Guild,
            Some(guild_id),
            &speaker,
            msg,
            &status,
        )
        .await
    }

    async fn process_bot_dm(
        &self,
        bot_id: &BotId,
        speaker: Speaker,
        msg: IncomingChat,
        status: StatusSink,
        _delivery: Arc<dyn TurnDelivery>,
    ) -> Result<TurnReply, TurnError> {
        self.generic_turn(bot_id, GuildScope::Dm, None, &speaker, msg, &status)
            .await
    }
}

/// [`DbError`] を非 LLM 失敗（ペルソナ入りエラー応答の対象）へ写像する。
fn db_fail(e: DbError) -> TurnFailure {
    TurnFailure::NonLlm(TurnError::Failed(e.to_string()))
}

/// LLM 関連の失敗（固定文フォールバック対象）を組む。
fn llm_fail(msg: String) -> TurnFailure {
    TurnFailure::Llm(TurnError::Failed(msg))
}

/// FC ループの Gemini エラーを Node パリティで分類する。Node は数値 `status` のみで判定する
/// （レート制限 = 429 = [`GeminiError::RateLimited`]／サーバーエラー = 500/502/503/504 =
/// [`GeminiError::ServerError`]）。該当時は経路別の ⚠️ 文言を返し、それ以外は `None`（generic へ）。
fn classify_gemini_error(
    e: &GeminiError,
    rate_msg: &'static str,
    server_msg: &'static str,
) -> Option<&'static str> {
    match e {
        GeminiError::RateLimited { .. } => Some(rate_msg),
        GeminiError::ServerError {
            status: 500 | 502 | 503 | 504,
        } => Some(server_msg),
        _ => None,
    }
}

/// ログへ残す発言表現（Node `describeIncomingMessage`＝`text?.trim() || "[音声メッセージ]" || "[画像]" || ""`）。
/// 空白のみのテキストは空扱いで添付プレースホルダへ落とす（Node の `.trim()` パリティ）。
fn describe_incoming(msg: &IncomingChat) -> String {
    let text = msg.text.trim();
    if !text.is_empty() {
        text.to_owned()
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
/// 連続する同一 role は `\n` で結合し、Gemini が要求する user/model 交互列にする。**発言テキスト本体は
/// persist-before-load で既に履歴末尾に永続化済みのため再追加しない**（二重ユーザーターン＝交互列崩れの
/// 防止・Node は空履歴のときだけ `message.text` を積む）。添付画像/音声のみ直近 user content へ inline
/// data で合流させる。
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

    // 空履歴のときだけ最新発言を lone user turn として積む（Node `buildContentsFromHistory`
    // gemini.ts:1176-1178）。非空なら発言は既に履歴末尾にあるため積まない。
    if out.is_empty() {
        out.push(Content {
            role: Role::User,
            parts: vec![Part::text(msg.text.clone())],
        });
    }

    // 添付（画像・音声）を直近の user content へ inline data で合流（Node gemini.ts:1198-1205）。
    // 直近が user でなければ空テキスト + 添付だけの user content を積む。
    let mut inline = Vec::new();
    if let Some(img) = &msg.image {
        inline.push(Part::inline_data(
            img.mime_type.clone(),
            img.data_base64.clone(),
        ));
    }
    if let Some(aud) = &msg.audio {
        inline.push(Part::inline_data(
            aud.mime_type.clone(),
            aud.data_base64.clone(),
        ));
    }
    if !inline.is_empty() {
        match out.last_mut() {
            Some(last) if matches!(last.role, Role::User) => last.parts.extend(inline),
            _ => {
                let mut parts = vec![Part::text(String::new())];
                parts.extend(inline);
                out.push(Content {
                    role: Role::User,
                    parts,
                });
            }
        }
    }
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

/// ツールが積んだ [`ResponsePart::Embed`]（`showRichContent`/`sendChart`）を provider 中立
/// [`RichEmbed`] へ写像する（reply 層で Discord=twilight / desktop=APIEmbed JSON へ描画）。
fn rich_parts_to_embeds(parts: &[ResponsePart]) -> Vec<RichEmbed> {
    parts
        .iter()
        .filter_map(|p| match p {
            ResponsePart::Embed(e) => Some(RichEmbed {
                title: e.title.clone(),
                description: e.description.clone(),
                color: Some(e.color),
                fields: e
                    .fields
                    .iter()
                    .map(|f| EmbedField {
                        name: f.name.clone(),
                        value: f.value.clone(),
                        inline: f.inline,
                    })
                    .collect(),
                footer: e.footer.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// ツール生成メディア（`rich_parts`）を [`FileAttachment`] へ写像する（base64 デコード）。
fn rich_parts_to_files(parts: &[ResponsePart]) -> Vec<FileAttachment> {
    parts
        .iter()
        .filter_map(|p| match p {
            ResponsePart::InlineData { mime_type, data } => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .ok()?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(role: &str, content: &str) -> ContextEntry {
        ContextEntry {
            role: role.to_owned(),
            content: content.to_owned(),
        }
    }

    fn count_user_turns(contents: &[Content]) -> usize {
        contents
            .iter()
            .filter(|c| matches!(c.role, Role::User))
            .count()
    }

    /// persist-before-load で履歴末尾に既にある発言を build_contents が再追加しない
    /// （二重ユーザーターン回帰の防止・Node `buildContentsFromHistory` パリティ）。
    #[test]
    fn build_contents_does_not_duplicate_persisted_message() {
        let history = vec![
            entry("assistant", "前回の返信"),
            entry("user", "こんにちは"),
        ];
        let msg = IncomingChat {
            text: "こんにちは".to_owned(),
            ..IncomingChat::default()
        };
        let contents = build_contents(&history, &msg);
        assert_eq!(
            contents.len(),
            2,
            "履歴 2 件がそのまま 2 content（発言の再追加なし）"
        );
        assert!(matches!(contents[0].role, Role::Model));
        assert!(matches!(contents[1].role, Role::User));
        assert_eq!(contents[1].parts[0].text.as_deref(), Some("こんにちは"));
        assert_eq!(count_user_turns(&contents), 1, "user ターンは 1 つだけ");
    }

    /// 空履歴のときだけ最新発言を lone user turn として積む（Node gemini.ts:1176-1178）。
    #[test]
    fn build_contents_empty_history_pushes_lone_user_turn() {
        let msg = IncomingChat {
            text: "初回".to_owned(),
            ..IncomingChat::default()
        };
        let contents = build_contents(&[], &msg);
        assert_eq!(contents.len(), 1);
        assert!(matches!(contents[0].role, Role::User));
        assert_eq!(contents[0].parts[0].text.as_deref(), Some("初回"));
    }

    /// 添付は直近 user content へ inline data で合流し、ユーザーターンを増やさない
    /// （画像発言は "[画像]" として履歴末尾に永続化済み）。
    #[test]
    fn build_contents_merges_inline_media_into_last_user_turn() {
        let history = vec![entry("user", "[画像]")];
        let msg = IncomingChat {
            text: String::new(),
            image: Some(InlineMedia {
                mime_type: "image/png".to_owned(),
                data_base64: "AAAA".to_owned(),
            }),
            ..IncomingChat::default()
        };
        let contents = build_contents(&history, &msg);
        assert_eq!(
            count_user_turns(&contents),
            1,
            "添付でユーザーターンは増えない"
        );
        let last = contents.last().expect("content あり");
        assert!(
            last.parts.iter().any(|p| p.inline_data.is_some()),
            "末尾 user content に inline data が合流している"
        );
    }

    /// Node パリティ: 429=rate・{500,502,503,504}=server・他は None（generic へ）。経路別文言を通す。
    #[test]
    fn classify_gemini_error_matches_node_status_classification() {
        let rl = GeminiError::RateLimited { retry_after: None };
        // 秘書経路と汎用モード経路で別文言を返す（共有定数ではない）。
        assert_eq!(
            classify_gemini_error(
                &rl,
                SECRETARY_RATE_LIMIT_MESSAGE,
                SECRETARY_SERVER_ERROR_MESSAGE
            ),
            Some(SECRETARY_RATE_LIMIT_MESSAGE)
        );
        assert_eq!(
            classify_gemini_error(
                &rl,
                GENERIC_RATE_LIMIT_MESSAGE,
                GENERIC_SERVER_ERROR_MESSAGE
            ),
            Some(GENERIC_RATE_LIMIT_MESSAGE)
        );
        // サーバーエラーは Node isServerError の 4 ステータスのみ。
        for status in [500u16, 502, 503, 504] {
            let e = GeminiError::ServerError { status };
            assert_eq!(
                classify_gemini_error(&e, GENERIC_RATE_LIMIT_MESSAGE, GENERIC_SERVER_ERROR_MESSAGE),
                Some(GENERIC_SERVER_ERROR_MESSAGE),
                "status {status} は server-error"
            );
        }
        // 他の 5xx（501）・非上流系は分類対象外＝None（generic フォールバックへ）。
        for e in [
            GeminiError::ServerError { status: 501 },
            GeminiError::Status { status: 400 },
            GeminiError::Timeout,
            GeminiError::MaxIterations,
        ] {
            assert_eq!(
                classify_gemini_error(&e, GENERIC_RATE_LIMIT_MESSAGE, GENERIC_SERVER_ERROR_MESSAGE),
                None
            );
        }
    }

    /// `ResponsePart::Embed`（showRichContent）→ discord 中立 `RichEmbed` の写像を凍結する。
    #[test]
    fn rich_parts_to_embeds_maps_embed_part() {
        let parts = vec![
            ResponsePart::InlineData {
                mime_type: "image/png".to_owned(),
                data: "AAAA".to_owned(),
            },
            ResponsePart::Embed(yuuka_core::EmbedPart {
                title: Some("天気".to_owned()),
                description: Some("晴れ".to_owned()),
                color: 0x00b0f4,
                fields: vec![yuuka_core::EmbedFieldPart {
                    name: "最高".to_owned(),
                    value: "30".to_owned(),
                    inline: true,
                }],
                footer: Some("気象庁".to_owned()),
            }),
        ];
        let embeds = rich_parts_to_embeds(&parts);
        // InlineData は無視され Embed のみ写像される。
        assert_eq!(embeds.len(), 1);
        assert_eq!(embeds[0].title.as_deref(), Some("天気"));
        assert_eq!(embeds[0].color, Some(0x00b0f4));
        assert_eq!(embeds[0].fields.len(), 1);
        assert!(embeds[0].fields[0].inline);
        assert_eq!(embeds[0].footer.as_deref(), Some("気象庁"));
    }
}
