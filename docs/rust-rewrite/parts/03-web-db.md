# yuuka Rust 移行マスタープラン — 第6〜7部

> 対象: Web ランタイム / ルーティング / 認証 / 静的配信 / WebSocket（第6部）と DB 層・マイグレーション・データ分離（第7部）。
> 上位決定は [`00-decisions.md`](../00-decisions.md) に厳密整合。数値・型名は
> [`verification/rpt-axum-web-runtime.md`](../verification/rpt-axum-web-runtime.md),
> [`rpt-db-sqlx-vs-rusqlite.md`](../verification/rpt-db-sqlx-vs-rusqlite.md),
> [`rpt-migrations-sqlx-refinery.md`](../verification/rpt-migrations-sqlx-refinery.md),
> [`rpt-dual-sqlite-hazard.md`](../verification/rpt-dual-sqlite-hazard.md) の一次ソース照合に基づく。
> 現行不変条件は [`src/server.ts`](../../../src/server.ts), [`src/server/routeRegistry.ts`](../../../src/server/routeRegistry.ts),
> [`src/server/httpHelpers.ts`](../../../src/server/httpHelpers.ts), [`src/db/database.ts`](../../../src/db/database.ts),
> [`src/db/migrations.ts`](../../../src/db/migrations.ts) から抽出。
>
> **絶対制約の再掲（[`00-decisions.md`](../00-decisions.md) より）**: 厳格エラー（`thiserror` 具体列挙型のみ・`anyhow`/`eyre` 禁止）／常時稼働・自己復帰（致命的設定不備のみ fail-fast）／現行より高速・堅牢・マルチスレッド／カスタムモジュール拡張性／フロント⇄Rust 型の単一真実源。

---

## 6. Web ランタイム / ルーティング / 認証 / 静的配信 / WebSocket

### 6.0 クレート構成とバージョン運用ポリシー

| クレート | バージョン | feature（抜粋） | 役割 |
|---|---|---|---|
| `axum` | **0.8.9** | `ws`, `macros`, `http2`, `original-uri` | Web フレームワーク（hyper 1 の薄ラッパ・`tower::Service` 採用） |
| `hyper` | `^1.1`（axum 依存） | — | HTTP/1.1・HTTP/2 実装（**1.x 安定 API**・実務リスク低） |
| `tower` | `^0.5.2`（axum 依存） | `util` | ミドルウェア抽象（`Layer`/`Service`） |
| `tower-http` | **0.6.x 固定**（最新 0.6.11） | `fs`, `compression-br`, `compression-gzip`, `set-header`, `trace`, `cors`, `limit` | 静的配信・圧縮・ヘッダ・CORS |
| `tokio` | **1.52.x** | `rt-multi-thread`, `net`, `signal`, `macros` | 非同期ランタイム（`panic=unwind` 厳守） |
| `tokio-tungstenite`（間接） | axum `ws` に内包 | — | WebSocket（別クレート明示不要） |

**バージョン運用の絶対条件（[`rpt-axum-web-runtime.md`](../verification/rpt-axum-web-runtime.md) §1, §3）**:

- **axum は 0.x（1.0 未到達）**。semver 上マイナー更新（0.7→0.8）に破壊的変更が入る前提で運用する。実例: 0.8 でパスパラメータ構文が `/:id` → `/{id}` へ変更された。**「マイナー＝メジャー」とみなし**、`Cargo.toml` は `axum = "0.8"` で 0.8 系のパッチ追随のみ許可、0.9 は移行タスクとして明示レビューする。
- **tower-http は 0.6.x に固定**。axum 0.8.9 は `tower-http = ^0.6.8` を pin しており、`ServeDir` を `Router` に組み込む際に両者が公開する `http`/`tower` 型が同一メジャーである必要がある。tower-http 0.7.0（2026-06-15 リリース・非常に新しい）は圧縮の identity/ワイルドカード処理変更や暗黙 feature 削除など破壊的変更を含み、axum 0.8 対応版のリリースを確認するまで**採用しない**（過去に `tokio-rs/axum#2416` で同型バージョン不整合の非互換事例あり）。`deny.toml` に tower-http 0.7 系を `[[bans.deny]]` で暫定禁止しておくと事故を防げる。

**エラー写像の原則（[`00-decisions.md`](../00-decisions.md) エラー処理節と整合）**: HTTP レイヤの `WebError`（`thiserror` 列挙型）は**手書き `IntoResponse`** で写像する。`DbError`/`RepoError`/`AuthError` の内部 `Display` はクライアントへ漏らさず、`401`/`403`/`404`/`413`/`500` へ丸める（内部詳細は `tracing` にのみ出す）。`IntoResponse` の `match` は**同一クレート内で網羅**させ、バリアント追加漏れをコンパイルエラーで検知する（`#[non_exhaustive]` は付けない）。

---

### 6.1 現行自作ルータの不変条件 → axum への写像

現行は `registerRoutes`/`dispatchRoute`（[`routeRegistry.ts`](../../../src/server/routeRegistry.ts)）による自作ディスパッチで、以下の不変条件を持つ。**すべて axum で等価に保つ**。

| 現行の不変条件（出典） | 現行実装 | axum 写像 |
|---|---|---|
| 認可レベル `none`/`user`/`admin`（`RouteAuth`, contracts.ts:74） | `dispatchRoute` の逐次判定（401/403） | **`FromRequestParts` 実装型** `AuthenticatedUser`/`AdminUser`／任意は `Option<AuthenticatedUser>`（`OptionalFromRequestParts`） |
| `:param` パスパラメータ（`matchPath`, routeRegistry.ts:91） | 自作 split 照合 | axum `Path<T>` extractor（構文は `/{id}`） |
| CSRF（Origin/Referer/`Sec-Fetch-Site`、routeRegistry.ts:37-60） | POST/DELETE かつ `auth!="none"` で cross-site 拒否 | **tower レイヤ**（`CsrfLayer` 自作・下記 6.4） |
| 10MB ボディ上限（`MAX_BODY_BYTES`, routeRegistry.ts:111） | 手動カウント→413 | `DefaultBodyLimit::max(10 * 1024 * 1024)` |
| プロトタイプ汚染除去（`stripProtoKeys`, routeRegistry.ts:66） | `__proto__`/`constructor`/`prototype` 削除 | **serde が構造的に無効化**（`#[serde(deny_unknown_fields)]` + 型付き struct で未知キー拒否＝汚染面が存在しない） |
| HTTPS リダイレクト＋HSTS（server.ts:257-281） | 301＋`Strict-Transport-Security` | 起動時に proxy 終端前提を確認しつつ `SetResponseHeaderLayer` で HSTS 付与（リダイレクトは nginx 終端に委譲可） |
| CORS 限定反射（server.ts:284-310） | baseUrl 同一ホストのみ ACAO 反射 | `tower_http::cors::CorsLayer`（`AllowOrigin::predicate` で同一ホスト判定） |

