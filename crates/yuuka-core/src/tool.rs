//! Tool / ToolProvider 契約（§9・§12.2 の凍結契約4）。
//!
//! gemini/functions と全ドメインモジュールの共有契約。ここを凍結してから
//! Phase 1 の 9 ドメインを並行展開する。ToolContext は `UserId` を **不変借用**で持ち、
//! 副作用は戻り値 `ToolOutcome` に寄せる（R-24: `&mut ctx` にしない）。
//!
//! エラーは [`crate::error::ToolError`]（`PluginError` 別名）を用いる。

use async_trait::async_trait;
use serde_json::Value;

use crate::error::ToolError;
use crate::ids::{BotId, GuildId, UserId};

/// ツール実行の必須スコープと能力を運ぶコンテキスト。
///
/// `user_id` はデータ分離の必須キー（newtype で欠落を型排除）。副作用は本体を
/// **不変借用**で受け取り、結果を `ToolOutcome` へ寄せる設計とする（R-24）。
/// リッチ返信の embeds/files 等の可変バッファは Phase 1 の yuuka-tools 側で
/// 内部可変性（`Mutex` 等）に載せるため、契約の借用シグネチャは `&self` を保つ。
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub bot_id: BotId,
    /// データ分離キー。全リポジトリ呼び出し・能力スコープ判定に必須。
    pub user_id: UserId,
    /// ギルド常駐 Bot のみ Some（DM/秘書利用では None）。
    pub guild_id: Option<GuildId>,
    /// この呼び出しに許された能力集合（deny-by-default）。
    pub capabilities: CapabilitySet,
    /// リッチ返信が有効か（`false` のとき push 禁止は呼び出し側で担保）。
    pub rich_reply_enabled: bool,
}

impl ToolContext {
    #[must_use]
    pub fn new(bot_id: BotId, user_id: UserId) -> Self {
        Self {
            bot_id,
            user_id,
            guild_id: None,
            capabilities: CapabilitySet::default(),
            rich_reply_enabled: false,
        }
    }
}

/// deny-by-default の能力集合。付与された能力のみを保持する。
#[derive(Debug, Clone, Default)]
pub struct CapabilitySet {
    granted: Vec<String>,
}

impl CapabilitySet {
    #[must_use]
    pub fn from_granted(granted: Vec<String>) -> Self {
        Self { granted }
    }

    /// 指定能力が付与されているか。
    #[must_use]
    pub fn has(&self, capability: &str) -> bool {
        self.granted.iter().any(|c| c == capability)
    }

    #[must_use]
    pub fn granted(&self) -> &[String] {
        &self.granted
    }
}

/// Gemini `functionDeclarations` へ流す中間表現（provider 非依存）。
///
/// `parameters_json_schema` はフル JSON Schema（`parametersJsonSchema` へ直送。
/// sanitizer 通過前の生スキーマ）。名前制約（`[a-zA-Z0-9_:.-]`・1..=128）は
/// [`ToolName`] のスマートコンストラクタで型保証する。
#[derive(Debug, Clone)]
pub struct FunctionDeclaration {
    pub name: ToolName,
    pub description: String,
    pub parameters_json_schema: Value,
    /// 実行前にユーザー承認を要するか（現行 `requires_confirmation` 相当）。
    pub requires_confirmation: bool,
}

/// ツール実行結果。テキスト/構造化 payload とマルチモーダル parts を表現する。
///
/// エラーの JSON 丸め込みはここではせず、`Result<ToolOutcome, ToolError>` で返して
/// Gemini 側 `functionResponse` 組み立てで一元化する（握り潰さない・§9.1）。
#[derive(Debug, Clone)]
pub struct ToolOutcome {
    /// `functionResponse.response` に載せる本文。
    pub payload: Value,
    /// マルチモーダル `functionResponse.parts`（無ければ空）。
    pub parts: Vec<ResponsePart>,
}

