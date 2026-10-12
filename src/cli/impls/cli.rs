use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::path::Path;

use super::structs::CliCommand;

async fn call_cli(name: &str, arguments: &Value) -> Result<Value> {
    crate::automation::call(name, arguments, crate::automation::Frontend::Cli).await
}

pub(crate) fn run(args: &[String]) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("create CLI runtime")?;
    let command_name = args.get(2).map(String::as_str).unwrap_or("help");
    let command = CliCommand::parse(Some(command_name))
        .ok_or_else(|| anyhow!("unknown CLI command: {command_name}"))?;
    let value = match command {
        CliCommand::Export => {
            let path = args
                .get(3)
                .filter(|arg| !arg.starts_with('-'))
                .ok_or_else(|| {
                    anyhow!(
                        "usage: xenterm cli export <new-file.json> --include-credentials [--json]"
                    )
                })?;
            for arg in &args[4..] {
                if !matches!(arg.as_str(), "--include-credentials" | "--json") {
                    return Err(anyhow!(
                        "unknown export option (expected --include-credentials or --json)"
                    ));
                }
            }
            if !args[4..].iter().any(|arg| arg == "--include-credentials") {
                return Err(anyhow!(
                    "portable exports contain recoverable credentials; pass --include-credentials to write a private sessions export"
                ));
            }
            // Keep credential export local to an explicit CLI invocation. It is
            // deliberately not registered as an automation or MCP tool.
            let store = crate::config::ConfigStore::load().context("load XenTerm configuration")?;
            let count = store.export_to_new(std::path::Path::new(path))?;
            eprintln!("Warning: portable exports contain recoverable credentials. Keep the file private. Only saved sessions are exported; global settings, host trust and external key files are not included.");
            json!({"exported": count, "scope": "sessions", "includes_credentials": true})
        }
        CliCommand::Import => {
            let path = args
                .get(3)
                .filter(|arg| !arg.starts_with("--"))
                .ok_or_else(|| {
                    anyhow!("usage: xenterm cli import <export.json> [--dry-run] [--preserve-ids] [--json]")
                })?;
            for arg in &args[4..] {
                if !matches!(arg.as_str(), "--dry-run" | "--preserve-ids" | "--json") {
                    return Err(anyhow!("unknown import option"));
                }
            }
            let dry_run = args[4..].iter().any(|arg| arg == "--dry-run");
            if args[4..].iter().any(|arg| arg == "--preserve-ids") {
                require_independent_migration_profile(crate::config::has_explicit_data_dir())?;
                let mut store = if dry_run {
                    crate::config::ConfigStore::load_migration_preview()?
                } else {
                    crate::config::ConfigStore::load()?
                };
                let summary = store.import_from_preserving_ids(Path::new(path), dry_run)?;
                let mut value = serde_json::to_value(summary)?;
                value["dry_run"] = json!(dry_run);
                value
            } else {
                runtime.block_on(call_cli(
                    "import_sessions",
                    &json!({
                        "local_path": path, "dry_run": dry_run
                    }),
                ))?
            }
        }
        CliCommand::SyncNative => {
            require_independent_migration_profile(crate::config::has_explicit_data_dir())?;
            let path = args
                .get(3)
                .filter(|arg| !arg.starts_with("--"))
                .ok_or_else(|| {
                    anyhow!("usage: xenterm cli sync-native <sessions.json> [--dry-run] [--json]")
                })?;
            if args[4..]
                .iter()
                .any(|arg| !matches!(arg.as_str(), "--json" | "--dry-run"))
            {
                return Err(anyhow!(
                    "unknown sync-native option (expected --dry-run or --json)"
                ));
            }
            let dry_run = args[4..].iter().any(|arg| arg == "--dry-run");
            let mut store = if dry_run {
                crate::config::ConfigStore::load_migration_preview()?
            } else {
                crate::config::ConfigStore::load()?
            };
            let (updated, added) = store.sync_native_snapshot_preview(Path::new(path), dry_run)?;
            json!({ "updated": updated, "added": added, "dry_run": dry_run })
        }
        CliCommand::Sessions => {
            let group = option_value(args, "--group")?;
            runtime.block_on(call_cli("list_sessions", &json!({ "group": group })))?
        }
        CliCommand::Session => {
            let id = args
                .get(3)
                .ok_or_else(|| anyhow!("usage: xenterm cli session <session-id> [--json]"))?;
            runtime.block_on(call_cli("get_session", &json!({ "session_id": id })))?
        }
        CliCommand::Exec => {
            let id = args.get(3).ok_or_else(|| {
                anyhow!("usage: xenterm cli exec <session-id> [--timeout <seconds>] [--json] -- <command>")
            })?;
            let delimiter = args
                .iter()
                .position(|arg| arg == "--")
                .ok_or_else(|| anyhow!("exec command must follow --"))?;
            let remote_command = args[delimiter + 1..].join(" ");
            if remote_command.trim().is_empty() {
                return Err(anyhow!("remote command must not be empty"));
            }
            let timeout = option_value(&args[..delimiter], "--timeout")?
                .map(|value| value.parse::<u64>())
                .transpose()
                .context("--timeout must be a positive integer")?
                .unwrap_or(30);
            runtime.block_on(call_cli(
                "run_command",
                &json!({
                    "session_id": id,
                    "command": remote_command,
                    "timeout_seconds": timeout
                }),
            ))?
        }
        CliCommand::Files => {
            let id = args
                .get(3)
                .ok_or_else(|| anyhow!("usage: xenterm cli files <session-id> [path] [--json]"))?;
            let path = args
                .get(4)
                .filter(|value| !value.starts_with("--"))
                .map(String::as_str)
                .unwrap_or(".");
            runtime.block_on(call_cli(
                "list_remote_files",
                &json!({ "session_id": id, "path": path }),
            ))?
        }
        CliCommand::Read => {
            let id = args.get(3).ok_or_else(|| {
                anyhow!("usage: xenterm cli read <session-id> <remote-path> [--json]")
            })?;
            let path = args.get(4).ok_or_else(|| {
                anyhow!("usage: xenterm cli read <session-id> <remote-path> [--json]")
            })?;
            runtime.block_on(call_cli(
                "read_remote_text_file",
                &json!({ "session_id": id, "path": path }),
            ))?
        }
        CliCommand::Upload => {
            let id = args.get(3).ok_or_else(|| {
                anyhow!("usage: xenterm cli upload <session-id> <local-path> <remote-directory> [--json]")
            })?;
            let local_path = args.get(4).ok_or_else(|| anyhow!("missing local path"))?;
            let remote_directory = args
                .get(5)
                .ok_or_else(|| anyhow!("missing remote directory"))?;
            runtime.block_on(call_cli(
                "upload_file",
                &json!({
                    "session_id": id,
                    "local_path": local_path,
                    "remote_directory": remote_directory,
                    "timeout_seconds": 120
                }),
            ))?
        }
        CliCommand::Download => {
            let id = args.get(3).ok_or_else(|| {
                anyhow!("usage: xenterm cli download <session-id> <remote-path> <local-directory> [--json]")
            })?;
            let remote_path = args.get(4).ok_or_else(|| anyhow!("missing remote path"))?;
            let local_directory = args
                .get(5)
                .ok_or_else(|| anyhow!("missing local directory"))?;
            runtime.block_on(call_cli(
                "download_file",
                &json!({
                    "session_id": id,
                    "remote_path": remote_path,
                    "local_directory": local_directory,
                    "timeout_seconds": 120
                }),
            ))?
        }
        CliCommand::Permissions => {
            let mode = permissions_mode(&args[3..])?;
            let mut store = crate::config::ConfigStore::load()?;
            if let Some(mode) = mode {
                store.set_mcp_access_mode(mode);
                store.save()?;
            }
            let mut policy = crate::automation::access_policy(&store);
            policy["data_dir"] = json!(crate::config::data_dir());
            policy
        }
        CliCommand::Help => {
            print_help();
            return Ok(());
        }
    };

    let options = local_options(command, args);
    if options.iter().any(|arg| arg == "--json") {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        print_human(command, &value);
    }
    if command == CliCommand::Exec {
        ensure_remote_success(&value)?;
    }
    Ok(())
}

