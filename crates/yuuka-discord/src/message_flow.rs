//! メッセージハンドリングの 2 経路（現行 `src/bot.ts`）。
//!
//! - [`assistant_flow`] — 汎用モード（ギルド常駐アシスタント）。現行 `handleAssistantMessage`
//!   [`src/bot.ts:659-931`]。許可ギルド・メンバー制・Bot 専用キー・レート制限の防衛線を通す。
//! - [`secretary_flow`] — 秘書（デフォルト/独自）。現行 `setupMessageListener` インライン
//!   [`src/bot.ts:958-1263`]。登録ユーザー・共有アクセス・メンション/返信を通す。
//!
//! ルーティングは [`handle_message`]（`author.bot` 無視 → 冪等ガード → Bot 種別で分岐）。防衛線の
//! 判定は注入ポート（[`BotDirectory`]/[`RateLimiter`]/[`TurnProcessor`]）越し。twilight 送信は
//! [`crate::reply`] に隔離する。純ヘルパ（表示名解決・添付選別・返信プレフィックス）はユニットテスト。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use twilight_gateway::MessageSender;
use twilight_http::Client;
use twilight_model::channel::{Attachment, Message};
use twilight_model::id::marker::{ChannelMarker, MessageMarker, UserMarker};
use twilight_model::id::Id;
use yuuka_core::{BotId, DiscordError, GuildId, UserId};

use crate::idempotent::MessageDedup;
use crate::turn_gate::TurnGate;
use crate::ports::{
    rate_limit_message, BotDirectory, BotRecord, BotStatus, DeliverTarget, IncomingChat,
    InlineMedia, Notifier, RateLimiter, Speaker, StatusSink, TurnDelivery, TurnError,
    TurnProcessor, TurnReply,
};
use crate::presence::build_presence;
use crate::reply::{send_channel_reply, send_channel_text};
use crate::text::{
    is_supported_audio, strip_all_mentions, strip_self_mention, NON_MEMBER_GUIDANCE,
};

// ─── 定型文（現行文言 1:1） ────────────────────────────────────────────────────

const SECRETARY_DEFAULT_HELP: &str =
    "何かお手伝いできることはありますか？ 📋\n\nタスク管理、予定管理、家計管理、ブラウザ操作ができますよ！";
const SECRETARY_AUDIO_PLACEHOLDER: &str = "（音声メッセージを受信しました。内容を正確に文字起こしし、プレビューを提示してください。タスク依頼が含まれる場合はToDoへの変換を提案してください。）";
const ASSISTANT_AUDIO_PLACEHOLDER: &str =
    "（音声メッセージを受信しました。内容を正確に文字起こしし、内容に沿って応答してください。）";
const ASSISTANT_EMPTY_PROMPT: &str = "何かお手伝いできることはありますか？";
const GENERIC_ERROR: &str =
    "申し訳ございません、処理中にエラーが発生しました 😢\nしばらくしてからもう一度お試しください。";

// ─── 依存とランタイム文脈 ──────────────────────────────────────────────────────

/// フロー全体で共有する注入依存（Arc で spawn 先へ渡す）。
#[derive(Clone)]
pub struct FlowDeps {
    pub http: Arc<Client>,
    pub directory: Arc<dyn BotDirectory>,
    pub rate_limiter: Arc<dyn RateLimiter>,
    pub processor: Arc<dyn TurnProcessor>,
    pub notifier: Arc<dyn Notifier>,
    /// 二重応答冪等ガード（`(bot_user_id, message_id)`・TTL 60s）。
    pub dedup: Arc<MessageDedup>,
    /// メンバー外への利用案内スロットル（`(bot_id, user_id)`・TTL 5min・現行 `guidanceThrottle`）。
    pub guidance_throttle: Arc<MessageDedup>,
    /// 会話単位（`bot × channel`）のターン直列化ゲート（返信の交錯防止）。
    pub turn_gate: Arc<TurnGate>,
}

/// Ready で確定するテナントのランタイム情報。
#[derive(Clone)]
pub struct RuntimeBot {
    pub bot_id: BotId,
    pub bot_user_id: Id<UserMarker>,
    /// プレゼンス（`setBotStatus`）を spawn 先タスクから送るための cloneable sender。
    pub sender: MessageSender,
    /// ギルド別の Bot 統合ロール ID（`@Bot名` 補完がロールメンションになるケースの判定用。
    /// GUILD_CREATE の `role.tags.bot_id == 自 Bot` で控える）。
    pub integration_roles: Arc<std::sync::Mutex<std::collections::HashMap<u64, u64>>>,
}

