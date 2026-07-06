# レビュー対応方針 — Phase 0〜1 / T1 所見の修正計画

- 起票日: 2026-07-06
- 対象レビュー: [review-2026-07-06-phase1.md](review-2026-07-06-phase1.md)（HIGH 3・MED 12・LOW 群）
- 対象ブランチ: `feature/rust-rewrite`（HEAD: `501aee3`）
- 位置づけ: **本ドキュメントは修正着手前の「方針の唯一の基準」**。各所見の処置（対応／fail-closed 縮退＋deferred／却下）・実施順・検証方法・deferred として残す範囲をここで確定する。以後の修正コミットは本ドキュメントに整合させる。

---

## 0. 確定した上位方針（ユーザー判断）

| 論点 | 決定 |
|---|---|
| スコープ | **HIGH＋MED を段階バッチで実施**（レビュー推奨順）。LOW 群は最終バッチで fmt/CI ゲートと併せて処理。 |
| 移行期データ整合（M-6〜M-11） | **まず fail-closed**（「無音で劣化データを作る」経路を 400 拒否／正規化に倒し、DB 汚染の蓄積だけ止める）。許可掃除・トランザクション等の **Node 完全 parity は deferred として明示**。**例外: M-6（連鎖削除）はレビュー指摘を受け parity 実装に変更**（§5・Batch 6 参照）。 |
| mutation 応答形状（M-12） | **移行期は Node と厳密一致**。`success` フィールドで分岐する共有フロントを壊さない。golden test で凍結。「よりクリーンな形状」はカットオーバー後に再検討。 |

上位規範は変わらず [00-decisions.md](00-decisions.md) の絶対制約群（厳格エラー・自己復帰＝致命的設定不備のみ fail-fast・フロント⇄Rust 型の単一真実源）に整合させる。

---

## 1. 修正哲学（なぜこの倒し方か）

移行期は **新旧バックエンドが同一 SQLite と同一フロントを共有**する（[parts/06-migration-workflow.md](parts/06-migration-workflow.md) §11.3〜11.4）。この前提から、修正の優先度は「ユーザー可視のクラッシュ」より **「200 で成功するのに無音でデータが落ちる／劣化データを共有 DB に書く」型のパリティ欠陥**を上に置く。理由は 3 点:

1. **無音のデータ破壊は後付けで救えない**。既に NULL 上書き・孤児行・非正規化日時が書かれた後では、Node 側リマインドエンジンやソートが誤動作しても原因追跡が困難。
2. **フロントは共有・未改修**。Rust が返す形状／キー名が Node とズレると、`success` 分岐や snake_case 依存のコンポーネントがエラートースト経路や白画面に落ちる。カットオーバーまでフロントは触らない前提なので、**Rust 側が Node の実挙動に合わせる**のが唯一の安全策。
3. **deferred は「安全に拒否」であって「黙って劣化」ではない**。未実装の機能は 400 で明示拒否し、劣化データを DB に残さない。これが今回の fail-closed 方針の核。

---

## 2. 確定した地上真実 ＆ レビュー記述の訂正

修正の土台となる事実をコード・Node 参照・フロント実装で実測確認した。**レビューには一部事実誤認があり、本節を優先基準とする。**

### 2.1 wire 契約は「入力 camelCase・出力 snake_case」の非対称（最重要）

Node 参照とフロントを実測した結果、契約は左右非対称:

| 方向 | 形式 | 実測根拠 |
|---|---|---|
| **リクエスト body（入力）** | **camelCase** | Node は `body.dueDate` / `body.startDate` / `body.parentId` を読む（[src/server/routes/todoRoutes.ts](../../src/server/routes/todoRoutes.ts) `:153-156`）。フロントも `dueDate`/`startDate`/`parentId` を送る（[frontend/src/lib/api/services/taskApi.ts](../../frontend/src/lib/api/services/taskApi.ts) `:31-47`、各 modal）。 |
| **レスポンス（出力）** | **snake_case** | Node は `SELECT *` の生 row をそのまま返す（`due_date`/`start_date`/`parent_id`/`created_at`、[src/db/todoRepo.ts](../../src/db/todoRepo.ts)）。フロントの受信型も snake_case（`due_date`/`parent_id`/`subtasks`/`effective_progress`、[frontend/src/lib/api/types.ts](../../frontend/src/lib/api/types.ts) `:106-124`）。Node にグローバル camel/snake 変換層は存在しない。 |

