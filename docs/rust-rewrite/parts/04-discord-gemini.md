# 第8部 — Discord + Gemini / Function Calling

> 本書は [`00-decisions.md`](../00-decisions.md) の確定選定（#12 Discord=twilight 0.17.1 / #13 Gemini=自前 reqwest ラッパ / #14 API面=classic generateContent / #15 ToolProvider）を実装レベルへ落とす。一次ソース照合は [`verification/rpt-discord-serenity-twilight.md`](../verification/rpt-discord-serenity-twilight.md), [`verification/rpt-gemini-design-verify.md`](../verification/rpt-gemini-design-verify.md), [`verification/rpt-gemini-funccalling-mcp.md`](../verification/rpt-gemini-funccalling-mcp.md), [`verification/rpt-gemini-rest-crates.md`](../verification/rpt-gemini-rest-crates.md) を参照。現行実装は [`src/bot.ts`](../../../src/bot.ts), [`src/gemini.ts`](../../../src/gemini.ts), [`src/services/llmClient.ts`](../../../src/services/llmClient.ts)。
>
> 絶対制約（[`00-decisions.md`](../00-decisions.md) 冒頭）に整合させる：厳格エラー（thiserror 具体列挙型・握り潰し禁止）／常時稼働・自己復帰（supervisor + バックオフ、致命設定のみ fail-fast）／マルチスレッド／ユーザー製カスタムモジュール拡張性。本部は第5部（supervisor）・第9部（ToolProvider）と接続する。

---

## 8.0 現行アーキテクチャの要約（移植対象の1:1マップ）

移植の忠実度を担保するため、現行 TS の構造を先に確定させる。

**Discord 側（[`src/bot.ts`](../../../src/bot.ts)）**
- `discord.js` v14。**共有デフォルトクライアント** `client`（`system_default`）＋ **ユーザー別カスタムクライアント** `customClients: Map<botId, Client>`（[`src/bot.ts:78-81`](../../../src/bot.ts)）。
- `getBotClientForUser(botId)`：カスタムが ready なら優先、無ければデフォルト（[`src/bot.ts:87-93`](../../../src/bot.ts)）。
- `startCustomBot` は `startInFlight: Map<botId, Promise>` で**起動を直列化**（連打・restart 競合で destroy されない Client が Gateway に残り二重応答するのを防ぐ [`src/bot.ts:1320-1406`](../../../src/bot.ts)）。
- `restartDefaultBot(token)`：v14 は destroy 済み Client の再ログインを保証しないため**新インスタンスへ差し替え**、ESM live-binding で参照側へ反映（[`src/bot.ts:1427-1451`](../../../src/bot.ts)）。→ **twilight ではこの制約自体が消える**（Shard は値、トークン差し替えは新 Shard を spawn するだけ）。
- `claimMessageOnce(botUserId, messageId)`：`bot user id : message id` の TTL 冪等ガード（同一 identity の Client 重複時の二重応答防止 [`src/bot.ts:939-956`](../../../src/bot.ts)）。
- ボタン：`handleInteraction`（`share_accept`/`share_decline`/`memreq_*`/`persona_import`。`customId` を `action:id:extra` で分解、`interaction.update`/`reply({ephemeral})`/`followUp` [`src/bot.ts:377-532`](../../../src/bot.ts)）。
- プレゼンス演出 `setBotStatus`（thinking/writing/idle [`src/bot.ts:151-188`](../../../src/bot.ts)）、プロフィール同期（起動時＋1時間 [`src/bot.ts:195-232`](../../../src/bot.ts)）、`sendTyping` 5秒維持、2000字分割 `splitMessage`。

