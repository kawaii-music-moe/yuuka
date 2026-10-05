//! timeline ドメインの wire DTO（自クレートに配置し ts-rs で生成）。
//!
//! **機密フェイルクローズ**: `user_id`/`bot_id` 等の内部列は [`TimelineRecord`] の
//! **フィールドに存在させない**（既存 Node の生 row から内部列を落としたクリーンビュー）。
//! 生成 TS にも現れず漏洩は型的に不可能（R-13）。既存フロントは snake_case。
//!
//! DTO 名は全ドメイン共有の `generated/` 衝突回避のため `Timeline` 接頭辞を付ける。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// クライアントへ返すタイムライン記録（クリーンビュー・snake_case）。
///
/// `timeline_records` 表の全公開列を写す。内部スコープ列（user_id/bot_id）は持たない。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct TimelineRecord {
    pub id: i64,
    /// `'YYYY-MM-DD'`。
    pub date: String,
    /// datetime 文字列（`recorded_at`）。
    pub recorded_at: String,
    /// `"memo" | "expense" | "task_done" | "media" | "location"`。
    pub r#type: String,
    pub title: Option<String>,
    pub content: Option<String>,
    /// `todos.id` への参照（cross-domain・passthrough）。
    pub todo_id: Option<i64>,
    /// `expenses.id` への参照（cross-domain・passthrough）。
    pub expense_id: Option<i64>,
    pub amount: Option<f64>,
    pub expense_category: Option<String>,
    /// メディアファイル名のみ（`data/media/` 以下）。
    pub media_path: Option<String>,
    /// `"photo" | "video"`。
    pub media_type: Option<String>,
    pub location: Option<String>,
    pub created_at: String,
}

/// タイムライン記録の作成リクエスト（`POST /api/timeline/record` の body）。
///
/// **wire 契約**: Node `timelineRoutes.ts` の record ハンドラは body を **camelCase**
/// （`recordedAt`/`todoId`）で読む。`rename_all` 欠落時は無音で `None` に落ちるため camelCase を強制。
/// **出力ビュー [`TimelineRecord`] は snake_case のまま**。
///
/// **内部列の非露出（M-8）**: `expense_id`/`expense_category`/`media_path`/`media_type` は
/// **どのエンドポイントでも body から読まれないワイヤ非キー**（Node の record ハンドラは読まず、
/// expense 二重登録・メディアアップロード経路がサーバ内部で設定する。特に expense フローの body
/// キーは `category` であって `expenseCategory` ではない）。入力 DTO から除去し、直接指定を
/// 素通し INSERT しない。
///
/// 参照スコープ = プレーン記録（memo/location 等）に加え、cross-domain 副作用:
/// **`type=expense`** は `expenses` へ二次登録し `expense_id` を連結（route/tool とも `amount` 必須・
/// `category` は Node `b.category ?? "その他"`）、**`type=task_done`** は `todo_id` 指定時に
/// 紐付き todos を `done` に更新する（Node `completeTodo`）。内部列 `expense_id`/`expense_category`/
/// `media_path`/`media_type` は依然どのエンドポイントでも body から読まない（M-8・サーバ内部が設定）。
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "generated/")]
pub struct NewTimelineRecord {
    pub date: String,
    pub r#type: String,
    #[serde(default)]
    pub recorded_at: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub todo_id: Option<i64>,
    #[serde(default)]
    pub amount: Option<f64>,
    /// `type=expense` の家計簿カテゴリ（Node route は `b.category`・未指定は `"その他"`）。
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
}

/// クライアントへ返す計画ブロック（`day_plan_blocks` 表・snake_case クリーンビュー）。
///
/// 内部スコープ列（user_id/bot_id）は持たない（[`TimelineRecord`] と同じ機密フェイルクローズ）。
/// Node `DayPlanBlock` の生 row から内部列を落とした形。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct DayPlanBlock {
    pub id: i64,
    /// `'YYYY-MM-DD'`。
    pub date: String,
    /// `'HH:MM'`（`null`=時間未定）。
    pub start_time: Option<String>,
    /// `'HH:MM'`（`null`=終了未定）。
    pub end_time: Option<String>,
    /// `"task" | "transit" | "event" | "free"`。
    pub r#type: String,
    pub title: String,
    pub description: Option<String>,
    /// `todos.id` への参照（cross-domain・passthrough）。
    pub todo_id: Option<i64>,
    pub transit_from: Option<String>,
    pub transit_to: Option<String>,
    pub transit_line: Option<String>,
    pub position: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// 計画ブロックの作成リクエスト（`POST /api/timeline/plan` の body）。
