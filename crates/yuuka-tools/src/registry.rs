//! 中央レジストリ（§9.3）— provider の集合を保持し、リクエスト毎に `snapshot` を作る。
//!
//! 静的マージ（現行 `buildFunctionRegistry`）ではなく **provider 集合を保持し、`ctx` スコープで
//! `list`/`invoke` を叩く**（MCP/WASM は動的にツールが変わるため）。`snapshot(ctx)` を毎ターン
//! 作るのは*意図された設計*（Gemini は tools/config をインタラクション毎に再送する前提・§9.3）。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use yuuka_core::tool::FunctionDeclaration;
use yuuka_core::{ToolContext, ToolError, ToolName, ToolOutcome, ToolProvider};

/// provider の集合。Native/MCP/WASM を同一 [`ToolProvider`] として透過的に保持する。
#[derive(Default, Clone)]
pub struct ToolRegistry {
    providers: Vec<Arc<dyn ToolProvider>>,
}

impl ToolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// provider を追加する（登録順が名前衝突時の先勝ち順序になる）。
    pub fn register(&mut self, provider: Arc<dyn ToolProvider>) {
        self.providers.push(provider);
    }

    /// ビルダ形式で provider を追加する。
    #[must_use]
    pub fn with_provider(mut self, provider: Arc<dyn ToolProvider>) -> Self {
        self.providers.push(provider);
        self
    }

    /// 全 provider の `list` を集約し、`ToolName → provider` の索引を張る（`ctx` スコープ）。
    ///
    /// provider 横断の名前衝突は**先勝ち＋ログ警告**で握る（不正な MCP サーバがターンを
    /// クラッシュさせないため。native 内部の重複は `NativeProvider::register` が別途 `Err`）。
    #[must_use]
    pub fn snapshot(&self, ctx: &ToolContext) -> RegistrySnapshot {
        self.snapshot_with(ctx, None)
    }

    /// [`Self::snapshot`] に**そのターン限りの追加 provider**（例: 発話者スコープで探索した
    /// `McpProvider`）を末尾に重ねる。追加 provider は最後に評価されるため、既存 Native ツール名と
    /// 衝突した場合は先勝ちで無視される（不正な外部ツールが内蔵ツールを乗っ取れない）。
    ///
    /// MCP/WASM の動的ツールは `ctx`（発話者・Bot）に依存し起動時には確定しないため、レジストリへ
    /// 常設せず会話 1 ターンごとにここで層として足す（Node が FC ループ前に MCP モジュールをマージするのと同義）。
    #[must_use]
    pub fn snapshot_with(
        &self,
        ctx: &ToolContext,
        extra: Option<&Arc<dyn ToolProvider>>,
    ) -> RegistrySnapshot {
        let mut declarations: Vec<FunctionDeclaration> = Vec::new();
        let mut index: HashMap<ToolName, Arc<dyn ToolProvider>> = HashMap::new();
        for provider in self.providers.iter().chain(extra) {
            for decl in provider.list(ctx) {
                if index.contains_key(&decl.name) {
                    tracing::warn!(
                        tool = decl.name.as_str(),
                        "ツール名が provider 横断で衝突: 先勝ちで無視します"
                    );
                    continue;
                }
                index.insert(decl.name.clone(), Arc::clone(provider));
                declarations.push(decl);
            }
        }
        RegistrySnapshot {
            declarations,
            index,
        }
    }
}

/// あるターン（`ctx`）で確定したツール一覧と `name → provider` 索引。
///
/// [`ToolProvider`] を実装するので、そのまま gemini の function-calling ループへ
/// `&dyn ToolProvider` として渡せる（`list` はキャッシュ済み宣言を返す）。
pub struct RegistrySnapshot {
    declarations: Vec<FunctionDeclaration>,
    index: HashMap<ToolName, Arc<dyn ToolProvider>>,
}

impl RegistrySnapshot {
    /// この snapshot の関数宣言（Gemini `functionDeclarations` 生成へ）。
    #[must_use]
    pub fn declarations(&self) -> &[FunctionDeclaration] {
        &self.declarations
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.declarations.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.declarations.len()
    }

    /// 名前が解決可能か（現行 registry の `has`）。
    #[must_use]
    pub fn has(&self, name: &ToolName) -> bool {
        self.index.contains_key(name)
    }

    /// 名前でツールを dispatch する。未知ツールは [`ToolError::UnknownTool`]。
    ///
    /// # Errors
    /// 未知ツール／実行失敗／スコープ違反等で [`ToolError`]（握り潰さない）。
    pub async fn dispatch(
        &self,
        name: &ToolName,
        args: Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        let provider = self
            .index
            .get(name)
            .ok_or_else(|| ToolError::UnknownTool(name.as_str().to_owned()))?;
        provider.invoke(name, args, ctx).await
    }
}

#[async_trait]
impl ToolProvider for RegistrySnapshot {
    fn list(&self, _ctx: &ToolContext) -> Vec<FunctionDeclaration> {
        // snapshot 時点で ctx スコープ確定済み。以後は同一ターン内でキャッシュを返す。
        self.declarations.clone()
    }

    async fn invoke(
        &self,
        name: &ToolName,
        args: Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        self.dispatch(name, args, ctx).await
    }
}
