//! Long-lived, duplex-pipe child-process capability grant (arch-08 §5.2's request/poll bridge;
//! `pi-mcp-adapter-port.md` §3.1 — the locked WIT shape this backs verbatim). Backs MCP stdio
//! transport (`StdioClientTransport`, the majority real-world MCP server shape): unlike the
//! bounded, run-capture-return `exec` grant ([`crate::host::HostServices::exec`]), a `proc` handle
//! stays open across many host calls — the guest keeps writing to a live stdin and polling a live
//! stdout/stderr as the child produces output over time.
//!
//! The host owns each real `tokio::process::Child` + its buffered pipe-reader tasks in
//! [`ProcCaps`], keyed by an opaque `u32` handle (a guest cannot hold a live process across the
//! wasm boundary); the guest polls — the SAME pattern [`crate::caps::http::HttpCaps`]'s streaming-
//! body registry uses.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::process::ChildStdin;
use tokio::sync::watch;
use tokio::sync::Mutex as AsyncMutex;

/// The Pi `proc.kill("SIGTERM")` → `proc.kill("SIGKILL")` escalation's exact timing (Pi
/// `packages/coding-agent/src/core/exec.ts:56-61`): 5 real seconds of grace before forcing.
const DEFAULT_KILL_GRACE: Duration = Duration::from_secs(5);
/// Bounded confirmation wait AFTER sending SIGKILL — SIGKILL is not interceptable, so this should
/// resolve almost immediately once the waiter task reaps the child; a generous cap regardless (a
/// process wedged even past SIGKILL, e.g. stuck in uninterruptible D-state I/O, is the only way this
/// is ever hit).
const KILL_CONFIRM_TIMEOUT: Duration = Duration::from_secs(5);

/// A spawn request for the `proc` capability (mirrors the WIT `proc.spawn` params 1:1). `env` is
/// OVERLAID onto the host's own inherited environment (Pi `resolveEnv`, `server-manager.ts:422` —
/// copies `process.env` then applies overrides), never a full replacement. `capture_stderr` mirrors
/// Pi's debug-mode "inherit" vs "ignore" (`server-manager.ts:111`): `true` pipes + buffers stderr
/// for `read-stderr`; `false` routes it to the null device — NOT the host's own terminal (unlike
/// Node's literal `"inherit"`, mixing an arbitrary guest-spawned child's stderr into the host
/// process's own stdio would be an unrelated-output leak; routing to null instead achieves the same
/// "don't surface it on the MCP protocol stream" effect while keeping host/guest output separate).
#[derive(Clone, Debug, Default)]
pub struct ProcSpawnSpec {
    pub cmd: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
    pub capture_stderr: bool,
}

/// A byte buffer a background pump task appends to and a `read-*` call drains from the front.
type PipeBuf = Arc<Mutex<VecDeque<u8>>>;

/// One live child process the host owns on the guest's behalf. Background tasks continuously pump
/// the REAL stdout/stderr pipes into these buffers so output produced between two guest polls is
/// never lost (the WIT contract: an empty `read-stdout`/`read-stderr` means "no data yet", NOT
/// EOF — EOF is signalled only via a subsequent `poll-exit` returning `some`).
struct ProcEntry {
    pid: u32,
    /// `None` once the pipe is closed (write failed / never had one — `spawn` always pipes stdin,
    /// so this starts `Some`).
    stdin: AsyncMutex<Option<ChildStdin>>,
    stdout_buf: PipeBuf,
    stderr_buf: PipeBuf,
    /// `None` while running; `Some(code)` the instant the waiter task reaps the REAL exit (natural
    /// exit, or a signal this capability sent via [`ProcCaps::kill`]).
    exit_code: watch::Receiver<Option<i32>>,
}

