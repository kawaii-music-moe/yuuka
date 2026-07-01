I now have complete, primary-source-backed evidence. I've confirmed the existing code uses `@google/generative-ai` (the deprecated legacy SDK) with the classic `generateContent` REST path, model `gemini-3.1-flash-lite`. I've resolved the crucial 2026 finding: the Interactions API is now GA and the "front door," while generateContent is officially "legacy" but fully supported. Let me compile the verification report.

---

# Gemini × Rust 設計検証レポート（2026年7月時点）

> 一次情報（ai.google.dev / Google 公式ブログ / crates.io）で裏取り済み。推測箇所は明示。既存コードは `@google/generative-ai`（旧 Node SDK）＋クラシック `generateContent` 経路、モデル `gemini-3.1-flash-lite` を使用（`src/services/llmClient.ts:19`, `src/gemini.ts`）。

## ⚠️ 設計への影響（最重要・先頭掲示）

1. **【API面の分岐が2026年に発生】Interactions API が2026年6月にGA化し、Gemini の「正面玄関（primary interface）」に昇格した。** 従来の `generateContent` は公式に **「legacy（レガシー）」** と明記された。ただし **「fully supported（完全サポート継続）」** かつ **メインライン Gemini モデルは当面 generateContent にも投入され続ける** と公式明言あり。**結論: Rust 書き換えでは `generateContent` を採用してよい（既存挙動を1:1移植できる）が、「新しい frontier 機能・長時間エージェント機能は Interactions API 側にのみ載る」ことを設計上のリスクとして記録すべき。**（確信度: 高）

2. **【Node SDK 廃止】既存の `@google/generative-ai` は Google 非推奨。** Rust移植と同時に「薄い自前 reqwest ラッパ」へ寄せるのが妥当。crate 依存は限定的推奨（後述）。（確信度: 高）

3. **【リクエスト構造の非互換に注意】** Interactions API と generateContent は **JSON フィールド名が別物**（例: generateContent は `inlineData`/`mimeType`/`functionCall`/`functionResponse`、Interactions は `type:image`/`mime_type`/`function_call`/`function_result`/`call_id`/`previous_interaction_id`）。Web検索・WebFetch のサンプルが両APIで混在するため、**Rust struct は generateContent の camelCase 仕様に厳密に合わせること**（下記1参照）。既存TSコードは generateContent 準拠なので、そのまま移植すれば整合する。（確信度: 高）

4. **`gemini-3.1-flash-lite` は実在する GA モデル。コード変更不要。**（確信度: 高）

---

## 1. Function Calling の REST 構造（generateContent / 2026現行）

**結論:** 既存コードが使う構造は現行仕様で有効。並行呼び出しも公式サポート。Rust 移植は素直に可能。

**バージョン・エンドポイント:**
```
POST https://generativelanguage.googleapis.com/v1beta/{model=models/*}:generateContent
```
認証は `?key=API_KEY` クエリ or `x-goog-api-key` ヘッダ。

**リクエスト body（トップレベル、camelCase）:**
- `contents[]`（必須, `role`: `"user"`/`"model"`, `parts[]`）
- `tools[]`（`functionDeclarations[]` を含む）
- `toolConfig`（`functionCallingConfig`）
- `systemInstruction`（Content 型）
- `generationConfig`, `safetySettings[]`, `cachedContent`

**Part の各フィールド（camelCase）:**
- `text`（string）
- `inlineData`: `{ mimeType, data(base64) }`
- `functionCall`: `{ name, args(object) }` ← モデル出力
- `functionResponse`: `{ name, response(object) }` ← アプリが返す
- `fileData`: `{ fileUri, mimeType }`

**tools[].functionDeclarations[]:** `name` / `description` / `parameters`（JSON Schema の **OpenAPI サブセット**。公式に「only a subset of the OpenAPI schema is supported」）。

**toolConfig.functionCallingConfig.mode:** `MODE_UNSPECIFIED` / **`AUTO`**（既定・呼ぶか否かをモデル判断）/ **`ANY`**（必ずいずれかの関数を呼ぶ）/ **`NONE`**（呼ばない）。加えて **`allowedFunctionNames[]`** で ANY 時の候補を絞れる。→ 既存コードの「完了ハルシネーション是正で `mode:ANY` + `allowedFunctionNames`」ロジック（`src/gemini.ts:579`）はそのまま有効。（確信度: 高）

