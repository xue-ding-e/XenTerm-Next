//! SSH port forwarding / tunnels (#56).
//!
//! Local (-L) and dynamic (-D / SOCKS5) forwards run client-side: we listen on
//! a local TCP port and, per inbound connection, open a `direct-tcpip` channel
//! on the SSH session, then splice the two streams together. Remote (-R)
//! forwards are requested with `tcpip_forward` and serviced in the session
//! handler when the server opens channels back (see `ssh.rs`).

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use russh::client::{Handle, Msg};
use russh::Channel;
use tokio::io::{copy_bidirectional, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use crate::session::protocol::SessionEvent;
use crate::ssh::ClientHandler;

/// Emit a one-line notice into the terminal output stream.
fn notice(events: &UnboundedSender<SessionEvent>, msg: String) {
    let _ = events.send(SessionEvent::Output(format!("\r\n[xenterm] {msg}\r\n")));
}

fn bind_target(bind_addr: &str, bind_port: u16) -> String {
    let addr = if bind_addr.trim().is_empty() {
        "127.0.0.1"
    } else {
        bind_addr.trim()
    };
    // IPv6 literals must be bracketed for TcpListener::bind ("[::1]:8080");
    // an already-bracketed address is left as-is (#109).
    if addr.contains(':') && !addr.starts_with('[') {
        format!("[{addr}]:{bind_port}")
    } else {
        format!("{addr}:{bind_port}")
    }
}

/// Open a `direct-tcpip` channel to `host:port`, recording the originating peer
/// (some servers log / ACL on it).
async fn open_direct(
    handle: &Arc<Handle<ClientHandler>>,
    host: &str,
    port: u16,
    peer: SocketAddr,
) -> Result<Channel<Msg>, russh::Error> {
    handle
        .channel_open_direct_tcpip(
            host.to_string(),
            port as u32,
            peer.ip().to_string(),
            peer.port() as u32,
        )
        .await
}

/// A listener and all of its accepted clients share one lifetime. A successful
/// stop acknowledges that both have been dropped, so an immediate restart can
/// safely bind the same port. Dropping the owner also requests cancellation.
pub(crate) struct ListenerTask {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: JoinHandle<std::io::Result<()>>,
}

impl ListenerTask {
    pub(crate) fn spawn<F, Fut>(run: F) -> Self
    where F: FnOnce(tokio::sync::oneshot::Receiver<()>) -> Fut,
        Fut: std::future::Future<Output = std::io::Result<()>> + Send + 'static,
    {
        let (stop, stopped) = tokio::sync::oneshot::channel();
        Self { stop: Some(stop), task: tokio::spawn(run(stopped)) }
    }

    pub(crate) fn is_finished(&self) -> bool { self.task.is_finished() }

    pub(crate) async fn stop(mut self) -> std::io::Result<()> {
        if let Some(stop) = self.stop.take() { let _ = stop.send(()); }
        (&mut self.task).await.map_err(std::io::Error::other)?
    }
}

impl Drop for ListenerTask {
    fn drop(&mut self) {
        // Dropping the sender wakes the shutdown branch, including when the
        // enclosing SSH pump is cancelled rather than ending normally.
        self.stop.take();
    }
}

async fn bind_listener(forward: &crate::config::PortForward) -> std::io::Result<TcpListener> {
    tokio::time::timeout(std::time::Duration::from_secs(5),
        TcpListener::bind(bind_target(&forward.bind_addr, forward.bind_port))).await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "listener bind timed out"))?
}

/// Execute the same normalized destination used by runtime identity and row
/// reconstruction. In particular, imported whitespace must not make the first
/// start connect somewhere different from a restart of its displayed row.
fn forward_target(forward: &crate::config::PortForward) -> Option<(String, u16)> {
    if forward.kind == "dynamic" { None }
    else { Some((forward.host.trim().to_string(), forward.host_port)) }
}

