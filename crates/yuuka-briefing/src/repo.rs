//! briefing/report 配信設定のデータアクセス（Node `briefingConfigRepo`/`reportConfigRepo`）。
//!
//! `briefing_configs`（PK `(user_id, bot_id)`）と `report_configs`（UNIQUE `(user_id, bot_id, type)`）。
//! `weather_lat/lng`・feeds/keywords（JSON 配列）等の完全な briefing 更新は configureBriefing 側で
//! 扱う（本crateは report 更新 + 設定読み取りに限定）。

use rusqlite::params;
use serde_json::Value;
use yuuka_core::{CrossUserAccess, DbError};
use yuuka_db::map_sqlite;
use yuuka_web::Db;

/// briefing 設定の読み取りビュー（Node `getBriefingConfig` の主要フィールド）。
#[derive(Debug, Default, Clone)]
pub struct BriefingConfig {
    pub enabled: bool,
    pub schedule_cron: String,
    pub target_type: String,
    pub target_id: Option<String>,
    pub weather_lat: Option<f64>,
    pub weather_lng: Option<f64>,
    pub location_name: Option<String>,
    pub news_feeds: Vec<String>,
    pub news_keywords: Vec<String>,
}

/// briefing 設定の部分更新（present なフィールドのみ現在値へ重ねる・Node `key in obj` 意味論）。
#[derive(Debug, Default)]
pub struct BriefingPatch {
    pub enabled: Option<bool>,
    pub schedule_cron: Option<String>,
    /// `Some`=配信先種別を設定（'dm'/'channel'）・`None`=触らない。
    pub target_type: Option<String>,
    /// `Some(inner)`=`target_id` 列を設定（`inner=None` は NULL）・`None`=触らない。
    pub target_id: Option<Option<String>>,
    /// `Some(inner)`=`weather_lat` 列を設定（`inner=None` は NULL）・`None`=触らない。
    pub weather_lat: Option<Option<f64>>,
    pub weather_lng: Option<Option<f64>>,
    /// `Some(inner)`=`location_name` 列を設定（`inner=None` は NULL）・`None`=触らない。
    pub location_name: Option<Option<String>>,
    /// `Some` のときのみ `news_feeds` 列を更新する（add/remove 適用後の全体）。
    pub news_feeds: Option<Vec<String>>,
    pub news_keywords: Option<Vec<String>>,
}

/// report 設定 1 件（Node `ReportConfigRecord`）。
#[derive(Debug)]
pub struct ReportConfig {
    pub r#type: String,
    pub enabled: bool,
    pub schedule_cron: String,
    pub target_type: String,
    pub target_id: Option<String>,
}

/// briefing 設定を返す（未設定は既定値・Node `getBriefingConfig` は行が無ければ defaults）。
/// tool 側（configureBriefing の部分マージ・getBriefingConfig）で使う。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_briefing(db: &Db, user_id: &str, bot_id: &str) -> Result<BriefingConfig, DbError> {
    Ok(find_briefing(db, user_id, bot_id)
        .await?
        .unwrap_or_else(|| BriefingConfig {
            schedule_cron: "0 7 * * *".to_owned(),
            target_type: "dm".to_owned(),
            ..BriefingConfig::default()
        }))
}

/// briefing 設定の行を取得する（**行が無ければ `None`**・既定値へ畳まない）。
/// Web `GET /api/briefing-config` は未設定を `config: null` で返すため生の有無が要る。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn find_briefing(
    db: &Db,
    user_id: &str,
    bot_id: &str,
) -> Result<Option<BriefingConfig>, DbError> {
    let (u, b) = (user_id.to_owned(), bot_id.to_owned());
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT enabled, schedule_cron, target_type, target_id, weather_lat, \
                            weather_lng, location_name, news_feeds, news_keywords \
                     FROM briefing_configs WHERE user_id = ?1 AND bot_id = ?2",
                )
                .map_err(map_sqlite)?;
            let mut rows = stmt
                .query_map(params![u, b], |row| {
                    Ok(BriefingConfig {
                        enabled: row.get::<_, i64>(0)? != 0,
                        schedule_cron: row.get(1)?,
                        target_type: row.get(2)?,
                        target_id: row.get(3)?,
                        weather_lat: row.get(4)?,
                        weather_lng: row.get(5)?,
                        location_name: row.get(6)?,
                        news_feeds: parse_json_array(&row.get::<_, String>(7)?),
                        news_keywords: parse_json_array(&row.get::<_, String>(8)?),
                    })
                })
                .map_err(map_sqlite)?;
            match rows.next() {
                Some(v) => Ok(Some(v.map_err(map_sqlite)?)),
                None => Ok(None),
            }
        })
        .await
}

