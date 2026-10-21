//! provider 中立 DTO → twilight 型の写像と、チャンネルへの分割送信（現行の返信ブロック）。
//!
//! - [`to_twilight_embeds`] / [`to_twilight_components`] / [`to_twilight_attachments`] — 純写像
//!   （[`to_twilight_embeds`] のみ Discord の Embed 数上限で切り捨てる・#37）。
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
use crate::text::{self, DISCORD_MAX_EMBEDS, DISCORD_MAX_MESSAGE_LEN};

/// [`RichEmbed`] 群 → twilight [`Embed`] 群へ写像する。
///
/// Discord の 1 メッセージあたり Embed 上限（[`DISCORD_MAX_EMBEDS`]）を超える分は切り捨てる
/// （#37: `showRichContent` 複数呼び出し + `embed_recover` 復元分の合計が超過しうる。超過したまま
/// 送ると `create_message` が検証エラーで失敗し、本文ごと届かなくなる）。
#[must_use]
pub fn to_twilight_embeds(embeds: &[RichEmbed]) -> Vec<Embed> {
    embeds
        .iter()
        .take(DISCORD_MAX_EMBEDS)
        .map(to_twilight_embed)
        .collect()
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
    let chunks = reply_chunks(&reply.text);
    let extras = ReplyExtras::new(reply);
    send_chunks(http, channel_id, reply_to, &chunks, &extras).await
}

/// 返信本文を Discord 向けに整形してチャンクへ分ける。現行: 2000 字超なら改行境界で分割、そうでなければ
/// 全文を 1 チャンク（空文字でも添付付きで 1 通送る）。
fn reply_chunks(text: &str) -> Vec<String> {
    let text = text::to_discord_markdown(text);
    if text.chars().count() > DISCORD_MAX_MESSAGE_LEN {
        text::split_message(&text, DISCORD_MAX_MESSAGE_LEN)
    } else {
        vec![text]
    }
}

/// 最終チャンクにだけ付ける embeds/files/components（twilight 型へ写像済み）。
#[derive(Default)]
struct ReplyExtras {
    embeds: Vec<Embed>,
    attachments: Vec<Attachment>,
    components: Vec<Component>,
}

impl ReplyExtras {
    fn new(reply: &TurnReply) -> Self {
        Self {
            embeds: to_twilight_embeds(&reply.embeds),
            attachments: to_twilight_attachments(&reply.files),
            components: to_twilight_components(&reply.components),
        }
    }

    fn is_empty(&self) -> bool {
        self.embeds.is_empty() && self.attachments.is_empty() && self.components.is_empty()
    }
}

