# セルフホスト化の設計（2026-11-11）

- ステータス: 提案。着手順と設定項目を先に決め、issue ごとの実装はこの文書に従う。
- 対象 issue: [#234](https://github.com/kawaii-music-moe/yuuka/issues/234)（ログイン）・[#235](https://github.com/kawaii-music-moe/yuuka/issues/235)（固有表記）・[#236](https://github.com/kawaii-music-moe/yuuka/issues/236)（規約）・[#237](https://github.com/kawaii-music-moe/yuuka/issues/237)（配布）・[#238](https://github.com/kawaii-music-moe/yuuka/issues/238)（初回管理者）・[#240](https://github.com/kawaii-music-moe/yuuka/issues/240)（上限）・[#241](https://github.com/kawaii-music-moe/yuuka/issues/241)（バックアップ）・[#242](https://github.com/kawaii-music-moe/yuuka/issues/242)・[#243](https://github.com/kawaii-music-moe/yuuka/issues/243)（手順書）・[#248](https://github.com/kawaii-music-moe/yuuka/issues/248)（タイムゾーン）・[#250](https://github.com/kawaii-music-moe/yuuka/issues/250)
- 到達点: **sk-9.net と kawaii-music.moe に一切頼らず、リリースの成果物だけで新しいインスタンスを立て、ログインして使い始められる。** 運営者が決めるのは Discord アプリ・ドメイン・規約・バックアップ先だけ。

## 1. 今の固有依存の棚卸し

| 種類 | 場所 | 何が固有か | issue |
|---|---|---|---|
| ログイン | `crates/yuuka-core/src/config.rs` `DEFAULT_OIDC_ISSUER` / `DEFAULT_OIDC_LABEL`、`crates/yuuka-auth/src/oidc.rs` `identity()` | 既定 issuer が `account.sk-9.net`。ID トークンの `discord_id` claim を出せるのは sk-account だけ | #234 |
| 表記 | `crates/yuuka-orchestrator/src/system_prompt.rs`（案内プロンプト末尾）、`frontend/src/overlays/Login.svelte`（仮の表示名）、`frontend/src/overlays/Usage.svelte` | 「sk-9 Account」「sk-account」の固定文 | #235 |
| デスクトップ | `clients/desktop/src/config.rs` | 接続先の例 `yuuka.kawaii-music.moe`、アプリ ID `moe.kawaii-music.yuuka-desktop` | #235 |
| 運用スクリプト | `deploy/prod/run-yuuka.sh`（Nix の Chromium を探す）、`deploy/prod/deploy-develop.sh`（develop 追従） | こちらの運用そのもの | #235 |
| 規約 | `frontend/src/overlays/Terms.svelte` / `Privacy.svelte` | 組み込み固定文。運営者名・連絡先が無い | #236 |
| 日時 | `crates/yuuka-core/src/datetime.rs` `jst()`、`crates/yuuka-google/src/http.rs`、`crates/yuuka-briefing/src/service.rs`（天気 URL）、`deploy/systemd/*.example` の `TZ=Asia/Tokyo` | JST 固定と `Local` の混在。利用者設定 `users.timezone` は未使用 | #248 |
| 管理者 | `crates/yuuka-auth`（初回ログイン） | `ADMIN_DISCORD_IDS` 未設定だと最初のログイン者が管理者 | #238 |
| 配布 | なし | ソースから作る以外に導入手段が無い。バージョンも無い | #237 |
| 資源 | 作成系 API 全般、`data/browser_profiles/` | 利用者ごとの上限が無い | #240 |
| バックアップ | `deploy/prod/deploy-develop.sh` の退避のみ | 定期バックアップが無い | #241（本文書 §6） |

## 2. 決めること

### 2.1 ログイン方式（#234）

Yuuka は利用者を Discord ID で識別する（Bot が DM を送る相手＝利用者）。この前提は変えない。

| 案 | 内容 | 運営者の負担 | 実装の負担 | 評価 |
|---|---|---|---|---|
| 1. sk-account を公開して同梱 | 別プロセス（Node）と Discord Bot をもう 1 つ | 大（もう 1 つのサービス） | 公開の準備・保守 | 見送り（#185 の判断と同じ） |
| 2. **Discord OAuth2 で直接ログイン** | Yuuka が Discord の `identify` scope で認可し、返ってきた `id` をそのまま Discord ID にする | 小（Bot 用に Discord アプリを既に作っている。同じアプリに OAuth2 のリダイレクト URL を足すだけ） | `yuuka-auth` に認可コードフロー 1 本（PKCE・state Cookie・トークン交換・`/users/@me`）。セッション発行は OIDC と同じ経路を再利用 | **採用（セルフホストの標準）** |
| 3. 汎用 OIDC ＋ ログイン後に Discord を紐付け | Google 等でログインし、初回に Discord OAuth で ID を結び付ける | 中（プロバイダーと Discord の両方） | 2 段階の状態管理 | 案 2 の上に将来足せる。今はやらない |

決定:
- `AUTH_PROVIDER` を新設し、`discord`（標準）と `oidc`（今の実装）を選べるようにする。
- `OIDC_ISSUER` の既定値（sk-9.net）を外す。`AUTH_PROVIDER=oidc` で issuer 未設定なら起動時にエラーにする（こちらの本番は config.yaml に issuer を明記する）。
- Discord OAuth2 の戻り先は `{BASE_URL}/api/auth/discord/callback`。`BASE_URL` 未設定（`http://127.0.0.1:3000`）でも動くようにし、手順書で「Discord Developer Portal の Redirects に同じ URL を登録する」と書く。
- Back-Channel Logout は OIDC だけ。Discord 側には無い。
- 既存の利用者行（`users.discord_id`）はそのまま使えるので、データ移行は不要。OIDC の `sub` との結び付き（`oidc_links` 相当）は `AUTH_PROVIDER=discord` では作らない。

### 2.2 表記と設定値の差し込み（#235・#236）

- 画面とプロンプトの「sk-9 Account」は、サーバーの `GET /api/auth/provider` が返す `label` だけから出す。`Login.svelte` の仮の値は空文字にし、取得できるまでボタンの文言を「ログイン」にする。`Usage.svelte` と案内プロンプトも同じ値を埋め込む（プロンプトは `build_system_bot_instruction` に `login_label` を渡す）。
- 運営者情報を設定に追加する: `OPERATOR_NAME`（表示名）、`OPERATOR_CONTACT`（連絡先 URL かメール）。規約・ポリシーのテンプレートはこの 2 つを差し込み、未設定なら「運営者情報が未設定です」と表示する。
- 規約・ポリシーは「テンプレート」と明示し、`TERMS_URL` / `PRIVACY_POLICY_URL` が設定されていれば組み込み画面の代わりにそのリンクへ送る（設定は既にある。挙動を手順書に書く）。本文は現行機能（会話ログ・シナプス記憶・音声・ブラウザプロファイル・Discord・OIDC）に合わせて書き直し、免責は「故意または重過失を除く」に改める。
- デスクトップクライアントは接続先を必須にし、例は `https://yuuka.example.com`。アプリ ID は既存の設定ディレクトリを壊さないよう据え置き、将来の変更時に移行処理を付ける。
- `run-yuuka.sh` の Nix 分岐は `CHROME_EXECUTABLE_PATH` の案内に置き換え、`deploy-develop.sh` は「develop 追従運用（開発者向け）」として残し、リリースからの更新手順（§2.5）を別に書く。

### 2.3 タイムゾーン（#248）

- 案 A を採る: サーバー全体で 1 つの TZ（プロセスの `TZ`＝`chrono::Local`）に統一し、JST 固定の 3 箇所（`input_to_db_jst`・`google_time_to_local`・天気 URL）を `Local` と `iana_time_zone`（ホストの TZ 名）に寄せる。
- systemd ユニットの `TZ=Asia/Tokyo` は例として残すが、「ホストに合わせて変える」と注記する。
- 利用者設定の「タイムゾーン」欄は、使われるようになるまで画面から外す（DB 列は残す）。
- テストは `TZ` に依存しないよう、固定オフセットで組み立てる既存の形を保つ。

### 2.4 初回の管理者（#238）

- `ADMIN_DISCORD_IDS` 未設定で最初のログイン者を管理者にする挙動は、公開インスタンスでは危険。`YUUKA_INIT_DB=1` と同じ「明示オプトイン」に揃え、未設定なら管理者を作らず、起動ログと `/api/me` の `setupRequired` で案内する。
- 既存インスタンスは管理者が居るので影響なし。

### 2.5 配布（#237）

- タグ `vX.Y.Z` で GitHub Actions が次を作る: `yuuka-vX.Y.Z-linux-x86_64.tar.gz`（`yuuka` バイナリ、`dist/public`、`deploy/systemd/*.example`、`deploy/prod/config.yaml.example`、`docs/guide/*.md`）。
- バイナリに `yuuka --version` を足す（`CARGO_PKG_VERSION` ＋ git sha）。`/api/status` にも版を出す。
- `CHANGELOG.md` を置き、マイグレーション・設定キーの追加・互換性のない変更を節で分ける。
- 手順書に「リリースから更新する」節を足す: サービス停止 → `yuuka backup`（§6）→ 成果物の展開 → 起動（マイグレーションは起動時に自動）→ 確認。
- コンテナは要望が出てから（Docker 構成を廃止した経緯があるので、最初は tar.gz だけ）。

## 3. 設定項目の追加一覧

| キー | 既定 | 用途 | 節 |
|---|---|---|---|
| `AUTH_PROVIDER` | `discord` | `discord` / `oidc` | 2.1 |
| `DISCORD_OAUTH_CLIENT_ID` / `DISCORD_OAUTH_CLIENT_SECRET` | なし（`discord` では必須） | Discord アプリの OAuth2 | 2.1 |
| `OIDC_ISSUER` | **なし**（今は sk-9.net） | `oidc` のとき必須 | 2.1 |
| `OIDC_LABEL` | issuer のホスト名 | ログインボタン | 2.2 |
| `OPERATOR_NAME` / `OPERATOR_CONTACT` | なし | 規約・ポリシー・フッター | 2.2 |
| `TZ` | ホストの設定 | 日時の解釈 | 2.3 |
| `ADMIN_DISCORD_IDS` | なし | 初回管理者（未設定なら作らない） | 2.4 |
| `YUUKA_LIMIT_BOTS` / `YUUKA_LIMIT_WEBHOOKS` / `YUUKA_LIMIT_PLAYBOOK_SCHEDULES` / `YUUKA_LIMIT_REMINDERS` / `YUUKA_LIMIT_PERSONAS` | なし（上限なし） | 利用者ごとの件数上限 | 5 |
| `YUUKA_SCHEDULE_MIN_INTERVAL_MINUTES` | `0` | 定期実行の最短間隔 | 5 |
| `YUUKA_BROWSER_PROFILE_TTL_DAYS` | `30` | 使われないブラウザプロファイルの掃除 | 5 |

すべて `config.yaml` と環境変数の両方で指定できる（`Config::load_and_validate` の既存ルール）。`.env.example` と `docs/guide/setup.md` の一覧を同時に更新する。

## 4. 着手順序と依存関係

| 段階 | 内容 | 依存 | 規模 |
|---|---|---|---|
| 0 | 足場: TZ 統一（#248 案 A）、Node 版の記述統一（#250）、`yuuka --version`、`yuuka backup`（§6） | なし | 小〜中 |
| 1 | ログイン: `AUTH_PROVIDER` と Discord OAuth2（#234）、初回管理者のオプトイン（#238） | 0 の `--version` は不要。手順書（#243）と同時に書く | 中 |
| 2 | 表記と規約: ラベルの差し込み、運営者情報、規約テンプレートの書き直し（#235・#236） | 1（ラベルの出どころが `AUTH_PROVIDER` で変わる） | 中 |
| 3 | 資源: 利用者ごとの上限・最短間隔・プロファイル掃除（#240） | なし。1・2 と並行可 | 中 |
| 4 | 配布: リリース workflow・CHANGELOG・更新手順（#237） | 0〜3 がひととおり入ってから初版タグ | 中 |

段階 1 と 3 は触るファイルが重ならないので並行できる。段階 2 はログインの実装が終わってから。

## 5. 利用者ごとの上限（#240）の設計

### 5.1 件数の上限

- `yuuka-core` の `Config` に `limits: Limits { bots, webhooks, playbook_schedules, reminders, personas: Option<u32> }` を足す（`None`＝上限なし）。
- 各ドメインの repo に `count_for_user(user_id) -> u32` を 1 本ずつ足す（既にある一覧クエリの `count(*)` 版）。
- 作成ハンドラ（`POST /api/bots`（作成）・`/api/webhooks/create`・`/api/playbooks/schedules/save`（新規のときだけ）・`/api/reminders/add`・`/api/personas/save`（新規のときだけ））と、同じ操作をする LLM ツール（`addReminder` 等）の入口で、`count >= limit` なら `DomainError::Invalid("…は N 件までです。")` を返す。ツール側は `{success:false}` に写る（既存の `DomainError` の変換に乗る）。
- 上限は「利用者 × 全 Bot の合計」で数える。Bot ごとに分けると Bot を増やして回避できるため。
- 管理者も同じ上限に従う（管理者だけ別にする理由が無い。必要なら後で `YUUKA_LIMIT_ADMIN_*` を足す）。

### 5.2 定期実行の最短間隔

- `cron_util` に `min_interval(expr) -> Option<Duration>`（次の 2 回の発火の差。`*` を含む分・時のパターンだけ見る簡易版で十分）を足す。
- Playbook スケジュール保存と `REMINDER_CRON` 以外の利用者由来 cron（リマインドの `repeat_rule`、定期報告の `schedule_cron`）の登録時に、`YUUKA_SCHEDULE_MIN_INTERVAL_MINUTES` 未満なら 400 で断る。既存の行は変えない（実行時に止めると黙って動かなくなるため）。

### 5.3 ブラウザプロファイルの掃除

- `yuuka-services` に `BrowserProfileCleanupService`（毎日 1 回）を足し、`data/browser_profiles/<利用者>` の最終更新が `YUUKA_BROWSER_PROFILE_TTL_DAYS` より古く、かつその利用者の対話セッションが開いていなければ削除する。
- 利用者の削除（アカウント削除）時にもプロファイルを消す。今は残る。

## 6. DB のバックアップ（#241）

実装済み（この文書と同時に入れた）。

- `yuuka backup [--to <dir>] [--keep <n>] [--db <path>]`: `VACUUM INTO` で一貫したスナップショットを `yuuka-YYYYMMDD-HHMMSS.db` として書き、`PRAGMA integrity_check` を通し、持ち主だけが読める権限にし、古い世代を `--keep` 件（既定 14）だけ残して消す。サービスを止める必要は無い（WAL の読み取りスナップショット）。
- `deploy/systemd/yuuka-backup.service.example` と `yuuka-backup.timer.example`（毎日 04:00）。
- 戻し方と、`secret.key` が無いと暗号化された値を読めないことは `docs/guide/deployment.md` の「DB のバックアップ」に書いた。

## 7. 受け入れ条件（全体）

- [ ] `AUTH_PROVIDER=discord` で、sk-9.net を一切使わずにログインできる。
- [ ] 利用者に見える文言とプロンプトに `sk-9` / `kawaii-music.moe` が無い（`grep -rn 'sk-9\|kawaii-music' crates frontend clients --include=*.rs --include=*.svelte --include=*.ts --include=*.vue` が docs 以外で空）。
- [ ] `TZ` を `UTC` にしたホストで、予定・リマインド・定期報告が同じ基準の時刻で動く。
- [ ] リリースの tar.gz を展開し、手順書どおりに設定して起動すると、初回管理者の登録まで進む。
- [ ] `yuuka backup` の成果物から別のホストで起動できる（`secret.key` を一緒に移した場合に限り、保存済みトークンも使える）。
