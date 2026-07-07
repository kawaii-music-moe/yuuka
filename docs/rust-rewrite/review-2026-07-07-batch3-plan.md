# Batch 3 実装方針 — `/api/tasks` 形状（H-3）＋ 応答形状の Node 厳密一致（M-12）

- 起票日: 2026-07-07
- 対象: [review-2026-07-06-fix-policy.md](review-2026-07-06-fix-policy.md) Batch 3（**H-3＋M-12**）
- 前提: Batch 1（`9c8688d`）・Batch 2（`8c7d3e6`）承認済み。移行期の**共有フロント契約**に直結。
- 位置づけ: 実装着手前の方針。地上真実を Node 実装で実測確認済み。

---

## 0. スコープと完了条件

| 所見 | 内容 | 完了条件 |
|---|---|---|
| **H-3** | `GET /api/tasks` が Node と乖離（全件フラット・`created_at DESC`・フィルタ無視）。既存フロントは `TodoWithSubtasks`（親＋`subtasks` ネスト＋`effective_progress`）依存 | 親のみ＋`subtasks` ネスト＋`effective_progress` 算出＋`status`/`tag` フィルタ＋優先度→期日→作成日ソートを Node 一致に |
| **M-12** | mutation 応答形状の系統差（delete の `deletedId`、not-found の 404 vs 200、save 系のキー名） | 応答の HTTP ステータス・キー名・`success` セマンティクスを **Node 厳密一致**にし golden test で凍結。`*DeletedData` の `deletedId` 除去＋`generated/` 再生成 |

**推奨: Batch 3 を 2 コミットに分割**（fix-policy「1 コミット、必要なら 2」）:
- **3a = H-3**（todo ツリー化・todo クレート限定）
- **3b = M-12**（応答形状 parity・全 9 ドメイン＋Envelope＋generated 再生成）

---

## 1. 確定した地上真実（Node 実測）

### 1.1 H-3 — todo ツリーの構築（`src/db/todoRepo.ts`）

- **`ORDER_CLAUSE`**（`:126-132`・一覧共通）:
  ```
  ORDER BY
    CASE priority WHEN 'high' THEN 0 WHEN 'medium' THEN 1 WHEN 'low' THEN 2 ELSE 3 END,
    CASE WHEN due_date IS NULL THEN 1 ELSE 0 END,
    datetime(due_date) ASC,
    created_at DESC
  ```
- **`listTodos` フィルタ**（`:208-245`）: `status`（既定 `open`。`all` 以外は `WHERE status = ?`）、`tag`（`EXISTS (SELECT 1 FROM json_each(todos.tags) WHERE json_each.value = ?)`）、`parentId`（`null`→`parent_id IS NULL`）。
- **`listTodoTree`**（`:495-502`）: `listTodos({status, tag, parentId: null})` で**ルート親**を取得 → `attachSubtasks`。
- **`attachSubtasks`**（`:464-489`）: `WITH RECURSIVE tree(id)` で親群の**全子孫を 1 クエリ収集**（`ORDER_CLAUSE`）→ `buildTodoTree` でネスト化 → 指定ルートのみ返す。
- **`buildTodoTree`**（`:437-458`）: `id→node` マップを作り `parent_id` で子を親の `subtasks` に push（親不在の子はルートへ昇格）。`effective_progress` をボトムアップ算出。**行順（＝クエリの ORDER_CLAUSE 順）で `subtasks` に積む**。
- **`computeEffectiveProgress`**（`:407-431`）:
  - 子なし → `status === "done" ? 100 : (progress ?? 0)`。
  - 子あり → **葉**（子を持たないノード）の完了率 `Math.round(done / total * 100)`（`total===0` なら 0）。
- **GET `/api/tasks` ハンドラ**（`todoRoutes.ts:60-98`）: `status` クエリを `pending→open` / `done→done` / **その他（既定）→ all** に写像、`tag` クエリ。応答 `200 { success: true, tasks }`（`tasks: TodoWithSubtasks[]`）。

**clean view 原則（Batch 1 継承）**: Node は `SELECT *` で `user_id`/`bot_id`/`linked_payment_id`/`due_reminded` を**生 row で漏らす**。Rust の view DTO はこれらを**フィールドに持たない**（構造的フェイルクローズ＝Node より安全側の**意図的差分**、fix-policy §良い点）。H-3 の `TodoWithSubtasks` も同原則で内部列を除外し、`subtasks` と `effective_progress` を足す。

