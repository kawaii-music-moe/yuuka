# 第11〜12部 — 段階移行ロードマップ ＋ サブエージェント並行実装ワークフロー

> 本パートは [00-decisions.md](../00-decisions.md)（確定 ADR）に厳密整合する。技術選定は再決定しない。
> 一次ソース照合は以下を相対リンク引用する:
> [rpt-nginx-session-strangler](../verification/rpt-nginx-session-strangler.md) /
> [rpt-dual-sqlite-hazard](../verification/rpt-dual-sqlite-hazard.md) /
> [rpt-migrations-sqlx-refinery](../verification/rpt-migrations-sqlx-refinery.md) /
> [rpt-axum-web-runtime](../verification/rpt-axum-web-runtime.md) /
> [rpt-resilience-tokio-backon-recloser](../verification/rpt-resilience-tokio-backon-recloser.md)
>
> **本パートの位置づけ（最重要級）**: 第1〜10部が「何を作るか」を確定したのに対し、本パートは「**壊さずにどう置き換えるか（第11部）**」と「**それをどう高速に実装するか（第12部）**」を定める。前者は本番データ喪失・断続ログアウト・二重書き込みという不可逆事故の防波堤であり、後者は前者を並行実装で成立させる運用手順である。

---

## 第11部. 段階移行ロードマップ（ストラングラーフィグ）

### 11.0 前提事実（実ファイル確認済み・再検証不要）

- `src/db/database.ts` の PRAGMA は現状 `journal_mode=WAL` と `foreign_keys=ON` の 2 つのみ（`busy_timeout` は明示未設定だが、**better-sqlite3 の既定 5000ms が有効**＝[rpt-dual-sqlite-hazard §0](../verification/rpt-dual-sqlite-hazard.md) の重大訂正）。
- `src/db/migrations.ts` の `SCHEMA_VERSION = "17"`。`system_settings` 不在時に `"1"` を返し**レガシー全 DROP 分岐**（`:880-894`）へ落ちる破壊経路が実在（[rpt-migrations-sqlx-refinery](../verification/rpt-migrations-sqlx-refinery.md)）。
- `src/rust_synapse/src/storage.rs` は `SQLITE_OPEN_READ_ONLY` で開き `busy_timeout(3000)` 済み＝**read-only リーダーの稼働実績**（writer 実績ではない。[rpt-dual-sqlite-hazard §0](../verification/rpt-dual-sqlite-hazard.md)）。
- `nginx/nginx.conf` は**空ディレクトリ**（Docker volume マウント先の器のみ、config 未コミット）。→ リバースプロキシ設定は**本移行の成果物として新規に書く**。
- Cargo は `rust_crawler` / `rust_synapse` の **2 クレートが独立**。ルート `Cargo.toml`（workspace）は存在しない。

### 11.1 移行順序の依存グラフ — foundation を最初に凍結

移行順序は「下流が上流に依存しない DAG」を厳守する。**foundation クレート群を最初に凍結**しないと、後続の並行実装（第12部）で全エージェントが型で衝突する。

```
                    ┌─────────────────────────────────────────────┐
  Phase F           │ yuuka-core   : error / config / secret / UserId │
  (単独先行・凍結)   │ yuuka-db     : rusqlite pool / writer actor    │
                    │ yuuka-types  : wire DTO + ts-rs 生成基盤        │
                    └───────────────┬─────────────────────────────┘
                                    │ (全クレートがここに依存)
        ┌───────────────┬───────────┼───────────┬───────────────┐
   Phase A          Phase B      Phase C     Phase D          Phase E
   認証/session     静的配信+     ドメイン    gemini/         discord bot
   (auth)           単純GET      route群      functions       (twilight)
        │            (me/status)  (並行可)     (tool trait)         │
        └────────────┴───────────┴───────────┴───────────┬───────┘
                                                          │
                                            Phase G  services/cron
                                                          │
                                            Phase H  daemon 吸収
                                                     (crawler/synapse) → /ws/chat 最終カットオーバー
```

移行の実行順（＝nginx で Rust へ回すルートを増やす順）と各フェーズの完了判定:

