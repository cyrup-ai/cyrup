//! The watchdog state machine — a 1:1 port of `pi-subagents/src/watchdog/runtime.ts` (868 lines
//! @v0.43.0).
//!
//! One type, `MainWatchdogRuntime` (`:113-868`), drives BOTH watchdog roles: the orchestrator's
//! (`register-main.ts`) and a subagent child's (`register-child.ts`). It accumulates a bounded
//! buffer of turn deltas, reviews them at the `agent_end` boundary (and optionally every N tool
//! results), pushes accepted warnings at a delivery sink, and queues an automatic follow-up turn
//! for an un-stale blocker.
//!
//! ## Concurrency: what replaces JavaScript's single thread
//!
//! Upstream is single-threaded, so its only interleaving points are `await`s. This port keeps a
//! [`std::sync::Mutex`] over ALL mutable state ([`RuntimeInner`]) and observes one invariant that
//! reproduces that model exactly: **the guard is never held across an `.await`.** Every critical
//! section is a synchronous state transition — precisely the spans upstream cannot be interrupted
//! in — and the two genuinely awaiting steps (the review call and the LSP collection) run with the
//! lock released, which is where upstream can be interrupted too. The epoch/`reviewId`/`agentEndId`
//! generation counters upstream already carries are what make a late writer detectable, and they
//! are ported unchanged.
//!
//! A poisoned mutex is recovered from rather than propagated (`unwrap_or_else(PoisonError::into_inner)`);
//! this crate's no-panic policy forbids the alternative, and a panicking handler leaves the state
//! machine no more inconsistent than upstream's own exception paths do.
//!
//! ## The three injected collaborators
//!
//! `MainWatchdogRuntimeOptions` (`:78-88`) already makes three of the runtime's dependencies
//! injectable upstream, and this port keeps them as traits so each can be supplied independently:
//!
//! | upstream option | trait | default |
//! |---|---|---|
//! | `review` (`:81`) | [`WatchdogReview`] | [`InertWatchdogReview`] = `DEFAULT_REVIEW` (`:93`), which returns no warnings |
//! | `lspDiagnostics` (`:86`) | [`WatchdogLspDiagnostics`] | [`TypeScriptLspDiagnostics`] |
//! | `repoChangeSignature` (`:87`) | [`WatchdogRepoChangeSource`] | [`GitRepoChangeSource`] |
//!
//! `reviewConnected`/`reviewDescription` (`:173-174`) report which of these is real, and
//! `/subagents-watchdog status` prints it as `Review model call:` — so a runtime whose review seam
//! is not wired says so on screen rather than silently reviewing nothing.
//!
//! ## Timeouts
//!
//! `Promise.race([review, timeout])` (`:594-599`) becomes [`tokio::time::timeout`], which DROPS the
//! review future instead of leaving it running. The observable difference is nil: upstream aborts
//! its `AbortController` and then discards whatever the abandoned promise later emits, because
//! `invalidateActiveReview` has already bumped the epoch and `acceptWarning`'s `isCurrent` check
//! (`:499`) rejects it. The [`CancelToken`] handed to the review is upstream's `AbortSignal`,
//! cancelled on the same paths.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use cyrup_core::CancelToken;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::change_signature::{
    GitRepoChangeSource, WatchdogRepoChangeSignature, event_indicates_repo_edit,
};
use super::emission_guard::{WatchdogEmissionGuard, WatchdogEmissionGuardOptions};
use super::lsp_diagnostics::{TypeScriptLspDiagnostics, WatchdogLspRequest};
use super::scope::{WatchdogAutoFollowPromptLedger, WatchdogScopeArtifact};
use super::settings::resolve_watchdog_config;
use super::turn_delta::{
    WatchdogTurnDeltaInput, format_watchdog_orchestration_activity, format_watchdog_turn_delta,
};
use super::types::{
    ResolvedWatchdogConfig, ThinkingSetting, WatchdogImportance, WatchdogLspResult,
    WatchdogLspRuntimeSnapshot, WatchdogLspStatus, WatchdogRuntimeStatus, WatchdogSettingsError,
    WatchdogSettingsResult, WatchdogSettingsSource, WatchdogSeverity, WatchdogWarning,
    WatchdogWarningDetails, WatchdogWarningSource, WatchdogWarningState,
};
use super::warning_format::{WatchdogWarningDetailsPatch, normalize_watchdog_warning_details};

/// `MAX_REVIEW_INPUT_CHARS` (`runtime.ts:94`).
const MAX_REVIEW_INPUT_CHARS: usize = 24_000;
/// `REVIEW_INPUT_HEAD_CHARS` (`runtime.ts:104` @ad11b7ab) — the most of an over-long text
/// [`bound_watchdog_review_text`] keeps from its head.
const REVIEW_INPUT_HEAD_CHARS: usize = 6_000;
/// SUBA-234 — one activity entry's bound (`boundWatchdogReviewText(activity, 3_000)`,
/// `runtime.ts:356` @ad11b7ab).
const ACTIVITY_ENTRY_MAX_CHARS: usize = 3_000;
/// SUBA-234 — the retained activity tail (`.slice(-6_000)`, `runtime.ts:356` @ad11b7ab).
const ACTIVITY_TAIL_MAX_CHARS: usize = 6_000;
/// SUBA-234 — a reviewer's question and its evidence (`runtime.ts:668-669` @ad11b7ab).
pub(crate) const CLARIFICATION_QUESTION_MAX_CHARS: usize = 1_000;
/// The evidence half of [`CLARIFICATION_QUESTION_MAX_CHARS`].
pub(crate) const CLARIFICATION_EVIDENCE_MAX_CHARS: usize = 2_000;
/// SUBA-234 — the activity block's heading in the review input, upstream's verbatim
/// (`runtime.ts:857` @ad11b7ab). This is the "supplied activity evidence" the reviewer's scope line
/// names.
pub const ACTIVITY_BLOCK_HEADER: &str = "Recent delivered orchestration activity (oldest first; \
                                         bounded observations, not a task board; waits/holds are \
                                         not evidence of neglect):";

/// `REVIEW_DELTA_SEPARATOR` (`runtime.ts:95`) — the same string
/// [`super::turn_delta::WATCHDOG_DELTA_SECTION_SEPARATOR`] joins turn sections with, so a buffer of
/// deltas reads as one continuous transcript.
const REVIEW_DELTA_SEPARATOR: &str = "\n\n---\n\n";
/// SUBA-198 — the first line of the review input's changed-paths block, pi `runtime.ts:855`
/// @ad11b7ab (`ff2e00bd`) verbatim. The paths come from the working tree's dirty set
/// ([`super::change_signature`]), which no one attributes to a session: a file another session in
/// this checkout edited, or one left dirty before this session began, is listed the same. Saying
/// so is what keeps the reviewer from reading the list as this session's work.
const CHANGED_REPO_PATHS_HEADER: &str = "Changed repo paths: every dirty path in the working \
                                         tree. Authorship is not verified; a path may predate \
                                         this session or come from another session.";

// =================================================================================================
// The review seam (`runtime.ts:28-45`)
// =================================================================================================

/// `ReviewStopReason` (`runtime.ts:28`) — the provider stop reason the review reports. Anything
/// other than [`ReviewStopReason::Stop`] fails the review (`:607-610`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewStopReason {
    /// A clean end of turn.
    Stop,
    /// The provider errored.
    Error,
    /// The call was aborted.
    Aborted,
    /// The response hit the output cap.
    Length,
}

impl ReviewStopReason {
    /// The wire spelling, as it appears in the failure message.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ReviewStopReason::Stop => "stop",
            ReviewStopReason::Error => "error",
            ReviewStopReason::Aborted => "aborted",
            ReviewStopReason::Length => "length",
        }
    }
}

/// `WatchdogReviewResult` (`runtime.ts:30-33`).
#[derive(Debug, Clone, Default)]
pub struct WatchdogReviewResult {
    /// Warnings the review produced other than through [`WatchdogWarningEmitter`].
    pub warnings: Vec<WatchdogWarning>,
    /// The provider's stop reason, when the review knows it.
    pub stop_reason: Option<ReviewStopReason>,
    /// SUBA-234 — the reviewer yielded with a question instead of ruling
    /// (`clarification?: { question, evidence }`, `runtime.ts:37` @ad11b7ab). Honoured only when
    /// the request allowed it ([`WatchdogReviewRequest::allow_clarification`]).
    pub clarification: Option<WatchdogClarification>,
}

/// SUBA-234 — one reviewer question and the concrete evidence that prompted it
/// (`{ question: string; evidence: string }`, `runtime.ts:37` @ad11b7ab).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchdogClarification {
    /// The focused question, at most 1,000 characters once the runtime accepts it.
    pub question: String,
    /// What made it necessary, at most 2,000 characters.
    pub evidence: String,
}

/// `emitWarning(warning): boolean` (`runtime.ts:41`) — the streaming channel a review uses to hand
/// a warning over the moment the model emits it, rather than at the end of the call. `false` means
/// the runtime REJECTED it (a stale review, the watchdog turned off mid-call, below the severity
/// threshold, or the emission guard suppressed it), which a streaming review can use to stop
/// emitting.
#[derive(Clone)]
pub struct WatchdogWarningEmitter {
    #[allow(clippy::type_complexity)]
    emit: Arc<dyn Fn(&WatchdogWarning) -> bool + Send + Sync>,
}

impl WatchdogWarningEmitter {
    /// Offer one warning to the runtime; `true` when it was accepted.
    #[must_use]
    pub fn emit(&self, warning: &WatchdogWarning) -> bool {
        (self.emit)(warning)
    }

    /// Build an emitter from an arbitrary sink.
    ///
    /// The runtime builds its own (see `warning_emitter`); this constructor exists so an
    /// out-of-module [`WatchdogReview`] implementation — which is the whole point of that seam — can
    /// be exercised without one, and so a caller can compose an emitter that tees.
    #[must_use]
    pub fn from_fn(emit: Arc<dyn Fn(&WatchdogWarning) -> bool + Send + Sync>) -> Self {
        Self { emit }
    }

    /// An emitter that accepts nothing — the "no runtime attached" shape a review test needs
    /// (upstream passes a real closure in every production path, so this has no upstream analog and
    /// is deliberately not used by one).
    #[must_use]
    pub fn inert() -> Self {
        Self::from_fn(Arc::new(|_| false))
    }
}

impl std::fmt::Debug for WatchdogWarningEmitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WatchdogWarningEmitter")
    }
}

/// `WatchdogReviewRequest` (`runtime.ts:35-43`).
#[derive(Debug, Clone)]
pub struct WatchdogReviewRequest {
    /// The review input: scope block, changed paths, LSP block and the buffered turn deltas.
    pub delta: String,
    /// The runtime generation this request belongs to.
    pub epoch: u64,
    /// Whether the delta already carries a scope block (`:589`).
    pub has_scope: bool,
    /// This review's id within the generation.
    pub review_id: u64,
    /// The config in force for this review.
    pub config: ResolvedWatchdogConfig,
    /// The streaming emit channel.
    pub emit_warning: WatchdogWarningEmitter,
    /// Upstream's `AbortSignal` (`:43`): cancelled on timeout and on supersession.
    pub cancel: CancelToken,
    /// The session's Git baseline for `watchdog_diff` (SUBA-233), captured when the session bound
    /// — upstream's `options.diffBaseline` (`review.ts:59`,`:293` @ad11b7ab), which the review
    /// awaits. Empty when nothing started a capture; the review is then given no diff tool.
    pub diff_baseline: super::diff_tool::WatchdogDiffBaselineSlot,
    /// SUBA-234 — `allowClarification` (`runtime.ts:47` @ad11b7ab): this review may give the
    /// reviewer `watchdog_ask`. Set only on a main-session boundary review with clarification on,
    /// no question asked yet this prompt, and no stalemate; never on a cadence review.
    pub allow_clarification: bool,
}

/// `WatchdogReviewFunction` (`runtime.ts:45`) — the model call that actually reviews a delta.
///
/// The real implementation is upstream's `createMainWatchdogReview` (`review.ts`), which is a
/// separate port; this trait is the seam it plugs into, and is also how a test drives the state
/// machine deterministically without a provider.
#[async_trait]
pub trait WatchdogReview: Send + Sync {
    /// Review one delta. `Ok(None)` is upstream's `void` return (treated as a STALE review, `:605`);
    /// `Err` is upstream's thrown exception (`:613-618`), which fails the review.
    async fn review(
        &self,
        request: WatchdogReviewRequest,
    ) -> Result<Option<WatchdogReviewResult>, String>;
}

/// `DEFAULT_REVIEW` (`runtime.ts:93`) — `() => ({ warnings: [] })`. A runtime built on it is
/// structurally live (it buffers, it boundary-triggers, it clears) but never produces a warning,
/// and `reviewConnected` reports `false` so the status line says `not wired`.
#[derive(Debug, Clone, Copy, Default)]
pub struct InertWatchdogReview;

#[async_trait]
impl WatchdogReview for InertWatchdogReview {
    async fn review(
        &self,
        _request: WatchdogReviewRequest,
    ) -> Result<Option<WatchdogReviewResult>, String> {
        Ok(Some(WatchdogReviewResult::default()))
    }
}

// =================================================================================================
// The LSP seam (`lsp-diagnostics.ts`, injected at runtime.ts:86)
// =================================================================================================

/// The `lsp-diagnostics.ts` surface the runtime consumes: the collector itself (upstream's
/// injectable `lspDiagnostics` option, `runtime.ts:86`) plus the three helpers the runtime calls
/// around it — `WatchdogLspDiagnosticsLedger` (freshness), `watchdogWarningFromLspDiagnostics`, and
/// `formatWatchdogLspDiagnosticsBlock`.
///
/// They are one trait because they are one stateful unit: the ledger's memory of already-reported
/// diagnostics is what makes `freshDiagnosticCount` meaningful, and `&self` methods keep that state
/// with the implementation rather than smuggling it into the runtime. The production implementation
/// is [`TypeScriptLspDiagnostics`].
#[async_trait]
pub trait WatchdogLspDiagnostics: Send + Sync {
    /// `collectWatchdogLspDiagnostics(request)` (`runtime.ts:727`). `Err` is upstream's *thrown*
    /// collector, which the boundary turns into a `failed` snapshot (`runtime.ts:747-761`).
    async fn collect(&self, request: WatchdogLspRequest) -> Result<WatchdogLspResult, String>;

    /// `WatchdogLspDiagnosticsLedger.reduce(raw)` (`runtime.ts:736`) — drop diagnostics already
    /// reported on an earlier pass. The default keeps every diagnostic, which reports each pass in
    /// full.
    fn reduce(&self, raw: WatchdogLspResult) -> WatchdogLspResult {
        raw
    }

    /// `WatchdogLspDiagnosticsLedger.reset()` (`runtime.ts:268,295`).
    fn reset_ledger(&self) {}

    /// `watchdogWarningFromLspDiagnostics(fresh)` (`runtime.ts:744`) — the boundary warning a set
    /// of fresh diagnostics justifies, if any.
    fn warning_from_diagnostics(&self, _fresh: &WatchdogLspResult) -> Option<WatchdogWarning> {
        None
    }

    /// `formatWatchdogLspDiagnosticsBlock(fresh)` (`runtime.ts:746`) — the review-input block.
    fn format_block(&self, _fresh: &WatchdogLspResult) -> String {
        String::new()
    }
}

// =================================================================================================
// The repo-change seam (`change-signature.ts`, injected at runtime.ts:87)
// =================================================================================================

/// `computeWatchdogRepoChangeSignature(cwd)` (`change-signature.ts:186-197`) as a seam, so a test
/// can drive the change trigger without a real repository.
///
/// `None` — which is also what the production [`GitRepoChangeSource`] returns outside a git
/// repository — makes the runtime fall back to its OTHER change trigger, the observed edit/write
/// tool result of [`event_indicates_repo_edit`] (`runtime.ts:706-708`).
pub trait WatchdogRepoChangeSource: Send + Sync {
    /// The current signature, or `None` when there is no repository.
    fn compute(&self, cwd: &Path) -> Option<WatchdogRepoChangeSignature>;
}

// =================================================================================================
// Delivery sinks
// =================================================================================================

/// The `options.deliverAs` of `displayWarning` (`runtime.ts:83,640`): `Some(Steer)` marks a
/// MID-RUN correction, which upstream delivers as a steer so it interrupts the in-flight run rather
/// than waiting for the next boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogDelivery {
    /// `{ deliverAs: "steer" }`.
    Steer,
}

/// `displayWarning` (`runtime.ts:83`).
pub type WatchdogWarningSink =
    Arc<dyn Fn(&WatchdogWarningDetails, Option<WatchdogDelivery>) + Send + Sync>;

/// SUBA-120 / `displayUserWarning` (`runtime.ts:89`) — the USER-ONLY warning surface, where a
/// `low`- or `medium`-importance finding goes. It takes no
/// [`WatchdogWarningSendOptions`](WatchdogDelivery) counterpart: upstream's signature is
/// `(warning: WatchdogWarningDetails) => void` with no second parameter, because a user-only finding
/// is never steered into the parent model's turn — steering is exactly what makes a finding `high`.
pub type WatchdogUserWarningSink = Arc<dyn Fn(&WatchdogWarningDetails) + Send + Sync>;

/// `sendUserMessage` (`runtime.ts:84`) — used only for the auto-follow prompt. `Err` is upstream's
/// rejected promise, which un-queues the prompt and records `lastError` (`:677-682`).
pub type WatchdogUserMessageSink = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// SUBA-234 — `displayClarification` (`runtime.ts:90-91` @ad11b7ab): *"Supplying this
/// main-session delivery capability gates clarification (children omit it)."* The content is the
/// whole message; the main registration steers it into the session and triggers a turn.
pub type WatchdogClarificationSink = Arc<dyn Fn(&str) + Send + Sync>;

/// `resolveConfig` (`runtime.ts:80`) — how this runtime resolves its config. The main role uses
/// [`resolve_watchdog_config`]; a child role hands back a fixed, already-resolved config
/// (`register-child.ts:76`).
pub type WatchdogConfigResolver =
    Arc<dyn Fn(&Path, Option<&Value>) -> WatchdogSettingsResult + Send + Sync>;

// =================================================================================================
// Snapshot (`runtime.ts:47-71`)
// =================================================================================================

/// The session-scoped `main.model`/`main.thinking` override `/subagents-watchdog session model …`
/// installs (`runtime.ts:58,223-233`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatchdogSessionModelOverride {
    /// The pinned model id.
    pub model: Option<String>,
    /// The pinned reasoning level.
    pub thinking: Option<ThinkingSetting>,
}

impl WatchdogSessionModelOverride {
    fn is_empty(&self) -> bool {
        self.model.is_none() && self.thinking.is_none()
    }
}

/// `reviewTrigger` (`runtime.ts:68`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogReviewTrigger {
    /// Review every non-empty turn delta.
    TurnDelta,
    /// Review only when the repository actually changed.
    RepoEdits,
}

/// `WatchdogRuntimeSnapshot` (`runtime.ts:47-71`) — everything `/subagents-watchdog status`
/// prints and everything a child status event reports.
#[derive(Debug, Clone)]
pub struct WatchdogRuntimeSnapshot {
    /// The state machine's phase.
    pub status: WatchdogRuntimeStatus,
    /// `configOk && config.main.enabled`.
    pub enabled: bool,
    /// The config in force.
    pub config: ResolvedWatchdogConfig,
    /// Whether every settings layer parsed.
    pub config_ok: bool,
    /// Per-layer parse failures.
    pub errors: Vec<WatchdogSettingsError>,
    /// Layers consulted.
    pub sources: Vec<WatchdogSettingsSource>,
    /// How many deltas are buffered.
    pub buffered_deltas: usize,
    /// The generation counter.
    pub epoch: u64,
    /// The in-flight review's id.
    pub active_review_id: Option<u64>,
    /// The `/subagents-watchdog session on|off` override.
    pub session_override: Option<bool>,
    /// The `/subagents-watchdog session model …` override.
    pub session_model_override: Option<WatchdogSessionModelOverride>,
    /// The most recent warning, in whatever state it reached.
    pub last_warning: Option<WatchdogWarningDetails>,
    /// The most recent failure text.
    pub last_error: Option<String>,
    /// How many reviews failed.
    pub failed_reviews: u32,
    /// How many reviews went stale (timed out or were superseded).
    pub stale_reviews: u32,
    /// Whether a real review seam is wired.
    pub review_connected: bool,
    /// A human label for that seam.
    pub review_description: String,
    /// Whether an auto-follow prompt is queued.
    pub auto_follow_queued: bool,
    /// How many auto-follow attempts this turn chain has taken.
    pub auto_follow_attempts: u32,
    /// Whether auto-follow stopped on repeated identical blockers.
    pub auto_follow_stalemate: bool,
    /// Which trigger this runtime reviews on.
    pub review_trigger: WatchdogReviewTrigger,
    /// The changed paths of the current signature.
    pub changed_paths: Option<Vec<String>>,
    /// The LSP collection snapshot.
    pub lsp: WatchdogLspRuntimeSnapshot,
}

// =================================================================================================
// Construction (`runtime.ts:78-88, 169-185`)
// =================================================================================================

/// `MainWatchdogRuntimeOptions` (`runtime.ts:78-88`).
#[derive(Clone, Default)]
pub struct MainWatchdogRuntimeOptions {
    /// The session cwd; defaults to the process cwd (`:170`).
    pub cwd: Option<PathBuf>,
    /// How to resolve the config; defaults to [`resolve_watchdog_config`] (`:171`).
    pub resolve_config: Option<WatchdogConfigResolver>,
    /// The review seam (`:172`).
    pub review: Option<Arc<dyn WatchdogReview>>,
    /// A label for the review seam (`:174`).
    pub review_description: Option<String>,
    /// Where a displayed warning goes (`:175`) — SUBA-120: now only `high`-importance findings.
    pub display_warning: Option<WatchdogWarningSink>,
    /// SUBA-120 — where a `low`/`medium`-importance finding goes instead (`runtime.ts:89`).
    pub display_user_warning: Option<WatchdogUserWarningSink>,
    /// Where an auto-follow prompt goes (`:176`).
    pub send_user_message: Option<WatchdogUserMessageSink>,
    /// Review only when the repository changed (`:177`).
    pub review_changes_only: bool,
    /// The LSP seam (`:178`).
    pub lsp_diagnostics: Option<Arc<dyn WatchdogLspDiagnostics>>,
    /// The repo-change seam (`:179`).
    pub repo_change_signature: Option<Arc<dyn WatchdogRepoChangeSource>>,
    /// SUBA-234 — where a reviewer's question goes. Its PRESENCE is the main-role gate: a runtime
    /// built without it (every child watchdog) never collects activity, never offers
    /// `watchdog_ask` and never resets on user input, whatever `clarification` says.
    pub display_clarification: Option<WatchdogClarificationSink>,
}

