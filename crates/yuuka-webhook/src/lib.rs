//! yuuka-webhook — 外部 Webhook 受信・管理 Web-API（Node `webhookRoutes`/`webhookRepo` パリティ）。
//!
//! 受信 `POST /hook/{token}`（公開・レート制限・即時 200）と管理 `/api/webhooks/*`（auth:user）。
//! シークレットは [`yuuka_crypto::SystemCrypto`] で暗号化保存し、応答へは `has_secret` のみ。受信の実処理
//! （HMAC 検証・リプレイ検出・フィルタ・通知・todo/reminder 生成・受信履歴）は [`WebhookProcessor`] シーム
//! 越しに [`RealWebhookProcessor`] が行う（未配線時の既定は no-op の [`NullWebhookProcessor`]）。

mod processor;
pub mod repo;
mod routes;

pub use processor::{
    verify_hmac_sha256_hex, AuxTextGenerator, DeliveryStatus, RealWebhookProcessor, WebhookNotice,
    WebhookNotifier, WebhookOutcome, WebhookTarget,
};
pub use routes::{routes, routes_with, NullWebhookProcessor, WebhookProcessor};
