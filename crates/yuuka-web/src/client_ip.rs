//! レート制限用のクライアント IP 解決（Node `getClientIp` 相当）。
//!
//! 直前 peer が `TRUSTED_PROXIES`（[`crate::WebConfig::trusted_proxies`]）に含まれるときだけ
//! `X-Forwarded-For` を見る。エンドポイントごとに判定がずれないよう、ここに集約する。

use std::net::{IpAddr, Ipv6Addr};

use axum::http::HeaderMap;

/// 送信元 IP を決める。
///
/// 直前 peer が `trusted` に含まれるときだけ、`X-Forwarded-For` を右（プロキシに近い側）から見て
/// 信頼プロキシでない最初の値を採用する。IP として解釈できない要素が出たら、そこより左は
/// クライアントが自由に書ける値なので打ち切り、それまでに見た最後の信頼プロキシ（無ければ peer）を
/// 使う。IPv4-mapped IPv6（`::ffff:a.b.c.d`）は IPv4 にそろえてから比べる。
#[must_use]
pub fn client_ip(peer: Option<IpAddr>, headers: &HeaderMap, trusted: &[IpAddr]) -> Option<IpAddr> {
    let peer = peer?.to_canonical();
    let is_trusted = |ip: IpAddr| trusted.iter().any(|t| t.to_canonical() == ip);
    if !is_trusted(peer) {
        return Some(peer);
    }
    // 複数の XFF ヘッダは出現順に連結したものとして扱う（左がクライアント側）。
    let hops: Vec<Option<IpAddr>> = headers
        .get_all("x-forwarded-for")
        .iter()
        .flat_map(|v| match v.to_str() {
            Ok(s) => s
                .split(',')
                .map(|e| e.trim().parse::<IpAddr>().ok())
                .collect(),
            Err(_) => vec![None],
        })
        .collect();
    let mut chosen = peer;
    for hop in hops.into_iter().rev() {
        match hop.map(|ip| ip.to_canonical()) {
            Some(ip) if is_trusted(ip) => chosen = ip,
            Some(ip) => return Some(ip),
            None => break,
        }
    }
    Some(chosen)
}

/// レート制限で数える単位。IPv4 はアドレスそのもの、IPv6 は /64 にまとめる（IPv6 の利用者は
/// 普通 /64 を丸ごと持っていて、アドレスを替えるだけで制限を回避できるため）。
#[must_use]
pub fn rate_limit_key(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & !((1u128 << 64) - 1))),
        v4 @ IpAddr::V4(_) => v4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn xff(values: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for v in values {
            h.append("x-forwarded-for", v.parse().unwrap());
        }
        h
    }

    #[test]
    fn untrusted_peer_ignores_xff() {
        let h = xff(&["1.1.1.1"]);
        assert_eq!(
            client_ip(Some(ip("198.51.100.9")), &h, &[ip("10.0.0.1")]),
            Some(ip("198.51.100.9"))
        );
    }

    #[test]
    fn trusted_peer_takes_rightmost_untrusted_hop() {
        let h = xff(&["1.1.1.1, 192.0.2.7, 10.0.0.2"]);
        let trusted = [ip("10.0.0.1"), ip("10.0.0.2")];
        assert_eq!(
            client_ip(Some(ip("10.0.0.1")), &h, &trusted),
            Some(ip("192.0.2.7"))
        );
    }

    #[test]
    fn missing_xff_falls_back_to_peer() {
        let proxy = ip("10.0.0.1");
        assert_eq!(
            client_ip(Some(proxy), &HeaderMap::new(), &[proxy]),
            Some(proxy)
        );
    }

    #[test]
    fn all_trusted_hops_fall_back_to_leftmost_trusted() {
        let h = xff(&["10.0.0.3, 10.0.0.2"]);
        let trusted = [ip("10.0.0.1"), ip("10.0.0.2"), ip("10.0.0.3")];
        assert_eq!(
            client_ip(Some(ip("10.0.0.1")), &h, &trusted),
            Some(ip("10.0.0.3"))
        );
    }

    #[test]
    fn multiple_xff_headers_are_joined_in_order() {
        // 後のヘッダほどプロキシに近い。右端の非信頼値（2 本目）を採る。
        let h = xff(&["1.1.1.1", "192.0.2.7"]);
        let proxy = ip("10.0.0.1");
        assert_eq!(client_ip(Some(proxy), &h, &[proxy]), Some(ip("192.0.2.7")));
    }

    #[test]
    fn unparsable_hop_stops_the_walk() {
        // 信頼プロキシが解釈できない形式を足した場合、その左（クライアントが書いた値）は採らない。
        let proxy = ip("10.0.0.1");
        for bad in ["203.0.113.5:443", "[2001:db8::1]", "unknown", ""] {
            let h = xff(&[&format!("1.1.1.1, {bad}")]);
            assert_eq!(client_ip(Some(proxy), &h, &[proxy]), Some(proxy), "{bad}");
        }
        // 打ち切った時点までに見た信頼プロキシを使う。
        let trusted = [proxy, ip("10.0.0.2")];
        let h = xff(&["1.1.1.1, unknown, 10.0.0.2"]);
        assert_eq!(client_ip(Some(proxy), &h, &trusted), Some(ip("10.0.0.2")));
    }

    #[test]
    fn ipv4_mapped_addresses_are_canonicalized() {
        // デュアルスタックで待ち受けると IPv4 の peer は ::ffff:127.0.0.1 に見える。
        let h = xff(&["::ffff:192.0.2.7"]);
        assert_eq!(
            client_ip(Some(ip("::ffff:127.0.0.1")), &h, &[ip("127.0.0.1")]),
            Some(ip("192.0.2.7"))
        );
        assert_eq!(
            client_ip(Some(ip("::ffff:198.51.100.9")), &h, &[ip("127.0.0.1")]),
            Some(ip("198.51.100.9"))
        );
    }

    #[test]
    fn rate_limit_key_groups_ipv6_by_64() {
        assert_eq!(rate_limit_key(ip("2001:db8:0:0:aaaa::1")), ip("2001:db8::"));
        assert_eq!(
            rate_limit_key(ip("2001:db8:0:0:ffff:ffff:ffff:ffff")),
            ip("2001:db8::")
        );
        assert_ne!(rate_limit_key(ip("2001:db8:0:1::1")), ip("2001:db8::"));
        assert_eq!(rate_limit_key(ip("203.0.113.1")), ip("203.0.113.1"));
        assert_eq!(rate_limit_key(ip("::ffff:203.0.113.1")), ip("203.0.113.1"));
    }
}
