//! The in-pane inspector process — pi `src/inspectors/inspector-runner.ts` (153 lines @v0.68.0).
//!
//! An inspector is a terminal pane running a SEPARATE process that clears the screen, renders a
//! lifecycle dashboard for ONE async run every `refresh_ms`, and reads control lines off stdin
//! (`status`, `stop`, `steer <message>`, or plain guidance). It is a MIRROR: closing the pane does
//! not stop the run (`inspector-runner.ts:34`, the sentence [`format_inspector_dashboard`] emits
//! verbatim).
//!
//! Nothing in `inspectors/` ever SPAWNS this process. `openHerdrInspector` hands
//! [`InspectorLaunch::display_command`](super::types::InspectorLaunch::display_command) to
//! `herdr pane run <paneId> <command>` (`herdr/actions.ts:111`) and ghostty hands it to AppleScript
//! as `command of surfaceConfiguration` (`ghostty/actions.ts:15,61`); the TERMINAL HOST starts it.
//! `inspector.command` just returns that string (`actions.ts:130`). This module is therefore
//! reached from exactly two directions: [`super::actions::launch_for`] BUILDS the argv, and
//! `cyrup`'s `__subagent-inspector` predispatch hop RUNS it.
//!
//! # The cross-crate seam, and the failure mode it has
//!
//! [`crate::inspectors::types::INSPECTOR_SUBCOMMAND`] is the reserved argv token the launch puts where upstream puts
//! `inspector-runner.mjs`. CLASSIFICATION of that token lives in `crates/cyrup`
//! (`predispatch::classify_internal`) and DISPATCH lives in `crates/cyrup/src/main.rs`, because
//! `set_process_name` needs `unsafe` and only the binary crate may hold it
//! (`predispatch.rs`'s module doc). A `predispatch` arm added WITHOUT the `main.rs` arm does not
//! fail loudly — it falls through to clap, which rejects `--async-dir` with a usage error and exit
//! 2. The end-to-end test
//! `crates/cyrup-it/tests/subagents/inspector_runner_subcommand_integration.rs` exists to catch
//! exactly that split, and it is the only thing that can.
//!
//! # Testability
//!
//! [`run_inspector`] takes an injected argv, an injected [`tokio::io::AsyncBufRead`] stdin and an
//! injected [`tokio::io::AsyncWrite`] stdout, so the dashboard, the control verbs and the argv
//! parser are all unit-testable with no TTY, no pane and no herdr.

use std::path::{Path, PathBuf};

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

use crate::background::control::{
    StopRequest, read_status_file, request_async_steer, request_async_stop,
};
use crate::background::fleet_view::format_async_run_transcript;
use crate::background::{RunId, RunMode, RunPaths, RunState, RunStatus};
use crate::missions::MissionRecord;
use crate::missions::store::parse_mission_record;

use super::session_roots_codec::{SessionRootsDecodeError, decode_session_roots};

// =================================================================================================
// The reserved argv token and the header line both live in the CONTRACT
// =================================================================================================
//
// `INSPECTOR_SUBCOMMAND` and `INSPECTOR_HEADER_PREFIX` used to be declared here, in this
// implementation module, and were reached across module boundaries by `inspectors/herdr/actions.rs`
// and by `crates/cyrup/src/subagent_inspector_cmd.rs`. They are shared vocabulary, so they are in
// `inspectors/types.rs` now with the rest of the contract; see
// [`crate::inspectors::types::inspector_header_line`] for the coupling the header half carries.

/// The header on the degraded render, when the run's `status.json` cannot be read at all
/// (`inspector-runner.ts:127`'s `pi-subagents inspector`).
const INSPECTOR_HEADER_BARE: &str = "cyrup-inspector";

/// pi's `steeringMessagePreview` budget (`runs/background/steering.ts:20`).
const STEERING_MESSAGE_PREVIEW_LIMIT: usize = 160;

/// pi's transcript window for the dashboard body (`inspector-runner.ts:43`'s `lines: 60`).
const DASHBOARD_TRANSCRIPT_LINES: i64 = 60;

/// pi's `refreshMs` default and floor (`inspector-runner.ts:67-68`).
const DEFAULT_REFRESH_MS: u64 = 1_500;
/// The floor `--refresh-ms` is validated against (`inspector-runner.ts:68`).
const MIN_REFRESH_MS: u64 = 250;

/// The `source` every control request this process writes carries — pi's own literal
/// (`inspector-runner.ts:94,107`).
pub const INSPECTOR_CONTROL_SOURCE: &str = "inspector-runner";

/// `\x1b[2J\x1b[H` — clear screen, home cursor (`inspector-runner.ts:127,130`).
const CLEAR_SCREEN: &str = "\x1b[2J\x1b[H";

// =================================================================================================
// RunnerOptions + parse_args — `inspector-runner.ts:14-23`, `:50-79`
// =================================================================================================

/// pi `RunnerOptions` (`inspector-runner.ts:14-23`).
///
/// `allow_steer` / `allow_stop` are plain `bool` rather than `Option<bool>`: upstream's optional
/// fields are only ever produced by [`parse_args`], which always sets them
/// (`:76-77`), and every reader compares them against the literal `false` (`:45`, `:86`, `:105`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerOptions {
    /// The run's own directory — `<async_root>/<run_id>`, absolutised (`:70`).
    pub async_dir: PathBuf,
    /// The run this inspector mirrors.
    pub run_id: String,
    /// The child this inspector is scoped to, when it is child-specific (`:72`).
    pub index: Option<usize>,
    /// The mission record to render above the transcript, when one was resolved (`:73`).
    pub mission_path: Option<PathBuf>,
    /// The dashboard refresh period in milliseconds (`:67`, default 1 500, floor 250).
    pub refresh_ms: u64,
    /// Whether the authority policy allows steering from this pane (`:76`).
    pub allow_steer: bool,
    /// Whether it allows stopping (`:77`).
    pub allow_stop: bool,
    /// The trusted session roots the transcript reader is allowed to read a session file from
    /// (`:65`).
    pub session_roots: Vec<PathBuf>,
}

/// Every refusal pi's `parseArgs` throws (`inspector-runner.ts:55,60,63,68`), plus the one
/// `decodeSessionRoots` raises (`session-roots-codec.ts:27-30`).
///
/// Each `Display` is byte-identical to upstream's `throw new Error(...)` text, because this is what
/// the pane prints: `Inspector failed: {message}` (`:150`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InspectorArgvError {
    /// `:55` — a key that does not start with `--`, or a key with no value after it.
    #[error("Invalid inspector argument '{0}'.")]
    InvalidArgument(String),
    /// `:60` — neither `--async-dir` nor `--run-id` may be missing or empty.
    #[error("Inspector requires --async-dir and --run-id.")]
    MissingRequired,
    /// `:63`.
    #[error("--index must be a non-negative integer.")]
    InvalidIndex,
    /// `:68`.
    #[error("--refresh-ms must be an integer >= 250.")]
    InvalidRefreshMs,
    /// `:65` — the one stable sentence `decodeSessionRoots` collapses every failure into.
    #[error(transparent)]
    SessionRoots(#[from] SessionRootsDecodeError),
}

/// `Number(raw)` for the two numeric flags — JavaScript's own string→number coercion, as far as it
/// is reachable here.
///
/// Upstream feeds `Number()` straight into `Number.isInteger` (`:63`, `:68`), so the only
/// distinctions that survive are "an integer", "a non-integer number" and "not a number at all";
/// the last two collapse into the same refusal. Ported literally: an empty/whitespace-only string
/// is `0` (`Number("") === 0`, and `Number.isInteger(0)` is true — a genuinely reachable edge, not
/// an accident), `Infinity`/`NaN` are rejected by the finiteness test exactly as
/// `Number.isInteger` rejects them, and a fractional or exponent form that lands on a whole number
/// (`"1e3"`) is accepted exactly as upstream accepts it.
///
/// [CYRUP-DELTA, unrepresentable]: JavaScript also accepts the `0x`/`0o`/`0b` literal forms, which
/// [`f64::from_str`] does not. `--index 0x10` refuses here and parses as 16 upstream. Nothing
/// produces those forms — [`super::actions::launch_for`] writes `index.to_string()` and a human
/// typing the flag types decimal — and accepting them would mean hand-rolling a second numeric
/// grammar for one unreachable spelling.
fn js_whole_number(raw: &str) -> Option<f64> {
    let trimmed = raw.trim();
    let value = if trimmed.is_empty() {
        0.0
    } else {
        trimmed.parse::<f64>().ok()?
    };
    if value.is_finite() && value.fract() == 0.0 {
        Some(value)
    } else {
        None
    }
}