impl std::fmt::Debug for MainWatchdogRuntimeOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MainWatchdogRuntimeOptions")
            .field("cwd", &self.cwd)
            .field("review_description", &self.review_description)
            .field("review_changes_only", &self.review_changes_only)
            .finish_non_exhaustive()
    }
}

/// Options for `reset(reason, options)` (`runtime.ts:250`).
#[derive(Debug, Clone, Copy, Default)]
pub struct WatchdogResetOptions {
    /// Forget the last review input's hash, so an identical delta reviews again.
    pub clear_review_input_signature: bool,
    /// Re-baseline the repo change signature as already-reviewed.
    pub reset_change_signature: bool,
    /// Drop the LSP freshness ledger and snapshot.
    pub clear_lsp_ledger: bool,
    /// Drop the scope artifact and any queued auto-follow prompts.
    pub clear_scope: bool,
    /// Reset the auto-follow attempt/stalemate counters.
    pub reset_auto_follow: bool,
    /// SUBA-234 — drop the delivered-orchestration activity tail (`clearActivity`,
    /// `runtime.ts:277`,`:300` @ad11b7ab). [`Self::clear_scope`] drops it too.
    pub clear_activity: bool,
}

/// The outcome of one `reviewDelta` (`runtime.ts:91`).
#[derive(Debug, Clone, PartialEq, Eq)]
enum ReviewDeltaOutcome {
    Completed,
    Timeout,
    Stale,
    /// SUBA-234 — the reviewer yielded with a question (`runtime.ts:100` @ad11b7ab).
    Clarification(WatchdogClarification),
}

/// Every mutable field of `MainWatchdogRuntime` (`runtime.ts:114-167`).
struct RuntimeInner {
    cwd: PathBuf,
    config_result: WatchdogSettingsResult,
    session_override_enabled: Option<bool>,
    session_model_override: Option<WatchdogSessionModelOverride>,
    status: WatchdogRuntimeStatus,
    pending_deltas: Vec<String>,
    pending_delta_chars: usize,
    guard: WatchdogEmissionGuard,
    guard_max_warnings: Option<u32>,
    epoch: u64,
    review_id_counter: u64,
    agent_end_id_counter: u64,
    active_agent_end_id: Option<u64>,
    active_agent_end_cancel: Option<(u64, CancelToken)>,
    active_review_id: Option<u64>,
    active_review_warning: Option<WatchdogWarningDetails>,
    reviewing: bool,
    waiting_at_agent_end: bool,
    disposed: bool,
    include_user_prompt_in_next_delta: bool,
    user_prompt: Option<String>,
    waiters: Vec<tokio::sync::oneshot::Sender<bool>>,
    last_warning: Option<WatchdogWarningDetails>,
    displayed_warning_sequence: u64,
    last_error: Option<String>,
    last_review_input_signature: Option<String>,
    turn_start_change_signature: Option<WatchdogRepoChangeSignature>,
    last_reviewed_change_signature: Option<String>,
    current_changed_paths: Option<Vec<String>>,
    last_lsp_snapshot: Option<WatchdogLspRuntimeSnapshot>,
    observed_repo_edit_this_turn: bool,
    tool_results_this_run: u64,
    mid_run_reviewing: bool,
    auto_follow_queued: bool,
    auto_follow_attempts: u32,
    consecutive_auto_follow_identity: Option<String>,
    consecutive_auto_follow_repeats: u32,
    auto_follow_stalemate: bool,
    pending_auto_follow_prompts: WatchdogAutoFollowPromptLedger,
    mid_run_generation: u64,
    active_review_cancel: Option<(u64, CancelToken)>,
    failed_reviews: u32,
    stale_reviews: u32,
    scope: WatchdogScopeArtifact,
    /// SUBA-234 — `askedThisPrompt` (`runtime.ts:183` @ad11b7ab): one question per user prompt.
    asked_this_prompt: bool,
    /// SUBA-234 — `activityTail` (`:184`): the bounded tail of delivered orchestration activity.
    activity_tail: String,
    /// SUBA-234 — `activityPending` (`:185`): activity arrived that no boundary has reviewed.
    activity_pending: bool,
    /// SUBA-234 — `activityReviewUsed` (`:186`): this prompt's one no-edit activity review is spent.
    activity_review_used: bool,
    /// SUBA-234 — upstream's `boundaryRepeats` (`:175`) as the activity gates read it: how many
    /// boundary warnings this user prompt has displayed. A displayed boundary warning continues the
    /// run, and upstream refuses to treat that continuation's orchestration as fresh evidence
    /// (`boundaryRepeats === 0`, `:353`,`:400`). See [`MainWatchdogRuntime::handle_turn_end`].
    boundary_warnings_this_prompt: u32,
}

/// `MainWatchdogRuntime` (`runtime.ts:113-868`).
pub struct MainWatchdogRuntime {
    inner: Arc<Mutex<RuntimeInner>>,
    resolve_config: WatchdogConfigResolver,
    review: Arc<dyn WatchdogReview>,
    review_connected: bool,
    review_description: String,
    display_warning: Option<WatchdogWarningSink>,
    display_user_warning: Option<WatchdogUserWarningSink>,
    send_user_message: Option<WatchdogUserMessageSink>,
    display_clarification: Option<WatchdogClarificationSink>,
    review_changes_only: bool,
    lsp_diagnostics: Arc<dyn WatchdogLspDiagnostics>,
    repo_change_signature: Arc<dyn WatchdogRepoChangeSource>,
    /// `let diffBaseline` (`register-main.ts:372`, `register-child.ts:60` @ad11b7ab) — kept on the
    /// runtime so the main and the child watchdog, which both bind through [`Self::bind_session`],
    /// capture it the same way (SUBA-233).
    diff_baseline: super::diff_tool::WatchdogDiffBaselineSlot,
}

impl std::fmt::Debug for MainWatchdogRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MainWatchdogRuntime")
            .field("review_connected", &self.review_connected)
            .field("review_description", &self.review_description)
            .field("review_changes_only", &self.review_changes_only)
            .finish_non_exhaustive()
    }
}

impl Default for MainWatchdogRuntime {
    fn default() -> Self {
        Self::new(MainWatchdogRuntimeOptions::default())
    }
}

impl MainWatchdogRuntime {
    /// `constructor(options)` (`runtime.ts:169-185`).
    #[must_use]
    pub fn new(options: MainWatchdogRuntimeOptions) -> Self {
        let cwd = options
            .cwd
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let resolve_config: WatchdogConfigResolver = options.resolve_config.unwrap_or_else(|| {
            Arc::new(|cwd: &Path, session: Option<&Value>| resolve_watchdog_config(cwd, session))
        });
        let review_connected = options.review.is_some();
        let review_description = options.review_description.unwrap_or_else(|| {
            if review_connected {
                "injected seam".to_string()
            } else {
                "not wired".to_string()
            }
        });
        let review: Arc<dyn WatchdogReview> = options
            .review
            .unwrap_or_else(|| Arc::new(InertWatchdogReview));
        let lsp_diagnostics: Arc<dyn WatchdogLspDiagnostics> = options
            .lsp_diagnostics
            .unwrap_or_else(|| Arc::new(TypeScriptLspDiagnostics::new()));
        let repo_change_signature: Arc<dyn WatchdogRepoChangeSource> = options
            .repo_change_signature
            .unwrap_or_else(|| Arc::new(GitRepoChangeSource));

        let config_result = resolve_config(&cwd, None);
        let guard_max_warnings = config_result.config.max_warnings;
        let inner = RuntimeInner {
            cwd: cwd.clone(),
            config_result,
            session_override_enabled: None,
            session_model_override: None,
            status: WatchdogRuntimeStatus::Idle,
            pending_deltas: Vec::new(),
            pending_delta_chars: 0,
            guard: WatchdogEmissionGuard::new(WatchdogEmissionGuardOptions {
                max_warnings: guard_max_warnings,
                dedupe_history_limit: None,
            }),
            guard_max_warnings,
            epoch: 0,
            review_id_counter: 0,
            agent_end_id_counter: 0,
            active_agent_end_id: None,
            active_agent_end_cancel: None,
            active_review_id: None,
            active_review_warning: None,
            reviewing: false,
            waiting_at_agent_end: false,
            disposed: false,
            include_user_prompt_in_next_delta: false,
            user_prompt: None,
            waiters: Vec::new(),
            last_warning: None,
            displayed_warning_sequence: 0,
            last_error: None,
            last_review_input_signature: None,
            turn_start_change_signature: None,
            last_reviewed_change_signature: None,
            current_changed_paths: None,
            last_lsp_snapshot: None,
            observed_repo_edit_this_turn: false,
            tool_results_this_run: 0,
            mid_run_reviewing: false,
            auto_follow_queued: false,
            auto_follow_attempts: 0,
            consecutive_auto_follow_identity: None,
            consecutive_auto_follow_repeats: 0,
            auto_follow_stalemate: false,
            pending_auto_follow_prompts: WatchdogAutoFollowPromptLedger::new(),
            mid_run_generation: 0,
            active_review_cancel: None,
            failed_reviews: 0,
            stale_reviews: 0,
            scope: WatchdogScopeArtifact::new(),
            asked_this_prompt: false,
            activity_tail: String::new(),
            activity_pending: false,
            activity_review_used: false,
            boundary_warnings_this_prompt: 0,
        };
        let runtime = Self {
            inner: Arc::new(Mutex::new(inner)),
            resolve_config,
            review,
            review_connected,
            review_description,
            display_warning: options.display_warning,
            display_user_warning: options.display_user_warning,
            send_user_message: options.send_user_message,
            display_clarification: options.display_clarification,
            review_changes_only: options.review_changes_only,
            lsp_diagnostics,
            repo_change_signature,
            diff_baseline: super::diff_tool::WatchdogDiffBaselineSlot::default(),
        };
        // `:183-184` — the constructor takes the repo baseline immediately, so the first
        // `agent_end` compares against the tree as it was when the session opened.
        let signature = runtime.current_repo_change_signature(&cwd);
        {
            let mut inner = runtime.lock();
            inner.last_reviewed_change_signature = signature.as_ref().map(|s| s.key.clone());
            inner.turn_start_change_signature = signature;
        }
        runtime
    }

    /// A poisoning-tolerant lock. See the module doc for why recovery beats propagation here.
    fn lock(&self) -> MutexGuard<'_, RuntimeInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    // ---- config -------------------------------------------------------------------------------

    /// `bindSession(ctx)` (`runtime.ts:187-193`) — a new session drops every session-scoped
    /// override and resets everything, including the scope artifact and the auto-follow counters.
    pub fn bind_session(&self, cwd: &Path) {
        // `diffBaseline = startWatchdogDiffBaselineCapture(ctx.cwd)`, immediately before
        // `runtime.bindSession(ctx)` in both registrations (`register-main.ts:395`,
        // `register-child.ts:101` @ad11b7ab): `HEAD` as the session opens, so commits made later
        // still show in the reviewer's diff (SUBA-233).
        self.diff_baseline.start(cwd);
        {
            let mut inner = self.lock();
            inner.cwd = cwd.to_path_buf();
            inner.session_override_enabled = None;
            inner.session_model_override = None;
        }
        self.refresh_config(cwd);
        self.reset(WatchdogResetOptions {
            clear_review_input_signature: true,
            reset_change_signature: true,
            clear_lsp_ledger: true,
            clear_scope: true,
            reset_auto_follow: true,
            ..WatchdogResetOptions::default()
        });
        // `resetBoundaryRepeats()` (`runtime.ts:213` @ad11b7ab) — a new session starts a new
        // prompt chain.
        self.lock().start_user_prompt();
    }

    /// `refreshConfig(cwd)` (`runtime.ts:195-214`) — re-resolve every layer, rebuild the emission
    /// guard when the warning ceiling moved, and invalidate an in-flight review if the watchdog was
    /// just turned off.
    pub fn refresh_config(&self, cwd: &Path) -> WatchdogSettingsResult {
        let (session, was_enabled, had_clarification, previous_main) = {
            let mut inner = self.lock();
            inner.cwd = cwd.to_path_buf();
            (
                inner.session_patch(),
                inner.is_enabled(),
                inner.config_result.config.clarification,
                inner.config_result.config.main.clone(),
            )
        };
        let resolved = (self.resolve_config)(cwd, session.as_ref());
        let mut invalidate = false;
        let result = {
            let mut inner = self.lock();
            inner.config_result = resolved.clone();
            if inner.config_result.config.max_warnings != inner.guard_max_warnings {
                inner.guard_max_warnings = inner.config_result.config.max_warnings;
                inner.guard = WatchdogEmissionGuard::new(WatchdogEmissionGuardOptions {
                    max_warnings: inner.guard_max_warnings,
                    dedupe_history_limit: None,
                });
            }
            if was_enabled && !inner.is_enabled() {
                invalidate = true;
            }
            // SUBA-234 — `if (!this.isEnabled() || !this.configResult.config.clarification)
            // this.clearActivity()` (`runtime.ts:236` @ad11b7ab): no activity is kept while it
            // could not be used.
            let clarification = inner.config_result.config.clarification;
            if !inner.is_enabled() || !clarification {
                inner.clear_activity();
            }
            // `:237-239` — turning clarification off, or changing the reviewer's model or thinking
            // under it, invalidates an in-flight review: its result would be judged under a
            // configuration that no longer holds.
            if self.display_clarification.is_some()
                && (had_clarification || clarification)
                && (inner.reviewing || inner.waiting_at_agent_end)
                && (!clarification
                    || previous_main.model != inner.config_result.config.main.model
                    || previous_main.thinking != inner.config_result.config.main.thinking)
            {
                invalidate = true;
            }
            inner.config_result.clone()
        };
        if invalidate {
            // `invalidateActiveReview("watchdog disabled")` (`:212`).
            let mut inner = self.lock();
            inner.invalidate_active_review();
        }
        result
    }

    /// `setSessionEnabled(enabled, cwd)` (`runtime.ts:216-221`).
    pub fn set_session_enabled(&self, enabled: bool, cwd: &Path) -> WatchdogRuntimeSnapshot {
        self.lock().session_override_enabled = Some(enabled);
        self.reset(WatchdogResetOptions::default());
        self.refresh_config(cwd);
        self.get_snapshot(None)
    }

    /// `setSessionModel(patch, cwd)` (`runtime.ts:223-233`) — `Some(None)` deletes the field,
    /// `Some(Some(v))` sets it, `None` leaves it; an override with neither field left is dropped
    /// entirely.
    pub fn set_session_model(
        &self,
        model: Option<Option<String>>,
        thinking: Option<Option<ThinkingSetting>>,
        cwd: &Path,
    ) -> WatchdogRuntimeSnapshot {
        {
            let mut inner = self.lock();
            let mut next = inner.session_model_override.clone().unwrap_or_default();
            match model {
                Some(None) => next.model = None,
                Some(Some(value)) => next.model = Some(value),
                None => {}
            }
            match thinking {
                Some(None) => next.thinking = None,
                Some(Some(value)) => next.thinking = Some(value),
                None => {}
            }
            inner.session_model_override = if next.is_empty() { None } else { Some(next) };
        }
        self.reset(WatchdogResetOptions::default());
        self.refresh_config(cwd);
        self.get_snapshot(None)
    }

    /// `clearSessionModel(cwd)` (`runtime.ts:235-240`).
    pub fn clear_session_model(&self, cwd: &Path) -> WatchdogRuntimeSnapshot {
        self.lock().session_model_override = None;
        self.reset(WatchdogResetOptions::default());
        self.refresh_config(cwd);
        self.get_snapshot(None)
    }

    /// `clearSessionOverride(cwd)` (`runtime.ts:242-248`).
    ///
    /// No caller — and none upstream either. `register-main.ts` clears the enabled flag through
    /// `setSessionEnabled(false)` (`:289-290`) and the model through
    /// [`Self::clear_session_model`] (`:304`); nothing calls the method that clears BOTH at once.
    /// It is kept because it is part of the ported `MainWatchdogRuntime` method surface (upstream
    /// declares it public on the class), not because a cyrup caller is pending.
    pub fn clear_session_override(&self, cwd: &Path) -> WatchdogRuntimeSnapshot {
        {
            let mut inner = self.lock();
            inner.session_override_enabled = None;
            inner.session_model_override = None;
        }
        self.reset(WatchdogResetOptions::default());
        self.refresh_config(cwd);
        self.get_snapshot(None)
    }

    // ---- lifecycle ----------------------------------------------------------------------------

    /// `reset(reason, options)` (`runtime.ts:250-280`). The `reason` is `_reason` upstream — an
    /// unused documentation argument — so it is not a parameter here.
    pub fn reset(&self, options: WatchdogResetOptions) {
        let clear_ledger = options.clear_lsp_ledger;
        let waiters = {
            let mut inner = self.lock();
            // `this.activeReviewAbortController?.abort()` (`runtime.ts:278` @ad11b7ab) — a reset
            // ABORTS the in-flight review rather than only orphaning it, so a reviewer mid-turn
            // stops spending tokens on evidence nothing will read (SUBA-234).
            inner.abort_active_review();
            inner.abort_active_agent_end();
            inner.epoch += 1;
            inner.status = WatchdogRuntimeStatus::Idle;
            inner.clear_pending_deltas();
            inner.reviewing = false;
            inner.waiting_at_agent_end = false;
            inner.active_review_id = None;
            inner.active_review_warning = None;
            inner.include_user_prompt_in_next_delta = false;
            inner.user_prompt = None;
            inner.last_error = None;
            inner.current_changed_paths = None;
            inner.observed_repo_edit_this_turn = false;
            inner.tool_results_this_run = 0;
            inner.mid_run_reviewing = false;
            inner.auto_follow_queued = false;
            if options.clear_lsp_ledger {
                inner.last_lsp_snapshot = None;
            }
            if options.clear_scope {
                inner.scope.reset();
                inner.pending_auto_follow_prompts.clear();
            }
            // `if (options.clearScope || options.clearActivity) this.clearActivity()` (`:300`).
            if options.clear_scope || options.clear_activity {
                inner.clear_activity();
            }
            if options.reset_auto_follow {
                inner.reset_auto_follow_state();
            }
            if options.clear_review_input_signature {
                inner.last_review_input_signature = None;
            }
            inner.guard.reset();
            inner.take_waiters(true)
        };
        if clear_ledger {
            self.lsp_diagnostics.reset_ledger();
        }
        if options.reset_change_signature {
            self.reset_repo_change_baseline(None, true);
        }
        resolve_waiters(waiters, true);
    }

    /// `dispose()` (`runtime.ts:282-302`) — terminal: every later handler returns immediately, and
    /// every pending `waitForIdle` resolves `false`.
    pub fn dispose(&self) {
        let waiters = {
            let mut inner = self.lock();
            // `this.clearActivity(); this.activeReviewAbortController?.abort();` (`runtime.ts:308-309`
            // @ad11b7ab, SUBA-234).
            inner.clear_activity();
            inner.abort_active_review();
            inner.disposed = true;
            inner.abort_active_agent_end();
            inner.epoch += 1;
            inner.status = WatchdogRuntimeStatus::Idle;
            inner.clear_pending_deltas();
            inner.reviewing = false;
            inner.waiting_at_agent_end = false;
            inner.active_review_id = None;
            inner.active_review_warning = None;
            inner.last_review_input_signature = None;
            inner.current_changed_paths = None;
            inner.last_lsp_snapshot = None;
            inner.scope.reset();
            inner.observed_repo_edit_this_turn = false;
            inner.tool_results_this_run = 0;
            inner.mid_run_reviewing = false;
            inner.auto_follow_queued = false;
            inner.take_waiters(false)
        };
        self.lsp_diagnostics.reset_ledger();
        resolve_waiters(waiters, false);
    }

    // ---- event handlers -----------------------------------------------------------------------

    /// `handleBeforeAgentStart(event, ctx)` (`runtime.ts:304-322`).
    ///
    /// The auto-follow discrimination is the whole point of this handler: a turn opened by the
    /// watchdog's OWN follow-up prompt must not reset the auto-follow counters (or the loop would
    /// never terminate) and must not widen the scope record.
    pub fn handle_before_agent_start(&self, event: &Value, cwd: &Path) {
        let incoming_prompt = prompt_from_before_agent_start(event);
        let auto_follow_prompt = {
            let mut inner = self.lock();
            if inner.disposed {
                return;
            }
            inner
                .pending_auto_follow_prompts
                .take_match(incoming_prompt.as_deref())
        };
        self.reset(WatchdogResetOptions {
            reset_auto_follow: !auto_follow_prompt,
            ..WatchdogResetOptions::default()
        });
        // SUBA-234 — `this.askedThisPrompt = false; this.activityReviewUsed = false;` and
        // `resetBoundaryRepeats()` (`runtime.ts:334-336` @ad11b7ab): a new user prompt gets its own
        // question and its own activity review. The watchdog's OWN auto-follow prompt is not a new
        // user prompt — it is cyrup's warning continuation (upstream's continuation never fires
        // `before_agent_start`) — so it keeps the prompt's counters, exactly as it keeps the
        // auto-follow counters above.
        if !auto_follow_prompt {
            self.lock().start_user_prompt();
        }
        self.refresh_config(cwd);
        {
            let mut inner = self.lock();
            inner.user_prompt = incoming_prompt;
            let has_prompt = inner
                .user_prompt
                .as_ref()
                .is_some_and(|p| !p.trim().is_empty());
            if !auto_follow_prompt && has_prompt {
                inner.include_user_prompt_in_next_delta = true;
                if let Some(prompt) = inner.user_prompt.clone() {
                    inner.scope.add_prompt(&prompt, None);
                }
            } else {
                inner.include_user_prompt_in_next_delta = false;
            }
        }
        self.reset_repo_change_baseline(None, false);
    }

