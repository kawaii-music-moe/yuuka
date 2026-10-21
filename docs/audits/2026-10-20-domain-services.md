# D02 業務サービスの共通化と重複 API の整理（#126・#161 バックエンド）

## 範囲

[機能差分監査](2026-10-08-feature-parity.md) の D02（#126）と、#161 の受け入れ条件のうちバックエンド側（重複 API ごとの判断、Todo／Schedule／Finance／Context Note の業務処理を path ごとに持たないこと、残す API と非推奨 API の記録）を扱う。親は #97。作業ツリーに残っていた実装を develop に載せ替えた（#181）。

フロント側（`botApi.ts`／`botAttributeApi.ts` の一本化、`member-requests/{id}/decide` の形式統一、両 UI の役割と型契約）は [フロント契約の監査](2026-10-20-frontend-contracts.md) が扱う。

同じ ToDo・家計・ノートには 3 つの入口がある。

- 管理画面の HTTP API（`/api/tasks/*`・`/api/expenses/*`・`/api/context-note`・`/api/schedules/*`）
- PWA の HTTP API（`/api/client/*`）
- エージェントの会話ツール（`addTodo`・`addExpense`・`setContextNote` など）

以前は repo を共有していたが、入力検証・既定値・エラー分類は入口ごとに別実装だった。例として、繰り返し規則（cron 式）の検証は管理 API とツールにしかなく、ノートの文字数は PWA だけが UTF-16 で数えていた。PWA の共有ノートは `yuuka-orchestrator` に別の SQL として実装されていた。

## 共通サービスの責務

登録・更新・取消の業務処理を各ドメインの `service.rs` に集めた。予定は D03（#127）で先に共通化した `ScheduleService` に合わせた。

| サービス | 場所 | 操作 | サービスが持つ業務ルール |
|---|---|---|---|
| `TodoService` | `crates/yuuka-todo/src/service.rs` | 登録・更新・完了／未完了・進捗・削除 | タイトル必須。繰り返し規則は前後の空白を除き、空なら単発。実行時と同じ cron parser で検証する（R19）。サブタスクを持つ親の進捗は手動で更新しない。完了は `complete` に一本化（PWA のチェックボックスも同じ処理）。 |
| `FinanceService` | `crates/yuuka-finance/src/service.rs` | 記帳・修正・削除 | カテゴリ必須。金額は用途で 2 つに分ける：管理画面の `add_entry` は 0 以外（負の調整を含む・旧 Node からの管理 API の契約）、利用者の取引登録 `add_transaction`（PWA・ツール）は 1 円以上。`source` は `receipt_ocr` 以外を `manual` で保存する（D08）。修正の金額は 1 円以上、カテゴリは空にできない。 |
| `ContextNoteService` | `crates/yuuka-personal/src/service.rs` | 本文の読み書き、PWA のタイトル付き読み書き | 上限 10,000 は全入口で Node の `content.length` と同じ UTF-16 単位で数える。本文だけの保存（管理画面・ツール）はタイトルを変えない。PWA の保存はタイトルと本文を置き換え、空白だけのタイトルは `Shared note`。 |
| `ScheduleService` | `crates/yuuka-schedule/src/service.rs` | 登録・削除（Google 同期込み・D03） | タイトルと開始日時は必須（管理 API の検証をここへ移した）。リマインドの既定値と Google 同期は従来どおり。 |

失敗は `yuuka_web::DomainError` で返す。

- `Invalid(String)`：業務ルールに合わない入力。管理 API は `400 {success:false, message}`、PWA は `400 {message}`、ツールは `{success:false}` の結果に変換する。
- `Db(DbError)`：DB 失敗。HTTP は従来どおり 500（BUSY は 502）で、詳細はクライアントに返さない。ツールは従来どおり DB エラーの文面を実行エラーとして返す。

入口に残すのは、入力形式の解釈（JSON の欠落・型、パス引数、PWA の `datetime-local`、ツールの ISO 8601）と応答形式への変換だけ。ToDo の自動タグは INSERT／UPDATE トリガーで予約されるため（R05）、どの入口からの書き込みにも同じように付く。

秘書ターンの systemInstruction へのノート注入（`yuuka-orchestrator/src/context_note.rs` の `build_context_note_section`）は読み取り専用で、失敗時は空に縮退する。用途が違うので共通サービスを経由しない。PWA 用の `get_note`／`set_note` は orchestrator から削除し、`ContextNoteService` に移した。

## 入口ごとの差

業務ルールは共通にした。入口の用途で意図して違うものと、入力形式の違いだけを残す。

### ToDo

