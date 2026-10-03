//! `G-DOC-SOURCE-NO-MUTABLE-OBJECT`, and PICO5-PLAN S6's *"one canary that a published revision has
//! no `&mut` path"*.
//!
//! `spec.md:4528-4530` makes every published revision immutable for all time and `spec.md:1251-1253`
//! has to add that *"mutation of it or any retained descendant is unsupported"* — *unsupported*,
//! because nothing stops it: the quietest corruption available is a consumer sorting `value.tools` in
//! place and reordering every observer's retained revision.
//!
//! Here there is no mutable accessor to call, on the revision or on the root inside it. `E0599` twice,
//! and the absence of those two methods is the guarantee: a later change that adds either one makes
//! this file compile, which is the only way a missing method can fail a test.

use cyrup_pico::Revision;

struct Live;

fn canary(revision: &Revision<Live>) {
    let _mutable_map = revision.raw().as_map_mut();
    let _mutable_member = revision.raw().get_mut("tools");
}

fn main() {}
