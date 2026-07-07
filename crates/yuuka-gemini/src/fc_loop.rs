//! Function Calling ループ（§8.2.4）— 現行 `runFunctionCallingLoop`
//! [`src/gemini.ts:501-752`] の型付き 1:1 移植。
//!
//! 往復・並行呼び出し（`id` 相関）・`maxIterations`・完了ハルシネーション是正（mode=ANY）を保持。
//! バックエンドは [`GenerateBackend`] trait 越しなのでテストは canned レスポンスで駆動できる。
//! provider は `yuuka-core` の凍結 [`ToolProvider`] トレイト（Native/MCP/WASM 透過）。

use std::sync::Arc;

use serde_json::json;
use yuuka_core::tool::FunctionDeclaration as CoreFnDecl;
use yuuka_core::{GeminiError, ResponsePart, ToolContext, ToolName, ToolProvider};

use crate::client::GenerateBackend;
use crate::wire::{Content, FunctionDeclaration, FunctionResponse, Part, Role, ToolConfig};

/// 未実行の完了報告を検知した際に注入する是正プロンプト（[`src/gemini.ts:491-495`] 直移植）。
pub const COMPLETION_CORRECTION_PROMPT: &str = "【システム検証】あなたは今回のやり取りで一度もツール（関数）を呼び出していません。\
そのため、上記で報告した操作は実際には一切実行されていません。\
本当にその操作を行うのであれば、今すぐ対応する関数を呼び出してください。\
操作する必要がない、あるいは実行できない場合は、完了したかのように装わず、その旨を正直に伝えてください。";

/// ループ中のステータス（現行 `onStatusChange` 相当）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Thinking,
    Writing,
}

/// ステータス通知コールバック（省略可）。
pub type StatusCb = Arc<dyn Fn(Status) + Send + Sync>;

/// ループ実行オプション。
pub struct LoopOptions {
    /// 最大反復回数（現行 `maxIterations=10`）。
    pub max_iterations: usize,
    /// 完了是正の最大回数（現行 `maxCorrectionAttempts=2`）。
    pub max_corrections: usize,
    /// 完了是正時に mode=ANY で許可するツール名（プラン候補に限定）。空/None なら全許可。
    pub allowed_tool_names: Option<Vec<String>>,
    /// ステータス通知（thinking/writing）。
    pub on_status: Option<StatusCb>,
}

impl Default for LoopOptions {
    fn default() -> Self {
        Self {
            max_iterations: 10,
            max_corrections: 2,
            allowed_tool_names: None,
            on_status: None,
        }
    }
}

/// ループ結果。
#[derive(Debug, Clone, Default)]
pub struct LoopResult {
    /// 最終テキスト応答（空なら呼び出し側がフォールバック定型文を使う）。
    pub text: String,
    /// このターンで実際に dispatch したツール名（順序保持・browser 判定等に使う）。
    pub tool_calls: Vec<String>,
    /// ツール実行が積み上げたリッチ返信パート（embeds/images 相当・現行 ctx.embeds/files の置換）。
    pub rich_parts: Vec<ResponsePart>,
    /// `maxIterations` に到達して打ち切ったか（現行 `browserToolFailed=true` 相当の異常終了）。
    pub hit_max_iterations: bool,
}

/// 操作の「完了」を主張するテキストか（完了ハルシネーション検知・[`src/gemini.ts:480-488`]）。
///
/// 現行の日本語正規表現 2 本を、依存追加（`regex`）せずに手動走査で移植する:
/// - `(登録|追加|…)(し(ました|ておきました|ておきます|ますね?))`
/// - `(やって|して)おき(ました|ます(ね)?)`
#[must_use]
pub fn claims_action_completed(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    const VERBS: &[&str] = &[
        "登録", "追加", "削除", "設定", "記録", "保存", "作成", "更新", "消込", "予約", "同期",
        "変更", "オン", "オフ", "有効化", "無効化",
    ];
    // し + {ました, ておきました, ておきます, ます, ますね}
    const SHI_FORMS: &[&str] = &["しました", "しておきました", "しておきます", "しますね", "します"];
    for verb in VERBS {
        for form in SHI_FORMS {
            if text.contains(&format!("{verb}{form}")) {
                return true;
            }
        }
    }
    // (やって|して)おき(ました|ますね|ます)
    for prefix in ["やって", "して"] {
        for form in ["ました", "ますね", "ます"] {
            if text.contains(&format!("{prefix}おき{form}")) {
                return true;
            }
        }
    }
    false
}

/// core の provider 中立 `FunctionDeclaration`（IR）を Gemini wire 宣言へ変換する。
///
/// フル JSON Schema を `parametersJsonSchema` に載せ、`$schema` キーだけ最小サニタイズする
/// （深いサニタイズは provider 側の責務・§8.4）。
fn to_wire_declaration(d: &CoreFnDecl) -> FunctionDeclaration {
    FunctionDeclaration {
        name: d.name.as_str().to_owned(),
        description: d.description.clone(),
        parameters: None,
        parameters_json_schema: Some(sanitize_schema(&d.parameters_json_schema)),
    }
}

/// JSON Schema から `$schema` キーを再帰的に除去する（Gemini が拒否するため）。
fn sanitize_schema(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, val) in map {
                if k == "$schema" {
                    continue;
                }
                out.insert(k.clone(), sanitize_schema(val));
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(sanitize_schema).collect())
        }
        other => other.clone(),
    }
}

fn notify(opts: &LoopOptions, status: Status) {
    if let Some(cb) = &opts.on_status {
        cb(status);
    }
}

