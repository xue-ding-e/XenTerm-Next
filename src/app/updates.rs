//! Discovery and downloads for the Next GPUI desktop beta.

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, ensure, Context as _, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::version::{self, Channel, UpdateDecision, VersionInfo};

#[cfg(test)]
#[path = "updates_tests.rs"]
mod tests;
#[cfg(windows)]
#[path = "updates_windows.rs"]
mod windows;

const REPOSITORY: &str = "xue-ding-e/XenTerm-Next";
const RELEASE_PAGE: &str = "https://github.com/xue-ding-e/XenTerm-Next/releases/tag/beta";
const RELEASE_API: &str = "https://api.github.com/repos/xue-ding-e/XenTerm-Next/releases/tags/beta";
const RELEASE_ASSETS: &str =
    "https://github.com/xue-ding-e/XenTerm-Next/releases/expanded_assets/beta";
const DOWNLOAD_BASE: &str = "https://github.com/xue-ding-e/XenTerm-Next/releases/download/beta/";
const PREFIX: &str = "XenTerm-Next-beta-";
const MAX_METADATA: u64 = 2 * 1024 * 1024;
const MAX_PACKAGE: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) enum Install {
    Archive,
    #[cfg(windows)]
    Msi,
    #[cfg(windows)]
    ManagedPortable(PathBuf),
}

impl Install {
    fn current() -> Self {
        #[cfg(windows)]
        {
            return windows::current_install();
        }
        #[cfg(not(windows))]
        Self::Archive
    }

    fn package_suffix(&self) -> Option<&'static str> {
        match (std::env::consts::OS, std::env::consts::ARCH, self) {
            #[cfg(windows)]
            ("windows", "x86_64", Self::Msi) => Some("windows-x86_64.msi"),
            ("windows", "x86_64", _) => Some("windows-x86_64.zip"),
            ("linux", "x86_64", _) => Some("linux-x86_64.tar.gz"),
            ("macos", "aarch64", _) => Some("macos-aarch64.zip"),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub(crate) enum Check {
    Unavailable,
    Current,
    Available(Box<Update>),
}

#[derive(Clone, Debug)]
pub(crate) struct Update {
    pub version: String,
    pub install: Install,
    commit: String,
    asset: Asset,
}

#[derive(Debug)]
pub(crate) enum Download {
    Saved(PathBuf),
    #[cfg(windows)]
    InstallerStarted,
    #[cfg(windows)]
    RestartToApply,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    name: String,
    body: String,
    html_url: String,
    prerelease: bool,
    draft: bool,
    assets: Vec<Asset>,
}

#[derive(Clone, Debug, Deserialize)]
struct Asset {
    name: String,
    state: String,
    size: Option<u64>,
    digest: Option<String>,
    browser_download_url: String,
}

#[derive(Deserialize)]
struct Build {
    repository: String,
    commit: String,
    version_scheme: String,
    runtime_version: String,
    version_timestamp: u64,
    platforms: Vec<String>,
}

#[cfg(windows)]
#[derive(Deserialize)]
struct MsiReceipt {
    product: String,
    source_sha: String,
    runtime_version: String,
    sha256: String,
    upgrade_test_fixture: bool,
}

impl Release {
    fn promoted_commit(&self) -> Result<&str> {
        ensure!(
            self.tag_name == "beta"
                && self.html_url == RELEASE_PAGE
                && self.prerelease
                && !self.draft,
            "unexpected GPUI release"
        );
        // The publisher promotes the release text after verifying all assets.
        // Older and newly uploaded assets can coexist under this rolling tag.
        let mut commits = self
            .body
            .lines()
            .filter_map(|line| line.strip_prefix("源码 SHA: `")?.strip_suffix('`'));
        let commit = commits
            .next()
            .filter(|sha| is_hex(sha, 40))
            .context("missing promoted GPUI source SHA")?;
        ensure!(commits.next().is_none(), "ambiguous GPUI source SHA");
        ensure!(
            self.name == format!("XenTerm Next GPUI beta ({})", &commit[..12]),
            "release does not identify the promoted GPUI build"
        );
        Ok(commit)
    }

    fn asset(&self, name: &str) -> Result<&Asset> {
        let mut matches = self.assets.iter().filter(|asset| asset.name == name);
        let asset = matches
            .next()
            .with_context(|| format!("missing GPUI asset {name}"))?;
        ensure!(matches.next().is_none(), "duplicate GPUI asset {name}");
        ensure!(
            asset.state == "uploaded"
                && asset.size != Some(0)
                && asset.browser_download_url == format!("{DOWNLOAD_BASE}{name}"),
            "invalid GPUI asset {name}"
        );
        asset.sha256()?;
        Ok(asset)
    }
}

impl Asset {
    fn sha256(&self) -> Result<&str> {
        self.digest
            .as_deref()
            .and_then(|value| value.strip_prefix("sha256:"))
            .filter(|sha| is_hex(sha, 64))
            .with_context(|| format!("missing SHA-256 for {}", self.name))
    }
}

fn is_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn https_agent(timeout: Duration) -> ureq::Agent {
    let builder = ureq::AgentBuilder::new()
        .user_agent("XenTerm-Next-GPUI")
        .try_proxy_from_env(true)
        .timeout_connect(Duration::from_secs(8))
        .timeout_read(Duration::from_secs(30))
        .timeout(timeout);
    #[cfg(windows)]
    let builder = if ["HTTPS_PROXY", "ALL_PROXY", "HTTP_PROXY"]
        .iter()
        .all(|name| std::env::var_os(name).is_none_or(|value| value.is_empty()))
    {
        if let Some(proxy) = windows::system_proxy() {
            builder.proxy(proxy)
        } else {
            builder
        }
    } else {
        builder
    };
    builder.build()
}

fn read_metadata(mut input: impl Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    input
        .by_ref()
        .take(MAX_METADATA + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_METADATA,
        "update metadata exceeds its size limit"
    );
    Ok(bytes)
}

fn copy_verified(
    asset: &Asset,
    input: impl Read,
    output: &mut impl Write,
    limit: u64,
) -> Result<()> {
    let max_size = asset.size.unwrap_or(limit);
    ensure!(max_size <= limit, "update asset exceeds its size limit");
    let mut input = input.take(max_size + 1);
    let mut hasher = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    let mut size = 0;
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        size += count as u64;
        ensure!(size <= max_size, "update asset exceeds its size limit");
        hasher.update(&buffer[..count]);
        output.write_all(&buffer[..count])?;
    }
    ensure!(
        size != 0 && asset.size.is_none_or(|expected| size == expected),
        "incomplete update asset {}",
        asset.name
    );
    ensure!(
        format!("{:x}", hasher.finalize()) == asset.sha256()?,
        "SHA-256 mismatch for {}",
        asset.name
    );
    Ok(())
}

fn asset_metadata(agent: &ureq::Agent, asset: &Asset) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let response = agent.get(&asset.browser_download_url).call()?;
    copy_verified(asset, response.into_reader(), &mut bytes, MAX_METADATA)?;
    Ok(bytes)
}