**Gemini 側（[`src/gemini.ts`](../../../src/gemini.ts), [`src/services/llmClient.ts`](../../../src/services/llmClient.ts)）**
- `@google/generative-ai`（**廃止予定の旧 Node SDK**）＋ classic `generateContent`。モデル `gemini-3.1-flash-lite`（[`src/services/llmClient.ts:19,48`](../../../src/services/llmClient.ts)）。
- ユーザー別／Bot別に API キーをキャッシュ（`userAICache`/`botAICache`、キー変更で無効化 [`src/services/llmClient.ts:6-16`](../../../src/services/llmClient.ts)）。
- `generateWithRetry`：429/5xx を `RetryInfo.retryDelay` 優先＋指数バックオフでリトライ、120s タイムアウト（[`src/gemini.ts:399-464`](../../../src/gemini.ts)）。
- `runFunctionCallingLoop`：`maxIterations=10`、`functionCall`↔`functionResponse` 往復、**完了ハルシネーション是正**（`claimsActionCompleted` 検知→`mode:ANY`+`allowedFunctionNames` で1回だけ強制、`maxCorrectionAttempts=2` [`src/gemini.ts:501-746`](../../../src/gemini.ts)）。
- 3経路（秘書 `processMessage` / ギルド `processGuildMessage` / owner DM `processBotDmMessage`）が `runPlannedTurn` を共有（[`src/gemini.ts:793-926`](../../../src/gemini.ts)）。
- レジストリ `buildFunctionRegistry`：`declarations` と `dispatch(ctx, name, args)`、名前重複を throw（[`src/functions/registry.ts`](../../../src/functions/registry.ts)）。MCP は `getMcpFunctionModuleForBot` で動的マージ（[`src/functions/mcpDynamic.ts`](../../../src/functions/mcpDynamic.ts)）。

---

## 8.1 Discord（twilight 0.17.1）

### 8.1.1 crate 構成と却下理由

**採用：twilight 0.17.1**（[`verification/rpt-discord-serenity-twilight.md`](../verification/rpt-discord-serenity-twilight.md) で crates.io API 実測）。

| crate | version | 役割 |
|---|---|---|
| `twilight-gateway` | **0.17.1**（2025-12-13） | WebSocket。`Shard` = 1 ゲートウェイセッション。`create_recommended`/`create_iterator`、IDENTIFY レート制限 `Queue`。 |
| `twilight-http` | **0.17.1** | REST。`Client` / `Client::interaction(app_id)` → `InteractionClient`。 |
| `twilight-model` | **0.17.1** | 純データ型。`Interaction`, `Component`/`Button`, `http::interaction::{InteractionResponse, InteractionResponseType, InteractionResponseData}`。I/O 無し。 |
| `twilight-cache-inmemory` | **0.17.1**（任意） | 独立キャッシュ。**テナント別 or 無しを明示選択**（後述）。 |
| `twilight-util` | **0.17.0**（0.17.1 バンプ無し＝正） | builder（`InteractionResponseDataBuilder`）、permission 計算。MSRV 1.79。 |

**serenity 0.12.5 却下理由**（[`verification/rpt-discord-serenity-twilight.md`](../verification/rpt-discord-serenity-twilight.md)）：
1. **1 Client = 1 token = 1 所有イベントループ**が前提。現行 `customClients`（N テナント）＝ N 個の独立 Client 構築で「against the grain」。
2. 各 Client が既定で**自前 `Arc<Cache>`＋`Arc<Http>`** を抱える → N テナントでメモリ倍増。
3. sharding/再接続がライブラリ内部に隠蔽され、**テナント個別の再起動・バックオフポリシーが掛けにくい**（絶対制約2＝自己復帰と衝突）。

twilight は gateway/http/model/cache が疎結合で、`Shard` は「値として所有し poll するだけ」。N テナント = N Shard を各 tokio task で保持し、**HTTP は共有 / cache はテナント別 or 無し**を明示制御できる。

### 8.1.2 caller 駆動 poll loop と supervisor 統合（絶対制約2の核）

twilight `Shard` は再接続・resume を**内蔵するが、caller が poll し続ける間のみ**動く（[`verification/rpt-discord-serenity-twilight.md`](../verification/rpt-discord-serenity-twilight.md) の Shard docs 引用）：

> "Shards start out disconnected, but will on the first successful call to `poll_next` try to reconnect… `poll_next` must then be repeatedly called in order for the shard to maintain its connection and update its internal state."

これは自律バックグラウンド再接続では**なく caller 駆動**。イディオムは「per-shard ループでエラーを `Result` アイテムとして log-and-continue、次 poll で shard 自身が resume」。この形が**第5部 supervisor（自前 `JoinSet` + 指数バックオフ再 spawn）と直結**し、テナント別バックオフ／再起動／劣化縮退を掛けられる（serenity では困難）。

**テナント別 Shard を supervisor 配下の tokio task で poll する設計：**

