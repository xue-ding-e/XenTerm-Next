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
    match name {
        "list_sessions" => list_sessions(arguments, frontend),
        "get_session" => get_session(arguments, frontend),
        "run_command" => run_command(arguments, frontend).await,
        "list_remote_files" => list_remote_files(arguments, frontend).await,
        "read_remote_text_file" => read_remote_text_file(arguments, frontend).await,
        "upload_file" => upload_file(arguments, frontend).await,
        "download_file" => download_file(arguments, frontend).await,
        _ => Err(anyhow!("unknown tool: {name}")),
    }
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
            "file transfers are disabled in Settings > Interface > MCP"
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
    // The list side of the risk gate: a watched directory held for a human's
    // yes/no, same as the other file tools.
    if frontend.is_unattended() && store.mcp_approval_enabled() {
        let dirs = store.mcp_risky_dirs();
        let verdict = crate::automation::risk::assess_path(&path, &dirs);
        if verdict.is_risky() {
            let timeout = std::time::Duration::from_secs(store.mcp_approval_timeout_secs());
            let description = format!("{} ({})", session.name, session.host);
            let outcome = crate::automation::approval::request_approval(
                description,
                path.clone(),
                &verdict,
                timeout,
            )
            .await;
            let refusal = match outcome {
                crate::automation::approval::ApprovalOutcome::Approved => None,
                crate::automation::approval::ApprovalOutcome::DeniedByUser => {
                    Some((crate::i18n::t("用户拒绝", "denied by the user"), "denied-by-user"))
                }
                crate::automation::approval::ApprovalOutcome::DeniedByTimeout => Some((
                    crate::i18n::t("无人审批,超时自动拒绝", "auto-denied: nobody approved in time"),
                    "denied-by-timeout",
                )),
                crate::automation::approval::ApprovalOutcome::DeniedByLimit => Some((
                    crate::i18n::t("审批队列已满,自动拒绝", "auto-denied: the approval queue is full"),
                    "denied-by-limit",
                )),
                crate::automation::approval::ApprovalOutcome::DeniedUnreachable => Some((
                    crate::i18n::t("审批不可达,自动拒绝", "auto-denied: the approval flow was unreachable"),
                    "denied-unreachable",
                )),
            };
            if let Some((_who, code)) = refusal {
                return Err(anyhow!(
                    "file operation blocked by the risk-approval gate [{code}]: {path}. {}. {}",
                    verdict.summary(),
                    crate::i18n::t(
                        "不要重试该操作,不要尝试绕过审批,也不要代替用户做审批决定;是否继续由用户确认后另行发起。",
                        "Do not retry it, do not try to route around the approval, and do                          not approve on the user's behalf; continuing is the user's call."
                    )
                ));
            }
        }
    }
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
    let display_path = path.to_string();

    // The file tools' side of the risk gate: a path the user watches (the
    // same directory list run_command's approval uses) is held for a human's
    // yes/no before it is read, listed, written to or pulled from. Without
    // this, `read_remote_text_file /etc/shadow` would be the unapproved twin
    // of `cat /etc/shadow`.
    if frontend.is_unattended() && store.mcp_approval_enabled() {
        let dirs = store.mcp_risky_dirs();
        let verdict = crate::automation::risk::assess_path(
            &display_path,
            &dirs,
        );
        if verdict.is_risky() {
            let timeout = std::time::Duration::from_secs(store.mcp_approval_timeout_secs());
            let description = format!("{} ({})", session.name, session.host);
            let outcome = crate::automation::approval::request_approval(
                description,
                display_path.clone(),
                &verdict,
                timeout,
            )
            .await;
            let refusal = match outcome {
                crate::automation::approval::ApprovalOutcome::Approved => None,
                crate::automation::approval::ApprovalOutcome::DeniedByUser => {
                    Some((crate::i18n::t("用户拒绝", "denied by the user"), "denied-by-user"))
                }
                crate::automation::approval::ApprovalOutcome::DeniedByTimeout => Some((
                    crate::i18n::t("无人审批,超时自动拒绝", "auto-denied: nobody approved in time"),
                    "denied-by-timeout",
                )),
                crate::automation::approval::ApprovalOutcome::DeniedByLimit => Some((
                    crate::i18n::t("审批队列已满,自动拒绝", "auto-denied: the approval queue is full"),
                    "denied-by-limit",
                )),
                crate::automation::approval::ApprovalOutcome::DeniedUnreachable => Some((
                    crate::i18n::t("审批不可达,自动拒绝", "auto-denied: the approval flow was unreachable"),
                    "denied-unreachable",
                )),
            };
            if let Some((_who, code)) = refusal {
                return Err(anyhow!(
                    "file operation blocked by the risk-approval gate [{code}]: {display_path}. {}. {}",
                    verdict.summary(),
                    crate::i18n::t(
                        "不要重试该操作,不要尝试绕过审批,也不要代替用户做审批决定;是否继续由用户确认后另行发起。",
                        "Do not retry it, do not try to route around the approval, and do                          not approve on the user's behalf; continuing is the user's call."
                    ),
                ));
            }
        }
    }
    super::sftp::read_text(session, jump, path.to_string(), timeout).await
}