impl RuntimeBot {
    /// GUILD_CREATE で確認した Bot 統合ロールを控える。
    pub fn note_integration_role(&self, guild_id: u64, role_id: u64) {
        self.integration_roles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(guild_id, role_id);
    }

    /// ギルドの Bot 統合ロール ID（未知なら `None`）。
    fn integration_role(&self, guild_id: u64) -> Option<u64> {
        self.integration_roles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&guild_id)
            .copied()
    }
}

/// ターン処理へ渡すハンドル束（プレゼンス通知＋非同期配信）。
struct TurnHandles {
    status: StatusSink,
    delivery: Arc<dyn TurnDelivery>,
}

/// メッセージ受信のエントリ（現行 `messageCreate` リスナ先頭）。Bot 自身・二重処理を弾いて分岐する。
pub async fn handle_message(deps: &FlowDeps, rt: &RuntimeBot, message: Message) {
    // Bot 自身のメッセージは無視。
    if message.author.bot {
        return;
    }
    // 受信の可視化（正常経路はログを出さない設計のため、黙殺調査はこの debug を起点にする。
    // 有効化: RUST_LOG=info,yuuka_discord=debug）。
    tracing::debug!(
        bot_id = %rt.bot_id,
        guild_id = ?message.guild_id.map(twilight_model::id::Id::get),
        channel_id = %message.channel_id,
        author = %message.author.id,
        "MESSAGE_CREATE 受信"
    );
    // 二重応答冪等ガード（両経路の分岐より前・現行 [`src/bot.ts:969`]）。
    let bot_uid = rt.bot_user_id.get().to_string();
    let msg_id = message.id.get().to_string();
    if !deps.dedup.claim(&bot_uid, &msg_id) {
        tracing::debug!(bot_id = %rt.bot_id, msg_id, "黙殺: 二重応答ガード（処理済みメッセージ）");
        return;
    }
    // 同一会話（bot × channel）のターンを到着順に直列化する。persist-before-load のため、
    // 並行ターンが互いの未応答発言を履歴に拾うと返信が交錯する（複数会話の混線防止）。
    // 別チャンネル・別 Bot は並行のまま。ガードは本ターンの返信送信完了まで保持する。
    let gate_key = format!("{}:{}", rt.bot_id.as_str(), message.channel_id.get());
    let _turn = deps.turn_gate.acquire(&gate_key).await;
    // Bot 種別でルーティング（現行 `isGuildAssistantBot(botId)`）。
    let bot = match deps.directory.get_bot(&rt.bot_id).await {
        Some(b) => b,
        // system_default はレコードが無くても常に秘書として応答する（現行はデフォルト Bot の
        // レコードを引かず secretary 経路にハードコード）。custom はレコード必須（無ければ黙殺）。
        None if rt.bot_id.as_str() == BotId::SYSTEM_DEFAULT => default_secretary_record(),
        None => {
            tracing::debug!(bot_id = %rt.bot_id, "黙殺: Bot レコード無し");
            return;
        }
    };
    if bot.is_guild_assistant {
        assistant_flow(deps, rt, &bot, message).await;
    } else {
        secretary_flow(deps, rt, &bot, message).await;
    }
}

/// system_default にレコードが無い場合の既定秘書レコード。owner_id は秘書経路（is_custom=false）で
/// 参照されないため空。name/persona も秘書フローでは未使用。
fn default_secretary_record() -> BotRecord {
    BotRecord {
        id: BotId::system_default(),
        owner_id: UserId::new(""),
        name: "早瀬ユウカ".to_owned(),
        suspended: false,
        stopped: false,
        recommended_persona_id: None,
        is_guild_assistant: false,
        has_gemini_key: true,
    }
}

// ─── 汎用モード（アシスタント）フロー ─────────────────────────────────────────

