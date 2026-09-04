//! The dispatch engine (arch-08 §6.1): subscription-gated, deterministic load-order, block/mutate
//! chaining. Holds extensions in load order and ORs their current subscription bitsets on demand so
//! an event with zero subscribers returns after a handful of `u64` tests — no serialization, no
//! boundary crossing (R-08-034 / R-ARCH-EXT-014; the aggregate is recomputed rather than frozen at
//! load so a late `subscribe` is honoured — EXT-058). Every guest call is fault-contained (R-08-036): a trap, OOM, epoch
//! timeout, or panic is logged and never crashes the host. On most kinds the handler is then
//! SKIPPED and the chain continues (fail OPEN); on the fail-CLOSED kinds
//! ([`EventKind::fails_closed`] — `tool_call`, the permission seam) the fault BLOCKS the action
//! instead, matching Pi's uncaught `emitToolCall` (EXT-001).

use crate::contract::{HandledValue, HookOutcome, Reduced};
use crate::error::ExtError;
use crate::event::{EventKind, HostEvent, Subscriptions};
use crate::extension::Extension;
use cyrup_core::CancelToken;
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// The per-handler invocation budget for the native path — the in-process analog of the wasm epoch
/// deadline (R-ARCH-EXT-012). A cooperatively-yielding runaway handler is preempted and skipped.
const DEFAULT_INVOKE_BUDGET: Duration = Duration::from_secs(5);

/// A contained extension fault, surfaced to registered error listeners (Pi `ExtensionError`,
/// types.ts:1609; `extensionPath`/`event`/`error`). The host turns each contained fault into one of
/// these for UI surfacing / diagnostics (R-08-036) — whether the fault was skipped (fail open) or
/// blocked the action (fail closed).
#[derive(Clone, Debug)]
pub struct ExtensionError {
    pub extension: cyrup_core::ExtensionId,
    /// The event kind during which the fault occurred (`"tool_call"`, `"agent_start"`, …).
    pub event: &'static str,
    pub error: String,
}

/// A registered error listener (Pi `onError`). `Send + Sync` so it can be shared across the host.
pub type ErrorListener = Arc<dyn Fn(&ExtensionError) + Send + Sync>;

/// The subscription-gated, load-ordered dispatcher (arch-08 §3.1 / §6.1).
pub struct Dispatcher {
    inner: RwLock<DispatchInner>,
    budget: Duration,
    /// Error listeners notified when a guest fault is contained (Pi `onError`, R-08-036).
    error_listeners: RwLock<Vec<ErrorListener>>,
    /// The inter-extension bus fan-out (EXT-034), held WEAKLY — the fan-out owns this dispatcher
    /// (it needs [`Self::report_external`]), so a strong edge back would be a reference cycle.
    /// `None` on a bare dispatcher built outside an `ExtensionHost` (tests), in which case every
    /// drain is a no-op and nothing can be queued anyway.
    bus_drain: RwLock<Option<std::sync::Weak<dyn crate::bus::BusDrain>>>,
    /// Latch so the "lock poisoned" diagnostic is emitted ONCE rather than on every dispatch
    /// (poisoning is permanent for the lock's lifetime). See [`Self::note_poisoned`].
    poison_reported: std::sync::atomic::AtomicBool,
}

#[derive(Default)]
struct DispatchInner {
    /// Extensions in deterministic LOAD ORDER (R-08-004).
    exts: Vec<Arc<dyn Extension>>,
}

impl Default for Dispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Dispatcher {
    pub fn new() -> Self {
        Self::with_budget(DEFAULT_INVOKE_BUDGET)
    }

