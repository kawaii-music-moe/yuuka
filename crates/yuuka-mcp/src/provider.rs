//! `McpProvider`（§4.4）— 各ユーザーの登録済み + 有効な MCP サーバーのツールを Gemini FC ループへ
//! 動的登録する [`ToolProvider`]。Node `src/functions/mcpDynamic.ts` パリティ。
//!
//! **構築は非同期・使用は同期**: [`ToolProvider::list`] は同期契約だが、MCP のツール探索は DB 読み取り +
//! ネットワーク（`tools/list`）を要する。そこで会話 1 ターンの開始前に [`McpProvider::discover`] で
//! サーバー解決 → キャッシュ鮮度確認（TTL 1 時間）→ 動的ツール名の生成 → 宣言と dispatch 表の materialize
//! を済ませ、[`ToolProvider::list`] は生成済み宣言を（露出ゲート越しに）返すだけにする。Node が FC ループ
//! 前に `getMcpFunctionModuleForBot` で FunctionModule を組むのと同じ分業。
//!
//! **スコープ規約（Node v5/v7）**: 共有秘書（`system_default`）は発話者本人が付与した許可分 + システム
//! レベル登録のみ（[`repo::list_servers_granted_to_bot_scoped`]・クロステナント露出防止）。単一所有 Bot は
//! owner の許可をそのまま使う（[`repo::list_servers_granted_to_bot`]）。呼び出し時の再検証も同一スコープ。
//!
//! **露出ゲート**: MCP 動的ツールは秘書ツール（Node は `caps.has("mcp")` 経路のみ・秘書経路）。
//! [`ToolExposure`] を `mcp` 能力 + 秘書経路で構成し、`list` で `is_visible(ctx)` を通す。
//!
//! **live 検証は実環境へ延期**: 実 MCP サーバーへの `tools/list`/`tools/call` HTTP はネットワークを要する
//! ためユニットテスト対象外（[`crate::HttpMcpClient`] 同様）。ユニットテストは純ロジック（動的ツール名の
//! 生成/衝突退避、fake `McpClient` からの `list` 組み立て、`invoke` の dispatch 経路と `tools/call` 形状、
//! キャッシュ TTL、露出ゲート）に限定する。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use yuuka_core::tool::{FunctionDeclaration, ToolExposure};
use yuuka_core::{ToolContext, ToolError, ToolName, ToolOutcome, ToolProvider};
use yuuka_web::Db;

use crate::repo::{self, McpServerRecord};
use crate::{McpClient, McpTool};

/// Gemini Function 名の最大長（Node `MAX_FUNCTION_NAME_LENGTH` = 63・英数字/`_`・64 文字未満）。
const MAX_FUNCTION_NAME_LENGTH: usize = 63;

/// ツールキャッシュの TTL（Node `TOOLS_CACHE_TTL_MS` = 1 時間ごとに再取得・§4.4.2）。
pub const TOOLS_CACHE_TTL: Duration = Duration::from_secs(60 * 60);

/// 名前空間接頭辞（Node `mcp{serverId}_...`）。ここでは接頭辞だけを定数化し名前生成は関数で行う。
const MCP_PREFIX: &str = "mcp";

/// MCP ツールの Gemini Function 名を生成する（Node `mcpFunctionName`）。
///
/// `mcp{serverId}_{sanitized}` を 63 文字へ切り詰める。`sanitized` は `[^a-zA-Z0-9_]` を `_` へ置換。
#[must_use]
pub fn mcp_function_name(server_id: i64, tool_name: &str) -> String {
    let sanitized: String = tool_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    let name = format!("{MCP_PREFIX}{server_id}_{sanitized}");
    truncate_chars(&name, MAX_FUNCTION_NAME_LENGTH)
}

/// 名前衝突時に決定的な短ハッシュを付与して 63 文字以内で一意化する（Node `disambiguateFunctionName`）。
///
/// サニタイズ（`list-items` と `list.items` → `mcp1_list_items`）や 63 字切り詰めで別ツールが同名になり、
/// 2 番目が無言で捨てられる事故を防ぐ。ハッシュは `sha1(serverId:toolName)` の先頭 6 桁 hex。
fn disambiguate_function_name(server_id: i64, tool_name: &str, used: &HashSet<String>) -> String {
    let base = mcp_function_name(server_id, tool_name);
    let hash = short_hash(server_id, tool_name);
    // base を (63 - (hash+1)) 文字へ切り詰め + `_{hash}`（Node と同じ組み立て）。
    let head_len = MAX_FUNCTION_NAME_LENGTH.saturating_sub(hash.len() + 1);
    let mut candidate = format!("{}_{hash}", truncate_chars(&base, head_len));
    let mut n = 0u32;
    while used.contains(&candidate) {
        let tag = format!("_{hash}_{n}");
        let head_len = MAX_FUNCTION_NAME_LENGTH.saturating_sub(tag.len());
        candidate = format!("{}{tag}", truncate_chars(&base, head_len));
        n += 1;
    }
    candidate
}

