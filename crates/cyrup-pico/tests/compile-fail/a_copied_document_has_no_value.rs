//! `G-COPY-SOURCE-SNAPSHOT-ISOLATION`'s one unrepresentable half, and PICO5-PLAN S9's canary for it.
//!
//! `spec.md:1502-1504`: *"Definition-free copies publish explicit `document.copy` metadata rather than
//! a value. That metadata announces Storage-backed initial state and **is never interpreted as a
//! document value**."*
//!
//! Upstream can only say that. An unaccessed copy has no value in Session memory at all — storage
//! materialised the source and wrote the child's base, and the Session never saw it — so a consumer
//! that read a value off a copy announcement would be reading something invented. Here there is
//! nothing to read: [`cyrup_pico::Copied`] has no `value()`, no `ops()` and no root, and
//! [`cyrup_pico::Publication::copies`] is a different list from `changes()` precisely so a flag on
//! `Change` cannot leave the value field there to be misread.
//!
//! `E0599` twice. Adding either accessor makes this file compile, which is the only way a missing
//! method can fail a test.

use cyrup_pico::Copied;

fn canary(copied: &Copied) {
    let _value = copied.value();
    let _ops = copied.ops();
}

fn main() {}
