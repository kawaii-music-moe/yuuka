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

### 現在のホスト（systemd・develop から直接デプロイ）

このホストの本番は Docker ではなく systemd ユーザーサービス `yuuka.service`（作業ディレクトリは本体の
チェックアウト `/home/suki/apps/yuuka`・`deploy/prod/run-yuuka.sh` → `deploy/prod/bin/yuuka`）で動いている。
main には出さず、develop の最新版を次のスクリプトで本番へ反映する。

```bash
deploy/prod/deploy-develop.sh                    # origin/develop を fast-forward して本番へ
deploy/prod/deploy-develop.sh verify             # 確認だけ（画面・音声・CSP・マイグレーション・Discord）
deploy/prod/deploy-develop.sh rollback <退避先>  # 退避した状態へ戻す（DB も戻る）
```

- 本体のチェックアウトが develop で、追跡中のファイルに未コミットの変更が無いこと。
- ビルド（Rust・管理画面・PWA）は配信中の `dist/public` に書かず別の場所で作り、サービスを止めている数秒の
  間に入れ替える。止めている間に、それまでのバイナリ・画面・DB（WAL ごと）・設定を
  `deploy/prod/backups/<時刻>-develop-<sha>/` へ退避し、`deployment.json` に記録する。
- 確認に失敗しても自動では戻さない（マイグレーション後に古いバイナリへ戻すには DB も戻す必要があり、デプロイ後の
  書き込みが消えるため）。表示される `rollback` コマンドで戻すかを判断する。

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

### 音声モデル（PWA の音声チャット）
PWA の「音声」画面は、返答の読み上げを端末の GPU で Irodori-TTS を動かして生成する。モデル一式（約 1.7 GB）は
作業ディレクトリの `data/tts-models/<バージョン>/` に置くと `/tts-models/*` として配信される（無ければ配信せず、
PWA はブラウザ標準の読み上げに切り替える）。作り方と配置は [tools/irodori-onnx/README.md](../tools/irodori-onnx/README.md)
の `package.py` を参照（`--dest <実行ディレクトリ>/data/tts-models`）。リポジトリにもイメージにも含めないので、
インスタンスごとに配置する。既存バージョンのファイルは書き換えない（`immutable` で配信している）。
利用者が作った声（話者条件。サンプルの音声そのものではない）は DB の `client_voices`（V36）に本人のものとして
保存され、`/api/client/voices` で本人だけが読み書きする。

GPU（WebGPU）の無い端末のために、サーバーでも音声を生成できる（`clients/pwa/server/tts`・onnxruntime-node の CPU で
同じ推論コードを動かす）。音声生成サーバー `yuuka-tts.service`（[deploy/tts/yuuka-tts.service](tts/yuuka-tts.service)）を
本番と dev で 1 つ動かし、両方の config.yaml に `TTS_SERVER_URL: "http://127.0.0.1:3410"` を書くと、yuuka が
`/api/client/tts/*` をログイン済みの要求だけ中継する（未設定なら PWA は標準の読み上げ）。生成中は約 3.7 GB を使い、
10 分使われなければモデルを解放する。要求は 1 件ずつ順に処理する（1 文の生成で CPU をほぼ使い切るため）。

音声生成サーバーは自分では利用者を確かめない。別のマシンに置くとき（`TTS_HOST` を 127.0.0.1 以外にするとき）は、
yuuka からの要求に Ed25519 の署名を付けて、音声生成サーバーで確かめる（同じホストでも使える）。

1. 鍵を作る（本番と dev で別々に）: `cd clients/pwa && npx tsx server/tts/keygen.ts`
2. 秘密鍵を yuuka の config.yaml の `TTS_SIGNING_KEY` に書く（人に見せない）。
3. 公開鍵を `yuuka-tts.service` の `TTS_VERIFY_KEYS` に書く（本番と dev の分をカンマで並べる）。
4. yuuka を先に再起動してから、音声生成サーバーを再起動する（鍵の無い音声生成サーバーは署名を無視して受け付けるので、
   この順なら止めずに切り替えられる）。

署名するのは メソッド・パス・時刻・nonce・本文の SHA-256 で、音声生成サーバーは署名の無い要求（本文は読まない）・
時刻が 30 秒を超えてずれた要求・一度使った nonce を 401 で断る。2 台の時計は NTP で合わせておく。署名するパスは
`/speak` などなので、`TTS_SERVER_URL` にはパスを付けない（`http://<ホスト>:3410`）。
`TTS_VERIFY_KEYS` が無いまま 127.0.0.1 以外で待ち受けようとすると、音声生成サーバーは起動しない。
署名では通信は暗号化されない（返答の文章が平文で流れる）ので、LAN の外を通すなら WireGuard などで包む。

### cron を回すプロセスは厳密に 1 つ（P0-2）
`Dockerfile` は `ENV YUUKA_RUST_CRON=1` を焼き込むため、**コンテナ内 Rust が cron の所有者**になる。
同一 `data/` に対する writer は 1 つでなければならない（SQLite WAL 単一ライター）。同じ `DATA_DIR` を
指すコンテナを複数同時に起動しないこと。