/// 文字数（char 単位・Node `slice` パリティ）で切り詰める。
fn truncate_chars(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

/// `sha1(serverId:toolName)` の先頭 6 桁 hex（Node `createHash("sha1")...digest("hex").slice(0,6)`）。
fn short_hash(server_id: i64, tool_name: &str) -> String {
    use sha1::{Digest, Sha1};
    let digest = Sha1::digest(format!("{server_id}:{tool_name}").as_bytes());
    // hex 文字列の先頭 6 桁（= 3 バイト分）。
    let hex = hex::encode(digest);
    truncate_chars(&hex, 6)
}

/// dispatch 表の 1 エントリ（Function 名 → 所有サーバー + ツール名）。
#[derive(Debug, Clone)]
struct DispatchEntry {
    /// 所有サーバー ID（再検証・監査 detail に使う）。
    server_id: i64,
    /// 上流ツール名（`tools/call` の `name`）。
    tool_name: String,
    /// 表示名（監査 target・成功応答の `server`）。
    server_name: String,
}

/// スコープ解決の 2 モード（Node v7）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// 共有秘書（`system_default`）: 発話者本人が付与した許可 + システムレベルのみ。
    SharedSecretary,
    /// 単一所有 Bot: owner の許可 + システムレベル。
    OwnedBot,
}

/// 各ユーザーの MCP サーバー群から動的登録した provider。会話 1 ターンごとに [`Self::discover`] で作る。
pub struct McpProvider {
    /// 生成済みの動的ツール宣言（露出ゲートは [`ToolProvider::list`] で適用）。
    declarations: Vec<FunctionDeclaration>,
    /// Function 名 → dispatch エントリ（`invoke` の解決表）。
    dispatch: HashMap<ToolName, DispatchEntry>,
    /// 露出分類（MCP 動的ツール = `mcp` 能力 + 秘書経路・全ツール共通）。
    exposure: ToolExposure,
    /// `tools/call` 発行に使う外部 MCP クライアント。
    client: Arc<dyn McpClient>,
    /// 呼び出し時の可用性再検証（`invoke` で許可・有効の再チェック）に使う DB。
    db: Db,
    /// この provider を組んだ Bot ID（再検証スコープの鍵）。
    bot_id: String,
    /// スコープモード（共有秘書 or 単一所有 Bot）。
    scope: Scope,
}

impl McpProvider {
    /// MCP 動的ツールの露出分類（`mcp` 能力・秘書経路のみ・guild スコープ非依存）。
    #[must_use]
    fn mcp_exposure() -> ToolExposure {
        ToolExposure {
            capability: Some("mcp"),
            secretary: true,
            guild_assistant: false,
            requires_guild: false,
        }
    }