fn sftp_context(
    arguments: &Value,
    frontend: Frontend,
) -> Result<(Session, Option<Session>, Duration)> {
    let store = load_store(frontend)?;
    if frontend.is_unattended() && !store.mcp_use_saved_credentials() {
        return Err(anyhow!(
            "using saved credentials is disabled in Settings > Interface > MCP"
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
    let jump = if session.jump_session_id.trim().is_empty() {
        None
    } else {
        Some(
            store
                .get(&session.jump_session_id)
                .cloned()
                .ok_or_else(|| anyhow!("jump session not found: {}", session.jump_session_id))?,
        )
    };
    let timeout = optional_u64(arguments, "timeout_seconds")?
        .unwrap_or(DEFAULT_TIMEOUT_SECONDS)
        .clamp(1, MAX_TIMEOUT_SECONDS);
    Ok((session, jump, Duration::from_secs(timeout)))
}

fn load_store(frontend: Frontend) -> Result<ConfigStore> {
    let store = ConfigStore::load().context("load XenTerm configuration")?;
    // The one gate that stays a comparison against `Mcp` rather than going
    // through `is_unattended`. This is not a rule about what a caller may do, it
    // is the MCP server's own on-switch: turning the server off must stop MCP
    // clients and must leave everything else working, including a plugin.
    if frontend == Frontend::Mcp && !store.mcp_enabled() {
        return Err(anyhow!("MCP is disabled in Settings > Interface > MCP"));
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
            "listing sessions is disabled in Settings > Interface > MCP"
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
            "listing sessions is disabled in Settings > Interface > MCP"
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
            "using saved credentials is disabled in Settings > Interface > MCP"
        ));
    }
    if frontend.is_unattended() && !store.mcp_allow_commands() {
        return Err(anyhow!(
            "arbitrary command execution is disabled in Settings > Interface > MCP"
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
    let jump = if session.jump_session_id.trim().is_empty() {
        None
    } else {
        Some(
            store
                .get(&session.jump_session_id)
                .cloned()
                .ok_or_else(|| anyhow!("jump session not found: {}", session.jump_session_id))?,
        )
    };

    // The human gate. A caller nobody is watching decided to run this; if the
    // command trips the risk lists, someone at the main window gets to say so
    // before it runs — and if nobody is there, the answer is no.
    if frontend.is_unattended() && store.mcp_approval_enabled() {
        let verdict = crate::automation::risk::assess(
            &command,
            &store.mcp_risky_patterns(),
            &store.mcp_risky_dirs(),
        );
        if verdict.is_risky() {
            let timeout = std::time::Duration::from_secs(store.mcp_approval_timeout_secs());
            let description = format!("{} ({})", session.name, session.host);
            let outcome = crate::automation::approval::request_approval(
                description,
                command.to_string(),
                &verdict,
                timeout,
            )
            .await;
            // Each denial says who said no, because the caller is expected to
            // treat them differently: a user's denial is a decision to hand
            // back, a timeout is an absence to report — and neither is an
            // invitation to retry, sneak the command through another path, or
            // answer the approval prompt on the user's behalf.
            let refusal = match outcome {
                crate::automation::approval::ApprovalOutcome::Approved => None,
                crate::automation::approval::ApprovalOutcome::DeniedByUser => {
                    Some((crate::i18n::t("用户拒绝", "denied by the user"), "denied-by-user"))
                }
                crate::automation::approval::ApprovalOutcome::DeniedByTimeout => Some((
                    crate::i18n::t("无人审批,超时自动拒绝", "auto-denied: nobody approved in time"),
                    "denied-by-timeout",
                )),
                crate::automation::approval::ApprovalOutcome::DeniedByLimit => Some((
                    crate::i18n::t("审批队列已满,自动拒绝", "auto-denied: the approval queue is full"),
                    "denied-by-limit",
                )),
                crate::automation::approval::ApprovalOutcome::DeniedUnreachable => Some((
                    crate::i18n::t("审批不可达,自动拒绝", "auto-denied: the approval flow was unreachable"),
                    "denied-unreachable",
                )),
            };
            if let Some((who, code)) = refusal {
                return Err(anyhow!(
                    "command blocked by the risk-approval gate [{code}]: {}. {}. {}",
                    verdict.summary(),
                    who,
                    crate::i18n::t(
                        "不要重试该命令,不要尝试绕过审批,也不要代替用户做审批决定;是否继续由用户确认后另行发起。",
                        "Do not retry it, do not try to route around the approval, and do                          not approve on the user's behalf; continuing is the user's call, made                          from the terminal themselves."
                    ),
                ));
            }
        }
    }

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
    fn numeric_arguments_are_strict() {
        assert_eq!(optional_u64(&json!({}), "n").unwrap(), None);
        assert_eq!(optional_u64(&json!({ "n": 12 }), "n").unwrap(), Some(12));
        assert!(optional_u64(&json!({ "n": -1 }), "n").is_err());
        assert!(optional_u64(&json!({ "n": "12" }), "n").is_err());
    }
}
