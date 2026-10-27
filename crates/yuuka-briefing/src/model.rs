//! 定期報告のモデル（載せる内容の区分・登録内容・保存前の検証）。
//!
//! Web-API（`routes`）と LLM ツール（`tools`）は、どちらも変更内容を JSON（API と同じキー）で
//! [`ReportSettings::apply`] に渡し、[`ReportSettings::validate`] を通してから保存する。検証規則を
//! 1 か所にまとめ、経路によって保存できる内容が変わらないようにする。

use serde_json::Value;
use yuuka_core::cron_util;

/// 1 ユーザー・1 Bot あたりの登録上限。
pub const MAX_REPORTS: usize = 20;
/// 名前の最大文字数。
pub const MAX_NAME_CHARS: usize = 40;
/// RSS フィードの登録上限（取得側も先頭 10 件まで）。
pub const MAX_FEEDS: usize = 10;
/// ニュース絞り込みキーワードの上限。
pub const MAX_KEYWORDS: usize = 20;

/// 定期報告に載せる内容の区分（`periodic_reports.sections` の JSON 配列要素）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportSection {
    /// 天気予報（Open-Meteo）。
    Weather,
    /// 登録 RSS フィードのニュース。
    News,
    /// 完了・持ち越しタスク。
    Tasks,
    /// これからの予定。
    Schedules,
    /// 支払い予定。
    Payments,
    /// 家計（収入・支出）。
    Finance,
    /// 会話のトピック（補助生成のまとめにだけ載る）。
    Topics,
}

impl ReportSection {
    /// 全区分（表示・保存の順）。
    pub const ALL: [Self; 7] = [
        Self::Weather,
        Self::News,
        Self::Tasks,
        Self::Schedules,
        Self::Payments,
        Self::Finance,
        Self::Topics,
    ];

    /// 保存・API で使う区分キー。
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Weather => "weather",
            Self::News => "news",
            Self::Tasks => "tasks",
            Self::Schedules => "schedules",
            Self::Payments => "payments",
            Self::Finance => "finance",
            Self::Topics => "topics",
        }
    }

    /// 区分キーから引く（未知のキーは `None`）。
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.key() == key)
    }

    /// 本人だけが見るべき内容か。1 つでも含む定期報告はチャンネルへ送らず DM にだけ送る。
    #[must_use]
    pub const fn is_private(self) -> bool {
        matches!(
            self,
            Self::Tasks | Self::Schedules | Self::Payments | Self::Finance
        )
    }

    /// 期間の活動データ（集計と補助生成のまとめの対象）か。天気・ニュース以外。
    #[must_use]
    pub const fn is_activity(self) -> bool {
        !matches!(self, Self::Weather | Self::News)
    }
}

/// 配信先の種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TargetType {
    /// 登録したユーザー本人への DM。
    #[default]
    Dm,
    /// `target_id` のチャンネル。
    Channel,
}

impl TargetType {
    /// 保存・API で使うキー。
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Dm => "dm",
            Self::Channel => "channel",
        }
    }
}

/// 定期報告の登録内容（利用者が編集できる部分）。
#[derive(Debug, Clone, PartialEq)]
pub struct ReportSettings {
    pub name: String,
    pub enabled: bool,
    pub schedule_cron: String,
    pub target_type: TargetType,
    /// チャンネル指定時のチャンネル ID（DM のときは `None`）。
    pub target_id: Option<String>,
    /// 載せる内容（表示順・重複なし）。
    pub sections: Vec<ReportSection>,
    /// 天気の地点名（緯度経度が無いときはこの名前から地点を探す）。
    pub location_name: Option<String>,
    pub weather_lat: Option<f64>,
    pub weather_lng: Option<f64>,
    pub news_feeds: Vec<String>,
    pub news_keywords: Vec<String>,
}

impl Default for ReportSettings {
    /// 新規登録の初期値（有効・DM・内容未選択）。
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: true,
            schedule_cron: String::new(),
            target_type: TargetType::Dm,
            target_id: None,
            sections: Vec::new(),
            location_name: None,
            weather_lat: None,
            weather_lng: None,
            news_feeds: Vec::new(),
            news_keywords: Vec::new(),
        }
    }
}

impl ReportSettings {
    /// 指定の内容を載せるか。
    #[must_use]
    pub fn has(&self, section: ReportSection) -> bool {
        self.sections.contains(&section)
    }

