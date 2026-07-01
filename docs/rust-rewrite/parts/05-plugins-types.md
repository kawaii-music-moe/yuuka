# yuuka Rust 移行マスタープラン — 第9〜10部

> 対象: **第9部 ユーザー拡張モジュール基盤**（`ToolProvider` / Native / MCP / WASM の三系統）と **第10部 フロント⇄Rust 型連携**（単一真実源・自動生成・機密フェイルクローズ）。
>
> 本部の全決定は [`../00-decisions.md`](../00-decisions.md) の確定選定（#15〜#19）に厳密整合させる。ここでは *再決定はしない* — クレート/バージョン/採否は確定済みとして、設計とコード片へ落とし込む。
> 一次ソース: [`rpt-plugins-wasm-extism`](../verification/rpt-plugins-wasm-extism.md) / [`rpt-mcp-rmcp`](../verification/rpt-mcp-rmcp.md) / [`rpt-gemini-funccalling-mcp`](../verification/rpt-gemini-funccalling-mcp.md) / [`rpt-typegen-tsrs-utoipa`](../verification/rpt-typegen-tsrs-utoipa.md)。
> 現行実装: [`src/functions/registry.ts`](../../../src/functions/registry.ts) / [`src/functions/mcpDynamic.ts`](../../../src/functions/mcpDynamic.ts) / [`src/services/mcpClient.ts`](../../../src/services/mcpClient.ts) / [`src/types/contracts.ts`](../../../src/types/contracts.ts) / [`src/types/apiViews.ts`](../../../src/types/apiViews.ts) / [`frontend/src/lib/api/types.ts`](../../../frontend/src/lib/api/types.ts) / [`frontend/src/lib/api/client.ts`](../../../frontend/src/lib/api/client.ts)。

---

## 9. ユーザー拡張モジュール基盤（将来のカスタム拡張）

### 9.0 現行の姿と移行のゴール

現行 TS は「`FunctionModule` の集合を `buildFunctionRegistry` でマージ」する静的レジストリ（[`registry.ts`](../../../src/functions/registry.ts)）と、「MCP サーバの `tools_cache` から `FunctionDeclaration` を動的生成」する `mcpDynamic`（[`mcpDynamic.ts`](../../../src/functions/mcpDynamic.ts)）＋自前 JSON-RPC クライアント（[`mcpClient.ts`](../../../src/services/mcpClient.ts)）の二層で成り立つ。両者は Gemini の宣言配列と名前→ハンドラの `Map` を吐く点で構造が同じだが、**別コードパスで重複**している。

Rust 版のゴールは、この二つ（＋将来の非信頼ユーザープラグイン）を **単一の `ToolProvider` トレイト**の背後に統一し、中央レジストリが「宣言生成」「ディスパッチ」「namespace 衝突回避」「能力スコープ/データ分離」を*一箇所で*強制することにある。これは確定選定 #15（`ToolProvider` trait レジストリ／Native・MCP・WASM の三系統）の実装計画である。

三系統の役割分担（[`rpt-plugins-wasm-extism`](../verification/rpt-plugins-wasm-extism.md) の安全性ランキングに直結）:

| Provider | 実体 | 信頼境界 | 現行対応 |
|---|---|---|---|
| **NativeProvider** | 内蔵 Rust トレイト | 一級（自作コード） | `src/functions/*` |
| **McpProvider** | rmcp 2.0.0 client（外部 MCP サーバ接続） | **半信頼**（stdio=親権限を継承 / HTTP=別プロセス） | `mcpDynamic` + `mcpClient` |
| **WasmProvider** | Extism 1.30.0（wasmtime 上） | **非信頼**（deny-by-default サンドボックス） | 無（新規） |

> **動的 `.so`（libloading / abi_stable / stabby）は非信頼コードに不採用。** [`rpt-plugins-wasm-extism`](../verification/rpt-plugins-wasm-extism.md) が明言する通り abi_stable は *"doesn't include a sandbox, so if the plugin developer was a malicious actor, they'd have full access to the computer"* — サンドボックス皆無でホスト完全侵害。加えて **panic-across-FFI は UB**、`repr(Rust)` レイアウトは非安定でバージョン不一致 `.so` は silent メモリ破損。abi_stable 自体が 0.11.3（2023-10-12 が最終）で低メンテ。一級（自作）プラグインなら許容だが、ユーザー製カスタム拡張（=非信頼）には**絶対に採用しない**。

---

### 9.1 `ToolProvider` トレイトと `ToolSpec` / `ToolContext` / `ToolOutput`

中核トレイト。`async fn in trait`（Rust 1.96 stable、RPITIT）を使い `#[async_trait]` は不要。エラーは確定選定 #4 に従い **`thiserror` の具体列挙型 `PluginError`**（`anyhow` 禁止）。

