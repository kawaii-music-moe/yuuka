# 同梱ログイン（sk-account 互換）実装方針

- ステータス: 提案（[#185](https://github.com/kawaii-music-moe/yuuka/issues/185)）。**実装は保留中。着手前に必ずオーナーに確認し、了承を得てから始めること。**
- 作成日: 2026-10-20
- 雛形: sk-account（`/home/suki/web/sk-9.net/sk-account`、特に `AUTHENTICATION_SPEC.md`＝移植仕様書）
- 関連: `crates/yuuka-auth/src/routes.rs`（現行の sk-account ログイン）、`crates/yuuka-auth/src/users.rs`（`upsert_from_sk_account`）、`crates/yuuka-core/src/config.rs`、`crates/yuuka-db/migrations/V23__sk_account_auth.sql`・`V25__sk_account_only.sql`、`frontend/src/overlays/Login.svelte`

## 1. 背景と目的

V25 以降、Yuuka 本体は本人認証を持たず、外部の sk-account に任せている。この構成には次の問題がある。

- `SK_ACCOUNT_URL` の既定値は作者の `https://account.sk-9.net`。しかも sk-account 側は戻り先を `https://yuuka.kawaii-music.moe/api/auth/sk/callback` に固定している。このため第三者がセルフホストすると、そのままではログインできない。
- sk-account を自分で立てるには、Node ランタイム、別プロセス、Discord Bot を別途用意しなければならない。

そこでログイン方式を次の 2 つから選べるようにする。

| 方式 | `AUTH_PROVIDER` | 内容 |
|---|---|---|
| 自前で用意 | `external` | 運営者が用意した sk-account 互換サーバーを使う（＝現行の挙動）。 |
| 同梱 | `bundled` | sk-account を雛形にした Rust 実装を Yuuka のバイナリに同梱する。**Yuuka と同じドメイン（同一ホスト、または同じ親ドメインのサブドメイン）でだけ動く。** |

### 非目標

- パスワードやメールによるログイン（雛形と同じく、パスキー専用）。
- 汎用 IdP 化。同梱版は Yuuka 1 つのためだけにある。他サービス向けの `/api/auth/verify`、親ドメイン Cookie による SSO、sk-admin 向けの `/api/admin/*`、kawaii-account のロール／サービス登録は持たない。
- OIDC など、sk-account 互換プロトコル以外の外部 IdP への対応（必要になれば別途検討する）。

## 2. 全体像

Yuuka 本体の認証フローは**現行のまま**にする。流れは「`/api/auth/sk/start` → IdP の authorize → 一回限りの code → exchange → Yuuka 自身のセッション（Redis・`__Host-yuuka-session`）を発行」。差し替えるのは **code の交換先だけ**。

```
            ブラウザ
   ┌──────────┴───────────────────────────────┐
   │ /api/auth/sk/start → authorize → callback │（URL 形状は両方式で共通）
   ▼                                           ▼
 Yuuka 本体（yuuka-auth）               IdP（ログイン画面・パスキー）
   │  IdentityProvider::exchange(code)
   ├─ external: HTTP POST {SK_ACCOUNT_INTERNAL_URL}/api/services/yuuka/exchange（現行コードを移すだけ）
   └─ bundled : 同一プロセスの yuuka-account を直接呼ぶ（HTTP の exchange 口は公開しない）
```

同一ホストで動かす場合でも code フローを通す理由は次のとおり。

- Yuuka 本体を「ログイン機能を持たない」状態に保てる。本体が知るのは「code を本人情報に変換してくれる IdP」だけになる。
- 以降の処理（Redis セッション、デバイスフロー、`/ws/chat`、CSRF ガード、`users.sk_account_id` への紐付け）が方式によって分岐しない。
- 既存の `state` によるログイン CSRF 対策と、そのテスト（`crates/yuuka-auth/tests/auth_flow.rs`）をそのまま使える。

## 3. 設定

`config.yaml`／環境変数（既存の `getSetting` と同じ優先順）に次のキーを追加する。

| キー | 既定値 | 適用 | 意味 |
|---|---|---|---|
| `AUTH_PROVIDER` | `external` | 共通 | `external` か `bundled`。既定値は現行本番との互換のため。`example.yaml` には `bundled` を書く。 |
| `SK_ACCOUNT_URL` / `SK_ACCOUNT_INTERNAL_URL` | 現行どおり | external | 変更しない。 |
| `AUTH_PROVIDER_LABEL` | `sk-9 Account` | external | ログインボタンの表示名。現在は `Login.svelte` に直書きされている。 |
| `ACCOUNT_URL` | `${BASE_URL}/account` | bundled | 同梱ログインの公開 URL（オリジン＋パス）。 |
| `ACCOUNT_RP_ID` | `ACCOUNT_URL` のホスト | bundled | WebAuthn の RP ID。**同時に「同一ドメイン」の範囲でもある**（§4）。 |
| `ACCOUNT_RP_NAME` | `Yuuka` | bundled | パスキー作成時に表示される名前。 |
| `ACCOUNT_DB_PATH` | `./data/account.db` | bundled | 同梱ログイン専用の SQLite（§5.3）。 |
| `ACCOUNT_DISCORD_TOKEN` | なし | bundled | 登録用 Discord Bot のトークン。未設定だと新規登録と復旧ができない（ログインはできる）。 |
| `ACCOUNT_ALLOWED_DISCORD_IDS` | 空（制限なし） | bundled | 登録を許可する Discord ユーザー ID（雛形の `DISCORD_ALLOWED_USER_IDS`）。 |
| `ACCOUNT_SESSION_TTL_DAYS` | `30` | bundled | IdP セッションの TTL（雛形の固定値を設定可能にしたもの）。 |

`bundled` のときは `SK_ACCOUNT_*` を読まない。`external` のときは `ACCOUNT_*` を読まない。

## 4. 同一ドメイン要件

### 4.1 取りうる配置

| 型 | 例 | ルーティング |
|---|---|---|
| A. 同一ホスト | `BASE_URL=https://yuuka.example.com`、`ACCOUNT_URL` は省略（→ `https://yuuka.example.com/account`） | Yuuka のルーターに `/account` として `nest` する。 |
| B. サブドメイン | `BASE_URL=https://yuuka.example.com`、`ACCOUNT_URL=https://account.example.com`、`ACCOUNT_RP_ID=example.com` | 同じリスナーで **Host ヘッダーによって振り分ける**。`account.example.com` 宛ては同梱ログインのルーターだけに渡し、それ以外は Yuuka 本体に渡す。B 型では本体側に `/account` を載せない（WebAuthn のオリジンを 1 つに保つため）。 |

B 型も 1 プロセス・1 ポートのままにする。docker-compose の `HOST_PORT` は 1 つでよく、cloudflared の ingress で 2 つのホスト名を同じポートへ向けるだけで済む（`docs/guide/deployment.md` に例を追記する）。

### 4.2 起動時の検証（`bundled` のときだけ・違反したら fail-fast）

1. `BASE_URL` は必須。ただし、未設定の開発環境（`http://localhost:<PORT>`）は除く。
2. `ACCOUNT_URL` のスキームは `BASE_URL` と一致させる（本番は https）。
3. `ACCOUNT_URL` のホストは `ACCOUNT_RP_ID` と同じか、`.ACCOUNT_RP_ID` で終わること（WebAuthn の要件）。
4. **`BASE_URL` のホストも `ACCOUNT_RP_ID` と同じか、`.ACCOUNT_RP_ID` で終わること（同一ドメイン要件）。**
5. `ACCOUNT_RP_ID` にドットが無いもの（`com` など）は、`localhost` を除いて拒否する。完全な Public Suffix 判定はしない（ブラウザ側でも拒否される）。
6. A 型では、パスが `/` 以外で、かつ予約済みのプレフィックス（`/api` `/admin` `/ws` `/proxy` とアセット）と衝突しないこと。B 型ではパスは `/` に限る。
7. `ADMIN_DISCORD_IDS` が空でないこと（§7 の「最初のログイン者が admin」対策）。

| `BASE_URL` | `ACCOUNT_URL` | `ACCOUNT_RP_ID` | 結果 |
|---|---|---|---|
| `https://yuuka.example.com` | （省略） | （省略→`yuuka.example.com`） | OK（A 型） |
| `https://yuuka.example.com` | `https://account.example.com` | `example.com` | OK（B 型） |
| `https://example.com` | `https://account.example.com` | `example.com` | OK（B 型・Yuuka が apex） |
| `https://yuuka.example.com` | `https://account.example.com` | （省略→`account.example.com`） | NG：`BASE_URL` が RP ID の配下にない。エラー文で `ACCOUNT_RP_ID=example.com` の指定を案内する。 |
| `https://yuuka.example.com` | `https://login.other.net` | 任意 | NG：別ドメイン |

### 4.3 オリジン・Cookie・戻り先

- WebAuthn の `expectedOrigin` は `ACCOUNT_URL` のオリジンだけにする（A 型では Yuuka と同じオリジン）。RP ID を親ドメインにしていても、兄弟サブドメインで作られた署名はオリジン不一致で弾かれる。
- Cookie はすべて **host-only の `__Host-` 接頭辞**にする（開発の http では接頭辞なし。既存の `COOKIE_HOST`/`COOKIE_DEV` と同じ扱い）。
  - 対象は `__Host-yuuka-account-session`、`__Host-yuuka-account-reg`、`__Host-yuuka-account-challenge`。
  - 雛形の `COOKIE_DOMAIN`（親ドメインでの共有）は持たない。利用者が Yuuka だけなので共有する理由がない。また、兄弟サブドメインからの Cookie 上書き（ログイン CSRF）を防げる。この方針は既存の state Cookie と同じ。
- `return_to` は、`ACCOUNT_URL` と同じオリジン、または `BASE_URL` と同じオリジンだけを許可する。雛形の「RP_ID 配下のホストなら何でも許可」より狭い。
- 戻り先（callback）は `{BASE_URL}/api/auth/sk/callback` に固定し、設定から算出する（雛形のように直書きはしない）。

## 5. 同梱ログイン：新規 crate `yuuka-account`

### 5.1 雛形との対応

| sk-account | yuuka-account | 備考 |
|---|---|---|
| `src/config.ts` | `config.rs` | §4.2 の検証を含む。 |
| `src/db.ts` | `schema.rs`＋独自のマイグレーション（refinery） | §5.3 |
| `src/session.ts` | `session.rs` | 32B CSPRNG、SHA-256 hex で保存、1 時間ごとのスライディング延長。 |
| `src/passkey.ts` | `passkey.rs` | discoverable 必須、usernameless、challenge は一回限り（`DELETE … RETURNING`）。 |
| `src/registration.ts` | `registration.rs` | PIN の状態遷移、Discord ユーザーごとに 10 分で 5 回までの制限、登録完了のトランザクション。 |
| `src/bot/index.ts` | `bot.rs`（twilight） | §5.4 |
| `src/server/app.ts` | `routes.rs`（axum） | §5.2 |
| `client/` | `account-ui/`（Svelte 5＋Vite、`@simplewebauthn/browser`） | §5.5 |
| `registration.test.ts` / `app.test.ts` | Rust のテストへ移植 | §8 |

**WebAuthn ライブラリ**：本プロジェクトは rustls に固定しており、native-tls/openssl を避けている（`Cargo.toml` の reqwest の注記）。そのため openssl が必須の `webauthn-rs` は使わず、純 Rust の実装（第一候補は `webauthn-rp`）を step 3 で評価する。採用前に `cargo deny check` を通すこと。署名検証を自前で書くことはしない。

**DAG**：`yuuka-account` は `yuuka-auth` に依存しない（IdP と RP を分けておく）。両者をつなぐアダプター（`IdentityProvider` の bundled 実装）は `yuuka-supervisor` に置く。

### 5.2 HTTP API（パスは `ACCOUNT_URL` からの相対）

雛形の §12 を基本にし、次の点を変える。

| 区分 | エンドポイント | 扱い |
|---|---|---|
| そのまま | `/api/health`、`/api/discord`、`/api/discord/callback`、`/api/registration*`、`/api/login/*`、`/api/logout`、`/api/me`、`/api/me/profile/sync`、`/api/passkeys*` | 挙動とエラー文言は雛形に合わせる。 |
| 残す | `GET /api/services/yuuka/authorize?state=` | code を account.db（`service_authorization_codes`：60 秒・一回限り）に発行し、§4.3 の callback へリダイレクトする。 |
| 公開しない | `POST /api/services/yuuka/exchange` | プロセス内の `AccountService::exchange` に置き換える。認証なしのサーバー間 POST 口がなくなる。 |
| 持たない | `/api/auth/verify`、`/api/admin/*` | 非目標（§1）。 |

CSRF 対策：GET/HEAD 以外は `Origin == ACCOUNT_URL のオリジン` を厳密に確認する（雛形の §12.1）。これは Yuuka 本体の `csrf_guard`（Yuuka のセッション Cookie を持つリクエストだけが対象）とは独立して、同梱ログインのルーター側で必ずかける。

### 5.3 永続化

- **Yuuka 本体の DB とは別ファイル**（`ACCOUNT_DB_PATH`）にし、**スキーマは sk-account と互換**にする（`accounts` / `passkeys` / `registrations` / `sessions` / `service_authorization_codes` / `challenges` / `pin_attempts`、および `role` 列）。
  - こうしておくと、後で外部の sk-account へ移るときに account.db を持ち込むだけでアカウント ID を引き継げる（§6.4）。
- これに加えて `meta(instance_id)` を作成時に一度だけ採番する。発行元の識別に使う（§6.4）。
- DB アクセスは `yuuka-db` と同じ構成（writer actor 1 つ＋read pool）を、別ファイルに対して使う。
- Discord OAuth の state は雛形と同じくメモリに保持する（単一プロセスが前提）。

### 5.4 登録用 Discord Bot

- 雛形の §14 と同じ構成にする：User Install、`applications.commands`、DM 専用の `/register pin:`、返信は ephemeral。
- twilight-gateway のシャードを `Intents::empty()` で 1 つ張る。プロフィールの同期は twilight-http の `GET /users/{id}` で行う。
- Supervisor の JoinSet にバックオフ付きで再起動するタスクとして登録する（絶対制約 2）。トークンが無い場合や Bot が落ちた場合は、登録と復旧だけを止めて警告を出し、ログインは継続する。
- Yuuka のテナント Bot（`yuuka-discord`）は使わない。アカウントができる前に必要な Bot なので、テナントの仕組みに乗せると順序が逆になる。

### 5.5 画面（`account-ui/`）

- 雛形の `client/src/routes/{Login,Register,Account}.svelte` と `lib/api.ts` を移植し、pnpm workspace の 1 パッケージにする。ビルド結果は `dist/account/` に出力する。
- A 型と B 型でベースパスが変わるので、アセットは相対パスでビルドする。配信時に `index.html` へ `<base href>` と API の基点を差し込む（既存の google-site-verification の差し込みと同じ方式）。
- 「sk-9」の表記は使わず、`ACCOUNT_RP_NAME` を表示する。
- CSP は Yuuka と同じもの（`yuuka-web` の `CSP`）で足りる。`connect-src 'self'` と `img-src cdn.discordapp.com` は既にある。B 型では同梱ログイン側のルーターにも `apply_common_layers` をかける。
- `Dockerfile` の frontend-builder ステージに account-ui のビルドを加える。

## 6. Yuuka 本体側の変更

### 6.1 `IdentityProvider`（yuuka-auth）

```rust
#[async_trait]
pub trait IdentityProvider: Send + Sync {
    /// ブラウザを送る authorize URL（state 付き）。
    fn authorize_url(&self, state: &str) -> String;
    /// 一回限りの code を確認済みのアカウントへ交換する。
    async fn exchange(&self, code: &str) -> Result<VerifiedAccount, IdpError>;
    /// Yuuka のログアウトに合わせて IdP セッションも終わらせる（external では何もしない）。
    async fn end_session(&self, idp_session: &IdpSessionRef);
    /// 発行元の識別子（§6.4）。
    fn issuer(&self) -> String;
}
```

- `AuthRuntime` が持っている `sk_account_*_url` と `http` は `SkAccountHttp`（external の実装）へ移す。`start` / `callback` は trait 越しに呼ぶだけにする。
- `IdpError` は thiserror の具体的な列挙型にする（`InvalidCode` / `Unavailable` / `BadResponse`）。既存の 401 / 502 の振り分けはそのまま保つ。
- `users::upsert_from_sk_account` はそのまま使う。同梱版でも account.db の UUID を `users.sk_account_id` に入れる。

### 6.2 ログアウトの連動（bundled のみ）

同梱版では、Yuuka からログアウトしたのに IdP セッションが残っていると、次のログインがパスキーなしで素通りする。これを避ける。

- exchange の結果に、code を発行した IdP セッションへの参照を含める。
- Yuuka 側は Redis の `session_idp:{sha256(token)}` に、Yuuka セッションと同じ TTL で保存する。既存の `session:*` の JSON 形式は変えない。
- `/api/logout` で両方のセッションを消す。B 型では IdP の Cookie が Yuuka のホストに届かないため、Cookie ではなくこの参照を使って消す。

### 6.3 フロントエンド

- `GET /api/auth/provider`（認証不要）を追加する。返す値は `{ provider, label, accountUrl?, registerUrl? }`。
- `Login.svelte`：ボタンの文言をこのレスポンスから作る。bundled のときは「アカウントを作成」リンクも出す。リンク先は `registerUrl` で、`return_to` に `/api/auth/sk/start` を入れ、登録が済んだらそのままログイン状態になるようにする。
- 管理画面のアカウント管理（`AccountOverlay`）に、bundled のときだけ「パスキーの管理」（`ACCOUNT_URL` へのリンク）を加える。

### 6.4 方式の切り替えと発行元の記録

`users.sk_account_id` は IdP ごとに異なる ID 空間なので、IdP を取り替えると既存ユーザーが「この Discord ID は別の sk-account に結び付いています」で弾かれる。これを黙って壊さないようにする。

- 発行元を `system_settings` の `auth.issuer` に記録する。値は external なら `external:<SK_ACCOUNT_URL のオリジン>`、bundled なら `bundled:<account.db の instance_id>`。
- 起動時、記録が無ければ現在の値を書き込む（現行本番はこれで移行が終わる）。記録と現在の値が食い違い、かつ紐付け済みのユーザーがいる場合は fail-fast で起動を止め、次のサブコマンドを案内する。
  - `yuuka auth relink`：DB をバックアップしてから `users.sk_account_id` をすべて NULL にし、`auth.issuer` を更新する。次回のログインで、既存の「未紐付けの行は Discord ID が一致すれば一度だけ引き受ける」処理によって再び紐付く。
  - `yuuka auth accept-issuer`：紐付けは残し、`auth.issuer` だけを更新する。同じアカウント DB を別の URL に移した場合など、ID が変わらない場合に使う。
- 移行のパターン：
  - **external → bundled**：`relink`。パスキーは同梱側で新しく作る（Discord の PIN 手順）。
  - **bundled → external（sk-account）**：account.db を sk-account に持ち込めば ID が変わらないので `accept-issuer` で済む。ただし RP ID が変わる場合は、パスキーを作り直す必要がある（雛形 README の「ドメインを変えるとき」）。

## 7. セキュリティ上の判断

| 論点 | 判断 |
|---|---|
| 最初のログイン者が admin になる（`upsert_from_sk_account`） | 同梱版は登録を誰にでも開けうるので、知らない人が最初に登録すると admin を取られる。bundled では `ADMIN_DISCORD_IDS` を必須にする（§4.2-7）。 |
| 登録の開放範囲 | 既定は雛形と同じく制限なし。`ACCOUNT_ALLOWED_DISCORD_IDS` で絞れる。セットアップガイドでは、絞ることを推奨する。 |
| PIN 発行の濫用 | 雛形は 24 時間で掃除するだけ。`POST /api/registration` に IP 単位の簡易レート制限を加える（`TRUSTED_PROXIES` を踏まえた XFF 判定は既存のものを流用）。 |
| 本人性の根拠 | 雛形と同じく Discord アカウント（雛形の §15.2-1）。relink 後の再紐付けも同じ根拠に依存する。このことを文書に明記する。 |
| User Verification | 雛形と同じく `preferred` とし、必須にはしない。将来の資格情報機能などで再認証が必要になったら、設定で `required` にできる余地を残す。 |
| 親ドメインの RP ID（B 型） | オリジンを厳密に照合するので、兄弟サブドメインへの中継は成立しない。既定値は最も狭い `ACCOUNT_URL` のホストにし、B 型で必要なときだけ親ドメインへ広げる。 |
| 同一ドメイン上の複数インスタンス（prod/dev） | Cookie は `__Host-` なので衝突しない。ただし、RP ID を共有するとパスキー一覧に別インスタンスの項目が並んで紛らわしい。インスタンスごとにホストを RP ID にすることを推奨する。 |

## 8. 実装ステップ（PR 単位）

各 PR は develop から切る。手元での確認は `cargo check` で行い、テストは develop か CI で回す（`CLAUDE.md` の実ビルド制限に従う）。

1. **設定と trait の切り出し（挙動は変えない）**：`AUTH_PROVIDER` / `AUTH_PROVIDER_LABEL`、`IdentityProvider` と `SkAccountHttp`、`/api/auth/provider`、`Login.svelte` の文言。既存の `auth_flow.rs` がそのまま通ることを確認する。
2. **`yuuka-account` の土台**：設定と §4.2 の検証、account.db のスキーマとマイグレーション、セッション、challenge、登録の状態遷移と PIN 制限。`registration.test.ts` 相当のテストを移植する。
3. **WebAuthn**：ライブラリを選定し（`cargo deny`）、options の生成と検証、カウンター更新、最後の 1 つは削除できない制約を入れる。
4. **HTTP と結線**：axum のルーター、Origin 検証、`return_to`、authorize とプロセス内 exchange、A 型の `nest` と B 型の Host 振り分け、ログアウト連動、`auth.issuer` の検査、`yuuka auth relink|accept-issuer`。
5. **登録 Bot**：twilight による `/register` とプロフィール同期、supervisor への登録、縮退時の挙動。
6. **`account-ui`**：画面の移植、ベースパスの差し込み、Dockerfile。
7. **文書**：`example.yaml`、`docs/guide/setup.md`・`deployment.md`（A/B 型の cloudflared 例、Discord アプリの設定）、`docs/project_overview.md` の crate 一覧、external 方式のプロトコル契約（authorize/exchange）を明文化。

## 9. 受け入れテスト

- 雛形の §18 の項目のうち、§5.2 で「持たない」とした API に関するもの以外すべて。
- §4.2 の表の全行（起動時検証の OK/NG）。
- A 型：`/account/*` が同梱ログインに届き、Yuuka の既存ルートと SPA フォールバックを壊さない。
- B 型：Host によって振り分けられる。本体側の `/account` は 404 になる。同梱ログインのホストから Yuuka の API に届かない。
- authorize → callback → Yuuka セッション発行の一連の流れ（bundled ではプロセス内 exchange）。code の再利用、期限切れ、state の不一致を拒否する。
- ログアウト後のログインでは、パスキーを再び求められる（§6.2）。
- `auth.issuer` が食い違うと起動を止め、`relink` の後は Discord ID の一致で再び紐付く。
- external 方式の回帰：既存の `crates/yuuka-auth/tests/auth_flow.rs`。
- 手動 E2E：開発環境（`localhost`）と、dev 用サブドメインでの B 型。

## 10. 未決事項

- `AUTH_PROVIDER` の既定値。本方針では、現行本番との互換を優先して `external` にしている。新規インストールの迷いを無くすなら、未設定を起動エラーにする（明示を必須にする）案もある。
- A 型の既定パス `/account`。管理画面の「アカウント管理」（`/admin/account`）と名前が近い。`/auth` などに変えるかどうか。
- external 方式で sk-account 側の戻り先を設定化すること（sk-account リポジトリ側の変更）を、この作業に含めるかどうか。
