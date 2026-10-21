//! SSRF 防御（Node `src/utils/ssrfGuard.ts` の `assertSafeOutboundUrl` パリティ）。
//!
//! browser・MCP・briefing が外部 URL を取得する前に、宛先がプライベート/ループバック/リンクローカル/
//! メタデータ等の内部レンジでないことを検証する。スキームは http/https のみ、認証情報（user:pass@）
//! は禁止、ホスト名は DNS 解決して**全アドレス**を検査する。ブロック時は Node と同一の日本語文言を
//! `Err(String)` で返す。
//!
//! 通信時検査は呼び出しのたびに DNS を引く。結果のキャッシュ・allowlist による迂回はしない。
//! HTTP/Chromium の接続・リダイレクト制御は呼び出し元の責務。本 API は接続 IP を固定しないため、
//! 検査と接続の間の DNS rebinding（TOCTOU）を完全には防がない。
//! 登録時の簡易検査は [`RegistrationPolicy`] を必須とし、通信時検査の代替にはしない。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// 外向き URL の安全性を検証する（Node `assertSafeOutboundUrl`）。
///
/// # Errors
/// 形式不正・非 http(s)・認証情報付き・localhost・内部レンジ解決時に Node と同一文言を返す。
pub async fn assert_safe_outbound_url(raw: &str) -> Result<(), String> {
    assert_safe_outbound_url_with_lookup(raw, |host, port| async move {
        tokio::net::lookup_host((host.as_str(), port))
            .await
            .map(Iterator::collect)
    })
    .await
}

async fn assert_safe_outbound_url_with_lookup<F, Fut>(raw: &str, lookup: F) -> Result<(), String>
where
    F: FnOnce(String, u16) -> Fut,
    Fut: std::future::Future<Output = std::io::Result<Vec<SocketAddr>>>,
{
    let url = url::Url::parse(raw).map_err(|_| "URLの形式が不正です。".to_owned())?;

    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(format!(
            "許可されていないスキームです: {scheme}:（http/https のみ）"
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URLに認証情報（user:pass@）を含めることはできません。".to_owned());
    }

    match url.host() {
        Some(url::Host::Ipv4(ip)) => {
            if is_blocked_ipv4(ip) {
                return Err(format!(
                    "内部/予約済みアドレスへの接続は許可されていません: {ip}"
                ));
            }
            Ok(())
        }
        Some(url::Host::Ipv6(ip)) => {
            if is_blocked_ipv6(ip) {
                return Err(format!(
                    "内部/予約済みアドレスへの接続は許可されていません: {ip}"
                ));
            }
            Ok(())
        }
        Some(url::Host::Domain(domain)) => {
            let host = domain.to_ascii_lowercase();
            if host.is_empty() || host == "localhost" || host.ends_with(".localhost") {
                return Err("ローカルホストへの接続は許可されていません。".to_owned());
            }
            let port = url.port_or_known_default().unwrap_or(80);
            let addrs = lookup(host.clone(), port)
                .await
                .map_err(|_| format!("ホスト名を解決できませんでした: {host}"))?;
            let mut resolved = false;
            for addr in addrs {
                resolved = true;
                let ip = addr.ip();
                if is_blocked_ip(ip) {
                    return Err(format!(
                        "内部/予約済みアドレスに解決されるホストへの接続は許可されていません: {host} -> {ip}"
                    ));
                }
            }
            if resolved {
                Ok(())
            } else {
                Err(format!("ホスト名を解決できませんでした: {host}"))
            }
        }
        None => Err("ローカルホストへの接続は許可されていません。".to_owned()),
    }
}

/// IP が内部/予約レンジかを判定する（`true` = ブロック）。接続時の DNS 解決結果検査にも使う。
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_ipv4(v4),
        IpAddr::V6(v6) => is_blocked_ipv6(v6),
    }
}

/// IPv4 の内部/予約レンジ判定。
fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _d] = ip.octets();
    a == 0                                   // 0.0.0.0/8 "this host"
        || a == 10                           // 10.0.0.0/8 private
        || a == 127                          // 127.0.0.0/8 loopback
        || (a == 169 && b == 254)            // 169.254.0.0/16 link-local（メタデータ）
        || (a == 172 && (16..=31).contains(&b)) // 172.16.0.0/12 private
        || (a == 192 && b == 168)            // 192.168.0.0/16 private
        || (a == 192 && b == 0 && c == 0)    // 192.0.0.0/24 IETF
        || (a == 192 && b == 0 && c == 2)    // 192.0.2.0/24 TEST-NET-1
        || (a == 198 && (b == 18 || b == 19)) // 198.18.0.0/15 benchmark
        || (a == 198 && b == 51 && c == 100) // 198.51.100.0/24 TEST-NET-2
        || (a == 203 && b == 0 && c == 113)  // 203.0.113.0/24 TEST-NET-3
        || (a == 100 && (64..=127).contains(&b)) // 100.64.0.0/10 CGNAT
        || a >= 224 // 224.0.0.0/4 multicast + 240.0.0.0/4 reserved + broadcast
}

