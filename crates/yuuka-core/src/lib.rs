//! yuuka-core — 全クレートが依存する土台（config・層別エラー・newtype・secrets・
//! telemetry・Tool/Repo 契約トレイト）。
//!
//! DAG: `types → core`, `db → core`, `all → core`。**core は誰にも依存しない**。
//! 本クレートの契約は Phase 0（契約凍結）で確定し、以後変更しない（§12.2）。

pub mod config;
pub mod error;
pub mod ids;
pub mod scope;
pub mod secrets;
pub mod telemetry;
pub mod tool;

// よく使う型を crate ルートへ再エクスポート（feature crate の import を短くする）。
pub use config::Config;
pub use error::{
    AppError, AuthError, ConfigError, DbError, DiscordError, Fatality, GeminiError, IpcError,
    PluginError, RepoError, Retryability, ToolError, ValidationError, WebError,
};
pub use ids::{BotId, GuildId, UserId};
pub use scope::{CronScan, ScopedRepo, UserScope};
pub use tool::{
    CapabilitySet, FunctionDeclaration, ResponsePart, Tool, ToolContext, ToolName, ToolOutcome,
    ToolProvider,
};

#[cfg(test)]
mod tests {
    use super::error::{AppError, ConfigError, DbError, Fatality, WebError};
    use super::ids::{BotId, UserId};
    use super::tool::ToolName;

    #[test]
    fn web_error_status_covers_all_variants() {
        // 網羅 match が全バリアントで status を返すこと（追加漏れはコンパイルエラー側で検知）。
        assert_eq!(WebError::Unauthorized.status(), 401);
        assert_eq!(WebError::Forbidden.status(), 403);
        assert_eq!(WebError::NotFound.status(), 404);
        assert_eq!(WebError::Conflict.status(), 409);
        assert_eq!(WebError::Validation("bad".into()).status(), 400);
        assert_eq!(WebError::Upstream.status(), 502);
        assert_eq!(WebError::Internal.status(), 500);
    }

    #[test]
    fn internal_display_not_leaked_to_client() {
        // Internal は Display（機微）を漏らさず "internal" に丸める。
        assert_eq!(WebError::Internal.client_message(), "internal");
        // Validation のみユーザー入力由来なので露出してよい。
        assert_eq!(
            WebError::Validation("field x".into()).client_message(),
            "field x"
        );
    }

    #[test]
    fn fatality_classification_is_variant_specific() {
        let cfg: AppError = ConfigError::MissingSecret { name: "gemini" }.into();
        assert_eq!(cfg.fatality(), Fatality::Fatal);
        assert!(cfg.is_fatal());

        // DB は variant 別: Migration=Fatal（起動時 fail-fast）/ WriterGone=Permanent / Busy=Transient。
        // 一律 Transient だと起動時 schema 不一致が無限バックオフ再起動になる回帰を防ぐ。
        let mig: AppError = DbError::Migration {
            expected: "17".to_owned(),
            found: "16".to_owned(),
        }
        .into();
        assert_eq!(mig.fatality(), Fatality::Fatal);
        assert!(mig.is_fatal());

        let gone: AppError = DbError::WriterGone.into();
        assert_eq!(gone.fatality(), Fatality::Permanent);
        assert!(!gone.is_transient());

        let busy: AppError = DbError::Busy.into();
        assert_eq!(busy.fatality(), Fatality::Transient);
        assert!(busy.is_transient());

        let web: AppError = WebError::NotFound.into();
        assert_eq!(web.fatality(), Fatality::Permanent);
        assert!(!web.is_fatal());
        assert!(!web.is_transient());
    }

    #[test]
    fn tool_name_enforces_gemini_constraints() {
        let ok = ToolName::namespaced("native", "contacts_save").expect("valid name");
        assert_eq!(ok.as_str(), "native:contacts_save");

        // 許可外文字はサニタイズされる。
        let sanitized = ToolName::namespaced("mcp7", "list items!").expect("sanitized name");
        assert_eq!(sanitized.as_str(), "mcp7:list_items_");

        // 128 字超は拒否。
        let long = "a".repeat(200);
        assert!(ToolName::checked(long).is_err());

        // 空は拒否。
        assert!(ToolName::checked(String::new()).is_err());
    }

    #[test]
    fn ids_are_opaque_newtypes() {
        let u = UserId::new("123");
        assert_eq!(u.as_str(), "123");
        let b = BotId::system_default();
        assert_eq!(b.as_str(), "system_default");
    }
}