| # | フェーズ | 対象 | nginx で Rust へ回すもの | 前提 | 完了判定（全て満たして初めて切替） |
|---|---|---|---|---|---|
| **F** | foundation | error/config/secret/UserId/db pool/wire 型の 3 クレート | （まだ何も回さない・Rust は起動だけ） | — | `cargo build/clippy -D/deny check` + ts-rs 生成が空でも通る。型が **FROZEN** |
| **A** | 認証/セッション | `resolveRequestUser`, Redis session, desktop token, device flow | `/api/login /logout /me /api/auth/device/*` | F + **Redis 共有** | 下記 3 層ゲート + Node/Rust が同一 Redis セッションを相互に読める検証 |
| **B** | 静的+単純ルート | `serveStaticFile`, `/api/status`, `/api/setup/status` | 静的 `/`, `/assets/`, `/api/status` | A | 3 層ゲート + CSP/immutable/SPA fallback パリティ |
| **C** | 各ルート群 | todo→finance→schedule→timeline→reminder→personal→credential→playbook→persona | ドメイン単位で `^~ /api/tasks/` 等を順次 | A,B | ドメインごとに 3 層ゲート + カナリア SLO |
| **D** | gemini/functions | tool registry, planner, recall 注入 | （WS/bot 経路が使う内部層。HTTP は増えない） | C の repo | registry 重複名テスト + planner responseSchema テスト |
| **E** | discord bot | twilight マルチ接続, interaction | Discord 側は Node bot 停止で**排他カットオーバー** | D | bot supervisor 稼働 + Discord 側で Node/Rust 二重起動しない |
| **G** | services/cron | reminder/report/briefing/backup/… | cron は HTTP 非公開。Node cron 停止→Rust cron 起動 | C,D | cron パリティ + reminder 起動時即時実行で取りこぼし復帰 |
| **H** | daemon 吸収 | crawler/synapse を workspace 化・supervisor 監督 | **`/ws/chat` を Rust へ最終カットオーバー** | E,G | 全ゲート + Node 全停止 + 整数連番マイグレーションランナー導入 |

**各フェーズの完了判定＝3 層ゲート**（[rpt-axum-web-runtime](../verification/rpt-axum-web-runtime.md)・[rpt-migrations-sqlx-refinery](../verification/rpt-migrations-sqlx-refinery.md) と整合）:

1. **型ゲート**: `cargo build --workspace` + `cargo clippy --workspace --all-targets --all-features -- -D warnings` + `cargo deny check`（anyhow/eyre/color-eyre 混入をブロック）+ `git diff --exit-code`（ts-rs 生成物 drift 無し）。
2. **テストゲート（機能パリティ）**: `cargo test --workspace`。各ルート群は**ゴールデン差分テスト** —— 同一リクエストを Node(:7854) と Rust(:7900) の両方へ投げ、レスポンス JSON（success エンベロープ・キー名・snake/camel）とステータスが一致することを assert。
3. **verify ゲート（カナリア SLO）**: `deploy/instance.sh` の verify を流用（CSP `script-src 'self'`・hashed asset immutable・`/api/me` 200 を curl 検証。サーバ実装非依存でカットオーバー後の回帰検出にそのまま効く）。加えてフェーズ固有の不変条件（例: todo なら CSRF 403・10MB 413・`stripProtoKeys`・user_id スコープ越境が 403）と、カナリア期間中の**エラー率／p95 レイテンシ／`SQLITE_BUSY` 発生数**が SLO 内であること。

**なぜこの順か**:
- **A（認証）を最初**に回すのは、以降の全 user-scoped ルートが `resolveRequestUser` に依存し、Node と Rust が**同一 Redis セッションを読める**ことを検証してからでないと、C 以降を Rust に回した瞬間にログインが割れるため。
- **D（gemini）を C の後・E の前**に置くのは、tool ハンドラが C で移した repo 層（`todoRepo` 等の Rust 版）を呼び、bot（E）は D の tool registry に依存するため。
- **H を最後**にするのは、`/ws/chat` がターンキュー・1接続1Bot束縛・添付上限という状態機械を持ち、gemini(D)+bot(E)+services(G) が揃わないと等価な応答を返せないため。WS を最後にカットオーバーするのが最も回帰リスクが低い。

### 11.2 nginx strangler — 切替の唯一の真実源

旧 Node は `127.0.0.1:7854`、新 Rust は `127.0.0.1:7900`（別ポート）で並走させ、nginx が `location` 単位でどちらへ流すかを制御する。これがストラングラーの「絞め殺しダイヤル」になる。設計判断として**振り分け粒度は「プレフィックス `location` ブロック」に固定し、アプリ層フィーチャフラグや動的ルータは作らない**（切替状態を nginx 1 ファイルに集中させ、ロールバックを `nginx -s reload` 1 発にするため）。

```nginx
# /home/suki/web/kawaii-music.moe/nginx/nginx.conf/yuuka.conf （新規・本移行の成果物）
upstream yuuka_node { server 127.0.0.1:7854; keepalive 32; }
upstream yuuka_rust { server 127.0.0.1:7900; keepalive 32; }

# WebSocket の hop-by-hop Upgrade/Connection を条件転送（http ブロックに 1 つ）
map $http_upgrade $connection_upgrade { default upgrade; '' close; }

server {
    listen 443 ssl http2;
    server_name yuuka.kawaii-music.moe;   # 両 upstream は同一オリジン（__Host- Cookie 前提）

    # nginx は X-Forwarded-Proto を必ず立てる（Rust の HTTPS 判定誤作動防止）
    proxy_set_header Host              $host;
    proxy_set_header X-Real-IP         $remote_addr;
    proxy_set_header X-Forwarded-For   $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto https;

    # --- 移行済みルート群を Rust へ（フェーズ進行で 1 行ずつ増やす。URI 無し＝パス保存） ---
    location = /api/me      { proxy_pass http://yuuka_rust; }   # 末尾 URI 無し=書換なし
    location = /api/login   { proxy_pass http://yuuka_rust; }
    location ^~ /api/tasks/ { proxy_pass http://yuuka_rust; }
    # location ^~ /api/expenses/ { proxy_pass http://yuuka_rust; }  # 次フェーズで解禁

    # --- WebSocket は接続が長命なので明示。移行末期（Phase H）まで Node 固定 ---
    location = /ws/chat {
        proxy_pass http://yuuka_node;          # Phase H で yuuka_rust へ最終切替
        # proxy_http_version 1.1;              # nginx 1.29.7 未満なら必須
        proxy_set_header Upgrade    $http_upgrade;
        proxy_set_header Connection $connection_upgrade;
        proxy_read_timeout 3600s;              # 既定 60s だとアイドルで切断→ping 併用
    }

    # --- 既定フォールバック: 未指定は全て Node（フェイルセーフ） ---
    location / { proxy_pass http://yuuka_node; }
}
```

