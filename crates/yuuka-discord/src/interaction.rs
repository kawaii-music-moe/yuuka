//! ボタンインタラクション処理（現行 `handleInteraction` [`src/bot.ts:377-532`]）。
//!
//! `custom_id` を `action:id:extra` で分解し、共有招待の承認/辞退・利用申請の申請/承認/却下・
//! 推奨ペルソナのインポートを処理する。判定ロジック [`decide`] は twilight 非依存の注入ポート
//! （[`MembershipService`]/[`BotDirectory`]）越しなので fake でユニットテストできる。twilight への
//! 送信は [`respond`] に隔離する。エラーは握り潰さず（現行の空 `catch{}` は廃止）ログする。

use std::sync::Arc;

use twilight_http::Client;
use twilight_model::application::interaction::{Interaction, InteractionData};
use twilight_model::channel::message::component::ComponentType;
use twilight_model::channel::message::MessageFlags;
use twilight_model::http::interaction::{InteractionResponse, InteractionResponseType};
use twilight_util::builder::InteractionResponseDataBuilder;
use yuuka_core::{BotId, UserId};

use crate::ports::{self, BotDirectory, MemberDecision, MemberDmSender, MembershipService};
use crate::reply::to_twilight_components;

/// インタラクション処理に必要な依存（注入ポート＋twilight http）。
#[derive(Clone)]
pub struct InteractionDeps {
    pub http: Arc<Client>,
    pub membership: Arc<dyn MembershipService>,
    pub directory: Arc<dyn BotDirectory>,
    /// 利用申請の結果 DM 送信（Node `sendMemberRequestDM`/`sendMemberDecisionDM`）。
    pub dm: Arc<dyn MemberDmSender>,
}

/// フォローアップ送信の記述（共有承認後の推奨ペルソナ導入確認）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Followup {
    pub content: String,
    pub components: Vec<ports::ActionRow>,
}

/// インタラクションへの応答計画（twilight 非依存・テスト検証用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionPlan {
    /// ephemeral な即時返信（`interaction.reply({ ephemeral: true })`）。
    Reply(String),
    /// 元メッセージ更新でボタンを消して結果表示（`interaction.update`）。任意でフォローアップを続ける。
    Update {
        content: String,
        followup: Option<Followup>,
    },
    /// 何もしない（未知 action・非ボタン）。
    Ignore,
}

/// `custom_id` を `action:id:extra` へ分解する（現行 `customId.split(":")`・最大 3 分割）。
#[must_use]
pub fn parse_custom_id(custom_id: &str) -> (&str, &str, &str) {
    let mut parts = custom_id.splitn(3, ':');
    let action = parts.next().unwrap_or("");
    let id = parts.next().unwrap_or("");
    let extra = parts.next().unwrap_or("");
    (action, id, extra)
}

/// ボタン以外・データ無しなら `None`、ボタンなら `custom_id` を返す（現行 `interaction.isButton()`）。
#[must_use]
pub fn button_custom_id(interaction: &Interaction) -> Option<&str> {
    match interaction.data.as_ref()? {
        InteractionData::MessageComponent(data) if data.component_type == ComponentType::Button => {
            Some(&data.custom_id)
        }
        _ => None,
    }
}

