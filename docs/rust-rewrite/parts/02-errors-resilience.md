# yuuka バックエンド Rust 移行 マスタープラン — 第4〜5部

> 対象: セクション4「厳格エラーアーキテクチャ」・セクション5「自己復帰／スーパーバイザ設計」。
> 上位決定 [`00-decisions.md`](../00-decisions.md)（絶対制約1・2）に**厳密整合**。ここで技術選定を再決定はしない。数値は全て一次ソース照合済み（各節末のリンク参照）。
> 一次照合レポート: [errors-thiserror](../verification/rpt-errors-thiserror.md) / [clippy-cargodeny](../verification/rpt-clippy-cargodeny.md) / [resilience](../verification/rpt-resilience-tokio-backon-recloser.md)。

---

## 4. 厳格エラーアーキテクチャ（anyhow 禁止の実現と機械的強制）

**目的（絶対制約1）**: `anyhow`/`eyre`/`color-eyre`/`Box<dyn Error>` 等の型消去＝「握り潰し」を、規約ではなく**コンパイラと CI で機械的に不可能にする**。エラーはすべて `thiserror 2.0.18` の**層別・具体列挙型**で表現し、層をまたぐ変換は明示化する。

### 4.1 層別エラー分類体系

各層（クレート／モジュール境界）ごとに 1 つの具体エラー enum を持たせる。下位層の enum は上位層の enum のバリアントとして**明示的に**畳み込む（§4.3 の「層境界のみ `#[from]`」を参照）。

| エラー型 | 所属層 | 主な原因 | fail-fast? |
|---|---|---|---|
| `ConfigError` | 起動・設定 | config.yaml 欠落／型不一致／必須 secret 不備 | **致命的**（§5.6） |
| `DbError` | DB ドライバ（rusqlite） | SQLITE_BUSY、制約違反、I/O、シリアライズ | 回復可能（リトライ／縮退） |
| `RepoError` | リポジトリ（データアクセス） | `DbError` の意味付け、`NotFound`、`UserId` 欠落 | 回復可能 |
| `AuthError` | 認証・認可 | セッション無効、権限不足、トークン不正 | 回復可能（401/403 化） |
| `ValidationError` | 入力検証 | DTO 制約違反、範囲外、必須欠落 | 回復可能（400 化） |
| `GeminiError` | Gemini 薄ラッパ | 429/`RetryInfo`、5xx、パース失敗、safety block | 回復可能（backon＋ブレーカ） |
| `DiscordError` | Discord（twilight） | ゲートウェイ断、HTTP 4xx/5xx、レート制限 | 回復可能（テナント別再起動） |
| `PluginError` | ツール／プラグイン | Native/MCP/WASM 呼び出し失敗、能力スコープ違反、タイムアウト | 回復可能（当該ツールのみ失敗） |
| `IpcError` | プロセス間・synapse | synapse 接続断、プロトコル不整合 | 回復可能（縮退＝直近履歴のみ） |
| `WebError` | Web 面（axum ハンドラ） | 上記各層を HTTP へ写像する統合型 | 回復可能（`IntoResponse`） |

原則:
- **1 層 = 1 enum**。層内の全失敗モードをバリアントで列挙する（`Other(String)` 的な逃げ道を作らない）。
- クレート／モジュール**境界を越えて公開**する enum には `#[non_exhaustive]` を付ける（§4.4）。
- `WebError` は Web クレート内に置き、`Auth/Validation/Repo/Gemini/Plugin/Ipc` を**明示 match で**畳み込む統合層とする（§4.5 の `IntoResponse` で網羅 match）。

### 4.2 thiserror 2.0.18 での定義例

`thiserror` は**手続きマクロのみ**を提供する軽量クレート（Display/Error/From の導出）。ランタイム機能は最小限、という設計思想を前提にする。MSRV = Rust 1.68、edition 2021。属性は 2.0.18 の docs.rs / GitHub README で確認済み。

