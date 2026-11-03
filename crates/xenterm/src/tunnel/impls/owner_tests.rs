//! Real SSH and SOCKS fixtures, run in a child test process with an isolated
//! profile. No process-global env mutation in the parent test harness, no GUI,
//! custom accept-all host-key handler, installed service or user credentials.
use super::*;
use russh::keys::{ssh_key::private::Ed25519Keypair, PrivateKey, PublicKey};
use russh::{server, Channel};
use std::sync::atomic::AtomicUsize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;

struct Node {
    target: SocketAddr,
    echo: bool,
    auth: Arc<AtomicUsize>,
    sessions: Arc<AtomicUsize>,
    relays: Arc<Mutex<Vec<JoinHandle<()>>>>,
}
impl server::Handler for Node {
    type Error = russh::Error;
    async fn auth_password(
        &mut self,
        user: &str,
        password: &str,
    ) -> std::result::Result<server::Auth, Self::Error> {
        if user != "fixture" || password != "fixture-password" {
            return Ok(server::Auth::reject());
        }
        self.auth.fetch_add(1, Ordering::SeqCst);
        Ok(server::Auth::Accept)
    }
    async fn channel_open_session(
        &mut self,
        _channel: Channel<server::Msg>,
        _session: &mut server::Session,
    ) -> std::result::Result<bool, Self::Error> {
        self.sessions.fetch_add(1, Ordering::SeqCst);
        Ok(false) // A session/PTY/shell would make this fixture fail.
    }
    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<server::Msg>,
        host: &str,
        port: u32,
        _origin: &str,
        _origin_port: u32,
        _session: &mut server::Session,
    ) -> std::result::Result<bool, Self::Error> {
        let permitted = if self.echo {
            host == "echo.invalid" && port == 23
        } else {
            host == "127.0.0.1" && port == self.target.port() as u32
        };
        if !permitted {
            return Ok(false);
        }
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
    target: Session,
    forbidden: TcpListener,
    server_handles: Arc<Mutex<Vec<Option<server::Handle>>>>,
    jumps: Vec<Session>,
    servers: Vec<JoinHandle<std::result::Result<(), russh::Error>>>,
    relays: Arc<Mutex<Vec<JoinHandle<()>>>>,
    echo: Option<JoinHandle<()>>,
    auth: Vec<Arc<AtomicUsize>>,
    sessions: Arc<AtomicUsize>,
}
impl Fixture {
    async fn new(bind: u16, wrong_key: bool) -> Self {
        let forbidden = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let forbidden_port = forbidden.local_addr().unwrap().port();
        let server_handles = Arc::new(Mutex::new(vec![None; 3]));
        let echo_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let echo_addr = echo_listener.local_addr().unwrap();
        let echo = tokio::spawn(async move {
            let (mut socket, _) = echo_listener.accept().await.unwrap();
            let mut bytes = [0; 256];
            loop {
                let count = socket.read(&mut bytes).await.unwrap();
                if count == 0 {
                    break;
                }
                socket.write_all(&bytes[..count]).await.unwrap();
            }
        });
        let mut listeners = Vec::new();
        for _ in 0..3 {
            listeners.push(TcpListener::bind("127.0.0.1:0").await.unwrap());
        }
        let addresses: Vec<_> = listeners
            .iter()
            .map(|listener| listener.local_addr().unwrap())
            .collect();
        let relays = Arc::new(Mutex::new(Vec::new()));
        let sessions = Arc::new(AtomicUsize::new(0));
        let mut servers = Vec::new();
        let mut saved = Vec::new();
        let mut auth = Vec::new();
        let mut trust = String::new();
        for (index, listener) in listeners.into_iter().enumerate() {
            let key = PrivateKey::new(
                Ed25519Keypair::from_seed(&[40 + index as u8; 32]).into(),
                "owner fixture",
            )
            .unwrap();
            let expected = if wrong_key && index == 2 {
                PrivateKey::new(Ed25519Keypair::from_seed(&[99; 32]).into(), "wrong fixture")
                    .unwrap()
            } else {
                key.clone()
            };
            let wire_key = PublicKey::new(expected.public_key().key_data().clone(), "");
            trust.push_str(&format!(
                "127.0.0.1:{} {}\n",
                addresses[index].port(),
                wire_key.to_openssh().unwrap()
            ));
            let config = Arc::new(server::Config {
                keys: vec![key],
                ..Default::default()
            });
            let count = Arc::new(AtomicUsize::new(0));
            auth.push(count.clone());
            let node = Node {
                target: addresses.get(index + 1).copied().unwrap_or(echo_addr),
                echo: index == 2,
                auth: count,
                relays: relays.clone(),
                sessions: sessions.clone(),
            };
            let handles = server_handles.clone();
            servers.push(tokio::spawn(async move {
                let (stream, _) = listener.accept().await?;
                let running = server::run_stream(config, stream, node).await?;
                handles.lock().unwrap()[index] = Some(running.handle());
                running.await
            }));
            let mut session = Session::new_empty();
            session.host = "127.0.0.1".into();
            session.port = addresses[index].port();
            session.user = "fixture".into();
            session.auth = crate::config::AuthMethod::Password;
            session.password = crate::config::Secret::new("fixture-password");
            // These saved unrelated rules must not be activated by headless D.
            session.forwards.push(PortForward {
                kind: "remote".into(),
                bind_port: 42000 + index as u16,
                host: "127.0.0.1".into(),
                host_port: forbidden_port,
                ..Default::default()
            });
            saved.push(session);
        }
        std::fs::write(crate::config::data_dir().join("known_hosts"), trust).unwrap();
        let mut target = saved.pop().unwrap();
        target.id = "fixture-saved-target".into();
        target.forwards.push(PortForward {
            kind: "dynamic".into(),
            bind_port: bind,
            ..Default::default()
        });
        saved.reverse();
        Self {
            target,
            jumps: saved,
            servers,
            relays,
            echo: Some(echo),
            auth,
            sessions,
            forbidden,
            server_handles,
        }
    }
    async fn reject_unrequested_remote_maps(&self) {
        let handles: Vec<_> = self
            .server_handles
            .lock()
            .unwrap()
            .iter()
            .map(|handle| handle.clone().unwrap())
            .collect();
        for (index, handle) in handles.into_iter().enumerate() {
            let result = tokio::time::timeout(
                Duration::from_secs(2),
                handle.channel_open_forwarded_tcpip(
                    "127.0.0.1",
                    42000 + index as u32,
                    "127.0.0.1",
                    40001,
                ),
            )
            .await
            .expect("unsolicited channel confirmation stalled");
            if let Ok(channel) = result {
                // A confirmed SSH channel is not permission to open local TCP.
                // Explicitly close the test probe; do not claim auto peer EOF.
                channel.close().await.unwrap();
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(150), self.forbidden.accept())
                    .await
                    .is_err(),
                "a headless hop accepted a saved but unrequested remote mapping"
            );
        }
    }
    async fn finish(&mut self, used_echo: bool) {
        for server in &mut self.servers {
            let result = tokio::time::timeout(Duration::from_secs(3), server)
                .await
                .unwrap()
                .unwrap();
            assert!(!matches!(result, Err(russh::Error::Join(_))));
        }
        self.servers.clear();
        let relays = std::mem::take(&mut *self.relays.lock().unwrap());
        for relay in relays {
            tokio::time::timeout(Duration::from_secs(3), relay)
                .await
                .unwrap()
                .unwrap();
        }
        if let Some(echo) = self.echo.take() {
            if !used_echo {
                echo.abort();
            }
            let result = tokio::time::timeout(Duration::from_secs(3), echo)
                .await
                .unwrap();
            if used_echo {
                result.unwrap();
            } else {
                assert!(result.unwrap_err().is_cancelled());
            }
        }
        assert_eq!(self.sessions.load(Ordering::SeqCst), 0);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for task in &self.servers {
            task.abort();
        }
        for task in self.relays.lock().unwrap().iter() {
            task.abort();
        }
        if let Some(task) = &self.echo {
            task.abort();
        }
    }
}
async fn echo(address: SocketAddr) -> tokio::net::TcpStream {
    let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
    client.write_all(&[5, 1, 0]).await.unwrap();
    let mut greeting = [0; 2];
    client.read_exact(&mut greeting).await.unwrap();
    assert_eq!(greeting, [5, 0]);
    let mut request = vec![5, 1, 0, 3, 12];
    request.extend_from_slice(b"echo.invalid");
    request.extend_from_slice(&23u16.to_be_bytes());
    client.write_all(&request).await.unwrap();
    let mut reply = [0; 10];
    client.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[1], 0);
    client.write_all(b"no-terminal").await.unwrap();
    let mut echoed = [0; 11];
    client.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"no-terminal");
    client
}

