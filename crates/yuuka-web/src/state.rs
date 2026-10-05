//! アプリ共有状態（axum `State`）とドメイン共通の DB ハンドル束。

use std::path::Path;
use std::sync::Arc;

use axum::extract::FromRef;
use yuuka_core::DbError;
use yuuka_db::{pool::init_db_enabled, ReadPool, WriterHandle};

use crate::auth::AuthBackend;
use crate::config::WebConfig;

/// ドメイン repo が使う DB ハンドル束（read pool + 単一 writer actor）。
///
/// 各ドメイン（T1）はこの束から `TodoRepo` 等を per-request に安価に構築する
/// （writer は全ドメイン単一の直列 actor、read は共有プール）。
#[derive(Clone)]
pub struct Db {
    /// 読み取り専用プール（READ_ONLY・複数リーダー並行）。
    pub read: ReadPool,
    /// 単一 writer actor（全書き込みを直列化）。
    pub writer: WriterHandle,
}

impl Db {
    /// 既存 DB ファイルから read pool と writer actor を開く。
    ///
    /// 既定は DB ファイルが事前に用意されていることを前提とし、無ければ即エラーで起動失敗する
    /// （誤設定パスでの意図しない空 DB 生成を防ぐ・C-2）。`YUUKA_INIT_DB=1`
    /// （[`yuuka_db::pool::INIT_DB_ENV`]）を明示設定した場合のみ、無ければ新規作成して
    /// baseline から migrations を適用する（新規インスタンスのブートストラップ・issue #55）。
    ///
    /// writer を先に開く: ブートストラップ時は writer 側が DB ファイルと schema を作るため、
    /// read pool はその後に開くことで「新規作成したばかりでまだ存在しないファイル」を
    /// read-only で開いて失敗する事態を避ける。
    ///
    /// # Errors
    /// コネクション open / writer 起動に失敗した場合 [`DbError`]。
    pub fn open(path: &Path) -> Result<Self, DbError> {
        let writer = if init_db_enabled() {
            WriterHandle::spawn_bootstrapping(path.to_path_buf())?
        } else {
            WriterHandle::spawn(path.to_path_buf())?
        };
        let read = ReadPool::open(path)?;
        Ok(Self { read, writer })
    }
}

/// ルータ全体で共有する状態。認証・設定・DB ハンドルを保持し、ドメインハンドラは
/// `State<AppState>`（または `State<Db>` サブステート）で必要な部分を取り出す。
#[derive(Clone)]
pub struct AppState {
    /// 認証バックエンド（Cookie=Redis / Bearer=SQLite）。
    pub auth: Arc<dyn AuthBackend>,
    /// web 実行時設定。
    pub config: Arc<WebConfig>,
    /// ドメイン共通 DB ハンドル。
    pub db: Db,
}

impl AppState {
    /// 認証バックエンド・設定・DB から状態を作る。
    #[must_use]
    pub fn new(auth: Arc<dyn AuthBackend>, config: WebConfig, db: Db) -> Self {
        Self {
            auth,
            config: Arc::new(config),
            db,
        }
    }
}

/// ドメインハンドラが `State<Db>` で DB ハンドルだけを取り出せるようにする。
impl FromRef<AppState> for Db {
    fn from_ref(state: &AppState) -> Self {
        state.db.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::Db;
    use std::sync::Mutex;
    use yuuka_db::pool::INIT_DB_ENV;

    /// `YUUKA_INIT_DB` はプロセス環境変数なので、これを操作するテストは直列化する
    /// （他テストはこの変数を読まないため通常は不要だが、将来の追加に備えた保険）。
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn open_fails_fast_on_missing_db_by_default() {
        // 既定（YUUKA_INIT_DB 未設定）では、Node 撤去後も無ければ即エラー（C-2 維持・issue #55）。
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var(INIT_DB_ENV);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.sqlite");
        assert!(Db::open(&path).is_err());
        assert!(!path.exists(), "must not create an empty db file");
    }

    #[test]
    fn open_bootstraps_brand_new_db_when_init_db_enabled() {
        // issue #55: YUUKA_INIT_DB=1 を明示すると、新規インスタンス向けに無ければ DB を
        // 作成し baseline migrations を適用したうえで read pool + writer を開く。
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var(INIT_DB_ENV, "1");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fresh.sqlite");
        assert!(!path.exists(), "precondition: db must not exist yet");
        let opened = Db::open(&path);
        std::env::remove_var(INIT_DB_ENV);

        let db = opened.expect("bootstrapping open should succeed");
        assert!(path.exists(), "db file must be created");

        // schema が使える（system_settings に baseline スタンプがある）ことを確認。
        let version: String = rusqlite::Connection::open(&path)
            .unwrap()
            .query_row(
                "SELECT value FROM system_settings WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, "17");
        drop(db);
    }
}
