---
stage: qa
status: completed
updated: 2026-08-23 01:10
---

# Move The 25 Host-Target Ergonomic Tests Into cyrup-ext-sdk And Delete The False Pointer In src/tests/mod.rs

**Severity:** high · **Effort:** M · **Crate:** `crates/cyrup-ext-sdk`

## What is wrong

Two problems in one file, fixed by one move.

**(a) The SDK's behavioural test suite never runs in the merge gate.** `crates/cyrup-it/tests/ext/ergonomic.rs` holds 25 `#[test]`s (`grep -c '#\[test\]' crates/cyrup-it/tests/ext/ergonomic.rs` → 25) covering the ergonomic guest layer: subscription bitset, typed event dispatch, outcome lowering, guest tool/command execution, provider and autocomplete surfaces. Its header reads "Host-target unit tests for the ergonomic guest layer (arch-08 §3.6)" and it touches no wasm runtime. It is compiled only via `mod ergonomic;` at `crates/cyrup-it/tests/ext/main.rs:74`, inside the `[[test]] name = "ext"` target whose `required-features = ["it"]` (`crates/cyrup-it/Cargo.toml:190`) while `default = []` (`Cargo.toml:36`). So `cargo test -p cyrup-ext-sdk` (17 tests), `cargo test -p cyrup-ext` (293) and `cargo test --workspace` all pass without executing any of the 25. Baseline accounting: `rg -c '#\[test\]' crates/cyrup-ext-sdk/src/` → payload_fidelity 11, world_import_coverage 3, dialog_options_timeout 1, widget.rs 2 = the 17. Note `.flux/todo/CYRUP_IT_COMPILE_ERRORS.md` restores `--features it` compilation but does not turn `it` on for the default suite, so it does not fix this.

**(b) The crate's own test module asserts the opposite, pointing at a path that does not exist.** `crates/cyrup-ext-sdk/src/tests/mod.rs:7-9` says: "`tests/ergonomic.rs` deliberately STAYS external: it exercises the SDK strictly through the public API surface an extension author sees…". There is no crate-level `tests/` directory — `ls crates/cyrup-ext-sdk` → `Cargo.toml src wit`, and `git ls-files 'crates/cyrup-ext-sdk/tests/*' | wc -l` → 0. The file was moved out of the crate entirely: `git log --oneline --diff-filter=D -- crates/cyrup-ext-sdk/tests/ergonomic.rs` → `c3982b5`, and `git show --stat c3982b5 | grep ergonomic` → `.../tests => cyrup-it/tests/ext}/ergonomic.rs`. Meanwhile `crates/cyrup-it/tests/ext/main.rs:68-73` says the opposite in prose — that ergonomic.rs "is the ONLY module in this target that touches no wasm runtime at all" and "the first one to move into `crates/cyrup-ext-sdk/src` as a `#[cfg(test)]` module". Two in-tree comments give contradictory instructions about the same file.

Note `src/tests/mod.rs:1` ("relocated from `crates/cyrup-ext-sdk/tests/`") is past tense and historically accurate — only :7-9 is false.

## Why it matters

A regression in `ExtensionApi::dispatch` or `guest::run_tool` ships green today, and a maintainer reading the crate is told the coverage is deliberately elsewhere at a path they cannot find. Rustdoc never sees this file (`#[cfg(test)] mod tests`, lib.rs:35-36), so `.flux/todo/CARGO_DOC_WARNINGS.md` will not surface it.

## Fix

1. Move `crates/cyrup-it/tests/ext/ergonomic.rs` → `crates/cyrup-ext-sdk/src/tests/ergonomic.rs`, rewriting `use cyrup_ext_sdk::…` → `use crate::…` exactly as `src/tests/mod.rs:11-12` records was done for the three files already moved. The public-API-surface property survives: the tests go through `cyrup_ext_sdk::prelude::*` (ergonomic.rs:7), the same re-export module an author uses, which becomes `crate::prelude::*`.
2. Keep the file's existing `#![allow(clippy::unwrap_used, …)]` header (ergonomic.rs:5) — workspace lints deny `unwrap_used`/`expect_used`/`panic`/`indexing_slicing`.
3. Add `mod ergonomic;` to `crates/cyrup-ext-sdk/src/tests/mod.rs` (after line 16).
4. Delete `mod ergonomic;` at `crates/cyrup-it/tests/ext/main.rs:74` and the 6-line curation note above it (:68-73).
5. **Do NOT drop the `cyrup-ext-sdk` dev-dependency from `crates/cyrup-it/Cargo.toml`** — the rationale block at :104-115 records that `tests/ext/build_tier1.rs` also uses it as the authored-extension crate.
6. Rewrite `crates/cyrup-ext-sdk/src/tests/mod.rs:7-9` to describe reality: in-crate unit tests over the guest event payload structs and the ergonomic layer, no external `tests/` directory. Keep the reason an author-surface test drives `crate::prelude::*` rather than private items.
7. While moving, rename `all_thirty_events_are_registerable` (ergonomic.rs:372) and its stale comment at :373 — the assertion at :382 is `kinds.len() == 33`, and `mod kind` (api.rs:21-64) defines 33 discriminants. Neither is thirty.

## Acceptance Criteria

- [ ] `crates/cyrup-ext-sdk/src/tests/ergonomic.rs` exists and `grep -n 'mod ergonomic' crates/cyrup-ext-sdk/src/tests/mod.rs` matches
- [ ] `cargo test -p cyrup-ext-sdk` reports at least 42 passing tests (17 baseline + 25 moved)
- [ ] `grep -rn 'ergonomic' crates/cyrup-it/tests/ext/main.rs` returns nothing
- [ ] `grep -n 'STAYS external' crates/cyrup-ext-sdk/src/tests/mod.rs` returns nothing, and no line of that file references a `crates/cyrup-ext-sdk/tests/` path in the present tense
- [ ] `grep -rn 'cyrup-ext-sdk' crates/cyrup-it/Cargo.toml` still shows the dev-dependency (build_tier1.rs depends on it)
- [ ] `grep -rn 'all_thirty' crates/cyrup-ext-sdk/src` returns nothing
- [ ] `cargo clippy -p cyrup-ext-sdk --all-targets` reports 0 warnings
- [ ] `cargo check -p cyrup-ext-sdk --target wasm32-wasip2` reports 0 warnings, 0 errors
