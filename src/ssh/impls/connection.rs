//! Bounded SSH network stages. Human host-key decisions suspend the handshake
//! budget; credential and MFA prompts are deliberately outside network stages.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::{anyhow, Context as _, Result};
use futures::task::AtomicWaker;
use futures::FutureExt;
use russh::client::{self, Handle, Handler};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::watch;

// An automation call owns all of its target/jump transports, including those
// opened by a spawned SFTP worker. Unlike the handshake guard this remains
// armed after authentication, so cancellation interrupts stalled writes/rekeys.
#[derive(Default)]
struct TransportGroup {
    cancelled: bool,
    owned_producers: usize,
    owned_connections: usize,
    owned_tasks: Vec<futures::future::Shared<futures::future::BoxFuture<'static, CleanupOutcome>>>,
    streams: Vec<Weak<CancelState>>,
    active_streams: usize,
    drained: Arc<tokio::sync::Notify>,
}
tokio::task_local! { static AUTOMATION_TRANSPORTS: Arc<Mutex<TransportGroup>>; }
struct CancelGroupOnDrop(Arc<Mutex<TransportGroup>>);
impl Drop for CancelGroupOnDrop {
    fn drop(&mut self) {
        let mut group = self.0.lock().unwrap_or_else(|e| e.into_inner());
        group.cancelled = true;
        for state in group.streams.iter().filter_map(Weak::upgrade) {
            state.cancel();
        }
    }
}

pub(crate) async fn with_automation_cancellation<F: Future>(future: F) -> F::Output {
    let group = Arc::new(Mutex::new(TransportGroup::default()));
    let _guard = CancelGroupOnDrop(group.clone());
    AUTOMATION_TRANSPORTS.scope(group, future).await
}

/// Explicit cancellation scope for owners that must ACK physical transport
/// cleanup. The caller first joins its startup/client tasks, then drains this
/// scope. Drop-only cancellation remains a best-effort fallback, not an ACK.
pub(crate) struct DrainingTransportScope {
    group: Arc<Mutex<TransportGroup>>,
}

impl DrainingTransportScope {
    pub(crate) fn new() -> Self {
        Self { group: Arc::new(Mutex::new(TransportGroup::default())) }
    }

    pub(crate) async fn run<F: Future>(&self, future: F) -> F::Output {
        AUTOMATION_TRANSPORTS.scope(self.group.clone(), future).await
    }

    pub(crate) fn cancel(&self) {
        let mut group = self.group.lock().unwrap_or_else(|e| e.into_inner());
        group.cancelled = true;
        for state in group.streams.iter().filter_map(Weak::upgrade) { state.cancel(); }
    }

    /// Wait for the actual underlying streams to drop, not just their task's
    /// cancellation signal. Do not call while still owning a Handle/stream.
    /// No timeout is reported as success: an outer bounded caller may instead
    /// retain its stopping entry and return an explicit cleanup-pending error.
    pub(crate) async fn drain(&self) -> Result<()> {
        self.cancel();
        // Completed connect workers may register a session-cleanup task. Keep
        // joining newly registered tasks before declaring the scope drained.
        // Shared futures retain JoinHandles across a cancelled/timeout drain.
        let mut joined = 0;
        let mut confirmed = true;
        let notify = self.group.lock().unwrap_or_else(|e| e.into_inner()).drained.clone();
        loop {
            let changed = notify.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let (tasks, producers) = {
                let group = self.group.lock().unwrap_or_else(|e| e.into_inner());
                (group.owned_tasks[joined..].to_vec(), group.owned_producers)
            };
            if tasks.is_empty() {
                if producers == 0 { break; }
                changed.await;
                continue;
            }
            joined += tasks.len();
            for task in tasks { confirmed &= task.await == CleanupOutcome::Complete; }
        }
        let notify = self.group.lock().unwrap_or_else(|e| e.into_inner()).drained.clone();
        loop {
            let changed = notify.notified();
            tokio::pin!(changed);
            // Register before checking the count to avoid a lost final drop.
            changed.as_mut().enable();
            if self.group.lock().unwrap_or_else(|e| e.into_inner()).active_streams == 0 {
                anyhow::ensure!(confirmed, "cleanup_unconfirmed: SSH background task failed");
                return Ok(());
            }
            changed.await;
        }
    }

    pub(crate) async fn drain_until(&self, deadline: tokio::time::Instant) -> Result<()> {
        tokio::time::timeout_at(deadline, self.drain()).await
            .map_err(|_| anyhow!("cleanup_unconfirmed: SSH cleanup deadline expired"))?
    }
}

impl Drop for DrainingTransportScope {
    fn drop(&mut self) { self.cancel(); }
}

/// Declared after CancelStream's inner stream so its destructor runs only
/// after the OS socket/channel has actually been dropped (field drop order).
struct TransportCompletion(Arc<Mutex<TransportGroup>>);
impl Drop for TransportCompletion {
    fn drop(&mut self) {
        let mut group = self.0.lock().unwrap_or_else(|e| e.into_inner());
        group.active_streams -= 1;
        group.drained.notify_waiters();
    }
}

// Capture the task-local context before spawning; task locals do not inherit
// automatically. A late-starting child still observes an already-cancelled group.
pub(crate) fn inherit_automation_cancellation<F: Future>(
    future: F,
) -> impl Future<Output = F::Output> {
    let group = AUTOMATION_TRANSPORTS.try_with(Clone::clone).ok();
    async move {
        match group {
            Some(group) => AUTOMATION_TRANSPORTS.scope(group, future).await,
            None => future.await,
        }
    }
}

pub(crate) const SSH_NETWORK_TIMEOUT: Duration = Duration::from_secs(15);

/// One budget per stage, rather than a deadline for the whole login: several
/// healthy hops and an arbitrarily slow human decision must remain usable.
pub(crate) async fn network_stage<T, E, F>(label: &str, future: F) -> Result<T>
where
    E: Into<anyhow::Error>,
    F: Future<Output = std::result::Result<T, E>>,
{
    tokio::time::timeout(SSH_NETWORK_TIMEOUT, future)
        .await
        .map_err(|_| stage_timeout(label))?
        .map_err(Into::into)
        .with_context(|| label.to_owned())
}

