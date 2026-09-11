//! The workflow host-command executor (SCOPE_3e SUBTASK3) — pi `workflows/host-command.ts`
//! (235 LOC), complete. The one module in the SCOPE_3 family that spawns a shell and writes
//! files; the highest-risk code in the family (SCOPE_3e §0.2).
//!
//! # What is deliberately NOT here
//!
//! * **`quoteExecutableForShell` is not ported** (§0.9). On Unix it is the identity function, and
//!   its Windows arm exists only for Node's `spawn(cmdline, { shell: true })` tokenization, which
//!   cyrup does not use — the spawn goes through
//!   [`crate::exec::acceptance::lattice::verify::shell_command`] (`/bin/sh -c` on Unix, `cmd /C`
//!   on Windows). [CYRUP-DELTA, mechanism] the residual: on Windows, an unquoted absolute
//!   executable path containing a space is still tokenized by `cmd.exe`, exactly as it is for
//!   every `verify[]` command this crate already runs; fixing that is one bug for the whole
//!   crate, not a private workaround for this module.
//! * **No second process-tree controller.** Termination reuses
//!   [`crate::spawn::signal::terminate_on_timeout`] — its SIGTERM → 1000 ms → SIGKILL ladder is
//!   upstream's `{ termGraceMs: 1_000 }` controller exactly (§0.16) — and the win32 `taskkill
//!   /PID … /T /F` branch of pi's tree terminator (`host-command.ts:144-151`) is already
//!   `spawn/signal.rs`'s non-Unix stage 3 ([CYRUP-DELTA, mechanism]: same tree kill, one
//!   implementation).
//! * The group-emptiness verification (§0.17) asks upstream's `activeProcessGroupMembers`
//!   question — pi spawns `ps -axo pid=,pgid=,stat=` and regex-matches its stdout — as ONE
//!   syscall: see [`process_group_is_populated`].

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::display_text::truncate_to_bytes;
use crate::exec::output::{ContainedPath, resolve_single_output_claim_path};

/// pi `MAX_COMMAND_BYTES` (`host-command.ts:10`).
const MAX_COMMAND_BYTES: usize = 16 * 1024;
/// pi `MAX_OUTPUT_PATH_BYTES` (`:11`).
const MAX_OUTPUT_PATH_BYTES: usize = 240;
/// pi `MAX_CAPTURE_BYTES` (`:12`) — the combined stdout+stderr capture bound.
const MAX_CAPTURE_BYTES: usize = 1024 * 1024;
/// pi `MAX_PREVIEW_BYTES` (`:13`) — the per-stream preview bound.
const MAX_PREVIEW_BYTES: usize = 4 * 1024;
/// pi `MAX_TIMEOUT_MS` (`:14`).
const MAX_TIMEOUT_MS: u64 = 24 * 60 * 60 * 1000;

/// pi `killVerifyMs` default (`owned-process-tree.ts:5`, `DEFAULT_KILL_VERIFY_MS`) — the
/// group-emptiness verification window after the kill rung.
#[cfg(unix)]
const KILL_VERIFY: std::time::Duration = std::time::Duration::from_millis(1000);
/// pi `termGraceMs: 1_000` — host-command constructs its controller with exactly this grace
/// (`host-command.ts:181`), which also equals [`crate::spawn::signal::TIMEOUT_SIGTERM_GRACE`].
#[cfg(unix)]
const TERM_GRACE: std::time::Duration = std::time::Duration::from_millis(1000);
/// pi `VERIFY_INTERVAL_MS` (`owned-process-tree.ts:6`).
#[cfg(unix)]
const VERIFY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);

/// The literal `kind: "command"` on host-command params and results — pi's `kind: "command"`
/// literal type (`host-command.ts:17`, `:27`), a parse outcome rather than a field every reader
/// re-checks (the [`crate::workflows::SummaryVersion`] pattern).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostCommandKind;

impl HostCommandKind {
    /// The only value this type represents.
    pub const VALUE: &'static str = "command";
}

impl serde::Serialize for HostCommandKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(Self::VALUE)
    }
}

impl<'de> serde::Deserialize<'de> for HostCommandKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        if raw == Self::VALUE {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported host command kind '{raw}' (expected '{}')",
                Self::VALUE
            )))
        }
    }
}

/// pi `WorkflowHostCommandParams["role"]` (`host-command.ts:21`): `"ci" | "gate"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowHostCommandRole {
    /// A CI-shaped command.
    Ci,
    /// A gating command.
    Gate,
}

/// pi `WorkflowHostCommandParams` (`host-command.ts:16-23`), the canonical shape
/// [`normalize_workflow_host_command_params`] produces — that normalizer is the ONLY validating
/// boundary; this struct is plain data past it.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowHostCommandParams {
    /// Always `"command"`.
    pub kind: HostCommandKind,
    /// The shell command line, trimmed, ≤ 16384 UTF-8 bytes, NUL-free.
    pub command: String,
    /// The timeout, an integer 1..=86_400_000 ms.
    pub timeout_ms: u64,
    /// The explicit output path — trimmed, relative, traversal-free, control-free, ≤ 240 bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// The declared role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<WorkflowHostCommandRole>,
    /// The provider tag — trimmed, single-line, ≤ 64 bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

/// pi `WorkflowHostCommandResult["state"]` (`host-command.ts:29`): four mutually exclusive
/// outcomes. A timeout and a cancellation are DISTINCT states, both with `exit_code: None` when
/// the ladder killed the process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkflowHostCommandState {
    /// Exit 0, no spawn error, no cleanup error.
    Passed,
    /// Non-zero exit, spawn failure, or a process-tree cleanup failure (§0.17 — a cleanup error
    /// alone turns an exit-0 command into `Failed`).
    Failed,
    /// The command's own timeout fired.
    TimedOut,
    /// The workflow was aborted while the command ran.
    Stopped,
}

/// pi `WorkflowHostCommandResult` (`host-command.ts:25-36`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowHostCommandResult {
    /// The workflow key this command ran under.
    pub key: String,
    /// Always `"command"`.
    pub kind: HostCommandKind,
    /// `state == Passed`.
    pub ok: bool,
    /// The settled outcome.
    pub state: WorkflowHostCommandState,
    /// The observed exit code — `None` (upstream `null`, always present on the wire) when the
    /// process died of a signal or never spawned.
    pub exit_code: Option<i32>,
    /// The bounded stdout preview (≤ 4 KiB).
    pub stdout: String,
    /// The bounded stderr preview (≤ 4 KiB).
    pub stderr: String,
    /// Where the combined capture was written.
    pub output_path: PathBuf,
    /// Wall-clock milliseconds from spawn attempt to settle.
    pub duration_ms: u64,
    /// The settled error text, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The fields a host-command params object may carry (`host-command.ts:55`).