```rust
use std::sync::Arc;
use serde_json::Value;

/// ツール1件の宣言（Gemini functionDeclarations 生成の中間表現、provider 非依存）。
pub struct ToolSpec {
    /// 完全修飾ツール名（namespace 接頭辞込み・後述の制約を必ず満たす）。
    pub name: ToolName,
    pub description: String,
    /// フル JSON Schema（parametersJsonSchema へ流す。sanitizer 通過前の生スキーマ）。
    pub parameters_json_schema: Value,
    /// 実行前にユーザー承認を要するか（現行 requires_confirmation 相当）。
    pub requires_confirmation: bool,
}

/// ツール実行結果。テキスト/構造化/マルチモーダル parts を表現できる。
pub struct ToolOutput {
    /// Gemini functionResponse.response に載せる本文（JSON 文字列 or Struct）。
    pub payload: Value,
    /// マルチモーダル functionResponse.parts（Gemini 3。画像等。無ければ空）。
    pub parts: Vec<ResponsePart>,
}

#[async_trait_free] // = 素の async fn in trait（RPITIT）。表記は説明用。
pub trait ToolProvider: Send + Sync {
    /// この provider が現時点で公開する全ツール（namespace 接頭辞は provider 側で付与済み）。
    fn list(&self, ctx: &ToolContext) -> Vec<ToolSpec>;

    /// 単一ツールの実行。name は完全修飾名。args は sanitizer 通過後の引数。
    async fn invoke(
        &self,
        name: &ToolName,
        args: Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, PluginError>;
}
```

`ToolContext` は現行 [`contracts.ts`](../../../src/types/contracts.ts) の `ToolContext` を Rust newtype で写経しつつ、**データ分離キーを型で必須化**する（確定選定「`UserId` newtype を全リポジトリ署名に通す」の延長）。Discord 依存型（`EmbedBuilder` 等）は twilight の型に差し替える（第7部 Discord）。

```rust
/// ツール実行コンテキスト。UserId が全データ分離の必須キー（現行 contracts.ts:ToolContext）。
pub struct ToolContext {
    pub bot_id: BotId,
    /// データ分離キー。全リポジトリ呼び出し・能力スコープ判定に必須（newtype で欠落を型排除）。
    pub user_id: UserId,
    /// ギルド常駐 Bot のみ Some（DM/秘書利用では None）。
    pub guild_id: Option<GuildId>,
    /// この呼び出しに許された能力集合（後述 CapabilitySet。deny-by-default）。
    pub capabilities: CapabilitySet,
    /// リッチ返信キュー（richReplyEnabled=false のとき push 禁止は呼び出し側で担保）。
    pub embeds: Vec<Embed>,
    pub files: Vec<Attachment>,
    pub rich_reply_enabled: bool,
}
```

`PluginError`（層別具体列挙型・`#[non_exhaustive]`。`#[from]` は真の層境界のみ、乱用禁止 — 確定選定 #4）:

```rust
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PluginError {
    #[error("ツール名が制約に違反しています: {0}")]
    InvalidToolName(String),
    #[error("ツールが見つかりません: {0}")]
    UnknownTool(String),
    #[error("このツールは現在利用できません（無効化/削除/権限外）")]
    Unavailable,
    #[error("必要な能力が付与されていません: {capability}")]
    CapabilityDenied { capability: String },
    #[error("引数がスキーマに適合しません: {0}")]
    InvalidArguments(String),
    /// MCP client 層由来（rmcp）。層境界なので #[from] 可。
    #[error(transparent)]
    Mcp(#[from] McpError),
    /// WASM 実行層由来（Extism）。層境界なので #[from] 可。
    #[error(transparent)]
    Wasm(#[from] WasmError),
    #[error("ツール実行がタイムアウトしました")]
    Timeout,
}
```

> **設計上の要点:** 現行 `dispatch` はハンドラ内で `try/catch` してエラーを `{success:false,message}` の JSON 文字列に丸めている（[`registry.ts`](../../../src/functions/registry.ts) L59-71）。Rust では `Result<ToolOutput, PluginError>` を返し、**JSON への丸め込みは第8部 Gemini 側の `functionResponse` 組み立てで一元化**する（provider は握り潰さない＝確定選定「エラー握り潰し禁止」に整合）。ただしクライアントへ返す `message` は `DbError` 等の内部 Display を漏らさず丸める。

---

### 9.2 ツール名 namespace と Gemini 制約の型強制

Gemini の `FunctionDeclaration.name` 制約は [`rpt-gemini-funccalling-mcp`](../verification/rpt-gemini-funccalling-mcp.md) が REST リファレンスから確認済み: **`a-z / A-Z / 0-9 / _ : . -`、最大 128 文字**。現行 TS はここを **63 文字・`[a-zA-Z0-9_]` のみ**という*より厳しい旧制約*で切っている（[`mcpDynamic.ts`](../../../src/functions/mcpDynamic.ts) L19-28）が、これは古い前提。Rust 版では現行の実測制約（128 字 / `:` `.` `-` 許容）へ緩和しつつ、**newtype `ToolName` のスマートコンストラクタで一元検証**して「無効名を型的に作れない」ようにする。