#[test]
fn stalled_start_does_not_block_another_handles_release() {
    isolated(
        "tunnel::owner::lifecycle_tests::stalled_start_does_not_block_another_handles_release",
        || async {
            let mut fixture = Fixture::new(0, false).await;
            let owner = Arc::new(ForwardOwner::new(false));
            let ready = owner
                .start(
                    fixture.target.clone(),
                    fixture.jumps.clone(),
                    1,
                    1,
                    false,
                    None,
                )
                .await
                .unwrap();
            let address: SocketAddr = ready
                .proxy_url
                .strip_prefix("socks5h://")
                .unwrap()
                .parse()
                .unwrap();
            let mut client = echo(address).await;

            let silent = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let mut stalled = Session::new_empty();
            stalled.host = "pending.invalid".into();
            stalled.proxy = format!("http://{}", silent.local_addr().unwrap());
            stalled.forwards.push(PortForward {
                kind: "dynamic".into(),
                bind_port: 0,
                ..Default::default()
            });
            let worker = owner.clone();
            let start =
                tokio::spawn(async move { worker.start(stalled, vec![], 0, 1, false, None).await });
            // Accept is a barrier: B is registered and stuck in real HTTP CONNECT,
            // while A already has an authenticated three-hop route and SOCKS client.
            let (_transport, _) = tokio::time::timeout(Duration::from_secs(2), silent.accept())
                .await
                .unwrap()
                .unwrap();
            let released =
                tokio::time::timeout(Duration::from_secs(2), owner.release(&ready.handle))
                    .await
                    .expect("unrelated startup held release behind its authentication deadline")
                    .unwrap();
            assert_eq!(released.state, "released");
            assert!(
                !start.is_finished(),
                "releasing A also cancelled independent startup B"
            );
            let mut byte = [0];
            assert_eq!(client.read(&mut byte).await.unwrap(), 0);
            drop(TcpListener::bind(address).await.unwrap());
            fixture.finish(true).await;
            owner.shutdown().await.unwrap();
            assert!(start.await.unwrap().is_err());
            assert!(owner
                .entries
                .lock()
                .unwrap()
                .values()
                .all(|entry| entry.completed.load(Ordering::Acquire)));
        },
    );
}

/// A deterministic stand-in for a synchronous key KDF. Drop opens the barrier
/// even when an assertion fails, so the runtime never hangs on a test thread.
struct KeyLoadBarrier(Arc<(Mutex<bool>, std::sync::Condvar)>);
impl KeyLoadBarrier {
    fn new() -> Self {
        Self(Arc::new((Mutex::new(false), std::sync::Condvar::new())))
    }
    fn open(&self) {
        let (open, changed) = &*self.0;
        *open.lock().unwrap() = true;
        changed.notify_all();
    }
}
impl Drop for KeyLoadBarrier {
    fn drop(&mut self) {
        self.open();
    }
}