**帰結（レビュー H-1 の訂正）:**
- レビューの「**出力（ビュー）DTO は各クレートとも camelCase 付与済み**」は**事実誤認**。実際は全ドメインの view DTO（`Todo`/`Schedule`/`TimelineRecord`/`Contact`）が **snake_case で、これは正しい**（フロントの受信型と一致）。**view DTO に camelCase を足してはならない**（足すとフロントが壊れる）。
- 一方、レビュー H-1 の**結論（入力 DTO に camelCase が必要）は正しい**。現状 `NewSchedule` だけ `#[serde(rename_all = "camelCase")]` を持ち、`NewTodo`/`NewTimelineRecord`/`NewContact` 等が欠落 → camelCase の `dueDate` 等が `#[serde(default)]` で**無音で `None` に落ちる**。personal では update が全列上書きのため `contactInfo` 欠落が **既存連絡先を NULL で消去**する。
- **修正の向き（確定）**: 入力（`New*` / update 入力）DTO にのみ `rename_all = "camelCase"` を付与。**出力 view DTO は snake_case のまま据え置く**。「input=camelCase / output=snake_case」を本移行の wire 契約不変条件として明文化し、回帰テストで両方向を凍結する。

### 2.2 出力の camelCase 混入は逆に是正対象（レビュー後の追検証で不一致が確定）

対象は 3 struct でなく **`*DeletedData` 系の全 7 struct**: `DeletedData`（todo）/`ScheduleDeletedData`/`ContactDeletedData`/`TimelineDeletedData`/`ExpenseDeletedData`/`PersonaDeletedData`/`PlaybookDeletedData`/`CredentialDeletedData`。いずれも出力なのに `rename_all = "camelCase"`（`deletedId`）が付いている。

さらに追検証の結果、「一致している保証がない」ではなく**不一致が確定**:

- **Node は `deletedId` 自体を返さない**。delete/cancel 応答は `{ success: ok }` のみ（[src/server/routes/todoRoutes.ts](../../src/server/routes/todoRoutes.ts) `:261`。Node ルート全体を grep しても `deletedId` はゼロ件）。→ casing 是正ではなく**フィールドごと削除**が正解の見込み。
- `frontend/src/lib/api/generated/*DeletedData.ts` に `deletedId` が見えるが、これは **Rust DTO から ts-rs で生成されたファイル**であり Node 契約の証拠ではない。生成物を根拠に「フロントが使っている」と誤認しないこと。

→ Batch 3（応答形状の Node 厳密一致）で 7 struct を一括是正し、**`generated/` の再生成を完了条件に含める**。

### 2.3 契約凍結違反の新設ルート

`POST /api/reminders/delete` は **Node に存在せず**（Node は `/api/reminders/add` と `/api/reminders/cancel` のみ）、**フロントも未使用**。Rust が独自に追加した新設ルート（[crates/yuuka-reminder/src/routes.rs](../../crates/yuuka-reminder/src/routes.rs) `:42`）。契約凍結方針に反するため**移行期は撤去**（内部 CRUD 完備は保持してよいが公開ルートからは外す）。

---

## 3. バッチ計画（実施順と完了条件）

レビュー推奨順に沿い、各バッチを 1 コミット（必要なら 2）にまとめる。**各バッチは「フロント接続を阻む前提」→「Web 層 parity」→「自己復帰根幹」→「移行期データ整合」→「LOW＋衛生」の依存順**。

### Batch 1 — wire 契約パリティ（フロント接続の前提）【HIGH】
- **H-1**: 入力 DTO（`NewTodo`/todo update 入力・`NewTimelineRecord`・`NewContact`・その他 fan-out 入力）に `rename_all = "camelCase"` を付与。view DTO は snake_case 据え置き（2.1）。
- **検証**: `NewSchedule` 由来の wire テストを横展開し、**camelCase body → 正しくデシリアライズ**＋**view 出力が snake_case のまま**を各ドメインで凍結する回帰テスト。
- **完了条件**: `dueDate`/`startDate`/`parentId`/`recordedAt`/`todoId`/`contactInfo`/`remindBeforeMinutes` が全入力経路で受理され、既存フロント接続で NULL 消去が起きないことをテストで保証。