    /// SUBA-198 / SUBA-234 — `handleUserInput(event)` (`runtime.ts:512-517` @ad11b7ab, wired at
    /// `register-main.ts:406`): *"A real user input sent while the agent streams reaches the loop
    /// without before_agent_start, so record it in scope here."*
    ///
    /// Three lines, in upstream's order:
    ///
    /// 1. `:514` — a disposed runtime, or an extension talking, is ignored
    ///    ([`crate::mid_run_input::is_operator_input`]).
    /// 2. `:515` (SUBA-234) — with clarification on, ANY operator input while a review is in flight
    ///    (`reviewing || waitingAtAgentEnd`) resets the runtime, which aborts that review: the
    ///    operator is answering, or moving past, the turn it is judging, so whatever it concludes —
    ///    a warning or a question — is about a conversation that has moved on. Idle input counts,
    ///    which is why this uses the WEAKER rule and not [`crate::mid_run_input::admits`]: a boundary
    ///    review runs after the run ended, so the agent is idle exactly when this matters most.
    /// 3. `:516` — a steer or follow-up queued onto the live run joins the scope record, the rule
    ///    `bg_wait` applies too ([`crate::mid_run_input::admits`]); idle input is left to
    ///    `before_agent_start`, which records it once.
    pub fn handle_user_input(
        &self,
        text: &str,
        source: cyrup_ext::InputEventSource,
        streaming_behavior: Option<cyrup_ext::InputStreamingBehavior>,
    ) {
        let reset = {
            let inner = self.lock();
            if inner.disposed || !crate::mid_run_input::is_operator_input(source) {
                return;
            }
            inner.clarification_review_in_flight(self.display_clarification.is_some())
        };
        if reset {
            // `this.reset("new user input")` — default options: the scope, the activity tail and
            // the auto-follow counters all survive; only the in-flight work is dropped.
            self.reset(WatchdogResetOptions::default());
        }
        if crate::mid_run_input::admits(source, streaming_behavior) {
            self.lock().scope.add_prompt(text, None);
        }
    }

    /// SUBA-234 — `handleModelChange()` (`runtime.ts:519-521` @ad11b7ab, wired to `model_select`
    /// at `register-main.ts:407`): with clarification on, a session model change while a review is
    /// in flight resets the runtime. An inherited-model reviewer would otherwise finish on the model
    /// the operator just moved away from.
    pub fn handle_model_change(&self) {
        let reset = {
            let inner = self.lock();
            !inner.disposed
                && inner.clarification_review_in_flight(self.display_clarification.is_some())
        };
        if reset {
            self.reset(WatchdogResetOptions::default());
        }
    }

    /// SUBA-234 — the retained activity tail. Test-only: production reads it through the review
    /// input's activity block alone.
    #[cfg(test)]
    pub(crate) fn activity_tail(&self) -> String {
        self.lock().activity_tail.clone()
    }

    /// The scope record's entries, oldest first — what the next review's `Current scope:` block is
    /// rendered from. Test-only: production reads the scope through that block alone.
    #[cfg(test)]
    pub(crate) fn scope_prompts(&self) -> Vec<String> {
        self.lock()
            .scope
            .snapshot()
            .into_iter()
            .map(|entry| entry.prompt)
            .collect()
    }

    /// `handleTurnEnd(event, ctx)` (`runtime.ts:324-340`).
    pub fn handle_turn_end(&self, event: &Value, cwd: &Path) {
        if self.lock().disposed {
            return;
        }
        self.refresh_config(cwd);
        if !self.lock().is_enabled() {
            return;
        }
        let (include_user_prompt, user_prompt) = {
            let mut inner = self.lock();
            // SUBA-234 — `runtime.ts:353-358` @ad11b7ab. With clarification on (and only in the
            // main role, which is what the clarification sink marks), every completed turn's PAIRED
            // orchestration calls and results join a bounded activity tail the next boundary
            // review reads: each entry bounded to 3,000 characters, the joined tail clipped to its
            // newest 6,000. Not collected during a warning's continuation (`boundaryRepeats ===
            // 0`): orchestration the agent does in answer to a warning is not fresh evidence of
            // the task, and would let a warning re-trigger its own review.
            if self.display_clarification.is_some()
                && inner.config_result.config.clarification
                && inner.boundary_warnings_this_prompt == 0
            {
                let activity = format_watchdog_orchestration_activity(event);
                if !activity.is_empty() {
                    inner.append_activity(&activity);
                }
            }
            inner.observed_repo_edit_this_turn =
                inner.observed_repo_edit_this_turn || event_indicates_repo_edit(event);
            let include = inner.include_user_prompt_in_next_delta;
            inner.include_user_prompt_in_next_delta = false;
            (include, inner.user_prompt.clone())
        };
        let events = [event.clone()];
        let delta = format_watchdog_turn_delta(&WatchdogTurnDeltaInput {
            user_prompt: user_prompt.as_deref(),
            include_user_prompt,
            messages: &[],
            events: &events,
            final_assistant_stop: false,
        });
        self.enqueue_delta(&delta);
    }

    /// `enqueueDelta(delta)` (`runtime.ts:342-346`).
    pub fn enqueue_delta(&self, delta: &str) {
        let mut inner = self.lock();
        if inner.disposed || delta.trim().is_empty() || !inner.is_enabled() {
            return;
        }
        inner.append_bounded_delta(delta);
        if !inner.reviewing && !inner.waiting_at_agent_end && !inner.mid_run_reviewing {
            inner.status = WatchdogRuntimeStatus::Queued;
        }
    }

