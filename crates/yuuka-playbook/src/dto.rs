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