    /// 会話 1 ターンの MCP 動的ツールを探索して provider を構築する（Node `getMcpFunctionModuleForBot`）。
    ///
    /// 手順: (1) スコープ解決（`system_default` は発話者スコープ）→ enabled 絞り込み。(2) キャッシュが古い
    /// サーバーは `tools/list` で再取得（失敗はキャッシュ続行）。(3) 各ツールを `mcp{id}_{name}` へ命名
    /// （衝突は退避名で一意化）し宣言 + dispatch 表を組む。サーバー取得自体が失敗しても空 provider を返す
    /// （FC ループを落とさない・Node の try/catch パリティ）。
    ///
    /// `bot_id == "system_default"` を共有秘書とみなす（Node `isSharedSecretary`）。
    pub async fn discover(
        db: Db,
        client: Arc<dyn McpClient>,
        bot_id: &str,
        speaker_user_id: &str,
    ) -> Self {
        let scope = if bot_id == "system_default" {
            Scope::SharedSecretary
        } else {
            Scope::OwnedBot
        };

        // (1) サーバー解決（失敗は空 provider・Node catch）。
        let servers = match Self::resolve_servers(&db, bot_id, speaker_user_id, scope).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(bot = %bot_id, error = %e, "MCP サーバー一覧の取得に失敗（動的ツールをスキップ）");
                return Self::empty(db, client, bot_id, scope);
            }
        };

        let mut declarations = Vec::new();
        let mut dispatch = HashMap::new();
        let mut used_names: HashSet<String> = HashSet::new();

        for server in &servers {
            // (2) キャッシュ鮮度: 古ければ再取得（失敗は既存キャッシュで続行・Node ensureFreshToolsCache）。
            let tools = Self::ensure_fresh_tools(&db, client.as_ref(), server).await;

            for tool in tools {
                if tool.name.is_empty() {
                    continue;
                }
                // (3) Function 名（衝突は退避名で一意化・Node buildMcpFunctionModule）。
                let base = mcp_function_name(server.id, &tool.name);
                let fn_name = if used_names.contains(&base) {
                    let disambiguated = disambiguate_function_name(server.id, &tool.name, &used_names);
                    tracing::warn!(
                        server = %server.name,
                        base = %base,
                        renamed = %disambiguated,
                        tool = %tool.name,
                        "MCP Function 名が衝突したため退避名へ一意化"
                    );
                    disambiguated
                } else {
                    base
                };
                used_names.insert(fn_name.clone());

                // ToolName は namespace 付き（`mcp:{fn_name}`）で型保証する。fn_name は既に英数字/`_` のみ。
                let Ok(name) = ToolName::namespaced(MCP_PREFIX, &fn_name) else {
                    continue;
                };

                let description = build_description(server, tool.description.as_deref());
                declarations.push(FunctionDeclaration {
                    name: name.clone(),
                    description,
                    parameters_json_schema: normalize_schema(tool.input_schema),
                    // 確認要求はサーバー単位（`requires_confirmation`）。FC ループ側で承認導線を出す。
                    requires_confirmation: server.requires_confirmation == 1,
                });
                dispatch.insert(
                    name,
                    DispatchEntry {
                        server_id: server.id,
                        tool_name: tool.name,
                        server_name: server.name.clone(),
                    },
                );
            }
        }

        Self {
            declarations,
            dispatch,
            exposure: Self::mcp_exposure(),
            client,
            db,
            bot_id: bot_id.to_owned(),
            scope,
        }
    }

    /// 空 provider（サーバー未解決・取得失敗時）。
    fn empty(db: Db, client: Arc<dyn McpClient>, bot_id: &str, scope: Scope) -> Self {
        Self {
            declarations: Vec::new(),
            dispatch: HashMap::new(),
            exposure: Self::mcp_exposure(),
            client,
            db,
            bot_id: bot_id.to_owned(),
            scope,
        }
    }

    /// スコープに応じてサーバー一覧を解決し enabled のみへ絞る（Node の filter(enabled===1)）。
    async fn resolve_servers(
        db: &Db,
        bot_id: &str,
        speaker_user_id: &str,
        scope: Scope,
    ) -> Result<Vec<McpServerRecord>, yuuka_core::DbError> {
        let servers = match scope {
            Scope::SharedSecretary => {
                repo::list_servers_granted_to_bot_scoped(db, bot_id, speaker_user_id).await?
            }
            Scope::OwnedBot => repo::list_servers_granted_to_bot(db, bot_id).await?,
        };
        Ok(servers.into_iter().filter(|s| s.enabled == 1).collect())
    }

    /// キャッシュが TTL 超過なら `tools/list` で再取得して DB を更新する（Node `ensureFreshToolsCache`）。
    ///
    /// `tools_cache_updated` が null/不正日時は「未取得」とみなし必ず再取得する。再取得失敗は既存キャッシュ
    /// で続行する（FC ループを落とさない）。返り値は最新（または既存）キャッシュの完全ツール定義。
    async fn ensure_fresh_tools(db: &Db, client: &dyn McpClient, server: &McpServerRecord) -> Vec<McpTool> {
        if cache_is_stale(server.tools_cache_updated.as_deref()) {
            match client.refresh_tools(server).await {
                Ok(tools) => {
                    // キャッシュを永続化（inputSchema 込み）。取得後のレコードから再パースして返す。
                    let json_str = tools_to_cache_json(&tools);
                    if let Err(e) = repo::update_tools_cache(db, server.id, &json_str).await {
                        tracing::warn!(server = %server.name, error = %e, "MCP tools_cache の更新に失敗");
                    }
                    return tools;
                }
                Err(e) => {
                    tracing::warn!(
                        server = %server.name,
                        error = %e,
                        "MCP Tool キャッシュの更新に失敗（既存キャッシュで続行）"
                    );
                }
            }
        }
        // キャッシュヒット（または再取得失敗）: 既存キャッシュを完全パース。
        repo::parse_tools_cache_full(server)
    }
}

/// `refresh_tools` の結果を tools_cache 用 JSON（`{name, description?, inputSchema?}[]`）へ整形する。
/// routes.rs の `tools_to_cache_json` と同形（Node `updateToolsCache` パリティ）。
fn tools_to_cache_json(tools: &[McpTool]) -> String {
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

/// キャッシュが TTL 超過（または未取得）か（Node の `cacheAge > TOOLS_CACHE_TTL_MS`）。
///
/// `tools_cache_updated` は `datetime('now','localtime')` 形式（`YYYY-MM-DD HH:MM:SS`・ローカル時刻）。
/// null/パース不能は「未取得」とみなし常に stale（Node `Number.isFinite(parsedTs) ? ... : Infinity`）。
fn cache_is_stale(updated: Option<&str>) -> bool {
    let Some(raw) = updated else {
        return true; // 未取得 → 常に再取得。
    };
    match parse_local_datetime_secs(raw) {
        Some(then) => {
            let now = now_unix_secs();
            // now < then（時計巻き戻し等）でも stale 扱いにはしない（差が TTL 未満なら fresh）。
            now.saturating_sub(then) > TOOLS_CACHE_TTL.as_secs() as i64
        }
        None => true, // 不正日時 → 未取得扱い。
    }
}

/// 現在時刻の Unix 秒（ローカルとの整合のため実時間で比較する）。
fn now_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `YYYY-MM-DD HH:MM:SS`（ローカル時刻）を Unix 秒へ変換する。
///
/// `tools_cache_updated` は DB がローカルタイムゾーンで書く。TTL 比較には現在時刻もローカル基準で得た
/// Unix 秒を使うため、ここではローカル時刻の壁時計を「そのまま」秒へ換算し、同じローカル基準の now と
/// 引き算する（両者が同一 TZ オフセットのため差分は正しい）。DST 境界の 1 時間ずれは TTL=1h の再取得を
/// 高々 1 回早める/遅らせるだけで実害はない（Node も `Date.parse(local)` で同等の近似）。
fn parse_local_datetime_secs(raw: &str) -> Option<i64> {
    let raw = raw.trim().replace('T', " ");
    let (date, time) = raw.split_once(' ')?;
    let mut d = date.split('-');
    let year: i64 = d.next()?.parse().ok()?;
    let month: i64 = d.next()?.parse().ok()?;
    let day: i64 = d.next()?.parse().ok()?;
    let mut t = time.split(':');
    let hour: i64 = t.next()?.parse().ok()?;
    let min: i64 = t.next()?.parse().ok()?;
    let sec: i64 = t.next().unwrap_or("0").parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // 民間暦 → 通算日数（Howard Hinnant days_from_civil）。ローカル壁時計を「UTC とみなす」秒へ。
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3_600 + min * 60 + sec)
}

