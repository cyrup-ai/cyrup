//! Anthropic prompt-cache warming — pi `core/cache-warmer.ts` @v1.0.4.
//!
//! An Anthropic prompt-cache entry expires (5 minutes on the short tier, 1 hour on the long one).
//! If a turn outlasts that lifetime, the next real request pays a full cache WRITE of the whole
//! prompt instead of a cheap cache READ. Upstream re-sends the identical request with
//! `maxTokens: 1` just before expiry, which refreshes the entry for the price of one cache read
//! plus one output token — but only when the arithmetic says it saves money.
//!
//! The state machine, its economics and all five formatters live here, beside each other, exactly
//! as `cache-warmer.ts` keeps them (`:404-453`). The split against the front-end is the one cyrup
//! already uses for `cyrup_provider::cache_stats::CacheMiss`: the facts and their formatting live
//! beside the algorithm, and `cyrup-tui` calls the formatters from its notice and its `/session`
//! body.
//!
//! ## Mechanism deltas against upstream, and why each is forced
//!
//! * **Time is [`tokio::time::Instant`], not `Date.now()`.** Every deadline is an `Instant` and
//!   every timer a `tokio::time::sleep_until`, so the whole machine runs on the virtual clock
//!   `#[tokio::test(start_paused = true)]` installs. A warmer measured against
//!   `std::time::SystemTime` could only be tested by sleeping for real minutes.
//! * **`status` is `async`.** pi's getter is synchronous because its `SessionManager` is; cyrup's
//!   lives behind a `tokio::sync::Mutex`, and `evaluate` has to read the branch's last prompt size
//!   through it. Every cyrup caller of the status (`/session`) is already `async`.
//! * **Run identity is `Arc` pointer identity.** pi's reentrancy guard is `this.run !== run`
//!   (`validateRun`, `:364`); here the run is an [`Arc<ActiveRun>`] and the guard is
//!   `Arc::ptr_eq`, which is the same check with the same meaning.
//! * **One token replaces the `AbortController` AND the `setTimeout`.** `clear_run` cancels a
//!   [`CancelToken`] that both ends the armed `sleep_until` and aborts an in-flight warm request —
//!   pi's `clearTimeout(timer)` plus `controller.abort()` as a single signal. The run token is a
//!   child of the session's, so session disposal stops warming without a separate hook; pi's
//!   `timer.unref()` has no analogue because a detached tokio task cannot hold the runtime open
//!   the way an un-`unref`'d Node timer holds the event loop.
//! * **The warm request goes to the provider captured at `start`, not to the session's
//!   `StreamFn`.** Upstream calls `modelRuntime.streamSimple` rather than the agent's `streamFn`
//!   for the same reason: the agent's stream function is what *starts* a warming run, so routing a
//!   warm back through it would restart the run it belongs to on every refresh.

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::time::Instant;

use cyrup_config::CacheWarmingMode;
use cyrup_core::{CancelToken, EventStream, ModelId, ProviderId, StopReason, Usage};
use cyrup_provider::{
    Context, Model, ModelCost, Provider, StreamEvent, StreamOptions, collect_message, compute_cost,
    known_api, prompt_cache_ttl_ms,
};
use cyrup_session::entry::{Entry, KnownEntry};

/// Streaming warming never continues past this long after the real request that started it — pi
/// `MAX_WARMING_AGE_MS` (`cache-warmer.ts:16`).
const MAX_WARMING_AGE_MS: u64 = 60 * 60_000;
/// Idle warming uses a shorter horizon because continuation estimates become less reliable with
/// age — pi `MAX_IDLE_WARMING_AGE_MS` (`:18`).
const MAX_IDLE_WARMING_AGE_MS: u64 = 30 * 60_000;
/// A refresh is sent only when it is expected to save at least this many dollars — pi
/// `CACHE_WARMING_MINIMUM_EXPECTED_SAVINGS` (`:20`).
const CACHE_WARMING_MINIMUM_EXPECTED_SAVINGS: f64 = 0.05;
/// Chance that a real request arrives before the cache entry expires while the agent sits idle —
/// pi `IDLE_CONTINUATION_PROBABILITY` (`:26`), measured upstream and deliberately a constant.
const IDLE_CONTINUATION_PROBABILITY: f64 = 0.15;

/// Refresh at 90% of the TTL while preserving at least ten seconds of margin — pi
/// `getCacheWarmingDelayMs` (`cache-warmer.ts:29-32`):
///
/// ```ts
/// if (ttlMs <= 10_000) return undefined;
/// return Math.max(1, Math.floor(Math.min(ttlMs * 0.9, ttlMs - 10_000)));
/// ```
///
/// `None` means "do not schedule": a lifetime of ten seconds or less leaves no useful margin. The
/// `max(1, …)` floor is upstream's and cannot bite for any `ttl > 10_000` — it is kept so the
/// arithmetic is the same arithmetic.
#[must_use]
pub fn cache_warming_delay_ms(ttl_ms: u64) -> Option<u64> {
    if ttl_ms <= 10_000 {
        return None;
    }
    let ninety = (ttl_ms as f64 * 0.9).floor();
    let margin = (ttl_ms - 10_000) as f64;
    Some((ninety.min(margin) as u64).max(1))
}

/// Whether replaying the request with a one-token output cap leaves its cache entry untouched — pi
/// `isReplayable` (`cache-warmer.ts:55-58`).
///
/// Anthropic's budget-based thinking (Claude models without adaptive thinking) derives
/// `budget_tokens` from `max_tokens`, which Anthropic keys the message cache on; a `maxTokens: 1`
/// replay would get a different budget and could still think for thousands of tokens. So a
/// reasoning request on `anthropic-messages` is replayable only when the model forces adaptive
/// thinking.
///
/// pi's first conjunct is `!options?.reasoning`, and that falsiness is exact rather than loose:
/// its agent maps the level to `undefined` when thinking is off
/// (`packages/agent/src/agent.ts:471` @v1.0.4, `this._state.thinkingLevel === "off" ? undefined :
/// …`), so "no reasoning" and "reasoning off" are the same state upstream — which is what
/// [`cyrup_core::ModelThinkingLevel::Off`] is here.
#[must_use]
pub fn is_replayable(model: &Model, options: &StreamOptions) -> bool {
    if options.reasoning == cyrup_core::ModelThinkingLevel::Off
        || model.api.as_str() != known_api::ANTHROPIC_MESSAGES
    {
        return true;
    }
    model
        .compat
        .as_ref()
        .and_then(|c| c.force_adaptive_thinking)
        == Some(true)
}

