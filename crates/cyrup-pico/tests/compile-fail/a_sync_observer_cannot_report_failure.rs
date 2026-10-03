//! `G-SYNC-OBSERVERS-CAPTURE-ONLY`, prohibition 1 of 3: a line observer **cannot report failure**.
//!
//! ADR-0030 §2.3's row for `spec.md:1527-1533`: *"a throwing listener truncates the iteration **and**
//! reports a durable, adopted commit as a failure"*. `CommitObserver::observe` returns `()`, so there
//! is no channel through which an observer can tell the line that a commit it has already made durable
//! and already adopted did not happen.
//!
//! The error is `E0053`: the `impl`'s signature does not match the trait's. That code is the guarantee;
//! the wording is not.

use cyrup_pico::{CommitObserver, Publication};

struct Fussy;

impl CommitObserver for Fussy {
    fn observe(&self, _publication: &Publication) -> Result<(), String> {
        Err("the view is not ready".to_owned())
    }
}

fn main() {}