async fn assistant_flow(deps: &FlowDeps, rt: &RuntimeBot, bot: &BotRecord, message: Message) {
    let author = UserId::new(message.author.id.get().to_string());
    let is_dm = message.guild_id.is_none();

    // DM は owner のみ応答（owner 以外は黙殺・§4.3.2）。
    if is_dm && author != bot.owner_id {
        tracing::debug!(bot_id = %rt.bot_id, author = %author, "黙殺: owner 以外からの DM");
        return;
    }

    let guild_id: Option<GuildId> = message.guild_id.map(|g| GuildId::new(g.get().to_string()));
    if !is_dm {
        let Some(gid) = guild_id.as_ref() else {
            return;
        };
        // 許可ギルド（未許可は応答も記録もしない・§6）。
        if !deps.directory.is_guild_allowed(&rt.bot_id, gid).await {
            tracing::debug!(bot_id = %rt.bot_id, guild_id = %gid, "黙殺: 未許可ギルド");
            return;
        }
    }

    // 発言禁止チャンネル（`bot_muted_channels`）ではメンション/返信があっても一切応答しない（記録もしない）。
    // 有効化チャンネルの逆ゲート＝「黙殺」。利用資格・レート制限などより手前で判定する（Rust 新機能）。
    if let Some(gid) = guild_id.as_ref() {
        if deps
            .directory
            .is_channel_muted(&rt.bot_id, gid, &message.channel_id.get().to_string())
            .await
        {
            tracing::debug!(
                bot_id = %rt.bot_id,
                channel_id = %message.channel_id,
                "黙殺: 発言禁止チャンネル"
            );
            return;
        }
    }

    // メンション / Bot への返信 / 有効化チャンネルに応答（DM は常に対象）。
    // 有効化チャンネル（`bot_channels`）ではメンション/返信が無くても応答する（Rust 新機能）。
    let reference = resolve_reference(deps, &message).await;
    let is_reply_to_bot = reference
        .as_ref()
        .is_some_and(|r| r.author_id == rt.bot_user_id);
    let is_mentioned =
        is_user_mentioned(&message, rt.bot_user_id) || is_bot_role_mentioned(rt, &message);
    let is_enabled_channel = if let Some(gid) = guild_id.as_ref() {
        deps.directory
            .is_channel_enabled(&rt.bot_id, gid, &message.channel_id.get().to_string())
            .await
    } else {
        false
    };
    // 明示的に宛てられた（DM / メンション / Bot 返信）か。有効化チャンネルの「傍受」はここに含めない
    // ＝本文なしメッセージ（スタンプ等）へ定型プロンプトを返さない判定に使う（下記の空本文分岐）。
    let addressed = is_dm || is_mentioned || is_reply_to_bot;
    if !addressed && !is_enabled_channel {
        tracing::debug!(
            bot_id = %rt.bot_id,
            channel_id = %message.channel_id,
            "黙殺: 宛先外（メンション/返信なし・有効化チャンネル外）"
        );
        return;
    }

    // ギルドでの利用資格チェック（owner は暗黙メンバー／許可ロール保有者も可）。
    if !is_dm {
        let Some(gid) = guild_id.as_ref() else {
            return;
        };
        let role_ids: Vec<String> = message
            .member
            .as_ref()
            .map(|m| m.roles.iter().map(|r| r.get().to_string()).collect())
            .unwrap_or_default();
        let is_member = author == bot.owner_id
            || deps.directory.is_bot_member(&rt.bot_id, gid, &author).await
            || deps
                .directory
                .is_any_role_allowed(&rt.bot_id, gid, &role_ids)
                .await;
        if !is_member {
            send_non_member_guidance(deps, rt, &message, gid).await;
            return;
        }
        // Bot 専用キー必須（未設定なら応答しない・§4.3.3）。
        if !bot.has_gemini_key {
            tracing::warn!(bot_id = %rt.bot_id, "Gemini APIキー未設定のため応答しません");
            return;
        }
        // レート制限（超過時は LLM を呼ばず定型応答・§6）。
        let rate = deps.rate_limiter.consume(&rt.bot_id, gid, &author).await;
        if !rate.allowed {
            if let Some(exceeded) = rate.exceeded {
                send_channel_text(
                    &deps.http,
                    message.channel_id,
                    Some(message.id),
                    &rate_limit_message(exceeded),
                    &[],
                )
                .await;
            }
            return;
        }
    }

    let typing = TypingGuard::start(deps.http.clone(), message.channel_id);
    let status = make_status_sink(rt.sender.clone());

    // 自 Bot 宛メンションのみ除去（他ユーザーメンションは対象解決に残す）。統合ロール経由の
    // メンション（`<@&role>`）も自 Bot 宛として除去する。
    let text = strip_self_mention(&message.content, &bot_uid_str(rt));
    let text = match message.guild_id.and_then(|g| rt.integration_role(g.get())) {
        Some(role_id) => text.replace(&format!("<@&{role_id}>"), ""),
        None => text,
    };
    let text = text.trim().to_owned();
    let context_prefix = build_context_prefix(rt, reference.as_ref(), true);
    let full_text = format!("{context_prefix}{text}");

    let image = select_image(&message.attachments).cloned();
    let audio = select_audio(&message.attachments).cloned();
    let speaker = Speaker {
        user_id: author.clone(),
        display_name: resolve_display_name(&message),
    };
    let delivery = make_delivery(deps, rt, &message, is_dm, author.clone(), typing.handle());
    let handles = TurnHandles {
        status: status.clone(),
        delivery,
    };
    let discord_msg_id = Some(message.id.get().to_string());
    let reply_to_msg_id = reference_message_id(&message);
    let channel_id = Some(message.channel_id.get().to_string());

    let result: Result<TurnReply, TurnError> = if let Some(att) = audio {
        match fetch_inline_media(&att, "audio/ogg").await {
            Ok(media) => {
                let text = if full_text.trim().is_empty() {
                    ASSISTANT_AUDIO_PLACEHOLDER.to_owned()
                } else {
                    full_text.clone()
                };
                let chat = IncomingChat {
                    text,
                    audio: Some(media),
                    discord_msg_id,
                    reply_to_msg_id,
                    channel_id,
                    ..IncomingChat::default()
                };
                dispatch_assistant(deps, rt, is_dm, guild_id.as_ref(), speaker, chat, &handles)
                    .await
            }
            Err(e) => Err(TurnError::Failed(e.to_string())),
        }
    } else if let Some(att) = image {
        match fetch_inline_media(&att, "image/jpeg").await {
            Ok(media) => {
                let chat = IncomingChat {
                    text: full_text.clone(),
                    image: Some(media),
                    discord_msg_id,
                    reply_to_msg_id,
                    channel_id,
                    ..IncomingChat::default()
                };
                dispatch_assistant(deps, rt, is_dm, guild_id.as_ref(), speaker, chat, &handles)
                    .await
            }
            Err(e) => Err(TurnError::Failed(e.to_string())),
        }
    } else if !full_text.trim().is_empty() {
        let chat = IncomingChat {
            text: full_text.clone(),
            discord_msg_id,
            reply_to_msg_id,
            channel_id,
            ..IncomingChat::default()
        };
        dispatch_assistant(deps, rt, is_dm, guild_id.as_ref(), speaker, chat, &handles).await
    } else if addressed {
        // 本文なしメンション → 定型プロンプトを返して終了（タイマーは drop で停止）。
        drop(typing);
        status(BotStatus::Idle);
        send_channel_text(
            &deps.http,
            message.channel_id,
            Some(message.id),
            ASSISTANT_EMPTY_PROMPT,
            &[],
        )
        .await;
        return;
    } else {
        // 有効化チャンネルの本文なしメッセージ（スタンプ/添付のみ等）は黙殺する
        // （メンションされていないため、案内文を返すとチャンネルがスパムになる）。
        drop(typing);
        status(BotStatus::Idle);
        return;
    };

    drop(typing);
    status(BotStatus::Idle);
    finish_reply(deps, &message, result, /* skip_silent */ true).await;
}