```rust
// crates/db/src/error.rs — 最下段（ドライバ層）
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DbError {
    // 真の層境界: rusqlite -> DbError は #[from] で自動 From 生成。
    // #[from] は #[source] を暗黙に含むため #[source] は書かない。
    #[error("sqlite operation failed")]
    Sqlite(#[from] rusqlite::Error),

    #[error("write transaction is busy after retries")]
    Busy,

    // spawn_blocking の join 失敗など。transparent は「最下段で素通し」限定。
    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
}
```

```rust
// crates/repo/src/error.rs — 一段上（意味付け層）
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RepoError {
    // 層境界: DbError -> RepoError も #[from]。
    #[error("database error")]
    Db(#[from] DbError),

    // 追加コンテキストを持つバリアントは #[from] 不可（後述の衝突制約）。
    #[error("record not found: {kind} id={id}")]
    NotFound { kind: &'static str, id: i64 },

    // データ分離キー欠落を型で顕在化（UserId newtype と連動）。
    #[error("user scope missing for repository operation")]
    UserScopeMissing,
}
```

```rust
// crates/gemini/src/error.rs — 外部依存ラッパ（429/RetryInfo を掌握）
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GeminiError {
    #[error(transparent)]
    Http(#[from] reqwest::Error),

    // 現行の rate-limit バックオフを 1:1 移植するための構造化バリアント。
    // retry_after は RetryInfo(retryDelay) 由来。§5.3 のブレーカ/backon が参照。
    #[error("rate limited (retry after {retry_after:?})")]
    RateLimited { retry_after: Option<std::time::Duration> },

    #[error("gemini returned status {status}")]
    Status { status: u16 },

    #[error("response decode failed")]
    Decode { #[source] source: serde_json::Error },

    #[error("content blocked by safety filter")]
    SafetyBlocked,
}
```

属性の使い分け（一次確認済み）:
- `#[error("{field}")]` → `write!("{}", self.field)` / `#[error("{0}")]` → tuple 要素。`:?` で Debug 補間。
- **`#[from]`**: 付与バリアントごとに `From` impl を自動生成し、**そのバリアントはソースエラー（＋任意で `#[backtrace]` フィールド）以外のフィールドを持てない**。かつ `#[source]` を暗黙に含む（両方書かない）。
- **`#[source]`**: 下位エラーを `Error::source()` として公開しつつ、**追加コンテキストのフィールドを同居**させたいとき（`From` は生成されない）。
- **`#[error(transparent)]`**: Display と source を下位へ素通し（メッセージを足さない）。**最下段の "そのまま素通し" 用途に限定**。上位で多用するとコンテキストが失われ握り潰しに近づく。
- **`#[non_exhaustive]`**: これは thiserror ではなく**標準 Rust の言語属性**（§4.4）。derive の上に併記する。

> ⚠️ **1.x → 2.0 の非互換**: 2.0 で `#[error("{x}")]` のフィールド補間解決が厳格化された。1.x 前提のコード片をそのまま貼らない。新規は 2.0.18 固定で問題なし。

### 4.3 `#[from]` は層境界のみ・層跨ぎは明示 match

**規則**: `#[from]` を使ってよいのは「**下位層 enum → 直上位層 enum**」の 1 段の畳み込みだけ。それ以外（複数下位型の吸い込み、層を 2 段以上スキップする変換、HTTP 面への写像）は**明示的な `match`／`map_err` で変換**する。

理由:
1. **握り潰し回避**: `#[from]` を多用すると「どこでどの層のエラーが混入したか」の意味論が薄れ、anyhow 的な "何でも吸い込む" 型に退化する。層境界で意図的に変換することで、禁止方針（絶対制約1）と構造的に整合させる。
2. **`#[from]` の衝突制約**: 同じ下位型（例: `std::io::Error`）を複数バリアントで `#[from]` すると `From` impl が衝突し**コンパイル不能**。素通しが複数必要なら片方を `#[source]` に落とすか、newtype で下位型を分ける。→ そもそも「1 バリアント = 1 ソース型」が実質前提なので、層をまたぐ多対多の吸い込みは `#[from]` では表現できない。

明示変換の例（層を跨ぐので `#[from]` を使わない）:

