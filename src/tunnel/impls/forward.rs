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
use tokio::io::{copy_bidirectional, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
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

/// Local forward (-L): listen locally and tunnel each connection to
/// `target_host:target_port` reached from the SSH server's side.
pub fn spawn_local(
    handle: Arc<Handle<ClientHandler>>,
    bind_addr: String,
    bind_port: u16,
    target_host: String,
    target_port: u16,
    events: UnboundedSender<SessionEvent>,
) -> JoinHandle<()> {
    let bind = bind_target(&bind_addr, bind_port);
    tokio::spawn(async move {
        let listener = match TcpListener::bind(&bind).await {
            Ok(l) => l,
            Err(e) => {
                notice(&events, format!("-L {bind} 监听失败 / bind failed: {e}"));
                return;
            }
        };
        notice(&events, format!("-L {bind} → {target_host}:{target_port}"));
        loop {
            let (mut inbound, peer) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => break,
            };
            let handle = handle.clone();
            let host = target_host.clone();
            let ev = events.clone();
            tokio::spawn(async move {
                match open_direct(&handle, &host, target_port, peer).await {
                    Ok(ch) => {
                        let mut stream = ch.into_stream();
                        let _ = copy_bidirectional(&mut inbound, &mut stream).await;
                    }
                    Err(e) => notice(
                        &ev,
                        format!("-L {host}:{target_port} 连接失败 / open failed: {e}"),
                    ),
                }
            });
        }
    })
}

/// Dynamic forward (-D): a minimal SOCKS5 proxy. Each accepted connection
/// negotiates SOCKS5 (no auth, CONNECT only), then we open a `direct-tcpip`
/// channel to the requested destination and splice.
pub fn spawn_dynamic(
    handle: Arc<Handle<ClientHandler>>,
    bind_addr: String,
    bind_port: u16,
    events: UnboundedSender<SessionEvent>,
) -> JoinHandle<()> {
    let bind = bind_target(&bind_addr, bind_port);
    tokio::spawn(async move {
        let listener = match TcpListener::bind(&bind).await {
            Ok(l) => l,
            Err(e) => {
                notice(&events, format!("-D {bind} 监听失败 / bind failed: {e}"));
                return;
            }
        };
        notice(&events, format!("-D {bind} (SOCKS5)"));
        loop {
            let (inbound, peer) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => break,
            };
            let handle = handle.clone();
            let ev = events.clone();
            tokio::spawn(async move {
                if let Err(e) = socks5_serve(&handle, inbound, peer).await {
                    tracing::debug!("socks5 conn ended: {e}");
                    let _ = ev; // notices for SOCKS are too noisy; keep to trace
                }
            });
        }
    })
}

/// Negotiate no-auth without consuming any bytes from the following request.
async fn socks5_negotiate<S>(inbound: &mut S) -> std::io::Result<bool>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // Greeting: VER, NMETHODS, METHODS[NMETHODS].
    let mut head = [0u8; 2];
    inbound.read_exact(&mut head).await?;
    if head[0] != 0x05 {
        return Ok(false); // not SOCKS5
    }
    let nmethods = head[1] as usize;
    let mut methods = vec![0u8; nmethods];
    inbound.read_exact(&mut methods).await?;
    if !methods.contains(&0x00) {
        // RFC 1928 section 3: select only an offered method, otherwise reject.
        inbound.write_all(&[0x05, 0xff]).await?;
        return Ok(false);
    }
    // Reply: VER=5, METHOD=0 (no authentication).
    inbound.write_all(&[0x05, 0x00]).await?;
    Ok(true)
}

