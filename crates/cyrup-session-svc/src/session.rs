//! `AgentSession` — the single integration seam every front-end consumes (func-11 R-11-023).
//!
//! Wires the agent loop + tools + session persistence + config + resources + extensions behind one
//! async API: start/resume, prompt (→ an `EventStream<AgentSessionEvent>`), steer/follow-up,
//! interrupt, compaction, fork/branch + branch-summary, switch model — with durable persistence
//! across every turn. No mode reaches behaviour that does not flow through this object.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use cyrup_agent::{Agent, AgentMessage};
use cyrup_core::{
    AssistantMessage, CancelToken, Content, EntryId, EventStream, Message, ModelId,
    ModelRef, ModelThinkingLevel, ProviderId, SessionId, Usage,
};
use cyrup_ext::host::ControlOp;
use cyrup_ext::{
    CompactionReduction, HostEvent, InputEventSource, InputStreamingBehavior, Reduced, TreeReduction,
};
use cyrup_provider::{is_context_overflow, is_retryable_assistant_error, Model, RetryPolicy};
use cyrup_session::compaction::{
    context_tokens_from_usage, BranchSummaryOutput, BranchSummarySettings,
    CompactionOverride, CompactionPreparation, CompactionReason, CompactionSettings, Compactor,
    NoHooks,
};
use cyrup_session::context::SessionContext;
use cyrup_session::header::SessionHeader;
use cyrup_session::manager::SessionManager;
use cyrup_tools::{ProcOps, ShellConfig};
use tokio::sync::Mutex as AsyncMutex;

use crate::bash::{bash_message_payload, run_bash, BashOptions, BashResult};
use crate::compact::DynSummarizer;
use crate::error::SessionServiceError;
use crate::event::{
    core_message_to_agent, raw_message_to_agent, AgentSessionEvent, InputSource, PromptAccepted,
    PromptOptions, StreamingBehavior, SummarizationRetrySource, UserInput,
};
use crate::host_services::InjectMessage;
use crate::provider_swap::ProviderSwap;
use crate::services::AgentSessionServices;
use crate::subscriber::Fanout;
use crate::tools::{DynamicToolState, ToolInfo};

/// Upper bound on a `ControlOp::WaitIdle` drained at the command tier (SEAM-003). Pi's
/// `ctx.waitForIdle()` is a promise resolved by `_resolveIdleWaitIfIdle` and cannot wedge the
/// command path; cyrup's waits on the post-run driver watch, which a CONCURRENT run could hold
/// indefinitely. The op is bounded and its expiry reported rather than hanging the drain.
const WAIT_IDLE_CONTROL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Upper bound on the `await this.waitForIdle()` tail of [`AgentSession::abort_and_settle`]
/// (SEAM-024). Pi's `abort()` awaits unboundedly (agent-session.ts:1545), but its callers are a
/// browser-style event loop; here the same await sits on `dispose`, i.e. on every `quit`, every
/// session replacement and the RPC `abort` verb, so a tool wedged in an uninterruptible syscall
/// would otherwise make the process unkillable-by-Ctrl-C. On expiry the caller continues exactly as
/// the pre-SEAM-024 fire-and-forget `abort()` did — never worse than the old behaviour.
const ABORT_SETTLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The op's Pi-facing name, for the SEAM-003 failure diagnostic.
fn control_op_name(op: &ControlOp) -> &'static str {
    match op {
        ControlOp::NewSession { .. } => "new_session",
        ControlOp::Switch { .. } => "switch_session",
        ControlOp::Fork { .. } => "fork",
        ControlOp::Navigate { .. } => "navigate_tree",
        ControlOp::Reload => "reload",
        ControlOp::Compact { .. } => "compact",
        ControlOp::WaitIdle => "wait_idle",
        ControlOp::SendMessage { .. } => "send_message",
        ControlOp::SendUserMessage { .. } => "send_user_message",
        ControlOp::SetModel(_) => "set_model",
        ControlOp::SetThinkingLevel(_) => "set_thinking_level",
        ControlOp::Abort => "abort",
        ControlOp::Shutdown => "shutdown",
    }
}

/// Where a fork anchors relative to the selected entry (Pi `fork(entryId, {position})`,
/// agent-session-runtime.ts:259). `Before` anchors at the selected *user* message's parent and
/// extracts its text (for re-editing); `At` anchors at the selected entry itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ForkPosition {
    #[default]
    Before,
    At,
}

impl ForkPosition {
    /// pi's own literal — `position: "before" | "at"` (`core/extensions/types.ts:585-589`
    /// @v0.83.0), carried on `session_before_fork`. SEAM-012.
    pub const fn as_str(self) -> &'static str {
        match self {
            ForkPosition::Before => "before",
            ForkPosition::At => "at",
        }
    }
}

/// The outcome of an entry-anchored fork (Pi returns `{cancelled, selectedText}`,
/// agent-session-runtime.ts:262).
#[derive(Clone, Debug, Default)]
pub struct ForkOutcome {
    /// The new branched session id (the forked file's session id), if a new file was created.
    pub session_id: Option<SessionId>,
    /// For `position:"before"`, the selected user message's text (so a UI can pre-fill the editor).
    pub selected_text: Option<String>,
}

/// A single user message anchor for the `/tree`/`/fork` pickers (Pi `getUserMessagesForForking`,
/// agent-session.ts:2901).
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkAnchor {
    pub entry_id: EntryId,
    pub text: String,
}

/// The entry-type classification of a [`SessionDagNode`], mirroring the glyph switch Pi's tree
/// selector keys off (`tree-selector.ts:567-611`, `:762`). Kept UI-agnostic (a plain tag) so the
/// TUI maps it to its own `TreeKind` glyph without this layer depending on cyrup-tui.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionDagKind {
    /// A user/assistant/bash message entry (`●`).
    Message,
    /// A `tool_result` message entry (`⚙`).
    Tool,
    /// A `model_change` entry (`◆`).
    ModelChange,
    /// A `thinking_level_change` entry (`◇`).
    ThinkingChange,
    /// A `compaction` or `branch_summary` entry (`✓`).
    Compaction,
    /// Anything else (`session_info`/`label`/`custom`/unknown) — rendered as a message.
    Other,
}

/// One node of the **flattened session DAG** (feature #2): the flat-tree getter the `/tree` selector
/// was starved for. Produced by [`AgentSession::session_dag`] by walking the manager's real branch
/// tree (`SessionManager::tree`) in pre-order, carrying each node's parent link, depth, display label,
/// kind, fold-ability (has children), leaf-ness (the active branch tip), user-label, and timestamp —
/// exactly the `FlatNode` fields Pi's `flattenTree` computes (`tree-selector.ts:27-35`, `:199-320`).
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDagNode {
    /// The entry id (the branch/summarize target on confirm).
    pub entry_id: EntryId,
    /// The parent entry id (`None` for a root).
    pub parent_id: Option<EntryId>,
    /// Pre-order tree depth (0 = a root; roots get no connector, spec/tui/05 §5.1).
    pub depth: usize,
    /// The one-line display label (role-prefixed message text / `model → id` / `thinking → level` / …).
    pub label: String,
    /// The entry-type classification driving the row glyph.
    pub kind: SessionDagKind,
    /// Whether this node has descendants (renders the foldable `⊟`/`⊞` marker).
    pub foldable: bool,
    /// Whether this node is the current branch leaf (the active tip).
    pub is_leaf: bool,
    /// Whether the entry carries a user label (renders the `☆` star).
    pub has_label: bool,
    /// The entry's RFC3339 timestamp (drives the right-aligned time column).
    pub timestamp: String,
}

/// Options for the unified `/tree` navigation op (Pi `navigateTree(targetId, options)`,
/// agent-session.ts:2704). `summarize` runs the branch summarizer over the abandoned branch;
/// `custom_instructions`/`replace_instructions` steer that summary prompt (Pi
/// `branch-summarization.ts:318-336`); `label` is attached to the resulting summary entry (or, when
/// not summarizing, to the navigation target).
#[derive(Clone, Debug, Default)]
pub struct NavigateTreeOptions {
    pub summarize: bool,
    pub custom_instructions: Option<String>,
    pub replace_instructions: bool,
    pub label: Option<String>,
}

/// The outcome of [`AgentSession::navigate_tree`] (Pi navigateTree return,
/// agent-session.ts:2710): `editor_text` is the re-editable text when the target is a user/custom
/// message; `cancelled` is set when the op was a no-op or an extension vetoed it; `aborted` is set
/// when an in-flight summarization was cancelled; `summary_entry` is the appended branch summary.
#[derive(Clone, Debug, Default)]
pub struct NavigateTreeOutcome {
    pub editor_text: Option<String>,
    pub cancelled: bool,
    pub aborted: bool,
    pub summary_entry: Option<cyrup_session::compaction::BranchSummaryEntry>,
}

/// A scoped model in the `cycle_model` set (Pi `{model, thinkingLevel?}`, agent-session.ts:870). An
/// explicit `thinking_level` overrides the session level when cycled to; `None` inherits it.
#[derive(Clone, Debug)]
pub struct ScopedModel {
    pub model: Model,
    pub thinking_level: Option<ModelThinkingLevel>,
}

/// The typed result of [`AgentSession::cycle_model`] (Pi `ModelCycleResult`, agent-session.ts:1471).
/// `is_scoped` distinguishes the scoped-set path from the full-catalog path.
#[derive(Clone, Debug)]
pub struct ModelCycleResult {
    pub model: Model,
    pub thinking_level: ModelThinkingLevel,
    pub is_scoped: bool,
}

/// The disposition of the `input` extension event (Pi `InputEventResult.action`, runner.ts:1100).
/// A `transform` outcome rewrites the in-flight [`UserInput`] in place (via `EventPatch::Input`) and
/// then reports `Continue`, exactly as Pi folds `currentText`/`currentImages` before continuing.
enum InputDisposition {
    /// A handler fully serviced the submission (`handled`); no run or queue follows.
    Handled,
    /// No handler claimed it; proceed with expansion + run/queue (text/images may have been
    /// rewritten by a `transform` handler already applied to the [`UserInput`]).
    Continue,
}

/// Collapse the host-side [`InputSource`] onto Pi's three handler-visible `InputSource` values
/// (`"interactive" | "rpc" | "extension"`, extensions/types.ts:789). cyrup's richer provenance
/// (`Cli`/`Stdin`/`Sdk`/`Tui`) all present as `interactive` to a handler, exactly as Pi's host
/// passes `"interactive"` for any non-rpc submission (agent-session.ts:1021).
fn input_event_source(source: InputSource) -> InputEventSource {
    match source {
        InputSource::Rpc => InputEventSource::Rpc,
        InputSource::Cli | InputSource::Stdin | InputSource::Sdk | InputSource::Tui => {
            InputEventSource::Interactive
        }
    }
}

/// Map the queue selector onto the handler-visible `streamingBehavior` (Pi `"steer" | "followUp"`).
fn input_streaming_behavior(behavior: StreamingBehavior) -> InputStreamingBehavior {
    match behavior {
        StreamingBehavior::Steer => InputStreamingBehavior::Steer,
        StreamingBehavior::FollowUp => InputStreamingBehavior::FollowUp,
    }
}

/// Parse a guest `setModel` payload (a `control` capability arg) into `(provider, model)`. Accepts
/// either `"provider/model"` (Pi's `provider/model` id form) or `{ "provider": .., "model": .. }`.
/// Returns `None` for an unparseable payload (degrade, never panic).
fn parse_model_ref(v: &serde_json::Value) -> Option<(ProviderId, ModelId)> {
    if let Some(s) = v.as_str() {
        let (p, m) = s.split_once('/')?;
        if p.is_empty() || m.is_empty() {
            return None;
        }
        return Some((ProviderId::from(p), ModelId::from(m)));
    }
    let p = v.get("provider").and_then(serde_json::Value::as_str)?;
    let m = v.get("model").and_then(serde_json::Value::as_str)?;
    if p.is_empty() || m.is_empty() {
        return None;
    }
    Some((ProviderId::from(p), ModelId::from(m)))
}

/// What [`AgentSession::prepare`] resolved a submission to (the shared `prompt` preflight outcome).
enum Prepared {
    /// Assembled run input to dispatch to the agent.
    Run(Vec<AgentMessage>),
    /// An `input` handler serviced it; nothing to run.
    Handled,
    /// The agent is streaming; the (expanded) submission is queued via the carried behavior.
    Queued(StreamingBehavior, UserInput),
}

/// The build-time inputs the facade threads into [`AgentSession::from_parts`] for the subsystems
/// added beyond the core seam (retry/auto-compaction/bash/dynamic-tools/attribution). Grouped to
/// keep the constructor signature bounded.
pub(crate) struct SessionExtras {
    pub telemetry_enabled: bool,
    pub compaction_settings: CompactionSettings,
    pub branch_summary_settings: BranchSummarySettings,
    pub auto_compaction_enabled: bool,
    pub auto_retry_enabled: bool,
    pub retry_max_retries: u32,
    pub retry_base_delay_ms: u64,
    pub proc: Arc<dyn ProcOps>,
    pub shell: ShellConfig,
    /// `shellPath` setting (Pi `getShellPath`, settings-manager.ts:864-865); resolved fresh on
    /// every immediate-bash call (see [`AgentSession::execute_bash`]), never baked in at build time.
    pub shell_path: Option<String>,
    /// `shellCommandPrefix` setting (Pi `getShellCommandPrefix`, settings-manager.ts:895-896).
    pub shell_command_prefix: Option<String>,
    /// Shared with the session's [`crate::host_services::LiveHostServices`] backend so a loaded
    /// guest's `setActiveTools`/`getActiveTools` capability operates on the ONE authoritative
    /// active-tool view (Pi `getActiveTools`/`setActiveTools` bind to the same `agent.state.tools`,
    /// agent-session.ts:2281,2283).
    pub dynamic_tools: Arc<Mutex<DynamicToolState>>,
    /// The shared self-handle the builder also handed to the persist+fan-out subscriber.
    pub handle: Arc<SessionHandle>,
    /// The live session metadata the `bash` tool publishes to every child as `CYRUP_*`
    /// (Pi `resolveSpawnContext`, bash.ts:171-181). Shared with the `bash` tool the builder
    /// registered; [`AgentSession`] mutates it whenever the model or the thinking level changes, so
    /// the NEXT command sees the new values without a rebuild
    /// (Pi docs/environment-variables.md:27).
    pub bash_session_env: cyrup_tools::config::SessionEnvHandle,
    /// `read`'s view of whether the ACTIVE model accepts image input, re-pushed on every `/model`
    /// switch so the tool's non-vision warning tracks the live model rather than the startup one.
    pub read_model_vision: cyrup_tools::config::ModelVisionHandle,
}

/// A settable, weak self-reference so the persist+fan-out subscriber (which the agent owns) and the
/// post-run driver task can reach the `Arc<AgentSession>` that owns them. The owning `Arc` is created
/// by the caller (runtime / `into_shared`) AFTER `from_parts` returns, so the weak is filled in then
/// via [`AgentSession::into_shared`]. While unset (a plain by-value `AgentSession`), the post-run loop
/// is inert and the subscriber falls back to its legacy persist+fan-out-only behavior.
#[derive(Default)]
pub(crate) struct SessionHandle {
    weak: OnceLock<Weak<AgentSession>>,
}

impl SessionHandle {
    /// Upgrade to the owning session, if it was bound and is still alive.
    pub(crate) fn get(&self) -> Option<Arc<AgentSession>> {
        self.weak.get().and_then(Weak::upgrade)
    }
}

/// The integration seam (arch-11 §3.1). Cheaply shareable via `Arc`; every method is `&self`.
pub struct AgentSession {
    agent: Arc<Agent>,
    manager: Arc<AsyncMutex<SessionManager>>,
    fanout: Arc<Fanout>,
    /// The swappable stream source the agent loop streams through (`ProviderSwap`). Holds the
    /// currently-installed provider (faux offline default, or a resolved real provider) and is
    /// mutated in place on a cross-provider `/model` select so the agent streams against the new
    /// provider without rebuilding — 1:1 with Pi's live model+provider switch.
    provider: Arc<ProviderSwap>,
    services: AgentSessionServices,
    /// The active model address (mutated by `set_model`), or `None` when the session launched with
    /// no model at all — pi `AgentSession.model: Model | undefined` (agent-session.ts:866-868),
    /// which is `undefined` whenever `findInitialModel` found nothing to select
    /// (sdk.ts:216-218, model-resolver.ts:648-650). This is the state a credential-less first run
    /// starts in so the TUI can offer `/login` and then `/model` (SEAM-075).
    model: Mutex<Option<ModelRef>>,
    /// The resolved summarization/compaction model (kept in lockstep with `model`); `None` in the
    /// same modelless state.
    compaction_model: Mutex<Option<cyrup_provider::Model>>,
    /// The LIVE base system prompt — the value a run falls back to when no `before_agent_start`
    /// handler replaced it (Pi `private _baseSystemPrompt`, agent-session.ts:371).
    ///
    /// Seeded from the builder-assembled `services.system_prompt`, but MUTABLE thereafter: a
    /// tool-set rebuild rewrites it ([`Self::push_active_tools`]), exactly as Pi reassigns
    /// `this._baseSystemPrompt = this._rebuildSystemPrompt(validToolNames)` inside
    /// `setActiveToolsByName` (agent-session.ts:939). `services.system_prompt` is owned by value on
    /// an all-`&self` type and so is frozen at build time; reading the reset path from it made every
    /// run with a `before_agent_start` subscriber revert the prompt to the startup tool set.
    base_system_prompt: Mutex<String>,
    /// The `before_agent_start` handler's replacement prompt for the CURRENT run, or `None` when no
    /// handler replaced it — Pi `private _systemPromptOverride?: string` (agent-session.ts:373
    /// @v0.83.0). Assigned at `:1247`, cleared at `:1251`, and cleared again in `_runAgentPrompt`'s
    /// `finally` (`:1069`) so it never outlives its run. Every site that writes
    /// `agent.state.systemPrompt` resolves `this._systemPromptOverride ?? this._baseSystemPrompt`
    /// (`:534`, `:940`) — [`Self::effective_system_prompt`].
    ///
    /// Holding it apart from [`Self::base_system_prompt`] is the whole point: it is what lets the
    /// turn-boundary refresh re-push a system prompt WITHOUT undoing a handler's mid-run
    /// sanitization, which is why cyrup's single-slot version could not push one at all (DRIFT-033).
    system_prompt_override: Mutex<Option<String>>,
    compaction_settings: CompactionSettings,
    branch_summary_settings: BranchSummarySettings,
    /// Long-lived token handed to the extension subscriber (distinct from per-run cancellation).
    session_cancel: CancelToken,
    session_id: SessionId,
    /// Latches `true` the first time a `--mode json` run writes the session header (Pi
    /// `sessionManager.getHeader()` → JSONL line 1, print-mode.ts:112-117). Pi writes the header
    /// exactly ONCE, before the whole message loop in `runPrintMode`; cyrup replays follow-up
    /// prompts through additional [`crate::AgentSession::prompt`]-scoped `run_json` calls, so the
    /// header emitter ([`Self::claim_json_header`]) consults this latch to stay one-shot per session.
    json_header_written: AtomicBool,
    /// Latches `true` the first time this session is announced with `session_start`. Pi emits its
    /// `_sessionStartEvent` exactly once per `AgentSession` (agent-session.ts:2250, reached from
    /// `bindExtensions`); the latch keeps that one-shot contract when a host binds a session the
    /// runtime already announced.
    start_announced: AtomicBool,
    /// Facade-side mirror of the steering queue text (Pi `_steeringMessages`, agent-session.ts:476)
    /// for `queue_update` emission + introspection; the authoritative queue lives in the agent.
    steering_messages: Mutex<Vec<String>>,
    /// Facade-side mirror of the follow-up queue text (Pi `_followUpMessages`, agent-session.ts:477).
    follow_up_messages: Mutex<Vec<String>>,
    /// Warning surfaced when a resumed session's saved model could not be restored (Pi
    /// `modelFallbackMessage`, sdk.ts:91/192). `None` when the model resolved cleanly.
    model_fallback_message: Option<String>,
    /// Cancel handle for an in-flight manual compaction (Pi `_compactionAbortController`,
    /// agent-session.ts:1654); set while [`Self::compact`] runs, cleared in its `finally`.
    compaction_cancel: Mutex<Option<CancelToken>>,
    /// Cancel handle for an in-flight branch summarization (Pi `_branchSummaryAbortController`,
    /// agent-session.ts:1796).
    branch_summary_cancel: Mutex<Option<CancelToken>>,
    /// Messages staged to ride the NEXT prompt turn (Pi `_pendingNextTurnMessages`,
    /// agent-session.ts:1339); drained into the run by [`Self::assemble_run_messages`].
    pending_next_turn: Mutex<Vec<AgentMessage>>,
    /// Models available for `cycle_model` (Pi `_scopedModels`, agent-session.ts:870).
    scoped_models: Mutex<Vec<ScopedModel>>,
    /// Facade mirror of the agent's steering-queue mode (the agent exposes only a setter; Pi reads
    /// `agent.steeringMode`, agent-session.ts:845).
    steering_mode: Mutex<cyrup_agent::QueueMode>,
    /// Facade mirror of the agent's follow-up-queue mode (Pi `agent.followUpMode`, :850).
    follow_up_mode: Mutex<cyrup_agent::QueueMode>,
    /// Whether provider install-telemetry is on (gates default attribution headers, Pi sdk.ts:323).
    telemetry_enabled: bool,
    // ---- retry subsystem (Pi agent-session.ts:778,2484-2572) ----
    /// Current retry attempt (0 when not retrying; Pi `_retryAttempt`).
    retry_attempt: Mutex<u32>,
    /// Cancel handle for the in-flight backoff sleep (Pi `_retryAbortController`).
    retry_cancel: Mutex<Option<CancelToken>>,
    /// Runtime override of the settings `retry.enabled` toggle (Pi `setAutoRetryEnabled`).
    auto_retry_override: Mutex<Option<bool>>,
    /// `retry.enabled` default sourced from settings at build time.
    retry_enabled_default: bool,
    retry_max_retries: u32,
    retry_base_delay_ms: u64,
    // ---- auto-compaction (Pi agent-session.ts:831,1811-1905,2078-2086) ----
    /// Runtime override of the settings `compaction.enabled` toggle (Pi `setAutoCompactionEnabled`).
    auto_compaction_override: Mutex<Option<bool>>,
    auto_compaction_enabled_default: bool,
    /// Cancel handle for an in-flight auto-compaction (Pi `_autoCompactionAbortController`).
    auto_compaction_cancel: Mutex<Option<CancelToken>>,
    /// Set once after an overflow auto-compaction so a second overflow does not loop (Pi
    /// `_overflowRecoveryAttempted`, agent-session.ts:1859).
    overflow_recovery_attempted: Mutex<bool>,
    // ---- immediate-bash seam (Pi agent-session.ts:2582-2684) ----
    proc: Arc<dyn ProcOps>,
    shell: ShellConfig,
    shell_path: Option<String>,
    shell_command_prefix: Option<String>,
    /// Cancel handles for the in-flight `execute_bash` calls, one entry per call — Pi
    /// `private readonly _bashAbortControllers = new Set<AbortController>()`
    /// (agent-session.ts:337 @v0.83.0). Pi keys the set on `AbortController` object IDENTITY, not on
    /// the caller-supplied `options.id` (which is optional and may repeat), so cyrup keys it on a
    /// private monotonic handle minted per call by [`Self::next_bash_cancel_id`]. It was a single
    /// `Option<CancelToken>` slot: with two user bash commands in flight the second overwrote the
    /// first, so `abort_bash` reached only the newest and the first to finish cleared the slot while
    /// the other still ran (DRIFT-029).
    bash_cancels: Mutex<Vec<(u64, CancelToken)>>,
    /// Source of the identity handles in [`Self::bash_cancels`]; see that field.
    next_bash_cancel_id: std::sync::atomic::AtomicU64,
    /// Bash messages deferred while a run streams, flushed after the turn (Pi `_pendingBashMessages`).
    pending_bash: Mutex<Vec<AgentMessage>>,
    /// The live session metadata the `bash` TOOL publishes to every child as `CYRUP_*` (Pi
    /// `resolveSpawnContext`, bash.ts:171-181). The same handle the builder gave the registered
    /// `BashTool`; Pi reads these off a per-call `ExtensionContext`, so they track the session
    /// automatically. cyrup's `Tool::execute` takes no context, so the values are PUSHED here
    /// whenever the model or the thinking level changes — which is what makes "the values are
    /// resolved when each command starts. Switching models or changing the reasoning level
    /// therefore affects the next bash command" (docs/environment-variables.md:27) true of cyrup
    /// too, rather than only of the tool in isolation.
    bash_session_env: cyrup_tools::config::SessionEnvHandle,
    read_model_vision: cyrup_tools::config::ModelVisionHandle,
    // ---- dynamic tools (Pi agent-session.ts:786-828,2304) ----
    /// Shared (`Arc`) with [`crate::host_services::LiveHostServices`] so a live wasm guest's
    /// `setActiveTools`/`getActiveTools` and the host/CLI tool-toggle read+mutate the SAME state.
    dynamic_tools: Arc<Mutex<DynamicToolState>>,
    // ---- post-run execution loop (Pi `_runAgentPrompt`/`_handlePostAgentRun`,
    //      agent-session.ts:973-1022; the assembled-run driver) ----
    /// Weak self-reference, bound by [`Self::into_shared`]; shared with the persist+fan-out
    /// subscriber so both the subscriber's `_handleAgentEvent` work and the spawned post-run driver
    /// can reach `Arc<AgentSession>`.
    handle: Arc<SessionHandle>,
    /// The last assistant message of the in-flight run (Pi `_lastAssistantMessage`,
    /// agent-session.ts:510): set by the subscriber on every assistant `message_end`, taken by the
    /// driver in [`Self::handle_post_agent_run`].
    last_assistant: Mutex<Option<AssistantMessage>>,
    /// `true` while a post-run driver task owns the run (Pi has no analogue — cyrup spawns the loop in
    /// the background because `prompt` returns an event stream rather than awaiting the whole run).
    /// [`Self::wait_for_idle`] waits on this so a one-shot caller sees the WHOLE loop settle, not just
    /// the first `agent_end`.
    driver_tx: tokio::sync::watch::Sender<bool>,
    /// A keep-alive receiver so `driver_tx.send` never fails for want of a live receiver (a watch
    /// `Sender` with zero receivers drops the sent value); `wait_for_idle` subscribes fresh ones.
    _driver_keepalive: tokio::sync::watch::Receiver<bool>,
    // ---- extension control sinks (SEAM-003 / EXT-005) ----
    /// The RUNTIME-tier control sink (Pi `ExtensionCommandContextActions`, extensions/types.ts:
    /// 1652-1672), installed by [`crate::AgentSessionRuntime`] before this session is announced.
    /// `None` on a bare session built straight from [`crate::SessionBuilder`]; a runtime-tier op
    /// then surfaces [`SessionServiceError::NoRuntimeHost`] as a warning diagnostic rather than being
    /// silently dropped — Pi's pre-bind stubs throw `"Extension runtime not initialized…"` for the
    /// same reason (extensions/loader.ts:173-176 `notInitialized`).
    runtime_actions: OnceLock<Arc<dyn crate::runtime::RuntimeActions>>,
    /// Latches `true` when a loaded extension calls `ctx.shutdown()` (Pi `ctx.shutdown()`,
    /// extensions/types.ts:344 → `runner.shutdown()` → the host's `shutdownHandler`,
    /// rpc-mode.ts:344-346). Hosts poll [`Self::shutdown_requested`] at their next settle point —
    /// which is exactly what `agent_settled` is for (Pi rpc-mode.ts:355-358).
    shutdown_requested: AtomicBool,
}

