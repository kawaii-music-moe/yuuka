-- 定期報告: 朝報（briefing_configs）と日報・週報（report_configs）を、名前を付けて複数登録できる 1 つの
-- 仕組みへまとめる。載せる内容は区分キーの JSON 配列（weather / news / tasks / schedules / payments /
-- finance / topics）で、tasks・schedules・payments・finance を含むものは DM にだけ送る。
-- 旧テーブルは消さずに残す（以降は読み書きしない）。
CREATE TABLE periodic_reports (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        bot_id TEXT NOT NULL,
        name TEXT NOT NULL,
        enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
        schedule_cron TEXT NOT NULL,
        target_type TEXT NOT NULL DEFAULT 'dm' CHECK (target_type IN ('dm', 'channel')),
        target_id TEXT,
        sections TEXT NOT NULL DEFAULT '[]',
        location_name TEXT,
        weather_lat REAL,
        weather_lng REAL,
        news_feeds TEXT NOT NULL DEFAULT '[]',
        news_keywords TEXT NOT NULL DEFAULT '[]',
        created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        UNIQUE (user_id, bot_id, name),
        FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
      );
CREATE INDEX idx_periodic_reports_enabled ON periodic_reports(enabled);

-- 朝報 → 「朝報」。天気（地点名か緯度経度）・ニュース（フィード 1 件以上）のうち設定済みのものだけを
-- 載せる内容にする。どちらも未設定の朝報は中身が無いため引き継がない。
INSERT INTO periodic_reports
  (user_id, bot_id, name, enabled, schedule_cron, target_type, target_id, sections,
   location_name, weather_lat, weather_lng, news_feeds, news_keywords, updated_at)
SELECT user_id, bot_id, '朝報', enabled, schedule_cron,
       CASE WHEN target_type = 'channel' AND COALESCE(target_id, '') <> '' THEN 'channel' ELSE 'dm' END,
       CASE WHEN target_type = 'channel' AND COALESCE(target_id, '') <> '' THEN target_id END,
       CASE
         WHEN has_weather AND has_news THEN '["weather","news"]'
         WHEN has_weather THEN '["weather"]'
         ELSE '["news"]'
       END,
       location_name, weather_lat, weather_lng,
       CASE WHEN has_news THEN news_feeds ELSE '[]' END,
       CASE WHEN json_valid(news_keywords) THEN news_keywords ELSE '[]' END,
       updated_at
FROM (
  SELECT *,
         (weather_lat IS NOT NULL AND weather_lng IS NOT NULL)
           OR COALESCE(TRIM(location_name), '') <> '' AS has_weather,
         CASE WHEN json_valid(news_feeds) THEN json_array_length(news_feeds) ELSE 0 END > 0 AS has_news
  FROM briefing_configs
)
WHERE has_weather OR has_news;

-- 日報・週報 → 「日報」「週報」。載せる内容は V34 の sections（未設定は活動データ全部）。
-- タスク・予定・支払い予定・家計を含むものは DM にだけ送るため、配信先を DM にする。
INSERT INTO periodic_reports
  (user_id, bot_id, name, enabled, schedule_cron, target_type, target_id, sections, updated_at)
SELECT user_id, bot_id, name, enabled, schedule_cron,
       CASE WHEN has_private OR target_type <> 'channel' OR COALESCE(target_id, '') = '' THEN 'dm' ELSE 'channel' END,
       CASE WHEN has_private OR target_type <> 'channel' OR COALESCE(target_id, '') = '' THEN NULL ELSE target_id END,
       sections, updated_at
FROM (
  SELECT *,
         EXISTS (
           SELECT 1 FROM json_each(sections)
           WHERE value IN ('tasks', 'schedules', 'payments', 'finance')
         ) AS has_private
  FROM (
    SELECT user_id, bot_id, enabled, schedule_cron, target_type, target_id, updated_at,
           CASE type WHEN 'weekly' THEN '週報' ELSE '日報' END AS name,
           CASE
             WHEN sections IS NULL OR NOT json_valid(sections)
               THEN '["tasks","schedules","payments","finance","topics"]'
             WHEN json_array_length(sections) > 0 THEN sections
             ELSE '["tasks","schedules","payments","finance","topics"]'
           END AS sections
    FROM report_configs
    WHERE type IN ('daily', 'weekly')
  )
);
