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
}

impl ManagedForward {
    pub async fn start(snapshot: Arc<ResolvedRouteSnapshot>,
        authorized: Arc<dyn Fn() -> bool + Send + Sync>) -> Result<Self> {
        ensure!(authorized(), "dynamic tunnel authorization revoked");
        let listener = tokio::net::TcpListener::bind(snapshot.bind_address()?).await
            .map_err(|_| anyhow!("saved dynamic tunnel port unavailable; existing owner unchanged"))?;
        let address = listener.local_addr()?;
        let scope = Arc::new(DrainingTransportScope::new());
        let worker_scope = scope.clone();
        let (stop, mut stopped) = watch::channel(false);
        let (sender, ready) = oneshot::channel();
        let task = tokio::spawn(async move {
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
            // The eventual registry still linearizes generation revalidation
            // with ready publication; this callback alone is not that lock.
            ensure!(authorized() && !*stopped.borrow(), "dynamic startup authority changed");
            sender.send(address).map_err(|_| anyhow!("dynamic readiness receiver closed"))?;
            crate::tunnel::run_dynamic_managed(Arc::new(route), listener, stopped, authorized).await
                .map_err(|_| anyhow!("dynamic route stopped"))
        });
        Ok(Self { stop, scope, task: Some(task), ready: Some(ready), task_unconfirmed: false })
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
        relays: Arc<Mutex<Vec<JoinHandle<()>>>>,
        echo: Option<JoinHandle<()>>,
        auth_counts: Vec<Arc<AtomicUsize>>,
        final_entered: Arc<tokio::sync::Notify>,
        final_pause: Option<Arc<tokio::sync::Notify>>,
    }
    impl Fixture {
        async fn new(paused: bool) -> Self {
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
            let mut servers = Vec::new(); let mut sessions = Vec::new(); let mut auth_counts = Vec::new();
            let mut trust = String::new();
            for (index, listener) in listeners.into_iter().enumerate() {
                let key = PrivateKey::new(Ed25519Keypair::from_seed(&[40 + index as u8; 32]).into(), "route fixture").unwrap();
                let wire_key = PublicKey::new(key.public_key().key_data().clone(), "");
                trust.push_str(&format!("127.0.0.1:{} {}\n", addresses[index].port(), wire_key.to_openssh().unwrap()));
                let config = Arc::new(server::Config { keys: vec![key], ..Default::default() });
                let count = Arc::new(AtomicUsize::new(0)); auth_counts.push(count.clone());
                let node = Node { target: addresses.get(index + 1).copied().unwrap_or(echo_addr), echo: index == 2,
                    authenticated: count, relays: relays.clone(), pause: if index == 2 { final_pause.clone() } else { None },
                    entered: if index == 2 { final_entered.clone() } else { Arc::new(tokio::sync::Notify::new()) } };
                servers.push(tokio::spawn(async move {
                    let (stream, _) = listener.accept().await?;
                    let running = server::run_stream(config, stream, node).await?;
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
            Self { _directory: directory, snapshot: Arc::new(snapshot), servers, relays,
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
}
