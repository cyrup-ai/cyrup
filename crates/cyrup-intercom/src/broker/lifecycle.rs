//! The broker process lifecycle: bind, accept, idle-shutdown, tear down
//! (`broker.ts:123-152,181,286-296,606-633,636`).
//!
//! [`run`] is the `cyrup __intercom-broker` entrypoint, re-exported as `crate::broker::run` — the
//! only public item the module root itself contributes; the four `pub mod` siblings export their
//! own. It binds the listen target through
//! [`super::listener::BrokerListener`], claims the runtime files, runs the accept loop, and returns
//! once SIGTERM/SIGINT or the 5 s idle auto-shutdown has cleaned everything up.
//!
//! Split out of `broker/mod.rs` so the module root is a facade over the concerns rather than the
//! place the process is also implemented.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;

use crate::config;
use crate::paths;

use super::conn::spawn_connection;
use super::limits::SHUTDOWN_DELAY_MS;
use super::listener;
use super::runtime_claim;
use super::state::{BrokerState, lock};

/// Schedule the 5 s auto-shutdown check (`scheduleShutdownCheck`, `broker.ts:286-296`). Only one is
/// ever pending; a `register` in the window bumps `shutdown_gen`, making the pending check stale.
pub(super) fn schedule_shutdown_check(state: &Arc<Mutex<BrokerState>>) {
    let mut g = lock(state);
    if g.shutdown_scheduled {
        return;
    }
    g.shutdown_scheduled = true;
    let generation = g.shutdown_gen;
    let shutdown = g.shutdown.clone();
    let task_state = state.clone();
    // The handle is installed under the SAME lock the flag was set under, so `handle_register`
    // never observes `shutdown_scheduled == true` with an empty `shutdown_task` slot. The task's
    // first action is a 5 s sleep, so it cannot contend for this guard.
    g.shutdown_task = Some(tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(SHUTDOWN_DELAY_MS)).await;
        let empty_and_current = {
            let mut g = lock(&task_state);
            g.shutdown_scheduled = false;
            g.shutdown_task = None;
            g.shutdown_gen == generation && g.sessions.is_empty()
        };
        if empty_and_current {
            tracing::info!("no sessions connected, shutting down");
            shutdown.notify_one();
        }
    }));
}

/// Tear down the whole broker on shutdown (`shutdown`, `broker.ts:606-633`): end every session,
/// clear the maps, unlink the runtime files.
fn shutdown_broker(
    state: &Arc<Mutex<BrokerState>>,
    listen_target: &crate::transport::target::BrokerConnectTarget,
    pid_path: &std::path::Path,
    port_path: &std::path::Path,
) {
    {
        let mut g = lock(state);
        for (_id, h) in g.connections.drain() {
            h.close.notify_one();
        }
        g.sessions.clear();
        g.session_order.clear();
        g.ask_edges.clear();
        // `shutdown()` clears every routing table, not just the sessions
        // (`v0.10.1 broker/broker.ts:1411-1415`). Parked mail is in-memory only upstream too — a
        // broker restart loses it by design, which is why `MAILBOX_MESSAGE_RETENTION_MS` is a
        // liveness bound rather than a durability promise.
        g.message_receipt_routes.clear();
        g.disconnected_sessions.clear();
        g.mailbox_messages.clear();
        g.unregistered.clear();
    }
    // `unlinkSync(LISTEN_TARGET)` guarded by
    // `typeof LISTEN_TARGET === "string" && process.platform !== "win32"`
    // (`v0.10.1 broker/broker.ts:1416-1418`) — a named pipe has no filesystem entry to remove.
    listener::unlink_stale_endpoint(listen_target);
    // `try { unlinkSync(PORT_PATH) } catch {}` (`v0.10.1 broker/broker.ts:1423-1427`, comment
    // verbatim: "The TCP endpoint file only exists when opt-in TCP transport is active") — done
    // unconditionally and ignoring the error, so a socket-transport shutdown that has no endpoint
    // file is silent, and a stale one left by a crashed TCP broker cannot outlive this process and
    // point the next client at a dead port under a credential nobody holds.
    let _ = std::fs::remove_file(port_path);
    let _ = std::fs::remove_file(pid_path);
}

