//! The literal OS-subprocess spawn boundary: binary resolution, argv/env construction, bounded
//! parallel fan-out over real child processes, the linear chain/workflow graph, the depth-
//! recursion guard, git-worktree cwd isolation, and signal-escalation cancellation (func-SA
//! §5.3; arch-SA §6.4).
//!
//! # The mandated mechanism, concretely, in this file
//!
//! Every requirement this module implements traces back to func-SA §1.1's binding, non-negotiable
//! mechanism: a subagent run is ALWAYS a genuine OS subprocess re-exec of the `cyrup` binary
//! itself. [`resolve_spawn_command`] resolves *which* binary via `std::env::current_exe()` (with a
//! `CYRUP_SUBAGENT_BINARY` override), [`ChildSpawnSpec`] describes exactly how to invoke it, and
//! [`SpawnedChild`] wraps the resulting real `tokio::process::Command` child, reading its stdout
//! as NDJSON one line at a time — never an in-process object graph, never an in-process
//! nested-agent turn loop, never an event-relay standing in for the child's own execution.
//! Cancellation delegates entirely to [`crate::spawn::signal::terminate`]'s real SIGINT->SIGTERM
//! ->SIGKILL OS-signal escalation (R-SA-059) — this module never invents a second, competing
//! termination mechanism.
//!
//! This module owns the single-child spawn boundary itself (R-SA-045/046/047/048/057/058/067/068)
//! plus, via [`parallel`], the bounded-concurrency fan-out built directly on top of it
//! (R-SA-049/050/051/066/069); [`chain_graph`] and [`worktree`] are siblings built on top of
//! those same primitives.

/// Env-var-based depth-propagation guard (canonical; R-SA-054/055/056). See [`depth`] for the
/// sole algorithm every other depth reference in this crate (and this spec) defers to.
pub mod depth;

/// Bounded `Semaphore`-gated worker pool fan-out over real child OS processes
/// (R-SA-049/050/051/066/069). See [`parallel`] for the sole bounded-concurrency-over-real-
/// subprocesses primitive this crate introduces.
pub mod parallel;

/// SIGINT -> SIGTERM -> SIGKILL kill-escalation state machine (R-SA-059). See [`signal`] for the
/// sole `terminate()` implementation [`SpawnedChild::terminate`] delegates to.
pub mod signal;

/// Git-worktree cwd isolation for `worktree: true` parallel fan-out groups (R-SA-060..065). See
/// [`worktree`] for the dirty-tree precondition check, per-task worktree creation, diff harvest,
/// best-effort cleanup, and the optional per-worktree setup-hook JSON stdin/stdout contract.
pub mod worktree;

/// Nested-run ancestry path addressing (`CYRUP_SUBAGENT_PARENT_PATH`): safe-id validation,
/// sanitization, and env encode/decode. See [`nested_path`]; a faithful port of pi's
/// `runs/shared/nested-path.ts`.
pub mod nested_path;

/// Nested-run event relay + capability-gated control routing (C17): the [`nested_events::NestedRoute`]
/// event-sink/control-inbox protocol, capability tokens, fanout-child authorization env, and the
/// grandparent [`nested_events::project_nested_events`] registry projection. A faithful port of pi's
/// `runs/shared/nested-events.ts`.
pub mod nested_events;

/// The linear chain/workflow-graph walker (R-SA-052/053): `RunnerStep` (`SingleStep |
/// ParallelGroup | DynamicGroup`), `ChainGraph = Vec<RunnerStep>`, and `walk_chain`'s strict
/// fold-over-the-list dispatch, delegating group fan-out to [`parallel::run_bounded`] (and, for
/// `worktree: true` groups, [`worktree::setup_worktree_group`]) rather than re-implementing
/// either. See [`chain_graph`] for the sole `RunnerStep` definition every other module
/// (`discovery::types::ChainDefinition::steps`, `discovery::chains`/`discovery::management`, the
/// background runner-config file, a later phase's `exec/`) references rather than re-declaring.
pub mod chain_graph;

/// Deterministic subagent↔supervisor intercom addressing (a port of pi's
/// `intercom/intercom-bridge.ts:83-97`): [`intercom_target::resolve_subagent_intercom_target`] (each
/// child's own broker presence label + the parent's steer address) and
/// [`intercom_target::orchestrator_presence_target`] (a supervisor's own presence target), plus the
/// child-bridge identity env-var names the spawn overlay writes. Lives here (not in `cyrup-intercom`)
/// because the dependency edge runs `cyrup-intercom -> cyrup-ext-subagents`, and the parent-side
/// target computation runs at this crate's spawn site.
pub mod intercom_target;

/// Per-item dynamic fan-out semantics (R-SA-053 / C16): a faithful port of pi's
/// `runs/shared/dynamic-fanout.ts` supplying [`chain_graph::walk_chain`]'s `DynamicGroup` arm with
/// per-element `{item}`/`{item.path}` template substitution, `expand.key` item keys, the `maxItems`
/// cap, `onEmpty`, duplicate-key/colliding-id detection, and the collect-record shape + aggregate
/// schema validation. See [`dynamic_fanout`] for the pure, taxonomy-agnostic helpers the walker and
/// the chain-parse validator both drive.
pub mod dynamic_fanout;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cyrup_core::CancelToken;
use tokio::io::{AsyncBufReadExt, BufReader, Lines};
use tokio::process::{ChildStderr, ChildStdout};

use crate::error::SubagentError;
use crate::jsonl::BoundedJsonlWriter;

/// Threshold (characters) above which the task prompt MUST be written to a temp file and passed
/// as an `@<tempfile>` argv reference instead of a literal argument, to stay well clear of OS
/// argv-length limits (R-SA-047; func-SA target: 8000 characters).
pub const TASK_ARGV_INLINE_THRESHOLD: usize = 8000;

/// Bounded wait for a child that has emitted a final message but neither exits nor releases
/// stdio promptly afterward (R-SA-068) — long enough to absorb ordinary process-teardown latency
/// (flushing buffered writes, closing file descriptors) without masking a genuinely hung child
/// behind an indefinite wait.
pub const FINAL_DRAIN_TIMEOUT: Duration = Duration::from_millis(2000);

/// The env var carrying an override for which binary [`resolve_spawn_command`] re-execs
/// (R-SA-045 tier 1). Mirrors pi-subagents' `PI_SUBAGENT_PI_BINARY`.
pub const SUBAGENT_BINARY_ENV_VAR: &str = "CYRUP_SUBAGENT_BINARY";

/// The literal fallback executable name (R-SA-045 tier 3), resolved via `PATH` by the OS/`tokio`
/// when `current_exe()` itself fails.
const FALLBACK_BINARY_NAME: &str = "cyrup";

/// The resolved target binary + any base argv this crate's own build of `cyrup` always needs
/// ahead of the per-run arguments [`ChildSpawnSpec`] appends (R-SA-045).
///
/// `base_args` is currently always empty for cyrup (a single statically linked binary with no
/// interpreter/wrapper-script indirection to thread through, unlike pi-subagents' Node/Windows
/// `.cmd` dance) — the field is retained so a future platform-specific wrapper requirement (e.g.
/// a Windows launcher shim) has a place to attach extra leading argv without changing every call
/// site that constructs a [`ChildSpawnSpec`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnCommand {
    /// The resolved, executable path (or bare name, in the tier-3 `PATH`-lookup fallback case).
    pub binary: PathBuf,
    /// Argv entries that must precede every per-run argument this crate appends.
    pub base_args: Vec<String>,
}

/// Resolve which `cyrup` binary a spawned child re-execs, per R-SA-045's three-tier priority:
///
/// 1. `CYRUP_SUBAGENT_BINARY`, if set and non-blank (verbatim, no further resolution) — the
///    override escape hatch this crate's own tests use to substitute a scripted test binary.
/// 2. `std::env::current_exe()`, canonicalized to an absolute path — the default, production
///    path: a subagent always re-execs the exact binary that is currently running.
/// 3. The literal string `"cyrup"`, resolved via `PATH` at spawn time — only reached if
///    `current_exe()` itself fails (a rare, platform-dependent I/O failure), never treated as a
///    hard error since a `PATH`-relative fallback is still a reasonable last resort per R-SA-045's
///    own text.
///
/// This function never fails: every tier either produces a usable [`SpawnCommand`] or falls
/// through to the next, and the final tier is infallible by construction.
#[must_use]
pub fn resolve_spawn_command() -> SpawnCommand {
    resolve_spawn_command_from(|key| std::env::var(key).ok(), std::env::current_exe)
}

