//! `G-SYNC-OBSERVERS-CAPTURE-ONLY`, prohibition 2 of 3: a line observer **cannot await**.
//!
//! `spec.md:1521-1526` gives the commit the mutation line *"through callback execution, preparation,
//! storage settlement, committed baseline adoption, and publication enqueue"*, and `spec.md:1529`
//! adds that listeners *"must not throw, block, or call Session APIs"*. An observer that could await
//! would be awaiting inside that held window, which is §1 invariant 4.
//!
//! `CommitObserver::observe` is not `async` and the trait has no associated future, so there is no
//! shape in which an `async fn` is an implementation of it.

use cyrup_pico::{CommitObserver, Publication};

struct Patient;

impl CommitObserver for Patient {
    async fn observe(&self, _publication: &Publication) {
        tokio::task::yield_now().await;
    }
}

fn main() {}