const PARAM_FIELDS: [&str; 6] = ["kind", "command", "timeoutMs", "output", "role", "provider"];

/// pi's `cwdHint` remediation paragraph (`host-command.ts:56-58`) — appended verbatim (leading
/// space included; it concatenates directly after the `.`) when the unknown-field list contains
/// `cwd`, the single most likely author mistake.
const CWD_HINT: &str = " The host step does not accept per-step cwd; commands and relative \
                        output paths use the workflow cwd. Set cwd on the outer subagent \
                        request, or put a trusted directory change in command (for example, \
                        'cd /path/to/worktree && npm test').";

/// pi `normalizeWorkflowHostCommandParams` (`host-command.ts:52-87`).
///
/// Every rejection is `Err(message)` with `label` interpolated exactly as upstream does, so a
/// frontmatter rejection and a tool-param rejection read differently — the caller chooses the
/// label (the [`crate::exec::turn_budget::resolve_turn_budget_config`] pattern; upstream's
/// default label is `"runs.host params"`).
///
/// `command`, `output` and `provider` are all **trimmed before** their checks and the trimmed
/// value is what is stored — except `provider`'s CR/LF/NUL test, which upstream runs on the RAW
/// value (`:82`), deliberately: a provider that trims clean but arrived multi-line is still
/// rejected. The `output` traversal check splits on **both** separators (pi
/// `output.split(/[\\/]/)`), so a Windows-style `..\x` is caught on Unix too — never
/// `Path::components`, which would see one component named `..\x` and silently pass.
///
/// # Errors
///
/// The ten rejection strings, verbatim and in upstream's order (including the `cwd` remediation
/// paragraph with its leading space).
pub fn normalize_workflow_host_command_params(
    value: &Value,
    label: &str,
) -> Result<WorkflowHostCommandParams, String> {
    let Some(params) = value.as_object() else {
        return Err(format!("{label} must be an object."));
    };
    let unknown_fields: Vec<&str> = params
        .keys()
        .map(String::as_str)
        .filter(|field| !PARAM_FIELDS.contains(field))
        .collect();
    if !unknown_fields.is_empty() {
        let cwd_hint = if unknown_fields.contains(&"cwd") {
            CWD_HINT
        } else {
            ""
        };
        return Err(format!(
            "{label} has unsupported fields: {}.{cwd_hint}",
            unknown_fields.join(", ")
        ));
    }
    if params.get("kind").and_then(Value::as_str) != Some(HostCommandKind::VALUE) {
        return Err(format!("{label}.kind must be 'command'."));
    }
    let command = params.get("command").and_then(Value::as_str);
    let Some(command) = command.map(str::trim).filter(|command| !command.is_empty()) else {
        return Err(format!("{label}.command must be a non-empty string."));
    };
    if command.len() > MAX_COMMAND_BYTES || command.contains('\u{0}') {
        return Err(format!(
            "{label}.command exceeds the {MAX_COMMAND_BYTES}-byte limit or contains NUL."
        ));
    }
    let timeout_ms = params.get("timeoutMs").and_then(Value::as_f64);
    let timeout_ms = match timeout_ms {
        Some(raw)
            if raw.fract() == 0.0
                && raw >= 1.0
                && raw <= {
                    #[allow(clippy::cast_precision_loss)]
                    {
                        MAX_TIMEOUT_MS as f64
                    }
                } =>
        {
            raw as u64
        }
        _ => {
            return Err(format!(
                "{label}.timeoutMs must be an integer from 1 to {MAX_TIMEOUT_MS}."
            ));
        }
    };
    let output = match params.get("output") {
        None => None,
        Some(raw) => {
            let Some(output) = raw
                .as_str()
                .map(str::trim)
                .filter(|output| !output.is_empty())
            else {
                return Err(format!("{label}.output must be a non-empty relative path."));
            };
            let has_traversal = output.split(['\\', '/']).any(|segment| segment == "..");
            let has_control = output
                .chars()
                .any(|c| (c as u32) <= 0x1f || c as u32 == 0x7f);
            if Path::new(output).is_absolute()
                || has_traversal
                || has_control
                || output.len() > MAX_OUTPUT_PATH_BYTES
            {
                return Err(format!(
                    "{label}.output must be a bounded relative path without traversal or \
                     control characters."
                ));
            }
            Some(output.to_string())
        }
    };
    let role = match params.get("role") {
        None => None,
        Some(raw) => match raw.as_str() {
            Some("ci") => Some(WorkflowHostCommandRole::Ci),
            Some("gate") => Some(WorkflowHostCommandRole::Gate),
            _ => return Err(format!("{label}.role must be 'ci' or 'gate'.")),
        },
    };
    let provider = match params.get("provider") {
        None => None,
        Some(raw) => {
            let text = raw.as_str();
            let trimmed = text.map(str::trim).filter(|provider| !provider.is_empty());
            let raw_has_control = text.is_some_and(|text| text.contains(['\r', '\n', '\u{0}']));
            match trimmed {
                Some(provider) if !raw_has_control && provider.len() <= 64 => {
                    Some(provider.to_string())
                }
                _ => {
                    return Err(format!(
                        "{label}.provider must be a non-empty single-line string of at most \
                         64 bytes."
                    ));
                }
            }
        }
    };
    Ok(WorkflowHostCommandParams {
        kind: HostCommandKind,
        command: command.to_string(),
        timeout_ms,
        output,
        role,
        provider,
    })
}

/// pi `appendBounded` (`host-command.ts:89-92`). The early return is the runaway-process guard:
/// once the buffer has reached `limit` bytes, further chunks are DISCARDED rather than
/// concatenated-then-truncated.
///
/// It is a genuine short-circuit only for ASCII, where [`truncate_to_bytes`] lands exactly on
/// `limit`. For multi-byte tails the truncation can stop one or two bytes short, so the next
/// chunk is appended and re-truncated — bounded work per chunk, never unbounded memory growth.
/// Upstream has the identical property; it is not "fixed" into a divergence.
///
/// Chunks decode with [`String::from_utf8_lossy`] — upstream's `chunk.toString("utf8")` also
/// substitutes U+FFFD for invalid sequences, including a multi-byte character split across a
/// chunk boundary. Same behaviour, same artifact.
fn append_bounded(current: String, chunk: &[u8], limit: usize) -> String {
    if current.len() >= limit {
        return current;
    }
    let mut combined = current;
    combined.push_str(&String::from_utf8_lossy(chunk));
    truncate_to_bytes(&combined, limit)
}

