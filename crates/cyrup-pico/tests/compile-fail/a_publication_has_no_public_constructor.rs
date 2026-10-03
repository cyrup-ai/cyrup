//! The constructor half of `a_publication_cannot_be_forged.rs`, split out because `E0624` is a
//! type-check error and `E0451` comes from the privacy pass, which does not run once type-checking has
//! failed — in one file the constructor error would mask the field-privacy one.
//!
//! ADR-0030 §2.1 invariants 2 and 3 / `G-INV-2` and `G-INV-3`'s emitter half. `Publication::new` has
//! **one call site**, in `Tx::adopt`, after the pointer swaps succeeded, and it is `pub(crate)` so
//! that call site is the only one there can be. There is no `Default` either, so an empty publication
//! is not a way in, and no `Clone`, so a publication cannot be replayed — the observer side of that
//! last one is `a_sync_observer_cannot_retain_the_publication.rs`.
//!
//! This is the same pair `cyrup-pico-store`'s `id_has_no_construction_path.rs` pins for
//! `Id::<Entry>::new`, for the same reason: allocation belongs to exactly one place, and a `pub` on the
//! constructor is a one-word edit that no runtime test turns red.

use std::sync::Arc;

use cyrup_pico::{Change, Copied, Publication};
use cyrup_pico_store::Seq;

fn main() {
    let seq = Seq::new(core::num::NonZeroU64::MIN);
    let changes: Arc<[Change]> = Arc::from(Vec::new());
    let copies: Arc<[Copied]> = Arc::from(Vec::new());

    let _ = Publication::new(seq, changes, copies);
    let _ = Publication::default();
}
