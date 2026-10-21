# D06: 外向き URL/IP 検査の共通化（#130）

基準は `origin/develop` の `662f46a`。#130 本文・全コメント（0件）、親 #97、R25 の #156 本文・全コメントと `e36cf98` / `3c49934` / `d980503`、旧 `bb97bae5:src/utils/ssrfGuard.ts`、現行の通信経路を確認した。R25 は `893d792` でマージ済みであり、DNS 検査や RSS リダイレクト処理を再実装する必要はない。

## 責務と依存方向

`yuuka-outbound` は std / tokio / url だけに依存する下位クレートとし、browser / MCP / briefing が直接利用する。core の契約層へネットワーク依存を持ち込まず、briefing → browser（Chromium 等）の依存も取り除く。browser の公開済み `assert_safe_outbound_url` は互換 re-export を残す。

- `assert_safe_outbound_url(&str) -> Result<(), String>`: 通信時の厳密検査。既存と同じ URL パーサーで正規化し、http(s)・userinfo・localhost・全 DNS 解決先・IPv4/IPv6 を検査する。エラー文言と検査順を維持する。成功結果をキャッシュせず、呼び出しごとに再解決する。allowlist やガードを迂回する引数は設けない。
- `is_likely_public_http_url(&str, RegistrationPolicy::PublicHttp) -> bool`: 登録時は DNS を引かず、`url::Url` で正規化して通信時と同じ内部/予約 IP 範囲を拒否する。#130 では互換用の `BriefingLegacy` を残したが、#184 対応で差し替えた。`127.1`、整数 IPv4、IPv4-mapped IPv6、TEST-NET、CGNAT も登録時に拒否する。公開 IP・ドメイン名は許可し、URL パーサーの扱いに従って大文字 scheme は許可、不正ポートは拒否する。
- HTTP の生成・接続・認証・タイムアウト・再試行・リダイレクト・エラーの利用者向け変換は呼び出し元が引き続き担当する。IP 判定実装は共通クレートだけに置き、登録時・通信時で同じ関数を使う。

## 通信・リダイレクト経路の棚卸し

| 経路 | 検査と既存動作（今回維持） |
|---|---|
| briefing 設定の HTTP / configureBriefing | tools の薄いラッパーから登録時ポリシーを適用。通信許可とは区別する。 |
| briefing RSS (`service::fetch_news` → `fetch_feed`) | 各取得直前に共通厳密検査。自動 redirect は無効。`resp.url().join(Location)` で相対先を解決し、各ホップを再検査して最大5回追従。拒否・回数超過時の warn、個別失敗を飛ばす契約、最大10フィードも維持。 |
| browser fetchDynamicPage | tool 入口で厳密検査 → `search::fetch_html`（静的取得、最大3試行、redirect 最大10回）→ 必要なら Chromium `--dump-dom`。共通化前から静的 redirect 先・Chromium 内部の再検査はない。 |
| browser takePageScreenshot | tool 入口で厳密検査 → Chromium CLI。遷移・サブリソースは Chromium が処理する。 |
| browserInteractiveOpen | tool 入口で厳密検査 → `BrowserManager::open` → CDP goto。click/type/wait/status/fillCredential は既存セッションを利用し、遷移・サブリソースの独立した SSRF 検査はない。 |
| searchWeb | Google / DuckDuckGo の固定検索 URL → `search::fetch_html`。検索語はエンコードし、リンク先本文はここでは取得しない。 |
| MCP tools/list・tools/call・initialize・notifications/initialized | 共通 `rpc` の HTTP POST 直前に厳密検査。セッション再初期化・再試行も同じ経路。 |
| MCP dashboard probe / HTML | endpoint の origin からパスを生成 → `authed_get` の GET 直前に厳密検査。probe は失敗時 false、HTML は Err。 |
| MCP proxy | POST 直前に厳密検査。Bearer / session-id とエラー契約を維持。MCP の HTTP クライアントは supervisor から注入され、自動 redirect は既定動作のまま。 |
| MCP dashboard CSS | `fetch_akizakura_css` の既定は None（実取得なし）。HTML 中の link 等は閲覧ブラウザが取得する。 |
| briefing 天気 | Open-Meteo 固定 endpoint に数値の緯度経度を渡す。専用 reqwest client の既定 redirect。 |
| Google OAuth / Calendar / Drive backup | Google 固定 endpoint（ID 等はエンコード）と注入された reqwest client。既存 SSRF ガードの重複はない。 |
| Gemini（会話・要約・Webhook LLM・シナプス等） | 既定 Google API または管理側で構成する base_url。専用 reqwest client。既存 SSRF ガードの重複はない。 |
| account 認証 | 設定された内部 account URL の `/api/services/yuuka/exchange` を取得する。内部通信を意図する経路。 |
| Discord | twilight の固定 API / gateway と、Discord Attachment の URL を `reqwest::get` で取得する経路。後者は既定 redirect、既存 SSRF 検査なし。 |
| supervisor の疎通監視 | 固定 `127.0.0.1:<port>` の取得。内部通信を意図する経路。 |