**20以上のルートモジュールの再現（[`server.ts`](../../../src/server.ts):39-60）**: 現行は `authRoutes`〜`desktopClientRoutes` の 20 モジュールを `registerRoutes` で 1 レジストリに集約している。Rust では**モジュール別に `Router` を返す関数**を定義し、`Router::merge`（または `nest`）で合成する。ツリーが `merge` で平坦・型安全に構成でき、`AppState`（`Arc` 共有）を `with_state` で一括注入できる。

```rust
// crates/web/src/routes/mod.rs
pub fn app_router(state: AppState) -> Router {
    Router::new()
        .merge(auth::routes())          // 認証・登録（§5.4）
        .merge(settings::routes())      // ユーザー設定・Google OAuth
        .merge(bot::routes())           // Bot インスタンス・共有
        .merge(bot_attribute::routes())
        .merge(member_request::routes())
        .merge(todo::routes())          // ToDo（§3.2）
        .merge(schedule::routes())
        .merge(timeline::routes())
        .merge(finance::routes())       // 家計・予算・支払い予定
        .merge(playbook::routes())
        .merge(credential::routes())    // パスワードマネージャ（§6）
        .merge(admin::routes())
        .merge(reminder::routes())
        .merge(personal::routes())      // ノート・クリップボード・連絡先
        .merge(persona::routes())
        .merge(mcp::routes())
        .merge(integrated::routes())
        .merge(webhook::routes())       // 外部 Webhook 受信（auth: none）
        .merge(delivery::routes())      // 朝報・日報・週報
        .merge(device_auth::routes())   // desktop OAuth デバイスフロー
        .merge(device_mgmt::routes())
        .merge(desktop_client::routes())// Windows 版バイナリ配布
        .with_state(state)
        // ── 全体レイヤ（後入れ=外側）──
        .layer(DefaultBodyLimit::max(10 * 1024 * 1024)) // 10MB
        .layer(CsrfLayer::new(cfg.allowed_host.clone())) // Origin/Referer/Sec-Fetch-Site
        .layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP)))
        .layer(security_headers_layer())                 // X-Content-Type-Options 他 + HSTS
        .layer(cors_layer(&cfg))
        .layer(TraceLayer::new_for_http())
}
```

各モジュールは薄い `routes() -> Router<AppState>` を返す:

```rust
// crates/web/src/routes/todo.rs
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/todos",      get(list_todos).post(create_todo))
        .route("/api/todos/{id}", get(get_todo).delete(delete_todo))
}

// 認可は引数の型が強制する（AuthenticatedUser を取れば user 必須）
async fn list_todos(user: AuthenticatedUser, State(st): State<AppState>)
    -> Result<Json<ApiResponse<Vec<TodoDto>>>, WebError> { /* ... */ }
```

**`ApiResponse<T>` エンベロープ**（現行 `sendJson` の `{ success, message, ... }` 形）は Rust の単一 struct として型化し、ts-rs で TS へ生成する（[`00-decisions.md`](../00-decisions.md) フロント型連携節）。data ラッパの有無は現行踏襲。

---

### 6.2 認可の型強制（`FromRequestParts` extractor）

現行の `none`/`user`/`admin`（[`contracts.ts`](../../../src/types/contracts.ts):74）を、**型を引数に取ること自体が認可条件**になるパターンへ移す（[`rpt-axum-web-runtime.md`](../verification/rpt-axum-web-runtime.md) §5）。axum 0.8 は **RPITIT 化で `#[async_trait]` 不要**（旧 0.6/0.7 サンプルの流用は不可）。

```rust
// crates/web/src/auth/extract.rs
pub struct AuthenticatedUser(pub SessionUser); // user
pub struct AdminUser(pub SessionUser);         // admin

impl FromRequestParts<AppState> for AuthenticatedUser {
    type Rejection = WebError; // 401/403 を返す IntoResponse を実装済み
    async fn from_request_parts(parts: &mut Parts, st: &AppState)
        -> Result<Self, Self::Rejection>
    {
        // Cookie(__Host-yuuka-session) → Bearer(desktop) の順で解決（6.3）
        let user = resolve_request_user(parts, st).await
            .ok_or(WebError::Unauthorized)?;   // → 401
        Ok(Self(user))
    }
}

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = WebError;
    async fn from_request_parts(parts: &mut Parts, st: &AppState)
        -> Result<Self, Self::Rejection>
    {
        let AuthenticatedUser(user) = AuthenticatedUser::from_request_parts(parts, st).await?;
        if user.role != Role::Admin { return Err(WebError::Forbidden); } // → 403
        Ok(Self(user))
    }
}

// 任意認証（公開ルートでもセッションがあれば拾う。現行 dispatchRoute の else 分岐相当）
impl OptionalFromRequestParts<AppState> for AuthenticatedUser {
    type Rejection = Infallible;
    async fn from_request_parts(parts: &mut Parts, st: &AppState)
        -> Result<Option<Self>, Self::Rejection>
    {
        Ok(resolve_request_user(parts, st).await.map(AuthenticatedUser))
    }
}
```

- `AuthenticatedUser` を取るハンドラ＝`user` 必須、`AdminUser`＝`admin` 必須、`Option<AuthenticatedUser>`＝任意（現行 `auth:"none"` でセッションがあれば拾う挙動）。**認可レベルがハンドラ署名に可視化**され、付け忘れがコンパイル面で目立つ。
- `FromRequestParts`（ボディ非消費・順序自由）を使う。ボディ消費 extractor（`Json<T>` 等）は最後に 1 個だけ置く。
- `Rejection = WebError` にして 401/403 の `IntoResponse` を明示（設計しないと汎用 500 になる落とし穴。[`rpt-axum-web-runtime.md`](../verification/rpt-axum-web-runtime.md) §5）。

---

### 6.3 二経路認証（Cookie ＋ Bearer）の extractor 解決

現行 [`httpHelpers.ts`](../../../src/server/httpHelpers.ts) の `resolveRequestUser`（Cookie セッション → Bearer の順）を厳密移植する。

