# Svelte 移行 残作業計画書（続編 — ランタイム検証・遅延ロード・SW・Docker/Deploy・撤去）

> **⚠️ 履歴資料（Svelte 移行は完了済み・Node バックエンドは撤去済み）**
> 本書は、Node.js バックエンド（`src/server.ts`・`tsx`・`tsgo`・`node dist/index.js`・`STATIC_DIR` 環境変数・`src/public` の旧バニラ SPA 等）が存在した時期に書かれた移行の残作業計画書です。これらは現行リポジトリには存在せず、本文中のコマンド・ファイルパス・ポート・Dockerfile の行番号は**現行の手順ではありません**。
> 現行の構成は次を参照してください: 管理画面 SPA = `frontend/`（`pnpm dev` / `pnpm build:front`、出力は `dist/public`）、バックエンド = `crates/`（Rust。管理画面は `/admin` 配下に静的配信）、セットアップ = [guide/setup.md](guide/setup.md)、デプロイ = [guide/deployment.md](guide/deployment.md)、全体像 = [project_overview.md](project_overview.md)。

対象: `リポジトリルート` / ブランチ `feature/svelte-migration`
親設計書: `docs/svelte-migration-plan.md`（全16章。以下 §N はこの親書の章番号を指す）
本書の位置づけ: 親書 §5〜§7・§13・§14・§15 に残る「実装済み・ビルド緑・ただしランタイム未検証」の状態から本番デプロイまでを、実行可能粒度に落とした続編。バックエンド（`server.ts` の `serverHandler` / route registry / `src/server/routes/*` / API 契約）は原則不変。CSP `script-src 'self'`（`src/server.ts:80-81`）は不変。

---

## 1. 概要 / 前提 / ゴール

### 1.1 現在地（確定事実・各ファイルで裏取り済み）

| 項目 | 状態 | 根拠ファイル |
|---|---|---|
| P0 基盤 + 全15 Botタブ + 全オーバーレイ実装 | 完了 | `frontend/src/routes/`, `frontend/src/overlays/` |
| ビルド緑 | svelte-check 454 files 0 errors / `build:front` 成功 / backend `tsgo` 0 errors | — |
| **ランタイム実機動作** | **未検証（最大の未解決ゲート）** | — |
| `server.ts` の `PUBLIC_DIR`（`STATIC_DIR` env 分岐, 既定 `dist/public`） | 実装済み | `src/server.ts:66-68` |
| Cache-Control immutable 分岐（ハッシュ命名正規表現） | 実装済み | `src/server.ts:154-160` |
| GSV 実行時置換 | 実装済み・入力HTMLにコメント1個あり | `src/server.ts:171-185` / `frontend/index.html:7` |
| CSP 定数（`script-src 'self' https://static.cloudflareinsights.com` に加え **`worker-src 'self'` を明示保持**・`connect-src`/`frame-src`/`frame-ancestors` 含む） | 実装済み・不変方針 | `src/server.ts:80-81`（`worker-src 'self'` は `:81`） |
| `package.json` scripts（`dev:front`/`dev:all`/`build:front`/`typecheck:front`）と `build` 行末の `vite build` | 追記済み | `package.json:16-20` |
| `vite.config.ts`（proxy `/api`・`/ws/chat` のみ, `VITE_API_TARGET` 既定 7855, `base=/`, `assetsInlineLimit:0`, `outDir=../dist/public`, `$lib` alias） | 実装済み | `frontend/vite.config.ts` |
| `frontend/public/`（`theme-init.js`/`manifest.json`/`icons/`/`materials/`/`404.html`） | copy 済み・**`sw.js` は無い**（PWA 生成予定） | `frontend/public/` |
| `frontend/index.html`（GSVコメント / Google Fonts / **同期・非module** `<script src="/theme-init.js">`） | 実装済み | `frontend/index.html:7,19-28` |
| Service Worker 登録（`main.ts`） | **未実装（コメントのみ）** | `frontend/src/main.ts:5` |
| `vite-plugin-pwa` 依存 | **未導入**（`package.json` devDeps に無い） | `package.json:58-76` |
| 旧 `sw.js`（`CACHE="yuuka-v10"`, 固定パス `/app.js`・`/styles.css` を precache） | 残存・Viteハッシュ名と非互換 | `src/public/sw.js:1-10` |
| BotShell の15タブ | **全て静的 import** + `TAB_COMPONENTS` マップ + 動的 `<Current />` | `frontend/src/routes/BotShell.svelte:25-62,148,252` |
| App のオーバーレイ | 全て静的 import + `{#if effectiveView}` | `frontend/src/App.svelte:35-45,120-144` |
| tasks の chart.js 遅延分離 | **既に分離済み**（`chart-*.js` 202212 B。`frontend/src/routes/tasks/GanttChart.svelte:42` の `await import("chart.js")` が P1b 未着手でも既に効いている） | `dist/public/assets/chart-*.js` |
| 現状バンドル | 単一 `index-*.js`（実測 **354288 B**）+ `chart-*.js`（202212 B, 遅延分離済み）+ `index-*.css`（104454 B） | `dist/public/assets/` |
| Dockerfile stage2 に `vite build` ステージ | **未追加**（`Dockerfile:71-73` は `tsgo` + `cp src/assets` のみ） | `Dockerfile:71-73` |
| Dockerfile runtime の `COPY src/public` | 残存（`Dockerfile:105`） | `Dockerfile:105` |
| `docker-compose.dev-hot.yml` のコメント（「フロントは src/public をブラウザリロードで即反映」） | 旧前提のまま（Svelte化で崩れる） | `docker-compose.dev-hot.yml:4-6,30-32` |
| `deploy/instance.sh verify`（`health_check`） | `curl http_code` のみ（CSP/hashed/immutable 未検証） | `deploy/instance.sh:68-89` |

### 1.2 ゴール
- **RV（ランタイム検証）を最優先ゲート**として通し、`dev:all` 開発フローと `STATIC_DIR=dist/public node dist/index.js` 本番相当配信が実機で動くことを確認する。
- 当初要望「動的読み込みで軽く」を **P1b ルート遅延ロード**で実現（15タブ + 主要オーバーレイをルート毎チャンクに分離、初期JS削減）。
- 旧 v10 SW からの白画面事故を回避しつつ **P1a `vite-plugin-pwa`** で SW を CSP 準拠・self-host 化。
- **Docker §7** に vite build ステージを組み込み本番イメージでフロントを配信可能に。
- **Deploy §5.7/§15.x** で dev-hot / instance.sh のコメント同期と verify 強化。
- **P5** で旧 vanilla 資産を撤去し `PUBLIC_DIR=dist/public` に一本化。

### 1.3 非ゴール
- バックエンド API 契約・route registry・認証/セッション/DB スキーマの変更。
- CSP の緩和（`script-src 'self'` 維持）。**`worker-src 'self'` は既存のため SW 導入で CSP 変更は発生しない**（P1a 参照）。
- SSR 化・SvelteKit 導入。

---

## 2. フェーズ順序と依存関係

```
                    ┌─────────────────────────────────────────────┐
                    │  RV: ランタイム実機検証（最優先ゲート）        │
                    │  dev:all 起動 / 認証 / 全タブ smoke /         │
                    │  STATIC_DIR=dist/public 本番相当配信 curl     │
                    └───────────────────┬─────────────────────────┘
                            RV 合格が全後続の前提（ここが赤なら他は無意味）
          ┌──────────────────┬──────────┴──────────┬───────────────────┐
          ▼                  ▼                     ▼                   ▼
   ┌────────────┐   ┌────────────────┐   ┌────────────────┐   ┌──────────────┐
   │ P1b 遅延    │   │ P1a SW         │   │ MCPV           │   │ Docker §7     │
   │ ロード      │   │ vite-plugin-pwa│   │ MCP iframe     │   │ vite build    │
   │ (BotShell/  │   │ (self-host,    │   │ dev/prod 単独   │   │ ステージ追加   │
   │  App 動的)  │   │  /api・/hook   │   │ 検証            │   │ + COPY 調整    │
   │            │   │  bypass)       │   │                │   │              │
   └─────┬──────┘   └───────┬────────┘   └───────┬────────┘   └──────┬───────┘
         │ 互いに独立・並行可 │                    │                    │
         │ （どちらも build:front を叩くが成果物は別）                  │
         └──────────┬───────┴────────────────────┘                    │
                    │  P1a/P1b/MCPV 完了 → 本番ビルド確定               │
                    ▼                                                  │
              ┌───────────────────────────────────────────────────────┴──┐
              │ Deploy §5.7/§15.x                                          │
              │ docker-compose.dev-hot.yml コメント同期 /                  │
              │ deploy/instance.sh verify に CSP/hashed/immutable curl     │
              │ （Docker §7 完了に依存: verify は本番イメージ配信を検証）   │
              └───────────────────────────┬───────────────────────────────┘
                                          ▼
                          ┌───────────────────────────────────┐
                          │ P5 撤去（最後・不可逆に近い）        │
                          │ 旧 src/public/{app.js,styles.css,   │
                          │ index.html,vendor} 削除 /           │
                          │ Dockerfile COPY src/public 1行撤去 / │
                          │ STATIC_DIR ロールバック廃止          │
                          └───────────────────────────────────┘
```

