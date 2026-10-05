# Rust 版サーバー実装レビュー（Phase 0〜1 / T1 時点）

- 実施日: 2026-07-06
- 対象ブランチ: `feature/rust-rewrite`（HEAD: `501aee3`）
- 対象範囲: `crates/` 配下の実装済み 16 クレート、約 9,250 行（`yuuka-discord` / `yuuka-gemini` / `yuuka-services` / `yuuka-tools` はプレースホルダのため対象外）
- 手法: 領域別 4 系統の並行精読レビュー（Web/認証/起動系・基盤・ドメイン A 群・ドメイン B 群）＋ Node/TS 参照実装（`src/` 配下）との挙動対照 ＋ 機械検査の実測。HIGH 級所見はレビュー後に実コードで再検証済み。

## 機械検査の実測結果

| 検査 | 結果 |
|---|---|
| `cargo clippy --workspace --all-targets` | クリーン（警告ゼロ。unwrap/expect/panic 系 deny lint 通過） |
| `cargo test --workspace` | **90 passed / 0 failed** |
| `cargo deny check` | advisories / bans / licenses / sources すべて ok |
| `cargo fmt --check` | **差分あり（約 20 ファイル・全域に散在）** — フォーマット未適用コードが混在 |

絶対制約 1（エラー握り潰し禁止・panic 系禁止）は本番コードで違反ゼロ。`unwrap`/`expect` はすべて `cfg(test)` 内（`clippy.toml` の `allow-*-in-tests` で明示緩和済み）。

## 所見一覧（重大度順）

### HIGH

| # | 場所 | 所見 |
|---|---|---|
| H-1 | `crates/yuuka-todo/src/dto.rs:17,42` / `crates/yuuka-timeline/src/dto.rs`（入力系）/ `crates/yuuka-personal/src/dto.rs:21,43` | **入力 DTO の `#[serde(rename_all = "camelCase")]` 欠落の系統バグ**。フロント/Node 契約は `dueDate` / `startDate` / `parentId` / `recordedAt` / `todoId` / `contactInfo` 等 camelCase を送るが、Rust 側は snake_case のみ受理し `#[serde(default)]` で **無音で `None` に落ちる**（200 で成功するため気づけない）。personal では update が全列上書きのため **既存の連絡先情報が NULL で消去される**。`yuuka-schedule` の入力 DTO（`dto.rs:26,55`）だけ正しく付与されており、fan-out 時の一貫性逸脱。~~出力（ビュー）DTO は各クレートとも camelCase 付与済み~~ **← 訂正（2026-07-06）: 事実誤認。主要ビュー DTO は snake_case（フロント受信型と一致し正しい）。camelCase 付与済みの出力は `*DeletedData` 系 7 struct のみで、これは逆に是正対象。詳細は [review-2026-07-06-fix-policy.md](review-2026-07-06-fix-policy.md) §2.1〜2.2 を優先基準とする。** |
| H-2 | `crates/yuuka-web/src/lib.rs:50-58` / `static_files.rs` | **CSP・Referrer-Policy が全く付与されず、Node のセキュリティヘッダ群から後退**。Node は全静的応答に `Content-Security-Policy`（`unsafe-inline` を script-src から排除した実効 XSS 防御）と `Referrer-Policy` を付与（`src/server.ts:82-89`）。PLAN §6.6 は「そのままの値で移植」を明記するが、Rust の `apply_common_layers` は `nosniff` と `X-Frame-Options` のみ。TODO 表明もなく docstring は実装済み扱い。SPA を Rust が配信し始めた瞬間に XSS 多層防御が消える。 |
| H-3 | `crates/yuuka-todo/src/repo.rs:41-47` / `routes.rs:42-50` | **`GET /api/tasks` の形状・ソート・フィルタが Node と乖離**。Node は親のみ＋`subtasks` ネスト＋`effective_progress`、`status`/`tag` フィルタ、優先度→期日→作成日ソート。Rust は全件フラット・`created_at DESC` のみ・フィルタ無視。既存フロント（`TodoWithSubtasks` 依存）ではサブタスクが親と同列に重複表示され `done` フィルタも効かない。lib.rs の deferred 宣言にツリー化・フィルタ欠落の明記なし。 |

### MED

