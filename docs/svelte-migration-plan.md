# Svelte + 現Node.jsサーバー 移行手順書

> **⚠️ 履歴資料（Svelte 移行は完了済み・Node バックエンドは撤去済み）**
> 本書は、Node.js バックエンド（`src/server.ts`・`tsx`・`tsgo`・`node dist/index.js`・`STATIC_DIR` 環境変数・`src/public` の旧バニラ SPA 等）が存在した時期に書かれた移行手順書です。これらは現行リポジトリには存在せず、本文中のコマンド・ファイルパス・ポート・Dockerfile の行番号は**現行の手順ではありません**。
> 現行の構成は次を参照してください: 管理画面 SPA = `frontend/`（`pnpm dev` / `pnpm build:front`、出力は `dist/public`）、バックエンド = `crates/`（Rust。管理画面は `/admin` 配下に静的配信）、セットアップ = [guide/setup.md](guide/setup.md)、デプロイ = [guide/deployment.md](guide/deployment.md)、全体像 = [project_overview.md](project_overview.md)。

対象: `リポジトリルート`（Discord AI秘書Bot + Web管理ダッシュボード）
方針: フロントエンドを素の vanilla JS SPA から **Svelte(単体) + Vite** へ移行する。バックエンド（`src/server.ts` の素 http サーバー + route registry + API）は原則不変。

---

## 1. 概要 / ゴール / 非ゴール

### ゴール
- `src/public/app.js`（10,010行の単一 IIFE クロージャ）を、TypeScript + Svelte コンポーネント + ストア + 型付き API クライアントへ分解する。
- ビルド成果物を **現行の `serveStaticFile`（`src/server.ts`）が配信できる静的ファイル群**（`index.html` + ハッシュ付き `/assets/*`）として出力する。
- 厳格 CSP（`script-src 'self' https://static.cloudflareinsights.com`、inline script 禁止）を **本番でそのまま維持**する。
- 既存 URL 体系（`/bot/<tab>`、`/device?code=`、`/tasks/guide` 等）とデスクトップ連携・外部リンクを一切壊さない。
- **既存ユーザーの localStorage（旧 Bot 選択 4キー・テーマ）を無破壊で引き継ぐ**（§9.2 のマイグレーション必須）。

### 非ゴール（スコープ外）
- **バックエンドの変更**: `server.ts` の `serverHandler` / route registry / `src/server/routes/*` / API 契約は変えない。変更するのは `serveStaticFile` の Cache-Control 分岐・`PUBLIC_DIR` の切替のみ（後述、差分は最小）。
- **SSR 化**: SvelteKit の SSR やアダプタは導入しない（現サーバーの素 http + SPA フォールバックと不整合）。
- **CSP の緩和**: 本番 CSP は現状維持。dev のみ Vite 前段プロキシで回避する（`server.ts:76-77` の CSP 定数は触らない）。
- **`style-src 'unsafe-inline'` の除去**: index.html の `style=` は当面維持。CSP 強化は別タスク。
- **DB スキーマ / 認証方式 / セッション**: 一切変更しない。

---

## 2. 技術選定

### 2.1 フレームワーク: Svelte 単体 + Vite（SvelteKit を使わない）

| 選択肢 | 判定 | 理由 |
|---|---|---|
| **素の Svelte + Vite (SPA)** | ✅ 採用 | 現サーバー（素 http + 拡張子なし→`index.html` フォールバック + Cookie 認証）を無改修に近い形で維持できる。単一 `index.html` エントリ + ハッシュ付き `/assets/*` を出すだけ。 |
| SvelteKit (adapter-static SPA) | ❌ 不採用 | SPA モード(`fallback: index.html`)は可能だが、`load`/`+layout`/ルーティング規約が SSR 前提の思想で書かれ、現サーバー構成と概念がズレる。過剰。 |
| SvelteKit (SSR) | ❌ 不採用 | Node アダプタが必要になり「現サーバー維持」制約に真っ向から反する。 |

**SvelteKit を使わない理由の核心**: サーバーは `serveStaticFile` が `PUBLIC_DIR` 直下の `index.html` を全ての拡張子なしパスに返す純粋 SPA フォールバック方式（`server.ts:129-134`）。この配信モデルには「単一 HTML + JS バンドル」を出す素の Vite が最も適合し、SvelteKit のサーバー機能は全て不要。

### 2.2 SPA ルーター
現 `applyRoute()` / `navigateTo()` / `switchTab()` は History API 直書き。移行先は **軽量 History ルーター**。

- **第一候補: 薄い自前 History ルーター**（現 `navigateTo`→`applyRoute` の1対1構造をそのまま Svelte ストア + `{#if}`/`<svelte:component>` で再現）。11ルート分類 + 15 Bot タブと少数のため、外部依存を増やさず `$page`（`writable<URL>`）+ `popstate` リスナ1個で足りる。
- **代替: `svelte-spa-router` / `tinro`（History モード）**。ルート定義をテーブル化したい場合。

いずれでも **認可・プリセット別タブフィルタ・Bot 未選択リダイレクトはルーターに密結合させず**、認証ストア購読のルートガードとして分離する（§8, §9）。

### 2.3 TypeScript 化
- フロント全体を `.ts` / `.svelte`（`<script lang="ts">`）で書く。バックエンドの `src/types/contracts.ts` / zod スキーマと型を突き合わせる。
- 型チェックは `svelte-check`（新規 script）。既存 `tsgo` はバックエンド専用のまま。
- Biome の `lint`/`format` は `src/`（バックエンド）専用（`package.json:18-20`）。`.svelte` は Biome 非対応のため対象外のまま。必要なら `eslint-plugin-svelte` を別途併用（任意）。

---

## 3. 最終アーキテクチャ図（dev / prod）

### 本番 (prod)
```
[ビルド時]
  frontend/ (Svelte + Vite ソース)
        │  vite build (outDir = ../dist/public)
        ▼
  dist/public/
        ├── index.html            ← <!-- GOOGLE_SITE_VERIFICATION --> コメント残置
        ├── theme-init.js         ← publicDir から素通しコピー（ハッシュなし・type無し同期script）
        ├── manifest.json, sw.js, icons/, materials/, 404.html
        └── assets/
              ├── index-<hash>.js   (type=module, 'self' 由来)
              └── index-<hash>.css

[実行時]  ブラウザ ──HTTP──▶ node dist/index.js  (WORKDIR=/app, port 7854)
                                   │
                     serverHandler ├─ /api/*     → routeRegistry.dispatchRoute
                                   ├─ /ws/chat    → WebSocket upgrade
                                   └─ それ以外    → serveStaticFile(PUBLIC_DIR = dist/public)
                                                     ├ 拡張子なし → index.html (SPA fallback)
                                                     ├ /assets/**（ハッシュ付） → Cache-Control: immutable ★新規
                                                     └ その他（sw.js/index.html/manifest等） → no-cache（現状維持）
                        CSP: script-src 'self' ...  ← Vite の hashed module は 'self' 由来で適合
```

### 開発 (dev)
```
  ブラウザ ──▶ Vite dev server (port 5173, same-origin)
                 │  HMR/inline script/eval/WebSocket は 5173 内で完結（CSP なし）
                 │
                 ├─ Svelte ソース → HMR で即時反映
                 └─ server.proxy で転送（真に必要なのは2つだけ）:
                       /api      ─▶ 127.0.0.1:7855 (dev-hot標準) / 7854 (host tsx watch)
                       /ws/chat  ─▶ ws://同上 (ws:true)

  ※ CSP は API 応答にのみ乗る。HTML/JS は Vite が返すため厳格 CSP と衝突しない。
  ※ proxy は same-origin(5173) 扱い + changeOrigin:false のため Cookie/CSRF(Origin/Referer) 契約を破らない。
  ※ dev API サーバーは BASE_URL 未設定(=http) が前提（§5.5）。https だと __Host- Cookie しか受理されず proxy 越しにセッションが刺さらない。
```

**要点**: prod は Vite が inline を出さず全 JS/CSS を外部ハッシュファイル化するため現 CSP で通る。dev は Vite の HMR が inline/eval を使うため 7854 の CSP とは両立不能 → Vite を前段に立て `/api`・`/ws/chat` のみプロキシして回避する。

---

## 4. ディレクトリ構成（共存を含む）

移行中は **旧 `src/public`（vanilla）と新 `frontend`（Svelte）を段階共存**させる。Vite プロジェクトは `apps/yuuka/frontend/` に新設する。

