//! The connection supervisor — `ensureConnected` + `scheduleReconnect` (a port of
//! `pi-intercom/index.ts:794-861`, plus `getReconnectDelayMs` `:564-567`, the client `disconnected`
//! handler `:779-789`, the `startSessionRuntime` reset `:926-951`, the startup connect `:952-965`
//! and the `session_shutdown` teardown `:1060-1064`).
//!
//! Before this module the whole session had exactly ONE connect attempt (the `SessionStart`
//! background task): a broker that was not yet up, or a broker restart mid-session, left
//! `SharedIntercomState::client()` empty (or holding a dead client) for the rest of the process, so
//! every tool call, overlay command and seam relay failed permanently. pi recovers via a
//! rate-limited reconnect ladder; this is that ladder.
//!
//! ## Bound, backoff, cancellation — read this before changing anything here
//!
//! - **Backoff** is pi's literal table (`index.ts:565`): `[1000, 2000, 5000, 10000, 30000]` ms,
//!   indexed by `min(attempt, 4)`. The attempt counter is incremented inside the timer body (after
//!   the wait, before the attempt, `index.ts:804`) and reset to 0 on a successful connect
//!   (`:841`) and on [`begin_runtime`] (`:936`).
//! - **Attempt bound**: upstream has NO maximum attempt count, and neither does this port. The
//!   reconnect is bounded in *rate*, not in *attempts*: once the ladder saturates, the ceiling is
//!   one attempt per 30 s, i.e. at most 2 connect syscalls per minute — that is what rules out the
//!   busy-loop against a dead broker. An attempt cap would permanently kill intercom for a session
//!   that outlives a long broker outage, which is the exact failure this module exists to fix.
//! - **Cancellation** has three independent guards, all of them pi's:
//!   1. [`shutdown`] sets `shutting_down` and ABORTS the pending timer task
//!      (`shuttingDown = true; disposed = true; clearReconnectTimer()`, `index.ts:1060-1064`);
//!   2. every timer body re-checks `shutting_down`/`started` AFTER its sleep, so a shutdown that
//!      races the abort still cannot connect (`index.ts:801`);
//!   3. a *generation* stamp captured at schedule time is re-checked after the sleep, so a session
//!      REPLACEMENT (not just a shutdown) also invalidates an in-flight reconnect
//!      (`runtimeGeneration`, `index.ts:797,801`).
//!
//!   [`ensure_connected`] refuses outright while `shutting_down` (`index.ts:813-815`).
//! - **Single-flight**: pi dedups concurrent callers by returning the same `reconnectPromise`
//!   (`index.ts:826-827,851-856`). Rust has no shareable promise here, so the equivalent is an
//!   async gate plus an attempt *epoch*: a caller that waited on the gate re-checks the live client
//!   first, and if the epoch moved while it waited it adopts that attempt's failure instead of
//!   stacking a second connect. Net effect matches pi — N concurrent callers produce ONE connect.
//! - **Only a `Background` failure re-arms the ladder** (`index.ts:847-849`); a `Tool`/`Overlay`
//!   failure surfaces to its caller without arming a retry storm, and `Startup` arms it explicitly
//!   from its own catch (`index.ts:963-964`). This is a faithful port of pi's asymmetry, including
//!   its one sharp edge: `ensure_connected` clears a pending timer before attempting
//!   (`clearReconnectTimer()`, `index.ts:824`), so a failed tool-triggered attempt leaves the
//!   ladder disarmed until the next tool call or the next disconnect edge.
//! - **Duplicate delivery is not possible and therefore not handled** — the broker answers every
//!   send synchronously with `Delivered`/`DeliveryFailed` (`broker/mod.rs::handle_send`) and each
//!   message is either handed to exactly one live socket or parked in the mailbox for exactly one
//!   later `register`, never both: `flush_mailbox_for_session` SPLICES the entry out before it
//!   writes (`v0.10.1 broker/broker.ts:933-943`). **Superseded 2026-08-14 (ICOM-010): this note
//!   used to assert "there is no mailbox, no queue, no redelivery" — that was true of the v0.7.0
//!   broker it was written against and is no longer true of this one.** A message in flight when a
//!   socket drops is still lost rather than re-sent, so a reconnect cannot deliver it twice; what
//!   changed is that a message the broker ACCEPTS for a departed peer is now retained for
//!   `MAILBOX_MESSAGE_RETENTION_MS` instead of being refused `Session not found`. What DOES need
//!   care is the
//!   outbound reply waiter: pi rejects it on the disconnect edge BEFORE nulling the client
//!   (`index.ts:784`) so an ask cannot hang across a reconnect — [`handle_disconnect`] does the
//!   same via [`crate::reply_tracker::OutboundReplyWaiter::fail_pending`].

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::error::{IntercomError, Result};
use crate::identity::{ChildOrchestratorMetadata, ENV_INTERCOM_SESSION_ID, presence_name};
use crate::inbound::spawn_inbound_loop;
use crate::session_state::SharedIntercomState;
use crate::transport::client::IntercomClient;
use crate::transport::protocol::{SessionRegistration, now_ms};
use crate::transport::spawn::ensure_broker;
use crate::transport::target::broker_connect_target;