#[test]
fn cancelled_or_timed_out_key_load_is_joined_before_release_ack() {
    isolated("tunnel::owner::lifecycle_tests::cancelled_or_timed_out_key_load_is_joined_before_release_ack", || async {
        for connection_timeout in [false, true] {
            let owner = ForwardOwner::new(false);
            let session = Session::new_empty();
            let policy = crate::ssh::headless_policy::Policy::capture(&session, &[]).unwrap();
            let barrier = KeyLoadBarrier::new();
            let gate = barrier.0.clone();
            let finished = Arc::new(AtomicBool::new(false));
            let loaded = finished.clone();
            let (entered, entered_rx) = oneshot::channel();
            let (expire, expired) = oneshot::channel();
            let (stop, mut stopped) = watch::channel(false);
            let (state_tx, state) = watch::channel("starting");
            let task = tokio::spawn(owned_worker(policy, state_tx, async move {
                let loading = crate::ssh::headless_policy::load_private_key(move || {
                    let _ = entered.send(());
                    let (open, changed) = &*gate;
                    let mut open = open.lock().unwrap();
                    while !*open { open = changed.wait(open).unwrap(); }
                    loaded.store(true, Ordering::Release);
                    Err(anyhow!("synthetic private-key parse failure"))
                });
                if connection_timeout {
                    tokio::pin!(loading);
                    tokio::select! {
                        _ = expired => {},
                        _ = &mut loading => panic!("key loader escaped the closed barrier"),
                    }
                    assert!(tokio::time::timeout(Duration::from_millis(1), &mut loading).await.is_err());
                    ("failed", true)
                } else {
                    tokio::select! {
                        biased;
                        _ = stopped.changed() => ("released", true),
                        _ = loading => panic!("key loader escaped the closed barrier"),
                    }
                }
            }));
            let entry = Arc::new(Entry {
                status: ForwardStatus { handle: "blocked-key-load".into(), session_id: session.id,
                    forward_index: 0, proxy_url: "socks5h://127.0.0.1:1".into(),
                    state: "starting".into(), ownership: "synthetic-owner" },
                identity: Mutex::new(None), proxy_url: Mutex::new(String::new()),
                expires: None, permission_probe: None, state, stop,
                task: tokio::sync::Mutex::new(Some(task)),
                completed: AtomicBool::new(false), unconfirmed: AtomicBool::new(false),
                finished_at: Mutex::new(None),
            });
            owner.entries.lock().unwrap().insert(entry.status.handle.clone(), entry.clone());
            tokio::time::timeout(Duration::from_secs(2), entered_rx).await.unwrap().unwrap();
            if connection_timeout { expire.send(()).unwrap(); }

            // Even a competing release's join lock belongs to the total budget.
            let held = entry.task.lock().await;
            let deadline = Instant::now() + Duration::from_millis(25);
            let result = tokio::time::timeout(Duration::from_secs(1), entry.drain_until(deadline)).await
                .expect("cleanup deadline did not cover waiting for the join lock");
            assert!(result.err().unwrap().to_string().contains("cleanup_unconfirmed"));
            drop(held);

            // This is the real public owner release path, including its five
            // second budget. The blocked loader must prevent a successful ACK.
            let result = owner.release(&entry.status.handle).await;
            assert!(result.err().unwrap().to_string().contains("cleanup_unconfirmed"));
            assert_eq!(owner.status(&entry.status.handle).unwrap().state, "stopping");
            assert!(!finished.load(Ordering::Acquire));
            assert!(!entry.completed.load(Ordering::Acquire));
            assert!(entry.task.lock().await.as_ref().is_some_and(|task| !task.is_finished()));
            assert_eq!(owner.entries.lock().unwrap().len(), 1);

            barrier.open();
            assert_eq!(owner.release(&entry.status.handle).await.unwrap().state, "released");
            assert!(finished.load(Ordering::Acquire));
            assert!(entry.completed.load(Ordering::Acquire));
            assert!(owner.entries.lock().unwrap().values().all(|entry| entry.completed.load(Ordering::Acquire)));
        }
    });
}

/// Polling is allowed to harvest only finished workers, never wait on the
/// preparation/keyring/SSH path. All test profiles remain child-process local.
async fn wait_status(owner: &ForwardOwner, handle: &str, wanted: &str) -> ForwardStatus {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let status = owner.status(handle).unwrap();
            if status.state == wanted {
                return status;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("owner never reached {wanted}"))
}

fn saved_dynamic(port: u16) -> Session {
    let mut session = Session::new_empty();
    session.host = "synthetic.invalid".into();
    session.forwards.push(PortForward {
        kind: "dynamic".into(),
        bind_port: port,
        ..Default::default()
    });
    session
}

#[test]
fn begin_registers_before_preparation_and_cancel_joins_late_result_without_binding() {
    isolated("tunnel::owner::lifecycle_tests::begin_registers_before_preparation_and_cancel_joins_late_result_without_binding", || async {
        let owner = ForwardOwner::new(false);
        let reserve = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = reserve.local_addr().unwrap();
        drop(reserve);
        let session = saved_dynamic(address.port());
        let selector = session.id.clone();
        let barrier = KeyLoadBarrier::new();
        let gate = barrier.0.clone();
        let (entered, entered_rx) = oneshot::channel();
        let finished = Arc::new(AtomicBool::new(false));
        let prepared = finished.clone();
        let accepted = owner.begin(selector.clone(), 0, None, move || {
            let _ = entered.send(());
            let (open, changed) = &*gate;
            let mut open = open.lock().unwrap();
            while !*open { open = changed.wait(open).unwrap(); }
            let result = PreparedForward::capture(session, vec![], 0, 1);
            prepared.store(true, Ordering::Release);
            result
        }).unwrap();
        assert_eq!(accepted.state, "preparing");
        assert!(accepted.proxy_url.is_empty());
        tokio::time::timeout(Duration::from_secs(2), entered_rx).await.unwrap().unwrap();
        assert_eq!(owner.status(&accepted.handle).unwrap().state, "preparing");
        drop(TcpListener::bind(address).await.unwrap());
        assert!(owner.begin(selector.clone(), 0, None, || panic!("duplicate preparation must not run")).is_err());
        let other = ForwardOwner::new(false);
        assert!(other.status(&accepted.handle).is_err());
        assert!(other.request_release(&accepted.handle).is_err());

        assert_eq!(owner.request_release(&accepted.handle).unwrap().state, "stopping");
        let entry = owner.entry(&accepted.handle).unwrap();
        assert!(entry.drain_until(Instant::now() + Duration::from_millis(25)).await
            .unwrap_err().to_string().contains("cleanup_unconfirmed"));
        assert!(!entry.completed.load(Ordering::Acquire));
        assert!(!finished.load(Ordering::Acquire));
        assert!(owner.begin(selector, 0, None, || panic!("stopping slot must remain reserved")).is_err());
        barrier.open();
        let released = wait_status(&owner, &accepted.handle, "released").await;
        assert!(released.proxy_url.is_empty());
        assert!(finished.load(Ordering::Acquire));
        assert!(entry.completed.load(Ordering::Acquire));
        assert!(entry.task.lock().await.is_none(), "terminal status did not join its worker");
        assert!(entry.identity.lock().unwrap().is_none(), "late result reached listener setup");
        assert!(entry.proxy_url.lock().unwrap().is_empty());
        drop(TcpListener::bind(address).await.unwrap());
        assert_eq!(owner.release(&accepted.handle).await.unwrap().state, "released");
        assert_eq!(owner.request_release(&accepted.handle).unwrap().state, "released");
        owner.shutdown().await.unwrap();
    });
}

