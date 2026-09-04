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
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::watch;

mod npx_resolver;

/// The REAL `proc` teardown escalation's exact timing — the actual majority-case MCP-stdio
/// consumer, `StdioClientTransport.close()` (`@modelcontextprotocol/sdk@1.25.1`
/// `dist/cjs/client/stdio.js:144-179`): close stdin (EOF), wait 2000ms; if still alive, SIGTERM,
/// wait ANOTHER 2000ms; if still alive, SIGKILL. The SAME 2000ms constant backs BOTH waits in the
/// real transport (`setTimeout(resolve, 2000)` appears twice, stdio.js:159/167) — [`ProcCaps::kill`]
/// reuses this ONE value for both of its own two waits for the same reason.
///
/// NOT Pi's `packages/coding-agent/src/core/exec.ts:52-63` `killProcess` (that escalation is the
/// separate, bounded one-shot WASM `exec` capability grant's kill path — a genuinely different code
/// path from a long-lived duplex-pipe MCP transport child; see
/// `cyrup-tools::ops::local::LocalProc::exec_argv`, which reuses these SAME `terminate_pid`/
/// `kill_pid` single-pid primitives, for where that 5000ms timing is actually ported).
const DEFAULT_KILL_GRACE: Duration = Duration::from_secs(2);
/// Bounded confirmation wait AFTER sending SIGKILL. The real `StdioClientTransport.close()` fires
/// SIGKILL and returns immediately (fire-and-forget, stdio.js:169-176, no further wait) — but
/// [`ProcCaps::kill`]'s OWN contract (doc above) promises `Ok` only once the OS process is
/// CONFIRMED gone, which the real transport's `onclose` callback (not its `close()` return value)
/// is what actually signals in Node. SIGKILL is not interceptable, so this should resolve almost
/// immediately once the waiter task reaps the child; a generous cap regardless (a process wedged
/// even past SIGKILL, e.g. stuck in uninterruptible D-state I/O, is the only way this is ever hit).
const KILL_CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);

/// Bounds how long [`ProcCaps::write_stdin`] will block in TOTAL — waiting to acquire the per-handle
/// stdin lock AND performing the real pipe write — a single ceiling over the WHOLE call, not just the
/// write. Unlike [`DEFAULT_KILL_GRACE`]/[`KILL_CONFIRM_TIMEOUT`], there is no Pi/real-consumer exact
/// value to port here: Node's `ChildProcess.stdin.write()` (the real `StdioClientTransport.send`,
/// `@modelcontextprotocol/sdk@1.25.1` `dist/cjs/client/stdio.js:189-207`) is asynchronous and
/// non-blocking by construction — it returns `false` immediately once the OS pipe fills and lets
/// the caller await a later `'drain'` event, so the real transport never blocks a Node event-loop
/// tick on a slow/non-reading child the way `stdin.write_all(data).await` (a direct `.await` on
/// the SAME multi-threaded tokio worker `write_stdin`'s `block_in_place`+`block_on` bridge runs
/// on, `cyrup-session-svc/src/host_services.rs`) can. Without SOME bound, a guest that spawns a
/// child which never reads its stdin (or reads too slowly to keep up) can hang that worker thread
/// — and therefore the whole session's `write-stdin` call — indefinitely; `note_dialog_wait`
/// (closing the SEPARATE epoch-wedge finding this shares a root cause with) only forgives the WASM
/// epoch deadline, it does nothing to bound the underlying real wall-clock block. As with
/// [`MAX_PIPE_BUFFER_BYTES`]/[`MAX_SPAWNED_PROCESSES`] below, the point is FINITE, not a specific
/// magic number — this is a deliberately generous cap comfortably above any legitimate write to a
/// live, cooperating child, while still guaranteeing the call can never hang forever.
///
/// L4 review: [`ProcCaps::write_stdin`] wraps BOTH `entry.stdin.lock().await` and `write_all` in ONE
/// outer `tokio::time::timeout(WRITE_STDIN_TIMEOUT, ...)` — a version that bounded only the write
/// itself (locking first, unbounded, THEN racing the write against this constant) let two concurrent
/// `write_stdin` calls against the SAME handle compound to up to ~2x this ceiling: the second call's
/// `write_all` cannot even START until the first's lock-holding write finishes, but its own timeout
/// clock did not start ticking until it acquired the lock. Bounding lock-acquisition + write together
/// under one deadline closes that compounding gap. This is a DIFFERENT scenario from the `kill`-vs-
/// `write_stdin` race [`ProcCaps::kill`]'s `try_lock` (not `.lock().await`) already closes (`a9f776d`,
/// doc above `kill`'s own `entry.stdin.try_lock()` call) — that fix is about `kill` never blocking on
/// this mutex at all; this one is about two `write_stdin` calls contending for it.
const WRITE_STDIN_TIMEOUT: Duration = Duration::from_secs(30);

/// Bounds how many `proc.spawn` entries a single `ProcCaps` (one per session) can ever create.
/// Unlike [`crate::caps::http::HttpCaps`]'s `streams` registry, entries here are NEVER evicted by
/// design (doc on [`ProcCaps`] below: "no close/dispose call... entries live for the engine's
/// lifetime" — the guest can still drain trailing buffered output after a `kill`), so this is a cap
/// on TOTAL processes ever spawned over the session's life, not merely concurrently-live ones — each
/// entry holds a REAL OS process (until killed) plus two pump tasks (`spawn_pump`) and a waiter task,
/// so an unbounded registry lets a guest that keeps spawning without ever reusing/limiting itself
/// exhaust host process-table/task resources over time. No Pi-derived exact count to port — the real
/// consumer, `pi-mcp-adapter/server-manager.ts:41,60-83`'s `connections` map, bounds real-world
/// concurrent live children implicitly by the size of the user's OWN mcp server config (typically a
/// handful) — so, like `crate::caps::http::MAX_OPEN_STREAMS`, this is a deliberately generous cap
/// comfortably above any realistic legitimate count, while still guaranteeing bounded worst-case
/// growth.
const MAX_SPAWNED_PROCESSES: usize = 256;

/// A spawn request for the `proc` capability (mirrors the WIT `proc.spawn` params 1:1). `env` is
/// OVERLAID onto the host's own inherited environment (Pi `resolveEnv`, `server-manager.ts:422-435` —
/// copies `process.env`, then applies each override VALUE through `interpolateEnvRecord`/
/// `interpolateEnvVars`, `utils.ts:62-76`), never a full replacement — [`ProcCaps::spawn`] applies
/// this itself, see [`interpolate_env_vars`].
///
/// `cwd`, by contrast, MUST already be fully resolved (Pi `resolveConfigPath`,
/// `server-manager.ts:110`/`utils.ts:78-87`, already applied) by the time it reaches
/// [`ProcCaps::spawn`] — `spawn` uses it verbatim, with NO further interpolation/tilde-expansion.
/// This is a deliberate split from `env`: unlike Pi (where `resolveConfigPath(definition.cwd)` is
/// the ONLY source of a `cwd`, so resolving it right where it's consumed is equivalent to resolving
/// it right where it's guest/config-authored), cyrup's `LiveHostServices::proc_spawn`
/// (`cyrup-session-svc/src/host_services.rs`) also injects its OWN host-computed, already-trusted
/// default (the session's project directory) when a guest omits `cwd` entirely — a mechanism with no
/// Pi equivalent. Resolving `cwd` here, unconditionally, would re-interpolate THAT host-injected
/// default too, corrupting (or outright breaking spawn, if the resulting path doesn't exist) any
/// session whose real project directory happens to literally contain a `${...}`/`$env:...`
/// substring or start with `~`. So resolution now happens ONCE, at the true guest/config-authored
/// boundary — `host/live.rs`'s `proc::Host::spawn` WIT handler, which calls
/// [`resolve_config_path`] on the RAW guest-supplied string before it ever reaches
/// `LiveHostServices::proc_spawn`'s defaulting step — exactly mirroring where Pi's own
/// `resolveConfigPath(definition.cwd)` runs, on the config-authored value only.
///
/// `capture_stderr` mirrors Pi's debug-mode "inherit" vs "ignore" (`server-manager.ts:111`): `true`
/// pipes + buffers stderr for `read-stderr`; `false` routes it to the null device — NOT the host's
/// own terminal (unlike Node's literal `"inherit"`, mixing an arbitrary guest-spawned child's stderr
/// into the host process's own stdio would be an unrelated-output leak; routing to null instead
/// achieves the same "don't surface it on the MCP protocol stream" effect while keeping host/guest
/// output separate).
#[derive(Clone, Debug, Default)]
pub struct ProcSpawnSpec {
    pub cmd: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
    pub capture_stderr: bool,
}

/// Interpolate `${VAR}`/`$env:VAR` placeholders in `value` against the HOST's own environment — a
/// direct Rust port of Pi's `interpolateEnvVars` (`pi-mcp-adapter/utils.ts:62-66`), including its
/// exact two-pass order (every `${VAR}` is resolved FIRST, against a `\w+` = `[A-Za-z0-9_]+` name;
/// THEN every `$env:VAR` is resolved against the resulting string) and its "missing variable resolves
/// to the empty string" fallback (`process.env[name] ?? ""`). An unrecognized/malformed placeholder
/// (unterminated `${`, an empty or non-word name) is left byte-for-byte untouched, matching the JS
/// regex simply not matching such input.
pub(crate) fn interpolate_env_vars(value: &str) -> String {
    interpolate_env_vars_with(value, |name| std::env::var(name).ok())
}

