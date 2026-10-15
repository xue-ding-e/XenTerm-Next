fn main() {
    embed_windows_icon();
    inject_version();
}

/// Embed the application icon into the Windows executable so it shows up in
/// Explorer, the taskbar and shortcuts. No-op on non-Windows targets.
///
/// No application manifest is embedded here on purpose: GPUI's Windows
/// platform crate embeds one of its own, and two RT_MANIFEST resources of the
/// same id cannot be merged — the GNU linker kept both and the loader
/// ended up with the wrong activation context, killing the process at
/// 0xC0000139 STATUS_ENTRYPOINT_NOT_FOUND before `main`. GPUI's manifest
/// already declares Common-Controls 6.0 and PerMonitorV2, which is
/// everything the window needs. (assets/xenterm.exe.manifest is kept in the
/// tree only as a record of what ours declared.)
fn embed_windows_icon() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/xenterm.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/xenterm.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=failed to embed Windows icon: {e}");
        }
    }
}

/// Inject the application version string (版本号规范 v1 §5) as the compile-time
/// `XT_VERSION` that `src/version.rs` reads with `env!`. scripts/version.sh is
/// the single legal assembly point (§3/§4), so this only ever delegates to it —
/// including the format self-check, which makes any invalid XT_VERSION (or any
/// other resolution failure) fail the build instead of falling back (§7).
///
/// Rerun policy: the override env, the script and the git metadata the script
/// derives from (HEAD / refs) invalidate the injected value — a commit, branch
/// checkout or new tag must refresh the string, otherwise an incremental build
/// keeps reporting a stale hash in an otherwise-unchanged tree (§6). The cost
/// is one crate recompile after each such metadata change (source edits don't
/// trigger it), and a dev timestamp that rolls forward with the hash.
fn inject_version() {
    for name in [
        "XT_VERSION",
        "XT_CHANNEL",
        "XT_BASH",
        "GITHUB_SHA",
        "GITHUB_REF_TYPE",
        "GITHUB_REF_NAME",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    println!("cargo:rerun-if-changed=scripts/version.sh");
    // .git/refs is a directory, so cargo rescans it — that covers
    // refs/heads (new commits on the current branch) and refs/tags; a HEAD
    // file change covers branch switches, packed-refs covers post-gc refs.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");
    println!("cargo:rerun-if-changed=.git/packed-refs");

    let bash = find_bash().unwrap_or_else(|e| {
        panic!(
            "cannot locate bash for scripts/version.sh: {e}; \
        set XT_BASH to a bash executable (Git Bash on Windows)"
        )
    });
    let output = std::process::Command::new(&bash)
        .arg("scripts/version.sh")
        .output();
    let output = match output {
        Ok(o) => o,
        Err(e) => panic!(
            "failed to run `bash scripts/version.sh` ({e}); it needs Git Bash \
             on PATH — or set XT_VERSION to a 版本号规范 v1 §1 string directly"
        ),
    };
    if !output.status.success() {
        panic!(
            "scripts/version.sh failed ({}):\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim_end()
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let version = stdout.trim();
    // §4: stdout is exactly one version line. More lines mean the script is
    // broken — refuse rather than embed something arbitrary.
    if version.is_empty() || version.contains(['\n', '\r']) {
        panic!("scripts/version.sh printed {stdout:?} instead of a single version line");
    }
    println!("cargo:rustc-env=XT_VERSION={version}");
}

/// Resolve the bash that runs scripts/version.sh.
///
/// On Windows, spawning a bare "bash" via CreateProcess searches System32
/// BEFORE PATH, which lands on the WSL launcher (C:\Windows\System32\bash.exe)
/// instead of Git Bash — it exits 1 with a UTF-16 "no WSL distro" note on
/// stdout and never runs the script. So on Windows we scan PATH ourselves and
/// skip the Windows system directories; XT_BASH overrides the search.
#[cfg(windows)]
fn find_bash() -> std::io::Result<std::path::PathBuf> {
    use std::path::PathBuf;

    if let Some(bash) = std::env::var_os("XT_BASH") {
        return Ok(PathBuf::from(bash));
    }
    // Canonicalized once for stable prefix compares below.
    let system_dirs: Vec<PathBuf> = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .map(|root| {
            ["System32", "SysNative", "SysWOW64"]
                .iter()
                .filter_map(|sub| root.join(sub).canonicalize().ok())
                .collect()
        })
        .unwrap_or_default();
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let candidate = dir.join("bash.exe");
        if !candidate.is_file() {
            continue;
        }
        if let Ok(canonical) = candidate.canonicalize() {
            if system_dirs.iter().any(|sys| canonical.starts_with(sys)) {
                continue;
            }
        }
        return Ok(candidate);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "no bash.exe on PATH outside the Windows system directories",
    ))
}

#[cfg(not(windows))]
fn find_bash() -> std::io::Result<std::path::PathBuf> {
    Ok(std::env::var_os("XT_BASH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "bash".into()))
}
