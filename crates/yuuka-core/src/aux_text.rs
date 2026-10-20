//! ユーザー本人のキーで行う補助テキスト生成の共通ポート。

use async_trait::async_trait;

/// 補助 LLM のテキスト生成ポート（Node `generateAuxText`）。ユーザー本人の Gemini キーで生成し、
/// キー未設定・失敗時は `None` を返す（呼び出し側はフォールバック整形へ縮退する）。
#[async_trait]
pub trait AuxTextGenerator: Send + Sync {
    async fn generate_aux_text(&self, user_id: &str, prompt: &str) -> Option<String>;
}

/// 未配線時は生データへフォールバックする。
pub struct NullAuxTextGenerator;

#[async_trait]
impl AuxTextGenerator for NullAuxTextGenerator {
    async fn generate_aux_text(&self, _user_id: &str, _prompt: &str) -> Option<String> {
        None
    }
}
