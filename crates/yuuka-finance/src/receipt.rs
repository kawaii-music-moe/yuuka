//! レシート画像解析（Gemini vision OCR）のシーム（Node `services/receiptParser.ts` `parseReceipt`）。
//!
//! `POST /api/expenses/upload-receipt` は画像を Gemini vision に渡して家計簿へ記帳する。実処理は
//! ChatEngine（tool registry + ユーザー鍵復号 + FC ループ）に依存するため**ポート越し**にし、未配線時は
//! [`NullReceiptParser`]（利用不可）へ縮退する。入力検証（画像/MIME/レート制限の 4xx/429）は
//! ルート層で常に働く。gateway/ChatEngine を配線したら実装を注入すれば live 化する。

use async_trait::async_trait;
use serde_json::Value;

/// レシート解析の失敗（レート超過 or 未配線）。
#[derive(Debug)]
pub enum ReceiptError {
    /// レート制限超過（Node `rateLimitMessage`・そのまま 429 のメッセージへ）。
    RateLimited(String),
    /// Gemini vision 未配線（縮退）。
    Unavailable,
}

/// レシート画像を解析して家計簿へ記帳するポート（Node `parseReceipt`）。
#[async_trait]
pub trait ReceiptParser: Send + Sync {
    /// 画像（base64 + MIME）を Gemini vision で解析し、記帳結果（`ProcessResult` 相当の JSON）を返す。
    ///
    /// # Errors
    /// レート超過は [`ReceiptError::RateLimited`]、未配線は [`ReceiptError::Unavailable`]。
    async fn parse_receipt(
        &self,
        bot_id: &str,
        user_id: &str,
        image_base64: &str,
        mime_type: &str,
        additional_text: Option<&str>,
    ) -> Result<Value, ReceiptError>;
}

/// Gemini vision 未配線時の縮退（常に利用不可）。
pub struct NullReceiptParser;

#[async_trait]
impl ReceiptParser for NullReceiptParser {
    async fn parse_receipt(
        &self,
        _bot_id: &str,
        _user_id: &str,
        _image_base64: &str,
        _mime_type: &str,
        _additional_text: Option<&str>,
    ) -> Result<Value, ReceiptError> {
        Err(ReceiptError::Unavailable)
    }
}
