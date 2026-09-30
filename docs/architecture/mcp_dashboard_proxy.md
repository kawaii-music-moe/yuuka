# MCP ダッシュボード プロキシ アーキテクチャ

> **⚠️ 注記（旧 Node 実装時点の設計）**: 本書の「関連ファイル」表や CSP の出所などが挙げる `src/server.ts`・`src/server/routes/mcpRoutes.ts`・`src/services/mcpClient.ts`・`src/public/*` は Node バックエンド／旧バニラ SPA のファイルで、現行リポジトリには存在しません。現行の実装は `crates/yuuka-mcp`（ダッシュボード/プロキシ API）、`crates/yuuka-web`（本体の CSP・セキュリティヘッダ）、`frontend/src/routes/mcp/McpDashboardModal.svelte`（iframe 埋め込み UI）に移っています。方式（サンドボックス iframe・サーバー側でのトークン解決）の設計意図の参照用として読み、挙動の詳細は現行コードで確認してください。本文は旧実装のまま未更新です。

## 概要

MCP サーバー（例: ywrk-mcp）のダッシュボードを yuuka の管理画面に**サンドボックス iframe 埋め込み**で表示する際、
**ユーザーがトークンを手入力しなくても自動的に認証済み状態で起動する**仕組みです。

ダッシュボードの実体は MCP サーバー側の SPA（単一 HTML）ですが、その JS が必要とする Bearer トークンと
API エンドポイントを、yuuka がサーバー側で解決・中継します。ブラウザ（クライアント）には実 YWRK_TOKEN を渡しません。

> **埋め込み方式について（重要 / セキュリティの肝）**
>
> ダッシュボードHTMLは**サードパーティの MCP サーバー由来の任意コード**である。これを yuuka 本体の
> ドキュメント（Shadow DOM 含む）や `allow-same-origin` iframe で実行すると、その JS が yuuka の
> **Cookie・localStorage・DOM・同一オリジンAPI**へフルアクセスでき、実質的なオリジン内任意コード実行
> （XSS/RCE）になる。Shadow DOM は CSS/DOM ツリーを隔離するだけで **JS は隔離しない**点に注意。
>
> そこで **`<iframe sandbox="allow-scripts allow-forms">`（`allow-same-origin` を付けない）** に隔離する。
> サンドボックスにより iframe は **不透明オリジン**となり、ダッシュボードの JS は本体オリジンの資源へ
> 一切到達できない。CSS も iframe ドキュメント内に閉じ込められるため本体スタイルへの波及も起きない。
>
> 過去の実装（`allow-same-origin` 付き iframe / Shadow DOM 直接注入）は隔離が無効で、本方式で置き換えた。

---

## コンポーネント構成

```
[ブラウザ]
  │  yuuka セッション Cookie（dashboard ルートのナビゲーション要求にのみ届く）
  ▼
[yuuka (Node.js)]
  ├─ GET  /api/mcp-servers/:id/dashboard   ← HTML 取得・書き換え・専用CSPで text/html を返す（auth: user）
  └─ POST /proxy/mcp/:id/mcp               ← MCP API プロキシ（auth: none / proxyToken 単独認証）
       │  Authorization: Bearer <YWRK_TOKEN>  ← DB から復号
       ▼
[ywrk-mcp (Rust)]
  ├─ GET  /dashboard   ← SPA HTML を返す（無認証配信・frame-ancestors 'none'）
  └─ POST /mcp         ← MCP JSON-RPC（Bearer 認証必須）
```

iframe（不透明オリジン）→ yuuka の関係は次の通り:

```
[親ドキュメント: yuuka オリジン]
   <iframe sandbox="allow-scripts allow-forms"
           src="/api/mcp-servers/:id/dashboard">      ← src 取得は同一オリジンGET（Cookieが届く）
        │
        ▼
   [iframe ドキュメント: 不透明オリジン / 本体から完全隔離]
        SPA → fetch("/proxy/mcp/:id/mcp", Bearer proxyToken)   ← クロスオリジン（要 CORS: ACAO:null）
```

---