    pub fn with_budget(budget: Duration) -> Self {
        Self {
            inner: RwLock::new(DispatchInner::default()),
            budget,
            error_listeners: RwLock::new(Vec::new()),
            bus_drain: RwLock::new(None),
            poison_reported: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Attach the inter-extension bus fan-out (EXT-034). Called once by
    /// [`crate::ExtensionHost::new`]; the last handle wins.
    pub(crate) fn set_bus_drain(&self, drain: std::sync::Weak<dyn crate::bus::BusDrain>) {
        if let Ok(mut g) = self.bus_drain.write() {
            *g = Some(drain);
        }
    }

    /// Fan out anything a just-finished handler chain put on the inter-extension bus (EXT-034).
    ///
    /// pi needs no such call: `createEventBus().emit` runs every listener inline at the emit
    /// (`pi/packages/coding-agent/src/core/event-bus.ts:12-32` @v0.83.0), so a `pi.events.emit` from
    /// inside an event handler is delivered before the handler returns. cyrup has to defer past the
    /// emitting guest's own store, which makes THIS — after the subscriber loop, with no guest store
    /// held — the equivalent point. It is a no-op when a drain is already in progress or when no
    /// host attached a fan-out.
    ///
    /// `exclude` is the reason this takes an argument at all. A `Some` exclusion means a guest is
    /// SUSPENDED inside one of its own host imports right now (the `provider-stream.on-payload`
    /// path — see [`Self::dispatch_block_mutate_excluding`]), holding its single-instance
    /// `tokio::Mutex` store guard. Delivering a bus event to that guest would await the guard it
    /// already holds and HANG — not fail, hang, because a re-entrant `tokio::Mutex::lock` has no
    /// deadlock detection. So the drain is skipped entirely on those seams; the events stay queued
    /// and go out at the next seam that is not inside a guest, which is the same deferral the queue
    /// exists for. Upstream has no equivalent hazard: pi's runner is one JS process and a
    /// re-entered handler is an ordinary nested call.
    async fn drain_bus(&self, cancel: &CancelToken, exclude: Option<&cyrup_core::ExtensionId>) {
        if exclude.is_some() {
            return;
        }
        let drain = self
            .bus_drain
            .read()
            .ok()
            .and_then(|g| g.as_ref().and_then(|w| w.upgrade()));
        if let Some(drain) = drain {
            drain.drain_bus(cancel).await;
        }
    }

    /// Register an error listener (Pi `onError`, types.ts:1609): notified with a typed
    /// [`ExtensionError`] each time a guest fault is contained (R-08-036), on both the fail-open
    /// (handler skipped) and fail-closed (action blocked) dispositions.
    pub fn add_error_listener(&self, listener: ErrorListener) {
        if let Ok(mut g) = self.error_listeners.write() {
            g.push(listener);
        }
    }

    /// Surface a contained fault to every registered listener + tracing (never propagates). Fires
    /// for BOTH dispositions — the skipped fail-open case and the blocking fail-closed one — so a
    /// gate that denied because it faulted is never invisible.
    fn report(&self, kind: EventKind, id: &cyrup_core::ExtensionId, err: &ExtError) {
        // EXT-029. A run abort is NOT an extension fault. cyrup hands every handler a child of the
        // run's cancel token (`self.hooks.before_tool_call(ctx, self.cancel.child())`,
        // `cyrup-agent/src/agent.rs:1009`), so pressing Esc mid-dispatch surfaces
        // `ExtError::Cancelled` out of a perfectly healthy extension. Reporting it drives
        // `onError` and — since EXT-S03 wired that channel into the transcript — writes
        // `Extension "<id>" error: cancelled` into the interactive UI, blaming the extension for
        // the user's own abort.
        //
        // CYRUP-DELTA: pi has no counterpart to suppress. `emitToolCall`
        // (pi/packages/coding-agent/src/core/extensions/runner.ts:932-953 @v0.83.0) takes no
        // signal and has no cancellation race at all; the abort path returns
        // `createErrorToolResult("Operation aborted")` before the block branch
        // (packages/agent/src/agent-loop.ts:629-635). The cancellation is a cyrup-original
        // mechanism, so the rule that keeps it invisible to `onError` is cyrup-original too.
        if matches!(err, ExtError::Cancelled) {
            tracing::debug!(
                extension = %id,
                event = kind.name(),
                "extension call cancelled by run abort (not reported as an extension fault)"
            );
            return;
        }
        let event = kind.name();
        let disposition = if kind.fails_closed() {
            "blocking the action"
        } else {
            "skipping the handler"
        };
        tracing::warn!(
            extension = %id,
            event,
            error = %err,
            disposition,
            "extension call fault contained"
        );
        let payload = ExtensionError {
            extension: id.clone(),
            event,
            error: err.to_string(),
        };
        if let Ok(g) = self.error_listeners.read() {
            for l in g.iter() {
                l(&payload);
            }
        }
    }

    /// Surface a fault that did NOT come from an event dispatch — today the inter-extension bus
    /// (EXT-057). The bus has its own fan-out loop outside [`Self::dispatch_notify`] and friends,
    /// so a trapping `bus-deliver` or a queue dropped at the round bound had no way onto the
    /// `onError` channel `App::show_extension_error` drains. pi's bus surfaces its handler faults
    /// too (`catch (err) { console.error(\`Event handler error (${channel}):\`, err); }`,
    /// `core/event-bus.ts` @v0.83.0).
    pub fn report_external(&self, payload: ExtensionError) {
        if let Ok(g) = self.error_listeners.read() {
            for l in g.iter() {
                l(&payload);
            }
        }
    }

    /// Add an extension at the end of the load order.
    pub fn add(&self, ext: Arc<dyn Extension>) -> Result<(), ExtError> {
        let mut g = self.lock_write()?;
        g.exts.push(ext);
        Ok(())
    }

    /// Drop every loaded extension (hot-reload, R-08-005). After this the dispatcher has no
    /// subscribers; the loader re-adds the freshly-discovered set.
    pub fn clear(&self) -> Result<(), ExtError> {
        let mut g = self.lock_write()?;
        *g = DispatchInner::default();
        Ok(())
    }

    /// The union of every loaded extension's CURRENT subscription bitset (EXT-058).
    ///
    /// Computed on demand rather than folded once at [`Self::add`]. The load-time aggregate was a
    /// process-wide snapshot, so a guest that called the `subscribe` import from a live handler
    /// (which pi's `api.on` permits — `extensions/loader.ts:252-258` @v0.83.0, re-read at every
    /// emit) was gated out by [`Self::no_subscribers`] before its own
    /// [`Extension::subscriptions`] was ever consulted, with no log and no error. The cost is one
    /// `u64` OR per loaded extension — still O(extensions) with no serialization and no boundary
    /// crossing, which is what R-08-034 / R-ARCH-EXT-014 actually require.
    fn aggregate(&self) -> Subscriptions {
        match self.lock_read() {
            Ok(g) => g.exts.iter().fold(Subscriptions::empty(), |acc, e| {
                acc.union(e.subscriptions())
            }),
            Err(_) => {
                self.note_poisoned();
                // poisoned => behave as if nothing subscribed (never crash)
                Subscriptions::empty()
            }
        }
    }

    /// Report — ONCE — that the dispatcher's lock is poisoned and every extension event is
    /// therefore being dropped.
    ///
    /// CYRUP-DELTA: pi has no counterpart; JS has no lock poisoning and `runner.ts:806`'s handler
    /// lookup cannot fail. The fail-soft fallback itself is a legitimate cyrup-original mechanism
    /// (`bus.rs`/`facade.rs` degrade the same way), but poisoning is PERMANENT for the lock's
    /// lifetime, so a single panic taken under the write guard turns the whole extension event
    /// system into a silent, total no-op — indistinguishable from "nobody subscribed". The latch
    /// keeps the diagnostic off the per-dispatch hot path while making the state observable.
    fn note_poisoned(&self) {
        if !self
            .poison_reported
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            tracing::error!(
                "extension dispatcher lock poisoned; no extension events will be delivered for \
                 the rest of this process"
            );
        }
    }

    /// Whether the poisoned-lock diagnostic has already been latched (tests).
    #[cfg(test)]
    pub(crate) fn poison_reported(&self) -> bool {
        self.poison_reported
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// The cheap gate (R-08-034): true iff NO loaded extension subscribes to `kind`. Callers skip
    /// all serialization when this is true.
    pub fn no_subscribers(&self, kind: EventKind) -> bool {
        !self.aggregate().contains(kind)
    }

    /// Number of loaded extensions (diagnostics/tests).
    pub fn len(&self) -> usize {
        match self.lock_read() {
            Ok(g) => g.exts.len(),
            Err(_) => {
                self.note_poisoned();
                0
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn subscribers_for(&self, kind: EventKind) -> Vec<Arc<dyn Extension>> {
        match self.lock_read() {
            Ok(g) => g
                .exts
                .iter()
                .filter(|e| e.subscriptions().contains(kind))
                .cloned()
                .collect(),
            Err(_) => {
                self.note_poisoned();
                Vec::new()
            }
        }
    }

    /// Notify-only dispatch (return ignored, R-08-009). Subscription-gated; awaited in load order.
    pub async fn dispatch_notify(&self, ev: &HostEvent, cancel: &CancelToken) {
        self.dispatch_notify_excluding(ev, cancel, None).await
    }

    /// [`Self::dispatch_notify`] with one extension held out (EXT-052) — see
    /// [`Self::dispatch_block_mutate_excluding`] for why exclusion exists at all.
    pub async fn dispatch_notify_excluding(
        &self,
        ev: &HostEvent,
        cancel: &CancelToken,
        exclude: Option<&cyrup_core::ExtensionId>,
    ) {
        let kind = ev.kind();
        // EXT-034: the subscription gate (R-08-034) skips the HANDLER loop, never the drain. The
        // gate answers "does anyone subscribe to THIS event kind", which has nothing to do with
        // whether a bus event is waiting: an extension that only listens on `pi.events` declares no
        // event subscriptions at all, so gating the drain on it stranded the queue forever — pi
        // cannot stall a delivery this way because `emit` runs its listeners inline at the emit
        // (`pi/packages/coding-agent/src/core/event-bus.ts:12-32` @v0.83.0).
        if !self.no_subscribers(kind) {
            for ext in self.subscribers_for(kind) {
                if exclude.is_some_and(|x| ext.id() == x) {
                    continue;
                }
                // Fault-contained: an error is reported and skipped (R-08-036).
                if let Err(e) = self.invoke_contained(&ext, ev, cancel).await {
                    self.report(kind, ext.id(), &e);
                }
            }
        }
        self.drain_bus(cancel, exclude).await;
    }

    /// Collect EVERY subscribed extension's `handled` contribution for a discovery/aggregation event
    /// (Pi `resources_discover`/`project_trust`, runner.ts:197/1046). Unlike `dispatch_block_mutate`
    /// this does NOT short-circuit on the first `Handled`: it runs all subscribers in load order and
    /// returns each `(extension, value)` so the caller can fold them into a typed decision/aggregate
    /// (gap-08 #4). Faults are contained + skipped (R-08-036).
    pub async fn dispatch_collect_handled(
        &self,
        ev: &HostEvent,
        cancel: &CancelToken,
    ) -> Vec<(cyrup_core::ExtensionId, HandledValue)> {
        let kind = ev.kind();
        let mut out = Vec::new();
        // EXT-034: gate the handler loop, not the drain — see `dispatch_notify_excluding`.
        if !self.no_subscribers(kind) {
            for ext in self.subscribers_for(kind) {
                match self.invoke_contained(&ext, ev, cancel).await {
                    Ok(HookOutcome::Handled(v)) => out.push((ext.id().clone(), v)),
                    Ok(_) => {}
                    Err(e) => self.report(kind, ext.id(), &e),
                }
            }
        }
        self.drain_bus(cancel, None).await;
        out
    }

    /// Load-ordered dispatch that STOPS at the first extension whose `handled` value satisfies
    /// `decided` (Pi `emitProjectTrustEvent`, extensions/runner.ts:203-232: the loop `return`s the
    /// moment a handler answers anything other than `"undecided"`). Distinct from
    /// [`Self::dispatch_collect_handled`], which deliberately runs EVERY subscriber for an
    /// aggregation event (`resources_discover`). Using the collect-all variant for `project_trust`
    /// let a second extension's handler run — and side-effect — after the decision was already
    /// made. Faults are contained + skipped (R-08-036).
    pub async fn dispatch_first_handled(
        &self,
        ev: &HostEvent,
        cancel: &CancelToken,
        decided: impl Fn(&HandledValue) -> bool,
    ) -> Option<(cyrup_core::ExtensionId, HandledValue)> {
        let kind = ev.kind();
        // EXT-034: gate the handler loop, not the drain — see `dispatch_notify_excluding`.
        if self.no_subscribers(kind) {
            self.drain_bus(cancel, None).await;
            return None;
        }
        for ext in self.subscribers_for(kind) {
            match self.invoke_contained(&ext, ev, cancel).await {
                Ok(HookOutcome::Handled(v)) if decided(&v) => {
                    // EXT-034: the short-circuit return still has to drain — a handler that emitted
                    // AND decided is exactly the coordination case (a permission gate announcing its
                    // decision), and upstream that emit was delivered inline before the return.
                    self.drain_bus(cancel, None).await;
                    return Some((ext.id().clone(), v));
                }
                Ok(_) => {}
                Err(e) => self.report(kind, ext.id(), &e),
            }
        }
        self.drain_bus(cancel, None).await;
        None
    }

    /// Subscription-gated, load-ordered block/mutate chaining (arch-08 §6.1). First `Block` wins;
    /// `[mutate]` patches fold into `ev` so the next handler observes the folded value (R-08-011).
    pub async fn dispatch_block_mutate(&self, ev: HostEvent, cancel: &CancelToken) -> Reduced {
        self.dispatch_block_mutate_excluding(ev, cancel, None).await
    }

    /// [`Self::dispatch_block_mutate`] with one extension held out of the chain (EXT-052).
    ///
    /// CYRUP-DELTA: upstream never needs this. pi runs every subscribed handler because the whole
    /// runner is one JS process, so a provider extension that also subscribes to
    /// `before_provider_request` simply re-enters its own handler. cyrup's guest is suspended
    /// inside its own single-instance wasmtime `Store` while its `provider-stream.on-payload`
    /// import runs; re-entering that store would DEADLOCK on the store mutex rather than fail, so
    /// the emitting guest is excluded. Every OTHER subscriber runs exactly as it does upstream —
    /// which is the whole point of EXT-052, since the observers that matter are the other
    /// extensions.
    pub async fn dispatch_block_mutate_excluding(
        &self,
        ev: HostEvent,
        cancel: &CancelToken,
        exclude: Option<&cyrup_core::ExtensionId>,
    ) -> Reduced {
        let reduced = self.block_mutate_chain(ev, cancel, exclude).await;
        // EXT-034: drain on EVERY exit of the chain — including the first-block short-circuit,
        // which is precisely the handler most likely to have announced itself on `pi.events`.
        self.drain_bus(cancel, exclude).await;
        reduced
    }

    /// The block/mutate chain proper. Split out of [`Self::dispatch_block_mutate_excluding`] so the
    /// bus drain (EXT-034) covers its four early returns without four copies of the call.
    async fn block_mutate_chain(
        &self,
        mut ev: HostEvent,
        cancel: &CancelToken,
        exclude: Option<&cyrup_core::ExtensionId>,
    ) -> Reduced {
        let kind = ev.kind();
        if self.no_subscribers(kind) {
            return Reduced::Pass(Box::new(ev));
        }
        for ext in self.subscribers_for(kind) {
            if exclude.is_some_and(|x| ext.id() == x) {
                continue;
            }
            let outcome = match self.invoke_contained(&ext, &ev, cancel).await {
                Ok(o) => o,
                // A contained fault (returned error, guest trap/OOM, epoch or invocation-budget
                // timeout, native panic, (de)serialization failure, cancelled/unloaded instance) is
                // always reported (arch-08 §8) and never crashes the host. What happens NEXT is
                // per-kind (EXT-001):
                //
                // * fail CLOSED (`EventKind::fails_closed`, today `tool_call` only) — the fault
                //   BLOCKS the action, matching Pi: `emitToolCall` (runner.ts:932-953) has no
                //   try/catch, `agent-session.ts:475-487` re-throws `Extension failed, blocking
                //   execution: …`, and `agent-loop.ts:616-662` turns that into an immediate error
                //   result without executing the tool. Failing open here would let a trapped,
                //   panicking, or timed-out permission gate ALLOW the call it was meant to deny.
                //   Note this is a FAULT, not a decline: a handler that returns `Noop`/`Mutate`
                //   (declined to block) still proceeds, exactly as before.
                //
                // * fail OPEN (every other kind) — degrades to no-mutation and the chain continues,
                //   matching the per-handler `catch { continue }` in each of Pi's other emitters.
                Err(e) => {
                    self.report(kind, ext.id(), &e);
                    if kind.fails_closed() {
                        // EXT-029. A run abort still BLOCKS (the gated action must not run — see
                        // EXT-001; `fails_closed` is untouched) but carries NO reason, so nothing
                        // synthesizes "Extension failed, blocking execution: cancelled" out of the
                        // user's own Esc. `ExtHooks::before_tool_call` turns that reason-less block
                        // into `Proceed` when the run token is already cancelled, at which point
                        // `cyrup-agent`'s own re-check produces "Operation aborted" — pi's text
                        // (packages/agent/src/agent-loop.ts:629-635).
                        let reason = if matches!(e, ExtError::Cancelled) {
                            None
                        } else {
                            Some(format!("Extension failed, blocking execution: {e}"))
                        };
                        return Reduced::Blocked {
                            reason,
                            // A FAULT is not a terminate hint: pi's `terminate` can only come from
                            // a handler that returned `{block: true, terminate: true}`
                            // (types.ts:1072-1079 @v0.84.1), and a handler that trapped returned
                            // nothing at all.
                            terminate: cyrup_core::TerminateHint::Unspecified,
                            by: ext.id().clone(),
                        };
                    }
                    continue;
                }
            };
            match outcome {
                HookOutcome::Block { reason, terminate } => {
                    return Reduced::Blocked {
                        reason,
                        terminate,
                        by: ext.id().clone(),
                    };
                }
                HookOutcome::Handled(HandledValue(v)) => return Reduced::Handled(HandledValue(v)),
                HookOutcome::Mutate(patch) => ev.apply_patch(patch),
                HookOutcome::Noop => {}
            }
        }
        Reduced::Pass(Box::new(ev))
    }

    /// Wrap one guest call with the invocation budget. The `Extension` impl already contains panics
    /// (native) and traps/epoch/OOM (wasm); this adds the time budget for cooperative runaways.
    ///
    /// P-3 forgiveness: a handler that exposes a [`crate::native::HumanWaitGate`]
    /// ([`Extension::human_wait_gate`], today only the permission gate) may enter a sanctioned human
    /// wait; while that gate `is_waiting()` the budget watchdog is SUSPENDED so a slow human answer
    /// does not fire the budget and fail-OPEN the gate. A handler with no gate — or one whose gate is
    /// idle (a cooperative runaway that never began a human wait) — keeps the exact fail-fast timeout.
    async fn invoke_contained(
        &self,
        ext: &Arc<dyn Extension>,
        ev: &HostEvent,
        cancel: &CancelToken,
    ) -> Result<HookOutcome, ExtError> {
        let call = ext.invoke_event(ev, cancel);
        match ext.human_wait_gate() {
            Some(gate) => Self::invoke_with_human_wait_forgiveness(self.budget, &gate, call).await,
            None => match tokio::time::timeout(self.budget, call).await {
                Ok(r) => r,
                Err(_) => Err(ExtError::EpochTimeout),
            },
        }
    }

    /// The budget watchdog that honors a sanctioned human wait (P-3). The `select!` ALWAYS polls the
    /// handler future `call` (that is the branch that will drop the human-wait guard), racing it
    /// against the budget deadline. When the deadline elapses it forgives ONLY if a human wait is in
    /// progress — it re-arms the deadline a fresh budget out and keeps polling `call` (the budget clock
    /// advances only while NOT waiting); it must NOT await anything else here, or it would suspend the
    /// very handler whose completion ends the wait (a deadlock). With no human wait it fails the
    /// handler with `EpochTimeout`, exactly as the plain budget path does for a cooperative runaway.
    async fn invoke_with_human_wait_forgiveness<F>(
        budget: Duration,
        gate: &crate::native::HumanWaitGate,
        call: F,
    ) -> Result<HookOutcome, ExtError>
    where
        F: std::future::Future<Output = Result<HookOutcome, ExtError>>,
    {
        tokio::pin!(call);
        let mut deadline = tokio::time::Instant::now() + budget;
        loop {
            tokio::select! {
                biased;
                r = &mut call => return r,
                () = tokio::time::sleep_until(deadline) => {
                    if gate.is_waiting() {
                        // Sanctioned human wait still in progress: forgive — push the deadline out a
                        // fresh budget and loop, continuing to poll `call` (never suspend it).
                        deadline = tokio::time::Instant::now() + budget;
                    } else {
                        // No human wait ⇒ a cooperative runaway: unchanged fail-fast behavior.
                        return Err(ExtError::EpochTimeout);
                    }
                }
            }
        }
    }

    /// Poison the inner lock the only way a lock can be poisoned — a panic taken while the write
    /// guard is held — so the fail-soft degradation and its latched diagnostic are testable.
    #[cfg(test)]
    #[allow(
        clippy::panic,
        reason = "a panic under the write guard is the only way to poison it"
    )]
    pub(crate) fn poison_for_test(&self) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = self.inner.write();
            panic!("poisoning the dispatcher lock");
        }));
    }

    fn lock_read(&self) -> Result<std::sync::RwLockReadGuard<'_, DispatchInner>, ExtError> {
        self.inner
            .read()
            .map_err(|_| ExtError::Io("dispatcher lock poisoned".into()))
    }

    fn lock_write(&self) -> Result<std::sync::RwLockWriteGuard<'_, DispatchInner>, ExtError> {
        self.inner
            .write()
            .map_err(|_| ExtError::Io("dispatcher lock poisoned".into()))
    }
}