/// The real long-lived-subprocess capability engine (`pi-mcp-adapter-port.md` §3.1). One registry
/// entry per live `spawn`, keyed by an opaque `u32` handle the guest polls. The locked WIT shape has
/// no "close"/"dispose" call (only `kill`, which terminates but does not evict), so entries live for
/// the engine's lifetime by design — the guest can still `poll-exit`/drain trailing buffered output
/// after a `kill`, and a session's `ProcCaps` instance itself is bounded to the session's lifetime.
///
/// **Kill semantics, justified.** Pi's own `proc.kill("SIGTERM")` (the escalation this mirrors,
/// `exec.ts:52-63`) signals ONLY the immediate child — `StdioClientTransport`'s spawn
/// (`server-manager.ts:105-112`) is a plain, non-detached `child_process.spawn`, never a process-
/// group leader. [`ProcCaps::kill`] mirrors that 1:1: it signals the single child pid directly
/// (`cyrup_tools::terminate_pid`/`kill_pid`), NOT a `setsid`/`killpg` process-GROUP kill. Reusing
/// `cyrup-tools`' `kill_tree` (the `exec`/`bash` seam's group-kill machinery, R-03-027) here would
/// diverge from what Pi actually does for stdio MCP transport — that machinery exists because a
/// SHELL-spawned command tree needs group cleanup; a directly-`spawn`ed single MCP server process
/// does not, and killing a wider group than Pi itself would is an unjustified behavior change, not a
/// strictly-more-correct one. Accordingly `spawn` does NOT `setsid` the child either (contrast
/// `cyrup-tools::ops::local::build_argv_command`), keeping the child a plain, non-group-leader
/// process exactly like Node's `spawn(..., {shell:false})` with no `detached` option.
pub struct ProcCaps {
    registry: Mutex<HashMap<u32, Arc<ProcEntry>>>,
    next_handle: AtomicU32,
    /// How long [`Self::kill`] waits after SIGTERM before escalating to SIGKILL — Pi's exact
    /// 5000ms by default ([`DEFAULT_KILL_GRACE`]); overridable ONLY for tests
    /// ([`Self::with_kill_grace`]) so the SIGKILL-escalation path is exercisable without a real
    /// test waiting 5+ real seconds.
    kill_grace: Duration,
}

impl std::fmt::Debug for ProcCaps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcCaps").finish_non_exhaustive()
    }
}

impl Default for ProcCaps {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcCaps {
    pub fn new() -> Self {
        Self::with_kill_grace(DEFAULT_KILL_GRACE)
    }

    /// Build with a caller-supplied SIGTERM→SIGKILL grace period (tests only; production always
    /// gets Pi's real 5s via [`Self::new`]).
    pub fn with_kill_grace(kill_grace: Duration) -> Self {
        Self { registry: Mutex::new(HashMap::new()), next_handle: AtomicU32::new(1), kill_grace }
    }

    fn registry(&self) -> std::sync::MutexGuard<'_, HashMap<u32, Arc<ProcEntry>>> {
        self.registry.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn entry(&self, handle: u32) -> Result<Arc<ProcEntry>, String> {
        self.registry().get(&handle).cloned().ok_or_else(|| format!("no live process for handle {handle}"))
    }

    /// Spawn a REAL long-lived child (the WIT `proc.spawn`): pipes stdin/stdout always, stderr iff
    /// `spec.capture_stderr`. Background tasks immediately start pumping the real stdout/stderr
    /// pipes into per-handle buffers, and a background waiter reaps the child + records its real
    /// exit code the instant it terminates. Synchronous (no `.await` needed — `Command::spawn` is
    /// itself sync; only `tokio::spawn`, which needs a runtime context, not an async fn, to start
    /// the pump/waiter tasks).
    pub fn spawn(&self, spec: &ProcSpawnSpec) -> Result<u32, String> {
        let mut cmd = tokio::process::Command::new(&spec.cmd);
        cmd.args(&spec.args);
        if let Some(cwd) = &spec.cwd {
            cmd.current_dir(cwd);
        }
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(if spec.capture_stderr {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        });
        // Safety net only, not the kill mechanism: an explicit `kill` still runs the real
        // SIGTERM/SIGKILL escalation below. This just prevents a leaked child if the registry
        // entry (and this `ProcCaps`) is dropped without one — e.g. the guest unloads.
        cmd.kill_on_drop(true);

        let mut child = cmd.spawn().map_err(|e| format!("spawn {}: {e}", spec.cmd))?;
        let pid = child.id().ok_or_else(|| format!("spawn {}: no pid assigned", spec.cmd))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let stdout_buf: PipeBuf = Arc::new(Mutex::new(VecDeque::new()));
        let stderr_buf: PipeBuf = Arc::new(Mutex::new(VecDeque::new()));

        if let Some(out) = stdout {
            spawn_pump(out, stdout_buf.clone());
        }
        if let Some(err) = stderr {
            spawn_pump(err, stderr_buf.clone());
        }

        // Reap the child + publish its REAL exit code the instant it terminates (natural exit, or a
        // signal `kill` sent) — `poll_exit`/`kill` read this via the `watch` receiver, never
        // blocking on the process (or the child's own stdio) themselves.
        let (exit_tx, exit_rx) = watch::channel(None);
        tokio::spawn(async move {
            let code = match child.wait().await {
                Ok(status) => status.code().unwrap_or(0),
                Err(_) => -1,
            };
            let _ = exit_tx.send(Some(code));
        });

        let handle = self.next_handle.fetch_add(1, Ordering::Relaxed);
        let entry =
            Arc::new(ProcEntry { pid, stdin: AsyncMutex::new(stdin), stdout_buf, stderr_buf, exit_code: exit_rx });
        self.registry().insert(handle, entry);
        Ok(handle)
    }