impl AgentSession {
    /// Build from the assembled parts (called by [`crate::SessionBuilder::build`]).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_parts(
        agent: Arc<Agent>,
        manager: Arc<AsyncMutex<SessionManager>>,
        fanout: Arc<Fanout>,
        provider: Arc<ProviderSwap>,
        services: AgentSessionServices,
        model: Option<ModelRef>,
        session_cancel: CancelToken,
        session_id: SessionId,
        model_fallback_message: Option<String>,
        extras: SessionExtras,
    ) -> Self {
        let compaction_model = services.model.clone();
        let base_system_prompt = services.system_prompt.clone();
        // Seed the queue-mode mirrors from the resolved settings (the builder wired the same modes
        // into the agent), so the getters report the live mode without an agent-side getter.
        let eff = services.settings.effective();
        let steering_mode = crate::builder::parse_queue_mode(&eff.steering_mode());
        let follow_up_mode = crate::builder::parse_queue_mode(&eff.follow_up_mode());
        // The post-run-driver liveness channel; the keep-alive receiver keeps `send` from failing for
        // want of a live receiver (see field docs).
        let (driver_tx_init, driver_keepalive) = tokio::sync::watch::channel(false);
        Self {
            agent,
            manager,
            fanout,
            provider,
            services,
            model: Mutex::new(model),
            compaction_model: Mutex::new(compaction_model),
            base_system_prompt: Mutex::new(base_system_prompt),
            system_prompt_override: Mutex::new(None),
            compaction_settings: extras.compaction_settings,
            branch_summary_settings: extras.branch_summary_settings,
            session_cancel,
            session_id,
            json_header_written: AtomicBool::new(false),
            start_announced: AtomicBool::new(false),
            steering_messages: Mutex::new(Vec::new()),
            follow_up_messages: Mutex::new(Vec::new()),
            model_fallback_message,
            compaction_cancel: Mutex::new(None),
            branch_summary_cancel: Mutex::new(None),
            pending_next_turn: Mutex::new(Vec::new()),
            scoped_models: Mutex::new(Vec::new()),
            steering_mode: Mutex::new(steering_mode),
            follow_up_mode: Mutex::new(follow_up_mode),
            telemetry_enabled: extras.telemetry_enabled,
            retry_attempt: Mutex::new(0),
            retry_cancel: Mutex::new(None),
            auto_retry_override: Mutex::new(None),
            retry_enabled_default: extras.auto_retry_enabled,
            retry_max_retries: extras.retry_max_retries,
            retry_base_delay_ms: extras.retry_base_delay_ms,
            auto_compaction_override: Mutex::new(None),
            auto_compaction_enabled_default: extras.auto_compaction_enabled,
            auto_compaction_cancel: Mutex::new(None),
            overflow_recovery_attempted: Mutex::new(false),
            proc: extras.proc,
            shell: extras.shell,
            shell_path: extras.shell_path,
            shell_command_prefix: extras.shell_command_prefix,
            bash_cancels: Mutex::new(Vec::new()),
            next_bash_cancel_id: std::sync::atomic::AtomicU64::new(0),
            pending_bash: Mutex::new(Vec::new()),
            bash_session_env: extras.bash_session_env,
            read_model_vision: extras.read_model_vision,
            dynamic_tools: extras.dynamic_tools,
            handle: extras.handle,
            last_assistant: Mutex::new(None),
            driver_tx: driver_tx_init,
            _driver_keepalive: driver_keepalive,
            runtime_actions: OnceLock::new(),
            shutdown_requested: AtomicBool::new(false),
        }
    }

    /// Install the RUNTIME-tier control sink (SEAM-003). Called by [`crate::AgentSessionRuntime`] on
    /// every session it owns — the initial one before `bind_extensions()`, each replacement before
    /// its `session_start` — mirroring Pi, which passes `commandContextActions` INTO
    /// `bindExtensions` and re-passes it on every `rebindSession` (rpc-mode.ts:341-346). Idempotent:
    /// the first install wins (a host that also binds cannot displace the runtime's sink).
    pub fn install_runtime_actions(&self, actions: Arc<dyn crate::runtime::RuntimeActions>) {
        let _ = self.runtime_actions.set(actions);
    }

    /// Whether a loaded extension has asked the host to exit (Pi `ctx.shutdown()` → the host's
    /// `shutdownHandler` setting `shutdownRequested = true`, rpc-mode.ts:344-346). A host checks
    /// this at its settle point — see `AgentSessionEvent::AgentSettled` (SEAM-005), which is where
    /// Pi checks it (rpc-mode.ts:355-358).
    /// ORs two latches on purpose. The backend's is set SYNCHRONOUSLY inside
    /// `HostServices::control(ControlOp::Shutdown)` (Pi's `shutdownHandler`, rpc-mode.ts:344-346);
    /// this session's own is set by the turn-boundary control drain. Reading only the latter made
    /// the answer depend on whether a turn boundary happened to follow the request — a shutdown
    /// asked for from a background task on an idle session, or in the window after the in-flight
    /// run's drain had already run, stayed queued forever and the host never exited.
    pub fn shutdown_requested(&self) -> bool {
        self.shutdown_requested.load(Ordering::SeqCst)
            || self.services.host_services.shutdown_requested()
    }

    /// Wrap a freshly-built session in its owning `Arc` and bind the self-handle so the persist+fan-out
    /// subscriber and the post-run driver can reach it (arch-11; the assembled-run wiring). MUST be used
    /// instead of a bare `Arc::new` wherever the post-run execution loop has to run (the runtime, print
    /// mode, the SDK) so auto-retry / post-run auto-compaction / the queued-continuation actually fire
    /// from a completed turn.
    pub fn into_shared(self) -> Arc<Self> {
        let handle = self.handle.clone();
        let arc = Arc::new(self);
        let _ = handle.weak.set(Arc::downgrade(&arc));
        // Bind the late-bound message-injection sink (R-SA-101 / P-2): a background task calling
        // `LiveHostServices::inject_message` (e.g. cyrup-ext-subagents' completion sink, or a native
        // extension holding the P-1 host-services Arc) reaches THIS live session's turn loop. The sink
        // upgrades a weak self-handle and spawns the async inject/turn on the captured runtime, so the
        // SYNC caller never blocks for the whole turn. Bound only on a shared session — a by-value
        // session has no post-run driver to run the turn anyway. If `into_shared` runs outside a tokio
        // runtime (some by-value tests), the captured handle is `None` and the sink degrades to an
        // `Err` rather than panicking (workspace denies `panic`).
        let weak = Arc::downgrade(&arc);
        let runtime = tokio::runtime::Handle::try_current().ok();
        arc.services.host_services.set_inject_sink(Arc::new(move |msg: InjectMessage| {
            let session = weak.upgrade().ok_or("inject_message: session dropped")?;
            let runtime = runtime.clone().ok_or("inject_message: no runtime to inject on")?;
            runtime.spawn(async move {
                let _ = session
                    .inject_message(msg.content, msg.custom_type, msg.display, msg.trigger_turn)
                    .await;
            });
            Ok(())
        }));
        // EXT-005: give the capability backend a LIVE readback of run activity + a real interrupt,
        // so a guest's `ctx.isIdle()`/`ctx.hasPendingMessages()` answer from this session and its
        // `ctx.abort()` stops the run that is in flight (Pi binds all three straight to the session,
        // agent-session.ts:2409-2419). Weak, so the backend never keeps the session alive.
        arc.services
            .host_services
            .attach_session_activity(Arc::new(SessionActivityHandle(Arc::downgrade(&arc))));
        // EXT-037/EXT-038: give the capability backend the LIVE introspection catalog behind a
        // guest's `getCommands()` and the extension-tool provenance half of its `getAllTools()`. Pi
        // binds both straight to the session in `_bindExtensionCore` (agent-session.ts:2394,2397),
        // and only the session can see the prompt templates + skills `getCommands()` concatenates.
        // Weak, so the backend never keeps the session alive.
        arc.services
            .host_services
            .attach_session_catalog(Arc::new(SessionCatalogHandle(Arc::downgrade(&arc))));
        // AGENT-029: install pi's per-request `transformHeaders` (sdk.ts:312-328 @v0.83.0,
        // byte- and offset-identical at v0.84.1). pi merges provider-attribution + session-affinity
        // headers inside a callback closed over the `model` argument of THAT `streamSimple` call —
        // i.e. the model the loop chose for that turn (`agent-loop.ts:308`, whose `config.model` is
        // the possibly-overridden `nextTurnSnapshot.model ?? config.model` from `:237`). cyrup
        // latched them into `StateInner::headers`, written only by the two SESSION-level model-change
        // paths, so a per-turn `TurnUpdate::model` override retargeted the request while the previous
        // provider's attribution rode along. Weak, so the resolver never keeps the session alive.
        let hw = Arc::downgrade(&arc);
        arc.agent.set_header_fn(Some(Arc::new(move |m: &ModelRef| {
            hw.upgrade().and_then(|s| s.headers_for_model_ref(m))
        })));
        arc
    }

    /// Whether NO agent work is in flight (Pi `isIdle`, agent-session.ts:759). True only when both
    /// the post-run driver loop (retry / auto-compaction / queued continuation) and the agent's own
    /// run have settled — the same two latches [`Self::wait_for_idle`] waits on, read without
    /// awaiting so the SYNC `ctx.isIdle()` host import can answer.
    pub fn is_idle(&self) -> bool {
        !*self.driver_tx.borrow() && !self.agent.is_running()
    }

    /// Whether the session is processing an agent run **or a post-run continuation** — pi's
    /// `_isAgentRunActive` (`packages/coding-agent/src/core/agent-session.ts:313` @v0.83.0), set at
    /// the top of `_runAgentPrompt` at `:1062` and cleared only in `_emitAgentSettled` at `:582`, so
    /// it spans `_handlePostAgentRun()` and every `agent.continue()`.
    ///
    /// AGENT-030: this — not the agent's per-run streaming flag — is what pi's `get isStreaming()`
    /// returns (`:876-877`) and what `prompt()` consults at `:1159` to route a submission to
    /// `_queueSteer` / `_queueFollowUp` instead of starting a run. cyrup's
    /// [`Self::is_streaming`] reads `agent.snapshot().is_streaming`, which `SettlementGuard::drop`
    /// clears the moment each INDIVIDUAL run settles, so a prompt landing in the post-run gap (an
    /// auto-retry, an auto-compaction, a queued continuation) started a SECOND run that raced
    /// `drive_run`'s `continue_run()`.
    ///
    /// The two latches are the exact complement of [`Self::is_idle`]: `driver_tx` covers the whole
    /// post-run loop on a BOUND session, and `agent.is_running()` covers an unbound session, where
    /// `spawn_run` drives `agent.prompt` directly and no driver loop exists.
    pub fn is_run_active(&self) -> bool {
        !self.is_idle()
    }

    /// Lock a `std::sync::Mutex` ignoring poisoning (no panic; arch-00 no-panic).
    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    // -------------------------------------------------------------- subscriptions ----

    /// A long-lived event subscription (TUI / SDK observer) — lives until the stream is dropped.
    pub fn subscribe(&self) -> EventStream<AgentSessionEvent> {
        self.fanout.subscribe_persistent()
    }

    // ------------------------------------------------------------------- prompting ----

    /// Submit a user prompt and observe the run as a stream of [`AgentSessionEvent`] (R-11-005/007).
    ///
    /// The returned stream terminates after the run's `agent_end`. Errors only if the prompt could
    /// not be *accepted* (e.g. the agent is already streaming — use [`Self::steer`]/[`Self::follow_up`]).
    pub async fn prompt(
        &self,
        input: impl Into<UserInput>,
    ) -> Result<EventStream<AgentSessionEvent>, SessionServiceError> {
        // AGENT-030: the session-level run latch, not the agent's per-run streaming flag — pi's
        // `prompt()` consults `this.isStreaming`, which IS `_isAgentRunActive`
        // (agent-session.ts:876-877 / :1159 @v0.83.0). See [`Self::is_run_active`].
        if self.is_run_active() {
            return Err(SessionServiceError::StreamingNeedsBehavior);
        }
        // Register the run-scoped subscription BEFORE starting the run so no event is missed.
        let stream = self.fanout.subscribe_run();
        match self.prepare(input.into(), PromptOptions::default()).await? {
            Prepared::Run(messages) => {
                self.spawn_run(messages).await?;
                Ok(stream)
            }
            // An `input` handler serviced the submission (no run started); the stream stays idle.
            Prepared::Handled | Prepared::Queued(..) => Ok(stream),
        }
    }

    /// Submit a prompt, resolving only to the preflight acceptance (mirrors Pi). The run is observed
    /// via [`Self::subscribe`]. Used by adapters that manage their own persistent subscription.
    pub async fn prompt_accepted(
        &self,
        input: impl Into<UserInput>,
    ) -> Result<PromptAccepted, SessionServiceError> {
        self.prompt_with(input, PromptOptions::default()).await
    }

    /// Submit a prompt with per-call [`PromptOptions`] (Pi `prompt(text, options)`,
    /// agent-session.ts:998). Closes the in-`prompt` `streamingBehavior` seam (gap `#13`): while the
    /// agent is streaming, the (template-expanded) text is queued via steer/follow-up per
    /// `streaming_behavior` instead of being rejected, exactly as Pi does at agent-session.ts:1043-
    /// 1056. The `Result` itself is the `preflightResult` callback (`Ok` = accepted, `Err` = the
    /// preflight throw). An `input` extension handler may fully service the submission, yielding
    /// [`PromptAccepted::Handled`].
    pub async fn prompt_with(
        &self,
        input: impl Into<UserInput>,
        options: PromptOptions,
    ) -> Result<PromptAccepted, SessionServiceError> {
        match self.prepare(input.into(), options).await? {
            Prepared::Handled => Ok(PromptAccepted::Handled),
            Prepared::Queued(behavior, ui) => match behavior {
                StreamingBehavior::FollowUp => self.follow_up(ui).await,
                StreamingBehavior::Steer => self.steer(ui).await,
            },
            Prepared::Run(messages) => {
                self.spawn_run(messages).await?;
                Ok(PromptAccepted::Started)
            }
        }
    }

    /// Dispatch an assembled run. A BOUND session (via [`Self::into_shared`]) spawns the post-run
    /// driver task so auto-retry / post-run auto-compaction / queued continuations actually fire from
    /// the completed turn (Pi `_runAgentPrompt`, agent-session.ts:973-985). An unbound by-value session
    /// keeps the legacy behavior: start the run and let the subscriber terminate the run-scoped streams
    /// on `agent_end` (the post-run loop does not run).
    async fn spawn_run(&self, messages: Vec<AgentMessage>) -> Result<(), SessionServiceError> {
        match self.handle.get() {
            Some(this) => {
                // Flag the loop active BEFORE returning so an immediate `wait_for_idle` waits for the
                // WHOLE loop, not just the first `agent_end`.
                let _ = self.driver_tx.send(true);
                tokio::spawn(async move { this.drive_run(messages).await });
                Ok(())
            }
            None => {
                self.agent.prompt(messages).await?;
                Ok(())
            }
        }
    }

    /// The post-run execution loop (Pi `_runAgentPrompt` + `_handlePostAgentRun`,
    /// agent-session.ts:973-1022). Runs the prompt, then — for as long as the post-run handler asks —
    /// drives `agent.continue()` for an auto-retry, a threshold/overflow auto-compaction, or an
    /// `agent_end`-queued continuation. Spawned by [`Self::spawn_run`] on a bound session.
    async fn drive_run(self: Arc<Self>, messages: Vec<AgentMessage>) {
        if let Ok(handle) = self.agent.prompt(messages).await {
            let _ = handle.finished().await;
            // GAP-11: apply the event-tier control ops (set_model / set_thinking_level) a guest queued
            // from `on_message_end` / a mid-turn tool hook / `on_agent_end`. This runs at a STORE-FREE
            // point — the whole run's ordered subscriber dispatch has returned, so every
            // `LiveExtension.inner` store guard is released and the drain's `thinking_level_select` /
            // `model_select` re-emit is a fresh top-level guest call, never a re-entry into the
            // suspended event-hook store (see live.rs `set_thinking_level`). This is the "before the
            // next turn" point the control queue promises, so the SUBSEQUENT `continue_run` (and the
            // next user turn) reads the new `agent.model` / `thinking_level`. Uses the `Send`-safe
            // focused drain (not the full `apply_pending_control`) because this future is spawned:
            // only SetModel/SetThinkingLevel can reach the queue from an event handler.
            self.apply_pending_agent_control().await;
            while self.handle_post_agent_run().await {
                match self.agent.continue_run().await {
                    Ok(h) => {
                        let _ = h.finished().await;
                        // Same store-free turn-boundary drain after each continuation settles.
                        self.apply_pending_agent_control().await;
                    }
                    Err(_) => break,
                }
            }
        }
        // Pi `_runAgentPrompt`'s `finally` opens with `this._systemPromptOverride = undefined;`
        // (agent-session.ts:1069 @v0.83.0), BEFORE the bash flush and the settle emit — a
        // `before_agent_start` replacement is scoped to its own run and must not survive into the
        // next one (DRIFT-033).
        *Self::lock(&self.system_prompt_override) = None;
        // Pi `finally` (agent-session.ts:982-984): flush deferred bash messages from this turn.
        self.flush_pending_bash_messages().await;
        // SEAM-005: the run has FULLY settled — the post-run loop above is done, so no retry,
        // compaction or queued continuation will follow. This is exactly Pi's `_emitAgentSettled()`
        // call site: the `finally` of `_runAgentPrompt` (agent-session.ts:1063-1072), AFTER
        // `_flushPendingBashMessages()` and BEFORE the idle wait resolves.
        self.emit_agent_settled().await;
        // Terminate the run-scoped subscriptions returned by `prompt` now the whole loop has
        // settled. Ordered AFTER the settle emit so a run-scoped subscriber (what `prompt` hands
        // back) actually observes `agent_settled` as its last event.
        self.fanout.end_run();
        // Pi's `_resolveIdleWaitIfIdle()` runs in `_emitAgentSettled`'s own `finally` — i.e. the
        // idle wait releases only after the event has been delivered. `driver_tx` is cyrup's idle
        // latch, so it drops last.
        let _ = self.driver_tx.send(false);
    }

    /// Emit `agent_settled` (Pi `_emitAgentSettled`, agent-session.ts:581-588) — to the EXTENSION
    /// RUNNER first, then to the session subscribers, matching Pi's order exactly
    /// (`await this._extensionRunner.emit(...)` then `this._emit(...)`).
    ///
    /// Fires once per RUN, not once per agent loop: a turn that auto-retries produces two
    /// `agent_end`s and exactly one `agent_settled`. That is the whole reason the event exists —
    /// `agent_end` cannot tell a consumer whether more work is coming, which is why Pi's RPC host
    /// checks its shutdown request here and nowhere else (rpc-mode.ts:355-358).
    pub(crate) async fn emit_agent_settled(&self) {
        let cancel = self.session_cancel.child_token();
        self.services
            .ext_host
            .dispatcher()
            .dispatch_notify(&HostEvent::AgentSettled, &cancel)
            .await;
        self.fanout_emit(AgentSessionEvent::AgentSettled).await;
    }

    /// Decide whether the just-finished run needs a continuation (Pi `_handlePostAgentRun`,
    /// agent-session.ts:986-1013): retry a transient error after backoff, close a spent retry
    /// sequence, run a post-run threshold/overflow compaction, or continue for `agent_end`-queued
    /// messages. Returns `true` when the driver should `agent.continue()`.
    async fn handle_post_agent_run(&self) -> bool {
        let Some(msg) = Self::lock(&self.last_assistant).take() else { return false };
        // Retryable transient error → backoff + continue (Pi :991-993).
        if self.is_retryable_error(&msg) && self.prepare_retry(&msg).await {
            return true;
        }
        // A terminal error with a spent / non-retryable budget closes the retry sequence (Pi :995-1003).
        if msg.stop_reason == cyrup_core::StopReason::Error && self.retry_attempt() > 0 {
            let attempt = std::mem::replace(&mut *Self::lock(&self.retry_attempt), 0);
            self.fanout_emit(AgentSessionEvent::AutoRetryEnd {
                success: false,
                attempt,
                final_error: msg.error_message.clone(),
            })
            .await;
        }
        // Threshold / overflow post-run compaction → continue (Pi :1005-1007).
        if self.check_compaction(&msg, true).await.unwrap_or(false) {
            return true;
        }
        // Messages queued by `agent_end` extension handlers need a continuation (Pi :1009-1012).
        self.agent.has_queued_messages()
    }

    /// The persist+fan-out subscriber's `message_start` handler for a USER message (Pi
    /// `_handleAgentEvent` head, agent-session.ts:514-535): reset the overflow-recovery latch and, when
    /// the message text matches a queued steer/follow-up mirror entry, drop it and emit `queue_update`
    /// as the agent drains the queue.
    pub(crate) async fn on_user_message_start(&self, message: &AgentMessage) {
        *Self::lock(&self.overflow_recovery_attempted) = false;
        let Some(text) = agent_user_text(message) else { return };
        let mut drained = false;
        {
            let mut steer = Self::lock(&self.steering_messages);
            if let Some(pos) = steer.iter().position(|m| *m == text) {
                steer.remove(pos);
                drained = true;
            }
        }
        if !drained {
            let mut fu = Self::lock(&self.follow_up_messages);
            if let Some(pos) = fu.iter().position(|m| *m == text) {
                fu.remove(pos);
                drained = true;
            }
        }
        if drained {
            self.emit_queue_update().await;
        }
    }

    /// The subscriber's `message_end` handler for an ASSISTANT message (Pi `_handleAgentEvent` tail,
    /// agent-session.ts:562-577): track the last assistant message (drives the post-run loop) and — on
    /// a non-error response — clear the overflow latch and reset the retry counter, emitting
    /// `auto_retry_end{success:true}` if a retry sequence was in flight.
    pub(crate) async fn on_assistant_message_end(&self, assistant: &AssistantMessage) {
        *Self::lock(&self.last_assistant) = Some(assistant.clone());
        if assistant.stop_reason == cyrup_core::StopReason::Error {
            return;
        }
        *Self::lock(&self.overflow_recovery_attempted) = false;
        let attempt = {
            let mut at = Self::lock(&self.retry_attempt);
            let v = *at;
            if v > 0 {
                *at = 0;
            }
            v
        };
        if attempt > 0 {
            self.fanout_emit(AgentSessionEvent::AutoRetryEnd {
                success: true,
                attempt,
                final_error: None,
            })
            .await;
        }
    }

    /// The shared preflight Pi's `prompt` performs before either running or queueing
    /// (agent-session.ts:1003-1142): emit the `input` extension event (which may fully service the
    /// submission), then — if the agent is streaming — expand templates and route to the steer/
    /// follow-up queue per `streaming_behavior` (erroring when none is given), else assemble the run
    /// input. Returns the disposition the caller acts on.
    async fn prepare(
        &self,
        mut ui: UserInput,
        options: PromptOptions,
    ) -> Result<Prepared, SessionServiceError> {
        // AGENT-030: pi's whole preflight reads `this.isStreaming` == `_isAgentRunActive`
        // (agent-session.ts:1022 for the `input` event's `streamingBehavior`, `:1159` for the
        // queue routing) — the latch that spans `_handlePostAgentRun` and every `agent.continue()`,
        // not a per-run flag. See [`Self::is_run_active`].
        let streaming = self.is_run_active();
        // 0. Slash extension-command exec FIRST (Pi `_tryExecuteExtensionCommand`,
        //    agent-session.ts:1004-1013): for `expandPromptTemplates && text.startsWith("/")`, if a
        //    registered command name matches, run its handler and short-circuit (no prompt sent).
        //    Matches Pi's order: tried BEFORE the `input` event + before skill/template expansion.
        if ui.expand_templates
            && ui.text.starts_with('/')
            && self.try_execute_extension_command(&ui.text).await
        {
            return Ok(Prepared::Handled);
        }
        // 1. `input` extension event, emitted BEFORE expansion (Pi agent-session.ts:1015-1033). A
        //    handler that returns `handled` fully services the submission — no run, no queue; a
        //    `transform` handler rewrites `ui` (text/images) in place before continuing. The handler
        //    sees `streamingBehavior` only while streaming (Pi `this.isStreaming ? ... : undefined`,
        //    agent-session.ts:1022).
        let handler_behavior = if streaming { options.streaming_behavior } else { None };
        if matches!(
            self.emit_input_event(&mut ui, handler_behavior).await,
            InputDisposition::Handled
        ) {
            return Ok(Prepared::Handled);
        }
        // GAP-11: apply any event-tier control op (set_model / set_thinking_level) an `on_input`
        // handler just queued, at this STORE-FREE point — `emit_input_event` has returned, releasing
        // every `LiveExtension.inner` guard, so the drain's re-emit is a fresh top-level guest call
        // (never a re-entry). This makes an `on_input` `setModel`/`setThinkingLevel` take effect on
        // the turn now being assembled, matching Pi, whose synchronous `on_input` mutation lands
        // before the dispatched turn (agent-session.ts:1015-1033). The focused drain never re-enters
        // `prepare` (unlike the full `apply_pending_control`'s `SendUserMessage` arm), keeping this
        // hot path free of the boxed async-recursion edge.
        self.apply_pending_agent_control().await;
        // 2. While streaming, expand then queue per `streamingBehavior` (Pi agent-session.ts:1043-
        //    1056). Without a behavior the submission is rejected (Pi throws at :1044).
        if streaming {
            let behavior = options
                .streaming_behavior
                .ok_or(SessionServiceError::StreamingNeedsBehavior)?;
            let mut queued = ui;
            if queued.expand_templates {
                queued.text = self.expand_input_text(&queued.text);
            }
            return Ok(Prepared::Queued(behavior, queued));
        }
        // 3. Not streaming: run the full pre-send sequence + assemble the run input.
        Ok(Prepared::Run(self.prepare_and_assemble(ui).await?))
    }

    /// Emit the `input` extension event (Pi `emitInput`, runner.ts:1095). A handler may fully
    /// service the submission (`HookOutcome::Handled`/`Block` ⇒ [`InputDisposition::Handled`]) or
    /// *transform* it (`HookOutcome::Mutate(EventPatch::Input{..})`, Pi `action:"transform"`,
    /// runner.ts:1116-1119): the folded text/images flow back into `ui` and the submission continues
    /// with the rewritten content (Pi agent-session.ts:1029-1032).
    async fn emit_input_event(
        &self,
        ui: &mut UserInput,
        streaming_behavior: Option<StreamingBehavior>,
    ) -> InputDisposition {
        if self.services.ext_host.dispatcher().no_subscribers(cyrup_ext::EventKind::Input) {
            return InputDisposition::Continue;
        }
        let cancel = self.session_cancel.child_token();
        // Deliver the `source` (Pi `InputEvent.source`, agent-session.ts:1021) + the in-flight
        // `streamingBehavior` (`undefined` when idle, :1022) so a handler can branch on
        // interactive-vs-queued / steer-vs-follow-up before deciding (#13c).
        let event = HostEvent::Input {
            text: ui.text.clone(),
            images: ui.images.clone(),
            source: input_event_source(ui.source),
            streaming_behavior: streaming_behavior.map(input_streaming_behavior),
        };
        let reduced = self
            .services
            .ext_host
            .dispatcher()
            .dispatch_block_mutate(event, &cancel)
            .await;
        match reduced {
            Reduced::Handled(_) | Reduced::Blocked { .. } => InputDisposition::Handled,
            // Apply any `transform` the handler chain folded into the event (Pi
            // agent-session.ts:1029-1032: `currentText`/`currentImages` adopt the result).
            Reduced::Pass(ev) => {
                if let HostEvent::Input { text, images, .. } = *ev {
                    ui.text = text;
                    ui.images = images;
                }
                InputDisposition::Continue
            }
        }
    }

    /// Try to execute a registered extension slash command (Pi `_tryExecuteExtensionCommand`,
    /// agent-session.ts:1148-1172). Parses `/<name> <args>`, routes to the owning NATIVE extension
    /// (R-08-016), and runs its command-tier handler. Returns `true` when a command was serviced
    /// (the submission is fully handled — Pi returns `true` even when the handler errors, after
    /// surfacing the error), `false` when no command matched (fall through to normal handling).
    async fn try_execute_extension_command(&self, text: &str) -> bool {
        let body = text.strip_prefix('/').unwrap_or(text);
        let (name, args) = body.split_once(' ').unwrap_or((body, ""));
        if name.is_empty() {
            return false;
        }
        let cancel = self.session_cancel.child_token();
        // NATIVE built-ins first (R-08-016): route to the owning native extension.
        match self.services.ext_host.execute_native_command(name, args, &cancel).await {
            // A native extension owned + serviced the command (Pi short-circuits regardless of the
            // handler's own Ok/Err — the command was "handled").
            Ok(Some(payload)) => {
                // SURFACE THE HANDLER'S OUTPUT. This arm used to bind the payload to `_`.
                //
                // Pi's handler signature is `Promise<void>` and it talks to the user through
                // `ctx.ui.*` (`agent-session.ts:1278-1301` — the return value genuinely is
                // discarded there). cyrup's `NativeExtension::execute_command` instead returns
                // `Result<Option<String>>`, a cyrup-original channel, and its built-ins populate it:
                // `cyrup-ext-subagents` alone answers all 15 of its slash commands this way and
                // contains ZERO `notify` calls. Discarding the payload therefore made every one of
                // those commands silent — `/prompt-workflow list` ran, spawned, and printed nothing;
                // `/permission-system yoloMode on` wrote the config and said nothing. The seam was
                // advertised and unread, which is the same defect class as a mechanism wired to no
                // caller.
                //
                // Routing it to `notify` reproduces pi's OBSERVABLE behaviour (the user sees the
                // command's response) using the one UI channel that is already live end-to-end:
                // `LiveHostServices::notify` -> `UiEffect::Notify` -> the TUI's `showExtensionNotify`.
                // An empty payload stays silent, so a handler that deliberately says nothing still
                // says nothing.
                // The bind is the HANDLER's own `Result` — `execute_native_command` returns
                // `Result<Option<Result<Option<String>, ExtError>>, _>`: outer = routing, `Option` =
                // did a native extension own the name, inner = what the handler itself returned.
                self.surface_command_outcome(name, &payload);
                // SEAM-003: drain the control ops the native handler queued. This route used to
                // `return true` with NO drain at all, so a native built-in's `control(...)` sat in
                // the queue until some later WASM command happened to run. Pi keeps native + wasm
                // commands in one map and runs `commandContextActions` inline for both
                // (agent-session.ts:1183-1200), so both routes must drain identically. Boxed for the
                // same reason the wasm route is: a `SendUserMessage` op re-enters the prompt path.
                Box::pin(self.apply_pending_control()).await;
                return true;
            }
            // No NATIVE owner: the name may still belong to a LIVE wasm guest command. Pi keeps
            // native + wasm commands in ONE map (`getCommand`, agent-session.ts:1183), so both
            // routes are tried before falling through to normal prompt handling.
            Ok(None) => {}
            // Routing failure (e.g. poisoned lock): degrade to "not handled" (never panic).
            Err(_) => return false,
        }
        self.try_execute_wasm_command(name, args, &cancel).await
    }

    /// Surface a slash-command handler's OWN outcome to the user — the one behaviour the native and
    /// the wasm tier must share, factored out so they cannot drift again.
    ///
    /// pi keeps native and wasm commands in ONE map and runs them through a single
    /// `_tryExecuteExtensionCommand` (`core/agent-session.ts:1277-1301` @v0.83.0), so upstream there
    /// is exactly one behaviour and it is the SURFACING one: a thrown handler goes to
    /// `this._extensionRunner.emitError({extensionPath: `command:${commandName}`, event: "command",
    /// error})` (`:1294-1299`) and the command still reports handled. Routing it to `notify`
    /// reproduces that observable behaviour over the one UI channel that is live end-to-end
    /// (`LiveHostServices::notify` → `UiEffect::Notify` → the TUI's `show_extension_notify`, and the
    /// RPC drain's `extension_error` line).
    ///
    /// `Ok(Some(text))` has no pi counterpart — pi's handler signature is `Promise<void>` and it
    /// talks to the user through `ctx.ui.*` — but it is a real cyrup-side channel that the SDK's
    /// `CommandExec` populates (`cyrup-ext-sdk/src/guest.rs`'s `run_command` → `api.execute_command`),
    /// so a guest command that answers with text must speak exactly as the identical native command
    /// already does. An empty payload stays silent.
    pub(crate) fn surface_command_outcome(
        &self,
        name: &str,
        outcome: &Result<Option<String>, cyrup_ext::ExtError>,
    ) {
        match outcome {
            Ok(Some(text)) if !text.trim().is_empty() => {
                cyrup_ext::HostServices::notify(
                    &*self.services.host_services,
                    text,
                    cyrup_ext::NotifyKind::Info,
                );
            }
            // A handler that deliberately returns nothing stays silent.
            Ok(_) => {}
            Err(e) => {
                cyrup_ext::HostServices::notify(
                    &*self.services.host_services,
                    &format!("command:{name}: {e}"),
                    cyrup_ext::NotifyKind::Error,
                );
            }
        }
    }

    /// Execute a LIVE wasm-guest-registered slash command through the real run path (R-08-016; Pi
    /// `command.handler(args, ctx)`, agent-session.ts:1189-1200). Runs the guest's `execute-command`
    /// export at command tier, then drains + applies the session-tier control ops the guest queued
    /// via its `control` capability — Pi runs those inline in the handler's `createCommandContext`
    /// (agent-session.ts:1158); cyrup bridges the SYNC guest `control()` call to the ASYNC session
    /// effect here (arch-08 §6.3, mirrors [`Self::apply_pending_control`]). Returns `true` whenever a
    /// registered guest command was serviced — Pi returns `true` even when the handler throws
    /// (:1192-1200) — and `false` when no guest owns the name (fall through to a normal prompt).
    #[cfg(feature = "wasm-host")]
    async fn try_execute_wasm_command(&self, name: &str, args: &str, cancel: &CancelToken) -> bool {
        // Only a REGISTERED command routes here; an unknown `/name` falls through (Pi `getCommand`
        // returns `undefined` ⇒ `false`, agent-session.ts:1184).
        // SEAM-048 — resolve through the DISAMBIGUATED name, so the `name:2` spelling
        // `slash_command_catalog` advertises is a spelling the dispatcher accepts. pi's `getCommand`
        // matches on `invocationName` (`core/extensions/runner.ts:648`); the bare last-wins
        // `command_owner` did not, so an advertised `check:2` was unreachable.
        if !matches!(
            self.services.ext_host.registry().resolved_command_owner(name),
            Ok(Some(_))
        ) {
            return false;
        }
        // Run the guest handler and SURFACE its outcome, exactly as the native arm above does —
        // this used to be `let _ = …`, discarding both channels. pi treats a thrown handler as still
        // "handled" (`core/agent-session.ts:1292-1300`) but emits the error first; discarding it
        // here meant a trapping guest, an epoch-deadline interrupt, an `ExtError::Cancelled` or the
        // guest's own `execute-command` error return produced NOTHING — no transcript line, no
        // toast, no RPC `extension_error`, no log — while `return true` below still swallowed the
        // input, so the user pressed Enter on `/deploy` and the UI did nothing at all. The `true`
        // stays on every branch: a faulted command must not fall through to being treated as a
        // prompt.
        let outcome = self.services.ext_host.run_command(name, args, cancel).await;
        self.surface_command_outcome(name, &outcome);
        // Apply every control op the guest queued — session-tier (compact / set-model /
        // send-message / set-thinking-level / navigate / wait-idle) AND runtime-tier (new-session /
        // switch / fork / reload), the latter through the installed [`crate::RuntimeActions`] sink
        // (SEAM-003). This used to bind the runtime-tier ops to `_deferred` and drop them. Boxed: a
        // `send_user_message` op re-enters the prompt path (Pi `pi.sendMessage` from a command
        // handler), so the async future must introduce indirection to stay finitely sized.
        Box::pin(self.apply_pending_control()).await;
        true
    }

    /// Native-host fallback (no `wasm-host` feature): no live guest can own a command, so an
    /// unmatched slash falls through to normal prompt handling.
    #[cfg(not(feature = "wasm-host"))]
    async fn try_execute_wasm_command(
        &self,
        _name: &str,
        _args: &str,
        _cancel: &CancelToken,
    ) -> bool {
        false
    }

    /// Run the pre-send sequence Pi's `prompt` performs before dispatching the run
    /// (agent-session.ts:1037-1083): expand skill/prompt-template commands, flush any pending bash
    /// messages, run the `hasConfiguredAuth` precheck, and perform the pre-send compaction check
    /// (which catches an aborted last response). Then assemble the run input (`before_agent_start`
    /// hook + ordering). Returns the assembled run messages. Errors before any persistence on an auth
    /// miss (Pi `_getRequiredRequestAuth` throw → `preflightResult?.(false)`).
    async fn prepare_and_assemble(
        &self,
        mut input: UserInput,
    ) -> Result<Vec<AgentMessage>, SessionServiceError> {
        // 1. Skill (`/skill:name`) + prompt-template (`/name args`) expansion (agent-session.ts:1037).
        if input.expand_templates {
            input.text = self.expand_input_text(&input.text);
        }
        // 2. Flush deferred bash messages so ordering is intact (agent-session.ts:1058).
        self.flush_pending_bash_messages().await;
        // 3. Model + auth precheck. Pi validates the MODEL first —
        // `if (!this.model) { throw new Error(formatNoModelSelectedMessage()); }`
        // (agent-session.ts:1177-1180) — and only then the credential (`:1182-1195`). This is the
        // first turn of a modelless first run (SEAM-075): the answer is the `/login` → `/model`
        // instruction, surfaced as an error on the turn, never a process exit.
        {
            let model = Self::lock(&self.compaction_model)
                .clone()
                .ok_or(SessionServiceError::NoModelSelected)?;
            // PROV-037 — pi's refusal is a THREE-branch decision, not one
            // (`agent-session.ts:1182-1195` @v0.83.0):
            //
            //   const hasConfiguredAuth =
            //       this._modelRuntime.hasConfiguredAuth(this.model.provider) ||
            //       (await this._modelRuntime.checkAuth(this.model.provider)) !== undefined;
            //   if (!hasConfiguredAuth) {
            //       if (this._modelRuntime.isUsingOAuth(...)) throw new Error(`Authentication failed…`);
            //       throw new Error(formatNoApiKeyFoundMessage(this.model.provider));
            //   }
            //
            // cyrup consulted only the cached `has_configured_auth` and reported its own
            // `no configured auth for model: p/m`. Two consequences, both user-visible: a provider
            // whose credential is present but outside the cached configured set was refused where
            // pi re-checks and PROCEEDS, and an expired OAuth token produced a message that named
            // neither the provider nor `/login`.
            if !self.has_configured_auth(&model) && !self.recheck_provider_auth(&model).await {
                let provider = model.provider.as_str();
                return Err(SessionServiceError::AuthPreflightRefused(
                    if self.provider_is_oauth_backed(&model.provider).await {
                        crate::auth_guidance::format_oauth_reauthenticate_message(provider)
                    } else {
                        crate::auth_guidance::format_no_api_key_found_message(provider)
                    },
                ));
            }
        }
        // 4. Pre-send compaction check on the last assistant turn (agent-session.ts:1080-1083).
        if self.auto_compaction_enabled()
            && let Some(last) = self.last_assistant_message().await
        {
            let _ = self.check_compaction(&last, false).await?;
        }
        // 5. Assemble (before_agent_start hook + ordering).
        Ok(self.assemble_run_messages(input).await)
    }

    /// Expand a `/skill:name args` command to the skill block + args, or a `/name args` prompt
    /// template, leaving any other text unchanged (Pi `_expandSkillCommand` + `expandPromptTemplate`,
    /// agent-session.ts:1174-1204,1037-1041).
    fn expand_input_text(&self, text: &str) -> String {
        let expanded = self.expand_skill_command(text);
        let templates: Vec<_> = self.prompt_templates().winners().collect();
        cyrup_resources::expand_prompt_template(&expanded, templates)
    }

    /// `/skill:name args` → the skill block (Pi `_expandSkillCommand`, agent-session.ts:1174). Unknown
    /// skills / read failures pass the text through unchanged.
    fn expand_skill_command(&self, text: &str) -> String {
        let Some(rest) = text.strip_prefix("/skill:") else { return text.to_string() };
        let (name, args) = match rest.find(char::is_whitespace) {
            Some(i) => (&rest[..i], rest[i..].trim()),
            None => (rest, ""),
        };
        let Some(skill) = self.services.resources.skills.winners().find(|s| s.name == name) else {
            return text.to_string();
        };
        let Ok(content) = std::fs::read_to_string(&skill.skill_md) else {
            return text.to_string();
        };
        let body = strip_frontmatter(&content).trim().to_string();
        let block = format!(
            "<skill name=\"{}\" location=\"{}\">\nReferences are relative to {}.\n\n{}\n</skill>",
            skill.name,
            skill.skill_md.display(),
            skill.dir.display(),
            body
        );
        if args.is_empty() {
            block
        } else {
            format!("{block}\n\n{args}")
        }
    }

    /// The most recent assistant message on the current branch as a full [`AssistantMessage`] (for
    /// the compaction/retry checks), or `None`.
    async fn last_assistant_message(&self) -> Option<AssistantMessage> {
        self.messages().await.into_iter().rev().find_map(|m| match m {
            Message::Assistant(a) => Some(a),
            _ => None,
        })
    }

    /// Run the `before_agent_start` extension hook and assemble the run's input messages (R-06-014;
    /// Pi agent-session.ts:1105-1131). The hook chain may (a) **replace** the system prompt — applied
    /// to the agent before the run, and reset to the assembled base when no handler replaced it — and
    /// (b) **inject** additional messages, which are appended after the user message. Without this the
    /// assembled prompt was never offered to extensions (the gap the facade closes).
    async fn assemble_run_messages(&self, input: UserInput) -> Vec<AgentMessage> {
        let user_text = input.text.clone();
        let images = input.images.clone();
        let user_msg = input.into_agent_message();
        // Drain any messages staged for this turn (Pi `_pendingNextTurnMessages`,
        // agent-session.ts:1099-1103); they are injected AFTER the user message in the run input.
        let pending: Vec<AgentMessage> = std::mem::take(&mut *Self::lock(&self.pending_next_turn));

        // The LIVE base (Pi reads the mutable field: `this._baseSystemPrompt`, agent-session.ts:1228
        // into `emitBeforeAgentStart`, :1252 for the reset) — NOT the frozen builder-assembled
        // `services.system_prompt`, which predates every `set_active_tools_by_name` /
        // `refresh_extension_tools` rebuild this session performed.
        let base = self.base_system_prompt();
        // Fast path: no extension listens for `before_agent_start` — keep the assembled base prompt.
        if self.services.ext_host.dispatcher().no_subscribers(cyrup_ext::EventKind::BeforeAgentStart)
        {
            // No handler ran, so there is nothing to override with — pi's `else` branch
            // (agent-session.ts:1251 @v0.83.0) clears the slot for exactly this reason, and a stale
            // override from a PREVIOUS run must not leak into this one.
            *Self::lock(&self.system_prompt_override) = None;
            let mut messages = vec![user_msg];
            messages.extend(pending);
            return messages;
        }

        let event = HostEvent::BeforeAgentStart {
            prompt: user_text,
            images: serde_json::to_value(&images).unwrap_or(serde_json::Value::Null),
            system_prompt: base.clone(),
            options: serde_json::Value::Null,
            injected: Vec::new(),
        };
        let cancel = self.session_cancel.child_token();
        let reduced = self.services.ext_host.dispatcher().dispatch_block_mutate(event, &cancel).await;

        let mut messages = vec![user_msg];
        messages.extend(pending);
        // Pi `setActiveTools` (pi-permission-system index.ts:2155): a `before_agent_start` handler may
        // have RESTRICTED the active tool set via `HostServices::set_active_tools` (the permission
        // companion's `shouldExposeTool` shaping), which stages a `(tools, prompt)` push. Drain + apply
        // it IN-TURN here — before `spawn_run` — so the restriction shapes THIS turn (turn 1), not the
        // next turn boundary where `apply_pending_agent_control` would otherwise pick it up. Apply ONLY
        // the restricted tool ARRAY; the `DynamicToolState`-rebuilt prompt is DISCARDED so it cannot
        // clobber the handler's own sanitized system prompt applied just below (pi's `setActiveTools`
        // and its returned `systemPrompt` are independent). Draining it here also leaves
        // `pending_active_tools` empty for the later `apply_pending_agent_control` drains, so the
        // restriction is applied exactly once.
        if let Some((tools, _rebuilt_prompt)) =
            self.services.host_services.take_pending_active_tools()
        {
            self.agent.set_tools(tools).await;
        }
        if let Reduced::Pass(ev) = reduced
            && let HostEvent::BeforeAgentStart { system_prompt, injected, .. } = *ev
        {
            // Apply the (possibly handler-replaced / sanitized) system prompt; reset to base
            // otherwise. Pi's two branches are `if (result?.systemPrompt !== undefined) {
            // this._systemPromptOverride = result.systemPrompt; this.agent.state.systemPrompt =
            // result.systemPrompt; } else { this._systemPromptOverride = undefined;
            // this.agent.state.systemPrompt = this._baseSystemPrompt; }` (agent-session.ts:1246-1252
            // @v0.83.0) — the OVERRIDE SLOT is written on both, which is what makes the turn-boundary
            // refresh able to re-push `override ?? base` without clobbering this sanitization
            // (DRIFT-033).
            //
            // CYRUP-DELTA on the discriminator only: pi distinguishes "handler returned no prompt"
            // (`undefined`) from "handler returned one"; cyrup's `HostEvent::BeforeAgentStart`
            // carries the prompt as a mutated-in-place `String`, so a handler that returns the base
            // verbatim is indistinguishable from one that returns nothing. Equality with `base` is
            // therefore read as "no override", which agrees with pi on the resulting prompt for
            // every input and differs only in which slot holds the identical text.
            if system_prompt == base {
                *Self::lock(&self.system_prompt_override) = None;
                self.agent.set_system_prompt(base.clone()).await;
            } else {
                *Self::lock(&self.system_prompt_override) = Some(system_prompt.clone());
                self.agent.set_system_prompt(system_prompt).await;
            }
            messages.extend(injected.iter().map(core_message_to_agent));
        } else {
            // Blocked/Handled (no Pi analogue here): keep the base prompt, no injection.
            *Self::lock(&self.system_prompt_override) = None;
            self.agent.set_system_prompt(base.clone()).await;
        }
        messages
    }

    /// Await full settlement of the in-flight run AND its post-run loop (R-11-005). On a bound session
    /// the agent goes briefly idle BETWEEN a completed turn and a retry/compaction continuation, so
    /// this first awaits the post-run driver (`driver_tx` is `true` for the whole loop) and only then
    /// the agent — otherwise a one-shot caller would resume mid-loop.
    pub async fn wait_for_idle(&self) {
        let mut rx = self.driver_tx.subscribe();
        while *rx.borrow_and_update() {
            if rx.changed().await.is_err() {
                break;
            }
        }
        self.agent.wait_for_idle().await;
    }

    /// Enqueue a steering message (delivered after the current tool batch, func-02 §9). Mirrors the
    /// text into the facade queue + emits `queue_update` (Pi `_queueSteer`, agent-session.ts:1249).
    pub async fn steer(&self, input: impl Into<UserInput>) -> Result<PromptAccepted, SessionServiceError> {
        let mut ui = input.into();
        // Pi agent-session.ts:1242-1252: error on an extension command, then expand skill/template
        // BEFORE queueing — the queued text and the mirror must carry the expanded content.
        if ui.expand_templates {
            self.throw_if_extension_command(&ui.text)?;
            ui.text = self.expand_input_text(&ui.text);
        }
        Self::lock(&self.steering_messages).push(ui.text.clone());
        self.agent.steer(ui.into_agent_message());
        self.emit_queue_update().await;
        Ok(PromptAccepted::Queued(StreamingBehavior::Steer))
    }

    /// Enqueue a follow-up message (delivered after the agent goes idle, func-02 §9). Mirrors the
    /// text into the facade queue + emits `queue_update` (Pi `_queueFollowUp`, agent-session.ts:1266).
    pub async fn follow_up(
        &self,
        input: impl Into<UserInput>,
    ) -> Result<PromptAccepted, SessionServiceError> {
        let mut ui = input.into();
        // Pi agent-session.ts:1262-1272: error on an extension command, then expand skill/template
        // BEFORE queueing.
        if ui.expand_templates {
            self.throw_if_extension_command(&ui.text)?;
            ui.text = self.expand_input_text(&ui.text);
        }
        Self::lock(&self.follow_up_messages).push(ui.text.clone());
        self.agent.follow_up(ui.into_agent_message());
        self.emit_queue_update().await;
        Ok(PromptAccepted::Queued(StreamingBehavior::FollowUp))
    }

    /// Error if `text` is a registered extension command (Pi `_throwIfExtensionCommand`,
    /// agent-session.ts:1312-1321): extension commands cannot be queued via `steer`/`follow_up`.
    /// Only `/`-prefixed text is checked; the registry covers native + wasm commands.
    fn throw_if_extension_command(&self, text: &str) -> Result<(), SessionServiceError> {
        let Some(body) = text.strip_prefix('/') else { return Ok(()) };
        let name = body.split_once(' ').map_or(body, |(n, _)| n);
        if self.services.ext_host.registry().has_command(name).unwrap_or(false) {
            return Err(SessionServiceError::ExtensionCommandNotQueueable(name.to_string()));
        }
        Ok(())
    }

    /// The pending steering messages, in order (Pi `getSteeringMessages`, agent-session.ts:1408).
    pub fn steering_messages(&self) -> Vec<String> {
        Self::lock(&self.steering_messages).clone()
    }

    /// The pending follow-up messages, in order (Pi `getFollowUpMessages`, agent-session.ts:1412).
    pub fn follow_up_messages(&self) -> Vec<String> {
        Self::lock(&self.follow_up_messages).clone()
    }

    /// Total queued (steering + follow-up) message count (Pi `pendingMessageCount`,
    /// agent-session.ts:1393).
    pub fn pending_message_count(&self) -> usize {
        Self::lock(&self.steering_messages).len() + Self::lock(&self.follow_up_messages).len()
    }

    /// Clear both queues (Pi `clearQueue`, agent-session.ts:1416): drains the agent's authoritative
    /// queues and the facade mirrors, then emits `queue_update`.
    pub async fn clear_queue(&self) {
        self.agent.clear_all_queues();
        Self::lock(&self.steering_messages).clear();
        Self::lock(&self.follow_up_messages).clear();
        self.emit_queue_update().await;
    }

    /// Take-all both queues and RETURN what was drained, in Pi's `(steering, followUp)` shape
    /// (`AgentSession.clearQueue()` returns `{steering, followUp}`, agent-session.ts:1416 — the
    /// value `restoreQueuedMessagesToEditor` reads at interactive-mode.ts:4065).
    ///
    /// [`Self::clear_queue`] throws that value away, which forces a caller that wants the text to
    /// read `steering_messages()`/`follow_up_messages()` first and clear second — a lost-update race
    /// with a concurrent `steer`/`follow_up`. This is the atomic form: the mirrors and the agent's
    /// authoritative queues are taken in one pass (`Agent::drain_queues_for_restore`), then
    /// `queue_update` is emitted so the footer count drops to zero.
    pub async fn drain_queue(&self) -> (Vec<String>, Vec<String>) {
        // Both mirrors are taken under their guards together so the pair is consistent; the agent
        // drain happens after they are released, keeping the facade→agent lock nesting `steer` /
        // `follow_up` avoid (they too drop the mirror guard before calling into the agent).
        let drained = {
            let mut steering = Self::lock(&self.steering_messages);
            let mut follow_up = Self::lock(&self.follow_up_messages);
            (std::mem::take(&mut *steering), std::mem::take(&mut *follow_up))
        };
        self.agent.drain_queues_for_restore();
        self.emit_queue_update().await;
        drained
    }

    /// Emit a `queue_update` snapshot of both facade queues (Pi `_emitQueueUpdate`,
    /// agent-session.ts:1382).
    async fn emit_queue_update(&self) {
        let steering = Self::lock(&self.steering_messages).clone();
        let follow_up = Self::lock(&self.follow_up_messages).clone();
        self.fanout_emit(AgentSessionEvent::QueueUpdate { steering, follow_up }).await;
    }

    /// Interrupt the active run (idempotent, R-11-018 / func-02 R-02-045).
    ///
    /// SEAM-023 — the retry backoff is cancelled FIRST, exactly as Pi's `abort()` does
    /// (`abortRetry(); this.agent.abort(); await this.waitForIdle();`, agent-session.ts:1542-1546).
    /// `agent.abort()` cancels the PER-RUN token; the auto-retry backoff sleeps on a *separate*
    /// child of `session_cancel` ([`Self::prepare_retry`]), so without this an Escape / SIGINT /
    /// RPC `abort` landing during provider-retry backoff left the backoff running and the retry
    /// fired later against a session the user had already aborted.
    ///
    /// This is the SYNCHRONOUS half (what a signal handler and `ctx.abort()` need). Callers that
    /// must observe the run actually settle — teardown, compaction, the RPC `abort` verb — use
    /// [`Self::abort_and_settle`], which adds Pi's `await this.waitForIdle()` tail.
    pub fn abort(&self) {
        self.abort_retry();
        self.agent.abort();
    }

    /// Interrupt the active run **and await its settlement** — the full Pi `abort()`
    /// (agent-session.ts:1542-1546: `this.abortRetry(); this.agent.abort(); await
    /// this.waitForIdle();`), in that exact order.
    ///
    /// SEAM-024. The order is load-bearing and the reason this is not simply
    /// `wait_for_idle().await` after a plain abort: the retry backoff sleeps on a child of
    /// `session_cancel` that `agent.abort()` does not touch, so awaiting idle BEFORE cancelling it
    /// would block for the whole remaining backoff (up to `retry.baseDelayMs * 2^attempt`).
    ///
    /// Pi's `teardownCurrent` states why teardown must await: "Settle any active response first so
    /// the aborted turn (including tool results) is persisted to the outgoing session before it is
    /// replaced" (agent-session-runtime.ts:167-169), and its RPC `abort` verb likewise replies only
    /// after `await session.abort()` (rpc-mode.ts:427-430).
    ///
    /// Unlike Pi the wait is BOUNDED ([`ABORT_SETTLE_TIMEOUT`]): a wedged tool must not make `quit`
    /// hang forever. On expiry the caller proceeds exactly as the old fire-and-forget `abort()` did.
    pub async fn abort_and_settle(&self) {
        self.abort();
        let _ = tokio::time::timeout(ABORT_SETTLE_TIMEOUT, self.wait_for_idle()).await;
    }

    // ------------------------------------------------------------------- compaction ----

    /// Trigger a compaction of the current branch (R-11-014 `compact`; Pi `compact`,
    /// agent-session.ts:1647-1788). Aborts any active run first, emits
    /// `compaction_start`/`compaction_end`, offers the extension `session_before_compact` veto hook,
    /// appends a `CompactionEntry`, and notifies `session_compact`.
    ///
    /// Returns the [`crate::state::CompactionResult`] on success. A refusal is an **error**, never a
    /// success-with-`None` — Pi's `compact` is typed `Promise<CompactionResult>` and `throw`s
    /// (agent-session.ts:1801-1808/1823-1825), so an RPC client / SDK embedder gets a distinguishable
    /// reason: [`SessionServiceError::AlreadyCompacted`], [`SessionServiceError::NothingToCompact`]
    /// or [`SessionServiceError::CompactionCancelled`].
    pub async fn compact(
        &self,
        custom_instructions: Option<String>,
    ) -> Result<crate::state::CompactionResult, SessionServiceError> {
        let reason = CompactionReason::Manual;
        // Disconnect/abort dance: stop the active run before compacting AND wait for it to settle
        // — Pi is `this._disconnectFromAgent(); await this.abort();` (agent-session.ts:1784-1785),
        // and its `abort()` ends in `await this.waitForIdle()`. SEAM-024: compaction installs its
        // own cancel token and rewrites the branch immediately below, so starting that while the
        // aborted turn was still writing tool results raced the transcript it is about to compact.
        self.abort_and_settle().await;
        let cancel = self.session_cancel.child_token();
        // The slot is installed BY the guard so the two can never be written apart — see
        // [`CompactionCancelGuard`] for why a hand-written clear at each `return` is not enough in
        // Rust, and why the ordered `clear()` calls below still stand.
        let mut cancel_slot = CompactionCancelGuard::install(&self.compaction_cancel, cancel.clone());
        self.fanout_emit(AgentSessionEvent::CompactionStart { reason }).await;

        // Pi's very first statement inside `compact()`'s `try` is the model check —
        // `if (!this.model) { throw new Error(formatNoModelSelectedMessage()); }`
        // (agent-session.ts:1790-1792) — thrown AFTER `compaction_start` was emitted (`:1787`) and
        // caught by the same handler that emits `compaction_end` with
        // `errorMessage: "Compaction failed: …"` (`:1908-1917`), which is why the exit below mirrors
        // the `NothingToCompact` arm rather than returning bare.
        let current_model = Self::lock(&self.compaction_model).clone();
        let model = match current_model {
            Some(m) => m,
            None => {
                cancel_slot.clear();
                let err = SessionServiceError::NoModelSelected;
                self.fanout_emit(AgentSessionEvent::CompactionEnd {
                    reason,
                    result: None,
                    aborted: false,
                    will_retry: false,
                    error_message: Some(format!("Compaction failed: {err}")),
                })
                .await;
                return Err(err);
            }
        };
        // Pi: `this._summarizationRetryCallbacks({ source: "compaction", reason: "manual" })`
        // (agent-session.ts:1859).
        let (retry_observer, retry_rx) = crate::compact::summarization_retry_channel(
            SummarizationRetrySource::Compaction { reason },
        );
        let retry_pump = self.spawn_event_pump(retry_rx);
        let summarizer =
            DynSummarizer::new(self.provider.current(), model.clone(), self.summarization_retry())
                .with_observer(retry_observer);
        // Pi threads the session thinking level into every compaction summarization call
        // (`agent-session.ts:1855,2129`); `summarization_reasoning` applies the `model.reasoning`
        // gate before it reaches the request.
        let compactor = Compactor::new(summarizer, NoHooks).with_thinking(self.thinking_level().await);
        let settings = self.compaction_settings.clone();

        // Compute the REAL preparation BEFORE the extension hook (Pi computes `prepareCompaction`
        // then fires `session_before_compact` against it, agent-session.ts:1663-1693; L4 gap #5).
        // `None` ⇒ nothing to compact — this is the ONLY preparation (no double-prep: the same
        // `prep` feeds `run_compaction_prepared` below).
        let (prep, branch_entries) = {
            let guard = self.manager.lock().await;
            match compactor.prepare(&guard, &settings) {
                Some(x) => x,
                None => {
                    // Distinguish WHY, exactly as Pi does (agent-session.ts:1801-1807): a branch that
                    // already ends in a `compaction` entry is "Already compacted"; anything else is
                    // "Nothing to compact (session too small)".
                    let already = matches!(
                        guard.branch_path(None).last(),
                        Some(cyrup_session::entry::Entry::Known(
                            cyrup_session::entry::KnownEntry::Compaction { .. }
                        ))
                    );
                    drop(guard);
                    cancel_slot.clear();
                    let err = if already {
                        SessionServiceError::AlreadyCompacted
                    } else {
                        SessionServiceError::NothingToCompact
                    };
                    // Pi's catch emits `compaction_end` with `errorMessage: "Compaction failed: …"`
                    // for a non-abort throw (agent-session.ts:1908-1917).
                    self.fanout_emit(AgentSessionEvent::CompactionEnd {
                        reason,
                        result: None,
                        aborted: false,
                        will_retry: false,
                        error_message: Some(format!("Compaction failed: {err}")),
                    })
                    .await;
                    return Err(err);
                }
            }
        };

        // session_before_compact ext hook: veto (cancel) OR return a compaction override, both seen
        // against the real preparation (agent-session.ts:1672-1693).
        let external_override = match self
            .emit_before_compact(
                &prep,
                &branch_entries,
                custom_instructions.as_deref(),
                reason,
                false,
                &cancel,
            )
            .await
        {
            BeforeCompactOutcome::Cancel => {
                cancel_slot.clear();
                // Pi throws "Compaction cancelled" (agent-session.ts:1824); its catch classifies that
                // exact message as an ABORT, so `compaction_end` carries `aborted:true` and NO
                // errorMessage (agent-session.ts:1909-1916).
                self.fanout_emit(AgentSessionEvent::CompactionEnd {
                    reason,
                    result: None,
                    aborted: true,
                    will_retry: false,
                    error_message: None,
                })
                .await;
                return Err(SessionServiceError::CompactionCancelled);
            }
            BeforeCompactOutcome::Proceed(ov) => ov,
        };

        let mut guard = self.manager.lock().await;
        let result = compactor
            .run_compaction_prepared(
                &mut guard,
                &model,
                &settings,
                reason,
                custom_instructions,
                false,
                &prep,
                branch_entries,
                external_override,
                cancel,
            )
            .await;
        // Estimate the rebuilt context size for the result payload (Pi `estimateMessagesTokens`).
        // SESS-043 — the RAW projection, which is both the estimate basis below AND the transcript
        // pi re-seeds from (`this.agent.state.messages = sessionContext.messages`). It used to be
        // `build_context()`, the `convertToLlm`-flattened twin.
        let compacted_raw = guard.build_context_raw();
        // Pi measures `estimatedTokensAfter` over the RAW `AgentMessage` context, NOT the
        // `convertToLlm`-flattened one: `estimateMessagesTokens(sessionContext.messages)`
        // (agent-session.ts:1876 manual / :2157 auto) sums `estimateTokens` (compaction.ts:266-300)
        // over `buildSessionContext().messages`, and that list is
        // `buildContextEntries().flatMap(sessionEntryToContextMessages)` with every role intact
        // (session-manager.ts:461-469 composed with :383-408). So a retained `compactionSummary`
        // costs `summary.length/4` with NO wrapper prose, and an `excludeFromContext` (`!!`) bash
        // execution still costs `(command.length + output.length)/4`.
        //
        // Measuring `compacted_ctx` instead billed the ~107-char COMPACTION_SUMMARY wrapper that
        // `push_as_message` adds (cyrup-session/src/context.rs:16-18) — ~27 tokens, and a compacted
        // context ALWAYS leads with one — while silently dropping every `excludeFromContext` bash
        // entry, which `AgentMessage::push_llm` removes at the LLM boundary. It also disagreed with
        // `tokens_before` on the SAME `compaction_end` event, which `prepare_compaction` already
        // computes over the raw projection (cyrup-session/src/compaction/prepare.rs).
        let estimated_tokens_after: u64 = u64::from(
            compacted_raw
                .iter()
                .map(cyrup_session::compaction::tokens::estimate_agent_message)
                .fold(0u32, u32::saturating_add),
        );
        drop(guard);
        // Close the retry queue (the compactor owns the emitter) and flush it — with the manager
        // guard already released — so every `summarization_retry_*` lands BEFORE `compaction_end`.
        drop(compactor);
        let _ = retry_pump.await;
        cancel_slot.clear();

        match result {
            Ok(Some(entry)) => {
                // SEAM-112 — the re-seed is on the SUCCESS PATH ONLY. pi orders it
                // `appendCompaction(...)` → `this.agent.state.messages = sessionContext.messages;`
                // (agent-session.ts:1952-1955 manual / :2275-2280 auto), both of them AFTER the
                // `signal.aborted` early-return, so a cancelled / declined / failed compaction
                // leaves `agent.state.messages` exactly as the run found it.
                //
                // cyrup ran it unconditionally, before `match result`. That resurrected work a
                // failed compaction had no right to restore: overflow recovery calls
                // `drop_trailing_assistant` (`:4775-4781`) to remove the overflow response from the
                // agent transcript BEFORE compacting, but the response is already persisted (it was
                // written on `message_end`), so an aborted or erroring compaction re-seeded it
                // straight back out of the session file — the exact state `Agent::continue_run`
                // rejects with `ContinueFromAssistant`.
                //
                // pi `agent-session.ts:1955` (manual `compact`) / `:2280` (`_runAutoCompaction`):
                // re-seed the AGENT's in-memory transcript from the compacted context.
                // `appendCompaction` only writes the JSONL entry — this assignment is what actually
                // shrinks the next request.
                //
                // Without it `/compact` reported success and the TUI re-rendered a compacted
                // transcript from the session, while the very next turn still shipped the ENTIRE
                // pre-compaction history to the provider: zero token reduction, full cost. Overflow
                // recovery was worse than useless — `check_compaction` set
                // `overflow_recovery_attempted`, `continue_run()` resent the unchanged context, it
                // overflowed again, and the one-shot latch reported "Try reducing context or
                // switching to a larger-context model", blaming the model for a compaction that had
                // never taken effect.
                //
                // `navigate_tree` already did exactly this (`:1857-1862`, citing
                // `agent-session.ts:2871`); the two compaction paths were the ones that built the
                // context only to COUNT it.
                //
                // SESS-043 — seeded from the RAW projection. Folding `build_context().messages`
                // through `core_message_to_agent` produced a transcript of a different LENGTH and
                // different roles from pi's: `convertToLlm` DROPS every `excludeFromContext` (`!!`)
                // bash message and rewrites each summary into wrapper prose, so pi's
                // `messages.slice(0, -1)` arithmetic (`agent-session.ts:2008`, `:2188`, `:2703`)
                // and every agent-state token estimate ran over a different list.
                let compacted_messages: Vec<AgentMessage> =
                    compacted_raw.iter().map(raw_message_to_agent).collect();
                self.agent.set_messages(compacted_messages).await;
                let cr = crate::state::CompactionResult {
                    summary: entry.summary.clone(),
                    first_kept_entry_id: entry.first_kept_entry_id.to_string(),
                    tokens_before: entry.tokens_before,
                    estimated_tokens_after: Some(estimated_tokens_after),
                    // SEAM-034: pi's `usage?` (compaction.ts:93) — the token spend of the
                    // summarization call(s), already recorded on the compaction entry.
                    usage: entry.usage.clone(),
                    details: entry.details.clone(),
                };
                // session_compact ext notify (agent-session.ts:1740-1747): the full Pi payload —
                // the produced compaction entry, whether an extension drove it, reason, retry flag.
                let notify_cancel = self.session_cancel.child_token();
                self.services
                    .ext_host
                    .dispatcher()
                    .dispatch_notify(
                        &HostEvent::SessionCompact {
                            compaction_entry: serde_json::to_value(&entry)
                                .unwrap_or(serde_json::Value::Null),
                            from_extension: entry.from_hook,
                            reason: compaction_reason_str(reason).to_string(),
                            will_retry: false,
                        },
                        &notify_cancel,
                    )
                    .await;
                self.fanout_emit(AgentSessionEvent::CompactionEnd {
                    reason,
                    result: Some(cr.clone()),
                    aborted: false,
                    will_retry: false,
                    error_message: None,
                })
                .await;
                Ok(cr)
            }
            // The internal `CompactionHooks` seam cancelled (`BeforeCompactDecision::Cancel`) — the
            // same refusal Pi reports as "Compaction cancelled" (agent-session.ts:1824/1869).
            Ok(None) => {
                self.fanout_emit(AgentSessionEvent::CompactionEnd {
                    reason,
                    result: None,
                    aborted: true,
                    will_retry: false,
                    error_message: None,
                })
                .await;
                Err(SessionServiceError::CompactionCancelled)
            }
            Err(e) => {
                let aborted = matches!(e, cyrup_session::compaction::CompactionError::Aborted);
                let error_message = if aborted {
                    None
                } else {
                    Some(format!("Compaction failed: {e}"))
                };
                self.fanout_emit(AgentSessionEvent::CompactionEnd {
                    reason,
                    result: None,
                    aborted,
                    will_retry: false,
                    error_message,
                })
                .await;
                if aborted {
                    // An in-flight abort (Esc during `/compact` → `abort_compaction`) is the SAME
                    // refusal Pi raises as the bare `Compaction cancelled`
                    // (agent-session.ts:1869 `if (this._compactionAbortController.signal.aborted)
                    // { throw new Error("Compaction cancelled"); }`), propagated verbatim to an RPC
                    // client by rpc-mode.ts:789-795. Surfacing the wrapped
                    // `SessionServiceError::Compaction` here would emit `compaction: compaction
                    // cancelled` instead, and Pi's own catch classifies an abort by comparing
                    // `message === "Compaction cancelled"` (agent-session.ts:1911), so the exact
                    // string is load-bearing.
                    Err(SessionServiceError::CompactionCancelled)
                } else {
                    Err(e.into())
                }
            }
        }
    }

    /// Fire the `session_before_compact` extension hook against a REAL preparation and reduce the
    /// guest's decision (L4 gap #5). Shared by manual [`Self::compact`] and [`Self::run_auto_compaction`].
    /// Returns [`BeforeCompactOutcome::Cancel`] on a veto, else the (optional) compaction override.
    async fn emit_before_compact(
        &self,
        prep: &CompactionPreparation,
        branch_entries: &[cyrup_session::entry::Entry],
        custom_instructions: Option<&str>,
        reason: CompactionReason,
        will_retry: bool,
        cancel: &CancelToken,
    ) -> BeforeCompactOutcome {
        if self
            .services
            .ext_host
            .dispatcher()
            .no_subscribers(cyrup_ext::EventKind::SessionBeforeCompact)
        {
            return BeforeCompactOutcome::Proceed(None);
        }
        let preparation = compaction_preparation_value(prep);
        let branch = serde_json::to_value(branch_entries).unwrap_or_else(|_| serde_json::json!([]));
        match self
            .services
            .ext_host
            .emit_session_before_compact(
                preparation,
                branch,
                custom_instructions.map(str::to_string),
                compaction_reason_str(reason),
                will_retry,
                cancel,
            )
            .await
        {
            CompactionReduction::Blocked { .. } => BeforeCompactOutcome::Cancel,
            CompactionReduction::Override(v) => {
                BeforeCompactOutcome::Proceed(Some(parse_compaction_override(&v)))
            }
            CompactionReduction::Proceed => BeforeCompactOutcome::Proceed(None),
        }
    }

    /// Cancel an in-flight manual **or auto** compaction — Pi `abortCompaction`
    /// (`agent-session.ts:1930-1933` @v0.83.0), whose whole body is two aborts:
    ///
    /// ```ts
    /// abortCompaction(): void {
    ///     this._compactionAbortController?.abort();
    ///     this._autoCompactionAbortController?.abort();
    /// }
    /// ```
    ///
    /// SESS-041: the second line was missing. `run_auto_compaction` installs its own child token in
    /// `auto_compaction_cancel` (`:4535`) and NEVER writes `compaction_cancel`, so an auto
    /// compaction — the 10-18 s one a user actually wants to escape, since they did not ask for it
    /// — was unreachable from here: `abort_compaction` cancelled a `None` and returned, and the run
    /// went to completion. `is_compacting` already reads both (`:4393`), which is what made the
    /// asymmetry invisible.
    pub fn abort_compaction(&self) {
        if let Some(c) = Self::lock(&self.compaction_cancel).as_ref() {
            c.cancel();
        }
        if let Some(c) = Self::lock(&self.auto_compaction_cancel).as_ref() {
            c.cancel();
        }
    }

    /// Cancel an in-flight branch summarization (Pi `abortBranchSummary`, agent-session.ts:1796).
    pub fn abort_branch_summary(&self) {
        if let Some(c) = Self::lock(&self.branch_summary_cancel).as_ref() {
            c.cancel();
        }
    }

    // --------------------------------------------------------------- fork / branch ----

    /// Navigate the session leaf to `entry` (no file mutation; R-04-023).
    pub async fn branch(&self, entry: EntryId) -> Result<(), SessionServiceError> {
        self.manager.lock().await.branch(&entry)?;
        Ok(())
    }

    /// Navigate to `entry`, recording a branch-summary of the abandoned branch (R-04-024/R-05-016).
    /// Returns the summary text, if one was produced.
    pub async fn branch_with_summary(
        &self,
        entry: EntryId,
        user_wants_summary: bool,
    ) -> Result<Option<String>, SessionServiceError> {
        // Pi `navigateTree`: `if (options.summarize && !this.model) { throw new Error("No model
        // available for summarization"); }` (agent-session.ts:2910-2912) — a DIFFERENT string from
        // `formatNoModelSelectedMessage`, and gated on the caller actually asking for a summary, so
        // a modelless session can still navigate the tree without one.
        let current_model = Self::lock(&self.compaction_model).clone();
        let model = match current_model {
            Some(m) => m,
            // pi's gate is on the SUMMARY, not the navigation, so a modelless session still moves
            // the leaf; only the summarizing variant is refused.
            None if !user_wants_summary => {
                // `Compactor::run_branch_summary` takes `&Model` unconditionally even though it
                // only reads it to build the summary, so the no-summary navigation is done
                // directly here (`session.branch(&target_id)`, compaction/mod.rs:384). Residual:
                // the `session_before_tree`/`session_tree` hooks that `run_branch_summary` also
                // fires are skipped on this narrow path — `run_branch_summary` should take
                // `Option<&Model>` so they are not.
                self.manager.lock().await.branch(&entry)?;
                return Ok(None);
            }
            None => return Err(SessionServiceError::NoModelForSummarization),
        };
        // Pi: `this._summarizationRetryCallbacks({ source: "branchSummary" })`
        // (agent-session.ts:2998).
        let (retry_observer, retry_rx) =
            crate::compact::summarization_retry_channel(SummarizationRetrySource::BranchSummary);
        let retry_pump = self.spawn_event_pump(retry_rx);
        let summarizer =
            DynSummarizer::new(self.provider.current(), model.clone(), self.summarization_retry())
                .with_observer(retry_observer);
        let compactor = Compactor::new(summarizer, NoHooks);
        let cancel = self.session_cancel.child_token();
        *Self::lock(&self.branch_summary_cancel) = Some(cancel.clone());

        let mut guard = self.manager.lock().await;
        let old_leaf = guard.leaf_id().cloned();
        let entry_opt = compactor
            .run_branch_summary(
                &mut guard,
                &model,
                entry,
                old_leaf,
                user_wants_summary,
                &self.branch_summary_settings,
                cancel,
            )
            .await;
        drop(guard);
        // Close + flush the retry queue with the manager guard already released (see
        // `spawn_event_pump`).
        drop(compactor);
        let _ = retry_pump.await;
        *Self::lock(&self.branch_summary_cancel) = None;
        Ok(entry_opt?.map(|e| e.summary))
    }

    /// The unified `/tree` navigation op (Pi `navigateTree(targetId, options)`,
    /// agent-session.ts:2704-2895). Navigates the leaf to `target`, optionally summarizing the
    /// abandoned branch, and returns `{editor_text, cancelled, aborted, summary_entry}`:
    ///
    /// - No-op (`{cancelled:false}`) when `target` is already the leaf (agent-session.ts:2712).
    /// - The `session_before_tree` extension hook may veto the navigation (`{cancelled:true}`,
    ///   agent-session.ts:2757).
    /// - When summarizing, an aborted summarization returns `{cancelled:true, aborted:true}`
    ///   (agent-session.ts:2796).
    /// - A `user`/`custom_message` target re-roots the leaf at the target's PARENT and returns the
    ///   target's text as `editor_text` (so a UI can re-edit it); any other target navigates to the
    ///   target itself (agent-session.ts:2823-2841).
    /// - The summary is attached at the navigation target position via `branch_with_summary`
    ///   (agent-session.ts:2847); the `label` lands on the summary entry, or — with no summary — on
    ///   the target (agent-session.ts:2858/2867). Finally the agent transcript is rebuilt from the
    ///   navigated context and `session_tree` is emitted (agent-session.ts:2871-2884).
    pub async fn navigate_tree(
        &self,
        target: EntryId,
        options: NavigateTreeOptions,
    ) -> Result<NavigateTreeOutcome, SessionServiceError> {
        use cyrup_session::compaction::{
            branch_token_budget, collect_entries_for_branch_summary, prepare_branch_entries,
        };
        use cyrup_session::entry::{Entry, KnownEntry};

        // Phase 1 (guard held): read the session to compute the navigation target + the branch
        // collection, then build the real `TreePreparation` for the extension hook. The guard is
        // RELEASED before the hook so a guest may read the session during `session_before_tree`
        // without a re-entrant manager-lock deadlock (agent-session.ts:2704-2751; L4 gap #5).
        let (old_leaf, new_leaf, editor_text, collection, common_ancestor_id) = {
            let guard = self.manager.lock().await;
            let old_leaf = guard.leaf_id().cloned();

            // No-op if already at target, BEFORE the hook (agent-session.ts:2712).
            if old_leaf.as_ref() == Some(&target) {
                return Ok(NavigateTreeOutcome::default());
            }

            // "Model required for summarization" (agent-session.ts:2910-2912) — after the
            // already-at-target no-op and BEFORE the target lookup, exactly as pi orders it.
            if options.summarize && Self::lock(&self.model).is_none() {
                return Err(SessionServiceError::NoModelForSummarization);
            }

            // Target must exist (agent-session.ts:2721).
            let target_entry = guard
                .entry(&target)
                .cloned()
                .ok_or_else(|| SessionServiceError::InvalidForkEntry(target.to_string()))?;

            // Determine the new leaf position + re-editable text by target type
            // (agent-session.ts:2823-2841).
            let (new_leaf, editor_text): (Option<EntryId>, Option<String>) = match &target_entry {
                Entry::Known(KnownEntry::Message { .. })
                    if user_message_text(&target_entry).is_some() =>
                {
                    (target_entry.parent_id(), user_message_text(&target_entry))
                }
                Entry::Known(KnownEntry::CustomMessage { content, .. }) => {
                    (target_entry.parent_id(), Some(custom_message_text(content)))
                }
                _ => (Some(target.clone()), None),
            };

            let old_path: Vec<Entry> =
                guard.branch_path(old_leaf.as_ref()).into_iter().cloned().collect();
            let target_path: Vec<Entry> =
                guard.branch_path(Some(&target)).into_iter().cloned().collect();
            let collection = collect_entries_for_branch_summary(&old_path, &target_path);
            let common_ancestor_id = collection.common_ancestor_id.clone();
            (old_leaf, new_leaf, editor_text, collection, common_ancestor_id)
        };

        // Phase 2 (no guard): session_before_tree ext hook — veto OR a summary/customInstructions/
        // label override, against the real `TreePreparation` (agent-session.ts:2752-2783).
        let mut eff_custom_instructions = options.custom_instructions.clone();
        let mut eff_replace_instructions = options.replace_instructions;
        let mut eff_label = options.label.clone();
        let mut override_summary: Option<(String, serde_json::Value)> = None;
        if !self
            .services
            .ext_host
            .dispatcher()
            .no_subscribers(cyrup_ext::EventKind::SessionBeforeTree)
        {
            let preparation = serde_json::json!({
                "targetId": target,
                "oldLeafId": old_leaf,
                "commonAncestorId": common_ancestor_id,
                "entriesToSummarize": collection.entries,
                "userWantsSummary": options.summarize,
                "customInstructions": options.custom_instructions,
                "replaceInstructions": options.replace_instructions,
                "label": options.label,
            });
            let cancel = self.session_cancel.child_token();
            match self.services.ext_host.emit_session_before_tree(preparation, &cancel).await {
                TreeReduction::Blocked { .. } => {
                    return Ok(NavigateTreeOutcome { cancelled: true, ..Default::default() });
                }
                TreeReduction::Override(v) => {
                    if let Some(ci) = v.get("customInstructions").and_then(|s| s.as_str()) {
                        eff_custom_instructions = Some(ci.to_string());
                    }
                    if let Some(ri) = v.get("replaceInstructions").and_then(serde_json::Value::as_bool)
                    {
                        eff_replace_instructions = ri;
                    }
                    if let Some(lbl) = v.get("label").and_then(|s| s.as_str()) {
                        eff_label = Some(lbl.to_string());
                    }
                    // A summary override (Pi `SessionBeforeTreeResult.summary = {summary, details?}`)
                    // is used directly as the branch summary (fromExtension), skipping the model.
                    if let Some(s) = v.get("summary") {
                        let text = s
                            .get("summary")
                            .and_then(|t| t.as_str())
                            .or_else(|| s.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let details =
                            s.get("details").cloned().unwrap_or_else(|| serde_json::json!({}));
                        override_summary = Some((text, details));
                    }
                }
                TreeReduction::Proceed => {}
            }
        }

        // Phase 3 (STILL no guard): summarize the abandoned branch (unless the extension supplied
        // the summary), threading the (possibly overridden) instructions.
        //
        // The manager mutex is deliberately NOT held across this leg. Branch summarization is a full
        // provider round-trip plus its retry backoff — holding `self.manager` across it would stall
        // every other session-manager consumer (the tree/DAG readers a TUI polls, an extension's
        // `getEntries`, a concurrent `compact`) for the whole call. `AgentSession::compact` already
        // scopes its guard for exactly this reason; this path used to take the guard first and hold
        // it through the `.await`, which was invisible only because no front end could reach the
        // summarize branch (SESS-023). Pi has no equivalent hazard: its session manager is not
        // mutex-guarded.
        let mut from_extension_summary = false;
        // (text, details, usage) — `usage` is the summarization call's token spend, persisted on the
        // appended `branch_summary` entry (Pi `BranchSummaryEntry.usage`). An extension-supplied
        // summary reports none.
        let mut summary_payload: Option<(String, serde_json::Value, Option<Usage>)> = None;
        if let Some((text, details)) = override_summary {
            // The extension supplied the branch summary directly (agent-session.ts:2762-2775).
            if options.summarize {
                summary_payload = Some((text, details, None));
                from_extension_summary = true;
            }
        } else if options.summarize && !collection.entries.is_empty() {
            // Summarize the abandoned branch (agent-session.ts:2787). Pi still appends the non-empty
            // "No content to summarize" placeholder, so we gate only on the collected entry count.
            // Non-`None` here: the `options.summarize && !this.model` gate above already returned
            // `NoModelForSummarization` (agent-session.ts:2910-2912), and this arm is inside
            // `options.summarize`.
            let model = Self::lock(&self.compaction_model)
                .clone()
                .ok_or(SessionServiceError::NoModelForSummarization)?;
            // `(contextWindow || 128000) − reserve` (Pi `branch-summarization.ts:315-317`). The
            // fallback matters: without it a model reporting a zero context window would get budget
            // `0`, which `prepare_branch_entries` reads as "no limit".
            let budget =
                branch_token_budget(&model, self.branch_summary_settings.reserve_tokens);
            let prep = prepare_branch_entries(&collection.entries, budget);
            let cancel = self.session_cancel.child_token();
            *Self::lock(&self.branch_summary_cancel) = Some(cancel.clone());
            let result = self
                .generate_branch_summary_with_instructions(
                    &prep,
                    &model,
                    eff_custom_instructions.as_deref(),
                    eff_replace_instructions,
                    cancel,
                )
                .await;
            *Self::lock(&self.branch_summary_cancel) = None;
            match result {
                Ok(produced) => {
                    let details = serde_json::to_value(prep.file_ops.to_details())
                        .unwrap_or_else(|_| serde_json::json!({}));
                    summary_payload = Some((produced.text, details, produced.usage));
                }
                Err(cyrup_session::compaction::CompactionError::Aborted) => {
                    return Ok(NavigateTreeOutcome {
                        cancelled: true,
                        aborted: true,
                        ..Default::default()
                    });
                }
                Err(e) => return Err(e.into()),
            }
        }

        // Phase 4 (guard re-held): apply the navigation + summary/label (agent-session.ts:2845-2868).
        let mut guard = self.manager.lock().await;
        let summary_entry = match &summary_payload {
            Some((text, details, usage)) => {
                let id = guard.branch_with_summary(
                    new_leaf.as_ref(),
                    text.clone(),
                    Some(details.clone()),
                    usage.clone(),
                    from_extension_summary,
                )?;
                let entry = branch_summary_entry_of(&guard, &id);
                if let Some(label) = eff_label.as_deref() {
                    guard.append_label(&id, Some(label))?;
                }
                entry
            }
            None => {
                match new_leaf.as_ref() {
                    None => guard.reset_leaf(),
                    Some(id) => guard.branch(id)?,
                }
                // No summary entry to label → label the navigation target itself.
                if let Some(label) = eff_label.as_deref() {
                    guard.append_label(&target, Some(label))?;
                }
                None
            }
        };

        // Rebuild the agent transcript from the navigated context (agent-session.ts:2871).
        // SESS-043 — the RAW projection, matching pi's `this.agent.state.messages =
        // sessionContext.messages` (`agent-session.ts:3067-3068` @v0.83.0); see the two compaction
        // re-seeds for why the flattened twin gave the wrong list.
        let raw = guard.build_context_raw();
        let new_leaf_id = guard.leaf_id().cloned();
        drop(guard);
        let msgs: Vec<AgentMessage> = raw.iter().map(raw_message_to_agent).collect();
        self.agent.set_messages(msgs).await;

        // session_tree notify (agent-session.ts:2877). cyrup collapses the Pi payload into one
        // `tree` JSON value (the SDK forwards it to the guest as `tree_json`).
        if !self
            .services
            .ext_host
            .dispatcher()
            .no_subscribers(cyrup_ext::EventKind::SessionTree)
        {
            let tree = serde_json::json!({
                "newLeafId": new_leaf_id,
                "oldLeafId": old_leaf,
                "summaryEntry": summary_entry,
                "fromExtension": summary_payload.as_ref().map(|_| from_extension_summary),
            });
            let notify_cancel = self.session_cancel.child_token();
            self.services
                .ext_host
                .dispatcher()
                .dispatch_notify(&HostEvent::SessionTree { tree }, &notify_cancel)
                .await;
        }

        Ok(NavigateTreeOutcome { editor_text, cancelled: false, aborted: false, summary_entry })
    }

    /// Generate a branch summary with optional custom/replace instructions (Pi `generateBranchSummary`
    /// with `customInstructions`/`replaceInstructions`, branch-summarization.ts:318-336). cyrup-session's
    /// `generate_branch_summary` takes no instruction knobs, so the `/tree` op threads them here over
    /// the same public branch-summary primitives.
    async fn generate_branch_summary_with_instructions(
        &self,
        prep: &cyrup_session::compaction::BranchPreparation,
        model: &Model,
        custom_instructions: Option<&str>,
        replace_instructions: bool,
        cancel: CancelToken,
    ) -> Result<BranchSummaryOutput, cyrup_session::compaction::CompactionError> {
        use cyrup_session::compaction::{
            format_file_operations, serialize_conversation, SummarizationRequest, Summarizer,
            BRANCH_SUMMARY_EMPTY_PLACEHOLDER, BRANCH_SUMMARY_PREAMBLE, BRANCH_SUMMARY_PROMPT,
            SUMMARIZATION_SYSTEM_PROMPT,
        };
        // Pi short-circuits BEFORE the model call when there is nothing to summarize
        // (branch-summarization.ts:309-311).
        if prep.messages.is_empty() {
            return Ok(BranchSummaryOutput {
                text: BRANCH_SUMMARY_EMPTY_PLACEHOLDER.to_string(),
                usage: None,
            });
        }
        let transcript = serialize_conversation(&prep.messages);
        // Instruction selection (branch-summarization.ts:319-326): `replace` swaps the default
        // prompt; a bare custom instruction is appended as "Additional focus".
        let instructions = match custom_instructions {
            Some(ci) if !ci.is_empty() && replace_instructions => ci.to_string(),
            Some(ci) if !ci.is_empty() => {
                format!("{BRANCH_SUMMARY_PROMPT}\n\nAdditional focus: {ci}")
            }
            _ => BRANCH_SUMMARY_PROMPT.to_string(),
        };
        let prompt = format!("<conversation>\n{transcript}\n</conversation>\n\n{instructions}");
        // Pi: `this._summarizationRetryCallbacks({ source: "branchSummary" })`
        // (agent-session.ts:2998).
        let (retry_observer, retry_rx) =
            crate::compact::summarization_retry_channel(SummarizationRetrySource::BranchSummary);
        let retry_pump = self.spawn_event_pump(retry_rx);
        let summarizer =
            DynSummarizer::new(self.provider.current(), model.clone(), self.summarization_retry())
                .with_observer(retry_observer);
        let req = SummarizationRequest {
            system_prompt: SUMMARIZATION_SYSTEM_PROMPT,
            prompt_text: prompt,
            max_tokens: 2048,
            model: ModelRef {
                provider: model.provider.clone(),
                api: Some(model.api.clone()),
                model: model.id.clone(),
            },
            // Pi builds the branch-summary options inline (`{ apiKey, headers, env, signal,
            // maxTokens: 2048 }`, branch-summarization.ts:348) rather than through
            // `createSummarizationOptions`, so `reasoning` is never set for a branch summary.
            thinking: ModelThinkingLevel::Off,
        };
        let resp = summarizer.complete(req, cancel).await;
        // Close + flush the retry queue BEFORE the `?` early-returns on a failed summarization, so
        // an exhausted retry still reports its `summarization_retry_finished`.
        drop(summarizer);
        let _ = retry_pump.await;
        let resp = resp?;
        match resp.stop_reason {
            cyrup_core::StopReason::Error => Err(
                cyrup_session::compaction::CompactionError::Summarization(
                    resp.error_message.unwrap_or_default(),
                ),
            ),
            cyrup_core::StopReason::Aborted => {
                Err(cyrup_session::compaction::CompactionError::Aborted)
            }
            // An unsettled response is NOT a summary — same guard, and the same `Deferred`
            // rationale, as `cyrup_session::compaction::{summarize,branch}`.
            cyrup_core::StopReason::Pending | cyrup_core::StopReason::Deferred => {
                Err(cyrup_session::compaction::CompactionError::Summarization(
                    resp.error_message.unwrap_or_else(|| {
                        cyrup_session::compaction::PENDING_SUMMARY.to_string()
                    }),
                ))
            }
            cyrup_core::StopReason::Stop
            | cyrup_core::StopReason::Length
            | cyrup_core::StopReason::ToolUse => {
                let body = resp
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let (read, modified) = prep.file_ops.compute_lists();
                Ok(BranchSummaryOutput {
                    text: format!(
                        "{BRANCH_SUMMARY_PREAMBLE}{body}{}",
                        format_file_operations(&read, &modified)
                    ),
                    // The branch-summary call's token spend is persisted on the entry (Pi
                    // `BranchSummaryResult.usage`, `branch-summarization.ts:372`).
                    usage: Some(resp.usage),
                })
            }
        }
    }

    /// Republish `CYRUP_SESSION_ID` / `CYRUP_SESSION_FILE` from the LIVE manager for the next `bash`
    /// child.
    ///
    /// Pi never needs this: `resolveSpawnContext` calls `ctx.sessionManager.getSessionId()` /
    /// `getSessionFile()` at spawn time (bash.ts:172-174), so a `createBranchedSession` that mutates
    /// the manager in place is picked up automatically. cyrup's `Tool::execute` has no session
    /// context, so the branching paths — `fork`, `clone_at`, `fork_at_entry` — push instead.
    /// Without this a `bash` child run after `/fork` would report the PRE-fork session id and a
    /// session file that is no longer the one being appended to.
    fn republish_session_identity(&self, guard: &SessionManager) {
        let mut info = self.bash_session_env.get();
        info.session_id = Some(guard.session_id().to_string());
        info.session_file = guard.session_file().map(Path::to_path_buf);
        self.bash_session_env.set(info);
    }

    /// Fork the current persisted session into a new file under the same cwd (R-04-020/021).
    pub async fn fork(&self) -> Result<SessionId, SessionServiceError> {
        // A fork clones the active path through the current leaf into a new file.
        let mut guard = self.manager.lock().await;
        let layout = branch_layout(&guard);
        // Pi forks at an explicit leaf and mutates the manager in place
        // (`createBranchedSession(leafId)`, session-manager.ts:1292-1392). Fork-at-current-position
        // passes the current leaf; an empty session has nothing to fork.
        let leaf = guard.leaf_id().cloned().ok_or_else(|| {
            cyrup_session::SessionError::EmptyFork(
                guard.session_file().map(Path::to_path_buf).unwrap_or_default(),
            )
        })?;
        guard.create_branched_session(&leaf, &layout)?;
        self.republish_session_identity(&guard);
        let id = guard.session_id().clone();
        Ok(id)
    }

    /// Clone the session at an explicit entry (or the current leaf when `None`) into a new file,
    /// WITHOUT switching the active session to it (arch-11 `clone_at`; distinct from `fork`, which
    /// switches). Returns the new branched session id. Unlike `fork_at_entry`'s `before` anchoring,
    /// `clone_at` anchors the branch leaf at the selected entry itself (the full path up to and
    /// including it is cloned).
    pub async fn clone_at(&self, entry: Option<EntryId>) -> Result<SessionId, SessionServiceError> {
        let mut guard = self.manager.lock().await;
        let leaf = match entry {
            Some(e) => {
                guard
                    .entry(&e)
                    .ok_or_else(|| SessionServiceError::InvalidForkEntry(e.to_string()))?;
                e
            }
            None => guard.leaf_id().cloned().ok_or_else(|| {
                cyrup_session::SessionError::EmptyFork(
                    guard.session_file().map(Path::to_path_buf).unwrap_or_default(),
                )
            })?,
        };
        let layout = branch_layout(&guard);
        guard.create_branched_session(&leaf, &layout)?;
        self.republish_session_identity(&guard);
        Ok(guard.session_id().clone())
    }

    /// Entry-anchored fork (Pi `fork(entryId, {position})`, agent-session-runtime.ts:259-344). For
    /// `position:"before"` the anchor must be a *user* message; the new branch leaf is that message's
    /// parent and its text is returned as `selected_text` (so a UI can re-edit it). For
    /// `position:"at"` the new branch leaf is the selected entry itself. A persisted session forks
    /// into a new file via `createBranchedSession(leafId)`; an anchor with no parent (forking before
    /// the very first message) yields a fresh empty session.
    pub async fn fork_at_entry(
        &self,
        entry: &EntryId,
        position: ForkPosition,
    ) -> Result<ForkOutcome, SessionServiceError> {
        let mut guard = self.manager.lock().await;
        let (target_leaf, selected_text) = fork_anchor(&guard, entry, position)?;

        match target_leaf {
            Some(leaf) => {
                let layout = branch_layout(&guard);
                guard.create_branched_session(&leaf, &layout)?;
                self.republish_session_identity(&guard);
                let id = guard.session_id().clone();
                Ok(ForkOutcome { session_id: Some(id), selected_text })
            }
            // Forking before the first user message: nothing to branch from.
            None => Ok(ForkOutcome { session_id: None, selected_text }),
        }
    }

    /// Enumerate the user-message fork anchors on the current branch (Pi `getUserMessagesForForking`,
    /// agent-session.ts:2901) — each `{entry_id, text}` is a candidate the `/fork`/`/tree` UI offers.
    pub async fn user_messages_for_forking(&self) -> Vec<ForkAnchor> {
        let guard = self.manager.lock().await;
        let leaf = guard.leaf_id().cloned();
        guard
            .branch_path(leaf.as_ref())
            .into_iter()
            .filter_map(|e| user_message_text(e).map(|text| ForkAnchor { entry_id: e.id(), text }))
            .collect()
    }

    // --------------------------------------------------------------- naming / export ----

    /// The session's display name, if set (Pi `sessionName` getter, agent-session.ts:865).
    pub async fn session_name(&self) -> Option<String> {
        self.manager.lock().await.session_name()
    }

    /// Set the session's display name, persisting a `session_info` entry (Pi `setSessionName`,
    /// agent-session.ts:2690).
    pub async fn set_session_name(&self, name: &str) -> Result<(), SessionServiceError> {
        let resolved = {
            let mut guard = self.manager.lock().await;
            guard.append_session_info(name)?;
            guard.session_name()
        };
        // Emit `session_info_changed { name }` to every live subscription (Pi `_emit(event)`,
        // agent-session.ts:2714-2715); the `name` is re-read from the manager so it byte-matches Pi's
        // `getSessionName()` (an empty/whitespace name resolves to `None`).
        self.fanout_emit(AgentSessionEvent::SessionInfoChanged { name: resolved.clone() }).await;
        // EXT-011 — the SAME rename is an EXTENSION event upstream (`SessionInfoChangedEvent`,
        // `extensions/types.ts:571-575` @v0.83.0). The kind, the WIT export and the SDK hook all
        // existed; nothing EMITTED it, so a guest could subscribe to `session_info_changed` and
        // never be called. Notify-only, so it cannot block the rename.
        let notify_cancel = self.session_cancel.child_token();
        self.services
            .ext_host
            .dispatcher()
            .dispatch_notify(&HostEvent::SessionInfoChanged { name: resolved }, &notify_cancel)
            .await;
        Ok(())
    }

    /// Export the current session tree as JSONL (Pi `exportToJsonl`, agent-session.ts:3052). With a
    /// `path` the bytes are written there; otherwise the JSONL text is returned.
    pub async fn export_to_jsonl(
        &self,
        path: Option<&Path>,
    ) -> Result<Option<String>, SessionServiceError> {
        let guard = self.manager.lock().await;
        let mut buf: Vec<u8> = Vec::new();
        guard.export_jsonl(&mut buf)?;
        drop(guard);
        let text = String::from_utf8_lossy(&buf).into_owned();
        match path {
            Some(p) => {
                std::fs::write(p, text).map_err(|e| SessionServiceError::Io(e.to_string()))?;
                Ok(None)
            }
            None => Ok(Some(text)),
        }
    }

    /// Export the current session branch to a standalone HTML document (Pi `exportToHtml`,
    /// agent-session.ts:3022). With `path` the document is written there; otherwise the Pi default
    /// `cyrup-session-<basename>.html` (in the session cwd, basename = the session-file stem, else the
    /// session id) is used. Returns the resolved output path. The rich per-tool HTML cards
    /// (`export-html/tool-renderer.ts`) remain the one L5 residual; the document is a real transcript.
    pub async fn export_to_html(
        &self,
        path: Option<&Path>,
    ) -> Result<std::path::PathBuf, SessionServiceError> {
        let jsonl = {
            let guard = self.manager.lock().await;
            let mut buf: Vec<u8> = Vec::new();
            guard.export_jsonl(&mut buf)?;
            String::from_utf8_lossy(&buf).into_owned()
        };
        let html = crate::export::session_jsonl_to_html(&jsonl);
        let out = match path {
            Some(p) => p.to_path_buf(),
            None => {
                let basename = self
                    .session_file()
                    .await
                    .and_then(|f| f.file_stem().map(|s| s.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| self.session_id().as_str().to_string());
                self.services.cwd.join(format!("cyrup-session-{basename}.html"))
            }
        };
        std::fs::write(&out, html).map_err(|e| SessionServiceError::Io(e.to_string()))?;
        Ok(out)
    }

    /// The invocable slash commands a front-end can offer (Pi `get_commands`, rpc-mode.ts:653-683):
    /// registered extension commands (`source:"extension"`), prompt templates (`source:"prompt"`),
    /// and skills (`skill:<name>`, `source:"skill"`), each with a `name`/`description`/`source`/
    /// `sourceInfo` (rpc-types.ts `RpcSlashCommand`). `sourceInfo` is the full Pi `SourceInfo`
    /// (`{path, source, scope, origin, baseDir?}`, source-info.ts:6-12), wired from the
    /// `scope`/`origin` provenance the prompt/skill structs already carry
    /// ([`cyrup_resources::ResourceOrigin::source_info_json`]).
    pub fn slash_command_catalog(&self) -> Vec<serde_json::Value> {
        let mut out: Vec<serde_json::Value> = Vec::new();
        // Registered extension commands.
        //
        // SEAM-048 — sourced from `resolved_commands()`, NOT `command_descriptions()`. pi builds
        // each `RpcSlashCommand` from `command.invocationName` (`rpc-mode.ts:680-687`), which comes
        // from `getRegisteredCommands()` → `resolveRegisteredCommands()`
        // (`core/extensions/runner.ts:598-641`): duplicates get `${name}:${occurrence}` and the
        // list is in EXTENSION LOAD ORDER (`for (const ext of this.extensions) for (const command of
        // ext.commands.values())`, `:602-607`). `command_descriptions()` enumerated the last-wins
        // `HashMap`, so a duplicate command name made the second extension's command invisible AND
        // the list order shuffled between runs.
        //
        // SEAM-055 — `sourceInfo.path` is the OWNING EXTENSION's id, not `""`. pi's `SourceInfo.path`
        // is a non-optional `string` (`core/source-info.ts:6-12`) that `createSyntheticSourceInfo`
        // (`:24-40`) takes as its first positional argument precisely so a synthetic entry still
        // names something; an empty path collapses every extension command into one bucket for a
        // client grouping or filtering by source. `ResolvedCommand` carries the owner, which is why
        // the two fixes are one change.
        //
        // SEAM-084 — the remaining three fields of that `sourceInfo`, which SEAM-055 did not reach.
        // pi derives the whole object ONCE per extension in `createExtension`
        // (`core/extensions/loader.ts:433-444` @v0.83.0) and `registerCommand` copies it onto every
        // `RegisteredCommand`; `rpc-mode.ts:681-686` then passes `command.sourceInfo` straight
        // through. cyrup hard-coded a literal instead, so:
        //
        // * `source` was `"extension"` — a value that exists NOWHERE upstream. pi emits `"local"` for
        //   a filesystem-loaded extension and the `<prefix:…>` segment for a synthetic one, so a
        //   client grouping by `sourceInfo.source` could not tell the two apart. (The sibling
        //   TOP-LEVEL `"source": "extension"` below IS correct — that is `SlashCommandSource`,
        //   `core/slash-commands.ts:4`, a different field, and `rpc-mode.ts:684` really does emit it.)
        // * `baseDir` was absent entirely, so a client resolving a command's assets relative to its
        //   extension directory had nothing to resolve against.
        //
        // Both now come from the provenance the loader recorded ([`cyrup_ext::ExtensionProvenance`]).
        // `scope`/`origin` stay `"temporary"`/`"top-level"`: those are `createSyntheticSourceInfo`'s
        // defaults (`core/source-info.ts:36-37`), which `createExtension` never overrides.
        if let Ok(cmds) = self.services.ext_host.registry().resolved_commands() {
            for cmd in cmds {
                let prov = self
                    .services
                    .ext_host
                    .registry()
                    .extension_provenance(&cmd.owner)
                    .ok()
                    .flatten()
                    // No loader recorded one: the extension came in through neither the discovery
                    // nor the native path (a test harness registering straight into the registry is
                    // the only in-tree case). "Loaded by the host with no path" is upstream's
                    // `<inline>` case, so answer as that rather than inventing a fourth value.
                    .unwrap_or_else(cyrup_ext::ExtensionProvenance::inline);
                let mut source_info = serde_json::Map::new();
                source_info.insert("path".into(), serde_json::Value::from(cmd.owner.as_str()));
                source_info.insert("source".into(), serde_json::Value::from(prov.source));
                source_info.insert("scope".into(), serde_json::Value::from("temporary"));
                source_info.insert("origin".into(), serde_json::Value::from("top-level"));
                // `baseDir?: string` (`core/source-info.ts:11`) — the key is ABSENT for a synthetic
                // extension, exactly as `JSON.stringify` drops `baseDir: undefined`.
                if let Some(dir) = prov.base_dir {
                    source_info.insert("baseDir".into(), serde_json::Value::from(dir));
                }
                let mut entry = serde_json::Map::new();
                entry.insert("name".into(), serde_json::Value::from(cmd.invocation_name));
                // `RegisteredCommand.description?: string` (`core/extensions/types.ts:1163-1168`) —
                // an undescribed command OMITS the key rather than sending `""`. cyrup's
                // `CommandDescriptor.description` is a non-optional `String` whose empty value is
                // this port's representation of that absent field, so empty is omitted here.
                if !cmd.descriptor.description.is_empty() {
                    entry.insert("description".into(), serde_json::Value::from(cmd.descriptor.description));
                }
                entry.insert("source".into(), serde_json::Value::from("extension"));
                entry.insert("sourceInfo".into(), serde_json::Value::Object(source_info));
                out.push(serde_json::Value::Object(entry));
            }
        }
        // Prompt templates.
        for t in self.services.resources.prompts.winners() {
            let mut row = serde_json::json!({
                "name": t.name,
                "description": t.description,
                "source": "prompt",
                "sourceInfo": t.origin.source_info_json(&t.path),
            });
            // CMDHINT_01 — pi's INTERACTIVE registry carries `argumentHint` from the template itself
            // (`interactive-mode.ts:685-689`, spread-if-truthy; the sibling builtin carrier is `:640-644`).
            // pi's `get_commands` RPC omits it entirely (zero `argumentHint` in `rpc-mode.ts`) because
            // interactive mode never reads that RPC — but cyrup's TUI builds its registry from THIS catalog
            // (`app/run_arms.rs:31-38`), so without the key the hint is unreachable in the one mode pi shows
            // it in. Additive: the key is ABSENT when `None`, exactly as `JSON.stringify` drops the spread.
            // `PromptTemplate::argument_hint` is already empty-filtered at parse (`prompt.rs:112-113`).
            if let Some(hint) = t.argument_hint.as_deref()
                && let Some(obj) = row.as_object_mut()
            {
                obj.insert("argumentHint".into(), serde_json::Value::from(hint));
            }
            out.push(row);
        }
        // Skills (`/skill:<name>`).
        for s in self.services.resources.skills.winners() {
            out.push(serde_json::json!({
                "name": format!("skill:{}", s.name),
                "description": s.front.description.clone().unwrap_or_default(),
                "source": "skill",
                "sourceInfo": s.origin.source_info_json(&s.skill_md),
            }));
        }
        out
    }

    /// EVERY persisted entry except the session header, serialized (pi `get_entries`,
    /// rpc-mode.ts:609 → `SessionManager.getEntries()`, `core/session-manager.ts:1301`: "Get all
    /// session entries (excludes header)"). NOT the current branch — that is `getBranch()`
    /// (`:1260`), which the extension seam exposes separately as
    /// [`cyrup_ext::HostServices::branch`]. The body has always called `entries()`; only this line
    /// said "on the current branch".
    pub async fn entries_json(&self) -> Vec<serde_json::Value> {
        let guard = self.manager.lock().await;
        guard
            .entries()
            .iter()
            .filter_map(|e| serde_json::to_value(e).ok())
            .collect()
    }

    /// The session tree as `{entry, children, label?, labelTimestamp?}` nodes (Pi `get_tree`,
    /// rpc-mode.ts:622 → `SessionTreeNode`, `core/session-manager.ts:159-166`).
    ///
    /// SEAM-060 — `labelTimestamp` used to be dropped from every node, with an in-tree comment
    /// declaring the omission deliberate. It is not vestigial upstream: `labelTimestampsById` is
    /// maintained at `session-manager.ts:865`, `:970` and `:1247-1250` and read into the node at
    /// `:1318`, and the wire contract names the type directly (`modes/rpc/rpc-types.ts:202-208`).
    /// Without it a client cannot sort or age branch labels, cannot render "renamed 2 days ago",
    /// and cannot spot a label that predates the entries beneath it. Emitted with the same
    /// omit-when-`None` insert as `label`, so an unlabelled node still carries neither key.
    ///
    /// The per-node serializer is [`crate::host_services::tree_node_to_json`], shared with
    /// [`cyrup_ext::HostServices::tree`]: both must emit pi's one `SessionTreeNode` shape, and a
    /// second private copy here is precisely how the SEAM-060 omission survived on one side.
    pub async fn tree_json(&self) -> Vec<serde_json::Value> {
        let guard = self.manager.lock().await;
        guard.tree().iter().map(crate::host_services::tree_node_to_json).collect()
    }

    /// The **flattened session DAG** for the `/tree` selector (feature #2): the manager's real branch
    /// tree (`SessionManager::tree`) walked in pre-order into [`SessionDagNode`]s carrying parent/depth/
    /// label/kind/fold/leaf/label/timestamp — the flat-DAG getter the connector/fold/filter engine in
    /// `cyrup-tui::tree_selector` was data-starved for (audit: `/tree` showed a flat user-message
    /// list). Mirrors Pi `flattenTree` over `SessionManager.getTree()` (`tree-selector.ts:199-320`).
    pub async fn session_dag(&self) -> Vec<SessionDagNode> {
        let guard = self.manager.lock().await;
        let leaf = guard.leaf_id().cloned();
        let mut out = Vec::new();
        for root in guard.tree() {
            flatten_dag_node(&root, None, 0, leaf.as_ref(), &mut out);
        }
        out
    }

    // ------------------------------------------------------------------- lifecycle ----

    /// Dispose the session (Pi `AgentSession.dispose` via runtime `dispose`,
    /// agent-session-runtime.ts:390): abort any in-flight run **and wait for it to settle**, emit
    /// `session_shutdown`, and cancel the long-lived session token so the extension subscriber
    /// unwinds.
    ///
    /// SEAM-024 — the settle is not optional. Pi's `teardownCurrent` opens with
    /// `await this.session.abort()` and the comment "Settle any active response first so the
    /// aborted turn (including tool results) is persisted to the outgoing session before it is
    /// replaced" (agent-session-runtime.ts:167-169), and only then emits `session_shutdown` and
    /// disposes. cyrup collapses pi's `teardownCurrent` + `runtime.dispose` + `session.dispose`
    /// into this one method, so the await belongs here: it is on EVERY teardown path (`run.rs`,
    /// `main.rs`) and every replacement (`runtime.rs`). Previously the fire-and-forget `abort()`
    /// let `session_shutdown` be announced — and `session_cancel` fired — while the aborted turn
    /// was still writing its tool results.
    pub async fn dispose(&self, reason: &str) {
        self.dispose_with(reason, None, None).await;
    }

    /// [`Self::dispose`] with the host's `before_session_invalidate` hook (Pi
    /// `beforeSessionInvalidate`, agent-session-runtime.ts:76/:129-131).
    ///
    /// Pi's `teardownCurrent` is `await this.session.abort(); await emitSessionShutdownEvent(…);
    /// this.beforeSessionInvalidate?.(); this.session.dispose();` (agent-session-runtime.ts:167-177)
    /// and its `dispose()` repeats the last three lines (:398-404). So the hook's contract is
    /// positional: it runs **after** every `session_shutdown` handler has finished and **before**
    /// anything invalidates the session. Because cyrup collapses pi's `teardownCurrent` +
    /// `AgentSession.dispose` into this one method, the call site is here — after
    /// `dispatch_notify(SessionShutdown)` and before `session_cancel.cancel()`, which is cyrup's
    /// analog of pi's `_extensionRunner.invalidate("This extension ctx is stale after session
    /// replacement or reload…")` (agent-session.ts:848, inside `dispose()` at :837-853).
    ///
    /// The hook is deliberately **synchronous** (`Fn()`, pi's `() => void`): it exists so a host can
    /// detach extension-provided UI without yielding to the executor, i.e. without a window in which
    /// a component still rendered from the outgoing session's now-stale extension context can be
    /// driven. Awaiting inside it would reopen exactly that window, so there is nothing to await.
    ///
    /// [`Self::dispose`] passes `None`; the only producer of a `Some` is
    /// [`crate::runtime::AgentSessionRuntime`], which reads whatever the host registered via
    /// `set_before_session_invalidate`.
    /// `target_session_file` is pi's `teardownCurrent(reason, targetSessionFile?)` second argument
    /// (`agent-session-runtime.ts:165`), spread onto the event at `:171-174`: *"Destination session
    /// file when shutting down due to session replacement"*
    /// (`core/extensions/types.ts:616-621`). Every replacement path supplies it —
    /// `teardownCurrent("new", sessionManager.getSessionFile())` and its `resume`/`fork` siblings
    /// (`:239`, `:210`, `:300`/`:322`/`:341`) — and a plain quit passes `undefined`. Without it an
    /// extension observing a replacement could not tell WHICH session it was going to, so
    /// transcript-linking, audit trails and intercom identity handoff across a switch or fork were
    /// impossible. SEAM-025.
    pub async fn dispose_with(
        &self,
        reason: &str,
        before_invalidate: Option<crate::runtime::BeforeSessionInvalidate>,
        target_session_file: Option<String>,
    ) {
        self.abort_and_settle().await;
        self.fanout_emit(AgentSessionEvent::SessionShutdown { reason: reason.to_string() }).await;
        // Notify extensions, then release the long-lived token.
        let cancel = self.session_cancel.child_token();
        self.services
            .ext_host
            .dispatcher()
            .dispatch_notify(
                &HostEvent::SessionShutdown {
                    reason: reason.to_string(),
                    target_session_file,
                },
                &cancel,
            )
            .await;
        // Pi `this.beforeSessionInvalidate?.()` (agent-session-runtime.ts:176 and :403): the last
        // point at which this session — and the extension context bound to it — is still live.
        if let Some(hook) = before_invalidate {
            hook();
        }
        // pi `teardownCurrent` runs `this.beforeSessionInvalidate?.()` and then
        // `this.session.dispose()` (`core/agent-session-runtime.ts:176-177` @v0.83.0), and
        // `dispose()` ends in `this._extensionRunner.invalidate(<stale text>)`
        // (`core/agent-session.ts:851-853`) — the latch that makes a captured extension ctx refuse
        // to act, plus the event-bus unsubscribe sweep (`core/extensions/loader.ts:206-215`).
        //
        // cyrup reached exactly this position and then never invalidated: `invalidate_live` had
        // ONE caller in the workspace, `ExtensionHost::reload`. So `/new`, `/resume`, `/fork` and
        // `/switch` all left every live guest of the OUTGOING session un-stale and still
        // subscribed, and pi's `assertActive` refusal never fired for a call still in flight on
        // one of them: it could go on emitting and subscribing, and go on being reached by its own
        // host's dispatch, after the session it belongs to was torn down.
        //
        // What that does NOT mean — a claim that stood here and was wrong — is that those handlers
        // received events destined for the REPLACEMENT session's set. `SharedBus` is per host
        // (`ExtensionHost::new` builds its own, `cyrup-ext/src/facade.rs`) and the replacement gets
        // a fresh host from `SessionBuilder::build`, so the two sets never shared a bus and there
        // was no cross-talk between them. The damage is confined to the outgoing set acting on
        // after its session is gone, which is what `assertActive` exists to stop.
        //
        // `None` takes the ported default stale text (`GuestState::invalidate`); the call is a
        // no-op on the `--no-default-features` arm, which is why it needs no `#[cfg]` here. See
        // `impl Drop for AgentSession` for the backstop covering the paths that never get here.
        self.services.ext_host.invalidate_live(None);
        self.session_cancel.cancel();
    }

    /// Resolve a fork anchor against the **live** session manager (SEAM-009).
    ///
    /// Pi resolves the anchor BEFORE it splits on persistence: `getEntry(entryId)` +
    /// `throw new Error("Invalid entry ID for forking")` at agent-session-runtime.ts:275-276 and
    /// :282-283, i.e. strictly above the `isPersisted()` branch at :290. cyrup used to resolve it
    /// against a throwaway manager reopened from the session FILE, which meant an unsaved session
    /// had no validation at all (a bogus entry id "succeeded") and no anchor to branch at.
    ///
    /// Reading the live manager is also strictly more correct for the persisted case: a branched
    /// session defers its first file write until an assistant message exists
    /// (`create_branched_session`), so the on-disk copy can legitimately lag the in-memory entries.
    pub(crate) async fn fork_anchor_live(
        &self,
        entry: &EntryId,
        position: ForkPosition,
    ) -> Result<(Option<EntryId>, Option<String>), SessionServiceError> {
        let mgr = self.manager.lock().await;
        fork_anchor(&mgr, entry, position)
    }

    /// Branch the **live, non-persisted** session manager at `target_leaf`, IN PLACE (SEAM-009).
    ///
    /// Pi's in-memory fork branch mutates the very object the outgoing session still holds:
    /// `const sessionManager = this.session.sessionManager; …
    /// sessionManager.createBranchedSession(targetLeafId); await this.teardownCurrent("fork", …)`
    /// (agent-session-runtime.ts:333-341). Branching first and tearing down second is not
    /// incidental: the outgoing run is still writing, and everything it appends while it settles
    /// lands in the *already-branched* manager — which is the manager the fork is built from. That
    /// is how Pi honours its own teardown contract, "the aborted turn (including tool results) is
    /// persisted to the outgoing session before it is replaced" (:167-169), on the fork path.
    ///
    /// So this method deliberately does NOT hand the manager over; [`Self::take_manager`] does, and
    /// the caller must settle the outgoing run in between. Merging the two (branch + move in one
    /// step, as this used to) re-opens the data loss from the other side: every append made between
    /// the move and the teardown goes to the throwaway placeholder and is dropped with it.
    ///
    /// Before any of this, the in-memory arm built a `SessionTarget::New` session and the whole
    /// transcript was silently discarded — unrecoverable, since a non-persisted session has no file
    /// to recover it from.
    pub(crate) async fn branch_live_manager(
        &self,
        target_leaf: &EntryId,
    ) -> Result<(), SessionServiceError> {
        let mut guard = self.manager.lock().await;
        // `create_branched_session` returns early for a non-persisted manager (adopting the branch
        // in memory and returning `None`), so the layout is unused here — pass the manager's own,
        // which is what the persisted arm would use too.
        let layout = branch_layout(&guard);
        guard.create_branched_session(target_leaf, &layout)?;
        Ok(())
    }

    /// Move this session's manager out from behind its lock, leaving a fresh empty in-memory
    /// manager in its place, so `SessionFactory::build_from_manager` (which takes the manager by
    /// value) can adopt it — cyrup's stand-in for Pi passing `this.session.sessionManager` straight
    /// into `createRuntime` (agent-session-runtime.ts:341).
    ///
    /// **The caller must have settled this session's run first.** Anything the session writes after
    /// this call lands in the placeholder and is lost when the session is dropped. The sole caller
    /// (the runtime's non-persisted fork arm) awaits `abort_and_settle()` immediately before, then
    /// disposes and replaces the session.
    pub(crate) async fn take_manager(&self) -> Result<SessionManager, SessionServiceError> {
        let mut guard = self.manager.lock().await;
        let placeholder = SessionManager::in_memory(
            guard.cwd(),
            cyrup_session::manager::NewSessionOpts::default(),
        )?;
        Ok(std::mem::replace(&mut *guard, placeholder))
    }

    /// Invalidate every live subscription on replacement (R-11-021): emit the terminal
    /// `SessionReplaced{generation}` and drop all senders so consumers re-subscribe.
    pub async fn notify_replaced(&self, generation: u64) {
        self.fanout.invalidate(generation).await;
    }

    /// Bind this session to its extension host and announce it as a FRESH START (Pi
    /// `bindExtensions`, agent-session.ts:2229-2251, whose tail is
    /// `await this._extensionRunner.emit(this._sessionStartEvent)`; that event defaults to
    /// `{type:"session_start", reason:"startup"}` at agent-session.ts:389).
    ///
    /// This is the seam every host calls exactly once for the INITIAL session, before any prompt —
    /// pi does it from print-mode.ts:73, rpc-mode.ts:318 and interactive-mode.ts:1698. In cyrup the
    /// bindings themselves are installed at build time, so the remaining work is the announcement.
    ///
    /// Session REPLACEMENTS (`new`/`resume`/`fork`/`reload`) are announced by the runtime's install
    /// tail with their own reason, which is why this is idempotent per session: whichever tier
    /// announces first wins and a later bind is a no-op (pi likewise emits `_sessionStartEvent`
    /// exactly once per `AgentSession`).
    pub async fn bind_extensions(&self) {
        self.bind_extensions_with(BindOptions::default()).await;
    }

    /// [`Self::bind_extensions`] with pi's `bindExtensions({ … })` options — SEAM-006.
    ///
    /// pi's hosts pass THREE keys (`print-mode.ts:73-101` @v0.83.0, `:74-119` @v0.84.1):
    /// * `mode: mode === "json" ? "json" : "print"` (`:74`) — already live in cyrup, installed at
    ///   build time from `cfg.app_mode` (`builder.rs`'s `ext_mode` → `HostConfig { mode, has_ui }`),
    ///   because cyrup's builder knows the resolved mode where pi's `bindExtensions` is the first
    ///   place that does;
    /// * `commandContextActions` (`:75-97`) — already live as `RuntimeActions`, installed onto every
    ///   session by `AgentSessionRuntime::install_inner`;
    /// * `onError` (`:98-100`) — **this**, and it was the piece with no counterpart: the print and
    ///   json hosts registered no error sink at all, so an extension fault under `cyrup -p` or
    ///   `cyrup --mode json` was contained and never surfaced anywhere. See [`BindOptions::on_error`].
    pub async fn bind_extensions_with(&self, opts: BindOptions) {
        if let Some(listener) = opts.on_error {
            self.services.ext_host.add_error_listener(listener);
        }
        self.emit_session_start("startup", None).await;
    }

    /// Announce this (freshly-installed) session to its subscribers + extensions (Pi `session_start`,
    /// agent-session-runtime.ts:215). `reason` ∈ `startup`/`new`/`resume`/`fork`/`reload`.
    ///
    /// At most ONE announcement is emitted per session (pi emits `_sessionStartEvent` once, from
    /// `bindExtensions`); subsequent calls return without emitting.
    pub async fn emit_session_start(&self, reason: &str, previous_session_file: Option<String>) {
        if self.start_announced.swap(true, Ordering::SeqCst) {
            return;
        }
        self.fanout_emit(AgentSessionEvent::SessionStart {
            reason: reason.to_string(),
            previous_session_file: previous_session_file.clone(),
        })
        .await;
        let cancel = self.session_cancel.child_token();
        self.services
            .ext_host
            .dispatcher()
            .dispatch_notify(
                &HostEvent::SessionStart {
                    reason: reason.to_string(),
                    // SEAM-025 — pi's `SessionStartEvent.previousSessionFile?`, "Present for
                    // \"new\", \"resume\", and \"fork\"" (`core/extensions/types.ts:562-569`
                    // @v0.83.0), populated at `agent-session-runtime.ts:305`/`:328`/`:347`. The
                    // FACADE event already carried it; the EXTENSION event dropped it, so the one
                    // consumer that cannot see the facade — an extension — was the one that lost it.
                    previous_session_file: previous_session_file.clone(),
                },
                &cancel,
            )
            .await;
        // EXT-004: `session_start` is Pi's canonical place to register a tool dynamically
        // (`examples/extensions/dynamic-tools.ts`). Pi's `registerTool` refreshes the registry
        // inline; cyrup's crosses a SYNC wasm import, so the async push happens here — before any
        // prompt, so the very first turn already sees the tool.
        self.refresh_extension_tools().await;
    }

    // --------------------------------------------------------------- model control ----

    /// Switch the active model by pattern (`provider/id[:level]`), updating the agent, the
    /// compaction model, and recording a model-change entry (R-11-014 `set_model`).
    ///
    /// The pattern resolves against the FULL multi-provider registry (Pi resolves against the whole
    /// `modelRegistry`, not just the active provider) so a `/model` selection targeting a DIFFERENT
    /// provider than the current one resolves cleanly; [`Self::set_model_resolved`] then swaps the
    /// owning provider. Falls back to the current provider's own catalog for custom-id / offline faux
    /// models that are not part of the built-in registry.
    pub async fn set_model(&self, pattern: &str) -> Result<ModelRef, SessionServiceError> {
        let resolved = {
            let candidates = self.full_model_registry();
            let resolver = cyrup_config::ModelResolver::new(&candidates);
            resolver.parse_pattern(pattern, true).model
        }
        .ok_or_else(|| SessionServiceError::ModelNotFound(pattern.to_string()))?;
        self.set_model_resolved(resolved).await
    }

    /// Switch to a resolved [`Model`] (Pi `setModel(Model)`, agent-session.ts:1448-1463), running the
    /// `hasConfiguredAuth` precheck first. When the target model's provider differs from the currently
    /// installed one, the owning provider is resolved (env-backed credentials installed) and swapped
    /// into the agent's stream source in place — 1:1 with Pi switching model+provider together.
    /// Updates the agent + compaction model + attribution headers + host-services view and records a
    /// `model_change` entry.
    pub async fn set_model_resolved(&self, model: Model) -> Result<ModelRef, SessionServiceError> {
        if !self.has_configured_auth(&model) {
            return Err(SessionServiceError::NoConfiguredAuth(format!(
                "{}/{}",
                model.provider.as_str(),
                model.id.as_str()
            )));
        }
        self.install_owning_provider(&model)?;
        let previous = Self::lock(&self.model).clone();
        self.apply_model_change(&model, previous.as_ref(), "set", None).await?;
        Ok(ModelRef {
            provider: model.provider.clone(),
            api: Some(model.api.clone()),
            model: model.id.clone(),
        })
    }

    /// Cross-provider select: rebuild + install the owning provider so the agent loop streams
    /// against it (Pi switches model+provider live). A same-provider change is a no-op.
    ///
    /// Pi does not need this step: its `ModelRuntime` keeps every provider live and dispatches on
    /// `model.provider` inside `prepareRequest` (model-runtime.ts:445-470), so `setModel` and
    /// `cycleModel` alike are a bare `agent.state.model = next` assignment. cyrup installs exactly
    /// ONE provider at a time, so every path that can land on a model owned by another provider —
    /// [`Self::set_model_resolved`] **and** both `cycle_model` arms, whose candidate sets span the
    /// whole auth-filtered registry — has to swap it here or the next turn streams against the
    /// wrong provider.
    fn install_owning_provider(&self, model: &Model) -> Result<(), SessionServiceError> {
        if self.provider.current().id().as_str() == model.provider.as_str() {
            return Ok(());
        }
        // A guest-registered provider is already a realized `Provider` in the shared registry
        // (arch-08 §5.6); install it DIRECTLY so its models stream — the built-in
        // `ProviderResolver` seam (bin `select_provider`) knows only the Pi registry, not a guest
        // provider. Falls back to the resolver for a built-in cross-provider swap.
        if let Some(guest) = self.services.guest_providers.provider(model.provider.as_str()) {
            self.provider.store(guest);
        } else {
            self.provider.resolve_and_store(model.provider.as_str()).map_err(|e| {
                SessionServiceError::NoConfiguredAuth(format!(
                    "{}/{}: {e}",
                    model.provider.as_str(),
                    model.id.as_str()
                ))
            })?;
        }
        Ok(())
    }

    /// Whether the model has usable auth (Pi `modelRegistry.hasConfiguredAuth`, agent-session.ts:1449
    /// / model-registry.ts:658-664). Pi's check: the model's provider has configured auth — a stored
    /// credential, a runtime `--api-key`, or a known env var (e.g. `TOGETHER_API_KEY` for `together`).
    /// cyrup layers its offline-faux accommodation on top: a model the CURRENT injected provider
    /// exposes in its catalog is always usable (the scripted faux provider needs no key), so the
    /// active/offline model stays selectable exactly as before.
    pub fn has_configured_auth(&self, model: &Model) -> bool {
        if self.provider_has_configured_auth(&model.provider) {
            return true;
        }
        // A guest-registered provider carries its own credentials (apiKey/oauth in the registration,
        // Pi `providerRequestConfigs`, model-registry.ts:659-662), so its models are always available
        // in the selector — exactly as Pi's `hasConfiguredAuth` returns true when a provider request
        // config supplies a key.
        if self.services.guest_providers.has_provider(model.provider.as_str()) {
            return true;
        }
        self.provider
            .current()
            .models()
            .iter()
            .any(|m| m.provider == model.provider && m.id == model.id)
    }

    /// Whether `provider` has configured auth in the Pi sense — a stored credential / runtime
    /// `--api-key` / known env var (`env_keys`, e.g. `together` → `TOGETHER_API_KEY`), **or** a
    /// `models.json` block of its own carrying a configured `apiKey`.
    ///
    /// Both tiers live in one place, [`cyrup_config::provider_is_configured`], shared with the
    /// binary's default-launch predicate (`main.rs`) — the two used to be written out separately and
    /// had drifted, which is CFG-022. The models.json tier stays PRESENCE-ONLY: it never resolves the
    /// value, so a `!command` `apiKey` cannot execute a shell command on a status query; see that
    /// function's docs for why Pi's own check (provider-composer.ts:320-329) is pure too.
    ///
    /// Does NOT count the offline faux accommodation or guest-registered providers —
    /// [`Self::has_configured_auth`] adds those separately.
    fn provider_has_configured_auth(&self, provider: &ProviderId) -> bool {
        cyrup_config::provider_is_configured(
            &self.services.auth,
            &self.services.model_config,
            provider,
            None,
        )
    }

    /// pi's `checkAuth` SECOND CHANCE — `(await this._modelRuntime.checkAuth(provider)) !== undefined`
    /// (`agent-session.ts:1184` @v0.83.0). PROV-037.
    ///
    /// [`Self::has_configured_auth`] is the cached, synchronous `hasConfiguredAuth` half
    /// (`model-runtime.ts:372-374`, a `Set` lookup on a snapshot). This is the live half upstream
    /// ORs with it, and the OR is the whole point: a credential that exists but is not in the
    /// cached set — an `auth.json` written by `cyrup auth` in another terminal after this session
    /// started, an `/login` in a sibling process — lets the turn PROCEED where cyrup refused it.
    ///
    /// `[CYRUP-DELTA]` **mechanism.** pi re-runs `Models.checkAuth`, which resolves the provider's
    /// auth strategy from scratch. Reaching `cyrup_provider::Models::check_auth` from here would
    /// mean composing a whole registry per refusal (the session holds one installed `Provider`, not
    /// a `Models`), and the only reason upstream's re-check can differ from its cached answer is
    /// that the SNAPSHOT is stale — so this refreshes the snapshot ([`AuthStore::reload`], pi's own
    /// `AuthStorage.reload`, `auth-storage.ts:204-215`) and re-asks the same predicate. That covers
    /// every credential source `has_configured_auth` knows and reproduces the observable
    /// difference; what it does not cover is a provider whose auth is ambient in a way
    /// `provider_is_configured` cannot see (Bedrock's IAM chain, Vertex ADC), which is a gap in the
    /// PREDICATE and identical before and after this call.
    ///
    /// Runs only on the refusal path, so the file read costs nothing on a normal turn.
    async fn recheck_provider_auth(&self, model: &Model) -> bool {
        self.services.auth.reload();
        self.has_configured_auth(model)
    }

    /// pi `ModelRuntime.isUsingOAuth(providerId)` — `this.snapshot.auth.get(providerId)?.type ===
    /// "oauth"` (`model-runtime.ts:368-370` @v0.83.0). PROV-037.
    ///
    /// Reads the STORED credential's type, which is exactly what upstream's snapshot holds. A read
    /// failure or an absent credential is `false`, so the refusal falls to the api-key message —
    /// the same direction upstream's `?.` takes.
    async fn provider_is_oauth_backed(&self, provider: &ProviderId) -> bool {
        matches!(
            self.services.auth.read(provider).await,
            Ok(Some(cyrup_config::Credential::Oauth { .. }))
        )
    }

    /// Public view of [`Self::full_model_registry`] — every model the session can resolve, before
    /// the configured-auth filter [`Self::available_model_catalog`] applies.
    pub fn full_model_catalog(&self) -> Vec<Model> {
        self.full_model_registry()
    }

    /// The FULL multi-provider model registry, deduped by `provider/id`: the session's own installed
    /// provider + guest-registered providers + the compiled-in built-in catalogs, with
    /// `<agent_dir>/models.json` composed over the whole union LAST — Pi's single composed registry
    /// (`ModelRuntime.rebuildProviders`, model-runtime.ts:225-231). This is the resolution /
    /// enumeration source that spans providers, independent of which single provider is installed.
    fn full_model_registry(&self) -> Vec<Model> {
        // --- BASE layer, in Pi's `recomposeProvider` precedence (model-runtime.ts:201) ---
        // `base = nativeExtensionProviders.get(id) ?? builtins.get(id)`: a registered provider
        // shadows the compiled-in catalog, and the compiled-in catalog fills in the rest. The
        // session's own installed provider comes first because it also carries the offline faux
        // models and any custom-id model that is not a registry entry.
        let mut base: Vec<Model> = self.provider.current().models().to_vec();
        // Guest-registered providers (Pi folds `registerProvider` models into the same `ModelRegistry`
        // that `find`/`getAvailable`/`setModel` read, model-registry.ts:917-940).
        for m in self.services.guest_providers.models() {
            if !base.iter().any(|e| e.provider == m.provider && e.id == m.id) {
                base.push(m);
            }
        }
        for m in cyrup_provider::default_models(cyrup_provider::CreateModelsOptions {
            credentials: None,
            auth_context: None,
            // The pi.dev overlay loaded once at session-build time (DRIFT-007). Already in memory,
            // so this SYNC, hot registry read stays free of disk and network I/O.
            catalog_overlay: self.services.catalog_overlay.clone(),
        })
        .get_models(None)
        {
            if !base.iter().any(|e| e.provider == m.provider && e.id == m.id) {
                base.push(m);
            }
        }
        // --- TOP layer: `<agent_dir>/models.json` (CFG-002) ---
        // Pi composes LAST and REPLACES the provider in the collection
        // (`this.models.setProvider(composeModelProvider(...))`, model-runtime.ts:215), so the
        // overlay reaches EVERY consumer — including the provider the session is currently running
        // on, which is the whole point of a `baseUrl` / `compat` / `modelOverrides` block ("point my
        // provider at a proxy", "raise contextWindow on the model I'm using"). Composing over the
        // union rather than over the compiled-in catalogs alone is what keeps the current provider's
        // uncomposed entries from shadowing their composed counterparts. Composition errors were
        // already reported at startup (`StartupDiagnostics::models`); here a rejected provider block
        // simply keeps its built-ins.
        let (composed, _errors) = self.services.model_config.compose(&base);
        composed
    }

    /// The models the `/model` selector offers: the FULL registry filtered to CONFIGURED providers
    /// (Pi `modelRegistry.getAvailable()` = `getAll().filter(hasConfiguredAuth)`,
    /// model-registry.ts:644-646, surfaced by the selector at model-selector.ts:152). A provider is
    /// configured when it has a stored credential / runtime `--api-key` / known env var (so `together`
    /// appears once `TOGETHER_API_KEY` is set), plus cyrup's offline-faux accommodation keeps the
    /// current provider's own catalog (the scripted faux default) selectable. Deduped by `provider/id`.
    pub fn available_model_catalog(&self) -> Vec<Model> {
        self.full_model_registry().into_iter().filter(|m| self.has_configured_auth(m)).collect()
    }

    /// The provider-attribution + session-affinity headers this session attaches to provider requests
    /// for `model` (Pi `mergeProviderAttributionHeaders`, sdk.ts:323; #20). Computed from the merge
    /// function + the session's telemetry flag + id. The builder threads the resolved model's headers
    /// onto the agent at construction; this getter lets callers inspect/recompute them per model.
    pub fn attribution_headers(&self, model: &Model) -> Option<cyrup_provider::HeaderMap> {
        crate::attribution::merge_provider_attribution_headers(
            model,
            self.telemetry_enabled,
            Some(&self.session_id),
            &[],
        )
    }

    /// The attribution overlay for the model a TURN is actually going to, addressed by the loop's
    /// [`ModelRef`] rather than by a resolved [`Model`] (AGENT-029). This is the body of the
    /// `transform_headers` resolver installed in [`Self::into_shared`] — pi's per-request
    /// `transformHeaders` closure (`sdk.ts:318-327` @v0.83.0), whose `model` argument is the model
    /// of that very request.
    ///
    /// Fast path: the session's own resolved model, which is what every turn uses unless a
    /// `prepare_next_turn` hook retargeted the loop. Only a genuinely different model pays for a
    /// registry scan. `None` (an unknown model id) means "no opinion" and leaves the agent's static
    /// [`cyrup_agent::Agent::set_headers`] overlay in place.
    fn headers_for_model_ref(&self, m: &ModelRef) -> Option<cyrup_provider::HeaderMap> {
        let matches = |c: &Model| c.provider == m.provider && c.id == m.model;
        let current = Self::lock(&self.compaction_model).clone();
        if let Some(cur) = current.as_ref().filter(|c| matches(c)) {
            return self.attribution_headers(cur);
        }
        self.full_model_registry().into_iter().find(matches).and_then(|c| self.attribution_headers(&c))
    }

    /// Emit the `model_select` extension event when the model actually changes (Pi `_emitModelSelect`,
    /// agent-session.ts:1429-1440). `source` ∈ `set`/`cycle`/`restore`. No-op when the model is
    /// unchanged (Pi `modelsAreEqual` guard).
    async fn emit_model_select(
        &self,
        next: &cyrup_provider::Model,
        previous: Option<&ModelRef>,
        source: &str,
    ) {
        // `modelsAreEqual`: same provider + id. Pi's `previousModel` is `Model | undefined`
        // (agent-session.ts:1559-1571) and `modelsAreEqual(undefined, next)` is false, so the FIRST
        // `/model` on a modelless session emits with `previousModel: undefined`.
        if previous.is_some_and(|p| p.provider == next.provider && p.model == next.id) {
            return;
        }
        let cancel = self.session_cancel.child_token();
        // EXT-042: `model`, `previousModel` and `source` are THREE SIBLING fields on pi's
        // `ModelSelectEvent` (`core/extensions/types.ts:792-799` @v0.83.0), emitted as such at
        // `agent-session.ts:1565-1570`. They used to be nested INSIDE `model` here, so a ported
        // handler read `event.previousModel` and got `undefined`.
        let model_val = serde_json::json!({
            "provider": next.provider.as_str(),
            "id": next.id.as_str(),
        });
        let previous_model = previous.map(|p| {
            serde_json::json!({ "provider": p.provider.as_str(), "id": p.model.as_str() })
        });
        self.services
            .ext_host
            .dispatcher()
            .dispatch_notify(
                &HostEvent::ModelSelect {
                    model: model_val,
                    previous_model,
                    source: source.to_string(),
                },
                &cancel,
            )
            .await;
    }

    /// Set the active model directly from a provider+id pair (no pattern matching).
    pub async fn set_model_id(
        &self,
        provider: ProviderId,
        model: ModelId,
    ) -> Result<(), SessionServiceError> {
        let model_ref = ModelRef { provider: provider.clone(), api: None, model: model.clone() };
        self.agent.set_model(model_ref.clone()).await;
        *Self::lock(&self.model) = Some(model_ref.clone());
        self.bash_session_env.set_model(provider.to_string(), model.to_string());
        // Same per-request attribution rule as `apply_model_change` (pi `sdk.ts:318-327`). This
        // path has only the `ModelRef`, so resolve the full `Model` to recompute; if it cannot be
        // resolved the overlay is cleared rather than left stale — sending the OLD provider's
        // attribution is worse than sending none.
        let resolved = self
            .provider
            .current()
            .models()
            .iter()
            .find(|m| m.id == model_ref.model && m.provider == model_ref.provider)
            .cloned();
        self.agent
            .set_headers(resolved.as_ref().and_then(|m| self.attribution_headers(m)))
            .await;
        self.manager.lock().await.append_model_change(provider, model)?;
        Ok(())
    }

    /// Drain + apply control ops a loaded extension queued via its `control` capability (Pi
    /// `createCommandContext`, agent-session.ts:1158; arch-08 §6.3). This is the command-tier-safe
    /// point that bridges the SYNC guest `control()` call to the real ASYNC session effect: a guest
    /// that calls `session.setThinkingLevel(...)` / `setModel(...)` / `sendUserMessage(...)` / a
    /// compaction reaches [`crate::host_services::LiveHostServices`], which queues the op; here it is
    /// applied. Mutating from a command tier respects the deadlock rule (R-08-008): never called
    /// from inside the agent loop.
    ///
    /// SEAM-003: this is now a SINK, not a filter. It used to return the runtime-tier ops
    /// (`new_session`/`switch`/`fork`/`navigate`/`reload`/`wait_idle`/`send_message`) "for the
    /// runtime to act on" — and its single production caller (`try_execute_wasm_command`) dropped
    /// the returned vector, while the NATIVE command route never drained at all. Every op is now
    /// routed here:
    ///
    /// * `NewSession`/`Switch`/`Fork`/`Reload` → the installed [`crate::RuntimeActions`] sink (Pi
    ///   binds these to the real `runtimeHost.*` in every host, rpc-mode.ts:321-346).
    /// * `Navigate`/`WaitIdle`/`SendMessage`/`SendUserMessage`/`Compact` → applied in place; they
    ///   are session-local and need no runtime host.
    /// * `SetModel`/`SetThinkingLevel`/`Abort`/`Shutdown` → the `Send`-safe shared helper
    ///   [`Self::apply_agent_state_op`], so the event-tier drain handles them identically.
    ///
    /// A failure is reported through the extension host's error listener (the same channel a
    /// contained handler fault uses) — never a silent drop, and never a panic.
    pub async fn apply_pending_control(&self) {
        // Fan out the facade events a guest state-mutation queued (entry_appended/session_info_changed):
        // the guest appended/renamed synchronously via `LiveHostServices`; emit here — the same
        // command-tier-safe bridge point the control ops drain at — so listeners observe them.
        for ev in self.services.host_services.take_pending_events() {
            self.fanout_emit(ev).await;
        }
        // Push the tool set a guest `setActiveTools` restricted the session to onto the live agent
        // (Pi `setActiveTools` = `setActiveToolsByName`, agent-session.ts:2283,850-854). The guest
        // updated the authoritative dynamic-tool view synchronously across the wasm-suspended call
        // (so `getActiveTools` already reflects it); the ASYNC agent push lands here — the same
        // command-tier-safe bridge point control ops / pending events drain at — before the next turn.
        // EXT-004: surface any tool an extension registered since the last drain (Pi calls
        // `refreshTools()` from `registerTool` itself; cyrup's registration crosses a SYNC wasm
        // import, so the async agent push lands at this same bridge point). Ordered BEFORE the
        // explicit `setActiveTools` push below so an extension that registered a tool AND then
        // restricted the active set in the same handler gets what it asked for — in Pi the refresh
        // happens inside `registerTool`, i.e. strictly earlier than any later `setActiveTools`, and
        // `setActiveToolsByName` is always the last word.
        self.refresh_extension_tools().await;
        if let Some((tools, prompt)) = self.services.host_services.take_pending_active_tools() {
            self.push_active_tools(tools, prompt).await;
        }
        let ops = self.services.host_services.take_pending_control();
        for op in ops {
            // Agent-state + lifecycle ops (SetModel/SetThinkingLevel/Abort/Shutdown) apply in place
            // via the shared `Send`-safe helper; it returns `Some(op)` for anything it did not
            // handle so the routing below stays exhaustive.
            let Some(op) = self.apply_agent_state_op(op).await else { continue };
            let name = control_op_name(&op);
            let outcome = match op {
                ControlOp::SendUserMessage { content, .. } => {
                    // A guest `sendUserMessage` op re-enters the prompt path (`send_user_message` →
                    // `prompt_accepted` → `prepare` → `try_execute_extension_command`), closing an
                    // `async fn` cycle. Box this cold re-entry edge so the future stays finitely
                    // sized (E0733) without adding indirection to the hot prompt path.
                    Box::pin(self.send_user_message(content, None)).await.map(|_| ())
                }
                // Pi `ctx.compact(options)` (extensions/types.ts:344): `customInstructions`
                // (types.ts:296-300) rides the op through to the summarizer — the same
                // `Option<String>` a `/compact <instructions>` slash command passes.
                ControlOp::Compact { custom_instructions } => {
                    self.compact(custom_instructions).await.map(|_| ())
                }
                // ---- session-local runtime ops (no runtime host needed) ----
                ControlOp::Navigate { entry_id, opts } => {
                    Box::pin(self.control_navigate(&entry_id, &opts)).await
                }
                ControlOp::WaitIdle => {
                    // Pi's `waitForIdle` is a promise that cannot deadlock the command path; cyrup's
                    // waits on the post-run driver watch. This drain normally runs BEFORE
                    // `spawn_run`, so the flag is already false — but a concurrent run would
                    // otherwise block the command path indefinitely, so bound it and surface the
                    // expiry instead of hanging.
                    match tokio::time::timeout(WAIT_IDLE_CONTROL_TIMEOUT, self.wait_for_idle()).await
                    {
                        Ok(()) => Ok(()),
                        Err(_) => Err(SessionServiceError::Io(
                            "control op `wait_idle` timed out waiting for the agent to settle".into(),
                        )),
                    }
                }
                ControlOp::SendMessage { message, opts } => {
                    Box::pin(self.control_send_message(&message, &opts)).await
                }
                // ---- RUNTIME-tier ops: only a host that installed a `RuntimeActions` can do these ----
                ControlOp::NewSession { opts } => match self.runtime_actions.get() {
                    Some(rt) => rt.new_session(&opts).await,
                    None => Err(SessionServiceError::NoRuntimeHost("new_session")),
                },
                ControlOp::Switch { session_id, opts } => match self.runtime_actions.get() {
                    Some(rt) => rt.switch_session(&session_id, &opts).await,
                    None => Err(SessionServiceError::NoRuntimeHost("switch_session")),
                },
                ControlOp::Fork { entry_id, opts } => match self.runtime_actions.get() {
                    Some(rt) => rt.fork(&entry_id, &opts).await,
                    None => Err(SessionServiceError::NoRuntimeHost("fork")),
                },
                ControlOp::Reload => match self.runtime_actions.get() {
                    Some(rt) => rt.reload().await,
                    None => Err(SessionServiceError::NoRuntimeHost("reload")),
                },
                // Handled by `apply_agent_state_op` above; unreachable, but keep the match total so
                // a future `ControlOp` variant is a compile error rather than a silent drop.
                other => Err(SessionServiceError::Io(format!("unrouted control op: {other:?}"))),
            };
            if let Err(e) = outcome {
                self.report_control_failure(name, &e);
            }
        }
    }

    /// Apply a `ControlOp::Navigate` (Pi `ctx.navigateTree(targetId, {summarize, customInstructions,
    /// replaceInstructions, label})`, extensions/types.ts:1665-1668, bound to `session.navigateTree`
    /// at rpc-mode.ts:325-337).
    async fn control_navigate(
        &self,
        entry_id: &str,
        opts: &serde_json::Value,
    ) -> Result<(), SessionServiceError> {
        let options = NavigateTreeOptions {
            summarize: opts.get("summarize").and_then(serde_json::Value::as_bool).unwrap_or(false),
            custom_instructions: opts
                .get("customInstructions")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            replace_instructions: opts
                .get("replaceInstructions")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            label: opts.get("label").and_then(serde_json::Value::as_str).map(str::to_string),
        };
        self.navigate_tree(EntryId::from(entry_id), options).await.map(|_| ())
    }

    /// Apply a `ControlOp::SendMessage` (Pi `ctx.sendMessage(message, {triggerTurn, deliverAs})`,
    /// extensions/types.ts:395-398/1223). `message` is the guest's
    /// `Pick<CustomMessage, "customType"|"content"|"display"|"details">`.
    async fn control_send_message(
        &self,
        message: &serde_json::Value,
        opts: &serde_json::Value,
    ) -> Result<(), SessionServiceError> {
        use serde_json::Value;
        let custom_type = message
            .get("customType")
            .and_then(Value::as_str)
            .unwrap_or("extension")
            .to_string();
        let content = message.get("content").cloned().unwrap_or(Value::Null);
        let display = message.get("display").and_then(Value::as_bool).unwrap_or(true);
        let details = message.get("details").cloned();
        let deliver_as = match opts.get("deliverAs").and_then(Value::as_str) {
            Some("steer") => Some(crate::event::DeliverAs::Steer),
            Some("followUp") => Some(crate::event::DeliverAs::FollowUp),
            Some("nextTurn") => Some(crate::event::DeliverAs::NextTurn),
            _ => None,
        };
        // Pi's `triggerTurn` runs a fresh turn OVER the custom message when idle
        // (`_runAgentPrompt(appMessage)`); `deliverAs` takes precedence, exactly as in
        // `send_custom_message`/`inject_message`.
        let trigger_turn = opts.get("triggerTurn").and_then(Value::as_bool).unwrap_or(false);
        if trigger_turn && deliver_as.is_none() && !self.is_streaming().await {
            let msg = AgentMessage::Custom {
                kind: custom_type,
                payload: content,
                timestamp: Some(now_ms()),
            };
            return self.spawn_run(vec![msg]).await;
        }
        self.send_custom_message(&custom_type, content, display, details, deliver_as).await
    }

    /// Surface a control-op failure. SEAM-003's contract is that an op is either PERFORMED or
    /// REPORTED — never silently dropped, which is exactly what the old `let _deferred = …` did.
    /// Pi's pre-bind action stubs throw `"Extension runtime not initialized…"`
    /// (extensions/loader.ts:173-176 `notInitialized`) rather than no-op; cyrup cannot throw across
    /// the drain, so it warns.
    fn report_control_failure(&self, op: &str, err: &SessionServiceError) {
        tracing::warn!(op = %op, error = %err, "extension control op failed");
    }

    /// Apply a single AGENT-STATE / LIFECYCLE control op in place, returning `None` when it was one
    /// of those (handled) or `Some(op)` when it is some other op the caller must route itself.
    ///
    /// `SetModel`/`SetThinkingLevel` are pure agent-state mutations the next turn reads (Pi
    /// `setModel`/`setThinkingLevel`, agent-session.ts:1476-1490 / 1541-1572). `Abort`/`Shutdown`
    /// join them because Pi puts BOTH on the base `ExtensionContext` — "Available in all contexts"
    /// (extensions/types.ts:339,344) — so `cyrup-ext`'s `control::Host` deliberately does not
    /// `require_command_tier()` them and they can arrive from an EVENT handler. Handling them here,
    /// in the shared helper, is what makes the event-tier turn-boundary drain
    /// ([`Self::apply_pending_agent_control`]) service them instead of re-queueing them until some
    /// later command happens to run.
    ///
    /// Shared by [`Self::apply_pending_control`] (command-tier drain) and
    /// [`Self::apply_pending_agent_control`] so the two never drift. Note it does NOT touch the
    /// `send_user_message`/`compact` re-entry arms — whose prompt-path futures are `!Send` — so a
    /// caller that needs a `Send` future (the spawned post-run driver) can use it.
    async fn apply_agent_state_op(&self, op: ControlOp) -> Option<ControlOp> {
        match op {
            ControlOp::SetThinkingLevel(level) => {
                if let Some(lv) = crate::builder::thinking_level_from_str(&level) {
                    let _ = self.set_thinking_level(lv).await;
                }
                None
            }
            ControlOp::SetModel(v) => {
                if let Some((provider, model)) = parse_model_ref(&v) {
                    let _ = self.set_model_id(provider, model).await;
                }
                None
            }
            // Pi `ctx.abort()` (types.ts:339): "Abort the current agent run." Bound at
            // agent-session.ts:2405 to `void this.abort()`.
            ControlOp::Abort => {
                self.abort();
                None
            }
            // Pi `ctx.shutdown()` (types.ts:344) → the host's `shutdownHandler`, which in Pi's RPC
            // mode is exactly `() => { shutdownRequested = true }` (rpc-mode.ts:344-346); the host
            // acts on it at the next `agent_settled`.
            ControlOp::Shutdown => {
                self.shutdown_requested.store(true, Ordering::SeqCst);
                None
            }
            other => Some(other),
        }
    }

    /// GAP-11 event-tier turn-boundary drain: apply the AGENT-STATE control ops
    /// (`SetModel`/`SetThinkingLevel`) a guest queued from an EVENT handler (`on_message_end` /
    /// `on_input` / a mid-turn tool hook / `on_agent_end`), at a STORE-FREE point (after a run settles
    /// or after `emit_input_event` returns — every `LiveExtension.inner` store guard released), so the
    /// change takes effect on the SUBSEQUENT turn, matching Pi (which mutates synchronously from any
    /// handler, loader.ts:342-354). The re-emit (`thinking_level_select`/`model_select`) fires here as
    /// a fresh top-level guest call, never a re-entry into the suspended event-hook store.
    ///
    /// This is the `Send`-safe subset of [`Self::apply_pending_control`]: only SetModel/
    /// SetThinkingLevel can reach the queue from an event handler (every other control op stays
    /// command-tier-gated in live.rs), and this never touches the `!Send` `send_user_message`/
    /// `compact` arms — so it runs inside the spawned post-run driver ([`Self::drive_run`]). It also
    /// drains the same pending facade-event / active-tool fan-out `apply_pending_control` does, so a
    /// guest that appended/renamed/restricted tools from the event handler is observed here too. Any
    /// op it does not handle is re-queued (never dropped) for the command-tier drain.
    async fn apply_pending_agent_control(&self) {
        for ev in self.services.host_services.take_pending_events() {
            self.fanout_emit(ev).await;
        }
        // EXT-004, event-tier twin of the drain in `apply_pending_control` (same ordering rule:
        // the refresh runs first so an explicit `setActiveTools` still has the last word).
        self.refresh_extension_tools().await;
        if let Some((tools, prompt)) = self.services.host_services.take_pending_active_tools() {
            self.push_active_tools(tools, prompt).await;
        }
        for op in self.services.host_services.take_pending_control() {
            if let Some(other) = self.apply_agent_state_op(op).await {
                // Unreachable in practice — live.rs gates every non-base-context control op to the
                // command tier, so only SetModel/SetThinkingLevel/Abort/Shutdown can be queued from
                // an event handler, and `apply_agent_state_op` handles all four. Re-queue (never
                // drop) as a guard so a future gating change can't silently lose a command-tier op;
                // the command-tier drain (`apply_pending_control`) will handle it.
                let _ = cyrup_ext::host::HostServices::control(&*self.services.host_services, other);
            }
        }
    }

    // --------------------------------------------------------------- thinking control ----

    /// The agent's current thinking level (Pi `thinkingLevel` getter, agent-session.ts:763).
    pub async fn thinking_level(&self) -> ModelThinkingLevel {
        self.agent.snapshot().await.thinking_level
    }

    /// The thinking levels the active model supports (Pi `getAvailableThinkingLevels`,
    /// agent-session.ts:1721-1724). A non-reasoning model supports only `off`.
    pub fn available_thinking_levels(&self) -> Vec<ModelThinkingLevel> {
        // Pi `if (!this.model) return THINKING_LEVELS;` (agent-session.ts:1722), where
        // `const THINKING_LEVELS: ThinkingLevel[] = ["off", "minimal", "low", "medium", "high"]`
        // (agent-session.ts:297) — note it stops at `high`; `xhigh`/`max` are model-declared only.
        let Some(model) = ({ Self::lock(&self.compaction_model).clone() }) else {
            return vec![
                ModelThinkingLevel::Off,
                ModelThinkingLevel::Minimal,
                ModelThinkingLevel::Low,
                ModelThinkingLevel::Medium,
                ModelThinkingLevel::High,
            ];
        };
        cyrup_provider::get_supported_thinking_levels(&model)
    }

    /// Whether the active model supports reasoning/thinking (Pi `supportsThinking`,
    /// agent-session.ts:1729-1731).
    pub fn supports_thinking(&self) -> bool {
        // Pi `return !!this.model?.reasoning;` (agent-session.ts:1730) — false with no model.
        Self::lock(&self.compaction_model).as_ref().is_some_and(|m| m.reasoning)
    }

    /// Set the thinking level, clamping to the model's capabilities, persisting a
    /// `thinking_level_change` entry and emitting the `thinking_level_select` ext event + the
    /// facade event — but only when the effective level actually changes (Pi `setThinkingLevel`,
    /// agent-session.ts:1677-1698).
    pub async fn set_thinking_level(
        &self,
        level: ModelThinkingLevel,
    ) -> Result<ModelThinkingLevel, SessionServiceError> {
        let model = { Self::lock(&self.compaction_model).clone() };
        // Pi `_clampThinkingLevel`: `return this.model ? clampThinkingLevel(this.model, level)
        // : "off";` (agent-session.ts:1608-1610) — a modelless session clamps everything to off.
        let effective = match model.as_ref() {
            Some(m) => cyrup_provider::clamp_thinking_level(m, level),
            None => ModelThinkingLevel::Off,
        };
        let previous = self.agent.snapshot().await.thinking_level;
        self.agent.set_thinking_level(effective).await;
        // Republish `CYRUP_REASONING_LEVEL` for the NEXT `bash` child (Pi re-reads `ctx.thinkingLevel`
        // on every `resolveSpawnContext`, bash.ts:180). Pushed BEFORE the no-change early return so
        // the handle is authoritative even when this call is a clamp-to-the-same-value no-op.
        self.bash_session_env
            .set_reasoning_level(crate::builder::thinking_level_to_str(effective));
        if effective == previous {
            return Ok(effective);
        }
        let level_str = crate::builder::thinking_level_to_str(effective);
        self.manager.lock().await.append_thinking_level_change(&level_str)?;
        // Only with a model installed: a guest's `ctx.model` stays `undefined` on a modelless
        // session (pi's `ExtensionContext.model` is the optional `session.model`).
        if let (Some(mr), Some(m)) = (Self::lock(&self.model).clone(), model.as_ref()) {
            self.services.host_services.update_model(
                mr,
                m.context_window,
                Some(level_str.clone()),
            );
        }
        self.fanout_emit(AgentSessionEvent::ThinkingLevelChanged { level: level_str.clone() }).await;
        let cancel = self.session_cancel.child_token();
        self.services
            .ext_host
            .dispatcher()
            .dispatch_notify(
                &HostEvent::ThinkingLevelSelect {
                    level: level_str,
                    // pi's `previousLevel` (`extensions/types.ts:802-806`), read off the agent
                    // snapshot above and emitted only on a real change — the `effective ==
                    // previous` early return is pi's `if (isChanging)` guard
                    // (`agent-session.ts:1688-1697`).
                    previous_level: Some(
                        crate::builder::thinking_level_to_str(previous).to_string(),
                    ),
                },
                &cancel,
            )
            .await;
        Ok(effective)
    }

    /// Apply a `transport` settings value to the RUNNING agent — the second half of pi's
    /// `/settings` "Transport" handler:
    ///
    /// ```ts
    /// onTransportChange: (transport) => {
    ///     this.settingsManager.setTransport(transport);
    ///     this.session.agent.transport = transport;   // interactive-mode.ts:4215
    /// },
    /// ```
    ///
    /// cyrup did only the persist half, so cycling the row wrote JSON that nothing re-read until the
    /// next process start. `s` is the persisted `TransportSetting` string (`"auto" | "sse" |
    /// "websocket" | "websocket-cached"`); an unrecognized value falls back to `auto`, matching
    /// `getTransport()`'s `?? "auto"` (settings-manager.ts:751). Returns the transport actually
    /// applied. Takes effect from the NEXT run, exactly as pi's `createLoopConfig` read does
    /// (agent.ts:442).
    pub async fn set_transport(&self, s: &str) -> cyrup_provider::Transport {
        let t = crate::builder::parse_transport(s);
        self.agent.set_transport(Some(t)).await;
        t
    }

    /// Cycle to the next thinking level (Pi `cycleThinkingLevel`, agent-session.ts:1551). Returns
    /// `None` when the model does not support thinking.
    pub async fn cycle_thinking_level(&self) -> Result<Option<ModelThinkingLevel>, SessionServiceError> {
        if !self.supports_thinking() {
            return Ok(None);
        }
        let levels = self.available_thinking_levels();
        if levels.is_empty() {
            return Ok(None);
        }
        let current = self.thinking_level().await;
        let idx = levels.iter().position(|l| *l == current).unwrap_or(0);
        let Some(&next) = levels.get((idx + 1) % levels.len()) else {
            return Ok(None);
        };
        Ok(Some(self.set_thinking_level(next).await?))
    }

    // ----------------------------------------------------- steering / follow-up mode ----

    /// The agent's current steering mode (Pi `steeringMode` getter, agent-session.ts:845).
    pub fn steering_mode(&self) -> cyrup_agent::QueueMode {
        *Self::lock(&self.steering_mode)
    }

    /// The agent's current follow-up mode (Pi `followUpMode` getter, agent-session.ts:850).
    pub fn follow_up_mode(&self) -> cyrup_agent::QueueMode {
        *Self::lock(&self.follow_up_mode)
    }

    /// Set the steering-message delivery mode (Pi `setSteeringMode`, agent-session.ts:1631).
    pub fn set_steering_mode(&self, mode: cyrup_agent::QueueMode) {
        self.agent.set_steering_mode(mode);
        *Self::lock(&self.steering_mode) = mode;
    }

    /// Set the follow-up-message delivery mode (Pi `setFollowUpMode`, agent-session.ts:1640).
    pub fn set_follow_up_mode(&self, mode: cyrup_agent::QueueMode) {
        self.agent.set_follow_up_mode(mode);
        *Self::lock(&self.follow_up_mode) = mode;
    }

    // ----------------------------------------------------------------- read access ----

    /// The current model address, or `None` when the session has no model at all.
    ///
    /// Pi `get model(): Model<any> | undefined { return this.agent.state.model; }`
    /// (agent-session.ts:865-868) — documented there as "may be undefined if not yet selected".
    /// `None` is the state a credential-less first run launches in (SEAM-075): `main.ts:852-855`
    /// exits 1 on it in every NON-interactive mode, while interactive shows the
    /// `modelFallbackMessage` banner and waits for `/login` + `/model`.
    pub fn model(&self) -> Option<ModelRef> {
        Self::lock(&self.model).clone()
    }

    /// The model-restore fallback warning, if the resumed session's saved model was unavailable
    /// (Pi `modelFallbackMessage`, sdk.ts:91).
    pub fn model_fallback_message(&self) -> Option<&str> {
        self.model_fallback_message.as_deref()
    }

    /// Whether a run is currently streaming.
    pub async fn is_streaming(&self) -> bool {
        self.agent.snapshot().await.is_streaming
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// The session header record (Pi `sessionManager.getHeader()`, session-manager.ts:1208-1211):
    /// `{type:"session", version, id, timestamp, cwd, parentSession?}`. This is the passthrough a
    /// `--mode json` run serializes as JSONL line 1 before the event stream (Pi print-mode.ts:112-117).
    /// A live session always carries a header (unlike Pi's `getHeader` which is nominally nullable
    /// when no `session` entry exists — never the case for an opened/created manager), so this
    /// returns the header directly rather than an `Option`.
    pub async fn session_header(&self) -> SessionHeader {
        self.manager.lock().await.header().clone()
    }

    /// Claim the one-shot JSON-mode header emission for this session. Returns `true` for the FIRST
    /// caller and `false` thereafter, so that a multi-prompt `--mode json` run — whose initial
    /// submission and each follow-up are dispatched as separate `run_json` calls — writes the header
    /// line exactly once, matching Pi's single `getHeader()` write ahead of the whole message loop
    /// in `runPrintMode` (print-mode.ts:112-119).
    pub fn claim_json_header(&self) -> bool {
        !self.json_header_written.swap(true, Ordering::SeqCst)
    }

    /// The on-disk session file, if this session is persisted.
    pub async fn session_file(&self) -> Option<std::path::PathBuf> {
        self.manager.lock().await.session_file().map(Path::to_path_buf)
    }

    /// The cwd-bound services this session wired (settings/auth/resources/ext host/model/prompt).
    pub fn services(&self) -> &AgentSessionServices {
        &self.services
    }

    /// The captured extension CLI flag values threaded from the CLI (Pi `extensionFlagValues`,
    /// main.ts:634). A loaded extension consumes these via `applyExtensionFlagValues`; the read-only
    /// seam is surfaced here so the threading is observable end-to-end.
    pub fn extension_flag_values(&self) -> &[(String, crate::builder::ExtensionFlagValue)] {
        &self.services.extension_flag_values
    }

    /// The `trust.json` store path for this session (`agent_dir/trust.json`, Pi
    /// `EnvVars::trustPath`). The additive data seam the `/trust` selector writes through.
    pub fn trust_store_path(&self) -> std::path::PathBuf {
        self.services.agent_dir.join("trust.json")
    }

    /// The standard project-trust options for this session's cwd (Pi `getProjectTrustOptions`,
    /// trust-manager.ts:65; `cyrup_config::trust::trust_options`). Drives the `/trust` selector rows.
    pub fn project_trust_options(&self) -> Vec<cyrup_config::trust::TrustOption> {
        cyrup_config::trust::trust_options(&self.services.cwd, false)
    }

    /// The nearest saved trust decision for this session's cwd (Pi `findNearestTrustEntry`); `None`
    /// when no ancestor has a persisted decision. Read-only; surfaced in the `/trust` selector header.
    pub fn saved_trust_decision(&self) -> Option<cyrup_config::trust::TrustEntry> {
        cyrup_config::trust::TrustStore::new(self.trust_store_path())
            .nearest(&self.services.cwd)
            .ok()
            .flatten()
    }

    /// Persist a project-trust decision (the `updates` of a [`cyrup_config::trust::TrustOption`]) to
    /// the `trust.json` store (Pi `/trust` `onSelect` → `setProjectTrust`, trust-manager.ts). An empty
    /// `updates` (session-only option) writes nothing. The in-memory `services().project_trusted`
    /// reflects the new session only after a `/reload`, matching Pi.
    pub fn write_project_trust(
        &self,
        updates: &[(std::path::PathBuf, Option<cyrup_config::trust::TrustDecision>)],
    ) -> Result<(), SessionServiceError> {
        if updates.is_empty() {
            return Ok(());
        }
        cyrup_config::trust::TrustStore::new(self.trust_store_path()).set_many(updates)?;
        Ok(())
    }

    /// Persist a settings field to the on-disk store (Pi `/settings`/`/config` selector apply →
    /// `SettingsManager.setNested`). Writes via the manager's `&self` store seam; the in-memory
    /// `effective()` view reflects it after a `/reload`, matching Pi's apply-then-reload flow. A
    /// dotted `key` (`terminal.showImages`) addresses a nested field. Project writes require trust.
    pub fn persist_setting(
        &self,
        scope: cyrup_config::SettingsScope,
        key: &str,
        value: serde_json::Value,
    ) -> Result<(), SessionServiceError> {
        let path: Vec<&str> = key.split('.').filter(|s| !s.is_empty()).collect();
        self.services.settings.persist_nested(scope, &path, value)?;
        Ok(())
    }

    /// The sessions root directory for this session (`agent_dir/sessions`, the layout default). The
    /// additive seam the `/resume` selector lists from.
    pub fn sessions_root(&self) -> std::path::PathBuf {
        self.services.agent_dir.join("sessions")
    }

    /// The directory THIS session's files live in — Pi `sessionManager.getSessionDir()`
    /// (session-manager.ts:999). Under an explicit `--session-dir`, or after resuming a file from
    /// somewhere else, this is NOT `<sessions_root>/--<encoded-cwd>--`.
    pub fn session_dir(&self) -> &Path {
        &self.services.session_dir
    }

    /// List the persisted sessions for this session, newest-first (Pi `SessionManager.list`,
    /// session-manager.ts:1638 → the `/resume` selector). Reads the session's OWN directory, exactly
    /// as Pi's picker does — `SessionManager.list(this.sessionManager.getCwd(),
    /// this.sessionManager.getSessionDir())` (interactive-mode.ts:4867) — so an explicit
    /// `--session-dir` (or a session resumed from elsewhere) lists the sessions actually next to
    /// this one rather than the cwd-encoded default dir, which may be empty or hold an unrelated set.
    ///
    /// A custom directory may pool SEVERAL projects' sessions in one flat dir, so Pi filters it by
    /// cwd — `filterCwd = sessionDir !== undefined && dir !== getDefaultSessionDirPath(cwd)`
    /// (session-manager.ts:1639-1643); the picker always passes `getSessionDir()`, so the predicate
    /// reduces to "not the cwd-encoded default". The default dir already isolates by cwd and so is
    /// never filtered. Same predicate the CLI `--resume` path computes (`session_list_cwd_filter`,
    /// crates/cyrup/src/main.rs:1132-1138). An absent/empty dir yields an empty list, never an error.
    pub fn list_sessions(&self) -> Vec<cyrup_session::listing::SessionInfo> {
        let dir = &self.services.session_dir;
        let default_dir =
            cyrup_session::SessionLayout::new(self.sessions_root(), self.services.cwd.clone()).dir();
        let cwd_filter = (*dir != default_dir).then_some(self.services.cwd.as_path());
        cyrup_session::listing::list_in_dir(dir, cwd_filter, None)
    }

    /// Delete a persisted session **file** by path (Pi `/resume` in-list delete → `app.session.delete`
    /// → `SessionManager.delete`, session-selector.ts:540). Additive seam for the TUI session selector:
    /// removes the JSONL from disk. Refuses to delete *this* session's own file (Pi guards the active
    /// session). An already-absent file is a no-op (idempotent), never an error.
    pub fn delete_session_file(&self, path: &Path) -> Result<DeleteMethod, SessionServiceError> {
        if let Some(active) = self.manager_path()
            && same_file(&active, path)
        {
            return Err(SessionServiceError::Io(
                "refusing to delete the active session".to_string(),
            ));
        }
        delete_session_file_at(path)
    }

    /// Set a persisted session's display **name** by path (Pi `/resume` in-list rename →
    /// `onRenameSession` → `SessionManager.setSessionName`, session-selector.ts:585). Additive seam:
    /// opens the target file, appends a `session_info` entry (the same persisted record
    /// [`Self::set_session_name`] writes for the active session), and lets the store flush. For the
    /// *active* session this routes through the live manager so the in-memory tree stays consistent.
    pub async fn rename_session_file(
        &self,
        path: &Path,
        name: &str,
    ) -> Result<(), SessionServiceError> {
        if let Some(active) = self.manager_path()
            && same_file(&active, path)
        {
            return self.set_session_name(name).await;
        }
        let mut mgr = cyrup_session::SessionManager::open(path)?;
        mgr.append_session_info(name)?;
        Ok(())
    }

    /// The on-disk path of this session's own JSONL, if the live manager exposes one (used to guard the
    /// active session from a `/resume` delete/rename).
    fn manager_path(&self) -> Option<std::path::PathBuf> {
        self.manager.try_lock().ok().and_then(|g| g.session_file().map(Path::to_path_buf))
    }

    /// The system prompt the builder assembled at session start (arch-06). Frozen — it does NOT
    /// track mid-session tool-set rebuilds; use [`Self::base_system_prompt`] for the live base or
    /// [`Self::current_system_prompt`] for the agent's in-flight value.
    pub fn system_prompt(&self) -> &str {
        &self.services.system_prompt
    }

    /// The LIVE base system prompt (Pi `this._baseSystemPrompt`, agent-session.ts:371) — the value a
    /// run falls back to when no `before_agent_start` handler replaced it. Equal to
    /// [`Self::system_prompt`] until a tool-set rebuild (`/tools` toggle, a guest `setActiveTools`,
    /// or EXT-004 late tool registration) rewrites it via [`Self::push_active_tools`].
    pub fn base_system_prompt(&self) -> String {
        Self::lock(&self.base_system_prompt).clone()
    }

    /// The `before_agent_start` replacement in force for the CURRENT run, if any (Pi
    /// `this._systemPromptOverride`, agent-session.ts:373 @v0.83.0). `None` between runs and
    /// whenever no handler replaced the prompt.
    pub fn system_prompt_override(&self) -> Option<String> {
        Self::lock(&self.system_prompt_override).clone()
    }

    /// `override ?? base` — the exact expression pi evaluates at every site that writes
    /// `agent.state.systemPrompt` (agent-session.ts:534 in the turn-boundary refresh, `:940` in
    /// `setActiveToolsByName` @v0.83.0). This is the value the agent must be running with at any
    /// moment, and the value the per-turn refresh re-pushes (DRIFT-033).
    pub fn effective_system_prompt(&self) -> String {
        // Two statements, not one chained expression: the override guard must be released before
        // `base_system_prompt()` takes the second lock.
        let over = Self::lock(&self.system_prompt_override).clone();
        over.unwrap_or_else(|| self.base_system_prompt())
    }

    /// The agent's *current* system prompt — equal to the base unless a `before_agent_start` handler
    /// replaced it for the in-flight run (Pi `agent.state.systemPrompt`, agent-session.ts:1127).
    pub async fn current_system_prompt(&self) -> String {
        self.agent.snapshot().await.system_prompt
    }

    /// The current LLM context built from the session tree (leaf→root, R-04-011).
    pub async fn context(&self) -> SessionContext {
        self.manager.lock().await.build_context()
    }

    /// The persisted transcript messages on the current branch (R-11-014 `get_messages`).
    ///
    /// This is the **LLM-flattened** view (`convertToLlm`): a compaction/branch summary, an
    /// extension `custom` message and a `!` bash execution all arrive as `user` messages carrying
    /// their wrapper prose. Anything that RENDERS the conversation wants
    /// [`raw_context_messages`](Self::raw_context_messages) instead.
    pub async fn messages(&self) -> Vec<Message> {
        self.manager.lock().await.build_context().messages
    }

    /// The current branch's context with its **roles intact** — Pi's
    /// `buildContextEntries().flatMap(sessionEntryToContextMessages)`
    /// (`session-manager.ts:441-453` + `:383-408`), the input Pi's `renderSessionEntries` replays a
    /// resumed session from (interactive-mode.ts:3506-3516).
    ///
    /// Unlike [`messages`](Self::messages) this has NOT been through `convertToLlm`, so a
    /// `compactionSummary` / `branchSummary` / `custom` / `bashExecution` still identifies itself and
    /// a front-end can route it to its own component instead of drawing the wrapper text as a user
    /// turn.
    pub async fn raw_context_messages(&self) -> Vec<cyrup_session::agent_message::AgentMessage> {
        self.manager.lock().await.build_context_raw()
    }

    /// The id of the current branch leaf (Pi `sessionManager.getLeafId()`, agent-session.ts:2705).
    /// `None` before any entry exists / after a reset-to-root. Drives the `/tree` overlay's
    /// current-position marker and its `navigate_tree` no-op guard.
    pub async fn leaf_id(&self) -> Option<EntryId> {
        self.manager.lock().await.leaf_id().cloned()
    }

    /// The handle the `read` tool reads to decide whether the ACTIVE model accepts image input
    /// (pi `tools/read.ts`'s non-vision warning). Seeded from the resolved model at build and
    /// re-pushed by `apply_model_change`, so the warning tracks `/model` switches rather than the
    /// startup model.
    #[must_use]
    pub fn read_model_vision(&self) -> &cyrup_tools::config::ModelVisionHandle {
        &self.read_model_vision
    }

    /// The agent's LIVE per-request header overlay (pi `SimpleStreamOptions.headers`, recomputed
    /// per request in `streamFn`, `sdk.ts:318-327`). Tracks the active model via
    /// [`Self::attribution_headers`] on both model-change paths.
    pub async fn agent_headers(&self) -> Option<cyrup_provider::HeaderMap> {
        self.agent.snapshot().await.headers
    }

    /// The agent's current in-memory transcript (includes the streaming partial).
    pub async fn agent_messages(&self) -> Vec<cyrup_agent::AgentMessage> {
        self.agent.snapshot().await.messages
    }

    /// The most recent assistant message text on the current branch (print-mode helper).
    pub async fn last_assistant_text(&self) -> Option<String> {
        self.messages().await.into_iter().rev().find_map(|m| match m {
            Message::Assistant(AssistantMessage { content, .. }) => {
                let text: String = content
                    .iter()
                    .filter_map(|c| match c {
                        cyrup_core::Content::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                if text.is_empty() { None } else { Some(text) }
            }
            _ => None,
        })
    }

    // -------------------------------------------------------------------- state views ----

    /// Aggregate session stats (Pi `getSessionStats`, agent-session.ts:3112; RPC
    /// `get_session_stats`).
    ///
    /// SEAM-031: computed from `sessionManager.getEntries()` — ALL entries, including history a
    /// compaction replaced — not from the rebuilt LLM context, so token/cost totals reflect what was
    /// actually billed across the session (Pi's own docstring, agent-session.ts:3107-3111).
    pub async fn session_stats(&self) -> crate::state::SessionStats {
        let context_usage = self.stats_context_usage().await;
        let mgr = self.manager.lock().await;
        crate::state::SessionStats::from_entries(
            mgr.entries(),
            self.session_id.to_string(),
            mgr.session_file().map(|p| p.display().to_string()),
            context_usage,
        )
    }

    /// Per-model cost/token breakdown for `/session` (Pi `getUsageCostBreakdown(entries)`, called
    /// from `handleSessionCommand` at `interactive-mode.ts:5665` @v0.83.0). PROV-036.
    ///
    /// Reads the SAME `mgr.entries()` [`Self::session_stats`] reads — every entry, including
    /// history a compaction replaced — so the rows sum to `SessionStats::cost` exactly.
    pub async fn usage_cost_breakdown(&self) -> Vec<crate::state::UsageCostBreakdownEntry> {
        let mgr = self.manager.lock().await;
        crate::state::usage_cost_breakdown(mgr.entries())
    }

    /// Session-wide prompt-cache waste for `/session` (Pi `computeCacheWaste(entries,
    /// this.session.modelRuntime)`, `interactive-mode.ts:5660` @v0.83.0). PROV-035.
    ///
    /// The price source is the session's full model registry, which is what pi's `modelRuntime`
    /// argument resolves `getModel(provider, id)?.cost.cacheRead` against. A model the registry
    /// does not know prices at `0`, exactly as pi's `?? undefined` fallback does — so an unknown
    /// model still contributes its MISSED TOKENS, just no dollar figure.
    pub async fn cache_waste(&self) -> cyrup_provider::cache_stats::CacheWasteTotals {
        let models = self.full_model_registry();
        let mgr = self.manager.lock().await;
        let scan = crate::state::cache_scan_entries(mgr.entries());
        cyrup_provider::cache_stats::compute_cache_waste(&scan, &models)
    }

    /// The `contextUsage` sub-object of [`Self::session_stats`], in Pi's `ContextUsage` shape
    /// (`{tokens, contextWindow, percent}`, extensions/types.ts:288-294). `None` when no model /
    /// no known context window — Pi's `getContextUsage` returns `undefined` there
    /// (agent-session.ts:3165-3170).
    ///
    /// Public because it is a 1:1 port of `AgentSession.getContextUsage()`, which upstream's footer
    /// calls directly on every render (`footer.ts:108`) to build its `{pct}%/{window}` segment. The
    /// TUI needs exactly this three-state answer — including the `percent: null` case — which the
    /// coarser [`Self::context_usage`] (always a number) cannot express.
    pub async fn stats_context_usage(&self) -> Option<crate::state::StatsContextUsage> {
        let usage = self.context_usage().await;
        if usage.context_window == 0 {
            return None;
        }
        // Pi's post-compaction guard (agent-session.ts:3175-3197). After a compaction the last
        // assistant `usage` still describes the PRE-compaction context, so reporting it would show a
        // stale — and much larger — occupancy as if it were current. Pi only trusts a usage from an
        // assistant that responded AFTER the latest compaction on this branch, and where that
        // assistant neither aborted nor errored and actually consumed context. With no such
        // assistant the count is genuinely unknown, and Pi returns `{tokens: null, percent: null}`
        // while still reporting the window.
        //
        // Without this branch `tokens`/`percent` were unconditionally `Some`, so the `null` case the
        // struct's own doc comment describes was unreachable.
        if !self.has_post_compaction_usage().await {
            return Some(crate::state::StatsContextUsage {
                tokens: None,
                context_window: usage.context_window,
                percent: None,
            });
        }
        Some(crate::state::StatsContextUsage {
            tokens: Some(usage.used_tokens),
            context_window: usage.context_window,
            percent: Some(usage.fraction * 100.0),
        })
    }

    /// `true` when this branch's occupied-token count can be trusted — i.e. there is no compaction
    /// on the branch, or an assistant has responded since the latest one (Pi
    /// `getContextUsage`'s `hasPostCompactionUsage` scan, agent-session.ts:3181-3193).
    ///
    /// Scans backwards from the branch tail to the compaction boundary, matching Pi's loop
    /// direction, and accepts the first assistant that neither aborted nor errored and whose usage
    /// accounts for a non-zero context.
    async fn has_post_compaction_usage(&self) -> bool {
        use cyrup_core::StopReason;
        use cyrup_session::entry::{Entry, KnownEntry};
        use cyrup_session::AgentMessage;

        let guard = self.manager.lock().await;
        // Pi scans `sessionManager.getBranch()` (agent-session.ts:3174, indexed at :3181-3193) —
        // the ACTIVE BRANCH — not `getEntries()`. `entries()` is the flat append-only store
        // (`manager.rs:818`): after a `/fork` or a `/tree` navigation it also holds the abandoned
        // branch, so `rposition` could latch an OFF-BRANCH compaction as the boundary and `skip`
        // could then count an off-branch assistant as post-compaction usage — printing a stale
        // pre-compaction occupancy as current, the exact failure this guard exists to prevent.
        // `branch_path(None)` is cyrup's `getBranch()`, and is O(branch-depth) rather than O(all
        // entries) besides (TUI-092 F4 C2).
        let path = guard.branch_path(None);
        let Some(compaction_idx) = path
            .iter()
            .rposition(|e| matches!(e, Entry::Known(KnownEntry::Compaction { .. })))
        else {
            // No compaction on this branch: the last assistant usage is current by construction.
            return true;
        };
        path.iter()
            .copied()
            .skip(compaction_idx + 1)
            .rev()
            .filter_map(|e| match e {
                Entry::Known(KnownEntry::Message {
                    message: AgentMessage::Core(Message::Assistant(a)),
                    ..
                }) => Some(a),
                _ => None,
            })
            .any(|a| {
                // Same four-field sum `ContextUsage::from_last_assistant` uses, so "consumed
                // context" means the same thing in both places (Pi `calculateContextTokens`).
                let context_tokens =
                    a.usage.input + a.usage.cache_read + a.usage.cache_write + a.usage.output;
                !matches!(a.stop_reason, StopReason::Aborted | StopReason::Error)
                    && context_tokens > 0
            })
    }

    /// Context-window occupancy from the last assistant turn (Pi `getContextUsage`,
    /// agent-session.ts:3164-3208 @v0.83.0).
    ///
    /// Answers from a **reverse walk of the active branch's entries** — Pi's own
    /// `sessionManager.getBranch()` shape (`:3174`) — never from a rebuilt message list. The
    /// previous body called [`Self::messages`], i.e. `build_context()` →
    /// `build_context_messages()`, which deep-clones every message on the branch (tool payloads
    /// included) purely so this function could reverse the vector, take the first assistant, and
    /// drop the rest: O(session history) of allocation on **every** `MessageEnd`, awaited on the
    /// TUI run-loop task (TUI-092 F4).
    pub async fn context_usage(&self) -> crate::state::ContextUsage {
        use cyrup_core::StopReason;
        use cyrup_session::entry::{Entry, KnownEntry};
        use cyrup_session::AgentMessage;

        // Pi `getContextUsage`: `const model = this.model; if (!model) return undefined;`
        // (agent-session.ts:3165-3166) and `if (contextWindow <= 0) return undefined;` (:3168-3169).
        // Taken FIRST, exactly as Pi orders it — the model read precedes `getBranch()` at :3174 — so
        // the `compaction_model` leaf lock is released before the async `manager` guard is acquired
        // and no lock-nesting question arises at all. cyrup's return type is non-optional, so the
        // modelless case degrades to a zero window, which `from_last_assistant` already renders as
        // fraction 0.0 — the same "unknown occupancy" the TUI shows for an undefined usage.
        let window = { Self::lock(&self.compaction_model).as_ref().map_or(0, |m| m.context_window) };

        let guard = self.manager.lock().await;
        // The last assistant ON THE ACTIVE BRANCH, by parent-link walk — the same answer
        // `messages().await.iter().rev().find_map(..)` gave, without building or cloning the
        // branch's whole message list to get it.
        //
        // `StopReason::Deferred` is skipped because the OLD path could not return one:
        // `push_as_message`'s first arm drops a deferred assistant from the built context
        // (`cyrup-session/src/context.rs:62`, `is_deferred_assistant` at `:114-120`). A deferred
        // turn is a durable provider handle with empty content, not a settled context measurement
        // (`cyrup-core/src/message.rs:172-188`), so its `usage` must not drive the footer. cyrup
        // cannot produce one yet, but a Pi-written session carrying one must still read identically
        // (R-00-013). `filter_map(..).find(..)` — not `find_map` — so a deferred tail does not stop
        // the scan; same shape as the neighbour `has_post_compaction_usage`.
        let last = guard
            .branch_path(None)
            .into_iter()
            .rev()
            .filter_map(|e| match e {
                Entry::Known(KnownEntry::Message {
                    message: AgentMessage::Core(Message::Assistant(a)),
                    ..
                }) => Some(a),
                _ => None,
            })
            .find(|a| a.stop_reason != StopReason::Deferred);
        crate::state::ContextUsage::from_last_assistant(last, window)
    }

    /// A serializable snapshot of the session for RPC `get_state`.
    ///
    /// cyrup-original in shape: Pi's `RpcSessionState` (`modes/rpc/rpc-types.ts:95-108`, built at
    /// `modes/rpc/rpc-mode.ts:446-461`) carries twelve scalars and NO occupancy or stats, and Pi's
    /// `state` getter is `return this.agent.state` (agent-session.ts:863-865). The extra `stats` /
    /// `context_usage` fields are cyrup's.
    pub async fn state_view(&self) -> crate::state::SessionStateView {
        let stats = self.session_stats().await;
        let messages = self.messages().await;
        // ONE producer for occupancy, as upstream has: Pi's `getSessionStats` does not re-derive it
        // either, it returns `contextUsage: this.getContextUsage()` (agent-session.ts:3170).
        // Deriving it inline here duplicated the pre-F4 windowed-build scan, so it disagreed with
        // `GetContextUsage` whenever a compaction's kept window held no assistant while an earlier
        // pre-compaction assistant existed — including every unresolvable-v1 `first_kept_entry_id`
        // session, whose kept window is empty by construction (`cyrup-session/src/context.rs:166-172`).
        let context_usage = self.context_usage().await;
        let model = Self::lock(&self.model).clone();
        crate::state::SessionStateView {
            session_id: self.session_id.to_string(),
            cwd: self.services.cwd.display().to_string(),
            provider: model.as_ref().map(|m| m.provider.to_string()),
            model: model.as_ref().map(|m| m.model.to_string()),
            session_name: self.session_name().await,
            is_streaming: self.is_streaming().await,
            message_count: messages.len(),
            pending_message_count: self.pending_message_count(),
            stats,
            context_usage,
        }
    }

    async fn fanout_emit(&self, ev: AgentSessionEvent) {
        // Reuse the same fan-out the agent subscriber feeds; session-level events interleave with
        // agent events on the live streams.
        self.fanout.emit_external(ev).await;
    }

    /// Forward events posted to `rx` onto the seam fan-out, from a DEDICATED task.
    ///
    /// Pi emits `summarization_retry_*` / `bash_execution_update` inline from synchronous callbacks
    /// (`agent-session.ts:2645-2668` and `:2785-2787`); cyrup's fan-out is `async` (it awaits
    /// per-subscriber backpressure) and those callbacks are not, so they post to an unbounded
    /// channel that this task drains. Two properties are deliberate:
    ///
    /// * **A separate task, not an inline `select!`.** Every summarization-bearing operation runs
    ///   while holding the session-manager lock, and [`Self::fanout_emit`] can block on a lagging
    ///   subscriber's bounded channel. Emitting inline would mean awaiting that backpressure WITH
    ///   the manager lock held — and a subscriber that is both lagging and waiting on the manager
    ///   lock would deadlock the session. Every other emit site in this file drops the guard first
    ///   for exactly that reason; this task keeps that invariant without serializing the operation
    ///   behind the fan-out.
    /// * **Ends when the last sender drops.** Callers close the queue by dropping the emitter (the
    ///   compactor / bash sink that owns it) and then `await` the returned handle — AFTER releasing
    ///   the manager guard — which flushes every queued event BEFORE the operation's own terminal
    ///   event (`compaction_end`, the bash result), matching Pi's inline ordering. An early return
    ///   that never awaits the handle still terminates the task, because the emitter is dropped at
    ///   scope exit.
    fn spawn_event_pump(
        &self,
        mut rx: tokio::sync::mpsc::UnboundedReceiver<AgentSessionEvent>,
    ) -> tokio::task::JoinHandle<()> {
        let fanout = Arc::clone(&self.fanout);
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                fanout.emit_external(ev).await;
            }
        })
    }

    /// Persist a custom (non-LLM) message via the session tree (Pi `sendCustomMessage` durable path,
    /// agent-session.ts:1313). The agent transcript carries it as a `Custom` role for the next run.
    pub async fn append_custom_message(
        &self,
        custom_type: &str,
        content: serde_json::Value,
        display: bool,
    ) -> Result<EntryId, SessionServiceError> {
        let id = self
            .manager
            .lock()
            .await
            .append_custom_message(custom_type, content, display, None)?;
        Ok(id)
    }

    /// Send a user message that always triggers a turn (Pi `sendUserMessage`, agent-session.ts:1351).
    /// While the agent is streaming, the message is queued per `deliver_as` (steer / follow-up)
    /// instead of starting a new run.
    pub async fn send_user_message(
        &self,
        input: impl Into<UserInput>,
        deliver_as: Option<StreamingBehavior>,
    ) -> Result<PromptAccepted, SessionServiceError> {
        let ui = input.into();
        if self.is_streaming().await {
            return match deliver_as {
                Some(StreamingBehavior::FollowUp) => self.follow_up(ui).await,
                _ => self.steer(ui).await,
            };
        }
        self.prompt_accepted(ui).await
    }

    /// Send a custom (non-LLM) message with delivery timing (Pi `sendCustomMessage`,
    /// agent-session.ts:1307-1338). `nextTurn` stages the message to ride the next prompt; `steer`/
    /// `followUp` queue onto the active run while streaming; otherwise the message is persisted and
    /// surfaced via `message_start`/`message_end`.
    pub async fn send_custom_message(
        &self,
        custom_type: &str,
        content: serde_json::Value,
        display: bool,
        details: Option<serde_json::Value>,
        deliver_as: Option<crate::event::DeliverAs>,
    ) -> Result<(), SessionServiceError> {
        use crate::event::DeliverAs;
        let ts = now_ms();
        let msg = AgentMessage::Custom {
            kind: custom_type.to_string(),
            payload: content.clone(),
            timestamp: Some(ts),
        };
        match deliver_as {
            Some(DeliverAs::NextTurn) => {
                Self::lock(&self.pending_next_turn).push(msg);
            }
            _ if self.is_streaming().await => match deliver_as {
                Some(DeliverAs::FollowUp) => self.agent.follow_up(msg),
                _ => self.agent.steer(msg),
            },
            _ => {
                self.manager
                    .lock()
                    .await
                    .append_custom_message(custom_type, content, display, details)?;
                self.fanout_emit(AgentSessionEvent::MessageStart { message: msg.clone() }).await;
                self.fanout_emit(AgentSessionEvent::MessageEnd { message: msg }).await;
            }
        }
        Ok(())
    }

    /// Inject a host-originated message into the live session and optionally trigger an agent turn
    /// (Pi `sendCustomMessage(message, { triggerTurn })`, agent-session.ts:1337-1370). Backs the
    /// late-bound [`crate::host_services::LiveHostServices`] inject sink a background task drives
    /// (R-SA-101 / P-2) — the seam that surfaces a completed background result INTO the parent
    /// session's turn loop instead of stderr. Reproduces Pi's three cases:
    ///
    /// * **`custom_type = None`** — a plain user message: Pi `sendUserMessage`, which ALWAYS triggers a
    ///   turn (steer/follow-up while streaming). `display`/`trigger_turn` don't apply to a user message.
    /// * **`Some(kind)` while streaming** — queue the custom message onto the active run (Pi `steer`).
    /// * **`Some(kind)`, idle, `trigger_turn`** — run a fresh turn OVER the custom message (Pi
    ///   `_runAgentPrompt(appMessage)`, `spawn_run(vec![msg])`) — the `triggerTurn` branch cyrup's
    ///   `send_custom_message` lacked.
    /// * **`Some(kind)`, idle, no `trigger_turn`** — persist + surface durably (Pi's else-branch).
    pub async fn inject_message(
        &self,
        content: String,
        custom_type: Option<String>,
        display: bool,
        trigger_turn: bool,
    ) -> Result<(), SessionServiceError> {
        let Some(kind) = custom_type else {
            // A plain user message: Pi `sendUserMessage` always triggers a turn (and steers/follows-up
            // while streaming). Boxed like the `SendUserMessage` control edge (`apply_pending_control`)
            // so the re-entry into the prompt path stays finitely sized (E0733).
            let _ = Box::pin(self.send_user_message(content, None)).await?;
            return Ok(());
        };
        let msg = AgentMessage::Custom {
            kind: kind.clone(),
            payload: serde_json::Value::String(content.clone()),
            timestamp: Some(now_ms()),
        };
        if self.is_streaming().await {
            // Pi: while streaming, queue onto the active run (steer).
            self.agent.steer(msg);
        } else if trigger_turn {
            // Pi `_runAgentPrompt(appMessage)`: run a turn whose input IS the injected message.
            self.spawn_run(vec![msg]).await?;
        } else {
            // Pi else-branch: append durably + surface via message_start/message_end.
            self.manager
                .lock()
                .await
                .append_custom_message(&kind, serde_json::Value::String(content), display, None)?;
            self.fanout_emit(AgentSessionEvent::MessageStart { message: msg.clone() }).await;
            self.fanout_emit(AgentSessionEvent::MessageEnd { message: msg }).await;
        }
        Ok(())
    }

    // --------------------------------------------------------------- model cycling ----

    /// The models available for `cycle_model` (Pi `scopedModels` getter, agent-session.ts:870).
    pub fn scoped_models(&self) -> Vec<ScopedModel> {
        Self::lock(&self.scoped_models).clone()
    }

    /// Replace the scoped-model cycle set (Pi `setScopedModels`, agent-session.ts:875).
    ///
    /// Also re-seeds the guest-visible mirror behind `ctx.scopedModels` (EXT-045). pi exposes the
    /// SAME field to extensions off the base context — `getScopedModels: () => this._scopedModels`
    /// (`core/agent-session.ts:2416`), read by `get scopedModels()`
    /// (`core/extensions/runner.ts:706-709`) — so the extension seam and the `/scoped-models`
    /// command must never disagree. `LiveHostServices`'s read is SYNC and this set lives behind a
    /// lock it does not own, so it is mirrored the way the system prompt already is; doing it HERE,
    /// in the single writer, is what keeps the mirror from lagging.
    pub fn set_scoped_models(&self, models: Vec<ScopedModel>) {
        // pi's `ScopedModel` is `{model: Model<Api>; thinkingLevel?: ThinkingLevel}`
        // (`core/model-resolver.ts:63-67` @v0.83.0). `thinkingLevel` is OMITTED when unset, exactly
        // as an `undefined` field is absent from upstream's object.
        let mirrored: Vec<serde_json::Value> = models
            .iter()
            .map(|sm| {
                let mut obj = serde_json::Map::new();
                if let Ok(model) = serde_json::to_value(&sm.model) {
                    obj.insert("model".to_string(), model);
                }
                if let Some(level) = sm.thinking_level
                    && let Ok(level) = serde_json::to_value(level)
                {
                    obj.insert("thinkingLevel".to_string(), level);
                }
                serde_json::Value::Object(obj)
            })
            .collect();
        self.services.host_services.update_scoped_models(mirrored);
        *Self::lock(&self.scoped_models) = models;
    }

    /// Cycle to the next/previous model (Pi `cycleModel`, agent-session.ts:1601-1671). Cycles over
    /// the scoped set when one is configured (filtered to models with configured auth), else the
    /// AUTH-FILTERED registry ([`Self::available_model_catalog`]). Returns a typed
    /// [`ModelCycleResult`] distinguishing the scoped vs available
    /// path + the restored thinking level, or `None` when there is one-or-fewer candidate. Applies
    /// the model + re-clamps/restores the thinking level, persists a `model_change`, and emits
    /// `model_changed` + the `model_select` ext event.
    pub async fn cycle_model(
        &self,
        forward: bool,
    ) -> Result<Option<ModelCycleResult>, SessionServiceError> {
        let scoped = Self::lock(&self.scoped_models).clone();
        if scoped.is_empty() {
            self.cycle_available_model(forward).await
        } else {
            self.cycle_scoped_model(forward, &scoped).await
        }
    }

    /// Cycle over the scoped set, honoring per-model thinking levels (Pi `_cycleScopedModel`,
    /// agent-session.ts:1608-1641).
    async fn cycle_scoped_model(
        &self,
        forward: bool,
        scoped: &[ScopedModel],
    ) -> Result<Option<ModelCycleResult>, SessionServiceError> {
        let candidates: Vec<&ScopedModel> =
            scoped.iter().filter(|s| self.has_configured_auth(&s.model)).collect();
        if candidates.len() <= 1 {
            return Ok(None);
        }
        let current = Self::lock(&self.model).clone();
        let cur_idx = candidates
            .iter()
            // pi `modelsAreEqual(sm.model, currentModel)` with `currentModel: Model | undefined`
            // never matches when the session is modelless, so `findIndex` returns -1 and pi's
            // `if (currentIndex === -1) currentIndex = 0` starts the cycle at the head
            // (agent-session.ts:1618-1621; the same shape at :1647-1650 for the available set).
            .position(|s| {
                current.as_ref().is_some_and(|c| {
                    s.model.provider == c.provider && s.model.id == c.model
                })
            })
            .unwrap_or(0);
        let len = candidates.len();
        let next_idx = if forward { (cur_idx + 1) % len } else { (cur_idx + len - 1) % len };
        let Some(next) = candidates.get(next_idx).copied() else {
            return Ok(None);
        };
        // Explicit scoped thinking level overrides; `None` inherits the current session level.
        let explicit = next.thinking_level;
        self.install_owning_provider(&next.model)?;
        let new_level = self
            .apply_model_change(&next.model, current.as_ref(), "cycle", explicit)
            .await?;
        Ok(Some(ModelCycleResult { model: next.model.clone(), thinking_level: new_level, is_scoped: true }))
    }

    /// Cycle over the AUTH-FILTERED registry (Pi `_cycleAvailableModel`, agent-session.ts:1643-1670,
    /// whose first line is `const availableModels = await this._modelRuntime.getAvailable()`).
    ///
    /// `getAvailable()` is `getAll().filter(hasConfiguredAuth)` across EVERY provider
    /// (model-runtime.ts:315-329 → models.ts:394-409), which is exactly
    /// [`Self::available_model_catalog`]. cyrup previously cycled `provider.current().models()` —
    /// the ONE installed provider's own catalog — so a user with `ANTHROPIC_API_KEY` and
    /// `OPENAI_API_KEY` could never cycle off the provider they launched on, the same SEAM-004 bug
    /// already fixed for `set_model`/`get_available_models` on the RPC seam.
    async fn cycle_available_model(
        &self,
        forward: bool,
    ) -> Result<Option<ModelCycleResult>, SessionServiceError> {
        let candidates = self.available_model_catalog();
        if candidates.len() <= 1 {
            return Ok(None);
        }
        let current = Self::lock(&self.model).clone();
        let cur_idx = candidates
            .iter()
            // Same `findIndex → -1 → 0` fallback as the scoped path (agent-session.ts:1647-1650).
            .position(|m| {
                current.as_ref().is_some_and(|c| m.provider == c.provider && m.id == c.model)
            })
            .unwrap_or(0);
        let len = candidates.len();
        let next_idx = if forward { (cur_idx + 1) % len } else { (cur_idx + len - 1) % len };
        let Some(next) = candidates.get(next_idx).cloned() else {
            return Ok(None);
        };
        self.install_owning_provider(&next)?;
        let new_level = self.apply_model_change(&next, current.as_ref(), "cycle", None).await?;
        Ok(Some(ModelCycleResult { model: next, thinking_level: new_level, is_scoped: false }))
    }

    /// Apply a resolved model change: push to the agent, re-derive headers, persist, re-clamp/restore
    /// the thinking level, emit `model_changed` + `model_select`. Returns the new thinking level.
    /// Shared by [`Self::set_model_resolved`] and the cycle paths.
    async fn apply_model_change(
        &self,
        next: &Model,
        previous: Option<&ModelRef>,
        source: &str,
        explicit_thinking: Option<ModelThinkingLevel>,
    ) -> Result<ModelThinkingLevel, SessionServiceError> {
        let model_ref = ModelRef {
            provider: next.provider.clone(),
            api: Some(next.api.clone()),
            model: next.id.clone(),
        };
        self.agent.set_model(model_ref.clone()).await;
        *Self::lock(&self.model) = Some(model_ref.clone());
        *Self::lock(&self.compaction_model) = Some(next.clone());
        self.services.host_services.update_model(model_ref, next.context_window, None);
        // Republish `CYRUP_PROVIDER`/`CYRUP_MODEL` for the NEXT `bash` child (Pi re-reads `ctx.model`
        // on every `resolveSpawnContext`, bash.ts:175-178; docs/environment-variables.md:27).
        self.bash_session_env.set_model(next.provider.to_string(), next.id.to_string());
        // ...and re-push the new model's image capability for the same reason: `read`'s
        // non-vision warning must describe the model the NEXT read will actually run against,
        // not the one resolved at startup.
        self.read_model_vision.set(next.supports_image_input());
        // pi recomputes provider-attribution + opencode session-affinity headers INSIDE `streamFn`,
        // dispatched on the model the request is actually going to (`sdk.ts:318-327`). cyrup merged
        // them once at session build and pinned them via `AgentBuilder::headers`, so a
        // cross-provider `/model` switch kept sending the PREVIOUS provider's attribution — an
        // OpenRouter `HTTP-Referer`/`X-Title` on an Anthropic request, or a stale opencode
        // session-affinity header. `attribution_headers()` existed and computed this correctly
        // per-model; it simply had no caller.
        self.agent.set_headers(self.attribution_headers(next)).await;
        self.manager.lock().await.append_model_change(next.provider.clone(), next.id.clone())?;
        // Re-clamp the thinking level for the new model (explicit override or current session level).
        let level = match explicit_thinking {
            Some(l) => l,
            None => self.thinking_level().await,
        };
        let new_level = self.set_thinking_level(level).await?;
        self.fanout_emit(AgentSessionEvent::ModelChanged {
            provider: next.provider.to_string(),
            model: next.id.to_string(),
        })
        .await;
        self.emit_model_select(next, previous, source).await;
        Ok(new_level)
    }

    // ------------------------------------------------------------- facade accessors ----

    /// The file-based prompt templates discovered for this session (Pi `promptTemplates` getter,
    /// agent-session.ts:880).
    pub fn prompt_templates(
        &self,
    ) -> &cyrup_resources::ResourceSet<cyrup_resources::PromptTemplate> {
        &self.services.resources.prompts
    }

    /// The currently-installed provider's model catalog (Pi `modelRegistry` getter,
    /// agent-session.ts:1412). Returned by value because the underlying provider is now swappable
    /// (see [`ProviderSwap`]); for the cross-provider `/model` list use
    /// [`Self::available_model_catalog`], which spans the full configured registry.
    pub fn model_catalog(&self) -> Vec<cyrup_provider::Model> {
        self.provider.current().models().to_vec()
    }

    /// The session-scoped resource registry (Pi `resourceLoader` getter, agent-session.ts:363).
    pub fn resources(&self) -> &Arc<cyrup_resources::ResourceRegistry> {
        &self.services.resources
    }

    /// Read-only handle to the extension host (Pi `extensionRunner` getter, agent-session.ts:3142).
    pub fn ext_host(&self) -> &Arc<cyrup_ext::ExtensionHost> {
        &self.services.ext_host
    }

    /// Whether any loaded extension handles `kind` (pi `hasExtensionHandlers(eventType: string):
    /// boolean { return this._extensionRunner.hasHandlers(eventType); }`,
    /// `core/agent-session.ts:3334` @v0.83.0). The `:3135` this used to cite is inside an unrelated
    /// usage-totals accumulation loop.
    pub fn has_extension_handlers(&self, kind: cyrup_ext::EventKind) -> bool {
        !self.services.ext_host.dispatcher().no_subscribers(kind)
    }

    /// Load a live wasm extension COMPONENT into this session's host, injecting the session's
    /// [`crate::host_services::LiveHostServices`] as the capability backend (arch-08 §5.6; Pi
    /// `agent-session-services.ts` extension load). This is THE injection seam that retires the
    /// cyrup-ext §08 ledger row: the same `host_services` that drives live model/session/control
    /// state is what the guest's `models`/`session`/`control` imports reach. Behind the `wasm-host`
    /// feature (ON by default — the host is built with the Wasmtime engine). A guest that registers a
    /// slash command via this seam executes through the real run path end-to-end (proven by
    /// `tests/wasm_slash_command.rs`: `prompt("/greet …")` → `_tryExecuteExtensionCommand` → the
    /// guest's `execute-command` export).
    /// EXT-059 — `caps` is REQUIRED, not implied. This used to call
    /// [`cyrup_ext::ExtensionHost::load_wasm`], which is `load_wasm_with_caps(…,
    /// &Capabilities::host_granted())`: a byte-level load with a TOTAL grant, chosen silently by a
    /// function whose name says nothing about capabilities. `ExtensionHost::load_wasm_with_caps`
    /// existed the whole time and had no session-level caller, so no embedder could restrict a
    /// component it loaded through the session without dropping to the raw host.
    ///
    /// `Capabilities::host_granted()` is still the right answer for an EMBEDDER-supplied component
    /// — pi has no capability model at all, so an embedder's own extension is unconditionally
    /// total (`ExtensionHost::load_wasm`'s doc) — it just has to be SAID.
    #[cfg(feature = "wasm-host")]
    pub async fn load_wasm_extension(
        &self,
        id: cyrup_core::ExtensionId,
        bytes: &[u8],
        caps: &cyrup_ext::Capabilities,
    ) -> Result<Arc<cyrup_ext::host::LiveExtension>, SessionServiceError> {
        let services: Arc<dyn cyrup_ext::host::HostServices> = self.services.host_services.clone();
        Ok(self.services.ext_host.load_wasm_with_caps(id, bytes, services, caps).await?)
    }
}

// ============================================================================ retry subsystem ====
// Pi `agent-session.ts:778,561,2484-2572`. The agent layer drives provider-level retry
// (`max_retries`/`max_retry_delay_ms`); this is the SESSION-level retry-after-agent-end policy:
// when the final assistant turn carries a transient (retryable) error, the facade waits an
// exponential backoff and continues the agent, up to `retry.maxRetries`.
impl AgentSession {
    /// Current retry attempt (0 when not retrying; Pi `retryAttempt` getter, agent-session.ts:778).
    pub fn retry_attempt(&self) -> u32 {
        *Self::lock(&self.retry_attempt)
    }

    /// Whether a retry backoff is in flight (Pi `isRetrying` getter, agent-session.ts:2553).
    pub fn is_retrying(&self) -> bool {
        Self::lock(&self.retry_cancel).is_some()
    }

    /// Whether auto-retry is enabled (runtime override, else the settings default; Pi
    /// `autoRetryEnabled`, agent-session.ts:2558).
    pub fn auto_retry_enabled(&self) -> bool {
        Self::lock(&self.auto_retry_override).unwrap_or(self.retry_enabled_default)
    }

    /// The retry policy handed to every summarization call (compaction, turn-prefix, branch).
    ///
    /// Pi passes `this.settingsManager.getRetrySettings()` — the RESOLVED SETTINGS, not the
    /// interactive auto-retry toggle (`agent-session.ts:1858,2132,2997`), so this deliberately
    /// reads the settings defaults rather than [`Self::auto_retry_enabled`]: pausing the visible
    /// turn-level auto-retry must not silently make a transient socket close abort a whole
    /// compaction.
    pub fn summarization_retry(&self) -> RetryPolicy {
        RetryPolicy::new(
            self.retry_enabled_default,
            self.retry_max_retries,
            self.retry_base_delay_ms,
        )
    }

    /// Toggle auto-retry (Pi `setAutoRetryEnabled`, agent-session.ts:2565). Facade-side override of
    /// the settings `retry.enabled` value (settings persistence lives one layer down).
    pub fn set_auto_retry_enabled(&self, enabled: bool) {
        *Self::lock(&self.auto_retry_override) = Some(enabled);
    }

    /// Cancel an in-flight retry backoff (Pi `abortRetry`, agent-session.ts:2548).
    pub fn abort_retry(&self) {
        if let Some(c) = Self::lock(&self.retry_cancel).as_ref() {
            c.cancel();
        }
    }

    /// Whether an assistant error is retryable (Pi `_isRetryableError`, agent-session.ts:2484).
    /// Context-overflow is handled by compaction, never retry.
    pub fn is_retryable_error(&self, message: &AssistantMessage) -> bool {
        // Pi `if (isContextOverflow(message, this.model?.contextWindow ?? 0)) return false;`
        // (agent-session.ts:2637).
        let window = { Some(Self::lock(&self.compaction_model).as_ref().map_or(0, |m| m.context_window)) };
        if is_context_overflow(message, window) {
            return false;
        }
        is_retryable_assistant_error(message)
    }

    /// Whether the run that just ended will retry (Pi `_willRetryAfterAgentEnd`, agent-session.ts:561).
    /// True iff auto-retry is enabled, the budget is not exhausted, and the last assistant message is
    /// a retryable error.
    pub fn will_retry_after_agent_end(&self, messages: &[AgentMessage]) -> bool {
        if !self.auto_retry_enabled() || self.retry_attempt() >= self.retry_max_retries {
            return false;
        }
        messages
            .iter()
            .rev()
            .find_map(|m| match m {
                AgentMessage::Assistant(a) => Some(self.is_retryable_error(a)),
                _ => None,
            })
            .unwrap_or(false)
    }

    /// Prepare a retryable error for continuation with exponential backoff (Pi `_prepareRetry`,
    /// agent-session.ts:2495-2543). Returns `true` when the caller should continue the agent after
    /// the (abortable) backoff, `false` when retry is disabled, the budget is exhausted, or the wait
    /// was cancelled. Drops the trailing error message from the agent transcript before continuing.
    pub async fn prepare_retry(&self, message: &AssistantMessage) -> bool {
        if !self.auto_retry_enabled() {
            return false;
        }
        {
            let mut attempt = Self::lock(&self.retry_attempt);
            *attempt += 1;
            if *attempt > self.retry_max_retries {
                *attempt -= 1;
                return false;
            }
        }
        let attempt = self.retry_attempt();
        let delay_ms = self
            .retry_base_delay_ms
            .saturating_mul(2u64.saturating_pow(attempt.saturating_sub(1)));
        self.fanout_emit(AgentSessionEvent::AutoRetryStart {
            attempt,
            max_attempts: self.retry_max_retries,
            delay_ms,
            error_message: message.error_message.clone().unwrap_or_else(|| "Unknown error".into()),
        })
        .await;
        // Drop the trailing error message from the agent transcript (kept in session for history).
        self.drop_trailing_assistant().await;
        // Abortable exponential backoff.
        let cancel = self.session_cancel.child_token();
        *Self::lock(&self.retry_cancel) = Some(cancel.clone());
        let slept = cancel
            .run_until_cancelled(tokio::time::sleep(std::time::Duration::from_millis(delay_ms)))
            .await
            .is_some();
        *Self::lock(&self.retry_cancel) = None;
        if !slept {
            let attempt = std::mem::replace(&mut *Self::lock(&self.retry_attempt), 0);
            self.fanout_emit(AgentSessionEvent::AutoRetryEnd {
                success: false,
                attempt,
                final_error: Some("Retry cancelled".into()),
            })
            .await;
            return false;
        }
        true
    }

    /// Drop the trailing assistant message from the agent transcript (used by retry/overflow paths).
    async fn drop_trailing_assistant(&self) {
        let mut msgs = self.agent.snapshot().await.messages;
        if matches!(msgs.last(), Some(AgentMessage::Assistant(_))) {
            msgs.pop();
            self.agent.set_messages(msgs).await;
        }
    }
}

// ====================================================================== auto-compaction subsystem ====
// Pi `agent-session.ts:831,1811-1905,2078-2086`. The pre-send + post-run compaction trigger that
// keeps a long session inside its context window. Manual `compact` already exists; this adds the
// threshold/overflow auto-trigger + the enable toggle + `is_compacting`.
impl AgentSession {
    /// Whether any compaction (manual / auto / branch-summary) is running (Pi `isCompacting`,
    /// agent-session.ts:831).
    pub fn is_compacting(&self) -> bool {
        Self::lock(&self.compaction_cancel).is_some()
            || Self::lock(&self.auto_compaction_cancel).is_some()
            || Self::lock(&self.branch_summary_cancel).is_some()
    }

    /// Whether auto-compaction is enabled (runtime override, else the settings default; Pi
    /// `autoCompactionEnabled`, agent-session.ts:2086).
    pub fn auto_compaction_enabled(&self) -> bool {
        Self::lock(&self.auto_compaction_override).unwrap_or(self.auto_compaction_enabled_default)
    }

    /// Toggle auto-compaction (Pi `setAutoCompactionEnabled`, agent-session.ts:2078).
    pub fn set_auto_compaction_enabled(&self, enabled: bool) {
        *Self::lock(&self.auto_compaction_override) = Some(enabled);
    }

    /// Check whether the given assistant turn requires compaction and run it (Pi `_checkCompaction`,
    /// agent-session.ts:1808-1898). Returns `true` when a compaction ran. `skip_aborted` skips a
    /// user-cancelled turn (post-run); the pre-send check passes `false` to catch aborted responses.
    pub async fn check_compaction(
        &self,
        assistant: &AssistantMessage,
        skip_aborted: bool,
    ) -> Result<bool, SessionServiceError> {
        if !self.auto_compaction_enabled() {
            return Ok(false);
        }
        if skip_aborted && assistant.stop_reason == cyrup_core::StopReason::Aborted {
            return Ok(false);
        }
        // Pi `_checkCompaction` reads `const contextWindow = this.model?.contextWindow ?? 0;`
        // (agent-session.ts:1960) — a modelless session has window 0, which `shouldCompact` and
        // `isContextOverflow` both treat as "unknown", so nothing triggers.
        let model = { Self::lock(&self.compaction_model).clone() };
        let window = model.as_ref().map_or(0, |m| m.context_window);
        let same_model = {
            let cur = Self::lock(&self.model);
            cur.as_ref().is_some_and(|c| {
                assistant.provider == c.provider && assistant.model.as_str() == c.model.as_str()
            })
        };

        // Stale-compaction-boundary guard (Pi agent-session.ts:1859-1864): skip all checks if this
        // assistant turn predates the latest compaction boundary, so a stale pre-compaction
        // usage/error does not retrigger compaction on the first prompt after a compaction.
        let compaction_ts = self.latest_compaction_ts().await;
        if let Some(boundary_ts) = compaction_ts
            && assistant.timestamp <= boundary_ts
        {
            return Ok(false);
        }

        // Case 1: overflow — a context-overflow error/usage on the SAME model compacts (no retry
        // for a completed answer; the overflow-recovery flag guards an infinite loop).
        if same_model && is_context_overflow(assistant, Some(window)) {
            let will_retry = assistant.stop_reason != cyrup_core::StopReason::Stop;
            if !will_retry {
                return self.run_auto_compaction(CompactionReason::Overflow, false).await;
            }
            if *Self::lock(&self.overflow_recovery_attempted) {
                self.fanout_emit(AgentSessionEvent::CompactionEnd {
                    reason: CompactionReason::Overflow,
                    result: None,
                    aborted: false,
                    will_retry: false,
                    error_message: Some(
                        "Context overflow recovery failed after one compact-and-retry attempt. \
                         Try reducing context or switching to a larger-context model."
                            .to_string(),
                    ),
                })
                .await;
                return Ok(false);
            }
            *Self::lock(&self.overflow_recovery_attempted) = true;
            self.drop_trailing_assistant().await;
            return self.run_auto_compaction(CompactionReason::Overflow, will_retry).await;
        }

        // Case 2: threshold — the context is getting large (Pi agent-session.ts:1900-1927). Prefer the
        // assistant turn's OWN reported usage; only for an error / all-zero-usage message fall back to
        // estimating from the live context, with a post-compaction-usage verification so a kept
        // pre-compaction usage (stale, reflecting the old larger context) cannot falsely trigger.
        let settings = self.effective_compaction_settings();
        let direct_context_tokens = context_tokens_from_usage(&assistant.usage);
        let context_tokens: u32 = if assistant.stop_reason == cyrup_core::StopReason::Error
            || direct_context_tokens == 0
        {
            // SESS-028 — the estimate basis is the RAW `AgentMessage` transcript, not the
            // `convertToLlm`-flattened projection. pi reads `this.agent.state.messages`
            // (`agent-session.ts:2020-2021` @v0.83.0), an `AgentMessage[]` that keeps the
            // `bashExecution`/`branchSummary`/`compactionSummary`/`custom` roles intact.
            //
            // `self.messages()` is the flattened view: it renders summary wrappers into text
            // (over-counting them) and DROPS `excludeFromContext` bash messages that pi's raw
            // context still counts. Both errors are in the estimate that decides whether to
            // compact, so this fallback fired at a different context size than upstream — and
            // `Compactor::should_compact` next door already estimated over the raw basis
            // correctly, so the two disagreed inside one crate.
            // `raw_context_messages()` is `build_context_raw()` — `buildSessionContext(pathEntries)
            // .messages` (`session-manager.ts:389-403`), the same basis `Compactor::should_compact`
            // uses. It stands in for pi's `this.agent.state.messages`, and since SESS-043 that is
            // an equality rather than an approximation on every path that re-seeds the transcript:
            // the three re-seed sites now assign this exact list. It stays the basis (rather than
            // reading the agent back) because a live cyrup session ALSO appends its own
            // `!`-execution results straight onto the transcript as `AgentMessage::Custom`
            // (`record_bash_result`), which the session file — and therefore this projection —
            // holds as the authoritative copy.
            let messages = self.raw_context_messages().await;
            let estimate = cyrup_session::compaction::estimate_context_tokens_raw(&messages);
            let Some(last_usage_index) = estimate.last_usage_index else {
                return Ok(false); // No usage data at all.
            };
            // If the usage source predates the compaction boundary, its tokens are stale.
            if let (
                Some(boundary_ts),
                Some(cyrup_session::agent_message::AgentMessage::Core(Message::Assistant(usage_msg))),
            ) = (compaction_ts, messages.get(last_usage_index))
                && usage_msg.timestamp <= boundary_ts
            {
                return Ok(false);
            }
            estimate.tokens
        } else {
            direct_context_tokens
        };
        // Pi `shouldCompact`: contextTokens > contextWindow − reserveTokens (compaction.ts).
        let threshold = window.saturating_sub(u64::from(settings.reserve_tokens));
        if u64::from(context_tokens) > threshold {
            return self.run_auto_compaction(CompactionReason::Threshold, false).await;
        }
        Ok(false)
    }

    /// The unix-ms timestamp of the latest `compaction` entry on the current branch, or `None`
    /// (Pi `getLatestCompactionEntry(this.sessionManager.getBranch())`, agent-session.ts:1859).
    async fn latest_compaction_ts(&self) -> Option<i64> {
        let guard = self.manager.lock().await;
        guard.branch_path(None).into_iter().rev().find_map(|e| match e {
            cyrup_session::Entry::Known(cyrup_session::KnownEntry::Compaction { base, .. }) => {
                Some(cyrup_session::context::parse_entry_ts(&base.timestamp))
            }
            _ => None,
        })
    }

    /// Run an auto-compaction with its own abort controller + events (Pi `_runAutoCompaction`,
    /// agent-session.ts:1905-2076). Mirrors [`Self::compact`]'s dance but tagged with the auto
    /// `reason` and tracked under `auto_compaction_cancel` so `is_compacting`/`abort_compaction`
    /// observe it.
    async fn run_auto_compaction(
        &self,
        reason: CompactionReason,
        will_retry: bool,
    ) -> Result<bool, SessionServiceError> {
        // Pi's FIRST statement inside the try is `if (!this.model) { return false; }`
        // (agent-session.ts:2052-2054) — before `_emit({type:"compaction_start"})` (`:2072`) and
        // before `started = true`, so a modelless session emits NEITHER `compaction_start` nor
        // `compaction_end`; it just declines. The check therefore sits ahead of the emit here too.
        // (Unreachable in practice: `check_compaction`'s window is 0 with no model, so neither the
        // overflow nor the threshold arm fires — but pi guards it and so do we.)
        let Some(model) = ({ Self::lock(&self.compaction_model).clone() }) else {
            return Ok(false);
        };
        let cancel = self.session_cancel.child_token();
        // Same guard as the manual path — an auto compaction runs on the spawned `drive_run` task,
        // but `check_compaction` is also awaited from `prepare` on the CALLER's future
        // (`:1199`), which a racing caller can drop.
        let mut cancel_slot =
            CompactionCancelGuard::install(&self.auto_compaction_cancel, cancel.clone());
        self.fanout_emit(AgentSessionEvent::CompactionStart { reason }).await;

        // Pi: `this._summarizationRetryCallbacks({ source: "compaction", reason })` — the LIVE
        // threshold/overflow reason, not a literal (agent-session.ts:2133).
        let (retry_observer, retry_rx) = crate::compact::summarization_retry_channel(
            SummarizationRetrySource::Compaction { reason },
        );
        let retry_pump = self.spawn_event_pump(retry_rx);
        let summarizer =
            DynSummarizer::new(self.provider.current(), model.clone(), self.summarization_retry())
                .with_observer(retry_observer);
        // Pi threads the session thinking level into every compaction summarization call
        // (`agent-session.ts:1855,2129`); `summarization_reasoning` applies the `model.reasoning`
        // gate before it reaches the request.
        let compactor = Compactor::new(summarizer, NoHooks).with_thinking(self.thinking_level().await);
        let settings = self.effective_compaction_settings();

        // Compute the REAL preparation BEFORE the extension hook (L4 gap #5) — the ONLY preparation.
        let (prep, branch_entries) = {
            let guard = self.manager.lock().await;
            match compactor.prepare(&guard, &settings) {
                Some(x) => x,
                None => {
                    drop(guard);
                    cancel_slot.clear();
                    self.fanout_emit(AgentSessionEvent::CompactionEnd {
                        reason,
                        result: None,
                        aborted: false,
                        will_retry: false,
                        error_message: None,
                    })
                    .await;
                    return Ok(false);
                }
            }
        };

        // session_before_compact ext hook: veto OR compaction override, against the real preparation
        // (agent-session.ts:1980-1990).
        let external_override = match self
            .emit_before_compact(&prep, &branch_entries, None, reason, will_retry, &cancel)
            .await
        {
            BeforeCompactOutcome::Cancel => {
                cancel_slot.clear();
                // Pi agent-session.ts:1984-1990: a cancelling handler emits aborted:true, willRetry:false.
                self.fanout_emit(AgentSessionEvent::CompactionEnd {
                    reason,
                    result: None,
                    aborted: true,
                    will_retry: false,
                    error_message: None,
                })
                .await;
                return Ok(false);
            }
            BeforeCompactOutcome::Proceed(ov) => ov,
        };

        let mut guard = self.manager.lock().await;
        let result = compactor
            .run_compaction_prepared(
                &mut guard,
                &model,
                &settings,
                reason,
                None,
                will_retry,
                &prep,
                branch_entries,
                external_override,
                cancel,
            )
            .await;
        // Pi agent-session.ts:2045: estimate the rebuilt context for the result payload. Hoisted
        // out of the `Ok(Some(_))` arm (as `compact` already does) so the manager guard is released
        // on ONE path, before the retry queue is flushed.
        // SESS-043 — the RAW projection, which is both the estimate basis below AND the transcript
        // pi re-seeds from (`this.agent.state.messages = sessionContext.messages`). It used to be
        // `build_context()`, the `convertToLlm`-flattened twin.
        let compacted_raw = guard.build_context_raw();
        // Pi measures `estimatedTokensAfter` over the RAW `AgentMessage` context, NOT the
        // `convertToLlm`-flattened one: `estimateMessagesTokens(sessionContext.messages)`
        // (agent-session.ts:1876 manual / :2157 auto) sums `estimateTokens` (compaction.ts:266-300)
        // over `buildSessionContext().messages`, and that list is
        // `buildContextEntries().flatMap(sessionEntryToContextMessages)` with every role intact
        // (session-manager.ts:461-469 composed with :383-408). So a retained `compactionSummary`
        // costs `summary.length/4` with NO wrapper prose, and an `excludeFromContext` (`!!`) bash
        // execution still costs `(command.length + output.length)/4`.
        //
        // Measuring `compacted_ctx` instead billed the ~107-char COMPACTION_SUMMARY wrapper that
        // `push_as_message` adds (cyrup-session/src/context.rs:16-18) — ~27 tokens, and a compacted
        // context ALWAYS leads with one — while silently dropping every `excludeFromContext` bash
        // entry, which `AgentMessage::push_llm` removes at the LLM boundary. It also disagreed with
        // `tokens_before` on the SAME `compaction_end` event, which `prepare_compaction` already
        // computes over the raw projection (cyrup-session/src/compaction/prepare.rs).
        let estimated_tokens_after: u64 = u64::from(
            compacted_raw
                .iter()
                .map(cyrup_session::compaction::tokens::estimate_agent_message)
                .fold(0u32, u32::saturating_add),
        );
        drop(guard);
        // Close the retry queue (the compactor owns the emitter) and flush it — with the manager
        // guard already released — so every `summarization_retry_*` lands BEFORE `compaction_end`.
        drop(compactor);
        let _ = retry_pump.await;
        match result {
            Ok(Some(entry)) => {
                cancel_slot.clear();
                // SEAM-112 — the re-seed is on the SUCCESS PATH ONLY. pi runs
                // `appendCompaction(...)` → `this.agent.state.messages = sessionContext.messages;`
                // at `agent-session.ts:2275-2280`, AFTER the `signal.aborted` early-return at
                // `:2260-2275`, so a cancelled / declined / failed auto-compaction leaves
                // `agent.state.messages` exactly as the run found it.
                //
                // cyrup ran it unconditionally, before `match result`, which mattered most on the
                // path that reaches here: `check_compaction` (`:4868`) calls `drop_trailing_assistant`
                // (`:4775-4781`) to strip the overflow response from the agent transcript BEFORE
                // compacting, but that response was already persisted on `message_end`, so an
                // aborted or erroring compaction re-seeded it straight back out of the session
                // file — the exact state `Agent::continue_run` rejects with `ContinueFromAssistant`.
                //
                // pi `agent-session.ts:2280`: re-seed the AGENT's in-memory transcript from the
                // compacted context. `appendCompaction` only writes the JSONL entry — this
                // assignment is what actually shrinks the next request.
                //
                // Without it auto-compaction reported success while the very next turn still
                // shipped the ENTIRE pre-compaction history to the provider: zero token reduction,
                // full cost. Overflow recovery was worse than useless — `check_compaction` set
                // `overflow_recovery_attempted`, `continue_run()` resent the unchanged context, it
                // overflowed again, and the one-shot latch reported "Try reducing context or
                // switching to a larger-context model", blaming the model for a compaction that had
                // never taken effect.
                //
                // SESS-043 — seeded from the RAW projection. Folding `build_context().messages`
                // through `core_message_to_agent` produced a transcript of a different LENGTH and
                // different roles from pi's: `convertToLlm` DROPS every `excludeFromContext` (`!!`)
                // bash message and rewrites each summary into wrapper prose, so pi's
                // `messages.slice(0, -1)` arithmetic (`agent-session.ts:2008`, `:2188`, `:2703`)
                // and every agent-state token estimate ran over a different list.
                let compacted_messages: Vec<AgentMessage> =
                    compacted_raw.iter().map(raw_message_to_agent).collect();
                self.agent.set_messages(compacted_messages).await;
                let cr = crate::state::CompactionResult {
                    summary: entry.summary.clone(),
                    first_kept_entry_id: entry.first_kept_entry_id.to_string(),
                    tokens_before: entry.tokens_before,
                    estimated_tokens_after: Some(estimated_tokens_after),
                    // SEAM-034: pi's `usage?` (compaction.ts:93) — the token spend of the
                    // summarization call(s), already recorded on the compaction entry.
                    usage: entry.usage.clone(),
                    details: entry.details.clone(),
                };
                let notify_cancel = self.session_cancel.child_token();
                self.services
                    .ext_host
                    .dispatcher()
                    .dispatch_notify(
                        &HostEvent::SessionCompact {
                            compaction_entry: serde_json::to_value(&entry)
                                .unwrap_or(serde_json::Value::Null),
                            from_extension: entry.from_hook,
                            reason: compaction_reason_str(reason).to_string(),
                            will_retry,
                        },
                        &notify_cancel,
                    )
                    .await;
                // Pi agent-session.ts:2069: result present, aborted:false, carries the run's willRetry.
                self.fanout_emit(AgentSessionEvent::CompactionEnd {
                    reason,
                    result: Some(cr),
                    aborted: false,
                    will_retry,
                    error_message: None,
                })
                .await;
                // SEAM-112 — pi `agent-session.ts:2307-2317`, positioned exactly here (after the
                // `compaction_end` emit, before the `return true`), with upstream's own reasoning:
                // "The overflow response was persisted on message_end before _checkCompaction()
                // removed it from agent state. Rebuilding state from the new compaction can restore
                // that kept entry, leaving an assistant as the final message. agent.continue()
                // rejects that state, so remove the retriable error or truncated-length response
                // again before continuing the interrupted turn."
                //
                // Concretely on cyrup's side: `check_compaction` (`:4868`) calls
                // `drop_trailing_assistant`, then this run's re-seed above pulls the SAME response
                // back out of the session file (it was written on `message_end`). Without this
                // re-drop `handle_post_agent_run` (`:867-869`) returns `true`,
                // `Agent::continue_run` (cyrup-agent/src/agent.rs:1985-2026) sees a trailing
                // assistant with both queues empty and returns `ContinueFromAssistant`, and
                // `drive_run` (`:803`) breaks — overflow recovery compacts and never retries.
                //
                // The predicate is pi's exact one — `stopReason === "error" || === "length"` — and
                // is deliberately NARROWER than [`Self::drop_trailing_assistant`]'s "any trailing
                // assistant", which would also swallow a legitimately-completed `Stop`/`ToolUse`
                // turn that the compaction happened to leave last. Do not reuse that helper here.
                if will_retry {
                    let mut msgs = self.agent.snapshot().await.messages;
                    let retriable_tail = matches!(
                        msgs.last(),
                        Some(AgentMessage::Assistant(a))
                            if matches!(
                                a.stop_reason,
                                cyrup_core::StopReason::Error | cyrup_core::StopReason::Length
                            )
                    );
                    if retriable_tail {
                        msgs.pop();
                        self.agent.set_messages(msgs).await;
                    }
                }
                Ok(true)
            }
            Ok(None) => {
                cancel_slot.clear();
                self.fanout_emit(AgentSessionEvent::CompactionEnd {
                    reason,
                    result: None,
                    aborted: false,
                    will_retry: false,
                    error_message: None,
                })
                .await;
                Ok(false)
            }
            Err(e) => {
                cancel_slot.clear();
                let aborted = matches!(e, cyrup_session::compaction::CompactionError::Aborted);
                // Pi agent-session.ts:2083-2097: on a non-abort failure, emit the reason-tagged
                // recovery/auto-compaction error message; an abort emits no errorMessage.
                let error_message = if aborted {
                    None
                } else if reason == CompactionReason::Overflow {
                    Some(format!("Context overflow recovery failed: {e}"))
                } else {
                    Some(format!("Auto-compaction failed: {e}"))
                };
                self.fanout_emit(AgentSessionEvent::CompactionEnd {
                    reason,
                    result: None,
                    aborted,
                    will_retry: false,
                    error_message,
                })
                .await;
                if aborted {
                    Ok(false)
                } else {
                    Err(e.into())
                }
            }
        }
    }

    /// The effective compaction settings with the live `enabled` toggle applied.
    fn effective_compaction_settings(&self) -> CompactionSettings {
        CompactionSettings {
            enabled: self.auto_compaction_enabled(),
            reserve_tokens: self.compaction_settings.reserve_tokens,
            keep_recent_tokens: self.compaction_settings.keep_recent_tokens,
        }
    }
}

/// Mark this session's extension instances stale when the session itself goes away — the backstop
/// for every path that never reaches the tail of [`AgentSession::dispose_with`].
///
/// **[CYRUP-DELTA]** vs pi `AgentSession.dispose()` → `this._extensionRunner.invalidate(...)`
/// (`core/agent-session.ts:850-852` @v0.84.2). Upstream has NO destructor half at all, and cannot:
/// JavaScript gives a class no deterministic finalizer, so `invalidate` is reachable only from the
/// one explicit `dispose()` call. Rust does give one, and the ported call site sits behind three
/// `.await`s a dropped future never resumes past (see below), so the upstream mechanism alone is
/// strictly weaker HERE than it is there. This adds the half the language makes available; it
/// changes no observable behaviour on any path that does reach `dispose_with`, because
/// `GuestState::invalidate` is first-reason-wins (pi `extensions/loader.ts:207`).
///
/// # Why a `Drop` as well as the ordered call
///
/// `dispose_with` invalidates at pi's exact position: after `session_shutdown` has been fanned out
/// and every handler has finished, after the host's `before_session_invalidate` hook, and before
/// the session token is cancelled (pi `teardownCurrent` → `this.session.dispose()` →
/// `this._extensionRunner.invalidate(<stale text>)`, `core/agent-session-runtime.ts:176-177` +
/// `core/agent-session.ts:850-852` @v0.84.2). That ORDER is the contract, so this `Drop` is not a
/// replacement for it — it is the same relationship [`CompactionCancelGuard`] has with the
/// hand-written clears, and for the identical reason.
///
/// Three of `dispose_with`'s statements are `.await`s (`abort_and_settle`, `fanout_emit`,
/// `dispatch_notify`), and everything after them — the hook, the invalidation, the cancel — runs
/// only if the future is polled to completion. A Rust future can be dropped at any `.await`, and
/// this crate already documents callers that do exactly that to a session future: the
/// `tokio::time::timeout` around the `cyrup-sdk` handle and `run_rpc`'s `select!` dropping the
/// driver when the write pump reports a broken pipe (`cyrup-modes/src/rpc.rs:668-676`) — see
/// [`CompactionCancelGuard`]'s doc, which names both. `dispatch_notify` is the worst of the three
/// to be cut at: it awaits extension handlers, including guest calls across the wasm boundary, so
/// it is the statement most likely to be slow enough to be raced or to unwind on a native
/// extension's panic. In any of those cases the outgoing instances stayed un-stale forever and
/// pi's `assertActive` refusal never fired for a call still in flight on one of them.
///
/// A session that is DROPPED WITHOUT `dispose` at all is the same hole reached from the other side.
/// `cyrup_sdk::Session::close` documents that dropping a `Session` without calling it is silent,
/// and correctly explains that `Drop` cannot do the async half — `session_shutdown` is dispatched
/// and awaited. Invalidation is the SYNC half, and is therefore precisely the part a `Drop` can
/// still honour; this closes that half without disturbing the documented contract for the rest.
///
/// # Why this is safe to do unconditionally
///
/// * **Idempotent.** `GuestState::invalidate` is `if state.staleMessage) return;` — the FIRST
///   reason wins (pi `extensions/loader.ts:207`), so running after a normal `dispose_with` is a
///   no-op and cannot overwrite a more specific reason with the default text.
/// * **Correctly scoped.** It cannot stale the REPLACEMENT session's extensions, because a session
///   does not share its host: every replacement path goes through `SessionFactory::build*` →
///   `SessionBuilder::build`, which constructs a fresh `ExtensionHost` (`builder.rs`, `let
///   ext_host = Arc::new(host)`) and loads its own guest set into it. That mirrors upstream, where
///   each `createRuntime` builds a new `DefaultResourceLoader` and `await resourceLoader.reload()`
///   hands the replacement a fresh `createExtensionRuntime()` whose `staleMessage` is unset
///   (`core/agent-session-services.ts:154`, `core/resource-loader.ts:283`,
///   `core/extensions/loader.ts:174-178`). This is why pi can invalidate on every teardown without
///   disabling the session that follows, and why cyrup can too.
/// * **A no-op without live guests**, including the whole `--no-default-features` arm, where
///   `ExtensionHost::invalidate_live` is a `#[cfg]`'d empty body.
impl Drop for AgentSession {
    fn drop(&mut self) {
        self.services.ext_host.invalidate_live(None);
    }
}

/// Clears the compaction cancel-token slot it installed into, whatever happens to the future that
/// installed it — the same role [`BashCancelGuard`] plays for `_bashAbortControllers`, and for the
/// same reason.
///
/// pi's `compact` / `_runAutoCompaction` clear `this._compactionAbortController` /
/// `this._autoCompactionAbortController` on every settling path, and a JS `async fn` ALWAYS settles.
/// A Rust future does not: `AgentSession::compact` is a public API whose body is one 10-20 s
/// provider call surrounded by `.await`s, and any caller that races it — `tokio::time::timeout`
/// around the `cyrup-sdk` handle (`cyrup-sdk/src/handle.rs:285`), or `run_rpc`'s `select!` dropping
/// the driver when the write pump reports a broken pipe (`cyrup-modes/src/rpc.rs:668-676`) — drops
/// it mid-flight. The hand-written clears at each `return` cannot run then, so the slot keeps a
/// token nobody is awaiting and **`is_compacting()` answers true forever**. That is not a leaked
/// allocation: the TUI's Submit arm consults `session.is_compacting()` before anything else
/// (`cyrup-tui/src/app.rs`, the `AppAction::Submit if session.is_compacting()` arm), so every
/// subsequent prompt the user types is diverted into the compaction queue and drained by a
/// `compaction_end` that can never arrive — the session accepts input and silently sends none of
/// it, with no way out short of a restart.
///
/// [`Self::clear`] is what the settling paths call, so the slot is still emptied at pi's exact
/// point in the sequence — **before** `compaction_end` is fanned out, which is load-bearing: the
/// TUI drains its compaction queue in response to that event and would re-queue the whole batch if
/// `is_compacting()` were still true when it did. Clearing only in `Drop` would move the clear
/// after the emit and reintroduce that race, so this guard is a backstop for the dropped-future
/// path, not a replacement for the ordered clear. Once cleared it disarms, so it can never wipe a
/// token a later compaction installed.
struct CompactionCancelGuard<'a> {
    slot: &'a Mutex<Option<CancelToken>>,
    armed: bool,
}

