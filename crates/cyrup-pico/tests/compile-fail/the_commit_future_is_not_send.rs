//! What ADR-0030 §10's `_not_send: PhantomData<*const ()>` on `Tx` costs, pinned so the cost is a
//! committed fact rather than a remembered one.
//!
//! `Tx: !Send` propagates outward: a commit holds a `Tx`, so `SessionMut::commit`'s future is `!Send`,
//! so `Line::run`'s future is `!Send` too and F1's single committer cannot sit in a `tokio::spawn`ed
//! task — a host drives it on a `LocalSet` or a thread-pinned current-thread runtime (`src/committer.rs`
//! says so, and this is the proof of the sentence).
//!
//! This is a **consequence, not a hazard**, and it is a case for two reasons. First, the guarantee F2
//! credits to `!Send` — *"a future that borrows the transaction cannot be spawned"* — does not actually
//! depend on the marker: `'static` alone closes that, which is what
//! `a_future_borrowing_the_transaction_cannot_be_spawned.rs` records. So the marker is kept for
//! exactly the property below, and if a later change deletes it to make the committer spawnable, this
//! case is what notices. Second, the alternative to a case is discovering it from a `spawn` that will
//! not typecheck three weeks into a host integration.

use cyrup_pico::{CallbackError, SessionMut};
use cyrup_pico_store::Cx;

fn spawn_the_committer(session: SessionMut, cx: &'static Cx) {
    tokio::spawn(session.commit(cx, async |_tx| {
        Err::<((), _), _>(CallbackError::Abandoned("not reached".into()))
    }));
}

fn main() {}
