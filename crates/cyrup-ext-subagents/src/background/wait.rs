//! The `bg_wait` tool (SUBA-004): block the current turn until outstanding background subagent runs
//! finish — a port of pi-subagents' `src/runs/background/wait.ts` (present at the ported
//! v0.33.x–v0.34.0 baseline; added upstream by `05019cd`, first shipped in v0.33.0).
//!
//! Background subagent runs are detached: hop-1 spawns a real hop-2 runner process and returns
//! immediately. In an interactive session the orchestrator can end its turn and be woken by a
//! completion notification. That does not work when the orchestrator is a skill that must run to
//! completion, and cannot work at all non-interactively (`cyrup -p …`), where the whole task is a
//! single turn: once it ends, nothing is left to receive the notification. `wait` closes that gap
//! by keeping the turn alive until a tracked run reaches a terminal state.
//!
//! # Why blocking here is safe (the two escape hatches)
//!
//! A blocking primitive with no way out would let one wedged child hang the orchestrator forever.
//! There are two independent ways this loop always terminates:
//!
//! 1. **Timeout** — [`WaitParams::timeout_ms`], defaulting to [`DEFAULT_TIMEOUT_MS`] (30 minutes,
//!    pi's `DEFAULT_TIMEOUT_MS`). On expiry the wait returns an error result naming the runs still
//!    in flight. The runs are detached and keep going; only the *waiting* stops.
//! 2. **Cancellation** — the host's [`CancelToken`] for this tool call (pi's `AbortSignal`). The
//!    sleep is a `select!` against [`CancelToken::cancelled`], so aborting the turn wakes the loop
//!    immediately rather than after the remaining poll interval, and it returns an error result
//!    naming what was still active. Dropping the future (the host abandoning the call) likewise
//!    tears everything down — this loop owns no task, no thread and no spawned work.
//!
//! A third, quieter guarantee: with [`WaitDeps::stop_on_attention`] set (the default, and the
//! `bg_wait` tool's own mode), a run that trips the `needs_attention` heuristic ALSO ends the wait, in
//! either mode. A child that went idle or blocked on a decision would otherwise stall the loop
//! until the timeout, and the caller is exactly who has to act on it. Auto-drain is the documented
//! exception: with the flag off it waits THROUGH attention instead of resolving on it, bounded by
//! its own deadline rather than short-circuiting early — see the flag's own doc for why.
//!
//! # Wake mechanism (SUBA-034)
//!
//! pi subscribes to its in-process event bus so a completion wakes `wait` at once, with the poll as
//! a reconciliation fallback — and pi is explicit that the poll, not the event, is the source of
//! truth for what changed ("With no bus, `wait` degrades to pure polling"). cyrup now runs that
//! same two-part shape: [`WaitDeps::completion_bus`] carries the orchestrator's
//! [`super::watch::CompletionBus`], the loop `select!`s the subscription against the sleep, and
//! every wake — from either arm — re-reads authoritative state from disk through the same R-SA-079
//! reconciliation gate every other control action uses ([`super::run_status::list_active_runs`]).
//! The event is never itself the answer.
//!
//! **The delta that survives, and it is a latency floor rather than a mechanism difference.** pi's
//! publisher is the run itself (in-process), so upstream's wake is immediate. cyrup's runs are
//! detached OS processes whose only completion signal is the terminal [`super::ResultFile`]
//! (R-SA-077), and the in-process thing that first learns of it is
//! [`super::watch::ResultsWatcher`] — so a wake here is bounded below by that watcher's own
//! 500 ms [`super::watch::RESULTS_DIR_POLL_INTERVAL`]. What the bus removes is the SECOND,
//! independent [`DEFAULT_POLL_INTERVAL_MS`] (1 s, pi's own value) stacked on top of it. See
//! [`super::watch::CompletionBus`] for the `[CYRUP-DELTA]` and for why closing the remaining
//! 500 ms is a separate R-SA-098 decision.
//!
//! `completion_bus: None` is upstream's own no-bus degradation — pure polling, exactly as before.
//!
//! # Payload resolution
//!
//! A resolved wait reports the terminal PAYLOADS it covered, not merely a count:
//! [`super::wait_completions::collect_wait_completions`] projects every initially-tracked run that
//! reached a terminal state into a [`super::wait_completions::WaitCompletion`], which is what lands
//! on [`WaitOutcome::completions`] and, from there, on the tool result's `details.completions`.
//!
//! Resolution goes through [`super::result_index::result_payload_path_for_session_run`], which is
//! session-partitioned: a **staged** payload is visible to the session that owns it, and another
//! session's staged payload is not resolvable through this path at all.
//!
//! The in-process [`super::wait_completions::WaitCompletionStore`] is consulted FIRST — it is what
//! survives the completion watcher's delete-last, because it is registered ahead of
//! [`super::watch::CompletionBus`] in the watcher's composite observer
//! (`extension/executor/notices.rs`'s `CompositeCompletionObserver`, where
//! `self.wait_completions()` is the FIRST member). That ordering is what closes the wake-then-read
//! race:
//! a wait that wakes on the bus and immediately re-reads finds the store already holding what the
//! watcher is about to delete.
//!
//! # Resolving a completion after cleanup (SUBA-056)
//!
//! A resolved wait reports each terminal run's completion through
//! [`super::wait_completions::collect_wait_completions`], which resolves in three rungs: the
//! in-process record, the payload on disk, and — when the watcher has already delivered and
//! UNLINKED the payload — the durable replay record in [`super::completion_replay`]. The record is
//! written BEFORE the unlink (pi `result-watcher.ts:432-433`), which is the entire reason a `wait`
//! arriving after cleanup resolves instead of reporting a completion it demonstrably observed as
//! absent. It survives a process restart, which the in-process record does not, and expires at
//! [`super::watch::DEDUP_TTL`].
//!
//! # Scoping (SUBA-031)
//!
//! pi scopes `bg_wait` to `state.currentSessionId` (`activeRunsForSession` passes
//! `sessionId: deps.state.currentSessionId ?? undefined` into `listAsyncRuns`,
//! `subagent-wait.ts:265`), and cyrup now does the same through [`WaitDeps::session_id`]. The cwd
//! partition (`async_root`, derived per-cwd by [`super::run_artifact_roots`]) is still the outer
//! one; the session filter is the inner one, exactly as upstream layers them — pi's async root is
//! also per-scope and the session id narrows within it.
//!
//! This is the difference between "two cyrup sessions in the same repo block on each other's
//! background runs" and pi's behaviour, and it is also what makes the empty-set text
//! ("No active async runs **in this session**.") true; before the filter existed the message said
//! the opposite of what the code did.
//!
//! That reasoning is **not specific to `wait`**. It applies to every surface reading these
//! per-cwd roots, and for a long time `wait` was the only one that had it: result delivery
//! (`background/watch/`), the job tracker's restore path and the control operations all listed the
//! same shared directory with no ownership check at all. They now share one implementation of the
//! rule — [`crate::background::delivery::SessionGate`] for "may I act on this run?" and
//! [`crate::background::delivery::OwnershipSnapshot::owns`] for "may I consume this completion?".
//!
//! `session_id: None` (headless / unpersisted orchestrator) applies no filter, which is pi's own
//! falsy-`sessionId` path — see [`super::run_status::list_active_runs`] for why an unattributed run
//! is dropped when a filter IS supplied.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use cyrup_core::CancelToken;

use super::run_status::{ActiveRun, list_active_runs};
use super::wait_completions::WaitCompletion;
use super::{ActivityState, RunState};

/// States that mean a run is still in flight (pi `ACTIVE_STATES`, `runs/background/wait.ts:55` @v0.34.0). Matches exactly
/// what [`list_active_runs`] already returns.
const fn is_active(state: RunState) -> bool {
    matches!(state, RunState::Queued | RunState::Running)
}

/// pi `DEFAULT_TIMEOUT_MS` (`runs/background/wait.ts:57` @v0.34.0) — 30 minutes.
pub const DEFAULT_TIMEOUT_MS: u64 = 30 * 60 * 1000;
/// pi `MIN_POLL_INTERVAL_MS` (`runs/background/wait.ts:58` @v0.34.0) — the floor a caller-supplied interval is clamped to.
pub const MIN_POLL_INTERVAL_MS: u64 = 250;
/// pi `DEFAULT_POLL_INTERVAL_MS` (`runs/background/wait.ts:59` @v0.34.0).
pub const DEFAULT_POLL_INTERVAL_MS: u64 = 1000;

/// pi `WAIT_TOOL_ENABLED_ENV` (`runs/background/wait.ts:61` @v0.34.0), renamed into cyrup's `CYRUP_SUBAGENT_*` family.
pub const WAIT_TOOL_ENABLED_ENV: &str = "CYRUP_SUBAGENT_WAIT_TOOL_ENABLED";

const WAIT_TOOL_TRUE_VALUES: [&str; 5] = ["1", "true", "yes", "on", "enabled"];
const WAIT_TOOL_FALSE_VALUES: [&str; 5] = ["0", "false", "no", "off", "disabled"];

/// pi `parseWaitToolEnabledEnv` (`runs/background/wait.ts:70-77` @v0.34.0): a set vocabulary, and anything else is a hard
/// configuration error rather than a silently-ignored value.
///
/// # Errors
///
/// Returns pi's message when the value is set but is none of the accepted spellings.
pub fn parse_wait_tool_enabled_env(value: Option<&str>) -> Result<Option<bool>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let normalized = value.trim().to_lowercase();
    if WAIT_TOOL_TRUE_VALUES.contains(&normalized.as_str()) {
        return Ok(Some(true));
    }
    if WAIT_TOOL_FALSE_VALUES.contains(&normalized.as_str()) {
        return Ok(Some(false));
    }
    Err(format!(
        "{WAIT_TOOL_ENABLED_ENV} must be one of true/false, 1/0, yes/no, on/off, or \
         enabled/disabled."
    ))
}

/// pi's `config.waitTool`, which accepts either a bare boolean or `{ enabled?: boolean }`
/// (`configWaitToolEnabled`, `runs/background/wait.ts:78-88` @v0.34.0).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WaitToolSetting {
    /// `"waitTool": true` / `false`.
    Enabled(bool),
    /// `"waitTool": { "enabled": … }`.
    Object {
        /// The gate; omitted means "no opinion", so the default (enabled) applies.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enabled: Option<bool>,
    },
}

impl WaitToolSetting {
    /// The configured opinion, if any.
    #[must_use]
    pub fn enabled(&self) -> Option<bool> {
        match self {
            WaitToolSetting::Enabled(flag) => Some(*flag),
            WaitToolSetting::Object { enabled } => *enabled,
        }
    }
}

/// pi `resolveWaitToolConfig` (`runs/background/wait.ts:90-94` @v0.34.0): env wins over `config.waitTool`, and the tool is
/// enabled when neither says otherwise.
///
/// # Errors
///
/// Propagates [`parse_wait_tool_enabled_env`]'s rejection of an unrecognized env value.
pub fn resolve_wait_tool_enabled(
    config: Option<&WaitToolSetting>,
    env_value: Option<&str>,
) -> Result<bool, String> {
    Ok(parse_wait_tool_enabled_env(env_value)?
        .or_else(|| config.and_then(WaitToolSetting::enabled))
        .unwrap_or(true))
}

/// The `bg_wait` tool's parameter object (pi `WaitParams`, `runs/background/wait.ts:96-108` @v0.34.0).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WaitParams {
    /// Run id (or unambiguous prefix) to wait for. Omitted = every active run in this cwd.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Block until EVERY initially-active run is terminal. Default `false` = return as soon as the
    /// first one finishes, so a fleet manager can spawn a replacement and wait again. Ignored when
    /// `id` targets a single run (which always means "wait for that one").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub all: Option<bool>,
    /// Give up after this many milliseconds ([`DEFAULT_TIMEOUT_MS`] when unset or non-positive).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// pi `SubagentWaitParams.nonBlocking` (`extension/schemas.ts:407-409`): resolve `id` once,
    /// persist an exact-run wake subscription ([`super::wait_subscriptions`]) and return
    /// IMMEDIATELY instead of blocking. The session is then woken on completion, failure,
    /// attention, reconciliation failure or timeout — including in a later turn.
    ///
    /// Requires `id` and is incompatible with `all` (pi `subagent-wait.ts:561-566`): a
    /// registration binds ONE exact run identity, which a fleet-wide wait has no way to name.
    ///
    /// ⚠ The tool's JSON Schema (`extension/wait_tool.rs`'s `wait_tool_parameters`) ends with
    /// `"additionalProperties": false` and this struct carries `#[serde(default)]` without
    /// `deny_unknown_fields` — so editing one and not the other is a silent no-op in OPPOSITE
    /// directions (host rejection versus a serde drop). They move together or not at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub non_blocking: Option<bool>,
}

/// Injected environment for [`wait_for_subagents`] — the two run-storage roots, the poll cadence,
/// and the config gate.
#[derive(Clone, Debug)]
pub struct WaitDeps {
    /// The per-cwd async root holding every run directory ([`super::run_artifact_roots`]).
    pub async_root: PathBuf,
    /// The per-cwd results dir holding terminal [`super::ResultFile`]s.
    pub results_dir: PathBuf,
    /// Poll cadence, clamped to at least [`MIN_POLL_INTERVAL_MS`]. Tests drive this well below the
    /// 1s production default.
    pub poll_interval: Duration,
    /// `false` makes the tool return immediately without blocking (pi `deps.enabled`).
    pub enabled: bool,
    /// SUBA-031 — the live orchestrator session (pi `deps.state.currentSessionId`), narrowing every
    /// listing this wait performs. `None` applies no filter; see the module docs.
    pub session_id: Option<String>,
    /// SUBA-034 — the in-process completion bus this wait wakes on (pi's event-bus subscription,
    /// `runs/background/wait.ts`'s `onAsyncComplete` listener). `None` is upstream's own documented
    /// no-bus degradation to pure polling, and is what every construction that has no live watcher
    /// (tests, headless embedders) supplies.
    pub completion_bus: Option<crate::background::watch::CompletionBus>,
    /// The in-process record of payloads this process's watcher already consumed and deleted (pi
    /// `deps.state.completedResults`, read by `collectWaitCompletions` at
    /// `wait-completions.ts:164`). Populated by
    /// [`crate::background::wait_completions::WaitCompletionStore`]'s
    /// [`crate::background::watch::CompletionObserver`] impl, registered FIRST in the watcher's
    /// composite (`extension/executor/notices.rs`'s `CompositeCompletionObserver`).
    ///
    /// `Arc` rather than a borrow because [`WaitDeps`] is `Clone` and constructed well before the
    /// wait runs.
    ///
    /// **Not `Option`, unlike [`Self::completion_bus`].** For the bus the two states differ
    /// behaviourally — no subscription at all versus a subscription that never fires — so `None`
    /// says something `Some(empty)` cannot. For the store they are the same state: an empty map
    /// IS "this process has consumed nothing", which is exactly right for every caller with no
    /// live watcher (headless embedders, harnesses) and yields the pre-WORKFLOW_4 behaviour —
    /// resolution falls straight through to the on-disk rungs (`wait_completions/collect.rs:43-47`).
    pub wait_completions: std::sync::Arc<crate::background::wait_completions::WaitCompletionStore>,
    /// pi `deps.stopOnAttention` (`subagent-wait.ts:116,653,657`). `true` — the default and the `wait`
    /// tool's behaviour — makes a needs-attention run end the wait. `false` is auto-drain's mode:
    /// the drain re-enters this wait in a loop, so returning early on attention would spin it hot
    /// for the whole drain deadline.
    ///
    /// `[CYRUP-DELTA]` upstream's escape with the flag off — `stopOnAttention ||
    /// hasSupervisorTool(run)` (`:618-622`), so attention on an intercom-capable child still
    /// breaks the wait — has no cyrup analog, because [`ActiveRun`] carries no per-run tool
    /// inventory. With the flag off, cyrup waits through ALL attention, so such a child is bounded
    /// by the drain's 30-minute deadline rather than surfacing at once.
    pub stop_on_attention: bool,
    /// pi `deps.failOnFailedRuns` (`subagent-wait.ts:118,751`): report a resolved wait as an ERROR
    /// when any initially-tracked run ended failed. Off for the tool (a failed run is information,
    /// not a tool failure); on for the drain, whose contract is "everything landed, or say so".
    pub fail_on_failed_runs: bool,
    /// pi `deps.failOnAttention` (`subagent-wait.ts:120,768`): likewise for unresolved attention.
    pub fail_on_attention: bool,
    /// `ASYNC_NOTIFY_BUG_REPORT` F3 — the claim/answer ledger shared with the completion
    /// watcher's delivery decorator ([`crate::background::watch::InlineAnsweredSink`]). A live
    /// wait CLAIMS the runs it is about to block on (holding same-run deliveries off before their
    /// irrevocable enqueue — RC4) and records which of them its own response actually answered,
    /// so the watcher suppresses a standalone notification whose value the transcript already
    /// carries. `None` — the default, and what every construction that wired no ledger gets
    /// (tests, headless embedders) — changes nothing: no claim is ever taken and every delivery
    /// proceeds exactly as before, mirroring [`Self::completion_bus`]'s no-bus degradation.
    pub inline_answers: Option<crate::background::watch::InlineAnswerLedger>,
    /// SCOPE_11 — the durable-subscription arming seam, pi's `deps.subscribe`
    /// (`subagent-wait.ts:597`), supplied by `wait-tool.ts:33` as
    /// `...(subscriptions && ctx?.hasUI ? { subscribe: (input) => subscriptions.arm(input) } : {})`.
    ///
    /// `None` is upstream's own no-manager state and carries upstream's own refusal: a runtime
    /// with no long-lived manager, or with no UI, cannot honour a wake that arrives after the
    /// turn ends, so `nonBlocking` is REFUSED rather than silently downgraded to a blocking wait.
    /// That mirrors [`Self::completion_bus`]'s no-bus degradation in shape while differing in
    /// kind, deliberately: a missing bus costs latency, a missing manager costs the whole
    /// guarantee the caller asked for.
    pub subscribe: Option<WaitSubscribeHook>,
    /// VL-S11b — pi `deps.state.foregroundRuns` (`subagent-wait.ts:226`): the in-memory remembered
    /// foreground runs this session holds, which is where a DETACHED run lives (upstream never
    /// writes one to disk — `foreground-history.ts:67-69` — and cyrup's `persist.rs` refuses one).
    ///
    /// `None` is the no-executor degradation every other injected seam here has, and it means
    /// exactly what it meant before this field existed: the candidate set is async runs only.
    pub detached_foreground: Option<DetachedForegroundHook>,
}

/// [`WaitDeps::subscribe`]'s payload — pi's `deps.subscribe` closure.
///
/// A newtype rather than a bare `Arc<dyn …>` for one mechanical reason: [`WaitDeps`] derives
/// [`Debug`], and a trait object cannot. Wrapping it here keeps the hand-written `Debug` to three
/// lines instead of restating all fourteen `WaitDeps` fields.
#[derive(Clone)]
pub struct WaitSubscribeHook(std::sync::Arc<dyn WaitSubscriptionArming>);

impl WaitSubscribeHook {
    /// Wrap a live arming seam — in production
    /// [`super::wait_subscriptions::WaitSubscriptionManager`].
    #[must_use]
    pub fn new(arming: std::sync::Arc<dyn WaitSubscriptionArming>) -> Self {
        Self(arming)
    }

    /// pi `deps.subscribe(input)` (`:597`).
    ///
    /// # Errors
    ///
    /// Whatever the manager refuses with — in practice
    /// [`super::wait_subscriptions::NO_SESSION_IDENTITY`] or a failed record write. Upstream
    /// catches the throw and reports it as an error result (`:598-600`); so does the caller here.
    pub async fn arm(
        &self,
        input: super::wait_subscriptions::ArmWaitSubscriptionInput,
    ) -> Result<super::wait_subscriptions::WaitSubscriptionRecord, crate::error::SubagentError>
    {
        self.0.arm(input).await
    }
}

impl std::fmt::Debug for WaitSubscribeHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WaitSubscribeHook")
    }
}

/// The one method the `bg_wait` tool needs off the subscription manager — pi's
/// `Pick<WaitSubscriptionManager, "arm">` (`wait-tool.ts:12`).
///
/// A trait, not the concrete manager, because `background/wait.rs` must not depend on the
/// executor-owned lifecycle that builds one; the manager's own deps are injected from
/// `extension/executor/` for the same layering reason.
#[async_trait::async_trait]
pub trait WaitSubscriptionArming: Send + Sync {
    /// pi `arm` (`wait-subscriptions.ts:290-308`).
    ///
    /// # Errors
    ///
    /// See [`WaitSubscribeHook::arm`].
    async fn arm(
        &self,
        input: super::wait_subscriptions::ArmWaitSubscriptionInput,
    ) -> Result<super::wait_subscriptions::WaitSubscriptionRecord, crate::error::SubagentError>;
}