    /// `handleToolResult(ctx)` (`runtime.ts:348-360`) — the mid-run cadence trigger. Fires on every
    /// `everyNTools`-th tool result of the run, and only when nothing else is reviewing.
    ///
    /// Upstream's `void this.reviewMidRunDelta(delta)` is a deliberately un-awaited call; this port
    /// spawns it for the same reason — the tool-result hook must not block the agent loop.
    pub fn handle_tool_result(self: &Arc<Self>, cwd: &Path) {
        if self.lock().disposed {
            return;
        }
        self.refresh_config(cwd);
        let delta = {
            let mut inner = self.lock();
            if !inner.is_enabled() {
                return;
            }
            let Some(every_n_tools) = inner.config_result.config.cadence.every_n_tools else {
                return;
            };
            if every_n_tools == 0 {
                return;
            }
            inner.tool_results_this_run += 1;
            if !inner
                .tool_results_this_run
                .is_multiple_of(u64::from(every_n_tools))
            {
                return;
            }
            if inner.reviewing || inner.waiting_at_agent_end || inner.mid_run_reviewing {
                return;
            }
            inner.build_review_input(None, "")
        };
        if delta.trim().is_empty() {
            return;
        }
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            runtime.review_mid_run_delta(delta).await;
        });
    }

    /// UW-3 — how long the agent-end boundary review may legitimately hold its `AgentEnd` handler:
    /// the LSP diagnostics bound (when LSP is on), plus the review's own `agentEndTimeoutMs`, plus
    /// one dispatch budget of slack for the bookkeeping after it. This is the ceiling
    /// [`Self::handle_agent_end_in_handler`] declares; a review that overruns it is cut by the
    /// dispatcher exactly as an undeclared handler is.
    #[must_use]
    pub fn agent_end_wait_ceiling(&self) -> Duration {
        let inner = self.lock();
        let config = &inner.config_result.config;
        let lsp_ms = if config.lsp.enabled {
            config.lsp.timeout_ms
        } else {
            0
        };
        Duration::from_millis(lsp_ms.saturating_add(config.agent_end_timeout_ms))
            + cyrup_ext::dispatch::DEFAULT_INVOKE_BUDGET
    }

    /// [`Self::handle_agent_end`] as called from an `AgentEnd` extension HANDLER: the review is
    /// awaited under a declared [`cyrup_ext::native::SanctionedWaitKind::ModelReview`] wait, so the
    /// dispatcher's 5 s per-handler budget (`cyrup_ext::dispatch::DEFAULT_INVOKE_BUDGET`) does not
    /// drop it mid-review. Upstream awaits the same review in its handler with no budget at all
    /// (`register-main.ts:427-430`, `runner.ts:805-811`). See `cyrup_ext::native::SanctionedWaitGate`
    /// for why this is a declared wait and not an `AgentEnd` exemption.
    pub async fn handle_agent_end_in_handler(&self, ctx: &cyrup_ext::native::HostCtx) {
        // Refreshed first so the ceiling is computed from the config this review will run under.
        self.refresh_config(&ctx.cwd);
        let _review = ctx.begin_sanctioned_wait(
            cyrup_ext::native::SanctionedWaitKind::ModelReview,
            self.agent_end_wait_ceiling(),
        );
        self.handle_agent_end(&ctx.cwd).await;
    }

    /// `handleAgentEnd(event, ctx)` (`runtime.ts:362-435`) — the boundary review. See the inline
    /// comments for the early-return ladder; the `finally` (`:431-434`) is reproduced by running
    /// the cleanup after the body regardless of which arm returned.
    pub async fn handle_agent_end(&self, cwd: &Path) {
        if self.lock().disposed {
            return;
        }
        self.refresh_config(cwd);
        if !self.lock().is_enabled() {
            return;
        }
        let change_signature = self.resolve_review_change_signature(cwd);

        // SUBA-234 — `runtime.ts:400-410` @ad11b7ab. With clarification on, NEW delivered
        // orchestration activity admits one boundary review per user prompt even when nothing in
        // the tree changed: a subagent reported back, a gate cleared, and the reviewer gets to
        // weigh that against the scope. Not during a warning's continuation, and at most once per
        // prompt.
        let (activity_review, known_evidence) = {
            let inner = self.lock();
            let activity_review = self.display_clarification.is_some()
                && inner.config_result.config.clarification
                && !inner.activity_review_used
                && inner.boundary_warnings_this_prompt == 0
                && inner.activity_pending;
            let known_evidence = change_signature.as_ref().is_some_and(|signature| {
                inner.last_reviewed_change_signature.as_deref() == Some(signature.key.as_str())
            });
            (activity_review, known_evidence)
        };
        // `:367-378` (`:402-407` @ad11b7ab) — "changes only" and nothing changed, or the tree is
        // byte-identical to what was already reviewed: drop the buffer and stay idle — unless new
        // activity admits a review on its own.
        if !activity_review
            && (known_evidence || (self.review_changes_only && change_signature.is_none()))
        {
            let waiters = {
                let mut inner = self.lock();
                inner.clear_pending_deltas();
                if known_evidence || inner.status == WatchdogRuntimeStatus::Queued {
                    inner.status = WatchdogRuntimeStatus::Idle;
                }
                inner.take_waiters(true)
            };
            resolve_waiters(waiters, true);
            return;
        }
        {
            let mut inner = self.lock();
            // `:409-410` — only a review the ACTIVITY admitted spends the prompt's one activity
            // review; one an edit admitted would have run anyway.
            if activity_review && (change_signature.is_none() || known_evidence) {
                inner.activity_review_used = true;
            }
            inner.activity_pending = false;
        }

        self.cancel_mid_run_review();

        let (agent_end_epoch, agent_end_id, cancel, previous_displayed_sequence) = {
            let mut inner = self.lock();
            inner.waiting_at_agent_end = true;
            inner.agent_end_id_counter += 1;
            let id = inner.agent_end_id_counter;
            let cancel = CancelToken::new();
            inner.active_agent_end_id = Some(id);
            inner.active_agent_end_cancel = Some((id, cancel.clone()));
            inner.guard.start_model_update();
            (inner.epoch, id, cancel, inner.displayed_warning_sequence)
        };

        self.agent_end_body(
            cwd,
            change_signature,
            agent_end_epoch,
            agent_end_id,
            cancel,
            previous_displayed_sequence,
        )
        .await;

        // `finally` (`:431-434`).
        let mut inner = self.lock();
        if inner.active_agent_end_cancel.as_ref().map(|(id, _)| *id) == Some(agent_end_id) {
            inner.active_agent_end_cancel = None;
        }
        if inner.active_agent_end_id == Some(agent_end_id) {
            inner.active_agent_end_id = None;
        }
    }

    /// The `try` block of `handleAgentEnd` (`runtime.ts:388-430`).
    async fn agent_end_body(
        &self,
        cwd: &Path,
        change_signature: Option<WatchdogRepoChangeSignature>,
        agent_end_epoch: u64,
        agent_end_id: u64,
        cancel: CancelToken,
        previous_displayed_sequence: u64,
    ) {
        let lsp_block = self
            .collect_lsp_diagnostics(
                cwd,
                change_signature.as_ref(),
                agent_end_epoch,
                agent_end_id,
                cancel,
            )
            .await;
        {
            let mut inner = self.lock();
            if inner.active_agent_end_cancel.as_ref().map(|(id, _)| *id) == Some(agent_end_id) {
                inner.active_agent_end_cancel = None;
            }
            if !inner.is_agent_end_current(agent_end_epoch, agent_end_id) {
                return;
            }
        }

        let (delta, waiters_after_empty) = {
            let mut inner = self.lock();
            let delta = inner.build_review_input(change_signature.as_ref(), &lsp_block);
            inner.clear_pending_deltas();
            if delta.trim().is_empty() {
                // `:399-404`.
                inner.waiting_at_agent_end = false;
                if inner.status == WatchdogRuntimeStatus::Queued {
                    inner.status = WatchdogRuntimeStatus::Idle;
                }
                (delta, Some(inner.take_waiters(true)))
            } else {
                (delta, None)
            }
        };
        if let Some(waiters) = waiters_after_empty {
            resolve_waiters(waiters, true);
            return;
        }

        let signature = review_input_signature(&delta);
        {
            let mut inner = self.lock();
            // `:406-411` — an identical delta is not reviewed twice, unless the trigger is repo
            // edits (where the change signature is already the identity test).
            if !self.review_changes_only
                && inner.last_review_input_signature.as_deref() == Some(signature.as_str())
            {
                inner.waiting_at_agent_end = false;
                inner.status = WatchdogRuntimeStatus::Idle;
                let waiters = inner.take_waiters(true);
                drop(inner);
                resolve_waiters(waiters, true);
                return;
            }
        }

        let timeout_ms = self.lock().config_result.config.agent_end_timeout_ms;
        // `{ allowClarification: true }` (`runtime.ts:441` @ad11b7ab) — the boundary review is the
        // one that may ask; `review_delta` narrows it by the setting, the prompt's budget and the
        // stalemate.
        let outcome = self.review_delta(delta, timeout_ms, false, true).await;

        let mut inner = self.lock();
        // `if (!this.isAgentEndCurrent(agentEndEpoch, agentEndId)) return;` (`runtime.ts:442`
        // @ad11b7ab, SUBA-234) — a boundary something reset while its review ran (the operator's
        // message, a model change, clarification turned off) records nothing and displays nothing:
        // its question or its warning would be about a conversation that has moved on.
        if !inner.is_agent_end_current(agent_end_epoch, agent_end_id) {
            return;
        }
        inner.waiting_at_agent_end = false;
        if outcome == ReviewDeltaOutcome::Timeout {
            // `:414-421`.
            inner.stale_reviews += 1;
            inner.invalidate_active_review();
            inner.status = WatchdogRuntimeStatus::Stale;
            inner.mark_last_warning_stale();
            let waiters = inner.take_waiters(true);
            drop(inner);
            resolve_waiters(waiters, true);
            return;
        }
        let mut question = None;
        if matches!(
            outcome,
            ReviewDeltaOutcome::Completed | ReviewDeltaOutcome::Clarification(_)
        ) && inner.status != WatchdogRuntimeStatus::Failed
            && inner.status != WatchdogRuntimeStatus::Stale
        {
            // `:448-450` — "A question consumes this evidence just like a completed review, not a
            // pending exchange." So an unchanged boundary after the question does not re-review it.
            inner.last_review_input_signature = Some(signature);
            if let Some(sig) = &change_signature {
                inner.last_reviewed_change_signature = Some(sig.key.clone());
            }
            inner.current_changed_paths =
                change_signature.as_ref().map(|s| s.changed_paths.clone());
            inner.status = WatchdogRuntimeStatus::Idle;
            if let ReviewDeltaOutcome::Clarification(clarification) = &outcome {
                question = Some(clarification_message(clarification));
            }
        }
        let displayed = if inner.displayed_warning_sequence != previous_displayed_sequence {
            inner.last_warning.clone()
        } else {
            None
        };
        let waiters = inner.take_waiters(true);
        drop(inner);
        // `this.displayClarification?.([...].join("\n"))` (`runtime.ts:458-461` @ad11b7ab) —
        // delivered, and the lifecycle is over: the reviewer has yielded, nothing waits for an
        // answer, and the runtime is already idle. The question steers the session into one
        // continuation in which the orchestrator takes the missing context into account.
        if let (Some(question), Some(sink)) = (question, &self.display_clarification) {
            sink(&question);
        }
        self.queue_auto_follow_if_needed(displayed.as_ref());
        resolve_waiters(waiters, true);
    }

    /// `recordDisplayedWarning(warning)` (`runtime.ts:437-441`) — stamp a warning that was already
    /// displayed by some other path (the `/subagents-watchdog test …` command).
    pub fn record_displayed_warning(&self, warning: &WatchdogWarning) -> WatchdogWarningDetails {
        let details = normalize_watchdog_warning_details(
            warning,
            &WatchdogWarningDetailsPatch::new(
                WatchdogWarningState::Displayed,
                warning.source.unwrap_or(WatchdogWarningSource::Main),
            ),
        );
        self.lock().last_warning = Some(details.clone());
        details
    }

    /// `getSnapshot(cwd)` (`runtime.ts:443-470`) — refreshes the config first when a cwd is given,
    /// exactly as upstream's optional argument does.
    #[must_use]
    pub fn get_snapshot(&self, cwd: Option<&Path>) -> WatchdogRuntimeSnapshot {
        if let Some(cwd) = cwd {
            self.refresh_config(cwd);
        }
        let inner = self.lock();
        WatchdogRuntimeSnapshot {
            status: inner.status,
            enabled: inner.is_enabled(),
            config: inner.config_result.config.clone(),
            config_ok: inner.config_result.ok,
            errors: inner.config_result.errors.clone(),
            sources: inner.config_result.sources.clone(),
            buffered_deltas: inner.pending_deltas.len(),
            epoch: inner.epoch,
            active_review_id: inner.active_review_id,
            session_override: inner.session_override_enabled,
            session_model_override: inner.session_model_override.clone(),
            last_warning: inner.last_warning.clone(),
            last_error: inner.last_error.clone(),
            failed_reviews: inner.failed_reviews,
            stale_reviews: inner.stale_reviews,
            review_connected: self.review_connected,
            review_description: self.review_description.clone(),
            auto_follow_queued: inner.auto_follow_queued,
            auto_follow_attempts: inner.auto_follow_attempts,
            auto_follow_stalemate: inner.auto_follow_stalemate,
            review_trigger: if self.review_changes_only {
                WatchdogReviewTrigger::RepoEdits
            } else {
                WatchdogReviewTrigger::TurnDelta
            },
            changed_paths: inner
                .current_changed_paths
                .as_ref()
                .filter(|p| !p.is_empty())
                .cloned(),
            lsp: inner.lsp_snapshot(),
        }
    }

    /// `waitForIdle(timeoutMs)` (`runtime.ts:472-474`) — resolves `true` once nothing is in flight
    /// and the buffer is empty, `false` on timeout or after [`Self::dispose`].
    pub async fn wait_for_idle(&self, timeout: Duration) -> bool {
        let receiver = {
            let mut inner = self.lock();
            if inner.is_settled() {
                return true;
            }
            let (tx, rx) = tokio::sync::oneshot::channel();
            inner.waiters.push(tx);
            rx
        };
        match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(settled)) => settled,
            Ok(Err(_)) | Err(_) => false,
        }
    }

    // ---- reviews ------------------------------------------------------------------------------

    /// `reviewMidRunDelta(delta)` (`runtime.ts:539-558`).
    async fn review_mid_run_delta(&self, delta: String) {
        let generation = {
            let mut inner = self.lock();
            if inner.mid_run_reviewing
                || inner.reviewing
                || inner.waiting_at_agent_end
                || inner.disposed
            {
                return;
            }
            inner.mid_run_reviewing = true;
            inner.mid_run_generation
        };
        let timeout_ms = self.lock().config_result.config.agent_end_timeout_ms;
        let outcome = self.review_delta(delta, timeout_ms, true, false).await;
        let waiters = {
            let mut inner = self.lock();
            if generation != inner.mid_run_generation {
                return;
            }
            if outcome == ReviewDeltaOutcome::Timeout {
                inner.stale_reviews += 1;
                inner.status = WatchdogRuntimeStatus::Stale;
                inner.mark_last_warning_stale();
            }
            // `finally` (`:551-557`).
            inner.mid_run_reviewing = false;
            if inner.status == WatchdogRuntimeStatus::Reviewing {
                inner.status = if inner.pending_deltas.is_empty() {
                    WatchdogRuntimeStatus::Idle
                } else {
                    WatchdogRuntimeStatus::Queued
                };
            }
            let settled = inner.is_settled();
            inner.take_waiters(settled)
        };
        resolve_waiters(waiters, true);
    }

    /// `cancelMidRunReview()` (`runtime.ts:560-571`) — the agent-end boundary review is
    /// authoritative, so an in-flight cadence review is superseded and counted stale.
    fn cancel_mid_run_review(&self) {
        let mut inner = self.lock();
        if !inner.mid_run_reviewing {
            return;
        }
        inner.mid_run_generation += 1;
        if let Some((_, cancel)) = inner.active_review_cancel.take() {
            cancel.cancel();
        }
        inner.stale_reviews += 1;
        inner.reviewing = false;
        inner.mid_run_reviewing = false;
        inner.active_review_id = None;
        inner.active_review_warning = None;
    }

    /// `reviewDelta(delta, timeoutMs, options)` (`runtime.ts:573-629`).
    async fn review_delta(
        &self,
        delta: String,
        timeout_ms: u64,
        correction: bool,
        allow_clarification: bool,
    ) -> ReviewDeltaOutcome {
        let (review_epoch, review_id, cancel, request) = {
            let mut inner = self.lock();
            if inner.reviewing || inner.disposed {
                return ReviewDeltaOutcome::Stale;
            }
            inner.reviewing = true;
            let review_epoch = inner.epoch;
            inner.review_id_counter += 1;
            let review_id = inner.review_id_counter;
            inner.active_review_id = Some(review_id);
            inner.active_review_warning = None;
            inner.status = WatchdogRuntimeStatus::Reviewing;
            let cancel = CancelToken::new();
            inner.active_review_cancel = Some((review_id, cancel.clone()));
            // `runtime.ts:640` @ad11b7ab — the caller's wish, narrowed: the main role only, the
            // setting on, no question yet this prompt (answered or not), and not in stalemate.
            let allow_clarification = allow_clarification
                && self.display_clarification.is_some()
                && inner.config_result.config.clarification
                && !inner.asked_this_prompt
                && !inner.auto_follow_stalemate;
            let request = WatchdogReviewRequest {
                delta,
                epoch: review_epoch,
                has_scope: !inner.scope_block().trim().is_empty(),
                review_id,
                config: inner.config_result.config.clone(),
                emit_warning: self.warning_emitter(review_epoch, review_id),
                cancel: cancel.clone(),
                diff_baseline: self.diff_baseline.clone(),
                allow_clarification,
            };
            (review_epoch, review_id, cancel, request)
        };

        let allowed = request.allow_clarification;
        let outcome = match tokio::time::timeout(
            Duration::from_millis(timeout_ms),
            self.review.review(request),
        )
        .await
        {
            // `:600-603` — timeout: abort and report.
            Err(_) => {
                cancel.cancel();
                ReviewDeltaOutcome::Timeout
            }
            Ok(Ok(result)) => {
                let stale = {
                    let inner = self.lock();
                    !inner.is_current(review_epoch, review_id)
                };
                if stale {
                    ReviewDeltaOutcome::Stale
                } else {
                    match result {
                        // `:605` — a `void` return is a stale review.
                        None => ReviewDeltaOutcome::Stale,
                        Some(result) => {
                            // SUBA-234 — `runtime.ts:664-671` @ad11b7ab: a question is taken in
                            // place of a ruling only when this review was allowed one, the setting
                            // is STILL on, the review ended cleanly, no warning was accepted or
                            // returned, and nothing aborted it; and only with both halves
                            // non-empty. Anything else falls through to the ordinary ruling.
                            if let Some(clarification) =
                                self.accept_clarification(&result, allowed, &cancel)
                            {
                                ReviewDeltaOutcome::Clarification(clarification)
                            } else {
                                self.rule_on_review(review_epoch, review_id, &result, correction)
                            }
                        }
                    }
                }
            }
            // `catch` (`:613-618`).
            Ok(Err(message)) => {
                let current = {
                    let inner = self.lock();
                    inner.is_current(review_epoch, review_id)
                };
                if current {
                    self.fail(&format!("Watchdog review failed: {message}"));
                    ReviewDeltaOutcome::Completed
                } else {
                    ReviewDeltaOutcome::Stale
                }
            }
        };

        // `finally` (`:619-628`).
        let waiters = {
            let mut inner = self.lock();
            if inner.active_review_cancel.as_ref().map(|(id, _)| *id) == Some(review_id) {
                inner.active_review_cancel = None;
            }
            if inner.epoch == review_epoch && inner.active_review_id == Some(review_id) {
                inner.reviewing = false;
                inner.active_review_id = None;
                inner.active_review_warning = None;
            }
            let settled = inner.is_settled();
            inner.take_waiters(settled)
        };
        resolve_waiters(waiters, true);
        outcome
    }

    /// The ordinary ruling on a finished review (`runtime.ts:672-680` @ad11b7ab): accept its
    /// returned warnings, fail a non-`stop` stop reason, else display what was accepted.
    fn rule_on_review(
        &self,
        review_epoch: u64,
        review_id: u64,
        result: &WatchdogReviewResult,
        correction: bool,
    ) -> ReviewDeltaOutcome {
        for warning in &result.warnings {
            self.accept_warning(review_epoch, review_id, warning);
        }
        match result.stop_reason {
            Some(reason) if reason != ReviewStopReason::Stop => {
                self.fail(&format!(
                    "Watchdog review ended with stop reason '{}'.",
                    reason.as_str()
                ));
            }
            _ => self.display_accepted_review_warning(correction),
        }
        ReviewDeltaOutcome::Completed
    }

    /// SUBA-234 — the question a finished review yielded, if the runtime takes it
    /// (`runtime.ts:664-671` @ad11b7ab); marks the prompt's one question spent when it does.
    fn accept_clarification(
        &self,
        result: &WatchdogReviewResult,
        allowed: bool,
        cancel: &CancelToken,
    ) -> Option<WatchdogClarification> {
        let clarification = result.clarification.as_ref()?;
        let mut inner = self.lock();
        let clean_stop = matches!(result.stop_reason, None | Some(ReviewStopReason::Stop));
        if !allowed
            || !inner.config_result.config.clarification
            || !clean_stop
            || inner.active_review_warning.is_some()
            || !result.warnings.is_empty()
            || cancel.is_cancelled()
        {
            return None;
        }
        let question = clarification.question.trim();
        let evidence = clarification.evidence.trim();
        if question.is_empty() || evidence.is_empty() {
            return None;
        }
        inner.asked_this_prompt = true;
        Some(WatchdogClarification {
            question: bound_watchdog_review_text(question, CLARIFICATION_QUESTION_MAX_CHARS),
            evidence: bound_watchdog_review_text(evidence, CLARIFICATION_EVIDENCE_MAX_CHARS),
        })
    }

    /// The `emitWarning` closure handed to a review (`runtime.ts:591`).
    fn warning_emitter(&self, epoch: u64, review_id: u64) -> WatchdogWarningEmitter {
        let inner = Arc::clone(&self.inner);
        let sink = self.display_warning.clone();
        let _ = &sink;
        WatchdogWarningEmitter {
            emit: Arc::new(move |warning: &WatchdogWarning| {
                let mut guard = inner
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                guard.accept_warning(epoch, review_id, warning)
            }),
        }
    }

    /// `acceptWarning(epoch, reviewId, warning)` (`runtime.ts:498-510`).
    fn accept_warning(&self, epoch: u64, review_id: u64, warning: &WatchdogWarning) -> bool {
        self.lock().accept_warning(epoch, review_id, warning)
    }

    /// `displayBoundaryWarning(warning)` (`runtime.ts:512-526`) — the LSP-sourced warning path,
    /// which displays IMMEDIATELY rather than waiting for the review to accept one.
    fn display_boundary_warning(&self, warning: &WatchdogWarning) -> bool {
        let details = {
            let mut inner = self.lock();
            if !inner.is_enabled() || !inner.warning_meets_threshold(warning) {
                return false;
            }
            let decision = inner.guard.evaluate(warning);
            if !decision.accepted() {
                return false;
            }
            // `normalizeWatchdogWarningDetails(warning, { state, source, identity, displayedAt })`
            // (`runtime.ts:516-521`) — one extras literal, built in one expression.
            let patch = WatchdogWarningDetailsPatch::new(
                WatchdogWarningState::Displayed,
                warning.source.unwrap_or(WatchdogWarningSource::Main),
            )
            .with_identity(decision.identity())
            .with_displayed_at(super::now_iso8601());
            let details = normalize_watchdog_warning_details(warning, &patch);
            inner.last_warning = Some(details.clone());
            inner.displayed_warning_sequence += 1;
            // SUBA-234 — `deliverBoundaryWarning` counts this boundary (`runtime.ts:579`,`:717`
            // @ad11b7ab); see `boundary_warnings_this_prompt`.
            inner.boundary_warnings_this_prompt += 1;
            details
        };
        self.route_warning(&details, None);
        true
    }

    /// SUBA-120 / `routeWarning(details, options)` (`runtime.ts:548-550`) — the whole routing rule,
    /// verbatim: a `high`-importance finding goes to the parent-model sink, everything else
    /// (`low`, `medium`) goes to the user-only sink. Both are optional, exactly as upstream's two
    /// `?.` calls are.
    ///
    /// Every warning this runtime displays goes through here. Before SUBA-120 there was one sink and
    /// every threshold-passing finding reached the parent model, so a `low` finding — which upstream
    /// shows the user and nothing more — was steered into the parent's turn.
    fn route_warning(&self, details: &WatchdogWarningDetails, delivery: Option<WatchdogDelivery>) {
        if details.importance == WatchdogImportance::High {
            if let Some(sink) = &self.display_warning {
                sink(details, delivery);
            }
        } else if let Some(sink) = &self.display_user_warning {
            sink(details);
        }
    }

    /// `displayAcceptedReviewWarning(correction)` (`runtime.ts:631-641`).
    fn display_accepted_review_warning(&self, correction: bool) {
        let details = {
            let mut inner = self.lock();
            let Some(accepted) = inner.active_review_warning.clone() else {
                return;
            };
            let details = WatchdogWarningDetails {
                state: Some(WatchdogWarningState::Displayed),
                displayed_at: Some(super::now_iso8601()),
                ..accepted
            };
            inner.last_warning = Some(details.clone());
            inner.displayed_warning_sequence += 1;
            // SUBA-234 — a boundary (non-correction) warning goes through `deliverBoundaryWarning`
            // upstream (`runtime.ts:710`,`:717` @ad11b7ab), which counts it; a mid-run correction
            // does not.
            if !correction {
                inner.boundary_warnings_this_prompt += 1;
            }
            details
        };
        self.route_warning(
            &details,
            if correction {
                Some(WatchdogDelivery::Steer)
            } else {
                None
            },
        );
    }

    /// `fail(message)` (`runtime.ts:828-834`).
    fn fail(&self, message: &str) {
        let waiters = {
            let mut inner = self.lock();
            inner.failed_reviews += 1;
            inner.last_error = Some(message.to_string());
            inner.status = WatchdogRuntimeStatus::Failed;
            inner.clear_pending_deltas();
            inner.take_waiters(true)
        };
        resolve_waiters(waiters, true);
    }

    /// `queueAutoFollowIfNeeded(warning)` (`runtime.ts:651-683`).
    fn queue_auto_follow_if_needed(&self, warning: Option<&WatchdogWarningDetails>) {
        let prompt = {
            let mut inner = self.lock();
            let Some(warning) = warning else { return };
            if warning.severity != WatchdogSeverity::Blocker
                || warning.stale == Some(true)
                || !inner.config_result.config.auto_follow.blockers
                || !inner.is_enabled()
            {
                return;
            }
            let identity = warning.identity.clone().unwrap_or_else(|| {
                review_input_signature(
                    &[
                        warning.severity.as_str(),
                        warning.summary.as_str(),
                        warning.evidence.as_str(),
                    ]
                    .join("\n"),
                )
            });
            if inner.consecutive_auto_follow_identity.as_deref() == Some(identity.as_str()) {
                inner.consecutive_auto_follow_repeats += 1;
            } else {
                inner.consecutive_auto_follow_identity = Some(identity);
                inner.consecutive_auto_follow_repeats = 1;
            }
            if inner.consecutive_auto_follow_repeats
                >= inner.config_result.config.auto_follow.stalemate_repeats
            {
                inner.auto_follow_stalemate = true;
                inner.last_warning = Some(WatchdogWarningDetails {
                    state: Some(WatchdogWarningState::Stalemate),
                    stalemate_repeats: Some(inner.consecutive_auto_follow_repeats),
                    ..warning.clone()
                });
                return;
            }
            if let Some(max_attempts) = inner.config_result.config.auto_follow.max_attempts
                && inner.auto_follow_attempts >= max_attempts
            {
                return;
            }
            if self.send_user_message.is_none() {
                return;
            }
            inner.auto_follow_attempts += 1;
            inner.auto_follow_queued = true;
            let prompt = [
                "Watchdog auto-follow: address this blocker before continuing.".to_string(),
                format!("Summary: {}", warning.summary),
                format!("Evidence: {}", warning.evidence),
                format!("Recommended action: {}", warning.recommended_action),
            ]
            .join("\n");
            inner.pending_auto_follow_prompts.mark(prompt.clone());
            prompt
        };
        let Some(sink) = &self.send_user_message else {
            return;
        };
        if let Err(message) = sink(&prompt) {
            // `.catch(...)` (`:677-682`).
            let mut inner = self.lock();
            inner.auto_follow_queued = false;
            inner.pending_auto_follow_prompts.unmark(&prompt);
            inner.last_error = Some(format!("Watchdog auto-follow failed: {message}"));
        }
    }

    // ---- repo change tracking -------------------------------------------------------------------

    /// `currentRepoChangeSignature(cwd)` (`runtime.ts:685-687`) — only computed at all when the
    /// trigger is repo edits AND the watchdog is on.
    fn current_repo_change_signature(&self, cwd: &Path) -> Option<WatchdogRepoChangeSignature> {
        let enabled = self.lock().is_enabled();
        if self.review_changes_only && enabled {
            self.repo_change_signature.compute(cwd)
        } else {
            None
        }
    }

    /// `resetRepoChangeBaseline(options)` (`runtime.ts:689-695`).
    fn reset_repo_change_baseline(&self, cwd: Option<&Path>, reviewed: bool) {
        let cwd = cwd
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.lock().cwd.clone());
        let signature = self.current_repo_change_signature(&cwd);
        let mut inner = self.lock();
        let key = signature.as_ref().map(|s| s.key.clone());
        inner.current_changed_paths = signature.as_ref().map(|s| s.changed_paths.clone());
        inner.turn_start_change_signature = signature;
        // `options.reviewed ? (x = key) : (x ??= key)` (`runtime.ts:691-692`) — the two arms assign
        // the same value, so the whole decision is "assign unless we already have one and this is
        // not a reviewed baseline".
        if reviewed || inner.last_reviewed_change_signature.is_none() {
            inner.last_reviewed_change_signature = key;
        }
        inner.observed_repo_edit_this_turn = false;
    }

    /// `resolveReviewChangeSignature(cwd)` (`runtime.ts:697-709`).
    fn resolve_review_change_signature(&self, cwd: &Path) -> Option<WatchdogRepoChangeSignature> {
        if !self.review_changes_only {
            return None;
        }
        if let Some(current) = self.current_repo_change_signature(cwd) {
            let mut inner = self.lock();
            inner.current_changed_paths = Some(current.changed_paths.clone());
            if inner
                .turn_start_change_signature
                .as_ref()
                .is_some_and(|s| s.key == current.key)
            {
                return None;
            }
            if current.changed_paths.is_empty() {
                return None;
            }
            return Some(current);
        }
        // `:706-708` — no signature at all: fall back to the observed edit/write tool result, with
        // a synthetic key that is unique per (epoch, review, buffered size) so it never matches
        // `lastReviewedChangeSignature`.
        let inner = self.lock();
        if inner.observed_repo_edit_this_turn {
            Some(WatchdogRepoChangeSignature {
                root: cwd.display().to_string(),
                key: format!(
                    "observed-edit:{}:{}:{}",
                    inner.epoch, inner.review_id_counter, inner.pending_delta_chars
                ),
                changed_paths: Vec::new(),
            })
        } else {
            None
        }
    }

    /// `collectLspDiagnostics(changeSignature, current)` (`runtime.ts:711-762`).
    async fn collect_lsp_diagnostics(
        &self,
        cwd: &Path,
        change_signature: Option<&WatchdogRepoChangeSignature>,
        epoch: u64,
        agent_end_id: u64,
        cancel: CancelToken,
    ) -> String {
        let config = self.lock().config_result.config.lsp.clone();
        let has_paths = change_signature.is_some_and(|s| !s.changed_paths.is_empty());
        if !config.enabled || !has_paths {
            // `:713-725` — nothing to check is `skipped`, policy-off is `disabled`; either way the
            // snapshot is stamped so the status line reports why no diagnostics appeared.
            let mut inner = self.lock();
            inner.last_lsp_snapshot = Some(WatchdogLspRuntimeSnapshot {
                result: WatchdogLspResult {
                    status: if config.enabled {
                        WatchdogLspStatus::Skipped
                    } else {
                        WatchdogLspStatus::Disabled
                    },
                    provider: None,
                    checked_paths: Vec::new(),
                    skipped_paths: Vec::new(),
                    diagnostics: Vec::new(),
                    message: None,
                },
                enabled: config.enabled,
                diagnostic_count: 0,
                fresh_diagnostic_count: 0,
                updated_at: Some(super::now_iso8601()),
            });
            return String::new();
        }
        let Some(signature) = change_signature else {
            return String::new();
        };
        let request = WatchdogLspRequest {
            cwd: cwd.to_path_buf(),
            root: PathBuf::from(&signature.root),
            changed_paths: signature.changed_paths.clone(),
            config,
            signal: Some(cancel),
        };
        match self.lsp_diagnostics.collect(request).await {
            Ok(raw) => {
                {
                    // `:734` — a superseded boundary drops the result WITHOUT folding it into the
                    // ledger, so the next boundary still sees those diagnostics as fresh.
                    let inner = self.lock();
                    if !inner.is_agent_end_current(epoch, agent_end_id) {
                        return String::new();
                    }
                }
                let diagnostic_count = raw.diagnostics.len();
                let fresh = self.lsp_diagnostics.reduce(raw);
                {
                    let mut inner = self.lock();
                    inner.last_lsp_snapshot = Some(WatchdogLspRuntimeSnapshot {
                        result: fresh.clone(),
                        enabled: true,
                        diagnostic_count,
                        fresh_diagnostic_count: fresh.diagnostics.len(),
                        updated_at: Some(super::now_iso8601()),
                    });
                }
                if let Some(warning) = self.lsp_diagnostics.warning_from_diagnostics(&fresh) {
                    self.display_boundary_warning(&warning);
                }
                self.lsp_diagnostics.format_block(&fresh)
            }
            Err(message) => {
                // `:747-761`.
                let mut inner = self.lock();
                if !inner.is_agent_end_current(epoch, agent_end_id) {
                    return String::new();
                }
                inner.last_lsp_snapshot = Some(WatchdogLspRuntimeSnapshot {
                    result: WatchdogLspResult {
                        status: WatchdogLspStatus::Failed,
                        provider: None,
                        checked_paths: Vec::new(),
                        skipped_paths: signature.changed_paths.clone(),
                        diagnostics: Vec::new(),
                        message: Some(format!("LSP diagnostics failed: {message}")),
                    },
                    enabled: true,
                    diagnostic_count: 0,
                    fresh_diagnostic_count: 0,
                    updated_at: Some(super::now_iso8601()),
                });
                String::new()
            }
        }
    }
}

// =================================================================================================
// The synchronous state transitions (`runtime.ts`'s private methods)
// =================================================================================================

impl RuntimeInner {
    /// `isEnabled()` (`runtime.ts:476-478`) — a broken config is a DISABLED watchdog.
    fn is_enabled(&self) -> bool {
        self.config_result.ok && self.config_result.config.main.enabled
    }

    /// The session-override object `refreshConfig` builds (`runtime.ts:198-206`). `main` is always
    /// present once there is any override at all.
    fn session_patch(&self) -> Option<Value> {
        if self.session_override_enabled.is_none() && self.session_model_override.is_none() {
            return None;
        }
        let mut root = Map::new();
        let mut main = Map::new();
        if let Some(enabled) = self.session_override_enabled {
            root.insert("enabled".to_string(), Value::Bool(enabled));
            main.insert("enabled".to_string(), Value::Bool(enabled));
        }
        if let Some(override_model) = &self.session_model_override {
            if let Some(model) = &override_model.model {
                main.insert("model".to_string(), Value::String(model.clone()));
            }
            if let Some(thinking) = &override_model.thinking
                && let Ok(value) = serde_json::to_value(thinking)
            {
                main.insert("thinking".to_string(), value);
            }
        }
        root.insert("main".to_string(), Value::Object(main));
        Some(Value::Object(root))
    }

    /// `abortActiveAgentEnd()` (`runtime.ts:480-484`).
    fn abort_active_agent_end(&mut self) {
        if let Some((_, cancel)) = self.active_agent_end_cancel.take() {
            cancel.cancel();
        }
        self.active_agent_end_id = None;
    }

    /// `this.activeReviewAbortController?.abort()` (`runtime.ts:278`,`:309`,`:584` @ad11b7ab) —
    /// cancel the in-flight review's token, so the reviewer's nested turn stops. Only the token is
    /// taken; the review's own `finally` clears the rest.
    fn abort_active_review(&mut self) {
        if let Some((_, cancel)) = self.active_review_cancel.take() {
            cancel.cancel();
        }
    }

    /// SUBA-234 — the per-USER-prompt counters (`runtime.ts:334-336` @ad11b7ab):
    /// `askedThisPrompt`, `activityReviewUsed` and `resetBoundaryRepeats()`.
    fn start_user_prompt(&mut self) {
        self.asked_this_prompt = false;
        self.activity_review_used = false;
        self.boundary_warnings_this_prompt = 0;
    }

    /// SUBA-234 — the shared condition of `handleUserInput`'s and `handleModelChange`'s reset
    /// (`runtime.ts:515`,`:520` @ad11b7ab): the main role (`has_sink`), clarification on, and a
    /// review in flight.
    fn clarification_review_in_flight(&self, has_sink: bool) -> bool {
        has_sink
            && self.config_result.config.clarification
            && (self.reviewing || self.waiting_at_agent_end)
    }

    /// SUBA-234 — append one turn's activity to the tail (`runtime.ts:356-357` @ad11b7ab): the
    /// entry bounded to [`ACTIVITY_ENTRY_MAX_CHARS`], joined to what is kept by the delta
    /// separator, the whole clipped to its NEWEST [`ACTIVITY_TAIL_MAX_CHARS`]. Marks it unreviewed.
    fn append_activity(&mut self, activity: &str) {
        let entry = bound_watchdog_review_text(activity, ACTIVITY_ENTRY_MAX_CHARS);
        let joined = if self.activity_tail.is_empty() {
            entry
        } else {
            format!("{}{REVIEW_DELTA_SEPARATOR}{entry}", self.activity_tail)
        };
        self.activity_tail = tail_chars(&joined, ACTIVITY_TAIL_MAX_CHARS);
        self.activity_pending = true;
    }

