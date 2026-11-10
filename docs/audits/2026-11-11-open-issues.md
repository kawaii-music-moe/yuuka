# Open Issue の個別確認（2026-11-11）

対象は確認開始時の Open Issue #233〜#251（19 件）。本文・コメント・関連コードと既存 PR を確認した。開始時点で Open PR はなかった。以下は提出時点の記録で、マージ済み・解決済みを意味しない。

## 提出した修正

| Issue | 確認した内容 | 提出した PR と残る確認 |
| --- | --- | --- |
| [#233](https://github.com/kawaii-music-moe/yuuka/issues/233) | `frontend/src/lib/date.ts` の `toISOString()` が UTC 日付を返す。支出の時刻入力はローカルなので早朝に日付と時刻が不整合になる。 | [#252](https://github.com/kawaii-music-moe/yuuka/pull/252): ローカル暦で日付を作成。JST・年越し・UTC・Los Angeles の 5 ケースを Node で確認し、回帰テストを追加。サーバーと端末の異なる TZ の扱いは #248 に残る。 |
| [#238](https://github.com/kawaii-music-moe/yuuka/issues/238) | `yuuka-auth/src/users.rs` が `count == 0` で admin を付与するため、管理者以外の初回ログインでも昇格する。 | Draft [#254](https://github.com/kawaii-music-moe/yuuka/pull/254): 明示した ID だけを新規 admin にする。既存ロールは維持。初回・第三者が先にログイン・明示 admin のテストと手順を更新。Rust 実行検証が必要。 |
| [#239](https://github.com/kawaii-music-moe/yuuka/issues/239) | `yuuka-outbound` は mapped IPv4 以外の IPv6 変換形式を許可する。RSS は事前検査と実接続で名前解決が分かれる。 | Draft [#255](https://github.com/kawaii-music-moe/yuuka/pull/255): compatible・NAT64・6to4 の範囲を拒否し、RSS に検査付きリゾルバを使用。各リダイレクトの URL 検査を維持。Rust 実行検証が必要。 |
| [#242](https://github.com/kawaii-music-moe/yuuka/issues/242) | `Config` と `.env.example` の既定は 3000、example.yaml は 7854 の明示設定。手順が後者を既定と呼んでいる。 | [#253](https://github.com/kawaii-music-moe/yuuka/pull/253): 未設定時と設定例を分け、proxy・アクセス例の説明も修正。コードの既定値は変えない。 |
| [#243](https://github.com/kawaii-music-moe/yuuka/issues/243) | 実装の OAuth スコープは calendar / drive.file を含み、Discord は MESSAGE_CONTENT を要求する。公式資料では Discord の Intent 審査条件が Issue 本文と異なる。 | [#258](https://github.com/kawaii-music-moe/yuuka/pull/258): OAuth・Gemini・Discord の運営条件を公式リンク付きで案内。アプリ認証と、2026-06-10 に変わった Intent 審査の基準を区別。 |
| [#244](https://github.com/kawaii-music-moe/yuuka/issues/244) | 導入コミットは `5c121ea`。履歴だけでは出典を確認できなかったため管理者に確認し、自作素材との回答を得た。 | 部分対応 [#259](https://github.com/kawaii-music-moe/yuuka/pull/259): 出典と用途を記録。画像自体の明示的な許諾と原著作物の権利は別に確認が残る。自作との回答から新しいライセンスを推定せず、Issue を自動 Close しない。 |
| [#245](https://github.com/kawaii-music-moe/yuuka/issues/245) | JSON エラーの重複を確認。ただし `yuuka-timeline/src/routes.rs::not_found()` は本文 `Not Found` のプレーンテキストで、Issue の「同じ JSON」には当てはまらない。 | Draft [#256](https://github.com/kawaii-music-moe/yuuka/pull/256): 13 モジュールの同じ JSON 応答を共通化。タイムラインのプレーンテキストと各メッセージを維持。500 にログを追加。Rust の既存ルート回帰テストが必要。 |
| [#247](https://github.com/kawaii-music-moe/yuuka/issues/247) | 検索のリゾルバは private。briefing・Discord 添付のクライアントは呼び出しごとに生成される。MCP は reqwest 0.13、他は 0.12 で単純な型共有はできない。 | Draft [#255](https://github.com/kawaii-music-moe/yuuka/pull/255): 0.12 の検査付きクライアントを共通化して保持。Issue 記載の改修予定に従い MCP は対象外。プロキシによる再解決を防ぐため直接接続する挙動を文書化。 |
| [#248](https://github.com/kawaii-music-moe/yuuka/issues/248) | 利用者 timezone は保存されるが、予定・Google 変換の JST 固定と、リマインド・cron の Local を切り替えない。既存 DB はオフセットなしの日時文字列を持つ。 | 短期対策の Draft [#260](https://github.com/kawaii-music-moe/yuuka/pull/260): 設定を編集不可にし未対応と明示。保存済みの値は保持し送信対象から除外。現状の JST 運用前提を文書化。TZ 統一・既存日時移行は未実装なので Issue は残す。 |
| [#250](https://github.com/kawaii-music-moe/yuuka/issues/250) | engines と setup は Node 20、README・CONTRIBUTING・CI は Node 24。node:sqlite 以外に Vite 8 の要件もある。 | [#253](https://github.com/kawaii-music-moe/yuuka/pull/253): engines と setup を Node 24 以上へ揃える。単に SQLite の初期対応版まで下げない。 |
| [#251](https://github.com/kawaii-music-moe/yuuka/issues/251) | 多くの取得処理に呼び出し番号の保護がない。BotAccess は成功時の Bot ID だけ確認し、古い失敗や loading 更新までは防がない。Timeline の日付比較だけでは同じ日付での Bot 切替を防げない。 | 部分対応の Draft [#257](https://github.com/kawaii-music-moe/yuuka/pull/257): 共通 asyncState を 13 画面へ適用し、古い成功・失敗・loading と破棄後の反映を防ぐ。送信・削除処理の共通化は未対応なので Issue は残す。Node で競合を確認し、CI の Svelte 型検査も成功。 |

## 未実装の Issue と次の作業

| Issue | 実装との照合・残す理由 | 次に必要な作業 |
| --- | --- | --- |
| [#234](https://github.com/kawaii-music-moe/yuuka/issues/234) | `OidcClient::identity()` が設定した claim の数値 Discord ID を必須にする。issuer を変更できるだけでは一般的な IdP でのログインは成立しない。`sub` を Discord ID の代用にするとデータ所有者・Discord 側の認可を壊す。 | Discord OAuth で本人の ID を確認する経路と、既存 OIDC / Discord ID の紐付け・移行方針を定めて実装する。同梱パスキー認証の古い設計をそのまま実装しない。 |
| [#235](https://github.com/kawaii-music-moe/yuuka/issues/235) | Login の仮表示、Usage、システム Bot の案内に固定名が残る。一方、desktop の実際の未設定時接続先は既に `http://127.0.0.1:7854` で、公開ホスト名はその箇所ではビルド例にある。Nix の Chromium 探索は未設定時かつ実在する場合だけの補助。 | 表示名は provider API から取得する共通処理へ寄せる。desktop の接続先指定・アプリ ID / keyring / 保存設定の互換性を確認し、汎用手順と作者の運用手順を分ける。単純なドメイン全置換は避ける。 |
| [#236](https://github.com/kawaii-music-moe/yuuka/issues/236) | Privacy はパスワード保存と誤記し、会話・音声・ブラウザ Cookie 等が不足。Terms は固定 IdP 名と全部免責の表現を含む。カスタム規約 URL の設定は既にある。 | 画面を運営者向けテンプレートと明示し、実データ・外部送信・削除できる範囲を棚卸しする。運営者名・窓口・保存期間・年齢等を差し込む手順と、URL 差替えを公開前の手順に追加する。バックアップやブラウザプロファイルが退会時に自動消去されると根拠なく保証しない。 |
| [#237](https://github.com/kawaii-music-moe/yuuka/issues/237) | workflows は CI のみでタグ公開・リリース成果物の生成がない。現行 deploy は develop を更新しローカルでビルドする運用。 | 配布対象 OS / CPU、ネイティブ依存、yuuka と画面の配置、初回 DB 作成、更新・ロールバックを揃えた release workflow とリリースノートを実装する。実際の新規環境でインストール・更新を確認する。 |
| [#240](https://github.com/kawaii-music-moe/yuuka/issues/240) | Bot / webhook / playbook の作成処理に共通の利用者別 quota はない。画面だけの上限は LLM ツールや同時リクエストから迂回できる。 | DB writer トランザクション内で count と作成を一体化し、API とツールの共通サービスへ適用する。リソースの所有者・Bot ごとの数え方、cron の最短間隔、停止中のプロファイル掃除を決め、競合・既存超過データを検証する。 |
| [#241](https://github.com/kawaii-music-moe/yuuka/issues/241) | develop の確定済みコードにはサーバー全体の定期バックアップがない。一方、本体作業ツリーには snapshot・backup・systemd timer に関する未コミットの別作業がある。 | 既存作業と重複する実装は追加しなかった。別作業がまとまった段階で、SQLite の整合性、鍵の保全、権限、世代管理、実際の復元を確認して独立した PR にする。今回それらの未コミット変更を取り込んでいない。 |
| [#246](https://github.com/kawaii-music-moe/yuuka/issues/246) | 8 クレートの補助関数の重複を確認。ただし finance と timeline の現行 `arg_i64` は既に数値文字列も受理し、Issue 本文の「finance は文字列を受け付けない」は現コードと一致しない。todo は整数・整数文字列を受理し、小数切捨てとは仕様が異なる。 | まず同一仕様の payload / scope / error を共通化する。小数・非有限・範囲外数値の変換や切詰め文字数は仕様別にテストし、引数受理範囲の変更を挙動不変リファクタと混同しない。Rust 回帰検証を伴う別 PR が必要。 |
| [#249](https://github.com/kawaii-music-moe/yuuka/issues/249) | seed / 一時 DB / 偽認証の重複を確認。既存 `yuuka-web::test_support::seed_bot` 自体が最小 DDL と migration 済み DB の両方を扱うため、全 fixture を機械置換するとテストの前提が変わる。 | マイグレーション統合テストと最小スキーマの単体テストを区別し、共有 DB / user / SessionStore を段階的に導入する。各テストの認可前提と件数を確認しながら workspace テストを実行する。今回は Rust テストを実行できないため一括変更していない。 |

## 検証上の制約と共通 CI 失敗

- 本体の develop は既存の未コミット作業を保持した。修正は origin/develop を基点に独立した作業ブランチで行い、PR の宛先はすべて develop。マージ・デプロイは行っていない。
- AGENTS.md はコンパイルを develop 限定とするため、作業ブランチでは cargo check / clippy / test やフロントのビルド・テストを実行していない。検証のためだけの develop への切替も行っていない。Rust の PR はコンパイル・実行検証前の Draft。
- ローカルでは非コンパイルの構文解析・Node による TypeScript モジュール直接実行・Biome・cargo fmt・cargo metadata・diff 確認を実施。追加した Vitest / Rust テストは「追加」と「実行済み」を区別して各 PR に記載した。
- 各 PR の GitHub CI では Svelte / PWA の型検査が成功。一方、変更前からある `.codex/hooks.json` の書式が Biome を失敗させる。書式だけ直す [#261](https://github.com/kawaii-music-moe/yuuka/pull/261) を別途提出し、JSON の意味が同一であることと、リポジトリ全体の Biome がエラー 0 になることを確認した。変更フックの実行・信頼登録は行っていない。
- Rust CI の cargo-deny は既存依存の RUSTSEC-2026-0459（libsqlite3-sys）、RUSTSEC-2026-0457 / RUSTSEC-2026-0462（rmcp）で失敗。今回の変更前と同じ依存バージョンが対象。無視設定を追加せず、依存更新と互換性検証が必要な項目として残す。
- 現行 CI の Rust コンパイル・テストは develop → main だけで、今回の PR の成功した fmt から Rust の型検査成功は判断できない。