impl WaitDeps {
    /// Production defaults for `cwd`: the shared per-cwd roots and pi's 1s poll interval. The
    /// caller supplies the live session id (pi `deps.state.currentSessionId`); pass `None` only
    /// when there genuinely is no session identity.
    #[must_use]
    pub fn for_cwd(
        cwd: &std::path::Path,
        enabled: bool,
        session_id: Option<String>,
        roots: &crate::paths::Roots,
    ) -> Self {
        let roots = super::run_artifact_roots_in(roots, cwd);
        Self {
            async_root: roots.async_root,
            results_dir: roots.results_dir,
            poll_interval: Duration::from_millis(DEFAULT_POLL_INTERVAL_MS),
            enabled,
            session_id,
            completion_bus: None,
            wait_completions: std::sync::Arc::new(
                crate::background::wait_completions::WaitCompletionStore::default(),
            ),
            // pi's defaults: `deps.stopOnAttention !== false` (`subagent-wait.ts:618`) and
            // `deps.failOnFailedRuns === true` / `deps.failOnAttention === true` (`:708`,`:725`)
            // — i.e. attention breaks the wait and neither outcome flips the result to an error
            // unless a caller (auto-drain) opts in. Byte-identical behaviour for the `bg_wait` tool.
            stop_on_attention: true,
            fail_on_failed_runs: false,
            fail_on_attention: false,
            inline_answers: None,
            // pi's own no-manager shape: `wait-tool.ts:33` omits `subscribe` entirely unless the
            // extension holds a manager AND `ctx.hasUI`.
            subscribe: None,
            // No executor in hand here, so no remembered-run map to consult — see
            // [`Self::with_detached_foreground`].
            detached_foreground: None,
        }
    }

    /// VL-S11b — attach the executor's remembered-foreground-run projection, so this wait's
    /// candidate set is upstream's FULL one (async runs plus detached foreground runs,
    /// `subagent-wait.ts:671`) rather than its async half.
    ///
    /// Separate from [`Self::for_cwd`] for exactly the reason [`Self::with_completion_bus`] and
    /// [`Self::with_subscribe`] are: the map is executor-owned and outlives any single wait, while
    /// `for_cwd` is reachable from contexts that have no executor at all (the auto-drain's two
    /// construction sites are two of them, and upstream's drain likewise waits on async runs
    /// only).
    #[must_use]
    pub fn with_detached_foreground(mut self, source: Option<DetachedForegroundHook>) -> Self {
        self.detached_foreground = source;
        self
    }

    /// SUBA-034 — attach the orchestrator's live completion bus, so this wait wakes on the
    /// observation of a terminal result instead of re-deriving it one poll interval later.
    ///
    /// Separate from [`Self::for_cwd`] because the bus is owned by the extension (it is the same
    /// handle [`crate::background::watch::install_completion_watcher_with_observer`] publishes
    /// into), while `for_cwd` is reachable from contexts that have no watcher at all.
    #[must_use]
    pub fn with_completion_bus(
        mut self,
        bus: Option<crate::background::watch::CompletionBus>,
    ) -> Self {
        self.completion_bus = bus;
        self
    }

    /// Attach the executor-owned consumed-payload record, so a wait that lands after the watcher
    /// has delivered and deleted a payload still reports that completion.
    ///
    /// Separate from [`Self::for_cwd`] for the same reason [`Self::with_completion_bus`] and
    /// [`Self::with_inline_answers`] are: the store is executor-owned (it must outlive any single
    /// watcher install — the watcher is REPLACED on every `SessionStart`), while `for_cwd` is
    /// reachable from contexts that have no executor at all.
    #[must_use]
    pub fn with_wait_completions(
        mut self,
        store: std::sync::Arc<crate::background::wait_completions::WaitCompletionStore>,
    ) -> Self {
        self.wait_completions = store;
        self
    }

    /// `ASYNC_NOTIFY_BUG_REPORT` F3.4 — attach the executor-owned
    /// [`crate::background::watch::InlineAnswerLedger`], so this wait's claims can hold a
    /// same-run completion delivery off and its inline answers can suppress the duplicate
    /// standalone notification. Separate from [`Self::for_cwd`] for the same reason
    /// [`Self::with_completion_bus`] is: the ledger is executor-owned (it must outlive any single
    /// watcher install), while `for_cwd` is reachable from contexts that have no executor at all.
    #[must_use]
    pub fn with_inline_answers(
        mut self,
        ledger: Option<crate::background::watch::InlineAnswerLedger>,
    ) -> Self {
        self.inline_answers = ledger;
        self
    }

    /// SCOPE_11 — attach the executor-owned subscription manager, so `{ id, nonBlocking: true }`
    /// arms a durable wake instead of being refused. Separate from [`Self::for_cwd`] for exactly
    /// the reason [`Self::with_completion_bus`] is: the manager is executor-owned and must outlive
    /// any single wait, while `for_cwd` is reachable from contexts that have no executor at all.
    #[must_use]
    pub fn with_subscribe(mut self, subscribe: Option<WaitSubscribeHook>) -> Self {
        self.subscribe = subscribe;
        self
    }

    /// Override [`Self::stop_on_attention`] — auto-drain passes `false` so it can wait THROUGH a
    /// needs-attention run instead of spinning on it.
    #[must_use]
    pub fn with_stop_on_attention(mut self, stop_on_attention: bool) -> Self {
        self.stop_on_attention = stop_on_attention;
        self
    }

    /// Override [`Self::fail_on_failed_runs`] — auto-drain passes `true` so a failed child makes
    /// the drain throw rather than exit quietly.
    #[must_use]
    pub fn with_fail_on_failed_runs(mut self, fail_on_failed_runs: bool) -> Self {
        self.fail_on_failed_runs = fail_on_failed_runs;
        self
    }

    /// Override [`Self::fail_on_attention`] — auto-drain passes `true` so unresolved attention at
    /// the end of the wait is surfaced as an error.
    #[must_use]
    pub fn with_fail_on_attention(mut self, fail_on_attention: bool) -> Self {
        self.fail_on_attention = fail_on_attention;
        self
    }
}

/// pi `formatDuration` (`shared/formatters.ts:49-53`).
#[must_use]
pub fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        return format!("{ms}ms");
    }
    if ms < 60_000 {
        #[allow(clippy::cast_precision_loss)] // sub-minute values; the `.1` render needs a float
        let seconds = ms as f64 / 1000.0;
        return format!("{seconds:.1}s");
    }
    format!("{}m{}s", ms / 60_000, (ms % 60_000) / 1000)
}

// =================================================================================================
// VL-S11b — remembered DETACHED FOREGROUND runs, the other half of upstream's candidate set
// =================================================================================================

/// One remembered foreground run, as `bg_wait` sees it — pi `ForegroundResumeRun`
/// (`shared/types.ts:1470-1479`) narrowed to what `activeDetachedForegroundRuns` and its three
/// renderers (`subagent-wait.ts:224-249`, `:541-562`) actually read.
///
/// A projection rather than the record itself for this module's standing reason: `background/`
/// sits BELOW `extension/executor/`, where `ForegroundHistoryRun` lives, and every cross-layer
/// dependency here is injected as a value (the same shape [`WaitDeps::completion_bus`] and
/// [`WaitDeps::subscribe`] already use).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetachedForegroundRun {
    /// pi `run.runId`.
    pub run_id: String,
    /// pi `run.sessionId` — REQUIRED here, because `:225`'s filter drops a run without one.
    pub session_id: String,
    /// pi `run.children`.
    pub children: Vec<DetachedForegroundChild>,
}

/// One child of a remembered foreground run — pi `ForegroundResumeChild`, narrowed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetachedForegroundChild {
    /// pi `child.index` — the flat child index the initially-detached set is keyed on.
    pub index: usize,
    /// pi `child.agent`.
    pub agent: String,
    /// pi `child.status`, compared against the literal `"detached"` exactly as upstream does.
    pub status: String,
    /// pi `child.activityState` — the first half of `:242`'s attention test.
    pub activity_state: Option<ActivityState>,
    /// pi `child.currentTool` — the second half of `:242`'s attention test
    /// (`=== "contact_supervisor"`).
    pub current_tool: Option<String>,
    /// pi `child.transcriptPath`, read by the progress renderer (`:546`).
    pub transcript_path: Option<PathBuf>,
}

/// pi `deps.state.foregroundRuns` (`subagent-wait.ts:224`) — the injected seam that hands this
/// module a snapshot of the executor's in-memory remembered-run map.
///
/// A trait rather than the map itself because the map is `extension/executor`'s private field and
/// its value type is `pub(crate)` inside that facade. The production implementation is the
/// executor's own projection; `None` on [`WaitDeps::detached_foreground`] is the no-executor
/// degradation every other injected seam here already has.
pub trait DetachedForegroundRunsSource: Send + Sync {
    /// Every remembered foreground run this process currently holds, session id included. The
    /// filtering (session, `"detached"` children, id/`all`) is
    /// [`active_detached_foreground_runs`]'s, so a source cannot get it subtly wrong.
    fn snapshot(&self) -> Vec<DetachedForegroundRun>;
}

/// [`WaitDeps::detached_foreground`]'s payload.
///
/// A newtype for the same mechanical reason [`WaitSubscribeHook`] is one: [`WaitDeps`] derives
/// [`Debug`] and a trait object cannot.
#[derive(Clone)]
pub struct DetachedForegroundHook(std::sync::Arc<dyn DetachedForegroundRunsSource>);

impl DetachedForegroundHook {
    /// Wrap a live source — in production the executor's own `foreground_runs` projection.
    #[must_use]
    pub fn new(source: std::sync::Arc<dyn DetachedForegroundRunsSource>) -> Self {
        Self(source)
    }

    /// pi `[...deps.state.foregroundRuns.values()]` (`:224`).
    #[must_use]
    pub fn snapshot(&self) -> Vec<DetachedForegroundRun> {
        self.0.snapshot()
    }
}

impl std::fmt::Debug for DetachedForegroundHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DetachedForegroundHook(..)")
    }
}

/// pi `activeDetachedForegroundRuns` (`subagent-wait.ts:222-230`) — the candidate set `bg_wait`
/// gains alongside its async runs, and the reason `WaitTargetKind::Foreground` is reachable from
/// production at all.
///
/// Upstream's four filters, in order: a live session is REQUIRED (`:223`); a run must belong to it
/// AND have at least one `"detached"` child (`:225-226`); an `id` narrows by exact-or-prefix
/// (`:228`); and with no `id`, only `all: true` selects anything (`:229`) — a bare `bg_wait()` does
/// NOT block on a detached foreground run, because the fleet-wide default is "return as soon as
/// something finishes" and a detached child is the one thing that will not.
pub fn active_detached_foreground_runs(
    params: &WaitParams,
    deps: &WaitDeps,
) -> Vec<DetachedForegroundRun> {
    let (Some(session_id), Some(hook)) = (
        deps.session_id.as_deref(),
        deps.detached_foreground.as_ref(),
    ) else {
        return Vec::new();
    };
    let runs = hook.snapshot().into_iter().filter(|run| {
        run.session_id == session_id && run.children.iter().any(|child| child.status == "detached")
    });
    match params.id.as_deref() {
        Some(id) => runs
            .filter(|run| run.run_id == id || run.run_id.starts_with(id))
            .collect(),
        None if params.all == Some(true) => runs.collect(),
        None => Vec::new(),
    }
}

/// pi `foregroundChildrenNeedingAttention` (`subagent-wait.ts:241-243`): a still-`detached` child
/// parked on a blocking `contact_supervisor` ask.
fn foreground_children_needing_attention<'a>(
    run: &'a DetachedForegroundRun,
    indices: &std::collections::BTreeSet<usize>,
) -> Vec<&'a DetachedForegroundChild> {
    run.children
        .iter()
        .filter(|child| {
            indices.contains(&child.index)
                && child.status == "detached"
                && child.activity_state == Some(ActivityState::NeedsAttention)
                && child.current_tool.as_deref() == Some("contact_supervisor")
        })
        .collect()
}