```rust
/// Gemini functionDeclarations.name 制約を満たすことを型で保証する newtype。
/// 制約: [a-zA-Z0-9_:.-] のみ・1..=128 文字（rpt-gemini-funccalling-mcp 実測）。
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ToolName(String);

impl ToolName {
    pub const MAX_LEN: usize = 128;

    /// provider 種別ごとの namespace 接頭辞を付けて構築する。
    /// 例: Native → "native:contacts_save" / MCP → "mcp7:list_items" / WASM → "wasm:<plugin>:run"
    /// 衝突は provider 内で決定的短ハッシュ退避（現行 disambiguateFunctionName 相当）。
    pub fn namespaced(prefix: &str, raw: &str) -> Result<Self, PluginError> {
        // ':' は namespace 区切りとして使うが Gemini は ':' を許すのでそのまま採用可。
        let sanitized: String = raw
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') { c } else { '_' })
            .collect();
        let name = format!("{prefix}:{sanitized}");
        Self::checked(name)
    }

    fn checked(name: String) -> Result<Self, PluginError> {
        let ok = (1..=Self::MAX_LEN).contains(&name.len())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.' | '-'));
        if ok { Ok(Self(name)) } else { Err(PluginError::InvalidToolName(name)) }
    }
    pub fn as_str(&self) -> &str { &self.0 }
}
```

**namespace 接頭辞規約**（衝突回避の核心。現行はサニタイズ/切り詰めで別ツールが同名化し 2 番目が silent に捨てられる事故を `disambiguateFunctionName` で防いでいる — [`mcpDynamic.ts`](../../../src/functions/mcpDynamic.ts) L35-54）:

- `NativeProvider` → `native:<fn>`（例 `native:contacts_save`）
- `McpProvider` → `mcp<serverId>:<tool>`（現行 `mcp{serverId}_{tool}` の踏襲。`:` 区切りへ）
- `WasmProvider` → `wasm:<pluginId>:<export>`

128 字を超える場合／サニタイズ後に同一 provider 内で衝突した場合は、決定的 SHA-1 短ハッシュ（6 桁）を末尾付与して一意化する（現行ロジックを Rust へ 1:1 移植）。中央レジストリは *全 provider 横断*で `HashMap<ToolName, Arc<dyn ToolProvider>>` を張り、二重登録は起動時/生成時にエラー化する（現行 `registry.ts` の重複検出 L28-34 に対応）。

---

### 9.3 中央レジストリ

現行 `buildFunctionRegistry` の役割（宣言集約・重複検出・`has`・`dispatch`）を担う。ただし静的マージではなく **provider の集合を保持し、リクエスト毎に `list`/`invoke` を叩く**（MCP/WASM は動的だから）。

```rust
pub struct ToolRegistry {
    providers: Vec<Arc<dyn ToolProvider>>,
}

impl ToolRegistry {
    /// 全 provider の list を集約し、ToolName→provider の索引を張る。
    /// 名前衝突（provider 横断）は退避名で一意化しつつログ警告（現行 mcpDynamic の挙動）。
    pub fn snapshot(&self, ctx: &ToolContext) -> RegistrySnapshot {
        let mut specs = Vec::new();
        let mut index: HashMap<ToolName, Arc<dyn ToolProvider>> = HashMap::new();
        for p in &self.providers {
            for spec in p.list(ctx) {
                if index.contains_key(&spec.name) {
                    tracing::warn!(tool = spec.name.as_str(), "ツール名衝突: 退避します");
                    // 決定的ハッシュ退避（省略）
                }
                index.insert(spec.name.clone(), Arc::clone(p));
                specs.push(spec);
            }
        }
        RegistrySnapshot { specs, index }
    }
}

pub struct RegistrySnapshot {
    pub specs: Vec<ToolSpec>,           // → 第8部 functionDeclarations 生成へ
    index: HashMap<ToolName, Arc<dyn ToolProvider>>,
}

impl RegistrySnapshot {
    pub async fn dispatch(
        &self,
        name: &ToolName,
        args: Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, PluginError> {
        let provider = self.index.get(name).ok_or_else(|| PluginError::UnknownTool(name.as_str().into()))?;
        provider.invoke(name, args, ctx).await
    }
}
```

> **リクエスト毎の snapshot が正しい理由:** [`rpt-gemini-funccalling-mcp`](../verification/rpt-gemini-funccalling-mcp.md) が指摘する通り、Gemini は tools/config を**インタラクション毎に再送する前提**（Interactions API で `previous_interaction_id` を使っても再送必須）。動的 provider（MCP/WASM）はユーザーのスコープで見えるツールが変わるので、`snapshot(ctx)` を毎回作るのは*意図された設計*でありアンチパターンではない。