## dev インスタンス（systemd・現在のホスト）

現在のホストでは Docker を使わず、ユーザー systemd の `yuuka-dev.service` が
`127.0.0.1:3403` で Rust API を実行する。実行ディレクトリは
`/home/suki/apps/yuuka/deploy/dev/runtime` で、`.env`・`config.yaml`・DB・
`bin/yuuka` を持つ。`yuuka-redis.service` は本番と共有している。
フロントは開発中の作業ツリーから Vite で配信する。

### 初回準備と起動

Node.js 24 と pnpm、npm が必要。作業ツリーごとに依存をインストールする。

```bash
pnpm install --frozen-lockfile
npm --prefix clients/pwa ci
pnpm dev:start
```

`dev:start` は既存の `yuuka-dev.service` を起動し、同じユーザーの一時的な
`yuuka-dev-vite.service` で PWA と管理画面の Vite を常駐させる。
起動済みなら再起動しない。別の作業ツリーに切り替えるときは
`pnpm dev:stop` 後、新しい作業ツリーで `pnpm dev:start` を実行する。
新しいホストでは事前に dev API の systemd ユニット・設定・DB を用意する必要がある。

API が起動済みなら `pnpm dev` でも両方の Vite を前景で起動できる。
Ctrl-C で両方停止する。`pnpm dev:admin` は管理画面だけを起動する。
前景とバックグラウンドを同時には起動しない（ポート競合はエラーになる）。

```text
http://127.0.0.1:5173/          PWA（Vue・HMR）
  /admin/  -> 127.0.0.1:7855  管理画面（Svelte・HMR）
  /api     -> 127.0.0.1:3403  dev API
```

`VITE_API_TARGET`（既定 `http://127.0.0.1:3403`）、`VITE_PORT`（7855）、
`VITE_CLIENT_PORT`（5173）、`VITE_BIND_ADDR`（127.0.0.1）を環境変数で変更できる。
フロントの変更は HMR で反映される。Rust は自動再ビルドされない。
API の 3403 番ポートへ直接アクセスすると、runtime に配置されたビルド済み画面が返る。
HMR を使うブラウザやリバースプロキシは 5173 番へ接続する。
ローカルでは `127.0.0.1` の URL を使う。別ホスト名でアクセスする場合は、runtime の
`config.yaml` の `BASE_URL` をブラウザで開く URL に合わせて dev API を再起動する
（Cookie・ログインリダイレクト・CSRF 検証のため）。

公開中の dev は `https://yuuka-dev.sk-9.net`。設定例は
[systemd.config.yaml.example](dev/systemd.config.yaml.example) を参照。
トンネルの転送先は `http://127.0.0.1:5173` にする（API の 3403 番へ直接向けない）。
このホストでは `suki-tunnel add yuuka-dev.sk-9.net http://127.0.0.1:5173` で変更できる。

ログインは sk-account の OIDC で行う。dev は本番と別のクライアント `yuuka-dev` として、
sk-account の `oidc-clients.toml` に次を登録する（シークレットは `sk-account oidc new-secret` で作り、
ハッシュを sk-account に、シークレットを dev の `config.yaml` の `OIDC_CLIENT_SECRET` に置く）。

| 項目 | 本番（`yuuka`） | dev（`yuuka-dev`） |
|---|---|---|
| `redirect_uris` | `https://yuuka.sk-9.net/api/auth/oidc/callback` | `https://yuuka-dev.sk-9.net/api/auth/oidc/callback` |
| `backchannel_logout_uri` | `http://127.0.0.1:3402/api/auth/oidc/backchannel-logout` | `http://127.0.0.1:3403/api/auth/oidc/backchannel-logout` |
| `scopes` | `openid profile discord` | 同左 |

Yuuka は `BASE_URL` から戻り先を組み立てる。設定変更後は dev API を再起動し、
公開 URL でログインを最初からやり直す。


### 更新・停止・ログ

```bash
pnpm dev:update       # この作業ツリーで cargo build --release、dev バイナリ置換・再起動
pnpm dev:stop         # dev API と Vite を停止。共有 Redis は停止しない
pnpm dev:logs         # dev API の journal
pnpm dev:logs:vite    # バックグラウンド Vite の journal
pnpm dev:help
```

`dev:update` は systemd の WorkingDirectory にある `bin/yuuka` を更新する。
本番のサービス・バイナリは操作しない。設定・DB・静的ビルド成果物は更新しない。
`CARGO_TARGET_DIR` を設定している場合はその出力を使う。
フロントの依存を変更したら再インストールして Vite を起動し直す。
pnpm の `ERR_PNPM_VERIFY_DEPS_BEFORE_RUN` が出た場合も、まず
`pnpm install --frozen-lockfile` で作業ツリーの node_modules を同期する。

## Docker の dev インスタンス（従来構成）

現在のホストでは使わない（Docker 未導入。dev は上の systemd 構成で動かす）。Docker で dev API を動かす場合、
フロントはホストで `VITE_API_TARGET=http://127.0.0.1:7856 pnpm dev` を使う。

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

`HOST_PORT` は 7856 にする（7855 は Vite の管理画面が使う）。
