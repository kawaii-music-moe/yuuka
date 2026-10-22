//! メッセージハンドリングの 2 経路（現行 `src/bot.ts`）。
//!
//! - [`assistant_flow`] — 汎用モード（ギルド常駐アシスタント）。現行 `handleAssistantMessage`
//!   [`src/bot.ts:659-931`]。許可ギルド・メンバー制・Bot 専用キー・レート制限の防衛線を通す。
//! - [`secretary_flow`] — 秘書（デフォルト/独自）。現行 `setupMessageListener` インライン
//!   [`src/bot.ts:958-1263`]。登録ユーザー・共有アクセス・メンション/返信を通す。
//!
//! ルーティングは [`handle_message`]（`author.bot` 無視 → 冪等ガード → Bot 種別で分岐）。防衛線の
//! 判定は注入ポート（[`BotDirectory`]/[`TurnProcessor`]）越し。twilight 送信は
//! [`crate::reply`] に隔離する。純ヘルパ（表示名解決・添付選別・返信プレフィックス）はユニットテスト。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use twilight_gateway::MessageSender;
use twilight_http::Client;
use twilight_model::channel::{Attachment, Message};
use twilight_model::id::marker::{ChannelMarker, MessageMarker, RoleMarker, UserMarker};
use twilight_model::id::Id;
use yuuka_core::{BotId, DiscordError, GuildId, UserId};

use crate::idempotent::MessageDedup;
use crate::ports::{
    rate_limit_message, BotDirectory, BotRecord, BotStatus, DeliverTarget, IncomingChat,
    InlineMedia, Notifier, ReplyAncestor, Speaker, StatusSink, TurnDelivery, TurnError,
    TurnProcessor, TurnReply,
};
use crate::presence::build_presence;
use crate::reply::{send_channel_reply, send_channel_text};
use crate::streaming::StreamingReply;
use crate::text::{
    is_supported_audio, strip_all_mentions, strip_self_mention, NON_MEMBER_GUIDANCE,
};
use crate::turn_gate::TurnGate;

// ─── 定型文（現行文言 1:1） ────────────────────────────────────────────────────

const SECRETARY_DEFAULT_HELP: &str =
    "何かお手伝いできることはありますか？ 📋\n\nタスク管理、予定管理、家計管理、ブラウザ操作ができますよ！";
/// システム Bot（`system_default`）の既定ヘルプ。個人の秘書ではなく Yuuka の使い方・設定の案内役。
const SYSTEM_BOT_DEFAULT_HELP: &str =
    "Yuuka の使い方や設定について質問してください。管理画面のどこで設定できるかをご案内します。\n\nタスク・予定・家計などは、ご自身のエージェント（Bot）でご利用ください。";
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
    /// ギルド別の Bot 自身のロール（統合ロール＋保持ロール）。ロールメンション判定と
    /// `<@&統合ロール>` の除去に使う。gateway イベントで更新する（[`OwnGuildRoles`]）。
    pub own_roles: Arc<std::sync::Mutex<OwnGuildRoles>>,
}

impl RuntimeBot {
    /// 自ロール表を参照・更新する（GUILD_CREATE / GUILD_MEMBER_UPDATE / GUILD_DELETE から呼ぶ）。
    pub fn with_own_roles<R>(&self, f: impl FnOnce(&mut OwnGuildRoles) -> R) -> R {
        let mut roles = self
            .own_roles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut roles)
    }

    /// ギルドの Bot 統合ロール ID（未知なら `None`）。
    fn integration_role(&self, guild_id: u64) -> Option<u64> {
        self.with_own_roles(|r| r.integration_role(guild_id))
    }
}

