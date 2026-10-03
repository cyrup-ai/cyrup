//! ADR-0030 §7's fourth compile-fail case / `G-INV-8`: a `SessionMut` cannot be obtained from
//! `CommitOutcome::Uncertain`.
//!
//! *"An uncertain storage failure is fatal to the open Session. It publishes nothing and must be
//! reopened"* (`spec.md:76-78`). Upstream carries that with a `#poison` flag on an object that stays
//! in the caller's hands, so the mistake is a `catch` away — and `spec.md:4581` has to *ask* people
//! not to make it.
//!
//! Here the handle is **inside** the three non-fatal variants and absent from the fatal one, so `?`
//! cannot smuggle one out and a catch-and-continue has nothing to continue with. Three different
//! attempts are made below, because the guarantee is not one signature: it is that no path reaches a
//! handle from the fatal arm.

use cyrup_pico::{CommitOutcome, SessionMut, UncertainCommit};

/// Attempt 1: ask the fatal value for the handle.
fn from_the_fatal_value(fatal: UncertainCommit) -> SessionMut {
    fatal.session()
}

/// Attempt 2: destructure the variant as though it carried one.
fn from_the_variant(outcome: CommitOutcome<()>) -> SessionMut {
    match outcome {
        CommitOutcome::Uncertain { session, .. } => session,
        CommitOutcome::NothingToCommit { session, .. }
        | CommitOutcome::Committed { session, .. }
        | CommitOutcome::RolledBack { session, .. } => session,
    }
}

/// Attempt 3: go through the ergonomic helper's error arm.
fn from_or_fatal(outcome: CommitOutcome<()>) -> SessionMut {
    match outcome.or_fatal() {
        Ok((_settled, session)) => session,
        Err(fatal) => fatal.into_session(),
    }
}

fn main() {}
