# 第13〜14部 — リスク一覧と対策・オープンな決定事項

> 出典整合: [`00-decisions.md`](../00-decisions.md)（特に末尾「未解決の決定事項」）／[`design-checkpoint.md`](../design-checkpoint.md) 各領域「敵対的検証」節（A〜D）／[`verification/`](../verification/) 各レポート末尾の確信度・落とし穴。
> 本部の全リスクは一次ソース照合済み（crates.io / docs.rs / 公式ドキュメント）。確率・影響は移行文脈での相対評価。

---

## 13. リスク一覧と対策

凡例 — **影響**: 致命/高/中/低（致命=データ喪失・常時稼働破綻）。**確率**: 高/中/低（対策未実施時）。

### 13.1 データ層・並行性（最優先クラスタ）

| # | リスク | 影響 | 確率 | 対策 | 参照 |
|---|---|---|---|---|---|
| R-1 | **SQLite 二重 writer による即-BUSY**。Node(better-sqlite3)と Rust(rusqlite)が同一 `yuuka.db` を同時 open する移行期に、両側 DEFERRED Tx が write へアップグレード→ busy_timeout を**無視して即 `SQLITE_BUSY`**（busy_timeout では原理的に解決不能）。`db.transaction()` 多用箇所（personaRepo/userRepo/todoRepo/plannedPaymentRepo）が該当。 | 致命 | 高 | **単一 writer 集約を最優先**: 移行期は「Node が全書き込み・Rust は read-only、カットオーバー時に一度だけ writer を Rust へ移譲」。両writerやむなき場合のみ**全書き込みTxを `BEGIN IMMEDIATE`**（DEFERRED禁止）+ 両側 busy_timeout 明示 + アプリ層 `SQLITE_BUSY` backon リトライを**全て**課す。監視で `SQLITE_BUSY` 発生数を追い、急増=競合の兆候として検知。 | [dual-sqlite-hazard](../verification/rpt-dual-sqlite-hazard.md) §1.4/§2（確信度:高）／[00-decisions](../00-decisions.md) L103／checkpoint D-1, B-1 |
| R-2 | **アプリ内の並行 writer 競合**。単一プロセス内でも rusqlite 複数コネクションから同時書き込みを投げ busy→backoff を繰り返すと、公平性が無く**ライブロック/レイテンシ悪化**。マルチスレッド化（絶対制約3）が逆効果化。 | 高 | 中 | **単一 writer actor**（全書き込みを1タスク/1コネクションに直列化）+ **read pool**（deadpool-sqlite 0.13 / r2d2_sqlite 0.34）。同期呼び出しは `spawn_blocking`。PRAGMA 明示: `WAL`/`busy_timeout=5000`/`foreign_keys=ON`/`synchronous=NORMAL`。アプリ層 backoff は timeout 超過後の最終手段に限定。 | [db-sqlx-vs-rusqlite](../verification/rpt-db-sqlx-vs-rusqlite.md)（split-pool footgun、確信度:高）／[00-decisions](../00-decisions.md) L68-69／checkpoint B-1 |
| R-3 | **長寿命 reader が WAL checkpoint を阻害**。Rust 側の未 finalize statement が checkpoint をブロックし `-wal` が肥大化。 | 中 | 中 | Rust 側は statement を確実に `finalize`/`reset`。read pool のコネクション寿命を短く保つ。 | [dual-sqlite-hazard](../verification/rpt-dual-sqlite-hazard.md)／[00-decisions](../00-decisions.md) L103 |
| R-4 | **refinery baseline の既存DBへの適用ミス**。現行 v17 スキーマを V1 baseline に凍結する際、非冪等な baseline を既存DBに流すと二重生成/エラー。checkpoint は refinery を「動的 `PRAGMA table_info` 内省と相性が悪い」と指摘（→ 00-decisions は refinery 採用・冪等 baseline で解決する立場）。 | 高 | 中 | baseline(V1) を全て **`CREATE TABLE IF NOT EXISTS` の冪等 DDL** で書く（既存DB=無害・新規DB=生成）。**現行の SCHEMA_VERSION 不一致 DROP 再作成（データ喪失）は完全撤廃**。migration 所有権を**単一プロセスに一元化**（移行期の二重実行防止）。破壊的再構築が要る場合は `_new` テーブルへコピー→リネームの保持型のみ。 | [migrations](../verification/rpt-migrations-sqlx-refinery.md)／[00-decisions](../00-decisions.md) L71／checkpoint L180, L2251-2256 |