/// [`interpolate_env_vars`] against an injected `lookup` rather than the REAL process environment —
/// the real entry point above is a thin wrapper around this with `std::env::var`. Exists so the
/// substitution logic itself is unit-testable hermetically: `cyrup-ext` is `#![forbid(unsafe_code)]`
/// (`src/lib.rs:20`) crate-wide, and edition 2024 makes `std::env::set_var`/`remove_var` `unsafe fn`
/// — tests cannot mutate the real process environment at all, so they inject a fixed lookup instead.
fn interpolate_env_vars_with(
    value: &str,
    lookup: impl Fn(&str) -> Option<String> + Copy,
) -> String {
    interpolate_dollar_env(&interpolate_braces(value, lookup), lookup)
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The `\$\{(\w+)\}` half of [`interpolate_env_vars_with`].
fn interpolate_braces(value: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(value.len());
    let mut i = 0;
    while i < value.len() {
        if let Some(after_open) = value[i..].strip_prefix("${")
            && let Some(close_rel) = after_open.find('}')
            && let Some(name) = after_open.get(..close_rel)
            && !name.is_empty()
            && name.bytes().all(is_word_byte)
        {
            out.push_str(&lookup(name).unwrap_or_default());
            i += 2 + close_rel + 1;
            continue;
        }
        let ch = value[i..].chars().next().unwrap_or('\u{0}');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// The `\$env:(\w+)` half of [`interpolate_env_vars_with`] (the PowerShell-style form Pi also
/// supports).
fn interpolate_dollar_env(value: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    const PREFIX: &str = "$env:";
    let mut out = String::with_capacity(value.len());
    let mut i = 0;
    while i < value.len() {
        if value[i..].starts_with(PREFIX) {
            let rest = &value[i + PREFIX.len()..];
            let name_len = rest.bytes().take_while(|b| is_word_byte(*b)).count();
            if name_len > 0 {
                let name = &rest[..name_len];
                out.push_str(&lookup(name).unwrap_or_default());
                i += PREFIX.len() + name_len;
                continue;
            }
        }
        let ch = value[i..].chars().next().unwrap_or('\u{0}');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// The host's home directory, matching Node's real `os.homedir()` resolution closely enough to
/// satisfy the REAL consumer's actual need here (`HOME` on unix, `USERPROFILE` on Windows — the SAME
/// env-var-first source Node's libuv `uv_os_homedir` consults before falling back to a passwd-database
/// lookup, a fallback this does not replicate since no realistic MCP-server-launching environment
/// leaves `HOME` unset).
fn host_home_dir() -> Option<PathBuf> {
    // THE home ladder, shared: `CYRUP_HOME` -> `HOME` -> the OS home. The `CYRUP_HOME` rung is new
    // here and is the point — this resolver read `HOME` -> `USERPROFILE` only, so a sandbox that
    // moved every other tree left `~`-expansion in this one pointing at the real home.
    cyrup_config::paths::cyrup_home_dir_from(&|key| std::env::var_os(key))
}

/// Interpolate + tilde-expand a config-supplied path — a direct Rust port of Pi's
/// `resolveConfigPath` (`pi-mcp-adapter/utils.ts:78-87`): interpolate env vars first (same
/// [`interpolate_env_vars`] pass `resolveEnv`'s overrides get), then expand a LEADING `~` (exactly
/// `"~"`, or a `~/`/`~\` prefix) against [`host_home_dir`]. A path with no leading `~` (the common
/// case) round-trips through interpolation unchanged, same as Pi's own early-return shape.
pub(crate) fn resolve_config_path(value: &str) -> PathBuf {
    resolve_config_path_with(value, |name| std::env::var(name).ok(), host_home_dir)
}

/// [`resolve_config_path`] against injected `lookup`/`home` — see [`interpolate_env_vars_with`]'s doc
/// for why tests need this rather than mutating the real process environment/home directory.
fn resolve_config_path_with(
    value: &str,
    lookup: impl Fn(&str) -> Option<String> + Copy,
    home: impl Fn() -> Option<PathBuf>,
) -> PathBuf {
    let resolved = interpolate_env_vars_with(value, lookup);
    if resolved == "~" {
        return home().unwrap_or_else(|| PathBuf::from(resolved));
    }
    if let Some(rest) = resolved
        .strip_prefix("~/")
        .or_else(|| resolved.strip_prefix("~\\"))
        && let Some(home) = home()
    {
        return home.join(rest);
    }
    PathBuf::from(resolved)
}

/// Per-pipe cap on buffered-but-unread bytes. `proc` is request/poll-shaped (a guest drains via
/// `read-stdout`/`read-stderr` only across separate top-level host calls, potentially seconds apart
/// mid-LLM-turn) — unlike the real `StdioClientTransport`, whose Node `Readable` stream bounds
/// unread bytes via its own `highWaterMark` and lets the OS pipe itself throttle the child's
/// `write()` once that fills (stdio.js:99-100/108-111, real SDK v1.25.1). A raw unbounded
/// `VecDeque` here would let host memory grow without bound for a busy guest against a bursty child
/// (confirmed unbounded-memory/DoS vector). There is no Pi-derived exact byte count to port — the
/// point is FINITE, not a specific magic number — so this is a deliberately generous cap
/// (comfortably larger than any single realistic JSON-RPC message or log burst) that still
/// guarantees bounded worst-case growth; [`spawn_pump`] parks instead of reading once a pipe hits
/// this cap, which lets the OS-level pipe buffer fill and the CHILD's own `write()` block — the
/// same real backpressure the Node stream's `highWaterMark` provides.
const MAX_PIPE_BUFFER_BYTES: usize = 16 * 1024 * 1024;

/// One `read()` worth of pipe bytes — [`spawn_pump`]'s stack chunk.
///
/// Named (rather than an inline `[0u8; 8192]`) because it is part of the buffer's real contract:
/// [`PipeBufState::wait_for_room`] parks while `len >= MAX_PIPE_BUFFER_BYTES` and unparks at
/// `len == MAX_PIPE_BUFFER_BYTES - 1`, after which the pump may append a whole further chunk. The
/// true, intended bound is therefore `MAX_PIPE_BUFFER_BYTES + PIPE_CHUNK_BYTES - 1`, not the cap
/// exactly — the guarantee is *bounded*, never *byte-exact* (see `MAX_PIPE_BUFFER_BYTES`'s own
/// "FINITE, not a specific number"). Whether a sampler observes the plateau at the cap or one chunk
/// above it is pure scheduling. EXT-N01.
const PIPE_CHUNK_BYTES: usize = 8192;

/// A byte buffer a background pump task appends to and a `read-*` call drains from the front,
/// capped at [`MAX_PIPE_BUFFER_BYTES`]. `space_freed` wakes a pump task parked at the cap the
/// instant `drain` removes bytes (Tokio `Notify` stores one permit, so a `drain` that races ahead
/// of a pump's `len` check is never missed — see `[Self::wait_for_room]`).
struct PipeBufState {
    data: Mutex<VecDeque<u8>>,
    space_freed: tokio::sync::Notify,
}

impl PipeBufState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            data: Mutex::new(VecDeque::new()),
            space_freed: tokio::sync::Notify::new(),
        })
    }

    /// Park until buffered bytes drop below the cap (immediately if already under it) OR the child
    /// process has exited — whichever happens first. Racing `exited` here closes a distinct task
    /// leak from the OS-process leak `446b858` fixed: without it, a guest that never drains a
    /// buffer that reached the cap (e.g. it kills the process, or the process's own bursty output
    /// fills the cap, and the guest simply never calls `read-stdout`/`read-stderr` again) leaves
    /// [`spawn_pump`]'s task parked HERE forever — nothing but a `drain()` ever calls `notify_one`
    /// on `space_freed`, and a dead process can never be drained into by a guest that already gave
    /// up on it. Waking on exit lets the pump proceed to its next `read()`, which — since the
    /// child's own process-exit already closed the write end of the pipe — returns `Ok(0)`/`Err`
    /// immediately, letting the pump's own loop end cleanly instead of parking forever.
    async fn wait_for_room(&self, exited: &mut watch::Receiver<Option<i32>>) {
        loop {
            let len = self.data.lock().map(|g| g.len()).unwrap_or(0);
            if len < MAX_PIPE_BUFFER_BYTES || exited.borrow().is_some() {
                return;
            }
            tokio::select! {
                () = self.space_freed.notified() => {}
                _ = exited.changed() => {}
            }
        }
    }
}

type PipeBuf = Arc<PipeBufState>;

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
/// **Kill semantics, justified.** The real `StdioClientTransport.close()` (the escalation this
/// mirrors, `@modelcontextprotocol/sdk@1.25.1` `dist/cjs/client/stdio.js:144-179`) signals ONLY
/// the immediate child — `StdioClientTransport`'s spawn (`server-manager.ts:105-112`) is a plain,
/// non-detached `child_process.spawn` (via `cross-spawn`), never a process-group leader.
/// [`ProcCaps::kill`] mirrors that 1:1: it signals the single child pid directly
/// (`cyrup_tools::terminate_pid`/`kill_pid`), NOT a `setsid`/`killpg` process-GROUP kill. Reusing
/// `cyrup-tools`' `send_sigkill_tree` (the `exec`/`bash` seam's group-kill escalation, R-03-027)
/// here would diverge from what the real consumer does for stdio MCP transport — that machinery
/// exists because a SHELL-spawned command tree needs group cleanup; a directly-`spawn`ed single MCP
/// server process does not, and killing a wider group than the real transport itself would is an
/// unjustified behavior change, not a strictly-more-correct one.
/// Accordingly `spawn` does NOT `setsid` the child either — same choice, same reasoning, as
/// `cyrup-tools::ops::local::build_argv_command` (the WASM `exec` grant's spawn, whose real
/// consumer `exec.ts:41-45` likewise never sets `detached`) — keeping the child a plain,
/// non-group-leader process exactly like `cross-spawn`'s `spawn(..., {shell:false})` with no
/// `detached` option.
pub struct ProcCaps {
    registry: Mutex<HashMap<u32, Arc<ProcEntry>>>,
    next_handle: AtomicU32,
    /// How long [`Self::kill`] waits for the child to react to EACH of its two graceful legs
    /// (stdin-EOF, then SIGTERM) before escalating — the real transport's exact 2000ms by default
    /// ([`DEFAULT_KILL_GRACE`]); overridable ONLY for tests ([`Self::with_kill_grace`]) so the
    /// SIGKILL-escalation path is exercisable without a real test waiting 2+ real seconds per leg.
    kill_grace: Duration,
    /// How long [`Self::write_stdin`] blocks on a single write before giving up — the production
    /// default is [`WRITE_STDIN_TIMEOUT`] (30s); overridable ONLY for tests
    /// ([`Self::with_write_stdin_timeout`]) so the timeout-firing path is exercisable without a
    /// real test waiting 30 real seconds.
    write_stdin_timeout: Duration,
    /// Pids of children that were spawned at the OS level but LOST the race to be inserted into
    /// `registry` (`Self::spawn`'s atomic re-check against [`MAX_SPAWNED_PROCESSES`] — a real process
    /// was already forked before the cap could be re-checked, see the doc there). Never guest-visible
    /// (that `spawn` call itself returns `Err`, so no handle for the pid was ever handed out).
    /// `Self::spawn` already makes a best-effort immediate `kill_pid` for such a pid, but that single
    /// attempt's error is unavoidably swallowed there — `kill(2)`'s own failure modes (`ESRCH`/
    /// `EPERM`) are not meaningfully retryable in a tight loop, and this synchronous rejection path
    /// has no bounded-confirmation-wait budget the way the `async` [`Self::kill`] does. Recording the
    /// pid here — instead of dropping it on the floor entirely, with zero compensating cleanup for
    /// the rest of the host process's lifetime — gives it the SAME safety net [`Drop for ProcCaps`]
    /// already provides every successfully REGISTERED child (`446b858`): a second `kill_pid` attempt
    /// at session end, bounding the worst case to "leaked for the rest of THIS session" instead of
    /// "leaked for the rest of the host process". Naturally bounded (no separate cap needed): once
    /// `registry.len()` reaches [`MAX_SPAWNED_PROCESSES`] the FAST up-front check in `Self::spawn`
    /// rejects every later call before it ever forks a real process, so this narrow reject-after-fork
    /// race can only ever be hit while the registry is crossing that threshold — a handful of times
    /// per session at most, never on every subsequent `spawn` call the way the registry itself is.
    orphaned_pids: Mutex<Vec<u32>>,
}

impl std::fmt::Debug for ProcCaps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcCaps").finish_non_exhaustive()
    }
}