/// MCP ツールの宣言 description を組む（Node `[MCP拡張: {name}] {desc} {confirmNote}`・1000 字切り詰め）。
fn build_description(server: &McpServerRecord, tool_description: Option<&str>) -> String {
    let base = tool_description.filter(|s| !s.is_empty()).unwrap_or("");
    let confirm_note = if server.requires_confirmation == 1 {
        "【重要】この外部ツールを実行する前に、必ず「何をするか（ツール名と渡す引数）」をユーザーに見せて、OK（承認）をもらってから呼ぶこと。"
    } else {
        ""
    };
    let text = format!("[MCP拡張: {}] {base} {confirm_note}", server.name);
    truncate_chars(text.trim(), 1000)
}

/// MCP `inputSchema` を FC ループの `parametersJsonSchema`（フル JSON Schema）へ正規化する。
///
/// **Node との差分（意図的）**: Node は `jsonSchemaToGeminiSchema` で旧 `Schema` 型へ変換し、空プロパティの
/// object は `parameters` 自体を省く。Rust コアの [`FunctionDeclaration::parameters_json_schema`] は「フル
/// JSON Schema を `parametersJsonSchema` へ直送」する契約（[`crate::HttpMcpClient`] 同様に生スキーマを尊重）
/// なので、MCP の `inputSchema` をそのまま載せる（Gemini は JSON Schema を直接受け付ける）。schema 欠落は
/// 空 object（`{"type":"object"}`）へフォールバックする。
fn normalize_schema(input_schema: Option<Value>) -> Value {
    match input_schema {
        Some(v) if v.is_object() => v,
        _ => json!({ "type": "object" }),
    }
}

#[async_trait]
impl ToolProvider for McpProvider {
    fn list(&self, ctx: &ToolContext) -> Vec<FunctionDeclaration> {
        // 露出ゲート（Node `caps.has("mcp")` + 秘書経路）。不可視なら空（宣言も index も作らない）。
        if !self.exposure.is_visible(ctx) {
            return Vec::new();
        }
        self.declarations.clone()
    }

    async fn invoke(
        &self,
        name: &ToolName,
        args: Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        // 露出ゲート（能力/経路）: MCP ツールは秘書経路 + mcp 能力でのみ呼べる。
        if !self.exposure.is_visible(ctx) {
            return Err(ToolError::UnknownTool(name.as_str().to_owned()));
        }
        let Some(entry) = self.dispatch.get(name) else {
            return Err(ToolError::UnknownTool(name.as_str().to_owned()));
        };

        // セキュリティ: 呼び出し時点でも可用性を再検証する（無効化/削除・スコープ・§4.4.3）。発話者は ctx.user_id。
        if !self.is_still_available(entry.server_id, ctx.user_id.as_str()).await {
            return Ok(ToolOutcome::from_payload(json!({
                "success": false,
                "message": "このMCPサーバーは現在利用できません（無効化または削除されています）。",
            })));
        }

        // 最新レコードを取得（削除/認証情報の変化に追随・Node `getServerById`）。
        let fresh = match repo::get_server_by_id(&self.db, entry.server_id).await {
            Ok(Some(s)) => s,
            Ok(None) => {
                return Ok(ToolOutcome::from_payload(json!({
                    "success": false,
                    "message": "MCPサーバーが見つかりません。",
                })));
            }
            Err(e) => {
                return Ok(ToolOutcome::from_payload(json!({
                    "success": false,
                    "message": format!("MCPツール呼び出しに失敗しました: {e}"),
                })));
            }
        };

        // 監査ログ（actor=発話ユーザー・秘密値は含めない・Node `addAuditLog("mcp.call", ...)`）。
        let detail = json!({
            "botId": ctx.bot_id.as_str(),
            "credentialOwner": fresh.user_id,
        })
        .to_string();
        yuuka_auth::audit::add_audit_log(
            &self.db,
            ctx.user_id.as_str(),
            "mcp.call",
            Some(&format!("{}:{}", entry.server_name, entry.tool_name)),
            Some(&detail),
        )
        .await;

        // tools/call を発行（Node `callTool`）。成功/失敗は Node と同じ `{success, ...}` 形状で返す。
        match self.client.call_tool(&fresh, &entry.tool_name, args).await {
            Ok(result) => Ok(ToolOutcome::from_payload(json!({
                "success": true,
                "server": entry.server_name,
                "tool": entry.tool_name,
                // Node は 30000 文字で切り詰める（過大な結果でトークン枠を潰さない）。
                "result": truncate_chars(&result, 30_000),
            }))),
            Err(msg) => Ok(ToolOutcome::from_payload(json!({
                "success": false,
                "message": format!("MCPツール呼び出しに失敗しました: {msg}"),
            }))),
        }
    }
}