```
apps/yuuka/
├── src/                      ← バックエンド（不変）
│   ├── server.ts             ← serveStaticFile / PUBLIC_DIR のみ最小変更
│   ├── server/routes/*.ts
│   └── public/               ← 【移行中・P5まで凍結】旧 vanilla フルセット（ロールバック源）
│       └── index.html app.js styles.css theme-init.js sw.js manifest.json
│          icons/ materials/ 404.html vendor/chart.umd.min.js
├── frontend/                 ← 【新規】Vite + Svelte プロジェクト
│   ├── index.html            ← Vite 入力 HTML（<head> に GSV コメント / Google Fonts link / 同期 theme-init.js）
│   ├── vite.config.ts        ← outDir=../dist/public, publicDir=public, server.proxy
│   ├── tsconfig.json  svelte.config.js
│   ├── public/               ← 静的コピー資産（theme-init.js/manifest.json/icons/materials/404.html。sw.jsはPWAが生成）
│   │                            ★ src/public からは move ではなく COPY（§14 ロールバック整合）
│   └── src/
│       ├── main.ts           ← mount(App, {target}) + styles.css import
│       ├── App.svelte        ← ルーター + 認証ゲート + オーバーレイ束ね
│       ├── styles.css        ← 旧 styles.css を丸ごと持ち込み（global import）
│       ├── lib/
│       │   ├── api/          ← 型付き API クライアント（§10）
│       │   ├── router.ts     ← applyRoute 相当の薄いルーター
│       │   └── stores/       ← session / activeBot / theme / 各タブ（§9）
│       ├── routes/           ← 1 tab-view = 1 ルートコンポーネント
│       └── components/ui/    ← 横断共通部品（Icon, Modal, ... §11）
└── dist/
    ├── (tsgo 出力 .js)        ← バックエンド（dist/ 直下）
    ├── assets/               ← src/assets コピー（@napi-rs/canvas 等サーバー資産・Viteと無関係）
    └── public/               ← 【新規】vite build 出力先（PUBLIC_DIR の向き先）
```

**衝突回避（重要リスク）**: backend の tsgo は `dist/` 直下に `.js` を出す（tsconfig `outDir=./dist`, `rootDir=./src`）。Vite の `outDir` を `dist/` 直下にすると混在・相互破壊するため、必ず **`dist/public/`** に分離する。`emptyOutDir: true` は **`outDir`（=`dist/public`）配下のみ**をクリアするので `dist/` 直下の tsgo 出力（`dist/index.js` 等）や `dist/assets/` は消えない。**読者が `outDir` を誤って `dist/` 直下に向けると tsgo 出力を全消しするため厳禁**。ローカル build は必ず **tsgo（`dist/` 直下）→ vite build（`dist/public/`）** の順で別サブツリーに出す（§7）。

---

## 5. 開発フロー

### 5.1 依存追加（lockfile 更新が Docker 前提条件）
```
pnpm add -D vite @sveltejs/vite-plugin-svelte svelte svelte-check @tsconfig/svelte concurrently vite-plugin-pwa
# chart.js は既に dependencies にあり（package.json:42）。vendor UMD を廃し npm import へ
```
**重要**: Dockerfile の `pnpm install` は `--frozen-lockfile`。上記追加後に **`pnpm-lock.yaml` を更新・コミットしてからでないと Docker ビルドが lockfile 不一致で失敗する**（§7 前提条件）。`pnpm-workspace.yaml` に esbuild の `allowBuilds`（overrides `esbuild@<0.28.1`, `package.json:72`）が既存のため Vite の下地は整っている。

### 5.2 `vite.config.ts`（proxy が肝・target は env 化）
```ts
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// dev-hot(compose)標準フロー = 7855 / ホスト直 tsx watch(dev:host) = 7854
// prod の HOST_PORT は環境毎に異なる(dev=7855, prod=7701 等)ため、必ず env で切替可能に
const API = process.env.VITE_API_TARGET ?? "http://127.0.0.1:7855";

export default defineConfig({
  root: __dirname,
  base: "/",                    // ★デフォルト維持（/theme-init.js 等の絶対パス参照が書き換わらないよう）
  publicDir: "public",
  build: {
    outDir: "../dist/public",
    emptyOutDir: true,          // dist/public 配下のみクリア（dist/ 直下の tsgo 出力は無事）
    assetsDir: "assets",        // ★固定: ハッシュ資産を assets/ 直下に集約（chunk 含む）
    assetsInlineLimit: 0,       // CSP script-src 'self' 準拠: inline module/data-URI を出さない
  },
  plugins: [svelte()],
  server: {
    port: 5173,
    // 真に必要なのは /api と /ws/chat のみ（§5.6 参照）
    proxy: {
      "/api":     { target: API, changeOrigin: false },
      "/ws/chat": { target: API.replace("http", "ws"), ws: true, changeOrigin: false },
    },
  },
});
```
- **`ws: true` を `/ws/chat` に必須**（忘れるとチャット WebSocket が dev で動かない）。
- **`changeOrigin: false`** で same-origin を維持し、`Set-Cookie` のドメイン不一致・CSRF(Origin/Referer)403 を防ぐ。`routeRegistry` の `isCrossSiteStateChange` は Origin/Referer のホスト名を検証し、dev で baseUrl 未設定なら `isAllowedHost` が localhost/127.0.0.1 を許可する（`routeRegistry.ts:23-28`）。
- proxy 経由で `/api` は 5173 と同一オリジンに見えるため Cookie(`SameSite=Lax`) がそのまま通る。

### 5.3 併走スクリプト（`package.json` scripts へ追加）
```jsonc
"dev:front":     "vite --config frontend/vite.config.ts",
"dev:all":       "concurrently -k \"pnpm dev:host\" \"pnpm dev:front\"",
"build:front":   "vite build --config frontend/vite.config.ts",
"typecheck:front":"svelte-check --tsconfig frontend/tsconfig.json"
```
`dev:host`（`tsx watch src/index.ts`, `package.json:15`）は変更せず据え置き。API のホットリロードは従来通り。`dev:host` 単独起動時は cwd がプロジェクトルートで、既定 `PUBLIC_DIR=dist/public`（§6.1）が空だと `/` が 404 になる → **`dev:host` 単独で旧配信を見たいときは `STATIC_DIR=src/public` を明示**（§6.1 注記）。

### 5.4 `build` スクリプトへの統合（実態に即した追記位置）
`package.json:16` の実際の中身は cargo×2 → `mkdir/cp` → `tsgo` の順で、**`tsgo` が行末**にある。フロントを行末の `tsgo` の**直後**に `&&` で追記する:
```jsonc
// before (package.json:16):
"build": "... && cp -r src/assets/. dist/assets/ && tsgo",
// after:
"build": "... && cp -r src/assets/. dist/assets/ && tsgo && pnpm exec vite build --config frontend/vite.config.ts",
```
- この `build` は **cargo ビルドを含む重量スクリプト**で Docker の tsgo 独立 RUN とは別物。フロントだけ回したいときは `pnpm build:front` を使う。
- 挿入位置は必ず行末（tsgo の後）。中間に挿すと cargo/tsgo の順序が崩れる。

### 5.5 dev API サーバーの BASE_URL 前提（セッションが刺さる条件）
dev（非HTTPS）ではセッション Cookie 名が `yuuka-session`（`__Host-yuuka-session` は Secure 必須なので付かない、`httpHelpers.ts:53-57,98`）。`getSessionToken` は dev で両名を受理する。**だが dev API サーバーの `BASE_URL` が https に設定されていると `__Host-`（Secure）Cookie しか発行・受理されず、Vite の http proxy(5173) 越しには Cookie が刺さらず「ログインは成功するがセッションが維持されない」サイレント失敗になる。** → dev-hot / dev:host の API は **`BASE_URL` 未設定（=http）** で起動すること。

### 5.6 Vite proxy に `/hook`・`/proxy` を載せない理由
SPA(app.js) は `/hook` と `/proxy` を一切 fetch しない。
- `/hook/:token` は**外部サービスからの Webhook 受信**ルート（`webhookRoutes.ts`）。ブラウザ SPA の経路ではない。
- `/proxy/mcp/:id/mcp` は **sandboxed iframe（`sandbox="allow-scripts"`＝不透明オリジン）内の MCP ダッシュボード**からのみ呼ばれる（`mcpRoutes.ts:513`）。SPA 本体ではなく iframe 内ドキュメント経由。
→ **proxy に載せると「dev で MCP が動く」誤った安心材料**になる。proxy は `/api` と `/ws/chat` のみ。MCP iframe（`/api/mcp-servers/:id/dashboard` が独自 CSP で返す）が Vite 前段でも表示・動作するかは、**P4(BotMcp) 着手前に単独検証項目として実施**する（§8 備考・§11.3）。

