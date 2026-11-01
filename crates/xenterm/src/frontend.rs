//! Public façade for in-process frontends (the Flutter bridge).
//!
//! This is the only public surface over the session runtime and the rio-vt
//! terminal engine: frontends start a session by id, read a compact screen
//! snapshot, send input, resize, answer connect-time prompts and close it.
//! Everything below stays private so the UI kit never leaks into the core.
//!
//! The pump thread owns the session's event receiver: output is fed straight
//! into the engine (rio-vt), and a version counter lets a frontend poll cheaply
//! and only pay for a snapshot when the screen actually changed.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{ConfigStore, SessionKind};
use crate::session::protocol::{
    CredentialReply, CredentialResponder, HostKeyResponder, MfaResponder, ResourceMonitorState,
    RuntimeTunnelInfo, SessionEvent, SessionHandle,
};
use crate::terminal::{TerminalCell, TerminalColor, TerminalEngine};
use serde_json::{json, Value};

/// One pending blocking prompt (host key / credentials / MFA), answered by the
/// frontend through [`TerminalSession::answer_prompt`].
enum PendingPrompt {
    HostKey {
        host: String,
        port: u16,
        key_type: String,
        fingerprint: String,
        changed: bool,
        responder: HostKeyResponder,
    },
    Credential {
        host: String,
        user: String,
        need_user: bool,
        need_password: bool,
        responder: CredentialResponder,
    },
    Mfa {
        prompt: String,
        echo: bool,
        responder: MfaResponder,
    },
}

struct PromptEntry {
    id: u64,
    prompt: PendingPrompt,
}

struct SessionLog {
    file: File,
}

impl SessionLog {
    fn start(session: &crate::config::Session) -> Option<Self> {
        let dir = crate::config::data_dir().join("session_logs");
        std::fs::create_dir_all(&dir).ok()?;
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_millis();
        let safe: String = session
            .id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let path = dir.join(format!("{started}-{safe}.jsonl"));
        let mut file = File::create(&path).ok()?;
        let meta = json!({
            "kind": "meta",
            "session_id": session.id,
            "name": session.name,
            "host": session.host,
            "user": session.user,
            "port": session.port,
            "started_ms": started as u64,
        });
        writeln!(file, "{meta}").ok()?;
        Some(Self { file })
    }

    fn output(&mut self, text: &str) {
        let line = json!({"kind": "output", "text": text});
        let _ = writeln!(self.file, "{line}");
    }

    fn finish(&mut self, reason: Option<&str>) {
        let ended = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let _ = writeln!(
            self.file,
            "{}",
            json!({"kind": "end", "ended_ms": ended, "reason": reason})
        );
        let _ = self.file.flush();
    }
}

pub struct TerminalSession {
    handle: SessionHandle,
    engine: Mutex<TerminalEngine>,
    version: AtomicU64,
    status: Mutex<String>,
    closed: Mutex<Option<String>>,
    prompt: Mutex<Option<PromptEntry>>,
    prompt_seq: AtomicU64,
    log: Mutex<Option<SessionLog>>,
    /// Latest remote resource sample (SSH monitor channel) and its lifecycle.
    resource: Mutex<Option<Value>>,
    resource_state: Mutex<String>,
    /// Detected operating system for the host card (`debian`, `windows`, …).
    remote_os: Mutex<String>,
    /// Latest tunnel runtime state for this session's port forwards.
    tunnels: Mutex<Vec<RuntimeTunnelInfo>>,
}

fn local_os_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

fn resource_state_name(state: ResourceMonitorState) -> &'static str {
    match state {
        ResourceMonitorState::Waiting => "waiting",
        ResourceMonitorState::Available => "available",
        ResourceMonitorState::Unavailable => "unavailable",
        ResourceMonitorState::Stale => "stale",
        ResourceMonitorState::Paused => "paused",
        ResourceMonitorState::Unsupported => "unsupported",
    }
}

fn color_json(color: TerminalColor) -> Value {
    match color {
        TerminalColor::Default => Value::Null,
        TerminalColor::Idx(index) => json!({ "idx": index }),
        TerminalColor::Rgb(r, g, b) => json!({ "rgb": [r, g, b] }),
    }
}

fn style_json(cell: &TerminalCell) -> Value {
    json!({
        "fg": color_json(cell.fgcolor()),
        "bg": color_json(cell.bgcolor()),
        "b": cell.bold(),
        "i": cell.inverse(),
    })
}

fn pending_prompt_json(entry: &PromptEntry) -> Value {
    match &entry.prompt {
        PendingPrompt::HostKey {
            host,
            port,
            key_type,
            fingerprint,
            changed,
            ..
        } => json!({
            "id": entry.id,
            "kind": "host_key",
            "host": host,
            "port": port,
            "key_type": key_type,
            "fingerprint": fingerprint,
            "changed": changed,
        }),
        PendingPrompt::Credential {
            host,
            user,
            need_user,
            need_password,
            ..
        } => json!({
            "id": entry.id,
            "kind": "credential",
            "host": host,
            "user": user,
            "need_user": need_user,
            "need_password": need_password,
        }),
        PendingPrompt::Mfa { prompt, echo, .. } => json!({
            "id": entry.id,
            "kind": "mfa",
            "prompt": prompt,
            "echo": echo,
        }),
    }
}

