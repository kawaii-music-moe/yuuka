# シナプス駆動 認知アーキテクチャ 設計方針書

> **文書種別:** 設計方針書（提案 / RFC）。**Phase 0/1（R0/R1）実装済み** — schema v10 のシナプス記憶層・Rust シナプスエンジン・1st Hop 連想・経験統計の素地が実装され、実装規範 [`architecture/architecture_v2.md`](../architecture/architecture_v2.md) §13 へ昇格済み（R0/R1 コンポーネントは**常時有効**。エンジンバイナリ不在・未起動・タイムアウト時のみ現行挙動〔直近15件の生履歴注入〕へ自動デグレード）。Phase 2（2nd Hop 勝率提示）・Phase 3（ローカル SLM ハイブリッド）・Phase 4（投機/キャッシュ）は未実装。本書は設計思想の原典として維持する。
> **位置づけ:** 本書は設計思想の原典（why/what）。実装済み部分（R0/R1）の現行の実装は `crates/yuuka-synapse`（インプロセス）と `crates/yuuka-orchestrator` の `synapse_*.rs` で、**矛盾する場合は現行コードが正**。旧 Node 実装時点の規範である [`architecture/architecture_v2.md`](../architecture/architecture_v2.md) §13（schema v16）は履歴資料で、優先されない。未実装の Phase 2 以降は本書を指針とし、着手時は現行の Rust 実装・マイグレーション（`crates/yuuka-db/migrations/`）に合わせて具体化する。
> **⚠️ 旧 Node 実装時点の記述を含みます:** 本文が参照する `src/*.ts`・`src/rust_synapse/` 等は Node 実装（[#68](https://github.com/kawaii-music-moe/yuuka/pull/68) で撤去済み）のファイルで、現行リポジトリには存在しません（リンク切れを含む）。撤去前のソースは `git show 390df39^:<path>` で参照できます。詳細は [docs/index.md](../index.md)。
> **対象読者:** 設計判断を行う開発者・実装エージェント。
> **決定済みの前提（本書の3つの軸）:**
> 1. **ハイブリッドLLM構成** — 重い推論は Gemini、ルーティング/前処理/分類はローカル軽量LLM。
> 2. **記憶エンジンは比較のうえ推奨を提示** — DuckDB と SQLite+sqlite-vec を Yuuka の制約下で評価（§4）。
> 3. **設計方針＋研究裏付けに集中** — 実装マッピングは概略（§11 ロードマップ）に留める。

---

## 0. 要約（TL;DR）

軽量LLMに「長い履歴テキスト」を丸ごと渡す現行方式は、文脈の読み落とし・ハルシネーション・トークン肥大による遅延を招く。本書は、会話から抽出した**意味の最小単位＝シナプス**をデータ層（SQLite/DuckDB）に外付けで保持し、入力に対して **2-Hop 連想**（① 意味的に近いシナプス → ② そのトピックで過去に成功したツール・前提背景）でデータ層側が極小コンテキストを事前合成し、LLM の役割を「知識の記憶」から「提示情報に基づくルーティング判断」へ特化させる。

この設計は、近年の研究が示す3つの潮流に整合する。
- **外付け階層記憶**で軽量モデルの実効的な文脈長と一貫性を拡張する（MemGPT / MemoryOS / HippoRAG）。
- **強弱モデルのルーティング**で品質を保ったままコスト・遅延を削減する（Hybrid LLM / RouteLLM）。
- **経験（成功/失敗）の蓄積と再利用**でエピソード横断の判断精度を上げる（ExpeL / Reflexion）。

ただし重要な落とし穴として、**構造化出力（JSON 強制）は推論を最大 27pt 劣化させ得る**。これは「思考を先に、確定出力を後に」並べることで回避する（§6.2）。本書のトークンシェイピング（`thought→t`, `selected_tool→cmd`）はこの順序を満たす形で設計する。

---

## 1. 背景：現行 Yuuka の「思考補助」の実態と限界

設計の出発点として、現行実装を事実ベースで確認する（出典は実コード）。

### 1.1 設計着手前のベースライン（schema v9）

> 本節の表は、本書の**改善対象であるシナプス層導入前（schema v9）のベースライン**を記述する（現行コードの逐一の状態ではない）。R0/R1 実装で「過去会話の参照」「ベクトル検索」「成功/失敗の学習」は既に変化している（§1.2 の注記・実装規範 [`architecture_v2.md`](../architecture/architecture_v2.md) §13）。

| 要素 | ベースライン実装（改善対象） | 出典 |
|---|---|---|
| 文脈の与え方 | **直近 15 件**（ギルドは 30 件）の生の会話履歴をそのまま注入 | [`messageLogRepo.ts`](../../src/db/messageLogRepo.ts) `CONTEXT_LIMIT=15` / `getRecentContext(userId, botId, 15)`（[`gemini.ts:629`](../../src/gemini.ts#L629)） |
| システムプロンプト | ペルソナ＋コンテキストノート＋検索スキル仕様＋ツール説明を**毎回ほぼ全文**組み立て | [`gemini.ts`](../../src/gemini.ts) `buildSystemInstruction` |
| 過去会話の参照 | LLM が `searchConversationLogs` を**明示的に呼んだ時だけ** FTS5 全文検索（**R1 で廃止 → L2 連想想起へ置換**。時系列要約 `summarizeConversationTopic` は維持） | `message_logs_fts`（FTS5） |
| 操作履歴 | `actionRecorder` が直近30件の Function 呼び出しを **Redis に TTL 2時間**で保持（秘匿系は除外） | [`actionRecorder.ts`](../../src/services/actionRecorder.ts) `MAX_ACTIONS=30 / TTL=2h` |
| ベクトル検索 | **なし** | — |
| 成功/失敗の学習 | **なし**（fc_history は揮発、統計化されない） | — |

### 1.2 限界

1. **生履歴注入のスケール限界**：直近 N 件固定のため、N を増やすとトークンと遅延が線形に増え、N を絞ると古い前提（ユーザーの好み・制約）が窓から落ちる。「重要だが古い」事実を構造的に保持する仕組みがない。
2. **想起が受動的**：過去の知見は LLM が `searchConversationLogs` を**思いつかない限り**使われない。連想的・能動的な想起がない。<br>**（R1 実装で解消）** 受動的全文検索 `searchConversationLogs` は廃止し、L2 連想想起（能動・自動）へ置換済み。時系列の文脈が本質的な `summarizeConversationTopic`（トピック要約）のみ維持する（実装規範 architecture_v2.md §13.6）。
3. **経験が捨てられている**：どのツールがどの文脈で成功/失敗したかは `actionRecorder` に一瞬残るだけで、2時間で消え、将来の判断に還元されない。
4. **ツール選択の肥大**：機能（ToDo・家計・ブラウザ・連絡先・会話検索…）＋ MCP 動的ツールが増えるほど、全ツール定義を毎回プロンプトに載せる方式は破綻に近づく（研究上、素朴な全載せは概ね **10〜15 ツール**が限界とされる、§7）。

本書はこの 4 点を、**データ層への記憶外付け**と**ハイブリッド・ルーティング**で解く。

---

## 2. アーキテクチャ思想：3層コンテキストとシナプス

人間の脳が記憶を断片（シナプス）として保持し、現在の刺激に対し関連断片を有機的に結合して思考する機構を、データ層への外付けでミニマルに再現する。知識保持・関係探索・統計集計を高速な C++/ネイティブ層（SQLite/DuckDB）へ委ね、LLM には極限まで絞った「思考のカンペ（極小コンテキスト）」だけを渡す。

```
┌─────────────────────────────────────────────────────────┐
│ L0 固定知識層（不変・キャッシュ対象）                       │
│   指示・ツール定義・ハーネス仕様・ペルソナ骨格              │
├─────────────────────────────────────────────────────────┤
│ L1 ワーキング層（直近の会話・揮発）                         │
│   直近数ターン（現行の15件を縮約）                          │
├─────────────────────────────────────────────────────────┤
│ L2 連想記憶層（シナプス・永続・外付け）★本書の中核          │
│   入力に応じて 2-Hop で動的合成される極小コンテキスト        │
└─────────────────────────────────────────────────────────┘
```

### 研究的裏付け

この3層構造は、エージェント記憶研究の主流と一致する。

- **MemGPT** は LLM を OS に見立て、限られた内部コンテキスト（=L1）と外部ストレージ（=L2）を OS 的階層で仮想管理し、実効文脈長を拡張する。本書の L0/L1/L2 はこの仮想メモリ階層の単純化である。([MemGPT, arXiv:2310.08560](https://arxiv.org/abs/2310.08560))
- **MemoryOS** は短期・中期・長期の多層ストアでこの考えを精緻化する。([MemoryOS 系の議論は Agent-Memory Survey 参照](https://github.com/Shichun-Liu/Agent-Memory-Paper-List))
- **HippoRAG** は海馬指標理論に着想し、長期記憶を**グラフ構造**で索引化して連想的に想起する。本書の「シナプス＋2-Hop連想」はこの神経科学的メタファの実装的近似である。([HippoRAG, arXiv:2405.14831](https://arxiv.org/abs/2405.14831))
- 近年は **LiCoMemory**（軽量・認知的エージェント記憶）や **Mem0 / A-MEM** のように、**軽量で編集可能な記憶**を志向する流れが強く、重量級 GraphDB を避ける本書の方針と整合する。([LiCoMemory, arXiv:2511.01448](https://arxiv.org/pdf/2511.01448) / [Agent-Memory Survey](https://github.com/Shichun-Liu/Agent-Memory-Paper-List))

> **設計含意:** L2 を「1本の長文」ではなく「索引化された断片の集合」として持つこと自体が、軽量モデルの読み落とし・ハルシネーションを下げる第一の梃子になる。

---

## 3. ハイブリッドLLM構成（採用方針）

**決定:** 重い推論・最終生成は **Gemini**、前処理・ルーティング・分類・抽出は**ローカル軽量LLM（7〜8B 級）**に分担する二段構え。

### 3.1 役割分担

| 段 | 担当 | 役割 | 失敗時 |
|---|---|---|---|
| **前処理（Local SLM）** | ローカル軽量LLM | ① 入力の意図分類／難易度判定（ルーター）<br>② シナプス抽出（会話→断片の要約・トピック付与）<br>③ 2-Hop 結果の妥当性チェック（軽い再ランク） | Gemini へエスカレーション |
| **本推論（Gemini）** | Gemini（既存 `llmClient`） | 複雑な推論・最終応答生成・Function Call の確定 | リトライ（既存 `generateAuxText` 同様） |

ルーターは Hybrid LLM/RouteLLM の知見に従い、**「難易度 × 要求品質」**で振り分ける。easy（定型・既知トピック・高勝率ツールが明確）はローカルで完結を試み、hard（曖昧・新規・低信頼）は Gemini へ。

### 3.2 研究的裏付け

- **Hybrid LLM（ICLR 2024）**：クエリ難易度と要求品質に応じて強弱モデルへ振り分けるルーターで、**大モデル呼び出しを最大 40% 削減しつつ品質を維持**。「easy なクエリでは小モデルが大モデルに匹敵（時に凌駕）する」ことが前提。([arXiv:2404.14618](https://arxiv.org/html/2404.14618))
- **RouteLLM**：選好データから学習したルーターで、強弱モデル間を切替え、**品質を落とさず 2 倍超のコスト削減**。([arXiv:2406.18665](https://arxiv.org/html/2406.18665v4))
- **FrugalGPT**：信頼できる応答が得られるまで段階的に問い合わせる**カスケード**。本書のエスカレーション設計の原型。
- **ConsRoute**：クラウド-エッジ-デバイス間の整合性を意識した適応ルーティング。ローカル(エッジ) × Gemini(クラウド) の本構成と同型。([arXiv:2603.21237](https://arxiv.org/pdf/2603.21237))

### 3.3 Yuuka への含意

- 既存の [`llmClient.ts`](../../src/services/llmClient.ts)（`getUserGenAI` / `getBotGenAI` / `generateAuxText`）を**本推論側**としてそのまま使い、**前処理側にローカル SLM クライアントを新設**して二段化する。Gemini 鍵まわりの分離（ユーザー鍵 / Bot 専用鍵）は不変条件（architecture_v2 §5）なので踏襲する。
- ルーターのコスト指標は「Gemini API 呼び出し回数・トークン」。**ローカル前処理で easy を吸収するほど Gemini 課金が下がる**のが直接の便益。
- ローカル SLM の品質下限がリスク（§12）。前処理の各タスクは**構造化出力（§6）**でレールを敷き、ブレを抑える。

---

## 4. 記憶・統計エンジンの選定：DuckDB vs SQLite+sqlite-vec

方針書は DuckDB をハブに据えるが、Yuuka には既存制約がある（architecture_v2 §0.6「**新規依存の追加は原則禁止**」、既に `better-sqlite3` 採用、ESM/Node、ネイティブアドオン同梱可）。両者を Yuuka のワークロードで評価する。

### 4.1 ワークロードの実態

シナプス・統計はすべて **`user_id`（または `bot_id × user_id`）でスコープ**される（データ分離は不変条件）。1ユーザーあたりの規模は現実的に**数千〜数万行**で、

- **L2 想起 = 数千ベクトルの KNN**（極小。総当たりでもミリ秒）
- **勝率集計 = 1ユーザー分の軽い GROUP BY**（列指向の真価が出る規模ではない）

つまり **per-user スコープでは「巨大列指向分析」の出番が小さい**。DuckDB の強み（数百万行のベクトル化集計）は、横断的・全テナント分析や大規模コーパスで初めて効く。

### 4.2 比較表

| 観点 | **SQLite + sqlite-vec**（踏襲） | **DuckDB**（方針書） |
|---|---|---|
| Yuuka への導入 | `better-sqlite3` に**ロード可能拡張**として同居。**新規DBエンジン不要** | 第2のDBエンジン＋Nodeバインディング追加 |
| 依存の重さ | 極小（拡張1つ） | ネイティブバインディング（成熟度・ビルドを要検証） |
| ベクトル検索 | `vec0` 仮想テーブルで KNN。OLTPと**同一 .db・同一トランザクション** | 拡張で可能だが本領は分析 |
| 統計集計（勝率） | per-user 規模なら十分高速 | 列指向で大規模集計が速い（per-userでは過剰） |
| 書込み一貫性 | 会話ログと**同一書込み経路**（二重書込みなし） | OLTPを別管理にすると二重書込み/同期が発生 |
| データ分離との相性 | 既存 `user_id` スコープをそのまま継承 | 別ストアにするとスコープ実装を再構築 |
| 運用 | 既存バックアップ（[`backupService.ts`](../../src/services/backupService.ts)）に乗る | バックアップ・移行を別途設計 |

### 4.3 推奨：段階併用（SQLite を正、DuckDB は分析サイドカー）

> **推奨アーキテクチャ**
> 1. **OLTP（正）= SQLite（better-sqlite3）**：シナプス・ツール実績は既存 `message_logs` と**同じ .db**に持ち、`user_id` スコープと既存バックアップをそのまま継承する。
> 2. **ベクトル検索 = sqlite-vec（`vec0`）**：L2 の 1st Hop（KNN）を同一トランザクション内で実行。新規依存は拡張1つに留め、architecture_v2 §0.6 の精神を守る。KNN がスケールで律速した場合のみ **USearch を読み取り専用インメモリ・サイドカー**として横付け（ID マッピングのみ）。
> 3. **DuckDB = 任意の分析サイドカー（後付け）**：勝率集計の中間ビューが per-user を超えて重くなった段階で、DuckDB を**読み取り専用**で導入し、`sqlite_scanner` で**同じ SQLite ファイルを ATTACH** して集計する。これにより「DuckDB を統計エンジンに」という方針書の意図を、**OLTP を移行せず・二重書込みなしで**実現する。

この段階併用は、方針書のビジョン（DuckDB をハブにした統計・グラフ代用）を尊重しつつ、Yuuka の「最小依存・既存スキーマ統合・データ分離不変」を破らない。**Phase 1 は sqlite-vec のみで始められる**のが実利上の決め手。

---

## 5. データモデル（概略）

> 本書は方針書のため概略に留める（詳細列は実装フェーズで migrations へ）。すべて `user_id`（汎用モードは `bot_id × user_id`）でスコープし、`message_logs` と同一 .db に置く。

### 5.1 シナプス（記憶の断片）

```
synapses
  id, user_id(, bot_id, guild_id)     -- 分離キー（不変条件を継承）
  content        -- 意味の最小単位（好み・制約・前提・事実）
  topic_id       -- トピック（疑似グラフの結合キー）
  source_msg_id  -- 抽出元 message_logs への参照
  created_at, last_used_at, use_count, decay_score  -- 想起の鮮度/強度
synapse_vec (vec0 仮想テーブル)
  rowid = synapses.id, embedding(float[D])           -- 1st Hop の KNN 対象
```
- **抽出**：ローカル SLM（§3）が会話から content/topic を要約付与。種は既存 `message_logs`。
- **減衰/強化**：想起されるたび `use_count`・`last_used_at` を更新し、`decay_score` で鮮度を反映（時間的意味記憶の知見）。([Temporal Semantic Memory, arXiv:2601.07468](https://arxiv.org/pdf/2601.07468))

### 5.2 ツール実行実績（経験）

```
tool_outcomes
  id, user_id, topic_id, synapse_id(任意)
  tool_name, args_digest         -- 認証情報・秘匿系は除外（actionRecorder の除外規約を継承）
  status                         -- success | error
  latency_ms, created_at
```
- **種は既存 [`actionRecorder.ts`](../../src/services/actionRecorder.ts)**。現状 Redis に TTL 2h で揮発しているものを、**秘匿除外規約はそのまま**に SQLite へ永続化し統計の素地にする。

### 5.3 トピック別効率（中間ビュー＝勝率）

```
topic_tool_stats  (バックグラウンド集計 / マテビュー相当)
  topic_id, tool_name, success, total, success_rate, last_updated
```
- 2nd Hop の JOIN コストをリアルタイムから外すための事前集計。per-user 規模では SQLite で十分、肥大時に DuckDB サイドカー（§4.3）。

---

## 6. シナプス検索アルゴリズム（2-Hop 連想）

```
[ユーザー入力]
   │  (ローカル SLM で embedding 生成)
   ▼
【1st Hop: 直接関連】 入力ベクトルに意味的に近いシナプスを数件（KNN, sqlite-vec）
   │  ヒットしたシナプスの topic_id を集合化
   ▼
【2nd Hop: 連想展開】 topic_id をキーに JOIN で
   ・topic_tool_stats から「過去に成功率が高かったツール」
   ・同トピックの「前提背景シナプス」
   を結合抽出
   ▼
[極小コンテキスト（数百トークン）] → LLM へ
```

擬似 SQL（1クエリで前処理を完結させる思想）:

```sql
WITH hit AS (   -- 1st Hop: 意味的 KNN
  SELECT s.id, s.topic_id, s.content
  FROM synapse_vec v JOIN synapses s ON s.id = v.rowid
  WHERE s.user_id = :uid
  ORDER BY vec_distance_cosine(v.embedding, :q) LIMIT :k
)
SELECT h.content AS premise,                     -- 前提背景
       t.tool_name, t.success_rate               -- 2nd Hop: 高勝率ツール
FROM hit h
LEFT JOIN topic_tool_stats t
  ON t.topic_id = h.topic_id AND t.user_id = :uid
WHERE t.total >= :min_samples
ORDER BY t.success_rate DESC;
```

### 研究的裏付け

- **GraphRAG / 多ホップ検索**：エンティティ関係を辿る多ホップ検索は、単純ベクトル検索では繋がらない**間接的な根拠**を組織化できる。本書はフル GraphDB を避け、**SQL の JOIN で疑似グラフ**を表現する軽量版（LightRAG/HopRAG/PathRAG が示す「軽量グラフ＋多段検索」の系譜）。([GraphRAG survey & 系譜](https://github.com/DEEP-PolyU/Awesome-GraphRAG))
- **HippoRAG** の連想想起と同型（1st Hop=直接想起、2nd Hop=連想展開）。([arXiv:2405.14831](https://arxiv.org/abs/2405.14831))
- **留意（過剰検索の罠）**：反復・多段検索は精度を上げる一方、ノイズ混入・コスト増の落とし穴がある。Hop 数・件数・`min_samples` は保守的に。([When to use Graphs in RAG, arXiv:2506.05690](https://arxiv.org/html/2506.05690v3) / [Beyond Static Retrieval, arXiv:2509.25530](https://arxiv.org/pdf/2509.25530))

---

## 7. ツール選択のスケーリング（多ツール対応）

Yuuka は機能群＋MCP 動的ツールでツール数が増え続ける。**全ツールを毎回プロンプトに載せる方式は概ね 10〜15 ツールで頭打ち**になることが報告されている。2-Hop の 2nd Hop は、本質的に**ツール検索（tool retrieval）**として機能する：トピックに紐づく高勝率ツールだけを LLM に提示し、選択肢を絞る。

### 研究的裏付け
- ツール数が増えると「正しいツールを大集合から検索・選択できるか」が正答率を律速する。素朴な全載せの限界は概ね 10〜15。([The Tool Selection Problem](https://tianpan.co/blog/2026-04-09-tool-selection-problem-agent-tool-routing-at-scale))
- **ToolGen**（検索と呼び出しを生成に統合、47k ツール規模）/ **AnyTool**（階層ディレクトリで段階フィルタ）/ **Toolshed**（RAG-Tool Fusion）は、いずれも「**事前に候補を絞ってから LLM に渡す**」設計。本書の 2nd Hop はこの一種。([ToolGen, arXiv:2410.03439](https://arxiv.org/html/2410.03439v2) / [Toolshed, arXiv:2410.14594](https://arxiv.org/pdf/2410.14594))

> **設計含意:** 既存の MCP 動的ツール（[`mcpDynamic.ts`](../../src/functions/mcpDynamic.ts)）は数が読めない。2nd Hop による**トピック→候補ツール絞り込み**を MCP ツールにも適用し、提示ツール数を物理的に削る。

---

## 8. ハーネス（制約）設計

軽量モデルは自由演技で思考がブレる。システム側で明確なレール（ハーネス）を敷く。

### 8.1 構造化出力 ＋ Thought/Action 分離

「思考（Thought）」と「実行アクション（Action）」を分離したパース可能形式で出力させ、データ層（SQLite/DuckDB）をハブに**状況分析→ツール選択→結果解釈をアトミックに分解**（マイクロ・エージェント・ワークフロー）。

### 8.2 ★最重要の落とし穴：JSON 強制は推論を壊し得る

- **強い形式制約は推論精度を最大 27pt 劣化**させ得る（EMNLP 2024）。機序は「JSON が**答えフィールドを思考完了前に**吐かせ、熟慮を短絡する」こと。
- **回避策（研究の処方）**：
  1. **スキーマ内で「推論」を「答え」より前に置く**（reasoning-then-answer で +8pt 改善の報告）。
  2. **二段化**：まず自由形式のスクラッチパッドで推論 → **最終の構造化出力にだけ制約デコードを適用**。
- ([Let Me Speak Freely?（EMNLP 2024 Industry）の知見](https://www.emergentmind.com/topics/constrained-decoding-json-mode) / [Draft-Conditioned Constrained Decoding, arXiv:2603.03305](https://arxiv.org/pdf/2603.03305))

> **本書のトークンシェイピングはこの制約を満たす:** キー順は必ず **`t`(thought) → `cmd`(tool) → `args`**。`cmd` を先頭に置く短縮は**禁止**（推論短絡を招く）。短縮しても**順序＝思考が先**を不変条件とする。

### 8.3 ReAct と経験再利用

- **ReAct**（Thought→Action→Observation ループ）はエピソード内の制御を強くするが、**エピソード横断の経験再利用の機構を持たない**。([ReAct, arXiv:2210.03629](https://arxiv.org/abs/2210.03629))
- そこを **ExpeL（経験から洞察を抽出）/ Reflexion（失敗の自己反省を記憶）/ Agent Workflow Memory** が補う。本書の `tool_outcomes`＋`topic_tool_stats`（§5）は、この**横断的経験記憶を統計として凝縮**した実装に当たる。([ExpeL, arXiv:2308.10144](https://arxiv.org/pdf/2308.10144) / [Reflexion, arXiv:2303.11366](https://arxiv.org/abs/2303.11366))

### 8.4 ペルソナと構造化ハーネスの両立

**JSON 強制（ハーネス）とペルソナ適用は別レイヤーであり両立する。** LLM 出力には性質の異なる2つの面があり、構造化制約が掛かるのは前者だけである。

| | 制御プレーン（内部） | 提示プレーン（ユーザー向け） |
|---|---|---|
| 役割 | 思考・ツール選択・ルーティング | 最終応答の自然文 |
| 出力形式 | **構造化JSON（ハーネス）** | **自由文（制約しない）** |
| ペルソナ | **不要**（載せない） | **全面適用** |
| 担当（ハイブリッド §3） | ローカルSLM | Gemini |
| ユーザー可視 | ✗ | ○ |

ペルソナは「口調・人格＝ユーザーに見える自然文の表層」の性質であり、JSON が触れる制御プレーンとは干渉しない。これは §8.2 の処方（**制御・推論は自由形式 → 確定する構造化部分だけ制約**）そのものである。

- **推奨＝二段分離（パターンA）**：Stage 1（ローカルSLM・JSON・ペルソナなし）で `{t, cmd, args}` を出し、Stage 2（Gemini・自由文・ペルソナ全面）でツール結果を踏まえ応答生成。ペルソナ（最大20000字, architecture_v2 §2）を**喋る瞬間だけ** `(user_id, bot_id)` で引くため、L0 固定プレフィックス（キャッシュ対象）を汚さずトークンも減る。ペルソナBot分離（v8 `bot_active_personas`）とも整合し、「別Botが別人格で応答」する事故を構造的に防ぐ。
- **1コール妥協案（パターンB）**：JSON に自由文フィールド `reply` を **`t` の後ろ**に置く（`{"t":…,"cmd":…,"reply":"…ですよ。"}`）。自由テキストなので劣化リスクは小さいが、ペルソナ濃度・キャッシュ効率は A が上。
- **禁止**：最終応答の自然文を列挙的／硬いスキーマに押し込むこと（§8.2 の推論劣化に加え、口調が平板化し in-character 感を失う）。

---

## 9. レイテンシ最適化

### 9.1 トークンシェイピング
`thought→t`, `selected_tool→cmd` 等にキーを短縮し出力トークンを物理削減（生成 1.2〜1.5x の狙い）。**ただし §8.2 の順序制約を最優先**（速度のために答えを先頭へ出さない）。

### 9.2 プロンプト/プレフィックスキャッシュ
プロンプトを **`L0 固定（指示・ツール定義・ハーネス）` → `L2 シナプス（可変）` → `L1 入力（可変）`** の順に固定→可変で並べ、前方一致キャッシュを最大化。

- **ローカル SLM**：vLLM/llama.cpp の**プレフィックスキャッシュで TTFT を大幅短縮**（llama.cpp で最大 ~93%、vLLM で ~22%、長文では数秒→0.6秒級の報告）。([Prefix caching, vLLM docs](https://docs.vllm.ai/en/stable/design/prefix_caching/) / [llama.cpp 議論](https://github.com/ggml-org/llama.cpp/discussions/20574))
- **Gemini（本推論）**：同様に**コンテキストキャッシュ**が効くよう固定部を前置。固定→可変の順序原則は両者に共通で効く。

### 9.3 投機的実行（2種を区別）
1. **ハーネスの投機的実行（ツール・プレフェッチ）**：1st Hop の時点で `topic_tool_stats.success_rate` が閾値（例 ≥0.9）なら、LLM 出力を待たずに**バックグラウンドでツールを先行実行**。的中で体感ゼロ遅延、外れは破棄。Yuuka 適用時は**副作用のない読み取り系のみ**を対象に限定（書込み・認証情報系・課金系は禁止）。
2. **トークンレベルの投機的デコード**（ローカル SLM 側）：ドラフトモデルで先読み→並列検証で **2〜3x 高速化**。本書の(1)とは別レイヤーで併用可。注意：**ドラフトの言語モデル精度よりドラフトの遅延が支配的**で、素朴な実装はドラフト遅延が総時間の 60〜75% を占めうる。([Speculative Decoding Survey（ACL 2024）, arXiv:2401.07851](https://arxiv.org/pdf/2401.07851))

---

## 10. 評価・計測方針

方針書として「効いたか」を測る指標を先に定義する（Phase 0 で現行ベースライン取得）。

| 区分 | 指標 | 目標の向き |
|---|---|---|
| 遅延 | TTFT / 応答 p50・p95 | ↓ |
| コスト | 1応答あたり Gemini 呼び出し回数・トークン | ↓（ハイブリッドの主便益） |
| ルーティング | ローカル吸収率 / 誤エスカレーション率 | 吸収率↑・誤り↓ |
| 想起品質 | 2-Hop 提示の根拠的中率（人手/LLM 判定） | ↑ |
| ツール | トピック別 `success_rate`、誤ツール選択率 | ↑ / ↓ |
| 整合性 | ハルシネーション率・グラウンデッドネス | ↓ / ↑ |

長期記憶の評価は **長ホライズン記憶ベンチ**（AMA-Bench 等）の観点を流用し、回帰テスト化する。([AMA-Bench, arXiv:2602.22769](https://arxiv.org/pdf/2602.22769))

---

## 11. 段階導入ロードマップ（概略）

> 成果物方針（研究裏付け集中）に従い、実装マッピングは概略に留める。各 Phase は単独でユーザー価値が出る粒度。

| Phase | 内容 | 新規依存 | 主な便益 |
|---|---|---|---|
| **0 計測** | 現行ベースライン（§10 指標）取得。`actionRecorder` の SQLite 永続化（揮発→蓄積） | なし | 効果測定の土台 |
| **1 連想記憶** | `synapses`＋`synapse_vec`（sqlite-vec）、ローカル SLM でシナプス抽出、L2 2-Hop（1st Hop のみ） | sqlite-vec（拡張1） | 古い前提を落とさない・受動想起の解消 |
| **2 経験統計** | `tool_outcomes`＋`topic_tool_stats`、2nd Hop（高勝率ツール提示）、多ツール絞り込み | （肥大時のみ DuckDB サイドカー） | ツール選択精度↑・プロンプト縮小 |
| **3 ハイブリッド** | ローカル SLM ルーター（難易度×品質）でeasy 吸収、Gemini はhardのみ | ローカル推論ランタイム | Gemini コスト・遅延↓ |
| **4 投機/キャッシュ** | プレフィックスキャッシュ最適化、読み取り系ツールの投機的プレフェッチ | なし | 体感遅延↓ |

各 Phase 着手時に、該当部分を [`architecture/architecture_v2.md`](../architecture/architecture_v2.md) へ昇格し、migrations（schema v10+）として確定する。データ分離・認証情報非露出・暗号の不変条件（architecture_v2 §0）は全 Phase で厳守。

---

## 12. リスクと留意点

| リスク | 内容 | 緩和 |
|---|---|---|
| 推論短絡 | JSON 強制で推論劣化（最大 27pt） | 思考を先・答えを後（§8.2）、二段化 |
| 過剰検索 | 多段検索のノイズ・コスト増 | Hop/件数/`min_samples` を保守的に（§6） |
| ローカル品質下限 | SLM のブレ・誤分類 | 構造化ハーネス＋誤エスカレーション率を監視、閾値で Gemini フォールバック |
| 投機の副作用 | プレフェッチが書込み/課金を起こす | **読み取り系のみ**に限定（§9.3） |
| プライバシ/記憶汚染 | シナプスは個人情報。記憶の改ざん・毒化リスク | `user_id` 分離・PW 系は記憶対象外（actionRecorder 除外規約継承）、暗号は既存規約。記憶ガバナンス（SSGM 等）の観点を導入 |
| 統計の陳腐化 | 中間ビューの鮮度劣化 | バックグラウンド再集計＋`decay_score` |

記憶ガバナンス（進化する記憶のリスクと安全機構）は新興研究領域であり、設計初期から織り込む。([SSGM, arXiv:2603.11768](https://arxiv.org/html/2603.11768v1))

---

## 13. 設計の最大の強み

思考・記憶の重荷をデータ層（SQLite/DuckDB）へ完全分離することで、将来さらに軽量で優秀なローカル LLM が登場した際にも、**中心の LLM をカチャッと入れ替えるだけ**で、蓄積済みの「ユーザー固有のシナプス（結合度）」と「ツール勝率（経験）」をそのまま引き継いで進化させられる。ハイブリッド構成はこの入替えを**前処理側・本推論側それぞれ独立に**行えるようにし、移行リスクを分散する。

---

## 付録A. 参考文献（URL 確認済み）

**記憶アーキテクチャ**
- MemGPT: Towards LLMs as Operating Systems — [arXiv:2310.08560](https://arxiv.org/abs/2310.08560)
- HippoRAG: Neurobiologically Inspired Long-Term Memory — [arXiv:2405.14831](https://arxiv.org/abs/2405.14831)
- LiCoMemory: Lightweight and Cognitive Agentic Memory — [arXiv:2511.01448](https://arxiv.org/pdf/2511.01448)
- Memory in the Age of AI Agents: A Survey（論文リスト）— [GitHub](https://github.com/Shichun-Liu/Agent-Memory-Paper-List)
- Temporal Semantic Memory for Personalized LLM Agents — [arXiv:2601.07468](https://arxiv.org/pdf/2601.07468)
- AMA-Bench: 長ホライズン記憶評価 — [arXiv:2602.22769](https://arxiv.org/pdf/2602.22769)
- Governing Evolving Memory (SSGM) — [arXiv:2603.11768](https://arxiv.org/html/2603.11768v1)

**ハイブリッド/ルーティング**
- Hybrid LLM: Cost-Efficient and Quality-Aware Query Routing（ICLR 2024）— [arXiv:2404.14618](https://arxiv.org/html/2404.14618)
- RouteLLM: Learning to Route LLMs with Preference Data — [arXiv:2406.18665](https://arxiv.org/html/2406.18665v4)
- ConsRoute: Cloud-Edge-Device 適応ルーティング — [arXiv:2603.21237](https://arxiv.org/pdf/2603.21237)

**検索・グラフ**
- Awesome-GraphRAG（サーベイ集）— [GitHub](https://github.com/DEEP-PolyU/Awesome-GraphRAG)
- When to use Graphs in RAG — [arXiv:2506.05690](https://arxiv.org/html/2506.05690v3)
- Beyond Static Retrieval: Pitfalls of Iterative Retrieval in GraphRAG — [arXiv:2509.25530](https://arxiv.org/pdf/2509.25530)

**ツール使用・経験学習**
- ReAct: Synergizing Reasoning and Acting — [arXiv:2210.03629](https://arxiv.org/abs/2210.03629)
- Reflexion: Language Agents with Verbal Reinforcement Learning — [arXiv:2303.11366](https://arxiv.org/abs/2303.11366)
- ExpeL: LLM Agents Are Experiential Learners — [arXiv:2308.10144](https://arxiv.org/pdf/2308.10144)
- ToolGen: Unified Tool Retrieval and Calling via Generation — [arXiv:2410.03439](https://arxiv.org/html/2410.03439v2)
- Toolshed: Scale Tool-Equipped Agents — [arXiv:2410.14594](https://arxiv.org/pdf/2410.14594)
- The Tool Selection Problem（10〜15 ツール限界の議論）— [TianPan.co](https://tianpan.co/blog/2026-04-09-tool-selection-problem-agent-tool-routing-at-scale)

**構造化出力・推論**
- 構造化出力 / 制約デコードと推論劣化（Let Me Speak Freely? の知見）— [Constrained Decoding (JSON-mode)](https://www.emergentmind.com/topics/constrained-decoding-json-mode)
- Draft-Conditioned Constrained Decoding — [arXiv:2603.03305](https://arxiv.org/pdf/2603.03305)

**高速化**
- Speculative Decoding: A Comprehensive Survey（ACL 2024）— [arXiv:2401.07851](https://arxiv.org/pdf/2401.07851)
- Automatic Prefix Caching（vLLM）— [docs](https://docs.vllm.ai/en/stable/design/prefix_caching/)
- llama.cpp Host-Memory Prompt Caching — [GitHub Discussion](https://github.com/ggml-org/llama.cpp/discussions/20574)

> ⚠️ 一部の arXiv 番号は将来日付（2026 等）の検索結果に基づくものを含む。本実装着手前に各リンクの版・主張を再確認すること。
