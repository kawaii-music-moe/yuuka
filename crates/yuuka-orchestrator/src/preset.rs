//! Bot プリセット/ケーパビリティ解決サービス（Node `services/botCapabilities.ts`）。
//!
//! Bot 単位の capabilities（JSON 配列）とプリセット（内部 ID 固定・表示名は管理ページから変更可）を扱う。
//! capabilities のパース（不正値→秘書相当フォールバック）は [`crate::bot_repo::parse_capabilities`] に既存の
//! ため本モジュールはプリセット定義・逆引き・適用・表示名（`system_settings` 上書き）を担う。
//! Node のインメモリキャッシュは持たない（capabilities は都度 `bots` 行から解決するため不要）。

use rusqlite::params;
use yuuka_core::{CapabilitySet, DbError};
use yuuka_db::map_sqlite;
use yuuka_web::Db;

/// プリセットの内部 ID（Node `BotPresetId`・固定・表示名のみ変更可）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BotPresetId {
    Secretary,
    McpAssistant,
}

impl BotPresetId {
    /// 保存/API 上の文字列 ID。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Secretary => "secretary",
            Self::McpAssistant => "mcp_assistant",
        }
    }

    /// 文字列 ID から解決（未知は `None`・Node `presetInput in BOT_PRESETS`）。
    #[must_use]
    pub fn from_id(s: &str) -> Option<Self> {
        match s {
            "secretary" => Some(Self::Secretary),
            "mcp_assistant" => Some(Self::McpAssistant),
            _ => None,
        }
    }

    /// プリセットの capabilities（Node `BOT_PRESETS[id].capabilities`・`core` は暗黙のため含めない）。
    #[must_use]
    pub fn capabilities(self) -> &'static [&'static str] {
        match self {
            Self::Secretary => &["persona", "memory", "mcp", "secretary"],
            Self::McpAssistant => &["persona", "memory", "mcp"],
        }
    }

    /// 既定表示名（Node `defaultDisplayName`）。
    #[must_use]
    pub fn default_display_name(self) -> &'static str {
        match self {
            Self::Secretary => "パーソナル秘書",
            Self::McpAssistant => "汎用モード",
        }
    }

    /// capabilities の JSON 配列文字列（`bots.capabilities` へ保存する形）。
    #[must_use]
    pub fn capabilities_json(self) -> String {
        serde_json::to_string(self.capabilities()).unwrap_or_else(|_| "[]".to_owned())
    }

    /// 全プリセット（`list_presets` の反復用）。
    #[must_use]
    pub fn all() -> [Self; 2] {
        [Self::Secretary, Self::McpAssistant]
    }
}

/// capabilities からプリセット ID を逆引きする（Node `presetIdForCapabilities`）。
/// `secretary` を持てば秘書・持たなければ汎用モード。
#[must_use]
pub fn preset_id_for_capabilities(caps: &CapabilitySet) -> BotPresetId {
    if caps.has("secretary") {
        BotPresetId::Secretary
    } else {
        BotPresetId::McpAssistant
    }
}

/// Bot へプリセットを適用する（`bots.capabilities` を更新・Node `applyBotPreset`）。行が動けば `true`。
/// 認可・監査は呼び出し側。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn apply_bot_preset(db: &Db, bot_id: &str, preset: BotPresetId) -> Result<bool, DbError> {
    let (bot_id, json) = (bot_id.to_owned(), preset.capabilities_json());
    db.writer
        .transaction(move |tx| {
            let n = tx
                .execute(
                    "UPDATE bots SET capabilities = ?1, updated_at = datetime('now','localtime') \
                     WHERE id = ?2",
                    params![json, bot_id],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
        .await
}

/// プリセットの表示名（`system_settings` 上書き or 既定・Node `getPresetDisplayName`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn preset_display_name(db: &Db, preset: BotPresetId) -> Result<String, DbError> {
    let key = display_name_key(preset);
    Ok(get_system_setting(db, &key)
        .await?
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| preset.default_display_name().to_owned()))
}

/// プリセット表示名を設定する（trim→空は既定・50 字トリム・Node `setPresetDisplayName`）。
///
/// # Errors
/// 書き込み失敗時 [`DbError`]。
pub async fn set_preset_display_name(
    db: &Db,
    preset: BotPresetId,
    display_name: &str,
) -> Result<(), DbError> {
    let trimmed = display_name.trim();
    let value = if trimmed.is_empty() {
        preset.default_display_name().to_owned()
    } else {
        trimmed.chars().take(50).collect::<String>()
    };
    set_system_setting(db, &display_name_key(preset), &value).await
}

/// UI 用プリセット 1 件（Node `listPresets` の要素）。
#[derive(Debug, Clone)]
pub struct PresetView {
    pub id: &'static str,
    pub display_name: String,
    pub capabilities: Vec<&'static str>,
}

