//! 監査ログ（`audit_logs` テーブル）— Node `src/db/auditRepo.ts` `addAuditLog` パリティ。
//!
//! `INSERT INTO audit_logs (user_id, action, target, detail)`（`created_at` は DB 既定
//! `datetime('now','localtime')`）。パスワード・API キー等の秘密値を `target`/`detail` に含めない。
//! 監査は**ベストエフォート**：失敗しても本処理（ログイン等）を落とさず `warn` に残すだけ。

use rusqlite::params;
use yuuka_db::map_sqlite;
use yuuka_web::Db;

/// 監査ログを 1 行書く（ベストエフォート・エラーは握って `warn`）。
///
/// Node `addAuditLog(userId, action, target?, detail?)` と同じ列順。`target`/`detail` は任意。
pub async fn add_audit_log(
    db: &Db,
    user_id: &str,
    action: &str,
    target: Option<&str>,
    detail: Option<&str>,
) {
    let user_id = user_id.to_owned();
    // ログ用に action を控える（closure へは別クローンを move する）。target/detail は
    // 秘密混入の恐れがあるためログに出さない。
    let action_for_log = action.to_owned();
    let action = action.to_owned();
    let target = target.map(str::to_owned);
    let detail = detail.map(str::to_owned);
    let result = db
        .writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT INTO audit_logs (user_id, action, target, detail) VALUES (?1, ?2, ?3, ?4)",
                params![user_id, action, target, detail],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await;
    if let Err(e) = result {
        // 監査失敗で本処理を落とさない（可観測性のためログは残す）。
        tracing::warn!(error = %e, action = %action_for_log, "監査ログの書き込みに失敗");
    }
}