/// IPv6 の内部/予約レンジ判定（Node `isBlockedIpv6`）。
fn is_blocked_ipv6(ip: Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() {
        return true;
    }
    // IPv4-mapped (::ffff:a.b.c.d) は内側の v4 で判定。
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_blocked_ipv4(v4);
    }
    let [seg0, seg1, ..] = ip.segments();
    (seg0 & 0xffc0) == 0xfe80        // fe80::/10 link-local
        || (seg0 & 0xfe00) == 0xfc00 // fc00::/7 unique-local
        || (seg0 & 0xffc0) == 0xfec0 // fec0::/10 site-local（deprecated）
        || (seg0 & 0xff00) == 0xff00 // ff00::/8 multicast
        || (seg0 == 0x2001 && seg1 == 0x0db8) // 2001:db8::/32 documentation
}

/// 登録時だけに適用するポリシー。通信許可には使わない。
#[derive(Clone, Copy, Debug)]
pub enum RegistrationPolicy {
    /// URL を正規化し、通信時と同じ内部/予約 IP 範囲を拒否する（DNS は引かない）。
    PublicHttp,
}

/// DNS を引かず、http(s) URL とリテラル IP の安全性を検査する。
///
/// ドメイン名の解決先は検査しないため、取得時は必ず
/// [`assert_safe_outbound_url`] を併用する。
pub fn is_likely_public_http_url(raw: &str, policy: RegistrationPolicy) -> bool {
    let RegistrationPolicy::PublicHttp = policy;
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    match url.host() {
        Some(url::Host::Ipv4(ip)) => !is_blocked_ip(IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => !is_blocked_ip(IpAddr::V6(ip)),
        Some(url::Host::Domain(host)) => {
            !host.is_empty() && host != "localhost" && !host.ends_with(".localhost")
        }
        None => false,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_non_http_scheme() {
        let e = assert_safe_outbound_url("ftp://example.com")
            .await
            .unwrap_err();
        assert!(e.contains("許可されていないスキーム"));
    }

    #[tokio::test]
    async fn rejects_userinfo() {
        let e = assert_safe_outbound_url("http://user:pass@example.com")
            .await
            .unwrap_err();
        assert!(e.contains("認証情報"));
    }

    #[tokio::test]
    async fn rejects_localhost_and_ip_literals() {
        assert!(assert_safe_outbound_url("http://localhost/")
            .await
            .unwrap_err()
            .contains("ローカルホスト"));
        assert!(assert_safe_outbound_url("http://127.0.0.1/")
            .await
            .unwrap_err()
            .contains("内部/予約済み"));
        assert!(
            assert_safe_outbound_url("http://169.254.169.254/latest/meta-data")
                .await
                .unwrap_err()
                .contains("内部/予約済み")
        );
        assert!(assert_safe_outbound_url("http://[::1]/")
            .await
            .unwrap_err()
            .contains("内部/予約済み"));
    }

    #[test]
    fn ipv4_ranges_match_node() {
        for ip in [
            "0.0.0.1",
            "10.0.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "192.0.0.1",
            "192.0.2.5",
            "198.18.0.1",
            "198.51.100.9",
            "203.0.113.9",
            "100.64.0.1",
            "224.0.0.1",
            "255.255.255.255",
        ] {
            assert!(
                is_blocked_ipv4(ip.parse().unwrap()),
                "expected blocked: {ip}"
            );
        }
        for ip in [
            "8.8.8.8",
            "1.1.1.1",
            "172.15.0.1",
            "172.32.0.1",
            "100.63.0.1",
            "223.255.255.255",
        ] {
            assert!(
                !is_blocked_ipv4(ip.parse().unwrap()),
                "expected allowed: {ip}"
            );
        }
    }

    #[test]
    fn ipv6_ranges_match_node() {
        for ip in [
            "::1",
            "::",
            "fe80::1",
            "fc00::1",
            "fd00::1",
            "fec0::1",
            "ff02::1",
            "2001:db8::1",
        ] {
            assert!(
                is_blocked_ipv6(ip.parse().unwrap()),
                "expected blocked: {ip}"
            );
        }
        assert!(!is_blocked_ipv6("2606:4700:4700::1111".parse().unwrap()));
    }

    #[tokio::test]
    async fn literal_normalization_and_error_contract() {
        for raw in [
            "http://127.1/",
            "http://2130706433/",
            "http://0x7f000001/",
            "http://0177.0.0.1/",
            "http://[::ffff:127.0.0.1]/",
            "http://[::ffff:7f00:1]/",
        ] {
            assert!(
                assert_safe_outbound_url(raw)
                    .await
                    .unwrap_err()
                    .starts_with("内部/予約済みアドレスへの接続は許可されていません:"),
                "{raw}"
            );
        }
        for (raw, error) in [
            ("not-a-url", "URLの形式が不正です。"),
            (
                "ftp://example.com",
                "許可されていないスキームです: ftp:（http/https のみ）",
            ),
            (
                "http://user:pass@example.com",
                "URLに認証情報（user:pass@）を含めることはできません。",
            ),
            (
                "http://foo.LOCALHOST",
                "ローカルホストへの接続は許可されていません。",
            ),
        ] {
            assert_eq!(assert_safe_outbound_url(raw).await, Err(error.to_owned()));
        }
        for raw in [
            "HTTP://8.8.8.8/",
            "https://[2606:4700:4700::1111]/",
            "http://[::ffff:8.8.8.8]/",
        ] {
            assert_eq!(assert_safe_outbound_url(raw).await, Ok(()));
        }
    }

    #[tokio::test]
    async fn dns_checks_every_address_and_preserves_errors() {
        for ips in [
            vec!["8.8.8.8:443", "127.0.0.1:443"],
            vec!["127.0.0.1:443", "8.8.8.8:443"],
        ] {
            let result = assert_safe_outbound_url_with_lookup(
                "https://EXAMPLE.com/feed",
                |host, port| async move {
                    assert_eq!((host.as_str(), port), ("example.com", 443));
                    Ok(ips.into_iter().map(|ip| ip.parse().unwrap()).collect())
                },
            )
            .await;
            assert_eq!(result, Err("内部/予約済みアドレスに解決されるホストへの接続は許可されていません: example.com -> 127.0.0.1".to_owned()));
        }
        let result = assert_safe_outbound_url_with_lookup(
            "http://example.com:8080",
            |host, port| async move {
                assert_eq!((host.as_str(), port), ("example.com", 8080));
                Ok(vec![
                    "8.8.8.8:8080".parse().unwrap(),
                    "[2606:4700:4700::1111]:8080".parse().unwrap(),
                ])
            },
        )
        .await;
        assert_eq!(result, Ok(()));
        for result in [Ok(vec![]), Err(std::io::Error::other("lookup failed"))] {
            assert_eq!(
                assert_safe_outbound_url_with_lookup(
                    "http://example.com",
                    |_, _| async move { result }
                )
                .await,
                Err("ホスト名を解決できませんでした: example.com".to_owned())
            );
        }
        let result = assert_safe_outbound_url_with_lookup("http://example.com", |_, _| async {
            Ok(vec![
                "8.8.8.8:80".parse().unwrap(),
                "[::ffff:127.0.0.1]:80".parse().unwrap(),
            ])
        })
        .await;
        assert!(result
            .unwrap_err()
            .contains("example.com -> ::ffff:127.0.0.1"));
    }

    #[tokio::test]
    async fn dns_is_rechecked_on_each_use_and_skipped_for_literals() {
        let calls = std::cell::Cell::new(0);
        for expected in [true, false] {
            let result = assert_safe_outbound_url_with_lookup("http://example.com", |_, _| {
                let n = calls.get();
                calls.set(n + 1);
                async move {
                    Ok(vec![if n == 0 { "8.8.8.8:80" } else { "127.0.0.1:80" }
                        .parse()
                        .unwrap()])
                }
            })
            .await;
            assert_eq!(result.is_ok(), expected);
        }
        assert_eq!(calls.get(), 2);
        assert!(
            assert_safe_outbound_url_with_lookup("http://8.8.8.8", |_, _| {
                calls.set(calls.get() + 1);
                async { Ok(vec![]) }
            })
            .await
            .is_ok()
        );
        assert_eq!(calls.get(), 2);
    }

    #[tokio::test]
    async fn registration_rejects_reserved_and_normalized_ips_without_dns() {
        let policy = RegistrationPolicy::PublicHttp;
        for raw in [
            "http://127.1/",
            "http://2130706433/",
            "http://0x7f000001/",
            "http://0177.0.0.1/",
            "http://[::ffff:127.0.0.1]/",
            "http://192.0.2.1/",
            "http://198.51.100.1/",
            "http://203.0.113.1/",
            "http://198.18.0.1/",
            "http://198.19.255.255/",
            "http://100.64.0.1/",
            "http://100.127.255.255/",
            "http://224.0.0.1/",
            "http://240.0.0.1/",
            "http://255.255.255.255/",
            "http://[2001:db8::1]/",
            "http://[ff02::1]/",
            "http://[fec0::1]/",
        ] {
            assert!(!is_likely_public_http_url(raw, policy), "{raw}");
            assert!(assert_safe_outbound_url(raw).await.is_err(), "{raw}");
        }
        for raw in [
            "http://localhost/",
            "http://foo.localhost/",
            "http://127.0.0.1/",
            "http://10.1.2.3/",
            "http://169.254.169.254/",
            "http://192.0.0.1/",
            "http://[::1]/",
            "http://[fc00::1]/",
            "http://[fe80::1]/",
            "http://u:p@example.com/",
            "http://example.com:bad/",
            "ftp://example.com/",
        ] {
            assert!(!is_likely_public_http_url(raw, policy), "{raw}");
        }
        // 登録時に DNS は引かず、公開 IP と通常のドメイン名を許可する。
        for raw in [
            "https://example.invalid/",
            "http://8.8.8.8/",
            "HTTP://example.com/",
            "https://[2606:4700:4700::1111]/",
            "http://[::ffff:8.8.8.8]/",
        ] {
            assert!(is_likely_public_http_url(raw, policy), "{raw}");
        }
    }
}
