# D08 レシート OCR 共通化（#132）

## 決定した仕様と範囲

Issue #132、親 #97（調査時点で双方コメントなし）、[機能差分監査](2026-10-08-feature-parity.md)、[HTTP 対応表](2026-10-08-http-route-coverage.tsv)、[会話ツール対応表](2026-10-08-tool-coverage.tsv)、[仕様 §3.4.2](../spec/discordbot_spec.md) と現行 Rust／旧 Node `bb97bae5` を照合した。

仕様書は「プレビュー→承認→保存」とするが、旧 Node `src/services/receiptParser.ts` と現行の秘書ターンはその承認を機械的には強制しない。ユーザー承認により #132 では保存前確認を新設せず、既存の保存タイミングを維持する。共通内部結果は商品明細＋税込合計、PWA は従来の単一ドラフトへ変換する。旧 Node に存在した `source: receipt_ocr` の保存を既存 `addExpense`／repository に復旧する。専用記帳ツールは追加しない。

## 入口と副作用

| 入口 | 読み取りと保存 |
|---|---|
| Discord 秘書の画像（返信先画像を含む） | `TurnProcessor::parse_receipt` → `ChatEngine::receipt_turn` → 共通 OCR → 従来の秘書 FC ループ。保存は `addExpense` 等の既存ツールをモデルが呼んだ時。明らかにレシート以外の画像と、OCR の入力検証（エンコード後10 MiB超・許可外形式）に通らない画像は、従来の画像会話へ戻す。 |
| 管理画面 `POST /api/expenses/upload-receipt` | `ReceiptParserAdapter` → 同じ `receipt_turn`。既存の `{success, response: string}` 契約を維持。画面は従来から自動分類・登録と表示する。 |
| PWA `POST /api/client/finance/receipt` | 共通 OCR の summary を従来の `ReceiptDraft` として返す。明細の合算で税込合計を上書きしない。取引・会話ログ・記憶は保存しない。 |
| PWA 取引フォーム `POST /api/client/finance/transactions` | ユーザーが修正・登録する従来の手入力経路を維持（source は manual）。ドラフトを読み取っただけでは登録しない。 |
| WS `/ws/chat`／PWA チャット／Discord 汎用 Bot | 一般の画像・複数添付を扱う通常会話。レシート専用 OCR に一律置換しない。会話中のレシート依頼は従来の秘書プロンプトと既存 finance ツールで処理する。 |
| 管理 API の手入力・タイムライン等 | `ExpenseRepo::add` は引き続き manual。既存の記帳ツール `addExpense` は `add_with_source` を利用し、未知の source は manual に正規化する。 |

OCR は `readReceipt` の引数を受け取るだけで、ツール registry／dispatch／finance repo を呼ばない。専用のプロンプト・関数宣言・正規化は `yuuka-orchestrator/src/receipt_extract.rs` に集約する。記帳側の FC 入力へ明細と合計を渡し、合計と明細の二重記帳を避ける指示を加える。画像を再度 FC ループへ渡さない。OCR データは未信頼として扱い、画像内の命令に従わないよう指示する。

記帳経路には従来どおりツールの能力制限、予算消化率・消込候補、会話ログ、記憶抽出などの副作用がある。OCR 明細を新たなユーザー発言や記憶抽出入力として保存せず、従来のキャプション／画像プレースホルダを維持する。既存の `addExpense`、`findSettlementCandidates`、`settlePlannedPayment` を再利用し、OCR 内で消込・通知・ToDo 操作を追加しない。

## 入力・モデル・境界

- レシート専用 API と Discord の共通 OCR は `yuuka_core::receipt_image::validate` を利用する。MIME の大文字・パラメータを正規化し、従来の7形式（png/jpeg/jpg/webp/heic/heif/gif）、非空で正しい base64、エンコード後10 MiB以下を検査する。不正 base64 は従来の上流エラーからローカル拒否へ改善する。HTTP 全体の既存10 MiBボディ上限も継続する。画像形式の実体判定は Gemini に任せる。
- Discord の URL 取得・先頭画像選択、PWA の画像縮小、汎用チャットの複数添付検証はそのまま。Discord の取得は従来同様に全量読み込みであり、この上限はダウンロード前のメモリ制限にはならない。外向き通信全体の整理（D06）は対象外。
- OCR と記帳の双方は発話ユーザー自身の暗号化 Gemini キーを復号し、同じ保存済みモデルを利用する。Bot 所有者やシステムのキーへフォールバックしない。OCR が1回追加されるため、記帳経路の上流呼び出し数は増える。共有レート制限は従来どおり入口で1ターン分消費し、内部では二重消費しない。
- 管理 API の `resolve_scope`、PWA の `ClientScope`、Discord の登録・利用権確認を維持。OCR 自体は個人 DB データを検索しない。保存時は従来の `ToolContext` → `UserScope(user_id, bot_id)` を使い、OCR 由来の user_id／bot_id は受け付けない。
- 不正日付は None、未知カテゴリは「その他」、income 以外は expense、空白は trim。金額は従来の円文字列・カンマ・丸めを維持し、非有限値・範囲外・0以下は None。明細と集約に同じ正規化を適用する。明細が読めない場合も summary の合計は独立に維持する。

## 失敗・再送・重複

- 不正添付は LLM 呼び出し前に拒否する。ただし Discord では、入力検証に通らない写真（高解像度で10 MiB超、`image/avif` 等）も develop と同じく制限なしで画像会話へ渡し、ターンを失敗させない。管理画面・PWA は入口で同じ検証をして400を返すため、`receipt_turn` の検証エラーは Discord 経路でしか起きない。複数の OCR 関数呼び出し、壊れた引数、内容が空のレシートは判読失敗とし、記帳ターンを開始しない。明らかな非レシート画像だけは通常の画像会話へ戻す。PWA ではどちらも422。
- PWA のキー未設定400、判読失敗422、上流失敗502、構成不備503を維持。管理 API は従来の429／503とテキスト応答契約を維持し、未設定キー等は既存の案内文を返す。
- Gemini HTTP の一時障害リトライは既存クライアントを使用する。OCR に保存副作用はない。OCR＋記帳ターン全体や finance ツールを新たなリトライで包まない。
- 画像ハッシュによる重複排除、リクエスト ID、記帳の冪等性は旧 Node／現行ともレシート入口にない。同じ画像の再送は再記帳になり得る。複数ツール実行や保存後の応答失敗を跨ぐロールバックもない。今回の共通化でそれらを保証したとは扱わない。
- expenses.source は既存カラムを使う。migration は不要（V27／V28／V29の予約には触れない）。

## 検証

回帰テストを追加：共通添付検証、明細・税込合計の独立性、PWA ドラフト変換、異常金額、空／非レシート／複数抽出結果、不正添付で上流を呼ばないこと、下書きの副作用なし、秘書への抽出結果受け渡しと非永続化、通常写真の会話維持、Discord で OCR の入力検証に通らない写真（許可外形式・10 MiB超）の画像会話フォールバック、既存記帳ツールでの source 保存と別ユーザー／別Bot隔離。

非 develop では CLAUDE.md に従って Rust テストを実行せず、`cargo check --tests` で型検査する。実行済みチェックと未実行項目は PR 本文に記録する。実キーによる Gemini 通信、Discord／ブラウザの実機操作は未検証。通常 PR の CI は frontend／PWA の型検査、lint、Rust の fmt・cargo-deny を実行する。重い Rust clippy／build／test は develop→main の PR だけで実行するため、この PR の CI 成功は Rust テスト実行の代替ではない。
