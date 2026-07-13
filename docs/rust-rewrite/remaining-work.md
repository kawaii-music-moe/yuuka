# Rust 移行 — 残作業ロードマップ（本番投入までの ToDo 全集）

- 最終更新: 2026-07-14（**実装/ドキュメント整合セッション**: `b77506d`（P2 ドメイン Web-API 21 エンドポイント）を全ドキュメントへ反映。実測で全ゲート再検証 — build ✅ / clippy -D ✅ / **test 386** ✅ / deny exit0 ✅ / git 作業ツリー clean）
- 対象ブランチ: `feature/rust-rewrite`（未 push・HEAD=`b77506d`）
- git HEAD: `b77506d`（P2 ドメイン Web-API 拡張）
- 前提資料: [review-2026-07-06-fix-policy.md](review-2026-07-06-fix-policy.md)（修正方針の唯一の基準）・[review-2026-07-09-batch4-6.md](review-2026-07-09-batch4-6.md)・[PLAN.md](PLAN.md) §11（移行ロードマップ）
- **本セッション（2026-07-09e）の成果**: **P1-3 Discord live 化**を実装 — twilight 転送層（既存・不活性）を**起動配線**した。(1) 注入ポートの**本番 DB 実装**を新設（`yuuka-orchestrator`）: `DbBotDirectory`（Node `botRepo`/`botAttributesRepo`/`userRepo` パリティ・Bot メタ/list/共有アクセス/トークン復号[`SystemCrypto`]/メンバー・許可ロール・登録判定）・`DbMembership`（申請 submit/decide=承認で `bot_members` 追加・共有 accept/revoke・公開ペルソナ import・全て writer actor 単一 Tx）・`InMemoryRateLimiter`（Node `botRateLimit` 固定窓 5/分・100/日・1000/ギルド日・`system_settings` 上書き）。(2) **汎用モード**（`ChatEngine::process_guild`/`process_bot_dm`）実装 — Bot 専用 Gemini キー（`getBotGenAI` パリティ・`BOT_DEFAULT_MODEL`）・ギルド/DM 分離コンテキスト（`[名前]:` プレフィックス・guild 30 件/DM 15 件）・Bot 単位ペルソナ + 共有/個人ノートの `buildGuildSystemInstruction` 移植・FC ループ。(3) **`main.rs` 配線** — `DiscordManager::new(ports + processor=ChatEngine)` → `prepare()` で共有 `Messenger` 生成、各 `TenantRunner` を `DiscordTenantService` で **Supervisor 監督下**（panic 隔離 + 指数バックオフ・恒久クローズは非再起動）へ。ゲートウェイ起動は **`YUUKA_RUST_DISCORD` env ゲート**（既定 off＝二重 gateway/二重応答の回避・`YUUKA_RUST_CRON` と同思想）。**P1-4**: cron の `NullNotifier` を `Messenger`（`impl services::Notifier`）へ差し替え＝リマインド等が実 Discord へ配信可能に。**P1-1 残**: 登録コード DM を `MessengerRegistrationDm`（合成ルートアダプタ）で `Messenger` 経由に配線＝`/api/register` が実際に DM を送る。機械ゲート全緑（build/release/clippy-D/deny/**test 303**・+15: ポート DB 実装 7・汎用モード 3・guild prompt 2・build_contents 回帰 3）。
- **本セッション（2026-07-10）の成果**: **P1-3 未コミット diff の parity レビュー**（7 次元並列 + 各指摘を敵対的検証 = confirmed 14）を通し、確定 6 件を修正した。**(H) build_contents 二重ユーザーターン** — persist-before-load で履歴末尾に既にある発言を `build_contents` が再追加していた（Node `buildContentsFromHistory` は空履歴のときだけ `message.text` を積む）。修正: 空履歴のみ lone user turn・非空は添付のみ末尾 user content へ合流。**(H) notifier の DM フォールバック欠落** — `DiscordMessenger::send_to_user` が Channel 解決失敗で即 `false`（cron 経路は deliver_final を通らないため fallback 不能＝リマインダー永久リトライ）。修正: Channel 解決不能時に DM へフォールバック（Node `sendToUser` notifier.ts:130-142）。**(M) owner DM の context floor** — DM が秘書 floor を流用していた。修正: `recent_bot_dm_context`（floor `context_floor:{botId}:dm:{userId}`・Node `getBotDmContext`）を新設し DM 分岐で使用。**(M) レート制限の日窓** — `:d` 固定キー + 25h TTL の転がり窓だった。修正: `todaySuffix`（ローカル暦日 `YYYYMMDD`）を日キーへ付与し暦日境界でリセット。**(L) describe_incoming の trim**（空白のみを添付プレースホルダへ）・**(L) レート上限設定の parseInt 寛容パース**（先頭数字のみ解釈）。**defer（doc 化済み）**: 利用申請/決定の owner・applicant DM 未送（明示的な縮退シーム＝`DbMembership` への messenger 注入待ち・§P2-A）／`has_gemini_key` は presence 判定で Node `getBotGenAI` の復号検証より弱い（低・むしろ復号エラーを表面化＝ops 良）／汎用モードの LLM エラー文言（rate-limit/server-error 別の ⚠️ ＝`guildErrorResult` 相当）は未分類（低・`TurnError` へ分類貫通が必要）。
- **本セッション（2026-07-10b）の成果**: **P1-3 の残 3 件（縮退シーム）を実装**。**(1) 能力ゲート（P2-B）**: `ToolContext` に `mode: TurnMode`（Secretary/GuildAssistant）追加・`Tool::exposure()`（既定メソッド＝現行 23 ツールは全て secretary 分類）・`ToolExposure::is_visible`（経路 × 能力）・`NativeProvider::list` で filter・engine が `bot_repo::parse_capabilities`（Node `parseCapabilities` パリティ＝null/空/非配列/失敗は秘書相当フル）で `ctx.capabilities` を注入。秘書経路は `caps.has("secretary")` で 23 ツール露出、汎用モードは secretary ツールを一切露出しない（Node `getGuildAssistantFunctionModules` は guild-assistant モジュールのみ・未移植）。**残**: ユーザー別 `enabledModules`（`bot_user_modules`/`bots.enabled_modules` の selectable 絞り込み・Node 第 2 次元）は未移植＝module 選択 UI 系（P2-A・native.rs にコメント明記）。**(2) member-request DM**: `SubmitOutcome`/`DecisionOutcome` に owner/applicant/bot_name/request_id を露出し、interaction ハンドラが既存 text-exact ヘルパ（`send_member_request_dm`/`send_member_decision_dm`）を DB 確定後に呼ぶ（`MemberDmSender` ポートを `InteractionDeps` へ注入・fire-and-forget）。submit→owner 受付 DM（承認/却下ボタン）・decide→applicant 結果 DM。**(3) 汎用モード LLM エラー文言分類**: `classify_gemini_error`（429=`RateLimited`・{500,502,503,504}=`ServerError`・他は None→generic）で秘書/汎用の別文言（秘書は「（トークン枯渇など）」「（503等）」付き）を `Ok(TurnReply::text)` で返す（履歴非保存・Node `guildErrorResult`/`processMessage` catch）。**parity レビュー**（4 次元 + 敵対的検証 confirmed 7・全て low/medium・high 無し）を通し 7 件対応（capabilities NULL 耐性・stale doc・comment 正確化・classify/label_or 回帰テスト追加等）。機械ゲート全緑（build/release/clippy-D/deny/**test 310**・+7）。
- **本セッション（2026-07-10c）の成果**: **フォールバック関連の横断精査**（Discord 配信/会話エンジン/インフラの 3 クラスタ・約 25 箇所を Node と突き合わせ）。**修正 2 件**: (H) `NO_KEY_MESSAGE`（⚠️ Gemini キー未設定）を Rust だけが `message_logs` に assistant 保存していた — Node `processMessage` の catch は `saveAssistant` を通らず返すだけ＝⚠️ 定型応答は履歴非保存が正（保存すると以後の文脈ウィンドウを警告文で汚染）。除去 + 非保存を回帰 assert で固定。(M) `/ws/chat` の添付上限が Rust ハードコード 20MB — config `DESKTOP_MAX_UPLOAD_MB`（Node `desktopMaxUploadMb`）を core `Config` に追加し `ws_routes` へ貫通、too_large 文言も Node の上限値入り文面に一致。**精査で反証**: settings 空白のみ値の扱い（両者 `" "` 採用＝一致）・context floor パース失敗→0（一致）・レート上限 parseInt（一致）・deliver_final の DM 再送は全文再送で二重配信なし（一致）・SPA fallback は API を食わない（一致）。**意図的 divergence として明文化**: resolve_client/send_owner_dm の readiness 非ゲート（twilight REST は gateway 非依存＝Node が拒否するケースでも配信できる・改善側）／`or_default` の安全側 deny（Node は DB 例外→エラー返信・WAL 読取は BUSY 稀のため頑健性優先）／セッション 502（M-1 レビュー済み契約・Node は 401→再ログイン自己回復）／config fail-fast（Node は NaN 黙殺・Rust は起動拒否＝安全側）。**既知の未移植**: Node `fallbackText` の browser 分岐（browser ツール未移植のため到達不能・P2-B と同時に移植）。機械ゲート全緑（**test 310**・clippy-D/deny exit 0）。
- **本セッション（2026-07-10d）の成果**: **ペルソナ入りエラー応答**（ユーザー要望による Node からの意図的拡張）。ターン失敗を内部で `TurnFailure::{Llm, NonLlm}` に分類し、**非 LLM エラー（DB 障害等＝LLM は生きている）では固定の GENERIC_ERROR をやめ、LLM にペルソナ口調のエラー報告（1〜3 文・技術用語なし）を生成させて返す**。秘書経路＝ユーザーキー + アクティブペルソナ（無ければ `DEFAULT_PERSONA`）、汎用モード＝Bot 専用キー + Bot ペルソナ。生成はツール無し単発・履歴非保存（⚠️ 定型と同じ扱い）。**フォールバック連鎖**: ペルソナ生成→（生成不能: キー無し/復号不能/生成失敗）→従来の固定文。**LLM 関連エラー（レート/サーバー/鍵復号不能/backend 構築失敗/上流未分類）は従来どおり固定文**（LLM を呼べない/信頼できないため）。回帰テスト 3 件（秘書ペルソナ応答・キー無しフォールバック・汎用モード Bot ペルソナ応答）。機械ゲート全緑（**test 313**・clippy-D/deny exit 0）。
- **本セッション（2026-07-12）の成果**: **Web/LLM-API 面の本番投入準備**。cargo 全ゲート緑（build/clippy -D/deny exit0/**test 330**・+17）+ 実バイナリのライブ疎通（N2/B5/B6/CSRF）を確認したうえで、2026-07-12 監査の確定ブロッカー4件と、Web API 面の**敵対的セキュリティレビュー**（5次元×各指摘を独立検証＝29エージェント/23指摘/確定15・判定=**canary go**）の確定 low 指摘を修正した。**確定ブロッカー4件**: (B5) `/ws/chat` の CSWSH 退行＝汎用 `AuthenticatedUser`（Cookie 優先）が WS upgrade の ambient Cookie を受理していた → **Bearer 専用 `BearerUser` extractor** を新設し Bearer 限定化（Node `getBearerUser` パリティ・構造的 CSWSH 不能・回帰テスト追加）。(B6) 未登録 `/api/*` GET が SPA index.html(200) で握り潰し → `static_files.rs` に **全メソッド `/api/*`→JSON 404**（`{success:false,message:"APIエンドポイントが見つかりません。"}`・Node `server.ts:339`）。(B4) reminder `trigger_at` の字句比較バグ＝`T` 区切り ISO を生保存 → cron の空白区切り `datetime('now','localtime')` と字句比較で当日発火が翌日まで遅延 → `datetime::to_db_datetime`（Node `toDbDateTime` パリティ・`Z`/オフセット/小数秒/日付のみ対応）を新設し repo 境界正規化 + tool/**web route 双方で入力検証**。(N2) 暗号鍵未設定でも起動継続（Rust 固有退行）→ main.rs に `require_encryption_secret`（未設定/32 文字未満は起動拒否・Node `index.ts` §6.2）。**セキュリティレビュー確定 low 指摘**: (#1) CSRF 信頼アンカーがクライアント供給 `Host` だった → **`config.base_url` 由来の `allowed_host` allowlist** へ（Node `isAllowedHost`・Host 注入耐性・`WebConfig` へ貫通）。(#2) `Sec-Fetch-Site` が `cross-site` 以外を無検証許可 → `cross-site` のみ即拒否・他値は Origin allowlist へ委譲（`same-site` 兄弟サブドメイン対策）。(#3) 空 `Bearer ` が CSRF 免除される潜在バイパス → `has_bearer` を非空トークン必須に。(#4) `ScopedJson` の serde エラー文言（型/フィールド名）露出 → 固定文言化。**確定したが本セッション未対応（低・own-user/fail-closed・doc 済）**: (#5) reminder 過去日時の拒否/繰り返し自動前進の未移植（cron next は上位 crate 依存・P3-3 M-9 の残）／(#7) 不完全添付でフレーム全体を internal error（fail-closed の意図的厳格）。レビューが**反証/refuted した非問題**（対応不要）: WS Bearer 専用化の完全性・botId IDOR 防止・WS エラー/秘密の非漏洩・接続タスク panic 隔離・CSP/セキュリティヘッダ・error→status の内部 Display 秘匿・9 ドメインの認可/multi-tenant スコープ厳密性。**判定: 経路A（strangler カナリア・単一 vhost HTTPS）= GO**（critical/high/medium ゼロ）。経路B no-go は不変（routes 22%/tools 27%）。
- **本セッション（2026-07-13）の成果**: **P2 ドメイン Web-API 拡張**（`b77506d`・5 ドメインクレートに Node parity の新規 HTTP エンドポイント + DB 層 + DTO + テストを 21 エンドポイント追加。**test 330→386**・全緑 / clippy -D 緑 / build 緑・新規依存なし＝deny 緑不変）。全リポメソッドは `WHERE user_id=? AND bot_id=?` でスコープ束縛・全クエリ `params!` でパラメータ化・DTO は `user_id`/`bot_id` 非露出のクリーンビュー。**(finance)** 予算上限（`/api/expenses/budget-limits` GET/POST・`/delete`）+ 支払い予定の消込（`/plans` GET/`/add`/`/pay`/`/delete`）。`settle_plan` は Expense 記録・予定 settled・紐付き ToDo 自動 done を単一 Tx で実行。**(personal)** コンテキストノート（`/api/context-note` GET/POST・upsert）+ クリップボード（`/api/clipboard` GET・`/delete`・期限切れ自動除外）。**(playbook)** スケジュール（`/api/playbooks/schedules` GET/`/save`/`/toggle`/`/delete`）+ 実行履歴（`/runs` GET）。**(timeline)** 計画ブロック CRUD（`/api/timeline/plan` add/`/update`/`/delete`・day に blocks 同梱）。`UpdatePlanBlock` は `Option<Option<T>>` で Node の `"key" in obj` 意味論を再現。**(todo)** gantt/someday/detail GET + update/progress POST。子孫は再帰 CTE でスコープ束縛収集。`PriorityUpdate` は 3 値（据置/クリア/設定）で Node parity。**意図的な縮退シーム（本セッションで新設・下記 P2-A/C に反映済み）**: (1) **playbook スケジューラは未起動** — schedules/runs は永続化されるが、cron 式検証（croner がクレート依存に無い）と実行エンジン（`executePlaybook`）は deferred。**設定を保存してもマクロは自動実行されない**（reminder と同方針・別タスク）。(2) **finance** — receipt OCR（`upload-receipt`）と月次集計（total/breakdown/trend）は deferred。(3) **timeline** — media 保存/配信・`type=expense`/`type=task_done` の cross-domain 副作用は deferred。(4) **personal** — クリップボードの追加（`addEntry`）と TTL 一括削除 cron（`deleteExpired`）は deferred（誕生日リマインド cron は移植済み）。
- **前セッション（2026-07-09d）の成果**: **P1-1 資格情報認証発行**（`SessionStore` 発行+検証・7 ルート・bcrypt `$2b$` cost12・招待/監査/レート制限）+ **P1-2 会話**（`yuuka-orchestrator`・`/ws/chat`）。
- **前々セッション（2026-07-09c）の成果**: P0-1〜4・P1-5（暗号層）・P1-6/7・P1-4 アダプタ・P3-4/5。

---

## 0. 現状サマリ（判定）

**「ほぼ本番（Node 完全置き換え）」としてはまだ使えない。** 基盤設計は堅牢でユニットは全緑だが、ユーザーが実際に触れる経路（ログイン・会話・Discord・通知・秘密情報復号）が揃っていない。

| 面 | 実測 |
|---|---|
| `cargo build --release --workspace` | ✅ exit 0 |
| `cargo clippy --workspace --all-targets` | ✅ exit 0（ts-rs 良性 warning のみ） |
| `cargo test --workspace` | ✅ **386 passed / 0 failed**（2026-07-13 の P2 ドメイン Web-API で +56・2026-07-14 実測で再確認） |
| `cargo deny check` | ✅ **exit 0**（新規依存なし＝既存クレートのみで実装） |
| 保存時暗号層（Argon2id/AES-256-GCM） | ✅ **実装済**（P1-5・Node ゴールデンベクタでバイト単位パリティ・鍵ローテ起動時配線） |
| 認証発行（login/setup/logout/register/users） | ✅ **実装済**（P1-1・セッション発行 + bcrypt + 招待 + 監査 + レート制限。**登録 DM は P1-3 で開通**・OAuth は残） |
| HTTP ルート被覆 | 57 / 152 パス ≒ **37%**（認証 7 + `/api/me` + 9 ドメイン CRUD。2026-07-13 の P2 で todo/finance/timeline/personal/playbook を拡張） |
| Gemini ツール被覆 | 24 / 87 native ≒ **28%**（動的 MCP 0） |
| チャットオーケストレーション（秘書ターン） | ✅ **実装済**（P1-2・`yuuka-orchestrator`・実 TurnProcessor・統合テスト緑） |
| 汎用モード（guild/owner DM ターン） | ✅ **実装済**（P1-3・`process_guild`/`process_bot_dm`・Bot 専用キー + ギルド/DM 分離文脈・統合テスト緑。能力ゲート=全ツール露出は P2-B） |
| WebSocket `/ws/chat`（デスクトップ会話） | ✅ **実装済**（P1-2・Bearer 認証 + ready/status/done・live 統合テスト緑。interaction/deferred は縮退） |
| Discord live（gateway 起動 + Supervisor 監督） | ✅ **配線済**（P1-3・DB ポート + `DiscordManager.prepare` + `DiscordTenantService`。既定 off・`YUUKA_RUST_DISCORD=1` で起動＝Node bot 停止後） |
| Gemini FC ループ本体 | 1:1 移植 ≒ 95% 完成（秘書 + 汎用モードの両経路から到達可能・P1-2/P1-3） |
| 通知配信ブリッジ | ✅ **配線済**（P1-4・main の `NullNotifier`→`Messenger` 差し替え。デフォルト Bot 起動でリマインド等が実配信） |
| 登録 DM ブリッジ | ✅ **配線済**（P1-3・`MessengerRegistrationDm`＝`RegistrationDm`↔`Messenger` の合成アダプタ。`/api/register` が実 DM 送信） |

**進め方の2経路:**
- **経路 A（strangler 並走カナリア）** — Node が認証/会話/Discord/暗号を担い、Rust は移行済み CRUD の一部だけを共有 Redis セッション前提で配信。§7-A の前提を満たせば数日規模で到達可能。**（P1-1 により Rust 単独でのセッション発行も可能になった＝Rust だけでログイン→CRUD が回る。）**
- **経路 B（単独ほぼ本番）** — Rust だけで完結。P1-1〜P1-7 は全て着地（残は P1-1 の OAuth のみ＝P2-A へ移送）。残は主に P2（機能パリティ）+ P3（CI/fmt/残指摘）。**Discord ライブ確認**（実トークンでのメンション/DM 応答・二重処理回避のカットオーバー）は要実機検証。

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

- [x] **P1-3 Discord を実起動し、Supervisor 配下へ配線する** — **着地（2026-07-09e）+ parity レビュー済み（2026-07-10）**
  - 済: DB ポート実装（`DbBotDirectory`/`DbMembership`/`InMemoryRateLimiter`）+ 汎用モード（`process_guild`/`process_bot_dm`）+ `main.rs` 配線（`DiscordManager.prepare` → `DiscordTenantService` を Supervisor 監督下・`YUUKA_RUST_DISCORD` env ゲート既定 off）。parity レビュー確定 6 件を修正（本ファイル冒頭「2026-07-10 の成果」参照）。
  - 済（2026-07-10b・縮退シーム 3 件）:
    - [x] **利用申請/決定の Discord DM**（owner へ申請通知・applicant へ承認/却下通知）。interaction ハンドラが outcome 露出の id で既存ヘルパを呼ぶ（`MemberDmSender` 注入・fire-and-forget）。
    - [x] **汎用モード LLM エラーの文言分類**（rate-limit/server-error 別の ⚠️・秘書/汎用で別文言）。`classify_gemini_error` で `Ok(TurnReply::text)` を返す（履歴非保存）。
    - [x] **能力ゲート適用**（秘書 × 汎用モードの経路 × 能力集合）。残: ユーザー別 `enabledModules`（P2-A・下記 P2-B 参照）。
  - 残（実機）: 実トークンでのメンション/DM 応答確認・Node bot 停止のカットオーバー（二重処理回避）。

- [x] **P1-4 通知配信の橋渡し（Messenger → Notifier）を実装する** — **配線済み（P1-3 と一体・2026-07-09e）+ DM フォールバック修正済み（2026-07-10）**
  - 済: `crates/yuuka-discord/src/notify_bridge.rs`＝`impl yuuka_services::Notifier for DiscordMessenger`（`NotifyTarget`↔`DeliverTarget` 変換＝Default→DM・Channel 透過、空本文 false、`TurnReply::text` 化して `send_to_user` へ委譲）。孤児規則により discord 側に実装（services→discord 逆依存なし＝非循環）。target 写像を単体テストで凍結。`main.rs` の `NullNotifier`→`Arc<DiscordMessenger>` 差し替え済み。
  - 済（2026-07-10 parity 修正）: `send_to_user` の Channel 解決失敗時に DM フォールバック（Node `sendToUser`）。これが無いとチャンネルを閲覧不可のリマインダーが送信されず reminder が永久リトライ状態になっていた。
  - 完了条件: 期限到来リマインドが実 Discord チャンネルへ届く（実機確認は P1-3 のカットオーバー時）。

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

### P2-A Web ルート（57/152 → 埋める）

> 2026-07-13（`b77506d`）で 9 ドメイン CRUD のうち todo/finance/timeline/personal/playbook を拡張済み（下記ドメイン別を参照）。以下の管理・設定・Bot・MCP 系は未着手。

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
- [~] ドメイン別の残ルート（2026-07-13 `b77506d` で todo/finance/timeline/personal/playbook を拡張）:
  - [x] todo（9/9）: detail / gantt / progress / someday / update を追加（子孫は再帰 CTE でスコープ収集・`PriorityUpdate` 3 値 parity）
  - [~] finance（7/9）: budget-limits / plans/* を追加。**残**: upload-receipt（receipt OCR）/ 月次集計 total・breakdown・trend（deferred）
  - [~] timeline（7/8）: plan/*（add/update/delete）を追加。**残**: media 保存/配信・`type=expense`/`type=task_done` の cross-domain 副作用（deferred）
  - [~] personal（5/6）: clipboard（GET/delete）/ context-note を追加。**残**: クリップボード追加（addEntry）・TTL 一括削除 cron（deleteExpired）（deferred）
  - [ ] credential（2/3）: register（作成 = P1-5 暗号層に依存）
  - [~] playbook（7/8）: runs / schedules/* を追加。**残**: cron 式検証 + 実行エンジン（`executePlaybook`・croner 依存）＝**設定は永続化されるがスケジューラ未起動**（P2-C playbook-schedule と一体・下記参照）
  - （schedule 3/3・reminder は完了）

### P2-B Gemini ツール（24/87 → 埋める）

> **注意**: 2026-07-13（`b77506d`）は **Web ルート（P2-A）のみ**を拡張し、Gemini ツール（LLM が function-calling で呼ぶ側）は未追加。よって finance budget-limits/timeline plan/todo detail 等は **ダッシュボードからは操作できるが LLM からはまだ呼べない**。以下は据え置き（24/87）。

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
- [~] **capability ゲート適用**: 済（2026-07-10b）＝経路（秘書/汎用モード）× 能力集合で `NativeProvider.list()` を絞り込み（`ToolExposure`/`ctx.mode`+`ctx.capabilities`・Node `parseCapabilities`+`getFunctionModulesForCapabilities`/`getGuildAssistantFunctionModules` パリティ）。**残**: ユーザー別 `enabledModules`（`resolveEnabledModulesForUser`＝`bot_user_modules`/`bots.enabled_modules` の selectable モジュール絞り込み・Node の第 2 次元）が未移植＝module 選択 UI 設定に連動（本項の完了はこの実装で）。

### P2-C 常駐サービス（6 実装 + 4 予約シーム + 1 欠落）

- [ ] report（日報/週報）— 予約シーム no-op（Gemini aux-gen + charts 依存）
- [ ] briefing（朝報/天気/RSS）— 予約シーム no-op（weather/RSS HTTP + SSRF ガード依存）
- [ ] backup（Google Drive）— 予約シーム no-op。**自動バックアップが走らない**（per-user Drive OAuth 依存）。実データを扱うなら要注意。
- [ ] playbook-schedule（マクロ自動実行）— 予約シーム no-op（Gemini processMessage 依存）。**2026-07-13（`b77506d`）でスケジュール/実行履歴の CRUD API + 永続化は実装したが、cron 式検証（croner 依存）と実行エンジン（`executePlaybook`）は未移植＝設定は保存されるがマクロは自動実行されない**（reminder と同方針の縮退シーム・要注意）。
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
  - [x] M-9 reminder `trigger_at` 正規化 — 済（2026-07-12・B4）。`datetime::to_db_datetime` を repo 境界 + tool/web route で適用。**残**: 過去日時の拒否・繰り返しの次回自動前進は未移植（低・own-user・cron next は上位 crate 依存）
  - [ ] M-10 credential 許可フィルタ（bot_credential_access）
  - [ ] M-11 persona 適用中の delete 拒否
- [x] **P3-4 README 冒頭の古い記述を修正** — 済（[README.md](README.md) の「実装はまだ開始していない」を Phase 0〜5 着地の現況＋remaining-work.md 参照に更新）。
- [x] **P3-5 `/api/me` の DB 再取得 + 404 分岐** — 済（`yuuka-web/src/routes.rs`：セッション解決後に `SELECT username, role FROM users WHERE discord_id` を read pool で再取得し、消失時 404 `{success:false,message:"ユーザーが見つかりません。"}`＝Node parity。role は DB 権威。テスト `me_returns_404_when_user_deleted_from_db` 追加・既存 200 テストは users 行を seed）。
- [ ] **P3-6 index.html への google-site-verification meta 注入**（deferred・`static_files.rs`）。
- [x] **P3-7 Docker イメージのスリム化** — 済（2026-07-12）。`[profile.release]` に strip+thin-LTO（バイナリ 311MB→**24MB**）。`Dockerfile` を全面刷新し runtime=debian-slim に **yuuka バイナリ + `dist/public` のみ**同梱（Node/node_modules/chromium/フォント/dist/index.js/crawler/synapse/desktop.exe/docs 非同梱）。ビルド段は rust(yuuka のみ)+frontend(vite のみ)。**イメージ 59MB**・docker build/run で疎通確認済み。frontend ビルドのみ Node 段（vite）が必要なのは不変。
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
