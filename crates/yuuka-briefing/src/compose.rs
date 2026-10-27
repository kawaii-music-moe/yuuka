//! 定期報告 1 件を Embed にする（定時配信・テスト配信・`runPeriodicReportNow` の共通経路）。
//!
//! 構成: タイトル「📋 名前 期間」→ 説明に活動データのまとめ（補助生成・使えなければ生データ）→
//! 件数のインラインフィールド → 天気 → ニュース。フッタにデータの出どころを並べる。

use chrono::{DateTime, Local};
use yuuka_core::aux_text::AuxTextGenerator;
use yuuka_core::{DbError, EmbedFieldPart, EmbedPart};
use yuuka_web::Db;

use crate::activity::{self, ReportWindow};
use crate::model::{PeriodicReport, ReportSection};
use crate::service;

/// 定期報告の Embed の色。
pub const REPORT_COLOR: u32 = 0x58_65_F2;
/// Discord の Embed 全体の上限（6000 文字）に余裕を持たせた値。
const EMBED_BUDGET: usize = 5900;
/// 説明欄の上限（Discord は 4096 文字）。
const DESCRIPTION_MAX: usize = 4000;
/// フィールド名の上限（Discord は 256 文字）。
const FIELD_NAME_MAX: usize = 256;

/// 定期報告を `now` 時点の内容で生成する。期間は配信スケジュールから決める（[`ReportWindow`]）。
///
/// # Errors
/// 活動データの読み取り失敗時 [`DbError`]。要約素材の読み取り失敗は生データ本文へ縮退する。
pub async fn build_report_embed(
    db: &Db,
    aux_text: &dyn AuxTextGenerator,
    report: &PeriodicReport,
    now: DateTime<Local>,
) -> Result<EmbedPart, DbError> {
    let settings = &report.settings;
    let window = ReportWindow::for_schedule(&settings.schedule_cron, now);

    // 天気・ニュースは外部取得なので並行させる。
    let weather = async {
        if settings.has(ReportSection::Weather) {
            Some(service::weather_field(settings).await)
        } else {
            None
        }
    };
    let news = async {
        if settings.has(ReportSection::News) {
            Some(service::news_field(aux_text, &report.user_id, settings).await)
        } else {
            None
        }
    };
    let (weather, news) = tokio::join!(weather, news);

    let mut description = None;
    let mut fields = Vec::new();
    let mut footer = Vec::new();
    if settings.has_activity() {
        let data = activity::collect(db, &report.user_id, &report.bot_id, &window).await?;
        // 素材の読み取りも含め、要約経路の失敗で生データ配信を失わない。
        let summary = match activity::summary_prompt(db, report, &window, &data).await {
            Ok(prompt) => aux_text
                .generate_aux_text(&report.user_id, &prompt)
                .await
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty()),
            Err(e) => {
                tracing::warn!(user = %report.user_id, report = report.id, error = %e,
                    "定期報告の要約素材の取得に失敗（生データへ縮退）");
                None
            }
        };
        footer.push(if summary.is_some() {
            "Yuuka 自動生成レポート"
        } else {
            "Yuuka レポート（生データ）"
        });
        description = Some(summary.unwrap_or_else(|| activity::raw_body(&data, settings)));
        fields.extend(activity::count_fields(&data, settings));
    }
    for (field, source) in [
        (weather, "天気: Open-Meteo"),
        (news, "ニュース: 登録RSSフィード"),
    ] {
        if let Some((name, value)) = field {
            fields.push(EmbedFieldPart {
                name: name.chars().take(FIELD_NAME_MAX).collect(),
                value,
                inline: false,
            });
            footer.push(source);
        }
    }

    let title = format!("📋 {} {}", settings.name, window.label());
    let footer = footer.join(" / ");
    // 説明欄は Embed 全体の上限に収まる長さで切る。
    let used = title.chars().count()
        + footer.chars().count()
        + fields
            .iter()
            .map(|f| f.name.chars().count() + f.value.chars().count())
            .sum::<usize>();
    let max_description = EMBED_BUDGET.saturating_sub(used).min(DESCRIPTION_MAX);
    Ok(EmbedPart {
        title: Some(title),
        description: description.map(|d| d.chars().take(max_description).collect()),
        color: REPORT_COLOR,
        fields,
        footer: (!footer.is_empty()).then_some(footer),
    })
}