### 13.2 依存クレートの破壊的変更・保守リスク

| # | リスク | 影響 | 確率 | 対策 | 参照 |
|---|---|---|---|---|---|
| R-5 | **axum/tower-http の 0.x 破壊的変更**。axum は 0.x のためマイナー更新=破壊的変更前提。tower-http 0.7.0（2026-06-15）は新しく axum 0.8.9 が `^0.6.8` を pin、0.7 の axum 0.8 互換は未検証。 | 高 | 中 | **バージョン固定運用**（マイナー=メジャー扱い）。**tower-http は 0.6.x 固定**（型互換確認まで 0.6 系）。hyper は 1.x 安定で問題なし＝実務リスクの主因は axum/tower-http の 0.x semver。更新は互換確認とテスト後にのみ。 | [axum-web-runtime](../verification/rpt-axum-web-runtime.md) L37/L84/L153（確信度:高、0.7互換のみ中）／[00-decisions](../00-decisions.md) L60 |
| R-6 | **Gemini `generateContent` が将来 legacy 化・非推奨リスク**。2026-06 に Interactions API が GA 化し「正面玄関」へ昇格、generateContent は公式に "legacy" 明記（ただし fully supported 継続）。新 frontier / 長時間エージェント機能は Interactions 側のみに載る。 | 中 | 中 | **薄ラッパで API 層を抽象化**（reqwest+serde+thiserror 自前）。表面積を小さく保ち、将来の Interactions 移行を局所化。**camelCase struct に厳密固定**（Interactions の snake_case/`function_result`/`call_id` を混入させない）。preview モデル名を避け GA 名 `gemini-3.1-flash-lite` 固定。 | [gemini-design-verify](../verification/rpt-gemini-design-verify.md) L11/L60/L147（確信度:高）／[00-decisions](../00-decisions.md) L79/L81 |
| R-7 | **公式 Rust SDK 不在（Gemini）による自前保守負担**。`google-generative-ai-rs` はアーカイブ済&FC未実装で不適、`gemini-rust 1.7.1` は機能豊富だがエラー方針・API面固定が未検証（予備）。 | 中 | 中 | **自前 reqwest 0.13 薄ラッパを第一推奨**。429/`RetryInfo(retryDelay)` を thiserror variant で完全掌握（現行 rate-limit バックオフを1:1移植）。SDK 依存より表面積が小さく厳格エラー方針を貫ける。`gemini-rust` は採用時のみ実コードでエラー型・API面を要確認。 | [gemini-rest-crates](../verification/rpt-gemini-rest-crates.md)／[gemini-design-verify](../verification/rpt-gemini-design-verify.md) L120-121（確信度:中〜高）／[00-decisions](../00-decisions.md) L78 |
| R-8 | **backoff クレートの RUSTSEC**。`backoff` は RUSTSEC-2025-0012 で非メンテ宣言＝採用禁止（`cargo audit` が警告）。 | 中 | 低 | **backon 1.6.0 採用**（`ExponentialBuilder::default().with_jitter()` を**明示**、ジッタ必須）。`cargo deny check advisories` を CI に組み込み。 | [resilience](../verification/rpt-resilience-tokio-backon-recloser.md) L59-63（確信度:高）／[00-decisions](../00-decisions.md) L55 |
| R-9 | **サーキットブレーカ生態系の薄さ**。recloser 1.4.0 は tokio 明示保証がなく、他クレートは未成熟（failsafe 2年停滞、circuit_breaker 168行）。 | 中 | 中 | recloser 採用時は tokio 上で軽く PoC 検証。要件がシンプル（失敗率閾値+openタイマ）なら **`AtomicU*` の自前状態機械も対等な選択肢**（コア数十〜百数十行）。自前ブレーカ + backon の組合せが堅い。 | [resilience](../verification/rpt-resilience-tokio-backon-recloser.md) L83-87（確信度:中〜高）／[00-decisions](../00-decisions.md) L56 |