```rust
// tenant = 現行 customClients の1エントリ（system_default 含む）
struct TenantBot {
    bot_id: BotId,                 // "system_default" | Uuid 等
    token: Secret<String>,         // secrecy（復号済みトークンをログに出さない）
    intents: Intents,             // GUILDS|GUILD_MESSAGES|MESSAGE_CONTENT|DIRECT_MESSAGES
    http: Arc<twilight_http::Client>, // ★共有可（下記）
    // cache: Option<Arc<InMemoryCache>>  // テナント別 or None（下記）
}

// supervisor（第5部）が各テナントごとに spawn。JoinError(is_panic) を検知したら
// 指数バックオフで再 spawn。ここでは shard 内部エラーはループ内で吸収する。
async fn run_tenant(bot: TenantBot, mut shutdown: ShutdownToken) -> Result<(), DiscordError> {
    let mut shard = Shard::new(ShardId::ONE, bot.token.expose_secret().to_owned(), bot.intents);
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break, // tokio-graceful-shutdown からの伝播
            item = shard.next_event(EventTypeFlags::all()) => {
                match item {
                    Ok(event) => {
                        // spawn せず順次 or 明示 spawn。二重応答冪等ガード（8.1.5）を通す。
                        if let Err(e) = handle_event(&bot, event).await {
                            // ★握り潰さない：DiscordError variant をログ・メトリクス化して continue
                            tracing::warn!(bot_id = %bot.bot_id, error = %e, "event handling failed");
                        }
                    }
                    Err(source) => {
                        // 接続系エラーは致命ではない。次 poll で shard が resume/reconnect する。
                        tracing::warn!(bot_id = %bot.bot_id, error = %source, "shard error, continuing");
                        // fatal（例: 無効トークン）のみ break して supervisor に再起動判断を委ねる
                        if source.is_fatal() { return Err(DiscordError::ShardFatal { bot_id: bot.bot_id.clone() }); }
                    }
                }
            }
        }
    }
    Ok(())
}
```

**トークンローテーション（現行 `restartDefaultBot` [`src/bot.ts:1427`](../../../src/bot.ts) の置換）**：v14 の「destroy 済み Client は再ログイン不可」制約が消えるため、**当該テナントの task を supervisor 経由で停止 → 新トークンで新 Shard を張る**だけ。ESM live-binding のトリックは不要。`restartDefaultBot` は「supervisor に `RestartTenant{bot_id, new_token}` を送る」IPC に置換。

**起動直列化（現行 `startInFlight` [`src/bot.ts:1320`](../../../src/bot.ts)）**：Rust では supervisor が `HashMap<BotId, JoinHandle>` を単一 owner タスクで持ち、`spawn`/`abort` を直列化するので `startInFlight` は自然消滅（同一 bot の二重 Shard が構造的に起き得ない）。

### 8.1.3 HTTP クライアント共有・キャッシュ方針（明示）

- **`twilight_http::Client` は全テナントで1個を共有**（現行の各 Client 自前 Http を集約）。ただし**トークンはテナント別**なので、送信時にトークンを差し替える運用が要る。twilight-http は「proxying で多サービスがレート予算を共有」できる設計だが、**Bot API のレート制限はトークン単位**であるため、実装上は次のどちらか：
  - (a) **テナントごとに `twilight_http::Client` を1個**（トークン別・レート bucket 別。メモリは小さく、これが素直）。
  - (b) 共有1個＋リクエスト毎トークン注入（twilight の proxy 機構前提。複雑）。
  → **既定は (a)**（現行の「Client ごとに Http」を素直に踏襲・レート境界がトークンと一致）。共有は将来最適化。
- **キャッシュ：既定は「無し」**。現行 `getGuildOptionsForBot`（[`src/bot.ts:116-149`](../../../src/bot.ts)）が `guild.roles.fetch()`＋`members.cache` を読む用途に限り、**テナント別 `InMemoryCache`（`ResourceType::{ROLE, MEMBER, GUILD}` に絞る）** を任意で持つ。ロールは Guilds インテントで完全取得（REST fetch でも可）、メンバーは GuildMembers 特権インテント無しでキャッシュ済みのみ（現行と同じ不完全性 → UI で ID 手入力フォールバック）。twilight のキャッシュは**明示 feed**（イベントを自分で入れる）なので、不要なテナントは持たない＝メモリ節約。

