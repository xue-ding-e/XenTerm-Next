//! A length-capping wrapper for the SFTP channel stream.
//!
//! russh-sftp 2.1.1 reads every server packet with `read_packet`, which takes
//! the 4-byte length prefix at face value and allocates that many bytes before
//! validating anything (`vec![0; length as usize]`). A malicious SFTP server
//! therefore forces a ~4 GiB commit with a single `0xFFFFFFFF` length field —
//! repeat a few times and the process (or the pagefile) is exhausted.
//!
//! The crate reads nothing outside that length-prefixed framing (every read is
//! `read_u32` followed by a body `read_exact` of exactly that many bytes), so a
//! stream wrapper that validates the prefix before the body read may allocate
//! is a complete fix at our end (audit N-高1).
//!
//! OpenSSH's sftp-server buffers ~256 KiB per packet; 8 MiB leaves generous
//! headroom for exotic-but-legitimate servers while capping the attack at
//! 0.2% of its original size.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Largest SFTP packet body (length prefix excluded) accepted from a server.
pub(crate) const MAX_SFTP_PACKET: u32 = 8 * 1024 * 1024;

pub(crate) struct CappedSftpStream<S> {
    inner: S,
    header: [u8; 4],
    header_filled: usize,
    /// Bytes of body still expected for the current frame; 0 while the next
    /// thing to read is a fresh 4-byte length prefix.
    body_remaining: u32,
}

impl<S> CappedSftpStream<S> {
    pub(crate) fn new(inner: S) -> Self {
        Self {
            inner,
            header: [0; 4],
            header_filled: 0,
            body_remaining: 0,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for CappedSftpStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }


        if this.header_filled < 4 {
            // Length-prefix phase. Serve only the missing prefix bytes: the
            // stash keeps frame state without sub-borrowing the caller's buf,
            // and `put_slice` copies back exactly what the inner read gave.
            let want = (4 - this.header_filled).min(buf.remaining());
            let mut stash = [0u8; 4];
            let mut prefix = ReadBuf::new(&mut stash[..want]);
            match Pin::new(&mut this.inner).poll_read(cx, &mut prefix) {
                Poll::Ready(Ok(())) => {}
                other => return other,
            }
            let n = prefix.filled().len();
            if n == 0 {
                // EOF — at a frame boundary this is a clean end of stream and
                // russh-sftp's read_exact turns it into UnexpectedEof itself.
                return Poll::Ready(Ok(()));
            }
            this.header[this.header_filled..this.header_filled + n].copy_from_slice(&stash[..n]);
            buf.put_slice(&stash[..n]);
            this.header_filled += n;
            if this.header_filled < 4 {
                return Poll::Ready(Ok(()));
            }
            let length = u32::from_be_bytes(this.header);
            if length > MAX_SFTP_PACKET {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "SFTP packet length {length} exceeds the {MAX_SFTP_PACKET}-byte cap; \
                         the server is malformed or malicious"
                    ),
                )));
            }
            this.body_remaining = length;
            return Poll::Ready(Ok(()));
        }

        // Body phase: the frame length was validated, so the read passes
        // through. russh-sftp only reads bodies with `read_exact`, which never
        // asks for more than the unfilled remainder of the frame — enforce
        // that contract so no future reader can desync the framing silently.
        if buf.remaining() > this.body_remaining as usize {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SFTP body read exceeds the frame length (framing contract violation)",
            )));
        }
        let before = buf.remaining();
        match Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                let n = before - buf.remaining();
                this.body_remaining = this.body_remaining.saturating_sub(n as u32);
                // A frame finished; the next read starts a new length prefix.
                // Reset here — when the body is actually consumed — rather
                // than on entry, so an oversized read while a frame is open is
                // still refused by the contract guard above.
                if this.header_filled == 4 && this.body_remaining == 0 {
                    this.header_filled = 0;
                }
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for CappedSftpStream<S> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    /// A stream that hands out its bytes in fixed-size pieces, to exercise the
    /// prefix bookkeeping across partial reads.
    struct Chunky {
        data: Vec<u8>,
        pos: usize,
        chunk: usize,
    }

    impl AsyncRead for Chunky {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let this = self.get_mut();
            let n = this
                .chunk
                .min(this.data.len() - this.pos)
                .min(buf.remaining());
            if n == 0 {
                return Poll::Ready(Ok(()));
            }
            buf.put_slice(&this.data[this.pos..this.pos + n]);
            this.pos += n;
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for Chunky {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(0))
        }
        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    fn frame(body: &[u8]) -> Vec<u8> {
        let mut v = (body.len() as u32).to_be_bytes().to_vec();
        v.extend_from_slice(body);
        v
    }

    /// Read exactly the way russh-sftp's `read_packet` does: a 4-byte
    /// `read_u32` then a body `read_exact` of that many bytes.
    async fn read_frame<S: AsyncRead + Unpin>(stream: &mut S) -> io::Result<Vec<u8>> {
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let mut body = vec![0u8; u32::from_be_bytes(len_buf) as usize];
        stream.read_exact(&mut body).await?;
        Ok(body)
    }

    #[tokio::test]
    async fn passes_frames_through_in_small_pieces() {
        let mut stream = CappedSftpStream::new(Chunky {
            data: [frame(b"hello"), frame(b"world!")].concat(),
            pos: 0,
            chunk: 3,
        });
        assert_eq!(read_frame(&mut stream).await.unwrap(), b"hello");
        assert_eq!(read_frame(&mut stream).await.unwrap(), b"world!");
    }

    #[tokio::test]
    async fn rejects_an_oversized_length_without_allocating_the_body() {
        let mut data = u32::MAX.to_be_bytes().to_vec();
        data.extend_from_slice(&[0u8; 16]);
        let mut stream = CappedSftpStream::new(Chunky {
            data,
            pos: 0,
            chunk: 4,
        });
        // The cap trips while the length prefix itself is being read, before
        // any body-sized allocation exists.
        let mut len_buf = [0u8; 4];
        let err = stream.read_exact(&mut len_buf).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("exceeds"));
    }

    #[tokio::test]
    async fn eof_inside_a_prefix_is_reported() {
        let mut stream = CappedSftpStream::new(Chunky {
            data: vec![0, 0],
            pos: 0,
            chunk: 2,
        });
        let mut out = [0u8; 4];
        let err = stream.read_exact(&mut out).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn reading_past_the_frame_is_refused() {
        let mut stream = CappedSftpStream::new(Chunky {
            data: frame(b"abc"),
            pos: 0,
            chunk: 64,
        });
        // Consume only the length prefix: a 3-byte body is still pending.
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await.unwrap();
        // Asking for 64 bytes inside a 3-byte frame is a framing-contract
        // violation: it must fail loudly, not desync the stream.
        let mut out = [0u8; 64];
        let err = stream.read_exact(&mut out).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }
}
