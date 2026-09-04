//! G3 — a STALLED RPC client must not park the command loop.
//!
//! Pi's RPC host emits fire-and-forget. `output()` calls `writeRawStdout` (rpc-mode.ts:60), and
//! `writeRawStdout` (`packages/coding-agent/src/core/output-guard.ts:85-90` at v0.83.0) appends the
//! chunk to a module-level `rawStdoutWriteTail` promise chain and RETURNS — it is not `async` and no
//! caller awaits it:
//!
//! ```text
//! export function writeRawStdout(text: string): void {
//!     if (text.length === 0) return;
//!     rawStdoutWriteTail = rawStdoutWriteTail.then(() => writeRawStdoutChunk(text));
//!     void rawStdoutWriteTail.catch(() => { process.exit(1); });
//! }
//! ```
//!
//! Backpressure is a SEPARATE, explicit await (`waitForRawStdoutBackpressure`, `:95-101`) that pi
//! applies to the AGENT — `session.agent.subscribe(async () => { await
//! waitForRawStdoutBackpressure(); })` (rpc-mode.ts:360-362) — never to the emission itself.
//!
//! Cyrup awaited every emission INLINE inside the command `select!`: eight
//! `write_out(writer, …).await?` arms, and `write_out` does `write_all(...).await` +
//! `flush().await`. A client that stopped reading its end of the pipe therefore filled the socket
//! buffer and parked the ENTIRE loop inside `write_all` — no further stdin line could be read, so
//! `abort`, `abort_bash` and a guest's `ctx.shutdown()` became structurally undeliverable. The
//! commands whose whole purpose is to rescue a wedged session were disabled by exactly the
//! condition that wedges it.
//!
//! These tests drive the real `run_rpc` against a real `AgentSessionRuntime` over a real duplex
//! pipe, with a writer that genuinely returns `Poll::Pending` (a real stalled peer, not a mock that
//! merely records). The first proves a later command is still serviced while the writer is stalled;
//! the second pins the property the fix must not break — nothing queued during the stall is lost.

use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use crate::run_rpc;
use cyrup_core::StopReason;
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use cyrup_session_svc::{AgentSessionRuntime, SessionFactory};
use serde_json::Value;
use tokio::io::{AsyncWrite, AsyncWriteExt, BufReader};

use super::support::{Fixture, base_config_no_ext, create_runtime, fixture, parse_lines};

// ---------------------------------------------------------------------------------------------
// A genuinely stalled peer
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct GateInner {
    open: bool,
    bytes: Vec<u8>,
    waker: Option<Waker>,
}

/// A writer whose `poll_write` really returns `Poll::Pending` until [`Gate::open`] — the async
/// equivalent of a client that stopped draining its pipe. `attempts` counts how many times the
/// host actually tried to write, which is how a test knows the host is *at* the write and not
/// merely on its way there.
#[derive(Clone, Default)]
struct Gate {
    inner: Arc<Mutex<GateInner>>,
    attempts: Arc<AtomicUsize>,
}

impl Gate {
    fn open(&self) {
        let waker = {
            let mut g = self.inner.lock().unwrap();
            g.open = true;
            g.waker.take()
        };
        if let Some(w) = waker {
            w.wake();
        }
    }

    fn attempts(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }

    fn bytes(&self) -> Vec<u8> {
        self.inner.lock().unwrap().bytes.clone()
    }

    fn writer(&self) -> StalledWriter {
        StalledWriter(self.clone())
    }
}

struct StalledWriter(Gate);

impl AsyncWrite for StalledWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.0.attempts.fetch_add(1, Ordering::SeqCst);
        let mut g = self.0.inner.lock().unwrap();
        if !g.open {
            g.waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        g.bytes.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut g = self.0.inner.lock().unwrap();
        if !g.open {
            g.waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

// ---------------------------------------------------------------------------------------------
// The runtime under test
// ---------------------------------------------------------------------------------------------

async fn runtime(fx: &Fixture) -> Arc<AgentSessionRuntime> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("stalled-client answer")],
        StopReason::Stop,
    )]);
    let provider: Arc<dyn Provider> = faux;
    let cfg = base_config_no_ext(fx);
    let target = cfg.target.clone();
    create_runtime(SessionFactory::new(provider, cfg), target).await
}

