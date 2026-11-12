//! MCP 管理ルートハンドラ（登録・ツール取得・有効/無効・削除・OAuth 認可・MCP Apps）。
//!
//! すべて [`AuthenticatedUser`] extractor で認証を型強制する（例外は認可サーバーが読む Client ID
//! Metadata Document と、サンドボックスプロキシの静的ページ）。想定内の業務エラー
//! （400/403/404/409/502）は明示 JSON、想定外の [`DbError`](yuuka_core::DbError) のみ `?` で
//! [`ApiError`] に写像（500）。

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::header::{CACHE_CONTROL, LOCATION};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde_json::{json, Value};
use yuuka_web::{ApiError, AppState, AuthenticatedUser};

use crate::provider::tools_to_cache_json;
use crate::repo::{self, McpServerRecord};
use crate::{oauth, sandbox, AuthorizationCallback, AuthorizationStart, McpRuntime, McpTool};

// ─── 共通ヘルパ ───────────────────────────────────────────────────────────────

/// 一覧に載せるツール 1 件（MCP Apps の画面の有無と公開先を含む）。
fn tool_view(t: &McpTool) -> Value {
    json!({
        "name": t.name,
        "description": t.description.clone().unwrap_or_default(),
        "ui_resource_uri": t.ui_resource_uri(),
        "visible_to_model": t.visible_to_model(),
        "callable_from_app": t.callable_from_app(),
    })
}

/// Node `toSafeView`：認証情報・暗号化列を除いた安全なビュー（`granted_bot_ids`・`auth` は呼び出し側で足す）。
fn to_safe_view(server: &McpServerRecord) -> Value {
    let tools: Vec<Value> = repo::parse_tools_cache_full(server)
        .iter()
        .map(tool_view)
        .collect();
    json!({
        "id": server.id,
        "scope": server.scope(),
        "name": server.name,
        "endpoint_url": server.endpoint_url,
        "has_auth": server.has_auth(),
        "tools": tools,
        "tools_cache_updated": server.tools_cache_updated,
        "requires_confirmation": server.requires_confirmation == 1,
        "enabled": server.enabled == 1,
        "created_at": server.created_at,
    })
}

/// 業務エラー用の `{success:false, message}` JSON 応答。
fn err_json(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(json!({ "success": false, "message": message })),
    )
        .into_response()
}

/// `Number(body.id)` + `Number.isInteger` 相当。整数（number）または整数値の numeric string のみ受理。
/// Node の `Number.isInteger(Number(x))` に合わせ、小数・非数・欠落は `None`。
fn parse_body_id(v: Option<&Value>) -> Option<i64> {
    match v {
        Some(Value::Number(n)) => {
            // 整数として表せる number のみ（Node `Number.isInteger`）。
            if let Some(i) = n.as_i64() {
                Some(i)
            } else if let Some(u) = n.as_u64() {
                i64::try_from(u).ok()
            } else {
                None
            }
        }
        Some(Value::String(s)) => {
            let t = s.trim();
            // 整数リテラルのみ（"1.5"/"" は Number.isInteger で false）。
            t.parse::<i64>().ok()
        }
        _ => None,
    }
}

/// `typeof x === "string" ? x.trim() : ""`（Node の name/endpointUrl 取り出し）。
fn trimmed_string(v: Option<&Value>) -> String {
    v.and_then(Value::as_str).unwrap_or("").trim().to_owned()
}

// ─── GET /api/mcp-servers（一覧） ─────────────────────────────────────────────

pub(crate) async fn list(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<McpRuntime>>,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let uid = &user.0.discord_id;
    let admin = repo::is_admin(&state.db, uid).await?;
    let own = repo::list_servers_for_owner(&state.db, uid).await?;
    let system = repo::list_system_servers(&state.db).await?;
    let oauth_ids = repo::list_oauth_server_ids(&state.db).await?;

    let mut servers: Vec<Value> = Vec::with_capacity(own.len() + system.len());
    for (s, is_own) in own
        .iter()
        .map(|s| (s, true))
        .chain(system.iter().map(|s| (s, false)))
    {
        let mut view = to_safe_view(s);
        let bot_ids = if is_own {
            repo::list_bot_ids_for_server(&state.db, s.id).await?
        } else {
            Vec::new()
        };
        let oauth_connected = oauth_ids.contains(&s.id);
        let mode = if s.has_auth() {
            "bearer"
        } else if oauth_connected {
            "oauth"
        } else {
            "none"
        };
        if let Some(obj) = view.as_object_mut() {
            obj.insert("granted_bot_ids".to_owned(), json!(bot_ids));
            obj.insert(
                "can_manage".to_owned(),
                json!(repo::can_manage(s, uid, admin)),
            );
            obj.insert(
                "auth".to_owned(),
                json!({
                    "mode": mode,
                    "oauth_connected": oauth_connected,
                    "authorization_required": rt.client.authorization_required(s.id).await,
                }),
            );
        }
        servers.push(view);
    }
    Ok((
        StatusCode::OK,
        Json(json!({
            "success": true,
            "servers": servers,
            "apps": { "sandbox": sandbox_info(&state) },
        })),
    )
        .into_response())
}

// ─── POST /api/mcp-servers/add（追加・scope:"system" は Admin のみ） ────────────