impl TerminalSession {
    /// Looks the session up in the store, spawns its transport and starts the
    /// pump thread that feeds rio-vt. `scrollback` is in lines.
    pub fn start(
        runtime: &tokio::runtime::Handle,
        store: &ConfigStore,
        session_id: &str,
        cols: u16,
        rows: u16,
        scrollback: usize,
    ) -> Result<Arc<TerminalSession>, String> {
        let session = store
            .sessions()
            .iter()
            .find(|s| s.id == session_id)
            .cloned()
            .ok_or_else(|| format!("session {session_id} not found"))?;
        let jump = store
            .resolve_jump_chain(&session)
            .map_err(|error| error.to_string())?;
        let log = if store.session_logs() {
            SessionLog::start(&session)
        } else {
            None
        };
        Self::launch(runtime, session, jump, cols, rows, scrollback, log)
    }

    /// Starts an in-memory session that is not in the config store. Used by
    /// the local-terminal and serial quick-connect entries, which are meant
    /// to be temporary tabs rather than saved hosts.
    pub fn start_adhoc(
        runtime: &tokio::runtime::Handle,
        session: crate::config::Session,
        log_enabled: bool,
        cols: u16,
        rows: u16,
        scrollback: usize,
    ) -> Result<Arc<TerminalSession>, String> {
        let log = if log_enabled {
            SessionLog::start(&session)
        } else {
            None
        };
        Self::launch(runtime, session, Vec::new(), cols, rows, scrollback, log)
    }

    fn launch(
        runtime: &tokio::runtime::Handle,
        session: crate::config::Session,
        jump: Vec<crate::config::Session>,
        cols: u16,
        rows: u16,
        scrollback: usize,
        log: Option<SessionLog>,
    ) -> Result<Arc<TerminalSession>, String> {
        let tab_id = session.id.clone();
        let is_local = session.kind == SessionKind::Local;
        let (handle, mut events) = match session.kind {
            SessionKind::Ssh => {
                crate::ssh::spawn_session(runtime, tab_id, session, jump, cols as u32, rows as u32)
            }
            SessionKind::Serial => {
                crate::terminal::serial::spawn_serial_session(runtime, session.id.clone(), session)
            }
            SessionKind::Telnet => crate::terminal::telnet::spawn_telnet_session(
                runtime,
                tab_id,
                session,
                cols as u32,
                rows as u32,
            ),
            SessionKind::Local => crate::terminal::local::spawn_local_session(
                runtime,
                tab_id,
                session,
                cols as u32,
                rows as u32,
            ),
        };

        let session = Arc::new(TerminalSession {
            handle,
            engine: Mutex::new(TerminalEngine::new(rows, cols, scrollback)),
            version: AtomicU64::new(1),
            status: Mutex::new(String::new()),
            closed: Mutex::new(None),
            prompt: Mutex::new(None),
            prompt_seq: AtomicU64::new(1),
            log: Mutex::new(log),
            resource: Mutex::new(None),
            resource_state: Mutex::new("waiting".into()),
            remote_os: Mutex::new(if is_local {
                local_os_name().to_string()
            } else {
                String::new()
            }),
            tunnels: Mutex::new(Vec::new()),
        });

        let pump = session.clone();
        std::thread::Builder::new()
            .name("xenterm-terminal-pump".into())
            .spawn(move || {
                while let Some(event) = events.blocking_recv() {
                    match event {
                        SessionEvent::Output(text) => {
                            if let Ok(mut engine) = pump.engine.lock() {
                                engine.process(text.as_bytes());
                            }
                            if let Ok(mut log) = pump.log.lock() {
                                if let Some(log) = log.as_mut() {
                                    log.output(&text);
                                }
                            }
                            pump.version.fetch_add(1, Ordering::SeqCst);
                        }
                        SessionEvent::Status(status) => {
                            if let Ok(mut slot) = pump.status.lock() {
                                *slot = status;
                            }
                            pump.version.fetch_add(1, Ordering::SeqCst);
                        }
                        SessionEvent::Connected => {
                            if let Ok(mut slot) = pump.status.lock() {
                                *slot = "Connected".into();
                            }
                            pump.version.fetch_add(1, Ordering::SeqCst);
                        }
                        SessionEvent::Closed(reason) => {
                            if let Ok(mut slot) = pump.closed.lock() {
                                *slot = Some(reason.clone());
                            }
                            if let Ok(mut log) = pump.log.lock() {
                                if let Some(log) = log.as_mut() {
                                    log.finish(Some(&reason));
                                }
                            }
                            pump.version.fetch_add(1, Ordering::SeqCst);
                            break;
                        }
                        SessionEvent::HostKeyPrompt {
                            host,
                            port,
                            key_type,
                            fingerprint,
                            changed,
                            responder,
                        } => {
                            pump.set_prompt(PendingPrompt::HostKey {
                                host,
                                port,
                                key_type,
                                fingerprint,
                                changed,
                                responder,
                            });
                        }
                        SessionEvent::CredentialPrompt {
                            host,
                            user,
                            need_user,
                            need_password,
                            responder,
                            ..
                        } => {
                            pump.set_prompt(PendingPrompt::Credential {
                                host,
                                user,
                                need_user,
                                need_password,
                                responder,
                            });
                        }
                        SessionEvent::MfaPrompt {
                            prompt,
                            echo,
                            responder,
                            ..
                        } => {
                            pump.set_prompt(PendingPrompt::Mfa {
                                prompt,
                                echo,
                                responder,
                            });
                        }
                        SessionEvent::ResourceStats {
                            cpu_sampled,
                            cpu_percent,
                            mem_used_kib,
                            mem_total_kib,
                            swap_used_kib,
                            swap_total_kib,
                            net,
                            disks,
                            gpus,
                            gpu_unsupported,
                            current_user,
                            procs,
                            ..
                        } => {
                            let sample = json!({
                                "cpu_sampled": cpu_sampled,
                                "cpu_percent": cpu_percent,
                                "mem_used_kib": mem_used_kib,
                                "mem_total_kib": mem_total_kib,
                                "swap_used_kib": swap_used_kib,
                                "swap_total_kib": swap_total_kib,
                                "net": net.iter().map(|(name, rx, tx)| json!({
                                    "name": name, "rx": rx, "tx": tx
                                })).collect::<Vec<_>>(),
                                "disks": disks.iter().map(|(path, available, total)| json!({
                                    "path": path, "available": available, "total": total
                                })).collect::<Vec<_>>(),
                                "gpus": gpus.iter().map(|gpu| json!({
                                    "name": gpu.name,
                                    "util_percent": gpu.util_percent,
                                    "mem_used_mib": gpu.mem_used_mib,
                                    "mem_total_mib": gpu.mem_total_mib,
                                    "temp_c": gpu.temp_c,
                                })).collect::<Vec<_>>(),
                                "gpu_unsupported": gpu_unsupported,
                                "user": current_user,
                                "procs": procs.iter().map(|proc| json!({
                                    "pid": proc.pid,
                                    "user": proc.user,
                                    "cpu": proc.cpu,
                                    "mem": proc.mem,
                                    "command": proc.command,
                                })).collect::<Vec<_>>(),
                            });
                            if let Ok(mut slot) = pump.resource.lock() {
                                *slot = Some(sample);
                            }
                            if let Ok(mut state) = pump.resource_state.lock() {
                                *state = "available".into();
                            }
                            pump.version.fetch_add(1, Ordering::SeqCst);
                        }
                        SessionEvent::ResourceMonitorStatus { state } => {
                            if let Ok(mut slot) = pump.resource_state.lock() {
                                *slot = resource_state_name(state).into();
                            }
                            pump.version.fetch_add(1, Ordering::SeqCst);
                        }
                        SessionEvent::RemoteOs(os) => {
                            if let Ok(mut slot) = pump.remote_os.lock() {
                                *slot = os;
                            }
                            pump.version.fetch_add(1, Ordering::SeqCst);
                        }
                        SessionEvent::TunnelUpdate(rows) => {
                            if let Ok(mut slot) = pump.tunnels.lock() {
                                *slot = rows;
                            }
                            pump.version.fetch_add(1, Ordering::SeqCst);
                        }
                        _ => {}
                    }
                }
            })
            .map_err(|error| error.to_string())?;

        Ok(session)
    }