### 8.1.4 ボタンインタラクション（`InteractionResponse`）

現行 [`src/bot.ts:377-532`](../../../src/bot.ts) の `handleInteraction` を twilight-model + `InteractionClient` へ 1:1 移植する。

| 現行（discord.js） | twilight | 用途 |
|---|---|---|
| `interaction.reply({ content, ephemeral: true })` | `InteractionResponseType::ChannelMessageWithSource` + `InteractionResponseData{ flags: MessageFlags::EPHEMERAL, .. }` | 申請情報不正・非対象招待の通知 |
| `interaction.update({ content, components: [] })` | `InteractionResponseType::UpdateMessage` | 承認/辞退でボタンを消して結果表示（`share_accept`/`memreq_*`/`persona_import`） |
| `interaction.followUp({ content, components })` | `InteractionClient::create_followup(token)` | 推奨ペルソナのインポート確認 |

```rust
let interaction_client = http.interaction(application_id);
let resp = InteractionResponse {
    kind: InteractionResponseType::UpdateMessage,
    data: Some(InteractionResponseDataBuilder::new()  // twilight-util
        .content(format!("✅ Bot「**{name}**」へのアクセスが有効になりました！"))
        .components([])                                // ボタン除去
        .build()),
};
interaction_client.create_response(interaction.id, &interaction.token, &resp).await?;
```

`customId` 分解（`action:id:extra`）はそのまま：`custom_id.splitn(3, ':')`。`isButton()` は `interaction.data` が `InteractionData::MessageComponent` かつ `component_type == Button` で判定。エラー時の握り潰し（現行の空 `catch{}`）は**しない**：`DiscordError` にして log、必要時のみ ephemeral エラー返信。

### 8.1.5 二重応答冪等ガード・その他ユーティリティ

- `claimMessageOnce`（[`src/bot.ts:939-956`](../../../src/bot.ts)）：`Mutex<HashMap<(BotUserId, MessageId), Instant>>` + TTL 60s。または `moka`（TTL キャッシュ）で置換。supervisor が同一 Shard 重複を構造的に防ぐため**多重防御**として残す。
- `sendTyping` 5秒維持：`tokio::time::interval` + `shard`/`http` で `create_typing_trigger`。task ローカルで `select!` 終了アームと共に落とす（現行 finally の typingInterval clear に相当）。
- `splitMessage`（2000字・改行境界 [`src/bot.ts:1268-1288`](../../../src/bot.ts)）・`setBotStatus`（presence [`src/bot.ts:151`](../../../src/bot.ts)）はロジック直移植。
- 添付取得（画像/音声を fetch→base64）：`reqwest` で URL 取得 → `base64` encode（8.2.5 参照）。`SUPPORTED_AUDIO_TYPES`（[`src/bot.ts:61-72`](../../../src/bot.ts)）は定数配列へ。

---

## 8.2 Gemini（自前 reqwest 0.13 ラッパ）

### 8.2.1 crate 選定と却下理由

**採用：reqwest 0.13.4 + serde/serde_json + thiserror の薄い自前クライアント**（[`verification/rpt-gemini-rest-crates.md`](../verification/rpt-gemini-rest-crates.md)）。

- **公式 Rust SDK は不在**（Python/JS/Go/Java/C# のみ）。
- `google-generative-ai-rs`：**アーカイブ済（2025-07）・FC 未実装＝不適**。
- `gemini-rust 1.7.1`：機能豊富だが 3rd-party 保守依存、**厳格エラー方針（thiserror）と API 面固定（generateContent レガシー固定）が実コードで未確認＝予備**。
- 自前ラッパの理由：(1) 表面積が小さい（generateContent / streamGenerateContent の2エンドポイント＋Part/Content/Tool 群のみ）、(2) **429/`RetryInfo.retryDelay`/5xx/JSON deser/timeout を thiserror variant で完全掌握**（現行 rate-limit バックオフを 1:1 移植可）、(3) FC ループの並行実行・`mode:ANY` 是正・maxIterations といったアプリ固有制御を crate 抽象に縛られず書ける。

