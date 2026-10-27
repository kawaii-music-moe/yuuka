//! 定期報告（`periodic_reports`）のデータアクセス。
//!
//! 行は登録したユーザー・Bot（`user_id` + `bot_id`）に属し、Web-API・ツールは本人分だけを読み書きする。
//! 全ユーザー横断の走査は定時配信（[`list_enabled_reports`]・[`CrossUserAccess`] 証憑必須）だけ。
//! 名前の重複と登録上限は、書き込みと同じトランザクションの中で確かめる。

use rusqlite::{params, OptionalExtension, Row};
use serde_json::Value;
use yuuka_core::{CrossUserAccess, DbError};
use yuuka_db::map_sqlite;
use yuuka_web::Db;

use crate::model::{PeriodicReport, ReportSection, ReportSettings, TargetType, MAX_REPORTS};

const COLUMNS: &str = "id, user_id, bot_id, name, enabled, schedule_cron, target_type, target_id, \
                       sections, location_name, weather_lat, weather_lng, news_feeds, news_keywords";

/// 書き込みの結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriteOutcome {
    /// 保存できた（行の id）。
    Saved(i64),
    /// 同じユーザー・Bot に同じ名前の定期報告がある。
    NameTaken,
    /// 登録上限（[`MAX_REPORTS`]）に達している。
    LimitReached,
    /// 更新対象の行が無い（他人の行・削除済み）。
    NotFound,
}

/// 本人の定期報告を登録順に返す。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_reports(
    db: &Db,
    user_id: &str,
    bot_id: &str,
) -> Result<Vec<PeriodicReport>, DbError> {
    let (u, b) = (user_id.to_owned(), bot_id.to_owned());
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM periodic_reports \
                     WHERE user_id = ?1 AND bot_id = ?2 ORDER BY id ASC"
                ))
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![u, b], report_from_row)
                .map_err(map_sqlite)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(map_sqlite)
        })
        .await
}

/// 本人の定期報告を 1 件返す（他人の行・無い id は `None`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn find_report(
    db: &Db,
    user_id: &str,
    bot_id: &str,
    id: i64,
) -> Result<Option<PeriodicReport>, DbError> {
    let (u, b) = (user_id.to_owned(), bot_id.to_owned());
    db.read
        .read(move |conn| {
            conn.query_row(
                &format!(
                    "SELECT {COLUMNS} FROM periodic_reports \
                     WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3"
                ),
                params![id, u, b],
                report_from_row,
            )
            .optional()
            .map_err(map_sqlite)
        })
        .await
}

/// 有効な定期報告を**全ユーザー横断**で返す（定時配信の走査用・cron の判定は呼び出し側）。
///
/// 横断アクセスは cron/バッチ起点でしか作れない（[`CrossUserAccess`] 証憑必須・§7.3）。
///
/// # Errors
/// クエリ失敗時 [`DbError`]。
pub async fn list_enabled_reports(
    db: &Db,
    _cross: CrossUserAccess,
) -> Result<Vec<PeriodicReport>, DbError> {
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM periodic_reports WHERE enabled = 1 \
                     ORDER BY user_id ASC, bot_id ASC, id ASC"
                ))
                .map_err(map_sqlite)?;
            let rows = stmt.query_map([], report_from_row).map_err(map_sqlite)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(map_sqlite)
        })
        .await
}

/// 検証済みの内容で新しく登録する。
pub(crate) async fn insert_report(
    db: &Db,
    user_id: &str,
    bot_id: &str,
    settings: ReportSettings,
) -> Result<WriteOutcome, DbError> {
    let (u, b) = (user_id.to_owned(), bot_id.to_owned());
    db.writer
        .transaction(move |tx| {
            let count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM periodic_reports WHERE user_id = ?1 AND bot_id = ?2",
                    params![u, b],
                    |r| r.get(0),
                )
                .map_err(map_sqlite)?;
            if usize::try_from(count).unwrap_or(usize::MAX) >= MAX_REPORTS {
                return Ok(WriteOutcome::LimitReached);
            }
            if name_taken(tx, &u, &b, &settings.name, None)? {
                return Ok(WriteOutcome::NameTaken);
            }
            tx.execute(
                "INSERT INTO periodic_reports \
                   (user_id, bot_id, name, enabled, schedule_cron, target_type, target_id, sections, \
                    location_name, weather_lat, weather_lng, news_feeds, news_keywords) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    u,
                    b,
                    settings.name,
                    i64::from(settings.enabled),
                    settings.schedule_cron,
                    settings.target_type.key(),
                    settings.target_id,
                    sections_json(&settings.sections),
                    settings.location_name,
                    settings.weather_lat,
                    settings.weather_lng,
                    string_array_json(&settings.news_feeds),
                    string_array_json(&settings.news_keywords),
                ],
            )
            .map_err(map_sqlite)?;
            Ok(WriteOutcome::Saved(tx.last_insert_rowid()))
        })
        .await
}