**`proxy_pass` の末尾 URI 規約（静かに壊れる罠）**（[rpt-nginx-session-strangler A-1](../verification/rpt-nginx-session-strangler.md) 逐語確認）:
- **URI を付けない**（`proxy_pass http://yuuka_rust;`）→ リクエスト URI がそのまま渡り、両バックエンドが同一パス空間を共有できる。**本移行はこれを標準**とする。
- URI を付ける（`.../;` 等）と `location` にマッチした部分が置換される（`/api/tasks/x` → `/x`）。付ける／付けないでルーティングが静かに壊れるため、末尾スラッシュ規約を新旧で一致させる。
- 正規表現 `location` と named location では `proxy_pass` に URI を付けてはならない。

**WebSocket**（[rpt-nginx-session-strangler A-2](../verification/rpt-nginx-session-strangler.md) 逐語）: `Upgrade`/`Connection` は hop-by-hop でデフォルト転送されないため明示必須。`map $http_upgrade $connection_upgrade` で非 WS リクエストの keep-alive を壊さない。既定 60s タイムアウトで無通信接続が切れるので `proxy_read_timeout 3600s` 延長 + **バックエンドから定期 ping フレーム**（nginx 公式推奨）を併用。yuuka チャットは長時間アイドルしうるため ping 送出を強く推奨。

**Cookie/CSRF 上の不変条件**: 両 upstream は同一 `server_name`・同一 TLS 終端＝**同一オリジン**なので、`__Host-yuuka-session` Cookie・CORS・`Sec-Fetch-Site` 判定は upstream をまたいでも壊れない。ただし後述 11.3 のセッション共有が絶対条件。

### 11.3 共有 Redis 不透明トークン＝署名鍵共有不要

yuuka のセッションは「**CSPRNG の不透明トークンを共有 Redis にハッシュ保存**」する方式（署名 Cookie / JWT ではない）。トークン自体には意味がなくストア参照するだけなので、**共有署名鍵／HMAC シークレットは不要**（[rpt-nginx-session-strangler B-2](../verification/rpt-nginx-session-strangler.md)。JWT 方式なら検証鍵共有が必須になる決定的な差異）。両バックエンドは受け取った Cookie 値をハッシュ化して Redis を参照するだけで検証が成立する。

**両バックエンドで完全一致させるべき 4 項目**（一致必須。ズレると「片方が書いたセッションを他方が読めない」障害）:
1. **ハッシュ方式**: `sha256(token)` を同一エンコーディング（hex か base64、大小文字含め）で算出。
2. **Redis キー書式**: `session:{sha256(token)}` を一字一句同一に。**サービス別プレフィックスを付けない**（共有が目的なので同一キースペース）。
3. **値のシリアライズ**: JSON フィールド名・型・日時表現を共通化。Rust(serde)⇄Node で往復可能なスキーマにし、**契約テストで両方向のデシリアライズを検証**。
4. **TTL セマンティクス**: 期限秒数・スライディング更新（アクセス毎の `EXPIRE` リセット）の有無を揃える。片方だけスライディングだと片側で早期失効する。

**`__Host-` Cookie ゆえ同一オリジン必須**（[rpt-nginx-session-strangler B-1](../verification/rpt-nginx-session-strangler.md)、RFC 6265bis の規範的 MUST）: `__Host-` プレフィックスは (1)Secure (2)HTTPS オリジン発行 (3)**Domain 属性なし**（host-only） (4)Path=/ の**すべて**を強制し、いずれか欠けるとブラウザが Cookie を丸ごと破棄する。**Domain 禁止＝サブドメイン共有不可**なので、旧 Node と新 Rust は必ず同一プロキシ配下の同一オリジンで動く（これが 11.2 のパス分岐リバースプロキシを必須にする理由）。開発環境が HTTP だと `__Host-` が拒否されるため、ローカルも HTTPS 化するか環境ごとに Cookie 名を切り替える。

