//! Process-owned dynamic SOCKS, independent of TerminalSession/PTY. This is not
//! an IPC broker and never adopts an existing listener by guessing its port.
use crate::config::{PortForward, Session};
use anyhow::{anyhow, ensure, Result};
use futures::FutureExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

#[derive(Clone, Serialize)]
pub(crate) struct ForwardStatus {
    pub handle: String,
    pub session_id: String,
    pub forward_index: usize,
    pub proxy_url: String,
    pub state: String,
    pub ownership: &'static str,
}

const LIVE_LIMIT: usize = 32;
const TERMINAL_LIMIT: usize = 128;
// Measured from the confirmed join, pruned lazily by owner operations.
const TERMINAL_TTL: Duration = Duration::from_secs(5 * 60);

/// Frozen route inputs supplied only by trusted in-process preparation. This is
/// not a consistent profile loader or a public authorization/epoch constructor.
pub(crate) struct PreparedForward {
    session: Session,
    jumps: Vec<Session>,
    bind: SocketAddr,
    index: usize,
    identity: ReuseIdentity,
    policy: crate::ssh::headless_policy::Policy,
}
struct ReuseIdentity {
    fingerprint: [u8; 32],
    generation: u64,
    policy: [u8; 32],
    reusable: bool,
}
impl PreparedForward {
    pub(crate) fn capture(
        session: Session,
        jumps: Vec<Session>,
        index: usize,
        generation: u64,
    ) -> Result<Self> {
        ensure!(
            std::iter::once(&session)
                .chain(jumps.iter())
                .all(|hop| matches!(hop.kind, crate::config::SessionKind::Ssh)),
            "dynamic forwarding requires SSH sessions"
        );
        let rule = session
            .forwards
            .get(index)
            .ok_or_else(|| anyhow!("saved forward rule not found"))?;
        let bind = loopback_bind(rule)?;
        let policy = crate::ssh::headless_policy::Policy::capture(&session, &jumps)?;
        let identity = ReuseIdentity {
            fingerprint: route_fingerprint(&session, &jumps, index)?,
            generation,
            policy: policy.identity(),
            reusable: policy.reusable(),
        };
        Ok(Self {
            session,
            jumps,
            bind,
            index,
            identity,
            policy,
        })
    }
}

struct Entry {
    status: ForwardStatus,
    // Only private reuse digests and the ready address enter the registry;
    // prepared sessions, credentials and parsed keys stay in the owned worker.
    identity: Mutex<Option<ReuseIdentity>>,
    proxy_url: Mutex<String>,
    expires: Option<Instant>,
    state: watch::Receiver<&'static str>,
    stop: watch::Sender<bool>,
    // Retained across a timed-out or cancelled release; no early successful ACK.
    task: tokio::sync::Mutex<Option<JoinHandle<bool>>>,
    completed: AtomicBool,
    unconfirmed: AtomicBool,
    finished_at: Mutex<Option<Instant>>,
}
impl Entry {
    fn status(&self) -> ForwardStatus {
        self.reap();
        let mut status = self.status.clone();
        let state = *self.state.borrow();
        status.state = if self.unconfirmed.load(Ordering::Acquire) {
            "cleanup_unconfirmed"
        } else if self.completed.load(Ordering::Acquire) {
            if *self.stop.borrow() {
                "released"
            } else {
                state
            }
        } else if *self.stop.borrow() || !matches!(state, "preparing" | "starting" | "running") {
            // A terminal worker notification is not yet a confirmed join.
            "stopping"
        } else {
            state
        }
        .into();
        status.proxy_url = if status.state == "running" {
            self.proxy_url
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        } else {
            String::new()
        };
        status
    }
    fn joined(&self, result: std::result::Result<bool, tokio::task::JoinError>) -> Result<()> {
        if !matches!(result, Ok(true)) {
            self.unconfirmed.store(true, Ordering::Release);
            return Err(anyhow!(
                "cleanup_unconfirmed: dynamic owner failed to drain"
            ));
        }
        *self.finished_at.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        self.completed.store(true, Ordering::Release);
        Ok(())
    }
    /// Poll only an already-finished worker, through a borrow of its handle. An
    /// unexpected Pending keeps the handle owned; status never blocks on a join.
    fn reap(&self) {
        let Ok(mut task) = self.task.try_lock() else {
            return;
        };
        let Some(worker) = task.as_mut().filter(|worker| worker.is_finished()) else {
            return;
        };
        if let Some(result) = (&mut *worker).now_or_never() {
            task.take();
            let _ = self.joined(result);
        }
    }
    async fn drain(&self) -> Result<()> {
        self.drain_until(Instant::now() + Duration::from_secs(5))
            .await
    }
    async fn drain_until(&self, deadline: Instant) -> Result<()> {
        self.stop.send_replace(true);
        // The total budget includes waiting for another release's join lock.
        // Timeout drops only this borrow; the entry keeps the worker handle.
        tokio::time::timeout_at(deadline, async {
            let mut task = self.task.lock().await;
            if self.completed.load(Ordering::Acquire) {
                return Ok(());
            }
            let joined = match task.as_mut() {
                Some(task) => task.await,
                None => {
                    return Err(anyhow!(
                        "cleanup_unconfirmed: dynamic owner failed to drain"
                    ))
                }
            };
            task.take();
            self.joined(joined)
        })
        .await
        .map_err(|_| anyhow!("cleanup_unconfirmed: dynamic owner is still stopping"))?
    }
}
impl Drop for Entry {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}

