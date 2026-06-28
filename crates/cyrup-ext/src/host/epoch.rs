//! The epoch driver (arch-08 §5.3, R-ARCH-EXT-012). One background task increments the engine epoch
//! every TICK; each armed instance has a deadline = ticks_until(timeout). On `RunCancel.cancel()`
//! the bridge calls `engine.increment_epoch()` immediately to preempt any running guest. A guest
//! exceeding its deadline traps with `Trap::Interrupt`, caught and surfaced as `EpochTimeout` — the
//! host never crashes.

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
                tokio::select! {
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