**依存 crate（薄いラッパ用）：** `reqwest`（`rustls-tls`,`json`,`stream`）／`serde`,`serde_json`／`thiserror 2.0.18`／`base64`／`backon 1.6.0`／`secrecy`（API キー）／（SSE 採用時）`futures-util`,`eventsource-stream 0.2.3`／`tokio`。JSON Schema を Rust 型から生成するなら `schemars` を任意採用（ネイティブツール宣言用）。

### 8.2.2 classic generateContent の型（camelCase 厳密固定）

⚠️ **2026年に Interactions API が GA 化し「正面玄関」に昇格、generateContent は "legacy" だが完全サポート継続**（[`verification/rpt-gemini-design-verify.md`](../verification/rpt-gemini-design-verify.md), [`verification/rpt-gemini-rest-crates.md`](../verification/rpt-gemini-rest-crates.md)）。**将来リスクとして記録**しつつ、現行を1:1移植できる generateContent を採用。Web 資料は両 API 混在（generateContent=camelCase `inlineData`/`functionResponse`、Interactions=snake `function_result`/`call_id`）のため、**struct を camelCase に厳密固定して Interactions 語彙の混入を防ぐ**。

```rust
// エンドポイント: POST https://generativelanguage.googleapis.com/v1beta/{model=models/*}:generateContent
// 認証: x-goog-api-key ヘッダ（?key= は URL がログに残るため避ける）

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentRequest {
    contents: Vec<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<Content>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Tool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_config: Option<ToolConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenerationConfig>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Content { role: Role, parts: Vec<Part> }

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum Role { User, Model, Function } // ★role:"function" も来る（tool 結果返送時, verify 済）

// Part は oneof。内部タグ無しで各フィールドを Option 化するのが最も堅牢
// （Gemini は1 Part に1フィールドのみ入れてくる）。
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct Part {
    #[serde(skip_serializing_if = "Option::is_none")] text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] inline_data: Option<InlineData>,
    #[serde(skip_serializing_if = "Option::is_none")] function_call: Option<FunctionCall>,
    #[serde(skip_serializing_if = "Option::is_none")] function_response: Option<FunctionResponse>,
    #[serde(skip_serializing_if = "Option::is_none")] file_data: Option<FileData>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct InlineData { mime_type: String, data: String } // data = base64

#[derive(Serialize, Deserialize, Clone)]
struct FunctionCall { name: String, #[serde(default)] args: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")] id: Option<String> } // parallel 相関用

#[derive(Serialize, Deserialize, Clone)]
struct FunctionResponse { name: String, response: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")] id: Option<String> }

// tools[].functionDeclarations[]
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Tool { function_declarations: Vec<FunctionDeclaration> }

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct FunctionDeclaration {
    name: String,                       // [a-zA-Z0-9_:.-], 最大128字
    description: String,
    // ★プラグイン由来はフル JSON Schema を parametersJsonSchema に載せる（8.4 / 第9部）
    #[serde(skip_serializing_if = "Option::is_none")] parameters: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")] parameters_json_schema: Option<serde_json::Value>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ToolConfig { function_calling_config: FunctionCallingConfig }

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct FunctionCallingConfig {
    mode: FunctionCallingMode,
    #[serde(skip_serializing_if = "Option::is_none")] allowed_function_names: Option<Vec<String>>,
}

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")] // AUTO / ANY / NONE / VALIDATED
enum FunctionCallingMode { Auto, Any, None, Validated }

// レスポンス
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentResponse {
    #[serde(default)] candidates: Vec<Candidate>,
    #[serde(default)] usage_metadata: Option<UsageMetadata>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Candidate { content: Content, #[serde(default)] finish_reason: Option<String> }
```

> **既定は `AUTO`**（[`verification/rpt-gemini-funccalling-mcp.md`](../verification/rpt-gemini-funccalling-mcp.md)）。`ANY`（+`allowedFunctionNames`）は現行の完了ハルシネーション是正でのみ使う（常用は無限ツール呼び出しループを招く）。`VALIDATED` は Preview 扱いのため当面不使用。

### 8.2.3 エラー型（thiserror）とバックオフ（backon）

現行 `isRateLimitError`/`isServerError`/`RetryInfo` パース（[`src/gemini.ts:372-463`](../../../src/gemini.ts)）を thiserror variant へ 1:1 移植。**握り潰し禁止**（絶対制約1）。

