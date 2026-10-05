//! schedule ドメインの wire DTO（自クレートに配置し ts-rs で生成）。
//!
//! **機密フェイルクローズ**: `user_id`/`bot_id`/`reminded`/`google_event_id`/
//! `google_calendar_id` 等の内部・Google 同期列は [`Schedule`] の**フィールドに存在させない**
//! （既存 Node の返却相当のクリーンビュー）。生成 TS にも現れず漏洩は型的に不可能。
//! DTO 名はドメイン接頭辞（`Schedule*`）で generated/ 共有ディレクトリ内の衝突を回避する。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// クライアントへ返す予定（クリーンビュー・snake_case）。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct Schedule {
    pub id: i64,
    pub title: String,
    pub description: Option<String>,
    pub start_at: String,
    pub end_at: Option<String>,
    pub remind_before_minutes: i64,
    pub created_at: String,
}

/// 予定作成リクエスト（`POST /api/schedules/add` の body）。
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "generated/")]
pub struct NewSchedule {
    pub title: String,
    pub start_at: String,
    #[serde(default)]
    pub end_at: Option<String>,
    #[serde(default)]
    pub remind_before_minutes: Option<i64>,
    #[serde(default)]
    pub description: Option<String>,
}

/// `GET /api/schedules` のペイロード（`Envelope<ScheduleListData>` = `{success, schedules}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct ScheduleListData {
    pub schedules: Vec<Schedule>,
}

/// 単一予定を返すペイロード（add。`{success, schedule}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct ScheduleData {
    pub schedule: Schedule,
}
