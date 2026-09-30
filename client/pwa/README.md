# Agent Desk PWA

Discord エージェント基盤（Yuuka）向けの Web コントロールパネルです。Vue 3 + TypeScript + Vite を使用し、PWA としてインストールできます。Yuuka のバックエンド（Rust・`crates/`）が `/` で配信し、`/api/client/*`（`crates/yuuka-client-api`）を利用します。

## Development commands

From the repository root:

```bash
pnpm dev:client:mock
```

Starts the Vue Client at `http://localhost:5173`, the mock API at
`http://localhost:8787`, and the shared-login admin dev server (`frontend`)
at `http://localhost:5174` pointed at the same mock API. The Vite proxy is
automatically configured for the mock API, and `/login`/`/admin` redirect to
the admin dev server (`http://localhost:5174/admin/...`; the admin SPA is only
served under `/admin/`). Unauthenticated users are sent to `/admin/login` with
a `returnTo` of the current page, and after signing in the admin dev server
redirects that path back to the Client. In mock mode sign in with
`admin` / `pass`.

```bash
pnpm dev
```

Starts just the shared-login/admin Svelte dev server (`frontend`) on its own,
at `http://localhost:5173/admin/` (see `frontend/vite.config.ts`; the app is
served under the `/admin/` base path). Use this when you
only need the admin UI and don't need the Client mock stack above.

## 起動（`client/pwa` 単体）

```bash
npm install
npm run mock  # 別のターミナルで起動: http://localhost:8787
npm run dev   # http://localhost:5173
```

開発サーバーは `/api` を `VITE_API_PROXY_TARGET`（未設定時は `http://localhost:3000`。`client/pwa/vite.config.ts`）へプロキシします。`npm run dev` 単体ではモックには接続しません。

- モックに接続する: `VITE_API_PROXY_TARGET=http://localhost:8787 npm run dev`（repo ルートの `pnpm dev:client:mock` はこれを自動設定します）。
- ローカルのバックエンド（`cargo run --bin yuuka`）に接続する: `config.yaml` の `PORT` に合わせて指定します（`example.yaml` の既定は 7854）。例: `VITE_API_PROXY_TARGET=http://localhost:7854 npm run dev`。
- 別ホストの実サーバーを検証する: `VITE_API_PROXY_TARGET=https://your-agent.example` を設定します。バックエンドに CORS 設定はなく、Cookie 認証の状態変更リクエストは CSRF チェック（`Origin`/`Referer` のホストが `BASE_URL` 由来の許可ホストと一致すること。`BASE_URL` 未設定時は localhost のみ）を受けます。

型チェックは `npm run check`（`vue-tsc --noEmit`）、本番ビルドは `npm run build`（出力は `client/pwa/dist`）です。

## API 境界

画面は `src/api/gateway.ts` の `AgentGateway` だけに依存します。HTTP の URL、リクエスト、レスポンスの詳細は `src/api/httpAdapter.ts` に閉じ込めています。実 API は主に `/api/client/*`（バックエンド: `crates/yuuka-client-api`）で、Google 連携の開始のみ `/api/settings/google/oauth/url` を使います。API が変わったらこのアダプタのみを変更してください。

`mock/server.ts` は同じ Gateway 契約を検証するためのインメモリ API です。状態はプロセス停止時に破棄されます。

## チャットと Markdown

チャットは `ChatMessage` と `ChatReference` を契約に持ちます。`references` を返すことで、メッセージから TODO、カレンダー、家計、共有ノートを参照カードとして開けます。実エージェントは関連レコードを生成・検索した後、対応する `href` を返してください。

表示は CommonMark / GFM の見出し、表、タスクリスト、リンク、打消し、脚注、コードブロックとシンタックスハイライトに対応しています。HTML は無効化し、レンダリング結果もサニタイズしています。

## ビルドと配信

repo ルートで `pnpm build:pwa` を実行すると、`client/pwa` をビルド（依存が無ければ `npm ci` を先に実行）し、出力を `dist/public/pwa` へ配置します。バックエンドは `dist/public/pwa` があれば PWA を `/` で配信します（`crates/yuuka-supervisor/src/main.rs`）。Docker イメージでは pwa-builder ステージが同じ場所へ配置します（[Dockerfile](../../Dockerfile)）。

## Git 運用

ブランチ運用はリポジトリ共通（[CONTRIBUTING.md](../../CONTRIBUTING.md)）に従います。日常の変更は `develop` から切った作業ブランチにコミットし、`develop` 向けに PR を出します。`main` はリリース時にのみ `develop` からマージします。機能単位で `develop` からブランチを切り、1 機能 1 コミットを目安にします。