```rust
// Web ハンドラ内: AuthError と ValidationError を WebError へ明示畳み込み。
// これらは異なる層なので from ではなく match で意味付けする。
let user = authenticate(&parts)
    .map_err(|e: AuthError| match e {
        AuthError::SessionInvalid | AuthError::TokenMalformed => WebError::Unauthorized,
        AuthError::Forbidden => WebError::Forbidden,
    })?;
```

**Result 型エイリアス方針**: 各クレートで `pub type Result<T, E = ThisLayerError> = std::result::Result<T, E>;` を定義してよい。ただし**デフォルト型引数を 1 種類に固定**し、別層のエラーを返す関数では**明示的に完全型（`std::result::Result<T, OtherError>`）を書く**こと。「万能 `Result`」を作らない（それが anyhow の入口になる）。

### 4.4 `#[non_exhaustive]` の運用

- **標準 Rust の言語属性**（安定版・Rust Reference 記載）。thiserror とは独立。
- **クレート／モジュール境界を越えて公開**するエラー enum に付ける。付けると**定義クレート外の `match` はワイルドカード `_ =>` が必須**になり、将来のバリアント追加が下位コードを壊さない（非破壊）。
- **定義クレート内では効果なし**（自クレート内の網羅 match は従来どおり可）。→ これが §4.5 の設計と噛み合う: **`IntoResponse` を `WebError` と同一クレートに置けば `#[non_exhaustive]` でも完全網羅 match が書け**、バリアント追加漏れをコンパイルエラーで検知できる。
- 諸刃の剣: 別クレートで `WebError` を完全網羅マッピングしたい消費側には `_ =>` が強制され漏れが隠れる。→ **HTTP 写像は必ず定義クレート内に置く**ことでこの罠を回避する（下記）。

### 4.5 axum `IntoResponse` の手書き写像

**役割分担**: 型定義は `thiserror`、HTTP 写像は**手書きの `IntoResponse`**。`thiserror` は HTTP 写像を一切提供しない。`WebError` は `IntoResponse` と**同一クレート**に置き、`match self` を**網羅**（ワイルドカード無し）で書く。これによりバリアント追加時にコンパイルエラーで写像漏れを検知する。

```rust
// crates/web/src/error.rs — WebError と IntoResponse を同一クレートに置く
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug, thiserror::Error)]
pub enum WebError {          // ← ここでは #[non_exhaustive] を付けない
    #[error("unauthorized")] Unauthorized,     //   （同一クレート網羅 match を保つため）
    #[error("forbidden")]    Forbidden,
    #[error("not found")]    NotFound,
    #[error("invalid request: {0}")] Validation(String),
    #[error("upstream unavailable")] Upstream,   // Gemini/Discord/synapse 障害の丸め
    #[error("internal error")]       Internal,   // DbError/RepoError 等の機微を隠蔽
}

impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        // 網羅 match（_ アームを書かない）→ バリアント追加漏れ = コンパイルエラー。
        let (status, client_msg) = match self {
            WebError::Unauthorized  => (StatusCode::UNAUTHORIZED, "unauthorized"),
            WebError::Forbidden     => (StatusCode::FORBIDDEN, "forbidden"),
            WebError::NotFound      => (StatusCode::NOT_FOUND, "not found"),
            // Validation のみ Display を露出してよい（ユーザー入力由来・機微なし）。
            WebError::Validation(ref m) => {
                return (StatusCode::BAD_REQUEST, m.clone()).into_response();
            }
            WebError::Upstream      => (StatusCode::BAD_GATEWAY, "upstream unavailable"),
            // 内部エラーは Display をクライアントへ漏らさず "internal" に丸める。
            WebError::Internal      => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        };
        (status, client_msg).into_response()
    }
}
```

内部エラー（`DbError`/`RepoError` 等）から `WebError` への写像は**明示関数**で行い、`Db(_)` 系は必ず `WebError::Internal` に丸めて Display を漏らさない:

```rust
fn map_repo(e: RepoError) -> WebError {
    match e {
        RepoError::NotFound { .. }     => WebError::NotFound,
        RepoError::UserScopeMissing    => WebError::Forbidden,
        RepoError::Db(_)               => WebError::Internal, // 機微を丸める
    }
}
```