/// The `cyrup __intercom-broker` entrypoint (`new IntercomBroker().start()`, `broker.ts:636`).
/// Binds the listen target (Unix socket / Windows named pipe, [`listener::BrokerListener`]), writes
/// the pid file, runs the accept loop, and shuts down on SIGTERM/SIGINT or the 5 s idle
/// auto-shutdown. Returns once the endpoint + runtime files are cleaned up.
///
/// # Errors
/// Returns an I/O error if the intercom dir cannot be created or the listen target cannot be bound.
pub async fn run() -> std::io::Result<()> {
    // `ask_timeout_ms` hard-errors on an invalid env value, matching pi's uncaught throw
    // (`config.ts:14-16`) that crashes `new IntercomBroker()` — a class-field initializer that runs
    // INSIDE the constructor, i.e. before `.start()` ever binds the listener or writes any file
    // (`broker.ts:139`). Resolved here FIRST, before any startup side effect (dir/socket/pid), so an
    // invalid env value fails the whole process before anything is created — never a socket/pid file
    // left behind for an external caller to observe as a falsely "started" broker.
    let ask_timeout = config::ask_timeout_ms()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    let agent_dir = paths::agent_dir_path();
    let intercom_dir = paths::intercom_dir_path(&agent_dir);
    paths::ensure_intercom_runtime_dir(&intercom_dir)?;
    // `const LISTEN_TARGET = getBrokerListenTarget();` (`v0.9.2 broker/broker.ts:26`) — the socket
    // path on POSIX, the `\\.\pipe\cyrup-intercom-<agent dir>` name on Windows, or the loopback-TCP
    // endpoint under the Windows-only opt-in (`broker/paths.ts:107-116`). This replaces a direct
    // `paths::broker_socket_path(...)` read, which hard-coded the POSIX arm.
    let listen_target = crate::transport::target::broker_listen_target(&agent_dir);
    let pid_path = paths::broker_pid_path(&intercom_dir);
    // `PORT_PATH` (`broker.ts:27` via `getBrokerPortFilePath`): written only on the TCP arm, but
    // resolved unconditionally because `shutdown` unlinks it unconditionally (`:1423-1427`).
    let port_path = crate::transport::target::broker_port_file_path(&intercom_dir);

    // Claim the runtime BEFORE touching anything in it (`assertNoLiveBroker(PID_PATH)`,
    // `v0.9.2 broker/broker.ts:231`, sitting between `ensureIntercomRuntimeDir` at `:230` and the
    // stale-socket unlink at `:233-238`). A second broker must DECLINE while an incumbent is alive
    // rather than unlink its socket and bind its own: the incumbent keeps every connection it has
    // already accepted (the unlinked inode outlives its name) but is unreachable to new clients, so
    // the theft silently partitions the session graph instead of failing. Only a *live* pid refuses
    // — a stale `broker.pid` left by a SIGKILLed broker is still reclaimable, or a crash would wedge
    // intercom until a human deleted the file. See `broker::runtime_claim`.
    runtime_claim::assert_no_live_broker(&pid_path)?;

    // Unlink a stale socket left by a crashed broker (`v0.9.2 broker/broker.ts:233-238`;
    // `v0.7.0 broker/broker.ts:143-148`), under upstream's own
    // `typeof LISTEN_TARGET === "string" && platform !== "win32"` guard (`:116`).
    listener::unlink_stale_endpoint(&listen_target);
    // CYRUP-DELTA (`v0.9.2 broker/broker.ts:239` `net.createServer().listen(LISTEN_TARGET)`, and
    // `broker/paths.ts:65-74`, which has no length guard either): upstream loses the reason the same
    // way, so this is a shared robustness gap, not a parity divergence. What is added here is only
    // the DIAGNOSTIC — `sockaddr_un.sun_path` is 104 bytes on macOS and 108 on Linux, so a deep
    // `HOME`/`CYRUP_AGENT_DIR` makes this bind fail with a bare "path must be shorter than SUN_LEN"
    // that names neither the limit's cause nor the path. Naming both here is what makes the parent's
    // captured-stderr message (`transport::spawn::BrokerStderrTail`) actionable.
    let endpoint = describe_listen_target(&listen_target);
    let mut listener = listener::BrokerListener::bind(&listen_target).await.map_err(|e| {
        std::io::Error::new(
            e.kind(),
            format!("failed to bind the intercom broker endpoint at {endpoint} ({} bytes): {e}", endpoint.len()),
        )
    })?;
    // `onListening` (`broker.ts:127-146`): the string arm restricts the socket; the TCP arm
    // publishes the endpoint the kernel actually gave it, credentialled with this run's
    // `BROKER_STATE_ID`, into `broker.port.json` — the file `broker_connect_target` validates
    // (`transport/target.rs`'s ladder) and every client reads its `stateId` from.
    let endpoint_state_id = match &listen_target {
        crate::transport::target::BrokerConnectTarget::Socket(path) => {
            // `restrictIntercomRuntimeFile(LISTEN_TARGET)` for the string arm (`broker.ts:128-130`);
            // itself a no-op off POSIX (`paths.ts:128-135`).
            let _ = paths::restrict_intercom_runtime_file(path);
            None
        }
        crate::transport::target::BrokerConnectTarget::Tcp(target) => {
            // `const address = this.server.address(); if (!address || typeof address === "string")
            // throw new Error("Intercom TCP broker started without a TCP address")`
            // (`broker.ts:132-135`).
            let Some(addr) = listener.local_addr()? else {
                return Err(std::io::Error::other(
                    "Intercom TCP broker started without a TCP address",
                ));
            };
            // `const BROKER_STATE_ID = randomUUID()` (`broker.ts:29`) — one credential per broker
            // PROCESS, generated here (rather than at module load) so it cannot outlive, or be
            // reused across, the endpoint file that publishes it.
            let state_id = uuid::Uuid::new_v4().to_string();
            let endpoint = crate::transport::target::BrokerTcpEndpoint {
                host: target.host.clone(),
                port: addr.port(),
                state_id: Some(state_id.clone()),
            };
            // `writeFileSync(PORT_PATH, `${JSON.stringify(endpoint)}\n`, { mode:
            // INTERCOM_RUNTIME_FILE_MODE }); restrictIntercomRuntimeFile(PORT_PATH)`
            // (`broker.ts:140-141`). The 0600 mode is the real gate on the credential: a loopback
            // port is reachable by every process on the machine, so the file's permissions are what
            // keep `stateId` to this user.
            std::fs::write(&port_path, endpoint.to_port_file_body())?;
            let _ = paths::restrict_intercom_runtime_file(&port_path);
            Some(state_id)
        }
    };
    std::fs::write(&pid_path, std::process::id().to_string())?;
    let _ = paths::restrict_intercom_runtime_file(&pid_path);
    tracing::info!(pid = std::process::id(), endpoint = %endpoint, "intercom broker started");

    let shutdown = Arc::new(Notify::new());
    let state = Arc::new(Mutex::new(
        BrokerState::new(ask_timeout, shutdown.clone())
            .with_listen_endpoint(listener.is_trusted_local(), endpoint_state_id),
    ));
    let mut next_conn_id: u64 = 0;

    // `process.on("SIGTERM"|"SIGINT", () => this.shutdown())` (`broker.ts:181-182`).
    //
    // # [CYRUP-DELTA] — the terminate signal is per-platform because the OS is
    //
    // Upstream symbol: `broker.ts:181-182`. Node synthesises a `SIGTERM`/`SIGINT` listener on every
    // platform, but on Windows there are no POSIX signals underneath: libuv raises `SIGINT` from a
    // console Ctrl-C and simply never raises `SIGTERM` (`taskkill` without `/F` delivers a console
    // CTRL_CLOSE/CTRL_SHUTDOWN event instead). `tokio::signal::unix` does not exist off POSIX at
    // all, so the same intent is expressed with the platform's own events: Ctrl-C everywhere, plus
    // SIGTERM on POSIX and the console close/shutdown controls on Windows. The observable behaviour
    // — a polite terminate reaches `shutdown_broker`, so the pid file and (on POSIX) the socket are
    // removed rather than orphaned — is the same one upstream gets.
    let mut terminate = TerminateSignal::install()?;

    loop {
        tokio::select! {
            () = shutdown.notified() => break,
            _ = tokio::signal::ctrl_c() => break,
            () = terminate.recv() => break,
            accepted = listener.accept() => {
                match accepted {
                    Ok(stream) => {
                        let conn_id = next_conn_id;
                        next_conn_id = next_conn_id.wrapping_add(1);
                        spawn_connection(conn_id, stream, state.clone());
                    }
                    Err(e) => tracing::warn!(error = %e, "intercom broker accept failed"),
                }
            }
        }
    }

    tracing::info!("intercom broker shutting down");
    shutdown_broker(&state, &listen_target, &pid_path, &port_path);
    Ok(())
}