/// 本人の定期報告を検証済みの内容で置き換える。
pub(crate) async fn update_report(
    db: &Db,
    user_id: &str,
    bot_id: &str,
    id: i64,
    settings: ReportSettings,
) -> Result<WriteOutcome, DbError> {
    let (u, b) = (user_id.to_owned(), bot_id.to_owned());
    db.writer
        .transaction(move |tx| {
            if name_taken(tx, &u, &b, &settings.name, Some(id))? {
                return Ok(WriteOutcome::NameTaken);
            }
            let changed = tx
                .execute(
                    "UPDATE periodic_reports SET \
                       name = ?4, enabled = ?5, schedule_cron = ?6, target_type = ?7, \
                       target_id = ?8, sections = ?9, location_name = ?10, weather_lat = ?11, \
                       weather_lng = ?12, news_feeds = ?13, news_keywords = ?14, \
                       updated_at = datetime('now', 'localtime') \
                     WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3",
                    params![
                        id,
                        u,
                        b,
                        settings.name,
                        i64::from(settings.enabled),
                        settings.schedule_cron,
                        settings.target_type.key(),
                        settings.target_id,
                        sections_json(&settings.sections),
                        settings.location_name,
                        settings.weather_lat,
                        settings.weather_lng,
                        string_array_json(&settings.news_feeds),
                        string_array_json(&settings.news_keywords),
                    ],
                )
                .map_err(map_sqlite)?;
            Ok(if changed == 0 {
                WriteOutcome::NotFound
            } else {
                WriteOutcome::Saved(id)
            })
        })
        .await
}

/// 本人の定期報告を削除する（消えたら `true`）。
pub(crate) async fn delete_report(
    db: &Db,
    user_id: &str,
    bot_id: &str,
    id: i64,
) -> Result<bool, DbError> {
    let (u, b) = (user_id.to_owned(), bot_id.to_owned());
    db.writer
        .execute(move |conn| {
            let deleted = conn
                .execute(
                    "DELETE FROM periodic_reports WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3",
                    params![id, u, b],
                )
                .map_err(map_sqlite)?;
            Ok(deleted > 0)
        })
        .await
}

fn name_taken(
    tx: &rusqlite::Transaction<'_>,
    user_id: &str,
    bot_id: &str,
    name: &str,
    exclude_id: Option<i64>,
) -> Result<bool, DbError> {
    tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM periodic_reports \
         WHERE user_id = ?1 AND bot_id = ?2 AND name = ?3 AND id IS NOT ?4)",
        params![user_id, bot_id, name, exclude_id],
        |r| r.get(0),
    )
    .map_err(map_sqlite)
}

/// 1 行（[`COLUMNS`] の順）を読む。
fn report_from_row(row: &Row<'_>) -> rusqlite::Result<PeriodicReport> {
    let target_type = if row.get::<_, String>(6)? == "channel" {
        TargetType::Channel
    } else {
        TargetType::Dm
    };
    Ok(PeriodicReport {
        id: row.get(0)?,
        user_id: row.get(1)?,
        bot_id: row.get(2)?,
        settings: ReportSettings {
            name: row.get(3)?,
            enabled: row.get::<_, i64>(4)? != 0,
            schedule_cron: row.get(5)?,
            target_type,
            target_id: row.get(7)?,
            sections: parse_json_array(&row.get::<_, String>(8)?)
                .iter()
                .filter_map(|key| ReportSection::from_key(key))
                .collect(),
            location_name: row.get(9)?,
            weather_lat: row.get(10)?,
            weather_lng: row.get(11)?,
            news_feeds: parse_json_array(&row.get::<_, String>(12)?),
            news_keywords: parse_json_array(&row.get::<_, String>(13)?),
        },
    })
}

fn sections_json(sections: &[ReportSection]) -> String {
    Value::from(sections.iter().map(|s| s.key()).collect::<Vec<_>>()).to_string()
}

fn string_array_json(items: &[String]) -> String {
    Value::from(items.to_vec()).to_string()
}

/// JSON 配列文字列を `Vec<String>` へ（破損は空）。
fn parse_json_array(raw: &str) -> Vec<String> {
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Array(arr)) => arr
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}