落とし穴（一次確認済み）:
- ハンドラ戻り値を `Result<impl IntoResponse, E>` にすると `?` の型推論が壊れやすい。**戻り値は具象化**（`Result<Json<T>, WebError>` 等）する。
- axum は **0.8.9**（`#[async_trait]` 不要・RPITIT）。パターンは 0.7/0.8 共通。詳細な extractor 設計は [axum-web-runtime](../verification/rpt-axum-web-runtime.md) 参照。

### 4.6 機械的強制の完全レシピ（コピペ可）

検証環境: Rust stable **1.96.1** / cargo-deny **0.19.9**。以下は一次ソース（clippy 生 HTML / Cargo Book / cargo-deny 公式テンプレート）で検証済み。

**(a) root `Cargo.toml` — restriction lint を 8 個「個別に」deny**

```toml
[workspace.lints.clippy]
# 注意: [workspace.lints.clippy] 内では clippy:: プレフィックス無しの裸の lint 名。
# 8 個すべて restriction グループ・デフォルト allow。個別列挙が必須。
unwrap_used        = "deny"
expect_used        = "deny"
panic              = "deny"
todo               = "deny"
unimplemented      = "deny"
unreachable        = "deny"
indexing_slicing   = "deny"
panic_in_result_fn = "deny"
```

> **一括禁止しない理由**: `restriction` グループの全体有効化（`clippy::restriction = "deny"`）は**公式が明確に禁止**している。グループには相互矛盾する lint が含まれ、`blanket_clippy_restriction_lints` 専用 lint で警告される。**必ず 8 個を個別列挙**する。`panic` 単独 lint は `unreachable!` 等を捕捉しないため、8 個併用で初めてパニック経路が網羅される。

**(b) 各メンバー crate の `Cargo.toml` — workspace lint をオプトイン**

```toml
[lints]
workspace = true
```

> **メンバーは自動継承しない**（Rust 1.74 安定・RFC 3389）。新規 crate 追加時にこの 2 行を忘れると workspace lint が丸ごと無視される。かつ `[lints] workspace = true` と個別 lint を**同一テーブルに併記するのはハードエラー**。CI で「全メンバーがこの行を持つか」を検査する仕組みを併設すると安全。

**(c) `clippy.toml` — テストコードの緩和**

```toml
# unwrap_used / expect_used / indexing_slicing はテストでも発火する。
# テスト内に限り緩和（本番コードは deny のまま）。
allow-unwrap-in-tests           = true
allow-expect-in-tests           = true
allow-indexing-slicing-in-tests = true
```

**(d) `deny.toml` — anyhow/eyre/color-eyre を `[bans]` で全面 BAN**

```toml
[bans]
multiple-versions = "warn"
wildcards         = "allow"

# フィールド名は現行の `crate`（PackageSpec）。旧 `name`/`version` 形式は非推奨。
# eyre を BAN しても color-eyre は別 crate 名なので個別に列挙する。
deny = [
    { crate = "anyhow",     reason = "banned: 具体列挙型(thiserror)のみ許容。型消去禁止" },
    { crate = "eyre",       reason = "banned: dynamic error-context crate 禁止" },
    { crate = "color-eyre", reason = "banned: eyre 派生" },
    { crate = "backoff",    reason = "banned: RUSTSEC-2025-0012 非メンテ。backon を使用" },
]
```

> `[bans]` は依存グラフ（`cargo metadata`）全体を見るので、推移的依存に anyhow 等が混入しても捕捉する（意図通り）。本当に必要な wrapper 経由の利用を許すなら `wrappers = [...]` を使うが、今回は全面 BAN なので不要。`backoff` の BAN は §5.3 と連動（RUSTSEC-2025-0012）。

**(e) CI ゲーティングコマンド**

```bash
# clippy: 全ターゲット・全フィーチャで警告をエラー化（manifest の deny と二重化）
cargo clippy --all-targets --all-features -- -D warnings

# cargo-deny: 全チェック（advisories/bans/licenses/sources）
cargo deny check
# anyhow/eyre/backoff ゲートだけ高速に回すなら bans 単独（ネットワーク不要）:
cargo deny check bans
```