## トークン自動注入の仕組み（`GET /api/mcp-servers/:id/dashboard`）

このルートは **iframe の `src` として直接読み込まれる**ため、`text/html` を返す（JSON ではない）。
real URL なので親の CSP を継承せず、**このルート専用の CSP** をレスポンスヘッダで付与できる。

1. yuuka サーバーが ywrk-mcp の `/dashboard` を Bearer 認証付きで取得（HTML）
2. 取得した HTML に対して以下の書き換えを実施してから text/html で返す:

   | 書き換え対象 | 変更前 | 変更後 |
   |---|---|---|
   | `var MCP_PATH` | `"/mcp"` | `"/proxy/mcp/<id>/mcp"` |
   | SPA の `tokenFromHash()` 関数 | `location.hash` からトークンを読む | `window.__mcpProxyToken__` を返す |
   | akizakura.css の `<link>` | 外部CDN参照 | サーバー側で取得した CSS を `<style>` でインライン化（失敗時は `<link>` を残す）|
   | `<head>` 直後 | — | `<script>window.__mcpProxyToken__ = "<proxyToken>";</script>` |

   ※ `<proxyToken>` は 256-bit 乱数（hex）の短命トークン（有効期限1時間）。`window.__mcpProxyToken__` は
     iframe の window 内に閉じるため、隔離により親からも他オリジンからも読めない。
   ※ **書き換え検証**: 各 `String.replace` は不一致時に元文字列をそのまま返すため、`MCP_PATH` と
     `tokenFromHash` の置換が実際に起きたかを検証し、別リポジトリ(ywrk-mcp/dashboard.html)の
     フォーマットがドリフトした場合は **壊れたページを 200 で返さず 502 で失敗させる**（サイレント破損防止）。
   ※ 失敗時は JSON ではなく簡素な **HTML エラーページ**を返す（iframe 内に生 JSON を表示させない）。

3. クライアント（`openMcpDashboard`）は `<iframe sandbox="allow-scripts allow-forms" src=...>` を生成し、
   モーダル内のコンテナへ追加するだけ（HTML の再パースやスクリプト再生成は不要）。

4. iframe 内で SPA が起動すると:
   - `<head>` 内の `<script>` が先に実行 → `window.__mcpProxyToken__` にトークンをセット（iframe の window）
   - SPA の `boot()` が書き換え後の `tokenFromHash()` でトークンを取得
   - トークンが非 null → トークン入力フォーム（token-gate）は表示されず、即座にデータ取得へ進む

---

## サンドボックス iframe の仕組み（`openMcpDashboard` / `injectMcpDashboardHtml` は廃止）

```
1. <iframe sandbox="allow-scripts allow-forms"> を生成（allow-same-origin は付けない）
2. iframe.src = "/api/mcp-servers/:id/dashboard"   （サーバーが text/html を返す）
3. コンテナへ append
```

- `allow-same-origin` を**付けない**ことで iframe は不透明オリジンになり、yuuka 本体の
  Cookie・localStorage・DOM・`window.parent` へのアクセスが SOP により遮断される。
- `allow-forms` は SPA のフォーム送信のため。必要なら `allow-modals` 等を追加してよいが、いずれも
  **`allow-same-origin` は決して併記しない**（併記するとサンドボックスが実質無効化される）。
- モーダルを閉じる際は `teardownMcpDashboard()` で iframe を破棄する（コンテナを空にするだけ。
  iframe 破棄により SPA が登録した window スコープのリスナーも一緒に消える）。

---

## CSP（Content-Security-Policy）⚠️ 二層構成

| 文書 | CSP の出所 | 要点 |
|---|---|---|
| 親（yuuka 本体, index.html） | `src/server.ts` の `CSP` 定数（静的配信に付与）| `frame-src 'self'` で同一オリジンの dashboard ルートを許可。akizakura 等のダッシュボード固有オリジンは**不要**（iframe 内に閉じるため除去済み）|
| iframe（ダッシュボード） | `mcpRoutes.ts` の dashboard ルートが**レスポンスヘッダで付与**| real URL なので親 CSP を継承せず独立。下記参照 |