    /// Write to the child's REAL stdin (the WIT `proc.write-stdin`). `Err` once the pipe is closed
    /// (child exited / closed stdin) — mirrors a real broken-pipe write failure, never a panic.
    pub async fn write_stdin(&self, handle: u32, data: &[u8]) -> Result<u32, String> {
        let entry = self.entry(handle)?;
        let mut guard = entry.stdin.lock().await;
        let Some(stdin) = guard.as_mut() else {
            return Err(format!("stdin is closed for handle {handle}"));
        };
        if let Err(e) = stdin.write_all(data).await {
            // A closed/broken pipe is terminal — drop the handle so future writes fail fast with the
            // SAME message instead of a fresh (possibly different) io error each time.
            *guard = None;
            return Err(format!("write_stdin: {e}"));
        }
        Ok(u32::try_from(data.len()).unwrap_or(u32::MAX))
    }

    /// Drain whatever REAL stdout bytes are currently buffered (the WIT `proc.read-stdout`) — empty
    /// means "no data yet", NEVER EOF (EOF is signalled only via [`Self::poll_exit`]).
    pub fn read_stdout(&self, handle: u32, max_bytes: u32) -> Result<Vec<u8>, String> {
        Self::drain(&self.entry(handle)?.stdout_buf, max_bytes)
    }

    /// Drain whatever REAL stderr bytes are currently buffered (the WIT `proc.read-stderr`).
    /// Permanently empty when the process was spawned with `capture_stderr: false` (nothing was
    /// ever piped) — a legitimate "no data" answer, never an error.
    pub fn read_stderr(&self, handle: u32, max_bytes: u32) -> Result<Vec<u8>, String> {
        Self::drain(&self.entry(handle)?.stderr_buf, max_bytes)
    }

    fn drain(buf: &PipeBuf, max_bytes: u32) -> Result<Vec<u8>, String> {
        let mut g = buf.lock().map_err(|_| "proc pipe buffer lock poisoned".to_string())?;
        let n = (max_bytes as usize).min(g.len());
        Ok(g.drain(..n).collect())
    }

    /// Poll whether the child has exited (the WIT `proc.poll-exit`); `Some(code)` once terminated,
    /// `None` while still running OR for an unknown handle (the WIT signature carries no error
    /// channel — an unknown handle degrades to "not exited" rather than panicking/erroring).
    pub fn poll_exit(&self, handle: u32) -> Option<i32> {
        let entry = self.registry().get(&handle).cloned()?;
        *entry.exit_code.borrow()
    }

    /// Terminate the child (the WIT `proc.kill`): SIGTERM, then SIGKILL after the grace period IF
    /// still alive — Pi's exact escalation (`exec.ts:52-63`). Returns `Ok` only once the OS process
    /// is CONFIRMED gone (the waiter task observed real termination), never a fire-and-forget signal
    /// send. Idempotent: killing an already-exited handle is a no-op `Ok`.
    pub async fn kill(&self, handle: u32) -> Result<(), String> {
        let entry = self.entry(handle)?;
        if entry.exit_code.borrow().is_some() {
            return Ok(()); // already exited — never re-signal a reaped pid.
        }

        cyrup_tools::terminate_pid(entry.pid)
            .map_err(|e| format!("SIGTERM pid {}: {e}", entry.pid))?;

        if Self::wait_exited(&entry, self.kill_grace).await {
            return Ok(()); // SIGTERM worked within the grace period — no escalation needed.
        }

        // Grace period elapsed and the child is still alive: force it (Pi's exact
        // "if (!proc.killed) proc.kill('SIGKILL')" after 5s, exec.ts:57-61).
        cyrup_tools::kill_pid(entry.pid).map_err(|e| format!("SIGKILL pid {}: {e}", entry.pid))?;

        if Self::wait_exited(&entry, KILL_CONFIRM_TIMEOUT).await {
            Ok(())
        } else {
            Err(format!("process {} did not terminate after SIGKILL", entry.pid))
        }
    }