- **Cookie 経路**: `__Host-yuuka-session`（HTTPS 本番）／開発時のみ `yuuka-session` も受理（`getSessionToken` の分岐、httpHelpers.ts:50-57）。トークンは**不透明トークンを共有 Redis にハッシュ保存**する現行方式を維持（[`rpt-dual-sqlite-hazard.md`](../verification/rpt-dual-sqlite-hazard.md) §3.2 と整合＝署名鍵共有不要）。Redis クライアントは `AppState` に載せる。
- **Bearer 経路**: `Authorization: Bearer <token>`（desktop。`getBearerUser`, httpHelpers.ts:138）。`desktop_tokens` 表を sha256 照合（第7部の repo 経由）。**Bearer はアンビエント資格情報でない＝CSRF 非該当**（Origin チェックを課さない・6.4）。

```rust
async fn resolve_request_user(parts: &Parts, st: &AppState) -> Option<SessionUser> {
    if let Some(tok) = session_cookie(parts, &st.cfg) {            // __Host-yuuka-session
        if let Some(u) = st.sessions.lookup(&tok).await { return Some(u); } // Redis ハッシュ照合
    }
    if let Some(bearer) = bearer_token(parts) {                    // Authorization: Bearer
        return st.desktop_auth.verify(&bearer).await.ok();        // desktop_tokens sha256
    }
    None
}
```

`__Host-` prefix Cookie は Domain 不可＝**同一オリジン必須**。nginx 単一オリジン背後（新旧バックエンドの出し分け）と整合する（[`rpt-dual-sqlite-hazard.md`](../verification/rpt-dual-sqlite-hazard.md) §3.2）。Cookie 発行時の属性 `Path=/; HttpOnly; Secure; SameSite=Lax` は現行 `setSessionCookie`（httpHelpers.ts:89）を厳密踏襲。

---

### 6.4 CSRF ／ ボディ上限 ／ プロト汚染（tower レイヤ）

**CSRF（現行 `isCrossSiteStateChange`, routeRegistry.ts:37-60 の完全移植）**: `POST`/`DELETE` かつ認可必須ルートに対し、以下を多層で判定する自作 `CsrfLayer`。

1. `Sec-Fetch-Site: cross-site` を明示拒否。
2. `Origin`（`null` 以外）のホストが `config.baseUrl` と不一致なら拒否。
3. `Origin` 無ければ `Referer` のホストで同上判定。
4. 両方無い＝判定不能は `SameSite=Lax` に委ねて**許可**（現行と同一の緩和）。
5. `auth:"none"`（Webhook 受信等）は対象外＝クロスオリジンが正当。

```rust
// CsrfLayer::call の中核（tower::Service 実装）
fn is_cross_site_state_change(req: &Request, allowed_host: &str) -> bool {
    if req.headers().get("sec-fetch-site").is_some_and(|v| v == "cross-site") { return true; }
    if let Some(origin) = req.headers().get(ORIGIN).and_then(|v| v.to_str().ok()) {
        if origin != "null" { return host_of(origin).as_deref() != Some(allowed_host); }
    }
    if let Some(referer) = req.headers().get(REFERER).and_then(|v| v.to_str().ok()) {
        return host_of(referer).as_deref() != Some(allowed_host);
    }
    false // 判定不能 → SameSite=Lax に委ねる
}
```

メソッド・認可レベルの条件分岐はルート定義側のマーカー（拡張 `Extension` かパス prefix `/api/`）で判別する。レイヤ適用順序は tower の「後入れ＝外側」規約に注意し、CSRF は認可 extractor より外側に置く。

**ボディ上限**: `DefaultBodyLimit::max(10 * 1024 * 1024)`（現行 `MAX_BODY_BYTES` = 10MB・レシート画像 base64 考慮、routeRegistry.ts:111）。超過は axum が 413 を返す。

**プロト汚染**: 現行 `stripProtoKeys`（`__proto__`/`constructor`/`prototype` 除去）は**JS 固有のハザード**。Rust は serde で**型付き struct にデシリアライズ**するため、そもそもプロトタイプ連鎖が存在せず攻撃面が消滅する。DTO には `#[serde(deny_unknown_fields)]` を付け、未知キーを 422/400 で弾く（防御の明示化）。

---

### 6.5 静的配信（`ServeDir` プリコンプレス ＋ SPA フォールバック）

現行 [`server.ts`](../../../src/server.ts):115-236 の `serveStaticFile` を移植する。不変条件: パストラバーサル防御（`PUBLIC_DIR + sep` 前方一致、server.ts:124）、拡張子なしパスの SPA フォールバック（index.html）、Vite ハッシュ付きアセットの `immutable` キャッシュ／それ以外 `no-cache`（server.ts:154-162）、gzip 事前圧縮（COMPRESSIBLE_EXTS）。

```rust
// dist/public を配信。.br/.gz サイドカーは Accept-Encoding に応じて自動選択。
let serve_dir = ServeDir::new(&cfg.public_dir)
    .precompressed_br()      // dist/public/foo.js.br
    .precompressed_gzip()    // dist/public/foo.js.gz
    .append_index_html_on_directories(true)
    // 拡張子なし SPA ルートは index.html へフォールバック
    .fallback(ServeFile::new(cfg.public_dir.join("index.html")));

let app = app_router(state).fallback_service(serve_dir);
```

- **プリコンプレス**（[`rpt-axum-web-runtime.md`](../verification/rpt-axum-web-runtime.md) §3）: `.precompressed_br()`/`.precompressed_gzip()` は `.br`/`.gz` サイドカーを配信し、無ければ非圧縮へフォールバック。**サイドカーはビルド時に事前生成が前提**（`ServeDir` は動的圧縮しない）。Docker ビルド段（[`00-decisions.md`](../00-decisions.md) デプロイ節）で Vite 出力後に `brotli`/`gzip` を全 `COMPRESSIBLE` 資産へ生成する。
- **パストラバーサル**: `ServeDir` は内部で正規化しルート外アクセスを弾くが、現行の明示 403 と等価な防御を保つ（現行不変条件・[`00-decisions.md`](../00-decisions.md) セキュリティ不変条件）。
- **キャッシュ／CSP 差し込み**: Vite ハッシュ資産の `Cache-Control: public, max-age=31536000, immutable` と index.html 系の `no-cache` の出し分けは `ServeDir` 前段の薄いミドルウェア（パスで判定）で付与。CSP 等セキュリティヘッダは 6.6 の `SetResponseHeaderLayer` が全レスポンスに乗せる。
- **index.html への `google-site-verification` 差し込み**（server.ts:182）はビルド時に確定させるか、起動時に 1 度だけ読み込んでメモリキャッシュした `Html` を返す専用ハンドラで対応（都度 I/O を避ける）。