#[test]
fn blocked_preparation_does_not_block_active_owner_release() {
    isolated(
        "tunnel::owner::lifecycle_tests::blocked_preparation_does_not_block_active_owner_release",
        || async {
            let owner = ForwardOwner::new(false);
            let mut fixture = Fixture::new(0, false).await;
            let target = fixture.target.clone();
            let jumps = fixture.jumps.clone();
            let accepted = owner
                .begin(target.id.clone(), 1, None, move || {
                    PreparedForward::capture(target, jumps, 1, 1)
                })
                .unwrap();
            assert_eq!(accepted.state, "preparing");
            assert!(accepted.proxy_url.is_empty());
            let ready = wait_status(&owner, &accepted.handle, "running").await;
            let reused = owner
                .start(
                    fixture.target.clone(),
                    fixture.jumps.clone(),
                    1,
                    1,
                    true,
                    None,
                )
                .await
                .unwrap();
            assert_eq!(reused.handle, accepted.handle);
            let address = ready
                .proxy_url
                .strip_prefix("socks5h://")
                .unwrap()
                .parse()
                .unwrap();
            let mut client = echo(address).await;
            let barrier = KeyLoadBarrier::new();
            let gate = barrier.0.clone();
            let (entered, entered_rx) = oneshot::channel();
            let pending = owner
                .begin("blocked-preparation".into(), 0, None, move || {
                    let _ = entered.send(());
                    let (open, changed) = &*gate;
                    let mut open = open.lock().unwrap();
                    while !*open {
                        open = changed.wait(open).unwrap();
                    }
                    Err(anyhow!("synthetic preparation failure"))
                })
                .unwrap();
            tokio::time::timeout(Duration::from_secs(2), entered_rx)
                .await
                .unwrap()
                .unwrap();
            let released =
                tokio::time::timeout(Duration::from_secs(2), owner.release(&ready.handle))
                    .await
                    .expect("preparation blocked another handle's release")
                    .unwrap();
            assert_eq!(released.state, "released");
            assert_eq!(owner.status(&pending.handle).unwrap().state, "preparing");
            let mut byte = [0];
            assert_eq!(client.read(&mut byte).await.unwrap(), 0);
            drop(TcpListener::bind(address).await.unwrap());
            owner.request_release(&pending.handle).unwrap();
            barrier.open();
            owner.release(&pending.handle).await.unwrap();
            fixture.finish(true).await;
            owner.shutdown().await.unwrap();
        },
    );
}

#[test]
fn terminal_retention_is_bounded_without_evicting_pending_or_unconfirmed_work() {
    isolated("tunnel::owner::lifecycle_tests::terminal_retention_is_bounded_without_evicting_pending_or_unconfirmed_work", || async {
        let owner = ForwardOwner::new(false);
        let barrier = KeyLoadBarrier::new();
        let gate = barrier.0.clone();
        let (entered, entered_rx) = oneshot::channel();
        let pending = owner.begin("pending-retention".into(), 0, None, move || {
            let _ = entered.send(());
            let (open, changed) = &*gate;
            let mut open = open.lock().unwrap();
            while !*open { open = changed.wait(open).unwrap(); }
            Err(anyhow!("synthetic preparation failure"))
        }).unwrap();
        tokio::time::timeout(Duration::from_secs(2), entered_rx).await.unwrap().unwrap();
        let uncertain = owner.begin("unconfirmed-retention".into(), 0, None,
            || panic!("synthetic owned preparation panic")).unwrap();
        wait_status(&owner, &uncertain.handle, "cleanup_unconfirmed").await;
        let mut first = None;
        let mut last = None;
        for index in 0..TERMINAL_LIMIT + 2 {
            let accepted = owner.begin(format!("failed-{index}"), 0, None,
                || Err(anyhow!("synthetic secret must not enter status"))).unwrap();
            let failed = wait_status(&owner, &accepted.handle, "failed").await;
            assert!(!serde_json::to_string(&failed).unwrap().contains("synthetic secret"));
            first.get_or_insert(accepted.handle.clone());
            last = Some(accepted.handle);
        }
        assert!(owner.status(&first.unwrap()).is_err());
        assert_eq!(owner.status(&pending.handle).unwrap().state, "preparing");
        assert_eq!(owner.status(&uncertain.handle).unwrap().state, "cleanup_unconfirmed");
        assert!(owner.entries.lock().unwrap().len() <= TERMINAL_LIMIT + 2);
        assert_eq!(owner.release(&last.unwrap()).await.unwrap().state, "released");
        {
            let mut entries = owner.entries.lock().unwrap();
            ForwardOwner::prune(&mut entries, Instant::now() + TERMINAL_TTL);
            assert_eq!(entries.len(), 2);
            assert!(entries.contains_key(&pending.handle));
            assert!(entries.contains_key(&uncertain.handle));
        }
        assert!(owner.release(&uncertain.handle).await.is_err());
        owner.request_release(&pending.handle).unwrap();
        barrier.open();
        owner.release(&pending.handle).await.unwrap();
        assert!(owner.shutdown().await.is_err(), "unconfirmed cleanup must never become a successful shutdown ACK");
    });
}

