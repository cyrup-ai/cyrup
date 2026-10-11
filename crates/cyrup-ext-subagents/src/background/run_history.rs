//! Run history (pi `runs/shared/run-history.ts` @v0.76.1): the append-only, private,
//! task-redacted `run-history.jsonl` record of finished runs.
//!
//! Every write goes through [`record_run`] (pi `recordRun`, `:223-260`), which hardens the storage
//! (0700 dir, 0600 file), re-sanitizes any legacy or externally written lines in place, and
//! appends ONE line whose `task` is always `"[redacted]"` and whose `taskHash` is the SHA-256 of
//! the task text. Its two producers are the foreground single path
//! (`extension/executor/foreground.rs`, pi `subagent-executor.ts:4371,4396`) and the background
//! runner's terminal tail (`runner_main/finish.rs`, pi `subagent-runner.ts:5104-5122`, one row per
//! launched child step via [`plan_background_run_history`]).
//!
//! Split out of `background/mod.rs` behind its private-module facade (same pattern as
//! `runner_main/`): every public item here is re-exported at [`crate::background`], so consumer
//! paths are unchanged.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::exec::SingleResult;

use super::{StepState, temp_root_dir};

/// pi `ROTATE_READ_THRESHOLD` (`run-history.ts:20`): a [`load_runs_for_agent`] read that finds
/// MORE than this many lines rotates the file.
const ROTATE_READ_THRESHOLD: usize = 1200;
/// pi `ROTATE_KEEP` (`run-history.ts:21`): how many of the newest lines a rotation keeps.
const ROTATE_KEEP: usize = 1000;
/// pi `PRIVATE_DIR_MODE` (`run-history.ts:22`).
#[cfg(unix)]
const PRIVATE_DIR_MODE: u32 = 0o700;
/// pi `PRIVATE_FILE_MODE` (`run-history.ts:23`).
#[cfg(unix)]
const PRIVATE_FILE_MODE: u32 = 0o600;
/// pi `REDACTED_TASK` (`run-history.ts:24`): the only `task` value this module ever writes.
pub const REDACTED_TASK: &str = "[redacted]";
/// pi `historyFileStates`' eviction bound (`run-history.ts:101`).
const HISTORY_FILE_STATE_CAP: usize = 8;

/// pi `RunOutcome` (`run-history.ts:7`), spelled on disk exactly as upstream spells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    /// Exit code 0 with no terminal flag.
    Completed,
    /// A nonzero exit with no terminal flag and no unexplained signal.
    Failed,
    /// The run (or this step) hit its deadline.
    TimedOut,
    /// An explicit stop, or a nonzero exit by an unexplained process signal.
    Stopped,
    /// A soft interrupt (a paused background run records one of these per attempt).
    Interrupted,
}

/// One line of `run-history.jsonl` — pi `RunEntry` (`run-history.ts:9-18` @v0.76.1).
///
/// Field DECLARATION order is the on-disk key order (serde serializes in declaration order), and
/// it is upstream's object-literal order (`:240-249`): `agent, task, taskHash, ts, status,
/// outcome, duration, exit`. Every field but `agent` deserializes leniently because a legacy line
/// — upstream's or cyrup's own pre-SUBA-172 plaintext shape — carries no `taskHash`/`outcome`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RunHistoryEntry {
    /// The agent this entry records.
    pub agent: String,
    /// Always [`REDACTED_TASK`] on a line this module wrote or sanitized. The task text itself is
    /// never stored; [`Self::task_hash`] identifies it.
    #[serde(default)]
    pub task: String,
    /// Lowercase hex SHA-256 of the full task text (pi `hashTask`, `run-history.ts:31-33`).
    /// Absent only on a legacy line whose task was empty or already redacted.
    #[serde(rename = "taskHash", default, skip_serializing_if = "Option::is_none")]
    pub task_hash: Option<String>,
    /// Epoch **seconds** (pi's `Math.floor(Date.now() / 1000)`).
    #[serde(default)]
    pub ts: i64,
    /// `"ok"` for a clean exit, `"error"` otherwise (pi's `exitCode === 0 ? "ok" : "error"`).
    #[serde(default)]
    pub status: String,
    /// The explicit outcome (pi `:231-239`). `None` only on a legacy line: upstream never
    /// synthesises one for a line it did not write (`sanitizeHistoryLine`, `:46-68`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<RunOutcome>,
    /// The run's (or step's) duration in milliseconds.
    #[serde(default)]
    pub duration: i64,
    /// The failing exit code, present only when nonzero (pi's `...(exitCode !== 0 ? { exit } : {})`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
}

/// The terminal facts pi's `recordRun` reads off its fifth argument (`run-history.ts:228`):
/// `{ interrupted, processSignal, stopped, timedOut, turnBudgetExceeded }`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunTerminal {
    /// pi `terminal.interrupted`.
    pub interrupted: bool,
    /// pi `terminal.processSignal`.
    pub process_signal: Option<String>,
    /// pi `terminal.stopped`.
    pub stopped: bool,
    /// pi `terminal.timedOut`.
    pub timed_out: bool,
    /// pi `terminal.turnBudgetExceeded`.
    pub turn_budget_exceeded: bool,
}

impl RunTerminal {
    /// The terminal facts of one settled child result — what upstream passes when it hands the
    /// result itself as `terminal` (`subagent-executor.ts:4371,4396`).
    #[must_use]
    pub fn from_result(result: &SingleResult) -> Self {
        Self {
            interrupted: result.interrupted,
            process_signal: result.process_signal.clone(),
            stopped: result.stopped,
            timed_out: result.timed_out,
            turn_budget_exceeded: result.turn_budget_exceeded,
        }
    }
}

