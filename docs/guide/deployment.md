# Docker デプロイ・複数インスタンス運用

同一ホスト上で Yuuka を複数インスタンス（本番 `prod` / 開発 `dev` / 任意の `<name>`）として互いに隔離して起動するための運用ガイドです。1 つの `Dockerfile` / `docker-compose.yml` をインスタンスごとの設定で使い回します。

スクリプトの詳細リファレンスは [../../deploy/README.md](../../deploy/README.md) を参照。ローカル/開発で直接動かす場合は [setup.md](setup.md)。

---

## 前提

- Docker Engine + Docker Compose v2 以上
- ホストユーザーが `docker` グループに所属（`sudo` なしで `docker` を使える）

各インスタンスは `COMPOSE_PROJECT_NAME` で名前空間が分離され、専用のネットワーク・Redis コンテナ・データ（bind マウント）・ホストポートを持ちます。

---

## 初期セットアップ

インスタンスごとに `deploy/<name>/` を用意します（`*.example` をコピーして編集）。

```bash
cp deploy/prod/instance.env.example deploy/prod/instance.env
cp deploy/prod/config.yaml.example  deploy/prod/config.yaml
openssl rand -base64 48 | tr -d '\n' > deploy/prod/secret.key
chmod 600 deploy/prod/secret.key
# instance.env（ポート/パス/プロジェクト名）と config.yaml（招待コード/OAuth/BASE_URL 等）を編集
```

- `instance.env` — 非機密の起動パラメータ（`COMPOSE_PROJECT_NAME` / `HOST_PORT` / `BIND_ADDR` / `DATA_DIR` / `CONFIG_FILE` / `PUID` / `PGID` / `YUUKA_IMAGE_TAG` / `DOCKER_NET_SUBNET` / `TRUSTED_PROXIES`）。値に `$` を含めないこと。
  - `YUUKA_IMAGE_TAG` — このインスタンス専用の Docker イメージタグ（例: prod=`latest` / dev=`dev`）。他インスタンスと共有すると `update` が互いのイメージを巻き込んで壊す。
  - `DOCKER_NET_SUBNET` / `TRUSTED_PROXIES` — 下記「外部公開」を参照。他インスタンスと重複しない値にすること。
- `secret.key` — 暗号化シークレット（`YUUKA_ENCRYPTION_SECRET`）。生値のみを 1 行で保存。
- `config.yaml` — システム共通設定。`*.example` を参照。

> いずれも git 管理外（`*.example` だけが追跡されます）。

---

## デプロイ・更新

コードを変更したら `pnpm run deploy` だけで反映できます。内部で「現行イメージ退避 → ビルド（Rust crawler + tsgo）→ コンテナ再作成 → ヘルスチェック」を実行します。DB マイグレーションは起動時に自動適用されます。

| コマンド | 動作 |
|---|---|
| `pnpm run deploy` | 本番(prod)をビルドして反映 |
| `pnpm run deploy:dev` | 開発(dev)を反映 |
| `pnpm run deploy:rollback` | 直前イメージ（`yuuka:prev-prod`）へ戻す |
| `pnpm run deploy:verify` | ヘルスチェックのみ（HTTP / Bot ログイン / エラー件数） |
| `pnpm run deploy:logs` | 本番ログ追従 |
| `pnpm run deploy:ps` / `deploy:down` | 状態確認 / 停止・撤去 |

> `pnpm deploy`（`run` 無し）は pnpm 組込みコマンドと衝突するため、必ず `pnpm run deploy` を使います。

低レベルには `deploy/instance.sh <name> <update|rollback|verify|up -d|down|logs -f|build|ps>` でも操作できます。

再作成時は数秒のダウンタイムが発生し、Bot は自動で再ログインします。

---

## 新しいインスタンスを追加する