### 1.2 M-12 — 応答形状の系統差（`sendJson` 実測）

| 種別 | Node の形状 | Rust 現状 | 差分 |
|---|---|---|---|
| **delete（全 8 ドメイン）** | `200 { success: <bool> }`（削除可否のみ・**`deletedId` 無し**） | `{ success, deleted_* }` ＋ **未存在は 404** | `deletedId` 除去・常に 200・`success` は削除可否 |
| **todo complete** | `200 { success: !!todo, task? }`（未存在は `success:false`・`task` キー無し） | 未存在 **404** | 常に 200・`task` 任意 |
| **reminder cancel** | `200 { success:true, reminder }` / 未存在 **404** / status 不正 **409** | 未存在 404（409 無し） | 409 追加・`reminder` 同梱 |
| **save（persona）** | `200 { success:true, personas[], active_persona_id, max_length }`（**単一でなくリスト**） | `{ success, persona }` | 形状不一致（`active_persona_id` は M-11 deferred） |
| **save（playbook）** | `200 result`（`result = savePlaybook(...)` の `{ success, message, … }`） | `{ success, playbook }` | golden で確定 |
| **validation** | `400 { success:false, message }` | `400 { success:false, message }`（`ApiError`） | 概ね一致 |

**`*DeletedData` は 8 struct**（fix-policy §2.2）: `DeletedData`(todo)/`ScheduleDeletedData`/`CredentialDeletedData`(`deletedServiceName`)/`PlaybookDeletedData`(`deletedName`)/`ContactDeletedData`/`PersonaDeletedData`/`TimelineDeletedData`/`ExpenseDeletedData`。**Node は `deletedId`/`deletedServiceName`/`deletedName` を一切返さない**（生成物の `*DeletedData.ts` は Rust 由来の幽霊型でフロント非依存）。

**注意（実測で判明した個別事項）**:
- **finance の delete ルートは Rust 未配線**（`/api/expenses` + `/api/expenses/add` のみ）。`ExpenseDeletedData` は**未使用の幽霊型**。→ M-12 で DTO ごと除去（delete は finance 完成パスで Node 形状に配線）。
- **persona save の形状は M-11（`active_persona_id`・適用中表示）と絡む**。Batch 3 では「単一 persona → リスト形状」への是正のうち **deferred でない部分**（`personas[]`・`max_length`）を扱い、`active_persona_id` は **Batch 6（M-11）へ明示 deferred**。
- Node は **not-found を一律 200 にはしない**（persona save は `404`・reminder cancel は `404`/`409`）。**エンドポイント単位で golden 確定**する（「常に 200」の一般化は誤り）。

---

## 2. 実装設計

### 2.1 H-3（3a）— todo ツリー化

**DTO（`crates/yuuka-todo/src/dto.rs`）**: 再帰 view を追加。
```
// clean view（内部列を持たない）＋ subtasks ＋ effective_progress。snake_case（フロント受信型一致）。
pub struct TodoWithSubtasks {
    // Todo と同じ公開フィールド（id/title/description/due_date/start_date/priority/tags/
    //  status/progress/parent_id/repeat_*/created_at/updated_at）
    pub subtasks: Vec<TodoWithSubtasks>,   // 再帰（ts-rs は Array<TodoWithSubtasks> 生成）
    pub effective_progress: i64,
}
```
`TaskListData.tasks` を `Vec<Todo>` → `Vec<TodoWithSubtasks>` に変更。add/complete は従来どおり flat `Todo`（Node の add/complete も単一 record）。

**Repo（`repo.rs`）**: `list` を `list_tree(scope, status, tag)` に置換。
1. ルート親取得: `SELECT {cols} FROM todos WHERE user_id=?1 AND bot_id=?2 AND parent_id IS NULL [AND status=?] [AND EXISTS(json_each … tag)] {ORDER_CLAUSE}`。
2. 子孫収集: 親 id 群を `WITH RECURSIVE tree(id) AS (SELECT id … WHERE id IN (…) UNION ALL SELECT t.id FROM todos t JOIN tree ON t.parent_id=tree.id) SELECT {cols} FROM todos JOIN tree ON todos.id=tree.id {ORDER_CLAUSE}`。
3. Rust でツリー構築（`HashMap<i64, node>`・行順で `subtasks` に push・親不在はルート昇格）＋ `effective_progress` をボトムアップ算出（葉の `done/total`・`round(done/total*100)`・子なしは `status=="done"?100:progress`）。
4. ルートを親の取得順で返す。