/// アシスタントの DM/ギルド分岐（現行 `processBotDmMessage` / `processGuildMessage`）。
async fn dispatch_assistant(
    deps: &FlowDeps,
    rt: &RuntimeBot,
    is_dm: bool,
    guild_id: Option<&GuildId>,
    speaker: Speaker,
    chat: IncomingChat,
    handles: &TurnHandles,
) -> Result<TurnReply, TurnError> {
    if is_dm {
        deps.processor
            .process_bot_dm(
                &rt.bot_id,
                speaker,
                chat,
                handles.status.clone(),
                handles.delivery.clone(),
            )
            .await
    } else if let Some(gid) = guild_id {
        deps.processor
            .process_guild(
                &rt.bot_id,
                gid,
                speaker,
                chat,
                handles.status.clone(),
                handles.delivery.clone(),
            )
            .await
    } else {
        Err(TurnError::Failed(
            "guild id missing for guild turn".to_owned(),
        ))
    }
}

// ─── 秘書フロー ────────────────────────────────────────────────────────────────

async fn secretary_flow(deps: &FlowDeps, rt: &RuntimeBot, bot: &BotRecord, message: Message) {
    let author = UserId::new(message.author.id.get().to_string());

    // 独自秘書 Bot は owner か共有ユーザーのみ応答（system_default はこのチェック無し）。
    let is_custom = rt.bot_id.as_str() != BotId::SYSTEM_DEFAULT;
    if is_custom && author != bot.owner_id {
        let accessible = deps.directory.list_bots_for_user(&author).await;
        if !accessible.contains(&rt.bot_id) {
            return;
        }
    }

    // 登録ユーザーのみ応答（§5.4）。
    if !deps.directory.is_registered_user(&author).await {
        return;
    }

    let reference = resolve_reference(deps, &message).await;
    let is_reply_to_bot = reference
        .as_ref()
        .is_some_and(|r| r.author_id == rt.bot_user_id);
    let is_mentioned =
        is_user_mentioned(&message, rt.bot_user_id) || is_bot_role_mentioned(rt, &message);
    let is_dm = message.guild_id.is_none();
    if !is_mentioned && !is_dm && !is_reply_to_bot {
        return;
    }

    let typing = TypingGuard::start(deps.http.clone(), message.channel_id);
    let status = make_status_sink(rt.sender.clone());

    // 秘書は全メンションを除去（現行 [`src/bot.ts:1048`]）。
    let text = strip_all_mentions(&message.content);
    let text = text.trim().to_owned();
    let context_prefix = build_context_prefix(rt, reference.as_ref(), false);
    let full_text = format!("{context_prefix}{text}");

    // 画像は現メッセージ優先、無ければ返信先も探す（現行 [`src/bot.ts:1071-1078`]）。
    let image = select_image(&message.attachments).cloned().or_else(|| {
        reference
            .as_ref()
            .and_then(|r| select_image(&r.attachments).cloned())
    });
    let audio = select_audio(&message.attachments).cloned();
    let delivery = make_delivery(deps, rt, &message, is_dm, author.clone(), typing.handle());
    let discord_msg_id = Some(message.id.get().to_string());
    let reply_to_msg_id = reference_message_id(&message);
    let channel_id = Some(message.channel_id.get().to_string());

    let result: Result<TurnReply, TurnError> = if let Some(att) = audio {
        match fetch_inline_media(&att, "audio/ogg").await {
            Ok(media) => {
                // 現行 `const instruction = fullText.trim() || placeholder`（trim 済み）。
                let instruction = if full_text.trim().is_empty() {
                    SECRETARY_AUDIO_PLACEHOLDER.to_owned()
                } else {
                    full_text.trim().to_owned()
                };
                let chat = IncomingChat {
                    text: instruction,
                    audio: Some(media),
                    discord_msg_id,
                    reply_to_msg_id,
                    channel_id,
                    ..IncomingChat::default()
                };
                deps.processor
                    .process_secretary(&rt.bot_id, &author, chat, status.clone(), delivery.clone())
                    .await
            }
            Err(e) => Err(TurnError::Failed(e.to_string())),
        }
    } else if let Some(att) = image {
        match fetch_inline_media(&att, "image/jpeg").await {
            Ok(media) => {
                // レシート OCR は秘書専用。caption は現メッセージ本文（空なら None）。
                let caption = if text.is_empty() {
                    None
                } else {
                    Some(text.clone())
                };
                deps.processor
                    .parse_receipt(
                        &rt.bot_id,
                        &author,
                        media,
                        caption,
                        status.clone(),
                        delivery.clone(),
                    )
                    .await
            }
            Err(e) => Err(TurnError::Failed(e.to_string())),
        }
    } else if !full_text.trim().is_empty() {
        let chat = IncomingChat {
            text: full_text.clone(),
            discord_msg_id,
            reply_to_msg_id,
            channel_id,
            ..IncomingChat::default()
        };
        deps.processor
            .process_secretary(&rt.bot_id, &author, chat, status.clone(), delivery.clone())
            .await
    } else {
        Ok(TurnReply::text(SECRETARY_DEFAULT_HELP))
    };

    drop(typing);
    status(BotStatus::Idle);
    // 秘書は空応答でも既定ヘルプで必ず何か送る（silent skip 無し）。
    finish_reply(deps, &message, result, /* skip_silent */ false).await;
}

