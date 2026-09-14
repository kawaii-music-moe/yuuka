//! マルチテナント管理（現行 `startBot`/`stopBot`/`notifier` の統合点）。
//!
//! [`DiscordManager`] は注入ポートを保持し、[`prepare`](DiscordManager::prepare) で起動対象テナントの
//! トークンを解決して**テナント別 twilight http クライアント**（§8.1.3 の既定案 (a)）を作り、各テナントを
//! [`TenantRunner`]（supervisor が `run` を監督する単位）として返す。あわせて [`DiscordMessenger`]
//! （[`Notifier`] 実装＋DM 送信）を作り、FlowDelivery / services から共有配信基盤として使わせる。
//!
//! 起動対象は、停止・停止処分中でなく Discord トークンを持つ**明示作成済み Bot**だけである。

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use secrecy::SecretString;
use twilight_http::Client;
use twilight_model::id::marker::{ChannelMarker, UserMarker};
use twilight_model::id::Id;
use yuuka_core::{BotId, DiscordError, UserId};

use crate::idempotent::{MessageDedup, GUIDANCE_TTL};
use crate::interaction::InteractionDeps;
use crate::message_flow::FlowDeps;
use crate::ports::{
    ActionRow, BotDirectory, Button, ButtonStyle, DeliverTarget, MembershipService, Notifier,
    RateLimiter, TurnProcessor, TurnReply,
};
use crate::reply::send_channel_reply;
use crate::tenant::{run_tenant, TenantConfig, TenantStatus};
use crate::turn_gate::TurnGate;

/// 注入ポート束（呼び出し側が実装を渡す）。
#[derive(Clone)]
pub struct ManagerPorts {
    pub directory: Arc<dyn BotDirectory>,
    pub rate_limiter: Arc<dyn RateLimiter>,
    pub processor: Arc<dyn TurnProcessor>,
    pub membership: Arc<dyn MembershipService>,
}

/// マルチテナント Discord 管理。
pub struct DiscordManager {
    ports: ManagerPorts,
    dedup: Arc<MessageDedup>,
    guidance_throttle: Arc<MessageDedup>,
    turn_gate: Arc<TurnGate>,
}

/// [`prepare`](DiscordManager::prepare) の結果: 監督対象テナント群と共有 Messenger。
pub struct Prepared {
    /// supervisor が各々を [`crate::run_tenant`] 監督する単位。
    pub runners: Vec<TenantRunner>,
    /// 共有配信基盤（notifier + DM 送信）。services/WS へ注入する。
    pub messenger: Arc<DiscordMessenger>,
}

impl DiscordManager {
    #[must_use]
    pub fn new(ports: ManagerPorts) -> Self {
        Self {
            ports,
            dedup: Arc::new(MessageDedup::new()),
            guidance_throttle: Arc::new(MessageDedup::with_ttl(GUIDANCE_TTL)),
            turn_gate: Arc::new(TurnGate::new()),
        }
    }

    /// 起動対象テナントのトークンを解決してクライアントを作り、runner 群＋Messenger を返す。
    ///
    /// トークン欠落・停止/停止処分中の Bot は起動対象から除外する（現行 `startBot` フィルタ）。
    pub async fn prepare(&self) -> Prepared {
        let mut eligible: Vec<(BotId, SecretString)> = Vec::new();

        // 個別 Bot（stopped/suspended でなくトークンがあるもの）のみ起動する。
        for bot in self.ports.directory.list_all_bots().await {
            if bot.stopped || bot.suspended {
                continue;
            }
            if let Some(token) = self.ports.directory.decrypt_token(&bot.id).await {
                eligible.push((bot.id, token));
            }
        }

        // テナント別 http クライアント。
        let mut clients: HashMap<BotId, Arc<Client>> = HashMap::new();
        for (bot_id, token) in &eligible {
            clients.insert(bot_id.clone(), Arc::new(new_client(token)));
        }

        let messenger = Arc::new(DiscordMessenger {
            clients: RwLock::new(clients.clone()),
            directory: self.ports.directory.clone(),
        });

        // runner 群。
        let mut runners = Vec::with_capacity(eligible.len());
        for (bot_id, token) in eligible {
            let http = clients
                .get(&bot_id)
                .cloned()
                .unwrap_or_else(|| Arc::new(new_client(&token)));
            runners.push(self.build_runner(bot_id, token, http, &messenger));
        }

        Prepared { runners, messenger }
    }