/// インタラクション処理の判定本体（現行 `handleInteraction` の分岐 1:1）。
///
/// `invoker` は操作したユーザー、`applicant_label`/`guild_label` は memreq_apply の可読性向上のため
/// 呼び出し側が best-effort 解決した表示名（現行 `interaction.guild.members.fetch(...).displayName`）。
///
/// **利用申請の受付/結果 DM（Node `sendMemberRequestDM`/`sendMemberDecisionDM`）は本関数から送る**
/// （Discord ボタン経路）。将来 `/ws/chat` がコンポーネント dispatch を持つ場合、Node が「全経路で DM を
/// 送る」挙動（`componentInteractionService`）に合わせて DM 送信を再配線する必要がある。
pub async fn decide(
    deps: &InteractionDeps,
    action: &str,
    id_str: &str,
    extra: &str,
    invoker: &UserId,
    applicant_label: Option<String>,
    guild_label: Option<String>,
) -> InteractionPlan {
    match action {
        // ── 利用申請（メンバー外ユーザーがボタンから申請） ──
        "memreq_apply" => {
            let (bot_id, guild_id) = (id_str, extra);
            if bot_id.is_empty() || guild_id.is_empty() {
                return InteractionPlan::Reply("申請情報が不正です。".to_owned());
            }
            // owner 宛 DM 用の表示ラベルを先に解決（Node: `applicantLabel?.trim() || ユーザー ${id}`）。
            let applicant_dm_label = label_or(applicant_label.as_deref(), || {
                format!("ユーザー {}", invoker.as_str())
            });
            let guild_dm_label = label_or(guild_label.as_deref(), || format!("ギルド {guild_id}"));
            let outcome = deps
                .membership
                .submit_member_request(
                    &BotId::new(bot_id),
                    guild_id,
                    invoker,
                    applicant_label,
                    guild_label,
                )
                .await;
            // 受付を Bot オーナーへ DM（承認/却下ボタン付き・Node `sendMemberRequestDM`）。ボタン経由は
            // note 無し。fire-and-forget（false は握り潰す・DB 上の申請は Web 管理から拾える）。
            if outcome.ok {
                if let (Some(owner), Some(name), Some(rid)) =
                    (&outcome.owner_id, &outcome.bot_name, outcome.request_id)
                {
                    deps.dm
                        .send_request_dm(
                            owner,
                            name,
                            &applicant_dm_label,
                            &guild_dm_label,
                            None,
                            rid,
                        )
                        .await;
                }
            }
            let msg = if outcome.ok {
                "✅ 利用申請を送信しました。Bot作成者の承認をお待ちください。".to_owned()
            } else {
                format!("⚠️ {}", outcome.message)
            };
            InteractionPlan::Reply(msg)
        }

        // ── 利用申請の承認/却下（Bot オーナーが DM ボタンから操作） ──
        "memreq_approve" | "memreq_reject" => {
            let decision = if action == "memreq_approve" {
                MemberDecision::Approved
            } else {
                MemberDecision::Rejected
            };
            let Some(request_id) = parse_id(id_str) else {
                return InteractionPlan::Reply("申請が見つかりません。".to_owned());
            };
            let outcome = deps
                .membership
                .decide_member_request(request_id, decision, invoker)
                .await;
            if !outcome.ok {
                return InteractionPlan::Reply(outcome.message);
            }
            // 申請者へ結果 DM（Node `sendMemberDecisionDM`・ボタンなし・fire-and-forget）。
            if let (Some(applicant), Some(name)) = (&outcome.applicant_id, &outcome.bot_name) {
                let approved = matches!(outcome.status, Some(MemberDecision::Approved));
                deps.dm.send_decision_dm(applicant, name, approved).await;
            }
            let bot_name = outcome.bot_name.unwrap_or_default();
            let content = match outcome.status {
                Some(MemberDecision::Approved) => {
                    format!("✅ Bot「**{bot_name}**」の利用申請を承認しました。")
                }
                _ => format!("🚫 Bot「**{bot_name}**」の利用申請を却下しました。"),
            };
            InteractionPlan::Update {
                content,
                followup: None,
            }
        }

        // ── 共有招待の承認/辞退 ──
        "share_accept" | "share_decline" => {
            let share = match parse_id(id_str) {
                Some(sid) => deps.membership.get_share(sid).await,
                None => None,
            };
            let Some(share) = share else {
                return InteractionPlan::Reply(
                    "この招待はあなた宛ではないか、既に無効です。".to_owned(),
                );
            };
            if &share.shared_user_id != invoker {
                return InteractionPlan::Reply(
                    "この招待はあなた宛ではないか、既に無効です。".to_owned(),
                );
            }
            if share.status != "pending" {
                return InteractionPlan::Reply("この招待は既に処理済みです。".to_owned());
            }
            if action == "share_decline" {
                deps.membership
                    .revoke_share(&share.bot_id, &share.shared_user_id)
                    .await;
                return InteractionPlan::Update {
                    content: "招待を辞退しました。".to_owned(),
                    followup: None,
                };
            }
            // accept
            deps.membership
                .accept_share(&share.bot_id, &share.shared_user_id)
                .await;
            let bot = deps.directory.get_bot(&share.bot_id).await;
            let bot_name = bot
                .as_ref()
                .map_or_else(|| share.bot_id.as_str().to_owned(), |b| b.name.clone());
            let content = format!("✅ Bot「**{bot_name}**」へのアクセスが有効になりました！");

            // 推奨ペルソナ（公開）のインポート確認をフォローアップ（§5.2.2）。
            let followup =
                build_persona_followup(deps, bot.and_then(|b| b.recommended_persona_id)).await;
            InteractionPlan::Update { content, followup }
        }

        // ── 推奨ペルソナのインポート ──
        "persona_import" => {
            if !deps.directory.is_registered_user(invoker).await {
                return InteractionPlan::Reply("先にユーザー登録を完了してください。".to_owned());
            }
            let Some(persona_id) = parse_id(id_str) else {
                return InteractionPlan::Update {
                    content:
                        "ペルソナのインポートに失敗しました（非公開化された可能性があります）。"
                            .to_owned(),
                    followup: None,
                };
            };
            let ok = deps.membership.import_persona(invoker, persona_id).await;
            let content = if ok {
                "✅ ペルソナをインポートしました。管理画面の「ペルソナ」から適用できます。"
                    .to_owned()
            } else {
                "ペルソナのインポートに失敗しました（非公開化された可能性があります）。".to_owned()
            };
            InteractionPlan::Update {
                content,
                followup: None,
            }
        }

        _ => InteractionPlan::Ignore,
    }
}