---

### 6.6 セキュリティヘッダ ／ 動的圧縮（役割別レイヤ）

現行 `CSP`/`SECURITY_HEADERS`（server.ts:82-89）を**そのままの値で**移植する（[`00-decisions.md`](../00-decisions.md) セキュリティ不変条件）。**静的ヘッダは `SetResponseHeaderLayer`、圧縮は `CompressionLayer`＝責務が別**（[`rpt-axum-web-runtime.md`](../verification/rpt-axum-web-runtime.md) §4）。

```rust
const CSP: &str = "default-src 'self'; script-src 'self' https://static.cloudflareinsights.com; \
style-src 'self' 'unsafe-inline' https://fonts.googleapis.com https://fonts.gstatic.com; \
font-src 'self' https://fonts.gstatic.com https://fonts.googleapis.com; \
img-src 'self' data: https://assets-global.website-files.com https://cdn.discordapp.com; \
connect-src 'self' https://cloudflareinsights.com; worker-src 'self'; frame-src 'self'; frame-ancestors 'self';";

fn security_headers_layer() -> impl Layer<...> {
    ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::overriding(header::CONTENT_SECURITY_POLICY, hv(CSP)))
        .layer(SetResponseHeaderLayer::overriding(HeaderName::from_static("x-content-type-options"), hv("nosniff")))
        .layer(SetResponseHeaderLayer::overriding(header::X_FRAME_OPTIONS, hv("SAMEORIGIN")))
        .layer(SetResponseHeaderLayer::overriding(header::REFERRER_POLICY, hv("strict-origin-when-cross-origin")))
        // HSTS: HTTPS 本番のみ（config で分岐）。max-age=63072000; includeSubDomains（server.ts:277）
        .layer(SetResponseHeaderLayer::if_not_present(header::STRICT_TRANSPORT_SECURITY, hv("max-age=63072000; includeSubDomains")))
}

// 動的圧縮は別レイヤ。ServeDir のプリコンプレス済みレスポンスは content-encoding を持つため二重圧縮されない。
let compression = CompressionLayer::new().br(true).gzip(true);
```

- **CSP から `unsafe-inline` を script-src で外している**現行の実効的 XSS 多層防御（server.ts:79-83）を厳守。インライン JS を足す場合は nonce/hash 方式へ（現行コメントの制約を継承）。
- `overriding()`（既存同名ヘッダを置換）を CSP に使う。HSTS は本番のみで `if_not_present()` 相当。
- MCP ダッシュボードの隔離 iframe（`sandbox="allow-scripts"`＝不透明オリジン）に返す専用ルート（現行 `mcpRoutes`）は**そのルートが独自 CSP を返す**構成を維持（本体 CSP は `frame-src 'self'` で同一オリジン dashboard を許可、server.ts:72-83）。

---

### 6.7 WebSocket（`/ws/chat`）

現行 [`server.ts`](../../../src/server.ts):385-407 の `upgrade` ハンドラ（Bearer 認証＋`?botId=` 所有/共有検証、`hasBotAccess`）を axum の内蔵 WS へ写像する（[`rpt-axum-web-runtime.md`](../verification/rpt-axum-web-runtime.md) §2）。**`ws` feature 必須**（未指定はコンパイルエラー）。別クレート不要。

```rust
// crates/web/src/routes/chat_ws.rs
pub fn routes() -> Router<AppState> {
    Router::new().route("/ws/chat", get(ws_upgrade))
}

async fn ws_upgrade(
    ws: WebSocketUpgrade,             // ボディ消費 extractor → 引数の最後
    Query(q): Query<ChatWsQuery>,     // ?botId=
    user: BearerUser,                 // ネイティブ Bearer 専用 extractor（Cookie 経路は使わない）
    State(st): State<AppState>,
) -> Result<Response, WebError> {
    // 接続時に 1 Bot へ束縛。未指定は system_default（server.ts:398）
    let bot_id = q.bot_id.unwrap_or_else(|| "system_default".into());
    if !st.bots.has_access(&user.0.discord_id, &bot_id).await { // hasBotAccess 相当
        return Err(WebError::Forbidden); // 403
    }
    Ok(ws.on_upgrade(move |socket| handle_chat(socket, user.0, bot_id, st)))
}
```

- **Bearer 認証**: WS upgrade は `getBearerUser` のみ（Cookie 経路を持たない・現行踏襲、server.ts:391）。`BearerUser` extractor は 401 を返す。ネイティブ Bearer は CSRF 非該当なので Origin チェックを課さない。
- **ping/pong・ターンキュー**: 現行 `chatWebSocket`（`chatWss`/`handleChatConnection`）のターン直列化（ユーザーごとの発話キュー）とアイドル切断防止の ping を `handle_chat` 内で維持。axum の `WebSocket` は `Stream + Sink`（`Message` enum）で、`futures` の分割により read/write 並行。
- nginx 側は `map $http_upgrade $connection_upgrade` ＋ `Upgrade`/`Connection` 転送 ＋ `proxy_read_timeout 3600s` ＋ **バックエンド ping**（[`rpt-dual-sqlite-hazard.md`](../verification/rpt-dual-sqlite-hazard.md) §3.3）。移行期は WS を新旧同時に割らず 1 upstream に固定。
- 停止時: 現行 `stopWebServer`（server.ts:421）の「全 WS 接続を閉じてから HTTP を停止」を、tokio-graceful-shutdown のサブシステム停止フックで再現（[`00-decisions.md`](../00-decisions.md) 自己復帰節と統合）。

---

### 6.8 config.yaml の起動時厳密検証

現行 `config`（`baseUrl`/`port`/`host`/`trustedProxies`/`sessionTtlDays`/`googleSiteVerification` 等を参照）を、**型付き構造体へ厳密デシリアライズ**する。`serde` + `figment`（または `config` crate）で `config.yaml` を読み、**必須欠落・型不一致は起動時に fail-fast**（[`00-decisions.md`](../00-decisions.md) 絶対制約2「致命的設定不備のみ fail-fast」）。

```rust
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    pub base_url: Option<Url>,             // https:// なら Secure/__Host- + HSTS を強制
    pub host: IpAddr,
    pub port: u16,
    pub db_path: PathBuf,
    pub session_ttl_days: u32,
    pub trusted_proxies: Vec<IpAddr>,      // XFF 信頼判定（getClientIp, httpHelpers.ts:65）
    pub google_site_verification: Option<String>,
    // ...機密は本 struct に平文で持たず secrecy::SecretString（[`00-decisions.md`](../00-decisions.md) 機密フェイルクローズ節）
}
```

