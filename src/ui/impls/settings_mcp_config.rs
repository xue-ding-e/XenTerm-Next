//! The local MCP client example. It describes an invocation, not a shell script:
//! paths and arguments are serialized separately so spaces and quotes stay data.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConfigError {
    ExecutableUnavailable,
    InvalidExecutable,
    InvalidProfile,
    Serialization,
}

/// Do not pin the default desktop profile with --data-dir: explicit profiles
/// deliberately disable the desktop keyring. Only reproduce that option when
/// this process already selected an explicit profile at startup.
pub(super) fn client_config(
    executable: &Path,
    explicit_profile: Option<&Path>,
) -> Result<String, ConfigError> {
    let command = absolute_unicode_path(executable).ok_or(ConfigError::InvalidExecutable)?;
    let mut args = Vec::new();
    if let Some(profile) = explicit_profile {
        let profile = absolute_unicode_path(profile).ok_or(ConfigError::InvalidProfile)?;
        // configure_profile consumes only leading options, before StartMode
        // detects the mcp serve subcommand.
        args.extend(["--data-dir", profile]);
    }
    args.extend(["mcp", "serve"]);
    serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": {
            "xenterm": {
                "command": command,
                "args": args
            }
        }
    }))
    .map_err(|_| ConfigError::Serialization)
}

fn absolute_unicode_path(path: &Path) -> Option<&str> {
    let text = path.to_str()?;
    // The CLI rejects control characters in profile paths. Applying the same
    // boundary to the executable avoids showing a misleading launch example.
    (path.is_absolute() && !text.chars().any(char::is_control)).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn executable() -> std::path::PathBuf {
        std::env::temp_dir().join("XenTerm tools").join("xenterm")
    }

    #[test]
    fn default_desktop_config_uses_exact_executable_and_stdio_arguments_only() {
        let executable = executable();
        let value: Value =
            serde_json::from_str(&client_config(&executable, None).unwrap()).unwrap();
        assert_eq!(
            value,
            json!({
                "mcpServers": {
                    "xenterm": { "command": executable.to_str().unwrap(), "args": ["mcp", "serve"] }
                }
            })
        );
        assert!(value["mcpServers"]["xenterm"].get("env").is_none());
    }

    #[test]
    fn explicit_profile_precedes_the_mcp_subcommand_and_round_trips_special_characters() {
        let executable = std::env::temp_dir()
            .join("XenTerm \"beta\"\\中文 & $tools")
            .join("xenterm");
        let profile = std::env::temp_dir().join("MCP \"profile\"\\中文 & $data");
        let encoded = client_config(&executable, Some(&profile)).unwrap();
        let value: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(
            value["mcpServers"]["xenterm"]["command"],
            executable.to_str().unwrap()
        );
        assert_eq!(
            value["mcpServers"]["xenterm"]["args"],
            json!(["--data-dir", profile.to_str().unwrap(), "mcp", "serve"])
        );
        assert!(encoded.contains("\\\"beta\\\""));
        assert!(encoded.contains("\\\\"));
        assert!(!encoded.contains("--allow-config-import"));
        assert!(!encoded.contains("--http-config"));
    }

    #[test]
    fn missing_relative_or_control_character_paths_do_not_produce_a_copyable_example() {
        for path in [Path::new(""), Path::new("xenterm"), Path::new("./xenterm")] {
            assert_eq!(
                client_config(path, None),
                Err(ConfigError::InvalidExecutable)
            );
            assert_eq!(
                client_config(&executable(), Some(path)),
                Err(ConfigError::InvalidProfile)
            );
        }
        let invalid = std::env::temp_dir().join("bad\npath");
        assert_eq!(
            client_config(&invalid, None),
            Err(ConfigError::InvalidExecutable)
        );
        assert_eq!(
            client_config(&executable(), Some(&invalid)),
            Err(ConfigError::InvalidProfile)
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_paths_fail_instead_of_copying_a_different_lossy_path() {
        use std::os::unix::ffi::OsStringExt;
        let invalid =
            std::path::PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/xenterm-\xff".to_vec()));
        assert_eq!(
            client_config(&invalid, None),
            Err(ConfigError::InvalidExecutable)
        );
        assert_eq!(
            client_config(&executable(), Some(&invalid)),
            Err(ConfigError::InvalidProfile)
        );
    }
}
