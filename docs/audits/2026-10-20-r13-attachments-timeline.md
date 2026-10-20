# R13 会話添付のタイムライン保存（#110）

## 決定と調査根拠

ユーザー承認: Discord/PWA/WSの秘書経路、現在の発話者本人の写真・動画、複数選択と元の順序、任意URL禁止、配信時user/Bot認可、未参照実体削除、期限削除なし、実体バックアップ、保存失敗を成功にしない、送信元単位の冪等性と明示的再保存。V29を使用する。

調査元は #110 / 親 #97（両方コメントなし）、`2026-10-08-feature-parity.md`、HTTP/tool対応TSV、仕様書、Rust移行監査。CLAUDE.mdの非developでの実ビルド・テスト禁止に従う。適用されるAGENTS.mdはなし。元checkoutには触れていない。

旧Node基準は `bb97bae5cb21a36fbddf6a700a7c76122d409b37` の `src/functions/timelineFunctions.ts` / `src/db/timelineRepo.ts` / `src/server/routes/timelineRoutes.ts` / `src/bot.ts`。旧版はLLM引数の任意URLを無制限に取得し、ファイル保存後にDB登録。添付なしmediaも成功、再試行で重複、削除は行のみ、配信は認証のみだった。Rust基準は `662f46a`。会話ツールのmedia分岐がなく、Webアップロードのみ存在した。

## 経路・境界の監査

| 経路 | 実装・境界 |
| --- | --- |
| Discord秘書 | 現在の受信メッセージだけを列挙。CDNのHTTPS `/attachments/` URLのみ、redirectなし、30秒timeout、合計20MiBを読込中に検査。返信先添付は文脈のみで保存候補にしない。写真は既存parse_receiptと同じ秘書エンジンへ渡し、OCRルールも利用できる。音声併送は文脈として保持。 |
| PWA | 認証済み発話者が受理済みのmessage_log IDを `pwa:<id>` として使用。添付の元indexを保持し、音声/PDF等を候補から除外。既存202処理・会話履歴の本体とは独立にtimeline用コピーを保存。 |
| WebSocket | 接続時Bearer認証・Bot固定を維持。`image/audio`を後方互換で受理し、`attachments[]`と安定した`messageId`を追加。省略する旧クライアントは受信ごとのID、送るクライアントは再送で同じIDを使う。デスクトップ送信フレームもIDを保持。 |
| 汎用モード/サービス | 保存用catalogを注入しない。過去ログ・返信元・生成添付・任意URLを保存元にできない。 |
| 会話ツール | ToolContextの認証スコープと信頼済みcatalogのみ使用。添付indexが未存在、不正MIME/base64、空実体、件数/容量超過なら全体を拒否。URL引数も拒否。 |
| Web upload | 同じ検証とrepoの原子的保存を利用。sourceなしのアップロードは従来どおり別記録。内部パスをHTTP bodyから指定する契約は追加しない。 |
| 配信 | `media_path`から本人の記録を検索し、共通`has_bot_access`で現在の所有/active共有を検査。system_default・他人・共有解除・未存在は404。旧URL形式のまま、DB本体優先・旧ディスクfallback。no-store/nosniff。 |

## 永続化・失敗・重複・順序

V29で `timeline_media_assets` を追加。本体BLOB/MIME/filename、認証スコープ、送信元、添付ID、position、save_keyを保存。既存timeline行・列は変更しない。親記録へのCASCADEを持つ。新規保存ではディスク書込を行わず、複数添付の記録と実体を単一writer Txで一括保存する。検証・乱数・DB失敗は成功を返さず、部分記録/実体を残さない。

UNIQUE(user_id, bot_id, source_message_id, source_attachment_id, save_key)と単一writerで同時再試行も冪等。標準save_keyは空、明示的再保存は別の安定キー。再試行で既存の日時・題名・実体を上書きしない。削除はキーも除去するため、その後の保存は新規になる。新しいfilenameは128bit乱数、乱数失敗時は保存失敗。