/// A pending caller owns cancellation, not the worker's raw JoinHandle. Dropping
/// the request asks its retained lifecycle worker to stop and drain clients.
struct Pending(Arc<Entry>, bool);
impl Drop for Pending {
    fn drop(&mut self) {
        if self.1 {
            self.0.stop.send_replace(true);
        }
    }
}

/// One trusted frontend authorization domain. `ensure` is idempotent within
/// this owner, not an additional lease; releasing its handle releases the one
/// listener. Independent frontends must not share this object implicitly.
/// No public/native/wire constructor is exposed in this core-only slice.
pub(crate) struct ForwardOwner {
    closed: AtomicBool,
    shutdown: watch::Sender<bool>,
    enforce_permissions: bool,
    entries: Mutex<HashMap<String, Arc<Entry>>>,
}
impl ForwardOwner {
    pub(crate) fn unattended() -> Self {
        Self::new(true)
    }
    fn new(enforce_permissions: bool) -> Self {
        Self {
            closed: AtomicBool::new(false),
            shutdown: watch::channel(false).0,
            enforce_permissions,
            entries: Mutex::new(HashMap::new()),
        }
    }
    fn permitted(&self) -> bool {
        permitted(self.enforce_permissions)
    }

    /// Inputs must come from the trusted profile/config service, never directly
    /// from untrusted tool arguments. Generation is that service's authorization
    /// cycle, not a user-controlled way to upgrade an existing route.
    pub(crate) async fn start(
        &self,
        session: Session,
        jumps: Vec<Session>,
        index: usize,
        generation: u64,
        reuse: bool,
        expires: Option<Instant>,
    ) -> Result<ForwardStatus> {
        ensure!(
            !self.closed.load(Ordering::Acquire),
            "dynamic owner is closed"
        );
        ensure!(
            self.permitted(),
            "saved-credential dynamic forwarding is not authorized"
        );
        ensure!(
            expires.is_none_or(|end| end > Instant::now()),
            "dynamic authorization expired"
        );
        // Preserve the existing trusted start/reuse contract. The new begin
        // entry point below instead owns slow preparation after registration.
        let prepared = PreparedForward::capture(session, jumps, index, generation)?;
        let session_id = prepared.session.id.clone();
        // Recheck after potentially slow policy capture, but never under the
        // registry lock. The worker repeats admission before opening SSH.
        ensure!(
            self.permitted(),
            "saved-credential dynamic forwarding is not authorized"
        );
        ensure!(
            expires.is_none_or(|end| end > Instant::now()),
            "dynamic authorization expired"
        );
        let entry = {
            let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            Self::prune(&mut entries, Instant::now());
            ensure!(
                !self.closed.load(Ordering::Acquire),
                "dynamic owner is closed"
            );
            if let Some(entry) = Self::existing(&entries, &session_id, index) {
                ensure!(reuse, "dynamic tunnel already exists; ensure or release it");
                let identity = entry.identity.lock().unwrap_or_else(|e| e.into_inner());
                let identity = identity
                    .as_ref()
                    .ok_or_else(|| anyhow!("dynamic tunnel is still preparing"))?;
                ensure!(
                    identity.reusable && prepared.identity.reusable,
                    "path-backed credential reuse is unavailable; release and start explicitly"
                );
                ensure!(
                    identity.fingerprint == prepared.identity.fingerprint
                        && identity.policy == prepared.identity.policy
                        && identity.generation == generation
                        && entry.expires == expires,
                    "dynamic configuration or authorization changed; release before restarting"
                );
                let status = entry.status();
                ensure!(
                    status.state == "running" && !*entry.stop.borrow(),
                    "dynamic tunnel is not running; release before restarting"
                );
                return Ok(status);
            }
            self.register(&mut entries, session_id, index, expires, move || {
                Ok(prepared)
            })?
        };
        let mut pending = Pending(entry.clone(), true);
        let mut state = entry.state.clone();
        loop {
            let phase = *state.borrow_and_update();
            if phase == "running" {
                if self.closed.load(Ordering::Acquire)
                    || !self.permitted()
                    || *entry.stop.borrow()
                    || expires.is_some_and(|end| end <= Instant::now())
                {
                    break;
                }
                let status = entry.status();
                if status.state != "running" {
                    break;
                }
                pending.1 = false;
                return Ok(status);
            }
            if !matches!(phase, "preparing" | "starting") || state.changed().await.is_err() {
                break;
            }
        }
        entry.drain().await?;
        // Compatibility start never returned this failed handle to its caller.
        // Explicit begin, in contrast, keeps its queryable failure tombstone.
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&entry.status.handle);
        Err(anyhow!("dynamic startup failed or was cancelled; verify saved credentials, trusted keys and route"))
    }

    /// Register before slow work. The trusted closure is one synchronous owned
    /// preparation job: it must not spawn detached work or bind/connect itself.
    /// Its result is validated against this saved selector before any listener.
    /// No public/native/wire adapter or caller-provided authorization is added.
    pub(crate) fn begin(
        &self,
        session_id: String,
        index: usize,
        expires: Option<Instant>,
        prepare: impl FnOnce() -> Result<PreparedForward> + Send + 'static,
    ) -> Result<ForwardStatus> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        Self::prune(&mut entries, Instant::now());
        ensure!(
            !self.closed.load(Ordering::Acquire),
            "dynamic owner is closed"
        );
        ensure!(
            Self::existing(&entries, &session_id, index).is_none(),
            "dynamic tunnel already exists; ensure or release it"
        );
        let entry = self.register(&mut entries, session_id, index, expires, prepare)?;
        // This is acceptance, never a claim that authorization/SSH is ready.
        Ok(entry.status.clone())
    }
    fn existing<'a>(
        entries: &'a HashMap<String, Arc<Entry>>,
        session_id: &str,
        index: usize,
    ) -> Option<&'a Arc<Entry>> {
        entries.values().find(|entry| {
            !entry.completed.load(Ordering::Acquire)
                && entry.status.session_id == session_id
                && entry.status.forward_index == index
        })
    }
    fn register(
        &self,
        entries: &mut HashMap<String, Arc<Entry>>,
        session_id: String,
        index: usize,
        expires: Option<Instant>,
        prepare: impl FnOnce() -> Result<PreparedForward> + Send + 'static,
    ) -> Result<Arc<Entry>> {
        ensure!(
            entries
                .values()
                .filter(|entry| !entry.completed.load(Ordering::Acquire))
                .count()
                < LIVE_LIMIT,
            "dynamic owner limit reached; release unused handles"
        );
        ensure!(
            expires.is_none_or(|end| end > Instant::now()),
            "dynamic authorization expired"
        );
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| anyhow!("dynamic owner requires a runtime"))?;
        let (state_tx, state) = watch::channel("preparing");
        let (stop, _) = watch::channel(false);
        let entry = Arc::new(Entry {
            status: ForwardStatus {
                handle: uuid::Uuid::new_v4().to_string(),
                session_id,
                forward_index: index,
                proxy_url: String::new(),
                state: "preparing".into(),
                ownership: "current-owner-only; no external listener adoption",
            },
            identity: Mutex::new(None),
            proxy_url: Mutex::new(String::new()),
            expires,
            state,
            stop,
            task: tokio::sync::Mutex::new(None),
            completed: AtomicBool::new(false),
            unconfirmed: AtomicBool::new(false),
            finished_at: Mutex::new(None),
        });
        // Registration has no await or disk/network work. The sole map lock
        // excludes shutdown until the complete worker handle has been retained.
        entries.insert(entry.status.handle.clone(), entry.clone());
        let task = runtime.spawn(prepare_worker(
            entry.clone(),
            state_tx,
            self.shutdown.subscribe(),
            self.enforce_permissions,
            prepare,
        ));
        *entry.task.try_lock().expect("unpublished worker handle") = Some(task);
        Ok(entry)
    }
    fn prune(entries: &mut HashMap<String, Arc<Entry>>, now: Instant) {
        // Cancelled startup records are reclaimed only after their actual worker
        // has joined; a live/stopping bind is never forgotten to make space.
        let mut terminal = Vec::new();
        for (handle, entry) in entries.iter() {
            entry.reap();
            if entry.completed.load(Ordering::Acquire) {
                if let Some(at) = *entry.finished_at.lock().unwrap_or_else(|e| e.into_inner()) {
                    terminal.push((at, handle.clone()));
                }
            }
        }
        terminal.sort_unstable();
        let excess = terminal.len().saturating_sub(TERMINAL_LIMIT);
        for (index, (at, handle)) in terminal.into_iter().enumerate() {
            if index < excess || now.saturating_duration_since(at) >= TERMINAL_TTL {
                entries.remove(&handle);
            }
        }
    }
    fn entry(&self, handle: &str) -> Result<Arc<Entry>> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        Self::prune(&mut entries, Instant::now());
        entries
            .get(handle)
            .cloned()
            .ok_or_else(|| anyhow!("tunnel handle not found in this owner"))
    }
    pub(crate) fn status(&self, handle: &str) -> Result<ForwardStatus> {
        Ok(self.entry(handle)?.status())
    }
    /// Only requests cancellation. A stopping/unconfirmed result is not a
    /// release acknowledgement; status can harvest the eventual confirmed join.
    pub(crate) fn request_release(&self, handle: &str) -> Result<ForwardStatus> {
        let entry = self.entry(handle)?;
        entry.stop.send_replace(true);
        Ok(entry.status())
    }
    pub(crate) async fn release(&self, handle: &str) -> Result<ForwardStatus> {
        let entry = self.entry(handle)?;
        entry.drain().await?;
        Ok(entry.status())
    }
    pub(crate) async fn shutdown(&self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        self.closed.store(true, Ordering::Release);
        self.shutdown.send_replace(true);
        let entries: Vec<_> = {
            let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            Self::prune(&mut entries, Instant::now());
            entries.values().cloned().collect()
        };
        for entry in entries {
            entry.drain_until(deadline).await?;
        }
        Ok(())
    }
}
impl Drop for ForwardOwner {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
        self.shutdown.send_replace(true);
        // Async shutdown is required for an acknowledgement. Drop only requests
        // cancellation; workers retain their listener until its drain finishes.
    }
}
/// No configuration or other I/O belongs in preparation cancellation polling.
fn preparation_cancel_reason(
    stopped: &watch::Receiver<bool>,
    shutdown: &watch::Receiver<bool>,
    expires: Option<Instant>,
) -> Option<&'static str> {
    if *stopped.borrow() {
        Some("released")
    } else if *shutdown.borrow() {
        Some("owner_closed")
    } else if expires.is_some_and(|end| end <= Instant::now()) {
        Some("authorization_expired")
    } else {
        None
    }
}

