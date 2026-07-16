//! Discord ライブ情報のシーム（`guild-options` のロール/メンバー候補・`sync-discord` の Bot ユーザー取得）。
//!
//! Node の `getGuildOptionsForBot`（Discord クライアントからギルドのロール/メンバーを取得）と
//! `customClients`/`defaultBotClient` の `user` 参照に対応する。Discord gateway 未配線時は
//! [`NullDiscordLive`]（利用不可・空一覧・ユーザー無し）へ縮退する。DB 効果（プロフィール同期の書き込み）は
//! 呼び出し側ハンドラで常に働く。gateway を配線したら実装を注入すれば live 化する。

use async_trait::async_trait;

/// Discord 上の Bot ユーザー（`sync-discord` のプロフィール同期に使う）。
#[derive(Debug, Clone)]
pub struct DiscordBotUser {
    /// Discord ユーザー ID（= application id 相当）。
    pub id: String,
    /// 表示ユーザー名。
    pub username: String,
    /// アバター URL（`displayAvatarURL()`）。
    pub avatar_url: String,
}

/// ギルドのロール/メンバー候補（プルダウン用・Node `getGuildOptionsForBot` の戻り）。
#[derive(Debug, Clone, Default)]
pub struct GuildOptions {
    pub roles: Vec<GuildEntry>,
    pub members: Vec<GuildEntry>,
    /// メンバー一覧が完全か（GuildMembers intent 無しでは false）。
    pub members_complete: bool,
    /// Bot がオンラインかつ当該ギルドに在籍しているか。
    pub available: bool,
}

/// ロール/メンバーの `{id, name}` 候補。
#[derive(Debug, Clone)]
pub struct GuildEntry {
    pub id: String,
    pub name: String,
}

/// Discord ライブ情報ポート（ギルド候補取得 + Bot ユーザー参照）。
#[async_trait]
pub trait DiscordLive: Send + Sync {
    /// 独自クライアント（`customClients.get(bot_id)?.user`）の Bot ユーザー。未起動は `None`。
    async fn custom_bot_user(&self, bot_id: &str) -> Option<DiscordBotUser>;
    /// デフォルトクライアント（`defaultBotClient.user`）の Bot ユーザー。未準備は `None`。
    async fn default_bot_user(&self) -> Option<DiscordBotUser>;
    /// ギルドのロール/メンバー候補（`getGuildOptionsForBot`）。
    async fn guild_options(&self, bot_id: &str, guild_id: &str) -> GuildOptions;
}

/// Discord gateway 未配線時の縮退実装（Bot ユーザー無し・ギルド候補は利用不可）。
pub struct NullDiscordLive;

#[async_trait]
impl DiscordLive for NullDiscordLive {
    async fn custom_bot_user(&self, _bot_id: &str) -> Option<DiscordBotUser> {
        None
    }
    async fn default_bot_user(&self) -> Option<DiscordBotUser> {
        None
    }
    async fn guild_options(&self, _bot_id: &str, _guild_id: &str) -> GuildOptions {
        GuildOptions::default()
    }
}
