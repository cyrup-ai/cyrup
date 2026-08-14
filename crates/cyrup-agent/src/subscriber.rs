//! The notify-only event subscriber seam (func-02 R-02-012/048).
//!
//! Distinct from [`crate::hooks::Hooks`] (the MUTATING seam): an `EventSubscriber` only observes.
//! Every event is delivered to each subscriber, awaited in registration order, before the next
//! event is emitted. A subscriber MAY read state, enqueue steering/follow-up, or `abort()` during
//! dispatch, but MUST NOT re-enter `prompt`/`continue`/`wait_for_idle` to completion (arch-02 §5.5).

use crate::event::AgentEvent;
use cyrup_core::CancelToken;

/// A notify-only observer of the ordered event stream (func-02 R-02-012). Awaited in registration
/// order; `agent_end` subscribers' completion gates settlement (func-02 R-02-047/048). A PANICKING
/// subscriber fails the run, exactly as a throwing listener does upstream — see
/// [`crate::agent`]'s `RunCtx::emit` and AGENT-033.
#[async_trait::async_trait]
pub trait EventSubscriber: Send + Sync {
    /// AGENT-S02 — `cancel` is the run's abort signal, the second argument pi passes to every
    /// listener: `await listener(event, signal)` (`packages/agent/src/agent.ts:574` @v0.83.0,
    /// `:589` @v0.84.1; the listener type is declared at `:243`). A subscriber doing expensive work
    /// — streaming to a remote client, rendering, persisting — could not previously observe that
    /// the run it was servicing had been aborted, so it ran to completion and the abort's latency
    /// benefit was lost for exactly the listeners that make abort worth having.
    ///
    /// It is a fresh CHILD of the run token: cancelling it does not cancel the run.
    async fn on_event(&self, event: &AgentEvent, cancel: CancelToken);
}
