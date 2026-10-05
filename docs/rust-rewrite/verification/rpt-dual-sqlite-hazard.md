All facts are now verified against primary sources and the actual codebase. I have everything needed to write the definitive report. Key finding: the stated premise "Node has no busy_timeout" is **incorrect** — better-sqlite3 defaults to 5000ms, and yuuka relies on that default. This materially changes the risk assessment. Also confirmed: synapse opens **read-only** with `SQLITE_OPEN_READ_ONLY`, so it is not even a potential writer.

Let me compose the final verification report.

---

# 検証レポート: Node(better-sqlite3) ↔ Rust(rusqlite/sqlx) 同一SQLite WAL 同時アクセスの安全性

対象: yuuka バックエンドの Node/TS → Rust ストラングラーフィグ移行における「同一 WAL DB への複数プロセス同時アクセス」ハザード。全て 2026年時点の一次情報(sqlite.org 公式ドキュメント / nginx.org 公式 / 各ライブラリ公式)で裏取り済み。推測箇所は明示。

---

## 0. 前提の重大訂正(コードで実測。確信度: 高)

調査の過程で、既知前提の一つが**事実と異なる**ことが判明した。これは (1)(2) の結論を根本的に変えるため最初に訂正する。

- 前提「Node(better-sqlite3)側に busy_timeout 未設定」は**誤り**。
- 実コード `src/db/database.ts:16` は `new Database(config.dbPath)` を `timeout` オプション無しで呼んでいる。
- better-sqlite3 公式 API ドキュメントは `options.timeout` を「ロックされた DB でクエリ実行時に `SQLITE_BUSY` を投げるまで待つミリ秒数(**default: 5000**)」と定義している。
- つまり **Node 側は既定で `sqlite3_busy_timeout(5000)` 相当が有効**。「Node が待たず即 500 を返す」という CHECKPOINT ドキュメント (`docs/rust-rewrite-plan-CHECKPOINT.md:6180`) の危険シナリオの前提は成立していない。
- なお `synchronous` は未設定なので WAL 既定の `NORMAL`(WAL では耐障害性上ほぼ問題なし)。

もう一点、`yuuka-synapse` の実コード (`src/rust_synapse/src/storage.rs:31-39`) を確認: `Connection::open_with_flags(..., SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_URI)` で開いており、**writer では構造的にあり得ない**(読み取り専用)。加えて `busy_timeout(3000)` を設定済み。よって「Rust writer が既に稼働している precedent」ではなく「**Rust read-only reader の稼働 precedent**」が正確。これは (2) の評価に直結する。

