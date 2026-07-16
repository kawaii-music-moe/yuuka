//! 型付き Config と厳密 load/validate（§6.8）。
//!
//! `config.yaml`（存在すれば）＋環境変数を **既存 Node `getSetting` と同一の優先順**
//! （`yaml[KEY] ?? env[KEY] ?? default`・キーは **UPPERCASE**）で読み、型付き構造体へ
//! 落とす。認証発行（P1-1）に必要な `INVITE_CODES`/`ADMIN_DISCORD_IDS` は取り込む
//! （起動時の invite シード・初期 admin 昇格に使う）。それ以外の未知キー（`GOOGLE_*`/
//! `REMINDER_CRON` 等・bot/他系用）は **無視**する（web 起動に不要なため）。型不一致・不正値は起動時に fail-fast
//! （`ConfigError`。§5.6 の唯一の致命ポイント）。機密は本 struct に平文で持たず
//! [`crate::secrets`] の `SecretString` で扱う。

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde_yaml::{Mapping, Value};

use crate::error::ConfigError;
use crate::secrets::SecretString;

/// 起動時に検証済みの型付き設定。以後サービスループへ不変で渡す。
#[derive(Debug, Clone)]
pub struct Config {
    /// 公開ベース URL（`https://` なら Cookie ハードニング + HSTS を強制）。
    pub base_url: Option<String>,
    /// バインドホスト（`HOST`・既定 `127.0.0.1`）。
    pub host: IpAddr,
    /// バインドポート（`PORT`・既定 `3000`）。
    pub port: u16,
    /// SQLite DB ファイルパス（`DB_PATH`・既定 `./data/yuuka.db`）。
    pub db_path: PathBuf,
    /// セッション TTL（日・`SESSION_TTL_DAYS`・既定 7）。
    pub session_ttl_days: u32,
    /// Redis 接続 URL（`REDIS_URL`・既定 `redis://127.0.0.1:6379`）。
    pub redis_url: String,
    /// XFF 信頼判定に使う信頼プロキシ（`TRUSTED_PROXIES`・カンマ/YAML 配列）。
    pub trusted_proxies: Vec<IpAddr>,
    /// index.html へ差し込む google-site-verification 値（`GOOGLE_SITE_VERIFICATION`）。
    pub google_site_verification: Option<String>,
    /// プライバシーポリシー URL（`PRIVACY_POLICY_URL`・`/api/me` で返す）。
    pub privacy_policy_url: String,
    /// 利用規約 URL（`TERMS_URL`・同上）。
    pub terms_url: String,
    /// 保存時暗号化のマスタ秘密（`YUUKA_ENCRYPTION_SECRET`）。API キー・Discord トークン・
    /// OAuth トークン・資格情報等の at-rest 暗号鍵の導出材料（Node `config.secretKey`）。
    /// 未設定なら `None`＝暗号層は fail-closed（[`crate::secrets`] で redact 保持）。
    pub encryption_secret: Option<SecretString>,
    /// 鍵ローテーション用の新秘密（`YUUKA_ENCRYPTION_SECRET_NEW`）。設定時は起動時に
    /// 旧鍵→新鍵で全暗号化列を再暗号化する（Node `config.secretKeyNew` / `rotateSecretKey`）。
    pub encryption_secret_new: Option<SecretString>,
    /// 起動時に DB へ投入する招待コード一覧（`INVITE_CODES`・カンマ/YAML 配列・Node `config.inviteCodes`）。
    pub invite_codes: Vec<String>,
    /// 初期 admin に昇格する Discord ユーザー ID（`ADMIN_DISCORD_IDS`・任意・Node `config.adminDiscordIds`）。
    /// `createUser` はこのリストに含まれる ID（または最初のユーザー）を admin ロールで作成する。
    pub admin_discord_ids: Vec<String>,
    /// デスクトップ `/ws/chat` の 1 メッセージ添付上限（MB・`DESKTOP_MAX_UPLOAD_MB`・既定 20・
    /// Node `config.desktopMaxUploadMb`）。
    pub desktop_max_upload_mb: u32,
    /// リマインダーのポーリング cron 式（`REMINDER_CRON`・既定 `* * * * *`・Node `config.reminderCron`）。
    /// `/api/status` の設定表示に返す。
    pub reminder_cron: String,
    /// Google OAuth2 クライアント ID（`GOOGLE_CLIENT_ID`・未設定は Google 連携無効・Node `config.googleClientId`）。
    pub google_client_id: Option<String>,
    /// Google OAuth2 クライアントシークレット（`GOOGLE_CLIENT_SECRET`・同上・Node `config.googleClientSecret`）。
    pub google_client_secret: Option<String>,
}

