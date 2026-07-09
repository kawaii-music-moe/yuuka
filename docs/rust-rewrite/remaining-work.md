# Rust 移行 — 残作業ロードマップ（本番投入までの ToDo 全集）

- 最終更新: 2026-07-09（実装セッション 2026-07-09d で **P1-1 認証発行 + P1-2 会話（オーケストレーション + /ws/chat）** を着地）
- 対象ブランチ: `feature/rust-rewrite`（未 push）
- git HEAD: `7a4c07f`（旧セッション）+ 本セッションの P1-1 コミット群
- 前提資料: [review-2026-07-06-fix-policy.md](review-2026-07-06-fix-policy.md)（修正方針の唯一の基準）・[review-2026-07-09-batch4-6.md](review-2026-07-09-batch4-6.md)・[PLAN.md](PLAN.md) §11（移行ロードマップ）
- **本セッション（2026-07-09d）の成果**: **P1-1 のうち資格情報認証発行**を実装 — `SessionStore` を検証専用→**発行+検証**へ拡張（Node `sessionService.ts` パリティ・in-memory フォールバック）、`/api/setup/status`・`/api/setup`・`/api/register`・`/api/register/verify`・`/api/login`・`/api/logout`・`/api/users` の 7 ルート、bcrypt cost 12（`$2b$` Node 相互運用・`spawn_blocking`）、パスワードポリシー・招待コード（seed/consume）・監査ログ・DM チャレンジ登録状態機械（`RegistrationDm` ポート・Discord live まで `NullRegistrationDm`）・レート制限・`ConnectInfo` クライアント IP・Cookie 発行。`main.rs` に `AuthRuntime` 配線（`SessionStore` を発行↔検証で共有）+ invite 起動時シード。多エージェント・アドバーサリレビュー実施 → 確定 2 指摘（legal URL の `system_settings` 優先=Node `publicLegalUrls` パリティ、暗号未設定時の setup 半端 commit 回避）を修正。機械ゲート全緑（build/release/clippy-D/deny/**test 278**）。統合テストで **ブラウザ login/setup → セッション発行 → /api/me 200** を通しで確認。
- **前セッション（2026-07-09c）の成果**: P0-1〜4・P1-5（暗号層）・P1-6/7・P1-4 アダプタ・P3-4/5。

---

## 0. 現状サマリ（判定）

**「ほぼ本番（Node 完全置き換え）」としてはまだ使えない。** 基盤設計は堅牢でユニットは全緑だが、ユーザーが実際に触れる経路（ログイン・会話・Discord・通知・秘密情報復号）が揃っていない。

| 面 | 実測 |
|---|---|
| `cargo build --release --workspace` | ✅ exit 0 |
| `cargo clippy --workspace --all-targets` | ✅ exit 0（ts-rs 良性 warning のみ） |
| `cargo test --workspace` | ✅ **278 passed / 0 failed**（P1-1 で +30: 認証発行・セッション発行・bcrypt・pending・settings 等） |
| `cargo deny check` | ✅ **exit 0**（bcrypt/blowfish 追加後も緑） |
| 保存時暗号層（Argon2id/AES-256-GCM） | ✅ **実装済**（P1-5・Node ゴールデンベクタでバイト単位パリティ・鍵ローテ起動時配線） |
| 認証発行（login/setup/logout/register/users） | ✅ **実装済**（P1-1・セッション発行 + bcrypt + 招待 + 監査 + レート制限。OAuth は残） |
| HTTP ルート被覆 | 34 / 152 パス ≒ **22%**（`/api/me` + 認証 7 ルートを追加） |
| Gemini ツール被覆 | 24 / 87 native ≒ **28%**（動的 MCP 0） |
| チャットオーケストレーション（秘書ターン） | ✅ **実装済**（P1-2・`yuuka-orchestrator`・実 TurnProcessor・統合テスト緑） |
| WebSocket `/ws/chat`（デスクトップ会話） | ✅ **実装済**（P1-2・Bearer 認証 + ready/status/done・live 統合テスト緑。interaction/deferred は縮退） |
| Gemini FC ループ本体 | 1:1 移植 ≒ 95% 完成（**オーケストレーション層から到達可能に**・P1-2） |
| 通知配信ブリッジ | 🔶 **アダプタ実装済**（P1-4）・main の messenger 差し替え（P1-3）待ち |
| 登録 DM ブリッジ | 🔶 **ポート実装済**（P1-1・`RegistrationDm`）・Discord live 化（P1-3）で adapter 配線待ち |

**進め方の2経路:**
- **経路 A（strangler 並走カナリア）** — Node が認証/会話/Discord/暗号を担い、Rust は移行済み CRUD の一部だけを共有 Redis セッション前提で配信。§7-A の前提を満たせば数日規模で到達可能。**（P1-1 により Rust 単独でのセッション発行も可能になった＝Rust だけでログイン→CRUD が回る。）**
- **経路 B（単独ほぼ本番）** — Rust だけで完結。P1〜P2 のブロッカーを全て潰す必要があり、現状は全体の約 1/4。相当先。

---

## 優先度の凡例

- **P0 — データ安全 / 起動前必須**: これを飛ばすとデータ喪失・即クラッシュ。何より先。
- **P1 — 致命ブロッカー**: 単独「ほぼ本番」を名乗るのに不可欠。無いとユーザーが何もできない。
- **P2 — 機能パリティ**: Node にあって Rust に無い機能面。順次埋める。
- **P3 — 品質 / 運用衛生**: CI・fmt・古い記述・LOW 指摘など。

チェックボックスはそのまま進捗トラッキングに使用可。

---

## P0 — データ安全 / 起動前必須（最優先）

- [x] **P0-1 未コミットの V17 マイグレーション修正をコミットし、以後 V17 を凍結する** — 済（`61934e8`）
  - 内容: working tree にある `crates/yuuka-db/migrations/V17__baseline.sql`（fts5 仮想テーブル + トリガ ×3 に `IF NOT EXISTS`、末尾に `system_settings.schema_version='17'` upsert）と `crates/yuuka-db/src/schema.rs`（refinery の SQLITE_BUSY/LOCKED を Transient 分類）を確定コミットする。
  - なぜ: 現 HEAD の V17 のまま既存 DB へ起動すると「object already exists」で**移行 Fatal**。かつ `schema_version='17'` スタンプが無い DB を後で Node が開くと legacy-v1 誤検出で**コアテーブル全 DROP（全データ喪失）**。
  - 完了条件: コミット済み。カットオーバー後は **V17 を二度と編集せず、変更は V18+** で行う旨を運用ルールとして明記（[review-2026-07-06-fix-policy.md](review-2026-07-06-fix-policy.md) §157）。
  - 注意: 旧 Rust バイナリで一度でも migrate した dev/test DB があると `refinery_schema_history` に旧 checksum が残り `abort_divergent` で Fatal → その DB は `refinery_schema_history` 削除か作り直し。現行 `data/yuuka.db` は履歴なしなので修正込みなら初回起動クリーン。

- [x] **P0-2 cron の所有を片側専任にする（二重 writer ハザード回避）** — 済（`deploy/README.md` 「移行期の運用注意」に cron 所有ルール明記。コンテナ Rust=cron 所有／旧 systemd 同時起動禁止／経路 A は `YUUKA_RUST_CRON=0`）
  - 内容: `YUUKA_RUST_CRON=1` が `Dockerfile:116` に焼き込まれている。Node cron と同時稼働させない。Rust cron を使うなら Node 側 cron を停止、使わないなら Rust の env を落とす。
  - なぜ: 同一 SQLite WAL への writer 同時 2 つは即 `SQLITE_BUSY`（設計最重要リスク R-1）。
  - 完了条件: 稼働構成で cron を回すプロセスが厳密に 1 つであることを確認。

- [x] **P0-3 nginx strangler のポート整合とルーティング明示** — 済（`deploy/nginx/yuuka.conf`：現行 Rust 直起動では本ファイル不使用＝Tunnel→:7854 直達である旨を冒頭バナーで明記。経路 A 用としては catch-all と `/ws/chat` を `yuuka_node` に戻し「既定 Node・移行済みだけ Rust」の fail-safe に整合。Rust 並走時は `PORT=7900` 起動が前提と明示）
  - 内容: `deploy/nginx/yuuka.conf` の `yuuka_rust` upstream は `:7900` を指すが、コンテナは `:7854` を listen。catch-all と `/ws/chat` は Rust を向くのにコメントは「fail-safe で Node」と矛盾。ポートを合わせ、各 location の向き先を実態に一致させる（または Cloudflare Tunnel 直叩きでこのファイルを使わない旨を明記）。
  - なぜ: 現状のまま適用すると catch-all が存在しないポートを叩き **502**。
  - 完了条件: 実際に流すトラフィックの経路が設定と一致し、意図通り Node/Rust に分岐する。

- [x] **P0-4 起動前に DB を事前シードする運用を明記** — 済（`deploy/README.md` 「DB は起動前に必ず存在させる」。Rust は `SQLITE_OPEN_CREATE` を付けず無ければ起動失敗＝旧 Node 作成の `data/yuuka.db` を再利用する手順を記載）
  - 内容: Rust は存在しない DB を作らない（`open_conn` がエラー）。fresh インスタンスは Node が作成した `data/yuuka.db` を再利用する。
  - 完了条件: 初回起動手順書に「DB 事前作成」を記載。

---

## P1 — 致命ブロッカー（単独ほぼ本番に不可欠）

- [~] **P1-1 認証の発行経路** — **資格情報ログインは実装済み・OAuth は残（P2-A へ）**
  - 済（2026-07-09d）: `/api/login` `/api/logout` `/api/register` `/api/register/verify` `/api/setup` `/api/setup/status` `/api/users`（Node `authRoutes.ts` パリティ）を `crates/yuuka-auth/`（`routes.rs`・`AuthRuntime`）に実装。`SessionStore` に**発行**（`create`/`destroy` + in-memory フォールバック）を追加し、`CompositeAuth`（検証）と**同一ストアを共有**（`main.rs` で `sessions.clone()`）。bcrypt cost 12（`$2b$`・Node bcryptjs 相互運用・`spawn_blocking`）、`verifyPasswordConstantTime`（不在ユーザーもダミー比較でタイミングオラクル対策）、パスワードポリシー（8 文字/2 種/denylist fail-open・UTF-16 長）、招待コード（`is_valid`/atomic consume/起動時 seed）、監査ログ、DM チャレンジ登録（`PendingStore` + `RegistrationDm` ポート・`NullRegistrationDm` 縮退）、レート制限（login lockout・register-send window）、`ConnectInfo`+XFF クライアント IP、Cookie 発行（`setSessionCookie` パリティ）。config に `INVITE_CODES`/`ADMIN_DISCORD_IDS` 追加。統合テストで **login/setup → セッション発行 → /api/me 200** を確認。
  - 残: **Google/Discord OAuth フロー**（`settingsRoutes.ts` の url + callback・`GOOGLE_CLIENT_ID`/`GOOGLE_CLIENT_SECRET`）は未移植 → **P2-A（設定系ルート）へ移す**。
  - 残（P1-3 と一体）: `register` の確認コード DM は `NullRegistrationDm` のため現状 502。Discord live 化で `impl RegistrationDm for DiscordMessenger`（既存 `send_registration_code_dm` へ委譲・notify_bridge と同型）を足し `Arc<DiscordMessenger>` を注入すれば届く。
  - 残（経路 A のライブ確認）: 共有 Redis 稼働下で Node が発行した Cookie を Rust が検証、Rust が発行した Cookie を Node が検証、の相互運用を実 Redis で確認（キー書式・sha256hex・camelCase JSON は一致済み）。

- [~] **P1-2 会話経路** — **オーケストレーション中核（実 TurnProcessor）は実装済み・transport 配線が残**
  - 済（2026-07-09d・新 `crates/yuuka-orchestrator`）: **チャットオーケストレーション層 + 実 `TurnProcessor`** を実装。`ChatEngine::secretary_turn`＝Node `processMessage`（秘書経路）パリティ: リッチ返信フラグ → ユーザー発言永続化（`describeIncomingMessage`）→ 直近 15 件を古い順ロード → `contents` 組立（連続同一 role を `\n` 結合・添付 inline data）→ `buildSystemInstruction`（DEFAULT_PERSONA/ペルソナ + 情報保存/承認/リッチ返信/音声/ファクトチェック/機能一覧/システムルール[現在日時・**未実行の完了報告禁止**]を verbatim 移植）→ ユーザーの Gemini キー復号（`SystemCrypto`・秘書経路は `users` の鍵）→ **FC ループ**（既存 `run_function_calling_loop`）→ アシスタント応答を必ず永続化 → `TurnReply`。`message_log`/`user`/`persona` repo 新設（`message_logs` の add/recent_context[floor=`system_settings` `context_floor:`]/clear_context）。`impl TurnProcessor for ChatEngine`（`process_secretary`/`parse_receipt` 実装）。`GeminiFactory` トレイトで fake backend 注入 → 統合テストで user 発言→履歴→鍵復号→FC ループ→assistant 保存→reply を通し検証。
  - 済 **(1) `/ws/chat` WebSocket transport**（2026-07-09d・`crates/yuuka-supervisor/src/ws.rs`）: axum WS upgrade + **Bearer（desktop token）認証**（`AuthenticatedUser` extractor）+ `?botId=` 束縛/`has_bot_access` 検証（未指定 system_default）+ WS-native ping/pong 30s。フレームは `clients/desktop/src/model.rs` 契約と一致: 受信 msg/reset/ping/interaction、送信 **ready/status/done/error**。`ready` は `listBotsForUser`（bots + bot_shares active）を BotInfo 化・束縛 Bot は合成フォールバック（`toBotInfo` パリティ）。msg → Gemini キー事前チェック（`no_gemini_key`）→ `ChatEngine::secretary_turn` → status（thinking/writing）→ done。reset → `clear_context`。送信は split+mpsc の writer タスクへ集約。`build_app` に `ws_routes` 追加・main.rs で `ChatEngine`（tool registry + crypto + db）構築 + 配線。**live 統合テスト**（実 WS クライアント → 実サーバ・fake Gemini）で ready→msg→done を通し検証。
  - 残 **(2) 縮退シームの本体化（後続・任意）**: ターンプランナー・シナプス想起（Phase H daemon）・**非同期配信**（interim/push・deferred）・**interaction 配信**（コンポーネント/update）・**能力ゲート**（現状全ツール露出＝P2-B）・返信チェーン・上限は raw バイト長判定（現状フレーム長概算）。
  - 残 **(3) Discord 経路**: `ChatEngine`（`Arc<dyn TurnProcessor>`）を P1-3 の `DiscordManager` へ注入。汎用モード（guild/owner DM・Bot 専用キー）の `process_guild`/`process_bot_dm` は現状未実装（P1-3 で実装）。
  - 完了条件: **デスクトップから 1 往復の会話が成立しツール呼び出しが実行される**（`/ws/chat` で成立・Web ダッシュボードは WS チャット未使用のため対象外）。残は Discord 経路（P1-3）。

- [ ] **P1-3 Discord を実起動し、Supervisor 配下へ配線する**
  - 内容: `main.rs` が `DiscordManager` を構築せず `.prepare()` もテナント登録もしない。twilight 転送層（shard loop・マルチテナント・message/button ルーティング・DM ヘルパ・権限ガード）は構築 + test 済みだが**不活性**。
  - なぜ: 起動しないので bot が一切反応しない。**秘書経路の実 TurnProcessor（`yuuka_orchestrator::ChatEngine`）は P1-2 で実装済み**＝`Arc<dyn TurnProcessor>` として注入できる（汎用モード guild/owner DM の `process_guild`/`process_bot_dm` は本 Phase で実装）。
  - 対象: `crates/yuuka-supervisor/src/main.rs`、`crates/yuuka-supervisor/src/discord.rs`（既存 `DiscordTenantService` アダプタは未使用のまま存在）。実ポート（`BotDirectory`/`MembershipService`/`RateLimiter`）の DB 実装も必要（現状 trait のみ・bot repo 未整備）。
  - 完了条件: main が Discord テナントを Supervisor に登録し、メンション/DM に実応答。restart/backoff 監督下。

- [~] **P1-4 通知配信の橋渡し（Messenger → Notifier）を実装する** — **アダプタ実装済み・配線は P1-3 待ち**
  - 済: `crates/yuuka-discord/src/notify_bridge.rs`＝`impl yuuka_services::Notifier for DiscordMessenger`（`NotifyTarget`↔`DeliverTarget` 変換＝Default→DM・Channel 透過、空本文 false、`TurnReply::text` 化して `send_to_user` へ委譲）。孤児規則により discord 側に実装（services→discord 逆依存なし＝非循環）。target 写像を単体テストで凍結。
  - 残（P1-3 と一体）: `main.rs` の `NullNotifier` を、Discord live 化で構築した `Arc<DiscordMessenger>` へ差し替える 1 行のみ。
  - なぜ: reminder/birthday/payment サービスは動くが送信が常に `false` で **リマインダーが永遠に届かない**。アダプタが埋まったので、あとは messenger を注入すれば届く。
  - 完了条件: 期限到来リマインドが実 Discord チャンネルへ届く（＝P1-3 の messenger 構築 + 上記差し替え）。

- [x] **P1-5 秘密情報の暗号層（Argon2id + AES-256-GCM）を実装する** — 済（新 `crates/yuuka-crypto`）。scrypt システム鍵 + Argon2id ユーザー鍵 + AES-256-GCM を Node `src/utils/crypto.ts` と **バイト単位パリティ**で実装（Node 実出力のゴールデンベクタ `golden_parity_with_node` で凍結）。`config` が `YUUKA_ENCRYPTION_SECRET`/`_NEW` を `SecretString` で読込。鍵ローテ（`rotate_secret_key`＝Node `ENCRYPTED_COLUMNS` パリティ）を supervisor 起動時に **writer actor 上で 1 回**実行（R-2 遵守）。**残（消費側の配線は P2）**: credential register/decrypt ルート・Discord/Gemini トークン復号は `SystemCrypto`/`decrypt_text` を呼ぶだけ（本層で提供済み）。
  - 内容: **Rust 全クレートが読む env は `YUUKA_RUST_CRON` ただ 1 つ**。`YUUKA_ENCRYPTION_SECRET` / `YUUKA_ENCRYPTION_SECRET_NEW`（鍵ローテ）/ `GOOGLE_CLIENT_SECRET` は未読。復号層が「本クレート外」のまま存在しない（`yuuka-credential` は register/decrypt を deferred）。
  - なぜ: 保存済み資格情報・Discord トークン・Gemini キー・Google リフレッシュトークンの**復号が全滅**。at-rest 秘密に依存する機能が全て非機能。
  - 対象: 新規 crypto crate（Argon2id 鍵導出 + AES-256-GCM）、`crates/yuuka-core/src/secrets.rs`（現状 in-memory `SecretString` ラッパのみ）、`crates/yuuka-credential/`、config で `YUUKA_ENCRYPTION_SECRET` 必須化（Node は未設定なら起動しない）。
  - 完了条件: DB の `encrypted_password/iv/auth_tag` を復号でき、鍵ローテ（`_NEW`）も Node パリティ。

- [x] **P1-6 未コミットの Batch 4/5/6 修正を確定コミットする** — 済（`61934e8`・P0-1 と同一コミット）
  - 内容: working tree の M-1（認証縮退 Bearer 継続 + `tracing::warn!`）/ M-2（413 区別・空ボディ `{}` 化）/ M-4（Auth::Backend のみ Transient）/ M-5（writer `catch_unwind` panic 隔離）/ M-6（連鎖削除 parity）を含む 12 ファイル差分。レビュー承認済み（[review-2026-07-09-batch4-6.md](review-2026-07-09-batch4-6.md)）。
  - 完了条件: コミット済み。P0-1 と同一コミットに含めてよい。

- [x] **P1-7 `cargo deny` を緑に戻す** — 済（`19443ae`・選択肢 (i) 採用: crawler/synapse を members→exclude へ。`cargo deny check` exit 0）
  - 内容: 補助クレート由来の 3 系統を解消 —（a）`fxhash` unmaintained RUSTSEC-2025-0057（scraper←`yuuka-crawler`）、（b）MPL-2.0 未許可 ×4、（c）`yuuka-crawler`/`yuuka-synapse` の license 欄欠落で unlicensed。
  - なぜ: 計画（`Cargo.toml` 冒頭）は「最終 Phase H まで crawler/synapse を members に入れない」としていたが取り込まれており、workspace ゲートが赤。
  - 選択肢: (i) 両クレートを members から exclude に戻す（計画準拠・最速）、(ii) license 欄追加 + `deny.toml` に MPL-2.0 許可追加 + fxhash を `rustc-hash` へ差し替え or advisory 個別許可。
  - 完了条件: `cargo deny check` exit 0。

---

## P2 — 機能パリティ（Node にあって Rust に無い）

### P2-A Web ルート（27/152 → 埋める）

- [ ] 管理系 `/api/admin/*`（users/bots/invite-codes/stats/audit-logs/system-settings 等 ~13）— Node `adminRoutes.ts`
- [ ] 設定系 `/api/settings/*`（profile/password/gemini/discord/google OAuth/backup/delete-account 等 ~12）— `settingsRoutes.ts`
- [ ] Bot 管理 `/api/bots` `/profile` `/shares*` `/sync-discord` — `botRoutes.ts`
- [ ] Bot 属性 `/api/bots/attributes` `/modules` `/presets` `/assistant/*`（~13）— `botAttributeRoutes.ts`
- [ ] MCP `/api/mcp-servers*` `/proxy/mcp/:id/mcp`（~8）— `mcpRoutes.ts`
- [ ] Webhooks `/api/webhooks*` `/hook/:token`（~6）— `webhookRoutes.ts`
- [ ] Integrated `/api/integrated/*`（google accounts/calendars/grants・bot start/stop/restart 等 ~12）— `integratedRoutes.ts`
- [ ] Device/Desktop `/api/auth/device/*` `/api/devices*` `/api/desktop/*` — `deviceAuthRoutes.ts` 他
- [ ] Delivery `/api/briefing-config` `/briefing/test` `/report-configs*` — `deliveryRoutes.ts`
- [ ] Member requests `/api/bots/member-requests*` — `memberRequestRoutes.ts`
- [ ] Persona marketplace `/api/personas/marketplace*` `/activate` `/import` `/publish` `/admin/personas/*`（現状 save/delete/list のみ）
- [ ] `/api/status`（ヘルスチェック）・`/api/setup/status`（コメントで「後続」と言及したまま未配線）
- [ ] ドメイン別の残ルート:
  - [ ] todo（4/9）: detail / gantt / progress / someday / update
  - [ ] finance（2/9）: budget-limits / plans/* / upload-receipt
  - [ ] timeline（3/8）: media 配信 / plan/*
  - [ ] personal（3/6）: clipboard / context-note
  - [ ] credential（2/3）: register（作成 = P1-5 暗号層に依存）
  - [ ] playbook（3/8）: runs / schedules/*
  - （schedule 3/3・reminder は完了）

### P2-B Gemini ツール（24/87 → 埋める）

- [ ] todo: 11 本（addSubtask/updateTodo/updateTaskProgress/editTodoTags/listTasksByTag/listTodoTags/getTaskDetail/getTaskUsageGuide/applyTaskPriorities/stopTodoRoutine/getRecentActionHistory）
- [ ] finance: ~16 本（getMonthlySummary/getCategoryBreakdown/budget limits/planned payments 一式/findSettlementCandidates 等）
- [ ] timeline: createDayPlanBlock/listDayPlan/deleteDayPlanBlock 等
- [ ] personal: clipboard(3)/notes(context-note)/conversation(2)
- [ ] credential: add/update/browserFillCredential（暗号層依存）
- [ ] browser 一式（searchWeb/fetchDynamicPage/takePageScreenshot/browserInteractive ~9）— 対応クレート無し
- [ ] chart（sendChart）
- [ ] briefing（configureBriefing/getBriefingConfig/runBriefingNow/configureReport）
- [ ] richContent（showRichContent・常時 on のコア）
- [ ] botAssistant（メンバー管理 + guild/personal ノート ~10）
- [ ] **MCP 動的ツール**（`McpProvider` は未実装。`yuuka-tools/src/lib.rs` で deferred）
- [ ] **capability ゲート適用**: 現状 `NativeProvider.list()` が全ツールを返し、bot 属性による絞り込みが未適用（`getFunctionModulesForCapabilities` 相当が無い）→ 全 bot に全ツール露出。

### P2-C 常駐サービス（6 実装 + 4 予約シーム + 1 欠落）

- [ ] report（日報/週報）— 予約シーム no-op（Gemini aux-gen + charts 依存）
- [ ] briefing（朝報/天気/RSS）— 予約シーム no-op（weather/RSS HTTP + SSRF ガード依存）
- [ ] backup（Google Drive）— 予約シーム no-op。**自動バックアップが走らない**（per-user Drive OAuth 依存）。実データを扱うなら要注意。
- [ ] playbook-schedule（マクロ自動実行）— 予約シーム no-op（Gemini processMessage 依存）
- [ ] synapse engine（認知想起）— **予約シームですらなく完全欠落**（Node は外部 Rust synapse daemon を spawn）。Phase H の daemon 吸収で対応。

### P2-D 設定キーの取り込み

- [ ] `GOOGLE_CLIENT_ID` / `GOOGLE_CLIENT_SECRET`（OAuth）
- [ ] `INVITE_CODES`（起動時シード）
- [ ] `ADMIN_DISCORD_IDS`（初期 admin bootstrap）
- [ ] `REMINDER_CRON` 等の cron スケジュール上書きキー（現状 Rust は自前スケジュール固定）

---

## P3 — 品質 / 運用衛生

- [ ] **P3-1 CI ゲートの新設**: build + clippy(-D warnings) + test + `cargo deny` + `gen-types --check`(drift) + `cargo fmt --check` を CI 化（現状ローカル手動のみ）。
- [ ] **P3-2 `cargo fmt` 差分の解消**（2026-07-09 実測で ~193 hunk・ほぼ全 crate に及ぶ）。**P3-1 の CI ゲート新設と同一コミットで一括正規化する**（部分 fmt は別種の不整合を生むため単発の workspace 全体 `cargo fmt --all` を推奨）。
- [ ] **P3-3 残レビュー指摘 M-7〜M-11（fail-closed）**:
  - [ ] M-7 priority 正規化 + float `2.0` 受理幅
  - [ ] M-8 finance amount 検証
  - [ ] M-9 reminder `trigger_at` 正規化（**ISO 入力で壊れる懸念** = add 時の日時正規化欠落）
  - [ ] M-10 credential 許可フィルタ（bot_credential_access）
  - [ ] M-11 persona 適用中の delete 拒否
- [x] **P3-4 README 冒頭の古い記述を修正** — 済（[README.md](README.md) の「実装はまだ開始していない」を Phase 0〜5 着地の現況＋remaining-work.md 参照に更新）。
- [x] **P3-5 `/api/me` の DB 再取得 + 404 分岐** — 済（`yuuka-web/src/routes.rs`：セッション解決後に `SELECT username, role FROM users WHERE discord_id` を read pool で再取得し、消失時 404 `{success:false,message:"ユーザーが見つかりません。"}`＝Node parity。role は DB 権威。テスト `me_returns_404_when_user_deleted_from_db` 追加・既存 200 テストは users 行を seed）。
- [ ] **P3-6 index.html への google-site-verification meta 注入**（deferred・`static_files.rs`）。
- [ ] **P3-7 Docker イメージのスリム化**（Node ランタイム / node_modules / dist/index.js / chromium は Rust 直起動では未使用。ハイブリッドで肥大。frontend ビルドのみ Node 段が必要）。
- [ ] **P3-8 LOW 群**（`reminders/delete` 撤去の是非・float priority 受理幅ドキュメント化・repo docstring stale 等）。

---

## 4. 参考：フェーズ対応（[PLAN.md](PLAN.md) §11）

| フェーズ | 内容 | 現在地 |
|---|---|---|
| Phase 0 | 契約凍結（core/db/types） | ✅ 完了 |
| Phase 1 | Web/認証/静的/ドメイン CRUD | 🔶 ほぼ完了（P3 の残指摘・P2-A の深掘り残） |
| Phase 2 | Gemini + tools + 全ドメインツール登録 | 🔶 FC ループ + 24 ツール（上位層・残ツール未） |
| Phase 3 | Discord（twilight） | 🔶 転送層のみ（live 未起動 = P1-3） |
| Phase 4 | 常駐サービス | 🔶 6 実装 / 4 予約シーム（配信橋渡し未 = P1-4） |
| Phase 5 | Dockerfile/nginx カットオーバー | 🔶 配管切替済（整合は P0-3） |
| Phase D/E/G | gemini 上位層 / discord live / services 本体 | ⬜ 主に P1〜P2 |
| Phase H | synapse/crawler の daemon 吸収 + JoinSet 全体監督 | ⬜ 未 |

---

## 5. 完了チェックリスト（経路別）

### 経路 A — strangler 並走カナリア（最短で限定検証）
- [x] P0-1 V17 コミット + 凍結（`61934e8`）
- [x] P0-2 cron 片側専任（`deploy/README.md`）
- [x] P0-3 nginx ポート整合（`deploy/nginx/yuuka.conf`）
- [x] P0-4 DB 事前シード（`deploy/README.md`）
- [ ] 共有 Redis セッション鍵/キー書式が Node と一致することを実 Redis で確認（**未検証・要確認**）
- [ ] 移行済み CRUD サブセットのみを Rust へ向け、それ以外は Node（fail-safe）
- → これで「非暗号フィールドの CRUD を Rust が捌く」限定 near-prod 検証が可能。

### 経路 B — 単独ほぼ本番（Node 撤去）
- [ ] P1-1〜P1-7 を全て解消
- [ ] P2-A/B/C/D を実用十分な水準まで
- [ ] P3-1 CI ゲート常時緑
- [ ] 実 Redis 稼働下の Cookie 検証ライブ確認（環境に Redis 必要）
- [ ] push / prod deploy / `YUUKA_RUST_CRON` 本番 ON（= Node cron 停止カットオーバー）は**ユーザー最終判断**

---

## 付録：主要ファイル早見

| 関心事 | ファイル |
|---|---|
| 起動配線（web のみ監督・discord/cron 未配線） | `crates/yuuka-supervisor/src/main.rs` |
| 予約シーム 4 no-op | `crates/yuuka-services/src/deferred.rs`, `.../lib.rs` |
| NullNotifier | `crates/yuuka-services/src/notifier.rs` |
| Discord 転送層（不活性） | `crates/yuuka-discord/src/{manager,message_flow,ports}.rs` |
| 未使用アダプタ | `crates/yuuka-supervisor/src/discord.rs` |
| config ローダ（暗号 env 未読） | `crates/yuuka-core/src/config.rs`, `.../secrets.rs` |
| 暗号 deferred | `crates/yuuka-credential/src/{lib,routes,tools}.rs` |
| V17 baseline（未コミット修正） | `crates/yuuka-db/migrations/V17__baseline.sql`, `.../src/schema.rs` |
| nginx strangler | `deploy/nginx/yuuka.conf` |
| Dockerfile（`YUUKA_RUST_CRON=1` 焼込・Rust CMD） | `Dockerfile` |
| Node パリティ基準 | `src/index.ts`（起動）, `src/server/`（ルート）, `src/functions/`（ツール）, `src/services/`（cron）, `src/bot.ts`（Discord）, `src/gemini.ts`（FC + 上位層）, `src/server/chatWebSocket.ts`（WS） |