/// pi `resolveWorkflowHostOutputClaimPath` (`host-command.ts:94-96`) — the one-line re-export of
/// [`resolve_single_output_claim_path`] upstream keeps, kept here too because the *caller* is the
/// workflow and the name documents which claim is meant.
#[must_use]
pub fn resolve_workflow_host_output_claim_path(output_path: &Path) -> PathBuf {
    resolve_single_output_claim_path(output_path)
}

/// pi `writeExplicitOutput` (`host-command.ts:119-142`) — atomic, private, retried (on non-Unix
/// only).
///
/// `O_WRONLY|O_CREAT|O_EXCL` at mode `0600` into `.cyrup-host-output-{pid}-{uuid}.tmp` **in the
/// destination directory** (a cross-device rename would break atomicity, which is the entire
/// point — never the OS temp directory; this path is deterministic and unlinked on every exit
/// path),
/// write, chmod `0600`, then rename over the destination — retried through
/// [`crate::exec::run_fanout_budget::FILE_SYSTEM_RETRY_DELAYS_MS`] **only on non-Unix**
/// (upstream's `process.platform === "win32" ? DEFAULT_FILE_SYSTEM_RETRY_DELAYS_MS : []`).
///
/// The `finally` (`:137-141`) unlinks the temp file swallowing only `ENOENT` — and upstream's
/// `throw` inside that `finally` REPLACES the pending error/return, so a non-`ENOENT` unlink
/// failure surfaces raw here too, displacing even the wrapped message.
///
/// # Errors
///
/// `runs.host('{key}') could not atomically replace its output.` wrapping any inner failure.
fn write_explicit_output(
    parent: &ContainedPath,
    output_path: &Path,
    capture: &str,
    key: &str,
) -> Result<(), String> {
    let wrapped = || format!("runs.host('{key}') could not atomically replace its output.");
    let Some(name) = output_path.file_name() else {
        return Err(wrapped());
    };
    let destination = parent.join_file_name(name);
    let temporary_name = format!(
        ".cyrup-host-output-{}-{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4()
    );
    let temporary = parent.join_file_name(std::ffi::OsStr::new(&temporary_name));

    let attempt = write_explicit_output_attempt(&temporary, &destination, capture);
    // The `finally`: unlink the temp file on every exit path, swallowing only "already gone".
    let cleanup = match std::fs::remove_file(&temporary) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    };
    match (attempt, cleanup) {
        (result, Ok(())) => result.map_err(|_| wrapped()),
        // A cleanup throw replaces the pending result, as upstream's finally does.
        (_, Err(cleanup_error)) => Err(cleanup_error),
    }
}

/// The fallible body of [`write_explicit_output`]: create-exclusive, write, chmod, rename.
fn write_explicit_output_attempt(
    temporary: &Path,
    destination: &Path,
    capture: &str,
) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(temporary)?;
    file.write_all(capture.as_bytes())?;
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temporary, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(unix)]
    {
        std::fs::rename(temporary, destination)
    }
    #[cfg(not(unix))]
    {
        // Upstream retries the rename only on win32, through the shared delay ladder.
        let mut attempt = 0usize;
        loop {
            match std::fs::rename(temporary, destination) {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let Some(delay) =
                        crate::exec::run_fanout_budget::FILE_SYSTEM_RETRY_DELAYS_MS.get(attempt)
                    else {
                        return Err(error);
                    };
                    attempt = attempt.saturating_add(1);
                    std::thread::sleep(std::time::Duration::from_millis(*delay));
                }
            }
        }
    }
}

/// pi `fs.writeFileSync(outputPath, capture, { encoding: "utf8", mode: 0o600 })`
/// (`host-command.ts:225`) — the **default** output path is written plainly: no temp file, no
/// rename, mode `0600` applied at creation only (§0.18).
///
/// Two writers, deliberately. The atomic writer exists for the EXPLICIT path because that
/// destination lives in the workflow cwd where the just-executed command could have planted a
/// symlink or an existing file; the default path is runtime-computed under the artifacts
/// directory (a sanitized single segment, claimed once), where none of that is reachable. Honest
/// limit: temp+rename also buys crash-safety for concurrent readers, and *that* half would apply
/// here too — the asymmetry is principled on the symlink axis and arguably an upstream
/// inconsistency on the durability axis. It is ported because it is upstream's observable
/// behaviour (the explicit writer raises a distinct error and transiently creates a `.tmp`
/// sibling); unifying the writers would be a behaviour change smuggled into a port.
fn write_default_output(output_path: &Path, capture: &str) -> Result<(), String> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(output_path)
        .map_err(|error| error.to_string())?;
    file.write_all(capture.as_bytes())
        .map_err(|error| error.to_string())
}

/// §0.17's one-syscall group-emptiness probe.
///
/// Does any process remain in `pgid`'s group? pi `activeProcessGroupMembers`
/// (`owned-process-tree.ts:24-35`) answers this by spawning `ps -axo pid=,pgid=,stat=` and
/// regex-matching its stdout. `kill(-pgid, 0)` is the same question as a single syscall: POSIX
/// specifies signal 0 as an existence-and-permission probe that delivers nothing, and `ESRCH`
/// means "no process in that group". No subprocess, no locale dependency, no zombie-state parsing
/// (a zombie is already unsignalable, which is upstream's `stat.startsWith("Z")` filter for
/// free).
///
/// [CYRUP-DELTA, mechanism] — same predicate, one syscall instead of a `ps` scrape.
#[cfg(unix)]
fn process_group_is_populated(pgid: i32) -> bool {
    !matches!(
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(-pgid),
            None::<nix::sys::signal::Signal>
        ),
        Err(nix::errno::Errno::ESRCH)
    )
}

/// pi `waitUntilGroupTerminal` (`owned-process-tree.ts:39-53`): poll
/// [`process_group_is_populated`] every [`VERIFY_INTERVAL`] against a deadline. `true` = the
/// group emptied in time.
#[cfg(unix)]
async fn wait_until_group_empty(pgid: i32, window: std::time::Duration) -> bool {
    let deadline = tokio::time::Instant::now() + window;
    loop {
        if !process_group_is_populated(pgid) {
            return true;
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return false;
        }
        tokio::time::sleep(VERIFY_INTERVAL.min(deadline - now)).await;
    }
}