```rust
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GeminiError {
    #[error("rate limited (429){}", retry_after.map(|d| format!(", retry after {}s", d.as_secs())).unwrap_or_default())]
    RateLimited { retry_after: Option<std::time::Duration> }, // RetryInfo.retryDelay を保持
    #[error("server error (status {status})")]
    ServerError { status: u16 },                              // 500/502/503/504
    #[error("http transport error")]
    Transport(#[from] reqwest::Error),
    #[error("response deserialize failed")]
    Deserialize(#[source] serde_json::Error),
    #[error("request timed out")]
    Timeout,
    #[error("api key missing or undecryptable for {scope}")]
    KeyUnavailable { scope: String },                         // ユーザー/Bot キー未設定
    #[error("max function-calling iterations exceeded")]
    MaxIterations,
}
```

- **429/RetryInfo**：`google.rpc.RetryInfo` の `retryDelay`（`"37s"` 形式）を error body の `error.details[]` から抽出し `RateLimited.retry_after` に載せる。
- **バックオフ**：`backon 1.6.0` の `ExponentialBuilder::default().with_jitter()`（ジッタ**明示**）。`RetryInfo` があればそれを尊重、無ければ指数（現行の `min(1000*2^(attempt+1), 60000)` 相当）。`RateLimited`/`ServerError`/`Timeout` のみリトライ対象（`when` 述語で判定）。`maxRetries=3`（現行同値）。
- **サーキットブレーカ**：Gemini エンドポイントに `recloser 1.4.0`（または `AtomicU*` 自前状態機械）を併用（[`00-decisions.md`](../00-decisions.md) #8）。連続失敗で open→即 fail で無駄打ちを防ぎ、劣化縮退（現行のユーザー向け「混み合っています」定型応答）へ落とす。
- **タイムアウト**：`reqwest::Client` に `.timeout(Duration::from_secs(120))`（FC ループ用）／補助生成は 60s（現行 [`src/services/llmClient.ts:137`](../../../src/services/llmClient.ts) と一致）。

### 8.2.4 Function Calling ループ（現行 `runFunctionCallingLoop` の型付き移植）

[`src/gemini.ts:501-746`](../../../src/gemini.ts) を struct 化。往復・並行呼び出し・maxIterations・完了是正を保持。

```rust
struct LoopResult { text: String, browser_tool_called: bool, browser_tool_failed: bool }

async fn run_function_calling_loop(
    client: &GeminiClient,           // API キー・モデルを保持（getUserGenAI/getBotGenAI 相当）
    system_instruction: &str,
    registry: &ToolRegistry,         // 第9部 ToolProvider レジストリ（下記 8.4）
    contents: &mut Vec<Content>,
    ctx: &ToolContext,               // userId / botId / guildId（データ分離キー）
    on_status: &StatusCb,
    opts: LoopOptions,               // record_actions, on_heavy_detected, heavy_runtime_ms, allowed_tool_names
) -> Result<LoopResult, GeminiError> {
    on_status(Status::Thinking);
    let mut resp = client.generate(system_instruction, &registry.declarations(), contents, None).await?;

    let max_iterations = 10;
    let (mut iterations, mut total_calls, mut corrections) = (0usize, 0usize, 0usize);
    let max_corrections = 2;

    while iterations < max_iterations {
        let Some(cand) = resp.candidates.into_iter().next() else { break };
        let calls: Vec<FunctionCall> = cand.content.parts.iter()
            .filter_map(|p| p.function_call.clone()).collect();

        if calls.is_empty() {
            // 完了ハルシネーション是正: このターンで一度も関数を呼ばず「登録しました」等を主張
            if total_calls == 0 && corrections < max_corrections && !registry.is_empty() {
                let text = cand.content.parts.iter().filter_map(|p| p.text.as_deref()).collect::<String>();
                if claims_action_completed(&text) {
                    corrections += 1;
                    contents.push(cand.content.clone());
                    contents.push(Content::user_text(COMPLETION_CORRECTION_PROMPT));
                    on_status(Status::Thinking);
                    let tc = ToolConfig { function_calling_config: FunctionCallingConfig {
                        mode: FunctionCallingMode::Any,                 // ★構造的に関数呼び出しを強制
                        allowed_function_names: opts.allowed_tool_names.clone(), // プラン候補に限定
                    }};
                    resp = client.generate(system_instruction, &registry.declarations(), contents, Some(tc)).await?;
                    iterations += 1;
                    continue;
                }
            }
            break;
        }

        total_calls += calls.len();
        let mut response_parts: Vec<Part> = Vec::with_capacity(calls.len());

        // 並行呼び出し公式サポート。id を相関に保持し、独立ツールは join_all で並行実行してよい。
        for fc in &calls {
            // 実行時エスカレーション（重いツール or 規定時間超で一度だけ一時応答）
            maybe_escalate_heavy(&opts, fc, /* elapsed */);
            // ★第9部レジストリへ dispatch（Native/MCP/WASM を同一 trait で）
            let out = registry.invoke(ctx, &fc.name, fc.args.clone()).await
                .unwrap_or_else(|e| ToolOutput::error(e.to_string())); // 個別失敗は握らず JSON 化して返す
            record_tool_outcome(ctx, &fc.name, &out /* success/error, latency */);
            response_parts.push(Part { function_response: Some(FunctionResponse {
                name: fc.name.clone(),
                response: out.into_json(),
                id: fc.id.clone(),               // ★並行相関
            }), ..Default::default() });
        }

        contents.push(cand.content);                                     // model の functionCall 入り content
        contents.push(Content { role: Role::User, parts: response_parts }); // functionResponse を返送
        on_status(Status::Writing);
        resp = client.generate(system_instruction, &registry.declarations(), contents, None).await?;
        iterations += 1;
    }

    if iterations >= max_iterations { /* browser_tool_failed = true 相当 or GeminiError::MaxIterations */ }
    let text = resp.candidates.first().map(collect_text).unwrap_or_default();
    Ok(LoopResult { text, /* .. */ })
}
```

