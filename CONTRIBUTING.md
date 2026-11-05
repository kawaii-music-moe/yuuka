# コントリビューションガイド

Yuuka への貢献ありがとうございます。このドキュメントは開発フローと最低限のルールをまとめた単一の参照点です。詳細なセットアップ・仕様は [`docs/index.md`](docs/index.md)、dev / 本番の運用手順は [`deploy/README.md`](deploy/README.md) を参照してください。

## 開発環境

- **Rust / cargo**（バックエンド本体。toolchain は `rust-toolchain.toml` で固定）
- **Node.js 24** + **pnpm 9**（管理画面 `frontend/`。pnpm 11 系は lockfile と合わない）
- **npm**（PWA クライアント `clients/pwa/`。pnpm workspace の外なので npm で扱う）

作業ツリーごとに依存をインストールし、dev を起動します。

```bash
pnpm install --frozen-lockfile
npm --prefix clients/pwa ci
pnpm dev:start      # dev API（yuuka-dev.service）+ PWA・管理画面の Vite（yuuka-dev-vite.service）
```

| URL | 中身 |
|---|---|
| `http://127.0.0.1:5173/` | PWA（Vue・HMR） |
| `http://127.0.0.1:5173/admin/` | 管理画面（Svelte・HMR、内部は `127.0.0.1:7855`） |
| `http://127.0.0.1:5173/api` | dev API（`127.0.0.1:3403`） |

- フロントの変更は HMR で反映されます。Rust の変更は `pnpm dev:update` でビルドして dev API を入れ替えます。
- 停止は `pnpm dev:stop`、ログは `pnpm dev:logs` / `pnpm dev:logs:vite`、一覧は `pnpm dev:help`。
- 別の作業ツリーへ切り替えるときは `pnpm dev:stop` してから、新しい作業ツリーで `pnpm dev:start` します。
- dev のログインは sk-account の OIDC（クライアント `yuuka-dev`）です。登録の仕方は [`deploy/README.md`](deploy/README.md) を参照してください。

設定（環境変数・config.yaml 等）の詳細は [`docs/guide/setup.md`](docs/guide/setup.md) を参照してください。

### ビルドはブランチを選んで行う

開発サーバーのメモリ（15 GB）は、複数の worktree で並列に Rust をビルドすると足りなくなります。そのため:

- 開発中の確認は `cargo check`（必要に応じて `cargo clippy` / `cargo fmt`）だけで行います。
- `cargo build` / `cargo test` / `cargo run` / `cargo nextest`、それらを呼ぶ `pnpm build` / `pnpm dev:update` などの実ビルドは、`develop` ブランチでの検証時だけ行います。
- Claude Code では `.claude/hooks/guard-cargo-build.sh` が develop 以外でのこれらのコマンドを止めます。

## ブランチ運用

- `develop` … 統合ブランチ。**機能・修正の PR はここへ向けて作成します。** 本番も develop から反映します。
- `main` … 旧リリースブランチ。現在は更新していません（本番への反映に main は使いません）。
- 作業ブランチ … `develop` から切り、用途に応じた接頭辞を付けます。
  - `feat/...` 新機能
  - `fix/...` バグ修正
  - `refactor/...` 挙動を変えないリファクタ
  - `docs/...` ドキュメント
  - `chore/...` / `ci/...` 雑務・CI

```bash
git fetch origin
git worktree add .claude/worktrees/something -b fix/something origin/develop
# ... 変更 ...
```

本体のチェックアウト（`/home/suki/apps/yuuka`）は develop のまま保ちます。本番の `yuuka.service` と dev の Vite がここを作業ディレクトリにしているためです。作業は worktree で行ってください。

## コミット前のチェック

PR を出す前に、作業ブランチで以下が通ることを確認してください。

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
pnpm check                      # = pnpm typecheck:front && pnpm lint
npm --prefix clients/pwa run check
```

- `pnpm typecheck:front` … `svelte-check`（型エラー 0）
- `pnpm lint` … `biome check frontend/src scripts`（`pnpm lint:fix` で自動修正）
- `npm --prefix clients/pwa run check` … `vue-tsc --noEmit`

テスト（`cargo test --workspace`・`pnpm test:front`・`npm --prefix clients/pwa test`）は develop での検証時に実行します。

### CI

| ワークフロー | PR で毎回 | develop → main の PR だけ |
|---|---|---|
| `ci.yml` | `svelte-check`・`biome`・`vue-tsc` | 管理画面・PWA のビルド、PWA のテスト |
| `rust-ci.yml`（Rust 関係のファイルを変えたとき） | `cargo fmt --check`・cargo-deny・マイグレーション回帰（`verify-sd07.py` / `verify-r13.mjs`） | clippy・release ビルド・`cargo test` |

clippy とテストは CI では develop → main の PR でしか走らないので、ローカルで clippy を通し、develop で `cargo test` を確認してください。

### コミットメッセージ

[Conventional Commits](https://www.conventionalcommits.org/) 風（`feat:` / `fix:` / `docs:` / `refactor:` / `chore:` / `ci:`、例: `feat(discord): ...`）にします。関連 issue は本文で `Closes #123` のように紐付けてください。

## Pull Request

1. `develop` をベースに PR を作成します（テンプレートが自動挿入されます）。
2. テンプレートの **変更概要 / テスト / デプロイ影響** を埋めてください。
3. 上のチェックが通ること、秘密情報を含めていないことを確認します。
4. 画面（UI）が変わる PR は、その旨を書いて確認を待ちます。
5. レビュー後にマージします（merge commit）。

## ビルドとデプロイ

```bash
pnpm build          # Rust バイナリ（target/release/yuuka）+ 管理画面 SPA（dist/public）+ PWA（dist/public/pwa）
# 個別に実行する場合は build:front → build:pwa の順（build:front は dist/public を空にするため、
# 先に build:pwa を実行すると dist/public/pwa が消える）。PWA のビルド成果物はコミットしない（#46）。
```

本番（systemd の `yuuka.service`）へは、develop にマージした後に本体のチェックアウトで反映します。

```bash
pnpm run deploy                       # origin/develop を fast-forward → ビルド → 退避 → 入れ替え → 確認
pnpm run deploy:verify                # 確認だけ
pnpm run deploy:rollback -- <退避先>
```

- DB マイグレーションは起動時に自動で適用されます。確認に失敗しても自動では戻らないので、表示される `rollback` で戻すかを判断します。
- 音声モデル（`data/tts-models`）はリポジトリに含めず、手で配置します（[`tools/irodori-onnx/README.md`](tools/irodori-onnx/README.md)）。
- 新しく本番を立てる手順は [`docs/guide/deployment.md`](docs/guide/deployment.md) を参照してください。

## セキュリティ

脆弱性は公開 issue ではなく、[セキュリティアドバイザリ](https://github.com/kawaii-music-moe/yuuka/security/advisories/new) で非公開に報告してください。トークン・APIキー・個人情報をコミットやログ、issue/PR に含めないでください。