### 5.7 docker-compose.dev-hot への組み込み（案A 推奨）
現 `docker-compose.dev-hot.yml` は `./src:/app/src` をマウントし「フロントは `src/public` をブラウザリロードで即反映（ビルド不要）」が前提（L4-6, L30-32）。**Svelte 化でこの前提は崩れる**。

- **案A（推奨・最小変更）**: フロントは**ホストで `pnpm dev:front`（Vite 5173）**を動かし、Vite proxy で `/api`・`/ws/chat` を dev コンテナ公開ポート `${HOST_PORT}`（dev=7855）へ転送。dev-hot コンテナは **API のホットリロードに専念**（`tsx watch` は無改修）。`src/public` マウントは P5 まで残してよい（旧資産の即反映・ロールバック用）。
- **案B**: compose に vite サービスを1つ追加。案A より重い。

**必ず更新するドキュメント**（同期漏れが事故源）:
- `docker-compose.dev-hot.yml` の L4-6 / L30-32 コメント（「フロントは vite dev 別プロセス」へ）
- `deploy/instance.sh` の hot 説明

---

## 6. ビルド統合とサーバー側の最小変更

### 6.1 `PUBLIC_DIR` の切替（環境変数で dev/prod 両対応）
**before**（`src/server.ts:64`）:
```ts
const PUBLIC_DIR = path.resolve(process.cwd(), "src", "public");
```
**after**:
```ts
// 既定は Vite 出力先。移行途中・ロールバックは STATIC_DIR=src/public で旧資産に戻せる。
// runtime は WORKDIR /app で node dist/index.js 起動のため cwd=/app、dist/public を解決可能。
const PUBLIC_DIR = process.env.STATIC_DIR
  ? path.resolve(process.cwd(), process.env.STATIC_DIR)
  : path.resolve(process.cwd(), "dist", "public");
```
`serveStaticFile` 内の `PUBLIC_DIR` 参照（パストラバーサル防御 `:116-125`、SPA フォールバック `:133`、404 `:135`）はそのまま機能する。
- **副作用の明示**: 既定が `dist/public` に変わると、**`dev:host` 単独起動（Vite なし）で `dist/public` が未ビルド＝空だと `/` が 404**。移行期間中の dev:host 単独フローは `STATIC_DIR=src/public` を明示するか、prod だけ `STATIC_DIR=dist/public` を compose/instance.env で注入し既定は `src/public` のままにする運用を推奨。

### 6.2 Cache-Control の immutable 分岐（ハッシュ付きアセット・gzip 二重圧縮の回避込み）
現状は全ファイルに `no-cache, no-store, must-revalidate` 固定（`:150, :178`）。Vite の `/assets/*.<hash>.js|css` は内容ハッシュで一意 → 長期不変キャッシュが可能。`sw.js`/`index.html`/`manifest.json`/`404.html`/`theme-init.js` は **no-cache 維持が必須**（特に `sw.js` は更新伝播のため）。

**判定はディレクトリ名の完全一致ではなく「assets/ 配下（入れ子含む）かつファイル名にハッシュを含む」で行う**（脆い `path.dirname === join(PUBLIC_DIR,"assets")` は不採用。Vite が `assets/fonts/…` 等の入れ子や chunk を出すと no-cache に落ち、逆に public/ 配下に `assets` 名ディレクトリを置くと誤 immutable 化するため）。`serveStaticFile` 内、`responseHeaders` を組む直前（`:148` 付近）にヘルパを挟む:
```ts
const assetsRoot = path.join(PUBLIC_DIR, "assets");
// assets/ 配下（入れ子可）かつ Vite のハッシュ命名（-<8+桁ハッシュ>.ext）に一致
const isHashedAsset =
  finalPath.startsWith(assetsRoot + path.sep) &&
  /-[A-Za-z0-9_-]{8,}\.(js|css|woff2?|png|jpe?g|svg|webp)$/.test(finalPath);
const cacheControl = isHashedAsset
  ? "public, max-age=31536000, immutable"
  : "no-cache, no-store, must-revalidate";
```
`responseHeaders`（`:150`）と `index.html` gzip 失敗フォールバック（`:178`）の両方の `Cache-Control` をこの `cacheControl` に差し替える。`index.html` は `assets/` 配下でないため自動的に no-cache のまま。`theme-init.js`/`manifest.json`/`sw.js`/`404.html` が確実に no-cache 側に落ちることを**テストで固定**する。

**gzip 二重圧縮リスクの明示（現状 `serveStaticFile` の実装を精査した結論）**:
- 現サーバーは stream を `zlib.createGzip()` へパイプするため **`Content-Length` を付けない**（`:208-217` 相当。ETag/Last-Modified も無い）。ハッシュ immutable 資産では Content-Length 欠落が CDN/Cloudflare 前段のキャッシュ効率・Range・条件付きリクエストを劣化させ得る。
- **`vite-plugin-pwa`/Vite に `.js.gz`/`.css.gz`（precompress）を生成させない**こと（generateSW 既定は gzip を生成しない）。生成すると現 `serveStaticFile` はそれを扱わず素の `.js` を都度再圧縮し、プリ圧縮成果物が無駄になる。
- 対応方針は手順書として二択を明記: **(A) precompress を出さず現行の都度 gzip のまま**（`COMPRESSIBLE_EXTS` に `.js`/`.css` があるので immutable 資産にも gzip + `Vary: Accept-Encoding` が効く。追加改修不要）。**(B) precompress を採用するなら** `serveStaticFile` に「`.gz` 存在時に差し替え + `Content-Length` 付与」ロジックを足す。**今回は (A) を既定**とし、Content-Length 欠落による前段キャッシュ劣化は許容（必要なら小アセットのみ Buffer 一括 gzip + Content-Length へ切替を将来検討）。

### 6.3 CSP は本番不変（Vite hashed module が 'self' で通る）
- Vite prod は `type=module` の hashed ES module + `<link rel=modulepreload>` を出力し、全て `'self'` 由来の外部参照。inline script は生成されない（`assetsInlineLimit:0` で data-URI/inline も抑止）→ **`server.ts:76-77` の CSP 定数は無改修**。
- `theme-init.js` は現状も外部 `<script src>` なので CSP 準拠のまま（§12）。
- chart.js を npm import 化すれば `/vendor/chart.umd.min.js` 参照が消え CSP はさらに素直になる。
- **注意（`server.ts:71-72` コメント）**: CSP はメモリ上の定数。万一 CSP を触ったら dist 再ビルド + node プロセス再起動が必須。今回の移行では CSP を触らないのが前提。

### 6.4 GOOGLE_SITE_VERIFICATION 実行時置換の維持
`serveStaticFile:157-171`（`ext === ".html" && basename === "index.html"`）は `index.html` 配信時のみ `<!-- GOOGLE_SITE_VERIFICATION -->` を `config.googleSiteVerification` で実行時置換する（未設定なら空置換）。**Vite 入力 HTML（`frontend/index.html`）の `<head>` に同じコメントを1個だけ残す**。Vite はコメントを既定で保持するので出力 `index.html` にも残り、`basename === "index.html"` 判定が効いて無改修で機能する。
- **リスク**: `build.minify`（既定 esbuild）が HTML コメントを削る可能性。**build 後に検証を固定**: `grep -c 'GOOGLE_SITE_VERIFICATION' dist/public/index.html` が **1** を返すこと。0 なら `build.minify: false`（HTML のみ）または `transformIndexHtml` フックでコメント保護。

---

## 7. 本番 Docker 統合

### 7.1 前提条件（先に満たす）
- §5.1 の依存を `devDependencies` に追加し、**`pnpm-lock.yaml` を更新・コミット済み**であること（Dockerfile は `pnpm install --frozen-lockfile`。未更新だと install で失敗）。

### 7.2 node-builder（stage 2）
`frontend/` は `vite/svelte` に依存するため、**依存 install 後・COPY src の並び**で `COPY frontend ./frontend` を追加する（`:69` 付近）。ビルド順序は **`pnpm install` → `tsgo` → `vite build` → `pnpm prune --prod`** を厳守（prune 後だと vite/svelte が消えて失敗）。

**before**（`Dockerfile:71-73`）:
```dockerfile
RUN pnpm exec tsgo \
 && mkdir -p dist/bin dist/assets \
 && cp -r src/assets/. dist/assets/
```
**after**:
```dockerfile
RUN pnpm exec tsgo \
 && pnpm exec vite build --config frontend/vite.config.ts \
 && mkdir -p dist/bin dist/assets \
 && cp -r src/assets/. dist/assets/
```

