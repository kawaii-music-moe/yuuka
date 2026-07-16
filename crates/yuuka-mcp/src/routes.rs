//! MCP 管理 + プロキシ ルートハンドラ（Node `mcpRoutes.ts` パリティ）。
//!
//! 管理系 7 ルートは [`AuthenticatedUser`] extractor で `auth:"user"` を型強制する。proxy 2 ルートは
//! `auth:"none"`（proxyToken 主体・Cookie 非依存）。レスポンス本文・日本語メッセージ・ステータスコードは
//! Node と**バイト単位一致**。想定内の業務エラー（400/401/403/404/502）は明示 JSON、想定外の
//! [`DbError`](yuuka_core::DbError) のみ `?` で [`ApiError`] に写像（500）。
//!
//! ダッシュボード HTML の書き換えパイプライン（`<base>` 除去 → `MCP_PATH` 差替 → `tokenFromHash` 差替 →
//! トークン注入 → akizakura CSS インライン化）は実クライアント配線時に正しく効くよう忠実に実装する。
//! [`NullMcpClient`] は `fetch_dashboard_html` が常に `Err` のため success 経路は実行時に到達しないが、
//! パイプライン自体はユニットテストで検証する。

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, VARY, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde_json::{json, Value};
use yuuka_web::{ApiError, AppState, AuthenticatedUser};

use crate::repo::{self, McpServerRecord};
use crate::McpRuntime;

// ─── 共通ヘルパ ───────────────────────────────────────────────────────────────

