//! Process/session-owned, shell-free dynamic forwards. No global daemon, IPC,
//! credential file or guessed ownership of an already listening port.
use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{anyhow, bail, ensure, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::config::{PortForward, Session};

#[derive(Clone)]
struct Context {
    owner: Arc<ForwardOwner>,
    expires: Option<Instant>,
}
tokio::task_local! { static OWNER: Context; }

pub(crate) async fn with_forward_owner<F: Future>(
    owner: Arc<ForwardOwner>, expires: Option<Instant>, future: F,
) -> F::Output {
    OWNER.scope(Context { owner, expires }, future).await
}

pub(crate) fn forward_owner() -> Result<(Arc<ForwardOwner>, Option<Instant>)> {
    OWNER.try_with(|context| (context.owner.clone(), context.expires))
        .map_err(|_| anyhow!("dynamic tunnel requires a live CLI/MCP owner"))
}

#[derive(Clone, Serialize)]
pub(crate) struct ForwardStatus {
    pub handle: String,
    pub session_id: String,
    pub forward_index: usize,
    pub proxy_url: String,
    pub state: String,
    pub owner_pid: u32,
    pub ownership: &'static str,
}

struct Entry {
    status: ForwardStatus,
    fingerprint: [u8; 32],
    state: watch::Receiver<&'static str>,
    task: Option<JoinHandle<()>>,
}
impl Drop for Entry {
    fn drop(&mut self) {
        if let Some(task) = &self.task { task.abort(); }
    }
}
impl Entry {
    fn status(&self) -> ForwardStatus {
        let mut status = self.status.clone();
        status.state = (*self.state.borrow()).to_string();
        if status.state == "running" && self.task.as_ref().is_none_or(JoinHandle::is_finished) {
            status.state = "disconnected".into();
        }
        status
    }
}

/// One owner per stdio connection, authenticated HTTP session, or foreground
/// CLI. Drop aborts its tasks; their transport guards close every SSH hop.
pub(crate) struct ForwardOwner {
    serial: tokio::sync::Mutex<()>,
    closed: AtomicBool,
    shutdown_tx: watch::Sender<bool>,
    enforce_permissions: bool,
    entries: Mutex<HashMap<String, Entry>>,
}

impl Default for ForwardOwner {
    fn default() -> Self {
        Self { serial: tokio::sync::Mutex::new(()), closed: AtomicBool::new(false),
            shutdown_tx: watch::channel(false).0, enforce_permissions: false,
            entries: Mutex::new(HashMap::new()) }
    }
}

pub(crate) fn loopback_bind(rule: &PortForward) -> Result<SocketAddr> {
    ensure!(rule.kind == "dynamic", "selected forward rule is not dynamic SOCKS5");
    let text = rule.bind_addr.trim().trim_start_matches('[').trim_end_matches(']');
    let address: IpAddr = match text {
        "" | "localhost" => "127.0.0.1".parse().unwrap(),
        _ => text.parse().map_err(|_| anyhow!("dynamic tunnel bind must be a loopback IP address"))?,
    };
    // Unauthenticated SOCKS must never silently expose a saved wildcard bind to
    // a network. Broader listening needs a separately designed access policy.
    ensure!(address.is_loopback(), "unattended dynamic tunnels require a loopback bind address");
    Ok(SocketAddr::new(address, rule.bind_port))
}

/// Only inputs consumed by the SSH route/authentication and selected dynamic
/// listener belong to reuse identity. UI touches, labels, notes, terminal
/// settings and unrelated forward rules must not invalidate a live tunnel.
#[derive(Serialize)]
struct RouteHop<'a> {
    id: &'a str,
    host: &'a str,
    port: u16,
    user: &'a str,
    auth: crate::config::AuthMethod,
    password: &'a crate::config::Secret,
    private_key_path: &'a str,
    private_key_inline: &'a crate::config::Secret,
    proxy: &'a str,
}

