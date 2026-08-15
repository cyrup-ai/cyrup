//! The broker's **listen** side — `net.createServer().listen(LISTEN_TARGET)`
//! (`pi-intercom` **v0.9.2** `broker/broker.ts:123,149-152`).
//!
//! Upstream has one polymorphic call. `net.Server.listen(path)` binds a Unix domain socket on POSIX
//! and a named pipe on Windows; `listen({host,port})` binds TCP. Which one it is comes from
//! `getBrokerListenTarget()` (`broker/paths.ts:107-116`), ported at
//! [`crate::transport::target::broker_listen_target`].
//!
//! Rust has no such polymorphism: `tokio::net::UnixListener` and
//! `tokio::net::windows::named_pipe::NamedPipeServer` are distinct types that do not even exist on
//! each other's platform. [`BrokerListener`] is that one `net.Server`, erased the same way
//! [`crate::transport::stream::BrokerStream`] erases the *connected* side (`net.Socket`) for the
//! client — the two are deliberately symmetrical, and both yield a `BrokerStream` so every layer
//! above (`reader_task` / `writer_task` / `BrokerState`) is transport-blind exactly as pi's is.
//!
//! ## Why this file exists at all
//!
//! Before it, `broker/mod.rs` imported `tokio::net::UnixListener` and `tokio::net::unix::{
//! OwnedReadHalf, OwnedWriteHalf}` at module scope under an ungated `pub mod broker;`. That made
//! the WHOLE crate fail to compile for `*-pc-windows-*` — and `cyrup-intercom` is a non-optional
//! dependency of the `cyrup` binary, so the binary could not be produced for Windows at all. The
//! two `#[cfg(windows)]` arms that already existed elsewhere in the crate
//! ([`crate::transport::stream`]'s named-pipe client with its `ERROR_PIPE_BUSY` retry, and
//! [`crate::transport::spawn`]'s `DETACHED_PROCESS | CREATE_NO_WINDOW` arm) had therefore never
//! been compiled by anything: they read as ported work while being unbuildable text.

use std::path::Path;

use crate::transport::stream::BrokerStream;
use crate::transport::target::BrokerConnectTarget;

/// The bound broker endpoint — pi's single `net.Server` (`broker.ts:123`).
#[derive(Debug)]
pub enum BrokerListener {
    /// A bound Unix domain socket (`listen("<intercomDir>/broker.sock")`).
    #[cfg(unix)]
    Unix(tokio::net::UnixListener),
    /// A bound Windows named pipe (`listen("\\\\.\\pipe\\cyrup-intercom-…")`).
    ///
    /// Win32 named pipes have no listener/connection split: a *server instance* IS the connection
    /// once a client attaches. Serving more than one client therefore means holding one idle
    /// instance to hand out and creating its replacement the moment it is claimed — the accept loop
    /// Node's `net.Server` runs internally. The path is kept so replacements can be created.
    #[cfg(windows)]
    Pipe {
        /// The pipe name, re-used to create each replacement instance.
        path: std::path::PathBuf,
        /// The idle instance currently waiting for a client.
        pending: tokio::net::windows::named_pipe::NamedPipeServer,
    },
}

