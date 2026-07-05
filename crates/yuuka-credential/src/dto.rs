//! credential ドメインの wire DTO（自クレートに配置し ts-rs で生成）。
//!
//! **機密フェイルクローズ（最重要）**: `credentials` テーブルの暗号化列
//! （`encrypted_password` / `iv` / `auth_tag`）と `user_id` は [`Credential`] の
//! **フィールドに存在させない**（既存 Node `CredentialIndexEntry` 相当のクリーンビュー）。
//! 生成 TS にも現れず、暗号文・鍵材料の漏洩は型的に不可能（R-13）。既存フロントは snake_case。
//!
//! DTO 名は全ドメイン共有の `generated/` 衝突を避けるため `Credential` 接頭辞を付ける。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// クライアントへ返す認証情報インデックス（クリーンビュー・snake_case）。
///
/// パスワード関連列（`encrypted_password` / `iv` / `auth_tag`）は**フィールドに存在しない**。
/// Node `listCredentials` の `SELECT service_name, username, url, updated_at` と一致。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct Credential {
    pub service_name: String,
    pub username: String,
    pub url: Option<String>,
    pub updated_at: String,
}

/// `GET /api/credentials` のペイロード（`Envelope<CredentialListData>` = `{success, credentials}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct CredentialListData {
    pub credentials: Vec<Credential>,
}

/// `POST /api/credentials/delete` の body（`serviceName`）。
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "generated/")]
pub struct DeleteCredential {
    pub service_name: String,
}

/// 削除結果（`{success}` に加えて削除した service_name を返す）。
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "generated/")]
pub struct CredentialDeletedData {
    pub deleted_service_name: String,
}
