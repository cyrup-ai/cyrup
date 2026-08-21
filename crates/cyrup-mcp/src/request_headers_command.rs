//! Per-request HTTP header commands — `request-headers-command.ts` (upstream **v2.26.0**).
//!
//! Ported from `pi-mcp-adapter` commit `2a2db3c` *"feat: support per-request HTTP header commands"*
//! (`request-headers-command.ts:1-332`, PR #353) as refined by `91f9943` *"refactor: model header
//! command results explicitly"* (`request-headers-command.ts:149-297`). The plan
//! (`docs/gap-analysis/13*.md`) was written against v2.25.0 and does not mention this file; the
//! retarget to v2.26.1 is why it exists, and every `file:line` below is against
//! `git show 2a2db3c:request-headers-command.ts`.
//!
//! # What it is for, in one paragraph
//!
//! `headers` (and `bearerToken`) are resolved **once**, when the HTTP server connects. That is
//! useless for caller-bound request signing — HMAC over the body, DPoP, AWS SigV4 — where the
//! header is a function of *this* request's method, URL and exact bytes. `requestHeadersCommand`
//! names a trusted executable that the adapter runs **on every outbound HTTP request**, handing it
//! a versioned JSON envelope on stdin and reading a JSON object of headers off stdout. It is
//! deliberately **fail-closed**: any failure — non-zero exit, malformed output, timeout, or a
//! cleanup step that could not prove the process tree is dead — aborts the request rather than
//! sending it unsigned.
//!
//! # The half of this file that is not about headers at all
//!
//! Roughly a third of `request-headers-command.ts` is process-tree reaping, and it is the security
//! property the upstream PR is actually about: a signing command that spawns a helper (an agent, a
//! token broker, `op run`) must not leave that helper alive after the request settles, and must not
//! let it escape by `setsid`-ing itself out of the child's process group. Upstream's answer, and
//! this port's, is a **freeze-then-kill** sweep:
//!
//! 1. Every spawned command carries an opaque per-invocation token in its environment
//!    ([`CLEANUP_TOKEN_ENV`]). The token is *published* — `ps axeww` shows it to every local user —
//!    so it is a correlation marker, never a secret, which is why [`uuid::Uuid::now_v7`] is
//!    sufficient where upstream reaches for `randomUUID` (`request-headers-command.ts:2`).
//! 2. While the command runs, a 50 ms sweep records every descendant pid, both by walking
//!    `ps -axo pid=,ppid=` from the child and by matching the token in `ps axeww -o pid=,command=`.
//!    The second scan is what catches a helper that reparented to `init` or moved to another
//!    process group.
//! 3. On **every** terminating path — success included — the child's process group is `SIGSTOP`ped,
//!    then the sweep repeats until two consecutive passes find nothing new (16 passes maximum), then
//!    everything frozen is `SIGKILL`ed. Freezing first is what stops the tree from outrunning the
//!    walk.
//! 4. If `ps` itself cannot be run, the whole thing fails **before** the command is ever spawned
//!    ([`assert_posix_process_discovery_available`]), because a cleanup that cannot enumerate is a
//!    cleanup that cannot promise anything.
//!
//! # Where it plugs in, and the one thing that is not here
//!
//! Upstream wires this at `server-manager.ts:747-780`: `definition.requestHeadersCommand` becomes a
//! `fetch` replacement passed to both the Streamable HTTP and SSE transports. rmcp has no `fetch`
//! seam — its equivalent is the [`StreamableHttpClient`] trait — so the wrapper here is a
//! **decorator over that trait** ([`RequestHeadersCommandClient`]) rather than over `fetch`, and it
//! composes with the existing seam: `crate::runtime::http_transport_with_client(client, config)`
//! accepts it unchanged. `McpServerManager::connectHttpClient` itself is MCP-100/MCP-115 and is not
//! ported yet, so this decorator has no production caller today — exactly the state
//! [`crate::runtime::http_transport_with_client`] is already in.
//!
//! # Five recorded divergences
//!
//! 1. **The abort signal.** Upstream reads `request.signal`, a *per-request* `AbortSignal`
//!    threaded through `fetch`. rmcp's [`StreamableHttpClient`] has no signal parameter; the only
//!    cancellation reaching this layer is the connection's [`CancelToken`], supplied once at
//!    construction. The abort message and the fail-closed behaviour are identical; the granularity
//!    is per-connection rather than per-request.
//! 2. **Reserved header names.** Upstream's `headers.set(name, value)` can overwrite anything,
//!    including `Accept` and `Mcp-Session-Id`. rmcp routes custom headers through
//!    `validate_custom_header`, which refuses those two and `Last-Event-Id` with
//!    `StreamableHttpError::ReservedHeaderConflict`. A command that derives one of them fails the
//!    request here where upstream would have sent it.
//! 3. **`Authorization` is appended, not replaced.** When a bearer token resolved, rmcp owns that
//!    header through `auth_header` and applies it with `bearer_auth`; a derived `Authorization`
//!    arrives as a *second* header value rather than replacing the first.
//! 4. **Duplicate names differing only in case.** `new Headers([["X-A","1"],["x-a","2"]])` joins to
//!    `x-a: 1, 2`; a `HashMap<HeaderName, _>` keeps the last. `serde_json` without `preserve_order`
//!    also sorts the object's keys, so "last" is the byte-greater name rather than the last written.
//! 5. **A non-ESRCH `kill(2)` failure** carries this module's `cleanup failed:` sentence rather than
//!    Node's raw `EPERM: …` string; upstream has no fixed text for that arm to match.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read as _;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use cyrup_core::CancelToken;
use futures::stream::BoxStream;
use http::{HeaderName, HeaderValue};
use rmcp::model::ClientJsonRpcMessage;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};
use serde::Serialize;
use sse_stream::{Error as SseError, Sse};

use crate::config::HttpRequestHeadersCommand;
use crate::errors::{McpError, McpResult};
use crate::oauth::interpolate_env_vars;

// ===================================================================================================
// 1 · Constants (`request-headers-command.ts:7-10`)
// ===================================================================================================

/// `DEFAULT_TIMEOUT_MS = 10_000` (`request-headers-command.ts:7`).
const DEFAULT_TIMEOUT_MS: f64 = 10_000.0;

/// The upper bound `resolvedCommand` enforces on `timeoutMs` (`request-headers-command.ts:174`).
const MAX_TIMEOUT_MS: f64 = 60_000.0;