**完了是正の判定**（`claims_action_completed`、[`src/gemini.ts:480-495`](../../../src/gemini.ts)）：現行の日本語正規表現 2 本（`(登録|追加|…)(し(ました|ておきました|…))` と `(やって|して)おき(ました|…)`）をそのまま Rust `regex` へ移植。`COMPLETION_CORRECTION_PROMPT` も文言直移植。

**3経路の共有（`runPlannedTurn` [`src/gemini.ts:793-926`](../../../src/gemini.ts)）**：ターンプランナー（軽量 LLM でプラン→systemInstruction 注入）、予測非同期（`deferred`）／実行時エスカレーション（`onInterim`）、最終文面の会話ログ保存、シナプス抽出（`onFinal`）を共通クロージャ化。プランの候補ツールを実在ツールへ絞って `allowed_tool_names` に渡す点も保持。

### 8.2.5 マルチモーダル・SSE・モデル

- **マルチモーダル**：`inlineData{ mimeType, data(base64) }` でレシート／音声を渡す（現行 [`src/gemini.ts:1035-1050`](../../../src/gemini.ts) と一致）。**inline は総リクエスト 20MB まで**（公式）。レシート1枚・ボイスメモは inline で十分。**20MB 超のみ Files API**（`files.upload`→`fileData{fileUri,mimeType}`、将来対応）。画像を `reqwest` で取得 → `base64::engine::general_purpose::STANDARD.encode(bytes)`。対応 MIME：`image/png|jpeg|webp|heic|heif`、音声は現行 `SUPPORTED_AUDIO_TYPES`。
- **SSE（任意・パリティ上は不要）**：現行は非ストリーミング（一括＋疑似「入力中…」）。導入時は `streamGenerateContent?alt=sse`（**`alt=sse` 必須**、無いと巨大 JSON 配列）＋ `reqwest::Response::bytes_stream()` → `eventsource-stream 0.2.3` → 各 `data:` 行を `serde_json::from_str::<GenerateContentResponse>`。**行バッファ必須**（チャンク境界で JSON が割れる）。FC 併用時は「functionCall はストリーム完結後に実行」する制御が要る。
- **モデル**：`gemini-3.1-flash-lite`（**GA・変更不要**、[`verification/rpt-gemini-design-verify.md`](../verification/rpt-gemini-design-verify.md)）。`-preview` 名は使わない（`gemini-3.1-flash-lite-preview` は 2026-07-09 廃止）。モデルはユーザー設定（`conf.model`）／Bot 既定（`BOT_DEFAULT_MODEL` [`src/services/llmClient.ts:19`](../../../src/services/llmClient.ts)）から解決。API キーは `secrecy::Secret<String>` で保持し、キャッシュ（現行 `userAICache`/`botAICache`）はキー変更で無効化。