fn permissions_mode(args: &[String]) -> Result<Option<crate::config::McpAccessMode>> {
    let positional: Vec<&str> = args
        .iter()
        .filter(|arg| arg.as_str() != "--json")
        .map(String::as_str)
        .collect();
    match positional.as_slice() {
        [] => Ok(None),
        [value] => crate::config::McpAccessMode::parse(value)
            .map(Some)
            .ok_or_else(|| anyhow!("unknown access mode: {value}; expected restricted or full")),
        _ => Err(anyhow!(
            "usage: xenterm cli permissions [restricted|full] [--json]"
        )),
    }
}

/// A remote program's flags after `--` must never change local CLI behavior.
fn local_options(command: CliCommand, args: &[String]) -> &[String] {
    if command == CliCommand::Exec {
        &args[..args
            .iter()
            .position(|arg| arg == "--")
            .unwrap_or(args.len())]
    } else {
        args
    }
}

/// Preserve the result output for scripts, but make failed remote commands fail
/// locally too. A missing exit status cannot be treated as confirmed success.
fn ensure_remote_success(value: &Value) -> Result<()> {
    anyhow::ensure!(
        value.get("timed_out").and_then(Value::as_bool) != Some(true),
        "remote command timed out"
    );
    anyhow::ensure!(
        value.get("exit_code").and_then(Value::as_i64) == Some(0),
        "remote command failed"
    );
    Ok(())
}