pub(super) fn route_fingerprint(session: &Session, jumps: &[Session], index: usize) -> Result<[u8; 32]> {
    let rule = session.forwards.get(index).ok_or_else(|| anyhow!("saved forward rule not found"))?;
    let route: Vec<_> = jumps.iter().chain(std::iter::once(session)).enumerate().map(|(i, hop)| RouteHop {
        id: &hop.id, host: &hop.host, port: hop.port, user: &hop.user, auth: hop.auth,
        password: &hop.password, private_key_path: &hop.private_key_path,
        private_key_inline: &hop.private_key_inline,
        // Jump chains are immediate-first: the last jump opens TCP/proxy.
        // Without jumps, the target at index zero opens TCP. All other
        // hops use direct-tcpip channels and do not consume saved proxies.
        proxy: if i == jumps.len().saturating_sub(1) { &hop.proxy } else { "" },
    }).collect();
    let bytes = zeroize::Zeroizing::new(serde_json::to_vec(&(route, loopback_bind(rule)?))?);
    Ok(Sha256::digest(bytes.as_slice()).into())
}

impl ForwardOwner {
    pub(crate) fn unattended() -> Self {
        Self { enforce_permissions: true, ..Default::default() }
    }
    pub(crate) async fn start(
        &self, session: Session, jumps: Vec<Session>, index: usize,
        reuse: bool, expires: Option<Instant>,
    ) -> Result<ForwardStatus> {
        let rule = session.forwards.get(index)
            .ok_or_else(|| anyhow!("saved forward rule not found"))?;
        let bind = loopback_bind(rule)?;
        // A digest detects edited routes/credentials without retaining another
        // plaintext configuration or exposing any of it in status responses.
        let fingerprint = route_fingerprint(&session, &jumps, index)?;
        let _serial = self.serial.lock().await;
        ensure!(!self.closed.load(Ordering::Acquire), "dynamic tunnel owner is closed");
        {
            let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = entries.values().find(|e|
                e.status.session_id == session.id && e.status.forward_index == index) {
                ensure!(reuse, "dynamic tunnel already exists; use ensure_dynamic_tunnel or release it");
                ensure!(entry.fingerprint == fingerprint, "saved tunnel configuration changed; release the old handle before restarting");
                let status = entry.status();
                ensure!(status.state == "running", "dynamic tunnel is no longer running; release the old handle before restarting");
                return Ok(status);
            }
            ensure!(entries.len() < 32, "dynamic tunnel owner limit reached; release unused handles");
        }
        // Reserve the actual port before connecting. A conflict never kills or
        // attaches to another process, whose identity we cannot establish.
        let listener = TcpListener::bind(bind).await.map_err(|error| match error.kind() {
            std::io::ErrorKind::AddrInUse => anyhow!("dynamic tunnel port is in use; no existing listener was changed or adopted"),
            _ => anyhow!("cannot bind the saved dynamic tunnel address"),
        })?;
        let address = listener.local_addr()?;
        let status = ForwardStatus {
            handle: uuid::Uuid::new_v4().to_string(), session_id: session.id.clone(),
            forward_index: index, proxy_url: format!("socks5h://{address}"),
            state: "starting".into(), owner_pid: std::process::id(),
            ownership: "current-owner-only; other processes are not adopted",
        };
        let (state_tx, state) = watch::channel("starting");
        let (ready_tx, ready_rx) = oneshot::channel();
        let enforce_permissions = self.enforce_permissions;
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let task = tokio::spawn(crate::ssh::connection::with_automation_cancellation(async move {
            let run = async {
                // Startup is bounded across the entire multi-hop route; each
                // individual SSH stage also keeps the shared network deadline.
                let connection = tokio::time::timeout(Duration::from_secs(60),
                    crate::ssh::open_forward_connection(&session, &jumps)).await;
                let (handle, _jumps) = match connection {
                    Ok(Ok(value)) => value,
                    _ => {
                        let _ = ready_tx.send(Err(anyhow!("dynamic tunnel SSH connection failed; check saved authentication, trusted host keys and jump route")));
                        return;
                    }
                };
                state_tx.send_replace("running");
                if ready_tx.send(Ok(())).is_err() { return; }
                let _ = super::forward::run_dynamic(Arc::new(handle), listener).await;
            };
            let permission_revoked = async {
                if !enforce_permissions { std::future::pending::<()>().await; }
                loop {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    let permitted = crate::config::ConfigStore::load().is_ok_and(|store|
                        store.mcp_enabled() && store.mcp_use_saved_credentials() && store.mcp_allow_commands());
                    if !permitted { break; }
                }
            };
            let expired = async {
                match expires {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending::<()>().await,
                }
            };
            let owner_closed = async {
                let already_closed = *shutdown_rx.borrow();
                if !already_closed { let _ = shutdown_rx.changed().await; }
            };
            let final_state = tokio::select! {
                biased;
                _ = owner_closed => "owner_closed",
                _ = expired => "authorization_expired",
                _ = permission_revoked => "permission_revoked",
                _ = run => "disconnected",
            };
            state_tx.send_replace(final_state);
        }));
        let mut entry = Entry { status, fingerprint, state, task: Some(task) };
        // Entry's Drop also handles caller cancellation during startup. Never
        // detach a transport when an HTTP request times out before its reply.
        ready_rx.await.map_err(|_| anyhow!("dynamic tunnel startup was cancelled or its authorization expired"))??;
        entry.status.state = "running".into();
        let status = entry.status();
        ensure!(status.state == "running", "dynamic tunnel stopped before startup completed");
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        ensure!(!self.closed.load(Ordering::Acquire), "dynamic tunnel owner is closed");
        entries.insert(status.handle.clone(), entry);
        Ok(status)
    }