/// Prompt size of the most recent real request on the branch, as reported by the provider — pi
/// `lastPromptTokens` (`cache-warmer.ts:61-70`).
///
/// Walks the branch BACKWARDS and answers the first assistant message's
/// `input + cacheRead + cacheWrite`; `0` when the branch holds none. It matches on assistant
/// messages rather than taking the last entry precisely because a warm's own `usage` entry
/// advances the branch leaf (`session-manager.ts:1191-1196`), so the last entry is routinely not a
/// message at all.
#[must_use]
pub fn last_prompt_tokens(entries: &[&Entry]) -> u64 {
    for entry in entries.iter().rev() {
        let Entry::Known(KnownEntry::Message { message, .. }) = entry else {
            continue;
        };
        let cyrup_session::AgentMessage::Core(cyrup_core::Message::Assistant(a)) = message else {
            continue;
        };
        return a
            .usage
            .input
            .saturating_add(a.usage.cache_read)
            .saturating_add(a.usage.cache_write);
    }
    0
}

/// pi's local `price(model, tokens)` (`cache-warmer.ts:72-86`): the dollar cost of a usage vector
/// with only the named components set.
///
/// Built on [`cyrup_provider::compute_cost`], which IS `calculateCost` — same tier ladder keyed on
/// `input + cacheRead + cacheWrite`, same Anthropic 1h rule. `cache_write_1h` is left unset, so a
/// write prices at the short `cacheWrite` rate: upstream's `price` has no 1h field at all.
fn price(cost: &ModelCost, input: u64, output: u64, cache_read: u64, cache_write: u64) -> f64 {
    let usage = Usage {
        input,
        output,
        cache_read,
        cache_write,
        ..Default::default()
    };
    compute_cost(cost, &usage).total
}

/// pi `CacheWarmingAction` (`cache-warmer.ts:88`) — the only two values there are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheWarmingAction {
    Warm,
    Stop,
}

/// pi `CacheWarmingDecision.phase` (`cache-warmer.ts:93`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheWarmingPhase {
    /// The agent run that sent the request is still active.
    Streaming,
    Idle,
}

/// Inputs and outcome of one warm-or-stop decision, as shown by `/session` — pi
/// `CacheWarmingDecision` (`cache-warmer.ts:91-106`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CacheWarmingDecision {
    pub phase: CacheWarmingPhase,
    /// Price of this refresh: a cache read of the prompt plus one output token.
    pub warm_cost: f64,
    /// Extra price of the next real request if the cache entry is lost.
    pub miss_cost: f64,
    /// Estimated chance that a real request arrives before the entry expires.
    pub continuation_probability: f64,
    /// `continuation_probability * miss_cost - warm_cost`.
    pub expected_savings: f64,
    /// False when the prompt size or the model's prices are unknown.
    pub economics_available: bool,
    /// pi's decision: `Warm` when `expected_savings` is at least $0.05.
    pub action: CacheWarmingAction,
}

/// Fired before each refresh with pi's decision filled in — pi `CacheWarmingDecisionEvent`
/// (`cache-warmer.ts:112-115`), which is a `Pick` of exactly these four fields.
///
/// `phase`, `expected_savings` and `economics_available` are withheld on purpose: upstream's
/// comment is *"Everything else an extension might want (model, idle state, context size) is on
/// the context."* Do not widen it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CacheWarmingDecisionEvent {
    pub warm_cost: f64,
    pub miss_cost: f64,
    pub continuation_probability: f64,
    pub action: CacheWarmingAction,
}

/// The extension seam: lets a handler override `event.action` — pi's `decide` constructor argument
/// (`cache-warmer.ts:169`, supplied from `sdk.ts:316-321` as
/// `extensionRunnerRef.current?.emitCacheWarmingDecision(event) ?? event.action`).
///
/// It answers with the FINAL action, so a decider that has no opinion returns `event.action`
/// unchanged — which is what [`PiDecision`] does, and which is the default every session gets
/// until the `cache_warming_decision` extension hook (EXT-085) is wired in. The warmer treats a
/// returned action that differs from pi's as an extension override: it stops with
/// `"stopped by extension"` or tags the persisted usage entry `"extension override"`.
///
/// Failures are the implementor's to swallow or surface; the warmer additionally wraps the whole
/// call so a panicking or erroring decider cannot disturb a run (pi's second swallow,
/// `cache-warmer.ts:315-317`).
pub trait CacheWarmingDecider: Send + Sync {
    /// The final action for this refresh.
    fn decide<'a>(
        &'a self,
        event: &'a CacheWarmingDecisionEvent,
    ) -> BoxFuture<'a, CacheWarmingAction>;
}

/// The default decider: pi's own action, unchanged — `decide = async (event) => event.action`
/// (`cache-warmer.ts:177`).
pub struct PiDecision;

impl CacheWarmingDecider for PiDecision {
    fn decide<'a>(
        &'a self,
        event: &'a CacheWarmingDecisionEvent,
    ) -> BoxFuture<'a, CacheWarmingAction> {
        Box::pin(std::future::ready(event.action))
    }
}

/// pi `CacheWarmingStatus.state` (`cache-warmer.ts:124`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheWarmingState {
    Inactive,
    /// A refresh timer is armed.
    Scheduled,
    /// A warm request is in flight.
    Refreshing,
}

/// pi `CacheWarmingStatus` (`cache-warmer.ts:122-132`).
#[derive(Clone, Debug, PartialEq)]
pub struct CacheWarmingStatus {
    pub state: CacheWarmingState,
    /// Why nothing is scheduled.
    pub reason: Option<String>,
    /// How long until the next decision. pi carries `nextWarmAt` as a wall-clock millisecond and
    /// its formatter consumes `nextWarmAt - now`; the arithmetic here is in
    /// [`tokio::time::Instant`], which has no wall-clock projection, so the status carries the
    /// remaining duration the formatter actually reads.
    pub next_warm_in: Option<Duration>,
    /// The pending decision, or the decision that stopped warming.
    pub decision: Option<CacheWarmingDecision>,
    /// True when an extension changed `decision.action`.
    pub extension_override: bool,
}

/// How a warm request reaches a provider — the seam pi fills with
/// `Pick<ModelRuntime, "streamSimple">` (`cache-warmer.ts:165`).
///
/// A trait rather than a bare `Arc<dyn Provider>` so a test can assert on the exact
/// [`StreamOptions`] a warm carried without standing up a provider catalog;
/// [`ProviderWarmStream`] is the production implementation.
pub trait CacheWarmStream: Send + Sync {
    fn stream_warm(
        &self,
        model: &Model,
        ctx: &Context,
        opts: &StreamOptions,
    ) -> EventStream<StreamEvent>;
}