/// ギルドごとの Bot 自身のロール控え（現行 discord.js の `guild.members.me.roles` 相当）。
///
/// - 統合ロール: GUILD_CREATE の `role.tags.bot_id == 自 Bot` で控える（`@Bot名` 補完のロールメンション）。
/// - 保持ロール: GUILD_CREATE の自メンバー（Discord は intent に関わらず自 Bot を `members` に含める）
///   と、自分宛の GUILD_MEMBER_UPDATE で丸ごと置き換える（手動付与の共用ロール等）。
///
/// メッセージごとの member fetch はしない。@everyone ロール（ID = guild_id）は `mention_everyone`
/// 側で扱うので保持ロールから除く。
#[derive(Debug, Default)]
pub struct OwnGuildRoles {
    integration: std::collections::HashMap<u64, u64>,
    held: std::collections::HashMap<u64, std::collections::HashSet<u64>>,
}

impl OwnGuildRoles {
    /// 統合ロールを控える。
    pub fn note_integration_role(&mut self, guild_id: u64, role_id: u64) {
        self.integration.insert(guild_id, role_id);
    }

    /// 自メンバーの保持ロールを丸ごと置き換える（@everyone ロールは除く）。
    pub fn set_held_roles(&mut self, guild_id: u64, roles: impl IntoIterator<Item = u64>) {
        let held = roles.into_iter().filter(|&r| r != guild_id).collect();
        self.held.insert(guild_id, held);
    }

    /// ギルドから離脱した際に控えを捨てる。
    pub fn forget_guild(&mut self, guild_id: u64) {
        self.integration.remove(&guild_id);
        self.held.remove(&guild_id);
    }

    /// ギルドの統合ロール ID（未知なら `None`）。
    #[must_use]
    pub fn integration_role(&self, guild_id: u64) -> Option<u64> {
        self.integration.get(&guild_id).copied()
    }

