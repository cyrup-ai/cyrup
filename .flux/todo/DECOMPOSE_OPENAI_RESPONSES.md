---
stage: new
status: done
updated: 2026-08-22 22:40
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

- [ ] `openai_responses.rs` is replaced by `openai_responses/` (modules + `tests/`)
- [ ] No module exceeds ~350 lines
- [ ] Item-name multiset identical before and after (sort, diff -> empty)
- [ ] Every file differs from its pre-split content by nothing but the `pub(super)` token, module headers and `use` blocks
- [ ] `api/mod.rs` unedited; `azure_openai_responses.rs` (which shares this wire shape) still compiles untouched
- [ ] `cargo build -p cyrup-provider --all-targets` — 0 errors, 0 warnings
- [ ] `cargo clippy -p cyrup-provider --all-targets` — still 14 warnings, none new in this module
- [ ] `cargo doc -p cyrup-provider --no-deps` — still **0** warnings (broken links are denied, so any new one is a build error)
- [ ] `cargo test -p cyrup-provider --lib` — 1118 pass, 32 tests still run in this module
