//! The session protocol: the vocabulary a running session and whatever drives it
//! exchange, plus the routes a tab's events travel over.
//!
//! This is not an SSH implementation detail, which is why it is not in
//! `crate::ssh`. `crate::terminal` (local, telnet, serial, zmodem), `crate::sftp`,
//! `crate::tunnel`, `crate::core`, `crate::app`, `crate::automation` and
//! `crate::ui` all speak it, and every one of them used to depend on the SSH
//! module merely to name a message type. Moving the vocabulary here leaves
//! `crate::ssh` as one consumer among many.
//!
//! The file is deliberately a leaf: its only crate-internal dependencies are
//! `crate::config`, `crate::core`, `crate::sftp` and `crate::terminal`, none of
//! which reach back into it, so no cycle can pass through this module. That
//! property is what lets the terminal layer describe a session without adopting
//! the SSH layer, and it is asserted in `crate::arch_guards`.
//!
//! The types arrived from `src/ssh/struct/{command,event,responders,remote_fs,system}.rs`
//! and from `crate::app::core` (the tab route), which is where the first session
//! implementation kept them. Nothing here behaves differently for the move.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use crate::config::{PortForward, Secret};
use crate::core::EventSink;
use crate::sftp::{SftpHandles, SftpLastCwd};
use crate::terminal::TermBuffers;

// --- Commands: what a driver sends into a running session ---------------------

/// Commands posted to the worker task by whoever drives the session.
#[derive(Debug)]
pub enum SessionCommand {
    /// Send raw bytes directly to the PTY (individual keystrokes, no modification).
    RawInput(Vec<u8>),
    /// Notify the remote PTY of a terminal resize.
    Resize(u32, u32),
    /// Start or pause periodic local/remote resource monitoring for this session.
    SetResourceMonitoring(bool),
    /// Start a runtime-only SSH tunnel for this connected session (#206).
    AddTunnel { id: String, forward: PortForward },
    /// Stop a runtime tunnel created for this connected session (#206).
    StopTunnel(String),
    /// Terminate one remote process on a short-lived exec channel. Supplying a
    /// password selects the privileged `sudo -S` path; the secret is never
    /// written to the interactive PTY or shell history.
    KillProcess {
        pid: u32,
        root_password: Option<Secret>,
        reply: tokio::sync::oneshot::Sender<ProcessKillResult>,
    },
    /// Gracefully disconnect and drop the session.
    Close,
}

#[derive(Debug)]
pub struct ProcessKillResult {
    pub success: bool,
    pub message: String,
}

// --- Events: what a running session reports back ------------------------------

