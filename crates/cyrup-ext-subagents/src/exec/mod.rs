//! Foreground single-run execution: prompt/argv construction, NDJSON consumption, final-output
//! extraction, acceptance-gate evaluation, completion-mutation guard, model-fallback retry
//! ladder (func-SA §5.2; arch-SA §6.3).
//!
//! This is the integration module: it owns `run_sync`/`RunOptions`/`AgentConfig`/`SingleResult`
//! (arch-SA §3.4) and `plan_batch` (arch-SA §6.6's eager whole-batch fork-context resolution),
//! wiring together every sibling module in this subtree:
//!
//! - [`ndjson`] — the `SubagentEvent` tagged union and `consume_stdout`, the sole NDJSON parser
//!   this module folds progress/usage state from (R-SA-026/057/058).
//! - [`output`] — final-output extraction, file-only output-path handoff, UTF-8-safe truncation
//!   (R-SA-024/025/029/031/042).
//! - [`structured`] — structured-output extraction from the child's event stream + parent-side
//!   JSON-Schema re-validation via the `jsonschema` crate (R-SA-030).
//! - [`completion_guard`] — implementation-expecting classification + mutating-tool-call scan
//!   (R-SA-034).
//! - [`fallback`] — the model-fallback ladder-construction/retry-classification/usage-aggregation
//!   algorithms (R-SA-035..041/044); this module supplies the `AttemptRunner` implementation that
//!   actually spawns a real child OS process per attempt.
//! - [`acceptance`] — the acceptance-provenance ledger: contract injection, gate evaluation, and
//!   REAL `verify[]` subprocess execution (R-SA-023/030/032/033).
//! - [`crate::fork_context::ForkContextResolver`] — [`plan_batch`] resolves every batch step's
//!   fork-context up front, before any child process for that batch is spawned (R-SA-137, arch-SA
//!   §6.6's eager-whole-batch-validation rule).
//!
//! # The mandated mechanism, concretely, in this file
//!
//! [`run_sync`]'s per-attempt driver ([`SpawnedChildAttemptRunner`]) spawns a REAL OS subprocess
//! for every model-fallback attempt via [`crate::spawn::SpawnedChild::spawn`] — never an
//! in-process nested agent turn loop, never an in-process event-relay standing in for the child's
//! own execution (func-SA §1.1). Cancellation is threaded as two independent
//! `cyrup_core::CancelToken`s (`RunOptions.cancel` for hard abort, `RunOptions.interrupt` for a
//! soft, per-run interrupt) raced via `tokio::select!` against
//! [`crate::spawn::SpawnedChild::terminate`]'s real SIGINT->SIGTERM->SIGKILL escalation ladder —
//! this module never invents a second, competing cancellation mechanism.

/// The acceptance-provenance ledger: contract injection, gate evaluation, and REAL `verify[]`
/// subprocess execution (R-SA-023/030/032/033; DI-SA-5).
pub mod acceptance;

/// Implementation-expecting classification and mutating-tool-call scan (R-SA-034).
pub mod completion_guard;

/// The model-fallback attempt loop (`build_model_candidates`, `is_retryable_model_failure`,
/// `run_fallback_ladder`) — R-SA-035/036/037/038/039/040/041/044.
pub mod fallback;

/// The NDJSON event-stream parser (`SubagentEvent`, `consume_stdout`) — R-SA-026/057/058.
pub mod ndjson;

/// Final-output extraction (R-SA-029), file-only output-path stat-snapshot handoff
/// (R-SA-024/025/031), and UTF-8-safe output truncation (R-SA-042).
pub mod output;

/// Parent-side structured-output extraction and JSON-Schema re-validation (R-SA-030).
pub mod structured;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Instant;

use cyrup_core::{CancelToken, ModelId, ProviderId, Usage};

use crate::discovery::types::{AgentDefinition, AgentReadScope, OutputMode, SystemPromptMode, ToolRef};
use crate::error::SubagentError;
use crate::exec::acceptance::{
    AcceptanceContract, AcceptanceLedger, CleanCompletionGate, apply_post_hoc_correction,
    evaluate_acceptance, inject_acceptance_contract,
};
use crate::exec::completion_guard::evaluate_completion_mutation_guard;
use crate::exec::fallback::{
    AttemptRunner, AttemptSignal, ModelAttempt, ModelOverride, build_model_candidates,
    run_fallback_ladder,
};
use crate::exec::ndjson::SubagentEvent;
use crate::exec::output::{
    OutputCap, build_output_path_system_prompt_instruction, extract_final_output,
    resolve_output_handoff, snapshot_output_file, truncate_output, validate_file_only_requires_path,
};
use crate::exec::structured::{StructuredOutcome, resolve_structured_output};
use crate::fork_context::{ContextMode, ForkContext, ForkContextResolver};
use crate::spawn::depth::DepthEnvelope;
use crate::spawn::{ChildSpawnSpec, SpawnCommand, SpawnedChild};

/// R-SA-028 (MUST) — bounded recent-output buffer cap: `recent_output` in a live progress
/// snapshot MUST be capped at 50 lines (oldest evicted first) while the run is active.
pub const RECENT_OUTPUT_CAP: usize = 50;

// ================================================================================================
// AgentConfig / RunOptions / SingleResult (arch-SA §3.4)
// ================================================================================================

/// The resolved, execution-ready subset of an [`AgentDefinition`] this module's foreground
/// executor needs (arch-SA §3.4). Deliberately narrower than the full `AgentDefinition` — this
/// type carries only what `run_sync` itself branches on, not discovery/management metadata
/// (`source`, `file_path`, `present_fields`, …) that has no bearing on one execution.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// The agent's local (unqualified) name — feeds [`completion_guard::expects_implementation_mutation`]'s
    /// `agent` classification input and [`acceptance::AcceptanceContract::heuristic_default`].
    pub name: String,
    pub model: Option<ModelId>,
    pub fallback_models: Vec<ModelId>,
    pub system_prompt_mode: SystemPromptMode,
    pub system_prompt_body: String,
    pub tools: Option<Vec<ToolRef>>,
    pub output: Option<crate::discovery::types::OutputSpec>,
    /// `None`/`Some(true)` leaves the completion-mutation guard active (subject to that
    /// subsystem's own read-only-tools short-circuit); `Some(false)` disables it entirely
    /// (R-SA-034).
    pub completion_guard: Option<bool>,
    /// The byte/line truncation budget for this agent's delivered output (R-SA-042). Reuses
    /// [`output::OutputCap`] directly (the type `exec/output.rs` already defines and tests)
    /// rather than inventing a second, competing cap type — architecture.md §3.4's illustrative
    /// `AgentConfig::max_output: OutputCap` sketch predates that module's own landing; this field
    /// is the real wiring of that sketch onto the type that actually exists.
    pub max_output: OutputCap,
    /// Effective recursion-depth ceiling this agent declares for ITS OWN children, feeding
    /// [`crate::spawn::depth::next_envelope`]'s tightening-only merge (R-SA-056) — `None` means
    /// "no agent-level tightening; pass the inherited ceiling through unchanged".
    pub max_subagent_depth: Option<u32>,
    /// Depth envelope this process itself resolved at startup ([`crate::spawn::depth::resolve_effective_depth`]),
    /// threaded through so `run_sync` can compute the CHILD's envelope via `next_envelope` before
    /// ever building the spawn env overlay (R-SA-054/055/056).
    pub depth: DepthEnvelope,
}

impl AgentConfig {
    /// Build an [`AgentConfig`] from a fully-resolved [`AgentDefinition`] plus the depth envelope
    /// this process itself resolved. A thin projection, not a re-derivation: every field here is
    /// copied straight off `agent`, never reclassified.
    #[must_use]
    pub fn from_agent_definition(agent: &AgentDefinition, depth: DepthEnvelope) -> Self {
        Self {
            name: agent.local_name.clone(),
            model: agent.model.clone(),
            fallback_models: agent.fallback_models.clone(),
            system_prompt_mode: agent.system_prompt_mode,
            system_prompt_body: agent.system_prompt_body.clone(),
            tools: agent.tools.clone(),
            output: agent.output.clone(),
            completion_guard: agent.completion_guard,
            max_output: OutputCap::default(),
            max_subagent_depth: agent.max_subagent_depth,
            depth,
        }
    }
}