/// pi `recordRun`'s outcome ladder (`run-history.ts:231-239`), verbatim: stopped, then
/// interrupted, then timed out, then a nonzero exit by an UNEXPLAINED process signal (pi
/// `isUnexplainedProcessSignal`, `process-signal.ts:21-35`) is "stopped", else the exit code.
#[must_use]
pub fn run_outcome(exit_code: i32, terminal: &RunTerminal) -> RunOutcome {
    if terminal.stopped {
        RunOutcome::Stopped
    } else if terminal.interrupted {
        RunOutcome::Interrupted
    } else if terminal.timed_out {
        RunOutcome::TimedOut
    } else if exit_code != 0
        && crate::tui::intercom::is_unexplained_process_signal(
            terminal.process_signal.as_deref(),
            terminal.interrupted,
            terminal.timed_out,
            terminal.stopped,
            terminal.turn_budget_exceeded,
        )
    {
        RunOutcome::Stopped
    } else if exit_code == 0 {
        RunOutcome::Completed
    } else {
        RunOutcome::Failed
    }
}

/// `getHistoryPath()` (`runs/shared/run-history.ts:27-29` @v0.76.1):
/// `path.join(getAgentDir(), "run-history.jsonl")`.
///
/// Run history is DELIBERATELY not under [`temp_root_dir`]: it is durable user state, so it belongs
/// in the agent dir. Nothing in cyrup reads it back today — upstream's only reader,
/// [`load_runs_for_agent`] (`loadRunsForAgent`), has no production caller at v0.76.1 either, so
/// the file is write-only from the product's point of view in both runtimes. The previous
/// `<home>/.cyrup/subagents/run-history.jsonl` sat in the run-scratch tree that this module
/// correctly treats as disposable.
#[must_use]
pub fn run_history_path() -> PathBuf {
    crate::paths::agent_dir().join("run-history.jsonl")
}

/// The run-history file for a run whose per-run directories hang off `async_root`.
///
/// For every run that actually lives in this user's canonical scratch root ([`temp_root_dir`]) —
/// which is every production run, since `resolve_background_storage_roots` derives its roots from
/// [`run_artifact_roots`](crate::background::run_artifact_roots) or from an inherited nested route that is itself under that root — this
/// is exactly pi's unconditional [`run_history_path`].
///
/// [CYRUP-DELTA] a run whose roots were REDIRECTED somewhere else records its history beside those
/// roots instead of in the real user's agent dir. This is the same principle C7 established for the
/// results dir: **the runner honours the absolute roots it was handed and never re-derives a path
/// the orchestrator did not choose.** History was the one write still ignoring that, and the cost
/// was measurable — a full workspace gate put 136 lines of synthetic test history (`researcher` /
/// `"do the thing"` / `scout`) into a developer's real `~/.cyrup/agent/run-history.jsonl`, because
/// in-process `run()` callers hand it a `TempDir` for every path EXCEPT this one.
#[must_use]
pub fn run_history_path_for(async_root: &Path) -> PathBuf {
    if async_root.starts_with(temp_root_dir()) {
        return run_history_path();
    }
    async_root
        .parent()
        .unwrap_or(async_root)
        .join("run-history.jsonl")
}

/// pi `hashTask` (`run-history.ts:31-33`): lowercase hex SHA-256 of the FULL task text.
fn hash_task(task: &str) -> String {
    crate::exec::mcp_direct_tools::hex_sha256(task)
}

/// pi `hardenHistoryStorage` (`run-history.ts:35-44`): create the dir at 0700, then chmod the dir
/// to 0700 and an existing file to 0600 when either is not already exactly that. The `mkdir`
/// failure propagates (upstream's `mkdirSync` throws into `recordRun`'s outer `catch`); both
/// chmods swallow their errors. Note this re-modes a PRE-EXISTING agent dir too, which is
/// upstream's behaviour for `~/.pi/agent`.
fn harden_history_storage(history_path: &Path) -> std::io::Result<()> {
    let Some(dir) = history_path.parent() else {
        return Ok(());
    };
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
        builder.mode(PRIVATE_DIR_MODE);
        builder.create(dir)?;
        if let Ok(meta) = std::fs::metadata(dir)
            && meta.permissions().mode() & 0o777 != PRIVATE_DIR_MODE
        {
            let _ =
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(PRIVATE_DIR_MODE));
        }
        if let Ok(meta) = std::fs::metadata(history_path)
            && meta.permissions().mode() & 0o777 != PRIVATE_FILE_MODE
        {
            let _ = std::fs::set_permissions(
                history_path,
                std::fs::Permissions::from_mode(PRIVATE_FILE_MODE),
            );
        }
    }
    #[cfg(not(unix))]
    builder.create(dir)?;
    Ok(())
}

/// pi `sanitizeHistoryLine` (`run-history.ts:46-68`).
///
/// `JSON.parse` failure, `null`, and any non-object primitive are dropped (`!value || typeof
/// value !== "object"`). An array IS a JS object, so it survives and is re-emitted by the spread as
/// an index-keyed object — reproduced by [`js_object_spread`]. `task` is overwritten in place with
/// [`REDACTED_TASK`]; an existing non-empty string `taskHash` is kept, otherwise a non-empty,
/// non-redacted string task is hashed and the hash spread in (a NEW key lands last, an existing
/// one keeps its place). Unknown keys pass through, and no `outcome` is ever synthesised.
fn sanitize_history_line(line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let mut record = js_object_spread(value)?;
    let task = record
        .get("task")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let task_hash = match record.get("taskHash").and_then(serde_json::Value::as_str) {
        Some(existing) if !existing.is_empty() => Some(existing.to_string()),
        _ if !task.is_empty() && task != REDACTED_TASK => Some(hash_task(&task)),
        _ => None,
    };
    record.insert(
        "task".to_string(),
        serde_json::Value::String(REDACTED_TASK.to_string()),
    );
    if let Some(task_hash) = task_hash {
        record.insert("taskHash".to_string(), serde_json::Value::String(task_hash));
    }
    serde_json::to_string(&serde_json::Value::Object(js_property_order(record))).ok()
}

/// `{ ...value }` for a parsed JSON value that passed `typeof value === "object" && value`: an
/// object is itself, an array becomes `{"0": …, "1": …}`, anything else is not an object.
fn js_object_spread(
    value: serde_json::Value,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    match value {
        serde_json::Value::Object(map) => Some(map),
        serde_json::Value::Array(items) => Some(
            items
                .into_iter()
                .enumerate()
                .map(|(index, item)| (index.to_string(), item))
                .collect(),
        ),
        _ => None,
    }
}