| # | 場所 | 所見 |
|---|---|---|
| M-1 | `crates/yuuka-web/src/auth.rs:85-89` + `session.rs:75` | **Redis 実行時障害で Cookie 保持者が全員 502 になり、Bearer フォールバックも走らない**。Node は session 解決の失敗を catch→null で Bearer 継続。Rust は `?` で伝播するため、有効な Bearer を持つデスクトップクライアントも巻き込まれ、`OptionalUser` の任意認証ルートまで 502 化。docstring の「Redis 到達不能でも縮退」は起動時 failure にしか成立しない。 |
| M-2 | `crates/yuuka-web/src/extract.rs:62-66` | body 上限超過（10MB）が Node の 413 でなく 400 になる／**空ボディ（`Content-Length: 0`）が 400 拒否**される（Node は `{}` で続行）。全フィールド任意の DTO や削除系 POST の parity break。 |
| M-3 | `crates/yuuka-web/src/static_files.rs:37-51` | 静的配信のキャッシュ/404 挙動差 3 点: (a) `/assets` の **404 にも `immutable` 1 年キャッシュ**が付き、デプロイ中の先行 404 が恒久化。(b) index.html 等の非ハッシュ資産に `no-cache, no-store` が無く古い SPA シェルが残留→白画面リスク。(c) 拡張子付き未存在パス（`/sw.js` 等）にも index.html が 200 で返り、SW 更新・欠落検知が静かに壊れる（Node は拡張子なしのみ SPA フォールバック）。 |
| M-4 | `crates/yuuka-core/src/error.rs:347` | `AppError::fatality()` が `Auth(_)` を一律 `Permanent` 分類。`AuthError::Backend` 自身の doc（「回復可能な上流障害」）および PLAN §5.6 と矛盾し、**Redis 断からの自己復帰が働かない**。DbError 側では「一律分類禁止」の教訓を明記しており自己矛盾。 |
| M-5 | `crates/yuuka-db/src/writer.rs:39-41` | writer スレッドのジョブ実行に `catch_unwind` がなく、**panic 1 発で writer が黙って死に以後の全書き込みが `WriterGone`**。しかも `WriterGone` は `Permanent` 分類のため supervisor の再 spawn 対象外（実際は spawn やり直しで直る障害）。`arithmetic_side_effects` lint は無いため debug ビルドの整数オーバーフロー等で現実に踏み得る。 |
| M-6 | `crates/yuuka-todo/src/repo.rs:170-184` | delete が子孫を連鎖削除しない（Node は `WITH RECURSIVE`）。**孤児サブタスクがデータ汚染として蓄積**し、フラット list にルートとして出現し続ける。 |
| M-7 | `crates/yuuka-todo/src/routes.rs:52-64` | `priority` の正規化・検証が皆無（Node は `normalizePriority`）。任意文字列が永続化され、併走期の Node 側 CASE ソートを壊す。旧 UI 互換の数値 `priority: 2` も 400（Node は受理）。 |
| M-8 | `crates/yuuka-timeline/src/dto.rs:47-71` / `routes.rs:60-77` | `POST /api/timeline/record` が Node で受理しない内部列（`expense_id`/`media_path`/`media_type`）の直接指定を素通し INSERT。`type=expense` の `amount` 必須検証・expenses 二重登録も無く、フロントの `category` キーとも不一致。**deferred が「拒否」でなく「無音で劣化データを作る」方向に倒れている**。 |
| M-9 | `crates/yuuka-reminder/src/repo.rs:111-139` | `trigger_at` を無検証・無正規化で保存（Node は `toDbDateTime` 正規化＋cron 検証＋過去日時処理）。`"2026-07-06T12:00"` 形式が混入すると **共有 SQLite を読む Node 側リマインドエンジンの比較・ソートが誤動作**。保存済みデータ汚染は後付け修正で救えない。 |
| M-10 | `crates/yuuka-credential/src/routes.rs:41-72` | (a) GET 一覧に `bot_credential_access` 許可フィルタが無く owner の全認証情報インデックスを返す。(b) delete が許可掃除（`deleteAllGrantsForCredential` 相当）と監査ログ `credential.delete` を書かない。混在運用中に「同名再登録で許可が復活」するリスクが DB に残る。（deferred 宣言はあるが移行期に実害化） |
| M-11 | `crates/yuuka-persona/src/repo.rs:158-171` | delete が personas 行のみ削除（Node はトランザクションで適用解除＋`recommended_persona_id` 解除を同時実行）。**適用中参照がダングリング**し Node runtime が消えた ID を返す。あわせて `PersonaListData` に `active_persona_id` が無く「適用中」表示を失う。 |
| M-12 | 各クレート routes（例 `crates/yuuka-playbook/src/routes.rs:68`） | **mutation 応答形状の系統差**: Node の 200+`{success:false}` → Rust は 404、save 系の `{success,message}` → `{success,<entity>}` 等。doc で「golden test で最終確定」と宣言された意図的差分だが、`success` フィールドで分岐する既存フロントはエラートースト経路に変わる。golden test 前に方針確定が必要。 |