/// Events emitted back to the UI thread.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// Free-form status text for the tab header / status line.
    Status(String),
    /// A chunk of stdout/stderr output from the remote shell.
    Output(String),
    /// Connection is up.
    Connected,
    /// Connection closed (either cleanly or after an error).
    Closed(String),
    /// The server presented a host key that is unknown or has changed; the UI
    /// must show a confirmation dialog and answer via `responder` (#109-5). The
    /// handler is blocked awaiting that answer.
    HostKeyPrompt {
        host: String,
        port: u16,
        key_type: String,
        fingerprint: String,
        /// True when a *different* key was previously stored (possible MITM).
        changed: bool,
        responder: HostKeyResponder,
    },
    /// The session is missing a username and/or password; the UI must prompt for
    /// them and answer via `responder`. The auth flow is blocked meanwhile (#110).
    CredentialPrompt {
        session_id: String,
        host: String,
        user: String,
        need_user: bool,
        need_password: bool,
        responder: CredentialResponder,
    },
    /// A keyboard-interactive challenge that isn't the account password —
    /// typically an MFA / OTP / verification-code prompt from a bastion such as
    /// JumpServer. The UI shows `prompt` and answers via `responder`; the auth
    /// flow is blocked meanwhile (#86-MFA).
    MfaPrompt {
        session_id: String,
        host: String,
        /// The server's prompt text, e.g. "MFA code: " / "Verification code:".
        prompt: String,
        /// Whether typed input should be visible (false = hide, like a password).
        echo: bool,
        responder: MfaResponder,
    },
    /// Remote machine resource sample (from the monitor channel).
    /// Memory/swap are in KiB (as reported by /proc/meminfo).
    ResourceStats {
        cpu_percent: f32,
        mem_used_kib: u64,
        mem_total_kib: u64,
        swap_used_kib: u64,
        swap_total_kib: u64,
        /// Per-interface (name, rx_bytes_per_sec, tx_bytes_per_sec).
        net: Vec<(String, u64, u64)>,
        /// Per-filesystem (mount_point, available_bytes, total_bytes).
        disks: Vec<(String, u64, u64)>,
        /// Effective login name reported by the remote host (`id -un`).
        current_user: String,
        /// Top processes by CPU (#23). Empty if the host's `ps` is unusable.
        procs: Vec<ProcInfo>,
        /// Detailed system information for the detached system-info window.
        /// Detailed data is present only for the separately delayed one-shot
        /// system-information probe; lightweight resource samples leave it None.
        sys: Option<SystemDetails>,
    },

    /// Effective user and top-process snapshot from the dedicated lightweight
    /// process channel. Keeping this separate prevents a slow `df`, `lspci`, or
    /// other system-information probe from freezing the process window.
    ProcessStats {
        current_user: String,
        procs: Vec<ProcInfo>,
    },

    /// A command the user ran in the terminal, captured via the shell hook
    /// (OSC 697) so it can join the command-box history (#113).
    CommandRan(String),

    /// Runtime tunnel state changed (#206).
    TunnelUpdate(Vec<RuntimeTunnelInfo>),

    // --- SFTP events -------------------------------------------------------
    /// The shell's current working directory changed (parsed from OSC 7).
    CwdChanged(String),
    /// SFTP directory listing arrived.
    SftpEntries {
        path: String,
        entries: Vec<RemoteEntry>,
    },
    /// Free-form SFTP status message (progress, errors, etc.).
    SftpStatus(String),
    /// A directory listing failed (e.g. permission denied): show the message and
    /// stop the panel's loading spinner without disturbing the current view (#112).
    SftpError(String),
    /// Directory tree structure changed (full rebuild pushed on every toggle).
    SftpTreeUpdate(Vec<RemoteTreeNode>),
    /// File-transfer progress / completion (download or upload).
    SftpTransfer {
        id: String,
        name: String,
        is_upload: bool,
        transferred: u64,
        total: u64,
        state: u8, // 0 = active, 1 = done, 2 = error
        msg: String,
    },
    /// A remote text file loaded for the built-in viewer/editor (#70). On
    /// failure (too large, binary, non-UTF-8, I/O error) `error` is non-empty
    /// and `content` is empty.
    SftpFileText {
        path: String,
        name: String,
        content: String,
        edit: bool,
        error: String,
    },
}

// --- The handle a driver holds ------------------------------------------------

/// Handle retained by the driver to talk to a running session.
pub struct SessionHandle {
    #[allow(dead_code)] // used by future resize / reconnect flows
    pub tab_id: String,
    pub commands: UnboundedSender<SessionCommand>,
    #[allow(dead_code)] // keep alive; detach on Drop is fine for v0.1
    pub join: JoinHandle<()>,
}

impl SessionHandle {
    pub fn send_raw(&self, bytes: Vec<u8>) {
        let _ = self.commands.send(SessionCommand::RawInput(bytes));
    }

    pub fn resize(&self, cols: u32, rows: u32) {
        let _ = self.commands.send(SessionCommand::Resize(cols, rows));
    }

    pub fn set_resource_monitoring(&self, enabled: bool) {
        let _ = self
            .commands
            .send(SessionCommand::SetResourceMonitoring(enabled));
    }