impl Config {
    /// `config.yaml`（あれば）＋環境変数を読み、厳密検証して返す。**起動時のみ**呼ぶ。
    ///
    /// `path` が無ければ Node 同様に警告扱いで env/既定へフォールバックする（欠落は致命に
    /// しない。壊れた YAML＝パース失敗のみ致命）。
    ///
    /// # Errors
    /// YAML パース失敗・型不一致・不正値で [`ConfigError`]。
    pub fn load_and_validate(path: &Path) -> Result<Self, ConfigError> {
        // config.yaml があれば mapping として読む（無ければ空＝env/既定のみ）。壊れた
        // YAML はパース失敗＝致命（§5.6）。未知キーは Mapping なので自然に許容される。
        let yaml: Mapping = match std::fs::read_to_string(path) {
            Ok(raw) => {
                serde_yaml::from_str(&raw).map_err(|source| ConfigError::Parse { source })?
            }
            Err(_) => Mapping::new(),
        };

        // getSetting(KEY) = yaml[KEY] ?? env[KEY]（Node `config.ts` と同一優先順）。
        let get = |key: &str| get_setting(&yaml, key);

        let host = parse_field(
            "HOST",
            &get("HOST").unwrap_or_else(|| "127.0.0.1".to_owned()),
        )?;
        let port = parse_field("PORT", &get("PORT").unwrap_or_else(|| "3000".to_owned()))?;
        let db_path = PathBuf::from(get("DB_PATH").unwrap_or_else(|| "./data/yuuka.db".to_owned()));
        let session_ttl_days = parse_field(
            "SESSION_TTL_DAYS",
            &get("SESSION_TTL_DAYS").unwrap_or_else(|| "7".to_owned()),
        )?;
        let redis_url = get("REDIS_URL").unwrap_or_else(|| "redis://127.0.0.1:6379".to_owned());
        let trusted_proxies = parse_ip_list("TRUSTED_PROXIES", get("TRUSTED_PROXIES").as_deref())?;

        let cfg = Self {
            base_url: non_empty(get("BASE_URL")),
            host,
            port,
            db_path,
            session_ttl_days,
            redis_url,
            trusted_proxies,
            google_site_verification: non_empty(get("GOOGLE_SITE_VERIFICATION")),
            privacy_policy_url: get("PRIVACY_POLICY_URL").unwrap_or_default(),
            terms_url: get("TERMS_URL").unwrap_or_default(),
            encryption_secret: non_empty(get("YUUKA_ENCRYPTION_SECRET")).map(SecretString::from),
            encryption_secret_new: non_empty(get("YUUKA_ENCRYPTION_SECRET_NEW"))
                .map(SecretString::from),
            invite_codes: parse_string_list(get("INVITE_CODES").as_deref()),
            admin_discord_ids: parse_string_list(get("ADMIN_DISCORD_IDS").as_deref()),
            desktop_max_upload_mb: parse_field(
                "DESKTOP_MAX_UPLOAD_MB",
                &get("DESKTOP_MAX_UPLOAD_MB").unwrap_or_else(|| "20".to_owned()),
            )?,
            reminder_cron: get("REMINDER_CRON").unwrap_or_else(|| "* * * * *".to_owned()),
            google_client_id: non_empty(get("GOOGLE_CLIENT_ID")),
            google_client_secret: non_empty(get("GOOGLE_CLIENT_SECRET")),
        };
        cfg.validate()?;
        Ok(cfg)
    }

    /// パース済み設定の値レベル検証。
    ///
    /// # Errors
    /// ポート 0 や TTL 0 等の不正値で [`ConfigError::InvalidValue`]。
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.port == 0 {
            return Err(ConfigError::InvalidValue {
                field: "port",
                reason: "port must be non-zero".to_owned(),
            });
        }
        if self.session_ttl_days == 0 {
            return Err(ConfigError::InvalidValue {
                field: "session_ttl_days",
                reason: "session TTL must be at least 1 day".to_owned(),
            });
        }
        Ok(())
    }

    /// HTTPS 本番デプロイか（`base_url` が `https://` で始まるか・§6.8 の型表現）。
    #[must_use]
    pub fn is_https_deployment(&self) -> bool {
        self.base_url
            .as_deref()
            .is_some_and(|u| u.to_ascii_lowercase().starts_with("https://"))
    }
}

/// `yaml[key] ?? env[key]`（Node `getSetting` と同一・yaml 優先）。空文字も値として扱う。
fn get_setting(yaml: &Mapping, key: &str) -> Option<String> {
    yaml_string(yaml, key).or_else(|| std::env::var(key).ok())
}

