//! The `inspector.*` dispatcher and everything the four verbs need before a backend is consulted
//! (pi `src/inspectors/actions.ts`, 148 lines @v0.68.0).
//!
//! Five pieces, in upstream's own order: [`trusted_dir`] (`actions.ts:34`), [`resolve_target`]
//! (`:49`), [`mission_for`] (`:79`), [`launch_for`] (`:90`) and [`handle_inspector_action`]
//! (`:120`).
//!
//! # This module is the half that works with NOTHING installed
//!
//! `inspector.command` returns its answer at `actions.ts:130`, one line BEFORE `deps.plugins` is
//! read at `:131`. On a box with no herdr and no ghostty it is the one verb of the seven that does
//! real work, and it is not an error — it hands the operator the exact command to run the
//! inspector standalone. The other three degrade to upstream's own sentences, and two of those
//! three are deliberately NOT errors:
//!
//! | verb | with no backend | error? |
//! |---|---|---|
//! | `inspector.command` | the full launch string, including the base64 `--session-roots` | no |
//! | `inspector.open` | [`NO_INSPECTOR_PLUGIN_AVAILABLE`] | **yes** |
//! | `inspector.status` | `No inspector plugin owns this binding for async run {runId}.` | no |
//! | `inspector.close` | the same sentence | no |
//!
//! The `isError`-false pair is upstream's `result(text)` with the second argument OMITTED
//! (`actions.ts:139`): a status query with no inspector open is a normal answer, not a failure.
//! See `inspectors/types.rs`'s module doc for how `isError` maps onto cyrup's `Result`.
//!
//! **Nothing touches disk on the refusal path.** `launchFor` is called INSIDE the plugin loop
//! (`actions.ts:134`), so with zero available plugins the mission lookup never runs, no binding is
//! written, and `<asyncDir>/inspectors/` is not created. Both plugins' `available()` are pure
//! env/platform reads (`herdr/plugin.ts:13-14`, `ghostty/plugin.ts:13`) — no binary probe — so the
//! refusal is instant and cannot hang.

use std::path::{Path, PathBuf};

use cyrup_core::{CancelToken, Content, ToolError, ToolResult};

use super::plugins::InspectorPlugin;
use super::session_roots_codec::encode_session_roots;
use super::shell_command::{format_shell_command, host_platform};
use super::types::{
    InspectorAction, InspectorContext, InspectorLaunch, InspectorParams, InspectorTarget,
};
use crate::background::{RunDir, RunStatus, resolve_async_run_id};
use crate::identity::SessionId;
use crate::missions::types::MissionStoreConfig;
use crate::registration::authority::{
    AuthorityAction, AuthorityDecision, AuthorityPolicyConfig, resolve_authority_decision,
};

/// pi `actions.ts:136`, verbatim — the only sentence `inspector.open` can answer with when no
/// backend is available, and the most-read string in this subsystem on a stock Linux box.
pub const NO_INSPECTOR_PLUGIN_AVAILABLE: &str = "No inspector plugin is available. Start a supported inspector host, or use inspector.command for a standalone command.";

/// pi `INSPECTOR_LEASE_WAIT_MS` (`actions.ts:21-22` @ad11b7ab, SUBA-201):
///
/// ```text
/// // Covers a provider's slowest normal open (a 15 s split or launch plus its follow-up commands).
/// const INSPECTOR_LEASE_WAIT_MS = 30_000;
/// ```
pub const INSPECTOR_LEASE_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

// =================================================================================================
// Inputs
// =================================================================================================

/// The target-selection half of pi's `InspectorParams` (`inspectors/types.ts:7-13`).
///
/// **Why this is not [`InspectorParams`].** The frozen contract's [`InspectorParams`] carries only
/// what a PLUGIN reads (`pane_id`, `focus`); upstream's interface additionally carries the four
/// fields only the DISPATCHER reads — `id`, `runId`, `dir`, `index` — which no plugin ever touches.
/// Splitting them keeps the plugin seam minimal without inventing a second spelling of a shared
/// type: [`Self::plugin_params`] is the one conversion, and it is the only place the two meet.
///
/// `index` is `i64`, not `usize`, deliberately: upstream's range refusal reports the value the
/// caller sent, including a negative one (`actions.ts:74`), and narrowing at the edge would make
/// `-1` indistinguishable from a missing field.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InspectorRequest {
    /// pi `params.id`.
    pub id: Option<String>,
    /// pi `params.runId` — consulted only when `id` is absent (`actions.ts:50`).
    pub run_id: Option<String>,
    /// pi `params.dir`: an explicit async run directory, which bypasses id resolution entirely.
    pub dir: Option<PathBuf>,
    /// pi `params.index`: which child of the run to inspect.
    pub index: Option<i64>,
    /// pi `params.focus`: whether the host should focus the pane it opens.
    pub focus: Option<bool>,
    /// An explicit pane to reuse, for hosts that accept one.
    pub pane_id: Option<String>,
}

impl InspectorRequest {
    /// The subset a plugin is handed (`actions.ts:134`'s third argument).
    #[must_use]
    pub fn plugin_params(&self) -> InspectorParams {
        InspectorParams {
            pane_id: self.pane_id.clone(),
            focus: self.focus,
        }
    }
}

/// One background run this process is currently tracking — pi's `state.asyncJobs` /
/// `state.fleetJobs` entries, flattened to the two fields `inspectors/` actually reads off them
/// (`actions.ts:38` for the directory, `:94` for the session root).
///
/// Merged into ONE list because upstream spreads both maps into one array before searching
/// (`actions.ts:38`, `:93`) and never distinguishes them afterwards. Order is significant only for
/// [`launch_for`]'s session-root lookup, which takes the first entry matching the run id — so a
/// caller assembling this list puts `asyncJobs` ahead of `fleetJobs`, as upstream's `??` does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveInspectorJob {
    /// The run id this job tracks.
    pub run_id: String,
    /// The run's async directory, as this process recorded it.
    pub async_dir: PathBuf,
    /// The session root the run's transcript lives under, when the job knows one.
    pub session_root: Option<PathBuf>,
}

/// pi `InspectorDispatcherDeps` (`actions.ts:106-119`), re-expressed against cyrup's own
/// resolvers.
///
/// Upstream threads a whole `SubagentState` plus module-global `DIRS`; cyrup has neither, so the
/// three things upstream reads off them are named explicitly: [`Self::async_dir_root`] /
/// [`Self::results_dir`] (upstream's `DIRS.async` / `DIRS.results` defaults, which here have no
/// global to fall back to and are therefore required), [`Self::live_jobs`] (the two job maps), and
/// [`Self::current_session`] (which cyrup's `resolve_async_run_id` takes where pi's
/// `resolveSubagentRunId` reads it off `state`).
///
/// `now` is absent: the clock a plugin needs is the plugin's own (the contract's
/// [`InspectorContext`] carries none). Upstream's `deps.signal` is not a field either: it is
/// [`handle_inspector_action`]'s `cancel` parameter, because the token belongs to the CALL (the
/// tool edge's turn), not to the deps a surface assembles once (SUBA-201).
pub struct InspectorDispatcherDeps {
    /// The project root the verb was invoked in — pi `deps.cwd` (`actions.ts:113`).
    pub cwd: PathBuf,
    /// The async run root to resolve ids and containment against (pi `DIRS.async`).
    pub async_dir_root: PathBuf,
    /// The terminal-results dir the id resolver also searches (pi `DIRS.results`).
    pub results_dir: PathBuf,
    /// The session whose runs are visible, or `None` for a host with no identity (which
    /// `resolve_async_run_id` treats permissively, `run_id_resolver.rs:237-239`).
    pub current_session: Option<SessionId>,
    /// Every run this process is tracking, `asyncJobs` first — see [`LiveInspectorJob`].
    pub live_jobs: Vec<LiveInspectorJob>,
    /// The trusted session roots the inspector may read transcripts under — pi
    /// `deps.sessionRoots ?? state.trustedSessionRoots` (`actions.ts:94`), already resolved by the
    /// caller because cyrup computes it from config + the live parent session file
    /// (`extension/executor/paths.rs:196`).
    pub session_roots: Vec<PathBuf>,
    /// The mission-store override, when the call carried one.
    pub missions: Option<MissionStoreConfig>,
    /// The agent-dir override `resolve_mission_store_location` takes; `None` in production.
    pub agent_dir_override: Option<PathBuf>,
    /// The authority policy that decides `--allow-steer` / `--allow-stop`.
    pub authority_policy: Option<AuthorityPolicyConfig>,
    /// The backends to consult, in host-preference order — pi `deps.plugins ?? []`
    /// (`actions.ts:131`). Production passes
    /// [`builtin_inspector_plugins()`](super::plugins::builtin_inspector_plugins); an empty list
    /// is a legitimate configuration and is what drives [`NO_INSPECTOR_PLUGIN_AVAILABLE`].
    pub plugins: Vec<Box<dyn InspectorPlugin>>,
    /// The environment every backend's `available()` gate reads — pi `deps.env ?? process.env`
    /// (`actions.ts:126`), folded onto [`InspectorContext::env`] by [`build_context`].
    ///
    /// Production passes [`process_env`]. A test passes a map, which is the only way to drive the
    /// not-installed path — and the installed one — through this dispatcher rather than through a
    /// backend's own private field.
    pub env: std::collections::BTreeMap<String, String>,
    /// How long `inspector.open` / `inspector.close` wait for another open or close of the same
    /// target — pi `leaseWaitMs` (`actions.ts` @ad11b7ab):
    ///
    /// ```text
    /// /** How long open and close wait for another open or close of the same target. */
    /// leaseWaitMs?: number;
    /// ```
    ///
    /// Production passes [`INSPECTOR_LEASE_WAIT`]; a test passes a short bound to reach the
    /// timeout sentence (SUBA-201).
    pub lease_wait: std::time::Duration,
}

/// The real process environment, as [`InspectorDispatcherDeps::env`] wants it — pi's
/// `process.env` (`actions.ts:126`).
///
/// A copy rather than a live view: upstream reads a live object, cyrup reads `std::env::vars()`
/// once per verb. The difference is unobservable here, because a verb's gate is decided in one
/// synchronous expression and nothing in this process mutates its own environment mid-verb (the
/// crate is `#![forbid(unsafe_code)]`, and `std::env::set_var` is `unsafe` in Rust 2024).
#[must_use]
pub fn process_env() -> std::collections::BTreeMap<String, String> {
    std::env::vars().collect()
}

/// What [`resolve_target`] produces: the contract's [`InspectorTarget`] plus the lifecycle status
/// it was validated against.
///
/// The status rides alongside rather than inside the target because the frozen
/// [`InspectorTarget`] carries no status snapshot, where pi's does
/// (`inspectors/types.ts:19-23`: `{ cwd, state, steps }`). Everything this module needs it for —
/// the child-index range check and the launch cwd — happens here; see this module's
/// `[CONTRACT-GAP]` note on [`build_context`].
#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    /// The resolved run/child.
    pub target: InspectorTarget,
    /// The `status.json` that resolution read, verbatim.
    pub status: RunStatus,
}

