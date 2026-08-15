//! The epoch driver (arch-08 §5.3, R-ARCH-EXT-012). One background task increments the engine epoch
//! every TICK; each armed instance has a deadline = ticks_until(timeout). A guest exceeding its
//! deadline traps with `Trap::Interrupt`, caught and surfaced as `EpochTimeout` — the host never
//! crashes.
//!
//! The cancel-and-preempt bridge this doc used to describe as fact — "On `RunCancel.cancel()` the
//! bridge calls `engine.increment_epoch()` immediately to preempt any running guest" — is
//! [`crate::WasmRuntime::preempt_all`], which **has no caller** (EXT-M09). What actually reaches a
//! running guest on cancellation is the `cancel.cancelled()` arm of
//! [`crate::host::LiveExtension`]'s `tokio::select!`s, which drops the in-flight call future. The
//! ticker below is unaffected either way: it is the thing that makes a deadline expire at all.

use cyrup_core::RunCancel;
use std::time::Duration;
use tokio::task::JoinHandle;
use wasmtime::Engine;

/// Default epoch tick (arch-08 §5.3 suggests ~5ms).
pub const DEFAULT_TICK: Duration = Duration::from_millis(5);

/// Drives `engine.increment_epoch()` on a fixed cadence until cancelled.
pub struct EpochDriver {
    handle: JoinHandle<()>,
}

impl EpochDriver {
    /// Spawn the background ticker. It exits when `cancel` fires.
    pub fn spawn(engine: Engine, tick: Duration, cancel: RunCancel) -> Self {
        let token = cancel.token();
        let handle = tokio::spawn(async move {
            let mut iv = tokio::time::interval(tick);
            iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                // `biased;` so cancellation always wins a tie. A freshly built `tokio::interval`
                // fires its FIRST tick immediately, so on the very first iteration both arms are
                // ready whenever the token was already cancelled at spawn — and an unbiased
                // `select!` picks at RANDOM, unlike a JS race, which cannot have two things ready at
                // once. The driver would then increment the epoch of an engine that is shutting
                // down and loop, exiting only on a later coin flip. Ordered, the shutdown is
                // deterministic: cancelled means stop, on the first poll, every time.
                tokio::select! {
                    biased;
                    _ = token.cancelled() => break,
                    _ = iv.tick() => engine.increment_epoch(),
                }
            }
        });
        Self { handle }
    }

    /// Abort the driver task.
    pub fn stop(self) {
        self.handle.abort();
    }
}

impl Drop for EpochDriver {
    fn drop(&mut self) {
        self.handle.abort();
    }
}
