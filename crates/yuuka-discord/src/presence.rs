//! プレゼンス演出（現行 `setBotStatus` [`src/bot.ts:151-188`]）。
//!
//! [`BotStatus`] を twilight の [`UpdatePresence`] ゲートウェイコマンドへ写像する。Shard へは
//! `shard.command(&presence)` で送る（tenant ループ側）。
//!
//! 現行は `type: Custom` の `state` フィールドに「考え中...」等を載せる（Custom アクティビティは
//! `name` ではなく `state` が表示される）。twilight の `MinimalActivity` は `state` を持たない
//! （`From` が `state: None` を固定する）ため、`Activity` を経由して `state` を明示設定する。
//!
//! idle は現行 `activities: []`（クリア）だが `UpdatePresence::new` は空 activities を
//! `MissingActivity` で拒否するため、`state: None` の Custom アクティビティ 1 つで表す
//! （表示テキストが無い＝実質クリアと同じ見え方）。

use twilight_model::gateway::payload::outgoing::UpdatePresence;
use twilight_model::gateway::presence::{Activity, ActivityType, MinimalActivity, Status};
use yuuka_core::DiscordError;

use crate::ports::BotStatus;

/// [`BotStatus`] → [`UpdatePresence`]（現行 `setBotStatus` の 3 状態）。
///
/// # Errors
/// アクティビティ構築に失敗した場合（実際上は起きない）[`DiscordError::Transport`]。
pub fn build_presence(status: BotStatus) -> Result<UpdatePresence, DiscordError> {
    // state = Custom アクティビティに表示されるテキスト（None は「表示なし」＝idle クリア相当）。
    let (state, discord_status) = match status {
        BotStatus::Thinking => (Some("考え中..."), Status::DoNotDisturb),
        BotStatus::Writing => (Some("書き込み中..."), Status::Online),
        BotStatus::Idle => (None, Status::Online),
    };
    // MinimalActivity は state を持てないので Activity へ変換してから state を差し込む
    // （現行の `{ name: "custom", type: Custom, state }` と一致）。
    let mut activity: Activity = MinimalActivity {
        kind: ActivityType::Custom,
        name: "custom".to_owned(),
        url: None,
    }
    .into();
    activity.state = state.map(str::to_owned);

    UpdatePresence::new(vec![activity], false, None, discord_status)
        .map_err(|e| DiscordError::Transport(format!("presence: {e}")))
}
