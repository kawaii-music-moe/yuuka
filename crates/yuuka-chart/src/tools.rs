//! sendChart ツール（Node `chartFunctions.sendChart` パリティ）。
//!
//! 数値データをグラフ画像（PNG）にして返信へ添付する。画像は [`ResponsePart::InlineData`]
//! （engine `rich_parts_to_files` が `FileAttachment` へ変換）+ タイトルカード [`ResponsePart::Embed`]
//! として積む。cap=secretary・リッチ返信無効時は生成しない（§3.0.5）。

use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use serde_json::{json, Value};
use yuuka_core::tool::FunctionDeclaration;
use yuuka_core::{
    EmbedPart, ResponsePart, Tool, ToolContext, ToolError, ToolName, ToolOutcome,
};

use crate::render::{self, ChartSpec, ChartType, Series};

/// データ点の上限（Node）。
const MAX_POINTS: usize = 30;
/// タイトル/データ系パープル（§3.0.2）。
const CHART_COLOR: u32 = 0x009b_59b6;

/// このドメインが公開する Native ツール（sendChart 1 本）。
///
/// # Errors
/// ツール名が Gemini 制約に反する場合（コンパイル時定数のため通常発生しない）。
pub fn tools() -> Result<Vec<Arc<dyn Tool>>, ToolError> {
    Ok(vec![Arc::new(SendChartTool {
        name: ToolName::checked("sendChart".to_owned())?,
    })])
}

fn fail(message: impl Into<String>) -> ToolOutcome {
    ToolOutcome::from_payload(json!({ "success": false, "message": message.into() }))
}

/// JSON 配列を f64 ベクタへ（非数値は NaN・Node `Number(v)` 相当）。
fn as_f64_vec(v: Option<&Value>) -> Option<Vec<f64>> {
    v.and_then(Value::as_array).map(|arr| {
        arr.iter()
            .map(|x| x.as_f64().unwrap_or(f64::NAN))
            .collect()
    })
}

/// JSON 配列を文字列ベクタへ（Node `String(l)`）。
fn as_str_vec(v: Option<&Value>) -> Option<Vec<String>> {
    v.and_then(Value::as_array).map(|arr| {
        arr.iter()
            .map(|x| x.as_str().map_or_else(|| x.to_string(), str::to_owned))
            .collect()
    })
}

