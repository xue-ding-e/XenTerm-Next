use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

use crate::config::{ConfigStore, Session};

use super::structs::Frontend;

const DEFAULT_TIMEOUT_SECONDS: u64 = 30;
const MAX_TIMEOUT_SECONDS: u64 = 300;
const DEFAULT_MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

pub(crate) async fn call(name: &str, arguments: &Value, frontend: Frontend) -> Result<Value> {
    let mut record = crate::automation::execution_log::ExecutionGuard::begin(name, arguments, frontend);
    let result = crate::ssh::connection::with_automation_cancellation(call_inner(name, arguments, frontend)).await;
    if let Some(record) = &mut record {
        record.finish(&result);
    }
    result
}

async fn call_inner(name: &str, arguments: &Value, frontend: Frontend) -> Result<Value> {
    match name {
        "get_access_policy" => get_access_policy(arguments, frontend),
        "list_sessions" => list_sessions(arguments, frontend),
        "get_session" => get_session(arguments, frontend),
        "import_sessions" => import_sessions(arguments, frontend),
        "run_command" => run_command(arguments, frontend).await,
        "list_remote_files" => list_remote_files(arguments, frontend).await,
        "read_remote_text_file" => read_remote_text_file(arguments, frontend).await,
        "upload_file" => upload_file(arguments, frontend).await,
        "download_file" => download_file(arguments, frontend).await,
        _ => Err(anyhow!("unknown tool: {name}")),
    }
}

pub(crate) fn access_policy(store: &ConfigStore) -> Value {
    json!({
        "mode": store.mcp_access_mode(),
        "mcp_enabled": store.mcp_enabled(),
        "use_saved_credentials": store.mcp_use_saved_credentials(),
        "allow_commands": store.mcp_allow_commands(),
        "allow_file_transfers": store.mcp_allow_file_transfers(),
        "execution_log_enabled": store.mcp_execution_log_enabled(),
        "execution_log_retention_days": store.mcp_execution_log_retention_days(),
    })
}

fn get_access_policy(arguments: &Value, frontend: Frontend) -> Result<Value> {
    let object = arguments
        .as_object()
        .ok_or_else(|| anyhow!("access policy arguments must be an object"))?;
    if !object.is_empty() {
        return Err(anyhow!("get_access_policy is read-only and accepts no arguments"));
    }
    let store = load_store(frontend)?;
    Ok(access_policy(&store))
}

/// Import is append-only, preview-by-default for protocol callers, and requires
/// a process-level opt-in in addition to the persisted file-transfer gate.
fn import_sessions(arguments: &Value, frontend: Frontend) -> Result<Value> {
    let object = arguments.as_object().ok_or_else(|| anyhow!("import arguments must be an object"))?;
    if object.keys().any(|key| !matches!(key.as_str(), "local_path" | "dry_run")) {
        return Err(anyhow!("unknown import argument"));
    }
    let path = required_string(arguments, "local_path")?;
    if path.trim().is_empty() || path.chars().any(char::is_control) {
        return Err(anyhow!("local_path must be a nonempty valid path"));
    }
    let dry_run = optional_bool(arguments, "dry_run")?.unwrap_or(true);
    if !dry_run && !frontend.allows_config_import() {
        return Err(anyhow!("configuration import is disabled; restart MCP with --allow-config-import to apply imports"));
    }
    let mut store = load_store(frontend)?;
    enforce_transfer_permissions(&store, frontend)?;
    let summary = store.import_from_preview(std::path::Path::new(path), dry_run)?;
    let mut result = serde_json::to_value(summary)?;
    result["dry_run"] = json!(dry_run);
    Ok(result)
}

