//! Compile-fail proof for the two things ADR-0030 F6 §D claims are not checks.
//!
//! | case | claim | what would undo it |
//! |---|---|---|
//! | `a_writable_store_needs_the_lock` | *"a writable backend cannot be built without the proof, so 'forgot to lock' is a compile error"* | a `JsonlStore::open` that takes no lock, added "for tests" |
//! | `a_store_lock_cannot_be_struct_literalled` | `StoreLock::acquire` is the only constructor | making a field public |
//! | `a_store_lock_cannot_be_cloned` | [`StoreLock`] is RAII and not `Clone` | a derived `Clone` |
//! | `a_read_only_store_cannot_commit` | *"`open_read_only` taking no lock and **having no `commit`**"* | implementing `Storage` for the read-only store and refusing at runtime |
//!
//! **One guarantee per file where the compiler forces it.** rustc stops after the first pass that
//! produces an error, so a file mixing a missing argument (`E0061`) with a missing method (`E0599`) would
//! pin only the first and leave the other silently unenforced.
//!
//! The snapshots are rustc output and are toolchain-sensitive. Regenerate with
//! `TRYBUILD=overwrite cargo test -p cyrup-pico-store-jsonl --test compile_fail` after a toolchain bump
//! and read the diff: a changed *error code* is a change in the guarantee, a changed *wording* is not.
//!
//! [`StoreLock`]: cyrup_pico_store_jsonl::StoreLock

#[test]
fn the_unrepresentable_states_do_not_compile() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile-fail/*.rs");
}
