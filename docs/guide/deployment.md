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

最初にログインした利用者（または `ADMIN_DISCORD_IDS` に含まれる利用者）が管理者になります。

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

## 外部サービスを使う運営者の確認事項

各インスタンスの運営者が、自分の Google Cloud プロジェクト・API キー・Discord アプリについて確認します。以下は 2026-11-11 に公式資料を確認した内容です。公開前や規模が変わる際はリンク先と各管理コンソールの最新案内を確認してください。

### Google OAuth（カレンダー・Drive）

Yuuka は `calendar` と `drive.file` に加えてプロフィール用のスコープを要求します（`crates/yuuka-google/src/http.rs` の `OAUTH_SCOPES`）。公開するプロジェクトには同意画面の運営者情報・ドメイン・規約・プライバシーポリシーと、実際に要求するスコープを登録し、必要な審査を受けてください。

- External / Testing は登録済みのテストユーザー最大 100 人に限られ、認可とリフレッシュトークンは原則 7 日で失効します。Yuuka の Calendar / Drive スコープは基本プロフィールだけの例外に該当しません。継続運用で Testing のままにすると同期やバックアップが止まり、再連携が必要になります。
- In production への公開と、機密・制限付きスコープの審査完了は別です。未承認スコープで未確認アプリの警告が出る場合は、プロジェクトの累計新規利用者数にも上限があります。

公式: [利用対象・公開状態・上限](https://support.google.com/cloud/answer/15549945?hl=en)、[OAuth 2.0 のトークン失効条件](https://developers.google.com/identity/protocols/oauth2/)、[本番公開と審査](https://developers.google.com/identity/protocols/oauth2/production-readiness/overview)。

### Gemini API（入力データ・年齢・地域）

API キーは利用者が登録しますが、運営者は、会話、予定・タスク、家計やレシート、連絡先、音声・画像などが、利用する機能に応じて Google に送信されることを利用者に説明してください。

無料サービスでは入力・出力が製品改善や人によるレビューに使われる場合があり、規約は機微情報・機密情報・個人情報を送らないよう求めています。家計・連絡先などを扱う運用は無料枠を前提にせず、課金状態とデータ取扱条件を確認してください。有料サービスにも不正利用監視等の取扱いがあり、「保存されない」とは案内しないでください。

Gemini API の規約には 18 歳以上という条件、18 歳未満がアクセスしうる API クライアントへの制限、提供地域・用途の条件があります。EEA・スイス・英国の利用者向け提供は有料サービスの条件も確認してください。用途と対象者が条件に適合するか、公開前に判断してください。

公式: [Gemini API 追加利用規約](https://ai.google.dev/gemini-api/terms)、[提供地域](https://ai.google.dev/gemini-api/docs/available-regions)。

### Discord（アプリ認証と特権 Intent は別）

Yuuka の Gateway 接続は `MESSAGE_CONTENT` を要求します（`crates/yuuka-discord/src/tenant.rs`）。Developer Portal の Bot 設定でも必要な Intent を有効にしてください。未設定・未承認なら接続が拒否される場合があります。

- **アプリ認証**: 100 サーバーを超えて Bot を導入するには認証が必要です。Developer Portal の App Verification の要件を満たしてください。
- **特権 Intent の審査**: 2026-06-10 から、サーバー数ではなく、導入先全体で Bot にアクセスできるユニークユーザー数が基準です。10,000 人のしきい値に近づいたら Portal の通知と申請案内を確認してください。承認済みでも毎年の再申請が必要です。

公式: [アプリ認証](https://support-dev.discord.com/hc/en-us/articles/23926564536471-How-Do-I-Get-My-App-Verified)、[特権 Intent の審査](https://docs.discord.com/developers/gateway/getting-started-with-privileged-intent-review)、[特権 Intent の申請案内](https://support-dev.discord.com/hc/en-us/articles/6205754771351-How-do-I-get-Privileged-Intents-for-my-bot)。