### 13.3 プラグイン・拡張性・型連携

| # | リスク | 影響 | 確率 | 対策 | 参照 |
|---|---|---|---|---|---|
| R-10 | **非信頼プラグインのサンドボックス境界崩壊**。動的 `.so`（libloading/abi_stable）はサンドボックス無し＝ホスト完全侵害、panic-across-FFI が UB で supervisor が隔離しきれない。 | 致命 | 低 | **Extism 1.30.0（wasmtime 上）で非信頼プラグインを実行**。`Manifest` の `allowed_hosts`/`allowed_paths`/memory/timeout で **deny-by-default** 能力付与。wasmtime の trap を `Result`（`PluginError::Trap`）へ変換し制約1と親和。**動的 `.so` は非信頼コードに不採用**を明記。 | [plugins-wasm-extism](../verification/rpt-plugins-wasm-extism.md) L35/L61/L118（確信度:高）／[00-decisions](../00-decisions.md) L91-92／checkpoint D-3 |
| R-11 | **cargo-component 停滞リスク**。cargo-component 0.21.1（2025-03、~15ヶ月更新なし）。Extism の PDK crate cadence も slowish、Manifest フィールド名は SDK 版で要確認（中確信度）。 | 中 | 低 | Extism は独自 ABI で cargo-component 非依存。標準志向なら 生 wasmtime 46 + Component Model/WASI 0.2（`wasm32-wasip2` + wit-bindgen）が代替。Manifest フィールド名は docs.rs `Manifest` で採用時確認。 | [plugins-wasm-extism](../verification/rpt-plugins-wasm-extism.md) L48/L71（確信度:高、Manifest詳細のみ中） |
| R-12 | **型生成ドリフト**。Rust DTO と commit 済み TS（現行 `types.ts` 655行の二重管理）が乖離し、フロント⇄バックの型不整合。 | 高 | 中 | **CI git diff ゲート**: 生成（`cargo run --bin gen-types` / xtask）→`git diff --exit-code`。Rust DTO と commit 済み TS の乖離をブロックし現行「コメント頼み」を機械保証へ。build.rs でなく **xtask パターン**（副作用で drift 検査と競合しない）。 | [typegen-tsrs-utoipa](../verification/rpt-typegen-tsrs-utoipa.md)／[00-decisions](../00-decisions.md) L98／checkpoint L1032, L3489 |
| R-13 | **成功パス機密漏洩**。エラーの `public_message` allowlist は成功レスポンスの機密列 strip を代替しない。現行 zod allowlist（`discord_token_encrypted` 等を落とす）を型で置換しないと漏洩。 | 高 | 中 | **専用 DTO struct で構造的保証**: 機密列を DTO の**フィールドに存在させない**→生成 TS にも現れず漏洩は型的に不可能。DB row 構造体を直接 `Json` で返すことを clippy `disallowed-types` またはモジュール境界で禁止。in-memory 機密は `secrecy`。 | [00-decisions](../00-decisions.md) L97／checkpoint B-2, L2055 |

### 13.4 常時稼働・自己復帰（絶対制約2）

