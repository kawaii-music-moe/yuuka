//! Yuuka の利用者 ID（[`UserId`]）と Discord のユーザー ID（[`DiscordUserId`]）の変換。
//!
//! Discord から来た操作の主（メッセージの送信者・インタラクションの操作者）を利用者に直すときと、
//! 利用者へ DM を送る・チャンネル権限を確かめるときに Discord ID が要るときは、必ずここを通す
//! （`docs/design/optional_discord.md` §3.2）。
//!
//! 引き当ては [`BotDirectory::user_for_discord`] / [`BotDirectory::discord_for_user`] に任せる。
//! 本番の実装（orchestrator の `DbBotDirectory`）は `users.discord_id` を引く。

use twilight_model::id::marker::UserMarker;
use twilight_model::id::Id;
use yuuka_core::{DiscordUserId, UserId};

use crate::ports::BotDirectory;

/// twilight の ID を [`DiscordUserId`] に包む。
#[must_use]
pub(crate) fn discord_user(id: Id<UserMarker>) -> DiscordUserId {
    DiscordUserId::new(id.get().to_string())
}

/// [`DiscordUserId`] を twilight の ID に直す。snowflake として読めなければ `None`。
#[must_use]
pub(crate) fn to_twilight(discord: &DiscordUserId) -> Option<Id<UserMarker>> {
    discord.as_str().parse::<Id<UserMarker>>().ok()
}

/// Discord ユーザーを Yuuka の利用者 ID に直す。
pub(crate) async fn user_from_twilight(directory: &dyn BotDirectory, id: Id<UserMarker>) -> UserId {
    directory.user_for_discord(&discord_user(id)).await
}

/// 利用者が連携している Discord ユーザーを twilight の ID で返す。未連携・不正な値なら `None`。
pub(crate) async fn twilight_of(
    directory: &dyn BotDirectory,
    user: &UserId,
) -> Option<Id<UserMarker>> {
    directory
        .discord_for_user(user)
        .await
        .as_ref()
        .and_then(to_twilight)
}

/// 既定の引き当て（[`BotDirectory`] の既定実装）：利用者 ID と Discord ID を同じ値とみなす。
#[must_use]
pub(crate) fn identity_user(discord: &DiscordUserId) -> UserId {
    UserId::new(discord.as_str())
}

/// 既定の引き当て（[`BotDirectory`] の既定実装）：利用者 ID と Discord ID を同じ値とみなす。
#[must_use]
pub(crate) fn identity_discord(user: &UserId) -> Option<DiscordUserId> {
    Some(DiscordUserId::new(user.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snowflakes_round_trip_and_others_have_no_twilight_id() {
        let id = Id::<UserMarker>::new(123_456_789_012_345_678);
        let discord = discord_user(id);
        assert_eq!(discord.as_str(), "123456789012345678");
        assert_eq!(to_twilight(&discord), Some(id));
        for raw in ["", "0", "0190f5c2-7a3b-7c1d-8e9f-0123456789ab"] {
            assert_eq!(to_twilight(&DiscordUserId::new(raw)), None, "{raw}");
        }
        assert_eq!(identity_user(&discord).as_str(), "123456789012345678");
        assert_eq!(
            identity_discord(&UserId::new("42")).map(|d| d.as_str().to_owned()),
            Some("42".to_owned())
        );
    }
}