関連ファイル(絶対パス):
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/db/database.ts`
- `/home/suki/web/kawaii-music.moe/apps/yuuka/src/rust_synapse/src/storage.rs`

---

## 1. SQLite 公式のクロスプロセス並行アクセス仕様(確信度: 高)

### 1.1 単一writer・複数reader 原則
公式 [Write-Ahead Logging](https://sqlite.org/wal.html) の逐語:
> "Because writers do nothing that would interfere with the actions of readers, writers and readers can run at the same time. However, since there is only one WAL file, **there can only be one writer at a time**."

WAL の本質は「reader が writer をブロックせず、writer が reader をブロックしない」が「**writer は同時に1つだけ**」。これはスレッドではなく**プロセス横断で**成立する(次項のロック機構による)。

### 1.2 ロック機構(POSIX file lock / -wal / -shm)
- [File Locking And Concurrency](https://sqlite.org/lockingv3.html): "SQLite uses **POSIX advisory locks** to implement locking on Unix." ロック状態は SHARED / RESERVED / PENDING / EXCLUSIVE。SHARED は複数プロセスが同時保持可、RESERVED/EXCLUSIVE は排他。ロック取得失敗時は `SQLITE_BUSY` を返す。
- WAL では加えて `-shm`(共有メモリ / wal-index)と `-wal` ファイルを**全プロセスが共有**。公式 wal.html 逐語:
> "**All processes using a database must be on the same host computer**; WAL does not work over a network filesystem. This is because WAL requires all processes to share a small amount of memory... the use of shared memory means that all readers must exist on the same machine."

### 1.3 「同一ホスト・別プロセスの Node と Rust 同時アクセスは公式に安全か」
**安全(公式仕様上サポート)**。SQLite の並行制御は「同一ホスト上の複数プロセスが POSIX advisory lock + 共有 -shm で協調する」ことを設計目標としている。使用ライブラリ(better-sqlite3 / rusqlite / sqlx)は同一の SQLite C ライブラリ(amalgamation)を各々リンクしており、**同一のロックプロトコルを喋る**。したがって Node と Rust が別プロセスで同一 DB を開くこと自体は、単一プロセス複数コネクションと同じ扱いになる。**writer が同時に2つ以上にならない限り、データ整合性は公式に保証される**。

### 1.4 SQLITE_BUSY と busy_timeout の挙動
- `SQLITE_BUSY` は WAL でも発生しうる。wal.html 逐語: "there are some obscure cases where a query against a WAL-mode database can return `SQLITE_BUSY`, so applications should be prepared for that happenstance."
- [sqlite3_busy_timeout](https://sqlite.org/c3ref/busy_timeout.html): busy handler が最大 `ms` ミリ秒スリープしながらリトライし、超過後に `SQLITE_BUSY` を返す。**busy_timeout は各コネクション(=各プロセス)ごとに独立設定**。
- **最重要の落とし穴 — デッドロック回避で busy handler がスキップされる**([busy_handler](https://sqlite.org/c3ref/busy_handler.html) 逐語):
> "If SQLite determines that invoking the busy handler could result in a **deadlock**, it will go ahead and **return `SQLITE_BUSY`** to the application **instead of invoking the busy handler**."

これは「両コネクションが**DEFERRED トランザクションで読み取り開始 → 両方が書き込みへアップグレード**」で起きる。片方が先に write を握ると、もう片方のアップグレードは busy_timeout に関係なく**即 `SQLITE_BUSY`**([Bert Hubert の検証](https://berthub.eu/articles/posts/a-brief-post-on-sqlite3-database-locked-despite-timeout/) 逐語):
> "When the transaction on the left wanted to upgrade itself to a read-write transaction, SQLite could not allow this... An upgrade cannot be delayed. **The fix: If you know you are going to write in a transaction, start it with a write, or use BEGIN IMMEDIATE.**"

なお better-sqlite3 の `db.transaction()` は既定で `BEGIN DEFERRED` を発行する(=アップグレードデッドロックの当事者になりうる)。yuuka は `personaRepo/userRepo/todoRepo/plannedPaymentRepo` 等で `db.transaction()` を多用しており、**移行期に Rust 側 writer が存在すると、この即-BUSY が現実化する**。

### 1.5 WAL チェックポイント競合
- 自動チェックポイントは既定で WAL が1000ページに達した COMMIT 時、方式は **PASSIVE**(wal.html 逐語: "All checkpoints initiated by ... the automatic checkpoint mechanism are PASSIVE.")。PASSIVE は reader/writer を待たず、終わらなければ未完のまま戻る(WAL は縮まない)。
- **長時間 read トランザクションがチェックポイントを妨げる**(wal.html): チェックポインタは「現在の reader の end mark を越えるページ」で停止せねばならず、"a long-running read transaction can prevent a checkpointer from making progress"。overlapping reader が常在すると WAL が無制限に肥大化(**checkpoint starvation**)。
- [フォーラム(drh 回答)](https://sqlite.org/forum/info/7da967e0141c7a1466755f8659f7cb5e38ddbdb9aec8c78df5cb0fea22f75cf6): 妨げるのは「接続の存在」ではなく「**アクティブな read トランザクション / 未 reset の prepared statement**」。回避は "reader gaps"(誰も読んでいない瞬間)を作ること。TRUNCATE/RESTART は WAL を実際に縮められるが reader をブロックしうる。
- yuuka への含意: **Rust reader(synapse や新規参照系)が長寿命の prepared statement を握りっぱなしにすると、Node writer のチェックポイントが進まず WAL が肥大する**。Rust 側は各クエリ後に statement を確実に finalize/reset すること。

### 1.6 落とし穴サマリ
| 落とし穴 | 根拠 | yuuka での該当 |
|---|---|---|
| NFS等ネットワークFS禁止 | wal.html / lockingv3.html | 同一ホスト運用なら無害。**Docker で DB を volume 共有する場合、両コンテナが同一ノードの同一 bind mount を見ること必須**(ネットワークvolume不可) |
| -shm/-wal 全プロセス共有 | wal.html | 同一パスを開けば自動。パス相違に注意 |
| DEFERRED→write アップグレードで即BUSY | busy_handler.html / Bert Hubert | `db.transaction()` 多用箇所。**BEGIN IMMEDIATE 化が必要** |
| 長寿命 reader が checkpoint を阻害 | wal.html / forum | Rust reader の statement 管理 |
| busy_timeout は各プロセス独立 | busy_timeout.html | 両側で明示設定が必須 |

---

## 2. 移行期間中の安全パターン(確信度: 高)

### 2.1 結論: 「単一writer原則」を最優先で守る設計にせよ
公式仕様が保証するのは「writer は同時に1つ」まで。**両プロセスから任意タイミングで書くと、1.4 のアップグレード即-BUSY が原理的に不可避**(busy_timeout では救えない)。したがって:

**推奨(最も安全): 書き込みは常に単一プロセスに集約する。**
- Phase を通じて **Node が全書き込みを担い、Rust は read-only**。カットオーバー時に「Node の書き込みを停止 → Rust に書き込み権を移譲」を一度だけ行う(瞬間的な writer 切替)。
- これは synapse の既存設計(read-only)と完全に一致し、precedent として実証済み。横断テーブル(`message_logs` / `tool_outcomes` / `synapses` / `system_settings`)が「ドメイン単位で片側に分割」できない問題(CHECKPOINT:6180 が指摘)も、この方式なら発生しない。

### 2.2 両writer を許容せざるを得ない場合の必須条件
どうしても移行期に両側書き込みが必要なら、以下**全て**を満たすこと(それでも即-BUSY のリスクは残るため非推奨):
1. **両プロセスに busy_timeout を明示**(Node: `new Database(path,{timeout:5000})` か `db.pragma("busy_timeout=5000")`、Rust rusqlite: `conn.busy_timeout(...)` / sqlx: `SqliteConnectOptions::busy_timeout`)。
2. **全書き込みトランザクションを `BEGIN IMMEDIATE` で開始**(DEFERRED 禁止)。better-sqlite3 は `db.transaction(fn).immediate(...)`(要 v11 系の immediate API)または手動 `BEGIN IMMEDIATE`。これで 1.4 の即-BUSY を回避し、待機可能な通常 BUSY に格下げできる。
3. **アプリ層で `SQLITE_BUSY` リトライ**(指数バックオフ + ジッタ)を両側に実装。busy_timeout を超える競合や、handler スキップの残存ケースに備える。
4. 書き込みを**短く保つ**(長大トランザクション禁止)、reader の statement を確実に reset。

### 2.3 synapse の busy_timeout(3000) 読み取り実績の評価
- **評価: 「両writer 安全」の実証にはならない。ただし「同一ホスト複数プロセスで同一 WAL を安全に共有できる」ことの有力な実証**。
- synapse は **read-only** で開いているため writer 競合を一切起こさない。よってこの precedent が示すのは「Rust(rusqlite) と Node(better-sqlite3) が同一 DB を並行して開き、POSIX lock/-shm 協調が正常動作し、busy_timeout でロック競合を吸収できる」という点まで。
- busy_timeout(3000) の値自体は妥当だが、Node 側既定の 5000 と**不一致**。運用上は**両側を同一値(推奨 5000ms)に揃える**のが望ましい(片側だけ短いと、そちらが先に諦める)。
- 重要: synapse の実績を「Rust writer も安全」と**過大解釈しないこと**。writer 追加は 1.4 の質的に異なるハザードを持ち込む。

---

## 3. nginx リバースプロキシでのストラングラー(確信度: 高)

### 3.1 URLパス単位の振り分け(location + upstream)
定石は upstream ブロックで新旧2系統を定義し、`location` で URI パスごとに `proxy_pass`。yuuka は既に per-location upstream 構成なので、新 Rust 系統を upstream 追加し、移行済みパスの `location` だけ差し替える。例:
```nginx
upstream yuuka_node { server 127.0.0.1:3000; }
upstream yuuka_rust { server 127.0.0.1:3001; }