    fn set_prompt(&self, prompt: PendingPrompt) {
        let id = self.prompt_seq.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut slot) = self.prompt.lock() {
            *slot = Some(PromptEntry { id, prompt });
        }
        self.version.fetch_add(1, Ordering::SeqCst);
    }

    /// Cheap change counter: poll this, and only read the snapshot when it moves.
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }

    /// Compact JSON screen for the renderer: rows of styled runs plus cursor,
    /// modes, status, close reason and any pending prompt.
    pub fn snapshot_json(&self) -> String {
        let engine = match self.engine.lock() {
            Ok(engine) => engine,
            Err(poisoned) => poisoned.into_inner(),
        };
        let (rows, cols) = engine.size();
        let (cursor_row, cursor_col) = engine.cursor_position();
        let mut out_rows = Vec::with_capacity(rows as usize);
        for row in 0..rows {
            let mut text = String::new();
            let mut runs: Vec<Value> = Vec::new();
            let mut run_text = String::new();
            let mut run_cells = 0usize;
            let mut run_style: Option<Value> = None;
            for col in 0..cols {
                let cell = engine.cell(row, col);
                if let Some(ref cell) = cell {
                    if cell.is_wide_continuation() {
                        continue;
                    }
                }
                let style = cell.as_ref().map(style_json);
                let contents = cell
                    .as_ref()
                    .map(|cell| cell.contents())
                    .unwrap_or_else(|| " ".to_string());
                let cells = if cell.as_ref().map(|c| c.is_wide()).unwrap_or(false) {
                    2
                } else {
                    1
                };
                if run_style.as_ref() != style.as_ref() {
                    if let Some(previous) = run_style.take() {
                        runs.push(json!({ "t": run_text, "c": run_cells, "s": previous }));
                    }
                    run_style = style;
                    run_text = String::new();
                    run_cells = 0;
                }
                run_cells += cells;
                run_text.push_str(&contents);
                text.push_str(&contents);
            }
            if let Some(previous) = run_style.take() {
                runs.push(json!({ "t": run_text, "c": run_cells, "s": previous }));
            }
            out_rows.push(json!({ "text": text, "runs": runs }));
        }
        let prompt = self
            .prompt
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(pending_prompt_json));
        let status = self
            .status
            .lock()
            .map(|slot| slot.clone())
            .unwrap_or_default();
        let closed = self.closed.lock().ok().and_then(|slot| slot.clone());
        let resource = self.resource.lock().ok().and_then(|slot| slot.clone());
        let resource_state = self
            .resource_state
            .lock()
            .map(|slot| slot.clone())
            .unwrap_or_else(|_| "waiting".into());
        let remote_os = self
            .remote_os
            .lock()
            .map(|slot| slot.clone())
            .unwrap_or_default();
        json!({
            "rows": out_rows,
            "cols": cols,
            "rowCount": rows,
            "cursor": [cursor_row, cursor_col],
            "cursorHidden": engine.cursor_hidden(),
            "applicationCursor": engine.application_cursor(),
            "alternate": engine.alternate_screen(),
            "status": status,
            "closed": closed,
            "prompt": prompt,
            "resource": resource,
            "resourceState": resource_state,
            "remoteOs": remote_os,
        })
        .to_string()
    }

    /// This session's live port-forward tunnels, as a JSON array. Empty until
    /// the session reports its first `TunnelUpdate` (i.e. for local/serial
    /// sessions and before any forward opens).
    pub fn tunnels_json(&self) -> String {
        let tunnels = self
            .tunnels
            .lock()
            .map(|slot| slot.clone())
            .unwrap_or_default();
        let rows: Vec<Value> = tunnels
            .iter()
            .map(|t| {
                json!({
                    "id": t.id,
                    "name": t.name,
                    "kind": t.kind,
                    "bindAddr": t.bind_addr,
                    "bindPort": t.bind_port,
                    "host": t.host,
                    "hostPort": t.host_port,
                    "active": t.active,
                    "status": t.status,
                })
            })
            .collect();
        serde_json::to_string(&rows).unwrap_or_else(|_| "[]".into())
    }

    pub fn send_input(&self, bytes: &[u8]) {
        self.handle.send_raw(bytes.to_vec());
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        if let Ok(mut engine) = self.engine.lock() {
            engine.resize(rows, cols);
        }
        self.handle.resize(cols as u32, rows as u32);
        self.version.fetch_add(1, Ordering::SeqCst);
    }

    pub fn close(&self) {
        self.handle.close();
    }

    /// Answers the current prompt (no-op when the id no longer matches).
    /// Host-key prompts use `accept`; credentials use `username`/`password`;
    /// MFA uses `text`.
    pub fn answer_prompt(
        &self,
        id: u64,
        accept: bool,
        username: Option<String>,
        password: Option<String>,
        text: Option<String>,
    ) {
        let entry = self
            .prompt
            .lock()
            .ok()
            .and_then(|mut slot| match slot.as_ref() {
                Some(entry) if entry.id == id => slot.take(),
                _ => None,
            });
        let Some(entry) = entry else { return };
        match entry.prompt {
            PendingPrompt::HostKey { responder, .. } => responder.respond(accept),
            PendingPrompt::Credential {
                responder, user, ..
            } => {
                if accept {
                    let user = username.filter(|u| !u.is_empty()).unwrap_or(user);
                    let reply: CredentialReply = (user, password.unwrap_or_default(), false);
                    responder.respond(Some(reply));
                } else {
                    responder.respond(None);
                }
            }
            PendingPrompt::Mfa { responder, .. } => {
                if accept {
                    responder.respond(text);
                } else {
                    responder.respond(None);
                }
            }
        }
        self.version.fetch_add(1, Ordering::SeqCst);
    }
}