選択indexを昇順に並べてINSERTし、positionも保存。各添付は既存UIで表示できる独立media記録になる。結果のrecordsも添付順、単一recordフィールドも互換として返す。既存のday/listはrecorded_at→id順を維持する。添付順保証は一括選択の保存単位であり、別々の保存要求は既存の記録日時順になる。

## バックアップ・削除・保持

- 新規実体は記録削除でCASCADEされる。ユーザー/Bot削除ではV29の専用triggerがR13の記録と実体を削除する。期限による削除はない。
- 旧ディスク実体は、record削除のwriter Tx内で全スコープを通じた残存参照数を調べ、最後の参照だけ削除。削除I/O失敗はエラーとしてTxを戻す。旧方式のディスクとDBの二資源をOS停止まで含めて原子化するものではない（ファイル除去後・DB commit前の停止は旧ファイル欠落になり得る）。新規DB本体にはこの窓がない。
- Drive用exportは本人のtimeline行と所有Bot行、対応assetsを同一DB read snapshotでコピー。従来漏れていた共有Bot上の本人timeline行も対象。所有Botの共有ユーザー行を含む既存バックアップ方針は維持する。
- 旧ファイルはエクスポート対象の記録だけを取り込み、エクスポートDB内assetsに格納する。本番の旧ファイルを移動/削除しない。path traversalや保存先外symlink、欠落/読込失敗はバックアップ失敗にする。旧ファイルは外部資源なので読込時の削除競合も失敗になる。
- Drive ZIPのdata/yuuka.dbに実体を含み、PIN付き移行パッケージは同じ完成済みexportからdata/mediaも作る。独立したライブDB再読込によるメタデータ/実体不一致を避ける。R13削除triggerもexportに保持する。外部Drive通信・既存リストア運用の実行検証は未実施。
- 既存のmedia_path/media_type、写真/video UI、day APIのレスポンスは維持。ファイルを直接data/mediaから読む独自運用では、新規実体がDB保存になった点に注意する。

## 独立性とmigration補足検証

このブランチはorigin/develop `662f46a` 起点で、他PRのcommitやV27/V28ファイルは含まない。V29だけで現行V26から適用できる。

補足検証に使った外部SQL:

- #188 V27: `041bcd5dbf8c557aeab01511afce17afb0dfa6f5`
- #190 V28: `24dff0935428b3e620edc76b18edb21a4462b529`

両PRのSQLだけを/tmpに取得し、V27は同PRの要求どおりTx外でFK=OFF、Tx完了後ONへ復元し、V28→このPRのV29を実行した。他PRのRust migration runnerや業務処理はこの検証の対象外。

## 検証

- `node scripts/verify-r13.mjs`: SQLite 3.53.3でV26→V29、既存metadata保持、スコープ付きUNIQUE、明示的再保存、本体保持、バッチrollback、record/Bot/user削除が成功。
- `node scripts/verify-r13.mjs /tmp/issue110-V27.sql /tmp/issue110-V28.sql`: V27→V28→V29も同じ検査に成功。Node SQLiteが拒否する既存V17の冗長FTS shadow table CREATE 4文のみ省略（仮想表が同じshadowを作成）。V29は原文を使用。
- 軽量Rust CIで同じNode/SQLite検査を継続実行する。`Biome 2.5.0 check frontend/src scripts`も成功（既存警告あり）。
- Rust回帰コード: 会話FCへのcatalog注入、PWA HTTP→実ツール→DB、WS複数/順序/安定ID/返信元除外、不正添付/任意URL拒否、冪等/再保存/Bot分離、保存rollback、配信認可/失効、実体削除、旧ファイル最後の参照、バックアップ本体/欠落失敗。
- `CARGO_BUILD_JOBS=1 cargo check --workspace --all-targets`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo fmt --all -- --check`、`git diff --check`: 成功（既存ts-rs serde属性警告あり）。
- デスクトップの変更対象model.rsをlibとして参照する一時manifestで`cargo check --all-targets`: 成功。GUI全体は対象外。
- 最終CIの結果はPR本文に記載。非developのためcargo build/test/run/nextestは未実行。追加Rust回帰テストの実行結果、実Discord/Gemini/Drive通信、ブラウザ操作、デスクトップアプリ全体の検証は未確認。