async fn upload_file(arguments: &Value, frontend: Frontend) -> Result<Value> {
    let store = load_store(frontend)?;
    enforce_transfer_permissions(&store, frontend)?;
    drop(store);
    let (session, jump, timeout) = sftp_context(arguments, frontend)?;
    let local_path = std::path::PathBuf::from(required_string(arguments, "local_path")?);
    if !local_path.is_file() {
        return Err(anyhow!(
            "local upload source is not a regular file: {}",
            local_path.display()
        ));
    }
    let remote_directory = required_string(arguments, "remote_directory")?;
    super::sftp::transfer(
        session,
        jump,
        crate::sftp::SftpCommand::Upload {
            local: local_path,
            remote_dir: remote_directory.to_string(),
            cleanup_after: None,
        },
        true,
        timeout,
    )
    .await
}

async fn download_file(arguments: &Value, frontend: Frontend) -> Result<Value> {
    let store = load_store(frontend)?;
    enforce_transfer_permissions(&store, frontend)?;
    drop(store);
    let (session, jump, timeout) = sftp_context(arguments, frontend)?;
    let remote_path = required_string(arguments, "remote_path")?;
    let local_directory = std::path::PathBuf::from(required_string(arguments, "local_directory")?);
    if !local_directory.is_dir() {
        return Err(anyhow!(
            "local download destination is not an existing directory: {}",
            local_directory.display()
        ));
    }
    let file_name = remote_path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| anyhow!("remote_path must identify a file"))?;
    if local_directory.join(file_name).exists() {
        return Err(anyhow!(
            "download destination already exists: {}",
            local_directory.join(file_name).display()
        ));
    }
    let mut result = super::sftp::transfer(
        session,
        jump,
        crate::sftp::SftpCommand::Download {
            remote: remote_path.to_string(),
            local_dir: local_directory.to_string_lossy().into_owned(),
            conflict: crate::sftp::DownloadConflict::Replace,
        },
        false,
        timeout,
    )
    .await?;
    if let Some(object) = result.as_object_mut() {
        object.insert(
            "local_path".to_string(),
            json!(local_directory.join(file_name).to_string_lossy()),
        );
    }
    Ok(result)
}

fn enforce_transfer_permissions(store: &ConfigStore, frontend: Frontend) -> Result<()> {
    if frontend.is_unattended() && !store.mcp_allow_file_transfers() {
        return Err(anyhow!(
            "file transfers are disabled in Settings > MCP & permissions"
        ));
    }
    Ok(())
}

async fn list_remote_files(arguments: &Value, frontend: Frontend) -> Result<Value> {
    // Reading a remote directory is a file transfer for permission purposes:
    // the same gate as upload/download (audit M-08).
    let store = load_store(frontend)?;
    enforce_transfer_permissions(&store, frontend)?;
    let (session, jump, timeout) = sftp_context(arguments, frontend)?;
    let path = optional_string(arguments, "path")?
        .unwrap_or(".")
        .to_string();
    super::sftp::list(session, jump, path, timeout).await
}

async fn read_remote_text_file(arguments: &Value, frontend: Frontend) -> Result<Value> {
    // Reading remote file contents exfiltrates data just like a download, so
    // it passes the same transfer gate (audit M-08).
    let store = load_store(frontend)?;
    enforce_transfer_permissions(&store, frontend)?;
    let (session, jump, timeout) = sftp_context(arguments, frontend)?;
    let path = required_string(arguments, "path")?;
    if path.trim().is_empty() {
        return Err(anyhow!("path must not be empty"));
    }
    super::sftp::read_text(session, jump, path.to_string(), timeout).await
}

fn sftp_context(
    arguments: &Value,
    frontend: Frontend,
) -> Result<(Session, Vec<Session>, Duration)> {
    let store = load_store(frontend)?;
    if frontend.is_unattended() && !store.mcp_use_saved_credentials() {
        return Err(anyhow!(
            "using saved credentials is disabled in Settings > MCP & permissions"
        ));
    }
    let id = required_string(arguments, "session_id")?;
    let session = store
        .get(id)
        .cloned()
        .ok_or_else(|| anyhow!("session not found: {id}"))?;
    if session.kind.as_str() != "ssh" {
        return Err(anyhow!("SFTP tools only support SSH sessions"));
    }
    let jump = store.resolve_jump_chain(&session)?;
    let timeout = optional_u64(arguments, "timeout_seconds")?
        .unwrap_or(DEFAULT_TIMEOUT_SECONDS)
        .clamp(1, MAX_TIMEOUT_SECONDS);
    Ok((session, jump, Duration::from_secs(timeout)))
}

