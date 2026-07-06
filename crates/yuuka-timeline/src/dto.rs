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
/// 参照スコープ = プレーン記録（memo/task_done/location 等）。`amount` は Node が body から読む
/// （`Number(b.amount)`）ため残置する。**`type=expense` の expenses 二重登録・`amount` 必須検証・
/// `category` キー処理は deferred（Batch 6 / M-8）**。`type=task_done` の todos.complete 連携も deferred。
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
    #[serde(default)]
    pub location: Option<String>,
}

/// `GET /api/timeline/day` の記録ペイロード（`Envelope<TimelineDayData>` = `{success, records}`）。
///
/// Node は `{success, blocks, records}` を返すが、`day_plan_blocks` は deferred のため
/// T1 コアでは `records` のみ返す（blocks は day_plan_blocks 実装時に追加）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct TimelineDayData {
    pub records: Vec<TimelineRecord>,
}

/// 単一記録を返すペイロード（add。`{success, record}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct TimelineRecordData {
    pub record: TimelineRecord,
}

/// 削除結果（`{success, deletedId}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "generated/")]
pub struct TimelineDeletedData {
    pub deleted_id: i64,
}
