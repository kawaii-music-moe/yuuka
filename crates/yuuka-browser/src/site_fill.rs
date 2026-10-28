//! 認証情報の自動入力（`browserFillCredential`・#205）の照合と入力 JS。
//!
//! 現在ページが登録サイトと同じオリジンのときだけ入力する。照合と代入はページ内の 1 回の JS 実行に
//! まとめる（Rust 側で照合してから入力するまでの間に、ページ自身の JS 遷移・meta refresh・
//! フォーカス移動が入っても、別オリジンへは入力しない）。入力先は種類で絞り（パスワードは
//! `input[type=password]` だけ）、属性の部分一致によるフォールバックは使わない（同じホスト上の
//! ユーザー投稿コンテンツから、コメント欄などへパスワードを入れさせる誘導を防ぐ）。

use async_trait::async_trait;
use reqwest::Url;
use serde_json::json;

/// 入力する欄の種類（入力先の `input` の type を絞るのに使う）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialField {
    /// `input` の type が text / email / tel のものだけ。
    Username,
    /// `input[type=password]` だけ。
    Password,
}

impl CredentialField {
    fn as_js(self) -> &'static str {
        match self {
            Self::Username => "username",
            Self::Password => "password",
        }
    }
}

/// 入力する 1 欄（種類・セレクタ・値）。
#[derive(Debug, Clone, Copy)]
pub struct FillField<'a> {
    pub kind: CredentialField,
    pub selector: &'a str,
    pub value: &'a str,
}

/// [`SiteCredentialFiller::fill_fields_for_site`] の失敗。どの場合も何も入力していない
/// （全欄の確認が済んでから、まとめて代入するため）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FillError {
    /// ブラウザのセッションを用意できない。
    Launch(String),
    /// いま開いているページが登録サイトと一致しない。
    SiteMismatch { current: String, registered: String },
    /// 照合の後、入力する前にページが別オリジンへ切り替わった。
    PageChanged,
    /// `index` 番目の欄が見つからない（セレクタが不正・要素が無い・`input` でない）。
    FieldNotFound { index: usize },
    /// `index` 番目の欄の種類が合わない（パスワードを password 以外の欄へ入れようとした等）。
    FieldTypeRejected { index: usize },
    /// ページでの JS 実行に失敗した。
    Script,
}

/// 登録サイトと現在ページを照合してから、認証情報を入力する（`BrowserManager` が実装する。
/// テストでは差し替える）。
#[async_trait]
pub trait SiteCredentialFiller: Send + Sync {
    /// `user` の対話ブラウザで、現在ページが `registered_url` と同じサイトのときだけ `fields` を入力する。
    ///
    /// # Errors
    /// [`FillError`] のとおり。失敗したときは何も入力していない。
    async fn fill_fields_for_site(
        &self,
        user: &str,
        registered_url: &str,
        fields: &[FillField<'_>],
    ) -> Result<(), FillError>;
}

/// 登録 URL を解釈する。スキームが無ければ `https://` を補う（画面・API とも http(s) を強制して
/// いないため、`github.com` のような登録がありうる）。
fn parse_registered(registered: &str) -> Option<Url> {
    let s = registered.trim();
    if s.is_empty() {
        return None;
    }
    if s.contains("://") {
        Url::parse(s).ok()
    } else {
        Url::parse(&format!("https://{s}")).ok()
    }
}

/// 現在 URL が登録 URL と同じサイトか。ホストとポートが一致し、https 登録なら https のままのこと。
/// http 登録から https への昇格は許し、そのとき既定ポート同士（80 と 443）は同じとみなす。
/// 解釈できない URL・http(s) 以外は常に不一致。
#[must_use]
pub fn url_matches_site(registered: &str, current: &str) -> bool {
    let (Some(reg), Ok(cur)) = (parse_registered(registered), Url::parse(current)) else {
        return false;
    };
    if !matches!(reg.scheme(), "http" | "https") || !matches!(cur.scheme(), "http" | "https") {
        return false;
    }
    if reg.scheme() == "https" && cur.scheme() != "https" {
        return false;
    }
    let same_host = match (reg.host_str(), cur.host_str()) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        _ => false,
    };
    let upgraded_default_ports = reg.scheme() == "http"
        && cur.scheme() == "https"
        && reg.port().is_none()
        && cur.port().is_none();
    let same_port =
        reg.port_or_known_default() == cur.port_or_known_default() || upgraded_default_ports;
    same_host && same_port
}