impl BrokerListener {
    /// Bind `target` (`this.server.listen(LISTEN_TARGET, onListening)`, `broker.ts:149-152`).
    ///
    /// # Errors
    /// Propagates the bind/create failure. The Unix arm's message names the path AND its byte
    /// length (see the CYRUP-DELTA at the call site in [`crate::broker::run`]); the opt-in
    /// loopback-TCP listen target is rejected here, see [`Self::bind`]'s `Tcp` arm.
    pub async fn bind(target: &BrokerConnectTarget) -> std::io::Result<Self> {
        match target {
            BrokerConnectTarget::Socket(path) => Self::bind_socket(path),
            // `getBrokerListenTarget` only ever returns the TCP arm on Windows with
            // `CYRUP_INTERCOM_TRANSPORT=tcp` / `CYRUP_INTERCOM_TCP=1` explicitly set
            // (`paths.ts:107-116` via `shouldUseWindowsTcpTransport`, `:44-59`). The BROKER half of
            // that opt-in — `BROKER_STATE_ID`, the `broker.port.json` endpoint file
            // (`broker.ts:134-141`) and the `requiresEndpointAuth` credential gate on
            // `health`/`register` (`broker.ts:284-305`) — is the one piece of pi-intercom this port
            // has never carried; it is recorded as deferred in this crate's module docs
            // (`broker/mod.rs`, `paths.rs`) and in the port doc §10-Q2.
            //
            // It is refused LOUDLY rather than silently downgraded to the pipe: falling back would
            // bind an endpoint no client is looking for (`broker_connect_target` reads
            // `broker.port.json` under the same env), so every session would spawn a fresh broker
            // and time out, which reads as an intercom outage rather than an unimplemented opt-in.
            BrokerConnectTarget::Tcp(_) => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "the opt-in loopback-TCP intercom transport (CYRUP_INTERCOM_TRANSPORT=tcp / \
                 CYRUP_INTERCOM_TCP=1) is not implemented by this broker; unset it to use the \
                 default named-pipe transport",
            )),
        }
    }

    /// The `string` listen target: a Unix domain socket on POSIX, a named pipe on Windows —
    /// upstream's one `listen(path)` call (`broker.ts:150`).
    #[cfg(unix)]
    fn bind_socket(path: &Path) -> std::io::Result<Self> {
        tokio::net::UnixListener::bind(path).map(Self::Unix)
    }

    /// The Windows half of upstream's `listen(path)`.
    ///
    /// `first_pipe_instance(true)` is what makes this a *claim*: if another process already owns
    /// this pipe name the create fails with `ERROR_ACCESS_DENIED` instead of silently adding a
    /// second server to someone else's pipe and stealing half its clients. That is the named-pipe
    /// equivalent of the Unix side's `assert_no_live_broker` +
    /// bind-after-unlink, and it is the reason a Windows broker does NOT unlink anything first:
    /// there is no filesystem entry to remove, and the OS reclaims the name when the last instance
    /// closes.
    #[cfg(windows)]
    fn bind_socket(path: &Path) -> std::io::Result<Self> {
        let pending = tokio::net::windows::named_pipe::ServerOptions::new()
            .first_pipe_instance(true)
            .create(path)?;
        Ok(Self::Pipe { path: path.to_path_buf(), pending })
    }

    /// Await the next client (`net.Server`'s internal accept loop feeding
    /// `this.handleConnection`, `broker.ts:123`).
    ///
    /// # Errors
    /// Propagates the accept/connect failure. `broker::run` logs and keeps looping, matching
    /// upstream's server-level `error` handling: one bad accept never ends the broker.
    pub async fn accept(&mut self) -> std::io::Result<BrokerStream> {
        match self {
            #[cfg(unix)]
            Self::Unix(listener) => {
                let (stream, _addr) = listener.accept().await?;
                Ok(BrokerStream::new(stream))
            }
            #[cfg(windows)]
            Self::Pipe { path, pending } => {
                // `connect()` resolves when a client attaches to THIS instance; the instance then
                // *is* that connection, so a fresh one has to take its place before it is handed
                // out or the pipe would serve exactly one client for the broker's lifetime.
                pending.connect().await?;
                let next = tokio::net::windows::named_pipe::ServerOptions::new().create(&*path)?;
                let connected = std::mem::replace(pending, next);
                Ok(BrokerStream::new(connected))
            }
        }
    }

    /// Whether a session registering over this endpoint is `trustedLocal` — upstream's
    /// `typeof LISTEN_TARGET === "string" && process.platform !== "win32"` (`broker.ts:365`).
    /// True only on a real Unix domain socket: a named pipe carries no peer credential the broker
    /// can read, and neither does TCP.
    #[must_use]
    pub const fn is_trusted_local(&self) -> bool {
        match self {
            #[cfg(unix)]
            Self::Unix(_) => true,
            #[cfg(windows)]
            Self::Pipe { .. } => false,
        }
    }

}

