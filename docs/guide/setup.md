# セットアップ・設定（ローカル / 開発）

ローカルや開発環境で動かす手順です。本番は [本番デプロイ](deployment.md) を参照してください。

関連: [機能一覧](features.md) / [本番デプロイ](deployment.md)

---

## 動作要件

- **Rust ツールチェイン** — バックエンド本体（`crates/`）のビルド・起動に使用します。`rust-toolchain.toml` でバージョンを固定しているため、[rustup](https://rustup.rs/) を入れておけば `cargo` 実行時に自動で該当バージョンが解決されます。
- **Node.js 24 以上** / **pnpm 9** — 管理画面 SPA（`frontend/`）のビルド・開発用のみに使用します（バックエンドの実行には不要）。
- **Redis**（推奨・任意） — ログインセッションの保存に使用します（会話履歴の正本は SQLite）。未接続・到達不能でも起動は継続し、セッションは Cookie のみへ縮退します。
- **Chromium 実行環境**（任意） — ブラウザ自動操作ツール（`fetchDynamicPage` 等）用。`CHROME_EXECUTABLE_PATH` で明示するか、システムの `/usr/bin/chromium` 等が自動検出されます。未検出でもそれ以外の機能は動作します。同時に起動する Chromium の数は `YUUKA_BROWSER_MAX_PROCESSES` で制限できます（未設定なら上限なし）。

---

## 1. 依存関係のインストール

```bash
pnpm install
```

フロントエンド（Svelte/Vite）の依存関係のみをインストールします。バックエンド（Rust）の依存は `cargo build`/`cargo run` 時に取得されます。

> PWA クライアント（`clients/pwa/`）は npm で別管理です。`pnpm build:pwa` は依存が無ければ `npm ci` でインストールしてからビルドします。開発時の起動方法は [clients/pwa/README.md](../../clients/pwa/README.md) を参照してください。

## 2. 設定ファイルの作成

### `config.yaml`（一般設定）

テンプレートをコピーして作成します。git 管理外です。

```bash
cp example.yaml config.yaml
```

主な項目:

- **`DB_PATH`**: SQLite データベースの保存パス（デフォルト: `./data/yuuka.db`）。`DB_PATH` が指すファイルが無い状態で起動すると既定では即エラーになる。新規インスタンスの初回起動のみ、環境変数 `YUUKA_INIT_DB=1`（`.env` ではなく実際の環境変数として設定）を付けて起動すると、無ければ DB を新規作成して migrations を適用する（詳細は後述）。
- **`REDIS_URL`**: セッション管理用の Redis 接続 URL。
- **`PORT` / `HOST`**: バックエンドサーバーがリスンするポートとホスト名。未設定時はローカル接続のみ (`127.0.0.1` / `3000`)。`example.yaml` では `PORT: 7854` を明示しています。リバースプロキシ経由で公開する場合は `HOST` を `"0.0.0.0"` に変更します。
- **`GOOGLE_CLIENT_ID` / `GOOGLE_CLIENT_SECRET`**: Google OAuth 認証情報（カレンダー/Drive 連携用のシステム共通設定、任意）。
- **`BASE_URL`**: 外部公開 HTTPS ベース URL（OAuth リダイレクト・Cookie ハードニング判定に使用）。
- **`OIDC_*`**: ログインに使う OIDC プロバイダー。Yuuka はパスワードを持たず、本人認証は OIDC プロバイダーだけで行う。
  - `OIDC_ISSUER`: issuer（省略時は標準の sk-account `https://account.sk-9.net`）。discovery（`/.well-known/openid-configuration`）からエンドポイントを取る。
  - `OIDC_INTERNAL_URL`: サーバーからプロバイダーへ問い合わせる内部 URL（任意。同じマシンで動かすとき）。
  - `OIDC_CLIENT_ID`（既定 `yuuka`）/ `OIDC_CLIENT_SECRET`（必須。未設定だとログインできない）: プロバイダーに登録したクライアント。戻り先は `{BASE_URL}/api/auth/oidc/callback`、Back-Channel Logout の受け口は `/api/auth/oidc/backchannel-logout`。
  - `OIDC_SCOPE`（既定 `openid profile discord`）/ `OIDC_DISCORD_ID_CLAIM`（既定 `discord_id`）: Yuuka は利用者を Discord ID で識別するので、ID トークンのこの claim に Discord ID が必要。
  - `OIDC_LABEL`: ログインボタンの表示名（省略時は標準なら `sk-9 Account`、それ以外は issuer のホスト名）。
- **`ADMIN_DISCORD_IDS`**: 初回ログイン時に Admin として作成する Discord ユーザー ID（未設定なら全員 User。初回ログイン前に管理者の ID を指定）。

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
- systemd 等で運用する場合は `Environment=` で直接渡す方が確実です（`.env` の source 忘れを避けられます）。本番では `deploy/prod/secret.key` を `deploy/prod/run-yuuka.sh` が環境変数にします（[deployment.md](deployment.md) 参照）。

> **既存 DB を新しい版で開くとき（sk-account 移行の注意）**: マイグレーション V25 は、sk-account に紐付いていない利用者（`users.sk_account_id` が空）を**所有データごと削除**します。紐付けは V23 を含む版で各利用者が sk-account で一度ログインしたときに Discord ID で行われます。V22 以前の DB をいきなり現行版で起動すると V23〜V25 が続けて適用され、誰もログインしていないため全利用者が削除されます。旧 DB を移す前に必ずバックアップを取り、先に紐付けを済ませてください。

### 新規 DB の作成（`YUUKA_INIT_DB`）

Rust バックエンドは既定では**存在しない DB ファイルを新規作成しません**（`DB_PATH` の設定ミスで意図せず空 DB を作ってしまう事故を防ぐための安全策）。まっさらな環境で初めて起動する場合は、初回起動時のみ環境変数 `YUUKA_INIT_DB=1` を設定してください。DB ファイル（と親ディレクトリ）が無ければ新規作成し、マイグレーション（baseline を含む）を適用します。起動確認後は `YUUKA_INIT_DB` を外して（または未設定に戻して）再起動してください（既定 off を維持するため）。既存 DB がある場合、この設定は「無ければ作る」だけなので通常起動でデータが失われることはありません。

```bash
YUUKA_INIT_DB=1 cargo run --bin yuuka   # 初回のみ。DB（と data/ ディレクトリ）が無ければ作成してマイグレーションを適用する
# 起動を確認したら Ctrl-C で止め、YUUKA_INIT_DB を外して（unset するか .env から外して）再度起動する
```

[#55](https://github.com/kawaii-music-moe/yuuka/issues/55) の修正として [PR #64](https://github.com/kawaii-music-moe/yuuka/pull/64) で導入されました。

## 3. ビルド・起動

現在の systemd ホストでの起動は [deploy/README.md](../../deploy/README.md#dev) を参照。
`pnpm dev` は PWA と管理画面の両方を起動するため、初回は `npm --prefix clients/pwa ci` も実行する。
以下は API を自分で起動する場合の手順。

開発モード（バックエンド + フロントエンドを別々に起動、フロントエンドはホットリロード）:

```bash
# ターミナル1: バックエンド（config.yaml を読む cwd で実行すること）
cargo run --bin yuuka

# ターミナル2: フロントエンド（Vite）。config.yaml の PORT（example.yaml では 7854）へ proxy する
VITE_API_TARGET=http://127.0.0.1:7854 pnpm dev
```

> `pnpm dev` のデフォルト proxy 先は `http://127.0.0.1:3403` です。`cargo run` を直接使うローカル実行では、`config.yaml` の `PORT`（未設定時 `3000`、`example.yaml` では `7854`）に合わせて `VITE_API_TARGET` を明示してください。systemd の dev API（`yuuka-dev.service`）はこの既定の 3403 で待ち受けます。

プロダクション相当:

```bash
pnpm build              # cargo build --release --bin yuuka + 管理画面 SPA（dist/public）+ PWA（dist/public/pwa）
./target/release/yuuka  # config.yaml のある repo ルートで実行（dist/public と dist/public/pwa を静的配信）
```

> PWA のビルド成果物（`dist/public/pwa`）はリポジトリにコミットされません（#46）。`pnpm build` は `build:front`（`dist/public` を空にして管理画面をビルド）→ `build:pwa` の順で実行します。個別に実行する場合もこの順にしてください（逆順だと `dist/public/pwa` が消えます）。`dist/public/pwa` が無い場合、サーバーは警告を出して PWA の静的配信だけを無効化します（管理画面と API は動作）。

`example.yaml` の `PORT: 7854` を使う場合、起動後はブラウザで `http://localhost:7854/admin/`（管理画面。`/` は PWA）にアクセスします。

1. ログイン: ログイン画面のボタン（標準は「sk-9 Accountでログイン」。`OIDC_LABEL` で変更可）から OIDC プロバイダーでログインします。Yuuka 側でのアカウント登録はありません。初回ログイン前に `ADMIN_DISCORD_IDS` に管理者の Discord ID を設定してください。指定した ID の新規利用者だけが管理者になります。未設定なら、最初のログイン者も一般利用者になります。既存の権限は維持され、設定を後から追加しても既存利用者は自動昇格しません。既存の一般利用者を管理者にする場合は、既存管理者が管理画面から権限を変更してください。
2. デフォルト Bot のトークン設定: 管理者がシステムデフォルト Bot（使い方の案内役）のトークンを管理画面から設定します。
3. 個人設定: 各ユーザーは自分の Gemini API キー（必須）、Google OAuth 連携、ペルソナ等を管理画面から設定し、自分の Bot を作成します（ToDo・家計などの個人の機能は自分の Bot か共有された Bot で使います）。

### 外向き HTTP の接続制限

検索・定期報告（天気・RSS）・Discord のタイムライン保存候補の取得は、接続時に DNS 解決先を検査する共通クライアントを使います。内部 IP と IPv4-compatible・NAT64・6to4 の IPv6 範囲は拒否します。検査後の再解決を避けるため、この経路では `HTTP_PROXY` / `HTTPS_PROXY` 等の環境変数を使わず直接接続します。プロキシ経由でしか外部に接続できない環境では、これらの取得は失敗します。

### タイムゾーンの現在の制約

利用者設定に保存された `timezone` は、現在のバックエンドの日時計算には使われません。管理画面には項目を表示せず、通知先などの保存時にも送信しません。保存済みの値は DB に残りますが、利用者ごとの日時解釈・cron の実行時刻は変更されません。

バックエンドにはサーバーのローカル TZ を使うリマインド・cron と、JST 固定で解釈する予定・Google カレンダー変換が混在しています。現状の運用は systemd ユニット例と同じ `TZ=Asia/Tokyo` を前提にしてください。JST 以外のホストで同一の入力時刻が一致することは保証されません。管理画面のローカル日時入力は端末の TZ にも依存します。

既存 DB はオフセットなしの日時を保存しているため、稼働中のサーバーの TZ 変更にはデータ移行を含む検討が必要です。単に TZ を変更して既存データの問題が解消するわけではありません。バックエンド全体の統一は [#248](https://github.com/kawaii-music-moe/yuuka/issues/248) の継続課題です。
