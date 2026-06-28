//! The dispatch engine (arch-08 §6.1): subscription-gated, deterministic load-order, block/mutate
//! chaining. Holds extensions in load order plus an aggregate subscription bitset so an event with
//! zero subscribers returns in a single branch — no serialization, no boundary crossing
//! (R-08-034 / R-ARCH-EXT-014). Every guest call is fault-contained (R-08-036): a trap, OOM, epoch
//! timeout, or panic is logged and the handler is SKIPPED — the chain continues, the host never
//! crashes.

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

/// The subscription-gated, load-ordered dispatcher (arch-08 §3.1 / §6.1).
pub struct Dispatcher {
    inner: RwLock<DispatchInner>,
    budget: Duration,
}

#[derive(Default)]
struct DispatchInner {
    /// Extensions in deterministic LOAD ORDER (R-08-004).
    exts: Vec<Arc<dyn Extension>>,
    /// Union of every extension's subscription bitset — the cheap zero-subscriber gate.
    aggregate: Subscriptions,
}

impl Default for Dispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Dispatcher {
    pub fn new() -> Self {
        Self { inner: RwLock::new(DispatchInner::default()), budget: DEFAULT_INVOKE_BUDGET }
    }

    pub fn with_budget(budget: Duration) -> Self {
        Self { inner: RwLock::new(DispatchInner::default()), budget }
    }

    /// Add an extension at the end of the load order and fold its subscriptions into the aggregate.
    pub fn add(&self, ext: Arc<dyn Extension>) -> Result<(), ExtError> {
        let mut g = self.lock_write()?;
        g.aggregate = g.aggregate.union(*ext.subscriptions());
        g.exts.push(ext);
        Ok(())
    }

    /// The cheap gate (R-08-034): true iff NO loaded extension subscribes to `kind`. A single
    /// `bitset & kind` test; callers skip all serialization when this is true.
    pub fn no_subscribers(&self, kind: EventKind) -> bool {
        match self.lock_read() {
            Ok(g) => !g.aggregate.contains(kind),
            Err(_) => true, // poisoned => behave as if nothing subscribed (never crash)
        }
    }

    /// Number of loaded extensions (diagnostics/tests).
    pub fn len(&self) -> usize {
        self.lock_read().map(|g| g.exts.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn subscribers_for(&self, kind: EventKind) -> Vec<Arc<dyn Extension>> {
        match self.lock_read() {
            Ok(g) => g.exts.iter().filter(|e| e.subscriptions().contains(kind)).cloned().collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Notify-only dispatch (return ignored, R-08-009). Subscription-gated; awaited in load order.
    pub async fn dispatch_notify(&self, ev: &HostEvent, cancel: &CancelToken) {
        let kind = ev.kind();
        if self.no_subscribers(kind) {
            return;
        }
        for ext in self.subscribers_for(kind) {
            // Fault-contained: an error is reported and skipped (R-08-036).
            if let Err(e) = self.invoke_contained(&ext, ev, cancel).await {
                report(ext.id(), &e);
            }
        }
    }

    /// Subscription-gated, load-ordered block/mutate chaining (arch-08 §6.1). First `Block` wins;
    /// `[mutate]` patches fold into `ev` so the next handler observes the folded value (R-08-011).
    pub async fn dispatch_block_mutate(
        &self,
        mut ev: HostEvent,
        cancel: &CancelToken,
    ) -> Reduced {
        let kind = ev.kind();
        if self.no_subscribers(kind) {
            return Reduced::Pass(ev);
        }
        for ext in self.subscribers_for(kind) {
            let outcome = match self.invoke_contained(&ext, &ev, cancel).await {
                Ok(o) => o,
                // A faulting hook degrades to no-mutation (the action proceeds) + a surfaced
                // warning (arch-08 §8). Never aborts the chain, never crashes the host.
                Err(e) => {
                    report(ext.id(), &e);
                    continue;
                }
            };
            match outcome {
                HookOutcome::Block { reason } => {
                    return Reduced::Blocked { reason, by: ext.id().clone() }
                }
                HookOutcome::Handled(HandledValue(v)) => {
                    return Reduced::Handled(HandledValue(v))
                }
                HookOutcome::Mutate(patch) => ev.apply_patch(patch),
                HookOutcome::Noop => {}
            }
        }
        Reduced::Pass(ev)
    }

    /// Wrap one guest call with the invocation budget. The `Extension` impl already contains panics
    /// (native) and traps/epoch/OOM (wasm); this adds the time budget for cooperative runaways.
    async fn invoke_contained(
        &self,
        ext: &Arc<dyn Extension>,
        ev: &HostEvent,
        cancel: &CancelToken,
    ) -> Result<HookOutcome, ExtError> {
        match tokio::time::timeout(self.budget, ext.invoke_event(ev, cancel)).await {
            Ok(r) => r,
            Err(_) => Err(ExtError::EpochTimeout),
        }
    }

    fn lock_read(&self) -> Result<std::sync::RwLockReadGuard<'_, DispatchInner>, ExtError> {
        self.inner.read().map_err(|_| ExtError::Io("dispatcher lock poisoned".into()))
    }

    fn lock_write(&self) -> Result<std::sync::RwLockWriteGuard<'_, DispatchInner>, ExtError> {
        self.inner.write().map_err(|_| ExtError::Io("dispatcher lock poisoned".into()))
    }
}

/// Surface a contained fault. Diagnostics only (off by default, R-00-001) — never propagates.
fn report(id: &cyrup_core::ExtensionId, err: &ExtError) {
    tracing::warn!(extension = %id, error = %err, "extension call contained (skipped)");
}