**依存関係の要点**:
- **RV は全後続のブロッカー**。RV が赤（例: proxy 越しにセッションが刺さらない、タブが白画面）なら、その原因を潰すまで P1a/P1b/Docker に着手しない。
- **P1a（SW）と P1b（遅延ロード）は相互独立で並行可**。両者とも `vite build` の出力を変えるが、P1b は chunk 構成、P1a は `sw.js` 生成と別成果物。ただし **P1a の precache manifest は P1b のチャンク分割後のハッシュ資産を対象にすべき**なので、両方を入れたら最後に一度まとめて `build:front` → SW manifest 確認する（precache 対象チャンク名が P1b 分割後のものに追随していることを確認）。
- **MCPV は P1a/P1b と独立**。ただし MCP iframe が dev で不動作なら BotMcp タブの RV smoke が不完全になるため、RV の一部として MCP タブだけ「iframe 表示可否」を切り出して先行確認してよい。
- **Docker §7 は P1a/P1b 完了後**（本番イメージに入れる成果物が確定してから）。
- **Deploy §15.x の verify 強化は Docker §7 完了に依存**（verify は本番相当イメージの配信を curl するため）。dev-hot コメント同期（§5.7）は Docker §7 と独立に先行可。
- **P5 は最後**。RV/P1a/P1b/MCPV/Docker/Deploy が全て緑になり、`STATIC_DIR=src/public` ロールバックが不要と確信できてから。

---

## 3. 各フェーズ詳細

---

### RV — ランタイム実機検証（最優先ゲート）

#### 目的
ビルドが緑でも未検証な「proxy 越しの認証・セッション維持・全タブ描画・本番相当配信」を実機で通し、後続フェーズの土台を確定する。

#### 具体手順

**RV-1. dev API サーバを BASE_URL 未設定（=http）で起動**
親書 §5.5 の通り、dev（非HTTPS）ではセッション Cookie 名が `yuuka-session`（`__Host-` は Secure 必須で付かない）。API の `BASE_URL` が https だと `__Host-` Cookie しか発行されず、Vite の http proxy(5173) 越しに Cookie が刺さらない「ログインは成功するがセッション維持されない」サイレント失敗になる。
```bash
# dev-hot コンテナ(7855) を使う場合 — instance.env に BASE_URL を入れない
deploy/instance.sh dev hot -d
# もしくはホスト直 tsx watch(7854)。この場合 VITE_API_TARGET を 7854 に上書き
cd リポジトリルート && env -u BASE_URL pnpm dev:host
```
確認: 起動ログに `BASE_URL` 由来の https 表示が無いこと。DevTools Application → Cookies で `yuuka-session`（`__Host-` 無し）が出ること。

> **ポートの正体（重要・混同注意）**: ホスト直 `node dist/index.js` / `pnpm dev:host` が listen する **7854 は、リポジトリ直下 `config.yaml` の `PORT: 7854`**（`config.ts:98` `port: getSetting('PORT','3000')`、`CONFIG_PATH=cwd/config.yaml`, `config.ts:7`）に由来する。**dev-hot コンテナの公開ポート HOST_PORT=7855**（`deploy/dev/instance.env`）とは別レイヤ。`config.yaml` が無い/PORT 未設定なら既定 3000 になる点にも注意。以降の BASE 指定はこの区別に従う（host-direct=7854 / dev-hot コンテナ=7855）。

**RV-2. dev:all 起動確認**
```bash
# dev-hot(7855) を使うなら VITE_API_TARGET は既定でよい
cd リポジトリルート && pnpm dev:all
# ホスト直 7854 を使うなら:
# VITE_API_TARGET=http://127.0.0.1:7854 pnpm dev:all
```
確認: Vite が 5173 で listen。ブラウザで `http://localhost:5173/` を開き、FOUC（テーマ未適用のちらつき）が無いこと（`theme-init.js` 同期実行の検証）。

**RV-3. proxy 越しのログイン → セッション維持**
- `/login` で login/register。`/api/login` が 5173 same-origin（`changeOrigin:false`）で通り、`Set-Cookie: yuuka-session=...` が刺さること（DevTools Network で確認）。
- ページ再読込後も `/api/me`（`bootstrapSession`）が 200 で `currentUser` を埋め、`/login` に戻されないこと。→ これが緑なら §5.5 の Cookie 契約が正しい。
- `/ws/chat` の WebSocket が接続すること（`vite.config.ts` の `ws:true` 検証）。DevTools Network → WS でハンドシェイク成立を確認。

**RV-4. 各ビュー smoke（15タブ + 全オーバーレイ）**
`App.svelte` の `effectiveView` 分岐（`App.svelte:66-94`）と `BotShell` の `TAB_COMPONENTS`（`BotShell.svelte:46-62`）を一通り踏む。
- Bot選択 → `/bot/config` に入り、サイドバー `$menuItems`（プリセット別フィルタ, `BotShell.svelte:117-124`）が secretary/mcp_assistant で正しく出し分くこと。
- 15タブ（dashboard/tasks/timeline/schedules/expenses/reminders/personal/personas/delivery/webhooks/mcp/playbooks/discord/config/devices）を順に開き、白画面・console error が無いこと。特に:
  - **tasks**: chart.js 動的 import（**`frontend/src/routes/tasks/GanttChart.svelte:42`** の `await import("chart.js")`。実ファイルは `routes/` 直下ではなく `tasks/` サブディレクトリにある。`BotTasks.svelte:26` が `import GanttChart from "./tasks/GanttChart.svelte"` で参照）が dev で解決し Gantt が描画されること。※この動的 import は既に `chart-vM59ydkj.js`（202212 B）へ分離済みで、P1b 未着手でも既に効いている。
  - **mcp**: MCP iframe（→ MCPV で別途詳細検証。ここでは「タブが例外を投げず開く」まで）。
- 公開ルート `/usage`・`/terms`・`/privacy`・`/tasks/guide` を **未ログイン状態**で開き、401 で `/login` に弾かれないこと（`App.svelte:64,70` の `PUBLIC_VIEWS` と client の 401 集中ハンドラ除外の検証）。
- `/admin` を非admin で開き `/`（bots）へ丸められること（`App.svelte:84`）。
- `/device?code=xxx` で `?code=` が保持されること。

**RV-5. 本番相当配信の検証（STATIC_DIR=dist/public）**
```bash
cd リポジトリルート
pnpm build:front            # dist/public/ を再生成
# GSV コメントとハッシュ資産の静的検証
grep -c 'GOOGLE_SITE_VERIFICATION' dist/public/index.html   # 期待: 1（未置換のプレースホルダコメント）
grep -oE '/assets/[^"]+-[A-Za-z0-9_-]{8,}\.js' dist/public/index.html | head  # ハッシュ module 参照あり
# 本番相当で node 起動（backend も tsgo 済みが前提。無ければ pnpm build か tsgo を先に）
STATIC_DIR=dist/public node dist/index.js &
# 検証対象は host-direct プロセス。7854 = config.yaml の PORT 値（config.ts:98, cwd=リポジトリ直下）。
# host-direct 起動時のみ 7854 が有効。dev-hot コンテナ経由で検証するなら BASE=http://127.0.0.1:7855（HOST_PORT）に読み替える。
BASE=http://127.0.0.1:7854
curl -sI "$BASE/" | grep -i 'content-security-policy'       # script-src 'self' / worker-src 'self' を確認
curl -s  "$BASE/" | grep -oE 'src="/assets/[^"]+\.js"'      # hashed module
ASSET=$(curl -s "$BASE/" | grep -oE '/assets/[^"]+\.js' | head -1)
curl -sI "$BASE$ASSET" | grep -i 'cache-control'            # immutable を確認
curl -sI "$BASE/theme-init.js" | grep -i 'cache-control'    # no-cache を確認（immutable でないこと）
```
確認: `/assets/*.js` が `public, max-age=31536000, immutable`、`index.html`/`theme-init.js` が `no-cache`。`server.ts:154-160` の正規表現分岐が実際に効くこと。CSP に `script-src 'self'` と `worker-src 'self'` の両方が載ること（後者は既存。P1a で SW を入れても不変）。