/// [`CacheWarmStream`] over the provider that was installed when the run started.
///
/// Capturing the provider is faithful rather than a shortcut: a provider swap only happens on a
/// cross-provider `/model` select, which changes the model too — and that already makes
/// `is_current` false, so the run stops before the captured provider could be the stale one.
pub struct ProviderWarmStream(pub Arc<dyn Provider>);

impl CacheWarmStream for ProviderWarmStream {
    fn stream_warm(
        &self,
        model: &Model,
        ctx: &Context,
        opts: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        self.0.stream(model, ctx, opts)
    }
}

/// What the warmer needs from the session — pi's
/// `Pick<SessionManager, "appendUsage" | "getBranch">` (`cache-warmer.ts:166`) plus its
/// `onWarmed` fan-out (`:171`), which in cyrup is the `entry_appended` event the TUI's live
/// transcript arm already renders.
///
/// Narrow so the state machine is unit-testable without a real session, which is how upstream's
/// own tests drive it.
pub trait CacheWarmingHost: Send + Sync {
    /// `lastPromptTokens(sessionManager.getBranch())`.
    fn last_prompt_tokens(&self) -> BoxFuture<'_, u64>;

    /// `sessionManager.appendUsage("cache_warm", provider, model, usage, note)` followed by
    /// `onWarmed(entry)`.
    ///
    /// Both halves are one method because cyrup's `onWarmed` is not a caller-supplied callback but
    /// a fixed fan-out of the entry that was just written — a live warm that appended without
    /// emitting would show nothing until the session was resumed.
    fn record_warm(
        &self,
        provider: ProviderId,
        model: ModelId,
        usage: Usage,
        note: Option<String>,
    ) -> BoxFuture<'_, ()>;
}

/// The request whose prompt cache entry should be kept warm, exactly as it was sent — pi
/// `CacheWarmRequest` (`cache-warmer.ts:135-139`).
pub struct CacheWarmRequest {
    /// Where a warm goes. pi holds `model` and reaches its runtime; cyrup captures the installed
    /// provider behind this seam (see [`ProviderWarmStream`]).
    pub stream: Arc<dyn CacheWarmStream>,
    /// The CONCRETE model, already resolved from the request's `ModelRef` — `evaluate`,
    /// [`is_replayable`] and [`cyrup_provider::prompt_cache_ttl_ms`] all need its metadata.
    pub model: Model,
    pub context: Context,
    pub options: StreamOptions,
}

/// Whether the session's model and transcript still match the request — pi's `isCurrent` closure
/// (`cache-warmer.ts:143`, built by `cacheContextIsCurrent`, `sdk.ts:348-362`).
pub type IsCurrent = Arc<dyn Fn() -> bool + Send + Sync>;

/// The mutable half of a run. pi mutates its `ActiveRun` object fields in place; cyrup keeps the
/// run itself immutable and shareable (`Arc`) and puts the fields a reschedule rewrites here.
struct RunMut {
    phase: CacheWarmingPhase,
    next_warm_at: Instant,
    /// Latest safe time to send this refresh, leaving half the original expiry margin.
    refresh_deadline_at: Instant,
    /// Set while a refresh that an extension forced is in flight.
    extension_override: bool,
    /// pi encodes "a refresh is in flight" as `run.timer === undefined` (`cache-warmer.ts:192`);
    /// an explicit flag says the same thing without making the timer handle load-bearing. It must
    /// therefore be raised and lowered at exactly the points where upstream's `timer` is cleared
    /// and re-armed — the TOP of [`CacheWarmer::refresh`] and [`CacheWarmer::schedule`] — and not
    /// around the warm request alone. Narrowing it is not cosmetic: `status` renders `Scheduled`
    /// with `next_warm_in == 0` where upstream renders `Refreshing` (so `/session` says
    /// `Decision now (…)` instead of `Warming cache (…)`), and it un-guards the
    /// `!economics_available && !refreshing` branch, which then prints
    /// `Inactive (cache economics unavailable)` while a warm is being decided.
    refreshing: bool,
    timer: Option<tokio::task::JoinHandle<()>>,
}

/// pi `ActiveRun` (`cache-warmer.ts:141-155`).
struct ActiveRun {
    request: CacheWarmRequest,
    is_current: IsCurrent,
    ttl_ms: u64,
    delay_ms: u64,
    started_at: Instant,
    /// pi's `AbortController` and its `setTimeout` handle, as one signal.
    cancel: CancelToken,
    state: Mutex<RunMut>,
}

/// The warmer's own mutable state: the live run, and the status a stopped run left behind.
struct WarmerState {
    run: Option<Arc<ActiveRun>>,
    /// pi `private inactive: CacheWarmingStatus` (`cache-warmer.ts:164`), seeded to
    /// `{inactive, "waiting for first request"}` at `:183`.
    inactive: CacheWarmingStatus,
}

/// Keeps one prompt cache entry alive by re-sending its request with a one-token output cap before
/// the entry expires — pi `class CacheWarmer` (`cache-warmer.ts:162-401`).
///
/// [`Self::start`] replaces any previous run; warm requests never extend the fixed safety windows.
pub struct CacheWarmer {
    /// The self-handle the armed timer task upgrades. `Arc::new_cyclic` fills it, so a warmer is
    /// only ever reachable as an `Arc` — a timer that outlived its warmer upgrades to `None` and
    /// does nothing.
    me: Weak<CacheWarmer>,
    state: Mutex<WarmerState>,
    host: Arc<dyn CacheWarmingHost>,
    /// pi's `() => settingsManager.getCacheWarmingMode()` (`sdk.ts:319`) — read LIVE at every
    /// checkpoint, never cached, because `/settings` can flip it mid-run.
    ///
    /// It is a cell here rather than a settings read because cyrup's [`crate::AgentSession`] owns
    /// its [`cyrup_config::SettingsManager`] by value on an all-`&self` type, so the session's copy
    /// is frozen at build time. The live half of a `/settings` row therefore pushes the new value
    /// in ([`Self::set_mode`] + [`Self::on_mode_changed`], which is literally what pi's
    /// `setCacheWarmingMode` does), exactly as the `transport` row already pushes into the running
    /// agent. Seeded from `EffectiveSettings::cache_warming_mode()`.
    mode: Mutex<CacheWarmingMode>,
    decide: Arc<dyn CacheWarmingDecider>,
    /// The session's cancellation root. Every run token is a child of it, so disposing the session
    /// cancels an armed timer and an in-flight warm without a separate teardown hook.
    session_cancel: CancelToken,
}

