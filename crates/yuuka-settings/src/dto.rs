//! settings API のリクエスト DTO（Node `settingsRoutes.ts` のボディ形状）。
//!
//! `/api/settings/user` だけは Node の `key in body` 意味論（present な列のみ更新）を厳密に再現するため
//! 型付き struct でなく `serde_json::Value` を直接受ける（ルート側で present 判定する）。

use serde::Deserialize;
use serde_json::Value;

/// `POST /api/settings/profile`。
#[derive(Debug, Default, Deserialize)]
pub(crate) struct ProfileInput {
    #[serde(default)]
    pub username: Option<String>,
}

/// `POST /api/settings/password`。
#[derive(Debug, Default, Deserialize)]
pub(crate) struct PasswordInput {
    #[serde(default, rename = "currentPassword")]
    pub current_password: Option<String>,
    #[serde(default, rename = "newPassword")]
    pub new_password: Option<String>,
}

/// `POST /api/settings/delete-account`。
#[derive(Debug, Default, Deserialize)]
pub(crate) struct DeleteAccountInput {
    #[serde(default)]
    pub password: Option<String>,
}

/// `POST /api/settings/gemini`。
#[derive(Debug, Default, Deserialize)]
pub(crate) struct GeminiInput {
    #[serde(default, rename = "apiKey")]
    pub api_key: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

/// `POST /api/settings/backup`。`intervalHours`/`generations` は number・numeric string の両方を受ける
/// （Node `Number(x) || fallback`）ため `Value` で受けてルート側で JS 風強制する。
#[derive(Debug, Default, Deserialize)]
pub(crate) struct BackupInput {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default, rename = "folderId")]
    pub folder_id: Option<String>,
    #[serde(default, rename = "intervalHours")]
    pub interval_hours: Option<Value>,
    #[serde(default)]
    pub generations: Option<Value>,
}