impl<'a> CompactionCancelGuard<'a> {
    /// Install `cancel` into `slot` and arm the guard.
    fn install(slot: &'a Mutex<Option<CancelToken>>, cancel: CancelToken) -> Self {
        *AgentSession::lock(slot) = Some(cancel);
        Self { slot, armed: true }
    }

    /// Clear the slot now (pi's ordered clear), and disarm.
    fn clear(&mut self) {
        if self.armed {
            *AgentSession::lock(self.slot) = None;
            self.armed = false;
        }
    }
}

impl Drop for CompactionCancelGuard<'_> {
    fn drop(&mut self) {
        self.clear();
    }
}

// =========================================================================== immediate-bash seam ====
// Pi `agent-session.ts:2582-2684`. The out-of-loop bash RPC path.

/// Removes this call's entry from [`AgentSession::bash_cancels`] on drop — cyrup's stand-in for pi's
/// `finally { this._bashAbortControllers.delete(abortController); }` (agent-session.ts:2794-2796
/// @v0.83.0).
///
/// A guard rather than a removal written at each exit, because pi's `finally` covers exits Rust
/// reaches differently: the early `?` on `Custom shell path not found`, the `?` on the backend
/// failure pi lets propagate straight *through* its `finally` uncaught, and — with no JS counterpart
/// at all — an `execute_bash` future DROPPED at an `.await` by a caller that went away, which a JS
/// `async` function cannot do because it always settles. Without the guard that last case leaks a
/// dead handle into the set and makes `is_bash_running()` answer true forever.
struct BashCancelGuard<'a> {
    session: &'a AgentSession,
    id: u64,
}

