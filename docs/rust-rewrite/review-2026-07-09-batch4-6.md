# Batch 4/5/6 修正レビュー — 認証縮退・自己復帰・連鎖削除・移行期ガード（M-1/M-2/M-4/M-5/M-6＋migration hazard）

- 実施日: 2026-07-09
- 対象: 未コミットの working tree 差分（12 ファイル・+420/-28。コミット時に本行へハッシュを追記する）
- 判定基準: [review-2026-07-06-fix-policy.md](review-2026-07-06-fix-policy.md) Batch 4・Batch 5・Batch 6（M-6 分）の完了条件
- **判定: 承認（対象バッチの完了条件を全て満たす）**。指摘は運用注意 1 件（refinery checksum）と記録 1 件のみ。修正要求なし。

## 機械検査の実測結果

| 検査 | 結果 |
|---|---|
| `cargo test -p yuuka-core -p yuuka-db -p yuuka-todo -p yuuka-web` | **60 passed / 0 failed**（M-1×2・M-2×2・M-4×1・M-5×1・M-6×1・migration×1・repeat ゲート×1 の新規テストを含む。リグレッションなし） |
| `cargo clippy -p ... --all-targets -- -D warnings` | クリーン（exit 0。ts-rs の既知良性 warning のみ、Batch 1 レビューで記録済みのもの） |

## 完了条件との突き合わせ

### Batch 4 — Web 層 parity（M-1/M-2）

| 項目 | 実装 | テストによる凍結 |
|---|---|---|
| M-1: 認証縮退 | `yuuka-web/src/auth.rs` `resolve_user` — session/desktop 各ストア呼び出しの `Err` を `?` 伝播せず **catch→次経路継続**（Node `httpHelpers.ts` の `getSessionUser`/`getBearerUser` try/catch 踏襲）。fix-policy 必須要件の **縮退時 `tracing::warn!`** も両経路に実装（`yuuka-web/Cargo.toml` に tracing 追加） | `m1_redis_down_falls_back_to_bearer`（Redis 断でも有効 Bearer は 200）・`m1_redis_down_without_bearer_is_401_not_502`（Cookie のみは 502 でなく 401 縮退） |
| M-2: 413 区別 | `WebError::PayloadTooLarge`（413）を新設し、`extract.rs` で Bytes 抽出 rejection の status が 413 のときに写像（Node `server.ts` の `413 Payload Too Large` parity） | `payload_too_large_maps_to_413_m2` |
| M-2: 空ボディ | `Content-Length: 0` を 400 拒否せず `{}` として続行（全フィールド任意 DTO・削除系 POST の Node parity）。非空は従来どおり厳密 JSON パース | `empty_body_is_treated_as_empty_object_m2`（`{}` 化＋botId の query フォールバック維持まで確認） |

### Batch 5 — 自己復帰の根幹（M-4/M-5）

| 項目 | 実装 | テストによる凍結 |
|---|---|---|
| M-4: Auth 一律 Permanent 是正 | `AuthError::fatality()` を新設し `AppError::fatality()` から variant 別に委譲。`Backend`（ストア到達不能）のみ `Transient`、`SessionInvalid`/`TokenMalformed`/`Forbidden` は `Permanent`。core 内網羅 match のためバリアント追加漏れはコンパイルエラー化される | `auth_backend_is_transient_others_permanent_m4`（AuthError 直・AppError 経由の両方） |
| M-5: writer panic 隔離 | `writer.rs` のジョブ実行を `catch_unwind(AssertUnwindSafe)` で **タスク境界隔離**。panic した job の oneshot sender は unwind 中に drop され当該呼び出しのみ `WriterGone`、進行中 Tx は `Transaction` の Drop でロールバック、writer スレッドと後続ジョブは継続 | `writer_survives_job_panic_m5`（panic 呼び出しが `WriterGone` を受け取り、**後続の書き込みが成功して読み戻せる**ところまで確認） |

Batch 5 完了条件の「supervisor が再 spawn して復帰」については、**catch_unwind 隔離により writer 自体が死ななくなった**ため再 spawn を要するシナリオが消滅（この writer は生 `std::thread` で JoinSet 監督外、という前提ごと解消）。完了条件の趣旨（panic 1 発で全書き込み恒久停止、を防ぐ）はより強い形で達成している。

### Batch 6（M-6 分）— 連鎖削除 parity

| 項目 | 実装 | テストによる凍結 |
|---|---|---|
| M-6: 連鎖削除 | `todo/repo.rs` `delete` を `WITH RECURSIVE descendants` の一括 DELETE に変更（Node `deleteTodo` `todoRepo.ts:364-381` parity）。再帰段にも scope 検査を付け、クロススコープ `parent_id` 連鎖でも他人の行を消さない（Node より厳格側） | `delete_cascades_to_descendants_m6`（孫まで消滅・孤児のルート昇格なし・無関係タスク無傷） |
| 付随: repeat の parent ゲート | `add` で **サブタスク（parent_id あり）の repeat_\* を NULL 化**（Node `todoRepo.ts:176-178` `parentId == null ? repeatRule : null` parity）。無いと recurrence サービスが子タスクを複製する | `subtask_does_not_inherit_repeat_fields` |