---

### 9.4 NativeProvider

現行 `src/functions/*` の内蔵機能（連絡先/家計/予定/配達 等）を束ねる。各機能は Rust トレイト実装で登録し、`ToolSpec` は `schemars` で Rust 引数型から JSON Schema を導出（`parametersJsonSchema` へ直行できるフル JSON Schema）。namespace は `native:`。ここは信頼コードなのでサンドボックス不要、`ctx.user_id` によるデータ分離のみ厳守する。

---

### 9.5 McpProvider — rmcp 2.0.0（公式SDK, client）

確定選定 #16。現行の**自前 JSON-RPC 実装**（[`mcpClient.ts`](../../../src/services/mcpClient.ts) の `rpcRequest`/`parseSseBody`/`ensureInitialized`/`callRpc`）を **rmcp 2.0.0 の client へ全面置換**する。[`rpt-mcp-rmcp`](../verification/rpt-mcp-rmcp.md) の通り rmcp は `modelcontextprotocol` org 所有の公式 SDK、2.0.0（2026-06-29）、spec **2025-11-25** をターゲット、server/client 両対応。

**トランスポート対応**（[`rpt-mcp-rmcp`](../verification/rpt-mcp-rmcp.md)）:

- **stdio**: `TokioChildProcess::new(Command::new("npx")…)` で外部サーバを子プロセス起動。ローカル同梱の一級プラグイン向け。
- **Streamable HTTP**: `StreamableHttpClientTransport`。現行が実装している経路（`endpoint_url` への JSON-RPC over HTTP、SSE 応答パース）はこれに一致 — **自前 SSE パーサ（[`mcpClient.ts`](../../../src/services/mcpClient.ts) L82-109）は rmcp が内包するので破棄**できる。

```rust
use rmcp::{ServiceExt, transport::{TokioChildProcess, StreamableHttpClientTransport}};

pub struct McpProvider {
    /// 接続済みクライアント（1 外部サーバ = 1 client）。tools/list_changed で再取得。
    clients: HashMap<McpServerId, McpClientHandle>,
    /// serverId×tool → 完全修飾 ToolName の対応表（namespace 済み）。
    catalog: HashMap<ToolName, (McpServerId, String /*raw tool name*/)>,
}
```

**現行資産の吸収マッピング:**

| 現行 TS（[`mcpClient.ts`](../../../src/services/mcpClient.ts) / [`mcpDynamic.ts`](../../../src/functions/mcpDynamic.ts)） | rmcp / Rust 版 |
|---|---|
| `rpcRequest` / `parseSseBody` / 手書き JSON-RPC | rmcp `Transport` + `serve()`（自前廃止） |
| `ensureInitialized`（initialize→initialized 通知） | rmcp のハンドシェイクが内包 |
| `listTools` → `tools_cache` | `list_all_tools()`（ページネーション集約込み） |
| `callTool`（content.text 連結・`isError` 判定） | `call_tool()` の結果 content を `ToolOutput` へ |
| `refreshToolsCache` + TTL 1h（[`mcpDynamic.ts`](../../../src/functions/mcpDynamic.ts) L21,177-202） | TTL 再取得は維持しつつ、**`notifications/tools/list_changed` 購読で能動更新**（rmcp 対応。ホットリロード可） |
| `buildAuthHeader`（AES 復号 Bearer 注入） | rmcp transport のヘッダ設定へ移植。in-memory の平文資格情報は **`secrecy::SecretString`** で保持（第10部と共通方針） |
| `McpToolError`（`isError:true` は再試行しない） | `PluginError::Mcp` で表現。副作用ある `tools/call` の二重実行防止方針を維持 |
| SSRF 再検証（`assertSafeOutboundUrl`、DNS リバインディング対策） | **維持必須**。rmcp の HTTP transport 送出直前に宛先再検証フックを噛ませる（プライベート IP 到達遮断は自前 SSRF ガードを移植） |
| `requires_confirmation`（実行前ユーザー承認） | `ToolSpec.requires_confirmation` へ |

**スコープ/データ分離の吸収**: 現行の最重要ロジック — 共有秘書（`system_default`）では**発話者本人が付与した許可分＋システムレベル（`user_id IS NULL`）のみ**に絞り、他人の資格情報を抱えた MCP サーバが発話者の会話へ漏れないようにする（[`mcpDynamic.ts`](../../../src/functions/mcpDynamic.ts) L326-361）— は `McpProvider::list(ctx)` 内で `ctx.user_id` を使って*リクエスト毎に*再評価する。**呼び出し時点の再検証**（現行 `isStillAvailable` クロージャ L271-279）も `invoke` 冒頭で同じスコープ判定を再実行して `PluginError::Unavailable` を返す（無効化/削除/権限外の TOCTOU を塞ぐ）。監査ログ（現行 `mcp.call` に `botId`/`credentialOwner` を記録 L294-302、秘密値は含めない）も踏襲。