> **ハッシュ正規表現の閾値注記**: 上の `-[A-Za-z0-9_-]{8,}\.js`（8字以上）は Vite 既定ハッシュ=base64url **8字ちょうど**（例 `index-B1q1mpir.js`）に対する下限ギリギリの判定。`server.ts:157` の immutable 判定正規表現も同一閾値で連動するため、`vite.config.ts` で `output.assetFileNames`/hash 長を変えるなら**両所を同時更新**する（この RV-5 grep・§15.x verify・`server.ts:157`）。より頑健にするなら下限を `{6,}` へ統一する選択肢もある。

注意: サーバ応答の `/` は `server.ts:171-185` の GSV 実行時置換を通るため、`curl` 側では `GOOGLE_SITE_VERIFICATION` コメントが env 未設定なら残存・設定済みなら `<meta name="google-site-verification">` に置換される。上の静的 grep（`dist/public/index.html`）は置換前の入力を見ている点に注意（RV-5 の grep=1 と §15.x verify の meta チェックは別レイヤ）。なお `curl` はデフォルトで `Accept-Encoding` を送らないため、全 curl 応答は非圧縮（`server.ts:105-108` `acceptsGzip=false` 経路）で返り、`Content-Encoding`/`Vary` は付かず grep は生 HTML/ヘッダに当たる。

#### 失敗時の切り分け手順
| 症状 | 切り分け | 対処 |
|---|---|---|
| ログインは通るが再読込でログイン画面に戻る | DevTools → Cookies に `yuuka-session` があるか / `__Host-` だけになっていないか | dev API の `BASE_URL` を unset（RV-1）。§5.5 の典型failure |
| `/api/*` が 404 / CORS | Vite proxy target が 7855/7854 のどちらを指すか（`VITE_API_TARGET`）。7855=dev-hot コンテナ、7854=host-direct（config.yaml PORT） | API の実ポートに合わせ `VITE_API_TARGET` を上書き |
| `/ws/chat` が繋がらない | `vite.config.ts:31-35` の `ws:true` が効いているか | target が `ws://` に置換されているか確認（`API.replace("http","ws")`） |
| POST/DELETE が CSRF 403 | `changeOrigin:false`（`vite.config.ts:30,34`）で same-origin か / Origin ヘッダが 5173 か | proxy 設定の changeOrigin を触らない。§5.2 |
| bot-scoped データが「別Botに見える」/空 | 対象 API に botId が乗っているか（GET=query, POST/DELETE=body）。サーバは未指定を `system_default` にサイレントフォールバック（§10.1） | `lib/api/client.ts` の `scope:'bot'` 注入を確認。除外プレフィックス誤爆も疑う |
| タブが白画面・console error | 該当 `Bot*.svelte` の onMount fetch エラーか import エラーか | Network で API 応答を確認。import エラーなら P1b 遅延ロード前の静的 import 問題を切り分け |
| 本番配信で `/` が 404 | `STATIC_DIR=dist/public` が未ビルドで空 | `pnpm build:front` を先に。§6.1 副作用 |
| GSV meta が付かない（config 設定済みなのに） | 実行時置換（`server.ts:171-185`）が env を読めているか / 入力コメントが minify で消えていないか | `build.minify` の HTML 扱いを確認。§6.4 リスク |

#### 想定リスクと緩和
- **セッション不達（最頻）** → RV-1 を最初に固定。BASE_URL 未設定を手順の1番に置く。
- **MCP iframe が dev で不動作**（`/proxy/mcp` は proxy に載せていない, §5.6）→ MCPV へ分離し、RV では「タブが開く」までを合格ラインにする。

#### ロールバック
RV はコード変更を伴わない検証フェーズ。問題が出たら該当箇所を修正（多くは client/router/store）してから再検証。サーバ配信で問題が出たら `STATIC_DIR=src/public` で旧 vanilla 配信に即戻せる（`server.ts:66-68`）。

---

### P1b — ルート遅延ロード（当初要望「動的読み込みで軽く」の実現）

#### 目的
`BotShell.svelte` の15タブ静的 import（`BotShell.svelte:25-39`）と `App.svelte` のオーバーレイ静的 import（`App.svelte:35-45`）を動的 `import()` 化し、ルート毎チャンクに分離。初期 `index-*.js`（実測 354288 B）を削減する。

#### Svelte 5 + Vite での正しい動的 import パターン

Svelte 5 には React の `lazy()` に相当する API は無い。`<svelte:component>` は非推奨化されており、現行 `BotShell` は既に動的 `Component` 変数を直接タグにする Svelte 5 方式（`<Current />`, `BotShell.svelte:148,252`）を使っている。動的 import は **`{#await import(...)}` で Promise を解決し、`.default` を動的コンポーネントとしてレンダリング**する。`{@const}` はブロック内でのみ許可される制約に留意（既存コードで確認済み）。

**パターンA（推奨・BotShell の TAB_COMPONENTS を loader マップ化）**
```svelte
<!-- before: BotShell.svelte:25-62 — 静的 import + 実体マップ -->
import BotDashboard from "./BotDashboard.svelte";
...（15行の static import）
const TAB_COMPONENTS: Record<BotTab, Component<Record<string, never>>> = {
    dashboard: BotDashboard, tasks: BotTasks, ... devices: BotDevices,
};
const Current = $derived(TAB_COMPONENTS[tab] ?? BotConfig);
```
```svelte
<!-- after: loader（() => import()）のマップに変える -->
import type { Component } from "svelte";
type Loader = () => Promise<{ default: Component<Record<string, never>> }>;

// Vite は各 import() を個別チャンクに分割する
const TAB_LOADERS: Record<BotTab, Loader> = {
    dashboard: () => import("./BotDashboard.svelte"),
    tasks:     () => import("./BotTasks.svelte"),
    timeline:  () => import("./BotTimeline.svelte"),
    schedules: () => import("./BotSchedules.svelte"),
    expenses:  () => import("./BotExpenses.svelte"),
    reminders: () => import("./BotReminders.svelte"),
    personal:  () => import("./BotPersonal.svelte"),
    personas:  () => import("./BotPersonas.svelte"),
    delivery:  () => import("./BotDelivery.svelte"),
    webhooks:  () => import("./BotWebhooks.svelte"),
    mcp:       () => import("./BotMcp.svelte"),
    playbooks: () => import("./BotPlaybooks.svelte"),
    discord:   () => import("./BotDiscord.svelte"),
    config:    () => import("./BotConfig.svelte"),
    devices:   () => import("./BotDevices.svelte"),
};
// tab をキーに Promise を一度だけ生成（load() を毎回呼ぶ再フェッチを避ける）
const modulePromise = $derived((TAB_LOADERS[tab] ?? TAB_LOADERS.config)());
```
```svelte
<!-- 本文: <Current /> を {#await} 置換（BotShell.svelte:251-253） -->
<div class="content-view-container">
    {#await modulePromise}
        <div class="tab-loading" aria-busy="true"></div>
    {:then module}
        {@const TabView = module.default}
        <TabView />
    {:catch}
        <div class="tab-error">タブの読み込みに失敗しました。再読込してください。</div>
    {/await}
</div>
```
`{#await modulePromise}` は `modulePromise`（`$derived`）が `tab` 変更で再生成されたときだけ再評価される。同一タブ内の他の再レンダリングでは同じ Promise 参照のままなので無駄なフェッチが起きない。Vite は動的 import されたチャンクを内部でキャッシュするため、同一タブへの再遷移でネットワーク往復は発生しない（2回目以降は解決済みモジュールが即返る）。

**パターンB（App のオーバーレイ — 採用形は `LazyView` に統一）**
`App.svelte:35-45` の各オーバーレイを loader 化。ただし `Login`/`BotSelection`/`BotShell` は初期表示で頻繁に踏むため、**分割対象を「重いオーバーレイ（IntegratedOverlay/AdminOverlay）」に絞る判断も可**。採用形は **`LazyView.svelte`（loader を prop で受け、内部で `const promise = $derived(loader())` して `{#await promise}` する薄いラッパ）を正**とし、`App.svelte:120-144` の各分岐を `<LazyView loader={loadAdmin} />` に統一する。
```svelte
<!-- before -->
import AdminOverlay from "./overlays/AdminOverlay.svelte";
{:else if effectiveView === "admin"}
    <AdminOverlay />
<!-- after（採用形） -->
const loadAdmin = () => import("./overlays/AdminOverlay.svelte");
{:else if effectiveView === "admin"}
    <LazyView loader={loadAdmin} />
```
```svelte
<!-- 参考: テンプレ直書き（非推奨・比較用）。採用しない -->
{:else if effectiveView === "admin"}
    {#await loadAdmin() then m}{@const V = m.default}<V />{/await}
```
直書き例を採らない理由: `{#await loadAdmin()}` をテンプレに直書きすると、その分岐が `{#if}`/`{:else if}` で mount される限りは1回評価だが、`effectiveView` が別ビューへ移り再び admin へ戻ると**ブロック再 mount 時**に新しい Promise が作られ `loadAdmin()` が再度呼ばれる（Vite のモジュールキャッシュがあるので実 fetch は1回で実害は小さいが、責務が曖昧）。`LazyView` 内部で `$derived(loader())` に束ねれば Promise 安定化の責務が1箇所に集約される。「再レンダリング毎に再評価」ではなく正確には「**ブロック再 mount 毎（Vite module cache で実 fetch は1回）**」。