/// 照合に通った現在 URL のオリジン（JS 側で `location.origin` と比べる値）。
pub(crate) fn origin_of(current: &str) -> Option<String> {
    Url::parse(current)
        .ok()
        .map(|u| u.origin().ascii_serialization())
}

/// エラー文言用のサイト表示（ホストと、既定でないポートだけ。パスやクエリは出さない）。
fn site_label(url: Option<Url>) -> String {
    let Some(u) = url else {
        return "不明".to_owned();
    };
    match (u.host_str(), u.port()) {
        (Some(h), Some(p)) => format!("{h}:{p}"),
        (Some(h), None) => h.to_owned(),
        _ => "不明".to_owned(),
    }
}

/// 照合失敗のエラーを作る（現在ページと登録サイトの両方を表示用に持つ）。
pub(crate) fn site_mismatch(registered: &str, current: &str) -> FillError {
    FillError::SiteMismatch {
        current: site_label(Url::parse(current).ok()),
        registered: site_label(parse_registered(registered)),
    }
}

/// 照合と代入を 1 回で行う JS。オリジンが変わっていれば何もしない。全欄を確かめてから代入する。
///
/// 戻り値は `"ok"` / `"origin"` / `"notfound:<i>"` / `"type:<i>"`（[`parse_fill_result`] で解釈）。
/// `selectors` は解決済みの CSS セレクタ（数値 ID は `[data-yuuka-id=...]` に変換済み）。
pub(crate) fn fill_js(
    expected_origin: &str,
    fields: &[FillField<'_>],
    selectors: &[String],
) -> String {
    let spec: Vec<_> = fields
        .iter()
        .zip(selectors)
        .map(|(f, sel)| json!({ "sel": sel, "kind": f.kind.as_js(), "value": f.value }))
        .collect();
    let origin_js = json!(expected_origin).to_string();
    let spec_js = json!(spec).to_string();
    format!(
        "(() => {{ const expected = {origin_js}; const fields = {spec_js}; \
         if (location.origin !== expected) return 'origin'; \
         const targets = []; \
         for (let i = 0; i < fields.length; i++) {{ \
           let el = null; \
           try {{ el = document.querySelector(fields[i].sel); }} catch (e) {{ return 'notfound:' + i; }} \
           if (!el || !(el instanceof HTMLInputElement)) return 'notfound:' + i; \
           const t = el.type; \
           const ok = fields[i].kind === 'password' ? t === 'password' : (t === 'text' || t === 'email' || t === 'tel'); \
           if (!ok) return 'type:' + i; \
           targets.push(el); }} \
         const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; \
         targets.forEach((el, i) => {{ el.focus(); setter.call(el, fields[i].value); \
           el.dispatchEvent(new Event('input', {{ bubbles: true }})); \
           el.dispatchEvent(new Event('change', {{ bubbles: true }})); }}); \
         return 'ok'; }})()"
    )
}

/// [`fill_js`] の戻り値を解釈する。
pub(crate) fn parse_fill_result(result: &str) -> Result<(), FillError> {
    let index = |rest: &str| rest.parse::<usize>().map_err(|_| FillError::Script);
    match result {
        "ok" => Ok(()),
        "origin" => Err(FillError::PageChanged),
        _ => {
            if let Some(rest) = result.strip_prefix("notfound:") {
                Err(FillError::FieldNotFound {
                    index: index(rest)?,
                })
            } else if let Some(rest) = result.strip_prefix("type:") {
                Err(FillError::FieldTypeRejected {
                    index: index(rest)?,
                })
            } else {
                Err(FillError::Script)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_host_matches_across_paths() {
        assert!(url_matches_site(
            "https://github.com/login",
            "https://github.com/session?x=1"
        ));
        assert!(url_matches_site(
            "https://GitHub.com/",
            "https://github.com/login"
        ));
    }

    #[test]
    fn other_or_lookalike_hosts_are_rejected() {
        for current in [
            "https://evil.example/login",
            "https://github.com.evil.example/",
            "https://evil.example/github.com",
            "https://github.com@evil.example/",
        ] {
            assert!(
                !url_matches_site("https://github.com", current),
                "{current}"
            );
        }
    }

    #[test]
    fn blank_unparsable_and_non_http_are_rejected() {
        assert!(!url_matches_site("https://github.com", ""));
        assert!(!url_matches_site("https://github.com", "about:blank"));
        assert!(!url_matches_site("", "https://github.com"));
        assert!(!url_matches_site("   ", "https://github.com"));
        assert!(!url_matches_site(
            "https://github.com",
            "file:///etc/passwd"
        ));
        assert!(!url_matches_site("ftp://github.com", "https://github.com"));
    }

    #[test]
    fn https_registration_rejects_http_downgrade() {
        assert!(!url_matches_site(
            "https://github.com",
            "http://github.com/login"
        ));
        assert!(url_matches_site(
            "http://intranet.test",
            "https://intranet.test/"
        ));
    }

    /// スキーム無しの登録は https として扱う（以前は常に不一致になっていた）。
    #[test]
    fn scheme_less_registration_is_treated_as_https() {
        assert!(url_matches_site("github.com", "https://github.com/login"));
        assert!(url_matches_site("github.com/login", "https://github.com/"));
        assert!(!url_matches_site("github.com", "http://github.com/login"));
        assert!(url_matches_site(
            "intranet.test:8443",
            "https://intranet.test:8443/"
        ));
    }

    #[test]
    fn ports_must_match() {
        assert!(!url_matches_site(
            "https://intranet.test:8443",
            "https://intranet.test:9000/"
        ));
        assert!(!url_matches_site(
            "https://intranet.test:8443",
            "https://intranet.test/"
        ));
        assert!(url_matches_site(
            "https://intranet.test:443",
            "https://intranet.test/"
        ));
        // http→https の昇格は既定ポート同士なら同じとみなすが、明示ポートは比べる。
        assert!(!url_matches_site(
            "http://intranet.test:8080",
            "https://intranet.test/"
        ));
    }

    #[test]
    fn mismatch_reports_both_sites_without_paths() {
        assert_eq!(
            site_mismatch("github.com", "https://evil.example:8443/a?b=c"),
            FillError::SiteMismatch {
                current: "evil.example:8443".to_owned(),
                registered: "github.com".to_owned(),
            }
        );
        assert_eq!(
            site_mismatch("", "about:blank"),
            FillError::SiteMismatch {
                current: "不明".to_owned(),
                registered: "不明".to_owned(),
            }
        );
    }

    #[test]
    fn origin_of_drops_path() {
        assert_eq!(
            origin_of("https://github.com/login?x=1").as_deref(),
            Some("https://github.com")
        );
        assert_eq!(
            origin_of("https://intranet.test:8443/").as_deref(),
            Some("https://intranet.test:8443")
        );
    }

    #[test]
    fn fill_js_checks_origin_and_types_and_embeds_values_safely() {
        let fields = [
            FillField {
                kind: CredentialField::Username,
                selector: "1",
                value: "me",
            },
            FillField {
                kind: CredentialField::Password,
                selector: "2",
                value: "p\"w'</script>\\",
            },
        ];
        let selectors = ["[data-yuuka-id=\"1\"]".to_owned(), "#pw".to_owned()];
        let js = fill_js("https://github.com", &fields, &selectors);
        assert!(js.contains("location.origin !== expected"));
        assert!(js.contains(r#"const expected = "https://github.com";"#));
        assert!(js.contains("t === 'password'"));
        // 値は JSON 文字列として埋め込まれる（引用符・バックスラッシュはエスケープ）。
        assert!(js.contains(r#""value":"p\"w'</script>\\""#));
        assert!(js.contains(r#""sel":"[data-yuuka-id=\"1\"]""#));
        // 部分一致のフォールバックは使わない。
        assert!(!js.contains("includes("));
    }

    #[test]
    fn parse_fill_result_maps_codes() {
        assert_eq!(parse_fill_result("ok"), Ok(()));
        assert_eq!(parse_fill_result("origin"), Err(FillError::PageChanged));
        assert_eq!(
            parse_fill_result("notfound:1"),
            Err(FillError::FieldNotFound { index: 1 })
        );
        assert_eq!(
            parse_fill_result("type:0"),
            Err(FillError::FieldTypeRejected { index: 0 })
        );
        assert_eq!(parse_fill_result("type:x"), Err(FillError::Script));
        assert_eq!(parse_fill_result(""), Err(FillError::Script));
    }
}