**aggregator/gateway パターン**: [`rpt-mcp-rmcp`](../verification/rpt-mcp-rmcp.md) の「virtual MCP server / gateway」= N サーバに接続しツールを集約して単一カタログとして再公開する構図は、まさに McpProvider の設計そのもの。namespace 接頭辞（`mcp<serverId>:`）が gateway の衝突回避に対応する。

---

### 9.6 WasmProvider — Extism 1.30.0（非信頼ユーザープラグイン）

確定選定 #17。**非信頼なユーザー製プラグイン**専用。[`rpt-plugins-wasm-extism`](../verification/rpt-plugins-wasm-extism.md) が示す通り、Extism は wasmtime 上の高レベル抽象で「文字列/バイト/JSON をやり取りする既製 ABI」を提供し、*"fully sandboxes the execution of all plug-in code"*、WASI を superset として持ちつつ**システム資源アクセスは deny-by-default**。polyglot PDK（Rust/Go/JS/Zig 等）でユーザーは好きな言語でプラグインを書ける。

`extism` crate 1.30.0（2026-06-04、host SDK、wasmtime backing）を採用。

```rust
use extism::{Manifest, Wasm, Plugin, PTR};

pub struct WasmProvider {
    plugins: HashMap<PluginId, WasmPluginEntry>,
}

struct WasmPluginEntry {
    manifest: Manifest, // deny-by-default で構築（下記）
    spec: Vec<ToolSpec>,
}
```

**deny-by-default マニフェスト**（能力付与は明示のみ。[`rpt-plugins-wasm-extism`](../verification/rpt-plugins-wasm-extism.md) の `allowed_hosts`/`allowed_paths`/memory/timeout。※ SDK バージョンで正確なフィールド名は docs.rs `Manifest` 要確認＝レポートで MEDIUM フラグ）:

```rust
fn build_sandbox_manifest(wasm: Wasm, grant: &CapabilityGrant) -> Manifest {
    Manifest::new([wasm])
        // 既定は空 = 何も触れない。付与された分だけ開ける（フェイルクローズ）。
        .with_allowed_hosts(grant.allowed_hosts.iter().cloned()) // 送信 HTTP 先の allowlist
        .with_allowed_paths(grant.allowed_paths.iter().cloned()) // FS マッピング（原則 空）
        .with_memory_max(grant.memory_max_pages)                 // 線形メモリ上限
        .with_timeout(grant.timeout)                             // 実行時間上限（無限ループ殺し）
}
```

**host functions**: ユーザープラグインへ「安全に露出してよい能力」だけを host function として渡す（DB 直アクセスは渡さず、`ctx.user_id` スコープ済みの高レベル API のみ）。`UserData` は共有プラグインプールで `Send + Sync` 必須。

**能力モデル / ライフサイクル:**
- プラグインは `Manifest` から `Plugin::new` で生成、更新版バイト列で作り直す＝ホットリロード（[`rpt-plugins-wasm-extism`](../verification/rpt-plugins-wasm-extism.md) は専用 API 無しと注記＝MEDIUM。作り直しで対応）。
- 実行前に `ctx.capabilities`（`CapabilitySet`）と `CapabilityGrant` を突き合わせ、マニフェストへ反映。付与外の host/path はマニフェストに載らない＝**構造的に到達不能**。
- タイムアウト/メモリ超過は `WasmError` → `PluginError::Wasm`/`Timeout` へ。プロセスは殺さない（wasmtime のサンドボックス内で完結）。

> **標準志向の代替（記録のみ・初期不採用）:** 生 wasmtime 46 + Component Model + WASI 0.2（stable）。WIT + `wit-bindgen 0.58` で型付きインターフェース、fuel/epoch/`ResourceLimiter` で細粒度の資源制御。Extism より定型コードは増えるが標準トラック。確定選定は Extism 優先（ergonomics）。**Extism は独自 ABI で Component Model 非採用**（[`rpt-plugins-wasm-extism`](../verification/rpt-plugins-wasm-extism.md) MEDIUM）である点は将来の移行検討事項として記録。

---

### 9.7 第8部 Gemini との接続（宣言生成・並行相関・sanitizer）

第8部（Gemini）から見た本基盤の接続点:

1. **リクエスト毎の宣言生成**: `RegistrySnapshot::specs` → 各 `ToolSpec.parameters_json_schema` を **`parametersJsonSchema`**（フル JSON Schema）へ載せる。[`rpt-gemini-funccalling-mcp`](../verification/rpt-gemini-funccalling-mcp.md) の最重要事実 — `parametersJsonSchema` は `$ref`/`$defs`/`additionalProperties`/`prefixItems` を許容しバックエンドへ直送されるため、旧 `parameters`（OpenAPI 3.0.3 サブセット）路より sanitize が少なくて済む。現行 `jsonSchemaToGeminiSchema`（[`mcpDynamic.ts`](../../../src/functions/mcpDynamic.ts) L60-174）の重い down-conversion は**大幅に不要化**する。