#[test]
fn pending_entries_enforce_the_live_limit_and_shutdown_joins_them() {
    isolated("tunnel::owner::lifecycle_tests::pending_entries_enforce_the_live_limit_and_shutdown_joins_them", || async {
        let owner = ForwardOwner::new(false);
        let barrier = KeyLoadBarrier::new();
        for index in 0..LIVE_LIMIT {
            let gate = barrier.0.clone();
            owner.begin(format!("pending-{index}"), 0, None, move || {
                let (open, changed) = &*gate;
                let mut open = open.lock().unwrap();
                while !*open { open = changed.wait(open).unwrap(); }
                Err(anyhow!("synthetic preparation failure"))
            }).unwrap();
        }
        assert!(owner.begin("over-limit".into(), 0, None, || panic!("over-limit work started")).is_err());
        assert_eq!(owner.entries.lock().unwrap().len(), LIVE_LIMIT);
        let handles: Vec<_> = owner.entries.lock().unwrap().keys().cloned().collect();
        for handle in &handles { owner.request_release(handle).unwrap(); }
        barrier.open();
        owner.shutdown().await.unwrap();
        for handle in handles {
            assert_eq!(owner.status(&handle).unwrap().state, "released");
            assert!(owner.entry(&handle).unwrap().task.lock().await.is_none());
        }
        assert!(owner.begin("closed".into(), 0, None, || panic!("closed owner started work")).is_err());
    });
}

#[test]
fn begin_rejects_mismatched_preparation_before_binding_and_keeps_unattended_gate() {
    isolated("tunnel::owner::lifecycle_tests::begin_rejects_mismatched_preparation_before_binding_and_keeps_unattended_gate", || async {
        let owner = ForwardOwner::new(false);
        for wrong_id in [false, true] {
            let session = saved_dynamic(0);
            let selector = if wrong_id { "different-saved-id".into() } else { session.id.clone() };
            let accepted = owner.begin(selector, if wrong_id { 0 } else { 1 }, None,
                move || PreparedForward::capture(session, vec![], 0, 1)).unwrap();
            wait_status(&owner, &accepted.handle, "failed").await;
            let entry = owner.entry(&accepted.handle).unwrap();
            assert!(entry.identity.lock().unwrap().is_none());
            assert!(entry.proxy_url.lock().unwrap().is_empty());
        }
        let denied = ForwardOwner::unattended();
        let called = Arc::new(AtomicBool::new(false));
        let ran = called.clone();
        let accepted = denied.begin("denied".into(), 0, None, move || {
            ran.store(true, Ordering::Release);
            Err(anyhow!("unauthorized preparation ran"))
        }).unwrap();
        wait_status(&denied, &accepted.handle, "failed").await;
        assert!(!called.load(Ordering::Acquire));
        owner.shutdown().await.unwrap();
        denied.shutdown().await.unwrap();
    });
}

#[test]
fn panicked_operation_still_joins_its_owned_key_loader() {
    isolated(
        "tunnel::owner::lifecycle_tests::panicked_operation_still_joins_its_owned_key_loader",
        || async {
            let session = Session::new_empty();
            let policy = crate::ssh::headless_policy::Policy::capture(&session, &[]).unwrap();
            let barrier = KeyLoadBarrier::new();
            let gate = barrier.0.clone();
            let finished = Arc::new(AtomicBool::new(false));
            let loaded = finished.clone();
            let (entered, entered_rx) = oneshot::channel();
            let (explode, exploded) = oneshot::channel::<()>();
            let (state_tx, mut state) = watch::channel("starting");
            let mut task = tokio::spawn(owned_worker(policy, state_tx, async move {
                let loading = crate::ssh::headless_policy::load_private_key(move || {
                    let _ = entered.send(());
                    let (open, changed) = &*gate;
                    let mut open = open.lock().unwrap();
                    while !*open {
                        open = changed.wait(open).unwrap();
                    }
                    loaded.store(true, Ordering::Release);
                    Err(anyhow!("synthetic key failure"))
                });
                tokio::select! {
                    _ = exploded => panic!("synthetic operation panic after starting a key loader"),
                    _ = loading => panic!("loader escaped its preparation barrier"),
                }
            }));
            tokio::time::timeout(Duration::from_secs(2), entered_rx)
                .await
                .unwrap()
                .unwrap();
            explode.send(()).unwrap();
            tokio::time::timeout(Duration::from_secs(2), async {
                while *state.borrow_and_update() != "stopping" {
                    state.changed().await.unwrap();
                }
            })
            .await
            .unwrap();
            assert!(!finished.load(Ordering::Acquire));
            assert!(tokio::time::timeout(Duration::from_millis(25), &mut task)
                .await
                .is_err());
            barrier.open();
            assert!(
                !task.await.unwrap(),
                "panic must not become a successful cleanup ACK"
            );
            assert!(finished.load(Ordering::Acquire));
            assert_eq!(*state.borrow(), "cleanup_unconfirmed");
        },
    );
}

#[test]
fn expiring_preparation_retains_its_slot_until_the_job_is_joined() {
    isolated("tunnel::owner::lifecycle_tests::expiring_preparation_retains_its_slot_until_the_job_is_joined", || async {
        let owner = ForwardOwner::new(false);
        let barrier = KeyLoadBarrier::new();
        let gate = barrier.0.clone();
        let (entered, entered_rx) = oneshot::channel();
        let deadline = Instant::now() + Duration::from_secs(2);
        let accepted = owner.begin("expiring-preparation".into(), 0, Some(deadline), move || {
            let _ = entered.send(());
            let (open, changed) = &*gate;
            let mut open = open.lock().unwrap();
            while !*open { open = changed.wait(open).unwrap(); }
            Err(anyhow!("late synthetic preparation result"))
        }).unwrap();
        tokio::time::timeout(Duration::from_secs(2), entered_rx).await.unwrap().unwrap();
        // The barrier, not a sleep, holds the actual job across its deadline.
        tokio::time::timeout(Duration::from_secs(3), async {
            while owner.status(&accepted.handle).unwrap().state != "stopping" {
                tokio::task::yield_now().await;
            }
        }).await.unwrap();
        let entry = owner.entry(&accepted.handle).unwrap();
        assert!(!entry.completed.load(Ordering::Acquire));
        assert!(owner.begin("expiring-preparation".into(), 0, None,
            || panic!("expired but unjoined work lost its slot")).is_err());
        barrier.open();
        wait_status(&owner, &accepted.handle, "authorization_expired").await;
        assert!(entry.task.lock().await.is_none());
        owner.shutdown().await.unwrap();
    });
}