/// 処理結果を返信送信する（エラーは定型文へ縮退・現行 try/catch）。`skip_silent` は
/// アシスタントの「空応答は黙殺」挙動（秘書は false）。
async fn finish_reply(
    deps: &FlowDeps,
    message: &Message,
    result: Result<TurnReply, TurnError>,
    skip_silent: bool,
) {
    match result {
        Ok(reply) => {
            if skip_silent && reply.is_silent() {
                return;
            }
            send_channel_reply(&deps.http, message.channel_id, Some(message.id), &reply).await;
        }
        Err(e) => {
            tracing::error!(error = %e, "ターン処理に失敗（定型エラーへ縮退）");
            send_channel_text(
                &deps.http,
                message.channel_id,
                Some(message.id),
                GENERIC_ERROR,
                &[],
            )
            .await;
        }
    }
}

/// メンバー外への利用案内（LLM は呼ばず・ギルド内のみ申請ボタン付き・現行 `sendNonMemberGuidance`）。
async fn send_non_member_guidance(
    deps: &FlowDeps,
    rt: &RuntimeBot,
    message: &Message,
    guild_id: &GuildId,
) {
    // 連投スパムで Discord レート制限を踏まないよう 5 分スロットル（現行 guidanceThrottle）。
    let author_id = message.author.id.get().to_string();
    if !deps.guidance_throttle.claim(rt.bot_id.as_str(), &author_id) {
        return;
    }
    let components = crate::reply::to_twilight_components(&[crate::ports::ActionRow {
        buttons: vec![crate::ports::Button {
            custom_id: format!("memreq_apply:{}:{}", rt.bot_id, guild_id),
            label: "利用申請する".to_owned(),
            style: crate::ports::ButtonStyle::Primary,
        }],
    }]);
    send_channel_text(
        &deps.http,
        message.channel_id,
        Some(message.id),
        NON_MEMBER_GUIDANCE,
        &components,
    )
    .await;
}