/// Node `toSafeView`：認証情報・暗号化列を除いた安全なビュー（`granted_bot_ids` は呼び出し側で足す）。
fn to_safe_view(server: &McpServerRecord) -> Value {
    let tools: Vec<Value> = repo::parse_tools_cache(server)
        .into_iter()
        .map(|t| json!({ "name": t.name, "description": t.description }))
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
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let uid = &user.0.discord_id;
    let own = repo::list_servers_for_owner(&state.db, uid).await?;
    let system = repo::list_system_servers(&state.db).await?;

    let mut servers: Vec<Value> = Vec::with_capacity(own.len() + system.len());
    for s in &own {
        let mut view = to_safe_view(s);
        let bot_ids = repo::list_bot_ids_for_server(&state.db, s.id).await?;
        if let Some(obj) = view.as_object_mut() {
            obj.insert("granted_bot_ids".to_owned(), json!(bot_ids));
        }
        servers.push(view);
    }
    for s in &system {
        let mut view = to_safe_view(s);
        if let Some(obj) = view.as_object_mut() {
            obj.insert("granted_bot_ids".to_owned(), json!(Vec::<String>::new()));
        }
        servers.push(view);
    }
    Ok((StatusCode::OK, Json(json!({ "success": true, "servers": servers }))).into_response())
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

/// [`McpTool`](crate::McpTool) 一覧を tools_cache 用 JSON（`{name, description?, inputSchema?}[]`）へ整形する。
///
/// Node `updateToolsCache(JSON.stringify(McpToolDef[]))` パリティ: `description`/`inputSchema` は
/// 存在時のみ載せる。`inputSchema` を保持することで、FC ループの動的ツール登録（[`crate::McpProvider`]）が
/// キャッシュヒット時にもパラメータスキーマを復元できる。
fn tools_to_cache_json(tools: &[crate::McpTool]) -> String {
    let arr: Vec<Value> = tools
        .iter()
        .map(|t| {
            let mut obj = serde_json::Map::new();
            obj.insert("name".to_owned(), json!(t.name));
            if let Some(d) = &t.description {
                obj.insert("description".to_owned(), json!(d));
            }
            if let Some(schema) = &t.input_schema {
                obj.insert("inputSchema".to_owned(), schema.clone());
            }
            Value::Object(obj)
        })
        .collect();
    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".to_owned())
}

// ─── POST /api/mcp-servers/refresh（ツール一覧の再取得） ───────────────────────

pub(crate) async fn refresh(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<McpRuntime>>,
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let Some(id) = parse_body_id(body.get("id")) else {
        return Ok(err_json(StatusCode::NOT_FOUND, "MCPサーバーが見つかりません。"));
    };
    let admin = repo::is_admin(&state.db, &user.0.discord_id).await?;
    let server = repo::get_server_by_id(&state.db, id).await?;
    let Some(server) = server.filter(|s| repo::can_manage(s, &user.0.discord_id, admin)) else {
        return Ok(err_json(StatusCode::NOT_FOUND, "MCPサーバーが見つかりません。"));
    };

    match rt.client.refresh_tools(&server).await {
        Ok(tools) => {
            let json_str = tools_to_cache_json(&tools);
            repo::update_tools_cache(&state.db, id, &json_str).await?;
            let fresh = repo::get_server_by_id(&state.db, id).await?.unwrap_or(server);
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
        return Ok(err_json(StatusCode::NOT_FOUND, "MCPサーバーが見つかりません。"));
    };
    // Node `ctx.body.enabled === true`（真偽値 true のみ enabled）。
    let enabled = body.get("enabled") == Some(&Value::Bool(true));
    let admin = repo::is_admin(&state.db, &user.0.discord_id).await?;
    let server = repo::get_server_by_id(&state.db, id).await?;
    let Some(_server) = server.filter(|s| repo::can_manage(s, &user.0.discord_id, admin)) else {
        return Ok(err_json(StatusCode::NOT_FOUND, "MCPサーバーが見つかりません。"));
    };
    repo::set_enabled(&state.db, id, enabled).await?;
    // 無効化したら発行済みプロキシトークンを即時失効させる。
    if !enabled {
        rt.tokens.revoke_for_server(id);
    }
    let message = if enabled {
        "MCPサーバーを有効化しました。"
    } else {
        "MCPサーバーを無効化しました。"
    };
    Ok((StatusCode::OK, Json(json!({ "success": true, "message": message }))).into_response())
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
        rt.tokens.revoke_for_server(id);
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
    Ok((StatusCode::OK, Json(json!({ "success": ok, "message": message }))).into_response())
}

// ─── GET /api/mcp-servers/:id/dashboard/status（管理ページ提供の有無） ──────────

pub(crate) async fn dashboard_status(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<McpRuntime>>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let Some(server) = resolve_manageable(&state, &user, &id).await? else {
        return Ok(err_json(StatusCode::NOT_FOUND, "MCPサーバーが見つかりません。"));
    };
    let available = rt.client.probe_dashboard(&server).await;
    Ok((
        StatusCode::OK,
        Json(json!({ "success": true, "available": available })),
    )
        .into_response())
}

/// `:id` パス値から number へ、行取得・canManage まで通す共通前段（不成立は `Ok(None)`）。
async fn resolve_manageable(
    state: &AppState,
    user: &AuthenticatedUser,
    id_str: &str,
) -> Result<Option<McpServerRecord>, ApiError> {
    // Node: Number(ctx.params.id) → Number.isInteger 判定（小数・非数は不成立）。
    let Some(id) = id_str.trim().parse::<i64>().ok() else {
        return Ok(None);
    };
    let admin = repo::is_admin(&state.db, &user.0.discord_id).await?;
    let server = repo::get_server_by_id(&state.db, id).await?;
    Ok(server.filter(|s| repo::can_manage(s, &user.0.discord_id, admin)))
}

// ─── GET /api/mcp-servers/:id/dashboard（管理ページ HTML・iframe 用） ───────────

pub(crate) async fn dashboard(
    user: AuthenticatedUser,
    Extension(rt): Extension<Arc<McpRuntime>>,
    State(state): State<AppState>,
    Path(id_str): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let Some(server) = resolve_manageable(&state, &user, &id_str).await? else {
        return Ok(frame_error(StatusCode::NOT_FOUND, "MCPサーバーが見つかりません。"));
    };
    if !rt.client.probe_dashboard(&server).await {
        return Ok(frame_error(
            StatusCode::NOT_FOUND,
            "このMCPサーバーは管理ページを提供していません。",
        ));
    }
    let (status, html) = match rt.client.fetch_dashboard_html(&server).await {
        Ok(v) => v,
        Err(e) => {
            return Ok(frame_error(
                StatusCode::BAD_GATEWAY,
                &format!("管理ページの取得に失敗しました: {e}"),
            ));
        }
    };
    if status != 200 {
        return Ok(frame_error(
            StatusCode::BAD_GATEWAY,
            &format!("管理ページの取得に失敗しました (HTTP {status})。"),
        ));
    }

    // 自己オリジン（config.base_url 由来・未設定時は Host ヘッダ）。
    let self_origin = self_origin(&state, &headers);
    let proxy_token = rt.tokens.issue(server.id, &user.0.discord_id);
    let akizakura = rt.client.fetch_akizakura_css().await;

    match rewrite_dashboard(&html, server.id, &self_origin, &proxy_token, akizakura.as_deref()) {
        Ok(final_html) => {
            let frame_csp = frame_csp(&self_origin);
            let mut resp = (StatusCode::OK, final_html).into_response();
            let h = resp.headers_mut();
            h.insert(
                CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            );
            h.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
            if let Ok(v) = HeaderValue::from_str(&frame_csp) {
                h.insert(CONTENT_SECURITY_POLICY, v);
            }
            h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
            Ok(resp)
        }
        Err(msg) => Ok(frame_error(StatusCode::BAD_GATEWAY, &msg)),
    }
}

/// config.base_url のオリジン、未設定時は `http://{host}`（Node `selfOrigin`）。
fn self_origin(state: &AppState, headers: &HeaderMap) -> String {
    if let Some(base) = state.config.base_url.as_deref() {
        if let Some(origin) = url_origin(base) {
            return origin;
        }
    }
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    format!("http://{host}")
}

/// URL の origin（`scheme://host[:port]`）を素朴に抽出する（Node `new URL(base).origin`）。
fn url_origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme.is_empty() || rest.is_empty() {
        return None;
    }
    // authority はパス・クエリ・フラグメント境界まで。
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|a| !a.is_empty())?;
    Some(format!("{scheme}://{authority}"))
}