CI 落とし穴:
- **`cargo build` では clippy lint は発火しない**。必ず `cargo clippy` を回す（manifest の `[workspace.lints.clippy]` は clippy 実行時に効く）。
- `--all-targets` は **doctest を含まない**。doctest 内の `unwrap` も塞ぐなら別途 `cargo test --doc` 系／`RUSTDOCFLAGS` を検討。
- `cargo deny check` の `advisories` はネットワークが要る。`bans` はネットワーク不要なので、握り潰し crate ゲートは `cargo deny check bans` に分離して高速化できる。

詳細は [clippy-cargodeny](../verification/rpt-clippy-cargodeny.md) を参照。

### 4.7 `panic = "unwind"` を保つ理由（セクション5との接続）

Cargo profile を `panic = "abort"` にすると、**tokio の spawn 境界によるパニック隔離が無効化され、単一タスクの panic でプロセスが即死**する。これは絶対制約2（些細な障害で落ちない）と真っ向から対立する。したがって**全プロファイルで `panic = "unwind"`（デフォルト）を厳守**する。

これが §5 の supervisor の前提: spawn したタスクの panic はプロセスを殺さず `JoinError(is_panic)` として観測でき、それを検知して**指数バックオフで再起動**できる。restriction lint（§4.6a）で明示的な `panic!`/`unwrap`/`unreachable!` はコンパイル時に排除しつつ、**それでも起きうる予期せぬ panic を実行時に unwind で受け止めて再起動する**——この二段構えが自己復帰の土台になる。

---

## 5. 自己復帰・スーパーバイザ設計（常時稼働）

**目的（絶対制約2）**: 些細な障害でプロセス全体を落とさない。全長寿命サービスを監督下に置き、panic/失敗を検知して**指数バックオフで個別再起動**、外部依存には**サーキットブレーカ**、依存断時は**劣化縮退**。fail-fast は**起動時の config/secret 不備のみ**。

検証環境: tokio **1.52.x** / tokio-graceful-shutdown **0.19.3** / backon **1.6.0** / recloser **1.4.0**。全て一次ソース照合済み（[resilience](../verification/rpt-resilience-tokio-backon-recloser.md)）。

### 5.1 監督ツリー（supervision tree）の対象

以下の全長寿命サービスを**監督下タスク**にする。各々が独立に落ち・独立に再起動される（1 つの障害が他へ波及しない）:

- **web**（axum サーバ）
- **discord: テナント別ゲートウェイ接続**（twilight `Shard` の caller 駆動 poll loop を各テナント 1 タスク）
- **synapse**（IPC クライアント）
- **crawler**
- **各 cron ジョブ**（スケジュール実行のループ）
- **redis**（セッションストア接続）

Discord がテナント別に独立タスクなのは、twilight の caller 駆動 poll loop を supervisor と統合し**テナント別バックオフ／再起動**を掛けられるため（[discord](../verification/rpt-discord-serenity-twilight.md)、[decisions §Discord](../00-decisions.md)）。

### 5.2 自前 JoinSet supervisor + join_next ループ

**確定した土台事実（一次確認）**: `tokio::spawn` したタスク内の panic は**プロセスを殺さず当該タスクに隔離**され、`JoinHandle`／`JoinSet::join_next()` が `Err(JoinError)` を返す。`JoinError::is_panic()` が `true`、`into_panic()` でペイロードを取得できる。親へ伝播するかは**完全にプログラマ制御**（`resume_unwind` を呼ばない限り伝播しない）。→ `join_next` ループで検知して再 spawn する設計が成立する。

**設計**: `tokio::task::JoinSet`（標準 API・`rt` フィーチャのみ・unstable 不要）を `join_next()` で回し、`Err(JoinError)` を検知したら**該当サービスを指数バックオフで再 spawn** する自前スーパーバイザ。