/// The cleanup verdict after the timeout/cancel ladder has run — pi's `terminal.state ===
/// "unknown" ? terminal.reason : undefined` (`host-command.ts:216`), whose reason words are what
/// the error message interpolates. After [`crate::spawn::signal::terminate_on_timeout`] returns,
/// only the verification remains (the ladder itself already escalated), polled exactly as
/// `waitUntilGroupTerminal` does with the `killVerifyMs` window (§0.17).
#[cfg(unix)]
async fn cleanup_after_termination(pgid: Option<i32>) -> Option<String> {
    match pgid {
        // pi `terminate("timeout")` with no pid resolves `{ state: "unknown", reason:
        // "missing-process-id" }` (`host-command.ts:185-187`).
        None => Some("missing-process-id".to_string()),
        Some(pgid) => {
            if wait_until_group_empty(pgid, KILL_VERIFY).await {
                None
            } else {
                Some("verification-failed".to_string())
            }
        }
    }
}

/// Upstream's `cleanupError` is gated on `process.platform !== "win32"` (`host-command.ts:216`) —
/// on non-Unix it is always absent, so the whole verification compiles away.
#[cfg(not(unix))]
async fn cleanup_after_termination(_pgid: Option<i32>) -> Option<String> {
    None
}

/// The NORMAL exit path's sweep — pi `finishAfterWriterClose` **is** `terminate`
/// (`owned-process-tree.ts:104`, §0.17): even a clean exit SIGTERMs the whole group, verifies,
/// escalates to SIGKILL, and verifies again. A command that exits 0 but leaks a backgrounded
/// descendant that survives both rungs is reported `failed` with reason `verification-failed`.
///
/// This cannot go through [`crate::spawn::signal::terminate_on_timeout`]: on this path the child
/// itself is already reaped, so the `Child` no longer carries a pid — the sweep targets the
/// process GROUP by the pgid captured at spawn (`Command::process_group(0)` makes pgid == pid).
/// It is the §0.17 re-implementation of upstream's controller ladder, not a second controller.
#[cfg(unix)]
async fn cleanup_after_clean_exit(pgid: Option<i32>) -> Option<String> {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    let pgid = pgid?;
    let target = Pid::from_raw(-pgid);
    // Rung 1 — SIGTERM the group. ESRCH means "already empty", which the verification below
    // confirms for free (upstream's "absent" send result flows into the same wait).
    match kill(target, Signal::SIGTERM) {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
        Err(_) => return Some("signal-failed".to_string()),
    }
    if wait_until_group_empty(pgid, TERM_GRACE).await {
        return None;
    }
    // Rung 2 — SIGKILL. A send failure only matters if the group is still populated (pi
    // `owned-process-tree.ts:91-96`).
    match kill(target, Signal::SIGKILL) {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
        Err(_) => {
            if process_group_is_populated(pgid) {
                return Some("signal-failed".to_string());
            }
        }
    }
    if wait_until_group_empty(pgid, KILL_VERIFY).await {
        None
    } else {
        Some("verification-failed".to_string())
    }
}

/// See the Unix twin — upstream's `cleanupError` is absent on non-Unix by its platform gate.
#[cfg(not(unix))]
async fn cleanup_after_clean_exit(_pgid: Option<i32>) -> Option<String> {
    None
}

/// pi `finish`'s state/error derivation (`host-command.ts:212-218`) — ONE pure function computing
/// both, because the precedence is load-bearing and must never be re-expressed as statement order
/// at the call site (§A.1):
///
/// ```text
/// state = timed_out ? TimedOut : stopped ? Stopped
///       : (exit_code == 0 && no spawn error && no cleanup error) ? Passed : Failed
/// error = spawn_error | "Process-tree cleanup failed: {reason}."
///       | TimedOut ⇒ "Command timed out after {timeout_ms}ms."
///       | Stopped  ⇒ "Command stopped because the workflow was aborted."
///       | Failed   ⇒ "Command exited with code {exit_code | unknown}."
///       | Passed   ⇒ None
/// ```
fn settle_workflow_host_command(
    timed_out: bool,
    stopped: bool,
    exit_code: Option<i32>,
    spawn_error: Option<&str>,
    cleanup_error: Option<&str>,
    timeout_ms: u64,
) -> (WorkflowHostCommandState, Option<String>) {
    let state = if timed_out {
        WorkflowHostCommandState::TimedOut
    } else if stopped {
        WorkflowHostCommandState::Stopped
    } else if exit_code == Some(0) && spawn_error.is_none() && cleanup_error.is_none() {
        WorkflowHostCommandState::Passed
    } else {
        WorkflowHostCommandState::Failed
    };
    let error = if let Some(message) = spawn_error {
        Some(message.to_string())
    } else if let Some(reason) = cleanup_error {
        Some(format!("Process-tree cleanup failed: {reason}."))
    } else {
        match state {
            WorkflowHostCommandState::TimedOut => {
                Some(format!("Command timed out after {timeout_ms}ms."))
            }
            WorkflowHostCommandState::Stopped => {
                Some("Command stopped because the workflow was aborted.".to_string())
            }
            WorkflowHostCommandState::Failed => Some(format!(
                "Command exited with code {}.",
                exit_code.map_or_else(|| "unknown".to_string(), |code| code.to_string())
            )),
            WorkflowHostCommandState::Passed => None,
        }
    };
    (state, error)
}

/// Everything the spawn-and-drain phase produced, handed to the settle + write phase.
struct DrainedCommand {
    timed_out: bool,
    stopped: bool,
    exit_code: Option<i32>,
    spawn_error: Option<String>,
    cleanup_error: Option<String>,
    stdout: String,
    stderr: String,
    capture: String,
}

/// Read one chunk from an optional pipe; `Err`/EOF report `0`, which the caller treats as
/// "closed". Disabled-arm safe: with the pipe `None` this pends forever, and the `select!` guard
/// never polls it.
async fn read_chunk<R: tokio::io::AsyncRead + Unpin>(
    pipe: &mut Option<R>,
    buffer: &mut [u8],
) -> usize {
    match pipe.as_mut() {
        Some(pipe) => {
            use tokio::io::AsyncReadExt;
            pipe.read(buffer).await.unwrap_or(0)
        }
        None => std::future::pending().await,
    }
}