#### 変更ファイル
- `frontend/src/routes/BotShell.svelte`（15 static import → loader マップ + `{#await}`）
- `frontend/src/App.svelte`（オーバーレイ static import → loader + `<LazyView>`。最低限 IntegratedOverlay/AdminOverlay を分離）
- `frontend/src/lib/components/ui/LazyView.svelte`（loader を prop で受け内部 `$derived(loader())` + `{#await}` する薄いラッパ。重複削減 + Promise 安定化。採用形の中核なので任意ではなく必須）

#### 完了条件（数値は相対条件を主とする）
```bash
cd リポジトリルート && pnpm build:front
ls -la dist/public/assets/*.js
```
- 各 Bot タブ / 主要オーバーレイが **独立チャンク**（`BotTasks-*.js`, `BotDashboard-*.js`, `AdminOverlay-*.js` 等）に分離されていること（`ls -la dist/public/assets/*.js` の実測で分離を確認）。← **これが合格判定の主軸**。
- 初期 entry `index-*.js` が **現状 354288 B から有意（例: 100KB 超）に削減**されていること（**相対条件を主**とする）。共有依存（Svelte5 runtime, `components/ui`, stores, api/services, router）は entry に残り15分割しても単純比例しないため、**絶対値「概ね 200KB 以下 / gzip 70KB 以下」は努力目標・参考値に降格**する。数値未達それ自体を「失敗」と誤判定しないこと。
- `pnpm typecheck:front`（svelte-check）が 0 errors。
- RV-4 の全タブ smoke を再実行し、遷移時にタブ内容が `{#await}` 経由で正しく描画されること（初回のみ短い loading、2回目以降は即時）。

#### 想定リスクと緩和
- **`{#await}` のブロック再 mount で毎回 loader が呼ばれる** → `LazyView` 内部の `$derived` で Promise をキーに束ね、Vite のモジュールキャッシュに任せる（実 fetch は1回）。直書き `{#await load()}` を避け `<LazyView>` に集約。
- **過剰分割で共有 chunk が増えファーストペイントの往復が増える** → タブは分離、共通 UI（`components/ui`）・stores・api client は entry に残す（Vite の自動共有 chunk 化に委ねる）。
- **プリロードの喪失（初回タブ遷移が体感遅い）** → Bot選択後によく使う `config`/`dashboard` の loader を BotShell mount 時に投機プリフェッチ（`load()` を捨て呼び）する任意最適化。

#### ロールバック
BotShell/App の import 差分のみ。`git revert` で静的 import に戻せば元の単一チャンクへ即復帰。サーバ・API 無関係。フェーズ独立ブランチ推奨。

---

### P1a — Service Worker（vite-plugin-pwa, self-host, CSP準拠）

#### 目的
旧 `sw.js`（`CACHE="yuuka-v10"`, 固定パス `/app.js`・`/styles.css` を precache, `src/public/sw.js:1-10`）は Vite のハッシュ名（`index-<hash>.js`）と非互換で 404 precache。これを `vite-plugin-pwa`（Workbox）でビルド時 precache manifest 自動生成に置換し、旧 v10 SW からの白画面事故を防ぐ。**`vite-plugin-pwa` は最新 1.3.0 が Vite 8（本リポは 8.1.2）を peer サポート**する。

#### 具体手順

**P1a-0. CSP は変更不要（確定事項）**
`server.ts:81` の CSP 定数には既に **`worker-src 'self';` が明示的に存在する**（`... connect-src 'self' https://cloudflareinsights.com; worker-src 'self'; frame-src 'self'; frame-ancestors 'self';`）。したがって self-host の `sw.js`（`'self'` origin）は `worker-src 'self'` に明示許可済みで、**CSP 定数の変更は一切不要**（非ゴール 1.3 と整合。「worker-src が有るか要確認」「無ければ script-src フォールバック」「追加時は親書 CSP 不変方針と合意」といった条件分岐は不要）。**唯一の CSP 準拠要件は、CDN importScripts（`https://storage.googleapis.com/...`）を SW に出さないこと**（= `script-src 'self'` 準拠）で、これは P1a-2 の `inlineWorkboxRuntime:true` で担保する。

**P1a-1. 依存追加（lockfile 更新必須）**
`vite-plugin-pwa` は現在 devDeps に無い（`package.json:58-76`）。
```bash
cd リポジトリルート
pnpm add -D vite-plugin-pwa
git add package.json pnpm-lock.yaml   # Docker --frozen-lockfile 前提（§7.1 / Dockerfile:66）
```

**P1a-2. `vite.config.ts` に VitePWA プラグイン追加**
更新 UX の責任者は1つに固定する。`registerType:"autoUpdate"` を採るなら **`skipWaiting`/`clientsClaim` を明示指定しない**（autoUpdate が内部で更新フローを管理する。`autoUpdate` + `skipWaiting:true` + `clientsClaim:true` の三点セットは、稼働中タブが新旧チャンク混在→古い index が新チャンクを import して 404→白画面、という §13.3 で回避したい事故を再誘発するため禁止）。即時反映が必須なら `registerType` 据え置きのうえ `main.ts` 側で `registerSW` の更新ハンドラ（新 SW activate 後に一度だけ `location.reload()`）を持たせる方式を採る。ここでは autoUpdate に委ねる構成を正とする。
```ts
// before: frontend/vite.config.ts:19
plugins: [svelte()],
// after:
import { VitePWA } from "vite-plugin-pwa";
plugins: [
  svelte(),
  VitePWA({
    registerType: "autoUpdate",     // 更新フローの単一責任者。skipWaiting/clientsClaim は明示しない
    injectRegister: null,           // 登録は main.ts で明示（下記 P1a-4）
    workbox: {
      // ★ 既定は https://storage.googleapis.com/workbox-cdn を importScripts する。
      //    inlineWorkboxRuntime:true で Workbox を sw.js にインライン化し CDN 参照を消す（唯一の CSP 準拠要件）。
      inlineWorkboxRuntime: true,
      // precache manifest は Vite のハッシュ資産から自動生成（固定パス列挙を廃止）。
      // ★ html は precache しない（GSV 実行時置換をバイパスさせないため。下記注記(a)）。
      globPatterns: ["**/*.{js,css,woff2,png,svg,webp,json,ico}"],
      navigateFallback: "/index.html",
      // /api・/hook・/ws・/proxy を navigateFallback から除外
      navigateFallbackDenylist: [/^\/api\//, /^\/hook\//, /^\/ws\//, /^\/proxy\//],
      runtimeCaching: [
        // index.html / "/" は precache せず NetworkFirst のみ（GSV 置換を必ず通す。注記(a)）
        { urlPattern: ({ url }) => url.origin === self.location.origin &&
            !/^\/(api|hook|ws|proxy)\//.test(url.pathname) &&
            (url.pathname === "/" || url.pathname === "/index.html"),
          handler: "NetworkFirst", options: { cacheName: "app-shell" } },
        // その他 same-origin 静的資産（/api・/hook・/ws・/proxy は否定条件で除外）
        { urlPattern: ({ url }) => url.origin === self.location.origin &&
            !/^\/(api|hook|ws|proxy)\//.test(url.pathname),
          handler: "StaleWhileRevalidate", options: { cacheName: "static" } },
      ],
      // 旧キャッシュ掃除
      cleanupOutdatedCaches: true,
    },
    // precompress(.gz) を出さない（§6.2: 現 serveStaticFile は .gz を扱わず二重圧縮の無駄）
  }),
],
```
- **`generateSW`（既定）を採用**。旧 `sw.js` の手書きロジックは Workbox の runtimeCaching へ写像済み（`injectManifest` は手書き SW を維持したい場合の選択肢だが、旧固定パス precache を捨てるので generateSW が素直）。
- **`/api`・`/hook`・`/ws`・`/proxy` のバイパスは必須**（旧 `sw.js:48-49` の除外を落とすと Cookie セッション前提の認証・動的データがキャッシュされ壊れる）。**否定条件 `!/^\/(api|hook|ws|proxy)\//.test(url.pathname)` を runtimeCaching の各 `urlPattern` 本体に直接埋め込む**（注記任せにせずコード例自体を安全なデフォルトにする）。`navigateFallbackDenylist` は navigateFallback にのみ効き runtimeCaching には効かない点に注意。旧 sw.js は `method !== 'GET'` を return し `/api/`・`/hook/` を除外していたが、Workbox の runtimeCaching は既定で GET のみ対象のため、この否定条件で等価になる。

  | 旧 sw.js の除外 | Workbox 側の等価対応 |
  |---|---|
  | `if (req.method !== 'GET') return;`（sw.js） | runtimeCaching は既定で GET のみキャッシュ |
  | `/api/`・`/hook/` を握らない（sw.js:48-49） | 各 urlPattern に `!/^\/(api|hook|ws|proxy)\//` を埋め込み |
  | `/ws`・`/proxy`（新規） | 同否定条件 + navigateFallbackDenylist |

