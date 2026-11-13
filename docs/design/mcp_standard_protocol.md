# MCP を標準仕様どおりに使う（OAuth 2.1・MCP Apps）

> 状態: 実装済み（2026-11-12）。旧方式（ywrk-mcp 専用のダッシュボード埋め込み、`docs/architecture/mcp_dashboard_proxy.md`）は廃止した。

## 何を変えたか

| 部分 | 旧方式（独自） | 新方式（標準） |
|---|---|---|
| プロトコル | 公式 SDK `rmcp` 2.2 の Streamable HTTP | 同じく `rmcp`（3.9 に更新。OAuth の SSRF 勧告 RUSTSEC-2026-0457・0462 を解消） |
| 認証 | 固定の Bearer トークンを手入力して暗号化保存 | MCP 仕様の **Authorization（OAuth 2.1）**。固定トークンも OAuth 非対応サーバー向けに残す |
| 画面 | ywrk-mcp の `/dashboard/enable`・`/dashboard` を取得し、HTML を文字列置換（`MCP_PATH`・`tokenFromHash`）、`/proxy/mcp/:id/mcp` で生の JSON-RPC を中継 | **MCP Apps 拡張**（`io.modelcontextprotocol/ui`・仕様 2026-01-26）。ツールの `_meta.ui.resourceUri` が指す `ui://` リソースを表示し、画面からの呼び出しはホストが代わりに行う |
| HTTP 基盤 | MCP 用（reqwest 0.13・`rmcp` 用）とダッシュボード用（reqwest 0.12）の 2 系統 | `yuuka-mcp` は reqwest 0.13 の 1 系統だけ（MCP と OAuth で共用）。SSRF の判定は `yuuka-outbound` の `resolve_public_addrs` を共有 |

画面を出すのは **管理画面（連携サービス › MCP サーバー）だけ**。Web チャット・Discord では、これまでどおり結果のテキストだけを使う。

## OAuth 2.1 認可

手順そのもの（保護リソースメタデータ RFC 9728 → 認可サーバーメタデータ RFC 8414 / OIDC の発見、クライアント登録、PKCE S256、`resource` パラメータ RFC 8707、トークンの更新）は `rmcp` の `AuthorizationManager` / `AuthorizationSession` に任せる。yuuka が持つのは差し込み口だけ（`crates/yuuka-mcp/src/oauth.rs`）。

1. 接続時にサーバーが 401/403 を返したら、その `WWW-Authenticate` を覚え、一覧に「認可が必要」と出す。
2. 管理画面の「OAuthで認可」→ `POST /api/mcp-servers/:id/oauth/start`。発見とクライアント登録を行い、認可 URL を返す。ブラウザはその URL へ遷移する。
   - クライアント登録の優先順位（仕様どおり）: 事前登録（`clientId`・`clientSecret` を body で渡したとき）→ **Client ID Metadata Document**（認可サーバーが `client_id_metadata_document_supported` を示し、`BASE_URL` が https のとき。文書は `GET /api/mcp-servers/oauth/client-metadata.json`）→ 動的クライアント登録。
   - コールバック URL は `{BASE_URL}/api/mcp-servers/oauth/callback`。`BASE_URL` が無いと始められない（Host ヘッダから組み立てない）。
3. 認可サーバーから `GET /api/mcp-servers/oauth/callback?code&state` に戻る。`state` から認可待ちのセッションを引き（10 分・一度きり・始めたユーザーと同一であること）、コードをトークンへ交換する。続けて `tools/list` を取り直し、`/integrated/mcp?mcp_oauth=success|error&message=...` へ戻す。
4. 資格情報（rmcp の `StoredCredentials`：client_id・トークン応答・付与スコープ・発行者）は JSON にして `mcp_oauth_credentials`（V38）へ `SystemCrypto` で暗号化して保存する。鍵ローテーションの対象（`yuuka-crypto` の `ENCRYPTED_COLUMNS`）。サーバー削除で一緒に消える。
5. 以降の接続は `rmcp` の `AuthClient` が保存済みトークンを付け、期限切れや 401 のときは更新トークンで取り直す。「認可を解除」（`POST /api/mcp-servers/:id/oauth/disconnect`）で資格情報を消す。