上記の対象外経路に新たなガードや allowlist は追加しない。HTTP 応答としてクライアントへ返す OAuth / login 等の redirect はサーバー側 URL 取得とは区別した。

## 維持する保護と制約

IPv4 の private / loopback / link-local / TEST-NET / benchmark / CGNAT / multicast / reserved、IPv6 の loopback / unspecified / ULA / link-local / site-local / multicast / documentation と IPv4-mapped の内部 IPv4 判定を維持する。短縮・整数・16進・8進 IPv4 は既存の url crate の正規化後に検査する。IPv6 の受理範囲も共通化前と同じにする。

DNS 検査と実接続は別々に解決するため、TOCTOU は既存コード・旧 Node ともに残る（#156 のレビューでも指摘済み）。この API は「検査した IP への接続」を保証しない。解決結果のキャッシュや検査位置の前倒しをせず、RSS 各ホップ再検査も維持することで保護を弱めない。接続 IP の固定、browser / MCP の各 redirect、CDP ナビゲーションやサブリソース、プロキシ経由の接続を含めた完全な接続制御は、既存動作の維持を求める今回の共通化には含めない。

## #184 の追加対応（2026-10-22）

登録時の弱い IP 判定と独自パーサーを削除した。共通クレートで CGNAT・benchmark・TEST-NET・multicast/reserved・IPv4 射影・IPv6 site-local/documentation と特殊 IPv4 表記を拒否するテストを追加し、朝報設定 API は 400、configureBriefing ツールは success:false を返して設定を保存しないことを検証する回帰コードも追加した。取得時の DNS 解決先・リダイレクト検査は継続する。

`cargo fmt --all -- --check`、`cargo clippy -p yuuka-outbound -p yuuka-briefing --all-targets -j 2 -- -D warnings`、`pnpm --config.verifyDepsBeforeRun=false check` は成功（svelte-check 0 errors/warnings、Biome に既存警告あり）。pnpm は既存依存を利用し、インストール済み版とリポジトリ設定の差による実行前の依存検証だけを無効化した。CLAUDE.md の develop 以外での実ビルド禁止に従い、Rust テストは追加と型検査までで実行していない。

## #130 実装時の回帰確認

共通クレートに既存の IPv4/IPv6 レンジテストを移し、特殊表記・日本語エラー全文・公開 IP の受理、DNS の公開/内部混在（両順序）・IPv4-mapped 混在・空回答・解決失敗・呼び出しごとの再検査・IP リテラルでは DNS 不要、登録時と通信時の差を追加した。DNS の差し替えは非公開関数に限定し、本番 API に迂回口を公開しない。

browser の拒否テスト、MCP の拒否テスト、R25 の実 `fetch_news` 経由の内部接続0回・公開先の相対 redirect・上限超過テストは引き続き同じ本体を対象にする。CLAUDE.md に従い、この非 develop ブランチでは cargo fmt / check / clippy でテストコードも静的検証し、build / test / run / nextest は実行しない。通常の develop 向け PR の Rust CI は fmt / cargo-deny のみで、重い build / test ジョブはスキップされるため、CI 成功とテスト実行済みを混同しない。

実施結果: `cargo fmt --all -- --check`、`cargo check --locked -p yuuka-outbound -p yuuka-browser -p yuuka-mcp -p yuuka-briefing -p yuuka-credential --all-targets -j 2`、同じ対象の `cargo clippy --locked ... --all-targets -j 2 -- -D warnings` は成功。`git diff --check` も成功。旧 browser / MCP の検査実装が同一であること、および移行後の通信時検査本体・厳密 IP 範囲・登録パーサーが注入用 lookup と用途引数以外は旧実装と一致することを、コメント・空白を除く静的比較でも確認した。テストはコード追加・型検査までであり、このブランチ上では実行していない。
