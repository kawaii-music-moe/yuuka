`resilience-rs` isn't published under that exact crate name (404). I have all the verified primary-source data needed. Let me compile the final report.

---

# Tokio 監視・バックオフ・サーキットブレーカ調査 (2026-07-01 時点)

全て crates.io / docs.rs / rustsec.org の一次情報で検証済み。tokio 最新安定版は **1.52.3**（2026-05-08 リリース、[crates.io](https://crates.io/crates/tokio)）。

---

## 1. 長寿命タスク監視 & パニック隔離 (JoinSet / spawn の挙動)

### 確定した重要事実（正確性の要）
**spawn したタスク内のパニックはプロセスを殺さない。当該タスクに隔離され、`JoinHandle`（および `JoinSet::join_next`）が `Err(JoinError)` として返す。** `JoinError::is_panic()` が `true`、`into_panic()` でパニックペイロードを取り出せる。パニックを親に伝播させるかは完全にプログラマ制御（`panic::resume_unwind(err.into_panic())` を呼ばない限り伝播しない）。
- 根拠: [docs.rs `JoinError`](https://docs.rs/tokio/latest/tokio/task/struct.JoinError.html) — "Returns true if the error was caused by the task panicking"、`is_panic()` / `into_panic()` / `try_into_panic()` の存在。
- 根拠: [tokio discussion #5624](https://github.com/tokio-rs/tokio/discussions/5624) — spawned task のパニックは JoinHandle 経由でエラー化。

これは監視設計の土台として決定的に重要: **`tokio::spawn` した監視タスクが panic しても runtime/プロセスは生き続ける** ので、`join_next` ループで検知して再起動する設計が成立する。

- **推奨**: `tokio::task::JoinSet`（標準 API、外部クレート不要）を `join_next()` ループで回し、`Err(JoinError)` を検知したら該当タスクを再 spawn する自前スーパーバイザ。
- **バージョン**: tokio 1.52.3（JoinSet は `rt` フィーチャのみで利用可、**安定 API・unstable フラグ不要**）。[docs.rs JoinSet](https://docs.rs/tokio/latest/tokio/task/struct.JoinSet.html)
- **根拠**: JoinSet は `tokio::task` の安定公開 API（2026-05-08 の 1.52.3 で提供）。
- **落とし穴**:
  1. **`JoinSet::join_all()` は使うな**（監視用途では）。docs 明記: "If any tasks on the JoinSet fail with a `JoinError`, then this call to `join_all` will panic and all remaining tasks on the JoinSet are cancelled." → 監視では必ず **`join_next()` を手動ループ**して各エラーを個別処理する。
  2. **JoinSet を drop すると全タスクが即 abort** される。スーパーバイザ本体が落ちると配下も全滅するので、スーパーバイザ自身の生存を別レイヤで保証する。
  3. `catch_unwind` は非同期には基本不要（tokio が spawn 境界で捕捉する）。`std::panic::catch_unwind` を `.await` をまたいで使うのは `UnwindSafe` 制約でハマりやすく非推奨。パニック隔離は spawn 境界に任せるのが定石。
  4. `panic = "abort"`（Cargo profile）にしていると**この隔離が無効化**され panic でプロセスが即死する。self-healing 要件下では `panic = "unwind"`（デフォルト）を維持すること。
- **確信度**: **高**（tokio 公式 docs で直接確認）。

---

## 2. `tokio-graceful-shutdown`（サブシステムツリー / 監視）

- **推奨**: 採用推奨。サブシステムのツリー構造、graceful shutdown 伝播、パニック/エラー時の親への伝播とツリー単位停止を提供。長寿命監視タスクの停止協調に有用。
- **バージョン**: **0.19.3**（2026-04-02 リリース）。[crates.io](https://crates.io/crates/tokio-graceful-shutdown) / [GitHub Finomnis/tokio-graceful-shutdown](https://github.com/Finomnis/tokio-graceful-shutdown)
- **根拠**: crates.io API 上のバージョン履歴で活発な更新を確認 — 0.19.3 (2026-04-02)、0.19.0/0.18.0 (2025-09-28)、0.17.x (2025-09)。累計 DL 約 567K。**2026 年も活発にメンテされている**（直近 3 ヶ月以内にリリース）。
- **落とし穴**:
  1. **0.x 系のためセマンティックには不安定**。マイナー更新（0.17→0.18→0.19）で API 破壊があり得るので、バージョンをピン留めしアップグレード時は CHANGELOG 確認。
  2. 提供するのは主に**「停止協調 + エラー伝播」**であって、「タスクの自動再起動（restart-on-failure）」そのものは主目的ではない。self-recover の再起動ロジックは自前 or JoinSet と組み合わせる必要がある。「subsystem がエラー/パニックしたらツリーを畳んで graceful shutdown」型で、Erlang 的な individual restart supervisor ではない点に注意。
- **確信度**: **高**（crates.io で版・日付を直接確認）。

---

## 3. 指数バックオフ・クレート

| クレート | 最新版 | 最終リリース | 2026 メンテ状況 |
|---|---|---|---|
| **backon** | **1.6.0** | 2025-10-18 | ✅ 活発（累計 5,700 万+ DL） |
| backoff | 0.4.0 | **2021-12-14** | ❌ **非メンテ（RUSTSEC-2025-0012）** |
| tryhard | 0.5.2 | 2025-06-23 | ✅ メンテ（Embark Studios） |
| exponential-backoff | 2.1.0 | 2025-04-17 | 🟡 生成器のみ、緩やか |
| again | 0.1.2 | **2020-05-31** | ❌ 実質放置 |

- **推奨**: **`backon` 1.6.0** — retry + 指数バックオフ + ジッタ の第一選択。
- **バージョン**: **1.6.0**（2025-10-18）。[crates.io](https://crates.io/crates/backon) / [docs.rs](https://docs.rs/backon/)
- **根拠**:
  - `backoff` は **RUSTSEC-2025-0012**（2025-03-07 発行、2025-08-06 更新）で公式に "no longer actively maintained" と宣言され、**代替として `backon` が明示推奨**されている。[RUSTSEC-2025-0012](https://rustsec.org/advisories/RUSTSEC-2025-0012.html)。backoff 最終版 0.4.0 は 2021-12-14 で 4 年半更新なし（[crates.io/backoff](https://crates.io/crates/backoff)）。
  - backon は async/blocking 両対応、指数/定数/フィボナッチ戦略、ジッタ、Retry-After 動的バックオフ、no-std/wasm 対応。累計 5,700 万 DL 超、2025-10 リリースで活発（[GitHub Xuanwo/backon](https://github.com/Xuanwo/backon)）。**`backon` は事実上 `backoff` の後継**。
- **落とし穴**:
  1. **`backoff` は絶対に新規採用しない**（セキュリティ監査 `cargo audit` で RUSTSEC 警告が出る）。既存依存があれば移行。API は近く移行容易。
  2. backon はジッタが**デフォルト無効**な戦略があるため、`ExponentialBuilder::default().with_jitter()` を**明示的に有効化**すること（thundering herd 回避に必須）。
  3. `tryhard` も良メンテだが future 専用でジッタ設定が backon ほど柔軟でない。`exponential-backoff` は**遅延値を生成するだけ**でリトライ実行ループは自前（軽量だが機能薄）。用途が単純なら選択肢だが、監視/self-heal では backon 推奨。
- **確信度**: **高**（RUSTSEC 公式 + crates.io で全版確認）。

---

## 4. サーキットブレーカ・クレート

| クレート | 最新版 | 最終リリース | 状況 |
|---|---|---|---|
| **recloser** | **1.4.0** | **2026-06-20** | ✅ 活発（async 対応） |
| failsafe (failsafe-rs) | 1.3.0 | 2024-07-05 | 🟡 準放置気味（2 年更新なし） |
| circuit_breaker | 0.1.1 | 2024-10-07 | ❌ 未成熟（168 行、DL 約 2.3 万） |
| resilience-rs | — | — | ❌ **その名の crate は存在せず（crates.io 404）** |

- **推奨**: **`recloser` 1.4.0** — マルチスレッド Tokio + 長寿命タスクという本件要件に最適。
- **バージョン**: **1.4.0**（2026-06-20、11 日前）。[crates.io](https://crates.io/crates/recloser) / [docs.rs](https://docs.rs/recloser/latest/recloser/) / [GitHub lerouxrgd/recloser](https://github.com/lerouxrgd/recloser)
- **根拠**:
  - recloser: リングバッファ実装の**並行**サーキットブレーカ。Closed/Open/HalfOpen の 3 状態、`RecloserBuilder` で失敗率・バッファ長を設定。`AsyncRecloser` ラッパで futures 対応（`recloser.call(future)`）。docs 上「マルチスレッドで failsafe 比 **10 倍高速**」。バージョン履歴が活発 — 1.4.0 (2026-06-20)、1.3.1 (2025-11-30)、1.3.0 (2025-11-01)、1.2.0 (2025-08-19)。**2026 年に継続リリースされている唯一の本格クレート**。
  - failsafe-rs: 機能は豊富（スライディングウィンドウ、`futures-support` feature で async 対応）だが**最終版 1.3.0 が 2024-07-05 で約 2 年更新なし**（[crates.io/failsafe](https://crates.io/crates/failsafe)）。累計 DL は最多（1,540 万）で枯れてはいるが、メンテ観点では recloser に劣る。
  - `circuit_breaker`（Mahmud Bello 作）は 0.1.1 / 168 行 / DL 約 2.3 万で**未成熟**、本番監視には非推奨（[crates.io/circuit_breaker](https://crates.io/crates/circuit_breaker)）。
  - `resilience-rs` はその crate 名で crates.io に**存在しない**（API 404）。lib.rs に別名/別実体で紹介記事がある程度で、依存に加えられない。
- **落とし穴**:
  1. recloser の `AsyncRecloser` は「futures-aware」だが docs 上 **tokio 明示保証はない**（標準 futures で動作）。tokio ランタイム上で問題なく動くが、採用前に自分のタスク構成で軽く PoC 検証を推奨。
  2. **サーキットブレーカ・エコシステムは全体として指数バックオフ系ほど成熟していない**。要件がシンプル（失敗率閾値 + open タイマ）なら **`AtomicU*` + 状態機械で自前実装も十分現実的**（circuit breaker のコアは数十〜百数十行）。外部クレートの 0.x/準放置リスクを負いたくない場合は自前ブレーカ + backon（リトライ）の組み合わせが堅い。
  3. failsafe を選ぶ場合は**メンテ停滞**を許容できるかを判断（機能は成熟・安定だが新規 fix は期待薄）。
- **確信度**: **中〜高**。crates.io で版・日付は確定（高）。ただし「recloser を本番監視の第一推奨」とする点は、ブレーカ生態系全体が薄いこと・tokio 明示保証がないことから **中**。自前実装が有力な代替である点を強調。

---

## まとめ（self-healing 設計への示唆）

- **監視の土台**: `panic = "unwind"` を維持 + `JoinSet::join_next()` ループで `Err(JoinError)` 検知 → 再 spawn。パニックはプロセスを殺さない（**確信度 高、tokio 公式確認済み**）。
- **停止協調**: `tokio-graceful-shutdown` 0.19.3（ただし再起動ロジックは自前補完）。
- **リトライ**: `backon` 1.6.0 一択（`backoff` は RUSTSEC で非メンテ確定、採用禁止）、ジッタ明示 ON。
- **ブレーカ**: `recloser` 1.4.0 が現行最有力だが生態系が薄いため**自前実装も対等な選択肢**。`failsafe` は枯れてはいるが 2 年更新なし。

全バージョン・日付は crates.io API で検証済み（tokio 1.52.3 / tokio-graceful-shutdown 0.19.3 / backon 1.6.0 / recloser 1.4.0 / failsafe 1.3.0）。