/// pi's reconnect backoff ladder (`getReconnectDelayMs`, `index.ts:565`), in milliseconds. The last
/// entry is the ceiling: every attempt past the table length waits 30 s, which is what bounds the
/// reconnect in rate (see the module docs).
pub const RECONNECT_BACKOFF_MS: [u64; 5] = [1000, 2000, 5000, 10_000, 30_000];

/// `backoffMs[Math.min(reconnectAttempt, backoffMs.length - 1)]` (`index.ts:566`).
#[must_use]
pub fn reconnect_delay_ms(attempt: u32) -> u64 {
    let last = RECONNECT_BACKOFF_MS.len().saturating_sub(1);
    let idx = usize::try_from(attempt).unwrap_or(last).min(last);
    RECONNECT_BACKOFF_MS.get(idx).copied().unwrap_or(30_000)
}

/// Why a connect was requested (pi's `reason: "startup" | "background" | "tool" | "overlay"`,
/// `index.ts:810`). Only [`ConnectReason::Background`] re-arms the reconnect ladder from a failure
/// (`index.ts:847-849`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectReason {
    /// The `SessionStart` connect (`index.ts:959`).
    Startup,
    /// The reconnect ladder itself, and the subagent-relay path (`index.ts:806,1000`).
    Background,
    /// A tool call — `intercom` / `contact_supervisor` (`index.ts:1231,1477`).
    Tool,
    /// The `/intercom` overlay command (`index.ts:1827,1864`).
    Overlay,
}

/// Everything a reconnect needs to rebuild an identical registration, captured once at
/// `SessionStart` (pi keeps these as the closure vars `runtimeContext`/`currentSessionId`/
/// `currentModel`/`sessionStartedAt`, `index.ts:940-947`).
#[derive(Clone, Debug)]
pub struct ConnectParams {
    /// The resolved agent dir (the broker socket + spawn lock live under it).
    pub agent_dir: PathBuf,
    /// This session's child-orchestrator metadata, when it is a subagent child.
    pub metadata: Option<ChildOrchestratorMetadata>,
    /// The model id reported in presence (`ctx.model?.id ?? "unknown"`, `index.ts:944`).
    pub model: Option<String>,
}

/// The connection supervisor's state, owned by [`SharedIntercomState`].
#[derive(Debug, Default)]
pub struct ConnectSupervisor {
    params: Mutex<Option<Arc<ConnectParams>>>,
    /// pi `shuttingDown || disposed` — refuses every connect once the session tears down.
    shutting_down: AtomicBool,
    /// "A runtime is CURRENTLY active" — set by [`begin_runtime`], **cleared** by [`shutdown`], so a
    /// pre-session or post-shutdown `schedule_reconnect` is a no-op.
    ///
    /// This is deliberately NOT pi's `runtimeStarted` (see [`Self::runtime_ever_started`]), even
    /// though an earlier comment here claimed it was: pi never clears `runtimeStarted`, and the two
    /// meanings diverge exactly at shutdown.
    started: AtomicBool,
    /// pi `runtimeStarted` (`v0.10.1 index.ts:522`, set at `:1253`) — a LATCH: it goes true at the
    /// first `startSessionRuntime` and is never set back to false anywhere in upstream.
    ///
    /// Its one consumer is `sendIncomingMessage`'s guard (`:877`,
    /// `if (runtimeStarted && !getLiveContext(runtimeContext, generation)) return;`) and the relay's
    /// `relayStillLive` (`:1311`). Both read it as "is there a runtime this delivery could be stale
    /// relative to" — and after a shutdown there certainly is, so folding this into
    /// [`Self::started`] would let a post-shutdown delivery bypass the fence entirely, which is the
    /// inverse of the guard's purpose.
    runtime_ever_started: AtomicBool,
    /// pi `runtimeGeneration` (`index.ts:449,936,1062`): bumped by [`begin_runtime`]/[`shutdown`];
    /// an in-flight reconnect whose stamp no longer matches is dropped on the floor.
    generation: AtomicU64,
    /// pi `reconnectAttempt` (`index.ts:447`) — the backoff-ladder index.
    attempt: AtomicU32,
    /// pi `reconnectTimer` (`index.ts:439`). Replacing/clearing it aborts the sleeping task, which
    /// is the cancellation path a shutdown uses.
    timer: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// True while a connect attempt is actually running (pi's non-null `reconnectPromise` as seen
    /// by `scheduleReconnect`'s early return, `index.ts:795`).
    connecting: AtomicBool,
    /// The single-flight gate (see the module docs).
    gate: tokio::sync::Mutex<()>,
    /// Incremented once per completed attempt; lets a caller that queued behind the gate tell
    /// "nobody has tried yet" from "an attempt just failed" (pi's shared `reconnectPromise`).
    epoch: AtomicU64,
    /// The most recent attempt's failure, replayed to callers that queued behind it.
    last_error: Mutex<Option<String>>,
    /// The broker-assigned session id from the last successful connect, re-offered on reconnect so
    /// this session keeps its identity across the drop (broker identity takeover,
    /// `broker/mod.rs:303`; pi keeps `currentSessionId` stable for the same reason).
    last_session_id: Mutex<Option<String>>,
    /// pi `currentSessionId` (`v0.10.1 index.ts:507`), captured from
    /// `ctx.sessionManager.getSessionId()` at `startSessionRuntime` (`:1266`).
    ///
    /// Distinct from [`Self::last_session_id`], which is the id the BROKER last assigned. This one
    /// is the HOST's session id at the moment the runtime started, and its only consumer is
    /// [`is_live_at`]'s `ctx.sessionManager.getSessionId() !== currentSessionId` check (`:651`) —
    /// the detector for "the extension context was swapped under an in-flight task", which the
    /// generation counter alone does not catch.
    runtime_session_id: Mutex<Option<String>>,
}