### Batch 2 — 静的配信 ＆ セキュリティヘッダ（SPA 配信の前提）【HIGH＋MED】
- **H-2**: `CSP`（script-src から `unsafe-inline` 除外）と `Referrer-Policy` を全静的応答に付与。PLAN §6.6 の**同一文字列**を `SetResponseHeaderLayer` で移植（[docs/rust-rewrite/PLAN.md](PLAN.md) §6.6、値は既定義済み）。
- **M-3**: 静的配信のキャッシュ/404 挙動を Node 一致に:
  - (a) `/assets` の **404 に `immutable` キャッシュを付けない**。
  - (b) index.html 等の非ハッシュ資産に `no-cache, no-store` を付与（古い SPA シェル残留＝白画面の防止）。
  - (c) SPA フォールバックは**拡張子なしパスのみ**。`/sw.js` 等の拡張子付き未存在パスは index.html を返さず 404（SW 更新・欠落検知を壊さない）。
- **完了条件**: Rust が SPA を配信し始める**前**に、XSS 多層防御とキャッシュ挙動が Node と等価。ヘッダ有無・キャッシュ・SPA フォールバック分岐のテスト。

### Batch 3 — `/api/tasks` 形状 ＆ 応答形状契約の確定【HIGH＋MED】
- **H-3**: `GET /api/tasks` を Node 一致に = **親のみ＋`subtasks` ネスト**、`effective_progress` 算出、`status`/`tag` フィルタ、**優先度→期日→作成日ソート**（Node の `ORDER_CLAUSE`／`TodoWithSubtasks`）。
- **M-12**: mutation 応答形状を **Node と厳密一致**（0 節の決定）。Node の `200 + {success:false}` を Rust が 404 に変えている箇所、save 系の `{success,message}` vs `{success,<entity>}` 等を Node 実挙動に合わせる。**2.2 の `*DeletedData` 全 7 struct もここで是正**（Node は `deletedId` を返さないためフィールドごと削除の見込み。Node 実応答を golden として確定）。
- **完了条件**: 応答の HTTP ステータス・キー名・`success` セマンティクスを golden test で凍結。フロントの `success` 分岐がエラートースト経路に落ちない。**ts-rs `generated/` を再生成し、`deletedId` 等の幽霊フィールドが生成物から消えていること**。

### Batch 4 — Web 層 parity（認証縮退・ボディ制限）【MED】
- **M-1**: session 解決の Redis 実行時障害を `?` 伝播にせず **catch→null で Bearer フォールバック継続**（Node 踏襲）。有効 Bearer を持つデスクトップクライアントや `OptionalUser` 任意認証ルートを 502 に巻き込まない。**縮退時の `tracing::warn!` を必須とする** — 無音で null に畳むと「全員 Cookie 認証が静かに効かなくなる」障害が追跡不能になるため（Batch 7 の「502 の原因を tracing に残す」と同一原則）。
- **M-2**: body 上限超過（10MB）を **413**（Node 準拠、現状 400）に。**空ボディ（`Content-Length: 0`）を 400 拒否せず `{}` 相当で続行**（全フィールド任意 DTO・削除系 POST の parity）。
- **完了条件**: Redis 断中に Bearer 経路が生きる／413・空ボディ挙動のテスト。

### Batch 5 — 自己復帰の根幹【MED】
- **M-4**: `AppError::fatality()` の `Auth(_)` 一律 `Permanent` を是正。`AuthError::Backend`（回復可能な上流障害）を **`Transient` 分類**にし、Redis 断からの自己復帰を機能させる（PLAN §5.6/§5.7 と整合、[docs/rust-rewrite/PLAN.md](PLAN.md) §5.7）。
- **M-5**: writer スレッドのジョブ実行を **`catch_unwind` で隔離**し、panic 1 発で writer 全滅 → 以後全書き込み `WriterGone` を防ぐ。あわせて **`WriterGone` を再 spawn 可能な分類に**（supervisor 再起動対象）。debug ビルドの整数オーバーフロー等で現実に踏み得るため必須。
- **完了条件**: writer panic 後に supervisor が再 spawn して書き込みが復帰するテスト。Auth::Backend が `Transient` に分類されるテスト。