/// The spawn/drain/terminate phase of [`execute_workflow_host_command`].
async fn run_and_drain(
    mut child: tokio::process::Child,
    timeout_ms: u64,
    cancel: &cyrup_core::CancelToken,
) -> DrainedCommand {
    let pgid: Option<i32> = child.id().and_then(|pid| i32::try_from(pid).ok());
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let mut stdout_buf = vec![0u8; 65536];
    let mut stderr_buf = vec![0u8; 65536];
    let mut stdout_text = String::new();
    let mut stderr_text = String::new();
    let mut capture = String::new();
    let mut timed_out = false;
    let mut stopped = false;
    let mut exited = false;
    let mut exit_status: Option<std::process::ExitStatus> = None;
    let mut wait_error: Option<String> = None;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);

    // pi's event loop: data events append bounded; the timeout stays armed until settle (so an
    // exited command whose pipes are still held by a descendant can still time out, exactly as
    // upstream's un-cleared timer fires before `close`); an already-cancelled token aborts
    // immediately after spawn (`if (input.signal.aborted) onAbort()`, `:183`).
    loop {
        if timed_out || stopped {
            break;
        }
        if exited && stdout_pipe.is_none() && stderr_pipe.is_none() {
            break;
        }
        let stdout_open = stdout_pipe.is_some();
        let stderr_open = stderr_pipe.is_some();
        tokio::select! {
            biased;
            read = read_chunk(&mut stdout_pipe, &mut stdout_buf), if stdout_open => {
                if read == 0 {
                    stdout_pipe = None;
                } else {
                    let chunk = stdout_buf.get(..read).unwrap_or_default();
                    stdout_text = append_bounded(stdout_text, chunk, MAX_PREVIEW_BYTES);
                    capture = append_bounded(capture, chunk, MAX_CAPTURE_BYTES);
                }
            }
            read = read_chunk(&mut stderr_pipe, &mut stderr_buf), if stderr_open => {
                if read == 0 {
                    stderr_pipe = None;
                } else {
                    let chunk = stderr_buf.get(..read).unwrap_or_default();
                    stderr_text = append_bounded(stderr_text, chunk, MAX_PREVIEW_BYTES);
                    capture = append_bounded(capture, chunk, MAX_CAPTURE_BYTES);
                }
            }
            status = child.wait(), if !exited => {
                exited = true;
                match status {
                    Ok(status) => exit_status = Some(status),
                    // pi routes a wait-side failure through the same `finish(null, error)` arm a
                    // spawn error takes.
                    Err(error) => wait_error = Some(error.to_string()),
                }
            }
            () = tokio::time::sleep_until(deadline), if !timed_out && !stopped => {
                timed_out = true;
            }
            () = cancel.cancelled(), if !timed_out && !stopped => {
                stopped = true;
            }
        }
    }

    let cleanup_error = if timed_out || stopped {
        // pi's tree terminator / `controller.terminate()` (`host-command.ts:144-151`, `:184`):
        // the SIGTERM → 1000 ms → SIGKILL
        // group ladder, confirmed reaped — `spawn/signal.rs`'s ladder verbatim, default grace
        // (§0.16: the constants already coincide; no custom grace).
        if let Ok(status) = crate::spawn::signal::terminate_on_timeout(&mut child).await
            && !exited
        {
            exit_status = Some(status);
        }
        // Capture whatever the dying group flushed — upstream's data events keep firing until
        // `close`. Bounded by the ladder's own grace so a descendant that ESCAPED the group and
        // holds the pipes cannot hang the workflow ([CYRUP-DELTA, mechanism]: upstream's `close`
        // never fires in that case and it hangs; the drain here is deadline-bounded, the same
        // hang-avoidance `exec/acceptance`'s verify runner already applies).
        let drain_deadline =
            tokio::time::Instant::now() + crate::spawn::signal::TIMEOUT_SIGTERM_GRACE;
        loop {
            let stdout_open = stdout_pipe.is_some();
            let stderr_open = stderr_pipe.is_some();
            if !stdout_open && !stderr_open {
                break;
            }
            tokio::select! {
                biased;
                read = read_chunk(&mut stdout_pipe, &mut stdout_buf), if stdout_open => {
                    if read == 0 {
                        stdout_pipe = None;
                    } else {
                        let chunk = stdout_buf.get(..read).unwrap_or_default();
                        stdout_text = append_bounded(stdout_text, chunk, MAX_PREVIEW_BYTES);
                        capture = append_bounded(capture, chunk, MAX_CAPTURE_BYTES);
                    }
                }
                read = read_chunk(&mut stderr_pipe, &mut stderr_buf), if stderr_open => {
                    if read == 0 {
                        stderr_pipe = None;
                    } else {
                        let chunk = stderr_buf.get(..read).unwrap_or_default();
                        stderr_text = append_bounded(stderr_text, chunk, MAX_PREVIEW_BYTES);
                        capture = append_bounded(capture, chunk, MAX_CAPTURE_BYTES);
                    }
                }
                () = tokio::time::sleep_until(drain_deadline) => break,
            }
        }
        cleanup_after_termination(pgid).await
    } else {
        // The NORMAL exit path still sweeps and verifies the group — `finishAfterWriterClose`
        // IS `terminate` (§0.17).
        cleanup_after_clean_exit(pgid).await
    };

    DrainedCommand {
        timed_out,
        stopped,
        exit_code: exit_status.and_then(|status| status.code()),
        spawn_error: wait_error,
        cleanup_error,
        stdout: stdout_text,
        stderr: stderr_text,
        capture,
    }
}

