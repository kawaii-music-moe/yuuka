//! Yuuka の利用者 ID（[`UserId`]）と Discord のユーザー ID（[`DiscordUserId`]）の変換。
//!
//! Discord から来た操作の主（メッセージの送信者・インタラクションの操作者）を利用者に直すときと、
//! 利用者へ DM を送る・チャンネル権限を確かめるときに Discord ID が要るときは、必ずここを通す
//! （`docs/design/optional_discord.md` §3.2・§8-1）。
//!
//! 現時点では `users` の主キーが Discord ID そのものなので、どちらも恒等変換になる。Discord 連携を
//! 任意にする段階で `users.discord_id` の参照に置き換え、未連携の利用者には `None` を返すようにする。

use twilight_model::id::marker::UserMarker;
use twilight_model::id::Id;
use yuuka_core::{DiscordUserId, UserId};

/// twilight の ID を [`DiscordUserId`] に包む。
#[must_use]
pub(crate) fn discord_user(id: Id<UserMarker>) -> DiscordUserId {
    DiscordUserId::new(id.get().to_string())
}

/// Discord ユーザーを Yuuka の利用者 ID に直す（現時点では恒等変換）。
#[must_use]
pub(crate) fn user_of(discord: &DiscordUserId) -> UserId {
    UserId::new(discord.as_str())
}

/// twilight の ID から、直接 Yuuka の利用者 ID を得る。
#[must_use]
pub(crate) fn user_from_twilight(id: Id<UserMarker>) -> UserId {
    user_of(&discord_user(id))
}

/// 利用者が連携している Discord ユーザーを返す（現時点では恒等変換で、常に `Some`）。
#[must_use]
pub(crate) fn discord_of(user: &UserId) -> Option<DiscordUserId> {
    Some(DiscordUserId::new(user.as_str()))
}

/// [`DiscordUserId`] を twilight の ID に直す。snowflake として読めなければ `None`。
#[must_use]
pub(crate) fn to_twilight(discord: &DiscordUserId) -> Option<Id<UserMarker>> {
    discord.as_str().parse::<Id<UserMarker>>().ok()
}

/// 利用者の Discord ID を twilight の ID で返す。未連携・不正な値なら `None`。
#[must_use]
pub(crate) fn twilight_of(user: &UserId) -> Option<Id<UserMarker>> {
    discord_of(user).as_ref().and_then(to_twilight)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twilight_round_trip_is_identity_for_now() {
        let id = Id::<UserMarker>::new(123_456_789_012_345_678);
        let user = user_from_twilight(id);
        assert_eq!(user.as_str(), "123456789012345678");
        assert_eq!(twilight_of(&user), Some(id));
    }

    #[test]
    fn non_snowflake_user_has_no_twilight_id() {
        assert_eq!(twilight_of(&UserId::new("")), None);
        assert_eq!(twilight_of(&UserId::new("0")), None);
        assert_eq!(
            twilight_of(&UserId::new("0190f5c2-7a3b-7c1d-8e9f-0123456789ab")),
            None
        );
    }
}
