//! Format validation for values entering the config layer from untrusted
//! files (imports, pasted lists). Pure predicates, no I/O.
//!
//! Lives in the bottom layer so every import path — `~/.ssh/config`,
//! FinalShell exports, pasted batch lists, proxy URLs — can share one
//! definition of "this string is a hostname and nothing else".

/// Accepts IPv4 / IPv6 literals and DNS-style hostnames; rejects anything with
/// shell metacharacters, whitespace, control characters, scheme prefixes
/// (`http://…`), etc. This stops a malformed or hostile import from flowing
/// unchecked into a saved session or an HTTP CONNECT request line. Internal
/// IPs are intentionally allowed — they are a legitimate SSH target and can't
/// be told apart by format.
pub(crate) fn is_valid_hostname(s: &str) -> bool {
    // IP literals (including private/loopback addresses) are always fine.
    if s.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    if s.is_empty() || s.len() > 253 {
        return false;
    }
    // DNS-style: dot-separated labels of [A-Za-z0-9_-], no empty or
    // hyphen-edged labels.
    s.split('.').all(|label| {
        let b = label.as_bytes();
        !b.is_empty()
            && b.len() <= 63
            && b.iter()
                .all(|&c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
            && b[0] != b'-'
            && b[b.len() - 1] != b'-'
    })
}

/// Borrowed structural pieces shared by connection parsing and credential
/// mapping. Userinfo is literal: percent characters are not URL-decoded, and
/// the final `@` separates it from the endpoint. No Debug impl because auth can
/// contain a password.
pub(crate) struct ProxyUrlParts<'a> {
    pub(crate) scheme: Option<&'a str>,
    pub(crate) auth: Option<(&'a str, &'a str)>,
    pub(crate) hostport: &'a str,
}

pub(crate) fn split_proxy_url(value: &str) -> ProxyUrlParts<'_> {
    let (scheme, rest) = match value.split_once("://") {
        Some((scheme, rest)) => (Some(scheme), rest),
        None => (None, value),
    };
    let (auth, hostport) = match rest.rsplit_once('@') {
        Some((userinfo, hostport)) => (
            Some(userinfo.split_once(':').unwrap_or((userinfo, ""))),
            hostport,
        ),
        None => (None, rest),
    };
    ProxyUrlParts {
        scheme,
        auth,
        hostport,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_literals_and_dns_names() {
        assert!(is_valid_hostname("example.com"));
        assert!(is_valid_hostname("db.internal"));
        assert!(is_valid_hostname("192.168.1.10"));
        assert!(is_valid_hostname("::1"));
        assert!(is_valid_hostname("a-b.example.com"));
    }

    #[test]
    fn rejects_injection_shapes() {
        assert!(!is_valid_hostname(""));
        assert!(!is_valid_hostname("pro xy"));
        assert!(!is_valid_hostname("evil\r\nX-Injected: 1"));
        assert!(!is_valid_hostname("http://evil"));
        assert!(!is_valid_hostname("-leading"));
        assert!(!is_valid_hostname("trailing-"));
        assert!(!is_valid_hostname("a..b"));
    }
}
