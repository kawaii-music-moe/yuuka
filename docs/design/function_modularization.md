# 機能モジュール化 設計書（Function Modularization）

- ステータス: P1〜P6 実装済み（秘書モード・ユーザー×Bot単位）。汎用モード・MCP は次フェーズ
- ブランチ: `feature/function-modularization`（カタログ分割リファクタは `refactor/function-modularization-cleanup`）
- 作成日: 2026-06-30
- 関連: `src/functions/moduleCatalog.ts`, `src/functions/index.ts`, `src/services/botModules.ts`, `src/services/botCapabilities.ts`, `src/server/routes/botAttributeRoutes.ts`

> **⚠️ 旧 Node 実装時点の記述です:** 上記の `src/*.ts` は Node 実装（[#68](https://github.com/kawaii-music-moe/yuuka/pull/68) で撤去済み）のファイルで、現行リポジトリには存在しません（撤去前のソースは `git show 390df39^:<path>` で参照可）。現行の Rust 実装では、モジュールのカタログと `GET/POST /api/bots/modules` は `crates/yuuka-orchestrator`（`module_catalog.rs`・`bot_attr_routes.rs`）、ユーザー×Bot 上書き層は `bot_user_modules` テーブル（`crates/yuuka-db/migrations/V17__baseline.sql`）にあります。**LLM へ渡すツール宣言を有効モジュールで絞り込む処理は現状未移植**です（`crates/yuuka-tools/src/native.rs` の注記参照）。本文の実装規範への言及（architecture_v2 §14 等）は履歴資料を指し、現行の規範ではありません。矛盾する場合は現行コードが正です。

## 1. 背景と目的

Function が18ファイル・約6,400行まで増え、多機能で便利になった一方、肥大化している。
全Botに全Functionの宣言を渡すことで、

- LLMコンテキストの無駄（不要なツール宣言の混入）
- 管理UIに全機能の設定が常に並び、ユーザーに不要な項目まで露出

という問題が出ている。

**目的**: ユーザーが Bot ごとに「有効にする機能モジュール」を選択でき、
- LLMには有効モジュールの宣言のみ渡す
- 管理UIには有効モジュールの設定のみ表示する

ことで、パーソナライズされた軽量な体験を提供する。

## 2. 決定事項（確定）

| 論点 | 決定 |
|------|------|
| 粒度 | **機能モジュール単位**（todo / finance / schedule … 約14モジュール）でON/OFF |
| スコープ | **Botごと**（既存 `bots.capabilities` と同列に保存） |
| capability との関係 | **capability配下の追加レイヤ**。capabilityで大枠を決め、その中で個別モジュールをON/OFF。後方互換（未設定=全有効）を維持 |

## 3. 設計概要

### 3.1 二段フィルタリングモデル

メッセージ受信時の Function 解決を二段にする。

```
1. capability フィルタ（既存）   : Botが持つ capability のモジュールのみ
2. enabled-module フィルタ（新規）: そのうちユーザーがONにしたモジュールのみ
                                     ※未設定(NULL)なら全モジュールON＝現行と完全一致
```

`core` capability のモジュール（`showRichContent` 等）は**常に有効・選択不可**とし、フィルタ対象外。

### 3.2 モジュールIDの導入

現状 `MODULE_CAPABILITY_MAP` はモジュール参照のみを持つ。
各エントリに安定した `id`（kebab/camel の文字列キー）と表示用メタを付与する。

```ts
// src/functions/moduleCatalog.ts （新規）
export interface ModuleCatalogEntry {
  id: string;              // 例: "todo", "finance"（永続化キー。変更不可）
  module: FunctionModule;
  cap: "core" | BotCapability;
  label: string;           // UI表示名（例: "ToDo・タスク管理"）
  description: string;      // UIの説明
  selectable: boolean;     // false = 常時有効（core等）。UIに出さない/OFF不可
  settingsKey?: string;    // 対応する管理UIタブ/設定セクションのキー（任意）
}

export const MODULE_CATALOG: ModuleCatalogEntry[] = [ /* … */ ];
```

`src/functions/index.ts` の `MODULE_CAPABILITY_MAP` はこのカタログから導出する（重複定義を避ける）。
モジュールIDは永続化キーになるため**リネーム禁止**（移行が必要な場合はエイリアス表を別途用意）。

> **実装メモ（refactor）**: 当初カタログ・各モジュール定義・解決ロジックは `index.ts` に同居していたが、
> 肥大化したため以下へ分割した（挙動不変）。
> - `src/functions/moduleCatalog.ts`: `MODULE_CATALOG` ＋ 解決関数
>   （`getFunctionModulesForCapabilities` / `getGuildAssistantFunctionModules` / `listSelectableModules` / `isKnownSelectableModule` / `getBaseFunctionModules`）。
> - `src/functions/browserModule.ts`: ブラウザ操作アダプタ（§3.5）。
> - `src/functions/richContentModule.ts`: リッチ返信（core・常時有効）。
> - `src/functions/index.ts`: 既存 import パス互換のための再エクスポート用ファサード。

### 3.3 候補モジュールID一覧（初期案）

| id | label | cap | selectable |
|----|-------|-----|-----------|
| todo | ToDo・タスク管理 | secretary | ✓ |
| schedule | スケジュール | secretary | ✓ |
| reminder | リマインダー | secretary | ✓ |
| finance | 家計・支出管理 | secretary | ✓ |
| browser | ブラウザ操作・Web検索 | secretary | ✓ |
| credential | 認証情報の保管 | secretary | ✓ |
| playbook | プレイブック・自動化 | secretary | ✓ |
| clipboard | クリップボード共有 | secretary | ✓ |
| contact | 連絡先 | secretary | ✓ |
| briefing | 朝刊・ニュース | secretary | ✓ |
| chart | グラフ生成 | secretary | ✓ |
| note | 個人メモ | memory | ✓ |
| conversation | 会話履歴・記憶検索 | memory | ✓ |
| richContent | リッチ返信 | core | ✗（常時有効） |

※ `mcp` 系（動的）と汎用モード（`getGuildAssistantFunctionModules`）の扱いは §6 で別途検討。

## 3.5 改訂: ユーザー×Bot 上書き層（P6）

当初は「Botごと」スコープだったが、共有のデフォルトBot（`system_default`: Discordトークンを
持たないユーザーへの提供 + システム通知）でも**ユーザーごと**に機能を切り替えたいという
要件により、**ユーザー×Bot単位の上書き層**を追加した。

解決順（発話ユーザー視点）:

```
1. ユーザー上書き  bot_user_modules(bot_id, user_id)  … 行があればこれを採用（[]=全OFFも尊重）
2. Bot既定        bots.enabled_modules               … 上書きが無ければフォールバック（ベースライン）
3. 全有効         （どちらも未設定なら）              … 後方互換
```

- 適用範囲: **全Bot**でユーザー個別（デフォルトBot含む）。
- 編集権限: アクセス権のある各ユーザーが**自分の上書きのみ**変更（owner限定を撤廃）。
- `bots.enabled_modules`（Bot既定）はベースラインとして残置（owner/admin用の編集UIは次フェーズ）。

## 4. データモデル

### 4.1 スキーマ拡張

`bots` テーブルに新列を追加（`src/db/migrations.ts`）。

```sql
ALTER TABLE bots ADD COLUMN enabled_modules TEXT; -- JSON配列。NULL = 全モジュール有効
```

- **NULL**: 後方互換。未設定の既存Botは現行どおり全モジュール有効。
- `["todo","finance"]`: 該当IDのみ有効（selectableなものに対して適用）。
- `[]`: selectableモジュールを全てOFF（coreのみ動作）。

セマンティクス: `selectable=false` のモジュールはこの配列の内容に関わらず常に有効。

### 4.2 解決ロジック

```ts
// src/services/botModules.ts （新規 or botCapabilities.ts に同居）
// 1. enabled_modules を読む（NULL→null）
// 2. capability解決の結果に対し、
//    - core/selectable=false は常に通す
//    - selectable=true は enabledSet===null なら全通し、else id∈enabledSet のみ
// capabilitiesCache と同様のメモリキャッシュ＋invalidateを用意
```

`getFunctionModulesForCapabilities(caps)` に enabledModules 引数を追加し、
`MODULE_CATALOG` のメタを使ってフィルタする形に拡張する。

## 5. API・UI

### 5.1 API

`src/server/routes/botAttributeRoutes.ts` に追加。

- **GET `/api/bots/modules`**: カタログ（selectableなモジュールのid/label/description/cap）＋当該Botの現在の有効状態を返す。
- **POST `/api/bots/modules`**: `{ botId, enabledModules: string[] | null }` を保存。
  - 認可は既存 `requireOwnedBot()` を踏襲。
  - 保存後に `invalidateBotModulesCache(botId)` ＋ `invalidateBotCapabilitiesCache(botId)`。
  - capability に属さないIDが来たら無視 or 400。

### 5.2 管理UI（`src/public/app.js` / `index.html`）

1. Bot属性カードに「有効な機能」セクション（チェックボックスリスト）を追加。
   - capability配下の selectable モジュールのみ列挙。Botが持たない capability のモジュールは非表示。
2. **設定タブの動的表示**: 各機能の設定タブ（ToDo設定・家計設定など）を、
   `enabled_modules` に含まれるモジュールのものだけ表示する。
   - `ModuleCatalogEntry.settingsKey` で「モジュール ↔ 設定タブ」を対応づける。
   - これが「管理UIにもその機能の設定のみ表示」要件の実体。

## 6. 未決事項 / 次フェーズ

- **MCP動的モジュール**: 既にBot単位でツール登録される。モジュールON/OFFの枠に乗せるか、現状維持か要検討。
- **汎用モード**（`getGuildAssistantFunctionModules`）への適用範囲。秘書モード優先で先行実装し、汎用モードは段階適用。
- **依存関係**: モジュール間に暗黙依存があれば（例: chart が finance データ前提）UIで注意喚起 or 自動有効化を検討。
- **プリセット**: 「最小構成」「フル」などモジュール選択のプリセットを将来用意。

## 7. 実装ステップ（フェーズ分割）

1. **P1 カタログ基盤**: `moduleCatalog.ts` 新設、`index.ts` をカタログ導出に置換（挙動不変・テスト維持）。
2. **P2 永続化**: migration（`enabled_modules`列）＋ `botRepo` 読み書き＋ 解決ロジック＋キャッシュ。
3. **P3 ランタイム適用**: `gemini.ts` / `getFunctionModulesForCapabilities` に enabledModules を流す。
4. **P4 API**: GET/POST `/api/bots/modules`。
5. **P5 UI**: チェックボックス＋設定タブの動的表示。
6. **P6 仕上げ**: ドキュメント更新、汎用モード/MCPの扱い確定。

各フェーズは「挙動不変（既存Bot=全有効）」を不変条件として、独立してマージ可能にする。