/// A disconnect queues a local request but can still block behind a full
/// transport queue. Cleanup must not prevent reporting an already known result.
pub(crate) async fn disconnect_ssh<H: Handler>(handle: &Handle<H>, reason: &str) {
    let _ = tokio::time::timeout(
        Duration::from_secs(1),
        handle.disconnect(russh::Disconnect::ByApplication, reason, ""),
    )
    .await;
}

fn stage_timeout(label: &str) -> anyhow::Error {
    anyhow!(
        "{label} timed out after {} seconds",
        SSH_NETWORK_TIMEOUT.as_secs()
    )
}

#[derive(Clone)]
pub(crate) struct HostKeyWait(watch::Sender<bool>);

impl Default for HostKeyWait {
    fn default() -> Self {
        Self(watch::channel(false).0)
    }
}

impl HostKeyWait {
    pub(crate) fn pause(&self) -> HostKeyPause {
        self.0.send_replace(true);
        HostKeyPause(self.clone())
    }
}

pub(crate) struct HostKeyPause(HostKeyWait);

impl Drop for HostKeyPause {
    fn drop(&mut self) {
        let HostKeyWait(waiting) = &self.0;
        waiting.send_replace(false);
    }
}

pub(crate) trait HandshakeHandler: Handler {
    fn host_key_wait(&self) -> &HostKeyWait;
}

async fn prompt_aware_timeout<T>(
    duration: Duration,
    mut waiting: watch::Receiver<bool>,
    future: impl Future<Output = T>,
) -> std::result::Result<T, ()> {
    tokio::pin!(future);
    let mut remaining = duration;
    let mut watching = true;
    loop {
        let paused = *waiting.borrow_and_update();
        let started = tokio::time::Instant::now();
        let deadline = async {
            if paused {
                std::future::pending::<()>().await;
            } else {
                tokio::time::sleep(remaining).await;
            }
        };
        tokio::select! {
            // Process a completed operation / newly opened prompt before expiry.
            biased;
            result = &mut future => return Ok(result),
            changed = waiting.changed(), if watching => watching = changed.is_ok(),
            _ = deadline => return Err(()),
        }
        if !paused {
            remaining = remaining.saturating_sub(started.elapsed());
        }
    }
}

/// russh spawns its KEX task before connect_stream resolves. Dropping only the
/// connect future can orphan that task, so cancellation also wakes and closes
/// its underlying stream. Disarm the guard only after the handle is returned.
pub(crate) async fn connect_ssh_stream<H, S>(
    config: Arc<client::Config>,
    stream: S,
    handler: H,
    label: &str,
) -> Result<Handle<H>>
where
    H: HandshakeHandler + Send + 'static,
    H::Error: std::error::Error + Send + Sync + 'static,
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let waiting = handler.host_key_wait().0.subscribe();
    let (stream, mut cancel) = CancelStream::new(stream);
    let result = prompt_aware_timeout(
        SSH_NETWORK_TIMEOUT,
        waiting,
        client::connect_stream(config, stream, handler),
    )
    .await
    .map_err(|_| stage_timeout(label))?
    .with_context(|| label.to_owned())?;
    cancel.0 = None;
    Ok(result)
}

#[derive(Default)]
struct CancelState {
    cancelled: AtomicBool,
    changed: tokio::sync::Notify,
    reader: AtomicWaker,
    writer: AtomicWaker,
}

impl CancelState {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.changed.notify_waiters();
        self.reader.wake();
        self.writer.wake();
    }
}

struct CancelOnDrop(Option<Arc<CancelState>>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(state) = self.0.take() {
            state.cancel();
        }
    }
}

struct CancelStream<S> {
    inner: S,
    state: Arc<CancelState>,
    _completion: Option<TransportCompletion>,
}

impl<S> CancelStream<S> {
    fn new(inner: S) -> (Self, CancelOnDrop) {
        let state = Arc::new(CancelState::default());
        let completion = AUTOMATION_TRANSPORTS.try_with(|owner| {
            let mut group = owner.lock().unwrap_or_else(|e| e.into_inner());
            if group.cancelled {
                state.cancel();
            }
            group.streams.push(Arc::downgrade(&state));
            group.active_streams += 1;
            TransportCompletion(owner.clone())
        }).ok();
        (
            Self {
                inner,
                state: state.clone(),
                _completion: completion,
            },
            CancelOnDrop(Some(state)),
        )
    }

