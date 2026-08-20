//! In-flight refresh dedup (1:1 port of the `inflightRefresh` memo in Pi `createProvider`,
//! `packages/ai/src/models.ts:556-616` @v0.83.0 — the memo at `:598`, the `finally` at `:611-613`).
//!
//! The previous citation here, `models.ts:353-363`, is wrong at the pinned tag: those lines are
//! `Models.readCredential`. Upstream carries the same memo in two more places, and both agree with
//! the shape ported below — `remote-catalog-provider.ts:50,56,117` and
//! `images-models.ts:252,262,266`.
//!
//! A dynamic provider's `refreshModels()` must collapse concurrent calls onto a SINGLE underlying
//! fetch (`inflightRefresh ??= (async () => { … })()`), and clear the memo once the fetch settles
//! (the `finally` block) so a later call re-fetches. On rejection the catalog stays at its
//! last-known state and the error propagates to the caller. [`RefreshDedup`] is the reusable
//! primitive a `Provider::refresh_models` override uses to reproduce that behavior.
//!
//! **Two JS→Rust guarantee gaps live in this file**, both fixed and both regression-tested below.
//! Upstream's `finally` sits INSIDE the async IIFE, so it belongs to the promise and a JS `async`
//! fn always settles; and `??=` is atomic because the event loop is single-threaded. Neither
//! guarantee survives translation: a Rust future can be dropped at any `.await`, and a
//! read-then-publish across two threads is not atomic. See [`MemoClear`] and the publish comment in
//! [`RefreshDedup::run`].

use crate::error::ProviderError;
use futures::future::{BoxFuture, FutureExt, Shared};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

/// Shared output. [`futures::future::Shared`] requires a `Clone` output and [`ProviderError`] is not
/// `Clone`, so the result is shared behind an `Arc` and each awaiter receives a
/// [`ProviderError::reproduce`] copy — the original error variant/code is preserved (it is NOT
/// coerced to `model_source` here; `Models::refresh` owns that wrapping, matching Pi).
type SharedOut = Arc<Result<(), ProviderError>>;
type SharedFut = Shared<BoxFuture<'static, SharedOut>>;

/// The memo slot: the in-flight fetch plus the generation that published it. The generation makes
/// [`MemoClear`] idempotent against a slot that has already moved on.
type Slot = Arc<Mutex<Option<(u64, SharedFut)>>>;

/// Monotonic publication counter — see [`MemoClear`].
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Clears the memo when the fetch it guards settles OR is abandoned.
///
/// This is the port of Pi's `finally { inflightRefresh = undefined; }`. The placement is the whole
/// point: upstream's `finally` is INSIDE the async IIFE, so it belongs to the promise rather than to
/// any caller, and a JS `async` fn always settles. Holding this guard inside the shared future body
/// reproduces that — the clear runs when the fetch completes regardless of WHICH caller drove it,
/// and it also covers the case JS cannot produce, where the future is dropped un-settled.
///
/// The `Weak` handle is load-bearing: the slot owns the shared future, which owns this guard, so a
/// strong reference here would be an `Arc` cycle that leaks the slot forever.
struct MemoClear {
    slot: Weak<Mutex<Option<(u64, SharedFut)>>>,
    generation: u64,
}

impl Drop for MemoClear {
    fn drop(&mut self) {
        let Some(slot) = self.slot.upgrade() else {
            // The whole `RefreshDedup` is being dropped; the slot is going away with it.
            return;
        };
        // Take the future out UNDER the lock but drop it OUTSIDE: dropping a `Shared` can drop the
        // inner future, which owns a `MemoClear`, which would re-enter this non-reentrant `Mutex`.
        let taken = match slot.lock() {
            Ok(mut current) => match current.as_ref() {
                // Only clear our OWN publication, never a newer one.
                Some((generation, _)) if *generation == self.generation => current.take(),
                _ => None,
            },
            Err(_) => None,
        };
        drop(taken);
    }
}

/// Per-provider in-flight refresh memo (Pi `inflightRefresh`).
#[derive(Default)]
pub struct RefreshDedup {
    inflight: Slot,
}

impl RefreshDedup {
    pub fn new() -> Self {
        Self {
            inflight: Arc::new(Mutex::new(None)),
        }
    }

