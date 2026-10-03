//! `Lifetime`'s fields are private and `contains` is the only membership accessor (ADR-0030 §2.2
//! row 3), so the `<=`/`<` asymmetry has one implementation rather than one per read site.
//! `PageLimit` and `DefVersion` likewise have no `Default` and no `From` of their primitive —
//! a default page limit and a default definition version are both values no caller means.

use cyrup_pico_store::{DefVersion, Lifetime, PageLimit, Seq};

fn main() {
    let lt = Lifetime::open(Seq::FIRST);

    let _ = lt.created_at;
    let _ = lt.retired_at;

    let _ = PageLimit::default();
    let _ = PageLimit::from(50_u32);
    let _ = DefVersion::default();
}
