//! The wire: one connection, one request line, one response line.
//!
//! ## One request per connection is not a choice
//!
//! herdr's `handle_connection_with_stop` (`tmp/herdr/src/api/server.rs:156-317`) reads **one** line
//! via `read_initial_request_line` (`:168`, `:528-537`), dispatches it, writes one line (`:301`),
//! and returns. There is no read loop. Every independent client agrees, which is how you know it is
//! the contract and not an accident:
//!
//! - herdr's own Rust client calls `self.connect()` per request (`src/api/client.rs:55-61`).
//! - the Python client opens `AF_UNIX`, `sendall`, one `recv`, close (`client.py:403-408`).
//! - pi's `SocketRpcClient.call` does `net.createConnection` per call
//!   (`src/runs/shared/herdr-connection.ts:77` @v0.68.0).
//!
//! The consequence that matters downstream: the `socket-api.mdx:118-130` rule that
//! `session.snapshot` must run on a *different* connection from `events.subscribe` is structural,
//! not advice.
//!
//! ## The bounds, and which protocol each belongs to
//!
//! herdr has **two** local protocols and they have different limits. The JSON socket API bounds a
//! request line at **1 MiB** — `MAX_INITIAL_REQUEST_BYTES` (`tmp/herdr/src/api/server.rs:32`) —
//! with a 5 s read deadline (`:30`, `INITIAL_REQUEST_TIMEOUT`) and a 5 s write deadline (`:31`,
//! `STREAM_WRITE_TIMEOUT`). The 32 MiB figure that circulates is `MAX_GRAPHICS_FRAME_SIZE`
//! (`tmp/herdr/src/protocol/wire.rs:29`) on herdr's separate **binary** client-shell protocol,
//! whose own module doc (`wire.rs:1-8`) says it covers "direct-terminal and internal operations".
//! `socket-api.mdx:195-196` mentions 32 MiB about pane graphics, not about this.
//!
//! herdr sets no bound on its *response* line, so [`MAX_RESPONSE_BYTES`] is this client's own.

use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, ReadBuf};

use crate::error::{ApiError, ApiErrorCode, HerdrError, Result, Unavailable};
use crate::schema::{Request, ResponseResult, WireResponse};

/// `MAX_INITIAL_REQUEST_BYTES` (`tmp/herdr/src/api/server.rs:32`) — 1 MiB.
///
/// Checked here before a byte is written, so an over-long request fails with a
/// [`HerdrError::TooLarge`] naming the bound instead of herdr closing the connection with
/// `"api request line is too large"` (`server.rs:563-567`) and this client reporting a bare EOF.
///
/// **The bound is the JSON line, not the line plus its terminator** — herdr's is the same, and
/// the parity is exact rather than approximate. `read_initial_request_line_with_limits`
/// accumulates the request byte by byte and tests `bytes.len() > max_bytes` **only after pushing
/// a non-newline byte** (`server.rs:556-568`); the `\n` branch above it breaks out before any
/// length check. So herdr accepts a JSON line of exactly `MAX_REQUEST_BYTES` and refuses at
/// `MAX_REQUEST_BYTES + 1`, and [`write_line`] refuses at exactly the same place. Counting the
/// newline here would make this client one byte stricter than the server and silently break a
/// `pane.send_input` herdr would have executed.
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;

/// This client's response-line bound — 4 MiB.
///
/// herdr sets **none**: `read_initial_request_line_with_limits` bounds what the server reads, and
/// nothing bounds what it writes. An unbounded read on a socket is an unbounded allocation, so the
/// bound is pi's `MAX_RPC_BYTES` (`src/runs/shared/herdr-connection.ts:9` @v0.68.0), chosen there
/// for the same reason and sized for the one genuinely large reply, `session.snapshot` over a big
/// session.
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// `STREAM_WRITE_TIMEOUT` (`tmp/herdr/src/api/server.rs:31`) — herdr's own write deadline, applied
/// to this client's write so neither side can block the other indefinitely.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// The default deadline for a whole call: connect, write, and read one line.
///
/// 15 s, matching pi (`src/runs/shared/herdr-connection.ts:78` @v0.68.0), and deliberately longer
/// than herdr's own 5 s `APP_RESPONSE_TIMEOUT` (`tmp/herdr/src/api/server.rs:29`) so a herdr that
/// is merely slow answers rather than being cut off — herdr will report its own `timeout` code.
///
/// It exists because **there is no herdr-side deadline on a plain dispatch**:
/// `tmp/herdr/src/api/server.rs:911-913` is a `recv()` with `None` timeout. Without this deadline
/// a busy UI is a hang.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// Every duplex byte stream this transport can carry.
trait LocalIo: AsyncRead + AsyncWrite + Send + Unpin + 'static {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> LocalIo for T {}

