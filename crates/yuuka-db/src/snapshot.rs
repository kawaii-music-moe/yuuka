//! DB の一貫したスナップショット（`VACUUM INTO`）。
//!
//! 稼働中（WAL・書き込み中）の DB からも、読み取りトランザクション 1 本で 1 ファイルのコピーを作る。
//! 保存先は WAL を使わない形（`journal_mode=DELETE`）に直し、`PRAGMA integrity_check` を通してから、
//! 持ち主だけが読める権限（0600）にする。`yuuka backup`（定期バックアップ・issue #241）と
//! xtask `salvage-snapshot`（旧環境からの移行）が共用する。

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};
use yuuka_core::DbError;

use crate::map_sqlite;

/// `from` の一貫したコピーを `to` に書く。`to` が既にあれば上書きせずエラーにする。
///
/// # Errors
/// 保存先が既にある・親ディレクトリを作れない・コピーや整合性検査に失敗した場合 [`DbError::Operation`]。
/// 整合性検査に失敗したコピーは残さない。
pub fn snapshot(from: &Path, to: &Path) -> Result<(), DbError> {
    if to.exists() {
        return Err(DbError::Operation(format!(
            "{} は既にあります。別の保存先を指定してください。",
            to.display()
        )));
    }
    if let Some(dir) = to.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| DbError::Operation(format!("{} を作れません: {e}", dir.display())))?;
    }
    let target = to.to_str().ok_or_else(|| {
        DbError::Operation(format!(
            "保存先のパスが UTF-8 ではありません: {}",
            to.display()
        ))
    })?;

    let source = Connection::open_with_flags(
        from,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| DbError::Operation(format!("{} を開けません: {e}", from.display())))?;
    // 書き込み中でも少し待てば読み取りスナップショットを取れる。
    source
        .busy_timeout(Duration::from_secs(5))
        .map_err(map_sqlite)?;
    source
        .execute("VACUUM INTO ?1", [target])
        .map_err(|e| DbError::Operation(format!("スナップショットに失敗: {e}")))?;
    drop(source);

    let verified = verify_copy(to);
    if let Err(e) = verified {
        // 壊れたコピーを残すと、戻すときにそれを選んでしまう。
        let _ = std::fs::remove_file(to);
        return Err(e);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // 会話ログや暗号化された認証情報を含むため、持ち主だけが読めるようにする。
        std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| DbError::Operation(format!("権限を設定できません: {e}")))?;
    }
    Ok(())
}

/// コピーを 1 ファイルで持ち運べる形（WAL なし）に直し、整合性を検査する。
fn verify_copy(to: &Path) -> Result<(), DbError> {
    let copy = Connection::open_with_flags(
        to,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(map_sqlite)?;
    copy.execute_batch("PRAGMA journal_mode = DELETE;")
        .map_err(map_sqlite)?;
    let check: String = copy
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .map_err(map_sqlite)?;
    if check != "ok" {
        return Err(DbError::Operation(format!(
            "スナップショットの整合性検査に失敗: {check}"
        )));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn seeded_source(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("live.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; \
             CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT NOT NULL); \
             INSERT INTO notes (body) VALUES ('a'), ('b'), ('c');",
        )
        .unwrap();
        path
    }

    #[test]
    fn copies_rows_from_a_wal_database_into_a_single_file() {
        let dir = tempfile::tempdir().unwrap();
        let from = seeded_source(dir.path());
        // WAL に書き込み中のまま（接続を閉じない）でもスナップショットが取れる。
        let writer = Connection::open(&from).unwrap();
        writer
            .execute("INSERT INTO notes (body) VALUES ('d')", [])
            .unwrap();

        let to = dir.path().join("backups").join("copy.db");
        snapshot(&from, &to).unwrap();

        let copy = Connection::open_with_flags(&to, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let n: i64 = copy
            .query_row("SELECT count(*) FROM notes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 4);
        let mode: String = copy
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "delete");
        assert!(!to.with_extension("db-wal").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&to).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn refuses_to_overwrite_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let from = seeded_source(dir.path());
        let to = dir.path().join("copy.db");
        std::fs::write(&to, b"keep me").unwrap();
        let err = snapshot(&from, &to).unwrap_err();
        assert!(err.to_string().contains("既にあります"), "{err}");
        assert_eq!(std::fs::read(&to).unwrap(), b"keep me");
    }

    #[test]
    fn missing_source_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = snapshot(&dir.path().join("none.db"), &dir.path().join("copy.db")).unwrap_err();
        assert!(err.to_string().contains("開けません"), "{err}");
    }
}
