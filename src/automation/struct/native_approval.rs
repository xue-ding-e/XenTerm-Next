//! A visible confirmation window for Windows headless MCP processes.
//! Commands are sent as JSON data, never interpolated into PowerShell code.

use std::io::{self, Write};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Once;
use std::time::Duration;

use fs2::FileExt;

use super::approval::{self, ApprovalRequest};

pub(super) fn ensure_worker() {
    static START: Once = Once::new();
    START.call_once(|| {
        if let Err(error) = std::thread::Builder::new()
            .name("mcp-native-approval".into())
            .spawn(run)
        {
            tracing::warn!("could not start native approval presenter: {error}");
        }
    });
}

fn run() {
    let queue = approval::queue_dir();
    loop {
        if let Err(error) = present_next(&queue) {
            tracing::warn!("native approval presenter: {error}");
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn present_next(queue: &Path) -> io::Result<()> {
    let requests = approval::pending_requests_in(queue);
    let Some(request) = requests.first() else {
        return Ok(());
    };
    // All stdio processes share one presenter; another process can still
    // resolve the request through the same audited decision lock.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(queue.join(".native-presenter.lock"))?;
    if lock.try_lock_exclusive().is_err() {
        return Ok(());
    }
    let Some(current) = approval::request_state_in(queue, &request.id) else {
        return Ok(());
    };
    if current.status != "pending" || approval::past_deadline(&current) {
        return Ok(());
    }
    if let Err(error) = show(queue, &current) {
        approval::resolve_unreachable_in(queue, &current.id);
        return Err(error);
    }
    Ok(())
}

fn show(queue: &Path, request: &ApprovalRequest) -> io::Result<()> {
    let companion = std::env::current_exe()?
        .parent()
        .ok_or_else(|| io::Error::other("approval executable has no parent"))?
        .join("xenterm-approval.exe");
    if companion.is_file() {
        match show_gpui(queue, request, &companion) {
            Ok(()) => return Ok(()),
            Err(error) => {
                let still_pending = approval::request_state_in(queue, &request.id)
                    .is_some_and(|r| r.status == "pending" && !approval::past_deadline(&r));
                if !still_pending {
                    return Ok(());
                }
                tracing::warn!("GPUI approval unavailable, using native fallback: {error}");
            }
        }
    }
    let system_root = std::env::var_os("SystemRoot")
        .ok_or_else(|| io::Error::other("SystemRoot is unavailable"))?;
    let executable = Path::new(&system_root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let mut child = Command::new(executable)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-STA",
            "-Command",
            include_str!("native_approval.ps1"),
        ])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW hides only the console.
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()?;
    let mut input = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("missing prompt input"))?;
    let mut body = serde_json::to_value(request).map_err(io::Error::other)?;
    if let Ok(store) = crate::config::ConfigStore::load() {
        body["theme"] = serde_json::json!(store.theme_pref());
        body["language"] = serde_json::json!(store.language());
    }
    let body = serde_json::to_vec(&body).map_err(io::Error::other)?;
    if let Err(error) = input.write_all(&body) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    drop(input);
    loop {
        if let Some(status) = child.try_wait()? {
            match status.code() {
                Some(0) => {
                    approval::resolve_manual_in(queue, &request.id, true);
                }
                Some(1) => {
                    approval::resolve_manual_in(queue, &request.id, false);
                }
                _ => {
                    return Err(io::Error::other(
                        "native confirmation window could not be displayed",
                    ))
                }
            }
            return Ok(());
        }
        // Close an obsolete prompt if a desktop window answered first, the
        // requester went away, or the same decision deadline has passed.
        let pending = approval::request_state_in(queue, &request.id)
            .is_some_and(|r| r.status == "pending" && !approval::past_deadline(&r));
        if !pending {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn show_gpui(queue: &Path, request: &ApprovalRequest, executable: &Path) -> io::Result<()> {
    let profile = queue
        .parent()
        .ok_or_else(|| io::Error::other("missing approval profile"))?;
    let mut child = Command::new(executable)
        .arg("--data-dir")
        .arg(profile)
        .arg("approval-ui")
        .arg(&request.id)
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    loop {
        let current = approval::request_state_in(queue, &request.id);
        if current
            .as_ref()
            .is_none_or(|r| r.status != "pending" || approval::past_deadline(r))
        {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(());
        }
        if child.try_wait()?.is_some() {
            // Process exit is never an approval. Only the audited queue decision
            // written by the GPUI buttons can authorize the waiting command.
            let decided = approval::request_state_in(queue, &request.id)
                .is_none_or(|r| r.status != "pending");
            return if decided {
                Ok(())
            } else {
                Err(io::Error::other(
                    "GPUI approval window exited without a decision",
                ))
            };
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
