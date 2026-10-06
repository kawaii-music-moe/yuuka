//! レシート画像から家計簿フォームの下書きを読み取る（登録はしない）。
//!
//! 管理画面の `upload-receipt` は秘書ターン（ツール呼び出し）でそのまま記帳するが、PWA ではユーザーが
//! 内容を直してから自分で登録したい。そこで Gemini に画像を渡し、`fillExpenseForm` 関数の呼び出しを
//! 強制（`toolConfig` mode=ANY）して、フォームの各欄を構造化された引数として受け取る。関数は実行せず、
//! 引数を検証して [`ReceiptDraft`] にするだけなので、DB には何も書かない。

use serde::Serialize;
use serde_json::{json, Value};
use yuuka_core::GeminiError;
use yuuka_gemini::{Content, FunctionDeclaration, GenerateBackend, Part, Role, ToolConfig};

/// 家計簿のカテゴリ（管理画面・PWA の選択肢と同じ）。
pub const EXPENSE_CATEGORIES: [&str; 9] = [
    "食費",
    "日用品",
    "交通費",
    "光熱費",
    "通信費",
    "医療費",
    "娯楽",
    "衣服",
    "その他",
];

const FUNCTION_NAME: &str = "fillExpenseForm";

/// 読み取った家計簿フォームの下書き。読み取れなかった欄は `None`（フォーム側で入力してもらう）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiptDraft {
    /// `YYYY-MM-DD`。
    pub date: Option<String>,
    /// `expense` / `income`。
    pub kind: String,
    /// [`EXPENSE_CATEGORIES`] のいずれか。
    pub category: String,
    pub description: String,
    /// 合計金額（円・正の整数）。
    pub amount: Option<i64>,
}

/// 読み取りの失敗。
#[derive(Debug)]
pub enum ReceiptExtractError {
    /// ユーザーの Gemini キーが未設定。
    NoGeminiKey,
    /// キーの復号ができない等、サーバー側の構成で読み取りを実行できない。
    Unavailable,
    /// Gemini 呼び出しの失敗（レート制限・上流障害など）。
    Llm(GeminiError),
    /// 応答に下書きが含まれていない（レシートとして読めなかった）。
    Unreadable,
}

fn declaration() -> FunctionDeclaration {
    FunctionDeclaration {
        name: FUNCTION_NAME.to_owned(),
        description: "レシート画像から読み取った内容で家計簿の入力フォームを埋める。".to_owned(),
        parameters: None,
        parameters_json_schema: Some(json!({
            "type": "object",
            "properties": {
                "date": { "type": "string", "description": "購入日。形式: YYYY-MM-DD。読み取れなければ空文字" },
                "kind": { "type": "string", "enum": ["expense", "income"], "description": "支出なら expense、返金・収入なら income" },
                "category": { "type": "string", "enum": EXPENSE_CATEGORIES, "description": "最も当てはまるカテゴリ" },
                "description": { "type": "string", "description": "内容。店名と主な品目を短く（例: サミット: 卵, 牛乳）" },
                "amount": { "type": "integer", "description": "支払った合計金額（税込・円）。読み取れなければ 0" }
            },
            "required": ["date", "kind", "category", "description", "amount"]
        })),
    }
}

fn instruction(today: &str) -> String {
    format!(
        "このレシート画像を読み取り、{FUNCTION_NAME} を 1 回だけ呼び出して家計簿の入力欄を埋めてください。\
         今日の日付は {today} です。年が省略されている日付は今日に近い年として解釈してください。\
         金額は合計（税込）を使い、小計やお釣り・お預かり金額と取り違えないでください。"
    )
}

/// 関数呼び出しの引数を検証して下書きにする。範囲外・不正な値はその欄を空（既定値）にする。
fn parse_draft(args: &Value) -> ReceiptDraft {
    let text = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_owned()
    };
    let date =
        Some(text("date")).filter(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok());
    let kind = if text("kind") == "income" {
        "income"
    } else {
        "expense"
    };
    let category = text("category");
    let category = if EXPENSE_CATEGORIES.contains(&category.as_str()) {
        category
    } else {
        "その他".to_owned()
    };
    // 数値でも "1,280" のような文字列でも受ける。
    let amount = match args.get("amount") {
        Some(Value::Number(n)) => n.as_f64().map(|f| f.round() as i64),
        Some(Value::String(s)) => s
            .replace([',', '，', '円', '¥', '￥'], "")
            .trim()
            .parse::<f64>()
            .ok()
            .map(|f| f.round() as i64),
        _ => None,
    }
    .filter(|n| *n > 0);
    ReceiptDraft {
        date,
        kind: kind.to_owned(),
        category,
        description: text("description"),
        amount,
    }
}

