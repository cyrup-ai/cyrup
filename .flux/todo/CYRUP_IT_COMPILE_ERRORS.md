---
stage: new
status: done
updated: 2026-08-22 14:00
---

# Fix All Errors In `cyrup-it --features it`

## Description

fix all errors in `cyrup-it --features it`

The integration-test crate does not compile under the feature that arms it. Discovered on
2026-08-22 while verifying an unrelated change to `crates/cyrup-it/tests/mcp/activation.rs`, which
could not be checked any other way.

### Why it went unnoticed

Every `cyrup-it` `[[test]]` target is `required-features = ["it"]`, and that feature is OFF by
default — deliberately, so the merge gate (`cargo test --workspace`) does not build, link or run
any of them. `cargo check --workspace --all-targets` therefore reports clean while these targets
have drifted into not building at all. Nothing in the everyday gate touches them.

This is a concrete instance of the class `BUILD_FEATURE_COMBINATIONS.md` predicts; that task should
probably absorb the CI half of the fix rather than duplicating it here.

### What was measured

`cargo check -p cyrup-it --features it --all-targets`, output truncated at 8 lines — **the list
below is partial and the first job is to enumerate it fully.** Every error seen so far is one
family: struct literals that have fallen behind fields added to the types they construct.

| file | line(s) | missing |
|---|---|---|
| `tests/subagents/background_cascade_integration.rs` | 203 | `usage_budget` on `RunnerConfig` |
| `tests/subagents/background_runner_main_integration.rs` | 220, 327, 470, 590 | `usage_budget` on `RunnerConfig` |
| `tests/subagents/child_protocol_stream_integration.rs` | 126 | `steer_ack_dir`, `steer_capability_path`, `usage_budget` on `RunOptions` |
| `tests/subagents/child_stderr_drain_integration.rs` | 125 | same three on `RunOptions` |
| `tests/subagents/child_written_output_authorship.rs` | 110 | same three on `RunOptions` |

All of it in `tests/subagents/`. The `mcp` target compiles clean on its own
(`cargo check -p cyrup-it --features it --test mcp`), so the breakage may be confined to one
target — worth confirming per-target before assuming otherwise.

### Note on the fix

These are integration tests asserting real behaviour, so a missing field is a question, not a
mechanical fill-in: whoever added `usage_budget`, `steer_ack_dir` and `steer_capability_path`
chose defaults for production, and each test needs the value that keeps its assertion meaningful.
`..Default::default()` would compile and could silently change what several of these tests prove.
Read each test's intent before choosing.

## Acceptance Criteria

- [ ] `cargo check -p cyrup-it --features it --all-targets` compiles clean — full enumeration, not just the five files above
- [ ] Each added field carries the value that preserves that test's assertion, not a blanket default
- [ ] `cargo nextest run -p cyrup-it --features it` runs; any test that then FAILS is triaged and recorded rather than left silent
- [ ] The gate that would have caught this is added, or the work is explicitly handed to `BUILD_FEATURE_COMBINATIONS.md`
- [ ] No production source changes — this is test-side drift