impl Drop for ProcCaps {
    /// Restores the safety net [`Self::spawn`]'s `cmd.kill_on_drop(true)` comment promises but never
    /// actually delivers: that flag only fires when tokio's OWN `Child` value is dropped, but
    /// `spawn` immediately moves the `Child` into a DETACHED `tokio::spawn` waiter task (so the exit
    /// code keeps getting reaped/published even if the caller never polls again) — so `kill_on_drop`
    /// never fires when THIS `ProcCaps` (the registry of `pid`s) goes away, and any child a guest
    /// never explicitly `kill`ed leaks as a real, still-running OS process for the rest of the host
    /// process's lifetime (confirmed empirically: dropping `ProcCaps` with a live `sleep` child left
    /// its pid alive under `pgrep` well past the drop).
    ///
    /// Mirrors tokio's OWN `kill_on_drop` semantics exactly — a direct SIGKILL, no graceful
    /// escalation (`Drop::drop` cannot `.await` the multi-second grace [`Self::kill`] runs; tokio's
    /// own `ChildDropGuard::drop`, `tokio::process::Command`'s doc, does the identical direct-SIGKILL
    /// thing for the same reason) — for every entry that hasn't already published an exit code. The
    /// still-running waiter task spawned in [`Self::spawn`] is untouched by this drop and keeps
    /// running independently: it observes the SIGKILL-induced exit via its own `child.wait()` and
    /// reaps the OS process (no zombie left behind) even though nothing is left to read the exit
    /// code by that point — the entry (and this whole registry) is gone with `self`.
    fn drop(&mut self) {
        for entry in self.registry().values() {
            if entry.exit_code.borrow().is_none() {
                let _ = cyrup_tools::kill_pid(entry.pid);
            }
        }
        // Second-chance sweep for pids that lost `Self::spawn`'s atomic cap race ([`Self::orphaned_pids`]'s
        // doc) — same best-effort swallow idiom as above; whatever pid this is has no OTHER
        // compensating cleanup, since it was never accepted into `registry` in the first place.
        for pid in self
            .orphaned_pids
            .lock()
            .map(|mut g| std::mem::take(&mut *g))
            .unwrap_or_default()
        {
            let _ = cyrup_tools::kill_pid(pid);
        }
    }
}

impl Default for ProcCaps {
    fn default() -> Self {
        Self::new()
    }
}

/// Pure mapping from an [`npx_resolver::resolve_npx_binary`] result onto the `(cmd, args)`
/// [`ProcCaps::spawn`] actually launches — split out from `spawn` itself so it's unit-testable
/// without any real process/filesystem I/O. Mirrors `server-manager.ts:100-101` exactly:
/// `command = resolved.isJs ? "node" : resolved.binPath; args = resolved.isJs ? [resolved.binPath,
/// ...resolved.extraArgs] : resolved.extraArgs;` — and, for `resolved === null` (here, `None`),
/// `:97-104`'s `if (resolved) { ... }` simply never reassigns `command`/`args`, i.e. the ORIGINAL
/// guest-supplied `spec.cmd`/`spec.args` pass through verbatim.
fn apply_npx_resolution(
    resolved: Option<npx_resolver::NpxResolution>,
    spec: &ProcSpawnSpec,
) -> (String, Vec<String>) {
    match resolved {
        Some(r) if r.is_js => {
            let mut args = vec![r.bin_path];
            args.extend(r.extra_args);
            ("node".to_string(), args)
        }
        Some(r) => (r.bin_path, r.extra_args),
        None => (spec.cmd.clone(), spec.args.clone()),
    }
}

impl ProcCaps {
    pub fn new() -> Self {
        Self::with_kill_grace(DEFAULT_KILL_GRACE)
    }

    /// Build with a caller-supplied per-leg grace period (tests only; production always gets the
    /// real transport's exact 2s-per-leg via [`Self::new`]).
    pub fn with_kill_grace(kill_grace: Duration) -> Self {
        Self {
            registry: Mutex::new(HashMap::new()),
            next_handle: AtomicU32::new(1),
            kill_grace,
            write_stdin_timeout: WRITE_STDIN_TIMEOUT,
            orphaned_pids: Mutex::new(Vec::new()),
        }
    }

    /// Build with a caller-supplied [`Self::write_stdin`] timeout (tests only; production always
    /// gets the real generous default, [`WRITE_STDIN_TIMEOUT`], via [`Self::new`]).
    pub fn with_write_stdin_timeout(write_stdin_timeout: Duration) -> Self {
        Self {
            registry: Mutex::new(HashMap::new()),
            next_handle: AtomicU32::new(1),
            kill_grace: DEFAULT_KILL_GRACE,
            write_stdin_timeout,
            orphaned_pids: Mutex::new(Vec::new()),
        }
    }

    fn registry(&self) -> std::sync::MutexGuard<'_, HashMap<u32, Arc<ProcEntry>>> {
        self.registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn entry(&self, handle: u32) -> Result<Arc<ProcEntry>, String> {
        self.registry()
            .get(&handle)
            .cloned()
            .ok_or_else(|| format!("no live process for handle {handle}"))
    }

    /// Spawn a REAL long-lived child (the WIT `proc.spawn`): pipes stdin/stdout always, stderr iff
    /// `spec.capture_stderr`. Background tasks immediately start pumping the real stdout/stderr
    /// pipes into per-handle buffers, and a background waiter reaps the child + records its real
    /// exit code the instant it terminates. Mostly synchronous (`Command::spawn` is itself sync;
    /// only `tokio::spawn`, which needs a runtime context, not an async fn, to start the
    /// pump/waiter tasks) — EXCEPT for the `npx`/`npm` resolution step immediately below, which can
    /// block briefly (or, on a cold cache, up to `npx_resolver`'s 30s force-cache-population
    /// ceiling); that step runs inside `tokio::task::block_in_place` for exactly that reason.
    pub fn spawn(&self, spec: &ProcSpawnSpec) -> Result<u32, String> {
        // Reject BEFORE spending a real process spawn if already at the cap (checked again,
        // atomically with the insert, below — this is a fast up-front rejection, not the only
        // gate: two concurrent `spawn` calls can both pass this check before either inserts,
        // mirrors `HttpCaps::request_stream`'s identical two-step check, `caps/http.rs`).
        if self.registry().len() >= MAX_SPAWNED_PROCESSES {
            return Err(format!(
                "too many processes spawned via this capability ({MAX_SPAWNED_PROCESSES} already \
                 in the registry, killed or not) — this grant does not evict entries"
            ));
        }
        // Mirrors `server-manager.ts:97-104`: an `npx`/`npm`-shaped invocation is resolved down to
        // the REAL underlying binary BEFORE the real child is ever spawned, so the pid this
        // registry tracks (and `kill`, below, can actually signal) is the real MCP server, not a
        // transient `npm`/`npx` launcher whose own real child can otherwise survive `kill` as an
        // orphan (see `npx_resolver`'s module doc for the full citation). Gated on the exact
        // command name so the overwhelmingly common non-npx case never pays for the
        // `block_in_place` + resolution-module call at all.
        let resolved = if spec.cmd == "npx" || spec.cmd == "npm" {
            tokio::task::block_in_place(|| npx_resolver::resolve_npx_binary(&spec.cmd, &spec.args))
        } else {
            None
        };
        let (resolved_cmd, resolved_args) = apply_npx_resolution(resolved, spec);
        let mut cmd = tokio::process::Command::new(&resolved_cmd);
        cmd.args(&resolved_args);
        if let Some(cwd) = &spec.cwd {
            // `cwd` is used VERBATIM — NO interpolation/tilde-expansion here. It must already be
            // fully resolved by the caller: see [`ProcSpawnSpec`]'s doc for why (a host-injected
            // trusted default, with no Pi equivalent, shares this field with genuine guest/config
            // values, and only the latter should ever be run through Pi's `resolveConfigPath`).
            cmd.current_dir(cwd);
        }
        for (k, v) in &spec.env {
            // Pi `resolveEnv`/`interpolateEnvRecord` (server-manager.ts:422-435, utils.ts:68-76):
            // interpolate `${VAR}`/`$env:VAR` in each VALUE — keys are never interpolated.
            cmd.env(k, interpolate_env_vars(v));
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

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("spawn {resolved_cmd}: {e}"))?;
        let pid = child
            .id()
            .ok_or_else(|| format!("spawn {resolved_cmd}: no pid assigned"))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let stdout_buf: PipeBuf = PipeBufState::new();
        let stderr_buf: PipeBuf = PipeBufState::new();

        // Created BEFORE the pumps spawn so each gets its own clone of the SAME watch to race
        // against in `wait_for_room` (see `spawn_pump`'s doc) — `poll_exit`/`kill` read the ORIGINAL
        // `exit_rx` via the `watch` receiver stored on the entry below, never blocking on the
        // process (or the child's own stdio) themselves.
        let (exit_tx, exit_rx) = watch::channel(None);

        // Built BEFORE the pump/waiter tasks spawn (rather than after, as before) so the waiter task
        // below can hold its own `Arc` clone and close `entry.stdin` the instant it reaps a NATURAL
        // exit — without this, a child that is never explicitly `kill`ed (the common case: most
        // stdio-loop MCP servers just run until the guest stops talking to them, never a guest-
        // initiated teardown) leaked its real `ChildStdin` write-end fd for the rest of the session,
        // since neither `write_stdin`'s error path nor `kill`'s phase-1 close ever ran for it.
        let entry = Arc::new(ProcEntry {
            pid,
            stdin: AsyncMutex::new(stdin),
            stdout_buf: stdout_buf.clone(),
            stderr_buf: stderr_buf.clone(),
            exit_code: exit_rx.clone(),
        });

        if let Some(out) = stdout {
            spawn_pump(out, stdout_buf, exit_rx.clone());
        }
        if let Some(err) = stderr {
            spawn_pump(err, stderr_buf, exit_rx.clone());
        }

        // Reap the child + publish its REAL exit code the instant it terminates (natural exit, or a
        // signal `kill` sent) — and close the stdin write end too (see the comment on `entry` above):
        // this is the ONLY place a naturally-exiting child's stdin ever gets closed, since `kill`'s
        // own phase-1 close only runs for a guest-initiated teardown.
        let entry_for_waiter = entry.clone();
        tokio::spawn(async move {
            let code = match child.wait().await {
                Ok(status) => status.code().unwrap_or(0),
                Err(_) => -1,
            };
            *entry_for_waiter.stdin.lock().await = None;
            let _ = exit_tx.send(Some(code));
        });

        let handle = self.next_handle.fetch_add(1, Ordering::Relaxed);
        {
            let mut reg = self.registry();
            // Re-checked atomically with the insert (the up-front check above is a fast-path
            // only — a concurrent `spawn` could have raced past it in between, see the doc on
            // that check): a real process was already spawned above, so reject-after-the-fact
            // means killing it directly (`kill_pid`, not the graceful `Self::kill` escalation —
            // this entry was never accepted into the registry, so the guest has no handle to
            // negotiate a graceful shutdown with) rather than leaking it. The pump/waiter tasks
            // spawned above still end cleanly on their own once the process dies (`spawn_pump`
            // reads EOF, the waiter reaps and publishes to an `exit_tx` nobody reads) — nothing
            // here needs to cancel them explicitly.
            if reg.len() >= MAX_SPAWNED_PROCESSES {
                drop(reg);
                let _ = cyrup_tools::kill_pid(pid);
                // A failed kill above has NO other compensating cleanup — this pid never made it
                // into `registry`, so `Drop for ProcCaps` (which only sweeps registered entries)
                // would otherwise never see it. Record it regardless of whether the immediate
                // attempt reported success (a harmless no-op re-kill of an already-dead pid if it
                // did) so `Drop` gets a second chance at session end ([`Self::orphaned_pids`]'s doc).
                if let Ok(mut g) = self.orphaned_pids.lock() {
                    g.push(pid);
                }
                return Err(format!(
                    "too many processes spawned via this capability ({MAX_SPAWNED_PROCESSES} \
                     already in the registry, killed or not) — this grant does not evict entries"
                ));
            }
            reg.insert(handle, entry);
        }
        Ok(handle)
    }

