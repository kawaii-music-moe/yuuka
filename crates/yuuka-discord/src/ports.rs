//! Discord 層の外部依存を表す**注入ポート（トレイト）**と provider 中立 DTO（§8.1・§8.3）。
//!
//! `yuuka-discord` は twilight トランスポート＋テナント監督に徹し、以下は**呼び出し側が実装して
//! 注入する**（現行 `src/bot.ts` が `gemini.ts`/`services/*`/`db/*` を直接 import していた依存を、
//! テスト可能なトレイト境界へ反転させる）。gemini の [`GenerateBackend`]・web の [`AuthBackend`] と
//! 同じ規律：本番は実装を注入し、テストは fake を注入してネットワーク無しで意味論を検証する。
//!
//! - [`TurnProcessor`]  — ターン処理（現行 `processMessage`/`processGuildMessage`/
//!   `processBotDmMessage`/`parseReceipt`）。オーケストレーション本体は Phase 4 で実装される。
//! - [`BotDirectory`]   — Bot メタデータ・アクセス判定・トークン復号・プロフィール同期
//!   （現行 `db/botRepo`/`db/botAttributesRepo`/`db/userRepo`/`services/botCapabilities`）。
//! - [`MembershipService`] — 利用申請・共有招待・ペルソナインポート（ボタンフロー、
//!   現行 `services/memberRequest`/`db/botRepo`/`db/personaRepo`）。
//! - [`RateLimiter`]    — レート制限（現行 `services/botRateLimit`）。
//! - [`TurnDelivery`]   — 重い処理の一時応答／最終配信ハンドル（現行 `TurnAsyncDelivery`）。
//!   これは**本クレートが実装して processor へ渡す**方向のポート（notifier 相当は本クレートの
//!   Messenger が担う）。

use std::sync::Arc;

use async_trait::async_trait;
use yuuka_core::{BotId, GuildId, UserId};

// ─── 共通 DTO ────────────────────────────────────────────────────────────────

/// base64 埋め込みメディア（画像・音声）。現行 `ChatMessage.imageData`/`audioData` 相当。
#[derive(Debug, Clone)]
pub struct InlineMedia {
    /// base64 エンコード済みデータ。
    pub data_base64: String,
    /// MIME（`image/jpeg` / `audio/ogg` 等・`;` 以降は除去済み）。
    pub mime_type: String,
}

/// ターン処理への入力（現行 `ChatMessage`）。
#[derive(Debug, Clone, Default)]
pub struct IncomingChat {
    /// 本文（メンション除去・返信先プレフィックス結合済み）。
    pub text: String,
    /// 添付画像（あれば）。
    pub image: Option<InlineMedia>,
    /// 添付音声（あれば）。
    pub audio: Option<InlineMedia>,
    /// 受信 Discord メッセージ ID（会話ログ永続化用）。
    pub discord_msg_id: Option<String>,
    /// 返信元 Discord メッセージ ID（返信チェーン解決用）。
    pub reply_to_msg_id: Option<String>,
}

/// 発話者（現行 `speaker { userId, displayName }`）。
#[derive(Debug, Clone)]
pub struct Speaker {
    pub user_id: UserId,
    pub display_name: String,
}

/// Bot プレゼンス状態（現行 `setBotStatus` の 3 値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BotStatus {
    /// 考え中（dnd + カスタムアクティビティ）。
    Thinking,
    /// 書き込み中（online + カスタムアクティビティ）。
    Writing,
    /// 待機（online・アクティビティ無し）。
    Idle,
}

/// プレゼンス通知シンク（processor へ渡す・現行 `statusCallback`）。
pub type StatusSink = Arc<dyn Fn(BotStatus) + Send + Sync>;