### 7.3 runtime（stage 3）の COPY（二重 assets 構造に注意）
runtime は複数 COPY を持つ。**P5 で削除するのは `COPY src/public` の1行のみ**で、隣接する `COPY src/assets`（@napi-rs/canvas 等サーバー資産）を巻き添えで消すと canvas 描画等が壊れる。

- `COPY --from=node-builder /app/dist ./dist` は既に `dist/` 全体（`dist/public` + `dist/assets`）を拾う。
- **`COPY --from=node-builder /app/src/public ./src/public`** … 移行途中（〜P4）は**残す**（`STATIC_DIR=src/public` ロールバック用）。**P5 で削除**し `PUBLIC_DIR=dist/public` に一本化。
- **`COPY --from=node-builder /app/src/assets ./src/assets`** … **常に残す**（Vite と無関係のサーバー実行時資産）。
- `dist/assets`（`:73` の `src/assets` コピー）と runtime の `src/assets` COPY が別物で両方必要かは実装前に確認。**Vite の `/assets/` とは名前が近いだけの別物**。

### 7.4 ローカル build
§5.4 の通り `package.json:16` 行末の tsgo 直後に `&& pnpm exec vite build --config frontend/vite.config.ts` を追記済み。ローカル `pnpm build` でフロントも出る（ただし cargo ビルド込みの重量スクリプト。フロント単体は `pnpm build:front`）。

---

## 8. ルーティング移行対応表

現 `applyRoute()`（`app.js:368-583`）と `switchTab()`（`app.js:302-359`）の全パスを網羅。`BOT_TABS`=15個。既存 URL は全て互換維持（デスクトップが `/device?code=`、外部リンクが `/tasks/guide` 等を叩くため破壊不可）。

| 現行パス | 表示（現） | Svelte ルート | ガード | 備考 |
|---|---|---|---|---|
| `/`, `/bots`, `/index.html` | bot-selection-overlay + `fetchBotList()` | `BotSelection.svelte` | 要ログイン | エイリアス3つ維持 |
| `/login` | login-overlay | `Login.svelte` | none | login/register タブ切替はローカル state |
| `/bot`, `/bot/<tab>` | app-container + `switchTab(tab)` | `BotShell.svelte` → 子 `routes/Bot*.svelte` | 要ログイン + Bot選択済み | 既定 `config`、未知タブ→`config` |
| `/integrated` | integrated-overlay + `fetchIntegratedOverview()` | `IntegratedOverlay.svelte` | 要ログイン | |
| `/admin` | admin-overlay + `fetchAdminData()` | `AdminOverlay.svelte` | 要 `role==='admin'`（非adminは`/`へ） | |
| `/account` | account-overlay + `fetchAccountSettings()` | `AccountOverlay.svelte` | 要ログイン | |
| `/device` | device-overlay + `showDeviceApprovalView()` | `DeviceOverlay.svelte` | ログイン往復対応 | `?code=` を `$page.url.searchParams` で保持。device-auth は botId 無し（§10.1） |
| `/usage` | usage-overlay | `Usage.svelte` | **公開**（`PUBLIC_PATHS`） | セッション前描画 |
| `/terms` | terms-overlay | `Terms.svelte` | **公開** | |
| `/privacy` | privacy-overlay | `Privacy.svelte` | **公開** | |
| `/tasks/guide` | task-guide-overlay | `TasksGuide.svelte` | **公開** | 外部リンク先 |
| 未知パス | ログイン済→`/` / 未ログイン→`/login` | フォールバック | — | |

**`/bot/<tab>` の15タブ** → `routes/` 配下:
`dashboard→BotDashboard`, `tasks→BotTasks`, `timeline→BotTimeline`, `schedules→BotSchedules`, `expenses→BotExpenses`, `reminders→BotReminders`, `personal→BotPersonal`, `personas→BotPersonas`, `delivery→BotDelivery`, `webhooks→BotWebhooks`, `mcp→BotMcp`, `playbooks→BotPlaybooks`, `discord→BotDiscord`, `config→BotConfig`, `devices→BotDevices`。

移行実装:
- `cleanPath` 正規化（`?`/`#`/末尾スラッシュ除去）はそのまま流用。
- `popstate` 手動処理は不要化（ルーターが1個の `popstate` で担う）。`/device` の `?code=` のみ `$page.url.searchParams` から読む。
- **`PUBLIC_PATHS`（`/usage`,`/terms`,`/privacy`,`/tasks/guide`）は「認証を待たず描画できる公開ルート」として明示分離**し、`/api/me` 前でも描画。**この分離は §10.2 の 401 ハンドラと連動必須**（公開ルート上では 401 で `/login` へバウンスしない）。
- プリセット別タブフィルタ（`SECRETARY_ONLY_TABS`/`ASSISTANT_ONLY_TABS`→config フォールバック）は `derived(activeBot)` なメニュー配列で表現。
- **MCP タブ**: `/proxy/mcp` は SPA 経路でないため、dev の Vite 前段で iframe が動くかを P4 前に単独検証（§5.6・§11.3）。

---

## 9. 状態管理（Svelte stores 設計）

現状: クロージャローカル let（`activeTab`, `activeUserId`, `activeUserRole` 等）+ `window.currentBotId` + localStorage 4キー に分散。これを4系統のストアに集約する。

### 9.1 `stores/session.ts`
```ts
import { writable, derived } from "svelte/store";
export type SessionUser = { discordId: string; username: string; role: "user"|"admin" } | null;
export const currentUser = writable<SessionUser>(null); // /api/me の単一の真実
export const isAdmin  = derived(currentUser, (u) => u?.role === "admin");
export const isAuthed = derived(currentUser, (u) => u !== null);
```
- `activeUserId`/`activeUserRole` → `currentUser` に統合。認証ゲート・admin ガードはこの derived を購読。
- `initAppSession()`（`app.js:1347`）相当は `bootstrapSession()`: 起動時に `/api/me`（auth:'user'、未ログインで 401、`authRoutes.ts:464`）を叩き `currentUser` を埋める。**この起動時 401 は「匿名」として `currentUser=null` にするだけで、`/login` へ遷移しない**（§10.2）。login/logout/register/プロフィール更新後に再取得。

### 9.2 `stores/activeBot.ts`（localStorage 同期 + 旧4キー one-time マイグレーション）
既存ユーザーの localStorage には旧4キー（`currentBotId`/`Name`/`Avatar`/`Preset` 相当）が入っている。**新キー `currentBot`（単一 JSON）が無ければ旧4キーから復元し新キーへ書き戻す**。これを怠ると移行直後の全既存ユーザーが `activeBot=null` に落ち「Bot 未選択」へ戻される。
```ts
import { writable } from "svelte/store";
type Bot = { id: string; name: string; avatar: string; preset: string } | null;
const KEY = "currentBot";

function load(): Bot {
  if (typeof window === "undefined") return null;
  const raw = localStorage.getItem(KEY);
  if (raw) { try { return JSON.parse(raw) as Bot; } catch { /* fallthrough */ } }
  // one-time マイグレーション: 旧4キー → 新オブジェクト
  const id = localStorage.getItem("currentBotId");
  if (id) {
    const migrated: Bot = {
      id,
      name:   localStorage.getItem("currentBotName")   ?? "",
      avatar: localStorage.getItem("currentBotAvatar") ?? "",
      preset: localStorage.getItem("currentBotPreset") ?? "",
    };
    localStorage.setItem(KEY, JSON.stringify(migrated)); // 書き戻し
    return migrated;
  }
  return null;
}

export const activeBot = writable<Bot>(load());
activeBot.subscribe((b) => {
  if (typeof window === "undefined") return;
  if (b) {
    localStorage.setItem(KEY, JSON.stringify(b));
    // 旧タブ/旧sw.jsと共存する移行期間中は旧4キーも書き続けて後方互換を保つ
    localStorage.setItem("currentBotId", b.id);
    localStorage.setItem("currentBotName", b.name);
    localStorage.setItem("currentBotAvatar", b.avatar);
    localStorage.setItem("currentBotPreset", b.preset);
  } else {
    localStorage.removeItem(KEY);
  }
});
```
- `window.currentBotId` のグローバル共有 → `activeBot` の import に置換。`selectBot()` の localStorage 書き込みは `activeBot.set(...)` に集約。