厳守事項（一次確認済みの落とし穴）:
- **`JoinSet::join_all()` は使わない**。docs 明記: 1 つでも `JoinError` で失敗すると `join_all` は **panic し残り全タスクを cancel** する。監視では必ず `join_next()` を手動ループして各エラーを個別処理する。
- **`JoinSet` を drop すると配下タスクが即 abort**。supervisor 本体の生存を別レイヤで保証する（supervisor が落ちれば配下全滅）。
- **`panic = "unwind"` 必須**（§4.7）。`abort` だと隔離が無効化。
- `catch_unwind` を `.await` 跨ぎで使わない（`UnwindSafe` 制約で破綻しやすい）。パニック隔離は spawn 境界に委ねる。

```rust
use std::time::Duration;
use tokio::task::{JoinSet, JoinError};
use backon::{ExponentialBuilder, BackoffBuilder};

/// 監督対象サービスの識別子。再起動時にどれを起こし直すか判別する。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Service {
    Web,
    Discord(TenantId),
    Synapse,
    Crawler,
    Cron(CronId),
    Redis,
}

/// 各サービスの本体（長寿命ループ）。Ok(()) は「意図的停止」、
/// Err はサービス固有の回復可能エラー。panic は JoinError として捕捉される。
async fn run_service(svc: Service, ctx: AppCtx) -> Result<(), ServiceError> {
    match svc {
        Service::Web           => web::serve(ctx).await,
        Service::Discord(t)    => discord::run_tenant(t, ctx).await,
        Service::Synapse       => synapse::run(ctx).await,
        Service::Crawler       => crawler::run(ctx).await,
        Service::Cron(id)      => cron::run(id, ctx).await,
        Service::Redis         => redis::run(ctx).await,
    }
}

/// supervisor ループ: join_next で終了/panic を検知し、指数バックオフで再 spawn。
async fn supervise(services: Vec<Service>, ctx: AppCtx, shutdown: ShutdownGuard) {
    let mut set: JoinSet<(Service, Result<(), ServiceError>)> = JoinSet::new();

    // 各サービスの再起動ごとの backoff 状態を保持（サービス別に独立）。
    let mut backoff: std::collections::HashMap<Service, _> = Default::default();

    let spawn_one = |set: &mut JoinSet<_>, svc: Service, ctx: AppCtx| {
        set.spawn(async move {
            // run_service 内の panic はここで JoinError 化される（プロセスは死なない）。
            (svc, run_service(svc, ctx).await)
        });
    };

    for svc in services.iter().copied() {
        spawn_one(&mut set, svc, ctx.clone());
    }

    while let Some(joined) = set.join_next().await {
        // 停止協調中なら再起動しない（§5.4）。
        if shutdown.is_shutting_down() {
            continue;
        }

        let svc = match joined {
            // 正常な JoinResult（サービスが Ok/Err を返して終了）。
            Ok((svc, Ok(()))) => {
                // 意図的停止: cron 一巡完了など。ポリシーに応じ再起動 or 放置。
                svc
            }
            Ok((svc, Err(e))) => {
                tracing::warn!(?svc, error = %e, "service returned recoverable error");
                svc
            }
            // タスクが panic した（is_panic）。プロセスは生きている。
            Err(join_err) if join_err.is_panic() => {
                // JoinError には Service が乗らないため、id() 等で対応付ける実装にする。
                let svc = resolve_service_of(&join_err);
                tracing::error!(?svc, "service PANICKED, isolated by spawn boundary");
                svc
            }
            // abort されたタスク（drop 等）。停止協調なら上で continue 済み。
            Err(_aborted) => continue,
        };

        // サービス別の指数バックオフ（ジッタ明示）で再 spawn。
        let bo = backoff
            .entry(svc)
            .or_insert_with(|| {
                ExponentialBuilder::default()
                    .with_jitter()                       // ← 明示 ON 必須（thundering herd 回避）
                    .with_min_delay(Duration::from_millis(200))
                    .with_max_delay(Duration::from_secs(30))
                    .build()
            });
        let delay = bo.next().unwrap_or(Duration::from_secs(30));
        tracing::info!(?svc, ?delay, "restarting service after backoff");
        tokio::time::sleep(delay).await;

        spawn_one(&mut set, svc, ctx.clone());
    }
}
```

