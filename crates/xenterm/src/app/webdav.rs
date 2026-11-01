use super::*;
use crate::webdav::WebDavAcceptAnyCertVerifier;

fn webdav_url(base: &str, remote_path: &str) -> Result<String> {
    let base = base.trim().trim_end_matches('/');
    if !base.starts_with("http://") && !base.starts_with("https://") {
        anyhow::bail!(
            "{}",
            t(
                "WebDAV 地址必须以 http:// 或 https:// 开头",
                "WebDAV URL must start with http:// or https://"
            )
        );
    }
    if base.starts_with("http://") && webdav_url_uses_port(base, 5006) {
        anyhow::bail!(
            "{}",
            t(
                "飞牛 WebDAV 的 5006 通常是 HTTPS 端口，请改用 https://...:5006；如果要用 HTTP，请改用 5005 端口",
                "FnOS WebDAV port 5006 is usually HTTPS; use https://...:5006, or use port 5005 for HTTP"
            )
        );
    }
    if base.starts_with("https://") && webdav_url_uses_port(base, 5005) {
        anyhow::bail!(
            "{}",
            t(
                "飞牛 WebDAV 的 5005 通常是 HTTP 端口，请改用 http://...:5005；如果要用 HTTPS，请改用 5006 端口",
                "FnOS WebDAV port 5005 is usually HTTP; use http://...:5005, or use port 5006 for HTTPS"
            )
        );
    }
    if base.ends_with(".json") {
        return Ok(base.to_string());
    }
    let remote = remote_path.trim().trim_start_matches('/');
    if (webdav_url_uses_port(base, 5005) || webdav_url_uses_port(base, 5006))
        && !webdav_url_has_path(base)
        && !remote.contains('/')
    {
        anyhow::bail!(
            "{}",
            t(
                "飞牛 WebDAV 需要写入某个共享目录，不能直接写到根路径；请把 WebDAV 地址改成 https://IP:5006/all/，或把远端文件改成 all/meatshell-connections.json",
                "FnOS WebDAV needs a writable shared folder, not the server root; use https://IP:5006/all/ or set the remote file to all/meatshell-connections.json"
            )
        );
    }
    if remote.is_empty() {
        anyhow::bail!("{}", t("远端文件不能为空", "remote file cannot be empty"));
    }
    Ok(format!("{base}/{remote}"))
}

fn webdav_url_uses_port(base: &str, port: u16) -> bool {
    let Some(authority) = base.split("://").nth(1) else {
        return false;
    };
    let host_port = authority.split('/').next().unwrap_or(authority);
    host_port
        .rsplit_once(':')
        .and_then(|(_, p)| p.parse::<u16>().ok())
        == Some(port)
}

fn webdav_url_has_path(base: &str) -> bool {
    let Some(authority) = base.split("://").nth(1) else {
        return false;
    };
    authority
        .split_once('/')
        .is_some_and(|(_, path)| !path.is_empty())
}

fn webdav_auth_header(username: &str, password: &str) -> Option<String> {
    if username.is_empty() && password.is_empty() {
        return None;
    }
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    Some(format!(
        "Basic {}",
        STANDARD.encode(format!("{username}:{password}"))
    ))
}

fn webdav_auth_req(mut req: ureq::Request, auth: Option<&str>) -> ureq::Request {
    if let Some(auth) = auth {
        req = req.set("Authorization", auth);
    }
    req
}

fn webdav_agent(accept_invalid_certs: bool) -> ureq::Agent {
    let mut builder = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(20));
    if accept_invalid_certs {
        let tls_config = ureq::rustls::ClientConfig::builder_with_provider(
            ureq::rustls::crypto::ring::default_provider().into(),
        )
        .with_protocol_versions(&[&ureq::rustls::version::TLS12, &ureq::rustls::version::TLS13])
        .expect("rustls ring provider supports TLS 1.2 and TLS 1.3")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(WebDavAcceptAnyCertVerifier))
        .with_no_client_auth();
        builder = builder.tls_config(Arc::new(tls_config));
    }
    builder.build()
}

