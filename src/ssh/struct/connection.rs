use crate::config::Secret;

/// Result of checking a server key against the known-hosts store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyStatus {
    Unknown,
    Match,
    Changed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ProxyKind {
    Socks5,
    Http,
    /// Parsed but rejected at connect time: an HTTPS proxy needs a TLS layer
    /// we do not implement, and silently treating it as a plaintext HTTP
    /// proxy would send the Proxy-Authorization header in the clear.
    Https,
}

#[derive(Clone)]
pub struct ProxyConfig {
    pub(crate) kind: ProxyKind,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) auth: Option<(String, Secret)>,
}

impl std::fmt::Debug for ProxyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyConfig")
            .field("kind", &self.kind)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("auth", &self.auth.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

/// One importable host parsed from `~/.ssh/config`.
#[derive(Debug, Clone)]
pub struct ImportedHost {
    pub alias: String,
    pub hostname: String,
    pub user: String,
    pub port: u16,
    pub identity_file: String,
}