| # | リスク | 影響 | 確率 | 対策 | 参照 |
|---|---|---|---|---|---|
| R-14 | **移行中セッション/インメモリフォールバックの他系不可視**。Redis ダウン→インメモリセッションはプロセスローカルで、移行期は Node/Rust 間で他系に不可視＝**断続ログアウト**。 | 中 | 中 | フォールバック発動を**メトリクス/アラート化**（Redis 断は SPOF、即検知）。共有 Redis 復旧を優先経路とし、インメモリ縮退は短時間限定。移行期はセッション検証を「トークンの sha256 ハッシュ方式・Redis キー書式 `session:{sha256(token)}`・シリアライズ・TTL の完全一致」で両側成立させる（署名鍵共有は不要）。 | [nginx-session-strangler](../verification/rpt-nginx-session-strangler.md) L119/L249（確信度:高）／[00-decisions](../00-decisions.md) L57/L102 |
| R-15 | **twilight caller駆動 poll loop の supervise ミス**。twilight `Shard` は再接続/resume を内蔵するが caller 駆動 poll loop のため、supervisor で正しく回さないと**再接続しない**。N個の動的増減ボットで1ボット障害の隔離（他ボットは生存）が型に無い。 | 高 | 中 | supervisor を「名前付き単一タスク」から**per-bot supervisor tree（動的子タスク集合）**へ拡張。`DiscordError` に bot_id を載せ、1ボットの gateway 切断は**そのボットのみ**テナント別バックオフ再接続。各 Bot=1アクター（1タスク+コマンド mpsc）で二重 Client・トークン差替レースを型で消す。 | [discord-serenity-twilight](../verification/rpt-discord-serenity-twilight.md)（確信度:高）／[00-decisions](../00-decisions.md) L74／checkpoint B-4, L3728 |
| R-16 | **supervisor が `Permanent` を無限リトライ**。恒久障害（設定ミス等）をタスクが返し続けると 500ms→30s で永久リトライ＝**自己復帰ではなく自己ループ**。CPU スピン・ログ汚染。 | 高 | 中 | `Retryability` を supervisor 動作へ**別扱い**: `Transient`=即バックオフ再起動、`Permanent`=バックオフ上限到達で停止 or degraded 状態へ落とし管理UI通知。N回連続 Permanent で当該サービスを circuit open。 | checkpoint A-3（確信度:高）／[resilience](../verification/rpt-resilience-tokio-backon-recloser.md) L37-39 |
| R-17 | **稼働中 Fatal でプロセス即死**。`CryptoError::Fatal` を起動時と稼働時（リクエスト毎の復号）で共用すると、**1ユーザーの1トークン復号失敗でプロセス全体が落ちる**＝制約2の真逆。`panic=abort` にすると全 panic 隔離が無効化。 | 致命 | 低 | `CryptoError` を**起動時鍵検証用**（`Fatal` 可）と**稼働時復号用**（常に `Permanent`＝当該ボット停止/管理UI通知）に**型で分割**。`panic="unwind"` を厳守（`abort` 禁止）。致命的=起動時 config/secret 不備のみ。 | checkpoint A-4（確信度:中）／[resilience](../verification/rpt-resilience-tokio-backon-recloser.md) L23-28／[00-decisions](../00-decisions.md) L52/L57 |

### 13.5 開発プロセス・ツールチェーン

| # | リスク | 影響 | 確率 | 対策 | 参照 |
|---|---|---|---|---|---|
| R-18 | **厳格 lint/clippy deny がテスト・doctest・数値カーネルで誤発火**。`unwrap_used`/`expect_used`/`panic`/`indexing_slicing` deny がテストコードで誤発火。`indexing_slicing` deny は synapse 数値カーネル（`vec[i]`/`slice[a..b]` 多用）と衝突し大量 `#[allow]` を生む（allow 上限と正面衝突・制約3の高速性にも反する）。 | 中 | 高 | `clippy.toml` で `allow-unwrap-in-tests=true`/`allow-expect-in-tests=true`/`allow-indexing-slicing-in-tests=true`。**restriction グループ一括禁止は厳禁**（`blanket_clippy_restriction_lints` 警告）＝8個を**個別に** deny。`indexing_slicing` は**層別ポリシー**: 信頼境界クレート（HTTP/認可/DB）のみ deny、数値カーネル（synapse/embedder）はクレート単位で `allow`。allow カウントはクレート別上限 or 新規追加禁止（ベースライン比較）。 | [clippy-cargodeny](../verification/rpt-clippy-cargodeny.md) L29/L34/L64（確信度:高）／checkpoint A-2 |
| R-19 | **cargo-deny の古い `name=` 記法混入**。古い記事の `{ name="anyhow", version="..." }` 形式は非推奨、コピペで deprecation 警告/将来エラー。 | 低 | 中 | `deny.toml` の `[bans].deny` は**現行 `crate=` フィールド**を使用（`crate="anyhow"` / `crate="anyhow@<1"`）。`cargo deny check bans` はネットワーク不要＝高速ゲート、`advisories` はネットワーク要。CI: `cargo clippy --all-targets --all-features -- -D warnings` + `cargo deny check`。 | [clippy-cargodeny](../verification/rpt-clippy-cargodeny.md) L94/L118/L121（確信度:高）／[00-decisions](../00-decisions.md) L49 |
| R-20 | **thiserror 1.x→2.0 補間非互換**。2.0 で `#[error("{x}")]` のフィールド補間解決順序が厳格化、1.x 前提コードで補間対象がずれる。 | 低 | 低 | 新規は **2.0.18 固定**で問題なし。`#[from]` は真の層境界のみ（同型複数バリアント衝突を回避）、層跨ぎは明示 `match`。`#[error(transparent)]` は最下段限定。 | [errors-thiserror](../verification/rpt-errors-thiserror.md) L20-23（確信度:高）／[00-decisions](../00-decisions.md) L45-46 |
| R-21 | **`let _ = ...(` grep ゲートの形骸化**。誤検知（`tx.send`/guard 束縛/`writeln!`）だらけで CI 常時赤→開発者が `// intentional-ignore:` を機械貼付、本物の黙殺（`.ok()`/`.unwrap_or_default()`）は捕捉できない。 | 中 | 中 | grep をやめ clippy の `let_underscore_must_use`/`let_underscore_future`/`let_underscore_untyped` を deny 昇格し `#[must_use]`（`Result` は自動）破棄をコンパイラ検出。捨てて良い箇所は名前付きヘルパー（`ignore_cache_miss`）へ一本化。正確を期すなら dylint カスタム lint。 | checkpoint A-1（確信度:高） |

