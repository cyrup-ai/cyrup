//! `G-INV-3`, observer half: an observer **captures immutable state and cannot retain the
//! publication itself**.
//!
//! §1 invariant 3 is *"all visible progress is durable; there is no volatile path"*, and ADR-0030 F5's
//! second failure mode is a publication that outlives its commit window — replayed, or held and
//! emitted again by something that is not the one publish. `Publication` is not `Clone`, not `Copy`,
//! and is handed to an observer by shared reference, so moving it out of the borrow is `E0507`.
//!
//! What an observer *may* keep is the already-immutable state inside it: a `DocRoot` clone is a
//! refcount bump, which is `spec.md:1527-1533`'s *"may only capture immutable state"* satisfied by
//! what the type hands over rather than by a rule.

use cyrup_pico::Publication;

fn canary(publication: &Publication) {
    let _kept: Publication = *publication;
}

fn main() {}