- **注記(a) GSV 置換と precache の相互作用**: `index.html` を precache すると `server.ts:171-185` の GSV 実行時置換（`<!-- GOOGLE_SITE_VERIFICATION -->` → meta 置換）を SW キャッシュがバイパスし、config で GSV 設定してもキャッシュ版の未置換 index が返る恐れがある。`navigateFallback:"/index.html"` と相まって素の index が navigate に使われるため、**globPatterns から `html` を外し、`/` と `/index.html` は NetworkFirst の runtimeCaching のみに任せて precache しない**。完了条件に「GSV 置換が SW キャッシュ後も反映される」実機確認を追加する。
- **注記(b) precache と server gzip の関係**: SW precache は非圧縮のアセットを保持し、ネットワーク往復時のみ `server.ts` の gzip が効く。両者は二重圧縮でなく別レイヤだが、巨大な `index-*.js`（現状 entry）を SW が保持するとストレージを消費する点は認識しておく（P1b でチャンク分割すれば1個あたりは小さくなる）。

**P1a-3. `frontend/public/sw.js` は置かない**
`frontend/public/` に `sw.js` は既に無い（確認済み）。generateSW が `dist/public/sw.js` を生成する。**旧 `src/public/sw.js` は P5 まで残すが、それは `STATIC_DIR=src/public` ロールバック用**であり、`dist/public` 配信では新 SW が使われる。

**P1a-4. `main.ts` で SW 登録（現在コメントのみ, `main.ts:5`）**
```ts
// before: frontend/src/main.ts:5 — コメントのみ、登録なし
// Service Worker 登録は P1 (vite-plugin-pwa) で行う。旧 src/public/sw.js は参照しない。
// after: virtual モジュールで登録（inline script 不要 = CSP 準拠）
import { registerSW } from "virtual:pwa-register";
registerSW({ immediate: true });
```
`virtual:pwa-register` は Vite バンドルに含まれる `'self'` 由来 module のため CSP 適合。**型解決は reference directive で行う**: `frontend/src/vite-env.d.ts`（現状 `/// <reference types="svelte" />` と `/// <reference types="vite/client" />` のみ）に **`/// <reference types="vite-plugin-pwa/client" />` を1行追加**する。`frontend/tsconfig.json:15` の `types`（現状 `["svelte","vite/client","node"]`）に足す方式は module augmentation 型では効きにくいため reference directive を推奨。これを入れないと `pnpm typecheck:front`（svelte-check）が `virtual:pwa-register` を解決できず TS2307 で落ちる。

#### 完了条件（検証コマンド含む）
```bash
cd リポジトリルート && pnpm build:front
# (0) 型が通る（virtual:pwa-register 解決）
pnpm typecheck:front   # 0 errors（TS2307 が出ないこと）
# (a) sw.js が生成されている
test -f dist/public/sw.js && echo "sw.js OK"
# (b) CDN importScripts が無い（CSP 違反の混入なし）— 空であること
grep -n 'googleapis\|workbox-cdn\|importScripts.*http' dist/public/sw.js || echo "no CDN: OK"
# (c) precache manifest がハッシュ資産を参照（固定 /app.js を precache しない）
grep -o 'index-[A-Za-z0-9_-]*\.js' dist/public/sw.js | head
grep -c '/app.js\|/styles.css' dist/public/sw.js   # 期待: 0（旧固定パス不在）
# (d) index.html を precache していない（GSV 置換をバイパスしない）
grep -c 'index\.html' dist/public/sw.js            # precache manifest に無いこと（0 が望ましい）
```
- 実機（RV 環境）で、**既存 v10 SW 登録済みのウィンドウ**と**シークレット窓**の両方で:
  - 新デプロイ後、旧キャッシュが削除され（`cleanupOutdatedCaches` + autoUpdate の更新フロー）、新ハッシュ資産が取得され**白画面が出ない**こと（§13.3）。
  - `/api/*` が SW にキャッシュされない（Network で毎回 200、SW から返らない）こと。`/hook`・`/ws`・`/proxy` も同様。
  - **config で GSV を設定した状態で `/` を再読込し、SW キャッシュ後も `<meta name="google-site-verification">` が反映される**こと（注記(a)の検証）。
- CSP 検証: DevTools Console に `worker-src`/`script-src` 由来の SW 登録失敗が出ないこと。

#### 想定リスクと緩和
- **CDN importScripts が残り CSP 違反で SW 登録失敗** → `inlineWorkboxRuntime:true` + build 後 grep（上記 b）を CI/verify に固定。
- **旧 v10 SW 遷移期の白画面** → `index.html`/`sw.js` の no-cache 維持（`server.ts:158-160` で assets 以外は no-cache）で新 index.html が必ず network から取れる + autoUpdate の更新フローに一任（`skipWaiting`/`clientsClaim` の明示は避ける）。実機二窓検証を完了条件化。
- **GSV 未反映（precache バイパス）** → `html` を precache から外し `/`・`/index.html` を NetworkFirst のみに（注記(a)）。実機で置換反映を確認。
- **`/api` キャッシュ汚染** → runtimeCaching の各 `urlPattern` に否定条件を**コード本体で**埋め込む（P1a-2）。除外を落とさない。

#### ロールバック
`vite.config.ts` の VitePWA 追加と `main.ts` の登録行を revert すれば SW 生成が止まる。**ただし既にブラウザに新 SW が登録された後の撤回は難しい**ため、撤回時は「空の kill-switch SW（全 `caches` を削除し `self.registration.unregister()` する sw.js）」を配る手を用意しておく。基本は前方修正で対応。

---

### MCPV — BotMcp iframe の dev/prod 単独検証

#### 目的
`BotMcp` は iframe（`/api/mcp-servers/:id/dashboard` が独自 CSP で返す sandboxed opaque origin, `frontend/src/routes/BotMcp.svelte`）。iframe 内から `/proxy/mcp/:id/mcp` を叩くが、**この `/proxy` は Vite proxy に載せていない**（§5.6: SPA 本体の経路でなく iframe 内ドキュメント経由のため）。dev の Vite 前段で iframe が表示・動作するかは未検証。

#### 具体手順
- **dev（Vite 5173）**: mcp タブを開き、iframe が `/api/mcp-servers/:id/dashboard`（proxy 経由 `/api` に載っている）を読み込むか確認。iframe 内が `/proxy/mcp/:id/mcp` を叩く段は proxy 未定義のため 5173 で 404 になり得る → その場合 iframe 内 fetch が同一 origin(5173) に飛ぶかを DevTools で確認し、必要なら **dev のみ `/proxy` を暫定 proxy に追加するか、MCP は本番相当（RV-5 の `node dist/index.js`）でのみ検証する**方針を確定。
- **prod 相当（`STATIC_DIR=dist/public node dist/index.js`）**: 7854（config.yaml PORT）単独で iframe が dashboard を表示し、`/proxy/mcp` が同一 origin で解決すること。opaque origin sandbox 下で描画が壊れないこと。

#### 完了条件
- prod 相当で MCP タブの iframe が表示され、MCP ダッシュボードが操作できること。
- dev での可否を明文化（「dev では iframe 表示のみ / MCP 操作は prod 相当で検証」等、結論を docs に1行残す）。

#### 想定リスク / ロールバック
- リスク: `/proxy` が dev で不達 → dev では表示のみに割り切り、prod 相当検証を正とする（§5.6 の設計判断通り）。
- ロールバック: 検証のみ。コード変更なし。dev proxy に `/proxy` を足した場合はその1エントリを revert。

