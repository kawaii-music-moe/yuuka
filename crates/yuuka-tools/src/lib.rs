//! yuuka-tools — `ToolProvider` 中央レジストリ（§9）。
//!
//! 現行 `buildFunctionRegistry`（宣言集約・重複検出・dispatch）を **provider 集合の
//! snapshot モデル**へ置換する。3 系統（Native / MCP=rmcp / WASM=Extism）を同一
//! [`yuuka_core::ToolProvider`] トレイト越しに透過的に扱う。
//!
//! - **Phase 2b（本コミット）**: [`ToolRegistry`]/[`RegistrySnapshot`]/[`NativeProvider`]。
//! - **後続**: `McpProvider`（rmcp 2.0.0）/`WasmProvider`（Extism）を trait 境界越しに追加
//!   （Native+MCP 先行の Phase 0 決定。いずれも `Arc<dyn ToolProvider>` として `register` するだけ）。
//!
//! DAG: `tools → core`。

pub mod native;
pub mod registry;

pub use native::NativeProvider;
pub use registry::{RegistrySnapshot, ToolRegistry};

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde_json::{json, Value};
    use yuuka_core::tool::FunctionDeclaration;
    use yuuka_core::{
        BotId, Tool, ToolContext, ToolError, ToolName, ToolOutcome, ToolProvider, UserId,
    };

    use super::*;

    /// 引数をそのまま payload に載せて返す最小ツール（呼び出し確認用）。
    struct EchoTool {
        name: ToolName,
    }

    impl EchoTool {
        fn new(raw: &str) -> Self {
            Self {
                name: ToolName::namespaced("native", raw).unwrap(),
            }
        }
    }

    #[async_trait]
    impl Tool for EchoTool {
        fn declaration(&self) -> FunctionDeclaration {
            FunctionDeclaration {
                name: self.name.clone(),
                description: format!("echo {}", self.name),
                parameters_json_schema: json!({"type":"object"}),
                requires_confirmation: false,
            }
        }
        async fn call(&self, _ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError> {
            Ok(ToolOutcome::from_payload(
                json!({"success": true, "echo": args, "tool": self.name.as_str()}),
            ))
        }
    }

    fn ctx() -> ToolContext {
        ToolContext::new(BotId::system_default(), UserId::new("u1"))
    }

    #[test]
    fn native_provider_detects_duplicate_registration() {
        let mut np = NativeProvider::new();
        np.register(Arc::new(EchoTool::new("add_todo"))).unwrap();
        let dup = np.register(Arc::new(EchoTool::new("add_todo")));
        assert!(matches!(dup, Err(ToolError::Execution(_))));
        assert_eq!(np.len(), 1);
    }

    #[tokio::test]
    async fn native_provider_lists_and_invokes() {
        let np = NativeProvider::new()
            .with_tool(Arc::new(EchoTool::new("add_todo")))
            .unwrap()
            .with_tool(Arc::new(EchoTool::new("list_todos")))
            .unwrap();

        let decls = np.list(&ctx());
        assert_eq!(decls.len(), 2);

        let name = ToolName::namespaced("native", "add_todo").unwrap();
        let out = np
            .invoke(&name, json!({"title": "x"}), &ctx())
            .await
            .unwrap();
        assert_eq!(out.payload["echo"], json!({"title": "x"}));
        assert_eq!(out.payload["tool"], "native:add_todo");
    }

    #[tokio::test]
    async fn native_provider_unknown_tool_errors() {
        let np = NativeProvider::new()
            .with_tool(Arc::new(EchoTool::new("add_todo")))
            .unwrap();
        let unknown = ToolName::namespaced("native", "nope").unwrap();
        let err = np.invoke(&unknown, json!({}), &ctx()).await.unwrap_err();
        assert!(matches!(err, ToolError::UnknownTool(_)));
    }

    #[tokio::test]
    async fn registry_snapshot_aggregates_and_dispatches() {
        let todo = Arc::new(
            NativeProvider::new()
                .with_tool(Arc::new(EchoTool::new("add_todo")))
                .unwrap(),
        );
        let finance = Arc::new(
            NativeProvider::new()
                .with_tool(Arc::new(EchoTool::new("add_expense")))
                .unwrap(),
        );
        let registry = ToolRegistry::new()
            .with_provider(todo)
            .with_provider(finance);

        let snap = registry.snapshot(&ctx());
        assert_eq!(snap.len(), 2);

        let name = ToolName::namespaced("native", "add_expense").unwrap();
        assert!(snap.has(&name));
        let out = snap.dispatch(&name, json!({"amount": 500}), &ctx()).await.unwrap();
        assert_eq!(out.payload["tool"], "native:add_expense");

        // 未知は UnknownTool。
        let missing = ToolName::namespaced("native", "ghost").unwrap();
        let err = snap.dispatch(&missing, json!({}), &ctx()).await.unwrap_err();
        assert!(matches!(err, ToolError::UnknownTool(_)));
    }

    #[tokio::test]
    async fn snapshot_usable_as_dyn_tool_provider() {
        // gemini ループへ &dyn ToolProvider として渡せること（list/invoke 経由）。
        let registry = ToolRegistry::new().with_provider(Arc::new(
            NativeProvider::new()
                .with_tool(Arc::new(EchoTool::new("add_todo")))
                .unwrap(),
        ));
        let snap = registry.snapshot(&ctx());
        let provider: &dyn ToolProvider = &snap;
        assert_eq!(provider.list(&ctx()).len(), 1);
        let name = ToolName::namespaced("native", "add_todo").unwrap();
        let out = provider.invoke(&name, json!({}), &ctx()).await.unwrap();
        assert_eq!(out.payload["success"], true);
    }

    #[tokio::test]
    async fn cross_provider_name_collision_first_wins() {
        // 2 provider が同名を公開 → 宣言は 1 つ、dispatch は先勝ち provider へ。
        let first = Arc::new(
            NativeProvider::new()
                .with_tool(Arc::new(EchoTool::new("dup")))
                .unwrap(),
        );
        // 別 payload を返すツールを 2 番目に。名前衝突で無視される。
        struct SecondTool {
            name: ToolName,
        }
        #[async_trait]
        impl Tool for SecondTool {
            fn declaration(&self) -> FunctionDeclaration {
                FunctionDeclaration {
                    name: self.name.clone(),
                    description: "second".to_owned(),
                    parameters_json_schema: json!({"type":"object"}),
                    requires_confirmation: false,
                }
            }
            async fn call(
                &self,
                _ctx: &ToolContext,
                _args: Value,
            ) -> Result<ToolOutcome, ToolError> {
                Ok(ToolOutcome::from_payload(json!({"from": "second"})))
            }
        }
        let second = Arc::new(
            NativeProvider::new()
                .with_tool(Arc::new(SecondTool {
                    name: ToolName::namespaced("native", "dup").unwrap(),
                }))
                .unwrap(),
        );

        let registry = ToolRegistry::new()
            .with_provider(first)
            .with_provider(second);
        let snap = registry.snapshot(&ctx());
        assert_eq!(snap.len(), 1, "衝突名は 1 宣言に集約");

        let name = ToolName::namespaced("native", "dup").unwrap();
        let out = snap.dispatch(&name, json!({}), &ctx()).await.unwrap();
        // 先勝ち＝EchoTool（"tool" フィールドを持つ）。SecondTool の "from" ではない。
        assert_eq!(out.payload["tool"], "native:dup");
    }
}