    /// Run `fetch` deduplicated: the first caller starts it, concurrent callers await that same
    /// in-flight future, and the memo clears once the fetch settles so a later call re-fetches
    /// (Pi `inflightRefresh ??= (async () => { try { … } finally { inflightRefresh = undefined } })()`).
    pub async fn run<F, Fut>(&self, fetch: F) -> Result<(), ProviderError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<(), ProviderError>> + Send + 'static,
    {
        // Fast path — someone already has a fetch in flight. The lock guard is dropped inside this
        // expression, so no await is ever held under the lock.
        let existing: Option<SharedFut> = self
            .inflight
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(|(_, shared)| shared.clone()));
        if let Some(shared) = existing {
            return finish(shared.await);
        }

        // Build a candidate. This runs the caller's closure, so it is deliberately NOT done under
        // the lock — but it is side-effect free: constructing a future does not start the fetch,
        // only polling it does. That is what makes losing the publish race below free.
        let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        let guard = MemoClear {
            slot: Arc::downgrade(&self.inflight),
            generation,
        };
        let fetch = fetch();
        let candidate: SharedFut = async move {
            // Dropped when this body settles OR when the future is dropped un-settled — Pi's
            // `finally`, plus the case JS cannot produce.
            let _clear_on_settle = guard;
            fetch.await
        }
        .map(Arc::new)
        .boxed()
        .shared();

        // Publish, re-checking under the SAME lock acquisition that publishes. Pi's `??=` is atomic
        // because the event loop is single-threaded; a Rust read-then-publish is not, so two threads
        // can both observe an empty memo. The loser must adopt the winner's future rather than await
        // its own: each fetch WRITES the provider's catalog, so two live fetches race to a
        // last-writer-wins result instead of deduplicating. Dropping the un-polled candidate is what
        // guarantees the loser's fetch never runs at all.
        let shared = match self.inflight.lock() {
            Ok(mut slot) => match slot.as_ref() {
                Some((_, winner)) => winner.clone(),
                None => {
                    *slot = Some((generation, candidate.clone()));
                    candidate
                }
            },
            // A poisoned mutex runs the fetch un-deduplicated rather than panicking.
            Err(_) => candidate,
        };

