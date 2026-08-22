---
stage: qa
status: completed
updated: 2026-08-22 18:06
---

# Record the No-Panic Lint Blind Spot Inside the str_id! Macro

## Description

The workspace no-panic policy (root Cargo.toml:97-101, `unwrap_used`/`expect_used`/`panic`/`indexing_slicing` = deny) does not reach inside `macro_rules!` expansions for three of its four lints. Verified on rustc/clippy 1.98.0 by injecting probe methods containing `unwrap()`, `expect()`, `panic!` and an index into the `str_id!` body at crates/cyrup-core/src/lib.rs:74-78: only `indexing_slicing` fired (8x, once per invocation); the identical operations in an ordinary `impl` block in the same file produced all three of the other errors. `str_id!` (lib.rs:47-79) is the crate's only macro and emits all eight public id types (lib.rs:82-89), so a reviewer reading the deny list gets false assurance about that body. The cheap, zero-risk fix is a comment recording the verified gap; a structural refactor into a non-macro generic core is optional follow-on work and must not change serde output (`#[serde(transparent)]` at lib.rs:50).

## Evidence

```
Probe inside the str_id! body -> clippy emitted only "error: indexing may panic" x8; identical code in a plain `impl ModelRef` block in the same file emitted "used `unwrap()`", "used `expect()`" and "`panic` should not be present in production code". Toolchain: rustc 1.98.0 / clippy 0.1.98. `grep -rn "macro_rules!" crates/cyrup-core/src/` -> one hit, lib.rs:47. Policy: /home/user/cyrup/Cargo.toml:97-101. All probe edits reverted.
```

## Acceptance Criteria

- [ ] A comment immediately above `macro_rules! str_id` at crates/cyrup-core/src/lib.rs:46 states that `clippy::unwrap_used`, `expect_used` and `panic` do not fire inside macro expansions (verified on rustc/clippy 1.98.0), that only `indexing_slicing` does, and that the body must therefore be kept free of fallible operations.
- [ ] The `str_id!` body contains no `unwrap`, `expect`, `panic!`, or indexing operation after the task (verify by reading lib.rs:47-79).
- [ ] No attempt is made to raise or re-configure the deny levels — the gap is in clippy's from-expansion filtering, not in this crate's configuration.
- [ ] If the optional structural refactor is done, `cargo test -p cyrup-core --lib` still passes including `id_roundtrips_and_displays` (lib.rs:104-110), and `Debug` output for an id is still `ProviderId("anthropic")`.
- [ ] No change to `#[serde(transparent)]` or to any id type's serialized form.

## Provenance

Found by the cyrup-core hygiene audit workflow (2026-08-22), dimension-fanned and adversarially
verified. Severity **medium**, estimated effort **small**.
