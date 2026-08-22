---
stage: qa
status: completed
updated: 2026-08-22 23:21
---

# Fix The Two Failing Tests

## Description

Measured 2026-08-22 on `claude/project-build-env-space-d0t3jz`, rustc 1.98.0:

```
cargo nextest run --workspace --no-fail-fast
Summary [135.639s] 7859 tests run: 7857 passed, 2 failed, 8 skipped
```

Neither is caused by the branch it was measured on — that branch touches only `.claude/` and
`.flux/`, no Rust.

## 1. `cyrup-ext-subagents prompt_runtime::tests::a_dropped_flush_future_does_not_wedge_the_steering_inbox`

**Status unknown: regression or flake. Establishing which is the first job.** At PR #30's merge the
same suite was 7858/7859 with only `rpc_cycle_model` failing, so this has appeared since — either a
real regression on `main`, or an intermittent that PR #30's run happened to miss.

What fails is the test's own **precondition**, not its subject:

```
panicked at crates/cyrup-ext-subagents/src/prompt_runtime.rs:2654:13:
the first poll must reach an await for this to exercise a mid-body drop
```

The test polls `inbox.flush()` once by hand with `Waker::noop()` and asserts `Poll::Pending`, then
drops the future mid-body to prove `FlushGuard::drop` releases the re-entrancy latch. When the
first poll returns `Ready` instead, the drop under test never happens — so **a failure here means
the test could not set itself up, not that the latch is broken.**

Nothing guarantees that first poll yields. It depends on whether the directory read reaches a real
await, under a `current_thread` runtime (`#[tokio::test]`'s default). That is a scheduling
assumption, which is a fragile thing to assert.

Determine first whether the product behaviour changed — does a dropped flush still release the
latch? — and only then make the test robust. If the answer is "behaviour is fine, the precondition
is fragile", drive the future to a known await point deterministically rather than assuming one
poll gets there. **Do not weaken the assertion to make it pass**: the latch-release property is
real and worth pinning, and the surrounding doc comment explains why.

## 2. `cyrup-modes tests::modes::rpc_cycle_model_spans_the_full_auth_filtered_registry`

Long-standing and documented (`docs/gap-analysis/13-cyrup-mcp-STATUS.md:107`). Fails on any host
with ambient AWS credentials.

Same root cause as the seven host-dependent tests fixed in PR #30, but those were fixable in place
because their env access was local. This one reaches the env tier through `AuthStore::has_auth`
deep inside the session runtime, so pinning it needs an ambient-credentials override plumbed
across crates.

The pattern to follow is `crates/cyrup-config/src/env_keys.rs`, where an injectable `Ambient` tier
(`Process` vs `Fixed(&HashMap)`) was added behind an unchanged public API — `find_env_keys` /
`get_env_api_key` delegate to `_in` variants. Do the same for `AuthStore`'s env reads rather than
scrubbing the environment around the test, which only works until something runs in parallel.

## Acceptance Criteria

- [ ] The `prompt_runtime` failure is classified as regression or flake, with evidence
- [ ] If a regression: the latch behaviour is fixed and pinned by ablation (break it, watch the test fail)
- [ ] If a flake: the test reaches its await point deterministically, with the latch assertion intact
- [ ] `AuthStore` takes an injectable ambient tier; `rpc_cycle_model` passes with AWS credentials present
- [ ] `cargo nextest run --workspace` is 7859/7859