    /// 単一テナントの runner を動的に組む（web からの起動/再起動・トークン更新後の再構築）。
    ///
    /// トークンを DB から復号し直し（`restart_default` 等の呼び出し前に保存済み前提）、新しい
    /// REST クライアントを作って messenger にも登録する（通知経路が新トークンを使えるように）。
    /// トークン未登録なら `None`（呼び出し側が「起動失敗」として扱う）。
    pub async fn prepare_runner(
        &self,
        bot_id: &BotId,
        messenger: &Arc<DiscordMessenger>,
    ) -> Option<TenantRunner> {
        let token = self.ports.directory.decrypt_token(bot_id).await?;
        let http = Arc::new(new_client(&token));
        messenger.register_client(bot_id, http.clone());
        Some(self.build_runner(bot_id.clone(), token, http, messenger))
    }

    /// FlowDeps / InteractionDeps を束ねて 1 runner を構築する（prepare / prepare_runner 共通）。
    fn build_runner(
        &self,
        bot_id: BotId,
        token: SecretString,
        http: Arc<Client>,
        messenger: &Arc<DiscordMessenger>,
    ) -> TenantRunner {
        let flow = FlowDeps {
            http: http.clone(),
            directory: self.ports.directory.clone(),
            rate_limiter: self.ports.rate_limiter.clone(),
            processor: self.ports.processor.clone(),
            notifier: messenger.clone(),
            dedup: self.dedup.clone(),
            guidance_throttle: self.guidance_throttle.clone(),
            turn_gate: self.turn_gate.clone(),
        };
        let interaction_deps = InteractionDeps {
            http,
            membership: self.ports.membership.clone(),
            directory: self.ports.directory.clone(),
            dm: messenger.clone(),
        };
        TenantRunner {
            config: TenantConfig { bot_id, token },
            flow,
            interaction_deps,
            status: Arc::new(TenantStatus::default()),
        }
    }
}

/// 1 テナントを supervisor 監督下で走らせる単位。
pub struct TenantRunner {
    pub config: TenantConfig,
    flow: FlowDeps,
    interaction_deps: InteractionDeps,
    status: Arc<TenantStatus>,
}

impl TenantRunner {
    /// テナントの bot_id。
    #[must_use]
    pub fn bot_id(&self) -> &BotId {
        &self.config.bot_id
    }

    /// gateway 接続状態セル（web 層の稼働表示・起動完了待ちが参照する）。
    #[must_use]
    pub fn status(&self) -> Arc<TenantStatus> {
        self.status.clone()
    }

    /// テナント別 REST クライアント（web 層の guild-options 照会が使う）。
    #[must_use]
    pub fn http(&self) -> Arc<Client> {
        self.interaction_deps.http.clone()
    }

    /// Shard poll ループを走らせる（`cancel` まで）。supervisor アダプタから呼ぶ。
    ///
    /// `&self` で再実行可能（依存は clone・token は borrow）。supervisor が panic/一過性障害後に
    /// 同一 runner を再 spawn できる（絶対制約2）。
    ///
    /// # Errors
    /// 恒久クローズ時 [`DiscordError::ShardClosedFatal`]（[`run_tenant`] を参照）。
    pub async fn run(&self, cancel: impl Future<Output = ()> + Send) -> Result<(), DiscordError> {
        run_tenant(
            &self.config,
            self.flow.clone(),
            self.interaction_deps.clone(),
            &self.status,
            cancel,
        )
        .await
    }
}

/// 共有配信基盤（現行 `services/notifier` + DM 送信ヘルパ）。
///
/// クライアント表は `RwLock`（web からの動的 Bot 起動・トークン更新で REST クライアントを
/// 差し替えられるように）。読みは短時間の read lock でクローンを取り出し、await を跨がない。
pub struct DiscordMessenger {
    clients: RwLock<HashMap<BotId, Arc<Client>>>,
    directory: Arc<dyn BotDirectory>,
}

impl DiscordMessenger {
    /// 動的起動/トークン更新時に REST クライアントを登録・差し替える（prepare_runner から呼ぶ）。
    pub fn register_client(&self, bot_id: &BotId, client: Arc<Client>) {
        if let Ok(mut map) = self.clients.write() {
            map.insert(bot_id.clone(), client);
        }
    }