/// frame 用 CSP（Node `frameCsp`・末尾セミコロン付き）。
fn frame_csp(self_origin: &str) -> String {
    [
        "default-src 'none'".to_owned(),
        "base-uri 'none'".to_owned(),
        "script-src 'unsafe-inline'".to_owned(),
        "style-src 'unsafe-inline' https://akizakura.pages.dev https://fonts.googleapis.com"
            .to_owned(),
        "font-src https://fonts.gstatic.com data:".to_owned(),
        format!("img-src 'self' data: {self_origin}"),
        format!("connect-src {self_origin}"),
        "frame-ancestors 'self'".to_owned(),
    ]
    .join("; ")
        + ";"
}

// ─── ダッシュボード HTML 書き換えパイプライン（Node パリティ） ─────────────────

/// HTML を iframe 用に書き換える（`<base>` 除去 → `MCP_PATH` 差替 → `tokenFromHash` 差替 →
/// トークン注入 → akizakura CSS インライン化）。書き換えに失敗した段があれば `Err(message)`。
fn rewrite_dashboard(
    html: &str,
    id: i64,
    self_origin: &str,
    proxy_token: &str,
    akizakura_css: Option<&str>,
) -> Result<String, String> {
    // 1) `<base ...>` の最初の 1 個を除去（Node `/<base\b[^>]*>/i` replace ""）。
    let rewritten = remove_first_base_tag(html);

    // 2) `var MCP_PATH = "..."` をプロキシ絶対 URL へ差替。不一致（不発）は 502。
    let proxy_mcp_url = format!("{self_origin}/proxy/mcp/{id}/mcp");
    let replacement = format!("var MCP_PATH = \"{proxy_mcp_url}\"");
    let (rewritten, replaced) = replace_mcp_path(&rewritten, &replacement);
    if !replaced {
        return Err("管理ページの形式が想定と異なるため表示できません（MCP_PATH）。".to_owned());
    }

    // 3) `function tokenFromHash() {...}` を window 変数返却へ差替。不発は 502。
    let (rewritten, replaced) = replace_token_from_hash(&rewritten);
    if !replaced {
        return Err(
            "管理ページの形式が想定と異なるため表示できません（tokenFromHash）。".to_owned(),
        );
    }

    // 4) akizakura の <link> を <style> でインライン化（取得できたときのみ・見つからなければ残置）。
    let rewritten = match akizakura_css {
        Some(css) => inline_akizakura(&rewritten, css),
        None => rewritten,
    };

    // 5) `<head ...>` 直後にトークン注入スクリプトを差し込む（無ければ先頭へ prepend）。
    let auto_token_script = format!("<script>window.__mcpProxyToken__ = \"{proxy_token}\";</script>");
    let final_html = inject_after_head(&rewritten, &auto_token_script);
    Ok(final_html)
}