2. **sanitizer（それでも必要）**: `$schema` 除去（ルートで拒否される）、`default` の除去/変換、`oneOf`/`allOf`→`anyOf` 畳み込み、`additionalProperties` エッジケースのガード、過度なネストの平坦化。provider 非依存の共通 sanitizer を snapshot→宣言生成の間に一段挟む（LibreChat 等が実装する定石）。

3. **並行相関 `functionCall.id`**: [`rpt-gemini-funccalling-mcp`](../verification/rpt-gemini-funccalling-mcp.md) の通り 2026-03-17 更新で per-call `id` が正式追加。並行呼び出しでは 1 応答に複数 `functionCall` パートが返る。各 `functionCall`(`name`,`args`,`id`) を `RegistrySnapshot::dispatch` へ回し、`ToolOutput` を `functionResponse`(`name`,`response`,同一 `id`；マルチモーダルは `parts[]`) へ組み立てる。`id` を保持して相関を崩さない。ループは `maxIterations` で制限（`ANY` モードの無限ループ失敗モード対策）。

4. **`functionCallingConfig` モード**: `AUTO`（既定）/`ANY`+`allowedFunctionNames`（完了ハルシネーション是正）/`NONE`/`VALIDATED`(Preview)。`allowedFunctionNames` には `RegistrySnapshot` の namespace 済み `ToolName` をそのまま流せる。

> **MCP のもう一つの配線（不採用の記録）:** Gemini REST には `Tool.mcpServers[]`（サーバ側 MCP 委譲）や SDK の `ClientSession` 直渡し（クライアント側 auto-FC）がある（[`rpt-gemini-funccalling-mcp`](../verification/rpt-gemini-funccalling-mcp.md)）。しかし本基盤は **MCP を単なる `ToolProvider` として自前ループで扱う**方針（WASM/Native と統一的な sanitize・`id`・namespace・スコープ管理を効かせるため）。Google 委譲はコードは減るが per-call 制御を失い SDK experimental のため採らない。

---

### 9.8 初期スコープの提案（[`00-decisions.md`](../00-decisions.md) 未解決 #5 への回答）

未解決事項 #5（3系統を最初から揃えるか、WASM を後続にするか）に対する本部の推奨:

- **フェーズ A（移行同時）**: NativeProvider ＋ McpProvider を先行。これで現行 `src/functions/*` と `mcpDynamic`/`mcpClient` のパリティを達成（機能後退ゼロ）。
- **フェーズ B（後続）**: WasmProvider を追加。非信頼プラグインは新機能であり現行に対応物が無く、サンドボックス/能力モデル/マニフェスト UI 等の追加設計を要するため、パリティ達成後に切り出すのがリスク最小。トレイト境界（`ToolProvider`）は最初から確定させておくので、B の追加は*レジストリへ provider を 1 つ足すだけ*で済む。

---

## 10. フロント⇄Rust 型連携（単一真実源・自動生成・機密フェイルクローズ）

### 10.0 現行の二重管理と移行のゴール

現行フロントは手書きの [`frontend/src/lib/api/types.ts`](../../../frontend/src/lib/api/types.ts)（655 行）に `BotView` 等を**手で写経**しており、サーバ側の zod ビュー（[`apiViews.ts`](../../../src/types/apiViews.ts)）と**二重管理**になっている（`BotView` が両側に存在。ズレたら実行時まで気付けない）。[`rpt-typegen-tsrs-utoipa`](../verification/rpt-typegen-tsrs-utoipa.md) を根拠に、これを **Rust を単一真実源とする自動生成**へ置換する（確定選定 #18）。

### 10.1 第一推奨: ts-rs 12.0.1（型のみ・単一真実源）

確定選定 #18。[`rpt-typegen-tsrs-utoipa`](../verification/rpt-typegen-tsrs-utoipa.md) の通り ts-rs **12.0.1**（2026-01-31、stable、活発、`Aleph-Alpha/ts-rs`）は「Rust HTTP バックエンド + Vite SPA の共有型」に最も直接的に合致。`#[derive(TS)]` ＋ `#[ts(export)]` ＋ `cargo test` で `./bindings` へ `.ts` を書き出す（`TS_RS_EXPORT_DIR` で出力先変更可）。serde 属性（`rename`/`rename_all`/`tag`/`content`/`untagged`/`skip`/`flatten`/`default` 等）を尊重する。

