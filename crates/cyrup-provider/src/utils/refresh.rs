//! In-flight refresh dedup (1:1 port of the `inflightRefresh` memo in Pi
//! `models.ts:createProvider`, lines 353-363).
//!
//! A dynamic provider's `refreshModels()` must collapse concurrent calls onto a SINGLE underlying
//! fetch (`inflightRefresh ??= (async () => { … })()`), and clear the memo once the fetch settles
//! (the `finally` block) so a later call re-fetches. On rejection the catalog stays at its
//! last-known state and the error propagates to the caller. [`RefreshDedup`] is the reusable
//! primitive a `Provider::refresh_models` override uses to reproduce that behavior.

use crate::error::ProviderError;
use futures::future::{BoxFuture, FutureExt, Shared};
use std::future::Future;
use std::sync::{Arc, Mutex};

/// Shared output. [`futures::future::Shared`] requires a `Clone` output and [`ProviderError`] is not
/// `Clone`, so the result is shared behind an `Arc` and each awaiter receives a
/// [`ProviderError::reproduce`] copy — the original error variant/code is preserved (it is NOT
/// coerced to `model_source` here; `Models::refresh` owns that wrapping, matching Pi).
type SharedOut = Arc<Result<(), ProviderError>>;
type SharedFut = Shared<BoxFuture<'static, SharedOut>>;

/// Per-provider in-flight refresh memo (Pi `inflightRefresh`).
#[derive(Default)]
pub struct RefreshDedup {
    inflight: Mutex<Option<SharedFut>>,
}

impl RefreshDedup {
    pub fn new() -> Self {
        Self { inflight: Mutex::new(None) }
    }

    /// Run `fetch` deduplicated: the first caller starts it; concurrent callers await the same
    /// in-flight future; after it settles the originating caller clears the memo so a later call
    /// retries (Pi `finally { inflightRefresh = undefined; }`).
    pub async fn run<F, Fut>(&self, fetch: F) -> Result<(), ProviderError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<(), ProviderError>> + Send + 'static,
    {
        // Fast path — a concurrent caller already has a fetch in flight: clone and await it. The
        // lock guard is read-and-dropped inside this expression (no await held under the lock).
        let existing: Option<SharedFut> =
            self.inflight.lock().ok().and_then(|slot| slot.clone());
        if let Some(shared) = existing {
            return finish(shared.await);
        }

        // Owner path — build the fetch future exactly once and publish it for concurrent callers
        // (Pi `inflightRefresh ??= (...)()`). A poisoned mutex simply skips publication (the fetch
        // still runs, just un-deduplicated) rather than panicking.
        let shared = fetch().map(Arc::new).boxed().shared();
        if let Ok(mut slot) = self.inflight.lock()
            && slot.is_none()
        {
            *slot = Some(shared.clone());
        }

        let out = shared.await;

        // Clear the memo once the fetch settles so a later call retries (Pi `finally`).
        if let Ok(mut slot) = self.inflight.lock() {
            *slot = None;
        }

        finish(out)
    }
}

/// Reproduce the shared output as an owned `Result` for one awaiter (the original error
/// variant/code is preserved via [`ProviderError::reproduce`]).
fn finish(out: SharedOut) -> Result<(), ProviderError> {
    match out.as_ref() {
        Ok(()) => Ok(()),
        Err(e) => Err(e.reproduce()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// Concurrent callers share ONE fetch (Pi `inflightRefresh ??=`); a later call after the memo
    /// clears re-fetches (Pi `finally`).
    #[tokio::test]
    async fn concurrent_callers_share_one_fetch_then_retry() {
        let dedup = RefreshDedup::new();
        let count = Arc::new(AtomicUsize::new(0));

        let mk = || {
            let count = count.clone();
            move || {
                let count = count.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    Ok(())
                }
            }
        };

        // Two concurrent refreshes → exactly one underlying fetch.
        let (a, b) = tokio::join!(dedup.run(mk()), dedup.run(mk()));
        a.expect("ok");
        b.expect("ok");
        assert_eq!(count.load(Ordering::SeqCst), 1, "concurrent calls share one fetch");

        // After the memo clears, a fresh call re-fetches.
        dedup.run(mk()).await.expect("ok");
        assert_eq!(count.load(Ordering::SeqCst), 2, "later call retries");
    }

    /// A failing fetch propagates to the caller with its ORIGINAL error variant/code preserved (the
    /// dedup does not coerce it to `model_source` — `Models::refresh` owns that wrapping). The
    /// catalog list is unaffected (the caller leaves the last-known list in place).
    #[tokio::test]
    async fn failing_fetch_preserves_original_error() {
        let dedup = RefreshDedup::new();
        let err = dedup
            .run(|| async { Err(ProviderError::Transport("network down".into())) })
            .await
            .expect_err("should fail");
        assert_eq!(err.code(), "transport");
        assert!(err.to_string().contains("network down"));
    }
}