/// ターン処理の結果（現行 `ProcessResult`）。provider 中立。
#[derive(Debug, Clone, Default)]
pub struct TurnReply {
    /// 最終テキスト（空かつ embeds/files 無しなら「応答なし＝黙殺」）。
    pub text: String,
    /// リッチ埋め込み（現行 `embeds`）。
    pub embeds: Vec<RichEmbed>,
    /// ファイル添付（グラフ PNG 等・現行 `files`）。
    pub files: Vec<FileAttachment>,
    /// 対話コンポーネント（action row・現行 `components`）。
    pub components: Vec<ActionRow>,
    /// 重い処理を非同期化したため text は一時応答で最終は別途配信される（現行 `deferred`）。
    pub deferred: bool,
}

impl TurnReply {
    /// テキストのみの結果。
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    /// テキストも埋め込みもファイルも無い＝黙殺すべき空応答か（現行 `!text && embeds.length===0 && files.length===0`）。
    #[must_use]
    pub fn is_silent(&self) -> bool {
        self.text.is_empty() && self.embeds.is_empty() && self.files.is_empty()
    }
}

/// ファイル添付（現行 `{ attachment: Buffer, name }`）。
#[derive(Debug, Clone)]
pub struct FileAttachment {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// リッチ埋め込みの provider 中立記述（reply 層で twilight `Embed` へ写像）。
#[derive(Debug, Clone, Default)]
pub struct RichEmbed {
    pub title: Option<String>,
    pub description: Option<String>,
    /// 0xRRGGBB。
    pub color: Option<u32>,
    pub fields: Vec<EmbedField>,
    pub footer: Option<String>,
}

/// 埋め込みフィールド。
#[derive(Debug, Clone)]
pub struct EmbedField {
    pub name: String,
    pub value: String,
    pub inline: bool,
}

/// ボタン行の provider 中立記述（reply/interaction 層で twilight `Component` へ写像）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActionRow {
    pub buttons: Vec<Button>,
}

/// ボタン（現行 `ButtonBuilder`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    /// `action:id:extra` 形式の custom_id。
    pub custom_id: String,
    pub label: String,
    pub style: ButtonStyle,
}

/// ボタンスタイル（現行 `ButtonStyle`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonStyle {
    Primary,
    Secondary,
    Success,
    Danger,
}

/// 送信先（現行 notifier `NotifyTarget`）。
#[derive(Debug, Clone)]
pub enum DeliverTarget {
    /// DM（userId を使う）。
    Dm,
    /// チャンネル（channel id）。
    Channel(String),
}

// ─── TurnDelivery（本クレートが実装 → processor へ渡す） ────────────────────────

/// 重い処理ターンの非同期配信ハンドル（現行 `TurnAsyncDelivery`）。
///
/// 本クレートの `FlowDelivery` が実装し、[`TurnProcessor`] の各メソッドへ渡す。processor は実行時に
/// 重い処理を検知したら [`on_interim`](Self::on_interim) で一時応答し、事前予測で非同期化した
/// ターンは完了後に [`deliver_final`](Self::deliver_final) で配信する。配信先（DM/チャンネル）は
/// メッセージ受信時に確定するため実装が保持し、引数には取らない（現行 `deliverFinal(payload)` と同じ）。
#[async_trait]
pub trait TurnDelivery: Send + Sync {
    /// 実行時に重い処理を検知した際の一時応答（「入力中…」を止める用途も兼ねる）。
    async fn on_interim(&self, text: String);

    /// 事前予測で非同期化したターンの最終結果を、完了後に配信する。
    async fn deliver_final(&self, reply: TurnReply);
}

/// ユーザーへの通知配信（現行 `services/notifier.sendToUser`）。本クレートの Messenger が実装し、
/// services/WS など上位層へ注入して使わせる（リマインド・日報等の共通配信基盤）。
#[async_trait]
pub trait Notifier: Send + Sync {
    /// ユーザーへ配信する（`bot_id` 本人のクライアントで届け、オフライン時は既定 Bot へフォールバック）。
    /// 送信成功なら `true`（現行 `sendToUser` の戻り）。
    async fn send_to_user(
        &self,
        user_id: &UserId,
        reply: TurnReply,
        target: DeliverTarget,
        bot_id: &BotId,
    ) -> bool;
}

// ─── TurnProcessor（オーケストレーション・Phase 4 実装） ────────────────────────