impl ConnectSupervisor {
    fn params(&self) -> Option<Arc<ConnectParams>> {
        self.params.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn last_error(&self) -> Option<String> {
        self.last_error.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn last_session_id(&self) -> Option<String> {
        self.last_session_id.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Install (or, with `None`, clear) the reconnect timer, ABORTING whichever task it replaces —
    /// pi `clearReconnectTimer()` (`index.ts:510-516`). This is the cancellation path: a sleeping
    /// backoff task is killed here, it does not merely no-op afterwards.
    fn set_timer(&self, handle: Option<tokio::task::JoinHandle<()>>) {
        let previous = std::mem::replace(
            &mut *self.timer.lock().unwrap_or_else(|e| e.into_inner()),
            handle,
        );
        if let Some(previous) = previous {
            previous.abort();
        }
    }

    /// Release the timer slot WITHOUT aborting it — what the timer body itself calls on entry
    /// (`reconnectTimer = null` as the first statement of pi's callback, `index.ts:800`), since the
    /// handle in the slot at that moment is its own.
    fn release_timer(&self) {
        let _ = std::mem::take(&mut *self.timer.lock().unwrap_or_else(|e| e.into_inner()));
    }

    /// Whether a backoff task is currently armed (test/observability accessor).
    #[must_use]
    pub fn reconnect_armed(&self) -> bool {
        self.timer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|h| !h.is_finished())
    }

    /// The current backoff-ladder index (test/observability accessor).
    #[must_use]
    pub fn attempt(&self) -> u32 {
        self.attempt.load(Ordering::SeqCst)
    }

    /// Whether the session has torn down (test/observability accessor).
    #[must_use]
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    /// `const messageGeneration = runtimeGeneration` (`v0.10.1 index.ts:903`) — the stamp an
    /// in-flight task captures so it can tell, later, whether the runtime it started under is still
    /// the live one.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// pi `runtimeStarted` (`v0.10.1 index.ts:522,1253`) — false until the FIRST `SessionStart` and
    /// true forever after, which is what lets the local subagent relay deliver before any runtime
    /// generation exists while still fencing every delivery once one does.
    #[must_use]
    pub fn runtime_ever_started(&self) -> bool {
        self.runtime_ever_started.load(Ordering::SeqCst)
    }

    fn runtime_session_id(&self) -> Option<String> {
        self.runtime_session_id.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// `getLiveContext(ctx, generation)` (`v0.10.1 index.ts:646-659`, 14 lines):
///
/// ```text
/// if (disposed || shuttingDown || generation !== runtimeGeneration || !ctx) return null;
/// try {
///   if (currentSessionId && ctx.sessionManager.getSessionId() !== currentSessionId) return null;
///   void ctx.hasUI;
///   return ctx;
/// } catch { return null; }
/// ```
///
/// Upstream calls this at SIX points around a single inbound message, not once, because every
/// `await` in that path is a place the runtime can be replaced underneath the task. Rust makes the
/// hazard sharper, not softer: a future can be dropped at any `.await`, and `tokio::spawn`ed work
/// outlives the runtime that spawned it unless something fences it.
///
/// Mapping, term by term:
/// - `disposed || shuttingDown` → [`ConnectSupervisor::is_shutting_down`] (cyrup latches one flag
///   where pi has two; `shutdown` sets it and `begin_runtime` clears it, so the pair is covered);
/// - the `currentSessionId` mismatch → the id captured by [`begin_runtime`] vs the LIVE
///   `HostServices::session_id()`;
/// - `void ctx.hasUI` (a probe that a throwing context is caught) has no counterpart: cyrup's
///   `HostServices` accessors return values, not exceptions.
///
/// # [CYRUP-DELTA] — pi's `!ctx` limb is deliberately NOT mapped to "no host services"
///
/// `runtimeContext` is non-null for every running pi session, so `!ctx` means "the extension was
/// disposed". cyrup's [`crate::session_state::SharedIntercomState::host_services`] is an `Option`
/// for a different reason — a headless or degraded session legitimately has none — and the crate
/// already degrades through that state rather than treating it as dead: `send_incoming_message`
/// returns `false`, and `auto_reply_non_interactive` still tells the SENDER the peer is busy, which
/// is the one thing a headless session must keep doing. Folding `host_services().is_none()` into
/// this predicate would have silently converted "no human surface" into "drop the message before it
/// is even recorded", which is a regression pi does not have and ICOM-049 does not ask for. The
/// "not started yet" half of `!ctx` is covered by [`ConnectSupervisor::runtime_ever_started`], which callers
/// gate on exactly where upstream writes `runtimeStarted &&` (`v0.10.1 index.ts:877`).
#[must_use]
pub fn is_live_at(state: &SharedIntercomState, generation: u64) -> bool {
    let sup = &state.connect;
    if sup.shutting_down.load(Ordering::SeqCst) || sup.generation.load(Ordering::SeqCst) != generation
    {
        return false;
    }
    // `if (currentSessionId && ctx.sessionManager.getSessionId() !== currentSessionId)` — the guard
    // is skipped entirely while `currentSessionId` is null (JS `&&` short-circuit), which is the
    // pre-`startSessionRuntime` window. It is also skipped when there is no live `HostServices` to
    // read the current id off, per the delta above.
    match (sup.runtime_session_id(), state.host_services()) {
        (Some(captured), Some(services)) => {
            services.session_id().as_deref() == Some(captured.as_str())
        }
        _ => true,
    }
}

/// `startSessionRuntime` (`index.ts:926-951`), connection half: drop any previous client, clear the
/// shutdown flags, bump the generation (invalidating any in-flight reconnect from the previous
/// session), reset the backoff ladder and stash the params every later attempt rebuilds its
/// registration from.
pub fn begin_runtime(state: &Arc<SharedIntercomState>, params: ConnectParams) {
    let sup = &state.connect;
    if let Some(previous) = state.client() {
        state.set_client(None);
        previous.disconnect();
    }
    sup.shutting_down.store(false, Ordering::SeqCst);
    sup.started.store(true, Ordering::SeqCst);
    // `runtimeStarted = true` (`v0.10.1 index.ts:1253`) — latched, never cleared.
    sup.runtime_ever_started.store(true, Ordering::SeqCst);
    // ICOM-056 / `v0.12.0 index.ts:1577,1582`: settle everything this runtime change orphans BEFORE
    // the generation bump. Settling after it would leave `is_live_at` inside the settle path looking
    // at the NEW generation, and every trace would be silently dropped instead of answered.
    crate::outbox::fail_pending_outbox_requests(
        state,
        sup.generation(),
        crate::outbox::OutboxResultCode::SessionEnded,
        "Session replaced",
    );
    // The dedupe window IS the runtime, so a requestId replayed across a restart is legal.
    state.clear_outbox_request_ids();
    sup.generation.fetch_add(1, Ordering::SeqCst);
    sup.attempt.store(0, Ordering::SeqCst);
    sup.set_timer(None);
    *sup.last_error.lock().unwrap_or_else(|e| e.into_inner()) = None;
    // `currentSessionId = ctx.sessionManager.getSessionId()` (`v0.10.1 index.ts:1266`) — captured
    // here, read only by `is_live_at`.
    *sup.runtime_session_id.lock().unwrap_or_else(|e| e.into_inner()) =
        state.host_services().and_then(|services| services.session_id());
    *sup.params.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(params));
}

/// The `session_shutdown` teardown (`index.ts:1060-1064`), connection half: latch `shuttingDown`/
/// `disposed`, bump the generation and ABORT the pending backoff task. After this, both
/// [`schedule_reconnect`] and [`ensure_connected`] are permanently refused for this runtime — a
/// deliberate shutdown never reconnects.
pub fn shutdown(state: &Arc<SharedIntercomState>) {
    let sup = &state.connect;
    sup.shutting_down.store(true, Ordering::SeqCst);
    sup.started.store(false, Ordering::SeqCst);
    // ICOM-056 / `v0.12.0 index.ts:1731`: same ordering rule as `begin_runtime` — settle first, bump
    // second.
    crate::outbox::fail_pending_outbox_requests(
        state,
        sup.generation(),
        crate::outbox::OutboxResultCode::SessionEnded,
        "Session shutting down",
    );
    sup.generation.fetch_add(1, Ordering::SeqCst);
    sup.set_timer(None);
    *sup.runtime_session_id.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *sup.params.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// `scheduleReconnect` (`index.ts:794-809`): arm ONE backoff task at the current ladder delay.
///
/// Early-returns (all pi's, `index.ts:795`) when the runtime is shutting down or never started,
/// when a timer is already armed (the anti-thundering-herd guard), or when an attempt is already in
/// flight. The task itself re-checks the generation + the shutdown flags after its sleep, then
/// advances the ladder and runs a `Background` attempt whose own failure re-arms the next rung.
pub fn schedule_reconnect(state: &Arc<SharedIntercomState>) {
    let sup = &state.connect;
    if sup.shutting_down.load(Ordering::SeqCst) || !sup.started.load(Ordering::SeqCst) {
        return;
    }
    if sup.connecting.load(Ordering::SeqCst) || sup.reconnect_armed() {
        return;
    }
    let generation = sup.generation.load(Ordering::SeqCst);
    let delay = reconnect_delay_ms(sup.attempt.load(Ordering::SeqCst));
    // Weak, so a sleeping backoff task never keeps a torn-down session's state alive.
    let weak = Arc::downgrade(state);
    let handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(delay)).await;
        let Some(state) = weak.upgrade() else {
            return;
        };
        // `reconnectTimer = null` first (index.ts:800) so the failure below can arm the next rung.
        state.connect.release_timer();
        if state.connect.generation.load(Ordering::SeqCst) != generation
            || state.connect.shutting_down.load(Ordering::SeqCst)
            || !state.connect.started.load(Ordering::SeqCst)
        {
            return;
        }
        state.connect.attempt.fetch_add(1, Ordering::SeqCst);
        // A failure here has already queued the next retry (index.ts:806-808).
        let _ = ensure_connected(&state, ConnectReason::Background).await;
    });
    sup.set_timer(Some(handle));
}

/// `ensureConnected(reason)` (`index.ts:810-861`): return the live client, or run exactly one
/// deduplicated connect attempt (spawn the broker if needed → register → attach the inbound loop).
///
/// # Errors
/// [`IntercomError::Client`] when intercom is disabled, when the session is shutting down, when the
/// runtime has not started (no `SessionStart` yet), or when the attempt itself failed;
/// [`IntercomError::Broker`] when the broker could not be spawned.
pub async fn ensure_connected(
    state: &Arc<SharedIntercomState>,
    reason: ConnectReason,
) -> Result<Arc<IntercomClient>> {
    if !state.config.enabled {
        return Err(IntercomError::Client("Intercom disabled".to_string()));
    }
    let sup = &state.connect;
    if sup.shutting_down.load(Ordering::SeqCst) {
        return Err(IntercomError::Client("Intercom shutting down".to_string()));
    }
    if let Some(client) = state.client()
        && client.is_connected()
    {
        return Ok(client);
    }
    let Some(params) = sup.params() else {
        return Err(IntercomError::Client("Intercom runtime not initialized".to_string()));
    };
    let generation = sup.generation.load(Ordering::SeqCst);
    // `clearReconnectTimer()` (index.ts:824): we are about to attempt now, so a pending rung is
    // redundant. (Called from the timer body this is a no-op — it already released its own slot.)
    sup.set_timer(None);

    let epoch_at_start = sup.epoch.load(Ordering::SeqCst);
    let _gate = sup.gate.lock().await;
    // Someone else may have connected — or failed — while we queued behind the gate.
    if let Some(client) = state.client()
        && client.is_connected()
    {
        return Ok(client);
    }
    if sup.epoch.load(Ordering::SeqCst) != epoch_at_start {
        // A concurrent attempt completed and left us unconnected: adopt its failure, exactly as pi's
        // shared `reconnectPromise` hands every queued caller the same rejection, instead of
        // stacking a second connect per caller.
        return Err(IntercomError::Client(
            sup.last_error().unwrap_or_else(|| "intercom connect failed".to_string()),
        ));
    }
    if sup.shutting_down.load(Ordering::SeqCst) || sup.generation.load(Ordering::SeqCst) != generation {
        return Err(IntercomError::Client("Intercom shutting down".to_string()));
    }

    sup.connecting.store(true, Ordering::SeqCst);
    let result = connect_once(state, &params, generation).await;
    sup.connecting.store(false, Ordering::SeqCst);
    sup.epoch.fetch_add(1, Ordering::SeqCst);

    match result {
        Ok(client) => {
            *sup.last_error.lock().unwrap_or_else(|e| e.into_inner()) = None;
            // `reconnectAttempt = 0` (index.ts:841): the ladder restarts from 1 s next time.
            sup.attempt.store(0, Ordering::SeqCst);
            Ok(client)
        }
        Err(e) => {
            *sup.last_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(e.to_string());
            // Only a background attempt re-arms the ladder (index.ts:847-849); a tool/overlay
            // failure surfaces to its caller, and startup arms it from its own catch.
            if reason == ConnectReason::Background {
                schedule_reconnect(state);
            }
            Err(e)
        }
    }
}

/// One attempt: ensure the broker is up, register, and — if the runtime is still the one we started
/// for — publish the client and attach the inbound loop (`index.ts:829-846`).
async fn connect_once(
    state: &Arc<SharedIntercomState>,
    params: &ConnectParams,
    generation: u64,
) -> Result<Arc<IntercomClient>> {
    ensure_broker(&params.agent_dir).await?;
    // `const target = getBrokerConnectTarget();` — the socket path on POSIX, the
    // `\\.\pipe\cyrup-intercom-<agent dir>` name on Windows, or the loopback-TCP endpoint read back
    // out of `broker.port.json` under the Windows-only opt-in (`broker/paths.ts:76-105`). This
    // replaces a direct `paths::broker_socket_path(...)` read, which hard-coded the POSIX arm — the
    // same fix `broker/lifecycle.rs:118` already made on the listen side. It matters because the
    // `ensure_broker` call above confirms the broker is connectable through
    // `target::broker_connect_target` (`transport/spawn.rs:305,378`) and returns no target, so this
    // must re-resolve the SAME way or a session dials an endpoint no broker is listening on.
    let target = broker_connect_target(&params.agent_dir)?;
    let registration = build_registration(state, params);
    // Register under THIS SESSION'S OWN id — pi `await nextClient.connect(buildRegistration(),
    // currentSessionId)` (`index.ts:833`), where `currentSessionId = ctx.sessionManager
    // .getSessionId()` (`index.ts:945`). The live `HostServices::session_id()` is cyrup's
    // `sessionManager.getSessionId()`.
    //
    // This used to read `CYRUP_INTERCOM_SESSION_ID` from the process env instead, which has ZERO
    // writers anywhere in the workspace, so the offered id was always `None` on a first connect and
    // the broker minted a random UUID (`broker/mod.rs:319-320`, `broker.ts:346-352`). Two
    // consequences, both user-visible:
    //   * a session's broker identity was unrelated to its agent session id, so the id column of
    //     `intercom{list}` / the `/intercom` picker showed a UUID that names nothing else in cyrup
    //     and NO peer could ever address a session by the session id it actually has;
    //   * [`build_registration`] derives the presence alias `subagent-chat-<id[0:8]>` from the REAL
    //     `HostServices::session_id()`, so the alias' 8 hex chars matched no listed id — the alias
    //     is supposed to BE the readable form of the id it registers under.
    // Reading the env var was also actively wrong in pi's own terms: pi *publishes*
    // `PI_INTERCOM_SESSION_ID` (`publishIntercomSessionId`, `index.ts:612-614,946`) for CHILDREN to
    // inherit and read back as their SUPERVISOR's id (`readChildOrchestratorMetadata`,
    // `index.ts:86-87`) — it never re-reads it as its own registration id. A child that inherited it
    // would have re-registered under its parent's id and taken the parent's broker slot over
    // (`handle_register` identity takeover).
    //
    // `last_session_id()` stays as the fallback for a session with no live `HostServices` bound
    // (headless/degraded): a reconnect then still re-offers the identity the broker previously
    // assigned rather than appearing as a second, stale participant.
    //
    // `resolveConfiguredIntercomSessionId` (`v0.10.1 index.ts:434-436`) sits IN FRONT of that:
    //
    //   return process.env[STABLE_INTERCOM_SESSION_ID_ENV]?.trim() || config.stableId || piSessionId;
    //
    // — an explicitly configured stable id wins over the host's per-process one, so a restarted
    // worker keeps the address its peers already hold instead of orphaning every stored target.
    let session_id = configured_stable_session_id(state)
        .or_else(|| {
            state
                .host_services()
                .and_then(|services| services.session_id())
                .map(|id| id.trim().to_string())
                .filter(|id| !id.is_empty())
        })
        .or_else(|| state.connect.last_session_id());
    let client = Arc::new(IntercomClient::connect_target(&target, registration, session_id).await?);
    if state.connect.shutting_down.load(Ordering::SeqCst)
        || state.connect.generation.load(Ordering::SeqCst) != generation
    {
        // `Intercom runtime no longer active` (index.ts:837-840): never leave a registered client
        // behind for a session that has moved on.
        client.disconnect();
        return Err(IntercomError::Client("Intercom runtime no longer active".to_string()));
    }
    *state.connect.last_session_id.lock().unwrap_or_else(|e| e.into_inner()) = client.session_id();
    state.set_client(Some(client.clone()));
    spawn_inbound_loop(state.clone(), client.clone());
    Ok(client)
}

/// The client `disconnected` handler (`index.ts:779-789`): fail any in-flight outbound ask, drop the
/// dead client, and arm the reconnect ladder — unless this session is deliberately tearing down.
///
/// The identity check mirrors pi's `if (client !== nextClient) return;`: a late `Disconnected` from
/// a SUPERSEDED connection must not clear the client a newer attempt already installed.
pub fn handle_disconnect(state: &Arc<SharedIntercomState>, client: &Arc<IntercomClient>, reason: &str) {
    match state.client() {
        Some(live) if Arc::ptr_eq(&live, client) => {}
        _ => return,
    }
    // Reject BEFORE nulling the client (index.ts:783-784) so a blocking ask fails on the drop edge
    // instead of hanging until its 10-minute ask timeout, across a reconnect that can never carry
    // the answer (the broker has no mailbox — see the module docs).
    state.waiter.fail_pending(&format!("Disconnected while waiting for reply: {reason}"));
    state.set_client(None);
    let sup = &state.connect;
    if sup.shutting_down.load(Ordering::SeqCst) || !sup.started.load(Ordering::SeqCst) {
        return;
    }
    sup.set_timer(None);
    schedule_reconnect(state);
}

/// Build this session's broker registration (pi `buildRegistration`, `index.ts:583-604`).
///
/// The presence name is:
///   1. a subagent child's own deterministic label (`metadata.session_name`), else
///   2. (a top-level/plain orchestrator) the presence name derived from the LIVE `HostServices` —
///      `presence_name(session_name, session_id)` — matching pi `buildPresenceIdentity`
///      (`pi-intercom/index.ts:387-389`). This is REQUIRED so a spawned child can address this
///      orchestrator: the child's `CYRUP_SUBAGENT_ORCHESTRATOR_TARGET` is
///      `orchestrator_presence_target(session_name, session_id)` over the SAME session id/name, so
///      the two independently-produced strings match at the broker.
///   3. else the `CYRUP_INTERCOM_SESSION_ID`-derived alias (refined post-register).
#[must_use]
pub fn build_registration(state: &SharedIntercomState, params: &ConnectParams) -> SessionRegistration {
    let identity = presence_identity(state, params.metadata.as_ref());
    SessionRegistration {
        // `{ ...identity }` (`v0.10.1 index.ts:772-774`) — name AND the alias flag.
        runtime_fallback_alias: identity.name.as_ref().map(|_| identity.runtime_fallback_alias),
        name: identity.name,
        cwd: state.cwd.to_string_lossy().to_string(),
        model: params.model.clone().unwrap_or_else(|| "cyrup".to_string()),
        pid: std::process::id().into(),
        started_at: now_ms().into(),
        last_activity: now_ms().into(),
        // `buildRegistration` sets `status: currentStatus()` (`v0.10.1 index.ts:772-780`), NOT the
        // raw configured suffix. `build_registration` is rebuilt on EVERY reconnect rung
        // (`connect_once`), so reading `config.status` here re-registered a session that dropped
        // mid-tool-call as having no lifecycle status at all.
        status: Some(state.current_status()),
        extra: Default::default(),
    }
}

/// `resolveConfiguredIntercomSessionId` (`v0.10.1 index.ts:434-436`), the env/config half:
/// `process.env[STABLE_INTERCOM_SESSION_ID_ENV]?.trim() || config.stableId`.
///
/// JS `||` is falsy-based, so a blank env value falls through to `config.stableId`; `config.stableId`
/// is already trimmed non-empty by `parse_config` (`v0.10.1 config.ts:141-150`).
#[must_use]
pub fn configured_stable_session_id(state: &SharedIntercomState) -> Option<String> {
    std::env::var(crate::identity::ENV_INTERCOM_STABLE_ID)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .or_else(|| state.config.stable_id.clone())
}

/// `buildPresenceIdentity(pi, sessionId).name` (`v0.10.1 index.ts:427-433`) — recomputed from the
/// LIVE host every time, never a snapshot.
///
/// Extracted out of [`build_registration`] so the registration and every later presence re-sync
/// produce the SAME string; upstream gets that for free because both go through
/// `buildPresenceIdentity`. The `metadata.session_name` first tier is cyrup's: a subagent child
/// registers under the deterministic label its launcher minted, and re-deriving the name from the
/// host would change the address the parent's `CYRUP_SUBAGENT_ORCHESTRATOR_TARGET` was built
/// against.
#[must_use]
pub fn presence_identity_name(
    state: &SharedIntercomState,
    metadata: Option<&crate::identity::ChildOrchestratorMetadata>,
) -> Option<String> {
    presence_identity(state, metadata).name
}

/// `buildPresenceIdentity(pi, sessionId)` (`v0.10.1 index.ts:427-433`) — `{ name,
/// runtimeFallbackAlias }` as ONE value, because upstream spreads it as one
/// (`{ ...identity, status, ...contextUsage }`, `:815`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PresenceIdentity {
    /// The presence name this session registers/advertises under.
    pub name: Option<String>,
    /// `!sessionName?.trim()` — true only when [`Self::name`] is a synthesized alias rather than a
    /// name the user or launcher chose.
    pub runtime_fallback_alias: bool,
}

