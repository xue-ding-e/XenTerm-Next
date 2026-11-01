//! Outbound proxy support for SSH / SFTP connections (issue #7).
//!
//! Establishes the TCP stream to the target host **through a proxy**, then the
//! caller hands that stream to `russh::client::connect_stream`.  Both proxy
//! kinds end up as a transparent `TcpStream`:
//!
//! * **SOCKS5** (`socks5://` / `socks5h://`) via `tokio-socks`; after the
//!   handshake we unwrap to the inner `TcpStream`.
//! * **HTTP / HTTPS CONNECT** (`http://` / `https://`): we issue an HTTP
//!   `CONNECT host:port` and reuse the same socket as the tunnel.
//!
//! The proxy is taken from the per-session setting, falling back to the standard
//! `ALL_PROXY` / `all_proxy` environment variable.

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::structs::{ProxyConfig, ProxyKind};
use crate::config::Secret;

/// Resolve the proxy for a session: the explicit `session_proxy` string if set,
/// otherwise the `ALL_PROXY` / `all_proxy` environment variable.  `Ok(None)`
/// means a direct connection.  A proxy string that is set but unparseable is an
/// **error**, not a silent direct connection — a typo'd URL must never quietly
/// bypass the proxy the user asked for (audit M-13).
pub fn resolve(session_proxy: &str) -> Result<Option<ProxyConfig>> {
    resolve_with_env(session_proxy, |name| std::env::var(name).ok())
}

/// Keep environment lookup injectable so tests need not mutate process-global
/// proxy settings while other tests may be resolving connections.
fn resolve_with_env(
    session_proxy: &str,
    mut env: impl FnMut(&str) -> Option<String>,
) -> Result<Option<ProxyConfig>> {
    let s = session_proxy.trim();
    if !s.is_empty() {
        return parse(s)
            .map(Some)
            .ok_or_else(|| invalid_proxy_url("session proxy"));
    }
    for var in ["ALL_PROXY", "all_proxy"] {
        if let Some(v) = env(var) {
            if !v.trim().is_empty() {
                let v = v.trim();
                return parse(v).map(Some).ok_or_else(|| invalid_proxy_url(var));
            }
        }
    }
    Ok(None)
}

fn invalid_proxy_url(source: &str) -> anyhow::Error {
    anyhow!(
        "invalid {source} URL: expected socks5://, socks5h:// or http:// \
         [user:pass@]host:port"
    )
}

/// Parse a proxy URL: `scheme://[user:pass@]host:port`.
fn parse(url: &str) -> Option<ProxyConfig> {
    let parts = crate::config::validation::split_proxy_url(url);
    let kind = match parts
        .scheme
        .unwrap_or("socks5")
        .to_ascii_lowercase()
        .as_str()
    {
        "socks5" | "socks5h" | "socks" => ProxyKind::Socks5,
        "http" => ProxyKind::Http,
        // Kept distinct so connect() can reject it explicitly instead of
        // silently downgrading to a plaintext CONNECT (audit M-06).
        "https" => ProxyKind::Https,
        _ => return None,
    };
    let auth = parts
        .auth
        .map(|(user, password)| (user.to_string(), Secret::new(password)));
    let hostport = parts.hostport.trim_end_matches('/');
    let (host, port) = hostport.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    // Bracketed IPv6 (`[::1]:1080`) — validate the address, keep it unbracketed
    // for the tuple-based connects that follow.
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if !super::ssh_config::is_valid_hostname(host) {
        return None;
    }
    Some(ProxyConfig {
        kind,
        host: host.to_string(),
        port,
        auth,
    })
}

/// Human-readable description of where we're connecting (for status messages).
pub fn describe(cfg: &ProxyConfig) -> String {
    let scheme = match cfg.kind {
        ProxyKind::Socks5 => "socks5",
        ProxyKind::Http => "http",
        ProxyKind::Https => "https",
    };
    format!("{}://{}:{}", scheme, cfg.host, cfg.port)
}

/// Open a TCP stream to `target_host:target_port` through the proxy.
pub async fn connect(cfg: &ProxyConfig, target_host: &str, target_port: u16) -> Result<TcpStream> {
    match cfg.kind {
        ProxyKind::Socks5 => connect_socks5(cfg, target_host, target_port).await,
        ProxyKind::Http => connect_http(cfg, target_host, target_port).await,
        ProxyKind::Https => bail!(
            "https:// proxies are not supported: without TLS the CONNECT tunnel \
             (and its Proxy-Authorization header) would be sent in the clear. \
             Use http:// or socks5://, or remove the proxy for a direct connection."
        ),
    }
}

async fn connect_socks5(cfg: &ProxyConfig, host: &str, port: u16) -> Result<TcpStream> {
    use tokio_socks::tcp::Socks5Stream;
    let proxy = (cfg.host.as_str(), cfg.port);
    let target = (host, port);
    let stream = match &cfg.auth {
        Some((u, p)) => Socks5Stream::connect_with_password(proxy, target, u, p.as_str())
            .await
            .context("SOCKS5 proxy connect failed")?,
        None => Socks5Stream::connect(proxy, target)
            .await
            .context("SOCKS5 proxy connect failed")?,
    };
    // After the handshake the underlying socket is a transparent tunnel.
    Ok(stream.into_inner())
}