/// `JSON.stringify`'s key order for an ordinary object: integer-index keys ascending first, then
/// every other key in insertion order (ECMA-262 `OrdinaryOwnPropertyKeys`). The spread in
/// [`sanitize_history_line`] re-creates the object, so upstream emits this order even when the
/// line on disk had another.
fn js_property_order(
    record: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    fn array_index(key: &str) -> Option<u32> {
        let index: u32 = key.parse().ok()?;
        (index != u32::MAX && index.to_string() == key).then_some(index)
    }
    let (mut indexed, named): (Vec<_>, Vec<_>) = record
        .into_iter()
        .partition(|(key, _)| array_index(key).is_some());
    indexed.sort_by_key(|(key, _)| array_index(key));
    indexed.into_iter().chain(named).collect()
}

/// pi `sanitizeHistoryLines` (`run-history.ts:70-85`): sanitize every non-blank line, reporting
/// whether any was dropped or rewritten.
fn sanitize_history_lines(raw: &str) -> (Vec<String>, bool) {
    let mut lines = Vec::new();
    let mut changed = false;
    for line in raw.split('\n') {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match sanitize_history_line(trimmed) {
            Some(sanitized) => {
                if sanitized != trimmed {
                    changed = true;
                }
                lines.push(sanitized);
            }
            None => changed = true,
        }
    }
    (lines, changed)
}

/// Open `path` for writing with the private file mode applied on CREATE (pi's `{ mode:
/// PRIVATE_FILE_MODE }` / the `openSync` mode argument).
fn private_open_options() -> std::fs::OpenOptions {
    let mut options = std::fs::OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(PRIVATE_FILE_MODE);
    }
    options
}

/// pi `writePrivateHistory` (`run-history.ts:87-90`): a full, NON-atomic rewrite (upstream's
/// `writeFileSync`), then a best-effort chmod to 0600 — the create mode alone does not re-mode a
/// file that already existed.
fn write_private_history(history_path: &Path, lines: &[String]) -> std::io::Result<()> {
    let body = if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    };
    private_open_options()
        .write(true)
        .create(true)
        .truncate(true)
        .open(history_path)?
        .write_all(body.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(
            history_path,
            std::fs::Permissions::from_mode(PRIVATE_FILE_MODE),
        );
    }
    Ok(())
}

/// The stat identity pi's `historyFileStates` compares (`run-history.ts:25,113-117`): mtime,
/// ctime, size and inode. ctime/inode exist only on unix; elsewhere the pair degrades to
/// mtime + size.
#[derive(Clone, Debug, PartialEq, Eq)]
struct HistoryFileIdentity {
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    ctime: (i64, i64),
    #[cfg(unix)]
    ino: u64,
    size: u64,
}

impl HistoryFileIdentity {
    fn of(meta: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt as _;
        Self {
            modified: meta.modified().ok(),
            #[cfg(unix)]
            ctime: (meta.ctime(), meta.ctime_nsec()),
            #[cfg(unix)]
            ino: meta.ino(),
            size: meta.len(),
        }
    }
}

/// pi `historyFileStates` (`run-history.ts:25`): path → (identity, sanitized line count), FIFO
/// bounded at [`HISTORY_FILE_STATE_CAP`] (`:101`). Purely an optimisation — a hit means the file
/// is byte-for-byte what this process last sanitized, so the read is skipped.
type HistoryFileStates = Vec<(PathBuf, HistoryFileIdentity, usize)>;

fn history_file_states() -> std::sync::MutexGuard<'static, HistoryFileStates> {
    static STATES: std::sync::Mutex<HistoryFileStates> = std::sync::Mutex::new(Vec::new());
    STATES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// pi `rememberHistoryFile` (`run-history.ts:92-102`). Re-setting an existing key keeps its FIFO
/// position, as a JS `Map#set` does.
fn remember_history_file(history_path: &Path, line_count: usize) -> std::io::Result<()> {
    let identity = HistoryFileIdentity::of(&std::fs::metadata(history_path)?);
    let mut states = history_file_states();
    match states.iter_mut().find(|(path, _, _)| path == history_path) {
        Some(entry) => {
            entry.1 = identity;
            entry.2 = line_count;
        }
        None => states.push((history_path.to_path_buf(), identity, line_count)),
    }
    if states.len() > HISTORY_FILE_STATE_CAP {
        states.remove(0);
    }
    Ok(())
}

fn forget_history_file(history_path: &Path) {
    history_file_states().retain(|(path, _, _)| path != history_path);
}

/// pi `sanitizeHistoryFile` (`run-history.ts:104-125`): `0` for a missing file; the cached count
/// when the stat identity is unchanged; otherwise read, sanitize, rewrite if anything changed, and
/// remember.
fn sanitize_history_file(history_path: &Path) -> std::io::Result<usize> {
    let meta = match std::fs::metadata(history_path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err),
    };
    let identity = HistoryFileIdentity::of(&meta);
    if let Some((_, _, count)) = history_file_states()
        .iter()
        .find(|(path, cached, _)| path == history_path && *cached == identity)
    {
        return Ok(*count);
    }
    let raw = std::fs::read_to_string(history_path)?;
    let (lines, changed) = sanitize_history_lines(&raw);
    if changed {
        write_private_history(history_path, &lines)?;
    }
    remember_history_file(history_path, lines.len())?;
    Ok(lines.len())
}

/// pi `appendPrivateHistoryLine` (`run-history.ts:127-134`): `O_APPEND|O_CREAT|O_WRONLY` at 0600
/// and ONE write of `line\n` — the only concurrency guarantee upstream relies on.
fn append_private_history_line(history_path: &Path, line: &str) -> std::io::Result<()> {
    let mut file = private_open_options()
        .append(true)
        .create(true)
        .open(history_path)?;
    file.write_all(format!("{line}\n").as_bytes())
}