/// R-SA-041: distinguishes "the caller didn't specify a model override" from "explicitly use this
/// model" — re-exported here under `exec`'s own namespace so `RunOptions::model_override` has a
/// stable, documented home even though the type itself is [`fallback`]'s (one canonical owner,
/// consumed by this module rather than redefined).
pub use crate::exec::fallback::ModelOverride as RunModelOverride;

/// Every per-call parameter [`run_sync`] needs beyond the resolved [`AgentConfig`] and task text
/// (arch-SA §3.4). Threaded through unmodified across every model-fallback attempt for this one
/// task (R-SA-035's deadline-monotonicity requirement, restated at the type level: nothing in
/// this struct is ever recomputed mid-ladder).
#[derive(Debug, Clone)]
pub struct RunOptions {
    pub cwd: PathBuf,
    /// Monotonically shrinking deadline, computed ONCE at the start of the outer (chain/parallel/
    /// single) call and passed through unmodified to every subsequent attempt (R-SA-035).
    /// `None` means no wall-clock timeout at all.
    pub deadline_at: Option<Instant>,
    pub output_path: Option<PathBuf>,
    pub output_mode: OutputMode,
    pub structured_output_schema: Option<serde_json::Value>,
    /// R-SA-041's inherit sentinel — `Inherit` MUST NOT itself fall through to a global
    /// cross-session default inside [`run_sync`]; a caller wanting that global-default behavior
    /// resolves it explicitly before constructing this struct.
    pub model_override: ModelOverride,
    pub preferred_provider: Option<ProviderId>,
    pub available_models: Vec<ModelId>,
    /// Hard-abort cancellation, raced independently of `interrupt` (arch-SA §5.1).
    pub cancel: CancelToken,
    /// Soft, per-run interrupt — distinct downstream consequences from `cancel`/timeout
    /// (R-SA-084 vs. R-SA-036); this module treats an interrupt firing identically to a timeout
    /// for ladder-termination purposes (both stop the fallback ladder outright) but records it
    /// under its own `interrupted` flag on [`SingleResult`] rather than conflating it with
    /// `timed_out`.
    pub interrupt: CancelToken,
    pub share: Option<bool>,
    pub session_dir: Option<PathBuf>,
    /// R-SA-043: when `Some(false)`, even a still-running progress snapshot omits per-turn
    /// detail; `None`/`Some(true)` is the default fuller shape. `run_sync`'s own return value
    /// (always a terminal, compacted [`SingleResult`]) is unaffected either way — see
    /// [`SingleResult`]'s own doc comment for exactly what compaction means for a *completed*
    /// result vs. a live callback snapshot.
    pub include_progress: Option<bool>,
    pub agent_scope: Option<AgentReadScope>,
    /// Explicit acceptance-contract override for this task (func-SA §4.2 `acceptance`); `None`
    /// defers to [`AcceptanceContract::heuristic_default`] (R-SA-023).
    pub acceptance: Option<AcceptanceContract>,
    /// Resolved fork-context for this task, if any — normally produced by [`plan_batch`] ahead of
    /// time (R-SA-137) and threaded straight through here; `Fresh` (the default) when this task
    /// runs with no inherited session state.
    pub fork_context: ForkContext,
}

/// One row of a completed run's per-attempt history, re-exported under `exec`'s own namespace so
/// callers of `run_sync` never need to import `exec::fallback` directly for this shape (one
/// canonical owner: [`fallback::ModelAttempt`]).
pub use crate::exec::fallback::ModelAttempt as RunModelAttempt;

/// The full, terminal outcome of one `run_sync` call (arch-SA §3.4). This is always the
/// **compacted** (R-SA-043) shape: no raw per-turn messages, no live `progress` object — only the
/// summarized fields below. A still-running progress snapshot used for live update callbacks
/// (`RunOptions.include_progress`-gated, §4.3) is a materially different, richer shape this crate
/// does not construct in this module (that belongs to `tui/` once it exists); `SingleResult` is
/// exclusively the terminal return value.
///
/// `PartialEq`/`Serialize`/`Deserialize` are derived (beyond the original `Debug, Clone`) because
/// `background::ResultFile` (func-SA §4.5, R-SA-077/166) embeds `Vec<SingleResult>` directly and
/// must round-trip it through `status.json`/the terminal result file exactly like every other
/// field on that struct — a bare `Debug, Clone` shape cannot satisfy `write_atomic_json`'s
/// `T: Serialize` bound (R-SA-076).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SingleResult {
    pub agent: String,
    pub task: String,
    pub exit_code: i32,
    pub usage: Usage,
    pub model: Option<ModelId>,
    pub attempted_models: Vec<ModelId>,
    pub model_attempts: Vec<ModelAttempt>,
    pub final_output: Option<String>,
    pub structured_output: Option<serde_json::Value>,
    pub acceptance: Option<AcceptanceLedger>,
    /// R-SA-037: an intercom-style blocking detach signal was observed — bypasses acceptance,
    /// completion-guard, and output truncation entirely. Always `false` in every build of this
    /// crate today: see [`crate::exec::fallback::AttemptSignal::detached`]'s doc comment for the
    /// full, current explanation of why no live trigger for this exists yet and exactly what
    /// future work (the R-SA-119/120 intercom wiring) would set it.
    pub detached: bool,
    /// A soft interrupt was observed (`RunOptions.interrupt` fired) — like a timeout, this
    /// terminates the fallback ladder outright without advancing, but is recorded under its own
    /// flag rather than folded into `timed_out` (R-SA-084 vs. R-SA-036 have distinct downstream
    /// consequences a caller may want to distinguish).
    pub interrupted: bool,
    pub timed_out: bool,
    pub error: Option<String>,
    /// Summarized tool-call names actually observed across the winning attempt's transcript —
    /// R-SA-043's "only summarized `tool_calls`" compaction requirement. Never the raw per-turn
    /// message list.
    pub tool_calls: Vec<String>,
    /// Whether [`output::truncate_output`] actually cut the delivered `final_output` (R-SA-042).
    pub output_truncated: bool,
}

// ================================================================================================
// AgentProgress: the live per-attempt fold (R-SA-027/028)
// ================================================================================================

/// The live, in-memory progress state one attempt accumulates as its child's NDJSON stdout is
/// consumed (R-SA-027/028). This is the "still-running" shape architecture.md §4.3/R-SA-043
/// contrasts with [`SingleResult`]'s own compacted, terminal shape — never returned to
/// `run_sync`'s own caller directly; folded down into `SingleResult`'s summarized fields once the
/// attempt (and then the whole fallback ladder) settles.
#[derive(Debug, Clone, Default)]
pub struct AgentProgress {
    /// Running additive [`Usage`] total for THIS attempt alone (cross-attempt aggregation, which
    /// is additive across the whole ladder including failed attempts, is
    /// [`fallback::run_fallback_ladder`]'s own separate concern, R-SA-040) — every `MessageEnd`
    /// event's `usage` is folded in here as it is observed (R-SA-027).
    pub usage: Usage,
    /// Number of `ToolExecutionStart` events observed so far this attempt (R-SA-027).
    pub tool_count: u32,
    /// The most recently started tool's name, if any tool call has started and none more recent
    /// has superseded it (R-SA-027's "set `current_tool`").
    pub current_tool: Option<String>,
    /// Bounded ring buffer of recent raw NDJSON lines, oldest evicted first once
    /// [`RECENT_OUTPUT_CAP`] is exceeded (R-SA-028). Kept as raw text (not parsed events) since
    /// R-SA-028's own text speaks of "recent output" as a rendering/log concern, not a
    /// re-parseable event queue.
    pub recent_output: VecDeque<String>,
    /// Every `MessageEnd` event observed this attempt, in chronological (parse) order — the exact
    /// input [`output::extract_final_output`] (R-SA-029) needs, and what
    /// [`completion_guard::has_mutation_tool_call`]/[`evaluate_completion_mutation_guard`]
    /// (R-SA-034) scans alongside `tool_events` below.
    pub message_end_events: Vec<SubagentEvent>,
    /// Every `ToolExecutionEnd` event observed this attempt, in chronological order — feeds
    /// [`completion_guard::has_mutation_tool_call`] (R-SA-034) and the summarized `tool_calls`
    /// list [`SingleResult`] carries (R-SA-043).
    pub tool_end_events: Vec<SubagentEvent>,
    /// The full parsed transcript of every recognized event this attempt observed, in
    /// chronological order — feeds [`structured::resolve_structured_output`] (R-SA-030), which
    /// needs more than the two narrower vectors above; `run_sync` also reads this directly for
    /// that R-SA-030 wiring, alongside `message_end_events`/`tool_end_events` for its own
    /// R-SA-029/034 wiring.
    pub all_events: Vec<SubagentEvent>,
}

