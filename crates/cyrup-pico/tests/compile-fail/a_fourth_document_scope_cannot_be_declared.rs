//! The closed-set half of `spec.md:927-943`: there are exactly three document scopes and exactly two
//! shapes, and a fourth of either is a storage-format event rather than an additive change.
//!
//! `Placement` and `Shape` are sealed by a private supertrait — the pattern `cyrup-pico-store`'s
//! `IdKind` uses for the five id kinds (ADR-0030 F6 §A) — so a definition cannot invent a scope whose
//! records no backend knows how to index, whose lifetime rule nothing implements, and whose
//! `DocumentScope` has no arm to serialize into.
//!
//! The error to expect is `E0277`: the private supertrait is not implemented and cannot be.

struct Global;

impl cyrup_pico::Placement for Global {
    type Policy = ();
}

fn main() {}
