//! yuuka-richcontent — リッチ返信 Embed ツール（core capability・常時有効）。
//!
//! `showRichContent`: 天気/ニュース/一覧/確認/エラー通知などを色付きカード（Discord Embed /
//! desktop APIEmbed）に整えて返信へ添付する。Node `richContentModule`（`buildRichContentEmbed`）
//! パリティ。ツールは Embed を [`ResponsePart::Embed`] として積むだけで、実描画は reply 層
//! （Discord = twilight `Embed` / desktop = APIEmbed JSON）が担う。core モジュール（selectable=false）
//! ＝秘書経路・汎用モード経路の両方で常時露出する（Node は richContentModule を無条件で push）。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use yuuka_core::tool::FunctionDeclaration;
use yuuka_core::{
    EmbedFieldPart, EmbedPart, ResponsePart, Tool, ToolContext, ToolError, ToolExposure, ToolName,
    ToolOutcome,
};

/// このクレートが公開するツール（showRichContent）。
///
/// # Errors
/// ツール名が Gemini 制約に反する場合 [`ToolError`]。
pub fn tools() -> Result<Vec<Arc<dyn Tool>>, ToolError> {
    Ok(vec![Arc::new(ShowRichContentTool {
        name: ToolName::checked("showRichContent".to_owned())?,
    })])
}

struct ShowRichContentTool {
    name: ToolName,
}

#[async_trait]
impl Tool for ShowRichContentTool {
    fn exposure(&self) -> ToolExposure {
        // core（常時有効・selectable=false）。秘書・汎用モードの両経路で露出（Node richContentModule）。
        ToolExposure {
            capability: None,
            secretary: true,
            guild_assistant: true,
            requires_guild: false,
        }
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "天気・ニュース・株価・路線情報・一覧などを、色付きカード（DiscordのEmbed）に整えて見せる。\n\
                ・一覧やまとめ、確認のお願い、エラー通知など、文章だけより見やすく伝えたい時に積極的に使う。\n\
                ・このツールはカードを送信待ちに積むだけで、あとで返信の文章と一緒にDiscordへ届く。\n\
                ・グラフ画像にした方がよい数値データ → 代わりに sendChart を使う。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "カードの見出し（例: '🌤️ 東京の今日の天気'）" },
                    "description": { "type": "string", "description": "見出しのすぐ下に出す説明文（省略可）" },
                    "color": { "type": "string", "description": "カードの色（内容に合わせて選ぶ）: default=青・ふつうの情報, success=緑・完了, warning=黄・注意, error=赤・失敗, weather=空色・天気/朝の知らせ, finance=金色・家計/支払い, task=紫・タスク/予定, info=水色, news=オレンジ, data=紫" },
                    "fields": {
                        "type": "array",
                        "description": "カードに並べる項目の配列。各項目は名前と値のペア。最大25件まで",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string", "description": "項目の見出し（ラベル）" },
                                "value": { "type": "string", "description": "フィールドの値" },
                                "inline": { "type": "boolean", "description": "横並び表示にするか（デフォルト: false）" }
                            },
                            "required": ["name", "value"]
                        }
                    },
                    "footer": { "type": "string", "description": "フッターに表示する補足テキスト（例: 'データ提供: 気象庁'）（任意）" }
                },
                "required": ["title"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        // リッチ返信が無効なら Embed を生成しない（§3.0.5・Node richContentModule handler）。
        if !ctx.rich_reply_enabled {
            return Ok(ToolOutcome::from_payload(json!({
                "success": false,
                "message": "ユーザー設定によりリッチ返信は無効です。内容はプレーンテキストで伝えてください。",
            })));
        }

        // title は必須（Node `!data.title` = 空/未指定は fail。trim しない）。
        let title = args
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if title.is_empty() {
            return Ok(ToolOutcome::from_payload(json!({
                "success": false,
                "message": "title は必須です。",
            })));
        }

        let embed = build_embed(title, &args);
        Ok(ToolOutcome {
            payload: json!({
                "success": true,
                "message": "Embedを返信に添付しました。本文では要点のみ簡潔に補足してください。",
            }),
            parts: vec![ResponsePart::Embed(embed)],
        })
    }
}