- `base_url` が `https://` の場合のみ Cookie ハードニング（`__Host-` + Secure）と HSTS を有効化（現行 `isHttpsDeployment`, httpHelpers.ts:43 の判定を型で表現）。
- `ConfigError`（`thiserror`）で欠落・不正値を列挙し、起動時に `Err` を返して**プロセスを即終了**（ここだけは fail-fast が正しい）。

---

### 6.9 第6部で保つセキュリティ不変条件（チェックリスト）

[`00-decisions.md`](../00-decisions.md) の「移植で保つ不変条件」を、上記の写像先とともに再掲する。

| 不変条件 | 現行の担保 | Rust の担保 |
|---|---|---|
| CSP | server.ts:82（script-src から unsafe-inline 除外） | `SetResponseHeaderLayer::overriding`・値は同一文字列 |
| セキュリティヘッダ | X-Content-Type-Options/X-Frame-Options/Referrer-Policy/HSTS | `SetResponseHeaderLayer`（6.6） |
| CSRF | Origin/Referer/Sec-Fetch-Site（routeRegistry.ts:37） | `CsrfLayer`（6.4・4段判定を厳密移植） |
| パストラバーサル防御 | PUBLIC_DIR 前方一致 403（server.ts:124） | `ServeDir` の正規化＋ルート外拒否（6.5） |
| 機密非漏洩 | zod allowlist / エラーの丸め | 専用 DTO（機密をフィールドに持たない）＋ `WebError` の内部 Display 非露出（6.0） |
| CORS 限定反射 | baseUrl 同一ホストのみ ACAO（server.ts:284） | `CorsLayer` の predicate（6.1） |
| プロト汚染除去 | stripProtoKeys（routeRegistry.ts:66） | serde 型付き＋`deny_unknown_fields`（6.4） |

---

## 7. DB 層 / マイグレーション / データ分離

### 7.0 現行 DB 実装の不変条件

現行 [`database.ts`](../../../src/db/database.ts) は `better-sqlite3` の単一コネクションで `journal_mode=WAL` / `foreign_keys=ON` を設定（busy_timeout は better-sqlite3 既定 5000ms に暗黙依存、[`rpt-dual-sqlite-hazard.md`](../verification/rpt-dual-sqlite-hazard.md) §0 で実測訂正済み）。**SQLite が唯一の真実源で書き手は Node のみ**、`rust_synapse` は read-only（同 §0）。この「単一 writer」不変条件を Rust でも構造的に保証する。

---

### 7.1 rusqlite ＋ 単一 writer actor ＋ read pool

**採用（[`00-decisions.md`](../00-decisions.md) #9・[`rpt-db-sqlx-vs-rusqlite.md`](../verification/rpt-db-sqlx-vs-rusqlite.md) §2, §3）**:

| クレート | バージョン | 役割 |
|---|---|---|
| `rusqlite` | **0.40.1**（`bundled` feature） | SQLite C API 薄ラッパ。**bundled = SQLite 3.53.2 を静的リンク**（再現性・システム SQLite 非依存） |
| `libsqlite3-sys`（間接） | `^0.38.1` | bundled amalgamation |
| `deadpool-sqlite` | **0.13.0** | read 専用の非同期プール（`interact()` で内部 blocking 実行。`spawn_blocking` 手書き不要） |
| （代替）`r2d2_sqlite` | 0.34.0 | 同期プール（`spawn_blocking` と併用する場合） |
| `backon` | 1.6.0（[`00-decisions.md`](../00-decisions.md) #7） | `SQLITE_BUSY` リトライ（ジッタ必須。`backoff` は RUSTSEC-2025-0012 で禁止） |

rusqlite は**同期 API**。tokio 上で直呼びは禁止で、`spawn_blocking` か `deadpool-sqlite` の `interact()` でブロッキングプールへ逃がす（[`rpt-db-sqlx-vs-rusqlite.md`](../verification/rpt-db-sqlx-vs-rusqlite.md) §2）。

**アーキテクチャ: 単一 writer actor ＋ read pool**（SQLite の「多 reader・単一 writer」モデルに 1:1 対応。[`rpt-db-sqlx-vs-rusqlite.md`](../verification/rpt-db-sqlx-vs-rusqlite.md) §3 の推奨）。

```rust
// crates/db/src/writer.rs
// 全書き込みを 1 タスク・1 コネクションに直列化する actor。
pub struct WriterHandle { tx: mpsc::Sender<WriteJob> }

struct WriteJob {
    // クロージャで rusqlite::Transaction を受け取り任意の書き込みを実行。
    run: Box<dyn FnOnce(&mut rusqlite::Transaction) -> Result<(), DbError> + Send>,
    done: oneshot::Sender<Result<(), DbError>>,
}

impl WriterHandle {
    pub fn spawn(db_path: &Path) -> Result<Self, DbError> {
        let (tx, mut rx) = mpsc::channel::<WriteJob>(256);
        let conn = open_conn(db_path, /*read_only=*/false)?; // PRAGMA を明示設定（7.2）
        // 専用 blocking スレッドで受信ループ。panic は supervisor が JoinError で検知し再spawn。
        std::thread::Builder::new().name("db-writer".into()).spawn(move || {
            let mut conn = conn;
            while let Some(job) = rx.blocking_recv() {
                // 全書き込み Tx は BEGIN IMMEDIATE（7.2）。BUSY は backon で吸収。
                let res = with_immediate_retry(&mut conn, job.run);
                let _ = job.done.send(res);
            }
        })?;
        Ok(Self { tx })
    }

    pub async fn write<F, T>(&self, f: F) -> Result<T, DbError>
    where F: FnOnce(&mut rusqlite::Transaction) -> Result<T, DbError> + Send + 'static, T: Send + 'static
    { /* job を送り oneshot を await。writer が落ちていれば DbError::WriterGone */ }
}
```

```rust
// crates/db/src/reader.rs — 多コネクション read pool（WAL の reader はスケールする）
pub struct ReadPool(deadpool_sqlite::Pool);

impl ReadPool {
    pub fn open(db_path: &Path, size: usize) -> Result<Self, DbError> {
        let cfg = deadpool_sqlite::Config::new(db_path);
        let pool = cfg.builder(Runtime::Tokio1)?
            .max_size(size)
            .post_create(hook_apply_read_pragmas()) // 各接続に read 用 PRAGMA
            .build()?;
        Ok(Self(pool))
    }
    pub async fn read<F, T>(&self, f: F) -> Result<T, DbError>
    where F: FnOnce(&rusqlite::Connection) -> Result<T, DbError> + Send + 'static, T: Send + 'static
    {
        let conn = self.0.get().await?;
        conn.interact(move |c| f(c)).await.map_err(DbError::Interact)?
    }
}
```