    fn check(&self, cx: &Context<'_>, read: bool) -> io::Result<()> {
        if read {
            self.state.reader.register(cx.waker());
        } else {
            self.state.writer.register(cx.waker());
        }
        if self.state.cancelled.load(Ordering::Acquire) {
            Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "SSH handshake cancelled",
            ))
        } else {
            Ok(())
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for CancelStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.check(cx, true)?;
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for CancelStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.check(cx, false)?;
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.check(cx, false)?;
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.check(cx, false)?;
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn unanswered_network_stage_expires() {
        let wait = HostKeyWait::default();
        let result = prompt_aware_timeout(
            Duration::from_millis(20),
            wait.0.subscribe(),
            std::future::pending::<()>(),
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn human_host_key_wait_does_not_expire_handshake_budget() {
        let wait = HostKeyWait::default();
        let prompt = wait.clone();
        let result =
            prompt_aware_timeout(Duration::from_millis(30), wait.0.subscribe(), async move {
                let pause = prompt.pause();
                tokio::time::sleep(Duration::from_millis(100)).await;
                drop(pause);
                tokio::time::sleep(Duration::from_millis(5)).await;
                42
            })
            .await;
        assert_eq!(result, Ok(42));
    }

    #[tokio::test]
    async fn handshake_budget_resumes_after_human_prompt() {
        let wait = HostKeyWait::default();
        let prompt = wait.clone();
        let result =
            prompt_aware_timeout(Duration::from_millis(20), wait.0.subscribe(), async move {
                let pause = prompt.pause();
                tokio::time::sleep(Duration::from_millis(60)).await;
                drop(pause);
                std::future::pending::<()>().await;
            })
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn cancelling_handshake_wakes_detached_transport_io() {
        let (left, _right) = tokio::io::duplex(64);
        let (mut stream, cancel) = CancelStream::new(left);
        let reader = tokio::spawn(async move { stream.read_u8().await });
        tokio::task::yield_now().await;
        drop(cancel);
        let result = tokio::time::timeout(Duration::from_secs(1), reader)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::ConnectionAborted);
    }

    struct TestHandler(HostKeyWait);

    impl Handler for TestHandler {
        type Error = russh::Error;
    }

    impl HandshakeHandler for TestHandler {
        fn host_key_wait(&self) -> &HostKeyWait {
            &self.0
        }
    }

    #[tokio::test]
    async fn dropping_inflight_handshake_closes_russh_kex_task() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let (stream, peer) = tokio::io::duplex(65536);
        let handshake = tokio::spawn(connect_ssh_stream(
            Arc::new(client::Config::default()),
            stream,
            TestHandler(HostKeyWait::default()),
            "fixture KEX",
        ));
        let mut peer = BufReader::new(peer);
        tokio::time::timeout(Duration::from_secs(2), async {
            let mut banner = String::new();
            peer.read_line(&mut banner).await.unwrap();
            assert!(banner.starts_with("SSH-2.0-"));
            peer.write_all(b"SSH-2.0-fixture\r\n").await.unwrap();
            let mut packet = [0; 65536];
            // Receiving KEX data proves russh has spawned its transport task.
            assert!(peer.read(&mut packet).await.unwrap() > 0);
            handshake.abort();
            assert!(matches!(handshake.await, Err(error) if error.is_cancelled()));
            loop {
                match peer.read(&mut packet).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        })
        .await
        .expect("cancelled KEX transport was orphaned");
    }
}

#[cfg(test)]
mod automation_cancellation_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn automation_end_cancels_established_transport_after_handshake_guard_disarms() {
        let (mut stream, _peer) = with_automation_cancellation(async {
            let (transport, peer) = tokio::io::duplex(8);
            let (stream, mut handshake_guard) = CancelStream::new(transport);
            handshake_guard.0 = None;
            (stream, peer)
        })
        .await;
        assert!(stream.read_u8().await.is_err());
    }

    #[tokio::test]
    async fn cancellation_wakes_stalled_background_writer() {
        let (writer, _peer) = with_automation_cancellation(async {
            let (transport, peer) = tokio::io::duplex(1);
            let (mut stream, mut handshake_guard) = CancelStream::new(transport);
            handshake_guard.0 = None;
            let writer = tokio::spawn(async move { stream.write_all(&[1; 1024]).await });
            tokio::task::yield_now().await;
            (writer, peer)
        })
        .await;
        assert!(tokio::time::timeout(Duration::from_secs(1), writer)
            .await
            .unwrap()
            .unwrap()
            .is_err());
    }

    #[tokio::test]
    async fn late_spawn_inherits_already_cancelled_group() {
        let child = with_automation_cancellation(async {
            inherit_automation_cancellation(async {
                let (transport, _peer) = tokio::io::duplex(8);
                let (mut stream, mut handshake_guard) = CancelStream::new(transport);
                handshake_guard.0 = None;
                stream.write_all(b"no").await
            })
        })
        .await;
        assert!(child.await.is_err());
    }
}

#[cfg(test)]
mod draining_transport_tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn drain_waits_for_actual_socket_drop_not_just_cancellation() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let stream = tokio::net::TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
        let (mut peer, _) = listener.accept().await.unwrap();
        let scope = DrainingTransportScope::new();
        let (mut stream, guard) = scope.run(async { CancelStream::new(stream) }).await;
        // Cancellation alone must not claim resources are already drained.
        assert!(tokio::time::timeout(Duration::from_millis(20), scope.drain()).await.is_err());
        let mut byte = [0];
        assert!(stream.read(&mut byte).await.is_err());
        drop(stream); drop(guard);
        tokio::time::timeout(Duration::from_secs(1), scope.drain()).await.unwrap().unwrap();
        assert_eq!(tokio::time::timeout(Duration::from_secs(1), peer.read(&mut byte)).await.unwrap().unwrap(), 0);
    }

    #[tokio::test]
    async fn drain_waits_for_every_nested_transport_and_already_cancelled_scope() {
        let scope = DrainingTransportScope::new();
        let (a, _) = tokio::io::duplex(32);
        let (b, _) = tokio::io::duplex(32);
        let (one, guard_one) = scope.run(async { CancelStream::new(a) }).await;
        scope.cancel();
        let (two, guard_two) = scope.run(async { CancelStream::new(b) }).await;
        drop(one); drop(guard_one);
        assert!(tokio::time::timeout(Duration::from_millis(20), scope.drain()).await.is_err());
        drop(two); drop(guard_two);
        tokio::time::timeout(Duration::from_secs(1), scope.drain()).await.unwrap().unwrap();
    }
}

/// Normalized join evidence only. Never cache arbitrary error text (which may
/// contain remote-controlled data) in the scope's shared result futures.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CleanupOutcome { Complete, Unconfirmed }

fn session_join_outcome(result: std::result::Result<(), russh::Error>) -> CleanupOutcome {
    match result {
        Err(russh::Error::Join(_)) => CleanupOutcome::Unconfirmed,
        // Handle::Future was actually awaited: normal disconnect/I/O/protocol
        // errors still prove that task terminated. A Join failure does not.
        _ => CleanupOutcome::Complete,
    }
}

fn track_owned_task(group: &mut TransportGroup, task: tokio::task::JoinHandle<CleanupOutcome>) {
    group.owned_tasks.push(task.map(|result| result.unwrap_or(CleanupOutcome::Unconfirmed)).boxed().shared());
}

struct OwnedProducer(Arc<Mutex<TransportGroup>>);
impl Drop for OwnedProducer {
    fn drop(&mut self) {
        let mut group = self.0.lock().unwrap_or_else(|e| e.into_inner());
        group.owned_producers -= 1;
        group.drained.notify_waiters();
    }
}