/// One remembered SSH host key, projected for the frontend.
pub struct KnownHostInfo {
    pub id: String,
    pub key_type: String,
    pub fingerprint: String,
}

/// Every stored host key (TOFU `known_hosts`).
pub fn known_hosts() -> Vec<KnownHostInfo> {
    crate::ssh::known_hosts::entries()
        .into_iter()
        .map(|entry| KnownHostInfo {
            id: entry.id,
            key_type: entry.key_type,
            fingerprint: entry.fingerprint,
        })
        .collect()
}

/// Forgets one `host:port` entry. Returns whether anything was removed.
pub fn forget_known_host(id: &str) -> Result<bool, String> {
    crate::ssh::known_hosts::forget(id).map_err(|error| error.to_string())
}

/// One serial port the OS reports, for the serial connection dialog.
pub struct SerialPortInfo {
    pub name: String,
    pub kind: String,
}

/// Enumerate serial ports (best effort: an OS error reads as no ports and the
/// dialog still accepts a typed device path).
pub fn serial_ports() -> Vec<SerialPortInfo> {
    use serialport::SerialPortType;
    serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .map(|port| {
            let kind = match &port.port_type {
                SerialPortType::UsbPort(info) => {
                    let mut label = "USB".to_string();
                    if let Some(product) = info.product.as_deref() {
                        if !product.trim().is_empty() {
                            label.push_str(" · ");
                            label.push_str(product.trim());
                        }
                    }
                    label
                }
                SerialPortType::PciPort => "PCI".into(),
                SerialPortType::BluetoothPort => "Bluetooth".into(),
                SerialPortType::Unknown => "Serial".into(),
            };
            SerialPortInfo {
                name: port.port_name,
                kind,
            }
        })
        .collect()
}