**往復（ツール実行ループ）の正確な形:**
1. `contents` にユーザ発話を積んで `generateContent`。
2. レスポンス `candidates[0].content.parts[]` に **1個以上の `functionCall`** が返る。
3. アプリが各関数を実行。
4. **モデルの `content`（functionCall入り）を `contents` に push → 続けて `role:"user"`（もしくは `"function"`）で `parts:[{functionResponse:{name, response}}]` を push** → 再度 `generateContent`。
5. `functionCall` が出なくなるまで反復（既存の maxIterations=10 ループと同型）。

→ 既存 TS（`src/gemini.ts:699-720`）の往復実装が **現行仕様のリファレンス実装そのもの**。Rust 移植はこの形をそのまま型付き struct に落とせばよい。

**複数ツール並行呼び出し:** **公式サポート。**「parallel function calling（1ターンで独立した複数関数を同時に返す）」＋「compositional/sequential function calling（get_location→get_weather のような連鎖）」の両方が明記。既存ループは `functionCalls.length` を回して全件実行しており、並行呼び出しに対応済み。（確信度: 高）

> 注意: Interactions API 系ドキュメントでは同概念が `tool_choice.allowed_tools.mode`（小文字 `auto`/`any`/`none`/`validated`）、`function_result`＋`call_id` という**別スキーマ**で書かれている。generateContent へ移植する際にこれを混入させないこと（⚠️影響3）。

---

## 2. ストリーミング（streamGenerateContent + SSE）

**結論:** SSE で逐次取得可能。reqwest でストリーム行分割 → `data:` 行の JSON を逐次 deserialize する方針で堅牢。

**エンドポイント:**
```
POST https://generativelanguage.googleapis.com/v1beta/{model=models/*}:streamGenerateContent?alt=sse
```
- **`alt=sse` 必須。** 付けないと「SSE ではなく1個の巨大 JSON 配列」で返る（公式挙動）。
- Content-Type: `text/event-stream`。各イベントは `data: {"candidates":[...],"usageMetadata":{...}}` 形式。トークンは `candidates[0].content.parts[0].text` を連結。`finishReason` で終端検知。

**reqwest 実装方針（推奨）:**
- `reqwest::Response::bytes_stream()`（`futures_util::StreamExt`）でチャンクを受け、行バッファに貯めて `\n` 区切りで `data: ` プレフィックス行を抽出 → `serde_json::from_str::<GenerateContentResponse>()`。
- もしくは SSE 専用 crate（`eventsource-stream` 等）を薄く噛ませる。
- **注意点（落とし穴）:** チャンク境界が JSON/行の途中で切れるため、必ず**行バッファリング**すること（チャンク単位で parse すると壊れる）。既存 yuuka は非ストリーミング（`generateContent` 一括 + 疑似「入力中…」演出）なので、**ストリーミングは移植必須ではない**。導入するなら Function Calling ループとの併用時に「functionCall はストリーム途中でも完結してから実行」する制御が要る。（確信度: 高＝エンドポイント/SSE形式、中＝reqwest実装細部は一次コード例未確認）

---

## 3. マルチモーダル（レシート画像入力）

**結論:** 既存の `inlineData`（base64）方式が現行仕様で有効。20MB 超・再利用時のみ Files API。レシート1枚は基本 inline で十分。

**現行の与え方（generateContent, camelCase）:**
```json
{ "inlineData": { "mimeType": "image/jpeg", "data": "<base64>" } }
```
→ 既存 `src/gemini.ts:1036` の実装と一致。

**サイズ閾値（落とし穴）:** **inline は「リクエスト総量（テキスト＋システム指示＋inlineバイト）20MB まで」**（公式明記）。レシート写真1枚は通常この範囲内なので inline で問題なし。20MB 超や複数リクエストでの画像再利用は **Files API**（`files.upload` → `fileData:{ fileUri, mimeType }`）。

**対応 MIME:** `image/png` / `image/jpeg` / `image/webp` / `image/heic` / `image/heif`。