/// Bind before returning an owner: returning Ok is the readiness acknowledgement,
/// not merely evidence that a background task was scheduled.
pub(crate) async fn start_forward(
    handle: Arc<Handle<ClientHandler>>,
    forward: crate::config::PortForward,
    events: UnboundedSender<SessionEvent>,
) -> std::io::Result<ListenerTask> {
    let bind = bind_target(&forward.bind_addr, forward.bind_port);
    let listener = bind_listener(&forward).await?;
    let target = forward_target(&forward);
    notice(&events, match &target {
        Some((host, port)) => format!("-L {bind} → {host}:{port}"),
        None => format!("-D {bind} (SOCKS5)"),
    });
    Ok(ListenerTask::spawn(|stopped| run_listener(handle, listener, target, Some(events), async move {
        let _ = stopped.await;
    })))
}

async fn run_listener(
    handle: Arc<Handle<ClientHandler>>,
    listener: TcpListener,
    target: Option<(String, u16)>,
    events: Option<UnboundedSender<SessionEvent>>,
    shutdown: impl std::future::Future<Output = ()>,
) -> std::io::Result<()> {
    let health_handle = handle.clone();
    serve_listener(listener, shutdown, move || health_handle.is_closed(), move |mut inbound, peer| {
        let handle = handle.clone();
        let target = target.clone();
        let events = events.clone();
        async move {
            if let Some((host, port)) = target {
                match open_direct(&handle, &host, port, peer).await {
                    Ok(channel) => {
                        let mut stream = channel.into_stream();
                        let _ = copy_bidirectional(&mut inbound, &mut stream).await;
                    }
                    Err(error) => if let Some(events) = events {
                        notice(&events, format!("-L {host}:{port} 连接失败 / open failed: {error}"));
                    },
                }
            } else {
                let _ = socks5_serve(&handle, inbound, peer).await;
            }
        }
    }).await
}

/// The common accept/drain boundary is testable without an SSH server. The
/// production closure owns each complete local or SOCKS forwarding operation.
async fn serve_listener<F, Fut>(
    listener: TcpListener,
    shutdown: impl std::future::Future<Output = ()>,
    closed: impl Fn() -> bool,
    serve: F,
) -> std::io::Result<()>
where F: Fn(TcpStream, SocketAddr) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::pin!(shutdown);
    let mut clients = tokio::task::JoinSet::new();
    let mut health = tokio::time::interval(std::time::Duration::from_millis(200));
    let result = loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => break Ok(()),
            _ = health.tick() => {
                if closed() {
                    break Err(std::io::Error::new(std::io::ErrorKind::ConnectionAborted,
                        "SSH transport disconnected"));
                }
            }
            // Completed tasks still count toward JoinSet::len until reaped.
            // With biased selection, drain them before a perpetually ready
            // accept backlog so the client cap measures live clients, not history.
            _ = clients.join_next(), if !clients.is_empty() => {}
            client = listener.accept() => {
                let (inbound, peer) = match client {
                    Ok(client) => client,
                    Err(error) => break Err(error),
                };
                // Bound unauthenticated local SOCKS clients as well as SSH channels.
                if clients.len() >= 128 { drop(inbound); continue; }
                clients.spawn(serve(inbound, peer));
            }
        }
    };
    // Close the accepting socket first; stop is not acknowledged until every
    // accepted connection has also released its socket/channel.
    drop(listener);
    clients.abort_all();
    while clients.join_next().await.is_some() {}
    result
}