/// 全プリセットの `{ id, display_name, capabilities }`（Node `listPresets`）。
///
/// # Errors
/// 読み取り失敗時 [`DbError`]。
pub async fn list_presets(db: &Db) -> Result<Vec<PresetView>, DbError> {
    let mut out = Vec::with_capacity(2);
    for preset in BotPresetId::all() {
        out.push(PresetView {
            id: preset.as_str(),
            display_name: preset_display_name(db, preset).await?,
            capabilities: preset.capabilities().to_vec(),
        });
    }
    Ok(out)
}

fn display_name_key(preset: BotPresetId) -> String {
    format!("preset_display_name:{}", preset.as_str())
}

/// `system_settings` の 1 値を読む（未登録は `None`・Node `getSystemSetting`）。
pub(crate) async fn get_system_setting(db: &Db, key: &str) -> Result<Option<String>, DbError> {
    let key = key.to_owned();
    db.read
        .read(move |conn| {
            conn.query_row(
                "SELECT value FROM system_settings WHERE key = ?1",
                params![key],
                |r| r.get::<_, String>(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(map_sqlite(other)),
            })
        })
        .await
}

/// `system_settings` を upsert する（Node `setSystemSetting`＝`INSERT OR REPLACE`）。
pub(crate) async fn set_system_setting(db: &Db, key: &str, value: &str) -> Result<(), DbError> {
    let (key, value) = (key.to_owned(), value.to_owned());
    db.writer
        .transaction(move |tx| {
            tx.execute(
                "INSERT OR REPLACE INTO system_settings (key, value, updated_at) \
                 VALUES (?1, ?2, datetime('now','localtime'))",
                params![key, value],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use yuuka_core::CapabilitySet;

    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn seed_db() -> Db {
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yuuka_preset_test_{}_{seq}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let conn = rusqlite::Connection::open(&path).expect("create empty");
            drop(conn);
        }
        let db = Db::open(&path).expect("open");
        {
            let conn = rusqlite::Connection::open(&path).expect("seed conn");
            conn.execute(
                "INSERT OR IGNORE INTO users (discord_id, username, password_hash, salt) \
                 VALUES ('owner', 'owner', 'x', 'x')",
                [],
            )
            .expect("seed user");
            conn.execute(
                "INSERT INTO bots (id, user_id, name) VALUES ('b1', 'owner', 'TestBot')",
                [],
            )
            .expect("seed bot");
        }
        db
    }

    #[test]
    fn id_roundtrip_and_capabilities() {
        assert_eq!(
            BotPresetId::from_id("secretary"),
            Some(BotPresetId::Secretary)
        );
        assert_eq!(
            BotPresetId::from_id("mcp_assistant"),
            Some(BotPresetId::McpAssistant)
        );
        assert_eq!(BotPresetId::from_id("bogus"), None);
        assert_eq!(BotPresetId::Secretary.as_str(), "secretary");
        assert_eq!(
            BotPresetId::McpAssistant.capabilities(),
            &["persona", "memory", "mcp"]
        );
        assert_eq!(
            BotPresetId::Secretary.capabilities_json(),
            r#"["persona","memory","mcp","secretary"]"#
        );
    }

    #[test]
    fn preset_reverse_lookup() {
        let secretary = CapabilitySet::from_granted(
            ["persona", "memory", "mcp", "secretary"]
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
        );
        let general = CapabilitySet::from_granted(
            ["persona", "memory", "mcp"]
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
        );
        assert_eq!(
            preset_id_for_capabilities(&secretary),
            BotPresetId::Secretary
        );
        assert_eq!(
            preset_id_for_capabilities(&general),
            BotPresetId::McpAssistant
        );
    }

    #[tokio::test]
    async fn apply_preset_updates_capabilities() {
        let db = seed_db();
        // 既定 bots.capabilities は秘書相当。汎用へ切替。
        assert!(apply_bot_preset(&db, "b1", BotPresetId::McpAssistant)
            .await
            .unwrap());
        let bot = crate::bot_repo::get_bot(&db, "b1").await.unwrap().unwrap();
        assert_eq!(
            preset_id_for_capabilities(&bot.capability_set()),
            BotPresetId::McpAssistant
        );
        // 存在しない Bot は false。
        assert!(!apply_bot_preset(&db, "nope", BotPresetId::Secretary)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn display_name_default_then_override() {
        let db = seed_db();
        // 既定。
        assert_eq!(
            preset_display_name(&db, BotPresetId::Secretary)
                .await
                .unwrap(),
            "パーソナル秘書"
        );
        // 上書き。
        set_preset_display_name(&db, BotPresetId::Secretary, "  マイ秘書  ")
            .await
            .unwrap();
        assert_eq!(
            preset_display_name(&db, BotPresetId::Secretary)
                .await
                .unwrap(),
            "マイ秘書"
        );
        // 空文字は既定へ戻す。
        set_preset_display_name(&db, BotPresetId::Secretary, "   ")
            .await
            .unwrap();
        assert_eq!(
            preset_display_name(&db, BotPresetId::Secretary)
                .await
                .unwrap(),
            "パーソナル秘書"
        );

        let presets = list_presets(&db).await.unwrap();
        assert_eq!(presets.len(), 2);
        assert_eq!(presets[0].id, "secretary");
        assert_eq!(presets[1].id, "mcp_assistant");
    }
}
