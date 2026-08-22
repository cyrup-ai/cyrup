//! Cancellation — one token, threaded everywhere (arch-00 §3.2).
//!
//! `CancelToken` is `tokio_util::sync::CancellationToken` (the single `AbortSignal` equivalent).
//! `RunCancel` is the per-run root threaded — by clone / `child()` — into the provider stream,
//! every hook, every `Tool::execute`, and the WASM epoch deadline.

use std::future::Future;

pub use tokio_util::sync::CancellationToken as CancelToken;

/// One root cancellation token per agent run (arch-00 §3.2). No subsystem invents its own abort
/// flag; this is the only mechanism.
#[derive(Clone, Debug, Default)]
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

    /// Root cancellation reaches every derived token — the `child()` contract that carries a run
    /// abort into hooks and `Tool::execute` (arch-00 §3.2).
    #[tokio::test(flavor = "current_thread")]
    async fn root_cancel_propagates_to_child_and_clone() {
        let rc = RunCancel::new();
        let child = rc.child();
        let clone = rc.token();
        assert!(!child.is_cancelled());
        assert!(!clone.is_cancelled());

        rc.cancel();

        assert!(child.is_cancelled());
        assert!(clone.is_cancelled());
        let out = child.run_until_cancelled(std::future::pending::<()>()).await;
        assert_eq!(out, None);
    }

    /// A child is cancellable independently: it must not abort the run or poison later children.
    #[tokio::test(flavor = "current_thread")]
    async fn child_cancel_does_not_propagate_to_root() {
        let rc = RunCancel::new();
        let child = rc.child();
        child.cancel();

        assert!(child.is_cancelled());
        assert!(!rc.is_cancelled());
        assert!(!rc.token().is_cancelled());
        assert!(!rc.child().is_cancelled());
        let out = rc.run_until(async { 7u8 }).await;
        assert_eq!(out, Some(7));
    }

    /// `cancel()` is idempotent (func-02 R-02-045): the second call is a no-op, not a re-arm.
    #[tokio::test(flavor = "current_thread")]
    async fn cancel_is_idempotent() {
        let rc = RunCancel::new();
        rc.cancel();
        assert!(rc.is_cancelled());
        rc.cancel();
        assert!(rc.is_cancelled());
        assert!(rc.token().is_cancelled());
        assert!(rc.child().is_cancelled());
        let out = rc.run_until(async { 42u8 }).await;
        assert_eq!(out, None);
    }
}