fn arg_string(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

struct SendChartTool {
    name: ToolName,
}

#[async_trait]
impl Tool for SendChartTool {
    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name.clone(),
            description: "数値データをグラフ画像にして、返信に貼り付けて見せる。\n\
                ・数字を見て分かりやすくしたい時に使う（例: カテゴリ別の支出内訳、月ごとの収支の動き、タスクの完了率、予算の消化ぐあい、気温の移り変わり、項目の比べっこ）。\n\
                ・グラフの形は type で選ぶ: 構成比は pie、完了率などは doughnut、項目比較は bar、プログレスバー風は horizontalBar、時系列の推移は line。\n\
                ・2つのデータを並べて比べたい時は second_values を渡す（例: 収入と支出、最高気温と最低気温）。\n\
                ・グラフは暗い色合いで描かれ、画像として添付される。1回の返信に貼れるのは1枚まで。"
                .to_owned(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "type": { "type": "string", "description": "グラフの形を選ぶ。pie=円グラフ（構成比）, doughnut=ドーナツ（完了率など）, bar=縦棒（項目の比較）, horizontalBar=横棒（予算消化率のプログレスバー風）, line=折れ線（時系列の推移）。" },
                    "title": { "type": "string", "description": "グラフの見出し（例: '6月のカテゴリ別支出'）。" },
                    "labels": { "type": "array", "items": { "type": "string" }, "description": "各データの名前を並べた配列（例: ['食費','日用品','娯楽'] や ['1月','2月','3月']）。" },
                    "values": { "type": "array", "items": { "type": "number" }, "description": "labels と同じ並び順・同じ件数の数値データ。" },
                    "series_label": { "type": "string", "description": "values のデータ系列につける名前（例: '支出'）。省略してよい。" },
                    "second_values": { "type": "array", "items": { "type": "number" }, "description": "並べて比べるための2本目の数値データ（省略可。例: 収入の系列）。pie と doughnut では使えない。" },
                    "second_label": { "type": "string", "description": "2本目のデータ系列につける名前（例: '収入'）。省略してよい。" }
                },
                "required": ["type", "title", "labels", "values"]
            }),
            requires_confirmation: false,
        }
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
        // リッチ返信無効ならグラフを生成しない（§3.0.5）。
        if !ctx.rich_reply_enabled {
            return Ok(fail(
                "ユーザー設定によりリッチ返信（グラフ）は無効です。数値はテキストで簡潔に伝えてください。",
            ));
        }

        let Some(kind) = args.get("type").and_then(Value::as_str).and_then(ChartType::parse) else {
            return Ok(fail(
                "グラフ種別が不正です。pie / doughnut / bar / horizontalBar / line のいずれかを指定してください。",
            ));
        };
        let title = arg_string(&args, "title").unwrap_or_default();
        let labels = as_str_vec(args.get("labels")).unwrap_or_default();
        let values = as_f64_vec(args.get("values")).unwrap_or_default();

        if labels.is_empty() || values.is_empty() {
            return Ok(fail("labels と values は必須です。"));
        }
        if labels.len() != values.len() {
            return Ok(fail("labels と values の件数が一致していません。"));
        }
        if values.iter().any(|v| !v.is_finite()) {
            return Ok(fail("values に数値でない要素が含まれています。"));
        }
        if labels.len() > MAX_POINTS {
            return Ok(fail(
                "データ点が多すぎます（最大30件）。集約してから再度呼び出してください。",
            ));
        }

        // 第2系列（pie/doughnut では不可・件数一致必須）。
        let secondary = match as_f64_vec(args.get("second_values")) {
            Some(sv) if !sv.is_empty() => {
                if matches!(kind, ChartType::Pie | ChartType::Doughnut) {
                    return Ok(fail("pie / doughnut では第2系列は使用できません。"));
                }
                if sv.len() != labels.len() {
                    return Ok(fail("second_values の件数が labels と一致していません。"));
                }
                if sv.iter().any(|v| !v.is_finite()) {
                    return Ok(fail("second_values に数値でない要素が含まれています。"));
                }
                Some(Series {
                    label: Some(arg_string(&args, "second_label").unwrap_or_else(|| "系列2".to_owned())),
                    values: sv,
                })
            }
            _ => None,
        };

        let spec = ChartSpec {
            kind,
            title: title.clone(),
            labels,
            primary: Series {
                label: arg_string(&args, "series_label"),
                values,
            },
            secondary,
        };

        let png = match render::render(&spec) {
            Ok(bytes) => bytes,
            Err(_) => return Ok(fail("グラフの生成に失敗しました。")),
        };
        let data = base64::engine::general_purpose::STANDARD.encode(&png);

        Ok(ToolOutcome {
            payload: json!({
                "success": true,
                "message": format!(
                    "グラフ「{title}」を生成し、返信に添付しました。本文では要点を簡潔に補足してください（数値の羅列は不要です）。"
                ),
            }),
            parts: vec![
                ResponsePart::InlineData {
                    mime_type: "image/png".to_owned(),
                    data,
                },
                ResponsePart::Embed(EmbedPart {
                    title: Some(format!("📊 {title}")),
                    description: None,
                    color: CHART_COLOR,
                    fields: Vec::new(),
                    footer: None,
                }),
            ],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yuuka_core::{BotId, UserId};

    fn ctx(rich: bool) -> ToolContext {
        let mut c = ToolContext::new(BotId::system_default(), UserId::new("u"));
        c.rich_reply_enabled = rich;
        c
    }

    fn tool() -> Arc<dyn Tool> {
        tools().unwrap().into_iter().next().unwrap()
    }

    #[tokio::test]
    async fn rejects_when_rich_disabled() {
        let out = tool()
            .call(&ctx(false), json!({"type": "bar", "title": "t", "labels": ["a"], "values": [1]}))
            .await
            .unwrap();
        assert_eq!(out.payload["success"], false);
    }

    #[tokio::test]
    async fn validates_inputs() {
        let t = tool();
        // 不正 type。
        assert_eq!(
            t.call(&ctx(true), json!({"type": "x", "title": "t", "labels": ["a"], "values": [1]}))
                .await
                .unwrap()
                .payload["success"],
            false
        );
        // 件数不一致。
        assert_eq!(
            t.call(&ctx(true), json!({"type": "bar", "title": "t", "labels": ["a", "b"], "values": [1]}))
                .await
                .unwrap()
                .payload["success"],
            false
        );
        // pie で第2系列 → 不可。
        assert_eq!(
            t.call(&ctx(true), json!({"type": "pie", "title": "t", "labels": ["a"], "values": [1], "second_values": [2]}))
                .await
                .unwrap()
                .payload["success"],
            false
        );
    }

    #[tokio::test]
    async fn renders_png_and_attaches() {
        let out = tool()
            .call(
                &ctx(true),
                json!({"type": "bar", "title": "テスト", "labels": ["食費", "娯楽"], "values": [1200, 800], "series_label": "支出"}),
            )
            .await
            .unwrap();
        assert_eq!(out.payload["success"], true);
        // InlineData(png) + Embed の 2 パートが積まれる。
        assert_eq!(out.parts.len(), 2);
        let has_png = out.parts.iter().any(|p| matches!(
            p,
            ResponsePart::InlineData { mime_type, data } if mime_type == "image/png" && !data.is_empty()
        ));
        assert!(has_png, "PNG が InlineData として添付される");
    }

    #[tokio::test]
    async fn renders_all_types() {
        for kind in ["pie", "doughnut", "bar", "horizontalBar", "line"] {
            let out = tool()
                .call(
                    &ctx(true),
                    json!({"type": kind, "title": "t", "labels": ["a", "b", "c"], "values": [3, 5, 2]}),
                )
                .await
                .unwrap();
            assert_eq!(out.payload["success"], true, "type={kind}");
            assert_eq!(out.parts.len(), 2, "type={kind}");
        }
    }
}
