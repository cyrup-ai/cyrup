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
    /// A bound loopback-TCP endpoint (`listen({ host, port })`, `broker.ts:149-151`), the opt-in
    /// Windows transport (`PI_INTERCOM_TRANSPORT=tcp` / `PI_INTERCOM_TCP=1`, `paths.ts:44-59`).
    ///
    /// Compiled on EVERY platform, exactly as upstream's branch is: `getBrokerListenTarget` only
    /// selects it on Windows, but the arm is neither `#[cfg]`-gated nor unreachable — a unix host
    /// binds and accepts it identically, which is what makes it testable here at all (the crate
    /// `#![forbid(unsafe_code)]`s, so no test can flip `process.platform`).
    Tcp(tokio::net::TcpListener),
}

impl BrokerListener {
    /// Bind `target` (`this.server.listen(LISTEN_TARGET, onListening)`, `broker.ts:149-152`).
    ///
    /// # Errors
    /// Propagates the bind/create failure. The Unix arm's message names the path AND its byte
    /// length (see the CYRUP-DELTA at the call site in [`crate::broker::run`]); the loopback-TCP arm
    /// binds port `0` and the CHOSEN port is read back with [`Self::local_addr`].
    pub async fn bind(target: &BrokerConnectTarget) -> std::io::Result<Self> {
        match target {
            BrokerConnectTarget::Socket(path) => Self::bind_socket(path),
            // `this.server.listen({ host: LISTEN_TARGET.host, port: LISTEN_TARGET.port })`
            // (`broker.ts:150-151`). The listen target's port is `0` (`paths.ts:112`), i.e.
            // bind-any: the kernel picks, and the CHOSEN port is what `broker.port.json` publishes
            // — read back with [`Self::local_addr`] at the call site (`broker.ts:131-141`), never
            // from this target.
            BrokerConnectTarget::Tcp(endpoint) => {
                tokio::net::TcpListener::bind((endpoint.host.as_str(), endpoint.port))
                    .await
                    .map(Self::Tcp)
            }
        }
    }

    /// The endpoint actually bound, for the TCP arm only (`this.server.address()`,
    /// `broker.ts:132`). `None` on the socket/pipe arms, which have no address —
    /// upstream's `if (typeof LISTEN_TARGET === "string")` split at `:128`.
    ///
    /// # Errors
    /// Propagates the `getsockname` failure, which is upstream's
    /// `throw new Error("Intercom TCP broker started without a TCP address")` (`:134`).
    pub fn local_addr(&self) -> std::io::Result<Option<std::net::SocketAddr>> {
        match self {
            #[cfg(unix)]
            Self::Unix(_) => Ok(None),
            #[cfg(windows)]
            Self::Pipe { .. } => Ok(None),
            Self::Tcp(listener) => listener.local_addr().map(Some),
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
            Self::Tcp(listener) => {
                let (stream, _addr) = listener.accept().await?;
                // The client side sets `noDelay` for the same reason
                // (`transport::stream::BrokerStream::connect`): Node has had it on by default since
                // v18, and the intercom wire is small request/response frames where Nagle would add
                // up to 40 ms to every ack.
                let _ = stream.set_nodelay(true);
                Ok(BrokerStream::new(stream))
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
            Self::Tcp(_) => false,
        }
    }

    /// `const requiresEndpointAuth = typeof LISTEN_TARGET !== "string"` (`broker.ts:284`): only the
    /// TCP endpoint demands the per-run `stateId` credential on `health`/`register`. A socket or
    /// pipe is reachable only by a peer that can already open the filesystem/namespace entry, which
    /// is the credential; a loopback port is reachable by every process on the machine, which is
    /// why the file the port is published in is `0600` and its `stateId` is the actual gate.
    #[must_use]
    pub const fn requires_endpoint_auth(&self) -> bool {
        matches!(self, Self::Tcp(_))
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

    /// ICOM-015 — the opt-in loopback-TCP listen target BINDS, accepts a real client, and publishes
    /// a `broker.port.json` body that this crate's own client-side validation ladder accepts.
    ///
    /// Pre-fix `bind` returned `ErrorKind::Unsupported` for this arm and the test asserted that
    /// refusal, so this fails on the first line. The end-to-end shape is what matters: upstream's
    /// listen target carries port `0` (`paths.ts:112`) and the endpoint file publishes the port the
    /// KERNEL chose (`broker.ts:132-140`), so a port read back from the listen target rather than
    /// from `local_addr()` would publish `0` and every client would fail the `port <= 0` rung.
    #[tokio::test]
    async fn the_tcp_listen_target_binds_accepts_and_publishes_a_readable_endpoint() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut listener = BrokerListener::bind(&BrokerConnectTarget::Tcp(BrokerTcpEndpoint {
            host: INTERCOM_TCP_HOST.to_string(),
            // `{ transport: "tcp", host: INTERCOM_TCP_HOST, port: 0 }` — bind-any (`paths.ts:112`).
            port: 0,
            state_id: None,
        }))
        .await
        .expect("the loopback TCP endpoint binds");

        let addr = listener.local_addr().expect("local_addr").expect("the TCP arm has an address");
        assert_ne!(addr.port(), 0, "the published port is the one the kernel chose, never the 0 bound");

        // A TCP peer carries no uid, so it is never `trustedLocal` (`broker.ts:365`), and it is the
        // one endpoint that demands the `stateId` credential (`:284`).
        assert!(!listener.is_trusted_local());
        assert!(listener.requires_endpoint_auth());

        // The exact file body `run` writes, back through the client-side ladder that reads it.
        let published = BrokerTcpEndpoint {
            host: INTERCOM_TCP_HOST.to_string(),
            port: addr.port(),
            state_id: Some("run-state-id".to_string()),
        };
        let dir = tempfile::tempdir().unwrap();
        let intercom_dir = dir.path();
        std::fs::write(
            crate::transport::target::broker_port_file_path(intercom_dir),
            published.to_port_file_body(),
        )
        .unwrap();
        // `broker_connect_target_in`, not `_from`: the `_from` form derives `<agentDir>/intercom/`
        // (`paths.ts:79`), and this fixture's temp dir IS the intercom dir. `Platform::Windows` +
        // the transport env is the only combination under which `shouldUseWindowsTcpTransport`
        // selects the TCP arm at all (`paths.ts:44-59`), and the crate `#![forbid(unsafe_code)]`s,
        // so the platform and the env both have to be injected rather than set.
        let parsed = crate::transport::target::broker_connect_target_in(
            crate::transport::target::Platform::Windows,
            |k| (k == crate::transport::target::ENV_INTERCOM_TRANSPORT).then(|| "tcp".to_string()),
            intercom_dir,
            intercom_dir,
        )
        .expect("the published endpoint file passes the client validation ladder");
        assert_eq!(parsed, BrokerConnectTarget::Tcp(published));

        // And the bound listener really serves that endpoint.
        let client = tokio::spawn(async move {
            let mut stream = BrokerStream::connect(&parsed).await.expect("connects");
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