| 項目 | 管理 API `/api/tasks/*` | PWA `/api/client/todos` | ツール |
|---|---|---|---|
| ID の解釈 | JSON の `id`。0・欠落は 400（`update`・`progress`） | パスの文字列。数値でなければ 404 | `todo_id`。欠落は案内文 |
| タイトル | 共通（空は 400 `title is required`）。保存値はそのまま | 前後の空白を除いてから共通の検証 | 前後の空白を除く。欠落は案内文 |
| 繰り返し規則 | 共通の検証（不正は 400） | 指定できない | 同じ parser で先に確認し案内文を返す。初回期日の必須はツールだけの指示 |
| 完了 | `POST /api/tasks/complete`（該当なしは `200 {success:false}`） | `PATCH {completed}`（未完了へ戻せる・該当なしは 404） | `completeTodo` |
| 応答 | `Envelope`（`{success, task}`）・数値 ID・ツリー | 201・素の JSON・文字列 ID・`completed`・`list` | 日本語メッセージ付きの結果 |

タグ編集・優先度の整理・ルーチン停止はツールにしか入口がないため、ツールから repo を直接呼ぶ。

### 家計

| 項目 | 管理 API `POST /api/expenses/add` | PWA `POST /api/client/finance/transactions` | ツール `addExpense` |
|---|---|---|---|
| 金額 | 0 以外（負の調整可） | 数値・数値文字列を整数円に切り捨て。1 円以上 | 1 円以上（先に案内文） |
| カテゴリ | 共通（空は 400） | 前後の空白を除いてから共通の検証 | 前後の空白を除く |
| source | `manual` | `manual` | `manual`／`receipt_ocr`（D08） |
| 記帳後 | なし | なし | 支出なら予算消化率・消込候補を案内（R16・会話向けの補足としてツールに残す） |
| 応答 | `{success, expense}` | 201・文字列 ID・`kind` | 日本語メッセージ付きの結果 |

修正・削除（`updateExpense`／`deleteExpense`）はツールにしか入口がないが、記帳と同じサービスを通す。

### コンテキストノート

| 項目 | 管理 API `/api/context-note` | PWA `/api/client/shared-note` | ツール |
|---|---|---|---|
| 上限超過 | 400 `コンテキストノートは10,000文字以内です（現在: N文字）` | 400 `shared note exceeds 10000 chars`（従来の文言） | `setContextNote` は管理 API と同じ文言、`appendContextNote` は追記専用の案内文 |
| タイトル | 扱わない（保存しても変えない） | 読み書きする | 扱わない |
| 応答 | `{success, content, updated_at, max_length}` | `{id, title, body, updatedAt}`（UTC ISO） | `length`／`total_length` も UTF-16 で数える |

### 予定

必須項目の検証だけをサービスへ移した。日時形式の解釈は入口に残る。管理 API は受けた文字列を repo が JST として解釈して保存する（解釈できなければそのまま保存）。PWA は `datetime-local` だけを受け付け、終了が開始より前なら 400。ツールは解釈できない日時を案内文で返す。終了と開始の前後関係の検査は PWA だけにある（D03 からの既存の差。今回は揃えていない）。

## #161 の重複 API に対する判断

すべて「共存」とし、業務処理はサービスに統合した。廃止する path はない。PWA の API は文字列 ID・camelCase・`Envelope` なし・201 などの別のワイヤ契約で、インストール済みの PWA が使い続けるため、path を一本化すると互換性が壊れる。重複の害（検証・既定値の食い違い）はサービス層の統合で解消した。

| PWA API | 既存 API | 判断 | 共通化した処理・残した差 |
|---|---|---|---|
| `GET/POST /api/client/todos` | `GET /api/tasks`, `POST /api/tasks/add` | 共存（書き込みは統合） | 登録は `TodoService::add`。一覧は PWA が全件フラット、管理画面がツリーと絞り込み（読み取りの形が違うので repo の別メソッド） |
| `PATCH /api/client/todos/{id}` | `POST /api/tasks/update`, `POST /api/tasks/complete` | 共存（統合） | 完了は `TodoService::complete`、未完了へ戻すのは `update` |
| `GET/POST /api/client/calendar/events` | `GET /api/schedules`, `POST /api/schedules/add` | 共存（統合済み・D03） | 登録は `ScheduleService::add`（必須検証を今回移した）。一覧は PWA が暦日の範囲、管理画面が今後 N 日 |
| `GET /api/client/finance/summary` | `GET /api/expenses` の集計 | 共存 | 読み取りのみ。同じ `ExpenseRepo` の月次集計を使う |
| `GET/POST /api/client/finance/transactions` | `GET /api/expenses`, `POST /api/expenses/add` | 共存（書き込みは統合） | 記帳は `FinanceService`。管理画面の負の調整と PWA の 1 円以上は用途の差としてサービスの 2 メソッドで明示 |
| `POST /api/client/finance/receipt` | `POST /api/expenses/upload-receipt` | 共存（OCR は統合済み・D08 #132） | PWA は下書きを返すだけで登録しない。管理画面は秘書ターンへ渡す。副作用の違いは維持 |
| `GET/PUT /api/client/shared-note` | `GET/POST /api/context-note` | 共存（統合） | `ContextNoteService`。PWA だけタイトルを扱う |
| `GET/PUT /api/client/settings` | `/api/settings/gemini`, `/api/personas/*` | 共存（今回は対象外） | モデルの許可リスト（`ALLOWED_MODELS`）とペルソナの上限（`PERSONA_MAX_LENGTH`）は既に共通の定義を使う。PWA は意図的に小さい設定契約 |