/// What one SSH-config import changed.
pub struct ImportSummary {
    pub added: usize,
    pub skipped: usize,
    pub message: String,
}

fn apply_import(
    store: &mut ConfigStore,
    hosts: Vec<crate::ssh::ImportedHost>,
) -> Result<ImportSummary, String> {
    let fresh = crate::core::ssh_import::sessions_to_add(store.sessions(), &hosts);
    let added = fresh.len();
    let skipped = hosts.len().saturating_sub(added);
    for session in fresh {
        store.upsert(session);
    }
    if added > 0 {
        store.save().map_err(|error| error.to_string())?;
    }
    Ok(ImportSummary {
        added,
        skipped,
        message: if added == 0 {
            "No new hosts to import".into()
        } else {
            format!("Imported {added} host{}", if added == 1 { "" } else { "s" })
        },
    })
}

/// Imports hosts from OpenSSH config text (the caller supplies the file).
pub fn import_ssh_config_text(
    store: &mut ConfigStore,
    text: &str,
) -> Result<ImportSummary, String> {
    let home = crate::ssh::ssh_config::home_dir().unwrap_or_default();
    let hosts = crate::ssh::ssh_config::parse_str(text, &home);
    apply_import(store, hosts)
}

/// Imports the user's `~/.ssh/config`, if it exists.
pub fn import_ssh_config_default(store: &mut ConfigStore) -> Result<ImportSummary, String> {
    apply_import(store, crate::ssh::ssh_config::parse_default())
}

/// One SSH key credential referenced by a saved session. Key material never
/// leaves the core: only the path, algorithm, fingerprint and usage are exposed.
pub struct KeyInfo {
    pub session_id: String,
    pub session_name: String,
    pub host: String,
    pub user: String,
    /// "file" / "inline"
    pub source: String,
    /// Display file name for file-backed keys; empty for pasted keys.
    pub path: String,
    /// The session's configured path, used by edit/remove actions.
    pub full_path: String,
    pub key_type: String,
    pub fingerprint: String,
    /// "available" (parsed), "locked" (encrypted/unreadable), "missing".
    pub status: String,
}

/// One saved SSH session reached through one or more jump hosts.
pub struct JumpChainInfo {
    pub session_id: String,
    pub session_name: String,
    pub host: String,
    /// Jump host display names, outermost first.
    pub jumps: Vec<String>,
}

/// Sessions with a configured jump chain, resolved to display names.
pub fn jump_chains(store: &ConfigStore) -> Vec<JumpChainInfo> {
    let display = |id: &str| {
        store
            .sessions()
            .iter()
            .find(|session| session.id == id)
            .map(|session| {
                if session.name.is_empty() {
                    session.host.clone()
                } else {
                    session.name.clone()
                }
            })
            .unwrap_or_default()
    };
    store
        .sessions()
        .iter()
        .filter(|session| {
            session.kind == SessionKind::Ssh
                && (!session.jump_session_id.trim().is_empty()
                    || !session.jump_session_ids.is_empty())
        })
        .map(|session| {
            let ids: Vec<String> = if session.jump_session_ids.is_empty() {
                vec![session.jump_session_id.clone()]
            } else {
                session.jump_session_ids.clone()
            };
            JumpChainInfo {
                session_id: session.id.clone(),
                session_name: if session.name.is_empty() {
                    session.host.clone()
                } else {
                    session.name.clone()
                },
                host: session.host.clone(),
                jumps: ids
                    .iter()
                    .filter(|id| !id.trim().is_empty())
                    .map(|id| display(id))
                    .filter(|name| !name.is_empty())
                    .collect(),
            }
        })
        .filter(|chain| !chain.jumps.is_empty())
        .collect()
}

/// Repoints every session that uses `old_path` to `new_path`.
pub fn update_key_path(
    store: &mut ConfigStore,
    old_path: &str,
    new_path: &str,
) -> Result<usize, String> {
    let old = old_path.trim();
    let new = new_path.trim();
    if old.is_empty() || new.is_empty() {
        return Err("the key path cannot be empty".into());
    }
    let mut changed = 0usize;
    let mut sessions: Vec<crate::config::Session> = store
        .sessions()
        .iter()
        .filter(|session| session.private_key_path.trim() == old)
        .cloned()
        .collect();
    for session in &mut sessions {
        session.private_key_path = new.to_string();
        session.auth = crate::config::AuthMethod::Key;
        changed += 1;
    }
    if changed > 0 {
        for session in sessions {
            store.upsert(session);
        }
        store.save().map_err(|error| error.to_string())?;
    }
    Ok(changed)
}

/// Drops the key reference from every session that uses `path` (the key file
/// itself is untouched): those sessions fall back to password auth.
pub fn remove_key(store: &mut ConfigStore, path: &str) -> Result<usize, String> {
    let target = path.trim();
    if target.is_empty() {
        return Err("this key has no file path".into());
    }
    let mut changed = 0usize;
    let mut sessions: Vec<crate::config::Session> = store
        .sessions()
        .iter()
        .filter(|session| session.private_key_path.trim() == target)
        .cloned()
        .collect();
    for session in &mut sessions {
        session.private_key_path.clear();
        session.auth = crate::config::AuthMethod::Password;
        changed += 1;
    }
    if changed > 0 {
        for session in sessions {
            store.upsert(session);
        }
        store.save().map_err(|error| error.to_string())?;
    }
    Ok(changed)
}