impl Drop for BashCancelGuard<'_> {
    fn drop(&mut self) {
        AgentSession::lock(&self.session.bash_cancels).retain(|(id, _)| *id != self.id);
    }
}

impl AgentSession {
    /// Execute a bash command out-of-band and record its result (Pi `executeBash`,
    /// agent-session.ts:2588). Streams combined output to `on_chunk`; the result is recorded into the
    /// transcript (or deferred while a run streams).
    ///
    /// Fires NO extension event of its own — Pi's `executeBash` (agent-session.ts:2582-2684) has zero
    /// `emitUserBash` emission even at HEAD; in Pi the emission lives at the two front-end CALLERS,
    /// which each emit `user_bash` for themselves and only then call into this executor:
    /// `interactive-mode.ts:6010-6060`'s `handleBashCommand` (the `!`/`!!`-prefix handler) and
    /// `rpc-mode.ts:558-579`'s `case "bash"` (given its emission by pi `5d548ae9`, 2026-07-28,
    /// "fix: rpc bash no longer bypass user_bash", #7214). cyrup shares one wrapper across both:
    /// [`Self::execute_bash_with_user_event`] — that is what `crates/cyrup-modes/src/rpc.rs`'s
    /// `SessionCommand::Bash` arm calls. Call this bare method only when the caller is NOT a
    /// user-initiated bash front-end (it is also the fall-through of that wrapper).
    ///
    /// A genuine backend failure is returned as `Err` and NEVER recorded into history — Pi's
    /// `executeBash` only calls `recordBashResult` on the success path inside its `try` block
    /// (`agent-session.ts:2628-2643`); a rejection from `executeBashWithOperations` propagates
    /// straight through the `finally` (which only deletes this call's handle from
    /// `_bashAbortControllers`, agent-session.ts:2794-2796) uncaught, all the way to the RPC
    /// dispatcher's `catch` (`rpc-mode.ts:756-772`).
    pub async fn execute_bash(
        &self,
        command: &str,
        options: BashOptions,
        on_chunk: crate::bash::BashChunkSink,
    ) -> Result<BashResult, SessionServiceError> {
        // Pi `const abortController = new AbortController(); this._bashAbortControllers.add(...)`
        // (agent-session.ts:2770-2771 @v0.83.0) — one handle PER CALL, added to the set, removed in
        // the `finally` (here: `_bash_guard`'s drop). Concurrent calls each keep their own.
        let cancel = self.session_cancel.child_token();
        let bash_cancel_id = self.next_bash_cancel_id();
        Self::lock(&self.bash_cancels).push((bash_cancel_id, cancel.clone()));
        let _bash_guard = BashCancelGuard { session: self, id: bash_cancel_id };
        let cwd = self.services.cwd.clone();
        // Managed bin dir (Pi `getBinDir()`, `config.ts:549`: `join(getAgentDir(), "bin")`), matching
        // `cyrup_config::ConfigDirs::bin_dir()`'s layout — see `run_bash`'s doc comment.
        let bin_dir = self.services.agent_dir.join("bin");
        // Apply the `shellCommandPrefix` setting (Pi `executeBash`, agent-session.ts:2624-2627):
        // prepend it before the command, joined by a newline — the same prefix application the
        // agent-loop `bash` tool already performs (`cyrup-tools/src/tools/bash.rs:99-102`). The
        // ORIGINAL `command` (not this resolved one) is still what gets recorded into history below,
        // matching Pi's `recordBashResult(command, result, options)` (agent-session.ts:2628).
        let resolved_command = match &self.shell_command_prefix {
            Some(prefix) => format!("{prefix}\n{command}"),
            None => command.to_string(),
        };
        // Resolve the shell fresh on THIS call, honoring a custom `shellPath` setting (Pi's
        // `createLocalBashOperations({ shellPath })` resolves `getShellConfig(shellPath)` inside
        // `exec` on every `executeBash` invocation — bash.ts:69/89 — never baked in once at session
        // build time); a missing custom path surfaces the same `Custom shell path not found` error
        // as the agent-loop `bash` tool (`cyrup-tools/src/tools/bash.rs:108-111`).
        let shell = match self.shell_path.as_deref() {
            Some(p) => match ShellConfig::resolve(Some(p)) {
                Ok(shell) => shell,
                // `_bash_guard` performs pi's `finally` removal on this path too.
                Err(e) => return Err(e.into()),
            },
            None => self.shell.clone(),
        };
        // Pi wraps the caller's `onChunk` and emits `bash_execution_update` for EVERY delta,
        // whether or not a sink was supplied (agent-session.ts:2784-2787):
        //   onChunk: (delta) => { onChunk?.(delta); this._emit({type:"bash_execution_update", …}) }
        // so a front-end that only observes events still renders live output. The sync callback
        // posts to a queue drained by `spawn_event_pump` (see its doc for why).
        let (chunk_tx, chunk_rx) = tokio::sync::mpsc::unbounded_channel();
        let bash_id = options.id.clone();
        let mut caller_sink = on_chunk;
        let sink: crate::bash::BashChunkSink = Some(Box::new(move |delta: &str| {
            if let Some(cb) = caller_sink.as_mut() {
                cb(delta);
            }
            let _ = chunk_tx.send(AgentSessionEvent::BashExecutionUpdate {
                id: bash_id.clone(),
                delta: delta.to_string(),
            });
        }));
        let chunk_pump = self.spawn_event_pump(chunk_rx);
        // Pi `options?.operations ?? createLocalBashOperations({ shellPath })`
        // (agent-session.ts:2782): a caller-supplied backend replaces the local one for THIS call
        // only. `run_bash` takes the `??` whole — `None` is the local branch it always took.
        let outcome = run_bash(
            &self.proc,
            &shell,
            options.operations.as_deref(),
            cwd,
            resolved_command,
            Some(bin_dir.as_path()),
            cancel,
            sink,
        )
        .await;
        // `run_bash` consumed the sink, so its `chunk_tx` is already dropped: awaiting the pump
        // flushes every delta before the caller sees the result.
        let _ = chunk_pump.await;
        // No explicit removal here — `_bash_guard` is pi's `finally` and fires on BOTH the `?` below
        // and the `Ok` return, and only for THIS call's handle. Clearing the whole slot here is what
        // used to make `is_bash_running()` lie while a concurrent command was still running.
        let result = outcome?;
        self.record_bash_result(command, &result, options).await;
        Ok(result)
    }