impl ToolOutcome {
    /// payload のみの結果を作る（parts 無し）。
    #[must_use]
    pub fn from_payload(payload: Value) -> Self {
        Self {
            payload,
            parts: Vec::new(),
        }
    }
}

/// マルチモーダル応答パート（画像等）。Phase 1 で具体バリアントを拡張する。
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ResponsePart {
    /// base64 埋め込みデータ（`inlineData` 相当）。
    InlineData { mime_type: String, data: String },
}

/// Gemini `FunctionDeclaration.name` 制約（`[a-zA-Z0-9_:.-]`・1..=128 文字）を
/// 型で保証する newtype。無効名を構築できない（§9.2）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ToolName(String);

impl ToolName {
    pub const MAX_LEN: usize = 128;

    /// provider 種別の namespace 接頭辞を付けて構築する。
    /// 例: Native → `native:contacts_save` / MCP → `mcp7:list_items`。
    /// **接頭辞・ツール名の双方**を許可外文字 `_` へサニタイズしてから検査する
    /// （`:` は名前空間区切り専用なので各セグメントには含めない）。
    ///
    /// # Errors
    /// サニタイズ後も長さ/文字集合制約を満たさない場合 [`ToolError::InvalidToolName`]。
    pub fn namespaced(prefix: &str, raw: &str) -> Result<Self, ToolError> {
        let prefix = Self::sanitize_segment(prefix);
        let raw = Self::sanitize_segment(raw);
        Self::checked(format!("{prefix}:{raw}"))
    }

    /// 名前空間セグメント（接頭辞 or ツール名）を許可文字集合へサニタイズする。
    /// `:` は含めない（区切り専用）ので `_` へ落とす。
    fn sanitize_segment(s: &str) -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }

    /// 完全修飾名を直接検査して構築する。
    ///
    /// # Errors
    /// 長さ（1..=128 **文字**）または文字集合（`[a-zA-Z0-9_:.-]`）に反する場合
    /// [`ToolError::InvalidToolName`]。
    pub fn checked(name: String) -> Result<Self, ToolError> {
        // Gemini 制約は「128 文字」。charset は ASCII 限定なので実質 byte 数と一致するが、
        // 意図を明示するため文字数で判定する。
        let len_ok = (1..=Self::MAX_LEN).contains(&name.chars().count());
        let charset_ok = name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.' | '-'));
        if len_ok && charset_ok {
            Ok(Self(name))
        } else {
            Err(ToolError::InvalidToolName(name))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ToolName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 単一ツールの契約（§12.2 の凍結トレイト）。
///
/// `ctx` は **不変借用**（副作用は `ToolOutcome` に寄せる・R-24）。`async fn in trait`
/// は dyn 互換のため `#[async_trait]` で object-safe 化する（レジストリが `dyn Tool` を持つ）。
#[async_trait]
pub trait Tool: Send + Sync {
    /// このツールの Gemini 宣言を返す。
    fn declaration(&self) -> FunctionDeclaration;

    /// ツールを実行する。`args` は sanitizer 通過後の引数。
    ///
    /// # Errors
    /// 実行失敗・スコープ違反・タイムアウト等で [`ToolError`] を返す（握り潰さない）。
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError>;
}

/// ツール群を公開する provider の契約（Native/MCP/WASM 共通・§9.1）。
///
/// `list` はリクエスト毎に `ctx` スコープで見えるツールを返す（動的 provider 対応）。
#[async_trait]
pub trait ToolProvider: Send + Sync {
    /// この provider が現時点（`ctx` スコープ）で公開する全ツール宣言。
    fn list(&self, ctx: &ToolContext) -> Vec<FunctionDeclaration>;

    /// 完全修飾名でツールを実行する。
    ///
    /// # Errors
    /// 未知ツール・スコープ違反・実行失敗等で [`ToolError`] を返す。
    async fn invoke(
        &self,
        name: &ToolName,
        args: Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutcome, ToolError>;
}