### LOW（要点のみ）

- `crates/yuuka-web/src/csrf.rs:27-34,71-80` — Bearer 併存時の CSRF 免除が Node の実挙動より緩い（Cookie＋無効 Bearer の cross-site POST が素通り。ブラウザ制約により実悪用は不可）／Origin 照合先が Host ヘッダで PLAN §6.4（baseUrl）と乖離。
- `crates/yuuka-web/src/session.rs:34-38` — `redis_url` に secrecy 未適用。`redis://:pass@host` 形式時、接続失敗の warn ログへ資格情報断片が漏れ得る。
- `crates/yuuka-auth/src/session.rs:87-88` / `desktop.rs:42,61` — `expire` 失敗の無音破棄、`map_err(|_| AuthError::Backend)` の原因喪失。tracing にも出ず 502 の原因が追えない。
- `crates/yuuka-supervisor/src/main.rs:69-72` — `tokio-graceful-shutdown` 未使用（workspace 依存宣言のみ）＋ドレイン待ちに上限が無く、ハング接続 1 本で SIGTERM 後も終了しない。（後続増分と docstring に明記済み）
- `crates/yuuka-core/src/config.rs:55` — config ファイル読取エラーを無警告で空 Mapping に畳む（doc コメントは「警告扱い」と主張、Node は console.warn を出す）。「設定が静かに全部既定値」になる。
- `crates/yuuka-core/src/config.rs:157-170` — `TRUSTED_PROXIES` が `IpAddr` パース必須になり、Node で通っていた設定値（`::ffff:127.0.0.1` 等）の受理集合が変化。fail-fast 方向だが既存 config.yaml が通らない可能性。
- `crates/yuuka-db/src/lib.rs:6-7` — doc drift: 「deadpool-sqlite」と記載するが実装は内製プール。workspace Cargo.toml に未使用の `deadpool-sqlite` / `refinery` が宣言されたまま。
- `crates/yuuka-db/src/schema.rs:34-38` — 起動時スキーマ検査が一過性の `SQLITE_BUSY` も「schema incompatible」として起動拒否＋誤誘導ログにする。
- `crates/yuuka-db/src/pool.rs:140` — freelist 空振り時の同期 `open_conn` が async 直上で走り executor を短時間ブロック。
- 全ドメイン共通 — mutation 成功後の `get()` がトランザクション外の ReadPool で行われ、並行 delete と重なると「更新は成功したのに 404」の微小レース（todo 参照実装由来）。
- `crates/yuuka-finance/src/routes.rs:47-56` — 数値の受理幅が Node より狭い（文字列 `"1500"` を 400）。schedule の `remindBeforeMinutes` も同様。
- `crates/yuuka-reminder/src/routes.rs:42` — `POST /api/reminders/delete` は Node に存在しないルートの新設（契約凍結方針との整合を要確認）。
- `crates/yuuka-playbook/src/repo.rs:116-122` / `crates/yuuka-persona/src/repo.rs:175-187` — doc の「400 に変換」は虚偽（`DbError::Operation` は 500 写像）。現状到達不能だがコメントと実装の乖離。
- `crates/yuuka-timeline/src/routes.rs:52-53` — 空文字 `?date=` の扱いが Node と逆（コメントの主張と Node 実挙動が不一致）。
- リポジトリ全体 — `cargo fmt --check` 差分が約 20 ファイル。CI ゲート化を推奨。

### 所見ゼロを確認した観点