- **書き込みは必ず `WriterHandle::write`**（型で単一直列化を強制＝複数 writer 競合が原理的に起きない）。読み取りは `ReadPool::read`。
- reader は**各クエリ後に statement を確実に finalize/reset**（長寿命 reader が checkpoint を阻害＝WAL 肥大。[`rpt-dual-sqlite-hazard.md`](../verification/rpt-dual-sqlite-hazard.md) §1.5）。rusqlite は `Statement` の drop で finalize されるため、prepared statement を跨いで保持しない設計にする。
- writer スレッドの panic は **`panic=unwind`＋JoinSet supervisor** が検知して再 spawn（[`00-decisions.md`](../00-decisions.md) 自己復帰節）。再 spawn 中の write 要求は `DbError::WriterGone` で明示エラー化し、backon 上位リトライへ委ねる。

---

### 7.2 PRAGMA の明示設定と BEGIN IMMEDIATE

現行は WAL/foreign_keys のみ明示で busy_timeout は既定依存だが、Rust は**全 PRAGMA を明示**する（rusqlite は既定を一切設定しない。[`rpt-db-sqlx-vs-rusqlite.md`](../verification/rpt-db-sqlx-vs-rusqlite.md) §2）。

```rust
fn open_conn(path: &Path, read_only: bool) -> Result<Connection, DbError> {
    let flags = if read_only {
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_URI
    };
    let conn = Connection::open_with_flags(path, flags)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;   // 現行踏襲
    conn.busy_timeout(Duration::from_millis(5000))?;    // Node 既定 5000 と一致（[`rpt-dual-sqlite-hazard.md`](../verification/rpt-dual-sqlite-hazard.md) §2.3）
    conn.pragma_update(None, "foreign_keys", "ON")?;    // 現行踏襲（database.ts:18）
    conn.pragma_update(None, "synchronous", "NORMAL")?; // WAL 下で十分・耐障害性ほぼ問題なし
    Ok(conn)
}
```

- **全書き込み Tx は `BEGIN IMMEDIATE`**（DEFERRED→write アップグレードの即-`SQLITE_BUSY` を回避。busy_timeout では救えないハザード＝[`rpt-dual-sqlite-hazard.md`](../verification/rpt-dual-sqlite-hazard.md) §1.4）:

```rust
fn with_immediate_retry<F, T>(conn: &mut Connection, f: F) -> Result<T, DbError>
where F: FnOnce(&mut Transaction) -> Result<T, DbError>
{
    // rusqlite: BEGIN IMMEDIATE を明示
    let backoff = ExponentialBuilder::default().with_jitter(); // backon・ジッタ必須
    (|| {
        let mut tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let out = f(&mut tx)?;
        tx.commit()?;
        Ok(out)
    })
    .retry(backoff)
    .when(|e: &DbError| e.is_sqlite_busy()) // SQLITE_BUSY のみリトライ
    .call()
}
```

- `busy_timeout=5000` を両側で揃える（synapse は現行 3000。運用上 5000 へ統一推奨。[`rpt-dual-sqlite-hazard.md`](../verification/rpt-dual-sqlite-hazard.md) §2.3）。
- 移行期の SQLite 二重アクセスは**「Node 全書き込み・Rust read-only、カットオーバー時に一度だけ writer 移譲」**を最優先（[`rpt-dual-sqlite-hazard.md`](../verification/rpt-dual-sqlite-hazard.md) §2.1）。両 writer は原則許容しない。

---

### 7.3 全テーブルの型付き repo とデータ分離（`UserId` newtype）

現行 [`migrations.ts`](../../../src/db/migrations.ts) の全 CREATE TABLE を、rusqlite の型付きリポジトリへ移す。**データ分離キー欠落をコンパイル時に防ぐため `UserId` newtype を全 repo 署名に通す**（[`00-decisions.md`](../00-decisions.md) #19・DB 節）。

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)] // ts-rs で TS 化
pub struct UserId(String);   // Discord ユーザーID（データ分離キー）
pub struct BotId(String);    // 既定 "system_default"