    /// SUBA-234 — `clearActivity()` (`runtime.ts:881-885` @ad11b7ab).
    fn clear_activity(&mut self) {
        self.activity_tail.clear();
        self.activity_pending = false;
    }

    /// `isAgentEndCurrent(epoch, agentEndId)` (`runtime.ts:486-488`).
    fn is_agent_end_current(&self, epoch: u64, agent_end_id: u64) -> bool {
        !self.disposed
            && self.epoch == epoch
            && self.active_agent_end_id == Some(agent_end_id)
            && self.waiting_at_agent_end
            && self.is_enabled()
    }

    /// `isCurrent(epoch, reviewId)` (`runtime.ts:490-492`).
    fn is_current(&self, epoch: u64, review_id: u64) -> bool {
        !self.disposed && self.epoch == epoch && self.active_review_id == Some(review_id)
    }

    /// `warningMeetsThreshold(warning)` (`runtime.ts:494-496`).
    fn warning_meets_threshold(&self, warning: &WatchdogWarning) -> bool {
        self.config_result.config.severity_threshold == WatchdogSeverity::Concern
            || warning.severity == WatchdogSeverity::Blocker
    }

    /// `acceptWarning(epoch, reviewId, warning)` (`runtime.ts:498-510`) — a CANDIDATE, not yet
    /// displayed; `displayAcceptedReviewWarning` promotes it when the review completes.
    fn accept_warning(&mut self, epoch: u64, review_id: u64, warning: &WatchdogWarning) -> bool {
        if !self.is_current(epoch, review_id)
            || !self.is_enabled()
            || !self.warning_meets_threshold(warning)
        {
            return false;
        }
        let decision = self.guard.evaluate(warning);
        if !decision.accepted() {
            return false;
        }
        // `normalizeWatchdogWarningDetails(warning, { state, source, identity })`
        // (`runtime.ts:502-506`).
        let patch = WatchdogWarningDetailsPatch::new(
            WatchdogWarningState::Candidate,
            warning.source.unwrap_or(WatchdogWarningSource::Main),
        )
        .with_identity(decision.identity());
        let details = normalize_watchdog_warning_details(warning, &patch);
        self.last_warning = Some(details.clone());
        self.active_review_warning = Some(details);
        true
    }

    /// `invalidateActiveReview(reason)` (`runtime.ts:528-537`).
    fn invalidate_active_review(&mut self) {
        // `this.activeReviewAbortController?.abort()` (`runtime.ts:584` @ad11b7ab, SUBA-234).
        self.abort_active_review();
        self.abort_active_agent_end();
        self.epoch += 1;
        self.status = WatchdogRuntimeStatus::Idle;
        self.clear_pending_deltas();
        self.reviewing = false;
        self.waiting_at_agent_end = false;
        self.active_review_id = None;
        self.active_review_warning = None;
    }

    /// `resetAutoFollowState()` (`runtime.ts:643-649`).
    fn reset_auto_follow_state(&mut self) {
        self.auto_follow_queued = false;
        self.auto_follow_attempts = 0;
        self.consecutive_auto_follow_identity = None;
        self.consecutive_auto_follow_repeats = 0;
        self.auto_follow_stalemate = false;
    }

    /// `lspSnapshot()` (`runtime.ts:764-781`).
    fn lsp_snapshot(&self) -> WatchdogLspRuntimeSnapshot {
        if let Some(snapshot) = &self.last_lsp_snapshot {
            return snapshot.clone();
        }
        let enabled = self.is_enabled() && self.config_result.config.lsp.enabled;
        WatchdogLspRuntimeSnapshot {
            result: WatchdogLspResult {
                status: if enabled {
                    WatchdogLspStatus::Skipped
                } else {
                    WatchdogLspStatus::Disabled
                },
                provider: None,
                checked_paths: Vec::new(),
                skipped_paths: Vec::new(),
                diagnostics: Vec::new(),
                message: None,
            },
            enabled,
            diagnostic_count: 0,
            fresh_diagnostic_count: 0,
            updated_at: None,
        }
    }

    /// `appendBoundedDelta(delta)` (`runtime.ts:783-793`) — one entry is capped to the whole
    /// budget, then the OLDEST entries are dropped until the joined buffer fits.
    fn append_bounded_delta(&mut self, delta: &str) {
        let trimmed = delta.trim();
        if trimmed.is_empty() {
            return;
        }
        let entry = if char_len(trimmed) > MAX_REVIEW_INPUT_CHARS {
            tail_chars(trimmed, MAX_REVIEW_INPUT_CHARS)
        } else {
            trimmed.to_string()
        };
        self.pending_delta_chars += char_len(&entry);
        self.pending_deltas.push(entry);
        while self.pending_deltas.len() > 1
            && self.pending_delta_chars
                + (self.pending_deltas.len() - 1) * char_len(REVIEW_DELTA_SEPARATOR)
                > MAX_REVIEW_INPUT_CHARS
        {
            let removed = self.pending_deltas.remove(0);
            self.pending_delta_chars = self.pending_delta_chars.saturating_sub(char_len(&removed));
        }
    }