/// 推奨ペルソナが公開なら、インポート確認ボタン付きフォローアップを組み立てる。
async fn build_persona_followup(
    deps: &InteractionDeps,
    recommended_persona_id: Option<i64>,
) -> Option<Followup> {
    let persona_id = recommended_persona_id?;
    let persona = deps.membership.get_public_persona(persona_id).await?;
    // ラベルはペルソナ名を 60 字で切る（現行 `persona.name.slice(0, 60)`）。
    let short_name: String = persona.name.chars().take(60).collect();
    Some(Followup {
        content:
            "このBotの推奨ペルソナをインポートしますか？（任意です。インポート後は独立したコピーとなります）"
                .to_owned(),
        components: vec![ports::ActionRow {
            buttons: vec![ports::Button {
                custom_id: format!("persona_import:{}", persona.id),
                label: format!("ペルソナ「{short_name}」をインポート"),
                style: ports::ButtonStyle::Primary,
            }],
        }],
    })
}

/// `parseInt(idStr, 10)` 相当（非数値/空は None）。
fn parse_id(s: &str) -> Option<i64> {
    s.trim().parse::<i64>().ok()
}

/// 表示ラベルを trim して非空なら採用、無ければ `fallback`（Node の `label?.trim() || default`）。
fn label_or(label: Option<&str>, fallback: impl FnOnce() -> String) -> String {
    match label.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => s.to_owned(),
        None => fallback(),
    }
}

/// 判定結果を twilight で応答送信する（現行 `interaction.reply/update/followUp`）。
///
/// 失敗は握り潰さずログする。`application_id`/`interaction.id`/`interaction.token` を使い
/// `InteractionClient::create_response`（＋必要なら `create_followup`）を呼ぶ。
pub async fn respond(deps: &InteractionDeps, interaction: &Interaction, plan: InteractionPlan) {
    let client = deps.http.interaction(interaction.application_id);
    match plan {
        InteractionPlan::Ignore => {}
        InteractionPlan::Reply(content) => {
            let data = InteractionResponseDataBuilder::new()
                .content(content)
                .flags(MessageFlags::EPHEMERAL)
                .build();
            let resp = InteractionResponse {
                kind: InteractionResponseType::ChannelMessageWithSource,
                data: Some(data),
            };
            if let Err(e) = client
                .create_response(interaction.id, &interaction.token, &resp)
                .await
            {
                tracing::warn!(error = %e, "インタラクション応答の送信に失敗");
            }
        }
        InteractionPlan::Update { content, followup } => {
            // 元メッセージを更新し、ボタンを消す（components を空で送る）。
            let data = InteractionResponseDataBuilder::new()
                .content(content)
                .components(std::iter::empty())
                .build();
            let resp = InteractionResponse {
                kind: InteractionResponseType::UpdateMessage,
                data: Some(data),
            };
            if let Err(e) = client
                .create_response(interaction.id, &interaction.token, &resp)
                .await
            {
                tracing::warn!(error = %e, "インタラクション更新の送信に失敗");
                return;
            }
            if let Some(followup) = followup {
                let components = to_twilight_components(&followup.components);
                let mut req = client
                    .create_followup(&interaction.token)
                    .content(&followup.content);
                if !components.is_empty() {
                    req = req.components(&components);
                }
                if let Err(e) = req.await {
                    tracing::warn!(error = %e, "フォローアップの送信に失敗");
                }
            }
        }
    }
}

