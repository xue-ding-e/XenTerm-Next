use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read},
    os::windows::{
        ffi::OsStringExt,
        fs::{MetadataExt, OpenOptionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use ::windows::{
    core::{w, PCWSTR},
    Win32::{
        Storage::FileSystem::{FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT},
        System::{
            Registry::{
                RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
                RRF_SUBKEY_WOW6464KEY,
            },
            SystemInformation::GetSystemDirectoryW,
            Threading::CREATE_NO_WINDOW,
        },
    },
};
use anyhow::{ensure, Context as _, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{Install, MAX_PACKAGE, PREFIX, REPOSITORY};

const LOCAL_REPO: &str = "https://github.com/xue-ding-e/XenTerm-Next.git";

pub(super) fn current_install() -> Install {
    if let Ok(exe) = std::env::current_exe().and_then(fs::canonicalize) {
        if installed_executable()
            .and_then(|path| path.canonicalize().ok())
            .as_ref()
            == Some(&exe)
        {
            return Install::Msi;
        }
        if let Some(runtime) = managed_runtime(&exe) {
            return Install::ManagedPortable(runtime);
        }
    }
    Install::Archive
}

fn installed_executable() -> Option<PathBuf> {
    let folder = PathBuf::from(registry_text(
        w!("Software\\XenTerm-Next\\GPUI\\Installer"),
        w!("InstallDir"),
    )?);
    folder.is_absolute().then(|| folder.join("xenterm.exe"))
}

fn registry_text(key: PCWSTR, name: PCWSTR) -> Option<OsString> {
    let mut buffer = [0u16; 4096];
    let mut bytes = std::mem::size_of_val(&buffer) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            name,
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if status.is_err() {
        return None;
    }
    let length = buffer.iter().position(|&value| value == 0)?;
    Some(OsString::from_wide(&buffer[..length]))
}

pub(super) fn system_proxy() -> Option<ureq::Proxy> {
    let key = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings");
    let mut enabled = 0u32;
    let mut bytes = std::mem::size_of_val(&enabled) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            w!("ProxyEnable"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut enabled).cast()),
            Some(&mut bytes),
        )
    };
    if status.is_err() || enabled == 0 {
        return None;
    }
    // One agent handles GitHub's release metadata and its download redirects.
    // Honor a bypass for any of the hosts used by that agent.
    if let Some(bypass) = registry_text(key, w!("ProxyOverride")) {
        let hosts = [
            "github.com",
            "api.github.com",
            "release-assets.githubusercontent.com",
            "objects.githubusercontent.com",
        ];
        if bypass
            .to_string_lossy()
            .split(';')
            .map(str::trim)
            .any(|rule| {
                let pattern = rule
                    .split('*')
                    .map(regex::escape)
                    .collect::<Vec<_>>()
                    .join(".*");
                regex::Regex::new(&format!("(?i)^{pattern}$"))
                    .is_ok_and(|regex| hosts.iter().any(|host| regex.is_match(host)))
            })
        {
            return None;
        }
    }
    let value = registry_text(key, w!("ProxyServer"))?.into_string().ok()?;
    let route = if value.contains('=') {
        value
            .split(';')
            .filter_map(|entry| entry.split_once('='))
            .find(|(scheme, _)| scheme.trim().eq_ignore_ascii_case("https"))?
            .1
    } else {
        value.as_str()
    };
    ureq::Proxy::new(route.split_whitespace().next()?).ok()
}

fn system_tool(name: &str) -> Result<PathBuf> {
    let mut buffer = [0u16; 4096];
    let len = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
    ensure!(
        (1..buffer.len()).contains(&len),
        "Windows system directory is unavailable"
    );
    Ok(PathBuf::from(OsString::from_wide(&buffer[..len])).join(name))
}

pub(super) fn start_installer(path: &Path) -> Result<()> {
    ensure!(
        matches!(current_install(), Install::Msi),
        "running executable is outside the GPUI MSI installation"
    );
    Command::new(system_tool("msiexec.exe")?)
        .arg("/i")
        .arg(path)
        .arg("/norestart")
        .spawn()
        .context("open the GPUI Windows installer")?;
    Ok(())
}

fn plain_path(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0,
        "GPUI runtime path is a Windows reparse point"
    );
    Ok(())
}