fn webdav_error(e: ureq::Error) -> anyhow::Error {
    if let ureq::Error::Status(status, response) = e {
        let url = response.get_url().to_string();
        let body = response.into_string().unwrap_or_default();
        let body = body.trim();
        let detail = if body.is_empty() {
            String::new()
        } else {
            format!(": {}", body.chars().take(240).collect::<String>())
        };
        if status == 400 {
            return anyhow::anyhow!(
                "{}: {url}: status code 400{detail}",
                t(
                    "请求被 WebDAV 服务拒绝，请检查地址协议/端口是否匹配，以及远端文件所在目录是否已开启 WebDAV 协议访问",
                    "WebDAV rejected the request; check the URL scheme/port and whether the remote folder allows WebDAV access"
                )
            );
        }
        if status == 405 {
            return anyhow::anyhow!(
                "{}: {url}: status code 405{detail}",
                t(
                    "当前 WebDAV 路径不允许上传；飞牛请写入已开启协议访问的共享目录，例如 WebDAV 地址填 https://IP:5006/all/，或远端文件填 all/meatshell-connections.json",
                    "The current WebDAV path does not allow upload; for FnOS, write into a shared folder such as https://IP:5006/all/ or set remote file to all/meatshell-connections.json"
                )
            );
        }
        return anyhow::anyhow!("{url}: status code {status}{detail}");
    }
    let msg = e.to_string();
    if msg.contains("UnknownIssuer") || msg.contains("invalid peer certificate") {
        anyhow::anyhow!(
            "{} ({msg})",
            t(
                "HTTPS 证书不受信任；如果这是可信 NAS/局域网 WebDAV，请在设置里开启“信任自签名/内网证书”",
                "HTTPS certificate is not trusted; enable \"Trust self-signed / intranet certs\" for a trusted NAS/LAN WebDAV"
            )
        )
    } else {
        anyhow::anyhow!("{msg}")
    }
}

fn webdav_parent_dirs(url: &str) -> Vec<String> {
    let Some((scheme, rest)) = url.split_once("://") else {
        return Vec::new();
    };
    let Some((authority, path)) = rest.split_once('/') else {
        return Vec::new();
    };
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    if parts.len() <= 1 {
        return Vec::new();
    }
    let mut dirs = Vec::with_capacity(parts.len() - 1);
    let mut current = format!("{scheme}://{authority}");
    for part in parts.iter().take(parts.len() - 1) {
        current.push('/');
        current.push_str(part);
        current.push('/');
        dirs.push(current.clone());
        current.pop();
    }
    dirs
}

fn webdav_dir_missing_or_no_create_error() -> anyhow::Error {
    anyhow::anyhow!(
        "{}",
        t(
            "文件夹不存在也无权限创建",
            "folder does not exist and cannot be created"
        )
    )
}

fn webdav_dir_exists(agent: &ureq::Agent, url: &str, auth: Option<&str>) -> Result<bool> {
    let req = webdav_auth_req(agent.request("PROPFIND", url).set("Depth", "0"), auth);
    match req.call() {
        Ok(_) => Ok(true),
        Err(ureq::Error::Status(status, _)) if status == 404 || status == 409 => Ok(false),
        Err(ureq::Error::Status(status, _)) if status == 401 || status == 403 || status == 405 => {
            Err(webdav_dir_missing_or_no_create_error())
        }
        Err(e) => Err(webdav_error(e)),
    }
}

fn webdav_create_dir(agent: &ureq::Agent, url: &str, auth: Option<&str>) -> Result<()> {
    let req = webdav_auth_req(agent.request("MKCOL", url), auth);
    match req.call() {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(status, _)) if status == 405 => Ok(()),
        Err(ureq::Error::Status(status, _))
            if status == 401 || status == 403 || status == 404 || status == 409 =>
        {
            Err(webdav_dir_missing_or_no_create_error())
        }
        Err(e) => Err(webdav_error(e)),
    }
}

fn webdav_ensure_parent_dirs(agent: &ureq::Agent, url: &str, auth: Option<&str>) -> Result<()> {
    for dir in webdav_parent_dirs(url) {
        if !webdav_dir_exists(agent, &dir, auth)? {
            webdav_create_dir(agent, &dir, auth)?;
        }
    }
    Ok(())
}