/// `<base\b[^>]*>` の最初の 1 個を除去する（大小無視・Node `/<base\b[^>]*>/i`）。
fn remove_first_base_tag(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut search = 0usize;
    while let Some(rel) = lower.get(search..).and_then(|s| s.find("<base")) {
        let start = search + rel;
        // `\b` 境界: `<base` の直後は「単語構成文字（英数字/_）でない」こと（`<basefoo>` を除外）。
        let after = lower.as_bytes().get(start + 5).copied();
        let is_boundary = match after {
            Some(b) => !(b.is_ascii_alphanumeric() || b == b'_'),
            None => true,
        };
        if is_boundary {
            // `>` まで（`[^>]*>`）。見つからなければ除去不能として原文維持。
            if let Some(end_rel) = lower.get(start..).and_then(|s| s.find('>')) {
                let end = start + end_rel + 1;
                let mut out = String::with_capacity(html.len());
                if let (Some(head), Some(tail)) = (html.get(..start), html.get(end..)) {
                    out.push_str(head);
                    out.push_str(tail);
                    return out;
                }
            }
            return html.to_owned();
        }
        search = start + 5;
    }
    html.to_owned()
}

/// `var MCP_PATH = "[^"]*"` の最初の 1 個を差替（Node `/var MCP_PATH = "[^"]*"/`）。
/// 返り値 `(新文字列, 置換が起きたか)`。
fn replace_mcp_path(html: &str, replacement: &str) -> (String, bool) {
    const PREFIX: &str = "var MCP_PATH = \"";
    let Some(start) = html.find(PREFIX) else {
        return (html.to_owned(), false);
    };
    let value_start = start + PREFIX.len();
    // `[^"]*` は次の `"` まで（同一行・改行を跨いでも Node 正規表現の `.` 除外に該当しないため `"` のみ境界）。
    let Some(rel_end) = html.get(value_start..).and_then(|s| s.find('"')) else {
        return (html.to_owned(), false);
    };
    let end = value_start + rel_end + 1; // 閉じ `"` を含む。
    let (Some(head), Some(tail)) = (html.get(..start), html.get(end..)) else {
        return (html.to_owned(), false);
    };
    let mut out = String::with_capacity(head.len() + replacement.len() + tail.len());
    out.push_str(head);
    out.push_str(replacement);
    out.push_str(tail);
    (out, true)
}

/// `function tokenFromHash() {...}`（`\s*` + `{[^}]*}`）を window 変数返却へ差替。
/// Node `/function tokenFromHash\(\)\s*\{[^}]*\}/`。返り値 `(新文字列, 置換が起きたか)`。
fn replace_token_from_hash(html: &str) -> (String, bool) {
    const REPLACEMENT: &str =
        "function tokenFromHash() { return window.__mcpProxyToken__ || null; }";
    const HEAD: &str = "function tokenFromHash()";
    let Some(start) = html.find(HEAD) else {
        return (html.to_owned(), false);
    };
    // HEAD 後の `\s*`（空白）をスキップして `{` を要求。
    let after_head = start + HEAD.len();
    let ws_len = html
        .get(after_head..)
        .map(|s| s.len() - s.trim_start().len())
        .unwrap_or(0);
    let brace_pos = after_head + ws_len;
    if html.as_bytes().get(brace_pos).copied() != Some(b'{') {
        return (html.to_owned(), false);
    }
    // `[^}]*}` は次の `}` まで。
    let Some(rel_close) = html.get(brace_pos..).and_then(|s| s.find('}')) else {
        return (html.to_owned(), false);
    };
    let end = brace_pos + rel_close + 1; // 閉じ `}` を含む。
    let (Some(head), Some(tail)) = (html.get(..start), html.get(end..)) else {
        return (html.to_owned(), false);
    };
    let mut out = String::with_capacity(head.len() + REPLACEMENT.len() + tail.len());
    out.push_str(head);
    out.push_str(REPLACEMENT);
    out.push_str(tail);
    (out, true)
}

