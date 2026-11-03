//! One broker-owned listener resource; the registry remains the sole lease and
//! bind-slot authority. Not yet connected to GUI/CLI/MCP production defaults.
use std::net::SocketAddr;
use std::sync::Arc;
use anyhow::{anyhow, ensure, Result};
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use crate::ssh::connection::DrainingTransportScope;
use super::snapshot::ResolvedRouteSnapshot;

pub(super) struct ManagedForward {
    stop: watch::Sender<bool>,
    scope: Arc<DrainingTransportScope>,
    task: Option<JoinHandle<Result<()>>>,
    ready: Option<oneshot::Receiver<SocketAddr>>,
    task_unconfirmed: bool,
    activation: Option<oneshot::Sender<()>>,
    finished: watch::Receiver<bool>,
    #[cfg(test)] bound: SocketAddr,
}

struct RuntimeCompletion(watch::Sender<bool>);
impl Drop for RuntimeCompletion {
    fn drop(&mut self) { self.0.send_replace(true); }
}

impl ManagedForward {
    #[cfg(test)]
    pub async fn start(snapshot: Arc<ResolvedRouteSnapshot>,
        authorized: Arc<dyn Fn() -> bool + Send + Sync>) -> Result<Self> {
        let mut resource = Self::prepare(snapshot, authorized).await?;
        resource.activate()?;
        Ok(resource)
    }