---

### Docker §7 — vite build ステージ組み込み

#### 目的
本番イメージ（`Dockerfile` stage2 node-builder）に `vite build` を追加し `dist/public/` を生成、runtime が新フロントを配信できるようにする。ビルド順序 `install → tsgo → vite build → prune` を厳守（prune 後だと vite/svelte が消えて失敗）。

#### 具体手順・差分

**Docker-1. 前提（lockfile）**
`vite`/`svelte`/`@sveltejs/vite-plugin-svelte`/`svelte-check`/`@tsconfig/svelte`/`concurrently` は既に devDeps にある（`package.json:60-75`）。P1a で `vite-plugin-pwa` を足したら `pnpm-lock.yaml` をコミット済みであること（`Dockerfile:66` は `--frozen-lockfile`）。

**Docker-2. `COPY frontend` を追加**（node-builder, `Dockerfile:68-70` 付近）
```dockerfile
# before: Dockerfile:68-70
COPY tsconfig.json ./
COPY src ./src
COPY docs ./docs
# after: frontend/ も取り込む（vite build の入力）
COPY tsconfig.json ./
COPY src ./src
COPY docs ./docs
COPY frontend ./frontend
```

**Docker-3. tsgo の後に vite build を挿入**（`Dockerfile:71-73` の RUN チェーン内、prune=`:82` より前）
```dockerfile
# before: Dockerfile:71-73
RUN pnpm exec tsgo \
 && mkdir -p dist/bin dist/assets \
 && cp -r src/assets/. dist/assets/
# after: tsgo → vite build → (assets コピー)。prune(:82) より前で実行される
RUN pnpm exec tsgo \
 && pnpm exec vite build --config frontend/vite.config.ts \
 && mkdir -p dist/bin dist/assets \
 && cp -r src/assets/. dist/assets/
```
- `vite build` の `outDir=../dist/public`（`vite.config.ts:14`）に出力。`vite build` の cwd と `--config` の解決に注意: `vite.config.ts` 内の `root`/`outDir` は config ファイルからの相対（`root=frontend`, `outDir=../dist/public` → `/app/dist/public`）になる想定。`--config frontend/vite.config.ts` で cwd=`/app` から実行しても config 基準で解決されることを、Docker-4 の完了条件（イメージ内 `ls /app/dist/public`）で必ず実測確認する。想定外の出力先になった場合は `RUN cd frontend && pnpm exec vite build` へ切り替える。
- `emptyOutDir` は `dist/public` 配下のみクリアし、`dist/` 直下の tsgo 出力（`dist/index.js` 等）や `dist/assets/`（`src/assets` コピー = @napi-rs/canvas 等サーバー資産）は消えない（§4 の衝突回避）。ただし vite build を tsgo の**後**に置くと `dist/assets` コピーは vite build の後段なので順序上も安全。
- `pnpm prune --prod`（`Dockerfile:82`）は vite build の**後**なので vite/svelte が生きている。順序 OK。

**Docker-4. runtime COPY**（`Dockerfile:104-106`）
```dockerfile
COPY --from=node-builder /app/dist ./dist          # dist/public(新フロント) + dist/assets を両方拾う（変更不要）
COPY --from=node-builder /app/src/public ./src/public   # 〜P5 は残す（STATIC_DIR=src/public ロールバック用）
COPY --from=node-builder /app/src/assets ./src/assets   # 常に残す（Vite と無関係のサーバー実行時資産）
```
- **`COPY /app/dist`（`:104`）は既に `dist/public` を内包して拾う**ため、vite 出力を runtime に運ぶ追加 COPY は不要。
- **P5 まで `COPY src/public`（`:105`）は残す**（1行のみ）。`COPY src/assets`（`:106`）は Vite の `/assets/` とは別物で**常に残す（不可侵）**。

**Docker-5. PUBLIC_DIR 既定**
runtime は WORKDIR `/app` で `node dist/index.js`（`Dockerfile:93,113`）→ cwd=/app、`server.ts:66-68` の既定 `dist/public` が解決する。移行期間中は compose/instance.env で `STATIC_DIR=dist/public` を明示注入（既定を src/public のままにしたい運用を選ぶ場合）か、既定のまま `dist/public` を使う。**P5 で `STATIC_DIR` を廃し既定一本化**。

#### 完了条件
```bash
cd リポジトリルート
deploy/instance.sh dev update            # dc build → up -d → health_check
# イメージ内の dist/public を確認
docker run --rm --entrypoint sh yuuka:latest -c 'ls -la /app/dist/public/assets | head; grep -c GOOGLE_SITE_VERIFICATION /app/dist/public/index.html; ls /app/dist/index.js /app/dist/assets'
```
- `docker build` が lockfile 不一致・prune 順序で失敗しないこと。
- イメージ内 `/app/dist/public/index.html` と `/app/dist/public/assets/*` が存在し、health_check（`instance.sh:68`）が HTTP 200。
- `/app/dist/index.js`（tsgo 出力）と `/app/dist/assets/`（サーバー資産）が vite build に消されず健在。

#### 想定リスク / 緩和
- **lockfile 不一致** → P1a 後に `pnpm-lock.yaml` コミット（Docker-1）。
- **prune 前 vite build を書き忘れ** → 順序を RUN 内 `&&` チェーンで固定（Docker-3）。
- **outDir 誤指定 / config 解決先ずれで tsgo 出力全消し or public 空** → `vite.config.ts:14` の `outDir=../dist/public` を触らず、Docker-4 完了条件でイメージ内実測（`ls /app/dist/public` と `ls /app/dist/index.js`）。§4 の厳守事項。

#### ロールバック
`Dockerfile` の差分（`COPY frontend` と `vite build` 行）を revert すれば旧イメージ挙動。デプロイ済みなら `deploy/instance.sh <inst> rollback`（`instance.sh:113-120`）で `yuuka:prev-<inst>` へ即戻し。

---

### Deploy §5.7 / §15.x — dev-hot コメント同期 & verify 強化

#### 目的
`docker-compose.dev-hot.yml` の旧前提コメント（「フロントは src/public をブラウザリロードで即反映」）を Svelte 化後の実態（案A: ホストで `pnpm dev:front` 併走）へ同期し、`deploy/instance.sh` の `health_check`（`:68-89`）に CSP/hashed/immutable の curl を追加（手動 DevTools 依存を排除）。

#### 具体手順・差分

**Deploy-1. `docker-compose.dev-hot.yml` コメント同期**（`:4-6`, `:30-32`）
```yaml
# before: docker-compose.dev-hot.yml:4-6
# TypeScript: tsx watch でファイル変更時に自動再起動
# Frontend (src/public/): ブラウザリロードで即反映（ビルド不要）
# Rust バイナリ: dist/bin/ を read-only マウント（事前 cargo build 済みを想定）
# after:
# TypeScript: tsx watch でファイル変更時に自動再起動
# Frontend: Svelte + Vite。このコンテナは API のみ担当。フロントはホストで
#           `pnpm dev:front`(Vite 5173) を併走させ、Vite proxy が /api・/ws/chat を
#           このコンテナの公開ポート(${HOST_PORT}=dev:7855) へ転送する（案A, 親書§5.7）。
#           dev API は BASE_URL 未設定(=http) 前提（§5.5: 未設定でないとセッション不達）。
# Rust バイナリ: イメージに内蔵（dist/bin/）。
```
```yaml
# before: docker-compose.dev-hot.yml:30-32
# ホットリロード: TypeScript ソース + フロントエンド (src/public/) をマウント
# Rust バイナリはイメージに焼き込み済みのため dist/bin/ マウント不要
- ./src:/app/src
# after:
# ホットリロード: TypeScript ソースをマウント（API のホットリロード用）。
# フロントは Vite(ホスト 5173) が担当するためここではマウントしない。
# src/public マウントは P5 まで残置可（STATIC_DIR=src/public ロールバック検証用）。
- ./src:/app/src
```
（`- ./src:/app/src` 自体は変更しない。src/public のホットリロードは P5 まで無害に残せる。コメントのみ同期。）

