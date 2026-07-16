# nginx ストラングラー設定（Node→Rust 段階カットオーバー）

[`yuuka.conf`](yuuka.conf) は Node/TS→Rust 全面移行（`docs/rust-rewrite/PLAN.md` 第11部）の
**唯一の切替真実源**。旧 Node と新 Rust を別ポートで並走させ、`location` 単位でどちらへ流すかを
1 ファイルで制御する（アプリ層フィーチャフラグは作らない＝ロールバックを `nginx -s reload` 1 発に）。

## ポート

| upstream | プロセス | 待受 | 備考 |
|---|---|---|---|
| `yuuka_node` | 現行 Node/TS | `127.0.0.1:7854` | 既存 `PORT`（`config.yaml` / deploy 設定） |
| `yuuka_rust` | 新 Rust（`yuuka` バイナリ） | `127.0.0.1:7900` | Rust インスタンスは **`PORT=7900`** で起動する |

> Rust の `PORT` は Node と別値にする。`yuuka-core` の config は `yaml[KEY] ?? env[KEY]`（yaml 優先）
> のため、Rust 用に **`PORT: 7900` を書いた別 `config.yaml`** を渡すか、`PORT` を書かない config +
> `PORT=7900` 環境変数で起動する（yaml に PORT があると env は無視される点に注意）。

## 運用（ダイヤルの回し方）

1. あるフェーズ（PLAN §11.1 の F→A→B→C→D→E→G→H）の **3 層ゲート**を緑にする:
   - 型: `cargo build/clippy -D/deny check` + ts-rs 生成物 drift 無し
   - テスト: `cargo test --workspace` + Node/Rust ゴールデン差分一致
   - verify: `deploy/instance.sh` の CSP/immutable/`/api/me` 検証 + フェーズ固有不変条件
2. `yuuka.conf` の該当 `location` を uncomment（`yuuka_node` → `yuuka_rust`）。**1 群ずつ**。
3. `nginx -t && nginx -s reload`（無停止）。カナリアで `X-Served-By` / エラー率 / p95 /
   **`SQLITE_BUSY` 発生数**を監視（BUSY 急増 = writer 競合 = §11.4 違反の兆候）。
4. 異常時は該当行を `yuuka_node` に戻して `nginx -s reload` で即ロールバック（トラフィックは即時。
   ただしデータ層は非対称なので**ロールバック窓の間はスキーマ凍結** = Rust は DDL 不発行）。

## 不変条件（絶対に守る）

- **単一 writer 集約（§11.4・最重要）**: あるドメインの書き込みは Node/Rust の一方のみ。書き込み系
  ルートを `split_clients` で % 分割しない（両側 DEFERRED→write で即 `SQLITE_BUSY`・busy_timeout で
  解決不能）。エンドポイント単位で一括切替する。
- **同一オリジン（§11.3）**: `__Host-yuuka-session` は host-only。両 upstream は同一 `server_name`・
  同一 TLS 終端の背後に置く。セッションは共有 Redis の不透明トークン参照なので署名鍵共有は不要。
- **WebSocket は最後（Phase H）**: `/ws/chat` はターンキュー・1接続1Bot束縛・添付上限の状態機械を
  持つため、gemini/bot/services が揃うまで Node 固定。切替時も % 分割せずエンドポイント一括。
- **レガシー削除は検証後の最終ステップ**: 旧 Node の `location`・旧コードは検証完了まで残す
  （早まるとロールバックがオブジェクト復元＋データ再生になる）。