**Route（`routes.rs`）**: `list` に `status`/`tag` クエリを追加。`status`: `pending→open` / `done→done` / 既定 `all`。`resolve_scope` は従来どおり。

**据え置き（H-3 対象外・deferred 明示）**: `/api/tasks/update`・`/api/tasks/progress`・gantt/someday・progress log・15 tool・recurrence 実行（いずれも Rust 未配線）。本バッチは `GET /api/tasks` の形状のみ。

### 2.2 M-12（3b）— 応答形状 parity

**(a) delete parity（系統・8 ドメイン）**
- **bare success エンベロープ**を用意。`yuuka-types` に空ペイロード `EmptyData {}`（`Serialize`/`TS`・`#[serde(flatten)]` で無へ畳む）を追加し、delete ハンドラは `Ok(Json(Envelope { success: ok, message: None, data: EmptyData {} }))` = `200 { success: <bool> }`。
- **`*DeletedData` 8 struct を除去**（DTO・`export_bindings`・routes の参照）。未存在でも 404 でなく 200。credential/playbook の `deletedServiceName`/`deletedName` も同様に消える。
- `ExpenseDeletedData`（未配線）は DTO ごと削除。

**(b) todo complete parity**
- 応答を `{ success: <found>, task? }` に。`task: Option<Todo>`（`skip_serializing_if=Option::is_none`）、未存在は `200 { success:false }`。

**(c) 個別 golden（エンドポイント単位で Node 実応答に一致）**
- reminder cancel: `200 { success, reminder }` / 未存在 `404` / status 不正 `409`（現状 404 のみ → 409 追加＋`reminder` 同梱）。
- persona save: `{ success, personas[], max_length }` へ（`active_persona_id` は **Batch 6/M-11 deferred**・当面キー欠落を明示）。
- playbook save: Node `result` 形状（`{ success, message, … }`）に golden 一致。
- schedule/finance/personal/timeline の add/save/list: 現状 `{success, <entity>}` で概ね一致するが **golden で全 mutation を確定**。

**(d) generated 再生成**: `xtask gen-types` で `*DeletedData.ts` 8 件が消え、`TodoWithSubtasks` の `subtasks`/`effective_progress` が反映されることを完了条件に含める（`gen-types --check` exit 0）。

---

## 3. 検証（golden test 戦略）

- **golden fixture**: Node の実応答（ステータス・キー・`success`）を各 mutation エンドポイントの期待値として `mod tests` に直書きし、Rust 応答と突き合わせて凍結。Node 実装（`sendJson` の第 2・3 引数）を一次ソースとする。
- **H-3 テスト**: 親＋2 階層のサブタスクを seed し、(1) ネスト構造、(2) `effective_progress`（葉の完了率・子なしは progress/done）、(3) `status=pending/done/all` フィルタ、(4) 優先度→期日→作成日ソート、(5) 内部列（`user_id`/`bot_id`/`linked_payment_id`）非露出、を凍結。
- **M-12 テスト**: delete が `200 {success:bool}`（`deletedId` 非存在）、complete 未存在が `200 {success:false}`、reminder cancel の 404/409、を凍結。
- **機械検査ゲート**: `cargo test --workspace` 全通過（103→追加分・回帰無）／`clippy --all-targets -D warnings` exit 0／`gen-types --check` exit 0／`cargo deny`。fmt は Batch 7。
- **敵対的再レビュー**: fix-policy §4-5 に従い、**Batch 3 完了後に Batch 1〜3 の修正差分を敵対的に 1 周**する。

---

## 4. 影響ファイル（見込み）

- **3a**: `crates/yuuka-todo/src/{dto.rs,repo.rs,routes.rs,lib.rs}`（`TodoWithSubtasks`・`list_tree`・status/tag・テスト・export_bindings）。
- **3b**: `crates/yuuka-types/src/envelope.rs`（`EmptyData`）＋ 全 9 ドメインの `dto.rs`（`*DeletedData` 除去）・`routes.rs`（delete/complete/cancel/save 形状）・`lib.rs`（export_bindings 更新・golden テスト）。`frontend/src/lib/api/generated/`（再生成で `*DeletedData.ts` 削除）。

---

## 5. 却下／据え置きの明示