    pub(crate) fn status(&self, handle: &str) -> Result<ForwardStatus> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).get(handle)
            .map(Entry::status).ok_or_else(|| anyhow!("tunnel handle not found in this owner"))
    }

    pub(crate) async fn release(&self, handle: &str) -> Result<ForwardStatus> {
        let _serial = self.serial.lock().await;
        let mut entry = self.entries.lock().unwrap_or_else(|e| e.into_inner()).remove(handle)
            .ok_or_else(|| anyhow!("tunnel handle not found in this owner"))?;
        if let Some(task) = entry.task.take() {
            task.abort();
            let _ = task.await; // Listener/clients/transport guards have dropped.
        }
        let mut status = entry.status.clone();
        status.state = "released".into();
        Ok(status)
    }

    pub(crate) fn shutdown(&self) {
        self.closed.store(true, Ordering::Release);
        self.shutdown_tx.send_replace(true);
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    pub(crate) async fn wait_stopped(&self, handle: &str) -> Result<()> {
        loop {
            if self.status(handle)?.state != "running" { bail!("dynamic tunnel disconnected"); }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unattended_bind_is_loopback_only() {
        for bind in ["", "localhost", "127.0.0.1", "::1", "[::1]"] {
            let rule = PortForward { kind: "dynamic".into(), bind_addr: bind.into(), ..Default::default() };
            assert!(loopback_bind(&rule).unwrap().ip().is_loopback());
        }
        for bind in ["0.0.0.0", "::", "192.0.2.1", "proxy.example"] {
            let rule = PortForward { kind: "dynamic".into(), bind_addr: bind.into(), ..Default::default() };
            assert!(loopback_bind(&rule).is_err());
        }
    }

    fn fixture_session(port: u16) -> Session {
        let mut session = Session::new_empty();
        session.host = "target.invalid".into();
        session.forwards.push(PortForward { kind: "dynamic".into(), bind_port: port, ..Default::default() });
        session
    }

    #[test]
    fn reuse_identity_ignores_ui_metadata_but_tracks_selected_route_and_auth() {
        let mut session = fixture_session(1080);
        let mut jumps = vec![Session::new_empty()];
        let original = route_fingerprint(&session, &jumps, 0).unwrap();
        session.name = "renamed".into();
        session.note = "edited note".into();
        session.group = "different folder".into();
        session.last_used = Some("later".into());
        session.forwards[0].name = "different label".into();
        session.forwards.push(PortForward::default());
        session.proxy = "ignored-on-inner-hop".into();
        jumps[0].last_used = Some("later".into());
        jumps[0].name = "renamed jump".into();
        assert_eq!(route_fingerprint(&session, &jumps, 0).unwrap(), original);
        session.forwards[0].bind_port += 1;
        assert_ne!(route_fingerprint(&session, &jumps, 0).unwrap(), original);
        session.forwards[0].bind_port -= 1;
        jumps[0].password = crate::config::Secret::new("synthetic-changed-password");
        assert_ne!(route_fingerprint(&session, &jumps, 0).unwrap(), original);
        jumps[0].password = crate::config::Secret::default();
        jumps[0].host = "different-jump.invalid".into();
        assert_ne!(route_fingerprint(&session, &jumps, 0).unwrap(), original);
    }

    #[test]
    fn reuse_fingerprint_tracks_only_the_actual_outer_tcp_proxy() {
        for count in 0..=3 {
            let mut session = fixture_session(1080);
            let mut jumps: Vec<_> = (0..count).map(|_| Session::new_empty()).collect();
            let original = route_fingerprint(&session, &jumps, 0).unwrap();
            if let Some(outer) = jumps.last_mut() {
                outer.proxy = "socks5://proxy-a.invalid:1080".into();
            } else {
                session.proxy = "socks5://proxy-a.invalid:1080".into();
            }
            let changed = route_fingerprint(&session, &jumps, 0).unwrap();
            assert_ne!(changed, original, "outer proxy ignored with {count} jumps");
            if count > 0 {
                session.proxy = "socks5://ignored-target.invalid:1080".into();
                for hop in jumps.iter_mut().take(count - 1) {
                    hop.proxy = "socks5://ignored-inner.invalid:1080".into();
                }
                assert_eq!(route_fingerprint(&session, &jumps, 0).unwrap(), changed);
                jumps.last_mut().unwrap().proxy = "socks5://proxy-b.invalid:1080".into();
                assert_ne!(route_fingerprint(&session, &jumps, 0).unwrap(), changed);
            }
        }
    }

    // A synthetic HTTP proxy accepts TCP but withholds CONNECT completion.
    // This forces a real pending SSH route, not a mocked future or parser test.
    async fn pending_start(expires: Option<Instant>) -> (Arc<ForwardOwner>, JoinHandle<Result<ForwardStatus>>, TcpListener, SocketAddr) {
        let silent = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let reserve = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bind = reserve.local_addr().unwrap();
        drop(reserve);
        let mut session = fixture_session(bind.port());
        session.proxy = format!("http://{}", silent.local_addr().unwrap());
        let owner = Arc::new(ForwardOwner::default());
        let worker = owner.clone();
        let start = tokio::spawn(async move { worker.start(session, vec![], 0, false, expires).await });
        (owner, start, silent, bind)
    }

    async fn assert_listener_released(bind: SocketAddr) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if TcpListener::bind(bind).await.is_ok() { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("startup left the dynamic listener bound");
    }

    #[tokio::test]
    async fn cancelled_start_rolls_back_listener_and_pending_proxy_transport() {
        use tokio::io::AsyncReadExt;
        let (_owner, start, silent, bind) = pending_start(None).await;
        let (mut transport, _) = tokio::time::timeout(Duration::from_secs(2), silent.accept()).await.unwrap().unwrap();
        start.abort();
        let _ = start.await;
        assert_listener_released(bind).await;
        let mut bytes = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), transport.read_to_end(&mut bytes)).await
            .expect("cancelled startup left an SSH proxy transport open").unwrap();
    }

    #[tokio::test]
    async fn expired_start_rolls_back_without_ever_becoming_ready() {
        let (_owner, start, silent, bind) = pending_start(Some(Instant::now() + Duration::from_millis(200))).await;
        let (_transport, _) = tokio::time::timeout(Duration::from_secs(2), silent.accept()).await.unwrap().unwrap();
        let result = tokio::time::timeout(Duration::from_secs(2), start).await.unwrap().unwrap();
        assert!(result.is_err());
        assert_listener_released(bind).await;
    }


    #[tokio::test]
    async fn owner_shutdown_cancels_a_start_not_yet_registered() {
        let (owner, start, silent, bind) = pending_start(None).await;
        let (_transport, _) = tokio::time::timeout(Duration::from_secs(2), silent.accept()).await.unwrap().unwrap();
        owner.shutdown();
        let result = tokio::time::timeout(Duration::from_secs(2), start).await.unwrap().unwrap();
        assert!(result.is_err());
        assert_listener_released(bind).await;
        assert!(owner.entries.lock().unwrap().is_empty());
    }

}