// The profile may become unreadable after the transport gate passed. Drop the
// underlying error chain for MCP; HTTP formats complete chains for tool errors.
fn config_load_error(error: anyhow::Error, frontend: Frontend) -> anyhow::Error {
    if frontend.is_mcp() {
        anyhow!(crate::automation::MCP_CONFIG_UNAVAILABLE_MESSAGE)
    } else {
        error.context("load XenTerm configuration")
    }
}

fn load_store(frontend: Frontend) -> Result<ConfigStore> {
    let store = ConfigStore::load().map_err(|error| config_load_error(error, frontend))?;
    // The one gate that stays a comparison against `Mcp` rather than going
    // through `is_unattended`. This is not a rule about what a caller may do, it
    // is the MCP server's own on-switch: turning the server off must stop MCP
    // clients and must leave everything else working, including a plugin.
    if frontend.is_mcp() && !store.mcp_enabled() {
        return Err(anyhow!("MCP is disabled in Settings > MCP & permissions"));
    }
    Ok(store)
}

fn list_sessions(arguments: &Value, frontend: Frontend) -> Result<Value> {
    let store = load_store(frontend)?;
    // Session metadata (hosts, users, group layout) is reconnaissance data for
    // an unattended caller, so it sits behind the saved-credentials switch
    // together with everything else that reaches the sessions (audit L-12).
    if frontend.is_unattended() && !store.mcp_use_saved_credentials() {
        return Err(anyhow!(
            "listing sessions is disabled in Settings > MCP & permissions"
        ));
    }
    let group = optional_string(arguments, "group")?;
    let sessions: Vec<Value> = store
        .sessions()
        .iter()
        .filter(|session| group.map_or(true, |group| session.group == group))
        .map(safe_session)
        .collect();
    Ok(json!({ "sessions": sessions }))
}

fn get_session(arguments: &Value, frontend: Frontend) -> Result<Value> {
    let store = load_store(frontend)?;
    if frontend.is_unattended() && !store.mcp_use_saved_credentials() {
        return Err(anyhow!(
            "listing sessions is disabled in Settings > MCP & permissions"
        ));
    }
    let id = required_string(arguments, "session_id")?;
    let session = store
        .get(id)
        .ok_or_else(|| anyhow!("session not found: {id}"))?;
    Ok(safe_session(session))
}

async fn run_command(arguments: &Value, frontend: Frontend) -> Result<Value> {
    let store = load_store(frontend)?;
    if frontend.is_unattended() && !store.mcp_use_saved_credentials() {
        return Err(anyhow!(
            "using saved credentials is disabled in Settings > MCP & permissions"
        ));
    }
    if frontend.is_unattended() && !store.mcp_allow_commands() {
        return Err(anyhow!(
            "arbitrary command execution is disabled in Settings > MCP & permissions"
        ));
    }

    let id = required_string(arguments, "session_id")?;
    let command = required_string(arguments, "command")?;
    if command.trim().is_empty() {
        return Err(anyhow!("command must not be empty"));
    }
    let timeout_seconds = optional_u64(arguments, "timeout_seconds")?
        .unwrap_or(DEFAULT_TIMEOUT_SECONDS)
        .clamp(1, MAX_TIMEOUT_SECONDS);
    let max_output_bytes = optional_u64(arguments, "max_output_bytes")?
        .unwrap_or(DEFAULT_MAX_OUTPUT_BYTES as u64)
        .clamp(1024, MAX_OUTPUT_BYTES as u64) as usize;

    let session = store
        .get(id)
        .cloned()
        .ok_or_else(|| anyhow!("session not found: {id}"))?;
    if session.kind.as_str() != "ssh" {
        return Err(anyhow!("run_command only supports SSH sessions"));
    }
    let jump = store.resolve_jump_chain(&session)?;

    let result = crate::ssh::execute_command(
        session,
        jump,
        command,
        Duration::from_secs(timeout_seconds),
        max_output_bytes,
    )
    .await?;
    serde_json::to_value(result).context("serialize command result")
}

