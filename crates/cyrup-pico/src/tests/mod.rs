//! S4's behaviour suite. ADR-0030 §7 sets its shape, and PICO5-PLAN S4 names the six cases.
//!
//! * *"`NothingToCommit` allocating no sequence"* — [`outcome`].
//! * *"`RolledBack` leaving the Session usable"* — [`outcome`].
//! * *"both `Uncertain` paths killing the loop and closing every ticket"* — [`line`].
//! * *"`adopt` being allocation-free (assert by counting)"* — the map-growth half is [`adoption`];
//!   the malloc-counting half is `tests/adoption_is_allocation_free.rs`, which needs a
//!   `#[global_allocator]` and therefore `unsafe`, which this crate forbids.
//! * *"a nested commit through a `Committer` from inside a callback being impossible to write at all
//!   (the canary is that the type has no such method)"* — the canary is
//!   `tests/compile-fail/a_nested_commit_cannot_be_started_from_the_transaction.rs`, which pins the
//!   `E0599` rather than a comment pinning it.
//!
//! PICO5-PLAN **S5** adds three modules beside them, named by the four cases S5 asks for:
//!
//! * [`defs`] — the definitions the suite speaks, token parsing, and *"parser tests for `DocAddress`
//!   singleton-versus-keyless"*.
//! * [`migration`] — *"the migration ladder (four arms)"*, *"the first-write-after-migration base
//!   even when the migrated value is deeply equal"*, the checkpoint predicate's exactly-once
//!   evaluation, and the token-versus-record agreement check.
//! * [`reopen`] — *"unaccessed documents with unavailable definitions surviving a reopen
//!   byte-identically"*.
//!
//! PICO5-PLAN **S6** adds one more, [`observation`], which is self-contained: its own definition and
//! its own address, so the observer and subscription cases do not share fixtures with S4's or S5's.
//!
//! PICO5-PLAN **S9** adds [`forks`], self-contained for the same reason: the two cases the plan names
//! by hand — *"the order-independence case explicitly"* and *"a behaviour test that later source
//! changes, retirement and reopen cannot affect the child"* — plus one case per sentence of
//! `spec.md` §3.7.
//!
//! The compile-fail half lives in `tests/compile-fail/` and is driven by `tests/compile_fail.rs`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod adoption;
mod defs;
mod documents;
mod forks;
mod line;
mod migration;
mod observation;
mod outcome;
mod reopen;
mod store_double;

use crate::{DocAddress, DocToken};
use defs::Live;

/// The `test.live` token: a Session-scoped singleton at version 1.
fn live() -> DocToken<Live> {
    DocToken::define().expect("test.live is a definition")
}

/// The Session's `test.live` singleton, as a typed address.
fn live_address() -> DocAddress<Live> {
    live().at()
}

/// A single-segment path.
fn path(key: &str) -> cyrup_pico_doc::Path {
    cyrup_pico_doc::Path::new([cyrup_pico_doc::Seg::key(key).expect("a safe key")])
        .expect("a safe path")
}
