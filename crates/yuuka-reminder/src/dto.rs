//! reminder ドメインの wire DTO（自クレートに配置し ts-rs で生成）。
//!
//! **機密フェイルクローズ**: 内部列 `user_id`/`bot_id` は [`Reminder`] の**フィールドに
//! 存在させない**（Node `ReminderRecord` から user_id/bot_id を落としたクリーンビュー）。
//! 生成 TS にも現れず漏洩は型的に不可能。DTO 名は generated/ 共有のため `Reminder*` 接頭辞。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// クライアントへ返すリマインド（クリーンビュー・snake_case）。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct Reminder {
    pub id: i64,
    pub message: String,
    /// 送信予定日時 `'YYYY-MM-DD HH:MM:SS'`（ローカルタイム）。
    pub trigger_at: String,
    /// 繰り返しの cron 式。単発は `None`。
    pub repeat_rule: Option<String>,
    /// `"dm" | "channel"`。
    pub target_type: String,
    pub target_id: Option<String>,
    /// `"pending" | "sent" | "cancelled"`。
    pub status: String,
    /// `"manual" | "todo" | "schedule" | "payment" | "birthday" | "webhook"`。
    pub source: String,
    pub source_id: Option<String>,
    pub created_at: String,
}

/// リマインド作成リクエスト（`POST /api/reminders/add` の body）。
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct NewReminder {
    pub message: String,
    /// 送信予定日時（`'YYYY-MM-DD HH:MM:SS'` 等）。
    pub trigger_at: String,
    #[serde(default)]
    pub repeat_rule: Option<String>,
    /// `"dm" | "channel"`。未指定は `"dm"`。
    #[serde(default)]
    pub target_type: Option<String>,
    #[serde(default)]
    pub target_id: Option<String>,
}

/// `GET /api/reminders` のペイロード（`Envelope<ReminderListData>` = `{success, reminders}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct ReminderListData {
    pub reminders: Vec<Reminder>,
}

/// 単一リマインドを返すペイロード（add/cancel。`{success, reminder}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct ReminderData {
    pub reminder: Reminder,
}