impl AgentProgress {
    /// Fold one parsed [`SubagentEvent`] into this progress state (R-SA-027). Every `MessageEnd`
    /// event's usage is accumulated additively (never last-wins — mirrors
    /// [`fallback::add_usage`]'s own contract, restated here at the per-attempt granularity); every
    /// `ToolExecutionStart` increments `tool_count` and sets `current_tool`.
    pub fn record_event(&mut self, event: SubagentEvent) {
        if let Some(usage) = event.assistant_usage() {
            crate::exec::fallback::add_usage(&mut self.usage, &usage);
        }
        match &event {
            SubagentEvent::ToolExecutionStart { tool_name, .. } => {
                self.tool_count += 1;
                self.current_tool = Some(tool_name.clone());
            }
            SubagentEvent::MessageEnd { .. } => {
                self.message_end_events.push(event.clone());
            }
            SubagentEvent::ToolExecutionEnd { .. } => {
                self.tool_end_events.push(event.clone());
            }
            _ => {}
        }
        self.all_events.push(event);
    }

    /// Push one raw NDJSON line into the bounded `recent_output` ring buffer (R-SA-028): capped
    /// at [`RECENT_OUTPUT_CAP`] lines, oldest evicted first.
    pub fn record_raw_line(&mut self, line: &str) {
        if self.recent_output.len() >= RECENT_OUTPUT_CAP {
            self.recent_output.pop_front();
        }
        self.recent_output.push_back(line.to_string());
    }

    /// Summarized tool-call names observed this attempt (R-SA-043's compaction target), in
    /// chronological order, deduplicated only in the trivial sense of "one entry per
    /// `ToolExecutionEnd` event" (repeats of the same tool name across multiple calls are
    /// preserved, matching how many real tool calls actually happened).
    #[must_use]
    pub fn summarized_tool_calls(&self) -> Vec<String> {
        self.tool_end_events
            .iter()
            .filter_map(|event| match event {
                SubagentEvent::ToolExecutionEnd { tool_name, .. } => Some(tool_name.clone()),
                _ => None,
            })
            .collect()
    }
}

// ================================================================================================
// SubagentSpawner: the seam production spawning goes through (mirrors AttemptRunner's own
// production-vs-test seam, one level down at the real-subprocess boundary)
// ================================================================================================

/// Everything one attempt's spawn needs beyond what [`AgentConfig`]/[`RunOptions`] already carry —
/// factored out so [`SpawnedChildAttemptRunner`] can build a [`ChildSpawnSpec`] without repeating
/// argv/env assembly inline in `run_attempt` itself.
struct AttemptSpawnPlan {
    spec: ChildSpawnSpec,
}

/// Build the argv + env overlay for one attempt against `model` (R-SA-024/047/048/054).
///
/// Argv (in order, per R-SA-024/047's base contract): `--print`, `--mode`, `json`, `--model`,
/// `<model>`, an optional `--tools <comma-list>` (present only when `agent.tools` is `Some`, per
/// R-SA-024's "an optional tools-allowlist flag (present only if the agent declares one)"),  an
/// optional `--session <path>` (when `opts.fork_context` resolved a session file path), then the
/// task prompt last (via [`ChildSpawnSpec::resolve_task_arg`], R-SA-047's `@<tempfile>` overflow
/// rule).
///
/// System prompt steering for `output_mode == FileOnly` (R-SA-024's system-prompt half) is
/// applied to `task` BEFORE this function is called — see [`build_task_text`] — since this crate's
/// spawn contract carries the system prompt as part of the composed task/system text handed to
/// the child rather than a separate `--system-prompt` argv flag for subagent runs specifically
/// (mirroring `agent.system_prompt_mode`'s own task-text-composition role, R-SA-024's own
/// wording: "steered at generation time... not merely conveyed via argv").
///
/// # Errors
///
/// Propagates [`ChildSpawnSpec::resolve_task_arg`]'s error (temp-file creation failure for an
/// over-threshold task).
fn build_attempt_spawn_plan(
    agent: &AgentConfig,
    model: &ModelId,
    task_text: &str,
    opts: &RunOptions,
    depth: DepthEnvelope,
    temp_dir: &std::path::Path,
) -> Result<AttemptSpawnPlan, SubagentError> {
    let command = crate::spawn::resolve_spawn_command();

    let mut args: Vec<String> = vec![
        "--print".to_string(),
        "--mode".to_string(),
        "json".to_string(),
        "--model".to_string(),
        model.as_str().to_string(),
    ];

    // R-SA-024: an optional tools-allowlist flag, present ONLY if the agent declares one.
    if let Some(tools) = &agent.tools {
        let allowlist = tools
            .iter()
            .map(tool_ref_cli_name)
            .collect::<Vec<_>>()
            .join(",");
        args.push("--tools".to_string());
        args.push(allowlist);
    }

    if let Some(session_path) = &opts.fork_context.session_file_path {
        args.push("--session".to_string());
        args.push(session_path.display().to_string());
    }

    let (task_arg, temp_file) = ChildSpawnSpec::resolve_task_arg(task_text, temp_dir)?;

    let mut env_overlay = crate::spawn::depth::to_env_overlay(&depth);
    // Model-inherit sentinel (R-SA-041) never leaks a global default into the child's own
    // resolution beyond what `--model` above already pins explicitly for this attempt.
    env_overlay.insert("CYRUP_SUBAGENT_RUN".to_string(), "1".to_string());

    let cwd = opts.cwd.clone();

    Ok(AttemptSpawnPlan {
        spec: ChildSpawnSpec {
            command: SpawnCommand {
                binary: command.binary,
                base_args: command.base_args,
            },
            args,
            task_arg,
            env_overlay,
            cwd,
            temp_files: temp_file.into_iter().collect(),
        },
    })
}

/// Render one [`ToolRef`] as the literal string `--tools`'s comma-list expects — the frontmatter
/// literal for [`ToolRef::Builtin`], the `mcp:`-prefixed literal (already carried verbatim) for
/// [`ToolRef::Mcp`], and the extension-path literal for [`ToolRef::ExtensionPath`] — i.e. the
/// exact inverse of parsing, never a re-derived or renamed identifier.
fn tool_ref_cli_name(tool_ref: &ToolRef) -> &str {
    match tool_ref {
        ToolRef::Builtin(name) | ToolRef::Mcp(name) | ToolRef::ExtensionPath(name) => name,
    }
}

/// Compose the final task text handed to the child: acceptance-contract injection (R-SA-023) then
/// output-path system-prompt steering (R-SA-024's file-only half) then, when
/// `agent.system_prompt_mode == Append`, the agent's own system-prompt body appended after both
/// (mirroring [`crate::discovery::types::SystemPromptMode::Append`]'s documented role: this
/// agent's own frontmatter prose combines with orchestrator-injected scaffolding rather than
/// replacing it). `Replace` mode leaves the agent's own `system_prompt_body` for the spawned
/// child's own system-prompt resolution to apply independently (out of this module's scope — this
/// function only ever touches the TASK text, never the child's actual `--system-prompt`
/// invocation, which this crate does not set at all, letting the child's own agent-persona
/// resolution own that).
fn build_task_text(agent: &AgentConfig, task: &str, opts: &RunOptions, contract: &AcceptanceContract) -> String {
    let with_acceptance = inject_acceptance_contract(task, contract);
    let with_output_path = match opts.output_mode {
        OutputMode::FileOnly => {
            let path = opts.output_path.as_deref();
            match build_output_path_system_prompt_instruction(path) {
                Some(instruction) => format!("{with_acceptance}\n\n{instruction}"),
                None => with_acceptance,
            }
        }
        OutputMode::FileAndInline | OutputMode::Inline => with_acceptance,
    };
    match agent.system_prompt_mode {
        SystemPromptMode::Append if !agent.system_prompt_body.is_empty() => {
            format!("{with_output_path}\n\n{}", agent.system_prompt_body)
        }
        SystemPromptMode::Append | SystemPromptMode::Replace => with_output_path,
    }
}

