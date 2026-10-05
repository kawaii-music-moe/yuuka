//! REST ヘルパ（reqwest + Bearer）。
//!
//! 仕様: backend_api.md §1.5（Bearer 認証）/ §4（端末管理）/ client_design.md §9。
//! 現状の利用箇所は `GET /api/bots`（Bot 一覧・オーブ画像取得）と
//! `GET /api/devices`（端末一覧）。トークン失効（ログアウト）は
//! `POST /api/devices/revoke`（自端末）を使う。

use serde::Deserialize;

use crate::config::rest_base;
use crate::model::BotInfo;

/// REST 呼び出しのエラー。
#[derive(Debug, thiserror::Error)]
pub enum RestError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    /// 401: トークン失効。呼び出し側は保存トークンを破棄して再ログインへ。
    #[error("unauthorized (token invalid/expired)")]
    Unauthorized,
}

/// `GET /api/devices` の 1 要素（backend_api.md §4）。
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceInfo {
    pub id: String,
    #[serde(default)]
    pub device_name: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub last_used_at: Option<String>,
    /// この端末自身か。
    #[serde(default)]
    pub current: Option<bool>,
}

/// Bearer 付きの `reqwest::RequestBuilder` を作る共通ヘルパ。
fn bearer(req: reqwest::RequestBuilder, token: &str) -> reqwest::RequestBuilder {
    req.bearer_auth(token)
}

/// 401 を [`RestError::Unauthorized`] に正規化してから JSON を読む。
async fn json_or_unauthorized<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
) -> Result<T, RestError> {
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(RestError::Unauthorized);
    }
    Ok(resp.error_for_status()?.json().await?)
}

/// `GET /api/bots` のレスポンス封筒。
///
/// 注意: `/api/bots` は **`{ "success": true, "bots": [...] }`** を返す（裸の配列ではない）。
/// 旧実装は `Vec<BotInfo>` を直接デシリアライズしようとして必ず失敗し、Net 層が
/// Bot 解決に永久リトライ → WS 未接続 → ログインが「認証待ち」のまま固まっていた。
/// また各要素は WS の `ready.bots` と違い `primary` を含まない（`BotInfo` 側で既定 false）。
#[derive(Debug, Deserialize)]
struct BotsResponse {
    #[serde(default)]
    bots: Vec<BotInfo>,
}

/// `GET /api/bots` — 所有/共有 Bot の一覧。
///
/// オーブ画像（`discord_avatar_url`）と切替セレクタに使う。要素は [`BotInfo`]。
pub async fn get_bots(client: &reqwest::Client, token: &str) -> Result<Vec<BotInfo>, RestError> {
    let resp = bearer(client.get(format!("{}/api/bots", rest_base())), token)
        .send()
        .await?;
    let body: BotsResponse = json_or_unauthorized(resp).await?;
    Ok(body.bots)
}

/// Bot のアイコン画像バイト列を取得する（オーブ描画用。client_design.md §4.1）。
///
/// `discord_avatar_url` は通常 Discord CDN の公開 URL（認証不要）なので Bearer は付けない。
/// 取得は Net スレッドで行い（UI スレッドを塞がない）、結果は `NetEvent::Avatar` で UI へ渡す。
pub async fn fetch_avatar(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, RestError> {
    let resp = client.get(url).send().await?.error_for_status()?;
    Ok(resp.bytes().await?.to_vec())
}

/// `GET /api/devices` — 端末一覧（端末管理・自端末判定）。
pub async fn get_devices(
    client: &reqwest::Client,
    token: &str,
) -> Result<Vec<DeviceInfo>, RestError> {
    let resp = bearer(client.get(format!("{}/api/devices", rest_base())), token)
        .send()
        .await?;
    json_or_unauthorized(resp).await
}

/// `POST /api/devices/revoke` — 自端末トークンの失効（ログアウト時。client_design.md §8）。
pub async fn revoke_device(
    client: &reqwest::Client,
    token: &str,
    device_id: &str,
) -> Result<(), RestError> {
    let resp = bearer(
        client.post(format!("{}/api/devices/revoke", rest_base())),
        token,
    )
    .json(&serde_json::json!({ "id": device_id }))
    .send()
    .await?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(RestError::Unauthorized);
    }
    resp.error_for_status()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::BotsResponse;

    /// 回帰: `/api/bots` は `{ success, bots:[...] }` を返し、各要素は `primary` を
    /// 含まない（多数の追加フィールドあり）。封筒から `bots` を取り出せること。
    /// 旧実装（`Vec<BotInfo>` 直接）はこの形で必ず失敗し、ログインが固まっていた。
    #[test]
    fn parses_wrapped_bots_response() {
        // 実際の dev 応答（system_default。primary 無し・追加フィールド多数）。
        let json = r#"{"success":true,"bots":[{"id":"system_default","user_id":"493051731555516417","name":"システムデフォルト","recommended_persona_id":null,"persona_id":null,"capabilities":"[\"persona\"]","discord_username":"鬼方カヨコ","discord_avatar_url":"https://cdn.example/a.webp","discord_application_id":null,"suspended":0,"created_at":"2026-01-01","updated_at":"2026-01-01","preset":"secretary","preset_display_name":"秘書","has_gemini_key":false,"has_token":false,"running":false,"connected":false,"shared":false}]}"#;
        let parsed: BotsResponse = serde_json::from_str(json).expect("wrapped bots response must parse");
        assert_eq!(parsed.bots.len(), 1);
        assert_eq!(parsed.bots[0].id, "system_default");
        assert_eq!(parsed.bots[0].name, "システムデフォルト");
        // primary は応答に無いので既定 false。
        assert!(!parsed.bots[0].primary);
        assert!(parsed.bots[0].discord_avatar_url.is_some());
    }

    /// 旧実装が誤っていたことの裏付け: 封筒オブジェクトは `Vec<BotInfo>` には解釈できない。
    #[test]
    fn bare_vec_deserialization_fails_on_wrapped_object() {
        let json = r#"{"success":true,"bots":[]}"#;
        let as_vec: Result<Vec<crate::model::BotInfo>, _> = serde_json::from_str(json);
        assert!(as_vec.is_err());
    }
}