    /// Wait up to `timeout` for the waiter task to publish a real exit code; `true` if it did.
    /// Checks the CURRENT value first (never misses an exit that already landed before this call
    /// started waiting), then awaits further `watch` changes for the remainder of the budget.
    async fn wait_exited(entry: &ProcEntry, timeout: Duration) -> bool {
        if entry.exit_code.borrow().is_some() {
            return true;
        }
        let mut rx = entry.exit_code.clone();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return rx.borrow().is_some();
            }
            match tokio::time::timeout(remaining, rx.changed()).await {
                Ok(Ok(())) => {
                    if rx.borrow().is_some() {
                        return true;
                    }
                    // A `watch` change that isn't the exit publish shouldn't happen (only one sender
                    // write ever occurs), but loop defensively rather than assume.
                }
                Ok(Err(_)) => return rx.borrow().is_some(), // sender dropped without publishing
                Err(_) => return rx.borrow().is_some(),     // timed out
            }
        }
    }
}

/// Continuously pump a real pipe (`AsyncRead`) into `buf` until EOF/error — the background task
/// that keeps `read-stdout`/`read-stderr` non-lossy between polls.
fn spawn_pump<R>(mut reader: R, buf: PipeBuf)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Ok(mut g) = buf.lock() {
                        g.extend(chunk.get(..n).unwrap_or(&[]).iter().copied());
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn spec(cmd: &str, args: &[&str]) -> ProcSpawnSpec {
        ProcSpawnSpec {
            cmd: cmd.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            env: Vec::new(),
            cwd: None,
            capture_stderr: false,
        }
    }

    /// A real `cat` is a genuine duplex pipe: write a line, poll stdout across MULTIPLE calls until
    /// the real echoed bytes show up (proving a live pipe, not a one-shot capture), then again for a
    /// second line — proving the SAME handle stays open and keeps echoing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_is_a_real_live_duplex_pipe_across_multiple_polls() {
        let caps = ProcCaps::new();
        let handle = caps.spawn(&spec("cat", &[])).expect("cat spawns");

        caps.write_stdin(handle, b"first\n").await.expect("write first line");
        let got = poll_until_contains(&caps, handle, b"first\n").await;
        assert!(got, "the real cat echoed the first line back across multiple polls");

        caps.write_stdin(handle, b"second\n").await.expect("write second line on the SAME handle");
        let got = poll_until_contains(&caps, handle, b"second\n").await;
        assert!(got, "the SAME live pipe kept echoing after the first round-trip");

        assert_eq!(caps.poll_exit(handle), None, "cat is still running (no EOF sent yet)");

        caps.kill(handle).await.expect("kill terminates the real cat");
        assert!(caps.poll_exit(handle).is_some(), "poll_exit reports the real exit after kill");
    }

    /// Poll `read_stdout` repeatedly (never blocking) until the accumulated bytes contain `needle`
    /// or a generous deadline elapses.
    async fn poll_until_contains(caps: &ProcCaps, handle: u32, needle: &[u8]) -> bool {
        let mut acc = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            let chunk = caps.read_stdout(handle, 4096).expect("read_stdout on a live handle");
            acc.extend_from_slice(&chunk);
            if acc.windows(needle.len()).any(|w| w == needle) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    /// `poll_exit` is `None` while genuinely running and `Some(code)` with the REAL exit code once
    /// the child exits on its own (no `kill` involved) — proving the background waiter observes a
    /// natural exit, not just a `kill`-driven one.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn poll_exit_reports_still_running_then_the_real_natural_exit_code() {
        let caps = ProcCaps::new();
        let handle = caps.spawn(&spec("sh", &["-c", "sleep 0.2; exit 7"])).expect("sh spawns");
        assert_eq!(caps.poll_exit(handle), None, "still running immediately after spawn");

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut code = None;
        while tokio::time::Instant::now() < deadline {
            if let Some(c) = caps.poll_exit(handle) {
                code = Some(c);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(code, Some(7), "the REAL natural exit code round-trips (no kill involved)");
    }

    /// `kill` on a NORMAL (SIGTERM-obeying) child terminates it well within the grace period, and
    /// the OS process is verifiably gone afterward (not merely a successful WIT-level return) —
    /// checked via `kill -0`, which fails once the pid no longer exists.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn kill_terminates_a_real_running_child_and_the_os_process_is_gone() {
        let caps = ProcCaps::new();
        let handle = caps.spawn(&spec("sleep", &["30"])).expect("sleep spawns");
        assert_eq!(caps.poll_exit(handle), None, "sleep is genuinely still running");

        let started = tokio::time::Instant::now();
        caps.kill(handle).await.expect("kill terminates the real sleep");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "a SIGTERM-obeying child dies well within the grace period, no SIGKILL escalation needed"
        );
        assert!(caps.poll_exit(handle).is_some(), "poll_exit reflects the real termination");

        // Independently verify at the OS level (never just trust our own accounting): the pid must
        // no longer exist. `kill -0` sends no signal; it only checks existence/permission.
        let pid = {
            let reg = caps.registry();
            reg.get(&handle).expect("entry still present after kill").pid
        };
        let alive = std::process::Command::new("kill").args(["-0", &pid.to_string()]).status();
        assert!(
            alive.map(|s| !s.success()).unwrap_or(true),
            "kill -0 on the killed pid must fail — the OS process is really gone"
        );
    }

    /// The FORCED SIGKILL escalation, exercised deterministically: a child that installs `trap ''
    /// TERM` ignores SIGTERM outright, so `kill` MUST wait out the (test-shortened) grace period and
    /// then send SIGKILL — which the child cannot ignore — to actually terminate it. Proves the
    /// escalation path is real, not just documented.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn kill_escalates_to_sigkill_when_the_child_ignores_sigterm() {
        // A short grace period so this test stays fast — the escalation TIMING is Pi's real 5000ms
        // in production ([`DEFAULT_KILL_GRACE`]); only the test injects a shorter one.
        let caps = ProcCaps::with_kill_grace(Duration::from_millis(150));
        let handle = caps
            .spawn(&spec("sh", &["-c", "trap '' TERM; while true; do sleep 1; done"]))
            .expect("the SIGTERM-ignoring shell spawns");
        assert_eq!(caps.poll_exit(handle), None, "genuinely running before kill");
        // Let the shell actually REACH `trap '' TERM` before we signal it — otherwise SIGTERM can
        // race the trap install and kill it via the (still-default) terminate disposition, which
        // would falsely "pass" this test without ever exercising the SIGKILL escalation.
        tokio::time::sleep(Duration::from_millis(100)).await;

        let pid = {
            let reg = caps.registry();
            reg.get(&handle).expect("entry present").pid
        };

        let started = tokio::time::Instant::now();
        caps.kill(handle).await.expect("kill still terminates it via the SIGKILL escalation");
        assert!(
            started.elapsed() >= Duration::from_millis(150),
            "the grace period was genuinely waited out before escalating (elapsed {:?})",
            started.elapsed()
        );
        assert!(caps.poll_exit(handle).is_some(), "poll_exit reflects the SIGKILL-forced termination");

        let alive = std::process::Command::new("kill").args(["-0", &pid.to_string()]).status();
        assert!(
            alive.map(|s| !s.success()).unwrap_or(true),
            "kill -0 on the SIGKILLed pid must fail — the OS process is really gone, SIGTERM alone \
             would have left it running"
        );
    }

    /// `capture_stderr: false` routes stderr to the null device: `read_stderr` legitimately stays
    /// empty forever (never an error) even though the child DOES write to stderr.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn uncaptured_stderr_reads_empty_never_errors() {
        let caps = ProcCaps::new();
        let mut s = spec("sh", &["-c", "echo to-stderr 1>&2; sleep 0.2"]);
        s.capture_stderr = false;
        let handle = caps.spawn(&s).expect("spawns");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            caps.read_stderr(handle, 4096).expect("read_stderr never errors"),
            Vec::<u8>::new(),
            "uncaptured stderr yields nothing, not an error"
        );
    }

    /// An unknown handle is a real `Err` for every call EXCEPT `poll_exit` (which has no error
    /// channel in the WIT signature and degrades to `None`, per its doc).
    #[tokio::test]
    async fn unknown_handle_errors_except_poll_exit() {
        let caps = ProcCaps::new();
        assert!(caps.write_stdin(999, b"x").await.is_err());
        assert!(caps.read_stdout(999, 10).is_err());
        assert!(caps.read_stderr(999, 10).is_err());
        assert!(caps.kill(999).await.is_err());
        assert_eq!(caps.poll_exit(999), None);
    }
}