server {
    # 移行済みルートのみ Rust へ
    location /api/todos/ { proxy_pass http://yuuka_rust; }
    # 残り全部は従来 Node(フォールバック)
    location / { proxy_pass http://yuuka_node; }
}
```
段階シフトを細かくやるなら `split_clients`(%配分カナリア)や `map` でのルーティングも使える。`X-Served-By` 等のヘッダを付けてどちらが応答したか可観測にするのが実務定石。`proxy_set_header Host/X-Real-IP/X-Forwarded-For` は忘れず設定。**注意**: `proxy_pass` の末尾スラッシュ有無で URI 書換挙動が変わる(nginx の既知の罠)ので、パス prefix を保つ場合はスラッシュ規約を新旧で一致させること。

### 3.2 共有 Redis セッションの両バックエンド検証(確信度: 高)
- yuuka のセッションは「**CSPRNG の不透明トークンを共有 Redis にハッシュ保存**」(署名 Cookie ではない)。この方式なら**署名鍵の共有は不要**。両バックエンドは受け取った `__Host-yuuka-session` Cookie 値をハッシュして Redis を参照するだけで検証が成立する。Rust 側は Node と**同一のハッシュアルゴリズム・同一の Redis キー命名規約**を実装すれば良い(鍵配布・JWT 検証鍵同期の類は一切不要)。
- Cookie 属性 `__Host-yuuka-session (SameSite=Lax, HttpOnly, Secure, Path=/)` は**発行側の実装**であり、両バックエンドが同一ドメイン・同一パス(`Path=/`)配下で提供される限り、nginx がどちらへ振っても同一 Cookie が送られる。`__Host-` prefix は Secure + Path=/ + Domain 無し を要求するので、両系統とも HTTPS 終端配下(nginx が TLS 終端)であることを確認。
- 結論: **Redis 参照でセッション検証が両側で成立する。署名鍵共有は不要**(前提の確認事項に対して肯定)。移行時は「トークンのハッシュ方式と Redis スキーマの完全一致」だけを担保すればよい。

### 3.3 WebSocket(/ws/chat)の proxy 設定(確信度: 高)
nginx 公式 [WebSocket proxying](https://nginx.org/en/docs/http/websocket.html) に厳密準拠:
```nginx
map $http_upgrade $connection_upgrade { default upgrade; '' close; }

location /ws/chat {
    proxy_pass http://yuuka_node;          # 移行後は yuuka_rust
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection $connection_upgrade;
    proxy_set_header Host $host;
    proxy_read_timeout 3600s;              # 既定60sだとアイドルで切断される
}
```
- 公式逐語: Upgrade/Connection は hop-by-hop なので明示転送が必須。`map` で Upgrade 有無に応じ Connection を `upgrade`/`close` に切替えるのが推奨形。
- タイムアウト: 公式逐語 "By default, the connection will be closed if the proxied server does not transmit any data within **60 seconds**." → `/ws/chat` のような長寿命接続は `proxy_read_timeout` を延長するか、**バックエンドが定期的に WebSocket ping frame を送る**(公式推奨)。yuuka のチャットは長時間アイドルしうるので ping 送出を強く推奨。
- nginx 1.29.7 以降は `proxy_http_version 1.1;` 不要(既定 1.1)。それ以前を使うなら明示のこと。
- **Cookie セッションからの getSession(noServer + handleUpgrade)**: アップグレード時の HTTP リクエストにも Cookie は載るため、nginx が `/ws/chat` を該当バックエンドへ振れば、そのバックエンドが Cookie → Redis 参照でセッション解決できる。3.2 と同じ理屈で新旧どちらでも成立。**移行時は WS エンドポイントを新旧同時に割らない**(1つの upstream に固定)のが安全。

---

## 4. 段階移行の完了判定・ロールバック・カナリア(確信度: 中〜高)

「実務の定石」部分は確立された運用知識(確信度: 中)、SQLite/nginx に固有の判定条件(確信度: 高)。

### 4.1 カナリア
- nginx `split_clients` で新 Rust へ 1% → 5% → 25% → 100% と段階配分。ステートフルな WS(`/ws/chat`)は**パーセント配分に不向き**(接続の粘着性が要る)なので、REST から先に移し、WS は最後にエンドポイント単位で一括切替する。
- 各段階で `X-Served-By` とエラー率・p95 レイテンシ・**SQLITE_BUSY 発生数**を監視。BUSY の急増は writer 競合(§2.1 違反)の兆候。

### 4.2 完了判定
1. 対象ルートの Rust 応答が旧 Node と機能等価(shadow/diff テストで検証)。
2. **書き込み権の移譲が完了し、Node writer が完全停止**(§2.1 の単一writer切替が済んでいる)。
3. `SQLITE_BUSY` / DB エラー率がベースライン以下。
4. WAL サイズが健全(checkpoint starvation の兆候なし)。
5. セッション(Redis)整合が新旧で一致。

### 4.3 ロールバック
- nginx の `location`/`upstream`/`split_clients` を旧 Node に戻すだけで**トラフィックは即ロールバック可**(リロードはゼロダウンタイム)。
- **ただしデータ層のロールバックは非対称**。カットオーバーで Rust に書き込み権を移した後は、Rust が書いたデータを Node のスキーマ/コードが読める保証が要る。**移行期はスキーマを不変に保ち**、Rust migration の所有権を Node と衝突させない(どちらが `migrations.ts` 相当を実行するか一元化する — CHECKPOINT が未設計と指摘した点)。ロールバック窓を持つなら、その間**スキーマ変更を凍結**するのが安全。
- Cookie/Redis セッションは新旧共通なので、トラフィックを戻してもユーザーは再ログイン不要(§3.2 の利点)。

### 4.4 実務上の最小変更(今すぐ・移行と独立に可能)
- `src/db/database.ts` に `db.pragma("busy_timeout = 5000")` を**明示追加**して既定依存をやめる(可読性・意図明確化。挙動は既に 5000 なので低リスク)。
- 書き込みトランザクションの `BEGIN IMMEDIATE` 化を検討(現状 Node 単独 writer なら必須ではないが、Rust writer 追加前の前提整備)。

---

## 総括

| 項目 | 結論 | 確信度 |
|---|---|---|
| (1) 同一ホスト・別プロセス Node+Rust の同時アクセス | **公式に安全**。POSIX lock + 共有 -wal/-shm で協調。writer は同時1つが絶対条件。NFS 不可。長寿命 reader が checkpoint を阻害 | 高 |
| (2) 安全設計 | **書き込みを単一プロセスに集約(Node 全書き→カットオーバーで Rust へ一度だけ移譲)が最安全**。両writer 許容なら busy_timeout 明示 + BEGIN IMMEDIATE + アプリ層リトライが全て必須。synapse(3000) は read-only 実績で「複数プロセス共存」は実証するが「両writer 安全」は実証しない | 高 |
| (3) nginx ストラングラー | location+upstream 振り分け、Redis 不透明トークンで**署名鍵共有不要**、WebSocket は Upgrade/Connection 転送 + proxy_read_timeout 延長 + ping、いずれも公式準拠で成立 | 高 |
| (4) 完了判定/ロールバック/カナリア | トラフィックは nginx で即ロールバック可だが**データ層は非対称**。移行期スキーマ凍結・migration 所有権一元化・単一writer 切替が鍵 | 中〜高 |

**最重要の訂正**: 既知前提「Node に busy_timeout 未設定 → 即 BUSY」は**誤り**。実コードは better-sqlite3 既定の 5000ms を使用。真のハザードは「Node が待たない」ことではなく「**両側が書くと DEFERRED トランザクションのアップグレード時に busy_timeout を無視して即 SQLITE_BUSY が返る**」点であり、これは busy_timeout では解決不能で、**単一writer 集約か BEGIN IMMEDIATE でしか防げない**。

**主要根拠(一次情報)**:
- [SQLite WAL](https://sqlite.org/wal.html) / [File Locking](https://sqlite.org/lockingv3.html) / [busy_timeout](https://sqlite.org/c3ref/busy_timeout.html) / [busy_handler(デッドロック時 handler スキップ)](https://sqlite.org/c3ref/busy_handler.html) / [checkpoint starvation フォーラム(drh)](https://sqlite.org/forum/info/7da967e0141c7a1466755f8659f7cb5e38ddbdb9aec8c78df5cb0fea22f75cf6)
- [nginx WebSocket proxying(公式)](https://nginx.org/en/docs/http/websocket.html)
- [better-sqlite3 API(timeout default 5000)](https://github.com/WiseLibs/better-sqlite3/blob/master/docs/api.md) / [sqlx SqliteConnectOptions](https://docs.rs/sqlx/latest/sqlx/sqlite/struct.SqliteConnectOptions.html)
- [Bert Hubert: DEFERRED アップグレードで timeout が効かない検証](https://berthub.eu/articles/posts/a-brief-post-on-sqlite3-database-locked-despite-timeout/)
- 実コード: `/home/suki/web/kawaii-music.moe/apps/yuuka/src/db/database.ts`, `/home/suki/web/kawaii-music.moe/apps/yuuka/src/rust_synapse/src/storage.rs`