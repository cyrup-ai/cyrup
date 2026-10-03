//! ADR-0030 §2.1 invariants 2 and 3 / `G-INV-2` and `G-INV-3`'s **emitter** half: a `Publication`
//! struct literal has no spelling outside `src/witness.rs`, so there is no second emitter and nothing
//! becomes visible before its commit succeeded.
//!
//! Upstream carries both invariants with a sentence — `#publish()` has exactly one call site, placed
//! after `await commit()` and after `tx.adopt(seq)` — and `spec.md:4591` lists a volatile publication
//! path as a declared **non-goal**, which is the admission that nothing but that one call site enforces
//! them. The failure is not exotic: the live view wants a tool result rendered inside the 100–200 ms
//! throttle window, so somebody adds `publish_provisional(..)`, and now an observer renders state a
//! crash erases.
//!
//! Here the only producer is `Tx::adopt`, which consumes the crate-private `Durable` the kernel's
//! single `Storage::commit` call site minted. `Publication`'s *name* is public, because ADR-0030 §10
//! writes it `pub(crate)` and in the same block puts it in a public trait's method signature, which is
//! `E0446` — so the name had to be public and every construction path closed instead. That makes the
//! invalid program writable from here, which is why this is a case and not a note saying it could not
//! be written. `_seal: ()` is the field that turns the literal into `E0451`.
//!
//! The constructor half is a **second file**, `a_publication_has_no_public_constructor.rs`, and not a
//! second statement here: `E0624` is a type-check error and `E0451` comes from the privacy pass, which
//! does not run once type-checking has failed. One file would therefore pin only the first of the two.

use std::sync::Arc;

use cyrup_pico::{Change, Copied, Publication};
use cyrup_pico_store::Seq;

fn main() {
    let seq = Seq::new(core::num::NonZeroU64::MIN);
    let changes: Arc<[Change]> = Arc::from(Vec::new());
    let copies: Arc<[Copied]> = Arc::from(Vec::new());

    let _ = Publication {
        seq,
        changes,
        copies,
        _seal: (),
    };
}