    /// 本人だけが見るべき内容（[`ReportSection::is_private`]）を含むか。
    #[must_use]
    pub fn has_private(&self) -> bool {
        self.sections.iter().any(|s| s.is_private())
    }

    /// 期間の活動データ（[`ReportSection::is_activity`]）を含むか。
    #[must_use]
    pub fn has_activity(&self) -> bool {
        self.sections.iter().any(|s| s.is_activity())
    }

    /// 送り先のチャンネル ID。DM 宛て・ID 未設定・非公開の内容を含むときは `None`（＝本人への DM）。
    ///
    /// 保存時の検証に加えて送信直前にもここで判定し、非公開の内容がチャンネルへ出ないようにする。
    #[must_use]
    pub fn channel_target(&self) -> Option<&str> {
        if self.target_type != TargetType::Channel || self.has_private() {
            return None;
        }
        self.target_id.as_deref().filter(|id| !id.is_empty())
    }

    /// 変更内容（API と同じキーの JSON オブジェクト）のうち、含まれるキーだけを現在値へ重ねる。
    /// 値の型が違うときは利用者向けメッセージの `Err`。中身の妥当性は [`Self::validate`] で見る。
    ///
    /// # Errors
    /// オブジェクトでない・値の型が違うとき。
    pub fn apply(&mut self, changes: &Value) -> Result<(), String> {
        let Value::Object(map) = changes else {
            return Err("変更内容はオブジェクトで指定してください。".to_owned());
        };
        for (key, value) in map {
            match key.as_str() {
                "name" => self.name = required_string(key, value)?,
                "enabled" => {
                    self.enabled = value
                        .as_bool()
                        .ok_or_else(|| format!("{key} は true / false で指定してください。"))?;
                }
                "schedule_cron" => self.schedule_cron = required_string(key, value)?,
                "target_type" => {
                    self.target_type = match value.as_str() {
                        Some("dm") => TargetType::Dm,
                        Some("channel") => TargetType::Channel,
                        _ => {
                            return Err(
                                "target_type は 'dm' か 'channel' を指定してください。".to_owned()
                            )
                        }
                    };
                }
                "target_id" => self.target_id = optional_string(key, value)?,
                "sections" => {
                    let keys = string_array(key, value)?;
                    let mut picked = Vec::with_capacity(keys.len());
                    for k in &keys {
                        let section = ReportSection::from_key(k).ok_or_else(|| {
                            format!(
                                "sections に知らない内容「{k}」があります。{}",
                                section_keys()
                            )
                        })?;
                        picked.push(section);
                    }
                    self.sections = picked;
                }
                "location_name" => self.location_name = optional_string(key, value)?,
                "weather_lat" => self.weather_lat = optional_number(key, value)?,
                "weather_lng" => self.weather_lng = optional_number(key, value)?,
                "news_feeds" => self.news_feeds = string_array(key, value)?,
                "news_keywords" => self.news_keywords = string_array(key, value)?,
                // 認可スコープ用の botId や id は呼び出し側が読む。未知のキーは無視する。
                _ => {}
            }
        }
        Ok(())
    }