/// ターン処理エラー（processor 内部失敗）。呼び出し側はこれを受けて定型エラー文面を返す
/// （現行 bot.ts の try/catch → 「処理中にエラーが発生しました」に相当）。握り潰さない。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TurnError {
    /// 上流（Gemini/synapse 等）障害。
    #[error("turn processing failed: {0}")]
    Failed(String),
}

/// ターン処理（現行 `gemini.ts` の 3 経路＋レシート）。
///
/// **本クレートはこの結果を Discord へ配信するだけ**で、systemInstruction 組立・シナプス想起・
/// ターンプランナー・会話ログ保存といったオーケストレーションは実装側（Phase 4 の gemini 上位層）の
/// 責務（§8.2 の役割分担）。個別失敗は [`TurnError`] で返し、呼び出し側が定型応答へ縮退する。
#[async_trait]
pub trait TurnProcessor: Send + Sync {
    /// 秘書経路（現行 `processMessage`）。DM・ギルド双方の秘書 Bot が使う。
    async fn process_secretary(
        &self,
        bot_id: &BotId,
        user_id: &UserId,
        msg: IncomingChat,
        status: StatusSink,
        delivery: Arc<dyn TurnDelivery>,
    ) -> Result<TurnReply, TurnError>;

    /// 秘書のレシート画像経路（現行 `parseReceipt`）。
    async fn parse_receipt(
        &self,
        bot_id: &BotId,
        user_id: &UserId,
        image: InlineMedia,
        caption: Option<String>,
        status: StatusSink,
        delivery: Arc<dyn TurnDelivery>,
    ) -> Result<TurnReply, TurnError>;

    /// 汎用モード・ギルド経路（現行 `processGuildMessage`）。
    async fn process_guild(
        &self,
        bot_id: &BotId,
        guild_id: &GuildId,
        speaker: Speaker,
        msg: IncomingChat,
        status: StatusSink,
        delivery: Arc<dyn TurnDelivery>,
    ) -> Result<TurnReply, TurnError>;

    /// 汎用モード・owner DM 経路（現行 `processBotDmMessage`）。
    async fn process_bot_dm(
        &self,
        bot_id: &BotId,
        speaker: Speaker,
        msg: IncomingChat,
        status: StatusSink,
        delivery: Arc<dyn TurnDelivery>,
    ) -> Result<TurnReply, TurnError>;
}

// ─── BotDirectory（Bot メタデータ・アクセス判定） ──────────────────────────────

/// Bot レコード（現行 `botRepo.getBotById` の必要フィールド射影）。
#[derive(Debug, Clone)]
pub struct BotRecord {
    pub id: BotId,
    /// オーナー（現行 `bot.user_id`）。DM は owner のみ応答・暗黙メンバー。
    pub owner_id: UserId,
    pub name: String,
    /// 停止処分中（現行 `isBotSuspended`）。起動しない。
    pub suspended: bool,
    /// オーナーが手動停止（現行 `bot.stopped === 1`）。自動起動しない。
    pub stopped: bool,
    /// 推奨ペルソナ ID（共有承認後のインポート確認）。
    pub recommended_persona_id: Option<i64>,
    /// 汎用モード（ギルド常駐アシスタント）か（現行 `isGuildAssistantBot` = `!has("secretary")`）。
    pub is_guild_assistant: bool,
    /// Bot 専用 Gemini キーが有効か（現行 `getBotGenAI(botId) != null`）。汎用モードで未設定なら応答しない。
    pub has_gemini_key: bool,
}

/// Bot メタデータ・アクセス判定・トークン・プロフィール（現行 `db/botRepo`/`botAttributesRepo`/
/// `userRepo`/`botCapabilities`）。全て `user_id` をデータ分離キーに通す。
#[async_trait]
pub trait BotDirectory: Send + Sync {
    /// Bot を引く（現行 `getBotById`）。
    async fn get_bot(&self, bot_id: &BotId) -> Option<BotRecord>;

    /// 全 Bot（起動時の一括起動・現行 `listAllBots`）。
    async fn list_all_bots(&self) -> Vec<BotRecord>;

