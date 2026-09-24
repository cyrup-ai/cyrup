//! The unified extension abstraction (arch-08 §3.2). A native built-in and a WASM component are
//! interchangeable at the dispatch layer: both are an [`Extension`] handle and the dispatcher never
//! branches on kind — it calls [`Extension::invoke_event`] and lets the impl decide in-process vs.
//! boundary.

use crate::contract::HookOutcome;
use crate::error::ExtError;
use crate::event::{HostEvent, Subscriptions};
use cyrup_core::{CancelToken, ExtensionId};

/// Whether an extension runs in-process (native) or across the wasm boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtKind {
    Native,
    Wasm,
}

/// One loaded extension, native or wasm (arch-08 §3.2).
#[async_trait::async_trait]
pub trait Extension: Send + Sync {
    fn id(&self) -> &ExtensionId;
    fn kind(&self) -> ExtKind;
    /// Which events this extension subscribes to RIGHT NOW. Drives the subscription gate: events
    /// with zero subscribers never serialize/cross (R-ARCH-EXT-014).
    ///
    /// EXT-058: by VALUE, and re-read on every dispatch. pi keeps no snapshot — `api.on` mutates
    /// `extension.handlers` (`extensions/loader.ts:252-258` @v0.83.0) and each emitter looks the
    /// handler list up at dispatch (`runner.ts:806` and its twelve siblings), so an extension that
    /// subscribes after `init` receives the next event. A `&Subscriptions` return forced every impl
    /// to own a frozen bitset built at load time, which silently dropped exactly that case; the
    /// bitset is `Copy` so returning it costs nothing.
    fn subscriptions(&self) -> Subscriptions;

    /// Dispatch one event. Returns this extension's contribution to the block/mutate/notify
    /// reduction. `cancel` bounds the call (epoch deadline for wasm). A fault MUST surface as
    /// `Err`, never a panic/crash (R-08-036).
    async fn invoke_event(
        &self,
        ev: &HostEvent,
        cancel: &CancelToken,
    ) -> Result<HookOutcome, ExtError>;

    /// The sanctioned-long-wait coordinator for this extension's dispatch-budget forgiveness (P-3,
    /// `spec/extensions/cyrup-permission-system-port.md §4`; UW-3). `None` (default) ⇒ no
    /// forgiveness: the dispatcher applies the plain invocation-budget timeout (wasm guests already
    /// carry their own epoch forgiveness for UI round-trips). A native returns its ctx's gate so a
    /// handler that DECLARES a long wait — a human answer, a watchdog model review — is not dropped
    /// mid-wait. See [`crate::native::SanctionedWaitGate`]. The name predates the generalisation.
    fn human_wait_gate(&self) -> Option<std::sync::Arc<crate::native::HumanWaitGate>> {
        None
    }
}