/// akizakura の `<link ... href="https://akizakura.pages.dev/akizakura.css" ...>` を `<style>` へ置換。
/// Node `/<link\b[^>]*href="https:\/\/akizakura\.pages\.dev\/akizakura\.css"[^>]*>/i`。
/// 見つからなければ原文維持（Node は console.error のみ）。
fn inline_akizakura(html: &str, css: &str) -> String {
    const AKIZAKURA_HREF: &str = "https://akizakura.pages.dev/akizakura.css";
    let lower = html.to_ascii_lowercase();
    // href の位置を探し、そこから前方の `<link`（単語境界）・後方の `>` を確定する。
    let Some(href_pos) = lower.find(&AKIZAKURA_HREF.to_ascii_lowercase()) else {
        return html.to_owned();
    };
    // href_pos より前で最も近い `<link`（境界）を探す。
    let Some(link_rel) = lower.get(..href_pos).and_then(|s| s.rfind("<link")) else {
        return html.to_owned();
    };
    let start = link_rel;
    let after = lower.as_bytes().get(start + 5).copied();
    let is_boundary = match after {
        Some(b) => !(b.is_ascii_alphanumeric() || b == b'_'),
        None => true,
    };
    if !is_boundary {
        return html.to_owned();
    }
    // start から次の `>` までがタグ（`[^>]*>` はタグ内に `>` を含まない前提）。
    let Some(gt_rel) = lower.get(start..).and_then(|s| s.find('>')) else {
        return html.to_owned();
    };
    let end = start + gt_rel + 1;
    // タグ内に href が含まれることを確認（別 <link>...><link href=akizakura> の誤爆防止）。
    if href_pos >= end {
        return html.to_owned();
    }
    let (Some(head), Some(tail)) = (html.get(..start), html.get(end..)) else {
        return html.to_owned();
    };
    let mut out = String::with_capacity(head.len() + css.len() + tail.len() + 40);
    out.push_str(head);
    out.push_str("<style data-akizakura>");
    out.push_str(css);
    out.push_str("</style>");
    out.push_str(tail);
    out
}

/// `<head ...>` の最初の 1 個の直後にスクリプトを差し込む（`<header>` を誤検出しないよう境界を要求）。
/// Node `/<head(\s[^>]*)?>/i`。無ければ先頭へ prepend。
fn inject_after_head(html: &str, script: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut search = 0usize;
    while let Some(rel) = lower.get(search..).and_then(|s| s.find("<head")) {
        let start = search + rel;
        let after = lower.as_bytes().get(start + 5).copied();
        // Node `<head(\s[^>]*)?>`: `<head` の直後は `>` か 空白（`<header>` は `r` で不一致）。
        match after {
            Some(b'>') => {
                let end = start + 6; // `<head>`
                return splice(html, end, script);
            }
            Some(b) if b.is_ascii_whitespace() => {
                if let Some(gt_rel) = lower.get(start..).and_then(|s| s.find('>')) {
                    let end = start + gt_rel + 1;
                    return splice(html, end, script);
                }
                return format!("{script}{html}");
            }
            _ => {
                // `<header` 等 → 次の候補へ。
                search = start + 5;
            }
        }
    }
    format!("{script}{html}")
}

/// `at` バイト位置に `insert` を差し込む（境界不正時は原文維持で prepend しない）。
fn splice(html: &str, at: usize, insert: &str) -> String {
    let (Some(head), Some(tail)) = (html.get(..at), html.get(at..)) else {
        return html.to_owned();
    };
    let mut out = String::with_capacity(head.len() + insert.len() + tail.len());
    out.push_str(head);
    out.push_str(insert);
    out.push_str(tail);
    out
}

/// iframe 用エラーページ（text/html・Node `sendFrameError`）。`&<>` を HTML エスケープし最小 CSP を付与。
fn frame_error(status: StatusCode, message: &str) -> Response {
    let safe = message
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let body = format!(
        "<!doctype html><html lang=\"ja\"><head><meta charset=\"utf-8\"><meta name=\"color-scheme\" content=\"light dark\"></head><body style=\"font-family:system-ui,-apple-system,sans-serif;padding:24px;color:#52525b;font-size:0.9rem;\">{safe}</body></html>"
    );
    let mut resp = (status, body).into_response();
    let h = resp.headers_mut();
    h.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    h.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'self';",
        ),
    );
    h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