/// [`PresenceIdentity`] for this session — see [`presence_identity_name`].
#[must_use]
pub fn presence_identity(
    state: &SharedIntercomState,
    metadata: Option<&crate::identity::ChildOrchestratorMetadata>,
) -> PresenceIdentity {
    // A launcher-assigned child label is a CHOSEN name, not a synthesized one, so it clears the
    // flag exactly as a `/name` would.
    if let Some(name) = metadata.and_then(|m| m.session_name.clone()) {
        return PresenceIdentity { name: Some(name), runtime_fallback_alias: false };
    }
    if let Some(services) = state.host_services()
        && let Some(id) = services.session_id().filter(|id| !id.is_empty())
    {
        let session_name = services.session_name();
        return PresenceIdentity {
            name: Some(presence_name(session_name.as_deref(), &id)),
            runtime_fallback_alias: session_name.is_none_or(|n| n.trim().is_empty()),
        };
    }
    // No live host: the id-derived alias is by construction synthesized.
    match std::env::var(ENV_INTERCOM_SESSION_ID).ok() {
        Some(id) => {
            PresenceIdentity { name: Some(presence_name(None, &id)), runtime_fallback_alias: true }
        }
        None => PresenceIdentity::default(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
    use super::*;
    use crate::config::IntercomConfig;

    fn state() -> Arc<SharedIntercomState> {
        Arc::new(SharedIntercomState::new(
            IntercomConfig::default(),
            600_000,
            std::path::PathBuf::from("/w"),
        ))
    }

    /// Params whose agent dir can never be created (its parent is a regular FILE), so every attempt
    /// fails immediately inside `ensure_broker` — no broker is ever spawned by these unit tests.
    fn unreachable_params(dir: &tempfile::TempDir) -> ConnectParams {
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, b"not a directory").unwrap();
        ConnectParams {
            agent_dir: blocked.join("agent"),
            metadata: None,
            model: Some("test-model".to_string()),
        }
    }

    fn error_text<T>(result: Result<T>) -> String {
        result.err().map(|e| e.to_string()).unwrap_or_else(|| "<ok>".to_string())
    }

    /// pi `getReconnectDelayMs` (`index.ts:564-567`) verbatim, including the saturating ceiling that
    /// is what bounds the ladder in rate.
    #[test]
    fn backoff_ladder_matches_pi_and_saturates_at_30s() {
        assert_eq!(reconnect_delay_ms(0), 1000);
        assert_eq!(reconnect_delay_ms(1), 2000);
        assert_eq!(reconnect_delay_ms(2), 5000);
        assert_eq!(reconnect_delay_ms(3), 10_000);
        assert_eq!(reconnect_delay_ms(4), 30_000);
        assert_eq!(reconnect_delay_ms(5), 30_000);
        assert_eq!(reconnect_delay_ms(u32::MAX), 30_000);
    }

    /// Before `SessionStart` there is no runtime: `schedule_reconnect` must not arm anything and
    /// `ensure_connected` must refuse rather than connect with a half-built registration.
    #[tokio::test]
    async fn no_runtime_means_no_ladder_and_no_connect() {
        let state = state();
        schedule_reconnect(&state);
        assert!(!state.connect.reconnect_armed());
        let err = error_text(ensure_connected(&state, ConnectReason::Tool).await);
        assert!(err.contains("not initialized"), "{err}");
    }

    /// Hazard 3, guard 1: a deliberate shutdown must refuse every later connect AND leave no armed
    /// backoff task behind.
    #[tokio::test]
    async fn shutdown_refuses_connects_and_disarms_the_ladder() {
        let dir = tempfile::tempdir().unwrap();
        let state = state();
        begin_runtime(&state, unreachable_params(&dir));
        schedule_reconnect(&state);
        assert!(state.connect.reconnect_armed(), "a started runtime arms the ladder");

        shutdown(&state);
        assert!(!state.connect.reconnect_armed(), "shutdown aborts the pending backoff task");

        // And it stays disarmed: a disconnect edge after shutdown is a no-op.
        schedule_reconnect(&state);
        assert!(!state.connect.reconnect_armed());
        let err = error_text(ensure_connected(&state, ConnectReason::Background).await);
        assert!(err.contains("shutting down"), "{err}");
        // The refusal above must not have armed anything either.
        assert!(!state.connect.reconnect_armed());
    }

    /// Hazard 3, anti-thundering-herd: N disconnect/relay edges in a row arm ONE task, not N.
    #[tokio::test]
    async fn scheduling_is_idempotent_while_a_rung_is_armed() {
        let dir = tempfile::tempdir().unwrap();
        let state = state();
        begin_runtime(&state, unreachable_params(&dir));
        schedule_reconnect(&state);
        let attempt_before = state.connect.attempt();
        for _ in 0..10 {
            schedule_reconnect(&state);
        }
        assert!(state.connect.reconnect_armed());
        assert_eq!(state.connect.attempt(), attempt_before, "no rung is consumed by re-scheduling");
        shutdown(&state);
    }

    /// Hazard 3, the busy-loop assertion: a failing rung WAITS its backoff. The agent dir can never
    /// be created, so each attempt fails within microseconds — an unbounded immediate-retry
    /// reconnect would therefore have burned thousands of attempts inside the first 300 ms.
    /// Instead: 0 attempts at 300 ms, exactly 1 past 1000 ms (rung 0), and still exactly 1 at
    /// 2.0 s because rung 1 waits a further 2000 ms.
    ///
    /// ICOM-025 — driven on a PAUSED clock (`start_paused = true` + `tokio::time::advance`) rather
    /// than a real one. The wall-clock version gave 300 ms of slack over a 1000 ms timer plus a
    /// spawn hop plus a filesystem-failing `ensure_broker`, which is intermittent red on a loaded
    /// tree and completes in ~2 s even when green; the standard reaction (widen the sleep) hides
    /// the very backoff regression this is the only guard for. `advance` also yields to the
    /// scheduler, so the awakened rung actually runs before each assertion.
    #[tokio::test(start_paused = true)]
    async fn a_failing_rung_waits_its_backoff_instead_of_busy_looping() {
        let dir = tempfile::tempdir().unwrap();
        let state = state();
        begin_runtime(&state, unreachable_params(&dir));
        assert_eq!(state.connect.attempt(), 0);
        schedule_reconnect(&state);

        tokio::time::advance(Duration::from_millis(300)).await;
        tokio::task::yield_now().await;
        assert_eq!(state.connect.attempt(), 0, "rung 0 must not fire before its 1000ms backoff");

        tokio::time::advance(Duration::from_millis(1000)).await;
        tokio::task::yield_now().await;
        assert_eq!(state.connect.attempt(), 1, "rung 0 fired exactly once");
        assert!(state.connect.reconnect_armed(), "its failure armed rung 1");

        tokio::time::advance(Duration::from_millis(700)).await;
        tokio::task::yield_now().await;
        assert_eq!(state.connect.attempt(), 1, "rung 1 backs off 2000ms; it does not retry immediately");
        shutdown(&state);
    }

    /// `begin_runtime` resets the ladder (pi `reconnectAttempt = 0`, `index.ts:936`) so a new
    /// session does not inherit the previous one's 30 s ceiling.
    #[tokio::test]
    async fn begin_runtime_resets_the_ladder() {
        let dir = tempfile::tempdir().unwrap();
        let state = state();
        begin_runtime(&state, unreachable_params(&dir));
        state.connect.attempt.store(4, Ordering::SeqCst);
        begin_runtime(&state, unreachable_params(&dir));
        assert_eq!(state.connect.attempt(), 0);
        assert!(!state.connect.reconnect_armed());
        shutdown(&state);
    }
}