/// The preparation JoinHandle never escapes this retained worker. Cancellation
/// stops admission immediately but joins the synchronous job before reporting a
/// terminal outcome; a late prepared route is dropped without binding its port.
async fn prepare_worker(
    entry: Arc<Entry>,
    state: watch::Sender<&'static str>,
    shutdown: watch::Receiver<bool>,
    enforce: bool,
    prepare: impl FnOnce() -> Result<PreparedForward> + Send + 'static,
) -> bool {
    let mut stopped = entry.stop.subscribe();
    let mut owner_closed = shutdown.clone();
    let expires = entry.expires;
    if let Some(reason) = preparation_cancel_reason(&stopped, &owner_closed, expires) {
        state.send_replace(reason);
        return true;
    }
    let mut job = tokio::task::spawn_blocking(move || {
        ensure!(
            permitted(enforce),
            "saved-credential dynamic forwarding is not authorized"
        );
        prepare()
    });
    let cancellation = async {
        loop {
            if let Some(reason) = preparation_cancel_reason(&stopped, &owner_closed, expires) {
                break reason;
            }
            tokio::select! {
                _ = stopped.changed() => {},
                _ = owner_closed.changed() => {},
                _ = tokio::time::sleep(Duration::from_millis(100)) => {},
            }
        }
    };
    tokio::pin!(cancellation);
    let prepared = tokio::select! {
        biased;
        reason = &mut cancellation => {
            state.send_replace("stopping");
            let joined = job.await;
            // Drop any successful result before terminal notification/return.
            let confirmed = joined.is_ok();
            drop(joined);
            state.send_replace(if confirmed { reason } else { "cleanup_unconfirmed" });
            return confirmed;
        },
        result = &mut job => result,
    };
    let prepared = match prepared {
        Ok(Ok(prepared)) => prepared,
        Ok(Err(_)) => {
            state.send_replace("failed");
            return true;
        }
        Err(_) => {
            state.send_replace("cleanup_unconfirmed");
            return false;
        }
    };
    if let Some(reason) = preparation_cancel_reason(&entry.stop.subscribe(), &shutdown, expires) {
        drop(prepared);
        state.send_replace(reason);
        return true;
    }
    if prepared.session.id != entry.status.session_id
        || prepared.index != entry.status.forward_index
    {
        state.send_replace("failed");
        return true;
    }
    state.send_replace("starting");
    let listener = match TcpListener::bind(prepared.bind).await {
        Ok(listener) => listener,
        Err(_) => {
            state.send_replace("failed");
            return true;
        }
    };
    let address = match listener.local_addr() {
        Ok(address) => address,
        Err(_) => {
            state.send_replace("failed");
            return true;
        }
    };
    *entry.proxy_url.lock().unwrap_or_else(|e| e.into_inner()) = format!("socks5h://{address}");
    *entry.identity.lock().unwrap_or_else(|e| e.into_inner()) = Some(prepared.identity);
    owned_worker(
        prepared.policy,
        state.clone(),
        run(
            prepared.session,
            prepared.jumps,
            listener,
            state,
            entry.stop.subscribe(),
            shutdown,
            enforce,
            expires,
        ),
    )
    .await
}