// ─── FlowDelivery（TurnAsyncDelivery 実装） ────────────────────────────────────

/// 非同期配信ハンドル。processor が保持し、一時応答・最終配信を Discord へ返す。
struct FlowDelivery {
    http: Arc<Client>,
    notifier: Arc<dyn Notifier>,
    channel_id: Id<ChannelMarker>,
    reply_to: Id<MessageMarker>,
    typing_abort: tokio::task::AbortHandle,
    user_id: UserId,
    bot_id: BotId,
    is_dm: bool,
}

#[async_trait]
impl TurnDelivery for FlowDelivery {
    async fn on_interim(&self, text: String) {
        // 「入力中…」を止めてから一時応答を即返す（現行 onInterim）。
        self.typing_abort.abort();
        send_channel_reply(
            &self.http,
            self.channel_id,
            Some(self.reply_to),
            &TurnReply::text(text),
        )
        .await;
    }

    async fn deliver_final(&self, reply: TurnReply) {
        let target = if self.is_dm {
            DeliverTarget::Dm
        } else {
            DeliverTarget::Channel(self.channel_id.get().to_string())
        };
        let ok = self
            .notifier
            .send_to_user(&self.user_id, reply.clone(), target, &self.bot_id)
            .await;
        // 同チャンネル送信に失敗したら DM へフォールバック（現行 deliverFinal）。
        if !ok && !self.is_dm {
            self.notifier
                .send_to_user(&self.user_id, reply, DeliverTarget::Dm, &self.bot_id)
                .await;
        }
    }
}

