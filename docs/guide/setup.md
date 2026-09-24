# セットアップ・設定（ローカル / 開発）

ローカルや開発環境で直接（Docker を使わずに）動かす手順です。本番運用は [Docker デプロイ](deployment.md) を推奨します。

関連: [機能一覧](features.md) / [Docker デプロイ](deployment.md)

---

## 動作要件

- **Node.js 20 以上** / **pnpm**
- **Rust ツールチェイン (stable)** — 検索クローラー (`src/rust_crawler`) のビルドに `cargo` を使用します（[rustup](https://rustup.rs/) でインストール可）
- **Redis** — 会話コンテキスト・セッション管理に使用します
- **Chromium 実行環境** — ブラウザ自動操作用。`pnpm install` 時に Puppeteer が Chromium を自動ダウンロードします。ヘッドレスな Linux サーバーでは Chromium の依存共有ライブラリが別途必要になる場合があります（システムに `/usr/bin/chromium` 等があればそちらも自動検出されます）。

---

## 1. 依存関係のインストール

```bash
pnpm install
```

## 2. 設定ファイルの作成

テンプレートをコピーして `config.yaml`（一般設定）と `.env`（機密設定）を作成します。どちらも git 管理外です。

```bash
cp example.yaml config.yaml
cp .env.example .env
```

### `.env`（環境変数）

- **`YUUKA_ENCRYPTION_SECRET`** 【必須】: 保存時暗号化（API キー・トークン・パスワードマネージャ）のマスターシークレット。未設定では起動しません。十分に長いランダム文字列を設定してください（生成例: `openssl rand -base64 48`）。ローテーションは `YUUKA_ENCRYPTION_SECRET_NEW` を併設して起動（手順は `.env.example` 参照）。
- `.env` の代わりに、systemd の `Environment=` 等で直接環境変数として渡すこともできます。

### `config.yaml`（一般設定）の主な項目

- **`INVITE_CODES`**: ユーザー登録に必須の招待コード。推測されにくい独自の値に変更してください。
- **`DB_PATH`**: SQLite データベースの保存パス（デフォルト: `./data/yuuka.db`）。`DB_PATH` が指す
  ファイルが無い状態で起動すると既定では即エラーになる。新規インスタンスの初回起動のみ、環境変数
  `YUUKA_INIT_DB=1`（`.env` ではなく実際の環境変数として設定）を付けて起動すると、無ければ DB を
  新規作成して migrations を適用する。起動確認後は外しておく（パス誤設定での意図しない空 DB 生成
  を防ぐため・既定 off）。
- **`REDIS_URL`**: 会話コンテキスト・セッション管理用の Redis 接続 URL。
- **`PORT` / `HOST`**: 管理画面サーバーがリスンするポートとホスト名。デフォルトはローカル接続のみ (`127.0.0.1`)。リバースプロキシ経由で公開する場合は `"0.0.0.0"` に変更します。
- **`GOOGLE_CLIENT_ID` / `GOOGLE_CLIENT_SECRET`**: Google OAuth 認証情報（カレンダー/Drive 連携用のシステム共通設定、任意）。
- **`BASE_URL`**: 外部公開 HTTPS ベース URL（OAuth リダイレクト・Webhook URL の生成に使用）。
- **`ADMIN_DISCORD_IDS`**: 初期 Admin に昇格する Discord ユーザー ID（任意。未設定なら最初の登録者が Admin）。

> プレリリース版（v1 スキーマ）からの移行について: 現行バージョンはデータベーススキーマを全面再構築しています。旧スキーマを検出すると自動で再作成され、旧データは破棄されます。また、プレリリース版を `YUUKA_ENCRYPTION_SECRET` なしで運用していた場合は、`YUUKA_ENCRYPTION_SECRET_NEW` に新しい鍵を設定して一度起動すると、既存の暗号化データが新しい鍵で再暗号化されます。

## 3. ビルド・起動

開発モード（ホットリロード）:

```bash
pnpm dev
```

プロダクション（Docker を使わない場合）:

```bash
pnpm build   # Rust クローラーのビルド + アセットコピー + TypeScript コンパイル
pnpm start   # 本番サーバーの起動
```

起動後、ブラウザで `http://localhost:7854`（設定したポート）にアクセスします。

1. 初期セットアップ: 最初のユーザーとして管理者アカウントを登録します。
2. デフォルト Bot のトークン設定: システム全体のデフォルト Bot トークンを入力して起動します。
3. 個人設定: 各ユーザーは自分の Gemini API キー（必須）、Google OAuth 連携、ペルソナ等を管理画面から設定します。