/// Handle one SOCKS5 client connection end-to-end.
async fn socks5_serve(
    handle: &Arc<Handle<ClientHandler>>,
    mut inbound: TcpStream,
    peer: SocketAddr,
) -> std::io::Result<()> {
    // Greeting: VER, NMETHODS, METHODS[NMETHODS].
    let mut head = [0u8; 2];
    inbound.read_exact(&mut head).await?;
    if head[0] != 0x05 {
        return Ok(()); // not SOCKS5
    }
    let nmethods = head[1] as usize;
    let mut methods = vec![0u8; nmethods];
    inbound.read_exact(&mut methods).await?;
    // Reply: VER=5, METHOD=0 (no authentication).
    inbound.write_all(&[0x05, 0x00]).await?;

    // Request: VER, CMD, RSV, ATYP, DST.ADDR, DST.PORT.
    let mut req = [0u8; 4];
    inbound.read_exact(&mut req).await?;
    if req[0] != 0x05 {
        return Ok(());
    }
    if req[1] != 0x01 {
        // Only CONNECT is supported → reply "command not supported".
        let _ = inbound.write_all(&socks_reply(0x07)).await;
        return Ok(());
    }
    let host = match req[3] {
        0x01 => {
            let mut a = [0u8; 4];
            inbound.read_exact(&mut a).await?;
            Ipv4Addr::from(a).to_string()
        }
        0x04 => {
            let mut a = [0u8; 16];
            inbound.read_exact(&mut a).await?;
            Ipv6Addr::from(a).to_string()
        }
        0x03 => {
            let mut len = [0u8; 1];
            inbound.read_exact(&mut len).await?;
            let mut d = vec![0u8; len[0] as usize];
            inbound.read_exact(&mut d).await?;
            String::from_utf8_lossy(&d).into_owned()
        }
        _ => {
            let _ = inbound.write_all(&socks_reply(0x08)).await; // addr type unsupported
            return Ok(());
        }
    };
    let mut port = [0u8; 2];
    inbound.read_exact(&mut port).await?;
    let port = u16::from_be_bytes(port);

    match open_direct(handle, &host, port, peer).await {
        Ok(ch) => {
            inbound.write_all(&socks_reply(0x00)).await?; // succeeded
            let mut stream = ch.into_stream();
            let _ = copy_bidirectional(&mut inbound, &mut stream).await;
        }
        Err(_) => {
            let _ = inbound.write_all(&socks_reply(0x05)).await; // connection refused
        }
    }
    Ok(())
}

/// A SOCKS5 reply with the given reply code and a zeroed bound address
/// (`0.0.0.0:0`) — clients don't need the real bound address for CONNECT.
fn socks_reply(code: u8) -> [u8; 10] {
    [0x05, code, 0x00, 0x01, 0, 0, 0, 0, 0, 0]
}