fn permitted(enforce: bool) -> bool {
    !enforce
        || crate::config::ConfigStore::load().is_ok_and(|store| {
            store.mcp_enabled() && store.mcp_use_saved_credentials() && store.mcp_allow_commands()
        })
}
async fn owned_worker(
    policy: crate::ssh::headless_policy::Policy,
    state: watch::Sender<&'static str>,
    operation: impl std::future::Future<Output = (&'static str, bool)>,
) -> bool {
    let (reason, confirmed) = policy
        .run(async {
            // Even an operation panic must pass through Policy::run's owned
            // loader joins. A panic remains unconfirmed, never a clean ACK.
            let outcome = std::panic::AssertUnwindSafe(operation)
                .catch_unwind()
                .await
                .unwrap_or(("cleanup_unconfirmed", false));
            // The route is gone, but a cancelled blocking key loader can still be
            // live. Publish a terminal state only after its join.
            state.send_replace("stopping");
            outcome
        })
        .await
        .unwrap_or(("cleanup_unconfirmed", false));
    state.send_replace(if confirmed {
        reason
    } else {
        "cleanup_unconfirmed"
    });
    confirmed
}
async fn cancelled(
    mut stop: watch::Receiver<bool>,
    mut shutdown: watch::Receiver<bool>,
    enforce: bool,
    expires: Option<Instant>,
) -> &'static str {
    loop {
        if *stop.borrow() {
            return "released";
        }
        if *shutdown.borrow() {
            return "owner_closed";
        }
        if !permitted(enforce) {
            return "permission_revoked";
        }
        if expires.is_some_and(|end| end <= Instant::now()) {
            return "authorization_expired";
        }
        tokio::select! {
            _ = stop.changed() => {},
            _ = shutdown.changed() => {},
            _ = tokio::time::sleep(Duration::from_millis(100)) => {},
        }
    }
}
async fn run(
    session: Session,
    jumps: Vec<Session>,
    listener: TcpListener,
    state: watch::Sender<&'static str>,
    stopped: watch::Receiver<bool>,
    shutdown: watch::Receiver<bool>,
    enforce: bool,
    expires: Option<Instant>,
) -> (&'static str, bool) {
    crate::ssh::connection::with_automation_cancellation(async move {
        let cancellation = cancelled(stopped, shutdown, enforce, expires);
        tokio::pin!(cancellation);
        let connection = tokio::select! {
            biased;
            reason = &mut cancellation => return (reason, true),
            connection = tokio::time::timeout(Duration::from_secs(60),
                crate::ssh::open_forward_connection(&session, &jumps)) => connection,
        };
        let (handle, jump_handles) = match connection {
            Ok(Ok(route)) => route,
            _ => return ("failed", true),
        };
        if !permitted(enforce) || expires.is_some_and(|end| end <= Instant::now())
            || !crate::ssh::headless_policy::still_current(&session, &jumps) {
            return ("authorization_changed", true);
        }
        let handle = Arc::new(handle);
        let listener = super::forward::start_dynamic_bound(handle.clone(), listener);
        state.send_replace("running");
        let reason = tokio::select! {
            biased;
            reason = &mut cancellation => reason,
            _ = async { while !listener.is_finished() { tokio::time::sleep(Duration::from_millis(50)).await; } } => "disconnected",
        };
        state.send_replace("stopping");
        // Unlike aborting the enclosing future, this acknowledges the shared
        // accept loop has joined every owned SOCKS client and released its bind.
        let drained = listener.stop().await;
        crate::ssh::disconnect_ssh(&handle, "dynamic owner stopped").await;
        for hop in &jump_handles { crate::ssh::disconnect_ssh(hop, "dynamic owner stopped").await; }
        drop(handle); drop(jump_handles);
        // The enclosing transport group is cancelled on return. This does not
        // claim joins of russh's private detached implementation tasks.
        let confirmed = match drained { Ok(()) => true, Err(error) => error.kind() == std::io::ErrorKind::ConnectionAborted };
        (reason, confirmed)
    }).await
}
fn loopback_bind(rule: &PortForward) -> Result<SocketAddr> {
    ensure!(
        rule.kind == "dynamic",
        "selected forward rule is not dynamic SOCKS5"
    );
    let text = rule
        .bind_addr
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']');
    let address: IpAddr = match text {
        "" | "localhost" => "127.0.0.1".parse().unwrap(),
        _ => text
            .parse()
            .map_err(|_| anyhow!("dynamic tunnel bind must be a loopback IP address"))?,
    };
    // Unauthenticated SOCKS must never silently expose a saved wildcard bind to
    // a network. Broader listening needs a separately designed access policy.
    ensure!(
        address.is_loopback(),
        "unattended dynamic tunnels require a loopback bind address"
    );
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