/// One key's value out of [`parse_args`]'s pairwise scan.
///
/// A free function rather than a closure on purpose: a closure written
/// `|key: &str| -> Option<&str>` has its output lifetime ELIDED TO THE INPUT'S, which is the wrong
/// one — the value borrows from `argv`, not from the key being looked up — and does not compile.
fn lookup<'a>(values: &[(&'a str, &'a str)], key: &str) -> Option<&'a str> {
    values
        .iter()
        .find(|entry| entry.0 == key)
        .map(|entry| entry.1)
}

/// pi `parseArgs` (`inspector-runner.ts:50-79`) — a deliberately hand-rolled, strictly PAIRWISE
/// scan, matching `crates/cyrup/src/subagent_runner_cmd.rs:88`'s own stated rationale ("a
/// hand-rolled scan is clearer and lighter than pulling `clap` in for a one-flag internal contract
/// never shown to a user").
///
/// Strictly pairwise is load-bearing and is NOT a loose flag parser: `argv[i]` must start with
/// `--` and `argv[i + 1]` must exist, or the whole parse refuses (`:55`). A repeated key takes the
/// LAST value (`Map.set`, `:56`).
///
/// `--allow-steer` / `--allow-stop` are `!== "false"` (`:76-77`), NOT a boolean parse: `--allow-steer
/// FALSE`, `--allow-steer 0` and `--allow-steer no` all mean TRUE. Ported literally, because the
/// producer ([`super::actions::launch_for`]) writes exactly `"true"`/`"false"` and anything else
/// arriving here came from a human editing the command by hand, where upstream's permissive
/// direction (steering stays available) is the safe one.
///
/// # Errors
///
/// [`InspectorArgvError`], whose `Display` is upstream's own sentence for that failure.
pub fn parse_args(argv: &[String]) -> Result<RunnerOptions, InspectorArgvError> {
    let mut values: Vec<(&str, &str)> = Vec::new();
    let mut index = 0usize;
    while let Some(key) = argv.get(index) {
        let value = argv.get(index.saturating_add(1));
        match value {
            Some(value) if key.starts_with("--") => {
                // `Map.set` semantics: a repeated key replaces the earlier value.
                if let Some(existing) = values.iter_mut().find(|entry| entry.0 == key.as_str()) {
                    existing.1 = value.as_str();
                } else {
                    values.push((key.as_str(), value.as_str()));
                }
            }
            _ => return Err(InspectorArgvError::InvalidArgument(key.clone())),
        }
        index = index.saturating_add(2);
    }
    // `if (!asyncDir || !runId)` (`:60`) — in JavaScript an EMPTY STRING is falsy, so
    // `--run-id ""` takes this arm rather than producing a run id nothing can match.
    let async_dir = lookup(&values, "--async-dir").filter(|value| !value.is_empty());
    let run_id = lookup(&values, "--run-id").filter(|value| !value.is_empty());
    let (Some(async_dir), Some(run_id)) = (async_dir, run_id) else {
        return Err(InspectorArgvError::MissingRequired);
    };

    let child_index = match lookup(&values, "--index") {
        None => None,
        Some(raw) => {
            let number = js_whole_number(raw).ok_or(InspectorArgvError::InvalidIndex)?;
            if number < 0.0 {
                return Err(InspectorArgvError::InvalidIndex);
            }
            // `usize` is the crate's index type; the `f64` above already proved it is a
            // non-negative whole number, and a value past `usize::MAX` cannot address a step.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let narrowed = number as usize;
            Some(narrowed)
        }
    };

    let session_roots = match lookup(&values, "--session-roots") {
        None => Vec::new(),
        Some(raw) => decode_session_roots(raw)?,
    };

    let refresh_ms = match lookup(&values, "--refresh-ms") {
        None => DEFAULT_REFRESH_MS,
        Some(raw) => {
            let number = js_whole_number(raw).ok_or(InspectorArgvError::InvalidRefreshMs)?;
            // A negative value narrows to 0, which the floor below refuses anyway — the same
            // verdict `Number.isInteger(refreshMs) || refreshMs < 250` reaches upstream.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let narrowed = if number < 0.0 { 0u64 } else { number as u64 };
            if narrowed < MIN_REFRESH_MS {
                return Err(InspectorArgvError::InvalidRefreshMs);
            }
            narrowed
        }
    };

    Ok(RunnerOptions {
        // `path.resolve(asyncDir)` (`:70`) — absolutised against the process cwd, WITHOUT
        // resolving symlinks (which `canonicalize` would do, and which would break a run whose
        // async root is reached through one).
        async_dir: std::path::absolute(async_dir).unwrap_or_else(|_| PathBuf::from(async_dir)),
        run_id: run_id.to_string(),
        index: child_index,
        // `values.get("--mission-path")` is a TRUTHINESS test (`:73`), so an empty value means no
        // mission, not a mission at the process cwd.
        mission_path: lookup(&values, "--mission-path")
            .filter(|value| !value.is_empty())
            .map(|value| std::path::absolute(value).unwrap_or_else(|_| PathBuf::from(value))),
        refresh_ms,
        allow_steer: lookup(&values, "--allow-steer") != Some("false"),
        allow_stop: lookup(&values, "--allow-stop") != Some("false"),
        session_roots,
    })
}

// =================================================================================================
// The lifecycle predicate — upstream's, NOT this crate's `RunState::is_terminal`
// =================================================================================================

/// pi `isTerminal` (`inspector-runner.ts:81-83`): `state !== "queued" && state !== "running"`.
///
/// **Deliberately not [`RunState::is_terminal`].** That one answers the crate's own question —
/// "did this run reach a terminal record" — and excludes [`RunState::Paused`], because a paused
/// run is resumable (`background/state.rs`'s `RunState::Paused` doc). Upstream's inspector
/// predicate INCLUDES paused: a paused run cannot be steered and cannot be stopped from the pane,
/// and its dashboard stops refreshing. Routing this through `RunState::is_terminal` would leave a
/// paused run's timer spinning forever and would accept a steer the runner has no live child to
/// deliver to.
fn is_terminal_for_inspector(state: RunState) -> bool {
    !matches!(state, RunState::Queued | RunState::Running)
}

// =================================================================================================
// steering_receipt — pi `runs/background/steering.ts:22-24`, `:41-46`
// =================================================================================================

/// pi `steeringMessagePreview` (`steering.ts:22-24`): redact, then preview to 160 UTF-16 units.
///
/// Both halves already exist in this crate —
/// `crate::watchdog::permission_arbiter::redact_secret_values` is the port of upstream's
/// `redactSecretValues` (`shared/permissions.ts:15,70`) and
/// [`preview_display_text`](crate::workflows::preview_display_text) is the port of
/// `previewDisplayText` (`shared/display-text.ts:95-100`), sanitise-then-ellipsise and all. This
/// function is only their composition, in upstream's order: REDACT FIRST, so a secret cannot be
/// split across the ellipsis boundary and survive half-redacted.
#[must_use]
pub fn steering_message_preview(message: &str) -> String {
    let redacted = crate::watchdog::permission_arbiter::redact_secret_values(message);
    crate::workflows::preview_display_text(&redacted, STEERING_MESSAGE_PREVIEW_LIMIT)
}

/// pi `steeringReceipt` (`steering.ts:41-46`) — the receipt sentence, then the message that was
/// actually sent, inside a fence LONGER than the longest backtick run the preview contains.
///
/// The widening fence is not decoration: a preview containing ```` ``` ```` would otherwise close
/// the block early and spill the rest of the message into the surrounding render as live markup.
/// Upstream scans for every `` `{3,} `` run and picks `max(2, longest) + 1` (`:43-44`); ported
/// exactly, including the `max(2, …)` floor that makes a preview with no fence at all use three
/// backticks.
#[must_use]
pub fn steering_receipt(message: &str, receipt: &str) -> String {
    let preview = steering_message_preview(message);
    let longest = longest_backtick_run(&preview).max(2);
    let fence = "`".repeat(longest.saturating_add(1));
    format!("{receipt}\n\nMessage sent:\n{fence}text\n{preview}\n{fence}")
}