impl CacheWarmer {
    /// Build a warmer. `decide` is the extension seam; pass [`PiDecision`] for "no extension has an
    /// opinion", which is pi's own constructor default (`cache-warmer.ts:177`).
    #[must_use]
    pub fn new(
        host: Arc<dyn CacheWarmingHost>,
        mode: CacheWarmingMode,
        decide: Arc<dyn CacheWarmingDecider>,
        session_cancel: CancelToken,
    ) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            me: me.clone(),
            state: Mutex::new(WarmerState {
                run: None,
                inactive: CacheWarmingStatus {
                    state: CacheWarmingState::Inactive,
                    reason: Some("waiting for first request".to_string()),
                    next_warm_in: None,
                    decision: None,
                    extension_override: false,
                },
            }),
            host,
            mode: Mutex::new(mode),
            decide,
            session_cancel,
        })
    }

    /// The live warming mode — pi `this.getMode()`.
    #[must_use]
    pub fn mode(&self) -> CacheWarmingMode {
        match self.mode.lock() {
            Ok(g) => *g,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    /// Push a new mode in WITHOUT reconciling the live run — the state a persisted
    /// `settingsManager.setCacheWarmingMode(mode)` leaves behind. [`Self::on_mode_changed`] is the
    /// reconcile, and pi's `AgentSession.setCacheWarmingMode` is exactly the two in that order
    /// (`agent-session.ts:1422-1425` @v1.0.4). They are separate because every checkpoint
    /// (`validate_run`, `mode_stop_reason`, `status`) re-reads the mode on its own, so a mode set
    /// with no reconcile still takes effect at the next timer — which is the behaviour upstream's
    /// own tests drive.
    pub fn set_mode(&self, mode: CacheWarmingMode) {
        match self.mode.lock() {
            Ok(mut g) => *g = mode,
            Err(poisoned) => *poisoned.into_inner() = mode,
        }
    }

    /// Poison-safe lock (no panic in a best-effort subsystem).
    fn lock(&self) -> std::sync::MutexGuard<'_, WarmerState> {
        match self.state.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn lock_run(run: &ActiveRun) -> std::sync::MutexGuard<'_, RunMut> {
        match run.state.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// pi `get status()` (`cache-warmer.ts:186-202`). `async` only because `evaluate` reads the
    /// branch; the logic is upstream's, in order.
    pub async fn status(&self) -> CacheWarmingStatus {
        if self.mode() == CacheWarmingMode::Off {
            return Self::inactive_with("cache warming disabled");
        }
        let run = self.lock().run.clone();
        let Some(run) = run else {
            return self.lock().inactive.clone();
        };
        if !(run.is_current)() {
            return Self::inactive_with("conversation context changed");
        }
        let decision = self.evaluate(&run).await;
        let (refreshing, extension_override, next_warm_at) = {
            let st = Self::lock_run(&run);
            (st.refreshing, st.extension_override, st.next_warm_at)
        };
        if !decision.economics_available && !refreshing {
            return Self::inactive_with("cache economics unavailable");
        }
        CacheWarmingStatus {
            state: if refreshing {
                CacheWarmingState::Refreshing
            } else {
                CacheWarmingState::Scheduled
            },
            reason: None,
            next_warm_in: Some(next_warm_at.saturating_duration_since(Instant::now())),
            decision: Some(decision),
            extension_override,
        }
    }

    fn inactive_with(reason: &str) -> CacheWarmingStatus {
        CacheWarmingStatus {
            state: CacheWarmingState::Inactive,
            reason: Some(reason.to_string()),
            next_warm_in: None,
            decision: None,
            extension_override: false,
        }
    }

    /// Keep the prompt cache entry written by `request` warm while `is_current` holds — pi `start`
    /// (`cache-warmer.ts:205-243`).
    pub fn start(&self, request: CacheWarmRequest, is_current: IsCurrent) {
        self.clear_run();
        if self.mode() == CacheWarmingMode::Off {
            self.stop("cache warming disabled", None, false);
            return;
        }
        if !is_replayable(&request.model, &request.options) {
            self.stop("request cannot be replayed safely", None, false);
            return;
        }
        let ttl_ms = prompt_cache_ttl_ms(
            &request.model,
            request.options.cache_retention,
            request.options.env.as_ref(),
        );
        let Some(ttl_ms) = ttl_ms else {
            self.stop(
                if request.options.cache_retention == Some(cyrup_provider::CacheRetention::None) {
                    "request disabled prompt caching"
                } else {
                    "cache lifetime unavailable"
                },
                None,
                false,
            );
            return;
        };
        let Some(delay_ms) = cache_warming_delay_ms(ttl_ms) else {
            self.stop("cache lifetime unavailable", None, false);
            return;
        };
        let now = Instant::now();
        let run = Arc::new(ActiveRun {
            request,
            is_current,
            ttl_ms,
            delay_ms,
            started_at: now,
            cancel: self.session_cancel.child_token(),
            state: Mutex::new(RunMut {
                phase: CacheWarmingPhase::Streaming,
                next_warm_at: now,
                refresh_deadline_at: now,
                extension_override: false,
                refreshing: false,
                timer: None,
            }),
        });
        self.lock().run = Some(run.clone());
        self.schedule(&run);
    }

    /// pi `onAgentSettled` (`cache-warmer.ts:245-257`).
    pub fn on_agent_settled(&self) {
        let run = self.lock().run.clone();
        let Some(run) = run else { return };
        if self.mode() == CacheWarmingMode::Streaming {
            self.stop("agent run settled", None, false);
            return;
        }
        let (next_warm_at, started_at) = {
            let mut st = Self::lock_run(&run);
            st.phase = CacheWarmingPhase::Idle;
            (st.next_warm_at, run.started_at)
        };
        let deadline = started_at + Duration::from_millis(MAX_IDLE_WARMING_AGE_MS);
        if next_warm_at > deadline || Instant::now() >= deadline {
            self.stop("30-minute idle safety limit reached", None, false);
        }
    }

    /// Reconcile an active run after the persisted warming mode changes — pi `onModeChanged`
    /// (`cache-warmer.ts:260-265`).
    pub fn on_mode_changed(&self) {
        let run = self.lock().run.clone();
        let Some(run) = run else { return };
        if let Some(reason) = self.mode_stop_reason(&run) {
            self.stop(reason, None, false);
        }
    }

    /// pi `cancel` (`cache-warmer.ts:267-269`).
    ///
    /// The reason is upstream's own literal, and it means ONE thing: the session is being
    /// disposed. pi's only caller is `dispose()` (`agent-session.ts:1395-1397` @v1.0.4), and
    /// cyrup's only caller is [`crate::AgentSession`]'s teardown. It is deliberately NOT a reason
    /// the guide documents, because a live session must never render it — see
    /// [`Self::stop_unwarmable_model`] for the case that used to.
    pub fn cancel(&self) {
        self.stop("inactive", None, false);
    }

    /// Stop an active run because the request's model cannot be warmed at all: a virtual selection
    /// (which never reaches a provider, so there is no cache entry to refresh) or an id the
    /// installed provider's catalog does not carry (so the model a warm would replay against is
    /// not the one the request used).
    ///
    /// Upstream has no such short-circuit — `sdk.ts:409-411` @v1.0.4 hands `start` whatever model
    /// the `streamFn` was called with, and `start` answers for itself: a model with no
    /// `promptCache` stops with `"cache lifetime unavailable"`. This exists because cyrup resolves
    /// the catalog row BEFORE `start` (the warm needs an owned [`Model`], not a [`ModelRef`]), so
    /// the unresolvable case has to be named here instead. It carries upstream's reason for the
    /// same situation rather than [`Self::cancel`]'s `"inactive"`, which would tell a user on a
    /// virtual-model selection nothing at all and is not a reason the guide documents.
    pub fn stop_unwarmable_model(&self) {
        self.stop("cache lifetime unavailable", None, false);
    }

    /// pi `clearRun` (`cache-warmer.ts:271-277`): forget the run, kill its timer, abort its
    /// request.
    fn clear_run(&self) {
        let run = self.lock().run.take();
        let Some(run) = run else { return };
        let timer = Self::lock_run(&run).timer.take();
        // One signal for both halves (see the module header): the token ends the armed
        // `sleep_until` and aborts an in-flight warm through `StreamOptions::cancel`.
        run.cancel.cancel();
        if let Some(t) = timer {
            t.abort();
        }
    }

    /// pi `stop(reason, stopped?)` (`cache-warmer.ts:279-282`).
    fn stop(&self, reason: &str, decision: Option<CacheWarmingDecision>, extension_override: bool) {
        self.clear_run();
        self.lock().inactive = CacheWarmingStatus {
            state: CacheWarmingState::Inactive,
            reason: Some(reason.to_string()),
            next_warm_in: None,
            decision,
            extension_override,
        };
    }

    /// pi `schedule(run)` (`cache-warmer.ts:284-298`).
    fn schedule(&self, run: &Arc<ActiveRun>) {
        let now = Instant::now();
        let next_warm_at = now + Duration::from_millis(run.delay_ms);
        // A timer can run late after sleep or event-loop blockage. Keep half of the planned
        // pre-expiry margin for that delay and request dispatch; a late refresh is likely a
        // full-price cache write, not a cache warm.
        let refresh_deadline_at =
            next_warm_at + Duration::from_millis((run.ttl_ms - run.delay_ms) / 2);
        let phase = {
            let mut st = Self::lock_run(run);
            st.extension_override = false;
            // The counterpart of the raise at the top of `refresh`: upstream lowers `refreshing`
            // by re-arming `run.timer`, which is this function (`cache-warmer.ts:297`).
            st.refreshing = false;
            st.next_warm_at = next_warm_at;
            st.refresh_deadline_at = refresh_deadline_at;
            st.phase
        };
        let window = match phase {
            CacheWarmingPhase::Idle => MAX_IDLE_WARMING_AGE_MS,
            CacheWarmingPhase::Streaming => MAX_WARMING_AGE_MS,
        };
        let deadline = run.started_at + Duration::from_millis(window);
        if next_warm_at > deadline || now >= deadline {
            self.stop(
                match phase {
                    CacheWarmingPhase::Idle => "30-minute idle safety limit reached",
                    CacheWarmingPhase::Streaming => "one-hour safety limit reached",
                },
                None,
                false,
            );
            return;
        }
        let me = self.me.clone();
        let cancel = run.cancel.clone();
        let armed = run.clone();
        let handle = tokio::spawn(async move {
            tokio::select! {
                _ = cancel.cancelled() => {}
                _ = tokio::time::sleep_until(next_warm_at) => {
                    if let Some(warmer) = me.upgrade() {
                        warmer.refresh(armed).await;
                    }
                }
            }
        });
        Self::lock_run(run).timer = Some(handle);
    }

    /// pi `refresh(run)` (`cache-warmer.ts:300-355`).
    async fn refresh(&self, run: Arc<ActiveRun>) {
        {
            // Upstream's `run.timer = undefined` IS its `refreshing`, so the two move together:
            // from here until `schedule` re-arms, this run is refreshing as far as `status` is
            // concerned. The decide window below is deliberately allowed to run to the refresh
            // deadline, so it is the longest part of that span and the part a user is most likely
            // to catch with `/session`.
            let mut st = Self::lock_run(&run);
            st.timer = None;
            st.refreshing = true;
        }
        if !self.validate_run(&run) {
            return;
        }
        if self.refresh_deadline_missed(&run) {
            return;
        }
        let decision = self.evaluate(&run).await;
        let event = CacheWarmingDecisionEvent {
            warm_cost: decision.warm_cost,
            miss_cost: decision.miss_cost,
            continuation_probability: decision.continuation_probability,
            action: decision.action,
        };
        // "Extension failures fall back to pi's own decision" (`cache-warmer.ts:315-317`). The
        // `AssertUnwindSafe` catch covers a panicking decider too, which is the Rust shape of the
        // same promise: cache warming must not affect the active agent run.
        //
        // EXT-085: the call is also BOUNDED, by the time left to this run's refresh deadline. Once
        // a decider is an extension hook it is third-party code — a guest handler, or several —
        // and pi's `await this.decide(...)` has no deadline at all because a hung JS handler still
        // yields its event loop. A hung Rust future does not: the refresh task would simply never
        // return, leaving the run armed with no timer, no stop reason and a status stuck on
        // `scheduled`, i.e. a broken extension silently disables warming for the rest of the
        // session. The refresh deadline is the one bound that is already meaningful here — past it
        // a warm is a full-price cache write, not a warm, so an answer that arrives later cannot
        // be acted on anyway. On elapse pi's own action stands and the deadline re-check below
        // stops the run with `"cache refresh deadline missed"`, which is exactly what a slow
        // handler deserves and what upstream's own post-decision re-check is there for.
        let budget = Self::lock_run(&run)
            .refresh_deadline_at
            .saturating_duration_since(Instant::now());
        let action = match tokio::time::timeout(
            budget,
            futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(
                self.decide.decide(&event),
            )),
        )
        .await
        {
            Ok(Ok(a)) => a,
            Ok(Err(_)) | Err(_) => decision.action,
        };
        if !self.validate_run(&run) || self.refresh_deadline_missed(&run) {
            return;
        }
        let extension_override = action != decision.action;
        if action == CacheWarmingAction::Stop {
            let reason = if extension_override {
                "stopped by extension"
            } else if decision.economics_available {
                "expected savings below threshold"
            } else {
                "cache economics unavailable"
            };
            self.stop(reason, Some(decision), extension_override);
            return;
        }

        Self::lock_run(&run).extension_override = extension_override;
        let warm_opts = StreamOptions {
            max_tokens: Some(1),
            max_retries: Some(0),
            cancel: Some(run.cancel.clone()),
            ..run.request.options.clone()
        };
        let message = collect_message(run.request.stream.stream_warm(
            &run.request.model,
            &run.request.context,
            &warm_opts,
        ))
        .await;
        if !self.validate_run(&run) {
            return;
        }
        if message.stop_reason != StopReason::Error && message.stop_reason != StopReason::Aborted {
            self.host
                .record_warm(
                    message.provider.clone(),
                    ModelId::from(
                        message
                            .response_model
                            .clone()
                            .unwrap_or_else(|| message.model.clone()),
                    ),
                    message.usage.clone(),
                    extension_override.then(|| "extension override".to_string()),
                )
                .await;
        }
        // `if (this.run === run) this.schedule(run)` — the run may have been replaced while the
        // warm was in flight.
        let still_live = self
            .lock()
            .run
            .as_ref()
            .is_some_and(|r| Arc::ptr_eq(r, &run));
        if still_live {
            self.schedule(&run);
        }
    }

    /// pi `refreshDeadlineMissed` (`cache-warmer.ts:357-361`).
    fn refresh_deadline_missed(&self, run: &Arc<ActiveRun>) -> bool {
        if Instant::now() <= Self::lock_run(run).refresh_deadline_at {
            return false;
        }
        self.stop("cache refresh deadline missed", None, false);
        true
    }

    /// pi `validateRun` (`cache-warmer.ts:363-369`).
    fn validate_run(&self, run: &Arc<ActiveRun>) -> bool {
        if !self
            .lock()
            .run
            .as_ref()
            .is_some_and(|r| Arc::ptr_eq(r, run))
        {
            return false;
        }
        let reason = self.mode_stop_reason(run).or(if (run.is_current)() {
            None
        } else {
            Some("conversation context changed")
        });
        match reason {
            None => true,
            Some(reason) => {
                self.stop(reason, None, false);
                false
            }
        }
    }

    /// pi `getModeStopReason` (`cache-warmer.ts:371-376`).
    fn mode_stop_reason(&self, run: &Arc<ActiveRun>) -> Option<&'static str> {
        match self.mode() {
            CacheWarmingMode::Off => Some("cache warming disabled"),
            CacheWarmingMode::Streaming if Self::lock_run(run).phase == CacheWarmingPhase::Idle => {
                Some("agent run settled")
            }
            _ => None,
        }
    }

    /// pi `evaluate(run)` (`cache-warmer.ts:378-400`).
    async fn evaluate(&self, run: &Arc<ActiveRun>) -> CacheWarmingDecision {
        let cost = &run.request.model.cost;
        let prompt_tokens = self.host.last_prompt_tokens().await;
        let cache_hit_cost = price(cost, 0, 0, prompt_tokens, 0);
        let cache_miss_cost = if cost.cache_write > 0.0 {
            price(cost, 0, 0, 0, prompt_tokens)
        } else {
            price(cost, prompt_tokens, 0, 0, 0)
        };
        let warm_cost = price(cost, 0, 1, prompt_tokens, 0);
        let miss_cost = (cache_miss_cost - cache_hit_cost).max(0.0);
        let phase = Self::lock_run(run).phase;
        let continuation_probability = match phase {
            CacheWarmingPhase::Idle => IDLE_CONTINUATION_PROBABILITY,
            CacheWarmingPhase::Streaming => 1.0,
        };
        let economics_available =
            prompt_tokens > 0 && (cache_hit_cost > 0.0 || cache_miss_cost > 0.0);
        let expected_savings = continuation_probability * miss_cost - warm_cost;
        CacheWarmingDecision {
            phase,
            warm_cost,
            miss_cost,
            continuation_probability,
            expected_savings,
            economics_available,
            action: if expected_savings >= CACHE_WARMING_MINIMUM_EXPECTED_SAVINGS {
                CacheWarmingAction::Warm
            } else {
                CacheWarmingAction::Stop
            },
        }
    }

    /// Disarm the live run's timer WITHOUT touching the run — the test seam for upstream's
    /// `vi.clearAllTimers()` (`test/cache-warmer.test.ts:183`, `:207`), which is how a timer that
    /// fires late after a machine sleep is simulated. Paired with
    /// [`Self::refresh_active_run_for_test`].
    #[cfg(test)]
    pub(crate) fn disarm_timer_for_test(&self) {
        let timer = self
            .lock()
            .run
            .clone()
            .and_then(|run| Self::lock_run(&run).timer.take());
        if let Some(t) = timer {
            t.abort();
        }
    }

    /// Run the refresh for the live run inline — upstream's `internal.refresh(internal.run)`
    /// (`test/cache-warmer.test.ts:186`). Returns `false` when there is no live run.
    #[cfg(test)]
    pub(crate) async fn refresh_active_run_for_test(&self) -> bool {
        let Some(run) = self.lock().run.clone() else {
            return false;
        };
        self.refresh(run).await;
        true
    }

    /// Whether a run is live (test-only; upstream reads `internal.run` directly).
    #[cfg(test)]
    pub(crate) fn has_active_run(&self) -> bool {
        self.lock().run.is_some()
    }
}

