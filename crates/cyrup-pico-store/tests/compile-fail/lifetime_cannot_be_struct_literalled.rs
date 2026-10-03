//! A separate case from `value_newtypes_have_no_bypass.rs` on purpose: rustc emits the
//! struct-literal privacy error (`E0451`) in a pass that runs *after* type checking, so a file
//! whose earlier lines already fail type checking never reaches it and the snapshot would silently
//! record nothing. This case therefore holds exactly one error.
//!
//! What it pins: `Lifetime::new` is the only way to build the interval, so the
//! inverted-interval rejection cannot be routed around (ADR-0030 §2.2 row 3).

use cyrup_pico_store::{Lifetime, Seq};

fn main() {
    let _ = Lifetime {
        created_at: Seq::FIRST,
        retired_at: None,
    };
}