// Per-session actors are bounded and retained across dropped caller futures.
// Cleanup seals admission before joining them and taking the raw SSH handle.
struct ManagedOpTasks {
    accepting: bool,
    failed: bool,
    #[cfg(test)]
    last_open_slot: Option<Weak<OpenedChannelSlot>>,
    tasks: Vec<futures::future::Shared<futures::future::BoxFuture<'static, CleanupOutcome>>>,
}
impl Default for ManagedOpTasks {
    fn default() -> Self { Self { accepting: true, failed: false, tasks: Vec::new(),
        #[cfg(test)] last_open_slot: None,
    } }
}
#[derive(Default)]
struct OpenedChannelState {
    channel: Option<russh::Channel<client::Msg>>,
    abandoned: bool,
    claimed: bool,
}
struct OpenedChannelSlot {
    state: Mutex<OpenedChannelState>,
    changed: tokio::sync::Notify,
}
/// Queued and claimed-but-not-consumed results both stay actor-owned. Only
/// synchronous conversion to a stream transfers the channel to a client task.
pub(crate) struct ManagedOpenedChannel { slot: Arc<OpenedChannelSlot> }
impl ManagedOpenedChannel {
    pub(crate) fn into_stream(self) -> std::result::Result<russh::ChannelStream<client::Msg>, russh::Error> {
        let mut state = self.slot.state.lock().unwrap_or_else(|e| e.into_inner());
        let channel = state.channel.take().ok_or(russh::Error::Disconnect)?;
        state.claimed = true;
        self.slot.changed.notify_one();
        Ok(channel.into_stream())
    }
}
impl Drop for ManagedOpenedChannel {
    fn drop(&mut self) {
        self.slot.state.lock().unwrap_or_else(|e| e.into_inner()).abandoned = true;
        self.slot.changed.notify_one();
    }
}
async fn wait_transport_cancelled(state: &CancelState) {
    loop {
        let changed = state.changed.notified();
        tokio::pin!(changed); changed.as_mut().enable();
        if state.cancelled.load(Ordering::Acquire) { return; }
        changed.await;
    }
}

/// Never exposes or transfers the raw russh Handle. The scope owns the cleanup
/// actor before this guard can be published. Dropping a queued, claimed or
/// active guard all cancel the same actor; successful oneshot send is NOT an
/// ownership-transfer acknowledgement.
pub(crate) struct ManagedSshConnection<H: Handler<Error = russh::Error> + Send + 'static> {
    slot: Arc<tokio::sync::RwLock<Option<Handle<H>>>>,
    operations: Arc<Mutex<ManagedOpTasks>>,
    cancellation: Arc<CancelState>,
}

impl<H: Handler<Error = russh::Error> + Send + 'static> ManagedSshConnection<H> {
    pub(crate) fn is_closed(&self) -> bool {
        if self.cancellation.cancelled.load(Ordering::Acquire) { return true; }
        match self.slot.try_read() {
            Ok(slot) => slot.as_ref().is_none_or(Handle::is_closed),
            Err(_) => false, // an in-flight bounded operation owns the lock
        }
    }

    pub(crate) async fn authenticate_saved(&self, session: &crate::config::Session,
        events: &tokio::sync::mpsc::UnboundedSender<crate::session::protocol::SessionEvent>) -> Result<bool> {
        let mut slot = self.slot.write().await;
        anyhow::ensure!(!self.cancellation.cancelled.load(Ordering::Acquire), "SSH connection is closing");
        let handle = slot.as_mut().ok_or_else(|| anyhow!("SSH connection is closed"))?;
        let (user, password) = crate::ssh::resolve_credentials(session, events).await
            .ok_or_else(|| anyhow!("saved SSH credentials are unavailable"))?;
        let password = zeroize::Zeroizing::new(password);
        crate::ssh::authenticate_once(handle, session, &user, &password, events).await
    }

    pub(crate) async fn channel_open_direct_tcpip(&self, host: &str, port: u16,
        origin: &str, origin_port: u16) -> std::result::Result<ManagedOpenedChannel, russh::Error> {
        self.channel_open_with_budget(host, port, origin, origin_port, SSH_NETWORK_TIMEOUT).await
    }

    async fn channel_open_with_budget(&self, host: &str, port: u16,
        origin: &str, origin_port: u16, budget: Duration)
        -> std::result::Result<ManagedOpenedChannel, russh::Error> {
        let deadline = tokio::time::Instant::now() + budget;
        let slot = Arc::new(OpenedChannelSlot { state: Mutex::new(OpenedChannelState::default()),
            changed: tokio::sync::Notify::new() });
        let pending = ManagedOpenedChannel { slot: slot.clone() };
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let handle_slot = self.slot.clone();
        let cancellation = self.cancellation.clone();
        let host = host.to_owned(); let origin = origin.to_owned();
        {
            let mut operations = self.operations.lock().unwrap_or_else(|e| e.into_inner());
            let mut failed = false;
            operations.tasks.retain(|task| match task.clone().now_or_never() {
                Some(result) => { failed |= result == CleanupOutcome::Unconfirmed; false }
                None => true,
            });
            operations.failed |= failed;
            if !operations.accepting || operations.failed || operations.tasks.len() >= 32
                || cancellation.cancelled.load(Ordering::Acquire) {
                return Err(russh::Error::Disconnect);
            }
            #[cfg(test)] { operations.last_open_slot = Some(Arc::downgrade(&slot)); }
            let actor = tokio::spawn(async move {
                // Read borrowing allows independent opens to proceed. Auth
                // and final cleanup retain exclusive write ownership.
                let open = async {
                    let handle = handle_slot.read().await;
                    let handle = handle.as_ref().ok_or(russh::Error::Disconnect)?;
                    handle.channel_open_direct_tcpip(host, port as u32, origin, origin_port as u32).await
                };
                let channel = tokio::select! {
                    biased;
                    _ = wait_transport_cancelled(&cancellation) => return CleanupOutcome::Complete,
                    result = tokio::time::timeout_at(deadline, open) => match result {
                        Ok(Ok(channel)) => channel,
                        Ok(Err(_)) => return CleanupOutcome::Complete,
                        Err(_) => { cancellation.cancel(); return CleanupOutcome::Complete; }
                    }
                };
                slot.state.lock().unwrap_or_else(|e| e.into_inner()).channel = Some(channel);
                let _ = sender.send(()); // a notification, never a raw Channel
                loop {
                    let changed = slot.changed.notified();
                    tokio::pin!(changed); changed.as_mut().enable();
                    let abandoned = {
                        let state = slot.state.lock().unwrap_or_else(|e| e.into_inner());
                        if state.claimed { return CleanupOutcome::Complete; }
                        state.abandoned
                    };
                    if abandoned { break; }
                    tokio::select! {
                        _ = changed => {},
                        _ = wait_transport_cancelled(&cancellation) => break,
                        _ = tokio::time::sleep_until(deadline) => {
                            // No acknowledgement by the handoff deadline.
                            // Treat as abandoned, not as transport failure.
                            break;
                        }
                    }
                }
                let channel = slot.state.lock().unwrap_or_else(|e| e.into_inner()).channel.take();
                if let Some(mut channel) = channel {
                    let close = async {
                        channel.close().await?;
                        while let Some(message) = channel.wait().await {
                            if matches!(message, russh::ChannelMsg::Close) { break; }
                        }
                        Ok::<(), russh::Error>(())
                    };
                    tokio::select! {
                        _ = wait_transport_cancelled(&cancellation) => {},
                        result = tokio::time::timeout(SSH_NETWORK_TIMEOUT, close) => {
                            if !matches!(result, Ok(Ok(()))) { cancellation.cancel(); }
                        }
                    }
                }
                CleanupOutcome::Complete
            });
            operations.tasks.push(actor.map(|result| result.unwrap_or(CleanupOutcome::Unconfirmed)).boxed().shared());
        }
        receiver.await.map_err(|_| russh::Error::Disconnect)?;
        Ok(pending)
    }

    // Narrow operations borrow the controlled handle; no public take/DerefMut
    // escape hatch can drop a raw handle between claim and owner registration.
    pub(crate) async fn authenticate_password(&self, user: &str, password: &str)
        -> Result<client::AuthResult> {
        anyhow::ensure!(!self.cancellation.cancelled.load(Ordering::Acquire), "SSH connection is closing");
        let mut slot = self.slot.write().await;
        anyhow::ensure!(!self.cancellation.cancelled.load(Ordering::Acquire), "SSH connection is closing");
        let handle = slot.as_mut().ok_or_else(|| anyhow!("SSH connection is closed"))?;
        Ok(handle.authenticate_password(user, password).await?)
    }
}

