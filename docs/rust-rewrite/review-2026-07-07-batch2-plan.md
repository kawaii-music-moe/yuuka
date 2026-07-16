# Batch 2 実装方針 — セキュリティヘッダ（H-2）＋静的配信のキャッシュ/404（M-3）

- 起票日: 2026-07-07
- 対象: [review-2026-07-06-fix-policy.md](review-2026-07-06-fix-policy.md) Batch 2（**H-2＋M-3**）
- 前提: Batch 1（`9c8688d`）承認済み。**Rust が SPA を配信し始める前**に潰すべき層（fix-policy §1）。
- 位置づけ: 実装着手前の方針。地上真実を Node 実装（`src/server.ts`）で実測確認済み。

---

## 0. スコープと完了条件

| 所見 | 内容 | 完了条件 |
|---|---|---|
| **H-2** | CSP・Referrer-Policy が全く付与されず Node のセキュリティヘッダ群から後退。SPA を Rust が配信し始めた瞬間に XSS 多層防御が消える | 全応答（静的＋API）に Node と同一値の CSP／Referrer-Policy を付与。HTTPS 本番のみ HSTS |
| **M-3** | 静的配信のキャッシュ/404 挙動差 3 点（(a) `/assets` 404 も immutable、(b) index.html 等に no-cache 無し、(c) 拡張子付き未存在も index.html フォールバック） | (a)(b)(c) を Node `serveStaticFile` と一致させる |

**却下/据え置き（誤修正の予防・本バッチ対象外）:**
- **google-site-verification 注入**（`<!-- GOOGLE_SITE_VERIFICATION -->` 置換）: 既存 deferred（[static_files.rs](../../crates/yuuka-web/src/static_files.rs) 冒頭）。SEO 用途・機能非依存。Batch 2 では index.html を素のまま配信し据え置く。
- **HTTP→HTTPS 301 リダイレクト**（Node `server.ts:257-272`）: strangler の **nginx がエッジで担う**（Rust はTLS終端後を受ける）。Rust バックエンドには実装しない。
- **動的圧縮 `CompressionLayer`**: 設計は**ビルド時プリコンプレス（.br/.gz サイドカー）**（PLAN §6.5・現行 ServeDir 実装）。動的圧縮は §6.6 の別項目で、レビュー所見（H-2/M-3）に含まれないため本バッチ非対象。
- **MCP ダッシュボードの隔離 iframe 専用 CSP**（Node `server.ts:74`・PLAN §6.6）: `mcpRoutes` 自体が Rust 未実装（deferred）。本体 CSP のみで足り、per-route CSP 上書きは不要。

---

## 1. 確定した地上真実（Node 実測）

### 1.1 セキュリティヘッダ（`src/server.ts:82-88`）

```
CSP = "default-src 'self'; script-src 'self' https://static.cloudflareinsights.com; \
style-src 'self' 'unsafe-inline' https://fonts.googleapis.com https://fonts.gstatic.com; \
font-src 'self' https://fonts.gstatic.com https://fonts.googleapis.com; \
img-src 'self' data: https://assets-global.website-files.com https://cdn.discordapp.com; \
connect-src 'self' https://cloudflareinsights.com; worker-src 'self'; frame-src 'self'; frame-ancestors 'self';"

SECURITY_HEADERS = {
  "X-Content-Type-Options": "nosniff",             // Rust: 付与済み
  "X-Frame-Options": "SAMEORIGIN",                 // Rust: 付与済み
  "Referrer-Policy": "strict-origin-when-cross-origin",   // ← 欠落（H-2）
  "Content-Security-Policy": CSP,                   // ← 欠落（H-2）
}
```

- **CSP は `script-src` から `'unsafe-inline'` を除外**した実効的 XSS 多層防御（`server.ts:79-81`）。値は PLAN §6.6 の定数と一致。**一字一句そのまま移植**。
- `style-src` の `'unsafe-inline'` はテンプレート内 `style=` のため維持（Node 踏襲）。
- **HSTS**（`server.ts:277-280`）: `config.baseUrl` が `https://` 始まりの時のみ、全応答に `Strict-Transport-Security: max-age=63072000; includeSubDomains`。preload は含めない。

