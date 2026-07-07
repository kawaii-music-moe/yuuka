//! persona ドメインの wire DTO（自クレートに配置し ts-rs で生成）。
//!
//! **機密フェイルクローズ**: `owner_id`（所有者の Discord ID）は [`Persona`] の
//! **フィールドに存在させない**（既存 Node の `PersonaRecord` から所有者 ID を落とした
//! クリーンビュー）。生成 TS にも現れず漏洩は型的に不可能。
//! DTO 名は全ドメイン共有の `generated/` で衝突しないようドメイン接頭辞を付ける。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// ペルソナは §4.1.2 で 20,000 文字まで（アプリ層で検証）。
pub const PERSONA_MAX_LENGTH: usize = 20_000;

/// クライアントへ返すペルソナ（クリーンビュー・snake_case・`owner_id` を含めない）。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct Persona {
    pub id: i64,
    pub name: String,
    /// システムプロンプト本文（自分のペルソナは全文を返す）。
    pub prompt: String,
    /// マーケットプレイス公開フラグ（DB は 0/1、wire は bool）。
    pub is_public: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// ペルソナ作成／更新リクエスト（`POST /api/personas/save` の body）。
///
/// `id` が指定され実在すれば更新、無ければ新規作成（Node `personas/save` 準拠）。
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export_to = "generated/")]
pub struct SavePersona {
    #[serde(default)]
    pub id: Option<i64>,
    pub name: String,
    #[serde(default)]
    pub prompt: String,
}

/// `GET /api/personas` のペイロード（`Envelope<PersonaListData>`）。
///
/// 適用中ペルソナ ID（`bot_active_personas`）はコア CRUD 外のため deferred。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct PersonaListData {
    pub personas: Vec<Persona>,
    pub max_length: i64,
}

/// 単一ペルソナを返すペイロード（save。`{success, persona}`）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct PersonaData {
    pub persona: Persona,
}