**Deploy-2. `deploy/instance.sh` の hot 説明同期**
同期対象は **`:11` のヘッダコメント**（hot の usage 説明）と、必要なら**末尾 `:163` の `usage:` echo**。`:124-125` は `hot)` の **case 分岐本体**（`:124`=case ラベル, `:125`=コメント）でありコード実体なので、コメント同期対象ではない（草案の「:124-125 付近の usage/help」は行番号ズレ）。
```bash
# before: instance.sh:11（ヘッダコメント）
#     hot       ホットリロードモード（tsx watch + src/ マウント。Rust はイメージに内蔵）
# after:
#     hot       ホットリロードモード（API=tsx watch + src/ マウント。Rust はイメージ内蔵）。
#               フロントはホストで `pnpm dev:front`(Vite 5173) を別途起動し proxy 経由で
#               このコンテナの /api・/ws/chat を叩く（親書§5.7 案A）。
```
（末尾 `:163` の `usage:` echo も1行に収まる範囲で「hot=API のみ / フロントは Vite 5173 併走」を追記してよい。`:124-125` の hot case 本体は触らない。）

**Deploy-3. verify に CSP/hashed/immutable curl 追加**（`health_check`, `instance.sh:68-89`）
`health_check` の HTTP 200 判定（`:83` の `if [ "$code" = "200" ]`）の成功ブランチ内に追記する。**既存 `:84` の `echo '✅ [$INST] デプロイ成功'` は新チェック群の最終行で置換**する（成功 echo を1行のみに保ち、二重出力を回避。health_check は成功ブランチが echo 1行のみで return せず関数末尾に落ちる構造のため、挿入位置を誤ると `✅ デプロイ成功` が二重に出る）。
```bash
# instance.sh:83 の `if [ "$code" = "200" ]; then` ブロック内。
# 既存 :84 の `echo '✅ [$INST] デプロイ成功'` を、下記チェック群 + 末尾の成功 echo で「置換」する。
if [ "$code" = "200" ]; then
  BASE="http://127.0.0.1:$port"
  # (a) CSP に script-src 'self'（curl -sI=HEAD。server.ts は method 非依存で writeHead:
  #     :222/:198 でヘッダを書くため HEAD でも CSP が返る）
  curl -sI "$BASE/" | grep -qi "content-security-policy:.*script-src 'self'" \
    || { echo "❌ CSP: script-src 'self' 不在"; return 1; }
  # (b) index.html が hashed module を参照（Vite 既定ハッシュ=8字ちょうど。
  #     server.ts:157 の immutable 正規表現と同一閾値で連動。hash 長を変えるなら両所同時更新）
  HTML="$(curl -s "$BASE/")"
  echo "$HTML" | grep -qE 'src="/assets/[^"]+-[A-Za-z0-9_-]{8,}\.js"' \
    || { echo "❌ hashed module 参照が無い"; return 1; }
  # (c) GSV meta（config 設定時のみ・警告に留める）
  echo "$HTML" | grep -q 'google-site-verification' || echo "⚠️  GSV 未注入（config 次第）"
  # (d) /assets/*.js が immutable
  ASSET="$(echo "$HTML" | grep -oE '/assets/[^\"]+\.js' | head -1)"
  curl -sI "$BASE$ASSET" | grep -qi "cache-control:.*immutable" \
    || { echo "❌ /assets/*.js が immutable でない"; return 1; }
  echo "✅ [$INST] デプロイ成功（CSP/hashed/immutable 検証済）"   # ← 旧 :84 の echo をこの1行で置換
else
  ...
fi
```
注意:
- この追加チェックは `/assets/*-hash.js` 参照を前提とするため、**新配信（`STATIC_DIR=dist/public`, Docker §7 完了後）でのみ緑**になる。旧 vanilla 配信（`STATIC_DIR=src/public`, `/app.js`）では (b)/(d) が false negative で落ちる。§2 の依存通り Docker §7 完了を有効化の前提とし、旧配信ロールバック中は追加チェックを一時 skip する（例: `[ "${STATIC_DIR:-dist/public}" = "src/public" ]` なら (b)(d) を警告に降格）ガードを入れると安全。
- GET/HEAD 混在は問題ない。health_check 本体は `curl -s -o /dev/null`（GET）で `$code` を得るが、(a)/(d) は `curl -sI`（HEAD）でヘッダを取る。`server.ts` が method 非依存でヘッダを `writeHead`（`:222`/`:198`）するため HEAD でも CSP/Cache-Control が返る。
- ハッシュ正規表現 `{8,}` は Vite 既定=8字ちょうどの下限ギリギリ。`server.ts:157` の immutable 判定と同一閾値で**連動**するため、hash 長を変えるなら RV-5 grep・この verify・`server.ts:157` を同時更新する（より頑健にするなら `{6,}` へ統一）。

#### 完了条件
```bash
deploy/instance.sh dev verify   # 200 + CSP/hashed/immutable が全て緑
```
- 新配信（Docker §7 完了後）で verify が緑。旧配信では該当チェックが警告降格 or skip で誤検知しないこと。
- 成功 echo が1行のみ（二重出力なし）。
- dev-hot コメントが実態と一致（レビューで確認）。

#### 想定リスク / ロールバック
- **verify が旧配信で常に落ちる** → Docker §7（新配信）完了後に verify 強化を有効化する順序を守る（§2 の依存）。旧配信ロールバック中の false negative は上記 STATIC_DIR ガードで回避。
- ロールバック: `instance.sh`/`compose` の差分 revert。挙動はコメントと verify チェックのみなのでロールバック容易。

---

### P5 — 旧 vanilla 資産の撤去

#### 目的
移行完了後、旧 vanilla 資産と `STATIC_DIR` ロールバック機構を撤去し `PUBLIC_DIR=dist/public` に一本化。

#### 撤去対象（正確な列挙）
```
削除するファイル（src/public 配下の旧 vanilla フルセット）:
  src/public/app.js
  src/public/styles.css
  src/public/index.html
  src/public/sw.js
  src/public/theme-init.js
  src/public/manifest.json
  src/public/404.html
  src/public/vendor/chart.umd.min.js
  src/public/icons/      （frontend/public/icons へ複製済みを確認後）
  src/public/materials/  （同上）
  → 実質 src/public ディレクトリごと撤去
```
```
Dockerfile から削除する行（1行のみ）:
  Dockerfile:105  COPY --from=node-builder /app/src/public ./src/public
```

#### 撤去してはいけない隣接（明示）
- **`Dockerfile:106` `COPY --from=node-builder /app/src/assets ./src/assets` は残す（不可侵）**。これは Vite の `/assets/` とは名前が近いだけの別物で、@napi-rs/canvas 等サーバー実行時資産。巻き添え削除すると canvas 描画等が壊れる（§7.3）。
- **`Dockerfile:72-73` の `mkdir -p dist/assets && cp -r src/assets/. dist/assets/` も残す**（`dist/assets` = サーバー資産）。
- `frontend/public/{theme-init.js,manifest.json,icons/,materials/,404.html}` は**新フロントの正**なので当然残す。

#### 手順
1. `frontend/public/` に icons/materials/theme-init.js/manifest.json/404.html が揃っていることを確認（済: `frontend/public/` に存在）。加えて `src/public/icons`・`src/public/materials` の内容が `frontend/public/` 側と一致することを `diff -r` で最終確認してから削除する。
2. `src/public/` 削除。
3. `Dockerfile:105` の `COPY src/public` 1行を削除。
4. `server.ts:66-68` の `STATIC_DIR` 分岐は**残してよい**（env が無ければ `dist/public` 既定で問題なし）が、運用上 `STATIC_DIR=src/public` ロールバックを廃止する旨を docs に明記。compose/instance.env から `STATIC_DIR=src/public` の注入があれば除去。§15.x verify の STATIC_DIR ガード（Deploy-3 注記）も、旧配信分岐が不要になるため撤去してよい。
5. 旧 SW kill: 新 SW（P1a）が `cleanupOutdatedCaches` で旧 v10 キャッシュを掃除するため、`src/public/sw.js` 削除後も新 SW が旧キャッシュを片付ける。

#### 完了条件
```bash
cd リポジトリルート
test ! -d src/public && echo "src/public removed"
grep -n 'COPY.*src/public' Dockerfile || echo "no src/public COPY"
grep -n 'COPY.*src/assets' Dockerfile && echo "src/assets COPY kept (OK)"
deploy/instance.sh dev update && deploy/instance.sh dev verify   # 新配信で verify 緑
```
- vanilla 資産ゼロ。`src/assets` COPY 健在。verify（§15.x）緑。

#### 想定リスク / ロールバック
- **P5 は不可逆に近い**（`STATIC_DIR=src/public` ロールバックを廃止するため）。→ **RV/P1a/P1b/MCPV/Docker/Deploy が全緑になるまで着手しない**。
- ロールバックは以降 **イメージロールバック（`deploy/instance.sh <inst> rollback` → `yuuka:prev-<inst>`, `instance.sh:113-120`）に一本化**。
- **注意（親書 §14 / MEMORY）**: 作業の引越しは move（develop から削除 + force-push）方針だが、`src/public` → `frontend/public` は**既に copy 済み**（ロールバック整合優先）。P5 はその copy 元を削除する最終工程であり、MEMORY の「copy は避け move」原則とは別レイヤ（ここは移行期ロールバック担保のための意図的な一時 copy の後始末）。