    /// ユーザーがアクセス可能な Bot（オーナー＋共有 active・現行 `listBotsForUser`）。
    async fn list_bots_for_user(&self, user_id: &UserId) -> Vec<BotId>;

    /// 復号済み Discord トークン（現行 `getDecryptedDiscordToken`）。無ければ None。
    async fn decrypt_token(&self, bot_id: &BotId) -> Option<secrecy::SecretString>;

    /// Discord プロフィール（名前・アバター・discord user id）を DB へ同期（現行 `updateBotDiscordProfile`）。
    async fn update_profile(
        &self,
        bot_id: &BotId,
        username: &str,
        avatar_url: &str,
        discord_user_id: &str,
    );

    /// Web 登録済みユーザーか（現行 `isRegisteredUser`）。秘書経路は登録ユーザーのみ応答。
    async fn is_registered_user(&self, user_id: &UserId) -> bool;

    /// 利用メンバーか（現行 `isBotMember`。owner は呼び出し側で暗黙メンバー扱い）。
    async fn is_bot_member(&self, bot_id: &BotId, guild_id: &GuildId, user_id: &UserId) -> bool;

    /// 許可ギルドか（現行 `isGuildAllowed`。未許可は応答も記録もしない）。
    async fn is_guild_allowed(&self, bot_id: &BotId, guild_id: &GuildId) -> bool;

    /// 保有ロールのいずれかが許可ロールか（現行 `isAnyRoleAllowed`）。
    async fn is_any_role_allowed(
        &self,
        bot_id: &BotId,
        guild_id: &GuildId,
        role_ids: &[String],
    ) -> bool;
}

// ─── MembershipService（ボタンフロー） ────────────────────────────────────────

/// 利用申請の可否（現行 `memberRequest` の decision）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberDecision {
    Approved,
    Rejected,
}

/// 利用申請 submit の結果（現行 `submitMemberRequest` の `{ ok, message }` + owner 宛 DM 用の id）。
#[derive(Debug, Clone, Default)]
pub struct SubmitOutcome {
    pub ok: bool,
    /// 失敗時のユーザー向けメッセージ（ok 時は空でよい）。
    pub message: String,
    /// 承認 DM の宛先オーナー（Node `sendMemberRequestDM` の ownerId）。ok 時のみ。
    pub owner_id: Option<String>,
    /// Bot 表示名（DM 本文）。ok 時のみ。
    pub bot_name: Option<String>,
    /// 申請 ID（承認/却下ボタンの custom_id）。ok 時のみ。
    pub request_id: Option<i64>,
}

/// 申請可否操作の結果（現行 `decideMemberRequestById` の `{ ok, message?, status?, botName? }`）。
#[derive(Debug, Clone, Default)]
pub struct DecisionOutcome {
    pub ok: bool,
    pub message: String,
    pub status: Option<MemberDecision>,
    pub bot_name: Option<String>,
    /// 申請者（結果 DM の宛先・Node `sendMemberDecisionDM` の applicantId）。ok 時のみ。
    pub applicant_id: Option<String>,
}

/// 共有招待レコード（現行 `getShareById`）。
#[derive(Debug, Clone)]
pub struct ShareRecord {
    pub bot_id: BotId,
    pub shared_user_id: UserId,
    /// `"pending"` 等（現行 `share.status`）。
    pub status: String,
}

/// 公開ペルソナ（現行 `getPersonaById` かつ `is_public === 1`）。
#[derive(Debug, Clone)]
pub struct PersonaRecord {
    pub id: i64,
    pub name: String,
}

/// 利用申請・共有招待・ペルソナインポート（ボタンフロー）。
#[async_trait]
pub trait MembershipService: Send + Sync {
    /// メンバー外ユーザーの利用申請（現行 `submitMemberRequest`）。
    async fn submit_member_request(
        &self,
        bot_id: &BotId,
        guild_id: &str,
        applicant: &UserId,
        applicant_label: Option<String>,
        guild_label: Option<String>,
    ) -> SubmitOutcome;