### 8.2.6 補助生成（Function Call なし）

現行 `generateAuxText` / `generateAuxMultimodal`（[`src/services/llmClient.ts:122-198`](../../../src/services/llmClient.ts)、タグ自動付与・要約・文字起こし等）は、同じ GeminiClient の `tools` 無し呼び出し＋短いリトライ（`maxRetries=2`）で移植。失敗時は現行同様 `None` を返しフォールバック（呼び出し側で縮退）。

---

## 8.3 第5部（supervisor）との接続点

- 各テナント Shard task は **supervisor の JoinSet 配下**。panic は `JoinError.is_panic()` で検知され指数バックオフ再 spawn（絶対制約2）。`panic = "unwind"` 厳守（[`00-decisions.md`](../00-decisions.md)）。
- **停止協調**：`tokio-graceful-shutdown 0.19.3` の `ShutdownToken` を各 task の `select!` に配線（現行 `stopBot`/`stopCustomBot` [`src/bot.ts:1411-1508`](../../../src/bot.ts) の置換）。SIGTERM 伝播でギルド/DM 応答を安全に打ち切る。
- **劣化縮退**：Gemini サーキットブレーカ open → ユーザーへ定型応答（現行 `guildErrorResult` [`src/gemini.ts:1358-1382`](../../../src/gemini.ts)）。synapse/Redis ダウン時の縮退は第5部・DB 部の方針に従う。
- **致命 fail-fast の限定**：無効トークン・設定不備は起動時に検出（`ConfigError`）。実行時の一時障害（429/5xx/接続断）は自己復帰対象で落とさない。

## 8.4 第9部（ToolProvider）との接続点

- Gemini の function calling レジストリ（現行 `buildFunctionRegistry` [`src/functions/registry.ts`](../../../src/functions/registry.ts)）は **第9部の `ToolProvider` トレイト＋中央レジストリ**へ置換。リクエスト毎に全 provider から `list()` → `FunctionDeclaration[]` を**動的生成**（現行の毎ターン再構築＝正しいパターン）。
- **スキーマは `parametersJsonSchema`（フル JSON Schema）を使用**（[`verification/rpt-gemini-funccalling-mcp.md`](../verification/rpt-gemini-funccalling-mcp.md)）：`$ref`/`$defs`/`additionalProperties`/`prefixItems` を受け付けバックエンドへそのまま転送。ただし **sanitizer で `$schema` 除去・`default` 除去/変換・`oneOf`/`allOf`→`anyOf`・深すぎるネスト平坦化**を通す（現行 `jsonSchemaToGeminiSchema` [`src/functions/mcpDynamic.ts`](../../../src/functions/mcpDynamic.ts) の最小変換を強化）。レガシー `parameters`（OpenAPI サブセット）は使わない方針。
- **ツール名**：`[a-zA-Z0-9_:.-]`・最大 **128字**（generateContent 制約。現行の 63字/`mcpDynamic` はやや保守的）。ソース別 **namespace 接頭辞**（例 `mcp__github__create_issue`）で衝突回避（現行 `mcpFunctionName`＋`disambiguateFunctionName` を踏襲）。
- **dispatch**：`functionCall.{name, args, id}` を registry でルックアップ → `ToolProvider::invoke(ctx, name, args)` → `functionResponse.{name, response, id}`。**`id` を並行相関に保持**。`ToolContext` に `UserId`（＋ `guildId`）を載せ、能力スコープ・データ分離を型で強制（現行 `ToolContext` [`src/types/contracts.ts:17-33`](../../../src/types/contracts.ts) を newtype 化）。個別ツール失敗は握り潰さず `{success:false, message}` JSON にして Gemini へ返す（現行 `dispatch` の catch と同挙動、ただし `PluginError` を経由）。
- **MCP**：現行 `mcpDynamic`/`mcpClient` は第9部の `McpProvider`（rmcp 2.0.0 client）に吸収。ネイティブは `NativeProvider`、非信頼ユーザー製は `WasmProvider`（Extism）。3系統を同一 `ToolProvider` として Gemini ループから透過的に扱う。