`/api/client/chat/*` は対応する既存 HTTP API がないため対象外（#161 のとおり）。

## 残す API と非推奨 API

- 残す API：上の表のすべての path（PWA・管理画面とも）。
- 非推奨 API：なし。PWA の旧クライアントとの互換のため、現時点で非推奨にする path はない。将来 path を減らす場合は、PWA のワイヤ契約を管理画面の `Envelope` 契約へ寄せる移行計画（両方を受け付ける期間を置く）が先に要る。
- 新しい入口を足すときは、repo を直接呼ばず該当する `service.rs` を通す。

## 振る舞いの変化

業務処理を共通にしたことで、次の点が変わる。

- ノートの文字数を全入口で UTF-16 で数える。管理画面とツールは以前 `chars()` で数えていたため、絵文字などサロゲートペアの多い本文では、以前は通った保存が 400／失敗になる場合がある（Node と同じ判定）。BMP の文字（日本語を含む）だけなら変わらない。`getContextNote` の `length` も UTF-16 になる。
- PWA の取引登録で、`0.5` のように 1 未満の小数を送ると以前は 0 円として保存されていた。1 円以上の判定が切り捨て後に行われるようになり、400 になる。
- 管理 API の ToDo・家計・予定の登録とノートの保存で、入力検証が `botId` の解決（403／404）の後になる。不正な入力と不正な `botId` が同時に来た場合、以前の 400 ではなく 403／404 を返す。
- PWA の ToDo 完了は、以前は `update(status=done)`、現在は管理画面と同じ `complete` を通る。保存結果は同じ。
- ツール `updateTaskProgress` で、確認後に別経路で削除された競合時の文言が「進捗更新に失敗しました」から「見つかりません」に変わる。
- `updateExpense` の金額・カテゴリの検証がサービスにも入った（ツールの事前確認と同じ条件なので、ツールからの見え方は変わらない）。

## 対象外

- 支払い予定・予算上限（`/api/expenses/plans/*`・`/api/expenses/budget-limits*` と対応ツール）。#161 の重複表になく、まだ入口ごとに repo を呼ぶ。同じ形で `FinanceService` に寄せられる。
- 必須項目の前後空白の扱い。PWA とツールは除き、管理 API は除かずに保存する（従来どおり）。保存値を変えるため今回は揃えていない。
- 予定の終了・開始の前後関係の検査（PWA のみ）。
- `addExpense` の補足情報の読み出し失敗（#182）、レート制限の集約（#192）。同じファイルに触れるが直していない。

## 回帰テスト

- `crates/yuuka-client-api/tests/agent_switch.rs`：PWA のルータに管理画面のドメインルータを合わせ、3 入口を同じ DB で確かめる。
  - ToDo：ツール・管理 API・PWA の登録が同じ一覧に載る。PWA の完了が管理画面の `status=done` に現れ、未完了へ戻せる。空タイトルは両 HTTP 入口で同じ 400 と文言。不正な繰り返し規則は行を書かない。
  - 家計：ツールの記帳と管理画面の負の調整が PWA の月次集計に入る。PWA の負・0・小数、ツールの負の金額、管理画面の 0、空カテゴリはどれも保存しない。
  - ノート：PWA のタイトルがツールの保存後も残る。絵文字 5,001 個（UTF-16 で 10,002）を管理 API・PWA・ツールのどれもが拒否し、ノートを変えない。各入口の文言を保つ。
  - 予定：ツール・管理 API・PWA の登録が同じ範囲の一覧に載る。リマインドの既定値 10 分。空タイトルは両 HTTP 入口で同じ 400。
- `crates/yuuka-personal/src/service.rs`：UTF-16 の数え方、上限の文言、PWA のタイトル保存と既定タイトル、本文だけの保存でタイトルが残ること、拒否した保存がノートを変えないこと（orchestrator から移した PWA 共有ノートのテストを含む）。
- `crates/yuuka-todo/src/service.rs`：空タイトル、空の繰り返し規則は単発、前後空白の除去、実在しない発火日の式を拒否。
- 既存：`yuuka-todo` の繰り返し規則・自動タグの route テスト、`yuuka-finance` の `receipt_ocr` 保存、`yuuka-schedule` の Google 同期、`yuuka-personal` のノート上限の route テスト。

CLAUDE.md に従い、develop 以外のブランチでは Rust テストを実行していない（`cargo check --all-targets` で型検査のみ）。develop へマージ後に `cargo test --workspace` で確認する。