> 実装メモ: `JoinError` にサービス識別子は乗らないので、`set.spawn` の `AbortHandle`/task id とサービスの対応表を別に持ち `resolve_service_of` で引く（上の擬似コードはその存在を前提にしている）。サービスが**安定稼働したら backoff をリセット**する（一定時間 Err なく回ったら `backoff.remove(&svc)`）と、断続障害で遅延が際限なく伸びるのを防げる。

### 5.3 リトライ: backon 1.6.0（ジッタ明示）と backoff 採用禁止

- **`backon 1.6.0`** を retry + 指数バックオフ + ジッタの第一選択とする。async/blocking 両対応、指数/定数/フィボナッチ、Retry-After 動的バックオフ、no-std/wasm 対応。
- **ジッタはデフォルト無効の戦略があるため `ExponentialBuilder::default().with_jitter()` を明示的に ON** にする（thundering herd 回避に必須）。§5.2 の supervisor でも §5.5 の外部依存リトライでも同様。
- **`backoff` クレートは採用禁止**: **RUSTSEC-2025-0012**（2025-03-07 発行）で公式に "no longer actively maintained" と宣言され、**代替として `backon` が明示推奨**されている。最終版 0.4.0 は 2021-12-14 で更新停止。`cargo audit` / `cargo deny check advisories` が警告を出す。→ §4.6(d) の `deny.toml` で crate 名 BAN 済み。

Gemini リトライは `GeminiError::RateLimited { retry_after }`（§4.2）の `retry_after` を backon の動的バックオフに渡し、現行の rate-limit 挙動を 1:1 で再現する。

### 5.4 tokio-graceful-shutdown 0.19.3 による停止協調（SIGTERM 伝播）

- **`tokio-graceful-shutdown 0.19.3`** をサブシステムツリー＋graceful shutdown 伝播に採用。SIGTERM 受信を各サブシステムへ伝播し、順序立てた停止を行う。
- **ただし「再起動」ロジックは自前補完**である点を明記する。tgs が提供するのは主に「**停止協調 + エラー伝播**」であり、Erlang 的な individual restart supervisor ではない（subsystem がエラー/panic したら**ツリーを畳んで graceful shutdown** する型）。→ **常時稼働のための個別再起動は §5.2 の自前 JoinSet supervisor が担い、tgs は停止フェーズの協調に用いる**、という役割分担にする。
- 0.x 系ゆえマイナー更新（0.17→0.18→0.19）で API 破壊があり得る。**バージョンをピン留め**し、更新時は CHANGELOG を確認する。
- supervisor ループは `ShutdownGuard`（上の擬似コードの `shutdown`）を参照し、停止協調中は**再起動しない**（`is_shutting_down()` で continue）。

### 5.5 外部依存ごとのサーキットブレーカ

外部依存（Gemini / Google / Discord）ごとに独立したサーキットブレーカを置き、連続失敗時に**即 fail（fast-fail）してバックオフ再試行の嵐を止める**。

- **`recloser 1.4.0`**: リングバッファ実装の並行サーキットブレーカ。Closed/Open/HalfOpen の 3 状態、`RecloserBuilder` で失敗率・バッファ長を設定、`AsyncRecloser` で futures 対応（`recloser.call(future)`）。2026 年に継続リリースされている本格クレート。
- **または自前 `AtomicU*` 状態機械**: ブレーカ生態系は backon ほど成熟しておらず、要件がシンプル（失敗率閾値＋open タイマ）なら数十〜百数十行の自前実装が**対等な選択肢**。0.x/準放置リスクを負いたくない場合はこちらが堅い（`failsafe` は機能成熟だが約 2 年更新停止のため非推奨）。
- 落とし穴: recloser の `AsyncRecloser` は「futures-aware」だが docs 上 **tokio 明示保証はない**（標準 futures で動作）。採用前に自タスク構成で軽く PoC 検証する。

