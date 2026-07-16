//! テナント別 Shard poll loop（§8.1.2・絶対制約2の核）。
//!
//! twilight の `Shard` は再接続・resume を内蔵するが **caller が poll し続ける間のみ**動く。本関数は
//! per-shard ループでシャードエラーを log-and-continue し（次 poll で shard 自身が resume）、恒久クローズ
//! （無効トークン等・`CloseCode::can_reconnect()==false`）のみ `Err(ShardClosedFatal)` で返して supervisor
//! に停止判断を委ねる。停止協調は外部から渡す `cancel` future（`select!` 分岐）で受ける。
//!
//! メッセージ/インタラクションは `tokio::spawn` で処理する（poll ループを塞がずハートビートを維持）。

use std::future::Future;

use secrecy::{ExposeSecret, SecretString};
use twilight_gateway::{
    CloseFrame, Event, EventTypeFlags, Intents, MessageSender, Shard, ShardId, StreamExt as _,
};
use twilight_model::gateway::CloseCode;
use twilight_model::guild::PartialMember;
use twilight_model::user::{CurrentUser, User};
use yuuka_core::{BotId, DiscordError};

use crate::interaction::{
    button_custom_id, decide, invoker_id, parse_custom_id, respond, InteractionDeps,
};
use crate::message_flow::{handle_message, FlowDeps, RuntimeBot};
use crate::ports::BotStatus;
use crate::presence::build_presence;

/// テナント（1 ボットトークン）の起動設定。
pub struct TenantConfig {
    pub bot_id: BotId,
    /// 復号済み Discord トークン（`secrecy` でログに漏らさない）。
    pub token: SecretString,
}

/// 現行 `DISCORD_CLIENT_OPTIONS.intents`（GUILDS/GUILD_MESSAGES/MESSAGE_CONTENT/DIRECT_MESSAGES）。
#[must_use]
pub fn default_intents() -> Intents {
    Intents::GUILDS | Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT | Intents::DIRECT_MESSAGES
}

/// 1 テナントの Shard を張り、`cancel` まで poll し続ける（現行 `startCustomBot`＋リスナ相当）。
///
/// # Errors
/// 恒久クローズ（無効トークン・DisallowedIntents 等）を検知した場合 [`DiscordError::ShardClosedFatal`]。
/// 一過性のシャードエラーはループ内で吸収する（プロセスも当タスクも落とさない）。
pub async fn run_tenant(
    cfg: &TenantConfig,
    flow: FlowDeps,
    interaction_deps: InteractionDeps,
    cancel: impl Future<Output = ()> + Send,
) -> Result<(), DiscordError> {
    let mut shard = Shard::new(
        ShardId::ONE,
        cfg.token.expose_secret().to_owned(),
        default_intents(),
    );
    tracing::info!(bot_id = %cfg.bot_id, "Discord テナント起動（Shard poll 開始）");

    let mut runtime: Option<RuntimeBot> = None;
    tokio::pin!(cancel);

    loop {
        tokio::select! {
            () = &mut cancel => {
                tracing::info!(bot_id = %cfg.bot_id, "停止協調: テナントを graceful に閉じます");
                break;
            }
            item = shard.next_event(EventTypeFlags::all()) => {
                let Some(item) = item else {
                    tracing::warn!(bot_id = %cfg.bot_id, "shard ストリーム終了");
                    break;
                };
                let event = match item {
                    Ok(event) => event,
                    Err(source) => {
                        // 0.17 の受信エラーは全て一過性。次 poll で shard が自動再接続する。
                        tracing::warn!(bot_id = %cfg.bot_id, error = %source, "shard 受信エラー（継続）");
                        continue;
                    }
                };

                match event {
                    Event::Ready(ready) => {
                        // sender は同期に取り出す（`&Shard` を await 跨ぎで保持しない＝Shard は
                        // 非 Sync なので、保持すると future が !Send になり supervisor に載らない）。
                        let sender = shard.sender();
                        runtime = Some(on_ready(cfg, &flow, sender, &ready.user).await);
                    }
                    Event::MessageCreate(msg) => {
                        if let Some(rt) = runtime.clone() {
                            let flow = flow.clone();
                            let message = msg.0;
                            tokio::spawn(async move {
                                handle_message(&flow, &rt, message).await;
                            });
                        }
                    }
                    Event::InteractionCreate(interaction) => {
                        let ideps = interaction_deps.clone();
                        let interaction = interaction.0;
                        tokio::spawn(async move {
                            handle_interaction_event(&ideps, interaction).await;
                        });
                    }
                    Event::GatewayClose(frame) => {
                        if is_fatal_close(frame.as_ref()) {
                            let code = frame.as_ref().map(|f| f.code);
                            tracing::error!(bot_id = %cfg.bot_id, ?code, "恒久クローズ（再起動しない）");
                            return Err(DiscordError::ShardClosedFatal { code });
                        }
                        tracing::warn!(bot_id = %cfg.bot_id, "gateway close（自動再接続）");
                    }
                    _ => {}
                }
            }
        }
    }

    // 停止協調/ストリーム終了で抜けた。close フレームを送って gateway セッションを綺麗に閉じる。
    // twilight は close フレームを **次 poll で** 送出するため、送出完了（Close 受領 or ストリーム終了）
    // まで短時間だけ poll し続ける（現行 destroy 相当）。無限待ちを避けるためタイムアウトで打ち切る。
    shard.close(CloseFrame::NORMAL);
    let drain = async {
        while let Some(item) = shard.next_event(EventTypeFlags::all()).await {
            if matches!(item, Ok(Event::GatewayClose(_))) {
                break;
            }
        }
    };
    if tokio::time::timeout(std::time::Duration::from_secs(5), drain)
        .await
        .is_err()
    {
        tracing::debug!(bot_id = %cfg.bot_id, "close フレーム送出の待機がタイムアウト");
    }
    Ok(())
}