/// レシート画像（base64 + MIME）を読み取り、フォームの下書きを返す。`today` は `YYYY-MM-DD`。
///
/// # Errors
/// Gemini 呼び出しの失敗は [`ReceiptExtractError::Llm`]、関数呼び出しが返らなければ
/// [`ReceiptExtractError::Unreadable`]。
pub async fn extract(
    backend: &dyn GenerateBackend,
    image_base64: &str,
    mime_type: &str,
    today: &str,
) -> Result<ReceiptDraft, ReceiptExtractError> {
    let contents = [Content {
        role: Role::User,
        parts: vec![
            Part::inline_data(mime_type, image_base64),
            Part::text(instruction(today)),
        ],
    }];
    let response = backend
        .generate(
            None,
            &[declaration()],
            &contents,
            Some(ToolConfig::force_any(Some(vec![FUNCTION_NAME.to_owned()]))),
        )
        .await
        .map_err(ReceiptExtractError::Llm)?;
    response
        .candidates
        .first()
        .map(yuuka_gemini::Candidate::function_calls)
        .unwrap_or_default()
        .into_iter()
        .find(|call| call.name == FUNCTION_NAME)
        .map(|call| parse_draft(&call.args))
        .ok_or(ReceiptExtractError::Unreadable)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::sync::Mutex;

    use async_trait::async_trait;
    use yuuka_gemini::GenerateContentResponse;

    use super::*;

    struct FakeBackend {
        response: Value,
        seen_tool_config: Mutex<Option<ToolConfig>>,
    }

    #[async_trait]
    impl GenerateBackend for FakeBackend {
        async fn generate(
            &self,
            _system_instruction: Option<&str>,
            declarations: &[FunctionDeclaration],
            contents: &[Content],
            tool_config: Option<ToolConfig>,
        ) -> Result<GenerateContentResponse, GeminiError> {
            assert_eq!(declarations[0].name, FUNCTION_NAME);
            assert!(contents[0].parts[0].inline_data.is_some(), "画像を渡す");
            *self.seen_tool_config.lock().unwrap() = tool_config;
            Ok(serde_json::from_value(self.response.clone()).unwrap())
        }
    }

    fn call_response(args: Value) -> Value {
        json!({ "candidates": [{ "content": { "role": "model", "parts": [
            { "functionCall": { "name": FUNCTION_NAME, "args": args } }
        ] } }] })
    }

    #[tokio::test]
    async fn returns_the_forced_function_call_as_a_draft() {
        let backend = FakeBackend {
            response: call_response(json!({
                "date": "2026-09-11", "kind": "expense", "category": "食費",
                "description": "サミット: 卵", "amount": 600
            })),
            seen_tool_config: Mutex::new(None),
        };
        let draft = extract(&backend, "aGVsbG8=", "image/jpeg", "2026-10-06")
            .await
            .unwrap();
        assert_eq!(
            draft,
            ReceiptDraft {
                date: Some("2026-09-11".to_owned()),
                kind: "expense".to_owned(),
                category: "食費".to_owned(),
                description: "サミット: 卵".to_owned(),
                amount: Some(600),
            }
        );
        // 関数呼び出しを強制している。
        let config =
            serde_json::to_value(backend.seen_tool_config.lock().unwrap().clone()).unwrap();
        assert_eq!(config["functionCallingConfig"]["mode"], "ANY");
    }

    #[tokio::test]
    async fn text_only_response_is_unreadable() {
        let backend = FakeBackend {
            response: json!({ "candidates": [{ "content": { "role": "model", "parts": [{ "text": "読めません" }] } }] }),
            seen_tool_config: Mutex::new(None),
        };
        let result = extract(&backend, "aGVsbG8=", "image/jpeg", "2026-10-06").await;
        assert!(matches!(result, Err(ReceiptExtractError::Unreadable)));
    }

    #[test]
    fn invalid_fields_fall_back_to_empty_or_defaults() {
        let draft = parse_draft(&json!({
            "date": "2026-02-30", "kind": "refund", "category": "外食",
            "description": "  コンビニ  ", "amount": "1,280円"
        }));
        assert_eq!(draft.date, None);
        assert_eq!(draft.kind, "expense");
        assert_eq!(draft.category, "その他");
        assert_eq!(draft.description, "コンビニ");
        assert_eq!(draft.amount, Some(1280));
        assert_eq!(parse_draft(&json!({ "amount": 0 })).amount, None);
        assert_eq!(parse_draft(&json!({ "kind": "income" })).kind, "income");
    }
}