/// A connected herdr API socket — a `UnixStream` on POSIX, a named pipe on Windows.
///
/// The box is forced by the language, not chosen: `tokio::net::UnixStream` and
/// `tokio::net::windows::named_pipe::NamedPipeClient` have no common concrete supertype, where
/// herdr's `interprocess::local_socket::Stream` and Node's `net.Socket` are each one type.
/// `crates/cyrup-intercom/src/transport/stream.rs:36-40` reached the same shape for the same
/// reason; this crate copies the shape rather than the crate, because depending on `cyrup-intercom`
/// from here would invert the layering (intercom is a consumer of this crate, not a provider to it).
pub struct LocalStream(Box<dyn LocalIo>);

impl std::fmt::Debug for LocalStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LocalStream")
    }
}

impl LocalStream {
    /// Wrap an already-connected duplex stream.
    pub fn new<T: AsyncRead + AsyncWrite + Send + Unpin + 'static>(io: T) -> Self {
        Self(Box::new(io))
    }

    /// Connect to herdr's API socket at `path`.
    ///
    /// # Errors
    /// [`Unavailable::NoSocket`] — carrying the path and the underlying `io::Error` — when nothing
    /// is listening, the socket file is stale, or the pipe does not exist. A connect failure is
    /// never flattened into a bare `io::Error`, because the path is the actionable half.
    pub async fn connect(path: &Path) -> Result<Self> {
        connect_local(path).await.map_err(|source| {
            tracing::debug!(path = %path.display(), %source, "herdr api socket is not connectable");
            HerdrError::Unavailable(Unavailable::NoSocket {
                path: path.to_path_buf(),
                source,
            })
        })
    }
}

#[cfg(unix)]
async fn connect_local(path: &Path) -> std::io::Result<LocalStream> {
    Ok(LocalStream::new(
        tokio::net::UnixStream::connect(path).await?,
    ))
}

/// The Windows arm.
///
/// herdr names its Windows pipe with `GenericNamespaced` over the **stringified socket path**
/// (`tmp/herdr/src/ipc.rs:44-51`), and `interprocess` maps a namespaced name to
/// `\\.\pipe\<that whole path>`. So the pipe this client must open is the literal prefix plus the
/// same file-style path herdr would have used on POSIX — confirmed independently by the Python
/// client, which spells it out (`client.py:410-419`,
/// `pipe_name = "\\\\.\\pipe\\" + str(self._socket_path)`). The `.sock` file itself is only a
/// marker on Windows (`tmp/herdr/src/ipc.rs:73`, `fs::write(path, windows_socket_marker())`).
///
/// `crates/cyrup-intercom/src/transport/stream.rs:109-130` does **not** prepend the prefix, because
/// its own paths are already pipe-form. Copying it verbatim would be the bug.
///
/// The `ERROR_PIPE_BUSY` (231) retry is copied from that same function: Node queues a connect
/// against a pipe whose instances are all busy, while the Win32 `CreateFile` underneath
/// `ClientOptions::open` fails immediately, so the wait has to be written out.
#[cfg(windows)]
async fn connect_local(path: &Path) -> std::io::Result<LocalStream> {
    use tokio::net::windows::named_pipe::ClientOptions;

    const ERROR_PIPE_BUSY: i32 = 231;
    const BUSY_RETRY_INTERVAL: Duration = Duration::from_millis(50);
    const BUSY_RETRY_TOTAL: Duration = Duration::from_secs(10);

    let name = format!(r"\\.\pipe\{}", path.to_string_lossy());
    let deadline = tokio::time::Instant::now() + BUSY_RETRY_TOTAL;
    loop {
        match ClientOptions::new().open(&name) {
            Ok(client) => return Ok(LocalStream::new(client)),
            Err(err)
                if err.raw_os_error() == Some(ERROR_PIPE_BUSY)
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(BUSY_RETRY_INTERVAL).await;
            }
            Err(err) => return Err(err),
        }
    }
}