fn confirm_checksum(sums: &[u8], asset: &Asset) -> Result<()> {
    let mut hashes = std::str::from_utf8(sums)?.lines().filter_map(|line| {
        let (hash, name) = line.split_once("  ")?;
        (name == asset.name).then_some(hash)
    });
    ensure!(
        hashes.next() == Some(asset.sha256()?) && hashes.next().is_none(),
        "release checksum mismatch for {}",
        asset.name
    );
    Ok(())
}

fn check_release(
    current: &str,
    release: Release,
    install: Install,
    suffix: &str,
    read_asset: impl Fn(&Asset) -> Result<Vec<u8>>,
) -> Result<Check> {
    let commit = release.promoted_commit()?;
    let prefix = format!("{PREFIX}{commit}-");
    let build_asset = release.asset(&format!("{prefix}BUILD.json"))?;
    let build: Build = serde_json::from_slice(&read_asset(build_asset)?)?;
    let version =
        VersionInfo::parse(&build.runtime_version).context("invalid GPUI build version")?;
    let (platform, _) = suffix.split_once('.').context("unsupported package type")?;
    ensure!(
        build.repository == REPOSITORY
            && build.commit == commit
            && build.version_scheme == "xt-v1"
            && build.platforms.iter().any(|name| name == platform)
            && version.channel == Channel::Beta
            && version.hash() == Some(&commit[..7])
            && version.timestamp() == Some(build.version_timestamp),
        "GPUI build provenance does not match its release"
    );
    if version::update_decision(current, &build.runtime_version) != Some(UpdateDecision::Update) {
        return Ok(Check::Current);
    }

    let asset = release.asset(&format!("{prefix}{suffix}"))?;
    let sums = read_asset(release.asset(&format!("{prefix}SHA256SUMS"))?)?;
    confirm_checksum(&sums, asset)?;
    confirm_checksum(&sums, build_asset)?;
    #[cfg(windows)]
    if matches!(install, Install::Msi) {
        let receipt_asset = release.asset(&format!("{}.json", asset.name))?;
        confirm_checksum(&sums, receipt_asset)?;
        let receipt: MsiReceipt = serde_json::from_slice(&read_asset(receipt_asset)?)?;
        ensure!(
            receipt.product == "XenTerm-Next GPUI"
                && receipt.source_sha == commit
                && receipt.runtime_version == build.runtime_version
                && receipt.sha256 == asset.sha256()?
                && !receipt.upgrade_test_fixture,
            "invalid GPUI installer receipt"
        );
    }
    Ok(Check::Available(Box::new(Update {
        version: build.runtime_version,
        install,
        commit: commit.to_owned(),
        asset: asset.clone(),
    })))
}

