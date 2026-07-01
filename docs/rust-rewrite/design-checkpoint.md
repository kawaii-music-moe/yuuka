# yuuka Rust移行計画 — 中間チェックポイント

> ワークフロー wf_456f35ec-ec8 の中断時スナップショット。
> 完了済み 22 エージェント分の生出力（Understand棚卸し / Design設計 / Critique検証）を保存。
> 最終統合(Synthesize)は未完了。resume で完了可能（本文末尾の手順参照）。

---

## [01] I now have a complete, verified picture of the DB layer. Producing the inventory
<sub>agentId: a0ce23436c6e81cc2</sub>

I now have a complete, verified picture of the DB layer. Producing the inventory.

---

# yuuka DB層 棚卸し（Rust移植論点つき）

基準ディレクトリ: `/home/suki/web/kawaii-music.moe/apps/yuuka/`

## 0. アーキテクチャ全体像

- SQLite が「正 (source of truth)」。`better-sqlite3` による**同期**呼び出し。書き手は Node のみ。
- Redis はキャッシュ/セッション/レート制限等の**揮発層**。障害時は SQLite または in-memory へフォールバックし、起動をブロックしない。
- スキーマ定義の唯一の源は `src/db/migrations.ts`（各 Repo はこれを前提に prepared statement を発行するのみ）。

---

## 1. 接続・PRAGMA・シングルトン (`src/db/database.ts`)

- `getDb()` が遅延シングルトン。初回に `config.dbPath` の親ディレクトリを `fs.mkdirSync(recursive)` で作成。
- `new Database(config.dbPath)` 後、**PRAGMA を2つだけ**設定:
  - `journal_mode = WAL`（`database.ts:17`）
  - `foreign_keys = ON`（`database.ts:18`）
- `busy_timeout` / `synchronous` の明示設定は**無い**（既定値に依存）。
- `closeDb()` で `db.close()` + シングルトン解除。

---

## 2. スキーマ versioning (`src/db/migrations.ts`)

### 方式
- `const SCHEMA_VERSION = "17"`（`migrations.ts:12`）。バージョンは `system_settings` テーブルの `key='schema_version'` に文字列で保存（`migrations.ts:1575`）。
- `getCurrentSchemaVersion()` は `system_settings` 不在時 `"1"` を返す（初回起動判定）。

### ⚠ DROP/再作成というロスありな挙動（ロスの場所）
1. **レガシー v1 全 DROP**（`migrations.ts:880-894`）: `currentVersion !== SCHEMA_VERSION && hasAnyLegacyTable && (currentVersion==="1" || hasLegacyUsers)` の条件で、`LEGACY_TABLES`（`users, bots, tasks, expenses, credentials, ...` 全16テーブル、`migrations.ts:15-32`）を `foreign_keys=OFF` 下で `DROP TABLE IF EXISTS`。**旧データは破棄**（コメントでも明言）。`hasLegacyUsers` は `users` に `salt` 列が無いことで旧スキーマを検出。
2. **`mcp_servers` FK欠落時の DROP 再作成**（`migrations.ts:1465-1470`）: FKの無い旧定義を検出したら `DROP TABLE`。登録済みMCPサーバーは**消える**（コメント「再登録が必要」）。
3. `message_logs` FK撤廃再構築（`migrations.ts:1109-1135`）はデータ保持型（rebuilt へ id明示コピー → DROP → RENAME）。データロスなし。

### 冪等ヘルパー
- `ensureColumns(db, table, defs[])`（`migrations.ts:38-51`）: `PRAGMA table_info` で列存在を確認し、無ければ `ALTER TABLE ADD COLUMN`。テーブル未作成なら no-op。
- `hasColumn` / `tableExists`（`migrations.ts:54-69`）。
- 段階マイグレーション関数群（全て冪等・`IF NOT EXISTS`・marker or 列/テーブル存在で移行済み判定）:
  - `migrateToBotScopedData`（v3, `:76`）— SIMPLE_TABLES は列追加、`budget_limits/context_notes/briefing_configs/report_configs/playbooks` は PK/UNIQUE に user_id を含むため `_rebuilt` テーブルへコピー再構築。
  - `migrateMcpToBotScope`（v4, `:220`）— `bot_id` 追加 + `bot_mcp_links` から backfill後 `DROP`。
  - `migrateMcpAccessOwnerScope`（v7, `:452`）— `bot_mcp_access` に `owner_id` 追加（クロステナント権限昇格の修正、v2テーブル入替）。
  - `migrateOwnerResourceGrants`（v5, `:325`）— `system_settings` の marker `v5_grants_backfilled` で**一度きり**保護 + `db.transaction` で包む。
  - `migrateBotGoogleAccountNullable`（v6）, `migratePersonaToBotScope`（v8, テーブル存在で判定）, `migrateToSynapseEngine`（v10）, `migrateToTaskProgress`（v12）, `migrateToDesktopTokens/MemberRequests/AllowedRoles/RoutineTasks`（v13-16）, `migrateToTimeline`（v17）。
- 実行順序に依存関係あり（`migrations.ts:1526-1572` のコメント参照。例: v7 を v5 より前に、複合インデックスは `bot_id` 列保証後）。

---

## 3. 全テーブル定義（migrations.ts の CREATE TABLE 全拾い）

| # | テーブル | 主要カラム / 型 | 制約・インデックス | 用途 |
|---|---|---|---|---|
| 1 | `system_settings` (`:848`) | `key TEXT PK`, `value TEXT NOT NULL`, `created_at/updated_at` | PK(key) | KV設定・schema_version・移行marker・コンテキストfloor |
| 2 | `users` (`:898`) | `discord_id TEXT PK`, `username`, `password_hash`(bcrypt12), `salt`(Argon2id用), `role`, `gemini_api_key_encrypted/iv/tag`, `gemini_model`, `google_refresh_token_*`, `google_calendar_id`, `google_calendars`, `active_persona_id`(レガシー), `timezone`, `backup_*` | UNIQUE INDEX `idx_users_username` | ユーザー・認証・個人設定 |
| 3 | `bots` (`:936`) | `id TEXT PK`, `user_id`(owner), `name`, `discord_token_encrypted/iv/tag`, `suspended`, `stopped`, `capabilities`(JSON), `persona_id`, `gemini_api_key_*`, `enabled_modules`(JSON) | FK user_id→users CASCADE; INDEX user | Botインスタンス |
| 4 | `bot_shares` (`:964`) | `id PK`, `bot_id`, `owner_id`, `shared_user_id`, `status` | UNIQUE(bot_id,shared_user_id); FK bot_id CASCADE; INDEX(shared_user_id,status) | Bot共有招待 |
| 5 | `bot_user_modules` (`:1003`) | `bot_id`, `user_id`, `enabled_modules`(JSON) | PK(bot_id,user_id); FK bot_id CASCADE | ユーザー×Bot機能モジュール上書き |
| 6 | `bot_context_notes` (`:1019`) | `bot_id`, `user_id`, `content` | PK(bot_id,user_id); FK bot_id CASCADE（**user_idにFKなし**） | Bot個人ノート |
| 7 | `bot_guild_notes` (`:1029`) | `bot_id`, `guild_id`, `content` | PK(bot_id,guild_id); FK bot_id CASCADE | ギルド共有ノート |
| 8 | `bot_guilds` (`:1039`) | `bot_id`, `guild_id` | PK(bot_id,guild_id); FK CASCADE | 応答許可ギルド |
| 9 | `bot_members` (`:1048`) | `bot_id`, `guild_id`, `user_id`, `added_by` | PK(bot_id,guild_id,user_id); FK CASCADE | 利用メンバー |
| 10 | `personas` (`:1061`) | `id PK`, `owner_id`, `name`, `prompt`, `is_public` | FK owner_id CASCADE; INDEX owner, public | ペルソナ |
| 11 | `message_logs` (`:1079`) | `id PK`, `user_id`, `bot_id`, `discord_msg_id`, `role`, `content`, `reply_to_msg_id`, `guild_id` | **FKなし**（未登録メンバー記録のため）; INDEX(user_id,id), discord_msg, (bot_id,guild_id,id) | 会話履歴（正） |
| 12 | `message_logs_fts` (`:1143`) | FTS5 virtual, `content`, `content='message_logs'`, `tokenize='trigram'` | トリガ ai/ad/au で同期 | 全文検索 |
| 13 | `todos` (`:1164`) | `id PK`, `user_id`, `title`, `due_date`, `start_date`, `priority`, `tags`(JSON), `status`, `progress`, `parent_id`(自己参照), `linked_payment_id`, `due_reminded`, `repeat_rule/until/count` | FK user_id CASCADE, FK parent_id→todos CASCADE; INDEX(user_id,status), parent | ToDo/タスク進捗/ルーチン |
| 14 | `schedules` (`:1193`) | `id PK`, `user_id`, `title`, `start_at`, `end_at`, `remind_before_minutes`, `reminded`, `google_event_id`, `google_calendar_id` | FK CASCADE; INDEX user, (reminded,start_at) | 予定(Googleカレンダー同期) |
| 15 | `reminders` (`:1213`) | `id PK`, `user_id`, `message`, `trigger_at`, `repeat_rule`, `target_type/id`, `status`, `source`, `source_id` | FK CASCADE; INDEX(status,trigger_at), (user_id,status) | リマインド |
| 16 | `expenses` (`:1233`) | `id PK`, `user_id`, `type`, `amount INTEGER`(円), `category`, `memo`, `date`, `time`, `source` | FK CASCADE; INDEX(user_id,date), (user_id,category,date) | 家計 |
| 17 | `budget_limits` (`:1249`) | `user_id`, `bot_id`, `category`, `limit_amount` | PK(user_id,bot_id,category); FK CASCADE | 予算上限 |
| 18 | `planned_payments` (`:1259`) | `id PK`, `user_id`, `title`, `amount`, `category`, `due_date`, `repeat_rule`, `status`, `settled_expense_id`, `linked_todo_id`, `linked_reminder_id` | FK CASCADE; INDEX(user_id,status,due_date) | 支払い予定 |
| 19 | `playbooks` (`:1281`) | `id PK`, `user_id`, `bot_id`, `name`, `title`, `keywords`(JSON), `steps` | UNIQUE(user_id,bot_id,name); FK CASCADE; INDEX user | マクロ |
| 20 | `playbook_schedules` (`:1297`) | `id PK`, `user_id`, `bot_id`, `playbook_name`, `cron_expression`, `enabled`, `last/next_run_at` | UNIQUE(user_id,playbook_name); FK CASCADE; INDEX user | マクロ定期実行 |
| 21 | `playbook_runs` (`:1314`) | `id PK`, `schedule_id`, `user_id`, `playbook_name`, `status`, `output`, `started/finished_at` | FK schedule_id CASCADE; INDEX user | マクロ実行ログ |
| 22 | `context_notes` (`:1330`) | `user_id`, `bot_id`, `content` | PK(user_id,bot_id); FK CASCADE | 秘書コンテキストノート |
| 23 | `clipboard_entries` (`:1339`) | `id PK`, `user_id`, `content`, `expires_at` | FK CASCADE; INDEX user, expires | クリップボード |
| 24 | `contacts` (`:1350`) | `id PK`, `user_id`, `name`, `birthday`, `relationship`, `tags`(JSON), `birthday_reminded_year` | FK CASCADE; INDEX user | 連絡先 |
| 25 | `credentials` (`:1369`) | `user_id`, `service_name`, `url`, `username`, `encrypted_password`, `iv`, `auth_tag` | PK(user_id,service_name); FK CASCADE | パスワードマネージャ（ユーザー鍵AES-GCM） |
| 26 | `webhook_endpoints` (`:1385`) | `id PK`, `user_id`, `name`, `token UNIQUE`, `secret_encrypted/iv/tag`, `notify_target_*`, `create_todo/reminder`, `enabled` | UNIQUE(token); FK CASCADE; INDEX user | 外部Webhook受信 |
| 27 | `webhook_deliveries` (`:1405`) | `id PK`, `endpoint_id`, `user_id`, `payload`, `status`, `detail` | FK endpoint_id CASCADE; INDEX endpoint | Webhook配信ログ |
| 28 | `briefing_configs` (`:1420`) | `user_id`, `bot_id`, `enabled`, `schedule_cron`, `weather_lat/lng`, `news_feeds/keywords`(JSON) | PK(user_id,bot_id); FK CASCADE | 朝報 |
| 29 | `report_configs` (`:1437`) | `id PK`, `user_id`, `bot_id`, `type`, `enabled`, `schedule_cron` | UNIQUE(user_id,bot_id,type); FK CASCADE | 日報/週報 |
| 30 | `mcp_servers` (`:1472`) | `id PK`, `user_id`(NULL=システム), `bot_id`(退役), `name`, `endpoint_url`, `auth_credential_encrypted/iv/tag`, `tools_cache`, `requires_confirmation`, `enabled` | FK user_id CASCADE; INDEX user, (user_id,bot_id) | MCPサーバー |
| 31 | `audit_logs` (`:1497`) | `id PK`, `user_id`, `action`, `target`, `detail` | **FKなし**; INDEX(user_id,created_at) | 監査ログ |
| 32 | `invite_codes` (`:1510`) | `code PK`, `created_by`, `used_by`, `used_at`, `revoked_at` | PK(code) | 招待コード |
| 33 | `bot_mcp_access` (`:331` v5, v7で再構築) | `bot_id`, `owner_id`, `mcp_server_id` | PK(bot_id,owner_id,mcp_server_id); FK×3 CASCADE; INDEX server, (bot_id,owner_id) | MCP許可リスト（owner分離） |
| 34 | `bot_credential_access` (`:345`) | `bot_id`, `owner_id`, `service_name` | PK(bot_id,owner_id,service_name); FK bot_id/owner_id CASCADE | 認証情報許可リスト |
| 35 | `user_google_accounts` (`:357`) | `id PK`, `user_id`, `email`, `refresh_token_encrypted/iv/tag`, `calendar_id`, `calendars`, `is_primary` | UNIQUE(user_id,email); FK CASCADE; INDEX user | Google複数アカウント |
| 36 | `bot_google_account` (`:378`, v6でnullable化) | `bot_id PK`, `google_account_id`(NULL可) | PK(bot_id); FK bot_id/google_account_id CASCADE | Bot使用Googleアカウント選択 |
| 37 | `bot_active_personas` (`:562` v8) | `user_id`, `bot_id`, `persona_id` | PK(user_id,bot_id); FK user_id/persona_id CASCADE; INDEX persona | 適用ペルソナ(Bot単位) |
| 38 | `tool_outcomes` (`:610` v10) | `id PK`, `user_id`, `bot_id`, `guild_id`, `topic_id`, `synapse_id`, `tool_name`, `args_digest`, `status`, `latency_ms` | **FKなし**; INDEX(user_id,bot_id,tool_name), (…,topic_id) | ツール実行実績（経験） |
| 39 | `topic_tool_stats` (`:628`) | `user_id`, `bot_id`, `topic_id`, `tool_name`, `success`, `total`, `success_rate REAL` | PK(user_id,bot_id,topic_id,tool_name) | トピック別ツール勝率 |
| 40 | `synapses` (`:642`) | `id PK`, `user_id`, `bot_id`, `guild_id`, `content`, `topic_id`, `source_msg_id`, `embedding BLOB`(float32 LE), `embedding_model_version`, `decay_score REAL`, `ctx_tod`, `ctx_dow` | **FKなし**; INDEX(user_id,bot_id), (…,topic_id) | L2連想記憶（Rustエンジンがread-only消費） |
| 41 | `task_progress_logs` (`:690` v12) | `id PK`, `user_id`, `bot_id`, `todo_id`, `progress`, `note` | FK user_id/todo_id CASCADE; INDEX(todo_id,created_at) | 進捗ログ |
| 42 | `desktop_tokens` (`:732` v13) | `id PK`, `user_id`, `token_hash UNIQUE`(sha256), `device_name`, `revoked` | UNIQUE(token_hash); FK CASCADE; INDEX user | デスクトップ長命トークン |
| 43 | `bot_member_requests` (`:753` v14) | `id PK`, `bot_id`, `guild_id`, `user_id`, `status`, `decided_by` | UNIQUE(bot_id,guild_id,user_id); FK bot_id CASCADE; INDEX(bot_id,status), user | 利用申請 |
| 44 | `bot_roles` (`:780` v15) | `bot_id`, `guild_id`, `role_id`, `role_name`, `added_by` | PK(bot_id,guild_id,role_id); FK bot_id CASCADE | 許可ロール |
| 45 | `day_plan_blocks` (`:800` v17) | `id PK`, `user_id`, `bot_id`, `date`, `start/end_time`, `type`, `title`, `todo_id`, `transit_*`, `position` | **FKなし**; INDEX(user_id,bot_id,date) | 計画ブロック |
| 46 | `timeline_records` (`:820` v17) | `id PK`, `user_id`, `bot_id`, `date`, `type`, `content`, `todo_id`, `expense_id`, `amount REAL`, `media_path`, `location` | **FKなし**; INDEX(user_id,bot_id,date) | 汎用記録層 |

**廃止テーブル**: `bot_mcp_links`（v4でDROP）。

**FKを意図的に張らないテーブル**（`user_id` に Web未登録の Discord ユーザーIDが入り得るため）: `message_logs`, `tool_outcomes`, `synapses`, `topic_tool_stats`, `bot_context_notes`(user_id側), `bot_user_modules`(user_id側), `day_plan_blocks`, `timeline_records`, `audit_logs`。

---

## 4. リポジトリパターン (`src/db/*Repo.ts`)

- **prepared statement**: 各メソッドで `getDb().prepare("... WHERE user_id = ?").get/all/run(...)`。ステートメントはキャッシュ保持せず**呼び出しごとに `prepare`**（better-sqlite3 内部で SQL文字列→ステートメントはキャッシュされるが、明示的な事前 prepare 定数化はしていない）。件数目安: `botRepo` 26、`userRepo` 25、`todoRepo` 25（`grep -c` より）。
- **同期呼び出し**: `.get()`/`.all()`/`.run()` すべて同期。Repo関数の大半は非 async（例 `getUserByDiscordId`）。async なのは Redis を併用する `messageLogRepo` 等のみ。
- **トランザクション**: `db.transaction(() => {...})` を使うのは `userRepo`(createUser)、`personaRepo`、`plannedPaymentRepo`、`todoRepo`、`toolOutcomeRepo`、`googleAccountRepo`、`migrations`。better-sqlite3 のトランザクションは**同期関数を包む**形式。
- **WAL / foreign_keys**: `database.ts` の接続時 PRAGMA で全体設定。移行の再構築中のみ `PRAGMA foreign_keys = OFF/ON` を局所的に切り替え。
- **JSON列**: `tags`/`capabilities`/`enabled_modules`/`calendars` 等は TEXT に JSON文字列で格納、Repo層で `JSON.parse` を try/catch し不正時は空配列フォールバック（例 `userRepo.ts:397-404`, `todoRepo.ts:115-120`）。
- **暗号化**: 暗号化/復号は Repo外（`llmClient`/`crypto.ts` 呼び出し側）。Repoは暗号文＋iv＋tagを素通し保存。

---

## 5. userId によるデータ分離の強制

- **原則（コメントで各Repo冒頭に明記）**: 「全クエリは `user_id`(DiscordユーザーID) を WHERE 必須条件とする」（`credentialRepo.ts:6`, `todoRepo.ts:6` 等）。
- 強制は**規約ベース**であり、DBレベルの Row-Level-Security は無い。各 prepared statement が手書きで `WHERE user_id = ?`（複合スコープは `AND bot_id = ?`）を含むことで担保。
- **PK/UNIQUE による構造的分離**: `credentials` PK(user_id,service_name)、`context_notes` PK(user_id,bot_id) 等、複合キーに user_id を含める。
- **削除の伝播**: `users` 削除時、FK `ON DELETE CASCADE` で関連が連鎖削除（`deleteUser`, `userRepo.ts:243`）。ただしFKを張らないテーブル（message_logs等）は**孤児化する**（コメントで許容と明記）。
- **例外（cross-user 走査）**: cron系の全ユーザー横断関数のみ user_id 無しを許容。`listOpenTodosDueWithinAcrossUsers`, `listOverdueRecurringAcrossUsers`, `listOverdueRoutinesAcrossUsers`, `listPendingDueWithinAcrossUsers`, `listEnabledBriefingConfigsAcrossUsers`, `listEnabledReportConfigsAcrossUsers`, `listAllUserIds`（`userRepo.ts:194`）。命名規約 `AcrossUsers` で明示。
- **クロステナントの過去の穴と修正**: `bot_mcp_access` に当初 `owner_id` が無く、共有秘書(`system_default`)経由で他ユーザーのMCP認証情報が全会話へ注入され得た（`migrations.ts:441-450` のコメント）。v7 で `owner_id` 次元を追加して修正。ランタイム解決も `mcpRepo` で発話者スコープ（`owner_id = speakerUserId`）＋システムレベル(`user_id IS NULL`)のみに限定（`mcpRepo.ts:96-112`）。
- **MCP削除権限**: 本人登録は本人のみ、システムレベル(`user_id IS NULL`)はAdminのみ（`mcpRepo.ts:243-245`）。

---

## 6. Redis の用途とフォールバック (`src/db/redis.ts` + 各サービス)

### 接続・障害耐性 (`redis.ts`)
- `initRedis()`: 再接続を**永遠に諦めない** reconnectStrategy（指数バックオフ上限30s、`redis.ts:17-29`）。初回接続は**5秒だけ待って**未接続でも起動続行（`Promise.race`、`redis.ts:59-70`）。
- `getRedisClient()`: `isRedisReady && client` の時のみ実クライアント、それ以外は **`null` を返す**（各利用側が null チェックでフォールバック分岐）。

### 用途とフォールバック一覧

| 用途 | ファイル | Redisキー | フォールバック |
|---|---|---|---|
| **セッション** | `services/sessionService.ts` | `session:{sha256(token)}`, `user_sessions:{userId}`(Set) | **in-memory Map**（`memorySessions`/`memoryUserSessions`, `:45-47`）。10分毎 sweep。Redis稼働中でもキー不在時 in-memory も確認（移行救済）。TTLスライディング。 |
| **会話コンテキストキャッシュ** | `db/messageLogRepo.ts` | `context:{botId}:secretary:{userId}` 他（rPush/lTrim, 直近15/30件, TTL=session連動） | **SQLite から再構築**（`WHERE user_id=? AND bot_id=? AND id>floor`, `:209`）。floor は `context_floor:*`（system_settings）でリセット境界管理。書込失敗は警告のみ（SQLiteが正）。 |
| **レート制限** | `services/botRateLimit.ts` | `mcp_rate:{botId}:{userId}:m/d`, `mcp_rate_guild:*`（incr+expire） | **in-memory カウンタ Map**（`memoryCounters`, `:39`）。「再起動でリセットするが防衛線として十分」。SQLiteには持たない。 |
| **OAuth state(CSRF nonce)** | `utils/oauthStateStore.ts` | `oauth_state:{state}`(TTL10分) | **in-memory Map**（`memoryStates`, `:22`）。 |
| **操作履歴(マクロ種)** | `services/actionRecorder.ts` | `fc_history:{userId}`(直近30件, TTL2h) | **in-memory Map**（`memoryFallback`, `:40`）。 |
| **デバイス認証** | `services/desktopAuthService.ts` | デバイスコード一時状態 | Redis揮発（永続化するのは `desktop_tokens` の発行済みトークンのみ、`migrations.ts:730` コメント） |

- `system_settings`（SQLite）はレート制限の閾値設定 (`mcp_rate_*`) やコンテキストfloorのバッキングストアも兼ねる（`systemSettingsRepo.ts`）。

---

## 7. Rust移植の論点

### A. 同期 SQLite を async world でどう扱うか
- 現状は `better-sqlite3` の**完全同期**API。Repo関数の大半が非async・即値return で、呼び出し側は「DBは即座に返る」前提で書かれている（トランザクションも同期クロージャ前提）。
- Rustでの選択肢と論点:
  - `rusqlite`（同期）を **`tokio::task::spawn_blocking`** でラップ → 現状のセマンティクス（同期・単一書き手）を最も忠実に保てる。WALと相性良。Repo境界を `async fn` にして内部で spawn_blocking する設計が移植コスト最小。
  - `sqlx`（async, compile-time checked）はDDLの動的 `ALTER`/`PRAGMA table_info` ベースの冪等移行と相性が悪い（このコードは実行時スキーマ内省が多い）。移行ロジックは `rusqlite` 直叩きが無難。
  - **単一書き手の維持が重要**: 現状「書き手は Node のみ」で並行書込を想定していない。Rust側で複数タスクから同時書込するなら、書込専用に**単一コネクション + Mutex/actor** に集約するか、`busy_timeout` を明示設定する必要がある（現状 `database.ts` は `busy_timeout` 未設定 = 並行時に即 `SQLITE_BUSY` の危険）。
  - `synapses.embedding` は float32 LE の BLOB で、既に「Rustシナプスエンジンが read-only 消費」を想定した設計（`migrations.ts:604-605`）。Rust移植時はこのread-only境界（索引喪失は性能劣化でありdata lossでない）を跨ぐ書込者が二重にならないよう注意。
- **トランザクション**: `db.transaction(fn)` の同期クロージャは `spawn_blocking` 内の1クロージャへ自然に対応。ネスト無し・短命なので移植容易。

### B. DROP/再作成の危険性
- **最大のリスクは `migrations.ts:880-894` のレガシー全DROP**。条件は `currentVersion==="1" || hasLegacyUsers`（`users`に`salt`列が無い）。Rust移植で `system_settings` を引き継がずに初期化すると `getCurrentSchemaVersion` が `"1"` を返し、**既存の本番テーブルが `hasAnyLegacyTable` 条件次第で全DROPされ得る**。移植時は schema_version の確実な引き継ぎと、この分岐の再現（またはより安全な no-destroy 化）が必須。
- **`mcp_servers` FK欠落DROP**（`:1465-1470`）も同様に**登録済みMCPサーバーを消す**破壊操作。`foreign_key_list` の内省結果に依存するため、Rust側 rusqlite で同じ内省が同じ結果を返すか検証が要る。
- 破壊系はすべて `PRAGMA foreign_keys = OFF` 下で実行。Rust移植では **PRAGMA はコネクション単位**である点に注意（`spawn_blocking` で毎回別コネクションを取るプールだと OFF/ON が意図した文に効かない）。移行は**単一専用コネクションで直列実行**すべき。
- 冪等移行（`ensureColumns`/`IF NOT EXISTS`/marker）は Rust でも素直に移植可。ただし marker (`v5_grants_backfilled`) とテーブル存在判定に依存する「一度きり」保証は、移行の途中クラッシュ耐性のためトランザクション境界を厳密に再現する必要がある（v5 は `db.transaction` で包んでいる、`:396-434`）。
- 提言: Rust移植を機に、破壊的分岐（レガシーDROP・mcp_servers DROP）を**明示的なバージョン番号駆動の一方向マイグレーション**へ置き換え、「バージョン不一致=DROP」という暗黙のロスありパスを廃するのが安全。schema_version を整数連番化し、各stepを冪等かつ非破壊にできれば「DROP/再作成」リスクは構造的に消せる。

### 補足リスク
- **データ分離が規約(手書きWHERE)頼み**である点はRust移植でも同じ弱点。移植時に user_id を型で強制する（例: `UserScopedRepo` に `user_id` を保持させ全クエリへ機械的に注入）設計にすると、`AcrossUsers` 系だけを明示的な別APIに隔離でき、クロステナント事故（v7で実際に起きた `bot_mcp_access` の穴）を型で防げる。
- **FK非設定テーブルの孤児**（message_logs/synapses/tool_outcomes等）はユーザー削除後も残る。Rust移植で「ユーザー削除時に user_id で明示DELETE」する掃除処理を足すか、現状の許容方針を踏襲するかを決める必要がある。

---

## [02] I now have a complete picture of the HTTP server core. Compiling the final repor
<sub>agentId: ae6736e2285a034ee</sub>

I now have a complete picture of the HTTP server core. Compiling the final report.

---

# Yuuka HTTPサーバ中核 棚卸し報告（Rust移植向け仕様）

対象: `/home/suki/web/kawaii-music.moe/apps/yuuka/src/`。全ルートは `server/routes/*.ts`（23本）に分離、`server.ts` が起動と静的配信、`server/routeRegistry.ts` がディスパッチ、`server/httpHelpers.ts` が認証補助、`server/chatWebSocket.ts` が `/ws/chat` を担う。

## 1. リクエスト処理パイプライン全体（`server.ts` serverHandler, L245-354）

順序（不変条件として厳守）:
1. **HTTPSリダイレクト** (L257-281): `config.baseUrl` が `https://` で始まり、かつ `checkHttps(req)` が false かつ `Host` のホスト名が baseUrl と一致する場合 `301` で `https://` へ。判定は `x-forwarded-proto: https` または `socket.encrypted`（`httpHelpers.checkHttps`）。
2. **HSTS付与** (L277): https本番なら全レスポンスに `Strict-Transport-Security: max-age=63072000; includeSubDomains` を `setHeader` で先付け（以降の全 writeHead に載る）。preload は付けない。
3. **CORS** (L284-310): `Origin` があり許可判定 true のときのみ `Access-Control-Allow-Origin: <反射>` + `Allow-Credentials: true`。許可条件は — baseUrl未設定なら `http://localhost:` / `http://127.0.0.1:` プレフィクスのみ、baseUrl設定時は `Origin` のホスト名が baseUrl ホスト名と一致する場合のみ。**本番でlocalhostは許可しない**（資格情報反射防止）。
4. **dispatchRoute** (L316-320): レジストリへ委譲。処理済みなら return。例外は 500 JSON。
5. **404 / 静的フォールバック** (L339-353): 未処理の `/api/*` は 404 JSON、それ以外は `serveStaticFile`（SPA）。

## 2. ルーティング機構（`routeRegistry.ts`）

- **登録**: `registerRoutes(defs)` が単一配列 `routes[]` へ push（L84-88）。`server.ts` L39-60 の登録順が走査順＝**線形走査で先勝ち**（L144-223）。
- **マッチ**: `matchPath(pattern, pathname)` (L91-109) は両者を `/` split→空要素除去し、**セグメント数が一致しなければ null**。`:name` セグメントは `decodeURIComponent(pathParts[i])` を `params[name]` に格納。非パラメータは完全一致必須。ワイルドカードや正規表現は無し。マッチは `method` 完全一致が前提（L145）。
- **CSRF** (L37-60, L149-159): `POST`/`DELETE` かつ `route.auth !== "none"` のみ対象。`isCrossSiteStateChange` の判定 —
  1. `Sec-Fetch-Site: cross-site` → **拒否**。
  2. `Origin` があり `!= "null"` → URL パースしホスト名が `isAllowedHost` でなければ拒否（パース失敗も拒否）。
  3. `Referer` があれば同様に判定。
  4. Origin/Referer とも無ければ**判定不能→許可**（Cookie SameSite=Lax に委譲）。
  `isAllowedHost` (L14-29): baseUrl設定時はそのホスト名のみ、未設定時は `localhost`/`127.0.0.1`/`[::1]`/`::1`。拒否時 `403` JSON。
- **認可** (L162-180): `auth !== "none"` なら `resolveUser()` を呼び、null なら `401`。`auth === "admin"` かつ `user.role !== "admin"` なら `403`。`auth === "none"` でもセッションがあれば取得（任意利用、失敗は握り潰し）。
- **ボディ** (L182-204): `POST`/`DELETE` のみ読む。`readBody` (L113-129) は `req.on("data")` 累積で **`total > 10MB`（`MAX_BODY_BYTES = 10*1024*1024`）で reject + `req.destroy()`**、超過は `413` JSON。JSONパース成功かつオブジェクトなら `stripProtoKeys` 後に `body` へ。JSON失敗（Webhook生ペイロード等）は `body={}` のまま `rawBody` のみ提供。
- **プロトタイプ汚染除去** `stripProtoKeys` (L66-81): 深さ6まで再帰。配列は各要素、オブジェクトは `__proto__`/`constructor`/`prototype` を `Object.hasOwn` で確認し `delete`、その後全キーを再帰。
- **ctx**: `{ req, res, url, user, body, rawBody, params }`。ハンドラ例外は `500` JSON（headersSent 未送信時のみ）。

## 3. 全HTTPルート一覧（method / path / auth / body / 動作）

**auth凡例**: none=無認証, user=要ログイン, admin=要admin。body列は主要フィールドのみ（すべてJSON、GET/一部はbodyなし）。

### authRoutes.ts（§5.4）
| M | path | auth | body | 動作 |
|---|---|---|---|---|
|GET|/api/setup/status|none|—|ユーザー0件か(needSetup)+法務URL|
|POST|/api/setup|none|discordId,username,password,geminiApiKey|初回=admin作成+Gemini暗号化保存+自動ログインCookie|
|POST|/api/register|none|discordId,username,password,inviteCode,geminiApiKey|招待コード事前検証→DMワンタイムコード送信(pending)。IP+ID単位レート制限5/15分|
|POST|/api/register/verify|none|discordId,code|DMコード検証→招待コードatomic消費→ユーザー作成|
|POST|/api/login|none|discordId,password|`verifyPasswordConstantTime`（列挙対策）。(IP,アカウント)単位 5回/15分ロックアウト。成功でセッションCookie|
|POST|/api/logout|user|—|`destroySession`+Cookie削除(Max-Age=0)|
|GET|/api/me|user|—|自分の discordId/username/role|
|GET|/api/users|user|—|自身のusernameのみ（互換）|

### settingsRoutes.ts（設定・ステータス・Google OAuth）
| M | path | auth | 動作 |
|---|---|---|---|
|GET|/api/status|user|ユーザー状態集約|
|POST|/api/settings/profile|user|プロフィール更新|
|POST|/api/settings/password|user|パスワード変更|
|POST|/api/settings/delete-account|user|アカウント削除|
|POST|/api/settings/user|user|ユーザー設定更新|
|POST|/api/settings/gemini|user|Gemini APIキー更新（暗号化）|
|GET/POST|/api/settings/discord|user|Discord連携設定 取得/更新|
|GET|/api/settings/google/oauth/url|user|OAuth開始URL（state=nonce, prompt=consent）|
|**GET**|**/api/settings/google/oauth/callback**|**none**|**リダイレクト応答のため none。ハンドラ内で `ctx.user` を手動確認→state を `consumeOAuthState` で消費し `stateUserId===userId` を検証（CSRF）。code交換→refresh_token必須→`addGoogleAccount`。全エラーは固定コードのみURLへ（上流メッセージ非漏洩）**|
|POST|/api/settings/calendars|user|利用カレンダー更新|
|POST|/api/settings/backup|user|バックアップ設定|
|POST|/api/settings/backup/trigger|user|バックアップ即時実行|

### botRoutes.ts（§5.1,5.2）
GET/POST/**DELETE** /api/bots（一覧/作成/削除）、POST /api/bots/sync-discord、/api/bots/profile、GET /api/bots/shares、POST /api/bots/shares/invite、/shares/revoke — 全 user。共有はowner本人スコープ。

### botAttributeRoutes.ts
GET /api/bots/presets, /usage, POST /api/bots/attributes, GET/POST /api/bots/modules, GET /api/bots/assistant-config, POST /api/bots/assistant/{gemini-key,persona,guilds,members,roles}, GET /api/bots/assistant/{guild-options,guild-note}, POST /api/bots/assistant/guild-note — 全 user。**GET/POST /api/admin/bot-attribute-settings — admin**。

### memberRequestRoutes.ts
POST /api/bots/member-requests（申請）, GET /mine, GET /api/bots/member-requests（承認者向け一覧）, POST /api/bots/member-requests/:id/decide — 全 user（ギルドowner/管理者スコープはハンドラ内判定）。

### todoRoutes.ts（§3.2）
GET /api/tasks, /gantt, /someday, /detail, POST /api/tasks/{add,update,progress,complete,delete} — 全 user。

### scheduleRoutes.ts / timelineRoutes.ts（§3.X）
schedule: GET /api/schedules, POST /add,/delete。
timeline: GET /api/timeline/day, POST /plan,/plan/update,/plan/delete,/record,/record/delete,/media（base64画像/動画保存）, **GET /api/timeline/media/:filename（認証付き配信、`Cache-Control: private`）**。全 user。`scope(ctx)` で userId+botId 分離、`hasBotAccess` 検証。

### financeRoutes.ts（§3.4）
GET /api/expenses, POST /add, /upload-receipt（base64画像・10MB上限の主因）, GET/POST /budget-limits, POST /budget-limits/delete, GET /plans, POST /plans/add,/plans/pay,/plans/delete — 全 user。

### playbookRoutes.ts（§3.6）
GET /api/playbooks, POST /save,/delete, GET /schedules, POST /schedules/save,/toggle,/delete, GET /runs — 全 user。

### credentialRoutes.ts（§6 パスワードマネージャ）
GET /api/credentials, POST /register, /delete — 全 user。値は暗号化保存。

### reminderRoutes.ts（§3.3）
GET /api/reminders, POST /add, /cancel — 全 user。ctx.user.discordId スコープ必須。

### personalRoutes.ts（§3.7,3.10,3.11）
GET/POST /api/context-note, GET /api/clipboard, POST /clipboard/delete, GET /api/contacts, POST /contacts/save,/contacts/delete — 全 user。discordId スコープ。

### personaRoutes.ts（§4.1）
GET /api/personas, POST /save,/delete,/activate,/publish, GET /marketplace, GET /marketplace/:id, POST /import, POST /api/bots/recommended-persona — user。**POST /api/admin/personas/unpublish, /delete — admin**。

### mcpRoutes.ts（§4.4）★特殊
| M | path | auth | 動作 |
|---|---|---|---|
|GET|/api/mcp-servers|user|一覧|
|POST|/api/mcp-servers/add,/refresh,/toggle,/delete|user|管理|
|GET|/api/mcp-servers/:id/dashboard/status|user|ダッシュボード有無|
|GET|/api/mcp-servers/:id/dashboard|user|**iframe用HTML。専用CSP `default-src 'none'; script-src 'unsafe-inline'; connect-src <selfOrigin絶対>` 等。`<base>`除去、`MCP_PATH`をproxy絶対URLへ書換、`tokenFromHash()`を書換、proxyToken(256bit,serverId束縛,TTL,失効可)注入、akizakura.cssインライン化。書換失敗時は200を返さず502**|
|**OPTIONS**|**/proxy/mcp/:id/mcp**|**none**|**CORSプリフライト。`ACAO: null` + `Allow-Credentials`削除 + 要求ヘッダ反射**|
|**POST**|**/proxy/mcp/:id/mcp**|**none**|**Bearer=proxyToken検証→発行ユーザーをDB再取得し`canManage`+`enabled`毎回再検証→`assertSafeOutboundUrl`(SSRF/DNSリバインド対策)→上流MCPへ中継。`ACAO: null`固定+Credentials削除。res 'close'でAbort**|

不透明オリジン(sandbox iframe)からの呼び出しのため Cookie は届かず、認証は **proxyToken 単独**が主体。

### integratedRoutes.ts（v5, owner横断）
GET /api/integrated/overview, POST /bots/{start,stop,restart,clear-history}, /grants/{mcp,credential,google}, /google/accounts/{primary,delete,calendars}, GET /google/accounts/:id/calendars — 全 user・owner本人スコープ。

### webhookRoutes.ts（§3.13）★特殊
| M | path | auth | 動作 |
|---|---|---|---|
|**POST**|**/hook/:token**|**none**|**トークン単位30/分レート制限→endpoint検証(無効なら410)→即`200 accepted`応答後に`processIncomingWebhook(endpoint, rawBody, signatureHeader)`を非同期実行。署名は`x-hub-signature-256`等**|
|GET|/api/webhooks|user|一覧|
|POST|/api/webhooks/create,/update,/delete|user|管理|
|GET|/api/webhooks/deliveries|user|配信履歴|

### deliveryRoutes.ts（§3.8,3.9）
GET/POST /api/briefing-config, POST /briefing/test, GET/POST /api/report-configs, POST /report-configs/test — 全 user。

### adminRoutes.ts（§5.3・全 admin）
POST /api/admin/default-bot/token, GET /api/admin/stats, GET/POST /api/admin/system-settings, GET /api/admin/users, POST /users/role, /users/delete, GET /api/admin/audit-logs, GET /api/admin/bots, POST /bots/suspend,/bots/unsuspend, GET/POST /api/admin/invite-codes, POST /api/admin/invite-codes/:code/revoke, **DELETE /api/admin/invite-codes/:code**。

### deviceAuthRoutes.ts（RFC8628型デバイスフロー）
| M | path | auth | 動作 |
|---|---|---|---|
|POST|/api/auth/device/code|none|device_code+user_code発行（device_code自体がcapability）|
|POST|/api/auth/device/approve|user|user_codeを本人が承認（persist失敗は503, expired=410, notfound=404）|
|POST|/api/auth/device/token|none|device_codeをポーリング交換。`authorization_pending`/`slow_down`(200), `expired_token`(410), `approved`→`{access_token, token_type:Bearer, expires_in, user}`|

### deviceMgmtRoutes.ts
GET /api/devices, POST /api/devices/revoke — user（Cookie/Bearer両経路本人）。

### desktopClientRoutes.ts
GET /api/desktop/info（バイナリメタ）, GET /api/desktop/download（`yuuka-desktop.exe`をattachment配信、`application/vnd.microsoft.portable-executable`）— user。

## 4. 認証／セッション

- **二経路**（`httpHelpers.resolveRequestUser`, L153-159）: **Cookieセッション → Bearerトークンの順**で解決。先勝ち。
- **Cookie** (L50-108): https本番(`isHttpsDeployment`= baseUrl が https)では `__Host-yuuka-session` **のみ**受理、非Secure名は無視。開発では `__Host-yuuka-session` または `yuuka-session`。発行属性: `Path=/; HttpOnly; [Secure]; SameSite=Lax; Max-Age=<sessionTtlDays日>`。名前はデプロイ設定で決定（リクエスト毎推定でなく）。
- **セッション保存** (`services/sessionService.ts`): **Redis** 主。値は `sha256(token)` でキー化（生トークン非保存）。TTL=`config.sessionTtlDays`日、**アクセス毎に`expire`延長＝スライディングウィンドウ**。`user_sessions:<discordId>` set で逆引き。**Redis不通時は in-memory Map フォールバック**（再起動で消えるのは許容）。
- **Bearer/desktop** (L127-146, `services/desktopAuthService.verifyToken`): `Authorization: Bearer <token>` → `sha256Hex` → **SQLite `desktop_tokens` テーブル**を `getActiveDesktopTokenByHash`（revoked=0 かつ TTL内）で照合→ユーザー引き直し→`touchDesktopToken`（last_used更新）。TTL=`config.desktopTokenTtlDays`。device_code一時状態はRedis(揮発)+memoryフォールバック。
- **CSRF非該当**: Bearerはアンビエント資格情報でないため Origin チェックの対象外（Cookie経路のみ）。
- **SessionUser** = `{ discordId, username, role: "user"|"admin" }`。role はセッション/DBから毎回導出（admin判定はレジストリで実施）。

## 5. 静的配信（`server.ts serveStaticFile`, L115-236）

- **配信元** `PUBLIC_DIR`: `STATIC_DIR` env か既定 `cwd/dist/public`。
- **パストラバーサル防御** (L122-131): `path.normalize(path.join(PUBLIC_DIR, urlPath))` が `PUBLIC_DIR + sep` で始まる（または PUBLIC_DIR 自身）でなければ **403**。
- **SPAフォールバック** (L133-148): stat失敗かつ**拡張子なし**なら `index.html` を配信、拡張子ありなら `404.html`（無ければ text）。
- **CSP** (L82-83, const): `default-src 'self'; script-src 'self' https://static.cloudflareinsights.com; style-src 'self' 'unsafe-inline' fonts...; img-src 'self' data: assets-global cdn.discordapp.com; connect-src 'self' cloudflareinsights.com; worker-src 'self'; frame-src 'self'; frame-ancestors 'self';`。**script-srcに'unsafe-inline'なし**（インラインJS全排除済、多層XSS防御）。style-srcの'unsafe-inline'はテンプレートstyle=のため維持。
- **セキュリティヘッダ** (L84-89): `X-Content-Type-Options: nosniff`, `X-Frame-Options: SAMEORIGIN`, `Referrer-Policy: strict-origin-when-cross-origin`, `Content-Security-Policy`。全静的レスポンスに付与。
- **gzipオンザフライ** (L107-110, L152, L188-234): `Accept-Encoding: gzip` かつ拡張子が `.html/.css/.js/.json/.svg` のとき圧縮。index.htmlはメモリ圧縮、他は `zlib.createGzip()` でpipe。圧縮失敗は素データにフォールバック。`Vary: Accept-Encoding` 付与。
- **キャッシュ** (L154-162): `assets/` 配下かつ内容ハッシュ命名（`-[A-Za-z0-9_-]{8,}\.(js|css|woff2?|png|jpe?g|svg|webp)$`）は `public, max-age=31536000, immutable`。それ以外（index.html/theme-init.js/manifest.json/sw.js/404.html）は `no-cache, no-store, must-revalidate`。
- **GSA差し込み** (L173-207): `index.html` のみ、`config.googleSiteVerification` があれば `<!-- GOOGLE_SITE_VERIFICATION -->` を `<meta name="google-site-verification" content="...">` に置換、無ければ空文字に置換。
- **ストリーム安全** (L211-234): read/gzip stream の `error` で `res.destroy()`（未処理エラーでのプロセスクラッシュ回避）、`res.on("close")` で stream.destroy（切断時中断）。
- **timeline media** (`db/timelineRepo.resolveMediaPath`): `filename` に `/`・`\`・`..` を含めば null、`path.join(MEDIA_DIR, filename)` が `MEDIA_DIR + sep` 始まりでなければ null。ただしこの配信は auth:"user" ルート経由。

## 6. WebSocket `/ws/chat`（`server.ts` L385-407, `chatWebSocket.ts`）

- **upgrade** (L385-407): `pathname !== "/ws/chat"` は `socket.destroy()`。**`getBearerUser(req)`（Bearerのみ、Cookie不使用）**が null なら `401`。`?botId=`（既定 `system_default`）に対し `hasBotAccess(user.discordId, botId)` が false なら `403`。**ネイティブBearerはCSRF非該当のためOriginチェックなし**。`chatWss`（`noServer: true`）で `handleUpgrade`→`handleChatConnection`。
- **ready** (L85-91): 接続直後に `{ type:"ready", user, bot(束縛), bots(所有/共有一覧), maxUploadMb }`。
- **ping/pong keepalive** (L94-109): `PING_INTERVAL_MS=30_000`。`isAlive` フラグ、pongでtrue、無応答なら `ws.terminate()`。
- **ターンキュー/バックプレッシャ** (L112-173): `chain: Promise` で **1接続1ターン直列**（`chain.then(...)`）。`queued` カウンタ、`MAX_QUEUED_TURNS=4` 超過の `msg` は `{type:"error", code:"rate_limited"}`。
- **添付上限** (L73, L117): `config.desktopMaxUploadMb*1024*1024` 超のメッセージは `{code:"too_large"}`。
- **メッセージ種別** (L140-233): `ping`(無応答), `reset`(`handleReset`), `msg`(`handleChatMessage`, image/audioは`{mime,data}`検証), `interaction`(ボタン; `dispatchComponentInteraction`, WS responder が update/reply/followUp を frame化)。不正JSONは `{code:"internal"}`。
- **停止** (`stopWebServer` L421-436): 全 `chatWss.clients` を `terminate()` 後に `server.close()`。

## 7. Rust移植で忠実に再現すべき不変条件

**セキュリティ:**
1. CSRF: POST/DELETE かつ auth≠none で `Sec-Fetch-Site: cross-site` / Origin / Referer を **上記の優先順・判定不能時は許可** で判定し 403。パース失敗＝拒否。auth:"none"（webhook/proxy/device）は対象外。
2. ボディ10MB厳格上限→413。超過時は接続破棄。
3. `stripProtoKeys`（`__proto__`/`constructor`/`prototype`、深さ6、配列含む）をJSONパース直後に必ず適用。
4. admin判定はレジストリで `user.role==="admin"` を毎回実施（セッション/DB由来のroleを信頼）。
5. Cookie: https本番は `__Host-` 名 + Secure + HttpOnly + SameSite=Lax のみ受理・発行。名前はデプロイ設定で決定。トークンはハッシュ保存（生値非保存）。
6. CORS: 本番でlocalhost反射禁止。許可Originのみ ACAO反射 + Allow-Credentials。
7. MCP proxy: proxyToken 単独認証（serverId束縛/TTL/失効/256bit）、**毎回 canManage+enabled を再検証**、中継直前に `assertSafeOutboundUrl`（SSRF/DNSリバインド）、`ACAO: null` + Credentials削除（ACAO:null と Credentials:true の同居を禁止）。書換失敗時は200を返さず502。
8. iframe隔離: dashboard は sandbox（allow-scripts のみ＝不透明オリジン）、専用CSP `default-src 'none'`、`<base>`除去。
9. Login列挙対策: 定時間比較(`verifyPasswordConstantTime`)、(IP,アカウント)単位ロックアウト。register: DMチャレンジ + IP+ID単位レート制限、招待コードは本人確認後にatomic消費。
10. OAuth callback: auth:"none"だがハンドラ内で `ctx.user` 確認 + state nonce を消費し `stateUserId===userId` 検証。上流エラーメッセージ非漏洩。
11. 静的配信: normalize後の PUBLIC_DIR 前方一致でトラバーサル遮断。CSP/nosniff/X-Frame/Referrer-Policy 全付与。media は `/`,`\`,`..` 拒否 + ディレクトリ境界チェック。
12. HTTPSリダイレクト + HSTS(2年,includeSubDomains) を https本番で全応答に付与。webhook `/hook/:token` は署名(`x-hub-signature-256`)を `rawBody` で検証。

**データ分離:**
13. 全 user リソースは `ctx.user.discordId`（+ botId）でスコープ。timeline/reminder/personal/finance 等は所有者外アクセス不可。bot操作は `hasBotAccess`/`canManage`/owner本人で毎回検証。
14. WS は 1接続=1Bot束縛（upgrade時に hasBotAccess）、1接続1ターン直列、queue上限4、添付 `desktopMaxUploadMb` 上限。
15. セッション保存はRedis（sha256キー、スライディングTTL）+ memoryフォールバック。desktopトークンはSQLite `desktop_tokens`（token_hash, revoked, TTL）。device_codeはRedis+memory。

**参照ファイル**（全て絶対パス）: `.../src/server.ts`, `.../src/server/routeRegistry.ts`, `.../src/server/httpHelpers.ts`, `.../src/server/chatWebSocket.ts`, `.../src/server/routes/*.ts`(23本), `.../src/types/contracts.ts`, `.../src/services/sessionService.ts`, `.../src/services/desktopAuthService.ts`, `.../src/db/desktopTokenRepo.ts`, `.../src/db/timelineRepo.ts`。

---

## [03] I have all the files. Here is the inventory.
<sub>agentId: a3f8231141dccf19c</sub>

I have all the files. Here is the inventory.

---

# Discord ボット層と起動ライフサイクル 棚卸し（apps/yuuka）

## 1. 起動/終了シーケンス — `src/index.ts`

### 起動（`main()`）
順序は厳密に直列で、依存関係順に並んでいる:

1. `import "./utils/timezone.js"` を**最初の副作用**として実行（他モジュールが Date/DB を読む前に JST を既定化）。
2. `applyGoogleHttpFix()` — 全 Google 呼び出しより前に一度だけ。
3. **暗号化シークレット必須/強度チェック**: `config.secretKey`/`config.secretKeyNew` のどちらも無ければ `process.exit(1)`。両者とも 32 文字未満なら `exit(1)`（`YUUKA_ENCRYPTION_SECRET_NEW` のみ設定は旧鍵からのローテーション起動として許容）。
4. `runMigrations()` — DB スキーマ適用（await）。
5. `config.secretKeyNew` があれば `rotateSecretKey(getDb())` — 鍵ローテーション（§6.2.1）。
6. 招待コード初期投入 `seedInitialCodes()`。
7. `await initRedis()` — 会話コンテキスト/セッション。
8. `await startWebServer()` — 管理画面。
9. `await startBot()` — Discord（後述）。
10. **cron 系を同期起動**: reminder → playbook → clipboard cleanup → birthday → payment recurrence → todo recurrence → report → briefing → backup。
11. `await startSynapseEngine()`（Rust シナプスのウォームスタート、バイナリ不在時はデグレード）→ `startMetricsLogging()`。

つまり **migration → (rotate) → redis → web → bot → cron群 → synapse/metrics** の順。Bot は Web の後、cron の前。

### Graceful shutdown（`gracefulShutdown()`）
- `shuttingDown` フラグで**多重呼び出しを冪等化**（SIGINT/SIGTERM 二重受信・uncaughtException 経由の再入を無視）。
- **15秒 watchdog**: `setTimeout(...).unref()`。停止処理がハングしても systemd の SIGKILL を待たず `exit(1)`。unref しているのでこれ自体はイベントループを延命しない。
- 停止順序は起動の**逆順**（metrics → synapse → backup → … → reminder → web → bot → `closeDb()` → `await closeRedis()`）。`try/catch` で一括ラップし、途中で例外が出ても握り潰してログ。

### シグナル/例外の扱い
| ハンドラ | 挙動 |
|---|---|
| `SIGINT` / `SIGTERM` | `gracefulShutdown()` を await → `process.exit(0)` |
| `unhandledRejection` | **ログのみ・稼働継続**（discord.js/cron 深部の 1 件の拒否で全体を落とさない設計思想） |
| `uncaughtException` | **プロセス状態破損の可能性**として `gracefulShutdown()` → `finally { exit(1) }`（systemd Restart=always での再起動を期待） |
| `main().catch()` | 起動失敗時は web/db/redis を最小限クローズして `exit(1)` |

**論点（後述の Rust 設計に効く）**: unhandledRejection=継続 / uncaughtException=再起動、という「回復可能性による二分」が明確。これは Rust では panic（≈uncaught）と `Result` 伝播（≈handled rejection）の使い分けに対応する。

---

## 2. `bot.ts`: Client 初期化とマルチインスタンス管理

### Client オプション
`DISCORD_CLIENT_OPTIONS` を全 Client（デフォルト・カスタム）で共有:
- **intents**: `Guilds`, `GuildMessages`, `MessageContent`, `DirectMessages`。**`GuildMembers` 特権インテントは使わない** — このためメンバー一覧はキャッシュ済みのみ（`getGuildOptionsForBot` が `members_complete: false` を返し UI が ID 手入力で補う）。ロールは `Guilds` で完全取得可能。
- **partials**: `Channel`, `Message`（DM チャンネルや未キャッシュメッセージ＝返信先 fetch のため）。

### 共有クライアント + customClients
- `export let client: Client`（**`let`＝差し替え可能**）がデフォルト共有 Bot（`system_default`）。
- `export const customClients = new Map<botId, Client>()` がユーザー別トークンの独自 Bot 群。
- ESM の named import が**ライブバインディング**であることを明示的に利用: `restartDefaultBot` が `client` へ再代入すると notifier 等の参照側にも即反映される（コメントで意図が明記）。

### getBotClientForUser / トークンローテーション
- `getBotClientForUser(botId)`: `customClients.get(botId)` が存在し `readyAt` が立っていればそれを、無ければデフォルト `client` を返す（オフライン独自 Bot はデフォルトへフォールバック）。
- **`restartDefaultBot(token)`**: discord.js v14 は **destroy 済み Client の再ログインを保証しない**ため、同一インスタンス再利用をせず必ず新しい `Client` を作る。手順: 旧 `client.destroy()` → `new Client()` → `attachDefaultClientHandlers()` でリスナー再登録 → `login(token)` 成功後に `client = newClient` を差し替え。失敗時は新 Client を destroy して false。
- **`startCustomBot(botId)`**: `startInFlight: Map<botId, Promise>` で**同時起動を直列化**。`startCustomBotInner` は最初の await まで同期実行されるため、in-flight 登録が制御をイベントループに戻す前に完了する（後続の同時呼び出しが必ず合流）。これは「destroy されない Client が Gateway に残り二重応答する」バグへの対策。inner は suspended チェック → トークン復号 → 既存 Client を destroy/delete → 新 Client 生成 → リスナー登録 → login → Map 登録。
- `stopCustomBot` / `stopBot`: `destroy()` して Map から削除/clear。`stopBot` は profileSyncTimer も clearInterval。
- **プロフィール同期**: `startProfileSyncTimer()` が 1 時間ごとに全 Client の username/avatar を DB へ同期。タイマーは `.unref()`（graceful shutdown を妨げない多重防御）。

### リスナー登録の一元化
`attachDefaultClientHandlers(botClient)` が `Events.Error`（**'error' リスナー必須＝無いと非同期リスナー内例外が Node クラッシュ**）/ `ClientReady` / `InteractionCreate` / `setupMessageListener` を登録。初期化時とリスタート時で同一関数を使い登録漏れ・二重登録を防ぐ。カスタム Client も同様に `Events.Error` を必ず張る。

### startBot()
`system_default` トークンを復号して login（無ければ「初期セットアップ未完了」ログのみで継続）→ `listAllBots()` を回し、`system_default` 以外かつ `stopped !== 1` の Bot を `startCustomBot` で順次起動（`stopped=1` は再起動後も停止維持）。

---

## 3. インタラクション処理

### ボタンのみ
`handleInteraction` は先頭で `if (!interaction.isButton()) return;` — **ボタンインタラクションのみ**を処理。スラッシュコマンド/セレクトメニュー等は扱わない。

### customId パース規約
`customId.split(":")` → `[action, idStr, extra]`。規約は **`action:id[:extra]`**。分岐する action:
- `memreq_apply:<botId>:<guildId>` — メンバー外ユーザーの利用申請（guildId が extra）。
- `memreq_approve` / `memreq_reject`:`<requestId>` — オーナーが DM ボタンで承認/却下（`interaction.update` で元メッセージを書き換え）。
- `share_accept` / `share_decline`:`<shareId>` — Bot 共有招待の承認/辞退（宛先・pending 検証あり）。承認後、推奨ペルソナ（public）があれば `followUp` で `persona_import:<personaId>` ボタンを提示。
- `persona_import:<personaId>` — 登録ユーザーのみ、`importPersona` でコピー生成。

エラー時は `console.error` の上、未応答なら ephemeral で「処理中にエラー」を返す（返信自体の失敗も握り潰し）。

### チャネル中立ディスパッチャ — `componentInteractionService.ts`
`bot.ts` の `handleInteraction` と**ほぼ同一の分岐ロジックを二重実装**しているのが `dispatchComponentInteraction`。こちらは Discord/WS（デスクトップ）両対応のため `InteractionResponder` 抽象（`update`/`reply`/`followUp`）を挟む。追加で `demo_echo`（デスクトップ実証デモ）を持ち、`resolveApplicantContext` コールバックで申請者ラベルを注入。**同じ customId 規約・同じ 5 アクション**を扱う。
→ **論点**: bot.ts 側とロジックが重複しており、片方だけ更新されるとドリフトするリスク。将来的には bot.ts の `handleInteraction` も `dispatchComponentInteraction` に寄せて Discord responder を渡す形へ統合すべき。

### gemini への受け渡し（メッセージ経路）
`setupMessageListener(botClient, botId?)` が `messageCreate` を受ける。共通ガード:
1. `message.author.bot` を無視。
2. `!botClient.isReady() || !botClient.token` なら処理しない（destroy 直後/再起動中の "Expected token to be set" 回避）。
3. **冪等ガード** `claimMessageOnce(botUserId, messageId)`（TTL 60s、`bot user id : message id` キー、2000 件超で遅延掃除）— 同一 identity の Client が複数存在しても 1 メッセージ 1 応答。別 Bot 同時メンションは user id が違うので両方応答。

その後 **2 系統に分岐**:

**(A) 汎用モード（`isGuildAssistantBot(botId)` が true）** → `handleAssistantMessage`:
- DM は owner のみ応答。ギルドは `isGuildAllowed` 許可リスト（未許可は応答も記録もしない §6）。
- メンション or Bot への返信のみ（DM は常に対象）。
- 利用メンバー判定: `owner || isBotMember || isAnyRoleAllowed`。非メンバーには `sendNonMemberGuidance`（5 分スロットル、ギルド内なら `memreq_apply` ボタン付き）。
- **Bot 専用キー必須** `getBotGenAI(botId)` — 無ければ黙殺。
- **レート制限** `consumeRateLimit` 超過なら `rateLimitMessage` を返し LLM を呼ばない。
- 添付（画像/音声）を base64 化して `ChatMessage` に載せ、`processGuildMessage(botId, guildId, speaker, msg, statusCallback, asyncDelivery)` または DM は `processBotDmMessage(botId, speaker, ...)` を呼ぶ。

**(B) 秘書モード（デフォルト/独自秘書 Bot）**:
- カスタム Bot はオーナー or 共有ユーザー（`listBotsForUser`）でなければ無視。
- `isRegisteredUser` 必須（§5.4）。
- メンション/DM/Bot への返信のいずれかでのみ応答。
- 画像は `parseReceipt`（レシート OCR 秘書機能）、音声は文字起こし指示付きで `processMessage`、テキストは `processMessage(resolvedBotId, userId, chatMessage, statusCallback, asyncDelivery)`。

3 つの gemini エントリの使い分け:
- `processMessage(botId, userId, …)` — 秘書モード（DM/ギルド問わず登録ユーザー個人ワークスペース）。
- `processGuildMessage(botId, guildId, speaker, …)` — 汎用モードのギルド発話。
- `processBotDmMessage(botId, speaker, …)` — 汎用モードの owner DM。

**共通の非同期配送** `TurnAsyncDelivery`: `onInterim`（重い処理判明時に「入力中」を止めて一次返信）と `deliverFinal`（事前予測で非同期化したターンの最終結果を `sendToUser` で同チャンネル→失敗時 DM フォールバック）。`statusCallback` が `setBotStatus`（thinking/writing/idle の Presence）を駆動。返信は `toDiscordMarkdown` 変換後、2000 字超は `splitMessage` で分割し `safeReply`（例外を投げない返信ヘルパー）で送る。typing 維持タイマーは全経路の `finally` で必ず clear。

---

## 4. レート制限・通知・capabilities/modules

### `botRateLimit.ts`
- 3 軸: **user/分**（既定5）・**user/日**（既定100）・**guild/日**（既定1000）。閾値は `system_settings` で上書き可能。
- Redis TTL 付きカウンタ（`incr` → count===1 のとき `expire`）。**Redis 不通時はプロセス内 `memoryCounters` へフォールバック**（10 分ごと失効掃除、`.unref()`）。
- 日次キーは `todaySuffix()`（YYYYMMDD）で衝突回避、TTL 25h で日跨ぎ吸収。
- `consumeRateLimit` は 3 軸を順にインクリメントし最初の超過で `{allowed:false, exceeded}` を返す。→ **論点**: user/分を先にインクリメントするので、超過時も user/日・guild/日 はインクリメントされない（超過分をカウントに含めない設計）。

### `notifier.ts`
- `resolveClientForUser(userId, botId?)`: botId 指定時はそのBot本人 Client（オフラインならデフォルトへフォールバック）。無指定時は「起動中の秘書型独自 Bot → デフォルト」の順（汎用モード Bot は通知に使わない §4.2）。
- `sendToUser`: target 未指定なら `getUserNotifyTarget`（DM フォールバック）。**セキュリティ検証**: channel 送信時、ギルドチャンネルなら対象ユーザーが `ViewChannel` 権限を持つメンバーか検証（共有 Bot を踏み台にした第三者チャンネルへのデータ露出防止）。失敗時 DM フォールバック。空ペイロードは送らない。
- リマインド/日報/朝報/Webhook 通知の共通基盤で、`bot.ts` の `deliverFinal` からも呼ばれる。

### `botCapabilities.ts`
- capability = `persona | memory | mcp | secretary`（`core` は全 Bot 暗黙）。プリセットは 2 種: `secretary`（4 capability 全部）/ `mcp_assistant`（secretary 抜き＝汎用モード）。
- `resolveBotCapabilities` はインメモリキャッシュ（ホットパス）。不正 JSON/未登録 Bot は秘書相当へフォールバック（`system_default` の既存動作を保つ）。
- **`isGuildAssistantBot` = secretary を持たない** ＝ 汎用モード判定の要。`bot.ts` のメッセージ分岐と `notifier.ts` の通知除外がこれに依存。
- `applyBotPreset` で DB 更新 + `invalidateBotCapabilitiesCache`。表示名は `system_settings` で変更可能。

### `botModules.ts`
- capability の**後段フィルタ**。`enabled_modules`（selectable モジュール ID の JSON 配列）を解決。セマンティクス: `NULL`→全有効（後方互換）/ `[...]`→該当集合 / `[]`→core のみ。
- 2 段キャッシュ: Bot 単位 `enabledModulesCache` と ユーザー×Bot `userModulesCache`（キー `${botId} ${userId}`）。
- `resolveEnabledModulesForUser`: ①ユーザー個別上書き（`bot_user_modules`、`[]` も尊重）→②Bot 既定→③null（全有効）の優先順。無効化は botId/userId 粒度。
- **注記**: `bot.ts` からは直接呼ばれず gemini/functions 層で使われるが、capability→module の 2 段解決という設計を理解しておく必要がある。

---

## 5. Rust（serenity/twilight）での再設計 論点

### 5.1 複数ボット同時接続
- **twilight** が有利。`twilight-gateway` は 1 トークン = 1 `Shard`/`ShardGroup`。複数トークンは `Map<BotId, ...>` ではなく **`tokio::task` per トークン**でシャードを回し、全イベントを 1 本の `mpsc` に集約するファンインが自然。共有 HTTP は `twilight_http::Client` をトークンごとに 1 個（`Arc<Client>`）。
- **serenity** は `Client` が 1 トークン前提でランタイムを丸ごと抱えるため、N 個の `serenity::Client` を別タスクで走らせる形になり、現状の `customClients: Map` にほぼ 1:1 対応する。ただし serenity は「差し替え」に弱い（下記）。
- **intent の型安全化**: 現状の「`GuildMembers` を使わないためメンバーが不完全」という制約は、twilight/serenity では `Intents` ビットフラグで表現でき、キャッシュの `members_complete: false` セマンティクスをキャッシュ層の型（`Option<CompleteMemberList>`）に落とし込める。

### 5.2 トークン差し替え（restartDefaultBot 相当）
- JS 側は「destroy 済み Client は再利用不可 → 新インスタンス + ライブバインディング再代入」で解決している。Rust には ESM ライブバインディングが無いので、**`Arc<ArcSwap<BotClient>>` / `Arc<RwLock<BotClient>>`** で参照を差し替える設計にする。notifier 相当は `arc_swap.load()` で常に最新を掴む。
- twilight ではシャードは `tokio::task` なので、差し替え = **旧タスクへ shutdown シグナル（`CancellationToken`）→ 新トークンで新タスク spawn → `ArcSwap` を store**。serenity は `ShardManager::shutdown_all()` 後に `Client` を作り直す（JS の destroy→new とほぼ同型）。
- **直列化ガード**（`startInFlight`）は Rust では `tokio::sync::Mutex<HashMap<BotId, ...>>` または per-bot の `Mutex` で表現。より Rust らしくは各 Bot を**アクター**（1 タスク + コマンド `mpsc`）にして「起動/停止/差し替え」をコマンドとして直列処理すれば、二重 Client・レースが型レベルで消える（`startInFlight` も `claimMessageOnce` の一部も不要化）。

### 5.3 自己復帰（uncaughtException→再起動 / Events.Error→握り潰し）
- twilight/serenity の Gateway は**再接続を内蔵**（resume/identify）。JS で必須だった `Events.Error` リスナー（無いとクラッシュ）に相当する落とし穴は無く、`Shard::next_event()` が `Err` を返すだけ。個別イベント処理の panic は**タスク境界で隔離**され、`tokio::task::JoinHandle` の `Err(JoinError { is_panic })` で検知して当該 Bot タスクのみ再 spawn できる（プロセス全体を落とさない ＝ JS の unhandledRejection 継続方針を、より安全に実現）。
- ただし **panic=abort** ビルドだと 1 タスクの panic がプロセスを落とすので、Bot 層は `panic = "unwind"` + `catch_unwind`/タスク監視（supervisor パターン）を推奨。JS の「uncaughtException は状態破損とみなし再起動」に相当する境界を、**プロセス全体ではなくアクター単位**に狭められるのが Rust の利点。
- **graceful shutdown**: `tokio::signal` で SIGINT/SIGTERM を受け、`CancellationToken` を全 Bot タスク + cron タスクへ伝播、`tokio::time::timeout(15s, join_all(...))` で watchdog を表現（JS の unref 付き setTimeout と等価だが、`select!` でタイムアウトを構造化できる）。

### 5.4 その他移植上の論点
- **冪等配送** `claimMessageOnce`: アクター化で単一 Client になれば不要だが、複数プロセス/水平スケール時は Redis SETNX（TTL 付き）へ寄せるべき。現状のプロセス内 Map は単一プロセス前提。
- **レート制限のフォールバック**: Rust でも Redis + インメモリ二段は同型で書けるが、`incr`→`expire` の**非アトミック性**（count===1 判定と expire の間で落ちると TTL 無し永続キーが残る）は Lua スクリプト / `SET NX EX` パターンで原子化すべき。これは JS 版から引き継ぐ既存の弱点。
- **ライブバインディング依存の排除**: JS は `export let client` の再代入に強く依存しており、Rust ではこれが `ArcSwap` に置き換わる。notifier / getBotClientForUser の「readyAt チェック」は Rust では `Shard` の接続状態 or `Ready` 受信フラグを `AtomicBool` で保持する形になる。
- **インタラクションの重複ロジック**（handleInteraction と dispatchComponentInteraction）: Rust 移植時は最初から `InteractionResponder` トレイトに一本化し、Discord gateway interaction と WS 両方の実装を提供する（トレイトオブジェクト or ジェネリクス）。JS で既に抽象境界が切られているので、移植はこの中立層を正とすべき。

---

## 主要ファイルパス
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/index.ts` — 起動/終了ライフサイクル
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/bot.ts` — Client 管理・メッセージ/インタラクション処理
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/services/componentInteractionService.ts` — チャネル中立ディスパッチャ
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/services/botRateLimit.ts` — 3 軸レート制限
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/services/notifier.ts` — 通知基盤
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/services/botCapabilities.ts` — capability/preset 解決
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/services/botModules.ts` — enabled_modules 解決

---

## [04] I have a complete and thorough picture of the entire subsystem. Let me compile t
<sub>agentId: a14c17f060ab41d83</sub>

I have a complete and thorough picture of the entire subsystem. Let me compile the inventory.

# AI（Gemini）オーケストレーション / Function Calling / シナプス記憶 — 棚卸し報告

対象: `/home/suki/web/kawaii-music.moe/apps/yuuka`（Node/TypeScript の Discord アシスタント。ブランチ `feature/svelte-migration`）。SDK は `@google/generative-ai`（旧世代 GA SDK）。モデル既定は `gemini-3.1-flash-lite`。

---

## 1. gemini.ts のメッセージ処理フロー

エントリは3経路。すべて末尾で共通クロージャ `runPlannedTurn` に合流する。

- `processMessage(botId, userId, …)` — **秘書モード**（DM/個人）。ユーザー個別キー（`getUserGenAI`）で実行。
- `processGuildMessage(botId, guildId, speaker, …)` — **汎用モード（ギルド常駐）**。Bot 専用キー（`getBotGenAI`）で実行。
- `processBotDmMessage(botId, owner, …)` — **owner との動作確認 DM**。同じく Bot 専用キー。

### コンテキスト構築（秘書経路 `processMessage`、`src/gemini.ts:934`）
1. ユーザー発話を永続ログ＋直近キャッシュへ保存（`addMessageLog`）。空文字は `[音声メッセージ]`/`[画像]` で代替ログ。
2. 直近履歴を取得（`getRecentContext(userId, botId, 15)` — Redis 直近15件→SQLite フォールバック）。
3. **返信チェーン解決**（`resolveReplyChain`、深さ `config.replyChainMaxDepth`=10）。解決結果は先頭に `[返信チェーン…]` テキスト＋ダミー model 応答として `contents` へ前置。加えて **L2 recall のクエリ組み立てにも再利用**。
4. 履歴を Gemini `Content[]` へ変換。**同ロール連続は結合**して user/model が交互になるよう整形（Gemini の制約対策）。
5. 画像/音声は `inlineData`（base64）として直近 user コンテンツへ添付。

### ペルソナ適用・システムプロンプト構築（`buildSystemInstruction`、`:119`）
順序が固定: **ペルソナ → 記憶ツール使い分けルール → 承認フロー → リッチ返信ルール → 音声メモルール → リアルタイム情報の正確性 → 機能一覧 → システムルール（現在日時・カレンダー一覧）→ コンテキストノート（末尾）**。
- ペルソナ: `(user_id, bot_id)` 毎に独立（`getActivePersonaPrompt`）。未設定時は `DEFAULT_PERSONA`。汎用モードは Bot 単位ペルソナ（`buildGuildSystemInstruction`、`:1244`、注入順 ペルソナ→共有ノート→発話者個人ノート）。
- Google カレンダー連携時はカレンダー一覧を注入し `addSchedule` の `calendar_id` 選択を誘導。
- `docs/skills/search_skills.md` を起動時1回だけ読み込み（`loadSearchSkills`）、検索スキル仕様書としてインライン注入。

### シナプス recall 注入（`buildRecallSection` / `buildRecallQuery`、`:295`〜`:351`）
- 秘書スコープ `{ userId, botId }`（guild_id なし）。返信時は返信元直近2件＋現発話を結合し `RECALL_QUERY_MAX_LEN`=500 でキャップ（字面 n-gram 埋め込みの希釈回避）。
- `assembleRecall` を `synapseRecallK`(=5) で呼び、**時刻文脈再ランキング**（`SYNAPSE_TIME_BIAS_WEIGHT`=0.1、現在の時間帯/曜日）＋ **recency 加算ブースト**（`synapseRecencyWeight`=0.15、半減期時間指定）を渡す。
- ヒットした記憶は `touchSynapses` で鮮度更新。結果は `# 関連する過去の記憶（連想想起／参考情報）` として systemInstruction 末尾へ**追記**。エンジン無効/タイムアウト時は `""` を返し「直近履歴のみ」へデグレード。メトリクス `recall_hit`/`recall_empty` を記録。

### ツール登録（`:1060`〜`:1079`）
1. `resolveBotCapabilities(botId)` で Bot のケーパビリティ集合を得る。
2. `resolveEnabledModulesForUser(botId, userId)` でユーザー視点の有効モジュールへさらに絞る。
3. `getFunctionModulesForCapabilities(caps, enabledModules)` で静的モジュール群を取得。
4. `caps.has("mcp")` の場合のみ `getMcpFunctionModuleForBot(botId, userId)` で MCP 動的モジュールを取得。
5. `buildFunctionRegistry([...静的, mcpModule])` でマージ。

### ターンプラン → ツール呼び出しループ（`runPlannedTurn` `:793` → `runFunctionCallingLoop` `:501`）
- **プランナー**（Goal 1/2、`planTurn`）: 軽量 LLM に「コンパクトなツール索引」を渡し JSON スキーマ強制で `{steps, tools, weightMs, interim}` を得る。プランは `# 今回の処理プラン（内部の思考メモ）` として systemInstruction へ注入（`buildPlanSection`）。失敗時 null でデグレード。
- **重い処理の非同期化**（Goal 2）: `plan.heavy && asyncDelivery.deliverFinal` なら**一時応答を即返し**、本処理を背景 Promise で走らせ完了後 `deliverFinal` でフォローアップ送信。一時応答も会話ログへ残す（次ターンで「未応答」扱い→再実行される回帰を防止）。
- **ループ本体**（最大 `maxIterations`=10）:
  - `generateWithRetry`（429/5xx リトライ、`RetryInfo.retryDelay` 尊重の指数バックオフ、120秒タイムアウト）。
  - 応答から `functionCall` パーツを抽出→**各 fc を `registry.dispatch` で実行**→結果を `functionResponse` パーツとして `contents` へ push→再送信。
  - 各実行で `recordToolOutcome`（成功/失敗・レイテンシ・秘匿マスク済み引数ダイジェスト）を SQLite へ。`recordActions`（秘書のみ）で操作履歴を `actionRecorder` へ（Playbook 素材）。ブラウザ系ツールの成否を追跡。
  - **実行時エスカレーション**: 重いツール着手 or `heavyRuntimeMs`(=12000) 超過を検知したら一度だけ `onInterim`。
- **完了ハルシネーション是正**（`claimsActionCompleted` / `COMPLETION_CORRECTION_PROMPT`、`:480`〜`:594`）: ツールを1度も呼ばず「登録しました」等と自然文で完了主張した場合、`FunctionCallingMode.ANY` の `ToolConfig`（`allowedFunctionNames` はプラン候補ツールに限定）で**関数呼び出しを構造的に強制**して再生成（最大2回）。これが本コードベースの Function Calling 設計の核心的な安全弁。

### リッチ返信・最終文面確定（`finalize` `:849`）
- 最終テキストが空なら `fallbackText`（ブラウザ失敗を区別）で代替し、**必ず**会話ログへ保存。
- `ctx.embeds`（`showRichContent` が push）と `ctx.files`（`sendChart` の PNG）を `ProcessResult` として返却。`deferred` フラグで一時応答か最終かを示す。
- `onFinal` で `maybeExtractSynapse` を発火（R1: シナプス抽出）。

---

## 2. ToolContext / FunctionModule インターフェース（`src/types/contracts.ts`）

```ts
interface ToolContext {
  botId: string;
  userId: string;            // データ分離の必須キー
  guildId?: string;          // ギルド常駐 Bot のみ
  embeds: EmbedBuilder[];    // showRichContent が push するキュー
  files: { attachment: Buffer; name: string }[]; // sendChart が push
  richReplyEnabled: boolean; // false なら embeds/files への push 禁止
}

interface FunctionModule {
  declarations: FunctionDeclaration[];  // Gemini へ見せる宣言
  handlers: Record<string, (ctx, args) => Promise<string> | string>;
}

interface TurnAsyncDelivery {
  onInterim?: (text) => void | Promise<void>;
  deliverFinal?: (payload: {content, embeds, files, components?}) => Promise<void>;
}
```

`buildFunctionRegistry`（`registry.ts`）は複数 `FunctionModule` をマージし `{declarations, dispatch, has}` を返す。**宣言名・ハンドラ名の重複は throw**（起動時に検知）。`dispatch` は未知関数/例外を `{success:false, message}` の JSON 文字列に正規化（例外がループを壊さない設計）。ハンドラの戻り値は常に**文字列（多くは JSON 文字列）**で、`success:false` が失敗判定の統一契約。

---

## 3. Function モジュール全列挙

### カタログ駆動（`moduleCatalog.ts` の `MODULE_CATALOG`）
各エントリ: `{id（永続化キー・リネーム禁止）, module, cap, label, description, selectable, settingsKey?}`。`bots.enabled_modules` に `id` を保存。`selectable:false`（core）は常時有効。`cap` が `core` 以外で Bot が保持しなければ**宣言ごと除外**（LLM に見せない）。

ケーパビリティ型: `"persona" | "memory" | "mcp" | "secretary"`。プリセット: secretary=全部、mcp_assistant=`persona,memory,mcp`（secretary 無し=汎用モード）。

| モジュール（id） | cap | 提供ツール | 役割 |
|---|---|---|---|
| **todo** | secretary | addTodo, addSubtask, listTodos, completeTodo, deleteTodo, updateTodo, updateTaskProgress, getTaskDetail, listTodoTags, editTodoTags, listTasksByTag, getTaskUsageGuide, stopTodoRoutine, **organizeTaskPriorities**, **applyTaskPriorities** | タスク管理・タグ・優先度整理（organize→提示→承認→apply の2段確定）・ルーチン |
| **schedule** | secretary | addSchedule, listSchedules, deleteSchedule | 予定（Google カレンダー双方向同期、`local_only` で単発タイマー） |
| **timeline** | secretary | listDayPlan, createDayPlanBlock, deleteDayPlanBlock, addTimelineRecord | デイリータイムライン（行動計画・記録） |
| **reminder** | secretary | addReminder, listReminders, cancelReminder | cron 式含むリマインド |
| **finance** | secretary | addExpense, getMonthlySummary, getCategoryBreakdown, listRecentExpenses, getBudgetLimits, setBudgetLimit, deleteBudgetLimit, listPlannedPayments, addPlannedPayment, settlePlannedPayment, cancelPlannedPayment, **findSettlementCandidates**, linkPlannedPaymentTodo, linkPlannedPaymentReminder | 家計・予算・支払い予定・消込（レシート OCR 連携） |
| **browser** | secretary | fetchDynamicPage, takePageScreenshot, searchWeb, browserInteractiveOpen/Click/Type/Wait/Status/Close | Web 検索・ページ取得・対話ブラウザ自動操作（数値 ID 操作） |
| **credential** | secretary | listCredentialServices, addCredential, updateCredential, deleteCredential, **browserFillCredential** | 認証情報の暗号化保管・ブラウザへの直接注入（値は LLM に渡さない） |
| **playbook** | secretary | savePlaybook, findPlaybooks, getRecentActionHistory, runPlaybook, deletePlaybook | マクロ/Playbook（操作履歴ベース登録・承認後実行） |
| **note** | memory | appendContextNote, getContextNote, setContextNote | 長期コンテキストノート |
| **clipboard** | secretary | addClipboardEntry, listClipboardEntries, deleteClipboardEntry | TTL 付き一時メモ |
| **contact** | secretary | addContact, searchContacts, listContacts, updateContact, deleteContact | 連絡先 |
| **conversation** | memory | summarizeConversationTopic | 会話ログのトピック/期間要約 |
| **briefing** | secretary | configureBriefing, getBriefingConfig, configureReport, runBriefingNow | 朝刊/日報/週報 |
| **chart** | secretary | sendChart | 数値データのグラフ PNG 生成 |
| **richContent** | **core** | showRichContent | Embed カード（常時有効・selectable=false） |

### 汎用モード専用（`botAssistantFunctions.ts`、`getGuildAssistantFunctionModules`）
- `botMemberFunctions`（core・ギルドのみ）: addBotMember, removeBotMember, listBotMembers
- `botPersonalNoteFunctions`（memory）: appendMyNote, getMyNote, setMyNote
- `botGuildMemoryFunctions`（memory・ギルドのみ）: appendGuildNote, getGuildNote, setGuildNote, summarizeConversationTopic

### 束ね方・有効化の仕組み
- `registry.ts` = マージ機構（重複検知）。
- `index.ts` = 単なる再エクスポートのファサード。
- `moduleCatalog.ts` = 実体。3つの解決関数:
  - `getFunctionModulesForCapabilities(caps, enabledModules)` — 秘書系。cap ゲート＋ユーザー選択ゲート。
  - `getGuildAssistantFunctionModules(scope, caps)` — 汎用モード。core（richContent[+member]）＋ memory 時に note 群。
  - `getBaseFunctionModules()` — 後方互換フルセット。
- `browserModule.ts` / `richContentModule.ts` は専用ファイルに分離。`browserModule` は既存 `browserFunctions.ts`（`(userId, args)` 署名・変更禁止）への**アダプタ**（`ctx.userId` を渡すだけ）。

---

## 4. llmClient の API キー管理（`src/services/llmClient.ts`）

- **ユーザー別**: `getUserGenAI(userId)` — `getUserGeminiConfig` から `apiKeyEncrypted/Iv/Tag` を取り `decryptText`（AES-GCM）で復号。`userAICache`（Map、apiKey 一致でキャッシュ再利用）。モデルは `conf.model || "gemini-3.1-flash-lite"`。未設定/復号失敗は null（呼び出し側が管理 UI に警告）。
- **Bot 別**: `getBotGenAI(botId)` — `bots.gemini_api_key_*` を復号。`botAICache`。モデルは固定 `BOT_DEFAULT_MODEL = "gemini-3.1-flash-lite"`（Bot 単位モデル設定は Phase 2 課題）。汎用モードは**発話者が登録済みでも本人キーを使わない**（テナント分離）。
- 復号は `../utils/crypto.js` の `decryptText(cipher, iv, tag)`。
- 補助生成: `generateAuxText`（Function Call なし・リトライ2）／`generateAuxMultimodal`（音声文字起こし・画像解析）。いずれもユーザーキー前提で、キー無しは null（呼び出し側フォールバック）。タグ自動付与・Webhook 解釈・日報要約・ニュース要約等に利用。

---

## 5. シナプス（連想記憶）: Node 側呼び出しと抽出

### synapseEngine.ts — Rust デーモンとの IPC
- Rust バイナリ `yuuka-synapse` を `spawn(bin, ["daemon","--db",dbPath])` で**常駐**させ、**改行区切り JSON-RPC**（stdin/stdout）で通信。`SynapseDaemon` クラスが `pending` Map で id 相関、タイムアウト、`exit`/`error`/`stdin EPIPE` を握って**自動再起動**。バイナリ不在時は null で全 API がデグレード（現行=直近履歴のみ）。プロトコルは FROZEN 明記。
- 公開 API（すべて throw しない）:
  - `assembleRecall(scope, query, k, timeCtx?, recencyCtx?)` — **1st Hop 近傍探索**。timeout 2500ms（ユーザー応答を遅延させない）。時刻/recency 補正は正の重みのときのみ送信（既定は純粋意味 KNN）。
  - `indexSynapse(id, scope, topicId, content, timeCtx?, createdAtEpoch?)` — RAM 索引へ追加し**埋め込み(base64)＋model_version＋dim を返す**（Node が SQLite の唯一の書き手なので永続化は Node 側）。timeout 8000ms。
  - `forgetSynapse` / `reindexSynapses`（SQLite から RAM 再構築）/ `synapseHealth`。
  - `startSynapseEngine`（起動ウォームスタート）/ `stopSynapseEngine`。
- wire scope は snake_case（`user_id/bot_id/guild_id`）。id/command は封筒キーで末尾上書き、シナプス ID は別キー `sid`。

### 埋め込み・ベクトル索引・近傍探索（`src/rust_synapse/`）
- 依存ゼロ（tokio/serde/rusqlite/base64 のみ）。埋め込みは **`HashNgramEmbedder`（`model_version="hash-ngram-v1"`、FNV ハッシュ n-gram の決定論的レキシカル埋め込み）**。**外部 ML モデル不使用**。`onnx` フィーチャは空で用意され、将来 bge-micro INT8 を差し込む拡張点（`Embedder` トレイト経由）。
- 索引 `SynapseIndex`: `HashMap<Scope, Vec<Entry>>` の**スコープ別バケット**。`knn` は各 Entry と cosine を計算し、`time_ctx`（`weight*(time_affinity-0.5)`）と `recency`（`weight*recency_factor`、半減期減衰）を**意味スコアへ加算**して再ランキング（意味埋め込み自体には混ぜない）。
- 永続埋め込みは f32 little-endian バイト列で SQLite `synapses.embedding` に格納。起動時 `storage::load_index` で全件ロード（4バイト境界不正はスキップ）。

### synapseExtractor.ts — 抽出（LLM コストゼロ）
- `maybeExtractSynapse` はバックグラウンド・非 await・非 throw。
- **秘匿ガード2段**: `SECRET_GUARD_RE`（password/token/APIキー等のラベル語）＋ `containsSecretValue`（JWT/高エントロピー値の形状）。content 側も再チェック。architecture §9.3 の不変条件。
- 選別ヒューリスティック: 最小長 `MIN_USER_TEXT_LEN`=8、コマンド様（`^[!/.\$#@]`）除外、`MEMORABLE_RE`（好き/嫌い/習慣/誕生日/アレルギー/締め切り/覚え 等）にマッチ **or** `MEMORABLE_MIN_LEN`=80 以上で memorable。
- content は `MAX_CONTENT_LEN`=300 でキャップ。`deriveTopicId` は最長「名詞っぽい」トークン（32字）を粗く抽出。形成時の時刻文脈（ctxTod/ctxDow）と Unix エポックを記録。
- 永続化: Node が `insertSynapse`（SQLite）→ `indexSynapse`（RAM 索引・埋め込み受領）→ `updateSynapseEmbedding` で埋め込みバイト列を書き戻す。エンジン停止時は行だけ残り将来 `reindex` で補完。

### tool_outcome フィードバック（`db/toolOutcomeRepo.ts`）
- ループ内 `recordToolOutcome({userId, botId, guildId, topicId:null, toolName, argsDigest, status, latencyMs})`。引数は `buildOutcomeArgsDigest` で秘匿マスク。
- `getTopicToolStats` / `getToolOutcomeStats` を提供（**経験統計＝2nd Hop のトピック×ツール統計の素地**）。現状 `topic_id` は未付与（R2 課題）で、`assembleRecall` の戻り `tools` フィールドも現状ほぼ未活用のプレースホルダ。**recall→行動選択のフィードバックループは配管済みだが 2nd Hop（トピック紐付け）は未完**。

---

## 6. mcpDynamic: 動的 MCP ツールの取り込み（プラグイン設計の既存下地）

`getMcpFunctionModuleForBot(botId, speakerUserId)` が中核。

- **スコープ**: `system_default`（共有秘書）は `listServersGrantedToBotScoped(botId, speakerUserId)`（発話者本人が付与した許可＋システムレベル `user_id IS NULL` のみ）でクロステナント露出を防ぐ。所有 Bot は `listServersGrantedToBot(botId)`。`enabled===1` で絞る。
- **キャッシュ**: `ensureFreshToolsCache` が `tools_cache_updated` を見て TTL(=1時間) 超過なら `refreshToolsCache` で MCP サーバーへ再問い合わせ。NaN/不正日時は Infinity 扱いで必ず再取得。失敗しても旧キャッシュで続行。
- **スキーマ変換**: `jsonSchemaToGeminiSchema` が JSON Schema → Gemini `Schema` へ変換。`type` 配列（`["integer","null"]`=nullable、schemars 1.x の Option<T>）や `anyOf/oneOf/allOf`（null 以外の最初のサブスキーマへ委譲）を処理し、STRING への潰れを防ぐ。object/array/number/integer/boolean/string+enum をマップ。
- **名前生成と衝突回避**: `mcp{serverId}_{sanitized}`（63字上限）。サニタイズや切り詰めで別ツールが同名化する事故を `disambiguateFunctionName`（SHA1 短ハッシュ付与）で**捨てずに一意化**。
- **ハンドラ**: 実行時に (a) 利用可能性を**再検証**（無効化/削除されていないか）、(b) `getServerById` で最新レコード取得、(c) `addAuditLog("mcp.call", …, {botId, credentialOwner})` で監査（秘密値なし）、(d) `callTool` 実行、結果は 30000 字で切って `{success, server, tool, result}`。`requires_confirmation===1` は宣言 description に承認必須注記を埋め込み（LLM 側の承認フローに委ねる）。

**下地としての含意**: 「外部の宣言セット＋ハンドラを実行時に生成し、既存レジストリへ同一 `FunctionModule` 型でマージする」形は、将来のプラグイン機構そのもの。スキーマ変換・命名衝突回避・スコープ再検証・監査・TTL キャッシュが揃っており、任意プラグインを `FunctionModule` に射影できれば同じ配管に乗る。

---

## 7. Rust 移植の論点

### (A) Gemini 公式 Rust SDK 不在をどう埋めるか
現状の依存点は薄く、置換は現実的:
- **依存 SDK 面**: `getGenerativeModel({model, systemInstruction, tools:[{functionDeclarations}], toolConfig}, {timeout})` と `generateContent({contents})` の2 API、加えて `generationConfig.responseSchema`（プランナーの JSON 強制）、`inlineData`（base64 マルチモーダル）、`FunctionCallingMode.ANY`/`allowedFunctionNames` の `ToolConfig` のみ。これらは Generative Language REST API `v1beta:generateContent` の薄いラッパで再現できる。
- 推奨: `reqwest` + `serde` で **自前クライアント**を書く。`FunctionDeclaration`/`Schema`/`Content`/`Part`/`ToolConfig` を serde 構造体化。`SchemaType` enum とスキーマ変換（`jsonSchemaToGeminiSchema` 相当）を Rust に移す。schemars 由来の nullable/combinator 処理は Rust なら型から直接生成でき、むしろ有利。
- **リトライ/バックオフ**: `generateWithRetry` の `RetryInfo.retryDelay` パースは REST エラー body（`error.details[].@type == RetryInfo`）から復元。`tokio` + 指数バックオフ crate。
- **タイムアウト**: `reqwest` の per-request timeout（現状 120s/60s/20s）。
- **注意点**: `response.text()` は candidate.parts のテキスト連結、`functionCall` パーツ抽出も serde で明示的に。ストリーミングは現状未使用なので非対応で開始可。**Vertex ではなく AI Studio（API キー）系**なので認証は URL クエリ `?key=` のみで単純。プランナーの `responseMimeType: application/json` + `responseSchema` は REST で同名フィールドあり。

### (B) ツールレジストリをトレイトでどう表すか
現 `FunctionModule`（宣言配列＋名前引きハンドラ Map）を Rust トレイトへ:

```rust
#[async_trait]
trait Tool: Send + Sync {
    fn declaration(&self) -> FunctionDeclaration;
    async fn call(&self, ctx: &ToolContext, args: serde_json::Value) -> ToolResult; // 常に構造化
}
```
- **レジストリ**: `HashMap<String, Arc<dyn Tool>>`。現 `buildFunctionRegistry` の**名前重複 throw**は挿入時 `Result` で表現。`dispatch` の「未知関数/panic を `{success:false}` に正規化」は `catch_unwind` ではなく **`Result` を返す設計**にし、`ToolResult` enum（`Ok(Value)` / `Err(ToolError)`）へ統一。ハンドラ内 `try/catch → JSON` の現契約を型で保証できる。
- **戻り値契約**: 現状「文字列（多くは JSON）＋ `success:false` 判定」という緩い規約が随所に散っている（ブラウザ成否・tool_outcome の status 判定・完了ハルシネーション検知）。Rust では `ToolOutcome { status: Success|Error, payload: Value }` を**トレイト戻り値の第一級型**にして、この暗黙契約を明示化すべき（移植で最も価値が出る点）。
- **ケーパビリティ・ゲート**: `MODULE_CATALOG` は `struct ModuleEntry { id, cap, tools: Vec<Arc<dyn Tool>>, selectable, … }` の静的テーブル。cap/enabled ゲートは反復フィルタで等価。
- **ctx の可変副作用**: `ToolContext.embeds/files` への push（`showRichContent`/`sendChart`）は Rust では内部可変（`Mutex`/`RefCell` or ハンドラが `Vec` を返す関数型）へ。副作用を戻り値へ寄せる（`ToolResult` に `attachments` を含める）方が借用検査と相性が良い。
- **MCP 動的ツール**: `Arc<dyn Tool>` を実行時生成すれば静的ツールと同じ `HashMap` に自然に混ざる。命名衝突回避・スコープ再検証クロージャは、動的 Tool 実装が内部に保持する形へ移せる。**この動的マージ点がプラグイン API の trait 境界そのもの**になる。
- **既に Rust 側にある良い前例**: `synapseEngine` の FROZEN な IPC 契約と `Embedder` トレイト（`onnx` フィーチャ拡張点）は、同じ「トレイト＋差し替え可能実装」パターンで、ツールレジストリ設計の指針として流用できる。

---

### 参照した主なファイル（絶対パス）
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/gemini.ts`
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/services/llmClient.ts`
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/services/turnPlanner.ts`
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/services/synapseEngine.ts`
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/services/synapseExtractor.ts`
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/functions/{registry,index,moduleCatalog,mcpDynamic,browserModule,richContentModule}.ts` および各 `*Functions.ts`
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/types/contracts.ts`
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/db/toolOutcomeRepo.ts`, `db/synapseRepo.ts`
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/rust_synapse/src/{main,embedder,index,storage}.rs`, `Cargo.toml`
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/config.ts`, `services/botCapabilities.ts`

---

## [05] I have everything needed. The two Rust crates are independent (no workspace), sh
<sub>agentId: a6f3c2d8631688ff1</sub>

I have everything needed. The two Rust crates are independent (no workspace), sharing tokio/serde/serde_json but differing in heavy deps (reqwest/scraper vs rusqlite). Here is the complete inventory.

---

# 棚卸し報告: yuuka バックグラウンドサービス群・Rust常駐デーモン・外部連携

対象: `/home/suki/web/kawaii-music.moe/apps/yuuka`。全サービスは `src/index.ts` の起動シーケンス（`startWebServer` → `startBot` → cron群 → `startSynapseEngine` → `startMetricsLogging`）で開始される。

## 1. cron サービス群（node-cron）

すべて「毎分/毎時起床して DB を全ユーザー横断走査」型。多くが `cronMatchesNow`（`reportService.ts:26`）方式で「動的ジョブを作らず設定変更を即時反映」する設計。失敗は基本的に握りつぶし + 次ティック再試行。

| サービス | ファイル | スケジュール | 起動時即時実行 | 何をするか | 外部依存 | 失敗時挙動 |
|---|---|---|---|---|---|---|
| リマインドエンジン | `reminderEngine.ts` | `config.reminderCron`（毎分） | あり（§10復帰） | ①時刻指定リマインド送信＋`repeat_rule`(cron)を`cron-parser`で次回へ再スケジュール ②ToDo期限24h以内の未通知通知 ③Google同期予定のイベント前通知 | `cron-parser`, notifier(Discord) | `ticking`フラグで多重起動防止。送信失敗は`pending`のまま次ティック再試行。cron式破損は`markSent`で単発化し無限再送防止 |
| 支払い予定 繰り返し | `paymentRecurrenceService.ts` | `5 0 * * *`（毎日0:05） | あり | 期日超過の`repeat_rule`付き支払いを`advanceRecurring`（元行処理済み＋次回pending新規行生成）。過去期日は今日以降まで読み飛ばし（最大1000反復の安全弁） | `cron-parser` | `ticking`フラグ。cron破損はスキップしpending維持 |
| ルーチンタスク 繰り返し | `todoRecurrenceService.ts` | `10 0 * * *`（毎日0:10、支払いと時刻分散） | あり | ルーチンToDoの`due_date`を`advanceRoutine`で**同一行前進**（山積み防止）。終了条件: `repeat_count<=1` or `repeat_until`超過で`endRoutineById` | `paymentRecurrenceService.calcNextRecurringDueDate`を共用 | `ticking`フラグ。cron破損スキップ |
| 日報・週報 | `reportService.ts` | `* * * * *`（毎分、cron一致判定） | なし | 期間内の完了/持越ToDo・予定・支払い・家計収支・会話サンプルを集約→LLM要約（`generateAuxText`）→Embed配信 | LLM(Gemini), DB直SQL, notifier | LLM失敗は`buildFallbackText`（生データ）へ縮退 |
| 朝報（天気・ニュース） | `briefingService.ts` | `* * * * *`（毎分、cron一致判定） | なし | Open-Meteo天気(APIキー不要)＋`rss-parser`でユーザー登録フィード取得→LLMが3〜5件要約 | `fetch`(Open-Meteo), `rss-parser`, `ssrfGuard`, LLM | 天気失敗はメッセージ表示。RSS個別失敗は無視。LLM失敗はタイトル列挙フォールバック |
| バックアップ | `backupService.ts` | `15 * * * *`（毎時15分、間隔判定） | なし | ユーザー行のみ抽出した一時SQLite→`archiver`でZIP→個人Google Driveへタイムスタンプ付き新規アップロード→世代超過分を削除 | `better-sqlite3`, `archiver`, `googleDriveService` | ユーザー単位try/catch。世代削除失敗は警告のみ。一時ファイルは`finally`で掃除 |
| 誕生日リマインド | `birthdayReminderService.ts` | `0 8 * * *`（毎朝8時） | なし | 「翌日誕生日」の連絡先検出→年齢計算付き通知。`birthday_reminded_year`で年内重複防止 | notifier | 送信失敗時はマークせず翌日再試行 |
| クリップボードTTL掃除 | `clipboardCleanupService.ts` | `0 * * * *`（毎時0分） | あり | 期限切れ`clipboard_entries`を`deleteExpired`で削除 | DB(`clipboardRepo`) | try/catchでログのみ |
| Playbook定期実行 | `playbookScheduleService.ts` | **DB登録の個別cron式**（動的ジョブ、`activeTasks` Map管理） | 起動時にenabled全件登録 | ユーザー登録マクロを`processMessage`で本人のGeminiキー・データ文脈で実行。`playbook_runs`に実行履歴、結果をDiscord通知 | `gemini.processMessage`, notifier | `executePlaybook`内try/catch＋`registerCronJob`側でもcatch（未捕捉Promise拒否防止）。失敗は`finishRun('failed')`＋失敗通知 |
| メトリクス | `metrics.ts` | `setInterval` 5分（cronでない、`unref`） | — | インメモリのcounter/latency(p50/p95/avg)を定期`console.log`。プロセス再起動でリセット | なし | — |

補足: **Google Calendar 同期は cron ではない**。`scheduleFunctions.ts:189` で「予定取得ツール実行時」にオンデマンドで`syncGoogleCalendarToLocal`が呼ばれる（表示前に最大30日ぶん同期）。リマインドエンジンはその同期済みローカルキャッシュ(`schedules`)を読むだけ。

## 2. Rust常駐デーモン（IPC: 改行区切りJSON over stdin/stdout）

両デーモンとも `#[tokio::main]`、`BufReader(stdin).lines()` で1行1リクエスト、`stdout`へ`{json}\n`を書いて`flush`。Node側は`readline`で行分割し`id`で相関。**2つは別クレート**（`rust_crawler`, `rust_synapse`。ワークスペース化されていない）。

### 2.1 rust_crawler（`src/rust_crawler/src/main.rs`） — Node側: `browserService.ts`

**CLIモード**: `fetch <url>` / `fetch-js <url>` / `search <query>` / `screenshot <url> <output_path>` / `daemon`。
**IPC（`daemon`）プロトコル**:
- Request: `{"id":u64,"command":"fetch"|"fetch-js"|"search"|"screenshot","target":string,"extra":string?}`
- Response: `{"id":u64,"ok":bool,"result":string?,"error":string?}`（`result`は文字列単位。searchはJSON配列を文字列で運ぶ、screenshotは`output_path`をエコー）
- パース失敗は`id:0`固定のエラー応答。

各コマンド実体:
- `fetch`: `reqwest`（gzip/brotli/deflate、ブラウザ風ヘッダ、指数バックオフ3回、403/401は即中断）でHTML取得→`scraper`でDOM走査し独自Markdown化。100字未満はエラー扱い。
- `fetch-js`: `Chrome --headless --dump-dom --timeout=12000`でJSレンダリング後DOM取得→同Markdown化。
- `search`: Google + DuckDuckGo HTMLを`tokio::join!`で並列取得→RRF(k=60)＋権威性スコア(.go.jp等加点/matome等スパム減点)＋キーワード適合でランキング→上位8件をJSON。
- `screenshot`: Chrome CLI `--screenshot`を一時ディレクトリで実行→出力コピー。
- Chrome探索順: `CHROME_EXECUTABLE_PATH` env → 定番パス群 → `~/.cache/puppeteer/chrome/*/chrome-linux64(旧linux)/chrome`。

**Node側spawn（`browserService.ts`）**:
- バイナリ探索順(`CRAWLER_BIN_PATHS`): `src/rust_crawler/target/release/yuuka-crawler` → `target/debug/...` → `dist/bin/yuuka-crawler`。
- `CrawlerDaemon`クラスが常駐プロセスを保持（プロセス起動コストゼロ化）。`spawn(bin, ["daemon"])`, envに`CHROME_EXECUTABLE_PATH`注入。40秒タイムアウト。`error`/`exit`/`stdin error(EPIPE)`ハンドラで全pending reject＋`proc=null`（次リクエストで再起動）。
- **バイナリ不在時のデグレード**: `runRustCrawler`が`throw`→呼び出し側(`fetchCleanPageContent`/`searchWeb`/`takePageScreenshot`)が全て**Puppeteerフォールバック**へ縮退。`fetch`→`fetch-js`→Puppeteer(networkidle2)の三段構え。

### 2.2 rust_synapse（`src/rust_synapse/src/main.rs`） — Node側: `synapseEngine.ts`

「記憶の重い処理（埋め込み生成・RAMベクトル索引・近傍探索）をV8ヒープ外へ追い出す」独立プロセス。
**CLI**: `daemon --db <path> [--dim <N>]`（dim既定256）。`--db`省略時は env`YUUKA_DB_PATH`→`./data/yuuka.db`。起動時にread-onlyでDBを読み索引構築、失敗しても空索引で続行（panic厳禁）。
**IPCプロトコル（FROZEN）**:
- Request: `{"id":u64,"command":string, ...fields}`。1行を`Value`としてパースし、トップレベルから`id`/`command`を取得しつつ同`Value`からコマンド固有フィールドを`serde_json::from_value`で取り出す。
- **重要な設計**: `id`は「封筒(envelope)＝相関ID」。シナプスIDは衝突回避のため**別キー`sid`**で運ぶ。Node側は`{...fields, id, command}`の順で送り封筒id/commandを常に勝たせる。
- Response: `{"id":u64,"ok":bool,"result":object?,"error":string?}`（crawlerと違い`result`は**オブジェクト**）。

コマンド:
- `health` → `{model_version, dim, total}`
- `assemble`（scope, query, k, 時刻文脈now_tod/now_dow/time_weight, recency文脈now_epoch/recency_weight/recency_halflife_secs） → `{synapses:[{id,content,topic_id,score}], tools:[]}`（tools常に空=2nd Hop将来枠）。意味KNN後に時刻/recency再ランキング（各文脈は重み>0時のみ構築）。
- `index`（sid, scope, topic_id, content, ctx_tod/ctx_dow, created_at） → RAM索引へ挿入し`{embedding_b64, model_version, dim}`（永続化用埋め込みを返す）。埋め込み器は`HashNgramEmbedder`（ONNX差し替えは`make_embedder`のfeatureゲート枠）。
- `forget`（sid） → `{removed:bool}`
- `reindex` → read-onlyで開き直し再ロード。失敗時は現索引維持しエラー応答。

**Node側spawn（`synapseEngine.ts`）**:
- バイナリ探索順(`SYNAPSE_BIN_PATHS`): `src/rust_synapse/target/release/yuuka-synapse` → `target/debug/...` → `dist/bin/yuuka-synapse`（crawlerと同型）。
- `spawn(bin, ["daemon","--db",config.dbPath])`。`assemble`は2500ms（ユーザー応答を遅延させない）、`index`/`forget`/`reindex`は8000ms。
- **バイナリ不在時のデグレード**: `getDaemon`が`null`を返し（警告は一度だけ）、全公開API(`assembleRecall`等)が`null`を返す→呼び出し側は「現行挙動（直近履歴のみ）」へ縮退。**全APIが呼び出し側へthrowしない**契約。`startSynapseEngine`のウォームスタート失敗も投げない。

## 3. 外部連携ライブラリの用途

- **Puppeteer**（`browserService.ts`）: ①Rust不在/失敗時のフェッチ・検索・スクショのフォールバック ②`getInteractiveBrowser`によるユーザー別**永続対話ブラウザセッション**（`userDataDir`分離、5分無操作で自動クローズ）。`browserInteractive{Open,Click,Type,Wait,Status,Close}`でLLMがDOMを`data-yuuka-id`注釈付きMarkdownで観測しつつ操作。SSRFガード適用。
- **googleapis**（`googleCalendarService.ts` / `googleDriveService.ts`）: OAuth2(refresh_token復号)でCalendar v3（イベントCRUD＋双方向同期、5分キャッシュ）とDrive v3（バックアップアップロード/一覧/世代削除、scope=drive.file）。アカウント解決順: botId指定→bot割当、未指定→owner primary。
- **rss-parser**（`briefingService.ts`）: 朝報ニュースのRSS取得（timeout10s、フィード最大10件、SSRFガード）。
- **chart.js + @napi-rs/canvas**（`chartService.ts`）: サーバサイドでpie/doughnut/bar/horizontalBar/lineをPNG描画（Discordダークテーマ、CJKフォント自動検出）。家計・タスク等の可視化。
- **その他**: MCPクライアント（`mcpClient.ts`、JSON-RPC 2.0 over HTTP/SSE、session再初期化再試行だが副作用のあるtools/callはタイムアウト時に再試行しない）。Webhook受信（`webhookProcessor.ts`、HMAC-SHA256検証＋5分リプレイ防止＋LLM整形＋ToDo/リマインド変換）。secretService（Argon2id+AES-256-GCM、全操作監査ログ）。sessionService（Redis＋in-memoryフォールバック、ハッシュ化トークン、スライディングTTL）。

## 4. WebSocketチャットの turn 処理（chatChannelService 経由）

`server/chatWebSocket.ts` → `chatChannelService.handleChatMessage`。
- **直列化**: 接続ごとに `let chain = Promise.resolve()` を持ち、`msg`受信を`chain = chain.then(...)`で**逐次実行**（`queued`カウンタ、`MAX_QUEUED_TURNS=4`超で`rate_limited`）。`reset`は`handleReset`→`clearContext`。
- **handleChatMessage**: ①`config.desktopDemoComponents`かつ`/__demo_buttons`ならGemini抜きで即ボタン返却 ②`getUserGenAI`でGeminiキー未設定を事前弾き(`no_gemini_key`) ③`consumeRateLimit`（guildId空のユーザー軸）④`processMessage(botId,userId,chat,onStatusChange,asyncDelivery)`実行。
- **送出フレーム**（トランスポート非依存の`send`コールバック、notifierは通さない）: `ready`/`status(thinking|writing)`/`interim`/`done`/`push`/`update`/`error`。`onStatusChange`→status、deferred重処理は`asyncDelivery.onInterim`→interim、`deliverFinal`→push。最終は`done`（`serializeRich`でEmbed→APIEmbed JSON、Buffer→base64、ActionRow→API JSON）。`messageId`は`randomUUID`で合成（差分描画・interaction突合キー）。例外は`classifyError`で`rate_limited`/`internal`分類。

## 5. 論点: crawler/synapse を単一プロセス内 Tokio タスクへ統合できるか

**結論: 技術的には吸収可能。ただし現状の疎結合設計が意図的な安全弁になっており、統合は「単一バイナリ内マルチサービス」までに留めるのが妥当。Node子プロセスをやめて完全にNode常駐へ吸収するのは非推奨。**

統合を後押しする材料:
- 両デーモンのIPCフレーミングが**ほぼ同一**（改行区切りJSON、`{id,command,...}`→`{id,ok,result,error}`、`#[tokio::main]`、stdin/stdout）。差異は`result`が文字列(crawler)かオブジェクト(synapse)かのみで、統合バイナリなら`Value`に統一すればよい。
- 依存の共通部分が大きい（tokio full / serde / serde_json）。差分はcrawlerの`reqwest`+`scraper`+`ego-tree`、synapseの`rusqlite`+`base64`のみで衝突しない。
- Node側spawnロジック(`CrawlerDaemon`/`SynapseDaemon`)が**ほぼコピー**（pending Map、id相関、error/exit/EPIPEハンドラ、再起動）。1バイナリ`yuuka-engine daemon`にコマンド名前空間（`crawler.fetch` / `synapse.assemble`）を切って**1本のstdin/stdoutを共有**すれば、Node側も1クラスに統合でき保守面積が半減する。
- Rust内では両者を`tokio::spawn`の別タスクにでき、`assemble`(2500ms要求)のような低レイテンシ処理を、`fetch`/`fetch-js`(最大40s、外部I/O待ち)がブロックしない。現状crawlerデーモンは1行ずつ逐次処理でリクエストを直列化しているが、`tokio::spawn`で並行化すれば取り込みつつ改善できる。

統合を思いとどまらせる材料（＝現設計の価値）:
- **障害分離**: crawlerはChrome/reqwestで頻繁にクラッシュ・ハングし得る。現状は`exit`で当該デーモンだけ落ちて次リクエストで再起動し、synapse(RAM索引=数万件のベクトル)は無傷。**同一プロセスに束ねるとcrawler由来のクラッシュがsynapseのRAM索引を巻き添えにし、`reindex`(DB全読み)のコールドスタートが発生**する。特にsynapseは「重い処理をV8ヒープ外へ追い出す」のが存在理由なので、クラッシュ耐性の低いcrawlerと同居させるのは目的に反する。
- **メモリプロファイルが正反対**: synapseは長寿命・大RAM(索引常駐)、crawlerは短命バースト(Chromeサブプロセス乱立)。同一プロセスのメモリ会計が読みにくくなる。
- **Node子プロセスの完全排除（Tokio→N-API等でNode内へ）は非推奨**: synapseの存在意義そのものが「V8ヒープ外」。Node内スレッドに戻すと設計目的が消える。

**推奨する落とし所**: 「crawler/synapseを1つの`yuuka-engine`バイナリに統合し、内部を独立Tokioタスク＋コマンド名前空間で分離、ただしNodeからは引き続き**子プロセス**として起動」。これで(a)Node側spawn/IPCコードの重複排除、(b)ビルド/配布物の一本化(`dist/bin/yuuka-engine`)、(c)IPCの`{id,ok,result:Value,error}`統一、という保守メリットを得つつ、障害分離を残すなら**プロセスは2本のまま/1本のsupervisorが子タスクをそれぞれ別OSプロセスで監督**する構成も選べる。真の「単一プロセス1本」化は、synapseのRAM索引巻き添えリスクを許容できる場合のみ。

---
主要ファイル（絶対パス）:
- cron: `/home/suki/web/kawaii-music.moe/apps/yuuka/src/services/{reminderEngine,briefingService,reportService,backupService,birthdayReminderService,paymentRecurrenceService,todoRecurrenceService,clipboardCleanupService,playbookScheduleService,metrics}.ts`
- Rust: `/home/suki/web/kawaii-music.moe/apps/yuuka/src/rust_crawler/src/main.rs`, `/home/suki/web/kawaii-music.moe/apps/yuuka/src/rust_synapse/src/main.rs`
- Node IPC: `/home/suki/web/kawaii-music.moe/apps/yuuka/src/services/{browserService,synapseEngine}.ts`
- 外部連携: `/home/suki/web/kawaii-music.moe/apps/yuuka/src/services/{googleCalendarService,googleDriveService,receiptParser,chartService,mcpClient,secretService,webhookProcessor,sessionService}.ts`
- WSチャット: `/home/suki/web/kawaii-music.moe/apps/yuuka/src/services/chatChannelService.ts`（呼出元 `/home/suki/web/kawaii-music.moe/apps/yuuka/src/server/chatWebSocket.ts`）
- 起動配線: `/home/suki/web/kawaii-music.moe/apps/yuuka/src/index.ts:114-126`

---

## [06] I now have a complete picture. Let me write the inventory report as my final out
<sub>agentId: a5bbfa213c054a7ba</sub>

I now have a complete picture. Let me write the inventory report as my final output.

---

# 棚卸し報告: フロントエンド型連携 & ビルド/デプロイ (apps/yuuka)

対象ブランチ `feature/svelte-migration`。読了ファイル群は末尾に絶対パスで列挙。

## 1. 型連携の全体像

### 1.1 二層構造 (backend zod allowlist ↔ frontend 手書き型)

**backend 側 (真実の源になっているのは zod だが、範囲が極小):**
`/home/suki/web/kawaii-music.moe/apps/yuuka/src/types/apiViews.ts` には zod の `z.object()` ビューが **わずか 3 つ**しか無い(`botViewSchema` / `contactViewSchema` と、コメント上のもう1つ)。設計思想は良質:

- `z.object()` の既定 strip 挙動で「スキーマに無いキーは応答から構造的に除去」= フェイルクローズ。
- 機密列 (`*_encrypted` / `*_iv` / `*_tag` / `password_hash` / `salt`) は allowlist に載らないので DB レコードを丸ごと渡しても漏れない。
- 実際の適用は `botRoutes.ts:123` (`toBotView` → `botViewSchema.parse(...)`) と `personalRoutes.ts:39` (`contactViewSchema.parse(...)`) の **2 箇所のみ**。

**frontend 側 (手書き interface の海):**
`/home/suki/web/kawaii-music.moe/apps/yuuka/frontend/src/lib/api/types.ts` は 656 行・`export type|interface` が **101 個**、`*Response` 型が **50 個**。ここが bot/task/expense/schedule/reminder/timeline/persona/playbook/webhook/mcp/credential/delivery/admin/device 全ドメインの DTO を手書きで定義している。

### 1.2 手作業重複・乖離リスク (最重要指摘)

- **zod ビューは全体の 2 ドメインしかカバーしていない。** frontend の 101 型に対し backend の allowlist は 3 個。残り 48 の `*Response` はサーバの `routes/*.ts`(22ファイル) と `db/*Repo.ts` の実装形状を **人間が読んで写経している**。`types.ts` 冒頭コメント自身が「参照元: src/server/routes/*.ts, src/db/*Repo.ts」と明記=機械的保証が無いことを認めている。
- **同一型が 3 箇所で二重定義:** `BotView` は (a) `apiViews.ts` の `z.infer<typeof botViewSchema>`、(b) `types.ts` の手書き `interface BotView` (51-71行)、で **完全重複**。zod を single source にしていれば `z.infer` を frontend が import すれば済むのに、workspace 越えの import をしていないため手写しになっている。フィールド追加時に 2 箇所同時更新が必要。
- **キー名・形状の乖離が既にコメントで管理されている**(危険信号):
  - `timelineApi`: サーバは `blocks` キーで返すが `plan` ではない、と注記(types.ts:293)。
  - `webhooksResponse`: サーバは `endpoints` キーで返す(types.ts:420)。
  - `deviceApi.ts`: `types.ts の DeviceRecord は name 形状で不一致`と明記し、`DesktopDevice`(`device_name`/`current`)を **サービスファイル側で再定義**(deviceApi.ts:15-22)。同じ概念に2つの型がある。
  - `financeApi`: body は `description`/`limitAmount`/`plannedDate` (camelCase) だがレスポンスの `ExpenseRecord.memo` は snake_case、という「リクエストとレスポンスで語彙が違う」不整合を各メソッドの JSDoc で手当てしている。
  - これらは**型で守られておらず、コメントの正確さに依存**している。サーバがキー名を変えても TS はコンパイルエラーにならない。

### 1.3 ApiResponse エンベロープ規約

`type ApiResponse<T> = { success: boolean; message?: string } & T;` (types.ts:13)

- **data ラッパ無し・トップレベル直置き**。ペイロード(`bots`/`tasks`/`stats`等)は `success` と同じ階層に並ぶ。`BotListResponse = ApiResponse<{ bots: BotView[] }>` のように交差型で表現。
- **エンベロープ例外**が2系統ある:
  - `SetupStatusResponse`(types.ts:32): `success` を持たない生オブジェクト。
  - OAuth device flow (`DeviceCodeResponse` / `PollResult`): OAuth authorization 形状 (`device_code`/`error:"authorization_pending"`)。汎用 `request()` を通すと 200+error を握り潰すため `frontend/src/lib/api/device.ts` に**専用実装**を分離。

## 2. apiClient の規約 (`frontend/src/lib/api/client.ts`)

### 2.1 scope 注入 (opt-out を型で強制)

- `RequestOpts.scope: Scope` は **必須** (省略不可)。`scope:'bot'` は `activeBot` store の id を必ず注入、`scope:'user'` は絶対に注入しない。
- opt-in(フラグ時のみ注入)を明示的に不採用:注入漏れで bot-scoped API が silent に system_default へ落ちる事故を型で封じる設計判断(client.ts:8-11)。
- **注入経路の分岐** (client.ts:83-100):
  - `GET` / `FormData(multipart)` → `botId` を **query** に付与 (body を JSON 化しないため)。
  - JSON body → **body に merge** (`{...base, botId}`)。サーバは body 優先で両対応 (todo/finance/schedule/delivery/botAttribute で確認済みと注記)。
- `/api/bots` 配下・auth系・device・setup は「プレフィックス一致で除外」…ではなく、**実装上は各サービスファイルが `scope:'user'` を手で付ける責務**(`botApi.ts`/`authApi.ts`/`deviceApi.ts`/`adminApi.ts` 全て `const USER = {scope:'user'}`)。botId が要る所は呼び出し側が `query:{botId}` を明示(`botApi.usage`/`shares`)。

### 2.2 401 集中割り込み (client.ts:110-116)

- 401 で `currentUser.set(null)` を必ず実行。
- `isBootstrap:true`(起動時 `/api/me` プローブ) または公開ルート上(`isPublicPath`)では `/login` へ遷移させず匿名扱い。それ以外は `goto("/login")`。
- 常に `ApiError(401)` を throw。

### 2.3 success:false 複合判定 (client.ts:124)

`if (!res.ok || data.success === false) throw new ApiError(...)`
HTTP 200 でも `success:false` を失敗として扱う複合判定。`res.json().catch(()=>({}))` で JSON パース失敗も握る。`credentials:'same-origin'` 明示(dev は Vite proxy `changeOrigin:false` で same-origin 維持し Cookie を通す)。

## 3. ビルド (Dockerfile マルチステージ)

`/home/suki/web/kawaii-music.moe/apps/yuuka/Dockerfile` は **4 ステージ**(親書の3ステージ記述より1つ多い):

1. **rust-builder** (`rust:1-bookworm`): `yuuka-crawler`(ブラウザ不変層) と `yuuka-synapse`(記憶エンジン schema v10) を `cargo build --release`、`/yuuka-crawler`・`/yuuka-synapse` へ cp。
2. **desktop-builder** (`rust:1-bookworm`): mingw-w64 で `x86_64-pc-windows-gnu` クロスコンパイルし Windows `.exe` 生成。`ARG YUUKA_API_BASE` を exe に焼き込む(空文字は `option_env!` が `Some("")` を返し壊れるため unset)。**ビルド失敗してもデプロイをブロックしない**(`/out` を常に mkdir、失敗時は空=配布なし)。
3. **node-builder** (`node:24-bookworm`): pnpm@9.15.0 固定(pnpm11は overrides 無視で lockfile 不一致)。`pnpm install --frozen-lockfile` → **ビルド順序厳守: `tsgo` → `vite build` → `pnpm prune --prod`**。prune を先にすると vite/svelte(devDeps)が消え vite build が落ちるため順序が load-bearing。rust バイナリを `dist/bin/` へ、desktop exe を `dist/downloads/` へ COPY。
4. **runtime** (`node:24-bookworm-slim`): `TZ=Asia/Tokyo` 固定(SQLite `datetime('now','localtime')` が JST 前提)、system chromium + CJK/emoji フォント + tzdata。`node_modules`(prune済) と `dist` と `src/assets`(canvas 等の実行時資産・不可侵) を COPY。`USER node`(uid1000)、`EXPOSE 7854`。

### フロント出力先と静的配信の接続

- vite `outDir: ../dist/public`(frontend/vite.config.ts:50)、`emptyOutDir:true`(dist/public 配下のみクリア=dist/直下の tsgo 出力は無事)、`assetsDir:'assets'`、`assetsInlineLimit:0`(CSP `script-src 'self'` 準拠で inline/data-URI を出さない)。
- 配信は `src/server.ts` が `dist/public` を解決(server.ts:64-67)。ハッシュ資産(`-XXXXXXXX.js`)は `Cache-Control: public, max-age=31536000, immutable`(server.ts:161)、`index.html` は毎回 GSV meta を実行時置換(server.ts:183)するため precache 禁止(vite の PWA 設定で html を globPatterns から除外し NetworkFirst)。CSP は `script-src 'self' https://static.cloudflareinsights.com`(server.ts:83)。
- **verify がこの接続を e2e で検証**: instance.sh:90-109 が CSP に `script-src 'self'`、index.html の hashed module 参照(8桁 hash 正規表現は server.ts:157 の immutable 閾値と連動)、`/assets/*.js` の immutable を curl でチェック。

## 4. デプロイ

### docker-compose (パラメータ化・全インスタンス共用)

`docker-compose.yml`: 単一定義を prod/dev で使い回す。インスタンス差分は `deploy/<name>/instance.env`(`--env-file`) と `deploy/<name>/secret.key` で注入。

- **シークレット注入の肝**: 暗号化シークレットは `$` を含むため compose の変数展開を通さず、`environment:` の **pass-through 形式(値なし)** で literal 注入(compose.yml:31)。`instance.sh` が `YUUKA_ENCRYPTION_SECRET="$(cat secret.key)"` で `$` を再展開せず export(instance.sh:59)。`secret.key.new` があれば鍵ローテーション用に `_NEW` を export。
- volumes: `DATA_DIR:/app/data`、`CONFIG_FILE:/app/config.yaml:ro`。ports: `${BIND_ADDR:-127.0.0.1}:${HOST_PORT}:7854`。redis(キャッシュ専用・永続化なし `--save "" --appendonly no`)に `depends_on`。

### instance.sh (update / rollback / verify + hot)

- **update**: `yuuka:latest` を `yuuka:prev-<inst>` へ退避 → `dc build`(既定レイヤキャッシュ有効、`--no-cache` でフル) → `dc up -d` → `health_check`。
- **rollback**: `yuuka:prev-<inst>` を `yuuka:latest` へ tag し直して再作成。退避が無ければエラー。
- **verify**: `health_check` のみ (HTTP200 待機 + Botログイン件数 + 復号/decrypt エラー grep + §3 の CSP/hashed/immutable 検証)。
- **hot**: dev-hot モード (API=`tsx watch` + src/ マウント、Rust はイメージ内蔵、フロントはホストで `vite 5173` 併走 proxy)。同ポート/同DB を掴む通常 app を自動停止して排他。
- **prod/dev 分離**: `deploy/prod/` と `deploy/dev/` に各々 `instance.env`(COMPOSE_PROJECT_NAME=yuuka-dev, HOST_PORT=7855, DATA_DIR, YUUKA_API_BASE=https://yuuka-dev.kawaii-music.moe) + `config.yaml` + `secret.key`。`.example` 版を commit、実体は gitignore 前提。package.json scripts が `deploy`(prod update)/`deploy:dev`/`deploy:rollback` 等でラップ。

## 5. Rust 化時の二重管理解消の論点

現状の核心問題は **§1.2 の「backend 実装形状 → frontend 101型を人間が写経」**。Rust 化はこれを機械化する好機。

### 5.1 Rust型 → TS型 自動生成

- **`ts-rs` または `typeshare`** を採用し、Rust の response DTO 構造体に `#[derive(TS)]` / `#[typeshare]` を付けて `types.ts` を **build 時に生成**する。手書き `types.ts`(現656行)は生成物へ置換し、`frontend/src/lib/api/types.ts` を成果物ディレクトリに移す。
- **エンベロープの表現**: `ApiResponse<T>` は Rust 側で `struct Envelope<T> { success: bool, message: Option<String>, #[serde(flatten)] data: T }` として `#[serde(flatten)]` でトップレベル直置きを再現。ts-rs は flatten を交差型に落とせるので現行 `& T` 規約と一致させられる。
- **allowlist / 機密 strip の移設**: 現在の zod strip 相当は Rust では「serialize 用 DTO 構造体に機密列フィールドを持たせない」ことで**型レベルで構造的に保証**される(zod の parse ランタイム strip より強い)。`botViewSchema` の `has_gemini_key`/`has_token` のような派生 bool は DTO の `From<BotRecord>` 実装で計算。DB row 構造体と wire DTO 構造体を分離するのが要点。
- これで `BotView` の3重定義(zod infer / 手書き interface / DB row)が **Rust の DTO 1つ → 生成 TS 1つ** に収束。

### 5.2 scope / エンベロープ例外の扱い

- `scope('bot'|'user')` はサーバ実装ではなくクライアント規約なので生成対象外。ただし OpenAPI 的なルートメタ(パス・メソッド・auth・scope)を Rust 側マクロ/属性で宣言し、**サービス層(`services/*.ts` 20本)も生成**すれば `botApi`/`taskApi` の手書きも消せる(現状 JSDoc で管理している snake/camel 不整合も型に載る)。
- OAuth device flow(`PollResult`)はエンベロープ外の特殊形状。生成対象に含めつつ、client 側の専用ハンドリング(device.ts)は残す。

### 5.3 ビルドパイプライン組み替え

- 現状 node-builder が全 TS を持つ。Rust 化後は **rust-builder に codegen ステージを追加**: `cargo run --bin gen-types > frontend/src/lib/api/generated.ts`(または typeshare CLI)を `tsgo` の**前**に実行し、生成物を node-builder が消費。
- 生成物を **CI で drift チェック**(生成 → `git diff --exit-code`)し、Rust DTO と commit 済み TS の乖離をブロック。これが現行「コメント頼み」を機械保証に置換する最大の勝ち筋。
- サーバ本体が Rust(axum等)へ移るなら、静的配信(dist/public / immutable / CSP / GSV置換)も Rust 側 handler に移植が必要。instance.sh の verify(CSP/hashed/immutable curl 検証)は**サーバ実装非依存**なのでそのまま流用可 = 移行時の回帰検出に有効。
- desktop-builder / rust_synapse / rust_crawler は既に Rust なので、サーバ Rust 化で **単一 cargo workspace に統合**でき、マルチステージの rust-builder / desktop-builder / node-builder の3系統が rust-builder 系へ寄る(node-builder はフロント vite build 専用に縮小)。

### 5.4 残リスク / 注意

- 現行の「サーバは body 優先で botId 両対応」「サーバは `limitAmount`/`plannedDate` を読む」等のリクエスト語彙不整合は、生成では**リクエスト DTO も Rust で定義**すれば同時に解消するが、既存 route の後方互換(query/body 両受け)を Rust 側で維持する必要がある。
- zod のランタイム strip が持つ「将来追加列も自動で漏らさない」フェイルクローズ性は、Rust DTO の構造的分離で置換されるが、DB row 構造体を誤って serialize する事故を防ぐため `#[serde(deny_unknown_fields)]` や row/DTO 型の明確な分離規約を lint で強制すべき。

---

## 参照ファイル(絶対パス)
- 型連携: `/home/suki/web/kawaii-music.moe/apps/yuuka/src/types/apiViews.ts`, `.../src/types/contracts.ts`, `.../frontend/src/lib/api/types.ts`, `.../frontend/src/lib/api/client.ts`, `.../frontend/src/lib/api/device.ts`, `.../frontend/src/lib/api/index.ts`, サービス層 `.../frontend/src/lib/api/services/{botApi,financeApi,authApi,deviceApi,adminApi,taskApi}.ts`
- backend 適用点: `/home/suki/web/kawaii-music.moe/apps/yuuka/src/server/routes/botRoutes.ts:123`, `.../src/server/routes/personalRoutes.ts:39`, 静的配信 `.../src/server.ts:64-194`
- ビルド/デプロイ: `/home/suki/web/kawaii-music.moe/apps/yuuka/Dockerfile`, `.../docker-compose.yml`, `.../deploy/instance.sh`, `.../frontend/vite.config.ts`, `.../package.json`, `.../deploy/{prod,dev}/instance.env`

数量サマリ: frontend 手書き型 101個(うち `*Response` 50個) / サービス 20本 vs backend zod ビュー 3個・適用 2箇所 / route ファイル 22本。**乖離面積 ≒ 手書き101型のうち zod で守られているのは実質 2 ドメインのみ**。

---

## [07] Confirmed: existing crates use `Box<dyn Error>` and `Result<_, String>` — exactl
<sub>agentId: afd92e401346e996c</sub>

Confirmed: existing crates use `Box<dyn Error>` and `Result<_, String>` — exactly the anti-patterns the new architecture bans. I have what I need.

---

# 領域: 厳格エラーアーキテクチャ（anyhow 禁止）

Node/TS の「throw して上位で握り潰す」構造を、Rust では**すべての失敗経路に名前と型を与え、呼び出し側が `match` で分岐でき、機械的に強制される**構造へ置き換える。既存の `rust_synapse`（`Box<dyn Error>` / `Result<_, String>`）と `rust_crawler` も本アーキテクチャ配下に**移行対象**として組み込む（現状はこの制約に違反している。`src/rust_synapse/src/main.rs:337,368` の `Box<dyn Error>`、`:49` の `Result<_, String>` が具体的な負債）。

## 1. 層別エラー分類体系（error taxonomy）

ワークスペースを cargo workspace 化し（領域Aの前提）、エラーは**クレート＝層に一致させる**。各層は自分の具体エラー enum を1つ持ち、上位層のエラーはそれを `#[from]` で内包する。トップの `AppError` は「起動 supervisor が最終的に見る型」だけで、ビジネスロジックは層エラーを直接返す。

```
yuuka-error        … 全層共通の 0 依存 primitive（ErrorKind, Retryability, Redaction トレイト）
  ├ yuuka-config   … ConfigError      (fail-fast 専用。起動時のみ)
  ├ yuuka-crypto   … CryptoError      (AES-GCM/Argon2id 復号・鍵ローテーション)
  ├ yuuka-db       … DbError          (rusqlite ラップ・migration・スキーマ内省)
  ├ yuuka-redis    … CacheError       (Redis。全て回復可能=劣化縮退)
  ├ yuuka-auth     … AuthError        (session/bearer/CSRF/device flow/lockout)
  ├ yuuka-validate … ValidationError  (入力検証・prototype pollution 除去)
  ├ yuuka-gemini   … GeminiError      (REST クライアント・retry/timeout)
  ├ yuuka-discord  … DiscordError     (twilight gateway/http)
  ├ yuuka-google   … GoogleError      (OAuth/Calendar/Drive)
  ├ yuuka-plugin   … PluginError      (tool dispatch・MCP 動的ツール)
  ├ yuuka-ipc      … IpcError         (crawler/synapse デーモン IPC)
  └ yuuka-http     … HttpError → AppError へ集約（axum IntoResponse はここ）
```

**設計原則:** enum のヴァリアントは「HTTP ステータスへの写像」ではなく「**呼び出し側が異なる回復行動をとる境界**」で切る。`NotFound` と `Forbidden` を分けるのは、前者は 404 を返し後者は監査ログを打つ、という行動差があるから。逆に「DB の I/O エラー」と「DB の制約違反」を分けるのは、前者は retry 対象・後者は 400 だから。

## 2. thiserror による定義例

### 2.1 共通 primitive（`yuuka-error`）

回復可能／致命的の型的区別（制約2との接続点）を**トレイトで**持たせる。全層エラーがこれを実装し、supervisor と HTTP 層が統一的に問い合わせる。

```rust
// yuuka-error/src/lib.rs — 依存ゼロ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retryability {
    /// 一時障害。指数バックオフで再試行 or 劣化縮退してよい（Redis断・Gemini 5xx・DB busy）
    Transient,
    /// 恒久障害。同じ入力での再試行は無意味（検証失敗・NotFound・認可拒否）
    Permanent,
    /// 起動不能。プロセスは fail-fast で停止すべき（設定・シークレット不備）
    Fatal,
}

/// 全層エラーが必ず実装する。supervisor と HTTP 層はこれ経由でのみ判断する。
pub trait ClassifyError: std::error::Error {
    fn retryability(&self) -> Retryability;
    /// ユーザー/クライアントへ返してよい安全な文言。機密を含めない。
    fn public_message(&self) -> std::borrow::Cow<'static, str>;
    /// ログ相関用の安定コード（例 "db.busy", "gemini.rate_limited"）。
    fn code(&self) -> &'static str;
}
```

`Fatal` を型で持つのが制約2の核心。`ConfigError` と `CryptoError`（起動時鍵検証）だけが `Fatal` を返しうる。それ以外の層は `Transient`/`Permanent` のみを返す設計にし、「稼働中に Fatal が湧く経路」を型レベルで存在させない。

### 2.2 DbError（`yuuka-db`）

```rust
// yuuka-db/src/error.rs
use yuuka_error::{ClassifyError, Retryability};

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database is busy (contention)")]
    Busy,                                     // SQLITE_BUSY → retry
    #[error("row not found: {entity}")]
    NotFound { entity: &'static str },
    #[error("unique constraint violated: {constraint}")]
    Conflict { constraint: &'static str },
    #[error("foreign key violation")]
    ForeignKey,
    #[error("json column decode failed for {column}")]
    JsonColumn { column: &'static str, #[source] source: serde_json::Error },
    #[error("schema migration failed at step {version}")]
    Migration { version: u32, #[source] source: rusqlite::Error },
    /// 上記に分類できない下層エラー。丸呑みではなく「未分類」という名前を与える。
    #[error("sqlite error")]
    Sqlite(#[source] rusqlite::Error),
}

// rusqlite::Error → DbError は #[from] を使わず「分類する」変換にする。
// #[from] で素通しすると Busy/Conflict の区別が消えるため、明示的に From を書く。
impl From<rusqlite::Error> for DbError {
    fn from(e: rusqlite::Error) -> Self {
        use rusqlite::ffi::ErrorCode;
        match &e {
            rusqlite::Error::SqliteFailure(f, _) => match f.code {
                ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => DbError::Busy,
                ErrorCode::ConstraintViolation => {
                    // extended code で unique/fk を判別
                    if f.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY {
                        DbError::ForeignKey
                    } else {
                        DbError::Conflict { constraint: "unknown" }
                    }
                }
                _ => DbError::Sqlite(e),
            },
            rusqlite::Error::QueryReturnedNoRows => DbError::NotFound { entity: "row" },
            _ => DbError::Sqlite(e),
        }
    }
}

impl ClassifyError for DbError {
    fn retryability(&self) -> Retryability {
        match self {
            DbError::Busy => Retryability::Transient,   // 制約2: spawn_blocking + backoff で再試行
            _ => Retryability::Permanent,
        }
    }
    fn public_message(&self) -> std::borrow::Cow<'static, str> {
        match self {
            DbError::NotFound { .. } => "not found".into(),
            DbError::Conflict { .. } | DbError::ForeignKey => "conflict".into(),
            // 内部詳細（SQL・column名）は絶対に外へ出さない
            _ => "internal error".into(),
        }
    }
    fn code(&self) -> &'static str {
        match self {
            DbError::Busy => "db.busy",
            DbError::NotFound { .. } => "db.not_found",
            DbError::Conflict { .. } => "db.conflict",
            DbError::ForeignKey => "db.foreign_key",
            DbError::JsonColumn { .. } => "db.json_column",
            DbError::Migration { .. } => "db.migration",
            DbError::Sqlite(_) => "db.sqlite",
        }
    }
}
```

**要点:** `From<rusqlite::Error>` を `#[from]` で自動導出**しない**のが厳格性の肝。`#[from]` は下層エラーを1ヴァリアントに丸呑みし、`Busy`（retry すべき）と制約違反（400）の区別を消す。`? ` の人間工学は保ちつつ、変換関数で**分類を強制**する。棚卸し#2が指摘した「`busy_timeout` 未設定で並行時に即 `SQLITE_BUSY`」問題は、この `DbError::Busy → Transient` 経路 + 呼び出し側の backoff で型的に扱える。

### 2.3 GeminiError（`yuuka-gemini`）— retry 情報を型に載せる

棚卸し#4の `generateWithRetry`（`RetryInfo.retryDelay` 尊重）を型で表現する。

```rust
#[derive(Debug, thiserror::Error)]
pub enum GeminiError {
    #[error("rate limited by Gemini")]
    RateLimited { retry_after: Option<std::time::Duration> },  // REST error.details[].RetryInfo
    #[error("gemini upstream 5xx: {status}")]
    Upstream { status: u16 },
    #[error("request timed out after {0:?}")]
    Timeout(std::time::Duration),
    #[error("api key missing or invalid")]
    NoApiKey,                                  // ユーザーが未設定 → 管理UIへ誘導（劣化縮退）
    #[error("response had no candidates / blocked")]
    NoCandidates { finish_reason: Option<String> },
    #[error("failed to decode gemini response")]
    Decode(#[source] serde_json::Error),
    #[error("transport error")]
    Transport(#[source] reqwest::Error),
}

impl ClassifyError for GeminiError {
    fn retryability(&self) -> Retryability {
        match self {
            GeminiError::RateLimited { .. }
            | GeminiError::Upstream { .. }
            | GeminiError::Timeout(_)
            | GeminiError::Transport(_) => Retryability::Transient,
            GeminiError::NoApiKey
            | GeminiError::NoCandidates { .. }
            | GeminiError::Decode(_) => Retryability::Permanent,
        }
    }
    fn public_message(&self) -> std::borrow::Cow<'static, str> {
        match self {
            GeminiError::NoApiKey => "gemini api key not configured".into(),
            GeminiError::RateLimited { .. } => "temporarily rate limited".into(),
            _ => "assistant temporarily unavailable".into(),
        }
    }
    fn code(&self) -> &'static str { /* … "gemini.rate_limited" 等 */ "gemini.error" }
}
```

`retry_after` を `Option<Duration>` として**型に載せる**ことで、リトライループが「文字列パースで delay を再抽出」する現行の脆さを排除する。棚卸し#4のチャット層 `classifyError`（`rate_limited`/`internal` の2分類）は、この `GeminiError` の `code()` を直接使えば実装が消える。

### 2.4 認可・検証は「行動が違うから分ける」

```rust
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("no valid session or bearer token")]
    Unauthenticated,                      // → 401, currentUser.set(null) 相当
    #[error("insufficient role: need {need}")]
    Forbidden { need: &'static str },     // → 403 + audit log
    #[error("cross-site state change blocked")]
    CsrfRejected,                         // → 403（Sec-Fetch-Site/Origin/Referer 判定）
    #[error("account locked out")]
    LockedOut { retry_after: std::time::Duration },  // → 429（IP×アカウント）
    #[error("device authorization pending")]
    DevicePending,                        // → OAuth 200 + {error:"authorization_pending"}
    #[error("device code expired")]
    DeviceExpired,                        // → 410
}
```

`DevicePending`/`DeviceExpired` を**エラーヴァリアントとして持つ**ことで、棚卸し#6が指摘した「device flow が 200+error を返すため専用実装が要る」問題を、`IntoResponse` の写像側（§4）で一元的に処理できる。

## 3. `?` による合成と網羅的 match の指針

### 3.1 合成規約

- **層をまたぐ `?` は `#[from]` を許可**する（`HttpError` が `DbError`/`AuthError` を内包）。ただし**同一意味の下層 primitive（`rusqlite::Error` → `DbError`）は §2.2 の手書き `From` で分類を強制**し、`#[from]` を使わない。
- **`.map_err()` で握り潰す形（`let _ =` / `.ok()` / `.unwrap_or_default()` で失敗を捨てる）は §5 の lint で禁止**。捨ててよい失敗が本当にあるなら、その事実を型で表明する `fn ignore_cache_miss(r: Result<T, CacheError>) -> Option<T>` のような**名前付きヘルパー**を通し、grep 可能にする。

### 3.2 網羅的 match の強制

- 全エラー enum に `#[non_exhaustive]` を**付けない**。付けると呼び出し側の `match` にワイルドカードアーム `_ =>` が強制され、新ヴァリアント追加時のコンパイルエラー（＝分岐追加漏れ検知）が消えるため。**クレート境界内では網羅性を最大化する**。
- どうしても外部公開 API で後方互換が要る最上位 `AppError` のみ `#[non_exhaustive]` を許容。
- 呼び出し側が「一部だけ特別扱いし残りは共通処理」する場合は、`_ =>` ではなく**残りヴァリアントを明示的に列挙**するか、`match e.retryability() { Transient => …, Permanent => …, Fatal => … }` という**トレイト経由の網羅**に落とす。後者を推奨（新ヴァリアントが増えても `Retryability` の3値は固定なので分岐は安定）。

### 3.3 ログと伝播の分離（黙殺しない）

**原則: エラーは「発生地点で `?` 伝播するだけ・ログしない」、「境界（HTTP handler・supervisor・IPC ループ）で1回だけログする」。** 二重ログと握り潰しの両方を防ぐ。

```rust
// 悪い例（禁止）: ここでログして Err を捨てる = 黙殺
// if let Err(e) = repo.save(...) { tracing::warn!("{e}"); }

// 良い例: 伝播。ログは境界が担う。
repo.save(&note).await?;
```

劣化縮退（Redis 断など制約2）だけは「捨ててよい」が、その場合も**捨てた事実を必ず構造化ログに1行残す**。棚卸し#2の「書込失敗は警告のみ（SQLiteが正）」を型で表現する:

```rust
// Redis はキャッシュ＝失敗は縮退可。だが sink しつつ必ず観測する。
match cache.set_context(&key, &val).await {
    Ok(()) => {}
    Err(e) => tracing::warn!(code = e.code(), error = %e, "cache write degraded; SQLite is source of truth"),
}
```

これを乱用させないため、`CacheError` **だけ**が「warn してから続行」を許される層（`retryability()` が常に `Transient`）と規約で定め、他層で同じパターンが出たら code-review で弾く。

## 4. HTTP への写像（axum `IntoResponse`）

`yuuka-http` に `HttpError` を1つ置き、`IntoResponse` を実装。全 handler は `Result<Json<T>, HttpError>` を返す。**機密は `public_message()` 経由でのみ出す**（`Display`/`Debug` は絶対にレスポンスへ入れない）。

```rust
// yuuka-http/src/error.rs
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error(transparent)] Auth(#[from] AuthError),
    #[error(transparent)] Db(#[from] DbError),
    #[error(transparent)] Validation(#[from] ValidationError),
    #[error(transparent)] Gemini(#[from] GeminiError),
    #[error(transparent)] Plugin(#[from] PluginError),
    // …各層
}

impl HttpError {
    fn status(&self) -> axum::http::StatusCode {
        use axum::http::StatusCode as S;
        match self {
            HttpError::Auth(AuthError::Unauthenticated) => S::UNAUTHORIZED,
            HttpError::Auth(AuthError::Forbidden { .. })
            | HttpError::Auth(AuthError::CsrfRejected) => S::FORBIDDEN,
            HttpError::Auth(AuthError::LockedOut { .. }) => S::TOO_MANY_REQUESTS,
            HttpError::Auth(AuthError::DeviceExpired) => S::GONE,
            HttpError::Auth(AuthError::DevicePending) => S::OK, // OAuth 規約: 200+error body
            HttpError::Db(DbError::NotFound { .. }) => S::NOT_FOUND,
            HttpError::Db(DbError::Conflict { .. })
            | HttpError::Db(DbError::ForeignKey) => S::CONFLICT,
            HttpError::Validation(_) => S::BAD_REQUEST,
            HttpError::Gemini(GeminiError::RateLimited { .. }) => S::TOO_MANY_REQUESTS,
            // Transient な下層は 503、その他内部は 500
            e if e.retryability() == Retryability::Transient => S::SERVICE_UNAVAILABLE,
            _ => S::INTERNAL_SERVER_ERROR,
        }
    }
}

impl axum::response::IntoResponse for HttpError {
    fn into_response(self) -> axum::response::Response {
        let status = self.status();
        // ★ サーバ内部詳細は tracing にだけ流し、body には public_message のみ。
        //   5xx は error レベル、4xx は debug で観測（監査は Forbidden で別途）。
        if status.is_server_error() {
            tracing::error!(code = self.code(), error = ?self, "request failed (5xx)");
        } else {
            tracing::debug!(code = self.code(), "request failed ({})", status.as_u16());
        }
        // 現行 ApiResponse エンベロープ規約（success:false, message, トップレベル直置き）を再現
        let mut body = serde_json::json!({
            "success": false,
            "message": self.public_message(),
            "code": self.code(),
        });
        // OAuth device flow だけは success ではなく error フィールド形状
        if let HttpError::Auth(AuthError::DevicePending) = self {
            body = serde_json::json!({ "error": "authorization_pending" });
        }
        // Retry-After ヘッダを型から生成
        let mut resp = (status, axum::Json(body)).into_response();
        if let HttpError::Gemini(GeminiError::RateLimited { retry_after: Some(d) })
            | HttpError::Auth(AuthError::LockedOut { retry_after: d }) = &self
        {
            if let Ok(v) = axum::http::HeaderValue::from_str(&d.as_secs().to_string()) {
                resp.headers_mut().insert(axum::http::header::RETRY_AFTER, v);
            }
        }
        resp
    }
}
```

`self.code()` / `self.public_message()` は `HttpError` にも `ClassifyError` を実装して内包エラーへ委譲する（`match self { HttpError::Db(e) => e.code(), … }`）。

**機密漏洩の構造的遮断:**
1. body へ入るのは `public_message()` と `code()` **のみ**。`Display`（`#[error(...)]`）や `Debug` はレスポンスに到達不能（型的に `into_response` が触らない）。
2. `public_message()` は各エラーで**固定文言 or ホワイトリスト化した値**のみ返す（SQL・カラム名・上流メッセージ・スタックを含めない）。棚卸し#1のOAuth callback「上流エラーメッセージ非漏洩」「固定コードのみURLへ」がこの型契約で自動的に守られる。
3. これは棚卸し#6の zod allowlist（フェイルクローズ strip）の**エラー版**。「詳細を出すには `public_message` に明示追加が要る」= デフォルト非公開。

## 5. 禁止の機械的強制（CI ゲート）

### 5.1 clippy — workspace ルートの `Cargo.toml` で lint 昇格

`clippy.toml` や `#![deny]` の散在ではなく、**workspace の `[workspace.lints]` で一元管理**（Rust 1.74+）。各クレートは `[lints] workspace = true` の1行で継承する。

```toml
# ワークスペース root Cargo.toml
[workspace.lints.clippy]
unwrap_used         = "deny"
expect_used         = "deny"
panic               = "deny"
todo                = "deny"
unimplemented       = "deny"
unreachable         = "deny"
indexing_slicing    = "deny"   # a[i] パニックも禁止 → .get() 強制
missing_panics_doc  = "warn"
dbg_macro           = "deny"
print_stdout        = "deny"   # デーモンの stdout は IPC 専用。ログは stderr/tracing へ
map_err_ignore      = "warn"

[workspace.lints.rust]
unsafe_code = "forbid"

# 各メンバークレート Cargo.toml
[lints]
workspace = true
```

**「非起動パスでの `unwrap`/`expect` 禁止・起動パスは許容」の実現:** 起動時（設定パース・鍵検証）は `expect` を使いたくなるが、`expect_used = "deny"` は全域に効く。起動パスも `ConfigError`（`Fatal`）を `?` 伝播させ `main` が `Err(Fatal) → std::process::exit(1)` する形にして、**起動パスでも `expect` を使わない**のを推奨する。どうしても必要な箇所（テスト・`build.rs`・静的初期化）だけ**行単位 `#[allow(clippy::expect_used)]` + 理由コメント必須**とし、`allow` の総数を CI で数えて上限を設ける（grep で `#\[allow(clippy::` を数え、しきい値超過で fail）。これで「握り潰しの抜け穴」を可視化・監査可能にする。

### 5.2 cargo-deny — anyhow/eyre を依存グラフから banned

```toml
# deny.toml
[bans]
multiple-versions = "warn"
deny = [
    { name = "anyhow" },
    { name = "eyre" },
    { name = "color-eyre" },
    { name = "snafu" },        # 別系統だが方針統一のため（thiserror へ一本化）
    { name = "error-chain" },
]
# 推移的依存で anyhow が入るのは許容（自コードが使えないことが重要）だが、
# 直接依存だけを禁止したい場合は下記 skip でトップレベル判定に寄せる。

[bans.skip-tree]
# 例: reqwest 等が内部で anyhow を引く場合の推移的許可（自クレートの直接使用は上の deny が禁止）
```

`Box<dyn Error>` は crate ではないので cargo-deny では止まらない。これは **clippy カスタム lint が無いため grep ゲート**で禁止する（§5.3）。

### 5.3 CI ゲート（一括）

```bash
# ci/strict-errors.sh — 失敗で job を落とす
set -euo pipefail
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo deny check bans licenses advisories

# Box<dyn Error> / dyn Error の直接使用禁止（テスト除く）
if grep -rEn 'Box<\s*dyn\s+(std::error::)?Error|dyn\s+std::error::Error' \
     --include='*.rs' crates/ | grep -v '/tests/'; then
  echo "::error:: Box<dyn Error> is banned (制約1). Use a typed error enum."; exit 1
fi

# 黙殺パターン検知: `let _ = ...?` 相当・空 .ok() での失敗捨て
if grep -rEn 'let _ = [a-zA-Z_].*\(' --include='*.rs' crates/ | grep -v '// intentional-ignore:'; then
  echo "::error:: silent error drop detected. Annotate '// intentional-ignore: <reason>' or handle."; exit 1
fi

# #[allow(clippy::(unwrap|expect|panic)_used)] の総数上限（握り潰しの抜け穴監査）
allow_count=$(grep -rEc '#\[allow\(clippy::(unwrap_used|expect_used|panic)\)' --include='*.rs' crates/ | awk -F: '{s+=$2} END{print s+0}')
if [ "$allow_count" -gt 5 ]; then
  echo "::error:: too many strict-error allow escapes ($allow_count > 5)"; exit 1
fi
```

CI は `clippy -D warnings` + `cargo-deny` + 3本の grep ゲートで、**制約1の全条項（anyhow/eyre、Box<dyn Error>、unwrap/expect/panic/todo/unimplemented、黙殺）を機械的に不可能化**する。

## 6. Result 型エイリアス方針

**層ごとに1つのエイリアスを定義**し、クレート内では `Result<T>` と書く。**トップの神エイリアスは作らない**（`anyhow::Result` の再来になり分類が失われるため）。

```rust
// yuuka-db/src/lib.rs
pub type Result<T, E = DbError> = std::result::Result<T, E>;
// 呼び出し: fn get_user(&self, id: &str) -> Result<User> { … }

// yuuka-http/src/lib.rs
pub type Result<T, E = HttpError> = std::result::Result<T, E>;
```

- 層をまたぐ関数は**エイリアスを使わずフル型を書く**（`Result<T, DbError>` 等）。どのエラーを返すかが署名で自明になる。
- `std::result::Result` の shadow を避けるため、`use yuuka_db::Result;` は各モジュール冒頭で明示 import し、glob import しない。

## 7. 回復可能／致命的の型的区別 → supervisor 接続（制約2）

`Retryability` が制約2（自己復帰）と本領域の**唯一の接続面**。supervisor（領域C）は個別エラー enum を知らず、`ClassifyError::retryability()` だけを見る。

```rust
// 領域Cのサービス supervisor が使う契約（本領域が提供する型）
pub async fn run_supervised<F, Fut, E>(name: &str, mut task: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), E>>,
    E: yuuka_error::ClassifyError,
{
    let mut backoff = std::time::Duration::from_millis(500);
    loop {
        match task().await {
            Ok(()) => return,
            Err(e) => match e.retryability() {
                Retryability::Transient | Retryability::Permanent => {
                    // 稼働中は落とさない（制約2）。指数バックオフで再起動。
                    tracing::error!(service = name, code = e.code(), error = %e, "task failed; restarting");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                }
                Retryability::Fatal => {
                    // Fatal はここに来られない設計（起動時のみ）。来たら設定不変条件違反。
                    tracing::error!(service = name, code = e.code(), "fatal at runtime — invariant broken");
                    std::process::exit(1);
                }
            },
        }
    }
}
```

`main()` の起動シーケンス（棚卸し#3の migration→redis→web→bot→cron の直列起動）は逆に **`Fatal` を許容**する:

```rust
async fn main() -> std::process::ExitCode {
    match bootstrap().await {           // ConfigError/CryptoError(Fatal) を ? 伝播
        Ok(app) => { app.run().await; std::process::ExitCode::SUCCESS }
        Err(e) => {
            tracing::error!(code = e.code(), error = %e, "fatal bootstrap failure");
            std::process::ExitCode::FAILURE   // systemd Restart=always 前提
        }
    }
}
```

これで棚卸し#3の「unhandledRejection=継続 / uncaughtException=再起動」という Node の回復可能性二分が、Rust では **`Transient`/`Permanent`=supervisor 再起動（プロセス継続）/ `Fatal`=`exit(1)`** として**型で**表現される。panic 隔離（`tokio::task::JoinHandle` の `is_panic` 検知）は領域Cが担うが、その panic は「制約1で `unwrap`/`panic!` を禁止した結果ほぼ発生しない」ため、supervisor は主に `Err(e)` 経路で回る。

## 8. IPC デーモン（crawler/synapse）への適用

既存 `rust_synapse`（`Result<_, String>` / `Box<dyn Error>`）と `rust_crawler` を本アーキテクチャへ移行する。改行区切り JSON デーモンの **wire レベルのエラー**（`{id, ok:false, error:"..."}`）は残しつつ、**プロセス内部は `IpcError` 型**にする。

```rust
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("malformed request line")]
    BadРequestLine(#[source] serde_json::Error),
    #[error("unknown command: {0}")]
    UnknownCommand(String),
    #[error("missing field: {0}")]
    MissingField(&'static str),
    #[error("handler failed")]
    Handler(#[source] Box<dyn ClassifyError + Send + Sync>),  // ← trait object は ClassifyError に限定
}
```

**wire の `error` 文字列には `public_message()` を使う**（デーモンの stderr には `code` + `Debug` を tracing で出す）。棚卸し#5の「全 API が呼び出し側へ throw しない契約」は Rust では「デーモンプロセスは panic しない（`unwrap` 禁止で保証）＋ ハンドラの `Err` は `{ok:false,error}` へ写像」として型で担保される。**唯一 `Box<dyn ...>` を許すのはこの `Handler` ヴァリアントだけ**で、しかも `dyn Error` ではなく `dyn ClassifyError`（＝ retryability/code を持つ制約付き trait object）に限定する。これは §5.3 の grep ゲートで `dyn std::error::Error` のみを禁止し `dyn ClassifyError` は許可、という形で両立させる。

---

### この設計が満たす制約チェック
- **制約1（anyhow 禁止）**: thiserror 具体 enum・手書き `From` による分類強制・網羅 match（`#[non_exhaustive]` 不使用）・`?` 型付き伝播・clippy deny・cargo-deny bans・3本の grep CI ゲート。既存2クレートの `Box<dyn Error>`/`Result<_,String>` を移行対象として明記。
- **制約2（自己復帰）**: `Retryability{Transient,Permanent,Fatal}` を全エラーが `ClassifyError` で表明し、supervisor が型経由でのみ再起動/fail-fast を判断。`Fatal` は起動時型（Config/Crypto）に限定し稼働中経路に存在させない。
- **機密非漏洩**: HTTP body は `public_message()`+`code()` のみ、`Display`/`Debug` は到達不能。zod allowlist のエラー版フェイルクローズ。
- **黙殺禁止**: 伝播とログを境界で分離、劣化縮退は `CacheError`（常時 Transient）に限定し必ず1行観測、grep で `let _ =` を検知。

**主要参照ファイル（絶対パス・裏取り済）**: `/home/suki/web/kawaii-music.moe/apps/yuuka/src/rust_synapse/src/main.rs`（`:49` `Result<_,String>`、`:337`/`:368` `Box<dyn Error>` = 移行対象）、`/home/suki/web/kawaii-music.moe/apps/yuuka/src/rust_synapse/Cargo.toml`・`/home/suki/web/kawaii-music.moe/apps/yuuka/src/rust_crawler/Cargo.toml`（現依存にthiserror/anyhow無し＝新規追加）、`/home/suki/web/kawaii-music.moe/apps/yuuka/src/services/chatChannelService.ts:140`（`classifyError` → `GeminiError::code()` で置換可能）。

---

## [08] I have all the verified detail I need. Writing the design section now.
<sub>agentId: abc739ee50428c310</sub>

I have all the verified detail I need. Writing the design section now.

# 領域C設計: Webランタイム・ルーティング・ミドルウェア

## C.0 前提と結論の要約

axum 0.8 + tower / tower-http を採用する。理由: (a) tower の `Layer`/`Service` はミドルウェアを**型で合成**でき、認証レベル none|user|admin を後述の型付きルータで静的に強制できる。(b) `axum::extract::FromRequestParts` により `AuthenticatedUser` / `AdminUser` Extractor を作れば「adminルートに user だけ書いて admin チェックを忘れる」事故がコンパイルエラーになる。(c) tower-http に `CompressionLayer` / `ServeDir` / `SetResponseHeaderLayer` / `RequestBodyLimitLayer` が揃い、既存の生http実装(圧縮・静的配信・ボディ上限)を薄い設定で厳密置換できる。actix-web は独自 actor ランタイムで tokio ワークスティーリングと二重ランタイムになりがち、また Extractor の型合成が axum ほど素直でないため退ける。

現行 `routeRegistry.ts` の「線形走査+`method`完全一致+`:param`」は axum の `matchit` ルータ(radix tree)で**O(パス長)** に置換され、Node の O(ルート数) 線形走査より速い(絶対制約#3)。ただし**現行の先勝ちセマンティクスと `/proxy/mcp/:id/mcp` のような固定+パラメータ混在の優先順位**は matchit が静的セグメント優先で解決するため意味等価。

---

## C.1 crate workspace レイアウト

単一 `cargo workspace`。既存 `rust_synapse`/`rust_crawler` もこの workspace に取り込む(棚卸し#5の推奨「単一バイナリ `yuuka-engine`」を採用しつつ crate は分ける)。

```
apps/yuuka/
  Cargo.toml                # [workspace]
  crates/
    yuuka-foundation/       # config, error 基底, telemetry, supervisor, crypto
    yuuka-db/               # rusqlite + spawn_blocking, migrations, repo (領域: DB担当)
    yuuka-domain/           # DTO/wire型 (ts-rs derive), scope型, SessionUser 等
    yuuka-web/              # ★本領域: axum app, router, middleware, extractor, ws, static
    yuuka-discord/          # twilight bot supervisor (bot担当)
    yuuka-ai/              # gemini client, tool registry (ai担当)
    yuuka-engine/           # crawler+synapse 統合デーモン (services担当)
    yuuka-plugin-api/       # ★拡張点: Tool trait / RouteExtension trait を公開 (拡張性#4)
  bins/
    yuuka/                  # main: supervisor が web/bot/cron/engine を束ねる
    gen-types/              # ts-rs コード生成バイナリ (領域: frontend連携#5)
```

`yuuka-web` の依存(`crates/yuuka-web/Cargo.toml` 抜粋):

```toml
[dependencies]
axum = { version = "0.8", features = ["ws", "macros"] }
tower = { version = "0.5", features = ["util"] }
tower-http = { version = "0.6", features = [
  "fs", "compression-br", "compression-gzip", "set-header", "limit", "trace",
] }
tokio = { version = "1", features = ["rt-multi-thread", "net", "signal", "macros", "time"] }
hyper = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
yuuka-foundation = { path = "../yuuka-foundation" }
yuuka-domain = { path = "../yuuka-domain" }
```

`yuuka-web` は `yuuka-db`/`yuuka-ai` に**直接依存しない**。ハンドラが必要とする操作は `yuuka-domain` に定義したトレイト(`SessionStore`, `UserRepo` 等)越しに `AppState` から受け取る。これで web crate はドメインロジックのコンパイル時間・依存に巻き込まれず、テストで in-memory 実装を差せる。

---

## C.2 エラー型(絶対制約#1の中核)

`anyhow`/`eyre`/`Box<dyn Error>` は使わない。web層は**HTTPステータスへ全射する具体列挙型**を持ち、`IntoResponse` を手実装する。

```rust
// crates/yuuka-web/src/error.rs
use axum::{http::StatusCode, response::{IntoResponse, Response}, Json};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum WebError {
    #[error("認証が必要です")]
    Unauthenticated,
    #[error("管理者権限が必要です")]
    Forbidden,
    #[error("クロスサイトからのリクエストは許可されていません。")]
    CsrfRejected,
    #[error("リクエストボディが大きすぎます")]
    BodyTooLarge,
    #[error("不正なリクエストです: {0}")]
    BadRequest(&'static str),
    #[error("リソースが見つかりません")]
    NotFound,
    #[error("レート制限を超過しました")]
    RateLimited,
    // 下位層エラーは #[from] で型付き伝播（握り潰さない）
    #[error("セッションストア障害")]
    Session(#[from] SessionError),
    #[error("データベース障害")]
    Db(#[from] yuuka_db::DbError),
    #[error("ドメイン処理エラー")]
    Domain(#[from] yuuka_domain::DomainError),
}

impl WebError {
    fn status(&self) -> StatusCode {
        match self {
            WebError::Unauthenticated => StatusCode::UNAUTHORIZED,
            WebError::Forbidden | WebError::CsrfRejected => StatusCode::FORBIDDEN,
            WebError::BodyTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            WebError::BadRequest(_) => StatusCode::BAD_REQUEST,
            WebError::NotFound => StatusCode::NOT_FOUND,
            WebError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            // 内部障害はクライアントへ詳細を漏らさない（現行 500 JSON と等価）
            WebError::Session(_) | WebError::Db(_) | WebError::Domain(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }
    /// クライアントに見せる安全なメッセージ（上流の内部詳細は漏らさない）
    fn public_message(&self) -> String {
        match self {
            WebError::Session(_) | WebError::Db(_) | WebError::Domain(_) => {
                "内部エラーが発生しました".to_string()
            }
            other => other.to_string(),
        }
    }
}

impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        let status = self.status();
        // 内部障害は構造化ログへ具体型のまま記録（黙殺しない）
        if status == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!(error = ?self, "handler failed");
        }
        (status, Json(json!({ "success": false, "message": self.public_message() })))
            .into_response()
    }
}
```

ハンドラ戻り値は必ず `Result<T, WebError>`。`?` で下位の `DbError`/`SessionError` が `#[from]` 経由で型付き伝播する。**`.unwrap()`/`.expect()`/`panic!` はハンドラ内禁止**。機械的強制は C.9 の clippy 設定で行う。

現行 `dispatchRoute` の「ハンドラ例外→500 JSON(headersSent 未送信時のみ)」は、axum が `Response` を一度しか書かない構造なので `headersSent` チェック自体が不要になり、`IntoResponse` が等価に機能する。

---

## C.3 認証レベルの型付き強制(Extractor)

現行 `route.auth` の実行時チェックを**型で置換**する。none/user/admin を Extractor の有無で表す。

```rust
// crates/yuuka-web/src/auth.rs
use axum::extract::FromRequestParts;
use axum::http::request::Parts;

/// user 認証必須ルートはハンドラ引数に AuthUser を書く。
/// 無ければ user 情報が取れない＝admin/userルートを書けない → 認証漏れが型で防がれる。
pub struct AuthUser(pub SessionUser);
/// admin 認証必須ルート。role != admin なら抽出段階で 403。
pub struct AdminUser(pub SessionUser);
/// 任意認証（auth:"none" だがセッションがあれば使う。OAuth callback 等）。
pub struct OptionalUser(pub Option<SessionUser>);

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = WebError;
    async fn from_request_parts(p: &mut Parts, st: &AppState) -> Result<Self, WebError> {
        // Cookieセッション → Bearer の順（現行 resolveRequestUser と同順・先勝ち）
        let user = resolve_request_user(p, st).await?.ok_or(WebError::Unauthenticated)?;
        Ok(AuthUser(user))
    }
}

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = WebError;
    async fn from_request_parts(p: &mut Parts, st: &AppState) -> Result<Self, WebError> {
        let user = resolve_request_user(p, st).await?.ok_or(WebError::Unauthenticated)?;
        // admin 判定はここで毎回。role はセッション/DB由来を信頼（不変条件#4）
        if user.role != Role::Admin { return Err(WebError::Forbidden); }
        Ok(AdminUser(user))
    }
}
```

ハンドラ例:

```rust
// admin ルート: AdminUser を要求 → user だけで書くと型が合わずコンパイル不能
async fn admin_stats(AdminUser(_admin): AdminUser, State(st): State<AppState>)
    -> Result<Json<StatsView>, WebError> { /* ... */ }

// user ルート
async fn me(AuthUser(user): AuthUser) -> Result<Json<MeView>, WebError> {
    Ok(Json(MeView { discord_id: user.discord_id, username: user.username, role: user.role }))
}
```

`resolve_request_user` は httpHelpers.ts の `resolveRequestUser` を移植: Cookie→`SessionStore::get(sha256(token))`(スライディングTTL)→ null なら Bearer→`desktop_tokens` 照合。**両経路とも `Result<Option<SessionUser>, SessionError>`** を返し、Redis 不通は `SessionError` にせず in-memory フォールバック(棚卸し#2の memory Map 等価)へ縮退する(制約#2 graceful degradation)。「セッション取得失敗＝401」ではなく「トークン無効＝Ok(None)」「ストア障害＝degrade」を型で分ける。

Cookie 抽出は現行 `getSessionToken` を厳密移植: `isHttpsDeployment()`(config.baseUrl が https)なら `__Host-yuuka-session` のみ受理、開発では両名。発行は `setSessionCookie` 等価で `Path=/; HttpOnly; [Secure]; SameSite=Lax; Max-Age=...`。名前はデプロイ設定で決定(リクエスト毎推定しない)。

---

## C.4 ルーター構築と CSRF / ボディ上限 / proto汚染

### C.4.1 ルータ骨格

```rust
pub fn build_router(st: AppState) -> Router {
    let api = Router::new()
        // auth (none)
        .route("/api/setup/status", get(setup_status))
        .route("/api/login", post(login))
        // user
        .route("/api/me", get(me))
        .route("/api/tasks", get(list_tasks))
        // admin （AdminUser Extractor が強制）
        .route("/api/admin/stats", get(admin_stats))
        // proto汚染除去 + CSRF は下記レイヤで一括適用
        .layer(middleware::from_fn_with_state(st.clone(), csrf_guard))
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES)); // 10MB → 413

    let proxy = Router::new() // auth:"none" 群。CSRFレイヤを付けない
        .route("/proxy/mcp/{id}/mcp", post(mcp_proxy).options(mcp_proxy_preflight))
        .route("/hook/{token}", post(webhook_receive));

    Router::new()
        .merge(api)
        .merge(proxy)
        .route("/ws/chat", get(ws_chat))
        .fallback(static_or_spa)              // C.6 静的配信+SPAフォールバック
        .layer(SetResponseHeaderLayer::if_not_present(/* HSTS 本番のみ */))
        .layer(middleware::from_fn(https_redirect))  // C.4.4
        .layer(middleware::from_fn_with_state(st.clone(), cors_reflect)) // C.4.5
        .with_state(st)
}
```

`:param` は axum 0.8 で `{param}` 構文。現行 `matchPath` の `decodeURIComponent` は matchit がパーセントデコード込みで params を返すため等価。**セグメント数不一致=非マッチ**も radix tree の性質で同じ。

### C.4.2 CSRF レイヤ(不変条件#1の厳密移植)

現行 `isCrossSiteStateChange` を tower ミドルウェアへ。**POST/DELETE かつ auth≠none のみ**対象。auth≠none の判別は「CSRFレイヤを付けた `api` サブルータに載っているか」で表現し、`proxy`/`hook`/`ws` は物理的に別サブルータでレイヤ非適用にする(現行の `route.auth !== "none"` 分岐を**ルータ構造で**表す=より安全)。

```rust
async fn csrf_guard(req: Request, next: Next) -> Result<Response, WebError> {
    let m = req.method();
    if m == Method::POST || m == Method::DELETE {
        if is_cross_site_state_change(req.headers(), &CONFIG)? {
            return Err(WebError::CsrfRejected); // 403
        }
    }
    Ok(next.run(req).await)
}

fn is_cross_site_state_change(h: &HeaderMap, cfg: &Config) -> Result<bool, WebError> {
    // 1) Sec-Fetch-Site: cross-site → 拒否
    if h.get("sec-fetch-site").and_then(|v| v.to_str().ok()) == Some("cross-site") {
        return Ok(true);
    }
    // 2) Origin（!= "null"）→ ホスト名一致判定。パース失敗=拒否
    if let Some(origin) = h.get("origin").and_then(|v| v.to_str().ok()) {
        if origin != "null" {
            return Ok(match Url::parse(origin) {
                Ok(u) => !is_allowed_host(u.host_str().unwrap_or(""), cfg),
                Err(_) => true,
            });
        }
    }
    // 3) Referer 同様
    if let Some(referer) = h.get("referer").and_then(|v| v.to_str().ok()) {
        return Ok(match Url::parse(referer) {
            Ok(u) => !is_allowed_host(u.host_str().unwrap_or(""), cfg),
            Err(_) => true,
        });
    }
    // 4) 判定不能 → SameSite=Lax に委ねて許可
    Ok(false)
}
```

`is_allowed_host` は `routeRegistry.isAllowedHost` 移植(baseUrl設定時はそのホスト名のみ、未設定時 localhost/127.0.0.1/[::1]/::1)。

### C.4.3 ボディ上限 + proto汚染除去

10MB上限は `RequestBodyLimitLayer::new(10*1024*1024)`。超過は axum が `413 PAYLOAD_TOO_LARGE` を返す(現行 `readBody` の reject+destroy と等価。tower が接続レベルで打ち切る)。

**proto汚染対策の等価物**: Rust には `__proto__` プロトタイプ汚染は**言語的に存在しない**(serde_json は `Map<String,Value>` で `__proto__` は単なる文字列キー)。ゆえに `stripProtoKeys` の**移植は不要**。ただし JS フロント・下流の JSON 再シリアライズや MCP 中継先が JS の場合に汚染キーが通過しうるため、**JSON抽出器で危険キーを除去する薄いラッパ**を用意し等価な防御面を保つ:

```rust
/// serde_json::from_slice の後に __proto__/constructor/prototype を深さ6まで剥がす。
/// JS下流（フロント再構築・MCP中継先）へのプロト汚染持ち出しを防ぐ（不変条件#3の等価物）。
pub struct SanitizedJson<T>(pub T);
impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for SanitizedJson<T> {
    type Rejection = WebError;
    async fn from_request(req: Request, _s: &S) -> Result<Self, WebError> {
        let bytes = Bytes::from_request(req, _s).await
            .map_err(|_| WebError::BadRequest("body 読み取り失敗"))?;
        if bytes.is_empty() { /* body={} 相当は呼び出し側が Option で扱う */ }
        let mut v: Value = serde_json::from_slice(&bytes)
            .map_err(|_| WebError::BadRequest("JSON 不正"))?;
        strip_proto_keys(&mut v, 0);
        let t = serde_json::from_value(v).map_err(|_| WebError::BadRequest("スキーマ不一致"))?;
        Ok(SanitizedJson(t))
    }
}
```

Webhook の生ペイロード経路(`/hook/:token`)は `SanitizedJson` を使わず `Bytes` を直接取り、HMAC 検証を `rawBody` に対して行う(現行と同じ。JSONパース不能でも `body={}` で継続する挙動を、専用ハンドラで `Bytes`+署名検証として表現)。

### C.4.4 HTTPSリダイレクト + HSTS

`https_redirect` ミドルウェア: `config.baseUrl` が `https://` 始まり かつ `checkHttps(headers)`(=`x-forwarded-proto: https` or 接続 encrypted)が false かつ Host のホスト名が baseUrl と一致 → `301` で `https://` へ。HSTS は本番のみ全応答へ `Strict-Transport-Security: max-age=63072000; includeSubDomains`(preload無し)を `SetResponseHeaderLayer::overriding` で。tokio 背後のリバプロ想定で `x-forwarded-proto` を信頼するが、`trustedProxies` 検査(httpHelpers.getClientIp 移植)を通した peer のみ。

### C.4.5 CORS

tower-http の `CorsLayer` は「許可リスト反射 + credentials」を静的設定するが、現行は**baseUrl 有無で分岐する動的反射**(本番は baseUrl ホスト一致 origin のみ反射、未設定時のみ localhost プレフィクス)。この分岐は `CorsLayer::allow_origin(AllowOrigin::predicate(...))` で表現するより、明示的な `from_fn` ミドルウェア `cors_reflect` で書く方が現行ロジック(本番 localhost 反射禁止=資格情報反射防止)を**逐語的に**再現でき監査しやすい。→ **推奨: 手書き `cors_reflect`**。`CorsLayer` は「許可条件が静的」な単純ケース向けで、ここは条件が config 依存かつ credentials 反射のセキュリティ要件があるため手書きを選ぶ。

---

## C.5 静的配信(不変条件#11・棚卸し#6)

`tower_http::services::ServeDir` + `ServeFile` を土台にし、**事前圧縮 br/gzip + immutable キャッシュ + SPA フォールバック + CSP/セキュリティヘッダ**を重ねる。現行の「オンザフライ gzip」を**事前圧縮(vite build 成果物の `.br`/`.gz`)** に格上げする(制約#3の高速化: リクエスト毎圧縮を廃す)。

```rust
fn static_service() -> impl Service</* ... */> {
    ServeDir::new(&CONFIG.public_dir)
        .precompressed_br()          // *.br があれば Accept-Encoding: br で配信
        .precompressed_gzip()        // *.gz フォールバック
        .append_index_html_on_directories(false)
        .fallback(ServeFile::new(CONFIG.public_dir.join("index.html"))) // 拡張子なし=SPA
}
```

そのうえで `static_or_spa` fallback ハンドラが現行の細部を担う:

- **パストラバーサル防御**: `ServeDir` は `..` を正規化して境界外アクセスを拒否する(現行 normalize+前方一致と等価)。timeline media(`/`,`\`,`..` 拒否)は auth:"user" ルート側で `resolve_media_path` を移植。
- **SPA分岐**: stat 失敗かつ拡張子なし→`index.html`、拡張子あり→`404.html`(無ければテキスト)。ServeDir の `fallback` は一律 index を返すため、**fallback を包む薄いハンドラ**で「拡張子有無」を分岐する(現行 `serveStaticFile` L133-148 の意味を保持)。
- **immutable キャッシュ**: `assets/` 配下かつ内容ハッシュ命名(`-[A-Za-z0-9_-]{8,}\.(js|css|woff2?|png|jpe?g|svg|webp)$`)は `Cache-Control: public, max-age=31536000, immutable`。それ以外(index.html/theme-init.js/manifest.json/sw.js/404.html)は `no-cache, no-store, must-revalidate`。これは `SetResponseHeaderLayer` を path 述語で分けるより、応答生成時にパスを見て付与するのが確実。**instance.sh の verify(CSP/hashed/immutable の curl 検証)がサーバ実装非依存なので、この正規表現閾値を現行 server.ts:157 と一致させれば回帰検出がそのまま効く**(棚卸し#6)。
- **CSP/セキュリティヘッダ**: 全静的応答に現行と同一の CSP(`script-src 'self' https://static.cloudflareinsights.com` — inline JS 無し) + `X-Content-Type-Options: nosniff` + `X-Frame-Options: SAMEORIGIN` + `Referrer-Policy: strict-origin-when-cross-origin`。
- **GSV差し込み**: `index.html` のみ `config.google_site_verification` を `<!-- GOOGLE_SITE_VERIFICATION -->` に置換(無ければ空)。事前圧縮との整合のため **index.html は事前圧縮対象から外し**(現行も precache 禁止・NetworkFirst)、実行時に置換→都度 gzip か、置換後をメモリキャッシュ(config は起動時確定なので**起動時に1回置換して `Arc<Bytes>` 保持**が最速)。→ **推奨: 起動時1回置換+メモリ保持**。GSV は再起動まで不変ゆえリクエスト毎処理は無駄。

事前圧縮への移行は Dockerfile の vite build ステージに `vite-plugin-compression`(br+gzip 生成)を足すだけで、`ServeDir::precompressed_*` が拾う。

---

## C.6 WebSocket `/ws/chat`(不変条件#14)

`axum::extract::ws::WebSocketUpgrade`。upgrade 前に**Bearer のみ**でユーザー解決(Cookie 不使用=現行と同じ。ネイティブ Bearer は CSRF 非該当ゆえ Origin チェックなし)、`?botId=`(既定 `system_default`)に `has_bot_access` が false なら upgrade を拒否(403)。

```rust
async fn ws_chat(
    ws: WebSocketUpgrade,
    Query(q): Query<WsQuery>,
    State(st): State<AppState>,
    parts: Parts,            // Bearer 抽出用
) -> Result<Response, WebError> {
    let user = get_bearer_user(&parts, &st).await?.ok_or(WebError::Unauthenticated)?;
    let bot_id = q.bot_id.unwrap_or_else(|| "system_default".into());
    if !st.bots.has_bot_access(&user.discord_id, &bot_id).await? {
        return Err(WebError::Forbidden);
    }
    Ok(ws.max_message_size(st.cfg.desktop_max_upload_bytes)  // 添付上限
       .on_upgrade(move |socket| chat_connection(socket, user, bot_id, st)))
}
```

`chat_connection` は現行 `handleChatConnection` の不変条件を tokio task で:

- **1接続1ターン直列**: 現行の `chain: Promise` は、受信ループから `mpsc::Sender<Turn>` へ push、単一の**ターン処理 task が `mpsc::Receiver` から逐次 `recv().await`** することで直列化。`MAX_QUEUED_TURNS=4` は `mpsc::channel(4)` の容量で表現し、満杯時 `try_send` が `Err(Full)` → `{type:"error", code:"rate_limited"}` を返す(現行 queued カウンタと等価かつ**バックプレッシャが型で保証**される)。
- **ping/pong keepalive**: `PING_INTERVAL_MS=30_000` を `tokio::time::interval` で。`AtomicBool is_alive`、pong で true、無応答で `close`(現行 `terminate()` 相当)。
- **添付上限**: `max_message_size` で WebSocket フレーム段階で強制(現行のメッセージ後チェックより早い)。
- **メッセージ種別** ping/reset/msg/interaction は `serde` の `#[serde(tag="type")]` タグ付き enum で**網羅 match**。不正 JSON は `{code:"internal"}`(制約#1: `_ =>` で黙殺せず明示ブランチ)。
- **停止**: supervisor の `CancellationToken` を全接続 task へ配布。shutdown 時に `token.cancelled()` で `close` フレーム送信後 task 終了(現行 `terminate()`→`server.close()` 等価)。

---

## C.7 config.yaml → 型付き構造体の起動時厳密検証(制約#1・#2)

現行 `config.ts` は `getSetting` で env/yaml をゆるく読む。Rust では**起動時 fail-fast** の型付き検証にする。`serde` + `figment`(env と yaml の階層マージ)で読み、`Config::validate()` で致命的不備を列挙型で返す。

```rust
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("暗号化シークレット未設定: YUUKA_ENCRYPTION_SECRET も _NEW も無い")]
    MissingSecret,
    #[error("暗号化シークレットが32文字未満: {which}")]
    SecretTooShort { which: &'static str },
    #[error("PORT が不正: {0}")]
    InvalidPort(String),
    #[error("baseUrl の URL が不正: {0}")]
    InvalidBaseUrl(String),
    #[error("config.yaml 読み込み失敗: {0}")]
    Load(String),
}

pub struct Config {
    pub base_url: Option<Url>,
    pub port: u16,
    pub session_ttl_days: u32,
    pub desktop_token_ttl_days: u32,
    pub desktop_max_upload_bytes: usize,
    pub reply_chain_max_depth: u32,
    pub trusted_proxies: Vec<IpAddr>,
    pub public_dir: PathBuf,
    pub google_site_verification: Option<String>,
    secret_key: SecretString,          // Debug で伏字。ログ漏洩防止
    secret_key_new: Option<SecretString>,
}
```

**fail-fast の境界**(制約#2「本当に致命的」): 暗号化シークレット不備(現行 index.ts が `process.exit(1)`)・PORT/baseUrl 不正のみ起動停止。Redis 不通・engine バイナリ不在・Discord トークン欠落は**起動を止めない**(縮退)。`main` で `Config::load().and_then(Config::validate)` が `Err(ConfigError)` を返したら `eprintln!` + `std::process::exit(1)` — これは**起動パスなので `exit` を許可**(制約#1の「非起動パスでの panic 禁止」の例外境界)。secret は `secrecy::SecretString` で保持し `#[derive(Debug)]` に漏れない。

---

## C.8 マルチコア実行モデル & supervisor(制約#2・#3)

`#[tokio::main(flavor = "multi_thread")]`(worker_threads 既定=論理コア数)でワークスティーリングを効かせる。現行 Node は単一イベントループ(ワーカースレッド未使用)なので、CPU バウンド(JSON 直列化・静的圧縮・axum ルーティング)がコア数分並列化され制約#3を満たす。

**supervisor** は `bins/yuuka` に置き、web/bot/cron/engine を**個別 tokio task**で起動し panic を隔離する:

```rust
async fn supervise<F, Fut>(name: &'static str, token: CancellationToken, make: F)
where F: Fn() -> Fut + Send + 'static, Fut: Future<Output = Result<(), ServiceError>> + Send {
    let mut backoff = Duration::from_millis(500);
    loop {
        if token.is_cancelled() { return; }
        // task 境界で panic を隔離（JoinError::is_panic を検知）
        let handle = tokio::spawn(make());
        match handle.await {
            Ok(Ok(())) => return,                    // 正常終了
            Ok(Err(e)) => tracing::error!(service=name, error=?e, "service error, restarting"),
            Err(join) if join.is_panic() =>
                tracing::error!(service=name, "service PANICKED, restarting"), // プロセスは落ちない
            Err(_) => return,                        // cancel
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30)); // 指数バックオフ上限30s
    }
}
```

web サーバ自体は落ちてはならないので `axum::serve` を `supervise("web", ...)` 配下に置くが、**ハンドラ単位の panic は axum が `catch_unwind` 的にキャッチして 500 を返す**(tower の `CatchPanicLayer` を最外層に付与)ので、web task 全体の再起動は稀。`CatchPanicLayer::custom` で panic を `WebError` の 500 相当へ変換しログする(制約#1: 黙殺せず記録)。これで「1ハンドラの想定外 panic でプロセス全体が落ちる」を防ぎつつ、制約#1の「非起動パス panic 禁止」を**多層防御**(clippy で静的禁止 + CatchPanic で実行時網)する。

`Cargo.toml` は `panic = "unwind"`(既定)を維持。`panic = "abort"` にすると CatchPanicLayer も supervisor も無力化するため**禁止**(棚卸し#3の指摘と一致)。

graceful shutdown: `tokio::signal` で SIGINT/SIGTERM → `CancellationToken::cancel()` を全 supervise へ伝播、`tokio::time::timeout(Duration::from_secs(15), join_all(...))` で watchdog(現行 index.ts の 15秒 unref watchdog 等価)。

---

## C.9 clippy / cargo-deny による機械的強制(制約#1)

crate ルート `crates/yuuka-web/src/lib.rs`(および workspace 全 crate)に:

```rust
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![deny(clippy::todo, clippy::unimplemented, clippy::unreachable)]
#![deny(clippy::let_underscore_must_use, let_underscore_drop)] // let _ = fallible() 禁止
#![warn(clippy::dbg_macro)]
```

`clippy.toml`:
```toml
allow-unwrap-in-tests = true      # テストのみ許可
disallowed-methods = [
  # anyhow/eyre 系の混入を機械検出（依存に入れない前提だが二重防御）
]
```

`deny.toml`(cargo-deny)で **`anyhow`/`eyre`/`color-eyre` を依存グラフから禁止**:
```toml
[bans]
deny = [
  { name = "anyhow" }, { name = "eyre" }, { name = "color-eyre" },
]
```
CI で `cargo clippy --all-targets -- -D warnings` + `cargo deny check bans` を必須ゲートにする。これで制約#1(抽象エラー型全面禁止・非起動パス panic 禁止・エラー黙殺禁止)が**コンパイル/CI で機械強制**される。

---

## C.10 拡張性: プラグイン Web 拡張点(制約#4)

初期アーキテクチャに織り込む。`yuuka-plugin-api` が**ルート拡張トレイト**を公開し、user のカスタムモジュールが自前ルート群を axum `Router` として供給できる:

```rust
// crates/yuuka-plugin-api/src/lib.rs
#[async_trait]
pub trait RouteExtension: Send + Sync + 'static {
    /// プラグイン識別子（衝突検出用）
    fn name(&self) -> &'static str;
    /// このプラグインが提供するルータ。必ず /api/ext/<name>/ 名前空間配下に強制される。
    fn router(&self, state: PluginState) -> Router<AppState>;
    /// 要求する認証レベル（none/user/admin）。web層が Extractor で強制。
    fn required_auth(&self) -> RouteAuth;
}
```

`build_router` はコアルート登録後、レジストリの各 `RouteExtension` を `/api/ext/{name}/` 前置き + CSRF/ボディ上限/認証レイヤ**込み**でマージする。プラグインは生の axum ハンドラを書けるが、**認証・CSRF・ボディ上限・エラー変換(WebError)は web 層が強制**するため、プラグインが握り潰しエラーやプロト汚染防御漏れを起こせない(制約#1・#4の両立)。

これは棚卸し#4の MCP 動的ツール(`FunctionModule` を実行時マージ)と同じ「実行時に宣言セットを既存レジストリへ合流させる」パターンの Web 版であり、AI 層の `Tool` trait と対称的に設計する。rust_crawler/rust_synapse の「改行区切りJSONデーモン」パターンとの整合は、プラグインが外部プロセスを持つ場合 `yuuka-engine` の IPC 契約(`{id,command,...}`→`{id,ok,result:Value,error}`)を再利用する — web 層はプラグインルータから engine への呼び出しを `yuuka-domain` のトレイト越しに仲介する。

---

## C.11 フロント型連携との接点(制約#5)

本領域のハンドラ戻り値は `yuuka-domain` の wire DTO(`#[derive(serde::Serialize, ts_rs::TS)]`)のみ。**DB row 構造体を直接 `Json` で返すことを禁止**(clippy の `disallowed-types` で `yuuka_db::*Row` を web crate の `Json<T>` 型引数に出現させない lint、または row 型を web crate から `pub` に見せないモジュール境界で構造的に防ぐ)。これで現行 zod allowlist(機密列 strip のフェイルクローズ)を**型分離で置換**する — 機密列を持つ row 型が wire に到達しえない。エンベロープは `Envelope<T> { success: bool, message: Option<String>, #[serde(flatten)] data: T }` で現行 `ApiResponse<T> = {success} & T` を再現。`bins/gen-types` が `ts-rs` で `frontend/src/lib/api/generated.ts` を出力し、CI の `git diff --exit-code` で drift をブロックする。

---

## C.12 現行との差分・移植リスク(推測と事実の区別)

- **事実**: 現行 CSRF/proto/body/cookie/dual-auth ロジックは `routeRegistry.ts`・`httpHelpers.ts` を Read して逐語確認済み(本文の実装と一致)。上記移植はこの2ファイルの挙動を保存する。
- **推奨判断(単一化した箇所)**: CORS=手書き `cors_reflect`(CorsLayer でなく) / GSV=起動時1回置換メモリ保持(リクエスト毎でなく) / 静的配信=事前圧縮(オンザフライ gzip でなく) / bot管理の直列化=アクター(mpsc)化(現行 `startInFlight` Map でなく)。いずれも理由を各節に記載。
- **リスク**: 現行の「auth:"none" だがハンドラ内で `ctx.user` を手動確認」する OAuth callback(settingsRoutes)は、Extractor `OptionalUser` + ハンドラ内 state 検証で移植するが、**現行のフェイルクローズ(固定エラーコードのみURLへ、上流メッセージ非漏洩)を `WebError::public_message` の内部障害マスクで保証**する。MCP proxy の proxyToken 単独認証・毎回 canManage+enabled 再検証・`assertSafeOutboundUrl`(SSRF)・`ACAO: null`+Credentials 削除は auth:"none" 専用サブルータの独立ハンドラとして移植し、CSRF/Cookie レイヤを一切通さない(不変条件#7を構造で保証)。

参照した実ファイル(絶対パス): `/home/suki/web/kawaii-music.moe/apps/yuuka/src/server/routeRegistry.ts`, `/home/suki/web/kawaii-music.moe/apps/yuuka/src/server/httpHelpers.ts`, `/home/suki/web/kawaii-music.moe/apps/yuuka/src/config.ts`, `/home/suki/web/kawaii-music.moe/apps/yuuka/src/types/contracts.ts`。

---

## [09] Verified. The critical facts hold: destructive DROP branches at `migrations.ts:8
<sub>agentId: ac065835b52d91187</sub>

Verified. The critical facts hold: destructive DROP branches at `migrations.ts:882-894` (legacy) and `:1465-1470` (mcp FK), `busy_timeout`/`synchronous` unset, WAL+FK on, v5 wrapped in `db.transaction`, FK-off toggled in 3 rebuild sites, synapse embedding is Node-written f32 LE BLOB. Now writing the design section.

# DB層・マイグレーション・データ分離 — Rust設計

## 0. 結論(先出し)

| 論点 | 推奨 | 理由の核 |
|---|---|---|
| ドライバ | **`rusqlite` + `deadpool-sqlite`(spawn_blocking プール)** | rust_synapse が既に rusqlite。実行時スキーマ内省(`PRAGMA table_info`/`foreign_key_list`)依存の移行を忠実移植でき、`sqlx` のコンパイル時検査は動的DDLと衝突する |
| 書き込み並行 | **書き込み専用の単一コネクション + actor(mpsc)**、読みは別プール | 現行「書き手はNodeのみ」の不変条件を型で保つ。`SQLITE_BUSY` を構造的に排除 |
| マイグレーション | **`refinery` は使わず、内製の整数連番・前方専用・非破壊 runner** | DROP/再作成の暗黙ロスパスを廃し、各stepを型付きErrorで表現。既存の冪等内省ロジックをそのまま持てる |
| userId分離 | **`UserId` newtype + `Scoped<T>` リポジトリ + `AcrossUsers` 明示API** | v7で実際に起きたクロステナント穴を型で不変条件化 |
| Redis | **`fred`(接続supervisor内蔵) + `enum Store { Redis, Memory }` フォールバック** | 現行の「null返して各所で分岐」を単一の劣化縮退型に集約 |

---

## 1. ドライバ選定: sqlx か rusqlite か

### 判断: rusqlite を推奨

sqlx は魅力的(async native、`query!` マクロのコンパイル時検査)だが、**このコードベースでは決定的に不利**:

1. **実行時スキーマ内省が移行の中核**。`ensureColumns`(`migrations.ts:38-51`)は `PRAGMA table_info(x)` の結果を見て `ALTER TABLE ADD COLUMN` するか決める。`mcp_servers` の FK欠落判定は `PRAGMA foreign_key_list(mcp_servers)`(`migrations.ts:1459`)。sqlx の `query!` は**コンパイル時に実在DBへ接続してスキーマを固定**するため、「実行時にスキーマ形状が分岐する」この設計と根本的に噛み合わない(マイグレーション途中の中間スキーマに対する `query!` は書けない)。
2. **rust_synapse が既に rusqlite を使う**(`src/rust_synapse/src/storage.rs` が read-only で `synapses.embedding` をロード)。ドライバを揃えれば、将来サーバ本体と synapse を単一 cargo workspace に統合したとき依存が一本化する(領域#5 の統合方針と整合)。
3. **単一書き手・同期セマンティクスの忠実移植**。現行 better-sqlite3 は完全同期・`db.transaction(fn)` の同期クロージャ。rusqlite の `conn.transaction()` は同型で1:1移植できる。sqlx の async トランザクションは `.await` ポイントが増え、現行の「トランザクションは即座に閉じる」前提を崩す。

sqlx の唯一の勝ち筋であるコンパイル時検査は、後述の **型付きリポジトリ + `#[derive(FromRow)]` + テストでの全クエリ実行(smoke)** で実務上代替できる。

### Cargo 依存

```toml
[dependencies]
rusqlite = { version = "0.32", features = ["bundled", "serde_json", "blob", "functions", "trace"] }
# bundled: システムSQLite非依存でDockerランタイムを薄く保つ(現行 better-sqlite3 も同梱SQLite)
deadpool-sqlite = "0.9"     # spawn_blocking ベースの非同期プール
tokio = { version = "1", features = ["rt-multi-thread", "sync", "macros", "signal", "time"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
```

`bundled` を選ぶのは現行 better-sqlite3 が同梱SQLiteで動いており、Dockerランタイム(`node:24-bookworm-slim`相当)にlibsqlite3を入れずに済むため。FTS5(trigram)は `bundled` に含まれる。

---

## 2. 接続・PRAGMA・プール構成

現行 `database.ts` は WAL と `foreign_keys=ON` の**2つだけ**設定し、`busy_timeout`/`synchronous` は未設定＝既定依存。棚卸し#2 が指摘する通り、**Rustで複数タスク並行書き込みにすると即 `SQLITE_BUSY`**。これを設計で潰す。

### コネクション種別を型で分離

```rust
/// 書き込み専用の単一コネクション。actor が排他所有する。
pub struct WriteConn(rusqlite::Connection);
/// 読み取り専用プール。WAL により writer と並行して読める。
pub struct ReadPool(deadpool_sqlite::Pool);
```

全コネクション初期化で以下を**必ず**適用(現行の2つ + 欠落分):

```rust
fn init_conn(conn: &rusqlite::Connection, role: ConnRole) -> Result<(), DbError> {
    conn.pragma_update(None, "journal_mode", "WAL")?;      // 現行踏襲
    conn.pragma_update(None, "foreign_keys", "ON")?;        // 現行踏襲
    conn.pragma_update(None, "busy_timeout", 5000)?;        // ★新規: 5s。SQLITE_BUSY即死を防ぐ
    conn.pragma_update(None, "synchronous", "NORMAL")?;     // ★新規: WALでは NORMAL が耐久性/速度の定石
    conn.pragma_update(None, "wal_autocheckpoint", 1000)?;
    if matches!(role, ConnRole::Read) {
        conn.pragma_update(None, "query_only", "ON")?;      // 読み取りプールは物理的に書けない
    }
    Ok(())
}
```

`synchronous=NORMAL` は WAL 時に「電源断で最後のトランザクションを失い得るがDB破損はしない」保証で、SQLite 公式が WAL の推奨値とする。現行が既定(`FULL`)なので、これは**性能改善**(制約3: Nodeより高速)。

### 書き込み actor(単一書き手の型による強制)

```rust
enum WriteCmd {
    Exec { work: Box<dyn FnOnce(&mut rusqlite::Transaction) -> Result<WriteReply, DbError> + Send>,
           reply: oneshot::Sender<Result<WriteReply, DbError>> },
}

pub struct DbWriter { tx: mpsc::Sender<WriteCmd> }

impl DbWriter {
    /// 全書き込みは1本の actor task を通る = 並行書き込みが型レベルで不可能。
    pub async fn transact<T, F>(&self, f: F) -> Result<T, DbError>
    where F: FnOnce(&mut rusqlite::Transaction) -> Result<T, DbError> + Send + 'static, T: Send + 'static
    { /* oneshot で往復 */ }
}
```

actor 本体は `spawn_blocking` 上で `Connection` を所有し、mpsc を drain するループ。**これで現行「書き手はNodeのみ」を Rust の型で保証**する。読みはプールから並行に取れる(WAL の利点)。rust_synapse は依然 **read-only の別コネクション**で `synapses` を読むだけ(棚卸し#4: `embedding` は Nodeが唯一の書き手 → Rustでも「サーバ本体actor が唯一の書き手」に置換)。

### spawn_blocking で async に載せる

rusqlite は同期。`deadpool-sqlite` の `pool.get().await?.interact(|conn| { ... }).await` は内部で `spawn_blocking` する。読み取りリポジトリはこれを薄くラップ:

```rust
impl ReadPool {
    async fn query<T: Send + 'static>(
        &self,
        f: impl FnOnce(&rusqlite::Connection) -> Result<T, DbError> + Send + 'static,
    ) -> Result<T, DbError> {
        let obj = self.0.get().await.map_err(DbError::PoolExhausted)?;
        obj.interact(f).await.map_err(DbError::Interact)?  // InteractError も型で捕捉
    }
}
```

`interact` の `InteractError`(panic 隔離)を握り潰さず `DbError::Interact` に写す ── これが制約1(エラー黙殺禁止)の遵守点。

---

## 3. 型付きエラー(制約1の中核)

`anyhow` 全面禁止。DB層の失敗を網羅列挙する:

```rust
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("read pool exhausted")]
    PoolExhausted(#[source] deadpool_sqlite::PoolError),
    #[error("blocking task join failed")]
    Interact(#[source] deadpool_sqlite::InteractError),
    #[error("row not found: {entity} (scope={scope})")]
    NotFound { entity: &'static str, scope: String },
    #[error("json column '{column}' malformed")]
    JsonColumn { column: &'static str, #[source] source: serde_json::Error },
    #[error("migration {version} failed: {step}")]
    Migration { version: u32, step: &'static str, #[source] source: Box<DbError> },
    #[error("cross-tenant access attempted: {0}")]
    CrossTenant(&'static str),   // データ分離違反を型で表面化
}
```

呼び出し側は `match` で分岐可能。特に `JsonColumn` は現行の「不正JSONを空配列にフォールバック」(棚卸し#2 §4)を**型で明示**: リポジトリは `Result<Vec<T>, DbError>` を返し、フォールバックするか否かを**呼び出し側が判断**する(黙殺を関数境界の外へ出す)。

`rusqlite::Error` を `#[from]` で吸うが、これは「抽象化して握り潰す」ではなく **具体的な下位エラーを名前付きで保持**する使い方であり制約に合致する(`anyhow` は型情報を消すので禁止、`thiserror` の `#[from]` は型を保つので許可)。

### clippy/cargo-deny による機械強制(制約1)

`Cargo.toml` / `clippy.toml` / `deny.toml`:

```toml
# Cargo.toml (workspace lints)
[workspace.lints.clippy]
unwrap_used = "deny"
expect_used = "deny"
panic = "deny"
todo = "deny"
unimplemented = "deny"
let_underscore_must_use = "deny"   # let _ = fallible() の黙殺を禁止

# deny.toml (cargo-deny) — anyhow系の混入を物理的に阻止
[bans]
deny = [
  { name = "anyhow" }, { name = "eyre" }, { name = "color-eyre" },
]
```

非起動パスの `.unwrap()` を CI で落とす。起動時 fail-fast(§2 制約: シークレット不備等)だけは `main` 内の**明示的な `std::process::exit` パス**に隔離し、そこだけ `#[allow(clippy::expect_used)]` を局所付与して「致命的」と「回復可能」を型と lint 属性で区別する。

---

## 4. マイグレーション再設計(最重要・最大のリスク除去)

### 現状の危険(裏取り済み)

- **`migrations.ts:882-894`**: `currentVersion==="1" || hasLegacyUsers` で `LEGACY_TABLES` 16本を `DROP TABLE`。棚卸し#2 §B の警告通り、**Rust移行時に `system_settings` を引き継がず初期化すると `getCurrentSchemaVersion()` が `"1"` を返し、本番テーブルが全DROPされ得る**。
- **`migrations.ts:1465-1470`**: `mcp_servers` の FK欠落を内省検出したら無条件 `DROP TABLE`。登録済みMCPサーバーが消える。
- FK-off は `:196/888/1113` の3rebuild箇所でコネクション単位に切替。**プールで別コネクションを掴むと OFF が意図した文に効かない**(棚卸し#2 §B)。

### 判断: refinery ではなく内製 runner

`refinery` / `sqlx migrate` は「番号付きSQLファイルを順に流す」型で、**このコードベースの冪等・実行時内省マイグレーションと相性が悪い**(refinery は各migrationを1度だけ流す前提で、`table_info` を見て分岐する動的ステップを表現できない)。内製する。

**設計原則**:
1. **整数連番** `1..=N`。`system_settings.schema_version` は既存の文字列 `"17"` を引き継ぐ(`getCurrentSchemaVersion` 互換: 不在なら version 0 = 初回)。
2. **前方専用**。down は書かない(可逆考慮はするが、実運用はバックアップ前提)。
3. **各step は冪等かつ非破壊**。`DROP` を全廃。破壊が必要な再構築(FK追加等)は **`_new` テーブルへコピー→リネーム**の保持型のみ(現行の `message_logs` 再構築 `:1109-1135` と同型)。
4. **移行は書き込み actor の単一専用コネクションで直列実行**。FK-off/ON がコネクション単位で確実に効く。

```rust
pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    /// tx は移行専用コネクションのトランザクション。冪等に書く。
    pub up: fn(&rusqlite::Transaction) -> Result<(), DbError>,
}

pub const MIGRATIONS: &[Migration] = &[
    Migration { version: 1, name: "base_schema", up: m0001_base },
    Migration { version: 2, name: "bot_scoped_data", up: m0002_bot_scope },
    // ... v3..=v17 を既存関数から移植 ...
    Migration { version: 17, name: "timeline", up: m0017_timeline },
];

pub async fn run_migrations(writer: &DbWriter) -> Result<(), DbError> {
    writer.transact(|tx| {
        let current: u32 = current_schema_version(tx)?;  // 不在=0
        for m in MIGRATIONS.iter().filter(|m| m.version > current) {
            tx.pragma_update(None, "foreign_keys", "OFF")?;   // 再構築のため
            (m.up)(tx).map_err(|e| DbError::Migration {
                version: m.version, step: m.name, source: Box::new(e),
            })?;
            tx.pragma_update(None, "foreign_keys", "ON")?;
            set_schema_version(tx, m.version)?;
        }
        Ok(())
    }).await
}
```

> 補足: FK制約再有効化の検証を厳密にするなら、各step末尾で `PRAGMA foreign_key_check` を走らせ、違反行があれば `DbError::Migration` で停止する(現行にはないガードで、堅牢性向上=制約3)。全stepを1トランザクションに包むことで**途中クラッシュしても version が進まない**(現行の marker 依存の「一度きり」保証より強い原子性)。

### レガシーDROP分岐の廃止方針

**現行の「バージョン不一致 = DROP」を再現しない**。理由: これは開発初期(v1→v2)の一度きりの破壊であり、本番稼働中のDBには v17 が入っている。Rust移行runner は **version 0(=真の空DB)からのみ base schema を作り、既存本番DB(version 17)には差分migrationのみ適用**する。`hasLegacyUsers`(salt列欠落)判定は移植せず、代わりに:

```rust
fn current_schema_version(tx: &rusqlite::Transaction) -> Result<u32, DbError> {
    // system_settings 不在 = 真の新規DB = 0。
    // 既存本番は "17" を返す。Node時代の文字列を u32 へ。
    match tx.query_row(
        "SELECT value FROM system_settings WHERE key='schema_version'",
        [], |r| r.get::<_, String>(0)) {
        Ok(s) => s.parse().map_err(|_| DbError::Migration {
            version: 0, step: "parse_version", source: Box::new(DbError::CrossTenant("bad version")) }),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(0),
        Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::Unknown => Ok(0), // table不在
        Err(e) => Err(DbError::Sqlite(e)),
    }
}
```

**移行時の安全確認(fail-fast)**: 起動時、`system_settings` が存在するのに `schema_version` が読めない、または DB に `users` テーブルがあるのに version=0 と判定される「危険な組み合わせ」を検出したら **DROP せず即 `exit(1)`**(制約2: 設定不備の fail-fast)。これで棚卸し#2 §B の「本番全DROP」事故を構造的に排除する。

### mcp_servers FK欠落の非破壊移行

現行の `DROP TABLE mcp_servers` を、保持型 rebuild に置換:

```rust
fn m_fix_mcp_fk(tx: &rusqlite::Transaction) -> Result<(), DbError> {
    let fk_missing = tx.query_row(
        "SELECT COUNT(*) FROM pragma_foreign_key_list('mcp_servers')",
        [], |r| r.get::<_, i64>(0))? == 0;
    let exists = table_exists(tx, "mcp_servers")?;
    if exists && fk_missing {
        tx.execute_batch(r#"
            CREATE TABLE mcp_servers_new ( /* FK付き正しい定義 */ );
            INSERT INTO mcp_servers_new SELECT * FROM mcp_servers;  -- データ保持
            DROP TABLE mcp_servers;
            ALTER TABLE mcp_servers_new RENAME TO mcp_servers;
        "#)?;
    } else if !exists {
        tx.execute_batch("CREATE TABLE mcp_servers ( /* FK付き */ );")?;
    }
    Ok(())
}
```

登録済みMCPサーバーが**消えなくなる**(現行のデータロスを設計で解消)。

---

## 5. 型付き行(FromRow / serde)と wire DTO 分離

現行は Repo層で `.get()` 結果を手動マッピングし、JSON列を `JSON.parse` + try/catch(棚卸し#2 §4)。Rustでは:

```rust
#[derive(Debug, Clone)]
pub struct BotRow {
    pub id: BotId,
    pub user_id: UserId,            // owner
    pub name: String,
    pub discord_token_encrypted: Option<Vec<u8>>,  // 機密: DBrow型のみが持つ
    pub discord_token_iv: Option<Vec<u8>>,
    pub discord_token_tag: Option<Vec<u8>>,
    pub capabilities: JsonColumn<Vec<Capability>>,  // 後述のラッパで不正時を型化
    pub enabled_modules: JsonColumn<Option<Vec<ModuleId>>>,
    pub suspended: bool, pub stopped: bool,
}

impl BotRow {
    fn from_row(r: &rusqlite::Row) -> Result<Self, DbError> {
        Ok(Self {
            id: BotId(r.get("id")?),
            user_id: UserId(r.get("user_id")?),
            capabilities: JsonColumn::parse(r.get::<_, String>("capabilities")?, "capabilities")?,
            /* ... */
        })
    }
}
```

`rusqlite` に derive マクロはないので `serde_rusqlite` も選択肢だが、**`from_row` 明示手書きを推奨**(列名タイポをコンパイル時に近い形で検出しづらいが、機密列の取り扱いを1関数に集約でき、後述の wire DTO 分離と責務が噛み合う)。

### JSON列の型化(黙殺の排除)

```rust
pub enum JsonColumn<T> { Parsed(T), Malformed(String) }  // 生値を保持し「捨てない」
impl<T: DeserializeOwned> JsonColumn<T> {
    fn parse(s: String, col: &'static str) -> Result<Self, DbError> {
        match serde_json::from_str(&s) {
            Ok(v) => Ok(Self::Parsed(v)),
            Err(e) => { tracing::warn!(column=col, "malformed json, degrading"); Ok(Self::Malformed(s)) }
        }
    }
    pub fn or_default(self) -> T where T: Default {  // 呼び出し側が明示的にフォールバックを選ぶ
        match self { Self::Parsed(v) => v, Self::Malformed(_) => T::default() }
    }
}
```

現行の「不正JSON→空配列」は `.or_default()` の**明示呼び出し**になり、握り潰しが関数シグネチャに現れる。

### 機密strip を型で保証(領域#5 の二重管理解消)

現行 `apiViews.ts` の zod strip は**ランタイム**allowlist。Rust では **DB row型(`BotRow`)と wire DTO型(`BotView`)を別struct** にし、`*_encrypted` を DTO に持たせないことで**構造的に**漏洩不能にする(zod parse より強い)。

```rust
#[derive(Serialize, ts_rs::TS)]
#[ts(export)]
pub struct BotView {
    pub id: BotId, pub name: String,
    pub has_token: bool,        // discord_token_encrypted.is_some() から算出
    pub has_gemini_key: bool,
    pub capabilities: Vec<Capability>,
    // *_encrypted / *_iv / *_tag は存在しない = 漏れようがない
}
impl From<BotRow> for BotView {
    fn from(r: BotRow) -> Self {
        Self { id: r.id, name: r.name,
               has_token: r.discord_token_encrypted.is_some(),
               has_gemini_key: /* ... */, capabilities: r.capabilities.or_default() }
    }
}
```

`#[derive(ts_rs::TS)]` で `BotView` → TypeScript を build時生成(領域#5: frontend 101型の手写経を廃す)。**row型には `#[derive(TS)]` を絶対に付けない** ── これを clippy/grep lint で機械強制(`*Row` struct が `Serialize` derive を持たないことを CI チェック)。DTO/row 混同事故を型と lint の二重で防ぐ。

---

## 6. userId データ分離の不変条件化(v7の穴を型で塞ぐ)

現状は**規約ベース**(手書き `WHERE user_id = ?`)。棚卸し#2 §5 の通り、`bot_mcp_access` で実際にクロステナント権限昇格が起きた。型で不変条件化する。

### newtype で ID 混同を排除

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserId(pub String);   // Discord user id
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BotId(pub String);
// UserId と BotId は相互変換不能 = 引数順の取り違えをコンパイルエラーに
```

### Scoped リポジトリ: user_id 注入をクエリで強制

```rust
/// user スコープに束縛されたリポジトリ。全メソッドが user_id を自動注入する。
pub struct ScopedRepo<'a> { pool: &'a ReadPool, writer: &'a DbWriter, user: UserId }

impl<'a> ScopedRepo<'a> {
    pub async fn list_todos(&self, status: TodoStatus) -> Result<Vec<TodoRow>, DbError> {
        let uid = self.user.0.clone();
        self.pool.query(move |c| {
            let mut st = c.prepare_cached(
                "SELECT * FROM todos WHERE user_id = ?1 AND status = ?2 ORDER BY id")?;
            // user_id は ?1 に機械的に注入。ハンドラは user_id を "書き忘れられない"
            let rows = st.query_map(params![uid, status.as_str()], TodoRow::from_row)?;
            rows.collect::<Result<_,_>>().map_err(DbError::from)
        }).await
    }
}
```

ハンドラは `ctx.scoped()` から `ScopedRepo` を得るしかなく、生の `user_id: &str` を渡すAPIを**モジュール外に公開しない**(`pub(crate)` で封じる)。これで「WHERE user_id を書き忘れる」事故が起きない。

### bot_mcp_access のクロステナント: owner_id を型引数に

v7の修正(owner_id 次元追加)を**型で必須化**。共有秘書経由のMCP解決は「発話者(owner)スコープ」でしか呼べないシグネチャにする:

```rust
/// system_default 経由は必ず speaker(owner)スコープ。owner_id を省略する API は存在しない。
pub async fn list_servers_granted_scoped(
    &self, bot: &BotId, speaker: &UserId,   // ← speaker 必須。省略不能
) -> Result<Vec<McpServerRow>, DbError> {
    // WHERE owner_id = speaker OR user_id IS NULL(システムレベル) のみ
}
```

### AcrossUsers を別型で隔離

cron 系の全ユーザー横断(`listAllUserIds`, `list*AcrossUsers`)は `ScopedRepo` に置かず、**別struct `CrossUserRepo`** に隔離。命名規約 `AcrossUsers` を型境界に昇格し、「うっかり横断クエリを書く」ことを不可能にする:

```rust
/// 明示的に全ユーザー横断が必要な cron 専用。通常ハンドラからは import しない。
pub struct CrossUserRepo<'a> { pool: &'a ReadPool }
impl<'a> CrossUserRepo<'a> {
    pub async fn open_todos_due_within(&self, horizon: Duration) -> Result<Vec<TodoRow>, DbError> { /* ... */ }
    pub async fn all_user_ids(&self) -> Result<Vec<UserId>, DbError> { /* ... */ }
}
```

### FK非設定テーブルの孤児掃除(現行の弱点を改善)

現行は `message_logs`/`synapses`/`tool_outcomes` 等が FKなしでユーザー削除後に孤児化(棚卸し#2 §5、許容方針)。Rust では **`delete_user` トランザクション内で、FK非設定テーブルを user_id で明示DELETE** する掃除を追加(制約3: 堅牢性)。対象テーブルを定数配列で列挙し漏れを防ぐ:

```rust
const ORPHAN_TABLES_ON_USER_DELETE: &[&str] = &[
    "message_logs", "tool_outcomes", "synapses", "topic_tool_stats",
    "day_plan_blocks", "timeline_records", "audit_logs",
];
// delete_user tx 内で forget_synapse(RAM索引) も連動させる(synapse actor へ mpsc 通知)
```

---

## 7. トランザクション

rusqlite の `conn.transaction()` は現行 `db.transaction(fn)` と1:1。書き込み actor 内で完結させる:

```rust
writer.transact(|tx| {
    let sp = tx.savepoint()?;                    // ネストは savepoint で
    create_user_row(&sp, &user)?;
    seed_default_settings(&sp, &user.id)?;
    sp.commit()?;
    Ok(user.id)
}).await?;   // actor が commit/rollback を保証。panic時は Drop で自動 rollback
```

現行の `db.transaction` を使う7箇所(userRepo/personaRepo/plannedPaymentRepo/todoRepo/toolOutcomeRepo/googleAccountRepo/migrations)はすべてこの `writer.transact` へ移植。ネスト無し・短命なので移植容易(棚卸し#2 §A)。**エラー時 rollback は `Transaction::drop` が自動実行**するため、`let _ =` での明示ロールバック黙殺が不要になる。

---

## 8. Redis(fred)とフォールバック

### 判断: `fred` を推奨(`redis` crate ではなく)

- `fred` は**再接続 supervisor を内蔵**(指数バックオフ、`ReconnectPolicy`)。現行 `redis.ts:17-29` の「再接続を永遠に諦めない指数バックオフ上限30s」を設定1つで再現。制約2(自己復帰)に直結。
- `RedisPool` で複数接続、tokio native。
- `redis` crate + 自前再接続ループも可能だが、supervisorを自作する分だけ制約2の実装面積が増える。fred が既製で持つ。

```toml
fred = { version = "9", features = ["enable-rustls", "sentinel-auth"] }
```

### フォールバックを単一の型に集約(現行の「null分岐」を廃す)

現行は `getRedisClient()` が null を返し、**各利用側(session/rateLimit/oauthState/actionRecorder)がバラバラに null チェック**(棚卸し#2 §6)。Rust では劣化縮退を1つの enum に集約:

```rust
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("redis unavailable, served from memory")] Degraded,  // 縮退は"エラー"だが回復可能
    #[error("redis protocol error")] Protocol(#[from] fred::error::RedisError),
}

pub enum VolatileStore {
    Redis(fred::clients::RedisPool),
    Memory(Mutex<MemoryBackend>),   // in-memory Map フォールバック(session/rate/oauth/action共通)
}

impl VolatileStore {
    /// Redis到達不能時は Memory へ自動縮退。呼び出し側は成否を Result で受ける(黙殺しない)。
    pub async fn set_session(&self, key: SessionKey, uid: &UserId, ttl: Duration)
        -> Result<(), StoreError> { /* ... */ }
}
```

用途別のキー体系は現行を踏襲(棚卸し#2 §6):
- セッション `session:{sha256(token)}` + `user_sessions:{uid}` set、スライディングTTL。
- レート制限: 現行の `incr`→`expire` 非アトミック性(棚卸し#3 §5.4 の弱点)を **fred の Lua `EVALSHA` で原子化**(`local n=redis.call('INCR',k); if n==1 then redis.call('EXPIRE',k,ttl) end; return n`)。TTL無し永続キー残留を防ぐ。
- OAuth state / action history: memory フォールバック同型。

Redis 起動待ちは現行の「5秒だけ待って未接続でも起動続行」(`redis.ts:59-70`)を `tokio::time::timeout(Duration::from_secs(5), pool.wait_for_connect())` で再現。**未接続でも起動を止めない**(制約2: Redis一時障害は劣化縮退)。ただしシークレット/DBパス不備は fail-fast(§4 の起動時安全確認)。

---

## 9. rust_synapse 統合方針(rusqlite 共有)

rust_synapse は既に rusqlite で `synapses.embedding`(f32 LE BLOB)を **read-only 消費**(棚卸し#4)。書き込み(insert/embedding書き戻し)は Node が唯一の書き手。Rust移行後の境界:

- **書き手は「サーバ本体の書き込み actor」に一本化**。synapse エンジンは引き続き **read-only コネクションで `storage::load_index` する別プロセス/別タスク**。二重書き込み者を作らない(棚卸し#4 の read-only 境界を維持)。
- 統合形態は領域#5 の推奨に従い **`yuuka-engine` 単一バイナリ(crawler+synapse を別tokioタスク)を子プロセスとして spawn**、IPC は改行区切りJSON(FROZEN)。DB層としては「synapse は同じ SQLite ファイルを read-only で開く外部リーダー」と扱えばよく、WAL がサーバ本体の書き込みと synapse の読み取りを並行可能にする(WAL の読み書き非ブロッキング特性が効く)。
- **同一プロセス統合は非推奨**(領域#5 と一致): crawler のクラッシュが synapse の大RAM索引を巻き添えにする。DB層観点でも、synapse を別プロセスに保てば「サーバ本体 actor = 唯一の writer」不変条件がプロセス境界で自明に守られる。
- embedding 書き戻し(`updateSynapseEmbedding`)は **サーバ本体 actor が実行**し、synapse プロセスへは IPC で `index`(RAM索引更新)だけ依頼する現行フロー(Node insert → daemon index → Node update embedding)をそのまま踏襲。SQLite への書き込み経路が1本(actor)に収束する。

---

## 10. 拡張性(制約4)と型連携(制約5)の DB層フック

- **カスタムモジュールの永続化**: `MODULE_CATALOG` の `id`(リネーム禁止・永続キー)は Rust では `ModuleId(String)` newtype。プラグインが独自テーブルを足す場合、**プラグインごとに migration step を登録できる拡張点**を runner に設ける(`MIGRATIONS` にプラグイン提供の `Migration` を append。version 空間はコア `1..=999`、プラグイン `1000+` で予約)。冪等・非破壊の同じ規約を強制。
- **型連携の単一真実源**: §5 の row/DTO 分離 + `ts_rs` 生成を DB層で確立し、frontend の `types.ts` を生成物に置換。CI で `cargo run --bin gen-types` → `git diff --exit-code` で drift 検出(領域#5 の勝ち筋)。

---

## 主要参照ファイル(裏取り済み・絶対パス)

- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/db/database.ts`(接続・PRAGMA: WAL/FK-onのみ、busy_timeout未設定を確認)
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/db/migrations.ts`(`:882-894` レガシー全DROP、`:1465-1470` mcp_servers DROP、`:396-434` v5 を `db.transaction` で包む、FK-off は `:196/888/1113` の3rebuild箇所を確認)
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/db/redis.ts`(再接続戦略・5秒待ち起動続行・null返却フォールバック)
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/rust_synapse/src/storage.rs`(synapse の rusqlite read-only ロード)
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/types/apiViews.ts`(zod strip allowlist、row/DTO分離の移植元)

---

## [10] I have enough verified detail. The existing daemon supervision pattern (lazy res
<sub>agentId: ac77a404065ce3767</sub>

I have enough verified detail. The existing daemon supervision pattern (lazy restart on next request, `failAllPending`, EPIPE guard) is confirmed. I'll now write the design section.

# 領域C: 自己復帰・スーパーバイザ設計（常時稼働）

## C.0 設計の骨子と判断基準

現行 Node 実装の自己復帰哲学は既に明快で、これを Rust の型・タスク境界へ**より安全な形で**移植するのが本設計の目標である。裏取り済みの現行方針（`src/index.ts:179-202`）:

- `unhandledRejection` → **ログして継続**（`index.ts:183-188`）＝「回復可能な失敗は全体を巻き込まない」
- `uncaughtException` → **graceful shutdown して exit(1)**（`index.ts:192-202`）＝「状態破損は fail-fast し systemd に再起動させる」
- 起動時のシークレット不備 → **即 exit(1)**（`index.ts:61-86`）＝「致命的設定不備のみ起動前に落とす」
- Redis は初回5秒だけ待って未接続でも起動継続、再接続は永久に諦めない（`redis.ts:52-70, 17-28`）
- 停止は起動の逆順、15秒 watchdog で強制終了（`index.ts:138-165`）

この「回復可能 / 致命的」の二分を、Rust では **プロセス全体の panic ハンドラに依存せず、タスク境界で隔離した supervision tree** に落とし込む。JS の弱点（`uncaughtException` はプロセス全体の状態破損を前提に**プロセスごと**落とす）を、Rust では**アクター単位の再起動**に狭められるのが最大の勝ち筋である。

### 自己復帰 vs fail-fast の判断マトリクス（本設計の憲法）

| 事象 | 分類 | ポリシー | 型での表現 |
|---|---|---|---|
| シークレット未設定/32字未満 | **致命的（起動前）** | fail-fast, exit 78 (`EX_CONFIG`) | `StartupError::SecretMissing` を `main` が返す |
| DB マイグレーション失敗 | **致命的（起動前）** | fail-fast, exit 70 | `StartupError::Migration` |
| config.yaml パース不能 | **致命的（起動前）** | fail-fast, exit 78 | `StartupError::Config` |
| Web bind ポート使用中 | **致命的（起動前）** | fail-fast, exit 74 | `StartupError::Bind` |
| タスク内 panic | 回復可能 | 当該タスクのみ再 spawn（指数バックオフ） | `JoinError::is_panic()` を supervisor が捕捉 |
| Discord Gateway 切断 | 回復可能 | ライブラリ内蔵 resume＋タスク再起動 | `GatewayError`（後述） |
| Gemini 429/5xx | 回復可能（縮退） | circuit breaker→開なら即時ユーザーに縮退応答 | `LlmError::RateLimited`/`Upstream` |
| Redis 切断 | 回復可能（縮退） | インメモリ fallback、背景で再接続 | `CacheBackend::Memory` に切替 |
| synapse デーモン死亡 | 回復可能（縮退） | 直近履歴のみ（現行踏襲）、次要求で再起動 | `RecallOutcome::Degraded` |
| crawler デーモン死亡 | 回復可能（縮退） | ツールがエラー文字列を返す（LLM が別手段へ） | `ToolError::CrawlerUnavailable` |
| SQLite `SQLITE_BUSY` | 回復可能 | busy_timeout＋短リトライ | `DbError::Busy` |
| SQLite I/O error/corruption | **致命的** | fail-fast, exit 70（ディスク破損は状態破損） | `DbError::Corrupt` は `?` で main へ伝播 |

**絶対制約1（厳格エラー処理）との整合**: 上表の全分類は個別の `thiserror` 列挙バリアントに対応し、supervisor は `match` で網羅的に分岐する。`anyhow`/`Box<dyn Error>` は一切使わない。「握り潰し」に見える箇所（縮退）はすべて **名前のついた縮退バリアント**（`RecallOutcome::Degraded` 等）を返し、呼び出し側が縮退を認識・記録する。`let _ = ...` は禁止し、縮退は必ず型で表明する。

---

## C.1 supervision tree の構造

### C.1.1 全体構成

Tokio マルチスレッドランタイム（絶対制約3）上に、**ルート supervisor** が全長寿命サービスを**子タスク**として起動する。tree は2層で十分（過剰なネストは避ける）:

```
main (fail-fast ゾーン: config/secret/migration/bind)
└─ RootSupervisor  (Arc<AppState>, CancellationToken を保持)
   ├─ Supervised<WebServer>          restart: exponential, 致命的 bind は起動前に済
   ├─ Supervised<BotGateway>         per-token 子アクター群を内包
   │   ├─ BotActor(system_default)
   │   └─ BotActor(<custom botId>)…  ArcSwap で差し替え
   ├─ Supervised<SynapseClient>      子プロセス監督（daemon）
   ├─ Supervised<CrawlerClient>      子プロセス監督（daemon）
   ├─ Supervised<RedisCache>         再接続ループ、縮退は呼び出し側
   └─ CronScheduler                  9 本の cron ジョブを1タスクで tick
      ├─ reminder / playbook / clipboard / birthday
      ├─ paymentRecurrence / todoRecurrence
      └─ report / briefing / backup
```

### C.1.2 Supervised ラッパの型

各長寿命サービスは「**再起動可能な作業単位**」トレイトを実装し、汎用 supervisor が監督する。これが panic 隔離とバックオフ再起動の唯一の実装箇所になる（DRY）。

```rust
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use std::time::Duration;

/// 監督下タスクの分類。回復戦略を型で固定する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy {
    /// 常に再起動（Web/Bot/daemon）。指数バックオフ。
    Always,
    /// 一時停止のみで再起動しない（管理者が明示 stop した bot 等）
    Transient,
}

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("service '{name}' panicked in task")]
    Panicked { name: &'static str },
    #[error("service '{name}' exited with recoverable error: {source}")]
    Recoverable { name: &'static str, #[source] source: RecoverableCause },
    #[error("service '{name}' hit a fatal condition: {source}")]
    Fatal { name: &'static str, #[source] source: FatalCause },
}

/// 監督下で走る長寿命サービスの契約。
#[async_trait::async_trait]
pub trait Service: Send + 'static {
    fn name(&self) -> &'static str;
    fn restart_policy(&self) -> RestartPolicy;
    /// キャンセルトークンが発火するまで走り続ける。
    /// 正常終了(Ok) = 再起動しない意思表示。Err = 分類に応じ supervisor が判断。
    async fn run(&mut self, shutdown: CancellationToken) -> Result<(), RunError>;
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error(transparent)]
    Recoverable(#[from] RecoverableCause),
    /// これが返ると supervisor は再起動せず、致命扱いでルートへ伝播する。
    #[error(transparent)]
    Fatal(#[from] FatalCause),
}
```

supervisor 本体（**panic をタスク境界で捕捉する唯一の場所**）:

```rust
pub struct Supervisor {
    max_backoff: Duration,     // 30s（redis.ts:21 と同値）
    base_backoff: Duration,    // 500ms
    reset_after: Duration,     // 連続成功稼働がこれを超えたら backoff カウンタを 0 に
}

impl Supervisor {
    pub fn spawn<S: Service>(&self, mut svc: S, shutdown: CancellationToken)
        -> JoinHandle<()>
    {
        let name = svc.name();
        let policy = svc.restart_policy();
        let (base, max, reset_after) =
            (self.base_backoff, self.max_backoff, self.reset_after);

        tokio::spawn(async move {
            let mut attempt: u32 = 0;
            loop {
                if shutdown.is_cancelled() { return; }
                let started = tokio::time::Instant::now();

                // ★ panic 隔離: run() 内の panic はここで JoinError::is_panic() になる。
                //   Bot 層は panic="unwind"（Cargo.toml [profile]）でビルドする。
                let handle = tokio::spawn({
                    // svc を move するため一旦所有権を退避
                    let tok = shutdown.clone();
                    async move { let r = svc.run(tok).await; (svc, r) }
                });

                match handle.await {
                    Ok((returned_svc, Ok(()))) => {
                        // 正常終了 = 再起動不要（Transient 停止など）
                        svc = returned_svc;
                        if policy == RestartPolicy::Transient { return; }
                        // Always だが Ok で抜けた: shutdown 由来のはず
                        if shutdown.is_cancelled() { return; }
                        svc_log_restart(name, attempt, RestartReason::CleanExit);
                    }
                    Ok((returned_svc, Err(RunError::Fatal(cause)))) => {
                        // ★ 致命: 再起動しない。ルート supervisor へ通知して
                        //   プロセス全体の graceful shutdown → exit(1) を促す。
                        svc = returned_svc;
                        FATAL_TX.send(FatalCause::from((name, cause))).ok();
                        return;
                    }
                    Ok((returned_svc, Err(RunError::Recoverable(cause)))) => {
                        svc = returned_svc;
                        svc_log_restart(name, attempt, RestartReason::Recoverable(cause));
                    }
                    Err(join_err) if join_err.is_panic() => {
                        // ★ panic 隔離の核心。プロセスは生存、当該サービスのみ再起動。
                        //   panic した svc は失われるため、Service を Clone 可能 or
                        //   ファクトリ（下記 SpawnFn）で作り直す設計にする（後述）。
                        svc_log_restart(name, attempt, RestartReason::Panic);
                        // → ファクトリ版 spawn を使う（C.1.3）
                        return; // ファクトリ版に委譲
                    }
                    Err(join_err) => {
                        // cancelled（shutdown 経由）
                        if join_err.is_cancelled() { return; }
                        svc_log_restart(name, attempt, RestartReason::Panic);
                        return;
                    }
                }

                // 稼働が十分長ければ backoff をリセット（フラッピングとの区別）
                if started.elapsed() >= reset_after { attempt = 0; }
                let delay = backoff_delay(base, max, attempt);
                attempt = attempt.saturating_add(1);
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = shutdown.cancelled() => return,
                }
            }
        })
    }
}

fn backoff_delay(base: Duration, max: Duration, attempt: u32) -> Duration {
    // redis.ts:21 と同じ指数式: min(base * 2^min(attempt,6), max) + フルジッタ
    let exp = base.saturating_mul(1u32 << attempt.min(6));
    let capped = exp.min(max);
    // フルジッタ（同時再起動の thundering herd 回避、JS 版に無い改善）
    let jitter = fastrand::u64(0..=capped.as_millis() as u64);
    Duration::from_millis(jitter)
}
```

**選択の理由（ファクトリ vs 可搬 svc）**: panic すると `svc` の所有権が失われるため、`Service` を「値」で持つと再起動時に再構築できない。**推奨は「ファクトリクロージャ」方式**——`spawn` に `impl Fn() -> S` を渡し、panic/再起動のたびに新インスタンスを作る。これは現行の `restartDefaultBot` が「destroy 済み Client を再利用せず必ず新 `Client` を作る」（棚卸し#3 §2）方針と完全に一致する。以下を正式版とする:

### C.1.3 ファクトリ方式（正式版）

```rust
pub struct SpawnFn<S>(pub Arc<dyn Fn() -> S + Send + Sync>);

impl Supervisor {
    pub fn supervise<S: Service>(
        &self,
        factory: SpawnFn<S>,
        shutdown: CancellationToken,
        fatal_tx: tokio::sync::mpsc::UnboundedSender<FatalCause>,
    ) -> JoinHandle<()> {
        let cfg = self.clone();
        tokio::spawn(async move {
            let mut attempt: u32 = 0;
            loop {
                if shutdown.is_cancelled() { return; }
                let mut svc = (factory.0)();          // ★ 毎回新インスタンス
                let name = svc.name();
                let policy = svc.restart_policy();
                let tok = shutdown.clone();
                let started = tokio::time::Instant::now();

                let res = tokio::spawn(async move { svc.run(tok).await }).await;

                match res {
                    Ok(Ok(())) => {
                        if policy == RestartPolicy::Transient || shutdown.is_cancelled() {
                            return;
                        }
                    }
                    Ok(Err(RunError::Fatal(cause))) => {
                        let _ = fatal_tx.send(FatalCause::tagged(name, cause));
                        return;                        // 致命 → ルートへ
                    }
                    Ok(Err(RunError::Recoverable(cause))) => {
                        tracing::warn!(service = name, ?cause, attempt, "recoverable, restarting");
                    }
                    Err(je) if je.is_cancelled() => return,
                    Err(je) => {                        // ★ panic
                        debug_assert!(je.is_panic());
                        tracing::error!(service = name, attempt, "task panicked, isolating & restarting");
                    }
                }

                if started.elapsed() >= cfg.reset_after { attempt = 0; }
                let delay = backoff_delay(cfg.base_backoff, cfg.max_backoff, attempt);
                attempt = attempt.saturating_add(1);
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = shutdown.cancelled() => return,
                }
            }
        })
    }
}
```

**Cargo 設定（panic 隔離の前提）** — `panic = "abort"` だと1タスクの panic がプロセス全体を殺し supervision が無意味になる。必須:

```toml
# apps/yuuka/Cargo.toml
[profile.release]
panic = "unwind"   # ★ タスク境界での catch を可能にする（棚卸し#3 §5.3 の指摘と一致）
lto = "thin"
codegen-units = 1

[profile.dev]
panic = "unwind"
```

**clippy/deny による機械強制（絶対制約1）**:

```toml
# Cargo.toml [workspace.lints.clippy] または clippy.toml + CI
# 直接: unwrap/expect/panic/todo/unimplemented を deny
[workspace.lints.clippy]
unwrap_used = "deny"
expect_used = "deny"          # 起動パスのみ #[allow] を関数単位で明示許可
panic = "deny"
todo = "deny"
unimplemented = "deny"
# 握り潰し禁止
let_underscore_must_use = "deny"
```
`deny.toml`（cargo-deny）で `anyhow` / `eyre` / `color-eyre` を**依存グラフごと ban**:
```toml
[bans]
deny = [
  { name = "anyhow" }, { name = "eyre" }, { name = "color-eyre" },
]
```
起動パス（`main` と `StartupError` を返す関数のみ）で `expect` を許すため、それらの関数に `#[allow(clippy::expect_used)]` を関数単位で付け、レビューで可視化する。

---

## C.2 各サービスの回復戦略（具体）

### C.2.1 Discord Bot 層（BotGateway）

棚卸し#3 §5 の推奨に従い **twilight** を採用（複数トークンが `tokio::task` per shard で自然、差し替えが `ArcSwap` で表現できる）。BotGateway は内部に per-token アクターを持つ「複合サービス」。

- **各 BotActor = 1タスク + コマンド `mpsc`**。`Start` / `Stop` / `ReplaceToken(new)` をコマンドとして直列処理。これで現行の `startInFlight` Map（棚卸し#3 §2）の同時起動レースが**型レベルで消える**——1アクターは一度に1コマンドしか処理しないため。
- **トークン差し替え**（`restartDefaultBot` 相当）: 現行の「destroy→new Client→再代入（ライブバインディング）」を **`Arc<ArcSwap<BotHandle>>`** で置換。notifier 相当（後述）は `handle.load()` で常に最新シャードを掴む。旧タスクへ `CancellationToken` を発火 → 新トークンでシャード新規 spawn → `ArcSwap::store`。
- **Gateway 切断は fail-fast しない**: twilight のシャードは resume/identify を内蔵。`shard.next_event()` の `Err` は `GatewayError::Recoverable` にマップし、当該 BotActor のイベントループが continue するだけ（プロセスは無傷）。これは現行の「`Events.Error` が無いとクラッシュ」という JS の落とし穴（棚卸し#3 §2）が Rust では**構造的に存在しない**利点。
- **イベントハンドラの panic** は BotActor タスク境界で隔離され、supervisor が当該 bot のみ再 spawn。他 bot・Web・synapse は無傷。これが JS の `uncaughtException`（プロセス全体）より優れる点。

```rust
#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("gateway disconnected (will resume): {0}")]
    Recoverable(String),
    #[error("invalid bot token")]              // ← これは Fatal ではなく当該 bot の Transient 停止
    InvalidToken,
    #[error("intent not authorized: {0:?}")]
    IntentDenied(twilight_gateway::Intents),
}
```
**判断**: 無効トークンは**プロセスを落とさない**。当該 BotActor を `Transient` 停止（DB の `bots.suspended` 更新、管理 UI に警告）にする。トークンは runtime 差し替え可能なため fatal ではない。

### C.2.2 Web サーバ（axum）

- **bind は起動前 fail-fast**: `TcpListener::bind(addr)` を `main` で実行し `StartupError::Bind` として `?` 伝播。ポート使用中で無限再起動ループに陥らせない。
- bind 済み listener を `Service::run` に渡し、`axum::serve(...).with_graceful_shutdown(token.cancelled())` で走らせる。ハンドラ内 panic は tower の `CatchPanic` レイヤ＋タスク境界で500に落とし、接続単位で隔離（プロセス無傷）。これは現行 `serveStaticFile` の stream error で `res.destroy()` する多層防御（棚卸し#1 §5）の Rust 版。
- Web サービス自体の再起動は稀（ハンドラ隔離で足りる）だが、`accept` ループが致命エラー（fd 枯渇等）を返した場合のみ `Recoverable` で supervisor 再起動。

### C.2.3 synapse / crawler デーモン（子プロセス監督）

棚卸し#5 §5 の**推奨（2プロセスのまま、または統合バイナリを子プロセスとして）を採用**。理由: crawler は Chrome/reqwest で頻繁にクラッシュし得るが、synapse は大 RAM の索引常駐で長寿命——同居させると crawler クラッシュが synapse の RAM 索引を巻き添えにし `reindex`（DB 全読み）のコールドスタートを招く（§5 で明記の障害分離価値）。**プロセス分離を維持する。**

現行の「次要求時に lazy 再起動＋`failAllPending`＋EPIPE ガード」（`synapseEngine.ts` / `browserService.ts:178-197` で確認）を Rust では**専用 supervisor タスク**へ格上げする。lazy より積極的に、`exit` を検知したら**バックオフ後に即再 spawn**（次要求を待たない）——ただし crawler は短命バースト特性のため、synapse は eager 再起動 / crawler は現行踏襲の lazy 再起動と使い分ける。

```rust
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("daemon binary not found; degrading")]  // ← 縮退（バイナリ不在）
    BinaryMissing,
    #[error("daemon process exited: code={code:?}")]
    Exited { code: Option<i32> },
    #[error("request timed out after {0:?}")]
    Timeout(Duration),
    #[error("stdin write failed (EPIPE): {0}")]
    Pipe(#[source] std::io::Error),
    #[error("protocol decode error: {0}")]
    Decode(String),
}

/// 縮退を型で表明: 呼び出し側が「直近履歴のみ」への縮退を認識する。
pub enum RecallOutcome {
    Hit(RecallResult),
    Empty,
    Degraded(DaemonError),   // ★ let _ = でなく、必ずこの値を返す
}
```

**縮退の具体（絶対制約2）**:
- **synapse ダウン → `RecallOutcome::Degraded`**。`gemini` 相当の context builder は recall セクションを空文字にして「直近履歴のみ」で続行（現行 `buildRecallSection` が `""` を返す挙動、棚卸し#4 §1 と同一）。メトリクス `recall_degraded` を記録。**索引喪失は data loss ではなく性能劣化**——SQLite が正、再起動後 `reindex` で RAM 索引を再構築。
- **crawler ダウン → `ToolError::CrawlerUnavailable`**。ブラウザ系ツールハンドラが `{success:false, message}` 相当を返し、LLM が別手段（`searchWeb` の Puppeteer フォールバック相当）へ。**注意**: 現行の Node は Puppeteer フォールバックを持つ（棚卸し#5 §2.1）が、Rust 全面移行では Puppeteer が無いため、crawler 不在＝当該ツールが縮退エラーを返すのみ（LLM がユーザーに「Web 取得に失敗」と伝える）に変わる。これは劣化縮退として許容範囲（プロセスは落ちない）。

pending 相関の再起動時処理（現行 `failAllPending` 踏襲）:
```rust
// デーモン exit 検知時: 全 pending を Err(DaemonError::Exited) で解決し、
// 呼び出し側は Recall なら Degraded へ、index なら「行だけ SQLite に残し後で reindex」へ。
fn fail_all_pending(pending: &mut HashMap<u64, oneshot::Sender<Result<Value, DaemonError>>>,
                    cause: &DaemonError) {
    for (_, tx) in pending.drain() {
        let _ = tx.send(Err(cause.clone()));   // 受信側が drop していても無害（相手 drop のみ握る）
    }
}
```
（この `let _` は「送信先が既に drop」のみを握る唯一許容ケース。lint 例外を `#[allow]` で明示し、コメントで理由を残す。）

### C.2.4 Redis（キャッシュ縮退）

現行の二段構え（Redis 主 / インメモリ fallback、再接続は永久、初回5秒待ち）を型で表現。`getRedisClient()` が null を返し呼び出し側が分岐する現行パターン（`redis.ts:87-92`）を、**`CacheBackend` enum のディスパッチ**へ格上げする。

```rust
#[derive(Clone)]
pub enum CacheBackend {
    Redis(deadpool_redis::Pool),
    Memory(Arc<MemoryCache>),   // セッション/レート制限/oauth_state/actionRecorder の in-memory 実装
}

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("redis unavailable; served from memory fallback")]
    DegradedToMemory,
    #[error("redis command failed: {0}")]
    Command(#[from] deadpool_redis::redis::RedisError),
}
```

- **接続監督**: `deadpool-redis`（`redis` crate ベース）で pool を張り、背景タスクが `PING` で liveness を監視。切断時は `AtomicBool ready=false` にして `CacheBackend::Memory` へ論理切替、`ready` 復帰で Redis へ戻す。再接続は指数バックオフ（上限30s）で**永久**（`redis.ts:17-28` 準拠）。
- **縮退の具体**: セッション→`MemoryCache`（棚卸し#2 §6、再起動でセッション消失は許容）、レート制限→in-memory カウンタ（「再起動でリセットするが防衛線として十分」踏襲）、oauth_state / actionRecorder も同様。**セッションだけは重要**なので、Redis 復帰後も memory 側を残し読み合わせる「移行救済」（現行 `sessionService` の挙動）を維持。
- **`incr→expire` 非原子性の是正（棚卸し#3 §5.4 の既存弱点を移植機に解消）**: レート制限を `SET key val NX EX ttl` / Lua スクリプトで原子化。JS 版から引き継ぐ「count===1 判定と expire の間で落ちると TTL 無し永続キー」を Rust では原子コマンドで構造的に消す。
- **起動時**: `main` は Redis 接続を**待たない**（`initRedis` の5秒 race 相当は不要——最初から `CacheBackend::Memory` で起動し、背景タスクが接続でき次第 Redis へ昇格）。Redis 不達は**致命ではない**（fail-fast しない）。

### C.2.5 SQLite（DB）

棚卸し#2 §7A の推奨に従い **`rusqlite` + `tokio::task::spawn_blocking`**（sqlx でなく——動的 `ALTER`/`PRAGMA table_info` ベースの冪等移行と相性）。単一書き手の維持が重要。

- **書き込みは単一専用コネクション + アクター**（`mpsc` で全書き込みを直列化）。読み取りは read-only プールを別に。これで現行の「書き手は Node のみ」セマンティクスを保ち、`SQLITE_BUSY` を構造的に回避。
- **`busy_timeout` を明示設定**（現行 `database.ts` は未設定という弱点、棚卸し#2 §7A）: 接続時 `PRAGMA busy_timeout=5000; journal_mode=WAL; foreign_keys=ON`。
- **`DbError::Busy` は短リトライ、`DbError::Corrupt`/I/O は Fatal**（ディスク破損は状態破損＝fail-fast, exit 70）。

```rust
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database is busy (retryable)")]
    Busy,
    #[error("database is corrupt or disk I/O failed")]  // ★ Fatal
    Corrupt(#[source] rusqlite::Error),
    #[error("constraint violation: {0}")]
    Constraint(String),
    #[error("row not found")]
    NotFound,
}
```
マイグレーションは**単一専用コネクションで直列**（棚卸し#2 §7B の「PRAGMA はコネクション単位」注意と、破壊的 DROP を非破壊の連番マイグレーションへ置換する提言を採用）。マイグレーション失敗は `StartupError::Migration` で fail-fast。

---

## C.3 circuit breaker（外部依存ごと）

Gemini / Google / Discord(HTTP) の各上流に circuit breaker を挟む。目的は「上流障害時に無駄なリトライで自プロセスを詰まらせず、即座に縮退へ倒す」こと。ライブラリは薄いので**自前実装**（依存を増やさない／`anyhow` 系を混入させないため）。

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BreakerState { Closed, Open, HalfOpen }

pub struct CircuitBreaker {
    state: Arc<Mutex<BreakerInner>>,
    failure_threshold: u32,      // 連続失敗 N でオープン
    open_duration: Duration,     // オープン維持時間（この後 HalfOpen）
    half_open_max: u32,          // 試験通過数
}

#[derive(Debug, thiserror::Error)]
pub enum BreakerError<E> {
    #[error("circuit open for {service}; short-circuited")]
    Open { service: &'static str },
    #[error(transparent)]
    Inner(E),
}

impl CircuitBreaker {
    pub async fn call<T, E, F, Fut>(&self, service: &'static str, f: F)
        -> Result<T, BreakerError<E>>
    where F: FnOnce() -> Fut, Fut: Future<Output = Result<T, E>>,
    {
        match self.current() {
            BreakerState::Open => return Err(BreakerError::Open { service }),
            _ => {}
        }
        match f().await {
            Ok(v) => { self.record_success(); Ok(v) }
            Err(e) => { self.record_failure(); Err(BreakerError::Inner(e)) }
        }
    }
}
```

- **Gemini**: 棚卸し#4 §7A の通り自前 REST クライアント（`reqwest`+`serde`）。`generateWithRetry` の指数バックオフ（`RetryInfo.retryDelay` を error body から復元）は breaker の内側。breaker が Open のときは `LlmError::CircuitOpen` を即返し、**呼び出し側は「一時的に AI が混雑」旨のフォールバック文をユーザーへ**（会話ログには残す）。429 の連続はしきい値でオープン、`open_duration` 後に HalfOpen で1本試す。
- **Google（Calendar/Drive）**: breaker Open 時は「カレンダー同期を一時スキップ、ローカルキャッシュのみ表示」へ縮退（現行 `syncGoogleCalendarToLocal` はオンデマンドなので、失敗時ローカルの `schedules` を読むだけで自然に縮退、棚卸し#5 §1 補足）。backup は breaker Open ならその周期スキップ（次周期で再試行）。
- **Discord HTTP（`twilight_http`）**: Gateway とは別に REST 送信（notifier）用 breaker。Open 時は通知を**インメモリ再送キュー**に退避し、`open_duration` 後に flush（リマインド等の取りこぼし低減）。ただしキューは有界（例 1000 件）で溢れたら古いものを drop（`NotifyError::QueueFull` を記録）。

```rust
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("gemini circuit open; degraded reply served")]
    CircuitOpen,
    #[error("rate limited (retry after {retry_after:?})")]
    RateLimited { retry_after: Option<Duration> },
    #[error("upstream {status}: {body}")]
    Upstream { status: u16, body: String },
    #[error("request timed out")]
    Timeout,
    #[error("no api key configured for this scope")]   // ← 設定不備（縮退: UI に警告）
    NoApiKey,
}
```

---

## C.4 ヘルス / liveness / watchdog

### C.4.1 内部ヘルスレジストリ

各 supervised サービスが `Arc<ServiceHealth>` に状態を publish。ルートが集約。

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness { Up, Degraded, Down }

pub struct ServiceHealth {
    pub liveness: AtomicU8,          // Liveness を repr(u8) で
    pub last_ok: AtomicU64,          // epoch ms（最後に正常応答した時刻）
    pub restarts: AtomicU32,
}
```

- **liveness エンドポイント** `GET /healthz`（auth なし、Web サービスが提供）: プロセスが応答すれば 200。**プロセスが生きていること**のみを表す（Kubernetes/systemd の liveness probe 用）。
- **readiness エンドポイント** `GET /readyz`: DB 書き込みアクターが応答し、`system_default` bot が Ready かを見て 200/503。縮退（Redis down / synapse down）は **200 + `X-Degraded` ヘッダ**で表す（縮退は "not ready" ではない——ユーザーは使える）。
- **メトリクス** `GET /metrics`（管理者のみ）: 現行 `metrics.ts`（棚卸し#5 §1、5分間隔 console.log）を Prometheus テキスト形式へ。counter/latency（p50/p95）＋各サービスの `restarts` / `last_ok` / breaker 状態。

### C.4.2 watchdog（stuck 検出）

現行の「15秒シャットダウン watchdog」（`index.ts:138-144`）に加え、**稼働中の stuck 検出**を追加する（JS 版に無い改善）:

- 各 cron ジョブと DB 書き込みアクターは tick ごとに `last_ok` を更新。ルートの watchdog タスクが 60s ごとに走り、`last_ok` が閾値（例 cron は 90s、DB アクターは 30s）を超えて古いサービスを検出したら **`Liveness::Down` に落とし当該サービスへ `CancellationToken` を発火 → supervisor が再起動**。これで「デッドロック等で無言のまま固まる」ケースを回復対象にできる。
- watchdog 自身は最小依存・panic フリー（`saturating_sub` 等で算術も安全）。

---

## C.5 SIGTERM グレースフル停止順序

現行の「起動の逆順・15秒 watchdog・冪等（`shuttingDown` フラグ）」（`index.ts:133-177`）を Rust の構造化キャンセルで表現。

```rust
async fn run() -> Result<(), StartupError> {
    // ── fail-fast ゾーン（起動前の致命チェック） ──
    let config = Config::load()?;                 // StartupError::Config
    config.validate_secrets()?;                   // StartupError::SecretMissing / TooShort
    let db = Db::open_and_migrate(&config)?;       // StartupError::Migration / Bind 前に
    let listener = TcpListener::bind(config.addr).await
        .map_err(StartupError::Bind)?;             // ★ ポート使用中は起動前に落ちる

    // ── supervision ゾーン ──
    let root = CancellationToken::new();
    let (fatal_tx, mut fatal_rx) = mpsc::unbounded_channel::<FatalCause>();
    let sup = Supervisor::new();

    // 起動順（現行 index.ts 準拠: web → bot → cron群 → synapse → metrics）
    let handles = vec![
        sup.supervise(web_factory(listener_shared, db.clone()),  root.clone(), fatal_tx.clone()),
        sup.supervise(bot_factory(db.clone()),                   root.clone(), fatal_tx.clone()),
        sup.supervise(cron_factory(db.clone()),                  root.clone(), fatal_tx.clone()),
        sup.supervise(synapse_factory(&config),                  root.clone(), fatal_tx.clone()),
        sup.supervise(crawler_factory(&config),                  root.clone(), fatal_tx.clone()),
    ];

    // ── シグナル待ち ──
    tokio::select! {
        _ = shutdown_signal() => {                 // SIGINT/SIGTERM 両対応
            tracing::info!("signal received; graceful shutdown");
        }
        Some(cause) = fatal_rx.recv() => {         // ★ 致命が発生: 逆順停止して exit(1)
            tracing::error!(?cause, "fatal condition; shutting down to be restarted");
            graceful(root, handles, &db).await;
            return Err(StartupError::from(cause));  // main が exit(1) 相当を返す
        }
    }
    graceful(root, handles, &db).await;
    Ok(())
}

async fn graceful(root: CancellationToken, handles: Vec<JoinHandle<()>>, db: &Db) {
    root.cancel();  // 全 supervised タスクへ一斉伝播（各 run() が with_graceful_shutdown 等で受ける）
    // 15秒 watchdog（index.ts:138 と同値）。join を待つが溢れたら諦める。
    let joined = tokio::time::timeout(Duration::from_secs(15), join_all(handles)).await;
    if joined.is_err() {
        tracing::error!("graceful shutdown exceeded 15s; forcing exit");
    }
    // DB は最後（現行 closeDb→closeRedis の順、書き込み完了後に閉じる）
    db.shutdown().await;   // 書き込みアクターの mpsc を drain してから close
    // Redis / daemon 子プロセスは各 supervised タスク側で cancel 時に quit/kill 済み
}

async fn shutdown_signal() {
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM");
    let mut intr = signal(SignalKind::interrupt()).expect("install SIGINT");
    tokio::select! { _ = term.recv() => {}, _ = intr.recv() => {} }
}
```

**停止順序の要点**（現行逆順を維持）:
1. `root.cancel()` で **metrics → synapse/crawler → cron → bot → web** が並行にキャンセルを受ける。厳密な逆順が必要な依存は Bot（送信途中のターンを `deliver` まで待つ猶予）と DB（最後）のみ。
2. 各 daemon 子プロセスは `run()` のキャンセル時に **stdin close → 3秒待ち → SIGKILL**（現行 `browserService` の exit ハンドラ相当を停止側にも）。
3. **DB 書き込みアクターは最後**——mpsc を drain して未書き込みを吐き切ってから `close`。これが「停止順序」で最も重要（データ整合）。
4. 15秒 watchdog は現行同値。冪等性は `CancellationToken`（二重 cancel は無害）で自然に担保され、`shuttingDown` フラグ相当が不要になる。

**`shutdown_signal` の `expect`**: シグナルハンドラ設置失敗はプロセス起動不能に等しい起動パスなので `expect` を許容（`#[allow(clippy::expect_used)]` を関数に付与）。これは絶対制約1の「非起動パスでの unwrap/expect 禁止」に抵触しない（起動パスかつ設置失敗は fail-fast が正しい）。

---

## C.6 拡張性（絶対制約4）との接続

supervision tree はカスタムモジュールを**第一級で受け入れる**設計にする:

- **ユーザー拡張サービス** は `Service` トレイトを実装すれば `sup.supervise(user_factory, root, fatal_tx)` で**既存サービスと同格**に監督下へ入る。panic 隔離・バックオフ・ヘルス集約を無償で得る。拡張が panic してもコアは無傷（絶対制約2をプラグインにも適用）。
- **daemon パターンの再利用**: rust_crawler/rust_synapse の「改行区切り JSON デーモン」を汎用化した `DaemonSupervisor<Req, Res>`（`Req: Serialize`, `Res: DeserializeOwned`）を提供し、ユーザーが独自 Rust デーモンを同じ監督下に置ける。IPC 契約（`{id, command, ...}` → `{id, ok, result, error}`）は棚卸し#5 §5 推奨の `Value` 統一版を正とする。
- **MCP 動的ツール**（棚卸し#4 §6）は「実行時生成される `Arc<dyn Tool>`」で、これ自体はサービスではないが、MCP サーバへの HTTP 呼び出しに **§C.3 の circuit breaker をサーバ単位で適用**し、1つの不調 MCP が全体を詰まらせないようにする。

---

## C.7 現行からの改善点まとめ（この設計が加える価値）

1. **panic 隔離の粒度をプロセス→アクターへ**: JS の `uncaughtException`（プロセス全滅→systemd 再起動）を、Rust では当該サービスのみ再起動に狭める。1 bot のハンドラ panic が他 bot・Web・synapse を巻き込まない。
2. **stuck 検出 watchdog**（§C.4.2）: JS 版は「落ちる」障害しか回復できないが、「固まる」障害（デッドロック）も `last_ok` 監視で回復対象化。
3. **circuit breaker の明示化**（§C.3）: 現行は各所のリトライ＋タイムアウトに散在。Gemini/Google/Discord ごとに状態機械として集約し、Open 時の縮退挙動を型（`LlmError::CircuitOpen` 等）で強制。
4. **縮退を型で表明**（`RecallOutcome::Degraded` / `CacheBackend::Memory` / `ToolError::CrawlerUnavailable`）: 現行の「null を返して呼び出し側が分岐」を、握り潰し不能な enum バリアントへ。絶対制約1・2を同時に満たす。
5. **rate-limit / session の原子化・busy_timeout 明示**（§C.2.4/§C.2.5）: 棚卸しが指摘した既存の非原子性・`SQLITE_BUSY` リスクを移植機に構造的解消。
6. **フルジッタ付き指数バックオフ**（§C.1.3）: 複数 bot 同時再起動の thundering herd を回避（現行 `redis.ts` の式に jitter を追加）。

**参照した実ファイル**: `/home/suki/web/kawaii-music.moe/apps/yuuka/src/index.ts:61-210`（起動/停止/シグナル）、`.../src/db/redis.ts:17-92`（再接続・縮退・close）、`.../src/services/synapseEngine.ts:1-90`（daemon 監督型）、`.../src/services/browserService.ts:178-257`（exit/EPIPE/failAllPending 再起動パターン）。

---

## [11] I have enough to anchor the design. The two Rust crates are independent (no work
<sub>agentId: a43931c71d953a314</sub>

I have enough to anchor the design. The two Rust crates are independent (no workspace yet), the envelope has two documented exceptions (SetupStatus without `success`, OAuth device flow), and `BotView` is triple-defined. Let me write the self-contained design section.

---

# 領域: フロント⇄Rust 型連携（単一真実源）

## 0. 結論（先に推奨を確定させる）

| 論点 | 推奨 | 却下した代替 |
|---|---|---|
| 生成方式 | **ts-rs**（型のみ生成）+ **手書き薄い route bindings** | specta（tauri前提が濃い）、utoipa→OpenAPI→openapi-typescript（別領域「HTTPコア」の axum ルータと契約が二重管理になる） |
| 契約範囲 | **DTO型のみ自動生成。エンドポイント契約（path/method/scope）は生成しない** | フルOpenAPI生成（後述の理由で過剰） |
| 機密strip | **DB row 構造体と wire DTO 構造体を別型に分離。DTOは機密フィールドを持たない（`Serialize` を DTO のみに実装）** | serde skip 属性（付け忘れが漏洩に直結＝フェイルオープン） |
| エンベロープ | `ApiEnvelope<T>` を `#[serde(flatten)]` でトップレベル直置き。ts-rs 側は交差型 `{success,message} & T` を維持 | `{data: T}` ラッパ導入（現行フロント全書き換えになる。破壊的） |
| 配置 | `frontend/src/lib/api/generated/`（ドメイン別複数ファイル）+ バレル `index.ts` | 単一 `generated.ts`（655行の二の舞。差分レビュー困難） |
| ビルド統合 | **`cargo xtask gen-types`**（workspace 内の専用bin）→ Docker rust-builder ステージで実行 | build.rs（フロント成果物をビルド副作用で書くのは再現性・CI drift検査と相性が悪い） |
| CI drift検査 | `cargo xtask gen-types --check`（生成 → `git diff --exit-code` 相当を xtask 内で実装） | 別スクリプト |

理由の核心: **この移行では HTTP ルータ自体が Rust（axum 等）へ移る**（別領域「HTTPコア」）。utoipa は「axum ハンドラに `#[utoipa::path]` を付けて OpenAPI を吐く」方式だが、そうすると**ルート契約の真実源が2つ**（axum の `Router::route()` と utoipa 属性）になり、絶対制約5「単一真実源」に自ら反する。ルートの真実源は axum ルータ定義に一本化し、**型連携は型だけ**を ts-rs で運ぶのが最小で漏れない。specta は Tauri（`tauri-specta`）と組むときに価値が出るが、本件は Svelte+fetch であり specta の command 収集機構は不要。

---

## 1. なぜ ts-rs か（3方式の具体比較）

### 却下1: utoipa → OpenAPI → openapi-typescript
- 生成物が「型 + APIクライアント（paths）」まで含む。一見 §5.2「サービス層20本も消せる」に効くが、**axum の実ルータと `#[utoipa::path(get, path="/api/bots")]` 属性が二重管理**になる。パスtypo・method不一致・auth注記漏れをコンパイラは検出しない（utoipa 属性は文字列リテラル）。絶対制約5違反。
- OpenAPI の `oneOf`/`allOf`/nullable の TS 変換は openapi-typescript が `X | Y` に落とすが、現行の**エンベロープ交差型 `{success} & T`** を素直に再現できない（OpenAPI に交差型のネイティブ表現がなく `allOf` 経由になり、生成 TS が読みにくい）。
- 3ツールチェーン（utoipa + openapi生成 + openapi-typescript の npm 依存）で CI が脆い。

### 却下2: specta
- `specta::Type` derive + `specta-typescript` で ts-rs とほぼ同等の型生成は可能。だが specta の主用途は `tauri-specta` の command 型連携で、**関数シグネチャからの型収集**が売り。fetch ベースの本件では ts-rs との差が「enum の型表現がやや柔軟」程度で、追加依存に見合わない。
- specta 2.x は 2026-01 時点でも stable 前の API 変動があり、常時稼働の本番ビルドで固定したくない。

### 採用: ts-rs
- **型だけを愚直に生成**。`#[derive(TS)]` + `#[ts(export)]` で構造体/enum ごとに `.ts` を吐く。serde と独立した属性系（`#[ts(...)]`）を持ち、**serde の `flatten`/`rename`/`tag` を明示的に追従**できる（後述のエンベロープ表現で重要）。
- 生成が `cargo test`（ts-rs は export をテストとして走らせる設計）または専用 bin で完結し、**追加の外部CLI・npmツール不要**。Docker rust-builder に自然に載る。
- enum の TS 表現（`#[serde(tag="type")]` の discriminated union）を `#[ts]` 属性で正確に落とせる。OAuth device flow の `error` 判別に効く。

Cargo（新設する型クレート `yuuka-types`）:
```toml
[package]
name = "yuuka-types"
edition = "2021"

[dependencies]
serde = { version = "1", features = ["derive"] }
ts-rs = { version = "10", features = ["serde-compat", "no-serde-warnings"] }
serde_json = "1"

[features]
# 生成時のみ ts-rs の export を有効化（本番バイナリには型付けだけ残し export 副作用を含めない選択も可）
generate = []
```

---

## 2. DTO型のみか、エンドポイント契約まで含めるか

**型のみ生成を推奨。** ただし「型を持たない裸の fetch」に戻すのではなく、**手書きの薄い route bindings 層**（`frontend/src/lib/api/services/*.ts` を維持）が生成型を import して path/scope を指定する現構造をそのまま残す。

根拠:
1. path/method/auth/scope は **axum ルータの真実源**（別領域）に既にある。生成で二重化しない（制約5）。
2. `scope('bot'|'user')` は §2.1 の通り**クライアント規約であってサーバ実装ではない**。既存 `client.ts` の「scope 必須・opt-out を型で強制」という優れた設計判断を壊さない。生成対象に含めると、この判断がサーバ属性へ漏れ出す。
3. 現行フロントの `financeApi` 等が持つ「リクエストは camelCase / レスポンスは snake_case」の不整合は、**リクエストDTOも Rust で定義して生成**すれば型に載る（後述 §7）。エンドポイント契約全体を生成しなくても、req/res 両DTOの生成だけでこの実害は消える。

つまり生成物 = **レスポンスDTO型 + リクエストDTO型 + 共有enum**。paths とクライアントメソッドは手書き薄層で維持。

---

## 3. 機密列を型レベルで非シリアライズ化する（現 zod allowlist の置換）

### 3.1 設計原則: 「DB row 型」と「wire DTO 型」を別 struct にし、`Serialize` を DTO にしか実装しない

現状 zod は「parse時に未知キーを strip」する**ランタイム**フェイルクローズ。Rust ではこれを**コンパイル時**に強化できる。核心は「機密を持つ型は `Serialize` を実装しない」——実装がなければ、その型を axum の `Json(...)` に渡すコードは**コンパイルエラー**になる。フェイルオープン（付け忘れ漏洩）が構造的に不可能。

```rust
// ── DB row: 機密列を含む内部レコード。Serialize を"実装しない"（deny by construction）──
// rusqlite の FromRow 相当のみ derive。Serialize は絶対に derive しない。
#[derive(Debug, Clone)]
pub struct BotRecord {
    pub id: String,
    pub user_id: String,
    pub name: String,
    // ↓ 機密列。この型に Serialize が無いので、そもそも JSON へ出せない。
    pub discord_token_encrypted: Option<Vec<u8>>,
    pub discord_token_iv: Option<Vec<u8>>,
    pub discord_token_tag: Option<Vec<u8>>,
    pub gemini_api_key_encrypted: Option<Vec<u8>>,
    pub gemini_api_key_iv: Option<Vec<u8>>,
    pub gemini_api_key_tag: Option<Vec<u8>>,
    pub persona_id: Option<i64>,
    // ...内部管理列も全部ここ
}

// ── wire DTO: 応答へ出してよいフィールドだけ。ここにだけ Serialize + TS を付ける ──
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "generated/bot.ts")]
#[serde(deny_unknown_fields)]          // 逆方向（Deserialize時）にも締める
pub struct BotView {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub recommended_persona_id: Option<i64>,
    pub persona_id: Option<i64>,
    pub capabilities: String,
    pub discord_username: Option<String>,
    pub discord_avatar_url: Option<String>,
    pub discord_application_id: Option<String>,   // 公開ID: 出してよい（現 zod と同判断）
    pub suspended: i64,
    pub created_at: String,
    pub updated_at: String,
    pub preset: String,
    pub preset_display_name: String,
    // 派生bool（zod の has_gemini_key/has_token 相当）は From で計算する
    pub has_gemini_key: bool,
    pub has_token: bool,
    pub running: bool,
    pub connected: bool,
    pub shared: bool,
}
```

### 3.2 変換は `From` ではなく専用関数（`unwrap`/panic 禁止・型付きエラー）

`From<BotRecord> for BotView` は inflallible な変換にしか使えない。running/connected/shared は**ランタイム状態（Bot supervisor 問い合わせ）**を要するため純粋変換ではない。従って `From` ではなく、失敗を型で返す明示関数にする（絶対制約1）:

```rust
#[derive(Debug, thiserror::Error)]
pub enum BotViewError {
    #[error("bot preset is unknown: {0}")]
    UnknownPreset(String),
}

pub fn bot_view(rec: &BotRecord, rt: &BotRuntimeState) -> Result<BotView, BotViewError> {
    let preset = resolve_preset(&rec.capabilities).ok_or_else(|| {
        BotViewError::UnknownPreset(rec.capabilities.clone())
    })?;
    Ok(BotView {
        id: rec.id.clone(),
        user_id: rec.user_id.clone(),
        name: rec.name.clone(),
        recommended_persona_id: rec.recommended_persona_id,
        persona_id: rec.persona_id,
        capabilities: rec.capabilities.clone(),
        discord_username: rec.discord_username.clone(),
        discord_avatar_url: rec.discord_avatar_url.clone(),
        discord_application_id: rec.discord_application_id.clone(),
        suspended: rec.suspended,
        created_at: rec.created_at.clone(),
        updated_at: rec.updated_at.clone(),
        preset: preset.key().to_string(),
        preset_display_name: preset.display_name().to_string(),
        // 機密の"有無だけ"を bool へ。値は絶対に載せない。
        has_gemini_key: rec.gemini_api_key_encrypted.is_some(),
        has_token: rec.discord_token_encrypted.is_some(),
        running: rt.running,
        connected: rt.connected,
        shared: rt.shared,
    })
}
```

### 3.3 「機密型を serialize させない」を機械的に強制する多層防御

型分離だけでも強いが、`clippy`/`cargo-deny`/コンパイラで二重に締める（絶対制約1の「機械的強制」）:

1. **BotRecord に `Serialize` を実装しない**（最重要・これだけで axum `Json` へ渡せない）。
2. **暗号文フィールドの newtype 化**で二重ロック:
   ```rust
   /// 暗号文バイト列。Serialize を"意図的に実装しない"封筒型。
   /// これを含む型は連鎖的に Serialize 不可になる（漏洩の型レベル遮断）。
   #[derive(Debug, Clone)]
   pub struct Ciphertext(Vec<u8>);
   // impl Serialize for Ciphertext {}  ← 存在しない。誰かが後で足そうとしたら…
   ```
   `Ciphertext` に対する `Serialize` 実装の追加を **`cargo-deny` ではなく専用の `#[deny]` lint** で禁止できないため、代わりに **CI で `grep -rn "impl Serialize for Ciphertext" ; test $? -ne 0` 相当のガード**を xtask に入れる（後述 §6.4 の一部）。
3. **`#[serde(deny_unknown_fields)]`** を全 DTO に付与（Deserialize 経路でのフィールド混入も遮断。req DTO に効く）。
4. ts-rs 生成時、DTO に無いフィールドは TS にも出ない——**フロントが機密フィールド名を型として知る術がなくなる**（現状 types.ts のコメントに「stripしている」と書いてある情報漏洩すら消える）。

これで現 zod の「将来追加列も自動で漏らさない」フェイルクローズ性を、**ランタイムparse strip → コンパイル時の Serialize 不在**へ格上げできる（zod より強い保証。§5.4 の懸念に対する回答）。

---

## 4. `ApiResponse<T>` エンベロープ（data ラッパ無し）の表現

現行 `type ApiResponse<T> = { success: boolean; message?: string } & T;`（トップレベル直置き、交差型）を**厳守**する。`{data: T}` ラッパ導入はフロント全面書き換え＝破壊的なので却下。

### 4.1 Rust 側: `#[serde(flatten)]` で直置きを再現

```rust
#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "generated/envelope.ts")]
pub struct ApiEnvelope<T: Serialize + TS> {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(flatten)]        // ← payload を data ラッパ無しで同階層へ展開
    pub data: T,
}
```

`serde(flatten)` はランタイムで `{"success":true,"bots":[...]}` を生成する。ts-rs は `serde-compat` フィーチャ下で flatten を**交差型 `{ success: boolean; message?: string } & T`** に落とす。生成される `envelope.ts`:
```ts
export type ApiEnvelope<T> = { success: boolean; message?: string } & T;
```
これは現行 `types.ts:13` と**字面まで一致**。既存の `BotListResponse = ApiResponse<{bots: BotView[]}>` 群は、payload struct を定義して:
```rust
#[derive(Serialize, TS)]
#[ts(export, export_to = "generated/bot.ts")]
pub struct BotListPayload { pub bots: Vec<BotView> }
// 応答は axum で Json(ApiEnvelope { success:true, message:None, data: BotListPayload{bots} })
```
生成 TS: `export type BotListResponse = ApiEnvelope<BotListPayload>;`（薄層で alias）。

### 4.2 エンベロープ2例外を型で表現

実ファイルで確認した2つの例外を、生成型として明示的に分ける:

**例外A: `SetupStatusResponse`（success を持たない裸オブジェクト、types.ts:32）**
```rust
#[derive(Serialize, TS)]
#[ts(export, export_to = "generated/setup.ts")]
pub struct SetupStatusResponse {   // ApiEnvelope で包まない。裸で Json(...) する
    pub need_setup: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terms_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub privacy_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_url: Option<String>,
}
```
（現行フロントは `needSetup` camelCase。serde の `#[serde(rename_all="camelCase")]` を struct に付けて `need_setup`→`needSetup` を出力し、ts-rs も追従させる。）

**例外B: OAuth device flow（`device.ts` の専用実装）**
device flow は「200 + error」を正常系（pending/slow_down）として扱うため `ApiEnvelope` に載せてはならない。discriminated union で表現し、フロントの `device.ts` 専用ハンドラ（success判定を通さない）は**そのまま残す**（§5.2）:
```rust
#[derive(Serialize, TS)]
#[ts(export, export_to = "generated/device.ts")]
#[serde(rename_all = "snake_case")]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u32,
    pub expires_in: u32,
}
// トークン交換は「200+access_token」「200+error」「4xx+error」の3態。
// サーバは access_token or error の"どちらか"を持つ素の JSON を返す（RFC8628準拠）。
#[derive(Serialize, TS)]
#[ts(export, export_to = "generated/device.ts")]
#[serde(rename_all = "snake_case")]
pub struct DeviceTokenRaw {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_type: Option<String>,     // "Bearer"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,          // authorization_pending | slow_down | expired_token ...
}
```
フロントの `PollResult`（`{status:'authorized'|'pending'|'error'}`）は**クライアント内部の解釈型**なので生成しない。`device.ts:36-51` の分岐は `DeviceTokenRaw` を消費するだけに調整。

---

## 5. 生成物の配置・ビルド統合・CI drift検査

### 5.1 配置: ドメイン別複数ファイル + バレル

655行の単一 `types.ts` を1ファイル生成にすると差分レビューが地獄なので、ts-rs の `export_to` でドメイン分割:
```
frontend/src/lib/api/
├── generated/            ← 生成物。人間は編集禁止（先頭に自動生成ヘッダ）
│   ├── envelope.ts
│   ├── bot.ts
│   ├── task.ts
│   ├── finance.ts
│   ├── ...（ドメインごと）
│   └── index.ts          ← ts-rs が全 export を再輸出するバレル
├── types.ts              ← 移行中の手書き残（§8で段階的に空へ）
├── client.ts             ← 不変（scope規約はここ）
└── services/*.ts         ← 手書き薄層。generated から import
```
各生成ファイル先頭に `// @generated by yuuka-types via ts-rs — DO NOT EDIT` を出力（ts-rs の `--header` 相当を xtask で前置）。

### 5.2 生成トリガ: `cargo xtask gen-types`（build.rs ではない）

**推奨は xtask パターン**（`.cargo/config.toml` に `[alias] xtask = "run --package xtask --"`）。build.rs を却下する理由: build.rs でフロント成果物を書くと (a) `cargo build` の副作用でワークツリーが汚れ CI の drift 検査（git diff）と競合、(b) インクリメンタルビルドで走らない場合があり再現性が落ちる。xtask なら「型生成」という意図が明示的なコマンドになる。

新設 workspace（現在 rust_crawler / rust_synapse は独立クレート＝**まだ workspace 化されていない**ことを実確認済み。サーバ Rust 化を機に統合する。別領域「services」の推奨と一致）:
```toml
# apps/yuuka/Cargo.toml（新設ワークスペースルート）
[workspace]
resolver = "2"
members = [
    "crates/yuuka-types",     # DTO + ts-rs
    "crates/yuuka-server",    # axum
    "crates/yuuka-bot",       # twilight
    "crates/yuuka-engine",    # crawler+synapse 統合（別領域推奨）
    "xtask",
]
```
```rust
// xtask/src/main.rs — gen-types サブコマンド（抜粋。unwrap/expect/panic 禁止）
use std::process::ExitCode;

#[derive(Debug, thiserror::Error)]
enum XtaskError {
    #[error("unknown subcommand: {0}")]
    UnknownCommand(String),
    #[error("no subcommand given")]
    NoCommand,
    #[error("type export failed: {0}")]
    Export(#[from] yuuka_types::ExportError),
    #[error("generated files are stale; run `cargo xtask gen-types` and commit")]
    Drift,
    #[error("io error on {path}: {source}")]
    Io { path: String, source: std::io::Error },
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");   // 型付きエラーを名前ごと表示（黙殺しない）
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), XtaskError> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("gen-types") => {
            let check = args.next().as_deref() == Some("--check");
            yuuka_types::export_all()?;      // ts-rs の export をまとめて呼ぶ
            if check {
                verify_no_drift()?;          // §5.4
            }
            Ok(())
        }
        Some(other) => Err(XtaskError::UnknownCommand(other.to_string())),
        None => Err(XtaskError::NoCommand),
    }
}
```
`yuuka_types::export_all()` は ts-rs の各型の `<T as TS>::export()` を明示的に列挙して呼ぶ（`?` で `ExportError` 伝播。ts-rs の `export` は `Result` を返すのでこれが自然）。ts-rs の「テストとして export」慣習ではなく**明示 bin**にするのは、生成タイミングを CI で制御し `.unwrap()` を排除するため。

### 5.3 Docker rust-builder への組み込み

現 Dockerfile の rust-builder ステージ（crawler/synapse を cargo build）に **codegen を tsgo/vite の前**に挿入。フロントの `vite build` は生成物を消費するだけ:
```dockerfile
# rust-builder ステージ内、cargo build --release の後
RUN cargo xtask gen-types                                  # generated/*.ts を書き出す
# → node-builder ステージが frontend/src/lib/api/generated/ ごと COPY して vite build
```
ビルド順序の load-bearing 性（別領域「frontend-deploy」で確認された `tsgo → vite build → prune` 厳守）に、**gen-types を先頭**に足すだけ。生成物はリポジトリにコミット済み（§5.4）なので、Docker 内生成は「コミット済みと一致するはず」の再確認も兼ねる。

### 5.4 CI drift 検査（現「コメント頼み」を機械保証へ置換する最大の勝ち筋）

生成物を**コミットする**方針（フロント開発者が Rust ビルドなしで型を読めるため）。CI で乖離をブロック:
```rust
// xtask verify_no_drift（git 非依存で実装。git diff に頼らず内容比較）
fn verify_no_drift() -> Result<(), XtaskError> {
    // export_all() が書いた各ファイルを、生成前のスナップショットと比較する。
    // 実装: export を一時ディレクトリへ出し、コミット済みファイルと byte 比較。
    // 不一致があれば Drift を返す。
    if snapshot_differs()? {          // 具体比較関数（io エラーは Io で型付き伝播）
        return Err(XtaskError::Drift);
    }
    Ok(())
}
```
CI ジョブ（GitHub Actions 等）:
```yaml
- run: cargo xtask gen-types --check   # 乖離があれば非ゼロ終了でCI落ちる
```
これで「Rust DTO を変えたのに generated.ts を再生成し忘れてコミット」を**マージ前に検出**。現行の「types.ts 冒頭コメントで参照元を手管理」という機械的保証ゼロの状態を、drift 検査で完全置換する。追加で §3.3-2 の `Ciphertext` への `Serialize` 実装混入ガードも同 xtask に同居させる:
```rust
// 機密封筒型に Serialize を後付けする回帰を CI で禁止（漏洩の最終防波堤）
fn assert_no_ciphertext_serialize() -> Result<(), XtaskError> {
    // crates/ 配下を走査し "impl Serialize for Ciphertext" 系の出現を探す。
    // 見つかれば専用エラーで落とす（黙殺しない）。
}
```

---

## 6. 移行中に手書き `types.ts` とどう両立させるか

段階移行（big-bang 置換は 101 型同時で危険）。**ドメイン単位で「手書き削除 → 生成へ差し替え」**を進める:

1. **足場**: `generated/` を作り、まず `envelope.ts`（`ApiEnvelope<T>`）と `bot.ts`（`BotView`——現状 zod/手書き/DB row の三重定義。ここが最も効く）を生成。
2. **バレルで吸収**: `types.ts` の該当 `interface BotView` と `BotListResponse` を**削除**し、代わりに `types.ts` 冒頭で `export * from "./generated";` を置く。既存 `services/botApi.ts` の import（`from "./types"`）は無改変で解決される（バレル再輸出のため）。
3. **1ドメインずつ削る**: task → finance → schedule …の順で、Rust に req/res DTO を定義 → gen-types → `types.ts` から対応 interface を削除。**削除するたび `tsgo`（型チェック）が通ることを確認**（型不一致は即コンパイルエラーで露見。現 types.ts の snake/camel 不整合コメントが実害だったなら、ここで表面化して直せる）。
4. **キー名乖離の解消**: §1.2 で列挙された危険信号（`timelineApi` の `blocks` vs `plan`、`webhooks` の `endpoints`、`financeApi` の camel/snake 混在）は、payload struct のフィールド名を**サーバ実装（axum ハンドラが実際に返す JSON）に一致**させて定義する。ts-rs 生成型が実レスポンスと構造的に一致するので、以後コメントでなく型で守られる。
5. **完了条件**: `types.ts` が `export * from "./generated";` と、生成対象外の「クライアント内部解釈型」（`PollResult`・`Scope` 等）だけになる。この時点で手書き DTO はゼロ。
6. **並存中の安全**: 生成型と手書き型が同名で衝突すると TS がエラーにする（silent drift しない）。バレル方式なら「同名 export の二重」がコンパイルエラーになるので、移行漏れが型で検出される——**移行そのものがフェイルクローズ**。

---

## 7. リクエストDTOも生成対象に含める（req/res 語彙不整合の根治）

§5.4 の残リスク「リクエストは `limitAmount`/`plannedDate` (camel)、レスポンスは `memo` (snake)」は、**req DTO も ts-rs 生成**すれば型に載る:
```rust
#[derive(Deserialize, TS)]
#[ts(export, export_to = "generated/finance.ts")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]  // フロントは camel で送る
pub struct AddExpenseRequest {
    pub amount: i64,
    pub category: String,
    pub description: Option<String>,   // ← レスポンス側 memo とは別語彙。型で明示
    pub date: String,
}
```
`deny_unknown_fields` により、フロントが古い/typo キーを送れば axum の Deserialize が**構造的に 400** を返す（現行の「サーバは body 優先で botId 両対応」の後方互換は、`botId: Option<String>` を明示フィールドに持たせて維持）。生成 `AddExpenseRequest` 型をフロントの `financeApi.addExpense` 引数に使えば、JSDoc コメントで手当てしていた不整合が**呼び出し側のコンパイル時**に固定される。

---

## 8. 絶対制約との整合（自己チェック）

- **制約1（厳格エラー）**: xtask/変換関数すべて `thiserror` 列挙型 + `?` 伝播。`unwrap/expect/panic` は非起動パスで不使用（`ExitCode` で main を締める）。ts-rs の `export()` の `Result` を握り潰さず `#[from]` で伝播。
- **制約2（自己復帰）**: 型生成はビルド時処理でランタイム稼働に無関係。生成失敗は**ビルド fail-fast**（起動時の致命設定不備と同カテゴリ）で正しい。
- **制約3（高速・堅牢）**: ランタイムに zod の per-response parse が消える（DTO は静的型でシリアライズのみ）。strip のランタイムコストゼロ化＝高速化。
- **制約4（拡張性）**: `yuuka-types` は独立クレート。user のカスタムモジュールが独自 DTO を足すとき `#[derive(Serialize, TS)]` + `export_to` を付けるだけで生成パイプラインに乗る。MCP 動的ツールの引数スキーマは実行時 JSON Schema なので生成対象外（現状通り `jsonSchemaToGeminiSchema` 相当で扱う）——**静的DTOと動的スキーマの境界を保つ**。
- **制約5（単一真実源）**: Rust DTO 1定義 → 生成 TS 1定義。`BotView` の三重定義が解消。drift 検査で機械保証。ルート契約は axum に一本化し、型連携は型のみ運ぶことで「真実源が2つになる」utoipa 方式を回避。

---

## 参照（裏取り済みファイル、絶対パス）
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/types/apiViews.ts`（zod ビュー3個・strip フェイルクローズ・`has_gemini_key`/`has_token` 派生 bool を実確認）
- `/home/suki/web/kawaii-music.moe/apps/yuuka/frontend/src/lib/api/types.ts:13`（`ApiResponse<T> = {success,message} & T` 交差型）, `:32`（SetupStatus 例外）, `:51-71`（BotView 手書き＝三重定義の一角）
- `/home/suki/web/kawaii-music.moe/apps/yuuka/frontend/src/lib/api/client.ts`（scope 必須・opt-out・201 success:false 複合判定・同一 same-origin。**生成対象外のクライアント規約**であることを確認）
- `/home/suki/web/kawaii-music.moe/apps/yuuka/frontend/src/lib/api/device.ts`（OAuth device flow の専用実装＝エンベロープ例外B）
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/rust_synapse/Cargo.toml`, `src/rust_crawler/`（**両クレートが独立・未workspace化**を実確認。統合の前提）
- `frontend/src/lib/api/services/*.ts`（20本の手書き薄層＝移行中に維持する層）

---

## [12] 裏取りできた。`FunctionModule`/`registry.dispatch` の「文字列戻り＋`success:false`規約」「重複throw」「
<sub>agentId: ab94829708e3098ec</sub>

裏取りできた。`FunctionModule`/`registry.dispatch` の「文字列戻り＋`success:false`規約」「重複throw」「未知関数/例外を`{success:false}`へ正規化」を確認。以下、設計セクションとして自己完結で提示する。

---

# 領域C設計: Discord + Gemini/Function Calling の Rust 実装

## C-0. 全体方針と依存クレート確定

本領域は「常時稼働する Discord ゲートウェイ群」と「Gemini REST クライアント + ツールレジストリ」の2本柱。両者を **単一 `tokio` ランタイム内のアクター群**として構成し、各アクターを supervisor 配下に置く。既存 Node 実装の「回復可能=継続 / 状態破損=再起動」という二分（`index.ts` の `unhandledRejection` vs `uncaughtException`）を、**プロセス全体ではなくアクター単位**へ狭めるのが Rust 移植の最大の勝ち。

確定クレート（cargo workspace のルート `[workspace.dependencies]` に集約）:

```toml
# Discord
twilight-gateway   = "0.16"   # ← serenity ではなく twilight を推奨（C-1で論証）
twilight-http      = "0.16"
twilight-model     = "0.16"
# Gemini（公式Rust SDK不在 → 自前）
reqwest            = { version = "0.12", default-features = false, features = ["json", "gzip", "brotli", "rustls-tls", "stream"] }
serde              = { version = "1", features = ["derive"] }
serde_json         = "1"
# 非同期基盤・監督
tokio             = { version = "1", features = ["full"] }
tokio-util        = { version = "0.7", features = ["rt"] }   # CancellationToken
async-trait        = "0.1"
arc-swap           = "1.7"    # トークンローテーションの参照差し替え（C-2）
futures            = "0.3"
# エラー（anyhow/eyre は全面禁止 → thiserror のみ）
thiserror          = "2"
# リトライ/サーキットブレーカ（自前実装、下地のみ）
governor           = "0.7"    # レートリミッタ（既存 botRateLimit 相当の一部）
# 型生成（領域F連携）
ts-rs              = "10"
```

**明示的に不採用**: `anyhow` / `eyre` / `color-eyre` / `backoff`(内部で `Box<dyn Error>` を返す版がある) / `snafu`(context チェインが握り潰しに寄る)。リトライは自前の型付き実装にする（C-4）。

`cargo-deny` で禁止クレートを機械強制する（C-8）。

---

## C-1. Discord: serenity vs twilight — **twilight を推奨**

### 判定表（担当4観点）

| 観点 | serenity | **twilight（推奨）** |
|---|---|---|
| 複数ボット同時接続（`customClients` 等価） | `Client` が 1 トークン=1 ランタイム前提。N 個の `Client` を別タスクで抱える＝現状 `Map` に 1:1 だが各 `Client` が gateway+cache+http を丸ごと重複保持 | `Shard` は 1 トークン=1 接続の**軽量プリミティブ**。`twilight_gateway::Shard` を `tokio::task` per トークンで回し、全 event を単一 `mpsc` にファンインできる。HTTP は `Arc<twilight_http::Client>` をトークンごとに1個。**customClients に構造的に一致** |
| ボタンインタラクション | フレームワーク層が厚く、生 `Interaction` に触るのに `serenity::framework` を外す必要 | `Interaction` は `twilight_model::application::interaction::Interaction` の**素の enum**。`InteractionData::MessageComponent` を直接 match。既存 `customId.split(":")` 規約をそのまま移植可 |
| トークンローテーション（`restartDefaultBot`） | `Client` 再利用不可 → 作り直し（Node と同型の弱点を引き継ぐ） | Shard は単なるタスク。旧タスクへ `CancellationToken` → 新トークンで再 spawn → `ArcSwap` store。**「destroy済みClient再利用不可」問題自体が消える**（C-2） |
| 再接続耐性 | 内蔵（resume/identify）だが挙動がフレームワークに隠蔽 | `Shard::next_event()` が resume/reconnect を内蔵しつつ、失敗は `Err` として**呼び出し側に見える**。個別 event 処理の panic はタスク境界で隔離、`JoinError::is_panic()` で検知し当該 Bot のみ再 spawn |

### 決定
**twilight を採用する。** 決め手は3点:
1. **マルチテナント Bot が第一級要件**（`system_default` + N ユーザー Bot）で、`Shard`＝軽量接続プリミティブという twilight のモデルが `customClients: Map<BotId, Client>` に構造対応する。serenity の N 個フルスタック `Client` はメモリ・キャッシュ会計が読みにくい。
2. **トークンローテーションが Shard の spawn/cancel に還元**され、Node が苦しんだ「destroy済み再利用不可＋ライブバインディング再代入」の回避策一式（`restartDefaultBot` の新インスタンス生成、`startInFlight` の直列化ガード）が不要になる。
3. **エラーが型で見える**。twilight は握り潰さず `Err` を返すので、絶対制約1（thiserror＋網羅match）と噛み合う。serenity のフレームワークは内部で握り潰す箇所があり制約1と衝突する。

### intent の型化（`GuildMembers` 不使用の制約を型に落とす）

既存の「`GuildMembers` 特権 intent を使わない → メンバー一覧は不完全」を twilight の `Intents` ビットフラグで表現し、キャッシュ層は**不完全性を型で表す**:

```rust
use twilight_gateway::Intents;

const YUUKA_INTENTS: Intents = Intents::GUILDS
    .union(Intents::GUILD_MESSAGES)
    .union(Intents::MESSAGE_CONTENT)
    .union(Intents::DIRECT_MESSAGES);
// GUILD_MEMBERS は付けない（特権intent。メンバー一覧は常に不完全）

/// getGuildOptionsForBot の members_complete:false を型で表現。
/// UI が ID 手入力で補うセマンティクスを Option の外側で保証する。
pub enum GuildMemberList {
    /// GUILD_MEMBERS intent 不使用のため常にこちら。cached はキャッシュ済みのみ。
    Partial { cached: Vec<CachedMember> },
    // Complete バリアントは intent を足すまで構築不能（型で封じる）
}
```

---

## C-2. Bot アクター化とトークンローテーション

Node の `bot.ts`（`export let client` 差し替え / `customClients` / `startInFlight` 直列化 / `claimMessageOnce` 冪等ガード）を、**各 Bot = 1 アクター（1 タスク + コマンド `mpsc`）**へ置換する。起動・停止・トークン差し替えを**コマンドとして直列処理**すれば、二重 Client・レースが型で消える（棚卸し#3 §5.2 の結論を採用）。

### エラー型（絶対制約1: thiserror）

```rust
#[derive(Debug, thiserror::Error)]
pub enum BotError {
    #[error("bot {bot_id} のトークン復号に失敗: {source}")]
    TokenDecrypt { bot_id: String, source: CryptoError },
    #[error("bot {bot_id} は suspended のため起動不可")]
    Suspended { bot_id: String },
    #[error("bot {bot_id} のトークンが空（初回セットアップ未完了）")]
    NoToken { bot_id: String },
    #[error("gateway 接続失敗 bot={bot_id}: {source}")]
    Gateway { bot_id: String, #[source] source: twilight_gateway::Error },
    #[error("bot アクターへのコマンド送信に失敗（アクター停止済み）")]
    ActorGone,
}
```

`ActorGone` を独立バリアントにするのが要点。「アクターが既に落ちている」と「Discord API エラー」を呼び出し側が **match で判別・分岐**できる（制約1）。

### アクターとレジストリ

```rust
pub enum BotCommand {
    /// system_default のトークン差し替え（restartDefaultBot 相当）
    RotateToken { new_token: SecretString, reply: oneshot::Sender<Result<(), BotError>> },
    Stop { reply: oneshot::Sender<()> },
    Status { reply: oneshot::Sender<BotStatus> },
}

/// customClients + client（default）を統合。ライブバインディング再代入の Rust 版。
pub struct BotRegistry {
    bots: DashMap<BotId, BotHandle>,           // BotId → コマンド送信口
}
pub struct BotHandle {
    tx: mpsc::Sender<BotCommand>,
    /// notifier / getBotClientForUser が「常に最新の http/ready」を掴むための差し替え点。
    /// ESM ライブバインディング → ArcSwap へ写像（棚卸し#3 §5.2）。
    live: Arc<ArcSwap<BotLive>>,
}
pub struct BotLive {
    http: Arc<twilight_http::Client>,
    bot_user_id: Id<UserMarker>,
    ready: Arc<AtomicBool>,   // readyAt チェックの Rust 版（Shard 接続状態）
}
```

`getBotClientForUser(botId)` の「独自Botがoffなら default へフォールバック」は `live.load()` の `ready` を見て default handle へ落とす。`restartDefaultBot` の「新Shard spawn → `ArcSwap::store`」により、notifier 側は `live.load()` で常に最新を掴む（ライブバインディングと等価）。

### 冪等配送（`claimMessageOnce`）

単一プロセス・アクター化で 1 Bot=1 Shard なら本来不要。ただし**同一 identity の Bot が別 Bot にも共有される可能性**（同じ user id で複数 Bot がメンションされ得る）と将来の水平スケールに備え、**Redis `SET NX EX` へ寄せる**（棚卸し#3 §5.4）。Node のプロセス内 Map+TTL60s は単一プロセス前提なので採らない。

```rust
// claim_message_once: bot_user_id:message_id を SET NX EX。Redis 不通時は in-memory へ縮退（制約2）。
async fn claim_message_once(&self, key: &str) -> ClaimResult {
    match self.redis.set_nx_ex(key, "1", Duration::from_secs(60)).await {
        Ok(true)  => ClaimResult::First,
        Ok(false) => ClaimResult::Duplicate,
        Err(RedisError::Unavailable) => self.memory_claim.claim(key),  // 縮退
        Err(e) => ClaimResult::First,  // ← これは禁止。下記の通り握り潰さない
    }
}
```

上の最後の行は**制約1違反なので実際には書かない**。正しくは Redis の一時障害を `RedisError::Unavailable` に正規化して縮退経路へ流し、それ以外の Redis エラー（プロトコル異常等）は `ClaimResult::Error(RedisError)` として呼び出し側へ返し、呼び出し側が「冪等保証不能 → このメッセージは処理せず次の messageCreate に委ねる」を明示選択する。**「失敗時 First 扱い＝二重応答」を暗黙に選ばない**。

### supervisor（絶対制約2: 自己復帰）

```rust
/// 各 Bot アクター/cron/gemini worker を監督。指数バックオフで再 spawn。
pub struct Supervisor {
    shutdown: CancellationToken,
}
impl Supervisor {
    pub fn spawn_supervised<F, Fut>(&self, name: &'static str, mut factory: F)
    where F: FnMut() -> Fut + Send + 'static, Fut: Future<Output = Result<(), BotError>> + Send {
        let token = self.shutdown.clone();
        tokio::spawn(async move {
            let mut backoff = Duration::from_millis(500);
            loop {
                if token.is_cancelled() { return; }
                // panic 隔離: タスクを別 spawn し JoinHandle で is_panic 検知
                let handle = tokio::spawn(factory());
                match handle.await {
                    Ok(Ok(())) => return,                        // 正常終了
                    Ok(Err(e)) => log_typed(name, &e),           // 型付きエラー（黙殺しない）
                    Err(join) if join.is_panic() => tracing::error!(%name, "panic 隔離"),
                    Err(_) => return,                            // cancel 由来
                }
                tokio::select! {
                    _ = token.cancelled() => return,
                    _ = tokio::time::sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(Duration::from_secs(30)); // 上限30s（redis.ts と同値）
            }
        });
    }
}
```

- **panic=unwind** をこのクレートで強制（`Cargo.toml` の `[profile.release] panic = "unwind"`）。panic=abort だと 1 タスクの panic がプロセスを落とすので、supervisor 隔離のため unwind 必須（棚卸し#3 §5.3）。
- **fail-fast との境界**: 起動時のシークレット不備（`YUUKA_ENCRYPTION_SECRET` 欠落・32文字未満）だけは `main` で `std::process::exit(1)`。これは supervisor に載せない「本当に致命的」パス（制約2の但し書き）。既存 `index.ts` の exit(1) 条件と同一。

### graceful shutdown

`tokio::signal` で SIGINT/SIGTERM → `CancellationToken::cancel()` → 全アクターへ伝播 → `tokio::time::timeout(Duration::from_secs(15), join_all(handles))`。Node の 15秒 watchdog（`.unref()` 付き setTimeout）を `select!` で構造化。停止順は起動の逆順（metrics→synapse→…→bot）を `Vec<Box<dyn Shutdown>>` の逆順 drain で表現。

---

## C-3. Gemini REST クライアント（自前・型付き）

公式 Rust SDK 不在。棚卸し#4(A) の通り依存面は薄い（`generateContent` + `responseSchema` + `inlineData` + `ToolConfig` の4機能のみ）。AI Studio（APIキー系、`?key=`）を叩く `reqwest` ベースの型付きクライアントを書く。

### リクエスト/レスポンス構造体（serde）

```rust
#[derive(Serialize)]
pub struct GenerateContentRequest {
    pub contents: Vec<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<Content>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_config: Option<ToolConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<GenerationConfig>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Content { pub role: Role, pub parts: Vec<Part> }

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub enum Part {                                   // 内部タグ無し。serde が形状で判別
    Text { text: String },
    InlineData { inline_data: InlineData },        // { mimeType, data(base64) }
    FunctionCall { function_call: FunctionCall },  // { name, args: Value }
    FunctionResponse { function_response: FunctionResponse },
}

#[derive(Serialize)]
pub struct Tool { pub function_declarations: Vec<FunctionDeclaration> }

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolConfig { pub function_calling_config: FunctionCallingConfig }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionCallingConfig {
    pub mode: FunctionCallingMode,                 // AUTO | ANY | NONE
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_function_names: Option<Vec<String>>,  // 完了ハルシネーション是正で使用
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_mime_type: Option<String>,        // "application/json"（プランナー）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_schema: Option<Schema>,           // JSON強制
}
```

`Part` を internally-tagged enum ではなく**フィールド存在で判別**する形にするのが肝（Gemini wire は判別タグを持たない）。`#[serde(untagged)]` は誤マッチしやすいので、各バリアントを固有キー（`text`/`inlineData`/`functionCall`/`functionResponse`）にして serde の非タグ判別に頼らず、**カスタム `Deserialize` でキー存在を明示分岐**する（誤判定を型で防ぐ）。

### function-calling のツール宣言＝型付きスキーマ

Node の `jsonSchemaToGeminiSchema`（`type` 配列＝nullable、`anyOf/oneOf/allOf`、STRING 潰れ防止）を Rust では **`schemars` 由来ではなく `Schema` を直接 enum で持つ**:

```rust
#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Schema {
    String { #[serde(skip_serializing_if="Vec::is_empty")] r#enum: Vec<String>,
             #[serde(skip_serializing_if="Option::is_none")] description: Option<String> },
    Integer { description: Option<String> },
    Number  { description: Option<String> },
    Boolean { description: Option<String> },
    Array   { items: Box<Schema> },
    Object  { properties: BTreeMap<String, Schema>, required: Vec<String>,
              #[serde(skip_serializing_if="Option::is_none")] description: Option<String> },
}
```

これで「静的ツールは Rust の型から `Schema` を直接構築」でき、Node が手書き JSON でやっていた宣言が型安全になる（棚卸し#4(A) の「Rust なら有利」を具体化）。**MCP 動的ツール**だけは JSON Schema(実行時取得) → `Schema` 変換関数 `json_schema_to_gemini(&Value) -> Result<Schema, SchemaError>` が要る（C-7）。ここは Node の `jsonSchemaToGeminiSchema` の分岐（`type` 配列 nullable、combinator）を忠実移植し、変換不能は `SchemaError` で返す（黙殺しない）。

### エラー型（thiserror・網羅）

```rust
#[derive(Debug, thiserror::Error)]
pub enum GeminiError {
    #[error("HTTP転送失敗: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("Gemini API {status}: {code} {message}")]
    Api { status: u16, code: String, message: String, retry_after: Option<Duration> },
    #[error("レート制限 (429)、retryDelay={0:?}")]
    RateLimited(Option<Duration>),
    #[error("サーバ側一時障害 ({0})")]           // 5xx
    ServerTransient(u16),
    #[error("応答のパースに失敗: {0}")]
    Decode(#[source] serde_json::Error),
    #[error("APIキー未設定/復号失敗")]
    NoApiKey,
    #[error("タイムアウト（{0:?} 経過）")]
    Timeout(Duration),
    #[error("サーキットブレーカ open（{until:?} まで遮断）")]
    CircuitOpen { until: Instant },
}
```

`RateLimited` / `ServerTransient` / `Timeout` を **リトライ可能**、`Api`(4xx除く429) / `NoApiKey` / `Decode` を**リトライ不可**として型で区別（下の `is_retryable()`）。呼び出し側（`runFunctionCallingLoop` 相当）が match で分岐できる。

### リトライ（`RetryInfo.retryDelay` 尊重）

Node の `generateWithRetry`（429/5xx、`error.details[].@type == RetryInfo` の `retryDelay` を尊重、120秒タイムアウト）を型付き自前実装:

```rust
impl GeminiError {
    fn is_retryable(&self) -> bool {
        matches!(self, Self::RateLimited(_) | Self::ServerTransient(_)
                     | Self::Timeout(_) | Self::Transport(_))
    }
    fn retry_hint(&self) -> Option<Duration> {
        match self { Self::RateLimited(d) => *d, Self::Api { retry_after, .. } => *retry_after, _ => None }
    }
}

pub async fn generate_with_retry(&self, req: &GenerateContentRequest, max: u32)
    -> Result<GenerateContentResponse, GeminiError>
{
    let mut attempt = 0;
    let mut backoff = Duration::from_millis(500);
    loop {
        match tokio::time::timeout(Duration::from_secs(120), self.generate_once(req)).await {
            Ok(Ok(resp)) => return Ok(resp),
            Ok(Err(e)) if e.is_retryable() && attempt < max => {
                let wait = e.retry_hint().unwrap_or(backoff);   // RetryInfo 優先
                tokio::time::sleep(wait).await;
                backoff = (backoff * 2).min(Duration::from_secs(20));
                attempt += 1;
            }
            Ok(Err(e)) => return Err(e),                        // リトライ不可 or 上限 → 型で返す
            Err(_) => {
                if attempt < max { attempt += 1; continue; }
                return Err(GeminiError::Timeout(Duration::from_secs(120)));
            }
        }
    }
}
```

`error.details[].@type` の `RetryInfo` パースは `parse_api_error(status, body) -> GeminiError` で行い、`@type` に `type.googleapis.com/google.rpc.RetryInfo` を含む要素の `retryDelay`（`"3s"` 形式）を `Duration` へ。

### サーキットブレーカ（キー単位）

外部 Gemini の連続障害で全ターンを詰まらせないため、**APIキー（ユーザー/Bot）単位**のブレーカを持つ（劣化縮退＝制約2）:

```rust
pub struct CircuitBreaker { state: Mutex<CbState> }
enum CbState {
    Closed { fails: u32 },
    Open   { until: Instant },
    HalfOpen,
}
// 連続5失敗で Open(30s)。Open 中は即 GeminiError::CircuitOpen を返し、
// 呼び出し側（gemini ループ）は「LLM 縮退＝直近履歴のみで定型応答 or 一時応答」へ落とす。
```

ブレーカ open は**エラーを黙殺せず** `CircuitOpen{until}` として返し、ユーザーには「一時的に応答できない」旨を型経由で返す。Node には無い堅牢化で、制約3（現行より堅牢）を満たす。

### ストリーミング

現状 Node は未使用（棚卸し#4(A)）。`reqwest` の `features=["stream"]` は入れておくが、**初期実装は非ストリーミング（`:generateContent`）で開始**。将来 `:streamGenerateContent?alt=sse` を `reqwest::Response::bytes_stream()` + SSE パーサで足す拡張点だけ用意（`generate_stream()` を `trait GeminiApi` に予約メソッドとして置く）。ここで `unimplemented!()` は制約1で禁止なので、未実装期間は**そのメソッド自体を生やさない**（trait に足すのは実装と同時）。

### APIキーのユーザー別/ボット別管理と復号

Node の `llmClient`（`getUserGenAI`/`getBotGenAI`、AES-GCM 復号、`userAICache`/`botAICache`）を写像:

```rust
pub struct LlmClientPool {
    http: reqwest::Client,                     // 接続プールは全キー共有
    user_cache: DashMap<UserId, Arc<GeminiClient>>,  // apiKey 一致でキャッシュ再利用
    bot_cache:  DashMap<BotId,  Arc<GeminiClient>>,
    crypto: Arc<Crypto>,                       // AES-256-GCM 復号（領域B）
}
impl LlmClientPool {
    pub async fn user_client(&self, uid: &UserId) -> Result<Arc<GeminiClient>, GeminiError> {
        let conf = self.repo.user_gemini_config(uid).await.map_err(|_| GeminiError::NoApiKey)?;
        let key = self.crypto.decrypt_text(&conf.enc, &conf.iv, &conf.tag)
            .map_err(|_| GeminiError::NoApiKey)?;   // 復号失敗＝キー無し扱い（Nodeと同一）
        // apiKey ハッシュ一致ならキャッシュ再利用
        Ok(self.get_or_build(uid, key, conf.model.unwrap_or_else(|| DEFAULT_MODEL.into())))
    }
}
```

- 復号鍵は `SecretString`（`secrecy` crate）で保持し `Debug` で漏らさない。ログにキーが乗らないよう `GeminiClient` の `Debug` は手実装で `?key=` をマスク。
- **汎用モードは発話者が登録済みでも本人キーを使わない**（テナント分離、棚卸し#4 §4）を型で保証: `GuildTurn` 経路は `bot_client(bot_id)` しか呼べず `user_client` を型的に受け取らない（関数シグネチャで分離）。

---

## C-4. ツールレジストリ: トレイトベースへの写像

Node の `FunctionModule { declarations, handlers }` + `buildFunctionRegistry`（重複throw / dispatch が未知関数・例外を `{success:false}` へ正規化 / 戻り値は常に文字列）を、**トレイト＋型付き `ToolOutcome`** へ写す。棚卸し#4(B) の「緩い文字列規約を第一級型へ」を採用。

### Tool トレイトと戻り値契約

```rust
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn declaration(&self) -> &FunctionDeclaration;   // 名前は declaration().name が正
    async fn call(&self, ctx: &ToolContext, args: serde_json::Value) -> ToolOutcome;
}

/// 現状の「文字列＋success:false 判定」を型で固定化。
/// tool_outcome の status 判定・完了ハルシネーション検知・ブラウザ成否追跡が全てこの型に集約。
pub struct ToolOutcome {
    pub status: ToolStatus,             // Success | Failure
    pub payload: serde_json::Value,     // LLM へ返す functionResponse の中身
    pub attachments: Vec<Attachment>,   // showRichContent/sendChart の副作用を「戻り値」へ寄せる
}
pub enum ToolStatus { Success, Failure }
pub enum Attachment { Embed(EmbedJson), File { name: String, bytes: Bytes } }
```

**要点（借用検査との整合）**: Node の `ctx.embeds`/`ctx.files` への `push`（`showRichContent`/`sendChart`）は、Rust では `&mut ctx` の内部可変化ではなく **`ToolOutcome.attachments` として戻り値へ寄せる**（棚卸し#4(B)）。`ctx` は `&ToolContext`（不変借用）で済み、複数ツールの並行実行時も借用が衝突しない。`richReplyEnabled=false` のときは呼び出し側（ループ）が `attachments` を捨てる（ツール側は常に積んでよい＝責務が単純化）。

### ToolContext（Rust 版）

```rust
pub struct ToolContext {
    pub bot_id: BotId,
    pub user_id: UserId,             // データ分離の必須キー（型で non-optional）
    pub guild_id: Option<GuildId>,   // ギルド常駐 Bot のみ
    pub rich_reply_enabled: bool,
    // embeds/files は削除（ToolOutcome.attachments へ移動）
    pub deps: Arc<ToolDeps>,         // repo/crypto/notifier/browser 等の依存注入
}
```

`user_id` を `Option` にしない（Node のコメント「全リポジトリ呼び出しに必須」を型で強制）。領域Gの「user_id を型で強制する `UserScopedRepo`」と接続し、ツールが素の `user_id: String` を触れず `ctx.scoped_repo()` 経由でしか DB に到達できない設計にすると、棚卸し#2 §7 のクロステナント事故（`bot_mcp_access` の穴）を型で封じられる。

### レジストリ（重複throw の型化 / dispatch の正規化）

```rust
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("Function名が重複: {0}")]           // 起動時 fail-fast（Node の throw と同義）
    DuplicateName(String),
}

pub struct ToolRegistry { tools: HashMap<String, Arc<dyn Tool>> }

impl ToolRegistry {
    pub fn build(modules: Vec<Arc<dyn Tool>>) -> Result<Self, RegistryError> {
        let mut tools = HashMap::new();
        for t in modules {
            let name = t.declaration().name.clone();
            if tools.insert(name.clone(), t).is_some() {
                return Err(RegistryError::DuplicateName(name));  // 重複＝起動失敗
            }
        }
        Ok(Self { tools })
    }
    pub fn declarations(&self) -> Vec<&FunctionDeclaration> { /* ... */ }

    /// Node dispatch の「未知関数・panic を success:false へ正規化」を型で。
    pub async fn dispatch(&self, ctx: &ToolContext, name: &str, args: serde_json::Value) -> ToolOutcome {
        let Some(tool) = self.tools.get(name) else {
            return ToolOutcome::failure(json!({ "success": false, "message": format!("不明な関数: {name}") }));
        };
        // ハンドラ内 panic をループから隔離（catch_unwind ではなく別タスク境界）。
        // ただし Tool::call は Result を返さない設計（ToolOutcome に Failure を畳む）ので、
        // 通常の失敗は panic せず ToolStatus::Failure で返る。panic は真の bug のみ。
        match tokio::spawn({
            let tool = Arc::clone(tool); let ctx = ctx.clone_shallow(); 
            async move { tool.call(&ctx, args).await }
        }).await {
            Ok(outcome) => outcome,
            Err(join) if join.is_panic() => {
                tracing::error!(tool = name, "ツール panic を隔離");
                ToolOutcome::failure(json!({ "success": false, "message": "内部エラー" }))
            }
            Err(_) => ToolOutcome::failure(json!({ "success": false, "message": "キャンセル" })),
        }
    }
}
```

Node の `dispatch` が `try/catch → {success:false}` で「例外がループを壊さない」を保証していたのに対し、Rust では **`Tool::call` が `ToolOutcome`（Result ではない）を返す設計にして、通常の失敗経路で panic させない**。panic は真のバグのみで、それを `tokio::spawn` のタスク境界で隔離する（`catch_unwind` は非 `UnwindSafe` 型で扱いづらいためタスク境界を使う）。これで「失敗経路に名前と型」（制約1）と「panic でループ全体を壊さない」（制約2）を両立。

### モジュールカタログ（cap/module ゲート）

Node の `MODULE_CATALOG`（`id` 永続キー・`cap`・`selectable`・cap ゲート）を静的テーブル化:

```rust
pub struct ModuleEntry {
    pub id: &'static str,          // enabled_modules 永続キー（リネーム禁止）
    pub cap: Capability,           // Persona | Memory | Mcp | Secretary | Core
    pub selectable: bool,
    pub tools: Vec<Arc<dyn Tool>>,
}
pub enum Capability { Core, Persona, Memory, Mcp, Secretary }

/// getFunctionModulesForCapabilities + resolveEnabledModulesForUser の2段ゲートを1関数に。
pub fn resolve_tools(caps: &CapabilitySet, enabled: &EnabledModules) -> Vec<Arc<dyn Tool>> {
    MODULE_CATALOG.iter()
        .filter(|m| m.cap == Capability::Core || caps.contains(m.cap))  // capゲート
        .filter(|m| enabled.allows(m.id, m.selectable))                  // ユーザー選択ゲート（[]尊重）
        .flat_map(|m| m.tools.iter().cloned())
        .collect()
}
```

`EnabledModules::allows` は Node のセマンティクス（`NULL`→全有効 / `[...]`→集合 / `[]`→core のみ）を `enum EnabledModules { All, Only(HashSet<&str>) }` で型化。

---

## C-5. 完了ハルシネーション是正（Function Calling の核心安全弁）

Node の核心（`claimsActionCompleted` → `FunctionCallingMode.ANY` + `allowedFunctionNames` で構造的に関数呼び出しを強制、最大2回）を移植。これが本コードベースで最も価値のある FC 設計なので**忠実再現**する:

```rust
// ループ内: ツールを1度も呼ばず「登録しました」等と自然文完了主張したら、
// ANY モード + プラン候補ツールに絞った allowedFunctionNames で再生成。
if tool_calls_this_turn == 0 && claims_action_completed(&final_text) && correction < 2 {
    let cfg = ToolConfig {
        function_calling_config: FunctionCallingConfig {
            mode: FunctionCallingMode::Any,
            allowed_function_names: Some(plan.candidate_tools.clone()),  // プラン候補に限定
        },
    };
    // 同じ contents で ToolConfig だけ差し替えて再送 → functionCall を構造的に強制
    correction += 1;
    continue;
}
```

`claims_action_completed` は Node の正規表現（「登録しました」等）を Rust の `regex` で移植。`COMPLETION_CORRECTION_PROMPT` も定数移植。

---

## C-6. synapse recall 注入と tool_outcome フィードバック

領域外（棚卸し#4/#5 の `synapseEngine`/`toolOutcomeRepo`）だが、本領域の gemini ループが**呼び出す**ので接続点を定義する。

### recall 注入

`assembleRecall(scope, query, k, timeCtx?, recencyCtx?)` は Rust synapse デーモンへの IPC（改行区切りJSON、FROZEN プロトコル）。本領域からは**型付きクライアント**として叩く:

```rust
#[async_trait]
pub trait SynapseRecall: Send + Sync {
    /// タイムアウト2500ms。エンジン無効/timeout は Ok(None) で縮退（Err にしない＝制約2の劣化縮退）。
    async fn assemble(&self, scope: &SynapseScope, query: &str, k: usize,
                      time: Option<TimeCtx>, recency: Option<RecencyCtx>)
        -> Result<Option<RecallResult>, SynapseError>;
}
```

- **縮退の型設計**: 「エンジン不在/timeout＝直近履歴のみへデグレード」は `Ok(None)`（正常な縮退）、「IPC プロトコル異常」は `Err(SynapseError)`（バグ）を区別。Node の「全 API が throw しない」を、Rust では `Ok(None)` と `Err` の使い分けで**より厳密に**表現（制約1: 縮退とエラーを型で分ける）。
- recall クエリ組み立て（返信時は返信元直近2件＋現発話結合、`RECALL_QUERY_MAX_LEN=500` キャップ）と時刻/recency 再ランキング重み（`SYNAPSE_TIME_BIAS_WEIGHT=0.1`、`synapseRecencyWeight=0.15`）は本領域の `build_recall_query` / `build_recall_section` に移植。ヒット時 `touch_synapses` で鮮度更新、`recall_hit`/`recall_empty` メトリクス記録。

### tool_outcome フィードバック

ループ内で各ツール実行後に `record_tool_outcome`:

```rust
pub struct ToolOutcomeRecord {
    pub user_id: UserId, pub bot_id: BotId, pub guild_id: Option<GuildId>,
    pub topic_id: Option<String>,       // 現状 None（2nd Hop 未完、棚卸し#4 §5）
    pub tool_name: String,
    pub args_digest: String,            // build_outcome_args_digest で秘匿マスク
    pub status: ToolStatus,             // C-4 の ToolStatus をそのまま流用
    pub latency_ms: u64,
}
```

`ToolStatus` を C-4 のツール戻り値型と**同一型**にすることで、Node で散っていた「success:false 判定」「outcome status 判定」「完了ハルシネーション検知」の3箇所が単一の `ToolStatus` に収束（棚卸し#4(B) の主眼）。`args_digest` の秘匿マスク（`buildOutcomeArgsDigest` 相当）は移植必須。

---

## C-7. MCP 動的ツール = プラグイン API の trait 境界（領域G接続）

棚卸し#4 §6 の「外部の宣言＋ハンドラを実行時生成し既存レジストリへ同一型でマージ」は、**そのままプラグイン機構**。Rust では動的 MCP ツールを `Arc<dyn Tool>` として実行時生成すれば静的ツールと同じ `HashMap` に混ざる。

```rust
/// MCP サーバー1ツール = 1 Tool 実装。スコープ再検証・命名衝突回避・監査を内包。
pub struct McpTool {
    server_id: i64,
    decl: FunctionDeclaration,        // jsonSchemaToGeminiSchema 相当で生成済み
    speaker_scope: UserId,            // system_default は発話者本人スコープに限定（クロステナント防止）
    client: Arc<McpClient>,
}
#[async_trait]
impl Tool for McpTool {
    fn declaration(&self) -> &FunctionDeclaration { &self.decl }
    async fn call(&self, ctx: &ToolContext, args: serde_json::Value) -> ToolOutcome {
        // (a) 利用可能性を毎回再検証（無効化/削除されていないか）
        // (b) getServerById で最新レコード（enabled・canManage）
        // (c) addAuditLog("mcp.call", ...) で監査（秘密値なし）
        // (d) callTool 実行、結果 30000 字で切って {success, server, tool, result}
    }
}
```

- **命名衝突回避**: `mcp{serverId}_{sanitized}`（63字上限）＋衝突時 `disambiguate_function_name`（SHA1短ハッシュ付与、捨てずに一意化）を移植。
- **スコープ再検証**を `McpTool` 内部に持たせることで、レジストリ側は静的/動的を区別せず `dispatch` するだけ。**この動的マージ点がプラグイン trait 境界**そのもの（棚卸し#4(B) の結論）。
- **将来のユーザー独自モジュール**（制約4）: `trait Tool` を実装した外部クレートを `Arc<dyn Tool>` として `ToolRegistry::build` に渡す口を最初から開ける。MCP・crawler/synapse デーモン（改行区切りJSON）との整合は、外部プロセス型プラグインを `McpTool` と同型の「IPC を内包する `Tool` 実装」として表現すれば同じ配管に乗る。プラグイン登録は起動時の `Vec<Arc<dyn Tool>>` 構築点に集約し、重複名は `RegistryError::DuplicateName` で fail-fast。

---

## C-8. エラー処理の機械強制（絶対制約1）

設計に**強制機構**を織り込む:

```toml
# Cargo.toml（各クレート）
[lints.clippy]
unwrap_used   = "deny"
expect_used   = "deny"
panic         = "deny"
todo          = "deny"
unimplemented = "deny"
# 非起動パスでの .unwrap()/.expect()/panic!/todo!/unimplemented! を機械禁止。
# 起動時 fail-fast（main のシークレット検証等）は #[allow(clippy::expect_used)] を明示付与し、
# その箇所だけ許可（許可箇所が grep 可能になる）。
```

```toml
# deny.toml（cargo-deny）
[bans]
deny = [
  { name = "anyhow" }, { name = "eyre" }, { name = "color-eyre" },
  { name = "snafu" },                       # context チェインが握り潰しに寄る
]
```

- **エラー黙殺の禁止**: `let _ = fallible()` は clippy `let_underscore_must_use` + 独自 lint で検出。`Result` を捨てる箇所は必ず `match` か `?`。
- CI で `cargo clippy -- -D warnings` + `cargo deny check bans` をゲート。
- `thiserror` の各 enum は**網羅 match を呼び出し側に強制**（`#[non_exhaustive]` は付けない＝バリアント追加時に全 match が壊れて漏れを検出。ライブラリ公開しない内部クレートなので破壊的変更を歓迎する方針）。

---

## C-9. 型連携（領域F/制約5との接続点）

本領域が生む wire 型のうち **フロントへ出るもの**（Bot ステータス・ツール実行結果のダッシュボード表示等）は `#[derive(ts_rs::TS)]` を付け、`gen-types` バイナリで `types.ts` を生成する側に載せる。Gemini/Discord の内部 wire 型（`Content`/`Part` 等）はフロントに出さないので `TS` を付けない（DTO とドメイン型の分離＝棚卸し#6 §5.1）。ツール宣言の `FunctionDeclaration` は Gemini 送信専用で、フロント露出しない。

---

## まとめ（この領域の確定事項）

1. **Discord = twilight**。Bot をアクター化し、トークンローテーションを `ArcSwap` + Shard spawn/cancel に還元。`startInFlight`/`claimMessageOnce`（プロセス内Map）は不要化 or Redis SETNX へ。
2. **Gemini = reqwest 自前クライアント**。`generateContent`+`responseSchema`+`inlineData`+`ToolConfig` の薄い REST ラッパ。リトライは `RetryInfo` 尊重の型付き自前実装、キー単位サーキットブレーカで劣化縮退。ストリーミングは拡張点のみ用意。
3. **ツールレジストリ = `trait Tool` + `ToolOutcome`**。Node の「文字列＋success:false」の緩い規約を `ToolStatus` へ一元化。副作用（embeds/files）は戻り値の `attachments` へ寄せる。MCP 動的ツールが `Arc<dyn Tool>` としてプラグイン trait 境界になる。
4. **完了ハルシネーション是正**（`ANY`+`allowedFunctionNames`）を忠実移植。
5. **エラーは thiserror のみ**、`anyhow`/`unwrap` 等を clippy/cargo-deny で機械禁止。縮退（`Ok(None)`）と真のエラー（`Err`）を型で区別。supervisor + 指数バックオフ（上限30s）+ panic=unwind でアクター単位の自己復帰。fail-fast は起動時シークレット検証のみ。

参照した実ファイル: `src/types/contracts.ts`（`ToolContext`/`FunctionModule`/`TurnAsyncDelivery` の正確なシグネチャ）、`src/functions/registry.ts`（重複throw・dispatch の `{success:false}` 正規化・戻り値文字列規約）。

---

## [13] I have enough verified detail. The two files confirm the current `FunctionModule
<sub>agentId: af8e6955ac819128b</sub>

I have enough verified detail. The two files confirm the current `FunctionModule` shape (declarations + name→handler map, string return, throw-on-duplicate) and `ToolContext` (botId/userId/guildId + mutable embeds/files queues). Writing the design section now.

---

# 領域: ユーザー拡張モジュール・アーキテクチャ(将来)

## 0. 結論（先に推奨を確定）

**3層ハイブリッドを初期アーキテクチャに織り込む。** 後付けにしないための唯一の要件は「内蔵ツール・MCPツール・将来のユーザー拡張の3者が、同一の `Tool` トレイトと同一の `ToolRegistry` に収束すること」。この収束点さえ最初に作れば、拡張方式（WASM/サブプロセス）は後から実装を足すだけになる。

| 層 | 対象 | 方式 | 信頼境界 | 初期実装 |
|---|---|---|---|---|
| **T1 内蔵** | todo/schedule/finance 等 45ツール | コンパイル時 `Arc<dyn Tool>` レジストリ | 信頼済み（同一プロセス） | **P1で実装** |
| **T2 外部プロセス** | 既存MCPサーバー、rust_crawler/synapse型デーモン、ユーザーの「自前デーモン」 | サブプロセス/MCP（改行JSON or JSON-RPC）| 準信頼（プロセス分離・OS権限） | **P1で実装**（既存資産の吸収） |
| **T3 非信頼WASM** | user がアップロードする任意コード | wasmtime コンポーネント（WASI p2 + WIT） | **非信頼**（サンドボックス） | **P3で実装**（トレイト境界のみP1で確定） |

各方式の却下理由を制約に沿って明記する。

- **(a) 内蔵トレイトのみ** → 制約4を満たさない。ユーザーは再コンパイルなしに機能追加できない。内蔵専用として採用（T1）。
- **(b) 動的ネイティブ (.so / libloading / abi_stable)** → **全面却下**。`libloading::Library::get` は `unsafe fn`、シンボルの型を呼び出し側が unsafe に約束する。Rust の ABI は不安定でコンパイラバージョン間で壊れる。`abi_stable` は緩和するが、非信頼コードを**同一アドレス空間**にロードする時点で制約1（panic隔離）・制約2（自己復帰）と根本的に非互換。ユーザーの `.so` の panic は `panic=unwind` でも FFI 境界を越えると UB、`segfault` はホストプロセスごと落とす=常時稼働制約に違反。**採用しない。**
- **(c) WASM (wasmtime)** → 非信頼コード用に採用（T3）。フューエル計量・メモリ上限・ケイパビリティ注入で制約1/2を型と実行時の両面で守れる。
- **(d) サブプロセス/MCP** → 外部連携用に採用（T2）。既存の `mcpDynamic` / `CrawlerDaemon` / `SynapseDaemon` がすでにこのパターンで、新設計はこれを**同じトレイトに射影して吸収**する。

---

## 1. 中核トレイト：全層が収束する境界

現行 `FunctionModule`（`src/types/contracts.ts:55`）は「宣言配列 + 名前→ハンドラMap、戻り値は文字列、重複はthrow」。この緩い契約を Rust で型付けし直すのが最大の勝ち筋。棚卸し#4(B) の指摘通り、戻り値契約を第一級型にする。

```rust
// crate: yuuka-tool-core  （T1/T2/T3 と registry/gemini が共通依存）

use std::sync::Arc;
use serde_json::Value;

/// ツール宣言（Gemini function-calling へ射影する中立表現。SDK 型に依存しない）
#[derive(Clone, Debug, serde::Serialize)]
pub struct ToolDeclaration {
    pub name: String,               // レジストリ一意キー
    pub description: String,
    pub parameters: ToolSchema,     // JSON-Schema 中立表現（領域E が Gemini Schema へ変換）
    pub requires_confirmation: bool,
}

/// ツール実行の構造化結果。文字列JSON規約を型で置換（棚卸し#4(B)）。
pub enum ToolOutcome {
    Ok { payload: Value, attachments: Vec<Attachment> },
    /// 「回復可能な失敗」= LLM に返して続行させる（現行 success:false 相当）
    Failed { code: ToolFailure, message: String },
}

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn declaration(&self) -> &ToolDeclaration;
    /// 実行。ホスト側エラー（DB断など）は Err で型付き伝播、
    /// ツール意味上の失敗は Ok(ToolOutcome::Failed) で返す。この二分が制約2の核。
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutcome, ToolError>;
}
```

**`ToolError` は thiserror 列挙型（制約1）。** `anyhow` 相当は一切使わない。`dispatch` は `catch_unwind` に頼らず `Result` で正規化する（棚卸し#4(B)の推奨を採用）。

```rust
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("tool not found: {0}")]
    NotFound(String),
    #[error("argument decode failed: {0}")]
    BadArgs(#[source] serde_json::Error),
    #[error("capability denied: {needed:?} not granted to user {user_id}")]
    CapabilityDenied { needed: Capability, user_id: String },
    #[error("sandbox trap: {0}")]           // WASM: fuel切れ/memory超過/panic
    SandboxTrap(#[from] WasmTrap),
    #[error("subprocess ipc failed: {0}")]  // MCP/デーモン: timeout/EPIPE/exit
    Ipc(#[from] IpcError),
    #[error("host dependency down: {0}")]   // DB/Redis 等（制約2で縮退対象）
    HostDep(#[source] HostError),
}
```

**`ToolContext` の借用問題を解消する。** 現行の可変 `embeds/files` push（`contracts.ts:28-30`）は非信頼WASMには渡せない（ホストの `EmbedBuilder` を触らせない）。副作用を**戻り値の `attachments` に寄せる**（棚卸し#4(B) の推奨を全面採用）。

```rust
/// 非可変。ケイパビリティ・スコープ・userId 分離を型で保持。
pub struct ToolContext {
    pub bot_id: BotId,
    pub user_id: UserId,            // データ分離の必須キー（制約：全DB呼び出しへ機械注入）
    pub guild_id: Option<GuildId>,
    pub rich_reply_enabled: bool,
    caps: CapabilitySet,           // このユーザー×Botに付与された能力集合
    host: Arc<dyn HostCapabilities>, // T3 が呼べる唯一の副作用面（後述 §4）
}
impl ToolContext {
    /// 能力チェックを型で強制。未付与なら CapabilityDenied を返し呼び出し不能。
    pub fn require(&self, c: Capability) -> Result<Granted, ToolError> { … }
}
```

---

## 2. レジストリ・発見・ライフサイクル

`buildFunctionRegistry`（`registry.ts:8`）の「重複でthrow」を `Result` に、`HashMap` ベースに。**MCP動的ツールも T3 も同じ `Arc<dyn Tool>` として同じ Map に入る** — これが「後付けでない」ことの実体。

```rust
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    decls: Vec<ToolDeclaration>,   // Gemini へ渡す順序保持
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("duplicate tool name: {0}")]     // 現行 throw と同義
    Duplicate(String),
    #[error("name collision after sanitize: {0}")] // mcpDynamic の衝突回避を型化
    SanitizedCollision(String),
}

impl ToolRegistry {
    pub fn insert(&mut self, t: Arc<dyn Tool>) -> Result<(), RegistryError> { … }

    /// gemini.ts の resolveBotCapabilities → resolveEnabledModulesForUser →
    /// getMcpFunctionModuleForBot のマージ手順（棚卸し#4 §1.4）を、
    /// 静的T1・動的T2・T3 の3ソースから毎ターン組み立てる。
    pub fn build_for_turn(
        catalog: &StaticCatalog,        // T1（コンパイル時）
        caps: &CapabilitySet,           // Bot capability × user module ゲート
        mcp: &[Arc<dyn Tool>],          // T2: MCP/デーモン（実行時解決）
        user_ext: &[Arc<dyn Tool>],     // T3: WASM（実行時解決）
    ) -> Result<ToolRegistry, RegistryError> { … }
}
```

**ライフサイクル**は「毎ターン再構築」を維持（現行 gemini.ts が既にそうしている）。理由：MCP/WASM の有効化・無効化・権限剥奪をターン境界で即反映でき、キャッシュ無効化バグの温床（棚卸し#3の `invalidateBotCapabilitiesCache`）を減らせる。T2/T3 の**実体プロセス/インスタンス**は常駐（下記supervisor）で、レジストリ登録だけが毎ターンという分離にする。

---

## 3. T2：既存 MCP・デーモン資産の吸収（P1で完了する）

これが**制約4の「既存資産と整合」を最短で満たす**層。棚卸し#5 が「crawler/synapse/MCP の IPC がほぼ同一フレーミング」と結論している。新設計は**この3者を1つの `SubprocessTool` アダプタに射影**する。

```rust
/// 改行区切りJSONデーモン（rust_crawler/synapse型）と MCP(JSON-RPC/HTTP) の
/// 両方を Tool に射影するアダプタ。プロセスは supervisor 配下で常駐。
pub struct SubprocessTool {
    decl: ToolDeclaration,
    transport: Arc<dyn IpcTransport>,   // NdjsonStdio | McpHttp | McpSse
    reverify: Arc<dyn Fn(&ToolContext) -> Result<(), ToolError> + Send + Sync>,
}
```

**吸収の具体（3者 → 1トレイト）:**

1. **MCPサーバー（`mcpDynamic.ts`）**: `listServersGrantedToBotScoped` のスコープ再検証（発話者本人が付与 + system-level のみ）・`ensureFreshToolsCache`(TTL1h)・`jsonSchemaToGeminiSchema`・`disambiguateFunctionName`(SHA1)・`addAuditLog("mcp.call")` を **`SubprocessTool::call` 内の `reverify` クロージャ + `McpHttp` transport** に移設。棚卸し#2 §5 のクロステナント穴（v7の `owner_id`）は、`ToolContext.require` + transport 生成時の `owner_id = speakerUserId` 束縛で**型的に**再発防止。

2. **rust_crawler / rust_synapse**: 棚卸し#5 の推奨「1つの `yuuka-engine` バイナリ + コマンド名前空間、ただしNodeからは子プロセス」を採用。Rust本体化後も**別OSプロセスのまま**にする（制約2の障害分離：crawlerのChromeクラッシュがsynapseのRAM索引を巻き添えにしない）。`NdjsonStdio` transport が `{id,command,...}→{id,ok,result:Value,error}` を話す。**`id`封筒/`sid`衝突回避のFROZENプロトコルはそのまま**。

3. **ユーザーの自前デーモン（将来）**: user が「改行JSONを話す実行可能ファイル or MCPエンドポイントURL」を登録すれば、既存の `mcp_servers` テーブル（棚卸し#2 テーブル30）と同じ経路で `SubprocessTool` になる。**T3(WASM) より先に、実は T2 だけでユーザー拡張の相当部分が満たせる** — MCP は既に「ユーザーがURLを足す」拡張機構だから。

**推奨：ユーザー拡張の第一選択は T2（MCP/サブプロセス）とする。** WASM(T3)は「ネットワーク先を用意できない/超低レイテンシ/ホスト内データに近接したい」ケース専用に限定する。理由：T2 は OS のプロセス分離・cgroup・seccomp という枯れた隔離を使え、既存 `mcp_servers` の権限モデル・監査・TTLキャッシュをそのまま流用でき、実装リスクが最小。

---

## 4. T3：非信頼WASM（wasmtime + WIT、P3実装／境界はP1確定）

ユーザーが**任意コードそのもの**をアップロードする唯一の経路。libloading(.so)を却下した理由（§0）を、WASM は全て解決する。

**クレート:** `wasmtime`（`Config::consume_fuel(true)`, `epoch_interruption`）+ `wasmtime-wasi`（WASI p2）+ `wit-bindgen`（ホスト⇄ゲスト境界を WIT IDL で型付け）。`extism` は選ばない（内部で wasmtime を隠蔽し、フューエル/epoch/リンカの細粒度制御とカスタムWIT hostcall が扱いづらいため。自前 wasmtime 直叩きの方が制約1/2を精密に守れる）。

**制約1/2をWASMでどう守るか（設計の核心）:**

```rust
pub struct WasmTool {
    decl: ToolDeclaration,
    engine: Engine,                 // 全WASMツールで共有（コンパイル済モジュールキャッシュ）
    component: Component,            // 事前コンパイル済（アップロード時に検証）
    limits: WasmLimits,
}
struct WasmLimits {
    fuel_per_call: u64,             // CPU上限 → 超過で Trap（無限ループを殺す）
    max_memory_bytes: usize,        // StoreLimits でリニアメモリ上限
    wall_timeout: Duration,         // epoch_deadline で壁時計上限（fuelと二重化）
}

#[derive(Debug, thiserror::Error)]
pub enum WasmTrap {
    #[error("fuel exhausted (cpu budget)")] FuelExhausted,
    #[error("wall-clock timeout")]          Timeout,
    #[error("memory limit exceeded")]       OutOfMemory,
    #[error("guest panicked/unreachable")]  GuestTrap,
    #[error("capability call denied: {0:?}")] HostCallDenied(Capability),
}
```

- **panic隔離（制約1）**: ゲストの panic/unreachable は wasmtime の `Trap` になりホストへ `Result::Err` で返る。**ホストプロセスは決して落ちない**。`.so` と違いここが型で保証される。
- **無限ループ・暴走（制約2）**: `fuel` で命令数を、`epoch_deadline`（別スレッドが `engine.increment_epoch()` を刻む）で壁時計を計量。どちらか先に尽きれば `Trap`。ホストの他タスクは無傷。
- **能力の最小化（deny-by-default）**: WASI をそのまま与えない。**ホスト機能は WIT で宣言した hostcall のみ**露出する。ネットワーク・ファイル・環境変数は既定でゼロ。

```wit
// wit/yuuka-ext.wit — ユーザー拡張が呼べる唯一の副作用面
package yuuka:ext@0.1.0;

interface host {
    record scope { user-id: string, bot-id: string } // ホストが束縛し、ゲストは詐称不能

    // 付与された能力のみ。ホストが ToolContext.caps で毎回チェックし拒否可。
    kv-get: func(key: string) -> result<option<string>, host-error>;
    kv-put: func(key: string, val: string) -> result<_, host-error>;
    http-fetch: func(url: string, body: option<string>) -> result<string, host-error>;
    enum host-error { denied, not-found, upstream-down, quota-exceeded }
}
world tool {
    import host;
    export call: func(args: string) -> result<string, string>; // JSON in/out
}
```

**ここが userId 分離の要:** `scope.user-id` は**ホストが `ToolContext` から注入**し、ゲストは触れない。`kv-get`/`http-fetch` の実装は必ず `ctx.require(Capability::Kv)?` / `require(Capability::NetOut(allowlist))?` を通す。`http-fetch` はホスト側で既存の `assertSafeOutboundUrl`（SSRF/DNSリバインド、棚卸し#1 不変条件7）を再適用。→ WASMは「ネットに出たくても、ホストが許した宛先にしか出られない」。

---

## 5. マニフェスト・権限モデル（T2/T3 共通）

拡張は**必ずマニフェストで能力を宣言**し、ホストが付与した能力のみ実行時に有効化。宣言と付与の差分は `CapabilityDenied` で落とす（制約1：名前と型のある失敗）。

```rust
#[derive(serde::Deserialize)]
pub struct ExtManifest {
    pub id: String,                 // 永続キー（リネーム禁止、moduleCatalog の id 思想を踏襲）
    pub name: String,
    pub kind: ExtKind,              // Wasm { sha256 } | Subprocess { cmd } | McpUrl { url }
    pub tools: Vec<ToolDeclaration>,
    pub requested: Vec<Capability>, // ユーザーが承認する能力
    pub requires_confirmation: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Capability {
    Kv,                             // ext_kv テーブル（下記）へ user_id スコープ
    NetOut(Vec<HostPattern>),       // 宛先allowlist（SSRFガード後）
    ReadTodos, ReadFinance,         // ホストデータの読取（細粒度・DTO経由のみ）
    // 書込系は初期は付与不可（read-only拡張から始める＝爆発半径最小）
}
```

**永続化:** 既存 `mcp_servers`（テーブル30）を一般化した `user_extensions` テーブルを追加。`user_id`(所有者)・`kind`・`manifest_json`・`granted_caps_json`・`enabled`・`wasm_blob`(BLOB, sha256検証) 列。**FKは `user_id→users CASCADE`**（棚卸し#2 の分離規約に従う）。WASM が使う KV は `ext_kv(user_id, ext_id, key, value)` PK複合 = **user_id を PK に含めて構造的テナント分離**（棚卸し#2 §5 の推奨手法）。

**マニフェスト検証の fail-fast/fail-safe 二分（制約2）:**
- WASMバイナリの sha256 不一致・WIT world 不整合 → **登録拒否**（起動時致命ではなく、その拡張だけ無効化＝縮退）。
- 実行時の trap/timeout → **その1コールだけ失敗**、`ToolOutcome::Failed` で LLM に返して会話継続。

---

## 6. supervisor とライフサイクル（制約2の実装）

T2/T3 の実体は supervisor 配下の常駐タスク。棚卸し#3 §5.3 のアクターパターンを流用。

```rust
/// 各外部拡張プロセス/WASMインスタンスプールを1アクターで監督。
/// panic/exit を JoinError で検知し指数バックオフで再spawn（制約2）。
struct ExtSupervisor {
    ext_id: String,
    backoff: ExpBackoff,            // 例: 500ms → 上限30s（redis.ts の再接続戦略に倣う）
    cancel: CancellationToken,      // graceful shutdown（SIGTERM伝播）
}
```

- **T2(サブプロセス)**: `CrawlerDaemon`/`SynapseDaemon` の「exit/EPIPEで pending を全 reject → 次リクエストで再起動」を、`tokio::task` + `JoinHandle` 監視に置換。バイナリ不在は**縮退**（`Tool` を登録しない＝そのツールが LLM に見えないだけ、他は動く）。棚卸し#5 のデグレード契約を維持。
- **T3(WASM)**: `Engine` は共有、`Store` はコール毎に新規（状態リークなし）。1つの WASM の trap は `Store` 破棄のみで supervisor は無傷。
- **fail-fast は起動時シークレット/設定のみ**（制約2）。拡張の障害は全て縮退で吸収。

---

## 7. 領域E（Gemini function-calling）との統合

`ToolDeclaration`（中立JSON-Schema）→ 領域E が Gemini `Schema` へ1方向変換。現行 `jsonSchemaToGeminiSchema`（棚卸し#4 §6）の `type`配列/`anyOf` 処理をこの変換関数に集約する。`registry.dispatch` は領域Eのループ（`runFunctionCallingLoop`）から呼ばれ、`ToolOutcome::Ok.payload` を `functionResponse` パーツへ、`Failed` を `{success:false}` JSON へ直列化。**完了ハルシネーション是正（`FunctionCallingMode.ANY`）は `ToolDeclaration.name` の allowlist をそのまま渡せる** — 拡張ツールも内蔵と同格に強制対象になる。`requires_confirmation` は宣言 description に承認注記を埋める既存動作（mcpDynamic）を全 kind へ一般化。

**tool_outcome フィードバック**（棚卸し#4 §5）も拡張ツールへ自動適用：`recordToolOutcome` は `tool_name` 文字列ベースなので、T2/T3 のツール名も同じ経験統計（2nd Hop素地）に乗る。追加実装不要。

---

## 8. 制約適合チェックリスト（機械強制）

- **制約1**: `ToolError`/`ToolFailure`/`WasmTrap`/`IpcError`/`RegistryError` は全て `thiserror`。`anyhow`/`eyre` はワークスペースから **`cargo-deny` の `[bans]` で禁止**。`#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::todo)]` を tool-core クレートに付与（非起動パスの panic を CI で機械排除）。`let _ =` は `clippy::let_underscore_must_use` で検出。
- **制約2**: 拡張障害は全て `ToolOutcome::Failed` か `Tool` 未登録（縮退）に落ち、ホストは落ちない。supervisor が指数バックオフ再起動。fail-fast は起動時シークレットのみ。
- **制約3**: T1同一プロセス・マルチスレッド、T2別OSプロセス並列、T3 は `Store`毎並列で、マルチコア活用。同期SQLite越境は `spawn_blocking`（棚卸し#2 §7A）。
- **制約4**: 3方式すべて `Arc<dyn Tool>` に収束＝**初期アーキtextにユーザー拡張が織り込まれている**。既存 MCP/デーモンは T2 として吸収。
- **制約5**: `ToolDeclaration`/DTO は Rust 単一真実源。`ts-rs`/`typeshare` で TS 生成（領域F）。拡張の I/O スキーマも同経路。

## 9. 実装順序の推奨

1. **P1**: `yuuka-tool-core`（`Tool`/`ToolContext`/`ToolError`/`ToolRegistry`）+ T1内蔵45ツール + T2で既存MCP・crawler・synapse を `SubprocessTool` に吸収。**この時点でユーザーは MCP URL 追加による拡張が可能**（制約4を実質達成）。
2. **P2**: `user_extensions`/`ext_kv` テーブル + マニフェスト/権限モデル + supervisor。ユーザー自前デーモン(T2)登録UI。
3. **P3**: T3 WASM（wasmtime + WIT）。read-only能力から段階解放。

**要点**: WASM(T3) を最後に回しても制約4は P1 で満たせる。トレイト境界（§1）だけ最初に正しく引けば、T3 は「同じ Map に別種の `Arc<dyn Tool>` を足す」作業に縮退する。これが「後付けでない」設計の本質。

---

参照して裏取りしたファイル: `/home/suki/web/kawaii-music.moe/apps/yuuka/src/types/contracts.ts`（`ToolContext`/`FunctionModule` の現契約）、`/home/suki/web/kawaii-music.moe/apps/yuuka/src/functions/registry.ts`（`buildFunctionRegistry` の重複throw・dispatch正規化）。MCP動的ツール・デーモンIPC・権限スコープの詳細は棚卸し#1〜#5 に依拠。

---

## [14] nginx is a bare volume-mount dir with no config committed yet — the reverse-prox
<sub>agentId: ac19f427b5fa7896d</sub>

nginx is a bare volume-mount dir with no config committed yet — the reverse-proxy layer must be authored as part of this migration. I have all facts needed. Writing the final design section.

---

# 領域: 移行ロードマップ + サブエージェント並行実装の分解

> 裏取り済みの前提（実ファイル確認）:
> - `src/db/database.ts` の PRAGMA は **`journal_mode=WAL` と `foreign_keys=ON` の 2 つだけ**。`busy_timeout`/`synchronous` は未設定 → 併存書き込みで即 `SQLITE_BUSY` の危険が実在する。
> - `src/db/migrations.ts` の `SCHEMA_VERSION = "17"`。`getCurrentSchemaVersion()` は `system_settings` 不在時 `"1"` を返し、**レガシー全 DROP 分岐**（`:880-894`）へ落ちる破壊経路が実在。
> - Cargo は `src/rust_crawler/Cargo.toml` と `src/rust_synapse/Cargo.toml` の **2 クレートが独立**。ルート `Cargo.toml`（workspace）は**存在しない**。
> - `/home/suki/web/kawaii-music.moe/nginx/` は **`nginx.conf` という名の空ディレクトリ**しか無い（config 未コミット、Docker volume マウント先の器のみ）。→ リバースプロキシ設定は**この移行の成果物として新規に書く**必要がある。事実として明記する。

---

## 第一部: ストラングラーフィグ移行計画

### 0. 基本トポロジ — nginx を「切替の唯一の真実源」にする

現状 nginx はプレースホルダのため、まず **ルート単位の振り分けができる nginx を器に据える**。旧 Node は `127.0.0.1:7854`、新 Rust は `127.0.0.1:7900`（別ポート）で並走させ、nginx が `location` 単位でどちらへ流すかを制御する。これがストラングラーの「絞め殺し」ダイヤルになる。

```nginx
# /home/suki/web/kawaii-music.moe/nginx/nginx.conf/yuuka.conf （新規）
upstream yuuka_node { server 127.0.0.1:7854; keepalive 32; }
upstream yuuka_rust { server 127.0.0.1:7900; keepalive 32; }

# 既定は旧 Node（フェイルセーフ: 未移行ルートは必ず既存実装へ）
map $migration_default $_ { default yuuka_node; }

server {
    listen 443 ssl http2;
    server_name yuuka.kawaii-music.moe;

    # --- 移行済みルート群は Rust へ（フェーズ進行で1行ずつ増やす） ---
    location = /api/me            { proxy_pass http://yuuka_rust; }
    location = /api/login         { proxy_pass http://yuuka_rust; }
    location ^~ /api/tasks/       { proxy_pass http://yuuka_rust; }
    # location ^~ /api/expenses/  { proxy_pass http://yuuka_rust; }  # 次フェーズで解禁

    # --- WebSocket は接続が長命なので明示 ---
    location = /ws/chat {
        proxy_pass http://yuuka_node;        # 移行末期まで Node
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_read_timeout 3600s;
    }

    # --- 既定フォールバック: 未指定は全て Node ---
    location / { proxy_pass http://yuuka_node; }
}
```

**設計判断（推奨）**: 振り分け粒度は「**プレフィックスの `location` ブロック**」に固定し、**アプリ層のフィーチャフラグや動的ルータは作らない**。理由: (a) 振り分けの状態が nginx 1ファイルに集中し監査可能、(b) ロールバックが `location` 1行を Node に戻して `nginx -s reload` で即完了（無停止・無ビルド）、(c) Rust 側にルート選別ロジックを持ち込むと「どっちが正か」が二重化する。粒度をルート群（`^~ /api/tasks/`）にするのは、1ルートずつだと本数が多すぎ（棚卸し #1 の 23 ファイル）、逆に丸ごとだと不可分になるため、**repo/機能ドメイン境界＝1フェーズ**に合わせる。

**Cookie/CSRF 上の不変条件**: 両 upstream は**同一オリジン**（同じ `server_name`・同じ TLS 終端）なので、`__Host-yuuka-session` Cookie・CORS・`Sec-Fetch-Site` 判定（棚卸し #1 §2）は upstream をまたいでも壊れない。ただし **セッションストア（Redis）を両プロセスで共有**することが絶対条件（後述 §3）。nginx は `X-Forwarded-Proto: https` を必ず立て、Rust 側の HTTPS リダイレクト判定（`checkHttps` 相当）が誤作動しないようにする。

### 1. Foundation クレート先行 — 依存グラフ

移行順序は「下流が上流に依存しない DAG」を厳守する。**foundation を最初に凍結**しないと全サブエージェントが型で衝突する。

```
                    ┌─────────────────────────────────────────┐
  Phase F (単独先行) │ yuuka-core   : error / config / secret   │
                    │ yuuka-db     : rusqlite pool / migrate    │
                    │ yuuka-types  : wire DTO + ts-rs 生成       │
                    └───────────────┬─────────────────────────┘
                                    │ (全クレートがここに依存)
        ┌───────────────┬───────────┼───────────┬───────────────┐
   Phase A          Phase B      Phase C     Phase D          Phase E
   認証/session     静的配信+     ドメイン    gemini/         discord bot
   (auth crate)     単純GET      route群      functions       (twilight)
        │            (me/status)  (並行可)     (tool trait)         │
        └────────────┴───────────┴───────────┴───────────┬───────┘
                                                          │
                                            Phase G  services/cron
                                                          │
                                            Phase H  daemon 吸収
                                                     (crawler/synapse)
```

移行の実行順（=nginx で Rust へ回すルートを増やす順）:

| # | フェーズ | 対象 | nginx で Rust へ回すもの | 前提 |
|---|---|---|---|---|
| F | foundation | error/config/secret/db/types 3クレート | （まだ何も回さない・Rust は起動だけ） | — |
| A | 認証/セッション | `resolveRequestUser`, Redis session, desktop token, device flow | `/api/login /logout /me /api/auth/device/*` | F + Redis 共有 |
| B | 静的+単純ルート | `serveStaticFile`, `/api/status`, `/api/setup/status` | 静的 `/`, `/assets/`, `/api/status` | A(user解決) |
| C | 各ルート群 | todo→finance→schedule→timeline→reminder→personal→... | ドメイン単位で `^~ /api/tasks/` 等を順次 | A,B |
| D | gemini/functions | tool registry, planner, recall注入 | （WS/bot 経路が使う内部層、HTTP は増えない） | C の repo |
| E | discord bot | twilight マルチ接続, interaction | Discord 側は Node 停止でカットオーバー | D |
| G | services/cron | reminder/report/briefing/backup/... | cron は HTTP 非公開。Node の cron を停止し Rust へ | C,D |
| H | daemon 吸収 | crawler/synapse を Rust 本体の子 or タスクへ | `/ws/chat` を Rust へ最終カットオーバー | E,G |

**なぜこの順か（推奨理由）**:
- **A（認証）を最初に回す**のは、以降の全 user-scoped ルートが `resolveRequestUser` に依存するから。ここを Rust で先に確立し、Node と Rust が**同一 Redis セッションを読める**ことを検証してからでないと、C 以降を Rust に回した瞬間にログインが割れる。
- **D（gemini）を C の後・E の前**に置くのは、tool ハンドラが C で移した repo 層（`todoRepo` 等の Rust 版）を呼ぶため。逆に bot（E）は D の tool registry に依存する。
- **H を最後**にするのは、`/ws/chat`（棚卸し #1 §6）がターンキュー・1接続1Bot束縛・添付上限という状態機械を持ち、gemini(D)+bot(E)+services(G) が揃わないと等価な応答を返せないため。WS を最後にカットオーバーするのが最も回帰リスクが低い。

### 2. Foundation の具体 — エラー/config/db 型を凍結する

#### 2.1 エラー型（`yuuka-core::error`） — anyhow 全面禁止の実装

絶対制約1に従い、**クレート境界ごとに thiserror の列挙型**を定義し、`?` で型付き伝播する。`Box<dyn Error>`/`anyhow` は `cargo-deny` の `bans` で機械的に禁止する。

```rust
// yuuka-core/src/error.rs
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("必須シークレット {0} が未設定")]
    MissingSecret(&'static str),
    #[error("暗号化シークレットが 32 文字未満（現在 {len}）")]
    SecretTooShort { len: usize },
    #[error("config.yaml の解析に失敗: {0}")]
    Parse(#[from] serde_yaml::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("SQLite ビジー（競合）: {0}")]
    Busy(String),                       // ← 回復可能（リトライ対象）
    #[error("行が見つからない: table={table}")]
    NotFound { table: &'static str },   // ← 回復可能（404 へ写像）
    #[error("スキーマ移行の中断: {0}")]
    Migration(String),                  // ← 致命（起動時 fail-fast）
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}
```

**回復可能 / 致命の型による区別**（絶対制約2）。エラー列挙型に「これは劣化縮退でよいか、fail-fast か」を**分類メソッドで内包**させ、supervisor / HTTP 層がポリシーを分岐する:

```rust
pub enum Fatality { Recoverable, FatalAtStartup }

impl DbError {
    pub fn fatality(&self) -> Fatality {
        match self {
            DbError::Busy(_) | DbError::NotFound { .. } => Fatality::Recoverable,
            DbError::Migration(_)                       => Fatality::FatalAtStartup,
            DbError::Sqlite(_)                          => Fatality::Recoverable,
        }
    }
}
```

HTTP 層は最上位で `AppError` に集約し `match` で網羅的にステータス写像する（`_ =>` ワイルドカードを禁止 → 新バリアント追加時にコンパイルエラーで検出）:

```rust
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)] Db(#[from] DbError),
    #[error(transparent)] Auth(#[from] AuthError),
    #[error("csrf 拒否")] CsrfRejected,
    #[error("ボディ超過 {0} > 10MiB")] BodyTooLarge(usize),
}
impl AppError {
    pub fn status(&self) -> http::StatusCode {
        use http::StatusCode as S;
        match self {
            AppError::CsrfRejected                        => S::FORBIDDEN,
            AppError::BodyTooLarge(_)                     => S::PAYLOAD_TOO_LARGE,
            AppError::Auth(AuthError::Unauthenticated)    => S::UNAUTHORIZED,
            AppError::Auth(AuthError::Forbidden)          => S::FORBIDDEN,
            AppError::Db(DbError::NotFound { .. })        => S::NOT_FOUND,
            AppError::Db(_)                               => S::INTERNAL_SERVER_ERROR,
        }
    }
}
```

**エラー黙殺の禁止を機械強制**（絶対制約1）。`Cargo.toml` の workspace lint と `clippy.toml`:

```toml
# ワークスペース Cargo.toml
[workspace.lints.clippy]
unwrap_used = "deny"
expect_used = "deny"
panic = "deny"
todo = "deny"
unimplemented = "deny"
let_underscore_must_use = "deny"   # let _ = fallible() を禁止
[workspace.lints.rust]
unused_must_use = "deny"
```

```toml
# deny.toml （cargo-deny）
[bans]
deny = [
  { name = "anyhow" }, { name = "eyre" }, { name = "color-eyre" },
]
```

例外は `#[allow(clippy::expand_used)]` を**起動パスの main のみ**に局所付与（config 不備の fail-fast は許可されるため）。CI で `cargo clippy --workspace -- -D warnings` と `cargo deny check bans` をゲートにする。

#### 2.2 DB 層（`yuuka-db`） — 併存書き込みの安全化

**選択（推奨）**: `rusqlite` を `tokio::task::spawn_blocking` でラップ。理由は棚卸し #2 §7A の通り — 現状の「同期・単一書き手・実行時スキーマ内省」に最も忠実で移植コストが最小。`sqlx` は動的 `ALTER`/`PRAGMA table_info` ベースの冪等移行と compile-time チェックが噛み合わない。

**両プロセスが同一 SQLite を触る期間の必須設定**（現状の `busy_timeout` 未設定が最大の地雷）:

```rust
// yuuka-db/src/pool.rs
fn open_conn(path: &Path) -> Result<Connection, DbError> {
    let c = Connection::open(path)?;
    c.pragma_update(None, "journal_mode", "WAL")?;   // 既存と一致
    c.pragma_update(None, "foreign_keys", "ON")?;    // 既存と一致
    c.busy_timeout(Duration::from_millis(5000))?;    // ★ 新規: 併存必須
    c.pragma_update(None, "synchronous", "NORMAL")?; // WAL では NORMAL が定石
    Ok(c)
}
```

**単一書き手の維持（絶対制約3のマルチコアと両立させる方法）**:
- **読み取り**: `r2d2` 等の**読み取り専用コネクションプール**（N本、WAL は複数リーダー並行可）→ `spawn_blocking` でマルチコア並列読み。
- **書き込み**: **専用の単一書き込みコネクション**を 1 本の `tokio::task`（writer actor）に閉じ込め、`mpsc` でコマンドを直列受信。これで「並行書き込み → `SQLITE_BUSY`」を構造的に排除しつつ、better-sqlite3 の単一書き手セマンティクスを保つ。マルチコアは読み側で活かす（HTTP は読みが支配的）。
- **Node 併存中の書き込み衝突**: 移行の各フェーズで「あるドメインの**書き込み**は Node か Rust の**どちらか一方のみ**」を不変条件にする。nginx でルート群を Rust に回した瞬間、そのドメインの書き込みは Rust writer actor に一本化される。**同一テーブルを両プロセスが同時に書く状態を作らない**のがフェーズ設計の肝（例: `todos` を Rust に回したら Node の todo route は死んでいる＝nginx が Node へ流さない）。`busy_timeout=5s` は移行境界の一時的競合（cron が跨ぐ等）の保険。

**schema_version の扱い（破壊経路の封じ込め — 最優先事項）**:
棚卸し #2 §7B の通り、`getCurrentSchemaVersion()` が `"1"` を返すとレガシー全 DROP へ落ちる。**Rust は既存 DB のマイグレーションを一切実行しない**方針を推奨する:

- **移行期間中、スキーマ移行の権限は Node に一本化**。Rust の `yuuka-db` は起動時に `SELECT value FROM system_settings WHERE key='schema_version'` を読み、**期待する固定値（`"17"`）と一致しなければ `DbError::Migration` で fail-fast**（絶対制約2の「本当に致命的」に該当）。Rust は DDL を発行しない＝レガシー DROP 分岐も `mcp_servers` DROP 分岐も**そもそも Rust に移植しない**。
- これにより「Rust が `system_settings` を引き継がず初期化 → 本番テーブル全 DROP」という §7B の事故が**構造的に不可能**になる。
- Node 全停止後（Phase H 完了時）に、Rust 側へ**整数連番・一方向・非破壊のマイグレーションランナー**を新規に導入し、`SCHEMA_VERSION="17"` を `schema_version=17`（整数）として引き継ぐ 1 回きりの橋渡し migration を書く。以降は Rust が権限を持つ。

```rust
// yuuka-db/src/migrate.rs （移行期間中の Rust 側チェックのみ）
const EXPECTED_SCHEMA: &str = "17";
pub fn assert_schema_compatible(c: &Connection) -> Result<(), DbError> {
    let v: Option<String> = c.query_row(
        "SELECT value FROM system_settings WHERE key='schema_version'",
        [], |r| r.get(0)).optional()?;
    match v.as_deref() {
        Some(EXPECTED_SCHEMA) => Ok(()),
        Some(other) => Err(DbError::Migration(
            format!("schema {other} != 期待 {EXPECTED_SCHEMA}（Node で移行せよ。Rust は DDL 不発行）"))),
        None => Err(DbError::Migration("schema_version 不在。空 DB への Rust 起動は禁止".into())),
    }
}
```

**`synapses.embedding` の二重書き手回避**: read-only 消費境界（棚卸し #2 §7A）を維持。移行中も synapse の SQLite への**書き込みは常に一箇所**（現状 Node、Phase H で Rust writer actor）に限定。RAM 索引の再構築（`reindex`）は read-only なので跨いでも安全。

#### 2.3 型層（`yuuka-types`） — 二重管理の構造的解消（絶対制約5）

**選択（推奨）**: `ts-rs`（`typeshare` でなく）。理由: (a) `#[serde(flatten)]` を交差型へ落とせるので現行 `ApiResponse<T> = {success} & T` エンベロープ（棚卸し #6 §1.3）を型で再現できる、(b) `#[derive(TS)]` が enum/Option/Vec を素直に写像、(c) テストとして生成（`cargo test` で `.ts` を吐く）でき CI drift チェックに乗せやすい。typeshare は CLI 別実行で flatten 表現が弱い。

```rust
// yuuka-types/src/wire.rs
#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../frontend/src/lib/api/generated/")]
pub struct Envelope<T: ts_rs::TS> {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(flatten)]
    pub data: T,
}

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct BotView {              // 機密列フィールドを「持たない」＝型で strip
    pub id: String,
    pub name: String,
    pub has_gemini_key: bool,     // 派生 bool は From<BotRecord> で計算
    pub has_token: bool,
}
```

**機密漏洩の構造的封じ込め**（現行 zod allowlist の強化版）: DB row 型（`BotRecord`, `*_encrypted`/`iv`/`tag` を持つ）と wire DTO 型（`BotView`）を**別クレート・別モジュールに物理分離**し、row 型に `Serialize` を**実装しない**（derive しない）。これで「DB レコードを誤って JSON 化」がコンパイル時に不可能になる（zod のランタイム strip より強い）。変換は `impl From<BotRecord> for BotView` に一元化。`#[serde(deny_unknown_fields)]` はリクエスト DTO 側に付与。

CI drift ゲート: `cargo test`（`.ts` 生成）→ `git diff --exit-code frontend/src/lib/api/generated/`。Rust DTO と commit 済み TS の乖離を機械検出し、現行「コメント頼み」（棚卸し #6 §1.2）を廃止。

### 3. 各フェーズの完了判定（テスト/型/verify ゲート）

各フェーズは**3 層すべて**を満たして初めて nginx を Rust へ切り替える:

1. **型ゲート**: `cargo build --workspace` + `cargo clippy --workspace -- -D warnings` + `cargo deny check bans`（anyhow 等の混入をブロック）+ `git diff --exit-code`（ts-rs 生成物 drift 無し）。
2. **テストゲート**: `cargo test --workspace`。各ルート群はゴールデンテスト —— **同一リクエストを Node(:7854) と Rust(:7900) の両方へ投げ、レスポンス JSON（success エンベロープ・キー名・snake/camel）とステータスが一致**することを assert する差分テストを書く（棚卸し #6 §1.2 のキー名乖離を移行時に固定）。
3. **verify ゲート**: `deploy/instance.sh` の verify（棚卸し #6 §4）を流用。CSP `script-src 'self'`・hashed asset の immutable・`/api/me` 200 を curl 検証。これは**サーバ実装非依存**なので Rust カットオーバー後の回帰検出にそのまま効く（棚卸し #6 §5.3）。加えてフェーズ固有の不変条件（棚卸し #1 §7 の該当項目 —— 例: todo フェーズなら CSRF 403・10MB 413・`stripProtoKeys`・user_id スコープ越境が 403）を verify スクリプトに追記。

**ロールバック基準**: verify が落ちたら nginx の該当 `location` を `yuuka_node` に戻し `nginx -s reload`（無停止）。DB は Rust が書いた行も Node が読めるスキーマ互換なので、書き込みドメインを Node に戻しても整合する（§2.2 の schema 固定が効く）。

---

## 第二部: サブエージェント並行実装のワークフロー分解

### 4. 契約先行凍結 — エージェント衝突を型で消す

並行化の前提は「**モジュール間の境界（trait/型）を Phase 0 で凍結し、以後変更しない**」こと。凍結対象は 3 つ:

1. **`yuuka-core::error` の列挙型**（§2.1）— 全クレートが `#[from]` で依存。
2. **`yuuka-types` の wire DTO**（§2.3）— HTTP とフロントの契約。
3. **`Tool` トレイト**（棚卸し #4 §7B）— gemini/functions と全ドメインモジュールの契約:

```rust
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn declaration(&self) -> FunctionDeclaration;
    async fn call(&self, ctx: &ToolContext, args: serde_json::Value)
        -> Result<ToolOutcome, ToolError>;   // 文字列規約を型へ昇格
}
pub struct ToolOutcome { pub status: ToolStatus, pub payload: serde_json::Value,
                         pub attachments: Vec<Attachment> }   // embeds/files を戻り値へ
pub enum ToolStatus { Success, Failure }
```

さらに **Repo トレイト**を凍結し、user_id スコープを型で強制（棚卸し #2 §7 補足の弱点＝手書き WHERE を廃す）:

```rust
pub struct UserScope { user_id: DiscordId }   // 構築時に user_id を必ず束縛
impl TodoRepo {
    // 全メソッドが &UserScope を要求 → user_id 無しクエリを型で禁止
    async fn list(&self, scope: &UserScope) -> Result<Vec<Todo>, DbError>;
}
// AcrossUsers 系だけを別トレイトに隔離（cron 専用）
pub trait CronScan { async fn overdue_across_users(&self) -> Result<Vec<Todo>, DbError>; }
```

これで棚卸し #2 §5 の v7 クロステナント事故（`bot_mcp_access` の owner_id 欠落）が型レベルで再発不能になる。

### 5. 独立（並行可）vs 逐次（foundation 先行）の分解

| 並行性 | クレート/モジュール | 根拠 |
|---|---|---|
| **逐次（最初に単独）** | `yuuka-core`(error/config/secret), `yuuka-db`(pool/scope trait), `yuuka-types`(wire+ts-rs) | 全員が依存。ここが動くまで他は着手不可 |
| **並行 T1**（互いに独立、Repo trait 実装が別テーブル） | todo / finance / schedule / timeline / reminder / personal / credential / playbook / persona | 各ドメインは**別テーブル・別 route ファイル**（棚卸し #1）で、共有は foundation のみ。DTO と Tool 実装を各エージェントが担当 |
| **並行 T2**（横断だが T1 と独立） | auth/session, 静的配信, admin, webhook, MCP proxy | セッション・静的・監査は T1 のドメイン repo に依存しない |
| **逐次（T1 の後）** | gemini orchestrator, functions registry マージ, MCP dynamic | T1 の Repo/Tool 実装を集約する。registry は全 Tool を集める合流点 |
| **逐次（gemini の後）** | discord bot(twilight), WS chat, componentInteraction 統合 | tool registry と gemini エントリに依存 |
| **並行 T3**（bot と独立に進む） | services/cron(reminder/report/briefing/backup/…), notifier | Repo(T1)+notifier に依存するが gemini 内部には依存しない（report/briefing は `generateAuxText` のみ）。bot と並行可 |
| **逐次（最後）** | daemon 吸収（crawler/synapse を workspace 化） | 既存 Rust クレートを workspace に統合 |

**T1 の 9 ドメインが最大の並行度**。各ドメインは「Repo trait 実装 + wire DTO + Tool 実装 + route handler + ゴールデンテスト」を 1 エージェントが縦に持つ。境界は foundation の凍結型のみなので衝突しない。

### 6. worktree 分離での衝突回避（メモリの落とし穴を踏まえて）

**重要**: user の auto-memory `worktree-node-modules-symlink-pitfall`（worktree に node_modules symlink + `git add -A` で本体破壊）と `develop-no-history-delete`（move で引越し、copy 禁止）を厳守する。

- **workspace 化を Phase 0 で先に確定**: ルート `Cargo.toml`（現状不在）を `apps/yuuka/Cargo.toml` に新設し、`[workspace] members = ["crates/*", "src/rust_crawler", "src/rust_synapse"]` とする。**`Cargo.lock` はルート 1 つ**に集約。これを Phase 0 の最初のコミットにし、全 worktree がこの lock を共有。
- **エージェントごとに `git worktree` を分離**。ToolSearch 経由の `EnterWorktree` で各エージェントを別 worktree に隔離し、`crates/todo/`・`crates/finance/` のように**クレート境界＝ディレクトリ境界＝所有境界**を一致させる。1 クレート 1 worktree 1 エージェントなら、編集ファイルが物理的に交わらない。
- **共有ファイルの編集を Phase 0 に隔離**: `Cargo.toml`（members 追加）、`yuuka-core`、`yuuka-types` の凍結後は**触らせない**。members 追加のような共有編集が並行フェーズに漏れると衝突する → Phase 0 で全クレートの空スケルトン（`lib.rs` に `todo!()` 無しの最小 stub とトレイト impl の空実装）を先に生成してコミットし、以後エージェントは**自クレート内のファイルのみ**を編集する。
- **node_modules は worktree に持ち込まない**: Rust クレートは node_modules 不要。frontend の ts-rs 生成物だけは `frontend/src/lib/api/generated/` に出力されるが、これは生成専用ディレクトリで、frontend ビルドの worktree とは分離する。`git add -A` は**使わず**、常に明示パス `git add crates/todo/`。

### 7. モジュールごとの検証ゲート（cargo test/clippy）

各エージェントは自クレートについて以下を**マージ前に全通過**させる（ローカル worktree で完結）:

```
cargo build -p yuuka-todo
cargo clippy -p yuuka-todo -- -D warnings          # unwrap/expect/panic/todo を deny
cargo test  -p yuuka-todo                          # unit + golden(Node差分) + scope 越境テスト
cargo deny check bans                              # anyhow 混入ブロック（workspace 全体）
git diff --exit-code frontend/.../generated/       # ts-rs drift（DTO 変更時のみ）
```

**クレート単位の `-p` 指定**が並行性の鍵 —— エージェントは他クレートの未完成を待たずに自分のゲートを回せる（foundation は Phase 0 で確定済みなので依存は満たされている）。CI は最後に `cargo test --workspace` で統合を 1 回検証。

### 8. 具体的ワークフローのフェーズ設計

```
Phase 0  契約凍結（単一エージェント・逐次・最重要）
  - apps/yuuka/Cargo.toml workspace 新設、Cargo.lock 集約
  - crates/core（error 列挙型 + Fatality + config + secret）
  - crates/db（rusqlite pool + writer actor + UserScope trait + assert_schema_compatible）
  - crates/types（wire DTO + Envelope + ts-rs export + drift test）
  - Tool / Repo トレイト凍結、全クレートの空 stub コミット
  - clippy.toml / deny.toml / workspace lints を配置
  ゲート: cargo build --workspace, clippy -D, deny check, ts-rs 生成が空でも通る
  → ここまで 1 エージェント。以後の型は FROZEN。

Phase 1  並行ファンアウト（サブエージェント N 体）
  T1: [todo][finance][schedule][timeline][reminder][personal][credential][playbook][persona]
       各 = Repo impl + DTO + Tool impl + route handler + golden test（別 worktree）
  T2: [auth/session][static+admin][webhook][mcp-proxy]（別 worktree）
  T3: 着手可（services/cron の repo 依存が T1 で埋まり次第）
  各エージェントのゲート: §7 の -p 単位チェック
  合流: Phase 1 完了ドメインから nginx location を :7900 へ 1 群ずつ解禁
       （§3 の 3 層ゲート通過 → location 追加 → verify → 次へ）

Phase 2  合流（逐次）
  gemini orchestrator + functions registry マージ（全 Tool 収集、重複名 Result で検知）
  MCP dynamic（Arc<dyn Tool> 動的マージ）
  ゲート: registry 起動時の重複名テスト、planner responseSchema テスト

Phase 3  bot + WS（逐次、supervisor 前提）
  twilight マルチ接続（ArcSwap でトークン差替、per-bot アクター）
  WS chat（ターンキュー・1接続1Bot束縛・添付上限）
  各 bot タスクを supervisor 配下で catch + 指数バックオフ再起動（絶対制約2）
  カットオーバー: Node bot 停止 → Rust bot 起動（Discord 側は排他）

Phase 4  services/cron カットオーバー（T3 完成品を投入）
  Node の cron 停止 → Rust cron 起動（reminder は起動時即時実行で §10 復帰）

Phase 5  daemon 吸収 + 最終カットオーバー
  crawler/synapse を workspace member 化（既にRust）
  推奨: 別 OS プロセスのまま supervisor が監督（棚卸し #5 §5 の障害分離を維持）
  /ws/chat を nginx で :7900 へ最終切替 → Node 全停止
  Rust 側に整数連番マイグレーションランナー導入、schema_version=17 橋渡し
```

### 9. supervisor パターン（全 Phase 共通の自己復帰基盤・絶対制約2）

長命サービス（bot 各タスク・cron・daemon）は例外なく supervisor 配下に置く。panic をタスク境界で隔離し、指数バックオフで再起動する:

```rust
// crates/core/src/supervisor.rs
pub async fn supervise<F, Fut>(name: &'static str, cancel: CancellationToken, mut make: F)
where F: FnMut() -> Fut, Fut: Future<Output = Result<(), ServiceError>> + Send + 'static {
    let mut backoff = Duration::from_millis(200);
    loop {
        if cancel.is_cancelled() { return; }
        let h = tokio::spawn(make());                 // panic をタスク境界で隔離
        match h.await {
            Ok(Ok(()))  => backoff = Duration::from_millis(200),   // 正常終了→即再開
            Ok(Err(e))  => log_service_error(name, &e),            // 型付きエラー、黙殺しない
            Err(join)   => tracing::error!(%name, is_panic=join.is_panic(), "task died"),
        }
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = cancel.cancelled()          => return,
        }
        backoff = (backoff * 2).min(Duration::from_secs(30));      // 上限 30s
    }
}
```

- **`panic = "unwind"`** をリリースプロファイルに設定（`abort` だと 1 タスク panic がプロセスを落とす）。これで棚卸し #3 §5.3 の「uncaughtException=状態破損→当該アクターのみ再起動」を**プロセス全体でなくアクター単位**に狭める。
- **graceful shutdown**: `CancellationToken` を全タスクへ伝播、`tokio::time::timeout(Duration::from_secs(15), join_all(...))` で watchdog（現行 index.ts の 15 秒 unref setTimeout と等価だが `select!` で構造化）。
- **外部依存の劣化縮退**: Gemini/Discord/Google/Redis/DB の一時障害は各 Repo/Client が型付き `Err(...Recoverable)` を返し、呼び出し側が「直近履歴のみ」「memory フォールバック」等へ縮退（棚卸し #2 §6・#4 の synapse デグレードと同型）。**起動時のシークレット不備のみ**（`ConfigError::MissingSecret`）が fail-fast で `process::exit(1)` 相当。

### 10. 拡張性の初期織り込み（絶対制約4）

`Tool` トレイト（§4）が既にプラグイン境界。将来の user カスタムモジュールは:
- **同一 `Arc<dyn Tool>` を registry に動的マージ**（MCP dynamic と同じ配管、棚卸し #4 §6）。
- **改行区切り JSON デーモン契約**（crawler/synapse）を `trait DaemonPlugin` として一般化し、`{id, command, ...} → {id, ok, result: Value, error}` の統一フレーム（棚卸し #5 §5 の推奨統一形）で任意の外部 Rust/他言語プロセスを差し込めるようにする。`Embedder` トレイト（synapse の onnx feature ゲート）と同じ「トレイト+差し替え実装」パターンを踏襲。
- これを**後付けでなく Phase 0 の Tool/Daemon トレイト凍結時に境界として確定**する。

---

**最重要の 3 点（この領域の結論）**:
1. **nginx location 単位の絞め殺し**（アプリ層フラグ不採用）で、切替状態を 1 ファイルに集中させロールバックを `reload` 1 発にする。
2. **schema_version を Rust では固定値チェックのみ・DDL 不発行**にし、レガシー全 DROP 破壊経路を移植せず封じる。書き込みドメインは常に Node/Rust の一方に限定し `busy_timeout=5s` を保険にする。
3. **Phase 0 で error 列挙型・wire DTO・Tool/Repo/UserScope トレイトを凍結**してから T1 の 9 ドメインをサブエージェント並行展開する。凍結型 + 1クレート1worktree + `-p` 単位ゲート + `cargo deny`(anyhow 禁止) が、衝突ゼロ・絶対制約準拠を機械保証する。

---

## [15] I have enough grounding. I've confirmed: multi-bot runs as multiple `discord.js`
<sub>agentId: a625b52f91c36a37b</sub>

I have enough grounding. I've confirmed: multi-bot runs as multiple `discord.js` `Client` instances in one process (`customClients` Map), better-sqlite3 is **synchronous** (WAL, foreign_keys ON), userId scoping is per-query in WHERE clauses, and the synapse daemon already carries scope. Now I'll write the critique.

# 厳格エラーアーキテクチャ・ドラフト — 辛口反証レビュー

裏取り済みの事実: マルチボットは**単一プロセス内の複数 `discord.js` Client インスタンス**(`src/bot.ts:81` `customClients = new Map<string, Client>()`)、DB は**同期API** better-sqlite3(WAL/foreign_keys ON)、userId 分離は**各リポジトリのSQL WHERE句**で担保、synapse は既に `Scope{user_id,bot_id,guild_id}` を持つ。以下、計画の成否に関わる論点を優先度順に。

---

## A. 絶対制約の充足性(制約1・2)

### A-1. 【確信度:高】`let _ = ...(` の grep ゲートは誤検知だらけで、逆に本物の黙殺を素通しする — 機械的強制が破綻する
**どこ:** §5.3 の
```bash
grep -rEn 'let _ = [a-zA-Z_].*\(' --include='*.rs' crates/ | grep -v '// intentional-ignore:'
```
**なぜ問題か:**
- **誤検知:** `let _ = tx.send(msg);`(mpsc の意図的送信破棄)、`let _guard = mutex.lock()`(guard 束縛)、`let _ = writeln!(...)` 等、Rust では `let _ =` は正当な用途が非常に多い。CI が常時赤になり、開発者は `// intentional-ignore:` を機械的に貼るようになる → ゲートが**形骸化**する。
- **本物の黙殺は捕捉できない:** 制約1が本当に禁じたい黙殺は `let _ = fallible();` ではなく `.ok()` / `.unwrap_or_default()` / `if let Ok(x) = ...` で Err 側を書かない形。正規表現 `let _ = ...(` はこれらを1つも捕まえない。`res.ok();`(セミコロン単独)も漏れる。
- **`map_err_ignore = "warn"`** と矛盾: warn では CI(`-D warnings` は clippy 本体のみ、この lint は warn 指定)で落ちない。deny にしていない。

**どう直すか:**
1. grep をやめ、clippy の `let_underscore_must_use` / `let_underscore_future` / **`let_underscore_untyped`** を deny に昇格し、`#[must_use]` を返す関数(`Result` は自動的に must_use)の破棄をコンパイラ/clippy に検出させる。`Result` の `let _ =` 破棄は `let_underscore_drop`+`must_use` 経路で機械検出可能。
2. 「捨ててよい」は §3.1 の名前付きヘルパー(`ignore_cache_miss`)に**一本化**し、grep 対象を `.ok()` と `.unwrap_or_default()` の直後に `Result` が来る形へ変える(ただし後述の通り AST 無しでは限界)。正確を期すなら `dylint` でカスタム lint を書くのが本筋。grep ゲートは「あれば安心」の飾りで、**制約1の機械的強制の中核を担わせてはいけない**。

### A-2. 【確信度:高】`indexing_slicing = "deny"` は better-sqlite3 相当のホットパスと衝突し、大量の `#[allow]` を生む → §5.3 の allow 上限5と正面衝突
**どこ:** §5.1 `indexing_slicing = "deny"` と §5.3 `allow_count > 5 で fail`。
**なぜ問題か:** synapse の `index.rs`(ベクトル演算)や埋め込み器は `vec[i]`, `slice[a..b]` を多用する数値カーネルだ。`indexing_slicing` を deny にすると、これら全てを `.get(i).ok_or(...)` に書き換えるか `#[allow]` を貼るしかない。ベクトル内積のループで `.get()` を毎回呼べば**制約3(現行より高速)に反する**。結果、数十箇所の `#[allow(clippy::indexing_slicing)]` が必要になり、**allow上限5を即座に突破**して CI が恒久的に赤くなる。設計内で2つのゲートが互いに矛盾している。
**どう直すか:** `indexing_slicing` はワークスペース全体 deny をやめ、(a) HTTP/認可/DB など**信頼境界のクレートのみ** deny、(b) 数値カーネル(synapse/embedder)は `allow` をクレート単位で明示、という**層別ポリシー**にする。allow カウントも「クレート別上限」または「新規追加禁止(ベースライン比較)」に変える。一律5は数値計算クレートを1つ入れた瞬間に破綻する。

### A-3. 【確信度:高】supervisor が `Permanent` を無限リトライする — CPU 100% スピンと「常時稼働」の自壊
**どこ:** §7 `run_supervised` の
```rust
Retryability::Transient | Retryability::Permanent => { ... sleep(backoff); backoff *= 2 }
```
**なぜ問題か:** `Permanent`(検証失敗・設定ミスに近い恒久障害)をタスクが返し続けると、supervisor は **500ms→…→30s で永久にリトライ**する。恒久障害は定義上「同じ入力で再試行は無意味」(§2.1 のコメント自身がそう書いている)なのに、リトライループに入れている。設計コメントとコードが矛盾。長時間走るサービス(cron/webhook processor)が構成不変条件の誤りで `Permanent` を吐き続けると、30秒ごとに同じ失敗を延々ログに吐き、**自己復帰ではなく自己ループ**になる。observability も汚染される。
**どう直すか:** `Permanent` は「そのタスク単位は諦める(circuit open にして次のトリガーを待つ)」か「N回連続 Permanent で当該サービスを degraded 状態へ落とし管理UIへ通知」に分岐すべき。3値の `Retryability` を supervisor 動作へ素朴にマップしたのが誤り。少なくとも `Transient`(即バックオフ再起動)と `Permanent`(バックオフ上限に達したら停止 or アラート)は**別扱い**が必要。

### A-4. 【確信度:中】「稼働中に Fatal は型的に存在しない」は runtime の CryptoError で破れる
**どこ:** §2.1「`ConfigError` と `CryptoError`(起動時鍵検証)だけが `Fatal` を返しうる」「稼働中に Fatal が湧く経路を型レベルで存在させない」。
**なぜ問題か:** `src/utils/crypto.ts:240` は稼働中も復号を行う(暗号化トークン/Gemini鍵の復号は**リクエスト毎**)。鍵ローテーション中や DB 破損で**稼働中に**復号が失敗しうる。この失敗が `CryptoError::Fatal` に写像されると、§7 の supervisor が `std::process::exit(1)` を呼び、**1ユーザーの1トークンの復号失敗でプロセス全体が落ちる** — 制約2「些細な障害では絶対に落ちない」の真逆。「Fatal は起動時のみ」という不変条件は、同じ CryptoError 型を起動と稼働の両方で使う限り**型では保証されない**(コメントによる紳士協定)。
**どう直すか:** `CryptoError` を**起動時鍵検証用**(`KeyValidationError: Fatal 可)と**稼働時復号用**(`DecryptError`: 常に `Permanent`、劣化縮退=当該ボット停止/管理UI通知)に**型で分割**する。同じ enum に Fatal と非Fatal を混在させると「起動時のみ Fatal」の不変条件が破れる。

### A-5. 【確信度:中】IPC の `Handler(Box<dyn ClassifyError + Send + Sync>)` は object-safe にならない可能性
**どこ:** §8。`ClassifyError: std::error::Error` を trait object 化。`code(&self) -> &'static str`、`public_message(&self) -> Cow<'static,str>` は object-safe だが、`std::error::Error` のスーパートレイト境界と `Send + Sync` 併用、そして各層エラーが `From` で吸い上げる際に `Box<dyn ClassifyError>` へアップキャストする経路は `?` では自動で効かない(`From<ConcreteErr> for Box<dyn ClassifyError>` を手書きする必要)。「`?` の人間工学を保つ」という主張と相性が悪い。加えて `dyn ClassifyError` からは `source()` チェーンや downcast が失われ、境界での分類分岐が痩せる。
**どう直すか:** IPC ハンドラのエラーは trait object ではなく**具体 enum `IpcHandlerError`(全ハンドラ結果の和)**にする。`Box<dyn>` は制約1の趣旨(全失敗経路に名前と型)に照らして、たとえ `ClassifyError` 限定でも後退。ここだけ例外を作る合理性が薄い。

---

## B. 技術的正当性(制約3含む)

### B-1. 【確信度:高】DB を async に載せる罠が未解決 — rusqlite は同期・現行 better-sqlite3 も同期。`spawn_blocking + backoff` の Busy リトライは書き込みロックを悪化させる
**どこ:** §2.2 コメント「`DbError::Busy → Transient` 経路 + 呼び出し側の backoff で型的に扱える」、`spawn_blocking + backoff で再試行`。
**なぜ問題か:**
- rusqlite は同期。axum(tokio)から使うなら全DB呼び出しを `spawn_blocking` で包むか、コネクションプール(r2d2/deadpool)+専用スレッドが要る。この**基盤選択が本ドラフトに無い**。「呼び出し側の backoff」だけでは設計が成立しない。
- **SQLITE_BUSY を backoff リトライで解くのは半分間違い**。現行が WAL(`journal_mode=WAL`)である以上、正しい第一手は `busy_timeout` PRAGMA(SQLite が内部で待つ)であって、アプリ層リトライではない。棚卸し#2の指摘も本来 `busy_timeout` 設定漏れが原因。さらに WAL でも**writer は単一**なので、複数スレッドから同時書き込みを投げて busy→backoff を繰り返すと、公平性が無く**ライブロック/レイテンシ悪化**を招く。マルチスレッド化(制約3)が逆効果になりうる。
**どう直すか:** (1) 書き込みは**単一 writer コネクション + チャネル直列化**(または `deadpool-sqlite`)を明示し、(2) `busy_timeout` を PRAGMA で設定して SQLite 内部待機に任せ、(3) アプリ層 backoff は timeout 超過後の最終手段に限定、と設計へ**明記**する。`Busy→Transient→backoff` だけでは制約3の「堅牢かつ高速」を満たせない。

### B-2. 【確信度:高】型生成(制約5)がドラフトに**存在しない** — 領域外扱いだが本領域が最大の依存元
**どこ:** ドラフト全体。制約5(単一真実源=Rust から Svelte 型を自動生成)への言及がゼロ。
**なぜ問題か:** `apiViews.ts` の zod allowlist(機密 strip フェイルクローズ)を、本ドラフトは「§4 の `public_message()` がエラー版フェイルクローズだ」と主張するが、これは**エラー本文**の話であって、**成功レスポンスの機密列 strip** を1ミリも代替しない。`botViewSchema` が `discord_token_encrypted` 等を落とす機構(制約5・制約3の核)は本領域の `HttpError` とは別レイヤで、しかも `ClassifyError::public_message` の設計と**同じ「fail-close allowlist」思想を共有すべき**なのに接続が示されていない。「二重管理と機密漏洩を構造的に解消」という制約5は、成功パスの DTO 生成(ts-rs / specta / typeshare のいずれか)を決めないと未達。
**どう直すか:** 本領域は「エラーの public_message allowlist」と「成功 DTO の allowlist」を**同一の serde/型生成パイプラインで一体設計**すべき。最低限、どのクレート(`yuuka-http` の View 型)が型生成の真実源になり、`ts-rs`/`specta` のどれで Svelte 型を吐くかをドラフトに含める。現状これが欠けたまま「zod のエラー版」だと言い張っているのは論点のすり替え。

### B-3. 【確信度:中】`ClassifyError: std::error::Error` を全層必須にすると thiserror + 手書き impl の二重負担で、実質「全 enum に 3 メソッド手書き」を強制
**どこ:** §2.1 trait、§2.2〜2.4 の各 impl。
**なぜ問題か:** 12層(config/crypto/db/redis/auth/validate/gemini/discord/google/plugin/ipc/http)それぞれで `retryability`/`public_message`/`code` を**手書き match** する。ヴァリアント追加のたびに3つの match を全て更新(コンパイラは網羅性で `retryability`/`code` の漏れは捕まえるが、`public_message` に**新ヴァリアントの機密を誤って含める**のはコンパイラでは防げない)。§4 が謳う「機密は型的に到達不能」は、`public_message` の実装者が新ヴァリアントで内部詳細を返した瞬間に破れる。**型ではなく規律**に依存している。
**どう直すか:** `public_message` は「デフォルトで固定文字列、明示的に公開したいヴァリアントだけ override」を**マクロ or デフォルト実装**で担保し、"デフォルト非公開" をコンパイラ側に寄せる。あるいは `#[classify(retry = transient, code = "db.busy", public = "not found")]` の derive マクロを自作し、手書き match の抜け穴(§4-2 の主張)を実体化する。現状は「フェイルクローズ」と言いつつ実装は fail-open(書き忘れたら Display が漏れうる設計ではないが、public に内部値を入れる自由がある)。

### B-4. 【確信度:中】マルチボットは単一プロセス複数 Client(twilight 前提と現実の乖離)
**どこ:** §2 `yuuka-discord … DiscordError (twilight gateway/http)`。
**なぜ問題か:** 現行は `customClients: Map<string, Client>` で**1プロセス内に多数の gateway 接続**を張る(ユーザーごとのカスタムボット)。twilight への移行自体は妥当だが、本ドラフトの supervisor(§7 `run_supervised(name, task)`)は**サービス1個=タスク1個**を想定しており、**N個の動的に増減するボット接続**をどう supervise するか(ボット追加/削除/トークン更新 destroy→login のレース、`src/bot.ts:1316` が既に苦労している領域)が `DiscordError` と supervisor の接続として示されていない。1ボットの gateway 切断が `Transient` として supervisor に上がったとき、**そのボットだけ**再接続するのか**全ボット**なのかが型に無い。
**どう直すか:** supervisor 契約を「名前付き単一タスク」から「動的な子タスク集合(per-bot supervisor tree)」へ拡張し、`DiscordError` に**どのボットか**(bot_id)を載せる。1ボットの障害隔離が現行の暗黙不変条件(1ボット落ちても他は生きる)であり、これを型で保証しないと制約2を後退させる。

---

## C. 既存不変条件の完全性(制約 — 落としていないか)

### C-1. 【確信度:高】userId データ分離が本領域の視界に無い — `NotFound` と `Forbidden` の分離が漏洩に化ける
**どこ:** §1「`NotFound` と `Forbidden` を分けるのは…」、§2.2 `DbError::NotFound`。
**なぜ問題か:** 現行の分離は**SQL の `WHERE user_id = ?`**(`botRepo.ts:84` 等)で担保され、他人のリソースは**そもそも行が返らない=NotFound**になる。ここで設計が `NotFound`(404)と `Forbidden`(403+監査ログ)を「行動が違うから分ける」と推奨しているが、**他人のリソースへのアクセスは 403 ではなく 404 を返すのが正しい**(存在秘匿)。設計例の「Forbidden は監査ログを打つ」を素直に実装すると、開発者が「アクセス不可=Forbidden」と考えて **404 であるべき箇所を 403 にし、リソースの存在を漏らす**リスクがある。本領域は userId 分離という最重要不変条件に一度も触れていない。
**どう直すか:** ドラフトに「所有権不一致は原則 `NotFound`(存在秘匿)、`Forbidden` は認証済みだが権限不足の限定ケース」という**方針**を明記。`DbError::NotFound { entity }` を返す repo が **user_id スコープ済みクエリの結果**であることを型/命名で担保(例: `ScopedRepo` パターン)。制約1の「呼び出し側が判別」を、セキュリティ不変条件と衝突しない形に落とす必要がある。

### C-2. 【確信度:中】CSP/CSRF のうち **CSP** が完全に不在、CSRF も判定材料の型化どまり
**どこ:** §2.4 `AuthError::CsrfRejected`(Sec-Fetch-Site/Origin/Referer)。CSP への言及ゼロ。
**なぜ問題か:** 制約3(現行不変条件)は CSRF/CSP 両方を含む。CSRF はエラー型に載ったが、**CSP ヘッダ付与はエラー領域の話ではない**ため、この領域に無いのは一応妥当。ただしドラフトの制約チェック節は「CSRF/CSP を落としていない」と主張していないので誤りではない。問題は CSRF 判定の**フェイルクローズ**性: `CsrfRejected` を返す条件(Sec-Fetch-Site 欠如時にどうするか)が型に無く、現行の「ヘッダ欠如=拒否(fail-close)」が保たれるかがコード規律依存。
**どう直すか:** CSP はこの領域の対象外と**明言**(別領域=HTTP ミドルウェア領域へ委譲)。CSRF は「判定不能(ヘッダ欠如)は `CsrfRejected` に倒す」ことを型のコメントではなく**判定関数のデフォルト**に埋め、fail-close をテストで固定。

### C-3. 【確信度:中】劣化縮退を `CacheError` **のみ**に許すルールは Gemini/Google/Discord の縮退と矛盾
**どこ:** §3.3「`CacheError` **だけ**が warn してから続行を許される層」。
**なぜ問題か:** 制約2は「Gemini/Discord/Google/DB/Redis の一時障害を劣化縮退で吸収」と明記。ところが §3.3 は縮退を Cache だけに限定し「他層で同じパターンが出たら code-review で弾く」とした。しかし `GeminiError::NoApiKey`(§2.3 コメント「管理UIへ誘導(劣化縮退)」)や Google 障害時のカレンダー機能スキップは、**まさに Cache 以外の縮退**だ。設計内で「縮退は Cache 限定」と「Gemini は縮退する」が**自己矛盾**している。
**どう直すか:** 縮退の許可を「層」ではなく「**呼び出し文脈**」で定義する。`retryability()==Transient` かつ「その機能が任意(optional)である呼び出し点」でのみ縮退可、と規約化。Cache 限定は狭すぎて制約2を実装できない。縮退ヘルパー(§3.1 の名前付き関数)を層横断で用意し、grep 可能にする方が制約2と整合する。

---

## D. 移行実現性(制約4含む)

### D-1. 【確信度:高】段階移行の**並行稼働**が本ドラフトに無い — Node と Rust が同一 SQLite を同時に開く整合性が未設計
**どこ:** ドラフト全体(移行対象と言うのみ)。
**なぜ問題か:** synapse/crawler は既に Rust だが、**本体バックエンドは TS**。全面書き換え中は Node(better-sqlite3, WAL)と Rust(rusqlite, WAL)が**同じ `yuuka.db` を同時に開く**期間が必ず発生する。WAL の複数プロセス writer、`foreign_keys` PRAGMA の各接続設定、`busy_timeout` の両側整合、migration の所有権(どちらが migrations.ts の 1400 行超を実行するか)が全て未設計。§2.2 の `DbError` はこの並行期の**別プロセス由来の Busy 頻発**を想定していない。
**どう直すか:** 移行戦略として (a) DB を Rust 側の1プロセスに所有させ Node はIPC経由、または (b) 機能単位で**テーブル所有を分割**し同時 writer を避ける、のどちらかを決める。migration の二重実行防止(バージョン所有)も必須。「既存2クレートを配下に組み込む」だけでは本体移行の並行期を乗り切れない。

### D-2. 【確信度:中】拡張性(制約4)= プラグイン/カスタムモジュールの**動的ロード**とエラー型の静的網羅が原理的に衝突
**どこ:** §3.2「全 enum に `#[non_exhaustive]` を付けない=網羅性最大化」vs 制約4「user が独自カスタムモジュールを追加」。
**なぜ問題か:** ユーザー独自モジュールが**独自の失敗種別**を持つのは必然だが、`PluginError` を `#[non_exhaustive]` 無しの閉じた enum にすると、新プラグインのエラーを**本体のコンパイル時 enum に追加**しないと表現できない=ユーザー拡張が本体改変を要する。逆に §8 のように `Box<dyn ClassifyError>` を許すと、今度は「全失敗に名前と型」「網羅 match」の制約1が緩む。**この2制約は根本的にトレードオフ**で、ドラフトはこれを直視していない(§8 で IPC だけ trait object を例外にしているが、プラグイン一般には未適用)。
**どう直すか:** プラグイン境界は「本体 enum に `PluginError::Custom { plugin_id, code: &str, retry: Retryability, public: Cow }`」という**構造化された1ヴァリアント**にし、動的部分をデータ(文字列 code+分類)として持つ。trait object を避けつつ拡張を許す。`#[non_exhaustive]` は `AppError` と `PluginError` に**限定的に許可**する方が制約1と制約4を両立できる(§3.2 の「AppError のみ許容」を PluginError にも拡張)。

### D-3. 【確信度:低】WASM サンドボックスの言及ゼロ(任務が問うているのに設計に無い)
**どこ:** 任務は「WASMサンドボックス」の罠を問うが、ドラフトはプラグインを `PluginError (tool dispatch・MCP)` としか書かず、**実行隔離の手段(WASM/wasmtime/別プロセス)を示さない**。
**なぜ問題か:** ユーザー独自モジュール(制約4)を**同一プロセスで動的ロード**すると、制約1が禁じた `panic!`/`unwrap` をプラグインが持ち込んで supervisor で隔離しきれない(catch_unwind は UB を防げない、FFI 境界での abort もある)。WASM(wasmtime)なら trap を `Result` に変換でき制約1と親和的だが、**その選択がドラフトに無い**。
**どう直すか:** プラグイン実行を wasmtime の Result 化 trap 隔離に載せ、`PluginError::Trap` として型化する、という方針を追記。少なくとも「同一プロセス dylib ロードは制約1の panic 禁止を破るので採らない」という判断を明記すべき。

---

## 妥当と認める点(反証できなかった部分)

- **`#[from]` を使わず手書き `From<rusqlite::Error>` で Busy/Constraint を分類する**(§2.2)は正当かつ的確。`#[from]` の丸呑みが分類を消すという指摘は正しい。**確信度:高で妥当**。
- **`retry_after: Option<Duration>` を `GeminiError::RateLimited` に型で載せる**(§2.3)は、現行 `classifyError` の正規表現分類(`/rate|429|quota|制限/i`, chatChannelService)より確実に堅牢。**妥当**。
- **ログは境界で1回・発生点は `?` 伝播のみ**(§3.3)の原則自体は正しい(二重ログ回避)。ただし縮退の層限定(C-3)は誤り。
- **`[workspace.lints]` で一元管理 + `[lints] workspace=true` 継承**(§5.1, Rust 1.74+)は実在し妥当。cargo-deny の `[bans].deny` で anyhow/eyre を直接依存禁止するのも正しい使い方。**妥当**。
- **`ClassifyError::code()` を安定文字列コードにしてログ相関に使う**設計は、フロント型連携(制約5)とも将来整合しうる良い抽象。ただし B-2 の型生成が無いと片肺。

---

## 総括(計画の成否に関わる Top 5)
1. **B-1/D-1(DB async & 並行移行)** — rusqlite 同期↔axum async の基盤、WAL 単一 writer、Node/Rust 同時 open が未設計。ここが崩れると制約3(高速堅牢)と移行そのものが破綻。**最優先**。
2. **A-3(supervisor が Permanent を無限リトライ)** — 制約2の自己復帰が自己ループ化。設計コメントとコードの矛盾。
3. **B-2(制約5=型生成の不在)** — 「zod のエラー版」は成功パス機密 strip を代替しない。単一真実源が未達。
4. **C-1(userId 分離と NotFound/Forbidden)** — 存在秘匿を破る 403 誘導リスク。最重要セキュリティ不変条件が視界外。
5. **A-1/A-2(grep ゲート形骸化 & lint 相互矛盾)** — 制約1の「機械的強制」の中核が誤検知と allow 上限衝突で機能しない。

ドラフトの型設計の骨(thiserror 具体 enum・手書き分類 From・ClassifyError トレイト)は堅実。破綻するのは**エラー型の外側**(DB 実行モデル・supervisor 動作・移行並行性・型生成・セキュリティ不変条件との接続)で、そこが本領域が「絵に描いた餅」になるか否かの分水嶺。

**参照した実ファイル(絶対パス):**
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/rust_synapse/src/main.rs`(`:49` `Result<_,String>`、`:337`/`:368` `Box<dyn Error>` = 移行対象・裏取り済)
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/bot.ts`(`:81` `customClients: Map<string,Client>` = 単一プロセス複数ボット、`:1316` トークン更新レース)
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/db/database.ts`(`:16-18` better-sqlite3 同期・WAL・foreign_keys ON)
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/types/apiViews.ts`(zod allowlist フェイルクローズ = 制約5 の現行機構、本ドラフト未接続)
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/db/botRepo.ts`(`:83-84` `WHERE b.user_id = ?` = userId 分離の実体)
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/services/chatChannelService.ts`(`classifyError` 正規表現分類 = GeminiError で置換対象)

---

## [16] I now have all facts verified. The draft's claims about the current code are lar
<sub>agentId: abdcd1badf3781d4c</sub>

I now have all facts verified. The draft's claims about the current code are largely accurate. Let me note one important detail about the ts-rs type generation approach that the draft misses, and check whether the `resolve_request_user` error typing has a contradiction.

The draft's `resolve_request_user` returns `Result<Option<SessionUser>, SessionError>` and claims "ストア障害＝degrade". But the current `getSession` **never surfaces a store error** — it swallows Redis errors internally and falls back. So `SessionError` would only arise from a *fully* failed path. There's a subtle but important contradiction in the draft: the `AuthUser` extractor does `resolve_request_user(p, st).await?` — the `?` propagates `SessionError` into `WebError::Session` → **500**. But when would `SessionError` fire if degradation is internal? Only if degradation *itself* fails. That's actually correct design, but the draft never defines what makes `SessionError` non-degradable. Let me write the final review.

I have all verified detail I need. Writing the review.

---

# 領域C設計レビュー（辛口・反証志向）

参照確認済み実ファイル: `httpHelpers.ts`(159行), `routeRegistry.ts`(226行), `sessionService.ts`, `db/database.ts`, `db/botRepo.ts`, `server.ts`(CSP/キャッシュ), `chatWebSocket.ts`(242行), `types/apiViews.ts`(48行), `frontend/src/lib/api/types.ts`(655行)。ドラフトの現行コード引用はおおむね正確だが、いくつかの**設計の要になる誤り・見落とし**がある。

---

## 【最優先】計画の成否に関わる論点

### 1. SQLite同期↔async の致命的ミスマッチ — `has_bot_access(...).await` は現行と非対称（確信度: 高）

**どこが**: C.6 の `st.bots.has_bot_access(&user.discord_id, &bot_id).await?`、および C.1 で `yuuka-db` が `rusqlite + spawn_blocking` と書かれている点。

**なぜ問題か**: 現行 `botRepo.hasBotAccess(userId, botId)` は `better-sqlite3` による**完全同期**関数で、`await` されていない（`financeRoutes.ts`/`personalRoutes.ts`/`reminderRoutes.ts` で `rawBotId && hasBotAccess(userId, rawBotId)` と同期呼び出し）。ドラフトは「rusqlite + spawn_blocking」を掲げるが、これは設計全体に波及する重い決定であり、ドラフトは影響を過小評価している:
- **C.1 の「yuuka-web は yuuka-db に直接依存しない、トレイト越し」**と組み合わせると、`hasBotAccess` 相当は `async_trait` になる。だが権限チェックのような**µ秒級のインメモリSQLite読み取り**を毎回 `spawn_blocking`(スレッドプールへディスパッチ＝数µs〜数十µsのオーバーヘッド+スレッドプール枯渇リスク)に載せるのは、制約#3「現行より高速」に逆行しうる。現行はイベントループ上の同期呼びで実質ゼルコスト。
- リクエストあたり `hasBotAccess` が複数回呼ばれる(financeRoutes は1ハンドラで最大数回)ため、spawn_blocking のブロッキングスレッドプール(既定512)を食い潰す DoS 表面が増える。

**どう直すべきか**: (a) 権限・セッション判定など**短命な読み取り専用クエリは `Connection` プールを `parking_lot::Mutex` で保持し同期呼び**、`spawn_blocking` は重いクエリ/書き込みのみに限定する、と C.1/C.6 に明記する。(b) あるいは web ハンドラ全体を `spawn_blocking` の外に置き、DBアクセスは専用の**単一 writer + 読み取りコネクションプール**(rusqlite の `r2d2` or `deadpool-sqlite`)にする方針を明示。いずれにせよ「全DB呼び出しを async トレイト+spawn_blocking」という素朴案は現行の同期軽量チェックより遅くなる可能性が高く、ベンチ前提を設計に書くべき。

---

### 2. `resolve_request_user` の "degrade" と `SessionError→500` が論理矛盾（確信度: 高）

**どこが**: C.3「両経路とも `Result<Option<SessionUser>, SessionError>` を返し、Redis 不通は `SessionError` にせず in-memory フォールバックへ縮退」。だが `AuthUser::from_request_parts` は `resolve_request_user(p, st).await?` と `?` 伝播し、C.2 で `WebError::Session(#[from] SessionError)` は **500**。

**なぜ問題か**: 現行 `getSessionUser` は `try { return await getSession(req) } catch { return null }` で**あらゆる例外を握り潰して null(=未認証扱い)** に落とす。さらに `getSession` 内部で Redis 例外を catch し in-memory へフォールバックする。つまり現行は「セッション解決は**決して500を出さない**、失敗は常に `null`(→401)」。ドラフトの型設計では、`SessionError` が発生する経路が定義されておらず、もし何らかの `SessionError` を返す設計にすると **現行が401を返す場面で500を返す**回帰になる。逆に `SessionError` を一切返さないなら `Result<Option, SessionError>` の `Err` 腕は**永久にデッドコード**で、型として無意味。

**制約#1との衝突**: これは「すべての失敗経路に名前と型を与え、呼び出し側が判別・分岐できる」という制約#1と、制約#2「一時障害は縮退で吸収」の**境界が設計されていない**ことの露呈。現行の「握り潰して null」を Rust で厳密再現するなら、`SessionError` を返してはならず、`resolve_request_user` は `Result<Option<SessionUser>, Infallible>` あるいは縮退を型に埋め込んだ `SessionResolution { user: Option<SessionUser>, degraded: bool }` を返すべき。

**どう直すべきか**: セッション解決を **2層に分離**して型で書く:
- `SessionStore::get` は `Result<Option<SessionUser>, StoreError>` を返す(Redis I/O 障害を型で持つ)。
- `resolve_request_user` は `StoreError` を **`match` で吸収**し `tracing::warn!(degraded=true)` + in-memory 参照へフォールバックして `Ok(Option)` を返す。**ここで `?` を使ってはいけない**(制約#1のエラー黙殺禁止に見えるが、これは「明示的に縮退へ分岐」なので黙殺ではない — その旨をコメントで正当化)。
- Extractor は `Ok(None)→401`、`Ok(Some)→通過` のみ。500 は原理的に発生しない。
ドラフトは「トークン無効＝Ok(None)/ストア障害＝degrade」と口では言うが、コードは `?` で500へ流している。**言と実装が食い違っている**ので修正必須。

---

### 3. ts-rs による型生成は「機密漏洩の構造的解消」を**保証しない**（確信度: 高）

**どこが**: C.11「DB row 構造体を直接 `Json` で返すことを禁止…機密列を持つ row 型が wire に到達しえない」+ 制約#5。

**なぜ問題か**: 現行の防御(`apiViews.ts` の zod allowlist)は**フェイルクローズ**(スキーマに無い列は `strip` で自動除去。うっかり新列を追加しても漏れない)。ドラフトの ts-rs 案は**フェイルオープン**に反転する: wire DTO を `#[derive(Serialize, TS)]` で定義し、開発者が DTO に機密フィールドを**足せば**そのまま JSON にもTSにも出る。「row を web に見せないモジュール境界」は、DTO を**手で**書き写す運用に依存し、`From<Row> for Dto` で全フィールドをコピーした瞬間に破れる。zod allowlist の「明示列挙しない限り出ない」という**構造的フェイルクローズ性が失われる**。これは制約#5「機密漏洩リスクを構造的に解消」に真っ向から反する — むしろ悪化する。

さらに ts-rs 固有の罠:
- `#[serde(flatten)]` を ts-rs は完全にはサポートしない(バージョンにより flatten の TS 出力が壊れる/型が緩む)。C.11 の `Envelope<T> { ..., #[serde(flatten)] data: T }` は ts-rs で `generated.ts` を出すと `T` の展開が期待通りにならないケースが既知。**flatten を前提にした Envelope は生成型と実 JSON がズレる**リスク。
- ts-rs は enum の serde tag/rename の一部表現で TS 側に `never` や過度に緩い union を吐くことがある。

**どう直すべきか**: (a) 機密フェイルクローズを維持するため、wire DTO を**手書きの薄い struct + 明示 `From` で列挙**し、かつ**「row 型に `#[serde(deny_unknown_fields)]` 相当のコンパイル時 allowlist」を macro で強制**する(例: `wire_view!` proc-macro で許可列のみを DTO に写像し、未列挙列はコンパイルエラー)。(b) flatten は使わず `Envelope<T>` を**タグ無しの具体構造**にして ts-rs の flatten 依存を避ける。(c) CI に「row 型と DTO の列差分が機密列を含まないことを検査する専用テスト」を追加し、zod allowlist と同等のフェイルクローズを**別の機構で**再建する。ドラフトの「型分離だけで解決」は楽観的すぎる。

---

### 4. `CatchPanicLayer` を「制約#1の多層防御」と呼ぶのは自己矛盾（確信度: 中〜高）

**どこが**: C.8「ハンドラ単位の panic は tower の `CatchPanicLayer` がキャッチして500…制約#1の非起動パス panic 禁止を多層防御(clippy 静的禁止 + CatchPanic 実行時網)」。

**なぜ問題か**: 制約#1は「非起動パスでの panic! を**禁止**」。`CatchPanicLayer` を張るという設計は「panic は起きうる前提でネットを張る」ことであり、**「起きない」を保証する clippy とは矛盾するメッセージ**。より実務的な問題として:
- `CatchPanicLayer` が拾えるのは**同期的な** unwind のみ。ハンドラが `tokio::spawn` した子タスク内の panic、あるいは `.await` を跨いだ後にドロップ時 panic するようなケースは**このレイヤでは拾えない**。C.6 の WS `chat_connection` は `on_upgrade(move |socket| ...)` で**別タスク**に飛ぶため、`CatchPanicLayer`(HTTPリクエスト/レスポンス経路)の**外側**にいる。WS ターン処理タスクが panic すると `CatchPanicLayer` は無力で、そのタスクだけ静かに死ぬ(接続がハングする)。
- `spawn_blocking` 内 panic は `JoinError::is_panic` としてしか回収できず、CatchPanicLayer 対象外。上記#1で spawn_blocking を多用するなら、この経路の panic 回収を別途設計する必要がある。

**どう直すべきか**: (a) 「clippy で静的禁止」と「CatchPanic で網」の**関係を再定義**: clippy は開発時の一次防御、CatchPanic は「それでも到達した未知の panic を500に変換しログする最終防御」であり**矛盾ではなく縦深**である、と正しく整理する(現行文言は「禁止したものにネットを張る」ように読め監査で突っ込まれる)。(b) **WS/spawn_blocking の panic 回収を明示**: WS ターンタスクを `tokio::spawn` する箇所は必ず `JoinHandle` を supervisor 相当で `await` し `is_panic` を検知して接続を `close` する。C.6 に「WS タスク panic は CatchPanicLayer 対象外」と明記し個別対策を書く。

---

### 5. CSRF レイヤの `?` が制約#1違反かつ挙動非等価（確信度: 中）

**どこが**: C.4.2 `csrf_guard` 内 `is_cross_site_state_change(req.headers(), &CONFIG)?`、および `is_cross_site_state_change` が `Result<bool, WebError>` を返す設計。

**なぜ問題か**: 現行 `isCrossSiteStateChange` は `boolean` を返す**純関数で、決して throw しない**(URL パース失敗は `catch { return true }` で拒否側に倒す)。ドラフトが `Result<bool, WebError>` にしている理由が本文に無く、`?` で `WebError` を伝播させる余地を作っている。もし `is_cross_site_state_change` が `Err` を返す実装になると、**現行が「拒否(403 CsrfRejected)」する場面で別のエラー→別ステータス**になる回帰。純粋な判定に `Result` を持ち込むと「判定不能」と「内部エラー」の区別が曖昧になり、制約#1の「失敗経路に名前を与える」の趣旨に反して**無名の失敗**を作る。

**どう直すべきか**: `fn is_cross_site_state_change(&HeaderMap, &Config) -> bool` に戻す(現行と同一シグネチャ)。URL パース失敗・ヘッダ非UTF8 は現行同様すべて `true`(拒否側)へ倒す。`csrf_guard` は `if is_cross_site_state_change(...) { return Err(WebError::CsrfRejected) }` のみ。`Result` は不要。

---

## 【中優先】既存不変条件の見落とし

### 6. `getClientIp` の trustedProxies / XFF 移植が C 全体から欠落（確信度: 高）

**どこが**: C.4.4 で「`trustedProxies` 検査(getClientIp 移植)を通した peer のみ」と**一言触れるだけ**。だが `getClientIp` は右端から最初の非信頼プロキシを採るという**非自明なロジック**(httpHelpers L65-83)で、レート制限のキー(制約に直結)とHTTPS判定の両方に効く。

**なぜ問題か**: axum/hyper は `ConnectInfo<SocketAddr>` で peer を取れるが、**XFF パースと trustedProxies 照合は自前実装が必須**。リバースプロキシ(現行 Docker 構成)背後では `peer` は常にプロキシIPになるため、ここを間違えると**全ユーザーが同一IP扱い→レート制限が事実上無効化**、あるいはXFF偽装でレート制限回避。C はこれを設計項目として立てていない。

**どう直すべきか**: C.4 に「ClientIp Extractor」を独立節として追加。`ConnectInfo` の peer が `trusted_proxies` に含まれる時のみ XFF を右端から走査、と現行L65-83を逐語移植。レート制限レイヤ(そもそも C に**レート制限の設計が無い** — `WebError::RateLimited` は定義されているのに実装層が空)もあわせて設計する。

### 7. WS の直列化を `mpsc::channel(4)` にするとセマンティクスがズレる（確信度: 中）

**どこが**: C.6「`MAX_QUEUED_TURNS=4` は `mpsc::channel(4)` の容量で表現、満杯時 `try_send`→`Err(Full)`→rate_limited」。

**なぜ問題か**: 現行(chatWebSocket L112-170)は `chain: Promise` に**現在処理中の1件 + キュー**という構造で、`queued` カウンタは「チェーンに積まれた待ち件数」。`if (queued >= MAX_QUEUED_TURNS)` で拒否。`mpsc::channel(4)` は**バッファ4 + 受信側が今処理中の1**で実効5、かつ**送信中の一時的空きのタイミング**で現行と拒否閾値が1件ズレる。さらに現行の `queued++/queued--` は処理完了で減るが、mpsc は `recv()` した瞬間(処理開始時)にバッファから抜けるため、**「処理中1件+キュー4件」の状態が現行と mpsc で数え方が違う**。境界(ちょうど満杯付近)でクライアント体感が変わる。

**どう直すべきか**: 意味等価を厳密にするなら `channel(MAX_QUEUED_TURNS)` ではなく、**`AtomicUsize` の明示カウンタ + `channel(cap)` を大きめに取り、`fetch_add` で現行の `queued>=4` を逐語再現**する。または「1件ズレを許容する」と明記して回帰テストの許容差分を宣言する。「容量で表現＝等価」は不正確。

### 8. CSP を「現行と同一」と書いているが提示CSPは不完全（確信度: 中）

**どこが**: C.5「現行と同一の CSP(`script-src 'self' https://static.cloudflareinsights.com`)」。

**なぜ問題か**: 実 CSP(server.ts L83)は `default-src / style-src 'unsafe-inline' + fonts / font-src / img-src(data: discordapp cdn 等) / connect-src 'self' https://cloudflareinsights.com / worker-src / frame-src / frame-ancestors` を含む**9ディレクティブの複合**。ドラフトは `script-src` 一行だけ引用して「同一」と称しており、移植時に他ディレクティブ(特に `img-src https://cdn.discordapp.com` — アバター表示、`worker-src 'self'` — SW、`connect-src` — アナリティクス)を**取りこぼす**と機能破壊。

**どう直すべきか**: C.5 に CSP 全文(server.ts L83)を定数として貼り、`SetResponseHeaderLayer` で一括付与すると明記。「同一」は全ディレクティブを指すことを保証。

### 9. `stripProtoKeys` 移植不要論は半分正しく半分危険（確信度: 中）

**どこが**: C.4.3「Rust に proto 汚染は言語的に存在しない…移植不要。ただし薄いラッパで等価防御」。

**なぜ問題か**: 結論(SanitizedJson で剥がす)は妥当だが、現行 `stripProtoKeys` は `constructor`/`prototype` も削る(routeRegistry L73)。ドラフトの `strip_proto_keys` コメントは `__proto__/constructor/prototype` と書くが、**実際に剥がすべきキー集合と深さ(現行 depth>6)を逐語一致させないと下流JS(MCP中継先)で差が出る**。より重要なのは、現行は**Webhook 生ペイロード経路では stripProtoKeys を通さず**(L196-198 の catch で rawBody のみ)、かつ**MCP proxy 経由の JSON は現行どのstrip対象か**を C は検証していない。「等価な防御面」と言うなら、strip を適用する経路/しない経路の対応表が必要。

**どう直すべきか**: `SanitizedJson` の剥離キー・深さを現行と定数一致させ、**適用経路の対応表**(api群=適用 / hook=非適用でBytes直 / MCP proxy=中継先がJSなら適用)を C.4.3 に明記。

---

## 【技術的正当性】クレート/API の実在性

- **axum 0.8 の `{param}` 構文、`FromRequestParts`、`WebSocketUpgrade`、`CatchPanicLayer`(tower-http)、`RequestBodyLimitLayer`、`ServeDir::precompressed_br/gzip`、`CompressionLayer`** — いずれも実在し妥当。**妥当**。
- **`figment` による env/yaml 階層マージ、`secrecy::SecretString`、`ts-rs`、`twilight`(Discord)** — 実在。**妥当**(ただし ts-rs の flatten 問題は#3参照)。
- **`#[tokio::main(flavor="multi_thread")]` + ワークスティーリング** — 妥当。ただし制約#3「現行より高速」の**論拠が「Node は単一イベントループ」という一点**に依存しており、Node も I/O は libuv スレッドプールで並列化される。CPUバウンドが本当に支配的かはベンチ要。**確信度中**: 高速化の主張は「静的圧縮の事前化」「radix ルータ」で裏付けた方が堅く、マルチコアだけを根拠にしない方がよい。
- **matchit の「静的セグメント優先で先勝ち等価」** — matchit は静的>パラメータの優先順位を持つが、現行 `routeRegistry` は**登録順の線形先勝ち**(L144 のループ)。両者は**大半のケースで一致するが完全等価ではない**。例えば現行で `/api/foo/:id` を `/api/foo/bar`(静的)より**先に登録**すると現行は `:id` が勝つが、matchit は必ず静的 `bar` が勝つ。**確信度中**: 現行の登録順に順序依存があるルートが1つでもあれば挙動が変わる。移行前に「登録順に依存する重複ルートが存在しないか」を全 registerRoutes 呼び出しで監査すべき、と C.12 リスクに追加を。

---

## 【移行実現性】

### 10. 段階移行のパスが「全面書き換え一括カットオーバー」前提で、並行運用の設計が無い（確信度: 中）

**どこが**: C 全体が「Rust で置換」を前提。だが制約群は Gemini/Discord/DB/Redis 依存の巨大バックエンドの**全面書き換え**であり、C.1 で `rust_synapse`/`rust_crawler` を workspace に取り込むと書くが、**TS版と Rust版を並行させる期間のルーティング分担**(どのパスから Rust に載せ替えるか)が設計されていない。

**なぜ問題か**: 一括カットオーバーは、CSRF/セッション/機密allowlist という**セキュリティ不変条件を一度に全部載せ替える**ことになり、回帰リスクが最大化。instance.sh の verify(curl による CSP/immutable 検証)はサーバ非依存なので**そこは救い**だが、認証・データ分離は curl 検証では捕捉できない。

**どう直すべきか**: Rust を**フロントプロキシ**として先に立て、未移植パスは TS バックエンドへ `reverse proxy` する段階案を C.12 に追加。セッションは Redis 共有(現行 `session:{sha256}` キー体系)なので **Rust と TS が同一 Redis を読めば認証は両実装で共通化できる** — この「Redis 共有でセッション互換」を移行の要として明記すると並行運用が現実的になる。in-memory フォールバックは**プロセス別**なので、並行期間はフォールバックが割れる点に注意(Redis 必須期間を設ける)。

### 11. `bins/gen-types` の drift ブロックはDTO追加時に機密を素通しさせうる（確信度: 中）

**どこが**: C.11「CI の `git diff --exit-code` で drift をブロック」。

**なぜ問題か**: これは「Rust DTO と生成TSの一致」を保証するだけで、「**DTO に機密が入っていないこと**」は一切保証しない(#3 と同根)。開発者が `secret_token` を DTO に足すと、生成TSも更新され diff は**通ってしまう**(生成物とソースが一致するため)。フェイルクローズにならない。

**どう直すべきか**: #3 の allowlist macro + 機密列検査テストを CI ゲートに追加。「diff一致」は型同期の保証であって漏洩防止ではない、と明記。

---

## 妥当と明言できる項目

- **axum + tower/tower-http 採用、actix 却下理由(二重ランタイム/Extractor型合成)** — 妥当。
- **`AuthUser`/`AdminUser` Extractor で認証レベルを型強制**(admin ルートに user を書くとコンパイルエラー) — 現行の実行時 `route.auth` チェック(L163-172)を型に上げる発想は正しく、**制約#1/#4 に資する良い設計**。妥当。
- **proxy/hook/ws を CSRF レイヤ非適用の別サブルータに物理分離**(現行 `route.auth!=="none"` 分岐をルータ構造で表現) — 現行L151の分岐より安全で、**妥当かつ改善**。
- **fail-fast 境界(secret/PORT/baseUrl のみ起動停止、Redis/engine/Discord は縮退)** — 現行(index.ts の secret 不備で exit)と整合。起動パスの `exit(1)` を制約#1例外とする線引きも妥当。**妥当**。
- **事前圧縮(vite `.br`/`.gz` + `ServeDir::precompressed_*`)への格上げ** — オンザフライ gzip 廃止は制約#3に資する。**妥当**。
- **clippy `#![deny(unwrap_used/expect_used/panic/todo/unimplemented)]` + cargo-deny で anyhow/eyre 禁止** — 制約#1の機械強制として**妥当かつ必須**。ただし `allow-unwrap-in-tests=true` は現行の非テストパス方針と整合。
- **GSV 起動時1回置換+`Arc<Bytes>` 保持** — config 不変ゆえ妥当。
- **immutable キャッシュ正規表現を server.ts L157 と一致させ instance.sh verify で回帰検出** — 検証機構の再利用は**妥当かつ賢い**。

---

## 総括(辛口)

設計の**骨格(axum/tower、Extractor 型強制、ルータ構造分離、clippy/deny 強制)は堅実**で、制約#1/#4 の型・機械強制の方向性は正しい。しかし**3つの主張が制約に反するか自己矛盾**している:

1. **ts-rs 型生成が機密フェイルクローズを"構造的に解消"する**(#3, #11)— 実際はフェイルオープンに反転し**制約#5に逆行**。最重要。
2. **セッション解決の縮退が `?`→500 と矛盾**(#2)— 言と実装が食い違い、現行の「失敗は常に401」を回帰させる。
3. **SQLite を全 async+spawn_blocking にする素朴案**(#1)— 現行の同期軽量チェックより遅くなりうる。制約#3の"高速"を自壊させかねない。

加えて **getClientIp/レート制限の設計欠落**(#6)、**WS タスク panic が CatchPanicLayer 圏外**(#4)、**CSP 全文の取りこぼしリスク**(#8)は、移植時にセキュリティ/機能の穴を作る。**#1〜#3 を解決しない限りこの設計は制約充足を主張できない**。

---

## [17] Note: the existing synapse Cargo.toml pins `rusqlite` **without** the `serde_jso
<sub>agentId: aea60d34a79355815</sub>

Note: the existing synapse Cargo.toml pins `rusqlite` **without** the `serde_json`/`blob`/`functions`/`trace` features the draft assumes are compatible — worth a quick sanity note but not load-bearing. I have enough. Writing the review.

---

# DB層・マイグレーション・データ分離ドラフト — 辛口レビュー

裏取りを実施。ドラフトの主張の大半は正確だが、**設計の根幹に関わる事実誤認が1件、制約充足の抜け道が複数、そして「データ分離」を名乗りながら落としている不変条件が複数**ある。重要度順に述べる。

---

## A. 致命的（計画の成否に関わる）

### A-1. 【事実誤認 → §8 の設計前提が崩れる】Redis フォールバックは「in-memory Map」ではなく用途ごとに異なり、一部は SQLite 由来（確信度: 高）

**どこが**: §8「`Memory(Mutex<MemoryBackend>)` … session/rate/oauth/action 共通」。

**なぜ問題か**: 実装を読むと、フォールバックは単一の「memory backend」ではない。3系統に分かれる:
- `sessionService.ts` / `oauthStateStore.ts` / `actionRecorder.ts` / `botRateLimit.ts` → **プロセスローカルの in-memory Map**（再起動で消える、許容）。
- `messageLogRepo.ts` の `context:{userId}` → Redis はあくまでキャッシュで、**正は SQLite `message_logs`。キャッシュ消失時は SQLite から直近15件を再構築**する（`redis.ts` のコメント「SQLite フォールバックで稼働」はこれを指す）。

ドラフトが「session/rate/oauth/action 共通の memory backend に集約」と書いたのは正しい方向だが、**messageLog の "SQLite が正・Redis はキャッシュ" という第4パターンを `VolatileStore` enum に押し込むと意味が壊れる**。`context` は揮発ストアではなく SQLite の派生ビューであり、`VolatileStore::Memory` に入れると「再構築可能な永続キャッシュ」という不変条件（synapse RAM索引と同じ性質）を失う。さらに `redis.ts` のログ文言「SQLite フォールバック」を額面通り読むと、ドラフト著者は**セッションまで SQLite にフォールバックすると誤解した可能性**があり、その場合 §8 の memory backend 設計自体が現行と乖離する。

**どう直すべきか**: 揮発ストアを2階層に分離して型化する。
```rust
// (1) 純揮発（消えても再構築不能・消失許容）: session/oauth/action/rateLimit
enum VolatileStore { Redis(...), Memory(...) }
// (2) SQLite派生キャッシュ（Redis消失時は DB から再構築）: message context / synapse
struct DerivedCache<Src> { redis: Option<...>, rebuild_from: Src }
```
`context` と synapse RAM索引を同じ「再構築可能キャッシュ」トレイトの下に置けば、制約2の劣化縮退が型で一貫する。現状ドラフトはこの区別を欠き、実装時に「context を memory Map に入れて再起動で会話文脈が飛ぶ」退行を招く。

---

### A-2. 【制約1の抜け道】`JsonColumn::parse` が `Err` を返さず `Ok(Malformed)` を返す設計は、握り潰しを型から追放できていない（確信度: 高）

**どこが**: §5「JSON列の型化」。`parse` は不正JSON時に `tracing::warn!` して `Ok(Self::Malformed(s))` を返す。

**なぜ問題か**: ドラフトは「握り潰しをシグネチャに現す」と主張するが、**`or_default()` を呼んだ瞬間、生値 `s` は捨てられ、警告ログ以外に痕跡が残らない**。これは制約1が禁じる「劣化を黙殺」そのものである。しかも `parse` が `Result<Self, DbError>` を返すシグネチャなのに内部で握って `Ok` を返すのは、**「失敗経路に名前と型を与える」原則の違反を型で隠蔽している**。呼び出し側は `.or_default()` を書くだけで、判別も分岐もせず既定値に流せてしまう — 現行 TS の try/catch→空配列と実質同じ黙殺で、制約1の狙いを達成していない。

**どう直すべきか**: フォールバックを**呼び出し側の明示的 `match` に強制**する。`JsonColumn<T>` を返さず、`Result<T, DbError::JsonColumn>` を返し、フォールバックしたい箇所だけが `match e { DbError::JsonColumn{..} => T::default(), other => return Err(other) }` と**手で書く**。さらに `Malformed` の生値を捨てるなら、捨てる前に `audit_logs` へ記録する経路を義務化（clippy では表現できないので、`or_default` を `#[deprecated]` にしてレビュー強制する等）。

---

### A-3. 【データ分離の穴を型で塞げていない】`ScopedRepo` は `SELECT *` を許し、機密列 strip の不変条件が row→DTO 変換に依存したまま漏れうる（確信度: 中〜高）

**どこが**: §5 の `BotRow` は `discord_token_encrypted` を保持、§6 の `list_todos` は `SELECT *`。

**なぜ問題か**: 2つの穴が合成される。
1. `ScopedRepo` は user_id 注入を強制するが、**返す型は `Vec<TodoRow>` = row 型そのもの**。row 型が機密列を持つ `BotRow` の場合、`ScopedRepo::get_bot` が `BotRow` を返せば、**ハンドラが `BotRow` を直接 serialize してしまう事故は型では防げない**。ドラフトは「row 型に `Serialize` を付けないことを CI grep で強制」と書くが、これは**型ではなくlint頼み**であり、制約1が要求する「型で不変条件化」に達していない。grep は `#[derive(Serialize)]` を弾いても、手書き `impl Serialize` や `serde_json::to_value(&row.some_field)` の部分漏洩は捕まえられない。
2. `SELECT *` は**マイグレーションで列が増えたときに機密列を自動的に row に取り込む**。`from_row` が列名指定でも、`SELECT *` の列順依存コードが混ざれば破綻する。現行 `apiViews.ts` の allowlist は「増えた列はデフォルト遮断（フェイルクローズ）」だが、`SELECT *`＋`from_row` は**新列をデフォルト取り込み（フェイルオープン）**で、フェイルクローズ性を失う。

**どう直すべきか**:
- 機密列を `BotRow` から**別の `Sensitive<T>` ラッパ**に隔離し、`Sensitive` は `Serialize` を実装しない newtype にする（型で漏洩不能。lint 不要）。`has_token: bool` は `Sensitive::is_present()` から算出。
- リポジトリは `SELECT *` を禁止し、**明示列リスト定数**を使う。新列は明示追加しない限り取り込まれない＝フェイルクローズを維持。clippy では `SELECT \*` の grep 禁止を CI に足す。

---

## B. 重要（堅牢性・正当性）

### B-1. 【自己復帰が絵に描いた餅になる箇所】単一書き込み actor は「supervisor 配下で panic 隔離・指数バックオフ再起動」と矛盾しうる（確信度: 高）

**どこが**: §2 書き込み actor ＋ 制約2「各サービスを supervisor 配下で再起動」。

**なぜ問題か**: 書き込み actor が panic して supervisor が再起動すると、**`mpsc::Sender` を握っている全ハンドラの `oneshot` が `RecvError` で落ち、in-flight トランザクションが失われる**。ドラフトはこの障害の型（`DbError` に `WriterGone` 相当が無い）を定義していない。さらに actor 再起動で新しい `Connection` を開くと、**WAL チェックポイントや `busy_timeout` の途中状態、savepoint スタックが失われる**。「書き込み actor = 唯一の writer」を supervisor 再起動と両立させるには、再起動時の未処理コマンドの扱い（drop して呼び出し側へ `WriterRestarted` を返すか、再キューするか）を型とポリシーで決める必要があるが、ドラフトは無言。実装すると「actor が死ぬたびに全書き込みが silent に失敗 → 制約2の "些細な障害で落ちない" 違反」になる。

**どう直すべきか**:
- `DbError` に `#[error("writer restarted")] WriterUnavailable` を追加し、`oneshot` の `RecvError` を必ずこれに写す（黙殺禁止）。
- 書き込み actor の panic は「回復可能」に分類し、supervisor 再起動＋呼び出し側リトライ（冪等な書き込みのみ）を明示。**非冪等トランザクション（例: `plannedPaymentRepo` の advance）は自動リトライ禁止**を型で区別（`Idempotent` マーカートレイト）。
- そもそも「書き込み actor が panic する余地」を減らすため、actor 本体は `catch_unwind` 相当で `DbError::Internal` に落とし、**panic 自体を型付きエラーに変換して actor を延命**する設計を推奨（supervisor 再起動は最終手段）。

### B-2. 【FK-off の事実誤認 → 移行 runner の網羅性が不足】FK-off は「3 rebuild 箇所」ではなく最低5箇所（確信度: 高）

**どこが**: §4「FK-off は `:196/888/1113` の3rebuild箇所」。

**なぜ問題か**: 実際には `:196`, `:460`（bot_mcp_access owner_id 移行=v7）, `:525`（bot_google_account nullable=v6）, `:888`, `:1113` の**最低5箇所**で `foreign_keys = OFF` が使われる。ドラフトが数え落とした `:460`/`:525` は**まさに「トランザクション内で FK-off して rebuild」するパターン**で、ドラフトの `run_migrations` が全stepを1つの外側トランザクションに包む設計と**衝突する**: SQLite は**トランザクション内での `PRAGMA foreign_keys` 変更を無視する**（`foreign_keys` はトランザクション境界外でのみ有効）。つまり §4 の `run_migrations` の `tx.pragma_update("foreign_keys","OFF")` は**トランザクション内なので効かず**、FK違反を含む rebuild が失敗するか、意図せず制約が効いたまま `DROP TABLE` して FK カスケードが暴発する。

**どう直すべきか**: これは §4 のコード例の致命的バグ。FK-off が必要な step は**外側トランザクションの外で** `PRAGMA foreign_keys=OFF` → step 実行（step 内で個別トランザクション）→ `PRAGMA foreign_keys=ON` → `PRAGMA foreign_key_check` の順にする。全step単一トランザクション化（原子性の売り）と FK-off 要件は**両立しない**ので、「FK-off を要する step」と「要さない step」を型で分け、前者は原子性を1step単位に落とす。ドラフトの「全stepを1トランザクションで途中クラッシュしてもversion進まない」という売り文句は、FK-off step が存在する時点で**部分的に成立しない**ことを明記すべき。

### B-3. 【synchronous=NORMAL の副作用を過小評価】(確信度: 中)

**どこが**: §2「`synchronous=NORMAL` は WAL の定石・性能改善」。

**なぜ問題か**: 正しいが、**現行 better-sqlite3 の既定は `FULL` ではなく実は WAL 有効時でも `FULL` のまま**であり、`NORMAL` への変更は「電源断で直近コミットを失いうる」耐久性トレードオフを**新たに導入**する。ドラフトは「DB破損はしない」と正しく述べるが、**支払い記録（`plannedPaymentRepo`）や監査ログ（`audit_logs`）のように "直近1コミットの喪失" が業務上問題になるデータ**があるかを検討していない。制約3（堅牢性）と耐久性のトレードオフを一律 `NORMAL` で決めるのは乱暴。

**どう直すべきか**: `NORMAL` を既定にしつつ、金銭・監査系の書き込み後に**明示的 `PRAGMA wal_checkpoint(TRUNCATE)` または該当トランザクションだけ `synchronous=FULL`** を選べる型（`Durability::{Fast, Durable}`）を `writer.transact` の引数にする。

### B-4. 【crate 実在性・整合】fred v9 / ts_rs は妥当だが rusqlite features に落とし穴（確信度: 中）

- `fred` v9 の `ReconnectPolicy`（指数バックオフ内蔵）、`ts_rs::TS` はいずれも実在・妥当。**妥当**。
- ただし既存 `src/rust_synapse/Cargo.toml` は `rusqlite = { version = "0.32", features = ["bundled"] }` のみ。ドラフトは `["bundled","serde_json","blob","functions","trace"]` を要求するが、**workspace で feature unification が起きると synapse プロセスにも `functions`/`trace` がリンクされ、read-only の売り（"書き込み構造的に不可能"）とは無関係にバイナリが太る**。また synapse は既に独自 Cargo.toml を持つ別crateなので「workspace統合で依存一本化」（§1-2, §9）は**feature 差分の解決を要し、自動では一本化しない**。
- `deadpool-sqlite` の `interact` クロージャは `'static` 制約が厳しく、`ScopedRepo` の `move` クロージャに `user_id: String` を毎回 clone する（§6 の `self.user.0.clone()`）のは**ホットパスでのアロケーション**。制約3（Nodeより高速）に対し、`Arc<str>` 化を検討すべき。**中程度の性能懸念**。

**どう直すべきか**: synapse crate の feature を明示的に固定（`default-features = false`）し、feature unification を `resolver = "2"` で分離。ID newtype は `Arc<str>` バックにする。

---

## C. 落としている不変条件（既存機能の完全性）

### C-1. 【CSRF/CSP/SameSite が「データ分離」設計から完全に欠落】（確信度: 高）

**どこが**: ドラフト全体。§8 でセッションを扱うのに、**CSRF 防御・CSP・Cookie の `SameSite=Lax; HttpOnly`（`httpHelpers.ts:106`）・Origin/Referer 検証（`routeRegistry.ts:32`）に一言も触れていない**。

**なぜ問題か**: セッションストアを Rust に移すなら、セッション Cookie の発行属性・CSRF トークン経路・「Bearer は CSRF 非該当（Origin チェックは Cookie 経路のみ）」という**現行の緻密な認証境界**を移植対象に含めねばならない。ここを落とすと、Rust 移行時にセッション永続化だけ移して**CSRF フェイルクローズが退行**する典型事故になる。データ分離レビューとしての穴。

**どう直すべきか**: §8 に「セッション Cookie 属性・CSRF Origin 検証・Bearer 例外」を型で表す節を追加。`SessionKey` 発行時に Cookie 属性を型で強制し、`enum AuthPath { Cookie, Bearer }` で CSRF 適用を分岐（Bearer は非該当を型で表現）。

### C-2. 【rate limit の "Lua で原子化" は現行の防衛線を弱めうる】（確信度: 中）

**どこが**: §8「`incr`→`expire` 非アトミック性を Lua EVALSHA で原子化」。

**なぜ問題か**: 改善方向は正しいが、`botRateLimit.ts` の**インメモリフォールバック `incrementMemoryCounter` は Lua を使えない**。Redis 不通時は memory Map で `incr+expire` 相当を実行する。ドラフトは Lua 原子化を Redis 経路にしか適用しておらず、**フォールバック経路の原子性は依然として担保されない**。さらに現行は「memory はプロセスローカル＝マルチプロセスでレート制限が緩む」既知の割り切りがあるが、Rust でマルチスレッド化しても**プロセス複数化（supervisor 再起動含む）でカウンタがリセットされる**性質は変わらない。制約3「マルチコア活用」を進めると、**スレッド間 memory 共有（`Mutex<Map>`）の競合がレート制限のホットパスに乗る**。

**どう直すべきか**: memory フォールバックは `DashMap` + atomic で原子化。Redis/memory の両経路が同じ「incr-with-ttl」トレイトを実装し、原子性を**両実装で保証**することを型で要求。

### C-3. 【`delete_user` の孤児掃除テーブル列挙が漏れ得る】（確信度: 中）

**どこが**: §6 の `ORPHAN_TABLES_ON_USER_DELETE` 定数配列。

**なぜ問題か**: 「定数配列で列挙し漏れを防ぐ」と言うが、**新テーブル追加時に配列へ足し忘れると孤児が再発する**。これは配列が「真実源」ではなくスキーマの二重管理になっている。制約5（単一真実源）の精神に反する。

**どう直すべきか**: user_id 列を持つ FK非設定テーブルを**マイグレーション定義から導出**する（各テーブル定義に `owns_user_id: bool` メタを持たせ、runner が孤児掃除リストを生成）。あるいは起動時に `PRAGMA table_info` で `user_id` 列を持つが FK を持たないテーブルを内省列挙し、配列との差分を fail-fast チェック。

---

## D. 妥当と認める点

- **rusqlite 選定（§1）**: 実行時スキーマ内省が移行中核、sqlx `query!` のコンパイル時DB接続との非互換、synapse との整合 — 論拠は正確で**妥当**。
- **`busy_timeout` 未設定＝Rust並行書き込みで即 BUSY の指摘（§2）**: 現行コード（database.ts が WAL/FK のみ）で裏取り済み。**妥当**。
- **レガシー全DROP・mcp_servers DROP の非破壊 rebuild 化（§4）**: `:882-894`, `:1465-1470` の DROP は実在し、Rust 初期化で version 誤判定→全DROP のリスクは**現実的**。保持型 rebuild への置換は**妥当**（ただし B-2 の FK-off バグを直す前提）。
- **UserId/BotId newtype による引数取り違え防止（§6）**: **妥当**。
- **synapse を別プロセス read-only 維持・書き手を actor に一本化（§9）**: `storage.rs` の READ_ONLY フラグ強制・「空索引で稼働継続」方針と整合。**妥当**。
- **ts_rs による row/DTO 分離＋CI drift 検出（§5, §10）**: 方向性は**妥当**（ただし A-3 の通り「lint 強制」ではなく「型で漏洩不能」に格上げが必要）。

---

## 総括（優先度）

**着手前に必ず直すべき**: A-1（Redis フォールバックの事実誤認 → §8 設計の作り直し）、A-2（JSON黙殺の型隠蔽）、B-2（トランザクション内 FK-off が効かない → §4 コード例のバグ）。この3つは**現状のまま実装すると退行または不動作**する。

**設計に節を追加すべき欠落**: C-1（CSRF/CSP/Cookie 属性）、B-1（書き込み actor と supervisor 再起動の整合）。

**強化推奨**: A-3（機密strip を lint でなく型で）、B-3（耐久性トレードオフの選択型）、C-2/C-3。

主要参照ファイル（裏取り済み・絶対パス）:
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/db/redis.ts`（フォールバックは用途別／SQLite再構築は message context のみ）
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/db/messageLogRepo.ts`（context キャッシュは SQLite が正）
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/db/migrations.ts`（FK-off は :196/460/525/888/1113 の5箇所、v5/v7 は tx 内で FK-off）
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/services/botRateLimit.ts`（memory フォールバックは Lua 非対応）
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/server.ts` / `src/server/routeRegistry.ts` / `src/server/httpHelpers.ts`（CSP・CSRF Origin検証・SameSite=Lax）
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/rust_synapse/src/storage.rs` / `Cargo.toml`（read-only 強制・rusqlite feature は bundled のみ）

---

## [18] I have all the verification I need. The codebase confirms every citation, and I 
<sub>agentId: ae350bcc349bdc99a</sub>

I have all the verification I need. The codebase confirms every citation, and I have concrete knowledge of the ts-rs limitations that break this plan. Writing the critique.

---

# 辛口レビュー: フロント⇄Rust 型連携（単一真実源）ドラフト

裏取りの結論を先に: 引用ファイル（`apiViews.ts` 48行/zod 3ビュー、`types.ts` の `ApiResponse<T>` 交差型 L13・SetupStatus 例外 L32・BotView 手書き L51、`client.ts` の scope 規約、`device.ts` の OAuth 例外、services 20本、rust_synapse/crawler が独立クレート）はすべて実在を確認した。引用の正確さは高い。**しかし中核メカニズム 2 点が ts-rs の実挙動と食い違っており、このままでは計画の背骨が折れる。**

---

## A. 致命的欠陥（計画の成否に直結）

### A-1. ts-rs は `#[serde(flatten)]` を交差型に落とせない — エンベロープ表現の前提が崩壊【確信度: 高】

**どこが**: §4.1。「`serde(flatten)` を ts-rs が `serde-compat` 下で交差型 `{success,message} & T` に落とす」「生成 `envelope.ts` が現行 `types.ts:13` と字面まで一致」という主張。

**なぜ問題か**: ts-rs は `#[serde(flatten)]` を**サポートしていない**。ts-rs の既知の制約で、flatten を含む struct は生成時に `flatten` を無視して通常フィールドとして扱うか、`serde-compat` 有効時はコンパイル/エクスポートで警告・不正型を吐く（flatten されたジェネリック `T` を交差型 `& T` として展開する機能は実装されていない）。さらに **ts-rs のジェネリック型パラメータ `T` に対する `#[serde(flatten)] data: T` は、ts-rs のジェネリクス export と flatten が二重に噛むため、そもそも `export()` が意図した交差型を生成しない**。つまり「字面まで一致」は実測ではなく期待であり、実際は生成が破綻するかフィールドがフラットに落ちない。この 1 点が崩れると §0 の推奨表（却下理由「utoipa は交差型を素直に再現できない」＝ ts-rs なら再現できる、という対比）の根拠自体が消える。

**どう直すべきか**:
1. **エンベロープをジェネリック flatten で表現しない**。各レスポンスごとに `success`/`message` を含む具象 DTO を Rust 側で定義し（マクロで定型化）、ts-rs にはフラットな具象型を吐かせる。TS 側の `ApiEnvelope<T>` 交差型ヘルパは**手書きで 1 行維持**（生成対象外の「クライアント規約」に分類。現行 `types.ts:13` をそのまま残す）。
2. あるいは **ts-rs を諦め、この 1 点のためだけに具象 wrapper を許容**し、`type XResponse = { success: boolean; message?: string } & XPayload;` の交差 alias を xtask 側の文字列結合で後付け生成する（ts-rs 生成の payload に手書きテンプレで被せる）。いずれにせよ「flatten が交差型に落ちる」前提のコードは書けないので、§4.1 のコード例は削除・差し替え必須。

---

### A-2. rusqlite は async ではない — マルチスレッド前提（制約3）と衝突する設計欠落【確信度: 高】

**どこが**: 依存に `rusqlite`（synapse Cargo.toml で確認、`bundled`）を引き継ぐ前提だが、ドラフトは axum（tokio async）サーバへの統合を語りながら SQLite の同期↔async 境界に一切触れていない。制約3「マルチコアを活かすマルチスレッド実行」を掲げているのに、DB アクセス戦略が無い。

**なぜ問題か**: `rusqlite::Connection` は `!Sync` かつ同期ブロッキング。axum ハンドラ（async）内で直接呼ぶと tokio ワーカースレッドをブロックし、マルチコアどころか**現行 Node（single-thread event loop + better-sqlite3 同期）より悪化する**恐れがある。さらに `Connection` は `Send` だが `Sync` ではないため、`Arc<Connection>` を複数タスクで共有できず、素朴実装は `Mutex<Connection>` で**全 DB アクセスを直列化**してしまう。yuuka は「複数ボット常時接続」（制約2・4）で、各ボットが並行に DB を叩く。直列化は致命的なスループット劣化。これは型連携領域の外だが、**同じ workspace 統合（§5.2 で本ドラフトが新設を主導）に乗るため、本ドラフトが「workspace 設計の親」を名乗る以上、DB 並行モデルの欠落は本ドラフトの責任範囲**。

**どう直すべきか**: (a) `tokio::task::spawn_blocking` + コネクションプール（`r2d2` + `r2d2_sqlite`、または `deadpool-sqlite`）を DB 層の初期設計に明記。(b) SQLite を WAL モードにし read 並行を許可、write は単一 writer に直列化する現実を型・ポリシーで明示。(c) 「rusqlite 同期を async 境界でどう隔離するか」を workspace 設計の必須項目として本ドラフトの参照領域に追記。触れないまま「マルチコアで高速」と主張するのは絵に描いた餅。

---

### A-3. `#[serde(deny_unknown_fields)]` と `#[serde(flatten)]` は**併用不可** — §3/§4/§7 が相互矛盾【確信度: 高】

**どこが**: §3.1 と §7 で「全 DTO に `deny_unknown_fields` を付与」と宣言。同時に §4.1 で `ApiEnvelope<T>` は `#[serde(flatten)] data: T`。

**なぜ問題か**: serde の仕様上、`deny_unknown_fields` は `flatten` を含む struct（および flatten される側）と**共存できない**（serde が「未知フィールド」を判定できなくなるため、コンパイルは通るが Deserialize が実行時に必ず失敗するか、serde のドキュメントで明示的に非対応）。req DTO に `deny_unknown_fields` を付けて 400 を返す設計（§7 の後方互換・typo 遮断の核心）と、エンベロープ flatten 設計が同一方針の下で両立しない。「全 DTO に付与」は達成不能。

**どう直すべきか**: `deny_unknown_fields` は **flatten を含まない葉の req/res DTO に限定**すると明記し、「全 DTO」を撤回。エンベロープを A-1 の通り具象化すれば flatten が消え、この矛盾も自動的に解消する（A-1 の修正が A-3 を巻き取る）。

---

## B. 重大リスク（放置すると後で破綻）

### B-1. 「Serialize を実装しないから漏れない」は axum の `Extension`/ログ/エラー経路を塞げていない【確信度: 中〜高】

**どこが**: §3.1「`BotRecord` に `Serialize` が無いので axum `Json(...)` に渡せない＝コンパイルエラー＝フェイルオープン不可能」。

**なぜ問題か**: これは「JSON レスポンスボディ」経路のみを塞ぐ。実際の漏洩面はもっと広い:
- **`Debug` 派生**（§3.1 の `BotRecord` は `#[derive(Debug)]`）。`tracing::error!("{:?}", rec)` や panic メッセージ、supervisor の再起動ログ（制約2）で暗号文・IV が**そのままログへ出る**。ログ集約基盤に機密が流れる。zod strip では起きなかった新規漏洩面。
- **`serde_json::to_value` 経由の間接シリアライズ**（監査ログ `auditRepo`、エラーレスポンスに `context` を載せる実装）で、`Serialize` を持つ別型に `BotRecord` のフィールドを詰め替える人的ミスは型では防げない。
- `Ciphertext` newtype の「Serialize 実装追加を grep で禁止」（§3.3-2, §5.4）は**脆い**。`impl Serialize for Ciphertext` という正確な文字列にしかマッチせず、`#[derive(Serialize)]` 付与、`serde(with=...)`、`to_vec()` での中身取り出し後の別型格納、マクロ展開経由をすべて素通しする。grep ガードをセキュリティ境界の最終防波堤に据えるのは危険。

**どう直すべきか**: (a) `Ciphertext` に手書き `Debug`（`f.write_str("Ciphertext(<redacted>)")`）と、`Display` 非実装を義務化。`BotRecord` の `Debug` も redaction カスタム実装、または `secrecy` クレートの `Secret<T>`（`Debug`/`Serialize` を構造的に封じる実績ある型）を採用。(b) grep ガードは補助と位置づけ、主防御を型（`secrecy`）に置く。(c) 機密を含む型がログ/監査経路に渡らないことを、`clippy` の `disallowed_methods`（`serde_json::to_value` を機密型に対して禁止する lint は書けないため）ではなく**アーキテクチャ（機密は復号時のみ短命に存在し record に保持しない）**で担保する方針を明記。

### B-2. drift 検査 `--check` はコミット済み生成物を「信頼できる真実」と仮定するが、生成器バージョン差で偽陰性/偽陽性【確信度: 中】

**どこが**: §5.4。`cargo xtask gen-types --check` で byte 比較。

**なぜ問題か**: ts-rs のマイナーバージョン更新で出力フォーマット（インデント・改行・`type` vs `interface`・null 表現）が変わると、DTO を一切変えていないのに全 generated ファイルが drift 判定になり CI が赤くなる（偽陽性）。逆に開発者ローカルの ts-rs が古いと、CI を通した生成物と手元が食い違う。生成物をコミットする方針（フロント開発者が Rust ビルド無しで読むため、という理由は妥当）と byte 一致検査の組み合わせは、**生成器バージョンを Cargo.lock で厳密固定し、CI と同一トリップで生成した場合のみ健全**。

**どう直すべきか**: (a) `ts-rs` を `=10.x.y` で完全ピン（`^` 禁止）し Cargo.lock をコミット。(b) drift 検査は生成器バージョンも比較対象に含める（生成ヘッダに ts-rs バージョンを刻み、不一致は「再生成せよ」の明示エラーに）。(c) CI の生成環境を Docker rust-builder と同一イメージに固定し「ローカルで生成 → CI で再生成 → byte 比較」の環境差をゼロにする。

### B-3. `has_gemini_key`/`has_token`/`running`/`connected` の派生 bool を Rust `bool`、既存 int 列を `i64` にする型ズレ【確信度: 中】

**どこが**: §3.1 の `BotView`。`suspended: i64`, `has_gemini_key: bool`, `running: bool`。現行 zod は `suspended: z.number()`（TS `number`）、`has_gemini_key: z.boolean()`。

**なぜ問題か**: `suspended` を `i64` にすると ts-rs は `number` を吐き一致するが、**SQLite に boolean 型が無い**ため DB では `suspended` は 0/1 の integer。`running`/`connected`/`shared` は runtime 由来の真の bool でよいが、`suspended` を DB から `bool` に読むか `i64` のまま出すかで**フロント契約が変わる**。ドラフトは `suspended: i64` を選び現行 `z.number()` と揃えたが、これは「本来 bool の意味を int で運ぶ」現行の技術的負債を Rust 側でも固定化する選択で、単一真実源化の好機を逃している。判断自体は「破壊を避ける」で妥当だが、**意図的選択であることの明記が無い**（無自覚だと後で誰かが `bool` に直してフロントを壊す）。

**どう直すべきか**: `suspended: i64`（現行互換）を選ぶなら「意味は bool だが現行 TS 契約維持のため int 型を保持」とコメント義務化。将来 bool 化するなら移行対象として §6 のドメイン移行リストに載せる。

---

## C. 見落とし・整合性の穴

### C-1. `SessionUser`・`camelCase` 混在 DTO の生成方針が未定義【確信度: 中】
`types.ts` は `SessionUser { discordId, username, role }`（camelCase）と `BotView { user_id, created_at }`（snake_case）が**同一ファイルに混在**することを確認した。ドラフトは §4.2 で「struct に `rename_all="camelCase"` を付ける」と個別対応を示すが、**どの DTO が camel でどれが snake かの棚卸しが無い**。101 型を移行する際、per-struct で casing を実レスポンスに手で合わせるのは事故の温床（§6-4 のキー名乖離と同根）。→ 移行前に「現行 TS 各型の実 casing 一覧」を作り、Rust 側 `rename_all` 指定の正誤を型チェックで担保する工程を §6 に追加すべき。

### C-2. userId データ分離（不変条件）の型レベル表現が無い【確信度: 中】
`apiViews.ts` の `contactViewSchema` コメントは「user_id/bot_id は不可」と明示（確認済み）。だが `botViewSchema` は `user_id` を**含む**（L13）。つまり「user_id を出す/出さない」はビューごとに異なる判断で、現行はコメント + zod allowlist で管理。ドラフトの型分離設計は「機密列」は塞ぐが、**「他ユーザーの行を取ってこない」データ分離（WHERE user_id = ?）は型では守れない**——これはクエリ層の不変条件で、型連携領域の外だが、ドラフトが「zod allowlist を構造的に置換」と大書しているため**「allowlist が守っていたのは列 strip だけで、行分離は別レイヤ」という切り分けの明記が無い**と、読者が「型分離で userId 分離も守られる」と誤読する。→ 「本設計は列レベル漏洩のみ解決。行レベル（userId scoping）はリポジトリ層クエリの責務で別領域」と明記必須。

### C-3. CSRF/CSP・`credentials:same-origin`・401 集中割り込みへの言及ゼロ【確信度: 中】
`client.ts` は same-origin cookie・401 集中リダイレクト・`success:false` 複合判定という**セキュリティ/UX 不変条件**を持つ（確認済み）。ドラフトは「client.ts は不変（scope 規約はここ）」と 1 行で退けるが、axum サーバ移行後に**CSRF 対策（現行が cookie ベースなら SameSite/CSRF token）・CSP ヘッダ**を誰がどう維持するかが型連携領域で触れられていない。型は運べても、`401→currentUser.set(null)` や `200+success:false` を返す**サーバ側の応答契約**が Rust 実装で再現される保証が本ドラフトに無い。→ 少なくとも「エンベロープの `success:false` セマンティクス（HTTP 200 でも業務失敗）を Rust axum が維持する」ことを DTO 契約の一部として明記し、device flow 同様「client.ts の複合判定を壊さない応答形状」を制約に加えるべき。

### C-4. `SetupStatusResponse` の `rename_all` と `skip_serializing_if` の組み合わせ検証欠落【確信度: 低】
§4.2 例外A で `need_setup`→`needSetup` の rename と `Option` フィールドの `skip_serializing_if` を併用。ts-rs は `skip_serializing_if` を型に反映しない（`Option<T>` は常に `T | null` を吐く）ため、**サーバは省略するがフロント型は `null` を期待しない `terms_url?: string`** という現行 optional（`?`）との差が出る。ts-rs は `Option<T>` を `T | null` にするか `T | undefined`（`optional_fields` 設定依存）にするかでフロント契約が変わる。→ ts-rs の `#[ts(optional)]` を明示し `?:` を出させるか、`Option` の TS 表現方針（`| null` vs `?`）を全体で 1 つに固定する必要がある。現行 `types.ts` は `?:`（undefined）なので `#[ts(optional)]` 統一が要る。

---

## D. 妥当と認める点（辛口でも認めるべき箇所）

- **型のみ生成 + ルート契約は axum に一本化（utoipa 却下）**: 妥当。utoipa 属性文字列とルータの二重管理は制約5に反するという分析は正しい。
- **DB row 型と wire DTO 型の分離**: 方向性は妥当。「Serialize 不在＝コンパイルエラー」は zod のランタイム strip よりコンパイル時に強い、という主張は JSON ボディ経路に限れば正しい（B-1 の Debug/ログ面を塞げば真に強い）。
- **From ではなく型付きエラー関数（`bot_view` + `BotViewError`）**: 制約1に正しく従っている。runtime 状態を要する変換を infallible `From` にしない判断は的確。
- **段階移行のバレル再輸出 + 同名衝突で TS がエラー（フェイルクローズ移行）**: §6-6 の「移行漏れが型で検出される」は妥当で、big-bang を避ける判断も正しい。
- **req DTO も生成（§7）**: camel/snake 語彙不整合を型に載せる方針は妥当（ただし A-3 の deny_unknown_fields 制約に服する）。
- **xtask 採用・build.rs 却下**: CI drift 検査との相性で build.rs を避ける理由は妥当。
- **引用の正確性**: 全参照ファイルの実在と行内容を確認、齟齬なし。

---

## 総括（優先順位）

計画の骨格（型のみ生成・型分離・段階移行）は健全だが、**具体メカニズム 3 点が ts-rs/serde/rusqlite の実挙動と衝突**しており、このまま実装に入ると最初のスパイクで詰まる:

1. **A-1（flatten→交差型は ts-rs で不可）** — エンベロープ設計を具象 DTO + 手書き交差 alias に差し替え。§0 推奨表の対 utoipa 論拠も要修正。
2. **A-3（deny_unknown_fields × flatten 併用不可）** — A-1 修正に連動して解消するが「全 DTO に付与」は撤回。
3. **A-2（rusqlite 同期 × async マルチコア）** — 本ドラフトが workspace 新設の親を名乗る以上、DB 並行モデル（spawn_blocking + pool + WAL）を設計に取り込む。
4. **B-1（Debug/ログ経由の機密漏洩）** — `secrecy` 等で Serialize だけでなく Debug も封じる。grep ガードは主防御にしない。

C-2/C-3（userId 行分離・CSRF/CSP は本領域外で別レイヤの責務、と明記）は「握り潰し」ではなく**責任境界の宣言漏れ**であり、明記すれば足りる。

最も危険なのは A-1: 「生成 TS が現行と字面一致」という**未検証の期待を設計の中核根拠に据えている**点。実装前に ts-rs で `ApiEnvelope<T>` の flatten を 1 本スパイクして出力を目視確認することを、着手の第一ゲートにすべき。

**関連ファイル（絶対パス）**:
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/types/apiViews.ts`（zod 3ビュー・48行・`user_id` を含む botView / `contactView` は user_id 除外）
- `/home/suki/web/kawaii-music.moe/apps/yuuka/frontend/src/lib/api/types.ts`（`ApiResponse<T>` L13 交差型・SetupStatus L32 camelCase 例外・BotView L51 snake_case・SessionUser camelCase 混在）
- `/home/suki/web/kawaii-music.moe/apps/yuuka/frontend/src/lib/api/client.ts`（same-origin/401集中/`success:false`複合判定＝維持すべき応答契約）
- `/home/suki/web/kawaii-music.moe/apps/yuuka/frontend/src/lib/api/device.ts`（OAuth 200+error 例外）
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/db/userRepo.ts`（`gemini_api_key_encrypted/_iv/_tag`・`google_refresh_token_*` 機密列 SELECT の実在）
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/rust_synapse/Cargo.toml`（`rusqlite 0.32 bundled`・独立クレート・未 workspace 化を確認）

---

## [19] All claims verified against the real codebase. I now have enough to write the ad
<sub>agentId: a1002ab8858dd6b41</sub>

All claims verified against the real codebase. I now have enough to write the adversarial review.

# 領域C 設計ドラフト レビュー（辛口・反証）

裏取りは実ファイルで確認済み。以下、計画の成否に関わる論点を優先度順に。各指摘は「どこが/なぜ/どう直すか」＋確信度。

---

## 1. 絶対制約の充足（厳格エラー処理・自己復帰）

### C-1. 【致命・確信度:高】ファクトリ方式の supervisor が二重 `tokio::spawn` で panic を捕捉するが、`panic="unwind"` でも「アボートする panic」を取りこぼす
**どこ**: §C.1.3 の `tokio::spawn(async move { svc.run(tok).await }).await` で `JoinError::is_panic()` に頼る設計全体。
**なぜ**: (a) tokio のタスク境界で捕捉できるのは **unwind する panic のみ**。だが `run()` 内で別途 `std::thread::spawn` した OS スレッド（`spawn_blocking` は tokio 管理だが、rusqlite の C コールバックや FFI、あるいはユーザー拡張が張る生スレッド）で panic すると、そのスレッドの panic は**プロセス全体を abort** する（Rust のデフォルトで、キャッチする境界が無いスレッドの panic は `std::process::abort`）。supervision tree は無力。(b) さらに深刻なのは、Rust の panic の一部は unwind ではなく即 abort になる経路がある——**panic-in-panic（Drop 中の panic）**、`alloc` エラー、スタックオーバーフロー。これらは `panic="unwind"` でも捕捉不能でプロセスが死ぬ。設計は「アクター単位再起動が最大の勝ち筋」と謳うが、この前提は**限定的にしか成り立たない**。
**どう直すか**: (1) 「panic 隔離でプロセスは絶対落ちない」という強い主張をトーンダウンし、**プロセス abort は依然あり得る前提で、systemd `Restart=always` を最終防衛線として明記**（現行の `uncaughtException→exit(1)→systemd 再起動` を Rust でも残す。JS を「劣る」と断じた C.0/C.7 の論調は誤り）。(2) `spawn_blocking` 内は `std::panic::catch_unwind` で明示的に包み、DB アクターが OS スレッド panic で消えないようにする。(3) ユーザー拡張が生スレッドを張る可能性に対し、拡張 API では生スレッド禁止 or `catch_unwind` 必須をトレイト契約に書く。

### C-2. 【致命・確信度:高】`FATAL_TX` / `fatal_tx` の設計に、致命が発生したのに「送信先が既に drop」で握り潰される穴。しかも制約1違反
**どこ**: §C.1.2 の `FATAL_TX.send(...).ok()` と §C.1.3 の `let _ = fatal_tx.send(...)`。
**なぜ**: `.ok()` と `let _ =` は**まさに制約1が禁じる「失敗の黙殺」**。ここは「致命エラーが起きたのでプロセスを落とす」という最重要経路であり、送信失敗＝ルート supervisor が既に落ちている＝**最も落としてはいけない取りこぼし**。制約1は `let _ = fallible()` を明示禁止、`clippy::let_underscore_must_use = "deny"` を自分で設定しているのに、設計本文がそれに違反している。自己矛盾。
**どう直すか**: `fatal_tx` を `unbounded` にして送信失敗が起きない設計にするのは弱い（rx drop はあり得る）。正しくは、致命通知を**チャネルではなく `Arc<AtomicBool> + Notify` か `tokio::sync::watch`** にし、送信は必ず成功する形にする。加えて「fatal を通知できなかった」ケースの最終手段として `std::process::exit(70)` を supervisor 内に直書きする（起動パス外だが「致命の伝播不能」は fail-fast が正しく、制約1の趣旨に沿う）。`.ok()`/`let _=` は全廃。

### C-3. 【重大・確信度:高】`Supervised<RedisCache>` を supervision tree に載せる図と、C.2.4 の「main は Redis 接続を待たない／最初から Memory で起動」が矛盾
**どこ**: §C.1.1 のツリー図に `Supervised<RedisCache> 再接続ループ` があるが、§C.2.4 は「背景タスクが `PING` で監視」「`AtomicBool ready`」で `CacheBackend` を論理切替、と別モデルを提示。
**なぜ**: Redis は「再起動される長寿命サービス（Service トレイト実装）」なのか「常駐する接続監視タスク＋論理フラグ」なのか、二つの設計が併存。前者だと `CacheBackend::Redis(pool)` を握っている呼び出し側（axum ハンドラ、レート制限）が、Redis サービス再起動のたびに**古い pool ハンドルを掴んだまま**になる。`ArcSwap<CacheBackend>` 相当のディスパッチ機構が図に無い。
**どう直すか**: Redis は Service ではなく「`Arc<ArcSwap<CacheBackend>>` を更新する背景 liveness タスク」に一本化し、ツリー図から `Supervised<RedisCache>` を削る。呼び出し側は毎回 `load()` で最新 backend を取る（Bot の `ArcSwap<BotHandle>` と同じ機構を再利用）。

### C-4. 【重大・確信度:中】`reset_after` バックオフリセットが「即クラッシュ→即再起動」のフラッピングを止められないケース
**どこ**: §C.1.3 `if started.elapsed() >= cfg.reset_after { attempt = 0; }`。
**なぜ**: `run()` が factory 生成直後に即 `Err(Fatal)` を返す場合は return するので問題ない。が、**回復可能エラーで即死を繰り返す**（例: crawler バイナリが壊れていて起動即 exit）と、`started.elapsed()` は常に小さいので `attempt` は増え続けバックオフは効く——ここは正しい。だが逆に、`reset_after`（例30s）**ちょうど超えた直後に毎回死ぬ**サービスは、毎回 `attempt=0` にリセットされ、**永久に base_backoff（500ms）で高頻度再起動**し続ける。30s ごとの CPU スパイク・ログ洪水・上流への再接続嵐。現行 JS には無いが Rust 設計が新規に導入するリスク。
**どう直すか**: 「連続再起動回数の絶対上限（例: 5分窓で N 回超えたら `Transient` 降格＋アラート）」を circuit-breaker 的に追加。`reset_after` 単独では不十分。ヘルスの `restarts` カウンタを窓集計して閾値判定する。

### C-5. 【重大・確信度:高】stuck 検出 watchdog（§C.4.2）が `CancellationToken` 発火で回復するという主張は、デッドロックには効かない
**どこ**: §C.4.2「デッドロック等で無言のまま固まるケースを回復対象にできる」→ `CancellationToken` 発火 → supervisor 再起動。
**なぜ**: `CancellationToken` は**協調的キャンセル**。タスクが `select!` で `token.cancelled()` を待つ地点に到達して初めて効く。真にデッドロックした（mutex 二重 lock、`spawn_blocking` 内の無限ループ、同期 I/O ハング）タスクは**キャンセルポイントに永久に到達しない**ので、トークンを発火しても止まらない。tokio はタスクを強制 kill できない。よって「固まる障害を回復対象化」は**実現しない**（`abort()` してもキャンセルセーフでない箇所では効かない、`spawn_blocking` スレッドは abort 不可）。
**どう直すか**: (1) デッドロック回復は「当該サブシステムのプロセス分離」でしか根本解決しない（daemon は別プロセスなので kill 可能——だから daemon 化する価値がある、と正直に書く）。(2) インプロセスのタスクについては watchdog は「検出してメトリクス/アラート＋readiness を Down にする」までに留め、「自動回復する」と過大宣伝しない。(3) 本当に stuck 回復が要件なら、そのサブシステムを子プロセス側へ寄せる。

### C-6. 【中・確信度:中】`fail_all_pending` の `let _ = tx.send(...)` を「唯一許容」と自己認定しているが、制約1の抜け道を作る前例になる
**どこ**: §C.2.3 の `let _ = tx.send(Err(cause.clone()));`。
**なぜ**: 主張自体（oneshot の受信側 drop は無害）は技術的に正しい。が、「ここだけは許す」を設計本文で認めると、`clippy::let_underscore_must_use = "deny"` を機械強制する方針と**運用上ぶつかる**（`#[allow]` の乱発の起点になる）。制約1は「すべての失敗経路に名前と型」を要求している。
**どう直すか**: `oneshot::Sender::send` の戻り値は `Result<(), T>`（送れなかった値が返る）。`if tx.send(...).is_err() { metrics::pending_receiver_gone.inc(); }` として**握り潰さずカウントする**。`let _` を使わずに済み、制約1に完全準拠。

---

## 2. 技術的正当性（クレート/API/罠）

### C-7. 【重大・確信度:高】axum の `CatchPanic` は panic="abort" 下では機能せず、かつ「接続単位で隔離」の記述が誤り
**どこ**: §C.2.2「tower の `CatchPanic` レイヤ＋タスク境界で500に落とし、接続単位で隔離」。
**なぜ**: (a) `tower_http::catch_panic::CatchPanicLayer` は内部で `catch_unwind` を使うため `panic="unwind"` が前提——これは Cargo 設定と整合。ただし (b)「**接続単位**で隔離」は不正確で、実際は**リクエスト（ハンドラ future）単位**。より重要なのは (c) axum/hyper は各リクエストを個別タスクで回すので、`CatchPanic` を使わずとも 1 ハンドラ panic が他接続を巻き込むことは元々ない。設計が「多層防御」を強調する割に、粒度の記述が甘い。実害は小さいが「Rust なら安全」の論拠として使うには精度不足。
**どう直すか**: 「リクエスト単位で `CatchPanic` により 500 応答＋panic を握る」と正確化。`CatchPanic` の panic ハンドラは**ログ＋メトリクス必須**（でないと制約1的に panic が消える）。

### C-8. 【致命・確信度:高】単一の真実源（Rust）→ TS 型自動生成が、現行の zod ランタイム fail-close を捨てる設計欠落
**どこ**: 制約5への言及がドラフト本文にほぼ無い（C 領域の主題外ではあるが、`apiViews.ts` は 48 行の zod allowlist で**ランタイムに `parse()` で strip する**フェイルクローズが核心。確認済み）。ts-rs / specta 等での型生成は**コンパイル時の型定義**しか出さず、**ランタイムの機密列 strip をしない**。
**なぜ**: Rust の `#[derive(Serialize)]` した struct をそのまま返せば、struct にフィールドが無い＝出ない、で strip 相当は成立する。しかし現行の防御は「生 DB レコード（`*_encrypted`/`*_iv`/`password_hash`）がシリアライザに紛れても構造的に落ちる」という**フェイルクローズ**。Rust で `serde_json::to_value(db_row)` 的な動的経路や `#[serde(flatten)]`、`HashMap<String,Value>` を一箇所でも使うと機密が漏れる。型生成は「二重管理解消」には効くが「機密フェイルクローズ」を**自動では保証しない**。C 領域は自己復帰が主題だが、制約5は全設計に反映必須と明記されている——ここが空白。
**どう直すか**: (1) API 応答 struct は**専用 View struct のみ**（DB struct を直接 Serialize しない）をアーキテクチャ規約化。(2) DB struct に `#[derive(Serialize)]` を付けない（付けたら漏れ得る）ことを **cargo clippy カスタム lint or grep CI で機械強制**。(3) 型生成は View struct からのみ。この規約を C 領域の「拡張性」節（ユーザー拡張が API を生やす場合）にも波及させる必要がある——現状ノーガード。

### C-9. 【重大・確信度:中】twilight のマルチシャード per-token モデルと「BotActor=1タスク+mpsc で直列化」がスループットのボトルネックになり得る
**どこ**: §C.2.1「各 BotActor = 1タスク + コマンド `mpsc`」。
**なぜ**: `Start/Stop/ReplaceToken` を直列化するのは正しい（起動レース解消）。だが記述が曖昧で、**イベント処理まで 1 タスク直列**に読める。1 bot が多数ギルド/DMで同時にメッセージを受けると、1タスク直列では**現行 Node（イベントごとに非同期並行）より遅くなる**恐れ——制約3（現行より高速）に反する。制御コマンドの直列化とイベント処理の並行は分離すべき。
**どう直すか**: 「制御プレーン（Start/Stop/Replace）は 1 アクターで直列」「データプレーン（受信イベント処理）は `tokio::spawn` で per-event 並行、共有状態は `Arc`」と**明確に二層化**。ArcSwap<BotHandle> の load はデータプレーン側。

### C-10. 【中・確信度:中】rusqlite 書き込み単一アクター + `spawn_blocking` の組み合わせで、WAL 下でも「単一書き手アクター」がスループット上限になる
**どこ**: §C.2.5「書き込みは単一専用コネクション + アクター（mpsc で全書き込みを直列化）」。
**なぜ**: SQLite は元々書き込みが単一なので直列化自体は妥当。だが `spawn_blocking` はデフォルトのブロッキングスレッドプール（512上限）を使い、**書き込みアクターが 1 コネクションを 1 スレッドに固定**するなら、全書き込みが 1 スレッド分の速度に律速。現行 better-sqlite3 は**同期呼び出しでイベントループを直接ブロック**する（速いが単一スレッド）。Rust 版が「マルチコアを活かす」（制約3）と謳う一方、書き込みは結局単一——ここは**現行と同等以上を主張するなら測定前提**にすべき。読み取りプールの並行化は正しい。
**どう直すか**: 「書き込みは SQLite の制約上単一で、現行と同等（劣化なし）。高速化は主に読み取り並行化とマルチコア他サービスから得る」と正直に位置づけ、制約3の根拠を「書き込みが速くなる」に置かない。バッチ書き込み（トランザクション束ね）を明記。

### C-11. 【中・確信度:高】`fastrand::u64(0..=capped.as_millis() as u64)` のフルジッタが「最低待機ゼロ」を許し、thundering herd 対策が逆効果になり得る
**どこ**: §C.1.3 `backoff_delay`。
**なぜ**: フルジッタは `[0, capped]` の一様乱数。jitter が 0 近傍を引くと**即再試行**になり、フラッピング時に上限を効かせたい意図に反する。またフルジッタは正しくは「AWS 推奨の full jitter」だが、`capped` が `base*2^attempt` の**キャップ後**の値に対して 0〜cap を引くので、attempt が増えても最短ゼロは変わらず、収束が遅い。加えて `as u64` キャストで `Duration` が大きいと `as_millis()`（u128）→u64 で理論上切り詰め（実害は小、上限30sなら問題なし）。
**どう直すか**: 「decorrelated jitter」か、最低下限を設けた `[base/2, capped]` に。`sleep(0)` を防ぐ下限を必須化。

### C-12. 【中・確信度:中】`1u32 << attempt.min(6)` と `base.saturating_mul` の順序で早期オーバーフロー飽和
**どこ**: §C.1.2 `backoff_delay` の `base.saturating_mul(1u32 << attempt.min(6))`。
**なぜ**: `Duration::saturating_mul` は `u32` 引数。`1u32 << 6 = 64`。base=500ms なら 32000ms、min(max=30s) で 30s——動く。ただし §C.1.2 版と §C.1.3 版で `backoff_delay` の実装が微妙に違う（前者は shift、後者は本文に式なし）。**二版併存**でどちらが正か不明。
**どう直すか**: `backoff_delay` を一箇所に確定。`attempt.min(6)` の妥当性（64倍で cap 到達）をコメントで固定。

---

## 3. 既存機能の完全性（不変条件の欠落）

### C-13. 【重大・確信度:高】現行の CSP / Origin-based CSRF / SameSite を Rust 移行設計が一切継承していない
**どこ**: §C.2.2 Web サーバ節。確認したところ現行 `server.ts:83` に厳密な CSP（`frame-ancestors 'self'` 等）、`server.ts:284-306` に Origin allowlist ベースの CSRF 防御、Bearer は CSRF 非該当という設計が実装済み。ドラフトは `/healthz` `/readyz` `/metrics` は書くが、**これらセキュリティ不変条件の移植に無言**。
**なぜ**: axum 移行で tower のミドルウェア順序を誤ると CSP ヘッダ欠落・Origin チェック漏れが起きる。制約3の「堅牢」を謳う設計が、実装済みの防御層をスコープ外扱いしている。特に `/metrics`（管理者のみ）と書くが**認証ミドルウェアの適用範囲**が未設計——Prometheus エンドポイントは内部情報漏洩源。
**どう直すか**: Web 節に「移植必須の不変条件」チェックリストを追加: CSP ヘッダ（`server.ts:83` の内容を型付き定数へ）、Origin allowlist CSRF、SameSite cookie、`/metrics` の管理者認証。tower ミドルウェア適用順序を明示（CSP は全レスポンス、CSRF は Cookie 経路のみ）。

### C-14. 【重大・確信度:高】userId データ分離（全 DB クエリの `WHERE user_id = ?` スコープ）が supervision/縮退設計で触れられていない
**どこ**: 全体。現行は `botAttributesRepo.ts:89` 等、全リポジトリが `user_id`/`bot_id`/`guild_id` でスコープ。制約5の `apiViews` はスコープキーを応答から strip。
**なぜ**: Redis→Memory 縮退時（§C.2.4）、in-memory のセッション/レート制限/oauth_state が**userId スコープを保持しているか**が未記述。特に「セッションを Redis 復帰後も memory と読み合わせる移行救済」は、**userId 越境で他人セッションを引く**リスクの温床。in-memory 実装のキー設計を誤ると分離が壊れる。SynapseScope（`user_id/bot_id/guild_id`、確認済み）を daemon IPC で渡す設計はあるが、縮退時の「直近履歴のみ」がスコープ内履歴に限定される保証が本文に無い。
**どう直すか**: 縮退フォールバック（Memory cache、synapse degraded）すべてに「userId/botId/guildId スコープを縮退経路でも保持」を不変条件として明記。in-memory cache のキーは必ずスコープキー prefix 付き。`RecallOutcome::Degraded` の「直近履歴」がスコープ内 SQLite クエリ由来であることを型/コメントで固定。

### C-15. 【中・確信度:中】notifier のライブバインディング（`bot.ts:78 export let client` / `:1441 client=newClient`）を ArcSwap 化する際の「取りこぼし」設計が浅い
**どこ**: §C.2.1「notifier 相当は `handle.load()` で常に最新シャードを掴む」。
**なぜ**: 現行は ES module のライブバインディングで、`restartDefaultBot` 後に notifier（`sendToUser`）が自動的に新 Client を見る（確認済み `bot.ts:76,1441`）。ArcSwap 置換は妥当だが、**トークン差し替えの瞬間に飛んでいる通知**（リマインド送信中に `ArcSwap::store`）が旧ハンドルで失敗した場合の扱いが未設計。§C.3 の Discord HTTP breaker の再送キューと連携すべきだが、リンクされていない。
**どう直すか**: 差し替え中の送信失敗は `NotifyError` として再送キュー（§C.3）へ回す明示的経路を書く。ArcSwap の load→send の間の race に対し、send 失敗時 1 回だけ load しなおしてリトライ。

### C-16. 【中・確信度:中】crawler 縮退で Puppeteer フォールバック喪失を「許容範囲」と断じているが、機能後退の影響評価が無い
**どこ**: §C.2.3「Rust 全面移行では Puppeteer が無いため…LLM がユーザーに『Web 取得に失敗』と伝える…許容範囲」。
**なぜ**: 現行は crawler ダウン時に Puppeteer フォールバックで**機能継続**（棚卸し#5 §2.1）。Rust 版はフォールバック無しで**機能停止**。これは「劣化縮退」ではなく**機能削減**。crawler（Chrome ベース）が頻繁クラッシュし得ると自ら認めている（§C.2.3）以上、フォールバック喪失は**可用性の実質低下**。制約3「現行より堅牢」と矛盾しかねない。
**どう直すか**: 「crawler の再起動を eager 化してフォールバック不在を補う」か、「軽量な reqwest ベースの静的取得フォールバックを Rust 側に持つ」を代替として設計。「許容範囲」の一言で流さず、可用性トレードオフを明記。

---

## 4. 移行実現性

### C-17. 【重大・確信度:高】段階移行の並行実装で、Node と Rust が同一 SQLite に同時アクセスする期間の「単一書き手」不変が壊れる
**どこ**: 全体（移行戦略が本文に無いこと自体が問題）。現行は「書き手は Node のみ」を前提に `busy_timeout` すら未設定（§C.2.5 が指摘）。
**なぜ**: 全面書き換え中、機能ごとに Rust へ切る並行運用をすると、**Node と Rust の 2 プロセスが同じ DB に書く**。WAL でも書き込みは単一で、両者が書けば `SQLITE_BUSY` 頻発＋現行 Node の未設定 `busy_timeout` で即エラー。Rust 側の「単一書き手アクター」も**プロセス内**の単一性しか保証しない——別プロセス Node がいれば無意味。
**どう直すか**: 移行戦略を明記。選択肢: (a) DB 書き込みは移行完了まで片方に固定（Rust を読み取り専用サイドカーから始める）、(b) 書き込みを Rust daemon に一本化し Node は IPC 経由、(c) ビッグバン切替（並行書き込み期間を作らない）。少なくとも「並行書き込み期間中は両者 `busy_timeout` 設定必須」を前提条件化。

### C-18. 【中・確信度:中】9本 cron を「1タスクで tick」する設計が、1ジョブの長時間実行で他ジョブを遅延させる
**どこ**: §C.1.1 `CronScheduler 9本の cron ジョブを1タスクで tick`。
**なぜ**: backup（Google Drive 全体、§C.3 で breaker 対象＝遅い可能性大）や report が 1 tick 内で同期的に走ると、同 tick の reminder/birthday が遅延。現行は各 `startXxxService` が独立タイマー（`index.ts:114-122` 確認）で**互いに独立**。1タスク集約は**現行の独立性を後退**させ、制約3に反する。
**どう直すか**: cron は「1タスクでスケジュール判定、実行は `tokio::spawn` で切り離す」か、重いジョブ（backup/report/briefing）は別 supervised タスクに分離。`last_ok` watchdog（§C.4.2）が「9本を1タスク」だと**どのジョブが stuck か判別不能**な問題も併発するので、ジョブ単位の liveness に。

### C-19. 【中・確信度:中】`async_trait` 採用がホットパスに乗ると割り当てコスト。かつ将来の native async trait への移行負債
**どこ**: §C.1.2 `#[async_trait::async_trait] pub trait Service`。
**なぜ**: Service::run は長寿命で 1 回しか呼ばれないので `async_trait` の Box 割り当ては無害——ここは妥当。だが拡張性（制約4）で「ユーザー拡張が Tool トレイトを大量実装」する場合、`async_trait` を Tool のホットパスに使うと per-call Box 割り当て。2025 時点で Rust は native async fn in trait が安定化済み（`impl Trait` 制約はあるが dyn 不要な箇所は native 化可能）。
**どう直すか**: Service（低頻度）は `async_trait` で可。Tool 等ホットパス trait は native async fn in trait or 手書き Future を検討、と使い分けを明記。

---

## 妥当と認める点（過度に叩かない）

- **回復/致命の二分マトリクス（C.0）**: 各分類→thiserror バリアント→網羅 match の骨格は制約1・2に正しく沿う。**妥当**。
- **ファクトリ方式で再起動時に新インスタンス生成**（現行 `restartDefaultBot` の「destroy 済み Client 再利用しない」と一致、`bot.ts` で確認）: **妥当**。
- **bind/migration/secret を起動前 fail-fast、exit code を sysexits に対応**: 現行 `index.ts:61-89` の哲学を正しく型化。**妥当**。
- **synapse/crawler のプロセス分離維持**（障害分離）: 技術的に正当。**妥当**（ただし C-16 の可用性後退は別問題）。
- **rate-limit の `incr→expire` 非原子性を Lua/`SET NX EX` で解消**: 現行弱点の正しい是正。**妥当**。
- **cargo-deny で anyhow/eyre を依存グラフごと ban**: 制約1の機械強制として有効。**妥当**（ただし C-2/C-6 の本文自己違反を先に直すこと）。

---

## 総括（計画の成否に関わる Top 5）

1. **C-2 / C-6**: 設計本文が自ら `.ok()` / `let _ =` で制約1を破っている（致命通知の握り潰し）。**最優先で全廃**。
2. **C-1 / C-5**: 「panic 隔離でプロセスは落ちない」「stuck も自動回復」は過大宣伝。**OS スレッド panic・abort 経路・非協調キャンセルの限界を認め、systemd 再起動を最終防衛線として残す**。JS を劣ると断じた論調は要修正。
3. **C-8 / C-13 / C-14**: 制約5（機密フェイルクローズ）・CSP/CSRF・userId 分離という**現行の実装済みセキュリティ不変条件**が設計から抜けている。「全設計に反映必須」の制約に対する空白。
4. **C-17**: 並行移行期の**単一書き手不変の崩壊**。移行戦略が本文に無いこと自体がリスク。
5. **C-3 / C-9 / C-18**: supervision tree 図と本文の**モデル不整合**（Redis の二重設計、Bot の直列/並行未分離、cron 集約による独立性後退）。制約3の高速性根拠が弱い。

参照実ファイル: `/home/suki/web/kawaii-music.moe/apps/yuuka/src/index.ts:61-210`、`src/db/redis.ts:10-119`、`src/services/synapseEngine.ts:78-120`、`src/services/browserService.ts:174-259`、`src/types/apiViews.ts:1-48`（48行・zod strip fail-close）、`src/server.ts:83,284-306`（CSP/Origin-CSRF）、`src/bot.ts:76-78,1427-1441`（notifier ライブバインディング）、`frontend/src/lib/api/types.ts`（655行・手書き重複）。

---

## [20] I have all the evidence I need. Let me compile the adversarial review.
<sub>agentId: a81b36bdf1848e195</sub>

I have all the evidence I need. Let me compile the adversarial review.

# 領域C設計ドラフトへの辛口レビュー（反証）

裏取りの結果、ドラフトの現行コード理解（registry の `{success:false}` 正規化・重複throw・戻り値文字列規約、bot.ts の `restartDefaultBot`/`customClients`/`startInFlight`/`claimMessageOnce`/`readyAt`、apiViews の zod strip allowlist、AES-256-GCM 暗号化）は**概ね正確**。その上で、計画の成否に関わる欠陥を優先度順に列挙する。

---

## 【最重要】1. SQLite が同期ドライバ前提であることを設計が完全に無視している（確信度: 高）

**どこが問題か**: ドラフトは「単一 tokio ランタイム内のアクター群」「マルチコアを活かすマルチスレッド」を大前提にしているが、現行 DB 層は `better-sqlite3`（`src/db/database.ts`）= **完全同期 API**（`db.prepare().get()/.all()/.run()`）で全 repo が書かれている。ドラフトの `ToolContext.deps.repo`、`LlmClientPool::user_client` の `self.repo.user_gemini_config(uid).await`、C-6 の recall/`record_tool_outcome`、C-4 のツール群 — **すべてが async 前提**で書かれているが、Rust の SQLite エコシステムでこれをどう満たすかが一言も無い。

**なぜ致命的か**:
- Rust には `better-sqlite3` 相当の「速い同期 SQLite」しかない（`rusqlite`）。これを tokio の async タスクから**直接呼ぶと、同期 I/O がワーカースレッドをブロックし**、`tokio::spawn` した Bot アクター群・Gemini ワーカーが芋づるで停止する。「アクター単位の自己復帰」どころか、DB が少し詰まると**ランタイム全体のスループットが崩れる**。制約3（現行より高速・堅牢）と正面衝突する。
- 選択肢は3つあり、**どれもドラフトの前提を書き換える**:
  1. `sqlx`（async・コンパイル時検証）→ ただし SQLite バックエンドは内部で同期呼び出しを `spawn_blocking` に載せているだけで、真の並列書き込みは SQLite 側の単一ライタ制約で不可能。
  2. `rusqlite` + `tokio::task::spawn_blocking` を全 DB 呼び出しでラップ → ドラフトの全 async シグネチャに `spawn_blocking` 境界と専用スレッドプール設計が必要。
  3. 専用 DB アクター（1スレッドが接続を占有し `mpsc` で受ける）。

**どう直すべきか**: C-0 に「**DB アクセス方針**」セクションを新設し、(a) `rusqlite` を専用 blocking スレッドプール（`spawn_blocking` 上限を tokio の `max_blocking_threads` で明示制御）に載せるか、(b) 書き込み専用の単一 DB アクター + 読み取り用 WAL 並列接続プール、のどちらかを**型で**確定させること。SQLite の単一ライタ制約（WAL でも書き込みは直列）を「制約3の高速化」の期待値に織り込み、`SQLITE_BUSY` を `DbError::Busy`（リトライ可能）として thiserror 列挙に入れる。これが無いと C-3〜C-6 の async シグネチャ全体が絵に描いた餅。

---

## 2. `Ok(None)` による「正常な縮退」表現が制約1の握り潰しの抜け道になっている（確信度: 高）

**どこが問題か**: C-6 で「エンジン無効/timeout は `Ok(None)` で縮退」「IPC プロトコル異常は `Err`」と分けているが、この境界は**呼び出し側が縮退したことを型で強制的に知る手段がない**。`Ok(None)` は「該当recallが無かった（正常な空）」と「エンジンが落ちていて取得を諦めた（縮退）」を**同じ値に潰している**。これはまさに制約1が禁じる「抽象化して握り潰す」構造そのもの — anyhow を使っていないだけで、意味論的には同じ情報損失が起きている。

**なぜ問題か**: 「LLM は縮退中だが応答は返った」ことをメトリクス/監視が区別できず、常時稼働の観測性（制約2の実効性）が損なわれる。Redis 縮退（C-2 の `memory_claim` フォールバック）でも同じ構造 — `ClaimResult::First` に memory 縮退が混ざると「Redis 死亡中でも First として二重応答リスクを負っている」ことが型に出ない。

**どう直すべきか**: 縮退は `Option` に潰さず**専用型**で表現する。`enum RecallResult { Hit(Vec<...>), Empty, Degraded { reason: DegradeReason } }` のように「空」と「縮退による空」を別バリアントに。Redis も `ClaimResult::First { degraded: bool }` かバリアント分割。「縮退したか」が呼び出し側の match を必ず通り、メトリクス記録が漏れない構造にする。

---

## 3. `tokio::spawn` によるツール panic 隔離は `catch_unwind` の問題を解決していない（確信度: 高）

**どこが問題か**: C-4 の `dispatch` は「`catch_unwind` は非 `UnwindSafe` で扱いづらいためタスク境界（`tokio::spawn`）で隔離」としているが、**`tokio::spawn` されたタスク内の panic も、そのタスクが `UnwindSafe` でないクロージャを掴んでいれば同じ問題を持つ**わけではない — が、より深刻な点として **panic した際にそのツールが握っていたロック（`Mutex`）が毒され（poisoned）、共有state が壊れる**ことに無防備。`ctx.clone_shallow()` で `Arc<ToolDeps>` を共有している以上、あるツールの panic 中に保持していた `Arc<Mutex<...>>` は `PoisonError` を残し、**次以降の全ツールの `lock()` が失敗**する。panic を隔離したつもりが、共有依存経由でクロステナントに障害が伝播する。

さらに `panic = "unwind"`（C-2）と併せても、**panic unwind 中に別の drop で二重 panic すればプロセスが abort する** — supervisor 隔離が効かないケースが残る。

**どう直すべきか**: (a) 共有可変 state は `Mutex` ではなく `parking_lot::Mutex`（poison しない）か、そもそも actor/`mpsc` に寄せて共有可変を排除する。(b) ツールが panic した事実を `ToolStatus::Failure` に畳むだけでなく、**「panic した tool を一定回数でサーキットブレーク（無効化）」**するポリシーを入れる（毎ターン同じ panic を繰り返して隔離コストを払い続ける退行を防ぐ）。(c) `tokio::spawn` per ツール呼び出しは高頻度パスでの spawn コスト・キャンセル安全性の懸念があるので、`FuturesUnordered` + 手動 `AssertUnwindSafe` catch の比較検討を明記。

---

## 4. `#[non_exhaustive] を付けない」方針が cargo-deny/clippy 強制と矛盾しうる（確信度: 中）

**どこが問題か**: C-8 で「内部クレートなので `#[non_exhaustive]` を付けず、バリアント追加時に全 match が壊れて漏れを検出」とある。方針自体は妥当だが、これは**エラー型を跨いで `#[from]` で自動変換している箇所（`Transport(#[from] reqwest::Error)` 等）と衝突する**。`#[from]` があると上位エラーへ透過的に吸い上げられ、「新バリアントが追加されても中間層が `?` で素通しして、末端の match だけが壊れる」— つまり**壊れてほしい中間層が壊れない**。網羅性強制が `#[from]` チェーンの本数だけ骨抜きになる。

**どう直すべきか**: エラー変換は `#[from]` の乱用を避け、**層の境界で明示的に `.map_err()` で意味変換**する（Gemini の transport エラーを「このドメインではどう扱うか」を各層が明示）。少なくとも `#[from]` を使う箇所は「意味を変えず素通ししてよい」とレビューで確認したものに限定するルールを C-8 に加える。

---

## 5. 型生成（ts-rs）が現行の「フェイルクローズ機密 strip」を再現できていない（確信度: 高）

**どこが問題か**: 制約5と C-9 の核心。現行 `apiViews.ts` は **zod の `.parse()` を通すことで「スキーマに列挙した列以外は strip」= allowlist フェイルクローズ**を実現している（機密列 `*_encrypted`/`*_iv`/`*_tag`/`password_hash` が DB レコードから紛れ込んでも出力されない）。しかも `has_gemini_key`/`has_token` は**生列ではなく計算済み boolean**（暗号文の存在を bool に潰したもの）。

ところが ts-rs は **Rust 構造体 → TS 型定義の生成**であって、**実行時のシリアライズ時 strip を一切保証しない**。「Rust を単一の真実源に」しても、DTO 構造体に機密フィールドを1つ足せば ts-rs は黙って型を生成し、`serde` はそれをそのまま JSON に出す。**フェイルクローズ（列挙漏れ=出さない）が、Rust では フェイルオープン（構造体に足す=出る）に反転する**。これは制約5が名指しで解消を求めた「機密漏洩リスク」を**むしろ悪化**させる。

**どう直すべきか**:
- ドメイン型（DB行）と DTO 型を**別クレート・別モジュールで物理分離**し、DTO には機密フィールドを**そもそも定義できない**構造にする（C-9 は「TS を付けない」しか言っておらず、機密が DTO に混ざらない保証が無い）。
- DTO への変換を「ドメイン型から必要フィールドを**明示的にコピーする `From` 実装のみ**」に限定し、`#[serde(flatten)]` や生レコードの派生を禁止（clippy/lint で機械強制）。
- `has_gemini_key` のような計算 bool は「暗号文フィールドを DTO が保持しない」ことをコンパイル時に保証（暗号文は `SecretString`、DTO は `bool` のみ）。
- テストで「全 DTO の serde 出力に機密パターン（`_encrypted`/`_iv`/`_tag`/`token`/`hash`）が含まれない」ことをスナップショット/property test で検証。**フェイルクローズを型でなくテストで補う**必要がある点を C-9 に明記すべき。

---

## 6. `panic = "unwind"` を「このクレートで強制」できると誤解している（確信度: 高）

**どこが問題か**: C-2 は「`[profile.release] panic = "unwind"` をこのクレートで強制」と書くが、**`panic` プロファイル設定はワークスペース/最終バイナリ（トップレベル `Cargo.toml`）でしか効かない**。ライブラリクレート側の `[profile]` は Cargo に無視される（`profiles for the non root package will be ignored` 警告が出る）。「各クレートで機械強制」という C-8 の思想と組み合わせると、**設定した気になって実際は default（unwind）に依存しているだけ**という穴になる。誰かが最終バイナリで `panic = "abort"`（バイナリサイズ削減でよくやる）にした瞬間、supervisor 隔離が全滅する。

**どう直すべきか**: `panic = "unwind"` は**ワークスペースルートの `[profile.release]`/`[profile.dev]` に置く**と明記。さらに `panic=abort` を CI で検出する（`cargo metadata` か build script でプロファイルを検査して fail させる）ガードを C-8 に追加。supervisor の隔離が panic 戦略に**暗黙依存**していることを設計上の前提条件として明示する。

---

## 7. トークンローテーションの `ArcSwap` 差し替えに競合窓が残る（確信度: 中）

**どこが問題か**: C-2 は「旧 Shard へ `CancellationToken` → 新トークンで再 spawn → `ArcSwap::store`」で Node の欠陥が「消える」と主張するが、**新旧 Shard が同一 bot_user_id で同時に gateway 接続している瞬間**が構造的に残る（旧のキャンセル完了前に新を spawn すると、Discord 側が両接続にイベントを配る）。これは `claimMessageOnce` を Redis SETNX に寄せた理由そのもの（ドラフトも「同一 identity が共有されうる」と認めている）だが、**ローテーション中の二重接続を「消えた」と書くのは過大**。`ArcSwap::store` は notifier 側の読み取りを最新化するだけで、gateway 側の二重受信は防がない。

**どう直すべきか**: ローテーションを「**旧 Shard の close 完了を待ってから新 spawn**」の直列プロトコル（`oneshot` で close 確認 → spawn）として型で表現し、`RotateToken` コマンド処理を「cancel → await joined → spawn → store」の順序に固定。二重接続窓をゼロにできない場合（Discord の resume 猶予等）は `claimMessageOnce` が**唯一の防波堤**であることを明記し、「ローテーション中は Redis 必須（memory 縮退では二重応答を許容する）」とデグレードのトレードオフを正直に書く。

---

## 8. Bot ごとの `RwLock`/`DashMap` 前提でのメンバーキャッシュ不完全性が UI 契約と切れている（確信度: 中）

**どこが問題か**: C-1 の `GuildMemberList::Partial { cached }` は型として綺麗だが、現行 `getGuildOptionsForBot` が返す `members_complete:false` は**フロント（Svelte）が「ID 手入力で補う」UI 分岐に使っている**契約値。C-9 は「Bot ステータス等フロントに出る型は ts-rs」と言うが、この `Partial`/`Complete` enum を TS に落とすと **Rust 側で `Complete` バリアントが構築不能（型で封じた）なのに、TS 側には両バリアントが生成され**、フロントが到達不能な `Complete` 分岐を書けてしまう。単一真実源が「到達不能状態を真実として輸出する」ねじれ。

**どう直すべきか**: フロント契約用 DTO は enum ではなく `{ members: [...], complete: false }` のような**フラットな bool フィールド**にし（現行 `members_complete` と 1:1）、内部ドメインの `GuildMemberList` enum とは C-5 の DTO 分離ルールで切り離す。「型で封じた不変条件」を輸出型に混ぜない。

---

## 9. governor / cargo-deny / twilight 0.16 のバージョン整合（確信度: 中）

- **twilight 0.16 は実在**するが、`twilight_gateway::Error` の型名・`Shard::next_event()` のシグネチャは 0.15→0.16 で変更されている。ドラフトの `Gateway { source: twilight_gateway::Error }` は**実際の 0.16 では `ReceiveMessageError` 等に分岐**しており、`twilight_gateway::Error` という単一型で受けられない可能性が高い。→ 実装前に 0.16 の実際のエラー型を確認し thiserror 列挙をそれに合わせる（確信度: 中、要一次確認）。
- **`backoff` クレート排除の理由づけが不正確**: 「内部で `Box<dyn Error>` を返す版がある」は誤り。`backoff` はジェネリックエラーで、握り潰しはしない。排除自体は「自前実装で型を握る」方針として妥当だが、**理由が事実誤認**なので設計の説得力を損なう。理由は「リトライ分類を自前 `is_retryable()` に一元化したい」に差し替えるべき（確信度: 中）。
- **`cargo-deny` の `[bans]` は依存グラフに間接的に入った anyhow を検出できるが、`thiserror` が内部で `anyhow` を使っていないかは別**。禁止は妥当だが「機械強制で完全」と過信しないこと（`deny` は名前一致のみ、re-export された握り潰しは防げない）。

---

## 10. 移行実現性: 「単一 tokio ランタイム全面書き換え」と段階移行の両立が未設計（確信度: 中）

**どこが問題か**: ドラフトは C-0 で「全面書き換え」を宣言するが、制約4（将来のカスタムモジュール）や制約5（型生成）は言及する一方、**Node → Rust の並行稼働期間の設計が皆無**。現行は better-sqlite3 の単一ファイル DB を Node が握っている。Rust プロセスが同じ SQLite ファイルを同時に開くと、**WAL モードでもプロセス跨ぎの書き込み競合・busy が頻発**する（`better-sqlite3` と `rusqlite` が別プロセスで同一 DB を叩く構成）。Discord Bot は「同一トークンで2プロセスが gateway 接続」すると Discord 側が片方を切る。つまり**機能単位の段階カットオーバーが SQLite 共有と Bot トークン占有の両方で物理的に困難**。

**どう直すべきか**: 移行戦略セクションを新設し、(a) DB は「Rust が唯一の writer、Node は読み取りのみ or 完全停止」のカットオーバー順序を決める、(b) Bot gateway は「トークンごとに Node か Rust のどちらか一方だけが接続」を保証するルーティング（機能フラグ）を設計、(c) HTTP API 層とツール実行層を先に Rust 化して Node から IPC で呼ぶ「ストラングラー」経路（既存の改行区切り JSON デーモンパターンを流用）を検討。**「全面書き換え」を一括投入にすると SQLite と Bot トークンで詰む**ことを明記。

---

## 妥当と認める点

- **twilight 採用の判断**（マルチテナント Bot = Shard 軽量プリミティブ、エラーが `Err` で見える）は制約1/3と整合し妥当。
- **`FunctionModule` → `trait Tool` + `ToolOutcome` 写像**、副作用（embeds/files）を `&mut ctx` でなく戻り値 `attachments` へ寄せる借用設計は妥当かつ Rust 的に正しい。
- **`user_id` を `Option` にしない型強制**（現行コメント「全リポジトリ呼び出しに必須」の型化）は、クロステナント事故防止として正しい方向。ただし §5 で指摘した通り DTO 分離と組で完成させること。
- **完了ハルシネーション是正**（`ANY`+`allowedFunctionNames`、最大2回）の忠実移植方針は妥当（本コードベースの中核安全弁を正しく最重要視している）。
- **MCP 動的ツール = `Arc<dyn Tool>` プラグイン境界**（静的/動的を同一 HashMap にマージ、命名衝突の SHA1 一意化移植）は制約4の拡張性要件に対して構造的に正しい。
- **`RegistryError::DuplicateName` の起動時 fail-fast**（Node の throw と同義）は妥当。

---

## 優先度サマリ

| # | 論点 | 確信度 | 計画への影響 |
|---|---|---|---|
| 1 | SQLite 同期ドライバをasync前提設計が無視 | 高 | **致命的**（全 async シグネチャが崩れる） |
| 5 | ts-rs がフェイルクローズを再現できない（機密がフェイルオープンに反転） | 高 | **致命的**（制約5の主目的を悪化） |
| 2 | `Ok(None)` 縮退が握り潰しの抜け道 | 高 | 大（制約1/観測性） |
| 3 | ツール panic 隔離が poison/二重panicに無防備 | 高 | 大（制約2の実効性） |
| 6 | `panic=unwind` をライブラリクレートで強制不能 | 高 | 大（supervisor 隔離の前提崩壊） |
| 10 | 段階移行が SQLite共有/Botトークン占有で詰む | 中 | 大（移行実現性） |
| 7 | ローテーション二重接続窓は「消えない」 | 中 | 中 |
| 4 | `#[from]` チェーンが網羅性強制を骨抜き | 中 | 中 |
| 8 | member 不完全性 enum が UI 契約とねじれ | 中 | 中 |
| 9 | twilight 0.16 エラー型・backoff 排除理由の事実誤認 | 中 | 小〜中（要一次確認） |

最優先で塞ぐべきは **#1（SQLite async 戦略）と #5（ts-rs のフェイルクローズ喪失）**。この2つは「設計の細部」ではなく、ドラフトが依拠する2大前提（async アクター群 / 単一真実源での機密防御）を根本から崩す。参照した実ファイル: `/home/suki/web/kawaii-music.moe/apps/yuuka/src/db/database.ts`、`/home/suki/web/kawaii-music.moe/apps/yuuka/src/types/apiViews.ts`、`/home/suki/web/kawaii-music.moe/apps/yuuka/src/functions/registry.ts`、`/home/suki/web/kawaii-music.moe/apps/yuuka/src/bot.ts`。

---

## [21] I have the two verified files. The draft's claims about the current `ToolContext
<sub>agentId: ace65fb5415c4f0cc</sub>

I have the two verified files. The draft's claims about the current `ToolContext` and `FunctionModule` shapes are accurate. I have enough to write a sharp critique. Note the draft references "棚卸し#1〜#5" documents I cannot see, so I'll flag where its claims depend on unverifiable assertions but focus on architecturally load-bearing defects.

---

# 辛口レビュー: ユーザー拡張モジュール・アーキテクチャ(将来)

総評: 骨格(3層収束 + `Arc<dyn Tool>` 単一境界)は妥当で、libloading却下の論拠も正しい。ただし**絶対制約1(厳格エラー)と制約2(自己復帰)には、設計自身が作り込んでいる握り潰しの穴と、この領域固有の落ち方の見落とし**がある。制約5(型生成)にも実現不能な部分がある。以下、成否に関わる順。

---

## A. 絶対制約1(厳格エラー)の破れ

### A-1. `async_trait` + `#![deny(clippy::unwrap_used)]` は WASM/サブプロセス層で機能しない — 握り潰しの主経路が無防備 (確信度: 高)

**どこ:** §1 の `#[async_trait::async_trait] trait Tool`、§8 の「`#![deny(clippy::unwrap_used, ...)]` を tool-core クレートに付与」。

**なぜ問題か:** 制約1の機械強制を `clippy::unwrap_used` の deny に賭けているが、これはクレート単位のリントであって**依存クレート内・生成コード・別クレート(T2/T3実装)には効かない**。T2の `SubprocessTool` 実装、T3の `WasmTool` 実装、既存 crawler/synapse アダプタは当然別モジュール/別クレートに置かれる。そこに `deny` を付け忘れれば無防備で、しかも「tool-core クレートに付与」としか書いていない。握り潰しが最も起きやすいのは**IPCのデコード・WASMの戻り値パース(§4の `result<string,string>` の string→Value)** であり、まさに tool-core の外側。加えて `.unwrap()` を機械排除しても `arr[i]` のインデックスパニック・整数除算・`Duration::from_...` のオーバーフロー・`slice::split_at` 等はリント対象外で残る。

さらに深刻なのは、**dispatch経路そのものが握り潰しを構造的に持つ**点。§7で「`ToolOutcome::Failed` を `{success:false}` JSON へ直列化」してLLMに返すとしているが、これは現行 `registry.ts:61-70` の `catch(err){ return {success:false, message: err.message} }` を**そのまま踏襲**している。つまり `ToolError::HostDep(DB断)` すら「LLMに返して会話継続」できてしまう設計で、制約1の「呼び出し側が判別・分岐できること」が dispatch境界で消える。型を厳格にしても、最終段で全部 stringに潰すなら意味がない。

**どう直すべきか:**
1. リントは `[workspace.lints]`(Cargo.toml のワークスペース継承)で**全メンバークレートに強制**し、「tool-coreに付与」ではなく `lints.workspace = true` を全 Cargo.toml に必須化(これ自体を CI で grep 検査)。
2. `unwrap_used` だけでなく `indexing_slicing`, `arithmetic_side_effects`, `clippy::panic_in_result_fn` を追加。
3. dispatch段で `ToolError` を**二分岐**させる: `ToolError::HostDep` / `SandboxTrap(Timeout)` のような**ホスト側障害はLLMに `{success:false}` で返さず**、上位(領域E)へ `Err` で伝播し、リトライ or 縮退判断を型で分ける。「LLMに見せてよい失敗(`ToolOutcome::Failed`)」と「ホスト障害(`ToolError`)」の境界を、dispatchの戻り値型 `Result<ToolOutcome, ToolError>` のまま**上に出す**(現行の string 潰しを廃止する)こと。§7 は現状「両方 stringに畳む」と読め、ここが制約1最大の抜け道。

### A-2. `cargo-deny [bans]` は transitive依存の anyhow を止められない (確信度: 中)

**どこ:** §8「`anyhow`/`eyre` はワークスペースから `cargo-deny` の `[bans]` で禁止」。

**なぜ問題か:** `wasmtime` は内部で `anyhow` を**公開API型として**使う(`wasmtime::Error` は歴史的に `anyhow::Error` エイリアス/相互変換を持つ)。`Linker::instantiate` 等が `anyhow::Result` を返す。`[bans].deny` で `anyhow` を全面禁止すると**wasmtime自体がビルド不能**になる。`skip`/`skip-tree` で例外にすると、今度は「wasmtimeを触るホストコードに `anyhow::Error` が漏れてくる」ので、制約1の「握り潰し型の全面禁止」がT3境界で破れる。

**どう直すべきか:** `[bans]` は「直接依存として anyhow を宣言することの禁止」に留め、wasmtime境界には**`anyhow::Error` → `WasmTrap`/`WasmSetupError` への明示変換層**を1関数に閉じ込める(`fn classify_wasmtime_err(e: anyhow::Error) -> WasmTrap`)。この変換関数だけが anyhow に触れてよい隔離ポイントとして設計に明記する。「wasmtimeが anyhow に依存する」事実を設計が認識していないのは穴。

---

## B. 絶対制約2(自己復帰・常時稼働)の破れ

### B-1. 「毎ターン再構築」レジストリと「常駐supervisor」の間に、この領域固有のデッドロック/枯渇経路がある (確信度: 高)

**どこ:** §2「レジストリ登録だけが毎ターン、実体は常駐」、§6 supervisor。

**なぜ問題か:** 常駐プロセス(T2)が指数バックオフ再起動中(例: crawlerのChrome連続クラッシュで backoff 30s上限に張り付き)のとき、`build_for_turn` は毎ターンこのツールをどう扱うのか設計が空白。選択肢は (a)登録する→呼ばれるとIPCタイムアウトまでブロック、(b)登録しない→ツールがLLMから消える。draftは§6で「バイナリ不在は縮退(登録しない)」とだけ書くが、**「起動試行中/バックオフ中/half-open」という中間状態**の扱いがない。毎ターン同期的に「今 healthyか?」を supervisor に問い合わせる設計だと、その問い合わせ自体が**ホットパス(gemini応答ループ)をブロック**し得る。制約3(高速)とも衝突する。

さらに、T3で「`Store` はコール毎に新規」+ 常駐 `Engine` は正しいが、**wasmtime `Component`のコンパイルは重い**。「アップロード時に事前コンパイル」と書くが、プロセス再起動後の初回ロード(`Component::deserialize`)コストと、**複数ボット(マルチテナント)で同一ext_idを共有するか否か**が未設計。ボット毎に `Engine`/`Component`キャッシュを持つとメモリ爆発、共有すると `StoreLimits` のメモリ会計がテナント跨ぎになる。

**どう直すべきか:**
1. supervisorに明示的な**サーキットブレーカ状態(Closed/Open/HalfOpen)**を持たせ、`build_for_turn` は**ノンブロッキングに現在状態のスナップショット**(Atomicなヘルスフラグやwatchチャネルの`borrow()`)だけ読む。Open中のツールは「登録するが declaration に "現在利用不可" を付けない=そもそも登録しない」を選び、**IPC発行を待たせない**。
2. `Component` は**プロセス全体で1つの `Engine` + `ext_id→Component` キャッシュ**、`Store` にのみテナント(`user_id`)スコープと `StoreLimits` を持たせる。メモリ会計は Store 単位=呼び出し単位で正しくテナント分離。これを§4/§6に明記。

### B-2. supervisorの再起動が「毒メッセージ(poison pill)」で無限バックオフループになる経路が塞がれていない (確信度: 中)

**どこ:** §6「exit/EPIPEで pending を全 reject → 次リクエストで再起動」「指数バックオフで再spawn」。

**なぜ問題か:** ユーザー拡張(T2自前デーモン)が**特定の入力で必ずクラッシュ**する場合、そのツールが呼ばれる→クラッシュ→再起動→また呼ばれる→…。バックオフは「再起動間隔」を延ばすが、**クラッシュを誘発するツールコール自体は毎ターン再投入され得る**(LLMが同じ関数呼び出しを繰り返す)。draftには「連続失敗N回でツールを一定時間隔離(quarantine)」する上限がない。制約2の「些細な障害では落ちない」は守れても、**特定拡張が実質DoSになる**。ユーザー拡張(=信頼できない実装)を許す以上、これは必須。

**どう直すべきか:** supervisorに「連続失敗回数の上限 → 到達で `enabled=false` 相当の自動隔離 + 監査ログ + ユーザー通知」を入れる。バックオフ(間隔)と隔離(打ち切り)は別ポリシーとして型で分離する。

### B-3. 「fail-fastは起動時シークレットのみ」だが、複数ボットのDiscord接続はこの原則と両立しない (確信度: 中)

**どこ:** §6「fail-fast は起動時シークレット/設定のみ」。

**なぜ問題か:** yuukaは複数ボット(`botId`)を扱う。あるボットのDiscordトークンが無効/失効していた場合、これは「起動時シークレット不備」に見えるが、**他ボットは正常に動かせる**。ここで fail-fast(プロセス停止)すると1ボットの失効が全ボットを巻き添えにする=制約2違反。逆に全部縮退にすると「本当にシークレット総崩れ」を検知できない。draftの二分法(起動時シークレット=fail-fast / それ以外=縮退)は**ボット粒度を欠いており粗すぎる**。

**どう直すべきか:** fail-fastの粒度を「プロセス起動に必須の共有シークレット(DB URL, マスター鍵)」に限定し、**個別ボットのトークン/外部連携は起動時でも縮退(そのボットのみ無効化して起動継続)**にする。「致命的」の定義を "プロセス全体が無意味になるか" で線引きし直す。

---

## C. 技術的正当性の罠

### C-1. SQLite + マルチスレッド + `spawn_blocking` は、この設計だと書き込みで詰まる (確信度: 高)

**どこ:** §8 制約3「同期SQLite越境は `spawn_blocking`」、§5 `ext_kv` / `user_extensions` テーブル。

**なぜ問題か:** SQLiteは**単一ライタ**。`spawn_blocking` はスレッドプールに逃がすだけで並行書き込み競合(`SQLITE_BUSY`)は解決しない。T3のWASMが `kv-put` を高頻度で呼ぶ設計(§4)で、複数WASM/複数ボットが並行に `ext_kv` へ書くと `SQLITE_BUSY`/`database is locked` が多発する。draftはWALモード・単一ライタコネクション・busy_timeoutに一切触れていない。制約3(現行より堅牢)を主張しながら、DB層の並行性モデルが空白。

**どう直すべきか:** (1) WALモード必須明記、(2)**書き込みは単一の専用コネクション(actor/mpscチャネル経由でシリアライズ)**、読み取りはコネクションプール、(3) `busy_timeout` + `SQLITE_BUSY` を `HostError` の一種として型付け縮退。これを§5に設計として書く。「`spawn_blocking` すれば良い」は誤り。

### C-2. WASI p2 + wit-bindgen + wasmtime component の「アップロード時コンパイル→保存」は、wasmtimeバージョン固定の呪いを負う (確信度: 中)

**どこ:** §4「事前コンパイル済(アップロード時に検証)」、§5 `wasm_blob`(BLOB, sha256検証)。

**なぜ問題か:** `Component::serialize`/`deserialize`(cranelift事前コンパイル成果物)は**wasmtimeのバージョン・ターゲット・`Config`に厳密に紐づく**。DBに事前コンパイル済バイナリを保存すると、wasmtimeをアップグレードした瞬間に全 `deserialize` が失敗する。draftの `wasm_blob` が「元の .wasm」なのか「serialize済ネイティブ」なのか曖昧。後者なら運用で確実に事故る。

**どう直すべきか:** DBには**元の `.wasm`(と sha256)のみ保存**し、事前コンパイル成果物は**起動時に再生成する揮発キャッシュ(ディスク/メモリ)**として扱う。`Engine` の `Config` ハッシュをキャッシュキーに含める。§4/§5を「保存=ソース .wasm、コンパイル=起動時/初回ロード時」に修正。

### C-3. `ts-rs`/`typeshare` で `serde_json::Value` / JSON-Schema動的型は生成できない — 制約5の核が生成不能 (確信度: 高)

**どこ:** §1 `ToolOutcome::Ok { payload: Value }`、§8 制約5「`ts-rs`/`typeshare` で TS 生成」「拡張の I/O スキーマも同経路」。

**なぜ問題か:** 制約5は「単一真実源(Rust)から自動生成し、`types.ts` の手書き重複と機密漏洩を構造的に解消」。ところが `ts-rs`/`typeshare` は**静的な `#[derive]` 型しかTSに落とせない**。ツールの入出力ペイロードは `serde_json::Value`(動的)であり、これは TS では `any`/`unknown` にしかならない。つまり**拡張ツールのI/O型は自動生成の恩恵をゼロ**しか受けず、制約5が最も効いてほしい「拡張のI/Oスキーマ」でこそ型安全が消える。さらに機密フェイルクローズ(現行 `apiViews.ts` の zod allowlist で機密列strip)は、`ts-rs`が**型の形しか生成せず「どの列を出してよいか」のランタイム強制を生成しない**。型生成とフェイルクローズは別問題なのに、draftは制約5でこれを一緒くたにして「解決」と主張している。

**どう直すべきか:**
1. DTOは `Value` ではなく**具体的な `#[derive(TS, Serialize)]` 型**を定義し、`ToolOutcome::Ok` を `payload: Value` から**ジェネリック or 型付きバリアント**へ。動的が不可避な拡張I/Oは「TS側 `unknown` + ランタイムzod検証」と割り切り、制約5の適用範囲を正直に線引きする。
2. 機密strip(フェイルクローズ)は型生成では代替不能。**API境界DTOを「機密列を型として持たない別struct(view struct)」にし、DBエンティティ→viewの変換を必須化**(コンパイル時に機密列がviewに存在しない=構造的strip)。zod allowlistのRust版は「viewに機密フィールドを置かない」型設計で達成する。これを制約5の本体に据える。draftはこの区別を欠く。

### C-4. `disambiguateFunctionName`(SHA1)を維持しつつ `ToolDeclaration.name` を「レジストリ一意キー」にすると衝突検出とGemini名前制約が二重管理になる (確信度: 中)

**どこ:** §1「name: レジストリ一意キー」、§3「`disambiguateFunctionName`(SHA1)」、§2 `SanitizedCollision`。

**なぜ問題か:** Geminiのfunction名は文字種・長さ制約がある。MCP由来ツール名をサニタイズ→SHA1付与して衝突回避する現行動作を維持するなら、「レジストリの一意キー」と「Geminiに渡すサニタイズ後の名前」は**別物**になる。draftは `ToolDeclaration.name` 一本に混ぜており、`build_for_turn` で「サニタイズ後に再衝突(`SanitizedCollision`)」を検出する時点で、**内部キーと外部名の対応表**が必要。設計にそのマップ(内部id ⇄ Gemini提示名)が無い。LLMは提示名で呼び返してくるので、dispatchは提示名→内部Toolの逆引きが必須。

**どう直すべきか:** `ToolDeclaration` に `id`(内部安定キー)と `gemini_name`(サニタイズ後・毎ターン生成可)を分離し、レジストリは両方でindexする。dispatchは `gemini_name→Arc<dyn Tool>`。§1/§2を2キー設計に。

---

## D. 既存不変条件の取りこぼし

### D-1. userIdデータ分離は「型で機械注入」と言うが、その強制機構が空欄 (確信度: 高)

**どこ:** §1「全DB呼び出しへ機械注入」、§5 `ext_kv(user_id,...)` PK複合。

**なぜ問題か:** 現行 `ToolContext.userId` は「全リポジトリ呼び出しに必須」(contracts.ts:20-21)だが、これはコメント上の規約で**コンパイラは強制していない**。draftは「型で機械注入」と繰り返すが、**どうやって**の設計が無い。`ToolContext` に `user_id` を持たせるだけでは、リポジトリ関数が引数で受け取り忘れれば分離は破れる(現行と同じ穴が残る)。T3のWASMは `scope.user-id` をホスト注入で守れる(これは良い)が、**T1/T2のホスト側リポジトリ層**の強制が未設計。制約(現行不変条件)の中核を「型で」と言い切っているのに機構が無いのは危険。

**どう直すべきか:** リポジトリ層を「`user_id` を握った `ScopedRepo<'_>` を `ToolContext` からしか生成できない」型(生成関数を `ToolContext::scoped_repo()` のみに)にし、**素の `user_id: &str` を取るクエリ関数をprivate化**。呼び出し側は `ScopedRepo` 経由でしかDBに触れない=注入忘れが型エラー。これを§1に明記。

### D-2. CSP/CSRF/機密ヘッダは領域外扱いされているが、拡張のhttp-fetch/自前デーモン登録UIで再登場する (確信度: 中)

**どこ:** draft全体でCSRF/CSPへの言及なし。§5「ユーザー自前デーモン(T2)登録UI」、§4 `http-fetch`。

**なぜ問題か:** 任務はCSRF/CSPを不変条件に挙げているが、draftはこの領域を「拡張実行」に閉じて完全に無視している。しかし**拡張登録(WASMアップロード/デーモンURL登録)は状態変更エンドポイント**でありCSRF保護対象。かつ `sendJson`(contracts.ts:99)は `X-Content-Type-Options`/`X-Frame-Options` を付けるが**CSPは無い**。ユーザー生成の拡張メタ(name/description)が管理UIに表示されるなら**格納型XSS**面が増える。draftはSSRF(assertSafeOutboundUrl再適用)には触れたが、拡張導入で増える**登録側の攻撃面**を落としている。

**どう直すべきか:** 拡張登録APIをCSRFトークン必須ルートとして明記、拡張メタ表示はサニタイズ/CSP、`http-fetch` allowlistは§4通りで良いがユーザー拡張の name/description の長さ・文字種バリデーションを `ExtManifest` デコード時に型で強制。

### D-3. 監査ログ(`addAuditLog("mcp.call")`)をreverifyクロージャに移設、で監査の網羅性が落ちる (確信度: 中)

**どこ:** §3「`addAuditLog("mcp.call")` を `reverify` クロージャに移設」。

**なぜ問題か:** 監査をT2の `reverify` クロージャに寄せると、**T3(WASM)とT1(内蔵)の呼び出しが監査ログに乗らない**。しかもクロージャ実装者が呼び忘れれば監査欠落。監査は「全拡張呼び出しで必ず」が不変条件のはずで、per-層のクロージャ任せは網羅性を壊す。

**どう直すべきか:** 監査は個別Tool実装ではなく **dispatch境界(レジストリの `dispatch`)で全 kind 一律に記録**する。`tool_name`/`user_id`/`outcome種別`/所要時間を dispatch が構造化ログ+監査テーブルへ。§7の `recordToolOutcome` と同じ dispatch フックに監査も同居させる。

---

## E. 移行実現性

### E-1. 「P1で crawler/synapse/MCPをRustのSubprocessToolに吸収」だが、現行はNode本体。並行期にIPCが二重化する (確信度: 中)

**どこ:** §3/§9「P1でRust `SubprocessTool` に吸収」「Nodeからは子プロセス」。

**なぜ問題か:** 全面書き換え計画の途中では、**geminiループがまだNode**の期間がある。その間、Rustの `SubprocessTool` を作っても呼び出し元(gemini)がNodeなら、Node→(新Rust dispatch層)→子プロセス、という**二段IPC**か、あるいはNodeが直接子プロセスを叩く現行維持のどちらか。draftは「P1でツールコア + 吸収」を一気にやる前提だが、**geminiループ(領域E)のRust化とツールコアのRust化の順序依存**が明示されていない。ツールコアだけ先にRust化しても、Nodeのgeminiから `Arc<dyn Tool>` は呼べない。

**どう直すべきか:** 移行順を「領域E(geminiループ)Rust化 → 同時にツールコア」に束ねるか、または過渡期は**Rustツールコアを1本のNDJSONデーモンとしてNodeに露出**(既存パターンに自ら乗る)し、Node gemini→Rustツールコアデーモン→各拡張、の一段で繋ぐ。P1のスコープを「E とセット」か「デーモン露出」かで明記しないと、P1が宙に浮く。

### E-2. 「毎ターン再構築」を現行踏襲と言うが、現行の重複throwは起動時ではなくターン内 — Rustで `build_for_turn` が `Err` を返したときの継続動作が未定義 (確信度: 中)

**どこ:** §2 `build_for_turn -> Result<_, RegistryError>`、「毎ターン再構築」。

**なぜ問題か:** 現行 `buildFunctionRegistry` の重複throw(registry.ts:32,38)は**起動時に一度**呼ばれる想定の構造。draftはこれを**毎ターン**呼ぶ設計に変え、かつ `Duplicate`/`SanitizedCollision` を `Err` で返す。すると「ユーザーが2つの拡張で同名ツールを登録した」だけで、`build_for_turn` が毎ターン `Err`→そのターンの応答が丸ごと失敗、という**縮退できない致命化**が起きる。制約2(些細な障害で落ちない)に反する。ユーザー拡張は衝突を必ず起こす(信頼できない入力)。

**どう直すべきか:** `build_for_turn` は衝突ツールを**スキップ+警告して継続**(そのターンを殺さない)し、`Err` を返すのは「T1内蔵同士の衝突=開発バグ」だけに限定。ユーザー拡張(T2/T3)由来の衝突は縮退(片方無効化+通知)。「起動時致命 vs ターン内縮退」の線引きをレジストリ構築にも適用する。

---

## F. 妥当と認める点

- **libloading/.so全面却下**(§0-b): 論拠(unsafe get、ABI不安定、同一アドレス空間でのpanic/segfault隔離不能)は正確。制約2との非互換の指摘は妥当。
- **3層が `Arc<dyn Tool>` に収束させる**基本方針(§0,§1): 「後付けにしない=収束点を先に作る」は正しい設計判断。
- **副作用を戻り値`attachments`へ寄せ、`ToolContext`を非可変化**(§1): 現行の可変 `embeds/files` push(contracts.ts:28-30)を非信頼WASMに渡せない問題の解決として正しい。
- **T2優先/WASMを限定用途に後回し**(§3,§9): OSプロセス分離という枯れた隔離を先に使い、WASMを最後に回す判断はリスク管理として妥当。制約4がP1で実質達成という論理も正しい(MCP URL追加=既存のユーザー拡張機構)。
- **fuel + epoch_deadline の二重計量**(§4): wasmtimeでCPUと壁時計を別々に殺す標準手法として正確。`extism`回避の理由も妥当。

---

## 優先度サマリ(成否に直結する順)

| # | 論点 | 確信度 | 制約 |
|---|---|---|---|
| A-1 | dispatch最終段のstring潰し=握り潰し継続 / lintがクレート外に効かない | 高 | 1 |
| C-1 | SQLite単一ライタ、`spawn_blocking`では並行書き込み解決せず | 高 | 3 |
| C-3 | `ts-rs`は`Value`/動的I/O・機密stripを生成できず、制約5の核が不成立 | 高 | 5 |
| D-1 | userId分離「型で機械注入」の強制機構が空欄 | 高 | 3(現行不変条件) |
| B-1 | 毎ターン再構築×常駐supervisorのブロッキング/中間状態未設計 | 高 | 2,3 |
| E-2 | `build_for_turn`が衝突でターン丸ごと致命化=縮退不能 | 中 | 2 |
| A-2 | wasmtimeがanyhow公開依存、`[bans]`全面禁止でビルド不能 | 中 | 1 |
| C-2 | serialize済WASMをDB保存=wasmtimeバージョン固定の事故 | 中 | 2 |
| B-2/B-3 | poison pill無限再起動 / fail-fastがボット粒度を欠く | 中 | 2 |
| D-2/D-3 | 拡張登録UIのCSRF/XSS・監査をクロージャ任せで網羅性喪失 | 中 | 3(現行不変条件) |
| C-4 | 内部キーとGemini提示名の二重管理・逆引きマップ欠落 | 中 | — |
| E-1 | 過渡期のNode↔Rust二段IPC・P1スコープの順序依存が宙に浮く | 中 | 4 |

**最も設計をやり直すべき点はA-1(厳格エラーの最終段潰し)とC-3(制約5が生成技術的に不成立)**。この2つは「制約を満たした」という主張自体が破れており、チェックリスト§8の自己申告(制約1○/制約5○)は不正確。

注記: 本レビューは提示2ファイル(`contracts.ts`/`registry.ts`)で検証可能な範囲を裏取りした。draftが依拠する「棚卸し#1〜#5」は当方で参照不能のため、それらに基づく主張(SSRFガードの現存、MCPクロステナント穴v7、TTL1hキャッシュ等)の事実性は未検証。これらが実在するかは別途確認を要する。

---

## [22] 前提は正確だ。1点だけドラフトの記述に不正確さがある: DROP 分岐（:880-894）は `currentVersion === "1"` **かつ** `
<sub>agentId: acede2d440c237bf1</sub>

前提は正確だ。1点だけドラフトの記述に不正確さがある: DROP 分岐（:880-894）は `currentVersion === "1"` **かつ** `hasAnyLegacyTable` の複合条件で、単に "1" を返すだけでは発火しない。ただしこれはドラフトの結論（Rust は DDL 不発行にすべき）を弱めるものではない。

これで十分な裏取りができた。反証レビューを執筆する。

---

# 辛口レビュー: 移行ロードマップ + サブエージェント並行実装の分解

前提4点（PRAGMA 2つのみ / SCHEMA_VERSION="17" と DROP 破壊経路 / workspace 不在 / nginx 空ディレクトリ）は実ファイルで確認、いずれも正確。ただしドラフトの「DROP 分岐（:880-894）へ落ちる」記述は不正確: 実際の発火条件は `currentVersion !== "17" && hasAnyLegacyTable && (currentVersion === "1" || hasLegacyUsers)` の**複合条件**であり、`getCurrentSchemaVersion()` が "1" を返すだけでは発火しない（`hasAnyLegacyTable` が必要）。この不正確さ自体は結論を弱めないが、「事実として明記する」と胸を張る文書が事実を1つ雑に書いているのは減点。以下、計画の成否に関わる論点を優先度順に。

---

## A. 絶対制約の充足 — 握り潰しの抜け道

### A-1. `serde_json::Value` が anyhow 禁止の「型付きエラー」原則を骨抜きにする 【確信度: 高】

**どこが/なぜ**: `Tool::call(...) -> Result<ToolOutcome, ToolError>` の `args: serde_json::Value` と `ToolOutcome.payload: serde_json::Value` は、絶対制約1「すべての失敗経路に名前と型を与え、呼び出し側が判別・分岐できること」の**最大の抜け穴**。anyhow を deny.toml で禁止しても、`Value` を各ツール内部で手動パースする瞬間に「文字列キーの取り違え・型不一致・欠落フィールド」が**型では検出されない動的エラー**に退化する。ToolError にいくら網羅的 enum を作っても、`args["user_id"].as_str().ok_or(...)` の連鎖が各ツールに散らばれば、それは anyhow を別名で再発明したのと同じ。Gemini function calling の性質上 `Value` 入口は避けられないが、ドラフトは「文字列規約を型へ昇格」と自称しつつ入口を `Value` のままにしている点で自己矛盾。

**どう直す**: Tool トレイトをジェネリック化し、`trait Tool { type Args: DeserializeOwned + JsonSchema; type Output: Serialize + TS; ... }` とする。`declaration()` は `schemars` で `Args` から FunctionDeclaration を**自動生成**（手書き宣言と実パースの乖離＝Gemini が送る形と受ける形のズレも同時に消える）。registry へは `erased-serde` 相当のアダプタ層（`Box<dyn ErasedTool>`）で `Value` ↔ 型付きの境界を**1箇所に集約**し、個別ツールは `Value` を一切見ない。これで「Value 手パースの散在」を構造的に禁止できる。ドラフトの現案のままだと制約1は入口で破れている。

### A-2. supervisor の catch_unwind 隔離は「毒された状態」を復旧しない 【確信度: 高】

**どこが/なぜ**: §9 の supervisor は `tokio::spawn(make())` の JoinHandle で panic を捕捉し再起動する。しかし panic した時点で**共有状態（`Arc<Mutex<T>>` の中身、writer actor の mpsc、Redis コネクションプール）が不整合のまま残る**。特に致命的なのが §2.2 の **writer actor**: 単一 tokio::task に mpsc で書き込みを直列化する設計だが、この writer タスク自体が panic したら、supervisor が再起動しても (a) mpsc の送信側は `SendError` で全 Repo 呼び出しが失敗し続ける（チャネルが閉じている）、(b) panic 途中の未コミットトランザクションが宙吊り。ドラフトは「アクター単位に狭める」と書くが、writer actor は**単一障害点**であり、ここが落ちると全書き込みドメインが同時に死ぬ。「アクター単位の隔離」というより「全書き込みの単一 SPOF」。

さらに `std::sync::Mutex` を tokio task 内で使い panic すると **poison** し、以降の `lock()` が全部 Err になる。ドラフトは Mutex poison / channel closed の復旧経路を型でもポリシーでも定義していない。

**どう直す**: (1) writer actor を supervisor 化するなら、再起動時に mpsc チャネルごと作り直し、送信側は `ArcSwap<Sender>` で新チャネルへ張り替える（旧 Sender を掴んでいる Repo は Recoverable エラーでリトライ）。(2) writer 内では `parking_lot::Mutex`（poison しない）を使うか、そもそも Connection を task にムーブして所有し共有 Mutex を持たない。(3) 「writer actor は panic しない」ことをレビューで担保する（内部は全 `Result` 伝播、`.unwrap()` ゼロ）方が現実的 — supervisor 頼みの再起動は writer には適さない。この論点をドラフトは完全に見落としている。

### A-3. `panic = "unwind"` 前提と `catch_unwind` の相性、そして clippy deny の実効性 【確信度: 中】

**どこが/なぜ**: §9 は「`panic = "unwind"` をリリースに設定、`abort` だと1タスク panic がプロセスを落とす」とする。正しいが、`tokio::spawn` した future の panic は JoinHandle 経由で捕捉できる一方、**spawn していない同一タスク内の panic** はそのまま伝播する。より重要なのは、clippy の `unwrap_used = "deny"` は**自クレートのソースにしか効かない**こと。依存クレート（rusqlite, twilight, tokio 内部）や、マクロ展開で挿入される `unwrap`、`[]` インデックス（`slice[i]` は clippy unwrap_used に引っかからず panic する）、整数オーバーフロー（debug パニック）、`Vec::remove` 等は野放し。「非起動パスで panic しない」を clippy だけで機械強制できるという含意は**過信**。

**どう直す**: (1) `clippy::indexing_slicing = "deny"`、`arithmetic_side_effects = "deny"` を lint に追加（`[]` と算術 panic を封じる）。(2) integration test で全ルートに対し panic=abort ビルドの fuzz/property test を回し、panic が本当に起きないことを実証。(3) supervisor は「起きないはずの panic の最後の防波堤」と位置づけ、常態の障害処理は型付き Err で行う——ドラフトはこの区別を明示すべき。lint 一覧に上記2つが欠けているのは実害あり。

---

## B. 技術的正当性

### B-1. ts-rs の `#[serde(flatten)]` サポートは実は限定的 — Envelope 設計が破綻しうる 【確信度: 高】

**どこが/なぜ**: §2.3 は「ts-rs は `#[serde(flatten)]` を交差型へ落とせる」を typeshare より優れる根拠にしている。しかし ts-rs の flatten サポートは歴史的にバグが多く、特に **`#[ts(export)]` されるジェネリック型 `Envelope<T>` の `#[serde(flatten)] data: T` は、`T` がジェネリックパラメータのままだと `T & { success: boolean }` を正しく生成できない**ケースがある（ジェネリック境界越しの flatten は ts-rs のバージョンにより `unknown` や型引数の展開漏れになる）。加えて `skip_serializing_if = "Option::is_none"` は**シリアライズ時のランタイム挙動**であって、ts-rs は `message: Option<String>` を `message: string | null`（あるいは `message?: string`）と生成する——この「実際には省略されるが型は present」というズレは、フロントの型安全を偽装する。ドラフトが誇る「単一の真実源」が、まさに一番使う Envelope で崩れる可能性。

**どう直す**: (1) Envelope をジェネリック flatten で表現せず、各レスポンスを**具体型として個別に derive**するか、成功/失敗を tagged enum（`#[serde(tag="success")]` 風でなく `Result` 相当の判別可能ユニオン）にして ts-rs が確実に写せる形にする。(2) 採用前に**実際に `cargo test` で .ts を生成して目視**し、`Envelope<BotView>` が期待通り出るか検証してからアーキ決定する（今のドラフトは「できるはず」で書いており未検証）。(3) 代替として `specta` + `tauri-specta` 系や、`schemars` → `json-schema-to-typescript` パイプラインも比較すべき。ts-rs 一択の断定は危うい。

### B-2. 「読み取り専用プール」でも WAL の writer 直列化とチェックポイントが読みをブロックする 【確信度: 中】

**どこが/なぜ**: §2.2 は「WAL は複数リーダー並行可 → r2d2 で N 本読み → マルチコア並列読み」とする。方向性は正しいが罠が2つ。(1) **WAL checkpoint**（デフォルト自動、1000ページ）が走る間、writer と checkpointer が競合し `SQLITE_BUSY` を返しうる。読み取り専用接続も長時間トランザクションを保持すると WAL が肥大化し checkpoint が進まず（"WAL 肥大化問題"）、性能が線形に劣化する。(2) `busy_timeout=5s` を全接続に設定しているが、**読み取り専用プール接続で 5s ブロックが起きると、その間 tokio の `spawn_blocking` スレッドプール（デフォルト最大512だが実質コア数依存で飽和しうる）を占有**し、他の全ブロッキング処理（他の DB 読み、ファイル IO）を巻き込んで詰まる。「マルチコアで速い」が checkpoint 競合と spawn_blocking 飽和で逆に詰まる。

**どう直す**: (1) `wal_autocheckpoint` を明示調整し、checkpoint を専用の低頻度タスクで `PRAGMA wal_checkpoint(TRUNCATE)` する（writer actor に同居させると読みと衝突しないタイミングで実行できる）。(2) 読み取りは短命トランザクション厳守（`BEGIN` を握らない）を型/lint で強制。(3) `spawn_blocking` ではなく DB 専用の**独立 rayon スレッドプール or 固定サイズの専用スレッド**に読みを隔離し、tokio blocking pool と混ぜない。ドラフトの「読みでマルチコアを活かす」は、この隔離設計まで書かないと机上論。

### B-3. Node/better-sqlite3 と Rust/rusqlite の SQLite **同一プロセス外・同一ファイル**同時オープンの現実 【確信度: 高】

**どこが/なぜ**: これが移行実現性の核心。§0/§2.2 は「両プロセスが同一 SQLite を触る期間」を前提に `busy_timeout=5s` と「書き込みドメインは片方のみ」で守るとする。しかし WAL モードで**別々の OS プロセス**（Node と Rust）が同一 DB を開くとき、WAL/SHM ファイル（`-wal`, `-shm`）は共有され、**両プロセスの busy_timeout・checkpoint 設定が独立**。Node 側（better-sqlite3）は現状 `busy_timeout` **未設定**（前提で確認済み: PRAGMA は WAL と foreign_keys のみ）。つまり Rust が 5s 待っても、**Node 側は待たずに即 `SQLITE_BUSY` を投げて 500 を返す**。移行期間中、Node のあらゆる書き込み（cron の跨ぎ書き込み、reminder、report）が Rust writer のトランザクション中に当たると Node 側が即死する。ドラフトは「書き込みドメインを片方に限定」で回避するとするが、**system_settings, message_logs, tool_outcomes, synapses など複数ドメインが横断的に書くテーブル**（cron、gemini のツール実行ログ、セッション由来の更新）は「ドメイン単位で片方」に綺麗に分割できない。

**どう直す**: (1) 移行の**前提作業として Node 側に `busy_timeout` を先に入れる**（database.ts に1行 `db.pragma("busy_timeout = 5000")`）——これは Rust 移行と独立に今すぐやるべき低リスク改善。(2) 横断テーブル（message_logs/tool_outcomes/synapses/system_settings）を「ドメイン境界で分割」の対象外と認め、**これらの書き込みは Phase H まで Node 一本、Rust は read-only** と明示（synapse は既にそう書いているが tool_outcomes/message_logs も同様に扱う必要——gemini(D) を Rust に回すとツール実行ログを Rust が書きたくなり、ここで衝突する）。(3) 最悪ケースとして「同一 DB 二プロセス書き込みは一切やらない」設計、つまり**書き込みは常に単一プロセス**（Node が全部書く→カットオーバー時に一括で Rust へ、その瞬間 Node の書き込みを止める）も検討すべき。ドラフトの「ドメイン単位で書き込みを分割」は横断テーブルで破綻する。**この点が段階移行の最大リスク**で、ドラフトの楽観が最も危険。

### B-4. Node 側の schema DROP 破壊経路は「Rust が DDL を出さない」だけでは封じきれない 【確信度: 中】

**どこが/なぜ**: §2.2 は「Rust は DDL 不発行 → レガシー DROP を移植しない → 事故が構造的に不可能」とする。Rust 側の事故は確かに防げる。しかし実ファイル（:880-894）を読むと、DROP 発火は `currentVersion === "1" || hasLegacyUsers` かつ `hasAnyLegacyTable` かつ `!== "17"` の複合条件で、**Node 側**が起動時マイグレーションを走らせる限り残り続ける。移行期間中は「スキーマ移行の権限を Node に一本化」する方針なので、**Node は毎起動 `runMigrations` を呼ぶ**。ここで問題: Rust が assert_schema_compatible で "17" を要求する一方、もし何らかの理由（バックアップリストア、`synchronous=NORMAL` 化に伴う WAL 未チェックポイントでのクラッシュ）で `system_settings` の schema_version 行が読めない状態になると、**次の Node 起動が currentVersion="1" を得て `hasAnyLegacyTable`(users/bots は必ず在る) 経由で LEGACY_TABLES 全 DROP** に落ちる。Rust の DDL 不発行はこのシナリオを一切守らない——DROP を撃つのは Node。

**どう直す**: (1) Node 側の DROP 分岐に**追加ガード**を入れる: `schema_version` 行が「読めない」場合と「"1"」を区別し、"1" が本当に v1 スキーマ（`hasLegacyUsers===true`、salt 列なし）のときだけ DROP、それ以外（行欠落だが salt 列あり＝実は最新）は DROP せず fail-fast。これは Rust 移行と独立に今すぐ入れるべき防御。(2) `synchronous=NORMAL` を Rust に入れる（§2.2）と、Node 側は依然デフォルト（WAL では NORMAL 相当だが未明示）で、**クラッシュ時の system_settings 耐久性**が両プロセスで揃わない。移行中は両者の synchronous を合わせるべき。ドラフトは「Rust が DDL 出さないから安全」で思考停止しており、DROP の実行主体（Node）への防御が抜けている。

### B-5. twilight マルチボット接続と ArcSwap トークン差し替えの罠 【確信度: 中】

**どこが/なぜ**: §8 Phase 3 は「twilight マルチ接続、ArcSwap でトークン差替、per-bot アクター」とする。twilight は実在し妥当なクレートだが、(1) **Discord Gateway は identify レート制限**（`max_concurrency`、大規模 bot では 1000 guild/session、identify は 5s に1回等）があり、複数 bot を「per-bot アクター + supervisor 指数バックオフ再起動」で素朴に回すと、**再接続嵐（thundering herd）で Discord 側にレート制限され全 bot が同時に締め出される**。supervisor の指数バックオフは 200ms 起点・上限 30s だが、Discord の identify 制限は分単位のことがあり、200ms 再試行はグローバルレート制限（Cloudflare 1015）を誘発しうる。(2) **ArcSwap でトークンを差し替えても、既存の Gateway WebSocket セッションは古いトークンで確立済み**——トークン変更は再 identify（=再接続）が必要で、ArcSwap の atomic 差し替えだけでは既存接続に反映されない。「差し替えたのに古い接続が生きている」状態の管理をドラフトは書いていない。

**どう直す**: (1) 再接続バックオフは Discord Gateway 用に**専用ポリシー**（`session_start_limit`/`max_concurrency` を尊重、resume を優先、full reconnect は分単位バックオフ）を持ち、汎用 supervisor（§9, 200ms 起点）とは**別扱い**にする。twilight の `Shard` は resume/reconnect を内蔵するので、supervisor で丸ごと再 spawn するのは二重管理でむしろ有害——twilight のシャード再接続に委ね、supervisor はシャードタスクの「本当に死んだ」場合のみ介入する層分けが要る。(2) トークン差し替えは「新トークンで新接続を確立→旧接続を graceful close」の明示シーケンスにする。§9 の汎用 supervisor を bot にそのまま被せる設計は Discord のプロトコル制約を無視しており危険。

### B-6. 拡張性の「他言語プロセス差し込み」とサンドボックス欠如 【確信度: 中】

**どこが/なぜ**: §10 は user カスタムモジュールを「Arc<dyn Tool> 動的マージ」「改行区切り JSON デーモンを DaemonPlugin として一般化し任意の外部 Rust/他言語プロセスを差し込める」とする。絶対制約4（拡張性）には応えているが、**セキュリティ境界が完全に欠落**。任意の外部プロセスを差し込めるなら、それは (a) DB ファイル・シークレット・Redis へ無制限アクセスできるプロセスを user が足せることを意味し、(b) `Arc<dyn Tool>` の動的マージは同一プロセス内で動くので、カスタムツールの panic が(A-2 の通り)共有状態を毒し、無限ループが tokio ランタイムを飽和させる。ドラフトの他の部分は「型で機密を strip」「フェイルクローズ」を徹底しているのに、拡張点だけ**任意コード実行を無防備に許す**のは一貫性がない。WASM サンドボックス（wasmtime + component model）に触れてすらいない。

**どう直す**: (1) カスタムモジュールは**プロセス分離のデーモン契約（改行 JSON）を必須**にし、`Arc<dyn Tool>` in-process 動的ロードは**信頼済み first-party のみ**に限定する（user 拡張は必ず別プロセス）。(2) 別プロセスには権限（DB read-only か、特定テーブルのみか、シークレットアクセス可否）を**マニフェストで宣言**させ、ホストが仲介する（ケイパビリティモデル）。(3) 将来 in-process 拡張を許すなら wasmtime component model でメモリ/CPU/IO を制限。「後付けでなく Phase 0 で境界を確定」と言うなら、**この権限モデルこそ Phase 0 で凍結すべき**——ドラフトは差し込み口だけ作って権限境界を Phase 0 の凍結対象に入れていない。

---

## C. 既存機能の完全性

### C-1. UserScope 型強制は「across-users cron」と Value 入口で穴が開く 【確信度: 中】

**どこが/なぜ**: §4 の `UserScope` で user_id 越境を型封じするのは優れた案。ただし (1) `CronScan::overdue_across_users()` を「別トレイトに隔離」するが、**cron が拾った across-users の Todo を gemini/notifier に渡す経路**で user_id の取り違えが起きうる（reminder を別 user に送る事故）。型で「この Todo はどの user のものか」を Todo 値自体に持たせ、送信先解決も同じ user_id から導く不変条件が要る——トレイト分離だけでは「跨いで取得した後の混線」を防げない。(2) A-1 の `Value` args で `user_id` を受ける経路があると、UserScope の型強制を**迂回**して生 user_id が API から入る。棚卸し #2 §5 の v7 クロステナント事故（`bot_mcp_access` owner_id 欠落）の再発防止を謳うが、`bot_mcp_access` のような**多対多の所有権テーブル**は「単一 user_id スコープ」では表現できず、UserScope だけでは owner_id/bot_id 双方の検証が要る。

**どう直す**: (1) Todo 等のドメイン型に `owner: UserId` を**非公開フィールド**で持たせ、`Deserialize` で外部から詐称できないようにする（wire DTO とは別型）。notifier は `todo.owner` からのみ送信先を導く。(2) 多対多テーブルは `(OwnerScope, ResourceId)` の複合スコープ型を別途定義。(3) A-1 の型付き Args 化と併せて、user_id が `Value` から生で入る経路をゼロにする。ドラフトの UserScope は good だが「across-users 後の混線」と「多対多所有権」の2ケースで不足。

### C-2. 機密フェイルクローズ: row 型に Serialize を derive しない、は verify されない 【確信度: 中】

**どこが/なぜ**: §2.3 の「BotRecord に Serialize を実装しない → コンパイル時に JSON 化不能」は zod ランタイム strip より強く、良い設計。ただし穴が2つ。(1) `serde_json::Value` を経由すると `to_value(&bot_record)` は **Serialize が無ければコンパイルエラー**になるので守られるが、`format!("{:?}", bot_record)` や **tracing のログに `?bot_record` を出す**と Debug 経由で `*_encrypted`/`iv`/`tag` が**平文ログに漏れる**。Debug は `#[derive(Debug)]` で付いており（error.rs でも Debug derive 前提）、機密フィールドを持つ row 型に Debug が付くと**ログ漏洩**が起きる。(2) ドラフトは wire DTO 側を allowlist にするが、**新しい機密列が DB に追加されたとき**、それが誤って wire DTO に足されないことを保証する仕組み（現行 zod allowlist の「明示列挙」に相当）が、From 変換の手書きに委ねられており drift しうる。

**どう直す**: (1) 機密フィールドは `Secret<String>`（`secrecy` クレート）でラップし、Debug/Display が `[REDACTED]` を出すようにする——ログ漏洩を型で封じる。row 型の `#[derive(Debug)]` を安易に付けない lint も検討。(2) wire DTO への機密列混入を防ぐため、`BotView` に「機密でないことを表すマーカートレイト」を要求するか、機密列を持つ型からの From を1箇所に集約し**その1関数だけをレビュー対象**にする（現行 zod allowlist と同じ監査点を1つに保つ）。ドラフトは「Serialize 無し」で満足しているが Debug/ログ経路が抜けている。

### C-3. CSRF/CSP/Sec-Fetch とデグレード動作の等価性検証 【確信度: 中】

**どこが/なぜ**: §3 verify ゲートで CSP `script-src 'self'`・CSRF 403・`Sec-Fetch-Site` を curl 検証、`X-Forwarded-Proto` を nginx で立てる、は妥当。ただし (1) **`Sec-Fetch-Site` 判定は Node/Rust で実装が別**になり、ゴールデンテスト（同一リクエストを両方へ）でも `Sec-Fetch-*` ヘッダを付けたブラウザ相当のリクエストを網羅しないと、Rust 側で判定ロジックの微妙な差（例: `same-origin` vs `none` の扱い、GET の扱い）が漏れる。curl の手動ヘッダでは実ブラウザの組み合わせを再現しきれない。(2) デグレード動作の等価性——「直近履歴のみ」「memory フォールバック」（§9）——は**ゴールデンテストでは検出できない**（正常系 JSON 一致しか見ない）。Gemini/Redis を意図的に落とした状態での縮退レスポンスが Node と Rust で等価かを検証するゲートが無い。移行後「障害時だけ挙動が変わる」回帰が verify をすり抜ける。

**どう直す**: (1) `Sec-Fetch-Site` の全組み合わせ（same-origin/same-site/cross-site/none × GET/POST/unsafe method）をテーブル駆動テストにし、Node の実装を**仕様として抽出**してから Rust に写す（両実装比較でなく、まず正解表を作る）。(2) **障害注入ゴールデンテスト**を追加: Redis/Gemini をモックで落とし、縮退レスポンスの形が Node と一致するか assert。§3 のゲートに「劣化縮退の等価性」を明示的に含める。ドラフトの3層ゲートは正常系に偏っており、まさに絶対制約2の劣化縮退が検証されない。

---

## D. 移行実現性

### D-1. 「Phase 0 で全型凍結、以後変更しない」は 9 ドメイン並行で必ず破れる 【確信度: 高】

**どこが/なぜ**: §4/§8 は error 列挙型・wire DTO・Tool/Repo/UserScope を Phase 0 で凍結し「以後変更しない」で並行衝突ゼロを主張。これは**理想論として破綻する**。理由: (1) T1 の 9 ドメインを実装し始めると、必ず「foundation に足りない共通型/エラーバリアント/トレイトメソッド」が発見される（例: あるドメインだけ必要な `DbError` バリアント、Tool トレイトに `ctx` で渡すべき追加情報）。凍結を守るとドメイン側が歪んだ回避策を書き、破ると全 worktree が再ビルド＆衝突。(2) `AppError` の網羅 match（`_ =>` 禁止）は**新バリアント追加で全 match がコンパイルエラー**になる設計——これは検出力として good だが、**並行中に foundation の AppError にバリアントを1つ足すと、9 worktree 全部の HTTP 層がビルド不能**になる。凍結の強制力が高いほど、変更の伝播コストが致命的。

**どう直す**: (1) 「凍結」でなく「**追加のみ許可・変更/削除禁止（semver additive）**」に緩める。エラー enum は `#[non_exhaustive]` を付け、`AppError::status()` の match に**意図的に**限定的な `_ => S::INTERNAL_SERVER_ERROR` を1箇所だけ許可（新バリアントで即クラッシュを避ける）——ただしこれは絶対制約1の「`_ =>` 禁止」と真っ向から衝突する。この矛盾をドラフトは自覚していない: **`#[non_exhaustive]` + 網羅 match は両立しない**。(2) foundation 変更を吸収する「foundation メンテナ役の単一エージェント」を Phase 1 と**並走**させ、追加要求をシリアライズして反映→他 worktree は定期 rebase。(3) Tool/Repo トレイトはメソッド追加が全実装を壊すので、`ctx: &ToolContext` を**拡張可能な構造体**（フィールド追加は既存実装を壊さない）にして、トレイトメソッド自体の変更頻度を下げる。ドラフトの「凍結すれば衝突ゼロ」は9並行の現実で最も楽観的な仮定。

### D-2. ゴールデン差分テスト（Node vs Rust 同時起動）の前提が重い 【確信度: 中】

**どこが/なぜ**: §3 テストゲートは「同一リクエストを Node(:7854) と Rust(:7900) 両方へ投げ JSON 一致を assert」。理想だが実現コストが過小評価。(1) **状態を持つエンドポイント**（POST で todo 作成→GET で一覧）は、両プロセスが**同一 DB を共有**していないと結果が一致しない。共有すると B-3 の二重書き込み問題に直撃。別 DB にすると初期状態を完全同期する必要があり、ID 採番（AUTOINCREMENT）・タイムスタンプ（`datetime('now','localtime')`）が両者で必ずズレる。(2) `datetime('now','localtime')` の**localtime 依存**は、Rust 側の時刻生成と Node 側で秒単位のズレが出て JSON 不一致になる（created_at 比較で毎回落ちる）。ゴールデンテストがフレーキーになり形骸化するリスク。

**どう直す**: (1) 比較は**時刻・自動採番 ID を正規化（マスク）**してから diff（JSON の created_at/id を除外 or 相対検証）。(2) 状態依存テストは「同一シードの読み取り専用 DB スナップショット」に対する GET のみをゴールデン対象にし、書き込み系は**契約テスト**（リクエスト/レスポンスの schema 一致）に留める。(3) `datetime('now','localtime')` は移行を機に **UTC + アプリ層フォーマット**へ寄せる検討（localtime は二プロセス・タイムゾーン差でバグの温床）。ドラフトのゴールデンテストは「両起動して比較」を軽く書きすぎ。

### D-3. workspace 化と既存 rust_crawler/rust_synapse の Cargo.lock 統合リスク 【確信度: 中】

**どこが/なぜ**: §6 は `[workspace] members = ["crates/*", "src/rust_crawler", "src/rust_synapse"]`、`Cargo.lock` ルート集約とする。しかし現状この2クレートは**独立 Cargo.toml・独立 lock**（前提で確認）。workspace 化すると (1) **依存バージョンが workspace 全体で統一解決**され、synapse の onnx（重い、特定バージョン固定の可能性）と新クレート群の依存が**バージョン衝突**しうる。特に synapse が古い tokio/特定 ort(onnxruntime) を要求すると、新コードの最新 tokio と揃わず lock 解決が破綻 or どちらかがビルド不能。(2) メモリの落とし穴（`worktree-node-modules-symlink-pitfall`）は node_modules だが、**Cargo の worktree では `target/` が worktree 間で共有されない**とビルドが激遅になり、共有すると**別 feature フラグ（synapse の onnx feature）での再ビルド衝突**が起きる。§6 は node_modules の罠は書くが target/ の罠を書いていない。

**どう直す**: (1) workspace 化前に rust_crawler/rust_synapse の依存を**新クレート群と互換なバージョンへ上げられるか**を先に検証（特に ort/onnx, tokio）。非互換なら synapse を**当面 workspace 外の独立クレートのまま**にし（§5/§8 でも「別 OS プロセスのまま」推奨なので workspace member 化は必須でない）、Phase 5 まで統合を遅延。(2) worktree 間で `CARGO_TARGET_DIR` を**共有せず**各 worktree 独立の target にする（ビルドは遅いが衝突ゼロ）、または sccache で実質共有。§6 の「1クレート1worktree で物理的に交わらない」は target/ を考慮すると素朴すぎる。ドラフトは Cargo workspace の feature 統一/lock 解決/target 共有の3罠を見落とし。

### D-4. nginx `location = /api/login` 等の完全一致 + WebSocket 振り分けの正しさ 【確信度: 中】

**どこが/なぜ**: §0 の nginx conf に技術的な誤り・危うさ。(1) `location = /api/auth/device/*` を「解禁」する意図だが `=`（完全一致）は**ワイルドカード不可**——ドラフトの表では `/api/auth/device/*` を Rust へ回すとあるが、`location =` では `/api/auth/device/xxx` にマッチしない。`^~ /api/auth/device/` が正しい。(2) `map $migration_default $_ { ... }` は**変数を定義するだけで使われていない**デッドコード（`$_` はどこにも参照されない）。(3) `keepalive 32` を upstream に書くなら、location 側で `proxy_http_version 1.1; proxy_set_header Connection "";` が**非WS location にも必要**（さもないと keepalive が効かない）——WS location にしか Connection ヘッダを書いていない。(4) **CSP `script-src 'self'` と nginx**: hashed asset の immutable キャッシュ（verify ゲートが確認）を nginx が担うが、Rust/Node どちらが静的配信するかで `Cache-Control`/`ETag` の生成が変わり、CSP nonce/hash を使う場合 nginx 通過で壊れうる。ドラフトは静的配信を Phase B で Rust に回すが、nginx との Cache-Control 責任分界が未定義。

**どう直す**: (1) device flow は `^~ /api/auth/device/` に修正。(2) 未使用の `map` を削除するか、実際にフォールバック制御に使うなら `proxy_pass $target` 形式で結線。(3) 全 proxy_pass location に `proxy_http_version 1.1; proxy_set_header Connection "";`（WS 以外）を入れ keepalive を実効化。(4) 静的配信の Cache-Control/immutable/ETag の生成主体を「Rust が生成、nginx は透過」か「nginx が付与」か明文化し、verify ゲートの CSP/immutable チェックがどちらでも通ることを確認。nginx conf は「切替の唯一の真実源」を自称する割に、書かれた設定に実バグが複数。

---

## 妥当と認める主要判断（反証できなかった点）

- **nginx location 単位のストラングラー + アプリ層フラグ不採用**（§0）: ロールバックを `reload` 1発に集約する判断は妥当。状態を1ファイルに集中させる利点は本物。ただし D-4 の実装バグは要修正。
- **Rust は移行中 DDL を発行しない**（§2.2）: 方針自体は妥当で正しい。ただし B-4 の通り「DROP を撃つ主体は Node」なので Node 側防御が別途必須——Rust 側の DDL 不発行だけでは不十分、という補完が要る。
- **writer actor による書き込み直列化**（§2.2）: better-sqlite3 の単一書き手セマンティクス維持として方向は妥当。ただし A-2（SPOF/panic 復旧）と B-3（二プロセス）の補強が必須。
- **row 型に Serialize を derive しない機密封じ**（§2.3）: zod ランタイムより強い、優れた構造化。C-2 の Debug/ログ経路だけ塞げば良い。
- **UserScope 型でクエリスコープ強制**（§4）: v7 事故の再発防止として良い設計。C-1 の2ケース補完で完成する。

---

## 最優先で潰すべき論点（この計画の成否を分ける順）

1. **B-3（二プロセス同一 SQLite 書き込み + Node の busy_timeout 未設定）** — 段階移行の最大の地雷。横断テーブルは「ドメイン単位で片方」に分割できず、Node が即 SQLITE_BUSY で死ぬ。**今すぐ Node に busy_timeout を入れ、横断テーブルは Phase H まで Node 書き込み一本**に修正。
2. **D-1（凍結 vs `#[non_exhaustive]` の矛盾）** — 「凍結で衝突ゼロ」と「網羅 match で `_` 禁止」は9並行下で両立不能。**additive-only + foundation メンテナ並走 + ctx 構造体化**へ。絶対制約1と設計が正面衝突している自覚が要る。
3. **A-1（Value 入口が anyhow 禁止を骨抜き）** — Tool の `Args` 型付け・schemars 宣言自動生成で入口を型化しないと、制約1は入口で破れている。
4. **A-2（writer actor の panic/poison 復旧欠如）** — SPOF が落ちると全書き込み死。supervisor 頼みでなく writer 内を panic-free にする方針明記。
5. **B-4（Node 側 DROP 破壊経路の残存）** — schema_version 行欠落で Node が全 DROP。Node 側に「行欠落 ≠ v1」ガードを追加。

これらはいずれも「計画が動き出してから顕在化し、動き出すと後戻りが高コスト」な論点。特に 1 と 2 は Phase 0/Phase 1 の設計前提そのものを揺るがすため、着手前に解消すべき。

---