/// YAML マッピングからキーの値を文字列化して取り出す（配列は Node 同様カンマ結合）。
fn yaml_string(yaml: &Mapping, key: &str) -> Option<String> {
    match yaml.get(Value::from(key))? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        // getSettingArray/getSetting は配列をカンマ結合する（config.ts）。
        Value::Sequence(seq) => Some(
            seq.iter()
                .filter_map(|v| match v {
                    Value::String(s) => Some(s.clone()),
                    Value::Number(n) => Some(n.to_string()),
                    Value::Bool(b) => Some(b.to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(","),
        ),
        _ => None,
    }
}

/// 空文字を `None` に畳む（Node は "" を返すが、Rust は「未設定」を Option で表現）。
fn non_empty(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.is_empty())
}

/// 単一値の型付きパース（失敗は [`ConfigError::InvalidValue`]）。
fn parse_field<T: std::str::FromStr>(field: &'static str, raw: &str) -> Result<T, ConfigError> {
    raw.trim()
        .parse::<T>()
        .map_err(|_| ConfigError::InvalidValue {
            field,
            reason: format!("could not parse value {raw:?}"),
        })
}

/// カンマ区切りの文字列リストをパースする（`getSettingArray` 相当：split(',')→trim→空除去）。
///
/// YAML 配列は `yaml_string` が事前にカンマ結合するため、env のカンマ区切りと同一経路で処理できる。
fn parse_string_list(raw: Option<&str>) -> Vec<String> {
    raw.map(|r| {
        r.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    })
    .unwrap_or_default()
}

/// カンマ区切りの IP リストをパースする（空・未設定は空 Vec・`getSettingArray` 相当）。
fn parse_ip_list(field: &'static str, raw: Option<&str>) -> Result<Vec<IpAddr>, ConfigError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<IpAddr>().map_err(|_| ConfigError::InvalidValue {
                field,
                reason: format!("invalid IP {s:?}"),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::Config;
    use std::io::Write;

    fn write_yaml(body: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().expect("tmp");
        f.write_all(body.as_bytes()).expect("write");
        f
    }

    #[test]
    fn loads_real_uppercase_yaml_and_ignores_unknown_keys() {
        // 実 config.yaml 形式（UPPERCASE・bot 用の未知キー混在）を読めること。
        let f = write_yaml(
            "DB_PATH: \"./data/yuuka.db\"\n\
             REDIS_URL: \"redis://127.0.0.1:6379\"\n\
             PORT: 7854\n\
             HOST: \"0.0.0.0\"\n\
             SESSION_TTL_DAYS: 14\n\
             TERMS_URL: \"https://ex.test/terms\"\n\
             REMINDER_CRON: \"* * * * *\"\n\
             INVITE_CODES:\n  - \"a\"\n  - \"b\"\n\
             ADMIN_DISCORD_IDS: \"111, 222\"\n\
             GOOGLE_CLIENT_ID: \"xxx\"\n",
        );
        let cfg = Config::load_and_validate(f.path()).expect("load");
        assert_eq!(cfg.port, 7854);
        assert_eq!(cfg.host.to_string(), "0.0.0.0");
        assert_eq!(cfg.session_ttl_days, 14);
        assert_eq!(cfg.db_path.to_str().unwrap(), "./data/yuuka.db");
        assert_eq!(cfg.redis_url, "redis://127.0.0.1:6379");
        assert_eq!(cfg.terms_url, "https://ex.test/terms");
        assert!(!cfg.is_https_deployment());
        // P1-1: 招待コード（YAML 配列→カンマ結合→分割）と admin ID（カンマ区切り・trim）を取り込む。
        assert_eq!(cfg.invite_codes, vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(
            cfg.admin_discord_ids,
            vec!["111".to_owned(), "222".to_owned()]
        );
    }

    #[test]
    fn missing_file_falls_back_to_defaults() {
        // 欠落は致命にしない（env/既定へ）。パス不在でも既定で組み上がる。
        let cfg = Config::load_and_validate(std::path::Path::new("/no/such/yuuka-config.yaml"))
            .expect("defaults");
        assert_eq!(cfg.port, 3000);
        assert_eq!(cfg.host.to_string(), "127.0.0.1");
        assert_eq!(cfg.session_ttl_days, 7);
        assert_eq!(cfg.redis_url, "redis://127.0.0.1:6379");
    }

    #[test]
    fn invalid_port_is_rejected() {
        let f = write_yaml("PORT: \"not-a-number\"\n");
        assert!(Config::load_and_validate(f.path()).is_err());
    }

    #[test]
    fn https_base_url_flags_https_deployment() {
        let f = write_yaml("BASE_URL: \"https://yuuka.example\"\n");
        let cfg = Config::load_and_validate(f.path()).expect("load");
        assert!(cfg.is_https_deployment());
    }
}