### 13.6 移行戦略・組織

| # | リスク | 影響 | 確率 | 対策 | 参照 |
|---|---|---|---|---|---|
| R-22 | **移行の長期化・ファサード恒久化（strangler アンチパターン）**。過渡的アーキテクチャが恒久化し、ロールバック無し/ビッグバン/共有 DB 密結合/可観測性欠如/プロキシ SPOF に陥る。 | 高 | 中 | **完了基準を per-slice で明文化**（機能パリティ=契約テスト合格・カナリアで所定期間 SLO 内・セッション/データ整合性検証済み→done）。**レガシー削除は検証後の最終ステップ**、旧経路を warm 維持し nginx 1行で即ロールバック。ロールバック窓ではスキーマ凍結。カナリアは REST 先・WS 最後。shadow/mirror は**読み取り専用/冪等のみ**（書込バックエンドへ mirror=二重書込）。 | [nginx-session-strangler](../verification/rpt-nginx-session-strangler.md) L265/L323/L384/L427（確信度:高）／[00-decisions](../00-decisions.md) L104 |
| R-23 | **`proxy_pass` 末尾スラッシュ罠**。nginx `proxy_pass` の末尾 URI 有無で書換挙動が変わり、新旧でパス prefix がずれる。 | 中 | 中 | **per-location `proxy_pass`（末尾 URI 無しでパス保存）**、スラッシュ規約を新旧で一致。WebSocket は `map $http_upgrade $connection_upgrade` + `Upgrade`/`Connection` 転送 + `proxy_read_timeout 3600s` + バックエンド ping。`__Host-` Cookie は Domain 不可＝**同一オリジン必須**（nginx 単一オリジン背後で出し分け）。 | [nginx-session-strangler](../verification/rpt-nginx-session-strangler.md) L114/L184（確信度:高）／[00-decisions](../00-decisions.md) L102 |
| R-24 | **Rust 学習コスト/借用チェッカーによる速度低下**。特に `ToolContext.embeds/files` への `&mut` push が非信頼 WASM に渡せず・並行ツール実行で借用衝突。 | 中 | 中 | 副作用を**戻り値へ寄せる**（`ToolOutcome.attachments`）＝`ctx` は `&ToolContext`（不変借用）で済み並行実行でも衝突しない。段階移行（strangler）で1スライスずつ習熟。既存 rust_synapse/crawler の知見を流用。 | checkpoint L776/L4058/L4378／[00-decisions](../00-decisions.md) 絶対制約4 |

---

## 14. オープンな決定事項（ユーザー判断が要る点）

> [`00-decisions.md`](../00-decisions.md) 末尾5項目を判断材料（トレードオフ）とともに展開。各項目に**既定（推奨）／対抗案／判断の分かれ目**を明示。

### 14.1 フロント型生成 — ts-rs vs utoipa+openapi-typescript