/// Inventory of key credentials across saved sessions. Parsing only happens for
/// unencrypted file keys; pasted/encrypted keys are listed without material.
pub fn key_inventory(store: &ConfigStore) -> Vec<KeyInfo> {
    store
        .sessions()
        .iter()
        .filter_map(|session| {
            let path = session.private_key_path.trim();
            let inline = !session.private_key_inline.is_empty();
            if path.is_empty() && !inline {
                return None;
            }
            let (source, key_name) = if !path.is_empty() {
                ("file", path.to_string())
            } else {
                ("inline", String::new())
            };
            let mut key_type = String::new();
            let mut fingerprint = String::new();
            let mut status = "available".to_string();
            if source == "inline" {
                status = "locked".into();
            } else {
                let normalised = path.replace('\\', "/");
                let key_path = normalised
                    .strip_suffix(".pub")
                    .map(str::to_string)
                    .unwrap_or(normalised);
                if !std::path::Path::new(&key_path).exists() {
                    status = "missing".into();
                } else if key_path.to_ascii_lowercase().ends_with(".ppk") {
                    key_type = "PuTTY PPK".into();
                } else {
                    match crate::ssh::load_session_private_key(session, "") {
                        Ok(key) => {
                            let public = key.public_key();
                            key_type = public.algorithm().as_str().to_string();
                            fingerprint =
                                public.fingerprint(russh::keys::HashAlg::Sha256).to_string();
                        }
                        Err(_) => status = "locked".into(),
                    }
                }
            }
            let name = if session.name.is_empty() {
                session.host.clone()
            } else {
                session.name.clone()
            };
            let file_name = if key_name.is_empty() {
                String::new()
            } else {
                std::path::Path::new(&key_name)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(&key_name)
                    .to_string()
            };
            Some(KeyInfo {
                session_id: session.id.clone(),
                session_name: name,
                host: session.host.clone(),
                user: session.user.clone(),
                source: source.into(),
                path: file_name,
                full_path: key_name,
                key_type,
                fingerprint,
                status,
            })
        })
        .collect()
}

/// One saved session transcript.
pub struct SessionLogEntry {
    pub file: String,
    pub session_id: String,
    pub name: String,
    pub host: String,
    pub user: String,
    pub started_ms: u64,
    pub size: u64,
}

fn log_path(name: &str) -> Result<std::path::PathBuf, String> {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || !name.ends_with(".jsonl")
    {
        return Err("invalid log name".into());
    }
    Ok(crate::config::data_dir().join("session_logs").join(name))
}

/// Transcripts on disk, newest first.
pub fn session_logs() -> Vec<SessionLogEntry> {
    let dir = crate::config::data_dir().join("session_logs");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut logs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(file) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Ok(handle) = File::open(&path) else {
            continue;
        };
        let mut first = String::new();
        if BufReader::new(handle).read_line(&mut first).is_err() {
            continue;
        }
        let Ok(meta) = serde_json::from_str::<Value>(&first) else {
            continue;
        };
        if meta["kind"] != "meta" {
            continue;
        }
        logs.push(SessionLogEntry {
            file: file.to_string(),
            session_id: meta["session_id"].as_str().unwrap_or_default().to_string(),
            name: meta["name"].as_str().unwrap_or_default().to_string(),
            host: meta["host"].as_str().unwrap_or_default().to_string(),
            user: meta["user"].as_str().unwrap_or_default().to_string(),
            started_ms: meta["started_ms"].as_u64().unwrap_or(0),
            size: entry.metadata().map(|m| m.len()).unwrap_or(0),
        });
    }
    logs.sort_by(|a, b| b.started_ms.cmp(&a.started_ms));
    logs
}

/// Removes terminal control sequences from a recorded stream for display.
/// Raw bytes stay in the JSONL file; this is a read-time projection so a
/// transcript reads like text instead of escape soup.
fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            if c == '\r' {
                continue;
            }
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('[') => {
                chars.next();
                for n in chars.by_ref() {
                    if ('@'..='~').contains(&n) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                while let Some(n) = chars.next() {
                    if n == '\u{7}' {
                        break;
                    }
                    if n == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            Some('(') | Some(')') => {
                chars.next();
                chars.next();
            }
            _ => {}
        }
    }
    out
}

