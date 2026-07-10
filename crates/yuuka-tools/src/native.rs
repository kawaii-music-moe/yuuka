//! `NativeProvider`（§9.4）— 内蔵 Rust ツール（現行 `src/functions/*` 相当）を束ねる。
//!
//! 信頼コードなのでサンドボックス不要。`ctx.user_id` によるデータ分離のみ厳守する。
//! namespace 接頭辞は `native:`（`ToolName::namespaced` で型保証）。各ドメインクレート
//! （yuuka-todo/finance/…）が `Tool` 実装をここへ登録する（Phase 2c）。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use yuuka_core::tool::FunctionDeclaration;
use yuuka_core::{Tool, ToolContext, ToolError, ToolName, ToolOutcome, ToolProvider};

/// 内蔵ツールの集合。名前は完全修飾 [`ToolName`]（`native:add_todo` 等）で索引する。
#[derive(Default)]
pub struct NativeProvider {
    tools: HashMap<ToolName, Arc<dyn Tool>>,
}

impl NativeProvider {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// ツールを登録する。宣言名の重複は**プログラマ誤り**として `Err` を返す
    /// （現行 `buildFunctionRegistry` の「重複で throw」と同義。ただしプロセスは落とさない）。
    ///
    /// # Errors
    /// 同名ツールが既に登録済みなら [`ToolError::Execution`]（重複登録）。
    pub fn register(&mut self, tool: Arc<dyn Tool>) -> Result<(), ToolError> {
        let name = tool.declaration().name;
        if self.tools.contains_key(&name) {
            return Err(ToolError::Execution(format!(
                "ツール名が重複しています: {name}"
            )));
        }
        self.tools.insert(name, tool);
        Ok(())
    }

    /// ビルダ形式で登録する（複数登録の連結用）。
    ///
    /// # Errors
    /// 重複登録時は [`ToolError::Execution`]。
    pub fn with_tool(mut self, tool: Arc<dyn Tool>) -> Result<Self, ToolError> {
        self.register(tool)?;
        Ok(self)
    }

    /// 登録済みツール数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

#[async_trait]
impl ToolProvider for NativeProvider {
    fn list(&self, ctx: &ToolContext) -> Vec<FunctionDeclaration> {
        // 能力ゲート: 経路（秘書/汎用モード）× 能力集合で見えるツールだけを返す。露出不可のツールは宣言も
        // index も作られない（snapshot が本 list から index を構築）ため invoke も不可。
        //
        // **部分パリティ**: Node `getFunctionModulesForCapabilities` は「能力」に加えユーザー別の
        // `enabledModules`（`bot_user_modules`/`bots.enabled_modules` の selectable モジュール絞り込み・
        // `resolveEnabledModulesForUser`）も適用するが、その第 2 次元は未移植（module 選択 UI 系＝P2-A）。
        // 現状は能力 + 経路の 2 軸のみ。
        self.tools
            .values()
            .filter(|t| t.exposure().is_visible(ctx))
            .map(|t| t.declaration())
            .collect()
    }

    async fn invoke(
        &self,
        name: &ToolName,
        args: Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| ToolError::UnknownTool(name.as_str().to_owned()))?;
        tool.call(ctx, args).await
    }
}