- **SQL インジェクション**: 全ドメインでゼロ。`format!` 埋め込みはカラム定数のみ、ユーザー入力は全て bind パラメータ（schedule の `'+' || ?3 || ' days'` も Node と同一のパラメータ化）。
- **クロス bot 認可（body-botId 系統）**: ゼロ。全 mutation が `ScopedJson`（body 優先→query フォールバック、Node `resolveBotId` の nullish 優先と一致）＋共通 `resolve_scope`（未アクセスは `system_default` へフェイルクローズ）で一貫。過去の系統バグ修正の取りこぼしなし。
- **credential の秘密値漏洩**: ゼロ。暗号化列は SELECT 列定数に存在せず、DTO にフィールド自体が無く、tracing/Debug 経由の漏れなし。
- **認証バイパス・セッション固定・タイミング攻撃・path traversal**: ゼロ。`__Host-` Cookie 限定受理、sha256 キー照合、`ServeDir` 正規化を確認。
- **panic 系 lint の掻い潜り**: 本番コードでゼロ（`Semaphore::new` の理論上 panic 経路 1 件は現状定数のみで実害なし）。

## 良い点

- **認可の型強制と一元化**: `AuthenticatedUser`/`AdminUser`/`OptionalUser` extractor、`ScopedJson`、`resolve_scope`、`CrossUserAccess` 証憑 + `UserScope` newtype により、最も危険なクラスのバグ（認可漏れ・スコープ迷子・クロステナント）が構造的に封じられている。
- **厳格エラーアーキテクチャの貫徹**: 網羅 match（`_ =>` 禁止）でバリアント追加漏れがコンパイルエラー化。`#[from]` は真の層境界のみ。内部 Display をクライアントに漏らさない `client_message()` がテストで凍結。
- **dual-SQLite hazard 対策が報告書どおり構造的に実装**: BEGIN IMMEDIATE 強制・busy_timeout 明示・read-only+query_only・CREATE フラグ除去・DDL 不発行の schema_version ガード。それぞれにテストあり。
- **クリーンビュー DTO**: `user_id`/`bot_id`/暗号化列がフィールド自体に存在しない構造的フェイルクローズ。Node が生 row を返す点より安全側。
- **テストが攻撃シナリオ・回帰防止指向**: CSRF マトリクス、`__Host-` 強制、スコープ分離、内部列非露出、ts-rs export 登録漏れのソース走査検出、生成 TS への機密フィールド混入検出。
- **逸脱の文書化**: deadpool-sqlite 非採用理由、deferred 項目の各 lib.rs 冒頭列挙など、意図的スコープ縮小と実装漏れが区別可能。

## 推奨対応順

1. **H-1（入力 DTO の camelCase 欠落）** — 既存フロント接続で即発火・データ消去を伴う。todo/timeline/personal の入力 DTO へ `rename_all` 付与＋wire 形式の回帰テスト追加。schedule 型のテストを横展開。
2. **H-2（CSP 欠落）＋ M-3（静的配信のキャッシュ/404）** — Rust が SPA を配信し始める前に必須。
3. **H-3（/api/tasks 形状乖離）＋ M-12（応答形状方針）** — golden test 整備の前に契約を確定。
4. **M-1/M-2（認証縮退・413/空ボディ）** — Web 層の parity 修正としてまとめて。
5. **M-4/M-5（Auth 一律 Permanent・writer panic 非隔離）** — 自己復帰設計の根幹。supervisor 本配線前に。
6. **移行期データ整合性系（M-6〜M-11）** — Node と DB を共有する期間に汚染が蓄積する項目。deferred のままにする場合は最低限「無音劣化」を 400 拒否へ倒す。
7. LOW 群と `cargo fmt` 適用＋CI ゲート化。

## 総評

Phase 0 で敷いた基盤（厳格エラー・型による認可強制・dual-SQLite 対策）は設計文書への忠実度が高く、fan-out された 9 ドメインもパターン一貫性を保っている。機械検査は fmt を除き全通過で、テストも回帰防止に実質的に効いている。一方で **wire 契約のパリティに系統的な穴**（入力 DTO の camelCase、静的配信ヘッダ、応答形状）があり、これらは「200 で成功するのに無音でデータが落ちる」型のため、既存フロントを接続する増分の**前**に潰す必要がある。deferred 宣言の規律は良いが、いくつかは「未実装＝安全に拒否」ではなく「未実装＝劣化データを黙って作る」方向に倒れており、移行期の DB 共有を踏まえた優先度の再評価を推奨する。