### 1.2 静的配信の挙動（`serveStaticFile`・`src/server.ts:115-236`）

| 局面 | Node の挙動 |
|---|---|
| `/` / `/?...` | `/index.html` に正規化して配信（200） |
| パストラバーサル | `PUBLIC_DIR + sep` 前方一致で外れたら **403** |
| ファイル存在 | MIME 判定して 200（`MIME_TYPES` 表、無ければ `application/octet-stream`） |
| **未存在・拡張子なし** | **index.html を配信（200・SPA ルーティング）** |
| **未存在・拡張子あり** | **`404.html` を 404 で配信**（無ければ `"404 Not Found"` テキスト・404） |
| **キャッシュ（ハッシュ資産）** | `assets/` 配下 **かつ** 正規表現 `-[A-Za-z0-9_-]{8,}\.(js\|css\|woff2?\|png\|jpe?g\|svg\|webp)$` に一致 → `public, max-age=31536000, immutable` |
| **キャッシュ（それ以外）** | index.html/theme-init.js/manifest.json/sw.js/404.html 等 → **`no-cache, no-store, must-revalidate`** |
| セキュリティヘッダ | 200 応答（および index フォールバック）に `SECURITY_HEADERS` を付与。403/404 は付与しない |

- 実ファイル確認: `dist/public/` に `index.html`・`404.html`・`sw.js`・`theme-init.js`・`manifest.json`・`assets/`（Vite ハッシュ資産）・`icons/`・`materials/` が実在。
- **M-3(c) の実害**: デプロイで削除された `/old-abc.js` や更新中の `/sw.js` が、拡張子ありにもかかわらず index.html を 200 で返すと、SW 更新検知・欠落検知が静かに壊れる。Node は拡張子ありは 404。

### 1.3 Rust 現状（差分の所在）

- [crates/yuuka-web/src/lib.rs](../../crates/yuuka-web/src/lib.rs) `apply_common_layers`: `nosniff` と `X-Frame-Options: SAMEORIGIN` のみ。**CSP/Referrer/HSTS 無し**（H-2）。全応答（API＋静的）を包む位置にある。
- [crates/yuuka-web/src/static_files.rs](../../crates/yuuka-web/src/static_files.rs) `mount_static`: `/assets` を immutable で nest（**404 にも immutable が乗る＝M-3(a)**）＋ spa `ServeDir` の `.fallback(ServeFile::index)` が**全未存在パスを index.html にフォールバック（拡張子ありも＝M-3(c)）**。**no-cache を一切付けない（index.html 等＝M-3(b)）**。
- [crates/yuuka-supervisor/src/lib.rs](../../crates/yuuka-supervisor/src/lib.rs) `build_app`: `mount_static(routes, dir)` → `apply_common_layers(routes).with_state(state)`。**共通レイヤが静的配信も含め全応答を包む**ため、CSP を `apply_common_layers` に足せば静的にも乗る（PLAN §6.6 の意図と一致）。
- [crates/yuuka-web/src/state.rs](../../crates/yuuka-web/src/state.rs) `AppState.config: Arc<WebConfig>`、`WebConfig.https = is_https_deployment()`（[config.rs](../../crates/yuuka-web/src/config.rs)）で HTTPS 判定済み。HSTS のゲートに使える。

---

## 2. 実装設計

### 2.1 H-2 — セキュリティヘッダを `apply_common_layers` へ集約

- `apply_common_layers` に **CSP** と **Referrer-Policy** の `SetResponseHeaderLayer::overriding` を無条件追加（既存の nosniff / X-Frame-Options と同列）。CSP 定数は Node 値をそのまま `const CSP: &str`。
- **HSTS は config ゲート**（`https == true` の時だけレイヤ追加）。`apply_common_layers` は現状 config を受け取らないため、**シグネチャを `apply_common_layers<S>(router, https: bool)` に変更**する。
  - 呼び出し側 `build_app`（supervisor）: `apply_common_layers(routes, state.config.https)` に変更（`state.config` は `with_state` 前に読める）。
  - `build_router`（web/lib.rs）・各テストの呼び出しも更新（テストは `false` 既定でよい）。
  - HSTS 値: `max-age=63072000; includeSubDomains`。`if_not_present` 相当（既存を壊さない）。