| 観点 | 既定（推奨）= **ts-rs 12.0.1** | 対抗案 = **utoipa 5.5.0 → OpenAPI 3.1 → openapi-typescript 7.13.0（+openapi-fetch）** |
|---|---|---|
| 生成範囲 | 型のみ（`ApiResponse<T>` エンベロープを型化） | エンドポイント契約＋**型付きクライアント**まで |
| 工数 | 低（現行 `types.ts` 655行の二重管理を直接解消） | 増（OpenAPI アノテーション+2段パイプライン） |
| 保守 | 活発（repo 非アーカイブ、2026-06-17 コミット、serde-compat 安定） | OpenAPI 標準に載る＝将来的な相互運用性 |
| 却下 | — | **specta は不適**（v2 が RC 継続・Tauri 志向・rspc 終了） |

- **判断の分かれ目**: エンドポイント契約と型付きクライアント（`openapi-fetch`）まで欲しいか。型の単一真実源だけなら ts-rs で十分・低リスク。REST 契約テストやクライアント自動生成の投資対効果を取るなら utoipa（上位互換）。
- **推奨既定値**: **ts-rs**。契約まで欲しくなった時点で utoipa へ拡張可能（後戻り不可ではない）。
- 参照: [typegen-tsrs-utoipa](../verification/rpt-typegen-tsrs-utoipa.md)（確信度:高）／[00-decisions](../00-decisions.md) L96, L109

### 14.2 DB ドライバ — rusqlite vs sqlx

| 観点 | 既定（推奨）= **rusqlite 0.40.1（bundled SQLite 3.53.2）** | 対抗案 = **sqlx 0.9（sqlx-sqlite）** |
|---|---|---|
| synapse 統一 | ○ 既存 `rust_synapse` が rusqlite 使用＝統一 | × 二重ドライバ化 |
| 書込予測性 | ○ 単一 writer thread を自然に構造化・明示的 | △ default プール（5〜50接続）は WAL 書込で**アンチパターン**（~20倍性能差、要 split-pool: read pool + `max_connections(1)` write pool） |
| コンパイル時検査 | × 実行時のみ | ○ compile-time-checked queries（動的 `ALTER`/`PRAGMA table_info` 内省とは相性悪） |
| async | 要 `spawn_blocking` | ネイティブ async |

- **判断の分かれ目**: コンパイル時クエリ検査の価値 vs 書込並行の footgun と synapse 二重化。sqlx を選ぶ場合は **split-pool パターン**（read pool + 単一接続 write pool）+ `WAL`/明示 busy_timeout/`synchronous=NORMAL` を**必須**採用しないと `SQLITE_BUSY`/ロック飢餓（sqlx-SQLite の最頻報告バグ）。
- **推奨既定値**: **rusqlite**。synapse 統一・書込経路の予測可能性・SQLite 機能の深い制御を優先。sqlx 選択でも single-writer 規律は同様に必須。
- 参照: [db-sqlx-vs-rusqlite](../verification/rpt-db-sqlx-vs-rusqlite.md)（split-pool footgun L60/L70/L75、確信度:高）／[00-decisions](../00-decisions.md) L67-69, L110

### 14.3 Gemini API 面 — classic generateContent（1:1移植）vs Interactions API 先行

| 観点 | 既定（推奨）= **classic `generateContent` v1beta で開始** | 対抗案 = **最初から Interactions API を狙う** |
|---|---|---|
| 移植コスト | ○ 現行 TS コード（camelCase・`src/gemini.ts:579` の ANY モード等）を 1:1 移植 | × Rust 生態系ほぼ未対応＝薄ラッパ自前必須・実装未知数 |
| サポート状況 | "legacy" だが **fully supported 継続**、メインライン model は当面投入継続 | GA「正面玄関」・新 frontier / 長時間エージェント機能はこちらのみ |
| リスク | 将来の legacy 縮退（記録済リスク R-6） | 早期採用リスク・移行資料が両 API 混在で誤りやすい |

- **判断の分かれ目**: 移植の確実性（既存挙動 1:1）を取るか、将来の frontier 機能アクセスを先取りするか。両 API は JSON フィールド名が別物（generateContent=`inlineData`/`functionResponse` camelCase、Interactions=`function_result`/`call_id`/`previous_interaction_id` snake系）で混在が事故源。
- **推奨既定値**: **generateContent で開始し、Interactions 移行を将来課題**。薄ラッパで API 層を抽象化し移行を局所化（R-6）。camelCase struct に厳密固定。
- 参照: [gemini-design-verify](../verification/rpt-gemini-design-verify.md) L11/L15/L60/L147（確信度:高）／[00-decisions](../00-decisions.md) L79, L111