**インメモリフォールバックの gotcha**（[rpt-nginx-session-strangler B-3](../verification/rpt-nginx-session-strangler.md)）: Redis 断時のインメモリフォールバックは**プロセスローカル**で他バックエンドから不可視。フォールバック中に nginx が旧↔新をパスで分岐すると、同一ユーザーの連続リクエストが別プロセスへ渡り、ローカル Map のセッションが見えず**断続的ログアウト**（再現困難なフラッピング障害）が起きる。→ フォールバック発動を必ずメトリクス／アラート化し、Redis を HA 化してフォールバック依存を最小化する。フォールバック中は片系に固定（sticky）してセッションの見え方の分裂を防ぐ。

### 11.4 SQLite 移行ハザードの核心【最重要・強調】

同一ホスト・別プロセスの Node+Rust 同時アクセスは**公式に安全**（POSIX advisory lock + 共有 `-wal`/`-shm`、ただし **NFS 等ネットワーク FS 不可**）。使用ライブラリ（better-sqlite3 / rusqlite）は同一 SQLite C ライブラリをリンクし同一ロックプロトコルを喋るため、別プロセスで同一 DB を開くこと自体は単一プロセス複数コネクションと同じ扱いになる（[rpt-dual-sqlite-hazard §1](../verification/rpt-dual-sqlite-hazard.md)）。

**ただし writer は同時に 1 つが絶対条件**（WAL 公式: "there can only be one writer at a time"）。そして最大の落とし穴は次の一点である:

> **両 writer が DEFERRED トランザクションで読み取り開始 → 両方が書き込みへアップグレード**すると、片方が先に write を握った時点で、もう片方のアップグレードは **busy_timeout を無視して即 `SQLITE_BUSY`** を返す（[rpt-dual-sqlite-hazard §1.4](../verification/rpt-dual-sqlite-hazard.md)。SQLite はデッドロックを検知すると busy handler を呼ばず即 BUSY を返す）。**これは busy_timeout では解決不能**であり、`busy_timeout` 値をいくら上げても防げない。better-sqlite3 の `db.transaction()` は既定で `BEGIN DEFERRED` を発行し、yuuka は各 repo で多用しているため、移行期に Rust 側 writer が存在すればこの即-BUSY が現実化する。

**→ 移行期の設計原則（最優先・強く推奨）**: **移行期は「Node が全書き込み・Rust は read-only」を貫き、カットオーバー時に一度だけ writer を Rust へ移譲する＝単一 writer 集約**。これは synapse の read-only 実績と完全に整合する。フェーズ設計として「あるドメインの**書き込み**は Node か Rust の**どちらか一方のみ**」を不変条件にし、nginx でルート群を Rust に回した瞬間そのドメインの書き込みは Rust writer actor へ一本化される（例: `todos` を Rust に回したら Node の todo route は死んでいる＝nginx が Node へ流さない）。横断テーブル（`message_logs` / `tool_outcomes` / `synapses` / `system_settings`）が「ドメイン単位で片側に分割」できない問題も、単一 writer 集約なら発生しない。

**Rust 側 DB 層の必須設定**（[rpt-dual-sqlite-hazard §2](../verification/rpt-dual-sqlite-hazard.md) と [00-decisions.md](../00-decisions.md) の PRAGMA 決定）:

```rust
// yuuka-db/src/pool.rs
fn open_conn(path: &Path) -> Result<Connection, DbError> {
    let c = Connection::open(path)?;
    c.pragma_update(None, "journal_mode", "WAL")?;   // 既存と一致
    c.pragma_update(None, "foreign_keys", "ON")?;    // 既存と一致
    c.busy_timeout(Duration::from_millis(5000))?;    // Node 既定 5000 と揃える
    c.pragma_update(None, "synchronous", "NORMAL")?; // WAL では NORMAL が定石
    Ok(c)
}
```

- **読み取り**: `deadpool-sqlite` / `r2d2_sqlite` の read pool（N 本、WAL は複数リーダー並行可）→ `spawn_blocking` でマルチコア並列読み。
- **書き込み**: **専用の単一書き込みコネクションを 1 本の `tokio::task`（writer actor）に閉じ込め、`mpsc` でコマンドを直列受信**。これで「並行書き込み→即-BUSY」を構造的に排除しつつ、マルチコアは読み側で活かす（HTTP は読みが支配的）。

**両 writer をやむなく許容する場合の必須条件**（それでも非推奨。以下**全て**を課す。[rpt-dual-sqlite-hazard §2.2](../verification/rpt-dual-sqlite-hazard.md)）:
1. **両プロセスに busy_timeout を明示**（推奨 5000ms に統一。片側だけ短いとそちらが先に諦める）。
2. **全書き込みトランザクションを `BEGIN IMMEDIATE` で開始**（DEFERRED 禁止）。これで §11.4 の即-BUSY を、待機可能な通常 BUSY に格下げできる。
3. **アプリ層で `SQLITE_BUSY` リトライ**（`backon` の指数バックオフ + ジッタ）を両側に実装。
4. 書き込みを短く保ち、reader の statement を確実に finalize/reset する。

