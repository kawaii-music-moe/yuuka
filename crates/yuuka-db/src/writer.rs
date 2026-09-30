//! 単一 writer actor（全書き込みを 1 タスク・1 コネクションへ直列化）。
//!
//! これにより「プロセス内の並行 writer → 即-BUSY / ライブロック」（R-2）を構造的に排除する。
//! マルチコアは読み側（[`crate::ReadPool`]）で活かす（HTTP は読みが支配的）。

use std::path::PathBuf;

use rusqlite::{Connection, TransactionBehavior};
use tokio::sync::{mpsc, oneshot};
use yuuka_core::DbError;

use crate::map_sqlite;
use crate::pool::open_conn;

/// writer スレッドへ送る型消去済みジョブ。結果は各ジョブが内包する oneshot で返す。
type WriteJob = Box<dyn FnOnce(&mut Connection) + Send>;

/// 単一 writer actor へのハンドル（`Clone` 可・全クローンが同一 writer へ直列送信）。
#[derive(Clone)]
pub struct WriterHandle {
    tx: mpsc::Sender<WriteJob>,
}

impl WriterHandle {
    /// writer コネクションを開き、専用スレッドで直列処理を開始する。
    ///
    /// コネクションは 1 本のみをこのスレッドが所有し、他所からは触れない（並行 writer 排除）。
    /// DB ファイルが存在しない場合は `CREATE` を付けずに即エラーとなる（C-2）。新規インスタンスの
    /// ブートストラップ（無ければ作る）が必要な場合は [`Self::spawn_bootstrapping`] を使う。
    ///
    /// # Errors
    /// コネクション open（PRAGMA 含む）やスレッド生成に失敗した場合 [`DbError`]。
    pub fn spawn(path: PathBuf) -> Result<Self, DbError> {
        // writer は READ_WRITE で開く（CREATE は付けない）。
        let conn = open_conn(&path, false)?;
        Self::spawn_with_conn(conn)
    }

    /// [`Self::spawn`] と同じだが、DB ファイルが存在しなければ新規作成してから開く
    /// （`SQLITE_OPEN_CREATE`・親ディレクトリも無ければ作成）。既存ファイルには影響しない。
    ///
    /// 呼び出し側が [`crate::pool::init_db_enabled`]（`YUUKA_INIT_DB=1` 等）で明示オプトイン
    /// した場合にのみ使うこと。誤設定パスで無条件に空 DB を作ってしまわないよう、既定経路
    /// （[`Self::spawn`]）は `CREATE` を付けない（issue #55）。
    ///
    /// # Errors
    /// コネクション open（親ディレクトリ作成・PRAGMA 含む）やスレッド生成、migrations 失敗時
    /// [`DbError`]。
    pub fn spawn_bootstrapping(path: PathBuf) -> Result<Self, DbError> {
        let conn = crate::pool::create_conn(&path)?;
        Self::spawn_with_conn(conn)
    }

    /// 開いた writer コネクションに migrations を適用し、専用スレッドで直列処理を開始する
    /// （[`Self::spawn`] / [`Self::spawn_bootstrapping`] 共通部）。
    fn spawn_with_conn(mut conn: Connection) -> Result<Self, DbError> {
        // Phase 5: Rust が DDL の所有権を持ち、起動時にマイグレーションを適用する。
        crate::schema::run_migrations(&mut conn)?;

        let (tx, mut rx) = mpsc::channel::<WriteJob>(256);
        std::thread::Builder::new()
            .name("yuuka-db-writer".to_owned())
            .spawn(move || {
                // 非同期ランタイム外の専用スレッドなので blocking_recv でよい。
                while let Some(job) = rx.blocking_recv() {
                    // M-5: ジョブ内 panic（debug の整数オーバーフロー等）を **タスク境界で隔離**する。
                    // catch_unwind が無いと 1 発の panic で writer スレッドが unwind して死に、
                    // 以後の全書き込みが恒久的に `WriterGone` になる（この writer は生 std::thread で
                    // supervisor の JoinSet 監督外のため誰も再 spawn しない）。panic した job の
                    // oneshot sender は unwind 中に drop され、その呼び出しだけが `WriterGone` を
                    // 受け取る（他ジョブは継続）。進行中 Tx は Drop でロールバックされ conn は健全。
                    let outcome =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&mut conn)));
                    if outcome.is_err() {
                        tracing::error!(
                            "yuuka-db writer: ジョブが panic。タスク境界で隔離し writer を継続します"
                        );
                    }
                }
            })
            .map_err(|e| DbError::Operation(format!("spawn writer thread: {e}")))?;
        Ok(Self { tx })
    }

    /// 任意の書き込みクロージャを writer actor で直列実行する。
    ///
    /// トランザクションが要る通常経路は [`WriterHandle::transaction`] を使う
    /// （BEGIN IMMEDIATE を強制するため）。
    ///
    /// # Errors
    /// writer 消失時 [`DbError::WriterGone`]、クロージャ内失敗時はその [`DbError`]。
    pub async fn execute<F, T>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&mut Connection) -> Result<T, DbError> + Send + 'static,
        T: Send + 'static,
    {
        let (rtx, rrx) = oneshot::channel::<Result<T, DbError>>();
        let job: WriteJob = Box::new(move |conn| {
            // 受信側が drop 済みでも send の失敗は無視（結果を待つ者がいないだけ）。
            let _ = rtx.send(f(conn));
        });
        self.tx.send(job).await.map_err(|_| DbError::WriterGone)?;
        rrx.await.map_err(|_| DbError::WriterGone)?
    }

    /// **全書込 Tx の既定経路**: `BEGIN IMMEDIATE` で開始し、DEFERRED→write
    /// アップグレードの即-BUSY（R-1）を回避する。クロージャには IMMEDIATE Tx を渡す。
    ///
    /// # Errors
    /// Tx 開始・クロージャ・commit のいずれかが失敗した場合 [`DbError`]。
    pub async fn transaction<F, T>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&rusqlite::Transaction) -> Result<T, DbError> + Send + 'static,
        T: Send + 'static,
    {
        self.execute(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite)?;
            let out = f(&tx)?;
            tx.commit().map_err(map_sqlite)?;
            Ok(out)
        })
        .await
    }
}