// ─── OPTIONS /proxy/mcp/:id/mcp（CORS プリフライト） ───────────────────────────

pub(crate) async fn proxy_preflight(headers: HeaderMap) -> Response {
    // 要求されたヘッダをそのまま許可リストへ反映（無ければ既定・Node）。
    let req_headers = headers
        .get("access-control-request-headers")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .unwrap_or("authorization, content-type, accept, mcp-protocol-version")
        .to_owned();

    let mut resp = StatusCode::NO_CONTENT.into_response();
    let h = resp.headers_mut();
    h.insert(
        HeaderName::from_static("access-control-allow-origin"),
        HeaderValue::from_static("null"),
    );
    h.insert(
        HeaderName::from_static("access-control-allow-methods"),
        HeaderValue::from_static("POST, OPTIONS"),
    );
    if let Ok(v) = HeaderValue::from_str(&req_headers) {
        h.insert(
            HeaderName::from_static("access-control-allow-headers"),
            v,
        );
    }
    h.insert(
        HeaderName::from_static("access-control-max-age"),
        HeaderValue::from_static("600"),
    );
    h.insert(VARY, HeaderValue::from_static("Origin"));
    // NO Allow-Credentials（ACAO:null + 資格情報無しの安全な組合せ）。
    resp
}

// ─── POST /proxy/mcp/:id/mcp（エンドポイント中継） ─────────────────────────────

pub(crate) async fn proxy_post(
    Extension(rt): Extension<Arc<McpRuntime>>,
    State(state): State<AppState>,
    Path(id_str): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    // Node: Number(ctx.params.id) → Number.isInteger（不成立は 400）。
    let Some(id) = id_str.trim().parse::<i64>().ok() else {
        return Ok(proxy_cors(err_json(
            StatusCode::BAD_REQUEST,
            "不正なサーバーIDです。",
        )));
    };

    // Authorization: Bearer <proxyToken> を検証。
    let bearer = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    let Some(token_user_id) = rt.tokens.validate(bearer, id) else {
        return Ok(proxy_cors(err_json(
            StatusCode::UNAUTHORIZED,
            "プロキシトークンが無効または期限切れです。",
        )));
    };

    // 発行ユーザーの現在の実在・ロールを DB から引き直して再検証する。
    let Some(is_admin) = repo::lookup_user_admin(&state.db, &token_user_id).await? else {
        return Ok(proxy_cors(err_json(
            StatusCode::FORBIDDEN,
            "トークン発行ユーザーが存在しません。",
        )));
    };

    let Some(server) = repo::get_server_by_id(&state.db, id).await? else {
        return Ok(proxy_cors(err_json(
            StatusCode::NOT_FOUND,
            "MCPサーバーが見つかりません。",
        )));
    };
    if !repo::can_manage(&server, &token_user_id, is_admin) {
        return Ok(proxy_cors(err_json(
            StatusCode::FORBIDDEN,
            "このMCPサーバーを操作する権限がありません。",
        )));
    }
    if server.enabled != 1 {
        return Ok(proxy_cors(err_json(
            StatusCode::FORBIDDEN,
            "このMCPサーバーは無効化されています。",
        )));
    }

    let accept = headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok());

    match rt.client.proxy(&server, body, accept).await {
        Ok(up) => {
            let status = StatusCode::from_u16(up.status).unwrap_or(StatusCode::BAD_GATEWAY);
            let mut resp = (status, up.body).into_response();
            let h = resp.headers_mut();
            if let Ok(ct) = HeaderValue::from_str(&up.content_type) {
                h.insert(CONTENT_TYPE, ct);
            }
            h.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
            if let Some(sid) = up.mcp_session_id.as_deref() {
                if let Ok(v) = HeaderValue::from_str(sid) {
                    h.insert(HeaderName::from_static("mcp-session-id"), v);
                }
            }
            // 同一オリジン中継のため CORS ヘッダーは付けない（Node と同じ）。
            Ok(resp)
        }
        Err(e) => Ok(proxy_cors(err_json(
            StatusCode::BAD_GATEWAY,
            &format!("MCPサーバーへの接続に失敗しました: {e}"),
        ))),
    }
}

