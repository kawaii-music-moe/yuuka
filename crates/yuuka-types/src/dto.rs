//! 代表 wire DTO（§10.2・§12.2 の凍結契約3）。
//!
//! **機密フェイルクローズは構造で保証**する: `*_encrypted` / `*_iv` / `*_tag` /
//! `password_hash` / `salt` 等の機密列を DTO の**フィールドに存在させない**。
//! → ts-rs 生成 TS にも現れず、成功レスポンス経由の漏洩が型的に不可能になる（R-13）。
//!
//! Phase 0 では代表 DTO のみ凍結し、生成器（xtask gen-types）と単一真実源の仕組みを確立する。
//! 各ドメインの網羅 DTO は Phase 1 で該当クレートが `yuuka-types` に追加する。

use serde::Serialize;
use ts_rs::TS;

/// ユーザーロール。生成 TS では `"user" | "admin"` のユニオンになる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export_to = "generated/")]
pub enum Role {
    User,
    Admin,
}

/// セッションユーザー（`GET /api/me` の `user`）。既存フロントは camelCase。
///
/// 機密（トークン・鍵）は含めない。表示に必要な最小フィールドのみ。
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "generated/")]
pub struct SessionUser {
    pub discord_id: String,
    pub username: String,
    pub role: Role,
}

/// Bot の要約ビュー（`botViewSchema` 相当の抜粋）。既存フロントは snake_case。
///
/// **機密トークン/APIキーの暗号文は含めない**（構造的フェイルクローズ）。
/// 露出してよい存在フラグ（`has_token` 等）のみで機密の有無を表す。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct BotSummary {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub has_token: bool,
    pub has_gemini_key: bool,
    pub running: bool,
    pub connected: bool,
}