    /// Write to the child's REAL stdin (the WIT `proc.write-stdin`). `Err` once the pipe is closed
    /// (child exited / closed stdin) — mirrors a real broken-pipe write failure, never a panic.
    /// Bounded by [`WRITE_STDIN_TIMEOUT`] (its doc explains why no Pi-derived exact value exists):
    /// a child that never reads its stdin (or reads too slowly to keep up) gets a bounded `Err`
    /// instead of hanging this call — and the real tokio worker thread backing it — forever. The
    /// bound covers BOTH acquiring the per-handle stdin lock AND the write itself (one outer
    /// `tokio::time::timeout` around both, [`WRITE_STDIN_TIMEOUT`]'s doc) — a version that only timed
    /// the write let a SECOND concurrent `write_stdin` against the same handle wait unboundedly for
    /// the first's lock, then get its own full timeout budget on top, compounding to ~2x the
    /// documented ceiling.
    pub async fn write_stdin(&self, handle: u32, data: &[u8]) -> Result<u32, String> {
        let entry = self.entry(handle)?;
        let attempt = tokio::time::timeout(self.write_stdin_timeout, async {
            let mut guard = entry.stdin.lock().await;
            let Some(stdin) = guard.as_mut() else {
                return Err(None); // Sentinel: pipe already closed — not a real io error.
            };
            match stdin.write_all(data).await {
                Ok(()) => Ok(()),
                Err(e) => {
                    // A closed/broken pipe is terminal — drop the handle so future writes fail fast
                    // with the SAME message instead of a fresh (possibly different) io error each
                    // time.
                    *guard = None;
                    Err(Some(e))
                }
            }
        })
        .await;
        match attempt {
            Ok(Ok(())) => Ok(u32::try_from(data.len()).unwrap_or(u32::MAX)),
            Ok(Err(None)) => Err(format!("stdin is closed for handle {handle}")),
            Ok(Err(Some(e))) => Err(format!("write_stdin: {e}")),
            Err(_) => {
                // Timed out either waiting for the lock or with the write still in flight:
                // `write_all` may have already flushed SOME of `data` to the real pipe before the
                // deadline (an inherent, unavoidable ambiguity of bounding a partial-write future —
                // the SAME ambiguity a socket-write timeout has everywhere else). The stdin handle
                // itself is left open regardless — a slow-but-alive child may still finish draining
                // the pipe and read the bytes that did land, or a concurrent write holding the lock
                // may still complete; only a REAL io error is treated as terminal.
                Err(format!(
                    "write_stdin: timed out after {:?} acquiring the stdin lock or writing to \
                     handle {handle} (the child may not be reading its stdin, or a concurrent write \
                     is still in flight)",
                    self.write_stdin_timeout
                ))
            }
        }
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
        let out = {
            let mut g = buf
                .data
                .lock()
                .map_err(|_| "proc pipe buffer lock poisoned".to_string())?;
            let n = (max_bytes as usize).min(g.len());
            g.drain(..n).collect::<Vec<u8>>()
        };
        // Wake a pump task parked at MAX_PIPE_BUFFER_BYTES regardless of whether this drain freed
        // any bytes — a spurious wake just re-checks `len` and re-parks if still full, harmless.
        buf.space_freed.notify_one();
        Ok(out)
    }

    /// Poll whether the child has exited (the WIT `proc.poll-exit`); `Some(code)` once terminated,
    /// `None` while still running OR for an unknown handle (the WIT signature carries no error
    /// channel — an unknown handle degrades to "not exited" rather than panicking/erroring).
    pub fn poll_exit(&self, handle: u32) -> Option<i32> {
        let entry = self.registry().get(&handle).cloned()?;
        *entry.exit_code.borrow()
    }

