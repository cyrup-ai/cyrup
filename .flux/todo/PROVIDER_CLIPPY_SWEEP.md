---
stage: new
status: done
updated: 2026-08-22 22:40
---

# Clear The 36 Zero-Risk clippy Warnings In cyrup-provider

## Description

`cargo clippy -p cyrup-provider --all-targets` reports **37 distinct warnings** (23 from the lib
build, 37 from the lib-test build of which 23 are duplicates), across only **8 lint kinds in 7
files**. 36 of the 37 are mechanical; exactly one should be accepted rather than fixed.

Distribution, re-measured at clean HEAD by the verifier:

| lint | count | where |
|---|---|---|
| `doc_lazy_continuation` | 11 | 2 doc comments |
| `doc_overindented_list_items` | 5 | same 2 doc comments |
| `unnecessary_get_then_check` | 13 | `#[cfg(test)]` assertions |
| `chunks_exact_to_as_chunks` | 4 | `auth/oauth/sha256.rs` 44:27, 69:51, 74:30, 86:46 |
| `field_reassign_with_default` | 1 | test |
| `unnecessary_first_then_check` | 1 | — |
| `collapsible_if` | 1 | — |
| `result_large_err` | 1 | **do not fix** — see below |

**No overlap with CARGO_DOC_WARNINGS.md.** That task is bounded by what
`cargo doc --workspace --no-deps` reports (75 in this crate). These clippy doc-markdown lints are
invisible to a default `cargo doc` run — verified by line: the doc run hits
`openai_codex_responses.rs:13` and `:50`, clippy hits `:699-701`, and the doc run does not touch
`collection.rs` at all.

**`cargo clippy --fix` currently applies ZERO fixes to this crate.** The cause is a single broken
clippy suggestion in `sha256.rs` (E0596 at line 75) that makes cargo fix revert the whole batch.
The failure is loud and self-explanatory, not silent — but it does block bulk fixing, so the four
`sha256.rs` warnings are folded into this task to unblock the rest.

`sha256.rs` is a hand-rolled FIPS 180-4 port and the only file in the workspace carrying a
`#[rustfmt::skip]`. Treat its four warnings with more care than the other 32: change the iteration
form only, never the arithmetic, and confirm the SHA-256 tests still pass.

**Do not fix `result_large_err` on `run_inner`** in `bedrock_converse_stream/driver.rs`. It is a
pre-existing, accepted warning about a large `Err` variant on the internal driver; "fixing" it means
boxing `BedrockFailure`, which is a behaviour-relevant change to a freshly-verified module. Record
the acceptance instead.

## Acceptance Criteria

- [ ] `cargo clippy -p cyrup-provider --all-targets` drops from 37 warnings to exactly 1
- [ ] The 1 remaining is `result_large_err` on `run_inner`, and its acceptance is recorded in a comment at the declaration
- [ ] `cargo clippy --fix -p cyrup-provider --all-targets` completes without reverting (the sha256 blocker is gone)
- [ ] No change to any arithmetic in `auth/oauth/sha256.rs` — only the iteration form
- [ ] `cargo test -p cyrup-provider --lib` — 1118 pass, including the 5 sha256 tests
- [ ] `cargo doc -p cyrup-provider --no-deps` — still exactly 75 warnings (this task must not move that number)
- [ ] `cargo build --workspace` clean
