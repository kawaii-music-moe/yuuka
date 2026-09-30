//! `/api/client/*`（PWA）の wire DTO — Node `clientRoutes.ts` の応答形状と 1:1（camelCase・
//! `client/pwa/src/api/contracts.ts` が唯一の契約）。
//!
//! 一般ダッシュボード（`yuuka-todo`/`yuuka-finance`/`yuuka-schedule` 等）の snake_case
//! クリーンビュー・`Envelope<T>` 包みとは**別の契約**のため、ここに独立した DTO を持つ
//! （PWA は Envelope を知らない・`id` は文字列）。ts-rs は使わない（PWA は手書き TS 契約を
//! 正としており、`generated/` へ二重に真実を持たせない）。

use serde::{Deserialize, Serialize};

/// `GET /api/client/status`。
#[derive(Debug, Clone, Serialize)]
pub struct StatusView {
    pub status: &'static str,
    pub service: &'static str,
    #[serde(rename = "checkedAt")]
    pub checked_at: String,
}

/// `GET`/`PUT /api/client/settings`（`AgentSettings`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    pub google_connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub google_account: Option<String>,
    pub model: String,
    pub max_tokens: i64,
    pub temperature: f64,
    pub persona: String,
}

/// `PUT /api/client/settings` の body。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SettingsUpdate {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub persona: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<serde_json::Value>,
    #[serde(default)]
    pub temperature: Option<serde_json::Value>,
}

/// `GET`/`PUT /api/client/shared-note`（`SharedNote`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedNoteView {
    pub id: &'static str,
    pub title: String,
    pub body: String,
    pub updated_at: String,
}

/// `PUT /api/client/shared-note` の body。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SharedNoteUpdate {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
}

/// `GET`/`POST`/`PATCH /api/client/todos*`（`Todo`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoView {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    pub completed: bool,
    pub list: String,
}

/// `POST /api/client/todos` の body。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct NewTodoInput {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, rename = "dueDate")]
    pub due_date: Option<String>,
    #[serde(default)]
    pub list: Option<String>,
}

/// `PATCH /api/client/todos/:id` の body。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TodoPatch {
    #[serde(default)]
    pub completed: Option<bool>,
}

/// `GET /api/client/calendar/events` の 1 件（`CalendarEvent`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEventView {
    pub id: String,
    pub title: String,
    pub starts_at: String,
    pub ends_at: String,
    /// Node 互換（生の Google カレンダー ID または `"Personal"`）。既存クライアント契約を壊さない。
    pub calendar: String,
    /// 人間可読ラベル（issue #47）。`calendar` を置き換えず追加する新フィールド（現行 PWA
    /// クライアント（PR #63 時点）は未消費・将来の表示改善用）。
    pub calendar_name: String,
    pub color: &'static str,
}

/// `GET /api/client/finance/summary`（`FinanceSummary`）。
#[derive(Debug, Clone, Serialize)]
pub struct FinanceSummaryView {
    pub income: i64,
    pub expense: i64,
    pub balance: i64,
    pub month: String,
}

/// `GET`/`POST /api/client/finance/transactions` の 1 件（`Transaction`）。
#[derive(Debug, Clone, Serialize)]
pub struct TransactionView {
    pub id: String,
    pub date: String,
    pub category: String,
    pub description: String,
    pub amount: i64,
    pub kind: String,
}

/// `POST /api/client/finance/transactions` の body。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct NewTransactionInput {
    #[serde(default)]
    pub amount: Option<serde_json::Value>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
}

/// `POST /api/client/chat/messages` の body（issue #41）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ChatSendInput {
    #[serde(default)]
    pub content: Option<String>,
}

/// `POST /api/client/chat/messages` の応答（issue #41 PR #75 レビュー・P1: 非同期配信）。
///
/// 重いターン本体はバックグラウンドで実行するため `202 Accepted` を即座に返す（同期 `fetch` で
/// 180 秒待つ旧設計だとリバースプロキシのタイムアウトが先に来て JSON 504 すら届かない問題の解消）。
/// クライアントは `sinceId` を起点に `GET /api/client/chat/messages` をポーリングし、
/// `id > sinceId` のアシスタント応答（`role:"agent"`）が現れたら完了とみなす（詳細は
/// `routes.rs` の `chat_send` ハンドラ doc 参照）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSendAccepted {
    /// 常に `"pending"`（将来 SSE 等を足す余地のため文字列で固定）。
    pub status: &'static str,
    /// この送信より前の直近 `message_logs.id`（文字列化・`ChatMessageView::id` と型を揃える）。
    /// ポーリングはこの値より大きい `id` を持つ `role:"agent"` の出現を待つ。
    pub since_id: String,
}

/// `GET /api/client/chat/messages` の 1 件（`ChatMessage`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessageView {
    pub id: String,
    /// `"user" | "agent"`。
    pub role: &'static str,
    pub content: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub references: Option<Vec<ChatReferenceView>>,
    /// リッチ埋め込み（issue #41 PR #75 レビュー・P2）。無ければ省略（配列を送らない＝旧クライアント
    /// 互換）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub embeds: Vec<ChatEmbedView>,
    /// ファイル添付（issue #41 PR #75 レビュー・P2）。実バイトは `url` の認証済みエンドポイントへ。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<ChatFileView>,
}

/// [`ChatMessageView::embeds`] の 1 件（`yuuka_discord::RichEmbed` の wire 表現・camelCase）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatEmbedView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 0xRRGGBB。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<ChatEmbedFieldView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub footer: Option<String>,
}

/// [`ChatEmbedView::fields`] の 1 件。
#[derive(Debug, Clone, Serialize)]
pub struct ChatEmbedFieldView {
    pub name: String,
    pub value: String,
    pub inline: bool,
}

/// [`ChatMessageView::files`] の 1 件（issue #41 PR #75 レビュー・P2）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatFileView {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    /// `GET /api/client/chat/attachments/:id`（認証必須・所有者スコープ）。
    pub url: String,
}

/// [`ChatMessageView::references`] の 1 件（`ChatReference`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatReferenceView {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub href: &'static str,
    pub meta: &'static str,
}

/// 汎用エラー応答（Node `sendJson(res, code, { message })` パリティ）。
#[derive(Debug, Clone, Serialize)]
pub struct MessageBody {
    pub message: String,
}

impl MessageBody {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}