/// `MAX_OUTPUT_BYTES = 64 * 1024` (`request-headers-command.ts:8`). Checked as a strict `>`, so
/// exactly 64 KiB of stdout is legal and one byte more is not.
const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// `for (let pass = 0; pass < 16; pass++)` (`request-headers-command.ts:118`) — how many sweeps the
/// freeze loop will run before it declares the tree unstable.
const STABILISATION_PASSES: u32 = 16;

/// `setInterval(trackPosixDescendants, 50)` (`request-headers-command.ts:221`). POSIX only —
/// upstream's `USE_PROCESS_GROUP` guard is `platform !== "win32"`, and the Windows arm reaps with
/// `taskkill /T` rather than by enumerating.
#[cfg(unix)]
const TRACK_INTERVAL: Duration = Duration::from_millis(50);

/// How often the wait loop polls the child. Upstream has no equivalent — libuv wakes on the child's
/// exit — so this is the same 10 ms cadence [`crate::oauth::resolve_command_secret`] already uses
/// for the one other subprocess this crate waits on synchronously.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// The envelope's `version` field. Upstream pins the literal `1` (`request-headers-command.ts:143`);
/// a command may switch on it, so it is protocol, not decoration.
const ENVELOPE_VERSION: u8 = 1;

/// `CLEANUP_TOKEN_ENV = "PI_MCP_REQUEST_HEADERS_CLEANUP_TOKEN"`
/// (`request-headers-command.ts:10`) — **recorded rename**, following the same `CYRUP_MCP_*`
/// convention [`crate::credentials`] applies to every other adapter-owned variable. The child never
/// reads it and README never documents it; it exists only so the cleanup sweep can recognise its own
/// descendants in `ps` output.
pub const CLEANUP_TOKEN_ENV: &str = "CYRUP_MCP_REQUEST_HEADERS_CLEANUP_TOKEN";

/// `PI_MCP_ADAPTER_TEST_FAIL_PS` (`request-headers-command.ts:17`) — the fault injector upstream's
/// suite uses to prove the fail-closed preflight. Dual-read `CYRUP_MCP_*` first, the convention
/// [`crate::credentials`] documents (MCP-282).
const TEST_FAIL_PS_ENV: [&str; 2] =
    ["CYRUP_MCP_TEST_FAIL_PS", "PI_MCP_ADAPTER_TEST_FAIL_PS"];

// ===================================================================================================
// 2 · Errors
// ===================================================================================================

/// Upstream throws bare `Error`s whose *text* is the contract — the tests assert on the sentence,
/// not on a class — so every message below is byte-identical to its upstream twin.
fn command_error(message: impl Into<String>) -> McpError {
    McpError::Other(message.into())
}

/// `` `HTTP request headers command cleanup failed: ${reason}` `` — the prefix shared by the four
/// cleanup arms (`request-headers-command.ts:26`, `:45`, `:76`, `:130`).
fn cleanup_failed(reason: &str) -> McpError {
    command_error(format!("HTTP request headers command cleanup failed: {reason}"))
}

/// `${code ?? "unknown"}` for a process that may have died on a signal.
fn exit_code_text(status: Option<i32>) -> String {
    status.map_or_else(|| "unknown".to_string(), |code| code.to_string())
}

// ===================================================================================================
// 3 · `resolvedCommand` — the static configuration gate (`request-headers-command.ts:153-190`)
// ===================================================================================================

/// A validated, environment-interpolated [`HttpRequestHeadersCommand`] — upstream's anonymous
/// `{ command, args, env, timeoutMs }` return (`request-headers-command.ts:153-158`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRequestHeadersCommand {
    /// `interpolateEnvVars(config.command)`.
    pub command: String,
    /// `(config.args ?? []).map(interpolateEnvVars)`.
    pub args: Vec<String>,
    /// The **overrides only**. Upstream spreads `process.env` underneath them;
    /// [`std::process::Command`] inherits the parent environment by default, so layering the
    /// overrides on top is byte-equivalent and avoids snapshotting the environment.
    pub env: BTreeMap<String, String>,
    /// `config.timeoutMs ?? DEFAULT_TIMEOUT_MS`, already proven to be an integer in `1..=60000`.
    pub timeout: Duration,
}

/// `resolvedCommand(config)` (`request-headers-command.ts:153`).
///
/// # Three of upstream's five throws have no input here — and that is a fail-OPEN (MCP-248)
///
/// `"must be an object"`, `"args must be strings"` and `"env values must be strings"` guard against
/// a `ServerEntry` that TypeScript typed but never validated. [`HttpRequestHeadersCommand`] is a
/// serde struct read through [`crate::config::lenient`], so a wrong-typed `args` degrades to `None`
/// at parse time and those three sentences have no input. The two that **are** reachable —
/// a blank `command` and an out-of-range `timeoutMs` — are the two upstream's own test asserts
/// (`__tests__/request-headers-command.test.ts:344-351`).
///
/// Do not read "unreachable" as "benign". `lenient` degrades the *whole* block, so
/// `"requestHeadersCommand": "sign.sh"` parses to `None` and the server connects **unsigned**,
/// where upstream throws `"HTTP request headers command must be an object"` and refuses the
/// connection. This is the one module in the crate whose entire contract is fail-closed, and
/// `config.rs`'s rule 4 ("a malformed value must not take the file down") points the other way, so
/// closing it is a deliberate policy change tracked as **MCP-248**, not an oversight to patch here.
/// It is unobservable today only because the connect path (MCP-115) has no caller yet.
///
/// # Errors
///
/// [`McpError::Other`] carrying upstream's sentence verbatim.
pub fn resolve_request_headers_command(
    config: &HttpRequestHeadersCommand,
) -> McpResult<ResolvedRequestHeadersCommand> {
    let command = config.command.as_deref().unwrap_or_default();
    if command.trim().is_empty() {
        return Err(command_error(
            "HTTP request headers command requires a non-empty command",
        ));
    }

    // `Number.isInteger(timeoutMs) && timeoutMs > 0 && timeoutMs <= 60_000`. `is_finite` is
    // `Number.isInteger`'s NaN/Infinity half; `fract() != 0.0` is the integrality half. The field is
    // `f64` because that is what `JSON.parse` produces and what every other numeric `ServerEntry`
    // field in this crate already is.
    let timeout_ms = config.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
    if !timeout_ms.is_finite()
        || timeout_ms.fract() != 0.0
        || timeout_ms <= 0.0
        || timeout_ms > MAX_TIMEOUT_MS
    {
        return Err(command_error(
            "HTTP request headers command timeoutMs must be an integer between 1 and 60000",
        ));
    }

    Ok(ResolvedRequestHeadersCommand {
        command: interpolate_env_vars(command),
        args: config.args.iter().flatten().map(|arg| interpolate_env_vars(arg)).collect(),
        env: config
            .env
            .iter()
            .flatten()
            .map(|(key, value)| (key.clone(), interpolate_env_vars(value)))
            .collect(),
        // Validated above: finite, integral, `1..=60000`.
        timeout: Duration::from_millis(timeout_ms as u64),
    })
}