/// Remove a stale endpoint left by a crashed broker
/// (`try { unlinkSync(LISTEN_TARGET) } catch {}`, **v0.9.2** `broker/broker.ts:116-120`, repeated at
/// shutdown `:1416-1418`).
///
/// Upstream guards BOTH sites with `typeof LISTEN_TARGET === "string" && process.platform !==
/// "win32"`: a Windows named pipe has no filesystem entry to unlink, and a TCP endpoint has no path
/// at all. cyrup previously unlinked unconditionally, which was harmless only because the crate
/// could not be built for Windows.
pub fn unlink_stale_endpoint(target: &BrokerConnectTarget) {
    if cfg!(windows) {
        return;
    }
    if let BrokerConnectTarget::Socket(path) = target {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
    use super::*;
    use crate::transport::target::{BrokerTcpEndpoint, INTERCOM_TCP_HOST};

    /// The listen side accepts a real client and hands back a duplex stream, on whichever transport
    /// this platform's `getBrokerListenTarget` selects. This is the proof that the accept loop and
    /// the connect side (`BrokerStream::connect`) actually meet — on Windows that pairs the
    /// `NamedPipeServer` arm added here with the `ERROR_PIPE_BUSY` client arm at
    /// `transport/stream.rs`, which until now had never been compiled, let alone run.
    #[tokio::test]
    async fn a_bound_listener_accepts_a_client_and_round_trips_bytes() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        let target = BrokerConnectTarget::Socket(dir.path().join("broker.sock"));
        #[cfg(windows)]
        let target = BrokerConnectTarget::Socket(std::path::PathBuf::from(format!(
            r"\\.\pipe\cyrup-intercom-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        )));

        let mut listener = BrokerListener::bind(&target).await.expect("binds the listen target");
        let connect_target = target.clone();
        let client = tokio::spawn(async move {
            let mut stream = BrokerStream::connect(&connect_target).await.expect("connects");
            stream.write_all(b"ping").await.expect("write");
            let mut buf = [0u8; 4];
            stream.read_exact(&mut buf).await.expect("read");
            buf
        });

        let mut accepted = listener.accept().await.expect("accepts");
        let mut buf = [0u8; 4];
        accepted.read_exact(&mut buf).await.expect("server read");
        assert_eq!(&buf, b"ping");
        accepted.write_all(b"pong").await.expect("server write");

        assert_eq!(&client.await.unwrap(), b"pong");
        drop(dir);
    }

    /// `trustedLocal` is `typeof LISTEN_TARGET === "string" && platform !== "win32"`
    /// (`broker.ts:365`) — the peer-credential claim the broker stamps on every registered session.
    /// Asserted per arm, because the two arms must disagree: a Unix socket carries a peer uid, a
    /// named pipe does not.
    #[tokio::test]
    async fn trusted_local_is_the_unix_socket_arm_only() {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        let target = BrokerConnectTarget::Socket(dir.path().join("broker.sock"));
        #[cfg(windows)]
        let target = BrokerConnectTarget::Socket(std::path::PathBuf::from(format!(
            r"\\.\pipe\cyrup-intercom-trust-{}",
            std::process::id()
        )));
        let listener = BrokerListener::bind(&target).await.expect("binds");
        assert_eq!(listener.is_trusted_local(), cfg!(unix));
        drop(dir);
    }

    /// The opt-in loopback-TCP listen target is refused with a message that names the env vars, on
    /// EVERY platform — never silently downgraded to the socket/pipe endpoint, which would bind an
    /// endpoint no client under that env is looking for.
    #[tokio::test]
    async fn the_opt_in_tcp_listen_target_is_refused_rather_than_silently_downgraded() {
        let err = BrokerListener::bind(&BrokerConnectTarget::Tcp(BrokerTcpEndpoint {
            host: INTERCOM_TCP_HOST.to_string(),
            port: 0,
            state_id: None,
        }))
        .await
        .expect_err("the TCP broker half is not implemented");
        assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
        assert!(err.to_string().contains("CYRUP_INTERCOM_TRANSPORT=tcp"), "{err}");
    }

    /// `unlinkSync(LISTEN_TARGET)` is guarded by `platform !== "win32"` at BOTH upstream sites
    /// (`broker.ts:116-120,1416-1418`). Asserted on both arms: on Unix a stale socket file is
    /// removed so a crashed broker's leftovers stay reclaimable; on Windows there is no filesystem
    /// entry and the call must be inert.
    #[test]
    fn stale_endpoint_removal_follows_upstreams_platform_guard() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broker.sock");
        std::fs::write(&path, b"stale").unwrap();

        unlink_stale_endpoint(&BrokerConnectTarget::Socket(path.clone()));
        assert_eq!(path.exists(), cfg!(windows), "unlink is the non-Windows arm only");

        // A TCP listen target has no path; the call must never touch the filesystem for it.
        unlink_stale_endpoint(&BrokerConnectTarget::Tcp(BrokerTcpEndpoint {
            host: INTERCOM_TCP_HOST.to_string(),
            port: 0,
            state_id: None,
        }));
    }
}