/// チャンクを順に新しいメッセージとして送り、最終チャンクにだけ `extras` を付ける。全部送れたら `true`。
async fn send_chunks(
    http: &Client,
    channel_id: Id<ChannelMarker>,
    reply_to: Option<Id<MessageMarker>>,
    chunks: &[String],
    extras: &ReplyExtras,
) -> bool {
    let last = chunks.len().saturating_sub(1);
    for (i, chunk) in chunks.iter().enumerate() {
        let is_last = i == last;
        let mut req = http.create_message(channel_id).content(chunk);
        if let Some(rid) = reply_to {
            req = req.reply(rid);
        }
        if is_last {
            if !extras.embeds.is_empty() {
                req = req.embeds(&extras.embeds);
            }
            if !extras.attachments.is_empty() {
                req = req.attachments(&extras.attachments);
            }
            if !extras.components.is_empty() {
                req = req.components(&extras.components);
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

/// 逐次表示する本文（生成途中の本文全体）を、表示用に整えて 1 通に収まる先頭部分だけ返す。
///
/// 最終結果で Embed へ復元して本文から消す ```json ブロック（`embed_recover`）は、書きかけの JSON が
/// 見えないようにその手前で切る。2000 字を超えた分は最終結果で続きのメッセージとして送る。
pub(crate) fn stream_preview(raw: &str) -> String {
    let visible = ["```json", "```\n{"]
        .iter()
        .filter_map(|fence| raw.find(fence))
        .min()
        .map_or(raw, |end| &raw[..end]);
    reply_chunks(visible.trim_end())
        .into_iter()
        .next()
        .unwrap_or_default()
}

/// 逐次表示の最初の 1 通を返信として送り、そのメッセージ ID を返す（送れなければ `None`）。
pub(crate) async fn send_stream_start(
    http: &Client,
    channel_id: Id<ChannelMarker>,
    reply_to: Id<MessageMarker>,
    content: &str,
) -> Option<Id<MessageMarker>> {
    let sent = http
        .create_message(channel_id)
        .content(content)
        .reply(reply_to)
        .await;
    match sent {
        Ok(resp) => match resp.model().await {
            Ok(message) => Some(message.id),
            Err(e) => {
                tracing::warn!(error = %e, channel_id = %channel_id, "逐次表示の返信を読み取れません");
                None
            }
        },
        Err(e) => {
            tracing::warn!(error = %e, channel_id = %channel_id, "逐次表示の返信を送れません");
            None
        }
    }
}

/// 逐次表示中の返信の本文を差し替える（失敗はログのみ・最終結果の確定でもう一度直す）。
pub(crate) async fn edit_stream_text(
    http: &Client,
    channel_id: Id<ChannelMarker>,
    message_id: Id<MessageMarker>,
    content: &str,
) {
    if let Err(e) = http
        .update_message(channel_id, message_id)
        .content(Some(content))
        .await
    {
        tracing::warn!(error = %e, channel_id = %channel_id, "逐次表示の返信を編集できません");
    }
}

/// 逐次表示した返信（`message_id`・いまの表示 `shown`）を最終結果で確定する。全部届けられたら `true`。
///
/// 1 通目は編集で差し替え、2000 字を超えた残りは続けて送る（embeds/files/components は最終チャンクへ）。
/// 表示中の本文と同じで付けるものも無ければ編集しない（「編集済み」を増やさない）。編集できなければ
/// 逐次表示の返信を消し、[`send_channel_reply`] で送り直す。
pub(crate) async fn complete_stream(
    http: &Client,
    channel_id: Id<ChannelMarker>,
    reply_to: Id<MessageMarker>,
    message_id: Id<MessageMarker>,
    shown: &str,
    reply: &TurnReply,
) -> bool {
    let chunks = reply_chunks(&reply.text);
    let extras = ReplyExtras::new(reply);
    let (first, rest) = chunks
        .split_first()
        .map_or(("", &[][..]), |(first, rest)| (first.as_str(), rest));
    // 続きを送るなら添付は最後のチャンクへ回し、1 通目は本文だけにする。
    let none = ReplyExtras::default();
    let first_extras = if rest.is_empty() { &extras } else { &none };
    if first != shown || !first_extras.is_empty() {
        let mut req = http
            .update_message(channel_id, message_id)
            .content(Some(first));
        if !first_extras.embeds.is_empty() {
            req = req.embeds(Some(&first_extras.embeds));
        }
        if !first_extras.attachments.is_empty() {
            req = req.attachments(&first_extras.attachments);
        }
        if !first_extras.components.is_empty() {
            req = req.components(Some(&first_extras.components));
        }
        if let Err(e) = req.await {
            tracing::warn!(error = %e, channel_id = %channel_id, "逐次表示の返信を確定できません（送り直します）");
            delete_stream(http, channel_id, message_id).await;
            return send_channel_reply(http, channel_id, Some(reply_to), reply).await;
        }
    }
    send_chunks(http, channel_id, Some(reply_to), rest, &extras).await
}

/// 逐次表示した返信を消す（最終結果が黙殺・送り直しのとき）。失敗はログのみ。
pub(crate) async fn delete_stream(
    http: &Client,
    channel_id: Id<ChannelMarker>,
    message_id: Id<MessageMarker>,
) {
    if let Err(e) = http.delete_message(channel_id, message_id).await {
        tracing::warn!(error = %e, channel_id = %channel_id, "逐次表示の返信を消せません");
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn embed(title: &str) -> RichEmbed {
        RichEmbed {
            title: Some(title.to_owned()),
            ..RichEmbed::default()
        }
    }

    /// #37: `showRichContent` 複数回 + `embed_recover` 復元分の合計が Discord 上限（10）を
    /// 超えても、超過分を切り捨てて検証エラーを起こさないこと。
    #[test]
    fn to_twilight_embeds_caps_at_discord_max() {
        let embeds: Vec<RichEmbed> = (0..15).map(|i| embed(&format!("e{i}"))).collect();
        let out = to_twilight_embeds(&embeds);
        assert_eq!(out.len(), DISCORD_MAX_EMBEDS);
        assert_eq!(out[0].title.as_deref(), Some("e0"));
        assert_eq!(out[9].title.as_deref(), Some("e9"));
    }

    /// 逐次表示は Embed へ復元する JSON ブロックの手前で切り、1 通に収まる先頭部分だけを見せる。
    #[test]
    fn stream_preview_hides_embed_json_and_fits_one_message() {
        assert_eq!(
            stream_preview("天気です。\n```json\n{\"title\":"),
            "天気です。"
        );
        assert_eq!(stream_preview("天気です。\n```\n{\"ti"), "天気です。");
        assert_eq!(
            stream_preview("```rust\nfn main() {}\n```"),
            "```rust\nfn main() {}\n```"
        );
        let long = "あ".repeat(DISCORD_MAX_MESSAGE_LEN + 10);
        assert!(stream_preview(&long).chars().count() <= DISCORD_MAX_MESSAGE_LEN);
        assert_eq!(stream_preview("   "), "");
    }

    #[test]
    fn to_twilight_embeds_keeps_all_under_cap() {
        let embeds: Vec<RichEmbed> = (0..3).map(|i| embed(&format!("e{i}"))).collect();
        let out = to_twilight_embeds(&embeds);
        assert_eq!(out.len(), 3);
    }
}