    /// bot_id 指定時のクライアント解決。対象の個別 Bot が起動していない場合は配信しない。
    ///
    /// **意図的 divergence**: Node は `readyAt`（gateway 接続済み）を要求するが、twilight の HTTP
    /// クライアントはトークンだけで REST 送信できる（gateway 非依存）ため readiness ゲートを設けない。
    /// gateway 未接続でも DM/チャンネル送信は成功する＝Node が拒否するケースでも配信できる（改善側）。
    fn resolve_client(&self, bot_id: &BotId) -> Option<Arc<Client>> {
        self.clients
            .read()
            .ok()
            .and_then(|m| m.get(bot_id).cloned())
    }

    /// DM チャンネルを開いて channel id を得る。
    async fn open_dm(client: &Client, user_id: Id<UserMarker>) -> Option<Id<ChannelMarker>> {
        match client.create_private_channel(user_id).await {
            Ok(resp) => match resp.model().await {
                Ok(channel) => Some(channel.id),
                Err(e) => {
                    tracing::warn!(error = %e, "DM チャンネルの取得に失敗");
                    None
                }
            },
            Err(e) => {
                tracing::warn!(error = %e, "DM チャンネルの作成に失敗");
                None
            }
        }
    }

    /// 送信先を twilight channel id へ解決する。ギルドチャンネル宛は対象ユーザーの在籍を検証し、
    /// 未在籍/検証失敗なら `None`（呼び出し側が DM へフォールバック）。
    async fn resolve_channel(
        client: &Client,
        user_id: &UserId,
        target: &DeliverTarget,
    ) -> Option<Id<ChannelMarker>> {
        match target {
            DeliverTarget::Channel(id) => {
                let channel_id = id.parse::<Id<ChannelMarker>>().ok()?;
                // 現行 notifier.ts の第三者チャンネル露出ガード: 共有/デフォルト Bot を踏み台に、
                // 対象ユーザーが見られないチャンネルへその人のデータを流し込むのを防ぐ。
                if Self::user_can_view_channel(client, channel_id, user_id).await {
                    Some(channel_id)
                } else {
                    None
                }
            }
            DeliverTarget::Dm => {
                let uid = user_id.as_str().parse::<Id<UserMarker>>().ok()?;
                Self::open_dm(client, uid).await
            }
        }
    }

    /// 対象ユーザーがこのチャンネルを閲覧してよいか（現行 notifier.ts の membership + ViewChannel）。
    ///
    /// チャンネルを引いて guild を判定し、ギルドチャンネルなら対象ユーザーがそのギルドのメンバーで
    /// あることを REST で確認する（在籍＝第一次ガード）。厳密な per-channel ViewChannel overwrite 判定は
    /// twilight-util の権限計算で精緻化予定（services が任意チャンネル宛を注入する Phase 4）。ギルド外
    /// （DM/グループ）チャンネルはガード対象外。検証失敗は安全側（不可）に倒し DM フォールバックさせる。
    async fn user_can_view_channel(
        client: &Client,
        channel_id: Id<ChannelMarker>,
        user_id: &UserId,
    ) -> bool {
        let Ok(uid) = user_id.as_str().parse::<Id<UserMarker>>() else {
            return false;
        };
        let channel = match client.channel(channel_id).await {
            Ok(resp) => resp.model().await.ok(),
            Err(_) => None,
        };
        let Some(channel) = channel else {
            return false;
        };
        match channel.guild_id {
            // ギルド外チャンネル（DM 等）は露出ガードの対象外。
            None => true,
            // ギルドチャンネル: 対象ユーザーが在籍していれば許可（member fetch 成功＝在籍）。
            Some(guild_id) => client.guild_member(guild_id, uid).await.is_ok(),
        }
    }

    /// Bot を特定できない所有者通知は送らない。共有既定 Bot への横流しを禁止する。
    async fn send_owner_dm(&self, _user_id: &str, _content: &str, _components: &[ActionRow]) -> bool {
        tracing::warn!("送信元 Bot が未指定の所有者 DM は送信しません");
        false
    }

    // ── DM 送信ヘルパ（web/services が呼ぶ・現行 sendShareInviteDM 等） ──