iframe ルートの CSP（`mcpRoutes.ts` 内で動的生成）:

```
default-src 'none';
base-uri 'none';
script-src 'unsafe-inline';
style-src 'unsafe-inline' https://akizakura.pages.dev https://fonts.googleapis.com;
font-src https://fonts.gstatic.com data:;
img-src 'self' data: <yuuka絶対オリジン>;
connect-src <yuuka絶対オリジン>;
frame-ancestors 'self';
```

> **重要**: `connect-src` を `'self'` ではなく **yuuka の絶対オリジン**（`config.baseUrl` 由来、未設定時は
> リクエストの Host）で指定する。iframe は不透明オリジンのため `'self'` が `/proxy/mcp` に解決されず
> fetch がブロックされる（旧実装で `allow-same-origin` を付けざるを得なかった根本原因）。絶対オリジン指定なら
> 不透明オリジンからでも `/proxy/mcp` へ到達できる。
>
> akizakura はサーバー側で `<style>` インライン化するため通常は `'unsafe-inline'` で通る。取得失敗時の
> フォールバック（元の `<link>` を残す）のために `style-src` に `akizakura.pages.dev` を許可している。
> 別の MCP ダッシュボードが別オリジンの CSS/フォント/画像を読む場合は、このルートの CSP を調整する
> （本体 CSP には影響しない）。

---

## MCP API プロキシの仕組み（`POST /proxy/mcp/:id/mcp`）

SPA の JS が `fetch("/proxy/mcp/<id>/mcp", { headers: { Authorization: "Bearer <proxyToken>" } })` を呼ぶ。
iframe は不透明オリジンのため**クロスオリジン**となり、Cookie は届かない。よって認証は **proxyToken 単独**を
主体とし、CORS は **`Access-Control-Allow-Origin: null`（Credentials 無し）** の安全な組合せで応答する。
`Authorization` カスタムヘッダのため**プリフライト(OPTIONS)** が飛ぶので、専用の OPTIONS ルートで応答する。

このルートは **`auth: "none"`**（Cookie セッションに依存しない）であり、以下を検証する:

1. **proxyToken**: サーバー内メモリの `proxyTokens` Map で `serverId` 一致と有効期限を確認
2. **発行ユーザーの再解決**: トークンに束縛した `userId` を `getUserByDiscordId` で引き直し、ユーザーが
   存在しなければ拒否（発行後の削除に追従）
3. **canManage / enabled の再検証**: 発行ユーザーの**現在のロール・権限**（降格）やサーバーの有効状態
   （無効化）が変化していないかを毎回再チェック
4. DB から暗号化された `YWRK_TOKEN` を取得し AES-GCM で復号
5. 復号した本物 Bearer トークンを付与して ywrk-mcp の `/mcp` へ POST をそのまま転送
6. ywrk-mcp のレスポンス（JSON または SSE）をブラウザへストリーミングで返す（ACAO:null を付与）

```
Browser (隔離iframe内SPA)          yuuka                     ywrk-mcp
  OPTIONS /proxy/mcp/1/mcp  →   204 + ACAO:null（プリフライト）
  POST /proxy/mcp/1/mcp     →   proxyToken 検証
  Origin: null                  + userId 再解決 + canManage + enabled
  Authorization: Bearer    →    復号(YWRK_TOKEN)
  <proxyToken>                  POST /mcp                  →  Bearer <real>
                           ←   SSE / JSON (streaming, ACAO:null) ←
```

なお、サーバーを**無効化・削除**すると、発行済み proxyToken は即時失効する
（`revokeProxyTokensForServer`）。開きっぱなしのダッシュボードが無効化後に中継し続けるのを防ぐ。

---

## セキュリティ設計