**長寿命 reader の checkpoint 阻害注意**（[rpt-dual-sqlite-hazard §1.5](../verification/rpt-dual-sqlite-hazard.md)）: 妨げるのは「接続の存在」ではなく「**アクティブな read トランザクション／未 reset の prepared statement**」。Rust reader（synapse や新規参照系）が長寿命の statement を握りっぱなしにすると、writer のチェックポイントが進まず WAL が無制限に肥大化（checkpoint starvation）する。Rust 側は各クエリ後に statement を確実に finalize/reset すること。Docker で DB を volume 共有する場合、両コンテナが**同一ノードの同一 bind mount**を見ること必須（ネットワーク volume 不可）。

**schema_version の破壊経路封じ込め**（[rpt-migrations-sqlx-refinery §3](../verification/rpt-migrations-sqlx-refinery.md)）:
- **移行期間中、スキーマ移行の権限は Node に一本化**。Rust の `yuuka-db` は起動時に `SELECT value FROM system_settings WHERE key='schema_version'` を読み、**期待固定値 `"17"` と一致しなければ `DbError::Migration` で fail-fast**（回復不能＝起動時の致命に該当）。**Rust は DDL を一切発行しない**＝レガシー DROP 分岐も `mcp_servers` DROP 分岐も**そもそも Rust に移植しない**。これで「Rust が `system_settings` を引き継がず初期化→本番テーブル全 DROP」の事故が構造的に不可能になる。
- Node 全停止後（Phase H 完了時）に、Rust 側へ **refinery による整数連番・前方専用・非破壊のマイグレーションランナー**を導入し、現行 v17 スキーマを `CREATE TABLE IF NOT EXISTS` の冪等 baseline (V1) として凍結。`SCHEMA_VERSION="17"` → `schema_version=17`（整数）へ引き継ぐ 1 回きりの橋渡し migration を書く。以降は Rust が権限を持つ。

### 11.5 カナリア・シャドウ・ロールバック

- **カナリア**（[rpt-nginx-session-strangler C-3](../verification/rpt-nginx-session-strangler.md)）: `split_clients` で新 Rust へ 1%→5%→25%→100% と段階配分。各段階で `X-Served-By` ヘッダ・エラー率・p95・**`SQLITE_BUSY` 発生数**を監視（BUSY 急増は writer 競合＝§11.4 違反の兆候）。**REST を先に、WS を最後に**移す。ステートフルな `/ws/chat` は接続の粘着性が要るため**パーセント配分せず、エンドポイント単位で一括切替**する。

  ```nginx
  # カナリア段階配分の例（split_clients。REST ルートのみに適用）
  split_clients "${remote_addr}${http_user_agent}" $tasks_backend {
      5%   yuuka_rust;   # まず 5%
      *    yuuka_node;
  }
  location ^~ /api/tasks/ { proxy_pass http://$tasks_backend; }  # 変数使用時は resolver 注意
  ```

- **shadow/mirror は読み取り専用／冪等のみ**（[rpt-nginx-session-strangler C-3](../verification/rpt-nginx-session-strangler.md)）: nginx `ngx_http_mirror_module` で実トラフィックを複製し新 Rust へ送り応答を破棄できるが、**ミラー先が共有 DB／共有 Redis に書くと二重書き込み・セッション汚染・重複通知**を起こす。したがってシャドウは**読み取り専用／冪等なエンドポイント限定**、または Rust 側を dry-run（書き込み無効）／分離ストアにする。§11.3 の共有 Redis はまさに副作用対象なので、書き込み経路のシャドウは行わない。

  ```nginx
  location ^~ /api/tasks/ {
      mirror /shadow_rust;            # 応答は無視（GET 系のみ安全）
      proxy_pass http://yuuka_node;
  }
  location = /shadow_rust { internal; proxy_pass http://yuuka_rust$request_uri; }
  ```

- **レガシー削除は検証後の最終ステップ**（[rpt-nginx-session-strangler C-4](../verification/rpt-nginx-session-strangler.md)、Microsoft 公式が明言）: 旧経路（旧 Node の `location`・旧テーブル・旧コード）は**検証完了まで削除しない**。早まると、ロールバックが「オブジェクト復元＋データ再生」になり工数・リスクが激増する。
- **旧経路 warm 維持で即ロールバック**: 旧 Node の該当 `location` を残し、`location` 1 行を `yuuka_node` に戻して `nginx -s reload`（無停止・無ビルド）で即座に旧経路へ戻せる状態を保つ。トラフィックは即ロールバック可だが**データ層のロールバックは非対称**なので、**ロールバック窓の間はスキーマを凍結**する（§11.4 の schema 固定＝Rust は DDL 不発行が効き、Rust が書いた行も Node が読める互換を保証）。Cookie/Redis セッションは新旧共通なので、トラフィックを戻してもユーザーは再ログイン不要。

---

## 第12部. 複数サブエージェント並行実装ワークフローの分解 ＋ オーケストレーション

第11部の実装を「複数サブエージェントのワークフロー」で高速並行実行するための実務ガイド。**本パート末尾の「実地教訓」は、この計画そのものの作成過程で得た一次経験であり、必ず織り込む。**

### 12.1 依存グラフに基づく並行／逐次の切り分け