/// The pure core of [`resolve_spawn_command`], parameterized over the env lookup and
/// `current_exe` resolver so the three-tier priority can be exercised deterministically in unit
/// tests without mutating real process environment state (`std::env::set_var`/`remove_var` are
/// `unsafe` as of the 2024 edition; this crate is `#![forbid(unsafe_code)]`, so tests inject
/// lookup/resolver closures instead of touching the real environment at all — mirrors
/// `spawn::depth`'s identical `resolve_effective_depth`/`resolve_effective_depth_from` split).
fn resolve_spawn_command_from(
    env_lookup: impl Fn(&str) -> Option<String>,
    current_exe: impl FnOnce() -> std::io::Result<PathBuf>,
) -> SpawnCommand {
    if let Some(bin) = env_lookup(SUBAGENT_BINARY_ENV_VAR)
        && !bin.trim().is_empty()
    {
        return SpawnCommand {
            binary: PathBuf::from(bin),
            base_args: Vec::new(),
        };
    }
    if let Ok(exe) = current_exe() {
        return SpawnCommand {
            binary: exe,
            base_args: Vec::new(),
        };
    }
    SpawnCommand {
        binary: PathBuf::from(FALLBACK_BINARY_NAME),
        base_args: Vec::new(),
    }
}

/// Everything needed to spawn exactly one child subprocess (R-SA-045/046/047/048).
///
/// Built by the caller (the foreground executor, a parallel-fan-out worker, or the background
/// runner's step driver — none of which are implemented in this file) and consumed by
/// [`SpawnedChild::spawn`]. Every field here maps to one concrete piece of the spawn contract:
/// `command` is *what* binary to run (R-SA-045), `args` and `task`/`task_arg` together are the
/// argv base contract (R-SA-047), `env_overlay` is the overlay half of the inherit-then-overlay
/// ordering (R-SA-048; the "inherit" half is `tokio::process::Command`'s own default behavior —
/// see [`SpawnedChild::spawn`]'s doc comment), `cwd` is the working directory (worktree isolation,
/// when present, sets this to a dedicated worktree path rather than a shared cwd), and
/// `temp_files` tracks any `@<tempfile>` argv reference so it can be cleaned up on every exit path
/// (R-SA-067).
#[derive(Debug, Clone)]
pub struct ChildSpawnSpec {
    /// The resolved binary + any mandatory leading argv (R-SA-045).
    pub command: SpawnCommand,
    /// Argv entries appended after `command.base_args`, in order — everything except the task
    /// prompt itself (mode flags, `--model`, tools-allowlist flag, `--session`, etc.). The task
    /// prompt argument (literal or `@<tempfile>`) is appended last by [`ChildSpawnSpec::build_argv`]
    /// so every call site constructs it via [`ChildSpawnSpec::with_task`] rather than
    /// hand-assembling the ordering itself.
    pub args: Vec<String>,
    /// The task prompt, already resolved to its final argv form: either the literal prompt text
    /// (short enough to pass directly) or an `@<tempfile>` reference (R-SA-047, task length over
    /// [`TASK_ARGV_INLINE_THRESHOLD`] characters).
    pub task_arg: String,
    /// Environment overlay applied ON TOP of the child's fully inherited environment (R-SA-048).
    /// This crate MUST NEVER call `env_clear()` anywhere in the spawn path — see
    /// [`SpawnedChild::spawn`]'s doc comment for exactly how inherit-then-overlay ordering is
    /// achieved.
    pub env_overlay: HashMap<String, String>,
    /// The child's working directory.
    pub cwd: PathBuf,
    /// Temp files created while building this spec (long task text written to disk per
    /// R-SA-047, or a system-prompt-override temp file) that MUST be removed after the child
    /// exits, on both the success and failure paths (R-SA-067). [`SpawnedChild::spawn`] takes
    /// ownership of this list and [`SpawnedChild::terminate`]/the drain-then-exit path in
    /// [`SpawnedChild::spawn`]'s caller are responsible for invoking [`cleanup_temp_files`] — see
    /// that function's doc comment for the exact contract.
    pub temp_files: Vec<PathBuf>,
}

