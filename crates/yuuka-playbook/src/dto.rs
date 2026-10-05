//! playbook ドメインの wire DTO（自クレートに配置し ts-rs で生成）。
//!
//! **機密フェイルクローズ**: `user_id`/`bot_id`/`id`/`created_at`/`updated_at` 等の内部列は
//! [`Playbook`] の**フィールドに存在させない**（既存 Node の `findPlaybooks` が返す
//! クリーンビュー name/title/keywords/description/steps 相当）。生成 TS にも現れず漏洩は
//! 型的に不可能（R-13）。既存フロントは snake_case。
//!
//! DTO 名は generated/*.ts 共有ディレクトリ衝突回避のため `Playbook` 接頭辞を付ける。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// クライアントへ返す playbook（クリーンビュー・snake_case）。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct Playbook {
    /// 正規化済みマクロ名（英数・`-`・`_` のみ／小文字。スコープ内で一意）。
    pub name: String,
    pub title: String,
    /// パース済みキーワード（DB は JSON 文字列 `keywords` で保持）。
    pub keywords: Vec<String>,
    pub description: String,
    /// Markdown 手順 または Function Call 列の記述。
    pub steps: String,
}

/// playbook 保存リクエスト（`POST /api/playbooks/save` の body・upsert）。
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct NewPlaybook {
    pub name: String,
    pub title: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub description: String,
    pub steps: String,
}

/// `GET /api/playbooks` のペイロード（`Envelope<PlaybookListData>` = `{success, playbooks}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct PlaybookListData {
    pub playbooks: Vec<Playbook>,
}

/// 単一 playbook を返すペイロード（save。`{success, playbook}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct PlaybookData {
    pub playbook: Playbook,
}

// ─── 定期実行スケジュール / 実行履歴 ─────────────────────────────────────────

/// クライアントへ返す定期実行スケジュール（snake_case）。
///
/// **機密フェイルクローズ**: Node `PlaybookSchedule` interface / `SELECT *` は `user_id` を
/// 含むが、他ドメイン（reminder 等）と同じ構造的フェイルクローズに合わせ、内部の所有者列
/// `user_id` は**フィールドに存在させない**（生成 TS にも現れず漏洩は型的に不可能）。
/// `bot_id` は Node interface 上も業務データ（結果通知先の Bot）としてクライアントに露出する
/// ため保持する。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct PlaybookSchedule {
    pub id: i64,
    /// 実行結果の通知に使う Bot インスタンス。
    pub bot_id: String,
    pub playbook_name: String,
    pub cron_expression: String,
    pub description: String,
    /// `enabled` は DB では 0/1。ここでは Node `rowToSchedule`（`row.enabled === 1`）に
    /// 合わせ bool に変換して返す。
    pub enabled: bool,
    pub last_run_at: Option<String>,
    pub next_run_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// クライアントへ返す定期実行の実行履歴（snake_case）。
///
/// **機密フェイルクローズ**: `user_id` はフィールドに存在させない（上記 [`PlaybookSchedule`]
/// と同方針）。`bot_id` は Node の `SELECT *` が返す業務列として保持する。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct PlaybookRun {
    pub id: i64,
    pub schedule_id: i64,
    pub bot_id: String,
    pub playbook_name: String,
    /// `"running" | "success" | "failed"`。
    pub status: String,
    pub output: String,
    pub started_at: String,
    pub finished_at: Option<String>,
}

/// スケジュール保存リクエスト（`POST /api/playbooks/schedules/save` の body・upsert）。
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct NewSchedule {
    pub playbook_name: String,
    pub cron_expression: String,
    #[serde(default)]
    pub description: String,
    /// 未指定は Node の `enabled !== false`（＝ `true`）に合わせて有効。
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// スケジュール有効／無効切替リクエスト（`POST /api/playbooks/schedules/toggle`）。
///
/// `id` は Node の `id == null` チェックを保つため `Option`（未指定は route が
/// 「idは必須です。」を 400 で返す）。
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct ToggleScheduleInput {
    #[serde(default)]
    pub id: Option<i64>,
    /// Node `!!enabled`（未指定・falsy は無効化）。
    #[serde(default)]
    pub enabled: bool,
}

/// スケジュール削除リクエスト（`POST /api/playbooks/schedules/delete`）。
///
/// `id` は Node の `id == null` チェックを保つため `Option`（未指定は route が
/// 「idは必須です。」を 400 で返す）。
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct ScheduleIdInput {
    #[serde(default)]
    pub id: Option<i64>,
}

/// `GET /api/playbooks/schedules` のペイロード（`{success, schedules}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct ScheduleListData {
    pub schedules: Vec<PlaybookSchedule>,
}

/// 単一スケジュールを返すペイロード（save 成功時。`{success, message, schedule}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct ScheduleData {
    pub schedule: PlaybookSchedule,
}

/// `GET /api/playbooks/runs` のペイロード（`{success, runs}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct RunListData {
    pub runs: Vec<PlaybookRun>,
}