impl AsyncRead for LocalStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for LocalStream {
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

/// Write one `\n`-terminated JSON line, bounded and deadlined.
///
/// Framing: one JSON value per `\n`-terminated line, both directions
/// (`tmp/herdr/src/api/client.rs:185-190` writes it, `tmp/herdr/src/api/server.rs:539-583` reads
/// it).
///
/// # Errors
/// [`HerdrError::TooLarge`] when the line would exceed [`MAX_REQUEST_BYTES`];
/// [`HerdrError::Timeout`] when the write does not complete inside [`WRITE_TIMEOUT`];
/// [`HerdrError::Io`] otherwise.
pub async fn write_line<W: AsyncWrite + Unpin>(
    writer: &mut W,
    method: &'static str,
    line: &str,
) -> Result<()> {
    // `>` and not `>=`, and the newline is NOT counted: herdr's own check is
    // `bytes.len() > max_bytes` over the bytes before the terminator (`server.rs:556-568`). See
    // [`MAX_REQUEST_BYTES`].
    if line.len() > MAX_REQUEST_BYTES {
        return Err(HerdrError::TooLarge {
            method,
            limit: MAX_REQUEST_BYTES,
        });
    }
    let write = async {
        writer.write_all(line.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
        Ok::<(), std::io::Error>(())
    };
    match tokio::time::timeout(WRITE_TIMEOUT, write).await {
        Ok(result) => result.map_err(HerdrError::Io),
        Err(_elapsed) => Err(HerdrError::Timeout {
            method,
            timeout: WRITE_TIMEOUT,
        }),
    }
}

/// Read one `\n`-terminated line, refusing anything over [`MAX_RESPONSE_BYTES`].
///
/// The bound is enforced **while** reading rather than after, so a server that streams megabytes
/// with no newline cannot make this client allocate without limit.
///
/// # Errors
/// [`HerdrError::Closed`] on EOF before any newline; [`HerdrError::TooLarge`] past the bound;
/// [`HerdrError::Io`] otherwise.
pub async fn read_line<R: AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
    method: &'static str,
) -> Result<String> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Err(HerdrError::Closed { method });
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let taken = newline.map_or(available.len(), |index| index + 1);
        line.extend_from_slice(available.get(..taken).unwrap_or_default());
        reader.consume(taken);
        if line.len() > MAX_RESPONSE_BYTES {
            return Err(HerdrError::TooLarge {
                method,
                limit: MAX_RESPONSE_BYTES,
            });
        }
        if newline.is_some() {
            break;
        }
    }
    String::from_utf8(line)
        .map_err(|err| HerdrError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, err)))
}

/// One request, one connection, one answer — the whole round trip, under one deadline.
///
/// # Errors
/// Every arm of [`HerdrError`]. The two worth stating explicitly:
///
/// - **A success whose `id` is not the id that was sent is refused** with
///   [`HerdrError::IdMismatch`] and the connection is dropped. One request per connection means a
///   mismatched id cannot be a pipelining artefact, so the payload describes something other than
///   what was asked — accepting it would hand a caller another pane's data as its own. pi drops it
///   too (`herdr-connection.ts:86`).
/// - **An error envelope is fatal whatever its `id`**, including the empty one. herdr writes
///   `{"id":"","error":{"code":"invalid_request",…}}` when the line did not deserialise and it
///   could not recover a correlation id (`tmp/herdr/src/api/server.rs:180-201`). Requiring a
///   matching id before treating an error as fatal would make that shape invisible, and the call
///   would sit until its deadline instead of reporting what herdr actually said.
pub async fn request(
    socket: &Path,
    request: &Request,
    timeout: Duration,
) -> Result<ResponseResult> {
    let method = request.method.name();
    let line = serde_json::to_string(request).map_err(|err| HerdrError::Io(err.into()))?;

    let exchange = async {
        let stream = LocalStream::connect(socket).await?;
        let mut reader = BufReader::new(stream);
        write_line(&mut reader, method, &line).await?;
        read_line(&mut reader, method).await
    };

    let answer = match tokio::time::timeout(timeout, exchange).await {
        Ok(result) => result?,
        Err(_elapsed) => return Err(HerdrError::Timeout { method, timeout }),
    };

    match WireResponse::decode(answer.trim_end_matches(['\r', '\n'])) {
        Ok(WireResponse::Success(success)) => {
            if success.id == request.id {
                Ok(success.result)
            } else {
                Err(HerdrError::IdMismatch {
                    method,
                    sent: request.id.clone(),
                    got: success.id,
                })
            }
        }
        Ok(WireResponse::Error(failure)) => Err(HerdrError::Api {
            method,
            source: ApiError {
                code: ApiErrorCode::from_wire(&failure.error.code),
                message: failure.error.message,
            },
        }),
        Err(source) => Err(HerdrError::Malformed { method, source }),
    }
}