/// Record one finished run — pi `recordRun` (`run-history.ts:223-260` @v0.76.1).
///
/// The line is `{agent, task: "[redacted]", taskHash: sha256(task), ts, status, outcome,
/// duration, exit?}`. Order of operations is upstream's: harden the storage, sanitize the existing
/// file (its own failure only forgets the stat cache), append, then advance the cache. Every error
/// is swallowed — history recording must never fail the run that is finishing (upstream wraps the
/// whole body in `try {} catch {}`). It does NOT rotate: upstream rotates only on read
/// ([`load_runs_for_agent`]).
///
/// Synchronous `std::fs`, like upstream's: the file is small and the writes are single syscalls.
pub fn record_run(
    history_path: &Path,
    agent: &str,
    task: &str,
    exit_code: i32,
    duration_ms: i64,
    terminal: &RunTerminal,
) {
    let entry = RunHistoryEntry {
        agent: agent.to_string(),
        task: REDACTED_TASK.to_string(),
        task_hash: Some(hash_task(task)),
        ts: crate::time::now_epoch_millis() / 1000,
        status: if exit_code == 0 { "ok" } else { "error" }.to_string(),
        outcome: Some(run_outcome(exit_code, terminal)),
        duration: duration_ms,
        exit: (exit_code != 0).then_some(exit_code),
    };
    let attempt = || -> std::io::Result<()> {
        let line = serde_json::to_string(&entry).map_err(std::io::Error::other)?;
        harden_history_storage(history_path)?;
        let line_count = sanitize_history_file(history_path).ok();
        append_private_history_line(history_path, &line)?;
        match line_count {
            None => forget_history_file(history_path),
            Some(count) => remember_history_file(history_path, count + 1)?,
        }
        Ok(())
    };
    if let Err(err) = attempt() {
        tracing::debug!(
            path = %history_path.display(),
            error = %err,
            "run-history recording failed; the run is unaffected"
        );
    }
}

/// pi `loadRunsForAgent` (`run-history.ts:262-288` @v0.76.1): every entry for `agent`, NEWEST
/// FIRST.
///
/// Hardens (errors swallowed), reads (a missing or unreadable file is empty), sanitizes, and —
/// the ONLY place rotation lives — keeps the newest [`ROTATE_KEEP`] lines when more than
/// [`ROTATE_READ_THRESHOLD`] remain, rewriting the file when anything changed. Upstream has no
/// production caller of this at v0.76.1 (only its tests), and neither does cyrup: it is ported as
/// the tested reader, not wired to a consumer that does not exist upstream.
#[must_use]
pub fn load_runs_for_agent(history_path: &Path, agent: &str) -> Vec<RunHistoryEntry> {
    let _ = harden_history_storage(history_path);
    let Ok(raw) = std::fs::read_to_string(history_path) else {
        return Vec::new();
    };
    let (mut lines, mut changed) = sanitize_history_lines(&raw);
    if lines.len() > ROTATE_READ_THRESHOLD {
        lines.drain(..lines.len() - ROTATE_KEEP);
        changed = true;
    }
    let _ = (|| -> std::io::Result<()> {
        if changed {
            write_private_history(history_path, &lines)?;
        }
        remember_history_file(history_path, lines.len())
    })();
    let mut entries: Vec<RunHistoryEntry> = lines
        .iter()
        .filter_map(|line| serde_json::from_str::<RunHistoryEntry>(line).ok())
        .filter(|entry| entry.agent == agent)
        .collect();
    entries.reverse();
    entries
}

/// pi `backgroundRunHistoryTask` (`run-history.ts:136-145`): a run whose ONLY top-level step is a
/// single step records that step's task (pi prefers `launchBindingTask`, which cyrup has no
/// analogue of); every other run — multi-step, a lone fan-out, a lone attached root (whose task
/// upstream builds as `""`, `async-execution.ts:1352`) — records the mode word, else `"run"`.
#[must_use]
pub fn background_run_history_task(sole_single_task: Option<&str>, result_mode: &str) -> String {
    match sole_single_task {
        Some(task) if !task.is_empty() => task.to_string(),
        _ if !result_mode.is_empty() => result_mode.to_string(),
        _ => "run".to_string(),
    }
}

/// The task of a run's sole top-level step, when it is a [`RunnerStep::SingleStep`] — the
/// `steps.length === 1 && steps[0].task` half of [`background_run_history_task`].
///
/// [`RunnerStep::SingleStep`]: crate::spawn::chain_graph::RunnerStep::SingleStep
#[must_use]
pub fn sole_single_step_task(steps: &[crate::spawn::chain_graph::RunnerStep]) -> Option<&str> {
    match steps {
        [crate::spawn::chain_graph::RunnerStep::SingleStep(spec)] => Some(spec.task.as_str()),
        _ => None,
    }
}

/// One flat status step as [`plan_background_run_history`] reads it — the object the runner
/// maps `statusPayload.steps` into (`subagent-runner.ts:5106-5114`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackgroundRunHistoryStep<'a> {
    /// pi `step.agent`.
    pub agent: &'a str,
    /// pi `step.status`.
    pub status: StepState,
    /// pi `step.durationMs`; `None` falls back to the run duration.
    pub duration_ms: Option<i64>,
    /// pi `step.timedOut`.
    pub timed_out: bool,
    /// pi `step.stopped`.
    pub stopped: bool,
    /// pi `launchedFlatIndices.has(index)` — whether a child was actually dispatched.
    pub launched: bool,
}

/// pi `planBackgroundRunHistory`'s input (`run-history.ts:182-191`).
#[derive(Clone, Debug)]
pub struct BackgroundRunHistoryInput<'a> {
    /// [`background_run_history_task`]'s result — the one task string every row hashes.
    pub task: String,
    /// pi `statusSteps`.
    pub status_steps: &'a [BackgroundRunHistoryStep<'a>],
    /// pi `stepResults`, aligned with `status_steps` by flat index; read only for
    /// `processSignal`.
    pub step_results: &'a [SingleResult],
    /// pi `runDurationMs`.
    pub run_duration_ms: i64,
    /// pi run-wide `stopped`.
    pub stopped: bool,
    /// pi run-wide `interrupted`.
    pub interrupted: bool,
    /// pi run-wide `timedOut`.
    pub timed_out: bool,
}

