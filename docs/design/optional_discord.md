# Discord 連携の任意化 設計方針

- ステータス: 方針確定（2026-11-28・§9）。実装は §8 の順に進める
- 作成日: 2026-11-28
- 関連: [#234](https://github.com/kawaii-music-moe/yuuka/issues/234)（sk-account 以外の OIDC でログインできない）、`docs/design/bundled_login.md`（提案のみで閉じた同梱ログイン案）
- 主な対象: `crates/yuuka-db/migrations/V17__baseline.sql`（`users`）、`crates/yuuka-auth/src/{oidc.rs,users.rs,routes.rs,session.rs}`、`crates/yuuka-types/src/dto.rs`（`SessionUser`）、`crates/yuuka-services/src/notifier.rs`、`crates/yuuka-reminder/src/notify_target.rs`、`crates/yuuka-discord`、`frontend/src/lib/stores/session.ts`

## 1. 背景と目的

Yuuka には Web クライアント（PWA・管理画面）とデスクトップクライアントがある。それでも利用者は **Discord ID で識別** されていて、Discord アカウントがなければ使えない。

- `users` の主キーは `discord_id`（V17）。ほかの約 40 テーブルが `REFERENCES users(discord_id)` で参照している。
- OIDC ログインは、ID トークンに `discord_id` claim（`OIDC_DISCORD_ID_CLAIM`）が無いと拒否する（`oidc.rs` の `identity`）。この claim を出すのは sk-account だけ。
- `SessionUser { discord_id, username, role }` が全 API・WS・フロントエンドに行き渡っている（`discord_id|discordId` は 117 ファイル・525 か所）。
- 初期管理者の指定は `ADMIN_DISCORD_IDS` だけ。
- 通知は Discord の DM かチャンネルにしか届かない（`Notifier` は Discord Messenger へのアダプターだけ）。

**目的**: 利用者の識別を Discord から切り離し、Discord は「連携すると Discord 上の機能が使えるようになるオプション」にする。これで #234 の受け入れ条件（sk-9.net に頼らず、任意の OIDC プロバイダーで新しいインスタンスを立ててログインできる）も満たせる。

### 非目標

- パスワードやパスキーによる Yuuka 独自のログイン。本人認証は引き続き OIDC プロバイダーに任せる（同梱ログインは別途検討）。
- Discord 以外のチャット連携（Slack など）。ただし、通知の送り先を差し替えられる形にはしておく。

## 2. 方針の要点

1. **内部ユーザー ID を導入する。** `users.discord_id`（主キー）を `users.id` に改名する。既存ユーザーは「id = 旧 Discord ID」のまま引き継ぐので、参照側テーブルの値は書き換えない。新規ユーザーには UUID（v7）を振る。
2. **Discord ID は `users.discord_id`（NULL 可・一意）として別の列に持つ。** これが「連携済み」の印になる。
3. **ログインは OIDC の `(issuer, sub)` だけで成立させる。** `discord_id` claim があれば、ログイン時に自動で連携する（sk-account 利用者は今と同じ体験になる）。
4. **Discord 連携は、後から Discord OAuth2（`identify`）で付け外しできる。**
5. **通知に Web の送り先を足す。** Discord 未連携のユーザーや、Discord Bot が動いていないときは Web（アプリ内通知＋WS、将来は Web Push）に届ける。

## 3. データモデル

### 3.1 マイグレーション（V39 想定）

```sql
-- 主キー列を改名する。SQLite 3.26+ は他テーブルの REFERENCES users(discord_id) も追随して書き換える
-- （rusqlite は bundled なので条件を満たす。legacy_alter_table は OFF のまま）。
ALTER TABLE users RENAME COLUMN discord_id TO id;

-- Discord 連携（NULL = 未連携）
ALTER TABLE users ADD COLUMN discord_id TEXT;
UPDATE users SET discord_id = id;               -- 既存ユーザーは全員連携済み
CREATE UNIQUE INDEX idx_users_discord_id ON users(discord_id) WHERE discord_id IS NOT NULL;
ALTER TABLE users ADD COLUMN discord_linked_at TEXT;

-- （§8-4 で別のマイグレーションにする）OIDC の発行元。sk_account_id は oidc_sub に改名し、(issuer, sub) を一意にする
ALTER TABLE users RENAME COLUMN sk_account_id TO oidc_sub;
ALTER TABLE users ADD COLUMN oidc_issuer TEXT;
UPDATE users SET oidc_issuer = :current_issuer WHERE oidc_sub IS NOT NULL;   -- 起動時の設定値で埋める（§3.3）
DROP INDEX idx_users_sk_account_id;
CREATE UNIQUE INDEX idx_users_oidc ON users(oidc_issuer, oidc_sub) WHERE oidc_sub IS NOT NULL;
```

- 参照側の列名はすでに `user_id` / `owner_id` なので、改名は `users` 側だけで済む。
- テスト: 改名後に `PRAGMA foreign_key_check` が空であることと、`ON DELETE CASCADE` が効くことを確かめる。

### 3.2 ID の扱い

| 型 | 中身 | 使う場所 |
|---|---|---|
| `UserId`（既存の `yuuka_core::UserId`） | `users.id`。旧ユーザーは数字列、新ユーザーは UUID。**不透明な文字列として扱い、形式に意味を持たせない** | DB・API・WS・セッション・ツール |
| `DiscordUserId`（新設） | Discord snowflake | `yuuka-discord` と、Discord に送る境界だけ |

`UserId` を Discord API に直接渡しているところはコンパイルエラーで見つかるようにする。境界では `users.discord_id` を引いて変換する。

### 3.3 OIDC の発行元

- `oidc_issuer` は、マイグレーションを流したあと最初の起動時に、現在の `OIDC_ISSUER`（省略時は既定の sk-account）で埋める。refinery の SQL からは設定値が見えないため、supervisor の起動処理で一度だけ `UPDATE … WHERE oidc_issuer IS NULL` を実行する。
- 発行元を取り替えると別人として扱われる。既存ユーザーは「Discord 連携が一致すれば引き受ける」（§4.2）か、管理者が付け替える（§7）。

## 4. 認証フロー（yuuka-auth）

### 4.1 ログイン（`finish_login`）

1. ID トークンを検証する（現行どおり）。
2. `(iss, sub)` でユーザーを引く。
3. 見つからなければ、**claim の Discord ID**（任意・`OIDC_DISCORD_ID_CLAIM`）で、未紐付けの既存ユーザーを一度だけ引き受ける。現行の「未紐付けの行は Discord ID が一致すれば引き受ける」処理を一般化したもの。
4. それでも無ければ、新規ユーザーを作る（`id = UUIDv7`）。新規登録を許すかどうかは、次の設定で決める。
   - `REGISTRATION_MODE`: `open` / `invite`（招待コード。旧 `invite.rs` は V25 で廃止済みなので作り直す）/ `closed`。既定は `invite`。今は事実上、プロバイダー側で絞っている前提なので、Yuuka 側でも絞れるようにする。
5. claim に Discord ID があり、まだ未連携なら連携する。その Discord ID がほかのユーザーに連携済みなら、**統合はせず、ログインをエラーにする**（「この Discord アカウントは別のユーザーに連携されています」）。黙ってログインだけ通すと、管理者が衝突に気づけないため。
6. 表示名は `name` → `preferred_username` → `email` → 「ユーザー」の順に取る。

### 4.2 初期管理者

- `ADMIN_DISCORD_IDS` は残す（Discord を連携した時点でも昇格判定する）。
- `ADMIN_OIDC_SUBJECTS`（`sub` の列挙）と `ADMIN_EMAILS`（`email_verified=true` のときだけ有効）を追加する。
- 3 つとも空なら起動時に警告し、初期セットアップでは管理者を作れないことを案内する（#238 の方針「明示指定に限る」を守る）。

### 4.3 セッション

- `SessionUser.discord_id` を `user_id`（利用者 ID）に改名する（§8-3）。
- **送受信の名前は当面 `discordId` のまま据え置く。** Redis に残っているセッション値と、配布済みのデスクトップクライアント（`clients/desktop` の `UserInfo` が `discordId` を必須で読む）を壊さないため。中身は不透明な利用者 ID で、Discord 未連携の利用者では UUID になる。クライアントは識別子として扱うだけなので、これで動く。
- Discord 連携の状態は、`SessionUser` ではなく `GET /api/me` の `MeData` に `discordLinked`（と連携中の Discord ID）として足す（§8-6）。`userId` への改名は、配布済みクライアントが両方を読めるようになってから別途行う。
- `desktop_tokens` などの SQLite 側は、すでに `users` を JOIN しているので、列名を直すだけで済む。

## 5. Discord 連携（後付け）

- `GET /api/me/discord/link/start` → Discord OAuth2 の authorize（scope `identify`・state と PKCE は OIDC と同じ実装を流用）→ `GET /api/me/discord/link/callback`。
- 設定は `DISCORD_OAUTH_CLIENT_ID` / `DISCORD_OAUTH_CLIENT_SECRET` の 2 つ。未設定なら連携ボタンを出さない（OIDC の claim による自動連携は、引き続き動く）。
- 衝突: その Discord ID が別ユーザーに連携済みなら、エラー（409）にする。アカウントの統合は提供しない（§9）。
- 解除: `DELETE /api/me/discord`。条件は OIDC の紐付け（`oidc_sub`）があることだけ。これが無いとログイン手段がなくなるので拒否する。解除すると Discord 経由の機能（§6.2）が止まるので、画面で確かめてから実行する。テナント Bot のオーナー情報やメンバー申請など、Discord ID を前提にした行は消さずに残す。再び連携すれば、また使えるようになる。
- 監査ログ（`audit.rs`）に、連携・解除を記録する。

## 6. 機能ごとの扱い

### 6.1 通知（`yuuka-services::Notifier`）

- `NotifyTarget` に `Web` を足す。ユーザー既定（`users.notify_target_type`）の選択肢にも `web` を加える。
- 新設の `WebNotifier` は、アプリ内通知テーブル（`notifications`）へ保存し、接続中の WS（`/ws/chat`）へ配信する。PWA の Web Push は今回の範囲外（[#277](https://github.com/kawaii-music-moe/yuuka/issues/277)）。
- `DefaultTargetNotifier` の解決規則:
  - 既定が `dm`/`channel` で Discord 連携済み、かつ Discord Bot が稼働中 → Discord へ送る。
  - それ以外 → Web へ送る（現行の「配信できなければ pending のまま再試行」をやめ、Web に落とす）。
- 新規ユーザー（Discord 未連携）の既定は `web` にする。

### 6.2 Discord が前提の機能

| 機能 | 未連携のとき |
|---|---|
| テナント Bot（`yuuka-discord`）での会話・DM | 使えない。Bot 設定画面に「Discord を連携すると使えます」と出す |
| Bot へのメンバー申請・DM（V32・V33） | 申請の受け取りは Web でできる。DM 通知は Web 通知に置き換える |
| Discord 上で発生した入力（メッセージ・スラッシュコマンド） | `users.discord_id` から `UserId` を引く。引けない Discord ユーザーは、現行の「未登録ユーザー」と同じ扱い |
| Web クライアント・デスクトップ・MCP・Webhook | 影響なし（`UserId` で動く） |

Discord Bot（`yuuka-discord`）を起動しない構成も正式にサポートする。現在すでに `NullNotifier` で縮退しているので、それを設定（`DISCORD_ENABLED=false`、または Bot トークン未設定）で明示的に選べるようにし、起動ログで案内する。

### 6.3 フロントエンド・クライアント

- `session.ts` の `discordId` を `userId` に置き換え、`discordLinked`（または `discordId?`）を足す。
- 管理画面のアカウントに「Discord 連携」カードを置く（連携・解除・状態の表示）。
- Discord ID を前提にした画面（`BotDiscord.svelte` など）は、未連携なら案内を出す。
- `clients/desktop/src/model.rs` と `clients/pwa/src/api/auth.ts` も同じように直す。`ts-rs` の生成型は `SessionUser` を直せば追随する。

## 7. 運用・移行

- 既存の本番・dev: マイグレーションだけで移行できる（全員が連携済み、`id` は今の Discord ID のまま）。Redis セッションも §4.3 の互換で切れない。
- OIDC プロバイダーの付け替え: 新しいプロバイダーで初めてログインしたとき、claim の Discord ID が一致すれば引き継がれる。claim が出ないプロバイダーに移る場合に備えて、管理画面かサブコマンドで `oidc_sub` を付け替える手段を用意する（`yuuka users relink-oidc <user_id>`：次回ログインで引き受け可能な状態に戻す）。
- 文書: `docs/guide/setup.md` に「Discord なしで立てる最小構成」（OIDC と `ADMIN_OIDC_SUBJECTS` だけ）を書く。

## 8. 実装ステップ（PR 単位・すべて develop から）

1. **型の分離（挙動は変えない）**: `DiscordUserId` を新設し、Discord 境界での `UserId` → Discord ID の変換を関数 1 つに集める。この時点ではまだ恒等変換。
2. **DB の改名**: V39 で `users.discord_id`（主キー）を `id` に改名し、Discord 連携の列 `discord_id`（NULL 可・一意）と `discord_linked_at` を足す。SQL 文の `users.discord_id` を `users.id` に置き換える。既存利用者と、まだ Discord 必須のままのログインで作る利用者は `id = discord_id` なので、Discord 境界の変換（identity）は恒等のまま据え置き、§8-4 で `users.discord_id` の参照に切り替える。`oidc_issuer` と `sk_account_id` → `oidc_sub` の改名も §8-4 で行う。FK・CASCADE・トリガーの移行テストを足す。
3. **セッションと DTO**: `SessionUser.discord_id` を `user_id` に改名する（Rust 側のみ。送受信の名前は `discordId` のまま・§4.3）。フロントエンドとクライアントは変えない。
4. **OIDC の claim を任意にする**: 4a（§4.1・§4.2、`REGISTRATION_MODE` の `open` / `closed`、`ADMIN_OIDC_SUBJECTS` / `ADMIN_EMAILS`。Discord claim の無い ID トークンでログインできることを `auth_flow.rs` でテストする）と、4b（招待コードと `REGISTRATION_MODE=invite`。ここで既定を `invite` に切り替える）に分ける。4a の時点では、Discord ID 付きの新規利用者は `id = discord_id` のまま作る（Discord 境界の変換が DB を引くようになる §8-6 まで）。UUIDv7 は Discord の無い新規利用者に使う。
5. **Web 通知**: `notifications` テーブル、`WebNotifier`、既定の送り先の解決規則、WS 配信、画面の通知一覧。
6. **Discord 連携（後付け）**: OAuth2 による連携・解除 API と、アカウント画面。
7. **Discord なし構成の仕上げ**: `DISCORD_ENABLED`、Discord 前提画面の案内、setup.md・deployment.md、#234 のクローズ。

1〜3 は機械的な置き換えが中心で差分は大きいが、挙動は変わらない。4 以降で初めて、利用者から見える変化が出る。

## 9. 決定事項（2026-11-28 オーナー確認）

- 新規ユーザーの ID は **UUIDv7**（時刻順に並び、旧 ID の数字列と衝突しない）。
- `REGISTRATION_MODE` の既定は **`invite`**。
- Discord 連携の衝突（その Discord ID が別ユーザーに連携済み）は、**統合せずエラーにする**。後付け連携（§5）でも、ログイン時の自動連携（§4.1-5）でも同じ扱い。
- 連携の解除は、**OIDC の紐付け（`oidc_sub`）があれば許可する**。Discord ID を前提にした機能は、解除と同時に止まる（§6.2）。
- Web Push（PWA）は**今回の範囲に含めない**。[#277](https://github.com/kawaii-music-moe/yuuka/issues/277) で扱う。