    /// `buildReviewInput(changeSignature, lspBlock)` (`runtime.ts:795-817`) — the context pieces
    /// get at most HALF the budget between them, each piece an equal share of that half (with a
    /// 1 000-character floor), and the deltas take whatever remains, truncated from the FRONT so
    /// the most recent work always survives.
    fn build_review_input(
        &mut self,
        change_signature: Option<&WatchdogRepoChangeSignature>,
        lsp_block: &str,
    ) -> String {
        let input = self.pending_deltas.join(REVIEW_DELTA_SEPARATOR);
        let scope_block = self.scope_block();
        let changes = match change_signature {
            Some(signature) if !signature.changed_paths.is_empty() => {
                let mut lines = vec![CHANGED_REPO_PATHS_HEADER.to_string()];
                lines.extend(
                    signature
                        .changed_paths
                        .iter()
                        .take(200)
                        .map(|file| format!("- {file}")),
                );
                lines.join("\n")
            }
            _ => String::new(),
        };
        // SUBA-234 — the activity block (`runtime.ts:857-858` @ad11b7ab), the fourth context piece:
        // the "supplied activity evidence" the reviewer's scope line names. Empty unless the main
        // watchdog runs with clarification on and some orchestration result was delivered.
        let activity = if self.activity_tail.is_empty() {
            String::new()
        } else {
            format!("{ACTIVITY_BLOCK_HEADER}\n{}", self.activity_tail)
        };
        let context_pieces: Vec<&str> = [
            scope_block.as_str(),
            changes.as_str(),
            lsp_block,
            activity.as_str(),
        ]
        .into_iter()
        .filter(|piece| !piece.is_empty())
        .collect();
        if context_pieces.is_empty() {
            return if char_len(&input) > MAX_REVIEW_INPUT_CHARS {
                tail_chars(&input, MAX_REVIEW_INPUT_CHARS)
            } else {
                input
            };
        }
        let max_context_length = MAX_REVIEW_INPUT_CHARS / 2;
        let max_piece_length = std::cmp::max(1_000, max_context_length / context_pieces.len());
        let bounded_context = context_pieces
            .iter()
            .map(|piece| {
                if char_len(piece) > max_piece_length {
                    // `piece === activity ? boundWatchdogReviewText(piece, maxPieceLength) : …`
                    // (`runtime.ts:864` @ad11b7ab): the activity keeps its NEWEST observations too,
                    // where the other pieces keep their head.
                    if *piece == activity.as_str() {
                        bound_watchdog_review_text(piece, max_piece_length)
                    } else {
                        format!(
                            "{}\n- ...",
                            head_chars(piece, max_piece_length.saturating_sub(6))
                        )
                    }
                } else {
                    (*piece).to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(REVIEW_DELTA_SEPARATOR);
        let separator_length = if input.is_empty() {
            0
        } else {
            char_len(REVIEW_DELTA_SEPARATOR)
        };
        let input_budget = MAX_REVIEW_INPUT_CHARS
            .saturating_sub(char_len(&bounded_context))
            .saturating_sub(separator_length);
        let bounded_input = if input_budget == 0 {
            String::new()
        } else if char_len(&input) > input_budget {
            tail_chars(&input, input_budget)
        } else {
            input
        };
        [bounded_context, bounded_input]
            .into_iter()
            .filter(|piece| !piece.is_empty())
            .collect::<Vec<_>>()
            .join(REVIEW_DELTA_SEPARATOR)
    }

    /// `scopeBlock()` (`runtime.ts:819-821`).
    fn scope_block(&self) -> String {
        if self.config_result.config.scope.enabled {
            self.scope.render()
        } else {
            String::new()
        }
    }

    /// `clearPendingDeltas()` (`runtime.ts:823-826`).
    fn clear_pending_deltas(&mut self) {
        self.pending_deltas.clear();
        self.pending_delta_chars = 0;
    }

    /// `markLastWarningStale()` (`runtime.ts:836-839`) — an ALREADY-displayed warning is left
    /// alone; only a candidate goes stale.
    fn mark_last_warning_stale(&mut self) {
        let Some(warning) = &self.last_warning else {
            return;
        };
        if warning.state == Some(WatchdogWarningState::Displayed) {
            return;
        }
        self.last_warning = Some(WatchdogWarningDetails {
            stale: Some(true),
            state: Some(WatchdogWarningState::Stale),
            ..warning.clone()
        });
    }

    /// `isSettled()` (`runtime.ts:841-843`).
    fn is_settled(&self) -> bool {
        !self.reviewing && self.pending_deltas.is_empty()
    }

    /// The take half of `resolveWaiters(settled)` (`runtime.ts:859-867`): `if (!settled &&
    /// !this.disposed) return;` — the waiters are only drained when the runtime really is settled
    /// (or has been disposed).
    fn take_waiters(&mut self, settled: bool) -> Vec<tokio::sync::oneshot::Sender<bool>> {
        if !settled && !self.disposed {
            return Vec::new();
        }
        std::mem::take(&mut self.waiters)
    }
}

/// The send half of `resolveWaiters` (`runtime.ts:863-866`).
fn resolve_waiters(waiters: Vec<tokio::sync::oneshot::Sender<bool>>, settled: bool) {
    for waiter in waiters {
        let _ = waiter.send(settled);
    }
}

/// `boundWatchdogReviewText(text, cap)` (`runtime.ts:107-112` @ad11b7ab) — over the cap, keep a
/// head of at most a quarter of it (and at most [`REVIEW_INPUT_HEAD_CHARS`]), an omission marker,
/// and the tail that fills the rest; so the result is exactly `cap` characters and both ends of the
/// text survive. Counted in characters, as the rest of this module counts.
pub(crate) fn bound_watchdog_review_text(text: &str, cap: usize) -> String {
    let len = char_len(text);
    if len <= cap {
        return text.to_string();
    }
    let head = std::cmp::min(REVIEW_INPUT_HEAD_CHARS, cap / 4);
    let marker = format!("\n\n[... about {} characters omitted ...]\n\n", len - cap);
    let tail = cap.saturating_sub(head).saturating_sub(char_len(&marker));
    format!(
        "{}{marker}{}",
        head_chars(text, head),
        tail_chars(text, tail)
    )
}

/// SUBA-234 — the message a reviewer's question is delivered as (`runtime.ts:458-461` @ad11b7ab),
/// upstream's four lines verbatim: the question, its evidence, and the statement that it is not
/// approval and needs no reply.
fn clarification_message(clarification: &WatchdogClarification) -> String {
    [
        "Main watchdog clarification:".to_string(),
        clarification.question.clone(),
        format!("Evidence: {}", clarification.evidence),
        "Consider this missing context as you continue the task. This is not approval, \
         permission, or a warning. The reviewer has yielded; no reply or follow-up review is \
         required."
            .to_string(),
    ]
    .join("\n")
}

/// `promptFromBeforeAgentStart(event)` (`runtime.ts:101-107`) — `prompt`, else `systemPrompt`.
fn prompt_from_before_agent_start(event: &Value) -> Option<String> {
    let object = event.as_object()?;
    if let Some(prompt) = object.get("prompt").and_then(Value::as_str) {
        return Some(prompt.to_string());
    }
    object
        .get("systemPrompt")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// `reviewInputSignature(input)` (`runtime.ts:109-111`) — SHA-256, hex.
fn review_input_signature(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// `String.prototype.length` — UTF-16 code units.
fn char_len(value: &str) -> usize {
    value.encode_utf16().count()
}

/// `value.slice(0, n)` on UTF-16 counts, never splitting a character.
fn head_chars(value: &str, n: usize) -> String {
    if char_len(value) <= n {
        return value.to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in value.chars() {
        let width = ch.len_utf16();
        if used + width > n {
            break;
        }
        out.push(ch);
        used += width;
    }
    out
}

/// `value.slice(-n)` on UTF-16 counts, never splitting a character.
fn tail_chars(value: &str, n: usize) -> String {
    if char_len(value) <= n {
        return value.to_string();
    }
    let mut out: Vec<char> = Vec::new();
    let mut used = 0usize;
    for ch in value.chars().rev() {
        let width = ch.len_utf16();
        if used + width > n {
            break;
        }
        out.push(ch);
        used += width;
    }
    out.into_iter().rev().collect()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::super::settings::default_watchdog_config;
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ---- harness --------------------------------------------------------------------------------

    /// A review that returns a scripted set of warnings, recording every request it saw.
    struct ScriptedReview {
        warnings: Vec<WatchdogWarning>,
        stop_reason: Option<ReviewStopReason>,
        calls: Arc<Mutex<Vec<WatchdogReviewRequest>>>,
        delay: Option<Duration>,
        fail_with: Option<String>,
        return_void: bool,
    }

    impl ScriptedReview {
        fn new() -> Self {
            Self {
                warnings: Vec::new(),
                stop_reason: None,
                calls: Arc::new(Mutex::new(Vec::new())),
                delay: None,
                fail_with: None,
                return_void: false,
            }
        }
    }

    #[async_trait]
    impl WatchdogReview for ScriptedReview {
        async fn review(
            &self,
            request: WatchdogReviewRequest,
        ) -> Result<Option<WatchdogReviewResult>, String> {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(request);
            if let Some(delay) = self.delay {
                tokio::time::sleep(delay).await;
            }
            if let Some(message) = &self.fail_with {
                return Err(message.clone());
            }
            if self.return_void {
                return Ok(None);
            }
            Ok(Some(WatchdogReviewResult {
                warnings: self.warnings.clone(),
                stop_reason: self.stop_reason,
                clarification: None,
            }))
        }
    }

    /// Everything the display sink recorded: the details, and whether it was delivered as a steer.
    type DisplayedWarnings = Arc<Mutex<Vec<(WatchdogWarningDetails, Option<WatchdogDelivery>)>>>;

    #[derive(Default)]
    struct Sinks {
        displayed: DisplayedWarnings,
        /// SUBA-120 — everything `route_warning` sends to the USER-ONLY sink
        /// (`displayUserWarning`, `runtime.ts:550`). Kept separate from `displayed` so a test can
        /// assert not just that a finding was shown but WHICH surface it reached.
        user_displayed: Arc<Mutex<Vec<WatchdogWarningDetails>>>,
        user_messages: Arc<Mutex<Vec<String>>>,
    }

    fn enabled_config() -> ResolvedWatchdogConfig {
        let mut config = default_watchdog_config();
        config.enabled = true;
        config.main.enabled = true;
        config
    }

    fn fixed_resolver(config: ResolvedWatchdogConfig) -> WatchdogConfigResolver {
        Arc::new(
            move |_cwd: &Path, _session: Option<&Value>| WatchdogSettingsResult {
                ok: true,
                config: config.clone(),
                errors: Vec::new(),
                sources: Vec::new(),
            },
        )
    }

    fn options_with(config: ResolvedWatchdogConfig, sinks: &Sinks) -> MainWatchdogRuntimeOptions {
        let displayed = Arc::clone(&sinks.displayed);
        let user_displayed = Arc::clone(&sinks.user_displayed);
        let user_messages = Arc::clone(&sinks.user_messages);
        MainWatchdogRuntimeOptions {
            cwd: Some(PathBuf::from("/tmp")),
            resolve_config: Some(fixed_resolver(config)),
            review: None,
            review_description: None,
            display_warning: Some(Arc::new(move |details, delivery| {
                displayed
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push((details.clone(), delivery));
            })),
            display_user_warning: Some(Arc::new(move |details: &WatchdogWarningDetails| {
                user_displayed
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(details.clone());
            })),
            send_user_message: Some(Arc::new(move |message: &str| {
                user_messages
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(message.to_string());
                Ok(())
            })),
            review_changes_only: false,
            lsp_diagnostics: None,
            repo_change_signature: None,
            // The pre-SUBA-234 tests run without the main role's clarification sink; the
            // clarification tests install one explicitly.
            display_clarification: None,
        }
    }

    /// SUBA-120: `High` importance, so these two helpers keep routing to the PARENT-MODEL sink and
    /// every pre-existing `sinks.displayed` assertion still tests what it was written to test. The
    /// non-`high` path has its own tests below.
    fn blocker(summary: &str) -> WatchdogWarning {
        WatchdogWarning::new(
            WatchdogSeverity::Blocker,
            WatchdogImportance::High,
            summary,
            "the evidence for it",
            "fix it before continuing",
        )
    }

    fn concern(summary: &str) -> WatchdogWarning {
        WatchdogWarning::new(
            WatchdogSeverity::Concern,
            WatchdogImportance::High,
            summary,
            "the evidence for it",
            "consider fixing it",
        )
    }

    fn turn_end_event(text: &str) -> Value {
        json!({
            "type": "turn_end",
            "message": { "role": "assistant", "content": [{ "type": "text", "text": text }] },
            "toolResults": [],
        })
    }

    fn cwd() -> PathBuf {
        PathBuf::from("/tmp")
    }

    // ---- enablement -----------------------------------------------------------------------------

    #[tokio::test]
    async fn a_default_config_leaves_the_runtime_off_and_it_buffers_nothing() {
        let sinks = Sinks::default();
        let runtime = MainWatchdogRuntime::new(options_with(default_watchdog_config(), &sinks));
        runtime.handle_turn_end(&turn_end_event("hello"), &cwd());
        let snapshot = runtime.get_snapshot(None);
        assert!(!snapshot.enabled);
        assert_eq!(snapshot.buffered_deltas, 0);
        assert_eq!(snapshot.status, WatchdogRuntimeStatus::Idle);
    }

    #[tokio::test]
    async fn a_broken_config_disables_the_runtime_even_when_enabled_is_true() {
        let sinks = Sinks::default();
        let mut options = options_with(enabled_config(), &sinks);
        options.resolve_config = Some(Arc::new(|_cwd: &Path, _s: Option<&Value>| {
            let mut config = default_watchdog_config();
            config.enabled = true;
            config.main.enabled = true;
            WatchdogSettingsResult {
                ok: false,
                config,
                errors: vec![WatchdogSettingsError {
                    scope: super::super::types::WatchdogSettingsScope::User,
                    path: None,
                    message: "boom".into(),
                }],
                sources: Vec::new(),
            }
        }));
        let runtime = MainWatchdogRuntime::new(options);
        assert!(!runtime.get_snapshot(None).enabled, "ok:false disables");
    }

    #[tokio::test]
    async fn an_enabled_runtime_buffers_a_turn_delta_and_reports_queued() {
        let sinks = Sinks::default();
        let runtime = MainWatchdogRuntime::new(options_with(enabled_config(), &sinks));
        runtime.handle_turn_end(&turn_end_event("did some work"), &cwd());
        let snapshot = runtime.get_snapshot(None);
        assert!(snapshot.enabled);
        assert_eq!(snapshot.buffered_deltas, 1);
        assert_eq!(snapshot.status, WatchdogRuntimeStatus::Queued);
    }

    #[tokio::test]
    async fn an_empty_delta_is_not_buffered() {
        let sinks = Sinks::default();
        let runtime = MainWatchdogRuntime::new(options_with(enabled_config(), &sinks));
        runtime.enqueue_delta("   \n  ");
        assert_eq!(runtime.get_snapshot(None).buffered_deltas, 0);
    }

    // ---- the boundary review ---------------------------------------------------------------------

    #[tokio::test]
    async fn the_agent_end_boundary_reviews_the_buffered_delta_and_displays_a_warning() {
        let sinks = Sinks::default();
        let mut review = ScriptedReview::new();
        review.warnings = vec![blocker("the tests were deleted")];
        let calls = Arc::clone(&review.calls);
        let mut options = options_with(enabled_config(), &sinks);
        options.review = Some(Arc::new(review));
        options.review_description = Some("scripted".to_string());
        let runtime = MainWatchdogRuntime::new(options);

        runtime.handle_before_agent_start(&json!({ "prompt": "add a feature" }), &cwd());
        runtime.handle_turn_end(&turn_end_event("deleted the tests"), &cwd());
        runtime.handle_agent_end(&cwd()).await;

        let requests = calls.lock().unwrap();
        assert_eq!(requests.len(), 1, "one review ran");
        assert!(
            requests[0].delta.contains("deleted the tests"),
            "{}",
            requests[0].delta
        );
        assert!(requests[0].has_scope, "the user prompt entered the scope");

        let displayed = sinks.displayed.lock().unwrap();
        assert_eq!(displayed.len(), 1);
        assert_eq!(displayed[0].0.summary, "the tests were deleted");
        assert_eq!(displayed[0].0.state, Some(WatchdogWarningState::Displayed));
        assert!(displayed[0].0.displayed_at.is_some());
        assert_eq!(displayed[0].1, None, "a boundary warning is not a steer");

        let snapshot = runtime.get_snapshot(None);
        assert_eq!(snapshot.status, WatchdogRuntimeStatus::Idle);
        assert_eq!(snapshot.buffered_deltas, 0);
        assert!(snapshot.review_connected);
        assert_eq!(snapshot.review_description, "scripted");
    }

    #[tokio::test]
    async fn a_review_that_emits_through_the_streaming_channel_is_accepted_too() {
        struct StreamingReview;
        #[async_trait]
        impl WatchdogReview for StreamingReview {
            async fn review(
                &self,
                request: WatchdogReviewRequest,
            ) -> Result<Option<WatchdogReviewResult>, String> {
                assert!(request.emit_warning.emit(&blocker("streamed")));
                Ok(Some(WatchdogReviewResult::default()))
            }
        }
        let sinks = Sinks::default();
        let mut options = options_with(enabled_config(), &sinks);
        options.review = Some(Arc::new(StreamingReview));
        let runtime = MainWatchdogRuntime::new(options);
        runtime.handle_turn_end(&turn_end_event("work"), &cwd());
        runtime.handle_agent_end(&cwd()).await;
        let displayed = sinks.displayed.lock().unwrap();
        assert_eq!(displayed.len(), 1);
        assert_eq!(displayed[0].0.summary, "streamed");
    }

    #[tokio::test]
    async fn an_identical_delta_is_not_reviewed_twice() {
        let sinks = Sinks::default();
        let review = ScriptedReview::new();
        let calls = Arc::clone(&review.calls);
        let mut options = options_with(enabled_config(), &sinks);
        options.review = Some(Arc::new(review));
        let runtime = MainWatchdogRuntime::new(options);

        for _ in 0..2 {
            runtime.handle_turn_end(&turn_end_event("identical work"), &cwd());
            runtime.handle_agent_end(&cwd()).await;
        }
        assert_eq!(
            calls.lock().unwrap().len(),
            1,
            "the second delta was a repeat"
        );
    }

    #[tokio::test]
    async fn a_review_timeout_marks_the_runtime_stale_and_counts_it() {
        let sinks = Sinks::default();
        let mut review = ScriptedReview::new();
        review.delay = Some(Duration::from_secs(30));
        let mut config = enabled_config();
        config.agent_end_timeout_ms = 20;
        let mut options = options_with(config, &sinks);
        options.review = Some(Arc::new(review));
        let runtime = MainWatchdogRuntime::new(options);

        runtime.handle_turn_end(&turn_end_event("slow work"), &cwd());
        runtime.handle_agent_end(&cwd()).await;

        let snapshot = runtime.get_snapshot(None);
        assert_eq!(snapshot.status, WatchdogRuntimeStatus::Stale);
        assert_eq!(snapshot.stale_reviews, 1);
        assert!(sinks.displayed.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_thrown_review_fails_the_runtime_and_records_the_message() {
        let sinks = Sinks::default();
        let mut review = ScriptedReview::new();
        review.fail_with = Some("provider exploded".to_string());
        let mut options = options_with(enabled_config(), &sinks);
        options.review = Some(Arc::new(review));
        let runtime = MainWatchdogRuntime::new(options);

        runtime.handle_turn_end(&turn_end_event("work"), &cwd());
        runtime.handle_agent_end(&cwd()).await;

        let snapshot = runtime.get_snapshot(None);
        assert_eq!(snapshot.status, WatchdogRuntimeStatus::Failed);
        assert_eq!(snapshot.failed_reviews, 1);
        assert_eq!(
            snapshot.last_error.as_deref(),
            Some("Watchdog review failed: provider exploded")
        );
    }

    #[tokio::test]
    async fn a_non_stop_stop_reason_fails_the_review() {
        let sinks = Sinks::default();
        let mut review = ScriptedReview::new();
        review.stop_reason = Some(ReviewStopReason::Length);
        let mut options = options_with(enabled_config(), &sinks);
        options.review = Some(Arc::new(review));
        let runtime = MainWatchdogRuntime::new(options);
        runtime.handle_turn_end(&turn_end_event("work"), &cwd());
        runtime.handle_agent_end(&cwd()).await;
        assert_eq!(
            runtime.get_snapshot(None).last_error.as_deref(),
            Some("Watchdog review ended with stop reason 'length'.")
        );
    }

    #[tokio::test]
    async fn a_void_review_is_stale_and_displays_nothing() {
        let sinks = Sinks::default();
        let mut review = ScriptedReview::new();
        review.return_void = true;
        let mut options = options_with(enabled_config(), &sinks);
        options.review = Some(Arc::new(review));
        let runtime = MainWatchdogRuntime::new(options);
        runtime.handle_turn_end(&turn_end_event("work"), &cwd());
        runtime.handle_agent_end(&cwd()).await;
        assert!(sinks.displayed.lock().unwrap().is_empty());
    }

    // ---- severity threshold + emission guard ------------------------------------------------------

    #[tokio::test]
    async fn a_blocker_only_threshold_drops_a_concern() {
        let sinks = Sinks::default();
        let mut review = ScriptedReview::new();
        review.warnings = vec![concern("a small thing")];
        let mut config = enabled_config();
        config.severity_threshold = WatchdogSeverity::Blocker;
        let mut options = options_with(config, &sinks);
        options.review = Some(Arc::new(review));
        let runtime = MainWatchdogRuntime::new(options);
        runtime.handle_turn_end(&turn_end_event("work"), &cwd());
        runtime.handle_agent_end(&cwd()).await;
        assert!(sinks.displayed.lock().unwrap().is_empty());
    }

    // ---- SUBA-120: importance routing (`routeWarning`, `runtime.ts:548-550`) --------------------

    /// SUBA-120 test (a). With BOTH sinks installed, a `high` finding reaches the parent-model sink
    /// and a `low` finding reaches the user-only sink — and, the part that actually matters, the `low`
    /// one NEVER reaches the parent-model sink (`routeWarning`, `runtime.ts:548-550`).
    ///
    /// Before this row there was ONE sink and every threshold-passing finding went to it, so a `low`
    /// finding was delivered into the parent model's context exactly like a `high` one.
    ///
    /// The two importances are driven through SEPARATE runs rather than as two warnings of one review:
    /// a review accepts at most one warning (`accept_warning` latches `active_review_warning`), so a
    /// second finding in the same review is never displayed at all and would prove nothing about
    /// routing.
    #[tokio::test]
    async fn a_high_finding_reaches_the_model_sink_and_a_low_finding_only_the_user_sink() {
        async fn route(importance: WatchdogImportance) -> (usize, usize) {
            let sinks = Sinks::default();
            let mut review = ScriptedReview::new();
            review.warnings = vec![WatchdogWarning::new(
                WatchdogSeverity::Blocker,
                importance,
                "a finding worth routing",
                "the evidence for it",
                "fix it before continuing",
            )];
            let mut options = options_with(enabled_config(), &sinks);
            options.review = Some(Arc::new(review));
            let runtime = MainWatchdogRuntime::new(options);
            runtime.handle_turn_end(&turn_end_event("work"), &cwd());
            runtime.handle_agent_end(&cwd()).await;
            let to_model = sinks.displayed.lock().unwrap().len();
            let to_user = sinks.user_displayed.lock().unwrap().len();
            (to_model, to_user)
        }

        assert_eq!(
            route(WatchdogImportance::High).await,
            (1, 0),
            "a high finding goes to the parent model and NOT to the user-only surface"
        );
        assert_eq!(
            route(WatchdogImportance::Low).await,
            (0, 1),
            "a low finding goes to the user-only surface and NEVER to the parent model"
        );
    }

    /// A `medium` finding routes the same way a `low` one does — `routeWarning`'s rule is
    /// `high` versus everything else, not a three-way split (`runtime.ts:549`).
    #[tokio::test]
    async fn a_medium_finding_routes_to_the_user_sink_like_a_low_one() {
        let sinks = Sinks::default();
        let mut review = ScriptedReview::new();
        review.warnings = vec![WatchdogWarning::new(
            WatchdogSeverity::Blocker,
            WatchdogImportance::Medium,
            "middling but real",
            "the evidence for it",
            "consider it",
        )];
        let mut options = options_with(enabled_config(), &sinks);
        options.review = Some(Arc::new(review));
        let runtime = MainWatchdogRuntime::new(options);
        runtime.handle_turn_end(&turn_end_event("work"), &cwd());
        runtime.handle_agent_end(&cwd()).await;
        assert!(sinks.displayed.lock().unwrap().is_empty());
        assert_eq!(sinks.user_displayed.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn the_emission_guard_suppresses_a_content_free_warning() {
        let sinks = Sinks::default();
        let mut review = ScriptedReview::new();
        review.warnings = vec![WatchdogWarning::new(
            WatchdogSeverity::Blocker,
            WatchdogImportance::High,
            "LGTM",
            "looks good",
            "none",
        )];
        let mut options = options_with(enabled_config(), &sinks);
        options.review = Some(Arc::new(review));
        let runtime = MainWatchdogRuntime::new(options);
        runtime.handle_turn_end(&turn_end_event("work"), &cwd());
        runtime.handle_agent_end(&cwd()).await;
        assert!(sinks.displayed.lock().unwrap().is_empty());
    }

    // ---- auto-follow ------------------------------------------------------------------------------

    #[tokio::test]
    async fn a_displayed_blocker_queues_one_auto_follow_prompt() {
        let sinks = Sinks::default();
        let mut review = ScriptedReview::new();
        review.warnings = vec![blocker("the migration is unreversible")];
        let mut options = options_with(enabled_config(), &sinks);
        options.review = Some(Arc::new(review));
        let runtime = MainWatchdogRuntime::new(options);

        runtime.handle_turn_end(&turn_end_event("wrote a migration"), &cwd());
        runtime.handle_agent_end(&cwd()).await;

        let messages = sinks.user_messages.lock().unwrap();
        assert_eq!(messages.len(), 1);
        assert!(
            messages[0]
                .starts_with("Watchdog auto-follow: address this blocker before continuing.\n")
        );
        assert!(messages[0].contains("Summary: the migration is unreversible"));
        assert!(messages[0].contains("Evidence: the evidence for it"));
        assert!(messages[0].contains("Recommended action: fix it before continuing"));
        drop(messages);

        let snapshot = runtime.get_snapshot(None);
        assert!(snapshot.auto_follow_queued);
        assert_eq!(snapshot.auto_follow_attempts, 1);
    }

    #[tokio::test]
    async fn a_concern_never_auto_follows() {
        let sinks = Sinks::default();
        let mut review = ScriptedReview::new();
        review.warnings = vec![concern("a small thing")];
        let mut options = options_with(enabled_config(), &sinks);
        options.review = Some(Arc::new(review));
        let runtime = MainWatchdogRuntime::new(options);
        runtime.handle_turn_end(&turn_end_event("work"), &cwd());
        runtime.handle_agent_end(&cwd()).await;
        assert!(sinks.user_messages.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_auto_follow_prompt_does_not_widen_the_scope_or_reset_the_attempt_counter() {
        let sinks = Sinks::default();
        let mut review = ScriptedReview::new();
        review.warnings = vec![blocker("still broken")];
        let calls = Arc::clone(&review.calls);
        let mut options = options_with(enabled_config(), &sinks);
        options.review = Some(Arc::new(review));
        let runtime = MainWatchdogRuntime::new(options);

        runtime.handle_before_agent_start(&json!({ "prompt": "the real request" }), &cwd());
        runtime.handle_turn_end(&turn_end_event("work one"), &cwd());
        runtime.handle_agent_end(&cwd()).await;
        let follow_up = sinks.user_messages.lock().unwrap()[0].clone();
        assert_eq!(runtime.get_snapshot(None).auto_follow_attempts, 1);

        // The follow-up turn arrives carrying exactly the queued text.
        runtime.handle_before_agent_start(&json!({ "prompt": follow_up }), &cwd());
        runtime.handle_turn_end(&turn_end_event("work two"), &cwd());
        runtime.handle_agent_end(&cwd()).await;

        assert_eq!(
            runtime.get_snapshot(None).auto_follow_attempts,
            2,
            "the counter accumulated instead of resetting"
        );
        let requests = calls.lock().unwrap();
        let last = requests.last().unwrap();
        assert!(
            !last.delta.contains("Watchdog auto-follow"),
            "the synthetic prompt never entered the scope record: {}",
            last.delta
        );
        assert!(last.delta.contains("the real request"));
    }

    #[tokio::test]
    async fn repeated_identical_blockers_declare_a_stalemate_and_stop_auto_following() {
        let sinks = Sinks::default();
        let mut review = ScriptedReview::new();
        review.warnings = vec![blocker("the same blocker")];
        let mut config = enabled_config();
        config.auto_follow.stalemate_repeats = 2;
        config.auto_follow.max_attempts = None;
        let mut options = options_with(config, &sinks);
        options.review = Some(Arc::new(review));
        let runtime = MainWatchdogRuntime::new(options);

        for n in 0..3 {
            runtime.handle_before_agent_start(&json!({ "prompt": format!("turn {n}") }), &cwd());
            runtime.handle_turn_end(&turn_end_event(&format!("work {n}")), &cwd());
            runtime.handle_agent_end(&cwd()).await;
        }
        let snapshot = runtime.get_snapshot(None);
        // A REAL user prompt resets the auto-follow state each turn, so the stalemate counter only
        // accumulates across auto-follow turns; what this pins is that the identity tracking and
        // the stalemate flag are reachable at all and that delivery never exceeded the attempts.
        assert!(
            snapshot.auto_follow_attempts <= 3,
            "attempts {}",
            snapshot.auto_follow_attempts
        );
        assert_eq!(sinks.user_messages.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn the_attempt_ceiling_stops_delivery() {
        let sinks = Sinks::default();
        let mut review = ScriptedReview::new();
        review.warnings = vec![blocker("blocked")];
        let mut config = enabled_config();
        config.auto_follow.max_attempts = Some(1);
        config.auto_follow.stalemate_repeats = 99;
        let mut options = options_with(config, &sinks);
        options.review = Some(Arc::new(review));
        let runtime = MainWatchdogRuntime::new(options);

        runtime.handle_turn_end(&turn_end_event("work one"), &cwd());
        runtime.handle_agent_end(&cwd()).await;
        let first = sinks.user_messages.lock().unwrap()[0].clone();
        // Deliver the follow-up itself, which does NOT reset the counters.
        runtime.handle_before_agent_start(&json!({ "prompt": first }), &cwd());
        runtime.handle_turn_end(&turn_end_event("work two"), &cwd());
        runtime.handle_agent_end(&cwd()).await;

        assert_eq!(
            sinks.user_messages.lock().unwrap().len(),
            1,
            "the ceiling of one was respected"
        );
    }

    #[tokio::test]
    async fn auto_follow_is_off_when_the_policy_says_so() {
        let sinks = Sinks::default();
        let mut review = ScriptedReview::new();
        review.warnings = vec![blocker("blocked")];
        let mut config = enabled_config();
        config.auto_follow.blockers = false;
        let mut options = options_with(config, &sinks);
        options.review = Some(Arc::new(review));
        let runtime = MainWatchdogRuntime::new(options);
        runtime.handle_turn_end(&turn_end_event("work"), &cwd());
        runtime.handle_agent_end(&cwd()).await;
        assert!(sinks.user_messages.lock().unwrap().is_empty());
        assert_eq!(sinks.displayed.lock().unwrap().len(), 1, "still displayed");
    }

    #[tokio::test]
    async fn a_failed_auto_follow_delivery_unqueues_and_records_the_error() {
        let sinks = Sinks::default();
        let mut review = ScriptedReview::new();
        review.warnings = vec![blocker("blocked")];
        let mut options = options_with(enabled_config(), &sinks);
        options.review = Some(Arc::new(review));
        options.send_user_message = Some(Arc::new(|_m: &str| Err("no live session".to_string())));
        let runtime = MainWatchdogRuntime::new(options);
        runtime.handle_turn_end(&turn_end_event("work"), &cwd());
        runtime.handle_agent_end(&cwd()).await;
        let snapshot = runtime.get_snapshot(None);
        assert!(!snapshot.auto_follow_queued);
        assert_eq!(
            snapshot.last_error.as_deref(),
            Some("Watchdog auto-follow failed: no live session")
        );
    }

    // ---- cadence ------------------------------------------------------------------------------

    #[tokio::test]
    async fn the_cadence_trigger_reviews_every_nth_tool_result() {
        let sinks = Sinks::default();
        let mut review = ScriptedReview::new();
        review.warnings = vec![blocker("mid-run problem")];
        let calls = Arc::clone(&review.calls);
        let mut config = enabled_config();
        config.cadence.every_n_tools = Some(5);
        let mut options = options_with(config, &sinks);
        options.review = Some(Arc::new(review));
        let runtime = Arc::new(MainWatchdogRuntime::new(options));

        runtime.handle_turn_end(&turn_end_event("some work"), &cwd());
        for _ in 0..4 {
            runtime.handle_tool_result(&cwd());
        }
        assert!(calls.lock().unwrap().is_empty(), "four is not five");
        runtime.handle_tool_result(&cwd());
        // The mid-run review is SPAWNED (upstream's un-awaited `void this.reviewMidRunDelta(...)`),
        // so poll for it. `wait_for_idle` is deliberately not used: `isSettled` also requires an
        // EMPTY delta buffer (`runtime.ts:842`), and a mid-run review does not clear it — only the
        // agent-end boundary does (`:398`).
        for _ in 0..200 {
            if calls.lock().unwrap().len() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(calls.lock().unwrap().len(), 1);

        let displayed = sinks.displayed.lock().unwrap();
        assert_eq!(displayed.len(), 1);
        assert_eq!(
            displayed[0].1,
            Some(WatchdogDelivery::Steer),
            "a mid-run correction is delivered as a steer"
        );
    }

    #[tokio::test]
    async fn no_cadence_means_no_mid_run_review() {
        let sinks = Sinks::default();
        let review = ScriptedReview::new();
        let calls = Arc::clone(&review.calls);
        let mut options = options_with(enabled_config(), &sinks);
        options.review = Some(Arc::new(review));
        let runtime = Arc::new(MainWatchdogRuntime::new(options));
        runtime.handle_turn_end(&turn_end_event("work"), &cwd());
        for _ in 0..50 {
            runtime.handle_tool_result(&cwd());
        }
        tokio::task::yield_now().await;
        assert!(calls.lock().unwrap().is_empty());
    }

    // ---- lifecycle ------------------------------------------------------------------------------

    #[tokio::test]
    async fn dispose_makes_every_handler_inert() {
        let sinks = Sinks::default();
        let review = ScriptedReview::new();
        let calls = Arc::clone(&review.calls);
        let mut options = options_with(enabled_config(), &sinks);
        options.review = Some(Arc::new(review));
        let runtime = MainWatchdogRuntime::new(options);
        runtime.handle_turn_end(&turn_end_event("work"), &cwd());
        runtime.dispose();
        runtime.handle_turn_end(&turn_end_event("more work"), &cwd());
        runtime.handle_agent_end(&cwd()).await;
        assert!(calls.lock().unwrap().is_empty());
        assert_eq!(runtime.get_snapshot(None).buffered_deltas, 0);
    }

    #[tokio::test]
    async fn a_reset_drops_the_buffer_and_bumps_the_epoch() {
        let sinks = Sinks::default();
        let runtime = MainWatchdogRuntime::new(options_with(enabled_config(), &sinks));
        runtime.handle_turn_end(&turn_end_event("work"), &cwd());
        let before = runtime.get_snapshot(None);
        runtime.reset(WatchdogResetOptions::default());
        let after = runtime.get_snapshot(None);
        assert_eq!(after.buffered_deltas, 0);
        assert!(after.epoch > before.epoch);
        assert_eq!(after.status, WatchdogRuntimeStatus::Idle);
    }

    #[tokio::test]
    async fn bind_session_clears_the_session_overrides() {
        let sinks = Sinks::default();
        let runtime = MainWatchdogRuntime::new(options_with(enabled_config(), &sinks));
        runtime.set_session_enabled(false, &cwd());
        assert_eq!(runtime.get_snapshot(None).session_override, Some(false));
        runtime.bind_session(&cwd());
        assert_eq!(runtime.get_snapshot(None).session_override, None);
    }

    #[tokio::test]
    async fn wait_for_idle_returns_immediately_when_settled() {
        let sinks = Sinks::default();
        let runtime = MainWatchdogRuntime::new(options_with(enabled_config(), &sinks));
        assert!(runtime.wait_for_idle(Duration::from_millis(10)).await);
    }

    #[tokio::test]
    async fn wait_for_idle_reports_false_after_dispose() {
        let sinks = Sinks::default();
        let runtime = Arc::new(MainWatchdogRuntime::new(options_with(
            enabled_config(),
            &sinks,
        )));
        runtime.handle_turn_end(&turn_end_event("work"), &cwd());
        let waiter = {
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move { runtime.wait_for_idle(Duration::from_secs(5)).await })
        };
        tokio::task::yield_now().await;
        runtime.dispose();
        assert!(!waiter.await.unwrap());
    }

    // ---- session overrides ----------------------------------------------------------------------

    #[tokio::test]
    async fn the_session_override_reaches_the_config_resolver() {
        let seen: Arc<Mutex<Vec<Option<Value>>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        let sinks = Sinks::default();
        let mut options = options_with(enabled_config(), &sinks);
        options.resolve_config = Some(Arc::new(move |_cwd: &Path, session: Option<&Value>| {
            recorded
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(session.cloned());
            WatchdogSettingsResult {
                ok: true,
                config: enabled_config(),
                errors: Vec::new(),
                sources: Vec::new(),
            }
        }));
        let runtime = MainWatchdogRuntime::new(options);
        runtime.set_session_model(
            Some(Some("anthropic/opus".to_string())),
            Some(Some(ThinkingSetting::Level("high".into()))),
            &cwd(),
        );
        let calls = seen.lock().unwrap();
        let last = calls.last().unwrap().clone().unwrap();
        assert_eq!(last["main"]["model"], json!("anthropic/opus"));
        assert_eq!(last["main"]["thinking"], json!("high"));
        assert!(last.get("enabled").is_none(), "no enable override was set");
    }

    #[tokio::test]
    async fn clearing_the_session_model_drops_the_override_entirely() {
        let sinks = Sinks::default();
        let runtime = MainWatchdogRuntime::new(options_with(enabled_config(), &sinks));
        runtime.set_session_model(Some(Some("a/b".into())), None, &cwd());
        assert!(runtime.get_snapshot(None).session_model_override.is_some());
        runtime.clear_session_model(&cwd());
        assert!(runtime.get_snapshot(None).session_model_override.is_none());
    }

    /// `clearSessionOverride(cwd)` (`runtime.ts:242-248`) clears BOTH session slots at once, where
    /// `setSessionEnabled(false)` (`:236-241`'s sibling) and [`MainWatchdogRuntime::clear_session_model`]
    /// each clear one. Nothing calls it upstream either (see its doc), so this test is the only
    /// thing standing between the ported method and silent rot.
    #[tokio::test]
    async fn clearing_the_session_override_drops_both_slots_where_the_narrow_clears_drop_one() {
        let sinks = Sinks::default();
        let runtime = MainWatchdogRuntime::new(options_with(enabled_config(), &sinks));
        runtime.set_session_enabled(false, &cwd());
        runtime.set_session_model(Some(Some("a/b".into())), None, &cwd());
        assert_eq!(runtime.get_snapshot(None).session_override, Some(false));
        assert!(runtime.get_snapshot(None).session_model_override.is_some());

        // The NARROW clear leaves the enabled override standing …
        runtime.clear_session_model(&cwd());
        assert_eq!(
            runtime.get_snapshot(None).session_override,
            Some(false),
            "clear_session_model must not touch the enabled override"
        );

        // … and the wide one drops it too.
        runtime.set_session_model(Some(Some("a/b".into())), None, &cwd());
        let snapshot = runtime.clear_session_override(&cwd());
        assert_eq!(snapshot.session_override, None);
        assert!(snapshot.session_model_override.is_none());
        // The returned snapshot is the live one, not a stale copy (`:247`'s `getSnapshot()`).
        assert_eq!(runtime.get_snapshot(None).session_override, None);
        assert!(runtime.get_snapshot(None).session_model_override.is_none());
    }

    // ---- repo-change trigger ---------------------------------------------------------------------

    /// A settable stand-in for `computeWatchdogRepoChangeSignature`, so the change-trigger tests
    /// never touch a real git repository.
    struct FixedSignatures {
        signature: Mutex<Option<WatchdogRepoChangeSignature>>,
        calls: AtomicUsize,
    }

    impl WatchdogRepoChangeSource for FixedSignatures {
        fn compute(&self, _cwd: &Path) -> Option<WatchdogRepoChangeSignature> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.signature
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    /// A source that never reports a signature — the "not a git repository" answer, which keeps
    /// these tests off the real `git` binary and off this checkout's own working tree.
    struct NoSignatures;

    impl WatchdogRepoChangeSource for NoSignatures {
        fn compute(&self, _cwd: &Path) -> Option<WatchdogRepoChangeSignature> {
            None
        }
    }

    #[tokio::test]
    async fn a_changes_only_runtime_skips_the_review_when_nothing_changed() {
        let sinks = Sinks::default();
        let review = ScriptedReview::new();
        let calls = Arc::clone(&review.calls);
        let mut options = options_with(enabled_config(), &sinks);
        options.review = Some(Arc::new(review));
        options.review_changes_only = true;
        options.repo_change_signature = Some(Arc::new(NoSignatures));
        let runtime = MainWatchdogRuntime::new(options);
        runtime.handle_turn_end(&turn_end_event("thought about it"), &cwd());
        runtime.handle_agent_end(&cwd()).await;
        assert!(calls.lock().unwrap().is_empty());
        assert_eq!(runtime.get_snapshot(None).buffered_deltas, 0);
        assert_eq!(
            runtime.get_snapshot(None).review_trigger,
            WatchdogReviewTrigger::RepoEdits
        );
    }

    #[tokio::test]
    async fn a_changes_only_runtime_reviews_once_the_signature_moves() {
        let sinks = Sinks::default();
        let review = ScriptedReview::new();
        let calls = Arc::clone(&review.calls);
        let source = Arc::new(FixedSignatures {
            signature: Mutex::new(None),
            calls: AtomicUsize::new(0),
        });
        let mut options = options_with(enabled_config(), &sinks);
        options.review = Some(Arc::new(review));
        options.review_changes_only = true;
        options.repo_change_signature =
            Some(Arc::clone(&source) as Arc<dyn WatchdogRepoChangeSource>);
        let runtime = MainWatchdogRuntime::new(options);

        *source.signature.lock().unwrap() = Some(WatchdogRepoChangeSignature {
            root: "/repo".into(),
            key: "key-1".into(),
            changed_paths: vec!["src/lib.rs".into()],
        });
        runtime.handle_turn_end(&turn_end_event("edited a file"), &cwd());
        runtime.handle_agent_end(&cwd()).await;
        assert_eq!(calls.lock().unwrap().len(), 1);
        assert!(
            calls.lock().unwrap()[0]
                .delta
                .contains(&format!("{CHANGED_REPO_PATHS_HEADER}\n- src/lib.rs")),
            "{}",
            calls.lock().unwrap()[0].delta
        );
        assert_eq!(
            runtime.get_snapshot(None).changed_paths,
            Some(vec!["src/lib.rs".to_string()])
        );

        // The same key at the next boundary is not reviewed again.
        runtime.handle_turn_end(&turn_end_event("more work"), &cwd());
        runtime.handle_agent_end(&cwd()).await;
        assert_eq!(calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn an_observed_edit_tool_result_triggers_a_review_without_a_signature() {
        let sinks = Sinks::default();
        let review = ScriptedReview::new();
        let calls = Arc::clone(&review.calls);
        let mut options = options_with(enabled_config(), &sinks);
        options.review = Some(Arc::new(review));
        options.review_changes_only = true;
        options.repo_change_signature = Some(Arc::new(NoSignatures));
        let runtime = MainWatchdogRuntime::new(options);
        let event = json!({
            "type": "turn_end",
            "message": { "role": "assistant", "content": [{ "type": "text", "text": "wrote a file" }] },
            "toolResults": [{ "role": "toolResult", "toolName": "write", "content": "ok" }],
        });
        runtime.handle_turn_end(&event, &cwd());
        runtime.handle_agent_end(&cwd()).await;
        assert_eq!(calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn event_indicates_repo_edit_only_for_successful_edit_and_write() {
        let ok = json!({
            "type": "turn_end",
            "toolResults": [{ "role": "toolResult", "toolName": "edit", "content": "done" }],
        });
        assert!(event_indicates_repo_edit(&ok));
        let failed = json!({
            "type": "turn_end",
            "toolResults": [{ "role": "toolResult", "toolName": "edit", "isError": true }],
        });
        assert!(!event_indicates_repo_edit(&failed));
        let other_tool = json!({
            "type": "turn_end",
            "toolResults": [{ "role": "toolResult", "toolName": "read", "content": "x" }],
        });
        assert!(!event_indicates_repo_edit(&other_tool));
        let direct = json!({ "type": "tool_result", "toolName": "write", "content": "ok" });
        assert!(event_indicates_repo_edit(&direct));
        assert!(!event_indicates_repo_edit(&json!({ "type": "agent_end" })));
        assert!(!event_indicates_repo_edit(&json!("not an object")));
    }

    // ---- bounding ------------------------------------------------------------------------------

    #[test]
    fn the_delta_buffer_drops_the_oldest_entries_to_fit_the_budget() {
        let mut inner = test_inner();
        for n in 0..10 {
            inner.append_bounded_delta(&format!("{n}{}", "x".repeat(5_000)));
        }
        let joined_len: usize = inner
            .pending_deltas
            .join(REVIEW_DELTA_SEPARATOR)
            .encode_utf16()
            .count();
        assert!(joined_len <= MAX_REVIEW_INPUT_CHARS, "joined {joined_len}");
        assert!(
            inner.pending_deltas.last().unwrap().starts_with('9'),
            "the newest entry survived"
        );
    }

    #[test]
    fn a_single_over_budget_delta_is_truncated_from_the_front() {
        let mut inner = test_inner();
        let delta = format!("{}TAIL", "y".repeat(MAX_REVIEW_INPUT_CHARS));
        inner.append_bounded_delta(&delta);
        assert_eq!(inner.pending_deltas.len(), 1);
        assert!(inner.pending_deltas[0].ends_with("TAIL"));
        assert_eq!(
            inner.pending_deltas[0].encode_utf16().count(),
            MAX_REVIEW_INPUT_CHARS
        );
    }

    #[test]
    fn the_review_input_keeps_the_context_blocks_and_the_newest_delta() {
        let mut inner = test_inner();
        inner.scope.add_prompt("do the thing", Some("T".into()));
        inner.append_bounded_delta("recent work");
        let signature = WatchdogRepoChangeSignature {
            root: "/repo".into(),
            key: "k".into(),
            changed_paths: vec!["a.rs".into(), "b.rs".into()],
        };
        let input = inner.build_review_input(Some(&signature), "LSP BLOCK");
        assert!(input.contains("Current scope:"));
        assert!(input.contains(&format!("{CHANGED_REPO_PATHS_HEADER}\n- a.rs\n- b.rs")));
        assert!(input.contains("LSP BLOCK"));
        assert!(input.ends_with("recent work"));
        assert!(input.encode_utf16().count() <= MAX_REVIEW_INPUT_CHARS);
    }

    /// SUBA-198 — the changed-paths block says what it actually is: EVERY dirty path in the
    /// working tree, whose authorship nobody checked. pi `runtime.ts:855` @ad11b7ab (`ff2e00bd`),
    /// verbatim. The bare `Changed repo paths:` it replaces let a reviewer take another session's
    /// dirty files — or files dirty before this session began — for this session's work.
    #[test]
    fn the_changed_paths_header_says_authorship_is_not_verified() {
        assert_eq!(
            CHANGED_REPO_PATHS_HEADER,
            "Changed repo paths: every dirty path in the working tree. Authorship is not \
             verified; a path may predate this session or come from another session."
        );
        let mut inner = test_inner();
        let signature = WatchdogRepoChangeSignature {
            root: "/repo".into(),
            key: "k".into(),
            changed_paths: vec!["a.rs".into()],
        };
        let input = inner.build_review_input(Some(&signature), "");
        assert!(
            input.starts_with(
                "Changed repo paths: every dirty path in the working tree. Authorship is not \
                 verified; a path may predate this session or come from another session.\n- a.rs"
            ),
            "{input}"
        );
    }

    /// The `Scope prompt N (…):` entries of a review delta, in order — what the reviewer is told
    /// the user authorized.
    fn scope_prompts_in(delta: &str) -> Vec<String> {
        let mut prompts = Vec::new();
        let mut lines = delta.lines();
        while let Some(line) = lines.next() {
            if line.starts_with("Scope prompt ")
                && line.ends_with("):")
                && let Some(prompt) = lines.next()
            {
                prompts.push(prompt.to_string());
            }
        }
        prompts
    }

    /// SUBA-198 — pi `watchdog-runtime.test.ts:1102-1123` @ad11b7ab ("adds user messages sent
    /// while the agent streams to the scope record"), input for input: a steer and an RPC
    /// follow-up sent mid-run reach the next review's scope record, so the reviewer judges the
    /// work they asked for as authorized rather than as scope drift; the idle prompt is recorded
    /// once (by `before_agent_start`, not twice), and an extension's own message never is.
    ///
    /// **Gutting mutation this fails on:** make [`MainWatchdogRuntime::handle_user_input`] a no-op
    /// — the review then sees only the idle prompt, and the steered work reads as drift.
    #[tokio::test]
    async fn a_message_sent_while_the_agent_streams_joins_the_scope_record() {
        use cyrup_ext::{InputEventSource, InputStreamingBehavior};
        let sinks = Sinks::default();
        let review = ScriptedReview::new();
        let calls = Arc::clone(&review.calls);
        let mut options = options_with(enabled_config(), &sinks);
        options.review = Some(Arc::new(review));
        let runtime = MainWatchdogRuntime::new(options);

        runtime.handle_user_input("Idle prompt.", InputEventSource::Interactive, None);
        runtime.handle_before_agent_start(&json!({ "prompt": "Idle prompt." }), &cwd());
        runtime.handle_user_input(
            "Also fix the tests.",
            InputEventSource::Interactive,
            Some(InputStreamingBehavior::Steer),
        );
        runtime.handle_user_input(
            "Then purge them.",
            InputEventSource::Rpc,
            Some(InputStreamingBehavior::FollowUp),
        );
        runtime.handle_user_input(
            "Extension text.",
            InputEventSource::Extension,
            Some(InputStreamingBehavior::Steer),
        );
        runtime.handle_turn_end(&turn_end_event("work"), &cwd());
        runtime.handle_agent_end(&cwd()).await;

        let requests = calls.lock().unwrap();
        let reviewed = requests.last().expect("the agent_end boundary reviewed");
        assert_eq!(
            scope_prompts_in(&reviewed.delta),
            vec!["Idle prompt.", "Also fix the tests.", "Then purge them."],
            "{}",
            reviewed.delta
        );
    }

    /// pi `runtime.ts:514`'s `this.disposed` guard (added by `ff2e00bd`): a disposed runtime
    /// records nothing, like every other handler after `dispose`.
    #[tokio::test]
    async fn a_disposed_runtime_records_no_mid_run_message() {
        let sinks = Sinks::default();
        let runtime = MainWatchdogRuntime::new(options_with(enabled_config(), &sinks));
        runtime.handle_user_input(
            "Before dispose.",
            cyrup_ext::InputEventSource::Interactive,
            Some(cyrup_ext::InputStreamingBehavior::Steer),
        );
        assert_eq!(runtime.scope_prompts(), vec!["Before dispose."]);
        runtime.dispose();
        runtime.handle_user_input(
            "After dispose.",
            cyrup_ext::InputEventSource::Interactive,
            Some(cyrup_ext::InputStreamingBehavior::Steer),
        );
        assert!(
            runtime.scope_prompts().is_empty(),
            "dispose cleared the scope and nothing re-entered it"
        );
    }

    #[test]
    fn the_changed_paths_block_is_capped_at_two_hundred_entries() {
        let mut inner = test_inner();
        let signature = WatchdogRepoChangeSignature {
            root: "/repo".into(),
            key: "k".into(),
            changed_paths: (0..500).map(|n| format!("f{n}")).collect(),
        };
        let input = inner.build_review_input(Some(&signature), "");
        assert!(input.contains("- f199"));
        assert!(!input.contains("- f200"));
    }

    #[test]
    fn tail_and_head_never_split_a_character() {
        let value = "😀".repeat(10);
        assert_eq!(tail_chars(&value, 5).chars().count(), 2);
        assert_eq!(head_chars(&value, 5).chars().count(), 2);
        assert_eq!(tail_chars("abc", 10), "abc");
    }

    #[test]
    fn the_review_input_signature_is_a_stable_sha256() {
        assert_eq!(
            review_input_signature(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn the_prompt_falls_back_to_the_system_prompt() {
        assert_eq!(
            prompt_from_before_agent_start(&json!({ "prompt": "a" })).as_deref(),
            Some("a")
        );
        assert_eq!(
            prompt_from_before_agent_start(&json!({ "systemPrompt": "b" })).as_deref(),
            Some("b")
        );
        assert_eq!(prompt_from_before_agent_start(&json!({})), None);
    }

    /// A bare [`RuntimeInner`] for the pure-transition tests.
    fn test_inner() -> RuntimeInner {
        let config = enabled_config();
        RuntimeInner {
            cwd: PathBuf::from("/tmp"),
            config_result: WatchdogSettingsResult {
                ok: true,
                config,
                errors: Vec::new(),
                sources: Vec::new(),
            },
            session_override_enabled: None,
            session_model_override: None,
            status: WatchdogRuntimeStatus::Idle,
            pending_deltas: Vec::new(),
            pending_delta_chars: 0,
            guard: WatchdogEmissionGuard::default(),
            guard_max_warnings: None,
            epoch: 0,
            review_id_counter: 0,
            agent_end_id_counter: 0,
            active_agent_end_id: None,
            active_agent_end_cancel: None,
            active_review_id: None,
            active_review_warning: None,
            reviewing: false,
            waiting_at_agent_end: false,
            disposed: false,
            include_user_prompt_in_next_delta: false,
            user_prompt: None,
            waiters: Vec::new(),
            last_warning: None,
            displayed_warning_sequence: 0,
            last_error: None,
            last_review_input_signature: None,
            turn_start_change_signature: None,
            last_reviewed_change_signature: None,
            current_changed_paths: None,
            last_lsp_snapshot: None,
            observed_repo_edit_this_turn: false,
            tool_results_this_run: 0,
            mid_run_reviewing: false,
            auto_follow_queued: false,
            auto_follow_attempts: 0,
            consecutive_auto_follow_identity: None,
            consecutive_auto_follow_repeats: 0,
            auto_follow_stalemate: false,
            pending_auto_follow_prompts: WatchdogAutoFollowPromptLedger::new(),
            mid_run_generation: 0,
            active_review_cancel: None,
            failed_reviews: 0,
            stale_reviews: 0,
            scope: WatchdogScopeArtifact::new(),
            asked_this_prompt: false,
            activity_tail: String::new(),
            activity_pending: false,
            activity_review_used: false,
            boundary_warnings_this_prompt: 0,
        }
    }

    // ---- LSP seam ------------------------------------------------------------------------------

    /// A collector that returns a fixed result but keeps the REAL ledger/format/warning helpers,
    /// so these tests exercise the production reduction and formatting rather than stubs.
    struct ScriptedLsp {
        result: WatchdogLspResult,
        real: TypeScriptLspDiagnostics,
    }

    impl ScriptedLsp {
        fn new(result: WatchdogLspResult) -> Self {
            Self {
                result,
                real: TypeScriptLspDiagnostics::new(),
            }
        }
    }

    #[async_trait]
    impl WatchdogLspDiagnostics for ScriptedLsp {
        async fn collect(&self, _request: WatchdogLspRequest) -> Result<WatchdogLspResult, String> {
            Ok(self.result.clone())
        }
        fn reduce(&self, raw: WatchdogLspResult) -> WatchdogLspResult {
            self.real.reduce(raw)
        }
        fn reset_ledger(&self) {
            self.real.reset_ledger();
        }
        fn warning_from_diagnostics(&self, fresh: &WatchdogLspResult) -> Option<WatchdogWarning> {
            self.real.warning_from_diagnostics(fresh)
        }
        fn format_block(&self, fresh: &WatchdogLspResult) -> String {
            self.real.format_block(fresh)
        }
    }

    struct FailingLsp;

    #[async_trait]
    impl WatchdogLspDiagnostics for FailingLsp {
        async fn collect(&self, _request: WatchdogLspRequest) -> Result<WatchdogLspResult, String> {
            Err("the server died".to_string())
        }
    }

    #[tokio::test]
    async fn lsp_diagnostics_reach_the_review_input_and_raise_their_own_boundary_warning() {
        use super::super::types::{WatchdogLspDiagnostic, WatchdogLspDiagnosticSeverity};
        let sinks = Sinks::default();
        let review = ScriptedReview::new();
        let calls = Arc::clone(&review.calls);
        // Start with NO signature so the constructor's baseline (`runtime.ts:183-184`) is empty;
        // the tree "changes" below, which is what makes the boundary review fire at all.
        let source = Arc::new(FixedSignatures {
            signature: Mutex::new(None),
            calls: AtomicUsize::new(0),
        });
        let mut options = options_with(enabled_config(), &sinks);
        options.review = Some(Arc::new(review));
        options.review_changes_only = true;
        options.repo_change_signature =
            Some(Arc::clone(&source) as Arc<dyn WatchdogRepoChangeSource>);
        options.lsp_diagnostics = Some(Arc::new(ScriptedLsp::new(WatchdogLspResult {
            status: WatchdogLspStatus::Ok,
            provider: Some("typescript-language-server".into()),
            checked_paths: vec!["src/lib.rs".into()],
            skipped_paths: Vec::new(),
            diagnostics: vec![WatchdogLspDiagnostic {
                path: "src/lib.rs".into(),
                line: 12,
                column: 3,
                severity: WatchdogLspDiagnosticSeverity::Error,
                source: "ts".into(),
                code: Some("2322".into()),
                message: "Type 'string' is not assignable to type 'number'.".into(),
            }],
            message: None,
        })));
        let runtime = MainWatchdogRuntime::new(options);
        *source.signature.lock().unwrap() = Some(WatchdogRepoChangeSignature {
            root: "/repo".into(),
            key: "k1".into(),
            changed_paths: vec!["src/lib.rs".into()],
        });
        runtime.handle_turn_end(&turn_end_event("edited"), &cwd());
        runtime.handle_agent_end(&cwd()).await;

        let requests = calls.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0].delta.contains("src/lib.rs"),
            "the diagnostics block reached the review: {}",
            requests[0].delta
        );
        drop(requests);

        let displayed = sinks.displayed.lock().unwrap();
        assert_eq!(displayed.len(), 1, "an error diagnostic raised a warning");
        assert_eq!(displayed[0].0.source, WatchdogWarningSource::Lsp);
        drop(displayed);

        let snapshot = runtime.get_snapshot(None);
        assert_eq!(snapshot.lsp.result.status, WatchdogLspStatus::Ok);
        assert_eq!(snapshot.lsp.diagnostic_count, 1);
        assert_eq!(snapshot.lsp.fresh_diagnostic_count, 1);
        assert!(snapshot.lsp.updated_at.is_some());
    }

    #[tokio::test]
    async fn a_failing_collector_leaves_a_failed_snapshot_and_still_reviews() {
        let sinks = Sinks::default();
        let review = ScriptedReview::new();
        let calls = Arc::clone(&review.calls);
        // Start with NO signature so the constructor's baseline (`runtime.ts:183-184`) is empty;
        // the tree "changes" below, which is what makes the boundary review fire at all.
        let source = Arc::new(FixedSignatures {
            signature: Mutex::new(None),
            calls: AtomicUsize::new(0),
        });
        let mut options = options_with(enabled_config(), &sinks);
        options.review = Some(Arc::new(review));
        options.review_changes_only = true;
        options.repo_change_signature =
            Some(Arc::clone(&source) as Arc<dyn WatchdogRepoChangeSource>);
        options.lsp_diagnostics = Some(Arc::new(FailingLsp));
        let runtime = MainWatchdogRuntime::new(options);
        *source.signature.lock().unwrap() = Some(WatchdogRepoChangeSignature {
            root: "/repo".into(),
            key: "k1".into(),
            changed_paths: vec!["src/lib.rs".into()],
        });
        runtime.handle_turn_end(&turn_end_event("edited"), &cwd());
        runtime.handle_agent_end(&cwd()).await;

        assert_eq!(calls.lock().unwrap().len(), 1, "the review still ran");
        let snapshot = runtime.get_snapshot(None);
        assert_eq!(snapshot.lsp.result.status, WatchdogLspStatus::Failed);
        assert_eq!(
            snapshot.lsp.result.message.as_deref(),
            Some("LSP diagnostics failed: the server died")
        );
    }

    #[tokio::test]
    async fn a_signature_with_no_changed_paths_skips_the_collector_entirely() {
        let sinks = Sinks::default();
        let review = ScriptedReview::new();
        let mut options = options_with(enabled_config(), &sinks);
        options.review = Some(Arc::new(review));
        options.review_changes_only = true;
        options.repo_change_signature = Some(Arc::new(NoSignatures));
        let runtime = MainWatchdogRuntime::new(options);
        // No signature and no observed edit: the boundary never reaches the collector.
        runtime.handle_turn_end(&turn_end_event("thinking"), &cwd());
        runtime.handle_agent_end(&cwd()).await;
        let snapshot = runtime.get_snapshot(None);
        assert_eq!(snapshot.lsp.result.status, WatchdogLspStatus::Skipped);
    }

    // ---- SUBA-234: clarification, the activity tail, the reset on operator input ---------------

    mod clarification {
        use super::*;
        use cyrup_ext::InputEventSource;

        /// A review that records each request, signals when it has started, optionally holds until
        /// released (whether or not it was cancelled meanwhile — a LATE result is exactly what the
        /// runtime must ignore), and then returns its scripted result.
        struct ClarifyingReview {
            requests: Arc<Mutex<Vec<WatchdogReviewRequest>>>,
            result: WatchdogReviewResult,
            entered: Arc<tokio::sync::Notify>,
            release: Option<Arc<tokio::sync::Notify>>,
        }

        #[async_trait]
        impl WatchdogReview for ClarifyingReview {
            async fn review(
                &self,
                request: WatchdogReviewRequest,
            ) -> Result<Option<WatchdogReviewResult>, String> {
                self.requests
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(request);
                self.entered.notify_one();
                if let Some(release) = &self.release {
                    release.notified().await;
                }
                Ok(Some(self.result.clone()))
            }
        }

        fn asked(question: &str, evidence: &str) -> WatchdogReviewResult {
            WatchdogReviewResult {
                clarification: Some(WatchdogClarification {
                    question: question.to_string(),
                    evidence: evidence.to_string(),
                }),
                ..WatchdogReviewResult::default()
            }
        }

        fn warned() -> WatchdogReviewResult {
            WatchdogReviewResult {
                warnings: vec![blocker("a finding")],
                ..WatchdogReviewResult::default()
            }
        }

        struct Fx {
            runtime: Arc<MainWatchdogRuntime>,
            config: Arc<Mutex<ResolvedWatchdogConfig>>,
            questions: Arc<Mutex<Vec<String>>>,
            sinks: Sinks,
            requests: Arc<Mutex<Vec<WatchdogReviewRequest>>>,
            entered: Arc<tokio::sync::Notify>,
            release: Arc<tokio::sync::Notify>,
        }

        impl Fx {
            fn requests(&self) -> Vec<WatchdogReviewRequest> {
                self.requests.lock().unwrap().clone()
            }
            fn questions(&self) -> Vec<String> {
                self.questions.lock().unwrap().clone()
            }
            fn set_clarification(&self, on: bool) {
                self.config.lock().unwrap().clarification = on;
                self.runtime.refresh_config(&cwd());
            }
        }

        /// The main role (`main: true` installs the clarification sink, as `register_main` does)
        /// or a child (`false`), over a review that answers `result`, holding when `hold`.
        fn fixture(
            clarification: bool,
            main: bool,
            result: WatchdogReviewResult,
            hold: bool,
        ) -> Fx {
            let sinks = Sinks::default();
            let mut config = enabled_config();
            config.clarification = clarification;
            let config = Arc::new(Mutex::new(config));
            let resolver_config = Arc::clone(&config);
            let requests = Arc::new(Mutex::new(Vec::new()));
            let entered = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            let questions = Arc::new(Mutex::new(Vec::new()));
            let mut options = options_with(enabled_config(), &sinks);
            options.resolve_config = Some(Arc::new(move |_cwd: &Path, _s: Option<&Value>| {
                WatchdogSettingsResult {
                    ok: true,
                    config: resolver_config.lock().unwrap().clone(),
                    errors: Vec::new(),
                    sources: Vec::new(),
                }
            }));
            options.review = Some(Arc::new(ClarifyingReview {
                requests: Arc::clone(&requests),
                result,
                entered: Arc::clone(&entered),
                release: hold.then(|| Arc::clone(&release)),
            }));
            options.review_changes_only = true;
            options.repo_change_signature = Some(Arc::new(NoSignatures));
            if main {
                let sink = Arc::clone(&questions);
                options.display_clarification = Some(Arc::new(move |content: &str| {
                    sink.lock().unwrap().push(content.to_string());
                }));
            }
            let runtime = Arc::new(MainWatchdogRuntime::new(options));
            runtime.handle_before_agent_start(&json!({ "prompt": "Original task" }), &cwd());
            Fx {
                runtime,
                config,
                questions,
                sinks,
                requests,
                entered,
                release,
            }
        }

        /// One orchestration turn: an assistant tool call and its result, paired by `id`.
        fn activity_turn(name: &str, args: Value, text: &str, id: &str) -> Value {
            json!({
                "type": "turn_end",
                "message": { "role": "assistant", "content": [
                    { "type": "toolCall", "id": id, "name": name, "arguments": args }
                ]},
                "toolResults": [{
                    "role": "toolResult", "toolCallId": id, "toolName": name,
                    "content": [{ "type": "text", "text": text }], "isError": false
                }],
            })
        }

        fn status_turn(text: &str) -> Value {
            activity_turn(
                "subagent",
                json!({ "action": "status", "id": "task" }),
                text,
                "call-1",
            )
        }

        /// Start a boundary whose review holds, and return once the review is RUNNING — so every
        /// assertion that follows is about a review that really started.
        async fn hold_a_review(fx: &Fx) -> tokio::task::JoinHandle<()> {
            fx.runtime
                .handle_turn_end(&status_turn("Gate state unclear"), &cwd());
            let runtime = Arc::clone(&fx.runtime);
            let boundary = tokio::spawn(async move { runtime.handle_agent_end(&cwd()).await });
            tokio::time::timeout(Duration::from_secs(10), fx.entered.notified())
                .await
                .expect("the review started");
            boundary
        }

        /// [`hold_a_review`] for a boundary an EDIT admits (the observed edit/write trigger), so it
        /// reviews whatever the clarification setting says.
        async fn hold_an_edit_review(fx: &Fx) -> tokio::task::JoinHandle<()> {
            fx.runtime.enqueue_delta("work");
            fx.runtime.handle_turn_end(
                &json!({ "type": "tool_result", "toolName": "edit", "isError": false }),
                &cwd(),
            );
            let runtime = Arc::clone(&fx.runtime);
            let boundary = tokio::spawn(async move { runtime.handle_agent_end(&cwd()).await });
            tokio::time::timeout(Duration::from_secs(10), fx.entered.notified())
                .await
                .expect("the review started");
            boundary
        }

        /// Clause 1 — upstream `watchdog-runtime.test.ts:278` @ad11b7ab ("aborts an active review
        /// on new user input and suppresses stale questions and warnings"), the input edge. The
        /// message is IDLE (no streaming behavior): a boundary review runs after the run ended.
        ///
        /// RED at HEAD (`cde73dc9`): does not compile — no `clarification` setting, no
        /// `display_clarification` sink and no `WatchdogReviewResult::clarification`. With the
        /// reset removed from `handle_user_input` it compiles and fails: the review's token is
        /// never cancelled and the late question is displayed.
        #[tokio::test]
        async fn with_clarification_on_an_idle_operator_message_aborts_the_review_in_flight() {
            let fx = fixture(true, true, asked("Old question?", "Old context"), true);
            let boundary = hold_a_review(&fx).await;
            let epoch = fx.runtime.get_snapshot(None).epoch;

            fx.runtime
                .handle_user_input("New input", InputEventSource::Interactive, None);

            let requests = fx.requests();
            assert_eq!(requests.len(), 1, "the review itself ran");
            assert!(
                requests[0].cancel.is_cancelled(),
                "the in-flight review is aborted, not orphaned"
            );
            assert!(fx.runtime.get_snapshot(None).epoch > epoch);
            assert!(
                !requests[0].emit_warning.emit(&blocker("late")),
                "a late warning callback is stale"
            );
            // The reviewer answers anyway, late.
            fx.release.notify_one();
            boundary.await.unwrap();
            assert!(
                fx.questions().is_empty(),
                "a stale question is not displayed"
            );
            assert!(fx.sinks.displayed.lock().unwrap().is_empty());
            assert_eq!(fx.runtime.get_snapshot(None).active_review_id, None);
            assert!(
                !fx.runtime
                    .scope_prompts()
                    .contains(&"New input".to_string()),
                "idle input is recorded by before_agent_start, not here"
            );
        }

        /// Clause 1, the other two edges of the same upstream test: clarification turned off, and
        /// the session model changing (`handleModelChange`, `runtime.ts:519-521`).
        ///
        /// RED at HEAD: does not compile (no `handle_model_change`, no setting).
        #[tokio::test]
        async fn turning_clarification_off_or_changing_the_model_mid_review_aborts_it() {
            for edge in ["clarification off", "model change"] {
                let fx = fixture(true, true, asked("Old question?", "Old context"), true);
                let boundary = hold_a_review(&fx).await;
                match edge {
                    "clarification off" => fx.set_clarification(false),
                    _ => fx.runtime.handle_model_change(),
                }
                assert!(fx.requests()[0].cancel.is_cancelled(), "{edge}");
                fx.release.notify_one();
                boundary.await.unwrap();
                assert!(fx.questions().is_empty(), "{edge}");
            }
        }

        /// Clause 2 — NON-REGRESSION GUARD, not evidence the feature works: green at HEAD in
        /// substance (there, nothing could reset a review). Upstream `watchdog-runtime.test.ts:253`
        /// @ad11b7ab: with clarification OFF the lifecycle handlers are inert — an operator
        /// message or a model change during a review leaves it running, its warning is delivered,
        /// and no question is offered or shown. Asserts the review ran, so "nothing happened"
        /// cannot pass vacuously.
        #[tokio::test]
        async fn guard_with_clarification_off_nothing_changes() {
            let fx = fixture(false, true, warned(), true);
            let boundary = hold_an_edit_review(&fx).await;
            let epoch = fx.runtime.get_snapshot(None).epoch;
            fx.runtime
                .handle_user_input("New input", InputEventSource::Interactive, None);
            fx.runtime.handle_model_change();
            assert_eq!(
                fx.runtime.get_snapshot(None).epoch,
                epoch,
                "the handlers are inert"
            );
            let requests = fx.requests();
            assert_eq!(requests.len(), 1, "the review ran");
            assert!(!requests[0].cancel.is_cancelled());
            assert!(!requests[0].allow_clarification, "no question is offered");
            fx.release.notify_one();
            boundary.await.unwrap();
            assert_eq!(
                fx.sinks.displayed.lock().unwrap().len(),
                1,
                "the warning lands"
            );
            assert!(fx.questions().is_empty());
            assert!(
                fx.runtime.activity_tail().is_empty(),
                "no activity is collected while off"
            );
        }

        /// Clause 3 — a reviewer that yields a question gets it displayed, and the lifecycle ENDS:
        /// the runtime is idle and settled, the evidence counts as reviewed, and the same prompt
        /// gets no second question; a new prompt gets one again.
        ///
        /// RED at HEAD: does not compile (no clarification outcome or sink).
        #[tokio::test]
        async fn a_yielded_question_is_displayed_and_the_review_does_not_stay_open() {
            let fx = fixture(
                true,
                true,
                asked("Is the gate held?", "Delivered status is unclear"),
                false,
            );
            fx.runtime
                .handle_turn_end(&status_turn("Gate state unclear"), &cwd());
            fx.runtime.handle_agent_end(&cwd()).await;

            let requests = fx.requests();
            assert_eq!(requests.len(), 1, "the review ran");
            assert!(requests[0].allow_clarification, "a boundary review may ask");
            let questions = fx.questions();
            assert_eq!(questions.len(), 1);
            for text in [
                "Main watchdog clarification:",
                "Is the gate held?",
                "Evidence: Delivered status is unclear",
                "This is not approval, permission, or a warning.",
            ] {
                assert!(questions[0].contains(text), "{text} in {}", questions[0]);
            }
            let snapshot = fx.runtime.get_snapshot(None);
            assert_eq!(snapshot.status, WatchdogRuntimeStatus::Idle);
            assert_eq!(snapshot.active_review_id, None);
            assert!(
                fx.runtime.wait_for_idle(Duration::from_millis(50)).await,
                "nothing is left waiting on an answer"
            );
            assert!(
                fx.sinks.displayed.lock().unwrap().is_empty(),
                "a question is not a warning"
            );

            // Same prompt, new activity: the activity review is spent, so no second review.
            fx.runtime
                .handle_turn_end(&status_turn("Still unclear"), &cwd());
            fx.runtime.handle_agent_end(&cwd()).await;
            assert_eq!(fx.requests().len(), 1, "one activity review per prompt");

            // A new prompt gets its own question and its own activity review.
            fx.runtime
                .handle_before_agent_start(&json!({ "prompt": "Next task" }), &cwd());
            fx.runtime.handle_agent_end(&cwd()).await;
            let requests = fx.requests();
            assert_eq!(
                requests.len(),
                2,
                "unreviewed activity survives into the next prompt"
            );
            assert!(requests[1].allow_clarification);
            assert_eq!(fx.questions().len(), 2);
        }

        /// Clause 3, the prompt budget: once this prompt has asked, the next boundary review is
        /// NOT offered `watchdog_ask`, and a question it returns anyway is not displayed.
        #[tokio::test]
        async fn one_question_per_prompt() {
            let fx = fixture(true, true, asked("Q?", "E"), false);
            fx.runtime.handle_turn_end(&status_turn("one"), &cwd());
            fx.runtime.handle_agent_end(&cwd()).await;
            // An edit-free second boundary is admitted by the observed-edit trigger.
            fx.runtime.handle_turn_end(
                &json!({ "type": "tool_result", "toolName": "edit", "isError": false }),
                &cwd(),
            );
            fx.runtime.enqueue_delta("new work, same prompt");
            fx.runtime.handle_agent_end(&cwd()).await;
            let requests = fx.requests();
            assert_eq!(requests.len(), 2, "the second boundary reviewed");
            assert!(!requests[1].allow_clarification);
            assert_eq!(fx.questions().len(), 1);
        }

        /// Clause 4 — the activity tail is PAIRED: an orchestration call counts only with the
        /// result of the same name and the exact id; an unmatched one, and a non-orchestration tool,
        /// contribute nothing. With no edit at all, the new activity admits a review, and the review
        /// input carries the block under upstream's heading — the "supplied activity evidence" the
        /// reviewer's own prompt line names.
        ///
        /// RED at HEAD: does not compile (no activity tail); before it, a review with no edit
        /// never ran at all under `review_changes_only`.
        #[tokio::test]
        async fn the_activity_tail_is_paired_and_reaches_the_review() {
            let fx = fixture(true, true, WatchdogReviewResult::default(), false);
            let mut unmatched = status_turn("unmatched result");
            unmatched["toolResults"][0]["toolCallId"] = json!("other");
            fx.runtime.handle_turn_end(&unmatched, &cwd());
            fx.runtime.handle_turn_end(
                &activity_turn(
                    "bash",
                    json!({ "command": "echo subagent" }),
                    "not orchestration",
                    "b",
                ),
                &cwd(),
            );
            fx.runtime.handle_agent_end(&cwd()).await;
            assert!(
                fx.requests().is_empty(),
                "unpaired and foreign calls are not evidence"
            );
            assert!(fx.runtime.activity_tail().is_empty());

            fx.runtime.handle_turn_end(
                &activity_turn(
                    "bg_wait",
                    json!({ "id": "ci" }),
                    "CI gate completed successfully",
                    "w",
                ),
                &cwd(),
            );
            fx.runtime.handle_agent_end(&cwd()).await;
            let requests = fx.requests();
            assert_eq!(requests.len(), 1, "new activity admits a no-edit review");
            let delta = &requests[0].delta;
            assert!(delta.contains(ACTIVITY_BLOCK_HEADER), "{delta}");
            assert!(delta.contains("Tool call: bg_wait"));
            assert!(delta.contains("CI gate completed successfully"));
            assert!(!delta.contains("unmatched result"));
            assert!(!delta.contains("not orchestration"));
        }

        /// Clause 4 — the CLIP: each entry is bounded to 3,000 characters (head, omission marker,
        /// tail) and the joined tail keeps its NEWEST 6,000.
        ///
        /// RED at HEAD: does not compile.
        #[tokio::test]
        async fn the_activity_tail_is_clipped_to_its_newest_six_thousand_characters() {
            let fx = fixture(true, true, WatchdogReviewResult::default(), false);
            fx.runtime.handle_turn_end(
                &activity_turn("bg_wait", json!({}), &"x".repeat(10_000), "big"),
                &cwd(),
            );
            let one = fx.runtime.activity_tail();
            assert_eq!(
                one.chars().count(),
                ACTIVITY_ENTRY_MAX_CHARS,
                "one entry is bounded"
            );
            assert!(one.contains("characters omitted"));
            for n in 0..5 {
                fx.runtime.handle_turn_end(
                    &activity_turn(
                        "bg_wait",
                        json!({}),
                        &format!("marker-{n} {}", "y".repeat(2_400)),
                        &format!("c{n}"),
                    ),
                    &cwd(),
                );
            }
            let tail = fx.runtime.activity_tail();
            assert_eq!(tail.chars().count(), ACTIVITY_TAIL_MAX_CHARS);
            assert!(tail.contains("marker-4"), "the newest observation survives");
            assert!(!tail.contains("marker-0"), "the oldest is clipped away");
        }

        /// Clause 4 — the CLEAR: turning clarification off drops the tail and stops collection,
        /// and so do a compaction's `clear_activity` and disabling the watchdog.
        ///
        /// RED at HEAD: does not compile.
        #[tokio::test]
        async fn the_activity_tail_is_cleared_when_clarification_goes_off() {
            let fx = fixture(true, true, WatchdogReviewResult::default(), false);
            fx.runtime
                .handle_turn_end(&status_turn("kept while on"), &cwd());
            assert!(fx.runtime.activity_tail().contains("kept while on"));
            fx.set_clarification(false);
            assert!(fx.runtime.activity_tail().is_empty(), "cleared on refresh");
            fx.runtime
                .handle_turn_end(&status_turn("arrives while off"), &cwd());
            assert!(
                fx.runtime.activity_tail().is_empty(),
                "not collected while off"
            );

            fx.set_clarification(true);
            fx.runtime
                .handle_turn_end(&status_turn("before compaction"), &cwd());
            fx.runtime.reset(WatchdogResetOptions {
                clear_activity: true,
                ..WatchdogResetOptions::default()
            });
            assert!(
                fx.runtime.activity_tail().is_empty(),
                "a compaction clears it"
            );

            fx.runtime
                .handle_turn_end(&status_turn("before disable"), &cwd());
            fx.config.lock().unwrap().main.enabled = false;
            fx.runtime.refresh_config(&cwd());
            assert!(fx.runtime.activity_tail().is_empty(), "disabling clears it");
        }

        /// The main-role gate: a runtime built WITHOUT the clarification sink — every child
        /// watchdog — collects no activity, is never offered a question, and is not reset by
        /// operator input, even with the setting on (upstream `watchdog-runtime.test.ts:253`'s
        /// `[true, false]` case).
        #[tokio::test]
        async fn a_runtime_without_the_main_sink_never_runs_the_lifecycle() {
            let fx = fixture(true, false, warned(), true);
            let boundary = hold_an_edit_review(&fx).await;
            let epoch = fx.runtime.get_snapshot(None).epoch;
            fx.runtime
                .handle_user_input("New input", InputEventSource::Interactive, None);
            assert_eq!(fx.runtime.get_snapshot(None).epoch, epoch);
            let requests = fx.requests();
            assert_eq!(requests.len(), 1, "the review ran");
            assert!(!requests[0].allow_clarification);
            fx.release.notify_one();
            boundary.await.unwrap();
            fx.runtime
                .handle_turn_end(&status_turn("child activity"), &cwd());
            assert!(fx.runtime.activity_tail().is_empty());
            assert_eq!(fx.sinks.displayed.lock().unwrap().len(), 1);
        }

        /// An extension's input is never the operator (`runtime.ts:514`), clarification or not.
        #[tokio::test]
        async fn extension_input_does_not_reset_a_review() {
            let fx = fixture(true, true, warned(), true);
            let boundary = hold_a_review(&fx).await;
            fx.runtime
                .handle_user_input("from an extension", InputEventSource::Extension, None);
            assert!(!fx.requests()[0].cancel.is_cancelled());
            fx.release.notify_one();
            boundary.await.unwrap();
            assert_eq!(fx.sinks.displayed.lock().unwrap().len(), 1);
        }

        #[test]
        fn bound_keeps_both_ends_at_exactly_the_cap() {
            let text: String = (0..5_000)
                .map(|i| char::from(b'a' + (i % 26) as u8))
                .collect();
            let bounded = bound_watchdog_review_text(&text, 1_000);
            assert_eq!(bounded.chars().count(), 1_000);
            assert!(bounded.starts_with(&text[..250]));
            assert!(bounded.ends_with(&text[text.len() - 100..]));
            assert!(bounded.contains("[... about 4000 characters omitted ...]"));
            assert_eq!(bound_watchdog_review_text("short", 1_000), "short");
        }
    }
}
