//! `ScopedJson<T>` — JSON body を型 `T` へ読みつつ **botId を body 優先 → query
//! フォールバック**で解決する、全ドメイン共通の状態変更用抽出器。
//!
//! # なぜ必要か（fan-out 増幅バグの一元封じ込め）
//! フロント契約（`frontend/src/lib/api/client.ts`）は `scope:'bot'` のリクエストで、
//! **GET/FormData は botId を query に、JSON body（POST/PUT/PATCH/DELETE）は botId を
//! body に**注入する（サーバは body 優先で両対応、とコメントで明記）。Node の各ルートも
//! `resolveBotId` で `ctx.body.botId ?? query.botId` の順に読む。
//!
//! ところが Rust の状態変更ハンドラが `Query<BotQuery>` からしか botId を読まないと、
//! body の botId を取りこぼし、**非既定 bot での作成/更新/削除がすべて `system_default`
//! スコープへ落ちる**（別 bot 空間へデータが迷子・自 bot の削除が 404）。これは 1 ドメイン
//! ではなく状態変更ハンドラ全 18 本に増幅する系統バグなので、共通抽出器に一元化して封じる。
//!
//! GET 系（list/day）は従来どおり `Query<BotQuery>`（botId は query）で正しい。

use axum::extract::{FromRequest, FromRequestParts, Query, Request};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use yuuka_core::WebError;

use crate::error::ApiError;

/// `?botId=` を拾うためだけの query 形（`BotQuery` 相当・本抽出器内部専用）。
#[derive(Debug, Deserialize)]
struct BotIdQuery {
    #[serde(default, rename = "botId")]
    bot_id: Option<String>,
}

/// JSON body を `T` に読み、botId を **body 優先 → query** で解決する抽出器。
///
/// `bot_id` はそのまま [`crate::resolve_scope`] に渡す（`""`/`system_default`/未アクセスは
/// `resolve_scope` 側で `system_default` に畳まれる）。ハンドラ引数の**最後**に置くこと
/// （body を消費する `FromRequest` のため）。
pub struct ScopedJson<T> {
    /// 解決前の raw botId（body の `botId` を優先し、無ければ query の `botId`）。
    pub bot_id: Option<String>,
    /// ドメインの型付きペイロード。
    pub value: T,
}

impl<T, S> FromRequest<S> for ScopedJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        // body 消費前に query の botId（フォールバック）を取り出す。query が壊れていても
        // 認可漏れにはならない（未解決 → system_default）ので `.ok()` で握る。
        let (mut parts, body) = req.into_parts();
        let query_bot = Query::<BotIdQuery>::from_request_parts(&mut parts, state)
            .await
            .ok()
            .and_then(|q| q.0.bot_id);
        let req = Request::from_parts(parts, body);

        // body を JSON Value として読む（DefaultBodyLimit レイヤは Bytes 抽出にも効く）。
        let bytes = axum::body::Bytes::from_request(req, state)
            .await
            .map_err(|_| ApiError(WebError::Validation("unreadable request body".to_owned())))?;
        let json: Value = serde_json::from_slice(&bytes)
            .map_err(|_| ApiError(WebError::Validation("request body must be JSON".to_owned())))?;

        // body.botId ?? query.botId（Node `resolveBotId` と同一 nullish 優先）。
        // `botId` キーが在れば（空文字含む）body 値を採用、非存在時のみ query へ。
        let body_bot = json.get("botId").and_then(Value::as_str).map(str::to_owned);
        let bot_id = body_bot.or(query_bot);

        // 余分な `botId` キーは無視して T へ（既存 DTO は deny_unknown_fields 無し）。
        let value = serde_json::from_value(json)
            .map_err(|e| ApiError(WebError::Validation(format!("invalid request body: {e}"))))?;

        Ok(Self { bot_id, value })
    }
}

#[cfg(test)]
mod tests {
    use super::ScopedJson;
    use axum::body::Body;
    use axum::extract::FromRequest;
    use axum::http::Request;
    use serde_json::Value;

    fn post(uri: &str, body: &'static str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body))
            .expect("request")
    }

    #[tokio::test]
    async fn body_botid_wins_over_query() {
        // フロントは JSON body に botId を注入する。body 優先で拾えること。
        let req = post("/api/tasks/add?botId=queryBot", r#"{"botId":"bodyBot","title":"t"}"#);
        let s = ScopedJson::<Value>::from_request(req, &()).await.expect("extract");
        assert_eq!(s.bot_id.as_deref(), Some("bodyBot"));
        assert_eq!(s.value["title"], serde_json::json!("t"));
    }

    #[tokio::test]
    async fn falls_back_to_query_when_body_absent() {
        let req = post("/x?botId=queryBot", r#"{"title":"t"}"#);
        let s = ScopedJson::<Value>::from_request(req, &()).await.expect("extract");
        assert_eq!(s.bot_id.as_deref(), Some("queryBot"));
    }

    #[tokio::test]
    async fn none_when_botid_absent_everywhere() {
        // botId がどこにも無ければ None（呼び出し側で system_default に畳まれる）。
        let req = post("/x", r#"{"title":"t"}"#);
        let s = ScopedJson::<Value>::from_request(req, &()).await.expect("extract");
        assert_eq!(s.bot_id, None);
    }

    #[tokio::test]
    async fn malformed_json_is_rejected() {
        let req = post("/x", r#"{"title":"#);
        let r = ScopedJson::<Value>::from_request(req, &()).await;
        assert!(r.is_err(), "malformed JSON body must be rejected");
    }
}