        finish(shared.await)
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
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "concurrent calls share one fetch"
        );

        // After the memo clears, a fresh call re-fetches.
        dedup.run(mk()).await.expect("ok");
        assert_eq!(count.load(Ordering::SeqCst), 2, "later call retries");
    }

    /// **The publish race.** Pi's `inflightRefresh ??= (...)()` is atomic because the event loop is
    /// single-threaded. A read-then-publish in Rust is not: two worker threads can both observe an
    /// empty memo, and if the loser awaits its own future instead of adopting the winner's, BOTH
    /// fetches run and both write the provider's catalog — last-writer-wins, which is the opposite
    /// of what the memo exists to do.
    ///
    /// The barrier pins the interleaving that is otherwise a narrow window: both callers are held
    /// inside the closure (the synchronous half, which merely BUILDS the future) until each has
    /// passed the fast-path check, and only then are they released to contend for the publish lock.
    ///
    /// The barrier alone is NOT enough, and that gap was a real flake — one run in four of
    /// `cargo test -p cyrup-provider` on a loaded box, reported as `1117 passed; 1 failed`. The
    /// barrier orders the two callers' ENTRY into the publish region; it says nothing about when the
    /// loser leaves it. Releasing the winner's fetch as soon as the winner announced "started" (the
    /// signal this test used to wait on) let the winner run to completion — and completion drops
    /// [`MemoClear`], emptying the memo — while the loser was still descheduled between
    /// `gate.wait()` and its `self.inflight.lock()`. The loser then found an EMPTY memo and
    /// correctly, per Pi's `finally`, started a second fetch: `count == 2`, a scheduling artifact
    /// rather than the defect this test exists to catch.
    ///
    /// So the release now waits for BOTH callers to finish deciding. The signal is each caller's
    /// first `Poll::Pending`: [`RefreshDedup::run`] has no await point anywhere between its entry
    /// and `shared.await` — fast path, generation, closure, publish, all synchronous — and the
    /// future it ends up awaiting is parked on `held`, so the first `Pending` is exactly "this
    /// caller has published or adopted". It is observable for the LOSER too, which is the whole
    /// point: any signal sent from inside the fetch body only ever fires for the winner.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_lost_publish_race_does_not_start_a_second_fetch() {
        use std::future::poll_fn;

        let dedup = Arc::new(RefreshDedup::new());
        let count = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(std::sync::Barrier::new(2));
        // Holds the winner's fetch open — and therefore its memo published — until released below.
        let (release, held) = tokio::sync::watch::channel(false);
        // One permit per caller that has reached its first `Pending`, i.e. finished deciding.
        let decided = Arc::new(tokio::sync::Semaphore::new(0));

        let callers = (0..2).map(|_| {
            let dedup = Arc::clone(&dedup);
            let count = Arc::clone(&count);
            let gate = Arc::clone(&gate);
            let decided = Arc::clone(&decided);
            let mut held = held.clone();
            tokio::spawn(async move {
                let mut call = Box::pin(dedup.run(move || {
                    // Synchronous: both callers have cleared the fast path by the time either
                    // returns a future, so they collide on the publish.
                    gate.wait();
                    async move {
                        count.fetch_add(1, Ordering::SeqCst);
                        // Hold the fetch open so a second one would overlap observably.
                        let _ = held.changed().await;
                        Ok(())
                    }
                }));
                // Announce the publish decision, then hand the poll result straight back — this
                // wrapper observes `run`, it does not alter it.
                let mut announced = false;
                poll_fn(|cx| {
                    let polled = call.as_mut().poll(cx);
                    if polled.is_pending() && !announced {
                        announced = true;
                        decided.add_permits(1);
                    }
                    polled
                })
                .await
            })
        });
        let joined = futures::future::join_all(callers);

        // Both callers have published-or-adopted; only now may the winner's fetch settle. A caller
        // that wrongly started its OWN fetch also parks and reports here, so the broken case still
        // reaches the assertion below instead of hanging.
        let permits = decided
            .acquire_many(2)
            .await
            .expect("the semaphore is never closed");
        drop(permits);
        let _ = release.send(true);

        for result in joined.await {
            result.expect("task").expect("refresh ok");
        }
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "the caller that loses the publish race adopts the winner's fetch"
        );
    }

    /// **The dropped-owner memo leak.** Pi's `finally { inflightRefresh = undefined; }` lives INSIDE
    /// the async IIFE (`remote-catalog-provider.ts:56,116-118`, `images-models.ts:262-266`
    /// @v0.83.0), so it runs when the promise SETTLES — no matter which caller is awaiting, and a JS
    /// `async` fn always settles. A Rust future can be dropped at any `.await`, so a clear that sits
    /// in the owning caller's straight-line code after `shared.await` is skipped whenever that owner
    /// is cancelled — and `refresh_model_catalogs_with` cancels exactly this way, via
    /// `tokio::time::timeout` (`crates/cyrup/src/provider.rs:253-259`).
    ///
    /// The leak is permanent: a non-owner awaiter drives the abandoned fetch to completion, but only
    /// the owner ever cleared, so the memo stays pinned to a COMPLETED `Shared` whose output is
    /// cached — every later refresh of that provider returns the stale result and never re-fetches.
    #[tokio::test]
    async fn memo_clears_when_the_owner_is_cancelled_mid_fetch() {
        use futures::pin_mut;

        let dedup = RefreshDedup::new();
        let count = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();

        let counting = || {
            let count = count.clone();
            move || {
                let count = count.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            }
        };

        // Owner: poll once so the fetch starts and the memo is published, then DROP it mid-flight.
        {
            let owner = dedup.run(move || async move {
                let _ = rx.await;
                Ok(())
            });
            pin_mut!(owner);
            assert!(
                futures::poll!(owner.as_mut()).is_pending(),
                "owner parks on the blocked fetch, publishing the memo"
            );
        }

        // Unblock the abandoned fetch, then let a second caller drive it to completion. It shares
        // the in-flight future, so it must NOT start a fetch of its own.
        let _ = tx.send(());
        dedup.run(counting()).await.expect("shares the in-flight fetch");
        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "the second caller shares the owner's fetch rather than starting one"
        );

        // The fetch has now settled, so the memo must be clear and this call must really re-fetch.
        dedup.run(counting()).await.expect("ok");
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "a settled fetch clears the memo even though its owner was cancelled"
        );
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