/// A mission a run is bound to — pi `missionFor`'s `{ id, path }` (`actions.ts:79`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMission {
    /// The mission id.
    pub id: String,
    /// Its record file.
    pub path: PathBuf,
}

// =================================================================================================
// trustedDir — `actions.ts:29-48`
// =================================================================================================

/// pi `trustedDir` (`actions.ts:34-48`) — may this process launch an inspector against `dir`?
///
/// **Two rungs, and BOTH are load-bearing.**
///
/// 1. **Live-job match** (`:38-44`). Any directory whose realpath equals the `asyncDir` of a run
///    this process is tracking is trusted REGARDLESS of the configured root. Drop this rung and a
///    run launched under a non-default `async_dir_root` becomes uninspectable.
/// 2. **Containment** (`:46`). `root` must exist, and `pathWithin` is applied TWICE — once to the
///    literal pair, once to the realpath'd pair. Drop the realpath leg and a symlink planted
///    inside the root escapes it.
///
/// Ahead of both, upstream refuses a symlink or a non-directory outright (`:36`), and every
/// filesystem error anywhere in the function is `false`, never a throw (`:47`).
#[must_use]
pub fn trusted_dir(dir: &Path, deps: &InspectorDispatcherDeps) -> bool {
    let Ok(link_meta) = std::fs::symlink_metadata(dir) else {
        return false;
    };
    if link_meta.file_type().is_symlink() {
        return false;
    }
    if !std::fs::metadata(dir).is_ok_and(|meta| meta.is_dir()) {
        return false;
    }
    let Ok(real) = std::fs::canonicalize(dir) else {
        return false;
    };
    // Rung 1 — a directory this process is actively tracking.
    if deps
        .live_jobs
        .iter()
        .any(|job| std::fs::canonicalize(&job.async_dir).is_ok_and(|job_real| job_real == real))
    {
        return true;
    }
    // Rung 2 — containment under the configured root, literally AND after realpath.
    let root = &deps.async_dir_root;
    root.exists()
        && crate::paths::path_within(root, dir)
        && std::fs::canonicalize(root)
            .is_ok_and(|real_root| crate::paths::path_within(&real_root, &real))
}

// =================================================================================================
// resolveTarget — `actions.ts:49-78`
// =================================================================================================

async fn read_status(async_dir: &Path) -> Option<RunStatus> {
    let path = RunDir::for_existing(async_dir).status();
    crate::background::control::read_status_file(&path)
        .await
        .ok()
        .flatten()
}

/// pi `resolveTarget` (`actions.ts:49-78`) — which run, and which child of it, this verb is about.
///
/// Upstream returns `InspectorTarget | { error: string }`; here that is `Result<_, String>`, and
/// every one of the eight refusal sentences is upstream's, verbatim, at the same decision point:
///
/// | upstream | when |
/// |---|---|
/// | `Async run directory '{dir}' is outside trusted run roots.` (`:55`) | `dir` fails [`trusted_dir`] |
/// | `No async run status found in '{dir}'.` (`:57`) | `dir` has no readable `status.json` |
/// | `Run '{requested}' does not match status run '{found}'.` (`:58`) | `dir` + `id` disagree |
/// | `Inspector actions require id or dir.` (`:61`) | neither was given |
/// | `No subagent run found for '{requested}'.` (`:64`) | the id resolved to nothing |
/// | `Run '{id}' is not an inspectable async run with lifecycle artifacts.` (`:65`) | resolved, but with no run directory |
/// | the resolver's own message (`:69`) | an ambiguous prefix / unsafe token |
/// | `No lifecycle status exists for async run '{id}'.` (`:73`) | resolved, but `status.json` is unreadable |
/// | `Async run '{id}' has {n} children. Index {i} is out of range.` (`:74`) | `index` out of range |
///
/// `found.kind !== "async"` (`:65`'s first disjunct) has no cyrup counterpart: this crate's
/// resolver is the ASYNC slice only (`background/run_id_resolver.rs`'s module doc), so a
/// foreground id simply does not resolve and takes the `:64` arm instead. The second disjunct —
/// a resolved run with no `asyncDir` — is exactly reproduced.
///
/// # Errors
///
/// The sentence to hand the model, ready for `Err(ToolError::new(..))` at the dispatcher.
pub async fn resolve_target(
    request: &InspectorRequest,
    deps: &InspectorDispatcherDeps,
) -> Result<ResolvedTarget, String> {
    let requested = request
        .id
        .as_deref()
        .or(request.run_id.as_deref())
        .filter(|value| !value.is_empty());

    let (run_id, async_dir) = if let Some(dir) = request.dir.as_deref() {
        let async_dir = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
        if !trusted_dir(&async_dir, deps) {
            return Err(format!(
                "Async run directory '{}' is outside trusted run roots.",
                async_dir.display()
            ));
        }
        let Some(status) = read_status(&async_dir).await else {
            return Err(format!(
                "No async run status found in '{}'.",
                async_dir.display()
            ));
        };
        let found = status.run_id.as_str();
        if let Some(requested) = requested
            && requested != found
            && !found.starts_with(requested)
        {
            return Err(format!(
                "Run '{requested}' does not match status run '{found}'."
            ));
        }
        (found.to_string(), async_dir)
    } else {
        let Some(requested) = requested else {
            return Err("Inspector actions require id or dir.".to_string());
        };
        match resolve_async_run_id(
            requested,
            &deps.async_dir_root,
            &deps.results_dir,
            deps.current_session.as_ref(),
        ) {
            Ok(None) => return Err(format!("No subagent run found for '{requested}'.")),
            Ok(Some(found)) => {
                let Some(dir) = found.async_dir else {
                    return Err(format!(
                        "Run '{}' is not an inspectable async run with lifecycle artifacts.",
                        found.resolved_id.as_str()
                    ));
                };
                (found.resolved_id.as_str().to_string(), dir)
            }
            Err(cause) => return Err(cause.to_string()),
        }
    };

    // `actions.ts:72` re-reads the status even on the `dir` branch that already read one. Ported
    // as written: the two reads are a millisecond apart and the second is the one the range check
    // and the launch cwd are taken from, so collapsing them would change which snapshot wins.
    let Some(status) = read_status(&async_dir).await else {
        return Err(format!(
            "No lifecycle status exists for async run '{run_id}'."
        ));
    };

    let child_index = match request.index {
        None => None,
        Some(index) => {
            let children = status.steps.len();
            let Some(resolved) = usize::try_from(index).ok().filter(|idx| *idx < children) else {
                return Err(format!(
                    "Async run '{run_id}' has {children} children. Index {index} is out of range."
                ));
            };
            Some(resolved)
        }
    };

    Ok(ResolvedTarget {
        target: InspectorTarget {
            run_id,
            async_dir,
            child_index,
        },
        status,
    })
}

// =================================================================================================
// missionFor — `actions.ts:79-89`
// =================================================================================================

/// pi `missionFor` (`actions.ts:79-89`) — the mission backing this run, if any.
///
/// Two rungs, upstream's order: the run's own `mission.json` binding first
/// (`missions/lifecycle.rs:812`), then a scan of the mission store for a record whose `runs` name
/// this run id. Any failure on either rung is `None`, never an error — upstream wraps the whole
/// body in one `try/catch` returning `undefined` (`:86-88`), because a missing or corrupt mission
/// store must not make `inspector.command` fail.
#[must_use]
pub fn mission_for(
    target: &InspectorTarget,
    deps: &InspectorDispatcherDeps,
) -> Option<ResolvedMission> {
    // Upstream wraps BOTH rungs in one `try`, so a throw from `readMissionBinding` or from
    // `missionRecordPath` returns `undefined` WITHOUT falling through to the store scan. An `if
    // let Ok(..)` that dropped into the scan on error would be a behaviour change, not a
    // simplification: a corrupt binding would then silently resolve to whatever unrelated mission
    // happens to list this run id.
    let binding = crate::missions::lifecycle::read_mission_binding(&target.async_dir).ok()?;
    if let Some(binding) = binding {
        return crate::missions::store::mission_record_path(&binding.location, &binding.mission_id)
            .ok()
            .map(|path| ResolvedMission {
                id: binding.mission_id,
                path,
            });
    }
    let location = crate::missions::store::resolve_mission_store_location(
        &deps.cwd,
        deps.missions.as_ref(),
        deps.agent_dir_override.as_deref(),
    );
    let record = crate::missions::store::list_missions(&location)
        .records
        .into_iter()
        .find(|record| {
            record
                .runs
                .iter()
                .any(|run| run.run_id.as_str() == target.run_id)
        })?;
    let path = crate::missions::store::mission_record_path(&location, &record.id).ok()?;
    Some(ResolvedMission {
        id: record.id,
        path,
    })
}

// =================================================================================================
// launchFor — `actions.ts:90-104`
// =================================================================================================

/// pi `launchFor` (`actions.ts:90-104`) — the command a terminal host should run.
///
/// # The one structural delta, and why it is not a narrowing
///
/// Upstream builds `executable = resolveNodeExecutable()` and puts the runner SCRIPT first in
/// argv (`inspector-runner.mjs`, `:92`). cyrup ships one compiled binary with no interpreter to
/// hand a script to, so the same mechanism is a re-exec of this binary under a reserved argv
/// token: `executable = resolve_spawn_command().binary` and the script slot becomes
/// [`INSPECTOR_SUBCOMMAND`](super::types::INSPECTOR_SUBCOMMAND). That is exactly the delta
/// `crates/cyrup/src/subagent_runner_cmd.rs:28-48` already records for `__subagent-runner`; this
/// is its second token, not a new mechanism.
///
/// Everything else is upstream's, in upstream's order: `--async-dir`, `--run-id`, `--allow-steer`,
/// `--allow-stop`, `--session-roots` always; `--index` only when the target names a child (`:98`);
/// `--mission-path` only when a mission was resolved (`:99`).
///
/// `--allow-steer` / `--allow-stop` are the authority policy's verdict on `steerRun` / `stopRun`
/// (`:95-96`) — `"true"` only when the decision is [`AuthorityDecision::Auto`], so a `confirm` or
/// `forbid` policy produces an inspector whose in-pane controls are genuinely disabled rather than
/// one that prompts inside a pane with no UI to prompt through.
///
/// The session roots are the caller's list plus this run's own job root, de-duplicated in first-
/// seen order (upstream's `new Set([...a, ...b])`, `:94`).
#[must_use]
pub fn launch_for(target: &InspectorTarget, deps: &InspectorDispatcherDeps) -> InspectorLaunch {
    launch_for_with_mission(target, mission_for(target, deps).as_ref(), deps)
}