/// pi `summarizeForegroundChildren` (`subagent-wait.ts:232-239`): `"2 completed, 1 failed"` over
/// the initially-detached set, EXCLUDING any child still detached.
fn summarize_foreground_children(
    run: &DetachedForegroundRun,
    indices: &std::collections::BTreeSet<usize>,
) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for child in &run.children {
        if !indices.contains(&child.index) || child.status == "detached" {
            continue;
        }
        *counts.entry(child.status.as_str()).or_insert(0) += 1;
    }
    counts
        .into_iter()
        .map(|(status, count)| format!("{count} {status}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// pi `formatForegroundAttention` (`subagent-wait.ts:245-249`).
fn format_foreground_attention(
    run: &DetachedForegroundRun,
    children: &[&DetachedForegroundChild],
    elapsed_ms: u64,
) -> String {
    let child_list = children
        .iter()
        .map(|child| format!("{}#{}", child.agent, child.index))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Waited {} for remembered detached foreground run \"{}\"; attention required. {} child \
         run(s) need attention: {child_list}. Reply to any pending supervisor request, then call \
         {}({{ id: \"{}\" }}) again or inspect status; do not resume or launch a replacement \
         while it remains detached.",
        format_duration(elapsed_ms),
        run.run_id,
        children.len(),
        crate::extension::wait_tool::WAIT_TOOL_NAME,
        run.run_id
    )
}

/// pi `detachedForegroundWaitUpdate` (`subagent-wait.ts:541-556`) — the per-tick progress render.
///
/// `[CYRUP-DELTA]` two narrowings, both forced and both stated rather than faked. Upstream streams
/// this through `deps.onUpdate` (`:587`); cyrup's [`WaitDeps`] carries no update seam on this
/// path, so the render goes to the diagnostic log each tick — the same channel every other
/// unstreamed observation in this crate uses. And upstream enriches each line from
/// `readTranscriptActivity(child.transcriptPath)` (`:545`), a live NDJSON tail; the projection
/// this module is handed carries the child's `current_tool` but not its recent output, so the
/// per-child line names the tool in flight and says when no transcript is available, which is
/// upstream's own `"live transcript unavailable"` arm.
#[must_use]
pub fn detached_foreground_wait_update(
    run: &DetachedForegroundRun,
    pending_indices: &std::collections::BTreeSet<usize>,
    elapsed_ms: u64,
) -> String {
    let mut lines = vec![format!(
        "Waiting for detached foreground run \"{}\" · {}",
        run.run_id,
        format_duration(elapsed_ms)
    )];
    for child in &run.children {
        if !pending_indices.contains(&child.index) || child.status != "detached" {
            continue;
        }
        lines.push(format!(
            "{} · working after supervisor handoff",
            child.agent
        ));
        if let Some(tool) = child.current_tool.as_deref() {
            lines.push(format!("  current: {tool}"));
        }
        if child.transcript_path.is_none() {
            lines.push("  live transcript unavailable; waiting for completion event".to_string());
        }
    }
    lines.join("\n")
}

/// A run flagged as needing the orchestrator's attention (pi `needsAttention`, `runs/background/wait.ts:203-205` @v0.34.0).
fn needs_attention(run: &ActiveRun) -> bool {
    run.status.telemetry.activity_state == Some(ActivityState::NeedsAttention)
}

fn run_id_of(run: &ActiveRun) -> &str {
    run.status.run_id.as_str()
}

/// pi `matchesId` (`runs/background/wait.ts:198-200` @v0.34.0): exact id, or id prefix.
fn matches_id(run: &ActiveRun, id: &str) -> bool {
    run_id_of(run) == id || run_id_of(run).starts_with(id)
}

/// Queued/running runs for this cwd, optionally narrowed to `id` (pi `activeRunsForSession`).
async fn active_runs(id: Option<&str>, deps: &WaitDeps) -> Result<Vec<ActiveRun>, String> {
    let runs = list_active_runs(
        &deps.async_root,
        &deps.results_dir,
        deps.session_id.as_deref(),
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(match id {
        Some(id) => runs.into_iter().filter(|run| matches_id(run, id)).collect(),
        None => runs,
    })
}

/// Every initially-tracked run that has reached a TERMINAL state, reconciled from disk — pi's
/// `terminal` binding (`subagent-wait.ts:613`: `allRunsForSession(…).filter(run =>
/// !ACTIVE_STATES.includes(run.state) && initialAsyncIds.has(run.id))`).
///
/// Built by re-reading each initially-tracked run's own reconciled status rather than re-listing,
/// because [`list_active_runs`] deliberately surfaces only the active ones.
///
/// Returned as the statuses themselves rather than as a rendered summary because upstream derives
/// TWO independent things from this one list — `summarizeTerminalRuns(terminal, …)` at `:616` and
/// `formatResumeFirstFailedRunsNote(terminal)` at `:617` (SUBA-060) — and reading the run tree
/// twice for them would be both slower and racy against a run that changes state in between.
async fn terminal_runs_for(initial_ids: &[String], deps: &WaitDeps) -> Vec<super::RunStatus> {
    let mut terminal = Vec::new();
    for id in initial_ids {
        let paths = super::RunPaths::for_run(
            &deps.async_root,
            &deps.results_dir,
            &super::RunId::from_token(id.clone()),
        );
        // Best effort: a run whose record vanished simply contributes nothing to the summary. The
        // wait itself has already resolved; this is reporting only (pi wraps the whole block in a
        // `try`/`catch` with the same "summary is best-effort" comment).
        let Ok(status) = super::control::reconcile_before_control_op(&paths).await else {
            continue;
        };
        if is_active(status.state) {
            continue;
        }
        terminal.push(status);
    }
    terminal
}

/// pi `summarizeTerminalRuns` (`subagent-wait.ts:616`) — the `Outcome: …` bucket counts.
fn summarize_terminal_runs(terminal: &[super::RunStatus]) -> (usize, String) {
    let mut complete = 0usize;
    let mut failed = 0usize;
    let mut paused = 0usize;
    // G77: stopped runs are counted and reported in their own bucket, never folded into `failed`.
    let mut stopped = 0usize;
    for status in terminal {
        match status.state {
            RunState::Complete => complete += 1,
            // SUBA-100 — pi counts a partial run with the failed ones
            // (`run.state === "failed" || run.state === "partial"`, `subagent-wait.ts:817`).
            RunState::Failed | RunState::Partial => failed += 1,
            RunState::Paused => paused += 1,
            RunState::Stopped => stopped += 1,
            RunState::Queued | RunState::Running => {}
        }
    }
    let finished = complete + failed + paused + stopped;
    let mut parts: Vec<String> = Vec::new();
    if complete > 0 {
        parts.push(format!("{complete} complete"));
    }
    if failed > 0 {
        parts.push(format!("{failed} failed"));
    }
    // G77: placed between `failed` and `paused`, matching the fixed bucket order pi renders status
    // counts in (`result-intercom.ts:57-66 formatStatusCounts`: completed, failed, stopped, paused,
    // detached).
    if stopped > 0 {
        parts.push(format!("{stopped} stopped"));
    }
    if paused > 0 {
        parts.push(format!("{paused} paused"));
    }
    (finished, parts.join(", "))
}

fn join_ids(runs: &[ActiveRun]) -> String {
    runs.iter().map(run_id_of).collect::<Vec<_>>().join(", ")
}

fn join_ids_with_state(runs: &[ActiveRun]) -> String {
    runs.iter()
        .map(|run| {
            format!(
                "{} ({})",
                run_id_of(run),
                super::run_status::run_state_label(run.status.state)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// How a wait ended — pi's `isError` bit and its `details.wait` block, as the set of mutually
/// exclusive outcomes they actually encode (SCOPE_3 §A.1 row 2).
///
/// # Why an enum and not `is_error: bool`
///
/// The two non-resolutions that a `bool` cannot tell apart are exactly the two cyrup already got
/// wrong: an ABORT is an error upstream (`subagent-wait.ts:677`) and a TIMEOUT is **not**
/// (`windowElapsedResult`, `:360-378`, raised at `:679-685`) — it returns a successful result
/// carrying `details.wait = { reason: "window_elapsed", … }`. Collapsing them to one flag is how
/// the current `Err(...)` timeout survived review, and it is what makes auto-drain report
/// `Auto-drain failed for session '…': Wait timed out after …` where upstream reports
/// `Auto-drain timed out after {N}ms with background work still active in session '…'`
/// (`auto-drain.ts:58-59`, reachable only because `:73`'s `isError` check is false for a window
/// that merely elapsed).
///
/// Every variant names one upstream `return` site. Adding one forces every reader's `match` to
/// decide, which a `bool` never does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WaitVerdict {
    /// The tool is switched off — pi `:577`. Not an error: upstream calls bare `result(text)`.
    Disabled,
    /// Nothing was active and nothing terminal was replayable — pi `:644-646`. Not an error.
    NothingToWait,
    /// A run listing faulted, at entry (pi `:600-609`) or mid-loop (pi `:686-699`). Error.
    ListingFailed,
    /// An id prefix matched several active runs — pi `:619`. Error.
    AmbiguousId,
    /// The turn was aborted — pi `:677`. Error.
    Aborted,
    /// The wait window elapsed — pi `windowElapsedResult` (`:360-378`), raised at `:679-685`.
    /// **Not an error**, and the only verdict carrying structure: `activeRunIds` is what upstream
    /// puts on `details.wait` so a caller can re-target the runs it was waiting on without
    /// re-listing.
    WindowElapsed {
        /// pi `activeInitialRuns.map((run) => run.id)` (`:683`) — the initial∩active set at the
        /// moment the window closed, ids only (their states are already in the text).
        active_run_ids: Vec<String>,
    },
    /// Projecting the terminal payloads faulted — pi `:709-719`'s catch. Error. This is where
    /// [`super::wait_completions::collect_wait_completions`]' `Err` lands, and the ONLY place it
    /// can: a projection rejection is a reported wait, never a propagated one.
    CompletionsFailed,
    /// SCOPE_11 — `{ id, nonBlocking: true }` armed a durable subscription and returned at once
    /// (pi `:596`). **Not an error**: upstream's return there is a bare `result(text)`.
    ///
    /// Carries the token because it is the handle a caller uses to recognize the wake when it
    /// arrives, and because `subagent({ action: "status" })` renders the same value — the text
    /// already names it, and a consumer must not have to parse prose to get it back.
    SubscriptionArmed {
        /// The armed record's identity — [`super::wait_subscriptions::WaitSubscriptionRecord::token`].
        token: super::wait_subscriptions::SubscriptionToken,
    },
    /// VL-S11b — a wait on a remembered DETACHED FOREGROUND run could not be reconciled: the
    /// active session changed under it (pi `subagent-wait.ts:568-570`) or the run disappeared from
    /// the remembered map before any terminal child result was recorded (`:573-575`). Error, as
    /// both of upstream's are.
    ///
    /// Its own variant rather than [`Self::ListingFailed`] because nothing faulted — the listing
    /// succeeded and told the wait that what it was waiting for is no longer addressable from
    /// here, which is a different thing for a caller to decide about.
    ForegroundUnreconciled,
    /// SCOPE_11 — the non-blocking form was REFUSED. Error.
    ///
    /// One variant for upstream's four refusal sites (`:561` no `id`, `:564` combined with `all`,
    /// `:592` no manager or no UI, `:598` the `arm` itself threw) rather than four, because a
    /// reader's `match` has nothing different to do for them: every one of them means "this call
    /// did not register a wake, and did not block either". Which refusal it was lives in
    /// [`WaitOutcome::text`], where upstream also keeps it.
    SubscriptionRefused,
    /// The wait resolved. Error **iff** the deps flags say so — pi `:751`/`:768`.
    Resolved {
        /// pi `failedAsyncCount` (`:713`). Kept beside the flag because the two terminal renders
        /// and the verdict are built from one evaluation, not three.
        failed_runs: usize,
        /// pi `relevantAttention.length` (`:721`, `:725`).
        attention_runs: usize,
        /// `(deps.fail_on_failed_runs && failed_runs > 0) || (deps.fail_on_attention &&
        /// attention_runs > 0)` — pi `:751`/`:768` verbatim, evaluated ONCE at construction.
        /// SCOPE_2's predicate is unchanged in wording; only what it selects between changes.
        reported_as_error: bool,
    },
}

/// The full outcome of one wait — pi's `result(text, isError, completions)` (`:337-352`), which is
/// a single `AgentToolResult`, not a `Result`.
///
/// Replacing `Result<String, String>` is not a refactor for its own sake: upstream carries THREE
/// things out of this function and the old signature could carry one. The error bit belongs on the
/// value precisely because a wait can resolve successfully AND report failed runs
/// ([`WaitDeps::fail_on_failed_runs`], pi `:751`) — which `Err` cannot express without discarding
/// the completions that justify it.
#[derive(Clone, Debug)]
pub struct WaitOutcome {
    /// The rendered summary — every existing `Ok`/`Err` string lands here unchanged.
    pub text: String,
    /// How it ended.
    pub verdict: WaitVerdict,
    /// pi `details.completions` (`shared/types.ts:1408`). Empty is upstream's `undefined` — the key
    /// is omitted, never emitted as `[]` (pi `:349`).
    pub completions: Vec<WaitCompletion>,
}

impl WaitOutcome {
    /// THE constructor — pi `result(text, isError, completions)` (`:337-352`).
    ///
    /// The workflow-receipt append (`:339-341`) happens HERE rather than at each call site, for
    /// the reason upstream does it inside `result()`: both terminal returns must get it and
    /// neither may be able to forget. WORKFLOW_3 is what makes `workflow_receipt_path`
    /// non-`None`; before it, this loop is a no-op.
    ///
    /// It appends AFTER `text` is already complete — including after `ASYNC_NOTIFY_BUG_REPORT`
    /// F2's `resolved.appendix` — which is upstream's order and keeps the receipt lines last.
    #[must_use]
    pub fn new(text: String, verdict: WaitVerdict, completions: Vec<WaitCompletion>) -> Self {
        let mut text = text;
        for completion in &completions {
            if let Some(path) = completion.workflow_receipt_path.as_deref() {
                text.push_str(&format!(
                    "\nWorkflow receipt [{}]: {path}",
                    completion.run_id
                ));
            }
        }
        Self {
            text,
            verdict,
            completions,
        }
    }

    /// A non-resolution: a verdict with no completions to carry. Every `result(text, true)` site
    /// upstream (`:577`, `:608`, `:619`, `:677`, `:698`, `:718`) passes exactly two arguments —
    /// §0.2 — so this is not a shortcut, it is the faithful shape.
    #[must_use]
    pub fn plain(text: String, verdict: WaitVerdict) -> Self {
        Self::new(text, verdict, Vec::new())
    }

    /// pi's `...(isError ? { isError: true } : {})` (`:344`), derived rather than stored.
    ///
    /// The `match` is exhaustive on purpose: a new [`WaitVerdict`] variant must not silently
    /// inherit "not an error". No `_ =>` arm — SCOPE_3 §A.1 row 2 and §3's grep #4.
    #[must_use]
    pub fn is_error(&self) -> bool {
        match &self.verdict {
            WaitVerdict::Disabled
            | WaitVerdict::NothingToWait
            | WaitVerdict::SubscriptionArmed { .. }
            | WaitVerdict::WindowElapsed { .. } => false,
            WaitVerdict::ListingFailed
            | WaitVerdict::AmbiguousId
            | WaitVerdict::Aborted
            | WaitVerdict::SubscriptionRefused
            | WaitVerdict::ForegroundUnreconciled
            | WaitVerdict::CompletionsFailed => true,
            WaitVerdict::Resolved {
                reported_as_error, ..
            } => *reported_as_error,
        }
    }

    /// pi's `details` object — `result()`'s (`:346-350`) and `windowElapsedResult()`'s
    /// (`:367-376`) as the one value they are on the wire.
    ///
    /// Built HERE, not in `extension/wait_tool.rs`, because upstream builds it inside the two
    /// result constructors: a second consumer must not have to re-derive the shape, and
    /// `results: []` (`:348`/`:369`) is exactly the kind of key an open-coded caller forgets —
    /// cyrup's current object (`wait_tool.rs:165`) omits it, so a pi consumer reading
    /// `details.results` gets `undefined` where upstream gives `[]`.
    #[must_use]
    pub fn details(&self) -> serde_json::Value {
        let mut details = serde_json::json!({ "mode": "management", "results": [] });
        // §0.5e: this `let … else` compiles on the pinned stable toolchain (verified 2026-09-12
        // against rustc 1.97.1, edition 2024). Do not rewrite it into an unwrap/expect — the
        // workspace denies both (`Cargo.toml:101-102`).
        let Some(map) = details.as_object_mut() else {
            return details; // unreachable: the literal above is an object
        };
        if !self.completions.is_empty()
            && let Ok(value) = serde_json::to_value(&self.completions)
        {
            // pi omits the key entirely when the list is empty (`:349`).
            map.insert("completions".into(), value);
        }
        if let WaitVerdict::WindowElapsed { active_run_ids } = &self.verdict {
            map.insert(
                "wait".into(),
                serde_json::json!({
                    "reason": "window_elapsed",
                    "timedOut": true,
                    "activeRunIds": active_run_ids,
                    // Always present, always empty: pi emits the key unconditionally (`:374`) and
                    // cyrup has no background-work PROVIDER registry at all — the same unported
                    // second disjunct `auto_drain.rs:83-90` documents. Emitting `[]` keeps a pi
                    // consumer's destructuring valid; omitting it would hand it `undefined`.
                    "activeProviderItems": [],
                }),
            );
        }
        details
    }

    /// pi `completionUsage(completions)` → `AgentToolResult.usage` (`:318`, `:338`, `:345`),
    /// already composed with `toAgentToolUsage` inside
    /// [`super::wait_completions::completion_usage`]. `None` for a wait over children that
    /// reported no usage, which omits the key.
    #[must_use]
    pub fn usage(&self) -> Option<cyrup_core::Usage> {
        super::wait_completions::completion_usage(&self.completions)
    }
}

/// SCOPE_11 / pi `:704-714` — the one arming site, for BOTH target kinds.
///
/// Extracted rather than duplicated because upstream has one site too: `selected.kind` is
/// interpolated into the success sentence (`:710` — `for exact ${selected.kind} run ${selected.id}`),
/// so the async and foreground texts differ in exactly that one word and nothing else.
async fn arm_wait_subscription(
    kind: super::wait_subscriptions::WaitTargetKind,
    exact: &str,
    requested: &str,
    timeout: Duration,
    deps: &WaitDeps,
) -> WaitOutcome {
    let Some(subscribe) = deps.subscribe.as_ref() else {
        // pi `:705-707` — the `ctx?.hasUI`/no-manager refusal (`wait-tool.ts:33`), with pi's
        // `bg_wait` spelled through cyrup's own registered tool name.
        return WaitOutcome::plain(
            format!(
                "Non-blocking wait subscriptions require a long-lived interactive subagent \
                 runtime; this runtime can only use blocking {} calls.",
                crate::extension::wait_tool::WAIT_TOOL_NAME
            ),
            WaitVerdict::SubscriptionRefused,
        );
    };
    let input = super::wait_subscriptions::ArmWaitSubscriptionInput {
        target_kind: kind,
        run_id: super::RunId::from_token(exact.to_string()),
        requested_id: requested.to_string(),
        timeout_ms: timeout.as_millis().try_into().unwrap_or(u64::MAX),
    };
    // pi `:711-713` — the arm's own throw is reported as an error RESULT, never propagated.
    match subscribe.arm(input).await {
        Ok(record) => WaitOutcome::plain(
            format!(
                "Armed wait subscription {} for exact {kind} run {exact}. Returning immediately; \
                 this session will be woken on completion, failure, attention, reconciliation \
                 failure, or timeout. Inspect armed subscriptions with \
                 subagent({{ action: \"status\" }}).",
                record.token
            ),
            WaitVerdict::SubscriptionArmed {
                token: record.token,
            },
        ),
        Err(error) => WaitOutcome::plain(error.to_string(), WaitVerdict::SubscriptionRefused),
    }
}

/// pi `waitForSessionDetachedForegroundRuns` (`subagent-wait.ts:602-630`): wait each selected
/// remembered foreground run to a terminal child result, in order, returning on the FIRST one that
/// ends the wait.
///
/// Sequential and not concurrent because upstream is: an attention result or an abort from any one
/// run is the answer for all of them, and a caller told "reply to the supervisor request" must not
/// also be handed three other runs' progress.
async fn wait_for_detached_foreground_runs(
    runs: &[DetachedForegroundRun],
    cancel: &CancelToken,
    deps: &WaitDeps,
    started_at: Instant,
    poll_interval: Duration,
    timeout: Duration,
) -> WaitOutcome {
    let mut last_done: Option<WaitOutcome> = None;
    for run in runs {
        let outcome =
            wait_for_detached_foreground_run(run, cancel, deps, started_at, poll_interval, timeout)
                .await;
        // pi `:615-624` — an error, an attention yield or an elapsed window ends the whole wait;
        // only a plain `done` (a `Resolved` with no attention) lets the loop move to the next run.
        if !matches!(
            outcome.verdict,
            WaitVerdict::Resolved {
                attention_runs: 0,
                ..
            }
        ) {
            return outcome;
        }
        last_done = Some(outcome);
    }
    // Every selected run reached a terminal child result; upstream's loop likewise ends on the
    // LAST one's text (`:627`). The fallback is unreachable from `wait_for_subagents`, whose
    // caller guard is `!detached_foreground.is_empty()`, and is a value rather than a panic
    // because an empty slice is a legitimate thing to hand a helper.
    last_done.unwrap_or_else(|| {
        WaitOutcome::plain(
            "No remembered detached foreground run remained to wait for.".to_string(),
            WaitVerdict::NothingToWait,
        )
    })
}

/// pi `waitForDetachedForegroundRun` (`subagent-wait.ts:558-599`) — one run's poll loop, with
/// upstream's guard order preserved exactly: session changed ▸ run gone ▸ attention ▸ done ▸
/// progress ▸ abort ▸ window elapsed ▸ sleep.
///
/// `[CYRUP-DELTA]` upstream's `deps.hasPendingSupervisorRequest?.()` yield (`:577`) has no cyrup
/// seam on this path — [`WaitDeps`] carries no supervisor-request probe, and the ask lock that
/// would answer it lives on the executor above this layer. The ATTENTION guard immediately below
/// it covers the case that matters here (a detached child parked on `contact_supervisor`), which
/// is the only way a supervisor request can exist for a run this function is waiting on.
async fn wait_for_detached_foreground_run(
    run: &DetachedForegroundRun,
    cancel: &CancelToken,
    deps: &WaitDeps,
    started_at: Instant,
    poll_interval: Duration,
    timeout: Duration,
) -> WaitOutcome {
    // pi `:559` — the set is fixed at entry: a child that becomes detached later is not something
    // this wait promised to cover.
    let initial_detached: std::collections::BTreeSet<usize> = run
        .children
        .iter()
        .filter(|child| child.status == "detached")
        .map(|child| child.index)
        .collect();
    let run_id = run.run_id.as_str();
    loop {
        // pi `:561-563` — the session changed under the wait.
        if deps.session_id.as_deref() != Some(run.session_id.as_str()) {
            return WaitOutcome::plain(
                format!(
                    "Wait stopped because the active session changed while remembered foreground \
                     run \"{run_id}\" was still detached. Return to the originating session to \
                     inspect or wait for it. Reply to any pending supervisor request before \
                     resuming or launching a replacement."
                ),
                WaitVerdict::ForegroundUnreconciled,
            );
        }
        // pi `:564-566` — re-read the LIVE map every lap; the continuation task that owns the
        // child writes its terminal status there (`reconcile_detached_foreground_child`).
        let current = deps
            .detached_foreground
            .as_ref()
            .map(DetachedForegroundHook::snapshot)
            .unwrap_or_default()
            .into_iter()
            .find(|candidate| {
                candidate.run_id == run.run_id && candidate.session_id == run.session_id
            });
        let Some(current) = current else {
            return WaitOutcome::plain(
                format!(
                    "Remembered foreground run \"{run_id}\" disappeared before a terminal child \
                     result was recorded. Completion cannot be confirmed; do not launch a \
                     replacement without checking the originating child session."
                ),
                WaitVerdict::ForegroundUnreconciled,
            );
        };
        let elapsed = started_at.elapsed();
        let elapsed_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);

        // pi `:579-580` — attention wins over done, because a child parked on a supervisor ask has
        // not finished and the caller is exactly who has to unblock it.
        let attention = foreground_children_needing_attention(&current, &initial_detached);
        if !attention.is_empty() {
            return WaitOutcome::plain(
                format_foreground_attention(&current, &attention, elapsed_ms),
                WaitVerdict::Resolved {
                    failed_runs: 0,
                    attention_runs: attention.len(),
                    // pi `:768` — only the drain's `failOnAttention` turns this into an error.
                    reported_as_error: deps.fail_on_attention,
                },
            );
        }

        // pi `:581-586` — every initially-detached child has landed.
        let pending = current
            .children
            .iter()
            .any(|child| initial_detached.contains(&child.index) && child.status == "detached");
        if !pending {
            let outcome = summarize_foreground_children(&current, &initial_detached);
            let failed_runs = current
                .children
                .iter()
                .filter(|child| initial_detached.contains(&child.index) && child.status == "failed")
                .count();
            let outcome_text = if outcome.is_empty() {
                "no recovered child status".to_string()
            } else {
                outcome
            };
            return WaitOutcome::plain(
                format!(
                    "Waited {} for remembered detached foreground run \"{run_id}\"; done. \
                     Outcome: {outcome_text}. Completion event observed; inspect with \
                     subagent({{ action: \"status\", id: \"{run_id}\" }}) for recovered output.",
                    format_duration(elapsed_ms)
                ),
                WaitVerdict::Resolved {
                    failed_runs,
                    attention_runs: 0,
                    // pi `:751` — only the drain's `failOnFailedRuns` turns this into an error.
                    reported_as_error: deps.fail_on_failed_runs && failed_runs > 0,
                },
            );
        }

        // pi `:587-588` — the per-tick progress render; see
        // [`detached_foreground_wait_update`]'s own `[CYRUP-DELTA]` for where it goes here.
        tracing::debug!(
            run_id = %run_id,
            progress = %detached_foreground_wait_update(&current, &initial_detached, elapsed_ms),
            "waiting for a detached foreground run"
        );

        // pi `:589-591`.
        if cancel.is_cancelled() {
            return WaitOutcome::plain(
                format!(
                    "Wait aborted after {}. Remembered foreground run \"{run_id}\" remains \
                     detached. Reply to any pending supervisor request before resuming or \
                     launching a replacement.",
                    format_duration(elapsed_ms)
                ),
                WaitVerdict::Aborted,
            );
        }

        // pi `:592-597`.
        if elapsed >= timeout {
            let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
            return WaitOutcome::plain(
                format!(
                    "Wait window elapsed after {} with remembered foreground run \"{run_id}\" \
                     still detached. Reply to any pending supervisor request, then call {}({{ id: \
                     \"{run_id}\" }}) again or inspect status; do not resume or launch a \
                     replacement while it remains detached.",
                    format_duration(timeout_ms),
                    crate::extension::wait_tool::WAIT_TOOL_NAME
                ),
                WaitVerdict::WindowElapsed {
                    active_run_ids: vec![run.run_id.clone()],
                },
            );
        }

        // pi `await waitForWake(pollIntervalMs, signal, deps)` (`:598`). `biased` for the reason
        // the async loop's own sleep states: a cancelled token must win over a ready timer.
        let slice = poll_interval.min(timeout.saturating_sub(elapsed));
        tokio::select! {
            biased;
            () = cancel.cancelled() => {}
            () = tokio::time::sleep(slice) => {}
        }
    }
}

/// Block until the targeted background runs finish, the timeout elapses, or the turn is aborted —
/// pi `waitForSubagents` (`runs/background/wait.ts:264-394` @v0.34.0).
///
/// Returns a [`WaitOutcome`] naming exactly how the wait ended — pi's `result(text, isError,
/// completions)` (`subagent-wait.ts:337-352`) collapsed onto one value, the way every other
/// control action in this crate maps its own outcome onto the tool result's error flag. There is
/// no outer `Result`: every fallible step inside resolves to a [`WaitVerdict`] rather than a
/// propagated error — see [`WaitVerdict`]'s own doc for why an ABORT is an error and a TIMEOUT is
/// not.
pub async fn wait_for_subagents(
    params: &WaitParams,
    cancel: &CancelToken,
    deps: &WaitDeps,
) -> WaitOutcome {
    if !deps.enabled {
        return WaitOutcome::plain(
            format!(
                "Wait tool is disabled by config.waitTool or {WAIT_TOOL_ENABLED_ENV}; returning \
                 immediately without blocking background subagent runs. Active runs keep going, \
                 and you can inspect them with subagent({{ action: \"status\" }}) or wait for \
                 completion notifications."
            ),
            WaitVerdict::Disabled,
        );
    }

    // SCOPE_11 / pi `:561-566` — BEFORE any listing, because neither rejection depends on what is
    // on disk. A registration binds ONE exact run identity, so a prefix-less or fleet-wide
    // non-blocking wait has nothing to bind and must say so rather than silently blocking.
    if params.non_blocking == Some(true) {
        if params.id.is_none() {
            return WaitOutcome::plain(
                "Non-blocking wait subscriptions require id so the registration can bind one \
                 exact run identity."
                    .to_string(),
                WaitVerdict::SubscriptionRefused,
            );
        }
        if params.all == Some(true) {
            return WaitOutcome::plain(
                "nonBlocking cannot be combined with all; subscribe to one exact run id."
                    .to_string(),
                WaitVerdict::SubscriptionRefused,
            );
        }
    }

    let poll_interval = deps
        .poll_interval
        .max(Duration::from_millis(MIN_POLL_INTERVAL_MS));
    let timeout = Duration::from_millis(match params.timeout_ms {
        Some(ms) if ms > 0 => ms,
        _ => DEFAULT_TIMEOUT_MS,
    });
    let started_at = Instant::now();

    // A single named run always means "wait until that one is done", regardless of `all`.
    let wait_for_all = params.id.is_some() || params.all == Some(true);

    // SUBA-034: subscribe BEFORE the first listing, never after. A completion observed between the
    // snapshot below and a later subscription would be missed by BOTH — the snapshot still shows
    // the run active and the subscription starts after the edge — and the wait would then pay the
    // full poll interval anyway, which is exactly the latency this closes. `broadcast::Receiver`
    // only delivers values sent after `subscribe()`, so this ordering is the whole correctness
    // argument for the wake.
    let mut wake = deps
        .completion_bus
        .as_ref()
        .map(super::watch::CompletionBus::subscribe);
    // SCOPE_17 — the per-child blocks observed off the bus while this wait was in flight, keyed by
    // run id so a re-published completion cannot overwrite the first observation (`or_insert_with`
    // at the write site, never `insert`).
    let mut observed_summaries: BTreeMap<String, String> = BTreeMap::new();

    // pi `:600-609` — upstream reports a listing failure as a wait RESULT, not as a throw that
    // escapes `waitForSubagents`. `active_runs` already carries the human-readable message.
    let mut active = match active_runs(params.id.as_deref(), deps).await {
        Ok(active) => active,
        Err(text) => return WaitOutcome::plain(text, WaitVerdict::ListingFailed),
    };

    // VL-S11b — pi `:671`: the candidate set is async runs PLUS remembered detached FOREGROUND
    // runs (`activeDetachedForegroundRuns`). `bg_wait` had only the async half, which is why
    // `WaitTargetKind::Foreground` had no production producer until now.
    //
    // `[CYRUP-DELTA]` upstream MERGES the two lists into one candidate array and resolves the id
    // across both (`:681-690`), so an id matching one async run AND one foreground run is reported
    // as AMBIGUOUS. cyrup consults the foreground half only when the async half selected nothing,
    // so that collision resolves to the async run instead. It is not reachable in practice — a
    // `RunId` is minted once per run and the two maps are keyed by the same id space, so one id
    // cannot name both — but the resolution rule differs and is stated rather than implied.
    let detached_foreground = if active.is_empty() {
        active_detached_foreground_runs(params, deps)
    } else {
        // The async half selected something, so the foreground half is not consulted and its
        // snapshot is not taken — a wait that already has work to do pays nothing for this.
        Vec::new()
    };
    if !detached_foreground.is_empty() {
        // SCOPE_11 — the arming site's OTHER target kind, reachable from production for the first
        // time. pi `:704-714` arms whatever `selected.kind` resolved to; the record format, the
        // manager's restore path and its whole foreground reconcile branch
        // (`wait_subscriptions/manager.rs`) were already ported against a producer that did not
        // exist yet.
        if params.non_blocking == Some(true)
            && let Some(requested) = params.id.as_deref()
            && let Some(run) = detached_foreground.first()
        {
            return arm_wait_subscription(
                super::wait_subscriptions::WaitTargetKind::Foreground,
                &run.run_id,
                requested,
                timeout,
                deps,
            )
            .await;
        }
        return wait_for_detached_foreground_runs(
            &detached_foreground,
            cancel,
            deps,
            started_at,
            poll_interval,
            timeout,
        )
        .await;
    }

    if active.is_empty() {
        // Nothing is running. That is NOT the same as "there is nothing to report": the runs this
        // caller is asking about have very likely just finished, and their results are on disk
        // right now. Denying their existence is what sent an orchestrator to `bash` to `cat` an
        // artifact file it had already been told nothing about.
        // ASYNC_NOTIFY_BUG_REPORT F3.4 — the claim for this branch is taken INSIDE
        // `resolve_finished`, as soon as the run ids are known and before any payload is read.
        // Claiming out here, after the await, would leave that read window unprotected (RC4).
        return WaitOutcome::plain(
            match resolve_finished(params, deps).await {
                WaitResolution::Terminal(replay) => replay.render(),
                WaitResolution::Unknown => match &params.id {
                    Some(id) => format!("No active run matched \"{id}\". Nothing to wait for."),
                    None => {
                        "No active async runs in this session. Nothing to wait for.".to_string()
                    }
                },
            },
            WaitVerdict::NothingToWait,
        );
    }

    let mut effective_id = params.id.clone();
    if let Some(id) = params.id.as_deref() {
        let exact: Vec<ActiveRun> = active
            .iter()
            .filter(|run| run_id_of(run) == id)
            .cloned()
            .collect();
        if exact.len() == 1 {
            active = exact;
        } else if active.len() > 1 {
            return WaitOutcome::plain(
                format!(
                    "Ambiguous async run id prefix \"{id}\" matched {} active runs: {}. Pass a \
                     longer id.",
                    active.len(),
                    join_ids(&active)
                ),
                WaitVerdict::AmbiguousId,
            );
        }
        // Narrow to the single resolved id so later polls cannot pick up a different prefix match.
        effective_id = active.first().map(|run| run_id_of(run).to_string());
    }

    // SCOPE_11 / pi `:704-714` — THE arming site, and its position is the whole point: after the
    // ambiguity rejection at `:688-690` and after the id has been narrowed to ONE exact run, so the
    // record binds the resolved identity rather than the prefix the caller typed. Both are kept:
    // `run_id` is the exact id, `requested_id` is what the user asked for.
    //
    // Upstream reaches here only when a candidate matched (`selected` is defined); cyrup's
    // equivalent guard is the `active.is_empty()` return above, which already answered
    // `No active run matched "…"`.
    //
    // Upstream's `selected.kind` can be `async` or `foreground` (`:671`, `:682-684`), and so can
    // cyrup's:
    // the FOREGROUND arm is the `detached_foreground` branch above, which reaches the same
    // [`arm_wait_subscription`] with the other [`super::wait_subscriptions::WaitTargetKind`].
    if params.non_blocking == Some(true)
        && let Some(id) = params.id.as_deref()
        && let Some(exact) = effective_id.as_deref()
    {
        return arm_wait_subscription(
            super::wait_subscriptions::WaitTargetKind::Async,
            exact,
            id,
            timeout,
            deps,
        )
        .await;
    }

    // The set of runs in flight when the wait began. In first-completion mode we return as soon as
    // any of THESE leaves the active set — a run spawned by a concurrent turn does not satisfy it.
    let initial_ids: Vec<String> = active
        .iter()
        .map(|run| run_id_of(run).to_string())
        .collect();
    let initial_count = initial_ids.len();
    // ASYNC_NOTIFY_BUG_REPORT F3.4/RC4 — claim the tracked runs BEFORE the first `done()` check,
    // so the claim provably precedes any possible resolution: a delivery for one of these runs
    // now holds off ([`super::watch::InlineAnsweredSink`]) until this wait either answers it
    // inline (the standalone notification is then suppressed and its payload consumed against the
    // decorator's receipt) or releases the claim un-answered — the guard's `Drop` runs on the
    // timeout, abort and error returns alike, so a dead wait can never park a delivery.
    let mut inline_claim = deps.inline_answers.as_ref().map(|ledger| {
        ledger.claim(
            &initial_ids
                .iter()
                .map(|id| super::RunId::from_token(id.clone()))
                .collect::<Vec<_>>(),
        )
    });
    let mut pending: Vec<ActiveRun> = active
        .iter()
        .filter(|run| !needs_attention(run))
        .cloned()
        .collect();
    let mut attention: Vec<ActiveRun> = active
        .iter()
        .filter(|run| needs_attention(run))
        .cloned()
        .collect();

    let done = |pending: &[ActiveRun], attention: &[ActiveRun]| -> bool {
        // With the flag set — the default, and the `bg_wait` tool's mode — a run needing attention
        // breaks the wait in either mode: the caller has to act on it (nudge/resume/interrupt)
        // and blocking longer helps nothing. With it off (auto-drain), attention does NOT end the
        // wait — pi's `stopOnAttention || hasSupervisorTool(run)` gate (`subagent-wait.ts:618-622`;
        // the supervisor-tool escape is the documented [CYRUP-DELTA] on
        // [`WaitDeps::stop_on_attention`]).
        if deps.stop_on_attention && !attention.is_empty() {
            return true;
        }
        // An attention run is still ACTIVE: upstream's `isDone` counts membership in the FULL
        // active set (`subagent-wait.ts:623-630`), of which `attention` is a subset — so with the
        // short-circuit off it must keep the wait open rather than vanish from the accounting.
        // (With the flag on, these terms are vacuous: `attention` is empty past the return above.)
        let is_initial = |run: &&ActiveRun| initial_ids.iter().any(|id| id == run_id_of(run));
        if wait_for_all {
            return !pending.iter().any(|run| is_initial(&run))
                && !attention.iter().any(|run| is_initial(&run));
        }
        let still_active_initial =
            pending.iter().filter(is_initial).count() + attention.iter().filter(is_initial).count();
        still_active_initial < initial_count
    };

    while !done(&pending, &attention) {
        // pending + attention: upstream's abort/timeout messages name `activeInitialRuns` — the
        // FULL active∩initial set (`subagent-wait.ts:640-651`). With `stop_on_attention` off (the
        // drain), the wait can time out while ONLY attention runs remain, and a message built from
        // `pending` alone would then claim "0 run(s) still active" about a live run. With the flag
        // on, `attention` is empty at both sites (the `done` short-circuit exits first), so this
        // changes nothing for the `bg_wait` tool.
        if cancel.is_cancelled() {
            let in_flight: Vec<ActiveRun> =
                pending.iter().chain(attention.iter()).cloned().collect();
            return WaitOutcome::plain(
                format!(
                    "Wait aborted after {}. Still active: {}.",
                    format_duration(elapsed_ms(started_at)),
                    join_ids_with_state(&in_flight)
                ),
                WaitVerdict::Aborted,
            );
        }
        let elapsed = started_at.elapsed();
        if elapsed >= timeout {
            let in_flight: Vec<ActiveRun> =
                pending.iter().chain(attention.iter()).cloned().collect();
            // pi `activeInitialRuns.map((run) => run.id)` (`:683`). `in_flight` is cyrup's
            // `activeInitialRuns` — pending ∪ attention, already the set the message's
            // `n run(s) still active` count comes from — so the ids and the text cannot disagree.
            let active_run_ids: Vec<String> = in_flight
                .iter()
                .map(|run| run_id_of(run).to_string())
                .collect();
            return WaitOutcome::plain(
                format!(
                    "Wait timed out after {} with {} run(s) still active: {}. The runs are \
                     detached and keep going; call wait again or inspect with subagent({{ action: \
                     \"status\" }}).",
                    format_duration(timeout.as_millis().try_into().unwrap_or(u64::MAX)),
                    in_flight.len(),
                    join_ids_with_state(&in_flight)
                ),
                WaitVerdict::WindowElapsed { active_run_ids },
            );
        }

        // Escape hatch #1 + #2, in one place: never sleep past the deadline, and wake instantly on
        // cancellation instead of after the remaining poll interval.
        //
        // SUBA-034 adds the third arm: a completion observed by this process's own
        // [`super::watch::ResultsWatcher`] ends the sleep immediately, with the poll kept underneath
        // as reconciliation — pi's own arrangement, and pi is explicit that the poll, not the
        // event, is the source of truth for what changed. The loop re-reads the run tree from disk
        // on every iteration regardless of WHICH arm woke it, so a spurious or unrelated event
        // costs one extra listing and can never resolve a wait that has not actually finished.
        //
        // `biased;` is load-bearing: with a cancelled token AND a ready wake, an unbiased
        // `select!` picks at random, so an aborted turn could take the wake arm, do one more
        // listing, and only notice the cancellation on the next iteration. Upstream cannot express
        // that race at all (JS awaits settle in order), so the bias is what preserves its
        // behaviour — cancellation first, then the event, then the timer.
        let slice = poll_interval.min(timeout - elapsed);
        let mut wake_closed = false;
        match wake.as_mut() {
            Some(receiver) => {
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => {}
                    // A `Lagged` receiver is a wake-up, not an error: it means MORE completions
                    // landed than the bus buffered, which is the strongest possible signal that
                    // something changed. `Closed` — the watcher was torn down mid-wait — is the one
                    // outcome that must not be treated as an edge: `recv` would then return
                    // instantly forever and spin this loop at 100% CPU, so it retires the
                    // subscription instead and the remainder of the wait polls.
                    outcome = receiver.recv() => {
                        // SCOPE_17 — the wake arm already receives the completion; keeping its
                        // summary is what lets a resolved wait ANSWER instead of merely reporting
                        // a count. Filtered to `initial_ids` so a concurrent turn's run cannot
                        // inject its output into this wait's report.
                        if let Ok(event) = &outcome
                            && initial_ids.iter().any(|id| id == event.run_id.as_str())
                        {
                            observed_summaries
                                .entry(event.run_id.as_str().to_string())
                                .or_insert_with(|| event.summary.clone());
                        }
                        wake_closed = matches!(
                            outcome,
                            Err(tokio::sync::broadcast::error::RecvError::Closed)
                        );
                    }
                    () = tokio::time::sleep(slice) => {}
                }
            }
            None => {
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => {}
                    () = tokio::time::sleep(slice) => {}
                }
            }
        }
        if wake_closed {
            wake = None;
            // Pay the interval this iteration would otherwise have skipped, so retiring the
            // subscription costs the same cadence a wait that never had a bus pays.
            tokio::select! {
                biased;
                () = cancel.cancelled() => {}
                () = tokio::time::sleep(slice) => {}
            }
        }

        active = match active_runs(effective_id.as_deref(), deps).await {
            Ok(active) => active,
            Err(text) => return WaitOutcome::plain(text, WaitVerdict::ListingFailed),
        };
        pending = active
            .iter()
            .filter(|run| !needs_attention(run))
            .cloned()
            .collect();
        attention = active
            .iter()
            .filter(|run| needs_attention(run) && initial_ids.iter().any(|id| id == run_id_of(run)))
            .cloned()
            .collect();
    }

    let terminal_runs = terminal_runs_for(&initial_ids, deps).await;
    // ASYNC_NOTIFY_BUG_REPORT F2 — the shared resolution both renders read from (so neither can
    // drift): drain the bus backlog the poll may have raced past (RC2), replay from disk whatever
    // the bus never carried (RC3), and name what is still genuinely absent.
    let resolved = resolve_results(
        deps,
        wake.as_mut(),
        &initial_ids,
        &mut observed_summaries,
        &terminal_runs,
    )
    .await;
    if let Some(claim) = inline_claim.as_mut() {
        // F3.4 — the values this response actually carries: bus summary, exit drain, or disk
        // replay alike. Recorded on the claim's release, consumed one-use by the watcher's
        // decorator.
        for run_id in &resolved.answered {
            claim.answered(run_id);
        }
    }
    let (finished_count, terminal_summary) = summarize_terminal_runs(&terminal_runs);
    // pi `failedAsyncCount = terminal.filter((run) => run.state === "failed" || run.state ===
    // "partial").length` (`subagent-wait.ts:713`) — cyrup has no `Partial` variant, so the set is
    // `Failed` alone. Feeds the two `deps.failOnFailedRuns` error flips below (`:751`, `:768`).
    let failed_count = terminal_runs
        .iter()
        .filter(|status| status.state == RunState::Failed)
        .count();
    // SUBA-060 / pi `resumeGuidance = formatResumeFirstFailedRunsNote(terminal)`
    // (`subagent-wait.ts:617`), interpolated at `:750`/`:767` immediately after the outcome clause
    // and before the attention note. Empty unless a failed run actually has a revivable child
    // session, so an ordinary wait is unchanged.
    let resume_guidance =
        super::resume_guidance::format_resume_first_failed_runs_note(&terminal_runs);
    // NEW — pi `:716`, inside the try whose catch is `result(message, true)` (`:717-719`). A
    // collection failure is a REPORTED wait, never a propagated error and never a silently empty
    // completions list: the caller asked what finished, and "I could not tell" is an answer it
    // must receive as text.
    //
    // Two cyrup-only consequences of returning here, both deliberate and both already the timeout
    // path's behaviour: `resolved.appendix` is discarded — upstream's catch likewise returns only
    // the message — and `inline_claim`'s guard drops un-answered, releasing the claim without
    // authorising a suppression (ASYNC_NOTIFY_BUG_REPORT F3.4, the same shape
    // `a_timed_out_wait_releases_its_claim_without_authorising_a_suppression` pins).
    let completions = match super::wait_completions::collect_wait_completions(
        &terminal_runs,
        &deps.wait_completions,
        &deps.results_dir,
    )
    .await
    {
        Ok(completions) => completions,
        Err(error) => {
            return WaitOutcome::plain(error.to_string(), WaitVerdict::CompletionsFailed);
        }
    };
    let attention_note = if attention.is_empty() {
        String::new()
    } else {
        format!(
            " {} run(s) need attention: {} — inspect with subagent({{ action: \"status\" }}) then \
             nudge/resume/interrupt.",
            attention.len(),
            join_ids(&attention)
        )
    };
    let elapsed = format_duration(elapsed_ms(started_at));
    let outcome = if terminal_summary.is_empty() {
        String::new()
    } else {
        format!(" Outcome: {terminal_summary}.")
    };
    // pi `formatCompletionRecovery(completions)` (`subagent-wait.ts:732`) — empty unless a
    // TIMED-OUT child left tracked changes without its requested report, so an ordinary wait is
    // unchanged.
    let recovery_note = format_completion_recovery(&completions);

    if wait_for_all {
        let scope = match &params.id {
            Some(id) => format!("run \"{id}\""),
            None => format!("{initial_count} async run(s)"),
        };
        let status = if attention.is_empty() {
            "done"
        } else {
            "attention required"
        };
        let notification = if !attention.is_empty() {
            // Attention-flavoured: describes control state, not values — unchanged (F2.3).
            " Relevant completion/control events have been observed; inspect status if the \
             notification is not visible yet."
        } else if resolved.unanswered.is_empty() {
            // F2.3 — every terminal initial run's value is in this response; the `Results:`
            // appendix IS the notification, so the "inspect status" hint would mislead.
            ""
        } else {
            " Completion events have been observed; inspect status if the notification is not \
             visible yet."
        };
        let text = format!(
            "Waited {elapsed} for {scope}; \
             {status}.{outcome}{recovery_note}{resume_guidance}{attention_note}{notification}"
        );
        // SCOPE_17 + ASYNC_NOTIFY_BUG_REPORT F2 — appended last, after the sentence is already
        // complete. A wait that resolved through the poll now replays its values from the bus
        // backlog (exit drain) or the still-on-disk payload, so the appendix is empty only when
        // there is genuinely nothing to report.
        let text = format!("{text}{}", resolved.appendix);
        // pi `:749-753` — the SAME text either way; the flags only flip the result's error bit,
        // which is what makes auto-drain throw instead of exiting quietly.
        let reported_as_error = (deps.fail_on_failed_runs && failed_count > 0)
            || (deps.fail_on_attention && !attention.is_empty());
        return WaitOutcome::new(
            text,
            WaitVerdict::Resolved {
                failed_runs: failed_count,
                attention_runs: attention.len(),
                reported_as_error,
            },
            completions,
        );
    }

    // First-completion mode.
    let still_running = pending
        .iter()
        .filter(|run| initial_ids.iter().any(|id| id == run_id_of(run)))
        .count();
    let remainder = if still_running > 0 {
        format!(" {still_running} run(s) still in flight — call wait again to catch the next one.")
    } else if attention.is_empty() {
        " No runs remain in flight.".to_string()
    } else {
        " No other runs are waitable until attention is handled.".to_string()
    };
    let progress = if !attention.is_empty() && finished_count == 0 {
        format!(
            "{} of {initial_count} run(s) need attention",
            attention.len()
        )
    } else {
        format!("{finished_count} of {initial_count} run(s) finished")
    };
    let notification = if finished_count > 0 {
        if resolved.unanswered.is_empty() {
            // F2.3 — as in all-mode: every finished run's value is in this response.
            ""
        } else {
            " Completion events for the finished run(s) have been observed; inspect status if the \
             notification is not visible yet."
        }
    } else {
        // Attention-flavoured: describes control state, not values — unchanged (F2.3).
        " Relevant control events have been observed; inspect status if the notification is not \
         visible yet."
    };
    let text = format!(
        "Waited {elapsed}; \
         {progress}.{outcome}{recovery_note}{resume_guidance}{attention_note}{remainder}{notification}"
    );
    // SCOPE_17 + ASYNC_NOTIFY_BUG_REPORT F2 — same appended block as the all-mode return above,
    // same reasoning.
    let text = format!("{text}{}", resolved.appendix);
    // pi `:766-770` — as in the all-mode return above: same text, error bit per the deps flags.
    let reported_as_error = (deps.fail_on_failed_runs && failed_count > 0)
        || (deps.fail_on_attention && !attention.is_empty());
    WaitOutcome::new(
        text,
        WaitVerdict::Resolved {
            failed_runs: failed_count,
            attention_runs: attention.len(),
            reported_as_error,
        },
        completions,
    )
}

/// The per-child blocks for the runs this wait resolved, in `initial_ids` order.
///
/// Since ASYNC_NOTIFY_BUG_REPORT F2 the map this renders is fed by THREE sources — live bus
/// events, the exit drain of the receiver's backlog, and the disk replay of a still-on-disk
/// payload ([`resolve_results`]) — so a wait that resolved through the poll instead of the bus is
/// no longer a value-less outcome. Empty only when there is genuinely nothing to report.
fn format_observed_completions(
    initial_ids: &[String],
    observed: &BTreeMap<String, String>,
) -> String {
    let blocks: Vec<&str> = initial_ids
        .iter()
        .filter_map(|id| observed.get(id).map(String::as_str))
        .filter(|text| !text.trim().is_empty())
        .collect();
    if blocks.is_empty() {
        return String::new();
    }
    format!("\n\nResults:\n\n{}", blocks.join("\n\n"))
}

/// pi `formatCompletionRecovery` (`subagent-wait.ts:354-358`) — the bounded recovery route for
/// every timed-out child a wait covered, or empty.
///
/// Empty for an ordinary wait: [`crate::exec::mutation_evidence::format_timeout_recovery_lines`]
/// returns nothing unless `recovery_needed` is set (pi `mutation-evidence.ts:140`), which requires
/// a TIMED-OUT child that left tracked changes without its requested report. That gate is what
/// keeps a routine timeout quiet, and that function's own doc already names this call site as its
/// `""`-indent consumer (`exec/mutation_evidence/project.rs:93-95`: *"Indent widths in use: `""`
/// (SCOPE_3i / pi `subagent-wait.ts:357`)"*).
fn format_completion_recovery(completions: &[WaitCompletion]) -> String {
    let lines: Vec<String> = completions
        .iter()
        .flat_map(|completion| &completion.results)
        .flat_map(|child| {
            // pi `formatTimeoutRecoveryLines(child.timeoutRecovery)` (`:356`) — the default
            // `indent = ""` (pi `mutation-evidence.ts:138`), which is this surface's width.
            crate::exec::mutation_evidence::format_timeout_recovery_lines(
                child.timeout_recovery.as_ref(),
                "",
            )
        })
        .collect();
    if lines.is_empty() {
        String::new()
    } else {
        format!("\n{}", lines.join("\n"))
    }
}

/// Everything the response can say about the runs this wait tracked (`ASYNC_NOTIFY_BUG_REPORT`
/// F2's shared render tail — both modes read from one resolution, so neither can drift).
///
/// Three sources, in decreasing order of freshness, first one wins per run:
///   1. `observed_summaries` — bus events consumed by the wake arm while the wait was in flight.
///   2. the EXIT DRAIN — events already buffered in the receiver when the poll won the race
///      (RC2: the loop consumed at most one event per iteration, and the record poll routinely
///      observes the terminal state first).
///   3. the DISK REPLAY — the run's still-on-disk `ResultFile`, guaranteed present for a run that
///      went terminal during this turn (RC3: consumption requires a delivered injection, and the
///      injection pump cannot resolve before `wait_for_idle`).
struct ResolvedResults {
    /// Rendered `Results:` appendix (empty string when there is nothing to say).
    appendix: String,
    /// Terminal initial runs whose VALUE is still absent after all three sources (the
    /// records-only replay fallback names steps and artifacts, not the child's answer). Gates the
    /// trailing "inspect status" hint (F2.3): it renders only when this is non-empty.
    unanswered: Vec<String>,
    /// Run ids whose value this response actually carries — F3 records these on the claim.
    answered: Vec<super::RunId>,
}

/// Build [`ResolvedResults`] for a live wait that has just resolved: drain the bus backlog, then
/// replay from disk for every terminal run still missing a value.
async fn resolve_results(
    deps: &WaitDeps,
    wake: Option<&mut tokio::sync::broadcast::Receiver<super::watch::CompletionEvent>>,
    initial_ids: &[String],
    observed_summaries: &mut BTreeMap<String, String>,
    terminal_runs: &[super::RunStatus],
) -> ResolvedResults {
    // 1. Exit drain (F2.1): consume whatever is already buffered, applying the SAME `initial_ids`
    //    filter and `or_insert_with` the wake arm uses, so a re-published completion cannot
    //    overwrite the first observation and a concurrent turn's run cannot inject its output.
    if let Some(receiver) = wake {
        loop {
            match receiver.try_recv() {
                Ok(event) => {
                    if initial_ids.iter().any(|id| id == event.run_id.as_str()) {
                        observed_summaries
                            .entry(event.run_id.as_str().to_string())
                            .or_insert_with(|| event.summary.clone());
                    }
                }
                // `Lagged` means MORE events landed than the bus buffered — keep draining, the
                // remaining ones are still readable.
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break, // Empty | Closed
            }
        }
    }

    /// A non-empty per-run block — the same emptiness test [`format_observed_completions`]
    /// applies, so "has a value" and "renders a block" cannot disagree.
    fn has_value(summaries: &BTreeMap<String, String>, id: &str) -> bool {
        summaries
            .get(id)
            .is_some_and(|text| !text.trim().is_empty())
    }

    let mut answered: Vec<super::RunId> = initial_ids
        .iter()
        .filter(|id| has_value(observed_summaries, id))
        .map(|id| super::RunId::from_token((*id).clone()))
        .collect();

    // 2. Disk replay (F2.2): `terminal_runs_for` has already produced the reconciled status for
    //    every initial id, so the live path pays no second reconcile — it is one `RunPaths` away
    //    from the same replay the already-finished branch performs.
    let mut unanswered: Vec<String> = Vec::new();
    for status in terminal_runs {
        if has_value(observed_summaries, status.run_id.as_str()) {
            continue;
        }
        let paths = super::RunPaths::for_run(&deps.async_root, &deps.results_dir, &status.run_id);
        let (block, carries_value) = terminal_block_for(status, &paths).await;
        if carries_value {
            answered.push(status.run_id.clone());
        } else {
            // The records-only fallback (steps + artifacts pointer) is reported, but it is not
            // the child's answer — the hint stays honest and F3 must not suppress the standalone
            // notification that may still carry it.
            unanswered.push(status.run_id.as_str().to_string());
        }
        observed_summaries.insert(status.run_id.as_str().to_string(), block);
    }

    ResolvedResults {
        appendix: format_observed_completions(initial_ids, observed_summaries),
        unanswered,
        answered,
    }
}

/// What a `wait` with no active runs actually found.
///
/// The branch this replaces was a bare `if active.is_empty()` returning a fixed sentence, which
/// conflated two very different facts: "this id names a run that already finished" and "this id
/// names nothing at all". Naming them separately is what lets the first one answer with the run's
/// own result.
#[derive(Debug)]
enum WaitResolution {
    /// The request names run(s) that reached a terminal state; their outcome is replayed.
    Terminal(TerminalReplay),
    /// Nothing on disk matches — the only case that may say "nothing to wait for".
    Unknown,
}

/// One or more finished runs, rendered as the answer to a wait that arrived after the fact.
#[derive(Debug)]
struct TerminalReplay {
    /// How the caller addressed the runs (an id, or this session's recent history).
    scope: String,
    /// One `(run id, rendered block)` per run — the id is what lets the caller mark the value as
    /// answered inline (`ASYNC_NOTIFY_BUG_REPORT` F2.5/F3.4); the render joins the blocks and is
    /// byte-identical to when this held bare strings.
    blocks: Vec<(super::RunId, String)>,
}

impl TerminalReplay {
    fn render(&self) -> String {
        format!(
            "No runs are still active for {}; reporting the finished run(s).\n\n{}",
            self.scope,
            self.blocks
                .iter()
                .map(|(_, block)| block.as_str())
                .collect::<Vec<_>>()
                .join("\n\n")
        )
    }
}

/// Resolve a wait that found no active runs against the terminal records on disk.
async fn resolve_finished(params: &WaitParams, deps: &WaitDeps) -> WaitResolution {
    match params.id.as_deref() {
        Some(id) => {
            let Ok(Some(location)) = super::run_id_resolver::resolve_async_run_id(
                id,
                &deps.async_root,
                &deps.results_dir,
                deps.session_id
                    .as_deref()
                    .and_then(crate::identity::SessionId::parse)
                    .as_ref(),
            ) else {
                return WaitResolution::Unknown;
            };
            let paths = super::RunPaths::for_run(
                &deps.async_root,
                &deps.results_dir,
                &location.resolved_id,
            );
            // ASYNC_NOTIFY_BUG_REPORT F3.4/RC4 — claim BEFORE the payload read, not after it.
            // `terminal_block` reconciles the run records and reads the result file; a watcher
            // tick landing inside that window would otherwise reach the decorator's
            // `take_answered` check before this response has recorded its answer, and the
            // standalone duplicate would survive precisely in the "wait arrived just after the
            // run finished" case this branch exists to serve. Held across the read, released on
            // return.
            let mut claim = deps
                .inline_answers
                .as_ref()
                .map(|ledger| ledger.claim(std::slice::from_ref(&location.resolved_id)));
            match terminal_block(&paths).await {
                Some(block) => {
                    if let Some(claim) = claim.as_mut() {
                        claim.answered(&location.resolved_id);
                    }
                    WaitResolution::Terminal(TerminalReplay {
                        scope: format!("run \"{id}\""),
                        blocks: vec![(location.resolved_id.clone(), block)],
                    })
                }
                None => WaitResolution::Unknown,
            }
        }
        None => {
            // `all` with nothing active: report this session's recently-finished runs, which is
            // exactly the set a caller that just missed its completions is asking about.
            let Some(session_id) = deps.session_id.as_deref() else {
                return WaitResolution::Unknown;
            };
            let Some(session) = crate::identity::SessionId::parse(session_id) else {
                return WaitResolution::Unknown;
            };
            let Ok(run_ids) = super::terminal_run_index::read_recent_terminal_run_index(
                &deps.async_root,
                Some(&session),
                Some(TERMINAL_REPLAY_LIMIT),
            )
            .await
            else {
                return WaitResolution::Unknown;
            };
            // F3.4/RC4 — as in the single-id arm: the claim covers the whole replay, which reads
            // up to `TERMINAL_REPLAY_LIMIT` payloads off disk. Taken before the first read so no
            // delivery can slip past the decorator mid-loop.
            let mut claim = deps
                .inline_answers
                .as_ref()
                .map(|ledger| ledger.claim(&run_ids));
            let mut blocks = Vec::new();
            for run_id in run_ids {
                let paths = super::RunPaths::for_run(&deps.async_root, &deps.results_dir, &run_id);
                if let Some(block) = terminal_block(&paths).await {
                    if let Some(claim) = claim.as_mut() {
                        claim.answered(&run_id);
                    }
                    blocks.push((run_id, block));
                }
            }
            if blocks.is_empty() {
                WaitResolution::Unknown
            } else {
                WaitResolution::Terminal(TerminalReplay {
                    scope: "this session".to_string(),
                    blocks,
                })
            }
        }
    }
}

/// How many recently-finished runs an `all`-mode replay reports.
const TERMINAL_REPLAY_LIMIT: usize = 10;

/// One finished run's block: its per-child result text when the payload is still on disk, else its
/// recorded steps plus a pointer to the artifacts that outlive the payload.
///
/// Never deletes anything. Consumption is the watcher's sole authority — a `wait` that consumed a
/// payload would race the delivery that is about to notify the orchestrator about it.
async fn terminal_block(paths: &super::RunPaths) -> Option<String> {
    let status = super::control::reconcile_before_control_op(paths)
        .await
        .ok()?;
    if is_active(status.state) {
        return None;
    }
    Some(terminal_block_for(&status, paths).await.0)
}

/// The rendered body for one already-reconciled terminal run, plus whether that body actually
/// CARRIES the run's value (the payload's per-child summary) rather than the records-only
/// fallback. Split out of [`terminal_block`] (`ASYNC_NOTIFY_BUG_REPORT` F2.2) so the LIVE wait
/// path can reuse it without paying a second reconcile — [`terminal_runs_for`] has already
/// produced exactly these statuses — and so [`resolve_results`] can tell an answered value from a
/// pointer when deciding the trailing hint (F2.3) and the inline-answer set (F3).
///
/// Never deletes anything, same as [`terminal_block`]: consumption is the watcher's sole
/// authority.
async fn terminal_block_for(status: &super::RunStatus, paths: &super::RunPaths) -> (String, bool) {
    let mut lines = vec![format!(
        "{} ({})",
        status.run_id,
        super::run_status::run_state_label(status.state)
    )];

    let payload = match status.session_id.as_ref() {
        Some(session_id) => paths.resolve_result(session_id, &status.run_id).await,
        None => None,
    };
    let result = match &payload {
        Some(path) => tokio::fs::read(path)
            .await
            .ok()
            .and_then(|bytes| serde_json::from_slice::<super::ResultFile>(&bytes).ok()),
        None => None,
    };

    match result {
        Some(result) => {
            lines.push(super::watch::result_display_summary(&result));
            (lines.join("\n"), true)
        }
        None => {
            // The payload is gone (consumed by its delivery, or destroyed before it). The run's
            // own records still describe what happened, so the answer is the steps plus a pointer
            // rather than a shrug.
            for step in &status.steps {
                lines.push(format!(
                    "{}: {}",
                    step.agent,
                    super::run_status::step_state_label(step.status)
                ));
            }
            lines.push(format!("Artifacts: {}", paths.run_dir.display()));
            (lines.join("\n"), false)
        }
    }
}

fn elapsed_ms(since: Instant) -> u64 {
    since.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
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
    use crate::background::watch::{
        ClassifiedOutcome, CompletionBus, CompletionEvent, InlineAnswerLedger,
    };
    use crate::background::{RunId, RunMode, RunPaths, RunStatus};

    /// A completion event for `run` carrying `summary` — the input BOTH the wake arm and the exit
    /// drain consume, and both filter it against `initial_ids`.
    fn completion(run: &RunId, summary: &str) -> CompletionEvent {
        CompletionEvent {
            run_id: run.clone(),
            outcome: ClassifiedOutcome::Completed,
            summary: summary.to_string(),
        }
    }

    /// A terminal [`RunStatus`] whose recorded session matches the one `watch::tests`' published
    /// payloads carry.
    ///
    /// The session is REQUIRED, not decoration: [`terminal_block_for`] resolves the payload through
    /// [`RunPaths::resolve_result`], which is session-partitioned, so a `session_id: None` status
    /// can only ever take the records-only fallback — which is exactly why every pre-existing test
    /// in this module still sees the "inspect status" hint.
    fn terminal_status(run: &RunId) -> RunStatus {
        let mut status = RunStatus::queued(run.clone(), RunMode::Single, Some(1));
        status.state = RunState::Complete;
        status.session_id = crate::identity::SessionId::parse_opt(Some("test-session"));
        status
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        async_root: PathBuf,
        results_dir: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let async_root = dir.path().join("async");
            let results_dir = dir.path().join("results");
            std::fs::create_dir_all(&async_root).expect("mkdir async");
            std::fs::create_dir_all(&results_dir).expect("mkdir results");
            Self {
                _dir: dir,
                async_root,
                results_dir,
            }
        }

        fn deps(&self, enabled: bool) -> WaitDeps {
            WaitDeps {
                async_root: self.async_root.clone(),
                results_dir: self.results_dir.clone(),
                poll_interval: Duration::from_millis(MIN_POLL_INTERVAL_MS),
                enabled,
                session_id: None,
                // SUBA-034: the no-bus shape is upstream's own documented degradation, and it is
                // what every pre-existing test in this module exercises — so the polling path stays
                // covered exactly as before and only the two new tests opt into a bus.
                completion_bus: None,
                // The `bg_wait` tool's defaults (pi `subagent-wait.ts:618,708,725`); the auto-drain
                // tests override through the builders.
                stop_on_attention: true,
                fail_on_failed_runs: false,
                fail_on_attention: false,
                // No ledger — the documented degradation every pre-existing test exercises, same
                // as `completion_bus: None` above.
                inline_answers: None,
                wait_completions: std::sync::Arc::new(
                    crate::background::wait_completions::WaitCompletionStore::default(),
                ),
                // SCOPE_11 — no manager, which is upstream's own headless/no-UI shape and what
                // every pre-existing test in this module exercises. The `nonBlocking` tests below
                // opt into a fake arming seam explicitly.
                subscribe: None,
                // VL-S11b — no executor, so no remembered foreground runs; the detach tests below
                // attach a fake source explicitly.
                detached_foreground: None,
            }
        }

        fn paths(&self, run_id: &RunId) -> RunPaths {
            RunPaths::for_run(&self.async_root, &self.results_dir, run_id)
        }

        /// Write a `status.json` for a run in the given state — the on-disk shape
        /// `list_active_runs` reads.
        fn write_status(&self, run_id: &RunId, state: RunState, attention: bool) {
            self.write_status_for_session(run_id, state, attention, None);
        }

        /// SUBA-031 — the same writer, with the run's OWN recorded orchestrator session
        /// (pi `AsyncStatus.sessionId`, stamped from `config.sessionId` at
        /// `subagent-runner.ts:2088`).
        fn write_status_for_session(
            &self,
            run_id: &RunId,
            state: RunState,
            attention: bool,
            session_id: Option<&str>,
        ) {
            let paths = self.paths(run_id);
            std::fs::create_dir_all(&paths.run_dir).expect("mkdir run dir");
            let mut status = RunStatus::queued(run_id.clone(), RunMode::Single, Some(1));
            status.state = state;
            status.session_id = crate::identity::SessionId::parse_opt(session_id);
            if attention {
                status.telemetry.activity_state = Some(ActivityState::NeedsAttention);
            }
            std::fs::write(
                &paths.status,
                serde_json::to_string(&status).expect("status serializes"),
            )
            .expect("write status.json");
        }

        /// Write the authoritative terminal `ResultFile` — the real completion signal a detached
        /// hop-2 runner emits, and the one thing that flips a run terminal for every reader.
        fn settle(async_root: &std::path::Path, results_dir: &std::path::Path, run_id: &RunId) {
            let paths = RunPaths::for_run(async_root, results_dir, run_id);
            let result = crate::background::ResultFile {
                schedule_origin: None,
                id: run_id.clone(),
                run_id: run_id.clone(),
                agent: "worker".to_string(),
                mode: RunMode::Single,
                state: RunState::Complete,
                success: true,
                cwd: PathBuf::from("/tmp"),
                session_file: None,
                session_id: None,
                completion_owner_id: None,
                results: Vec::new(),
                workflow_children: None,
                workflow_receipt: None,
            };
            std::fs::write(
                &paths.legacy_result_root,
                serde_json::to_string(&result).expect("serializes"),
            )
            .expect("write result file");
        }
    }

    #[tokio::test]
    async fn with_no_active_runs_wait_returns_immediately() {
        let fx = Fixture::new();
        let text = wait_for_subagents(&WaitParams::default(), &CancelToken::new(), &fx.deps(true))
            .await
            .text;
        assert_eq!(
            text,
            "No active async runs in this session. Nothing to wait for."
        );
    }

    /// SUBA-031 — `wait` is scoped to the SESSION, not merely to the cwd, so two cyrup sessions in
    /// the same repository no longer block on each other's background runs.
    ///
    /// The first assertion is the control and it is not decoration: it proves the foreign run is on
    /// disk, active, and would otherwise be waited on — without it, the scoped assertion below
    /// would pass just as happily against an empty async root.
    ///
    /// The message is asserted too, because it was the visible half of the defect: the empty-set
    /// text has always said "in this session" while the scope was the cwd, so before this change
    /// the two sentences the tool could produce were both wrong for the same run.
    #[tokio::test]
    async fn wait_ignores_another_sessions_background_run() {
        let fx = Fixture::new();
        let foreign = RunId::new();
        fx.write_status_for_session(&foreign, RunState::Running, false, Some("session-b"));

        // Control: with no session identity (pi's falsy `sessionId`) the run IS in scope, so the
        // wait blocks on it — which is exactly what session A used to do.
        let unscoped = tokio::time::timeout(
            Duration::from_millis(700),
            wait_for_subagents(&WaitParams::default(), &CancelToken::new(), &fx.deps(true)),
        )
        .await;
        assert!(
            unscoped.is_err(),
            "an unscoped wait must block on the foreign run, got {unscoped:?}"
        );

        // Scoped to session A: the foreign run is out of scope, so the wait returns at once with
        // the empty-set text — which is now true rather than merely printed.
        let deps = WaitDeps {
            session_id: Some("session-a".to_string()),
            ..fx.deps(true)
        };
        let text = tokio::time::timeout(
            Duration::from_secs(5),
            wait_for_subagents(&WaitParams::default(), &CancelToken::new(), &deps),
        )
        .await
        .expect("a session-scoped wait must not block on another session's run")
        .text;
        assert_eq!(
            text,
            "No active async runs in this session. Nothing to wait for."
        );
    }

    /// THE behavior SUBA-004 exists for: `wait` actually BLOCKS while a background run is in
    /// flight, and returns only once that run's terminal result appears.
    #[tokio::test]
    async fn wait_blocks_until_the_background_run_actually_settles() {
        let fx = Fixture::new();
        let run_id = RunId::new();
        fx.write_status(&run_id, RunState::Running, false);

        // Prove it is still blocking a full poll interval in: nothing has settled yet.
        let early = tokio::time::timeout(
            Duration::from_millis(700),
            wait_for_subagents(&WaitParams::default(), &CancelToken::new(), &fx.deps(true)),
        )
        .await;
        assert!(
            early.is_err(),
            "wait must still be blocking while the run is Running, got {early:?}"
        );

        // Now settle the run the way a real detached runner does, and the SAME call must return.
        let settle = {
            let run_id = run_id.clone();
            let async_root = fx.async_root.clone();
            let results_dir = fx.results_dir.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                Fixture::settle(&async_root, &results_dir, &run_id);
            })
        };

        let text = tokio::time::timeout(
            Duration::from_secs(10),
            wait_for_subagents(&WaitParams::default(), &CancelToken::new(), &fx.deps(true)),
        )
        .await
        .expect("wait must resolve once the run settles")
        .text;
        settle.await.expect("settler task");

        assert!(
            text.contains("1 of 1 run(s) finished"),
            "the summary must report the finished run: {text}"
        );
        assert!(
            text.contains("Outcome: 1 complete."),
            "and how it came out: {text}"
        );
        assert!(
            !text.contains("Resume-first"),
            "SUBA-060 guidance is for FAILED runs only; a complete run must not carry it: {text}"
        );
    }

    /// SUBA-060 — a run that FAILS while the wait is in flight, and whose child session was
    /// persisted, must come back with pi's resume-first sentence naming the literal `resume` call
    /// (`subagent-wait.ts:617`). Without it the orchestrator's default response to "1 failed" is to
    /// spawn a replacement child and re-pay for every turn the failed one already took.
    ///
    /// The sibling assertion in
    /// [`wait_blocks_until_the_background_run_actually_settles`](Self) is the vacuous-pass guard on
    /// the other side: it pins that a COMPLETE run does not get the sentence, so this test cannot
    /// pass merely because the note is emitted unconditionally.
    #[tokio::test]
    async fn a_failed_run_with_a_persisted_child_session_returns_resume_first_guidance() {
        let fx = Fixture::new();
        let run_id = RunId::new();
        fx.write_status(&run_id, RunState::Running, false);

        let transcript = fx._dir.path().join("child-session.jsonl");
        std::fs::write(&transcript, b"{}").expect("write child transcript");

        let settle = {
            let run_id = run_id.clone();
            let paths = fx.paths(&run_id);
            let transcript = transcript.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                let mut status = RunStatus::queued(run_id, RunMode::Single, Some(1));
                status.state = RunState::Failed;
                status.ended_at = Some(crate::time::now_epoch_millis());
                let mut step = crate::background::StepStatus::pending("worker");
                step.status = crate::background::StepState::Failed;
                step.session_file = Some(transcript);
                status.steps = vec![step];
                std::fs::write(
                    &paths.status,
                    serde_json::to_string(&status).expect("status serializes"),
                )
                .expect("write failed status");
            })
        };

        let text = tokio::time::timeout(
            Duration::from_secs(10),
            wait_for_subagents(&WaitParams::default(), &CancelToken::new(), &fx.deps(true)),
        )
        .await
        .expect("wait must resolve once the run reaches a terminal state")
        .text;
        settle.await.expect("settler task");

        assert!(
            text.contains("Outcome: 1 failed."),
            "the run must be seen as failed: {text}"
        );
        assert!(
            text.contains(&format!(
                " Resume-first: failed run \"{}\" has a persisted child session. Revive the \
                 original run with subagent({{ action: \"resume\", id: \"{}\", message: \
                 \"Continue from the persisted child session and report the result.\" }}) before \
                 reporting failure or launching a replacement. Launch a replacement only if revive \
                 fails or the user explicitly asks for one.",
                run_id.as_str(),
                run_id.as_str()
            )),
            "pi's verbatim resume-first note must be present: {text}"
        );
    }

    /// Escape hatch #1: a wedged run cannot hold the orchestrator past the caller's timeout, and
    /// the message says the runs keep going so the caller knows nothing was killed.
    #[tokio::test]
    async fn a_hung_run_is_released_by_the_timeout_and_reported_as_still_active() {
        let fx = Fixture::new();
        let run_id = RunId::new();
        fx.write_status(&run_id, RunState::Running, false);

        let started = Instant::now();
        let outcome = wait_for_subagents(
            &WaitParams {
                timeout_ms: Some(400),
                ..WaitParams::default()
            },
            &CancelToken::new(),
            &fx.deps(true),
        )
        .await;
        assert!(
            started.elapsed() >= Duration::from_millis(350),
            "it must actually have waited"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "and not far past the deadline"
        );
        // A timeout is NOT an error (§0.1): it resolves to `WindowElapsed`, never `is_error()`.
        assert!(!outcome.is_error(), "a timeout must not be an error result");
        let WaitVerdict::WindowElapsed { active_run_ids } = &outcome.verdict else {
            panic!(
                "a timeout must resolve to WindowElapsed, got {:?}",
                outcome.verdict
            );
        };
        assert!(
            active_run_ids.contains(&run_id.as_str().to_string()),
            "the wedged run must be named in active_run_ids: {active_run_ids:?}"
        );
        let err = &outcome.text;
        assert!(err.starts_with("Wait timed out after "), "got: {err}");
        assert!(err.contains("1 run(s) still active"), "got: {err}");
        assert!(
            err.contains(run_id.as_str()),
            "the wedged run must be named: {err}"
        );
        assert!(
            err.contains("The runs are detached and keep going"),
            "got: {err}"
        );
    }

    /// `wait`'s own view of "what is still running" is
    /// [`super::run_status::list_active_runs`], and that listing now enumerates the ACTIVE-run
    /// index rather than the async root — so this is the wait path reaching
    /// [`crate::background::active_run_index::read_live_active_run_ids`] end to end, through
    /// [`wait_for_subagents`] and nothing else.
    ///
    /// Both runs are `running` on disk and only one carries an index marker, which is the single
    /// asymmetry a directory scan cannot see: a scan would name two runs in `active_run_ids`.
    #[tokio::test]
    async fn the_wait_sees_the_active_runs_the_index_lists() {
        let fx = Fixture::new();
        let indexed = RunId::from_token("run0waitidx");
        let unindexed = RunId::from_token("run0waitscan");
        fx.write_status(&indexed, RunState::Running, false);
        fx.write_status(&unindexed, RunState::Running, false);
        let paths = fx.paths(&indexed);
        let status: RunStatus = serde_json::from_slice(
            &std::fs::read(&paths.status).expect("read the status just written"),
        )
        .expect("status parses");
        crate::background::active_run_index::update_active_run_index(&paths.run_dir, &status)
            .await
            .expect("index the launch");

        let outcome = wait_for_subagents(
            &WaitParams {
                timeout_ms: Some(400),
                ..WaitParams::default()
            },
            &CancelToken::new(),
            &fx.deps(true),
        )
        .await;
        let WaitVerdict::WindowElapsed { active_run_ids } = &outcome.verdict else {
            panic!(
                "a timeout must resolve to WindowElapsed, got {:?}",
                outcome.verdict
            );
        };
        assert_eq!(
            active_run_ids,
            &vec![indexed.as_str().to_string()],
            "the wait waited on exactly the index's listing"
        );
        assert!(
            !outcome.text.contains(unindexed.as_str()),
            "the unindexed run is not in the report: {}",
            outcome.text
        );
    }

    /// Escape hatch #2: cancelling the turn releases the wait promptly — well inside one production
    /// poll interval, proving the sleep really is a `select!` on the token and not a fixed sleep.
    #[tokio::test]
    async fn cancelling_the_turn_releases_the_wait_without_waiting_out_the_poll_interval() {
        let fx = Fixture::new();
        let run_id = RunId::new();
        fx.write_status(&run_id, RunState::Running, false);

        let cancel = CancelToken::new();
        let canceller = {
            let cancel = cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(150)).await;
                cancel.cancel();
            })
        };

        let deps = WaitDeps {
            // A 30s poll interval: if cancellation did not wake the sleep, this test would hang.
            poll_interval: Duration::from_secs(30),
            ..fx.deps(true)
        };
        let started = Instant::now();
        let outcome = tokio::time::timeout(
            Duration::from_secs(10),
            wait_for_subagents(&WaitParams::default(), &cancel, &deps),
        )
        .await
        .expect("cancellation must break the wait, not wait out the 30s poll");
        canceller.await.expect("canceller task");

        assert!(
            started.elapsed() < Duration::from_secs(5),
            "cancellation must wake the sleep immediately, took {:?}",
            started.elapsed()
        );
        assert_eq!(
            outcome.verdict,
            WaitVerdict::Aborted,
            "an abort is an error result"
        );
        assert!(
            outcome.is_error(),
            "an aborted wait is reported as an error result"
        );
        let err = outcome.text;
        assert!(err.starts_with("Wait aborted after "), "got: {err}");
        assert!(
            err.contains(run_id.as_str()),
            "the still-active run must be named: {err}"
        );
    }

    /// A child that needs attention breaks the wait immediately in either mode — otherwise a stuck
    /// child stalls the loop until the timeout, which is exactly what the caller must not sit
    /// through.
    #[tokio::test]
    async fn a_run_needing_attention_breaks_the_wait_at_once() {
        let fx = Fixture::new();
        let run_id = RunId::new();
        fx.write_status(&run_id, RunState::Running, true);

        let started = Instant::now();
        let text = wait_for_subagents(
            &WaitParams {
                all: Some(true),
                ..WaitParams::default()
            },
            &CancelToken::new(),
            &fx.deps(true),
        )
        .await
        .text;
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "it must not have polled at all"
        );
        assert!(text.contains("attention required"), "got: {text}");
        assert!(text.contains("1 run(s) need attention"), "got: {text}");
        assert!(text.contains(run_id.as_str()), "got: {text}");
    }

    /// Auto-drain's mode (pi `stopOnAttention: false`, `auto-drain.ts:61`): a needs-attention run
    /// does NOT break the wait — it is waited THROUGH, still counted as active, and therefore
    /// bounded by the timeout. Without the accounting half of this (attention runs staying in the
    /// "still active" count), `all: true` would report done while the run is live; without the
    /// gate half, the drain loop would spin hot. The timeout message must also NAME the attention
    /// run — upstream builds it from the full active∩initial set (`subagent-wait.ts:640-651`).
    #[tokio::test]
    async fn with_stop_on_attention_off_an_attention_run_is_waited_through_not_resolved() {
        let fx = Fixture::new();
        let run_id = RunId::new();
        fx.write_status(&run_id, RunState::Running, true);

        let deps = fx.deps(true).with_stop_on_attention(false);
        let started = Instant::now();
        let outcome = wait_for_subagents(
            &WaitParams {
                all: Some(true),
                timeout_ms: Some(600),
                ..WaitParams::default()
            },
            &CancelToken::new(),
            &deps,
        )
        .await;
        assert!(
            started.elapsed() >= Duration::from_millis(550),
            "it must actually have waited through the attention, took {:?}",
            started.elapsed()
        );
        // With the flag off the wait must run out its window — not resolve on attention — and a
        // window that merely elapsed is NOT an error (§0.1).
        assert!(
            !outcome.is_error(),
            "a window-elapsed wait through attention must not be an error result"
        );
        assert!(matches!(outcome.verdict, WaitVerdict::WindowElapsed { .. }));
        let err = outcome.text;
        assert!(err.starts_with("Wait timed out after "), "got: {err}");
        assert!(err.contains("1 run(s) still active"), "got: {err}");
        assert!(
            err.contains(run_id.as_str()),
            "the attention run must be named as still active: {err}"
        );
    }

    /// pi `deps.failOnFailedRuns` (`subagent-wait.ts:708`): the SAME summary text, with the
    /// result's error bit flipped — which is what makes auto-drain throw on a failed child instead
    /// of exiting quietly. The control half (default flags → `Ok` with the same content) is pinned
    /// by [`a_failed_run_with_a_persisted_child_session_returns_resume_first_guidance`] above.
    #[tokio::test]
    async fn fail_on_failed_runs_reports_a_failed_outcome_as_an_error_with_the_same_text() {
        let fx = Fixture::new();
        let run_id = RunId::new();
        fx.write_status(&run_id, RunState::Running, false);

        let settle = {
            let run_id = run_id.clone();
            let paths = fx.paths(&run_id);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                let mut status = RunStatus::queued(run_id, RunMode::Single, Some(1));
                status.state = RunState::Failed;
                status.ended_at = Some(crate::time::now_epoch_millis());
                std::fs::write(
                    &paths.status,
                    serde_json::to_string(&status).expect("status serializes"),
                )
                .expect("write failed status");
            })
        };

        let deps = fx.deps(true).with_fail_on_failed_runs(true);
        let outcome = tokio::time::timeout(
            Duration::from_secs(10),
            wait_for_subagents(&WaitParams::default(), &CancelToken::new(), &deps),
        )
        .await
        .expect("wait must resolve once the run fails");
        settle.await.expect("settler task");

        assert!(
            outcome.is_error(),
            "a failed run must flip the resolved wait to an error"
        );
        assert!(matches!(
            outcome.verdict,
            WaitVerdict::Resolved {
                reported_as_error: true,
                ..
            }
        ));
        let err = outcome.text;
        assert!(
            err.starts_with("Waited "),
            "the payload is the ordinary summary, not a new error shape: {err}"
        );
        assert!(err.contains("Outcome: 1 failed."), "got: {err}");
    }

    /// pi `deps.failOnAttention` (`subagent-wait.ts:725`): unresolved attention flips the resolved
    /// wait's error bit, again with the SAME text the default mode returns as `Ok`.
    #[tokio::test]
    async fn fail_on_attention_reports_an_attention_resolution_as_an_error() {
        let fx = Fixture::new();
        let run_id = RunId::new();
        fx.write_status(&run_id, RunState::Running, true);

        let deps = fx.deps(true).with_fail_on_attention(true);
        let outcome = wait_for_subagents(
            &WaitParams {
                all: Some(true),
                ..WaitParams::default()
            },
            &CancelToken::new(),
            &deps,
        )
        .await;
        assert!(
            outcome.is_error(),
            "attention must flip the resolved wait to an error"
        );
        assert!(matches!(
            outcome.verdict,
            WaitVerdict::Resolved {
                reported_as_error: true,
                ..
            }
        ));
        let err = outcome.text;
        assert!(err.contains("attention required"), "got: {err}");
        assert!(err.contains("1 run(s) need attention"), "got: {err}");
        assert!(err.contains(run_id.as_str()), "got: {err}");
    }

    /// First-completion (default) vs `all: true`: with two runs in flight, settling ONE releases
    /// the default wait and leaves `all: true` still blocking. Both runs must be active when the
    /// wait starts — a run that was already terminal is not in the initial set and satisfies
    /// nothing (pi's `initialIds` rule).
    #[tokio::test]
    async fn first_completion_returns_early_while_all_true_keeps_waiting() {
        let fx = Fixture::new();
        let a = RunId::new();
        let b = RunId::new();
        fx.write_status(&a, RunState::Running, false);
        fx.write_status(&b, RunState::Running, false);

        // `all: true` is NOT satisfied while both are running.
        let blocked = tokio::time::timeout(
            Duration::from_millis(800),
            wait_for_subagents(
                &WaitParams {
                    all: Some(true),
                    ..WaitParams::default()
                },
                &CancelToken::new(),
                &fx.deps(true),
            ),
        )
        .await;
        assert!(
            blocked.is_err(),
            "all:true must block while both run, got {blocked:?}"
        );

        let settle = {
            let a = a.clone();
            let async_root = fx.async_root.clone();
            let results_dir = fx.results_dir.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                Fixture::settle(&async_root, &results_dir, &a);
            })
        };

        let text = tokio::time::timeout(
            Duration::from_secs(10),
            wait_for_subagents(&WaitParams::default(), &CancelToken::new(), &fx.deps(true)),
        )
        .await
        .expect("first-completion must return as soon as ONE of the two settles")
        .text;
        settle.await.expect("settler task");
        assert!(text.contains("1 of 2 run(s) finished"), "got: {text}");
        assert!(text.contains("1 run(s) still in flight"), "got: {text}");

        // ...and `all: true` is STILL not satisfied, because run b never settled.
        let still_blocked = tokio::time::timeout(
            Duration::from_millis(800),
            wait_for_subagents(
                &WaitParams {
                    all: Some(true),
                    ..WaitParams::default()
                },
                &CancelToken::new(),
                &fx.deps(true),
            ),
        )
        .await;
        assert!(
            still_blocked.is_err(),
            "all:true must keep waiting on the unsettled run, got {still_blocked:?}"
        );
        assert!(!b.as_str().is_empty(), "the second run id is real");
    }

    /// An id that matches several active runs is rejected rather than silently waiting on whichever
    /// one sorted first (pi's ambiguity guard).
    #[tokio::test]
    async fn an_ambiguous_id_prefix_is_rejected() {
        let fx = Fixture::new();
        // `RunId::from_token` keeps the literal token, so two ids can share a prefix on purpose.
        let a = RunId::from_token("abc111".to_string());
        let b = RunId::from_token("abc222".to_string());
        fx.write_status(&a, RunState::Running, false);
        fx.write_status(&b, RunState::Running, false);

        let outcome = wait_for_subagents(
            &WaitParams {
                id: Some("abc".to_string()),
                ..WaitParams::default()
            },
            &CancelToken::new(),
            &fx.deps(true),
        )
        .await;
        assert_eq!(
            outcome.verdict,
            WaitVerdict::AmbiguousId,
            "an ambiguous prefix must be rejected"
        );
        assert!(outcome.is_error());
        let err = outcome.text;
        assert!(err.starts_with("Ambiguous async run id prefix \"abc\" matched 2 active runs:"));
        assert!(err.ends_with("Pass a longer id."), "got: {err}");
    }

    #[tokio::test]
    async fn an_unmatched_id_says_so_instead_of_blocking() {
        let fx = Fixture::new();
        let run_id = RunId::new();
        fx.write_status(&run_id, RunState::Running, false);
        let text = wait_for_subagents(
            &WaitParams {
                id: Some("nope".to_string()),
                ..WaitParams::default()
            },
            &CancelToken::new(),
            &fx.deps(true),
        )
        .await
        .text;
        assert_eq!(text, "No active run matched \"nope\". Nothing to wait for.");
    }

    /// The config/env gate: disabled means "return at once without blocking", never "silently
    /// pretend the runs finished".
    #[tokio::test]
    async fn a_disabled_wait_returns_immediately_without_blocking() {
        let fx = Fixture::new();
        let run_id = RunId::new();
        fx.write_status(&run_id, RunState::Running, false);
        let started = Instant::now();
        let text = wait_for_subagents(&WaitParams::default(), &CancelToken::new(), &fx.deps(false))
            .await
            .text;
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(
            text.starts_with("Wait tool is disabled by config.waitTool or "),
            "got: {text}"
        );
        assert!(text.contains("Active runs keep going"), "got: {text}");
    }

    #[test]
    fn the_enabled_gate_resolves_env_over_config_and_defaults_to_on() {
        assert_eq!(resolve_wait_tool_enabled(None, None), Ok(true));
        assert_eq!(
            resolve_wait_tool_enabled(Some(&WaitToolSetting::Enabled(false)), None),
            Ok(false)
        );
        assert_eq!(
            resolve_wait_tool_enabled(
                Some(&WaitToolSetting::Object {
                    enabled: Some(false)
                }),
                None
            ),
            Ok(false)
        );
        assert_eq!(
            resolve_wait_tool_enabled(Some(&WaitToolSetting::Object { enabled: None }), None),
            Ok(true)
        );
        // Env wins over config, in both directions.
        assert_eq!(
            resolve_wait_tool_enabled(Some(&WaitToolSetting::Enabled(false)), Some("on")),
            Ok(true)
        );
        assert_eq!(
            resolve_wait_tool_enabled(Some(&WaitToolSetting::Enabled(true)), Some("disabled")),
            Ok(false)
        );
        // An unrecognized value is a configuration ERROR, not a silently-ignored one.
        assert_eq!(
            resolve_wait_tool_enabled(None, Some("maybe")),
            Err(
                "CYRUP_SUBAGENT_WAIT_TOOL_ENABLED must be one of true/false, 1/0, yes/no, on/off, \
                 or enabled/disabled."
                    .to_string()
            )
        );
    }

    #[test]
    fn wait_tool_setting_accepts_both_pi_shapes() {
        assert_eq!(
            serde_json::from_value::<WaitToolSetting>(serde_json::json!(false)).expect("bool form"),
            WaitToolSetting::Enabled(false)
        );
        assert_eq!(
            serde_json::from_value::<WaitToolSetting>(serde_json::json!({"enabled": true}))
                .expect("object form"),
            WaitToolSetting::Object {
                enabled: Some(true)
            }
        );
    }

    #[test]
    fn format_duration_matches_pi() {
        assert_eq!(format_duration(999), "999ms");
        assert_eq!(format_duration(1500), "1.5s");
        assert_eq!(format_duration(59_999), "60.0s");
        assert_eq!(format_duration(65_000), "1m5s");
        assert_eq!(format_duration(DEFAULT_TIMEOUT_MS), "30m0s");
    }

    // =============================================================================================
    // SUBA-034 — the event wake
    // =============================================================================================

    /// The item's own Verify, expressed against the mechanism rather than against wall-clock luck:
    /// a wait whose poll interval is FIVE SECONDS must still return promptly once the completion
    /// is observed and published, because the bus arm — not the timer — is what releases it.
    ///
    /// Deliberately not asserting "~50 ms": cyrup's publisher is the orchestrator's result-file
    /// watcher rather than the run itself, so the production floor is that watcher's own 500 ms
    /// cadence (see the module docs' `[CYRUP-DELTA]`). What IS asserted is the property the port
    /// adds — that the second, independent 1 s wait cadence no longer applies — and the 5 s /
    /// 2 s gap makes a regression to pure polling fail this test rather than merely slow it.
    #[tokio::test]
    async fn a_published_completion_releases_a_wait_long_before_its_poll_interval() {
        let fx = Fixture::new();
        let run = RunId::from_token("run-bus-wake");
        fx.write_status(&run, RunState::Running, false);

        let bus = crate::background::watch::CompletionBus::new();
        let deps = WaitDeps {
            poll_interval: Duration::from_secs(5),
            completion_bus: Some(bus.clone()),
            ..fx.deps(true)
        };

        let async_root = fx.async_root.clone();
        let results_dir = fx.results_dir.clone();
        let settle_run = run.clone();
        let settler = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            // The completion is the FILE — published second, exactly as the watcher does it, so a
            // waiter that woke on the event and re-read the tree finds terminal state waiting.
            Fixture::settle(&async_root, &results_dir, &settle_run);
            bus.publish(crate::background::watch::CompletionEvent {
                run_id: settle_run,
                outcome: crate::background::watch::ClassifiedOutcome::Completed,
                summary: String::new(),
            });
        });

        let started = Instant::now();
        let text = wait_for_subagents(&WaitParams::default(), &CancelToken::new(), &deps)
            .await
            .text;
        let elapsed = started.elapsed();
        settler.await.expect("settler task");

        assert!(
            elapsed < Duration::from_secs(2),
            "the bus arm must release the wait; a pure-poll regression would take ~5s, took {elapsed:?}"
        );
        assert!(
            text.contains("1 of 1 run(s) finished"),
            "the wake must still resolve through the on-disk reconciliation, not from the event: {text}"
        );
    }

    /// The event is a WAKE-UP, never the answer. A publish for a run that has NOT settled must
    /// leave the wait blocked — the poll under it stays the source of truth, which is upstream's
    /// own stated contract for the same subscription.
    ///
    /// Asserts presence before absence: the wait is first shown to be genuinely resolvable (it
    /// completes once the run really settles), so the "still blocked" half cannot pass because the
    /// wait was broken outright.
    #[tokio::test]
    async fn a_spurious_wake_does_not_resolve_a_wait_whose_run_is_still_active() {
        let fx = Fixture::new();
        let run = RunId::from_token("run-bus-spurious");
        fx.write_status(&run, RunState::Running, false);

        let bus = crate::background::watch::CompletionBus::new();
        let deps = WaitDeps {
            poll_interval: Duration::from_secs(5),
            completion_bus: Some(bus.clone()),
            ..fx.deps(true)
        };

        let cancel = CancelToken::new();
        let params = WaitParams::default();
        let mut waiting = Box::pin(wait_for_subagents(&params, &cancel, &deps));

        // Three wakes with nothing settled: each one costs the loop a listing and must put it
        // straight back to sleep.
        for _ in 0..3 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            bus.publish(crate::background::watch::CompletionEvent {
                run_id: RunId::from_token("run-bus-spurious"),
                outcome: crate::background::watch::ClassifiedOutcome::Completed,
                summary: String::new(),
            });
            assert!(
                tokio::time::timeout(Duration::from_millis(60), &mut waiting)
                    .await
                    .is_err(),
                "a wake for a still-running run must not resolve the wait"
            );
        }

        // …and the wait really was resolvable all along.
        Fixture::settle(&fx.async_root, &fx.results_dir, &run);
        bus.publish(crate::background::watch::CompletionEvent {
            run_id: run,
            outcome: crate::background::watch::ClassifiedOutcome::Completed,
            summary: String::new(),
        });
        let text = tokio::time::timeout(Duration::from_secs(2), waiting)
            .await
            .expect("the wait resolves once the run genuinely settles")
            .text;
        assert!(text.contains("1 of 1 run(s) finished"), "{text}");
    }

    /// A torn-down watcher (`SessionStart` replacing the session's watch, or the extension
    /// dropping it) closes every outstanding receiver. That must retire the subscription and leave
    /// the wait polling — NOT spin the loop, which is what an un-retired `Closed` receiver does
    /// (`recv` returns instantly, forever).
    #[tokio::test]
    async fn a_closed_bus_degrades_to_polling_instead_of_spinning() {
        let fx = Fixture::new();
        let run = RunId::from_token("run-bus-closed");
        fx.write_status(&run, RunState::Running, false);

        let bus = crate::background::watch::CompletionBus::new();
        let deps = WaitDeps {
            poll_interval: Duration::from_millis(MIN_POLL_INTERVAL_MS),
            completion_bus: Some(bus.clone()),
            ..fx.deps(true)
        };
        // Every sender gone ⇒ the receiver this wait takes will report `Closed` on its first recv.
        drop(bus);

        // The run is still active when the wait starts, so the wait genuinely enters the loop with
        // a dead subscription — the condition this test exists for — and only the POLL can release
        // it.
        let async_root = fx.async_root.clone();
        let results_dir = fx.results_dir.clone();
        let settle_run = run.clone();
        let settler = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Fixture::settle(&async_root, &results_dir, &settle_run);
        });

        let started = Instant::now();
        let text = wait_for_subagents(&WaitParams::default(), &CancelToken::new(), &deps)
            .await
            .text;
        settler.await.expect("settler task");
        assert!(text.contains("1 of 1 run(s) finished"), "{text}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a closed bus must degrade to the poll, not wedge the wait"
        );
    }

    #[test]
    fn active_states_are_exactly_queued_and_running() {
        assert!(is_active(RunState::Queued));
        assert!(is_active(RunState::Running));
        assert!(!is_active(RunState::Paused));
        assert!(!is_active(RunState::Complete));
        assert!(!is_active(RunState::Failed));
        // G77: a stopped run is terminal, so it is never "active" — `wait` must not block on one.
        assert!(!is_active(RunState::Stopped));
    }

    // =======================================================================================
    // ASYNC_NOTIFY_BUG_REPORT F2 — the resolution both renders are built from
    // =======================================================================================

    /// RC2, directly. The poll routinely wins the race against the bus, and the loop consumed at
    /// most ONE event per iteration — so an event still sitting in this wait's receiver at loop
    /// exit was dropped on the floor and the response carried a COUNT where the child's answer
    /// belonged. That is the defect that sent an orchestrator to `bash`/`cat` for a value it had
    /// already been handed.
    #[tokio::test]
    async fn the_exit_drain_recovers_an_event_the_poll_raced_past() {
        let fx = Fixture::new();
        let run = RunId::from_token("run-exit-drain");
        let bus = CompletionBus::new();
        let mut wake = bus.subscribe();
        // Buffered and never polled by a wake arm — exactly the state the loop exits in.
        bus.publish(completion(&run, "worker:\nDRAINED-ANSWER"));

        let mut observed = BTreeMap::new();
        let resolved = resolve_results(
            &fx.deps(true),
            Some(&mut wake),
            &[run.as_str().to_string()],
            &mut observed,
            &[],
        )
        .await;

        assert!(
            resolved.appendix.contains("DRAINED-ANSWER"),
            "the buffered event IS this run's answer: {}",
            resolved.appendix
        );
        assert_eq!(resolved.answered, vec![run.clone()]);
        assert!(
            resolved.unanswered.is_empty(),
            "nothing is missing, so the hint must not render"
        );
    }

    /// The drain applies the SAME `initial_ids` filter the wake arm does: a run started by a
    /// concurrent turn must not inject its output into this wait's report.
    #[tokio::test]
    async fn the_exit_drain_ignores_a_concurrent_turns_run() {
        let fx = Fixture::new();
        let mine = RunId::from_token("run-mine");
        let theirs = RunId::from_token("run-theirs");
        let bus = CompletionBus::new();
        let mut wake = bus.subscribe();
        bus.publish(completion(&theirs, "worker:\nNOT-MINE"));

        let mut observed = BTreeMap::new();
        let resolved = resolve_results(
            &fx.deps(true),
            Some(&mut wake),
            &[mine.as_str().to_string()],
            &mut observed,
            &[],
        )
        .await;

        assert!(resolved.appendix.is_empty(), "{}", resolved.appendix);
        assert!(resolved.answered.is_empty());
    }

    /// `or_insert_with`, never `insert`: a re-published completion cannot overwrite the first
    /// observation, so the wake arm's summary survives the drain that runs after it.
    #[tokio::test]
    async fn a_republished_completion_cannot_overwrite_the_first_observation() {
        let fx = Fixture::new();
        let run = RunId::from_token("run-republished");
        let bus = CompletionBus::new();
        let mut wake = bus.subscribe();
        bus.publish(completion(&run, "worker:\nSECOND"));

        let mut observed = BTreeMap::new();
        observed.insert(run.as_str().to_string(), "worker:\nFIRST".to_string());
        let resolved = resolve_results(
            &fx.deps(true),
            Some(&mut wake),
            &[run.as_str().to_string()],
            &mut observed,
            &[],
        )
        .await;

        assert!(resolved.appendix.contains("FIRST"), "{}", resolved.appendix);
        assert!(
            !resolved.appendix.contains("SECOND"),
            "{}",
            resolved.appendix
        );
    }

    /// RC3, directly. A wait that resolved through the POLL never saw a bus event at all — but the
    /// run's payload is still on disk (consumption requires a DELIVERED injection, and the pump
    /// cannot resolve before the session goes idle), so the value is recoverable.
    ///
    /// And it must NOT be deleted: consumption is the watcher's sole authority, and a `wait` that
    /// consumed a payload would race the very delivery about to notify the orchestrator about it.
    #[tokio::test]
    async fn a_terminal_run_with_no_bus_event_is_replayed_from_its_on_disk_payload() {
        use crate::background::watch::tests::{child_result, publish_result, result_with_children};

        let fx = Fixture::new();
        let run = RunId::from_token("run-disk-replay");
        let result = result_with_children(
            "run-disk-replay",
            RunState::Complete,
            true,
            None,
            vec![child_result("worker", Some("REPLAYED-ANSWER"), 0)],
        );
        publish_result(&fx.results_dir, &result).await;

        let status = terminal_status(&run);
        let mut observed = BTreeMap::new();
        let resolved = resolve_results(
            &fx.deps(true),
            None, // no bus at all — the pure-poll shape
            &[run.as_str().to_string()],
            &mut observed,
            std::slice::from_ref(&status),
        )
        .await;

        assert!(
            resolved.appendix.contains("REPLAYED-ANSWER"),
            "the still-on-disk payload IS the answer: {}",
            resolved.appendix
        );
        assert_eq!(resolved.answered, vec![run.clone()]);
        assert!(resolved.unanswered.is_empty());

        let payload = crate::background::result_index::owned_payload_path(
            &fx.results_dir,
            &crate::identity::SessionId::parse("test-session").expect("non-empty"),
            &run,
        );
        assert!(
            payload.exists(),
            "the replay must never delete the payload it read — consumption is the watcher's"
        );
    }

    /// The records-only fallback names steps and artifacts, NOT the child's answer. Such a run
    /// stays `unanswered`, which is what keeps the hint honest AND what stops F3 from suppressing a
    /// standalone notification that may still carry the value.
    #[tokio::test]
    async fn a_terminal_run_whose_payload_is_gone_is_reported_but_left_unanswered() {
        let fx = Fixture::new();
        let run = RunId::from_token("run-no-payload");
        let status = terminal_status(&run);

        let mut observed = BTreeMap::new();
        let resolved = resolve_results(
            &fx.deps(true),
            None,
            &[run.as_str().to_string()],
            &mut observed,
            std::slice::from_ref(&status),
        )
        .await;

        assert!(resolved.answered.is_empty(), "a pointer is not an answer");
        assert_eq!(resolved.unanswered, vec!["run-no-payload".to_string()]);
        assert!(
            resolved.appendix.contains("Artifacts:"),
            "the fallback still reports what it knows: {}",
            resolved.appendix
        );
    }

    /// F2.3. The "inspect status if the notification is not visible yet" sentence is a POINTER to a
    /// value that is elsewhere. When the response already carries the value it is simply false.
    ///
    /// Ordering note: the settler publishes only AFTER the wait is provably in its loop (a
    /// `broadcast::Receiver` never sees pre-`subscribe()` values), and publishes a SECOND time after
    /// settling so the exit drain covers the case where the poll won the race. Either path puts the
    /// value in the response, which is precisely F2's claim.
    #[tokio::test]
    async fn the_inspect_status_hint_is_dropped_once_the_response_carries_the_value() {
        let fx = Fixture::new();
        let run = RunId::from_token("run-hint-answered");
        fx.write_status(&run, RunState::Running, false);

        let bus = CompletionBus::new();
        let deps = WaitDeps {
            poll_interval: Duration::from_millis(MIN_POLL_INTERVAL_MS),
            completion_bus: Some(bus.clone()),
            ..fx.deps(true)
        };

        let async_root = fx.async_root.clone();
        let results_dir = fx.results_dir.clone();
        let settle_run = run.clone();
        let settler = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            bus.publish(completion(&settle_run, "worker:\nINLINE-ANSWER"));
            tokio::time::sleep(Duration::from_millis(50)).await;
            Fixture::settle(&async_root, &results_dir, &settle_run);
            tokio::time::sleep(Duration::from_millis(20)).await;
            bus.publish(completion(&settle_run, "worker:\nINLINE-ANSWER"));
        });

        let text = tokio::time::timeout(
            Duration::from_secs(15),
            wait_for_subagents(&WaitParams::default(), &CancelToken::new(), &deps),
        )
        .await
        .expect("the wait resolves once the run settles")
        .text;
        settler.await.expect("settler task");

        assert!(
            text.contains("INLINE-ANSWER"),
            "the value must be in the response: {text}"
        );
        assert!(text.contains("Results:"), "{text}");
        assert!(
            !text.contains("inspect status"),
            "the appendix IS the notification; the hint would mislead: {text}"
        );
    }

    /// The control, and it is not decoration: with NO value recovered the hint must still render,
    /// so the assertion above cannot pass merely because the sentence was deleted outright.
    ///
    /// (`Fixture::settle` writes only `legacy_result_root`, which `RunPaths::resolve_result` does
    /// not probe, so the replay legitimately falls back to records-only here.)
    #[tokio::test]
    async fn the_inspect_status_hint_still_renders_when_no_value_was_recovered() {
        let fx = Fixture::new();
        let run = RunId::from_token("run-hint-unanswered");
        fx.write_status(&run, RunState::Running, false);

        let async_root = fx.async_root.clone();
        let results_dir = fx.results_dir.clone();
        let settle_run = run.clone();
        let settler = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Fixture::settle(&async_root, &results_dir, &settle_run);
        });

        let text = tokio::time::timeout(
            Duration::from_secs(15),
            wait_for_subagents(&WaitParams::default(), &CancelToken::new(), &fx.deps(true)),
        )
        .await
        .expect("the wait resolves")
        .text;
        settler.await.expect("settler task");

        assert!(text.contains("1 of 1 run(s) finished"), "{text}");
        assert!(
            text.contains("inspect status"),
            "with no value in the response the pointer is the only honest answer: {text}"
        );
    }

    // =======================================================================================
    // ASYNC_NOTIFY_BUG_REPORT F3.4 — the claim/answer lifecycle
    // =======================================================================================

    /// F3.4 end to end through the real wait. The response carries the run's value, so after the
    /// wait returns the ledger holds exactly ONE redeemable suppression authority for it — which is
    /// precisely what [`crate::background::watch::InlineAnsweredSink`] redeems to keep the duplicate
    /// standalone notification out of the transcript. And the claim's `Drop` really ran, so no
    /// delivery is left parked.
    #[tokio::test]
    async fn a_wait_that_answers_a_run_inline_leaves_one_redeemable_suppression() {
        let fx = Fixture::new();
        let run = RunId::from_token("run-inline-answered");
        fx.write_status(&run, RunState::Running, false);

        let bus = CompletionBus::new();
        let ledger = InlineAnswerLedger::default();
        let deps = WaitDeps {
            poll_interval: Duration::from_millis(MIN_POLL_INTERVAL_MS),
            completion_bus: Some(bus.clone()),
            inline_answers: Some(ledger.clone()),
            ..fx.deps(true)
        };

        let async_root = fx.async_root.clone();
        let results_dir = fx.results_dir.clone();
        let settle_run = run.clone();
        let settler = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            bus.publish(completion(&settle_run, "worker:\nLEDGER-ANSWER"));
            tokio::time::sleep(Duration::from_millis(50)).await;
            Fixture::settle(&async_root, &results_dir, &settle_run);
            tokio::time::sleep(Duration::from_millis(20)).await;
            bus.publish(completion(&settle_run, "worker:\nLEDGER-ANSWER"));
        });

        let text = tokio::time::timeout(
            Duration::from_secs(15),
            wait_for_subagents(&WaitParams::default(), &CancelToken::new(), &deps),
        )
        .await
        .expect("the wait resolves")
        .text;
        settler.await.expect("settler task");

        assert!(text.contains("LEDGER-ANSWER"), "{text}");
        assert!(
            !ledger.is_claimed(&run),
            "the guard's Drop must run on the success path, or a delivery stays parked"
        );
        assert!(
            ledger.take_answered(&run),
            "the value this response carried is redeemable exactly once"
        );
        assert!(!ledger.take_answered(&run), "…and only once");
    }

    /// `Drop` runs on the ERROR paths too. A wait that timed out answered nothing, so it must leave
    /// neither a claim (which would park a delivery for up to `INLINE_CLAIM_MAX_WAIT`) nor an
    /// authority (which would suppress a notification whose value nobody has ever seen).
    #[tokio::test]
    async fn a_timed_out_wait_releases_its_claim_without_authorising_a_suppression() {
        let fx = Fixture::new();
        let run = RunId::from_token("run-timeout-claim");
        fx.write_status(&run, RunState::Running, false);

        let ledger = InlineAnswerLedger::default();
        let deps = WaitDeps {
            inline_answers: Some(ledger.clone()),
            ..fx.deps(true)
        };
        let params = WaitParams {
            timeout_ms: Some(300),
            ..WaitParams::default()
        };

        let outcome = wait_for_subagents(&params, &CancelToken::new(), &deps).await;
        assert!(
            !outcome.is_error(),
            "a timeout is not an error result (§0.1)"
        );
        assert!(matches!(outcome.verdict, WaitVerdict::WindowElapsed { .. }));
        let err = outcome.text;
        assert!(err.contains("timed out"), "{err}");
        assert!(
            !ledger.is_claimed(&run),
            "Drop must release the claim on the timeout path"
        );
        assert!(
            !ledger.take_answered(&run),
            "a timed-out wait answered nothing"
        );
    }

    /// The same guarantee on the ABORT path — the host cancelling the turn.
    #[tokio::test]
    async fn an_aborted_wait_releases_its_claim_without_authorising_a_suppression() {
        let fx = Fixture::new();
        let run = RunId::from_token("run-abort-claim");
        fx.write_status(&run, RunState::Running, false);

        let ledger = InlineAnswerLedger::default();
        let deps = WaitDeps {
            inline_answers: Some(ledger.clone()),
            ..fx.deps(true)
        };
        let cancel = CancelToken::new();
        let canceller = {
            let cancel = cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                cancel.cancel();
            })
        };

        let outcome = wait_for_subagents(&WaitParams::default(), &cancel, &deps).await;
        canceller.await.expect("canceller task");
        assert_eq!(
            outcome.verdict,
            WaitVerdict::Aborted,
            "an aborted wait reports the abort"
        );
        assert!(outcome.is_error());
        let err = outcome.text;
        assert!(err.contains("aborted"), "{err}");
        assert!(
            !ledger.is_claimed(&run),
            "Drop must release the claim on the abort path"
        );
        assert!(!ledger.take_answered(&run));
    }

    /// The documented degradation, pinned explicitly so "the ledger is optional" cannot quietly
    /// become "the ledger is required": [`WaitDeps::for_cwd`] wires none, and a wait with none
    /// behaves exactly as every pre-existing test in this module already asserts.
    #[tokio::test]
    async fn a_wait_with_no_ledger_is_unchanged() {
        let fx = Fixture::new();
        assert!(fx.deps(true).inline_answers.is_none());

        let sandbox = tempfile::tempdir().expect("tempdir");
        let roots = crate::paths::Roots::sandboxed(sandbox.path());
        let production = WaitDeps::for_cwd(sandbox.path(), true, None, &roots);
        assert!(
            production.inline_answers.is_none(),
            "for_cwd must keep the no-ledger degradation as the default"
        );

        let run = RunId::from_token("run-no-ledger");
        fx.write_status(&run, RunState::Running, false);
        let async_root = fx.async_root.clone();
        let results_dir = fx.results_dir.clone();
        let settle_run = run.clone();
        let settler = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Fixture::settle(&async_root, &results_dir, &settle_run);
        });

        let text = tokio::time::timeout(
            Duration::from_secs(15),
            wait_for_subagents(&WaitParams::default(), &CancelToken::new(), &fx.deps(true)),
        )
        .await
        .expect("the wait resolves")
        .text;
        settler.await.expect("settler task");
        assert!(text.contains("1 of 1 run(s) finished"), "{text}");
    }

    // =============================================================================================
    // SCOPE_11 — `{ id, nonBlocking: true }`
    // =============================================================================================

    /// The three stub seams a real [`crate::background::wait_subscriptions::WaitSubscriptionManager`]
    /// needs. Used rather than a fake arming closure so this test exercises the REAL `arm` — the
    /// on-disk record it asserts is the one production writes, not one the test wrote for itself.
    mod subscription_stubs {
        use crate::background::wait_completions::WaitCompletion;
        use crate::background::wait_subscriptions::{
            ForegroundSubscriptionProbe, ForegroundTargetState, SubscriptionNotifier,
            SubscriptionOutcome, SubscriptionSessions, WaitSubscriptionRecord,
        };

        pub(super) struct Sessions(pub(super) Option<&'static str>);

        impl SubscriptionSessions for Sessions {
            fn current_session_id(&self) -> Option<crate::identity::SessionId> {
                crate::identity::SessionId::parse_opt(self.0)
            }
        }

        pub(super) struct SilentNotifier;

        #[async_trait::async_trait]
        impl SubscriptionNotifier for SilentNotifier {
            async fn notify(
                &self,
                _record: &WaitSubscriptionRecord,
                _outcome: SubscriptionOutcome,
                _detail: &str,
                _completion: Option<&WaitCompletion>,
            ) -> bool {
                true
            }
        }

        pub(super) struct NoForeground;

        impl ForegroundSubscriptionProbe for NoForeground {
            fn probe(&self, _run_id: &crate::background::RunId) -> Option<ForegroundTargetState> {
                None
            }
        }
    }

    fn subscription_manager(
        fx: &Fixture,
        subs_dir: &std::path::Path,
        session_id: Option<&'static str>,
    ) -> std::sync::Arc<crate::background::wait_subscriptions::WaitSubscriptionManager> {
        crate::background::wait_subscriptions::WaitSubscriptionManager::new(
            crate::background::wait_subscriptions::WaitSubscriptionDeps {
                subscriptions_dir: subs_dir.to_path_buf(),
                async_root: fx.async_root.clone(),
                results_dir: fx.results_dir.clone(),
                sessions: std::sync::Arc::new(subscription_stubs::Sessions(session_id)),
                notifier: std::sync::Arc::new(subscription_stubs::SilentNotifier),
                foreground: std::sync::Arc::new(subscription_stubs::NoForeground),
                wait_completions: std::sync::Arc::new(
                    crate::background::wait_completions::WaitCompletionStore::default(),
                ),
            },
        )
    }

    // ---------------------------------------------------------------------------------------
    // VL-S11b — remembered DETACHED FOREGROUND runs
    // ---------------------------------------------------------------------------------------

    /// A settable stand-in for the executor's `foreground_runs` map.
    #[derive(Clone, Default)]
    struct FakeForegroundRuns(std::sync::Arc<std::sync::Mutex<Vec<DetachedForegroundRun>>>);

    impl FakeForegroundRuns {
        fn set(&self, runs: Vec<DetachedForegroundRun>) {
            *self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = runs;
        }

        fn hook(&self) -> DetachedForegroundHook {
            DetachedForegroundHook::new(std::sync::Arc::new(self.clone()))
        }
    }

    impl DetachedForegroundRunsSource for FakeForegroundRuns {
        fn snapshot(&self) -> Vec<DetachedForegroundRun> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    fn foreground_run(run_id: &str, session_id: &str, status: &str) -> DetachedForegroundRun {
        DetachedForegroundRun {
            run_id: run_id.to_string(),
            session_id: session_id.to_string(),
            children: vec![DetachedForegroundChild {
                index: 0,
                agent: "scout".to_string(),
                status: status.to_string(),
                activity_state: None,
                current_tool: None,
                transcript_path: None,
            }],
        }
    }

    /// pi `activeDetachedForegroundRuns` (`subagent-wait.ts:224-236`): all four filters.
    ///
    /// **Gutting mutation this fails on:** drop the `all` gate and a bare `bg_wait()` starts
    /// blocking on every detached foreground run in the session — the `no_id_without_all` assert
    /// fires. Drop the session filter and another session's run is waited on — the
    /// `other_session` assert fires.
    #[test]
    fn the_detached_foreground_candidate_set_honours_every_upstream_filter() {
        let fx = Fixture::new();
        let source = FakeForegroundRuns::default();
        source.set(vec![
            foreground_run("fg-alpha", "sess-a", "detached"),
            foreground_run("fg-beta", "sess-a", "completed"),
            foreground_run("fg-other", "sess-b", "detached"),
        ]);
        let mut deps = fx.deps(true);
        deps.session_id = Some("sess-a".to_string());
        deps.detached_foreground = Some(source.hook());

        // No id and no `all`: upstream selects nothing (`:231`).
        let no_id_without_all = active_detached_foreground_runs(&WaitParams::default(), &deps);
        assert!(
            no_id_without_all.is_empty(),
            "a bare wait must not block on a detached foreground run"
        );

        // `all: true`: only this session's runs, and only the ones with a detached child.
        let all = active_detached_foreground_runs(
            &WaitParams {
                all: Some(true),
                ..WaitParams::default()
            },
            &deps,
        );
        assert_eq!(all.len(), 1, "{all:?}");
        assert_eq!(all[0].run_id, "fg-alpha");
        let other_session = all.iter().any(|run| run.session_id == "sess-b");
        assert!(
            !other_session,
            "another session's run must never be selected"
        );

        // An id narrows by exact-or-prefix (`:230`).
        let by_prefix = active_detached_foreground_runs(
            &WaitParams {
                id: Some("fg-al".to_string()),
                ..WaitParams::default()
            },
            &deps,
        );
        assert_eq!(by_prefix.len(), 1);
        assert_eq!(by_prefix[0].run_id, "fg-alpha");

        // A settled foreground run is not a candidate at all — its child is not `"detached"`.
        let settled = active_detached_foreground_runs(
            &WaitParams {
                id: Some("fg-beta".to_string()),
                ..WaitParams::default()
            },
            &deps,
        );
        assert!(settled.is_empty(), "{settled:?}");

        // No source wired (every pre-`with_detached_foreground` construction) selects nothing.
        deps.detached_foreground = None;
        assert!(
            active_detached_foreground_runs(
                &WaitParams {
                    all: Some(true),
                    ..WaitParams::default()
                },
                &deps
            )
            .is_empty()
        );
    }

    /// `bg_wait({ id })` on a DETACHED foreground run BLOCKS, and resolves when the continuation
    /// task's reconcile flips the child off `"detached"`.
    ///
    /// This is the whole point of the candidate-set extension, and it is what the deleted
    /// `[CYRUP-DELTA]` said could not happen: there is no async run on disk here at all.
    ///
    /// **Gutting mutation this fails on:** remove the `detached_foreground` branch from
    /// `wait_for_subagents`. The async listing is empty, so the wait returns IMMEDIATELY with
    /// `No active run matched "fg-live". Nothing to wait for.` — the elapsed-time assert and both
    /// text asserts fire.
    #[tokio::test]
    async fn a_wait_on_a_detached_foreground_run_blocks_until_its_child_is_reconciled() {
        let fx = Fixture::new();
        let source = FakeForegroundRuns::default();
        source.set(vec![foreground_run("fg-live", "sess-a", "detached")]);
        let mut deps = fx.deps(true);
        deps.session_id = Some("sess-a".to_string());
        deps.poll_interval = Duration::from_millis(MIN_POLL_INTERVAL_MS);
        deps.detached_foreground = Some(source.hook());

        let settler = {
            let source = source.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(600)).await;
                source.set(vec![foreground_run("fg-live", "sess-a", "completed")]);
            })
        };

        let params = WaitParams {
            id: Some("fg-live".to_string()),
            timeout_ms: Some(15_000),
            ..WaitParams::default()
        };
        let started = Instant::now();
        let outcome = tokio::time::timeout(
            Duration::from_secs(10),
            wait_for_subagents(&params, &CancelToken::new(), &deps),
        )
        .await
        .expect("the foreground wait resolves");
        settler.await.expect("settler task");

        assert!(
            started.elapsed() >= Duration::from_millis(500),
            "the wait must have BLOCKED on the detached child, took {:?}",
            started.elapsed()
        );
        assert!(!outcome.is_error(), "{}", outcome.text);
        assert!(
            outcome
                .text
                .contains("for remembered detached foreground run \"fg-live\"; done."),
            "{}",
            outcome.text
        );
        assert!(
            outcome.text.contains("Outcome: 1 completed"),
            "{}",
            outcome.text
        );
    }

    /// A session switch under a foreground wait stops it — pi `:568-570` — as an ERROR, and it
    /// names the run rather than reporting a generic listing failure.
    ///
    /// **Gutting mutation this fails on:** drop the session re-check from the loop. The wait then
    /// blocks until its window elapses and the verdict is `WindowElapsed`, not
    /// `ForegroundUnreconciled`.
    #[tokio::test]
    async fn a_session_change_stops_a_detached_foreground_wait_with_its_own_refusal() {
        let fx = Fixture::new();
        let source = FakeForegroundRuns::default();
        source.set(vec![foreground_run("fg-moved", "sess-a", "detached")]);
        let mut deps = fx.deps(true);
        // The wait's own session is NOT the run's: upstream's `:561` comparison fails on the very
        // first lap, which is the same observable as a switch mid-wait.
        deps.session_id = Some("sess-a".to_string());
        deps.detached_foreground = Some(source.hook());
        let selected = active_detached_foreground_runs(
            &WaitParams {
                id: Some("fg-moved".to_string()),
                ..WaitParams::default()
            },
            &deps,
        );
        assert_eq!(selected.len(), 1);

        deps.session_id = Some("sess-b".to_string());
        let outcome = wait_for_detached_foreground_runs(
            &selected,
            &CancelToken::new(),
            &deps,
            Instant::now(),
            Duration::from_millis(MIN_POLL_INTERVAL_MS),
            Duration::from_secs(30),
        )
        .await;

        assert_eq!(outcome.verdict, WaitVerdict::ForegroundUnreconciled);
        assert!(outcome.is_error());
        assert!(
            outcome.text.starts_with(
                "Wait stopped because the active session changed while remembered foreground run \
                 \"fg-moved\" was still detached."
            ),
            "{}",
            outcome.text
        );
    }

    /// SCOPE_11 — `{ id, nonBlocking: true }` over a detached foreground run arms
    /// `WaitTargetKind::Foreground`, which had no production producer before VL-S11b.
    ///
    /// **Gutting mutation this fails on:** arm `WaitTargetKind::Async` for it (which is what the
    /// single arming site did before the candidate set grew). The record's `targetKind` is then
    /// `async`, the manager reconciles it through `run_status` against an async root that has no
    /// such run, and both the sentence and the on-disk assert fire.
    #[tokio::test]
    async fn a_non_blocking_wait_on_a_detached_foreground_run_arms_the_foreground_kind() {
        let fx = Fixture::new();
        let subs = fx.async_root.parent().expect("temp root").join("subs");
        let source = FakeForegroundRuns::default();
        source.set(vec![foreground_run("fg-arm-1", "sess-a", "detached")]);

        let manager = subscription_manager(&fx, &subs, Some("sess-a"));
        let mut deps = fx.deps(true);
        deps.session_id = Some("sess-a".to_string());
        deps.poll_interval = Duration::from_secs(10);
        deps.detached_foreground = Some(source.hook());
        let arming: std::sync::Arc<dyn WaitSubscriptionArming> = manager;
        deps.subscribe = Some(WaitSubscribeHook::new(arming));

        let params = WaitParams {
            id: Some("fg-arm".to_string()),
            non_blocking: Some(true),
            timeout_ms: Some(60 * 60 * 1000),
            ..WaitParams::default()
        };
        let started = Instant::now();
        let outcome = wait_for_subagents(&params, &CancelToken::new(), &deps).await;
        assert!(
            started.elapsed() < deps.poll_interval,
            "a non-blocking wait must not pay a poll interval: {:?}",
            started.elapsed()
        );

        let WaitVerdict::SubscriptionArmed { token } = &outcome.verdict else {
            panic!("expected SubscriptionArmed, got {:?}", outcome.verdict);
        };
        assert!(!outcome.is_error(), "{}", outcome.text);
        // pi `:710` interpolates `selected.kind`, so the word here is `foreground`, not `async`.
        assert!(
            outcome.text.contains(&format!(
                "Armed wait subscription {token} for exact foreground run fg-arm-1."
            )),
            "{}",
            outcome.text
        );
    }

    /// SUBTASK2 — the feature: the wait RETURNS instead of blocking, and it leaves a durable
    /// record behind.
    ///
    /// The run stays `Running` for the whole test and the poll interval is set to ten seconds, so
    /// a blocking wait could not return in under that. Returning fast alone would not be enough —
    /// a stub that simply returned would pass it — which is why the `<token>.json` on disk is
    /// asserted too, and why the token in the verdict must be the file's own name.
    #[tokio::test]
    async fn a_non_blocking_wait_arms_a_subscription_and_returns_immediately() {
        let fx = Fixture::new();
        let subs = fx.async_root.parent().expect("temp root").join("subs");
        let run = RunId::from_token("run-1".to_string());
        fx.write_status_for_session(&run, RunState::Running, false, Some("sess-a"));

        let manager = subscription_manager(&fx, &subs, Some("sess-a"));
        let mut deps = fx.deps(true);
        deps.session_id = Some("sess-a".to_string());
        // Far longer than this test's own budget: a blocking wait sleeps at least this long
        // before it re-lists, so "returned" and "blocked" cannot be confused.
        deps.poll_interval = Duration::from_secs(10);
        let arming: std::sync::Arc<dyn WaitSubscriptionArming> = manager;
        deps.subscribe = Some(WaitSubscribeHook::new(arming));

        let params = WaitParams {
            id: Some("run".to_string()),
            non_blocking: Some(true),
            timeout_ms: Some(60 * 60 * 1000),
            ..WaitParams::default()
        };
        let started = Instant::now();
        let outcome = wait_for_subagents(&params, &CancelToken::new(), &deps).await;
        assert!(
            started.elapsed() < deps.poll_interval,
            "a non-blocking wait must not pay even one poll interval: {:?}",
            started.elapsed()
        );

        let WaitVerdict::SubscriptionArmed { token } = &outcome.verdict else {
            panic!("expected SubscriptionArmed, got {:?}", outcome.verdict);
        };
        assert!(!outcome.is_error(), "pi `:596` is a bare non-error result");
        assert_eq!(
            outcome.text,
            format!(
                "Armed wait subscription {token} for exact async run run-1. Returning \
                 immediately; this session will be woken on completion, failure, attention, \
                 reconciliation failure, or timeout. Inspect armed subscriptions with subagent({{ \
                 action: \"status\" }})."
            )
        );
        // The durable half: a real record, named for the token the verdict carries.
        let path = subs.join(format!("{token}.json"));
        let bytes = std::fs::read(&path).expect("the record is on disk");
        let record = crate::background::wait_subscriptions::parse_record(&bytes)
            .expect("and it parses back");
        assert_eq!(&record.token, token);
        // pi keeps BOTH ids: the resolved exact one it binds, and the prefix the caller typed.
        assert_eq!(record.run_id.as_str(), "run-1");
        assert_eq!(record.requested_id, "run");
        assert_eq!(
            record.target_kind,
            crate::background::wait_subscriptions::WaitTargetKind::Async
        );
    }

    /// pi `:561-566` — both rejections, BEFORE any listing (there is no run on disk here at all,
    /// and neither refusal depends on one).
    #[tokio::test]
    async fn a_non_blocking_wait_requires_an_id_and_refuses_all() {
        let fx = Fixture::new();
        let deps = fx.deps(true);

        let no_id = wait_for_subagents(
            &WaitParams {
                non_blocking: Some(true),
                ..WaitParams::default()
            },
            &CancelToken::new(),
            &deps,
        )
        .await;
        assert_eq!(no_id.verdict, WaitVerdict::SubscriptionRefused);
        assert!(no_id.is_error());
        assert_eq!(
            no_id.text,
            "Non-blocking wait subscriptions require id so the registration can bind one exact \
             run identity."
        );

        let with_all = wait_for_subagents(
            &WaitParams {
                id: Some("run-1".to_string()),
                all: Some(true),
                non_blocking: Some(true),
                ..WaitParams::default()
            },
            &CancelToken::new(),
            &deps,
        )
        .await;
        assert_eq!(with_all.verdict, WaitVerdict::SubscriptionRefused);
        assert_eq!(
            with_all.text,
            "nonBlocking cannot be combined with all; subscribe to one exact run id."
        );
    }

    /// pi `:591-593` / `wait-tool.ts:33`'s `ctx?.hasUI` gate: with no manager the request is
    /// REFUSED, never silently downgraded to a blocking wait. `WaitDeps::for_cwd` leaves
    /// `subscribe: None`, so this is also the default every headless embedder gets.
    #[tokio::test]
    async fn a_non_blocking_wait_with_no_manager_is_refused() {
        let fx = Fixture::new();
        let run = RunId::from_token("run-1".to_string());
        fx.write_status(&run, RunState::Running, false);
        let deps = fx.deps(true);
        assert!(deps.subscribe.is_none());

        let outcome = wait_for_subagents(
            &WaitParams {
                id: Some("run-1".to_string()),
                non_blocking: Some(true),
                ..WaitParams::default()
            },
            &CancelToken::new(),
            &deps,
        )
        .await;
        assert_eq!(outcome.verdict, WaitVerdict::SubscriptionRefused);
        assert!(outcome.is_error());
        // VL-S8: the sentence interpolates `WAIT_TOOL_NAME`, and this literal is the assert that
        // makes the rename observable at THIS site. It is byte-identical to upstream's
        // `subagent-wait.ts:706` @v0.68.0, which also says `bg_wait` — so a revert of the const
        // fails here as well as at the registration.
        assert_eq!(
            outcome.text,
            "Non-blocking wait subscriptions require a long-lived interactive subagent runtime; \
             this runtime can only use blocking bg_wait calls."
        );
    }

    /// The arm's own refusal is reported as an error RESULT, never propagated (pi `:598-600`).
    #[tokio::test]
    async fn a_non_blocking_wait_reports_the_arms_own_refusal() {
        let fx = Fixture::new();
        let subs = fx.async_root.parent().expect("temp root").join("subs");
        let run = RunId::from_token("run-1".to_string());
        fx.write_status(&run, RunState::Running, false);
        // The manager has no session identity, so `arm` throws pi's `:292`.
        let manager = subscription_manager(&fx, &subs, None);
        let mut deps = fx.deps(true);
        let arming: std::sync::Arc<dyn WaitSubscriptionArming> = manager;
        deps.subscribe = Some(WaitSubscribeHook::new(arming));

        let outcome = wait_for_subagents(
            &WaitParams {
                id: Some("run-1".to_string()),
                non_blocking: Some(true),
                ..WaitParams::default()
            },
            &CancelToken::new(),
            &deps,
        )
        .await;
        assert_eq!(outcome.verdict, WaitVerdict::SubscriptionRefused);
        assert_eq!(
            outcome.text,
            crate::background::wait_subscriptions::NO_SESSION_IDENTITY
        );
        assert!(
            !subs.exists() || std::fs::read_dir(&subs).map(Iterator::count).unwrap_or(0) == 0,
            "a refused arm writes nothing"
        );
    }

    /// `nonBlocking` must survive the wire in BOTH directions: the JSON Schema advertises it and
    /// `WaitParams` deserializes it. Editing one without the other is a silent no-op — a host
    /// rejection one way, a serde drop the other.
    #[test]
    fn non_blocking_is_advertised_by_the_schema_and_parsed_by_the_params() {
        let schema = crate::extension::wait_tool::wait_tool_parameters();
        assert!(
            schema["properties"]["nonBlocking"].is_object(),
            "the schema closes with additionalProperties:false, so an unadvertised key is \
             rejected by the host before serde ever sees it: {schema}"
        );
        let parsed: WaitParams =
            serde_json::from_value(serde_json::json!({ "id": "run-1", "nonBlocking": true }))
                .expect("parses");
        assert_eq!(parsed.non_blocking, Some(true));
        // And it round-trips under pi's camelCase spelling.
        let value = serde_json::to_value(&parsed).expect("serializes");
        assert_eq!(value["nonBlocking"], serde_json::json!(true));
        // Omitted stays omitted, never `false` — pi's `...(x ? {x} : {})` discipline.
        let bare = serde_json::to_value(WaitParams::default()).expect("serializes");
        assert!(bare.get("nonBlocking").is_none(), "{bare}");
    }
}