impl McpProvider {
    /// 呼び出し時点の可用性再検証（Node `isStillAvailable` クロージャ）。
    ///
    /// スコープ内の可視サーバーに当該 ID があり、かつ enabled なら true。DB エラーは false（安全側）。
    async fn is_still_available(&self, server_id: i64, speaker_user_id: &str) -> bool {
        let servers = match self.scope {
            Scope::SharedSecretary => {
                repo::list_servers_granted_to_bot_scoped(&self.db, &self.bot_id, speaker_user_id).await
            }
            Scope::OwnedBot => repo::list_servers_granted_to_bot(&self.db, &self.bot_id).await,
        };
        match servers {
            Ok(list) => list.iter().any(|s| s.id == server_id && s.enabled == 1),
            Err(e) => {
                tracing::warn!(error = %e, "MCP 可用性の再検証に失敗（利用不可扱い）");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 動的ツール名の生成・衝突退避 ─────────────────────────────────────────────

    #[test]
    fn function_name_basic_and_sanitize() {
        assert_eq!(mcp_function_name(7, "listItems"), "mcp7_listItems");
        // 非 [a-zA-Z0-9_] は `_` へ（`list-items` / `list.items` は同じ形へ潰れる）。
        assert_eq!(mcp_function_name(1, "list-items"), "mcp1_list_items");
        assert_eq!(mcp_function_name(1, "list.items"), "mcp1_list_items");
    }

    #[test]
    fn function_name_truncated_to_63() {
        let long = "a".repeat(200);
        let name = mcp_function_name(1, &long);
        assert_eq!(name.chars().count(), MAX_FUNCTION_NAME_LENGTH);
        assert!(name.starts_with("mcp1_a"));
    }

    #[test]
    fn disambiguate_is_deterministic_and_unique() {
        let mut used = HashSet::new();
        // 衝突源: サニタイズで同名になる 2 ツール。
        let a = mcp_function_name(1, "list-items");
        used.insert(a.clone());
        let b = disambiguate_function_name(1, "list.items", &used);
        assert_ne!(a, b, "退避名は元と異なる");
        assert!(b.chars().count() <= MAX_FUNCTION_NAME_LENGTH);
        // 決定的: 同じ入力・同じ used で同じ結果。
        let b2 = disambiguate_function_name(1, "list.items", &used);
        assert_eq!(b, b2);
        // used に退避名も入れて再度衝突させると別名（_hash_0）になる。
        used.insert(b.clone());
        let c = disambiguate_function_name(1, "list.items", &used);
        assert_ne!(b, c);
        assert!(c.chars().count() <= MAX_FUNCTION_NAME_LENGTH);
    }

    #[test]
    fn short_hash_matches_node_sha1_prefix() {
        // Node: createHash("sha1").update("1:abc").digest("hex").slice(0,6)。
        // sha1("1:abc") = 30d54a1... の先頭 6 桁。決定的・入力で安定。
        let h = short_hash(1, "abc");
        assert_eq!(h.chars().count(), 6);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
        // 同一入力は同一ハッシュ・異なる入力は（ほぼ確実に）別ハッシュ。
        assert_eq!(h, short_hash(1, "abc"));
        assert_ne!(short_hash(1, "abc"), short_hash(2, "abc"));
        assert_ne!(short_hash(1, "abc"), short_hash(1, "abd"));
    }

    // ── スキーマ正規化 ────────────────────────────────────────────────────────

    #[test]
    fn schema_normalization() {
        // object はそのまま。
        let s = json!({ "type": "object", "properties": { "x": { "type": "string" } } });
        assert_eq!(normalize_schema(Some(s.clone())), s);
        // None は空 object。
        assert_eq!(normalize_schema(None), json!({ "type": "object" }));
        // 非 object（配列/文字列）は空 object へフォールバック。
        assert_eq!(normalize_schema(Some(json!([1, 2]))), json!({ "type": "object" }));
        assert_eq!(normalize_schema(Some(json!("x"))), json!({ "type": "object" }));
    }

    // ── description 生成 ─────────────────────────────────────────────────────

    fn rec(id: i64, name: &str, requires_confirmation: i64) -> McpServerRecord {
        McpServerRecord {
            id,
            user_id: Some("alice".to_owned()),
            name: name.to_owned(),
            endpoint_url: "https://mcp.example/mcp".to_owned(),
            auth_credential_encrypted: None,
            auth_credential_iv: None,
            auth_credential_tag: None,
            tools_cache: "[]".to_owned(),
            tools_cache_updated: None,
            requires_confirmation,
            enabled: 1,
            created_at: "now".to_owned(),
        }
    }

    #[test]
    fn description_includes_server_name_and_confirm_note() {
        let d = build_description(&rec(1, "MyMcp", 1), Some("Does things"));
        assert!(d.starts_with("[MCP拡張: MyMcp] Does things"));
        assert!(d.contains("承認")); // requires_confirmation=1 → 確認注記あり。

        // requires_confirmation=0 → 注記なし。description 欠落は空扱い（サーバー名のみ）。
        let d2 = build_description(&rec(2, "NoConfirm", 0), None);
        assert!(d2.starts_with("[MCP拡張: NoConfirm]"));
        assert!(!d2.contains("承認"));
        assert!(d2.chars().count() <= 1000);
    }

    // ── キャッシュ TTL 判定 ───────────────────────────────────────────────────

    #[test]
    fn cache_stale_for_null_and_bad_dates() {
        assert!(cache_is_stale(None), "未取得は stale");
        assert!(cache_is_stale(Some("not-a-date")), "不正日時は stale");
        assert!(cache_is_stale(Some("")), "空は stale");
    }

    #[test]
    fn cache_fresh_when_recent_stale_when_old() {
        // 現在時刻（ローカル壁時計を Unix 秒換算した基準）から数秒前 → fresh。
        let now = now_unix_secs();
        let recent = secs_to_local_string(now - 10);
        assert!(!cache_is_stale(Some(&recent)), "10 秒前は fresh: {recent}");
        // TTL(1h) + 余裕を超えて過去 → stale。
        let old = secs_to_local_string(now - (TOOLS_CACHE_TTL.as_secs() as i64) - 120);
        assert!(cache_is_stale(Some(&old)), "TTL 超過は stale: {old}");
    }

    #[test]
    fn datetime_parse_roundtrip() {
        // parse → 文字列化 → parse が一致（換算の可逆性）。
        let secs = 1_700_000_000;
        let s = secs_to_local_string(secs);
        assert_eq!(parse_local_datetime_secs(&s), Some(secs));
        // `T` 区切りも受ける（Node は " " を "T" へ置換して Date.parse）。
        let with_t = s.replace(' ', "T");
        assert_eq!(parse_local_datetime_secs(&with_t), Some(secs));
    }

    /// テスト用: Unix 秒を `YYYY-MM-DD HH:MM:SS` へ（`parse_local_datetime_secs` の逆写像）。
    fn secs_to_local_string(secs: i64) -> String {
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        let (h, mi, s) = (rem / 3_600, (rem % 3_600) / 60, rem % 60);
        // days_from_civil の逆（civil_from_days）。
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = if m <= 2 { y + 1 } else { y };
        format!("{year:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02}")
    }
}

// ─── provider の list/invoke 組み立て（fake McpClient + 実 SQLite・純ロジック） ─────────────────
#[cfg(test)]
mod provider_tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    use axum::body::Bytes;
    use rusqlite::{params, Connection};
    use yuuka_core::{BotId, CapabilitySet, TurnMode, UserId};

    use super::*;
    use crate::ProxyResponse;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    /// call_tool の呼び出しを記録し、名前ごとに固定応答を返す fake クライアント。
    #[derive(Default)]
    struct FakeMcpClient {
        /// 記録された `(tool_name, arguments)` の履歴。
        calls: Mutex<Vec<(String, Value)>>,
        /// tool_name → 応答（無ければ空文字を返す）。`Err` を返させたい場合は特別名 "boom"。
        replies: Mutex<HashMap<String, String>>,
    }

    #[async_trait]
    impl McpClient for FakeMcpClient {
        async fn probe_dashboard(&self, _s: &McpServerRecord) -> bool {
            false
        }
        async fn fetch_dashboard_html(&self, _s: &McpServerRecord) -> Result<(u16, String), String> {
            Err("unused".to_owned())
        }
        async fn refresh_tools(&self, _s: &McpServerRecord) -> Result<Vec<McpTool>, String> {
            // discover はキャッシュ鮮度で分岐する。ここでは常に「再取得」できるが、テストは
            // tools_cache_updated を新鮮にしてキャッシュヒット経路（DB 由来）で検証する。
            Err("refresh not used in these tests".to_owned())
        }
        async fn call_tool(
            &self,
            _s: &McpServerRecord,
            tool_name: &str,
            arguments: Value,
        ) -> Result<String, String> {
            self.calls
                .lock()
                .unwrap()
                .push((tool_name.to_owned(), arguments));
            if tool_name == "boom" {
                return Err("tool exploded".to_owned());
            }
            let replies = self.replies.lock().unwrap();
            Ok(replies.get(tool_name).cloned().unwrap_or_default())
        }
        async fn proxy(
            &self,
            _s: &McpServerRecord,
            _b: Bytes,
            _a: Option<&str>,
        ) -> Result<ProxyResponse, String> {
            Err("unused".to_owned())
        }
    }

    fn fresh_db() -> (Db, PathBuf) {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_mcp_provider_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        drop(Connection::open(&path).expect("create db file"));
        let db = Db::open(&path).expect("open db");
        // 全テスト共通の FK 充足: alice ユーザー + system_default Bot（owner=alice）。
        seed_base(&path);
        (db, path)
    }

    fn raw(path: &PathBuf) -> Connection {
        Connection::open(path).expect("raw conn")
    }

    /// FK 充足のためユーザーを 1 人挿入する（`mcp_servers.user_id` / `bot_mcp_access.owner_id` の参照先）。
    fn seed_user(path: &PathBuf, discord_id: &str) {
        raw(path)
            .execute(
                "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt, role) \
                 VALUES (?1, ?1, 'h', 'deadbeef', 'user')",
                params![discord_id],
            )
            .expect("seed user");
    }

    /// FK 充足のため Bot を 1 件挿入する（`bot_mcp_access.bot_id` の参照先）。
    fn seed_bot(path: &PathBuf, bot_id: &str, owner_id: &str) {
        raw(path)
            .execute(
                "INSERT OR IGNORE INTO bots (id, user_id, name) VALUES (?1, ?2, ?1)",
                params![bot_id, owner_id],
            )
            .expect("seed bot");
    }

    /// 共通の下ごしらえ: alice ユーザー + system_default Bot（owner=alice）。
    fn seed_base(path: &PathBuf) {
        seed_user(path, "alice");
        seed_bot(path, "system_default", "alice");
    }

    /// enabled・新鮮キャッシュ付きの MCP サーバーを直接挿入し、指定 Bot へ owner 許可を付与する。
    fn seed_server(
        path: &PathBuf,
        user_id: Option<&str>,
        name: &str,
        tools_cache: &str,
        enabled: i64,
    ) -> i64 {
        let conn = raw(path);
        // tools_cache_updated を「今」にしてキャッシュヒット経路（refresh 不要）にする。
        conn.execute(
            "INSERT INTO mcp_servers (user_id, name, endpoint_url, tools_cache, \
             tools_cache_updated, requires_confirmation, enabled) \
             VALUES (?1, ?2, 'https://mcp.example/mcp', ?3, datetime('now','localtime'), 0, ?4)",
            params![user_id, name, tools_cache, enabled],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn grant(path: &PathBuf, bot_id: &str, owner_id: &str, server_id: i64) {
        raw(path)
            .execute(
                "INSERT OR IGNORE INTO bot_mcp_access (bot_id, owner_id, mcp_server_id) \
                 VALUES (?1, ?2, ?3)",
                params![bot_id, owner_id, server_id],
            )
            .unwrap();
    }

    fn secretary_ctx() -> ToolContext {
        let mut c = ToolContext::new(BotId::system_default(), UserId::new("alice"));
        c.mode = TurnMode::Secretary;
        c.capabilities = CapabilitySet::from_granted(vec!["mcp".to_owned(), "secretary".to_owned()]);
        c
    }

    #[tokio::test]
    async fn discover_lists_namespaced_tools_from_cache() {
        let (db, path) = fresh_db();
        let sid = seed_server(
            &path,
            Some("alice"),
            "MyMcp",
            r#"[{"name":"echo","description":"repeat","inputSchema":{"type":"object","properties":{"x":{"type":"string"}}}},{"name":"","description":"drop-empty-name"}]"#,
            1,
        );
        grant(&path, "system_default", "alice", sid);

        let client = Arc::new(FakeMcpClient::default());
        let provider =
            McpProvider::discover(db, client, "system_default", "alice").await;

        let decls = provider.list(&secretary_ctx());
        // name 空は落とす → 1 ツール。名前は `mcp:mcp{id}_echo`。
        assert_eq!(decls.len(), 1);
        let expected = format!("mcp:mcp{sid}_echo");
        assert_eq!(decls[0].name.as_str(), expected);
        assert!(decls[0].description.starts_with("[MCP拡張: MyMcp] repeat"));
        // inputSchema はそのまま parametersJsonSchema へ。
        assert_eq!(decls[0].parameters_json_schema["type"], json!("object"));
        assert!(decls[0].parameters_json_schema["properties"]["x"].is_object());
    }

    #[tokio::test]
    async fn invoke_routes_to_tools_call_and_returns_success_shape() {
        let (db, path) = fresh_db();
        let sid = seed_server(
            &path,
            Some("alice"),
            "MyMcp",
            r#"[{"name":"echo","description":"d"}]"#,
            1,
        );
        grant(&path, "system_default", "alice", sid);

        let client = Arc::new(FakeMcpClient::default());
        client
            .replies
            .lock()
            .unwrap()
            .insert("echo".to_owned(), "hello-from-tool".to_owned());

        let provider =
            McpProvider::discover(db, client.clone(), "system_default", "alice").await;
        let ctx = secretary_ctx();
        let name = ToolName::namespaced("mcp", &format!("mcp{sid}_echo")).unwrap();

        let out = provider
            .invoke(&name, json!({ "x": 1 }), &ctx)
            .await
            .unwrap();
        assert_eq!(out.payload["success"], json!(true));
        assert_eq!(out.payload["server"], json!("MyMcp"));
        assert_eq!(out.payload["tool"], json!("echo"));
        assert_eq!(out.payload["result"], json!("hello-from-tool"));

        // 実際に tools/call が正しい名前と引数で呼ばれた。
        let calls = client.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "echo");
        assert_eq!(calls[0].1, json!({ "x": 1 }));
    }

    #[tokio::test]
    async fn invoke_tool_error_returns_failure_shape() {
        let (db, path) = fresh_db();
        let sid = seed_server(&path, Some("alice"), "MyMcp", r#"[{"name":"boom"}]"#, 1);
        grant(&path, "system_default", "alice", sid);

        let client = Arc::new(FakeMcpClient::default());
        let provider =
            McpProvider::discover(db, client, "system_default", "alice").await;
        let name = ToolName::namespaced("mcp", &format!("mcp{sid}_boom")).unwrap();
        let out = provider
            .invoke(&name, json!({}), &secretary_ctx())
            .await
            .unwrap();
        assert_eq!(out.payload["success"], json!(false));
        assert!(out.payload["message"]
            .as_str()
            .unwrap()
            .contains("tool exploded"));
    }

    #[tokio::test]
    async fn invoke_gated_off_for_generic_mode_and_missing_cap() {
        let (db, path) = fresh_db();
        let sid = seed_server(&path, Some("alice"), "MyMcp", r#"[{"name":"echo"}]"#, 1);
        grant(&path, "system_default", "alice", sid);
        let client = Arc::new(FakeMcpClient::default());
        let provider =
            McpProvider::discover(db, client.clone(), "system_default", "alice").await;
        let name = ToolName::namespaced("mcp", &format!("mcp{sid}_echo")).unwrap();

        // 汎用モードでは MCP ツールは list に出ない。
        let mut generic = secretary_ctx();
        generic.mode = TurnMode::GuildAssistant;
        assert_eq!(provider.list(&generic).len(), 0);
        // 汎用モードでの invoke は UnknownTool（露出ゲート）で弾かれ tools/call は呼ばれない。
        let err = provider.invoke(&name, json!({}), &generic).await.unwrap_err();
        assert!(matches!(err, ToolError::UnknownTool(_)));

        // 秘書経路でも mcp 能力が無ければ隠れる。
        let mut no_cap = secretary_ctx();
        no_cap.capabilities = CapabilitySet::from_granted(vec!["secretary".to_owned()]);
        assert_eq!(provider.list(&no_cap).len(), 0);
        assert!(client.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn disabled_server_is_hidden_at_discovery() {
        let (db, path) = fresh_db();
        // enabled=0 のサーバーは discover の解決時点で除外される（Node filter enabled===1）。
        let sid = seed_server(&path, Some("alice"), "Off", r#"[{"name":"echo"}]"#, 0);
        grant(&path, "system_default", "alice", sid);
        let client = Arc::new(FakeMcpClient::default());
        let provider = McpProvider::discover(db, client, "system_default", "alice").await;
        assert_eq!(provider.list(&secretary_ctx()).len(), 0);
    }

    #[tokio::test]
    async fn shared_secretary_scopes_to_speaker_but_includes_system_level() {
        let (db, path) = fresh_db();
        seed_user(&path, "bob");
        // alice が付与した owner サーバー・bob が付与した owner サーバー・システムレベル(user_id NULL)。
        let alice_sid = seed_server(&path, Some("alice"), "AliceMcp", r#"[{"name":"a"}]"#, 1);
        let bob_sid = seed_server(&path, Some("bob"), "BobMcp", r#"[{"name":"b"}]"#, 1);
        let sys_sid = seed_server(&path, None, "SysMcp", r#"[{"name":"s"}]"#, 1);
        grant(&path, "system_default", "alice", alice_sid);
        grant(&path, "system_default", "bob", bob_sid);
        // システムレベルは grant 不要（user_id IS NULL は全 Bot 利用可）。

        let client = Arc::new(FakeMcpClient::default());
        // 発話者 alice: 自分の許可 + システムレベルのみ（bob の許可は混ざらない）。
        let provider =
            McpProvider::discover(db, client, "system_default", "alice").await;
        let names: Vec<String> = provider
            .list(&secretary_ctx())
            .iter()
            .map(|d| d.name.as_str().to_owned())
            .collect();
        assert!(names.contains(&format!("mcp:mcp{alice_sid}_a")));
        assert!(names.contains(&format!("mcp:mcp{sys_sid}_s")));
        assert!(
            !names.iter().any(|n| n.contains(&format!("mcp{bob_sid}_"))),
            "他人(bob)が付与したサーバーは発話者(alice)へ露出しない: {names:?}"
        );
    }

    #[tokio::test]
    async fn invoke_rejects_when_server_disabled_after_discovery() {
        let (db, path) = fresh_db();
        let sid = seed_server(&path, Some("alice"), "MyMcp", r#"[{"name":"echo"}]"#, 1);
        grant(&path, "system_default", "alice", sid);
        let client = Arc::new(FakeMcpClient::default());
        let provider =
            McpProvider::discover(db, client.clone(), "system_default", "alice").await;
        // 探索後にサーバーを無効化 → 呼び出し時の再検証で弾く（§4.4.3）。
        raw(&path)
            .execute("UPDATE mcp_servers SET enabled = 0 WHERE id = ?1", params![sid])
            .unwrap();
        let name = ToolName::namespaced("mcp", &format!("mcp{sid}_echo")).unwrap();
        let out = provider
            .invoke(&name, json!({}), &secretary_ctx())
            .await
            .unwrap();
        assert_eq!(out.payload["success"], json!(false));
        assert!(out.payload["message"]
            .as_str()
            .unwrap()
            .contains("現在利用できません"));
        // 再検証で弾いたので tools/call は呼ばれない。
        assert!(client.calls.lock().unwrap().is_empty());
    }
}