### 9.3 `stores/theme.ts`（localStorage + 副作用同期）
`app.js` の `applyTheme()` の全副作用を再現（§12）:
```ts
export const theme = writable<"dark"|"light"|"blue-archive">(readInitial());
theme.subscribe((t) => {
  if (typeof document === "undefined") return;
  document.documentElement.setAttribute("data-theme", t);
  localStorage.setItem("yuuka-theme", t);
  document.querySelector('meta[name=theme-color]')?.setAttribute("content", colorOf(t));
});
```

### 9.4 各タブデータ
- `activeTab` は**廃止**し、URL(`$page`) を真実の源にする。
- ダッシュボード集計（`pendingTasksCount`/`totalExpensesVal` 等）は該当コンポーネントのローカル state か `stores/dashboard.ts` へ。
- 1タブ複数 fetch（personal は3本、personas は4本、config は芋づる4本）は `loadDataForActiveTab` の巨大 if-else を廃止し、各コンポーネントの `onMount`/`$effect` に分散。芋づる依存（config→credentials/shares/attribute）は各カード子コンポーネントが自身で取得して解消。

---

## 10. 型付き API クライアント層

### 10.1 契約の要点（守るべき不変契約・実挙動に即して修正）
- 認証は **Cookie セッションが主**（`__Host-yuuka-session`/`yuuka-session`、HttpOnly）。Bearer はデスクトップ専用でブラウザ SPA 不要。
- fetch の `credentials` は **`'same-origin'` を明示付与**する。ただし現 `app.js` は `credentials` を一切指定しておらず（0箇所）fetch 既定の `same-origin` に依存している。明示は正しい改善だが「現状の再現」ではなく明確化である点に注意。dev は Vite proxy（`changeOrigin:false`）で same-origin を維持し Cookie を送る。
- **botId のトランスポート契約（正確な記述）**: サーバは全スコープルートで `ctx.body.botId ?? ctx.url.searchParams.get("botId")` として **body・query の両方を method 非依存で読む（body 優先）**（`todoRoutes.ts`/`financeRoutes.ts`/`scheduleRoutes.ts`/`deliveryRoutes.ts`/`botAttributeRoutes.ts` で確認）。GET は body を持てないので query 必須。**現 `app.js`(41-57) は POST/DELETE で query と body の両方に botId を注入**している。→ 新クライアントは「GET=query、POST/DELETE=body に注入」で機能上安全だが、これは*契約*ではなく検証済みの意図的簡略化である（POST の query botId が無視されるわけではない。body が優先されるだけ）。
- **botId 未指定/無権限は `system_default` にサイレントフォールバック**（`todoRoutes.ts:36-37`, `financeRoutes.ts:51-52`, `deliveryRoutes.ts:34-36`, `botAttributeRoutes.ts:134-138`）。これがエラーにならないため「別 Bot のデータに見える」不具合の温床。
- **botId を付けない除外は「プレフィックス一致」**（現 `app.js:34-38` は `resource.startsWith(...)`）:
  - `/api/bots`（**配下すべて**: `/api/bots/sync-discord`, `/api/bots/profile`, `/api/bots/presets`, `/api/bots/usage`, `/api/bots/shares` 等）。これらは自前の body/query で botId を明示的に運ぶ（`botRoutes.ts:216,254,323,368`）ため、自動注入で上書きしてはならない。**`/api/bots/usage` は呼び出し側が手動で botId を付与**（`app.js:1806`）。
  - `/api/login`, `/api/register`, `/api/logout`, `/api/me`。
  - **device-auth `/api/auth/device/*` と device-management の3エンドポイント**（`app.js:3576-3577`: 「デバイス系3エンドポイントは botId を注入しない…stale な botId を混ぜない」）。ユーザースコープなので **botId を絶対に付けない**。
  - `/api/setup/status`, `/api/setup`。
- レスポンスは大半 `{ success: boolean, message?, ...payload }`（**`data` ラッパは無い**。ペイロードは `bots`/`tasks`/`user` 等トップレベル直置き）。**HTTP 200 でも `success:false` があり得る** → 成否は HTTP ステータス **と** `data.success` の複合判定。`message` は日本語ユーザー向け文言でトースト表示に流用可。
- **共通エンベロープの例外**（個別扱い）:
  - `/api/setup/status`: `{ needSetup, ... }`（`success` を持たない、`authRoutes.ts:106-109`）。
  - デバイストークン `/api/auth/device/token`: OAuth 形状。**200 + `{error:"authorization_pending"|"slow_down"}` はポーリング継続の正常状態**、400/410 は失効等（`deviceAuthRoutes.ts:77-93,82-84`）。汎用 `request()` の `!res.ok`→throw / `success` 判定に**通してはならない**（pending/slow_down を誤って握り潰す）。専用メソッドで OAuth `error` を検査する（§10.4）。
- CSRF: POST/DELETE で Origin/Referer/Sec-Fetch-Site 検証。same-origin 必須（dev は Vite proxy で same-origin 維持、§5.2）。

### 10.2 `lib/api/client.ts`（botId は「型で強制する opt-out 相当」・multipart 対応・401 例外）
現状の **native fetch グローバルモンキーパッチ（`app.js:25-65`）は廃止**し明示 `apiClient` に置換する。ここが移行の最重要ポイント。**現行は opt-out（除外リスト以外は全注入）**であり、ドラフトの opt-in（`bot:true` を渡した時だけ注入）は**注入漏れで全 bot-scoped API が silent に `system_default` に落ちる**危険がある。→ **型システムで bot スコープ要否を強制**し、bot-scoped サービスメソッドで botId 省略が**コンパイルエラー**になるようにする（実行時任意フラグにしない）。

```ts
import { get } from "svelte/store";
import { activeBot } from "$lib/stores/activeBot";
import { currentUser } from "$lib/stores/session";
import { goto, isPublicPath, currentRoute } from "$lib/router";

// scope:'bot' は botId 注入必須、'user' は絶対に注入しない（device/auth/settings/admin/bots系）
type Scope = "bot" | "user";
type Opts = {
  scope: Scope;                    // ★必須。省略不可（型で強制）
  body?: unknown;                  // object | FormData
  query?: Record<string, string>;
  isBootstrap?: boolean;           // 起動時 /api/me プローブ等: 401 でも遷移しない
};

class ApiError extends Error { constructor(public status: number, msg: string){ super(msg); } }

async function request<T>(method: string, path: string, opts: Opts): Promise<T & { success?: boolean; message?: string }> {
  const url = new URL(path, location.origin);
  const botId = opts.scope === "bot" ? get(activeBot)?.id : undefined;
  const isForm = opts.body instanceof FormData;

  if (opts.query) for (const [k, v] of Object.entries(opts.query)) url.searchParams.set(k, v);

  // botId 注入: GET/FormData はクエリ、それ以外(JSON body)は body（サーバは body 優先で両対応）
  let finalBody: BodyInit | undefined;
  let headers: Record<string, string> = {};
  if (isForm) {
    if (botId) url.searchParams.set("botId", botId);   // multipart は body を JSON 化しない → query へ
    finalBody = opts.body as FormData;                 // Content-Type はブラウザに委ねる（境界付与）
  } else if (method === "GET") {
    if (botId) url.searchParams.set("botId", botId);
    finalBody = undefined;
  } else {
    const base = (opts.body as object) ?? (botId ? {} : undefined);
    const merged = base && botId ? { ...base, botId } : base;
    if (merged !== undefined) { finalBody = JSON.stringify(merged); headers["Content-Type"] = "application/json"; }
  }

  const res = await fetch(url.toString(), { method, credentials: "same-origin", headers, body: finalBody });

  // 集中型 401: ただし bootstrap プローブ・公開ルート上では遷移しない（匿名として扱う）
  if (res.status === 401) {
    currentUser.set(null);
    if (!opts.isBootstrap && !isPublicPath(get(currentRoute))) goto("/login");
    throw new ApiError(401, "認証が必要です");
  }

  const data = await res.json().catch(() => ({}));
  if (!res.ok || data.success === false) throw new ApiError(res.status, data.message ?? "エラーが発生しました");
  return data;
}

export const api = {
  get:  <T>(p: string, o: Opts) => request<T>("GET", p, o),
  post: <T>(p: string, body?: unknown, o?: Omit<Opts,"body">) => request<T>("POST", p, { ...(o as Opts), body }),
  del:  <T>(p: string, o: Opts) => request<T>("DELETE", p, o),
};
export { ApiError };
```