foundation（core/db/型契約）は**逐次先行して凍結**し、その後 feature クレート（web/discord/gemini/tools/services）を並行投入する。どのクレートが独立かを表に示す:

| 並行性 | クレート／モジュール | 根拠（なぜその区分か） |
|---|---|---|
| **逐次（最初に単独・凍結）** | `yuuka-core`(error/config/secret/UserId), `yuuka-db`(pool/writer actor/UserScope trait), `yuuka-types`(wire DTO + ts-rs) | 全員が依存。ここが動くまで他は着手不可。**凍結後は触らせない** |
| **並行 T1**（互いに独立・別テーブル別 route） | todo / finance / schedule / timeline / reminder / personal / credential / playbook / persona（**9 ドメイン**） | 各ドメインは別テーブル・別 route ファイルで共有は foundation のみ。1 エージェントが「Repo impl + wire DTO + Tool impl + route handler + golden test」を縦に持つ。**最大の並行度** |
| **並行 T2**（横断だが T1 と独立） | auth/session, 静的配信+admin, webhook, MCP proxy | セッション・静的・監査は T1 のドメイン repo に依存しない |
| **逐次（T1 の後・合流点）** | gemini orchestrator, functions registry マージ, MCP dynamic | T1 の Repo/Tool 実装を集約。registry は全 Tool を集める合流点（重複名を `Result` で検知） |
| **逐次（gemini の後）** | discord bot(twilight), WS chat, componentInteraction 統合 | tool registry と gemini エントリに依存 |
| **並行 T3**（bot と独立） | services/cron(reminder/report/briefing/backup/…), notifier | Repo(T1)+notifier に依存するが gemini 内部には依存しない。bot と並行可 |
| **逐次（最後）** | daemon 吸収（crawler/synapse を workspace 化） | 既存 Rust クレートを workspace に統合。別 OS プロセスのまま supervisor 監督（障害分離維持） |

### 12.2 contract-first — トレイト／型を先に確定してから並行投入

並行化の前提は「**モジュール間の境界（trait/型）を Phase 0 で凍結し、以後変更しない**」こと。凍結対象は次の 5 つで、これを確定してからエージェントを並行投入すれば型の衝突が起きない:

1. **`yuuka-core::error` の層別エラー enum**（`ConfigError`/`DbError`/`AuthError`/`GeminiError`/… と最上位 `AppError`）— 全クレートが `#[from]` で依存。`AppError::status()` は網羅 `match`（`_ =>` 禁止）でバリアント追加漏れをコンパイルエラー化。
2. **`UserId` newtype** — 全リポジトリ署名に通し、データ分離キー欠落を型で防ぐ。
3. **`yuuka-types` の wire DTO + `Envelope<T>`** — HTTP とフロントの契約。機密列を DTO のフィールドに**持たせない**ことで漏洩を型的に不可能化。
4. **`ToolProvider` / `Tool` トレイト** — gemini/functions と全ドメインモジュールの契約:

   ```rust
   #[async_trait::async_trait]
   pub trait Tool: Send + Sync {
       fn declaration(&self) -> FunctionDeclaration;
       async fn call(&self, ctx: &ToolContext, args: serde_json::Value)
           -> Result<ToolOutcome, ToolError>;   // 文字列規約を型へ昇格
   }
   ```
5. **Repo トレイト（`UserScope` 束縛）** — user_id スコープを型で強制:

   ```rust
   pub struct UserScope { user_id: UserId }        // 構築時に user_id を必ず束縛
   impl TodoRepo {
       async fn list(&self, scope: &UserScope) -> Result<Vec<Todo>, DbError>;  // user_id 無しクエリを型で禁止
   }
   pub trait CronScan { async fn overdue_across_users(&self) -> Result<Vec<Todo>, DbError>; }  // 横断は別トレイトに隔離
   ```

これで過去に起きたクロステナント事故（`owner_id` 欠落）が型レベルで再発不能になる。

### 12.3 worktree 分離での衝突回避

ファイルを並行変更するエージェントは `git worktree` で分離する（`isolation: worktree`）。**別クレート＝別ファイルなら分離不要**だが、共有ファイル（`Cargo.toml` の members・foundation）を触るなら分離する。ユーザーの auto-memory の落とし穴を厳守する:

- **`worktree-node-modules-symlink-pitfall`**: worktree に node_modules symlink + `git add -A` で本体破壊。Rust クレートは node_modules 不要。`git add -A` は**使わず**常に明示パス `git add crates/todo/`。ts-rs 生成物は `frontend/src/lib/api/generated/` の生成専用ディレクトリで frontend ビルド worktree と分離。
- **`develop-no-history-delete`**: 作業の引越しは move（develop から削除＋force-push）。copy は重複で merge 衝突するため避ける。
- **workspace 化と Cargo.lock**: ルート `Cargo.toml`（現状不在）を `apps/yuuka/Cargo.toml` に Phase 0 で新設し `[workspace] members = ["crates/*", "src/rust_crawler", "src/rust_synapse"]`。**`Cargo.lock` はルート 1 つ**に集約し全 worktree で共有。**`CARGO_TARGET_DIR` は worktree 間で共有しない**（各 worktree 独立の target。衝突ゼロ優先。ビルド高速化は sccache で）。1 クレート 1 worktree 1 エージェントで編集ファイルが物理的に交わらない状態を作る。
- **共有ファイルの編集を Phase 0 に隔離**: members 追加のような共有編集が並行フェーズに漏れると衝突する → Phase 0 で全クレートの空スケルトン（`lib.rs` に最小 stub とトレイト空実装）を先に生成・コミットし、以後エージェントは**自クレート内のファイルのみ**を編集する。

