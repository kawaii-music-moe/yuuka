//! クリップボード TTL クリーンアップ（§3.10.3・現行 `clipboardCleanupService.ts`）。
//!
//! 毎時 0 分＋起動直後に、期限切れの `clipboard_entries` を削除する。クリップボードは
//! ユーザー向け CRUD ドメイン crate が未整備のため、cron 専用 SQL を本 crate が直接持つ
//! （ドメイン整備時に repo へ移す）。

use async_trait::async_trait;
use yuuka_core::DbError;
use yuuka_db::map_sqlite;

use crate::context::ServiceContext;
use crate::schedule::{CronService, Schedule};

/// 期限切れクリップボードエントリを削除する（削除件数を返す・現行 `deleteExpired`）。
async fn delete_expired(ctx: &ServiceContext) -> Result<usize, DbError> {
    ctx.db
        .writer
        .execute(|conn| {
            let n = conn
                .execute(
                    "DELETE FROM clipboard_entries \
                     WHERE expires_at IS NOT NULL AND expires_at <= datetime('now', 'localtime')",
                    [],
                )
                .map_err(map_sqlite)?;
            Ok(n)
        })
        .await
}

/// 期限切れクリップボードを定期削除するサービス。
pub struct ClipboardCleanupService;

#[async_trait]
impl CronService for ClipboardCleanupService {
    fn name(&self) -> &'static str {
        "clipboard"
    }

    fn schedule(&self) -> Schedule {
        Schedule::Hourly // "0 * * * *"
    }

    async fn tick(&self, ctx: &ServiceContext) {
        match delete_expired(ctx).await {
            Ok(n) if n > 0 => {
                tracing::info!(removed = n, "🧹 [Clipboard] 期限切れメモを削除しました")
            }
            Ok(_) => {}
            Err(e) => tracing::error!(error = %e, "[Clipboard] クリーンアップに失敗しました"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ctx_null, seeded_db};

    const CLIPBOARD_DDL: &str = "CREATE TABLE clipboard_entries (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL DEFAULT 'system_default',
        content TEXT NOT NULL,
        expires_at TEXT
    );";

    #[tokio::test]
    async fn deletes_only_expired_entries() {
        let (db, _dir) = seeded_db(CLIPBOARD_DDL);
        db.writer
            .execute(|conn| {
                conn.execute_batch(
                    "INSERT INTO clipboard_entries (user_id, content, expires_at) VALUES \
                     ('u','past', datetime('now','localtime','-1 hour')), \
                     ('u','future', datetime('now','localtime','+1 hour')), \
                     ('u','never', NULL);",
                )
                .map_err(map_sqlite)?;
                Ok(())
            })
            .await
            .unwrap();

        let ctx = ctx_null(db);
        assert_eq!(delete_expired(&ctx).await.unwrap(), 1);

        let remaining: i64 = ctx
            .db
            .read
            .read(|conn| {
                conn.query_row("SELECT COUNT(*) FROM clipboard_entries", [], |r| r.get(0))
                    .map_err(map_sqlite)
            })
            .await
            .unwrap();
        assert_eq!(remaining, 2);
    }
}