impl<H: Handler<Error = russh::Error> + Send + 'static> Drop for ManagedSshConnection<H> {
    fn drop(&mut self) { self.cancellation.cancel(); }
}

struct PendingManagedConnection<H: Handler<Error = russh::Error> + Send + 'static> {
    receiver: tokio::sync::oneshot::Receiver<std::result::Result<ManagedSshConnection<H>, CleanupOutcome>>,
    cancellation: Option<Arc<CancelState>>,
}

impl<H: Handler<Error = russh::Error> + Send + 'static> PendingManagedConnection<H> {
    async fn receive(mut self) -> Result<ManagedSshConnection<H>> {
        let guard = (&mut self.receiver).await.context("controlled SSH connection task ended")?
            .map_err(|_| anyhow!("controlled SSH handshake failed"))?;
        // There is no await between receiving the guard and disarming this
        // pending cancellation. If this future is dropped afterwards, the
        // returned guard itself still cancels the registered cleanup actor.
        self.cancellation.take();
        Ok(guard)
    }
}

impl<H: Handler<Error = russh::Error> + Send + 'static> Drop for PendingManagedConnection<H> {
    fn drop(&mut self) {
        if let Some(state) = &self.cancellation { state.cancel(); }
    }
}

impl DrainingTransportScope {
    async fn begin_connection<H, S>(&self, config: Arc<client::Config>, stream: S, handler: H)
        -> Result<PendingManagedConnection<H>>
    where
        H: HandshakeHandler + Handler<Error = russh::Error> + Send + 'static,
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let producer = {
            let mut group = self.group.lock().unwrap_or_else(|e| e.into_inner());
            anyhow::ensure!(!group.cancelled, "SSH owner is cancelled");
            anyhow::ensure!(group.owned_connections < 32, "SSH owned connection limit reached");
            group.owned_connections += 1;
            group.owned_producers += 1;
            OwnedProducer(self.group.clone())
        };
        let (stream, mut temporary_guard) = self.run(async { CancelStream::new(stream) }).await;
        let state = temporary_guard.0.take().expect("new stream cancellation state");
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let owned_group = self.group.clone();
        let owned_state = state.clone();
        {
            let mut group = self.group.lock().unwrap_or_else(|e| e.into_inner());
            anyhow::ensure!(!group.cancelled, "SSH owner is cancelled");
            // The producer reservation bounded the number of connections;
            // each contributes at most one connect and one session task.
            let task = tokio::spawn(async move {
                let result = client::connect_stream(config, stream, handler).await;
                let handle = match result {
                    Ok(handle) => handle,
                    Err(error) => {
                        let outcome = if matches!(&error, russh::Error::Join(_)) {
                            CleanupOutcome::Unconfirmed
                        } else { CleanupOutcome::Complete };
                        let _ = sender.send(Err(outcome));
                        return outcome;
                    }
                };
                let slot = Arc::new(tokio::sync::RwLock::new(Some(handle)));
                let operations = Arc::new(Mutex::new(ManagedOpTasks::default()));
                let cleanup_operations = operations.clone();
                let cleanup_slot = slot.clone();
                let cleanup_state = owned_state.clone();
                let cleanup = tokio::spawn(async move {
                    loop {
                        let cancelled = cleanup_state.changed.notified();
                        tokio::pin!(cancelled);
                        cancelled.as_mut().enable();
                        if cleanup_state.cancelled.load(Ordering::Acquire) { break; }
                        cancelled.await;
                    }
                    let (tasks, mut failed) = {
                        let mut operations = cleanup_operations.lock().unwrap_or_else(|e| e.into_inner());
                        operations.accepting = false;
                        (operations.tasks.clone(), operations.failed)
                    };
                    for task in tasks { failed |= task.await == CleanupOutcome::Unconfirmed; }
                    let mut slot = cleanup_slot.write().await;
                    let outcome = match slot.take() {
                        Some(handle) => session_join_outcome(handle.await),
                        None => CleanupOutcome::Unconfirmed,
                    };
                    if failed { CleanupOutcome::Unconfirmed } else { outcome }
                });
                // Register cleanup before publication, so both enqueue and
                // claim cancellation windows remain scope-owned and joinable.
                track_owned_task(&mut owned_group.lock().unwrap_or_else(|e| e.into_inner()), cleanup);
                let guard = ManagedSshConnection { slot, operations, cancellation: owned_state };
                let _ = sender.send(Ok(guard)); // failed send drops the guard
                CleanupOutcome::Complete
            });
            track_owned_task(&mut group, task);
        }
        drop(producer);
        Ok(PendingManagedConnection { receiver, cancellation: Some(state) })
    }

    /// New explicit API; existing callers returning raw Handle stay unchanged.
    /// Broker adapters must adopt this owned type before claiming joinable
    /// cancellation. A timeout drops PendingManagedConnection, not raw Handle.
    pub(crate) async fn connect_owned<H, S>(&self, config: Arc<client::Config>, stream: S, handler: H)
        -> Result<ManagedSshConnection<H>>
    where
        H: HandshakeHandler + Handler<Error = russh::Error> + Send + 'static,
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let pending = self.begin_connection(config, stream, handler).await?;
        tokio::time::timeout(SSH_NETWORK_TIMEOUT, pending.receive()).await
            .map_err(|_| anyhow!("controlled SSH handshake timed out"))?
    }
}

