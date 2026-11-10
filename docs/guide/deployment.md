# 本番デプロイ（systemd）

Yuuka の本番を、ユーザー systemd のサービスとしてホストで直接動かす手順です。リポジトリのチェックアウトを
作業ディレクトリにし、ビルドしたバイナリ・画面を配信します。dev や日々のデプロイの細かい挙動は
[deploy/README.md](../../deploy/README.md)、ローカルで試すだけなら [setup.md](setup.md) を参照してください。

---

## 前提

- Linux（ユーザー systemd が使えること。ログアウト後も動かすなら `loginctl enable-linger "$USER"`）
- Rust（`rust-toolchain.toml` の版）、Node.js 24 + pnpm 9、npm
- Redis、Chromium（ブラウザ操作を使う場合）、fonts-noto-cjk（グラフの日本語）
- 外部公開する場合はトンネル（Cloudflare Tunnel 等）かリバースプロキシ

---

## 初期セットアップ

チェックアウトを `~/yuuka` に置く例です（ユニットの例の `%h/yuuka` を、置いた場所に合わせて書き換えます）。

```bash
git clone https://github.com/kawaii-music-moe/yuuka.git ~/yuuka && cd ~/yuuka
git switch develop

# 設定とシークレット（どちらも git 管理外）
cp deploy/prod/config.yaml.example deploy/prod/config.yaml   # BASE_URL・OIDC・Google 等を編集
ln -s deploy/prod/config.yaml config.yaml                     # yuuka は作業ディレクトリの config.yaml を読む
openssl rand -base64 48 | tr -d '\n' > deploy/prod/secret.key
chmod 600 deploy/prod/config.yaml deploy/prod/secret.key

# ユニット
mkdir -p ~/.config/systemd/user
cp deploy/systemd/yuuka.service.example       ~/.config/systemd/user/yuuka.service
cp deploy/systemd/yuuka-redis.service.example ~/.config/systemd/user/yuuka-redis.service
systemctl --user daemon-reload
systemctl --user enable --now yuuka-redis.service
systemctl --user enable yuuka.service
```

- `secret.key` — 暗号化シークレット（`YUUKA_ENCRYPTION_SECRET`）。生値のみを 1 行で保存。失くすと保存済みのトークン・
  API キー等を読めなくなるので、別の場所にも控えておきます。
- `config.yaml` — キーの一覧は [.env.example](../../.env.example)。
- サーバーでの音声生成を使うなら `deploy/systemd/yuuka-tts.service.example` も置きます（[deploy/README.md](../../deploy/README.md#音声モデルpwa-の音声チャット)）。

### 初回のデプロイ

DB がまだ無いので、初回だけ `YUUKA_INIT_DB=1` を渡して新規作成させます。

```bash
mkdir -p data
systemctl --user set-environment YUUKA_INIT_DB=1
pnpm run deploy            # ビルド → deploy/prod/bin/yuuka と dist/public を置いて起動 → 確認
systemctl --user unset-environment YUUKA_INIT_DB
```

初回ログイン前に `ADMIN_DISCORD_IDS` に管理者の Discord ID を設定してください。指定した ID の新規利用者だけが管理者になります。未設定なら、最初のログイン者も一般利用者になります。既存の権限は維持され、設定を後から追加しても既存利用者は自動昇格しません。既存の一般利用者を管理者にする場合は、既存管理者が管理画面から権限を変更してください。

> **既存の DB を持ってくるとき**: `data/yuuka.db` に置きます。V22 以前の DB は V25 で sk-account に紐付いていない利用者が
> 所有データごと削除されるので、[setup.md](setup.md) の「既存 DB を新しい版で開くとき」を先に確認してください。

---

## デプロイ・更新

```bash
pnpm run deploy                       # origin/develop を fast-forward → ビルド → 退避 → 入れ替え → 確認
pnpm run deploy:verify                # 確認だけ
pnpm run deploy:rollback -- <退避先>  # 退避した状態（バイナリ・画面・DB）へ戻す
pnpm run deploy:logs                  # journal を追う
```

- 止めている数秒の間にバイナリ・画面・DB・設定を `deploy/prod/backups/` へ退避します（直近 3 件を残す）。
- DB マイグレーションは起動時に自動で当たります。前方専用なので、古い版へ戻すときは DB ごと戻ります（`rollback`）。
- 入れ替えの間は数秒止まり、Bot は自動で再ログインします。

> `pnpm deploy`（`run` 無し）は pnpm 組込みコマンドと衝突するため、必ず `pnpm run deploy` を使います。

---

## 暗号化シークレットのローテーション

1. `deploy/prod/secret.key.new` に新しい鍵を置く（`run-yuuka.sh` が `YUUKA_ENCRYPTION_SECRET_NEW` として渡す）。
2. `systemctl --user restart yuuka.service` → 起動時に全暗号化データが新しい鍵で再暗号化される。
3. 完了後 `secret.key` を新しい鍵に置き換え、`secret.key.new` を削除して再起動。

---

## 外部公開（リバースプロキシ / Cloudflare Tunnel）

yuuka は `config.yaml` の `HOST` / `PORT`（例: `127.0.0.1:3402`）で待ち受けます。トンネル / プロキシはこのポートへ
向け、`BASE_URL` をブラウザで開く公開 URL（`https://...`）にします（OAuth の戻り先・Cookie・CSRF 判定に使います）。

### 信頼するプロキシ（`TRUSTED_PROXIES`）

プロキシ経由では、yuuka から見た直前の接続元はプロキシになります。`X-Forwarded-For` から実クライアント IP を
取り出すには、そのプロキシの IP を `TRUSTED_PROXIES`（カンマ区切り）に設定します。同じホストで動かすなら
`127.0.0.1` です（未設定だと X-Forwarded-For を信頼せず、デバイスフロー等のレート制限がプロキシの IP 単位になります）。

### WebSocket（`/ws/chat`）の透過設定

デスクトップクライアント（`docs/design/desktop_client/`）は同一ポートの HTTP `upgrade` で `/ws/chat` を張ります。リバースプロキシは WebSocket upgrade を透過する必要があります。

- **nginx**: 対象 location（または `/`）に以下を付与します。

  ```nginx
  location /ws/chat {
      proxy_pass http://127.0.0.1:3402;
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

## AI プロバイダとモデル

プロセスの作業ディレクトリにある `.env` に次の既定値を指定できます。
現在対応するプロバイダは `gemini` のみです。

```dotenv
LLM_PROVIDER=gemini
LLM_MODEL=gemini-3.5-flash-lite
```

この2項目だけは起動時に自動読込され、同名の環境変数がある場合は環境変数を優先します。
不正なプロバイダ・モデルは起動時に拒否します。本番では作業ディレクトリ（チェックアウト）の `.env` に書きます。
既定値の変更には再起動が必要です。

「Bot設定」→「汎用モード設定」のAPIキー付近で、Botごとのプロバイダ・モデルを保存できます。
画面で保存した値を優先し、「既定値に戻す」で環境の既定値を再び使います。
画面での変更は次のメッセージから有効になり、APIキーは変更・削除されません。
秘書モードは「アカウント管理」のAPIキー設定画面で指定した個人モデルを使います。
既存の個人モデルは維持し、NULL/空の場合だけ環境の既定値へフォールバックします。
モデルの無料枠・利用上限はGoogle AI Studioで確認してください。
