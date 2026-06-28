//! The notify-only event subscriber seam (func-02 R-02-012/048).
//!
//! Distinct from [`crate::hooks::Hooks`] (the MUTATING seam): an `EventSubscriber` only observes.
//! Every event is delivered to each subscriber, awaited in registration order, before the next
//! event is emitted. A subscriber MAY read state, enqueue steering/follow-up, or `abort()` during
//! dispatch, but MUST NOT re-enter `prompt`/`continue`/`wait_for_idle` to completion (arch-02 §5.5).

use crate::event::AgentEvent;

/// A notify-only observer of the ordered event stream (func-02 R-02-012). Awaited in registration
/// order; a subscriber error is contained (it does not halt the loop), but `agent_end` subscribers'
/// completion gates settlement (func-02 R-02-047/048).
#[async_trait::async_trait]
pub trait EventSubscriber: Send + Sync {
    async fn on_event(&self, event: &AgentEvent);
}