///
/// **wire 契約**: Node `timelineRoutes.ts` の plan ハンドラは body を **camelCase**
/// （`startTime`/`endTime`/`todoId`/`transitFrom`/`transitTo`/`transitLine`）で読む。
/// `date`/`type`/`title` は必須（ハンドラで 400 検証）。他は任意で未指定は NULL（`position` は 0）。
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "generated/")]
pub struct NewDayPlanBlock {
    pub date: String,
    pub r#type: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub start_time: Option<String>,
    #[serde(default)]
    pub end_time: Option<String>,
    #[serde(default)]
    pub todo_id: Option<i64>,
    #[serde(default)]
    pub transit_from: Option<String>,
    #[serde(default)]
    pub transit_to: Option<String>,
    #[serde(default)]
    pub transit_line: Option<String>,
    #[serde(default)]
    pub position: Option<i64>,
}

/// 計画ブロックの部分更新リクエスト（`POST /api/timeline/plan/update` の body）。
///
/// **Node の per-field 存在意味論を厳密に写す**（`updateDayPlanBlock`）:
/// - `title`/`type`: **string のときだけ**更新（Node は route で `typeof === "string"` ガード）。
/// - `description`: **string のときだけ**更新し、**空文字は NULL に畳む**（Node `description || null`）。
/// - `start_time`/`end_time`/`todo_id`/`transit_from`/`transit_to`/`transit_line`:
///   **キーが存在すれば**更新（`null` 明示で NULL 化）。二重 `Option` でキー非存在（外側 `None`）と
///   `null` 明示（`Some(None)`）を区別する（Node の `"key" in b` 意味論）。
/// - `position`: **route から渡されない**ため本 DTO に含めない（Node parity・update では不変）。
///
/// `id` は body から読むが検証はハンドラで行う（`Number(b.id)` 相当）。
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "generated/")]
pub struct UpdatePlanBlock {
    pub id: i64,
    /// string のときのみ更新（`typeof === "string"` ガード）。
    #[serde(default)]
    pub title: Option<String>,
    /// string のときのみ更新・空文字は NULL（`description || null`）。
    #[serde(default)]
    pub description: Option<String>,
    /// string のときのみ更新（`typeof === "string"` ガード）。
    #[serde(default)]
    pub r#type: Option<String>,
    /// キー存在で更新・`null` 可（`"startTime" in b`）。
    #[serde(default, deserialize_with = "deserialize_present")]
    #[ts(optional)]
    pub start_time: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_present")]
    #[ts(optional)]
    pub end_time: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_present")]
    #[ts(optional)]
    pub todo_id: Option<Option<i64>>,
    #[serde(default, deserialize_with = "deserialize_present")]
    #[ts(optional)]
    pub transit_from: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_present")]
    #[ts(optional)]
    pub transit_to: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_present")]
    #[ts(optional)]
    pub transit_line: Option<Option<String>>,
}

/// `Option<Option<T>>` を **キー存在=`Some(...)`／非存在=`None`** に読む（serde 二重 Option）。
///
/// `#[serde(default)]` により非存在フィールドは外側 `None` に落ちる。存在すれば（`null` 含め）
/// この関数が呼ばれ `Some(inner)` を返す（`null` → `Some(None)`）。Node の `"key" in b` 意味論。
fn deserialize_present<'de, D, T>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(de).map(Some)
}

/// `GET /api/timeline/day` のペイロード（`Envelope<TimelineDayData>` = `{success, blocks, records}`）。
///
/// Node と同じく `blocks`（計画ブロック）と `records`（汎用記録）の両方を返す。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct TimelineDayData {
    pub blocks: Vec<DayPlanBlock>,
    pub records: Vec<TimelineRecord>,
}

/// 単一計画ブロックを返すペイロード（plan 作成/更新・`{success, block}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct DayPlanBlockData {
    pub block: DayPlanBlock,
}

/// 単一記録を返すペイロード（add。`{success, record}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct TimelineRecordData {
    pub record: TimelineRecord,
}

/// メディアアップロードのリクエスト（`POST /api/timeline/media` の body・base64 JSON）。
///
/// **wire 契約**: Node `timelineRoutes.ts` の media ハンドラは body を **camelCase**（`mimeType`/
/// `recordedAt`）で読む。`date`/`base64`/`mimeType` は必須（ハンドラで空文字含め 400 検証）。
/// サーバが `save_media_file` でファイル保存後、`type=media`・`media_path`/`media_type` を内部設定する
/// （クライアントは `media_path`/`media_type` を直接指定できない・M-8）。
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "generated/")]
pub struct NewTimelineMedia {
    pub date: String,
    /// メディア本体の base64（データ URL 前置なしの生 base64）。
    pub base64: String,
    /// `image/*` | `video/*`。許可 MIME 以外は 400。
    pub mime_type: String,
    #[serde(default)]
    pub recorded_at: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
}
