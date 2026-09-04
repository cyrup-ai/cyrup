//! The output contract of one [`crate::exec::run_sync`] call (arch-SA §3.4): [`SingleResult`].
//! Split out of `exec/mod.rs`'s own "AgentConfig / RunOptions / SingleResult" section;
//! [`crate::exec::agent_config`] is that section's input-contract half.

use cyrup_core::{ModelId, Usage};

use crate::exec::acceptance::AcceptanceLedger;
use crate::exec::fallback::ModelAttempt;
use crate::exec::tool_call_summary::ToolCallSummary;

/// The full, terminal outcome of one `run_sync` call (arch-SA §3.4). This is always the
/// **compacted** (R-SA-043) shape: no raw per-turn messages — only the summarized fields below.
/// The one opt-out is [`Self::progress`], which [`crate::exec::RunOptions::include_progress`] gates exactly as
/// pi's `includeProgress` gates `Details.progress`; see that field's own doc.
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
    /// completion-guard, and output truncation entirely. Set from a REAL blocking-detach signal (the
    /// R-SA-119/120 intercom wiring is now CLOSED): a child's blocking `contact_supervisor` ask on its
    /// NDJSON stdout is detected by [`crate::exec::drive_attempt::drive_attempt`], surfaced via [`crate::tui::intercom::spawn_clarify`]
    /// against the executor's `AskLock` (backed in production by the intercom companion's real broker
    /// `ClarifyChannel` threaded through [`crate::exec::RunOptions::clarify`]), and carried onto this flag — see
    /// [`crate::exec::fallback::AttemptSignal::detached`]'s doc comment for the full wiring trace. When
    /// no intercom channel is wired (headless / `RunOptions::clarify = None`) the drive loop still marks
    /// the attempt detached but the `AskLock` degrades to its no-live-channel fallback.
    pub detached: bool,
    /// A soft interrupt was observed (`RunOptions.interrupt` fired) — like a timeout, this
    /// terminates the fallback ladder outright without advancing, but is recorded under its own
    /// flag rather than folded into `timed_out` (R-SA-084 vs. R-SA-036 have distinct downstream
    /// consequences a caller may want to distinguish).
    pub interrupted: bool,
    pub timed_out: bool,
    /// G77/G104 — pi `SingleResult.stopped` (`shared/types.ts:879`, set by `runSubagent` at
    /// `subagent-runner.ts:2957`/`:2960`/`:2970`): this child was terminated by an explicit
    /// user/agent **stop** request, not by an interrupt, a deadline, or its own exit.
    ///
    /// A distinct flag from [`Self::interrupted`] and [`Self::timed_out`] because upstream reads
    /// all three separately and ranks them differently: `resolveSubagentResultStatus` returns
    /// `"stopped"` for it BEFORE it ever looks at `interrupted`/`success`/`exitCode`
    /// (`intercom/result-intercom.ts:32`), `chain-root-attachment.ts:87` short-circuits on it ahead
    /// of the child's own `success`, and `notify.ts:203` ORs it into the run-level stop verdict.
    ///
    /// `#[serde(default)]` + omit-when-false so a `status.json`/result file written before this
    /// field existed still round-trips (the same discipline `saved_output_path`/`control_events`
    /// follow), matching upstream's own optional `stopped?: boolean`.
    #[serde(default, skip_serializing_if = "crate::exec::is_false")]
    pub stopped: bool,
    /// pi `SingleResult.processSignal` (`shared/types.ts`, set from Node's `proc.on("close",
    /// (code, signal))` at `subagent-runner.ts:903`): the NAME of the OS signal that killed this
    /// child (`"SIGKILL"`, `"SIGTERM"`, …), or `None` on a normal exit.
    ///
    /// Carried onto the terminal result — not only onto
    /// [`crate::exec::fallback::StartupEvidence::process_signal`], where this crate already
    /// computed it — because `resolveSubagentResultStatus`'s fourth branch
    /// (`result-intercom.ts:35`) resolves an UNEXPLAINED signal death (a signal with no
    /// interrupt/timeout/stop/turn-budget to explain it, `runs/shared/process-signal.ts:5-19`) to
    /// `"stopped"`, and that branch is unreachable without this field. Same optional-on-the-wire
    /// discipline as [`Self::stopped`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_signal: Option<String>,
    /// SUBA-008 — pi `SingleResult.turnBudget?: TurnBudgetState` (`shared/types.ts:1188`, assigned
    /// by `updateTurnBudget` at `execution.ts:763`/`:773`/`:780`): the assistant-turn budget this
    /// run ran under and how it ended.
    ///
    /// `None` for every run that declared no budget, and omitted from the wire when `None`, so a
    /// `status.json`/result file written before this field existed still round-trips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_budget: Option<crate::exec::turn_budget::TurnBudgetState>,
    /// SUBA-008 — pi `SingleResult.turnBudgetExceeded?: boolean` (`shared/types.ts:1189`, set at
    /// `execution.ts:737`): the supervisor aborted this child for blowing its turn budget.
    ///
    /// Read by `resolveSubagentResultStatus` through `isUnexplainedProcessSignal`
    /// (`result-intercom.ts:35`, `runs/shared/process-signal.ts:5-19`) — a turn-budget kill is an
    /// EXPLAINED signal death, so without this field a budget abort was misreported as `stopped`.
    #[serde(default, skip_serializing_if = "crate::exec::is_false")]
    pub turn_budget_exceeded: bool,
    /// SUBA-008 — pi `SingleResult.wrapUpRequested?: boolean` (`shared/types.ts`, set at
    /// `execution.ts:768`/`:738`): the child was asked to wrap up because it reached the soft
    /// limit. True for a deferred or exceeded run as well, matching upstream's own derivation
    /// (`subagent-runner.ts:924`).
    #[serde(default, skip_serializing_if = "crate::exec::is_false")]
    pub wrap_up_requested: bool,
    /// SUBA-021 — pi `statusPayload.usageBudget` (`subagent-runner.ts:4411`, published onto the
    /// result at `:4471` and onto `status.json` via `async-status.ts:336`): the reported-consumption
    /// budget this run ran under and where it ended up.
    ///
    /// `None` for every run that declared no budget, and omitted from the wire when `None`, so a
    /// result file written before this field existed still round-trips. When
    /// [`crate::exec::usage_budget::UsageBudgetState::exhausted`] is set, [`Self::error`] carries
    /// [`crate::exec::usage_budget::usage_budget_exceeded_message`] — that pairing is upstream's
    /// own at `:4403-4404`, and it is what makes an exhausted budget a TERMINAL outcome rather than
    /// a note on a successful run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_budget: Option<crate::exec::usage_budget::UsageBudgetState>,
    pub error: Option<String>,
    /// pi `result.savedOutputPath` (`shared/types.ts:492`, assigned at
    /// `runs/foreground/execution.ts:963` from `resolveSingleOutput(...).savedPath`): the concrete
    /// file the R-SA-031 output-path handoff actually persisted this run's delivered output to,
    /// `None` when no `output_path` was requested, the run did not complete cleanly, or nothing
    /// was written.
    ///
    /// This is the SAME value the saved-output reference message folded into `final_output` is
    /// built from — carried as its own field because consumers need the bare path, not the prose:
    /// pi's `collectDynamicResults` emits it as a dynamic collect record's `outputPath`
    /// (`runs/shared/dynamic-fanout.ts:283`) so a later chain step can locate the file each
    /// fanned-out sibling wrote.
    ///
    /// `#[serde(default)]` + omit-when-absent so a `status.json`/result file written before this
    /// field existed still round-trips (the same discipline `control_events` below follows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_output_path: Option<String>,
    /// Summarized `{text, expandedText}` tool-call previews observed across the winning attempt's
    /// transcript — R-SA-043's "only summarized `tool_calls`" compaction requirement (pi's
    /// `ToolCallSummary[]`, `utils.ts:368-373`). Each carries a short and an expanded argument
    /// preview (pi `formatToolCall`), NOT a bare tool name. Never the raw per-turn message list.
    pub tool_calls: Vec<ToolCallSummary>,
    /// Whether [`crate::exec::output::truncate_output`] actually cut the delivered `final_output` (R-SA-042).
    pub output_truncated: bool,
    /// pi `result.controlEvents` (`execution.ts:1112`/`:1260`): every live-control event the
    /// WINNING attempt raised, in raise order, plus the post-settlement completion-guard raise
    /// (`:1234`). Empty for a run whose control config is disabled, whose `notifyOn` excluded both
    /// classes, or that simply never tripped a threshold — which is why it is `#[serde(default)]`
    /// and omitted from the wire when empty: a persisted `status.json`/result file written before
    /// this field existed still round-trips.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub control_events: Vec<crate::exec::control::ControlEvent>,
    /// pi `SingleResult.progress` (`pi-subagents/src/shared/types.ts:844`) — this run's own
    /// `AgentProgress` snapshot, and the home for what `includeProgress` gates.
    ///
    /// **`None` unless [`crate::exec::RunOptions::include_progress`] is `Some(true)`.** That is the whole
    /// contract: R-SA-043's compaction stays the default, `includeProgress` is its documented
    /// opt-out (pi `progress: params.includeProgress ? allProgress : undefined`,
    /// `runs/foreground/subagent-executor.ts:3071` for PARALLEL and `:3406` for SINGLE), and with
    /// the flag off or omitted this field skips serialization entirely so a returned/persisted
    /// `SingleResult` is byte-for-byte what it was before the field existed.
    ///
    /// When populated it has always been through
    /// [`crate::tui::events::LiveProgressSnapshot::compact_completed`] (pi
    /// `compactCompletedProgress` via `compactForegroundDetails`, `shared/utils.ts:414-421`), which
    /// for every SETTLED status empties the two per-run growth terms — the tool-history ring and
    /// the recent-output tail.
    ///
    /// **The one exception is upstream's, not this port's**: pi's `compactCompletedProgress` opens
    /// with `if (progress.status === "running") return progress;`, and an interrupt-PAUSED run is
    /// precisely the case pi leaves at `"running"` (`execution.ts:828`, returning at `:861` before
    /// the `completed`/`failed` assignment at `:907`). Such a snapshot keeps its rings — which is
    /// the point, since the caller is expected to resume the run. Both rings are bounded at PUSH
    /// time in this port ([`crate::tui::events::RECENT_TOOLS_CAP`] entries,
    /// [`crate::exec::RECENT_OUTPUT_CAP`] lines of at most [`crate::exec::RECENT_OUTPUT_LINE_CHARS`] chars each), so even
    /// that shape is O(1) in the child's chattiness. pi bounds neither on this path.
    ///
    /// **[CYRUP-DELTA] on placement.** pi carries the array one level UP, on
    /// `Details.progress: AgentProgress[]` (`shared/types.ts:908`), assembled as `allProgress` from each
    /// child's own `result.progress` (`subagent-executor.ts:3424,3444,3793,3819` @v0.43.0), and blanks
    /// `SingleResult.progress` in the returned `results` (`compactForegroundResult`,
    /// `utils.ts:404-412`). cyrup's SINGLE-mode tool `details` IS the serialized `SingleResult`
    /// (`extension.rs::route_single`) rather than a `Details` wrapper, so the snapshot lands on the
    /// field pi already declares for it and surfaces at the same JSON path (`details.progress`) a
    /// pi caller reads for a SINGLE run — one snapshot rather than a one-element array.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<crate::tui::events::LiveProgressSnapshot>,
}