pub(crate) async fn add(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<McpRuntime>>,
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let name = trimmed_string(body.get("name"));
    let endpoint_url = trimmed_string(body.get("endpointUrl"));
    // authCredential は「文字列のときのみ」採用（Node `typeof === "string"`）。
    let auth_credential = body.get("authCredential").and_then(Value::as_str);
    // requiresConfirmation は `!== false`（既定 true・Node）。
    let requires_confirmation = body.get("requiresConfirmation") != Some(&Value::Bool(false));
    // scope は "system" のときだけ system、その他は user。
    let is_system = body.get("scope").and_then(Value::as_str) == Some("system");

    if name.is_empty() || endpoint_url.is_empty() {
        return Ok(err_json(
            StatusCode::BAD_REQUEST,
            "name と endpointUrl は必須です。",
        ));
    }
    // Node の SSRF assertSafeOutboundUrl は外部シーム（本 crate では未検証・実クライアント側で行う）。
    // ここでは Node のもう 1 つの入口検証（system かつ非 admin）を先に効かせる。
    let admin = repo::is_admin(&state.db, &user.0.discord_id).await?;
    if is_system && !admin {
        return Ok(err_json(
            StatusCode::FORBIDDEN,
            "システムレベルのMCPサーバー登録はAdminのみ可能です。",
        ));
    }

    // 認証情報を暗号化する（空/非文字列は暗号化しない・Node `addServer` は trim して空なら null）。
    let enc = match auth_credential.map(str::trim).filter(|s| !s.is_empty()) {
        Some(cred) => {
            let Some(crypto) = rt.crypto.as_ref() else {
                // 暗号鍵無しで認証情報を保存できない → 500（Node は起動時に鍵必須）。
                return Err(ApiError(yuuka_core::WebError::Internal));
            };
            let e = crypto
                .encrypt_text(cred)
                .map_err(|_| ApiError(yuuka_core::WebError::Internal))?;
            Some((e.encrypted, e.iv, e.auth_tag))
        }
        None => None,
    };

    let owner = if is_system {
        None
    } else {
        Some(user.0.discord_id.clone())
    };
    let server = repo::add_server(
        &state.db,
        owner,
        &name,
        &endpoint_url,
        enc,
        requires_confirmation,
    )
    .await?;

    if is_system {
        yuuka_auth::audit::add_audit_log(
            &state.db,
            &user.0.discord_id,
            "admin.mcp_add",
            Some(&name),
            Some(&endpoint_url),
        )
        .await;
    }

    // 追加後に tools/list を試行してツール数を返す（Node §4.4.2 手順2）。
    let tools_message = match rt.client.refresh_tools(&server).await {
        Ok(tools) => {
            let json_str = tools_to_cache_json(&tools);
            repo::update_tools_cache(&state.db, server.id, &json_str).await?;
            format!("提供Tool {} 件を取得しました。", tools.len())
        }
        Err(e) => {
            format!("登録しましたが tools/list の取得に失敗しました: {e}")
        }
    };

    // 最新行を読み直して safe view を返す（Node `getServerById(server.id)!`）。
    let fresh = repo::get_server_by_id(&state.db, server.id)
        .await?
        .unwrap_or(server);
    Ok((
        StatusCode::OK,
        Json(json!({
            "success": true,
            "server": to_safe_view(&fresh),
            "message": format!("MCPサーバー「{name}」を登録しました。{tools_message}"),
        })),
    )
        .into_response())
}

// ─── POST /api/mcp-servers/refresh（ツール一覧の再取得） ───────────────────────

pub(crate) async fn refresh(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<McpRuntime>>,
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let Some(id) = parse_body_id(body.get("id")) else {
        return Ok(err_json(
            StatusCode::NOT_FOUND,
            "MCPサーバーが見つかりません。",
        ));
    };
    let admin = repo::is_admin(&state.db, &user.0.discord_id).await?;
    let server = repo::get_server_by_id(&state.db, id).await?;
    let Some(server) = server.filter(|s| repo::can_manage(s, &user.0.discord_id, admin)) else {
        return Ok(err_json(
            StatusCode::NOT_FOUND,
            "MCPサーバーが見つかりません。",
        ));
    };

    match rt.client.refresh_tools(&server).await {
        Ok(tools) => {
            let json_str = tools_to_cache_json(&tools);
            repo::update_tools_cache(&state.db, id, &json_str).await?;
            let fresh = repo::get_server_by_id(&state.db, id)
                .await?
                .unwrap_or(server);
            Ok((
                StatusCode::OK,
                Json(json!({
                    "success": true,
                    "server": to_safe_view(&fresh),
                    "message": format!("Toolキャッシュを更新しました ({}件)。", tools.len()),
                })),
            )
                .into_response())
        }
        Err(e) => Ok(err_json(
            StatusCode::BAD_GATEWAY,
            &format!("tools/list の取得に失敗しました: {e}"),
        )),
    }
}

// ─── POST /api/mcp-servers/toggle（有効/無効） ─────────────────────────────────

pub(crate) async fn toggle(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<McpRuntime>>,
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let Some(id) = parse_body_id(body.get("id")) else {
        return Ok(err_json(
            StatusCode::NOT_FOUND,
            "MCPサーバーが見つかりません。",
        ));
    };
    // Node `ctx.body.enabled === true`（真偽値 true のみ enabled）。
    let enabled = body.get("enabled") == Some(&Value::Bool(true));
    let admin = repo::is_admin(&state.db, &user.0.discord_id).await?;
    let server = repo::get_server_by_id(&state.db, id).await?;
    let Some(_server) = server.filter(|s| repo::can_manage(s, &user.0.discord_id, admin)) else {
        return Ok(err_json(
            StatusCode::NOT_FOUND,
            "MCPサーバーが見つかりません。",
        ));
    };
    repo::set_enabled(&state.db, id, enabled).await?;
    if !enabled {
        rt.client.forget(id).await;
    }
    let message = if enabled {
        "MCPサーバーを有効化しました。"
    } else {
        "MCPサーバーを無効化しました。"
    };
    Ok((
        StatusCode::OK,
        Json(json!({ "success": true, "message": message })),
    )
        .into_response())
}