/// cron 走査用の有効 briefing 設定 1 件（所有者列 `user_id`/`bot_id` + due 判定用 `schedule_cron`）。
///
/// 定時配信ループはこの一覧を走査し、`schedule_cron` が現在分にマッチする設定について
/// [`crate::build_briefing`]（`user_id`/`bot_id` で本人設定を再読込）→配信を行う。
#[derive(Debug, Clone)]
pub struct EnabledBriefing {
    pub user_id: String,
    pub bot_id: String,
    pub schedule_cron: String,
    /// 'dm' | 'channel'。
    pub target_type: String,
    pub target_id: Option<String>,
}

/// 有効な（`enabled=1`）朝報設定を**全ユーザー横断**で返す（Node `listEnabledBriefingConfigsAcrossUsers`
/// ＝`SELECT * FROM briefing_configs WHERE enabled = 1`）。cron 式の due 判定は呼び出し側（croner 保持層）。
///
/// 横断アクセスは cron/バッチ起点でしか作れない（[`CrossUserAccess`] 証憑必須・§7.3）。playbook の
/// `list_enabled_schedules` と同規律で `UserScope` 経路から隔離する。
///
/// # Errors
/// クエリ失敗時 [`DbError`]。
pub async fn list_enabled_briefings(
    db: &Db,
    _cross: CrossUserAccess,
) -> Result<Vec<EnabledBriefing>, DbError> {
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT user_id, bot_id, schedule_cron, target_type, target_id \
                     FROM briefing_configs WHERE enabled = 1 ORDER BY user_id ASC, bot_id ASC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(EnabledBriefing {
                        user_id: row.get(0)?,
                        bot_id: row.get(1)?,
                        schedule_cron: row.get(2)?,
                        target_type: row.get(3)?,
                        target_id: row.get(4)?,
                    })
                })
                .map_err(map_sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(map_sqlite)?);
            }
            Ok(out)
        })
        .await
}

/// briefing 設定を部分更新で upsert する（Node `upsertBriefingConfig`＝現在値に patch を重ねる）。
/// 返り値は更新後の設定。
///
/// # Errors
/// 書き込み・取得失敗時 [`DbError`]。
pub async fn upsert_briefing(
    db: &Db,
    user_id: &str,
    bot_id: &str,
    patch: BriefingPatch,
) -> Result<BriefingConfig, DbError> {
    // 現在値（無ければ既定）に patch を重ねる。
    let mut cfg = get_briefing(db, user_id, bot_id).await?;
    if let Some(v) = patch.enabled {
        cfg.enabled = v;
    }
    if let Some(v) = patch.schedule_cron {
        cfg.schedule_cron = v;
    }
    if let Some(v) = patch.target_type {
        cfg.target_type = v;
    }
    if let Some(v) = patch.target_id {
        cfg.target_id = v;
    }
    if let Some(v) = patch.weather_lat {
        cfg.weather_lat = v;
    }
    if let Some(v) = patch.weather_lng {
        cfg.weather_lng = v;
    }
    if let Some(v) = patch.location_name {
        cfg.location_name = v;
    }
    if let Some(v) = patch.news_feeds {
        cfg.news_feeds = v;
    }
    if let Some(v) = patch.news_keywords {
        cfg.news_keywords = v;
    }
    let out = cfg.clone();

    let (u, b) = (user_id.to_owned(), bot_id.to_owned());
    let feeds_json = serde_json::to_string(&cfg.news_feeds).unwrap_or_else(|_| "[]".to_owned());
    let kw_json = serde_json::to_string(&cfg.news_keywords).unwrap_or_else(|_| "[]".to_owned());
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT INTO briefing_configs \
                   (user_id, bot_id, enabled, schedule_cron, target_type, target_id, \
                    weather_lat, weather_lng, location_name, news_feeds, news_keywords) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
                 ON CONFLICT(user_id, bot_id) DO UPDATE SET \
                   enabled = excluded.enabled, schedule_cron = excluded.schedule_cron, \
                   target_type = excluded.target_type, target_id = excluded.target_id, \
                   weather_lat = excluded.weather_lat, weather_lng = excluded.weather_lng, \
                   location_name = excluded.location_name, news_feeds = excluded.news_feeds, \
                   news_keywords = excluded.news_keywords, \
                   updated_at = datetime('now', 'localtime')",
                params![
                    u,
                    b,
                    i64::from(cfg.enabled),
                    cfg.schedule_cron,
                    cfg.target_type,
                    cfg.target_id,
                    cfg.weather_lat,
                    cfg.weather_lng,
                    cfg.location_name,
                    feeds_json,
                    kw_json,
                ],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await?;
    Ok(out)
}

