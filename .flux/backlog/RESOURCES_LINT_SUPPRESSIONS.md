---
stage: new
status: done
updated: 2026-08-22 23:07
---

# Remove Dead Lint Suppressions In cyrup-resources

**Owns files:** `crates/cyrup-resources/src/lib.rs`, `crates/cyrup-resources/src/package/store.rs`,
`crates/cyrup-resources/src/package/git_url.rs`

> `src/discovery.rs` line 1779 is also in scope but that file is owned by
> `RESOURCES_RUSTDOC_WARNINGS` in the same wave — fold the one-line deletion into that task, or run
> the two tasks sequentially. Do not edit `discovery.rs` from here concurrently.

## Description

### 1. Dead `#[allow(clippy::too_many_arguments)]` on `scan_prompt_dir`

Parameter counts were counted directly from the signatures:

| Function | Line | Params | Verdict |
| --- | --- | --- | --- |
| `scan_skill_dir` | 1522 | 8 | allow is **earned** — keep |
| `add_local_entries` | 1670 | 10 | allow is **earned** — keep |
| `scan_prompt_dir` | 1780 | **7** | allow is **dead** — delete |

Clippy's `too_many_arguments` default threshold is 7 and fires only above it, so the allow at
`discovery.rs:1779` suppresses nothing.

**Fix:** delete line 1779 only. Leave 1521 and 1669 alone.

### 2. Dead `#![cfg_attr(not(test), deny(...))]` in `lib.rs:20-28`

It denies `unwrap_used`/`expect_used`/`panic`/`indexing_slicing` for non-test builds. But
`crates/cyrup-resources/Cargo.toml` has `[lints] workspace = true`, and the root `Cargo.toml`
already denies those same four **unconditionally** — test code included. The attribute is a strict
subset of what Cargo already applies, so it changes nothing.

Worse, it actively misleads: it implies test code is exempt from those denies. It is not — which is
exactly why every test module in this crate needs its own `#![allow]` block.

**Fix:** delete lines 20-28, keep `#![forbid(unsafe_code)]` on 19. Replace with a one-line `//!` note
naming `[lints] workspace = true` as the single source of the four denies.

### 3. Inconsistent `#[cfg(test)]` allow lists — resolve the contradiction first

Three in-src test modules spell the same suppression three different ways:

- `package/store.rs:126` — 3 lints, omits `indexing_slicing`
- `package/git_url.rs:937-942` — 4 lints, multi-line
- `package/manifest.rs:687` — 4 lints, single line

The audit produced **two conflicting proposals** here and neither was verified. Do not do both:

- **(a) Narrow** each list to only the lints its module actually trips.
- **(b) Unify** all of them on the four-lint multi-line form used by `src/tests/resources/mod.rs`.

**Decide (b).** Narrowing makes each site a puzzle ("why does this one differ?") and re-breaks the
moment someone adds an `unwrap` to a test. Uniformity is the hygiene win; the cost of listing a lint
that happens not to fire today is zero.

## Acceptance Criteria

- [ ] `discovery.rs:1779` deleted; the allows at 1521 and 1669 untouched
- [ ] `lib.rs` `cfg_attr` block gone, `#![forbid(unsafe_code)]` retained
- [ ] The three in-src test-module allow lists are byte-identical to each other, four lints, in the
      order `unwrap_used, expect_used, panic, indexing_slicing`
- [ ] `cargo clippy -p cyrup-resources --all-targets` reports no NEW findings
- [ ] `cargo test -p cyrup-resources` unchanged
