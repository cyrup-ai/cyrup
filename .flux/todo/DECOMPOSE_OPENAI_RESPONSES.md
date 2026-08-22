---
stage: new
status: done
updated: 2026-08-22 22:40
---

# Decompose api/openai_responses.rs Into Submodules

## Description

`crates/cyrup-provider/src/api/openai_responses.rs` is 3,057 lines (1,727 production + 1,330 test).
One `mod tests` at L1747 with 32 `#[test]` fns.

The verifier re-measured banners, item map, item sizes, impl contiguity and the pub-item count and
found them all exact — the only drift was off-by-one noise. The seams are clean and no production
item is large enough to force a judgement call.

**Coordination note:** `provider_env_value` at L293 in this file is `pub(crate)` and is one of the
5 byte-identical copies targeted by PROVIDER_SHARED_PLUMBING.md. Do not run both tasks on this file
concurrently.

## Reference

Follow `crates/cyrup-provider/src/api/bedrock_converse_stream/` exactly. Read it first. Required
properties: pure code movement; port-fidelity comments move verbatim; **never run `cargo fmt`**;
visibility minimized by strip-and-restore under the compiler; banner titles become `//!` headers.

## Acceptance Criteria

- [ ] `openai_responses.rs` is replaced by `openai_responses/` (modules + `tests/`)
- [ ] No module exceeds ~350 lines
- [ ] Item-name multiset identical before and after (sort, diff -> empty)
- [ ] Every file differs from its pre-split content by nothing but the `pub(super)` token, module headers and `use` blocks
- [ ] `api/mod.rs` unedited; `azure_openai_responses.rs` (which shares this wire shape) still compiles untouched
- [ ] `cargo build -p cyrup-provider --all-targets` — 0 errors, 0 warnings
- [ ] `cargo clippy -p cyrup-provider --all-targets` — still 37 warnings, none new here
- [ ] `cargo doc -p cyrup-provider --no-deps` — still 75 warnings, none new here
- [ ] `cargo test -p cyrup-provider --lib` — 1118 pass, 32 tests still run in this module