/// The length of the longest run of three-or-more backticks in `value` — upstream's
/// `[...preview.matchAll(/`{3,}/g)].map((match) => match[0]!.length)` (`steering.ts:43`).
///
/// Runs SHORTER than three are not candidates upstream (the regex demands `{3,}`) and are not
/// counted here either; the `max(2, …)` floor at the call site is what supplies the default.
fn longest_backtick_run(value: &str) -> usize {
    let mut longest = 0usize;
    let mut current = 0usize;
    for ch in value.chars() {
        if ch == '`' {
            current = current.saturating_add(1);
            if current >= 3 && current > longest {
                longest = current;
            }
        } else {
            current = 0;
        }
    }
    longest
}

// =================================================================================================
// The dashboard — `inspector-runner.ts:30-48`
// =================================================================================================

/// Everything [`format_inspector_dashboard`] renders from — pi's inline `input` object
/// (`inspector-runner.ts:30`).
#[derive(Debug, Clone, Copy)]
pub struct DashboardInput<'a> {
    /// The run's current on-disk status.
    pub status: &'a RunStatus,
    /// The run's own directory, for the transcript reader's containment check.
    pub async_dir: &'a Path,
    /// The child this pane is scoped to, when it is child-specific.
    pub index: Option<usize>,
    /// The mission backing the run, when one was resolved and parsed.
    pub mission: Option<&'a MissionRecord>,
    /// Whether the controls line offers steering.
    pub allow_steer: bool,
    /// Whether it offers stop.
    pub allow_stop: bool,
    /// Trusted roots for the session-transcript fallback.
    pub session_roots: &'a [PathBuf],
}

/// pi `formatInspectorDashboard` (`inspector-runner.ts:30-48`).
///
/// The BODY is [`format_async_run_transcript`], which already exists
/// (`background/fleet_view.rs:1034`) and is the port of upstream's own `formatAsyncRunTranscript`
/// (`fleet-view.ts:398-449`) — the same function upstream calls here (`:43`). It is reused, not
/// re-implemented: its three-source ladder (step output log → `recentOutput` ring → persisted
/// session JSONL) and its containment-checked reads are precisely what the pane needs, and a
/// second copy would be a second thing to drift.
///
/// Two ports worth naming:
///
/// * The CONTROLS line degrades on two independent axes (`:44-45`). `type guidance` is dropped by
///   `allow_steer == false` **or** by the pane not accepting plain guidance (no `--index` on a
///   non-`single` run); `steer <message>` is dropped by `allow_steer == false` alone; `stop` by
///   `allow_stop == false` alone; `status` is never dropped. A `single`-mode run with
///   `allow_stop == false` therefore still offers `type guidance`, which is the case a single
///   fixed string gets wrong.
/// * Upstream's transcript call cannot fail (it throws, and the throw escapes into the interval
///   callback). cyrup's returns `Err(pi's out-of-range sentence)` for an `--index` naming no step;
///   that text is pushed into the body rather than propagated, because a dashboard that renders
///   the reason is strictly better for the human reading the pane than a process that dies on its
///   first refresh.
#[must_use]
pub fn format_inspector_dashboard(input: &DashboardInput<'_>) -> String {
    let status = input.status;
    let mut lines: Vec<String> = vec![
        crate::inspectors::types::inspector_header_line(status.run_id.as_str()),
        "This inspector mirrors lifecycle artifacts; closing it does not stop the run.".to_string(),
        String::new(),
    ];

    if let Some(mission) = input.mission {
        lines.push(format!(
            "Mission: {} ({})",
            mission.title,
            mission.status.as_str()
        ));
        lines.push(format!("Mission id: {}", mission.id));
        let open: Vec<String> = mission
            .decisions
            .iter()
            .filter(|decision| decision.status == crate::missions::MissionDecisionStatus::Open)
            .map(|decision| format!("{}: {}", decision.id, decision.title))
            .collect();
        if !open.is_empty() {
            lines.push(format!("Open decisions: {}", open.join(" | ")));
        }
        lines.push(String::new());
    }

    let paths = run_paths_for(input.async_dir, status);
    match format_async_run_transcript(
        status,
        &paths,
        input.index,
        Some(DASHBOARD_TRANSCRIPT_LINES),
        input.session_roots,
    ) {
        Ok(body) => lines.push(body),
        Err(message) => lines.push(message),
    }

    let accepts_plain_guidance = input.index.is_some() || status.mode == RunMode::Single;
    let mut controls: Vec<&str> = Vec::new();
    if input.allow_steer && accepts_plain_guidance {
        controls.push("type guidance");
    }
    if input.allow_steer {
        controls.push("steer <message>");
    }
    if input.allow_stop {
        controls.push("stop");
    }
    controls.push("status");

    lines.push(String::new());
    lines.push(format!("Controls: {}", controls.join(" | ")));
    lines.push(
        "Supervisor replies remain in the parent cyrup session (subagent_supervisor/intercom); \
         this inspector is read-only."
            .to_string(),
    );
    lines.join("\n")
}

/// The [`RunPaths`] the transcript reader needs, from the one directory the pane was given.
///
/// [`RunPaths::for_run`] derives `<async_root>/<run_id>` (`RunDir::new`), so the async root is this
/// directory's parent and the run-id component is this directory's OWN final component — not
/// `--run-id`. Taking the component rather than the flag is what keeps `run_dir` byte-identical to
/// the directory that was handed in even if the two ever disagree; every path the transcript reads
/// (`step_output_log`, `events`, `run_log_md`) hangs off `run_dir`, so an inferred directory would
/// silently read another run's artifacts. `results_dir` is the async root: this reader never
/// touches it (`fleet_view.rs:1034-1145` consults only `run_dir`, `events` and `run_log_md`), and
/// the pane has no way to learn the real one.
fn run_paths_for(async_dir: &Path, status: &RunStatus) -> RunPaths {
    let async_root = async_dir.parent().unwrap_or(async_dir);
    let dir_token = async_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| status.run_id.as_str().to_string());
    RunPaths::for_run(async_root, async_root, &RunId::from_token(dir_token))
}

// =================================================================================================
// submit_inspector_control — `inspector-runner.ts:85-118`
// =================================================================================================

/// Every refusal `submitInspectorControl` and `queueInspectorSteer` throw
/// (`inspector-runner.ts:86,87,90,103,105,106,112,115,116`), plus the one failure channel cyrup has
/// and JavaScript does not: the control-channel write itself.
///
/// Each `Display` is byte-identical to upstream's, because the pane prints it as
/// `Control error: {message}` (`:138`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InspectorControlError {
    /// `:86`.
    #[error("Authority policy does not allow steer from this inspector.")]
    SteerForbidden,
    /// `:105`.
    #[error("Authority policy does not allow stop from this inspector.")]
    StopForbidden,
    /// `:103` — no `status.json`, or one naming a different run.
    #[error("Lifecycle status for run '{run_id}' is unavailable.")]
    StatusUnavailable {
        /// The run this pane was opened for.
        run_id: String,
    },
    /// `:87`.
    #[error("Run '{run_id}' is {state} and cannot be steered.")]
    NotSteerable {
        /// The run.
        run_id: String,
        /// Its wire-spelled state.
        state: &'static str,
    },
    /// `:106`.
    #[error("Run '{run_id}' is {state} and cannot be stopped.")]
    NotStoppable {
        /// The run.
        run_id: String,
        /// Its wire-spelled state.
        state: &'static str,
    },
    /// `:90` — an aggregate pane with no `--index`, on a run with nothing running.
    #[error(
        "No running child is available to steer. Open a child-specific inspector for a pending \
         child."
    )]
    NoRunningChild,
    /// `:112`.
    #[error("steer requires a message.")]
    EmptySteerMessage,
    /// `:115`.
    #[error(
        "Supervisor replies are owned by the parent cyrup session; use \
         subagent_supervisor/intercom there."
    )]
    ReplyNotOwnedHere,
    /// `:116`.
    #[error(
        "Plain guidance requires a child-specific inspector. Use steer <message> to target all \
         running children from the aggregate inspector."
    )]
    PlainGuidanceNeedsChild,
    /// cyrup-only: the control request could not be written. Upstream's `requestAsyncSteer` /
    /// `requestAsyncStop` are synchronous and throw into the same `catch` that renders
    /// `Control error: …` (`:138`), so this joins them rather than becoming a second channel.
    #[error("{0}")]
    ControlChannel(String),
}