/// Stable identifiers must stay within an independent local-key profile.
fn require_independent_migration_profile(explicit: bool) -> Result<()> {
    anyhow::ensure!(explicit,
        "stable-ID and native snapshot migrations require a separate --data-dir profile with local credential storage; export from the original application and initialize that independent profile with a portable import");
    Ok(())
}

fn option_value<'a>(args: &'a [String], option: &str) -> Result<Option<&'a str>> {
    let Some(index) = args.iter().position(|arg| arg == option) else {
        return Ok(None);
    };
    args.get(index + 1)
        .filter(|value| !value.starts_with("--"))
        .map(|value| Some(value.as_str()))
        .ok_or_else(|| anyhow!("missing value for {option}"))
}

fn print_human(command: CliCommand, value: &Value) {
    match command {
        CliCommand::Export => println!(
            "Exported {} saved sessions",
            value.get("exported").and_then(Value::as_u64).unwrap_or(0)
        ),
        CliCommand::Import => {
            println!(
                "{} {} sessions, skipped {} duplicates",
                if value.get("dry_run").and_then(Value::as_bool) == Some(true) {
                    "Would import"
                } else {
                    "Imported"
                },
                value.get("added").and_then(Value::as_u64).unwrap_or(0),
                value.get("skipped").and_then(Value::as_u64).unwrap_or(0)
            );
            if let Some(warnings) = value.get("warnings").and_then(Value::as_array) {
                for warning in warnings {
                    eprintln!(
                        "Warning ({} entries): {}",
                        warning.get("entries").and_then(Value::as_u64).unwrap_or(0),
                        text(warning, "message")
                    );
                }
            }
        }
        CliCommand::SyncNative => println!(
            "{} {} sessions, {} {} sessions",
            if value.get("dry_run").and_then(Value::as_bool) == Some(true) {
                "Would update"
            } else {
                "Updated"
            },
            value.get("updated").and_then(Value::as_u64).unwrap_or(0),
            if value.get("dry_run").and_then(Value::as_bool) == Some(true) {
                "would add"
            } else {
                "added"
            },
            value.get("added").and_then(Value::as_u64).unwrap_or(0)
        ),
        CliCommand::Sessions => {
            if let Some(sessions) = value.get("sessions").and_then(Value::as_array) {
                for session in sessions {
                    println!(
                        "{}\t{}@{}:{}\t{}",
                        text(session, "id"),
                        text(session, "user"),
                        text(session, "host"),
                        session.get("port").and_then(Value::as_u64).unwrap_or(0),
                        text(session, "name")
                    );
                }
            }
        }
        CliCommand::Permissions | CliCommand::Session => println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_default()
        ),
        CliCommand::Exec => {
            print!("{}", text(value, "stdout"));
            eprint!("{}", text(value, "stderr"));
            if value.get("timed_out").and_then(Value::as_bool) == Some(true) {
                eprintln!("command timed out");
            }
        }
        CliCommand::Files => {
            if let Some(entries) = value.get("entries").and_then(Value::as_array) {
                for entry in entries {
                    println!(
                        "{}\t{}\t{}",
                        if entry.get("is_directory").and_then(Value::as_bool) == Some(true) {
                            "dir"
                        } else {
                            "file"
                        },
                        entry.get("size").and_then(Value::as_u64).unwrap_or(0),
                        text(entry, "path")
                    );
                }
            }
        }
        CliCommand::Read => print!("{}", text(value, "content")),
        CliCommand::Upload | CliCommand::Download => println!(
            "{} {} bytes",
            text(value, "name"),
            value
                .get("transferred")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        ),
        CliCommand::Help => println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_default()
        ),
    }
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