/// The listen target rendered for diagnostics — the socket path / pipe name, or `host:port`.
fn describe_listen_target(target: &crate::transport::target::BrokerConnectTarget) -> String {
    match target {
        crate::transport::target::BrokerConnectTarget::Socket(path) => path.display().to_string(),
        crate::transport::target::BrokerConnectTarget::Tcp(e) => format!("{}:{}", e.host, e.port),
    }
}

/// The platform's "please terminate" event, standing in for upstream's `process.on("SIGTERM")`
/// (`broker.ts:181`). See the CYRUP-DELTA at its installation site in [`run`].
struct TerminateSignal {
    #[cfg(unix)]
    sigterm: tokio::signal::unix::Signal,
    #[cfg(windows)]
    close: tokio::signal::windows::CtrlClose,
    #[cfg(windows)]
    shutdown: tokio::signal::windows::CtrlShutdown,
}

impl TerminateSignal {
    /// Register the handler(s). Errors propagate exactly as the old bare
    /// `tokio::signal::unix::signal(...)?` did.
    fn install() -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self {
                sigterm: tokio::signal::unix::signal(
                    tokio::signal::unix::SignalKind::terminate(),
                )?,
            })
        }
        #[cfg(windows)]
        {
            Ok(Self {
                close: tokio::signal::windows::ctrl_close()?,
                shutdown: tokio::signal::windows::ctrl_shutdown()?,
            })
        }
    }

    /// Resolve on the first terminate-shaped event.
    async fn recv(&mut self) {
        #[cfg(unix)]
        {
            self.sigterm.recv().await;
        }
        #[cfg(windows)]
        {
            tokio::select! {
                _ = self.close.recv() => {}
                _ = self.shutdown.recv() => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::Mutex;
    use super::*;
    use super::super::state::BrokerState;
    use super::super::state::lock;
    use tokio::sync::Notify;

    /// ICOM-005 — `register` must NULL the pending auto-shutdown handle
    /// (`v0.10.1 broker/broker.ts:378-381`), not merely bump the generation. Red against the pre-fix
    /// code: `shutdown_scheduled` stayed `true`, so the NEXT disconnect's `schedule_shutdown_check`
    /// early-returned and the re-arm was lost, leaving an idle broker alive forever.
    #[tokio::test]
    async fn a_register_clears_the_pending_shutdown_so_a_later_disconnect_can_re_arm() {
        let state: Arc<Mutex<BrokerState>> =
            Arc::new(Mutex::new(BrokerState::new(30_000, Arc::new(Notify::new()))));
        // t=0: the last session left → a check is armed.
        schedule_shutdown_check(&state);
        assert!(lock(&state).shutdown_scheduled, "armed");

        // t=1: a register lands inside the 5 s window.
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut sid = None;
        lock(&state).handle_frame(
            1,
            &tx,
            &json!({
                "type": "register", "sessionId": "s1",
                "session": { "cwd": "/w", "model": "m", "pid": 1, "startedAt": 0, "lastActivity": 0 },
            }),
            &mut sid,
            1_000,
        );
        assert!(!lock(&state).shutdown_scheduled, "a register cancels the pending check");
        assert!(lock(&state).shutdown_task.is_none(), "and drops its handle");

        // t=2: that session disconnects → the check must arm AGAIN.
        lock(&state).sessions.clear();
        schedule_shutdown_check(&state);
        assert!(
            lock(&state).shutdown_scheduled,
            "the re-arm must not be swallowed by a stale `shutdown_scheduled`"
        );
    }
}