impl Drop for CacheWarmer {
    /// A dropped warmer must not leave a timer armed against a session that is gone.
    fn drop(&mut self) {
        let run = match self.state.lock() {
            Ok(mut g) => g.run.take(),
            Err(poisoned) => poisoned.into_inner().run.take(),
        };
        if let Some(run) = run {
            run.cancel.cancel();
        }
    }
}

/// pi `formatDollars` (`cache-warmer.ts:403-405`).
fn format_dollars(value: f64) -> String {
    if value < 0.0 {
        format!("-${:.3}", value.abs())
    } else {
        format!("${value:.3}")
    }
}

/// pi `formatCacheWarmingEconomics` (`cache-warmer.ts:407-416`).
fn format_cache_warming_economics(decision: &CacheWarmingDecision) -> String {
    if !decision.economics_available {
        return "cache economics unavailable".to_string();
    }
    let probability = (decision.continuation_probability * 100.0).round() as i64;
    let probability_text = match decision.phase {
        CacheWarmingPhase::Streaming => {
            format!("{probability}% continuation probability while agent is running")
        }
        CacheWarmingPhase::Idle => format!("{probability}% continuation probability"),
    };
    let comparison = match decision.action {
        CacheWarmingAction::Warm => ">=",
        CacheWarmingAction::Stop => "<",
    };
    format!(
        "{probability_text}, expected savings {} {comparison} ${:.3}",
        format_dollars(decision.expected_savings),
        CACHE_WARMING_MINIMUM_EXPECTED_SAVINGS
    )
}