ブレーカとリトライの合成: 各外部依存呼び出しは「**ブレーカ（open なら即エラー） → backon リトライ（ジッタ ON） → GeminiError/DiscordError で構造化**」の順に重ねる。ブレーカが open の間は backon を回さず即座に縮退（§5.6）へ落とす。

### 5.6 劣化縮退（degraded operation）の具体

依存が落ちても致命扱いにせず、機能を縮退して稼働を継続する:

- **synapse ダウン → 直近履歴のみで応答**（現行踏襲）。`IpcError` を捕捉し、フル履歴取得を諦めてローカルの直近分で継続する。
- **Redis ダウン → インメモリセッションへフォールバック**。`ServiceError`（redis 系）を捕捉し、プロセスローカルの in-memory ストアで受け付ける。
  - ⚠️ **移行期リスクの注記**: in-memory セッションは**プロセスローカルで他系（旧 Node バックエンド・他インスタンス）から不可視**。移行期は新旧バックエンドが共有 Redis の不透明トークンを参照する設計（[nginx-session-strangler](../verification/rpt-nginx-session-strangler.md)、[decisions §デプロイ](../00-decisions.md)）なので、Redis 断中の in-memory フォールバックは**断続ログアウト／セッション不一致**を生む。縮退はあくまで「完全ダウンよりまし」の一時措置と位置づけ、Redis 復帰を最優先で監視・再接続する。

### 5.7 「回復可能／致命的」の型・ポリシー境界

**fail-fast は起動時の config/secret 不備のみ**。それ以外は全て回復可能として supervisor/縮退で扱う。

- **致命的（fail-fast）= `ConfigError`（起動時）のみ**: config.yaml の欠落・型不一致・必須 secret 不備は、起動シーケンスで即座にプロセス終了（非ゼロ終了）させる。壊れた設定で中途半端に動くより即死が安全。secret は専用 DTO struct（機密をフィールドに存在させない）＋`secrecy` で扱う（[typegen](../verification/rpt-typegen-tsrs-utoipa.md)、[decisions §機密](../00-decisions.md)）。
- **回復可能 = 上記以外すべて**: `DbError`(BUSY 等)/`RepoError`/`AuthError`/`ValidationError`/`GeminiError`/`DiscordError`/`PluginError`/`IpcError`/`WebError`。これらは supervisor 再起動・backon リトライ・ブレーカ・劣化縮退・HTTP 4xx/5xx 化のいずれかで吸収し、**プロセス全体は落とさない**。

型・ポリシー境界の表現方針:
- 起動シーケンス（`main`）だけが `ConfigError` で早期 return して終了できる。**長寿命サービスのループ内では `ConfigError` を発生させない**（設定は起動時に検証済みの型付き構造体として渡す）。
- サービスループの戻り値型 `Result<(), ServiceError>` は**回復可能エラーのみ**を表す（致命エラーを混ぜない）。これにより「supervisor が受け取るエラー＝必ず再起動/縮退で対処可能」という不変条件を型で保証する。

```rust
// main の起動シーケンス: ここだけが fail-fast。
fn main() -> std::process::ExitCode {
    // config/secret 不備は即終了（唯一の致命ポイント）。
    let cfg = match Config::load_and_validate() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("fatal: config error: {e}");   // Display のみ、機微は出さない
            return std::process::ExitCode::FAILURE;
        }
    };
    // 以降はランタイムを起動し、supervise() へ。ここから先は「落ちない」。
    run_runtime(cfg)
}
```

---

### 参照レポート（一次ソース照合）
- [rpt-errors-thiserror](../verification/rpt-errors-thiserror.md) — thiserror 2.0.18 / 属性構文 / non_exhaustive / IntoResponse
- [rpt-clippy-cargodeny](../verification/rpt-clippy-cargodeny.md) — restriction lint 8 個 / workspace.lints / deny.toml bans / CI
- [rpt-resilience-tokio-backon-recloser](../verification/rpt-resilience-tokio-backon-recloser.md) — JoinSet 隔離 / tgs / backon / recloser / backoff 禁止
- 上位決定: [00-decisions.md](../00-decisions.md)（絶対制約1・2、§エラー処理、§自己復帰）
