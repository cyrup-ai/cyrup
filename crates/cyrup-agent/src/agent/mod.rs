//! The high-level [`Agent`] + the turn-based loop (arch-02 §3.5 / §6, func-02 §5/§6).
//!
//! One run = one tokio task that owns the `RunCancel` root. All event emission and hook invocation
//! happen on that single task, so ordering is deterministic; only tool `execute` bodies run
//! concurrently (on a `JoinSet`). The state lock is taken only for the synchronous reducer and is
//! never held across a subscriber `await` (deadlock-freedom, arch-02 §5.5).
//
// KNOWN GAPS (tracked):
// - R-02-020: DONE — JSON-Schema argument validation + coercion runs in preflight via
//   `cyrup_provider::validate_tool_call` (func-01 R-01-034): raw args are first normalized by the
//   tool's `prepare_arguments` compat shim (Pi `prepareToolCallArguments`, agent-loop.ts:548-560),
//   then validated/coerced before `before_tool_call`, and a validation failure yields an immediate
//   isError tool-result (the model retries) without executing the tool. Args mutated by
//   `before_tool_call` still run as-is, without re-validation (R-02-022).
// - A-02-10 (second half): no mutable-aliasing state getter is exposed (snapshots are copies and
//   setters copy-on-assign). Intentional Rust `[CYRUP-DELTA]` from the TS source.
// - thinkingBudgets (Pi `AgentOptions.thinkingBudgets`, agent.ts:112): DONE — forwarded via
//   `GenerationConfig.thinking_budgets` into `cyrup_provider::StreamOptions.thinking_budgets`
//   (anthropic-messages.ts:792-797 lowers it per-level). The unified `reasoning` level is forwarded
//   alongside it.
// - Proxy `StreamFn` (Pi `streamProxy`, proxy.ts): PORTED in `proxy/` — the wire enum
//   (`ProxyAssistantMessageEvent`), client-side partial rebuild (`ProxyMessageBuilder`, Pi
//   `processProxyEvent`), options/body (`ProxyStreamOptions`/`buildProxyRequestOptions`), and the
//   `POST {proxyUrl}/api/stream` bearer-SSE transport (`stream_proxy`/`ProxyStreamFn`). Transport
//   reuses cyrup-provider's existing SSE client (`open_sse`) — no new dependency.

mod builder;
mod facade;
mod lifecycle;
mod message;
mod prompt;
mod run;
mod util;

pub use builder::AgentBuilder;
pub use facade::Subscription;
pub use lifecycle::RunHandle;
pub use prompt::PromptInput;

// `crate::agent::{RunEntry, RunCtx, …}` — the paths `crate::loop_fn` imports; keep them resolving.
pub(crate) use run::{PromptSource, ResumePoint, RunBaseline, RunCtx, RunEntry, RunShared};

use crate::hooks::Hooks;
use crate::queue::{PendingQueue, ToolExecution};
use crate::state::{GenerationConfig, StateInner};
use crate::stream_fn::{ApiKeyResolver, StreamFn};
use crate::subscriber::EventSubscriber;
use cyrup_core::{ModelRef, RunCancel, SessionId};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

// ---------------------------------------------------------------------------
// The public Agent
// ---------------------------------------------------------------------------

/// The stateful, high-level agent front-ends and extensions use (func-02 R-02-057).
pub struct Agent {
    state: Arc<Mutex<StateInner>>,
    subscribers: Arc<Mutex<Vec<Arc<dyn EventSubscriber>>>>,
    steering: Arc<Mutex<PendingQueue>>,
    follow_up: Arc<Mutex<PendingQueue>>,
    hooks: Arc<dyn Hooks>,
    stream_fn: Arc<dyn StreamFn>,
    key_resolver: Option<Arc<dyn ApiKeyResolver>>,
    cancel_slot: Arc<Mutex<Option<RunCancel>>>,
    /// The SINGLE run-in-flight latch (R-02-045..048). `claim_and_snapshot` claims it with an atomic
    /// compare-and-set (`watch::Sender::send_if_modified`), [`lifecycle::SettlementGuard`] releases it, and
    /// both [`Agent::wait_for_idle`] and [`Agent::is_running`] read it — so "the waiter observed
    /// idle" and "a new run may start" are the same fact, never two facts written in sequence.
    running_tx: watch::Sender<bool>,
    running_rx: watch::Receiver<bool>,
    tool_execution: ToolExecution,
    session_id: Option<SessionId>,
    gen_config: GenerationConfig,
    /// AGENT-029 — the per-request header resolver (see [`HeaderFn`]). Installed by the session
    /// facade after construction; `None` for a bare embedder agent, which keeps reading the static
    /// `state.headers` overlay.
    header_fn: Arc<Mutex<Option<Arc<HeaderFn>>>>,
}

/// Resolve the per-request header overlay for the model a turn is ACTUALLY going to — cyrup's port
/// of pi's `transformHeaders` closure (`packages/coding-agent/src/core/sdk.ts:312-328` @v0.83.0,
/// byte- and offset-identical at v0.84.1).
///
/// pi merges attribution inside a per-request callback closed over the `model` argument of *that*
/// `streamSimple` invocation — i.e. the model the loop chose for that turn
/// (`agent-loop.ts:308`, whose `config.model` is the possibly-overridden
/// `nextTurnSnapshot.model ?? config.model` from `:237`). A model change of ANY origin therefore
/// gets the right headers by construction. cyrup previously read a latched `StateInner::headers`
/// snapshot whose only writers were the two SESSION-level model-change paths, so a per-turn
/// `TurnUpdate::model` override retargeted the request while the previous provider's attribution
/// headers rode along.
///
/// Returning `None` means "no opinion for this model" and falls back to the static
/// [`Agent::set_headers`] overlay.
pub type HeaderFn = dyn Fn(&ModelRef) -> Option<cyrup_provider::HeaderMap> + Send + Sync;
