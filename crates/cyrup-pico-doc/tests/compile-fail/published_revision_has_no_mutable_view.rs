//! `G-TRUSTED-IMMUTABLE` — ADR-0030 §2.2, `spec.md:4528-4530`: *"every published revision is
//! immutable for all time, with structural sharing."*
//!
//! Upstream this is a convention with the consequence stated outright: *"runtime freezing is not
//! provided"*, so a renderer that sorts `value.tools` in place silently reorders the committed
//! revision every other observer and every replica computed from — no write, no commit, no event to
//! attribute it to. Here there is no mutable view to reach for.
//!
//! This case covers the *absence of a mutable API*; its sibling
//! `a_published_container_cannot_be_mutated.rs` covers the borrow itself, in a file of its own because
//! a name-resolution error here aborts the compile before borrow checking ever runs.
//!
//! Note what is NOT asserted by either: that no variant of `DocValue` contains interior mutability.
//! That rule is load-bearing (it is what makes `Arc<DocValue>` immutable rather than merely shared)
//! and it cannot be expressed as a compile-fail case — it is the one thing a reviewer must check by
//! reading `src/value.rs`'s enum, and its documentation says so.

use cyrup_pico_doc::DocRoot;

fn main() {
    let root = DocRoot::empty();

    // No `&mut` accessor exists: `as_map` hands out a shared reference and there is no `as_map_mut`.
    let members = root.as_map_mut();

    // Nor through the value: `at_mut` is crate-private, deliberately.
    let mut value = root.clone().into_value();
    let _slot = cyrup_pico_doc::at_mut(&mut value, &cyrup_pico_doc::Path::root());

    let _ = (members, value);
}