    pub fn kill_process(
        &self,
        pid: u32,
        root_password: Option<Secret>,
    ) -> tokio::sync::oneshot::Receiver<ProcessKillResult> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let _ = self.commands.send(SessionCommand::KillProcess {
            pid,
            root_password,
            reply,
        });
        rx
    }

    pub fn close(&self) {
        let _ = self.commands.send(SessionCommand::Close);
    }
}

// --- Blocking prompts: how an answer gets back to a paused session ------------

/// Carries the user's answer to a host-key confirmation prompt back to the
/// blocked `check_server_key` handler. Wrapped in `Arc<Mutex<Option<…>>>` so the
/// enclosing [`SessionEvent`] stays `Clone` (a bare `oneshot::Sender` is not);
/// the first `respond` consumes the sender, later calls are no-ops.
#[derive(Clone)]
pub struct HostKeyResponder(Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<bool>>>>);

impl HostKeyResponder {
    pub fn new(tx: tokio::sync::oneshot::Sender<bool>) -> Self {
        Self(Arc::new(std::sync::Mutex::new(Some(tx))))
    }

    /// Deliver the user's decision (`true` = trust). Idempotent.
    pub fn respond(&self, accept: bool) {
        if let Ok(mut guard) = self.0.lock() {
            if let Some(tx) = guard.take() {
                let _ = tx.send(accept);
            }
        }
    }
}

impl std::fmt::Debug for HostKeyResponder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostKeyResponder")
    }
}

/// The user's answer to a connect-time credential prompt: `(username, password,
/// remember)`, or `None` if they cancelled.
pub type CredentialReply = (String, String, bool);

/// Carries the credential prompt's answer back to the blocked auth flow (#110).
/// `Arc<Mutex<Option<…>>>` so the enclosing [`SessionEvent`] stays `Clone`.
#[derive(Clone)]
pub struct CredentialResponder(
    Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<Option<CredentialReply>>>>>,
);

impl CredentialResponder {
    pub fn new(tx: tokio::sync::oneshot::Sender<Option<CredentialReply>>) -> Self {
        Self(Arc::new(std::sync::Mutex::new(Some(tx))))
    }

    /// Deliver the user's answer (`None` = cancelled). Idempotent.
    pub fn respond(&self, reply: Option<CredentialReply>) {
        if let Ok(mut guard) = self.0.lock() {
            if let Some(tx) = guard.take() {
                let _ = tx.send(reply);
            }
        }
    }
}

impl std::fmt::Debug for CredentialResponder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CredentialResponder")
    }
}

/// Carries the answer to a keyboard-interactive (MFA / verification-code) prompt
/// back to the blocked auth flow (#86-MFA). `None` = the user cancelled.
/// `Arc<Mutex<Option<…>>>` so the enclosing [`SessionEvent`] stays `Clone`.
#[derive(Clone)]
pub struct MfaResponder(Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<Option<String>>>>>);

impl MfaResponder {
    pub fn new(tx: tokio::sync::oneshot::Sender<Option<String>>) -> Self {
        Self(Arc::new(std::sync::Mutex::new(Some(tx))))
    }

    /// Deliver the user's answer (`None` = cancelled). Idempotent.
    pub fn respond(&self, reply: Option<String>) {
        if let Ok(mut guard) = self.0.lock() {
            if let Some(tx) = guard.take() {
                let _ = tx.send(reply);
            }
        }
    }
}

impl std::fmt::Debug for MfaResponder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MfaResponder")
    }
}

// --- The payloads those events carry -----------------------------------------

/// Metadata for a single remote filesystem entry returned by SFTP listing.
#[derive(Debug, Clone)]
pub struct RemoteEntry {
    pub name: String,
    pub full_path: String,
    pub is_dir: bool,
    /// Raw size in bytes (0 for directories or unknown).
    pub size: u64,
    /// Modification time as Unix timestamp (seconds, u32 = SFTP wire format).
    pub modified: u32,
    /// POSIX permission bits (the low 12, i.e. rwx + setuid/setgid/sticky).
    /// 0 when the server didn't report permissions. Used to prefill the chmod
    /// dialog (#84).
    pub mode: u32,
}