/// [`launch_for`] with the mission already resolved.
///
/// Split out so [`handle_inspector_action`] can look the mission up EXACTLY once per verb and hand
/// the same value to both the launch and the [`InspectorContext`] a plugin receives. Upstream has
/// no such need because its context carries no mission (`inspectors/types.ts:26-32`) and its
/// `launch.mission` is what `openHerdrInspector` reads (`herdr/actions.ts:123`); the frozen
/// contract moved those two fields onto the context, so this is where they are filled from.
#[must_use]
pub fn launch_for_with_mission(
    target: &InspectorTarget,
    mission: Option<&ResolvedMission>,
    deps: &InspectorDispatcherDeps,
) -> InspectorLaunch {
    let mut session_roots: Vec<PathBuf> = Vec::new();
    for root in deps.session_roots.iter().chain(
        deps.live_jobs
            .iter()
            .find(|job| job.run_id == target.run_id)
            .and_then(|job| job.session_root.as_ref()),
    ) {
        if !session_roots.contains(root) {
            session_roots.push(root.clone());
        }
    }

    let allow_steer =
        resolve_authority_decision(AuthorityAction::SteerRun, deps.authority_policy.as_ref())
            == AuthorityDecision::Auto;
    let allow_stop =
        resolve_authority_decision(AuthorityAction::StopRun, deps.authority_policy.as_ref())
            == AuthorityDecision::Auto;

    let spawn_command = crate::spawn::resolve_spawn_command();
    let exe = spawn_command.binary.to_string_lossy().into_owned();

    let mut args = spawn_command.base_args.clone();
    args.push(super::types::INSPECTOR_SUBCOMMAND.to_string());
    args.push("--async-dir".to_string());
    args.push(target.async_dir.to_string_lossy().into_owned());
    args.push("--run-id".to_string());
    args.push(target.run_id.clone());
    args.push("--allow-steer".to_string());
    args.push(allow_steer.to_string());
    args.push("--allow-stop".to_string());
    args.push(allow_stop.to_string());
    args.push("--session-roots".to_string());
    args.push(encode_session_roots(&session_roots));
    if let Some(index) = target.child_index {
        args.push("--index".to_string());
        args.push(index.to_string());
    }
    if let Some(mission) = mission {
        args.push("--mission-path".to_string());
        args.push(mission.path.to_string_lossy().into_owned());
    }

    let display_command = format_shell_command(&exe, &args, host_platform());
    InspectorLaunch {
        exe,
        args,
        display_command,
    }
}

// =================================================================================================
// handleInspectorAction — `actions.ts:120-148`
// =================================================================================================

/// pi's `result(text)` (`actions.ts:20-27`) — a management-mode tool result that is NOT an error.
fn management_text(text: impl Into<String>) -> ToolResult {
    ToolResult {
        content: vec![Content::text(text.into())],
        details: Some(serde_json::json!({ "mode": "management", "results": [] })),
        ..Default::default()
    }
}

/// Build the context a plugin receives.
///
/// # `[CONTRACT-GAP]` — what this cannot fill
///
/// The contract's [`InspectorContext`] has five fields. Upstream's has `cwd`, `signal`, `env`,
/// `now` and a `target` that carries a `{ cwd, state, steps }` status snapshot
/// (`inspectors/types.ts:15-32`). `env` is now one of cyrup's — see
/// [`InspectorContext::env`](crate::inspectors::types::InspectorContext::env) — and exactly one
/// absence is still visible to a backend:
///
/// * **`status.state`** — `statusHerdrInspector` prints `Run state: {state}` (`herdr/actions.ts:143`)
///   and its failure variant repeats it (`:141`).
///
/// [`trusted_dir`](InspectorContext::trusted_dir) is filled with upstream's `pane split --cwd`
/// argument, `context.target.status.cwd ?? context.cwd` (`herdr/actions.ts:105`) — the run's own
/// working directory when it recorded one, else the verb's cwd. That is the only directory in the
/// verb that is neither the async dir (already on the target) nor derivable by a backend.
fn build_context(
    resolved: &ResolvedTarget,
    mission: Option<&ResolvedMission>,
    deps: &InspectorDispatcherDeps,
) -> InspectorContext {
    InspectorContext {
        target: resolved.target.clone(),
        trusted_dir: resolved
            .status
            .cwd
            .clone()
            .unwrap_or_else(|| deps.cwd.clone()),
        mission_id: mission.map(|mission| mission.id.clone()),
        mission_path: mission.map(|mission| mission.path.clone()),
        // pi `env: deps.env ?? process.env` (`actions.ts:126`). The fallback is resolved by the
        // caller that builds the deps, so a backend's gate is a read of this map and never of the
        // ambient process — which is what makes the not-installed path drivable from here.
        env: deps.env.clone(),
    }
}

/// pi `handleInspectorAction` (`actions.ts:120-148`) — the whole `inspector.*` family.
///
/// The ORDER is the contract, not an implementation detail:
///
/// 1. Resolve the target. A refusal here is an error for every verb (`:122`).
/// 2. `inspector.command` answers from the launch alone and **returns before `deps.plugins` is
///    read** (`:130` vs `:131`). No backend, no disk write, not an error.
/// 3. `inspector.open` takes the FIRST plugin whose `available()` is true — `owns()` is NOT
///    consulted here (`:133-134`). It could not be: the first open has no binding, so `owns()` is
///    false for every backend and gating on it would make the verb permanently unreachable.
///    With none available: [`NO_INSPECTOR_PLUGIN_AVAILABLE`], `isError` true, nothing written.
/// 4. `inspector.status` / `inspector.close` find the plugin that `owns()` this binding (`:138`).
///    With none: `No inspector plugin owns this binding for async run {runId}.` — **not an
///    error** (`:139` passes no second argument).
/// 5. A found owner that has no `status`/`close` method answers
///    `Inspector plugin '{name}' does not support {status,close} for async run {runId}.`, which
///    IS an error (`:141-147`). Unreachable through
///    [`builtin_inspector_plugins`](super::plugins::builtin_inspector_plugins) — herdr supplies
///    both methods and ghostty's `owns` is always false — but a third-party backend could reach
///    it, and it is the meaning of the contract's `Option<..>` return.
///
/// 6. **`inspector.open` and `inspector.close` are serialized per target** (SUBA-201, pi
///    `5096985c` / #2728). `status` and `command` stay unlocked, as upstream. See
///    [`acquire_inspector_lease`] for the lock, the wait bound and the two sentences a waiter can
///    end with; the guard is held across `plugin.open` / `owner.close` and — for close — across
///    the `owns()` lookup too, so a close racing an open finds the binding that open wrote.
///
/// `cancel` is upstream's `deps.signal`: `Some` from the tool edge (the turn's token), `None`
/// from a surface that has no token to give (the fleet overlay's `Enter`/`H` key).
///
/// # Errors
///
/// Every upstream `isError: true` reply, as a [`ToolError`] whose message IS upstream's sentence.
pub async fn handle_inspector_action(
    action: InspectorAction,
    request: &InspectorRequest,
    deps: &InspectorDispatcherDeps,
    cancel: Option<&CancelToken>,
) -> Result<ToolResult, ToolError> {
    let resolved = resolve_target(request, deps)
        .await
        .map_err(ToolError::new)?;
    let params = request.plugin_params();
    let run_id = resolved.target.run_id.clone();

    if action == InspectorAction::Command {
        return Ok(management_text(
            launch_for(&resolved.target, deps).display_command,
        ));
    }

    if action == InspectorAction::Status {
        let context = build_context(&resolved, None, deps);
        let Some(owner) = deps.plugins.iter().find(|plugin| plugin.owns(&context)) else {
            return Ok(no_owner(&run_id));
        };
        return match owner.status(&context).await {
            Some(result) => result,
            None => Err(ToolError::new(format!(
                "Inspector plugin '{}' does not support status for async run {run_id}.",
                owner.name()
            ))),
        };
    }

    // pi `actions.ts` @ad11b7ab, after the status branch:
    //
    //     // Open and close read, create and delete the target's binding. Two callers at once, including another Pi
    //     // process sharing the run directory, would each open a pane and one binding would overwrite the other.
    //     ...
    //     try {
    //         // The wait may end with the lease in the same tick the caller cancelled; never act for a cancelled caller.
    //         if (deps.signal?.aborted) return result(`Inspector ${action} for async run ${target.runId}${label} was cancelled while waiting for another inspector open or close to finish.`, true);
    //         ...
    //     } finally {
    //         release();
    //     }
    //
    // `_lease` is the `finally { release() }`: dropped on every exit below, including an `Err`
    // from the plugin (pi's "releases the target after a failed open").
    let _lease = acquire_inspector_lease(action, &resolved.target, deps.lease_wait, cancel).await?;
    if cancel.is_some_and(CancelToken::is_cancelled) {
        return Err(ToolError::new(lease_cancelled_sentence(
            action,
            &resolved.target,
        )));
    }

    if action == InspectorAction::Open {
        // The mission lookup lives INSIDE the loop body, exactly as `actions.ts:134` does: with no
        // available backend, nothing reads the mission store and nothing touches disk.
        let bare = build_context(&resolved, None, deps);
        for plugin in &deps.plugins {
            if plugin.available(&bare).await {
                let mission = mission_for(&resolved.target, deps);
                let launch = launch_for_with_mission(&resolved.target, mission.as_ref(), deps);
                let context = build_context(&resolved, mission.as_ref(), deps);
                return plugin.open(&context, &launch, &params).await;
            }
        }
        return Err(ToolError::new(NO_INSPECTOR_PLUGIN_AVAILABLE));
    }

    let context = build_context(&resolved, None, deps);
    let Some(owner) = deps.plugins.iter().find(|plugin| plugin.owns(&context)) else {
        return Ok(no_owner(&run_id));
    };
    match owner.close(&context).await {
        Some(result) => result,
        None => Err(ToolError::new(format!(
            "Inspector plugin '{}' does not support close for async run {run_id}.",
            owner.name()
        ))),
    }
}

/// `actions.ts:139`'s not-an-error answer when no backend holds a binding.
fn no_owner(run_id: &str) -> ToolResult {
    management_text(format!(
        "No inspector plugin owns this binding for async run {run_id}."
    ))
}

/// pi's `label` (`actions.ts` @ad11b7ab): empty for the run root, `" child {index}"` for a child.
///
/// ```text
/// const label = target.index === undefined ? "" : ` child ${target.index}`;
/// ```
fn lease_label(target: &InspectorTarget) -> String {
    target
        .child_index
        .map_or_else(String::new, |index| format!(" child {index}"))
}

/// pi's abort sentence, verbatim (`actions.ts` @ad11b7ab):
/// `Inspector ${action} for async run ${target.runId}${label} was cancelled while waiting for
/// another inspector open or close to finish.`
fn lease_cancelled_sentence(action: InspectorAction, target: &InspectorTarget) -> String {
    format!(
        "Inspector {} for async run {}{} was cancelled while waiting for another inspector open or close to finish.",
        action.as_str(),
        target.run_id,
        lease_label(target)
    )
}

/// pi's timeout sentence, verbatim (`actions.ts` @ad11b7ab):
/// `Another inspector open or close for async run ${target.runId}${label} is still in progress.
/// Try again when it finishes.`
fn lease_timeout_sentence(target: &InspectorTarget) -> String {
    format!(
        "Another inspector open or close for async run {}{} is still in progress. Try again when it finishes.",
        target.run_id,
        lease_label(target)
    )
}