// 分離キーは引数の型が強制する（生の &str を受け取らない）
impl TodoRepo<'_> {
    pub fn list(&self, user: &UserId, bot: &BotId) -> Result<Vec<Todo>, RepoError> { /* WHERE user_id=?1 AND bot_id=?2 */ }
    pub fn create(&self, user: &UserId, bot: &BotId, input: NewTodo) -> Result<TodoId, RepoError> { /* ... */ }
}
```

**移植対象テーブル一覧（[`migrations.ts`](../../../src/db/migrations.ts) 現行 v17 スキーマ・全 46 表）**。分離キー欄は各行を型でスコープする際の必須引数を示す（`user_id` を持つ表は `UserId` を repo 署名に必須化）。

| 分類 | テーブル | 分離キー | 備考 |
|---|---|---|---|
| メタ | `system_settings` | — | schema_version 兼用（refinery 化で用途縮小・7.4） |
| ユーザー/認証 | `users` | PK=discord_id | 機密列（gemini/google 暗号化）は DTO に出さない |
| 招待 | `invite_codes` | — | |
| Bot | `bots` / `bot_shares` / `bot_user_modules` | user_id / owner_id / (bot_id,user_id) | |
| Bot 属性 | `bot_context_notes` / `bot_guild_notes` / `bot_guilds` / `bot_members` | (bot_id,user_id) / (bot_id,guild_id) 等 | `bot_context_notes`/`bot_user_modules` は user_id へ FK を張らない正式例外（汎用モードの未登録 Discord ユーザー） |
| ペルソナ | `personas` / `bot_active_personas` | owner_id / (user_id,bot_id) | |
| 会話履歴 | `message_logs` ＋ `message_logs_fts`（FTS5 trigram）＋トリガー ai/ad/au | user_id（**FK 無し**・未登録メンバー記録のため） | 7.5 参照 |
| 経験/記憶 | `tool_outcomes` / `topic_tool_stats` / `synapses` | user_id(+bot_id[+guild_id]) | synapse `embedding` BLOB は read-only 参照（[`00-decisions.md`](../00-decisions.md) Discord/synapse 節）。FK 無し |
| ToDo | `todos` ＋ `task_progress_logs` | user_id | todos は自己参照 parent_id |
| 予定 | `schedules` | user_id | |
| リマインド | `reminders` | user_id | |
| 家計 | `expenses` / `budget_limits` / `planned_payments` | user_id(+bot_id) | budget_limits PK に bot_id |
| Playbook | `playbooks` / `playbook_schedules` / `playbook_runs` | user_id(+bot_id) | |
| 個人 | `context_notes` / `clipboard_entries` / `contacts` | (user_id,bot_id) / user_id | |
| 認証情報 | `credentials` ＋ `bot_credential_access` | (user_id,service_name) / (bot_id,owner_id,service_name) | ユーザー鍵 Argon2id + AES-256-GCM（機密列は DTO 非出力） |
| Webhook | `webhook_endpoints` / `webhook_deliveries` | user_id | token UNIQUE |
| 配信 | `briefing_configs` / `report_configs` | (user_id,bot_id) | |
| MCP | `mcp_servers` ＋ `bot_mcp_access` | (user_id,bot_id) / (bot_id,owner_id,mcp_server_id) | `bot_mcp_links` は v4 で廃止・再作成しない。owner_id 次元は v7 のクロステナント修正 |
| Google | `user_google_accounts` / `bot_google_account` | user_id / bot_id | 暗号化リフレッシュトークンは DTO 非出力 |
| 監査 | `audit_logs` | user_id | 秘密値は記録禁止（現行不変条件） |
| デスクトップ | `desktop_tokens` | user_id | token_hash=sha256・UNIQUE |
| メンバー申請/ロール | `bot_member_requests` / `bot_roles` | (bot_id,guild_id,user_id) 等 | |
| タイムライン | `day_plan_blocks` / `timeline_records` | (user_id,bot_id) | v17 |

- **FK を張らない正式例外**（現行コメント準拠）: `message_logs` / `synapses` / `tool_outcomes` / `bot_context_notes` / `bot_user_modules` は user_id に users への FK を張らない（汎用モードで Web 未登録の Discord ユーザー ID も user_id に入るため）。repo でも FK 前提のロジックを持ち込まない。
- **機密フェイルクローズ**（[`00-decisions.md`](../00-decisions.md) #19）: `users`/`credentials`/`user_google_accounts`/`mcp_servers`/`bots` の暗号化列（`*_encrypted`/`*_iv`/`*_tag`）は**API DTO struct のフィールドに存在させない**→ ts-rs 生成 TS にも現れず、漏洩が型的に不可能。復号値のメモリ保持は `secrecy::SecretString`。

---

### 7.4 マイグレーション（refinery・前方専用・冪等 baseline）

**採用（[`00-decisions.md`](../00-decisions.md) #11・[`rpt-migrations-sqlx-refinery.md`](../verification/rpt-migrations-sqlx-refinery.md) §2, §3）**: `refinery 0.9.2`（`features = ["rusqlite"]`）。rusqlite ネイティブ・前方専用・`refinery_schema_history` で version+checksum 管理・`embed_migrations!` でバイナリ埋め込み。**rusqlite バージョン pin に注意**（refinery 0.9.2 が想定する rusqlite 系列に合わせる。[`rpt-migrations-sqlx-refinery.md`](../verification/rpt-migrations-sqlx-refinery.md) §2）。

**現行の破壊的パターンを撤廃する**。現行 [`migrations.ts`](../../../src/db/migrations.ts) は `SCHEMA_VERSION="17"`（:12）を `system_settings.schema_version` と比較し、**旧 v1 検出時に `PRAGMA foreign_keys=OFF` ＋ `LEGACY_TABLES` を `DROP TABLE`（:880-894）＝データ喪失**する分岐を持つ。これを**完全撤廃**する（[`00-decisions.md`](../00-decisions.md) DB 節・[`rpt-migrations-sqlx-refinery.md`](../verification/rpt-migrations-sqlx-refinery.md) §3）。

**baseline (V1) の凍結**: 現行 v17 の最終スキーマ（7.3 の全 46 表・全インデックス・FTS5・トリガー）を **`CREATE TABLE IF NOT EXISTS` / `CREATE INDEX IF NOT EXISTS` の冪等 DDL** として `migrations/V1__baseline.sql` に固める（[`rpt-migrations-sqlx-refinery.md`](../verification/rpt-migrations-sqlx-refinery.md) §3「idempotent baseline」）。

- **既存 DB**（実データあり・migration ledger 無し）: V1 は全て既存＝無害に通り、`refinery_schema_history` に記録される。**DROP は一切走らない**。
- **新規 DB**: V1 が実際に全スキーマを生成する。
- 現行の逐次移行関数（v3〜v17: `migrateToBotScopedData` 等の `ADD COLUMN`/`IF NOT EXISTS` ガード済みステップ）は、**最終形が baseline に畳み込まれている**ため個別再現不要。ただし `message_logs` の FK 撤廃再構築（:1109）や `bot_mcp_access` owner_id 追加（v7・:452）のような「既存 DB を旧定義から作り替える」ステップは、**baseline は最終定義のみを持つ**（新規 DB は最初から正しい）。**旧定義が残る本番 DB に対しては、V1 適用前に一度きりの明示的・レビュー済み移行**（V2 以降ではなく、baseline 導入と同時のデータ移行スクリプト）で吸収する。version 不一致の自動 DROP は決して使わない。

```rust
// crates/db/src/migrate.rs
mod embedded { refinery::embed_migrations!("./migrations"); } // V1__baseline.sql, V2__*.sql ...