**Rust 実装:** 画像バイト列を `base64` crate で encode し `inlineData` struct に詰めるだけ。Files API を使う場合はマルチパート/resumable upload が必要でやや重いので、**レシート用途なら inline 一本で開始し、Files API は将来対応でよい**。（確信度: 高。ただし image-understanding ページ自体は Interactions 系 `type:image`/`mime_type` サンプルを表示していた点に注意 → generateContent では必ず `inlineData`/`mimeType` を使う）

---

## 4. 現行モデルID（2026年7月）

**結論: `gemini-3.1-flash-lite` は実在の GA モデル。コードのモデル名は変更不要。**

`ai.google.dev/gemini-api/docs/models` で確認できた 2026年時点の主な GA モデル:
- **`gemini-3.5-flash`**（最新の高知能 Flash。GA）
- **`gemini-3.1-flash-lite`**（最安・低レイテンシ。GA）← **コードで使用中。有効。**
- `gemini-2.5-pro` / `gemini-2.5-flash` / `gemini-2.5-flash-lite`（GA、継続提供）

Preview（本番不可）: `gemini-3.1-pro-preview`, `gemini-3-flash-preview` 等。**落とし穴: `gemini-3.1-flash-lite-preview` は 2026-07-09 に廃止予定**なので、**suffix なしの GA 名 `gemini-3.1-flash-lite` を使うこと（コードは既に正しい）**。画像生成は `nano-banana-2`/`nano-banana-pro`（旧 `gemini-3.1-flash-image` 系）。（確信度: 高）

> 命名現況: `gemini-<世代>.<マイナー>-<tier>`。tier は `pro` / `flash` / `flash-lite`。3.x 系は 3.1 と 3.5 が併存（3.5 が上位知能、3.1-flash-lite がコスト最適）。

---

## 5. Rust クライアント: crate vs 自前 reqwest ラッパ

**結論: 「reqwest + serde の薄い自前型付きラッパ」を第一推奨。** コミュニティ crate は成熟度・保守にばらつきがあり、厳格エラー（thiserror）方針・レガシー generateContent 固定・Function Calling ループの細かな制御（mode=ANY 是正、並行実行、maxIterations）を自前で握る現設計と、自前ラッパの方が相性が良い。

**crate 実地調査（crates.io 一次情報, 2026）:**
- **`google-generative-ai-rs`（avastmick, 非公式・"Python SDK 模倣"）:** 最新 v0.3.4（**2024-12** で更新停滞気味）。**Function calling と embedContent が未実装（outline task 扱い）**、`reqwest`/`reqwest-streams` 依存が **outdated** と明記。→ **本用途では不適（Function Calling 必須のため除外）。**（確信度: 高）
- **`gemini-rust`（flachesis）:** 最新 **v1.7.1**、全 API 実装・Function Calling（`schemars` で OpenAPI schema 生成）・streaming・画像/バイナリ・**Gemini 3 Pro/Flash 対応**・batch・caching・thinking を謳う。DL 累計 ~3.3万、208 commits。**機能面は最も充実。** ただし 3rd-party 保守依存、エラー型が thiserror かは未確認、内部が generateContent/Interactions どちら固定かも明言なし。→ **採用するなら候補筆頭だが、API面（レガシー固定リスク）とエラー方針を実コードで要確認。**（確信度: 中＝機能記述はcrates.io記載、実装詳細は未検証）
- その他（`gemini-client-api`, `genai-rs`, `adk-gemini`, `gemini-rs` 等）: Function Calling/streaming を謳うものが複数あるが、いずれも個人メンテの新興 crate で、長期保守・エラー方針の保証は弱い。