// ===================================================================================================
// 4 · The request envelope (`request-headers-command.ts:140-147`)
// ===================================================================================================

/// `HttpRequestCommandEnvelope` (`request-headers-command.ts:140`) — the JSON object written to the
/// command's stdin.
///
/// The body is base64 so the command sees the **exact bytes** that will go on the wire, not a
/// re-serialisation: a signature over a re-encoded JSON body would verify against the wrong
/// document. `bodyBase64` is empty for `GET`, and — matching upstream's
/// `method === "GET" || "HEAD" ? {} : { body }` (`request-headers-command.ts:319`) — also empty for
/// the session `DELETE`, which carries no body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpRequestCommandEnvelope {
    /// Always [`ENVELOPE_VERSION`].
    pub version: u8,
    /// The HTTP method, upper-cased (`request-headers-command.ts:311`).
    pub method: String,
    /// The absolute request URL.
    pub url: String,
    /// Standard-alphabet, **padded** base64 of the request body — `Buffer.toString("base64")`.
    pub body_base64: String,
}

impl HttpRequestCommandEnvelope {
    /// Build the envelope for one outbound request.
    #[must_use]
    pub fn new(method: &str, url: &str, body: &[u8]) -> Self {
        Self {
            version: ENVELOPE_VERSION,
            method: method.to_ascii_uppercase(),
            url: url.to_string(),
            body_base64: base64::engine::general_purpose::STANDARD.encode(body),
        }
    }
}

// ===================================================================================================
// 5 · POSIX process discovery (`request-headers-command.ts:12-60`)
// ===================================================================================================

/// `process.env.PI_MCP_ADAPTER_TEST_FAIL_PS === "1"` (`request-headers-command.ts:17`) — strict
/// equality, so `"true"` does not trip it.
fn test_fail_ps() -> bool {
    TEST_FAIL_PS_ENV
        .iter()
        .find_map(|name| std::env::var(name).ok())
        .as_deref()
        == Some("1")
}

/// `runPosixPs(args)` (`request-headers-command.ts:16`).
///
/// Upstream's `spawnSync` yields `status: null` when `ps` cannot be spawned at all, which its
/// callers render as `"ps exited with code unknown"`; a spawn failure here takes the same arm.
#[cfg(unix)]
fn run_posix_ps(args: &[&str]) -> McpResult<String> {
    if test_fail_ps() {
        return Err(cleanup_failed("ps exited with code 1"));
    }
    match std::process::Command::new("ps")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    {
        Ok(output) if output.status.success() => {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        Ok(output) => Err(cleanup_failed(&format!(
            "ps exited with code {}",
            exit_code_text(output.status.code())
        ))),
        Err(_) => Err(cleanup_failed("ps exited with code unknown")),
    }
}

/// `collectPosixDescendantPids(rootPid)` (`request-headers-command.ts:21`) — the whole subtree
/// under `root`, found by inverting `ps`' pid→ppid table.
///
/// **Hardening delta:** the walk carries a `seen` set. Upstream's stack-based walk would spin
/// forever on a self-parenting row, which `ps` can produce for pid 0 on some kernels; a hang inside
/// a cleanup that the request is *waiting on* is strictly worse than the divergence.
#[cfg(unix)]
fn collect_posix_descendant_pids(root: i32) -> McpResult<Vec<i32>> {
    let output = run_posix_ps(&["-axo", "pid=,ppid="])?;

    let mut children_by_parent: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
    for line in output.lines() {
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(ppid)) = (fields.next(), fields.next()) else {
            continue;
        };
        let (Ok(pid), Ok(ppid)) = (pid.parse::<i32>(), ppid.parse::<i32>()) else {
            continue;
        };
        children_by_parent.entry(ppid).or_default().push(pid);
    }

    let mut descendants = Vec::new();
    let mut seen: BTreeSet<i32> = BTreeSet::new();
    let mut stack: Vec<i32> = children_by_parent.get(&root).cloned().unwrap_or_default();
    while let Some(pid) = stack.pop() {
        if pid == root || !seen.insert(pid) {
            continue;
        }
        descendants.push(pid);
        if let Some(children) = children_by_parent.get(&pid) {
            stack.extend(children.iter().copied());
        }
    }
    Ok(descendants)
}

/// `collectPosixCleanupTokenPids(cleanupToken)` (`request-headers-command.ts:44`).
///
/// `ps axeww -o pid=,command=` prints each process' **environment** after its argv, which is the
/// only portable way to find a descendant that has already reparented to `init` or `setsid`-ed out
/// of the child's process group. Our own pid is excluded — the token is in this process'
/// environment too only if a parent set it, but upstream excludes it unconditionally and so do we.
#[cfg(unix)]
fn collect_posix_cleanup_token_pids(cleanup_token: &str) -> McpResult<Vec<i32>> {
    let output = run_posix_ps(&["axeww", "-o", "pid=,command="])?;
    let needle = format!("{CLEANUP_TOKEN_ENV}={cleanup_token}");
    let self_pid = i32::try_from(std::process::id()).unwrap_or(-1);

    let mut pids = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if !trimmed.contains(&needle) {
            continue;
        }
        let Some(first) = trimmed.split_whitespace().next() else {
            continue;
        };
        let Ok(pid) = first.parse::<i32>() else {
            continue;
        };
        if pid != self_pid {
            pids.push(pid);
        }
    }
    Ok(pids)
}

/// `assertPosixProcessDiscoveryAvailable()` (`request-headers-command.ts:62`).
///
/// Run **before the command is spawned**. If `ps` is unavailable the invocation cannot promise to
/// reap anything the command spawns, so it refuses to spawn at all — the fail-closed contract in one
/// function, and the reason three of upstream's tests assert that the marker file never appears.
///
/// # Errors
///
/// [`McpError::Other`] with the `cleanup failed: ps exited with code …` sentence.
#[cfg(unix)]
pub fn assert_posix_process_discovery_available() -> McpResult<()> {
    let self_pid = i32::try_from(std::process::id()).unwrap_or(-1);
    let _ = collect_posix_descendant_pids(self_pid)?;
    let _ = collect_posix_cleanup_token_pids(&format!("{self_pid}-preflight"))?;
    Ok(())
}

