//! 定期報告の保存手順（Web-API とツールの共通経路）。
//!
//! 現在値（新規なら初期値）に変更内容を重ね、[`ReportSettings::validate`] を通してから書き込む。

use serde_json::Value;
use yuuka_core::DbError;
use yuuka_web::Db;

use crate::model::{PeriodicReport, ReportSettings, MAX_REPORTS};
use crate::repo::{self, WriteOutcome};

/// 保存できなかった理由。
#[derive(Debug)]
pub(crate) enum SaveError {
    /// 内容が不正（利用者向けメッセージ）。
    Invalid(String),
    /// 更新対象が無い。
    NotFound,
    Db(DbError),
}

impl From<DbError> for SaveError {
    fn from(e: DbError) -> Self {
        Self::Db(e)
    }
}

/// 定期報告を保存する。`id` が `None` なら新規登録、`Some` なら本人の既存行を部分更新する。
/// 返り値は保存後の内容。
pub(crate) async fn save_report(
    db: &Db,
    user_id: &str,
    bot_id: &str,
    id: Option<i64>,
    changes: &Value,
) -> Result<PeriodicReport, SaveError> {
    let mut settings = match id {
        Some(id) => {
            repo::find_report(db, user_id, bot_id, id)
                .await?
                .ok_or(SaveError::NotFound)?
                .settings
        }
        None => ReportSettings::default(),
    };
    settings
        .apply(changes)
        .and_then(|()| settings.validate())
        .map_err(SaveError::Invalid)?;

    let outcome = match id {
        Some(id) => repo::update_report(db, user_id, bot_id, id, settings.clone()).await?,
        None => repo::insert_report(db, user_id, bot_id, settings.clone()).await?,
    };
    match outcome {
        WriteOutcome::Saved(id) => Ok(PeriodicReport {
            id,
            user_id: user_id.to_owned(),
            bot_id: bot_id.to_owned(),
            settings,
        }),
        WriteOutcome::NameTaken => Err(SaveError::Invalid(format!(
            "「{}」という名前の定期報告は既にあります。",
            settings.name
        ))),
        WriteOutcome::LimitReached => Err(SaveError::Invalid(format!(
            "定期報告は{MAX_REPORTS}件まで登録できます。"
        ))),
        WriteOutcome::NotFound => Err(SaveError::NotFound),
    }
}
