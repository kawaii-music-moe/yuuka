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

### ローカルのビルド制限

開発ホストのメモリを守るため、コンパイルを伴う操作は `develop` のみで実行します。`cargo check`・`cargo clippy`・テスト・フロントエンドのビルドも対象です。必ず `scripts/run-build.sh <command> [args...]` を使い、共有ロックと Rust 1ジョブを適用してください。作業ブランチでは検証をスキップした理由を記録し、GitHub-hosted CI に検証を任せます。制限回避のためのブランチ変更は禁止です。

隔離した GitHub-hosted CI の例外は [AGENTS.md](AGENTS.md) を参照してください。ローカルの制限は self-hosted runner にも適用します。

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

## PR の検証

作業ブランチでは差分の確認とコンパイルを伴わない整形確認を行います。コンパイルはローカルで実行せず、PR を作成して CI の結果を確認してください。develop で実行する場合も共通ランナーを使用します。

### CI

検証は [ci.yml](.github/workflows/ci.yml) に統合しました。develop / main 向けの全 PR（ドキュメントのみも含む）と両ブランチへの push で動きます。PR では先端だけでなく、ベースブランチとの仮マージコミットを検証します。

| 対象 | 毎回実施する検証 |
|---|---|
| Rust workspace | fmt、cargo-deny、clippy（全 target / feature）、test・build（全 feature、locked） |
| 管理画面 | 型検査、lint、テスト、ビルド |
| PWA | 型検査、テスト、配信ディレクトリへのビルド（管理画面の後） |
| dev スクリプト | 既存テスト |
| DB / 添付スキーマ | verify-sd07.py、verify-r13.mjs |
| デスクトップ | Linux で fmt と全 feature の clippy / test / build（audio を含む） |
| 検証制御 | 共通ランナーとゲートの回帰テスト |

各ビルドは同一ジョブで直列実行し、Rust は1ジョブです。デスクトップの Windows 固有コード・GUI 操作、実サービス連携、本番 release 最適化の検証はこの CI の保証範囲外です。必要な確認は PR に記録してください。CI に本番の秘密情報を渡さず、デプロイや本番DBの更新も行いません。

最後の必須チェック `pr-gate` は `validate` が success の場合だけ成功します。失敗・中断・スキップ・結果不明はマージ不可です。CI を通すためにチェックを無効化したり、失敗を無視したりしないでください。既存コードの問題で通らない場合は原因を Issue に記録し、解決までマージを止めます。

GitHub 側の保護設定と試運転手順は [PR 検証の運用](docs/guide/pr-validation.md) を参照してください。最新のベースブランチとの CI 成功と、CL による最新差分のレビューをそろえてからマージします。

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
scripts/run-build.sh pnpm build          # Rust バイナリ（target/release/yuuka）+ 管理画面 SPA（dist/public）+ PWA（dist/public/pwa）
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
