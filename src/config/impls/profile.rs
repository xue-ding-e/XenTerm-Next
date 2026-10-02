//! Explicit, isolated profiles for unattended CLI/MCP processes.
use anyhow::{bail, Context, Result};
use std::path::PathBuf;

use super::{DATA_DIR, PINNED_DATA_DIR};

/// Resolve leading --data-dir (or XENTERM_DATA_DIR, then legacy
/// MEATSHELL_DATA_DIR), before tracing, configuration, or known-host access.
/// No fallback, migration, backup restore or shared keyring is used in a pinned
/// profile. Arguments after a subcommand / -- are deliberately not inspected.
pub fn configure_profile(args: &mut Vec<String>) -> Result<()> {
    let env =
        std::env::var_os("XENTERM_DATA_DIR").or_else(|| std::env::var_os("MEATSHELL_DATA_DIR"));
    let (path, consumed) = profile_argument(args, env.map(PathBuf::from))?;
    let Some(path) = path else {
        return Ok(());
    };
    if DATA_DIR.get().is_some() || PINNED_DATA_DIR.get().is_some() {
        bail!("configuration directory is already initialized");
    }
    std::fs::create_dir_all(&path).context("failed to create explicit data directory")?;
    let path = path
        .canonicalize()
        .context("failed to resolve explicit data directory")?;
    if !path.is_dir() {
        bail!("explicit data directory must be a directory");
    }
    // Run before pinning, tracing or any key/migration initialization. Rejected
    // existing profiles must remain byte-for-byte untouched.
    super::ConfigStore::preflight_explicit_profile(&path)?;
    PINNED_DATA_DIR
        .set(path)
        .map_err(|_| anyhow::anyhow!("configuration directory is already initialized"))?;
    args.drain(1..1 + consumed);
    Ok(())
}

pub fn has_explicit_data_dir() -> bool {
    PINNED_DATA_DIR.get().is_some()
}

fn profile_argument(args: &[String], env: Option<PathBuf>) -> Result<(Option<PathBuf>, usize)> {
    let mut selected = None;
    let mut index = 1;
    while let Some(arg) = args.get(index) {
        let path = if arg == "--data-dir" {
            index += 1;
            args.get(index)
                .filter(|path| !path.starts_with("--"))
                .ok_or_else(|| anyhow::anyhow!("--data-dir requires a path"))?
                .clone()
        } else if let Some(path) = arg.strip_prefix("--data-dir=") {
            path.to_string()
        } else {
            break;
        };
        if selected.is_some() {
            bail!("--data-dir may only be supplied once");
        }
        if path.trim().is_empty() || path.chars().any(char::is_control) {
            bail!("--data-dir requires a nonempty valid path");
        }
        selected = Some(PathBuf::from(path));
        index += 1;
    }
    let path = selected.or(env);
    if let Some(path) = &path {
        if path.to_string_lossy().trim().is_empty()
            || path.to_string_lossy().chars().any(char::is_control)
        {
            bail!("data directory environment variable must contain a valid path");
        }
    }
    Ok((path, index - 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }
    #[test]
    fn explicit_directory_precedes_environment_and_preserves_command_arguments() {
        let a = args(&[
            "xenterm",
            "--data-dir",
            "fixture-profile",
            "cli",
            "exec",
            "id",
            "--",
            "--data-dir",
            "remote",
        ]);
        assert_eq!(
            profile_argument(&a, Some("env-profile".into())).unwrap(),
            (Some("fixture-profile".into()), 2)
        );
        assert_eq!(
            profile_argument(&args(&["xenterm", "--data-dir=fixture", "mcp"]), None).unwrap(),
            (Some("fixture".into()), 1)
        );
        assert_eq!(
            profile_argument(
                &args(&["xenterm", "cli", "exec", "--data-dir", "remote"]),
                Some("env-profile".into())
            )
            .unwrap(),
            (Some("env-profile".into()), 0)
        );
    }
    #[test]
    fn malformed_or_duplicate_directories_fail_closed() {
        for a in [
            args(&["xenterm", "--data-dir"]),
            args(&["xenterm", "--data-dir="]),
            args(&["xenterm", "--data-dir", "--json"]),
            args(&["xenterm", "--data-dir=a", "--data-dir=b"]),
        ] {
            assert!(profile_argument(&a, None).is_err());
        }
        assert!(profile_argument(&args(&["xenterm"]), Some(PathBuf::new())).is_err());
    }
}