/// pi `executeWorkflowHostCommand` (`host-command.ts:153-235`).
///
/// **Pre-spawn order** (`:161-166`): resolve the output path (explicit ⇒ `cwd.join(output)`,
/// lexically resolved — `normalize` already rejected an absolute or traversing value); for the
/// explicit case reject an output that escapes — or *is* — the workflow cwd; then
/// [`ContainedPath::assert_within`] (explicit) or a plain `create_dir_all` (default).
///
/// **Spawn** goes through [`crate::exec::acceptance::lattice::verify::shell_command`] (§0.9 — see
/// the module doc for why `quoteExecutableForShell` has no port), with the workflow cwd, the
/// INHERITED environment (pi `env: process.env` — never cleared), null stdin, piped
/// stdout/stderr, and `process_group(0)` — the Unix analogue of pi's `detached: true` and what
/// lets `spawn/signal.rs`'s ladder reach descendants (§0.16). Structurally this follows
/// `exec/acceptance/model/verify/run.rs`, the already-reviewed template. A spawn failure is NOT
/// an `Err` — it settles as a `failed` result carrying the spawn error, exactly as upstream's
/// `child.on("error")` → `finish(null, error)` does.
///
/// **After settling**, the three write-side guards run in upstream's order (§0.8):
///
/// 1. a claimed output path that no longer matches ⇒ `output path changed after it was claimed.`;
/// 2. the explicit guard re-runs and its result must EQUAL the pre-spawn one (the
///    [`ContainedPath`] `PartialEq`) ⇒ `output parent changed while the command was running.` —
///    this equality is what catches a command that replaced its own output directory with a
///    symlink to somewhere still *inside* the cwd (both individual checks pass; only the equality
///    fails);
/// 3. the write — [`write_explicit_output`] (atomic) or [`write_default_output`] (plain, §0.18).
///
/// All three failures surface wrapped as
/// `runs.host('{key}') could not save command output: {message}` and as an **`Err`**, never as a
/// `state: "failed"` result — upstream's `reject` vs `resolve` split, and §A.1's line between an
/// expected business outcome (the command failed) and a technical failure that aborts (we could
/// not record what happened).
///
/// # Errors
///
/// Pre-spawn path/containment failures and post-run save failures, each with upstream's message.
pub async fn execute_workflow_host_command(
    key: &str,
    params: &WorkflowHostCommandParams,
    cwd: &Path,
    default_output_path: &Path,
    claimed_output_path: Option<&Path>,
    cancel: &cyrup_core::CancelToken,
) -> Result<WorkflowHostCommandResult, String> {
    // Step 1 — the destination (`:161`). `path.resolve` semantics: `output` is relative by
    // validation, so the join stays inside; the lexical normalization only elides `.` segments.
    let output_path = match &params.output {
        Some(output) => crate::exec::output::normalize_lexically(&cwd.join(output)),
        None => default_output_path.to_path_buf(),
    };
    // Step 2 — the explicit case may not escape, or BE, the workflow cwd (`:162-163`; the empty
    // relative is rejected too — the output may not be the cwd itself).
    if params.output.is_some() {
        let escapes = match output_path.strip_prefix(cwd) {
            Ok(rest) => rest.as_os_str().is_empty(),
            Err(_) => true,
        };
        if escapes {
            return Err(format!(
                "runs.host('{key}') output escapes the workflow cwd."
            ));
        }
    }
    // Step 3 — the guard (explicit) or the plain mkdir (default), both before the spawn.
    let explicit_parent = match &params.output {
        Some(_) => Some(ContainedPath::assert_within(cwd, &output_path, key)?),
        None => {
            if let Some(parent) = output_path.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            None
        }
    };

    let started = std::time::Instant::now();
    let mut command = crate::exec::acceptance::lattice::verify::shell_command(&params.command);
    command.current_dir(cwd);
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    {
        command.process_group(0);
    }

    let drained = match command.spawn() {
        Ok(child) => run_and_drain(child, params.timeout_ms, cancel).await,
        Err(error) => DrainedCommand {
            timed_out: false,
            stopped: false,
            exit_code: None,
            spawn_error: Some(error.to_string()),
            cleanup_error: None,
            stdout: String::new(),
            stderr: String::new(),
            capture: String::new(),
        },
    };

    let (state, error) = settle_workflow_host_command(
        drained.timed_out,
        drained.stopped,
        drained.exit_code,
        drained.spawn_error.as_deref(),
        drained.cleanup_error.as_deref(),
        params.timeout_ms,
    );

    // The three write-side guards (§0.8), all wrapped by one handler.
    let write_result: Result<(), String> = (|| {
        if let Some(claimed) = claimed_output_path
            && resolve_workflow_host_output_claim_path(&output_path) != claimed
        {
            return Err("output path changed after it was claimed.".to_string());
        }
        if let Some(pre_spawn) = &explicit_parent {
            let current = ContainedPath::assert_within(cwd, &output_path, key)?;
            if current != *pre_spawn {
                return Err("output parent changed while the command was running.".to_string());
            }
            write_explicit_output(pre_spawn, &output_path, &drained.capture, key)
        } else {
            write_default_output(&output_path, &drained.capture)
        }
    })();
    if let Err(message) = write_result {
        return Err(format!(
            "runs.host('{key}') could not save command output: {message}"
        ));
    }

    Ok(WorkflowHostCommandResult {
        key: key.to_string(),
        kind: HostCommandKind,
        ok: state == WorkflowHostCommandState::Passed,
        state,
        exit_code: drained.exit_code,
        stdout: drained.stdout,
        stderr: drained.stderr,
        output_path,
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        error,
    })
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use serde_json::json;

    fn params(command: &str, timeout_ms: u64, output: Option<&str>) -> WorkflowHostCommandParams {
        WorkflowHostCommandParams {
            kind: HostCommandKind,
            command: command.to_string(),
            timeout_ms,
            output: output.map(str::to_string),
            role: None,
            provider: None,
        }
    }

    /// The ten rejections, verbatim, including the `cwd` remediation paragraph with its leading
    /// space and the two-separator traversal split.
    #[test]
    fn normalize_rejections_are_verbatim() {
        let label = "runs.host params";
        let cases: [(serde_json::Value, String); 10] = [
            (json!([1]), format!("{label} must be an object.")),
            (
                json!({ "kind": "command", "command": "ls", "timeoutMs": 1000, "cwd": "/x", "extra": 1 }),
                format!("{label} has unsupported fields: cwd, extra.{CWD_HINT}"),
            ),
            (
                json!({ "kind": "shell", "command": "ls", "timeoutMs": 1000 }),
                format!("{label}.kind must be 'command'."),
            ),
            (
                json!({ "kind": "command", "command": "   ", "timeoutMs": 1000 }),
                format!("{label}.command must be a non-empty string."),
            ),
            (
                json!({ "kind": "command", "command": "ls\u{0}x", "timeoutMs": 1000 }),
                format!("{label}.command exceeds the 16384-byte limit or contains NUL."),
            ),
            (
                json!({ "kind": "command", "command": "ls", "timeoutMs": 0 }),
                format!("{label}.timeoutMs must be an integer from 1 to 86400000."),
            ),
            (
                json!({ "kind": "command", "command": "ls", "timeoutMs": 1000, "output": "  " }),
                format!("{label}.output must be a non-empty relative path."),
            ),
            (
                json!({ "kind": "command", "command": "ls", "timeoutMs": 1000, "output": "..\\x" }),
                format!(
                    "{label}.output must be a bounded relative path without traversal or \
                     control characters."
                ),
            ),
            (
                json!({ "kind": "command", "command": "ls", "timeoutMs": 1000, "role": "audit" }),
                format!("{label}.role must be 'ci' or 'gate'."),
            ),
            (
                json!({ "kind": "command", "command": "ls", "timeoutMs": 1000, "provider": "a\nb" }),
                format!(
                    "{label}.provider must be a non-empty single-line string of at most 64 bytes."
                ),
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(
                normalize_workflow_host_command_params(&input, label).expect_err("rejects"),
                expected
            );
        }
        // The command length bound counts UTF-8 bytes after trim.
        let long = "x".repeat(MAX_COMMAND_BYTES + 1);
        assert!(
            normalize_workflow_host_command_params(
                &json!({ "kind": "command", "command": long, "timeoutMs": 1 }),
                label
            )
            .is_err()
        );
        // Fractional and out-of-range timeouts are rejected; an integral f64 is accepted.
        for bad in [json!(1.5), json!(86_400_001u64), json!("1000")] {
            assert!(
                normalize_workflow_host_command_params(
                    &json!({ "kind": "command", "command": "ls", "timeoutMs": bad }),
                    label
                )
                .is_err()
            );
        }
        // Absolute output and control characters are rejected.
        for bad_output in ["/abs/path", "a\u{1}b", &"o".repeat(241)] {
            assert!(
                normalize_workflow_host_command_params(
                    &json!({
                        "kind": "command", "command": "ls", "timeoutMs": 1000,
                        "output": bad_output,
                    }),
                    label
                )
                .is_err(),
                "{bad_output:?}"
            );
        }
    }

    /// The happy path trims and stores; the provider control test runs on the RAW value.
    #[test]
    fn normalize_accepts_and_trims() {
        let normalized = normalize_workflow_host_command_params(
            &json!({
                "kind": "command",
                "command": "  npm test  ",
                "timeoutMs": 60000.0,
                "output": " sub/out.log ",
                "role": "ci",
                "provider": "  github  ",
            }),
            "runs.host params",
        )
        .expect("valid");
        assert_eq!(normalized.command, "npm test");
        assert_eq!(normalized.timeout_ms, 60000);
        assert_eq!(normalized.output.as_deref(), Some("sub/out.log"));
        assert_eq!(normalized.role, Some(WorkflowHostCommandRole::Ci));
        assert_eq!(normalized.provider.as_deref(), Some("github"));
        // A provider that trims clean but arrived multi-line is still rejected (raw test).
        assert!(
            normalize_workflow_host_command_params(
                &json!({
                    "kind": "command", "command": "ls", "timeoutMs": 1,
                    "provider": "github\n",
                }),
                "runs.host params"
            )
            .is_err()
        );
    }

    /// `appendBounded`: chunks are DISCARDED once the buffer is full; a partial append still
    /// reserves room for the ellipsis.
    #[test]
    fn append_bounded_discards_once_full() {
        let full = append_bounded(String::new(), &[b'a'; 100], 10);
        assert_eq!(full, "aaaaaaa...");
        assert_eq!(full.len(), 10);
        let unchanged = append_bounded(full.clone(), b"more", 10);
        assert_eq!(unchanged, full, "at the limit, further chunks are dropped");
        let grown = append_bounded("ab".to_string(), b"cd", 10);
        assert_eq!(grown, "abcd");
    }

    /// The settle precedence: spawn error ▸ cleanup error ▸ state-derived message; a cleanup
    /// error alone turns exit-0 into `Failed` (§0.17); timeout and cancel are distinct states.
    #[test]
    fn settle_precedence_is_one_function() {
        assert_eq!(
            settle_workflow_host_command(false, false, Some(0), None, None, 5),
            (WorkflowHostCommandState::Passed, None)
        );
        assert_eq!(
            settle_workflow_host_command(false, false, Some(3), None, None, 5),
            (
                WorkflowHostCommandState::Failed,
                Some("Command exited with code 3.".to_string())
            )
        );
        assert_eq!(
            settle_workflow_host_command(false, false, None, None, None, 5),
            (
                WorkflowHostCommandState::Failed,
                Some("Command exited with code unknown.".to_string())
            )
        );
        assert_eq!(
            settle_workflow_host_command(
                false,
                false,
                Some(0),
                None,
                Some("verification-failed"),
                5
            ),
            (
                WorkflowHostCommandState::Failed,
                Some("Process-tree cleanup failed: verification-failed.".to_string())
            )
        );
        assert_eq!(
            settle_workflow_host_command(false, false, Some(0), Some("spawn boom"), None, 5),
            (
                WorkflowHostCommandState::Failed,
                Some("spawn boom".to_string())
            )
        );
        assert_eq!(
            settle_workflow_host_command(true, false, None, None, None, 250),
            (
                WorkflowHostCommandState::TimedOut,
                Some("Command timed out after 250ms.".to_string())
            )
        );
        assert_eq!(
            settle_workflow_host_command(false, true, None, None, None, 250),
            (
                WorkflowHostCommandState::Stopped,
                Some("Command stopped because the workflow was aborted.".to_string())
            )
        );
        // spawn_error outranks the timeout message even in a timed-out state.
        assert_eq!(
            settle_workflow_host_command(true, false, None, Some("boom"), None, 250),
            (WorkflowHostCommandState::TimedOut, Some("boom".to_string()))
        );
    }

    /// Happy path: exit 0, bounded previews, the capture written to the DEFAULT path plainly at
    /// mode 0600, and a clean group sweep.
    #[tokio::test]
    async fn executes_and_writes_the_default_output() {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("outputs/host/key.log");
        let result = execute_workflow_host_command(
            "gate",
            &params("printf 'out'; printf 'err' >&2", 30_000, None),
            dir.path(),
            &output,
            None,
            &cyrup_core::CancelToken::new(),
        )
        .await
        .expect("resolves");
        assert_eq!(result.state, WorkflowHostCommandState::Passed);
        assert!(result.ok);
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stdout, "out");
        assert_eq!(result.stderr, "err");
        assert_eq!(result.error, None);
        assert_eq!(result.output_path, output);
        let written = std::fs::read_to_string(&output).expect("written");
        assert_eq!(written.len(), 6, "the capture holds both streams");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&output)
                .expect("meta")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    /// A non-zero exit settles `failed` with upstream's message; the output is still written.
    #[tokio::test]
    async fn a_failing_command_settles_failed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("out.log");
        let result = execute_workflow_host_command(
            "gate",
            &params("exit 3", 30_000, None),
            dir.path(),
            &output,
            None,
            &cyrup_core::CancelToken::new(),
        )
        .await
        .expect("resolves");
        assert_eq!(result.state, WorkflowHostCommandState::Failed);
        assert!(!result.ok);
        assert_eq!(result.exit_code, Some(3));
        assert_eq!(result.error.as_deref(), Some("Command exited with code 3."));
        assert!(output.exists());
    }

    /// A spawn failure is a `failed` RESULT (upstream `finish(null, error)`), not an `Err`.
    #[tokio::test]
    async fn a_spawn_failure_is_a_result_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing_cwd = dir.path().join("nope");
        let output = dir.path().join("out.log");
        let result = execute_workflow_host_command(
            "gate",
            &params("true", 30_000, None),
            &missing_cwd,
            &output,
            None,
            &cyrup_core::CancelToken::new(),
        )
        .await
        .expect("resolves with a failed state");
        assert_eq!(result.state, WorkflowHostCommandState::Failed);
        assert_eq!(result.exit_code, None);
        assert!(result.error.is_some());
    }

    /// A timeout: distinct state, `exit_code: None`, the ladder confirms the group dead.
    #[tokio::test]
    async fn a_timeout_settles_timed_out() {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("out.log");
        let result = execute_workflow_host_command(
            "gate",
            &params("sleep 30", 100, None),
            dir.path(),
            &output,
            None,
            &cyrup_core::CancelToken::new(),
        )
        .await
        .expect("resolves");
        assert_eq!(result.state, WorkflowHostCommandState::TimedOut);
        assert_eq!(result.exit_code, None);
        assert_eq!(
            result.error.as_deref(),
            Some("Command timed out after 100ms.")
        );
    }

    /// A pre-cancelled token: the distinct `stopped` state, immediately.
    #[tokio::test]
    async fn a_cancelled_token_settles_stopped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("out.log");
        let cancel = cyrup_core::CancelToken::new();
        cancel.cancel();
        let result = execute_workflow_host_command(
            "gate",
            &params("sleep 30", 30_000, None),
            dir.path(),
            &output,
            None,
            &cancel,
        )
        .await
        .expect("resolves");
        assert_eq!(result.state, WorkflowHostCommandState::Stopped);
        assert_eq!(result.exit_code, None);
        assert_eq!(
            result.error.as_deref(),
            Some("Command stopped because the workflow was aborted.")
        );
    }

    /// The explicit output path: resolved against the cwd, guarded, written atomically.
    #[tokio::test]
    async fn an_explicit_output_writes_atomically_inside_the_cwd() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = execute_workflow_host_command(
            "gate",
            &params("printf 'captured'", 30_000, Some("reports/out.log")),
            dir.path(),
            &dir.path().join("unused-default.log"),
            None,
            &cyrup_core::CancelToken::new(),
        )
        .await
        .expect("resolves");
        assert_eq!(result.state, WorkflowHostCommandState::Passed);
        let written = std::fs::read_to_string(dir.path().join("reports/out.log")).expect("written");
        assert_eq!(written, "captured");
        assert!(
            !dir.path().join("unused-default.log").exists(),
            "the explicit path replaces the default"
        );
        // No temp file remains.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("reports"))
            .expect("read dir")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    /// The dot output: validation admits `"."` but the escape check rejects it — the output may
    /// not BE the cwd.
    #[tokio::test]
    async fn an_output_that_is_the_cwd_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = execute_workflow_host_command(
            "gate",
            &params("true", 30_000, Some(".")),
            dir.path(),
            &dir.path().join("default.log"),
            None,
            &cyrup_core::CancelToken::new(),
        )
        .await
        .expect_err("rejects");
        assert_eq!(err, "runs.host('gate') output escapes the workflow cwd.");
    }

    /// The claim guard: a claimed path that no longer matches is an `Err`, wrapped once.
    #[tokio::test]
    async fn a_claim_mismatch_is_a_wrapped_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("out.log");
        let err = execute_workflow_host_command(
            "gate",
            &params("true", 30_000, None),
            dir.path(),
            &output,
            Some(Path::new("/somewhere/else.log")),
            &cyrup_core::CancelToken::new(),
        )
        .await
        .expect_err("rejects");
        assert_eq!(
            err,
            "runs.host('gate') could not save command output: output path changed after it \
             was claimed."
        );
    }

    /// A matching claim (resolved through the same helper) passes.
    #[tokio::test]
    async fn a_matching_claim_passes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("outputs/key.log");
        let claimed = resolve_workflow_host_output_claim_path(&output);
        let result = execute_workflow_host_command(
            "gate",
            &params("printf 'x'", 30_000, None),
            dir.path(),
            &output,
            Some(&claimed),
            &cyrup_core::CancelToken::new(),
        )
        .await
        .expect("resolves");
        assert_eq!(result.state, WorkflowHostCommandState::Passed);
    }

    /// §0.17 on the normal path: an exit-0 command that backgrounds a child still passes because
    /// the sweep SIGTERMs the group and verifies it empty — and the leaked child is actually
    /// gone afterwards. (The leaked child's stdio is redirected away: a descendant that KEEPS the
    /// pipes open delays settle until the timeout, upstream's own `close`-event behaviour.)
    #[cfg(unix)]
    #[tokio::test]
    async fn the_clean_exit_sweep_reaps_leaked_descendants() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("leaked.pid");
        let command = format!(
            "sleep 30 >/dev/null 2>&1 & echo $! > {} ; printf done",
            pid_file.display()
        );
        let result = execute_workflow_host_command(
            "gate",
            &params(&command, 30_000, None),
            dir.path(),
            &dir.path().join("out.log"),
            None,
            &cyrup_core::CancelToken::new(),
        )
        .await
        .expect("resolves");
        assert_eq!(
            result.state,
            WorkflowHostCommandState::Passed,
            "{:?}",
            result.error
        );
        let leaked: i32 = std::fs::read_to_string(&pid_file)
            .expect("pid recorded")
            .trim()
            .parse()
            .expect("a pid");
        // The sweep's SIGTERM reached the backgrounded sleep through the group.
        assert!(
            matches!(
                nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(leaked),
                    None::<nix::sys::signal::Signal>
                ),
                Err(nix::errno::Errno::ESRCH)
            ),
            "the leaked descendant was reaped by the clean-exit sweep"
        );
    }

    /// The group probe is one syscall and answers exactly the `ps` question.
    #[cfg(unix)]
    #[test]
    fn the_group_probe_reports_emptiness() {
        assert!(
            !process_group_is_populated(i32::MAX - 1),
            "an unused pgid reports empty (ESRCH)"
        );
        // Our own process group is populated by definition.
        let own_group = nix::unistd::getpgrp().as_raw();
        assert!(process_group_is_populated(own_group));
    }

    /// The wire shape: kebab-case state, `exitCode: null` always present, `error` omitted when
    /// absent.
    #[test]
    fn result_serializes_with_upstream_wire_words() {
        let result = WorkflowHostCommandResult {
            key: "k".to_string(),
            kind: HostCommandKind,
            ok: false,
            state: WorkflowHostCommandState::TimedOut,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            output_path: PathBuf::from("/tmp/x.log"),
            duration_ms: 5,
            error: None,
        };
        let json = serde_json::to_value(&result).expect("serializes");
        assert_eq!(json["state"], "timed-out");
        assert_eq!(json["kind"], "command");
        assert!(json["exitCode"].is_null(), "exitCode is present-and-null");
        assert!(json.get("error").is_none());
    }
}