**注意点（単体テストで固定必須）**:
- **除外は「プレフィックス一致」で厳密再現**（§10.1）。サービス層で `/api/bots/*`・`/api/login|register|logout|me`・`/api/auth/device/*`・device-management 3本・`/api/setup*` は必ず `scope:'user'`。これらに botId が混入 / データ API に botId 欠落 → サイレント不具合。
- **`/api/bots/usage` は `scope:'user'`** とし、呼び出し側が明示 botId を（`query` で）渡す（現 `app.js:1806` の手動注入を再現）。
- **multipart/FormData**（ペルソナ画像等のアップロード。現 `app.js:48` の `!options.body.startsWith("-----")` ガード相当）: `body instanceof FormData` なら **JSON 化せず Content-Type をブラウザに委ね、botId は query へ回す**。`app.js` の全 fetch を grep して FormData 送信箇所を洗い出し、対象エンドポイントを列挙してテストに「FormData 時 botId は body でなく query・非 JSON 化」を追加。
- **401 集中割り込みは公開ルート/bootstrap を除外**（§9.1・§8）。除外しないと `/usage`,`/terms`,`/privacy`,`/tasks/guide` 上の匿名ユーザーが `/api/me` プローブの 401 で `/login` へ弾かれる。

### 10.3 領域別サービス分割
`authApi`, `botApi`, `taskApi`, `financeApi`, `scheduleApi`, `timelineApi`, `reminderApi`, `settingsApi`, `personaApi`, `playbookApi`, `integratedApi`, `mcpApi`, `credentialApi`, `webhookApi`, `adminApi`, `deviceApi` に分ける。**bot-scoped 群（tasks/schedules/timeline/expenses/reminders/playbooks 等）は `scope:'bot'`、非スコープ群（auth/bots/settings/admin/device）は `scope:'user'` を型レベルで区別**。

**レスポンス型**: 共通エンベロープ `ApiResponse<T> = { success: boolean; message?: string } & T` を基本に、ペイロードキーはエンドポイント毎に個別 interface（`data` ラッパ不在に注意）。Bot 応答は zod `botViewSchema` で機密列（`_encrypted`/`_iv`/`_tag`）除去済み → **露出フィールド（`has_token`/`has_gemini_key`/`running`/`connected`/`shared`）のみ型に含める**。setup 系・デバイストークンは共通エンベロープから除外して個別型（§10.4）。

### 10.4 デバイストークンのポーリング（エンベロープ外の専用メソッド）
`deviceApi.pollToken` は汎用 `request()` を**通さない**。`/api/auth/device/token` の応答を OAuth 形状で直接扱う:
```ts
// 200 + {error:"authorization_pending"|"slow_down"} → ポーリング継続
// 200 + {access_token,...} → 承認完了
// 400/410 + {error} → 失効/不正。error コードを surface して停止
async function pollToken(deviceCode: string): Promise<PollResult> {
  const res = await fetch("/api/auth/device/token", {
    method: "POST", credentials: "same-origin",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ device_code: deviceCode }), // botId は付けない（user-scoped）
  });
  const data = await res.json().catch(() => ({}));
  if (res.status === 200 && data.access_token) return { status: "authorized", token: data.access_token };
  if (res.status === 200 && (data.error === "authorization_pending" || data.error === "slow_down"))
    return { status: "pending", slowDown: data.error === "slow_down" };
  return { status: "error", code: data.error ?? `http_${res.status}` }; // 400/410 等
}
```

---

## 11. コンポーネント分解計画

### 11.1 3層構造の固定
`1 tab-view section → 1 ルートコンポーネント` / `カード・行 → 子コンポーネント` / `モーダル → ストア駆動の共通 Modal でラップ`。

### 11.2 共通 UI 部品（最初に作る）— `components/ui/`
現状 `createElement` で数百箇所に重複する部品を抽出。**投資対効果が最大なのは Icon / EmptyState / StatusChip**。

| 部品 | 置換対象 |
|---|---|
| `Icon.svelte`（Material Symbols, `name` prop） | `<span class="material-symbols-outlined">` の手組み数百箇所 |
| `Modal.svelte`（bindable `open`, backdrop/ESC/close 内蔵） | HTML 静的モーダル約20個 + 総当たり `closeModal` 配列 + `btn-close` 一括バインド |
| `Card.svelte`（glass hover-lift, slot） | glass カード枠の重複 |
| `ProgressBar.svelte` | `buildProgressBar`(.task-progress-bar) |
| `Button.svelte`（variant: mini/trash/icon-sm） | `buildMiniButton` / btn-trash 等 |
| `Badge.svelte` / `StatusChip.svelte` | admin-role-badge / int 状態チップ（稼働中/接続中…/停止中/未設定 → status→ラベル/色マップ） |
| `EmptyState.svelte` | 各 `fetchXxx` に散在する「…がありません」 |
| `Checkbox.svelte` / `TagChip.svelte` / `MetaItem.svelte` / `CharCounter.svelte` | checkbox-custom / tag-chip / `buildMetaItem` / カウンタ |
| `ConfirmDialog.svelte` / `Toast.svelte` | `confirm()` / `alert()`（`app.js` の `TODO(security)` 解消も兼ねる） |

### 11.3 タブ別ルート/子コンポーネント（主要）
- **BotTasks**（最複雑・移行第一弾に推奨）: `TaskCard` / `SubtaskRow`（再帰=`<svelte:self>`）/ モーダル `TaskEditModal`・`SubtaskModal`・`TaskProgressModal` / `GanttChart`（**唯一の chart.js 利用箇所** → 動的 import + `Chart.register(...registerables)`、`onDestroy` で `destroy()`）。共通部品とモーダル機構の設計をここで一度に検証。
- **IntegratedOverlay**（移行第二弾に推奨）: `IntBotList`/`IntCredentials`/`IntMcpServers`/`IntGoogleAccounts`。現状 `innerHTML` テンプレートリテラル + `data-int-do` 属性 + 後付けリスナーの3段構え → `{#each}` + `on:click` 直結へ。手動 `intEsc()` は Svelte 自動エスケープで不要。`⋮` メニューは `Dropdown` + click-outside action で `closeIntMenus` 廃止。
- **BotDashboard**: 手描き SVG チャート群（`UsageChart`/`DonutChart`/`PriceTrendChart`）を `{#each}` で `<path>`/`<line>` バインド描画（`innerHTML` 廃止）。`xAt`/`yAt`/`toLine` は純関数で保持。chart.js 非依存。
- **AdminOverlay**: `AdminUsersTable`/`AdminBotsTable`/`AdminInviteCodesTable`/`AdminAuditLog`/`AdminPersonas`/`AdminSystemSettings`。行は `{#each}` で `createElement` 全廃、`RoleBadge`/`EmptyState` 抽出、`confirm()`/`alert()` → `ConfirmDialog`/`Toast`。
- **BotConfig / BotDelivery**: `getElementById().value=` 手続きを fetch 結果オブジェクト + `bind:value`/`bind:checked` に全置換。hidden トグルは `{#if isAdmin || !isSystemDefault}`。芋づる fetch を各カードの `onMount` へ解体。
- **BotMcp**: iframe（`/api/mcp-servers/:id/dashboard` が独自 CSP で返す。sandboxed opaque origin から `/proxy/mcp/:id/mcp` を叩く）を `bind` + `onDestroy` で `teardownMcpDashboard` 相当。**P4 着手前に dev(Vite 前段) で iframe が表示・動作するか単独検証**（§5.6）。
- 他: BotTimeline / BotSchedules / BotExpenses / BotReminders / BotPersonal / BotPersonas / BotWebhooks / BotPlaybooks / BotDevices。

### 11.4 XSS 原則
`{@html}` は原則禁止。`innerHTML`+`intEsc` 由来コードを機械的に移植して `{@html}` を使うと XSS を再導入する。Svelte の自動エスケープに委ねる。

---

## 12. テーマ / CSS 移行

### 12.1 styles.css は分割せずグローバル持ち込み
`styles.css`（4833行）はテーマ変数トークン + 全コンポーネントクラス + 3テーマ上書き（`[data-theme=x] .foo`）が一体で相互参照。**Svelte scoped CSS に分割すると横断参照とテーマ上書きが壊れる**。→ `main.ts` で `import "./styles.css"` してグローバルに読む。将来コンポーネント固有分だけ scoped `<style>` に漸進切り出し。

### 12.2 テーマ状態
`data-theme` を `<html>` に付ける方式は全 CSS セレクタが依存するため**維持**。`applyTheme()` の全副作用を `stores/theme.ts` の `subscribe` に移す（§9.3）: `data-theme` 属性 / `localStorage(yuuka-theme)` / `meta[theme-color]` / `.theme-option` の active。ヘッダ toggle（dark↔light）と設定タブの3ボタンはローカルイベント → `theme.set()`。