**推奨（厳格エラー方針との相性込み）:**
- **自前ラッパを基本線**とする。理由:
  1. 表面積が小さい（generateContent / streamGenerateContent の2エンドポイント＋ Part/Content/Tool/ToolConfig の struct 群だけ）。既存 TS 型定義がそのまま移植図面になる。
  2. **エラーを thiserror で完全掌握**できる（`#[error]` で 429=RateLimit / 5xx=ServerError / RetryInfo(`google.rpc.RetryInfo` の `retryDelay`) パース / JSON deser 失敗 / タイムアウト を variant 化）。既存の `isRateLimitError`/`isServerError`/RetryInfo バックオフ（`src/gemini.ts:372-463`）を 1:1 で型に落とせる。crate 経由だと 429/RetryInfo の取り出しがブラックボックス化しやすい。
  3. Function Calling ループの並行実行・`mode:ANY`+`allowedFunctionNames` 是正・maxIterations といった**アプリ固有の制御**を crate の抽象に縛られず書ける。
- **依存 crate（薄いラッパ用）:** `reqwest`(rustls, stream)＋`serde`/`serde_json`＋`thiserror`＋`base64`＋（ストリーム採用時）`futures-util`/`eventsource-stream`＋`tokio`。JSON Schema を Rust 型から自動生成したいなら `schemars` を任意採用。
- **`gemini-rust` は「プロトタイプ加速」用の予備案**。厳格エラー・レガシー固定が実装で確認できれば採用短絡も可。ただし本番の中核依存としては自前ラッパの方が制御・監査性で勝る。

（確信度: 中〜高。crate 存在・機能は一次確認済み。「自前ラッパ推奨」は厳格エラー＋Function Calling制御要件からの設計判断。）

---

## 総括（Rust 移植の設計指針）

| 項目 | 決定 | 確信度 |
|---|---|---|
| API 面 | **generateContent（v1beta, レガシーだが完全サポート）を採用**。既存挙動を1:1移植可。Interactions API は将来オプション | 高 |
| Function Calling | tools/functionDeclarations/toolConfig.functionCallingConfig(AUTO/ANY/NONE, allowedFunctionNames)、functionCall↔functionResponse 往復、並行呼び出し = すべて現行仕様で有効 | 高 |
| ストリーミング | streamGenerateContent?alt=sse。reqwest bytes_stream＋行バッファ。移植必須ではない | 高/中 |
| マルチモーダル | inlineData(mimeType,data base64)。20MB 超のみ Files API。レシートは inline で十分 | 高 |
| モデルID | `gemini-3.1-flash-lite`（GA）は実在。**変更不要**。preview suffix は使わない | 高 |
| Rust クライアント | **reqwest+serde+thiserror の自前薄ラッパを第一推奨**。`gemini-rust`(v1.7.1) は予備。`google-generative-ai-rs` は FC 未実装で不適 | 中〜高 |

**移植時の落とし穴 総まとめ:** (a) generateContent は camelCase・`inlineData`/`functionResponse`、Interactions は snake系・`function_result`/`call_id` — Web資料が両者混在するので **generateContent スキーマに厳密固定**。(b) SSE は `alt=sse` 必須＆行バッファ必須。(c) preview モデル名（特に `-preview` 付き flash-lite は 2026-07-09 廃止）を避け GA 名固定。(d) 429/5xx の RetryInfo(`retryDelay`) を thiserror variant で保持しバックオフ。(e) `ANY` モードは「必ず何か呼ぶ」ため、是正リトライ以外の常用は避け AUTO を既定に。

Sources:
- [Function calling with the Gemini API](https://ai.google.dev/gemini-api/docs/function-calling)
- [Generating content (generateContent REST reference)](https://ai.google.dev/api/generate-content)
- [Gemini generateContent API (legacy note)](https://ai.google.dev/gemini-api/docs/interactions)
- [Interactions API overview](https://ai.google.dev/gemini-api/docs/interactions-overview)
- [Interactions API GA announcement (Google blog)](https://blog.google/innovation-and-ai/technology/developers-tools/interactions-api-general-availability/)
- [Models list](https://ai.google.dev/gemini-api/docs/models)
- [Gemini 3.1 Flash-Lite model page](https://ai.google.dev/gemini-api/docs/models/gemini-3.1-flash-lite)
- [Image understanding](https://ai.google.dev/gemini-api/docs/image-understanding)
- [crates.io: gemini-rust](https://crates.io/crates/gemini-rust) / [github flachesis/gemini-rust](https://github.com/flachesis/gemini-rust)
- [lib.rs: google-generative-ai-rs](https://lib.rs/crates/google-generative-ai-rs)