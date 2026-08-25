---
severity: low
file: crates/cyrup-ext-subagents/src/discovery/management.rs
lines: 209-215
introduced: true
---

# Stale "duplicated rather than imported" comment now contradicts the code three lines below it

## Problem
This PR added `use super::package_name::{collapse_repeated_char, is_valid_package_identifier};`
to `management.rs` (line ~71) and deleted the module's local copies of those two functions,
replacing them with imports from the new `discovery/package_name.rs`. However the block comment
directly above `normalize_package_identifier` (lines 209-215) was left unchanged and still reads:

> Package-identifier validation (mirrors `frontmatter.rs::parse_package_name`'s validation
> grammar exactly, R-SA-006). **Duplicated locally (rather than importing a private helper from
> `frontmatter.rs`) since this module owns its own file and must not require edits to
> `frontmatter.rs` to build**; the two implementations are each unit-tested against the same
> fixture set to guard against drift.

This is now factually wrong: the module *does* import a helper (from `package_name.rs`, not
`frontmatter.rs`, but the "we duplicate rather than import" premise is exactly what this PR
retired). The new `package_name.rs` module doc even explicitly calls out this exact rationale as
retired:

> `discovery/management.rs` used to carry its own copy with a comment defending the duplication
> ("this module owns its own file and must not require edits to `frontmatter.rs` to build"). That
> rationale is retired...

but the old comment it's describing as retired was never actually deleted from `management.rs`.

The matching test comment at lines ~4154-4157 has the same problem:

```rust
#[test]
fn normalize_package_identifier_matches_frontmatter_rs_validation_fixtures() {
    // Same fixture set `frontmatter.rs`'s own tests pin, to guard the two duplicated
    // implementations against drift (see this module's header note on why the validator is
    // duplicated rather than imported).
```

`collapse_repeated_char`/`is_valid_package_identifier` are no longer duplicated — they're shared
via `package_name.rs`. Only the outer normalize/collapse-whitespace sequencing in
`normalize_package_identifier` itself remains separately written out (matching
`frontmatter.rs::parse_package_name`'s inline copy, and `chains.rs`'s `normalize_package_name`
before dedup — the exact case `package_name.rs`'s own doc comment says was deliberately left as
three differently-shaped callers). The comments' framing ("duplicated ... imported") no longer
matches what actually happens.

## Evidence
`crates/cyrup-ext-subagents/src/discovery/management.rs:68-71` (added by this PR):
```rust
use super::package_name::{collapse_repeated_char, is_valid_package_identifier};
```
directly contradicts the unchanged comment at `management.rs:209-215` three lines below it, and
the stale test comment at `management.rs:4154-4157`.

## Impact
Low risk to behavior (compiles fine, no functional bug), but a maintainer reading this comment
will be misled into thinking the validator logic is fully duplicated and must be kept in sync by
hand across two files, when in fact `collapse_repeated_char`/`is_valid_package_identifier` are
now single-sourced in `package_name.rs`. This is exactly the kind of stale rationale comment that
causes future contributors to re-introduce duplication "because the comment said to."

## Suggested Fix
Update the header comment (lines 209-215) and the test comment (lines 4154-4157) to reflect the
current state: `collapse_repeated_char`/`is_valid_package_identifier` are imported from
`super::package_name`; only the outer normalize-sequence in `normalize_package_identifier` is
still written out locally (to preserve `Option`-shaped error handling per
`package_name.rs`'s own doc comment on why three callers exist). Consider pointing the "why isn't
this fully unified too" explanation at `package_name.rs`'s module doc instead of re-stating a
rationale that no longer applies.