// ─── POST /api/mcp-servers/delete（削除） ──────────────────────────────────────

pub(crate) async fn delete(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<McpRuntime>>,
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let Some(id) = parse_body_id(body.get("id")) else {
        return Ok(err_json(StatusCode::BAD_REQUEST, "id は必須です。"));
    };
    // 削除前に行を引いておく（system 登録なら監査に name が要る・Node と同順）。
    let admin = repo::is_admin(&state.db, &user.0.discord_id).await?;
    let server = repo::get_server_by_id(&state.db, id).await?;
    let ok = repo::delete_server(&state.db, id, &user.0.discord_id, admin).await?;
    if ok {
        rt.client.forget(id).await;
        if let Some(s) = &server {
            if s.user_id.is_none() {
                yuuka_auth::audit::add_audit_log(
                    &state.db,
                    &user.0.discord_id,
                    "admin.mcp_delete",
                    Some(&s.name),
                    None,
                )
                .await;
            }
        }
    }
    let message = if ok {
        "MCPサーバーを削除しました。"
    } else {
        "MCPサーバーが見つからないか、削除権限がありません。"
    };
    Ok((
        StatusCode::OK,
        Json(json!({ "success": ok, "message": message })),
    )
        .into_response())
}

// ─── 共通: パスの :id から管理可能なサーバーを引く ─────────────────────────────

/// `:id` パス値から number へ、行取得・canManage まで通す共通前段（不成立は `Ok(None)`）。
async fn resolve_manageable(
    state: &AppState,
    user: &AuthenticatedUser,
    id_str: &str,
) -> Result<Option<McpServerRecord>, ApiError> {
    let Some(id) = id_str.trim().parse::<i64>().ok() else {
        return Ok(None);
    };
    let admin = repo::is_admin(&state.db, &user.0.discord_id).await?;
    let server = repo::get_server_by_id(&state.db, id).await?;
    Ok(server.filter(|s| repo::can_manage(s, &user.0.discord_id, admin)))
}

fn not_found() -> Response {
    err_json(StatusCode::NOT_FOUND, "MCPサーバーが見つかりません。")
}

// ─── OAuth 2.1 認可（MCP 仕様 Authorization） ─────────────────────────────────

/// `BASE_URL` のオリジン。OAuth のコールバック URL に使うため、未設定なら `None`（Host ヘッダからは
/// 組み立てない：認可サーバーに登録する URL を偽装されないため）。
fn configured_origin(state: &AppState) -> Option<String> {
    state
        .config
        .base_url
        .as_deref()
        .and_then(sandbox::url_origin)
}

/// `POST /api/mcp-servers/:id/oauth/start` — 認可 URL を返す（フロントはそこへ遷移する）。
///
/// body（任意）: `{ clientId, clientSecret }`（認可サーバーに事前登録したクライアントを使う場合）。
pub(crate) async fn oauth_start(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<McpRuntime>>,
    State(state): State<AppState>,
    Path(id_str): Path<String>,
    body: Option<Json<Value>>,
) -> Result<Response, ApiError> {
    let Some(server) = resolve_manageable(&state, &user, &id_str).await? else {
        return Ok(not_found());
    };
    let Some(origin) = configured_origin(&state) else {
        return Ok(err_json(
            StatusCode::BAD_REQUEST,
            "OAuth 認可には BASE_URL の設定が必要です。",
        ));
    };
    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let client_id = Some(trimmed_string(body.get("clientId"))).filter(|s| !s.is_empty());
    let client_secret = Some(trimmed_string(body.get("clientSecret"))).filter(|s| !s.is_empty());
    let start = AuthorizationStart {
        user_id: user.0.discord_id.clone(),
        redirect_uri: format!("{origin}{}", oauth::CALLBACK_PATH),
        // Client ID Metadata Document は HTTPS で公開しているときだけ使える（SEP-991）。
        client_metadata_url: origin
            .starts_with("https://")
            .then(|| format!("{origin}{}", oauth::CLIENT_METADATA_PATH)),
        client_id,
        client_secret,
    };
    match rt.client.start_authorization(&server, start).await {
        Ok(url) => Ok((
            StatusCode::OK,
            Json(json!({ "success": true, "authorizationUrl": url })),
        )
            .into_response()),
        Err(e) => Ok(err_json(StatusCode::BAD_GATEWAY, &e)),
    }
}