#[cfg(test)]
mod listener_lifecycle_tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::timeout;

    #[test]
    fn first_start_and_restart_execute_the_same_normalized_destination() {
        let imported = crate::config::PortForward { kind: "local".into(),
            bind_addr: "127.0.0.1".into(), bind_port: 41080,
            host: "  service.invalid \t".into(), host_port: 8080, ..Default::default() };
        let mut displayed = imported.clone();
        displayed.host = imported.host.trim().to_string();
        assert_eq!(crate::core::tunnel::runtime_key(&imported), crate::core::tunnel::runtime_key(&displayed));
        assert_eq!(forward_target(&imported), Some(("service.invalid".into(), 8080)));
        assert_eq!(forward_target(&imported), forward_target(&displayed));
        displayed.host = "different.invalid".into();
        assert_ne!(forward_target(&imported), forward_target(&displayed));
        assert_ne!(crate::core::tunnel::runtime_key(&imported), crate::core::tunnel::runtime_key(&displayed));
        let mut dynamic = imported;
        dynamic.kind = "dynamic".into();
        assert_eq!(forward_target(&dynamic), None, "SOCKS has no fixed destination");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn completed_clients_are_reaped_before_an_already_ready_accept_backlog() {
        use std::future::Future;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::task::Poll;

        timeout(Duration::from_secs(10), async {
            const CAP: usize = 128;
            const BACKLOG: usize = 16;
            let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
            let address = listener.local_addr().unwrap();
            let (release, released) = tokio::sync::watch::channel(false);
            let (done, mut completed) = tokio::sync::mpsc::unbounded_channel();
            let (stop, stopped) = tokio::sync::oneshot::channel();
            let accepted = Arc::new(AtomicUsize::new(0));
            let count = accepted.clone();
            let server = serve_listener(listener, async move { let _ = stopped.await; }, || false,
                move |mut stream, _| {
                    let index = count.fetch_add(1, Ordering::SeqCst);
                    let mut released = released.clone();
                    let done = done.clone();
                    async move {
                        if index < CAP {
                            while !*released.borrow() { released.changed().await.unwrap(); }
                            // No await after the notification: on this current-thread
                            // runtime the task is finished before the test receives it.
                            let _ = done.send(());
                        } else {
                            stream.write_all(b"ok").await.unwrap();
                        }
                    }
                });
            tokio::pin!(server);
            let mut initial = Vec::new();
            for _ in 0..CAP { initial.push(TcpStream::connect(address).await.unwrap()); }
            // Drive accepts until exactly the live-client cap, then deliberately
            // stop polling the accept loop while its client tasks finish.
            while accepted.load(Ordering::SeqCst) < CAP {
                std::future::poll_fn(|cx| {
                    assert!(server.as_mut().poll(cx).is_pending());
                    Poll::Ready(())
                }).await;
                tokio::task::yield_now().await;
            }
            release.send(true).unwrap();
            for _ in 0..CAP { completed.recv().await.unwrap(); }
            let mut backlog = Vec::new();
            for _ in 0..BACKLOG { backlog.push(TcpStream::connect(address).await.unwrap()); }
            // Both JoinSet completions and all these accepts are now ready.
            // The old accept-first select drops this entire backlog at len=128.
            tokio::task::yield_now().await;
            let probes = async {
                for stream in &mut backlog {
                    let mut reply = [0; 2];
                    stream.read_exact(&mut reply).await.expect("completed clients must not consume capacity");
                    assert_eq!(&reply, b"ok");
                }
                stop.send(()).unwrap();
            };
            let (result, ()) = tokio::join!(server.as_mut(), probes);
            result.unwrap();
            assert_eq!(accepted.load(Ordering::SeqCst), CAP + BACKLOG);
            drop(initial);
        }).await.expect("bounded ready-backlog recovery");
    }

    #[tokio::test]
    async fn an_external_listener_is_reported_busy_and_never_adopted_or_closed() {
        timeout(Duration::from_secs(3), async {
            let external = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
            let address = external.local_addr().unwrap();
            let forward = crate::config::PortForward { kind: "dynamic".into(),
                bind_addr: "127.0.0.1".into(), bind_port: address.port(), ..Default::default() };
            let error = bind_listener(&forward).await.unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
            let client = TcpStream::connect(address).await.unwrap();
            let (accepted, _) = external.accept().await.unwrap();
            drop((client, accepted, external));
        }).await.expect("bounded external-owner check");
    }

    #[tokio::test]
    async fn stop_drains_accepted_clients_before_same_port_restart() {
        timeout(Duration::from_secs(3), async {
            let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
            let address = listener.local_addr().unwrap();
            let (accepted, mut ready) = tokio::sync::mpsc::unbounded_channel();
            let owner = ListenerTask::spawn(|stopped| serve_listener(listener,
                async move { let _ = stopped.await; }, || false,
                move |mut stream, _| {
                    let accepted = accepted.clone();
                    async move {
                        let _ = accepted.send(());
                        let mut byte = [0];
                        let _ = stream.read(&mut byte).await;
                    }
                }));
            let mut client = TcpStream::connect(address).await.unwrap();
            ready.recv().await.unwrap();
            owner.stop().await.unwrap();
            let listener = TcpListener::bind(address).await.expect("stop acknowledged listener release");
            let mut byte = [0];
            assert_eq!(client.read(&mut byte).await.unwrap(), 0, "accepted client also closed");
            drop(listener);
        }).await.expect("bounded loopback lifecycle");
    }

    #[tokio::test]
    async fn dropping_owner_requests_listener_and_client_cleanup() {
        timeout(Duration::from_secs(3), async {
            let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
            let address = listener.local_addr().unwrap();
            let (accepted, mut ready) = tokio::sync::mpsc::unbounded_channel();
            let owner = ListenerTask::spawn(|stopped| serve_listener(listener,
                async move { let _ = stopped.await; }, || false,
                move |mut stream, _| {
                    let accepted = accepted.clone();
                    async move {
                        let _ = accepted.send(());
                        let mut byte = [0];
                        let _ = stream.read(&mut byte).await;
                    }
                }));
            let mut client = TcpStream::connect(address).await.unwrap();
            ready.recv().await.unwrap();
            drop(owner);
            let mut byte = [0];
            assert_eq!(client.read(&mut byte).await.unwrap(), 0);
            let listener = TcpListener::bind(address).await.unwrap();
            drop(listener);
        }).await.expect("dropping an owner cannot leave an orphan listener");
    }

}