### 移行期 migration hazard（fix-policy の Batch 外・データ全喪失ガード）

| 項目 | 実装 | テストによる凍結 |
|---|---|---|
| #1: `schema_version` 刻印 | `V17__baseline.sql` 末尾で `system_settings.schema_version='17'` を upsert（Node `migrations.ts:1574-1578` parity）。無いと **Rust が新規作成した DB を Node が開いたとき legacy-v1 誤検出でコアテーブル全 DROP**（[verification/rpt-migrations-sqlx-refinery.md](verification/rpt-migrations-sqlx-refinery.md) の破壊パス） | `baseline_reruns_on_fully_populated_db_and_stamps_schema_version` |
| #2: baseline 冪等化 | `CREATE VIRTUAL TABLE`（fts5）と `CREATE TRIGGER` ×3 に `IF NOT EXISTS` を付与。既存 Node DB へ refinery が baseline を流すとき「object already exists」で起動不能になるのを防ぐ | 同上（refinery 履歴を消して全オブジェクト既存状態で baseline 再走） |
| 付随: 一過性ロックの誤 Fatal 防止 | `schema.rs` `run_migrations` — refinery エラーのうち SQLITE_BUSY/LOCKED（"database is locked"/"is busy"）を `DbError::Busy`（Transient）へ分類し、恒久 Migration 失敗（Fatal＝即終了）への誤判定を防ぐ | —（文字列判定のため単体テスト対象外。下記記録参照） |

## レビューで独立に検証した事実

1. **M-6 の再帰 CTE にサイクル無限ループの懸念なし**: `parent_id` は `add` の insert 時のみ設定され（スコープ内実在検証つき）、**更新経路が存在しない**。新規行は既存の子孫を親に指せないため、サイクルは構造的に不可能で `UNION ALL` で安全。`list_tree` の既存 CTE と同型。
2. **#1 の前提充足**: `system_settings` は baseline 先頭（1〜6 行目）で `updated_at` 列つきで作成済みのため、末尾の upsert は新規 DB でも必ず成立する。`datetime('now','localtime')` も既存列デフォルトと一致。
3. **M-2 の 413 判定方法**: axum の Bytes 抽出 rejection を `into_response().status()` で判別する実装は、`DefaultBodyLimit` 由来の `PAYLOAD_TOO_LARGE` を確実に拾い、それ以外（切断等）は従来どおり 400 に落ちる。判定に文字列比較を使っていない点も良い。
4. **M-1 は無音縮退でない**: fix-policy Batch 4 の必須要件「縮退時の `tracing::warn!`」を両経路（session/desktop）で確認。「全員 Cookie 認証が静かに効かなくなる」障害の追跡可能性が確保されている。

## 所見

- **[運用注意] refinery checksum divergence** — `V17__baseline.sql` はコミット済みの内容を書き換えているため、**旧 checksum が `refinery_schema_history` に記録済みの DB が存在すると次回起動で divergent エラー**になる（refinery デフォルトは abort_divergent）。これは `schema.rs` の Busy 判定にも掛からず恒久 Migration 失敗（Fatal）扱い。ブランチ未 push・本番未稼働のため現時点で実害はないが、**旧バイナリでマイグレーションを流した dev/テスト用データディレクトリがあれば `refinery_schema_history` の削除か DB 作り直しが必要**。カットオーバー後は baseline の書き換え自体を禁止し、以後の変更は V18+ で行うこと。
- **[記録] `schema.rs` のロック判定が文字列マッチ** — refinery がエラー型を潰すためやむを得ない妥協。SQLITE_BUSY の実メッセージ "database is locked" / SQLITE_LOCKED の "database table is locked" は現行パターンで拾えるが、rusqlite 更新で文言が変わると静かに恒久扱いへ戻る。コメントに経緯明記済みのため現状維持で可（Batch 7 の CI 化とは独立）。

## 総評

Batch 4/5/6（M-6）は fix-policy の完了条件をテストつきで満たし、加えて移行期の最重要ガード（Node による全 DROP 誤発動の防止）を同時に固めた。特筆すべきは (1) M-5 を「supervisor 再 spawn」でなく **catch_unwind による発生源隔離**で解いたことで、完了条件の趣旨をより強い不変条件（writer は panic で死なない）に置き換えている点、(2) M-6 で Node parity に留まらず再帰段への scope 検査で **厳格側に倒した**点。残る Batch は 2/3（静的配信・応答形状）と 6 の残り（M-7〜M-11 fail-closed）・7（LOW＋CI）。
