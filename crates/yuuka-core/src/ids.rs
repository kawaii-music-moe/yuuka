//! データ分離キーの newtype（§7.3・§12.2 の凍結契約2）。
//!
//! `UserId` を全リポジトリ署名に通し、生の `&str` を受け取らせないことで
//! 分離キー欠落をコンパイル時に排除する。過去のクロステナント事故（`owner_id` 欠落）を
//! 型レベルで再発不能にする（§12.2）。

use serde::{Deserialize, Serialize};

/// Yuuka の利用者 ID（`users` の主キー・データ分離の必須キー）。
///
/// 中身は不透明な文字列として扱い、形式に意味を持たせない。現時点では Discord ユーザー ID と同じ値だが、
/// Discord 連携の任意化（`docs/design/optional_discord.md` §3.2）で Discord と切り離す。Discord へ
/// 渡すときは [`DiscordUserId`] へ変換する（変換は yuuka-discord の `identity` に集める）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserId(String);

impl UserId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Discord のユーザー ID（snowflake）。
///
/// Discord API とやり取りする境界だけで使う。Yuuka の利用者を指すときは [`UserId`] を使い、
/// 両者を取り違えないよう別の型にしている（`docs/design/optional_discord.md` §3.2）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DiscordUserId(String);

impl DiscordUserId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DiscordUserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Bot 識別子。既定は `"system_default"`（共有秘書）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BotId(String);

impl BotId {
    pub const SYSTEM_DEFAULT: &'static str = "system_default";

    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// 共有秘書の既定 Bot。
    #[must_use]
    pub fn system_default() -> Self {
        Self(Self::SYSTEM_DEFAULT.to_owned())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for BotId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Discord ギルド識別子（ギルド常駐 Bot のみ）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GuildId(String);

impl GuildId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for GuildId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
