//! ADR-0030 F6 §A: *"private fields, no `From<u64>`, no `Default`, no public `Id` constructor"*,
//! because allocation belongs to `Storage::mint` and nothing else — an in-memory fast path or a
//! stale cached high-water mark is how an id gets reissued, and a reissued id re-points history at
//! the wrong record.

use core::num::NonZeroU64;

use cyrup_pico_store::{Entry, EntryId, Id};

fn main() {
    let n = NonZeroU64::new(41).unwrap();

    let _ = Id::<Entry>::new(n);
    let _ = Id::<Entry>(n, core::marker::PhantomData);
    let _ = EntryId::from(41_u64);
    let _ = EntryId::default();
    let _: EntryId = 41_u64.into();
}