/// report 設定一覧を type 昇順で返す（Node `getReportConfigs`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn get_reports(
    db: &Db,
    user_id: &str,
    bot_id: &str,
) -> Result<Vec<ReportConfig>, DbError> {
    let (u, b) = (user_id.to_owned(), bot_id.to_owned());
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT type, enabled, schedule_cron, target_type, target_id \
                     FROM report_configs WHERE user_id = ?1 AND bot_id = ?2 ORDER BY type ASC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![u, b], |row| {
                    Ok(ReportConfig {
                        r#type: row.get(0)?,
                        enabled: row.get::<_, i64>(1)? != 0,
                        schedule_cron: row.get(2)?,
                        target_type: row.get(3)?,
                        target_id: row.get(4)?,
                    })
                })
                .map_err(map_sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(map_sqlite)?);
            }
            Ok(out)
        })
        .await
}

/// 指定 type の report 設定を取得（partial merge の現在値用）。
async fn get_report(
    db: &Db,
    user_id: &str,
    bot_id: &str,
    r#type: &str,
) -> Result<Option<ReportConfig>, DbError> {
    let (u, b, t) = (user_id.to_owned(), bot_id.to_owned(), r#type.to_owned());
    db.read
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT type, enabled, schedule_cron, target_type, target_id \
                     FROM report_configs WHERE user_id = ?1 AND bot_id = ?2 AND type = ?3",
                )
                .map_err(map_sqlite)?;
            let mut rows = stmt
                .query_map(params![u, b, t], |row| {
                    Ok(ReportConfig {
                        r#type: row.get(0)?,
                        enabled: row.get::<_, i64>(1)? != 0,
                        schedule_cron: row.get(2)?,
                        target_type: row.get(3)?,
                        target_id: row.get(4)?,
                    })
                })
                .map_err(map_sqlite)?;
            match rows.next() {
                Some(v) => Ok(Some(v.map_err(map_sqlite)?)),
                None => Ok(None),
            }
        })
        .await
}

/// report 設定の部分更新（present なフィールドのみ現在値に重ねる・Node `key in obj` 意味論）。
#[derive(Debug, Default)]
pub struct ReportPatch {
    pub enabled: Option<bool>,
    pub schedule_cron: Option<String>,
    pub target_type: Option<String>,
    /// `Some(inner)`=列を設定（`inner=None` は NULL）・`None`=触らない。
    pub target_id: Option<Option<String>>,
}

/// report 設定を部分更新で upsert する（Node `upsertReportConfig`＝現在値に partial を重ねる）。
/// 返り値は更新後の設定。
///
/// # Errors
/// 書き込み・取得失敗時 [`DbError`]。
pub async fn upsert_report(
    db: &Db,
    user_id: &str,
    bot_id: &str,
    r#type: &str,
    patch: ReportPatch,
) -> Result<ReportConfig, DbError> {
    // 現在値（無ければ既定）にpartialを重ねる。
    let current = get_report(db, user_id, bot_id, r#type).await?;
    let enabled = patch
        .enabled
        .unwrap_or_else(|| current.as_ref().is_some_and(|c| c.enabled));
    let cron = patch.schedule_cron.unwrap_or_else(|| {
        current
            .as_ref()
            .map_or_else(|| "0 21 * * *".to_owned(), |c| c.schedule_cron.clone())
    });
    let ttype = patch.target_type.unwrap_or_else(|| {
        current
            .as_ref()
            .map_or_else(|| "dm".to_owned(), |c| c.target_type.clone())
    });
    let tid = match patch.target_id {
        Some(v) => v,
        None => current.as_ref().and_then(|c| c.target_id.clone()),
    };

    let (u, b, t) = (user_id.to_owned(), bot_id.to_owned(), r#type.to_owned());
    let cron_c = cron.clone();
    let ttype_c = ttype.clone();
    let tid_c = tid.clone();
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT INTO report_configs \
                   (user_id, bot_id, type, enabled, schedule_cron, target_type, target_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
                 ON CONFLICT(user_id, bot_id, type) DO UPDATE SET \
                   enabled = excluded.enabled, schedule_cron = excluded.schedule_cron, \
                   target_type = excluded.target_type, target_id = excluded.target_id, \
                   updated_at = datetime('now', 'localtime')",
                params![u, b, t, i64::from(enabled), cron_c, ttype_c, tid_c],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await?;
    Ok(ReportConfig {
        r#type: r#type.to_owned(),
        enabled,
        schedule_cron: cron,
        target_type: ttype,
        target_id: tid,
    })
}

/// JSON 配列文字列を `Vec<String>` へ（破損は空・Node `parseJsonArray`）。
fn parse_json_array(raw: &str) -> Vec<String> {
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Array(arr)) => arr
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}
