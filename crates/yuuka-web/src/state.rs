//! アプリ共有状態（axum `State`）とドメイン共通の DB ハンドル束。

use std::path::Path;
use std::sync::Arc;

use axum::extract::FromRef;
use yuuka_core::DbError;
use yuuka_db::{ReadPool, WriterHandle};

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
    /// 既存 DB ファイルから read pool と writer actor を開く（本番は Node 作成済み前提）。
    ///
    /// # Errors
    /// コネクション open / writer 起動に失敗した場合 [`DbError`]。
    pub fn open(path: &Path) -> Result<Self, DbError> {
        Ok(Self {
            read: ReadPool::open(path)?,
            writer: WriterHandle::spawn(path.to_path_buf())?,
        })
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