認証の選び方は「固定トークンがあればそれ → OAuth の資格情報があればそれ → どちらも無ければ認証なし」。固定トークン登録済みのサーバーでは OAuth を始めない。

発見・登録・トークン取得の HTTP も、MCP 本体と同じ SSRF ガード付きクライアントで送る（`GuardedOAuthHttp`：内部アドレスに解決される名前は接続しない・IP リテラルを URL ごとに検査・リダイレクトは 1 段ずつ検査して最大 5 回）。

## MCP Apps

### サーバーとツール

- 初期化時に `capabilities.extensions["io.modelcontextprotocol/ui"] = { mimeTypes: ["text/html;profile=mcp-app"] }` を申告する（`rmcp_client.rs` の `client_config`）。
- `tools/list` の `_meta.ui`（`resourceUri`・`visibility`）を `tools_cache` に保存する（廃止予定の平坦なキー `_meta["ui/resourceUri"]` も読む）。
- `visibility` に `"model"` が無いツール（画面専用）は Gemini の Function Calling に出さない（仕様の MUST・`provider.rs`）。

### ホスト API（管理画面から呼ぶ）

| ルート | 役割 |
|---|---|
| `POST /api/mcp-servers/:id/apps/resource` `{tool}` | ツールの `ui://` リソースを `resources/read` し、MIME が `text/html;profile=mcp-app` であることを確かめて HTML とリソースの `_meta.ui`（csp・permissions）を返す（5MB まで） |
| `POST /api/mcp-servers/:id/apps/resource` `{uri}` | 画面からの `resources/read` の中継（`ReadResourceResult` をそのまま） |
| `POST /api/mcp-servers/:id/apps/call` `{name, arguments, from}` | `tools/call` の中継（`CallToolResult` をそのまま）。`from:"host"` は画面を持つツールだけ、`from:"app"` は `visibility` に `"app"` があるツールだけ受け付ける |

どれもサーバーを管理できるユーザー（本人登録、またはシステム登録かつ Admin）だけ。無効化中のサーバーは 409。

### 表示（`frontend/src/routes/mcp/McpAppModal.svelte`）

公式 SDK `@modelcontextprotocol/ext-apps` の `AppBridge` を使う（画面とのメッセージ形式を自前で書かない）。

1. 管理画面の「画面:」ボタン → `apps/resource` で HTML を受け取る。サーバーの「Tool実行前にユーザー確認を必須」が有効、または必須引数があるときは、引数（JSON）を確認してから進む。
2. サンドボックスプロキシ（下記）を iframe で開き、`AppBridge` を `PostMessageTransport` でつなぐ。
3. プロキシの `ui/notifications/sandbox-proxy-ready` → `sendSandboxResourceReady({html, csp, permissions})`。
4. 画面の `ui/notifications/initialized` → `ui/notifications/tool-input`（引数）→ `apps/call`（`from:"host"`）→ `ui/notifications/tool-result`。
5. 画面からの `tools/call` / `resources/read` は `apps/call`（`from:"app"`）/ `apps/resource`（`uri`）へ。`ui/open-link` は http(s) だけ新しいタブで開く。`ui/message` は管理画面に会話が無いので `isError` で断る。高さは `ui/notifications/size-changed` に合わせる（画面の 80% まで）。
6. 閉じるときは `ui/resource-teardown` を送り、最大 500ms 待ってから切る。

SDK（zod を含む）は大きいので、モーダルは初めて開くときに動的 import する。

### サンドボックスプロキシとオリジン（`crates/yuuka-mcp/src/sandbox.rs`・`sandbox.html`）