### 12.3 FOUC 対策（厳格 CSP との両立）
`theme-init.js` は `<head>` 早期の**同期実行が必須**（Svelte マウントは body 後半で走るためそのままだと FOUC）。CSP が inline script 禁止のため **inline 化は不可** → 現状の外部 `<script src="/theme-init.js">` を踏襲する。

- `frontend/index.html` の `<head>` に、**Vite バンドルより前**で `<script src="/theme-init.js"></script>` を置く。**`type=module`/`defer`/`async` を付けない同期 script** とすること（type=module を付けると Vite がバンドル対象に巻き込み、順序・ハッシュ化が発生する）。`base` はデフォルト `/` を維持（`/theme-init.js` の絶対パスが書き換わらないように）。
- `theme-init.js` は `frontend/public/theme-init.js` に置き、Vite が**ハッシュなしで素通しコピー**する（`/theme-init.js` の URL を維持）。
- `.theme-no-transition` 除去（現 `app.js:8`）は `App.svelte` の `onMount` で行う。
- `manifest.json` の `theme_color`/`background_color` `#121212` を `theme-init.js` と整合させる。
- **P0 完了条件（検証固定）**: build 後 `dist/public/index.html` を grep し、(a) `<!-- GOOGLE_SITE_VERIFICATION -->` が1個残存、(b) `<script src="/theme-init.js">` が `type=module`/`defer` 化されず `<head>` の Vite バンドル参照より前に居ること、を確認。

### 12.4 Google Fonts / Material Symbols
`<head>` の `<link>`（Inter/JetBrains Mono/Material Symbols + preconnect）を `frontend/index.html` にそのまま移植。CSP の `style-src`/`font-src` は Google Fonts 許可済み（`server.ts:77`）→ **無改修**。

---

## 13. PWA / Service Worker 移行

### 13.1 問題
現 `sw.js`（`CACHE="yuuka-v10"`）は `PRECACHE` に固定パス `/app.js`, `/styles.css` 等ハッシュなしを列挙（`sw.js:2-10`）。Vite はファイル名をハッシュ化（`index-<hash>.js`）するため **固定パスは 404 → precache されない**。旧 `sw.js` が本番ブラウザに残っていると、新デプロイ後もキャッシュ済み旧 `index.html` が古いハッシュ資産を要求し**白画面事故**の恐れ。

### 13.2 対応: `vite-plugin-pwa`（Workbox・self ホスト必須）
- `vite-plugin-pwa` の `injectManifest`（現戦略を保持したい場合）または `generateSW` を導入し、**ビルド時にハッシュ付きアセットの precache manifest を自動生成**。固定パス列挙は廃止。
- **Workbox ランタイムは self ホストする**（`workbox.inlineWorkboxRuntime: true`、または Workbox をローカルバンドル）。既定の `importScripts('https://storage.googleapis.com/workbox-cdn/...')` は **CSP `script-src 'self'` / `worker-src 'self'` に違反**し SW 登録が失敗する。**build 後に `dist/public/sw.js` に `googleapis`/CDN URL が無いことを grep で検証**するステップを追加。
- 現 `sw.js` の方針を Workbox ストラテジーへ写像:
  - アプリシェル（`/`,`*.js`,`*.css`,`*.html`＝現 `isAppShell` の `/\.(js|css|html)$/`）→ `NetworkFirst`
  - その他静的資産 → `StaleWhileRevalidate`
  - **`/api/` と `/hook/` は完全バイパス**（Workbox 移行時にこの除外を落とすと認証・動的データがキャッシュされ壊れる。Cookie セッション前提で特に危険）
  - `skipWaiting` + `clientsClaim` + 旧キャッシュ削除 → Workbox 標準（即時反映）
- `sw.js` は **`/sw.js`（origin ルート）配信を維持**。`manifest.json`・icons・`materials/yuka.webp` は `frontend/public/` に静的配置し **URL を維持**（`start_url='/'`, icons パス現行一致）。

### 13.3 旧 SW 退役の白画面リスク（実機検証を完了条件に）
新デプロイ直後、旧 v10 SW が Workbox 版へ更新される前の1回は旧 SW が fetch を制御する遷移期がある。緩和の要は **index.html/sw.js の no-cache 維持（§6.2）で新 index.html が必ず network から取れること + Workbox の `clientsClaim`/`skipWaiting` 即時反映**。リリースノート周知だけに頼らない。

**P1(SW 差し替え) 完了条件**: 既存 v10 SW 登録済みブラウザで、更新後に (a) 旧キャッシュ全削除、(b) 新ハッシュ資産が取得され白画面が出ないこと、を **実機（既存登録ありのウィンドウ + シークレット窓の両方）** で確認。index.html/sw.js の no-cache が旧 SW 配下でも効くことを検証項目化。

---

## 14. 段階的移行 vs 一括移行 / マイルストーン

**一括移行は非推奨**（`index.html` 3367行・`app.js` 10010行、リグレッション面積が過大）。**ストラングラー方式**でオーバーレイ/タブ単位に段階移行する。

| フェーズ | 内容 | 完了条件 |
|---|---|---|
| **P0 基盤** | `frontend/` 新設、vite.config（proxy=/api・/ws/chat のみ, target env 化, assetsDir 固定, base=/, assetsInlineLimit=0）、`styles.css` グローバル持ち込み、`theme-init.js`/`manifest.json`/icons を `frontend/public/` へ **copy**、`main.ts`/`App.svelte` 骨格、`stores/`（session/activeBot(旧4キー移行込み)/theme）、`lib/api/client.ts`、`components/ui/` | `pnpm dev:all` で Vite(5173) が起動し `/api/me` プロキシで認証が通る。FOUC なし。**build 後 GSV コメント=1 / theme-init.js が同期・非module** を grep 確認。 |
| **P1 骨格移行** | auth（login/register/DMチャレンジ/setup）・ルーター（applyRoute 全パス + PUBLIC_PATHS 分離）・session ゲート・Bot 選択・テーマ・SW(vite-plugin-pwa, self ホスト) | 全 URL がルーティング。401 集中ハンドラが公開ルート/bootstrap を除外して動作。**旧 v10 SW からの実機更新で白画面なし**（§13.3）。 |
| **P2 縮図タブ** | **BotTasks**（再帰 SubtaskRow / 4モーダル / ProgressBar・MiniButton・MetaItem / Chart.js ガント） | tasks タブ完全動作。共通 Modal 機構と chart.js 動的 import 検証済み。 |
| **P3 innerHTML 系** | **IntegratedOverlay** + AdminOverlay | 統合/管理が完全動作。`confirm/alert` → ConfirmDialog/Toast。自動エスケープで XSS 再導入なし。 |
| **P4 残タブ** | Dashboard/Timeline/Schedules/Expenses/Reminders/Personal/Personas/Delivery/Webhooks/**Mcp(iframe 単独検証済)**/Playbooks/Discord/Config/Devices/Account/Device | 全タブ移行完了。MCP iframe が dev/prod で動作確認済み。 |
| **P5 撤去** | 旧 `src/public/{app.js,styles.css,index.html,vendor/chart.umd.min.js}` 削除、`Dockerfile` の **`COPY src/public` の1行のみ**撤去（`COPY src/assets` は残す）、`PUBLIC_DIR=dist/public` 一本化 | vanilla 資産ゼロ。deploy verify(§15) で CSP/asset 機械チェック合格。 |

**共存中の切替とロールバック整合**: P1〜P4 は `STATIC_DIR` で新旧配信を切替えられる。**そのため P5 まで `src/public` は旧フルセット（index.html/app.js/styles.css/theme-init.js/manifest/sw/icons/materials/404/vendor）を凍結**し、`frontend/public` へは **move ではなく copy**（`develop-no-history-delete` の move 方針とは別で、ここは *ロールバック整合を優先して copy*）。**P5 以降は `STATIC_DIR` ロールバックを廃し、イメージロールバック（`deploy rollback`）に一本化**する。

---

## 15. リスクと緩和・ロールバック・デプロイ検証