impl ChildSpawnSpec {
    /// Resolves `task` into its final argv form per R-SA-047: a literal argument when short
    /// enough, or an `@<tempfile>` reference (writing `task` to a freshly created temp file
    /// registered in `temp_files`) when `task.chars().count()` exceeds
    /// [`TASK_ARGV_INLINE_THRESHOLD`].
    ///
    /// `temp_dir` is the directory the temp file is created in (callers normally pass a
    /// per-run scratch directory so cleanup can be verified/scoped independently of the OS-wide
    /// temp directory).
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::Spawn`] if the temp file cannot be created or written.
    pub fn resolve_task_arg(
        task: &str,
        temp_dir: &Path,
    ) -> Result<(String, Option<PathBuf>), SubagentError> {
        if task.chars().count() <= TASK_ARGV_INLINE_THRESHOLD {
            return Ok((task.to_string(), None));
        }
        let file_name = format!("subagent-task-{}.txt", uuid::Uuid::now_v7().as_simple());
        let path = temp_dir.join(file_name);
        std::fs::write(&path, task).map_err(SubagentError::Spawn)?;
        let arg = format!("@{}", path.display());
        Ok((arg, Some(path)))
    }

    /// Full argv for `tokio::process::Command::args`, in the mandated order: `command.base_args`,
    /// then `args`, then the task prompt argument last.
    #[must_use]
    pub fn build_argv(&self) -> Vec<String> {
        let mut argv = Vec::with_capacity(self.command.base_args.len() + self.args.len() + 1);
        argv.extend(self.command.base_args.iter().cloned());
        argv.extend(self.args.iter().cloned());
        argv.push(self.task_arg.clone());
        argv
    }
}

/// One NDJSON line from a spawned child's stdout (R-SA-057), parsed against `cyrup`'s own native
/// JSON-mode wire-event shape (`--mode json`, `cyrup-modes/src/print.rs`'s JSONL emitter).
///
/// This is deliberately a narrow, tolerant view: this crate has ZERO dependency on `cyrup-agent`
/// (arch-SA §2.1), so it does not import that crate's own event enum — instead it captures only
/// the handful of fields the spawn boundary and its immediate callers need (progress bookkeeping,
/// usage accumulation, tool-call counting), tagged by a `type` discriminant, with every other
/// event shape degrading to [`NdjsonEvent::Unknown`] rather than a parse error. A fuller,
/// purpose-built typed union for the foreground executor's own use (final-output extraction,
/// acceptance-report scanning, structured-output validation) is `exec/ndjson.rs`'s
/// `SubagentEvent` — a later phase of this crate's build-out (arch-SA §2.2) — which this module
/// does not depend on and does not attempt to replace; [`NdjsonEvent`] is scoped strictly to what
/// the raw spawn boundary itself needs to observe.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NdjsonEvent {
    /// A tool call started executing.
    ToolExecutionStart {
        /// The tool call's correlation id.
        call_id: String,
        /// The tool name.
        name: String,
    },
    /// A tool call finished executing.
    ToolExecutionEnd {
        /// The tool call's correlation id.
        call_id: String,
        /// Whether the tool call ended in an error.
        #[serde(default)]
        is_error: bool,
    },
    /// An assistant message completed, carrying token/cost usage for this turn.
    MessageEnd {
        /// The raw message payload (left as an opaque JSON value here; typed message parsing is
        /// `exec/ndjson.rs`'s concern, not the spawn boundary's).
        message: serde_json::Value,
        /// Per-message usage accounting, when present.
        #[serde(default)]
        usage: Option<serde_json::Value>,
    },
    /// Any event shape this narrow view does not specifically recognize. Never a parse error —
    /// an unrecognized `type` tag (or a shape this enum simply hasn't been taught yet) degrades
    /// here rather than aborting NDJSON consumption (R-SA-026's tolerance principle, applied at
    /// this layer too even though R-SA-026 itself is `exec/ndjson.rs`'s direct responsibility).
    #[serde(other)]
    Unknown,
}

/// One live spawned child; owns the kill-escalation state machine (R-SA-059) and the raw NDJSON
/// stdout read loop (R-SA-057/058).
///
/// Per arch-SA §5.1's ownership invariant, a `SpawnedChild` is owned by exactly one task at a
/// time — never shared bare across threads (mirrors arch-08 §5.1's "single-thread-per-Store"
/// invariant for WASM instances, applied here to child-process ownership instead).
/// One step of a child's stdout read loop, as returned by [`SpawnedChild::next_event_or_exit`].
///
/// [`SpawnedChild::next_event`] can only ever report "a line" or "EOF", which silently assumes EOF
/// always arrives once the child is done. It does not: stdout's write end is inherited by every
/// descendant, so a child that exits while a surviving grandchild still holds that pipe produces
/// **no EOF at all**. A read loop with only those two outcomes waits forever on something that can
/// never happen. [`Self::Exited`] is the missing third outcome.
#[derive(Debug)]
pub enum ChildStep {
    /// A stdout line arrived — or reading/teeing it failed, exactly as
    /// [`SpawnedChild::next_event`] reports it.
    Line(Result<NdjsonLine, SubagentError>),
    /// stdout reached EOF: the ordinary end of a well-behaved child.
    Eof,
    /// The process exited while its stdout is STILL OPEN (a surviving grandchild inherited the
    /// write end). Buffered lines written before the exit may still be readable, so the caller
    /// should keep draining under a bounded window rather than stopping here — but it must no
    /// longer wait on an EOF that will never come.
    Exited(std::io::Result<std::process::ExitStatus>),
}

pub struct SpawnedChild {
    child: tokio::process::Child,
    stdout_lines: Lines<BufReader<ChildStdout>>,
    /// The child's stderr reader. R-SA-046: stderr is diagnostic, never protocol data — but on a
    /// non-zero exit its trailing content is surfaced into the run's error (pi `execution.ts:686`),
    /// so the executor moves it out via [`SpawnedChild::take_stderr`] before consuming the child and
    /// drains the orphaned reader to EOF afterward (the dead child's closed write end guarantees a
    /// prompt EOF). `None` once taken.
    stderr_lines: Option<Lines<BufReader<ChildStderr>>>,
    /// Raw NDJSON stdout lines, teed unmodified as they are read (R-SA-058), written lazily via
    /// [`SpawnedChild::next_event`] rather than buffered and flushed at exit. Size-capped at
    /// [`crate::jsonl::DEFAULT_JSONL_CAP_BYTES`] per file (R-SA-136/146): once the cap is reached,
    /// further lines are silently dropped without erroring this run or corrupting the lines
    /// already written — see [`BoundedJsonlWriter`] for the exact contract.
    jsonl_writer: BoundedJsonlWriter,
    /// Temp files (long task text, system-prompt overrides) to remove once this child exits, on
    /// both the success and failure paths (R-SA-067) — see [`cleanup_temp_files`].
    temp_files: Vec<PathBuf>,
    /// Whether the underlying process has been confirmed to have exited (stdout hit EOF and
    /// `wait()` has been called) — tracked so [`SpawnedChild::terminate`] and any caller-side
    /// drain-then-exit path never double-`wait()` the same child.
    exited: bool,
}

/// One parsed line from a child's raw NDJSON stdout stream, alongside the line's original text
/// (already teed to the `.jsonl` artifact by the time this is returned).
#[derive(Debug, Clone)]
pub struct NdjsonLine {
    /// The raw line text, exactly as read from the child's stdout, before any parse attempt.
    pub raw: String,
    /// The parsed event, or `None` if the line failed to parse as JSON at all (R-SA-026's
    /// tolerance: a malformed line is skipped, never fatal — [`SpawnedChild::next_event`] simply
    /// surfaces `raw` with `parsed: None` rather than erroring the whole run).
    pub parsed: Option<NdjsonEvent>,
}

impl SpawnedChild {
    /// Spawn `spec` as a real child OS process (R-SA-045/046/047/048).
    ///
    /// Stdio wiring is exactly `stdin: null, stdout: piped, stderr: piped` (R-SA-046) — this
    /// crate's mandated mechanism has no use for writing to a child's stdin (communication is
    /// one-directional, parent-reads-child's-stdout-only, per func-SA §1.1), so stdin is closed
    /// rather than left inherited (which would otherwise let a child block waiting on terminal
    /// input it will never receive). stderr is piped (not merged into stdout) so the NDJSON
    /// stream on stdout is never interleaved with diagnostic/log text a child may write to
    /// stderr; stderr lines are still drained (never left to fill the pipe buffer and stall the
    /// child) but are not treated as protocol data.
    ///
    /// # Environment inheritance and overlay order (R-SA-048)
    ///
    /// The child's environment MUST inherit the parent process's full environment, then have
    /// `spec.env_overlay` layered on top, with overlay values always winning over inherited
    /// values of the same name. This crate achieves that ordering with NO special-cased merge
    /// logic: `tokio::process::Command::new` starts every command with the parent's environment
    /// already fully inherited by default, and `env_clear()` is never called anywhere in this
    /// crate (a `grep`-verifiable invariant every call site here and in `spawn::depth`/
    /// `spawn::signal` upholds); calling `.envs(spec.env_overlay)` afterward merges the overlay
    /// on top of that unmodified inherited base, so `tokio::process::Command`'s own environment-
    /// merge semantics — last write wins for a given key — is exactly the inherit-then-overlay
    /// ordering R-SA-048 requires. See [`tests::env_overlay_wins_over_inherited_value_of_the_same_name`]
    /// for a real-process proof of this ordering (not merely an assertion about `Command`'s
    /// documented behavior).
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::Spawn`] if the child fails to spawn, or if the `.jsonl` tee
    /// artifact cannot be created.
    pub async fn spawn(spec: ChildSpawnSpec, jsonl_path: &Path) -> Result<Self, SubagentError> {
        let argv = spec.build_argv();

        let mut command = tokio::process::Command::new(&spec.command.binary);
        command
            .args(&argv)
            .current_dir(&spec.cwd)
            .envs(&spec.env_overlay) // R-SA-048: overlay layered on top of the inherited base;
            // `env_clear()` is deliberately never called anywhere in this crate.
            .stdin(std::process::Stdio::null()) // R-SA-046
            .stdout(std::process::Stdio::piped()) // R-SA-046
            .stderr(std::process::Stdio::piped()); // R-SA-046
        #[cfg(unix)]
        {
            // Give each child its own process group so this crate's own signal-escalation
            // ladder (`spawn::signal::terminate`, R-SA-059) can target exactly this child (and
            // any of its own descendants) without racing the parent orchestrator's own signal
            // disposition — mirrors `cyrup_tools::ops::local`'s existing convention for spawned
            // subprocesses. `process_group` is an inherent method on `tokio::process::Command`
            // itself (not a `std::os::unix::process::CommandExt` extension trait method), so no
            // extra trait import is needed here.
            //
            // This makes the child a process-group LEADER (pgid == pid), which is exactly the
            // condition `spawn::signal::send_signal` detects in order to signal `-pgid` rather
            // than the bare pid. The two halves are a pair: detaching the group without also
            // switching the kill target would leave every escalation stage unable to reach the
            // descendants the subagent is blocked on, AND leave that subtree orphaned (detached
            // from the terminal's foreground group, so not even the user's Ctrl-C reaches it)
            // after stage 3. See `send_signal`'s docs for the full rationale.
            command.process_group(0);
        }

        let mut child = command.spawn().map_err(SubagentError::Spawn)?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SubagentError::Spawn(std::io::Error::other("child stdout not piped")))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| SubagentError::Spawn(std::io::Error::other("child stderr not piped")))?;

        let jsonl_writer = BoundedJsonlWriter::create(jsonl_path)
            .await
            .map_err(SubagentError::Spawn)?;

        Ok(Self {
            child,
            stdout_lines: BufReader::new(stdout).lines(),
            stderr_lines: Some(BufReader::new(stderr).lines()),
            jsonl_writer,
            temp_files: spec.temp_files,
            exited: false,
        })
    }

    /// The OS process id of the live child, when available (the child may already have exited
    /// and been reaped, in which case `tokio` no longer exposes a pid).
    #[must_use]
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    /// Read and return the next line of the child's stdout as NDJSON (R-SA-057/058).
    ///
    /// Per line: the raw bytes are teed, unmodified, to the `.jsonl` artifact file FIRST (R-SA-058
    /// — "as they are read", not buffered and written at exit), then a parse attempt is made;
    /// [`NdjsonLine::parsed`] is `None` on a parse failure rather than this method returning an
    /// error, so a single malformed line never aborts the read loop (R-SA-026's tolerance
    /// principle, restated at this layer since this is where lines are actually read). The caller
    /// (a later phase's `exec::ndjson::consume_stdout`-equivalent fold, or a test) is expected to
    /// drive its own progress/status state from each successfully parsed event before requesting
    /// the next line — this method itself does not maintain any progress state; it is a pure
    /// line-source.
    ///
    /// Returns `None` once the child's stdout stream reaches EOF (the child closed its stdout,
    /// normally because it exited). A tee-write failure surfaces as `Some(Err(..))` rather than
    /// silently dropping the line — losing the on-disk audit artifact is treated as a real error,
    /// distinct from a parse failure.
    pub async fn next_event(&mut self) -> Option<Result<NdjsonLine, SubagentError>> {
        let line = match self.stdout_lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => return None,
            Err(err) => return Some(Err(SubagentError::Spawn(err))),
        };

        if let Err(err) = self.jsonl_writer.write_line(&line).await {
            return Some(Err(SubagentError::Spawn(err)));
        }

        let parsed = serde_json::from_str::<NdjsonEvent>(&line).ok(); // R-SA-026: tolerated, never fatal
        Some(Ok(NdjsonLine { raw: line, parsed }))
    }

    /// [`SpawnedChild::next_event`], plus the third outcome that method cannot express: the process
    /// exiting while its stdout stays open. See [`ChildStep`] for why that is not hypothetical.
    ///
    /// The race lives INSIDE this method on purpose. A caller cannot put `next_event()` and
    /// `wait()` in two arms of its own `tokio::select!` — both need `&mut SpawnedChild`, so the
    /// borrow checker rejects it. That is why the executor's read loop had no exit arm at all: not
    /// an oversight about the failure mode, a structural block on expressing it. Racing the two
    /// disjoint fields behind a single `&mut` borrow is what unblocks it.
    ///
    /// `biased` with stdout FIRST is load-bearing: when a line and the exit are ready in the same
    /// poll, the line wins, so a child's final output is never dropped in favour of its own exit
    /// signal. Both halves are cancellation-safe (`Lines::next_line` retains partial data in its
    /// buffer; `Child::wait` records the status), which the surrounding loop already depends on —
    /// it has raced `next_event()` against cancel/deadline arms since it was written.
    ///
    /// After an [`ChildStep::Exited`] the child is marked reaped and this method degrades to a pure
    /// stdout read, so a caller that keeps draining never double-`wait()`s.
    pub async fn next_event_or_exit(&mut self) -> ChildStep {
        let Self {
            child,
            stdout_lines,
            jsonl_writer,
            exited,
            ..
        } = self;

        let read = if *exited {
            stdout_lines.next_line().await
        } else {
            tokio::select! {
                biased;
                line = stdout_lines.next_line() => line,
                status = child.wait() => {
                    *exited = true;
                    return ChildStep::Exited(status);
                }
            }
        };

        let line = match read {
            Ok(Some(line)) => line,
            Ok(None) => return ChildStep::Eof,
            Err(err) => return ChildStep::Line(Err(SubagentError::Spawn(err))),
        };

        if let Err(err) = jsonl_writer.write_line(&line).await {
            return ChildStep::Line(Err(SubagentError::Spawn(err)));
        }

        let parsed = serde_json::from_str::<NdjsonEvent>(&line).ok(); // R-SA-026: tolerated, never fatal
        ChildStep::Line(Ok(NdjsonLine { raw: line, parsed }))
    }

    /// Move the child's stderr reader out of this [`SpawnedChild`] so the executor can drain it
    /// independently of the stdout read loop and — after the child is consumed by
    /// [`SpawnedChild::terminate`]/[`SpawnedChild::finish`] — read whatever the (now-dead) child
    /// wrote to stderr, surfacing it into the run's error on a non-zero exit (pi `execution.ts:686`).
    ///
    /// Returns a [`CapturedStderr`] wrapper (rather than the raw tokio reader type) so the executor
    /// module never needs to name `Lines<BufReader<ChildStderr>>` itself. Returns an empty capture
    /// on the second and later calls (the reader is taken exactly once). stderr is not protocol data
    /// (R-SA-046) — this is purely for the diagnostic-into-error surfacing, never for parsing NDJSON.
    #[must_use]
    pub fn take_stderr(&mut self) -> CapturedStderr {
        CapturedStderr(self.stderr_lines.take())
    }

    /// Wait, with a bounded timeout, for a child that has already emitted its final message to
    /// exit and release its own stdio on its own (R-SA-068) — a "final drain" grace period that
    /// exists purely to avoid treating an otherwise-successful run as hung just because process
    /// teardown (flushing buffered writes, closing file descriptors) takes a little longer than
    /// the last NDJSON line's arrival.
    ///
    /// Returns `Ok(Some(status))` if the child exits within [`FINAL_DRAIN_TIMEOUT`]; `Ok(None)`
    /// if the timeout elapses first (the caller should then fall back to
    /// [`SpawnedChild::terminate`]'s real signal-escalation ladder rather than waiting forever);
    /// `Err` only on a genuine `wait()` I/O failure.
    ///
    /// This is deliberately NOT itself an escalation step — R-SA-068 explicitly frames the final
    /// drain as "without needing a hard-kill escalation for an otherwise-successful run": a
    /// timeout here does not send any signal, it only tells the caller the bounded wait is over
    /// so *they* can decide whether to escalate.
    pub async fn wait_final_drain(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        if self.exited {
            return Ok(self.child.wait().await.ok());
        }
        tokio::select! {
            biased;
            result = self.child.wait() => {
                let status = result?;
                self.exited = true;
                Ok(Some(status))
            }
            () = tokio::time::sleep(FINAL_DRAIN_TIMEOUT) => Ok(None),
        }
    }

    /// Drive the child through the SIGINT -> (grace) -> SIGTERM -> (grace) -> SIGKILL escalation
    /// ladder (R-SA-059), raced against `cancel`, and return once the OS process is CONFIRMED
    /// gone. Delegates entirely to [`crate::spawn::signal::terminate`] — this method exists only
    /// to also guarantee temp-file cleanup (R-SA-067) runs on this path, since `terminate` itself
    /// has no knowledge of `ChildSpawnSpec::temp_files`.
    ///
    /// Temp-file cleanup (R-SA-067) happens here unconditionally, regardless of which escalation
    /// stage actually confirmed termination — both the success path (child exits cleanly on its
    /// own, reached via [`SpawnedChild::wait_final_drain`] returning `Some`, in which case the
    /// caller need not call this method at all but temp-file cleanup must still happen; see
    /// [`cleanup_temp_files`]) and the failure/cancellation path (this method) MUST clean up.
    ///
    /// # Errors
    ///
    /// Returns an `Err` only if the underlying `wait()` call itself fails at the OS/tokio level;
    /// signal-send failures are swallowed by [`crate::spawn::signal::terminate`] per that
    /// function's own documented contract.
    pub async fn terminate(
        self,
        cancel: &CancelToken,
    ) -> std::io::Result<signal::TerminationOutcome> {
        self.terminate_with_graces(cancel, signal::EscalationGraces::default())
            .await
    }

    /// [`SpawnedChild::terminate`] with the ladder's two inter-rung grace periods supplied
    /// explicitly — see [`signal::EscalationGraces`]. Production goes through
    /// [`SpawnedChild::terminate`]; this exists so a test can assert WHICH escalation rung ended
    /// the child without that assertion secretly depending on the OS reaping it inside a
    /// one-second wall clock.
    ///
    /// # Errors
    ///
    /// Identical to [`SpawnedChild::terminate`]'s.
    pub async fn terminate_with_graces(
        mut self,
        cancel: &CancelToken,
        graces: signal::EscalationGraces,
    ) -> std::io::Result<signal::TerminationOutcome> {
        self.exited = true;
        let outcome = signal::terminate_with_graces(self.child, cancel, graces).await;
        cleanup_temp_files(&self.temp_files); // R-SA-067: cleaned up on this (failure/cancel) path too
        outcome
    }

    /// Explicit cleanup for the success path: once the caller has confirmed the child exited on
    /// its own (via [`SpawnedChild::wait_final_drain`] or by observing stdout EOF), it MUST call
    /// this to release the `.jsonl` writer handle and remove any temp files (R-SA-067) — the
    /// counterpart to [`SpawnedChild::terminate`]'s cleanup on the failure/cancellation path.
    /// Consuming `self` makes it a compile-time error to keep using a `SpawnedChild` after
    /// either exit path has run.
    pub fn finish(self) {
        cleanup_temp_files(&self.temp_files); // R-SA-067: cleaned up on this (success) path too
    }
}

