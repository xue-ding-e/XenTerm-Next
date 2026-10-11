//! Explicit, isolated profiles for unattended CLI/MCP processes.
use anyhow::{bail, Context, Result};
use std::path::{Component, Path, PathBuf};

use super::{DATA_DIR, PINNED_DATA_DIR};

/// Resolve leading --data-dir (or XENTERM_DATA_DIR, then legacy
/// MEATSHELL_DATA_DIR), before tracing, configuration, or known-host access.
/// No fallback, migration, backup restore or shared keyring is used in a pinned
/// profile. --data-dir after a subcommand / -- is deliberately not inspected.
/// Returns whether startup must remain read-only for a migration preview.
pub fn configure_profile(args: &mut Vec<String>) -> Result<bool> {
    let env =
        std::env::var_os("XENTERM_DATA_DIR").or_else(|| std::env::var_os("MEATSHELL_DATA_DIR"));
    let (path, consumed) = profile_argument(args, env.map(PathBuf::from))?;
    let preview = migration_preview(&args[1 + consumed..]);
    let Some(path) = path else {
        return Ok(preview);
    };
    if DATA_DIR.get().is_some() || PINNED_DATA_DIR.get().is_some() {
        bail!("configuration directory is already initialized");
    }
    let path = if preview {
        preview_directory(&path)?
    } else {
        std::fs::create_dir_all(&path).context("failed to create explicit data directory")?;
        let path = path
            .canonicalize()
            .context("failed to resolve explicit data directory")?;
        if !path.is_dir() {
            bail!("explicit data directory must be a directory");
        }
        path
    };
    // Run before pinning, tracing or any key/migration initialization. Rejected
    // existing profiles must remain byte-for-byte untouched.
    super::ConfigStore::preflight_explicit_profile(&path)?;
    PINNED_DATA_DIR
        .set(path)
        .map_err(|_| anyhow::anyhow!("configuration directory is already initialized"))?;
    args.drain(1..1 + consumed);
    Ok(preview)
}

/// Inspect only these commands' local option positions. A filename, remote
/// command, or option after `--` must never switch startup into preview mode.
fn migration_preview(command: &[String]) -> bool {
    if command.first().map(String::as_str) != Some("cli") {
        return false;
    }
    // A leading option in the source position is rejected by the CLI as a
    // missing path, but an explicit preview must still fail without writes.
    let option_start = if command.get(2).is_some_and(|arg| arg.starts_with("--")) {
        2
    } else {
        3
    };
    let options = command.get(option_start..).unwrap_or_default();
    let options = &options[..options
        .iter()
        .position(|arg| arg == "--")
        .unwrap_or(options.len())];
    options.iter().any(|arg| arg == "--dry-run")
        && match command.get(1).map(String::as_str) {
            Some("import") => options.iter().any(|arg| arg == "--preserve-ids"),
            Some("sync-native") => true,
            _ => false,
        }
}

/// Canonicalize existing ancestors without creating a new destination. Resolve
/// symlinks before `..`, just as filesystem traversal does for ordinary loads.
fn preview_directory(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path).context("failed to resolve explicit data directory")?;
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => continue,
            Component::ParentDir => {
                resolved.pop();
            }
            _ => {
                resolved.push(component.as_os_str());
                match std::fs::metadata(&resolved) {
                    Ok(metadata) if metadata.is_dir() => {
                        resolved = resolved
                            .canonicalize()
                            .context("failed to resolve explicit data directory")?;
                    }
                    Ok(_) => bail!("explicit data directory must be a directory"),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        // A dangling directory alias cannot select a new profile.
                        if std::fs::symlink_metadata(&resolved).is_ok() {
                            bail!("failed to resolve explicit data directory");
                        }
                    }
                    Err(_) => bail!("failed to resolve explicit data directory"),
                }
            }
        }
    }
    Ok(resolved)
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

    #[test]
    fn preview_detection_respects_command_and_option_boundaries() {
        for command in [
            args(&["cli", "import", "export.json", "--preserve-ids", "--dry-run"]),
            args(&["cli", "sync-native", "export.json", "--dry-run", "--json"]),
            args(&["cli", "sync-native", "--dry-run", "--json"]),
            args(&["cli", "import", "--dry-run", "--preserve-ids"]),
            args(&["cli", "import", "--preserve-ids", "--dry-run"]),
            // Bad options still fail without initializing the destination.
            args(&["cli", "sync-native", "export.json", "--dry-run", "--unknown"]),
        ] {
            assert!(migration_preview(&command));
        }
        for command in [
            args(&["cli", "import", "export.json", "--dry-run"]),
            args(&["cli", "import", "export.json", "--preserve-ids"]),
            args(&["cli", "sync-native", "export.json", "--", "--dry-run"]),
            args(&["cli", "exec", "id", "--", "--dry-run", "--preserve-ids"]),
            args(&["mcp", "serve", "file", "--dry-run", "--preserve-ids"]),
        ] {
            assert!(!migration_preview(&command));
        }
    }

    #[test]
    fn preview_directory_does_not_create_missing_ancestors() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("missing");
        let selected = missing.join("..").join("profile");
        assert_eq!(
            preview_directory(&selected).unwrap(),
            root.path().canonicalize().unwrap().join("profile")
        );
        assert!(!missing.exists());
        assert!(!root.path().join("profile").exists());
    }

    #[cfg(unix)]
    #[test]
    fn preview_directory_resolves_symlinks_before_parent_components() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("target/child")).unwrap();
        std::os::unix::fs::symlink(root.path().join("target/child"), root.path().join("alias"))
            .unwrap();
        let selected = root.path().join("alias/../profile");
        assert_eq!(
            preview_directory(&selected).unwrap(),
            root.path().canonicalize().unwrap().join("target/profile")
        );
        assert!(!root.path().join("target/profile").exists());
    }
}
