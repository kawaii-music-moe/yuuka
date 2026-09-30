# セットアップ・設定（ローカル / 開発）

ローカルや開発環境で直接（Docker を使わずに）動かす手順です。本番運用は [Docker デプロイ](deployment.md) を推奨します。

関連: [機能一覧](features.md) / [Docker デプロイ](deployment.md)

---

## 動作要件

- **Rust ツールチェイン** — バックエンド本体（`crates/`）のビルド・起動に使用します。`rust-toolchain.toml` でバージョンを固定しているため、[rustup](https://rustup.rs/) を入れておけば `cargo` 実行時に自動で該当バージョンが解決されます。
- **Node.js 20 以上** / **pnpm 9** — 管理画面 SPA（`frontend/`）のビルド・開発用のみに使用します（バックエンドの実行には不要）。
- **Redis**（推奨・任意） — 会話コンテキスト・セッション管理に使用します。未接続・到達不能でも起動は継続し、セッションは Cookie のみへ縮退します。
- **Chromium 実行環境**（任意） — ブラウザ自動操作ツール（`fetchDynamicPage` 等）用。`CHROME_EXECUTABLE_PATH` で明示するか、システムの `/usr/bin/chromium` 等が自動検出されます。未検出でもそれ以外の機能は動作します。

---

## 1. 依存関係のインストール

```bash
pnpm install
```

フロントエンド（Svelte/Vite）の依存関係のみをインストールします。バックエンド（Rust）の依存は `cargo build`/`cargo run` 時に取得されます。

> PWA クライアント（`client/pwa/`）は別管理です。ビルド方法は [client/pwa/README.md](../../client/pwa/README.md) を参照してください（既知の問題: #45）。

## 2. 設定ファイルの作成

### `config.yaml`（一般設定）

テンプレートをコピーして作成します。git 管理外です。

```bash
cp example.yaml config.yaml
```

主な項目:

- **`INVITE_CODES`**: ユーザー登録に必須の招待コード。推測されにくい独自の値に変更してください。
- **`DB_PATH`**: SQLite データベースの保存パス（デフォルト: `./data/yuuka.db`）。`DB_PATH` が指すファイルが無い状態で起動すると既定では即エラーになる。新規インスタンスの初回起動のみ、環境変数 `YUUKA_INIT_DB=1`（`.env` ではなく実際の環境変数として設定）を付けて起動すると、無ければ DB を新規作成して migrations を適用する（詳細は後述）。
- **`REDIS_URL`**: 会話コンテキスト・セッション管理用の Redis 接続 URL。
- **`PORT` / `HOST`**: バックエンドサーバーがリスンするポートとホスト名。デフォルトはローカル接続のみ (`127.0.0.1` / `7854`)。リバースプロキシ経由で公開する場合は `HOST` を `"0.0.0.0"` に変更します。
- **`GOOGLE_CLIENT_ID` / `GOOGLE_CLIENT_SECRET`**: Google OAuth 認証情報（カレンダー/Drive 連携用のシステム共通設定、任意）。
- **`BASE_URL`**: 外部公開 HTTPS ベース URL（OAuth リダイレクト・Cookie ハードニング判定に使用）。
- **`ADMIN_DISCORD_IDS`**: 初期 Admin に昇格する Discord ユーザー ID（任意。未設定なら最初の登録者が Admin）。

### 環境変数（機密設定）

**重要:** バックエンド（Rust）は `.env` ファイルを自動読込しません（dotenv 非対応）。`config.yaml` に無いキーは、プロセスの**実環境変数**として渡す必要があります。[`.env.example`](../../.env.example) は変数のリファレンスとして使い、値は以下のいずれかの方法で実際の環境変数にしてください。

```bash
# 例1: .env をリファレンスとして作成し、シェルで export してから起動する
cp .env.example .env
# 値を編集した後、起動前に読み込む（.env は export 文を含まないため set -a が必要）
set -a && source .env && set +a
cargo run --bin yuuka

# 例2: 直接 export する
export YUUKA_ENCRYPTION_SECRET="$(openssl rand -base64 48)"
```

- **`YUUKA_ENCRYPTION_SECRET`** 【必須】: 保存時暗号化（API キー・トークン・パスワードマネージャ）のマスターシークレット。未設定では起動しません。十分に長いランダム文字列を設定してください（生成例: `openssl rand -base64 48`）。ローテーションは `YUUKA_ENCRYPTION_SECRET_NEW` を併設して起動（手順は `.env.example` 参照）。
- systemd 等で運用する場合は `Environment=` で直接渡す方が確実です（`.env` の source 忘れを避けられます）。Docker デプロイでは `deploy/<instance>/secret.key` から自動的に環境変数化されます（[deployment.md](deployment.md) 参照）。

> プレリリース版（v1 スキーマ）からの移行について: 現行バージョンはデータベーススキーマを全面再構築しています。旧スキーマを検出すると自動で再作成され、旧データは破棄されます。また、プレリリース版を `YUUKA_ENCRYPTION_SECRET` なしで運用していた場合は、`YUUKA_ENCRYPTION_SECRET_NEW` に新しい鍵を設定して一度起動すると、既存の暗号化データが新しい鍵で再暗号化されます。

### 新規 DB の作成（`YUUKA_INIT_DB`）

Rust バックエンドは既定では**存在しない DB ファイルを新規作成しません**（`DB_PATH` の設定ミスで意図せず空 DB を作ってしまう事故を防ぐための安全策）。まっさらな環境で初めて起動する場合は、初回起動時のみ環境変数 `YUUKA_INIT_DB=1` を設定してください。DB ファイル（と親ディレクトリ）が無ければ新規作成し、マイグレーション（baseline を含む）を適用します。起動確認後は `YUUKA_INIT_DB` を外して（または未設定に戻して）再起動してください（既定 off を維持するため）。既存 DB がある場合、この設定は「無ければ作る」だけなので通常起動でデータが失われることはありません。

```bash
YUUKA_INIT_DB=1 cargo run --bin yuuka   # 初回のみ。DB（と data/ ディレクトリ）が無ければ作成してマイグレーションを適用する
# 起動を確認したら Ctrl-C で止め、YUUKA_INIT_DB を外して（unset するか .env から外して）再度起動する
```

[#55](https://github.com/kawaii-music-moe/yuuka/issues/55) の修正として [PR #64](https://github.com/kawaii-music-moe/yuuka/pull/64) で導入されました。

## 3. ビルド・起動

開発モード（バックエンド + フロントエンドを別々に起動、双方ホットリロード）:

```bash
# ターミナル1: バックエンド（config.yaml を読む cwd で実行すること）
cargo run --bin yuuka

# ターミナル2: フロントエンド（Vite）。config.yaml の PORT（既定 7854）へ proxy する
VITE_API_TARGET=http://127.0.0.1:7854 pnpm dev
```

> `pnpm dev` のデフォルト proxy 先は `http://127.0.0.1:7855`（Docker dev インスタンス用）です。`cargo run` を直接使うローカル実行では、`config.yaml` の `PORT`（既定 `7854`）に合わせて `VITE_API_TARGET` を明示してください。

プロダクション相当（Docker を使わない場合）:

```bash
pnpm build              # cargo build --release --bin yuuka + フロントエンドの本番ビルド（dist/public）
./target/release/yuuka  # config.yaml のある repo ルートで実行（dist/public を静的配信）
```

起動後、ブラウザで `http://localhost:7854`（設定したポート）にアクセスします。

1. 初期セットアップ: 最初のユーザーとして管理者アカウントを登録します。
2. デフォルト Bot のトークン設定: システム全体のデフォルト Bot トークンを入力して起動します。
3. 個人設定: 各ユーザーは自分の Gemini API キー（必須）、Google OAuth 連携、ペルソナ等を管理画面から設定します。