| リスク | 緩和策 |
|---|---|
| **botId 注入モデルの反転（opt-in 化）→ 全 bot-scoped API が silent に system_default** | opt-in にしない。`scope` を**型で必須化**し bot-scoped メソッドで省略をコンパイルエラーに。除外は**プレフィックス一致**で再現。単体テストで固定。 |
| **botId 除外の exact-match 化で `/api/bots/*` サブルートが誤注入** | 除外を `/api/bots` プレフィックス + auth/me/device/setup で実装。`/api/bots/usage` は呼び出し側手動 botId。 |
| **multipart 送信を JSON 化して破壊** | `body instanceof FormData` で Content-Type 自動・botId は query。FormData 箇所を grep 列挙しテスト。 |
| **401 集中ハンドラが公開ルート/bootstrap で匿名を弾く** | `isPublicPath`/`isBootstrap` で goto('/login') をゲート。 |
| **dev API が https BASE_URL でセッション不達** | dev API は BASE_URL 未設定(http) 前提を明記。 |
| **CSP `script-src 'self'` と Vite の衝突** | prod は外部ハッシュ化で通る。`assetsInlineLimit:0`。dev は proxy で迂回。deploy verify で機械チェック。 |
| **Workbox が CDN importScripts で CSP 違反** | `inlineWorkboxRuntime:true`（self ホスト）。build 後 sw.js に CDN URL 無しを grep。 |
| **gzip 二重圧縮 / precompress 無駄** | precompress(.gz) を出さない（既定 (A)）。出すなら serveStaticFile に .gz 差し替え+Content-Length。 |
| **immutable 判定の脆さ（入れ子/publicDir 誤判定）** | ディレクトリ完全一致でなく「assets/ 配下 + ハッシュ命名正規表現」で判定。no-cache 側をテスト固定。 |
| **FOUC 復活** | theme-init.js を `<head>` 同期・非module 外部 script として温存。build 後 grep 検証。 |
| **SW ハッシュ非互換で白画面** | vite-plugin-pwa で precache 自動生成。`/api`・`/hook` バイパス。旧 SW→新 SW 実機検証。 |
| **CSRF 契約破壊** | dev は proxy `changeOrigin:false` で same-origin。prod は同一オリジン。 |
| **GSV コメントを minify が削除** | build 後 `grep -c` が 1 であること。0 なら minify:false / transformIndexHtml 保護。 |
| **`dist/` 衝突・emptyOutDir 全消し** | outDir=dist/public に分離。emptyOutDir は dist/public 配下のみ。outDir を dist 直下に向けない。 |
| **Dockerfile 順序 / lockfile 不一致** | install → tsgo → vite build → prune 厳守。vite/svelte を devDeps 追加し pnpm-lock.yaml 更新・コミット。 |
| **runtime の src/assets 巻き添え削除** | P5 で消すのは `COPY src/public` の1行のみ。`COPY src/assets` は残す。 |
| **activeBot 旧4キー未移行で既存ユーザー Bot 未選択落ち** | load() に旧4キー→新キー one-time マイグレーション（§9.2）。 |
| **MCP iframe が dev で不動作** | /proxy は proxy に載せず、P4 前に iframe 単独検証。 |
| **オーバーレイ排他表示リグレッション** | `{#if}`/ルーターで単一表示ソースに統一。CSS `.active` と Svelte 条件の二重管理を避ける。 |

### 15.x deploy verify の強化（手動 DevTools に依存しない）
現 `deploy/instance.sh verify`（`:72`）は `curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:$port/` で **HTTP ステータスのみ**を見る。CSP・`/assets/*`・GSV 注入は検証しない。prod デプロイ後の自動検証として以下を verify に追加（具体コマンド）:
```sh
BASE="http://127.0.0.1:$port"
# (a) CSP に script-src 'self'
curl -sI "$BASE/" | grep -qi "content-security-policy:.*script-src 'self'" || { echo "CSP NG"; exit 1; }
# (b) index.html が hashed module を参照し inline script を含まない
HTML=$(curl -s "$BASE/")
echo "$HTML" | grep -qE 'src="/assets/[^"]+-[A-Za-z0-9_-]{8,}\.js"' || { echo "hashed module NG"; exit 1; }
# (c) GSV meta 注入（config 設定時）
echo "$HTML" | grep -q 'google-site-verification' || echo "WARN: GSV 未注入（config 次第）"
# (d) /assets/*.js が 200 かつ immutable
ASSET=$(echo "$HTML" | grep -oE '/assets/[^"]+\.js' | head -1)
curl -sI "$BASE$ASSET" | grep -qi "cache-control:.*immutable" || { echo "immutable NG"; exit 1; }
```

### ロールバック手順
- **サーバー**: `server.ts` の変更は `PUBLIC_DIR` の env 分岐 + Cache-Control 分岐の2点のみ。**〜P4 は `STATIC_DIR=src/public` で旧 vanilla 配信へ即時ロールバック**（旧 `src/public` を P5 まで凍結）。
- **Docker**: `COPY src/public` を P5 まで残すことで `STATIC_DIR=src/public` ロールバック可能。**P5 以降は `deploy rollback`（前イメージ）に一本化**。
- **段階粒度**: フェーズ単位でブランチを切り、各フェーズを独立ロールバック可能に保つ。

---

## 16. 着手チェックリスト（最初の1週間・順序付き）

1. **[環境確認]** dev-hot コンテナ（dev=7855）or ホスト `tsx watch`（7854）が API を返すことを確認。**dev API の `BASE_URL` が未設定(http)** であること（§5.5）。CSP が API 応答に乗ることを DevTools で確認。
2. **[足場]** `frontend/` 新設。§5.1 の依存を `pnpm add -D`。**`pnpm-lock.yaml` をコミット**（Docker `--frozen-lockfile` 前提）。`vite.config.ts`（`outDir=../dist/public`, `base=/`, `assetsDir=assets`, `assetsInlineLimit=0`, `publicDir=public`, `server.proxy` は **`/api`・`/ws/chat`(ws:true) のみ**, target を `VITE_API_TARGET` 既定 7855）。
3. **[静的資産 copy]** `theme-init.js`・`manifest.json`・`icons/`・`materials/`・`404.html` を `frontend/public/` へ **copy**（move しない=ロールバック凍結）。`sw.js` は PWA 生成へ。`frontend/index.html` の `<head>` に GSV コメント1個・Google Fonts link・**同期・非module** `<script src="/theme-init.js">`（バンドル参照より前）を配置。
4. **[CSS]** 旧 `styles.css` を `frontend/src/styles.css` へ丸ごと copy し `main.ts` で `import`。テーマ3種が FOUC なしで切替わることを確認。
5. **[ストア]** `stores/session.ts`（`currentUser`/`isAdmin`）、`stores/activeBot.ts`（**旧4キー one-time マイグレーション + browser ガード**）、`stores/theme.ts`（applyTheme 副作用移植）。
6. **[API クライアント]** `lib/api/client.ts`（same-origin + credentials、**scope 型必須**の botId 注入、プレフィックス除外、FormData 分岐、複合成否判定、公開ルート/bootstrap を除外した 401 割り込み）。**除外・FormData・botId 有無の単体テストを先に書く**。`deviceApi.pollToken` はエンベロープ外で実装（§10.4）。
7. **[ルーター]** `lib/router.ts`（`navigateTo`/`applyRoute` 相当、cleanPath 流用、popstate 1個、`isPublicPath`/`currentRoute` export）+ §8 全パステーブル + PUBLIC_PATHS 分離。
8. **[共通 UI]** `components/ui/` の Icon・Modal・EmptyState・StatusChip・Button・ProgressBar・ConfirmDialog・Toast を先に実装。
9. **[サーバー最小変更]** `server.ts` の `PUBLIC_DIR`（`STATIC_DIR` env 分岐、既定 `dist/public`）と Cache-Control immutable 分岐（**ハッシュ命名正規表現**）を実装。`pnpm build:front` の出力を `STATIC_DIR=dist/public node dist/index.js` で 7854 配信し、DevTools + §15.x の curl で **CSP 違反ゼロ / GSV meta 注入 / `/assets/*` immutable** を確認。build 後 `grep -c GOOGLE_SITE_VERIFICATION dist/public/index.html` = 1、`grep googleapis dist/public/sw.js` = 空 を確認。
10. **[P2 着手]** BotTasks を移行第一弾（共通 Modal 機構 + chart.js 動的 import を確定）。

---

主要な変更対象ファイル（すべて絶対パス）:
- 最小変更: `src/server.ts`（`PUBLIC_DIR` §6.1 / Cache-Control §6.2）
- ビルド統合: `Dockerfile`（§7）, `package.json`（scripts §5.3, build §5.4）
- lockfile: `pnpm-lock.yaml`（§5.1 更新・コミット必須）
- dev: `docker-compose.dev-hot.yml`, `deploy/instance.sh`（コメント同期 §5.7 / verify 強化 §15.x）
- 新設: `frontend/`（§4）
- 撤去（P5・1行のみ / `src/assets` COPY は残す）: `src/public/{app.js,styles.css,index.html,vendor/chart.umd.min.js}`