    /// Execute a **user-initiated** bash command: the entry point every user-facing bash front-end
    /// must call. Fires the `user_bash` extension event FIRST with the live `{command,
    /// excludeFromContext, cwd}` (Pi `UserBashEvent`, `extensions/types.ts:813-821`); a handler that
    /// returns a full `result` override (`UserBashEventResult.result`,
    /// `extensions/types.ts:1078-1083`) short-circuits local execution entirely and its result is
    /// still recorded through [`Self::record_bash_result`]; otherwise this falls through to the bare
    /// [`Self::execute_bash`] for normal execution.
    ///
    /// Pi emits at both front-ends rather than inside `executeBash`: the interactive `!`/`!!`-prefix
    /// handler (`interactive-mode.ts:6010-6060`, `handleBashCommand`) and the JSON-RPC `bash`
    /// command (`rpc-mode.ts:558-579`, `case "bash"` — emission added by pi `5d548ae9`, 2026-07-28,
    /// "fix: rpc bash no longer bypass user_bash", #7214, so an extension observing user bash no
    /// longer misses RPC-issued commands). Both cyrup front-ends therefore share this one wrapper.
    ///
    /// Pi's sibling `operations` remote-exec override — the other half of `UserBashEventResult` —
    /// is threaded through [`BashOptions::operations`] and honored by [`Self::execute_bash`]
    /// (`options?.operations ?? createLocalBashOperations({ shellPath })`,
    /// `agent-session.ts:2782`). **What this wrapper still cannot do is FILL it from the event
    /// result, and that is DRIFT-004 / SEAM-015's last remaining half.** Upstream's
    /// `rpc-mode.ts:576` can write `operations: eventResult?.operations` because a JS handler
    /// returns a live object; cyrup's extension I/O is serde values (ADR-0002), so the payload
    /// [`Self::emit_user_bash_event`] receives can carry the `operations` KEY but never a callable
    /// behind it. Closing it needs the `register-bash-operations` import + keyed
    /// `bash-operations-exec` export round-trip designed in full in `crates/cyrup-ext/src/lib.rs`'s
    /// CYRUP-DELTA register — a `crates/cyrup-ext` + `crates/cyrup-ext-sdk` change with a
    /// `HOST_WORLD` minor bump, not a change here. Once it exists this wrapper sets one field.
    pub async fn execute_bash_with_user_event(
        &self,
        command: &str,
        options: BashOptions,
        on_chunk: crate::bash::BashChunkSink,
    ) -> Result<BashResult, SessionServiceError> {
        if let Some(result) = self.emit_user_bash_event(command, options.exclude_from_context).await {
            self.record_bash_result(command, &result, options).await;
            return Ok(result);
        }
        self.execute_bash(command, options, on_chunk).await
    }