/// The child's stderr reader, moved out of a [`SpawnedChild`] by [`SpawnedChild::take_stderr`]. Its
/// trailing content is surfaced into a failed run's error (pi `execution.ts:686`: on a non-zero
/// exit `result.error = stderrBuf.trim()` when no richer error is already set). Kept opaque so the
/// executor never depends on the concrete `tokio::io::Lines`/`BufReader`/`ChildStderr` types.
pub struct CapturedStderr(Option<Lines<BufReader<ChildStderr>>>);

impl CapturedStderr {
    /// Read every remaining line of the child's stderr to EOF and return it as one string (lines
    /// re-joined with a trailing `\n` each, matching how the child wrote them). Intended to be
    /// called AFTER the child has been consumed (`terminate`/`finish`) and is therefore dead — the
    /// closed write end guarantees a prompt EOF, so this never blocks on a live child. A per-read
    /// bounded timeout is applied defensively so a pathological never-EOF pipe cannot hang the run.
    /// Returns an empty string when the reader was never present (e.g. a second call, or a spawn
    /// that never captured stderr).
    pub async fn drain_to_string(self) -> String {
        let Some(mut lines) = self.0 else {
            return String::new();
        };
        let mut buf = String::new();
        // Loops until EOF (`Ok(Ok(None))`), a read error (`Ok(Err(_))`), or the defensive per-read
        // timeout (`Err(_)`) — any of which fails the `while let` pattern and ends the drain.
        while let Ok(Ok(Some(line))) =
            tokio::time::timeout(Duration::from_secs(2), lines.next_line()).await
        {
            buf.push_str(&line);
            buf.push('\n');
        }
        buf
    }
}