/// Poll `f` until it returns true or `budget` elapses. Used instead of a fixed sleep so the test
/// survives heavy CPU contention: the deadline only bounds FAILURE, it never bounds success.
async fn within<F: FnMut() -> bool>(budget: Duration, mut f: F) -> bool {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        if f() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

// ---------------------------------------------------------------------------------------------
// G3
// ---------------------------------------------------------------------------------------------

/// The defect itself: with the peer stalled mid-write, a LATER command line must still be read and
/// serviced.
///
/// `set_auto_compaction` is the probe because its effect is observable off the wire
/// (`AgentSession::auto_compaction_enabled`) — the wire is exactly what is unavailable here — and
/// because it is infallible, so a `false` reading can only mean the command was never serviced.
///
/// The test waits for a real `poll_write` attempt before sending the probe, so the host is
/// provably AT the stalled write when the second line arrives; without that, `select!`'s random
/// branch choice could service the probe before the first response was ever attempted and the
/// assertion would hold vacuously.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stalled_client_cannot_park_the_command_loop() {
    let fx = fixture();
    let rt = runtime(&fx).await;

    let initial = rt.session().await.auto_compaction_enabled();
    let probe = !initial;

    // A real duplex pipe; the client's write half stays alive so the host never sees EOF and the
    // ONLY way the probe can take effect is the loop actually servicing it.
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let gate = Gate::default();

    let host = {
        let rt = Arc::clone(&rt);
        let gate = gate.clone();
        tokio::spawn(async move {
            let mut writer = gate.writer();
            run_rpc(&rt, BufReader::new(server), &mut writer).await
        })
    };

    // 1. A command whose response the host will try to write — and stall on.
    client
        .write_all(b"{\"type\":\"get_state\",\"id\":\"a\"}\n")
        .await
        .unwrap();
    assert!(
        within(Duration::from_secs(20), || gate.attempts() > 0).await,
        "the host must reach its first write (the stall point) before the probe is sent"
    );

    // 2. The probe, sent while the writer is provably stalled.
    let probe_line =
        format!("{{\"type\":\"set_auto_compaction\",\"id\":\"b\",\"enabled\":{probe}}}\n");
    client.write_all(probe_line.as_bytes()).await.unwrap();

    // Poll the session's own state (never the wire — the wire is what is unavailable). The deadline
    // bounds FAILURE only, so heavy CPU contention cannot turn a working loop red.
    let serviced = {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            if rt.session().await.auto_compaction_enabled() == probe {
                break true;
            }
            if tokio::time::Instant::now() >= deadline {
                break false;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };

    // Release the peer and close stdin no matter how the assertion goes, so a failure reports the
    // assertion rather than hanging the harness.
    gate.open();
    drop(client);
    let ended = tokio::time::timeout(Duration::from_secs(30), host).await;

    assert!(
        serviced,
        "G3: a command arriving while the client is stalled must still be serviced — the loop \
         awaited its writes inline inside the `select!`, so `abort`/`abort_bash`/`shutdown` could \
         not be delivered to a session whose client had stopped reading"
    );
    ended
        .expect("run_rpc must return once the peer drains and stdin closes")
        .unwrap()
        .unwrap();
}

/// The property the decoupling must not break: everything queued while the peer was stalled is
/// still on the wire, in order, before `run_rpc` returns (Pi's `await flushRawStdout()` on the
/// shutdown path, rpc-mode.ts:737). A queue that is dropped at teardown would trade one bug for a
/// worse one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_queued_during_the_stall_is_lost_at_shutdown() {
    let fx = fixture();
    let rt = runtime(&fx).await;

    let gate = Gate::default();
    let input = concat!(
        r#"{"type":"prompt","id":"1","message":"hello"}"#,
        "\n",
        r#"{"type":"get_state","id":"2"}"#,
        "\n",
    );
    let reader = std::io::Cursor::new(input.as_bytes().to_vec());

    let host = {
        let rt = Arc::clone(&rt);
        let gate = gate.clone();
        tokio::spawn(async move {
            let mut writer = gate.writer();
            run_rpc(&rt, reader, &mut writer).await
        })
    };

    // Let the whole run happen against a peer that never accepts a byte.
    assert!(
        within(Duration::from_secs(20), || gate.attempts() > 0).await,
        "the host must reach its first write"
    );
    assert!(
        gate.bytes().is_empty(),
        "a stalled peer has received nothing yet"
    );

    gate.open();
    tokio::time::timeout(Duration::from_secs(30), host)
        .await
        .expect("run_rpc returns after the peer drains")
        .unwrap()
        .unwrap();

    let lines = parse_lines(&gate.bytes());
    let types: Vec<&str> = lines
        .iter()
        .map(|v| v.get("type").and_then(Value::as_str).unwrap_or(""))
        .collect();
    assert!(
        types.contains(&"agent_settled"),
        "the run's terminal event survives the stall: {types:?}"
    );
    let responses: Vec<&Value> = lines
        .iter()
        .filter(|v| v.get("type").and_then(Value::as_str) == Some("response"))
        .collect();
    let ids: Vec<&str> = responses
        .iter()
        .filter_map(|v| v.get("id").and_then(Value::as_str))
        .collect();
    assert!(
        ids.contains(&"1") && ids.contains(&"2"),
        "both correlated responses survive the stall: {ids:?}"
    );
}