| 観点 | 対策 |
|---|---|
| サードパーティ JS の本体オリジン実行を防ぐ | `sandbox="allow-scripts"`（**`allow-same-origin` 無し**）で不透明オリジンに隔離。Cookie/localStorage/DOM/同一オリジンAPIへ到達不可 |
| 実 YWRK_TOKEN をブラウザに渡さない | 短命プロキシトークンのみ渡す。実 token は yuuka サーバー内でのみ復号 |
| トークンを推測・横取りされない | proxyToken は 256bit 乱数・`{serverId, userId}` 束縛・TTL1時間・無効化/削除で即時失効。iframe 隔離により親や他オリジンから読めない |
| 他ユーザーの MCP サーバーを操作できない | 中継のたびに発行ユーザーを再解決し `canManage` を再検証（system レベルは Admin のみ）|
| 発行後の権限・状態変化 | 中継のたびに `canManage`（降格）と `enabled`（無効化）を再検証。無効化/削除でトークン即時失効 |
| CORS の安全な最小化 | クロスオリジンは iframe→/proxy の1経路のみ。`ACAO:null` かつ `Allow-Credentials` 無し（Cookie 不使用）。認証は推測不能な proxyToken が主体 |
| 埋め込み SPA の DOM/スタイル隔離 | iframe ドキュメント内に CSS/JS を隔離。本体スタイルへ波及しない |
| SSE/長期接続のリソースリーク | `res.on("close")` で `AbortController` をキャンセル |
| 上流ストリームの途中エラー | reader ループでエラーを握り潰さず、レスポンスを異常終了させて「200 + 切り詰めボディ」を返さない |

---

## 関連ファイル

| ファイル | 役割 |
|---|---|
| `src/server/routes/mcpRoutes.ts` | `/api/mcp-servers/:id/dashboard`（HTML書き換え・トークン発行・書換検証・**専用CSPでtext/html返却**）・`/proxy/mcp/:id/mcp`（**token単独認証**・多層再検証・ACAO:null）・`OPTIONS /proxy/mcp/:id/mcp`（プリフライト）|
| `src/server.ts` | 本体 CSP 定数（`frame-src 'self'`。akizakura 等は iframe 側 CSP へ移動）|
| `src/services/mcpClient.ts` | `buildAuthHeader()`・`fetchMcpDashboardHtml()`・`fetchAkizakuraCss()`（インライン化用CSS取得・TTLキャッシュ）|
| `src/public/app.js` | `openMcpDashboard()`（サンドボックス iframe 生成）・`teardownMcpDashboard()`（破棄）|
| `src/public/index.html` | `<div id="mcp-dashboard-container">` の定義 |
| `ywrk-mcp/src/main.rs` | `/dashboard`（`frame-ancestors 'none'`）・`/dashboard/enable`（無効時 404）|
| `ywrk-mcp/src/dashboard.html` | SPA 本体。`tokenFromHash()` をサーバー側で書き換えて `window.__mcpProxyToken__` を参照させる |

---

## シーケンス図

```
ユーザー        ブラウザ              yuuka              ywrk-mcp
   │               │                    │                    │
   │ 管理ページ開く │                    │                    │
   │──────────────▶│ <iframe src=/api/mcp-servers/:id/dashboard>（Cookie付GET）│
   │               │───────────────────▶│ GET /dashboard     │
   │               │                    │───────────────────▶│
   │               │                    │◀─── HTML ─────────│
   │               │                    │ MCP_PATH/tokenFromHash 書換 │
   │               │                    │ akizakura inline・token注入 │
   │               │◀ text/html + 専用CSP│                    │
   │               │ 不透明オリジンで SPA 起動（本体から隔離）│
   │               │ window.__mcpProxyToken__ からtoken取得   │
   │               │                    │                    │
   │               │ OPTIONS /proxy/mcp/:id/mcp（プリフライト）│
   │               │◀ 204 + ACAO:null ──│                    │
   │               │ POST /proxy/mcp/:id/mcp（Origin: null）  │
   │               │ Authorization: Bearer <proxyToken>      │
   │               │───────────────────▶│ token検証 + userId再解決 │
   │               │                    │ canManage/enabled 再検証 │
   │               │                    │ POST /mcp（Bearer <real>）│
   │               │                    │───────────────────▶│
   │               │◀ stream(ACAO:null)─│◀─── SSE/JSON ─────│
   │◀─ データ表示 ─│                    │                    │
```
