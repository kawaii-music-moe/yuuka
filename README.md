# Yuuka — Discord Gemini Secretary Bot & Admin Dashboard

『Yuuka』は Google の **Gemini API** を活用した Discord 秘書ボットと Web 管理ダッシュボード（＋スマートフォン向け PWA）のハイブリッドな統合管理ソフトウェアです。ログインは OIDC（標準は sk-account）に一本化しています。全ユーザーデータは **Discord ユーザーID 単位で完全分離** され、Bot を共有しても会話履歴・家計・パスワード等が他ユーザーから見えることはありません。

タスク管理・予定（Google カレンダー同期）・家計・リマインド・ブラウザ自動操作・パスワードマネージャ・MCP 拡張などを自然な会話から扱えます。Web の PWA からはチャットに加えて、Irodori-TTS による音声での会話もできます。

## 構成

| 部分 | 場所 | 技術 |
|---|---|---|
| バックエンド（API・Discord Bot・cron） | `crates/`（`yuuka` バイナリ） | Rust（toolchain は `rust-toolchain.toml` で固定） |
| 管理画面（`/admin/`） | `frontend/` | Svelte + Vite（pnpm workspace） |
| PWA クライアント（`/`） | `clients/pwa/` | Vue + Vite（npm・pnpm workspace の外） |
| 音声生成サーバー（GPU の無い端末向け） | `clients/pwa/server/tts/` | Node + onnxruntime-node（`yuuka-tts.service`） |
| デスクトップクライアント | `clients/desktop/` | Rust / egui（開発中） |

- データは SQLite（`data/yuuka.db`）。Redis はセッションとキャッシュだけに使います。
- ログインは OIDC です。標準の認証サーバーは sk-account です。

## ドキュメント

| 目的 | ページ |
|---|---|
| できること一覧（機能） | [docs/guide/features.md](docs/guide/features.md) |
| セットアップ・設定 | [docs/guide/setup.md](docs/guide/setup.md) |
| dev / 本番の運用手順（systemd・Docker） | [deploy/README.md](deploy/README.md) |
| Docker での本番デプロイ・複数インスタンス運用 | [docs/guide/deployment.md](docs/guide/deployment.md) |
| 設計・仕様・アーキテクチャ（開発者 / AI 向け） | [docs/index.md](docs/index.md) |
| 開発の流れとルール | [CONTRIBUTING.md](CONTRIBUTING.md) |

## 開発

Node.js 24、pnpm 9、npm、Rust が必要です。作業ツリーごとに依存をインストールします。

```bash
pnpm install --frozen-lockfile
npm --prefix clients/pwa ci
pnpm dev:start  # dev API（yuuka-dev.service）と PWA・管理画面の Vite を起動
```

入口は `http://127.0.0.1:5173/`、管理画面は `/admin/` です（`/api` は dev API の `127.0.0.1:3403` へ中継）。
API が起動済みなら `pnpm dev` でフロントだけを前景で起動できます。
更新・停止・ログ・OIDC の登録は [dev 運用手順](deploy/README.md#dev-インスタンスsystemd現在のホスト) を参照してください。

## 本番

現在のホストの本番は Docker ではなく、systemd ユーザーサービスで動いています。

| サービス | 役割 |
|---|---|
| `yuuka.service` | 本番の API・Bot（`deploy/prod/run-yuuka.sh` → `deploy/prod/bin/yuuka`、`127.0.0.1:3402`） |
| `yuuka-dev.service` | dev の API・Bot（`deploy/dev/runtime`、`127.0.0.1:3403`） |
| `yuuka-redis.service` | 本番と dev で共有する Redis |
| `yuuka-tts.service` | 音声生成サーバー（`127.0.0.1:3410`） |

本番へは develop の最新版を次のスクリプトで反映します（main へは出しません）。

```bash
deploy/prod/deploy-develop.sh                    # origin/develop を fast-forward して本番へ
deploy/prod/deploy-develop.sh verify             # 確認だけ
deploy/prod/deploy-develop.sh rollback <退避先>  # 退避した状態へ戻す（DB も戻る）
```

退避先・確認の内容・音声モデル（`data/tts-models`）の配置は [deploy/README.md](deploy/README.md) を参照してください。
Docker で動かす場合（`pnpm run deploy`）の手順は [docs/guide/deployment.md](docs/guide/deployment.md) にあります。

## ライセンス・免責（ファンメイド作品）

本プロジェクトは、株式会社 Yostar および Nexon Games 社のゲーム『ブルーアーカイブ -Blue Archive-』の非公式ファンメイド作品です。キャラクター「早瀬ユウカ」、意匠、世界観等の著作権その他の知的財産権は、すべて原著作者（Nexon Games / Yostar 等）に帰属します。本プロジェクトは非営利目的のファン創作物であり、公式の二次創作ガイドラインに準拠して公開されています（v2 ではペルソナがユーザー設定制となり、デフォルトは汎用アシスタントです）。

プログラムコード自体は [MIT License](LICENSE) の下で公開されています。
