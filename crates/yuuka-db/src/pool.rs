//! コネクション open（PRAGMA 明示）と read pool（内製・rusqlite 0.40 直用）。
//!
//! read pool は deadpool-sqlite が rusqlite 0.40 未対応（^0.38 頭打ち・links 衝突）のため
//! Phase 0 は内製する: `tokio::sync::Semaphore` で同時借用数を上限管理し、コネクションを
//! freelist で再利用、同期クエリは `spawn_blocking` でブロッキングプールへ逃がす。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};
use tokio::sync::{Mutex, Semaphore};
use yuuka_core::DbError;

use crate::map_sqlite;

/// 既定の読み取り並行数（= 開くコネクション上限）。
const DEFAULT_READ_POOL_SIZE: usize = 4;

/// PRAGMA を明示設定して SQLite コネクションを開く（§11.4）。
///
/// `read_only=true` は **`SQLITE_OPEN_READ_ONLY` + `query_only`** で開き、read クロージャ内の
/// 誤った書き込みを SQLite 層で拒否する（＝writer actor を通らない第二 writer 経路を機械排除・R-2）。
/// `read_only=false`（writer 専用）は `SQLITE_OPEN_READ_WRITE` で開くが **`CREATE` は付けない**:
/// 移行期は Node が唯一の schema 所有者で DB は既存前提のため、パス誤設定時に空 DB を新規作成せず
/// 即 open エラーにする（C-2）。
///
/// PRAGMA:
/// - writer: `journal_mode=WAL` / `foreign_keys=ON` / `busy_timeout=5000` / `synchronous=NORMAL`
/// - reader: `busy_timeout=5000` + `query_only=ON`（WAL/整合系は DB 全体設定ゆえ read では触らない）
///
/// # Errors
/// open または PRAGMA 設定に失敗した場合 [`DbError`]。
pub fn open_conn(path: &Path, read_only: bool) -> Result<Connection, DbError> {
    // rusqlite 既定フラグから CREATE を外す（read は READ_ONLY、write は READ_WRITE）。
    let mode_flag = if read_only {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    };
    let flags = mode_flag | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags).map_err(map_sqlite)?;
    apply_pragmas(&conn, read_only)?;
    Ok(conn)
}

fn apply_pragmas(conn: &Connection, read_only: bool) -> Result<(), DbError> {
    // busy_timeout は read/write 双方で有効（checkpoint 中の一時ロック待ち）。
    conn.busy_timeout(Duration::from_millis(5000))
        .map_err(map_sqlite)?;

    if read_only {
        // READ_ONLY 接続では journal_mode/foreign_keys/synchronous を設定しない
        // （書込・DB 全体設定であり read では不要／不可）。query_only で二重に書込を禁止。
        conn.execute_batch("PRAGMA query_only=ON;")
            .map_err(map_sqlite)?;
        return Ok(());
    }

    // writer 接続: journal_mode は結果行を返すため query_row で受ける（execute だとエラー）。
    let jmode: String = conn
        .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
        .map_err(map_sqlite)?;
    if !jmode.eq_ignore_ascii_case("wal") {
        // 共有 DB が既に別モードの可能性。縮退はせず記録のみ（起動判断は上位）。
        tracing::warn!(journal_mode = %jmode, "journal_mode is not WAL");
    }
    conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL;")
        .map_err(map_sqlite)?;
    Ok(())
}

struct PoolInner {
    path: PathBuf,
    idle: Mutex<Vec<Connection>>,
    permits: Semaphore,
}

/// 読み取り専用コネクションプール（WAL は複数リーダー並行可）。
///
/// 書き込みは必ず [`crate::WriterHandle`] へ送る（本プールは読み取り専用の想定）。
#[derive(Clone)]
pub struct ReadPool {
    inner: Arc<PoolInner>,
}

impl ReadPool {
    /// 指定 DB パスの read pool を作る（既定並行数 [`DEFAULT_READ_POOL_SIZE`]）。
    /// 生成時に 1 本開いて経路を検証する。
    ///
    /// # Errors
    /// 初期コネクションの open に失敗した場合 [`DbError`]。
    pub fn open(path: &Path) -> Result<Self, DbError> {
        Self::open_with_size(path, DEFAULT_READ_POOL_SIZE)
    }

    /// 並行数を指定して read pool を作る。
    ///
    /// # Errors
    /// 初期コネクションの open に失敗した場合 [`DbError`]。
    pub fn open_with_size(path: &Path, size: usize) -> Result<Self, DbError> {
        let size = size.max(1);
        let first = open_conn(path, true)?;
        let inner = PoolInner {
            path: path.to_path_buf(),
            idle: Mutex::new(vec![first]),
            permits: Semaphore::new(size),
        };
        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    /// プールからコネクションを借り、ブロッキングクロージャで読み取りを行う。
    ///
    /// クロージャ内で statement を確実に drop（finalize/reset）し、長寿命 reader が
    /// checkpoint を阻害しないようにする（R-3）。同時借用数は Semaphore で上限管理。
    ///
    /// # Errors
    /// コネクション取得・join・クエリのいずれかが失敗した場合 [`DbError`]。
    pub async fn read<F, T>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&Connection) -> Result<T, DbError> + Send + 'static,
        T: Send + 'static,
    {
        let _permit = self
            .inner
            .permits
            .acquire()
            .await
            .map_err(|_| DbError::Operation("read pool closed".to_owned()))?;

        // freelist から再利用、無ければ新規 open（permit 保持中＝上限内）。
        let conn = {
            let mut idle = self.inner.idle.lock().await;
            idle.pop()
        };
        let conn = match conn {
            Some(c) => c,
            None => open_conn(&self.inner.path, true)?,
        };

        // 同期 rusqlite をブロッキングプールで実行し、コネクションは呼び出し側へ返す。
        let (conn, out) = tokio::task::spawn_blocking(move || {
            let out = f(&conn);
            (conn, out)
        })
        .await
        .map_err(|e| DbError::Operation(format!("read join: {e}")))?;

        self.inner.idle.lock().await.push(conn);
        out
    }
}
