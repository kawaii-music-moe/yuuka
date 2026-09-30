# 全体アーキテクチャ一新案（v3 提案）— Rust シナプスエンジン × ハイブリッドLLM

> **文書種別:** アーキテクチャ一新案（提案 / RFC）。**R0/R1 実装済み** — Rust シナプスエンジン（`src/rust_synapse/`、統合方式A=Node 子プロセス）・stdio 改行 JSON プロトコル・read-only WAL 参照・RAM 索引・SQLite v10 を実装し、実装規範 [`architecture/architecture_v2.md`](../architecture/architecture_v2.md) §13 へ昇格済み（機能フラグ既定 OFF）。R2/R3/R4 は未実装。埋め込みは現状ハッシュ n-gram（`Embedder` trait＋cargo feature `onnx` を将来の ONNX 換装点として用意）。本書はシステム実装面の原典として維持する。
> **位置づけ:** [`synapse_cognitive_architecture.md`](synapse_cognitive_architecture.md)（設計思想＝why/what）の**システム実装面（how/topology）**の対。旧 Node 実装時点の規範 [`architecture/architecture_v2.md`](../architecture/architecture_v2.md)（schema v9 時点。履歴資料）の**将来後継（v3）案**として書かれた提案書。**Node バックエンドは撤去済み**で、シナプスエンジンは `crates/yuuka-synapse` としてインプロセスのライブラリに統合されているため、本書の 3 プロセス構成（Node / Rust シナプス / ローカル SLM）は現行構成と異なる。**矛盾する場合は現行コードが正**。
> **⚠️ 旧 Node 実装時点の記述を含みます:** 本文が参照する `src/*.ts`・`src/rust_synapse/`・`src/rust_crawler/` 等は Node 実装（[#68](https://github.com/kawaii-music-moe/yuuka/pull/68) で撤去済み）のファイルで、現行リポジトリには存在しません（リンク切れを含む）。撤去前のソースは `git show 390df39^:<path>` で参照できます。詳細は [docs/index.md](../index.md)。
> **本書の主眼:** シナプス関連（埋め込み・ベクトル索引・2-Hop連想・統計）を **Rust の独立プロセス**として切り出し、Node(V8) ヒープのメモリトラブル（ヒープ肥大・GC停止・~4GB上限）を構造的に回避する。

---

## 0. 要約（TL;DR）

現行 Yuuka は **Node 単体オーケストレーション**で、1回の Gemini 呼び出しが「Function Call ループ＋最終生成＋ペルソナ」を兼ねる。本案は責務を3プロセスへ分離する。

1. **Node オーケストレータ（既存）** — Discord I/O・HTTP ダッシュボード・ツール dispatch・**Gemini 本推論＋ペルソナ提示**・**SQLite の唯一の書き手**。接着剤に専念。
2. **Rust シナプスエンジン（新規・常駐）** — 埋め込み生成(ONNX)・ベクトル索引(USearch, RAM)・2-Hop 連想・勝率集計・コンテキスト合成。**記憶の重い処理を V8 ヒープの外**へ。
3. **ローカル SLM ランタイム（新規・常駐）** — llama.cpp/vLLM。制御プレーン（ルーティング・抽出・分類）。

設計の核は **「SQLite を正（OLTP, 書き手は Node のみ）/ Rust は RAM 上の高速索引を保持し SQLite を読み取り専用で参照」**。索引は SQLite から再構築可能なため、**索引喪失・エンジン crash は非致命**（Node は現行＝直近履歴注入へ自動デグレード）。Rust 採用の狙いは速度より先に**決定的なメモリ管理と障害分離**である。

---

## 1. 現行アーキテクチャ（as-is）

```
Discord ◄──► [ Node 単体 (bot.ts / gemini.ts / server.ts) ] ◄──► Web Dashboard
                    │  1コールで FC ループ＋最終生成＋ペルソナ
                    ├──► Gemini API
                    ├──► SQLite (better-sqlite3)  ← 履歴の正
                    ├──► Redis (session / 会話キャッシュ / fc_history)
                    └──► Rust クローラー (子プロセス, stdin/stdout JSON)  ← browser不変層
```

- 文脈＝直近15件の生履歴注入（[`messageLogRepo.ts`](../../src/db/messageLogRepo.ts) `CONTEXT_LIMIT=15`）。
- 既に **Rust 子プロセス＋改行区切りJSON** の前例がある（[`browserService.ts`](../../src/services/browserService.ts) `spawn(binPath,["daemon"])` ＋ readline）。本案はこの grain を踏襲する。
- 限界は synapse 設計書 §1 を参照（受動想起・経験の揮発・ツール肥大・トークン肥大）。

---

## 2. 一新の動機

| 課題 | 現行 | 一新後 |
|---|---|---|
| 文脈の質 | 直近15件の生注入 | 2-Hop で合成した極小コンテキスト |
| 記憶の重い処理 | （存在しないが）やるなら V8 ヒープ上 | **Rust プロセスの RAM（off-heap, GC無し）** |
| 経験の活用 | fc_history が2hで揮発 | SQLite 永続＋勝率統計＋投機的実行 |
| ツール選択 | 全ツール毎回提示 | トピック→候補ツール絞り込み |
| コスト/遅延 | 全部 Gemini | ハイブリッド（easy はローカル吸収） |
| 障害時 | — | エンジン落ちても現行挙動へデグレード |

**「Rust にする理由」は §6 に集約。** 端的には、ベクトル索引と埋め込みを JS に持たせると V8 ヒープ肥大・GC スパイク・~4GB 上限に当たる。Rust の独立アドレス空間に置けば、これらは原理的に消える。

---

## 3. 一新後の全体構成（to-be）

```
                          ┌─────────────────────────────────┐
        Discord  ◄───────►│   Node オーケストレータ（既存）    │◄───► Web Dashboard (HTTP)
                          │  bot.ts / gemini.ts / server.ts  │
                          │  ・Discord I/O・ツール dispatch   │
                          │  ・Gemini 本推論＋ペルソナ提示    │
                          │  ・SQLite 唯一の書き手           │
                          └──┬──────────┬──────────┬─────────┘
        改行区切りJSON(stdio) │          │ HTTP     │ 既存(stdio)
                             ▼          ▼          ▼
            ┌───────────────────────┐ ┌──────────┐ ┌─────────────────┐
            │ Rust シナプスエンジン   │ │ローカルSLM │ │ Rust クローラー   │
            │ (常駐・新規)           │ │llama.cpp  │ │ (既存・不変)      │
            │ ・ONNX 埋め込み(ort)   │ │ /vLLM     │ └─────────────────┘
            │ ・USearch 索引(RAM)    │ │OpenAI互換 │
            │ ・2-Hop 連想/勝率集計   │ └──────────┘
            │ ・SQLite 読取専用(WAL)  │
            └──────────┬────────────┘
                       │ read-only (WAL)
        ┌──────────────▼───────────────┐        ┌───────────────┐
        │ SQLite（正/OLTP, WALモード）   │        │     Redis      │
        │ message_logs / synapses(+BLOB) │        │ session/会話   │
        │ tool_outcomes / topic_tool_stats│       │ キャッシュ/種   │
        │   ▲ writes = Node のみ          │       └───────────────┘
        └─────────────────────────────────┘
```

**プロセス分担の原則**
- **書き込み一元化**：SQLite への書き込みは Node だけ。Rust は **WAL モードで読み取り専用**接続（SQLite は WAL で「複数リーダー＋単一ライター」を許容）。二重ライター問題を回避。
- **状態の置き場所**：クエリ時のベクトル KNN は **Rust の RAM 上 USearch 索引**で完結し、SQLite に触らない。耐久コピー（embedding BLOB）は SQLite に Node が保存。
- **SLM は別ランタイム**：シナプスエンジンに SLM を同居させない（責務分離・障害分離）。Node が HTTP で SLM を、stdio で Rust を、それぞれ叩く。

---

## 4. リクエストのライフサイクル（制御フロー）

```
1. Discord メッセージ受信 (Node/bot.ts)
2. Node → SLM(HTTP): 意図分類・難易度判定（ルーター, §3 hybrid）
        └ easy 判定かつ高勝率ツール確定 → 投機的プレフェッチ起動（読み取り系のみ）
3. Node → Rust(stdio): assemble(user_id, bot_id, query)
        Rust: ① query 埋め込み → ② USearch KNN（1st Hop）
              → ③ topic_id 集合 → SQLite読取で topic_tool_stats / 前提シナプス JOIN（2nd Hop）
              → ④ 極小コンテキスト(JSON, 数百tok)＋候補ツールを返す
4. Node: 制御プレーン
        easy → SLM で {t,cmd,args}（ペルソナ無し・JSONハーネス, §8.4）
        hard → Gemini で同上
5. Node: ツール dispatch（既存 functions/*。認証情報は browserService へ直渡し＝不変）
6. Node: 提示プレーン → Gemini で自由文＋ペルソナ適用 (user_id,bot_id) で最終応答生成
7. Node → SQLite: 会話ログ・新規シナプス・tool_outcomes を書き込み
        Node → Rust(stdio): upsert(synapse_id, vector) で RAM 索引へ追加
8. バックグラウンド: Rust が topic_tool_stats を再集計（勝率更新）
```

各 Hop の件数・`min_samples` は保守的に（synapse 設計書 §6 の過剰検索の罠）。

---

## 5. Rust シナプスエンジンの設計

### 5.1 責務（owns）
- **埋め込み生成**：`bge-micro` 級を **INT8 量子化**でローカル駆動（API 遅延ゼロ）。
- **ベクトル索引**：USearch（RAM, mmap 可）。1st Hop の KNN。
- **連想合成**：2nd Hop（topic→勝率ツール・前提背景）の SQLite 読取 JOIN。
- **統計**：`topic_tool_stats` のバックグラウンド再集計。
- **トークンシェイピング**：LLM へ渡す極小コンテキストの整形（キー短縮・順序＝思考が先, §8.2/§8.4）。

### 5.2 推奨クレート
| 用途 | クレート | 備考 |
|---|---|---|
| 非同期ランタイム | `tokio` | 既存 crawler と同じ |
| 埋め込み(ONNX) | `ort`（ONNX Runtime）or `candle` | INT8 量子化モデル |
| ベクトル ANN | `usearch` | RAM/mmap、軽量、ID マッピング |
| SQLite 読取 | `rusqlite`（WAL, read-only open） | 書き込みはしない |
| プロトコル | `serde` / `serde_json` | 改行区切り JSON |

### 5.3 統合方式（決定ポイント）
| 方式 | 内容 | 評価 |
|---|---|---|
| **A. Node 子プロセス（推奨）** | crawler と同様 `spawn` ＋ stdin/stdout 改行JSON。Node がライフサイクル/再起動を管理 | 既存前例あり・実装最小・**独立アドレス空間でメモリ隔離は達成**。デフォルト推奨 |
| B. 独立 systemd デーモン＋unix socket | 独自 `MemoryMax` 上限・独立再起動・別ホスト配置可 | 隔離は最強だが運用増。エンジン単独でメモリ上限を切りたい/別マシンに出す時に採用 |
| C. napi-rs in-process アドオン | 同一プロセス。IPC ゼロで最低遅延 | V8 ヒープ外には置けるが crash/leak が Node を道連れにし得る。**メモリトラブル回避の趣旨と相反**するため非推奨 |

→ **A を既定**（crawler の grain に一致、子プロセス＝別アドレス空間で V8 ヒープから完全分離）。エンジンに独立メモリ上限が要る・水平分割したくなった時点で **B** へ。

### 5.4 ★メモリ安全設計（本案の主目的）
Rust 化に加え、以下で「メモリ関連トラブル」を能動的に潰す。
- **off-heap & GC 無し**：埋め込み・索引は Rust のヒープ/mmap に置き、V8 の GC 停止と ~4GB 上限から完全に解放。
- **有界構造**：ユーザー毎に索引サイズ上限。超過は **`decay_score` 駆動で明示退避（eviction）**。無制限増加を作らない。
- **mmap 索引**：USearch を mmap で持ち、RSS をページキャッシュに委ねて常駐メモリを平準化。
- **事前確保バッファ／アリーナ**：ホットパスのアロケーションを抑え、断片化とスパイクを回避。
- **バックプレッシャ**：stdio プロトコルに in-flight 上限。詰まったら受理を絞る（OOM より遅延を選ぶ）。
- **再構築可能＝非致命**：RAM 索引は SQLite の embedding BLOB から復元可能。**crash したら Node は索引無し（現行＝直近履歴注入）へデグレード**して応答継続、エンジン再起動後に索引を再ロード。
- **アドレス空間隔離**：子プロセス（方式A）でリーク/肥大が起きても Node 本体は無事。最悪は子の再起動で回復。

---

## 6. なぜ Rust か（設計判断の根拠）

| トラブル源（Node でやる場合） | Rust シナプスエンジンでの解 |
|---|---|
| 大量 embedding/索引を JS 配列で保持 → V8 ヒープ肥大・~4GB 上限 | off-heap（Rust ヒープ/mmap）に保持 |
| GC によるレイテンシスパイク | GC 無し・決定的 free |
| JS 製ベクトルライブラリのリーク | 所有権・借用で leak を型で抑止、有界構造で上限 |
| ネイティブアドオンの ABI 不整合 | 子プロセス＋JSON 境界（ABI 非依存） |
| ブロッキングな埋め込み計算が event loop を止める | 別プロセスで隔離、Node の event loop を解放 |

既存に **Rust crawler の運用実績**があり、ビルド・配布・通信の型が既にある（§8）。学習コスト・運用追加が最小で済むのも採用理由。

---

## 7. データ所有と整合性

- **正は SQLite**：`synapses`（content/topic/embedding BLOB）, `tool_outcomes`, `topic_tool_stats`。書き手は **Node のみ**。
- **Rust は派生・再構築可能**：RAM の USearch 索引は SQLite から再生成可能な**キャッシュ**扱い。喪失は性能劣化であって data loss ではない。
- **埋め込みのバージョニング**：モデル更新時は `embedding_model_version` 列で世代管理。世代不一致は Node が再埋め込みを発注（Rust `embed()` → BLOB 再書き込み）。
- **データ分離は不変条件として Rust 側でも強制**：すべての API 呼び出しは `user_id`（汎用モードは `bot_id × user_id` / `bot_id × guild_id`）を伴い、Rust の SQLite 読取は必ずこのスコープを WHERE に入れる（architecture_v2 §0.1 をプロセス跨ぎで継承）。
- **WAL 整合**：Node＝writer、Rust＝reader。Rust は若干古いスナップショットを読み得るが、想起用途では許容（新規シナプスは upsert で即 RAM 索引へ反映）。

---

## 8. ビルド・デプロイ

- **ビルド**：[`package.json`](../../package.json) の `build` は既に crawler を `cargo build --release` する。**第2クレート `src/rust_synapse/`** を追加し、同様に `dist/bin/yuuka-synapse` へ配置。
- **配布**：crawler と同じ探索パス方式（`target/release` → `dist/bin`）。
- **プロセス起動**：方式A（既定）では Node が起動時に子として spawn し、`exit` 監視で再起動（browserService の crawler 監視を踏襲）。
- **systemd**：既定では既存 [`yuuka.service`](../../yuuka.service) のまま（子プロセスは Node 配下）。方式B採用時のみ `yuuka-synapse.service` を追加し `MemoryMax=` でメモリ上限を設定。
- **ローカル SLM**：別ユニット（例 `yuuka-slm.service`）で llama.cpp/vLLM を OpenAI 互換サーバとして常駐。Node から `LOCAL_SLM_ENDPOINT` で接続。

---

## 9. 不変条件の保全（architecture_v2 §0 を跨プロセスで維持）

1. **データ分離**：Rust 含む全コンポーネントが `user_id`/複合スコープ必須（§7）。
2. **ブラウザ操作層は不変**：crawler・`browserService`・`browserFunctions` は触らない。シナプスエンジンは別系統。
3. **認証情報を渡さない**：シナプス抽出・埋め込み対象から **PW/トークン等を除外**（`actionRecorder` の除外規約をエンジン側でも継承）。**秘匿値の embedding 生成を禁止**。復号値は従来どおり `browserService` へ直渡しのみ。
4. **スキーマ前方移行**：新テーブルは migrations の v10+ として冪等追加（破壊的再構築なし）。
5. **暗号は既存規約**：embedding BLOB 自体は秘匿値を含めない前提のため平文 BLOB で可。秘匿に該当し得るシナプスはそもそも作らない（3 と同根）。

---

## 10. 段階的移行（Strangler パターン）

ビッグバン置換をしない。各段は**単独で出荷可能**かつ**新コンポーネント OFF で現行挙動にフォールバック**できること。

| 段 | 追加プロセス | 内容 | フォールバック |
|---|---|---|---|
| **R0** | なし | `actionRecorder` を SQLite 永続化（揮発→蓄積）、§ベースライン計測 | 既存どおり |
| **R1** | Rust エンジン | 埋め込み＋USearch＋1st Hop。Node はコンテキストに「想起」を**追加注入**（現行15件は維持） | エンジン落ち＝想起無しで現行挙動 |
| **R2** | （同上） | `tool_outcomes`＋`topic_tool_stats`＋2nd Hop。候補ツール絞り込み | 統計欠如＝全ツール提示にデグレード |
| **R3** | ローカル SLM | ハイブリッド・ルーター導入（easy 吸収）、制御/提示の二段分離（§8.4） | SLM 落ち＝全部 Gemini |
| **R4** | — | 投機的プレフェッチ（読み取り系限定）＋プレフィックスキャッシュ最適化 | 機能フラグで無効化 |

各段着手時に該当部を architecture_v2 へ昇格し、schema v10+ を確定。

---

## 11. リスクと留意点

| リスク | 内容 | 緩和 |
|---|---|---|
| 多プロセス運用の複雑化 | プロセス3つ＋既存 crawler/Redis | 方式A（子プロセス）で管理点を Node に集約、デグレード設計で可用性確保 |
| stdio 遅延 | 子プロセス IPC のオーバーヘッド | バッチ/単一往復の assemble API、必要時に方式B(socket)/Cへ |
| ローカル SLM の資源 | RAM/VRAM 消費・品質下限 | 量子化モデル、誤エスカレーション率を監視、閾値で Gemini フォールバック |
| 索引と SQLite の乖離 | upsert 漏れ・世代不一致 | 索引は SQLite から再構築可能・起動時整合チェック・`embedding_model_version` |
| メモリ上限の責務 | 方式Aは Node と cgroup 共有 | 上限分離が要れば方式B＋`MemoryMax` |
| プライバシ/記憶汚染 | シナプスは個人情報・毒化 | スコープ強制（§7/9）、秘匿除外（§9.3）、記憶ガバナンス観点 |

---

## 12. オープンな決定事項（着手前に確定）

1. **統合方式 A/B**（子プロセス vs 独立デーモン）— 既定 A、メモリ上限分離が要れば B。
2. **埋め込みモデル**（bge-micro 等の具体選定・次元 D・量子化方式）。
3. **ローカル SLM の具体**（モデル・ランタイム・量子化・常駐ホスト）。
4. **assemble API の確定スキーマ**（入出力 JSON・件数・閾値）。
5. **schema v10 のテーブル定義**（synapse 設計書 §5 を migrations へ）。

---

## 付録. 関連文書
- 設計思想・研究裏付け：[`synapse_cognitive_architecture.md`](synapse_cognitive_architecture.md)
- 旧 Node 実装時点の規範（履歴資料。権威ではない）：[`architecture/architecture_v2.md`](../architecture/architecture_v2.md)
- 既存 Rust デーモン前例：[`src/services/browserService.ts`](../../src/services/browserService.ts) / [`src/rust_crawler/`](../../src/rust_crawler/)