1. `deploy/<name>/` を作成し `instance.env` / `config.yaml` / `secret.key` を用意（`COMPOSE_PROJECT_NAME` / `HOST_PORT` / `YUUKA_IMAGE_TAG` / `DOCKER_NET_SUBNET` は他インスタンスと重複させない）。
2. `openssl rand -base64 48 | tr -d '\n' > deploy/<name>/secret.key && chmod 600 deploy/<name>/secret.key`
3. `deploy/instance.sh <name> update`

---

## 暗号化シークレットのローテーション

1. `deploy/<name>/secret.key.new` に新しい鍵を置く（`instance.sh` が `YUUKA_ENCRYPTION_SECRET_NEW` として渡す）。
2. `deploy/instance.sh <name> up -d` → 起動時に全暗号化データが新鍵で再暗号化される。
3. 完了後 `secret.key` を新鍵に置き換え、`secret.key.new` を削除して再起動。

---

## 外部公開（リバースプロキシ / Cloudflare Tunnel）

`instance.env` の `BIND_ADDR` で公開バインドアドレスを切り替えます。

- プロキシ/トンネルがホスト LAN IP へ接続する構成では `0.0.0.0`。
- 完全にローカル（`127.0.0.1`）からのみ受ける場合は `127.0.0.1` にし、プロキシ側も `localhost` を向けます。

ホスト公開ポートは `HOST_PORT`、コンテナ内待受は固定で `7854` です。

### 信頼するプロキシ（`TRUSTED_PROXIES`）

cloudflared / nginx 等のプロキシ経由では、Rust から見た直前の接続元は常に Docker ブリッジの
ゲートウェイ IP になります。`X-Forwarded-For` から実クライアント IP を取り出すには、この
ゲートウェイ IP を `TRUSTED_PROXIES`（カンマ区切り）に設定する必要があります（未設定だと
ログイン試行のレート制限キーが実質 IP 単位でなくなり、第三者が特定ユーザーの試行枠を
使い切ってロックアウトさせられる可能性があります）。

ゲートウェイ IP は Docker がネットワークを再作成するたびに変わりうるため、
`DOCKER_NET_SUBNET` で bridge のサブネットを固定してください（サブネットの先頭アドレスが
既定でゲートウェイ IP になります。例: `172.28.0.0/24` → `172.28.0.1`）。複数インスタンスを
同一ホストで動かす場合は、インスタンスごとに重複しないサブネットを割り当てます
（`deploy/prod/instance.env.example` / `deploy/dev/instance.env.example` 参照）。

### WebSocket（`/ws/chat`）の透過設定

デスクトップクライアント（`docs/design/desktop_client/`）は同一ポートの HTTP `upgrade` で `/ws/chat` を張ります。リバースプロキシは WebSocket upgrade を透過する必要があります。

- **nginx**: 対象 location（または `/`）に以下を付与します。

  ```nginx
  location /ws/chat {
      proxy_pass http://127.0.0.1:7854;
      proxy_http_version 1.1;
      proxy_set_header Upgrade $http_upgrade;
      proxy_set_header Connection "upgrade";
      proxy_set_header Host $host;
      proxy_read_timeout 3600s;   # アイドル接続をプッシュ受信のため長めに保持
  }
  ```

- **Cloudflare Tunnel**: WebSocket は既定で透過されます（追加設定不要）。
- 認証は `Authorization: Bearer <desktop token>`（Cookie 非依存）。プロキシで Authorization ヘッダを削らないこと。

---

## ロールバック

デプロイのたびに `yuuka:prev-<name>` タグへ自動退避されます。問題があれば直前イメージへ即戻せます。

```bash
pnpm run deploy:rollback        # = deploy/instance.sh prod rollback
```

---

## systemd からの移行（レガシー）

旧構成は systemd ユニット（`yuuka.service`）で常駐していましたが、現在は Docker 運用へ移行済みです。systemd へ一時的に戻す場合は、コンテナと systemd が同じデータを共有するため**同時起動は不可**（SQLite WAL 単一ライター）である点に注意してください。

```bash
deploy/instance.sh prod down
sudo systemctl enable --now yuuka
```