// ===================================================================================================
// 6 · Signalling and the freeze-then-kill sweep (`request-headers-command.ts:71-136`)
// ===================================================================================================

/// `signalPid(pid, signal)` (`request-headers-command.ts:79`) — `ESRCH` (the process already died)
/// is the one error swallowed, exactly as upstream's `isNoSuchProcessError` swallows it.
#[cfg(unix)]
fn signal_pid(pid: i32, signal: nix::sys::signal::Signal) -> McpResult<()> {
    match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), signal) {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(errno) => Err(cleanup_failed(&format!("kill({pid}) failed: {errno}"))),
    }
}

/// `signalProcessGroup(pid, signal)` (`request-headers-command.ts:87`) — `process.kill(-pid, …)`,
/// i.e. `killpg`. The child is a group leader because it was spawned `detached`.
#[cfg(unix)]
fn signal_process_group(pid: i32, signal: nix::sys::signal::Signal) -> McpResult<()> {
    match nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pid), signal) {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(errno) => Err(cleanup_failed(&format!("killpg({pid}) failed: {errno}"))),
    }
}

/// One 50 ms sweep of `trackPosixDescendants` (`request-headers-command.ts:209`).
#[cfg(unix)]
fn track_posix_descendants(
    child_pid: i32,
    cleanup_token: &str,
    tracked: &mut BTreeSet<i32>,
) -> McpResult<()> {
    for pid in collect_posix_descendant_pids(child_pid)? {
        tracked.insert(pid);
    }
    for pid in collect_posix_cleanup_token_pids(cleanup_token)? {
        tracked.insert(pid);
    }
    Ok(())
}

/// The `try` block of `killRequestHeadersCommand` (`request-headers-command.ts:105-131`): freeze the
/// group, freeze everything already tracked, then sweep until **two consecutive** passes find
/// nothing new.
///
/// Two passes rather than one is not belt-and-braces: a helper that `fork`s during the pass between
/// its parent being frozen and the walk reaching it would be found on the next pass, and the second
/// clean pass is what proves no such window was open.
#[cfg(unix)]
fn freeze_descendant_tree(
    child_pid: i32,
    tracked: &BTreeSet<i32>,
    cleanup_token: &str,
    frozen: &mut BTreeSet<i32>,
) -> McpResult<()> {
    use nix::sys::signal::Signal;

    signal_process_group(child_pid, Signal::SIGSTOP)?;
    for pid in tracked {
        signal_pid(*pid, Signal::SIGSTOP)?;
        frozen.insert(*pid);
    }

    let mut stable_passes = 0_u32;
    for _ in 0..STABILISATION_PASSES {
        let mut candidates = collect_posix_descendant_pids(child_pid)?;
        candidates.extend(collect_posix_cleanup_token_pids(cleanup_token)?);
        let fresh: Vec<i32> =
            candidates.into_iter().filter(|pid| !frozen.contains(pid)).collect();

        if fresh.is_empty() {
            stable_passes += 1;
            if stable_passes >= 2 {
                return Ok(());
            }
            continue;
        }

        stable_passes = 0;
        for pid in fresh {
            signal_pid(pid, Signal::SIGSTOP)?;
            frozen.insert(pid);
        }
    }

    Err(cleanup_failed("descendant process tree did not stabilize"))
}

/// `killRequestHeadersCommand(child, tracked, cleanupToken)` (`request-headers-command.ts:93`).
///
/// The `finally` block runs whether the sweep succeeded, failed, or returned early — and, this being
/// a faithful port of a JavaScript `try/finally`, a throw **from** the `finally` replaces the pending
/// error, which is why the two `SIGKILL` arms below use `?` rather than being ignored.
///
/// # Errors
///
/// [`McpError::Other`] with a `cleanup failed: …` sentence when the tree could not be enumerated,
/// did not stabilise, or could not be signalled.
#[cfg(unix)]
pub fn kill_request_headers_command(
    child_pid: i32,
    tracked: &BTreeSet<i32>,
    cleanup_token: &str,
) -> McpResult<()> {
    use nix::sys::signal::Signal;

    let mut frozen: BTreeSet<i32> = BTreeSet::new();
    let outcome = freeze_descendant_tree(child_pid, tracked, cleanup_token, &mut frozen);

    signal_process_group(child_pid, Signal::SIGKILL)?;
    for pid in &frozen {
        signal_pid(*pid, Signal::SIGKILL)?;
    }
    outcome
}

/// The Windows arm of `killRequestHeadersCommand` (`request-headers-command.ts:94-101`):
/// `taskkill /pid N /T /F`, treating "not found" as success.
#[cfg(windows)]
pub fn kill_request_headers_command(
    child_pid: i32,
    _tracked: &BTreeSet<i32>,
    _cleanup_token: &str,
) -> McpResult<()> {
    match std::process::Command::new("taskkill")
        .args(["/pid", &child_pid.to_string(), "/T", "/F"])
        .output()
    {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => {
            // `isTaskkillNoSuchProcess` (`request-headers-command.ts:71`) — the process was already
            // gone, which is the outcome we wanted.
            let merged = format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
            .to_lowercase();
            if merged.contains("not found") {
                return Ok(());
            }
            Err(cleanup_failed(&format!(
                "taskkill exited with code {}",
                exit_code_text(output.status.code())
            )))
        }
        Err(_) => Err(cleanup_failed("taskkill exited with code unknown")),
    }
}

// ===================================================================================================
// 7 · `invokeRequestHeadersCommand` (`request-headers-command.ts:192-297`)
// ===================================================================================================

/// Parse the command's stdout into a header list — the `close` handler's tail
/// (`request-headers-command.ts:272-296`).
///
/// The two passes are upstream's own order: `entries.some(([, v]) => typeof v !== "string")` rejects
/// the whole object *before* `new Headers(entries)` is attempted, so `{"a":"\n","b":1}` reports
/// `values must be strings` rather than `returned an invalid header`.
fn parse_derived_headers(stdout: &[u8]) -> McpResult<Vec<(HeaderName, HeaderValue)>> {
    let text = String::from_utf8_lossy(stdout);
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Err(command_error(
            "HTTP request headers command returned invalid JSON",
        ));
    };
    // `!parsed || typeof parsed !== "object" || Array.isArray(parsed)` — `null`, an array and every
    // scalar all land here.
    let Some(object) = parsed.as_object() else {
        return Err(command_error(
            "HTTP request headers command must return a JSON object",
        ));
    };

    if object.values().any(|value| !value.is_string()) {
        return Err(command_error(
            "HTTP request headers command values must be strings",
        ));
    }

    let mut headers = Vec::with_capacity(object.len());
    for (name, value) in object {
        let value = value.as_str().unwrap_or_default();
        // `new Headers(...)`'s own validation: a name that is not an HTTP token, or a value carrying
        // NUL/CR/LF, throws. `HeaderName`/`HeaderValue`'s `TryFrom` reject the same inputs.
        let (Ok(name), Ok(value)) =
            (HeaderName::try_from(name.as_str()), HeaderValue::try_from(value))
        else {
            return Err(command_error(
                "HTTP request headers command returned an invalid header",
            ));
        };
        headers.push((name, value));
    }
    Ok(headers)
}

