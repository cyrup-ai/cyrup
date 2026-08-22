---
stage: new
status: done
updated: 2026-08-22 06:00
---

# Verify The Build Under Non-Default Feature Combinations

## Description

**There are currently no compilation errors under the default feature set.** Measured
2026-08-22, rustc 1.98.0:

```
cargo check --workspace --all-targets
Finished `dev` profile in 1m 35s   # exit 0, 0 errors, 0 warnings
```

That covers all 21 crates plus `xtask` — every workspace member — and `--all-targets` includes
test, bench and example targets, so test code compiles too. The 2 failures in the suite are
runtime failures, not compilation failures; they have their own task (`TEST_FAILURES.md`).

What is **not** covered by that command, and is where compilation errors in this workspace would
actually hide:

1. **Non-default features.** Nine crates declare `[features]`: `cyrup-ext-subagents`, `cyrup-ext`,
   `cyrup-intercom`, `cyrup-it`, `cyrup-provider`, `cyrup-session-svc`, `cyrup-tools`,
   `cyrup-tui`, `cyrup`. `cargo check --workspace` builds exactly one combination of these.

2. **`cyrup-ext --no-default-features` specifically.** `cyrup-ext` sets
   `default = ["wasm-host"]`, and the whole Wasmtime host sits behind it. MCP-037a's verify line
   requires this crate to build **and pass** both with `--features wasm-host` and with
   `--no-default-features` — the bug it pinned existed only in one arm, and a single
   default-feature run is what would have caught it while a single `--no-default-features` run
   would have hidden it. That double run is currently a documented obligation, not an enforced
   one.

3. **The `wasm32-wasip2` target.** `setup.sh` installs it, so something is expected to build for
   it, but nothing in the everyday gate exercises it.

4. **`--all-features`**, which can fail where no single combination does — two features that are
   individually fine and jointly contradictory.

The useful output of this task is a *gate*, not a one-time green run: whatever combinations matter
should be in CI, or they will drift back.

`cargo hack --feature-powerset` is the standard tool if the combination count is manageable;
otherwise pick the combinations that correspond to real shipping configurations and say why the
others are excluded.

## Acceptance Criteria

- [ ] `cyrup-ext` builds and its tests pass under both `--features wasm-host` and `--no-default-features`
- [ ] The other eight feature-bearing crates are checked under their meaningful combinations
- [ ] `wasm32-wasip2` either builds for whatever targets it, or `setup.sh` stops installing it
- [ ] `--all-features` is checked across the workspace
- [ ] Whatever combinations matter are enforced in CI, not just verified once
- [ ] Any errors found are fixed, or recorded with the combination that triggers them
