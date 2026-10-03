//! `G-SYNC-OBSERVERS-CAPTURE-ONLY`, prohibition 3 of 3: a line observer **cannot re-enter the
//! Session**.
//!
//! ADR-0030 F1: *"observers are `Fn(&Publication) -> ()` with no Session handle"*, which upstream can
//! only ask for — `spec.md:1529`'s *"must not … call Session APIs"*. The argument an observer receives
//! carries a sequence and a list of changes and nothing that can commit, so a nested commit from an
//! observer is not a thing to forbid: there is no handle to call it on.
//!
//! `E0599`: no method named `session` on `Publication`.

use cyrup_pico::Publication;

fn canary(publication: &Publication) {
    let _reentrant = publication.session();
}

fn main() {}
