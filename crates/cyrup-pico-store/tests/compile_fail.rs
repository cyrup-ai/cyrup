//! Compile-fail proof for the rows ADR-0030 §2 classifies `unrepresentable`.
//!
//! Two slices' worth. **S1's** are the identity rows; **S3's** are the storage-contract rows, and they
//! are the ones ADR-0030 §7's own list argues hardest for, because each is enforced by a signature or by
//! the *absence* of one:
//!
//! | case | row | what would undo it |
//! |---|---|---|
//! | `a_batch_cannot_be_struct_literalled` | §2.1 row 1 | making one field public |
//! | `a_batch_cannot_be_deserialized` | §10 serde table | a `derive(Deserialize)` "for tests" |
//! | `there_is_no_ordered_write_list` | §2.2, `spec.md:4363` | a `writes()` accessor so a backend's loop reads like pi's |
//! | `a_built_batch_cannot_take_a_second_command` | F3's first two illegal states | a mutable accessor |
//! | `one_committer_at_a_time` | §2.2, `spec.md:4321-4323` | relaxing `commit` to `&self` to share a backend |
//! | `cursors_do_not_cross_scans` | §2.2, `spec.md:4320-4321` | one shared cursor type |
//! | `io_error_is_not_a_rejection` | §2.2, `spec.md:4307-4311` | a `From<io::Error>` for `CommitError` |
//!
//! **One guarantee per file where the compiler forces it.** rustc runs resolution, then type checking,
//! then privacy, then borrow checking, and stops after the first pass that produces an error — so a file
//! mixing a missing method (`E0599`) with a private field (`E0451`) or a bad borrow (`E0596`) reports
//! only the first, and the others would silently go unpinned. That is why
//! `a_batch_cannot_be_struct_literalled` and `a_built_batch_cannot_take_a_second_command` are cases of
//! their own rather than paragraphs in a bigger one.
//!
//! ADR-0030 §7 reserves `trybuild` for four kernel signatures and says *"Not for the newtypes —
//! their constructors are private and ordinary unit tests cover them"*. That reasoning covers the
//! construction half of F6 §A, which §2.2 classifies `guarded` and which unit tests do cover. It
//! does not cover the other half: §2.2 row 1 classifies *"an entity id and a commit sequence are
//! different kinds of thing"* as **`unrepresentable`**, and §1.5 classifies kind confusion the
//! same way. Neither is enforced by a runtime check that a test could trip — both are enforced
//! only by a signature, which means a well-meaning later change (a convenience `From<u64>`, a
//! `Deref` to the number, a widened type alias) would undo them with no test turning red.
//!
//! So these cases exist, and they are cheap: `trybuild` is a dev-dependency and is absent from the
//! `cargo build` graph.
//!
//! The snapshots in `compile-fail/*.stderr` are rustc output and are therefore toolchain-sensitive.
//! Regenerate them with `TRYBUILD=overwrite cargo test -p cyrup-pico-store --test compile_fail`
//! after a toolchain bump, and read the diff: a changed *error code* is a change in the guarantee,
//! a changed *wording* is not.

#[test]
fn the_unrepresentable_states_do_not_compile() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile-fail/*.rs");
}