    /// Emit the `user_bash` extension event and, if a handler fully serviced the command (Pi
    /// `UserBashEventResult.result`, `extensions/types.ts:1078-1083`), return its [`BashResult`]
    /// override so the caller skips local execution. Returns `None` when nobody subscribed or no
    /// result override was supplied. Carries the live `command`, the `exclude_from_context` flag
    /// (the interactive `!!` prefix, or the RPC command's `excludeFromContext ?? false`,
    /// `rpc-mode.ts:562`), and the session cwd (Pi `UserBashEvent`, `extensions/types.ts:813-821`).
    ///
    /// Matches Pi's `emitUserBash` (`extensions/runner.ts:955-981`) dispatch semantics: the FIRST
    /// truthy handler result wins and short-circuits the remaining handlers, and a handler that
    /// throws is caught and reported rather than being fatal — `dispatch_block_mutate` returning
    /// `Reduced::Handled` is cyrup's equivalent of the former, and the dispatcher's per-extension
    /// error isolation of the latter.
    async fn emit_user_bash_event(&self, command: &str, exclude_from_context: bool) -> Option<BashResult> {
        if self.services.ext_host.dispatcher().no_subscribers(cyrup_ext::EventKind::UserBash) {
            return None;
        }
        let cancel = self.session_cancel.child_token();
        let event = HostEvent::UserBash {
            command: command.to_string(),
            exclude_from_context,
            cwd: self.services.cwd.display().to_string(),
        };
        let reduced =
            self.services.ext_host.dispatcher().dispatch_block_mutate(event, &cancel).await;
        // A handler that returned a `UserBashEventResult.result` (Pi types.ts:1043-1048) fully
        // serviced the command; deserialize the override `BashResult`. Other outcomes fall through
        // to normal execution.
        if let Reduced::Handled(handled) = reduced {
            return handled
                .0
                .get("result")
                .cloned()
                .and_then(|r| serde_json::from_value::<BashResult>(r).ok());
        }
        None
    }