fn route_fingerprint(session: &Session, jumps: &[Session], index: usize) -> Result<[u8; 32]> {
    let rule = session
        .forwards
        .get(index)
        .ok_or_else(|| anyhow!("saved forward rule not found"))?;
    let route: Vec<_> = jumps
        .iter()
        .chain(std::iter::once(session))
        .enumerate()
        .map(|(i, hop)| RouteHop {
            id: &hop.id,
            host: &hop.host,
            port: hop.port,
            user: &hop.user,
            auth: hop.auth,
            password: &hop.password,
            private_key_path: &hop.private_key_path,
            private_key_inline: &hop.private_key_inline,
            // Jump chains are immediate-first: the last jump opens TCP/proxy.
            // Without jumps, the target at index zero opens TCP. All other
            // hops use direct-tcpip channels and do not consume saved proxies.
            proxy: if i == jumps.len().saturating_sub(1) {
                &hop.proxy
            } else {
                ""
            },
        })
        .collect();
    let bytes = zeroize::Zeroizing::new(serde_json::to_vec(&(route, loopback_bind(rule)?))?);
    Ok(Sha256::digest(bytes.as_slice()).into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unattended_bind_is_loopback_only() {
        for bind in ["", "localhost", "127.0.0.1", "::1", "[::1]"] {
            let rule = PortForward {
                kind: "dynamic".into(),
                bind_addr: bind.into(),
                ..Default::default()
            };
            assert!(loopback_bind(&rule).unwrap().ip().is_loopback());
        }
        for bind in ["0.0.0.0", "::", "192.0.2.1", "proxy.example"] {
            let rule = PortForward {
                kind: "dynamic".into(),
                bind_addr: bind.into(),
                ..Default::default()
            };
            assert!(loopback_bind(&rule).is_err());
        }
    }

    fn fixture_session(port: u16) -> Session {
        let mut session = Session::new_empty();
        session.host = "target.invalid".into();
        session.forwards.push(PortForward {
            kind: "dynamic".into(),
            bind_port: port,
            ..Default::default()
        });
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
    async fn pending_start(
        expires: Option<Instant>,
    ) -> (
        Arc<ForwardOwner>,
        JoinHandle<Result<ForwardStatus>>,
        TcpListener,
        SocketAddr,
    ) {
        let silent = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let reserve = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bind = reserve.local_addr().unwrap();
        drop(reserve);
        let mut session = fixture_session(bind.port());
        session.proxy = format!("http://{}", silent.local_addr().unwrap());
        let owner = Arc::new(ForwardOwner::new(false));
        let worker = owner.clone();
        let start =
            tokio::spawn(async move { worker.start(session, vec![], 0, 1, false, expires).await });
        (owner, start, silent, bind)
    }

    async fn assert_listener_released(bind: SocketAddr) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if TcpListener::bind(bind).await.is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("startup left the dynamic listener bound");
    }

    #[test]
    fn cancelled_start_rolls_back_listener_and_pending_proxy_transport() {
        super::lifecycle_tests::isolated(
            "tunnel::owner::tests::cancelled_start_rolls_back_listener_and_pending_proxy_transport",
            || async {
                use tokio::io::AsyncReadExt;
                let (_owner, start, silent, bind) = pending_start(None).await;
                let (mut transport, _) =
                    tokio::time::timeout(Duration::from_secs(2), silent.accept())
                        .await
                        .unwrap()
                        .unwrap();
                start.abort();
                let _ = start.await;
                assert_listener_released(bind).await;
                let mut bytes = Vec::new();
                tokio::time::timeout(Duration::from_secs(2), transport.read_to_end(&mut bytes))
                    .await
                    .expect("cancelled startup left an SSH proxy transport open")
                    .unwrap();
            },
        );
    }

    #[test]
    fn expired_start_rolls_back_without_ever_becoming_ready() {
        super::lifecycle_tests::isolated(
            "tunnel::owner::tests::expired_start_rolls_back_without_ever_becoming_ready",
            || async {
                let (_owner, start, silent, bind) =
                    pending_start(Some(Instant::now() + Duration::from_millis(200))).await;
                let (_transport, _) = tokio::time::timeout(Duration::from_secs(2), silent.accept())
                    .await
                    .unwrap()
                    .unwrap();
                let result = tokio::time::timeout(Duration::from_secs(2), start)
                    .await
                    .unwrap()
                    .unwrap();
                assert!(result.is_err());
                assert_listener_released(bind).await;
            },
        );
    }

    #[test]
    fn owner_shutdown_cancels_a_registered_pending_start() {
        super::lifecycle_tests::isolated(
            "tunnel::owner::tests::owner_shutdown_cancels_a_registered_pending_start",
            || async {
                let (owner, start, silent, bind) = pending_start(None).await;
                let (_transport, _) = tokio::time::timeout(Duration::from_secs(2), silent.accept())
                    .await
                    .unwrap()
                    .unwrap();
                owner.shutdown().await.unwrap();
                let result = tokio::time::timeout(Duration::from_secs(2), start)
                    .await
                    .unwrap()
                    .unwrap();
                assert!(result.is_err());
                assert_listener_released(bind).await;
                assert!(owner
                    .entries
                    .lock()
                    .unwrap()
                    .values()
                    .all(|entry| entry.completed.load(Ordering::Acquire)));
            },
        );
    }
}

#[cfg(test)]
#[path = "owner_tests.rs"]
mod lifecycle_tests;