async fn connect_http(cfg: &ProxyConfig, host: &str, port: u16) -> Result<TcpStream> {
    // The target host is interpolated into request text, so it must be a real
    // hostname or IP — a CRLF in it would smuggle extra header lines past the
    // proxy (audit M-07).
    if !super::ssh_config::is_valid_hostname(host) {
        bail!("invalid proxy target host {host:?}");
    }
    let mut s = TcpStream::connect((cfg.host.as_str(), cfg.port))
        .await
        .with_context(|| format!("connect to HTTP proxy {}:{} failed", cfg.host, cfg.port))?;

    let mut req = format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n");
    if let Some((u, p)) = &cfg.auth {
        let token = base64::engine::general_purpose::STANDARD.encode(format!("{u}:{}", p.as_str()));
        req.push_str(&format!("Proxy-Authorization: Basic {token}\r\n"));
    }
    req.push_str("Proxy-Connection: keep-alive\r\n\r\n");
    s.write_all(req.as_bytes())
        .await
        .context("write CONNECT to proxy")?;

    // Read response headers up to the blank line, bounded.
    let mut buf = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    loop {
        let n = s.read(&mut byte).await.context("read proxy response")?;
        if n == 0 {
            bail!("proxy closed the connection during CONNECT");
        }
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
        if buf.len() > 8192 {
            bail!("proxy CONNECT response too large");
        }
    }
    let head = String::from_utf8_lossy(&buf);
    let status_line = head.lines().next().unwrap_or("");
    // Expect "HTTP/1.x 200 ..." — anchored on the protocol token so a header
    // value or body text containing "200" can never pass for the status.
    let ok = status_line.starts_with("HTTP/")
        && status_line
            .split_whitespace()
            .nth(1)
            .map(|c| c == "200")
            .unwrap_or(false);
    if !ok {
        return Err(anyhow!("proxy CONNECT rejected: {}", status_line.trim()));
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_setting_means_direct() {
        assert!(resolve_with_env("", |_| None).unwrap().is_none());
        assert!(resolve_with_env("   ", |_| None).unwrap().is_none());
    }

    #[test]
    fn empty_setting_uses_the_environment_proxy_when_present() {
        for variable in ["ALL_PROXY", "all_proxy"] {
            let proxy = resolve_with_env("", |name| {
                (name == variable).then(|| "socks5://proxy.example:1080".to_string())
            })
            .unwrap()
            .expect("an environment proxy is a supported fallback");
            assert_eq!(proxy.host, "proxy.example");
            assert_eq!(proxy.port, 1080);
        }
    }

    #[test]
    fn an_unparseable_proxy_is_an_error_not_a_direct_connection() {
        // A typo'd URL must fail loudly; silently connecting directly would
        // bypass the proxy the user configured (audit M-13).
        for bad in ["socks9://proxy:1080", "http://proxy:notaport", "http://"] {
            assert!(resolve(bad).is_err(), "{bad} should not resolve");
        }
    }

    #[test]
    fn supported_proxy_forms_preserve_literal_credentials() {
        for scheme in ["", "socks5://", "socks5h://", "http://", "SOCKS5://"] {
            for password in ["synthetic-proxy-secret", "synthetic%40%3A%25secret", "synthetic:p@ss%word", "enc:v1:synthetic-literal"] {
                let proxy = format!("{scheme}fixture:{password}@127.0.0.1:1080");
                let parsed = resolve(&proxy).unwrap().unwrap();
                assert_eq!(parsed.auth.as_ref().unwrap().0, "fixture");
                assert_eq!(parsed.auth.as_ref().unwrap().1.as_str(), password);
            }
        }
        for proxy in ["127.0.0.1:1080", "fixture@127.0.0.1:1080", "fixture:@127.0.0.1:1080", "http://fixture:@127.0.0.1:1080"] {
            assert!(resolve(proxy).unwrap().is_some());
        }
    }

    #[test]
    fn malformed_proxy_errors_never_echo_credentials_or_endpoints() {
        let malformed = "socks9://synthetic-user:synthetic-secret@private.example:1080";
        for error in [
            resolve(malformed).unwrap_err(),
            resolve_with_env("", |name| (name == "ALL_PROXY").then(|| malformed.to_string())).unwrap_err(),
        ] {
            let diagnostic = format!("{error:#}");
            for hidden in [malformed, "synthetic-user", "synthetic-secret", "private.example"] {
                assert!(!diagnostic.contains(hidden));
            }
            assert!(diagnostic.contains("invalid"));
            assert!(diagnostic.contains("expected socks5://"));
        }
    }

    #[test]
    fn hosts_with_request_smuggling_characters_are_rejected() {
        // CRLF / whitespace / scheme prefixes in the proxy or target host must
        // not reach the CONNECT request line (audit M-07 / N-低8).
        assert!(parse("http://evil\r\nX-Injected: 1:8080").is_none());
        assert!(parse("http://pro xy:8080").is_none());
        assert!(parse("http://proxy.evil%0d%0a:8080").is_none());
        assert!(parse("http://proxy:8080").is_some());
        assert!(parse("socks5://[::1]:1080").is_some());
        // Percent-encoded CRLF in the *userinfo* is fine: credentials are
        // base64-encoded into the Proxy-Authorization header, never spliced
        // into request text.
        assert!(parse("http://evil%0d%0aX:1@proxy:8080").is_some());
    }

    #[tokio::test]
    async fn https_proxies_are_recognized_and_rejected_at_connect() {
        let cfg = parse("https://proxy:8443").unwrap();
        assert_eq!(cfg.kind, ProxyKind::Https);
        let err = connect(&cfg, "target", 22).await.unwrap_err();
        assert!(err.to_string().contains("not supported"));
    }
}
