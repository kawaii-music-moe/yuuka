//! provider 中立 DTO → twilight 型の写像と、チャンネルへの分割送信（現行の返信ブロック）。
//!
//! - [`to_twilight_embeds`] / [`to_twilight_components`] / [`to_twilight_attachments`] — 純写像。
//! - [`send_channel_reply`] — `toDiscordMarkdown` → 2000 字分割 → 最終チャンクへ embeds/files/components
//!   添付、という現行 [`src/bot.ts:1218-1246`] の送信手順を移植。送信失敗は `safeReply` 同様に
//!   ログして打ち切り、ハンドラ（ひいてはプロセス）を巻き込まない。

use twilight_http::Client;
use twilight_model::channel::message::component::{ActionRow, Button, ButtonStyle};
use twilight_model::channel::message::{Component, Embed};
use twilight_model::http::attachment::Attachment;
use twilight_model::id::marker::{ChannelMarker, MessageMarker};
use twilight_model::id::Id;
use twilight_util::builder::embed::{EmbedBuilder, EmbedFieldBuilder, EmbedFooterBuilder};

use crate::ports::{self, FileAttachment, RichEmbed, TurnReply};
use crate::text::{self, DISCORD_MAX_MESSAGE_LEN};

/// [`RichEmbed`] 群 → twilight [`Embed`] 群へ写像する。
#[must_use]
pub fn to_twilight_embeds(embeds: &[RichEmbed]) -> Vec<Embed> {
    embeds.iter().map(to_twilight_embed).collect()
}

fn to_twilight_embed(e: &RichEmbed) -> Embed {
    let mut builder = EmbedBuilder::new();
    if let Some(title) = &e.title {
        builder = builder.title(title.clone());
    }
    if let Some(desc) = &e.description {
        builder = builder.description(desc.clone());
    }
    if let Some(color) = e.color {
        builder = builder.color(color);
    }
    for f in &e.fields {
        let mut fb = EmbedFieldBuilder::new(f.name.clone(), f.value.clone());
        if f.inline {
            fb = fb.inline();
        }
        builder = builder.field(fb.build());
    }
    if let Some(footer) = &e.footer {
        builder = builder.footer(EmbedFooterBuilder::new(footer.clone()).build());
    }
    builder.build()
}

/// [`ports::ActionRow`] 群 → twilight [`Component`] 群（action row）へ写像する。
#[must_use]
pub fn to_twilight_components(rows: &[ports::ActionRow]) -> Vec<Component> {
    rows.iter()
        .map(|row| {
            Component::ActionRow(ActionRow {
                id: None,
                components: row.buttons.iter().map(to_twilight_button).collect(),
            })
        })
        .collect()
}

fn to_twilight_button(b: &ports::Button) -> Component {
    // twilight 0.17 の Button は Default を持たないため全フィールドを明示する。
    Component::Button(Button {
        id: None,
        custom_id: Some(b.custom_id.clone()),
        disabled: false,
        emoji: None,
        label: Some(b.label.clone()),
        style: to_twilight_button_style(b.style),
        url: None,
        sku_id: None,
    })
}

fn to_twilight_button_style(style: ports::ButtonStyle) -> ButtonStyle {
    match style {
        ports::ButtonStyle::Primary => ButtonStyle::Primary,
        ports::ButtonStyle::Secondary => ButtonStyle::Secondary,
        ports::ButtonStyle::Success => ButtonStyle::Success,
        ports::ButtonStyle::Danger => ButtonStyle::Danger,
    }
}

/// [`FileAttachment`] 群 → twilight [`Attachment`] 群へ写像する（id は per-message の連番）。
#[must_use]
pub fn to_twilight_attachments(files: &[FileAttachment]) -> Vec<Attachment> {
    files
        .iter()
        .enumerate()
        .map(|(i, f)| Attachment::from_bytes(f.name.clone(), f.bytes.clone(), i as u64))
        .collect()
}

/// 返信をチャンネルへ分割送信する（現行の返信ブロック 1:1）。成功可否を返す。
///
/// `to_discord_markdown` を適用 → 2000 字超は改行境界で分割 → 各チャンクを送信し、**最終チャンクにのみ**
/// embeds/files/components を添付する。`reply_to` を渡すと返信参照を付ける。送信失敗は握り潰さず
/// ログして以降のチャンクを打ち切る（`safeReply` 相当・プロセスは巻き込まない）。
///
/// 戻り値は「全チャンク送信成功なら `true`」。notifier の同チャンネル→DM フォールバック
/// （現行 `deliverFinal` の `if (!ok && !isDM)`）判定に使う。
pub async fn send_channel_reply(
    http: &Client,
    channel_id: Id<ChannelMarker>,
    reply_to: Option<Id<MessageMarker>>,
    reply: &TurnReply,
) -> bool {
    let text = text::to_discord_markdown(&reply.text);
    let embeds = to_twilight_embeds(&reply.embeds);
    let attachments = to_twilight_attachments(&reply.files);
    let components = to_twilight_components(&reply.components);

    // 現行: 2000 字超なら分割、そうでなければ全文を 1 チャンク（空文字でも添付付きで 1 通送る）。
    let chunks: Vec<String> = if text.chars().count() > DISCORD_MAX_MESSAGE_LEN {
        text::split_message(&text, DISCORD_MAX_MESSAGE_LEN)
    } else {
        vec![text]
    };

    let last = chunks.len().saturating_sub(1);
    for (i, chunk) in chunks.iter().enumerate() {
        let is_last = i == last;
        let mut req = http.create_message(channel_id).content(chunk);
        if let Some(rid) = reply_to {
            req = req.reply(rid);
        }
        if is_last {
            if !embeds.is_empty() {
                req = req.embeds(&embeds);
            }
            if !attachments.is_empty() {
                req = req.attachments(&attachments);
            }
            if !components.is_empty() {
                req = req.components(&components);
            }
        }
        if let Err(e) = req.await {
            // 送信不能（トークン喪失・権限不足・シャットダウン中のレース等）。以降のチャンクは諦める。
            tracing::warn!(error = %e, channel_id = %channel_id, "返信の送信に失敗（打ち切り）");
            return false;
        }
    }
    true
}

/// プレーンなテキスト 1 通を送る（利用案内・エラー定型文・DM など。任意で components 付き）。
/// 送信成功なら `true`（DM 送信の成否判定・現行 sender の boolean 戻りに使う）。
pub async fn send_channel_text(
    http: &Client,
    channel_id: Id<ChannelMarker>,
    reply_to: Option<Id<MessageMarker>>,
    content: &str,
    components: &[Component],
) -> bool {
    let mut req = http.create_message(channel_id).content(content);
    if let Some(rid) = reply_to {
        req = req.reply(rid);
    }
    if !components.is_empty() {
        req = req.components(components);
    }
    if let Err(e) = req.await {
        tracing::warn!(error = %e, channel_id = %channel_id, "テキスト送信に失敗");
        return false;
    }
    true
}