/// Ready を受けてランタイム情報を確定する（bot user id / application id / presence sender）。
/// プロフィール同期（現行 §4.3.2 起動時）と idle プレゼンスもここで行う。
async fn on_ready(
    cfg: &TenantConfig,
    flow: &FlowDeps,
    sender: MessageSender,
    user: &CurrentUser,
) -> RuntimeBot {
    let bot_user_id = user.id;

    // Discord プロフィールを DB へ同期。
    let avatar = current_user_avatar_url(user);
    flow.directory
        .update_profile(
            &cfg.bot_id,
            &user.name,
            &avatar,
            &bot_user_id.get().to_string(),
        )
        .await;

    // 起動時は idle プレゼンス（現行 `setBotStatus(client, "idle")`）。
    if let Ok(presence) = build_presence(BotStatus::Idle) {
        let _ = sender.command(&presence);
    }
    tracing::info!(bot_id = %cfg.bot_id, user = %user.name, "Discord ログイン成功");

    RuntimeBot {
        bot_id: cfg.bot_id.clone(),
        bot_user_id,
        sender,
    }
}

/// CurrentUser のアバター URL を組み立てる（現行 `displayAvatarURL()` 相当）。
fn current_user_avatar_url(user: &CurrentUser) -> String {
    match user.avatar {
        Some(hash) => format!(
            "https://cdn.discordapp.com/avatars/{}/{hash}.png",
            user.id.get()
        ),
        None => "https://cdn.discordapp.com/embed/avatars/0.png".to_owned(),
    }
}

/// インタラクションイベントを処理する（ボタン判定 → ラベル解決 → decide → respond）。
async fn handle_interaction_event(
    ideps: &InteractionDeps,
    interaction: twilight_model::application::interaction::Interaction,
) {
    // ボタン以外は無視。custom_id をコピーして借用を解放する。
    let parsed = {
        let Some(custom_id) = button_custom_id(&interaction) else {
            return;
        };
        let (action, id, extra) = parse_custom_id(custom_id);
        (action.to_owned(), id.to_owned(), extra.to_owned())
    };
    let (action, id_str, extra) = parsed;

    let Some(invoker) = invoker_id(&interaction) else {
        return;
    };

    // memreq_apply のみ、owner DM の可読性向上のため申請者名・ギルド名を best-effort 解決する。
    let (applicant_label, guild_label) = if action == "memreq_apply" {
        resolve_interaction_labels(ideps, &interaction).await
    } else {
        (None, None)
    };

    let plan = decide(
        ideps,
        &action,
        &id_str,
        &extra,
        &invoker,
        applicant_label,
        guild_label,
    )
    .await;
    respond(ideps, &interaction, plan).await;
}

/// 申請者表示名（member.nick → global_name → username）とギルド名（best-effort REST 取得）。
async fn resolve_interaction_labels(
    ideps: &InteractionDeps,
    interaction: &twilight_model::application::interaction::Interaction,
) -> (Option<String>, Option<String>) {
    let applicant = interaction
        .member
        .as_ref()
        .and_then(partial_member_nick)
        .or_else(|| interaction.author().and_then(user_global_name))
        .or_else(|| interaction.author().map(|u| u.name.clone()));

    let guild_label = match interaction.guild_id {
        Some(gid) => match ideps.http.guild(gid).await {
            Ok(resp) => resp.model().await.ok().map(|g| g.name),
            Err(_) => None,
        },
        None => None,
    };

    (applicant, guild_label)
}

fn partial_member_nick(member: &PartialMember) -> Option<String> {
    member.nick.clone()
}

fn user_global_name(user: &User) -> Option<String> {
    user.global_name.clone()
}

/// 恒久クローズ（再接続不能）か（`CloseCode::can_reconnect()==false`）。
fn is_fatal_close(frame: Option<&CloseFrame<'_>>) -> bool {
    let Some(frame) = frame else {
        return false;
    };
    match CloseCode::try_from(frame.code) {
        Ok(code) => !code.can_reconnect(),
        // 未知コードは再接続可能側に倒す（無限再起動を避けるより取りこぼしを避ける）。
        Err(_) => false,
    }
}