async fn exercise() {
    let denied = ForwardOwner::unattended();
    let mut unauthorized = Session::new_empty();
    unauthorized.forwards.push(PortForward {
        kind: "dynamic".into(),
        bind_port: 0,
        ..Default::default()
    });
    assert!(denied
        .start(unauthorized, vec![], 0, 1, false, None)
        .await
        .is_err());
    assert!(denied.entries.lock().unwrap().is_empty());
    let owner = ForwardOwner::new(false);
    for generation in 1..=2 {
        let mut fixture = Fixture::new(0, false).await;
        let ready = owner
            .start(
                fixture.target.clone(),
                fixture.jumps.clone(),
                1,
                generation,
                true,
                None,
            )
            .await
            .unwrap();
        let address: SocketAddr = ready
            .proxy_url
            .strip_prefix("socks5h://")
            .unwrap()
            .parse()
            .unwrap();
        assert_ne!(address.port(), 0);
        let again = owner
            .start(
                fixture.target.clone(),
                fixture.jumps.clone(),
                1,
                generation,
                true,
                None,
            )
            .await
            .unwrap();
        assert_eq!(ready.handle, again.handle);
        assert!(owner
            .start(
                fixture.target.clone(),
                fixture.jumps.clone(),
                1,
                generation + 1,
                true,
                None
            )
            .await
            .is_err());
        let other = ForwardOwner::new(false);
        assert!(other.status(&ready.handle).is_err());
        assert!(other.release(&ready.handle).await.is_err());
        let mut occupied = fixture.target.clone();
        occupied.forwards[1].bind_port = address.port();
        assert!(other
            .start(occupied, fixture.jumps.clone(), 1, generation, true, None)
            .await
            .is_err());
        let trust_path = crate::config::data_dir().join("known_hosts");
        let original_trust = std::fs::read_to_string(&trust_path).unwrap();
        std::fs::write(&trust_path, "# explicitly removed synthetic trust").unwrap();
        assert!(owner
            .start(
                fixture.target.clone(),
                fixture.jumps.clone(),
                1,
                generation,
                true,
                None
            )
            .await
            .is_err());
        std::fs::write(&trust_path, original_trust).unwrap();
        let mut client = echo(address).await;
        fixture.reject_unrequested_remote_maps().await;
        assert_eq!(
            owner.release(&ready.handle).await.unwrap().state,
            "released"
        );
        let mut byte = [0];
        assert_eq!(client.read(&mut byte).await.unwrap(), 0);
        assert!(tokio::net::TcpStream::connect(address).await.is_err());
        let rebound = TcpListener::bind(address).await.unwrap();
        drop(rebound);
        fixture.finish(true).await;
        for auth in &fixture.auth {
            assert_eq!(auth.load(Ordering::SeqCst), 1);
        }
    }
    let mut bad = Fixture::new(0, true).await;
    assert!(owner
        .start(bad.target.clone(), bad.jumps.clone(), 1, 3, false, None)
        .await
        .is_err());
    bad.finish(false).await;
    assert_eq!(bad.auth[2].load(Ordering::SeqCst), 0);
    owner.shutdown().await.unwrap();
    assert!(owner
        .start(bad.target.clone(), bad.jumps.clone(), 1, 4, true, None)
        .await
        .is_err());
}

#[test]
fn headless_owner_three_node_lifecycle() {
    isolated(
        "tunnel::owner::lifecycle_tests::headless_owner_three_node_lifecycle",
        exercise,
    );
}

