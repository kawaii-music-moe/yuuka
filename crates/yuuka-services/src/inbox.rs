//! Web の受信箱（`web_notifications`・docs/design/optional_discord.md §6.1）。
//!
//! 通知先を Web にしている利用者への通知を、Discord の代わりにここへ保存する。Embed は本文の
//! テキストへ畳み込んで保存する（受信箱は文字だけを表示する）。読み出しと既読は yuuka-settings の
//! `/api/notifications` が受け持つ。

use rusqlite::params;
use yuuka_core::EmbedPart;
use yuuka_db::map_sqlite;
use yuuka_web::Db;

use crate::notifier::Notification;

/// 受信箱へ 1 件保存する。本文が空（Embed も無い）なら保存せず `false`。DB 失敗も `false`
/// （呼び出し側の再試行に任せる）。
pub(crate) async fn store(db: &Db, notification: &Notification) -> bool {
    let content = render(notification);
    if content.trim().is_empty() {
        return false;
    }
    let user_id = notification.user_id.as_str().to_owned();
    let bot_id = notification.bot_id.as_str().to_owned();
    let result = db
        .writer
        .execute(move |conn| {
            conn.execute(
                "INSERT INTO web_notifications (user_id, bot_id, content) VALUES (?1, ?2, ?3)",
                params![user_id, bot_id, content],
            )
            .map_err(map_sqlite)
        })
        .await;
    match result {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(user_id = %notification.user_id, error = %e, "Web 通知を保存できない");
            false
        }
    }
}

/// 本文と Embed（タイトル・説明・フィールド・フッター）を 1 つのテキストにする。
fn render(notification: &Notification) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !notification.content.trim().is_empty() {
        parts.push(notification.content.trim().to_owned());
    }
    parts.extend(notification.embeds.iter().map(render_embed));
    parts.retain(|p| !p.is_empty());
    parts.join("\n\n")
}

fn render_embed(embed: &EmbedPart) -> String {
    let mut lines: Vec<String> = Vec::new();
    if let Some(title) = embed.title.as_deref().filter(|t| !t.trim().is_empty()) {
        lines.push(format!("【{}】", title.trim()));
    }
    if let Some(description) = embed
        .description
        .as_deref()
        .filter(|d| !d.trim().is_empty())
    {
        lines.push(description.trim().to_owned());
    }
    for field in &embed.fields {
        lines.push(format!("{}: {}", field.name, field.value));
    }
    if let Some(footer) = embed.footer.as_deref().filter(|f| !f.trim().is_empty()) {
        lines.push(footer.trim().to_owned());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use yuuka_core::{BotId, EmbedFieldPart, UserId};

    #[test]
    fn renders_text_and_embeds_into_one_body() {
        let mut n = Notification::text(UserId::new("u"), BotId::new("b"), "  本文  ");
        n.embeds.push(EmbedPart {
            title: Some("予定".to_owned()),
            description: Some("会議があります".to_owned()),
            color: 0,
            fields: vec![EmbedFieldPart {
                name: "時刻".to_owned(),
                value: "10:00".to_owned(),
                inline: false,
            }],
            footer: None,
        });
        assert_eq!(render(&n), "本文\n\n【予定】\n会議があります\n時刻: 10:00");
        let empty = Notification::text(UserId::new("u"), BotId::new("b"), "   ");
        assert_eq!(render(&empty), "");
    }
}
