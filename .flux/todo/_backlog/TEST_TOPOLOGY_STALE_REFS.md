---
stage: exec
status: done
updated: 2026-08-22 23:45
---

# Repoint Four Stale References To The Migrated tests/ Directory, Drop One Dead Dev-Dependency

> Source: `intercom-hygiene-audit` workflow. Severity **medium**, effort **small**.
> Every claim below was produced by a finder agent and then reproduced by an independent
> adversarial verifier; findings that did not reproduce were dropped.

## Scope

- `crates/cyrup-intercom/Cargo.toml`
- `crates/cyrup-intercom/src/tests/mod.rs`
- `crates/cyrup-intercom/src/bin/cyrup-intercom-broker.rs`

## Description

The crate's `tests/` directory was migrated to `crates/cyrup-it/tests/intercom/` (commits c3982b5,
63d729a). Four separate places in `cyrup-intercom` still describe the pre-migration world, and one
of them is a genuinely dead build edge rather than just stale prose.

This is filed as **one task** because all four references live in the same two files and the same
migration explains all of them. The audit surfaced them as four findings across two dimensions;
splitting them into four tasks would mean four agents editing `Cargo.toml` concurrently.

### The four references

| Where | Says | Truth |
|---|---|---|
| `Cargo.toml:80-86` | `cyrup-permission-system` dev-dep exists for `tests/shared_human_lock.rs` | That test is at `crates/cyrup-it/tests/intercom/`, and `cyrup-it/Cargo.toml:101` already declares its own copy. **This dev-dep is dead.** |
| `Cargo.toml:15-22` | `test-fixtures` supports "this crate's `tests/child_bridge_activation.rs`", opted into by "this crate's own `cargo test --features test-fixtures`" | Test moved; the real build edge is `crates/cyrup-it/build.rs:70`. The feature and `[[bin]]` are **live**, just misattributed. |
| `src/tests/mod.rs:6-7` | seam tests "remain under `tests/`" | They live in `crates/cyrup-it/tests/intercom/`. |
| `src/bin/cyrup-intercom-broker.rs:5-6` | cites "the `tests/broker_roundtrip.rs` integration proof" | Also relocated. |

### Verified dead, not merely suspected

`grep -rn 'cyrup.permission.system' crates/cyrup-intercom/` returns exactly one hit: the declaration
itself. `cargo metadata --no-deps` shows the crate's targets are lib + two bins with **no target of
kind `test`**, so nothing in this crate can reach a dev-dependency. The verifier commented out the
line and ran `cargo check -p cyrup-intercom --all-targets` to completion, then restored the file.

It is also the only `cyrup-*` dependency declared as a bare `path = ...` rather than
`workspace = true`, so it resolves as `req = "*"` instead of the workspace pin.

## Evidence

- `sed -n '78,86p' crates/cyrup-intercom/Cargo.toml` — line 80 begins `# The C3 cross-companion proof (\`tests/shared_human_lock.rs\`, reconciliation §1 / §4 step 6)`; line 86 is `cyrup-permission-system = { path = "../cyrup-permission-system" }`
- `find . -name 'shared_human_lock*' -not -path '*/target/*'` → exactly one hit: ./crates/cyrup-it/tests/intercom/shared_human_lock.rs
- `find crates/cyrup-intercom -type d -name tests` → only crates/cyrup-intercom/src/tests; `ls` of it → mod.rs, protocol_number_overflow.rs, protocol_residual_parity.rs
- `grep -rn 'cyrup.permission.system' crates/cyrup-intercom/ | wc -l` → 1, and that hit is crates/cyrup-intercom/Cargo.toml:86 (the declaration itself)
- `cargo metadata --format-version 1 --no-deps` for cyrup-intercom → TARGETS: ['lib'] cyrup_intercom, ['bin'] cyrup-intercom-broker, ['bin'] cyrup-intercom-child-fixture (required-features ['test-fixtures']); DEV DEPS: cyrup-permission-system (path, req `*`), tempfile ^3.27.0, tokio ^1. No target of kind 'test'.
- `cargo tree -p cyrup-intercom -e normal | grep -c cyrup-permission-system` → 0; `cargo tree -p cyrup-intercom -e dev --depth 1` lists cyrup-permission-system, tempfile, tokio
- `find crates/cyrup-permission-system/src -name '*.rs' | xargs wc -l | tail -1` → 17142 total
- crates/cyrup-it/Cargo.toml:101 — `cyrup-permission-system = { workspace = true }` (the live consumer)
- `grep -n '^cyrup-' crates/cyrup-intercom/Cargo.toml` → 26 cyrup-core, 27 cyrup-ext, 30 cyrup-ext-subagents, 35 cyrup-resources (all `workspace = true`), 86 cyrup-permission-system (bare path) — enumeration of every cyrup-* dep in the file
- /home/user/cyrup/Cargo.toml:118 — `cyrup-permission-system = { path = "crates/cyrup-permission-system", version = "0.0.0" }`, the workspace pin :86 bypasses
- Live dev-deps for contrast: crates/cyrup-intercom/src/connect.rs:704 / cwd.rs:100 / broker/listener.rs:218 use `tempfile::tempdir()`; connect.rs:750 uses `#[tokio::test(start_paused = true)]` with `tokio::time::advance` at :758
- crates/cyrup-intercom/Cargo.toml:15-16 — "used by this // crate's `tests/child_bridge_activation.rs` production-activation proof" (verified via `sed -n '15,22p' | cat -n`)
- crates/cyrup-intercom/Cargo.toml:22 — "# (this crate's own `cargo test --features test-fixtures`) explicitly opts in."
- `cargo metadata --format-version 1 --no-deps` for cyrup-intercom → targets ['lib'] cyrup_intercom, ['bin'] cyrup-intercom-broker, ['bin'] cyrup-intercom-child-fixture with required-features ['test-fixtures']; no 'test' kind target

## Acceptance Criteria

- [ ] `Cargo.toml:80-86` deleted — both the `cyrup-permission-system` dev-dep and its rationale
      comment. Do **not** relocate the comment text: `crates/cyrup-it/tests/intercom/shared_human_lock.rs:1-19`
      already carries a fuller version of the same C3 argument.
- [ ] `Cargo.toml:15-22` repointed to `crates/cyrup-it/tests/intercom/child_bridge_activation.rs`
      and `crates/cyrup-it/build.rs:70`. Keep every other sentence, including the never-shipped
      guarantee — `cargo metadata` confirms the `required-features` gate still holds.
- [ ] `src/tests/mod.rs:6-7` names `crates/cyrup-it/tests/intercom/`. The two-tier rationale it
      draws is still true; only the location is wrong.
- [ ] `src/bin/cyrup-intercom-broker.rs:5-6` repointed to `crates/cyrup-it/tests/intercom/broker_roundtrip.rs`.
- [ ] `tempfile` and `tokio` dev-deps left alone — both are exercised by in-crate `#[cfg(test)]`
      modules (`connect.rs:704`, `cwd.rs:100`, `broker/listener.rs:218`, and
      `#[tokio::test(start_paused = true)]` at `connect.rs:750`).
- [ ] `cargo check -p cyrup-intercom --all-targets` and `cargo test -p cyrup-intercom --lib` still green.
- [ ] `cargo build -p cyrup-it --features it` still resolves the fixture binary.