```rust
use ts_rs::TS;
use serde::Serialize;

/// GET /api/bots 等が返す Bot ビュー DTO。現行 apiViews.ts:botViewSchema と 1:1。
/// 機密列（*_encrypted / *_iv / *_tag）は「フィールドとして存在しない」＝生成 TS にも現れない。
#[derive(Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")] // 現行 JSON キー（snake_case）を維持
pub struct BotView {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub recommended_persona_id: Option<i64>,
    pub persona_id: Option<i64>,
    pub capabilities: String,
    pub discord_username: Option<String>,
    pub discord_avatar_url: Option<String>,
    pub discord_application_id: Option<String>,
    pub suspended: i64,
    pub created_at: String,
    pub updated_at: String,
    pub preset: String,
    pub preset_display_name: String,
    pub has_gemini_key: bool,
    pub has_token: bool,
    pub running: bool,
    pub connected: bool,
    pub shared: bool,
}
```

> **ts-rs の落とし穴（[`rpt-typegen-tsrs-utoipa`](../verification/rpt-typegen-tsrs-utoipa.md) HIGH）:** `skip_serializing`/`skip_serializing_if` は **`#[serde(default)]` 併用時のみ**生成型へ反映される。除外したいだけなら `#[ts(skip)]` を使う。ただし後述の通り、機密は**そもそも DTO のフィールドに置かない**方針なのでこの落とし穴には基本触れない。

**上位互換の選択肢（[`00-decisions.md`](../00-decisions.md) 未解決 #1）**: エンドポイント契約＋型付きクライアントまで欲しい場合は `utoipa 5.5.0 → OpenAPI 3.1 → openapi-typescript 7.13.0(+openapi-fetch)`。utoipa は `utoipa-axum` で axum ハンドラ登録と spec 生成を同時に行える。ts-rs は「型のみ」なので契約（どのルートがどの型を返すか）は型で縛れない — 現行 [`client.ts`](../../../frontend/src/lib/api/client.ts) の `api.get<T>()` のように *呼び出し側が T を指定*する形は残る。**既定は ts-rs**（低リスク・二重管理解消が主目的）、契約まで必要になった時点で utoipa へ格上げ。

> **specta 却下理由（[`rpt-typegen-tsrs-utoipa`](../verification/rpt-typegen-tsrs-utoipa.md)）:** v2 は 2026-07 時点で **RC のまま安定版なし**（`max_stable_version` は 1.0.5）。強みは Tauri 特化（typed commands/events）でブラウザ SPA には効かない。E2E RPC の要 rspc は **2025-03-12 に公式にメンテ終了**。本スタック（Rust HTTP + Svelte/Vite）には不適。

### 10.2 機密フェイルクローズ — 「専用 DTO struct」で構造的保証

確定選定 #19。現行の防御は zod allowlist（[`apiViews.ts`](../../../src/types/apiViews.ts)）: `z.object()` がスキーマ外キーを strip し、生 DB レコードが紛れても機密列（`*_encrypted`/`*_iv`/`*_tag`・`password_hash`・`salt`）が応答に出ない — *実行時*のフェイルクローズ。

Rust 版はこれを **コンパイル時保証**へ格上げする。核心は「**機密列を DTO struct のフィールドに存在させない**」こと。DB/ドメインモデル（機密列を持つ）と応答 DTO（安全列のみ）を**別 struct**にし、`Serialize`/`TS` は DTO 側のみに実装する。機密フィールドは*型に存在しない*ので、

- **シリアライズできない**（存在しないフィールドは JSON に出しようがない＝真のフェイルクローズ by construction）、
- **生成 TS にも現れない**（漏洩が型的に不可能）、
- 将来 DB に機密列を足しても、DTO に手で足さない限り**露出経路が生まれない**（zod allowlist は「新機密列を strip し忘れなければ安全」だが、DTO 分離は「足し忘れる＝安全側」）。

```rust
// ドメイン/DB モデル（機密列を持つ。Serialize も TS も導出しない＝配線ミスでも wire に出せない）
pub struct BotRecord {
    pub id: String,
    pub discord_token_encrypted: Vec<u8>, // ← 機密。DTO には存在しない
    pub gemini_key_encrypted: Vec<u8>,    // ← 機密
    pub gemini_key_iv: Vec<u8>,
    // ...安全列...
}

impl BotRecord {
    /// 応答 DTO への明示投影。機密列はここで「単に載せない」。
    pub fn to_view(&self, runtime: &BotRuntimeState) -> BotView {
        BotView {
            id: self.id.clone(),
            // 機密列は has_* の真偽値のみ公開（現行 has_gemini_key / has_token と同じ思想）
            has_gemini_key: !self.gemini_key_encrypted.is_empty(),
            has_token: !self.discord_token_encrypted.is_empty(),
            running: runtime.running,
            connected: runtime.connected,
            // ...安全列を明示コピー...
        }
    }
}
```

**in-memory 機密は `secrecy`**（[`rpt-typegen-tsrs-utoipa`](../verification/rpt-typegen-tsrs-utoipa.md)）: `SecretString`/`SecretBox` は `Debug` を redact し drop で zeroize、`expose_secret()` でのみ露出。復号後の Discord トークン・Gemini キー・MCP 資格情報（§9.5）をメモリ保持する間はこれで包む。ログ/`Debug` 経由の漏洩も塞ぐ。

