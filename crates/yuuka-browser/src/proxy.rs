//! Chromium 用の SSRF フィルタプロキシ（#206）。
//!
//! Chromium は `--proxy-server` を指すこのプロキシ経由でしか外へ出ない。遷移・リダイレクト・
//! JS による遷移・サブリソースは全てここを通るので、1 箇所で次を保証できる。
//! - 宛先ホストを**このプロキシが**解決し、内部/予約レンジが 1 つでもあれば拒否する
//! - 接続は検査済みアドレスへ直接つなぐ（再解決しない）＝DNS rebinding でも内部へ届かない
//!
//! 対応するのは `CONNECT`（https）と絶対 URI 形式の http のみ。http は 1 リクエスト 1 接続
//! （`Connection: close`）に固定し、keep-alive で別ホストへ使い回されないようにする。

use std::net::SocketAddr;
use std::sync::OnceLock;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::OnceCell;
use yuuka_outbound::resolve_public_addrs;

const MAX_HEAD: usize = 16 * 1024;

static PROXY_ADDR: OnceCell<SocketAddr> = OnceCell::const_new();

/// プロセス内で 1 つだけフィルタプロキシを起動し、待受アドレスを返す。
///
/// # Errors
/// ループバックでの待受に失敗したとき。
pub async fn ensure_proxy() -> Result<SocketAddr, String> {
    PROXY_ADDR
        .get_or_try_init(|| async {
            let listener = TcpListener::bind(("127.0.0.1", 0))
                .await
                .map_err(|e| format!("フィルタプロキシの起動に失敗しました: {e}"))?;
            let addr = listener
                .local_addr()
                .map_err(|e| format!("フィルタプロキシの起動に失敗しました: {e}"))?;
            tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        continue;
                    };
                    tokio::spawn(async move {
                        let _ = handle(stream).await;
                    });
                }
            });
            Ok(addr)
        })
        .await
        .copied()
}

/// Chromium へ渡す、プロキシ強制用の起動引数。
///
/// # Errors
/// プロキシの起動に失敗したとき。
pub async fn chromium_proxy_args() -> Result<Vec<String>, String> {
    let addr = ensure_proxy().await?;
    Ok(vec![
        format!("--proxy-server=http://{addr}"),
        // 既定ではループバックがプロキシ除外になる。除外を外し、localhost も必ずプロキシで止める。
        "--proxy-bypass-list=<-loopback>".to_owned(),
    ])
}

/// CLI 起動の Chromium へ渡すサンドボックス関連の引数。
#[must_use]
pub fn sandbox_args() -> Vec<&'static str> {
    if no_sandbox_required() {
        vec!["--no-sandbox"]
    } else {
        Vec::new()
    }
}

/// `--no-sandbox` を付けるか。root 実行（Docker 等でサンドボックスが使えない）か
/// `YUUKA_BROWSER_NO_SANDBOX=1` のときだけ付ける。それ以外はサンドボックスを有効にする。
#[must_use]
pub fn no_sandbox_required() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(|| {
        let forced = std::env::var("YUUKA_BROWSER_NO_SANDBOX").is_ok_and(|v| v == "1");
        forced || running_as_root()
    })
}

fn running_as_root() -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").is_ok_and(|m| m.uid() == 0)
}

async fn handle(mut client: TcpStream) -> std::io::Result<()> {
    let Some((head, rest)) = read_head(&mut client).await? else {
        return respond(&mut client, "400 Bad Request", "不正なリクエストです。").await;
    };
    let Some(first) = head.lines().next() else {
        return respond(&mut client, "400 Bad Request", "不正なリクエストです。").await;
    };
    let mut parts = first.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return respond(&mut client, "400 Bad Request", "不正なリクエストです。").await;
    };

    if method.eq_ignore_ascii_case("CONNECT") {
        let Some((host, port)) = split_host_port(target) else {
            return respond(&mut client, "400 Bad Request", "不正な宛先です。").await;
        };
        let mut upstream = match connect_checked(host, port).await {
            Ok(s) => s,
            Err(msg) => return respond(&mut client, "403 Forbidden", &msg).await,
        };
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
        if !rest.is_empty() {
            upstream.write_all(&rest).await?;
        }
        tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
        return Ok(());
    }

    // 平文 http（絶対 URI 形式）。
    let Ok(url) = reqwest::Url::parse(target) else {
        return respond(&mut client, "400 Bad Request", "不正な宛先です。").await;
    };
    if url.scheme() != "http" || !url.username().is_empty() || url.password().is_some() {
        return respond(&mut client, "403 Forbidden", "許可されていない宛先です。").await;
    }
    let Some(host) = url.host_str() else {
        return respond(&mut client, "400 Bad Request", "不正な宛先です。").await;
    };
    let port = url.port_or_known_default().unwrap_or(80);
    let mut upstream = match connect_checked(host, port).await {
        Ok(s) => s,
        Err(msg) => return respond(&mut client, "403 Forbidden", &msg).await,
    };
    let rewritten = rewrite_http_head(&head, method, &url);
    upstream.write_all(rewritten.as_bytes()).await?;
    if !rest.is_empty() {
        upstream.write_all(&rest).await?;
    }
    tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}