/// The production [`AttemptRunner`] implementation: spawns a REAL child OS process per
/// model-fallback attempt via [`SpawnedChild::spawn`] (func-SA §1.1's mandated mechanism),
/// consumes its NDJSON stdout through [`ndjson::consume_stdout`], folds R-SA-027/028 progress,
/// and races the whole attempt against `opts.cancel`/`opts.interrupt`/`opts.deadline_at` before
/// returning an [`AttemptSignal`] plus this attempt's own richer [`AttemptRecord`] payload.
struct SpawnedChildAttemptRunner<'a> {
    agent: &'a AgentConfig,
    task: &'a str,
    opts: &'a RunOptions,
    contract: &'a AcceptanceContract,
    /// Scratch directory for `@<tempfile>` task-text overflow (R-SA-047) and the per-attempt
    /// `.jsonl` tee artifact (R-SA-058).
    scratch_dir: PathBuf,
    attempt_index: u32,
}

/// The richer per-attempt payload [`SpawnedChildAttemptRunner::run_attempt`] returns alongside its
/// [`AttemptSignal`] — everything `run_sync`'s completion path (structured-output validation,
/// completion guard, acceptance evaluation, R-SA-033's ordering) needs from the WINNING attempt,
/// without `fallback::run_fallback_ladder` itself needing to know this shape at all (it only ever
/// touches [`AttemptSignal`]).
struct AttemptRecord {
    progress: AgentProgress,
    final_output: Option<String>,
}

#[async_trait::async_trait]
impl AttemptRunner for SpawnedChildAttemptRunner<'_> {
    type Attempt = AttemptRecord;

    async fn run_attempt(
        &mut self,
        model: &ModelId,
        attempt_note: Option<&str>,
    ) -> (AttemptSignal, Self::Attempt) {
        let mut progress = AgentProgress::default();
        if let Some(note) = attempt_note {
            progress.record_raw_line(note);
        }

        let task_text = build_task_text(self.agent, self.task, self.opts, self.contract);

        let plan = match build_attempt_spawn_plan(
            self.agent,
            model,
            &task_text,
            self.opts,
            self.agent.depth,
            &self.scratch_dir,
        ) {
            Ok(plan) => plan,
            Err(err) => {
                return (
                    AttemptSignal {
                        success: false,
                        exit_code: None,
                        error: Some(err.to_string()),
                        usage: Usage::default(),
                        timed_out: false,
                        detached: false,
                    },
                    AttemptRecord {
                        progress,
                        final_output: None,
                    },
                );
            }
        };

        let jsonl_path = self
            .scratch_dir
            .join(format!("attempt-{}.jsonl", self.attempt_index));
        self.attempt_index += 1;

        let child = match SpawnedChild::spawn(plan.spec, &jsonl_path).await {
            Ok(child) => child,
            Err(err) => {
                return (
                    AttemptSignal {
                        success: false,
                        exit_code: None,
                        error: Some(err.to_string()),
                        usage: Usage::default(),
                        timed_out: false,
                        detached: false,
                    },
                    AttemptRecord {
                        progress,
                        final_output: None,
                    },
                );
            }
        };

        let deadline_sleep = self
            .opts
            .deadline_at
            .map(|instant| tokio::time::sleep_until(tokio::time::Instant::from_std(instant)));
        let (timed_out, interrupted, exit_status) =
            drive_attempt(child, &mut progress, self.opts, deadline_sleep).await;

        let (exit_code, spawn_error) = match exit_status {
            Ok(Some(status)) => (status.code(), None),
            Ok(None) => (None, None), // terminated via signal escalation (timeout/interrupt/cancel)
            Err(err) => (None, Some(err.to_string())),
        };

        let final_output = extract_final_output(&progress.message_end_events);
        let success = !timed_out
            && !interrupted
            && spawn_error.is_none()
            && exit_code == Some(0);

        let error = spawn_error.or_else(|| {
            if success {
                None
            } else if timed_out {
                Some("subagent attempt timed out".to_string())
            } else if interrupted {
                Some("subagent attempt interrupted".to_string())
            } else {
                Some(format!(
                    "subagent attempt exited with code {}",
                    exit_code.map_or_else(|| "unknown".to_string(), |c| c.to_string())
                ))
            }
        });

        (
            AttemptSignal {
                success,
                exit_code,
                error,
                usage: progress.usage.clone(),
                timed_out,
                detached: false, // R-SA-037: no live trigger exists yet in this crate — see
                                 // `AttemptSignal::detached`'s doc comment (exec/fallback.rs) for
                                 // the full explanation and exactly what future work sets this.
            },
            AttemptRecord {
                progress,
                final_output,
            },
        )
    }

    fn snapshot_output_file(&mut self) {
        // R-SA-031: the actual snapshot value is consulted later, in `finalize_result`'s
        // file-only handoff — `run_fallback_ladder` only requires the snapshot to be TAKEN at
        // the correct point (immediately before each fresh spawn), which this no-op satisfies
        // trivially since `run_sync` itself takes the real snapshot once, outside the ladder, and
        // compares it once after the ladder settles (R-SA-031 is a whole-task stat-snapshot
        // heuristic, not a per-attempt one — a task's `output_path` does not change between
        // fallback attempts, so re-snapshotting per attempt would not observe anything new).
    }
}

/// Drive one spawned child to completion, folding every NDJSON line into `progress` (R-SA-027/028)
/// and racing the whole read loop against `opts.cancel`/`opts.interrupt`/an optional deadline
/// timer. Returns `(timed_out, interrupted, wait_result)`.
///
/// On timeout, cancel, or interrupt, the child is driven through
/// [`SpawnedChild::terminate`]'s real signal-escalation ladder (R-SA-036/059) — never a bare
/// `kill()`. `child` is taken by value (never `&mut`): [`SpawnedChild::terminate`]/
/// [`SpawnedChild::finish`] both consume `self` to guarantee temp-file cleanup runs exactly once
/// on every exit path (R-SA-067), so this function's own signature is shaped to always be able to
/// hand `child` off to whichever exit path is taken, with no placeholder/`Default` value ever
/// needed to satisfy a borrow.
async fn drive_attempt(
    mut child: SpawnedChild,
    progress: &mut AgentProgress,
    opts: &RunOptions,
    deadline_sleep: Option<tokio::time::Sleep>,
) -> (bool, bool, std::io::Result<Option<std::process::ExitStatus>>) {
    tokio::pin!(deadline_sleep);
    let cancel = opts.cancel.clone();
    let interrupt = opts.interrupt.clone();

    loop {
        let deadline_arm = async {
            match deadline_sleep.as_mut().as_pin_mut() {
                Some(sleep) => sleep.await,
                None => std::future::pending::<()>().await,
            }
        };

        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                let outcome = child.terminate(&cancel).await;
                return (false, false, outcome.map(|o| Some(o.status)));
            }
            () = interrupt.cancelled() => {
                let outcome = child.terminate(&cancel).await;
                return (false, true, outcome.map(|o| Some(o.status)));
            }
            () = deadline_arm => {
                // R-SA-036: timeout is a SOFT interrupt, not an immediate hard kill — it still
                // walks the full SIGINT->SIGTERM->SIGKILL ladder via `terminate`, exactly like
                // cancel/interrupt above; what makes it a timeout rather than a plain
                // cancellation is the `timed_out: true` flag this function returns, which is
                // what `run_fallback_ladder` (R-SA-036/6.3.2) actually branches on to stop the
                // ladder outright without advancing.
                let outcome = child.terminate(&cancel).await;
                return (true, false, outcome.map(|o| Some(o.status)));
            }
            next = child.next_event() => {
                match next {
                    Some(Ok(line)) => {
                        progress.record_raw_line(&line.raw);
                        // `SpawnedChild::next_event` parses against the spawn boundary's own
                        // narrow `spawn::NdjsonEvent` (progress-bookkeeping fields only, arch-SA
                        // §6.4); this module needs the fuller `exec::ndjson::SubagentEvent` union
                        // (final-output extraction, R-SA-029; completion-guard scanning,
                        // R-SA-034), so the identical raw line is re-parsed here through
                        // `ndjson::parse_line` — both are independent, tolerant views over the
                        // exact same wire bytes (`exec/ndjson.rs`'s own module doc), not a
                        // layering of one on top of the other.
                        if let Some(event) = crate::exec::ndjson::parse_line(&line.raw) {
                            progress.record_event(event);
                        }
                    }
                    Some(Err(_)) | None => {
                        // Stdout EOF (child exited) or a genuine read fault — either way, stop
                        // reading and wait for the real exit status below.
                        break;
                    }
                }
            }
        }
    }

    match child.wait_final_drain().await {
        Ok(Some(status)) => {
            child.finish(); // R-SA-067: success-path temp-file cleanup.
            (false, false, Ok(Some(status)))
        }
        Ok(None) => {
            // The child emitted a final message but did not exit/release stdio within
            // FINAL_DRAIN_TIMEOUT (R-SA-068) — fall back to the real signal-escalation ladder
            // rather than waiting indefinitely.
            let outcome = child.terminate(&cancel).await;
            (false, false, outcome.map(|o| Some(o.status)))
        }
        Err(err) => (false, false, Err(err)),
    }
}