- **配置理由**: `apply_common_layers` は API＋静的の全応答を包む。CSP/Referrer を全応答に乗せるのは PLAN §6.6（「SetResponseHeaderLayer が全レスポンスに乗せる」）と一致し、既存の nosniff/X-Frame 付与パターンとも整合。Node が 403/404 に CSP を付けない点との差は**安全側（防御強化）で、実害なし**。

### 2.2 M-3 — `mount_static` を Node `serveStaticFile` 準拠へ再設計

現行の「`/assets` immutable nest ＋ 全未存在 index フォールバック」を、**単一 `ServeDir` ＋ カスタムフォールバック ＋ キャッシュ制御ミドルウェア**へ置き換える（Node の単一 `serveStaticFile` 関数構造に対応）。

**(1) カスタムフォールバック（未存在時の分岐・M-3(c)）**
`ServeDir::new(dist_dir).precompressed_br().precompressed_gzip().fallback(<handler>)` の `<handler>` を、Node の未存在ロジックに一致させる:
- リクエストパス末尾セグメントに**拡張子なし** → `index.html` を **200** で返す（SPA ルーティング）。
- **拡張子あり** → `404.html` を **404** で返す（無ければ `"404 Not Found"`・404）。
- 拡張子判定は `std::path::Path::new(uri.path()).extension().is_some()`（Node `path.extname` と同じく末尾セグメント基準：`/dashboard/tasks`→なし、`/sw.js`→あり）。
- handler は `dist_dir`（`PathBuf` 複製）を保持し index.html/404.html を読む。

**(2) キャッシュ制御ミドルウェア（(a)(b) の出し分け）**
静的サービスのみに被せる `axum::middleware::from_fn` で、**リクエストパス＋応答ステータス**から `Cache-Control` を決定:
```
is_hashed = パスが /assets/ 配下 かつ Node 正規表現に一致
Cache-Control = (status == 200 && is_hashed) ? "public, max-age=31536000, immutable"
                                             : "no-cache, no-store, must-revalidate"
```
- これで 3 バグを一括是正:
  - **(a)** `/assets/xxx-<hash>.js` が未存在 → フォールバックで拡張子あり → **404** → `status != 200` で immutable にならない（no-cache）。
  - **(b)** `index.html`/`manifest.json`/`sw.js`/`theme-init.js` → `/assets` 外で非ハッシュ → **no-cache, no-store, must-revalidate**。
  - **(c)** は (1) のフォールバックで是正。
- `from_fn` は `Request`（axum Body）→ `Next` の形。ServeDir 応答ボディ（`ServeFileSystemResponseBody`）を `map_response(|r| r.map(Body::new))` で axum `Body` へ変換して合流させる（tower の型整合はここで吸収）。**この配線は静的サービス内のみ**に適用し、API ルートには `Cache-Control` を付けない（Node の API 応答は Cache-Control 無し＝parity）。
- **ハッシュ判定は手書き（`regex` 依存を追加しない）**。`regex` は未依存で、追加は `cargo deny` 審査＋コンパイル時間増を招くため。Node 正規表現 `-[A-Za-z0-9_-]{8,}\.(js|css|woff2?|png|jpe?g|svg|webp)$`（`assets/` 配下前提）を次で厳密移植:
  - `/assets/` 前方一致。末尾セグメントのファイル名を取る。
  - 拡張子（最後の `.` 以降）が `{js,css,woff,woff2,png,jpg,jpeg,svg,webp}` のいずれか。
  - **最初の `-`**（`find('-')`）以降・拡張子直前までの文字列が **8 文字以上** かつ全て `[A-Za-z0-9_-]`。
  - 「最初の `-`」を使うのは Node 正規表現の貪欲マッチ（charset に `-` を含む）と一致させるため（例 `foo-12345678-bar.js` は Node/本実装とも immutable、末尾 `-` 起点だと誤判定する）。
