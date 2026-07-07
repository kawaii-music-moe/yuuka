//! personal（連絡先）ドメインの wire DTO（自クレートに配置し ts-rs で生成）。
//!
//! **機密フェイルクローズ**: `user_id`/`bot_id`/`birthday_reminded_year` 等の内部列は
//! [`Contact`] の**フィールドに存在させない**（既存 Node の `toContactView` 相当のクリーン
//! ビュー）。生成 TS にも現れず漏洩は型的に不可能（R-13）。既存フロントは snake_case。
//!
//! DTO 名はドメイン接頭辞 `Contact*` を付け、共有 `generated/` 内の衝突を回避する。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// クライアントへ返す連絡先（クリーンビュー・snake_case）。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct Contact {
    pub id: i64,
    pub name: String,
    /// `'YYYY-MM-DD'` または `'--MM-DD'`（年不明）。
    pub birthday: Option<String>,
    pub relationship: Option<String>,
    pub contact_info: Option<String>,
    pub notes: Option<String>,
    /// パース済みタグ（DB は JSON 文字列 `tags` で保持）。
    pub tags: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// 連絡先の作成／更新リクエスト（`POST /api/contacts/save` の body）。
///
/// `id` があれば更新、無ければ新規作成（Node `personalRoutes` の `contacts/save` に一致）。
///
/// **wire 契約**: 既存フロント／Node は `contactInfo` を **camelCase** で送る（Node `personalRoutes`
/// の `body.contactInfo`）。`rename_all` 欠落時は無音で `None` に落ち、**update は全列上書きのため
/// 既存 `contact_info` を NULL で消去**する（H-1）。入力 DTO に camelCase を強制して防ぐ。
/// **出力ビュー [`Contact`] は snake_case のまま**。
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "generated/")]
pub struct NewContact {
    #[serde(default)]
    pub id: Option<i64>,
    pub name: String,
    #[serde(default)]
    pub birthday: Option<String>,
    #[serde(default)]
    pub relationship: Option<String>,
    #[serde(default)]
    pub contact_info: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// `GET /api/contacts` のペイロード（`Envelope<ContactListData>` = `{success, contacts}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct ContactListData {
    pub contacts: Vec<Contact>,
}

/// 単一連絡先を返すペイロード（save 新規。`{success, contact}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct ContactData {
    pub contact: Contact,
}