    /// 保存前に正規化して検証する。名前・キーワードの前後空白を除き、内容を表示順へ並べ、
    /// DM 宛てならチャンネル ID を消す。
    ///
    /// # Errors
    /// 保存できない内容のとき、利用者向けメッセージ。
    pub fn validate(&mut self) -> Result<(), String> {
        self.name = self.name.trim().to_owned();
        if self.name.is_empty() {
            return Err("名前を入力してください。".to_owned());
        }
        if self.name.chars().count() > MAX_NAME_CHARS {
            return Err(format!("名前は{MAX_NAME_CHARS}文字以内にしてください。"));
        }

        self.schedule_cron = self.schedule_cron.trim().to_owned();
        if !cron_util::is_valid(&self.schedule_cron) {
            return Err("配信スケジュールの cron 式が不正です。".to_owned());
        }

        let picked = std::mem::take(&mut self.sections);
        self.sections = ReportSection::ALL
            .into_iter()
            .filter(|s| picked.contains(s))
            .collect();
        if self.sections.is_empty() {
            return Err(format!(
                "報告する内容を1つ以上選んでください。{}",
                section_keys()
            ));
        }

        self.target_id = self
            .target_id
            .take()
            .map(|id| id.trim().to_owned())
            .filter(|id| !id.is_empty());
        match self.target_type {
            TargetType::Dm => self.target_id = None,
            TargetType::Channel => {
                if self.has_private() {
                    return Err(
                        "タスク・予定・支払い予定・家計を含む定期報告は DM にしか送れません。"
                            .to_owned(),
                    );
                }
                if self.target_id.is_none() {
                    return Err("報告先のチャンネルIDを入力してください。".to_owned());
                }
            }
        }

        self.location_name = self
            .location_name
            .take()
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty());
        if let Some(lat) = self.weather_lat {
            if !(-90.0..=90.0).contains(&lat) {
                return Err("緯度は -90〜90 の範囲で指定してください。".to_owned());
            }
        }
        if let Some(lng) = self.weather_lng {
            if !(-180.0..=180.0).contains(&lng) {
                return Err("経度は -180〜180 の範囲で指定してください。".to_owned());
            }
        }
        if self.weather_lat.is_some() != self.weather_lng.is_some() {
            return Err("緯度と経度は両方指定してください。".to_owned());
        }
        if self.has(ReportSection::Weather)
            && self.location_name.is_none()
            && self.weather_lat.is_none()
        {
            return Err("天気を載せるときは地点名か緯度・経度を指定してください。".to_owned());
        }

        let feeds = std::mem::take(&mut self.news_feeds);
        for feed in feeds {
            let feed = feed.trim().to_owned();
            if feed.is_empty() || self.news_feeds.contains(&feed) {
                continue;
            }
            if !is_likely_public_http_url(&feed) {
                return Err(format!(
                    "RSSフィードは http(s) の公開URLのみ登録できます（内部アドレスは不可）: {feed}"
                ));
            }
            self.news_feeds.push(feed);
        }
        if self.news_feeds.len() > MAX_FEEDS {
            return Err(format!("RSSフィードは{MAX_FEEDS}件まで登録できます。"));
        }
        if self.has(ReportSection::News) && self.news_feeds.is_empty() {
            return Err("ニュースを載せるときはRSSフィードを1件以上登録してください。".to_owned());
        }

        let keywords = std::mem::take(&mut self.news_keywords);
        for keyword in keywords {
            let keyword = keyword.trim().to_owned();
            if !keyword.is_empty() && !self.news_keywords.contains(&keyword) {
                self.news_keywords.push(keyword);
            }
        }
        if self.news_keywords.len() > MAX_KEYWORDS {
            return Err(format!(
                "ニュース絞り込みキーワードは{MAX_KEYWORDS}件までにしてください。"
            ));
        }
        Ok(())
    }
}

/// 登録済みの定期報告 1 件。
#[derive(Debug, Clone, PartialEq)]
pub struct PeriodicReport {
    pub id: i64,
    /// 登録したユーザー（データの持ち主・DM の宛先）。
    pub user_id: String,
    pub bot_id: String,
    pub settings: ReportSettings,
}

/// API・ツールへ返す定期報告の表現（キーは [`ReportSettings::apply`] と同じ）。
/// `private` は非公開の内容を含む（＝DM にだけ送る）かどうか。
pub(crate) fn report_view(report: &PeriodicReport) -> Value {
    let s = &report.settings;
    serde_json::json!({
        "id": report.id,
        "name": s.name,
        "enabled": s.enabled,
        "schedule_cron": s.schedule_cron,
        "target_type": s.target_type.key(),
        "target_id": s.target_id,
        "sections": s.sections.iter().map(|x| x.key()).collect::<Vec<_>>(),
        "location_name": s.location_name,
        "weather_lat": s.weather_lat,
        "weather_lng": s.weather_lng,
        "news_feeds": s.news_feeds,
        "news_keywords": s.news_keywords,
        "private": s.has_private(),
    })
}

/// フィード登録時の公開 URL ポリシー。取得時は別途 DNS を含む厳密検査を行う。
pub(crate) fn is_likely_public_http_url(raw: &str) -> bool {
    yuuka_outbound::is_likely_public_http_url(raw, yuuka_outbound::RegistrationPolicy::PublicHttp)
}

