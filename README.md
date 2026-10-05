# Yuuka — Discord Gemini Secretary Bot & Admin Dashboard

『Yuuka』は Google の **Gemini API** を活用した Discord 秘書ボットと Web 管理ダッシュボードのハイブリッドな統合管理ソフトウェアです。全ユーザーデータは **Discord ユーザーID 単位で完全分離** され、Bot を共有しても会話履歴・家計・パスワード等が他ユーザーから見えることはありません。

タスク管理・予定（Google カレンダー同期）・家計・リマインド・ブラウザ自動操作・パスワードマネージャ・MCP 拡張などを自然な会話から扱えます。

## ドキュメント

| 目的 | ページ |
|---|---|
| できること一覧（機能） | [docs/guide/features.md](docs/guide/features.md) |
| セットアップ・設定（ローカル / 開発） | [docs/guide/setup.md](docs/guide/setup.md) |
| Docker 本番デプロイ・複数インスタンス運用 | [docs/guide/deployment.md](docs/guide/deployment.md) |
| 設計・仕様・アーキテクチャ（開発者 / AI 向け） | [docs/index.md](docs/index.md) |

## クイックスタート（Docker）

```bash
# 1. 設定を用意（インスタンスごと。詳細は deployment.md）
cp deploy/prod/instance.env.example deploy/prod/instance.env
cp deploy/prod/config.yaml.example  deploy/prod/config.yaml
openssl rand -base64 48 | tr -d '\n' > deploy/prod/secret.key && chmod 600 deploy/prod/secret.key
# deploy/prod/instance.env と config.yaml を環境に合わせて編集

# 2. ビルドして起動（以後の更新もこれだけ）
pnpm run deploy
```

Docker を使わないローカル実行は [docs/guide/setup.md](docs/guide/setup.md) を参照してください。

## ライセンス・免責（ファンメイド作品）

本プロジェクトは、株式会社 Yostar および Nexon Games 社のゲーム『ブルーアーカイブ -Blue Archive-』の非公式ファンメイド作品です。キャラクター「早瀬ユウカ」、意匠、世界観等の著作権その他の知的財産権は、すべて原著作者（Nexon Games / Yostar 等）に帰属します。本プロジェクトは非営利目的のファン創作物であり、公式の二次創作ガイドラインに準拠して公開されています（v2 ではペルソナがユーザー設定制となり、デフォルトは汎用アシスタントです）。

プログラムコード自体は [MIT License](LICENSE) の下で公開されています。