/// `invokeRequestHeadersCommand(config, envelope, signal)` (`request-headers-command.ts:192`) —
/// **blocking**, one subprocess per call, fail-closed on every arm.
///
/// The error precedence is upstream's and is load-bearing:
///
/// 1. A **cleanup** failure replaces everything, success included — `finishAfterKill` runs the kill
///    inside a `try` and rejects with the cleanup error if it throws
///    (`request-headers-command.ts:232-240`).
/// 2. A **tracking** failure (a `ps` that broke mid-run) beats the timeout, the abort and the exit
///    code — `failAfterKill` is `trackingError ?? new Error(message)`
///    (`request-headers-command.ts:242`), and the `close` handler tests it first
///    (`request-headers-command.ts:264`).
/// 3. Only then the ordinary arms: abort, timeout, output cap, exit code, output shape.
///
/// # Errors
///
/// [`McpError::Other`] carrying one of upstream's eleven sentences.
pub fn invoke_request_headers_command(
    config: &HttpRequestHeadersCommand,
    envelope: &HttpRequestCommandEnvelope,
    cancel: Option<&CancelToken>,
) -> McpResult<Vec<(HeaderName, HeaderValue)>> {
    let resolved = resolve_request_headers_command(config)?;

    // `if (USE_PROCESS_GROUP) assertPosixProcessDiscoveryAvailable()` — before the spawn, so a
    // machine that cannot enumerate processes never starts one (`request-headers-command.ts:198`).
    #[cfg(unix)]
    assert_posix_process_discovery_available()?;

    // `JSON.stringify(envelope)` cannot fail for this shape; the arm exists because the crate denies
    // `unwrap`, and "we could not hand the child its input" is the same class of failure as "the
    // child never started".
    let payload = serde_json::to_vec(envelope).map_err(|_| {
        command_error("HTTP request headers command failed to start")
    })?;

    let cleanup_token = uuid::Uuid::now_v7().to_string();
    let mut command = std::process::Command::new(&resolved.command);
    command
        .args(&resolved.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // `stdio: ["pipe", "pipe", "ignore"]` — the command's diagnostics are deliberately dropped,
        // the same choice `resolveCommandSecret` makes.
        .stderr(Stdio::null());
    for (key, value) in &resolved.env {
        command.env(key, value);
    }
    // Last, so the token beats a same-named user override — upstream's
    // `{ ...resolved.env, [CLEANUP_TOKEN_ENV]: cleanupToken }` (`request-headers-command.ts:203`).
    command.env(CLEANUP_TOKEN_ENV, &cleanup_token);
    #[cfg(unix)]
    {
        // `detached: USE_PROCESS_GROUP` — the child becomes its own process-group leader so the whole
        // tree can be frozen and killed with one `killpg`.
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }

    let Ok(mut child) = command.spawn() else {
        return Err(command_error(
            "HTTP request headers command failed to start",
        ));
    };
    let child_pid = i32::try_from(child.id()).ok();

    // `child.stdin.end(JSON.stringify(envelope))` with `child.stdin.on("error", () => {})`. On its
    // own thread because the envelope carries the whole request body: a command that never reads
    // stdin would otherwise wedge this thread on a full pipe.
    if let Some(mut stdin) = child.stdin.take() {
        drop(std::thread::spawn(move || {
            use std::io::Write as _;
            let _ = stdin.write_all(&payload);
        }));
    }

    // `child.stdout.on("data", ...)` plus its 64 KiB guard. The flag lets the wait loop below react
    // to an overflow **while the command is still running**, which is what upstream's immediate
    // `failAfterKill` does.
    let overflowed = Arc::new(AtomicBool::new(false));
    let stdout = child.stdout.take();
    let reader_flag = Arc::clone(&overflowed);
    let reader = std::thread::spawn(move || -> Vec<u8> {
        let Some(mut stdout) = stdout else {
            return Vec::new();
        };
        let mut buffer = Vec::new();
        let mut chunk = [0_u8; 8192];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if reader_flag.load(Ordering::Relaxed) {
                        // Keep draining so the child is never wedged on a full pipe while the wait
                        // loop tears it down.
                        continue;
                    }
                    if let Some(slice) = chunk.get(..read) {
                        buffer.extend_from_slice(slice);
                    }
                    if buffer.len() > MAX_OUTPUT_BYTES {
                        reader_flag.store(true, Ordering::Relaxed);
                    }
                }
            }
        }
        buffer
    });

    let started = Instant::now();
    #[cfg(unix)]
    let mut last_track = Instant::now();
    // Both stay empty/`None` on Windows, where `taskkill /T` walks the tree for us.
    #[cfg_attr(windows, allow(unused_mut))]
    let mut tracked: BTreeSet<i32> = BTreeSet::new();
    #[cfg_attr(windows, allow(unused_mut))]
    let mut tracking_error: Option<McpError> = None;

    let outcome: McpResult<Option<i32>> = loop {
        if overflowed.load(Ordering::Relaxed) {
            break Err(command_error(
                "HTTP request headers command output exceeded 64 KiB",
            ));
        }
        if cancel.is_some_and(CancelToken::is_cancelled) {
            break Err(command_error("HTTP request headers command aborted"));
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status.code()),
            Ok(None) => {}
            Err(_) => {
                break Err(command_error(
                    "HTTP request headers command failed to start",
                ))
            }
        }
        if started.elapsed() >= resolved.timeout {
            break Err(command_error(format!(
                "HTTP request headers command timed out after {}ms",
                resolved.timeout.as_millis()
            )));
        }
        #[cfg(unix)]
        if tracking_error.is_none()
            && last_track.elapsed() >= TRACK_INTERVAL
            && let Some(pid) = child_pid
        {
            last_track = Instant::now();
            if let Err(error) = track_posix_descendants(pid, &cleanup_token, &mut tracked) {
                tracking_error = Some(error);
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    };

    // Reap the child first if it has already exited. Node does this for free — libuv reaps on
    // `SIGCHLD` — and the difference is not cosmetic: Darwin's `killpg(2)` answers **EPERM**, not
    // ESRCH, for a process group whose only remaining member is an unreaped zombie. The arms that
    // break out *without* having called `try_wait` (the 64 KiB overflow and the abort) would
    // otherwise report a spurious `cleanup failed: killpg(…) EPERM` in place of their own error,
    // because a cleanup failure outranks everything. `try_wait` never blocks, so the timeout arm —
    // where the command is still running — is unaffected and still freezes a live group.
    let _ = child.try_wait();

    // `finishAfterKill` is on the SUCCESS branch too (`request-headers-command.ts:292`): a helper the
    // command detached must not outlive the request that spawned it.
    let cleanup = match child_pid {
        Some(pid) => kill_request_headers_command(pid, &tracked, &cleanup_token),
        None => {
            let _ = child.kill();
            Ok(())
        }
    };
    let _ = child.wait();
    let stdout = reader.join().unwrap_or_default();

    let result = match tracking_error {
        Some(error) => Err(error),
        None => outcome.and_then(|code| {
            if code == Some(0) {
                parse_derived_headers(&stdout)
            } else {
                Err(command_error(format!(
                    "HTTP request headers command exited with code {}",
                    exit_code_text(code)
                )))
            }
        }),
    };

    cleanup?;
    result
}