/// `GET /api/mcp-servers/oauth/callback` — 認可サーバーから戻ってくる先（ブラウザの遷移）。
/// 結果をクエリに付けて管理画面（`/integrated/mcp`）へ戻す。
pub(crate) async fn oauth_callback(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<McpRuntime>>,
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let back = |result: &str, message: &str| {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("mcp_oauth", result)
            .append_pair("message", message)
            .finish();
        let location = format!("/integrated/mcp?{query}");
        (
            StatusCode::SEE_OTHER,
            [(LOCATION, location), (CACHE_CONTROL, "no-store".to_owned())],
        )
            .into_response()
    };
    if let Some(error) = q.get("error") {
        // 認可サーバーが返した文言はそのまま画面に出るので短く切る（表示側でエスケープされる）。
        let error: String = error.chars().take(100).collect();
        let detail = q.get("error_description").map_or(String::new(), |d| {
            format!("（{}）", d.chars().take(200).collect::<String>())
        });
        return back("error", &format!("認可されませんでした: {error}{detail}"));
    }
    let (Some(code), Some(csrf_state)) = (q.get("code"), q.get("state")) else {
        return back("error", "認可サーバーからの応答が不正です。");
    };
    let callback = AuthorizationCallback {
        code: code.clone(),
        state: csrf_state.clone(),
        issuer: q.get("iss").cloned(),
        user_id: user.0.discord_id.clone(),
    };
    let server_id = match rt.client.complete_authorization(callback).await {
        Ok(id) => id,
        Err(e) => return back("error", &e),
    };
    // 新しい資格情報で tools/list を取り直す（失敗しても認可自体は完了している）。
    let server = repo::get_server_by_id(&state.db, server_id)
        .await
        .ok()
        .flatten();
    let Some(server) = server else {
        return back("success", "認可が完了しました。");
    };
    let tools_message = match rt.client.refresh_tools(&server).await {
        Ok(tools) => {
            let json_str = tools_to_cache_json(&tools);
            match repo::update_tools_cache(&state.db, server.id, &json_str).await {
                Ok(()) => format!("提供Tool {} 件を取得しました。", tools.len()),
                Err(e) => format!("Tool一覧を保存できませんでした: {e}"),
            }
        }
        Err(e) => format!("Tool一覧の取得に失敗しました: {e}"),
    };
    back(
        "success",
        &format!(
            "MCPサーバー「{}」の認可が完了しました。{tools_message}",
            server.name
        ),
    )
}

/// `GET /api/mcp-servers/oauth/client-metadata.json`（認証なし）— Client ID Metadata Document。
pub(crate) async fn oauth_client_metadata(State(state): State<AppState>) -> Response {
    match configured_origin(&state) {
        Some(origin) if origin.starts_with("https://") => (
            StatusCode::OK,
            [(CACHE_CONTROL, "public, max-age=3600")],
            Json(oauth::client_metadata_document(&origin)),
        )
            .into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `POST /api/mcp-servers/:id/oauth/disconnect` — 保存した OAuth 資格情報を消す。
pub(crate) async fn oauth_disconnect(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<McpRuntime>>,
    State(state): State<AppState>,
    Path(id_str): Path<String>,
) -> Result<Response, ApiError> {
    let Some(server) = resolve_manageable(&state, &user, &id_str).await? else {
        return Ok(not_found());
    };
    repo::delete_oauth_credentials(&state.db, server.id).await?;
    rt.client.forget(server.id).await;
    Ok((
        StatusCode::OK,
        Json(json!({ "success": true, "message": "OAuth の認可を解除しました。" })),
    )
        .into_response())
}

// ─── MCP Apps ─────────────────────────────────────────────────────────────────

/// UI リソースの HTML の上限（巨大な応答で管理画面を固めない）。
const MAX_UI_HTML_BYTES: usize = 5 * 1024 * 1024;

/// tools_cache から名前でツールを引く。
fn cached_tool(server: &McpServerRecord, name: &str) -> Option<McpTool> {
    repo::parse_tools_cache_full(server)
        .into_iter()
        .find(|t| t.name == name)
}

/// `ReadResourceResult` から MCP Apps の HTML と、リソースの `_meta.ui` を取り出す。
fn ui_html_from(result: &Value, uri: &str) -> Result<(String, Value), String> {
    let contents = result
        .get("contents")
        .and_then(Value::as_array)
        .ok_or_else(|| "UI リソースの内容がありません。".to_owned())?;
    let item = contents
        .iter()
        .find(|c| c.get("uri").and_then(Value::as_str) == Some(uri))
        .or_else(|| contents.first())
        .ok_or_else(|| "UI リソースの内容がありません。".to_owned())?;
    let mime = item.get("mimeType").and_then(Value::as_str).unwrap_or("");
    // `text/html;profile=mcp-app`（空白の有無・大小は揺れを許す）。
    let normalized: String = mime
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    if normalized != crate::UI_RESOURCE_MIME_TYPE {
        return Err(format!(
            "UI リソースの種類が MCP Apps ではありません（{mime}）。"
        ));
    }
    let html = if let Some(text) = item.get("text").and_then(Value::as_str) {
        text.to_owned()
    } else if let Some(blob) = item.get("blob").and_then(Value::as_str) {
        let bytes =
            decode_base64(blob).ok_or_else(|| "UI リソースを復号できません。".to_owned())?;
        String::from_utf8(bytes).map_err(|_| "UI リソースが UTF-8 ではありません。".to_owned())?
    } else {
        return Err("UI リソースの内容がありません。".to_owned());
    };
    if html.len() > MAX_UI_HTML_BYTES {
        return Err("UI リソースが大きすぎます。".to_owned());
    }
    let meta = item
        .get("_meta")
        .and_then(|m| m.get("ui"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    Ok((html, meta))
}

/// 標準 base64（パディングあり・なし両対応）を復号する。
fn decode_base64(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some(u32::from(c - b'A')),
            b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
            b'+' | b'-' => Some(62),
            b'/' | b'_' => Some(63),
            _ => None,
        }
    }
    let clean: Vec<u8> = s
        .bytes()
        .filter(|b| !b.is_ascii_whitespace() && *b != b'=')
        .collect();
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);
    for chunk in clean.chunks(4) {
        let mut acc = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            acc |= val(c)? << (18 - 6 * i);
        }
        let bytes = acc.to_be_bytes();
        match chunk.len() {
            4 => out.extend_from_slice(&bytes[1..4]),
            3 => out.extend_from_slice(&bytes[1..3]),
            2 => out.push(bytes[1]),
            _ => return None,
        }
    }
    Some(out)
}