pub(super) fn isolated<F, Fut>(exact: &str, exercise: F)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    const MARKER: &str = "XENTERM_SYNTHETIC_OWNER_CHILD";
    if let Some(root) = std::env::var_os(MARKER) {
        crate::config::configure_profile_from_env().unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(30), exercise())
                .await
                .unwrap();
        });
        std::fs::write(
            std::path::Path::new(&root).join("completed"),
            b"actual-three-node-fixture-completed",
        )
        .unwrap();
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command.env_clear();
    for name in [
        "PATH",
        "SYSTEMROOT",
        "WINDIR",
        "LD_LIBRARY_PATH",
        "DYLD_LIBRARY_PATH",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    for name in [
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
        "TMP",
        "TEMP",
        "TMPDIR",
        "XENTERM_DATA_DIR",
    ] {
        let path = directory.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        command.env(name, path);
    }
    command.env(MARKER, directory.path()).args([
        "--exact",
        exact,
        "--test-threads=1",
        "--nocapture",
    ]);
    // No shell, inherited proxy/auth environment, or external server is used.
    let mut child = command.spawn().unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(40);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            assert_eq!(
                std::fs::read(directory.path().join("completed")).unwrap(),
                b"actual-three-node-fixture-completed"
            );
            break;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("isolated owner fixture exceeded deadline");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

// Regression candidates only: these tests have not yet been compiled or run.
// The isolated helper above pins a temporary profile and clears inherited
// credentials/proxy environment before any ConfigStore access.
fn save_permission_fixture(allowed: bool) {
    let root = std::env::var_os("XENTERM_SYNTHETIC_OWNER_CHILD")
        .expect("permission fixture requires the isolated child");
    assert!(crate::config::has_explicit_data_dir());
    assert!(crate::config::data_dir().starts_with(std::path::Path::new(&root)));
    // Explicit profiles disable native credential storage in ConfigStore::load;
    // the new fixture contains no sessions and uses only its local secret.key.
    let mut store = crate::config::ConfigStore::load().unwrap();
    store.set_mcp_enabled(allowed);
    store.set_mcp_use_saved_credentials(allowed);
    store.set_mcp_allow_commands(allowed);
    store.save().unwrap();
}

#[test]
fn permission_profile_lock_must_not_stall_cancellation_timer() {
    isolated("tunnel::owner::lifecycle_tests::permission_profile_lock_must_not_stall_cancellation_timer", || async {
        save_permission_fixture(true);
        // A separate OS thread releases the actual profile lock even if the
        // current-thread executor is stalled: failure cannot strand the lock.
        // No lockfile is deleted or replaced; the existing inode is preserved.
        let path = crate::config::data_dir().join("sessions.db.profile-lock");
        let (locked, ready) = std::sync::mpsc::channel();
        let (attempting, attempt) = std::sync::mpsc::channel();
        let (completed, completion) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let file = std::fs::OpenOptions::new().read(true).write(true).open(path).unwrap();
            fs2::FileExt::lock_exclusive(&file).unwrap();
            locked.send(()).unwrap();
            attempt.recv_timeout(Duration::from_secs(5)).unwrap();
            // Success requires cancellation to complete while this exact lock
            // is still held. The timeout only bounds a broken implementation;
            // elapsed wall time is not the correctness assertion.
            let cancelled_while_locked = completion.recv_timeout(Duration::from_secs(4)).is_ok();
            // Closing the handle releases the lock on Unix and Windows.
            drop(file);
            cancelled_while_locked
        });
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        // Use a separate current-thread runtime, not nested block_on on the
        // fixture runtime. Tokio timeouts cannot preempt synchronous polling.
        let cancelled_while_locked = tokio::task::spawn_blocking(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            attempting.send(()).unwrap();
            let mut permissions = PermissionChecks::default();
            runtime.block_on(async {
                let (stop, stopped) = watch::channel(false);
                let (_shutdown, shutdown) = watch::channel(false);
                let cancellation = cancelled(stopped, shutdown, true, None, &mut permissions);
                tokio::pin!(cancellation);
                tokio::select! {
                    biased;
                    reason = &mut cancellation => panic!("unexpected early cancellation: {reason}"),
                    _ = tokio::time::sleep(Duration::from_millis(50)) => {},
                }
                stop.send_replace(true);
                assert_eq!(cancellation.await, "released");
            });
            // A send after the lock-holder timed out is intentionally an error:
            // the joined flag below proves whether cancellation preceded unlock.
            let _ = completed.send(());
            let cancelled_while_locked = holder.join().unwrap();
            assert!(runtime.block_on(permissions.drain()));
            assert!(permissions.job.is_none(), "permission read survived its owner's drain");
            cancelled_while_locked
        }).await.unwrap();
        assert!(cancelled_while_locked,
            "profile permission I/O prevented cancellation while the profile lock was held");
    });
}

#[test]
fn revoked_permission_during_preparation_must_not_bind_late_route() {
    isolated("tunnel::owner::lifecycle_tests::revoked_permission_during_preparation_must_not_bind_late_route", || async {
        save_permission_fixture(true);
        let owner = ForwardOwner::unattended();
        let session = saved_dynamic(0);
        let barrier = KeyLoadBarrier::new();
        let gate = barrier.0.clone();
        let (entered, ready) = oneshot::channel();
        let accepted = owner.begin(session.id.clone(), 0, None, move || {
            let prepared = PreparedForward::capture(session, vec![], 0, 1)?;
            let _ = entered.send(());
            let (open, changed) = &*gate;
            let mut open = open.lock().unwrap();
            while !*open { open = changed.wait(open).unwrap(); }
            Ok(prepared)
        }).unwrap();
        tokio::time::timeout(Duration::from_secs(2), ready).await.unwrap().unwrap();
        // This save occurs strictly after initial admission, before the frozen
        // route returns. A later SSH refusal does not prove no listener bound.
        save_permission_fixture(false);
        barrier.open();
        wait_status(&owner, &accepted.handle, "permission_revoked").await;
        let entry = owner.entry(&accepted.handle).unwrap();
        assert!(entry.identity.lock().unwrap().is_none(), "revoked preparation reached listener setup");
        assert!(entry.proxy_url.lock().unwrap().is_empty(), "revoked preparation bound a listener before SSH refusal");
        owner.shutdown().await.unwrap();
    });
}

#[test]
fn external_permission_writer_fixture() {
    // The helper is inert in ordinary full-suite execution. Only an already
    // isolated parent may opt in; it supplies a private completion marker.
    let Some(marker) = std::env::var_os("XENTERM_PERMISSION_WRITER_DONE") else { return; };
    assert!(std::env::var_os("XENTERM_SYNTHETIC_OWNER_CHILD").is_some());
    crate::config::configure_profile_from_env().unwrap();
    assert_eq!(std::path::Path::new(&marker).parent(), Some(crate::config::data_dir().as_path()));
    save_permission_fixture(false);
    std::fs::write(marker, b"permission-revocation-committed").unwrap();
}

