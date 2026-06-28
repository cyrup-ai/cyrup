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
    /// Which events this extension subscribed to (built at init). Drives the subscription gate:
    /// events with zero subscribers never serialize/cross (R-ARCH-EXT-014).
    fn subscriptions(&self) -> &Subscriptions;

    /// Dispatch one event. Returns this extension's contribution to the block/mutate/notify
    /// reduction. `cancel` bounds the call (epoch deadline for wasm). A fault MUST surface as
    /// `Err`, never a panic/crash (R-08-036).
    async fn invoke_event(
        &self,
        ev: &HostEvent,
        cancel: &CancelToken,
    ) -> Result<HookOutcome, ExtError>;
}