### 14.4 edition — 2021 vs 2024

| 観点 | 既定（推奨）= **edition 2021** | 対抗案 = **edition 2024** |
|---|---|---|
| MSRV | 低め（幅広いツールチェーンで動作） | 2024 は新しめの rustc 必須（MSRV 引き上げ） |
| 安定性 | 枯れた挙動・移行資料豊富 | 新機能（RPIT lifetime capture 変更・`unsafe` 属性・`gen` 予約等）の破壊的差分に注意 |
| 現行整合 | checkpoint の Cargo.toml 例が `edition = "2021"` | — |

- **判断の分かれ目**: rustc 1.96.1 は 2024 対応済みで MSRV 制約は実質軽微だが、2024 は semantics 変更（クロージャキャプチャ・`static mut` 参照・match ergonomics 等）を含む。安定運用重視なら 2021、新エディション機能を積極活用するなら 2024。
- **推奨既定値**: **edition 2021**（枯れた挙動・現行 checkpoint 例と整合）。全 workspace メンバーで統一。新機能が必要になれば `cargo fix --edition` で移行可能。
- 参照: [00-decisions](../00-decisions.md) L112／checkpoint L3242（`edition = "2021"` 例）／rustc 1.96.1（[00-decisions](../00-decisions.md) L19）

### 14.5 プラグイン初期スコープ — Native+MCP 先行 vs 初期から3系統

| 観点 | 既定（推奨）= **Native+MCP を先行、WASM(Extism) は後続フェーズ** | 対抗案 = **初期から3系統（Native/MCP/WASM）揃える** |
|---|---|---|
| 初期工数 | 低（`ToolProvider` trait + Native/McpProvider のみ） | 高（Extism/wasmtime サンドボックス・Manifest 能力設計を初期に） |
| 現行パリティ | Native=現行 `src/functions/*`、MCP=現行 `mcpDynamic`/`mcpClient` を吸収＝**現行機能を即カバー** | 同上＋非信頼ユーザープラグイン |
| 非信頼拡張 | 後続まで不可（絶対制約4の完全達成は遅延） | 初期から可（ただし cargo-component 停滞・Manifest 詳細要確認 R-11） |

- **判断の分かれ目**: 絶対制約4（ユーザー製カスタムモジュール拡張）を**初期設計に織り込む**ことは必須だが、`ToolProvider` trait レジストリを最初に確立すれば WasmProvider は後から差し込める（trait 境界が拡張点）。非信頼プラグインの需要タイミングが分かれ目。
- **推奨既定値**: **Native+MCP 先行**、ただし `ToolProvider` trait とツール名 namespace（Gemini 128字/`[a-zA-Z0-9_:.-]` 準拠）を初期に確立し WASM を差し込み可能に。非信頼実行は Extism（deny-by-default Manifest）で後続フェーズ。
- 参照: [plugins-wasm-extism](../verification/rpt-plugins-wasm-extism.md)／[mcp-rmcp](../verification/rpt-mcp-rmcp.md)（確信度:高）／[00-decisions](../00-decisions.md) L84-92, L113

---

### 決定事項サマリ（推奨既定値の一覧）

| # | 決定事項 | 推奨既定値 | 主リスク |
|---|---|---|---|
| 1 | フロント型生成 | **ts-rs 12.0.1**（契約要件出現時に utoipa へ） | R-12 型ドリフト |
| 2 | DB ドライバ | **rusqlite 0.40.1**（single-writer 規律必須） | R-1/R-2 SQLite BUSY |
| 3 | Gemini API 面 | **classic generateContent で開始**（薄ラッパ抽象化） | R-6 legacy 化 |
| 4 | edition | **2021**（枯れた挙動・現行整合） | MSRV/semantics 差分 |
| 5 | プラグイン初期スコープ | **Native+MCP 先行**（trait で WASM 差込可） | R-10/R-11 サンドボックス |