- **既存テスト修正が必要**: 現行 `serves_hashed_asset_with_immutable_cache` は `app-abc123.js`（ハッシュ **6 文字**）を使うが、Node 正規表現 `{8,}` には**一致しない**（実 Vite ハッシュは 8 文字＝`BIDAdKj3`/`Dpmkag8p` 等）。M-3 修正後はこれが no-cache になるため、**テストのフィクスチャを 8 文字ハッシュ**（例 `app-BIDAdKj3.js`）へ更新し、6 文字（非ハッシュ扱い＝no-cache）のケースも 1 件追加して境界を凍結する。

**(3) 据え置き**: `/` → index.html は `ServeDir` の `append_index_html_on_directories`（既定 true）で従来通り。google-site-verification 注入は deferred（index.html を素のまま）。

**方針上の小さな逸脱（明示）**: Node は 404/403 に `Cache-Control` を**付けない**が、本設計は 404 に `no-cache, no-store, must-revalidate` を付ける。immutable 化を防ぐ M-3(a) の目的を満たしつつ、404 のキャッシュを積極的に抑止する**安全側**の差分。実害なし。

---

## 3. 検証（完了条件のテスト化）

`static_files.rs` の `mod tests`（既存 3 件は維持・必要なら assert 追加）＋ `lib.rs` の `apply_common_layers` テストに追加:

| 検証 | 期待 |
|---|---|
| ハッシュ資産 `/assets/app-BIDAdKj3.js`（実在・8文字ハッシュ） | 200 ＋ `Cache-Control: public, max-age=31536000, immutable` |
| 非ハッシュ `/assets/app-abc123.js`（6文字・境界） | 200 ＋ no-cache（Node `{8,}` 不一致＝immutable でない） |
| **`/assets/missing-BIDAdKj3.js`（未存在）** | **404 ＋ immutable でない**（no-cache）— M-3(a) |
| **`/index.html`・`/manifest.json`** | 200 ＋ **`no-cache, no-store, must-revalidate`** — M-3(b) |
| 拡張子なし `/dashboard/tasks` | 200 ＋ index.html 本体 ＋ no-cache（SPA） |
| **拡張子あり未存在 `/old-abc.js`・`/sw-missing.js`** | **404**（index.html を返さない）— M-3(c) |
| `/api/me`（API） | 静的キャッシュ層の影響を受けない（Cache-Control 無し） |
| **CSP/Referrer** | 静的応答・API 応答の両方に Node と同一値で付与 — H-2 |
| **HSTS** | `https=true` で付与・`https=false`（既定）で**非付与** |
| 既存 | `api_route_is_not_shadowed_by_static`（`/api/*` を食い潰さない）維持 |

**機械検査ゲート（Batch 1 と同基準）**: `cargo test --workspace` 全通過（97→追加分・リグレッション無）／`cargo clippy --all-targets --all-features -- -D warnings` exit 0／`gen-types --check` ドリフト無（本バッチは DTO 非変更のため影響無）。fmt は Batch 7。

---

## 4. 影響ファイル（見込み）

- `crates/yuuka-web/src/lib.rs` — `apply_common_layers` に CSP/Referrer/HSTS 追加＋シグネチャに `https: bool`。CSP 定数追加。ヘッダテスト追加。
- `crates/yuuka-web/src/static_files.rs` — `mount_static` 再設計（カスタムフォールバック＋キャッシュ層）。M-3 テスト追加。
- `crates/yuuka-supervisor/src/lib.rs` — `build_app` の `apply_common_layers` 呼び出しに `state.config.https` を渡す。既存 build_app テスト調整。
- 新規依存は**追加しない**（ハッシュ判定は手書き）。

---

## 5. 参照

- fix-policy: [review-2026-07-06-fix-policy.md](review-2026-07-06-fix-policy.md)（Batch 2）
- Batch 1 レビュー: [review-2026-07-07-batch1.md](review-2026-07-07-batch1.md)
- 設計: [PLAN.md](PLAN.md) §6.5（静的配信）§6.6（セキュリティヘッダ）
- Node 実装: [src/server.ts](../../src/server.ts) `:82-88`（ヘッダ）`:115-236`（serveStaticFile）`:257-280`（HSTS/リダイレクト）
