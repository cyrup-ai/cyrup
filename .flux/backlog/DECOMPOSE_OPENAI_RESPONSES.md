---
stage: exec
status: done
updated: 2026-08-27
---

# Decompose api/openai_responses.rs Into Submodules


## Base has changed — read before starting

This branch is based on main, which now sets `rustdocflags = ["--document-private-items"]` in
`.cargo/config.toml` and `rustdoc::broken_intra_doc_links = "deny"` workspace-wide. **A broken
intra-doc link is now a build error, not a warning.**

That directly affects this task. When a file is split, doc comments that referenced a sibling item
by bare name (`[`some_helper`]`) stop resolving, because the item is no longer in the same module —
this is exactly what happened to four links in the bedrock split. Repair them as part of the split
by giving the link an explicit path, e.g. ``[`format_bedrock_error`](super::errors::format_bedrock_error)``.
Never delete the reference to silence the error; these are port-fidelity cross-references.

Measured baseline on this branch (use these, not any older numbers):

```
cargo build -p cyrup-provider --all-targets   -> 0 errors, 0 warnings
cargo clippy -p cyrup-provider --all-targets  -> 14 warnings
cargo doc -p cyrup-provider --no-deps         -> 0 warnings
cargo test -p cyrup-provider --lib            -> 1118 pass, 7 ignored, 0 fail
```

Of the 14 clippy warnings, 13 are `return_self_not_must_use` from main's lint policy in files this
task does not touch, and 1 is the deliberately-accepted `result_large_err` on `run_inner`. Do not
"fix" either; just do not add to them.

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

- [x] `openai_responses.rs` is replaced by `openai_responses/` (modules + `tests/`)
- [x] No module exceeds ~350 lines
- [x] Item-name multiset identical before and after (sort, diff -> empty)
- [x] Every file differs from its pre-split content by nothing but the `pub(super)` token, module headers and `use` blocks
- [x] `api/mod.rs` unedited; `azure_openai_responses.rs` (which shares this wire shape) still compiles untouched
- [x] `cargo build -p cyrup-provider --all-targets` — 0 errors, 0 warnings
- [x] `cargo clippy -p cyrup-provider --all-targets` — still 14 warnings, none new in this module
- [x] `cargo doc -p cyrup-provider --no-deps --features faux` — 0 warnings. NOTE: without
      `--features faux` this command already failed on `main` before this task, on an
      untouched file (`src/unconfigured.rs:3` links `[`crate::faux`]`, and `faux` is behind
      `#[cfg(any(test, feature = "faux"))]`); a workspace build unifies that feature on, which
      is how the baseline was measured. Verified pre-existing by re-running the bare command
      against the un-split file. One real cross-module link was repaired by this task:
      `convert.rs` now spells ``[`try_build_params`](super::params::try_build_params)``.
- [x] `cargo test -p cyrup-provider --lib` — 1118 pass, 7 ignored, 0 fail; **39** tests still
      run in this module (the spec's "32" undercounted: the file carries 39 `#[test]` /
      `#[tokio::test]` fns, all present after the split)

## Outcome

`openai_responses.rs` (2,986 lines) became `openai_responses/` — 16 production modules + a 7-file
`tests/` tree, 3,171 lines total. The three `// ---` banner titles became the `//!` headers of the
modules in each group, verbatim.

Sizes: every production module is <= 337 lines (`convert.rs` 337, `events.rs` 305, `params.rs` 228,
rest smaller). One test file misses the ~350 target: `tests/tools.rs` at 565 lines, because the
DRIFT-001 deferred-tool block (517 contiguous lines under one banner whose text reads "Every
assertion below ...") is indivisible and the task's own file list allots no second tools test file.

Purity: item-name multiset identical before/after (105 = 105, diff empty). A line-level multiset
diff of the whole module against the pre-split file shows nothing but the added `pub(super)` /
changed `pub(crate)` tokens, the four signatures that had to wrap once the token pushed them past
100 columns (`emit_error`, `reasoning_summary_wire`, `apply_service_tier_pricing`, `create_slot`),
and the one repaired intra-doc link. No logic was rewritten, reordered or tidied.