/// pi `formatCacheWarmingDecisionTime(nextWarmAt, now)` (`cache-warmer.ts:418-430`), reading the
/// remaining duration the upstream formatter computes as `nextWarmAt - now`.
#[must_use]
pub fn format_cache_warming_decision_time(remaining: Option<Duration>) -> String {
    let Some(remaining) = remaining.filter(|d| !d.is_zero()) else {
        return "Decision now".to_string();
    };
    // `Math.ceil((nextWarmAt - now) / 1000)`.
    let mut remaining_seconds = remaining.as_millis().div_ceil(1000) as u64;
    let hours = remaining_seconds / 3600;
    remaining_seconds %= 3600;
    let minutes = remaining_seconds / 60;
    let seconds = remaining_seconds % 60;
    let mut parts: Vec<String> = Vec::new();
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes}m"));
    }
    if seconds > 0 || parts.is_empty() {
        parts.push(format!("{seconds}s"));
    }
    format!("Decision in {}", parts.join(" "))
}

/// One-line status for `/session` — pi `formatCacheWarmingStatus` (`cache-warmer.ts:433-446`).
///
/// The first predicate is counter-intuitive and is upstream's verbatim: `Inactive (reason)` when
/// there is NO decision, or when the state is inactive AND the economics are unavailable AND no
/// extension overrode anything. Every other combination reports a decision.
#[must_use]
pub fn format_cache_warming_status(status: &CacheWarmingStatus) -> String {
    let Some(decision) = status.decision.as_ref() else {
        return format!(
            "Inactive ({})",
            status.reason.as_deref().unwrap_or("unknown reason")
        );
    };
    if status.state == CacheWarmingState::Inactive
        && !decision.economics_available
        && !status.extension_override
    {
        return format!(
            "Inactive ({})",
            status.reason.as_deref().unwrap_or("unknown reason")
        );
    }
    let economics = format_cache_warming_economics(decision);
    let details = if status.extension_override {
        format!("extension override, {economics}")
    } else {
        format!(
            "{economics} -> {}",
            match decision.action {
                CacheWarmingAction::Warm => "warm",
                CacheWarmingAction::Stop => "stop",
            }
        )
    };
    match status.state {
        CacheWarmingState::Inactive => format!("Stopped ({details})"),
        CacheWarmingState::Refreshing => format!("Warming cache ({details})"),
        CacheWarmingState::Scheduled => format!(
            "{} ({details})",
            format_cache_warming_decision_time(status.next_warm_in)
        ),
    }
}