fn managed_runtime(exe: &Path) -> Option<PathBuf> {
    let current = exe.parent().filter(|path| path.ends_with("current"))?;
    let runtime = current.parent().filter(|path| path.ends_with("runtime"))?;
    let root = runtime.parent()?;
    let profile = root.join("data").canonicalize().ok()?;
    if crate::config::data_dir().canonicalize().ok()? != profile
        || !["source-next", "source"].iter().any(|source| {
            root.join(source)
                .join("scripts")
                .join("start-local.ps1")
                .is_file()
        })
    {
        return None;
    }
    for path in [exe, current, runtime] {
        plain_path(path).ok()?;
    }
    #[derive(Deserialize)]
    struct LocalBuild {
        repository: String,
        runtime_version: String,
    }
    let mut bytes = Vec::new();
    File::open(current.join("BUILD.json"))
        .ok()?
        .take(16 * 1024)
        .read_to_end(&mut bytes)
        .ok()?;
    // Windows PowerShell 5.1 writes a UTF-8 BOM in the local runtime manifest.
    let build: LocalBuild =
        serde_json::from_slice(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes)).ok()?;
    if build.repository != LOCAL_REPO
        || build.runtime_version.strip_prefix("xenterm ") != Some(crate::version::VERSION)
    {
        return None;
    }
    Some(runtime.to_path_buf())
}

fn extract_member(
    tar: &Path,
    archive: &Path,
    member: &str,
    output: &Path,
    limit: u64,
) -> Result<()> {
    // Extract to stdout and create the exact file ourselves. Archive names,
    // permissions and link attributes never determine a filesystem write.
    let mut dest = File::create_new(output)?;
    let mut task = Command::new(tar)
        .args(["-xOf"])
        .arg(archive)
        .arg(member)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW.0)
        .spawn()?;
    let stdout = task.stdout.take().context("could not read GPUI archive")?;
    let copied = io::copy(&mut stdout.take(limit + 1), &mut dest);
    if !matches!(&copied, Ok(size) if *size > 0 && *size <= limit) {
        let _ = task.kill();
    }
    let status = task.wait()?;
    ensure!(
        status.success() && copied? <= limit && dest.metadata()?.len() > 0,
        "could not extract GPUI archive member {member}"
    );
    dest.sync_all()?;
    Ok(())
}

pub(super) fn prepare_portable(
    stage: &Path,
    archive: &Path,
    commit: &str,
    version: &str,
) -> Result<()> {
    let bundle = format!("{PREFIX}{commit}-windows-x86_64");
    let tar = system_tool("tar.exe")?;
    fs::create_dir(stage.join("licenses"))?;
    for (file, limit) in [
        ("xenterm.exe", MAX_PACKAGE),
        ("BUILD.json", 16 * 1024),
        ("LICENSE", 1024 * 1024),
        ("THIRD_PARTY_NOTICES.md", 1024 * 1024),
        ("XenTerm-Next.txt", 1024 * 1024),
        ("licenses/HeroGPUI-APACHE-2.0", 1024 * 1024),
        ("licenses/HeroGPUI-NOTICE", 1024 * 1024),
    ] {
        extract_member(
            &tar,
            archive,
            &format!("{bundle}/{file}"),
            &stage.join(file),
            limit,
        )?;
    }
    let manifest_path = stage.join("BUILD.json");
    let manifest: serde_json::Value = serde_json::from_reader(File::open(&manifest_path)?)?;
    ensure!(
        manifest["repository"] == REPOSITORY
            && manifest["sha"] == commit
            && manifest["runtime_version"] == version
            && manifest["platform"] == "windows-x86_64",
        "GPUI portable bundle identity does not match the published release"
    );
    let mut hasher = Sha256::new();
    io::copy(&mut File::open(stage.join("xenterm.exe"))?, &mut hasher)?;
    // Match Test-LocalPackage, which verifies the staged binary again on the
    // next launcher run before rotating current/previous.
    let manifest = serde_json::json!({
        "repository": LOCAL_REPO,
        "commit": commit,
        "runtime_version": format!("xenterm {version}"),
        "target": "x86_64-pc-windows-msvc",
        "sha256": format!("{:x}", hasher.finalize()),
    });
    fs::write(manifest_path, serde_json::to_vec_pretty(&manifest)?)?;
    Ok(())
}

pub(super) fn stage_portable(
    runtime: &Path,
    archive: &Path,
    commit: &str,
    version: &str,
) -> Result<()> {
    let exe = std::env::current_exe()?.canonicalize()?;
    ensure!(
        managed_runtime(&exe).as_deref() == Some(runtime),
        "GPUI managed runtime changed"
    );
    let pending = runtime.join("pending");
    let stage = tempfile::Builder::new()
        .prefix(".gpui-update-")
        .tempdir_in(runtime)?;
    prepare_portable(stage.path(), archive, commit, version)?;
    // Same lock as scripts/local-runtime.ps1. Leave any other staged package in place.
    let _lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(runtime.join(".local-update.lock"))?;
    plain_path(runtime)?;
    ensure!(
        matches!(fs::symlink_metadata(&pending), Err(e) if e.kind() == io::ErrorKind::NotFound),
        "a GPUI update is already staged"
    );
    fs::rename(stage.path(), pending).context("stage the verified GPUI update")?;
    Ok(())
}