    /// Record a bash result into the transcript + session (Pi `recordBashResult`,
    /// agent-session.ts:2628). While a run streams, the message is deferred to avoid breaking
    /// tool_use/tool_result ordering and flushed after the turn.
    pub async fn record_bash_result(&self, command: &str, result: &BashResult, options: BashOptions) {
        let payload = bash_message_payload(command, result, options.exclude_from_context);
        let msg = AgentMessage::Custom {
            kind: "bashExecution".to_string(),
            payload: payload.clone(),
            timestamp: Some(now_ms()),
        };
        if self.is_streaming().await {
            Self::lock(&self.pending_bash).push(msg);
            return;
        }
        self.append_bash_message(msg, &payload).await;
    }

    /// Mint the next identity handle for [`Self::bash_cancels`] (pi gets identity for free from the
    /// `AbortController` object it puts in the set).
    fn next_bash_cancel_id(&self) -> u64 {
        self.next_bash_cancel_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Cancel EVERY running bash command (Pi `abortBash`, agent-session.ts:2832-2836 @v0.83.0:
    /// `for (const abortController of [...this._bashAbortControllers]) abortController.abort();`).
    ///
    /// The snapshot is pi's spread copy, and it is load-bearing here for a reason pi does not have:
    /// `CancelToken::cancel` runs registered callbacks synchronously, one of which could re-enter
    /// this session, so the lock is released before any token is fired.
    pub fn abort_bash(&self) {
        let snapshot: Vec<CancelToken> =
            Self::lock(&self.bash_cancels).iter().map(|(_, c)| c.clone()).collect();
        for c in snapshot {
            c.cancel();
        }
    }

    /// Whether ANY bash command is running (Pi `isBashRunning`, agent-session.ts:2839-2841
    /// @v0.83.0: `return this._bashAbortControllers.size > 0;`).
    pub fn is_bash_running(&self) -> bool {
        !Self::lock(&self.bash_cancels).is_empty()
    }

    /// Whether deferred bash messages await flush (Pi `hasPendingBashMessages`, agent-session.ts:2670).
    pub fn has_pending_bash_messages(&self) -> bool {
        !Self::lock(&self.pending_bash).is_empty()
    }

    /// Flush deferred bash messages to the transcript + session (Pi `_flushPendingBashMessages`,
    /// agent-session.ts:2675). Called before a new prompt so ordering is intact.
    pub async fn flush_pending_bash_messages(&self) {
        let pending: Vec<AgentMessage> = std::mem::take(&mut *Self::lock(&self.pending_bash));
        for msg in pending {
            if let AgentMessage::Custom { payload, .. } = &msg {
                let payload = payload.clone();
                self.append_bash_message(msg, &payload).await;
            }
        }
    }

    /// Append a bash message to the agent transcript + persist it durably.
    async fn append_bash_message(&self, msg: AgentMessage, payload: &serde_json::Value) {
        let mut msgs = self.agent.snapshot().await.messages;
        msgs.push(msg);
        self.agent.set_messages(msgs).await;
        let _ = self
            .manager
            .lock()
            .await
            .append_custom_message("bashExecution", payload.clone(), true, None);
    }
}

// =============================================================================== dynamic tools ====
// Pi `agent-session.ts:786-828,2304`. Mid-session tool toggling + system-prompt rebuild.
impl AgentSession {
    /// Names of the currently-active tools (Pi `getActiveToolNames`, agent-session.ts:786).
    pub fn active_tool_names(&self) -> Vec<String> {
        Self::lock(&self.dynamic_tools).active_names()
    }

    /// All enable-able tools with metadata (Pi `getAllTools`, agent-session.ts:794).
    pub fn all_tools(&self) -> Vec<ToolInfo> {
        Self::lock(&self.dynamic_tools).all()
    }

    /// One tool's definition by name (Pi `getToolDefinition`, agent-session.ts:806).
    pub fn tool_definition(&self, name: &str) -> Option<ToolInfo> {
        Self::lock(&self.dynamic_tools).get(name)
    }

    /// Push a rebuilt `(tools, system_prompt)` onto the agent for the next turn (Pi
    /// `setActiveToolsByName` tail, agent-session.ts:850-854). Shared by the host/CLI
    /// [`Self::set_active_tools_by_name`] path and the guest-driven drain in
    /// [`Self::apply_pending_control`] so both reach the live agent identically.
    async fn push_active_tools(&self, tools: Vec<Arc<dyn cyrup_core::Tool>>, prompt: String) {
        self.agent.set_tools(tools).await;
        // The rebuilt prompt is the new BASE, not just this turn's value (Pi
        // `this._baseSystemPrompt = this._rebuildSystemPrompt(validToolNames)`, agent-session.ts:939).
        // Without this write the next run's `before_agent_start` reset in
        // [`Self::assemble_run_messages`] would restore the startup prompt and the model would be
        // described the startup tool set for the rest of the session.
        *Self::lock(&self.base_system_prompt) = prompt.clone();
        // …and what reaches the AGENT is `override ?? base` — pi's very next line,
        // `this.agent.state.systemPrompt = this._systemPromptOverride ?? this._baseSystemPrompt;`
        // (agent-session.ts:940 @v0.83.0). A rebuild triggered mid-run by a tool registration used to
        // overwrite a `before_agent_start` handler's sanitized prompt with the raw rebuilt one
        // (DRIFT-033); resolving through the override slot is what stops it.
        let effective = self.effective_system_prompt();
        self.agent.set_system_prompt(effective).await;
        // EXT-005: keep the guest-visible `ctx.getSystemPrompt()` mirror in step with the agent —
        // a tool-set rebuild rewrites the prompt (Pi `_rebuildSystemPrompt`, agent-session.ts:2304)
        // and a guest reading it back must see the rebuilt one.
        self.services
            .host_services
            .update_prompt_state(Some(prompt), self.services.settings.project_trusted());
    }

    /// Surface tools an extension registered AFTER its `init` to the LIVE agent (EXT-004; Pi
    /// `refreshTools` → `_refreshToolRegistry`, extensions/loader.ts:249-256 →
    /// agent-session.ts:2452-2546).
    ///
    /// `ExtensionHost::refresh_tools` re-materializes a late descriptor into an executable
    /// `Arc<dyn Tool>`, but that alone only changes the extension host's view. The model's tool
    /// array and the system prompt come from [`crate::tools::DynamicToolState`], which the builder
    /// snapshots ONCE — so without this the tool existed and could not be called. Merging here
    /// mirrors Pi's tail exactly: new names are auto-activated (`if (!previousRegistryNames.has(
    /// toolName)) nextActiveToolNames.push(toolName)` … `setActiveToolsByName(...)`,
    /// agent-session.ts:2534-2545) and the rebuilt `(tools, prompt)` is pushed to the agent.
    ///
    /// Cheap and idempotent: a relaxed atomic load short-circuits when nothing was registered.
    pub(crate) async fn refresh_extension_tools(&self) {
        match self.services.ext_host.refresh_tools() {
            Ok(false) => return,
            Ok(true) => {}
            Err(e) => {
                tracing::warn!(error = %e, "extension tool refresh failed; the late tool stays invisible");
                return;
            }
        }
        // `&[]` = "no built-in base": what comes back is exactly the extension-contributed set,
        // which is what merges into the registry (the built-ins are already in it).
        let ext_tools = match self.services.ext_host.active_tools(&[]) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "extension tool refresh failed; the late tool stays invisible");
                return;
            }
        };
        let push = { Self::lock(&self.dynamic_tools).merge_registered(ext_tools) };
        if let Some((tools, prompt)) = push {
            self.push_active_tools(tools, prompt).await;
        }
    }

    /// The TURN-BOUNDARY tool refresh (Pi `_installAgentNextTurnRefresh`, agent-session.ts:519-540).
    /// Returns the tool array the agent should run the NEXT turn of the current run with, for
    /// `PolicyHooks::prepare_next_turn` to hand back as a [`cyrup_agent::TurnUpdate`].
    ///
    /// Pi's version is one line — `tools: this.agent.state.tools.slice()` — because `setActiveTools`
    /// mutates `agent.state.tools` synchronously and the loop re-reads its context every turn.
    /// cyrup's loop snapshots the array at run start, so the live value has to be pushed back in;
    /// the value itself still comes from `agent.state`, which is the single authority every mutation
    /// path already writes to ([`Self::push_active_tools`]).
    ///
    /// The two drains ahead of that read are the EXISTING EXT-004 mechanism, called at a new time
    /// rather than reimplemented — and in the same order as the post-run drain in
    /// [`Self::apply_pending_agent_control`]: the refresh runs first so an explicit `setActiveTools`
    /// still has the last word. Both are cheap no-ops when nothing changed (a relaxed atomic load
    /// and an `Option` take), which is the common case on every turn of every run.
    ///
    /// The rebuilt system prompt IS propagated now, through [`Self::effective_system_prompt`] —
    /// `PolicyHooks::prepare_next_turn` reads it beside this array and returns both, mirroring pi's
    /// `systemPrompt: this._systemPromptOverride ?? this._baseSystemPrompt` (agent-session.ts:534
    /// @v0.83.0). The reason it used not to be is gone: cyrup now keeps pi's two slots, so re-pushing
    /// resolves back to a `before_agent_start` handler's SANITIZED prompt (the permission companion's
    /// `shouldExposeTool` shaping) rather than clobbering it with the raw rebuild (DRIFT-033). Both
    /// drains below still discard their locally rebuilt prompt string for the same reason they always
    /// did — the authority is the base slot [`Self::push_active_tools`] writes, not a drain's
    /// by-product.
    pub(crate) async fn next_turn_tools(&self) -> Vec<Arc<dyn cyrup_core::Tool>> {
        // EXT-004: a tool an extension registered from a LIVE handler during this run.
        self.refresh_extension_tools().await;
        // A guest's `setActiveTools` queued from an event handler / mid-turn tool hook. Array only,
        // prompt discarded — see above, and the identical rule in `assemble_run_messages`.
        if let Some((tools, _rebuilt_prompt)) =
            self.services.host_services.take_pending_active_tools()
        {
            self.agent.set_tools(tools).await;
        }
        self.agent.tools().await
    }

    /// The agent's live model + thinking level, for the per-turn refresh to stamp over whatever the
    /// extension seam returned (AGENT-017). pi reads exactly these two off the AGENT — `model:
    /// this.agent.state.model` (`agent-session.ts:537` @v0.83.0) and `thinkingLevel:
    /// this.agent.state.thinkingLevel` (`:538`) — not the session's mirrors, and stamps them AFTER
    /// the `...previousSnapshot` spread so the session out-votes an extension override.
    pub(crate) async fn next_turn_model_baseline(
        &self,
    ) -> (ModelRef, cyrup_core::ModelThinkingLevel) {
        let snap = self.agent.snapshot().await;
        (snap.model, snap.thinking_level)
    }

    /// Set the active tool set by name, rebuilding the base system prompt and re-pushing both the
    /// tool array and the prompt to the agent for the next turn (Pi `setActiveToolsByName`,
    /// agent-session.ts:812). Unknown names are ignored.
    pub async fn set_active_tools_by_name(&self, names: &[String]) {
        let (tools, prompt) = { Self::lock(&self.dynamic_tools).set_active(names) };
        self.push_active_tools(tools, prompt).await;
    }

    /// Register additional custom tools into the enable-able registry (Pi `customTools`, sdk.ts:71,384).
    ///
    /// Each tool goes through [`cyrup_ext::ExtensionHost::wrap_tool`] first, exactly as the
    /// BUILD-TIME custom-tool path does (`builder.rs`'s
    /// `registry_tools.extend(cfg.custom_tools.iter().map(|t| ext_host.wrap_tool(t.clone())))`),
    /// which is the parity this method claims. pi wraps its SDK custom tools together with
    /// everything else in one `wrapRegisteredTools` pass (`core/agent-session.ts:2513`, over the
    /// `allCustomTools` list built at `:2472-2478`), so there is no upstream shape in which an
    /// SDK-supplied tool runs unwrapped. Unwrapped, the tool executed with NO extension
    /// `tool_call`/`tool_result` hooks around it — the permission gate and every observer extension
    /// were blind to it — and it never derived `addedToolNames`.
    pub fn register_custom_tools(&self, tools: Vec<Arc<dyn cyrup_core::Tool>>) {
        let wrapped = tools.into_iter().map(|t| self.services.ext_host.wrap_tool(t)).collect();
        Self::lock(&self.dynamic_tools).register_custom(wrapped);
    }
}