fn make_delivery(
    deps: &FlowDeps,
    rt: &RuntimeBot,
    message: &Message,
    is_dm: bool,
    user_id: UserId,
    typing_abort: tokio::task::AbortHandle,
) -> Arc<dyn TurnDelivery> {
    Arc::new(FlowDelivery {
        http: deps.http.clone(),
        notifier: deps.notifier.clone(),
        channel_id: message.channel_id,
        reply_to: message.id,
        typing_abort,
        user_id,
        bot_id: rt.bot_id.clone(),
        is_dm,
    })
}

// ─── typing 維持（現行 sendTyping 5 秒維持） ───────────────────────────────────

/// 「入力中…」を 5 秒ごとに維持するガード。drop または明示 abort で停止する。
struct TypingGuard {
    abort: tokio::task::AbortHandle,
}

impl TypingGuard {
    fn start(http: Arc<Client>, channel_id: Id<ChannelMarker>) -> Self {
        let jh = tokio::spawn(async move {
            // 初回トリガ。
            let _ = http.create_typing_trigger(channel_id).await;
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            interval.tick().await; // 直後の即時 tick を消費。
            loop {
                interval.tick().await;
                let _ = http.create_typing_trigger(channel_id).await;
            }
        });
        Self {
            abort: jh.abort_handle(),
        }
    }

    fn handle(&self) -> tokio::task::AbortHandle {
        self.abort.clone()
    }
}