/// Handle one SOCKS5 client connection end-to-end.
async fn socks5_serve(
    handle: &Arc<Handle<ClientHandler>>,
    mut inbound: TcpStream,
    peer: SocketAddr,
) -> std::io::Result<()> {
    if !socks5_negotiate(&mut inbound).await? {
        return Ok(());
    }

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
mod tests {
    use super::*;
    use std::io::ErrorKind;
    use std::time::Duration;

    use tokio::io::duplex;
    use tokio::time::timeout;

    // Only in-memory streams are used: no listener, SSH session or destination
    // connection is needed to check method selection and frame boundaries.
    async fn negotiate(input: &[u8]) -> (std::io::Result<bool>, Vec<u8>, Vec<u8>) {
        let (mut client, mut server) = duplex(512);
        let client_io = async {
            client.write_all(input).await.unwrap();
            client.shutdown().await.unwrap();
            let mut reply = Vec::new();
            client.read_to_end(&mut reply).await.unwrap();
            reply
        };
        let server_io = async {
            let result = socks5_negotiate(&mut server).await;
            let mut remaining = Vec::new();
            server.read_to_end(&mut remaining).await.unwrap();
            drop(server);
            (result, remaining)
        };
        let (reply, (result, remaining)) = timeout(Duration::from_secs(2), async {
            tokio::join!(client_io, server_io)
        })
        .await
        .expect("in-memory SOCKS5 negotiation must finish");
        (result, reply, remaining)
    }

    #[tokio::test]
    async fn socks5_negotiation_accepts_offered_no_auth() {
        for methods in [&[0x00][..], &[0x02, 0x00, 0x01], &[0x02, 0x01, 0x00]] {
            let mut input = vec![0x05, methods.len() as u8];
            input.extend_from_slice(methods);
            let (result, reply, remaining) = negotiate(&input).await;
            assert!(result.unwrap());
            assert_eq!(reply, [0x05, 0x00]);
            assert!(remaining.is_empty());
        }
    }

    #[tokio::test]
    async fn socks5_negotiation_rejects_unoffered_no_auth() {
        for methods in [&[0x02][..], &[0x01, 0x02], &[0xff], &[]] {
            let mut input = vec![0x05, methods.len() as u8];
            input.extend_from_slice(methods);
            let (result, reply, remaining) = negotiate(&input).await;
            assert!(!result.unwrap());
            assert_eq!(reply, [0x05, 0xff]);
            assert!(remaining.is_empty());
        }
    }

    #[tokio::test]
    async fn socks5_negotiation_reads_all_255_methods() {
        for last_method in [0x02, 0x00] {
            let mut input = vec![0x05, 0xff];
            input.extend_from_slice(&[0x02; 254]);
            input.push(last_method);
            let (result, reply, remaining) = negotiate(&input).await;
            let accepted = last_method == 0x00;
            assert_eq!(result.unwrap(), accepted);
            assert_eq!(reply, [0x05, if accepted { 0x00 } else { 0xff }]);
            assert!(remaining.is_empty());
        }
    }

    #[tokio::test]
    async fn socks5_negotiation_handles_fragmented_greeting_without_waiting_for_eof() {
        // Capacity one forces every greeting byte to arrive separately.
        let (mut client, mut server) = duplex(1);
        let client_io = async {
            client.write_all(&[0x05, 0x03, 0x02, 0x01, 0x00]).await.unwrap();
            let mut reply = [0u8; 2];
            client.read_exact(&mut reply).await.unwrap();
            assert_eq!(reply, [0x05, 0x00]);
        };
        let server_io = async {
            assert!(socks5_negotiate(&mut server).await.unwrap());
        };
        timeout(Duration::from_secs(2), async {
            tokio::join!(client_io, server_io);
        })
        .await
        .expect("fragmented greeting must finish before the client closes");
    }

    #[tokio::test]
    async fn socks5_negotiation_preserves_coalesced_request_and_payload() {
        let next_frame = [0x05, 0x01, 0x00, 0x01, 203, 0, 113, 7, 0x15, 0x38, 42];
        let mut input = vec![0x05, 0x01, 0x00];
        input.extend_from_slice(&next_frame);
        let (result, reply, remaining) = negotiate(&input).await;
        assert!(result.unwrap());
        assert_eq!(reply, [0x05, 0x00]);
        assert_eq!(remaining, next_frame);
    }

    #[tokio::test]
    async fn socks5_negotiation_does_not_select_a_method_from_the_next_frame() {
        let next_frame = [0x05, 0x01, 0x00, 0x01, 203, 0, 113, 7, 0x15, 0x38];
        let mut input = vec![0x05, 0x01, 0x02];
        input.extend_from_slice(&next_frame);
        let (result, reply, remaining) = negotiate(&input).await;
        assert!(!result.unwrap());
        assert_eq!(reply, [0x05, 0xff]);
        assert_eq!(remaining, next_frame);
    }

    #[tokio::test]
    async fn socks5_negotiation_rejects_truncated_frames_without_reply() {
        for input in [
            &[][..],
            &[0x05],
            &[0x05, 0x01],
            &[0x05, 0x02, 0x02],
            &[0x05, 0x02, 0x00],
        ] {
            let (result, reply, remaining) = negotiate(input).await;
            assert_eq!(result.unwrap_err().kind(), ErrorKind::UnexpectedEof);
            assert!(reply.is_empty());
            assert!(remaining.is_empty());
        }
    }

    #[tokio::test]
    async fn socks5_negotiation_rejects_other_versions_without_reply() {
        let (result, reply, remaining) = negotiate(&[0x04, 0x01, 0x00]).await;
        assert!(!result.unwrap());
        assert!(reply.is_empty());
        assert_eq!(remaining, [0x00]);
    }
}
