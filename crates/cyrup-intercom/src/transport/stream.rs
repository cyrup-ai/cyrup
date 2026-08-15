//! The concrete broker connection for each [`BrokerConnectTarget`] arm — a port of pi-intercom's
//! `connectToBrokerTarget` (`broker/client.ts:26-30` and the byte-identical copy at
//! `broker/spawn.ts:261-265`), v0.7.0.
//!
//! ```text
//! function connectToBrokerTarget(target: BrokerConnectTarget): net.Socket {
//!   return typeof target === "string"
//!     ? net.connect(target)                            // unix socket OR windows named pipe
//!     : net.connect({ host: target.host, port: target.port });   // opt-in loopback TCP
//! }
//! ```
//!
//! Node's `net.connect` is polymorphic over all three: given a path it opens a Unix domain socket on
//! POSIX and a named pipe on Windows, and given `{host,port}` it opens TCP. Rust has three distinct
//! types (`tokio::net::UnixStream`, `tokio::net::TcpStream`,
//! `tokio::net::windows::named_pipe::NamedPipeClient`) with no common concrete supertype, so this
//! module erases them behind [`BrokerStream`] — a boxed `AsyncRead + AsyncWrite`. **Mechanism
//! divergence forced by the language**: pi has one `net.Socket` class; Rust needs the box.
//! Behaviour — which target maps to which kind of connection — is unchanged.
//!
//! Splitting likewise diverges: `UnixStream`/`TcpStream` have inherent `into_split()` but
//! `NamedPipeClient` does not, so all three are split with [`tokio::io::split`], whose halves
//! re-acquire their shared lock per `poll` (a pending read never holds it across an `await`, so a
//! blocked read cannot starve a write — the property pi gets for free from Node's single duplex
//! `net.Socket`).

use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::error::Result;
use crate::transport::target::BrokerConnectTarget;

/// Every duplex byte stream this transport can carry.
trait BrokerIo: AsyncRead + AsyncWrite + Send + Unpin + 'static {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> BrokerIo for T {}

/// A connected broker socket, whichever transport carries it (`net.Socket`, `client.ts:26-30`).
pub struct BrokerStream(Box<dyn BrokerIo>);

/// The read half of a [`BrokerStream`].
pub type BrokerReadHalf = tokio::io::ReadHalf<BrokerStream>;
/// The write half of a [`BrokerStream`].
pub type BrokerWriteHalf = tokio::io::WriteHalf<BrokerStream>;

impl std::fmt::Debug for BrokerStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BrokerStream")
    }
}

impl BrokerStream {
    /// Wrap an already-connected duplex stream (used by the in-crate tests, which drive a
    /// `UnixStream::pair()` / a loopback `TcpStream` directly).
    pub fn new<T: AsyncRead + AsyncWrite + Send + Unpin + 'static>(io: T) -> Self {
        Self(Box::new(io))
    }

    /// `connectToBrokerTarget` (`client.ts:26-30`, `spawn.ts:261-265`).
    ///
    /// # Errors
    /// [`crate::error::IntercomError::Io`] if the socket / pipe / TCP endpoint cannot be connected.
    pub async fn connect(target: &BrokerConnectTarget) -> Result<Self> {
        match target {
            BrokerConnectTarget::Socket(path) => connect_socket(path).await,
            BrokerConnectTarget::Tcp(endpoint) => {
                // `net.connect({ host, port })`. `TcpStream::connect` takes `A: ToSocketAddrs`; the
                // host is always the literal loopback `INTERCOM_TCP_HOST` (`paths.ts:91` rejects
                // anything else), so this never performs a DNS lookup.
                let stream =
                    tokio::net::TcpStream::connect((endpoint.host.as_str(), endpoint.port)).await?;
                // Node sets `noDelay` on `net.connect` sockets by default since v18; the intercom
                // wire is small request/response frames where Nagle would add up to 40 ms of
                // latency to every `send` ack, so match Node here explicitly.
                let _ = stream.set_nodelay(true);
                Ok(Self::new(stream))
            }
        }
    }

    /// Split into independently-owned halves (pi passes the one `net.Socket` to both the reader and
    /// the writer).
    #[must_use]
    pub fn into_split(self) -> (BrokerReadHalf, BrokerWriteHalf) {
        tokio::io::split(self)
    }
}

/// The `string` arm of `BrokerConnectTarget`: a Unix domain socket on POSIX, a named pipe on
/// Windows (`paths.ts:65-74` picks which name; `net.connect(path)` picks which kind).
#[cfg(unix)]
async fn connect_socket(path: &std::path::Path) -> Result<BrokerStream> {
    let stream = tokio::net::UnixStream::connect(path).await?;
    Ok(BrokerStream::new(stream))
}