impl Drop for TypingGuard {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

// ─── 添付・参照・表示名（純ヘルパ＋fetch） ─────────────────────────────────────

/// 返信先メッセージの必要情報（純データ）。
struct RefInfo {
    author_id: Id<UserMarker>,
    author_name: String,
    content: String,
    attachments: Vec<Attachment>,
}

impl RefInfo {
    fn from_message(m: &Message) -> Self {
        // 現行は返信先著者名に username を使う（秘書 [`src/bot.ts:1057`]。汎用は member.nick 優先
        // だが、取得/埋め込みの返信先メッセージには member が無いため実質 username へフォールバック）。
        // member があればニックネームを優先する。
        let author_name = m
            .member
            .as_ref()
            .and_then(|pm| pm.nick.clone())
            .unwrap_or_else(|| m.author.name.clone());
        Self {
            author_id: m.author.id,
            author_name,
            content: m.content.clone(),
            attachments: m.attachments.clone(),
        }
    }
}

/// 返信先メッセージを解決する（埋め込み優先・無ければ REST 取得・現行 messages.fetch）。
async fn resolve_reference(deps: &FlowDeps, message: &Message) -> Option<RefInfo> {
    if let Some(referenced) = &message.referenced_message {
        return Some(RefInfo::from_message(referenced));
    }
    let mid = message.reference.as_ref().and_then(|r| r.message_id)?;
    match deps.http.message(message.channel_id, mid).await {
        Ok(resp) => match resp.model().await {
            Ok(m) => Some(RefInfo::from_message(&m)),
            Err(e) => {
                tracing::warn!(error = %e, "返信先メッセージの取得に失敗しました");
                None
            }
        },
        Err(e) => {
            tracing::warn!(error = %e, "返信先メッセージの取得に失敗しました");
            None
        }
    }
}

/// 返信先コンテキストプレフィックスを組み立てる（現行 contextPrefix）。`strip_self` は自 Bot
/// メンションのみ除去（アシスタント）か全除去（秘書）かの切替。
fn build_context_prefix(rt: &RuntimeBot, reference: Option<&RefInfo>, strip_self: bool) -> String {
    let Some(r) = reference else {
        return String::new();
    };
    let name = if r.author_id == rt.bot_user_id {
        "あなた".to_owned()
    } else {
        r.author_name.clone()
    };
    let clean = if strip_self {
        strip_self_mention(&r.content, &bot_uid_str(rt))
    } else {
        strip_all_mentions(&r.content)
    };
    let clean = clean.trim();
    if clean.is_empty() {
        String::new()
    } else {
        format!("[返信先メッセージ ({name}): \"{clean}\"]\n")
    }
}

fn bot_uid_str(rt: &RuntimeBot) -> String {
    rt.bot_user_id.get().to_string()
}

/// 与えられたユーザーがメンションされているか（現行 `message.mentions.has(botClient.user)`）。
///
/// discord.js の `has()` は直接メンションに加え **@everyone/@here**（`mention_everyone`）と
/// **Bot 自身が保持するロールへのロールメンション**も真とする。前者は本関数、後者は
/// [`is_bot_role_mentioned`]（統合ロール＝`@Bot名` 補完で付くロール）で移植する。統合ロール以外の
/// 保持ロール（例: 手動付与の共用ロール）へのメンションは未対応（既知の狭め・member fetch が必要）。
fn is_user_mentioned(message: &Message, user_id: Id<UserMarker>) -> bool {
    message.mention_everyone || message.mentions.iter().any(|m| m.id == user_id)
}

/// Bot の統合ロールへのロールメンションか。Discord の `@Bot名` 補完はユーザーメンションではなく
/// **統合ロールのロールメンション**になることがあり（`message.mentions` は空・`mention_roles` に
/// ロール ID）、discord.js の `mentions.has()` はこれを真とするため移植する。
fn is_bot_role_mentioned(rt: &RuntimeBot, message: &Message) -> bool {
    let Some(gid) = message.guild_id else {
        return false;
    };
    let Some(role_id) = rt.integration_role(gid.get()) else {
        return false;
    };
    message.mention_roles.iter().any(|r| r.get() == role_id)
}

/// ギルド表示名を解決する（nick → global_name → username・現行 `resolveDisplayName`）。
fn resolve_display_name(message: &Message) -> String {
    message
        .member
        .as_ref()
        .and_then(|m| m.nick.clone())
        .or_else(|| message.author.global_name.clone())
        .unwrap_or_else(|| message.author.name.clone())
}

/// 返信元メッセージ ID（会話ログ・チェーン解決用）。
fn reference_message_id(message: &Message) -> Option<String> {
    message
        .reference
        .as_ref()
        .and_then(|r| r.message_id)
        .map(|id| id.get().to_string())
}

/// 画像添付を選ぶ（content-type が `image/` 始まり・現行）。
fn select_image(attachments: &[Attachment]) -> Option<&Attachment> {
    attachments.iter().find(|a| {
        a.content_type
            .as_deref()
            .is_some_and(|ct| ct.starts_with("image/"))
    })
}

/// 音声添付を選ぶ（現行 SUPPORTED_AUDIO_TYPES / `audio/` 始まり）。
fn select_audio(attachments: &[Attachment]) -> Option<&Attachment> {
    attachments
        .iter()
        .find(|a| a.content_type.as_deref().is_some_and(is_supported_audio))
}

/// 添付を URL 取得 → base64 化する（現行 fetch → Buffer.toString("base64")）。
async fn fetch_inline_media(
    att: &Attachment,
    default_mime: &str,
) -> Result<InlineMedia, DiscordError> {
    let resp = reqwest::get(&att.url)
        .await
        .map_err(|e| DiscordError::Transport(format!("attachment fetch: {e}")))?;
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| DiscordError::Transport(format!("attachment read: {e}")))?;
    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let mime = att
        .content_type
        .as_deref()
        .map(|ct| ct.split(';').next().unwrap_or(ct).trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default_mime.to_owned());
    Ok(InlineMedia {
        data_base64: data,
        mime_type: mime,
    })
}

/// [`StatusSink`] を作る（プレゼンス更新を spawn 先から送る）。
fn make_status_sink(sender: MessageSender) -> StatusSink {
    Arc::new(move |status: BotStatus| match build_presence(status) {
        Ok(presence) => {
            let _ = sender.command(&presence);
        }
        Err(e) => tracing::debug!(error = %e, "presence 構築に失敗"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_audio_prefers_supported_types() {
        let mk = |ct: &str| Attachment {
            content_type: Some(ct.to_owned()),
            ..dummy_attachment()
        };
        let atts = vec![mk("image/png"), mk("audio/ogg; codecs=opus")];
        assert!(select_audio(&atts).is_some());
        assert!(select_image(&atts).is_some());
    }

    fn dummy_attachment() -> Attachment {
        // twilight Attachment は多フィールド。テストでは content_type 以外を既定で埋める。
        Attachment {
            content_type: None,
            ephemeral: false,
            filename: "f".to_owned(),
            description: None,
            duration_secs: None,
            flags: None,
            height: None,
            id: Id::new(1),
            proxy_url: String::new(),
            size: 0,
            title: None,
            url: "https://example.invalid/f".to_owned(),
            waveform: None,
            width: None,
        }
    }
}