/// Collapses the blank runs ConPTY emits while repainting, so a transcript
/// starts at its first real line instead of a screenful of empty rows.
fn tidy_transcript(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut blanks = 0;
    for line in input.lines() {
        if line.trim().is_empty() {
            blanks += 1;
            if blanks > 1 {
                continue;
            }
        } else {
            blanks = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim_start_matches('\n').to_string()
}

/// Reads one transcript: its meta line, the concatenated output and the end
/// timestamp when the session closed cleanly.
pub fn read_session_log(name: &str) -> Result<Value, String> {
    let path = log_path(name)?;
    let file = File::open(&path).map_err(|e| e.to_string())?;
    let mut meta: Option<Value> = None;
    let mut text = String::new();
    let mut ended: Option<u64> = None;
    let mut close_reason: Option<String> = None;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match value["kind"].as_str() {
            Some("meta") => meta = Some(value),
            Some("output") => {
                if let Some(chunk) = value["text"].as_str() {
                    text.push_str(chunk);
                }
            }
            Some("end") => {
                ended = value["ended_ms"].as_u64();
                close_reason = value["reason"].as_str().map(str::to_string);
            }
            _ => {}
        }
    }
    let meta = meta.ok_or_else(|| "the transcript has no metadata".to_string())?;
    Ok(json!({
        "meta": meta,
        "text": tidy_transcript(&strip_ansi(&text)),
        "ended_ms": ended,
        "close_reason": close_reason,
    }))
}

/// Deletes one transcript. Returns whether a file was removed.
pub fn delete_session_log(name: &str) -> Result<bool, String> {
    let path = log_path(name)?;
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

/// One SFTP browser session: a dedicated SSH connection whose listing,
/// transfers and errors are projected into a polled JSON snapshot.
pub struct SftpSession {
    handle: crate::sftp::SftpHandle,
    state: Mutex<SftpState>,
    version: AtomicU64,
    prompt: Mutex<Option<PromptEntry>>,
    prompt_seq: AtomicU64,
}

struct SftpState {
    path: String,
    entries: Vec<crate::session::protocol::RemoteEntry>,
    loading: bool,
    error: Option<String>,
    status: String,
    closed: Option<String>,
    transfers: Vec<TransferInfo>,
    tree: Vec<crate::session::protocol::RemoteTreeNode>,
}

struct TransferInfo {
    id: String,
    name: String,
    is_upload: bool,
    transferred: u64,
    total: u64,
    state: u8,
    msg: String,
}

impl SftpSession {
    /// Opens an SFTP channel for a saved session and starts listing its home.
    pub fn start(
        runtime: &tokio::runtime::Handle,
        store: &ConfigStore,
        session_id: &str,
    ) -> Result<Arc<SftpSession>, String> {
        let session = store
            .sessions()
            .iter()
            .find(|s| s.id == session_id)
            .cloned()
            .ok_or_else(|| format!("session {session_id} not found"))?;
        let jump = store
            .resolve_jump_chain(&session)
            .map_err(|error| error.to_string())?;
        let (events_tx, mut events) = tokio::sync::mpsc::unbounded_channel();
        let handle = crate::sftp::spawn_sftp(runtime, session, jump, events_tx);
        let session = Arc::new(SftpSession {
            handle,
            state: Mutex::new(SftpState {
                path: ".".into(),
                entries: Vec::new(),
                loading: true,
                error: None,
                status: String::new(),
                closed: None,
                transfers: Vec::new(),
                tree: Vec::new(),
            }),
            version: AtomicU64::new(1),
            prompt: Mutex::new(None),
            prompt_seq: AtomicU64::new(1),
        });
        session.handle.list_dir(".".into());
        // Seed the folder tree with the remote root expanded.
        session.handle.toggle_tree_node("/".into());

        let pump = session.clone();
        std::thread::Builder::new()
            .name("xenterm-sftp-pump".into())
            .spawn(move || {
                while let Some(event) = events.blocking_recv() {
                    let mut finished = false;
                    if let Ok(mut state) = pump.state.lock() {
                        match event {
                            SessionEvent::SftpEntries { path, entries } => {
                                state.path = path;
                                state.entries = entries;
                                state.loading = false;
                                state.error = None;
                            }
                            SessionEvent::SftpStatus(status) => {
                                state.status = status;
                            }
                            SessionEvent::SftpError(error) => {
                                state.error = Some(error);
                                state.loading = false;
                            }
                            SessionEvent::SftpTreeUpdate(nodes) => {
                                state.tree = nodes;
                            }
                            SessionEvent::SftpTransfer {
                                id,
                                name,
                                is_upload,
                                transferred,
                                total,
                                state: transfer_state,
                                msg,
                            } => {
                                let info = TransferInfo {
                                    id: id.clone(),
                                    name,
                                    is_upload,
                                    transferred,
                                    total,
                                    state: transfer_state,
                                    msg,
                                };
                                if let Some(slot) = state.transfers.iter_mut().find(|t| t.id == id)
                                {
                                    *slot = info;
                                } else {
                                    state.transfers.push(info);
                                    if state.transfers.len() > 20 {
                                        state.transfers.remove(0);
                                    }
                                }
                            }
                            SessionEvent::HostKeyPrompt {
                                host,
                                port,
                                key_type,
                                fingerprint,
                                changed,
                                responder,
                            } => {
                                drop(state);
                                pump.set_prompt(PendingPrompt::HostKey {
                                    host,
                                    port,
                                    key_type,
                                    fingerprint,
                                    changed,
                                    responder,
                                });
                            }
                            SessionEvent::CredentialPrompt {
                                host,
                                user,
                                need_user,
                                need_password,
                                responder,
                                ..
                            } => {
                                drop(state);
                                pump.set_prompt(PendingPrompt::Credential {
                                    host,
                                    user,
                                    need_user,
                                    need_password,
                                    responder,
                                });
                            }
                            SessionEvent::MfaPrompt {
                                prompt,
                                echo,
                                responder,
                                ..
                            } => {
                                drop(state);
                                pump.set_prompt(PendingPrompt::Mfa {
                                    prompt,
                                    echo,
                                    responder,
                                });
                            }
                            SessionEvent::Connected => {
                                state.status = "Connected".into();
                            }
                            SessionEvent::Closed(reason) => {
                                state.closed = Some(reason);
                                state.loading = false;
                                finished = true;
                            }
                            _ => {}
                        }
                    }
                    pump.version.fetch_add(1, Ordering::SeqCst);
                    if finished {
                        break;
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(session)
    }

    fn set_prompt(&self, prompt: PendingPrompt) {
        let id = self.prompt_seq.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut slot) = self.prompt.lock() {
            *slot = Some(PromptEntry { id, prompt });
        }
        self.version.fetch_add(1, Ordering::SeqCst);
    }

    /// Answers the current SFTP auth prompt (host key / credentials / MFA).
    pub fn answer_prompt(
        &self,
        id: u64,
        accept: bool,
        username: Option<String>,
        password: Option<String>,
        text: Option<String>,
    ) {
        let entry = self
            .prompt
            .lock()
            .ok()
            .and_then(|mut slot| match slot.as_ref() {
                Some(entry) if entry.id == id => slot.take(),
                _ => None,
            });
        let Some(entry) = entry else { return };
        match entry.prompt {
            PendingPrompt::HostKey { responder, .. } => responder.respond(accept),
            PendingPrompt::Credential {
                responder, user, ..
            } => {
                if accept {
                    let user = username.filter(|u| !u.is_empty()).unwrap_or(user);
                    responder.respond(Some((user, password.unwrap_or_default(), false)));
                } else {
                    responder.respond(None);
                }
            }
            PendingPrompt::Mfa { responder, .. } => {
                if accept {
                    responder.respond(text);
                } else {
                    responder.respond(None);
                }
            }
        }
        self.version.fetch_add(1, Ordering::SeqCst);
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }

    pub fn snapshot_json(&self) -> String {
        let state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        let entries: Vec<Value> = state
            .entries
            .iter()
            .map(|entry| {
                json!({
                    "name": entry.name,
                    "path": entry.full_path,
                    "dir": entry.is_dir,
                    "size": entry.size,
                    "modified": entry.modified,
                    "mode": entry.mode,
                })
            })
            .collect();
        let transfers: Vec<Value> = state
            .transfers
            .iter()
            .map(|transfer| {
                json!({
                    "id": transfer.id,
                    "name": transfer.name,
                    "upload": transfer.is_upload,
                    "transferred": transfer.transferred,
                    "total": transfer.total,
                    "state": transfer.state,
                    "msg": transfer.msg,
                })
            })
            .collect();
        let tree: Vec<Value> = state
            .tree
            .iter()
            .map(|node| {
                json!({
                    "path": node.path,
                    "name": node.name,
                    "depth": node.depth,
                    "expanded": node.expanded,
                    "has_children": node.has_children,
                })
            })
            .collect();
        let prompt = self
            .prompt
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(pending_prompt_json));
        json!({
            "path": state.path,
            "entries": entries,
            "tree": tree,
            "loading": state.loading,
            "error": state.error,
            "status": state.status,
            "closed": state.closed,
            "transfers": transfers,
            "prompt": prompt,
        })
        .to_string()
    }

    pub fn list(&self, path: String) {
        if let Ok(mut state) = self.state.lock() {
            state.loading = true;
            state.error = None;
        }
        self.version.fetch_add(1, Ordering::SeqCst);
        self.handle.list_dir(path);
    }

    pub fn refresh(&self) {
        let path = match self.state.lock() {
            Ok(state) => state.path.clone(),
            Err(poisoned) => poisoned.into_inner().path.clone(),
        };
        if let Ok(mut state) = self.state.lock() {
            state.loading = true;
            state.error = None;
        }
        self.version.fetch_add(1, Ordering::SeqCst);
        self.handle.refresh_dir(path);
    }

    pub fn toggle_tree(&self, path: String) {
        self.handle.toggle_tree_node(path);
    }

    pub fn download(&self, remote: String, local_dir: String) {
        self.handle
            .download(remote, local_dir, crate::sftp::DownloadConflict::KeepBoth);
    }

    pub fn upload(&self, local: String) {
        let remote_dir = match self.state.lock() {
            Ok(state) => state.path.clone(),
            Err(poisoned) => poisoned.into_inner().path.clone(),
        };
        self.handle
            .upload(std::path::PathBuf::from(local), remote_dir);
    }

    pub fn mkdir(&self, name: String) {
        let base = match self.state.lock() {
            Ok(state) => state.path.clone(),
            Err(poisoned) => poisoned.into_inner().path.clone(),
        };
        let name = name.trim_matches('/');
        if name.is_empty() {
            return;
        }
        let path = match base.as_str() {
            "." => name.to_string(),
            "/" => format!("/{name}"),
            other => format!("{other}/{name}"),
        };
        self.handle.mkdir(path);
    }

    pub fn delete(&self, path: String) {
        self.handle.delete(path);
    }

    pub fn close(&self) {
        self.handle.close();
    }
}

/// Convenience for a frontend that keeps a registry of sessions.

pub type SessionRegistry = HashMap<u64, Arc<TerminalSession>>;

/// Creates a session registry shared behind a mutex.
pub fn new_registry() -> Arc<Mutex<SessionRegistry>> {
    Arc::new(Mutex::new(SessionRegistry::new()))
}