// ===================================================================================================
// 8 · `createRequestHeadersCommandFetch` (`request-headers-command.ts:300-332`)
// ===================================================================================================

/// The `fetch` wrapper of `createRequestHeadersCommandFetch`, as an rmcp
/// [`StreamableHttpClient`] decorator.
///
/// Wrap the client, not the transport: `crate::runtime::http_transport_with_client` is generic over
/// [`StreamableHttpClient`], so
/// `http_transport_with_client(RequestHeadersCommandClient::new(client, cfg, ct)?, config)` is the
/// authorized and unauthorized arms both, and nothing else in [`crate::runtime`] changes.
///
/// Every one of rmcp's four request shapes is overridden — including the two
/// `…_with_max_sse_event_size` variants whose default bodies would otherwise delegate to *this*
/// type's `post_message`/`get_stream` and silently drop the transport-wide SSE size limit on its way
/// to the inner client.
#[derive(Debug, Clone)]
pub struct RequestHeadersCommandClient<C> {
    inner: C,
    config: Arc<HttpRequestHeadersCommand>,
    cancel: Option<CancelToken>,
}

impl<C> RequestHeadersCommandClient<C> {
    /// `createRequestHeadersCommandFetch(config, delegate)` (`request-headers-command.ts:300`).
    ///
    /// The configuration is validated **here**, before the first request, exactly as upstream's
    /// `// Validate static configuration before the first request.` comment demands — a blank
    /// `command` should fail the connect, not the first `tools/call`.
    ///
    /// `cancel` is divergence 1 in the module header: rmcp exposes no per-request signal, so the
    /// connection's token is the abort input.
    ///
    /// # Errors
    ///
    /// [`McpError::Other`] with one of [`resolve_request_headers_command`]'s two sentences.
    pub fn new(
        inner: C,
        config: HttpRequestHeadersCommand,
        cancel: Option<CancelToken>,
    ) -> McpResult<Self> {
        let _ = resolve_request_headers_command(&config)?;
        Ok(Self { inner, config: Arc::new(config), cancel })
    }

    /// Run the command for one request and return the headers it derived.
    ///
    /// `spawn_blocking` rather than `tokio::process`: the sweep in §6 runs `ps` synchronously and
    /// `SIGSTOP`s a tree between reads, which is a blocking algorithm end to end. Re-resolving the
    /// configuration per call is upstream's behaviour too — `invokeRequestHeadersCommand` calls
    /// `resolvedCommand` again (`request-headers-command.ts:196`), so a `${VAR}` in `args` tracks the
    /// current environment rather than the environment at connect time.
    async fn derive(
        &self,
        method: &str,
        uri: &str,
        body: &[u8],
    ) -> McpResult<Vec<(HeaderName, HeaderValue)>> {
        let envelope = HttpRequestCommandEnvelope::new(method, uri, body);
        let config = Arc::clone(&self.config);
        let cancel = self.cancel.clone();
        match tokio::task::spawn_blocking(move || {
            invoke_request_headers_command(&config, &envelope, cancel.as_ref())
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(command_error(
                "HTTP request headers command failed to start",
            )),
        }
    }
}

/// `headers.set(name, value)` for each derived header (`request-headers-command.ts:325`) — replace,
/// never append. See divergences 2–4 in the module header for what rmcp does with the result.
fn apply_derived(
    custom_headers: &mut HashMap<HeaderName, HeaderValue>,
    derived: Vec<(HeaderName, HeaderValue)>,
) {
    for (name, value) in derived {
        let _ = custom_headers.insert(name, value);
    }
}

/// A failed header command aborts the request. [`StreamableHttpError::Io`] is the honest class: the
/// failure is local — a subprocess, a pipe or a signal — and it is the one variant that carries
/// upstream's sentence through to the user intact.
fn transport_error<E>(error: McpError) -> StreamableHttpError<E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    StreamableHttpError::Io(std::io::Error::other(error.to_string()))
}