/// pi `submitInspectorControl` (`inspector-runner.ts:99-118`) — one line of stdin, one reply.
///
/// The ORDER is upstream's and is load-bearing at two points:
///
/// 1. An empty line and `status` answer `"Status refreshed."` **before** `status.json` is read
///    (`:101`), so pressing Enter in a pane whose run has been swept still refreshes instead of
///    refusing.
/// 2. `reply …` is rejected (`:115`) **after** `steer …` is matched (`:110`) and **before** the
///    plain-guidance fallthrough (`:116`), so `reply` never becomes guidance text.
///
/// # Errors
///
/// [`InspectorControlError`], whose `Display` is upstream's own sentence.
pub async fn submit_inspector_control(
    options: &RunnerOptions,
    line: &str,
) -> Result<String, InspectorControlError> {
    let command = line.trim();
    if command.is_empty() || command == "status" {
        return Ok("Status refreshed.".to_string());
    }

    let status =
        read_run_status(options)
            .await
            .ok_or_else(|| InspectorControlError::StatusUnavailable {
                run_id: options.run_id.clone(),
            })?;

    if command == "stop" {
        if !options.allow_stop {
            return Err(InspectorControlError::StopForbidden);
        }
        if is_terminal_for_inspector(status.state) {
            return Err(InspectorControlError::NotStoppable {
                run_id: options.run_id.clone(),
                state: status.state.as_wire_word(),
            });
        }
        request_async_stop(
            &options.async_dir,
            StopRequest::new(INSPECTOR_CONTROL_SOURCE, None),
        )
        .await
        .map_err(|error| InspectorControlError::ControlChannel(error.to_string()))?;
        return Ok(format!("Stop requested for run {}.", options.run_id));
    }

    // [CYRUP-EXCEEDS-UPSTREAM] the bare `steer` line reaches upstream's own refusal.
    //
    // Upstream writes the refusal (`inspector-runner.ts:112` — *"steer requires a message."*) and
    // then makes it UNREACHABLE: `command` is already `line.trim()`ed at `:100`, so a line that
    // starts with `steer ` can never have an empty remainder, and a line that is just `steer`
    // fails `startsWith("steer ")` and falls through to `:116`'s plain-guidance arm — which sends
    // the literal word `steer` to every running child as guidance. Matching the bare word here
    // routes it to the sentence upstream already wrote for it, in upstream's own bytes and in
    // upstream's own position (after `stop`, before `reply`), so the decision ORDER is unchanged
    // and only the dead branch becomes live.
    if command == "steer" || command.starts_with("steer ") {
        let message = command["steer".len()..].trim();
        if message.is_empty() {
            return Err(InspectorControlError::EmptySteerMessage);
        }
        return queue_inspector_steer(options, &status, message).await;
    }

    if command.starts_with("reply ") {
        return Err(InspectorControlError::ReplyNotOwnedHere);
    }

    if options.index.is_none() && status.mode != RunMode::Single {
        return Err(InspectorControlError::PlainGuidanceNeedsChild);
    }

    queue_inspector_steer(options, &status, command).await
}

/// pi `queueInspectorSteer` (`inspector-runner.ts:85-97`).
///
/// # [CYRUP-DELTA] the plural target collapses to `None`, and WHY the guard must not collapse with it
///
/// Upstream computes `runningIndexes` from `status.steps` and, when no single target is addressed,
/// puts the whole SNAPSHOT on the wire as `targetIndexes: runningIndexes` (`:93`). cyrup's
/// [`SteerRequest::target_index`](crate::background::control::SteerRequest::target_index) is
/// `Option<usize>` whose `None` already means "every currently running child, which is what the
/// runner fans it out to" (`background/control.rs:1183-1184`), so the plural collapses to `None` —
/// a RESOLUTION-TIME difference, not a wire-shape one, and the better answer: upstream's snapshot
/// can name a child that finished between the write and the drain, cyrup's resolves against the
/// run's actual state when the runner drains it.
///
/// Grep that keeps the premise honest:
/// `rg -n 'target_index' crates/cyrup-ext-subagents/src/background/control.rs` — one
/// `Option<usize>` field, no plural sibling.
///
/// **What the collapse must NOT take with it** is upstream's `:90` refusal. `None` would otherwise
/// succeed into an empty fan-out: the human types guidance, the pane says "queued", and nothing
/// ever receives it. So `running_indexes` is still computed here, PURELY to drive that guard.
async fn queue_inspector_steer(
    options: &RunnerOptions,
    status: &RunStatus,
    message: &str,
) -> Result<String, InspectorControlError> {
    if !options.allow_steer {
        return Err(InspectorControlError::SteerForbidden);
    }
    if is_terminal_for_inspector(status.state) {
        return Err(InspectorControlError::NotSteerable {
            run_id: options.run_id.clone(),
            state: status.state.as_wire_word(),
        });
    }
    let running_children = status
        .steps
        .iter()
        .filter(|step| step.status == crate::background::StepState::Running)
        .count();
    let target_index = options
        .index
        .or_else(|| (status.mode == RunMode::Single).then_some(0));
    if target_index.is_none() && running_children == 0 {
        return Err(InspectorControlError::NoRunningChild);
    }
    request_async_steer(
        &options.async_dir,
        message,
        target_index,
        Some(INSPECTOR_CONTROL_SOURCE),
    )
    .await
    .map_err(|error| InspectorControlError::ControlChannel(error.to_string()))?;
    Ok(steering_receipt(
        message,
        &format!("Steering queued for run {}.", options.run_id),
    ))
}

/// pi `readStatus(options.asyncDir)` plus its `status.runId !== options.runId` guard
/// (`inspector-runner.ts:102-103`, `:126`), as ONE fallible read.
///
/// [`read_status_file`] is this crate's port of upstream's `readStatus` — a RAW read with no
/// reconciliation, which is exactly what a read-only mirror wants: an inspector must never repair
/// or rewrite the run it is watching.
async fn read_run_status(options: &RunnerOptions) -> Option<RunStatus> {
    let path = crate::background::RunDir::for_existing(&options.async_dir).status();
    let status = read_status_file(&path).await.ok().flatten()?;
    (status.run_id.as_str() == options.run_id).then_some(status)
}

/// The mission record to render, or `None` — pi `readMission` (`inspector-runner.ts:25-28`).
///
/// Every failure is `None`: no path, an unreadable file, invalid JSON, a record
/// [`parse_mission_record`] refuses. Upstream's single `catch` (`:27`) makes no distinction, and
/// neither does this: a mission is decoration on a dashboard whose subject is the RUN, so a broken
/// mission file must never stop the pane from rendering.
async fn read_mission(path: Option<&Path>) -> Option<MissionRecord> {
    let path = path?;
    let bytes = tokio::fs::read(path).await.ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    parse_mission_record(&value, &path.to_string_lossy()).ok()
}

// =================================================================================================
// run_inspector — `inspector-runner.ts:120-146`
// =================================================================================================