/// `showRichContent` の引数から [`EmbedPart`] を構築する（Node `buildRichContentEmbed` パリティ）。
///
/// Node は `.setTimestamp()` で現在時刻を刻むが、Rust の reply 層 Embed 記述は timestamp を
/// 持たないため付与しない（カード上に時刻が出ない僅少な見た目差・許容）。
fn build_embed(title: &str, args: &Value) -> EmbedPart {
    // description: 空でなければ 4000 文字まで（Node `if (data.description)` truthy チェック）。
    let description = args
        .get("description")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(|s| clip_chars(s, 4000));

    // color: 既知キーのみ採用・未指定/未知は default（Node `COLOR_MAP[color ?? "default"] ?? default`）。
    let color = color_for(args.get("color").and_then(Value::as_str));

    // fields: 最大 25 件・name 256 / value 1024・inline 既定 false（Node `data.fields.slice(0,25)`）。
    let fields = args
        .get("fields")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .take(25)
                .map(|f| EmbedFieldPart {
                    name: clip_chars(
                        f.get("name").and_then(Value::as_str).unwrap_or_default(),
                        256,
                    ),
                    value: clip_chars(
                        f.get("value").and_then(Value::as_str).unwrap_or_default(),
                        1024,
                    ),
                    inline: f.get("inline").and_then(Value::as_bool).unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default();

    // footer: 空でなければ 2048 文字まで（Node `if (data.footer)` truthy チェック）。
    let footer = args
        .get("footer")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(|s| clip_chars(s, 2048));

    EmbedPart {
        title: Some(clip_chars(title, 256)),
        description,
        color,
        fields,
        footer,
    }
}

/// カラー名 → `0xRRGGBB`（Node `COLOR_MAP` / `EMBED_COLORS`。未指定/未知は default）。
fn color_for(name: Option<&str>) -> u32 {
    match name.unwrap_or("default") {
        "success" => 0x57f287,
        "warning" => 0xfee75c,
        "error" => 0xed4245,
        "weather" => 0x00b0f4,
        "finance" | "expense" => 0xf1c40f, // expense = finance
        "task" | "data" => 0x9b59b6,       // data = task
        "info" => 0x5bc0eb,
        "news" => 0xf5a623,
        _ => 0x5865f2, // default（未知キー含む）
    }
}

/// 先頭 `max` 文字で切り取る（省略記号なし・Node `String.prototype.slice(0, max)` 相当）。
///
/// JS `.slice` は UTF-16 単位・Rust は Unicode スカラー単位のため非 BMP で僅差。いずれも Discord
/// の上限に対する安全側クリップで、実入力（数十〜数百文字）では到達しないため実害はない。
fn clip_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        s.chars().take(max).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(rich: bool) -> ToolContext {
        ToolContext {
            bot_id: yuuka_core::BotId::system_default(),
            user_id: yuuka_core::UserId::new("u".to_owned()),
            guild_id: None,
            capabilities: yuuka_core::CapabilitySet::default(),
            mode: yuuka_core::TurnMode::Secretary,
            rich_reply_enabled: rich,
        }
    }

    fn embed_of(out: &ToolOutcome) -> Option<&EmbedPart> {
        match out.parts.first() {
            Some(ResponsePart::Embed(e)) => Some(e),
            _ => None,
        }
    }

    #[test]
    fn color_map_matches_node() {
        assert_eq!(color_for(None), 0x5865f2);
        assert_eq!(color_for(Some("default")), 0x5865f2);
        assert_eq!(color_for(Some("weather")), 0x00b0f4);
        assert_eq!(color_for(Some("data")), color_for(Some("task")));
        assert_eq!(color_for(Some("expense")), color_for(Some("finance")));
        // 未知キーは default。
        assert_eq!(color_for(Some("mauve")), 0x5865f2);
    }

    #[test]
    fn clip_is_hard_cut_without_ellipsis() {
        assert_eq!(clip_chars("あいう", 10), "あいう");
        assert_eq!(clip_chars("あいうえお", 3), "あいう");
    }

    #[tokio::test]
    async fn disabled_rich_reply_returns_no_embed() {
        let tool = &tools().unwrap()[0];
        let out = tool
            .call(&ctx(false), json!({ "title": "x" }))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);
        assert!(out.parts.is_empty());
    }

    #[tokio::test]
    async fn missing_title_fails() {
        let tool = &tools().unwrap()[0];
        // title 空。
        let out = tool.call(&ctx(true), json!({ "title": "" })).await.unwrap();
        assert_eq!(out.payload["success"], false);
        assert_eq!(out.payload["message"], "title は必須です。");
        assert!(out.parts.is_empty());
        // title 欠落。
        let out = tool.call(&ctx(true), json!({})).await.unwrap();
        assert_eq!(out.payload["success"], false);
    }

    #[tokio::test]
    async fn builds_embed_with_fields_and_color() {
        let tool = &tools().unwrap()[0];
        assert_eq!(tool.exposure().capability, None);
        assert!(tool.exposure().secretary && tool.exposure().guild_assistant);

        let out = tool
            .call(
                &ctx(true),
                json!({
                    "title": "🌤️ 東京の天気",
                    "description": "晴れ",
                    "color": "weather",
                    "fields": [
                        { "name": "最高", "value": "30℃", "inline": true },
                        { "name": "最低", "value": "22℃" }
                    ],
                    "footer": "データ提供: 気象庁"
                }),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        assert_eq!(out.parts.len(), 1);
        let e = embed_of(&out).unwrap();
        assert_eq!(e.title.as_deref(), Some("🌤️ 東京の天気"));
        assert_eq!(e.description.as_deref(), Some("晴れ"));
        assert_eq!(e.color, 0x00b0f4);
        assert_eq!(e.fields.len(), 2);
        assert!(e.fields[0].inline);
        assert!(!e.fields[1].inline); // inline 既定 false。
        assert_eq!(e.footer.as_deref(), Some("データ提供: 気象庁"));
    }

    #[tokio::test]
    async fn caps_fields_at_25_and_empty_optional_dropped() {
        let tool = &tools().unwrap()[0];
        let fields: Vec<Value> = (0..30)
            .map(|i| json!({ "name": format!("n{i}"), "value": "v" }))
            .collect();
        let out = tool
            .call(
                &ctx(true),
                json!({ "title": "t", "description": "", "fields": fields }),
            )
            .await
            .unwrap();
        let e = embed_of(&out).unwrap();
        assert_eq!(e.fields.len(), 25); // 25 件で打ち切り。
        assert_eq!(e.description, None); // 空 description は付与しない。
        assert_eq!(e.color, 0x5865f2); // color 未指定 → default。
    }
}