// ================================================================================================
// run_sync: the model-fallback attempt loop, wired end to end (arch-SA §6.3.2)
// ================================================================================================

/// Run one subagent task to completion, synchronously, against `agent`/`opts` (func-SA §5.2;
/// arch-SA §6.3.2).
///
/// # Pipeline (strict order, per R-SA-033's own ordering restated at the top level)
///
/// 0. R-SA-055 (SAFETY-CRITICAL): the recursion-depth guard ([`crate::spawn::depth::is_blocked`]
///    against `agent.depth`) runs FIRST, before anything else in this function — including
///    R-SA-025's own output-mode validation immediately below. `run_sync` is the sole real spawn
///    chokepoint in this crate (every production caller — the foreground tool dispatch, the
///    background runner's step loop, and every chain/parallel/dynamic fan-out child reached via
///    `chain_graph::walk_chain`/`spawn::parallel::run_bounded`'s `SingleStepExecutor` seam —
///    funnels through this one function before ever touching `SpawnedChild::spawn`), so gating
///    here is what makes the depth ceiling actually bind at runtime rather than merely existing as
///    a unit-tested-in-isolation predicate. A blocked attempt returns
///    [`SubagentError::DepthExceeded`]'s message as `SingleResult::error` with `exit_code: 1` and
///    spawns nothing.
/// 1. R-SA-025: file-only output mode requires an output path — fail fast, before any subprocess
///    is spawned, if violated.
/// 2. Resolve the effective acceptance contract (explicit `opts.acceptance`, or
///    [`AcceptanceContract::heuristic_default`], R-SA-023).
/// 3. R-SA-038: build the model-fallback candidate ladder.
/// 4. Drive [`fallback::run_fallback_ladder`] against a [`SpawnedChildAttemptRunner`] — every
///    candidate model gets a FRESH real child OS process (R-SA-039); R-SA-036 (timeout)/R-SA-037
///    (detach) both terminate the ladder outright without advancing, exactly as
///    `run_fallback_ladder` itself already enforces (this module supplies the signal, not the
///    ladder-control logic, which stays [`fallback`]'s sole responsibility).
/// 5. R-SA-030: structured-output extraction + parent-side JSON-Schema re-validation, via
///    [`structured::resolve_structured_output`] (arch-SA §12 item 13's resolved crate choice,
///    `jsonschema`). Only evaluated when the run is otherwise clean (exit 0, not detached/
///    interrupted/timed-out) — mirrors R-SA-032/033's own "don't re-diagnose an already-failed
///    attempt" gate. If `opts.structured_output_schema` is `None`, this step is a no-op
///    (`SingleResult::structured_output` stays `None`). If a schema IS declared: an extracted value
///    that validates populates `SingleResult::structured_output`; an extracted value that fails
///    validation, or no value at all when no plain-text fallback was produced either, forces
///    `exit_code = 1` with a validation-error `error` message — never silently downgraded, per
///    R-SA-030's "MUST also fail the run" text.
/// 6. R-SA-034: completion-mutation guard, via [`completion_guard::evaluate_completion_mutation_guard`].
/// 7. R-SA-032: acceptance-gate evaluation, gated on `exit_code == 0 && !detached && !interrupted
///    && !timed_out` (R-SA-033's own gate condition), via [`acceptance::evaluate_acceptance`].
/// 8. R-SA-033: post-hoc exit-code correction, via [`acceptance::apply_post_hoc_correction`].
/// 9. R-SA-042: UTF-8-safe output truncation, via [`output::truncate_output`].
/// 10. R-SA-043: result compaction — `SingleResult` itself IS the compacted shape (no raw
///     per-turn messages, no live `progress` object); `SingleResult::tool_calls` carries only the
///     summarized tool-name list.
///
/// R-SA-037 (intercom detach bypasses acceptance/completion-guard/truncation entirely) has no
/// live trigger path anywhere in this crate today — [`SpawnedChildAttemptRunner`] never sets
/// `AttemptSignal::detached`, so the `detached` branch below is dead code on every currently
/// reachable path but is retained (not `unreachable!()`'d — this crate forbids `panic!`/
/// `unreachable!` outside tests) so a later phase wiring intercom detach through
/// [`SpawnedChildAttemptRunner`] only needs to flip that one signal, not touch this function's own
/// gating logic at all. See [`crate::exec::fallback::AttemptSignal::detached`]'s doc comment for
/// the full explanation of why this is currently always `false` (no `SubagentEvent` on the wire
/// carries a clarify-block signal, and `tui::intercom`'s `AskLock` is not wired to this crate's
/// spawn/exec path) and exactly what the R-SA-119/120 follow-up work would need to do to set it.
pub async fn run_sync(agent: &AgentConfig, task: &str, opts: &RunOptions) -> SingleResult {
    // Step 0 (R-SA-055, SAFETY-CRITICAL): the recursion-depth guard MUST run before any spawn,
    // discovery, or worktree setup — this is `run_sync`'s very first action, ahead of even
    // R-SA-025's output-mode validation below, because `run_sync` is the sole chokepoint every
    // production spawn path in this crate funnels through (the foreground single-run tool
    // dispatch, the background hop-2 runner's per-step loop, and — via `chain_graph::walk_chain`/
    // `spawn::parallel::run_bounded`'s `SingleStepExecutor` seam — every chain step, parallel
    // fan-out child, and dynamic fan-out child as well). A blocked check returns an error result
    // telling the caller to complete the task directly, per R-SA-055's own text, and — because
    // this check precedes every other line of this function — zero subprocesses are ever spawned
    // for a blocked attempt.
    if crate::spawn::depth::is_blocked(&agent.depth) {
        let err = SubagentError::DepthExceeded {
            current: agent.depth.current_depth,
            max: agent.depth.max_depth,
        };
        return SingleResult {
            agent: agent.name.clone(),
            task: task.to_string(),
            exit_code: 1,
            usage: Usage::default(),
            model: None,
            attempted_models: Vec::new(),
            model_attempts: Vec::new(),
            final_output: None,
            structured_output: None,
            acceptance: None,
            detached: false,
            interrupted: false,
            timed_out: false,
            error: Some(err.to_string()),
            tool_calls: Vec::new(),
            output_truncated: false,
        };
    }

    // Step 1 (R-SA-025): fail fast before any subprocess spawns.
    if let Some(err) = validate_file_only_requires_path(opts.output_mode, opts.output_path.as_deref())
    {
        return SingleResult {
            agent: agent.name.clone(),
            task: task.to_string(),
            exit_code: 1,
            usage: Usage::default(),
            model: None,
            attempted_models: Vec::new(),
            model_attempts: Vec::new(),
            final_output: None,
            structured_output: None,
            acceptance: None,
            detached: false,
            interrupted: false,
            timed_out: false,
            error: Some(err.to_string()),
            tool_calls: Vec::new(),
            output_truncated: false,
        };
    }

    // Step 2 (R-SA-023): resolve the effective acceptance contract.
    let contract = opts
        .acceptance
        .clone()
        .unwrap_or_else(|| AcceptanceContract::heuristic_default(&agent.name, task));

    // Step 3 (R-SA-038).
    let candidates = build_model_candidates(
        &opts.model_override,
        agent.model.as_ref(),
        &agent.fallback_models,
        &opts.available_models,
    );

    if candidates.is_empty() {
        return SingleResult {
            agent: agent.name.clone(),
            task: task.to_string(),
            exit_code: 1,
            usage: Usage::default(),
            model: None,
            attempted_models: Vec::new(),
            model_attempts: Vec::new(),
            final_output: None,
            structured_output: None,
            acceptance: None,
            detached: false,
            interrupted: false,
            timed_out: false,
            error: Some(
                "no candidate model available for this subagent run (empty fallback ladder)"
                    .to_string(),
            ),
            tool_calls: Vec::new(),
            output_truncated: false,
        };
    }

    // R-SA-031: snapshot the output file's state ONCE, before the ladder starts (a task's
    // `output_path` is stable across fallback attempts — see `SpawnedChildAttemptRunner::
    // snapshot_output_file`'s own doc note for why re-snapshotting per attempt is unnecessary).
    let output_snapshot = snapshot_output_file(opts.output_path.as_deref());

    let scratch_dir = opts.cwd.join(".cyrup-subagent-scratch");
    if let Err(err) = std::fs::create_dir_all(&scratch_dir) {
        return SingleResult {
            agent: agent.name.clone(),
            task: task.to_string(),
            exit_code: 1,
            usage: Usage::default(),
            model: None,
            attempted_models: Vec::new(),
            model_attempts: Vec::new(),
            final_output: None,
            structured_output: None,
            acceptance: None,
            detached: false,
            interrupted: false,
            timed_out: false,
            error: Some(format!("failed to prepare subagent scratch directory: {err}")),
            tool_calls: Vec::new(),
            output_truncated: false,
        };
    }

    // Step 4: drive the fallback ladder.
    let mut runner = SpawnedChildAttemptRunner {
        agent,
        task,
        opts,
        contract: &contract,
        scratch_dir,
        attempt_index: 0,
    };
    let outcome = run_fallback_ladder(&candidates, &mut runner).await;

    let winning_model = outcome.attempted_models.last().cloned();
    let last_signal = outcome.last_signal;
    let last_attempt = outcome.last_attempt;

    let (timed_out, interrupted, detached, mut exit_code, mut error, mut final_output) =
        match (&last_signal, &last_attempt) {
            (Some(signal), Some(record)) => (
                signal.timed_out,
                false, // interrupted is folded into AttemptSignal.error/exit_code by
                       // drive_attempt today (no dedicated ladder-level flag exists on
                       // AttemptSignal) — see this fn's own doc note above on R-SA-037/084.
                signal.detached,
                signal.exit_code.unwrap_or(if signal.success { 0 } else { 1 }),
                signal.error.clone(),
                record.final_output.clone(),
            ),
            _ => (
                false,
                false,
                false,
                1,
                Some("subagent fallback ladder produced no attempt outcome".to_string()),
                None,
            ),
        };

    // R-SA-031: file-only/output-path handoff, once, against the aggregate captured output.
    if let Some(output_path) = opts.output_path.as_ref() {
        let captured = final_output.clone().unwrap_or_default();
        match resolve_output_handoff(output_path, &captured, output_snapshot) {
            crate::exec::output::OutputHandoff::ChildWrote { content } => {
                final_output = Some(content);
            }
            crate::exec::output::OutputHandoff::OrchestratorWrote {
                written: _,
                error: handoff_error,
            } => {
                if let Some(handoff_error) = handoff_error {
                    error = Some(match error {
                        Some(existing) => format!("{existing}; {handoff_error}"),
                        None => handoff_error,
                    });
                }
            }
        }
    }

    let progress = last_attempt.map(|record| record.progress).unwrap_or_default();

    // Step 5 (R-SA-030): structured-output extraction + parent-side JSON-Schema re-validation.
    // Only evaluated on an otherwise-clean run (mirrors the completion-guard/acceptance gate's own
    // "don't re-diagnose an already-failed attempt" discipline just below) — a run that already
    // failed for another reason (non-zero exit, timeout, detach, interrupt) must not additionally
    // be re-labeled by a structured-output check that never had a fair chance to run against a
    // clean transcript.
    let structured_output = if (CleanCompletionGate {
        exit_code,
        detached,
        interrupted,
        timed_out,
    })
    .is_clean()
    {
        match resolve_structured_output(opts.structured_output_schema.as_ref(), &progress.all_events)
        {
            StructuredOutcome::NotRequested => None,
            StructuredOutcome::Valid(value) => Some(value),
            StructuredOutcome::Missing => {
                // R-SA-030: absence is a hard failure UNLESS plain text was also produced as a
                // fallback.
                if final_output.as_deref().is_some_and(|text| !text.trim().is_empty()) {
                    None
                } else {
                    exit_code = 1;
                    error = Some(match error {
                        Some(existing) if !existing.trim().is_empty() => format!(
                            "{existing}; structured output missing: task declared a \
                             structured-output schema but the child produced neither a \
                             schema-valid structured output nor any plain-text fallback"
                        ),
                        _ => "structured output missing: task declared a structured-output \
                              schema but the child produced neither a schema-valid structured \
                              output nor any plain-text fallback"
                            .to_string(),
                    });
                    None
                }
            }
            StructuredOutcome::Invalid(message) => {
                exit_code = 1;
                error = Some(match error {
                    Some(existing) if !existing.trim().is_empty() => {
                        format!("{existing}; {message}")
                    }
                    _ => message,
                });
                None
            }
        }
    } else {
        None
    };

    // Step 6 (R-SA-034): completion-mutation guard — needs a real AgentDefinition-shaped view;
    // `evaluate_completion_mutation_guard` only reads `local_name`/`tools`/`completion_guard`, so
    // a minimal projection is built here rather than requiring `AgentConfig` to carry every other
    // `AgentDefinition` field this guard never touches.
    let guard_agent = completion_guard_projection(agent);
    let guard_result =
        evaluate_completion_mutation_guard(&guard_agent, task, &progress.all_events);

    let clean_gate = CleanCompletionGate {
        exit_code,
        detached,
        interrupted,
        timed_out,
    };

    if clean_gate.is_clean() && guard_result.triggered {
        exit_code = 1;
        error = Some(match error {
            Some(existing) if !existing.trim().is_empty() => format!(
                "{existing}; {}",
                crate::exec::completion_guard::COMPLETION_GUARD_ERROR_MESSAGE
            ),
            _ => crate::exec::completion_guard::COMPLETION_GUARD_ERROR_MESSAGE.to_string(),
        });
    }

    // Re-derive the gate AFTER the completion-guard correction above, since R-SA-033's own
    // acceptance-gate condition must observe the POST-guard exit code (a run the completion
    // guard already failed must not additionally run acceptance evaluation against a stale
    // "exit_code == 0" snapshot).
    let post_guard_gate = CleanCompletionGate {
        exit_code,
        detached,
        interrupted,
        timed_out,
    };

    // Step 7 (R-SA-032) + Step 8 (R-SA-033), unless R-SA-037 bypasses both entirely.
    let acceptance_ledger = if detached {
        None
    } else {
        let ledger = evaluate_acceptance(
            &contract,
            post_guard_gate,
            final_output.as_deref(),
            guard_result,
            &opts.cwd,
        )
        .await;

        let correction =
            apply_post_hoc_correction(&ledger, contract.explicit, post_guard_gate, error.as_deref());
        exit_code = correction.exit_code;
        error = correction.error;

        Some(ledger)
    };

    // Step 9 (R-SA-042), skipped entirely for a detached result (R-SA-037).
    let (final_output, output_truncated) = if detached {
        (final_output, false)
    } else {
        match final_output {
            Some(text) => {
                let result = truncate_output(&text, agent.max_output, None);
                (Some(result.text), result.truncated)
            }
            None => (None, false),
        }
    };

    // Step 10 (R-SA-043): compaction. `SingleResult` itself is the compacted shape; the
    // `include_progress` flag governs only a LIVE snapshot this function never constructs (no
    // live-callback path exists in this phase), so it has no further effect here beyond being
    // threaded through `RunOptions` for a future phase's live-progress plumbing to read.
    let _ = opts.include_progress;

    SingleResult {
        agent: agent.name.clone(),
        task: task.to_string(),
        exit_code,
        usage: outcome.aggregate_usage,
        model: winning_model,
        attempted_models: outcome.attempted_models,
        model_attempts: outcome.model_attempts,
        final_output,
        structured_output,
        acceptance: acceptance_ledger,
        detached,
        interrupted,
        timed_out,
        error,
        tool_calls: progress.summarized_tool_calls(),
        output_truncated,
    }
}