### Batch 6 — 移行期データ整合（fail-closed 優先・完全 parity は deferred）【MED】
各項目は **今すぐ fail-closed**（劣化データを DB に残さない最小対応）を実施し、**Node 完全 parity は deferred として lib.rs 冒頭に明示**する。

| # | fail-closed（今回・DB 汚染を止める） | deferred（完全 parity・別途） |
|---|---|---|
| M-6 | ~~子を持つ親の delete を 400 拒否~~ → **`WITH RECURSIVE` 連鎖削除の parity 実装に変更**。理由: 400 拒否は Node で成功する操作をエラートースト化するユーザー可視リグレッション（M-12「Node 厳密一致」と矛盾）であり、実装コストも「子の存在チェッククエリ」と大差ない（Node に完成形あり、[src/db/todoRepo.ts](../../src/db/todoRepo.ts) `:364-381`） | — （連鎖削除自体が parity） |
| M-7 | `priority` を **`normalizePriority` 相当で正規化／不正値拒否**、旧 UI 互換の数値 `priority:2` も受理。**注: 数値受理は `NewTodo.priority` の型変更（custom deserializer）を要し Batch 1 と同一ファイルを触るため、型変更は Batch 1 で済ませ正規化ロジックのみ本バッチで追加**（M-8 の timeline と同じ運用） | — （正規化自体が parity） |
| M-8 | 内部列（`expense_id`/`media_path`/`media_type`）の**直接指定を拒否**、`type=expense` の `amount` 必須検証。入力 DTO から内部列を除去（H-1 の timeline camelCase 化と同時に実施） | expenses 二重登録連携・`category` キー整合 |
| M-9 | `trigger_at` を **`toDbDateTime` 相当で正規化＋cron 検証**、非正規形（`"2026-07-06T12:00"`）を拒否 | 過去日時処理の完全再現 |
| M-10 | GET 一覧に **`bot_credential_access` 許可フィルタ**を適用（owner 全件露出の即時停止） | delete 時の許可掃除（`deleteAllGrantsForCredential` 相当）＋監査ログ `credential.delete` |
| M-11 | 適用中 persona の delete を **400 拒否**（ダングリング参照防止）、`PersonaListData` に `active_persona_id` を追加（「適用中」表示の復元） | delete のトランザクション化（適用解除＋`recommended_persona_id` 解除の同時実行） |

- **注**: M-8 の入力 DTO 是正は Batch 1（H-1）と同一ファイルを触るため、**timeline は Batch 1 で camelCase 化＋内部列除去をまとめて行い、amount 検証を Batch 6 で追加**する運用も可（コンフリクト回避）。実装時に確定。
- **完了条件**: 各 fail-closed 経路のテスト（拒否 400・正規化後の値）。deferred は lib.rs 冒頭列挙で「未実装＝安全に拒否」を明記。

