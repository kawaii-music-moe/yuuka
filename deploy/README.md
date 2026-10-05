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
    vite.compose.yml    dev フロント（PWA Client + 管理画面の Vite dev server・HMR）用 compose（API とは別プロジェクト yuuka-dev-vite）
    .env                vite.compose.yml のパラメータ（.env.example をコピー）※gitignore
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
deploy/instance.sh dev  up -d          # 開発インスタンス（ホスト :7856）
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

### DB は既定では起動前に必ず存在させる（P0-4 / issue #55）
Rust は**既定では存在しない DB を作らない**（`open_conn` は `SQLITE_OPEN_CREATE` を付けず、無ければ
即エラーで起動失敗）。`DATA_DIR` のパス誤設定で意図せず空 DB を作ってしまう事故を防ぐための挙動。
- 既存インスタンスの移行: 既存の `data/yuuka.db` をそのまま `DATA_DIR` に置く。
- **新規インスタンス**: 初回起動のみ `YUUKA_INIT_DB=1` を明示設定する（`instance.env` に追記、また
  は `docker run -e YUUKA_INIT_DB=1`）。`DATA_DIR/yuuka.db` が無ければ新規作成し、baseline（V17）
  から migrations を適用してすぐ使える状態にする（既存 DB があれば何もしない・上書きしない）。
  起動確認後は `instance.env` から `YUUKA_INIT_DB` を外して次回以降の起動に戻す（誤ってパスを
  変更した場合に空 DB を再生成させないため）。

### cron を回すプロセスは厳密に 1 つ（P0-2）
`Dockerfile` は `ENV YUUKA_RUST_CRON=1` を焼き込むため、**コンテナ内 Rust が cron の所有者**になる。
同一 `data/` に対する writer は 1 つでなければならない（SQLite WAL 単一ライター）。同じ `DATA_DIR` を
指すコンテナを複数同時に起動しないこと。

## dev インスタンス

### dev データの初期化

dev API を停止し、`deploy/dev/data` を空にしてから起動し直す場合は次を使う。

```bash
pnpm dev:reset                 # 既存の yuuka:dev イメージで起動（ビルドなし）
pnpm dev:reset -- --re-build   # イメージを --no-cache で再ビルドして起動
```

どちらも新しい空の SQLite DB を初期化する。再ビルド指定がないとき、`yuuka:dev` イメージが
存在しなければデータ削除前に終了する。prod のデータやイメージは操作しない。

### API（Rust バイナリ + SPA 配信）

dev は prod と同じ `docker-compose.yml` を使い、**インスタンス専用のイメージタグ**
（`deploy/dev/instance.env` の `YUUKA_IMAGE_TAG=dev` → `yuuka:dev`）で prod から分離される（#56 / #70）。
`update` / `rollback` は `yuuka:dev`・`yuuka:prev-dev` だけを触り、prod の `yuuka:latest` には影響しない。
そのため prod と同じ手順で更新できる:

```bash
deploy/instance.sh dev update      # yuuka:prev-dev へ退避 → ビルド → 再作成 → ヘルスチェック
deploy/instance.sh dev rollback    # 直前イメージへ戻す
deploy/instance.sh dev logs -f     # = pnpm run deploy:logs:dev
```

> **`YUUKA_IMAGE_TAG=dev` は必須。** 実際の `deploy/dev/instance.env`（gitignore 済）に無いと既定の
> `latest`（= prod と共有）になり、`dev update` が prod のタグを再ビルドしてしまう。
> `instance.env.example` からコピーした場合は入っている。古い `instance.env` は確認すること。
>
> 旧 `docker-compose.dev-rust.yml`（専用タグ `yuuka:dev-rust` を使う暫定オーバーレイ）は、タグ分離
> （#56 / #70）前の回避策で、現在は不要。`pnpm run deploy:dev` / `deploy:rollback:dev` は
> package.json から撤去済みなので、上記の `deploy/instance.sh dev …` を直接使う。

`HOST_PORT` は 7856 にする（5173・7855 は下記の Vite dev server が使う）。

### フロント（Vite dev server・HMR）— `deploy/dev/vite.compose.yml`

フロント開発用の Vite を Docker で常駐させる compose テンプレート（#57）。PWA Client
（`client/pwa`・`client` サービス）と管理画面（`frontend`・`vite` サービス）の 2 つの Vite を動かし、
外向きの入口は PWA Client の **5173 だけ**にまとめる（本番の Rust と同じく、同一オリジンの `/` に
PWA、`/admin` に管理画面が載る）。API コンテナ（`instance.sh dev …`）とは**別の compose
プロジェクト `yuuka-dev-vite`** なので、互いの `up` / `down` / `update` に影響しない。