fn safe_session(session: &Session) -> Value {
    json!({
        "id": session.id,
        "name": session.name,
        "kind": session.kind.as_str(),
        "host": session.host,
        "port": session.port,
        "user": session.user,
        "auth": session.auth.as_str(),
        "group": session.group,
        "has_saved_password": !session.password.is_empty(),
        "has_private_key": !session.private_key_path.trim().is_empty()
            || !session.private_key_inline.is_empty(),
        "jump_session_id": session.jump_session_id,
        "jump_session_ids": session.jump_session_ids,
        "has_proxy": !session.proxy.trim().is_empty(),
    })
}

fn required_string<'a>(arguments: &'a Value, key: &str) -> Result<&'a str> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing or invalid string argument: {key}"))
}

fn optional_string<'a>(arguments: &'a Value, key: &str) -> Result<Option<&'a str>> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(Some)
            .ok_or_else(|| anyhow!("invalid string argument: {key}")),
    }
}

fn optional_bool(arguments: &Value, key: &str) -> Result<Option<bool>> {
    match arguments.get(key) {
        None => Ok(None),
        Some(value) => value.as_bool().map(Some).ok_or_else(|| anyhow!("invalid boolean argument: {key}")),
    }
}

fn optional_u64(arguments: &Value, key: &str) -> Result<Option<u64>> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| anyhow!("invalid positive integer argument: {key}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_config_load_error_drops_sensitive_chain_but_cli_keeps_diagnostics() {
        let error = || anyhow!("credential=synthetic-secret").context("private/profile/path");
        let remote = config_load_error(error(), Frontend::Mcp { allow_config_import: false });
        assert_eq!(format!("{remote:#}"), crate::automation::MCP_CONFIG_UNAVAILABLE_MESSAGE);
        let local = config_load_error(error(), Frontend::Cli);
        assert!(format!("{local:#}").contains("synthetic-secret"));
    }

    #[test]
    fn import_arguments_are_strict_before_loading_a_profile() {
        assert_eq!(optional_bool(&json!({}), "dry_run").unwrap(), None);
        assert_eq!(optional_bool(&json!({"dry_run": false}), "dry_run").unwrap(), Some(false));
        for bad in [json!(null), json!("false"), json!(0)] {
            assert!(optional_bool(&json!({"dry_run": bad}), "dry_run").is_err());
        }
        assert!(import_sessions(&json!({"local_path": "fixture", "overwrite": true}), Frontend::Cli).is_err());
        assert!(import_sessions(&json!({"local_path": ""}), Frontend::Cli).is_err());
    }

    #[test]
    fn session_metadata_never_serializes_credential_or_note_fields() {
        let mut session = Session::new_empty();
        let sentinel = "synthetic-secret-redaction-sentinel";
        session.allow_secret_reveal = true;
        session.password = crate::config::Secret::new(sentinel);
        session.private_key_inline = crate::config::Secret::new(sentinel);
        session.private_key_path = sentinel.into();
        session.proxy = format!("socks5://fixture:{sentinel}@127.0.0.1:1080");
        session.note = sentinel.into();
        session.triggers.push(crate::config::SessionTrigger {response: crate::config::Secret::new(sentinel), ..Default::default()});
        let value = safe_session(&session);
        assert!(!value.to_string().contains(sentinel));
        assert_eq!(value["has_saved_password"], true);
        assert_eq!(value["has_private_key"], true);
    }

    #[test]
    fn numeric_arguments_are_strict() {
        assert_eq!(optional_u64(&json!({}), "n").unwrap(), None);
        assert_eq!(optional_u64(&json!({ "n": 12 }), "n").unwrap(), Some(12));
        assert!(optional_u64(&json!({ "n": -1 }), "n").is_err());
        assert!(optional_u64(&json!({ "n": "12" }), "n").is_err());
    }
}