/// One-line transcript text for a persisted cache-warming usage entry — pi
/// `formatCacheWarmingUsage` (`cache-warmer.ts:449-453` @v1.0.4):
///
/// ```ts
/// const note = entry.note ? ` (${entry.note})` : "";
/// const cost = entry.usage.cost.total.toFixed(6).replace(/(\.\d{3}\d*?)0+$/, "$1");
/// return `Cache warmed${note}: $${cost}`;
/// ```
///
/// The cost is six decimal places with trailing zeros trimmed **but never below three** — a warm
/// costs one cache read plus one output token, which is routinely under a tenth of a cent, so
/// `$0.000` would be the only thing a 3dp-only format could ever print. The regex is implemented
/// as "keep the first three decimals, trim trailing zeros from the remaining three", which is
/// exactly what `(\.\d{3}\d*?)0+$` matches: the lazy `\d*?` takes the shortest run that leaves
/// nothing but zeros to the end of the string.
///
/// `note` is already `Option`-shaped by [`cyrup_session::entry::KnownEntry::Usage`], whose writer
/// normalizes pi's falsy empty string to `None` — so an empty note cannot reach the ` (…)` suffix
/// from either the live or the replay path.
#[must_use]
pub fn format_cache_warming_usage(note: Option<&str>, cost_total: f64) -> String {
    let note = match note {
        Some(n) if !n.is_empty() => format!(" ({n})"),
        _ => String::new(),
    };
    format!("Cache warmed{note}: ${}", format_warm_cost(cost_total))
}

/// `value.toFixed(6).replace(/(\.\d{3}\d*?)0+$/, "$1")`.
///
/// Rust's `{:.6}` and JS `toFixed(6)` can only disagree on an exact decimal tie at the seventh
/// place, which no sum of IEEE-754 token prices lands on; every magnitude a warm produces formats
/// identically.
fn format_warm_cost(value: f64) -> String {
    let fixed = format!("{value:.6}");
    // `{:.6}` always emits a `.` followed by six digits, so both splits are infallible; the
    // `unwrap_or` arms are unreachable and simply decline to panic in a cost notice.
    let Some(dot) = fixed.find('.') else {
        return fixed;
    };
    let keep = dot + 4; // `.` + the three decimals the regex preserves
    if keep >= fixed.len() {
        return fixed;
    }
    let (head, tail) = fixed.split_at(keep);
    format!("{head}{}", tail.trim_end_matches('0'))
}

/// The production [`CacheWarmingHost`]: the live session tree plus the seam's fan-out.
///
/// pi hands the warmer `Pick<SessionManager, "appendUsage" | "getBranch">` directly and wires
/// `onWarmed` to `this._emit({type: "entry_appended", entry})` (`agent-session.ts:483-486`
/// @v1.0.4). cyrup's manager is behind a `tokio::sync::Mutex`, so both reads are `async`, and the
/// two halves of a recorded warm — the appended entry and the event that renders it — are one
/// method (see [`CacheWarmingHost::record_warm`]).
pub(crate) struct SessionCacheWarmingHost {
    manager: Arc<tokio::sync::Mutex<cyrup_session::manager::SessionManager>>,
    fanout: Arc<crate::subscriber::Fanout>,
}