#[test]
fn permission_read_observes_other_process_revocation_not_cached_store() {
    isolated("tunnel::owner::lifecycle_tests::permission_read_observes_other_process_revocation_not_cached_store", || async {
        save_permission_fixture(true);
        let stale = crate::config::ConfigStore::load().unwrap();
        assert!(permitted(true));
        let marker = crate::config::data_dir().join("permission-writer-done");
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command.env_clear();
        for name in ["PATH", "SYSTEMROOT", "WINDIR", "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH",
            "HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "XDG_DATA_HOME",
            "XDG_CACHE_HOME", "XDG_STATE_HOME", "TMP", "TEMP", "TMPDIR", "XENTERM_DATA_DIR",
            "XENTERM_SYNTHETIC_OWNER_CHILD"] {
            if let Some(value) = std::env::var_os(name) { command.env(name, value); }
        }
        command.env("XENTERM_PERMISSION_WRITER_DONE", &marker).args([
            "--exact", "tunnel::owner::lifecycle_tests::external_permission_writer_fixture",
            "--test-threads=1", "--nocapture",
        ]);
        // A cancelled fixture future or a wait error must not leave a writer
        // running against its temporary profile after the parent has failed.
        struct WriterChild(std::process::Child);
        impl Drop for WriterChild {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut child = WriterChild(command.spawn().unwrap());
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() { break status; }
            if std::time::Instant::now() >= deadline {
                panic!("permission writer exceeded its deadline");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert!(status.success());
        assert_eq!(std::fs::read(marker).unwrap(), b"permission-revocation-committed");
        assert!(stale.mcp_enabled(), "control must retain the old in-memory snapshot");
        assert!(!permitted(true), "external revocation must not reuse an old GUI store snapshot");
    });
}

async fn owner_waits_for_blocked_permission_read(close_owner: bool) {
    save_permission_fixture(true);
    let silent = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut session = saved_dynamic(0);
    session.proxy = format!("http://{}", silent.local_addr().unwrap());
    let probe = Arc::new(PermissionProbe::new());
    let mut entered = probe.entered.subscribe();
    let mut draining = probe.draining.subscribe();
    let mut owner = ForwardOwner::unattended();
    owner.permission_probe = Some(probe.clone());
    let owner = Arc::new(owner);
    let accepted = owner.begin(session.id.clone(), 0, None, move || {
        PreparedForward::capture(session, vec![], 0, 1)
    }).unwrap();
    let (_transport, _) = tokio::time::timeout(Duration::from_secs(2), silent.accept()).await.unwrap().unwrap();
    // This per-owner gate confirms the actual tracked blocking reader started.
    // The separate real-profile-lock regression covers ConfigStore lock I/O.
    tokio::time::timeout(Duration::from_secs(2), entered.wait_for(|entered| *entered)).await.unwrap().unwrap();
    assert!(!*probe.gate.lock().unwrap());
    let mut closing = if close_owner {
        let owner = owner.clone();
        Some(tokio::spawn(async move { owner.shutdown().await }))
    } else {
        assert_eq!(owner.request_release(&accepted.handle).unwrap().state, "stopping");
        None
    };
    // Not merely a slow transport: the route operation has exited and the owner
    // is specifically joining this still-gated permission reader.
    tokio::time::timeout(Duration::from_secs(2), draining.wait_for(|draining| *draining)).await.unwrap().unwrap();
    assert!(!*probe.gate.lock().unwrap());
    if let Some(closing) = closing.as_mut() {
        assert!(tokio::time::timeout(Duration::from_millis(150), closing).await.is_err(),
            "shutdown acknowledged before its permission reader joined");
    } else {
        assert!(tokio::time::timeout(Duration::from_millis(150), owner.release(&accepted.handle)).await.is_err(),
            "release acknowledged before its permission reader joined");
    }
    let entry = owner.entry(&accepted.handle).unwrap();
    assert!(!entry.completed.load(Ordering::Acquire));
    if close_owner {
        // The background shutdown still owns the join lock while it drains.
        // Never await that lock before opening the reader gate it depends on.
        assert!(entry.task.try_lock().is_err(), "shutdown lost its pending join");
    } else {
        assert!(entry.task.lock().await.is_some(), "timed-out release lost the worker handle");
    }
    probe.open();
    if let Some(closing) = closing { closing.await.unwrap().unwrap(); }
    assert_eq!(owner.release(&accepted.handle).await.unwrap().state, "released");
    assert!(entry.completed.load(Ordering::Acquire));
    assert!(entry.task.lock().await.is_none());
    owner.shutdown().await.unwrap();
}

#[test]
fn owner_release_waits_for_blocked_permission_read() {
    isolated("tunnel::owner::lifecycle_tests::owner_release_waits_for_blocked_permission_read", || async {
        owner_waits_for_blocked_permission_read(false).await;
    });
}

#[test]
fn owner_shutdown_waits_for_blocked_permission_read() {
    isolated("tunnel::owner::lifecycle_tests::owner_shutdown_waits_for_blocked_permission_read", || async {
        owner_waits_for_blocked_permission_read(true).await;
    });
}

#[test]
fn operation_panic_drains_in_flight_permission_reader_without_clean_ack() {
    isolated("tunnel::owner::lifecycle_tests::operation_panic_drains_in_flight_permission_reader_without_clean_ack", || async {
        save_permission_fixture(true);
        let session = saved_dynamic(0);
        let mut probe = PermissionProbe::new();
        probe.panic_operation = true;
        let probe = Arc::new(probe);
        let mut entered = probe.entered.subscribe();
        let mut draining = probe.draining.subscribe();
        let mut owner = ForwardOwner::unattended();
        owner.permission_probe = Some(probe.clone());
        let accepted = owner.begin(session.id.clone(), 0, None, move || {
            PreparedForward::capture(session, vec![], 0, 1)
        }).unwrap();
        tokio::time::timeout(Duration::from_secs(2), entered.wait_for(|entered| *entered)).await.unwrap().unwrap();
        // The injected panic is inside run_with_permissions, not a direct call
        // to tracker.drain. Reaching this signal proves run caught it and joined.
        tokio::time::timeout(Duration::from_secs(2), draining.wait_for(|draining| *draining)).await.unwrap().unwrap();
        assert!(!*probe.gate.lock().unwrap());
        let entry = owner.entry(&accepted.handle).unwrap();
        assert!(!entry.completed.load(Ordering::Acquire));
        assert!(tokio::time::timeout(Duration::from_millis(150), owner.release(&accepted.handle)).await.is_err(),
            "panic path acknowledged cleanup before its permission reader joined");
        assert!(entry.task.lock().await.is_some());
        probe.open();
        let result = owner.release(&accepted.handle).await;
        assert!(result.err().unwrap().to_string().contains("cleanup_unconfirmed"));
        assert_eq!(owner.status(&accepted.handle).unwrap().state, "cleanup_unconfirmed");
        assert!(!entry.completed.load(Ordering::Acquire), "operation panic became a clean ACK");
        assert!(entry.unconfirmed.load(Ordering::Acquire));
        assert!(entry.task.lock().await.is_none(), "permission drain did not finish the retained worker");
    });
}