仕様では、Web ホストは画面を「本体と別オリジンのサンドボックスプロキシ」経由で表示する（プロキシは `allow-scripts allow-same-origin`）。プロキシは `GET /mcp-apps/sandbox`（認証なしの静的ページ）で、ホストと内側の iframe（`srcdoc`）の間でメッセージを中継し、`ui/notifications/sandbox-*` だけ自分で処理する。ホストからのメッセージは `event.origin` が本体のオリジンであるものだけ受ける。

| `MCP_SANDBOX_ORIGIN` | プロキシの URL | iframe の sandbox | 画面のオリジン |
|---|---|---|---|
| 設定あり（例 `https://mcp-sandbox.example.com`） | `{MCP_SANDBOX_ORIGIN}/mcp-apps/sandbox` | `allow-scripts allow-same-origin allow-forms` | サンドボックス用の実オリジン（仕様どおり） |
| 未設定 | `/mcp-apps/sandbox`（本体） | `allow-scripts allow-forms` | 不透明オリジン |

- 設定ありの場合: サンドボックス用ホスト名を同じ yuuka に向ける（Cloudflare Tunnel 等）。そのホスト名では `GET /mcp-apps/sandbox` 以外を 404 にする（`sandbox_host_guard`。サブドメインに Cookie が届く設定でも API に触れさせない）。本体 CSP の `frame-src` にそのオリジンを足す（`yuuka_web::apply_common_layers_with`）。`BASE_URL` と同じオリジンは設定エラー。
- 未設定の場合: 本体の Cookie・DOM・localStorage からは隔離される（セキュリティ上の問題はない）。ただし画面の中で localStorage・Cookie・IndexedDB は使えず、画面から外部への fetch は `Origin: null` になる。サーバーとのやり取りをホスト経由（`tools/call`・`resources/read`）だけで行う画面なら問題なく動く。仕様の MUST（別オリジン）からは外れる。
- プロキシ自身の CSP は、内側の画面が読み込めるものの上限（https のみ・`object-src 'none'`・`frame-ancestors` は本体のオリジン）。`srcdoc` の iframe は親の CSP を引き継ぐ。個々の画面には、サーバーが `_meta.ui.csp` で申告したドメインだけを許す CSP（申告が無ければ仕様の既定）を `<meta>` で先頭に差し込む。申告値はスキーム付きオリジンだけを通す（`;` 等で別のディレクティブを差し込ませない）。
- 共通レイヤの CSP は「ルートが自分で付けていなければ付ける」に変えた（プロキシページの CSP を上書きしないため）。

## サーバー側（ywrk-mcp 等）に必要なこと

旧方式の `/dashboard`・`/dashboard/enable` と、HTML 内の `MCP_PATH`・`tokenFromHash` の約束事は不要になった。画面を出すには、MCP Apps に対応させる:

- 画面を開くツール（例 `open_dashboard`）を用意し、`_meta.ui.resourceUri: "ui://<サーバー名>/dashboard"` を付ける。画面の中からだけ呼ぶ操作用のツールには `_meta.ui.visibility: ["app"]` を付ける。
- `resources/read` で `ui://...` に対して `mimeType: "text/html;profile=mcp-app"` の HTML を返す。外部の CSS・フォント・API を使うなら `_meta.ui.csp`（`resourceDomains`・`connectDomains`）で申告する。
- 画面の JS は公式 SDK の `App`（`@modelcontextprotocol/ext-apps`）でホストとつなぎ、データ取得は `app.callServerTool(...)` で行う（トークンを画面に渡す必要は無い）。
- 認証は OAuth 2.1 の保護リソースとして振る舞う（401 + `WWW-Authenticate: Bearer resource_metadata="..."`、`/.well-known/oauth-protected-resource`）か、固定トークンを使い続ける。

## 検証の範囲

ユニット・ルートのテスト（`cargo test -p yuuka-mcp` ほか）で、ツールの公開先の判定、`tools_cache` の UI メタの保存と読み戻し、OAuth 資格情報の保存・置換・連鎖削除、各ルートの権限と入力検査、サンドボックスページの CSP、設定値の検証を確かめている。実際の MCP サーバー・認可サーバー・ブラウザでの往復は未検証（実環境で確認する）。