    /// Terminate the child (the WIT `proc.kill`): close stdin (EOF), then SIGTERM, then SIGKILL,
    /// each escalation gated on the child still being alive after the grace period — the real
    /// `StdioClientTransport.close()`'s exact three-phase escalation (`@modelcontextprotocol/sdk
    /// @1.25.1` `dist/cjs/client/stdio.js:144-179`), NOT Pi `exec.ts:52-63`'s unrelated bounded
    /// one-shot tool-run kill (see [`DEFAULT_KILL_GRACE`]'s doc for why that citation was wrong).
    /// Returns `Ok` only once the OS process is CONFIRMED gone (the waiter task observed real
    /// termination), never a fire-and-forget signal send. Idempotent: killing an already-exited
    /// handle is a no-op `Ok`.
    pub async fn kill(&self, handle: u32) -> Result<(), String> {
        let entry = self.entry(handle)?;
        if entry.exit_code.borrow().is_some() {
            return Ok(()); // already exited — never re-signal a reaped pid.
        }

        // Phase 1 — graceful stdin EOF (stdio.js:154's `processToClose.stdin?.end()`): many
        // stdio-loop MCP servers `read()` until EOF then exit cleanly on their own, needing no
        // signal at all. Dropping (never re-storing) the `ChildStdin` closes the real underlying
        // fd — the same effect as Node's `stdin.end()`.
        //
        // `try_lock`, NOT `.lock().await`: Node's `stream.end()` (the real `stdin?.end()` this
        // ports) is itself non-blocking — it returns immediately regardless of any buffered/
        // in-flight write, structurally independent of `send()`/`write()`
        // (`@modelcontextprotocol/sdk@1.25.1` `dist/cjs/client/stdio.js:144-179` vs `189-207`).
        // A blocking `.lock().await` here would instead let a concurrent [`Self::write_stdin`] —
        // which holds this SAME mutex for up to [`WRITE_STDIN_TIMEOUT`] (30s) against a slow/non-
        // reading child — stall `kill`'s entire escalation by up to 30s on top of its own ~6s
        // budget (`host/live.rs`'s epoch-forgiveness doc), a real, reachable divergence given Pi's
        // own parallel tool-execution model (`types.ts:455-462`). If the lock is busy, hand the
        // close off to a detached task that finishes it once the writer releases the mutex — kill
        // proceeds immediately to phase 2 instead of waiting on it (see the `stdin_closed` doc just
        // below for why skipping the grace-wait here is still correct).
        let stdin_closed = match entry.stdin.try_lock() {
            Ok(mut guard) => {
                *guard = None;
                true
            }
            Err(_) => {
                let entry_bg = entry.clone();
                tokio::spawn(async move {
                    *entry_bg.stdin.lock().await = None;
                });
                false
            }
        };
        // Only wait out the grace period when stdin was ACTUALLY closed just now — if a concurrent
        // write held the lock, stdin is still open (the background task above hasn't run yet), so a
        // grace-period wait here could only ever catch an unrelated natural exit, never a genuine
        // EOF-driven one; skip straight to SIGTERM instead of paying a needless `kill_grace` delay.
        if stdin_closed && Self::wait_exited(&entry, self.kill_grace).await {
            return Ok(()); // exited on stdin EOF alone — no signal needed (stdio.js:159-160).
        }

        // Phase 2 — SIGTERM (stdio.js:162), same 2000ms-real grace (stdio.js:167). On non-unix,
        // `terminate_pid` is a best-effort no-op (no portable single-pid graceful-signal primitive
        // there) and reports `Ok(false)` — skip the grace-period wait entirely rather than paying
        // a needless ~2s delay for a signal that was genuinely never sent.
        //
        // A SEND failure (most commonly `ESRCH`: the process already exited in the race between our
        // own `entry.exit_code` check above and this syscall — the OS process can die at any instant
        // between the two) must NOT abort the escalation: the real transport wraps this exact call in
        // try/catch-ignore (`stdio.js:162-166`: `try { kill('SIGTERM') } catch { /* ignore */ }`).
        // Treat any error identically to `Ok(false)` (nothing confirmed sent) — matches the existing
        // `Drop for ProcCaps`'s own `let _ = kill_pid(...)` idiom just below.
        let sigterm_sent = cyrup_tools::terminate_pid(entry.pid).unwrap_or(false);
        if sigterm_sent && Self::wait_exited(&entry, self.kill_grace).await {
            return Ok(()); // SIGTERM worked within the grace period — no further escalation needed.
        }

        // Phase 3 — SIGKILL (stdio.js:171). The real transport fires-and-forgets this and returns
        // immediately; this capability's own `Ok`-means-confirmed-gone contract (doc above) instead
        // waits out a bounded confirmation ([`KILL_CONFIRM_TIMEOUT`]'s doc). Same rationale as phase
        // 2: the real transport ignores a SIGKILL send failure too (`stdio.js:169-174`), most
        // commonly `ESRCH` if SIGTERM (or the process's own natural exit) already reaped it in the
        // interim. The `wait_exited` call below — not this send — is what actually confirms
        // termination; a failed SEND must not itself be treated as failed confirmation.
        let _ = cyrup_tools::kill_pid(entry.pid);

        if Self::wait_exited(&entry, KILL_CONFIRM_TIMEOUT).await {
            Ok(())
        } else {
            Err(format!(
                "process {} did not terminate after SIGKILL",
                entry.pid
            ))
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
/// that keeps `read-stdout`/`read-stderr` non-lossy between polls. `exited` is a clone of the SAME
/// watch the process waiter task publishes to — passed through to [`PipeBufState::wait_for_room`]
/// so a buffer parked at the cap wakes up (and this task ends) once the process is gone, instead of
/// staying parked forever waiting for a `drain()` a guest that gave up on this handle will never
/// call again (see `wait_for_room`'s doc). Returns the task's `JoinHandle` (unused by the production
/// caller, which is deliberately fire-and-forget — the SAME reason `Self::spawn`'s process-waiter
/// task isn't joined either — but lets tests observe the task genuinely completing).
fn spawn_pump<R>(
    mut reader: R,
    buf: PipeBuf,
    mut exited: watch::Receiver<Option<i32>>,
) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut chunk = [0u8; PIPE_CHUNK_BYTES];
        loop {
            // Backpressure: park here (never reading the OS pipe) once the buffer is at the cap.
            // This is what makes the cap a REAL bound rather than a drop-newest/drop-oldest hack —
            // the kernel pipe buffer fills and the CHILD's own `write()` blocks, exactly the
            // pressure a real Node `Readable` stream's `highWaterMark` applies (see
            // [`MAX_PIPE_BUFFER_BYTES`]'s doc).
            buf.wait_for_room(&mut exited).await;
            match reader.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Ok(mut g) = buf.data.lock() {
                        g.extend(chunk.get(..n).unwrap_or(&[]).iter().copied());
                    }
                }
            }
        }
    })
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

    /// A fixed, hermetic lookup (never touches the real process environment — `cyrup-ext` is
    /// `#![forbid(unsafe_code)]`, and edition 2024 makes `std::env::set_var` `unsafe fn`, so tests
    /// cannot mutate real env vars at all).
    fn fixed_lookup(
        pairs: &'static [(&'static str, &'static str)],
    ) -> impl Fn(&str) -> Option<String> + Copy {
        move |name: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_string())
        }
    }

    /// THE MEDIUM finding this closes: `interpolate_env_vars` must handle BOTH placeholder forms Pi's
    /// `interpolateEnvVars` supports (`${VAR}` and `$env:VAR`), resolve an unset variable to the empty
    /// string (never an error/panic), leave malformed placeholders untouched, and apply the two forms
    /// in Pi's exact SEQUENTIAL order (`${VAR}` fully resolved first, `$env:VAR` resolved second,
    /// against the ALREADY-`${}`-resolved string) — proven by a value whose `${...}` substitution
    /// itself yields a literal `$env:...` that must then ALSO be resolved.
    #[test]
    fn interpolate_env_vars_resolves_both_placeholder_forms_in_pis_exact_order() {
        let lookup = fixed_lookup(&[
            ("FOO", "foo-value"),
            ("BAR", "$env:BAZ"),
            ("BAZ", "baz-value"),
        ]);

        assert_eq!(interpolate_env_vars_with("${FOO}", lookup), "foo-value");
        assert_eq!(interpolate_env_vars_with("$env:FOO", lookup), "foo-value");
        assert_eq!(
            interpolate_env_vars_with("prefix-${FOO}-mid-$env:BAZ-suffix", lookup),
            "prefix-foo-value-mid-baz-value-suffix"
        );
        // Unset variable ⇒ empty string, never an error.
        assert_eq!(interpolate_env_vars_with("${MISSING}", lookup), "");
        assert_eq!(interpolate_env_vars_with("$env:MISSING", lookup), "");
        // Malformed placeholders are left byte-for-byte untouched (no matching `}`, empty name).
        assert_eq!(interpolate_env_vars_with("${FOO", lookup), "${FOO");
        assert_eq!(interpolate_env_vars_with("${}", lookup), "${}");
        assert_eq!(
            interpolate_env_vars_with("plain text, no placeholders", lookup),
            "plain text, no placeholders"
        );
        // Two-pass order: `${BAR}` resolves to the LITERAL string `$env:BAZ` in pass one, which pass
        // two then ALSO resolves — proving the passes are sequential over the whole string, not a
        // single combined scan (mirrors Pi's chained `.replace(...).replace(...)`).
        assert_eq!(
            interpolate_env_vars_with("${BAR}", lookup),
            "baz-value",
            "a value produced by the ${{}} pass must still be resolved by the subsequent $env: pass"
        );
    }

    /// THE MEDIUM finding this closes, the `cwd` half: `resolve_config_path` must interpolate THEN
    /// tilde-expand — a bare `~`, a `~/rest` prefix, and a `~\rest` prefix (Windows-style) all resolve
    /// against the injected home directory; a path with no leading `~` only gets interpolated.
    #[test]
    fn resolve_config_path_interpolates_then_tilde_expands() {
        let lookup = fixed_lookup(&[("PROJECT", "my-project")]);
        let home = || Some(PathBuf::from("/home/testuser"));

        assert_eq!(
            resolve_config_path_with("~", lookup, home),
            PathBuf::from("/home/testuser")
        );
        assert_eq!(
            resolve_config_path_with("~/${PROJECT}/servers", lookup, home),
            PathBuf::from("/home/testuser/my-project/servers")
        );
        assert_eq!(
            resolve_config_path_with("~\\${PROJECT}\\servers", lookup, home),
            PathBuf::from("/home/testuser/my-project\\servers"),
            "a Windows-style ~\\ prefix is also expanded"
        );
        // No leading `~` at all ⇒ interpolated but otherwise untouched (no tilde expansion applied).
        assert_eq!(
            resolve_config_path_with("/abs/${PROJECT}/path", lookup, home),
            PathBuf::from("/abs/my-project/path")
        );
        // No home available ⇒ the (interpolated) `~`-prefixed string passes through verbatim rather
        // than panicking or silently dropping the `~`.
        assert_eq!(
            resolve_config_path_with("~/x", lookup, || None),
            PathBuf::from("~/x"),
            "no home available must degrade gracefully, not panic"
        );
    }

    /// `apply_npx_resolution`'s three arms, matching `server-manager.ts:100-101` 1:1 — the finding-1
    /// fix (`proc.rs:461` before this fix: `spec.cmd` used verbatim with no npx/npm interception).
    #[test]
    fn apply_npx_resolution_matches_pi_exactly() {
        let original = spec("npx", &["-y", "@foo/bar"]);

        // resolved.isJs ⇒ command becomes "node", args become [binPath, ...extraArgs].
        let js_resolution = npx_resolver::NpxResolution {
            bin_path: "/cache/_npx/abc/node_modules/@foo/bar/cli.js".to_string(),
            extra_args: vec!["--flag".to_string()],
            is_js: true,
        };
        assert_eq!(
            apply_npx_resolution(Some(js_resolution), &original),
            (
                "node".to_string(),
                vec![
                    "/cache/_npx/abc/node_modules/@foo/bar/cli.js".to_string(),
                    "--flag".to_string()
                ]
            )
        );

        // !resolved.isJs (a native/shebang-less binary, or a non-`node` shebang) ⇒ command becomes
        // the resolved binPath directly, args become extraArgs verbatim (no `node` prepended).
        let native_resolution = npx_resolver::NpxResolution {
            bin_path: "/cache/_npx/abc/node_modules/.bin/foo-bar".to_string(),
            extra_args: vec!["--flag".to_string()],
            is_js: false,
        };
        assert_eq!(
            apply_npx_resolution(Some(native_resolution), &original),
            (
                "/cache/_npx/abc/node_modules/.bin/foo-bar".to_string(),
                vec!["--flag".to_string()]
            )
        );

        // resolved === null (here, None) ⇒ `command`/`args` pass through UNCHANGED — the original
        // guest-supplied `spec.cmd`/`spec.args`, not touched at all.
        assert_eq!(
            apply_npx_resolution(None, &original),
            (
                "npx".to_string(),
                vec!["-y".to_string(), "@foo/bar".to_string()]
            )
        );
    }

    /// Live end-to-end proof that [`ProcCaps::spawn`] actually RUNS `npx`/`npm` resolution (not
    /// just that the pure mapping above is correct in isolation): `npx --version` is a real,
    /// network-free, near-instant invocation whose args (`["--version"]`) `parse_npx_args` rejects
    /// (`--version` isn't `-y`/`--yes`/`-p`/`--package`/`--package=` and starts with `-`,
    /// `npx-resolver.ts:103-105`) — so `resolve_npx_binary` returns `None` immediately with ZERO
    /// filesystem/subprocess work, and `spawn` must fall through to launching the REAL `npx` off
    /// `PATH` with the ORIGINAL args verbatim, exactly like it did before this fix existed. Proves
    /// the new gate+`block_in_place`+resolution wiring in `spawn` doesn't corrupt the ordinary
    /// pass-through case. Skips (not fails) on a host with no `npx` on `PATH`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_falls_through_to_real_npx_when_resolution_declines() {
        let caps = ProcCaps::new();
        let Ok(handle) = caps.spawn(&spec("npx", &["--version"])) else {
            eprintln!("skipping: no `npx` on PATH in this environment");
            return;
        };
        let code = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if let Some(code) = caps.poll_exit(handle) {
                    return code;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("npx --version exits promptly, no network/install involved");
        assert_eq!(code, 0, "a real `npx --version` must exit 0");
    }

    /// A real `cat` is a genuine duplex pipe: write a line, poll stdout across MULTIPLE calls until
    /// the real echoed bytes show up (proving a live pipe, not a one-shot capture), then again for a
    /// second line — proving the SAME handle stays open and keeps echoing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_is_a_real_live_duplex_pipe_across_multiple_polls() {
        let caps = ProcCaps::new();
        let handle = caps.spawn(&spec("cat", &[])).expect("cat spawns");

        caps.write_stdin(handle, b"first\n")
            .await
            .expect("write first line");
        let got = poll_until_contains(&caps, handle, b"first\n").await;
        assert!(
            got,
            "the real cat echoed the first line back across multiple polls"
        );

        caps.write_stdin(handle, b"second\n")
            .await
            .expect("write second line on the SAME handle");
        let got = poll_until_contains(&caps, handle, b"second\n").await;
        assert!(
            got,
            "the SAME live pipe kept echoing after the first round-trip"
        );

        assert_eq!(
            caps.poll_exit(handle),
            None,
            "cat is still running (no EOF sent yet)"
        );

        caps.kill(handle)
            .await
            .expect("kill terminates the real cat");
        assert!(
            caps.poll_exit(handle).is_some(),
            "poll_exit reports the real exit after kill"
        );
    }

    /// Poll `read_stdout` repeatedly (never blocking) until the accumulated bytes contain `needle`
    /// or a generous deadline elapses.
    async fn poll_until_contains(caps: &ProcCaps, handle: u32, needle: &[u8]) -> bool {
        let mut acc = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            let chunk = caps
                .read_stdout(handle, 4096)
                .expect("read_stdout on a live handle");
            acc.extend_from_slice(&chunk);
            if acc.windows(needle.len()).any(|w| w == needle) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    /// End-to-end live proof (not just the unit-level `interpolate_env_vars_with` above): `spawn`'s
    /// `env` values are interpolated against the REAL host environment before reaching the real
    /// child. `HOME` is a variable this test can read but never needs to SET (`cyrup-ext` cannot
    /// mutate real env vars — see `fixed_lookup`'s doc) — a genuinely ambient one is guaranteed
    /// present on any host capable of running an MCP server child at all.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_interpolates_env_values_against_the_real_host_environment() {
        let real_home = std::env::var("HOME").expect("HOME is set in the test environment");
        let caps = ProcCaps::new();
        let mut s = spec("sh", &["-c", "printenv MY_GREETING"]);
        s.env = vec![(
            "MY_GREETING".to_string(),
            "hello-${HOME}-and-$env:HOME-again".to_string(),
        )];
        let handle = caps.spawn(&s).expect("sh spawns");

        // Wait for the real natural exit, then drain the real stdout it printed.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while caps.poll_exit(handle).is_none() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let out = caps
            .read_stdout(handle, 4096)
            .expect("read_stdout after exit");
        let printed = String::from_utf8_lossy(&out);
        let expected = format!("hello-{real_home}-and-{real_home}-again\n");
        assert_eq!(
            printed, expected,
            "the REAL child must see the REAL host $HOME substituted into both placeholder forms"
        );
    }

    /// End-to-end live proof for the `cwd` half of the split described on [`ProcSpawnSpec`]:
    /// `spawn` no longer does ANY tilde-expansion itself — [`resolve_config_path`] (the same
    /// function the real `host/live.rs` WIT boundary now calls on a raw guest string before ever
    /// building a [`ProcSpawnSpec`]) is applied HERE, by the test, to simulate that boundary; `spawn`
    /// is then proven to carry the ALREADY-resolved real host home directory through to a real child
    /// verbatim, with a `pwd` run inside it confirming the real `$HOME`.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_honors_an_already_resolved_tilde_cwd_verbatim() {
        let real_home = std::fs::canonicalize(std::env::var("HOME").expect("HOME is set"))
            .expect("HOME resolves to a real, canonical directory");
        let caps = ProcCaps::new();
        let mut s = spec("pwd", &[]);
        s.cwd = Some(resolve_config_path("~"));
        let handle = caps
            .spawn(&s)
            .expect("pwd spawns with an already-resolved ~ cwd");

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while caps.poll_exit(handle).is_none() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let out = caps
            .read_stdout(handle, 4096)
            .expect("read_stdout after exit");
        let printed = std::fs::canonicalize(String::from_utf8_lossy(&out).trim_end())
            .expect("pwd's real stdout is a real, canonical directory");
        assert_eq!(
            printed, real_home,
            "a bare `~` cwd must resolve to the REAL host home directory"
        );
    }

    /// `poll_exit` is `None` while genuinely running and `Some(code)` with the REAL exit code once
    /// the child exits on its own (no `kill` involved) — proving the background waiter observes a
    /// natural exit, not just a `kill`-driven one.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn poll_exit_reports_still_running_then_the_real_natural_exit_code() {
        let caps = ProcCaps::new();
        let handle = caps
            .spawn(&spec("sh", &["-c", "sleep 0.2; exit 7"]))
            .expect("sh spawns");
        assert_eq!(
            caps.poll_exit(handle),
            None,
            "still running immediately after spawn"
        );

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut code = None;
        while tokio::time::Instant::now() < deadline {
            if let Some(c) = caps.poll_exit(handle) {
                code = Some(c);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            code,
            Some(7),
            "the REAL natural exit code round-trips (no kill involved)"
        );
    }

    /// THE regression this closes: a child that exits ON ITS OWN (never explicitly `kill`ed — the
    /// common case for a well-behaved MCP stdio server the guest simply stops talking to) must still
    /// have its stdin write-end fd closed by the background waiter, exactly like `kill`'s own phase-1
    /// close does for a guest-initiated teardown. Pre-fix, `entry.stdin` was only ever cleared by
    /// `write_stdin`'s failure path or `kill`'s phase-1 — neither of which a naturally-exiting,
    /// never-`kill`ed, never-written-to child ever triggers — leaking one open `ChildStdin` fd to a
    /// dead process for the rest of the session.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn natural_exit_closes_the_stdin_write_end_no_fd_leak() {
        let caps = ProcCaps::new();
        let handle = caps
            .spawn(&spec("sh", &["-c", "exit 0"]))
            .expect("sh spawns");

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline && caps.poll_exit(handle).is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            caps.poll_exit(handle),
            Some(0),
            "the child really did exit naturally"
        );

        // No extra wait needed: the waiter task closes `entry.stdin` BEFORE it sends on `exit_tx`
        // (program order, same task — see `Self::spawn`'s doc), so `poll_exit` observing the exit
        // code above already proves the stdin close happened.
        let entry = caps
            .entry(handle)
            .expect("entry still present after natural exit");
        assert!(
            entry.stdin.lock().await.is_none(),
            "a naturally-exited child's stdin write-end must be closed, not left open to a dead \
             process for the rest of the session"
        );
    }

    /// `kill` on a NORMAL (SIGTERM-obeying) child terminates it well within the grace period, and
    /// the OS process is verifiably gone afterward (not merely a successful WIT-level return) —
    /// checked via `kill -0`, which fails once the pid no longer exists.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn kill_terminates_a_real_running_child_and_the_os_process_is_gone() {
        let caps = ProcCaps::new();
        let handle = caps.spawn(&spec("sleep", &["30"])).expect("sleep spawns");
        assert_eq!(
            caps.poll_exit(handle),
            None,
            "sleep is genuinely still running"
        );

        let started = tokio::time::Instant::now();
        caps.kill(handle)
            .await
            .expect("kill terminates the real sleep");
        assert!(
            // `sleep` never reads stdin, so phase 1 (stdin EOF) always eats its full REAL
            // production grace period (~2s, DEFAULT_KILL_GRACE) with no reaction; `sleep` IS
            // SIGTERM-obeying though, so phase 2 kills it near-instantly — well under a THIRD
            // grace period's worth of extra time, which is what a SIGKILL escalation would cost.
            started.elapsed() < Duration::from_secs(3),
            "a SIGTERM-obeying child (after the futile stdin-EOF leg) dies well within the second \
             grace period, no SIGKILL escalation needed: elapsed {:?}",
            started.elapsed()
        );
        assert!(
            caps.poll_exit(handle).is_some(),
            "poll_exit reflects the real termination"
        );

        // Independently verify at the OS level (never just trust our own accounting): the pid must
        // no longer exist. `kill -0` sends no signal; it only checks existence/permission.
        let pid = {
            let reg = caps.registry();
            reg.get(&handle)
                .expect("entry still present after kill")
                .pid
        };
        let alive = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status();
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
        // A short grace period so this test stays fast — the escalation TIMING is the real
        // transport's exact 2000ms-per-leg in production ([`DEFAULT_KILL_GRACE`]); only the test
        // injects a shorter one. This shell also never reads stdin, so phase 1 (stdin EOF) always
        // times out too — the full escalation now pays out TWO grace-period waits (stdin-EOF, then
        // ignored-SIGTERM) before SIGKILL, not one.
        let caps = ProcCaps::with_kill_grace(Duration::from_millis(150));
        let handle = caps
            .spawn(&spec(
                "sh",
                &["-c", "trap '' TERM; while true; do sleep 1; done"],
            ))
            .expect("the SIGTERM-ignoring shell spawns");
        assert_eq!(
            caps.poll_exit(handle),
            None,
            "genuinely running before kill"
        );
        // Let the shell actually REACH `trap '' TERM` before we signal it — otherwise SIGTERM can
        // race the trap install and kill it via the (still-default) terminate disposition, which
        // would falsely "pass" this test without ever exercising the SIGKILL escalation. The
        // mandatory phase-1 stdin-EOF wait (150ms) already provides this margin on its own, but
        // this sleep keeps the test robust even if phase 1 is ever removed/shortened further.
        tokio::time::sleep(Duration::from_millis(100)).await;

        let pid = {
            let reg = caps.registry();
            reg.get(&handle).expect("entry present").pid
        };

        let started = tokio::time::Instant::now();
        caps.kill(handle)
            .await
            .expect("kill still terminates it via the SIGKILL escalation");
        assert!(
            // Two full grace-period legs (stdin-EOF, then ignored-SIGTERM) genuinely elapse before
            // the SIGKILL escalation — not just one.
            started.elapsed() >= Duration::from_millis(150) * 2,
            "both grace-period legs were genuinely waited out before escalating (elapsed {:?})",
            started.elapsed()
        );
        assert!(
            caps.poll_exit(handle).is_some(),
            "poll_exit reflects the SIGKILL-forced termination"
        );

        let alive = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status();
        assert!(
            alive.map(|s| !s.success()).unwrap_or(true),
            "kill -0 on the SIGKILLed pid must fail — the OS process is really gone, SIGTERM alone \
             would have left it running"
        );
    }

    /// The graceful stdin-EOF-alone leg (fixes the raw audit's confirmed Finding 1: `kill` used to
    /// go straight to SIGTERM with no graceful phase at all). `cat`'s only exit condition is EOF on
    /// its stdin — closing it must let `cat` exit ON ITS OWN, with `kill` returning `Ok` in well
    /// under a single grace period. This is airtight by construction, not just by timing: SIGTERM
    /// is unreachable code in [`ProcCaps::kill`] until AFTER phase 1's `wait_exited` call returns
    /// `false`, so a fast `Ok` here can ONLY have come from the phase-1 early return — no signal
    /// was sent.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn kill_exits_a_child_via_graceful_stdin_eof_alone_no_signal_needed() {
        let caps = ProcCaps::new(); // real production grace (2s/leg) — proves this returns FAST.
        let handle = caps
            .spawn(&spec("sh", &["-c", "cat >/dev/null"]))
            .expect("spawns");
        assert_eq!(
            caps.poll_exit(handle),
            None,
            "genuinely running before kill"
        );

        let pid = {
            let reg = caps.registry();
            reg.get(&handle).expect("entry present").pid
        };

        let started = tokio::time::Instant::now();
        caps.kill(handle)
            .await
            .expect("kill terminates via graceful stdin EOF alone");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "a stdin-EOF-driven exit must be near-instant (well under the 2s production grace \
             period) — a slow return here would mean the graceful phase 1 path was NOT taken: \
             elapsed {:?}",
            started.elapsed()
        );
        assert!(
            caps.poll_exit(handle).is_some(),
            "poll_exit reflects the real termination"
        );

        let alive = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status();
        assert!(
            alive.map(|s| !s.success()).unwrap_or(true),
            "kill -0 on the EOF-exited pid must fail — the OS process is really gone"
        );
    }

    /// Establishes the PREMISE of the ESRCH-on-natural-exit-race fix: `cyrup_tools::terminate_pid`/
    /// `kill_pid` DO return a real `Err` when signaling a pid that's already gone — exactly the
    /// class of error `ProcCaps::kill`'s phases 2/3 used to propagate via `?` (turning a successful
    /// kill into a hard `Err` whenever the target process happened to exit in the race between
    /// `kill`'s own liveness check and the signal syscall). Deterministic (no timing race needed):
    /// spawns a real child, waits for it to genuinely exit and be reaped (confirmed via `kill -0`
    /// failing), THEN signals the now-dead pid directly.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminate_pid_and_kill_pid_report_err_for_an_already_reaped_pid() {
        let mut child = tokio::process::Command::new("true")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("`true` spawns");
        let pid = child.id().expect("spawned child has a pid");
        child
            .wait()
            .await
            .expect("`true` exits and is reaped almost immediately");

        let alive = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status();
        assert!(
            alive.map(|s| !s.success()).unwrap_or(true),
            "the pid must be genuinely gone before this test proceeds"
        );

        let sigterm_err = cyrup_tools::terminate_pid(pid);
        assert!(
            sigterm_err.is_err(),
            "signaling an already-reaped pid must report a real error (ESRCH), got {sigterm_err:?}"
        );
        let sigkill_err = cyrup_tools::kill_pid(pid);
        assert!(
            sigkill_err.is_err(),
            "SIGKILLing an already-reaped pid must report a real error (ESRCH), got {sigkill_err:?}"
        );
    }

    /// `ProcCaps::kill` must NEVER surface the ESRCH-class error the previous test proves is real:
    /// this is a structural (not timing-based) proof — `kill`'s phases 2/3 no longer have a `?`
    /// after `terminate_pid`/`kill_pid` at all (`unwrap_or(false)` / `let _ =`), so a signal-send
    /// failure literally cannot reach this function's `Result` anymore, regardless of how the OS
    /// schedules the race. This test exercises the NORMAL (non-race) full escalation end-to-end —
    /// covered already by the tests above — plus explicitly documents why no dedicated live-race
    /// repro is included: forcing this EXACT kernel-level race deterministically (the target process
    /// dying in the microsecond window between `ProcCaps::kill`'s own liveness check and the
    /// `kill(2)` syscall) is not reliably controllable from outside the OS scheduler without
    /// mocking `terminate_pid`/`kill_pid` behind an injectable seam, which `ProcCaps` does not
    /// (and, for a two-line libc wrapper, should not) have.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn kill_still_confirms_termination_with_the_error_propagation_removed() {
        let caps = ProcCaps::new();
        let handle = caps.spawn(&spec("sleep", &["30"])).expect("sleep spawns");
        caps.kill(handle)
            .await
            .expect("kill still confirms real termination");
        assert!(
            caps.poll_exit(handle).is_some(),
            "poll_exit reflects the real termination"
        );
    }

    /// A busy/slow guest that never calls `read_stdout` against a maximally-bursty child (`yes`,
    /// which writes as fast as the pipe accepts, forever) must NOT let host memory grow without
    /// bound — the fix for the raw audit's confirmed Finding 2 (`spawn_pump`'s buffer used to be a
    /// raw unbounded `VecDeque`, a genuine unbounded-memory/DoS vector: a guest busy mid-LLM-turn
    /// against a bursty MCP server would grow host RAM without limit). Checks the REAL buffered
    /// byte count directly (never draining) across two separate waits and confirms it plateaus at
    /// [`MAX_PIPE_BUFFER_BYTES`] rather than climbing past it, then drains and confirms the pump
    /// resumes (proving it was genuinely PARKED on backpressure, not dead/dropping data).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unread_stdout_from_a_bursty_child_is_bounded_not_unbounded() {
        let caps = ProcCaps::new();
        let handle = caps.spawn(&spec("yes", &[])).expect("yes spawns");

        let buffered_len = |caps: &ProcCaps| -> usize {
            let reg = caps.registry();
            let entry = reg.get(&handle).expect("entry present");
            entry
                .stdout_buf
                .data
                .lock()
                .expect("stdout buffer lock")
                .len()
        };

        // EXT-N01: the bound is the cap PLUS one chunk, because that is the pump's actual
        // invariant — `wait_for_room` unparks at `len == MAX_PIPE_BUFFER_BYTES - 1` and the pump
        // then appends a whole `PIPE_CHUNK_BYTES` read before checking again. Sampling exactly at
        // the cap vs. one chunk above it is decided by the scheduler, so the old `<= cap` assertion
        // was a coin flip: observed green in one full-workspace run and red in the next at 16781628
        // bytes, an overshoot of 4412 — well inside one chunk and therefore NOT unbounded growth.
        // Detection power is unaffected: an unbounded buffer fed by `yes` for 500 ms is hundreds of
        // megabytes, not cap + 8 KiB.
        let bound = MAX_PIPE_BUFFER_BYTES + PIPE_CHUNK_BYTES;

        // `yes` writes continuously; give the pump time to hit (and, correctly, stay pinned at)
        // the cap — an unbounded buffer would already be well past MAX_PIPE_BUFFER_BYTES here.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let first = buffered_len(&caps);
        assert!(
            first <= bound,
            "buffered stdout ({first} bytes) exceeded the cap + one chunk ({bound}) — unbounded growth"
        );

        // Wait again with STILL no read — a genuinely unbounded buffer would have grown further; a
        // correctly capped+backpressured one stays pinned at (never beyond) the cap.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let second = buffered_len(&caps);
        assert!(
            second <= bound,
            "buffered stdout grew past the cap on a second check ({second} bytes) — the pump did \
             not actually stop reading at the cap"
        );
        // And it is genuinely PLATEAUED, not merely under a generous ceiling: 500 ms of `yes` moves
        // far more than one chunk, so an uncapped pump could not possibly land within one chunk of
        // where it already was. This is the assertion that survives the widened bound.
        assert!(
            second.abs_diff(first) <= PIPE_CHUNK_BYTES,
            "buffered stdout moved by more than one chunk between the two samples \
             ({first} -> {second}) — the pump is not parked at the cap"
        );

        // Drain a big chunk and confirm the pump resumes filling the buffer again shortly after —
        // proving the parked pump (and the real `yes` child) are genuinely alive, gated by
        // backpressure, not dead or silently discarding bytes past the cap.
        let drained = caps
            .read_stdout(handle, MAX_PIPE_BUFFER_BYTES as u32)
            .expect("read_stdout drains");
        assert!(
            !drained.is_empty(),
            "the buffered bytes were real, not silently dropped"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            buffered_len(&caps) > 0,
            "the pump did not resume reading after space freed — it looks dead, not parked"
        );

        caps.kill(handle)
            .await
            .expect("kill terminates the real yes");
    }

    /// A fake `AsyncRead` mirroring a bursty child's live pipe: always offers more bytes while
    /// `exited` is unset, then returns a genuine EOF (0 bytes) once it's set — exactly how a REAL
    /// pipe behaves once the child process exits and the kernel closes the write end. Avoids having
    /// to actually push [`MAX_PIPE_BUFFER_BYTES`] (16 MiB) through a real OS pipe just to reach the
    /// cap in a test.
    struct BurstyUntilExited {
        exited: watch::Receiver<Option<i32>>,
    }
    impl tokio::io::AsyncRead for BurstyUntilExited {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            if self.exited.borrow().is_some() {
                return std::task::Poll::Ready(Ok(())); // 0 bytes filled == EOF
            }
            let n = buf.remaining();
            buf.initialize_unfilled_to(n).fill(b'x');
            buf.advance(n);
            std::task::Poll::Ready(Ok(()))
        }
    }

    /// Closes the pump-task leak (distinct from the OS-process leak `446b858` fixed): a pump parked
    /// at [`PipeBufState::wait_for_room`]'s cap, whose buffer NOBODY ever drains again (the guest
    /// killed the process and stopped polling, or simply abandoned the handle), must still let its
    /// task actually END once the process exits — not stay parked forever, since nothing but a
    /// `drain()` used to ever wake it. Drives [`spawn_pump`]/[`PipeBufState`] directly (bypassing
    /// `ProcCaps::spawn`, which doesn't expose the pump's `JoinHandle`) to OBSERVE the task
    /// genuinely completing via a bounded `tokio::time::timeout`, not just infer it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pump_task_ends_once_the_process_exits_even_with_a_full_undrained_buffer() {
        let buf = PipeBufState::new();
        // Mirrors `ProcCaps::spawn`'s own wiring exactly: ONE `watch` channel, cloned once for the
        // (fake) pipe reader's own EOF-on-exit behavior and once for `wait_for_room`'s race.
        let (exit_tx, exit_rx) = watch::channel(None);
        let pump = spawn_pump(
            BurstyUntilExited {
                exited: exit_rx.clone(),
            },
            buf.clone(),
            exit_rx,
        );

        // Let the pump run until the buffer fills to the cap and it parks — nobody ever drains it.
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let len = buf.data.lock().expect("lock").len();
                if len >= MAX_PIPE_BUFFER_BYTES {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the pump must fill the buffer to the cap");

        // Simulate the process exiting — nobody ever drains the buffer afterward.
        let _ = exit_tx.send(Some(0));

        // THE fix: the pump task must actually END once notified of the exit, not park forever.
        tokio::time::timeout(Duration::from_secs(2), pump)
            .await
            .expect("the pump task must end once the process exits, not park forever")
            .expect("the pump task must not panic");
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
            caps.read_stderr(handle, 4096)
                .expect("read_stderr never errors"),
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

    /// Closes the shared-host-resource-exhaustion finding: `spawn` is rejected once
    /// [`MAX_SPAWNED_PROCESSES`] entries already exist in the registry — a guest that keeps spawning
    /// without ever being bounded must NOT be able to grow the registry without limit. Registry
    /// entries never evict (by design — see `ProcCaps`'s own doc: no close/dispose call), so this is
    /// deliberately a TOTAL-ever-spawned cap, not a concurrently-running one — using `true` (exits
    /// almost instantly) for every spawn here means essentially all of them have already exited by
    /// the time the cap is reached, yet the cap still fires (proving it counts REGISTRY entries, not
    /// live processes).
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn spawn_rejects_once_the_registry_cap_is_reached() {
        let caps = ProcCaps::new();
        for _ in 0..MAX_SPAWNED_PROCESSES {
            caps.spawn(&spec("true", &[]))
                .expect("spawn succeeds under the cap");
        }
        let err = caps
            .spawn(&spec("true", &[]))
            .expect_err("one more spawn must be rejected at the cap");
        assert!(err.contains("too many processes spawned"), "got: {err}");
    }

    /// THE regression this fix closes: the cap check must be atomic WITH the registry insert, not
    /// a separate lock acquisition with real spawn work in between (a TOCTOU race) — otherwise many
    /// concurrent `spawn` calls that all observe "not yet at the cap" on the fast up-front check can
    /// ALL proceed to spawn a real process and insert, overshooting [`MAX_SPAWNED_PROCESSES`].
    ///
    /// Deterministic trigger: fill the registry to exactly ONE BELOW the cap sequentially, then fire
    /// many concurrent `spawn` calls at once — every single one is guaranteed to observe
    /// `len() == cap - 1` on the fast-path check (nothing has raced ahead of any of them yet), so
    /// without the atomic re-check at insert time, several would win the race to insert. With the
    /// fix, EXACTLY one succeeds and the registry never exceeds the cap.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn spawn_cap_check_is_atomic_with_the_insert_under_concurrent_spawns() {
        let caps = Arc::new(ProcCaps::new());
        for _ in 0..MAX_SPAWNED_PROCESSES - 1 {
            caps.spawn(&spec("true", &[]))
                .expect("spawn succeeds under the cap");
        }
        assert_eq!(
            caps.registry().len(),
            MAX_SPAWNED_PROCESSES - 1,
            "primed one below the cap"
        );

        let mut tasks = Vec::new();
        for _ in 0..50 {
            let caps = Arc::clone(&caps);
            tasks.push(tokio::spawn(async move {
                caps.spawn(&spec("true", &[])).is_ok()
            }));
        }
        let mut ok_count = 0usize;
        for t in tasks {
            if t.await.expect("task joins") {
                ok_count += 1;
            }
        }

        assert_eq!(
            ok_count, 1,
            "EXACTLY one of the 50 concurrent spawns racing the last cap slot must succeed"
        );
        assert_eq!(
            caps.registry().len(),
            MAX_SPAWNED_PROCESSES,
            "the registry must never overshoot the cap even under a concurrent race"
        );
        // A loser takes one of two paths: it either forked a real process and then lost the atomic
        // re-check (so its pid MUST be recorded for `Drop`'s second-chance sweep), or it lost the
        // CHEAPER fast up-front check once the winner had already inserted and never forked at all.
        // Which path each of the 49 takes is decided by real OS scheduling.
        //
        // So the only invariant this test can assert about the count is the UPPER bound. There is no
        // lower bound: under load the winner's insert can land before any other task reaches its
        // fork, in which case all 49 take the cheap path and `orphaned == 0` is perfectly correct.
        // This assertion previously demanded `orphaned > 0` and consequently failed ~1 workspace run
        // in 3 — asserting a scheduling outcome the test's own comment admitted it could not pin
        // down.
        //
        // Dropping the lower bound costs no coverage: that a forked race-loser is recorded AND
        // reaped is proven deterministically next door by
        // `orphaned_pid_from_a_lost_spawn_race_is_reaped_on_drop`, which constructs the orphan
        // directly instead of trying to win a race.
        let orphaned = caps.orphaned_pids.lock().expect("lock").len();
        assert!(
            orphaned <= 49,
            "orphaned_pids must never exceed the 49 race losers, got {orphaned}"
        );
    }

    /// THE regression this closes: a pid that lost `Self::spawn`'s atomic cap race (a real process
    /// was forked, then rejected at the atomic re-check) had its cleanup depend ENTIRELY on a single
    /// immediate `kill_pid` attempt whose error was silently swallowed with no other compensating
    /// cleanup — since the pid was never inserted into `registry`, `Drop for ProcCaps`'s sweep (which
    /// only iterated `registry`) could never see it either, leaking it as a live OS process for the
    /// rest of the HOST process's lifetime if that one attempt ever failed. Verifies the fix directly
    /// (independent of actually forcing the race, which can't deterministically make the immediate
    /// `kill_pid` fail): a pid recorded in `orphaned_pids` — the exact bookkeeping
    /// `Self::spawn`'s rejection branch performs — is confirmed dead once `ProcCaps` drops, restoring
    /// the SAME safety net every successfully-registered child already gets from `Drop`.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn orphaned_pid_from_a_lost_spawn_race_is_reaped_on_drop() {
        let caps = ProcCaps::new();
        // A real, independently-spawned, long-running process `ProcCaps` never registered at all —
        // exactly what `Self::spawn`'s atomic-recheck rejection branch hands to `orphaned_pids` (a
        // real forked pid with no registry entry), without depending on timing to force the race.
        let mut orphan = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("sleep spawns");
        let pid = orphan.id();
        caps.orphaned_pids.lock().expect("lock").push(pid);

        let alive_before = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status();
        assert!(
            alive_before.map(|s| s.success()).unwrap_or(false),
            "the orphan must be alive before drop"
        );

        drop(caps);
        // SIGKILL is not interceptable, but the OS still needs a scheduler tick to actually reap it.
        for _ in 0..50 {
            if orphan.try_wait().ok().flatten().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        let alive_after = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status();
        assert!(
            alive_after.map(|s| !s.success()).unwrap_or(true),
            "an orphaned pid recorded via the lost-spawn-race path must be confirmed dead once \
             ProcCaps drops — Drop's sweep must cover orphaned_pids, not just registry"
        );
    }

    /// Closes the confirmed audit finding: a still-running child that the guest never explicitly
    /// `kill`ed must NOT leak as a live OS process once `ProcCaps` itself goes away (session
    /// teardown / extension unload) — restoring the safety net [`ProcCaps::spawn`]'s
    /// `kill_on_drop(true)` comment promises. Verified at the OS level (never just our own
    /// accounting): `kill -0` on the pid must fail shortly after `drop(caps)`.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_proc_caps_kills_a_still_running_child_no_leak() {
        let caps = ProcCaps::new();
        let handle = caps.spawn(&spec("sleep", &["30"])).expect("sleep spawns");
        assert_eq!(
            caps.poll_exit(handle),
            None,
            "genuinely still running before drop"
        );

        let pid = {
            let reg = caps.registry();
            reg.get(&handle).expect("entry present").pid
        };
        // Independently confirm the OS process is alive BEFORE the drop, so a later "not found" is
        // meaningful rather than a false positive from a pid that never existed.
        let alive_before = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status();
        assert!(
            alive_before.map(|s| s.success()).unwrap_or(false),
            "sleep must be alive pre-drop"
        );

        drop(caps); // NO explicit `kill(handle)` call — this is the leak scenario.

        // Give the synchronous SIGKILL a brief moment to actually land at the OS level.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let alive_after = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status();
        assert!(
            alive_after.map(|s| !s.success()).unwrap_or(true),
            "kill -0 must fail after dropping ProcCaps — the child must not outlive it unkilled"
        );
    }

    /// THE regression this fix closes: `write_stdin` against a child that never reads its stdin
    /// must eventually give up with a bounded `Err`, not hang the calling task (and, in production,
    /// the real tokio worker thread `block_in_place`+`block_on` bridges this call onto) forever.
    /// `sleep` never touches its stdin at all, so once the payload exceeds the OS pipe's kernel
    /// buffer capacity, `write_all` genuinely blocks on write-readiness that will never come.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn write_stdin_times_out_against_a_child_that_never_reads_its_stdin() {
        let caps = ProcCaps::with_write_stdin_timeout(Duration::from_millis(200));
        let handle = caps.spawn(&spec("sleep", &["30"])).expect("sleep spawns");
        // Far larger than any realistic OS pipe buffer (typically 16-64KB) so `write_all` is
        // GUARANTEED to still be in flight, genuinely blocked on write-readiness, when the 200ms
        // timeout fires — not just fast enough to complete before it.
        let payload = vec![b'x'; 8 * 1024 * 1024];

        let started = tokio::time::Instant::now();
        let result = caps.write_stdin(handle, &payload).await;
        let elapsed = started.elapsed();

        let err = result.expect_err(
            "a write against a child that never reads its stdin must time out, not succeed",
        );
        assert!(
            err.contains("timed out"),
            "the error must identify itself as a timeout, not some other failure: {err}"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "the bounded 200ms timeout must fire — this must NEVER hang for the full write \
             duration (which, against a non-reading child, would be forever): got {elapsed:?}"
        );

        // The stdin handle is left open after a mere timeout (not a real io error) — a subsequent
        // `kill` must still work normally, proving the capability itself is not left wedged.
        caps.kill(handle)
            .await
            .expect("the capability survives a write_stdin timeout intact");
    }

    /// THE LOW finding this closes: `write_stdin`'s bound must cover BOTH acquiring the per-handle
    /// stdin lock AND the write — a version that only timed the write let a SECOND concurrent
    /// `write_stdin` against the SAME handle wait UNBOUNDED for the first call's lock, then get its
    /// own full `write_stdin_timeout` budget on top once it finally acquired it, compounding to ~2x
    /// the documented ceiling. Two concurrent calls race a real child that never reads its stdin, with
    /// a payload guaranteed to keep the first call's `write_all` genuinely in flight (holding the
    /// lock) for the WHOLE timeout window. Both must resolve within roughly ONE timeout window of
    /// the SECOND call being issued, not two.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_write_stdin_calls_against_the_same_handle_do_not_compound_the_timeout() {
        let caps = Arc::new(ProcCaps::with_write_stdin_timeout(Duration::from_millis(
            300,
        )));
        let handle = caps.spawn(&spec("sleep", &["30"])).expect("sleep spawns");
        let payload = || vec![b'x'; 8 * 1024 * 1024];

        let started = tokio::time::Instant::now();
        let first = {
            let caps = Arc::clone(&caps);
            let payload = payload();
            tokio::spawn(async move { caps.write_stdin(handle, &payload).await })
        };
        // Give the first call a head start so it genuinely holds the lock (mid-`write_all`) before
        // the second one even attempts to acquire it — proving the second call's wait genuinely
        // starts AFTER, not concurrently racing for, the same lock acquisition.
        tokio::time::sleep(Duration::from_millis(20)).await;
        let second = {
            let caps = Arc::clone(&caps);
            let payload = payload();
            tokio::spawn(async move { caps.write_stdin(handle, &payload).await })
        };

        let first_result = first.await.expect("first task joins");
        let second_result = second.await.expect("second task joins");
        let elapsed = started.elapsed();

        assert!(
            first_result.is_err(),
            "the first write against a non-reading child must time out"
        );
        assert!(
            second_result.is_err(),
            "the second write must also time out, not hang unbounded"
        );
        assert!(
            elapsed < Duration::from_millis(500),
            "two concurrent writes against the same handle must resolve within roughly ONE \
             300ms timeout window (~350ms with the 20ms head start + scheduling slack), NOT \
             compound to ~2x it (~600ms — the pre-fix bug: the second call's lock-acquisition wait \
             was unbounded, THEN it got a full fresh 300ms write timeout on top once it finally \
             acquired the lock): got {elapsed:?}"
        );

        caps.kill(handle)
            .await
            .expect("the capability survives concurrent write_stdin timeouts intact");
    }

    /// L4 review §5 — `kill`'s phase-1 stdin-close must NEVER be blocked by a CONCURRENT
    /// `write_stdin` against the SAME handle. Before the fix, both held `entry.stdin`'s mutex for
    /// the write's full in-flight duration (up to `write_stdin_timeout`, 30s in production) —
    /// `kill` could stall up to 30s on top of its own ~`kill_grace`*2 + `KILL_CONFIRM_TIMEOUT`
    /// budget. `write_stdin_timeout` is set deliberately long here (30s) so the write is
    /// GUARANTEED to still be genuinely in flight (blocked on write-readiness against `sleep`,
    /// which never reads its stdin) when `kill` races it — proving `kill` doesn't merely finish
    /// before some short timeout, but is structurally independent of the write's own deadline.
    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn kill_is_never_blocked_by_a_concurrent_write_stdin_against_the_same_handle() {
        let caps = Arc::new(ProcCaps {
            registry: Mutex::new(HashMap::new()),
            next_handle: AtomicU32::new(1),
            kill_grace: Duration::from_millis(150),
            write_stdin_timeout: Duration::from_secs(30),
            orphaned_pids: Mutex::new(Vec::new()),
        });
        let handle = caps.spawn(&spec("sleep", &["30"])).expect("sleep spawns");
        // Far larger than any realistic OS pipe buffer so `write_all` is genuinely still blocked,
        // not merely fast, once `kill` races it (same margin as the timeout test above).
        let payload = vec![b'x'; 8 * 1024 * 1024];

        let caps_w = caps.clone();
        let write_task = tokio::spawn(async move { caps_w.write_stdin(handle, &payload).await });
        // Let the write actually start and fill the OS pipe buffer so it is DEMONSTRABLY in flight
        // (not merely queued) by the time `kill` runs.
        tokio::time::sleep(Duration::from_millis(100)).await;

        let started = tokio::time::Instant::now();
        let result = caps.kill(handle).await;
        let elapsed = started.elapsed();

        assert!(
            result.is_ok(),
            "kill must still succeed while a write is in flight: {result:?}"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "kill must not be blocked by the concurrent write_stdin's 30s timeout (its own budget \
             is ~kill_grace*2 + KILL_CONFIRM_TIMEOUT, well under a second here): took {elapsed:?}"
        );

        // The in-flight write eventually resolves too (SIGKILL tears down the pipe, turning the
        // still-blocked `write_all` into a real broken-pipe `Err` well before its 30s timeout) —
        // clean up rather than leak the task.
        let _ = tokio::time::timeout(Duration::from_secs(5), write_task)
            .await
            .expect("the write must resolve once its target process is gone, not hang");
    }
}