/// One row [`plan_background_run_history`] asks [`record_run`] to write — pi
/// `BackgroundRunHistoryEntry` (`run-history.ts:147-153`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackgroundRunHistoryRow {
    /// The step's agent.
    pub agent: String,
    /// The task string to hash (the run's single task, or its mode word).
    pub task: String,
    /// `0` for a `complete` step, else `1`.
    pub exit_code: i32,
    /// The step's own duration, else the run's.
    pub duration_ms: i64,
    /// The terminal facts [`run_outcome`] reads.
    pub terminal: RunTerminal,
}

/// Census rows for one finished background run — pi `planBackgroundRunHistory`
/// (`run-history.ts:155-221` @v0.76.1), ported 1:1.
///
/// One row per flat step that actually LAUNCHED (`launched`) and is not `pending`. A step is a
/// success only when `complete` (`TERMINAL_STEP_STATUSES`, `:155`); a step that reached its own
/// end — `complete` or `failed` (`SELF_TERMINAL_STEP_STATUSES`, `:157`; cyrup has no `rejected`)
/// — keeps only its own `timedOut`/`stopped` flags, every other step also takes the run-wide
/// ones, its own overriding.
///
/// [`StepState::Partial`] is in NEITHER of upstream's sets even though upstream has the status
/// (`subagent-runner.ts:2213`, `execution.status: "partial"` at `:902`): it is therefore
/// `exitCode: 1` — `failed` by [`run_outcome`] — and not self-terminal, so a run-wide
/// stop/interrupt/timeout relabels it. That is upstream's rule as written, not a cyrup choice.
#[must_use]
pub fn plan_background_run_history(
    input: &BackgroundRunHistoryInput<'_>,
) -> Vec<BackgroundRunHistoryRow> {
    let mut rows = Vec::new();
    for (index, step) in input.status_steps.iter().enumerate() {
        if step.agent.is_empty() || step.status == StepState::Pending || !step.launched {
            continue;
        }
        let succeeded = step.status == StepState::Complete;
        let self_terminal = matches!(step.status, StepState::Complete | StepState::Failed);
        let terminal = RunTerminal {
            // `{ ...runTerminal, ...ownTerminal }` for a non-self-terminal step, `ownTerminal`
            // alone otherwise. `ownTerminal` carries only `true` flags, so "own overrides run"
            // reduces to OR for the two keys both halves share.
            stopped: step.stopped || (!self_terminal && input.stopped),
            timed_out: step.timed_out || (!self_terminal && input.timed_out),
            interrupted: !self_terminal && input.interrupted,
            process_signal: input
                .step_results
                .get(index)
                .and_then(|result| result.process_signal.clone()),
            turn_budget_exceeded: false,
        };
        rows.push(BackgroundRunHistoryRow {
            agent: step.agent.to_string(),
            task: input.task.clone(),
            exit_code: if succeeded { 0 } else { 1 },
            duration_ms: step.duration_ms.unwrap_or(input.run_duration_ms),
            terminal,
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use crate::background::run_artifact_roots;

    use super::*;

    fn history_in(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("agent").join("run-history.jsonl")
    }

    fn raw(path: &Path) -> String {
        std::fs::read_to_string(path).expect("history file exists")
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// pi `pi-coding-agent-dir.test.ts:229-246`: the task never reaches the file; the line is
    /// redacted, hashed, and keyed in upstream's exact order.
    #[test]
    fn record_run_redacts_and_hashes_the_task_in_upstreams_key_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = history_in(&dir);
        let task = "PROMPT_AUDIT_SENTINEL_1021 Inspect customer ACME token=SECRET";
        record_run(&path, "env-agent", task, 0, 42, &RunTerminal::default());

        let contents = raw(&path);
        for secret in [
            "PROMPT_AUDIT_SENTINEL_1021",
            "Inspect customer",
            "ACME",
            "SECRET",
        ] {
            assert!(!contents.contains(secret), "{secret} leaked: {contents}");
        }
        let expected_prefix = format!(
            "{{\"agent\":\"env-agent\",\"task\":\"[redacted]\",\"taskHash\":\"{}\",\"ts\":",
            hash_task(task)
        );
        assert!(contents.starts_with(&expected_prefix), "{contents}");
        assert!(
            contents
                .trim_end()
                .ends_with(",\"status\":\"ok\",\"outcome\":\"completed\",\"duration\":42}"),
            "no `exit` on a clean exit, outcome before duration: {contents}"
        );
        assert_eq!(hash_task(task).len(), 64);
        assert!(
            hash_task(task)
                .chars()
                .all(|c| matches!(c, '0'..='9' | 'a'..='f'))
        );

        let history = load_runs_for_agent(&path, "env-agent");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].task, REDACTED_TASK);
        assert_eq!(
            history[0].task_hash.as_deref(),
            Some(hash_task(task).as_str())
        );
        assert_eq!(history[0].status, "ok");
    }

    /// pi `assertPrivateHistoryModes` (`pi-coding-agent-dir.test.ts:42-46`) on a fresh write, and
    /// pi `:440-466`'s pre-existing 0755 dir / 0644 file being re-moded.
    #[cfg(unix)]
    #[test]
    fn record_run_creates_and_hardens_private_modes() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = history_in(&dir);
        record_run(&path, "a", "t", 0, 1, &RunTerminal::default());
        assert_eq!(mode_of(path.parent().unwrap()), 0o700);
        assert_eq!(mode_of(&path), 0o600);

        let dir = tempfile::tempdir().unwrap();
        let path = history_in(&dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::set_permissions(
            path.parent().unwrap(),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        std::fs::write(&path, "").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        record_run(&path, "a", "t", 0, 1, &RunTerminal::default());
        assert_eq!(mode_of(path.parent().unwrap()), 0o700);
        assert_eq!(mode_of(&path), 0o600);
    }

    /// pi `pi-coding-agent-dir.test.ts:257-271`, verbatim: explicit outcomes, newest first, and a
    /// legacy line keeps its `exit` and gains no outcome.
    #[test]
    fn outcome_ladder_matches_upstream_and_legacy_outcomes_are_not_guessed() {
        let dir = tempfile::tempdir().unwrap();
        let path = history_in(&dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "{\"agent\":\"outcome-agent\",\"task\":\"[redacted]\",\"ts\":1,\"status\":\"error\",\"duration\":1,\"exit\":143}\n",
        )
        .unwrap();
        let flag = |f: fn(&mut RunTerminal)| {
            let mut terminal = RunTerminal::default();
            f(&mut terminal);
            terminal
        };
        record_run(
            &path,
            "outcome-agent",
            "failed",
            1,
            2,
            &RunTerminal::default(),
        );
        record_run(
            &path,
            "outcome-agent",
            "timed out",
            1,
            3,
            &flag(|t| t.timed_out = true),
        );
        record_run(
            &path,
            "outcome-agent",
            "interrupted",
            1,
            4,
            &flag(|t| t.interrupted = true),
        );
        record_run(
            &path,
            "outcome-agent",
            "stopped",
            1,
            5,
            &flag(|t| t.stopped = true),
        );
        record_run(
            &path,
            "outcome-agent",
            "completed",
            0,
            6,
            &RunTerminal::default(),
        );
        record_run(
            &path,
            "outcome-agent",
            "signalled",
            143,
            7,
            &flag(|t| t.process_signal = Some("SIGTERM".to_string())),
        );

        let history = load_runs_for_agent(&path, "outcome-agent");
        let outcomes: Vec<Option<RunOutcome>> = history.iter().map(|e| e.outcome).collect();
        assert_eq!(
            outcomes,
            vec![
                Some(RunOutcome::Stopped),
                Some(RunOutcome::Completed),
                Some(RunOutcome::Stopped),
                Some(RunOutcome::Interrupted),
                Some(RunOutcome::TimedOut),
                Some(RunOutcome::Failed),
                None,
            ]
        );
        assert_eq!(history.last().unwrap().exit, Some(143));
        assert!(raw(&path).contains("\"outcome\":\"timed_out\""));

        // A signal explained by a turn-budget verdict is not a stop.
        let explained = RunTerminal {
            process_signal: Some("SIGTERM".to_string()),
            turn_budget_exceeded: true,
            ..RunTerminal::default()
        };
        assert_eq!(run_outcome(143, &explained), RunOutcome::Failed);
    }

    /// pi `pi-coding-agent-dir.test.ts:440-466`, plus cyrup's own legacy plaintext shape and the
    /// JS spread's treatment of an array line.
    #[test]
    fn existing_history_is_redacted_and_hardened_while_recording() {
        let dir = tempfile::tempdir().unwrap();
        let path = history_in(&dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let legacy_task = "legacy customer secret";
        std::fs::write(
            &path,
            format!(
                "{{\"agent\":\"env-agent\",\"task\":\"{legacy_task}\",\"ts\":1,\"status\":\"ok\",\"duration\":2,\"extra\":true}}\n\
                 not json with pasted secret\n\
                 null\n\
                 [\"array secret\"]\n"
            ),
        )
        .unwrap();

        record_run(
            &path,
            "env-agent",
            "new customer secret",
            1,
            9,
            &RunTerminal::default(),
        );

        let contents = raw(&path);
        for secret in [
            legacy_task,
            "new customer secret",
            "not json with pasted secret",
        ] {
            assert!(!contents.contains(secret), "{secret} leaked: {contents}");
        }
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 3, "non-JSON and null dropped: {contents}");
        assert_eq!(
            lines[0],
            format!(
                "{{\"agent\":\"env-agent\",\"task\":\"[redacted]\",\"ts\":1,\"status\":\"ok\",\"duration\":2,\"extra\":true,\"taskHash\":\"{}\"}}",
                hash_task(legacy_task)
            ),
            "task redacted in place, unknown key kept, new hash appended, no outcome synthesised"
        );
        // `{...["array secret"]}` is `{"0":"array secret"}`; it has no string `task`, so nothing
        // is hashed — but the `0` key passes through, exactly as upstream's spread leaves it.
        assert_eq!(lines[1], "{\"0\":\"array secret\",\"task\":\"[redacted]\"}");

        let history = load_runs_for_agent(&path, "env-agent");
        assert_eq!(history.len(), 2);
        assert_eq!(
            history[0].task_hash.as_deref(),
            Some(hash_task("new customer secret").as_str())
        );
        assert_eq!(history[0].status, "error");
        assert_eq!(history[0].exit, Some(1));
        assert_eq!(history[1].task, REDACTED_TASK);
        assert_eq!(
            history[1].task_hash.as_deref(),
            Some(hash_task(legacy_task).as_str())
        );
        assert_eq!(history[1].outcome, None);
    }

    /// pi `pi-coding-agent-dir.test.ts:468-483`: an external append invalidates the stat cache, so
    /// the next record re-sanitizes it.
    #[test]
    fn an_external_write_is_resanitized_on_the_next_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = history_in(&dir);
        record_run(
            &path,
            "env-agent",
            "first secret",
            0,
            1,
            &RunTerminal::default(),
        );
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(
            b"{\"agent\":\"env-agent\",\"task\":\"externally written secret\",\"ts\":2,\"status\":\"ok\",\"duration\":2}\n",
        )
        .unwrap();
        drop(file);

        record_run(
            &path,
            "env-agent",
            "second secret",
            0,
            3,
            &RunTerminal::default(),
        );

        let contents = raw(&path);
        for secret in ["first secret", "externally written secret", "second secret"] {
            assert!(!contents.contains(secret), "{secret} leaked: {contents}");
        }
        assert_eq!(load_runs_for_agent(&path, "env-agent").len(), 3);
    }

    /// pi `loadRunsForAgent`'s rotation (`run-history.ts:275-278`): MORE than 1200 lines keeps
    /// the newest 1000; exactly 1200 is left alone. Filtered by agent and newest first.
    #[test]
    fn load_runs_for_agent_rotates_only_past_the_read_threshold() {
        let line = |i: usize| {
            format!(
                "{{\"agent\":\"{}\",\"task\":\"[redacted]\",\"taskHash\":\"h{i}\",\"ts\":{i},\"status\":\"ok\",\"outcome\":\"completed\",\"duration\":{i}}}",
                if i.is_multiple_of(2) { "even" } else { "odd" }
            )
        };
        let seed = |path: &Path, count: usize| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let body: String = (0..count).map(|i| line(i) + "\n").collect();
            std::fs::write(path, body).unwrap();
        };

        let dir = tempfile::tempdir().unwrap();
        let path = history_in(&dir);
        seed(&path, ROTATE_READ_THRESHOLD);
        assert_eq!(load_runs_for_agent(&path, "even").len(), 600);
        assert_eq!(
            raw(&path).lines().count(),
            ROTATE_READ_THRESHOLD,
            "1200 is not > 1200"
        );

        let dir = tempfile::tempdir().unwrap();
        let path = history_in(&dir);
        seed(&path, ROTATE_READ_THRESHOLD + 1);
        let even = load_runs_for_agent(&path, "even");
        let contents = raw(&path);
        assert_eq!(contents.lines().count(), ROTATE_KEEP);
        assert_eq!(
            contents.lines().next().unwrap(),
            line(201),
            "the oldest 201 lines went"
        );
        assert_eq!(even.len(), 500);
        assert_eq!(even[0].ts, 1200, "newest first");
        assert_eq!(even.last().unwrap().ts, 202);
    }

    /// A pre-SUBA-172 cyrup line (plaintext task, no hash, no outcome) still deserializes, and an
    /// absent file reads as empty.
    #[test]
    fn load_runs_for_agent_tolerates_the_legacy_shape_and_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = history_in(&dir);
        assert!(load_runs_for_agent(&path, "researcher").is_empty());
        std::fs::write(
            &path,
            "{\"agent\":\"researcher\",\"task\":\"look into the thing\",\"ts\":5,\"status\":\"error\",\"duration\":7,\"exit\":7}\n",
        )
        .unwrap();
        let history = load_runs_for_agent(&path, "researcher");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].task, REDACTED_TASK);
        assert_eq!(
            history[0].task_hash.as_deref(),
            Some(hash_task("look into the thing").as_str())
        );
        assert_eq!(history[0].outcome, None);
        assert_eq!(history[0].exit, Some(7));
        assert!(
            !raw(&path).contains("look into the thing"),
            "the read rewrote it redacted"
        );
    }

    fn result(process_signal: Option<&str>) -> SingleResult {
        SingleResult {
            output_partial: false,
            execution: None,
            native_machine: None,
            runtime_acknowledged_extensions: None,
            skills_warning: None,
            watchdog: None,
            usage_budget: None,
            turn_budget: None,
            turn_budget_exceeded: false,
            wrap_up_requested: false,
            tool_budget_blocked: false,
            session_name: None,
            child_run_id: None,
            agent: "worker".to_string(),
            task: "a".to_string(),
            exit_code: 0,
            usage: cyrup_core::Usage::default(),
            turns: 0,
            model: None,
            attempted_models: Vec::new(),
            model_attempts: Vec::new(),
            final_output: None,
            structured_output: None,
            session_file: None,
            output_state: Default::default(),
            structured_output_path: None,
            artifact_paths: None,
            transcript_path: None,
            transcript_error: None,
            acceptance: None,
            detached: false,
            detached_reason: None,
            interrupted: false,
            timed_out: false,
            timeout_recovery: None,
            context_overflow: false,
            stopped: false,
            process_signal: process_signal.map(str::to_string),
            error: None,
            saved_output_path: None,
            tool_calls: Vec::new(),
            output_truncated: false,
            control_events: Vec::new(),
            progress: None,
            runner: None,
            external_process: None,
            tool_surface: crate::exec::tool_surface::ResolvedToolSurface::default(),
        }
    }

    fn step(
        agent: &str,
        status: StepState,
        duration_ms: Option<i64>,
    ) -> BackgroundRunHistoryStep<'_> {
        BackgroundRunHistoryStep {
            agent,
            status,
            duration_ms,
            timed_out: false,
            stopped: false,
            launched: true,
        }
    }

    fn plan(
        task: &str,
        status_steps: &[BackgroundRunHistoryStep<'_>],
        run_duration_ms: i64,
        flags: (bool, bool, bool),
    ) -> Vec<BackgroundRunHistoryRow> {
        plan_background_run_history(&BackgroundRunHistoryInput {
            task: task.to_string(),
            status_steps,
            step_results: &[],
            run_duration_ms,
            stopped: flags.0,
            interrupted: flags.1,
            timed_out: flags.2,
        })
    }

    /// pi `pi-coding-agent-dir.test.ts:507-532`: a sole single step hashes its task, everything
    /// else the mode word, and an empty task falls back.
    #[test]
    fn the_history_task_is_the_sole_single_task_else_the_mode_word() {
        assert_eq!(
            background_run_history_task(Some("fix the flaky test"), "single"),
            "fix the flaky test"
        );
        assert_eq!(background_run_history_task(None, "parallel"), "parallel");
        assert_eq!(background_run_history_task(None, "chain"), "chain");
        assert_eq!(background_run_history_task(Some(""), "single"), "single");
        assert_eq!(background_run_history_task(None, ""), "run");
    }

    /// pi `:534-553`: an unlaunched step gets no row whatever it was relabelled to.
    #[test]
    fn steps_the_runner_never_launched_get_no_row() {
        let mut stopped_ghost = step("stopped-ghost", StepState::Stopped, Some(0));
        stopped_ghost.launched = false;
        let mut timeout_ghost = step("timeout-ghost", StepState::Failed, Some(0));
        timeout_ghost.timed_out = true;
        timeout_ghost.launched = false;
        let launched = step("launched-stopped", StepState::Stopped, Some(50));
        let rows = plan(
            "parallel",
            &[stopped_ghost, timeout_ghost, launched],
            50,
            (true, false, false),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].agent, "launched-stopped");
        assert!(rows[0].terminal.stopped);
    }

    /// pi `:555-569`: a single-step run is one foreground-shaped row with an empty terminal.
    #[test]
    fn a_single_step_run_is_one_foreground_shaped_row() {
        let rows = plan(
            "fix the flaky test",
            &[step("worker", StepState::Complete, Some(4321))],
            5000,
            (false, false, false),
        );
        assert_eq!(
            rows,
            vec![BackgroundRunHistoryRow {
                agent: "worker".to_string(),
                task: "fix the flaky test".to_string(),
                exit_code: 0,
                duration_ms: 4321,
                terminal: RunTerminal::default(),
            }]
        );
    }

    /// pi `:571-587`: one row per child, the mode word, the run-duration fallback and the
    /// child's process signal.
    #[test]
    fn a_fan_out_records_one_row_per_child() {
        let steps = [
            step("worker", StepState::Complete, Some(100)),
            step("worker", StepState::Failed, None),
            step("reviewer", StepState::Complete, Some(300)),
        ];
        let results = [result(None), result(Some("SIGTERM")), result(None)];
        let rows = plan_background_run_history(&BackgroundRunHistoryInput {
            task: "parallel".to_string(),
            status_steps: &steps,
            step_results: &results,
            run_duration_ms: 999,
            stopped: false,
            interrupted: false,
            timed_out: false,
        });
        assert_eq!(
            rows.iter().map(|r| r.agent.as_str()).collect::<Vec<_>>(),
            ["worker", "worker", "reviewer"]
        );
        assert!(rows.iter().all(|r| r.task == "parallel"));
        assert_eq!(
            rows.iter().map(|r| r.exit_code).collect::<Vec<_>>(),
            [0, 1, 0]
        );
        assert_eq!(rows[1].duration_ms, 999);
        assert_eq!(rows[1].terminal.process_signal.as_deref(), Some("SIGTERM"));
    }

    /// pi `:589-603`: pending steps (a chain stopped early) get no row.
    #[test]
    fn pending_steps_get_no_row() {
        let rows = plan(
            "chain",
            &[
                step("worker", StepState::Failed, Some(50)),
                step("worker", StepState::Pending, None),
                step("reviewer", StepState::Pending, None),
            ],
            60,
            (false, false, false),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].exit_code, 1);
    }

    /// pi `:606-637`: a step that completed or failed on its own keeps its outcome when the run
    /// is interrupted later; the paused sibling takes the run's flag.
    #[test]
    fn a_self_terminal_step_keeps_its_own_outcome_when_the_run_is_interrupted() {
        for own in [StepState::Complete, StepState::Failed] {
            let rows = plan(
                "chain",
                &[
                    step("worker", own, Some(100)),
                    step("worker", StepState::Paused, None),
                ],
                200,
                (false, true, false),
            );
            assert_eq!(rows[0].terminal, RunTerminal::default(), "{own:?}");
            assert_eq!(rows[0].exit_code, i32::from(own != StepState::Complete));
            assert!(rows[1].terminal.interrupted);
        }
    }

    /// pi `:639-647` and `:649-661`: a self-terminal step's own timeout flag survives; run-level
    /// flags reach every non-terminal row, which is never a success.
    #[test]
    fn own_and_run_level_terminal_flags() {
        let mut timed_out = step("worker", StepState::Failed, Some(30));
        timed_out.timed_out = true;
        let rows = plan("a", &[timed_out], 30, (false, false, false));
        assert_eq!(
            rows[0].terminal,
            RunTerminal {
                timed_out: true,
                ..RunTerminal::default()
            }
        );
        assert_eq!(
            run_outcome(rows[0].exit_code, &rows[0].terminal),
            RunOutcome::TimedOut
        );

        let rows = plan(
            "chain",
            &[
                step("worker", StepState::Paused, None),
                step("worker", StepState::Stopped, None),
            ],
            10,
            (false, true, false),
        );
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter()
                .all(|r| r.terminal.interrupted && !r.terminal.timed_out)
        );
        assert!(rows.iter().all(|r| r.exit_code == 1));
    }

    /// `StepState::Partial` is in neither of upstream's sets (`run-history.ts:155,157`): exit 1,
    /// and NOT self-terminal, so a run-wide flag relabels it.
    #[test]
    fn a_partial_step_is_a_failure_that_run_flags_relabel() {
        let rows = plan(
            "single",
            &[step("worker", StepState::Partial, Some(5))],
            5,
            (false, false, false),
        );
        assert_eq!(
            run_outcome(rows[0].exit_code, &rows[0].terminal),
            RunOutcome::Failed
        );
        let rows = plan(
            "single",
            &[step("worker", StepState::Partial, Some(5))],
            5,
            (false, false, true),
        );
        assert_eq!(
            run_outcome(rows[0].exit_code, &rows[0].terminal),
            RunOutcome::TimedOut
        );
    }

    #[test]
    fn run_history_path_is_the_agent_dir_not_the_scratch_root() {
        let path = run_history_path();
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("run-history.jsonl")
        );
        assert_eq!(path.parent(), Some(crate::paths::agent_dir().as_path()));
        assert!(
            !path.starts_with(temp_root_dir()),
            "durable run history must not live inside the disposable scratch root: {path:?}"
        );
    }

    /// [`run_history_path_for`]: a run in the canonical scratch root records to the agent dir (pi's
    /// unconditional `getHistoryPath()`); a run whose roots were REDIRECTED records beside those
    /// roots instead.
    ///
    /// The second half is the regression: in-process `run()` callers hand the runner a `TempDir`
    /// for every path except this one, so re-deriving it wrote 136 lines of synthetic test history
    /// (`researcher`, `"do the thing"`, `scout`) into a real `~/.cyrup/agent/run-history.jsonl` on
    /// one full workspace gate.
    #[test]
    fn run_history_path_for_follows_a_redirected_run_and_never_the_real_agent_dir() {
        // Canonical root -> pi's unconditional agent-dir path, unchanged.
        let canonical = run_artifact_roots(Path::new("/some/project")).async_root;
        assert_eq!(
            run_history_path_for(&canonical),
            run_history_path(),
            "a run under the canonical scratch root keeps pi's `getAgentDir()/run-history.jsonl`"
        );

        // Redirected root (what every in-process `run()` test hands over) -> beside those roots.
        let redirected = Path::new("/some/tempdir/async");
        assert_eq!(
            run_history_path_for(redirected),
            PathBuf::from("/some/tempdir/run-history.jsonl")
        );
        assert_ne!(
            run_history_path_for(redirected),
            run_history_path(),
            "a redirected run must NEVER append to the real user's durable run history"
        );

        // A root with no parent degrades to the root itself rather than panicking.
        assert_eq!(
            run_history_path_for(Path::new("/")),
            PathBuf::from("/run-history.jsonl")
        );
    }
}