/// The Windows named-pipe client (`net.connect(\\.\pipe\pi-intercom-...)`).
///
/// Node queues a connect against a pipe whose server instances are all busy and completes it once
/// one frees up; the Win32 `CreateFile` this ultimately calls instead fails immediately with
/// `ERROR_PIPE_BUSY` (231), so the wait has to be written out. `NamedPipeClient` exposes no
/// `WaitNamedPipe`, so this polls at 50 ms for up to the same 10 s the client's own registration
/// timeout allows (`client.ts:182`) — a connect that would have hung in Node hangs the same length
/// of time here, and one that would have succeeded still does.
///
/// The 231 literal is `ERROR_PIPE_BUSY`; it is spelled numerically to avoid adding a
/// `windows-sys`/`winapi` dependency for a single constant.
#[cfg(windows)]
async fn connect_socket(path: &std::path::Path) -> Result<BrokerStream> {
    use tokio::net::windows::named_pipe::ClientOptions;

    const ERROR_PIPE_BUSY: i32 = 231;
    const BUSY_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
    const BUSY_RETRY_TOTAL: std::time::Duration = std::time::Duration::from_secs(10);

    let deadline = tokio::time::Instant::now() + BUSY_RETRY_TOTAL;
    loop {
        match ClientOptions::new().open(path) {
            Ok(client) => return Ok(BrokerStream::new(client)),
            Err(e)
                if e.raw_os_error() == Some(ERROR_PIPE_BUSY)
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(BUSY_RETRY_INTERVAL).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

impl AsyncRead for BrokerStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for BrokerStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
    use super::*;
    use crate::transport::target::{BrokerTcpEndpoint, INTERCOM_TCP_HOST};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// The TCP arm genuinely opens a loopback TCP connection (`client.ts:28`
    /// `net.connect({ host, port })`), over a real `TcpListener` bound to `127.0.0.1:0` — no
    /// network, no fixture socket file.
    #[tokio::test]
    async fn connect_opens_a_real_loopback_tcp_connection_for_the_tcp_target() {
        let listener = tokio::net::TcpListener::bind((INTERCOM_TCP_HOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut stream, peer) = listener.accept().await.unwrap();
            stream.write_all(b"pong").await.unwrap();
            peer.ip().to_string()
        });

        let target = BrokerConnectTarget::Tcp(BrokerTcpEndpoint {
            host: INTERCOM_TCP_HOST.to_string(),
            port,
            state_id: Some("state-1".to_string()),
        });
        let mut stream = BrokerStream::connect(&target).await.expect("connects over TCP");
        let mut buf = [0u8; 4];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"pong");
        assert_eq!(server.await.unwrap(), INTERCOM_TCP_HOST, "loopback only");
    }

    // Unix-domain-socket specific (`UnixStream::pair()` / `UnixListener`): the transport-neutral
    // behaviour it asserts is covered on Windows by the named-pipe arm of `broker::listener`.
    #[cfg(unix)]
    /// MIRROR (stays green): the `Socket` arm still opens a Unix domain socket, unchanged.
    #[tokio::test]
    async fn connect_opens_a_unix_socket_for_the_socket_target() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broker.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            stream.write_all(b"pong").await.unwrap();
        });

        let target = BrokerConnectTarget::Socket(path);
        let mut stream = BrokerStream::connect(&target).await.expect("connects over the socket");
        let mut buf = [0u8; 4];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"pong");
    }

    /// A TCP target pointing at a closed port surfaces the connect failure as
    /// [`crate::error::IntercomError::Io`] rather than hanging (`client.ts:171-176` rejects).
    #[tokio::test]
    async fn connect_surfaces_a_refused_tcp_endpoint_as_an_error() {
        // Bind then drop, so the port is unbound but was real. Dropping RETURNS that port to the
        // ephemeral pool, and five other tests in this same binary bind `127.0.0.1:0`
        // concurrently (`stream.rs:172`, `spawn.rs:428,471`, `client.rs:752`, and the sibling
        // above), so one of them can legitimately claim it between the drop and the connect — at
        // which point the connect SUCCEEDS and this test fails while the code under test is
        // correct. That is what it did in a loaded workspace run. Re-draw a port instead of
        // asserting the OS lost the race: the claim ("a closed TCP port surfaces as an error, it
        // does not hang") is unchanged, and only a port that is refused on none of the attempts
        // can fail it.
        let mut last: Option<Result<BrokerStream>> = None;
        for _ in 0..16 {
            let listener = tokio::net::TcpListener::bind((INTERCOM_TCP_HOST, 0)).await.unwrap();
            let port = listener.local_addr().unwrap().port();
            drop(listener);

            let target = BrokerConnectTarget::Tcp(BrokerTcpEndpoint {
                host: INTERCOM_TCP_HOST.to_string(),
                port,
                state_id: Some("s".to_string()),
            });
            let outcome = BrokerStream::connect(&target).await;
            if outcome.is_err() {
                return;
            }
            last = Some(outcome);
        }
        panic!("a just-released ephemeral port was re-bound on all 16 attempts: {:?}", last.map(|r| r.is_ok()));
    }

    // Unix-domain-socket specific (`UnixStream::pair()` / `UnixListener`): the transport-neutral
    // behaviour it asserts is covered on Windows by the named-pipe arm of `broker::listener`.
    #[cfg(unix)]
    /// The split halves are usable concurrently: a read pending on one half must not block a write
    /// on the other (the property Node's single duplex `net.Socket` gives pi for free).
    #[tokio::test]
    async fn split_halves_do_not_block_each_other() {
        let (a, mut b) = tokio::net::UnixStream::pair().unwrap();
        let (mut read_half, mut write_half) = BrokerStream::new(a).into_split();

        // Park a read that cannot complete until the peer writes.
        let reader = tokio::spawn(async move {
            let mut buf = [0u8; 2];
            read_half.read_exact(&mut buf).await.unwrap();
            buf
        });
        // With the read parked, this write must still land.
        write_half.write_all(b"hi").await.expect("write is not starved by the parked read");
        let mut peer_buf = [0u8; 2];
        b.read_exact(&mut peer_buf).await.unwrap();
        assert_eq!(&peer_buf, b"hi");

        b.write_all(b"ok").await.unwrap();
        assert_eq!(&reader.await.unwrap(), b"ok");
    }
}