```
ブラウザ / トンネル(公開ホスト) ─→ 127.0.0.1:5173 (client: PWA Client の Vite)
                                     ├─ /            PWA Client
                                     ├─ /admin/...  ─→ 127.0.0.1:7855 (vite: 管理画面の Vite・内部用)
                                     ├─ /login       → /admin/login へ redirect
                                     └─ /api        ─→ 127.0.0.1:7856 (dev API)
```

- `node:24-bookworm` + `corepack pnpm@9.15.0`（Dockerfile / CI と同じ。pnpm 11 系は lockfile 不一致になる）
- `network_mode: host`。Vite は既定で `127.0.0.1:5173`（PWA Client）と `127.0.0.1:7855`（管理画面）にのみ待受（外部公開はトンネル/プロキシ経由で 5173 へ）
- ソース（と `node_modules`）はホストのディレクトリを bind マウント。既定はこのリポジトリのルート
- 起動のたびに依存を入れてから Vite を起動（管理画面は `pnpm install --frozen-lockfile`、pnpm workspace 外の
  `client/pwa` は `npm install`。lockfile 不変なら即終了）
- 前提: dev API（`deploy/instance.sh dev update`）が `HOST_PORT`（7856）で起動していること

**パラメータ**（`deploy/dev/.env`。`cp deploy/dev/.env.example deploy/dev/.env` で作る。未設定なら既定値）:

| 変数 | 既定 | 意味 |
|---|---|---|
| `VITE_SRC_DIR` | `../..`（リポジトリルート） | Vite を動かすソースツリー。別 worktree / clone を使うときだけ絶対パスで指定 |
| `VITE_UID` / `VITE_GID` | `1000` / `1000` | コンテナ内の実行ユーザー。ソースツリーの所有者（`id -u` / `id -g`）に合わせる |
| `VITE_CLIENT_BIND_ADDR` / `VITE_CLIENT_PORT` | `127.0.0.1` / `5173` | 外向きの入口（PWA Client の Vite）の待受アドレス / ポート（`--strictPort`。使用中なら起動失敗） |
| `VITE_BIND_ADDR` / `VITE_PORT` | `127.0.0.1` / `7855` | 管理画面の Vite の待受アドレス / ポート（PWA Client からの proxy 先・内部用） |
| `VITE_API_TARGET` | `http://127.0.0.1:7856` | `/api`・`/ws/chat` の proxy 先（両 Vite 共通）。dev の `HOST_PORT` に合わせる |

Host ヘッダ検証は両 `vite.config.ts` の `server.allowedHosts: true` で外してある（dev server 専用）ため、
トンネルの公開ホスト名を設定する必要はない。

```bash
# 起動（初回）
cp deploy/dev/.env.example deploy/dev/.env      # 必要なら編集
docker compose -f deploy/dev/vite.compose.yml up -d
docker compose -f deploy/dev/vite.compose.yml logs -f client vite

# 更新: ソースの変更は HMR で即反映される（操作不要）。
#       pull 等で lockfile / 依存が変わったときだけ再起動する（起動時の install が走る）
docker compose -f deploy/dev/vite.compose.yml restart client vite

# 状態確認 / 停止
docker compose -f deploy/dev/vite.compose.yml ps
docker compose -f deploy/dev/vite.compose.yml stop     # 停止のみ（start / up -d で再開）
docker compose -f deploy/dev/vite.compose.yml down     # 停止・撤去
```

公開トンネルから使う場合は、`deploy/dev/config.yaml` の `BASE_URL` をブラウザで開く HTTPS URL
（例: `https://yuuka-dev.kawaii-music.moe`）に変更し、dev API を再起動する。
`BASE_URL: "http://localhost:5173"` のままでは、Cookie 認証の状態変更リクエストで
`Origin` が CSRF 許可ホストと一致せず 403 になる。ローカル専用利用なら既定値のままでよい。

`restart: unless-stopped` のため、ホスト / Docker デーモンの再起動後は自動で立ち上がる
（`stop` / `down` した場合は立ち上がらない）。トンネル（公開ホスト → `127.0.0.1:5173`）の設定は
このリポジトリの管理外。

> **デバッグ用の一時プロキシ**: API リクエストを覗きたいときは、リポジトリ外（または未コミットの
> 一時ファイル）で `127.0.0.1:7857` 等に待ち受けるリクエストロガー（リクエストを `7856` へ中継するだけの
> 小さな HTTP プロキシ。値は記録しない）を立て、`deploy/dev/.env` の `VITE_API_TARGET` を一時的にその
> ポートへ向けて `restart client vite` する。調査後は `VITE_API_TARGET` の行を消して（既定 7856 に戻る）
> `restart client vite`。恒久的なロガーはこのリポジトリには含めない（#57）。

ホストで直接 `pnpm dev`（Vite）を動かす場合も `VITE_API_TARGET` で proxy 先を指定する
（未指定の既定は `http://127.0.0.1:7855`。dev API へ向けるなら `VITE_API_TARGET=http://127.0.0.1:7856 pnpm dev`）。
