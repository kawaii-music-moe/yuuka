//! UI 層（egui ビュー）。
//!
//! 各ビューは [`crate::app::AppState`] を受け取り描画する「ビュー関数」として実装する
//! （client_design.md §3）。本フェーズは骨格（honest skeleton）であり、ロジックの
//! 配線（Net への送信等）は最小限。視覚的な作り込みは Phase 3〜5。

pub mod chat;
pub mod login;
pub mod overlay;
pub mod settings;

/// チャット履歴の 1 メッセージ（UI 表示用の最小モデル）。
///
/// `model.rs` の WS 型とは別に、UI 都合の表示状態（role・本文・添付プレビュー等）を
/// 保持する。会話履歴自体は永続化しない（正はサーバ。client_design.md §10）。
#[derive(Debug, Clone)]
pub struct ChatEntry {
    pub role: Role,
    /// 本文（assistant は Markdown としてレンダリングする）。
    pub text: String,
    /// 付随する Embed カード（done/push 由来）。
    pub embeds: Vec<crate::model::Embed>,
    /// 付随ファイル（インライン画像表示用。done/push 由来）。
    pub files: Vec<crate::model::FilePayload>,
    /// サーバ側メッセージ ID（interaction/update の突合キー。components 付き応答のみ）。
    pub message_id: Option<String>,
    /// 対話コンポーネント（action row）。interaction 押下対象（ws_components.md §6）。
    pub components: Vec<crate::model::ActionRow>,
}

impl ChatEntry {
    pub fn user(text: impl Into<String>) -> Self {
        ChatEntry {
            role: Role::User,
            text: text.into(),
            embeds: Vec::new(),
            files: Vec::new(),
            message_id: None,
            components: Vec::new(),
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        ChatEntry {
            role: Role::Assistant,
            text: text.into(),
            embeds: Vec::new(),
            files: Vec::new(),
            message_id: None,
            components: Vec::new(),
        }
    }
}

/// メッセージの送信者種別（左右/色分けに使う。client_design.md §5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}