/// The per-target open/close lock (SUBA-201) — pi `actions.ts` @ad11b7ab:
///
/// ```text
/// const label = target.index === undefined ? "" : ` child ${target.index}`;
/// const lockPath = path.join(fs.realpathSync.native(target.asyncDir), `inspector-${target.index ?? "root"}.lock`);
/// const deadline = Date.now() + (deps.leaseWaitMs ?? INSPECTOR_LEASE_WAIT_MS);
/// let release = tryLease(lockPath);
/// while (!release) {
///     if (deps.signal?.aborted) return result(`Inspector ${action} for async run ${target.runId}${label} was cancelled while waiting for another inspector open or close to finish.`, true);
///     if (Date.now() >= deadline) return result(`Another inspector open or close for async run ${target.runId}${label} is still in progress. Try again when it finishes.`, true);
///     await new Promise((resolve) => setTimeout(resolve, 50));
///     release = tryLease(lockPath);
/// }
/// ```
///
/// The lock path is the same: [`FileLock`](cyrup_config::lock::FileLock) appends `.lock` to
/// `<realpath asyncDir>/inspector-<index|root>`, so the file is pi's
/// `inspector-<index|root>.lock`. Per TARGET, not per run: another child's open is not held up
/// (pi's "without holding up other targets").
///
/// `[CYRUP-DELTA]` the mechanism. pi's `tryLease` (`src/shared/file-lease.ts`) is a `mkdir` lease
/// with an `owner.json` (`{token, pid, hostname, processStart}`) and dead-owner reclaim, because
/// node has no advisory lock. cyrup takes `flock` on the sidecar through
/// `cyrup_config::lock::FileLock` — the one lock discipline SUBA-183/184 use: the kernel releases
/// it when the owner dies (no pid-reuse window, no unreadable-owner wedge, nothing to reclaim),
/// and it excludes in-process callers too (layer 1 is a per-path async mutex, layer 2 `flock` on
/// an open file description). Consequences: the sidecar FILE persists after release (pi removes
/// its lease directory), it is created with the default umask rather than pi's `0o700`, and on
/// network filesystems where `flock` is local-only (sshfs, CIFS `nobrl`) two HOSTS are not
/// excluded, where pi's `mkdir` is NFS-atomic. The wait is not pi's 50 ms poll: layer 1 wakes on
/// release, layer 2 polls with backoff capped at 50 ms. The bound and the two sentences are pi's.
///
/// A cancel ends the wait with pi's abort sentence (`FileLock::acquire` returns
/// `ConfigError::Cancelled`, pre-checked before any wait); the caller re-checks after a GRANTED
/// lock, because a cancel during an in-flight attempt still yields the lock.
async fn acquire_inspector_lease(
    action: InspectorAction,
    target: &InspectorTarget,
    wait: std::time::Duration,
    cancel: Option<&CancelToken>,
) -> Result<cyrup_config::lock::FileLock, ToolError> {
    // pi `fs.realpathSync.native(target.asyncDir)` — throws on failure, which rejects the call.
    let physical = tokio::fs::canonicalize(&target.async_dir)
        .await
        .map_err(|error| {
            ToolError::new(format!(
                "Cannot resolve async run directory '{}': {error}",
                target.async_dir.display()
            ))
        })?;
    let slot = target
        .child_index
        .map_or_else(|| "root".to_string(), |index| index.to_string());
    let lock_target = physical.join(format!("inspector-{slot}"));
    match tokio::time::timeout(
        wait,
        cyrup_config::lock::FileLock::acquire(&lock_target, cancel),
    )
    .await
    {
        Ok(Ok(lock)) => Ok(lock),
        Ok(Err(cyrup_config::ConfigError::Cancelled)) => {
            Err(ToolError::new(lease_cancelled_sentence(action, target)))
        }
        Ok(Err(error)) => Err(ToolError::new(error.to_string())),
        Err(_elapsed) => Err(ToolError::new(lease_timeout_sentence(target))),
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

    use std::sync::Mutex;

    use super::*;
    use crate::background::{RunId, RunMode, RunState, StepStatus};

    // ---------------------------------------------------------------------------------------
    // Fixtures
    // ---------------------------------------------------------------------------------------

    struct Fixture {
        root: tempfile::TempDir,
        async_root: PathBuf,
        results_dir: PathBuf,
        cwd: PathBuf,
    }

    fn fixture() -> Fixture {
        let root = tempfile::tempdir().expect("tempdir");
        let async_root = root.path().join("async");
        let results_dir = root.path().join("results");
        let cwd = root.path().join("project");
        std::fs::create_dir_all(&async_root).expect("async root");
        std::fs::create_dir_all(&results_dir).expect("results dir");
        std::fs::create_dir_all(&cwd).expect("cwd");
        Fixture {
            root,
            async_root,
            results_dir,
            cwd,
        }
    }

    impl Fixture {
        fn deps(&self) -> InspectorDispatcherDeps {
            InspectorDispatcherDeps {
                cwd: self.cwd.clone(),
                async_dir_root: self.async_root.clone(),
                results_dir: self.results_dir.clone(),
                current_session: None,
                live_jobs: Vec::new(),
                session_roots: Vec::new(),
                // No herdr and no ghostty, which is what the built-in backends' gates read and
                // what this container actually is.
                env: std::collections::BTreeMap::new(),
                // A tempdir mission store, so a stray `~/.cyrup` on the dev box cannot leak a
                // real mission into `mission_for`'s second rung.
                missions: Some(MissionStoreConfig {
                    directory: Some(
                        self.root
                            .path()
                            .join("missions")
                            .to_string_lossy()
                            .into_owned(),
                    ),
                    global_index: Some(false),
                    ..MissionStoreConfig::default()
                }),
                agent_dir_override: Some(self.root.path().join("agent")),
                authority_policy: None,
                plugins: Vec::new(),
                lease_wait: INSPECTOR_LEASE_WAIT,
            }
        }

        /// Writes a run directory with a `status.json` carrying `children` steps.
        fn write_run(&self, run_id: &str, children: usize) -> PathBuf {
            let dir = self.async_root.join(run_id);
            std::fs::create_dir_all(&dir).expect("run dir");
            let mut status =
                RunStatus::queued(RunId::from_token(run_id), RunMode::Chain, Some(4242));
            status.state = RunState::Running;
            status.cwd = Some(self.cwd.clone());
            status.steps = (0..children)
                .map(|index| StepStatus::pending(format!("agent-{index}")))
                .collect();
            let json = serde_json::to_vec_pretty(&status).expect("serialize status");
            std::fs::write(RunDir::for_existing(&dir).status(), json).expect("write status");
            dir
        }
    }

    /// Rewrite an already-written `status.json`'s `cwd`, so a test can make the run's own working
    /// directory differ from the dispatcher's.
    fn rewrite_status_cwd(dir: &std::path::Path, cwd: &std::path::Path) {
        let path = RunDir::for_existing(dir).status();
        let bytes = std::fs::read(&path).expect("read status");
        let mut status: RunStatus = serde_json::from_slice(&bytes).expect("parse status");
        status.cwd = Some(cwd.to_path_buf());
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&status).expect("serialize"),
        )
        .expect("write");
    }

    fn request_for(id: &str) -> InspectorRequest {
        InspectorRequest {
            id: Some(id.to_string()),
            ..InspectorRequest::default()
        }
    }

    /// A shared call log, so a test can keep a handle on it after the plugin has been boxed into
    /// [`InspectorDispatcherDeps::plugins`].
    type CallLog = std::sync::Arc<Mutex<Vec<String>>>;

    /// A backend that records every consult, so a test can assert exactly what was consulted.
    #[derive(Default)]
    struct RecordingPlugin {
        available: bool,
        owns: bool,
        has_status: bool,
        has_close: bool,
        calls: CallLog,
        /// Which recorder this is, so a reply names its author.
        tag: &'static str,
    }

    #[async_trait::async_trait]
    impl InspectorPlugin for RecordingPlugin {
        fn name(&self) -> &'static str {
            "recorder"
        }

        async fn available(&self, _ctx: &InspectorContext) -> bool {
            if let Ok(mut calls) = self.calls.lock() {
                calls.push("available".to_string());
            }
            self.available
        }

        fn owns(&self, _ctx: &InspectorContext) -> bool {
            self.owns
        }

        async fn open(
            &self,
            ctx: &InspectorContext,
            launch: &InspectorLaunch,
            params: &InspectorParams,
        ) -> Result<ToolResult, ToolError> {
            if let Ok(mut calls) = self.calls.lock() {
                calls.push(format!(
                    "open:{}:{}:{}:{}",
                    ctx.target.run_id,
                    ctx.trusted_dir.display(),
                    launch.display_command,
                    params.focus.unwrap_or(false)
                ));
            }
            Ok(management_text("opened"))
        }

        async fn status(&self, _ctx: &InspectorContext) -> Option<Result<ToolResult, ToolError>> {
            // The reply carries the recorder's OWN tag. Two recorders answering the same bytes
            // would make "the owner answered" indistinguishable from "somebody answered", which
            // is exactly how `status_and_close_route_through_the_owning_plugin` used to pass with
            // the `owns()` filter removed.
            self.has_status
                .then(|| Ok(management_text(format!("plugin status:{}", self.tag))))
        }

        async fn close(&self, _ctx: &InspectorContext) -> Option<Result<ToolResult, ToolError>> {
            self.has_close
                .then(|| Ok(management_text(format!("plugin close:{}", self.tag))))
        }
    }

    fn text_of(result: &ToolResult) -> String {
        result
            .content
            .iter()
            .map(|content| match content {
                Content::Text { text, .. } => text.to_string(),
                _ => String::new(),
            })
            .collect()
    }

    // ---------------------------------------------------------------------------------------
    // T-TGT-1 — every refusal sentence resolveTarget can produce
    // ---------------------------------------------------------------------------------------

    /// T-TGT-1. Upstream's eight refusals, each at its own decision point. GUT any arm to a single
    /// generic "run not found" and exactly that row goes RED — which is the point: these are the
    /// sentences a model actually reads when it mistypes an id, and a shallow test that only
    /// checks `is_err()` would pass with all eight collapsed into one.
    #[tokio::test]
    async fn resolve_target_refuses_with_each_of_upstreams_sentences() {
        let fixture = fixture();
        let deps = fixture.deps();

        // 1. neither id nor dir
        let err = resolve_target(&InspectorRequest::default(), &deps)
            .await
            .expect_err("no id, no dir");
        assert_eq!(err, "Inspector actions require id or dir.");

        // 2. an id that resolves to nothing
        let err = resolve_target(&request_for("deadbeef"), &deps)
            .await
            .expect_err("unknown id");
        assert_eq!(err, "No subagent run found for 'deadbeef'.");

        // 3. a dir outside the trusted roots
        let outside = fixture.root.path().join("outside");
        std::fs::create_dir_all(&outside).expect("outside");
        let err = resolve_target(
            &InspectorRequest {
                dir: Some(outside.clone()),
                ..InspectorRequest::default()
            },
            &deps,
        )
        .await
        .expect_err("outside");
        assert_eq!(
            err,
            format!(
                "Async run directory '{}' is outside trusted run roots.",
                outside.display()
            )
        );

        // 4. a trusted dir with no status.json
        let empty = fixture.async_root.join("empty");
        std::fs::create_dir_all(&empty).expect("empty run dir");
        let err = resolve_target(
            &InspectorRequest {
                dir: Some(empty.clone()),
                ..InspectorRequest::default()
            },
            &deps,
        )
        .await
        .expect_err("no status");
        assert_eq!(
            err,
            format!("No async run status found in '{}'.", empty.display())
        );

        // 5. a dir whose status names a different run than the id
        let dir = fixture.write_run("aaaa1111", 2);
        let err = resolve_target(
            &InspectorRequest {
                id: Some("zzzz".to_string()),
                dir: Some(dir.clone()),
                ..InspectorRequest::default()
            },
            &deps,
        )
        .await
        .expect_err("mismatch");
        assert_eq!(err, "Run 'zzzz' does not match status run 'aaaa1111'.");

        // 6. an out-of-range child index (and the negative one, which `usize` alone would lose)
        for index in [2_i64, -1] {
            let err = resolve_target(
                &InspectorRequest {
                    id: Some("aaaa1111".to_string()),
                    index: Some(index),
                    ..InspectorRequest::default()
                },
                &deps,
            )
            .await
            .expect_err("out of range");
            assert_eq!(
                err,
                format!("Async run 'aaaa1111' has 2 children. Index {index} is out of range.")
            );
        }

        // 7. an ambiguous prefix reports the resolver's own message
        fixture.write_run("bbbb1111", 1);
        fixture.write_run("bbbb2222", 1);
        let err = resolve_target(&request_for("bbbb"), &deps)
            .await
            .expect_err("ambiguous");
        assert!(
            err.contains("Ambiguous subagent run id prefix 'bbbb'"),
            "resolver message expected, got {err}"
        );
    }

    /// The `:73` arm: the run resolves by id, but its `status.json` is gone by the time the second
    /// read happens. GUT the second read away and this goes RED.
    #[tokio::test]
    async fn a_run_whose_status_vanishes_reports_the_lifecycle_sentence() {
        let fixture = fixture();
        let deps = fixture.deps();
        let dir = fixture.write_run("cccc1111", 1);
        // Resolution by id only needs the directory to exist; the status read is separate.
        std::fs::remove_file(RunDir::for_existing(&dir).status()).expect("remove status");
        let err = resolve_target(&request_for("cccc1111"), &deps)
            .await
            .expect_err("no lifecycle status");
        assert_eq!(err, "No lifecycle status exists for async run 'cccc1111'.");
    }

    /// A valid target carries the child index through untouched.
    #[tokio::test]
    async fn a_valid_child_index_resolves_onto_the_target() {
        let fixture = fixture();
        let deps = fixture.deps();
        let dir = fixture.write_run("dddd1111", 3);
        let resolved = resolve_target(
            &InspectorRequest {
                id: Some("dddd1111".to_string()),
                index: Some(2),
                ..InspectorRequest::default()
            },
            &deps,
        )
        .await
        .expect("resolves");
        assert_eq!(resolved.target.run_id, "dddd1111");
        assert_eq!(resolved.target.async_dir, dir);
        assert_eq!(resolved.target.child_index, Some(2));
    }

    // ---------------------------------------------------------------------------------------
    // T-TRUST-1 — both rungs of trustedDir
    // ---------------------------------------------------------------------------------------

    /// T-TRUST-1. Rung 2 (containment) accepts a directory under the configured root and refuses
    /// one outside it; rung 1 (live job) accepts a directory under NO root at all. GUT rung 1 and
    /// the live-job row goes RED — which in the field means a run launched under a configured
    /// non-default async root becomes uninspectable. GUT the realpath leg of rung 2 and the
    /// symlink-escape row goes RED.
    #[test]
    fn trusted_dir_honours_both_rungs() {
        let fixture = fixture();
        let mut deps = fixture.deps();

        let inside = fixture.async_root.join("run-a");
        std::fs::create_dir_all(&inside).expect("inside");
        assert!(
            trusted_dir(&inside, &deps),
            "a dir under the root is trusted"
        );

        let elsewhere = fixture.root.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).expect("elsewhere");
        assert!(
            !trusted_dir(&elsewhere, &deps),
            "a dir outside the root is not trusted"
        );

        // Rung 1: the same outside dir becomes trusted once a live job names it.
        deps.live_jobs.push(LiveInspectorJob {
            run_id: "whatever".to_string(),
            async_dir: elsewhere.clone(),
            session_root: None,
        });
        assert!(
            trusted_dir(&elsewhere, &deps),
            "a live job's own dir is trusted regardless of the root"
        );

        // A missing directory is never trusted, and never a panic.
        assert!(!trusted_dir(&fixture.async_root.join("nope"), &deps));
    }

    /// The `:36` symlink refusal, and the second `pathWithin` at `:46`. A symlink placed INSIDE
    /// the root pointing outside it must not be trusted. GUT either the symlink check or the
    /// realpath leg and this goes RED.
    #[cfg(unix)]
    #[test]
    fn a_symlink_never_escapes_the_trusted_root() {
        let fixture = fixture();
        let deps = fixture.deps();
        let outside = fixture.root.path().join("outside-target");
        std::fs::create_dir_all(&outside).expect("outside");
        let link = fixture.async_root.join("sneaky");
        std::os::unix::fs::symlink(&outside, &link).expect("symlink");
        assert!(
            !trusted_dir(&link, &deps),
            "a symlink under the root must not be trusted"
        );

        // The OTHER half of `:36`, and the only rung that sees it. A symlink whose target is
        // itself inside the root passes BOTH `pathWithin` calls — literal and realpath'd — so the
        // realpath leg above cannot refuse it. Upstream refuses any symlink outright, before
        // containment is consulted at all, and deleting that check leaves every row above green.
        let inside = fixture.async_root.join("real-run");
        std::fs::create_dir_all(&inside).expect("inside target");
        let inward = fixture.async_root.join("inward-link");
        std::os::unix::fs::symlink(&inside, &inward).expect("symlink");
        assert!(
            trusted_dir(&inside, &deps),
            "the target itself is a plain directory under the root and IS trusted"
        );
        assert!(
            !trusted_dir(&inward, &deps),
            "a symlink is refused at `:36` even when its target is inside the root"
        );
    }

    // ---------------------------------------------------------------------------------------
    // T-CMD-1/2 — inspector.command with nothing installed
    // ---------------------------------------------------------------------------------------

    /// T-CMD-1. `inspector.command` answers with the full launch string on a box with no herdr and
    /// no ghostty, and it is NOT an error. GUT the `Command` arm to fall through to the plugin
    /// loop and this goes RED with upstream's "no inspector plugin" sentence — the exact
    /// regression that would make the feature useless on every Linux CI box.
    #[tokio::test]
    async fn inspector_command_answers_with_no_backend_installed() {
        let fixture = fixture();
        let deps = fixture.deps();
        fixture.write_run("eeee1111", 1);

        let result = handle_inspector_action(
            InspectorAction::Command,
            &request_for("eeee1111"),
            &deps,
            None,
        )
        .await
        .expect("command never fails for a resolvable run");

        let text = text_of(&result);
        assert!(
            text.contains("__subagent-inspector"),
            "launch must name the inspector subcommand: {text}"
        );
        assert!(text.contains("--async-dir"), "{text}");
        assert!(text.contains("--run-id"), "{text}");
        assert!(text.contains("--allow-steer"), "{text}");
        assert!(text.contains("--allow-stop"), "{text}");
        assert!(text.contains("--session-roots"), "{text}");
        assert_eq!(
            result.details,
            Some(serde_json::json!({ "mode": "management", "results": [] }))
        );
    }

    /// T-CMD-2. The `--session-roots` value is the BASE64 payload, not the raw JSON, and it
    /// round-trips. GUT `encode_session_roots` back to plain JSON and this goes RED — the failure
    /// in the field is a launch line PowerShell splits into several argv tokens.
    #[tokio::test]
    async fn the_launch_carries_the_base64_session_roots() {
        let fixture = fixture();
        let mut deps = fixture.deps();
        deps.session_roots = vec![PathBuf::from("/roots/a b"), PathBuf::from("/roots/c")];
        let dir = fixture.write_run("ffff1111", 1);

        let resolved = resolve_target(&request_for("ffff1111"), &deps)
            .await
            .expect("resolves");
        let launch = launch_for(&resolved.target, &deps);

        let roots_index = launch
            .args
            .iter()
            .position(|arg| arg == "--session-roots")
            .expect("--session-roots is always emitted");
        let encoded = &launch.args[roots_index + 1];
        assert!(!encoded.contains('"'), "payload must be shell-inert");
        assert_eq!(
            super::super::session_roots_codec::decode_session_roots(encoded).expect("decodes"),
            vec![PathBuf::from("/roots/a b"), PathBuf::from("/roots/c")]
        );
        let async_index = launch
            .args
            .iter()
            .position(|arg| arg == "--async-dir")
            .expect("--async-dir is always emitted");
        assert_eq!(
            launch.args[async_index + 1],
            dir.to_string_lossy(),
            "the async dir follows --async-dir"
        );
        assert!(launch.display_command.contains(encoded));
    }

    /// The job's own session root joins the caller's list, de-duplicated in first-seen order
    /// (`actions.ts:94`). GUT the job leg and this goes RED: the inspector would then refuse to
    /// read the very transcript it was opened for.
    #[tokio::test]
    async fn the_runs_own_session_root_joins_the_launch() {
        let fixture = fixture();
        let mut deps = fixture.deps();
        deps.session_roots = vec![PathBuf::from("/shared")];
        deps.live_jobs.push(LiveInspectorJob {
            run_id: "1111aaaa".to_string(),
            async_dir: fixture.async_root.join("1111aaaa"),
            session_root: Some(PathBuf::from("/job-root")),
        });
        fixture.write_run("1111aaaa", 1);

        let resolved = resolve_target(&request_for("1111aaaa"), &deps)
            .await
            .expect("resolves");
        let launch = launch_for(&resolved.target, &deps);
        let roots_index = launch
            .args
            .iter()
            .position(|arg| arg == "--session-roots")
            .expect("present");
        assert_eq!(
            super::super::session_roots_codec::decode_session_roots(&launch.args[roots_index + 1])
                .expect("decodes"),
            vec![PathBuf::from("/shared"), PathBuf::from("/job-root")]
        );
    }

    /// T-AUTH-1. `--allow-steer` / `--allow-stop` are the authority policy's verdict, not
    /// constants. GUT either to a hard-coded `"true"` and this goes RED; in the field that
    /// mutation hands an inspector pane controls the operator's policy forbids.
    #[tokio::test]
    async fn the_launch_flags_follow_the_authority_policy() {
        let fixture = fixture();
        fixture.write_run("2222aaaa", 1);

        let read_flag = |args: &[String], flag: &str| -> String {
            let index = args
                .iter()
                .position(|arg| arg == flag)
                .expect("flag present");
            args[index + 1].clone()
        };

        // Default policy: steerRun and stopRun are both `auto`.
        let deps = fixture.deps();
        let resolved = resolve_target(&request_for("2222aaaa"), &deps)
            .await
            .expect("resolves");
        let launch = launch_for(&resolved.target, &deps);
        assert_eq!(read_flag(&launch.args, "--allow-steer"), "true");
        assert_eq!(read_flag(&launch.args, "--allow-stop"), "true");

        // A policy that gates them produces `false` for both.
        let mut gated = fixture.deps();
        gated.authority_policy = Some(AuthorityPolicyConfig {
            steer_run: Some(AuthorityDecision::Confirm),
            stop_run: Some(AuthorityDecision::Forbid),
            ..AuthorityPolicyConfig::default()
        });
        let launch = launch_for(&resolved.target, &gated);
        assert_eq!(read_flag(&launch.args, "--allow-steer"), "false");
        assert_eq!(read_flag(&launch.args, "--allow-stop"), "false");
    }

    /// `--index` appears only for a child-scoped target (`actions.ts:98`).
    #[tokio::test]
    async fn the_child_index_flag_is_emitted_only_when_the_target_names_a_child() {
        let fixture = fixture();
        let deps = fixture.deps();
        fixture.write_run("3333aaaa", 3);

        let whole_run = resolve_target(&request_for("3333aaaa"), &deps)
            .await
            .expect("resolves");
        assert!(
            !launch_for(&whole_run.target, &deps)
                .args
                .iter()
                .any(|arg| arg == "--index")
        );

        let child = resolve_target(
            &InspectorRequest {
                id: Some("3333aaaa".to_string()),
                index: Some(1),
                ..InspectorRequest::default()
            },
            &deps,
        )
        .await
        .expect("resolves");
        let args = launch_for(&child.target, &deps).args;
        let index = args
            .iter()
            .position(|arg| arg == "--index")
            .expect("--index");
        assert_eq!(args[index + 1], "1");
    }

    /// `--mission-path` appears only when a mission was resolved, and it points at the record the
    /// binding names (`actions.ts:99`). GUT `mission_for`'s binding rung to `None` and this goes
    /// RED: the inspector pane would render no mission header for a mission-bound run.
    #[tokio::test]
    async fn a_mission_bound_run_carries_its_record_path_into_the_launch() {
        let fixture = fixture();
        let deps = fixture.deps();
        let dir = fixture.write_run("4444aaaa", 1);

        let resolved = resolve_target(&request_for("4444aaaa"), &deps)
            .await
            .expect("resolves");
        assert!(
            !launch_for(&resolved.target, &deps)
                .args
                .iter()
                .any(|arg| arg == "--mission-path"),
            "no binding, no mission flag"
        );

        // The binding file `read_mission_binding` looks for, with a mission dir of our own.
        let mission_dir = fixture.root.path().join("missions");
        std::fs::create_dir_all(&mission_dir).expect("mission dir");
        std::fs::write(
            dir.join(crate::missions::lifecycle::MISSION_BINDING_FILE),
            serde_json::json!({
                "schemaVersion": 1,
                "missionId": "mission-1",
                "projectRoot": fixture.cwd,
                "missionDir": mission_dir,
                "globalIndexDir": fixture.root.path().join("global-index"),
                "writeGlobalIndex": false,
            })
            .to_string(),
        )
        .expect("write binding");

        let mission = mission_for(&resolved.target, &deps).expect("binding resolves a mission");
        assert_eq!(mission.id, "mission-1");
        assert_eq!(mission.path, mission_dir.join("mission-1.json"));

        let args = launch_for(&resolved.target, &deps).args;
        let flag = args
            .iter()
            .position(|arg| arg == "--mission-path")
            .expect("--mission-path");
        assert_eq!(args[flag + 1], mission.path.to_string_lossy());
    }

    // ---------------------------------------------------------------------------------------
    // T-OPEN-0 / T-STAT-2 — the not-installed refusals
    // ---------------------------------------------------------------------------------------

    /// T-OPEN-0. With no available backend, `inspector.open` refuses with upstream's exact
    /// sentence, it IS an error, and **nothing is written to disk** — no `inspectors/` directory,
    /// no binding. GUT the `for` loop's `available()` guard to `true` and the assertion on the
    /// sentence goes RED; GUT `launch_for` out of the loop body and the "no disk writes"
    /// assertion goes RED for a mission-store read.
    #[tokio::test]
    async fn inspector_open_refuses_with_no_backend_and_writes_nothing() {
        let fixture = fixture();
        let deps = fixture.deps();
        let dir = fixture.write_run("5555aaaa", 1);

        let err =
            handle_inspector_action(InspectorAction::Open, &request_for("5555aaaa"), &deps, None)
                .await
                .expect_err("no backend is an error");
        assert_eq!(err.message, NO_INSPECTOR_PLUGIN_AVAILABLE);
        assert_eq!(
            err.message,
            "No inspector plugin is available. Start a supported inspector host, or use inspector.command for a standalone command."
        );
        assert!(
            !dir.join("inspectors").exists(),
            "the refusal path must not create the binding directory"
        );
        assert!(
            !dir.join(crate::missions::lifecycle::MISSION_BINDING_FILE)
                .exists(),
            "the refusal path writes nothing beside the run either"
        );
    }

    /// **The two BUILT-IN backends' `available()` gates, driven through the dispatcher.**
    ///
    /// This is the path a user on a box with no herdr and no ghostty takes, and before
    /// [`InspectorContext::env`] existed it could not be driven from here at all: each backend
    /// injected its own environment on its own struct, so the dispatcher — which is what decides
    /// which backend runs — had no way to say "nothing is installed". Every test of the
    /// not-installed answer was therefore a test of ONE plugin in isolation, and the one line that
    /// joins them (`for plugin in &deps.plugins { if plugin.available(&bare).await }`) was crossed
    /// by no test with a real backend in the list.
    ///
    /// Three environments, the REAL [`super::plugins::builtin_inspector_plugins`] in every one:
    ///
    /// 1. empty — neither backend's gate passes, so [`NO_INSPECTOR_PLUGIN_AVAILABLE`], an error,
    ///    and nothing on disk;
    /// 2. herdr's two variables — the herdr backend claims it and answers as herdr (there is no
    ///    socket at that path, so it is herdr's own install sentence, which is a DIFFERENT
    ///    sentence and proves the arm ran);
    /// 3. `TERM_PROGRAM=Ghostty` — no herdr, and ghostty's gate is `platform === "darwin"` too,
    ///    so on this Linux box it stays unavailable. The row is here because it is the one that
    ///    would go green by accident if `available()` ever dropped its platform conjunct.
    ///
    /// *Gutted by*: `build_context` passing a default `env` instead of `deps.env` (row 2 falls
    /// back to row 1's answer); either plugin reading the ambient process environment again
    /// (row 2 goes red in this container, which has no herdr); `HerdrInspectorPlugin::available`
    /// regaining its `self.client.is_some() ||` disjunct (row 1 goes red).
    #[tokio::test]
    async fn the_builtin_backends_gate_on_the_contexts_env_through_the_dispatcher() {
        let fixture = fixture();
        let dir = fixture.write_run("8888bbbb", 1);

        // 1 — nothing installed.
        let mut deps = fixture.deps();
        deps.plugins = super::super::plugins::builtin_inspector_plugins();
        let err =
            handle_inspector_action(InspectorAction::Open, &request_for("8888bbbb"), &deps, None)
                .await
                .expect_err("no backend is an error");
        assert_eq!(err.message, NO_INSPECTOR_PLUGIN_AVAILABLE);
        assert!(
            !dir.join("inspectors").exists(),
            "the refusal path must not create the binding directory"
        );

        // 2 — inside a herdr pane. The gate passes and the herdr backend answers; whatever it
        // says, it must not be the dispatcher's "no backend" sentence.
        let mut deps = fixture.deps();
        deps.plugins = super::super::plugins::builtin_inspector_plugins();
        deps.env = std::collections::BTreeMap::from([
            ("HERDR_ENV".to_owned(), "1".to_owned()),
            ("HERDR_PANE_ID".to_owned(), "w1:p1".to_owned()),
            (
                "HERDR_SOCKET_PATH".to_owned(),
                fixture
                    .root
                    .path()
                    .join("no-such-herdr.sock")
                    .to_string_lossy()
                    .into_owned(),
            ),
        ]);
        let answer =
            handle_inspector_action(InspectorAction::Open, &request_for("8888bbbb"), &deps, None)
                .await;
        let text = match &answer {
            Ok(result) => text_of(result),
            Err(error) => error.message.clone(),
        };
        assert_ne!(
            text, NO_INSPECTOR_PLUGIN_AVAILABLE,
            "herdr's gate passed, so the herdr arm must have run; got {text}"
        );
        assert!(
            text.starts_with("Herdr inspector error"),
            "the answer must be herdr's own, not the dispatcher's; got {text}"
        );

        // 3 — a Ghostty terminal on a NON-darwin host: still unavailable.
        let mut deps = fixture.deps();
        deps.plugins = super::super::plugins::builtin_inspector_plugins();
        deps.env =
            std::collections::BTreeMap::from([("TERM_PROGRAM".to_owned(), "Ghostty".to_owned())]);
        let err =
            handle_inspector_action(InspectorAction::Open, &request_for("8888bbbb"), &deps, None)
                .await
                .expect_err("ghostty needs darwin too");
        assert_eq!(err.message, NO_INSPECTOR_PLUGIN_AVAILABLE);
    }

    /// `available()` is consulted and `owns()` is NOT, for `inspector.open`. GUT the loop to gate
    /// on `owns()` as well and this goes RED — which is the bug that would make the FIRST open of
    /// any run permanently impossible, since no binding exists yet.
    #[tokio::test]
    async fn inspector_open_takes_the_first_available_plugin_without_consulting_owns() {
        let fixture = fixture();
        let mut deps = fixture.deps();
        fixture.write_run("6666aaaa", 1);
        deps.plugins.push(Box::new(RecordingPlugin {
            available: true,
            owns: false,
            ..RecordingPlugin::default()
        }));

        let result = handle_inspector_action(
            InspectorAction::Open,
            &InspectorRequest {
                id: Some("6666aaaa".to_string()),
                focus: Some(true),
                ..InspectorRequest::default()
            },
            &deps,
            None,
        )
        .await
        .expect("the available plugin opens");
        assert_eq!(text_of(&result), "opened");
    }

    /// The plugin receives the launch, the resolved target, the `focus` param and a
    /// `trusted_dir` that is the RUN's own cwd — `context.target.status.cwd ?? context.cwd`
    /// (`herdr/actions.ts:105`), which is what `pane split --cwd` is given. GUT `build_context`
    /// to always use `deps.cwd` and this goes RED; in the field that opens every inspector pane in
    /// the project root instead of the run's working directory.
    #[tokio::test]
    async fn the_plugin_receives_the_launch_and_the_runs_own_cwd() {
        let fixture = fixture();
        let mut deps = fixture.deps();
        let dir = fixture.write_run("7777aaaa", 1);
        // The run's own cwd must DIFFER from `deps.cwd`, or the assertion below cannot tell the
        // two apart and `build_context` returning `deps.cwd` unconditionally passes.
        let run_cwd = fixture.root.path().join("the-runs-own-cwd");
        std::fs::create_dir_all(&run_cwd).expect("run cwd");
        rewrite_status_cwd(&dir, &run_cwd);
        let calls: CallLog = CallLog::default();
        deps.plugins.push(Box::new(RecordingPlugin {
            available: true,
            calls: std::sync::Arc::clone(&calls),
            ..RecordingPlugin::default()
        }));

        handle_inspector_action(
            InspectorAction::Open,
            &InspectorRequest {
                id: Some("7777aaaa".to_string()),
                focus: Some(true),
                ..InspectorRequest::default()
            },
            &deps,
            None,
        )
        .await
        .expect("opens");

        let calls = calls.lock().expect("lock").clone();
        assert_eq!(calls.first().map(String::as_str), Some("available"));
        let open = calls.get(1).cloned().unwrap_or_default();
        assert!(open.starts_with("open:7777aaaa:"), "{open}");
        assert!(
            open.contains(&run_cwd.display().to_string()),
            "the run's own cwd must reach the plugin: {open}"
        );
        assert!(
            !open.contains(&format!(":{}:", fixture.cwd.display())),
            "the dispatcher's cwd must NOT be what the plugin is launched in: {open}"
        );
        assert!(open.contains("__subagent-inspector"), "{open}");
        assert!(
            open.ends_with(":true"),
            "focus must reach the plugin: {open}"
        );
    }

    /// T-STAT-2. With no owner, `inspector.status` and `inspector.close` answer upstream's
    /// sentence and are **NOT errors** — `actions.ts:139` passes no second argument. GUT either to
    /// `Err(..)` and this goes RED. This is the single easiest thing to get wrong in the port: the
    /// natural `Result` shape flips the flag, and a status query with no inspector open would then
    /// read to the model as a failure.
    #[tokio::test]
    async fn status_and_close_with_no_owner_are_answers_not_errors() {
        let fixture = fixture();
        let deps = fixture.deps();
        fixture.write_run("8888aaaa", 1);

        for action in [InspectorAction::Status, InspectorAction::Close] {
            let result = handle_inspector_action(action, &request_for("8888aaaa"), &deps, None)
                .await
                .unwrap_or_else(|err| panic!("{} must not be an error: {err:?}", action.as_str()));
            assert_eq!(
                text_of(&result),
                "No inspector plugin owns this binding for async run 8888aaaa."
            );
            assert_eq!(
                result.details,
                Some(serde_json::json!({ "mode": "management", "results": [] }))
            );
        }
    }

    /// A plugin that owns the binding answers both verbs; one that does not own it is skipped even
    /// when it is available. GUT the `owns()` filter to `available()` and this goes RED.
    #[tokio::test]
    async fn status_and_close_route_through_the_owning_plugin() {
        let fixture = fixture();
        let mut deps = fixture.deps();
        fixture.write_run("9999aaaa", 1);
        deps.plugins.push(Box::new(RecordingPlugin {
            available: true,
            owns: false,
            has_status: true,
            has_close: true,
            tag: "available-but-not-owning",
            ..RecordingPlugin::default()
        }));
        deps.plugins.push(Box::new(RecordingPlugin {
            available: false,
            owns: true,
            has_status: true,
            has_close: true,
            tag: "the-owner",
            ..RecordingPlugin::default()
        }));

        let status = handle_inspector_action(
            InspectorAction::Status,
            &request_for("9999aaaa"),
            &deps,
            None,
        )
        .await
        .expect("owner answers");
        assert_eq!(
            text_of(&status),
            "plugin status:the-owner",
            "the OWNER answers, not merely the first plugin in the list"
        );

        let close = handle_inspector_action(
            InspectorAction::Close,
            &request_for("9999aaaa"),
            &deps,
            None,
        )
        .await
        .expect("owner answers");
        assert_eq!(
            text_of(&close),
            "plugin close:the-owner",
            "the OWNER answers, not merely the first plugin in the list"
        );
    }

    /// The `actions.ts:141-147` arms: an owner with no `status`/`close` method. Both ARE errors.
    /// Unreachable through the two built-in backends (herdr supplies both methods, ghostty never
    /// owns), so this drives the contract's `Option<..>` return directly.
    #[tokio::test]
    async fn an_owner_without_the_method_reports_upstreams_unsupported_sentence() {
        let fixture = fixture();
        let mut deps = fixture.deps();
        fixture.write_run("aaaa9999", 1);
        deps.plugins.push(Box::new(RecordingPlugin {
            owns: true,
            has_status: false,
            has_close: false,
            ..RecordingPlugin::default()
        }));

        let err = handle_inspector_action(
            InspectorAction::Status,
            &request_for("aaaa9999"),
            &deps,
            None,
        )
        .await
        .expect_err("unsupported is an error");
        assert_eq!(
            err.message,
            "Inspector plugin 'recorder' does not support status for async run aaaa9999."
        );

        let err = handle_inspector_action(
            InspectorAction::Close,
            &request_for("aaaa9999"),
            &deps,
            None,
        )
        .await
        .expect_err("unsupported is an error");
        assert_eq!(
            err.message,
            "Inspector plugin 'recorder' does not support close for async run aaaa9999."
        );
    }

    /// A target that does not resolve is an error for EVERY verb, including `inspector.command`
    /// (`actions.ts:121-122` runs before the `:130` early return).
    #[tokio::test]
    async fn an_unresolvable_target_is_an_error_for_every_verb() {
        let fixture = fixture();
        let deps = fixture.deps();
        for action in [
            InspectorAction::Command,
            InspectorAction::Open,
            InspectorAction::Status,
            InspectorAction::Close,
        ] {
            let err = handle_inspector_action(action, &request_for("nope"), &deps, None)
                .await
                .expect_err("unresolvable");
            assert_eq!(err.message, "No subagent run found for 'nope'.");
        }
    }

    // ---------------------------------------------------------------------------------------
    // SUBA-201 — open and close are serialized per target (pi `5096985c` / #2728)
    // ---------------------------------------------------------------------------------------

    /// pi's `bindingProvider` (`test/unit/inspector-registration.test.ts` @5096985c): "A provider
    /// that reuses a saved binding and takes a moment between reading and writing it, like a real
    /// pane split." The moment is a gate the test releases, and `entered` counts how many opens
    /// have READ the binding — the signal that a second open got past the dispatcher.
    #[derive(Clone)]
    struct BindingHost {
        bindings: std::sync::Arc<Mutex<std::collections::BTreeMap<String, String>>>,
        panes: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        entered: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        entered_signal: std::sync::Arc<tokio::sync::Notify>,
        gate: std::sync::Arc<tokio::sync::Semaphore>,
    }

    impl BindingHost {
        fn new() -> Self {
            Self {
                bindings: std::sync::Arc::default(),
                panes: std::sync::Arc::default(),
                entered: std::sync::Arc::default(),
                entered_signal: std::sync::Arc::default(),
                gate: std::sync::Arc::new(tokio::sync::Semaphore::new(0)),
            }
        }

        fn key(ctx: &InspectorContext) -> String {
            ctx.target
                .child_index
                .map_or_else(|| "root".to_string(), |index| index.to_string())
        }

        fn entered(&self) -> usize {
            self.entered.load(std::sync::atomic::Ordering::SeqCst)
        }

        /// Wait (bounded) until `count` opens have entered the plugin.
        async fn wait_entered(&self, count: usize) {
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                while self.entered() < count {
                    self.entered_signal.notified().await;
                }
            })
            .await
            .expect("the open must reach the plugin");
        }
    }

    #[async_trait::async_trait]
    impl InspectorPlugin for BindingHost {
        fn name(&self) -> &'static str {
            "binding-host"
        }

        async fn available(&self, _ctx: &InspectorContext) -> bool {
            true
        }

        fn owns(&self, ctx: &InspectorContext) -> bool {
            self.bindings
                .lock()
                .is_ok_and(|bindings| bindings.contains_key(&Self::key(ctx)))
        }

        async fn open(
            &self,
            ctx: &InspectorContext,
            _launch: &InspectorLaunch,
            _params: &InspectorParams,
        ) -> Result<ToolResult, ToolError> {
            let key = Self::key(ctx);
            let existing = self.bindings.lock().expect("bindings").get(&key).cloned();
            self.entered
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.entered_signal.notify_one();
            self.gate.acquire().await.expect("gate").forget();
            if let Some(pane) = existing {
                return Ok(management_text(format!("reused {pane}")));
            }
            let pane = format!(
                "pane-{}",
                self.panes.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
            );
            self.bindings
                .lock()
                .expect("bindings")
                .insert(key, pane.clone());
            Ok(management_text(format!("opened {pane}")))
        }

        async fn status(&self, _ctx: &InspectorContext) -> Option<Result<ToolResult, ToolError>> {
            Some(Ok(management_text("status")))
        }

        async fn close(&self, ctx: &InspectorContext) -> Option<Result<ToolResult, ToolError>> {
            self.bindings
                .lock()
                .expect("bindings")
                .remove(&Self::key(ctx));
            Some(Ok(management_text("closed")))
        }
    }

    fn child_request(id: &str, index: Option<i64>) -> InspectorRequest {
        InspectorRequest {
            id: Some(id.to_string()),
            index,
            ..InspectorRequest::default()
        }
    }

    /// How long a "did it get past the lock?" probe waits. The ONLY flakiness these probes admit is
    /// a FALSE PASS at the unlocked HEAD (a starved runtime not getting the racing call into the
    /// plugin within this window); after the fix the racing call cannot get there at all.
    const NOT_ENTERED_WITHIN: std::time::Duration = std::time::Duration::from_millis(300);

    /// SUBA-201 row: "Two concurrent `inspector.open` calls on one target open one pane". pi's
    /// "runs concurrent opens of one target one at a time and opens one pane, without holding up
    /// other targets". The first open is parked inside the plugin; the second open of the SAME
    /// target must not enter the plugin while it is, and an open of ANOTHER child must. Without
    /// the lease both same-target opens read "no binding" and each creates a pane.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_opens_of_one_target_open_one_pane() {
        let fixture = fixture();
        let mut deps = fixture.deps();
        fixture.write_run("201aaaa1", 2);
        let host = BindingHost::new();
        deps.plugins.push(Box::new(host.clone()));
        let deps = std::sync::Arc::new(deps);
        let open = |index: Option<i64>| {
            let deps = std::sync::Arc::clone(&deps);
            tokio::spawn(async move {
                handle_inspector_action(
                    InspectorAction::Open,
                    &child_request("201aaaa1", index),
                    &deps,
                    None,
                )
                .await
            })
        };

        let first = open(Some(0));
        host.wait_entered(1).await;
        let second = open(Some(0));
        tokio::time::sleep(NOT_ENTERED_WITHIN).await;
        assert_eq!(
            host.entered(),
            1,
            "a second open of the same target entered the plugin while the first held it"
        );
        // Another child is another lock: it must not wait behind child 0.
        let other = open(Some(1));
        host.wait_entered(2).await;

        host.gate.add_permits(3);
        let first = text_of(&first.await.expect("join").expect("first open"));
        let second = text_of(&second.await.expect("join").expect("second open"));
        let other = text_of(&other.await.expect("join").expect("other open"));
        // The two targets' opens are released together, so WHICH of them numbers its pane first
        // is a race; compare per target, never the pane numbers across targets.
        let bindings = host.bindings.lock().expect("bindings").clone();
        let child0 = bindings.get("0").cloned().expect("child 0 bound");
        let child1 = bindings.get("1").cloned().expect("child 1 bound");
        assert_ne!(child0, child1);
        assert_eq!(first, format!("opened {child0}"));
        assert_eq!(second, format!("reused {child0}"));
        assert_eq!(other, format!("opened {child1}"));
        assert_eq!(
            host.panes.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "one pane per target"
        );
    }

    /// SUBA-201 row: "a racing close finds the binding the open wrote". pi's "waits for an
    /// in-flight open before closing the same target". Unlocked, the close runs while the open is
    /// still splitting, finds no owner, answers "No inspector plugin owns this binding" — and the
    /// pane the open then writes is orphaned.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_racing_close_finds_the_binding_the_open_wrote() {
        let fixture = fixture();
        let mut deps = fixture.deps();
        fixture.write_run("201bbbb1", 1);
        let host = BindingHost::new();
        deps.plugins.push(Box::new(host.clone()));
        let deps = std::sync::Arc::new(deps);

        let opening = {
            let deps = std::sync::Arc::clone(&deps);
            tokio::spawn(async move {
                handle_inspector_action(
                    InspectorAction::Open,
                    &request_for("201bbbb1"),
                    &deps,
                    None,
                )
                .await
            })
        };
        host.wait_entered(1).await;
        let mut closing = {
            let deps = std::sync::Arc::clone(&deps);
            tokio::spawn(async move {
                handle_inspector_action(
                    InspectorAction::Close,
                    &request_for("201bbbb1"),
                    &deps,
                    None,
                )
                .await
            })
        };
        let early = tokio::time::timeout(NOT_ENTERED_WITHIN, &mut closing).await;
        assert!(
            early.is_err(),
            "the close must wait for the in-flight open, it answered: {:?}",
            early.map(|joined| joined.map(|result| result.map(|ok| text_of(&ok))))
        );

        host.gate.add_permits(1);
        assert_eq!(
            text_of(&opening.await.expect("join").expect("open")),
            "opened pane-1"
        );
        let closed = closing.await.expect("join").expect("close");
        assert_eq!(text_of(&closed), "closed", "the plugin's close must answer");
        assert!(
            host.bindings.lock().expect("bindings").is_empty(),
            "the close must remove the binding the open wrote"
        );
    }

    /// Hold the target's lock the way ANOTHER process would: a raw `flock` on pi's
    /// `<realpath asyncDir>/inspector-<index|root>.lock`. `std::fs::File::lock` is `flock(2)` on
    /// unix, the lock `FileLock`'s layer 2 takes.
    fn hold_inspector_lock(run_dir: &Path, slot: &str) -> std::fs::File {
        let physical = std::fs::canonicalize(run_dir).expect("canonical run dir");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(physical.join(format!("inspector-{slot}.lock")))
            .expect("open lock file");
        file.lock().expect("flock");
        file
    }

    /// SUBA-201 row: "an aborted waiter returns the cancellation sentence" — pi's exact sentence,
    /// `isError`, and the plugin's `open` never called. Unlocked, the open goes straight to the
    /// plugin and answers "opened".
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_aborted_waiter_returns_the_cancellation_sentence() {
        let fixture = fixture();
        let mut deps = fixture.deps();
        let dir = fixture.write_run("201cccc1", 1);
        let calls: CallLog = CallLog::default();
        deps.plugins.push(Box::new(RecordingPlugin {
            available: true,
            calls: std::sync::Arc::clone(&calls),
            ..RecordingPlugin::default()
        }));
        let deps = std::sync::Arc::new(deps);
        let _held = hold_inspector_lock(&dir, "root");

        let cancel = CancelToken::new();
        let mut waiting = {
            let deps = std::sync::Arc::clone(&deps);
            let cancel = cancel.clone();
            tokio::spawn(async move {
                handle_inspector_action(
                    InspectorAction::Open,
                    &request_for("201cccc1"),
                    &deps,
                    Some(&cancel),
                )
                .await
            })
        };
        let early = tokio::time::timeout(NOT_ENTERED_WITHIN, &mut waiting).await;
        assert!(
            early.is_err(),
            "the open must wait for the held lock, it answered: {:?}",
            early.map(|joined| joined.map(|result| result.map(|ok| text_of(&ok))))
        );
        cancel.cancel();
        let err = tokio::time::timeout(std::time::Duration::from_secs(10), waiting)
            .await
            .expect("a cancel ends the wait")
            .expect("join")
            .expect_err("a cancelled waiter is an error");
        assert_eq!(
            err.message,
            "Inspector inspector.open for async run 201cccc1 was cancelled while waiting for another inspector open or close to finish."
        );
        assert!(
            !calls
                .lock()
                .expect("calls")
                .iter()
                .any(|call| call.starts_with("open:")),
            "a cancelled waiter must never reach the plugin's open"
        );
    }

    /// A caller already cancelled never acts, even on an UNCONTENDED target: the cancellation
    /// sentence, and the plugin's `open` never called. Also pins the action name and the
    /// ` child {index}` label on the close path.
    ///
    /// What answers here is `FileLock::acquire`'s own already-cancelled pre-check (layer 1's
    /// `is_cancelled()` before any wait), mapped to pi's sentence — NOT the post-acquire re-check
    /// in `handle_inspector_action`. That re-check is pi's "the wait may end with the lease in the
    /// same tick the caller cancelled" guard: it only matters for a cancel landing while a layer-2
    /// attempt is in flight, a window no test can open deterministically without a seam inside
    /// `cyrup_config::lock`. Mutation-checked: deleting it leaves this suite green, so it is
    /// covered by review against pi `actions.ts:158-159` @ad11b7ab, not by a test.
    #[tokio::test]
    async fn a_pre_cancelled_caller_never_reaches_the_plugin() {
        let fixture = fixture();
        let mut deps = fixture.deps();
        fixture.write_run("201dddd1", 2);
        let calls: CallLog = CallLog::default();
        deps.plugins.push(Box::new(RecordingPlugin {
            available: true,
            owns: true,
            has_close: true,
            calls: std::sync::Arc::clone(&calls),
            tag: "pre",
            ..RecordingPlugin::default()
        }));
        let cancel = CancelToken::new();
        cancel.cancel();

        let err = handle_inspector_action(
            InspectorAction::Close,
            &child_request("201dddd1", Some(1)),
            &deps,
            Some(&cancel),
        )
        .await
        .expect_err("a cancelled close is an error");
        assert_eq!(
            err.message,
            "Inspector inspector.close for async run 201dddd1 child 1 was cancelled while waiting for another inspector open or close to finish."
        );
        let err = handle_inspector_action(
            InspectorAction::Open,
            &request_for("201dddd1"),
            &deps,
            Some(&cancel),
        )
        .await
        .expect_err("a cancelled open is an error");
        assert_eq!(
            err.message,
            "Inspector inspector.open for async run 201dddd1 was cancelled while waiting for another inspector open or close to finish."
        );
        assert!(
            !calls
                .lock()
                .expect("calls")
                .iter()
                .any(|call| call.starts_with("open:")),
            "a cancelled caller must never reach the plugin's open"
        );

        // `status` and `command` stay unlocked AND unaffected by the token, as upstream.
        let status = handle_inspector_action(
            InspectorAction::Status,
            &request_for("201dddd1"),
            &deps,
            Some(&cancel),
        )
        .await;
        assert!(status.is_err(), "the recorder has no status method");
    }

    /// pi's "refuses to open while another open holds the target past the wait": a held lock and
    /// a short `lease_wait` end in upstream's timeout sentence, `isError`, plugin untouched. And
    /// `inspector.status` is NOT behind the lock.
    #[tokio::test]
    async fn a_held_target_past_the_wait_answers_the_timeout_sentence() {
        let fixture = fixture();
        let mut deps = fixture.deps();
        deps.lease_wait = std::time::Duration::from_millis(50);
        let dir = fixture.write_run("201eeee1", 2);
        let calls: CallLog = CallLog::default();
        deps.plugins.push(Box::new(RecordingPlugin {
            available: true,
            owns: true,
            has_status: true,
            calls: std::sync::Arc::clone(&calls),
            tag: "held",
            ..RecordingPlugin::default()
        }));
        let _root = hold_inspector_lock(&dir, "root");
        let _child = hold_inspector_lock(&dir, "1");

        let err =
            handle_inspector_action(InspectorAction::Open, &request_for("201eeee1"), &deps, None)
                .await
                .expect_err("a held target past the wait is an error");
        assert_eq!(
            err.message,
            "Another inspector open or close for async run 201eeee1 is still in progress. Try again when it finishes."
        );
        let err = handle_inspector_action(
            InspectorAction::Close,
            &child_request("201eeee1", Some(1)),
            &deps,
            None,
        )
        .await
        .expect_err("a held child past the wait is an error");
        assert_eq!(
            err.message,
            "Another inspector open or close for async run 201eeee1 child 1 is still in progress. Try again when it finishes."
        );
        assert!(
            !calls
                .lock()
                .expect("calls")
                .iter()
                .any(|call| call.starts_with("open:")),
            "a timed-out waiter must never reach the plugin's open"
        );

        let status = handle_inspector_action(
            InspectorAction::Status,
            &request_for("201eeee1"),
            &deps,
            None,
        )
        .await
        .expect("status is not behind the lock");
        assert_eq!(text_of(&status), "plugin status:held");
    }

    /// pi's "releases the target after a failed open": the guard is the `finally { release() }`,
    /// so an `Err` from the plugin leaves the target free for the next open.
    #[tokio::test]
    async fn a_failed_open_releases_the_target() {
        struct FailingOnce {
            failed: std::sync::atomic::AtomicBool,
        }
        #[async_trait::async_trait]
        impl InspectorPlugin for FailingOnce {
            fn name(&self) -> &'static str {
                "failing-once"
            }
            async fn available(&self, _ctx: &InspectorContext) -> bool {
                true
            }
            fn owns(&self, _ctx: &InspectorContext) -> bool {
                false
            }
            async fn open(
                &self,
                _ctx: &InspectorContext,
                _launch: &InspectorLaunch,
                _params: &InspectorParams,
            ) -> Result<ToolResult, ToolError> {
                if self.failed.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    Ok(management_text("opened"))
                } else {
                    Err(ToolError::new("host failed"))
                }
            }
            async fn status(
                &self,
                _ctx: &InspectorContext,
            ) -> Option<Result<ToolResult, ToolError>> {
                None
            }
            async fn close(
                &self,
                _ctx: &InspectorContext,
            ) -> Option<Result<ToolResult, ToolError>> {
                None
            }
        }

        let fixture = fixture();
        let mut deps = fixture.deps();
        fixture.write_run("201ffff1", 1);
        deps.plugins.push(Box::new(FailingOnce {
            failed: std::sync::atomic::AtomicBool::new(false),
        }));
        let err =
            handle_inspector_action(InspectorAction::Open, &request_for("201ffff1"), &deps, None)
                .await
                .expect_err("the host fails");
        assert_eq!(err.message, "host failed");
        let ok =
            handle_inspector_action(InspectorAction::Open, &request_for("201ffff1"), &deps, None)
                .await
                .expect("the target was released");
        assert_eq!(text_of(&ok), "opened");
    }
}