#[cfg(test)]
mod managed_connection_tests {
    use super::*;
    use russh::keys::{PrivateKey, PublicKey, ssh_key::private::Ed25519Keypair};

    struct FixtureServer;
    impl russh::server::Handler for FixtureServer { type Error = russh::Error; }
    struct FixtureClient { wait: HostKeyWait, key: PublicKey, panic_on_key: bool }
    impl Handler for FixtureClient {
        type Error = russh::Error;
        async fn check_server_key(&mut self, key: &PublicKey) -> std::result::Result<bool, Self::Error> {
            assert!(!self.panic_on_key, "synthetic handler panic");
            Ok(key.key_data() == self.key.key_data())
        }
    }
    impl HandshakeHandler for FixtureClient {
        fn host_key_wait(&self) -> &HostKeyWait { &self.wait }
    }

    struct OpenFixtureServer {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
        closed: Arc<tokio::sync::Notify>,
        clients: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    }
    impl russh::server::Handler for OpenFixtureServer {
        type Error = russh::Error;
        async fn auth_password(&mut self, user: &str, password: &str)
            -> std::result::Result<russh::server::Auth, Self::Error> {
            Ok(if user == "fixture" && password == "fixture-password" {
                russh::server::Auth::Accept
            } else { russh::server::Auth::reject() })
        }
        async fn channel_open_direct_tcpip(&mut self, channel: russh::Channel<russh::server::Msg>,
            host: &str, _port: u32, _origin: &str, _origin_port: u32,
            _session: &mut russh::server::Session) -> std::result::Result<bool, Self::Error> {
            let paused = host == "paused.invalid";
            if paused { self.entered.notify_one(); self.release.notified().await; }
            let closed = self.closed.clone();
            self.clients.lock().unwrap().push(tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut stream = channel.into_stream();
                let mut bytes = [0; 32];
                while let Ok(count) = stream.read(&mut bytes).await {
                    if count == 0 { break; }
                    if stream.write_all(&bytes[..count]).await.is_err() { break; }
                }
                if paused { closed.notify_one(); }
            }));
            Ok(true)
        }
    }

    #[derive(Default)]
    struct ReadyNotification(tokio::sync::Notify);
    impl std::task::Wake for ReadyNotification {
        fn wake(self: Arc<Self>) { self.0.notify_one(); }
        fn wake_by_ref(self: &Arc<Self>) { self.0.notify_one(); }
    }

    async fn assert_open_actors_complete(managed: &ManagedSshConnection<FixtureClient>) {
        let tasks = managed.operations.lock().unwrap().tasks.clone();
        for task in tasks {
            assert!(tokio::time::timeout(Duration::from_secs(3), task).await.unwrap() == CleanupOutcome::Complete);
        }
        assert!(!managed.cancellation.cancelled.load(Ordering::Acquire));
    }

    async fn unclaimed_notification_and_recycled_slots(managed: &ManagedSshConnection<FixtureClient>) {
        // Poll exactly once to register the real receiver waker, then never
        // poll again. That waker is reached only when the owned actor sends its
        // ready notification (or drops the sender); transport health and the
        // successful actor cleanup below distinguish success from failure.
        for _ in 0..40 {
            let mut request = Box::pin(managed.channel_open_direct_tcpip("echo.invalid", 23, "127.0.0.1", 5));
            let notification = Arc::new(ReadyNotification::default());
            let waker = std::task::Waker::from(notification.clone());
            assert!(request.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
            tokio::time::timeout(Duration::from_secs(3), notification.0.notified()).await.unwrap();
            let slot = managed.operations.lock().unwrap().last_open_slot.as_ref().unwrap().upgrade().unwrap();
            {
                let state = slot.state.lock().unwrap();
                assert!(state.channel.is_some());
                assert!(!state.claimed && !state.abandoned);
            }
            drop(request); // successful notification queued, receiver unpolled
            assert_open_actors_complete(managed).await;
        }
        assert!(managed.operations.lock().unwrap().tasks.len() <= 32);
    }

    async fn pending_open_capacity_recovers(managed: &ManagedSshConnection<FixtureClient>,
        entered: &tokio::sync::Notify, release: &tokio::sync::Notify) {
        assert_open_actors_complete(managed).await;
        let mut requests = Vec::new();
        let waker = futures::task::noop_waker();
        for index in 0..32 {
            let host = if index == 0 { "paused.invalid" } else { "echo.invalid" };
            let mut request = Box::pin(managed.channel_open_direct_tcpip(host, 23, "127.0.0.1", 6));
            assert!(request.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
            requests.push(request);
        }
        entered.notified().await;
        assert_eq!(managed.operations.lock().unwrap().tasks.len(), 32);
        assert!(managed.channel_open_direct_tcpip("echo.invalid", 23, "127.0.0.1", 7).await.is_err());
        assert!(!managed.is_closed()); // capacity rejection is not revocation
        drop(requests);
        release.notify_one();
        assert_open_actors_complete(managed).await;
        drop(managed.channel_open_direct_tcpip("echo.invalid", 23, "127.0.0.1", 8).await.unwrap());
        assert_open_actors_complete(managed).await;
    }

    #[tokio::test]
    async fn owned_open_actor_panic_is_sticky_normalized_cleanup_failure() {
        let scope = DrainingTransportScope::new();
        let (pending, server) = fixture(&scope, false).await;
        let managed = tokio::time::timeout(Duration::from_secs(3), pending.receive()).await.unwrap().unwrap();
        // Inject an actually panicking owned task into the operation ledger;
        // this tests the ledger's normalized join failure and sticky pruning,
        // not a fabricated russh network panic or a production trigger.
        let actor: tokio::task::JoinHandle<CleanupOutcome> = tokio::spawn(async { panic!("fixture-private-panic-payload") });
        let tracked = actor.map(|result| result.unwrap_or(CleanupOutcome::Unconfirmed)).boxed().shared();
        managed.operations.lock().unwrap().tasks.push(tracked.clone());
        assert!(tokio::time::timeout(Duration::from_secs(3), tracked).await.unwrap() == CleanupOutcome::Unconfirmed);
        for _ in 0..2 {
            assert!(managed.channel_open_direct_tcpip("fixture.invalid", 23, "127.0.0.1", 9).await.is_err());
            assert!(managed.operations.lock().unwrap().failed);
            let error = scope.drain_until(tokio::time::Instant::now() + Duration::from_secs(3)).await.unwrap_err();
            assert!(error.to_string().contains("cleanup_unconfirmed"));
            assert!(!error.to_string().contains("fixture-private-panic-payload"));
        }
        finish_server(server).await;
    }

    #[tokio::test]
    async fn abandoned_open_late_confirmation_preserves_another_active_echo() {
        exercise_managed_open(false).await;
    }
    #[tokio::test]
    async fn stalled_open_deadline_closes_unhealthy_transport_and_joins_actors() {
        exercise_managed_open(true).await;
    }
    async fn exercise_managed_open(expire: bool) {
        tokio::time::timeout(Duration::from_secs(30), async {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let scope = DrainingTransportScope::new();
            let key = PrivateKey::new(Ed25519Keypair::from_seed(&[35; 32]).into(), "open fixture").unwrap();
            let expected = key.public_key().clone();
            let entered = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            let closed = Arc::new(tokio::sync::Notify::new());
            let clients = Arc::new(Mutex::new(Vec::new()));
            let (client, server_stream) = tokio::io::duplex(65536);
            let handler = OpenFixtureServer { entered: entered.clone(), release: release.clone(),
                closed: closed.clone(), clients: clients.clone() };
            let server = tokio::spawn(async move {
                russh::server::run_stream(Arc::new(russh::server::Config {
                    keys: vec![key], ..Default::default()
                }), server_stream, handler).await?.await
            });
            let managed = Arc::new(scope.connect_owned(Arc::new(client::Config::default()), client,
                FixtureClient { wait: HostKeyWait::default(), key: expected, panic_on_key: false }).await.unwrap());
            assert!(managed.authenticate_password("fixture", "fixture-password").await.unwrap().success());
            let mut existing = managed.channel_open_direct_tcpip("echo.invalid", 23, "127.0.0.1", 1)
                .await.unwrap().into_stream().unwrap();
            existing.write_all(b"before").await.unwrap(); let mut echo = [0; 6];
            existing.read_exact(&mut echo).await.unwrap(); assert_eq!(&echo, b"before");
            let opening = managed.clone();
            let budget = if expire { Duration::from_millis(200) } else { Duration::from_secs(5) };
            let request = tokio::spawn(async move {
                opening.channel_open_with_budget("paused.invalid", 23, "127.0.0.1", 2, budget).await
            });
            entered.notified().await;
            if expire {
                assert!(tokio::time::timeout(Duration::from_secs(2), request).await.unwrap().unwrap().is_err());
                assert!(managed.is_closed());
                // A subsequent open cannot remain stuck behind the first.
                assert!(tokio::time::timeout(Duration::from_secs(1),
                    managed.channel_open_direct_tcpip("echo.invalid", 23, "127.0.0.1", 3)).await.unwrap().is_err());
                let mut byte = [0];
                assert_eq!(existing.read(&mut byte).await.unwrap(), 0);
            } else {
                request.abort(); assert!(matches!(request.await, Err(error) if error.is_cancelled()));
                assert!(!managed.is_closed());
                release.notify_one();
                // The abandoned A channel receives its late confirmation,
                // then the owned actor closes it. B remains usable afterwards.
                tokio::time::timeout(Duration::from_secs(2), closed.notified()).await.unwrap();
                // Observe the owned actor's actual join result before any
                // scope cancellation, not just the server worker's EOF.
                assert_open_actors_complete(&managed).await;
                existing.write_all(b"after!").await.unwrap();
                existing.read_exact(&mut echo).await.unwrap(); assert_eq!(&echo, b"after!");
                assert!(!managed.is_closed());
                // Claimed-result abandonment before into_stream also closes
                // only its own channel, never the reused transport.
                drop(managed.channel_open_direct_tcpip("echo.invalid", 23, "127.0.0.1", 4).await.unwrap());
                existing.write_all(b"again!").await.unwrap();
                existing.read_exact(&mut echo).await.unwrap(); assert_eq!(&echo, b"again!");
                assert_open_actors_complete(&managed).await;
                unclaimed_notification_and_recycled_slots(&managed).await;
                pending_open_capacity_recovers(&managed, &entered, &release).await;
                existing.write_all(b"slots!").await.unwrap();
                existing.read_exact(&mut echo).await.unwrap(); assert_eq!(&echo, b"slots!");
            }
            scope.drain_until(tokio::time::Instant::now() + Duration::from_secs(2)).await.unwrap();
            assert_eq!(scope.group.lock().unwrap().active_streams, 0);
            // In the expiry case the remote callback deliberately remains
            // paused until now. ACK proves local task/transport cleanup, not
            // control over remote callback execution.
            if expire { release.notify_one(); }
            finish_server(server).await;
            let clients = std::mem::take(&mut *clients.lock().unwrap());
            for client in clients { tokio::time::timeout(Duration::from_secs(2), client).await.unwrap().unwrap(); }
        }).await.expect("managed open fixture exceeded deadline");
    }

    #[tokio::test]
    async fn managed_open_budget_includes_waiting_for_exclusive_auth_slot() {
        let scope = DrainingTransportScope::new();
        let (pending, server) = fixture(&scope, false).await;
        let managed = tokio::time::timeout(Duration::from_secs(3), pending.receive()).await.unwrap().unwrap();
        let held = managed.slot.write().await;
        let result = tokio::time::timeout(Duration::from_secs(1),
            managed.channel_open_with_budget("fixture.invalid", 23, "127.0.0.1", 1, Duration::from_millis(50))).await.unwrap();
        assert!(result.is_err());
        assert!(managed.cancellation.cancelled.load(Ordering::Acquire));
        drop(held);
        scope.drain_until(tokio::time::Instant::now() + Duration::from_secs(3)).await.unwrap();
        finish_server(server).await;
    }

    #[tokio::test]
    async fn fixture_key_pin_ignores_comments_but_rejects_other_key_material() {
        let expected = PrivateKey::new(Ed25519Keypair::from_seed(&[31; 32]).into(), "managed fixture").unwrap();
        let other = PrivateKey::new(Ed25519Keypair::from_seed(&[33; 32]).into(), "managed fixture").unwrap();
        let wire_key = PublicKey::new(expected.public_key().key_data().clone(), "");
        let mut handler = FixtureClient { wait: HostKeyWait::default(), key: expected.public_key().clone(), panic_on_key: false };
        assert!(handler.check_server_key(&wire_key).await.unwrap());
        assert!(!handler.check_server_key(other.public_key()).await.unwrap());
    }

    async fn fixture(scope: &DrainingTransportScope, panic_on_key: bool)
        -> (PendingManagedConnection<FixtureClient>, tokio::task::JoinHandle<std::result::Result<(), russh::Error>>) {
        let key = PrivateKey::new(Ed25519Keypair::from_seed(&[31; 32]).into(), "managed fixture").unwrap();
        let expected = key.public_key().clone();
        let config = Arc::new(russh::server::Config { keys: vec![key], ..Default::default() });
        let (client, server) = tokio::io::duplex(65536);
        let server = tokio::spawn(async move {
            let session = russh::server::run_stream(config, server, FixtureServer).await?;
            session.await
        });
        let pending = scope.begin_connection(Arc::new(client::Config::default()), client,
            FixtureClient { wait: HostKeyWait::default(), key: expected, panic_on_key }).await.unwrap();
        (pending, server)
    }

    async fn finish_server(server: tokio::task::JoinHandle<std::result::Result<(), russh::Error>>) {
        // Transport disconnect is expected, but the server task itself must
        // complete; timeout/panic is not accepted as cleanup evidence.
        let transport_result = tokio::time::timeout(Duration::from_secs(3), server).await.unwrap().unwrap();
        assert!(!matches!(transport_result, Err(russh::Error::Join(_))), "server transport task failed to join");
    }

    #[tokio::test]
    async fn successful_enqueue_then_unpolled_receiver_drop_joins_session() {
        let scope = DrainingTransportScope::new();
        let (pending, server) = fixture(&scope, false).await;
        let published = scope.group.lock().unwrap().owned_tasks[0].clone();
        assert!(tokio::time::timeout(Duration::from_secs(3), published).await.unwrap() == CleanupOutcome::Complete);
        // Completion of the publishing worker is the deterministic barrier:
        // a successful KEX Handle is queued, and receiver has never been polled.
        assert_eq!(scope.group.lock().unwrap().owned_tasks.len(), 2);
        drop(pending);
        scope.drain_until(tokio::time::Instant::now() + Duration::from_secs(3)).await.unwrap();
        finish_server(server).await;
    }

    #[tokio::test]
    async fn successful_claim_then_drop_before_owner_registration_joins_session() {
        let scope = DrainingTransportScope::new();
        let (pending, server) = fixture(&scope, false).await;
        let claimed = tokio::time::timeout(Duration::from_secs(3), pending.receive()).await.unwrap().unwrap();
        // No owner registry has received this guard. Its drop still cancels
        // the cleanup actor registered before it crossed the channel boundary.
        drop(claimed);
        scope.drain_until(tokio::time::Instant::now() + Duration::from_secs(3)).await.unwrap();
        finish_server(server).await;
    }

    #[tokio::test]
    async fn real_russh_handler_panic_is_not_a_clean_shutdown() {
        let scope = DrainingTransportScope::new();
        let (pending, server) = fixture(&scope, true).await;
        assert!(tokio::time::timeout(Duration::from_secs(3), pending.receive()).await.unwrap().is_err());
        for _ in 0..2 {
            let error = scope.drain_until(tokio::time::Instant::now() + Duration::from_secs(3)).await.unwrap_err();
            assert!(error.to_string().contains("cleanup_unconfirmed"));
            assert!(!error.to_string().contains("synthetic handler panic"));
        }
        finish_server(server).await;
    }

    #[tokio::test]
    async fn bounded_drain_can_retry_without_detaching_its_owned_tasks() {
        let scope = DrainingTransportScope::new();
        let (pending, server) = fixture(&scope, false).await;
        let claimed = tokio::time::timeout(Duration::from_secs(3), pending.receive()).await.unwrap().unwrap();
        let held = claimed.slot.write().await;
        let error = scope.drain_until(tokio::time::Instant::now() + Duration::from_millis(10)).await.unwrap_err();
        assert!(error.to_string().contains("cleanup_unconfirmed"));
        drop(held); drop(claimed);
        scope.drain_until(tokio::time::Instant::now() + Duration::from_secs(3)).await.unwrap();
        finish_server(server).await;
    }
    #[tokio::test]
    async fn cancelled_pending_kex_retains_worker_until_internal_join_and_peer_close() {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
        let scope = DrainingTransportScope::new();
        let key = PrivateKey::new(Ed25519Keypair::from_seed(&[32; 32]).into(), "fixture").unwrap();
        let (stream, peer) = tokio::io::duplex(65536);
        let pending = scope.begin_connection(Arc::new(client::Config::default()), stream,
            FixtureClient { wait: HostKeyWait::default(), key: key.public_key().clone(), panic_on_key: false }).await.unwrap();
        let mut peer = BufReader::new(peer);
        tokio::time::timeout(Duration::from_secs(3), async {
            let mut banner = String::new(); peer.read_line(&mut banner).await.unwrap();
            assert!(banner.starts_with("SSH-2.0-"));
            peer.write_all(b"SSH-2.0-stalled-fixture\r\n").await.unwrap();
            let mut bytes = [0; 65536]; assert!(peer.read(&mut bytes).await.unwrap() > 0);
            drop(pending);
            scope.drain_until(tokio::time::Instant::now() + Duration::from_secs(2)).await.unwrap();
            loop { match peer.read(&mut bytes).await { Ok(0) | Err(_) => break, Ok(_) => {} } }
        }).await.unwrap();
        assert_eq!(scope.group.lock().unwrap().active_streams, 0);
    }

    #[tokio::test]
    async fn drain_does_not_complete_during_registered_producer_gap() {
        let scope = DrainingTransportScope::new();
        scope.group.lock().unwrap().owned_producers += 1;
        let producer = OwnedProducer(scope.group.clone());
        assert!(scope.drain_until(tokio::time::Instant::now() + Duration::from_millis(10)).await.is_err());
        drop(producer);
        scope.drain_until(tokio::time::Instant::now() + Duration::from_secs(1)).await.unwrap();
        assert!(scope.group.lock().unwrap().cancelled);
    }

}