    /// `mention_roles` のいずれかが自 Bot のロール（統合ロール＋保持ロール）か。DM（guild なし）は偽。
    #[must_use]
    pub fn is_mentioned(&self, guild_id: Option<u64>, mention_roles: &[Id<RoleMarker>]) -> bool {
        let Some(gid) = guild_id else {
            return false;
        };
        let integration = self.integration_role(gid);
        let held = self.held.get(&gid);
        mention_roles
            .iter()
            .map(|r| r.get())
            .any(|r| r != gid && (integration == Some(r) || held.is_some_and(|h| h.contains(&r))))
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
        // system_default はレコードが無くても常に応答する（秘書経路を通り、エンジンがシステム Bot の
        // 案内役として処理する）。custom はレコード必須（無ければ黙殺）。
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
    let stream = StreamingReply::new(
        deps.http.clone(),
        message.channel_id,
        message.id,
        typing.handle(),
    );
    let delivery = make_delivery(
        deps,
        rt,
        &message,
        is_dm,
        author.clone(),
        typing.handle(),
        Arc::clone(&stream),
    );
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
                    reply_chain: resolve_reply_chain(
                        deps,
                        &message,
                        reference.as_ref(),
                        rt.bot_user_id,
                    )
                    .await,
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
                    reply_chain: resolve_reply_chain(
                        deps,
                        &message,
                        reference.as_ref(),
                        rt.bot_user_id,
                    )
                    .await,
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
            reply_chain: resolve_reply_chain(deps, &message, reference.as_ref(), rt.bot_user_id)
                .await,
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
    finish_reply(deps, &message, result, /* skip_silent */ true, &stream).await;
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
    let stream = StreamingReply::new(
        deps.http.clone(),
        message.channel_id,
        message.id,
        typing.handle(),
    );
    let delivery = make_delivery(
        deps,
        rt,
        &message,
        is_dm,
        author.clone(),
        typing.handle(),
        Arc::clone(&stream),
    );
    let discord_msg_id = Some(message.id.get().to_string());
    let reply_to_msg_id = reference_message_id(&message);
    let channel_id = Some(message.channel_id.get().to_string());

    // Only the current message's authenticated author can supply saveable attachments.
    // Reference images remain context only; their bytes never enter the save catalog.
    // 会話の入力（画像1枚・音声）は従来どおりで、保存候補のバイト列は Gemini へ送らない。
    // 取得できない・上限を超える添付は候補から外すだけで、ターンは失敗させない。
    let timeline_attachments = current_timeline_attachments(message.id, &message.attachments).await;
    let result: Result<TurnReply, TurnError> = if let Some(att) = audio {
        match fetch_inline_media(&att, "audio/ogg").await {
            Ok(media) => {
                // 現行 `const instruction = fullText.trim() || placeholder`（trim 済み）。
                let instruction = if full_text.trim().is_empty() {
                    if is_custom {
                        SECRETARY_AUDIO_PLACEHOLDER
                    } else {
                        ASSISTANT_AUDIO_PLACEHOLDER
                    }
                    .to_owned()
                } else {
                    full_text.trim().to_owned()
                };
                let chat = IncomingChat {
                    text: instruction,
                    audio: Some(media),
                    timeline_attachments,
                    discord_msg_id,
                    reply_to_msg_id,
                    reply_chain: resolve_reply_chain(
                        deps,
                        &message,
                        reference.as_ref(),
                        rt.bot_user_id,
                    )
                    .await,
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
        // 現メッセージの画像は保存候補として取得済みなら再利用する（同じ添付を二重に取得しない）。
        let current = timeline_attachments
            .iter()
            .find(|a| a.attachment_id == att.id.to_string())
            .map(|a| InlineMedia {
                data_base64: a.data_base64.clone(),
                mime_type: a.mime_type.clone(),
            });
        let media = match current {
            Some(media) => Ok(media),
            None => fetch_inline_media(&att, "image/jpeg").await,
        };
        match media {
            Ok(media) => {
                // レシート OCR は秘書専用。キャプションは現メッセージ本文（空なら指示なし）。会話ログの
                // 紐づけ（R08）のため、メッセージ ID・返信先・チャンネル・返信元チェーンも渡す。
                let chat = IncomingChat {
                    text: text.clone(),
                    timeline_attachments,
                    discord_msg_id,
                    reply_to_msg_id,
                    reply_chain: resolve_reply_chain(
                        deps,
                        &message,
                        reference.as_ref(),
                        rt.bot_user_id,
                    )
                    .await,
                    channel_id,
                    ..IncomingChat::default()
                };
                deps.processor
                    .parse_receipt(
                        &rt.bot_id,
                        &author,
                        media,
                        chat,
                        status.clone(),
                        delivery.clone(),
                    )
                    .await
            }
            Err(e) => Err(TurnError::Failed(e.to_string())),
        }
    } else if !full_text.trim().is_empty() {
        // 動画だけを添えた発話もここに来る（動画は従来どおり会話の入力にせず、保存候補だけ渡す）。
        let chat = IncomingChat {
            text: full_text.clone(),
            timeline_attachments,
            discord_msg_id,
            reply_to_msg_id,
            reply_chain: resolve_reply_chain(deps, &message, reference.as_ref(), rt.bot_user_id)
                .await,
            channel_id,
            ..IncomingChat::default()
        };
        deps.processor
            .process_secretary(&rt.bot_id, &author, chat, status.clone(), delivery.clone())
            .await
    } else if is_custom {
        Ok(TurnReply::text(SECRETARY_DEFAULT_HELP))
    } else {
        Ok(TurnReply::text(SYSTEM_BOT_DEFAULT_HELP))
    };

    drop(typing);
    status(BotStatus::Idle);
    // 秘書は空応答でも既定ヘルプで必ず何か送る（silent skip 無し）。
    finish_reply(
        deps, &message, result, /* skip_silent */ false, &stream,
    )
    .await;
}

/// 処理結果を返信送信する（エラーは定型文へ縮退・現行 try/catch）。`skip_silent` は
/// アシスタントの「空応答は黙殺」挙動（秘書は false）。本文を逐次表示していたら、その返信を最終結果
/// （エラー時は定型文）で確定する。
async fn finish_reply(
    deps: &FlowDeps,
    message: &Message,
    result: Result<TurnReply, TurnError>,
    skip_silent: bool,
    stream: &StreamingReply,
) {
    match result {
        Ok(reply) => {
            // 後続配信へ回したターン（R07）は途中応答を送信済みで、最終結果は配信ハンドルが送る。
            if reply.deferred || stream.finish(&reply).await || (skip_silent && reply.is_silent()) {
                return;
            }
            send_channel_reply(&deps.http, message.channel_id, Some(message.id), &reply).await;
        }
        Err(TurnError::RateLimited(exceeded)) => {
            send_channel_text(
                &deps.http,
                message.channel_id,
                Some(message.id),
                &rate_limit_message(exceeded),
                &[],
            )
            .await;
        }
        Err(e) => {
            tracing::error!(error = %e, "ターン処理に失敗（定型エラーへ縮退）");
            if stream.finish(&TurnReply::text(GENERIC_ERROR)).await {
                return;
            }
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
    /// 生成中の本文の逐次表示（最終結果もこの返信へ確定する）。
    stream: Arc<StreamingReply>,
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
        // 本文を逐次表示していたら、その返信を最終結果で確定する（送り先は逐次表示と同じチャンネル）。
        if self.stream.finish(&reply).await {
            return;
        }
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

    fn shows_partial_text(&self) -> bool {
        true
    }

    fn on_partial_text(&self, text: &str) {
        self.stream.push(text);
    }
}

fn make_delivery(
    deps: &FlowDeps,
    rt: &RuntimeBot,
    message: &Message,
    is_dm: bool,
    user_id: UserId,
    typing_abort: tokio::task::AbortHandle,
    stream: Arc<StreamingReply>,
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
        stream,
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
    id: Id<MessageMarker>,
    /// この返信先がさらに返信していた先（同じチャンネルのときだけ・返信元チェーン用）。
    parent: Option<Id<MessageMarker>>,
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
        let parent = m
            .reference
            .as_ref()
            .filter(|r| r.channel_id.is_none_or(|c| c == m.channel_id))
            .and_then(|r| r.message_id);
        Self {
            id: m.id,
            parent,
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

/// 返信元チェーンの上限件数と 1 件の最大文字数（orchestrator `message_log` の
/// `REPLY_CHAIN_MAX`/`REPLY_CHAIN_ITEM_CHARS` と同じ値。engine 側でも同じ上限で揃える）。
const REPLY_CHAIN_MAX: usize = 8;
const REPLY_CHAIN_ITEM_CHARS: usize = 1200;
/// 返信元チェーンの取得にかける時間の上限（超えたら取得済みの分だけ使う）。
const REPLY_CHAIN_FETCH_TIMEOUT: Duration = Duration::from_secs(3);

/// 返信元チェーン（R08）を Discord から取得する。古い順・各件 `[発言者] 本文`（自 Bot は「あなた」）。
///
/// 返信先（[`resolve_reference`] で解決済み）から、さらにその返信先を受信チャンネル内だけで REST 取得して
/// 遡る（Discord は返信先を 1 段しか埋め込まないため）。返信のたびに REST を叩かないよう、応答すると
/// 決めた後のターン組み立てでだけ呼ぶ。循環・取得失敗・[`REPLY_CHAIN_FETCH_TIMEOUT`] 超過で打ち切り、
/// 本文のない発言（添付のみ等）は飛ばす。各件は Discord のメッセージ ID を持ち、engine が会話ログから
/// 復元した分（取得が途中で切れたときの補完）と ID で重複を除いて合わせる。
async fn resolve_reply_chain(
    deps: &FlowDeps,
    message: &Message,
    reference: Option<&RefInfo>,
    bot_user_id: Id<UserMarker>,
) -> Vec<ReplyAncestor> {
    let Some(reference) = reference else {
        return Vec::new();
    };
    let mut chain: Vec<ReplyAncestor> = reply_chain_entry(reference, bot_user_id)
        .into_iter()
        .collect();
    let mut seen = std::collections::HashSet::from([reference.id]);
    let mut next = reference.parent;
    let fetch = async {
        while chain.len() < REPLY_CHAIN_MAX {
            let Some(mid) = next.take() else {
                break;
            };
            if !seen.insert(mid) {
                break;
            }
            let ancestor = match deps.http.message(message.channel_id, mid).await {
                Ok(resp) => match resp.model().await {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::warn!(error = %e, "返信元チェーンの取得に失敗しました");
                        break;
                    }
                },
                Err(e) => {
                    tracing::warn!(error = %e, "返信元チェーンの取得に失敗しました");
                    break;
                }
            };
            if ancestor.channel_id != message.channel_id {
                break;
            }
            let info = RefInfo::from_message(&ancestor);
            chain.extend(reply_chain_entry(&info, bot_user_id));
            next = info.parent;
        }
    };
    if tokio::time::timeout(REPLY_CHAIN_FETCH_TIMEOUT, fetch)
        .await
        .is_err()
    {
        tracing::debug!("返信元チェーンの取得が時間切れ: 取得済みの分だけ使います");
    }
    chain.reverse();
    chain
}

/// 返信元チェーンの 1 件（本文が空なら `None`）。
fn reply_chain_entry(info: &RefInfo, bot_user_id: Id<UserMarker>) -> Option<ReplyAncestor> {
    let content = info.content.trim();
    if content.is_empty() {
        return None;
    }
    let name = if info.author_id == bot_user_id {
        "あなた"
    } else {
        info.author_name.as_str()
    };
    let content: String = content.chars().take(REPLY_CHAIN_ITEM_CHARS).collect();
    Some(ReplyAncestor {
        message_id: info.id.get().to_string(),
        text: format!("[{name}] {content}"),
    })
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
/// [`is_bot_role_mentioned`]（統合ロール＋手動付与の共用ロール等の保持ロール）で移植する。
fn is_user_mentioned(message: &Message, user_id: Id<UserMarker>) -> bool {
    message.mention_everyone || message.mentions.iter().any(|m| m.id == user_id)
}

/// Bot 自身のロールへのロールメンションか。Discord の `@Bot名` 補完はユーザーメンションではなく
/// **統合ロールのロールメンション**になることがあり（`message.mentions` は空・`mention_roles` に
/// ロール ID）、Bot が保持する一般ロール（共用ロール等）へのメンションも含めて discord.js の
/// `mentions.has()` は真とするため移植する。保持ロールは gateway イベントで控えた値で判定する
/// （[`OwnGuildRoles`]・メッセージごとの member fetch はしない）。
fn is_bot_role_mentioned(rt: &RuntimeBot, message: &Message) -> bool {
    let guild_id = message.guild_id.map(Id::get);
    rt.with_own_roles(|r| r.is_mentioned(guild_id, &message.mention_roles))
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

fn is_photo_or_video(a: &Attachment) -> bool {
    a.content_type
        .as_deref()
        .is_some_and(|m| m.starts_with("image/") || m.starts_with("video/"))
}

/// タイムライン保存候補の上限（保存ツール `addTimelineRecord` の検査と同じ値）。
const TIMELINE_ATTACHMENT_MAX_COUNT: usize = 10;
const TIMELINE_ATTACHMENT_MAX_TOTAL_BYTES: usize = 20 * 1024 * 1024;
/// 保存候補の取得にかける時間の上限（会話の応答をこれ以上待たせない）。
const TIMELINE_ATTACHMENT_FETCH_BUDGET: Duration = Duration::from_secs(30);

/// 現メッセージの写真・動画を、タイムライン保存の候補として取得する。
///
/// Discord の認証済み CDN（`/attachments/`）だけをリダイレクト無しで取得する。会話そのものは
/// 止めないため、取得に失敗した添付や、件数・合計サイズ・取得時間の上限を超える添付は候補から外す
/// （`warn` ログのみ）。候補は保存ツールの入力にだけ使い、Gemini へ inline では送らない。
async fn current_timeline_attachments(
    message_id: Id<MessageMarker>,
    attachments: &[Attachment],
) -> Vec<yuuka_core::tool::ConversationAttachment> {
    let candidates: Vec<(usize, &Attachment)> = attachments
        .iter()
        .enumerate()
        .filter(|(_, a)| is_photo_or_video(a))
        .collect();
    if candidates.is_empty() {
        return Vec::new();
    }
    let client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(TIMELINE_ATTACHMENT_FETCH_BUDGET)
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            tracing::warn!(error = %e, "保存候補の添付を取得できません（候補なしで続行）");
            return Vec::new();
        }
    };
    let deadline = tokio::time::Instant::now() + TIMELINE_ATTACHMENT_FETCH_BUDGET;
    let mut catalog = Vec::new();
    let mut total = 0usize;
    for (position, att) in candidates {
        if catalog.len() >= TIMELINE_ATTACHMENT_MAX_COUNT {
            tracing::warn!(attachment = %att.id, "保存候補の件数上限を超えた添付を除外");
            continue;
        }
        // Discord が通知するサイズで、取得する前に合計上限の超過を除外する（取得中も実バイト数で再確認）。
        let remaining = TIMELINE_ATTACHMENT_MAX_TOTAL_BYTES - total;
        if !usize::try_from(att.size).is_ok_and(|size| size <= remaining) {
            tracing::warn!(attachment = %att.id, size = att.size, "保存候補の合計サイズ上限を超えた添付を除外");
            continue;
        }
        let bytes = match tokio::time::timeout_at(
            deadline,
            fetch_timeline_attachment(&client, att, remaining),
        )
        .await
        {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(reason)) => {
                tracing::warn!(attachment = %att.id, reason, "保存候補の添付を除外");
                continue;
            }
            Err(_) => {
                tracing::warn!(attachment = %att.id, "保存候補の取得時間の上限に達したため、以降の添付を除外");
                break;
            }
        };
        total += bytes.len();
        let mime_type = att
            .content_type
            .as_deref()
            .unwrap_or_default()
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        catalog.push(yuuka_core::tool::ConversationAttachment {
            source_message_id: format!("discord:{message_id}"),
            attachment_id: att.id.to_string(),
            position,
            mime_type,
            data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        });
    }
    catalog
}

/// Fetch only Discord's authenticated current-message CDN attachments, with no redirects.
async fn fetch_timeline_attachment(
    client: &reqwest::Client,
    att: &Attachment,
    limit: usize,
) -> Result<Vec<u8>, &'static str> {
    let url = reqwest::Url::parse(&att.url).map_err(|_| "添付URLが不正です")?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("cdn.discordapp.com" | "media.discordapp.net")
        )
        || !url.path().starts_with("/attachments/")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return Err("Discord添付以外は取得しません");
    }
    let mut response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|_| "添付の取得に失敗しました")?;
    if response.status().is_redirection() {
        return Err("添付の転送は許可しません");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "添付の読込に失敗しました")?
    {
        if bytes.len() + chunk.len() > limit {
            return Err("添付合計の上限（20MiB）を超えます");
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.is_empty() {
        return Err("添付が空です");
    }
    Ok(bytes)
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

    /// 保存候補を作れない添付があっても失敗にせず、候補から外して会話を続ける（R13 レビュー指摘）。
    /// どちらの添付も取得前に除外されるため、ネットワークには出ない。
    #[tokio::test]
    async fn timeline_catalog_skips_oversized_and_foreign_attachments() {
        let video = Attachment {
            content_type: Some("video/mp4".to_owned()),
            size: 20 * 1024 * 1024 + 1,
            url: "https://cdn.discordapp.com/attachments/1/2/large.mp4".to_owned(),
            ..dummy_attachment()
        };
        let foreign = Attachment {
            content_type: Some("image/png".to_owned()),
            id: Id::new(2),
            size: 3,
            url: "https://example.invalid/attachments/photo.png".to_owned(),
            ..dummy_attachment()
        };
        let audio = Attachment {
            content_type: Some("audio/ogg".to_owned()),
            id: Id::new(3),
            ..dummy_attachment()
        };
        let catalog = current_timeline_attachments(Id::new(42), &[video, foreign, audio]).await;
        assert!(catalog.is_empty());
    }

    fn roles(ids: &[u64]) -> Vec<Id<RoleMarker>> {
        ids.iter().map(|&i| Id::new(i)).collect()
    }

    const GUILD: u64 = 100;
    const INTEGRATION: u64 = 200;
    const SHARED: u64 = 300;
    const UNRELATED: u64 = 400;

    fn own_roles() -> OwnGuildRoles {
        let mut r = OwnGuildRoles::default();
        r.note_integration_role(GUILD, INTEGRATION);
        // 実際の自メンバーは統合ロールも保持するが、統合ロール単独でも判定できることを見るため外す。
        r.set_held_roles(GUILD, [SHARED]);
        r
    }

    #[test]
    fn role_mention_hits_integration_role() {
        assert!(own_roles().is_mentioned(Some(GUILD), &roles(&[INTEGRATION])));
    }

    #[test]
    fn role_mention_hits_manually_assigned_held_role() {
        let r = own_roles();
        assert!(r.is_mentioned(Some(GUILD), &roles(&[SHARED])));
        assert!(r.is_mentioned(Some(GUILD), &roles(&[UNRELATED, SHARED])));
    }

    #[test]
    fn role_mention_ignores_unrelated_role_and_other_guild() {
        let r = own_roles();
        assert!(!r.is_mentioned(Some(GUILD), &roles(&[UNRELATED])));
        assert!(!r.is_mentioned(Some(GUILD), &[]));
        // 別ギルドでは同じ ID でも控えが無いので偽。
        assert!(!r.is_mentioned(Some(999), &roles(&[SHARED, INTEGRATION])));
    }

    #[test]
    fn role_mention_is_false_in_dm() {
        assert!(!own_roles().is_mentioned(None, &roles(&[SHARED, INTEGRATION])));
    }

    #[test]
    fn everyone_role_is_not_counted_as_held_role() {
        // @everyone ロール（ID = guild_id）は mention_everyone 側で扱うため二重に数えない。
        let mut r = OwnGuildRoles::default();
        r.set_held_roles(GUILD, [GUILD, SHARED]);
        assert!(!r.is_mentioned(Some(GUILD), &roles(&[GUILD])));
        assert!(r.is_mentioned(Some(GUILD), &roles(&[SHARED])));
    }

    #[test]
    fn held_roles_are_replaced_on_update_and_forgotten_on_leave() {
        let mut r = own_roles();
        // GUILD_MEMBER_UPDATE で共用ロールが外れ、別ロールが付いた。
        r.set_held_roles(GUILD, [UNRELATED]);
        assert!(!r.is_mentioned(Some(GUILD), &roles(&[SHARED])));
        assert!(r.is_mentioned(Some(GUILD), &roles(&[UNRELATED])));
        // 統合ロールは保持ロール更新の影響を受けない。
        assert_eq!(r.integration_role(GUILD), Some(INTEGRATION));

        r.forget_guild(GUILD);
        assert!(!r.is_mentioned(Some(GUILD), &roles(&[UNRELATED, INTEGRATION])));
        assert_eq!(r.integration_role(GUILD), None);
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