pub fn run_migrations(conn: &mut rusqlite::Connection) -> Result<(), MigrateError> {
    // 前方専用・冪等。適用済みは skip、適用済みファイルの改変は checksum 不一致で Err。
    let report = embedded::migrations::runner()
        .set_migration_table_name("refinery_schema_history")
        .run(conn)?;
    tracing::info!(applied = report.applied_migrations().len(), "migrations done");
    Ok(())
}
```

- **前方専用**（refinery は down を持たない。取り消しは新しい V を書く。[`rpt-migrations-sqlx-refinery.md`](../verification/rpt-migrations-sqlx-refinery.md) §2）。以後の変更は `V2__…`, `V3__…` の追記のみ。**適用済み migration は二度と編集しない**（checksum 検証で Err になる＝改竄検知）。
- **FK OFF が必要なテーブル再構築**（create-copy-drop-rename）を V2 以降で行う場合、SQLite の `PRAGMA foreign_keys=OFF` はトランザクション内で効かないため `PRAGMA defer_foreign_keys=ON`（Tx 内可）を使う（[`rpt-migrations-sqlx-refinery.md`](../verification/rpt-migrations-sqlx-refinery.md) §1）。
- **migration 所有権は単一プロセスに一元化**（[`00-decisions.md`](../00-decisions.md) DB 節）。移行期は「どちらが migration を実行するか」を Rust か Node の一方に固定し、両者が同時に走らせない（[`rpt-dual-sqlite-hazard.md`](../verification/rpt-dual-sqlite-hazard.md) §4.3・ロールバック窓ではスキーマ凍結）。migration は**単一 writer 経路**で実行する（writer actor 起動前の起動シーケンスで 1 度）。
- `system_settings.schema_version` 行は refinery 導入後は**マイグレーション判定に使わない**（version 判定は `refinery_schema_history` が担う）。`system_settings` 表自体は他用途（`v5_grants_backfilled` 等の marker）で残置。

---

### 7.5 FTS5 全文検索とトリガーの移植

`message_logs_fts`（FTS5・`tokenize='trigram'`・外部コンテンツ表 `content='message_logs'`, content_rowid='id'）とトリガー `message_logs_ai`/`ad`/`au`（[`migrations.ts`](../../../src/db/migrations.ts):1143-1160）を baseline V1 にそのまま含める。

- rusqlite の `bundled` SQLite **3.53.2 は FTS5 を内蔵**（`bundled` は FTS5 有効でビルドされる）。追加 feature 不要だが、CI で `SELECT * FROM pragma_compile_options` に `ENABLE_FTS5` があることを起動時に検証しておくと安全。
- トリガー（INSERT/DELETE/UPDATE で FTS を同期）は DDL としてそのまま維持。`message_logs` は writer actor 経由でのみ書くため、FTS 同期の一貫性は単一 writer 直列化で自然に保たれる。
- 全文検索クエリ（`MATCH`）は read pool 側で実行。

---

### 7.6 代替: sqlx 0.9（コンパイル時クエリ検査が欲しい場合）

**採用はしない**が、[`00-decisions.md`](../00-decisions.md) の未解決事項2（DB 選定）に対する代替として記録する。`sqlx 0.9.0`（`sqlx-sqlite 0.9.0`）は `query!`/`query_as!` の**コンパイル時クエリ検査**が魅力で、`.sqlx` オフラインモード（`sqlx-cli prepare`）で CI から実 DB 依存を外せる（[`rpt-db-sqlx-vs-rusqlite.md`](../verification/rpt-db-sqlx-vs-rusqlite.md) §1）。マイグレーションも `sqlx::migrate!()` ＋ `_sqlx_migrations` で同等の前方専用・checksum を得られ、`Migrator::skip` で baseline を「適用済み」マークできる（[`rpt-migrations-sqlx-refinery.md`](../verification/rpt-migrations-sqlx-refinery.md) §1, §3）。

**却下理由（1 段落）**: (1) **SQLite 書き込み footgun** — 既定の複数コネクション `SqlitePool` は WAL 書き込みで `busy_timeout` 競合・ロック飢餓を招き（実測 ~20x 劣化）、結局アプリ層で「read pool ＋ `max_connections(1)` write pool」の単一 writer 規律を手当てする必要があり、rusqlite の writer actor と同じ設計を別の抽象で再実装することになる（[`rpt-db-sqlx-vs-rusqlite.md`](../verification/rpt-db-sqlx-vs-rusqlite.md) §3）。(2) **synapse との二重化** — 既存 `rust_synapse` が rusqlite を使っており（[`rpt-dual-sqlite-hazard.md`](../verification/rpt-dual-sqlite-hazard.md) §0）、sqlx を足すと同一プロセス内に 2 系統の SQLite バインディング・依存が並立する。加えて SQLite のヌル可能性推論は Postgres より脆く（`LEFT JOIN` で `UnexpectedNull`）`as "col!"`/`col?` の冗長な override が要る。以上より rusqlite に一本化する。

---

### 7.7 第7部で保つ不変条件（チェックリスト）

| 不変条件 | 現行の担保 | Rust の担保 |
|---|---|---|
| WAL / FK ON | database.ts:17-18 | `open_conn` PRAGMA 明示（7.2） |
| busy_timeout 5000 | better-sqlite3 既定（暗黙） | `conn.busy_timeout(5000)` 明示（7.2） |
| 単一 writer | Node のみ書き込み | `WriterHandle` actor で型強制（7.1） |
| データ分離（user_id 必須） | repo が user_id をスコープ | `UserId` newtype を repo 署名に必須化（7.3） |
| 機密非漏洩 | zod allowlist | 専用 DTO（機密列をフィールドに持たない）＋ secrecy（7.3） |
| データ喪失の撤廃 | — （現行は DROP 分岐が残存） | refinery 前方専用・冪等 baseline・DROP 分岐撤廃（7.4） |
| FTS5 整合 | トリガー同期 | 単一 writer で同期一貫性・bundled FTS5（7.5） |

---

### 付録: 第6〜7部で確定した数値・型名（load-bearing）

- axum **0.8.9**（hyper `^1.1`, tower `^0.5.2`, tower-http **0.6.x 固定**・0.7.0 不採用）／`ws` feature／`FromRequestParts`・`OptionalFromRequestParts`／`ServeDir::precompressed_br()`/`precompressed_gzip()`／`SetResponseHeaderLayer::overriding`/`if_not_present`／`CompressionLayer`／`DefaultBodyLimit::max(10MB)`／`CorsLayer`。
- rusqlite **0.40.1**（bundled = SQLite **3.53.2**）／`deadpool-sqlite` **0.13.0**（代替 `r2d2_sqlite` 0.34.0）／PRAGMA: `journal_mode=WAL`・`busy_timeout=5000`・`foreign_keys=ON`・`synchronous=NORMAL`／`TransactionBehavior::Immediate`（BEGIN IMMEDIATE）／`backon` 1.6.0 で `SQLITE_BUSY` リトライ。
- refinery **0.9.2**（`rusqlite` feature・前方専用・`refinery_schema_history`・`embed_migrations!`）／baseline **V1** は `CREATE ... IF NOT EXISTS` 冪等／現行 SCHEMA_VERSION 不一致 DROP（migrations.ts:880-894）を**撤廃**。
- 現行全 **46 テーブル**（`bot_mcp_links` は v4 廃止で対象外）を型付き repo へ。`UserId` newtype でデータ分離を型保証。