    /// Bind and authenticate, but do not accept clients before the coordinator
    /// atomically validates the authorization cycle and grants activation.
    pub async fn prepare(snapshot: Arc<ResolvedRouteSnapshot>,
        authorized: Arc<dyn Fn() -> bool + Send + Sync>) -> Result<Self> {
        ensure!(authorized(), "dynamic tunnel authorization revoked");
        let listener = tokio::net::TcpListener::bind(snapshot.bind_address()?).await
            .map_err(|_| anyhow!("saved dynamic tunnel port unavailable; existing owner unchanged"))?;
        let address = listener.local_addr()?;
        let scope = Arc::new(DrainingTransportScope::new());
        let worker_scope = scope.clone();
        let (stop, mut stopped) = watch::channel(false);
        let (sender, ready) = oneshot::channel();
        let (activation, activated) = oneshot::channel();
        let (completion, finished) = watch::channel(false);
        let task = tokio::spawn(async move {
            let _completion = RuntimeCompletion(completion);
            let revoked = async {
                loop {
                    if !authorized() { break; }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            };
            let route = tokio::select! {
                biased;
                _ = async { if !*stopped.borrow() { let _ = stopped.changed().await; } } => {
                    return Err(anyhow!("dynamic startup cancelled"));
                }
                _ = revoked => return Err(anyhow!("dynamic authorization revoked")),
                connected = tokio::time::timeout(std::time::Duration::from_secs(60), snapshot.connect_managed(&worker_scope)) => {
                    connected.map_err(|_| anyhow!("dynamic startup timed out"))?
                        .map_err(|_| anyhow!("saved dynamic route authentication failed"))?
                }
            };
            // This is prepared, not externally ready. Only the coordinator
            // may grant activation after its locked generation revalidation.
            ensure!(authorized() && !*stopped.borrow(), "dynamic startup authority changed");
            sender.send(address).map_err(|_| anyhow!("dynamic readiness receiver closed"))?;
            tokio::select! {
                biased;
                _ = async { if !*stopped.borrow() { let _ = stopped.changed().await; } } => {
                    return Err(anyhow!("dynamic activation cancelled"));
                }
                activated = activated => activated.map_err(|_| anyhow!("dynamic activation abandoned"))?,
            }
            ensure!(authorized() && !*stopped.borrow(), "dynamic activation revoked");
            crate::tunnel::run_dynamic_managed(Arc::new(route), listener, stopped, authorized).await
                .map_err(|_| anyhow!("dynamic route stopped"))
        });
        Ok(Self { stop, scope, task: Some(task), ready: Some(ready), task_unconfirmed: false, activation: Some(activation), finished, #[cfg(test)] bound: address })
    }

    #[cfg(test)] pub fn bound_address(&self) -> SocketAddr { self.bound }
    pub fn finished(&self) -> watch::Receiver<bool> { self.finished.clone() }

    pub fn stop_handle(&self) -> watch::Sender<bool> { self.stop.clone() }

    pub fn activate(&mut self) -> Result<()> {
        self.activation.take().ok_or_else(|| anyhow!("activation already consumed"))?
            .send(()).map_err(|_| anyhow!("dynamic activation unavailable"))
    }

    pub async fn ready(&mut self) -> Result<SocketAddr> {
        self.ready.take().ok_or_else(|| anyhow!("readiness already consumed"))?
            .await.map_err(|_| anyhow!("dynamic startup failed or was cancelled"))
    }

    /// Call while the registry retains stopping/bind ownership. Timeout never
    /// acknowledges success or frees that slot. Retry awaits the same owned
    /// worker and managed session actors; it does not restart the listener.
    pub async fn drain_until(&mut self, deadline: tokio::time::Instant) -> Result<()> {
        self.stop.send_replace(true);
        self.scope.cancel();
        if let Some(task) = self.task.as_mut() {
            let result = tokio::time::timeout_at(deadline, task).await
                .map_err(|_| anyhow!("cleanup_unconfirmed: dynamic worker deadline expired"))?;
            self.task_unconfirmed |= result.is_err();
            self.task.take();
        }
        self.scope.drain_until(deadline).await?;
        ensure!(!self.task_unconfirmed, "cleanup_unconfirmed: dynamic worker failed to join");
        Ok(())
    }
}
impl Drop for ManagedForward {
    fn drop(&mut self) {
        self.stop.send_replace(true);
        self.scope.cancel();
        if let Some(task) = &self.task { task.abort(); }
        // Last-resort cancellation only. Drop cannot issue a successful ACK;
        // production integration must retain this resource until drain settles.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs::File;
    use std::sync::{Mutex, atomic::{AtomicUsize, Ordering}};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use russh::{server, Channel};
    use russh::keys::{PrivateKey, PublicKey, ssh_key::private::Ed25519Keypair};

    struct Node {
        target: SocketAddr,
        echo: bool,
        authenticated: Arc<AtomicUsize>,
        relays: Arc<Mutex<Vec<JoinHandle<()>>>>,
        pause: Option<Arc<tokio::sync::Notify>>,
        entered: Arc<tokio::sync::Notify>,
    }
    impl server::Handler for Node {
        type Error = russh::Error;
        async fn auth_password(&mut self, user: &str, password: &str) -> std::result::Result<server::Auth, Self::Error> {
            if user != "fixture" || password != "fixture-password" { return Ok(server::Auth::reject()); }
            self.authenticated.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            if let Some(pause) = &self.pause { pause.notified().await; }
            Ok(server::Auth::Accept)
        }
        async fn channel_open_direct_tcpip(&mut self, channel: Channel<server::Msg>, host: &str,
            port: u32, _origin: &str, _origin_port: u32, _session: &mut server::Session)
            -> std::result::Result<bool, Self::Error> {
            let permitted = if self.echo { host == "echo.invalid" && port == 23 }
                else { host == "127.0.0.1" && port == self.target.port() as u32 };
            if !permitted { return Ok(false); }
            let target = self.target;
            self.relays.lock().unwrap().push(tokio::spawn(async move {
                let mut tcp = tokio::net::TcpStream::connect(target).await.unwrap();
                let mut channel = channel.into_stream();
                let _ = tokio::io::copy_bidirectional(&mut channel, &mut tcp).await;
            }));
            Ok(true)
        }
    }
    struct Fixture {
        _directory: tempfile::TempDir,
        snapshot: Arc<ResolvedRouteSnapshot>,
        servers: Vec<JoinHandle<std::result::Result<(), russh::Error>>>,
        server_handles: Arc<Mutex<Vec<Option<server::Handle>>>>,
        relays: Arc<Mutex<Vec<JoinHandle<()>>>>,
        echo: Option<JoinHandle<()>>,
        auth_counts: Vec<Arc<AtomicUsize>>,
        final_entered: Arc<tokio::sync::Notify>,
        final_pause: Option<Arc<tokio::sync::Notify>>,
    }
    impl Fixture {
        async fn new(paused: bool) -> Self { Self::with_bad_last_key(paused, false).await }
        async fn with_bad_last_key(paused: bool, wrong_key: bool) -> Self {
            let directory = tempfile::tempdir().unwrap();
            let echo_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let echo_addr = echo_listener.local_addr().unwrap();
            let echo = tokio::spawn(async move {
                let (mut socket, _) = echo_listener.accept().await.unwrap();
                let mut bytes = [0; 256];
                loop { let count = socket.read(&mut bytes).await.unwrap(); if count == 0 { break; }
                    socket.write_all(&bytes[..count]).await.unwrap(); }
            });
            let mut listeners = Vec::new();
            for _ in 0..3 { listeners.push(tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap()); }
            let addresses: Vec<_> = listeners.iter().map(|listener| listener.local_addr().unwrap()).collect();
            let relays = Arc::new(Mutex::new(Vec::new()));
            let final_entered = Arc::new(tokio::sync::Notify::new());
            let final_pause = paused.then(|| Arc::new(tokio::sync::Notify::new()));
            let server_handles = Arc::new(Mutex::new(vec![None; 3]));
            let mut servers = Vec::new(); let mut sessions = Vec::new(); let mut auth_counts = Vec::new();
            let mut trust = String::new();
            for (index, listener) in listeners.into_iter().enumerate() {
                let key = PrivateKey::new(Ed25519Keypair::from_seed(&[40 + index as u8; 32]).into(), "route fixture").unwrap();
                let expected = if wrong_key && index == 2 {
                    PrivateKey::new(Ed25519Keypair::from_seed(&[99; 32]).into(), "wrong fixture key").unwrap()
                } else { key.clone() };
                let wire_key = PublicKey::new(expected.public_key().key_data().clone(), "");
                trust.push_str(&format!("127.0.0.1:{} {}\n", addresses[index].port(), wire_key.to_openssh().unwrap()));
                let config = Arc::new(server::Config { keys: vec![key], ..Default::default() });
                let count = Arc::new(AtomicUsize::new(0)); auth_counts.push(count.clone());
                let node = Node { target: addresses.get(index + 1).copied().unwrap_or(echo_addr), echo: index == 2,
                    authenticated: count, relays: relays.clone(), pause: if index == 2 { final_pause.clone() } else { None },
                    entered: if index == 2 { final_entered.clone() } else { Arc::new(tokio::sync::Notify::new()) } };
                let handles = server_handles.clone();
                servers.push(tokio::spawn(async move {
                    let (stream, _) = listener.accept().await?;
                    let running = server::run_stream(config, stream, node).await?;
                    handles.lock().unwrap()[index] = Some(running.handle());
                    running.await
                }));
                let mut session = crate::config::Session::new_empty(); session.host = "127.0.0.1".into();
                session.port = addresses[index].port(); session.user = "fixture".into();
                session.auth = crate::config::AuthMethod::Password;
                session.password = crate::config::Secret::new("fixture-password"); sessions.push(session);
            }
            let mut target = sessions.pop().unwrap();
            target.forwards.push(crate::config::PortForward { kind: "dynamic".into(), bind_port: 0, ..Default::default() });
            sessions.reverse(); // immediate-first saved jump chain
            let trust_path = directory.path().join("known_hosts"); std::fs::write(&trust_path, trust).unwrap();
            let mut options = std::fs::OpenOptions::new(); options.read(true);
            #[cfg(windows)] { use std::os::windows::fs::OpenOptionsExt; options.custom_flags(0x02000000); }
            let profile = options.open(directory.path()).unwrap();
            let snapshot = ResolvedRouteSnapshot::from_resolved_inputs(profile, target, sessions, 0, HashMap::new(),
                File::open(trust_path).unwrap(), 3, crate::ssh::proxy::ResolvedProxy::ExplicitDirect).unwrap();
            Self { _directory: directory, snapshot: Arc::new(snapshot), servers, server_handles, relays,
                echo: Some(echo), auth_counts, final_entered, final_pause }
        }
        async fn finish(&mut self, used_echo: bool) {
            if let Some(pause) = &self.final_pause { pause.notify_one(); }
            for server in &mut self.servers {
                let result = tokio::time::timeout(std::time::Duration::from_secs(3), server).await.unwrap().unwrap();
                assert!(!matches!(result, Err(russh::Error::Join(_))));
            }
            self.servers.clear();
            let relays = std::mem::take(&mut *self.relays.lock().unwrap());
            for relay in relays { tokio::time::timeout(std::time::Duration::from_secs(3), relay).await.unwrap().unwrap(); }
            if let Some(echo) = self.echo.take() {
                if !used_echo { echo.abort(); }
                let result = tokio::time::timeout(std::time::Duration::from_secs(3), echo).await.unwrap();
                if used_echo { result.unwrap(); } else { assert!(result.unwrap_err().is_cancelled()); }
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            for task in &self.servers { task.abort(); }
            for task in self.relays.lock().unwrap().iter() { task.abort(); }
            if let Some(task) = &self.echo { task.abort(); }
        }
    }

    #[tokio::test]
    async fn managed_three_node_socks_relay_drains_clients_listener_and_all_hops() {
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            let mut fixture = Fixture::new(false).await;
            let mut owner = ManagedForward::start(fixture.snapshot.clone(), Arc::new(|| true)).await.unwrap();
            let address = owner.ready().await.unwrap();
            let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
            client.write_all(&[5, 1, 0]).await.unwrap(); let mut greeting = [0; 2];
            client.read_exact(&mut greeting).await.unwrap(); assert_eq!(greeting, [5, 0]);
            let mut request = vec![5, 1, 0, 3, 12]; request.extend_from_slice(b"echo.invalid"); request.extend_from_slice(&23u16.to_be_bytes());
            client.write_all(&request).await.unwrap(); let mut reply = [0; 10];
            client.read_exact(&mut reply).await.unwrap(); assert_eq!(reply[1], 0);
            client.write_all(b"three-hop-payload").await.unwrap(); let mut echoed = [0; 17];
            client.read_exact(&mut echoed).await.unwrap(); assert_eq!(&echoed, b"three-hop-payload");
            owner.drain_until(tokio::time::Instant::now() + std::time::Duration::from_secs(4)).await.unwrap();
            let mut byte = [0]; assert_eq!(client.read(&mut byte).await.unwrap(), 0);
            assert!(tokio::net::TcpStream::connect(address).await.is_err());
            fixture.finish(true).await;
            for count in &fixture.auth_counts { assert_eq!(count.load(Ordering::SeqCst), 1); }
        }).await.expect("three-node managed relay exceeded test deadline");
    }

    #[tokio::test]
    async fn managed_startup_cancel_during_last_hop_auth_drains_preceding_hops() {
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            let mut fixture = Fixture::new(true).await;
            let mut owner = ManagedForward::start(fixture.snapshot.clone(), Arc::new(|| true)).await.unwrap();
            fixture.final_entered.notified().await;
            owner.drain_until(tokio::time::Instant::now() + std::time::Duration::from_secs(4)).await.unwrap();
            assert!(owner.ready().await.is_err());
            // This ACK covers local owned tasks and closed transports. The
            // synthetic remote auth callback remains paused until finish;
            // remote handler execution is not claimed as a drain guarantee.
            fixture.finish(false).await;
        }).await.expect("cancelled managed startup exceeded test deadline");
    }
    async fn coordinator_echo(address: std::net::SocketAddr) -> tokio::net::TcpStream {
        let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
        client.write_all(&[5, 1, 0]).await.unwrap(); let mut greeting = [0; 2];
        client.read_exact(&mut greeting).await.unwrap(); assert_eq!(greeting, [5, 0]);
        let mut request = vec![5, 1, 0, 3, 12]; request.extend_from_slice(b"echo.invalid"); request.extend_from_slice(&23u16.to_be_bytes());
        client.write_all(&request).await.unwrap(); let mut reply = [0; 10];
        client.read_exact(&mut reply).await.unwrap(); assert_eq!(reply[1], 0);
        client.write_all(b"coordinator").await.unwrap(); let mut echoed = [0; 11];
        client.read_exact(&mut echoed).await.unwrap(); assert_eq!(&echoed, b"coordinator");
        client
    }

    #[tokio::test]
    async fn coordinator_revocation_between_auth_and_activation_never_accepts() {
        use super::super::coordinator::{ActivationBarrier, Coordinator};
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            let mut fixture = Fixture::new(false).await;
            let barrier = Arc::new(ActivationBarrier::default());
            let mut coordinator = Coordinator::new(); coordinator.before_activation = Some(barrier.clone());
            coordinator.authorize_saved([1; 32], 3).unwrap();
            let connection = coordinator.connect_saved([1; 32], 3).unwrap();
            let epoch = coordinator.epoch();
            let reservation = coordinator.reserve(connection, &epoch, fixture.snapshot.clone(), true).unwrap();
            barrier.entered.notified().await;
            let address = barrier.prepared.lock().unwrap().unwrap();
            let mut queued = tokio::net::TcpStream::connect(address).await.unwrap();
            queued.write_all(&[5, 1, 0]).await.unwrap(); let mut byte = [0];
            assert!(tokio::time::timeout(std::time::Duration::from_millis(50), queued.read(&mut byte)).await.is_err());
            let mut revoke = Box::pin(coordinator.revoke([1; 32], 3,
                tokio::time::Instant::now() + std::time::Duration::from_secs(4)));
            assert!(futures::poll!(&mut revoke).is_pending());
            assert!(coordinator.reserve(connection, &epoch, fixture.snapshot.clone(), true).is_err());
            barrier.release.notify_one(); revoke.await.unwrap();
            assert!(reservation.ready().await.is_err());
            assert!(matches!(queued.read(&mut byte).await, Ok(0) | Err(_)));
            assert!(tokio::net::TcpStream::connect(address).await.is_err());
            fixture.finish(false).await;
            for count in &fixture.auth_counts { assert_eq!(count.load(Ordering::SeqCst), 1); }
        }).await.expect("pre-activation revocation fixture exceeded deadline");
    }

    #[tokio::test]
    async fn coordinator_concurrent_ensure_shares_one_route_and_independent_leases() {
        use super::super::coordinator::Coordinator;
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            let mut fixture = Fixture::new(false).await;
            let coordinator = Arc::new(Coordinator::new());
            coordinator.authorize_saved([1; 32], 3).unwrap();
            let first = coordinator.connect_saved([1; 32], 3).unwrap();
            let second = coordinator.connect_saved([1; 32], 3).unwrap();
            let barrier = Arc::new(tokio::sync::Barrier::new(3));
            let mut tasks = Vec::new();
            for connection in [first, second] {
                let coordinator = coordinator.clone(); let snapshot = fixture.snapshot.clone(); let barrier = barrier.clone();
                tasks.push(tokio::spawn(async move {
                    barrier.wait().await;
                    coordinator.reserve(connection, &coordinator.epoch(), snapshot, true).unwrap()
                }));
            }
            barrier.wait().await;
            let a = tasks.remove(0).await.unwrap(); let b = tasks.remove(0).await.unwrap();
            assert_ne!(a.acquisition.start, b.acquisition.start);
            assert_eq!(a.acquisition.listener, b.acquisition.listener);
            assert_ne!(a.acquisition.lease, b.acquisition.lease);
            let address = a.ready().await.unwrap(); assert_eq!(b.ready().await.unwrap(), address);
            let mut client = coordinator_echo(address).await;
            coordinator.release(first, &coordinator.epoch(), &a.acquisition.lease,
                tokio::time::Instant::now() + std::time::Duration::from_secs(3)).await.unwrap();
            assert_eq!(b.ready().await.unwrap(), address);
            client.write_all(b"still-live").await.unwrap(); let mut bytes = [0; 10];
            client.read_exact(&mut bytes).await.unwrap(); assert_eq!(&bytes, b"still-live");
            coordinator.release(second, &coordinator.epoch(), &b.acquisition.lease,
                tokio::time::Instant::now() + std::time::Duration::from_secs(4)).await.unwrap();
            let mut byte = [0]; assert_eq!(client.read(&mut byte).await.unwrap(), 0);
            assert!(tokio::net::TcpStream::connect(address).await.is_err());
            fixture.finish(true).await;
            for count in &fixture.auth_counts { assert_eq!(count.load(Ordering::SeqCst), 1); }
        }).await.expect("concurrent coordinator fixture exceeded deadline");
    }

    #[tokio::test]
    async fn coordinator_last_release_timeout_keeps_bind_reserved_until_confirmed_drain() {
        use super::super::coordinator::{ActivationBarrier, Coordinator};
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            let mut fixture = Fixture::new(false).await;
            let barrier = Arc::new(ActivationBarrier::default());
            let mut coordinator = Coordinator::new(); coordinator.before_drain = Some(barrier.clone());
            coordinator.authorize_saved([1; 32], 3).unwrap();
            let connection = coordinator.connect_saved([1; 32], 3).unwrap();
            let epoch = coordinator.epoch();
            let reservation = coordinator.reserve(connection, &epoch, fixture.snapshot.clone(), true).unwrap();
            let address = reservation.ready().await.unwrap();
            let mut client = coordinator_echo(address).await;
            let mut release = Box::pin(coordinator.release(connection, &epoch, &reservation.acquisition.lease,
                tokio::time::Instant::now() + std::time::Duration::from_millis(100)));
            assert!(futures::poll!(&mut release).is_pending());
            barrier.entered.notified().await;
            assert!(coordinator.reserve(connection, &epoch, fixture.snapshot.clone(), true).is_err());
            assert!(release.await.unwrap_err().to_string().contains("cleanup_unconfirmed"));
            assert!(coordinator.reserve(connection, &epoch, fixture.snapshot.clone(), true).is_err());
            barrier.release.notify_one();
            coordinator.drain(&reservation.acquisition.listener,
                tokio::time::Instant::now() + std::time::Duration::from_secs(4)).await.unwrap();
            let mut byte = [0]; assert_eq!(client.read(&mut byte).await.unwrap(), 0);
            assert!(tokio::net::TcpStream::connect(address).await.is_err());
            fixture.finish(true).await;
            // Reservation can be made again only after confirmed cleanup. The
            // fixture servers intentionally accept one transport each; cancel
            // this second startup rather than claiming another successful SSH.
            let next = coordinator.reserve(connection, &epoch, fixture.snapshot.clone(), true).unwrap();
            assert!(next.acquisition.start);
            assert_ne!(next.acquisition.listener, reservation.acquisition.listener);
            barrier.release.notify_one();
            coordinator.release(connection, &epoch, &next.acquisition.lease,
                tokio::time::Instant::now() + std::time::Duration::from_secs(4)).await.unwrap();
        }).await.expect("stopping reservation fixture exceeded deadline");
    }

    #[tokio::test]
    async fn coordinator_repeated_disconnect_waits_for_original_pending_drains() {
        use super::super::coordinator::{ActivationBarrier, Coordinator};
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            let mut fixture = Fixture::new(false).await;
            let barrier = Arc::new(ActivationBarrier::default());
            let mut coordinator = Coordinator::new(); coordinator.before_drain = Some(barrier.clone());
            coordinator.authorize_saved([1; 32], 3).unwrap();
            let connection = coordinator.connect_saved([1; 32], 3).unwrap();
            let observer = coordinator.connect_saved([1; 32], 3).unwrap();
            let epoch = coordinator.epoch();
            let reservation = coordinator.reserve(connection, &epoch, fixture.snapshot.clone(), true).unwrap();
            let address = reservation.ready().await.unwrap();
            let mut client = coordinator_echo(address).await;
            let mut first = Box::pin(coordinator.disconnect(connection,
                tokio::time::Instant::now() + std::time::Duration::from_millis(100)));
            assert!(futures::poll!(&mut first).is_pending());
            barrier.entered.notified().await;
            assert!(first.await.unwrap_err().to_string().contains("cleanup_unconfirmed"));
            // The connection and leases have already left Registry. Its
            // retained pending-drain record must still prevent an early ACK.
            assert!(coordinator.disconnect(connection, tokio::time::Instant::now()
                + std::time::Duration::from_millis(50)).await.unwrap_err().to_string().contains("cleanup_unconfirmed"));
            let error = coordinator.reserve(observer, &epoch, fixture.snapshot.clone(), true).err().unwrap();
            assert!(error.to_string().contains("Stopping"));
            barrier.release.notify_one();
            coordinator.disconnect(connection, tokio::time::Instant::now()
                + std::time::Duration::from_secs(4)).await.unwrap();
            coordinator.disconnect(connection, tokio::time::Instant::now()
                + std::time::Duration::from_secs(1)).await.unwrap();
            assert!(coordinator.disconnect(u64::MAX, tokio::time::Instant::now()
                + std::time::Duration::from_secs(1)).await.is_err());
            let mut byte = [0]; assert_eq!(client.read(&mut byte).await.unwrap(), 0);
            assert!(tokio::net::TcpStream::connect(address).await.is_err());
            fixture.finish(true).await;
        }).await.expect("repeated disconnect fixture exceeded deadline");
    }

    #[tokio::test]
    async fn coordinator_physical_drain_does_not_publish_before_lifecycle_join() {
        use super::super::coordinator::{ActivationBarrier, Coordinator};
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            let mut fixture = Fixture::new(false).await;
            let barrier = Arc::new(ActivationBarrier::default());
            let mut coordinator = Coordinator::new(); coordinator.after_runtime_drain = Some(barrier.clone());
            coordinator.authorize_saved([1; 32], 3).unwrap();
            let connection = coordinator.connect_saved([1; 32], 3).unwrap();
            let epoch = coordinator.epoch();
            let reservation = coordinator.reserve(connection, &epoch, fixture.snapshot.clone(), true).unwrap();
            let address = reservation.ready().await.unwrap();
            let mut client = coordinator_echo(address).await;
            let mut release = Box::pin(coordinator.release(connection, &epoch, &reservation.acquisition.lease,
                tokio::time::Instant::now() + std::time::Duration::from_millis(100)));
            assert!(futures::poll!(&mut release).is_pending());
            barrier.entered.notified().await;
            // All network resources are already closed, but our lifecycle
            // worker is deliberately still running and has not been joined.
            let mut byte = [0]; assert_eq!(client.read(&mut byte).await.unwrap(), 0);
            assert!(tokio::net::TcpStream::connect(address).await.is_err());
            assert!(release.await.unwrap_err().to_string().contains("cleanup_unconfirmed"));
            assert!(coordinator.drain(&reservation.acquisition.listener, tokio::time::Instant::now()
                + std::time::Duration::from_millis(50)).await.unwrap_err().to_string().contains("cleanup_unconfirmed"));
            assert!(coordinator.reserve(connection, &epoch, fixture.snapshot.clone(), true).err().unwrap()
                .to_string().contains("Stopping"));
            barrier.release.notify_one();
            coordinator.drain(&reservation.acquisition.listener, tokio::time::Instant::now()
                + std::time::Duration::from_secs(4)).await.unwrap();
            reservation.observe_completion().await.unwrap();
            fixture.finish(true).await;
        }).await.expect("lifecycle join barrier fixture exceeded deadline");
    }

    #[tokio::test]
    async fn coordinator_last_hop_hostkey_failure_automatically_drains_without_release() {
        use super::super::coordinator::Coordinator;
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            let mut fixture = Fixture::with_bad_last_key(false, true).await;
            let coordinator = Coordinator::new(); coordinator.authorize_saved([1; 32], 3).unwrap();
            let connection = coordinator.connect_saved([1; 32], 3).unwrap();
            let reservation = coordinator.reserve(connection, &coordinator.epoch(), fixture.snapshot.clone(), true).unwrap();
            assert!(reservation.ready().await.is_err());
            // Observer only awaits the owned lifecycle join; it does not call
            // stop/release/drain and cannot make this failure clean itself up.
            reservation.observe_completion().await.unwrap();
            let address = reservation.bound_address().unwrap();
            assert!(tokio::net::TcpStream::connect(address).await.is_err());
            fixture.finish(false).await;
            assert_eq!(fixture.auth_counts[0].load(Ordering::SeqCst), 1);
            assert_eq!(fixture.auth_counts[1].load(Ordering::SeqCst), 1);
            assert_eq!(fixture.auth_counts[2].load(Ordering::SeqCst), 0);
        }).await.expect("automatic failed-startup cleanup exceeded deadline");
    }

    #[tokio::test]
    async fn coordinator_server_disconnect_automatically_drains_without_release() {
        use super::super::coordinator::Coordinator;
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            let mut fixture = Fixture::new(false).await;
            let coordinator = Coordinator::new(); coordinator.authorize_saved([1; 32], 3).unwrap();
            let connection = coordinator.connect_saved([1; 32], 3).unwrap();
            let reservation = coordinator.reserve(connection, &coordinator.epoch(), fixture.snapshot.clone(), true).unwrap();
            let address = reservation.ready().await.unwrap();
            let mut client = coordinator_echo(address).await;
            let server = fixture.server_handles.lock().unwrap()[2].clone().unwrap();
            server.disconnect(russh::Disconnect::ByApplication, "synthetic server shutdown".into(), "".into()).await.unwrap();
            // The real server sends SSH disconnect. No wrapper task is aborted
            // and no coordinator stop/release/drain is invoked by this test.
            reservation.observe_completion().await.unwrap();
            let mut byte = [0]; assert_eq!(client.read(&mut byte).await.unwrap(), 0);
            assert!(tokio::net::TcpStream::connect(address).await.is_err());
            fixture.finish(true).await;
        }).await.expect("automatic disconnected-route cleanup exceeded deadline");
    }

}