/// invoker（操作ユーザー）の UserId を取り出す（`interaction.author_id()`）。
#[must_use]
pub fn invoker_id(interaction: &Interaction) -> Option<UserId> {
    interaction
        .author_id()
        .map(|id| UserId::new(id.get().to_string()))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;
    use crate::ports::{
        BotRecord, DecisionOutcome, MemberDmSender, PersonaRecord, ShareRecord, SubmitOutcome,
    };

    #[test]
    fn parse_custom_id_splits_three() {
        assert_eq!(
            parse_custom_id("share_accept:42"),
            ("share_accept", "42", "")
        );
        assert_eq!(
            parse_custom_id("memreq_apply:bot1:guild9"),
            ("memreq_apply", "bot1", "guild9")
        );
        assert_eq!(
            parse_custom_id("persona_import"),
            ("persona_import", "", "")
        );
        // extra に `:` が含まれても 3 分割で残りは extra へ（現行 split の 3 要素 destructure と一致）。
        assert_eq!(parse_custom_id("a:b:c:d"), ("a", "b", "c:d"));
    }

    // ── fake ports ──────────────────────────────────────────────────────────

    #[derive(Default)]
    struct FakeMembership {
        share: Option<ShareRecord>,
        persona: Option<PersonaRecord>,
        submit_ok: bool,
        decide: Option<DecisionOutcome>,
        import_ok: bool,
    }

    #[async_trait]
    impl MembershipService for FakeMembership {
        async fn submit_member_request(
            &self,
            _bot_id: &BotId,
            _guild_id: &str,
            _applicant: &UserId,
            _applicant_label: Option<String>,
            _guild_label: Option<String>,
        ) -> SubmitOutcome {
            SubmitOutcome {
                ok: self.submit_ok,
                message: if self.submit_ok {
                    String::new()
                } else {
                    "既に申請済みです。".to_owned()
                },
                owner_id: self.submit_ok.then(|| "owner1".to_owned()),
                bot_name: self.submit_ok.then(|| "テストBot".to_owned()),
                request_id: self.submit_ok.then_some(99),
            }
        }
        async fn decide_member_request(
            &self,
            _request_id: i64,
            _decision: MemberDecision,
            _actor: &UserId,
        ) -> DecisionOutcome {
            self.decide.clone().unwrap_or(DecisionOutcome {
                ok: false,
                message: "申請が見つかりません。".to_owned(),
                status: None,
                bot_name: None,
                applicant_id: None,
            })
        }
        async fn get_share(&self, _share_id: i64) -> Option<ShareRecord> {
            self.share.clone()
        }
        async fn accept_share(&self, _bot_id: &BotId, _shared_user: &UserId) {}
        async fn revoke_share(&self, _bot_id: &BotId, _shared_user: &UserId) {}
        async fn import_persona(&self, _user_id: &UserId, _persona_id: i64) -> bool {
            self.import_ok
        }
        async fn get_public_persona(&self, _persona_id: i64) -> Option<PersonaRecord> {
            self.persona.clone()
        }
    }

    #[derive(Default)]
    struct FakeDirectory {
        bot: Option<BotRecord>,
        registered: bool,
    }

    #[async_trait]
    impl BotDirectory for FakeDirectory {
        async fn get_bot(&self, _bot_id: &BotId) -> Option<BotRecord> {
            self.bot.clone()
        }
        async fn list_all_bots(&self) -> Vec<BotRecord> {
            Vec::new()
        }
        async fn list_bots_for_user(&self, _user_id: &UserId) -> Vec<BotId> {
            Vec::new()
        }
        async fn decrypt_token(&self, _bot_id: &BotId) -> Option<secrecy::SecretString> {
            None
        }
        async fn update_profile(&self, _b: &BotId, _u: &str, _a: &str, _d: &str) {}
        async fn is_registered_user(&self, _user_id: &UserId) -> bool {
            self.registered
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

    use yuuka_core::GuildId;

    /// 受付 DM の記録（owner_id, request_id, applicant_label, guild_label, note あり?）。
    type RequestDm = (String, i64, String, String, bool);

    /// 送信された DM を記録する fake（member DM 配線 + ラベル解決の検証用）。
    #[derive(Default)]
    struct FakeDmSender {
        request_dms: Mutex<Vec<RequestDm>>,
        decision_dms: Mutex<Vec<(String, bool)>>,
    }

    #[async_trait]
    impl MemberDmSender for FakeDmSender {
        async fn send_request_dm(
            &self,
            owner_id: &str,
            _bot_name: &str,
            applicant_label: &str,
            guild_label: &str,
            note: Option<&str>,
            request_id: i64,
        ) -> bool {
            self.request_dms.lock().unwrap().push((
                owner_id.to_owned(),
                request_id,
                applicant_label.to_owned(),
                guild_label.to_owned(),
                note.is_some(),
            ));
            true
        }
        async fn send_decision_dm(
            &self,
            applicant_id: &str,
            _bot_name: &str,
            approved: bool,
        ) -> bool {
            self.decision_dms
                .lock()
                .unwrap()
                .push((applicant_id.to_owned(), approved));
            true
        }
    }

    fn deps(membership: FakeMembership, directory: FakeDirectory) -> InteractionDeps {
        deps_dm(membership, directory, Arc::new(FakeDmSender::default()))
    }

    fn deps_dm(
        membership: FakeMembership,
        directory: FakeDirectory,
        dm: Arc<FakeDmSender>,
    ) -> InteractionDeps {
        InteractionDeps {
            http: Arc::new(Client::new(String::new())),
            membership: Arc::new(membership),
            directory: Arc::new(directory),
            dm,
        }
    }

    #[tokio::test]
    async fn memreq_apply_invalid_info() {
        let d = deps(FakeMembership::default(), FakeDirectory::default());
        let plan = decide(&d, "memreq_apply", "", "", &UserId::new("u"), None, None).await;
        assert_eq!(
            plan,
            InteractionPlan::Reply("申請情報が不正です。".to_owned())
        );
    }

    #[tokio::test]
    async fn memreq_apply_success() {
        let d = deps(
            FakeMembership {
                submit_ok: true,
                ..Default::default()
            },
            FakeDirectory::default(),
        );
        let plan = decide(
            &d,
            "memreq_apply",
            "bot1",
            "g9",
            &UserId::new("u"),
            None,
            None,
        )
        .await;
        assert_eq!(
            plan,
            InteractionPlan::Reply(
                "✅ 利用申請を送信しました。Bot作成者の承認をお待ちください。".to_owned()
            )
        );
    }

    #[tokio::test]
    async fn share_accept_wrong_recipient_is_rejected() {
        let d = deps(
            FakeMembership {
                share: Some(ShareRecord {
                    bot_id: BotId::new("bot1"),
                    shared_user_id: UserId::new("other"),
                    status: "pending".to_owned(),
                }),
                ..Default::default()
            },
            FakeDirectory::default(),
        );
        let plan = decide(&d, "share_accept", "1", "", &UserId::new("me"), None, None).await;
        assert_eq!(
            plan,
            InteractionPlan::Reply("この招待はあなた宛ではないか、既に無効です。".to_owned())
        );
    }

    #[tokio::test]
    async fn share_accept_offers_persona_followup() {
        let d = deps(
            FakeMembership {
                share: Some(ShareRecord {
                    bot_id: BotId::new("bot1"),
                    shared_user_id: UserId::new("me"),
                    status: "pending".to_owned(),
                }),
                persona: Some(PersonaRecord {
                    id: 7,
                    name: "ずんだもん".to_owned(),
                }),
                ..Default::default()
            },
            FakeDirectory {
                bot: Some(BotRecord {
                    id: BotId::new("bot1"),
                    owner_id: UserId::new("owner"),
                    name: "テスト秘書".to_owned(),
                    suspended: false,
                    stopped: false,
                    recommended_persona_id: Some(7),
                    is_guild_assistant: false,
                    has_gemini_key: true,
                }),
                registered: true,
            },
        );
        let plan = decide(&d, "share_accept", "1", "", &UserId::new("me"), None, None).await;
        assert!(
            matches!(plan, InteractionPlan::Update { .. }),
            "expected Update, got {plan:?}"
        );
        let InteractionPlan::Update { content, followup } = plan else {
            return;
        };
        assert!(content.contains("テスト秘書"), "bot 名を含む: {content}");
        let f = followup.expect("推奨ペルソナのフォローアップ");
        assert_eq!(f.components[0].buttons[0].custom_id, "persona_import:7");
    }

    #[tokio::test]
    async fn persona_import_requires_registration() {
        let d = deps(
            FakeMembership {
                import_ok: true,
                ..Default::default()
            },
            FakeDirectory {
                registered: false,
                ..Default::default()
            },
        );
        let plan = decide(&d, "persona_import", "7", "", &UserId::new("u"), None, None).await;
        assert_eq!(
            plan,
            InteractionPlan::Reply("先にユーザー登録を完了してください。".to_owned())
        );
    }

    #[tokio::test]
    async fn unknown_action_ignored() {
        let d = deps(FakeMembership::default(), FakeDirectory::default());
        let plan = decide(
            &d,
            "totally_unknown",
            "1",
            "",
            &UserId::new("u"),
            None,
            None,
        )
        .await;
        assert_eq!(plan, InteractionPlan::Ignore);
    }

    #[tokio::test]
    async fn memreq_apply_sends_owner_dm_on_success() {
        let dm = Arc::new(FakeDmSender::default());
        let d = deps_dm(
            FakeMembership {
                submit_ok: true,
                ..Default::default()
            },
            FakeDirectory::default(),
            dm.clone(),
        );
        let plan = decide(
            &d,
            "memreq_apply",
            "bot1",
            "g9",
            &UserId::new("u"),
            Some("申請太郎".to_owned()),
            Some("テストギルド".to_owned()),
        )
        .await;
        assert!(matches!(plan, InteractionPlan::Reply(_)));
        let sent = dm.request_dms.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "owner1", "owner 宛");
        assert_eq!(sent[0].1, 99, "request_id");
        assert_eq!(sent[0].2, "申請太郎", "trim 済み表示ラベル採用");
        assert_eq!(sent[0].3, "テストギルド");
        assert!(!sent[0].4, "ボタン経路は note なし");
    }

    #[tokio::test]
    async fn memreq_apply_dm_uses_label_fallback_when_absent() {
        let dm = Arc::new(FakeDmSender::default());
        let d = deps_dm(
            FakeMembership {
                submit_ok: true,
                ..Default::default()
            },
            FakeDirectory::default(),
            dm.clone(),
        );
        // applicant_label 無し・guild_label は空白のみ → Node の `label?.trim() || default` フォールバック。
        let _ = decide(
            &d,
            "memreq_apply",
            "bot1",
            "g9",
            &UserId::new("u"),
            None,
            Some("   ".to_owned()),
        )
        .await;
        let sent = dm.request_dms.lock().unwrap();
        assert_eq!(
            sent[0].2, "ユーザー u",
            "applicant 無しは ユーザー+id フォールバック"
        );
        assert_eq!(
            sent[0].3, "ギルド g9",
            "guild 空白は ギルド+guildId フォールバック"
        );
    }

    #[tokio::test]
    async fn memreq_decide_sends_applicant_dm_on_success() {
        let dm = Arc::new(FakeDmSender::default());
        let d = deps_dm(
            FakeMembership {
                decide: Some(DecisionOutcome {
                    ok: true,
                    message: String::new(),
                    status: Some(MemberDecision::Approved),
                    bot_name: Some("テストBot".to_owned()),
                    applicant_id: Some("applicant1".to_owned()),
                }),
                ..Default::default()
            },
            FakeDirectory::default(),
            dm.clone(),
        );
        let plan = decide(
            &d,
            "memreq_approve",
            "5",
            "",
            &UserId::new("owner"),
            None,
            None,
        )
        .await;
        assert!(matches!(plan, InteractionPlan::Update { .. }));
        let sent = dm.decision_dms.lock().unwrap();
        assert_eq!(sent.as_slice(), &[("applicant1".to_owned(), true)]);
    }
}