/// プロキシのエラー応答に付ける CORS ヘッダ（Node: ACAO:null・Allow-Credentials 無し・Vary:Origin）。
fn proxy_cors(mut resp: Response) -> Response {
    let h = resp.headers_mut();
    h.insert(
        HeaderName::from_static("access-control-allow-origin"),
        HeaderValue::from_static("null"),
    );
    h.insert(VARY, HeaderValue::from_static("Origin"));
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── HTML 書き換えパイプライン ──────────────────────────────────────────────

    const SAMPLE: &str = concat!(
        "<!doctype html><html><head><base href=\"https://mcp.example/dashboard/\">",
        "<link rel=\"stylesheet\" href=\"https://akizakura.pages.dev/akizakura.css\">",
        "</head><body><script>var MCP_PATH = \"https://mcp.example/mcp\";",
        "function tokenFromHash() { return location.hash.slice(1); }</script></body></html>",
    );

    #[test]
    fn rewrite_success_path() {
        let out = rewrite_dashboard(
            SAMPLE,
            42,
            "https://yuuka.test",
            "TOKEN123",
            Some(":root{--x:1}"),
        )
        .expect("rewrite ok");
        // <base> 除去。
        assert!(!out.contains("<base "));
        // MCP_PATH がプロキシ URL へ。
        assert!(out.contains("var MCP_PATH = \"https://yuuka.test/proxy/mcp/42/mcp\""));
        assert!(!out.contains("\"https://mcp.example/mcp\""));
        // tokenFromHash 差替。
        assert!(out.contains("return window.__mcpProxyToken__ || null;"));
        assert!(!out.contains("location.hash.slice(1)"));
        // akizakura インライン化。
        assert!(out.contains("<style data-akizakura>:root{--x:1}</style>"));
        assert!(!out.contains("akizakura.css\">"));
        // トークン注入が <head> 直後。
        assert!(out.contains("<head><script>window.__mcpProxyToken__ = \"TOKEN123\";</script>"));
    }

    #[test]
    fn rewrite_fails_on_missing_mcp_path() {
        let html = "<html><head></head><body>function tokenFromHash() {return null;}</body></html>";
        let err = rewrite_dashboard(html, 1, "https://y.test", "T", None).unwrap_err();
        assert_eq!(
            err,
            "管理ページの形式が想定と異なるため表示できません（MCP_PATH）。"
        );
    }

    #[test]
    fn rewrite_fails_on_missing_token_fn() {
        let html = "<html><head></head><body><script>var MCP_PATH = \"x\";</script></body></html>";
        let err = rewrite_dashboard(html, 1, "https://y.test", "T", None).unwrap_err();
        assert_eq!(
            err,
            "管理ページの形式が想定と異なるため表示できません（tokenFromHash）。"
        );
    }

    #[test]
    fn header_not_head_boundary() {
        // <header> は誤検出しない（トークンは先頭へ prepend される）。
        let html = "<header>x</header>var MCP_PATH = \"y\";function tokenFromHash() {return 1;}";
        let out = rewrite_dashboard(html, 1, "https://y.test", "T", None).unwrap();
        assert!(out.starts_with("<script>window.__mcpProxyToken__ = \"T\";</script><header>"));
    }

    #[test]
    fn base_tag_boundary_not_basefoo() {
        // `<basefoo>` は `<base>` 境界に一致しない（除去されない）。
        let html = "<basefoo>keep</basefoo>";
        assert_eq!(remove_first_base_tag(html), html);
        // 本物の <base ...> は除去。
        assert_eq!(remove_first_base_tag("a<base href=\"x\">b"), "ab");
    }

    #[test]
    fn url_origin_extract() {
        assert_eq!(
            url_origin("https://yuuka.test/path?q=1#h").as_deref(),
            Some("https://yuuka.test")
        );
        assert_eq!(
            url_origin("http://localhost:3000").as_deref(),
            Some("http://localhost:3000")
        );
        assert_eq!(url_origin("not-a-url"), None);
    }

    #[test]
    fn frame_error_escapes_and_headers() {
        let resp = frame_error(StatusCode::NOT_FOUND, "a<b>&c");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            resp.headers().get(CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            resp.headers().get(CONTENT_SECURITY_POLICY).unwrap(),
            "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'self';"
        );
    }

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

        use crate::{routes, McpRuntime, NullMcpClient, ProxyTokenManager};

        static SEQ: AtomicU64 = AtomicU64::new(0);

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
                    "INSERT INTO users (discord_id, username, password_hash, salt, role) \
                     VALUES ('alice','alice','x','deadbeef','user'),('root','root','x','deadbeef','admin')",
                    [],
                )
                .expect("seed users");
                // alice 所有のサーバー 1 件（enabled）。
                conn.execute(
                    "INSERT INTO mcp_servers (id, user_id, name, endpoint_url) \
                     VALUES (1, 'alice', 'a', 'https://x/mcp')",
                    [],
                )
                .expect("seed server");
            }
            db
        }

        fn app() -> axum::Router {
            let state = AppState::new(Arc::new(FakeAuth), WebConfig::default(), seed_db());
            let rt = Arc::new(McpRuntime::new(
                None,
                Arc::new(NullMcpClient),
                Arc::new(ProxyTokenManager::new()),
            ));
            routes(rt).with_state(state)
        }

        async fn get(app: &axum::Router, uri: &str, token: &str) -> (StatusCode, Value) {
            let mut b = Request::builder().method("GET").uri(uri);
            if !token.is_empty() {
                b = b.header("cookie", format!("__Host-yuuka-session={token}"));
            }
            let resp = app.clone().oneshot(b.body(Body::empty()).unwrap()).await.unwrap();
            let status = resp.status();
            let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            (status, json)
        }

        async fn post(app: &axum::Router, uri: &str, token: &str, body: &str) -> (StatusCode, Value) {
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

        #[tokio::test]
        async fn dashboard_status_null_probe_false() {
            let app = app();
            let (st, j) = get(&app, "/api/mcp-servers/1/dashboard/status", "alice").await;
            assert_eq!(st, StatusCode::OK);
            assert_eq!(j["success"], Value::Bool(true));
            assert_eq!(j["available"], Value::Bool(false));
        }

        #[tokio::test]
        async fn dashboard_html_null_probe_404_frame() {
            let app = app();
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri("/api/mcp-servers/1/dashboard")
                        .header("cookie", "__Host-yuuka-session=alice")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::NOT_FOUND);
            assert_eq!(
                resp.headers().get("content-type").unwrap(),
                "text/html; charset=utf-8"
            );
            let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            let html = String::from_utf8_lossy(&bytes);
            assert!(html.contains("このMCPサーバーは管理ページを提供していません。"));
        }

        #[tokio::test]
        async fn proxy_preflight_204_cors() {
            let app = app();
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("OPTIONS")
                        .uri("/proxy/mcp/1/mcp")
                        .header("access-control-request-headers", "authorization, x-custom")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::NO_CONTENT);
            let h = resp.headers();
            assert_eq!(h.get("access-control-allow-origin").unwrap(), "null");
            assert_eq!(h.get("access-control-allow-methods").unwrap(), "POST, OPTIONS");
            assert_eq!(h.get("access-control-allow-headers").unwrap(), "authorization, x-custom");
            assert_eq!(h.get("access-control-max-age").unwrap(), "600");
            assert!(h.get("access-control-allow-credentials").is_none());
        }

        #[tokio::test]
        async fn proxy_post_bad_id_400() {
            let app = app();
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/proxy/mcp/abc/mcp")
                        .body(Body::from("{}"))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
            let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            let j: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(j["message"], Value::String("不正なサーバーIDです。".to_owned()));
        }

        #[tokio::test]
        async fn proxy_post_missing_token_401() {
            let app = app();
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/proxy/mcp/1/mcp")
                        .body(Body::from("{}"))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
            // ACAO:null は付く（Cookie 非依存の安全な組合せ）。
            assert_eq!(
                resp.headers().get("access-control-allow-origin").unwrap(),
                "null"
            );
            let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            let j: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                j["message"],
                Value::String("プロキシトークンが無効または期限切れです。".to_owned())
            );
        }
    }
}
