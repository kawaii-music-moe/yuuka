# Yuuka Docker デプロイ / 複数インスタンス運用

同一ホスト上で Yuuka を **複数インスタンス**（本番 `prod` / 開発 `dev` / 任意の `<name>`）として
互いに隔離して起動するための Docker 構成です。1つの `Dockerfile` / `docker-compose.yml` を
インスタンスごとの設定で使い回します。

## 構成

```
Dockerfile              本番スリム（rust-builder → frontend-builder → debian-slim runtime。成果物=Rust バイナリ + SPA のみ・Node/chromium/node_modules 非同梱）
docker-compose.yml      パラメータ化された app + redis（インスタンス専用 redis を同梱）
deploy/
  instance.sh           インスタンス操作ヘルパー
  prod/
    instance.env        非機密の起動パラメータ（ポート/データパス/プロジェクト名 …）
    secret.key          暗号化シークレット（YUUKA_ENCRYPTION_SECRET）※gitignore
    config.yaml         システム共通設定（:ro マウント）※gitignore
  dev/
    instance.env / secret.key / config.yaml / data/
```

### 隔離のしくみ
- **プロジェクト名**（`COMPOSE_PROJECT_NAME`）でコンテナ/ネットワーク/ボリュームを名前空間分離
- **データ**: `DATA_DIR` を `/app/data` に bind マウント（DB・スクショ・ブラウザプロファイル等は全てこの下）
- **Redis**: インスタンスごとに専用コンテナ（キャッシュ専用・非永続。落ちても SQLite から再構築）
- **ポート**: `HOST_PORT` をホスト側 `127.0.0.1` に公開（外部公開はリバースプロキシ/トンネル経由）
- **シークレット**: `secret.key` をシェル経由で literal に渡す（`$` 等を含むため compose 変数展開を回避）

## 更新・デプロイ（推奨）

コードを変更したら **`pnpm run deploy`** だけでOK。内部で「現行イメージ退避 → ビルド
（Rust バイナリ + SPA）→ コンテナ再作成 → ヘルスチェック（HTTP/Botログイン/エラー件数）」を実行する。
DB マイグレーションは起動時に自動適用される。

```bash
pnpm run deploy            # 本番(prod)をビルドして反映（= deploy/instance.sh prod update）
pnpm run deploy:rollback   # 直前イメージ(yuuka:prev-prod)へ戻す
pnpm run deploy:verify     # ヘルスチェックのみ
pnpm run deploy:logs       # 本番ログ追従
pnpm run deploy:ps         # 状態確認
pnpm run deploy:down       # 本番停止・撤去
```
> 注: `pnpm deploy`（`run` 無し）は pnpm 組込みコマンドと衝突するため、必ず `pnpm run deploy` を使う。

## 使い方（個別コマンド）

```bash
deploy/instance.sh prod update         # = pnpm run deploy（推奨デプロイ）
deploy/instance.sh prod rollback       # 直前イメージへ戻す
deploy/instance.sh prod verify         # ヘルスチェックのみ

# 素の docker compose サブコマンドにも委譲できる
deploy/instance.sh prod build
deploy/instance.sh prod up -d
deploy/instance.sh prod logs -f
deploy/instance.sh prod ps
deploy/instance.sh prod down
deploy/instance.sh dev  up -d          # 開発インスタンス（ホスト :7855）
```

## 新しいインスタンスを追加する
1. `deploy/<name>/` を作成し `instance.env` / `config.yaml` / `secret.key` を用意
   （`COMPOSE_PROJECT_NAME` と `HOST_PORT` は他と重複させない）
2. `openssl rand -base64 48 | tr -d '\n' > deploy/<name>/secret.key && chmod 600 …`
3. `deploy/instance.sh <name> up -d`

## 暗号化シークレットのローテーション
1. `deploy/<name>/secret.key.new` に新しい鍵を置く（instance.sh が `YUUKA_ENCRYPTION_SECRET_NEW` として渡す）
2. `deploy/instance.sh <name> up -d` → 起動時に全暗号化データが新鍵で再暗号化される
3. 完了後 `secret.key` を新鍵に置き換え、`secret.key.new` を削除して再起動

## 運用注意（Rust 直起動）

本番イメージは Rust バイナリ単独（CMD `./yuuka`）。旧 Node 実装・systemd 運用・nginx strangler・
カットオーバースクリプトは撤去済み（最後に残っていたコミット: `bb97bae`）。

### DB は起動前に必ず存在させる（P0-4）
Rust は**存在しない DB を作らない**（`open_conn` は `SQLITE_OPEN_CREATE` を付けず、無ければ即エラー
で起動失敗）。まっさらなインスタンスを立ち上げる前に `DATA_DIR/yuuka.db` を用意する必要がある。
Node 撤去により「一度 Node で起動して DB を生成」する手順は使えないため、当面は `bb97bae` を
別 worktree にチェックアウトして旧 Node で DB を生成する（Rust 側での新規 DB 作成は #55）。

### cron を回すプロセスは厳密に 1 つ（P0-2）
`Dockerfile` は `ENV YUUKA_RUST_CRON=1` を焼き込むため、**コンテナ内 Rust が cron の所有者**になる。
同一 `data/` に対する writer は 1 つでなければならない（SQLite WAL 単一ライター）。同じ `DATA_DIR` を
指すコンテナを複数同時に起動しないこと。

## dev インスタンス — prod タグ共有に注意

> **⚠️ `deploy/instance.sh dev update`（`pnpm run deploy:dev`）は dev では使わない。**
> `docker-compose.yml` は全インスタンス共有で `image: yuuka:latest` を使う（prod もこのタグ）。
> `dev update` はこの **yuuka:latest を再ビルド**するため、prod の次回再作成時に dev ビルドへ
> 差し替わる（恒久対応は #56）。

dev は隔離オーバーレイ `docker-compose.dev-rust.yml`（`image: yuuka:dev-rust`）を重ねて起動する:

```bash
docker build -t yuuka:dev-rust -f Dockerfile .
export YUUKA_ENCRYPTION_SECRET="$(cat deploy/dev/secret.key)"
docker compose -p yuuka-dev --env-file deploy/dev/instance.env \
  -f docker-compose.yml -f docker-compose.dev-rust.yml up -d
```

フロント開発はホストで `pnpm dev`（Vite）を起動し、`VITE_API_TARGET` で dev API（`HOST_PORT`）へ proxy する。