---

## 4. 全体マイルストーン表

| フェーズ | 主要変更ファイル | 完了条件（検証） | 独立ロールバック |
|---|---|---|---|
| **RV** | （検証のみ・コード変更なし） | `dev:all` 起動 / proxy 越しログイン→セッション維持 / 15タブ+全オーバーレイ smoke / `STATIC_DIR=dist/public node dist/index.js`（BASE=7854=config.yaml PORT）で CSP・hashed・immutable curl 緑 | ✅ 検証のみ。問題は該当箇所修正 or `STATIC_DIR=src/public` 退避 |
| **P1b 遅延ロード** | `BotShell.svelte`, `App.svelte`, `ui/LazyView.svelte` | `build:front` で各タブ/主要オーバーレイが独立チャンク（合格判定の主軸）/ 初期 entry が 354288 B から有意削減（100KB 超目安。絶対値 200KB は参考） / svelte-check 0 / タブ smoke 緑 | ✅ import 差分 revert で単一チャンクへ |
| **P1a SW** | `vite.config.ts`(VitePWA), `main.ts`(registerSW), `vite-env.d.ts`(reference), `package.json`+lock | `dist/public/sw.js` 生成 / typecheck:front 0 / CDN URL grep 0 / 固定 `/app.js` precache 不在 / index.html 非precache(GSV 反映) / 旧v10→新SW 実機二窓で白画面なし / `/api` 非キャッシュ。CSP は変更なし（worker-src 既存） | ⚠️ config/main revert 可だが登録済SW撤回は kill-switch SW 必要 |
| **MCPV** | （検証中心・dev proxy 追加は任意1エントリ） | prod 相当で MCP iframe 表示・操作可 / dev 可否を docs 明記 | ✅ 検証のみ |
| **Docker §7** | `Dockerfile`(`COPY frontend`,`vite build`) | `deploy dev update` で build 成功 / イメージ内 `dist/public/*` 存在 / `dist/index.js`・`dist/assets` 健在 / health 200 | ✅ Dockerfile revert or `deploy rollback` |
| **Deploy §5.7/§15.x** | `docker-compose.dev-hot.yml`, `deploy/instance.sh` | dev-hot コメント実態一致 / `deploy dev verify` で CSP・hashed・immutable 緑（新配信）・成功 echo 単一 | ✅ 差分 revert |
| **P5 撤去** | `src/public/*` 削除, `Dockerfile`(`COPY src/public` 1行削除) | src/public 不在 / `src/assets` COPY 健在 / verify 緑 | ⚠️ 以降イメージロールバック一本化（`deploy rollback`） |

---

## 5. 着手順チェックリスト（最初の1日 = RV から）

**Day 1 — RV（ゲート）**
1. [ ] dev API を **BASE_URL 未設定(http)** で起動（`deploy/instance.sh dev hot -d` or `env -u BASE_URL pnpm dev:host`）。DevTools で `yuuka-session`（`__Host-`無し）Cookie を確認（§5.5）。
2. [ ] `pnpm dev:all`（必要なら `VITE_API_TARGET` を実ポートへ: dev-hot=7855 / host-direct=7854）。5173 で FOUC 無く起動。
3. [ ] proxy 越しに login → 再読込でセッション維持（`/api/me` 200・`/login` に戻らない）・`/ws/chat` WS 接続。
4. [ ] Bot 選択 → 15タブ（tasks の chart.js 動的 import: `frontend/src/routes/tasks/GanttChart.svelte:42` 含む）+ 公開4ルート(未ログイン) + admin ガード + `/device?code=` を smoke。白画面/console error ゼロ。
5. [ ] `pnpm build:front` → `STATIC_DIR=dist/public node dist/index.js`（BASE=7854=config.yaml PORT）で `/`・`/assets/*.js`(immutable)・`theme-init.js`(no-cache)・CSP(script-src+worker-src)・GSV を curl 検証（RV-5）。
6. [ ] 失敗は §RV「切り分け手順」表で原因分類。**RV 全緑を確認してから次へ**。

**Day 2 以降（RV 緑後・P1a と P1b は並行可）**
7. [ ] P1b: `BotShell.svelte` の TAB_LOADERS 化 + `App.svelte` オーバーレイ遅延化（`LazyView` 経由）→ チャンク分離（主軸）& 初期JS有意削減を実測確認。
8. [ ] P1a: `pnpm add -D vite-plugin-pwa` + lockfile コミット → VitePWA(self-host, autoUpdate 単一責任, `/api`・`/hook`・`/ws`・`/proxy` を urlPattern 本体で bypass, html 非precache) + `vite-env.d.ts` に `vite-plugin-pwa/client` reference + `main.ts` registerSW → build 後 grep(CDN 0) + typecheck 0 + 旧v10→新SW 実機二窓検証 + GSV 反映確認。CSP は変更しない（worker-src 既存）。
9. [ ] MCPV: prod 相当で MCP iframe 検証、dev 可否を docs 明記。
10. [ ] Docker §7: `COPY frontend` + `tsgo && vite build`（prune 前）挿入 → `deploy dev update` でイメージ検証（`ls /app/dist/public` 実測）。
11. [ ] Deploy: dev-hot コメント同期（`:11` ヘッダ / `:163` usage、`:124-125` case 本体は不可侵）+ `instance.sh` verify に CSP/hashed/immutable curl 追加（既存 `:84` 成功 echo を置換・STATIC_DIR ガード付き）→ `deploy dev verify` 緑。
12. [ ] P5（全緑確認後・最後）: `diff -r` 確認 → `src/public` 撤去 + `Dockerfile` の `COPY src/public` 1行削除（`COPY src/assets` は残す）+ `STATIC_DIR` ロールバック廃止 → `deploy rollback` 一本化。

---

### 本書で参照した実ファイル（すべて絶対パス）
- `frontend/src/routes/BotShell.svelte`（15静的 import・`TAB_COMPONENTS`・動的 `<Current />` = §P1b 対象）
- `frontend/src/routes/tasks/GanttChart.svelte`（`:42` chart.js 動的 import。既に `chart-*.js` へ分離済み）
- `frontend/src/App.svelte`（オーバーレイ静的 import・`effectiveView` = §P1b 対象）
- `frontend/src/main.ts`（SW 登録未実装 = §P1a 対象）
- `frontend/src/vite-env.d.ts`（`vite-plugin-pwa/client` reference 追加先 = §P1a）
- `frontend/vite.config.ts`（proxy・outDir・VitePWA 追加先）
- `frontend/index.html`（GSVコメント・同期 theme-init.js）
- `src/server.ts`（`PUBLIC_DIR:66-68`・immutable `:154-160`/正規表現 `:157`・GSV `:171-185`・CSP `:80-81`（`worker-src 'self'`=`:81`）・method 非依存 writeHead `:198,:222`・gzip 経路 `:105-108` = 不変）
- `src/config.ts`（`:7` CONFIG_PATH=cwd/config.yaml・`:98` `port: getSetting('PORT','3000')` = 7854 の由来）
- `config.yaml`（`PORT: 7854` = host-direct のポート）
- `src/public/sw.js`（旧 v10・固定パス precache = 退役対象）
- `Dockerfile`（`:66` --frozen-lockfile / `:68-73` COPY frontend + vite build 挿入 / `:82` prune / `:104-106` COPY 調整）
- `docker-compose.dev-hot.yml`（`:4-6,30-32` コメント同期）
- `deploy/instance.sh`（`:11` ヘッダ / `:163` usage echo / `:124-125` hot case 本体=不可侵 / `:68-89` verify 強化・`:83` 200 判定・`:84` 成功 echo 置換）
- `deploy/dev/instance.env`（HOST_PORT=7855 = dev-hot コンテナ公開ポート）
- `package.json`（`:16-20` scripts / `:58-76` devDeps・要 `vite-plugin-pwa` 追加）
- `docs/svelte-migration-plan.md`（親書 §5-§7,§13,§14,§15）

現状実測: 初期 `dist/public/assets/index-*.js` = 354288 B、`chart-*.js` = 202212 B（`tasks/GanttChart.svelte:42` の動的 import で既に分離済み）、`index-*.css` = 104454 B。`vite-plugin-pwa` は未導入（P1a で追加）。`server.ts` の PUBLIC_DIR/immutable/GSV/CSP（`worker-src 'self'` 含む）は実装済み（P0 完了）。