/// Function Calling ループ本体。`contents` は往復のたびに追記される（呼び出し側の履歴）。
///
/// # Errors
/// generateContent 呼び出し自体の失敗（429/5xx/timeout 等を出し切った後）を [`GeminiError`] で返す。
/// **個別ツールの失敗は握り潰さず** `{success:false, message}` として Gemini へ返送する
/// （ループは継続・現行 dispatch の catch と同挙動）。
pub async fn run_function_calling_loop(
    backend: &dyn GenerateBackend,
    provider: &dyn ToolProvider,
    system_instruction: &str,
    contents: &mut Vec<Content>,
    ctx: &ToolContext,
    opts: &LoopOptions,
) -> Result<LoopResult, GeminiError> {
    // リクエスト毎に全 provider から宣言を動的生成（現行の毎ターン再構築＝正しいパターン）。
    let decls: Vec<FunctionDeclaration> =
        provider.list(ctx).iter().map(to_wire_declaration).collect();

    notify(opts, Status::Thinking);
    let mut resp = backend
        .generate(Some(system_instruction), &decls, contents, None)
        .await?;

    let mut result = LoopResult::default();
    let mut iterations = 0usize;
    let mut total_calls = 0usize;
    let mut corrections = 0usize;

    loop {
        if iterations >= opts.max_iterations {
            result.hit_max_iterations = true;
            break;
        }
        let Some(candidate) = resp.candidates.first().cloned() else {
            break;
        };
        let calls = candidate.function_calls();

        if calls.is_empty() {
            // 完了ハルシネーション是正: このターンで一度も関数を呼ばず「登録しました」等を主張。
            if total_calls == 0 && corrections < opts.max_corrections && !decls.is_empty() {
                let text = candidate
                    .content
                    .as_ref()
                    .map(Content::collect_text)
                    .unwrap_or_default();
                if claims_action_completed(&text) {
                    corrections += 1;
                    if let Some(content) = candidate.content.clone() {
                        contents.push(content);
                    }
                    contents.push(Content::user_text(COMPLETION_CORRECTION_PROMPT));
                    notify(opts, Status::Thinking);
                    // mode=ANY で構造的に関数呼び出しを強制（AUTO のままだと是正後も繰り返し得る）。
                    let tc = ToolConfig::force_any(opts.allowed_tool_names.clone());
                    resp = backend
                        .generate(Some(system_instruction), &decls, contents, Some(tc))
                        .await?;
                    iterations += 1;
                    continue;
                }
            }
            break;
        }

        total_calls += calls.len();
        let mut response_parts: Vec<Part> = Vec::with_capacity(calls.len());

        // 並行呼び出し公式サポート。id を相関に保持する（独立ツールの並行実行は将来最適化）。
        for fc in &calls {
            result.tool_calls.push(fc.name.clone());
            let response = dispatch_one(provider, ctx, &fc.name, fc.args.clone(), &mut result).await;
            response_parts.push(Part::function_response(FunctionResponse {
                name: fc.name.clone(),
                response,
                id: fc.id.clone(),
            }));
        }

        // model の functionCall 入り content → functionResponse を返送。
        if let Some(content) = candidate.content.clone() {
            contents.push(content);
        }
        contents.push(Content {
            role: Role::User,
            parts: response_parts,
        });
        notify(opts, Status::Writing);
        resp = backend
            .generate(Some(system_instruction), &decls, contents, None)
            .await?;
        iterations += 1;
    }

    result.text = resp.text();
    Ok(result)
}

/// 1 ツールを dispatch し、`functionResponse.response` に載せる JSON を返す。
/// 個別失敗は握り潰さず `{success:false, message}` へ落とす（Gemini へ返してループ継続）。
async fn dispatch_one(
    provider: &dyn ToolProvider,
    ctx: &ToolContext,
    name: &str,
    args: serde_json::Value,
    result: &mut LoopResult,
) -> serde_json::Value {
    let tool_name = match ToolName::checked(name.to_owned()) {
        Ok(n) => n,
        Err(e) => return json!({ "success": false, "message": e.to_string() }),
    };
    match provider.invoke(&tool_name, args, ctx).await {
        Ok(outcome) => {
            // リッチ返信パート（embeds/images）を積み上げて呼び出し側へ渡す。
            result.rich_parts.extend(outcome.parts);
            outcome.payload
        }
        Err(e) => json!({ "success": false, "message": e.to_string() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claims_action_completed_matches_done_forms() {
        assert!(claims_action_completed("タスクを登録しました。"));
        assert!(claims_action_completed("リマインドを設定しておきました"));
        assert!(claims_action_completed("同期しておきますね"));
        assert!(claims_action_completed("やっておきました"));
        assert!(claims_action_completed("しておきますね"));
        assert!(claims_action_completed("有効化します"));
    }

    #[test]
    fn claims_action_completed_ignores_plain_text() {
        assert!(!claims_action_completed(""));
        assert!(!claims_action_completed("こんにちは、今日の天気は晴れです。"));
        assert!(!claims_action_completed("登録が必要ですか？"));
    }

    #[test]
    fn sanitize_schema_strips_dollar_schema_recursively() {
        let schema = json!({
            "$schema": "https://json-schema.org/draft-07/schema",
            "type": "object",
            "properties": {
                "x": { "$schema": "nested", "type": "string" }
            }
        });
        let out = sanitize_schema(&schema);
        assert!(out.get("$schema").is_none());
        assert!(out["properties"]["x"].get("$schema").is_none());
        assert_eq!(out["properties"]["x"]["type"], "string");
    }
}
