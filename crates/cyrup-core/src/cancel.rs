//! Cancellation — one token, threaded everywhere (arch-00 §3.2).
//!
//! `CancelToken` is `tokio_util::sync::CancellationToken` (the single `AbortSignal` equivalent).
//! `RunCancel` is the per-run root threaded — by clone / `child()` — into the provider stream,
//! every hook, every `Tool::execute`, and the WASM epoch deadline.

use std::future::Future;

pub use tokio_util::sync::CancellationToken as CancelToken;

/// One root cancellation token per agent run (arch-00 §3.2). No subsystem invents its own abort
/// flag; this is the only mechanism.
#[derive(Clone, Default)]
pub struct RunCancel {
    root: CancelToken,
}

impl RunCancel {
    pub fn new() -> Self {
        Self { root: CancelToken::new() }
    }

    /// A clone of the root token (cancelled together with the run).
    pub fn token(&self) -> CancelToken {
        self.root.clone()
    }

    /// A child token (cancellable independently, but also cancelled when the root is).
    pub fn child(&self) -> CancelToken {
        self.root.child_token()
    }

    /// Idempotent (func-02 R-02-045).
    pub fn cancel(&self) {
        self.root.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.root.is_cancelled()
    }

    /// Race a future against cancellation; resolves `None` if cancelled first.
    pub async fn run_until<F: Future>(&self, fut: F) -> Option<F::Output> {
        self.root.run_until_cancelled(fut).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn run_until_yields_none_when_cancelled() {
        let rc = RunCancel::new();
        rc.cancel();
        assert!(rc.is_cancelled());
        let out = rc.run_until(async { 42u8 }).await;
        assert_eq!(out, None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_until_yields_value_when_not_cancelled() {
        let rc = RunCancel::new();
        let out = rc.run_until(async { 42u8 }).await;
        assert_eq!(out, Some(42));
    }
}
