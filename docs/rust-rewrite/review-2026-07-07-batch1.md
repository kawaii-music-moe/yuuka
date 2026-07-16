# Batch 1 修正レビュー — 入力 DTO の wire 契約パリティ（H-1/M-7/M-8）

- 実施日: 2026-07-07
- 対象コミット: `9c8688d`（`fix(rust-rewrite): レビュー Batch 1 — 入力DTOのwire契約パリティ（H-1/M-7/M-8）`）
- 判定基準: [review-2026-07-06-fix-policy.md](review-2026-07-06-fix-policy.md) Batch 1 の完了条件
- **判定: 承認（完了条件を全て満たす）**。指摘は LOW 1 件と記録事項のみ。修正要求なし。

## 機械検査の実測結果

| 検査 | 結果 |
|---|---|
| `cargo test --workspace` | **97 passed / 0 failed**（前回 90 → wire 回帰テスト 7 件追加、リグレッションなし） |
| `cargo clippy --workspace --all-targets` | 実質クリーン。warning 2 件は ts-rs が `deserialize_with` を解釈できない既知の良性診断（コミットメッセージに記載済み。属性は無視され生成 TS は `string \| null` で正しい。exit 0） |
| `cargo run -p xtask -- gen-types --check` | **exit 0（ドリフトなし）**。生成 TS に `dueDate`/`startDate`/`contactInfo`/`recordedAt`/`todoId` が反映済みを実物確認 |
| `cargo fmt --check` | 差分あり（**Batch 1 以前からの持ち越し**。Batch 7 スコープ、本コミットの新規リグレッションではない） |

## 完了条件との突き合わせ

fix-policy Batch 1 完了条件「`dueDate`/`startDate`/`parentId`/`recordedAt`/`todoId`/`contactInfo`/`remindBeforeMinutes` が全入力経路で受理され、既存フロント接続で NULL 消去が起きないことをテストで保証」——**全て達成**。

| 項目 | 実装 | テストによる凍結 |
|---|---|---|
| H-1: `NewTodo` camelCase | `#[serde(rename_all = "camelCase")]` 付与（[dto.rs](../../crates/yuuka-todo/src/dto.rs)） | `newtodo_wire_contract_is_camelcase_in`（camelCase 受理＋snake_case 非受理の両方向）＋ ルート経由 E2E `route_add_accepts_camelcase_and_numeric_priority`（永続化まで確認） |
| H-1: `NewContact` camelCase | 同上 | `new_contact_wire_contract_camelcase` ＋ **NULL 消去シナリオそのものの回帰テスト** `route_save_update_preserves_contact_info`（create→update で `contact_info` 保持を確認） |
| H-1: `NewTimelineRecord` camelCase | 同上 | `new_timeline_record_wire_contract`（`recordedAt`/`todoId` 受理） |
| M-7（Batch 1 分）: 数値 priority | custom deserializer `deserialize_priority`（数値 0/1/2・文字列 3 値・不正値/`""`/`null` は `None`） | Node `normalizePriority`（todoRoutes.ts:23-31）と**規則単位で厳密一致を実測確認**（下記） |
| M-8（Batch 1 分）: 内部列除去 | `expense_id`/`expense_category`/`media_path`/`media_type` を DTO と INSERT 文の両方から除去（列省略でデフォルト NULL） | `new_timeline_record_wire_contract` が「`mediaPath: "/etc/passwd"` 等を送っても無視・400 にしない（Node と同挙動）」まで凍結 |
| `remindBeforeMinutes` | 既存の `NewSchedule` で対応済み（変更不要） | 既存テスト |

## レビューで独立に検証した事実

1. **`normalizePriority` の厳密一致**: Node 実装（`""`/`null`→null、`2|"high"`→high、`1|"medium"`→medium、`0|"low"`→low、他→undefined。呼び出し側で `?? undefined`）と Rust 実装を規則単位で照合し、実効挙動の一致を確認。
2. **reminder の snake_case 例外は正当**: Node `reminderRoutes.ts` は `ctx.body.trigger_at`/`repeat_rule`/`target_type`/`target_id` を **snake_case で読む**ことを実測確認。`NewReminder` を変更しなかった判断は正しく、さらに `new_reminder_wire_contract_is_snake_case` ロックテストで「誤って一律 camelCase 化した場合に落ちる」回帰ガードまで入っている。**fix-policy §2.1 の不変条件「入力=camelCase」にはこの例外を追記した**（本レビューで反映済み）。
3. **H-1 の取りこぼしなし**: 残りの入力 DTO（`NewExpense`/`SavePersona`/`NewPlaybook`/`DeleteCredential`/`NewSchedule`）を全数確認。複数語フィールドは `DeleteCredential.service_name`（camelCase 付与済み）と `NewSchedule`（付与済み）のみで、他は全て単語 1 語（`r#type` は rename_all 下でも `"type"` のまま）。Node `personaRoutes` の `isPublic` は公開切替エンドポイント（未実装・deferred）の入力であり `SavePersona` の欠落ではない。
4. **出力ビュー DTO は snake_case 据え置き**（fix-policy §2.1 の「view DTO に camelCase を足してはならない」を遵守）。`*DeletedData` の是正は方針どおり Batch 3 送り。

## 所見

- **[LOW] `crates/yuuka-todo/src/dto.rs` `deserialize_priority`** — JSON 浮動小数 `priority: 2.0` の受理幅が Node と異なる。JS は `2.0 === 2` が真のため Node は `"high"` に正規化するが、serde_json の float は `as_i64()` が `None` を返すため Rust は未設定に畳む。旧 UI・現行フロントとも整数/文字列しか送らないため実害はほぼゼロ。対応するなら `as_f64()` フォールバック 3 行（Batch 6 の M-7 残作業と同時で可）。**却下（記録のみ）でも妥当**。
- **[記録] ts-rs の良性 warning 2 件** — CI ゲートを「warning 文字列の grep」で組むと誤検知するため、Batch 7 の CI 化では **exit code 判定**にすること。
- **[記録] 生成 TS の `priority: string \| null`** — サーバは数値 0/1/2 も受理するが、TS 型には現れない。数値受理は「旧 UI 互換のサーバ側救済」であり新規クライアントに勧める形ではないため、**型が狭いのは意図どおり**（dto の doc コメントにも明記あり）。
- **[記録] M-8 の残余** — expense フローの `category` キーは Batch 6 まで無視され続ける（DTO doc・fix-policy に deferred 明記済み。方針との乖離なし）。

## 総評

Batch 1 は fix-policy に対する忠実度が高く、修正・テスト・生成物・ドキュメントの 4 点が同一コミットで整合している。特筆すべきは (1) バグの発火シナリオそのもの（update での NULL 消去）を E2E で凍結していること、(2) 一律 camelCase 化に流されず reminder の snake_case 例外を Node 実測で発見し、逆方向のロックテストまで敷いたこと。この 2 点は以後のバッチでも踏襲すべきパターン。次は Batch 2（CSP＋静的配信）へ進んで問題ない。