/// Project an [`AgentConfig`] down to the minimal [`AgentDefinition`] shape
/// [`evaluate_completion_mutation_guard`] actually reads (`local_name`, `tools`,
/// `completion_guard`) — every other field is populated with an inert default since the guard
/// never inspects them. Kept private and narrowly scoped rather than exposing a
/// `From<&AgentConfig> for AgentDefinition` impl crate-wide, since a "mostly-fake"
/// `AgentDefinition` is only ever valid for this one guard call, not as a general conversion.
fn completion_guard_projection(agent: &AgentConfig) -> AgentDefinition {
    AgentDefinition {
        name: agent.name.clone(),
        local_name: agent.name.clone(),
        package_name: None,
        description: String::new(),
        tools: agent.tools.clone(),
        extensions: None,
        subagent_only_extensions: Vec::new(),
        model: agent.model.clone(),
        fallback_models: agent.fallback_models.clone(),
        thinking: None,
        system_prompt_mode: agent.system_prompt_mode,
        inherit_project_context: false,
        inherit_skills: false,
        skills: Vec::new(),
        default_reads: None,
        default_progress: None,
        output: agent.output.clone(),
        completion_guard: agent.completion_guard,
        interactive: None,
        max_subagent_depth: agent.max_subagent_depth,
        default_context: None,
        disabled: None,
        system_prompt_body: agent.system_prompt_body.clone(),
        source: crate::discovery::types::AgentSource::User,
        file_path: PathBuf::new(),
        present_fields: std::collections::HashSet::new(),
        extra_fields: std::collections::BTreeMap::new(),
        override_info: None,
        model_source: None,
    }
}