/// サンドボックスプロキシの URL と、別オリジンで隔離しているか。
fn sandbox_info(state: &AppState) -> Value {
    match state
        .config
        .mcp_sandbox_origin
        .as_deref()
        .and_then(sandbox::url_origin)
    {
        Some(origin) => json!({
            "url": format!("{origin}{}", sandbox::SANDBOX_PATH),
            "origin": origin,
            "isolated": true,
        }),
        None => json!({ "url": sandbox::SANDBOX_PATH, "origin": Value::Null, "isolated": false }),
    }
}

/// `POST /api/mcp-servers/:id/apps/resource` — ツールに紐づく UI リソース（HTML と `_meta.ui`）を返す。
///
/// body: `{ tool }`（tools_cache のツール名）または `{ uri }`（画面からの `resources/read` 用）。
/// `tool` のときは、そのツールの `_meta.ui.resourceUri` を読む。
pub(crate) async fn apps_resource(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<McpRuntime>>,
    State(state): State<AppState>,
    Path(id_str): Path<String>,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let Some(server) = resolve_manageable(&state, &user, &id_str).await? else {
        return Ok(not_found());
    };
    if server.enabled != 1 {
        return Ok(err_json(
            StatusCode::CONFLICT,
            "MCPサーバーが無効化されています。",
        ));
    }
    // 画面からの resources/read：ReadResourceResult をそのまま返す。
    if let Some(uri) = body.get("uri").and_then(Value::as_str) {
        return Ok(match rt.client.read_resource(&server, uri).await {
            Ok(result) => (
                StatusCode::OK,
                Json(json!({ "success": true, "result": result })),
            )
                .into_response(),
            Err(e) => err_json(StatusCode::BAD_GATEWAY, &e),
        });
    }
    let tool_name = trimmed_string(body.get("tool"));
    let Some(tool) = cached_tool(&server, &tool_name) else {
        return Ok(err_json(StatusCode::NOT_FOUND, "ツールが見つかりません。"));
    };
    let Some(uri) = tool.ui_resource_uri() else {
        return Ok(err_json(
            StatusCode::BAD_REQUEST,
            "このツールには画面（MCP Apps）がありません。",
        ));
    };
    let result = match rt.client.read_resource(&server, uri).await {
        Ok(r) => r,
        Err(e) => {
            return Ok(err_json(
                StatusCode::BAD_GATEWAY,
                &format!("UI リソースの取得に失敗しました: {e}"),
            ))
        }
    };
    match ui_html_from(&result, uri) {
        Ok((html, ui)) => Ok((
            StatusCode::OK,
            Json(json!({
                "success": true,
                "html": html,
                "ui": ui,
                "tool": {
                    "name": tool.name,
                    "description": tool.description,
                    "inputSchema": tool.input_schema,
                    "_meta": { "ui": tool.ui },
                },
                "sandbox": sandbox_info(&state),
            })),
        )
            .into_response()),
        Err(e) => Ok(err_json(StatusCode::BAD_GATEWAY, &e)),
    }
}

