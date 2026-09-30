# Yuuka v2 アーキテクチャ（旧 Node/TypeScript 実装・DB schema v16 時点の履歴資料）

> **⚠️ 履歴資料です。現行の実装規範ではありません。**
>
> 本書は、バックエンドを Rust へ全面移行する**前**の Node.js / TypeScript 実装（仕様書 docs/spec/discordbot_spec.md v0.6.2 準拠・DB schema v16）に対する設計・実装規範として書かれたものです。
> Node バックエンドは [#68](https://github.com/kawaii-music-moe/yuuka/pull/68) で撤去済みのため、本書が前提とする `src/*.ts`（`src/services/browserService.ts`・`src/db/migrations.ts`・`src/types/contracts.ts`・`src/utils/crypto.ts` 等）、TypeScript / ESM の規約、`SCHEMA_VERSION="16"` は**現行リポジトリには存在しません**。以下の本文は旧実装のまま**未更新**であり、「必ず従う規範」「他の文書より優先する文書」としては扱わないでください（旧版にあった「本書を優先する」旨の記述は撤回しました）。
> 設計意図（why）・不変条件の考え方・旧実装との対応を辿るための参照用として残しています。撤去前のソースは `git show 390df39^:<path>`（例: `git show 390df39^:src/db/migrations.ts`）で参照できます。Rust 版に合わせた全面的な書き直しは未実施です。
>
> **現行の情報源（source of truth）**
>
> | 知りたいこと | 現行の参照先 |
> |---|---|
> | 実装（バックエンド） | `crates/`（Rust workspace。クレート一覧は [Cargo.toml](../../Cargo.toml) の `[workspace] members`、全体像は [project_overview.md](../project_overview.md)） |
> | DB スキーマ | `crates/yuuka-db/migrations/`（`V17__baseline.sql`＝Node 版最終スキーマ〔本書の v16 より新しい v17〕を冪等に作成し、`V18`〜`V22` を前方専用で追加。`refinery` で適用） |
> | フロントとの型の共有 | `crates/yuuka-types` の DTO を `cargo run -p xtask -- gen-types` で `frontend/src/lib/api/generated/` へ生成 |
> | 管理画面 / PWA | `frontend/`（Svelte 5 + Vite）/ `client/pwa/`（Vue 3 + Vite。[README](../../client/pwa/README.md)） |
> | セットアップ・運用 | [docs/guide/setup.md](../guide/setup.md) / [features.md](../guide/features.md) / [deployment.md](../guide/deployment.md) |
> | 機能仕様 | [docs/spec/](../spec/discordbot_spec.md) |
> | Rust 移行の経緯・現況 | [docs/rust-rewrite/](../rust-rewrite/README.md)（[remaining-work.md](../rust-rewrite/remaining-work.md)） |

---

## 0. 最重要原則

1. **データ分離**: 全ユーザーデータは `user_id`（DiscordユーザーID, TEXT）を WHERE 句の必須条件とする。`user_id` なしのワイルドカードクエリ禁止（cron系の全件走査は例外として明示コメントを書く）。
   **例外パターン（Bot属性 docs/spec/bot_attributes_requirements.md §6）**: 汎用モード（MCPアシスタント）のデータは `bot_id × user_id`（bot_context_notes）/ `bot_id × guild_id`（bot_guild_notes, bot_guilds, message_logs のギルド会話）の複合スコープを正式な分離キーとする。この user_id にはWebアカウント未登録のDiscordユーザーIDも入るため、message_logs / bot_context_notes / bot_members は users へのFKを持たない。
2. **ブラウザ操作層は不変**: `src/services/browserService.ts`・`src/rust_crawler/`・`src/functions/browserFunctions.ts` の既存アプローチ（Rustクローラーデーモン→Puppeteerフォールバック、ユーザー別永続セッション、`data-yuuka-id` 数値IDアノテーション）は変更しない。新機能はこの上に乗せる。
3. **認証情報はLLMに渡さない**: パスワードマネージャの復号値は `browserService` へ直接渡す。Function Call の戻り値・ログ・プロンプトに含めてはならない。
4. **スキーマは前方移行する**: 現在の `SCHEMA_VERSION` は **"16"**。初版（v2 全面再構築）以降は破壊的再構築は行わず、`migrations.ts` の各 `migrate*` 関数が冪等な ALTER / 追加テーブルで段階移行する（v3〜v16 の概要は §2 末尾参照）。
5. **コメント・ログは日本語**、既存コードのスタイル（セクション区切りコメント `// ─── ... ───`、絵文字ログ）を踏襲する。
6. **TypeScript / ESM**: import は `.js` 拡張子付き相対パス。型は明示的に。新規依存の追加は原則禁止（導入済み: bcryptjs, @node-rs/argon2, rss-parser, @napi-rs/canvas, chart.js, cron-parser, archiver, **ws**。lint/format は `@biomejs/biome`、型チェックは `tsgo`）。**シナプスエンジン（§13）は新規 npm 依存を追加せず**、ベクトル索引・埋め込みを独立した Rust crate（`src/rust_synapse/`、既存 crawler と同型）に閉じ込めることでこの原則を守る。**デスクトップクライアント（`docs/design/desktop_client/`）の `/ws/chat` WebSocket 受け口に限り、定番・transitive 依存ゼロの `ws` を意図的な例外として追加（2026-06-23 オーナー承認。代替のミニマル手実装よりフレーム/バックプレッシャ保守コストが小さい）。

---

## 1. 共有コントラクト（src/types/contracts.ts）

全モジュールは `src/types/contracts.ts` の型に依存する。**このファイルを変更してはならない**（変更が必要なら統合フェーズで行う）。

```ts
// ToolContext: Function Call ハンドラに渡される実行コンテキスト
export interface ToolContext {
  botId: string;     // 実行中のBotインスタンスID（通知送信のクライアント解決に使用）
  userId: string;    // DiscordユーザーID（データ分離キー。全リポジトリ呼び出しに必須）
  guildId?: string;  // 発話ギルドID（汎用モード等のギルド常駐Botのみ。DM/秘書では undefined）
  embeds: EmbedBuilder[];                 // リッチ返信キュー（push すると返信に添付）
  files: { attachment: Buffer; name: string }[]; // ファイル添付キュー（グラフPNG等）
  richReplyEnabled: boolean;              // ユーザー設定（falseならembeds/files禁止）
}

// FunctionModule: 各機能モジュールが export する束
export interface FunctionModule {
  declarations: FunctionDeclaration[];
  handlers: Record<string, (ctx: ToolContext, args: Record<string, unknown>) => Promise<string> | string>;
}

// RouteModule: HTTPルートモジュール
export type RouteAuth = "none" | "user" | "admin";
export interface RouteRequestCtx {
  req: IncomingMessage; res: ServerResponse;
  url: URL;
  user: SessionUser | null;     // auth:"none" の場合 null の可能性あり
  body: Record<string, unknown>; // JSONボディ（パース済、無ければ {}）
  rawBody: Buffer;               // HMAC検証用の生ボディ
  params: Record<string, string>; // パスパターン :name の解決値
}
export interface RouteDef {
  method: "GET" | "POST" | "DELETE" | "OPTIONS";
  path: string;                  // 例 "/api/contacts", "/hook/:token"
  auth: RouteAuth;
  handler: (ctx: RouteRequestCtx) => Promise<void>;
}
export interface SessionUser { discordId: string; username: string; role: "user" | "admin"; }
```

ハンドラ内のレスポンスは `sendJson(res, statusCode, obj)`（contracts.ts からexport）を使う。

### Function ハンドラの戻り値
- JSON文字列を返す（既存スタイル）: `JSON.stringify({ success: true, ... })` / `{ success: false, message })`。
- ユーザーへの確認が必要な操作（仕様の承認フロー）は、確認待ちであることを示すJSONを返し、**LLMがユーザーへ確認文を提示**する。承認後にLLMが確定用Functionを再度呼ぶ2段階方式とする（例: `organizeTaskPriorities` → 提案JSON → ユーザー承認 → `applyTaskPriorities`）。

---

## 2. DBスキーマ v16（src/db/migrations.ts が唯一の定義元）

スキーマは migrations.ts に集約済み（現 `SCHEMA_VERSION="16"`）。各リポジトリは**テーブルを再定義しない**。主要テーブル（全列は migrations.ts 参照）:

| テーブル | キー/スコープ | 用途 |
|---|---|---|
| users | discord_id PK | 認証(bcrypt cost12) + salt(hex) + ユーザー設定(rich_reply_enabled, remind_default_minutes, notify_target_*, active_persona_id〔レガシー: ペルソナ適用は v8 で bot_active_personas へ移行〕, timezone) + Gemini鍵(enc) + バックアップ設定 |
| bots | id PK, owner user_id | Botインスタンス（Discordトークンenc, recommended_persona_id, suspended〔管理者処分〕, **stopped〔v9: オーナー手動停止。再起動後も維持〕**, **capabilities〔JSON: ケーパビリティ集合〕**, **persona_id〔v8: Bot単位ペルソナ〕**, **gemini_api_key_*〔Bot専用キーenc。汎用モードは設定必須〕**, discord_application_id〔招待リンク導出〕, **enabled_modules〔機能モジュール化: 有効モジュールIDのJSON配列。NULL=全有効。Bot単位の既定値（フォールバック）〕**） |
| bot_user_modules | (bot_id, user_id) PK | **機能モジュール化のユーザー×Bot上書き層**。行があれば enabled_modules(JSON配列) を採用、無ければ bots.enabled_modules へフォールバック。`[]`=全OFF も尊重。users へFKなし（未登録Discord IDも可）。詳細 [function_modularization.md](../design/function_modularization.md) |
| bot_shares | bot_id, shared_user_id | 共有招待 status: pending/active/revoked |
| personas | id PK, owner_id | name, prompt(≤20000), is_public |
| message_logs (+ message_logs_fts FTS5) | user_id (+ bot_id, guild_id) | 全会話永続化, discord_msg_id, reply_to_msg_id, role: user/assistant。guild_id=NULL はDM/秘書利用、非NULLは汎用モードのギルド会話 |
| todos | user_id | title, description, due_date, priority(high/medium/low/null), tags(JSON), status(open/done), linked_payment_id。**v12: start_date / progress(0-100) / parent_id〔自己参照・1階層サブタスク〕**。**v16: repeat_rule(cron) / repeat_until / repeat_count〔ルーチン（繰り返し）タスク〕** |
| task_progress_logs | user_id, todo_id | **v12: タスク進捗の履歴（ガント・進捗推移）** |
| schedules | user_id | Googleカレンダー同期予定（既存構造を user スコープ化） |
| reminders | user_id | message, trigger_at, repeat_rule(cron), target_type(dm/channel)+target_id, status(pending/sent/cancelled), source(manual/todo/schedule/payment/birthday/webhook), source_id |
| expenses | user_id | type(income/expense), amount, category, memo, date, source(manual/receipt_ocr) |
| budget_limits | user_id, category | 月次予算上限 |
| planned_payments | user_id | title, amount, category, due_date, repeat_rule, status(pending/settled/cancelled), settled_expense_id, linked_todo_id, linked_reminder_id |
| playbooks | user_id, name UNIQUE | マクロ=Playbook（title, keywords JSON, description, steps テキスト）。仕様§3.6のMacroはこのテーブルで実現する |
| playbook_schedules / playbook_runs | user_id | cron定期実行（既存機能を user スコープ化、bot_id は通知用に保持） |
| context_notes | user_id PK | content(≤10000) 単一ドキュメント |
| clipboard_entries | user_id | content, expires_at(NULL=無期限) |
| contacts | user_id | name, birthday, relationship, contact_info, notes, tags(JSON) |
| credentials | user_id, service_name | PWマネージャ。username, url, encrypted_password/iv/auth_tag（**ユーザー鍵で暗号化**） |
| webhook_endpoints | user_id, token UNIQUE | name, secret(enc), notify_target, template, filter_keyword, create_todo, create_reminder, enabled |
| webhook_deliveries | endpoint_id | 受信監査ログ payload, status |
| briefing_configs | user_id PK | 朝報: enabled, schedule_cron, target, weather_lat/lng, location_name, news_feeds(JSON), news_keywords(JSON) |
| report_configs | user_id, type(daily/weekly) | enabled, schedule_cron, target |
| mcp_servers | id PK, owner_id, (user_id, bot_id) スコープ | name, endpoint_url, auth_credential(enc), tools_cache(JSON), enabled, requires_confirmation。v4 で (user_id, bot_id) へ、v7 で owner_id 次元を追加（クロステナント露出修正） |
| audit_logs | user_id | action, target, detail（PW本体は記録禁止） |
| invite_codes / system_settings | 既存どおり |

**シナプス認知アーキテクチャ（v10。設計: docs/design/synapse_cognitive_architecture.md・architecture_renewal_v3.md。詳細は §13）**。すべて `user_id`（汎用モードは `bot_id × guild_id` も）でスコープし、`message_logs` と同一 .db。**書き手は Node のみ**（Rust エンジンは read-only 参照）:

| テーブル | キー/スコープ | 用途 |
|---|---|---|
| synapses (+ embedding BLOB) | user_id (+ bot_id, guild_id) | L2 連想記憶＝記憶の断片。content, topic_id, source_msg_id, embedding(float32 LE BLOB), embedding_model_version, last_used_at, use_count, decay_score。秘匿値は格納禁止（§0.3 継承） |
| tool_outcomes | user_id (+ bot_id, guild_id) | ツール実行実績（経験）。tool_name, args_digest（秘匿マスク済）, status(success/error), latency_ms, topic_id。actionRecorder の揮発(Redis 2h)を SQLite へ永続化 |
| topic_tool_stats | (user_id, bot_id, topic_id, tool_name) PK | トピック別ツール勝率（中間ビュー）。success/total/success_rate を tool_outcomes からインクリメンタル更新（2nd Hop 用） |

**汎用モード（MCPアシスタント）／Bot属性のテーブル**（bot_attributes_requirements.md §5。複合スコープが正式な分離キー）:

| テーブル | キー/スコープ | 用途 |
|---|---|---|
| bot_active_personas | (user_id, bot_id) PK | v8: 秘書ペルソナの適用状態（旧 users.active_persona_id を Bot 単位へ分離） |
| bot_context_notes | (bot_id, user_id) | 汎用モードの個人ノート（users へFKなし＝未登録Discord IDも可） |
| bot_guild_notes | (bot_id, guild_id) | 汎用モードのギルド共有ノート |
| bot_guilds | (bot_id, guild_id) | 応答許可ギルドリスト |
| bot_members | (bot_id, guild_id, user_id) | 汎用モードの利用メンバー（users へFKなし） |
| bot_mcp_access | (bot_id, owner_id, mcp_server_id) | Bot が利用可能な MCP サーバーの許可リスト |
| bot_credential_access | (bot_id, owner_id, service_name) | Bot が利用可能な認証情報の許可リスト |
| user_google_accounts | owner(user_id) | v5: Google 複数アカウント連携（OAuth enc, primary フラグ） |
| bot_google_account | (bot_id) | v5: Bot ごとに割り当てる Google アカウント（NULL 許容 v7+） |
| bot_member_requests | (bot_id, guild_id, user_id) | **v14: ギルド利用メンバーの利用申請（承認制）。db/botMemberRequestRepo.ts** |
| bot_roles | (bot_id, guild_id) | **v15: 汎用モードで応答を許可する Discord ロール（利用可能ロール）** |

**デスクトップクライアント（汎用チャット API）のテーブル**（[design/desktop_client/](../design/desktop_client/index.md)。Phase 0/1 バックエンド実装済み）:

| テーブル | キー/スコープ | 用途 |
|---|---|---|
| desktop_tokens | user_id | **v13: OAuth デバイスフロー用の長命トークン（device_code / access_token のハッシュ、デバイス名、失効）。db/desktopTokenRepo.ts** |

**マイグレーション履歴**（各 `migrate*` 関数は冪等。新規DBは最終形を直接 CREATE）:
v2=初版全面再構築 / v3=ユーザーデータ各表へ bot_id 付与（`migrateToBotScopedData`）/ v4=MCP を (user_id, bot_id) 化（`migrateMcpToBotScope`）/ v5=owner リソース許可・Google複数アカウント（`migrateOwnerResourceGrants`）/ v6–v7=MCP/リソース許可へ owner_id 次元追加・bot_google_account を NULL 許容化（`migrateMcpAccessOwnerScope`・`migrateBotGoogleAccountNullable`）/ v8=ペルソナを Bot 単位化（`migratePersonaToBotScope`）/ v9=bots.stopped 追加 / **v10=シナプス記憶層（synapses・tool_outcomes・topic_tool_stats）を新規テーブルとして冪等追加（`migrateToSynapseEngine`）。既存テーブルへの変更なし＝完全に後方互換** / v11=synapses へ時刻文脈列（再ランキング専用） / v12=タスク進捗管理（todos へ start_date/progress/parent_id 追加 + task_progress_logs、`migrateToTaskProgress`） / v13=デスクトップクライアント用トークン表 desktop_tokens（`migrateToDesktopTokens`） / v14=ギルド利用申請 bot_member_requests（`migrateToMemberRequests`） / v15=利用可能ロール bot_roles（`migrateToAllowedRoles`） / **v16=ルーチン（繰り返し）タスク（todos へ repeat_rule/repeat_until/repeat_count、`migrateToRoutineTasks`）**。機能モジュール化（bots.enabled_modules 列 + bot_user_modules テーブル）はベーススキーマへ冪等統合済み。いずれも新規テーブル/列追加のみ＝後方互換。

日時は既存スタイル（`datetime('now','localtime')` の `YYYY-MM-DD HH:MM:SS` テキスト）に合わせる。

---

## 3. 暗号（src/utils/crypto.ts v2 — 変更禁止、利用のみ）

- `encryptText / decryptText`: システム鍵（scrypt(YUUKA_ENCRYPTION_SECRET)）。**用途: APIキー・Discordトークン・OAuthトークン・Webhookシークレット・MCP認証情報**。
- `encryptForUser(userId, text)` / `decryptForUser(userId, enc, iv, tag)`: **Argon2id(YUUKA_ENCRYPTION_SECRET, user.salt)** から導出した32byte鍵 + AES-256-GCM。**用途: パスワードマネージャ（credentials テーブル）のみ**。鍵はメモリ内キャッシュ。
- YUUKA_ENCRYPTION_SECRET は `config.secretKey`（環境変数/設定 `YUUKA_ENCRYPTION_SECRET`、後方互換で `SECRET_KEY`）。
- `YUUKA_ENCRYPTION_SECRET_NEW` が設定されている場合、起動時に `rotateSecretKey()` が全暗号化データを再暗号化する（crypto.ts 実装済）。

## 4. 認証・セッション

- パスワード: bcryptjs cost 12。ポリシー: 8文字以上 + 大文字/小文字/数字/記号のうち2種以上 + `src/assets/common-passwords-10k.txt`（読み込みは `passwordPolicy.ts`）との一致拒否。
- セッション: Redis `session:{sha256(token)}` に SessionUser JSON、TTL 7日、アクセス毎にTTL延長（スライディング）。Redis不通時は in-memory フォールバック。ユーザー毎の発行済セット `user_sessions:{userId}` を保持し、パスワード変更時に全失効。
- 実装: `src/services/sessionService.ts`（createSession/getSession/touchSession/destroySession/destroyAllForUser）。

## 5. LLM層

- `src/services/llmClient.ts`（基盤提供）:
  - `getUserGenAI(userId): { genAI, model } | null` — **ユーザー自身のGemini APIキーのみ**使用（仕様§4.2。秘書系の対話・補助生成はこちら）。
  - `getBotGenAI(botId): { genAI, model } | null` — **Bot専用キー**（bots テーブルへ暗号化保存）。汎用モード（MCPアシスタント）の対話のみが使用し、発話ユーザーの個人キーは使わない（bot_attributes_requirements.md §4.3.3）。
  - `generateAuxText(userId, prompt, systemInstruction?): Promise<string>` — タグ自動付与・Webhook解釈・レポート要約などの補助生成用（Function Callなし、リトライ付き）。
- `src/gemini.ts`: コンテキスト復元、返信チェーン解決、ペルソナ+コンテキストノート注入、MCP動的ツール、リッチ返信ゲート。Redis 会話キャッシュキーは利用形態で分かれる（`src/db/messageLogRepo.ts`）:
  - `context:{botId}:secretary:{userId}` — 秘書モード（個人 DM）。
  - `context:{botId}:{guildId}` — 汎用モードのギルド会話。
  - `context:{botId}:dm:{ownerId}` — 汎用モードの owner DM。
  - リセット境界は `context_floor:{botId}:{userId}` / `context_floor:{botId}:dm:{ownerId}`（会話履歴クリアの基準時刻）。
- 会話履歴の正は `message_logs`（SQLite, (user_id, bot_id, guild_id) スコープ）。Redisキャッシュ消失時はSQLiteから再構築。

## 6. リッチ返信（§3.0）

- Embed: 既存 `showRichContent` を継続。カラー規約は仕様§3.0.2 の HEX を `src/utils/embeds.ts` の COLOR_MAP に追加。
- グラフ: `src/services/chartService.ts` が chart.js + @napi-rs/canvas でPNG生成（ダークテーマ: 背景 #2B2D31, 文字 #FFFFFF, フォントは内蔵）。Function `sendChart`（chartFunctions.ts）が `ctx.files` にPNGを push し、Embed の image に `attachment://chart.png` を設定。
- `ctx.richReplyEnabled === false` のとき、リッチ系ハンドラは何もせず `{success:false, message:"ユーザー設定によりリッチ返信は無効です。テキストで返答してください。"}` を返す。

## 7. マクロ（§3.6）= Playbook 統合方針

仕様の「マクロ」は既存 Playbook 機構で実現する（名称はユーザー向けに「マクロ（Playbook）」と案内）。
- 説明ベース登録: 既存 `savePlaybook`。
- 実行ベース登録: `src/services/actionRecorder.ts` が Redis `fc_history:{userId}`（直近30件、TTL 2h）に Function Call 名+引数要約を記録（gemini.ts が dispatch 毎に記録）。**認証情報系・記録系自身は記録から除外**。新Function `saveMacroFromRecentActions` が履歴を取得しLLM向けに返し、LLMが steps へ要約して `savePlaybook` を呼ぶ。
- 呼び出し: `findPlaybooks` でマッチ→LLMがユーザーへ実行確認→承認後 `runPlaybook(name)`（playbookFunctions.ts。steps を返しLLMがその手順に従って実行する既存方式）。
- 定期実行: 既存 playbook_schedules を継続（user_id スコープ化）。

## 8. パスワードマネージャ（§6）とブラウザの調和

`src/functions/credentialFunctions.ts` を以下のFunction群に置き換える:
| Function | 動作 |
|---|---|
| `listCredentialServices` | サービス名+ユーザー名一覧（パスワードなし） |
| `addCredential` | 登録（LLMはユーザーから値を受領した直後のみ呼ぶ。確認フロー必須の旨をdescriptionに明記） |
| `updateCredential` / `deleteCredential` | 同上 |
| `browserFillCredential` | **{service_name, username_selector?, password_selector?}** を受け、復号した値を `browserService.browserInteractiveType` で直接入力。セレクタ省略時はページ内の `input[type=password]` と直前のtext/email inputを自動検出。戻り値は success/message のみ。 |
旧 `getCredential`（平文返却）は**廃止**。全アクセスは `auditRepo.addAuditLog()` に記録（パスワード本体は記録しない）。

## 9. cron系サービス共通

- node-cron でジョブ登録、`cron-parser` で次回実行時刻計算。
- 通知送信は `src/services/notifier.ts`（基盤提供）: `sendToUser(userId, payload: {content?, embeds?, files?}, target?: {type:"dm"|"channel", id})` — ユーザー設定の既定送信先 or 指定先へ、`getBotClientForUser` で解決したクライアントから送信。
- 停止からの復帰: reminders は `trigger_at <= now AND status='pending'` を起動時に処理（仕様§10）。

## 10. ファイル所有マップ（並列実装時の競合防止）

各モジュールは「所有ファイル」以外を**編集禁止**（読むのは自由）。共有ファイル（gemini.ts, bot.ts, index.ts, server.ts, functions/index.ts, public/*）は統合フェーズでのみ編集する。

| モジュール | 所有ファイル |
|---|---|
| 基盤(完了) | types/contracts.ts, db/migrations.ts, db/database.ts, utils/crypto.ts, config.ts, db/auditRepo.ts, services/llmClient.ts, services/notifier.ts, server/routeRegistry.ts, functions/registry.ts |
| auth | services/sessionService.ts, services/passwordPolicy.ts, services/pendingRegistration.ts, db/userRepo.ts, server/routes/authRoutes.ts, server/routes/settingsRoutes.ts |
| messagelog | db/messageLogRepo.ts, functions/conversationFunctions.ts |
| todo | db/todoRepo.ts, functions/todoFunctions.ts, services/autoTagService.ts, server/routes/todoRoutes.ts |
| reminder | db/reminderRepo.ts, services/reminderEngine.ts, functions/reminderFunctions.ts, server/routes/reminderRoutes.ts |
| finance | db/expenseRepo.ts, db/plannedPaymentRepo.ts, functions/financeFunctions.ts, services/paymentRecurrenceService.ts, services/receiptParser.ts, server/routes/financeRoutes.ts |
| pwmanager | db/credentialRepo.ts, db/credentialAccessRepo.ts, services/secretService.ts, functions/credentialFunctions.ts, server/routes/credentialRoutes.ts |
| personal | db/contextNoteRepo.ts, db/clipboardRepo.ts, db/contactRepo.ts, functions/noteFunctions.ts, functions/clipboardFunctions.ts, functions/contactFunctions.ts, services/clipboardCleanupService.ts, services/birthdayReminderService.ts, server/routes/personalRoutes.ts |
| macro | services/playbookService.ts, services/actionRecorder.ts, functions/playbookFunctions.ts, services/playbookScheduleService.ts, server/routes/playbookRoutes.ts |
| reports | db/reportConfigRepo.ts, db/briefingConfigRepo.ts, services/reportService.ts, services/briefingService.ts, functions/briefingFunctions.ts, server/routes/deliveryRoutes.ts |
| webhook | db/webhookRepo.ts, services/webhookProcessor.ts, server/routes/webhookRoutes.ts |
| mcp | db/mcpRepo.ts, services/mcpClient.ts, functions/mcpDynamic.ts, server/routes/mcpRoutes.ts |
| persona | db/personaRepo.ts, server/routes/personaRoutes.ts |
| botattributes（Bot属性 docs/spec/bot_attributes_requirements.md） | services/botCapabilities.ts, services/botRateLimit.ts, services/memberRequest.ts, db/botAttributesRepo.ts, db/botNoteRepo.ts, db/botMemberRequestRepo.ts, functions/botAssistantFunctions.ts, server/routes/botAttributeRoutes.ts, server/routes/memberRequestRoutes.ts |
| 機能モジュール化（§14。docs/design/function_modularization.md） | functions/moduleCatalog.ts, functions/browserModule.ts, functions/richContentModule.ts, services/botModules.ts, db/botUserModulesRepo.ts（API/UI は botAttributeRoutes.ts・public/* と統合） |
| desktopclient（§15。汎用チャット API。docs/design/desktop_client/） | server/chatWebSocket.ts, services/chatChannelService.ts, services/componentInteractionService.ts, services/desktopAuthService.ts, db/desktopTokenRepo.ts, server/routes/deviceAuthRoutes.ts, server/routes/deviceMgmtRoutes.ts, server/routes/desktopClientRoutes.ts |
| charts | services/chartService.ts, functions/chartFunctions.ts |
| calendar | services/googleCalendarService.ts, services/googleDriveService.ts, services/backupService.ts, db/scheduleRepo.ts, db/googleAccountRepo.ts, functions/scheduleFunctions.ts, server/routes/scheduleRoutes.ts |
| synapse（§13。シナプス認知アーキ v3 / schema v10） | src/rust_synapse/*（独立 Rust crate）, services/synapseEngine.ts, services/synapseExtractor.ts, services/metrics.ts, db/synapseRepo.ts, db/toolOutcomeRepo.ts |
| 統合 | gemini.ts, bot.ts, index.ts, server.ts, functions/index.ts, db/botRepo.ts, db/inviteRepo.ts, db/systemSettingsRepo.ts, db/database.ts, db/redis.ts, utils/embeds.ts, server/routes/botRoutes.ts（Bot管理）, server/routes/integratedRoutes.ts（統合管理: Bot起動/停止/再起動・リソース許可・Google複数アカウント）, server/routes/adminRoutes.ts, public/* |

## 11. Function 命名規約（最終レジストリ）

既存名は維持しユーザー体験の互換を保つ。新規は lowerCamelCase。最終的に functions/index.ts（統合フェーズ）が各モジュールの `FunctionModule` をマージする。名前衝突禁止。各モジュールの declarations の description は日本語で、LLMが適切に使い分けられるよう具体的に書く（既存ファイルの書きぶりを踏襲）。

## 12. HTTPルート規約

- 新機能のルートは `src/server/routes/*.ts` に `RouteDef[]` を export し、`server/routeRegistry.ts` の `registerRoutes()` で登録する。server.ts はリクエスト毎にまずレジストリを照合する（統合フェーズで接続）。
- 認可: `auth:"user"` はセッション必須。リソースは必ず `ctx.user.discordId` でスコープする。`auth:"admin"` は role 確認。
- Webhook受信 `POST /hook/:token` のみ `auth:"none"`。

## 13. シナプス認知アーキテクチャ（schema v10 / R0・R1 実装済み）

設計思想は [`docs/design/synapse_cognitive_architecture.md`](../design/synapse_cognitive_architecture.md)（why/what）、システム実装面は [`docs/design/architecture_renewal_v3.md`](../design/architecture_renewal_v3.md)（how/topology）。本節は**実装済み部分（R0/R1）**を実装規範へ昇格したもの。R2（2nd Hop 勝率提示）・R3（ローカル SLM ハイブリッド）・R4（投機/キャッシュ）は未実装の将来フェーズ。

### 13.1 構成（プロセス分離）
- **Node オーケストレータ（既存）**: Discord I/O・ツール dispatch・Gemini 本推論＋ペルソナ・**SQLite の唯一の書き手**。
- **Rust シナプスエンジン `yuuka-synapse`（新規・常駐）**: 埋め込み生成・RAM ベクトル索引（per-scope ブルートフォース cosine KNN）・1st Hop 連想。記憶の重い処理を **V8 ヒープの外**へ。crawler と同じ「子プロセス＋改行区切り JSON（stdio）」流儀（`services/synapseEngine.ts` が `CrawlerDaemon` を踏襲）。SQLite は **read-only（WAL）**で起動時・reindex 時のみ参照。

### 13.2 IPC プロトコル（凍結）
改行区切り JSON。要求 `{"id":<u64>,"command":<string>,...fields}` / 応答 `{"id":<u64>,"ok":<bool>,"result":<obj|null>,"error":<string|null>}`。`id` は純粋な相関 ID。**シナプス ID は封筒 id と衝突させないため別キー `sid`** で運ぶ（index/forget）。コマンド: `health` / `assemble`（1st Hop KNN）/ `index`（埋め込み生成＋RAM 追加、永続化用 embedding を base64 で返す）/ `forget` / `reindex`。embedding BLOB は **float32 little-endian** 連結（Node が SQLite へ永続化、Rust が起動時に復元）。

### 13.3 不変条件の保全（§0 を跨プロセスで維持）
1. **データ分離**: Rust の全 API は scope `{user_id, bot_id, guild_id}` を伴い、KNN はスコープ完全一致のバケット内のみを探索（クロステナント遮断）。Node 側 repo も全クエリ user_id+bot_id スコープ。
2. **認証情報非露出**: シナプス content・tool_outcomes.args_digest に秘匿値を入れない（§0.3 継承）。**多層防御**で担保する: (a) キー名ベース（`sanitizeArgsForLog`＝秘匿キー名/認証系 Function をマスク、`synapseExtractor` のキーワード正規表現）。(b) **値の形状ベース**（`utils/secretGuard.ts`＝JWT・APIキー・PEM・高エントロピートークンを検出。`browserInteractiveType` 等の自由入力ツールはタイプ値を構造的に伏せ、シナプス抽出はトークン形状でも該当発話を除外）。キー名マスクだけでは「非秘匿名キーにタイプされた認証情報」を取りこぼすため (b) を必須とする。
3. **スキーマ前方移行**: v10 は新規テーブルのみの冪等追加（破壊的変更なし）。
4. **障害分離＝デグレード**: エンジン無効・未起動・crash・タイムアウト時、`assembleRecall`/`indexSynapse` 等は **null を返し現行挙動（直近15件の生履歴注入）へフォールバック**。RAM 索引は SQLite の embedding BLOB から再構築可能なキャッシュであり、喪失は性能劣化であって data loss ではない。Rust デーモンは不正入力・DB 不在でも panic せず応答を継続する。

### 13.4 構成（`config.ts`。R0/R1 コンポーネントは**常時有効**）
R0/R1 のコンポーネント（ツール実績記録・Rust エンジン起動＋L2 想起注入＋シナプス索引・会話ターンからのシナプス抽出・§10 メトリクス）は**常時有効**。エンジンはバイナリ（`dist/bin/yuuka-synapse` 等）があれば起動し、不在・未起動・タイムアウト時のみ現行挙動（直近15件の生履歴注入）へ自動デグレードする（§13.3-4）。シナプス抽出は**ヒューリスティック**（LLM コストゼロ）。時刻文脈の再ランキング重みは **0.1 固定**（`gemini.ts` 内定数 `SYNAPSE_TIME_BIAS_WEIGHT`、意味埋め込みには混ぜず KNN 後のスコア補正のみ）。設定可能なチューニング値は以下のみ（有効/無効の切替は持たない）。

| 設定（環境変数） | 役割 |
|---|---|
| `SYNAPSE_RECALL_K` | 1st Hop KNN の取得件数（既定 5） |

### 13.5 統合点（`gemini.ts` 秘書モード）
1. ツール dispatch ごとに `recordToolOutcome`（status/latency、秘匿マスク済 digest）。
2. `buildRecallSection` が `assembleRecall` で 1st Hop 想起を取得し、`systemInstruction` 末尾へ「関連する過去の記憶」セクションとして追記（想起したシナプスは鮮度更新）。
3. 応答確定後 `maybeExtractSynapse` を fire-and-forget で起動（永続化＋索引登録）。

> 埋め込みは現状 R1 ではローカルの**ハッシュ n-gram（`model_version="hash-ngram-v1"`、語彙的フォールバック）**。`Embedder` trait と cargo feature `onnx` を差し替え点として用意済み（将来 bge-micro INT8 等の ONNX モデルへ換装）。汎用モード（ギルド常駐）への想起/抽出の配線は R1 では秘書モードのみ実装で、tool_outcomes 記録のみ共通 dispatch 経由で両モードに効く。

### 13.6 会話ログ機能との整合（重複の解消）
L2 連想想起は「過去の知見を能動的・連想的に思い出す」役割を担い、LLM が明示的に呼んだ時だけ動く受動的な全文検索 `searchConversationLogs` を置き換える（設計 [`synapse_cognitive_architecture.md`](../design/synapse_cognitive_architecture.md) §1.2 の「想起が受動的」という限界の解消）。このため **`searchConversationLogs`（秘書・ギルド両モード）はコードから廃止**した。一方、時系列順の文脈が本質的に重要で連想想起では代替できない **`summarizeConversationTopic`（トピック要約）は維持**する。会話の永続化（`message_logs` + `message_logs_fts` FTS5）はシナプス抽出の種・トピック要約・日報の会話サンプル（`reportService`）・返信チェーン・コスト可視化・監査に不可欠なため不変。

---

## 14. 機能モジュール化（schema v16 / P1〜P6 実装済み）

設計の全文は [`docs/design/function_modularization.md`](../design/function_modularization.md)。本節は実装規範への要点。

- **目的**: ユーザーが Bot ごとに「有効にする機能モジュール」を選び、有効モジュールの宣言のみ LLM へ渡し、管理 UI も該当設定のみ表示する。
- **粒度**: 機能モジュール単位（todo / finance / schedule … 約14モジュール）。安定 ID（永続化キー＝**リネーム禁止**）を `src/functions/moduleCatalog.ts` の `MODULE_CATALOG` が一元管理し、`MODULE_CAPABILITY_MAP` はこれから導出する（重複定義しない）。
- **二段フィルタ**: ① capability フィルタ（Bot が持つ capability のモジュール）→ ② enabled-module フィルタ（ユーザーが ON にしたモジュール）。`core`（`richContent` 等 `selectable=false`）は常時有効・選択不可でフィルタ対象外。
- **解決順（P6 ユーザー×Bot 上書き層）**: ① `bot_user_modules(bot_id, user_id)` に行があればそれを採用（`[]`=全 OFF も尊重）→ ② 無ければ `bots.enabled_modules`（Bot 既定）→ ③ どちらも未設定なら全有効（後方互換）。デフォルト Bot（`system_default`）含む全 Bot でユーザー個別に切替可能。編集権限は**アクセス権のある各ユーザーが自分の上書きのみ**。
- **解決ロジック/キャッシュ**: `src/services/botModules.ts`（`resolveEnabledModulesForUser` / `setUserModules` / `resolveBotEnabledModules` / `invalidate*Cache`）。`gemini.ts` が `getFunctionModulesForCapabilities(caps, enabledModules)` 経由で適用。永続化は `src/db/botUserModulesRepo.ts`。
- **API**: `GET/POST /api/bots/modules`（`botAttributeRoutes.ts`）。保存後にモジュール/能力キャッシュを invalidate。
- **不変条件**: 「未設定＝全有効」を破らない（既存 Bot は挙動不変）。capability に属さない ID は適用しない。

---

## 15. 汎用チャット API / デスクトップクライアント（Phase 0/1 バックエンド実装済み）

設計は [`docs/design/desktop_client/`](../design/desktop_client/index.md)。Discord 非依存の会話コア `processMessage()` を**無改修で再利用**し、クライアント非依存の汎用チャット入口を新設する。バックエンド（Phase 0/1）は実装済み・**実クライアント（egui）は Phase 2 以降で未実装**。

- **認証（Phase 0）**: OAuth デバイスフロー。`/api/device/*`（`deviceAuthRoutes.ts`）でコード発行→Web 承認→長命トークン交付。トークンは `desktopAuthService.ts` がハッシュ保存（`desktop_tokens`、`desktopTokenRepo.ts`）。デバイス管理は `deviceMgmtRoutes.ts`、クライアント配布は `desktopClientRoutes.ts`。
- **WS チャット（Phase 1）**: `GET /ws/chat`（`chatWebSocket.ts`）。`server.ts` の upgrade を Bearer トークン + `?botId=` の所有/共有検証で受理（1 接続 = 1 Bot 束縛）。`chatChannelService.ts` が WS フレーム ↔ `processMessage` を橋渡しし、ボタン等コンポーネントは `componentInteractionService.ts` で処理。
- **プロトコル**: 受信 `msg`/`reset`/`ping`/`interaction`、送信 `ready`/`status`/`interim`/`done`/`push`/`update`/`error`（詳細は [backend_api.md](../design/desktop_client/backend_api.md)）。
- **不変条件**: 会話コア（`gemini.ts` / `turnPlanner.ts` / `llmClient.ts` / `functions/*`）は触らない。`primary` は Bot プライマリ概念が未導入のため `id === "system_default"` を暫定採用。