// ================================================================================================
// plan_batch: eager whole-batch fork-context resolution (arch-SA §6.6, R-SA-137)
// ================================================================================================

/// One batch step's fork-context request, as [`plan_batch`] needs it: an index (for
/// [`ForkContextResolver`]'s own per-index caching) and the requested [`ContextMode`].
#[derive(Debug, Clone, Copy)]
pub struct BatchForkRequest {
    pub index: u32,
    pub requested: ContextMode,
}

/// R-SA-137 (MUST) — eagerly resolve EVERY step's [`ForkContext`] in `requests`, before spawning
/// ANY child process for the batch, via [`ForkContextResolver::resolve`] — the sole owner of
/// fork-context logic in this crate (arch-SA §6.6; this function never re-derives any part of
/// that algorithm, it only sequences calls into it).
///
/// If ANY resolution errors, the WHOLE batch aborts immediately — this function returns that
/// first error without attempting any further request, and (by construction: this function
/// spawns nothing itself) zero subprocesses have been spawned for this batch at the point of
/// failure. Implementing this lazily (validating step N's fork only when execution reaches step
/// N) would violate the fail-fast intent R-SA-137 requires; `plan_batch` exists specifically so a
/// caller (a later phase's chain/parallel dispatch in `exec/`, or the background hand-off's
/// one-shot runner-config construction, arch-SA §6.5) can call this ONCE, up front, for a whole
/// batch and only proceed to spawning if every resolution in `requests` succeeded.
///
/// On success, returns one [`ForkContext`] per request, in the SAME order as `requests` — a
/// caller zips this back against its own step list by position, mirroring R-SA-051's
/// position-preserving-regardless-of-completion-order discipline (restated here at plan time
/// rather than execution time, since fork-context resolution for a `Fresh` step is synchronous
/// and effectively instantaneous, so there is no meaningful "completion order" to preserve beyond
/// simply awaiting each request in the order given).
///
/// # Errors
///
/// Propagates the first [`SubagentError`] any individual [`ForkContextResolver::resolve`] call
/// returns (`ForkRequiresLeaf`/`ForkRequiresPersistedParent`/`ForkFailed`) — never falls back to
/// [`ContextMode::Fresh`] for a request that explicitly asked for [`ContextMode::Fork`]
/// (R-SA-137/DI-SA-2's fail-hard rule, restated at the batch level).
pub async fn plan_batch(
    resolver: &ForkContextResolver,
    requests: &[BatchForkRequest],
) -> Result<Vec<ForkContext>, SubagentError> {
    let mut resolved = Vec::with_capacity(requests.len());
    for request in requests {
        let ctx = resolver.resolve(request.requested, request.index).await?;
        resolved.push(ctx);
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use crate::exec::acceptance::AcceptanceStatus;

    fn sample_agent_config(model: &str, fallback: &[&str]) -> AgentConfig {
        AgentConfig {
            name: "worker".to_string(),
            model: Some(ModelId::from(model)),
            fallback_models: fallback.iter().map(|m| ModelId::from(*m)).collect(),
            system_prompt_mode: SystemPromptMode::Replace,
            system_prompt_body: String::new(),
            tools: None,
            output: None,
            completion_guard: Some(false),
            max_output: OutputCap::default(),
            max_subagent_depth: None,
            depth: DepthEnvelope {
                current_depth: 0,
                max_depth: 5,
            },
        }
    }

    fn base_opts(cwd: &std::path::Path, available: &[&str]) -> RunOptions {
        RunOptions {
            cwd: cwd.to_path_buf(),
            deadline_at: None,
            output_path: None,
            output_mode: OutputMode::Inline,
            structured_output_schema: None,
            model_override: ModelOverride::Inherit,
            preferred_provider: None,
            available_models: available.iter().map(|m| ModelId::from(*m)).collect(),
            cancel: CancelToken::new(),
            interrupt: CancelToken::new(),
            share: None,
            session_dir: None,
            include_progress: None,
            agent_scope: None,
            acceptance: Some(AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![])),
            fork_context: ForkContext::fresh(),
        }
    }

    // ---- AgentProgress: R-SA-027/028 folding ----

    #[test]
    fn record_event_accumulates_usage_additively_across_multiple_message_end_events() {
        let mut progress = AgentProgress::default();
        let ev1 = SubagentEvent::MessageEnd {
            message: serde_json::json!({
                "role": "assistant", "content": [],
                "usage": {"input": 10, "output": 5, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 15, "cost": {"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0,"total":0.0}}
            }),
        };
        let ev2 = SubagentEvent::MessageEnd {
            message: serde_json::json!({
                "role": "assistant", "content": [],
                "usage": {"input": 3, "output": 2, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 5, "cost": {"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0,"total":0.0}}
            }),
        };
        progress.record_event(ev1);
        progress.record_event(ev2);
        assert_eq!(progress.usage.input, 13);
        assert_eq!(progress.usage.output, 7);
        assert_eq!(progress.message_end_events.len(), 2);
    }

    #[test]
    fn record_event_increments_tool_count_and_sets_current_tool() {
        let mut progress = AgentProgress::default();
        progress.record_event(SubagentEvent::ToolExecutionStart {
            tool_call_id: "c1".into(),
            tool_name: "bash".to_string(),
            args: serde_json::Value::Null,
        });
        progress.record_event(SubagentEvent::ToolExecutionStart {
            tool_call_id: "c2".into(),
            tool_name: "edit".to_string(),
            args: serde_json::Value::Null,
        });
        assert_eq!(progress.tool_count, 2);
        assert_eq!(progress.current_tool.as_deref(), Some("edit"));
    }

    #[test]
    fn recent_output_buffer_is_capped_at_50_lines_oldest_evicted_first() {
        let mut progress = AgentProgress::default();
        for i in 0..(RECENT_OUTPUT_CAP + 10) {
            progress.record_raw_line(&format!("line-{i}"));
        }
        assert_eq!(progress.recent_output.len(), RECENT_OUTPUT_CAP);
        assert_eq!(progress.recent_output.front().map(String::as_str), Some("line-10"));
        let expected_last = format!("line-{}", RECENT_OUTPUT_CAP + 9);
        assert_eq!(
            progress.recent_output.back().map(String::as_str),
            Some(expected_last.as_str())
        );
    }

    #[test]
    fn summarized_tool_calls_reflects_every_tool_execution_end_in_order() {
        let mut progress = AgentProgress::default();
        progress.record_event(SubagentEvent::ToolExecutionEnd {
            tool_call_id: "c1".into(),
            tool_name: "bash".to_string(),
            result: serde_json::Value::Null,
            is_error: false,
        });
        progress.record_event(SubagentEvent::ToolExecutionEnd {
            tool_call_id: "c2".into(),
            tool_name: "edit".to_string(),
            result: serde_json::Value::Null,
            is_error: false,
        });
        assert_eq!(
            progress.summarized_tool_calls(),
            vec!["bash".to_string(), "edit".to_string()]
        );
    }

    // ---- build_task_text / build_attempt_spawn_plan ----

    #[test]
    fn build_task_text_injects_acceptance_contract_and_output_path_instruction() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_mode = SystemPromptMode::Replace;
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.output_mode = OutputMode::FileOnly;
        opts.output_path = Some(dir.path().join("out.md"));
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Checked, vec![]);

        let text = build_task_text(&agent, "do the thing", &opts, &contract);
        assert!(text.starts_with("do the thing"));
        assert!(text.contains("Acceptance Contract"));
        assert!(text.contains("out.md"));
    }

    #[test]
    fn build_task_text_appends_system_prompt_body_in_append_mode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_mode = SystemPromptMode::Append;
        agent.system_prompt_body = "You are a delegate persona.".to_string();
        let opts = base_opts(dir.path(), &["m1"]);
        let contract = AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![]);

        let text = build_task_text(&agent, "do the thing", &opts, &contract);
        assert!(text.contains("You are a delegate persona."));
    }

    #[test]
    fn build_attempt_spawn_plan_includes_tools_flag_only_when_agent_declares_tools() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.tools = Some(vec![
            ToolRef::Builtin("read".to_string()),
            ToolRef::Builtin("edit".to_string()),
        ]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path())
            .expect("plan builds");
        let argv = plan.spec.build_argv();
        let tools_idx = argv.iter().position(|a| a == "--tools").expect("--tools present");
        assert_eq!(argv[tools_idx + 1], "read,edit");
    }

    #[test]
    fn build_attempt_spawn_plan_omits_tools_flag_when_agent_declares_no_allowlist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path())
            .expect("plan builds");
        assert!(!plan.spec.build_argv().contains(&"--tools".to_string()));
    }

    #[test]
    fn build_attempt_spawn_plan_includes_session_flag_when_fork_context_resolved() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.fork_context = ForkContext {
            mode: ContextMode::Fork,
            session_file_path: Some(dir.path().join("parent-branch.jsonl")),
        };
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path())
            .expect("plan builds");
        let argv = plan.spec.build_argv();
        let idx = argv.iter().position(|a| a == "--session").expect("--session present");
        assert!(argv[idx + 1].contains("parent-branch.jsonl"));
    }

    #[test]
    fn build_attempt_spawn_plan_propagates_depth_envelope_into_env_overlay() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 2,
            max_depth: 4,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path())
            .expect("plan builds");
        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::depth::DEPTH_ENV_VAR),
            Some(&"2".to_string())
        );
        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::depth::MAX_DEPTH_ENV_VAR),
            Some(&"4".to_string())
        );
    }

    // ---- run_sync: depth guard runs first, before anything else (R-SA-055, SAFETY-CRITICAL) ----

    #[tokio::test]
    async fn run_sync_rejects_a_blocked_depth_envelope_before_any_spawn_setup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        // current_depth == max_depth: is_blocked() must be true (R-SA-055's own `>=` semantics,
        // not merely `>`).
        agent.depth = DepthEnvelope {
            current_depth: 3,
            max_depth: 3,
        };
        let opts = base_opts(dir.path(), &["m1"]);

        let result = run_sync(&agent, "do something", &opts).await;

        assert_eq!(result.exit_code, 1, "a blocked depth attempt must report failure: {result:?}");
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("depth limit exceeded"),
            "expected a DepthExceeded-shaped error message, got: {:?}",
            result.error
        );
        assert!(result.attempted_models.is_empty(), "no model attempt may ever be made");
        assert!(result.model_attempts.is_empty());
        assert_eq!(result.usage, Usage::default(), "no usage can have accrued");
        // The load-bearing proof that this rejection happens BEFORE any spawn setup: `run_sync`'s
        // scratch-directory creation (the very first filesystem side effect any subsequent spawn
        // attempt would need) must never have run at all.
        assert!(
            !dir.path().join(".cyrup-subagent-scratch").exists(),
            "the depth guard must reject before the spawn-scratch directory is ever created"
        );
    }

    #[tokio::test]
    async fn run_sync_rejects_when_depth_has_defensively_exceeded_the_ceiling() {
        // current_depth > max_depth (should never occur given each hop only increments by one past
        // a checked gate, but the guard must still be a safe `>=`, matching
        // `spawn::depth::is_blocked`'s own defense-in-depth comparison).
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.depth = DepthEnvelope {
            current_depth: 9,
            max_depth: 2,
        };
        let opts = base_opts(dir.path(), &["m1"]);

        let result = run_sync(&agent, "do something", &opts).await;

        assert_eq!(result.exit_code, 1);
        assert!(!dir.path().join(".cyrup-subagent-scratch").exists());
    }

    #[tokio::test]
    async fn run_sync_proceeds_normally_when_strictly_below_the_depth_ceiling() {
        // The negative case: a non-blocked envelope must NOT be rejected by the depth guard —
        // proven by observing this attempt fails for the ordinary, UNRELATED "no candidate model"
        // reason (this test supplies no available models), never a DepthExceeded message, so the
        // depth guard is proven to be neither a false-positive gate nor accidentally bypassed by a
        // change to this function's own step ordering.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.model = None;
        agent.depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let opts = base_opts(dir.path(), &[]); // no available models: ladder is empty downstream

        let result = run_sync(&agent, "do something", &opts).await;

        assert_eq!(result.exit_code, 1);
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("no candidate model"),
            "a non-blocked depth must fall through to the NEXT gate (empty ladder), not be \
             rejected by the depth guard itself, got: {:?}",
            result.error
        );
    }

    // ---- run_sync: pre-spawn fail-fast (R-SA-025) ----

    #[tokio::test]
    async fn run_sync_fails_fast_on_file_only_mode_without_output_path_before_any_spawn() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.output_mode = OutputMode::FileOnly;
        opts.output_path = None;

        let result = run_sync(&agent, "do something", &opts).await;
        assert_eq!(result.exit_code, 1);
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("output path")
        );
        // No scratch dir should have been created since this fails before any spawn setup.
        assert!(!dir.path().join(".cyrup-subagent-scratch").exists());
    }

    #[tokio::test]
    async fn run_sync_fails_with_empty_ladder_when_no_model_is_resolvable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.model = None;
        let opts = base_opts(dir.path(), &[]); // nothing available
        let result = run_sync(&agent, "do something", &opts).await;
        assert_eq!(result.exit_code, 1);
        assert!(result.attempted_models.is_empty());
    }

    // ---- plan_batch: eager whole-batch fork-context resolution (R-SA-137) ----

    #[tokio::test]
    async fn plan_batch_resolves_every_fresh_request_in_order() {
        let root = tempfile::tempdir().expect("tempdir");
        let cwd = PathBuf::from("/proj/plan-batch-test");
        let layout = cyrup_session::SessionLayout::new(root.path().to_path_buf(), cwd.clone());
        let manager = cyrup_session::SessionManager::in_memory(&cwd, cyrup_session::NewSessionOpts::default())
            .expect("create in-memory session");
        let manager = std::sync::Arc::new(tokio::sync::Mutex::new(manager));
        let resolver = ForkContextResolver::new(manager, layout);

        let requests = vec![
            BatchForkRequest {
                index: 0,
                requested: ContextMode::Fresh,
            },
            BatchForkRequest {
                index: 1,
                requested: ContextMode::Fresh,
            },
        ];
        let resolved = plan_batch(&resolver, &requests).await.expect("resolves");
        assert_eq!(resolved.len(), 2);
        assert!(resolved.iter().all(|ctx| ctx.mode == ContextMode::Fresh));
    }

    #[tokio::test]
    async fn plan_batch_aborts_whole_batch_on_first_fork_failure_zero_side_effects() {
        let root = tempfile::tempdir().expect("tempdir");
        let cwd = PathBuf::from("/proj/plan-batch-abort-test");
        let layout = cyrup_session::SessionLayout::new(root.path().to_path_buf(), cwd.clone());
        // Unpersisted in-memory session: any Fork request must fail hard (R-SA-137/DI-SA-2).
        let manager = cyrup_session::SessionManager::in_memory(&cwd, cyrup_session::NewSessionOpts::default())
            .expect("create in-memory session");
        let manager = std::sync::Arc::new(tokio::sync::Mutex::new(manager));
        let resolver = ForkContextResolver::new(manager, layout);

        let requests = vec![
            BatchForkRequest {
                index: 0,
                requested: ContextMode::Fresh,
            },
            BatchForkRequest {
                index: 1,
                requested: ContextMode::Fork, // must fail: unpersisted parent
            },
            BatchForkRequest {
                index: 2,
                requested: ContextMode::Fresh,
            },
        ];
        let err = plan_batch(&resolver, &requests)
            .await
            .expect_err("must abort on the second request's failure");
        assert!(matches!(
            err,
            SubagentError::ForkRequiresPersistedParent | SubagentError::ForkRequiresLeaf
        ));

        // No filesystem state created anywhere under root — proof zero subprocess/session-branch
        // side effects occurred beyond the failed resolution itself.
        let any_files = std::fs::read_dir(root.path())
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false);
        assert!(!any_files);
    }
}