/// Remove every temp file in `paths`, best-effort: a single file already missing (e.g. this crate
/// raced its own cleanup, or a caller pre-removed it) or otherwise unremovable is not treated as
/// a fatal error — R-SA-067 requires cleanup to run on both the success and failure paths, but
/// says nothing about failing the whole run over a temp-file removal that didn't take, so a
/// leftover-but-harmless temp file is preferred over surfacing a spurious error from what is
/// otherwise a successful (or already-failed, and about to report that failure) run.
fn cleanup_temp_files(paths: &[PathBuf]) {
    for path in paths {
        if let Err(err) = std::fs::remove_file(path)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "failed to remove subagent temp file (R-SA-067); leaving it in place"
            );
        }
    }
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
    use std::collections::HashMap as StdHashMap;

    fn lookup_from(
        vars: StdHashMap<&'static str, &'static str>,
    ) -> impl Fn(&str) -> Option<String> {
        move |key| vars.get(key).map(|v| (*v).to_string())
    }

    // ---- resolve_spawn_command: three-tier priority (R-SA-045) ----

    #[test]
    fn resolve_spawn_command_prefers_the_env_override_when_set_and_non_blank() {
        let vars = StdHashMap::from([(SUBAGENT_BINARY_ENV_VAR, "/opt/scripted/fake-cyrup")]);
        let resolved = resolve_spawn_command_from(lookup_from(vars), || {
            Ok(PathBuf::from("/should/not/be/used"))
        });
        assert_eq!(resolved.binary, PathBuf::from("/opt/scripted/fake-cyrup"));
        assert!(resolved.base_args.is_empty());
    }

    #[test]
    fn resolve_spawn_command_treats_a_blank_override_as_absent() {
        let vars = StdHashMap::from([(SUBAGENT_BINARY_ENV_VAR, "   ")]);
        let resolved = resolve_spawn_command_from(lookup_from(vars), || {
            Ok(PathBuf::from("/resolved/via/current-exe"))
        });
        assert_eq!(
            resolved.binary,
            PathBuf::from("/resolved/via/current-exe"),
            "a whitespace-only override must fall through to tier 2, not be used verbatim"
        );
    }

    #[test]
    fn resolve_spawn_command_falls_back_to_current_exe_when_no_override_is_set() {
        let resolved = resolve_spawn_command_from(lookup_from(StdHashMap::new()), || {
            Ok(PathBuf::from("/proc/self/exe-resolved"))
        });
        assert_eq!(resolved.binary, PathBuf::from("/proc/self/exe-resolved"));
    }

    #[test]
    fn resolve_spawn_command_falls_back_to_literal_cyrup_when_current_exe_fails() {
        let resolved = resolve_spawn_command_from(lookup_from(StdHashMap::new()), || {
            Err(std::io::Error::other("current_exe unavailable"))
        });
        assert_eq!(resolved.binary, PathBuf::from(FALLBACK_BINARY_NAME));
    }

    #[test]
    fn resolve_spawn_command_public_entry_point_resolves_the_real_process() {
        // A smoke test exercising the actual public entry point (which reads real
        // std::env::var / std::env::current_exe) rather than only the injectable core, so the
        // wiring is covered without asserting anything about the real environment's exact value
        // (which is out of this test's control under `cargo test`, e.g. CYRUP_SUBAGENT_BINARY may
        // or may not happen to be set in the ambient CI environment).
        let resolved = resolve_spawn_command();
        assert!(
            !resolved.binary.as_os_str().is_empty(),
            "some non-empty binary path/name must always be resolved"
        );
    }

    // ---- ChildSpawnSpec::resolve_task_arg (R-SA-047) ----

    #[test]
    fn resolve_task_arg_passes_a_short_task_through_literally() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let (arg, temp_file) = ChildSpawnSpec::resolve_task_arg("do the thing", dir.path())
            .expect("short task resolves");
        assert_eq!(arg, "do the thing");
        assert!(
            temp_file.is_none(),
            "a short task must not create a temp file"
        );
    }

    #[test]
    fn resolve_task_arg_writes_a_long_task_to_a_tempfile_reference() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let long_task = "x".repeat(TASK_ARGV_INLINE_THRESHOLD + 1);
        let (arg, temp_file) =
            ChildSpawnSpec::resolve_task_arg(&long_task, dir.path()).expect("long task resolves");
        assert!(
            arg.starts_with('@'),
            "a task over the threshold must become an @<tempfile> reference, got {arg}"
        );
        let temp_file = temp_file.expect("a temp file must have been created");
        let contents = std::fs::read_to_string(&temp_file).expect("temp file is readable");
        assert_eq!(
            contents, long_task,
            "the temp file must contain the exact task text verbatim"
        );
    }

    #[test]
    fn resolve_task_arg_boundary_is_inclusive_of_the_threshold() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let exactly_at_threshold = "y".repeat(TASK_ARGV_INLINE_THRESHOLD);
        let (_, temp_file) = ChildSpawnSpec::resolve_task_arg(&exactly_at_threshold, dir.path())
            .expect("boundary-length task resolves");
        assert!(
            temp_file.is_none(),
            "a task exactly AT the threshold must still be passed literally, not tempfile'd"
        );
    }

    // ---- ChildSpawnSpec::build_argv ordering ----

    #[test]
    fn build_argv_orders_base_args_then_args_then_task_last() {
        let spec = ChildSpawnSpec {
            command: SpawnCommand {
                binary: PathBuf::from("cyrup"),
                base_args: vec!["--base-flag".to_string()],
            },
            args: vec![
                "--print".to_string(),
                "--mode".to_string(),
                "json".to_string(),
            ],
            task_arg: "the task prompt".to_string(),
            env_overlay: HashMap::new(),
            cwd: std::env::temp_dir(),
            temp_files: Vec::new(),
        };
        assert_eq!(
            spec.build_argv(),
            vec![
                "--base-flag",
                "--print",
                "--mode",
                "json",
                "the task prompt"
            ],
            "base_args, then args, then the task prompt argument last"
        );
    }

    // ---- NdjsonEvent parsing tolerance ----

    #[test]
    fn ndjson_event_parses_known_shapes() {
        let ev: NdjsonEvent =
            serde_json::from_str(r#"{"type":"tool_execution_start","call_id":"c1","name":"bash"}"#)
                .expect("parses");
        assert_eq!(
            ev,
            NdjsonEvent::ToolExecutionStart {
                call_id: "c1".to_string(),
                name: "bash".to_string(),
            }
        );
    }

    #[test]
    fn ndjson_event_degrades_unknown_tags_rather_than_erroring() {
        let ev: NdjsonEvent = serde_json::from_str(r#"{"type":"some_future_event","x":1}"#)
            .expect("an unrecognized tag must still parse, degrading to Unknown");
        assert_eq!(ev, NdjsonEvent::Unknown);
    }

    // ---- Real-subprocess tests: argv/env construction against a scripted stand-in ----
    //
    // Per this crate's testing convention (no mocked subprocess behavior — see
    // `cyrup_ext::caps::proc`/`cyrup_tools::ops::local`'s own real-child-process tests), these
    // spawn a REAL `sh` process as the scripted stand-in `cyrup`-shaped binary (arch-SA §11's
    // "tiny test-double cyrup-shaped binary that emits scripted NDJSON on stdout" convention),
    // never a mock. `resolve_spawn_command`'s own `CYRUP_SUBAGENT_BINARY` override is exactly
    // the mechanism a real scripted-binary substitution would use in production, so exercising
    // `ChildSpawnSpec`/`SpawnedChild` directly against a hand-built spec below is equally
    // faithful without needing to actually set process-global env vars from a test (which, per
    // `spawn::depth`'s own tests, this crate avoids as a matter of policy under edition-2024's
    // `unsafe`-env-mutation rules).

    /// Resolves `sh` to its ABSOLUTE path up front (via this test process's own real, unmodified
    /// `PATH`) rather than leaving `Command::new("sh")` to re-resolve it via `PATH` at spawn
    /// time. This matters specifically for the env-overlay-ordering tests below: once a test
    /// overlays `PATH` in the CHILD's environment, a bare `"sh"` binary name would fail to
    /// resolve at spawn time (the overlay is exactly what R-SA-048 says must win) — using an
    /// already-resolved absolute path sidesteps that entirely and keeps those tests focused on
    /// what they actually assert (the ENV the child observes), not on `PATH`-based binary lookup.
    fn sh_command(script: &str) -> SpawnCommand {
        let sh_path = std::env::var_os("PATH")
            .and_then(|path| {
                std::env::split_paths(&path)
                    .map(|dir| dir.join("sh"))
                    .find(|candidate| candidate.is_file())
            })
            .unwrap_or_else(|| PathBuf::from("/bin/sh"));
        SpawnCommand {
            binary: sh_path,
            base_args: vec!["-c".to_string(), script.to_string()],
        }
    }

    /// SUBA-S06: a child that exits while a surviving grandchild still holds its stdout open never
    /// produces EOF, so a read loop whose only outcomes are "line" and "EOF" waits forever on
    /// something that cannot happen. The executor's `drive_attempt` was exactly that loop: with no
    /// `timeoutMs` and no terminal assistant stop from the child, not one of its arms could fire,
    /// and the tool call hung permanently while the activity tick spun once a second.
    ///
    /// BOTH halves are asserted on purpose. A fix for a hang is only meaningful if the old shape
    /// demonstrably hung, and a test that just checks the new call works would pass just as
    /// happily against a child that closes stdout normally — i.e. against the case that was never
    /// broken.
    #[tokio::test]
    async fn a_child_that_exits_holding_stdout_open_reports_exited_instead_of_hanging() {
        let dir = tempfile::tempdir().unwrap();
        // `sleep 30 &` inherits stdout's write end; the direct child then exits immediately. The
        // pipe stays open long after the process this crate waits on is gone.
        let spec = ChildSpawnSpec {
            command: sh_command("sleep 30 & exit 0"),
            args: Vec::new(),
            task_arg: String::new(),
            env_overlay: StdHashMap::new(),
            cwd: dir.path().to_path_buf(),
            temp_files: Vec::new(),
        };

        // The pre-fix shape: EOF cannot arrive, so this waits out the whole budget.
        let mut hung = SpawnedChild::spawn(spec.clone(), &dir.path().join("hang.jsonl"))
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(1500), hung.next_event())
                .await
                .is_err(),
            "next_event() must HANG here — if it ever starts returning, the failure mode SUBA-S06 \
             fixes has changed shape, and next_event_or_exit()'s justification needs re-checking \
             rather than the assertion being deleted"
        );
        let _ = hung.terminate(&CancelToken::new()).await;

        // The fix: process exit is itself a wake condition, so the loop is never stranded.
        let mut child = SpawnedChild::spawn(spec, &dir.path().join("exit.jsonl"))
            .await
            .unwrap();
        let step = tokio::time::timeout(Duration::from_secs(5), child.next_event_or_exit())
            .await
            .expect("next_event_or_exit() must observe the exit rather than hang");
        match step {
            ChildStep::Exited(status) => {
                assert!(status.unwrap().success(), "the child exited 0");
            }
            other => panic!("expected ChildStep::Exited, got {other:?}"),
        }
        let _ = child.terminate(&CancelToken::new()).await;
    }

    /// A real spawned child's argv is EXACTLY `base_args` then `args` then the task argument —
    /// verified by having the scripted child echo its own received arguments back as NDJSON,
    /// rather than merely asserting `build_argv`'s return value in isolation (which the earlier
    /// `build_argv_orders_base_args_then_args_then_task_last` test already covers as a pure unit
    /// test) — this test instead proves the SAME ordering survives an actual `tokio::process::
    /// Command::args` call against a real OS process.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawned_child_receives_the_exact_constructed_argv() {
        let dir = tempfile::tempdir().expect("real tempdir");
        // `sh -c '<script>' -- "$@"`: the script's own positional args start at $1, one JSON
        // object per received argument, echoed to stdout.
        let spec = ChildSpawnSpec {
            command: sh_command(
                r#"for a in "$@"; do printf '{"type":"unknown","arg":"%s"}\n' "$a"; done"#,
            ),
            args: vec![
                "--".to_string(),
                "--print".to_string(),
                "--mode".to_string(),
                "json".to_string(),
            ],
            task_arg: "the-task-prompt".to_string(),
            env_overlay: HashMap::new(),
            cwd: dir.path().to_path_buf(),
            temp_files: Vec::new(),
        };

        let jsonl_path = dir.path().join("events.jsonl");
        let mut child = SpawnedChild::spawn(spec, &jsonl_path)
            .await
            .expect("scripted sh child spawns");

        let mut received_args = Vec::new();
        while let Some(result) = child.next_event().await {
            let line = result.expect("no I/O error reading the scripted child's stdout");
            if let Some(arg) = line.raw.strip_prefix(r#"{"type":"unknown","arg":""#) {
                received_args.push(arg.trim_end_matches("\"}").to_string());
            }
        }
        child.finish();

        assert_eq!(
            received_args,
            vec!["--print", "--mode", "json", "the-task-prompt"],
            "the real child observed exactly args (after the -- separator) then the task last"
        );
    }

    /// R-SA-058: the raw stdout tee is written LIVE (line by line as read), not buffered and
    /// flushed only at process exit — verified by reading the `.jsonl` artifact from a SEPARATE
    /// handle mid-stream, before the scripted child has finished emitting all of its lines.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn raw_stdout_is_teed_to_the_jsonl_artifact_live_not_buffered_to_exit() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let spec = ChildSpawnSpec {
            command: sh_command(
                r#"printf '{"type":"unknown","n":1}\n'; sleep 2; printf '{"type":"unknown","n":2}\n'"#,
            ),
            args: Vec::new(),
            task_arg: String::new(),
            env_overlay: HashMap::new(),
            cwd: dir.path().to_path_buf(),
            temp_files: Vec::new(),
        };
        let jsonl_path = dir.path().join("live.jsonl");
        let mut child = SpawnedChild::spawn(spec, &jsonl_path)
            .await
            .expect("scripted sh child spawns");

        // Read exactly the first event, then check the artifact BEFORE reading the second
        // (which the scripted child deliberately delays by 2s) — the first line must already be
        // on disk.
        let first = child
            .next_event()
            .await
            .expect("first event is present")
            .expect("no I/O error");
        assert!(first.raw.contains("\"n\":1"));

        let mid_stream_contents = tokio::fs::read_to_string(&jsonl_path)
            .await
            .expect("artifact readable mid-stream");
        assert_eq!(
            mid_stream_contents.trim(),
            first.raw,
            "the FIRST line must already be flushed to disk before the child emits its second \
             (deliberately delayed) line — proving live teeing, not buffer-at-exit"
        );

        // Drain the rest so the child exits cleanly and the test does not leak a process.
        while child.next_event().await.is_some() {}
        child.finish();
    }

    /// R-SA-026's tolerance, exercised through the real spawn boundary: a malformed (non-JSON)
    /// line from the child's stdout must still be teed to the artifact (R-SA-058 makes no
    /// exception for unparseable lines) but must NOT abort the read loop or surface as an error —
    /// `NdjsonLine::parsed` is simply `None` for that one line, and subsequent well-formed lines
    /// still parse normally.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_malformed_line_is_teed_and_skipped_without_aborting_the_stream() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let spec = ChildSpawnSpec {
            command: sh_command(
                r#"printf 'not valid json at all\n'; printf '{"type":"unknown","ok":true}\n'"#,
            ),
            args: Vec::new(),
            task_arg: String::new(),
            env_overlay: HashMap::new(),
            cwd: dir.path().to_path_buf(),
            temp_files: Vec::new(),
        };
        let jsonl_path = dir.path().join("tolerant.jsonl");
        let mut child = SpawnedChild::spawn(spec, &jsonl_path)
            .await
            .expect("scripted sh child spawns");

        let first = child
            .next_event()
            .await
            .expect("first (malformed) line is still surfaced")
            .expect("no I/O error, even though the line is not valid JSON");
        assert_eq!(first.raw, "not valid json at all");
        assert!(
            first.parsed.is_none(),
            "a malformed line must parse to None, not error the run"
        );

        let second = child
            .next_event()
            .await
            .expect("the stream continues past the malformed line")
            .expect("no I/O error");
        assert!(
            second.parsed.is_some(),
            "a well-formed line after a malformed one still parses"
        );

        child.finish();

        let artifact = tokio::fs::read_to_string(&jsonl_path)
            .await
            .expect("artifact readable");
        assert!(
            artifact.contains("not valid json at all"),
            "the malformed line must still be teed to the artifact verbatim (R-SA-058 has no \
             carve-out for unparseable lines)"
        );
    }

    // ---- Env-overlay ordering: inherited-then-overlay (R-SA-048) ----

    /// The load-bearing ordering assertion: a variable present in the REAL inherited parent
    /// environment (set via `std::env::set_var` is avoided per this crate's policy — instead we
    /// rely on a variable `cargo test` itself already guarantees is set in every child process's
    /// inherited environment, `PATH`) must be OVERRIDABLE by `env_overlay`, and — the actual
    /// point of R-SA-048 — an overlay value for a key that ALSO exists in the inherited
    /// environment must win, never the reverse. This is proven by a real spawned child echoing
    /// back what it actually observes via `sh -c 'echo $VAR'`, not by inspecting `Command`'s
    /// internal state.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn env_overlay_wins_over_inherited_value_of_the_same_name() {
        let dir = tempfile::tempdir().expect("real tempdir");
        // `PATH` is guaranteed present in this test process's own environment (needed to find
        // `sh` in the first place) and is therefore guaranteed to be part of the child's
        // INHERITED environment before any overlay is applied.
        let inherited_path =
            std::env::var("PATH").expect("PATH is set in the test process's own environment");
        assert!(!inherited_path.is_empty());

        let mut overlay = HashMap::new();
        overlay.insert("PATH".to_string(), "/overlay-should-win".to_string());
        overlay.insert(
            "CYRUP_SUBAGENT_TEST_ONLY".to_string(),
            "overlay-value".to_string(),
        );

        let spec = ChildSpawnSpec {
            command: sh_command(
                r#"printf '{"type":"unknown","path":"%s"}\n' "$PATH"; \
                   printf '{"type":"unknown","custom":"%s"}\n' "$CYRUP_SUBAGENT_TEST_ONLY""#,
            ),
            args: Vec::new(),
            task_arg: String::new(),
            env_overlay: overlay,
            cwd: dir.path().to_path_buf(),
            temp_files: Vec::new(),
        };
        let jsonl_path = dir.path().join("env.jsonl");
        let mut child = SpawnedChild::spawn(spec, &jsonl_path)
            .await
            .expect("scripted sh child spawns");

        let mut lines = Vec::new();
        while let Some(result) = child.next_event().await {
            lines.push(result.expect("no I/O error").raw);
        }
        child.finish();

        assert!(
            lines
                .iter()
                .any(|l| l.contains(r#""path":"/overlay-should-win""#)),
            "the overlay value for PATH (a key ALSO present in the inherited environment) must \
             win over the inherited value — inherited-then-overlay, not overlay-then-inherited; \
             got lines: {lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l.contains(&inherited_path)),
            "the stale inherited PATH value must NOT leak through once overlaid"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains(r#""custom":"overlay-value""#)),
            "an overlay-only key (absent from the inherited environment) must still reach the \
             child — proving the overlay is additive on top of inheritance, not a replacement"
        );
    }

    /// The other half of R-SA-048: a key present ONLY in the inherited environment (never
    /// mentioned in `env_overlay` at all) must still reach the child unchanged — `env_clear()`
    /// must never be called anywhere on this path.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inherited_only_variables_survive_when_the_overlay_does_not_mention_them() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let inherited_path =
            std::env::var("PATH").expect("PATH is set in the test process's own environment");

        let spec = ChildSpawnSpec {
            command: sh_command(r#"printf '{"type":"unknown","path":"%s"}\n' "$PATH""#),
            args: Vec::new(),
            task_arg: String::new(),
            env_overlay: HashMap::new(), // empty overlay: nothing to override
            cwd: dir.path().to_path_buf(),
            temp_files: Vec::new(),
        };
        let jsonl_path = dir.path().join("inherit-only.jsonl");
        let mut child = SpawnedChild::spawn(spec, &jsonl_path)
            .await
            .expect("scripted sh child spawns");

        let first = child
            .next_event()
            .await
            .expect("event present")
            .expect("no I/O error");
        child.finish();

        assert!(
            first.raw.contains(&inherited_path),
            "PATH must be inherited verbatim when the overlay does not mention it at all \
             (env_clear() must never be called), got: {}",
            first.raw
        );
    }

    // ---- Temp-file cleanup (R-SA-067) ----

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn finish_cleans_up_temp_files_on_the_success_path() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let temp_file = dir.path().join("leftover-task.txt");
        std::fs::write(&temp_file, "long task text").expect("seed temp file");

        let spec = ChildSpawnSpec {
            command: sh_command("true"),
            args: Vec::new(),
            task_arg: String::new(),
            env_overlay: HashMap::new(),
            cwd: dir.path().to_path_buf(),
            temp_files: vec![temp_file.clone()],
        };
        let jsonl_path = dir.path().join("cleanup-success.jsonl");
        let mut child = SpawnedChild::spawn(spec, &jsonl_path)
            .await
            .expect("scripted sh child spawns");
        while child.next_event().await.is_some() {}
        child.finish();

        assert!(
            !temp_file.exists(),
            "finish() (the success path) must remove every registered temp file"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminate_cleans_up_temp_files_on_the_failure_path() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let temp_file = dir.path().join("leftover-task.txt");
        std::fs::write(&temp_file, "long task text").expect("seed temp file");

        let spec = ChildSpawnSpec {
            command: sh_command("trap '' INT TERM; while true; do sleep 1; done"),
            args: Vec::new(),
            task_arg: String::new(),
            env_overlay: HashMap::new(),
            cwd: dir.path().to_path_buf(),
            temp_files: vec![temp_file.clone()],
        };
        let jsonl_path = dir.path().join("cleanup-failure.jsonl");
        let child = SpawnedChild::spawn(spec, &jsonl_path)
            .await
            .expect("scripted sh child spawns");

        let cancel = CancelToken::new();
        let _outcome = child
            .terminate(&cancel)
            .await
            .expect("terminate confirms the real child is gone");

        assert!(
            !temp_file.exists(),
            "terminate() (the failure/cancellation path) must ALSO remove every registered temp \
             file, not only the success path"
        );
    }

    // ---- Bounded final-drain wait (R-SA-068) ----

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wait_final_drain_returns_promptly_for_a_child_that_exits_quickly() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let spec = ChildSpawnSpec {
            command: sh_command("exit 0"),
            args: Vec::new(),
            task_arg: String::new(),
            env_overlay: HashMap::new(),
            cwd: dir.path().to_path_buf(),
            temp_files: Vec::new(),
        };
        let jsonl_path = dir.path().join("drain-fast.jsonl");
        let mut child = SpawnedChild::spawn(spec, &jsonl_path)
            .await
            .expect("scripted sh child spawns");
        while child.next_event().await.is_some() {}

        let started = tokio::time::Instant::now();
        let status = child
            .wait_final_drain()
            .await
            .expect("no I/O error")
            .expect("a fast-exiting child must be observed within the drain timeout");
        assert!(status.success());
        assert!(
            started.elapsed() < FINAL_DRAIN_TIMEOUT,
            "a child that already exited must return near-instantly, not pay out the full \
             drain timeout"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wait_final_drain_times_out_on_a_child_that_never_exits() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let spec = ChildSpawnSpec {
            command: sh_command("while true; do sleep 1; done"),
            args: Vec::new(),
            task_arg: String::new(),
            env_overlay: HashMap::new(),
            cwd: dir.path().to_path_buf(),
            temp_files: Vec::new(),
        };
        let jsonl_path = dir.path().join("drain-hang.jsonl");
        let mut child = SpawnedChild::spawn(spec, &jsonl_path)
            .await
            .expect("scripted sh child spawns");

        let started = tokio::time::Instant::now();
        let result = child
            .wait_final_drain()
            .await
            .expect("no I/O error even on timeout");
        assert!(
            result.is_none(),
            "a genuinely hung child must report None (timed out), not a status"
        );
        assert!(
            started.elapsed() >= FINAL_DRAIN_TIMEOUT,
            "the full drain timeout must genuinely elapse before giving up"
        );

        // Clean up: the test must not leak the still-running child.
        let cancel = CancelToken::new();
        let _ = child.terminate(&cancel).await;
    }

    // ---- terminate() wiring to spawn::signal ----

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminate_delegates_to_the_real_signal_escalation_ladder() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let spec = ChildSpawnSpec {
            command: sh_command("while true; do sleep 1; done"),
            args: Vec::new(),
            task_arg: String::new(),
            env_overlay: HashMap::new(),
            cwd: dir.path().to_path_buf(),
            temp_files: Vec::new(),
        };
        let jsonl_path = dir.path().join("terminate.jsonl");
        let child = SpawnedChild::spawn(spec, &jsonl_path)
            .await
            .expect("scripted sh child spawns");
        let pid = child.id().expect("live child has a pid");

        let cancel = CancelToken::new();
        // A GENEROUS stage-1 grace, deliberately not the 1000ms production constant. The
        // behavioural claim under test is "a plain `sh` loop dies to SIGINT alone, so the ladder
        // stops at rung 1" — asserting that against `SIGINT_GRACE` silently also asserts "and the
        // OS reaps it, and this task gets scheduled, inside one second", which is false on a loaded
        // machine: with all cores pinned this test escalated to `Sigterm` after exactly 1.01s, i.e.
        // it failed on the wall clock while the SIGINT it claims to test had worked perfectly.
        // `SIGINT_GRACE` itself stays at 1000ms — it is the production value and it is correct;
        // only the test's assumption that reaping always beats it was wrong.
        let graces = signal::EscalationGraces {
            sigint: std::time::Duration::from_secs(30),
            ..signal::EscalationGraces::default()
        };
        let started = tokio::time::Instant::now();
        let outcome = child
            .terminate_with_graces(&cancel, graces)
            .await
            .expect("terminate confirms real exit");

        assert_eq!(
            outcome.stage,
            signal::EscalationStage::Sigint,
            "a plain sh loop (no signal traps) must die to SIGINT alone"
        );
        assert!(
            started.elapsed() < graces.sigint,
            "a SIGINT-obeying child must not require escalation past stage 1"
        );

        let alive = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status();
        assert!(
            alive.map(|s| !s.success()).unwrap_or(true),
            "kill -0 must fail after terminate() returns — the OS process is really gone"
        );
    }

    /// The escalation ladder must reach the child's DESCENDANTS, not only the direct child.
    ///
    /// `SpawnedChild::spawn` puts the child in its own process group precisely so this holds; a
    /// pid-only ladder would SIGKILL the direct child at stage 3 and leave its whole subtree
    /// running, orphaned into a detached process group nothing holds a handle to — a real process
    /// leak on every subagent cancel/timeout, since a subagent child is a `cyrup` re-exec that is
    /// normally blocked on a descendant it spawned itself (a bash-tool command, `cargo`, a nested
    /// subagent). Asserts against the OS, not this crate's bookkeeping.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminate_reaches_the_childs_own_descendants_not_just_the_direct_child() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let pid_path = dir.path().join("grandchild.pid");
        // A FOREGROUND descendant, matching the real subagent shape: the direct child sits in
        // `wait(2)` on a process it spawned itself (a bash-tool command, `cargo`, a nested
        // subagent). Deliberately not `sleep 300 &` — POSIX has a non-interactive shell set
        // SIGINT/SIGQUIT to SIG_IGN in an ASYNCHRONOUS child, which would make the descendant
        // immune to stage 1 for reasons that have nothing to do with signal targeting.
        //
        // The inner `sh` publishes its own pid via an atomic rename (so the file is never read
        // half-written) and then `exec`s, keeping that same pid — the readiness-marker idiom
        // `spawn::signal`'s own tests use, rather than a fixed sleep that CPU contention outruns.
        let script = format!(
            "sh -c 'echo $$ > \"{path}.tmp\"; mv \"{path}.tmp\" \"{path}\"; exec sleep 300'",
            path = pid_path.display()
        );
        let spec = ChildSpawnSpec {
            command: sh_command(&script),
            args: Vec::new(),
            task_arg: String::new(),
            env_overlay: HashMap::new(),
            cwd: dir.path().to_path_buf(),
            temp_files: Vec::new(),
        };
        let jsonl_path = dir.path().join("descendants.jsonl");
        let child = SpawnedChild::spawn(spec, &jsonl_path)
            .await
            .expect("scripted sh child spawns");
        let child_pid = child.id().expect("live child has a pid");

        let grandchild_pid = read_published_pid(&pid_path, Duration::from_secs(10))
            .await
            .expect("the child script publishes its descendant's pid");
        assert_ne!(
            grandchild_pid, child_pid,
            "the script must really have forked a separate descendant process"
        );

        let cancel = CancelToken::new();
        let _outcome = child
            .terminate(&cancel)
            .await
            .expect("terminate confirms real exit");

        assert!(
            pid_is_terminated(grandchild_pid, Duration::from_secs(10)).await,
            "the child's own descendant (pid {grandchild_pid}) must be terminated by the \
             escalation ladder too, not left running as an orphan after the direct child \
             (pid {child_pid}) was signalled"
        );
    }

    /// Poll for the pid published (via an atomic rename, so it is never read half-written) by a
    /// test child script, up to `timeout`. `None` means it never appeared.
    #[cfg(unix)]
    async fn read_published_pid(path: &Path, timeout: Duration) -> Option<u32> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Ok(contents) = std::fs::read_to_string(path)
                && let Ok(pid) = contents.trim().parse::<u32>()
            {
                return Some(pid);
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Poll, up to `timeout`, until `pid` is confirmed terminated at the OS level.
    ///
    /// "Terminated" means gone from the process table OR an un-reaped zombie: this pid's parent
    /// (the direct child) dies in the same escalation, so the zombie is awaiting reaping by
    /// whatever it was reparented to, which is outside this test's control and is not the thing
    /// under test — the thing under test is that it stopped running at all.
    #[cfg(unix)]
    async fn pid_is_terminated(pid: u32, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            // `kill(pid, None)` sends nothing; it only probes existence/permission. An error
            // (ESRCH) means the pid is genuinely gone from the process table.
            let nix_pid = nix::unistd::Pid::from_raw(pid as nix::libc::pid_t);
            if nix::sys::signal::kill(nix_pid, None).is_err() {
                return true;
            }
            // Still listed: on Linux that can be an un-reaped zombie. `/proc/<pid>/stat`'s state
            // field is the one right after the `(comm)` field, which may itself contain spaces —
            // hence splitting on the LAST ')' rather than on whitespace.
            if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                && stat
                    .rsplit_once(')')
                    .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z'))
            {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