/// `POST /api/mcp-servers/:id/apps/call` — `tools/call` を実行して `CallToolResult` を返す。
///
/// body: `{ name, arguments, from }`。`from` は `"host"`（管理画面が画面を開くときの呼び出し）か
/// `"app"`（画面の中からの呼び出し）。`"app"` は `_meta.ui.visibility` に `"app"` があるツールだけ、
/// `"host"` は画面（`resourceUri`）を持つツールだけ受け付ける。
pub(crate) async fn apps_call(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<McpRuntime>>,
    State(state): State<AppState>,
    Path(id_str): Path<String>,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let Some(server) = resolve_manageable(&state, &user, &id_str).await? else {
        return Ok(not_found());
    };
    if server.enabled != 1 {
        return Ok(err_json(
            StatusCode::CONFLICT,
            "MCPサーバーが無効化されています。",
        ));
    }
    let name = trimmed_string(body.get("name"));
    let Some(tool) = cached_tool(&server, &name) else {
        return Ok(err_json(StatusCode::NOT_FOUND, "ツールが見つかりません。"));
    };
    let from_app = body.get("from").and_then(Value::as_str) == Some("app");
    let allowed = if from_app {
        tool.callable_from_app()
    } else {
        tool.ui_resource_uri().is_some()
    };
    if !allowed {
        return Ok(err_json(
            StatusCode::FORBIDDEN,
            "このツールは画面から呼び出せません。",
        ));
    }
    let arguments = body.get("arguments").cloned().unwrap_or(Value::Null);
    match rt.client.call_tool_result(&server, &name, arguments).await {
        Ok(result) => Ok((
            StatusCode::OK,
            Json(json!({ "success": true, "result": result })),
        )
            .into_response()),
        Err(e) => Ok(err_json(StatusCode::BAD_GATEWAY, &e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_body_id_semantics() {
        assert_eq!(parse_body_id(Some(&json!(5))), Some(5));
        assert_eq!(parse_body_id(Some(&json!("7"))), Some(7));
        // 小数は Number.isInteger で false。
        assert_eq!(parse_body_id(Some(&json!(5.5))), None);
        assert_eq!(parse_body_id(Some(&json!("abc"))), None);
        assert_eq!(parse_body_id(Some(&json!(""))), None);
        assert_eq!(parse_body_id(None), None);
    }

    // ── ルート統合テスト（oneshot・FakeAuth・NullMcpClient・seeded temp DB） ─────

    mod routes_it {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::Arc;

        use axum::body::{to_bytes, Body};
        use axum::http::{Request, StatusCode};
        use serde_json::Value;
        use tower::ServiceExt;
        use yuuka_core::AuthError;
        use yuuka_types::{Role, SessionUser};
        use yuuka_web::{AppState, AuthBackend, Db, WebConfig};

        use crate::{routes, McpRuntime, NullMcpClient};

        static SEQ: AtomicU64 = AtomicU64::new(0);

        /// サーバー 1 の tools_cache: 画面付き（dash）・画面専用（save）・画面なし（plain）・
        /// AI 専用の画面付き（model_only）。
        const TOOLS: &str = r#"[
            {"name":"dash","_meta":{"ui":{"resourceUri":"ui://a/dash"}}},
            {"name":"save","_meta":{"ui":{"resourceUri":"ui://a/dash","visibility":["app"]}}},
            {"name":"plain","description":"no ui"},
            {"name":"model_only","_meta":{"ui":{"resourceUri":"ui://a/dash","visibility":["model"]}}}
        ]"#;

        struct FakeAuth;

        #[async_trait::async_trait]
        impl AuthBackend for FakeAuth {
            async fn session_user(&self, token: &str) -> Result<Option<SessionUser>, AuthError> {
                let user = match token {
                    "alice" => Some(("alice", Role::User)),
                    "root" => Some(("root", Role::Admin)),
                    _ => None,
                };
                Ok(user.map(|(id, role)| SessionUser {
                    discord_id: id.to_owned(),
                    username: id.to_owned(),
                    role,
                }))
            }
            async fn desktop_user(&self, _token: &str) -> Result<Option<SessionUser>, AuthError> {
                Ok(None)
            }
        }

        fn seed_db() -> Db {
            let seq = SEQ.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "yuuka_mcp_route_test_{}_{seq}.sqlite",
                std::process::id()
            ));
            let _ = std::fs::remove_file(&path);
            drop(rusqlite::Connection::open(&path).expect("create empty"));
            let db = Db::open(&path).expect("open");
            {
                let conn = rusqlite::Connection::open(&path).expect("seed conn");
                conn.execute(
                    "INSERT INTO users (discord_id, username, salt, role) \
                     VALUES ('alice','alice','deadbeef','user'),('root','root','deadbeef','admin')",
                    [],
                )
                .expect("seed users");
                // alice 所有のサーバー 1 件（enabled）。
                conn.execute(
                    "INSERT INTO mcp_servers (id, user_id, name, endpoint_url, tools_cache) \
                     VALUES (1, 'alice', 'a', 'https://x/mcp', ?1)",
                    [TOOLS],
                )
                .expect("seed server");
            }
            db
        }

        fn app() -> axum::Router {
            app_with(WebConfig::default())
        }

        fn app_with(config: WebConfig) -> axum::Router {
            let state = AppState::new(Arc::new(FakeAuth), config, seed_db());
            let rt = Arc::new(McpRuntime::new(None, Arc::new(NullMcpClient)));
            routes(rt).with_state(state)
        }

        async fn get(app: &axum::Router, uri: &str, token: &str) -> (StatusCode, Value) {
            let mut b = Request::builder().method("GET").uri(uri);
            if !token.is_empty() {
                b = b.header("cookie", format!("__Host-yuuka-session={token}"));
            }
            let resp = app
                .clone()
                .oneshot(b.body(Body::empty()).unwrap())
                .await
                .unwrap();
            let status = resp.status();
            let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            (status, json)
        }

        async fn post(
            app: &axum::Router,
            uri: &str,
            token: &str,
            body: &str,
        ) -> (StatusCode, Value) {
            let mut b = Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json");
            if !token.is_empty() {
                b = b.header("cookie", format!("__Host-yuuka-session={token}"));
            }
            let resp = app
                .clone()
                .oneshot(b.body(Body::from(body.to_owned())).unwrap())
                .await
                .unwrap();
            let status = resp.status();
            let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            (status, json)
        }

        #[tokio::test]
        async fn list_requires_auth_and_returns_own_plus_system() {
            let app = app();
            // 未ログイン → 401。
            let (st, _) = get(&app, "/api/mcp-servers", "").await;
            assert_eq!(st, StatusCode::UNAUTHORIZED);
            // alice → 自分の 1 件（granted_bot_ids 付き）。
            let (st, j) = get(&app, "/api/mcp-servers", "alice").await;
            assert_eq!(st, StatusCode::OK);
            assert_eq!(j["success"], Value::Bool(true));
            let servers = j["servers"].as_array().unwrap();
            assert_eq!(servers.len(), 1);
            assert_eq!(servers[0]["scope"], Value::String("user".to_owned()));
            assert!(servers[0]["granted_bot_ids"].is_array());
        }

        #[tokio::test]
        async fn add_null_client_returns_refresh_failure_message() {
            let app = app();
            let (st, j) = post(
                &app,
                "/api/mcp-servers/add",
                "alice",
                r#"{"name":"MyMcp","endpointUrl":"https://svc/mcp"}"#,
            )
            .await;
            assert_eq!(st, StatusCode::OK);
            assert_eq!(j["success"], Value::Bool(true));
            // NullMcpClient は refresh_tools が Err → 「登録しましたが…」メッセージ。
            let msg = j["message"].as_str().unwrap();
            assert!(msg.starts_with("MCPサーバー「MyMcp」を登録しました。登録しましたが tools/list の取得に失敗しました: "));
            assert_eq!(j["server"]["name"], Value::String("MyMcp".to_owned()));
        }

        #[tokio::test]
        async fn add_missing_fields_400() {
            let app = app();
            let (st, j) = post(&app, "/api/mcp-servers/add", "alice", r#"{"name":"x"}"#).await;
            assert_eq!(st, StatusCode::BAD_REQUEST);
            assert_eq!(
                j["message"],
                Value::String("name と endpointUrl は必須です。".to_owned())
            );
        }

        #[tokio::test]
        async fn add_system_scope_requires_admin() {
            let app = app();
            let (st, j) = post(
                &app,
                "/api/mcp-servers/add",
                "alice",
                r#"{"name":"sys","endpointUrl":"https://s/mcp","scope":"system"}"#,
            )
            .await;
            assert_eq!(st, StatusCode::FORBIDDEN);
            assert_eq!(
                j["message"],
                Value::String("システムレベルのMCPサーバー登録はAdminのみ可能です。".to_owned())
            );
        }

        #[tokio::test]
        async fn refresh_null_client_502() {
            let app = app();
            let (st, j) = post(&app, "/api/mcp-servers/refresh", "alice", r#"{"id":1}"#).await;
            assert_eq!(st, StatusCode::BAD_GATEWAY);
            let msg = j["message"].as_str().unwrap();
            assert!(msg.starts_with("tools/list の取得に失敗しました: "));
        }

        #[tokio::test]
        async fn refresh_not_owner_404() {
            let app = app();
            // root は alice 所有サーバーを canManage できない（admin でも本人所有には手を出せない）→ 404。
            let (st, j) = post(&app, "/api/mcp-servers/refresh", "root", r#"{"id":1}"#).await;
            assert_eq!(st, StatusCode::NOT_FOUND);
            assert_eq!(
                j["message"],
                Value::String("MCPサーバーが見つかりません。".to_owned())
            );
        }

        #[tokio::test]
        async fn toggle_and_delete_flow() {
            let app = app();
            // 無効化。
            let (st, j) = post(
                &app,
                "/api/mcp-servers/toggle",
                "alice",
                r#"{"id":1,"enabled":false}"#,
            )
            .await;
            assert_eq!(st, StatusCode::OK);
            assert_eq!(
                j["message"],
                Value::String("MCPサーバーを無効化しました。".to_owned())
            );
            // 有効化。
            let (st, j) = post(
                &app,
                "/api/mcp-servers/toggle",
                "alice",
                r#"{"id":1,"enabled":true}"#,
            )
            .await;
            assert_eq!(st, StatusCode::OK);
            assert_eq!(
                j["message"],
                Value::String("MCPサーバーを有効化しました。".to_owned())
            );
            // 削除。
            let (st, j) = post(&app, "/api/mcp-servers/delete", "alice", r#"{"id":1}"#).await;
            assert_eq!(st, StatusCode::OK);
            assert_eq!(j["success"], Value::Bool(true));
            assert_eq!(
                j["message"],
                Value::String("MCPサーバーを削除しました。".to_owned())
            );
            // 二重削除 → success:false。
            let (st, j) = post(&app, "/api/mcp-servers/delete", "alice", r#"{"id":1}"#).await;
            assert_eq!(st, StatusCode::OK);
            assert_eq!(j["success"], Value::Bool(false));
        }

        #[tokio::test]
        async fn delete_missing_id_400() {
            let app = app();
            let (st, j) = post(&app, "/api/mcp-servers/delete", "alice", r#"{}"#).await;
            assert_eq!(st, StatusCode::BAD_REQUEST);
            assert_eq!(j["message"], Value::String("id は必須です。".to_owned()));
        }

        // ── OAuth 認可 ────────────────────────────────────────────────────────

        #[tokio::test]
        async fn list_reports_auth_state_ui_tools_and_sandbox() {
            let app = app();
            let (st, j) = get(&app, "/api/mcp-servers", "alice").await;
            assert_eq!(st, StatusCode::OK);
            let s = &j["servers"][0];
            assert_eq!(s["auth"]["mode"], "none");
            assert_eq!(s["auth"]["oauth_connected"], false);
            assert_eq!(s["can_manage"], true);
            let tools = s["tools"].as_array().unwrap();
            assert_eq!(tools[0]["ui_resource_uri"], "ui://a/dash");
            assert_eq!(tools[1]["visible_to_model"], false);
            assert_eq!(tools[2]["ui_resource_uri"], Value::Null);
            // MCP_SANDBOX_ORIGIN 未設定 → 本体の不透明オリジン。
            assert_eq!(j["apps"]["sandbox"]["isolated"], false);
            assert_eq!(j["apps"]["sandbox"]["url"], "/mcp-apps/sandbox");

            let app = app_with(WebConfig {
                mcp_sandbox_origin: Some("https://mcp-sandbox.example".to_owned()),
                ..WebConfig::default()
            });
            let (_, j) = get(&app, "/api/mcp-servers", "alice").await;
            assert_eq!(j["apps"]["sandbox"]["isolated"], true);
            assert_eq!(
                j["apps"]["sandbox"]["url"],
                "https://mcp-sandbox.example/mcp-apps/sandbox"
            );
        }

        #[tokio::test]
        async fn oauth_start_needs_base_url_and_ownership() {
            let app = app();
            // BASE_URL 未設定 → コールバック URL を作れない。
            let (st, j) = post(&app, "/api/mcp-servers/1/oauth/start", "alice", "{}").await;
            assert_eq!(st, StatusCode::BAD_REQUEST);
            assert!(j["message"].as_str().unwrap().contains("BASE_URL"));

            let app = app_with(WebConfig {
                base_url: Some("https://yuuka.example".to_owned()),
                ..WebConfig::default()
            });
            // 所有者以外 → 404。
            let (st, _) = post(&app, "/api/mcp-servers/1/oauth/start", "root", "{}").await;
            assert_eq!(st, StatusCode::NOT_FOUND);
            // 所有者 → NullMcpClient なので 502（認可サーバーへ到達しない）。
            let (st, _) = post(&app, "/api/mcp-servers/1/oauth/start", "alice", "{}").await;
            assert_eq!(st, StatusCode::BAD_GATEWAY);
        }

        async fn location_of(app: &axum::Router, uri: &str) -> (StatusCode, String) {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(uri)
                        .header("cookie", "__Host-yuuka-session=alice")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let location = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            (resp.status(), location)
        }

        #[tokio::test]
        async fn oauth_callback_redirects_back_with_result() {
            let app = app();
            let (st, location) =
                location_of(&app, "/api/mcp-servers/oauth/callback?error=access_denied").await;
            assert_eq!(st, StatusCode::SEE_OTHER);
            assert!(location.starts_with("/integrated/mcp?mcp_oauth=error"));
            assert!(location.contains("access_denied"));

            // 未知の state → エラーで戻す（NullMcpClient）。
            let (st, location) =
                location_of(&app, "/api/mcp-servers/oauth/callback?code=c&state=s").await;
            assert_eq!(st, StatusCode::SEE_OTHER);
            assert!(location.contains("mcp_oauth=error"));
        }

        #[tokio::test]
        async fn client_metadata_only_on_https() {
            let (st, _) = get(&app(), "/api/mcp-servers/oauth/client-metadata.json", "").await;
            assert_eq!(st, StatusCode::NOT_FOUND);
            let app = app_with(WebConfig {
                base_url: Some("https://yuuka.example/".to_owned()),
                ..WebConfig::default()
            });
            let (st, j) = get(&app, "/api/mcp-servers/oauth/client-metadata.json", "").await;
            assert_eq!(st, StatusCode::OK);
            assert_eq!(
                j["client_id"],
                "https://yuuka.example/api/mcp-servers/oauth/client-metadata.json"
            );
        }

        #[tokio::test]
        async fn oauth_disconnect_ok_for_owner() {
            let app = app();
            let (st, j) = post(&app, "/api/mcp-servers/1/oauth/disconnect", "alice", "").await;
            assert_eq!(st, StatusCode::OK);
            assert_eq!(j["success"], true);
            let (st, _) = post(&app, "/api/mcp-servers/1/oauth/disconnect", "root", "").await;
            assert_eq!(st, StatusCode::NOT_FOUND);
        }

        // ── MCP Apps ──────────────────────────────────────────────────────────

        #[tokio::test]
        async fn apps_resource_checks_tool_has_ui() {
            let app = app();
            let url = "/api/mcp-servers/1/apps/resource";
            let (st, _) = post(&app, url, "alice", r#"{"tool":"missing"}"#).await;
            assert_eq!(st, StatusCode::NOT_FOUND);
            let (st, _) = post(&app, url, "alice", r#"{"tool":"plain"}"#).await;
            assert_eq!(st, StatusCode::BAD_REQUEST);
            // 画面付き → NullMcpClient なので取得失敗 502。
            let (st, _) = post(&app, url, "alice", r#"{"tool":"dash"}"#).await;
            assert_eq!(st, StatusCode::BAD_GATEWAY);
            // 他人のサーバー → 404。
            let (st, _) = post(&app, url, "root", r#"{"tool":"dash"}"#).await;
            assert_eq!(st, StatusCode::NOT_FOUND);
        }

        #[tokio::test]
        async fn apps_call_enforces_visibility() {
            let app = app();
            let url = "/api/mcp-servers/1/apps/call";
            // 画面からの呼び出しは visibility に "app" が要る。
            let (st, _) = post(&app, url, "alice", r#"{"name":"model_only","from":"app"}"#).await;
            assert_eq!(st, StatusCode::FORBIDDEN);
            // ホスト（画面を開く）からは画面付きツールだけ。
            let (st, _) = post(&app, url, "alice", r#"{"name":"plain","from":"host"}"#).await;
            assert_eq!(st, StatusCode::FORBIDDEN);
            // 許可された呼び出し → NullMcpClient なので 502。
            let (st, _) = post(&app, url, "alice", r#"{"name":"save","from":"app"}"#).await;
            assert_eq!(st, StatusCode::BAD_GATEWAY);
            let (st, _) = post(&app, url, "alice", r#"{"name":"dash","from":"host"}"#).await;
            assert_eq!(st, StatusCode::BAD_GATEWAY);
        }

        #[tokio::test]
        async fn sandbox_page_has_own_csp_and_host_origin() {
            let app = app_with(WebConfig {
                base_url: Some("https://yuuka.example".to_owned()),
                ..WebConfig::default()
            });
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/mcp-apps/sandbox")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let csp = resp.headers()["content-security-policy"]
                .to_str()
                .unwrap()
                .to_owned();
            assert!(csp.contains("frame-ancestors https://yuuka.example"));
            let html = String::from_utf8(
                to_bytes(resp.into_body(), usize::MAX)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap();
            assert!(html.contains(r#"content="https://yuuka.example""#));
        }
    }
}
