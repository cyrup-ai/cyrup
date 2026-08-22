---
stage: qa
status: completed
updated: 2026-08-22 19:46
---

# Decompose Manager Module Into Submodules — QA Rework

**QA rating: 9/10.** The decomposition itself is production quality and is accepted. One
defect remains, in the one dimension this refactor exists to improve: navigability.

## Outstanding

### 1. Stale section banner in `manager/mod.rs:111`

```rust
    // ----------------------------------------------------------------- append (R-04-016) ------

    fn make_base(&self) -> EntryBase {
```

The banner moved verbatim with the code beneath it, but the code beneath it is no longer
"append" — the nine `append_*` constructors now live in
[`manager/append.rs`](../../crates/cyrup-session/src/manager/append.rs), whose own module doc
already claims `//! The typed entry constructors (R-04-016)`. What actually sits under the
banner in `mod.rs` is the **write path**: `make_base`, `mint_id`, `push_entry`, `persist_last`,
`has_assistant_message`.

Three things now disagree with each other:

- `mod.rs:111` labels the section `append (R-04-016)`;
- `mod.rs:14-15` (the `## Layout` doc) calls the same code "the single write path every concern
  shares (`make_base`, `push_entry`, `persist_last`)";
- `append.rs:1` claims the `append` name and the R-04-016 tag for a different file.

A reader who lands on that banner goes looking for `append_message` in `mod.rs` and does not
find it. That is precisely the navigation failure the decomposition was meant to remove, and it
is the one instruction from the task's step 4 that was not carried out: *"Replace the removed
section banners with the module doc line of the file that now owns the concern; keep the
intra-file banners in `mod.rs` for the two groups it retains."* The retained group here is the
write path, not append.

**Fix:** retitle the banner to name what it now covers, keeping the R-number that belongs to
the write path itself. Something in the existing house style, e.g.

```rust
    // ------------------------------------------------------- write path (R-04-016/032/036) ---
```

`R-04-016/032/036` is the tag `push_entry`'s own doc comment already carries
(`mod.rs:126`), so it is the accurate citation for this group.

Do not touch `append.rs` — its module doc is correct as written. This is a one-line change to
`mod.rs`; nothing else in the tree needs to move.

## Adjudicated — considered and accepted, do NOT rework

These two items read as misses against the literal text of the previous definition of done.
Both were examined and both pass. Do not "fix" them.

- **`lifecycle.rs` is 203 lines against a "~200" target.** The overage is the three-line module
  doc plus the twelve-line import block sitting on top of 177 lines of irreducible verbatim
  content. Splitting a coherent concern to shave three lines would be worse code.
- **`git diff -M --stat` does not render the change as a rename.** A nine-way split leaves no
  destination holding a majority of the 957-line source (`mod.rs` retains 20%), so the default
  50% similarity threshold cannot pair them; only `-M15%` pairs `manager.rs → lifecycle.rs`.
  This is inherent to the split's granularity, not evidence of a rewrite. Verbatim preservation
  was established by stronger means — see below.

## Verified — complete, do not redo

- `manager.rs` removed; `manager/` holds exactly the nine prescribed files.
- Public API byte-identical: **40 public methods in, 40 out**, same names, same signatures.
  Submodules are `mod` (none `pub mod`); the only re-exports are `lifecycle::NewSessionOpts`
  and `tree::TreeNode`. `lib.rs` untouched.
- Verbatim preservation: all sixteen extracted regions appear byte-identical and contiguous in
  their new files; 114 `///` doc lines in and 114 out; the Pi-parity `session-manager.ts:NNNN`
  citation multiset shows zero losses.
- `cargo check --all-targets -p cyrup-session` — exit 0, **0 warnings**.
- `cargo clippy --all-targets -p cyrup-session` — exit 0, **0 findings** for the crate.
- `cargo check --workspace --all-targets` — exit 0, covering all ten crates that consume
  `SessionManager`. No downstream call site needed an edit.
- `cargo test -p cyrup-session` — **157 passed, 0 failed**. Behavior preserved.
- `cargo doc` adds no new warnings; the four that remain predate this work
  (`header.rs` ×2, `compaction/files.rs`, `prompt/builder.rs`) and belong to the queued
  `CARGO_DOC_WARNINGS` task.
- `load` correctly widened to `pub(super)` — required for sibling visibility, minimal, and
  nothing was widened to `pub(crate)`.

## Definition of done

- [ ] `manager/mod.rs:111` banner names the write path rather than `append`.
- [ ] `cargo check --all-targets -p cyrup-session` still clean.
