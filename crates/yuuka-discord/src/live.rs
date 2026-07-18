//! Discord ライブ照会ヘルパ（web 層 `guild-options` の REST 実装）。
//!
//! Node `getGuildOptionsForBot` の置換。Node は gateway キャッシュ（`guild.roles.fetch()` +
//! `guild.members.cache`）から引いたが、twilight は共有キャッシュを持たないため REST で引く。
//! メンバー一覧の REST は GuildMembers 特権 intent（開発者ポータルのトグル）が無いと拒否される
//! ため best-effort（失敗は空一覧・Node もキャッシュ分のみで `members_complete:false` だった）。

use twilight_http::Client;
use twilight_model::id::marker::GuildMarker;
use twilight_model::id::Id;

/// ロール/メンバーの `{id, name}` 候補（provider 中立・web 層の `GuildEntry` へ写像される）。
#[derive(Debug, Clone)]
pub struct GuildLiveEntry {
    pub id: String,
    pub name: String,
}

/// ギルドのロール/メンバー候補（Node `getGuildOptionsForBot` の戻り）。
#[derive(Debug, Clone, Default)]
pub struct GuildLiveOptions {
    pub roles: Vec<GuildLiveEntry>,
    pub members: Vec<GuildLiveEntry>,
    /// メンバー一覧が完全か（Node 同様、常に false＝部分一覧の可能性を UI へ伝える）。
    pub members_complete: bool,
    /// Bot が当該ギルドに参加しているか（guild fetch 成功＝在籍）。
    pub available: bool,
}

/// 指定ギルドのロール/メンバー候補を REST で引く（Node `getGuildOptionsForBot`）。
///
/// ギルド不到達（未参加・不正 ID）は `available:false`・空一覧。ロール/メンバーの取得失敗は
/// それぞれ空一覧に縮退する（Node と同じ best-effort）。
pub async fn fetch_guild_options(client: &Client, guild_id: &str) -> GuildLiveOptions {
    let Ok(gid) = guild_id.parse::<Id<GuildMarker>>() else {
        return GuildLiveOptions::default();
    };
    // 在籍確認（Node `guilds.fetch(guildId)`）。未参加/権限無しは available:false。
    if client.guild(gid).await.is_err() {
        return GuildLiveOptions::default();
    }

    // ロール: @everyone（id==guild id）と Bot 管理ロール（managed）を除外し名前順。
    let mut roles: Vec<GuildLiveEntry> = match client.roles(gid).await {
        Ok(resp) => match resp.model().await {
            Ok(list) => list
                .into_iter()
                .filter(|r| r.id.get() != gid.get() && !r.managed)
                .map(|r| GuildLiveEntry {
                    id: r.id.get().to_string(),
                    name: r.name,
                })
                .collect(),
            Err(e) => {
                tracing::warn!(guild_id, error = %e, "ロール一覧のデコードに失敗");
                Vec::new()
            }
        },
        Err(e) => {
            tracing::warn!(guild_id, error = %e, "ロール一覧の取得に失敗");
            Vec::new()
        }
    };
    roles.sort_by(|a, b| a.name.cmp(&b.name));

    // メンバー: GuildMembers 特権 intent 無しでは REST が拒否するため best-effort（失敗は空）。
    // Bot ユーザーは除外・表示名は nick → global_name → username（Node `displayName` 相当）。
    let mut members: Vec<GuildLiveEntry> = match client.guild_members(gid).limit(1000).await {
        Ok(resp) => match resp.model().await {
            Ok(list) => list
                .into_iter()
                .filter(|m| !m.user.bot)
                .map(|m| {
                    let name = m
                        .nick
                        .clone()
                        .or_else(|| m.user.global_name.clone())
                        .unwrap_or_else(|| m.user.name.clone());
                    GuildLiveEntry {
                        id: m.user.id.get().to_string(),
                        name,
                    }
                })
                .collect(),
            Err(e) => {
                tracing::debug!(guild_id, error = %e, "メンバー一覧のデコードに失敗（空一覧に縮退）");
                Vec::new()
            }
        },
        Err(e) => {
            tracing::debug!(guild_id, error = %e, "メンバー一覧の取得に失敗（特権 intent 無し等・空一覧に縮退）");
            Vec::new()
        }
    };
    members.sort_by(|a, b| a.name.cmp(&b.name));

    GuildLiveOptions {
        roles,
        members,
        members_complete: false,
        available: true,
    }
}
