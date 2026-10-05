//! admin API の DTO（リクエスト本体 + 管理画面向け参照ビュー）。
//!
//! 参照ビュー（`Serialize`）は Node `adminRoutes.ts` が返す JSON の **キー名・大小文字**を
//! そのまま再現する（`hasCustomToken`/`isRunning` は camelCase・DB 列由来は snake_case）。
//! 秘密値（トークン暗号文・パスワードハッシュ等）は一切フィールドに存在させない。

use serde::{Deserialize, Serialize};

// ─── リクエスト本体 ───────────────────────────────────────────────────────────

/// `POST /api/admin/default-bot/token`。
#[derive(Debug, Default, Deserialize)]
pub(crate) struct TokenInput {
    #[serde(default)]
    pub token: Option<String>,
}

/// `POST /api/admin/system-settings`。
#[derive(Debug, Default, Deserialize)]
pub(crate) struct SystemSettingsInput {
    #[serde(default, rename = "privacyPolicyUrl")]
    pub privacy_policy_url: Option<String>,
    #[serde(default, rename = "termsUrl")]
    pub terms_url: Option<String>,
}

/// `POST /api/admin/users/role`。
#[derive(Debug, Default, Deserialize)]
pub(crate) struct RoleInput {
    #[serde(default, rename = "targetUserId")]
    pub target_user_id: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
}

/// `POST /api/admin/users/delete`。
#[derive(Debug, Default, Deserialize)]
pub(crate) struct TargetUserInput {
    #[serde(default, rename = "targetUserId")]
    pub target_user_id: Option<String>,
}

/// `POST /api/admin/bots/suspend`・`/unsuspend`。
#[derive(Debug, Default, Deserialize)]
pub(crate) struct BotIdInput {
    #[serde(default, rename = "botId")]
    pub bot_id: Option<String>,
}

/// `POST /api/admin/invite-codes`。
#[derive(Debug, Default, Deserialize)]
pub(crate) struct CodeInput {
    #[serde(default)]
    pub code: Option<String>,
}

/// `GET /api/admin/audit-logs` のクエリ（Node は parseInt 寛容パース・action は前方一致）。
#[derive(Debug, Default, Deserialize)]
pub(crate) struct AuditQuery {
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub limit: Option<String>,
    #[serde(default)]
    pub offset: Option<String>,
}

// ─── 参照ビュー ───────────────────────────────────────────────────────────────

/// `GET /api/admin/stats` の `stats` オブジェクト。
#[derive(Debug, Serialize)]
pub struct AdminStats {
    #[serde(rename = "totalUsers")]
    pub total_users: i64,
    #[serde(rename = "totalBots")]
    pub total_bots: i64,
    #[serde(rename = "suspendedBots")]
    pub suspended_bots: i64,
    #[serde(rename = "totalInviteCodes")]
    pub total_invite_codes: i64,
    #[serde(rename = "usedInviteCodes")]
    pub used_invite_codes: i64,
    #[serde(rename = "availableInviteCodes")]
    pub available_invite_codes: i64,
}

/// `GET /api/admin/users` の 1 行（Node `listAllUsers`＝秘密値を含まない列のみ）。
#[derive(Debug, Serialize)]
pub struct AdminUserView {
    pub discord_id: String,
    pub username: String,
    pub role: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

/// `GET /api/admin/bots` の 1 行（オーナー名・稼働中・カスタムトークン有無を付与）。
#[derive(Debug, Serialize)]
pub struct AdminBotView {
    pub id: String,
    pub name: String,
    pub user_id: String,
    pub owner_username: String,
    pub discord_username: Option<String>,
    pub discord_avatar_url: Option<String>,
    pub suspended: i64,
    #[serde(rename = "hasCustomToken")]
    pub has_custom_token: bool,
    #[serde(rename = "isRunning")]
    pub is_running: bool,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

/// `GET /api/admin/invite-codes` の 1 行（Node `listInviteCodes`＝`SELECT *`）。
#[derive(Debug, Serialize)]
pub struct InviteCodeView {
    pub code: String,
    pub created_by: Option<String>,
    pub used_by: Option<String>,
    pub used_at: Option<String>,
    pub revoked_at: Option<String>,
    pub created_at: Option<String>,
}

/// `GET /api/admin/audit-logs` の 1 行（Node `listAuditLogs`＝`SELECT *`）。
#[derive(Debug, Serialize)]
pub struct AuditLogView {
    pub id: i64,
    pub user_id: String,
    pub action: String,
    pub target: Option<String>,
    pub detail: Option<String>,
    pub created_at: Option<String>,
}