    /// 共有招待 DM（承認/辞退ボタン付き・現行 `sendShareInviteDM`）。
    pub async fn send_share_invite_dm(
        &self,
        shared_user_id: &str,
        bot_name: &str,
        owner_name: &str,
        recommended_persona_name: Option<&str>,
        share_id: i64,
    ) -> bool {
        let persona_info = recommended_persona_name.map_or_else(String::new, |name| {
            format!(
                "\n\nこのBotには推奨ペルソナ「**{name}**」が設定されています。承認後にインポートするか選択できます。"
            )
        });
        let content = format!(
            "📨 **{owner_name}** さんがあなたをBot「**{bot_name}**」に招待しました。{persona_info}"
        );
        let components = vec![ActionRow {
            buttons: vec![
                Button {
                    custom_id: format!("share_accept:{share_id}"),
                    label: "承認する".to_owned(),
                    style: ButtonStyle::Success,
                },
                Button {
                    custom_id: format!("share_decline:{share_id}"),
                    label: "辞退する".to_owned(),
                    style: ButtonStyle::Secondary,
                },
            ],
        }];
        self.send_owner_dm(shared_user_id, &content, &components)
            .await
    }

    /// 利用申請の受付 DM（承認/却下ボタン付き・現行 `sendMemberRequestDM`）。
    pub async fn send_member_request_dm(
        &self,
        owner_id: &str,
        bot_name: &str,
        applicant_label: &str,
        guild_label: &str,
        note: Option<&str>,
        request_id: i64,
    ) -> bool {
        let note_line = note
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map_or_else(String::new, |n| format!("\n\n📝 申請メッセージ:\n> {n}"));
        let content = format!(
            "🙋 Bot「**{bot_name}**」への利用申請が届きました。\n\n\
             申請者: {applicant_label}\n\
             ギルド: {guild_label}{note_line}\n\n\
             下のボタンから承認/却下できます（管理画面の「利用申請」からも操作できます）。"
        );
        let components = vec![ActionRow {
            buttons: vec![
                Button {
                    custom_id: format!("memreq_approve:{request_id}"),
                    label: "承認する".to_owned(),
                    style: ButtonStyle::Success,
                },
                Button {
                    custom_id: format!("memreq_reject:{request_id}"),
                    label: "却下する".to_owned(),
                    style: ButtonStyle::Danger,
                },
            ],
        }];
        self.send_owner_dm(owner_id, &content, &components).await
    }

    /// 利用申請の結果 DM（現行 `sendMemberDecisionDM`）。
    pub async fn send_member_decision_dm(
        &self,
        applicant_id: &str,
        bot_name: &str,
        approved: bool,
    ) -> bool {
        let content = if approved {
            format!("✅ Bot「**{bot_name}**」の利用申請が承認されました。メンションや返信で利用できます。")
        } else {
            format!("🙇 Bot「**{bot_name}**」の利用申請は承認されませんでした。")
        };
        self.send_owner_dm(applicant_id, &content, &[]).await
    }

    /// 登録確認コード DM（現行 `sendRegistrationCodeDM`。コードはログに出さない）。
    pub async fn send_registration_code_dm(&self, discord_id: &str, code: &str) -> bool {
        let content = format!(
            "🔐 **Yuuka アカウント登録の確認コード**\n\n\
             確認コード: **{code}**\n\n\
             Web登録画面にこのコードを入力すると登録が完了します（10分間有効）。\n\
             ※ このDMに心当たりがない場合は、誰かがあなたのDiscord IDで登録を試みています。コードは入力しないでください。"
        );
        self.send_owner_dm(discord_id, &content, &[]).await
    }
}

#[async_trait]
impl crate::ports::MemberDmSender for DiscordMessenger {
    async fn send_request_dm(
        &self,
        owner_id: &str,
        bot_name: &str,
        applicant_label: &str,
        guild_label: &str,
        note: Option<&str>,
        request_id: i64,
    ) -> bool {
        self.send_member_request_dm(
            owner_id,
            bot_name,
            applicant_label,
            guild_label,
            note,
            request_id,
        )
        .await
    }

    async fn send_decision_dm(&self, applicant_id: &str, bot_name: &str, approved: bool) -> bool {
        self.send_member_decision_dm(applicant_id, bot_name, approved)
            .await
    }
}