fn print_help() {
    println!(
        "XenTerm CLI\n\n\
         Usage:\n\
           xenterm cli export <new-file.json> --include-credentials [--json]\n\
           xenterm cli import <export.json> [--dry-run] [--preserve-ids] [--json]\n\
           xenterm cli sync-native <sessions.json> [--dry-run] [--json]\n\
           xenterm cli permissions [restricted|full] [--json]\n\
           xenterm cli sessions [--group <name>] [--json]\n\
           xenterm cli session <session-id> [--json]\n\
           xenterm cli exec <session-id> [--timeout <seconds>] [--json] -- <command>\n\
           xenterm cli files <session-id> [path] [--json]\n\
           xenterm cli read <session-id> <remote-path> [--json]\n\
           xenterm cli upload <session-id> <local-path> <remote-directory> [--json]\n\
           xenterm cli download <session-id> <remote-path> <local-directory> [--json]"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissions_arguments_reject_unknown_or_ambiguous_modes() {
        assert_eq!(permissions_mode(&[]).unwrap(), None);
        assert_eq!(permissions_mode(&["--json".into()]).unwrap(), None);
        assert_eq!(
            permissions_mode(&["full".into(), "--json".into()]).unwrap(),
            Some(crate::config::McpAccessMode::Full)
        );
        assert_eq!(
            permissions_mode(&["restricted".into()]).unwrap(),
            Some(crate::config::McpAccessMode::Restricted)
        );
        assert!(permissions_mode(&["anything".into()]).is_err());
        assert!(permissions_mode(&["full".into(), "restricted".into()]).is_err());
        assert_eq!(
            CliCommand::parse(Some("permissions")),
            Some(CliCommand::Permissions)
        );
    }

    #[test]
    fn parses_option_values() {
        let args = vec![
            "xenterm".into(),
            "cli".into(),
            "sessions".into(),
            "--group".into(),
            "prod".into(),
        ];
        assert_eq!(option_value(&args, "--group").unwrap(), Some("prod"));
        assert_eq!(option_value(&args, "--json").unwrap(), None);
        assert_eq!(
            CliCommand::parse(Some("sync-native")),
            Some(CliCommand::SyncNative)
        );
    }

    #[test]
    fn migration_modes_require_explicit_independent_profiles() {
        assert!(require_independent_migration_profile(true).is_ok());
        assert!(require_independent_migration_profile(false)
            .unwrap_err()
            .to_string()
            .contains("--data-dir"));
    }

    #[test]
    fn remote_options_are_not_local_options() {
        let args = [
            "xenterm",
            "cli",
            "exec",
            "fixture",
            "--",
            "command",
            "--json",
            "--timeout",
            "not-a-number",
        ]
        .map(String::from);
        let local = local_options(CliCommand::Exec, &args);
        assert_eq!(local.len(), 4);
        assert!(!local.iter().any(|arg| arg == "--json"));
        assert_eq!(option_value(local, "--timeout").unwrap(), None);
        assert_eq!(local_options(CliCommand::Sessions, &args), args);
    }

    #[test]
    fn only_confirmed_zero_remote_exit_is_success() {
        assert!(ensure_remote_success(&json!({"exit_code": 0, "timed_out": false})).is_ok());
        for value in [
            json!({"exit_code": 17}),
            json!({"exit_code": null}),
            json!({}),
            json!({"exit_code": 0, "timed_out": true}),
        ] {
            assert!(ensure_remote_success(&value).is_err());
        }
    }
}
