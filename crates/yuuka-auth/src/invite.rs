//! 招待コード（`invite_codes` テーブル）— Node `src/db/inviteRepo.ts` パリティ。
//!
//! コードは「存在 ∧ 未使用（`used_by IS NULL`）∧ 未失効（`revoked_at IS NULL`）」で有効。消費は
//! 単一条件付き UPDATE でアトミック（同時 2 者のうち 1 者だけが `changes>0` を得る）。起動時シードは
//! `INSERT OR IGNORE`（冪等）で `created_by = NULL`。

use rusqlite::params;
use yuuka_db::map_sqlite;
use yuuka_core::DbError;
use yuuka_web::Db;

/// コードが有効（存在・未使用・未失効）か（Node `isValidCode`）。消費はしない（事前検証用）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn is_valid_code(db: &Db, code: &str) -> Result<bool, DbError> {
    let code = code.to_owned();
    db.read
        .read(move |conn| {
            let found: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM invite_codes \
                     WHERE code = ?1 AND used_by IS NULL AND revoked_at IS NULL LIMIT 1",
                    params![code],
                    |r| r.get(0),
                )
                .ok();
            Ok(found.is_some())
        })
        .await
}

/// コードをアトミックに消費する（Node `validateAndConsumeCode`）。未使用・未失効のときだけ
/// `used_by`/`used_at` を設定し `true`。既に使用/失効済み・不在なら `false`。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn validate_and_consume_code(
    db: &Db,
    code: &str,
    used_by_discord_id: &str,
) -> Result<bool, DbError> {
    let code = code.to_owned();
    let used_by = used_by_discord_id.to_owned();
    db.writer
        .transaction(move |tx| {
            let changed = tx
                .execute(
                    "UPDATE invite_codes \
                     SET used_by = ?1, used_at = datetime('now', 'localtime') \
                     WHERE code = ?2 AND used_by IS NULL AND revoked_at IS NULL",
                    params![used_by, code],
                )
                .map_err(map_sqlite)?;
            Ok(changed > 0)
        })
        .await
}

/// 起動時に招待コードを DB へ投入する（Node `seedInitialCodes`）。`INSERT OR IGNORE` で冪等。
///
/// 空・空白のみのコードはスキップする。シード行の `created_by` は `NULL`。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn seed_initial_codes(db: &Db, codes: &[String]) -> Result<usize, DbError> {
    let codes: Vec<String> = codes
        .iter()
        .map(|c| c.trim().to_owned())
        .filter(|c| !c.is_empty())
        .collect();
    if codes.is_empty() {
        return Ok(0);
    }
    db.writer
        .transaction(move |tx| {
            let mut inserted = 0usize;
            for code in &codes {
                inserted += tx
                    .execute(
                        "INSERT OR IGNORE INTO invite_codes (code, created_by) VALUES (?1, NULL)",
                        params![code],
                    )
                    .map_err(map_sqlite)?;
            }
            Ok(inserted)
        })
        .await
}
