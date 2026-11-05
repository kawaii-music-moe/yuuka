# Yuuka の運用（本番・dev）

本番も dev も、ユーザー systemd のサービスとしてホストで直接動かす（コンテナは使わない）。
本番はリポジトリのチェックアウトを作業ディレクトリにしてビルド済みのバイナリを動かし、dev は Vite の HMR で
画面を配信する。初めて立てる手順は [docs/guide/deployment.md](../docs/guide/deployment.md) を参照。

## 構成

```
deploy/
  systemd/                      ユーザー systemd のユニットの例（~/.config/systemd/user/ にコピーして使う）
    yuuka.service.example         本番（ExecStart = deploy/prod/run-yuuka.sh）
    yuuka-dev.service.example     dev API（作業ディレクトリ = deploy/dev/runtime）
    yuuka-redis.service.example   セッション用 Redis（本番と dev で共有・保存しない）
    yuuka-tts.service.example     サーバーでの音声生成（本番と dev で共有）
  prod/
    config.yaml.example         本番の設定の雛形（config.yaml は git 管理外。作業ディレクトリの config.yaml からリンクする）
    secret.key                  暗号化シークレット（YUUKA_ENCRYPTION_SECRET）※git 管理外
    run-yuuka.sh                yuuka.service の ExecStart。secret.key を環境変数にして bin/yuuka を起動する
    deploy-develop.sh           develop の最新版を本番へ出す・戻す・確かめる
    redis.conf                  yuuka-redis.service の設定
    bin/yuuka                   本番のバイナリ ※git 管理外（deploy-develop.sh が置く）
    backups/                    デプロイ時の退避（直近 3 件）※git 管理外
  dev/
    config.yaml.example         dev API の設定の雛形（deploy/dev/runtime/config.yaml にする）
    runtime/                    dev API の実行ディレクトリ（config.yaml・.env・data/・bin/yuuka）※git 管理外
```

本番の作業ディレクトリ（チェックアウト）には、ほかに `data/`（DB `yuuka.db`・音声モデル `tts-models/` など）と
`dist/public`（管理画面と PWA のビルド結果）がある。どちらも git 管理外。

## 本番のデプロイ

main には出さず、develop の最新版を本番へ反映する。

```bash
pnpm run deploy                       # = deploy/prod/deploy-develop.sh（origin/develop を fast-forward して本番へ）
pnpm run deploy:verify                # 確認だけ（画面・音声・CSP・マイグレーション・Discord）
pnpm run deploy:rollback -- <退避先>  # 退避した状態へ戻す（DB も戻る）
pnpm run deploy:logs                  # 本番の journal を追う
```
> `pnpm deploy`（`run` 無し）は pnpm 組込みコマンドと衝突するため、必ず `pnpm run deploy` を使う。

- 本体のチェックアウトが develop で、追跡中のファイルに未コミットの変更が無いこと。
- ビルド（Rust・管理画面・PWA）は配信中の `dist/public` に書かず別の場所で作り、サービスを止めている数秒の
  間に入れ替える。止めている間に、それまでのバイナリ・画面・DB（WAL ごと）・設定を
  `deploy/prod/backups/<時刻>-develop-<sha>/` へ退避し、`deployment.json` に記録する。
  確認まで通ったら、退避は新しい順に 3 件だけ残す（1 件で約 120MB。`KEEP_BACKUPS`）。
- 確認に失敗しても自動では戻さない（マイグレーション後に古いバイナリへ戻すには DB も戻す必要があり、デプロイ後の
  書き込みが消えるため）。表示される `rollback` コマンドで戻すかを判断する。退避も消さない。
- DB マイグレーションは起動時に自動で当たる。前方専用なので、戻すときは DB ごと戻す（rollback がそうする）。

## 暗号化シークレットのローテーション

1. `deploy/prod/secret.key.new` に新しい鍵を置く（run-yuuka.sh が `YUUKA_ENCRYPTION_SECRET_NEW` として渡す）。
2. `systemctl --user restart yuuka.service` → 起動時に全暗号化データが新しい鍵で再暗号化される。
3. 完了後 `secret.key` を新しい鍵に置き換え、`secret.key.new` を削除して再起動する。

## 運用の注意

### DB は既定では起動前に必ず存在させる（P0-4 / issue #55）
Rust は**既定では存在しない DB を作らない**（無ければ即エラーで起動失敗）。`DB_PATH` の誤設定で意図せず
空の DB を作ってしまう事故を防ぐため。新しいインスタンスだけ、初回起動のときに `YUUKA_INIT_DB=1` を渡す
（`systemctl --user set-environment YUUKA_INIT_DB=1` → 起動 → `unset-environment`）。無ければ新規作成して
baseline（V17）から migrations を当てる（既存 DB があれば何もしない）。

### 定期ジョブを回すプロセスは厳密に 1 つ（P0-2）
本番は run-yuuka.sh で `YUUKA_RUST_CRON=1`、dev はユニットで `YUUKA_RUST_CRON=0`。同じ DB に書くプロセスは
1 つでなければならない（SQLite WAL 単一ライター）。同じ `data/` を指すプロセスを同時に起動しないこと。

## 音声モデル（PWA の音声チャット）
PWA の「音声」画面は、返答の読み上げを端末の GPU で Irodori-TTS を動かして生成する。モデル一式（約 1.7 GB）は
作業ディレクトリの `data/tts-models/<バージョン>/` に置くと `/tts-models/*` として配信される（無ければ配信せず、
PWA はブラウザ標準の読み上げに切り替える）。作り方と配置は [tools/irodori-onnx/README.md](../tools/irodori-onnx/README.md)
の `package.py` を参照（`--dest <実行ディレクトリ>/data/tts-models`）。リポジトリには含めないので、
インスタンスごとに配置する。既存バージョンのファイルは書き換えない（`immutable` で配信している）。
利用者が作った声（話者条件。サンプルの音声そのものではない）は DB の `client_voices`（V36）に本人のものとして
保存され、`/api/client/voices` で本人だけが読み書きする。

GPU（WebGPU）の無い端末のために、サーバーでも音声を生成できる（`clients/pwa/server/tts`・onnxruntime-node の CPU で
同じ推論コードを動かす）。音声生成サーバー `yuuka-tts.service`（[deploy/systemd/yuuka-tts.service.example](systemd/yuuka-tts.service.example)）を
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

## dev

ユーザー systemd の `yuuka-dev.service`（[例](systemd/yuuka-dev.service.example)）が
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
[config.yaml.example](dev/config.yaml.example) を参照。
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