/// One node in the remote directory tree panel.
#[derive(Debug, Clone)]
pub struct RemoteTreeNode {
    pub path: String,
    pub name: String,
    pub depth: u32,
    pub expanded: bool,
    pub has_children: bool,
}

/// One process row sampled from the remote `ps` (#23). CPU/mem are percentages
/// as reported by `ps` (pcpu/pmem); `command` is the (width-truncated) args.
#[derive(Debug, Clone)]
pub struct ProcInfo {
    pub pid: u32,
    pub user: String,
    pub cpu: f32,
    pub mem: f32,
    pub command: String,
}

/// The detailed probe's tables, as the system-information window reads them.
///
/// `PartialEq` because a window that polls this store has to be able to tell a new
/// sample from the one it is already drawing: repainting seven tables every second to
/// show the same strings is the cost that comparison avoids.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SystemDetails {
    pub overview: Vec<(String, String)>,
    pub cpu_info: Vec<(String, String)>,
    pub gpu_info: Vec<(String, String)>,
    pub cpu_usage: Vec<(String, String)>,
    pub memory: Vec<(String, String)>,
    pub swap: Vec<(String, String)>,
    pub networks: Vec<(String, String, String, String, String)>,
    pub filesystems: Vec<(String, String, String, String, String)>,
}

/// One SSH tunnel row shown in the runtime tunnel panel (#206).
#[derive(Debug, Clone)]
pub struct RuntimeTunnelInfo {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub bind_addr: String,
    pub bind_port: u16,
    pub host: String,
    pub host_port: u16,
    pub active: bool,
    pub status: String,
}

// --- The route a tab's events travel over -------------------------------------

/// Where a tab's session events are currently delivered. Session pump
/// threads hold an `Arc<Mutex<TabRoute>>` per tab and re-read it on every
/// batch, so moving a tab to another window is just rewriting this struct —
/// the pumps keep running and immediately target the new window.
///
/// This used to live in `crate::app::core`, which put it *above* the connect path
/// that now carries it: `ConnectCtx` named `crate::app`, and `app` named this
/// crate's session context, so the two modules depended on each other. A route is
/// part of the protocol — it is where the protocol's events go — so it lives with
/// the rest of it, and both sides now depend downward on one leaf.
///
/// Names no toolkit type. The window is reached through `sink`.
///
/// Every field is thread-safe (the pumps run off the UI thread).
#[derive(Clone)]
pub struct TabRoute {
    /// This window's event destination. Replaced wholesale on detach/merge,
    /// which is all retargeting a running pump takes.
    pub sink: Arc<dyn EventSink>,
    /// Registry id of the window `sink` belongs to. Not derivable from the sink
    /// at the point it is needed: a registry that drops every route whose window
    /// closed has, by then, already dropped the window state — and with it the
    /// `Arc` a sink would be compared against.
    ///
    /// Kept although nothing reads it in this tree: the detach/merge path that
    /// needs it is not implemented in this shell, and this field is what that path
    /// will key on. It is written where the route is built, so the writer stays
    /// honest while the reader is still missing.
    #[allow(dead_code)]
    pub window_id: u64,
    /// The pump ingests terminal output into this directly, off the UI thread
    /// and mid-batch, rather than handing it to the sink.
    pub bufs: TermBuffers,
    pub sftp_handles: SftpHandles,
    pub sftp_last_cwd: SftpLastCwd,
    pub follow_cd: Arc<std::sync::atomic::AtomicBool>,
}

/// Tab id → its current delivery route. Shared with the pump threads.
pub type TabRoutes = Arc<std::sync::Mutex<HashMap<String, Arc<std::sync::Mutex<TabRoute>>>>>;
