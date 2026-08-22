---
stage: qa
status: needs-rework
updated: 2026-08-22 18:06
---

# Guard cyrup-core's Narrowed tokio Feature Set Against Silent Re-Widening

## What is already done

The narrowing itself landed. `crates/cyrup-core/Cargo.toml` now declares tokio directly as
`default-features = false, features = ["sync"]` (dev: `["macros", "rt", "sync"]`) instead of
`workspace = true`, with a comment recording why inheritance cannot express it (cargo hard-errors
on `default-features = false` overriding a workspace default) and that the bound applies to
per-crate builds only. Verified: `cargo tree -p cyrup-core -e normal -f "{p}|{f}"` prints
`tokio v1.52.3|default,sync,time` with no fs/process/signal/io-util/rt-multi-thread, `Cargo.lock`
is unchanged, and `cargo test -p cyrup-core` / `cargo clippy -p cyrup-core --all-targets` are clean.

## What remains — the AC5 guard, and why it was not done here

Nothing stops the next person from restoring `tokio = { workspace = true }` and silently pulling
the union of every member's features back in. The original AC asked for a guard. It was not added,
deliberately:

- A guard must observe the **Cargo feature graph**, which is resolved before any Rust is parsed, so
  no `#[cfg(test)]` unit test can express it. It has to shell out to `cargo tree`.
- That makes it an integration test, and `docs/TEST-ARCHITECTURE.md` §0 records a maintainer
  decision that **every crate keeps unit tests only**, with integration tests living in the single
  `crates/cyrup-it` crate. Adding `crates/cyrup-core/tests/` would violate that decision.
- The correct home is therefore a `[[test]]` target in `crates/cyrup-it`, modelled on
  `crates/cyrup-it/tests/bin/faux_not_in_normal_build.rs` — which is the same instrument class
  (`cargo tree --offline --locked`, a `CYRUP_SKIP_CARGO_GRAPH_TESTS` opt-out, and a loud failure
  when the instrument itself breaks rather than a silent pass).
- **Blocked on `CYRUP_IT_COMPILE_ERRORS`**: cyrup-it does not currently compile under
  `--features it`, so a guard added there would not run until that task is finished.

## Acceptance Criteria

- [ ] A feature-graph guard lives in `crates/cyrup-it` as part of an existing `[[test]]` target,
      not in a new `crates/cyrup-core/tests/` directory.
- [ ] It asserts `cargo tree -p cyrup-core -e normal` reports tokio WITHOUT `fs`, `process`,
      `signal`, `io-util` or `rt-multi-thread`, and fails loudly (not silently passes) if the
      `cargo tree` invocation itself errors.
- [ ] It follows `faux_not_in_normal_build.rs`'s opt-out convention (`CYRUP_SKIP_CARGO_GRAPH_TESTS`,
      affirmative values only) and documents in its module doc that the bound is per-crate, since a
      `--workspace` build still unifies tokio's features across members.
- [ ] The guard is demonstrated RED by temporarily restoring `tokio = { workspace = true }` in
      cyrup-core's manifest, then GREEN with the narrowing back in place.
- [ ] `CYRUP_IT_COMPILE_ERRORS` is resolved first, or this task explicitly records that the guard is
      committed but not yet executable.

## Provenance

Found by the cyrup-core hygiene audit workflow (2026-08-22). The narrowing was implemented by the
remediation workflow the same day; this file is the residual guard work only.