/// Upload the configuration to a WebDAV collection, creating the collection's parents.
///
/// `pub(crate)` because the UI shell drives the same upload from its sync path: the
/// shape of the URL, the auth header and the MKCOL walk are decisions rather than
/// drawings.
pub(crate) fn webdav_put_json(
    base_url: &str,
    remote_path: &str,
    username: &str,
    password: &str,
    accept_invalid_certs: bool,
    json: String,
) -> Result<()> {
    let url = webdav_url(base_url, remote_path)?;
    let agent = webdav_agent(accept_invalid_certs);
    let auth = webdav_auth_header(username, password);
    webdav_ensure_parent_dirs(&agent, &url, auth.as_deref())?;
    let req = webdav_auth_req(
        agent.put(&url).set("Content-Type", "application/json"),
        auth.as_deref(),
    );
    req.send_string(&json).map(|_| ()).map_err(webdav_error)
}

pub(crate) fn webdav_get_json(
    base_url: &str,
    remote_path: &str,
    username: &str,
    password: &str,
    accept_invalid_certs: bool,
) -> Result<String> {
    let url = webdav_url(base_url, remote_path)?;
    let agent = webdav_agent(accept_invalid_certs);
    let auth = webdav_auth_header(username, password);
    let req = webdav_auth_req(agent.get(&url), auth.as_deref());
    req.call()
        .map_err(webdav_error)?
        .into_string()
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// One WebDAV sync's inputs, read out of the store while it is still on this thread.
///
/// The request itself runs elsewhere — `ureq` blocks, and a twenty-second timeout would
/// be twenty seconds of a frozen window — so what crosses to that thread is this struct
/// and nothing else. The store itself never leaves the calling thread: its `Rc` is not
/// `Send`, and a sync that mutated the configuration from a worker thread would be a
/// data race with the session list next to it.
pub(crate) struct WebdavSync {
    url: String,
    remote_path: String,
    username: String,
    password: String,
    accept_invalid_certs: bool,
    /// Whether this sync is sending the configuration rather than fetching it. It only
    /// decides the wording of a failure, but a download that says "upload failed" is a
    /// message about the wrong request.
    upload: bool,
    /// The export to PUT, and how many connections it holds. `None` for a fetch.
    payload: Option<(String, usize)>,
}

/// What the network did, in the shape the status line needs.
///
/// A fetch's body is carried, not consumed: turning it into sessions is the store's job
/// and the store is only reachable back on the calling thread, so this struct is the
/// value that crosses back.
pub(crate) struct WebdavRun {
    upload: bool,
    count: Option<usize>,
    result: Result<Option<String>>,
}

impl WebdavSync {
    /// Gather what a sync needs, or the line the status bar should show instead.
    ///
    /// The two refusals are ordinary answers rather than errors to log: sync is off, or
    /// the configuration could not be turned into JSON. Both are the store's own wording
    /// because both are facts about the configuration, not about the request.
    pub(crate) fn prepare(store: &ConfigStore, upload: bool) -> Result<Self, String> {
        if !store.webdav_enabled() {
            return Err(t("请先启用 WebDAV 同步", "enable WebDAV sync first").to_string());
        }
        // The upload's payload is built here, while the store is reachable.
        let payload = if upload {
            match store.export_json() {
                Ok(payload) => Some(payload),
                Err(error) => {
                    return Err(format!("{}: {error}", t("导出失败", "export failed")));
                }
            }
        } else {
            None
        };
        Ok(Self {
            url: store.webdav_url().to_string(),
            remote_path: store.webdav_remote_path().to_string(),
            username: store.webdav_username().to_string(),
            password: store.webdav_password().to_string(),
            accept_invalid_certs: store.webdav_accept_invalid_certs(),
            upload,
            payload,
        })
    }

    /// Run the request, blocking the calling thread until the server answers or the
    /// agent's timeout expires.
    ///
    /// Blocking is deliberate and is why this is not called from a click handler: `ureq`
    /// has no async API. Callers hand this to a background executor and await the
    /// `WebdavRun` that comes back.
    pub(crate) fn run(self) -> WebdavRun {
        let count = self.payload.as_ref().map(|(_, count)| *count);
        let json = self.payload.map(|(json, _)| json);
        let result = if let Some(json) = json {
            webdav_put_json(
                &self.url,
                &self.remote_path,
                &self.username,
                &self.password,
                self.accept_invalid_certs,
                json,
            )
            .map(|_| None)
        } else {
            webdav_get_json(
                &self.url,
                &self.remote_path,
                &self.username,
                &self.password,
                self.accept_invalid_certs,
            )
            .map(Some)
        };
        WebdavRun {
            upload: self.upload,
            count,
            result,
        }
    }
}

impl WebdavRun {
    /// The one line the status bar shows for what came back.
    ///
    /// A fetched body is imported *here*, inside this line's own computation, and that
    /// placement is the point: a download that returned its body without handing it to
    /// `import_json` would report a success and change nothing on disk. `import_json` is
    /// what upserts the sessions and saves the file, and a failure to parse the body is
    /// reported rather than swallowed — the configuration is untouched, so the message is
    /// the only thing the user gets.
    pub(crate) fn status_line(self, store: &mut ConfigStore) -> String {
        match self.result {
            Ok(None) => format!(
                "{} {}",
                t("已上传连接", "uploaded connections"),
                self.count.unwrap_or_default()
            ),
            Ok(Some(body)) => match store.import_json(&body) {
                Ok((added, skipped)) => format!(
                    "{} {added} · {} {skipped}",
                    t("已导入", "imported"),
                    t("已跳过", "skipped")
                ),
                Err(error) => format!("{}: {error}", t("导入失败", "import failed")),
            },
            Err(error) => format!(
                "{}: {error}",
                if self.upload {
                    t("上传失败", "upload failed")
                } else {
                    t("下载失败", "download failed")
                }
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The URL is the one piece of this that a wrong answer ruins quietly: a base with a
    /// trailing slash, a remote path with or without one, and a path that is already
    /// there all have to come out as one URL with one slash between the parts.
    #[test]
    fn the_url_joins_base_and_remote_path_once() {
        assert_eq!(
            webdav_url("https://dav.example.com", "/meatshell/sessions.json").unwrap(),
            "https://dav.example.com/meatshell/sessions.json"
        );
        assert_eq!(
            webdav_url("https://dav.example.com/", "meatshell/sessions.json").unwrap(),
            "https://dav.example.com/meatshell/sessions.json"
        );
        assert_eq!(
            webdav_url("https://dav.example.com/base/", "/meatshell/x.json").unwrap(),
            "https://dav.example.com/base/meatshell/x.json"
        );
        // A remote path that is only a directory still lands under it, not on it.
        assert_eq!(
            webdav_url("https://dav.example.com", "/meatshell").unwrap(),
            "https://dav.example.com/meatshell"
        );
    }

    #[test]
    fn a_base_without_a_scheme_is_refused_rather_than_guessed_at() {
        assert!(webdav_url("dav.example.com", "/x.json").is_err());
        assert!(webdav_url("", "/x.json").is_err());
    }

    /// The auth header is absent only when there is nothing to send. A username with no
    /// password, and a password with no username, are both real configurations — some
    /// servers take a token as the password and ignore the user — so an empty half is not
    /// a reason to drop the header.
    #[test]
    fn the_auth_header_is_absent_only_when_both_halves_are() {
        let header = webdav_auth_header("me", "secret").expect("a header for a named user");
        assert!(header.starts_with("Basic "));
        assert!(webdav_auth_header("me", "").is_some());
        assert!(webdav_auth_header("", "secret").is_some());
        assert!(webdav_auth_header("", "").is_none());
    }

    /// Every directory above the file has to be created, from the top down, and each one
    /// is named as a collection: `MKCOL` on a URL without the trailing slash asks some
    /// servers to create a *file* and gets a 301 from others.
    #[test]
    fn the_parents_of_a_file_are_listed_from_the_top_down() {
        let parents = webdav_parent_dirs("https://dav.example.com/a/b/sessions.json");
        assert_eq!(
            parents,
            vec!["https://dav.example.com/a/", "https://dav.example.com/a/b/",]
        );
        // A file at the root has no parents to create.
        assert!(webdav_parent_dirs("https://dav.example.com/sessions.json").is_empty());
    }
}