/// Every way the pane process itself can fail before, or instead of, rendering.
#[derive(Debug, thiserror::Error)]
pub enum InspectorRunError {
    /// The argv did not parse. Printed by the caller as `Inspector failed: {message}` (`:150`).
    #[error(transparent)]
    Argv(#[from] InspectorArgvError),
    /// stdout could not be written — the pane went away mid-render.
    #[error("Inspector output failed: {0}")]
    Output(#[from] std::io::Error),
}

/// pi `runInspector` (`inspector-runner.ts:120-146`) — parse, render, then refresh on a timer while
/// reading control lines, with an injected argv, stdin and stdout.
///
/// `argv` is the subcommand's OWN arguments, i.e. everything after the `__subagent-inspector`
/// token, matching upstream's `process.argv.slice(2)` (`:120`).
///
/// The loop is upstream's, shaped to tokio:
///
/// * one render immediately (`:141`), before any timer is armed, so a pane shows its dashboard at
///   once rather than after `refresh_ms`;
/// * the timer is armed only if the run is NOT already terminal (`:142-145`) — upstream's
///   `timer.unref?.()` says the same thing in node's vocabulary: never hold the process open just
///   to re-render a settled run;
/// * a control line renders again straight after its reply becomes the `notice` (`:137-140`), and
///   a failed control renders `Control error: {message}` rather than ending the pane (`:138`);
/// * reaching a terminal state clears the timer (`:131-134`) while stdin stays readable, because
///   `status` on a settled run is still a legitimate thing to type.
///
/// The process ends when stdin reaches EOF — node's own exit condition once the readline interface
/// closes and the only remaining handle is an `unref`'d timer.
///
/// # Errors
///
/// [`InspectorRunError::Argv`] for an argv the parser refuses (nothing is rendered), or
/// [`InspectorRunError::Output`] if the pane's stdout cannot be written.
pub async fn run_inspector<R, W>(
    argv: &[String],
    stdin: R,
    mut stdout: W,
) -> Result<(), InspectorRunError>
where
    R: AsyncBufRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    let options = parse_args(argv)?;
    let mut notice = String::new();

    let mut lines = stdin.lines();
    let mut terminal = render(&options, &notice, &mut stdout).await?;
    let mut ticker = tokio::time::interval_at(
        tokio::time::Instant::now() + std::time::Duration::from_millis(options.refresh_ms),
        std::time::Duration::from_millis(options.refresh_ms),
    );
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        // `Lines::next_line` is cancellation-safe — the partially-read line lives in `lines`'s own
        // buffer, not in the future — so a refresh tick firing mid-line cannot eat a control
        // command. `AsyncBufReadExt::read_line`, which does hold the partial read in the future,
        // would be wrong here for exactly that reason.
        tokio::select! {
            line = lines.next_line() => {
                let Ok(Some(line)) = line else { break };
                notice = match submit_inspector_control(&options, &line).await {
                    Ok(reply) => reply,
                    Err(error) => format!("Control error: {error}"),
                };
                terminal = render(&options, &notice, &mut stdout).await?;
            }
            _ = ticker.tick(), if !terminal => {
                terminal = render(&options, &notice, &mut stdout).await?;
            }
        }
    }
    Ok(())
}