- **内部列の clean view 据え置き**（H-3）: Rust は `user_id`/`bot_id`/`linked_payment_id`/`due_reminded` を返さない。Node の生 row 漏洩に**合わせない**（安全側の意図的差分・fix-policy §良い点）。
- **persona `active_persona_id`** は Batch 6（M-11）へ deferred。Batch 3 では persona save をリスト形状に寄せるが当該キーは付けない（golden にも「当面欠落」を明記）。
- **finance delete ルート**は本バッチで新設しない（finance 完成パス）。`ExpenseDeletedData` 幽霊型のみ除去。
- **not-found の一律 200 化は却下**（Node はエンドポイント単位で 200/404/409 を使い分ける）。golden で個別確定する。

---

## 6. 参照

- fix-policy: [review-2026-07-06-fix-policy.md](review-2026-07-06-fix-policy.md)（Batch 3・§2.2 `*DeletedData`）
- Batch 2: [review-2026-07-07-batch2-plan.md](review-2026-07-07-batch2-plan.md)
- Node 実装: [src/db/todoRepo.ts](../../src/db/todoRepo.ts)（tree/progress/order）、[src/server/routes/todoRoutes.ts](../../src/server/routes/todoRoutes.ts)（GET /api/tasks）、各 `*Routes.ts` の `sendJson`
- 設計: [PLAN.md](PLAN.md) §6（Web/応答）

---

## 7. 実装結果（3b・2026-07-08）— 方針 §1.2/§2.2 からの差分と根拠

実装着手時に Node 実体とフロント消費を再照合し、**§1.2/§2.2 の記述に事実誤りと過剰簡略化**が見つかった。
共有フロント契約に直結するためユーザー確認（AskUserQuestion）を取り、**「ステータス意味論 parity」**方針で確定・実装した。

**フロント消費の地上真実（実コード確認）**: mutation 呼び出しは全て `api.post<ApiResponse>`（`{success, message?}` のみ型付け）。
`BotPersonas.svelte` 等は保存後に**リストを再 fetch**し、返り entity は読まず、`message` も `res.message ?? "既定文言"` と
**フォールバック付き**。⇒ フロントは実質 **HTTP ステータス（success）しか見ておらず**、body 形状・message 文字列・entity 有無に頑健。

- **§1.2 の persona save「`personas[]` リスト形状」は誤り**。Node `personaRoutes.ts` の save は実際には
  更新→`{success, message}`／作成→`{success, persona, message}` の**単一**。リスト化は Node と乖離するため**実装せず**、
  現状の `{success, persona}` を維持（フロントは再 fetch のため無影響）。§2.2(c)・§5 の persona-list 記述は撤回。
- **§2.2(a)「一律 EmptyData bare」は不正確**。Node の delete は todo/schedule/timeline/credential が bare `{success}`、
  contacts/persona/playbook は `message` 付き。方針確定に従い **全 delete を bare `{success:<bool>}`**（message 文字列は
  移植しない）に統一。フロントは message フォールバックがあり無影響。**本当に効く差分＝not-found を 404→200** に是正。
- **save 系（contacts/persona/playbook/schedule/finance/timeline）は変更せず** `{success, entity}` のまま。§1.2 が
  「更新は message のみ／作成は entity」の非対称を挙げるが、フロントは entity 非依存のため status parity で十分。
  playbook save の Node `result` 形状への寄せも見送り（同上・完成パスで再確認）。
- **実装した M-12 の中身**: (a) 7 delete ルート（todo/schedule/timeline/personal/credential/playbook/persona）を
  `Envelope::bare(ok)` 化＝`200 {success}`・該当無も 200・`deletedId` 非返却。(b) `*DeletedData` 8 struct＋export＋
  generated 8 `.ts` を除去、`EmptyData`/`EmptyData.ts` を新設。(c) todo complete を Node `{success:!!todo, task}` に
  （該当無 200 `{success:false}`・`task` キー無し）。(d) reminder cancel の **404（不在）/409（pending でない）** を実装
  （`yuuka-core::WebError::Conflict`=409 を追加。`WebError` は `#[non_exhaustive]` でないため `status()`/`client_message()`
  網羅 match を更新、`ApiError` は `.status()` 経由で 409 を写像）。reminder `delete`（Node 非存在ルート）は Batch 7 撤去予定のため据え置き。
- **golden test**: todo delete-bare＋complete-missing、schedule delete-bare、reminder cancel 404/409、credential delete-missing→200 を追加/更新。
- **ゲート**: `cargo test` 112 緑／`clippy -D warnings` 0／`gen-types --check` exit 0（drift 0）／`cargo deny` ok。