### 12.4 検証ゲート（per-module 完了判定）

各エージェントは自クレートについて以下を**マージ前に全通過**させる（ローカル worktree で完結）:

```
cargo build -p yuuka-todo
cargo clippy -p yuuka-todo --all-targets --all-features -- -D warnings  # unwrap/expect/panic/todo を deny
cargo test  -p yuuka-todo                          # unit + golden(Node 差分) + scope 越境テスト
cargo deny check                                   # anyhow/eyre/color-eyre 混入ブロック（workspace 全体）
git diff --exit-code frontend/.../generated/       # ts-rs drift（DTO 変更時のみ）
```

**クレート単位の `-p` 指定**が並行性の鍵 —— エージェントは他クレートの未完成を待たずに自分のゲートを回せる（foundation は Phase 0 で確定済みなので依存は満たされている）。CI は最後に `cargo test --workspace` で統合を 1 回検証する。

### 12.5 ワークフローのフェーズ設計案

`understand → contract 凍結 → 並行実装 → per-module 検証 → 統合` の 5 段構成。pipeline / parallel / loop-until-dry の使い分けを併記する:

```
Phase 0  契約凍結（単一エージェント・逐次・最重要）           ← pipeline の起点。ここは絶対に並行化しない
  - apps/yuuka/Cargo.toml workspace 新設、Cargo.lock 集約、clippy.toml / deny.toml / workspace lints 配置
  - crates/core（error 層別 enum + Fatality + config + secret + UserId）
  - crates/db（rusqlite pool + writer actor + UserScope trait + assert_schema_compatible）
  - crates/types（wire DTO + Envelope + ts-rs export + drift test）
  - Tool / Repo トレイト凍結、全クレートの空 stub コミット
  ゲート: cargo build/clippy -D/deny check、ts-rs 生成が空でも通る → 以後 型は FROZEN

Phase 1  並行ファンアウト（サブエージェント N 体・parallel）  ← worktree 分離で fan-out
  T1: [todo][finance][schedule][timeline][reminder][personal][credential][playbook][persona]
       各 = Repo impl + DTO + Tool impl + route handler + golden test（別 worktree・別クレート）
  T2: [auth/session][static+admin][webhook][mcp-proxy]（別 worktree）
  T3: 着手可（services/cron の repo 依存が T1 で埋まり次第）
  各エージェントのゲート: 12.4 の -p 単位チェック
  合流: 完了ドメインから nginx location を :7900 へ 1 群ずつ解禁（3 層ゲート → location 追加 → verify → 次へ）

Phase 2  合流（逐次・pipeline）                              ← 全 Tool を集める合流点
  gemini orchestrator + functions registry マージ（重複名を Result で検知）+ MCP dynamic
  ゲート: registry 起動時の重複名テスト、planner responseSchema テスト

Phase 3  bot + WS（逐次・supervisor 前提）
  twilight マルチ接続（per-bot アクター、トークン差替は close→spawn の直列プロトコル）
  WS chat（ターンキュー・1 接続 1Bot 束縛・添付上限）、各 bot タスクを supervisor 配下で catch + 指数バックオフ再起動
  カットオーバー: Node bot 停止 → Rust bot 起動（Discord 側は排他）

Phase 4  services/cron カットオーバー（T3 完成品を投入）
  Node cron 停止 → Rust cron 起動（reminder は起動時即時実行で取りこぼし復帰）

Phase 5  daemon 吸収 + 最終カットオーバー
  crawler/synapse を workspace member 化（既に Rust。別 OS プロセスのまま supervisor 監督で障害分離維持）
  /ws/chat を nginx で :7900 へ最終切替 → Node 全停止
  Rust 側に refinery 整数連番マイグレーションランナー導入、schema_version=17 橋渡し
```

**pipeline / parallel / loop-until-dry の使い分け**:
- **pipeline**（逐次）: Phase 0 → 1 → 2 → 3 → 4 → 5 の骨格。前段の成果物（凍結型・registry）が後段の入力になる依存があるため。
- **parallel**（fan-out）: Phase 1 の T1/T2/T3。境界が foundation の凍結型のみで、別クレート別 worktree なら物理的に交わらない。
- **loop-until-dry**（収束反復）: 各エージェント内の「実装 → 12.4 ゲート → clippy/test/deny 赤を潰す → 再実行」を green になるまで反復。フェーズ跨ぎの統合検証（`cargo test --workspace`）も、パリティ差分が枯れるまで反復する。