#[async_trait]
impl Notifier for DiscordMessenger {
    async fn send_to_user(
        &self,
        user_id: &UserId,
        reply: TurnReply,
        target: DeliverTarget,
        bot_id: &BotId,
    ) -> bool {
        let Some(client) = self.resolve_client(bot_id) else {
            tracing::warn!(user = %user_id, "利用可能な Bot クライアントがありません");
            return false;
        };
        // Channel 宛が解決不能（不明チャンネル / 対象ユーザーが閲覧不可）なら DM へフォールバックする
        // （Node `sendToUser` notifier.ts:130-142・cron 経路は deliver_final を通らないため本メソッドで担保）。
        let channel_id = match Self::resolve_channel(&client, user_id, &target).await {
            Some(cid) => cid,
            None => match &target {
                DeliverTarget::Channel(_) => {
                    let Some(dm) =
                        Self::resolve_channel(&client, user_id, &DeliverTarget::Dm).await
                    else {
                        return false;
                    };
                    tracing::info!(user = %user_id, "チャンネル宛が解決不能のため DM へフォールバック");
                    dm
                }
                DeliverTarget::Dm => return false,
            },
        };
        // notifier はフレッシュ送信（reply_to なし）。分割・添付は send_channel_reply に委譲。
        let _ = &self.directory; // 将来 botId 無し経路（listBotsForUser）で使用。
        send_channel_reply(&client, channel_id, None, &reply).await
    }
}

/// テナント別 http クライアントを作る（トークン別・レート bucket 別・§8.1.3 案 (a)）。
fn new_client(token: &SecretString) -> Client {
    use secrecy::ExposeSecret as _;
    Client::new(token.expose_secret().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn messenger_with(clients: &[&str]) -> DiscordMessenger {
        let map: HashMap<BotId, Arc<Client>> = clients
            .iter()
            .map(|id| (BotId::new(*id), Arc::new(Client::new(String::new()))))
            .collect();
        DiscordMessenger {
            clients: RwLock::new(map),
            directory: Arc::new(NoopDirectory),
        }
    }

    // twilight `Client::new` は ratelimiter で Tokio ランタイムを要求するため tokio::test で回す。
    #[tokio::test]
    async fn resolve_client_requires_the_requested_bot() {
        let m = messenger_with(&["botX"]);
        assert!(m.resolve_client(&BotId::new("botX")).is_some());
        assert!(m.resolve_client(&BotId::new("unknown")).is_none());
    }

    #[tokio::test]
    async fn resolve_client_none_when_not_registered() {
        let m = messenger_with(&["botX"]);
        // custom はある。
        assert!(m.resolve_client(&BotId::new("botX")).is_some());
        // 未知の custom かつデフォルト無し → None。
        assert!(m.resolve_client(&BotId::new("unknown")).is_none());
    }

    #[tokio::test]
    async fn register_client_updates_map_and_default() {
        let m = messenger_with(&[]);
        // 未登録 → None。
        assert!(m.resolve_client(&BotId::new("botX")).is_none());
        // 個別 Bot を動的登録 → 当該 Bot だけ解決可。
        m.register_client(&BotId::new("botX"), Arc::new(Client::new(String::new())));
        assert!(m.resolve_client(&BotId::new("botX")).is_some());
        assert!(m.resolve_client(&BotId::new("unknown")).is_none());
    }

    // 最小ダミー directory（resolve_client のテストでは実際には呼ばれない）。
    struct NoopDirectory;
    use crate::ports::BotRecord;
    use yuuka_core::GuildId;
    #[async_trait]
    impl BotDirectory for NoopDirectory {
        async fn get_bot(&self, _b: &BotId) -> Option<BotRecord> {
            None
        }
        async fn list_all_bots(&self) -> Vec<BotRecord> {
            Vec::new()
        }
        async fn list_bots_for_user(&self, _u: &UserId) -> Vec<BotId> {
            Vec::new()
        }
        async fn decrypt_token(&self, _b: &BotId) -> Option<SecretString> {
            None
        }
        async fn update_profile(&self, _b: &BotId, _u: &str, _a: &str, _d: &str) {}
        async fn is_registered_user(&self, _u: &UserId) -> bool {
            false
        }
        async fn is_bot_member(&self, _b: &BotId, _g: &GuildId, _u: &UserId) -> bool {
            false
        }
        async fn is_guild_allowed(&self, _b: &BotId, _g: &GuildId) -> bool {
            false
        }
        async fn is_channel_enabled(&self, _b: &BotId, _g: &GuildId, _c: &str) -> bool {
            false
        }
        async fn is_channel_muted(&self, _b: &BotId, _g: &GuildId, _c: &str) -> bool {
            false
        }
        async fn is_any_role_allowed(&self, _b: &BotId, _g: &GuildId, _r: &[String]) -> bool {
            false
        }
    }
}