/// 検査済みアドレスへ接続する（最初に成功したものを使う）。
async fn connect_checked(host: &str, port: u16) -> Result<TcpStream, String> {
    let addrs = resolve_public_addrs(host, port).await?;
    for addr in addrs {
        if let Ok(stream) = TcpStream::connect(addr).await {
            return Ok(stream);
        }
    }
    Err(format!("接続できませんでした: {host}"))
}

/// リクエストヘッダ部（`\r\n\r\n` まで）と、同じ読み取りで届いた残りを返す。
async fn read_head(stream: &mut TcpStream) -> std::io::Result<Option<(String, Vec<u8>)>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    loop {
        if let Some(pos) = find_head_end(&buf) {
            let rest = buf.split_off(pos);
            return Ok(Some((String::from_utf8_lossy(&buf).into_owned(), rest)));
        }
        if buf.len() > MAX_HEAD {
            return Ok(None);
        }
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(chunk.get(..n).unwrap_or_default());
    }
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

fn split_host_port(target: &str) -> Option<(&str, u16)> {
    let (host, port) = target.rsplit_once(':')?;
    Some((host, port.parse().ok()?))
}

/// 絶対 URI のリクエスト行を origin 形式へ直し、接続系ヘッダを `Connection: close` に置き換える。
fn rewrite_http_head(head: &str, method: &str, url: &reqwest::Url) -> String {
    let mut path = url.path().to_owned();
    if let Some(q) = url.query() {
        path.push('?');
        path.push_str(q);
    }
    let mut out = format!("{method} {path} HTTP/1.1\r\n");
    for line in head.lines().skip(1) {
        if line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("proxy-") || lower.starts_with("connection:") {
            continue;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    out.push_str("Connection: close\r\n\r\n");
    out
}

async fn respond(client: &mut TcpStream, status: &str, body: &str) -> std::io::Result<()> {
    let msg = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    client.write_all(msg.as_bytes()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn raw_request(req: &str) -> String {
        let addr = ensure_proxy().await.unwrap();
        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_all(req.as_bytes()).await.unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).await.unwrap();
        String::from_utf8_lossy(&out).into_owned()
    }

    #[tokio::test]
    async fn connect_to_internal_addresses_is_forbidden() {
        for target in [
            "127.0.0.1:80",
            "169.254.169.254:80",
            "10.0.0.1:443",
            "localhost:8080",
            "[::1]:80",
        ] {
            let res = raw_request(&format!(
                "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n"
            ))
            .await;
            assert!(res.starts_with("HTTP/1.1 403"), "{target}: {res}");
        }
    }

    #[tokio::test]
    async fn plain_http_to_internal_addresses_is_forbidden() {
        let res =
            raw_request("GET http://127.0.0.1:1/secret HTTP/1.1\r\nHost: 127.0.0.1:1\r\n\r\n")
                .await;
        assert!(res.starts_with("HTTP/1.1 403"), "{res}");
        let res =
            raw_request("GET http://169.254.169.254/latest/meta-data HTTP/1.1\r\nHost: x\r\n\r\n")
                .await;
        assert!(res.starts_with("HTTP/1.1 403"), "{res}");
    }

    #[tokio::test]
    async fn malformed_request_is_rejected() {
        let res = raw_request("GARBAGE\r\n\r\n").await;
        assert!(res.starts_with("HTTP/1.1 400"), "{res}");
    }

    #[test]
    fn rewrite_makes_origin_form_and_closes() {
        let url = reqwest::Url::parse("http://example.com/a/b?x=1").unwrap();
        let head = "GET http://example.com/a/b?x=1 HTTP/1.1\r\nHost: example.com\r\nProxy-Connection: keep-alive\r\nConnection: keep-alive\r\nAccept: */*\r\n\r\n";
        let out = rewrite_http_head(head, "GET", &url);
        assert!(out.starts_with("GET /a/b?x=1 HTTP/1.1\r\n"));
        assert!(out.contains("Host: example.com\r\n"));
        assert!(!out.to_ascii_lowercase().contains("proxy-connection"));
        assert!(!out.contains("keep-alive"));
        assert!(out.ends_with("Connection: close\r\n\r\n"));
    }
}