### 12.6 今回の実地教訓（本計画作成で得た一次経験・必ず適用）

> 本教訓は抽象論ではなく、**この移行計画そのものを複数エージェントで作成した過程**で実際に踏んだ事象に基づく。証跡: [design-checkpoint.md](../design-checkpoint.md) 冒頭は「ワークフロー wf_456f35ec-ec8 の**中断時スナップショット**。完了済み 22 エージェント分の生出力を保存。最終統合(Synthesize)は**未完了**。resume で完了可能」と記録している。

1. **バックグラウンド Workflow はセッション境界（ホストプロセス再起動）で "stopped" になり、最終統合が完走しないことがある。**
   → **journal + resumeFromRunId で再開可能に設計**する。かつ**各エージェント成果は即ファイルへ退避**する（transcript にしか無いと回収困難。実際 22/23 完了で中断し、生出力を design-checkpoint.md に退避していたからこそ resume 可能だった）。

2. **バックグラウンド Agent 呼び出しは完走・通知した（Workflow より堅牢だった）。**
   → 長時間ジョブは **Agent fan-out + ファイル退避**が安全。Workflow の自動オーケストレーションに全面依存せず、Agent 単位で成果を確定させながら進める。

3. **エージェントが最終メッセージに「他エージェント待ち」等のメタ文言を残すと、通知の result にレポート本体が入らない。**
   → 各エージェントに **「最終メッセージ＝成果物のみ、余談・進捗・メタ文言は禁止」** と明示指示し、**かつ Write でファイル出力**させる（result とファイルの二重化で回収漏れを防ぐ。本パートも「final text は Write 完了報告＋見出し一覧のみ」で運用している）。

4. **並行執筆は決定を先に凍結（ADR）してから投入すると齟齬が出ない。**
   → 本計画がその実例。[00-decisions.md](../00-decisions.md)（技術選定 ADR）を先に確定し、各執筆エージェントに「再決定禁止・厳密整合」を課したことで、15 領域の並行執筆でも矛盾が出なかった。実装フェーズも 12.2 の contract-first がこれに対応する。

5. **巨大成果は synthesis 1 エージェントに集約させず、パート分割 → 機械的結合が安定。**
   → design-checkpoint.md（約 58 万バイト・22 エージェント出力）を単一 synthesize で束ねようとして中断した。最終マスタープランは**パート分割**（本パートは第11〜12部）して各パートを独立 Write し、後で機械的に結合する方式が安定する。

### 12.7 supervisor 基盤（全 Phase 共通・自己復帰）

長命サービス（bot 各タスク・cron・daemon）は例外なく supervisor 配下に置く（[rpt-resilience-tokio-backon-recloser](../verification/rpt-resilience-tokio-backon-recloser.md)・[00-decisions.md](../00-decisions.md) の resilience 決定）。panic をタスク境界で隔離し指数バックオフで再起動する。`panic = "unwind"` を厳守（`abort` だと 1 タスク panic がプロセスを落とす）。`Transient`（即バックオフ再起動）と `Permanent`（上限到達で停止 or 管理 UI へアラート）を**別扱い**にし、恒久障害の無限スピンを避ける。外部依存（Gemini/Discord/Google/Redis/DB）の一時障害は型付き `Err(...Recoverable)` を返し呼び出し側が劣化縮退へ落とす。**起動時のシークレット不備のみ** fail-fast。Discord Gateway 再接続は twilight の Shard 内蔵 resume に委ね、汎用 supervisor で丸ごと再 spawn しない（二重管理回避。本当に死んだ場合のみ介入）。

---

## 本パートの結論（最重要 3 点）

1. **nginx location 単位の絞め殺し**（アプリ層フラグ不採用）で切替状態を 1 ファイルに集中させ、ロールバックを `nginx -s reload` 1 発にする。共有 Redis 不透明トークンで**署名鍵共有不要**、`__Host-` Cookie ゆえ**同一オリジン必須**、WS は `Upgrade`/`Connection` 転送 + `proxy_read_timeout 3600s` + ping。
2. **SQLite は「Node が全書き込み・Rust は read-only、カットオーバーで一度だけ writer 移譲」の単一 writer 集約が最優先**。両 writer の DEFERRED→write アップグレードは busy_timeout を無視した即 `SQLITE_BUSY` を招き解決不能。Rust は DDL 不発行（schema_version 固定値チェックのみ）でレガシー全 DROP 破壊経路を封じる。
3. **Phase 0 で error 層別 enum・UserId・wire DTO・Tool/Repo/UserScope トレイトを凍結（contract-first）**してから T1 の 9 ドメインをサブエージェント並行展開する。凍結型 + 1 クレート 1 worktree + `-p` 単位ゲート + `cargo deny` が衝突ゼロ・絶対制約準拠を機械保証する。**実地教訓**（journal+resume 設計、成果の即ファイル退避、最終メッセージ＝成果物のみ、ADR 先行凍結、パート分割→機械的結合）を必ず適用する。