/// The live [`crate::host_services::SessionActivity`] backing a guest's `ctx.isIdle()` /
/// `ctx.hasPendingMessages()` / `ctx.abort()` (EXT-005). Weak so the capability backend — which the
/// session itself owns — can never keep the session alive.
struct SessionActivityHandle(std::sync::Weak<AgentSession>);

impl crate::host_services::SessionActivity for SessionActivityHandle {
    fn is_idle(&self) -> bool {
        // A dropped session is not running anything: idle is the honest answer.
        self.0.upgrade().is_none_or(|s| s.is_idle())
    }

    fn pending_message_count(&self) -> usize {
        self.0.upgrade().map_or(0, |s| s.pending_message_count())
    }

    fn abort(&self) {
        if let Some(s) = self.0.upgrade() {
            s.abort();
        }
    }
}

/// The live [`crate::host_services::SessionCatalog`] backing a guest's `getCommands()` and the
/// extension-tool provenance half of its `getAllTools()` (EXT-037 / EXT-038). Weak for the same
/// reason [`SessionActivityHandle`] is.
struct SessionCatalogHandle(std::sync::Weak<AgentSession>);

impl crate::host_services::SessionCatalog for SessionCatalogHandle {
    fn commands(&self) -> Vec<serde_json::Value> {
        // Pi `getCommands()` = `[...extensionCommands, ...templates, ...skills]`
        // (agent-session.ts:2332-2354 @v0.83.0), which is exactly what `slash_command_catalog`
        // builds. A dropped session has no commands, which is the honest empty answer.
        self.0.upgrade().map(|s| s.slash_command_catalog()).unwrap_or_default()
    }

    fn extension_tool_source_info(&self) -> std::collections::HashMap<String, serde_json::Value> {
        // The `sourceInfo` half of pi's `_toolDefinitions` entry, for the extension-contributed
        // tools only (agent-session.ts:2482-2487: `definitionRegistry.set(name, {definition,
        // sourceInfo: tool.sourceInfo})`). `ExtensionRegistry::tool_info` already emits one row per
        // registered extension/guest tool carrying the same `SourceInfo` shape, so it is the map's
        // source; every other name in the dynamic registry is a built-in (or an SDK custom tool) and
        // gets the synthetic form on the host-services side.
        let Some(s) = self.0.upgrade() else { return std::collections::HashMap::new() };
        let Ok(rows) = s.services.ext_host.registry().tool_info() else {
            return std::collections::HashMap::new();
        };
        rows.into_iter()
            .filter_map(|r| {
                let name = r.get("name")?.as_str()?.to_string();
                Some((name, r.get("sourceInfo")?.clone()))
            })
            .collect()
    }
}

/// Strip a leading `---\n…\n---` YAML frontmatter block (Pi `stripFrontmatter`); returns the body
/// after it, or the original text when no frontmatter is present.
fn strip_frontmatter(content: &str) -> &str {
    let Some(rest) = content.strip_prefix("---\n").or_else(|| content.strip_prefix("---\r\n")) else {
        return content;
    };
    // Find the closing `---` line.
    if let Some(idx) = rest.find("\n---") {
        let after = &rest[idx + 4..];
        after.strip_prefix('\n').or_else(|| after.strip_prefix("\r\n")).unwrap_or(after)
    } else {
        content
    }
}

/// The concatenated text of a `user` agent message, or `None` for any other role (Pi
/// `_getUserMessageText`, agent-session.ts:589-595). Used to match a streaming user message against
/// the facade steer/follow-up queue mirrors so they drain in lockstep with the agent.
fn agent_user_text(m: &AgentMessage) -> Option<String> {
    match m {
        AgentMessage::User { content, .. } => Some(
            content
                .iter()
                .filter_map(|c| match c {
                    Content::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        ),
        _ => None,
    }
}

/// The reduced `session_before_compact` decision (L4 gap #5): cancel the compaction, or proceed with
/// an optional extension-supplied compaction override.
enum BeforeCompactOutcome {
    /// A handler vetoed the compaction (Pi `{cancel:true}`).
    Cancel,
    /// Proceed — `Some` carries the guest's compaction override (Pi
    /// `SessionBeforeCompactResult.compaction`), `None` runs the default model summarization.
    Proceed(Option<CompactionOverride>),
}

/// Serialize a [`CompactionPreparation`] into the Pi `CompactionPreparation` byte-shape (camelCase)
/// for the `session_before_compact` seam (compaction.ts:690-700): the guest reads the real cut point,
/// the messages to summarize, the file operations, and the compaction settings.
///
/// `messagesToSummarize`/`turnPrefixMessages` are RAW `AgentMessage`s (Pi's own element type), so a
/// guest sees `{"role":"bashExecution","command":…}` / `{"role":"custom","customType":…}` rather
/// than the `convertToLlm`-rendered user messages, and `!!`-excluded bash commands are included.
fn compaction_preparation_value(prep: &CompactionPreparation) -> serde_json::Value {
    serde_json::json!({
        "firstKeptEntryId": prep.first_kept_entry_id,
        "messagesToSummarize": prep.messages_to_summarize,
        "turnPrefixMessages": prep.turn_prefix_messages,
        "isSplitTurn": prep.is_split_turn,
        "tokensBefore": prep.tokens_before,
        "previousSummary": prep.previous_summary,
        "fileOps": prep.file_ops.to_details(),
        "settings": prep.settings,
    })
}

/// Parse a guest compaction override (Pi `SessionBeforeCompactResult.compaction`, a `CompactionResult`)
/// into a [`CompactionOverride`]. A missing `summary` degrades to empty (never a panic).
fn parse_compaction_override(v: &serde_json::Value) -> CompactionOverride {
    CompactionOverride {
        summary: v.get("summary").and_then(|s| s.as_str()).unwrap_or_default().to_string(),
        first_kept_entry_id: v.get("firstKeptEntryId").and_then(|s| s.as_str()).map(EntryId::from),
        tokens_before: v.get("tokensBefore").and_then(serde_json::Value::as_u64),
        details: v.get("details").cloned(),
        // Pi threads `extensionCompaction.usage` straight into `appendCompaction`
        // (`agent-session.ts:1844,1872`); a malformed/absent bag simply records no usage.
        usage: v.get("usage").and_then(|u| serde_json::from_value(u.clone()).ok()),
    }
}

/// The Pi wire string for a compaction `reason` (`"manual"|"threshold"|"overflow"`).
fn compaction_reason_str(r: CompactionReason) -> &'static str {
    match r {
        CompactionReason::Manual => "manual",
        CompactionReason::Threshold => "threshold",
        CompactionReason::Overflow => "overflow",
    }
}

/// Options for [`AgentSession::bind_extensions_with`] — pi's `bindExtensions({ … })` argument
/// (`modes/print-mode.ts:73-101` @v0.83.0). Only the key cyrup did not already satisfy elsewhere is
/// modelled; see that method for where `mode` and `commandContextActions` live. SEAM-006.
#[derive(Default)]
pub struct BindOptions {
    /// pi's `onError` (`print-mode.ts:98-100`):
    ///
    /// ```text
    /// onError: (err) => {
    ///     console.error(`Extension error (${err.extensionPath}): ${err.error}`);
    /// },
    /// ```
    ///
    /// Registered on the session's [`cyrup_ext::ExtensionHost`] via `add_error_listener`, the same
    /// channel the RPC host already used (`cyrup-modes/src/rpc.rs`). `None` keeps the previous
    /// behaviour — a contained fault is logged and nothing else.
    pub on_error: Option<cyrup_ext::ErrorListener>,
}

/// How a session file was removed — Pi's `{ method: "trash" | "unlink" }`
/// (`modes/interactive/components/session-selector.ts:646` @v0.83.0, identical at v0.84.1). The
/// caller renders a different status line for each (`:846`), so this is on the wire of the seam
/// rather than an implementation detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteMethod {
    /// The OS `trash` CLI accepted it — the conversation is recoverable from the desktop trash.
    Trash,
    /// Permanent `unlink`, Pi's fallback when `trash` is absent or refused.
    Unlink,
}

impl DeleteMethod {
    /// Pi's own status text: `result.method === "trash" ? "Session moved to trash" : "Session
    /// deleted"` (`session-selector.ts:846`).
    pub const fn status_message(self) -> &'static str {
        match self {
            Self::Trash => "Session moved to trash",
            Self::Unlink => "Session deleted",
        }
    }
}

/// Delete a session JSONL, **trying the `trash` CLI first** — a literal port of pi's
/// `deleteSessionFile` (`modes/interactive/components/session-selector.ts:644-679` @v0.83.0,
/// byte-identical at v0.84.1). SEAM-063.
///
/// ```text
/// const trashArgs = sessionPath.startsWith("-") ? ["--", sessionPath] : [sessionPath];
/// const trashResult = spawnSync("trash", trashArgs, { encoding: "utf-8" });
/// if (trashResult.status === 0 || !existsSync(sessionPath)) return { ok: true, method: "trash" };
/// try { await unlink(sessionPath); return { ok: true, method: "unlink" }; }
/// catch (err) { … return { ok: false, method: "unlink", error }; }
/// ```
///
/// Four clauses are load-bearing and are reproduced exactly:
/// * the `--` guard for a leading-dash path (`:649`), so a session file named `-x.jsonl` is not
///   read as a `trash` option;
/// * success on exit-0 **or** the file having disappeared (`:666`) — some `trash` builds exit
///   non-zero while still moving the file;
/// * only then the permanent `unlink` (`:672`);
/// * the failure string carries BOTH the unlink message and pi's `trash: …` hint (`:675-678`),
///   truncated to pi's 200 characters and joined with pi's ` · `.
///
/// An already-absent file is success (cyrup's pre-existing idempotence, kept: pi's `!existsSync`
/// clause reaches the same verdict on that input).
pub fn delete_session_file_at(path: &Path) -> Result<DeleteMethod, SessionServiceError> {
    let mut cmd = std::process::Command::new("trash");
    cmd.args(trash_args(path));
    let trash = cmd.output();

    // Pi's `getTrashErrorHint()` (:651-663): the spawn error message and/or the FIRST line of
    // stderr, joined with " · " and sliced to 200 chars, prefixed `trash: `.
    let trash_hint = match &trash {
        Err(e) => Some(format!("trash: {e}")),
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let first = stderr.trim().lines().next().unwrap_or("").to_string();
            (!first.is_empty()).then(|| {
                let mut s = format!("trash: {first}");
                s.truncate(pi_hint_char_boundary(&s, 200));
                s
            })
        }
    };

    // Pi: `if (trashResult.status === 0 || !existsSync(sessionPath))` (:666).
    let trash_ok = matches!(&trash, Ok(out) if out.status.success());
    if trash_ok || !path.exists() {
        return Ok(DeleteMethod::Trash);
    }

    // Pi: the permanent fallback (:672-674).
    match std::fs::remove_file(path) {
        Ok(()) => Ok(DeleteMethod::Unlink),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DeleteMethod::Unlink),
        Err(e) => Err(SessionServiceError::Io(match trash_hint {
            // Pi: `${unlinkError} (${trashErrorHint})` (:677).
            Some(hint) => format!("{e} ({hint})"),
            None => e.to_string(),
        })),
    }
}

/// Rename a persisted session **by path**, without a live session — the same two calls
/// [`AgentSession::rename_session_file`] makes for a non-active target (Pi `onRenameSession` →
/// `SessionManager.setSessionName`, `session-selector.ts:585`): open the JSONL, append a
/// `session_info` record.
///
/// Exists so the PRE-LAUNCH `--resume` picker can persist a rename it already accepts on screen; it
/// runs before any session services exist, and needs none. SEAM-062.
pub fn rename_session_file_at(path: &Path, name: &str) -> Result<(), SessionServiceError> {
    let mut mgr = cyrup_session::SessionManager::open(path)?;
    mgr.append_session_info(name)?;
    Ok(())
}

/// pi's `trashArgs` — `sessionPath.startsWith("-") ? ["--", sessionPath] : [sessionPath]`
/// (`modes/interactive/components/session-selector.ts:649` @v0.83.0).
///
/// The guard is load-bearing rather than defensive: a session file whose NAME begins with `-` (the
/// picker labels rows by their first message, and nothing stops a session id or a `--session-dir`
/// path producing one) would otherwise be parsed by `trash` as an option instead of a path.
/// Extracted so it is unit-testable without putting a stub `trash` on `PATH` — this crate forbids
/// `unsafe`, and `std::env::set_var` is `unsafe` under edition 2024.
pub(crate) fn trash_args(path: &Path) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = Vec::with_capacity(2);
    if path.to_string_lossy().starts_with('-') {
        args.push(std::ffi::OsString::from("--"));
    }
    args.push(path.as_os_str().to_os_string());
    args
}

/// The largest byte index `<= max` that is a char boundary — JS `String.slice(0, 200)` counts UTF-16
/// units and never panics, so the port must not either.
fn pi_hint_char_boundary(s: &str, max: usize) -> usize {
    if s.len() <= max {
        return s.len();
    }
    let mut idx = max;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// Current wall-clock time in milliseconds (Pi `Date.now()`); 0 on a clock fault.
fn now_ms() -> i64 {
    (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64
}

/// `true` when two paths point at the same session file. Compares canonicalized paths when both
/// resolve (handling `..`/symlinks), else falls back to a lexical compare.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => a == b,
    }
}

/// Recursively flatten a [`cyrup_session::manager::TreeNode`] into pre-order [`SessionDagNode`]s
/// (feature #2 helper; Pi `flattenTree` DFS, `tree-selector.ts:199-320`). Children are already
/// timestamp-sorted by the manager (`build_node`). `depth` is the pre-order tree depth (0 = root).
fn flatten_dag_node(
    node: &cyrup_session::manager::TreeNode,
    parent_id: Option<EntryId>,
    depth: usize,
    leaf: Option<&EntryId>,
    out: &mut Vec<SessionDagNode>,
) {
    let id = node.entry.id();
    let (kind, label) = dag_display(&node.entry);
    let label = match &node.label {
        Some(l) => format!("[{l}] {label}"),
        None => label,
    };
    out.push(SessionDagNode {
        entry_id: id.clone(),
        parent_id,
        depth,
        label,
        kind,
        foldable: !node.children.is_empty(),
        is_leaf: leaf == Some(&id),
        has_label: node.label.is_some(),
        timestamp: node.entry.base().map(|b| b.timestamp.clone()).unwrap_or_default(),
    });
    for child in &node.children {
        flatten_dag_node(child, Some(id.clone()), depth + 1, leaf, out);
    }
}

/// Classify an entry and derive its one-line tree label (Pi `getEntryDisplayText`,
/// `tree-selector.ts:762-830`, condensed to a single normalized line). Returns `(kind, label)`.
fn dag_display(e: &cyrup_session::Entry) -> (SessionDagKind, String) {
    use cyrup_session::agent_message::AgentMessage as SessMsg;
    use cyrup_session::entry::{Entry, KnownEntry};

    let normalize = |s: &str| s.replace(['\n', '\t'], " ").trim().to_string();
    let clip = |s: String| -> String {
        let out: String = s.chars().take(80).collect();
        out
    };
    match e {
        Entry::Known(KnownEntry::Message { message, .. }) => match message {
            SessMsg::Core(Message::User { content, .. }) => {
                (SessionDagKind::Message, clip(format!("user: {}", normalize(&join_text(content)))))
            }
            SessMsg::Core(Message::Assistant(m)) => {
                let text = normalize(&join_text(&m.content));
                let body = if text.is_empty() { "(no content)".to_string() } else { text };
                (SessionDagKind::Message, clip(format!("assistant: {body}")))
            }
            SessMsg::Core(Message::ToolResult { tool_name, .. }) => {
                (SessionDagKind::Tool, format!("[{tool_name}]"))
            }
            SessMsg::BashExecution(b) => {
                (SessionDagKind::Message, clip(format!("[bash]: {}", normalize(&b.command))))
            }
            SessMsg::Custom(c) => (SessionDagKind::Message, format!("[{}]", c.custom_type)),
            // Pi's `AgentMessage` union also admits the two summary roles inside a `type:"message"`
            // entry; label them like the equivalent standalone entries.
            SessMsg::BranchSummary(b) => (
                SessionDagKind::Compaction,
                clip(format!("branch summary: {}", normalize(&b.summary))),
            ),
            SessMsg::CompactionSummary(_) => {
                (SessionDagKind::Compaction, "compaction".to_string())
            }
        },
        Entry::Known(KnownEntry::ModelChange { model_id, .. }) => {
            (SessionDagKind::ModelChange, format!("model → {model_id}"))
        }
        Entry::Known(KnownEntry::ThinkingLevelChange { thinking_level, .. }) => {
            (SessionDagKind::ThinkingChange, format!("thinking → {thinking_level}"))
        }
        Entry::Known(KnownEntry::Compaction { .. }) => {
            (SessionDagKind::Compaction, "compaction".to_string())
        }
        Entry::Known(KnownEntry::BranchSummary { summary, .. }) => {
            (SessionDagKind::Compaction, clip(format!("branch summary: {}", normalize(summary))))
        }
        Entry::Known(KnownEntry::SessionInfo { name, .. }) => {
            (SessionDagKind::Other, format!("title: {}", name.clone().unwrap_or_default()))
        }
        Entry::Known(KnownEntry::CustomMessage { custom_type, .. }) => {
            (SessionDagKind::Other, format!("[{custom_type}]"))
        }
        Entry::Known(KnownEntry::Custom { custom_type, .. }) => {
            (SessionDagKind::Other, format!("custom {custom_type}"))
        }
        Entry::Known(KnownEntry::Label { label, .. }) => {
            (SessionDagKind::Other, format!("label {}", label.clone().unwrap_or_default()))
        }
        Entry::Unknown(_) => (SessionDagKind::Other, "(entry)".to_string()),
    }
}

/// Join the `text` parts of a content vector (helper for [`dag_display`]).
fn join_text(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// The concatenated text of a core `user` message entry, or `None` for any other entry/role.
fn user_message_text(e: &cyrup_session::Entry) -> Option<String> {
    use cyrup_session::agent_message::AgentMessage as SessMsg;
    use cyrup_session::entry::{Entry, KnownEntry};
    let Entry::Known(KnownEntry::Message { message, .. }) = e else { return None };
    let SessMsg::Core(Message::User { content, .. }) = message else { return None };
    let text: String = content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    Some(text)
}

/// The text of a `custom_message` entry (Pi `agent-session.ts:2833-2840`): a raw string is used as
/// is; an array is filtered to its `text` parts and joined.
fn custom_message_text(content: &serde_json::Value) -> String {
    use serde_json::Value;
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|c| {
                if c.get("type").and_then(Value::as_str) == Some("text") {
                    c.get("text").and_then(Value::as_str).map(str::to_string)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Build a [`cyrup_session::compaction::BranchSummaryEntry`] payload from a freshly appended summary
/// entry (mirrors cyrup-session's private `branch_summary_entry_of`, compaction/mod.rs:309) so the
/// `/tree` op can surface the entry without re-running the summarizer.
fn branch_summary_entry_of(
    mgr: &SessionManager,
    id: &EntryId,
) -> Option<cyrup_session::compaction::BranchSummaryEntry> {
    use cyrup_session::compaction::BranchSummaryEntry;
    use cyrup_session::entry::{Entry, KnownEntry};
    match mgr.entry(id) {
        Some(Entry::Known(KnownEntry::BranchSummary {
            base,
            from_id,
            summary,
            details,
            usage,
            from_hook,
        })) => Some(BranchSummaryEntry {
            id: base.id.clone(),
            parent_id: base.parent_id.clone(),
            summary: summary.clone(),
            from_id: from_id.clone(),
            from_hook: from_hook.unwrap_or(false),
            details: details.clone(),
            usage: usage.clone(),
        }),
        _ => None,
    }
}

/// For a `position:"before"` fork: require a user-message anchor and return `(parent_id, text)`.
fn user_message_anchor(e: &cyrup_session::Entry) -> Option<(Option<EntryId>, String)> {
    user_message_text(e).map(|text| (e.parent_id(), text))
}

/// The [`SessionLayout`] a fork/clone writes its new file into. Mirrors Pi
/// `createBranchedSession`'s reuse of `this.getSessionDir()` (session-manager.ts:918-920,1343): the
/// directory fixed once at manager construction, never re-derived or re-encoded on branch. cyrup's
/// equivalent of `this.sessionDir` is the currently-open session file's own parent directory, which
/// is ALREADY fully resolved (`<root>/--<encoded-cwd>--` for a default session, or a literal
/// `--session-dir`), so it must be used LITERALLY. Feeding it back through the *encoded*
/// [`SessionLayout::new`] would append `--<encoded-cwd>--` a second time and land the branch one
/// directory too deep — orphaning it from every listing/resume path (gap-analysis 05, Finding 1). An
/// in-memory session (no file) never persists a branch, so the default-root fallback is inert.
pub(crate) fn branch_layout(mgr: &SessionManager) -> cyrup_session::SessionLayout {
    match mgr.session_file().and_then(Path::parent) {
        Some(dir) => cyrup_session::SessionLayout::literal(dir.to_path_buf(), mgr.cwd().to_path_buf()),
        None => cyrup_session::SessionLayout::for_cwd(mgr.cwd().to_path_buf()),
    }
}

/// Resolve the branch leaf + optional selected-text for an entry-anchored fork (Pi
/// agent-session-runtime.ts:268-284). Shared by [`AgentSession::fork_at_entry`] and the runtime's
/// throwaway-manager fork path so the anchor semantics stay identical.
pub(crate) fn fork_anchor(
    mgr: &SessionManager,
    entry: &EntryId,
    position: ForkPosition,
) -> Result<(Option<EntryId>, Option<String>), SessionServiceError> {
    let selected = mgr
        .entry(entry)
        .ok_or_else(|| SessionServiceError::InvalidForkEntry(entry.to_string()))?;
    match position {
        ForkPosition::At => Ok((Some(selected.id()), None)),
        ForkPosition::Before => {
            let (parent, text) = user_message_anchor(selected)
                .ok_or_else(|| SessionServiceError::InvalidForkEntry(entry.to_string()))?;
            Ok((parent, Some(text)))
        }
    }
}
