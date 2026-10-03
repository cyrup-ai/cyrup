//! ADR-0030 §2.1 invariant 7 / `G-INV-7`, the **kernel** seam of *one committer*: a second mutation
//! handle onto one open store cannot be made.
//!
//! `cyrup-pico-store`'s `one_committer_at_a_time.rs` pins the *storage* seam — `E0499`/`E0596` on
//! `Storage::commit(&mut self, ..)`. That is the callee's half. This is the caller's half, and it is
//! the one a later change would undo: `SessionMut` owns `Box<dyn Storage>`, so the only way to get a
//! second committer is to duplicate the handle, and `derive(Clone)` is a one-line edit that no runtime
//! test turns red. `spec.md:4321-4323` is explicit that backends add **no second commit mutex**
//! because the Session is the sole committer, so the backends have nothing to fall back on.
//!
//! Three attempts, because the guarantee is not one signature: there is no `Clone`, no `Copy`, and no
//! second handle reachable from the read view.

use cyrup_pico::{Session, SessionMut};

/// Attempt 1: duplicate the handle directly. The binding is typed so the answer cannot be a cloned
/// *reference* standing in for a cloned handle.
fn two_handles(session: SessionMut) -> (SessionMut, SessionMut) {
    let second: SessionMut = session.clone();
    (session, second)
}

/// Attempt 2: ask the trait for it, which is what a generic helper would do.
fn through_the_bound(session: &SessionMut) {
    fn duplicate<T: Clone>(t: &T) -> T {
        t.clone()
    }
    let _ = duplicate(session);
}

/// Attempt 3: the clonable read view is a `Session`, and it does not hand one back.
fn from_the_read_view(session: &Session) -> SessionMut {
    session.clone().into_session_mut()
}

fn main() {}