    /// オーナーによる申請の承認/却下（現行 `decideMemberRequestById`）。
    async fn decide_member_request(
        &self,
        request_id: i64,
        decision: MemberDecision,
        actor: &UserId,
    ) -> DecisionOutcome;

    /// 共有招待を引く（現行 `getShareById`）。
    async fn get_share(&self, share_id: i64) -> Option<ShareRecord>;

    /// 共有招待を承認（現行 `acceptShareInvite`）。
    async fn accept_share(&self, bot_id: &BotId, shared_user: &UserId);

    /// 共有招待を辞退・取消（現行 `revokeShare`）。
    async fn revoke_share(&self, bot_id: &BotId, shared_user: &UserId);

    /// 公開ペルソナをインポート（現行 `importPersona`）。成功なら true。
    async fn import_persona(&self, user_id: &UserId, persona_id: i64) -> bool;

    /// 公開ペルソナ（`is_public===1`）を引く（現行 `getPersonaById` フィルタ済み）。
    async fn get_public_persona(&self, persona_id: i64) -> Option<PersonaRecord>;
}

/// 利用申請ボタンフローの結果 DM 送信（Node `sendMemberRequestDM`/`sendMemberDecisionDM`）。
///
/// interaction ハンドラが DB 確定後に呼ぶ。常に共有デフォルト Bot から送る（実装側で担保）。fire-and-forget
/// （戻り値の false は握り潰す・DB 上の申請は Web 管理から拾えるため）。fake でユニットテスト可能にするため
/// 注入ポートにする（[`DiscordMessenger`] が本実装）。
#[async_trait]
pub trait MemberDmSender: Send + Sync {
    /// 申請受付を Bot オーナーへ通知（承認/却下ボタン付き）。
    async fn send_request_dm(
        &self,
        owner_id: &str,
        bot_name: &str,
        applicant_label: &str,
        guild_label: &str,
        note: Option<&str>,
        request_id: i64,
    ) -> bool;

    /// 承認/却下の結果を申請者へ通知（ボタンなし）。
    async fn send_decision_dm(&self, applicant_id: &str, bot_name: &str, approved: bool) -> bool;
}

// ─── RateLimiter ─────────────────────────────────────────────────────────────

/// レート超過の種別（現行 `RateLimitResult.exceeded`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateExceeded {
    UserMinute,
    UserDay,
    GuildDay,
}

/// レート判定（現行 `RateLimitResult`）。
#[derive(Debug, Clone)]
pub struct RateDecision {
    pub allowed: bool,
    pub exceeded: Option<RateExceeded>,
}

impl RateDecision {
    /// 許可。
    #[must_use]
    pub fn allow() -> Self {
        Self {
            allowed: true,
            exceeded: None,
        }
    }

    /// 拒否（超過種別付き）。
    #[must_use]
    pub fn deny(exceeded: RateExceeded) -> Self {
        Self {
            allowed: false,
            exceeded: Some(exceeded),
        }
    }
}

/// 汎用モードのレート制限（現行 `consumeRateLimit`）。超過時は LLM を呼ばず定型応答。
#[async_trait]
pub trait RateLimiter: Send + Sync {
    /// 1 リクエストを消費して可否を返す（現行 `consumeRateLimit`）。
    async fn consume(&self, bot_id: &BotId, guild_id: &GuildId, user_id: &UserId) -> RateDecision;
}

/// レート超過時のユーザー向け定型文（現行 `rateLimitMessage` [`src/services/botRateLimit.ts:145-152`]・文言 1:1）。純関数。
#[must_use]
pub fn rate_limit_message(exceeded: RateExceeded) -> String {
    match exceeded {
        RateExceeded::UserMinute => {
            "⏳ 利用ペースが上限に達しました。1分ほど時間をおいてから再度お試しください。"
                .to_owned()
        }
        RateExceeded::UserDay => {
            "⏳ 本日のあなたの利用回数が上限に達しました。明日また利用できます。".to_owned()
        }
        RateExceeded::GuildDay => {
            "⏳ このサーバーの本日の利用回数が上限に達しました。明日また利用できます。".to_owned()
        }
    }
}
