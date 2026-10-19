//! 型付き Config と厳密 load/validate（§6.8）。
//!
//! `config.yaml`（存在すれば）＋環境変数を **既存 Node `getSetting` と同一の優先順**
//! （`yaml[KEY] ?? env[KEY] ?? default`・キーは **UPPERCASE**）で読み、型付き構造体へ
//! 落とす。認証発行（P1-1）に必要な `ADMIN_DISCORD_IDS` は取り込む（初期 admin 昇格に使う）。
//! それ以外の未知キー（`GOOGLE_*`/
//! `REMINDER_CRON` 等・bot/他系用）は **無視**する（web 起動に不要なため）。型不一致・不正値は起動時に fail-fast
//! （`ConfigError`。§5.6 の唯一の致命ポイント）。機密は本 struct に平文で持たず
//! [`crate::secrets`] の `SecretString` で扱う。

use std::net::IpAddr;
use std::path::{Path, PathBuf};

mod yaml;

pub use yaml::YamlError;
use yaml::{Mapping, Value};

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
    /// sk-account のブラウザ向け公開URL。
    pub sk_account_url: String,
    /// Yuuka サーバーから認可コードを交換する内部URL。
    pub sk_account_internal_url: String,
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
            Ok(raw) => yaml::from_str(&raw).map_err(|source| ConfigError::Parse { source })?,
            Err(_) => Mapping::default(),
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
            sk_account_url: get("SK_ACCOUNT_URL")
                .unwrap_or_else(|| "https://account.sk-9.net".to_owned()),
            sk_account_internal_url: get("SK_ACCOUNT_INTERNAL_URL")
                .unwrap_or_else(|| "http://127.0.0.1:3100".to_owned()),
            trusted_proxies,
            google_site_verification: non_empty(get("GOOGLE_SITE_VERIFICATION")),
            privacy_policy_url: get("PRIVACY_POLICY_URL").unwrap_or_default(),
            terms_url: get("TERMS_URL").unwrap_or_default(),
            encryption_secret: non_empty(get("YUUKA_ENCRYPTION_SECRET")).map(SecretString::from),
            encryption_secret_new: non_empty(get("YUUKA_ENCRYPTION_SECRET_NEW"))
                .map(SecretString::from),
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
    match yaml.get(key)? {
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
        // P1-1: admin ID（カンマ区切り・trim）を取り込む。廃止した INVITE_CODES は無視する。
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

    // ---- YAML 解釈の回帰テスト（serde_yaml 0.9 → granit-parser 移行で読み方を変えない） ----
    //
    // 期待値はすべて旧実装（serde_yaml 0.9 の `Mapping` + `yaml_string`）で実測した値。

    /// `src` を読み、`key` を `yaml_string` で文字列化する（パース失敗は `Err(())`）。
    fn setting(src: &str, key: &str) -> Result<Option<String>, ()> {
        let yaml = super::yaml::from_str(src).map_err(|_| ())?;
        Ok(super::yaml_string(&yaml, key))
    }

    fn s(v: &str) -> Result<Option<String>, ()> {
        Ok(Some(v.to_owned()))
    }

    #[test]
    fn repo_example_configs_load_as_before() {
        // リポジトリ同梱のテンプレートが従来どおりの値で読めること。
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let cfg = Config::load_and_validate(&root.join("example.yaml")).expect("example.yaml");
        assert_eq!(cfg.port, 7854);
        assert_eq!(cfg.host.to_string(), "127.0.0.1");
        assert_eq!(cfg.session_ttl_days, 7);
        assert_eq!(cfg.db_path.to_str(), Some("./data/yuuka.db"));
        assert_eq!(cfg.reminder_cron, "* * * * *");
        assert_eq!(
            cfg.invite_codes,
            vec![
                "change-me-to-random-code-1".to_owned(),
                "change-me-to-random-code-2".to_owned()
            ]
        );
        assert_eq!(cfg.base_url.as_deref(), Some("https://example.com"));
        assert_eq!(cfg.terms_url, "/terms");
        assert_eq!(cfg.google_client_id, None, "空文字は未設定に畳む");

        for instance in ["dev", "prod"] {
            let path = root.join(format!("deploy/{instance}/config.yaml.example"));
            let cfg = Config::load_and_validate(&path).expect("deploy example");
            assert_eq!(cfg.port, 7854);
            assert_eq!(cfg.invite_codes.len(), 2);
        }
    }

    #[test]
    fn yaml_numbers_and_strings_resolve_like_serde_yaml_09() {
        assert_eq!(setting("K: 7854\n", "K"), s("7854"));
        assert_eq!(setting("K: \"7854\"\n", "K"), s("7854"));
        assert_eq!(setting("K: -12\n", "K"), s("-12"));
        assert_eq!(setting("K: +12\n", "K"), s("12"));
        // 先頭 0 の数字列は YAML 1.2 で文字列（招待コード等の 0 落ちを防ぐ）。
        assert_eq!(setting("K: 0123\n", "K"), s("0123"));
        assert_eq!(setting("K: 00\n", "K"), s("00"));
        assert_eq!(setting("K: [0123, 7]\n", "K"), s("0123,7"));
        // 0x/0o/0b は小文字のみ整数（大文字や `_` 区切りは文字列のまま）。
        assert_eq!(setting("K: 0x1F\n", "K"), s("31"));
        assert_eq!(setting("K: 0o17\n", "K"), s("15"));
        assert_eq!(setting("K: 0b101\n", "K"), s("5"));
        assert_eq!(setting("K: 0X1F\n", "K"), s("0X1F"));
        assert_eq!(setting("K: 1_000\n", "K"), s("1_000"));
        // 浮動小数は ryu 表記（`7.0` は `7.0` のまま → u32 への変換は従来どおり失敗する）。
        assert_eq!(setting("K: 7.0\n", "K"), s("7.0"));
        assert_eq!(setting("K: 1e3\n", "K"), s("1000.0"));
        assert_eq!(setting("K: 1.5e16\n", "K"), s("1.5e16"));
        assert_eq!(setting("K: .inf\n", "K"), s(".inf"));
        assert_eq!(setting("K: -.inf\n", "K"), s("-.inf"));
        assert_eq!(setting("K: .NaN\n", "K"), s(".nan"));
        assert_eq!(setting("K: 1e400\n", "K"), s("1e400"), "f64 溢れは文字列");
        // Discord ID 規模（u64 内）は整数のまま桁落ちしない。
        assert_eq!(
            setting("K: 18446744073709551615\n", "K"),
            s("18446744073709551615")
        );
        // u64/i64 を超え u128/i128 に収まる整数は旧実装同様パース失敗。
        assert_eq!(setting("K: 18446744073709551616\n", "K"), Err(()));
        assert_eq!(setting("K: -9223372036854775809\n", "K"), Err(()));
        // bool は true/True/TRUE 系のみ。YAML 1.1 の yes/on 等は文字列。
        assert_eq!(setting("K: True\n", "K"), s("true"));
        assert_eq!(setting("K: FALSE\n", "K"), s("false"));
        assert_eq!(setting("K: tRuE\n", "K"), s("tRuE"));
        assert_eq!(setting("K: yes\n", "K"), s("yes"));
        assert_eq!(setting("K: on\n", "K"), s("on"));
        // IP・cron・URL・時刻風の値は文字列。
        assert_eq!(setting("K: 127.0.0.1\n", "K"), s("127.0.0.1"));
        assert_eq!(setting("K: 12:30:00\n", "K"), s("12:30:00"));
        assert_eq!(setting("K: 2026-10-19\n", "K"), s("2026-10-19"));
        assert_eq!(
            setting("K: redis://127.0.0.1:6379\n", "K"),
            s("redis://127.0.0.1:6379")
        );
    }

    #[test]
    fn yaml_null_and_empty_values() {
        // 空・`~`・null 系は値なし（→ env/既定へフォールバック）。引用符付きは文字列。
        assert_eq!(setting("K:\n", "K"), Ok(None));
        assert_eq!(setting("K: ~\n", "K"), Ok(None));
        assert_eq!(setting("K: null\n", "K"), Ok(None));
        assert_eq!(setting("K: NULL\n", "K"), Ok(None));
        assert_eq!(setting("K: nUll\n", "K"), s("nUll"));
        assert_eq!(setting("K: \"\"\n", "K"), s(""));
        assert_eq!(setting("K: ''\n", "K"), s(""));
        assert_eq!(setting("K: \"null\"\n", "K"), s("null"));
        // 配列内の null・入れ子は落とし、スカラーだけカンマ結合する。
        assert_eq!(
            setting("K: [1, \"2\", true, ~, [3], {a: 1}, yes]\n", "K"),
            s("1,2,true,yes")
        );
        assert_eq!(setting("K:\n  - a\n  -\n  - ~\n", "K"), s("a"));
        assert_eq!(setting("K: []\n", "K"), s(""));
        // mapping 値は文字列化しない。
        assert_eq!(setting("K: {a: 1}\n", "K"), Ok(None));
        // ファイル全体が空・コメントのみ・`---` のみなら空 mapping。
        assert_eq!(setting("", "K"), Ok(None));
        assert_eq!(setting("# only comment\n", "K"), Ok(None));
        assert_eq!(setting("---\n", "K"), Ok(None));
        // ルートが mapping 以外（null・配列・スカラー・引用空文字）はパース失敗。
        assert_eq!(setting("~\n", "K"), Err(()));
        assert_eq!(setting("- a\n", "K"), Err(()));
        assert_eq!(setting("a\n", "K"), Err(()));
        assert_eq!(setting("\"\"\n", "K"), Err(()));
    }

    #[test]
    fn yaml_multiline_strings() {
        assert_eq!(
            setting("K: |\n  line1\n  line2\n", "K"),
            s("line1\nline2\n")
        );
        assert_eq!(setting("K: |-\n  line1\n  line2\n", "K"), s("line1\nline2"));
        assert_eq!(setting("K: |+\n  a\n\n", "K"), s("a\n\n"));
        assert_eq!(
            setting("K: >\n  folded\n  text\n\n  para\n", "K"),
            s("folded text\npara\n")
        );
        assert_eq!(setting("K: >-\n  a\n  b\n", "K"), s("a b"));
        assert_eq!(setting("K: |2\n   indented\n", "K"), s(" indented\n"));
        // ファイル末尾の空ブロックスカラーは空文字。
        assert_eq!(setting("K: |\n", "K"), s(""));
        assert_eq!(setting("K: >\n", "K"), s(""));
        assert_eq!(
            setting("K:\n  multi\n  line plain\n", "K"),
            s("multi line plain")
        );
        assert_eq!(setting("K: a\n\n  b\n", "K"), s("a\nb"));
        assert_eq!(
            setting("K: \"double\n  quoted\n\n  lines\"\n", "K"),
            s("double quoted\nlines")
        );
        assert_eq!(setting("K: \"a\\\n  b\"\n", "K"), s("ab"));
        assert_eq!(setting("K: \"a\\tb\\u00e9\"\n", "K"), s("a\tbé"));
        assert_eq!(setting("K: 'it''s'\n", "K"), s("it's"));
        assert_eq!(setting("K: 1\r\nL: |\r\n  a\r\n  b\r\n", "L"), s("a\nb\n"));
    }

    #[test]
    fn yaml_anchors_aliases_and_merge_keys() {
        assert_eq!(setting("A: &x hello\nK: *x\n", "K"), s("hello"));
        assert_eq!(setting("A: &x 0123\nK: *x\n", "K"), s("0123"));
        assert_eq!(setting("A: &x [1, 2]\nK: *x\n", "K"), s("1,2"));
        assert_eq!(setting("A: &x\n  - p\nK: [*x, q]\n", "K"), s("q"));
        // 再定義したアンカーは後勝ち。キーへのアンカーも値として参照できる。
        assert_eq!(setting("A: &a 1\nB: &a 2\nK: *a\n", "K"), s("2"));
        assert_eq!(setting("&k K: 1\nL: *k\n", "L"), s("K"));
        // 未定義エイリアス・自己参照はパース失敗。
        assert_eq!(setting("K: *undefined\n", "K"), Err(()));
        assert_eq!(setting("K: &a [*a]\n", "K"), Err(()));
        // マージキー `<<` は展開しない（通常キー扱い）。
        assert_eq!(setting("base: &b\n  PORT: 1\n<<: *b\n", "PORT"), Ok(None));
        assert_eq!(setting("<<: {PORT: 1}\nPORT: 2\n", "PORT"), s("2"));
        // エイリアスの多重展開でも上限内なら従来どおり読める。
        let laughs = "a: &a [x, x, x, x, x, x, x, x, x, x]\n\
                      b: &b [*a, *a, *a, *a, *a, *a, *a, *a, *a, *a]\n\
                      c: &c [*b, *b, *b, *b, *b, *b, *b, *b, *b, *b]\n\
                      K: [*c, *c]\n";
        assert_eq!(setting(laughs, "K"), s(""));
    }

    #[test]
    fn yaml_unknown_keys_tags_and_structure() {
        // 未知キー・非文字列キー・キーの大小文字違いは無視（UPPERCASE の完全一致のみ）。
        let src = "UNKNOWN: {nested: [1, 2]}\n1: one\ntrue: t\nk: lower\n\"K\": quoted\n";
        assert_eq!(setting(src, "K"), s("quoted"));
        assert_eq!(setting(src, "UNKNOWN"), Ok(None));
        assert_eq!(setting("? K\n", "K"), Ok(None));
        // 重複キーは（型込みで同値なら）パース失敗。
        assert_eq!(setting("K: 1\nK: 2\n", "K"), Err(()));
        assert_eq!(setting("K: 1\n\"K\": 2\n", "K"), Err(()));
        assert_eq!(setting("0x10: a\n16: b\n", "K"), Err(()));
        assert_eq!(setting("1.0: a\n1: b\nK: c\n", "K"), s("c"));
        assert_eq!(setting("K: {a: 1, a: 2}\n", "K"), Err(()));
        // 複数ドキュメントはパース失敗（末尾の `...` やコメントは可）。
        assert_eq!(setting("K: 1\n---\nL: 2\n", "K"), Err(()));
        assert_eq!(setting("K: 1\n...\n# trailing\n", "K"), s("1"));
        // コア型タグは厳密に解釈、`!!str` 等は文字列、ローカルタグ付きは値として読まない。
        assert_eq!(setting("K: !!str 123\n", "K"), s("123"));
        assert_eq!(setting("K: !!binary aGVsbG8=\n", "K"), s("aGVsbG8="));
        assert_eq!(setting("K: !!int \"123\"\n", "K"), s("123"));
        assert_eq!(setting("K: !!float 1\n", "K"), s("1.0"));
        assert_eq!(setting("K: !!int 0123\n", "K"), Err(()));
        assert_eq!(setting("K: !!null ''\n", "K"), Err(()));
        assert_eq!(setting("K: !custom foo\n", "K"), Ok(None));
        assert_eq!(setting("K: [a, !x 6, !!str 5]\n", "K"), s("a,5"));
        assert_eq!(setting("!foo {K: 1}\n", "K"), s("1"), "ルートのタグは無視");
        // BOM 付きファイルも読める。入れ子は 128 段まで。
        assert_eq!(setting("\u{feff}K: 1\n", "K"), s("1"));
        let nested = |n: usize| format!("K: {}1{}\n", "[".repeat(n), "]".repeat(n));
        assert_eq!(setting(&nested(127), "K"), s(""));
        assert_eq!(setting(&nested(128), "K"), Err(()));
        // 構文エラーはパース失敗。
        assert_eq!(setting("K: [1, 2\n", "K"), Err(()));
        assert_eq!(setting("K: a: b\n", "K"), Err(()));
    }

    #[test]
    fn yaml_parse_error_is_config_parse_and_hides_values() {
        let f = write_yaml("SECRET_VALUE: [unclosed\n");
        let err = Config::load_and_validate(f.path()).expect_err("parse error");
        assert!(matches!(err, crate::error::ConfigError::Parse { .. }));
        let f = write_yaml("K: 18446744073709551616\n");
        let err = Config::load_and_validate(f.path()).expect_err("parse error");
        assert!(matches!(
            &err,
            crate::error::ConfigError::Parse { source }
                if !source.to_string().contains("18446744073709551616")
        ));
    }
}