pub(crate) fn check() -> Result<Check> {
    check_latest(version::VERSION, Install::current())
}

fn read_release(agent: &ureq::Agent) -> Result<Release> {
    let response = agent
        .get(RELEASE_API)
        .set("Accept", "application/vnd.github+json")
        .call();
    match response {
        Ok(response) => Ok(serde_json::from_slice(&read_metadata(
            response.into_reader(),
        )?)?),
        Err(ureq::Error::Status(status, response))
            if status == 429
                || (status == 403 && response.header("X-RateLimit-Remaining") == Some("0")) =>
        {
            let page = read_metadata(agent.get(RELEASE_PAGE).call()?.into_reader())?;
            let assets = read_metadata(agent.get(RELEASE_ASSETS).call()?.into_reader())?;
            release_from_web(std::str::from_utf8(&page)?, std::str::from_utf8(&assets)?)
        }
        Err(error) => Err(error.into()),
    }
}

fn release_from_web(page: &str, assets: &str) -> Result<Release> {
    // The public release and asset views remain available when the REST budget
    // is shared with other clients. Require the same promoted SHA and digests.
    let sha_pattern = regex::Regex::new(r"<p>源码 SHA: <code>([a-f0-9]{40})</code>")?;
    let mut commits = sha_pattern.captures_iter(page);
    let selected = commits
        .next()
        .context("missing promoted GPUI source SHA on release page")?;
    let sha = &selected[1];
    let name = format!("XenTerm Next GPUI beta ({})", &sha[..12]);
    ensure!(
        commits.next().is_none()
            && page.contains(&format!(
                "<title>Release {name} · {REPOSITORY} · GitHub</title>"
            ))
            && page.contains(&format!("href=\"{DOWNLOAD_BASE}{PREFIX}{sha}-BUILD.json\"")),
        "unexpected GPUI release page"
    );
    let pattern = regex::Regex::new(
        r#"<clipboard-copy [^>]*aria-label="Copy to clipboard digest for ([A-Za-z0-9._-]+)"[^>]*value="(sha256:[a-f0-9]{64})""#,
    )?;
    let assets = pattern
        .captures_iter(assets)
        .map(|capture| Asset {
            name: capture[1].to_owned(),
            browser_download_url: format!("{DOWNLOAD_BASE}{}", &capture[1]),
            state: "uploaded".into(),
            size: None,
            digest: Some(capture[2].to_owned()),
        })
        .collect();
    Ok(Release {
        tag_name: "beta".into(),
        name,
        body: format!("源码 SHA: `{sha}`"),
        html_url: RELEASE_PAGE.into(),
        prerelease: true,
        draft: false,
        assets,
    })
}

fn check_latest(current: &str, install: Install) -> Result<Check> {
    let Some(suffix) = install.package_suffix() else {
        return Ok(Check::Unavailable);
    };
    if VersionInfo::parse(current).map(|v| v.channel) != Some(Channel::Beta) {
        return Ok(Check::Unavailable);
    }
    let agent = https_agent(Duration::from_secs(15));
    let release = read_release(&agent)?;
    check_release(current, release, install, suffix, |asset| {
        asset_metadata(&agent, asset)
    })
}

impl Update {
    pub(crate) fn download(&self, directory: Option<&Path>) -> Result<Download> {
        // Keep the installer on disk so Windows Installer can open it after spawning.
        let staging = if directory.is_none() {
            Some(
                tempfile::Builder::new()
                    .prefix("xenterm-next-gpui-update-")
                    .tempdir()?,
            )
        } else {
            None
        };
        let destination = directory
            .or_else(|| staging.as_ref().map(|dir| dir.path()))
            .context("missing update destination")?;
        let path = destination.join(&self.asset.name);
        let mut temporary = tempfile::NamedTempFile::new_in(destination)?;
        let agent = https_agent(Duration::from_secs(300));
        let response = agent.get(&self.asset.browser_download_url).call()?;
        copy_verified(
            &self.asset,
            response.into_reader(),
            &mut temporary,
            MAX_PACKAGE,
        )?;
        temporary.as_file().sync_all()?;
        temporary.persist_noclobber(&path)?;
        match &self.install {
            Install::Archive if directory.is_some() => Ok(Download::Saved(path)),
            #[cfg(windows)]
            Install::Msi => {
                windows::start_installer(&path)?;
                if let Some(staging) = staging {
                    let _ = staging.keep();
                }
                Ok(Download::InstallerStarted)
            }
            #[cfg(windows)]
            Install::ManagedPortable(runtime) => {
                windows::stage_portable(runtime, &path, &self.commit, &self.version)?;
                Ok(Download::RestartToApply)
            }
            _ => bail!("select a download folder"),
        }
    }
}