impl SessionCacheWarmingHost {
    pub(crate) fn new(
        manager: Arc<tokio::sync::Mutex<cyrup_session::manager::SessionManager>>,
        fanout: Arc<crate::subscriber::Fanout>,
    ) -> Self {
        Self { manager, fanout }
    }
}

impl CacheWarmingHost for SessionCacheWarmingHost {
    fn last_prompt_tokens(&self) -> BoxFuture<'_, u64> {
        Box::pin(async move {
            let mgr = self.manager.lock().await;
            // `getBranch()` — the CURRENT branch, not every entry, so a `/tree` jump prices the
            // prompt the next request will actually send.
            last_prompt_tokens(&mgr.branch_path(None))
        })
    }

    fn record_warm(
        &self,
        provider: ProviderId,
        model: ModelId,
        usage: Usage,
        note: Option<String>,
    ) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let appended = {
                let mut mgr = self.manager.lock().await;
                mgr.append_usage("cache_warm", provider, model, usage, note.as_deref())
            };
            match appended {
                Ok(entry) => match serde_json::to_value(&entry) {
                    Ok(value) => {
                        self.fanout
                            .emit_external(crate::event::AgentSessionEvent::EntryAppended {
                                entry: value,
                            })
                            .await;
                    }
                    // Unreachable: the entry round-trips through the same `Serialize` the session
                    // file is written with. Best-effort, as the whole subsystem is.
                    Err(e) => {
                        tracing::debug!(error = %e, "cache-warm usage entry failed to serialize")
                    }
                },
                Err(e) => tracing::debug!(error = %e, "cache-warm usage entry failed to append"),
            }
        })
    }
}

/// The [`CacheWarmingDecider`] that lets extensions override the warm-or-stop verdict — EXT-085,
/// the cyrup counterpart of pi's `decide` closure
/// (`async (event) => extensionRunnerRef.current?.emitCacheWarmingDecision(event) ?? event.action`,
/// `core/sdk.ts:316-321` @v1.0.4).
///
/// It is a thin adapter: the whole reduction — run EVERY subscribed extension's handler in load
/// order, take the LAST readable `{action}`, contain and skip a handler that faults — lives in
/// `cyrup_ext`'s `aggregate_cache_warming_decision` / `fold_cache_warming_decision`, which is
/// where pi's `emitCacheWarmingDecision` (`core/extensions/runner.ts:1121-1142`) belongs. Here
/// there is only the host held WEAKLY (an extension host outliving nothing, so a torn-down session
/// answers with pi's own action) and the two enum conversions across the crate boundary.
///
/// Both of pi's fallbacks are reproduced. Upstream's `?? event.action` covers "no runner bound
/// yet", which here is the dropped `Weak`; its `try/catch` around the whole `decide` call is the
/// warmer's own `catch_unwind` + deadline timeout in [`CacheWarmer::refresh`], so a broken or slow
/// extension costs at most this one refresh.
pub struct ExtensionCacheWarmingDecider {
    host: Weak<cyrup_ext::ExtensionHost>,
    cancel: CancelToken,
}

impl ExtensionCacheWarmingDecider {
    pub fn new(host: &Arc<cyrup_ext::ExtensionHost>, cancel: CancelToken) -> Self {
        Self {
            host: Arc::downgrade(host),
            cancel,
        }
    }
}

impl CacheWarmingDecider for ExtensionCacheWarmingDecider {
    fn decide<'a>(
        &'a self,
        event: &'a CacheWarmingDecisionEvent,
    ) -> BoxFuture<'a, CacheWarmingAction> {
        Box::pin(async move {
            let Some(host) = self.host.upgrade() else {
                // pi's `?? event.action`: no runner, no override.
                return event.action;
            };
            let folded = host
                .aggregate_cache_warming_decision(
                    event.warm_cost,
                    event.miss_cost,
                    event.continuation_probability,
                    match event.action {
                        CacheWarmingAction::Warm => cyrup_ext::CacheWarmingAction::Warm,
                        CacheWarmingAction::Stop => cyrup_ext::CacheWarmingAction::Stop,
                    },
                    &self.cancel,
                )
                .await;
            match folded {
                cyrup_ext::CacheWarmingAction::Warm => CacheWarmingAction::Warm,
                cyrup_ext::CacheWarmingAction::Stop => CacheWarmingAction::Stop,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// pi `formatCacheWarmingUsage` (`cache-warmer.ts:449-453` @v1.0.4), byte for byte.
    ///
    /// **Red-proved** by replacing `format_warm_cost` with `format!("{value:.3}")`: the first three
    /// cases below failed (`$0.000` for all of them), which is exactly the bug the six-decimal
    /// format exists to avoid — and by dropping the `trim_end_matches`, which failed the `0.001`
    /// and `1.5` cases with `$0.001000` / `$1.500000`.
    #[test]
    fn seam131_warm_usage_line_matches_upstream() {
        assert_eq!(
            format_cache_warming_usage(None, 0.000_1),
            "Cache warmed: $0.0001"
        );
        assert_eq!(
            format_cache_warming_usage(None, 0.000_012),
            "Cache warmed: $0.000012"
        );
        assert_eq!(
            format_cache_warming_usage(None, 0.123_456),
            "Cache warmed: $0.123456"
        );
        // `.toFixed(6)` then trim: three decimals is the floor, so a true zero is `$0.000`.
        assert_eq!(
            format_cache_warming_usage(None, 0.0),
            "Cache warmed: $0.000"
        );
        assert_eq!(
            format_cache_warming_usage(None, 0.001),
            "Cache warmed: $0.001"
        );
        assert_eq!(
            format_cache_warming_usage(None, 1.5),
            "Cache warmed: $1.500"
        );
        // `entry.note ? \` (${entry.note})\` : ""`.
        assert_eq!(
            format_cache_warming_usage(Some("extension override"), 0.000_25),
            "Cache warmed (extension override): $0.00025"
        );
        // pi's `entry.note ?` is falsy for the empty string, which the entry writer already
        // normalizes away; the formatter declines it too rather than printing ` ()`.
        assert_eq!(
            format_cache_warming_usage(Some(""), 0.001),
            "Cache warmed: $0.001"
        );
    }
}