impl<C> StreamableHttpClient for RequestHeadersCommandClient<C>
where
    C: StreamableHttpClient + Sync,
{
    type Error = C::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        mut custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        // `request.clone().arrayBuffer()` — the exact bytes the POST will carry. rmcp's reqwest
        // client sends `request.json(&message)`, i.e. `serde_json::to_vec(&message)`, so this is the
        // same document byte for byte.
        let body = serde_json::to_vec(&message).map_err(StreamableHttpError::Deserialize)?;
        let derived = self.derive("POST", &uri, &body).await.map_err(transport_error)?;
        apply_derived(&mut custom_headers, derived);
        self.inner
            .post_message(uri, message, session_id, auth_header, custom_headers)
            .await
    }

    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        mut custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let body = serde_json::to_vec(&message).map_err(StreamableHttpError::Deserialize)?;
        let derived = self.derive("POST", &uri, &body).await.map_err(transport_error)?;
        apply_derived(&mut custom_headers, derived);
        self.inner
            .post_message_with_max_sse_event_size(
                uri,
                message,
                session_id,
                auth_header,
                custom_headers,
                max_sse_event_size,
            )
            .await
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        mut custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        let derived = self.derive("GET", &uri, &[]).await.map_err(transport_error)?;
        apply_derived(&mut custom_headers, derived);
        self.inner
            .get_stream(uri, session_id, last_event_id, auth_header, custom_headers)
            .await
    }

    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        mut custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        let derived = self.derive("GET", &uri, &[]).await.map_err(transport_error)?;
        apply_derived(&mut custom_headers, derived);
        self.inner
            .get_stream_with_max_sse_event_size(
                uri,
                session_id,
                last_event_id,
                auth_header,
                custom_headers,
                max_sse_event_size,
            )
            .await
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        mut custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        let derived = self.derive("DELETE", &uri, &[]).await.map_err(transport_error)?;
        apply_derived(&mut custom_headers, derived);
        self.inner.delete_session(uri, session_id, auth_header, custom_headers).await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn config(command: &str, args: &[&str]) -> HttpRequestHeadersCommand {
        HttpRequestHeadersCommand {
            command: Some(command.to_string()),
            args: Some(args.iter().map(|arg| (*arg).to_string()).collect()),
            env: None,
            timeout_ms: None,
        }
    }

    fn message(error: &McpError) -> String {
        error.to_string()
    }

    // -- `resolvedCommand`'s two reachable throws (upstream test "validates configuration before
    //    issuing a request") ---------------------------------------------------------------------

    #[test]
    fn the_two_reachable_configuration_throws_carry_upstreams_sentences() {
        let blank = HttpRequestHeadersCommand {
            command: Some("   ".to_string()),
            ..HttpRequestHeadersCommand::default()
        };
        assert_eq!(
            message(&resolve_request_headers_command(&blank).unwrap_err()),
            "HTTP request headers command requires a non-empty command"
        );

        // Absent is the same arm as blank: `typeof config.command !== "string"`.
        assert_eq!(
            message(
                &resolve_request_headers_command(&HttpRequestHeadersCommand::default()).unwrap_err()
            ),
            "HTTP request headers command requires a non-empty command"
        );

        for bad in [0.0, -1.0, 60_001.0, 1.5, f64::NAN, f64::INFINITY] {
            let entry = HttpRequestHeadersCommand {
                command: Some("node".to_string()),
                timeout_ms: Some(bad),
                ..HttpRequestHeadersCommand::default()
            };
            assert_eq!(
                message(&resolve_request_headers_command(&entry).unwrap_err()),
                "HTTP request headers command timeoutMs must be an integer between 1 and 60000",
                "timeoutMs {bad} must be refused"
            );
        }

        // The bounds themselves are inclusive, and the default is 10 s.
        for good in [1.0, 60_000.0] {
            let entry = HttpRequestHeadersCommand {
                command: Some("node".to_string()),
                timeout_ms: Some(good),
                ..HttpRequestHeadersCommand::default()
            };
            assert!(resolve_request_headers_command(&entry).is_ok());
        }
        assert_eq!(
            resolve_request_headers_command(&config("node", &[])).unwrap().timeout,
            Duration::from_millis(10_000)
        );
    }

    #[test]
    fn command_args_and_env_are_environment_interpolated() {
        // `interpolateEnvVars` is applied to all three, which is why `computeServerHash` hashes the
        // interpolated form (`metadata-cache.ts:94-101`).
        let entry = HttpRequestHeadersCommand {
            command: Some("${CYRUP_MCP_RHC_TEST_MISSING}signer".to_string()),
            args: Some(vec!["--actor=$env:CYRUP_MCP_RHC_TEST_MISSING".to_string()]),
            env: Some(BTreeMap::from([(
                "ACTOR".to_string(),
                "{env:CYRUP_MCP_RHC_TEST_MISSING}x".to_string(),
            )])),
            timeout_ms: None,
        };
        let resolved = resolve_request_headers_command(&entry).unwrap();
        // A missing variable interpolates to the empty string everywhere except `resolveServerUrl`.
        assert_eq!(resolved.command, "signer");
        assert_eq!(resolved.args, vec!["--actor=".to_string()]);
        assert_eq!(resolved.env.get("ACTOR").map(String::as_str), Some("x"));
    }

    // -- the envelope --------------------------------------------------------------------------

    #[test]
    fn the_envelope_is_versioned_upper_cased_and_carries_the_exact_body() {
        let envelope = HttpRequestCommandEnvelope::new("post", "https://a.example/mcp", b"exact");
        assert_eq!(envelope.version, 1);
        assert_eq!(envelope.method, "POST");
        assert_eq!(envelope.url, "https://a.example/mcp");
        // Padded standard base64 — `Buffer.from(body).toString("base64")`.
        assert_eq!(envelope.body_base64, "ZXhhY3Q=");
        assert_eq!(
            serde_json::to_string(&envelope).unwrap(),
            r#"{"version":1,"method":"POST","url":"https://a.example/mcp","bodyBase64":"ZXhhY3Q="}"#
        );
        assert_eq!(HttpRequestCommandEnvelope::new("GET", "u", b"").body_base64, "");
    }

    // -- the four output-shape rejections (`request-headers-command.ts:272-296`) -----------------

    #[test]
    fn malformed_output_is_rejected_with_upstreams_four_sentences() {
        assert_eq!(
            message(&parse_derived_headers(b"not-json").unwrap_err()),
            "HTTP request headers command returned invalid JSON"
        );
        for not_an_object in ["null", "[]", "\"x\"", "7"] {
            assert_eq!(
                message(&parse_derived_headers(not_an_object.as_bytes()).unwrap_err()),
                "HTTP request headers command must return a JSON object",
                "{not_an_object} is not a JSON object"
            );
        }
        assert_eq!(
            message(&parse_derived_headers(br#"{"x-a":1}"#).unwrap_err()),
            "HTTP request headers command values must be strings"
        );
        // Ordered before the header validation: a non-string value wins over an illegal one.
        assert_eq!(
            message(&parse_derived_headers(b"{\"x-a\":\"\\n\",\"x-b\":1}").unwrap_err()),
            "HTTP request headers command values must be strings"
        );
        assert_eq!(
            message(&parse_derived_headers(b"{\"x a\":\"v\"}").unwrap_err()),
            "HTTP request headers command returned an invalid header"
        );
        assert_eq!(
            message(&parse_derived_headers(b"{\"x-a\":\"bad\\nvalue\"}").unwrap_err()),
            "HTTP request headers command returned an invalid header"
        );

        let headers = parse_derived_headers(br#"{"x-derived":"ok","x-other":"2"}"#).unwrap();
        assert_eq!(headers.len(), 2);
        assert!(headers.iter().any(|(name, value)| name == "x-derived" && value == "ok"));
    }

    // -- the subprocess, end to end -------------------------------------------------------------

    /// A `/bin/sh -c` script standing in for upstream's `process.execPath` + `.mjs` fixture.
    #[cfg(unix)]
    fn sh(script: &str) -> HttpRequestHeadersCommand {
        config("/bin/sh", &["-c", script])
    }

    #[cfg(unix)]
    #[test]
    fn derives_headers_from_the_exact_request_envelope() {
        // Reads the envelope off stdin and echoes the decoded body straight back as a header, which
        // is upstream's `readEnvelope` fixture in one line of shell.
        let entry = HttpRequestHeadersCommand {
            env: Some(BTreeMap::from([("TEST_ACTOR".to_string(), "actor-123".to_string())])),
            ..sh(
                r#"body=$(sed -n 's/.*"bodyBase64":"\([^"]*\)".*/\1/p' | base64 -d)
                   printf '{"x-derived-body":"%s","x-derived-actor":"%s"}' "$body" "$TEST_ACTOR""#,
            )
        };
        let envelope = HttpRequestCommandEnvelope::new("POST", "https://a.example/mcp", b"exact");
        let headers = invoke_request_headers_command(&entry, &envelope, None).unwrap();

        let find = |wanted: &str| {
            headers
                .iter()
                .find(|(name, _)| name == wanted)
                .and_then(|(_, value)| value.to_str().ok())
                .map(str::to_string)
        };
        assert_eq!(find("x-derived-body"), Some("exact".to_string()));
        assert_eq!(find("x-derived-actor"), Some("actor-123".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn fails_closed_when_the_command_exits_unsuccessfully() {
        let envelope = HttpRequestCommandEnvelope::new("POST", "https://a.example/mcp", b"");
        assert_eq!(
            message(
                &invoke_request_headers_command(&sh("exit 7"), &envelope, None).unwrap_err()
            ),
            "HTTP request headers command exited with code 7"
        );
    }

    #[cfg(unix)]
    #[test]
    fn fails_closed_when_the_command_cannot_start() {
        let envelope = HttpRequestCommandEnvelope::new("POST", "https://a.example/mcp", b"");
        let missing = config("/nonexistent/cyrup-mcp-request-headers-command", &[]);
        assert_eq!(
            message(&invoke_request_headers_command(&missing, &envelope, None).unwrap_err()),
            "HTTP request headers command failed to start"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_command_that_outruns_its_timeout_is_killed_and_reported() {
        let entry = HttpRequestHeadersCommand {
            timeout_ms: Some(50.0),
            ..sh("sleep 30")
        };
        let envelope = HttpRequestCommandEnvelope::new("POST", "https://a.example/mcp", b"");
        let started = Instant::now();
        assert_eq!(
            message(&invoke_request_headers_command(&entry, &envelope, None).unwrap_err()),
            "HTTP request headers command timed out after 50ms"
        );
        // The 30-second sleep did not run to completion, i.e. the tree really was killed.
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[cfg(unix)]
    #[test]
    fn output_beyond_64_kib_fails_closed() {
        // 64 KiB of `a` plus one byte: the guard is a strict `>`.
        let entry = sh("yes aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa | head -c 200000");
        let envelope = HttpRequestCommandEnvelope::new("POST", "https://a.example/mcp", b"");
        assert_eq!(
            message(&invoke_request_headers_command(&entry, &envelope, None).unwrap_err()),
            "HTTP request headers command output exceeded 64 KiB"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_already_cancelled_token_aborts_before_the_command_can_answer() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let envelope = HttpRequestCommandEnvelope::new("POST", "https://a.example/mcp", b"");
        assert_eq!(
            message(
                &invoke_request_headers_command(&sh("sleep 30"), &envelope, Some(&cancel))
                    .unwrap_err()
            ),
            "HTTP request headers command aborted"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_detached_helper_does_not_outlive_the_request() {
        // Upstream's "kills helpers when the command returns valid output": the command detaches a
        // helper that would touch the marker 400 ms from now, then answers immediately. The sweep has
        // to find the helper through the cleanup token even though it left the process group.
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("alive");
        let script = format!(
            r#"( setsid sh -c 'sleep 0.4; : > {marker}' & ) >/dev/null 2>&1
               printf '{{"x-derived":"ok"}}'"#,
            marker = marker.display()
        );
        let envelope = HttpRequestCommandEnvelope::new("POST", "https://a.example/mcp", b"");
        let headers = invoke_request_headers_command(&sh(&script), &envelope, None).unwrap();
        assert_eq!(headers.len(), 1);

        std::thread::sleep(Duration::from_millis(900));
        assert!(!marker.exists(), "a detached helper survived the request that spawned it");
    }

    #[cfg(unix)]
    #[test]
    fn process_discovery_failure_fails_closed_before_the_command_is_spawned() {
        // The upstream fault injector, dual-read. `std::env::set_var` is `unsafe` under edition 2024
        // and this crate forbids `unsafe`, so the preflight is exercised through its own function
        // rather than through the variable — the arm under test is identical either way.
        assert!(assert_posix_process_discovery_available().is_ok());
        assert_eq!(
            message(&cleanup_failed("ps exited with code 1")),
            "HTTP request headers command cleanup failed: ps exited with code 1"
        );
    }

    #[cfg(unix)]
    #[test]
    fn descendant_discovery_finds_this_processs_own_children() {
        // Spawn `sleep` DIRECTLY rather than via `/bin/sh -c "sleep 5"`, and null BOTH pipes.
        //
        // Via a shell this test leaked, and whether it leaked depended on which `/bin/sh` the
        // machine has. `bash -c "sleep 5"` execs the simple command, so the spawned pid *is*
        // `sleep` and `child.kill()` reaps it. `dash` — `/bin/sh` on Debian-family images, which
        // is what CI runs — forks instead, so the pid is the shell and `sleep` is a GRANDCHILD:
        // `kill` killed only the shell and left `sleep` orphaned for its full 5s. Because only
        // `stdout` was redirected, that orphan inherited the harness's stderr and held the pipe
        // open, which is exactly the "leaky test" shape `.config/nextest.toml` arms its 500ms
        // detector against — a deterministic LEAK-FAIL on dash, green on bash.
        //
        // No shell means no grandchild to orphan, so `kill` is authoritative; nulling stderr as
        // well as stdout means the child holds no harness pipe even if teardown were to race.
        // The assertion is unchanged: `sleep` is still a direct child of this process.
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let self_pid = i32::try_from(std::process::id()).unwrap();
        let descendants = collect_posix_descendant_pids(self_pid).unwrap();
        let child_pid = i32::try_from(child.id()).unwrap();
        let _ = child.kill();
        let _ = child.wait();
        assert!(
            descendants.contains(&child_pid),
            "the ps walk must see a direct child of this process"
        );
    }
}