**`ApiResponse<T>` の型化**: 現行はレスポンスが `{ success, message? } & payload` の**トップレベル直置き**（`data` ラッパ不在。[`frontend/src/lib/api/types.ts`](../../../frontend/src/lib/api/types.ts) L13）。この形をサーバ側 Rust の serde でそのまま出し、フロントの `ApiResponse<T>` エンベロープ型も生成物と整合させる（`#[serde(flatten)]` でエンベロープと payload を平坦化するか、共通ラッパ struct を ts-rs で export）。現行 [`client.ts`](../../../frontend/src/lib/api/client.ts) の「`res.ok` と `data.success` の複合成否判定」はそのまま維持できる。

### 10.3 ビルド統合と CI ドリフト検出

**生成**（[`rpt-typegen-tsrs-utoipa`](../verification/rpt-typegen-tsrs-utoipa.md) の canonical レシピ）:

- ts-rs: `#[ts(export)]` を付けて `cargo test`（または `xtask` バイナリ）で `./bindings` へ書き出し → Vite/tsconfig をそのディレクトリへ向ける（または `frontend/src/lib/api/generated/` へコピー）。生成物は git 追跡する。
- utoipa 経路（採る場合）: `ApiDoc::openapi()` を `openapi.json` へ直列化 → `npx openapi-typescript ./openapi.json -o ./src/lib/api/schema.d.ts`。

**CI ドリフト検出**（確定選定 #19 の「生成→`git diff --exit-code`」）:

```bash
# ts-rs
cargo test export_bindings          # ./bindings を再生成
git diff --exit-code -- ./bindings  # 差分あれば非0終了＝生成物が古い
# utoipa 経路なら openapi.json と schema.d.ts の両層で同じガード
```

`xtask` にまとめて `cargo xtask gen-bindings` の一手で回せるようにし、CI ジョブと `pre-commit` の双方から呼ぶ。

### 10.4 移行中の手書き types.ts との併存（strangler 期）

第11部（デプロイ・段階移行、nginx strangler）と整合させる。移行はルート単位で進むので、型も**ルート単位で切り替える**:

1. **DTO を Rust 側に定義した順**に ts-rs で生成 → `frontend/src/lib/api/generated/*.ts` へ。
2. フロントは当該ルートの import を手書き [`types.ts`](../../../frontend/src/lib/api/types.ts) から `generated/` へ**1 型ずつ差し替え**る。手書き `types.ts` は「まだ Rust 化していないルートの型」だけを残す縮小するファイルとして併存させる。
3. 差し替え済み型は手書き側から削除（重複定義でのズレを防ぐ）。両側に同名 `BotView` が残る期間は、生成物を*正*とし手書きを段階的に消す。
4. 全ルート移行完了時点で手書き `types.ts` は `ApiResponse<T>` 等の生成不能な純ユーティリティ型のみへ縮小、または消滅。

これにより、Node と Rust が nginx 背後で混在する strangler 期でも「移行済みルートは Rust 生成型・未移行ルートは手書き型」がファイル分離で共存し、CI ドリフト検出は生成対象ディレクトリ（`generated/`）のみに掛けて誤検知を避ける。

---

## 付録: 本部が参照する確定選定・一次ソースの対応表

| 本部の設計要素 | 確定選定 | 一次ソース |
|---|---|---|
| `ToolProvider` trait / 三系統 | #15 | [`rpt-plugins-wasm-extism`](../verification/rpt-plugins-wasm-extism.md) |
| McpProvider（rmcp 2.0.0 client） | #16 | [`rpt-mcp-rmcp`](../verification/rpt-mcp-rmcp.md) |
| WasmProvider（Extism 1.30.0） | #17 | [`rpt-plugins-wasm-extism`](../verification/rpt-plugins-wasm-extism.md) |
| `.so` 不採用 | #15 注記 | [`rpt-plugins-wasm-extism`](../verification/rpt-plugins-wasm-extism.md) §3 |
| 宣言生成 / `parametersJsonSchema` / `id` 相関 | — | [`rpt-gemini-funccalling-mcp`](../verification/rpt-gemini-funccalling-mcp.md) |
| ts-rs 12.0.1（型のみ） | #18 | [`rpt-typegen-tsrs-utoipa`](../verification/rpt-typegen-tsrs-utoipa.md) |
| utoipa 上位互換 / specta 却下 | #18 | [`rpt-typegen-tsrs-utoipa`](../verification/rpt-typegen-tsrs-utoipa.md) |
| 専用 DTO / secrecy | #19 | [`rpt-typegen-tsrs-utoipa`](../verification/rpt-typegen-tsrs-utoipa.md) §fail-closed |