/// 選べる区分キーの案内文。
fn section_keys() -> String {
    let keys: Vec<&str> = ReportSection::ALL.iter().map(|s| s.key()).collect();
    format!("（{}）", keys.join(" / "))
}

fn required_string(key: &str, value: &Value) -> Result<String, String> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{key} は文字列で指定してください。"))
}

/// 文字列または null（null と空文字は未設定）。
fn optional_string(key: &str, value: &Value) -> Result<Option<String>, String> {
    match value {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s.clone())),
        _ => Err(format!("{key} は文字列で指定してください。")),
    }
}

/// 数値または null（null は未設定）。
fn optional_number(key: &str, value: &Value) -> Result<Option<f64>, String> {
    match value {
        Value::Null => Ok(None),
        Value::Number(n) => Ok(n.as_f64()),
        _ => Err(format!("{key} は数値で指定してください。")),
    }
}

fn string_array(key: &str, value: &Value) -> Result<Vec<String>, String> {
    let Value::Array(items) = value else {
        return Err(format!("{key} は文字列の配列で指定してください。"));
    };
    items
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{key} は文字列の配列で指定してください。"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn valid() -> ReportSettings {
        let mut s = ReportSettings::default();
        s.apply(&json!({
            "name": " 朝の報告 ",
            "schedule_cron": "0 7 * * *",
            "sections": ["topics", "weather", "weather"],
            "location_name": "東京",
        }))
        .unwrap();
        s
    }

    #[test]
    fn validate_normalizes_name_and_section_order() {
        let mut s = valid();
        s.validate().unwrap();
        assert_eq!(s.name, "朝の報告");
        assert_eq!(s.sections, [ReportSection::Weather, ReportSection::Topics]);
        assert!(s.enabled);
    }

    #[test]
    fn private_sections_cannot_go_to_channel() {
        let mut s = valid();
        s.apply(&json!({"target_type": "channel", "target_id": "123"}))
            .unwrap();
        // 天気・会話トピックだけならチャンネルへ送れる。
        s.validate().unwrap();
        assert_eq!(s.channel_target(), Some("123"));

        s.apply(&json!({"sections": ["weather", "tasks"]})).unwrap();
        let err = s.clone().validate().unwrap_err();
        assert!(err.contains("DM にしか送れません"), "{err}");
        // 保存済みの値が規則に反していても、送信先は DM に落とす。
        assert_eq!(s.channel_target(), None);
    }

    #[test]
    fn dm_target_drops_channel_id() {
        let mut s = valid();
        s.apply(&json!({"target_type": "dm", "target_id": "123"}))
            .unwrap();
        s.validate().unwrap();
        assert_eq!(s.target_id, None);
    }

    #[test]
    fn rejects_incomplete_settings() {
        for (changes, expected) in [
            (json!({"name": "  "}), "名前を入力"),
            (json!({"schedule_cron": "bad"}), "cron 式が不正"),
            (json!({"sections": []}), "1つ以上"),
            (json!({"sections": ["horoscope"]}), "知らない内容"),
            (
                json!({"target_type": "channel", "target_id": " "}),
                "チャンネルID",
            ),
            (json!({"location_name": null}), "地点名か緯度・経度"),
            (json!({"weather_lat": 35.6}), "両方"),
            (json!({"weather_lat": 95.0, "weather_lng": 0.0}), "緯度は"),
            (json!({"sections": ["news"]}), "RSSフィードを1件以上"),
            (
                json!({"sections": ["news"], "news_feeds": ["http://127.0.0.1/rss"]}),
                "公開URLのみ",
            ),
            (json!({"enabled": "yes"}), "true / false"),
        ] {
            let mut s = valid();
            let result = s.apply(&changes).and_then(|()| s.validate());
            let err = result.expect_err(&changes.to_string());
            assert!(err.contains(expected), "{changes}: {err}");
        }
    }

    #[test]
    fn feeds_and_keywords_are_trimmed_and_deduplicated() {
        let mut s = valid();
        s.apply(&json!({
            "sections": ["news"],
            "news_feeds": [" https://example.com/rss ", "https://example.com/rss", ""],
            "news_keywords": [" AI ", "AI", ""],
        }))
        .unwrap();
        s.validate().unwrap();
        assert_eq!(s.news_feeds, ["https://example.com/rss"]);
        assert_eq!(s.news_keywords, ["AI"]);
    }
}