### Batch 7 — LOW 群 ＋ 衛生（fmt/CI ゲート）【LOW】
- **撤去**: `POST /api/reminders/delete`（契約凍結、2.3）。
- **セキュリティ／可観測性**: `redis_url` に `secrecy` 適用（資格情報ログ漏れ防止）／`expire` 失敗と `map_err(|_|Backend)` の**原因を tracing に残す**（502 の原因追跡）／CSRF の Origin 照合先を Host でなく **baseUrl** に（PLAN §6.4）。
- **設定 parity**: config 読取エラーを無警告で空 Mapping に畳まず **warn 出力**（Node の `console.warn` 相当）／`TRUSTED_PROXIES` の `IpAddr` 厳格化で**既存 config.yaml（`::ffff:127.0.0.1` 等）が通るか実機確認**し、通らなければ受理集合を調整。
- **doc drift 是正**: `deadpool-sqlite` 記述と未使用依存（`deadpool-sqlite`/`refinery`）の除去／playbook・persona の「400 変換」虚偽コメント修正。
- **堅牢性小物**: schema 検査が一過性 `SQLITE_BUSY` を「incompatible」と誤判定しないよう区別。
- **fmt/CI**: `cargo fmt` 全域適用（約 20 ファイル）＋ **CI ゲート化**（`cargo fmt --check` / `clippy --workspace --all-targets` / `cargo deny check`）。
- **LOW 残項目の処置確定**（本ドキュメントの網羅性担保。漏れでなく意図であることの明示）:
  - **対応（本バッチ）**: finance/schedule の数値受理幅（文字列 `"1500"` を Node 同様 `Number()` 相当で受理 — M-7 の custom deserializer を共用）／reminder の `message`/`trigger_at` trim（M-9 の正規化と同時実施）。
  - **却下**: CSRF の Bearer 併存免除の緩さ（ブラウザは preflight 無しで `Authorization` を送れず実悪用不可。Node 側コメントの意図とも一致。カットオーバー後に Node 実装ごと厳格化を再検討）／personal の tags 非文字列救済（`[1,2]`→`["1","2"]`。握り潰し方向は Node と同一で実害僅少）／timeline の空 `?date=` フォールバック差（Rust 側が安全方向。コメントの「Node 同様」記述のみ Batch 7 の doc drift 是正で修正）。
  - **deferred のまま（今回対象外・明示のみ）**: supervisor の `tokio-graceful-shutdown` 未配線＋ドレイン上限（後続増分で既に宣言済み）／`pool.rs` freelist 空振りの同期 open_conn（micro）／mutation 後 `get()` の ReadPool レース（micro・todo 参照実装由来）／`Semaphore::new` の理論上 panic 経路（現状定数のみ。config 直結にする際に上限検査を追加）／reminder の `notify_target_*` 既定送信先（リマインドエンジン移植と同時に実装）。

---

## 4. 検証戦略（全バッチ共通）

1. **wire 回帰テスト**: 入力 camelCase → デシリアライズ、出力 snake_case → シリアライズを各ドメインで凍結（2.1 の不変条件をテスト化）。`NewSchedule` の既存テストを雛形に横展開。
2. **golden test（応答形状）**: Batch 3 で Node の実応答（ステータス・キー・`success`）をゴールデンとして固定。M-12 の恣意的差分をここで根絶。
3. **機械検査ゲート**: `clippy --workspace --all-targets`（警告ゼロ維持）／`cargo test --workspace`（現状 90 passed を回帰させない）／`cargo deny check`／`cargo fmt --check`。Batch 7 で CI に組み込む。
4. **絶対制約の維持**: 本番コードに `unwrap`/`expect`/`panic` を新規混入させない（`cfg(test)` 限定緩和を維持）。網羅 match（`_ =>` 禁止）を崩さない。
5. **敵対的再レビュー**: HIGH 修正（Batch 1〜3）完了後、修正差分に対する敵対的レビューを 1 周（過去の T1 fan-out と同運用）。

---

## 5. 却下／据え置きの明示（誤修正の予防）

- **view DTO への camelCase 付与は却下**（2.1）。フロント受信型が snake_case のため、足すと全ドメインで表示が壊れる。レビューの「出力 DTO は camelCase」記述に引きずられないこと。
- **応答形状の「クリーン化」は移行期は却下**（M-12＝Node 厳密一致）。カットオーバー後に別途検討。
- **M-7〜M-11 の完全 parity は今回 deferred**（fail-closed で汚染だけ止める）。ただし deferred は必ず lib.rs 冒頭で「未実装＝安全に拒否」と明記し、「未実装＝無音劣化」を残さない。**例外: M-6 は fail-closed でなく parity 実装に変更**（Batch 6 表参照。400 拒否がユーザー可視リグレッションになり、コストも連鎖削除と大差ないため）。

---

## 6. 参照

- レビュー本体: [review-2026-07-06-phase1.md](review-2026-07-06-phase1.md)
- 設計基準: [PLAN.md](PLAN.md)（§5.6/§5.7 fatality、§6.4 CSRF/body、§6.5 静的配信、§6.6 セキュリティヘッダ）
- 上位決定: [00-decisions.md](00-decisions.md)
- 移行前提: [parts/06-migration-workflow.md](parts/06-migration-workflow.md)（§11.3 共有 Redis、§11.4 SQLite 移行ハザード）