/// One screen — pi's `render` closure (`inspector-runner.ts:124-135`). Returns whether the run has
/// reached a state the timer should stop for.
///
/// The degraded branch (`:127`) is byte-for-byte upstream's shape: clear, a bare header, a blank
/// line, and the one sentence. It is reached when `status.json` is absent, unreadable, or names a
/// different run — the case where a run directory was swept out from under a still-open pane.
async fn render<W>(
    options: &RunnerOptions,
    notice: &str,
    stdout: &mut W,
) -> Result<bool, InspectorRunError>
where
    W: AsyncWrite + Unpin + Send,
{
    let Some(status) = read_run_status(options).await else {
        let text = format!(
            "{CLEAR_SCREEN}{INSPECTOR_HEADER_BARE}\n\nLifecycle status for {} is unavailable.\n",
            options.run_id
        );
        stdout.write_all(text.as_bytes()).await?;
        stdout.flush().await?;
        // Upstream returns early WITHOUT clearing the timer (`:128`), so a pane whose run
        // directory is briefly unreadable keeps retrying instead of freezing on the message.
        return Ok(false);
    };
    let mission = read_mission(options.mission_path.as_deref()).await;
    let dashboard = format_inspector_dashboard(&DashboardInput {
        status: &status,
        async_dir: &options.async_dir,
        index: options.index,
        mission: mission.as_ref(),
        allow_steer: options.allow_steer,
        allow_stop: options.allow_stop,
        session_roots: &options.session_roots,
    });
    let suffix = if notice.is_empty() {
        String::new()
    } else {
        format!("\n\n{notice}")
    };
    stdout
        .write_all(format!("{CLEAR_SCREEN}{dashboard}{suffix}\n> ").as_bytes())
        .await?;
    stdout.flush().await?;
    Ok(is_terminal_for_inspector(status.state))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    use crate::background::atomic::write_atomic_json;
    use crate::background::control::{SteerRequest, steer_requests_dir, stop_requests_dir};
    use crate::background::{StepState, StepStatus};
    use crate::inspectors::types::{INSPECTOR_HEADER_PREFIX, INSPECTOR_SUBCOMMAND};

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_string()).collect()
    }

    /// A real on-disk async run at `<root>/async/<run_id>`, in `state` with `steps`.
    async fn seed_run(
        root: &Path,
        run_id: &str,
        state: RunState,
        mode: RunMode,
        steps: Vec<StepStatus>,
    ) -> PathBuf {
        let async_root = root.join("async");
        let run = RunId::from_token(run_id.to_string());
        let paths = RunPaths::for_run(&async_root, &root.join("results"), &run);
        tokio::fs::create_dir_all(&paths.run_dir).await.unwrap();
        let mut status = RunStatus::queued(run, mode, Some(4242));
        status.state = state;
        status.steps = steps;
        write_atomic_json(&paths.status, &status).await.unwrap();
        paths.run_dir
    }

    fn options_for(async_dir: &Path, run_id: &str) -> RunnerOptions {
        RunnerOptions {
            async_dir: async_dir.to_path_buf(),
            run_id: run_id.to_string(),
            index: None,
            mission_path: None,
            refresh_ms: DEFAULT_REFRESH_MS,
            allow_steer: true,
            allow_stop: true,
            session_roots: Vec::new(),
        }
    }

    async fn only_steer_request(async_dir: &Path) -> SteerRequest {
        let mut entries = tokio::fs::read_dir(steer_requests_dir(async_dir))
            .await
            .expect("the steer-requests directory must exist");
        let mut found: Vec<SteerRequest> = Vec::new();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            let bytes = tokio::fs::read(entry.path()).await.unwrap();
            found.push(serde_json::from_slice(&bytes).unwrap());
        }
        assert_eq!(found.len(), 1, "expected exactly one steer request");
        found.remove(0)
    }

    // =============================================================================================
    // T-ARGV-1 — the argv parser matches `parseArgs` exactly
    // =============================================================================================

    /// GUT: change `if !key.starts_with("--")` in [`parse_args`]'s match guard to `true`, or drop
    /// the `value` arm — the odd-arity and bare-token rows go green-to-red immediately.
    #[test]
    fn the_runner_argv_parser_matches_parse_args_exactly() {
        // The happy path, with every flag `launch_for` emits.
        let parsed = parse_args(&argv(&[
            "--async-dir",
            "/runs/r1",
            "--run-id",
            "r1",
            "--allow-steer",
            "true",
            "--allow-stop",
            "false",
            "--session-roots",
            "WyIvYSIsIi9iIl0=",
            "--index",
            "2",
            "--mission-path",
            "/m/mission.json",
        ]))
        .expect("the full launch argv parses");
        assert_eq!(parsed.run_id, "r1");
        assert_eq!(parsed.index, Some(2));
        assert!(parsed.allow_steer);
        assert!(!parsed.allow_stop);
        assert_eq!(parsed.refresh_ms, DEFAULT_REFRESH_MS, "`:67`'s default");
        assert_eq!(
            parsed.session_roots,
            vec![PathBuf::from("/a"), PathBuf::from("/b")]
        );
        assert_eq!(
            parsed.mission_path,
            Some(PathBuf::from("/m/mission.json")),
            "an absolute --mission-path survives `path.resolve` unchanged"
        );

        // `:76-77` — a literal `"false"` and NOTHING else turns a flag off.
        for raw in ["FALSE", "0", "no", "False", ""] {
            let parsed = parse_args(&argv(&[
                "--async-dir",
                "/runs/r1",
                "--run-id",
                "r1",
                "--allow-steer",
                raw,
            ]))
            .expect("parses");
            assert!(
                parsed.allow_steer,
                "--allow-steer {raw:?} is NOT the literal \"false\", so steering stays on"
            );
        }

        // `Map.set` — a repeated key takes the LAST value.
        let parsed = parse_args(&argv(&[
            "--async-dir",
            "/runs/r1",
            "--run-id",
            "first",
            "--run-id",
            "last",
        ]))
        .expect("parses");
        assert_eq!(parsed.run_id, "last");

        // The refusal table, each row carrying upstream's own sentence.
        let cases: Vec<(Vec<String>, &str)> = vec![
            (
                argv(&["--async-dir", "/runs/r1", "--run-id"]),
                "Invalid inspector argument '--run-id'.",
            ),
            (
                argv(&["bare", "value"]),
                "Invalid inspector argument 'bare'.",
            ),
            (
                argv(&["--run-id", "r1"]),
                "Inspector requires --async-dir and --run-id.",
            ),
            (
                argv(&["--async-dir", "/runs/r1"]),
                "Inspector requires --async-dir and --run-id.",
            ),
            (
                argv(&["--async-dir", "", "--run-id", "r1"]),
                "Inspector requires --async-dir and --run-id.",
            ),
            (
                argv(&["--async-dir", "/runs/r1", "--run-id", ""]),
                "Inspector requires --async-dir and --run-id.",
            ),
            (
                argv(&["--async-dir", "/runs/r1", "--run-id", "r1", "--index", "-1"]),
                "--index must be a non-negative integer.",
            ),
            (
                argv(&[
                    "--async-dir",
                    "/runs/r1",
                    "--run-id",
                    "r1",
                    "--index",
                    "1.5",
                ]),
                "--index must be a non-negative integer.",
            ),
            (
                argv(&[
                    "--async-dir",
                    "/runs/r1",
                    "--run-id",
                    "r1",
                    "--index",
                    "two",
                ]),
                "--index must be a non-negative integer.",
            ),
            (
                argv(&[
                    "--async-dir",
                    "/runs/r1",
                    "--run-id",
                    "r1",
                    "--refresh-ms",
                    "249",
                ]),
                "--refresh-ms must be an integer >= 250.",
            ),
            (
                argv(&[
                    "--async-dir",
                    "/runs/r1",
                    "--run-id",
                    "r1",
                    "--refresh-ms",
                    "250.5",
                ]),
                "--refresh-ms must be an integer >= 250.",
            ),
            (
                argv(&[
                    "--async-dir",
                    "/runs/r1",
                    "--run-id",
                    "r1",
                    "--session-roots",
                    "not-base64!!",
                ]),
                "--session-roots must be a base64-encoded JSON array of strings.",
            ),
        ];
        for (input, expected) in cases {
            let error = parse_args(&input).expect_err("must refuse");
            assert_eq!(error.to_string(), expected, "for argv {input:?}");
        }

        // `Number("")` is 0 and `Number.isInteger(0)` is true — a real upstream edge, ported.
        let parsed = parse_args(&argv(&[
            "--async-dir",
            "/runs/r1",
            "--run-id",
            "r1",
            "--index",
            "",
        ]))
        .expect("an empty --index is Number(\"\") === 0 upstream");
        assert_eq!(parsed.index, Some(0));

        // `--refresh-ms 250` is the floor, inclusive.
        let parsed = parse_args(&argv(&[
            "--async-dir",
            "/runs/r1",
            "--run-id",
            "r1",
            "--refresh-ms",
            "250",
        ]))
        .expect("the floor itself is accepted");
        assert_eq!(parsed.refresh_ms, 250);
    }

    // =============================================================================================
    // T-DASH-1 — the dashboard header, mirror sentence and the four controls cases
    // =============================================================================================

    /// GUT: replace [`format_inspector_dashboard`]'s `controls` construction with a fixed
    /// `vec!["type guidance", "steer <message>", "stop", "status"]` — three of the four cases
    /// below go red. GUT the header line to drop [`crate::inspectors::types::INSPECTOR_HEADER_PREFIX`] and the first
    /// assertion goes red, which is the same string `herdr pane wait-output` matches on.
    #[tokio::test]
    async fn the_dashboard_header_and_controls_line_are_upstreams() {
        let tmp = tempfile::tempdir().unwrap();
        let async_dir = seed_run(
            tmp.path(),
            "run-dash",
            RunState::Running,
            RunMode::Single,
            vec![StepStatus::pending("worker")],
        )
        .await;
        let status_path = crate::background::RunDir::for_existing(&async_dir).status();
        let bytes = tokio::fs::read(&status_path).await.unwrap();
        let status: RunStatus = serde_json::from_slice(&bytes).unwrap();

        let render_with = |allow_steer: bool, allow_stop: bool, index: Option<usize>, mode| {
            let mut status = status.clone();
            status.mode = mode;
            format_inspector_dashboard(&DashboardInput {
                status: &status,
                async_dir: &async_dir,
                index,
                mission: None,
                allow_steer,
                allow_stop,
                session_roots: &[],
            })
        };

        let full = render_with(true, true, None, RunMode::Single);
        assert!(
            full.starts_with(&format!("{INSPECTOR_HEADER_PREFIX}run-dash\n")),
            "the header is the string the herdr backend waits for: {full}"
        );
        // **The join, asserted against the other half's own function.** The herdr backend confirms
        // a pane started the inspector with `pane wait-output --match <this>`; if the marker is
        // not a prefix of what this renderer actually prints, every `inspector.open` waits out
        // `INSPECTOR_READY_TIMEOUT_MS`, closes the pane it just opened and writes no binding — and
        // no test in `inspectors/herdr/` notices, because each one scripts the `wait-output`
        // answer. Both sides call `types::inspector_header_line`, and this is what says so.
        assert!(
            full.starts_with(&format!(
                "{}\n",
                crate::inspectors::herdr::actions::inspector_ready_marker("run-dash")
            )),
            "the herdr backend's wait marker is not this dashboard's first line: {full}"
        );
        assert!(
            full.contains(
                "This inspector mirrors lifecycle artifacts; closing it does not stop the run."
            ),
            "the mirror sentence (`inspector-runner.ts:34`) is missing"
        );
        assert!(
            full.contains("Controls: type guidance | steer <message> | stop | status"),
            "{full}"
        );
        assert!(
            full.contains(
                "Supervisor replies remain in the parent cyrup session \
                 (subagent_supervisor/intercom); this inspector is read-only."
            ),
            "{full}"
        );
        assert!(
            full.contains("Run: run-dash"),
            "the body must be `format_async_run_transcript`'s, not a re-implementation: {full}"
        );

        // V17: on a `single` run, `allow_stop == false` drops ONLY `stop`.
        assert!(
            render_with(true, false, None, RunMode::Single)
                .contains("Controls: type guidance | steer <message> | status"),
        );
        // `allow_steer == false` drops BOTH `type guidance` and `steer <message>`.
        assert!(
            render_with(false, true, None, RunMode::Single).contains("Controls: stop | status"),
        );
        assert!(render_with(false, false, None, RunMode::Single).contains("Controls: status"));
        // An AGGREGATE pane (no `--index`, mode != single) does not accept plain guidance.
        assert!(
            render_with(true, true, None, RunMode::Chain)
                .contains("Controls: steer <message> | stop | status"),
        );
        // …until it is scoped to a child.
        assert!(
            render_with(true, true, Some(0), RunMode::Chain)
                .contains("Controls: type guidance | steer <message> | stop | status"),
        );
    }

    // =============================================================================================
    // T-CTL-1 — steer and stop land REAL requests on the control channel
    // =============================================================================================

    /// GUT: delete the `request_async_steer(...)` call in [`queue_inspector_steer`] and return the
    /// receipt alone — the reply still looks right and this test still fails, because it asserts
    /// the FILE. Same for `request_async_stop`.
    #[tokio::test]
    async fn steer_and_stop_from_stdin_land_real_requests_on_the_control_channel() {
        let tmp = tempfile::tempdir().unwrap();
        let async_dir = seed_run(
            tmp.path(),
            "run-ctl",
            RunState::Running,
            RunMode::Single,
            vec![StepStatus::pending("worker")],
        )
        .await;
        let options = options_for(&async_dir, "run-ctl");

        let reply = submit_inspector_control(&options, "steer hello there")
            .await
            .expect("a steer on a running single-mode run is accepted");
        assert!(
            reply.starts_with("Steering queued for run run-ctl.\n\nMessage sent:\n```text\n"),
            "the receipt is `steeringReceipt`'s shape: {reply}"
        );
        assert!(reply.contains("hello there"), "{reply}");

        let request = only_steer_request(&async_dir).await;
        assert_eq!(request.message, "hello there");
        assert_eq!(request.source.as_deref(), Some(INSPECTOR_CONTROL_SOURCE));
        assert_eq!(
            request.target_index,
            Some(0),
            "`single` mode addresses child 0 (`inspector-runner.ts:89`)"
        );

        let reply = submit_inspector_control(&options, "stop")
            .await
            .expect("stop is accepted while the run is live");
        assert_eq!(reply, "Stop requested for run run-ctl.");
        let mut stop_entries = tokio::fs::read_dir(stop_requests_dir(&async_dir))
            .await
            .expect("the stop-requests directory must exist");
        let mut stops = 0usize;
        while let Some(entry) = stop_entries.next_entry().await.unwrap() {
            let bytes = tokio::fs::read(entry.path()).await.unwrap();
            let request: crate::background::control::StopRequest =
                serde_json::from_slice(&bytes).unwrap();
            assert_eq!(request.source, INSPECTOR_CONTROL_SOURCE);
            stops += 1;
        }
        assert_eq!(stops, 1, "exactly one stop request must have been written");

        // `status` and an empty line are non-errors that touch nothing.
        assert_eq!(
            submit_inspector_control(&options, "   ").await.unwrap(),
            "Status refreshed."
        );
        assert_eq!(
            submit_inspector_control(&options, "status").await.unwrap(),
            "Status refreshed."
        );
    }

    /// An aggregate pane with no `--index` steers EVERY running child, which cyrup spells as
    /// `target_index: None` — the `[CYRUP-DELTA]` on [`queue_inspector_steer`].
    ///
    /// GUT: make [`queue_inspector_steer`] fall back to `Some(0)` when `options.index` is `None`
    /// — the `None` assertion goes red, and the production behaviour silently narrows a fan-out
    /// steer to the first child.
    #[tokio::test]
    async fn an_aggregate_steer_targets_every_running_child() {
        let tmp = tempfile::tempdir().unwrap();
        let mut running = StepStatus::pending("worker");
        running.status = StepState::Running;
        let async_dir = seed_run(
            tmp.path(),
            "run-agg",
            RunState::Running,
            RunMode::Chain,
            vec![StepStatus::pending("first"), running],
        )
        .await;
        let options = options_for(&async_dir, "run-agg");

        submit_inspector_control(&options, "steer everyone")
            .await
            .expect("an aggregate steer with a running child is accepted");
        let request = only_steer_request(&async_dir).await;
        assert_eq!(
            request.target_index, None,
            "`None` is cyrup's 'every running child, resolved at drain time'"
        );
    }

    /// T-CTL-3 — the `:90` guard the plural-to-`None` collapse must not take with it.
    ///
    /// GUT: delete the `running_children == 0` check in [`queue_inspector_steer`] — the steer
    /// silently succeeds into an empty fan-out and this is the only test that notices.
    #[tokio::test]
    async fn an_aggregate_steer_with_no_running_child_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let async_dir = seed_run(
            tmp.path(),
            "run-idle",
            RunState::Running,
            RunMode::Chain,
            vec![StepStatus::pending("first"), StepStatus::pending("second")],
        )
        .await;
        let options = options_for(&async_dir, "run-idle");

        let error = submit_inspector_control(&options, "steer nobody")
            .await
            .expect_err("no running child means no fan-out target");
        assert_eq!(
            error.to_string(),
            "No running child is available to steer. Open a child-specific inspector for a \
             pending child."
        );
        assert!(
            !steer_requests_dir(&async_dir).exists(),
            "the refusal must precede the write"
        );
    }

    // =============================================================================================
    // T-CTL-2 — every control refusal, in upstream's words
    // =============================================================================================

    /// GUT: swap [`is_terminal_for_inspector`] for `RunState::is_terminal` — the PAUSED rows go
    /// red, because this crate's own predicate calls a paused run non-terminal while upstream's
    /// inspector calls it terminal.
    #[tokio::test]
    async fn the_control_refusals_are_upstreams() {
        let tmp = tempfile::tempdir().unwrap();

        // A live single-mode run, for the refusals that are not about lifecycle state.
        let live = seed_run(
            tmp.path().join("live").as_path(),
            "run-live",
            RunState::Running,
            RunMode::Single,
            vec![StepStatus::pending("worker")],
        )
        .await;
        let options = options_for(&live, "run-live");

        assert_eq!(
            submit_inspector_control(&options, "steer   ")
                .await
                .expect_err("an empty steer message")
                .to_string(),
            "steer requires a message."
        );
        assert_eq!(
            submit_inspector_control(&options, "reply hello")
                .await
                .expect_err("replies are the parent session's")
                .to_string(),
            "Supervisor replies are owned by the parent cyrup session; use \
             subagent_supervisor/intercom there."
        );

        // Plain guidance on an aggregate pane.
        let aggregate = seed_run(
            tmp.path().join("agg").as_path(),
            "run-chain",
            RunState::Running,
            RunMode::Chain,
            vec![StepStatus::pending("worker")],
        )
        .await;
        let aggregate_options = options_for(&aggregate, "run-chain");
        assert_eq!(
            submit_inspector_control(&aggregate_options, "just do it")
                .await
                .expect_err("plain guidance needs a child-specific pane")
                .to_string(),
            "Plain guidance requires a child-specific inspector. Use steer <message> to target \
             all running children from the aggregate inspector."
        );

        // Authority refusals.
        let mut no_steer = options.clone();
        no_steer.allow_steer = false;
        assert_eq!(
            submit_inspector_control(&no_steer, "steer nope")
                .await
                .expect_err("policy says no")
                .to_string(),
            "Authority policy does not allow steer from this inspector."
        );
        let mut no_stop = options.clone();
        no_stop.allow_stop = false;
        assert_eq!(
            submit_inspector_control(&no_stop, "stop")
                .await
                .expect_err("policy says no")
                .to_string(),
            "Authority policy does not allow stop from this inspector."
        );

        // A settled run, and a PAUSED one — upstream calls both terminal for this purpose.
        for (state, word) in [
            (RunState::Complete, "complete"),
            (RunState::Paused, "paused"),
            (RunState::Stopped, "stopped"),
        ] {
            let dir = seed_run(
                tmp.path().join(format!("settled-{word}")).as_path(),
                "run-settled",
                state,
                RunMode::Single,
                vec![StepStatus::pending("worker")],
            )
            .await;
            let settled = options_for(&dir, "run-settled");
            assert_eq!(
                submit_inspector_control(&settled, "steer late")
                    .await
                    .expect_err("a settled run cannot be steered")
                    .to_string(),
                format!("Run 'run-settled' is {word} and cannot be steered.")
            );
            assert_eq!(
                submit_inspector_control(&settled, "stop")
                    .await
                    .expect_err("a settled run cannot be stopped")
                    .to_string(),
                format!("Run 'run-settled' is {word} and cannot be stopped.")
            );
        }

        // No status file at all, and a status file naming a DIFFERENT run.
        let empty = tmp.path().join("empty");
        tokio::fs::create_dir_all(&empty).await.unwrap();
        let orphan = options_for(&empty, "run-missing");
        assert_eq!(
            submit_inspector_control(&orphan, "stop")
                .await
                .expect_err("no status.json")
                .to_string(),
            "Lifecycle status for run 'run-missing' is unavailable."
        );
        let mismatched = options_for(&live, "some-other-run");
        assert_eq!(
            submit_inspector_control(&mismatched, "stop")
                .await
                .expect_err("status.json names another run")
                .to_string(),
            "Lifecycle status for run 'some-other-run' is unavailable."
        );
    }

    // =============================================================================================
    // steering_receipt — the fence has to WIDEN
    // =============================================================================================

    /// GUT: replace the `longest_backtick_run(...).max(2) + 1` fence width with a constant 3 — the
    /// third row goes red, and in production a message containing a fenced code block would break
    /// out of the receipt and render as live markup in the pane.
    #[test]
    fn a_steering_receipt_fences_wider_than_the_message_it_quotes() {
        assert_eq!(
            steering_receipt("hello", "Queued."),
            "Queued.\n\nMessage sent:\n```text\nhello\n```"
        );
        assert_eq!(
            steering_receipt("a `b` c", "Queued."),
            "Queued.\n\nMessage sent:\n```text\na `b` c\n```",
            "runs shorter than three backticks are not candidates (`/`{{3,}}/`)"
        );
        assert_eq!(
            steering_receipt("```rust\nx\n```", "Queued."),
            "Queued.\n\nMessage sent:\n````text\n```rust x ```\n````",
            "sanitisation folds the newlines, and the fence widens to four"
        );
    }

    /// GUT: drop the `redact_secret_values` call in [`steering_message_preview`] — the credential
    /// reaches the pane, and this is the only test that sees it.
    #[test]
    fn a_steering_preview_is_redacted_before_it_is_truncated() {
        let preview = steering_message_preview("use Bearer sk-abcdefghijklmnop now");
        assert!(
            !preview.contains("abcdefghijklmnop"),
            "the credential survived: {preview}"
        );
        assert!(preview.contains("[redacted]"), "{preview}");

        let long = "x".repeat(400);
        let preview = steering_message_preview(&long);
        assert_eq!(
            preview.chars().count(),
            STEERING_MESSAGE_PREVIEW_LIMIT,
            "160 units including the ellipsis"
        );
        assert!(preview.ends_with("..."));
    }

    // =============================================================================================
    // run_inspector — the whole loop, with an injected stdin and stdout
    // =============================================================================================

    /// The library half of T-RUN-1: the same code the `__subagent-inspector` subcommand runs,
    /// driven with an in-memory stdin and stdout.
    ///
    /// GUT: make [`run_inspector`]'s `line` arm ignore its reply (`notice = String::new()`) — the
    /// receipt assertion goes red. GUT the initial `render` call and the header assertion goes red.
    #[tokio::test]
    async fn run_inspector_renders_then_lands_a_steer_from_its_injected_stdin() {
        let tmp = tempfile::tempdir().unwrap();
        let async_dir = seed_run(
            tmp.path(),
            "run-loop",
            RunState::Running,
            RunMode::Single,
            vec![StepStatus::pending("worker")],
        )
        .await;

        let argv = argv(&[
            "--async-dir",
            &async_dir.to_string_lossy(),
            "--run-id",
            "run-loop",
            "--allow-steer",
            "true",
            "--allow-stop",
            "true",
            "--refresh-ms",
            "250",
        ]);
        let stdin = &b"steer from stdin\n"[..];
        let mut stdout: Vec<u8> = Vec::new();
        run_inspector(&argv, tokio::io::BufReader::new(stdin), &mut stdout)
            .await
            .expect("the loop runs to stdin EOF and exits cleanly");

        let rendered = String::from_utf8(stdout).expect("the dashboard is UTF-8");
        assert!(
            rendered.contains(CLEAR_SCREEN),
            "every render clears the screen first"
        );
        assert!(rendered.contains(&format!("{INSPECTOR_HEADER_PREFIX}run-loop")));
        assert!(
            rendered.contains("Steering queued for run run-loop."),
            "the control reply must become the notice on the next render: {rendered}"
        );
        assert!(
            rendered.ends_with("\n> "),
            "the prompt is upstream's `:130`"
        );

        let request = only_steer_request(&async_dir).await;
        assert_eq!(request.message, "from stdin");
        assert_eq!(request.source.as_deref(), Some(INSPECTOR_CONTROL_SOURCE));
    }

    /// A control line that is refused renders `Control error: …` and keeps the pane alive
    /// (`inspector-runner.ts:138`).
    ///
    /// GUT: change the `Err` arm of [`run_inspector`]'s control match to `return Err(...)` — the
    /// pane dies on the first mistyped command and this goes red.
    #[tokio::test]
    async fn a_refused_control_line_becomes_a_notice_not_an_exit() {
        let tmp = tempfile::tempdir().unwrap();
        let async_dir = seed_run(
            tmp.path(),
            "run-notice",
            RunState::Running,
            RunMode::Single,
            vec![StepStatus::pending("worker")],
        )
        .await;
        let argv = argv(&[
            "--async-dir",
            &async_dir.to_string_lossy(),
            "--run-id",
            "run-notice",
        ]);
        let stdin = &b"reply nope\nstatus\n"[..];
        let mut stdout: Vec<u8> = Vec::new();
        run_inspector(&argv, tokio::io::BufReader::new(stdin), &mut stdout)
            .await
            .expect("a refused control does not end the pane");
        let rendered = String::from_utf8(stdout).unwrap();
        assert!(
            rendered.contains(
                "Control error: Supervisor replies are owned by the parent cyrup session;"
            ),
            "{rendered}"
        );
        assert!(
            rendered.ends_with("Status refreshed.\n> "),
            "the later `status` line replaced the notice: {rendered}"
        );
    }

    /// A run whose `status.json` is gone renders the degraded screen rather than dying.
    ///
    /// GUT: make [`render`]'s `None` arm return the error instead — the pane exits non-zero the
    /// moment a retention sweep removes the run, and this goes red.
    #[tokio::test]
    async fn a_missing_status_renders_upstreams_unavailable_screen() {
        let tmp = tempfile::tempdir().unwrap();
        let async_dir = tmp.path().join("async").join("run-gone");
        tokio::fs::create_dir_all(&async_dir).await.unwrap();
        let argv = argv(&[
            "--async-dir",
            &async_dir.to_string_lossy(),
            "--run-id",
            "run-gone",
        ]);
        let stdin: &[u8] = &[];
        let mut stdout: Vec<u8> = Vec::new();
        run_inspector(&argv, tokio::io::BufReader::new(stdin), &mut stdout)
            .await
            .expect("a swept run still renders");
        let rendered = String::from_utf8(stdout).unwrap();
        assert_eq!(
            rendered,
            format!(
                "{CLEAR_SCREEN}{INSPECTOR_HEADER_BARE}\n\nLifecycle status for run-gone is \
                 unavailable.\n"
            )
        );
    }

    /// The refresh timer is armed off ONE predicate, and that predicate is upstream's, not this
    /// crate's. `render` returns it, `run_inspector` arms the ticker on it, and a `Paused` run is
    /// the case where the two predicates disagree.
    ///
    /// GUT: swap [`is_terminal_for_inspector`]'s body for `state.is_terminal()` — the `Paused` row
    /// goes red, and in production a paused run's pane repaints forever while accepting steers the
    /// runner has no live child to deliver.
    #[tokio::test]
    async fn the_refresh_predicate_is_upstreams_not_run_state_is_terminal() {
        assert!(
            !RunState::Paused.is_terminal(),
            "this crate calls a paused run NON-terminal — that is the whole point of the delta"
        );
        assert!(
            is_terminal_for_inspector(RunState::Paused),
            "upstream's inspector predicate is `state !== \"queued\" && state !== \"running\"`"
        );

        let tmp = tempfile::tempdir().unwrap();
        for (state, expected) in [
            (RunState::Queued, false),
            (RunState::Running, false),
            (RunState::Paused, true),
            (RunState::Complete, true),
            (RunState::Failed, true),
            (RunState::Stopped, true),
        ] {
            assert_eq!(is_terminal_for_inspector(state), expected, "for {state:?}");
            let dir = seed_run(
                tmp.path().join(format!("{state:?}")).as_path(),
                "run-pred",
                state,
                RunMode::Single,
                vec![StepStatus::pending("worker")],
            )
            .await;
            let options = options_for(&dir, "run-pred");
            let mut stdout: Vec<u8> = Vec::new();
            let terminal = render(&options, "", &mut stdout)
                .await
                .expect("the dashboard renders");
            assert_eq!(
                terminal, expected,
                "`render` must report the timer verdict for {state:?}"
            );
            assert!(
                String::from_utf8(stdout)
                    .unwrap()
                    .contains(&format!("{INSPECTOR_HEADER_PREFIX}run-pred")),
                "every state still renders a dashboard"
            );
        }
    }

    /// The token the launch string embeds and the token `crates/cyrup` recognises must be the
    /// same bytes. Two independent literals, one asserted equality — the convention
    /// `background/spawn_detached.rs:82-85` states.
    #[test]
    fn the_subcommand_token_is_the_reserved_one() {
        assert_eq!(INSPECTOR_SUBCOMMAND, "__subagent-inspector");
        assert!(
            INSPECTOR_SUBCOMMAND.starts_with("__"),
            "an internal token must be `__`-prefixed so it cannot collide with a user verb"
        );
    }
}
