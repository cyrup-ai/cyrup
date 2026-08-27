---
stage: new
status: pending
priority: MEDIUM
tool: all
source: exec follow-up — observed independently by two agents, then reproduced
updated: 2026-08-27 15:30
---

# `cyrup-permission-system` extension tests flake under default parallelism

## Reproduction

`cargo test -p cyrup-permission-system --lib` fails intermittently. Measured on
this branch: **1 failure in 3 consecutive runs** at default parallelism, and
195/195 clean with `--test-threads=1`.

Two named tests carry it:
- `extension::tests::enabled_switch::the_install_probe_reads_the_same_resolved_config_as_the_enabled_switch`
- `extension::tests::install::auto_materialized_config_does_not_latch_the_gate_on`

This is **pre-existing** and unrelated to any task in this run. It was observed
independently by the powershell executor and the permission-manager executor —
each proved it was not their change — and then reproduced directly.

## Mechanism

`crates/cyrup-permission-system/src/extension/tests/support.rs:74`:

```rust
pub(super) fn without_install_env<T>(body: impl FnOnce() -> T) -> T {
    let _lock = crate::ext_config::env_lock().lock()…;
    let previous = std::env::var(INSTALL_ENV_VAR).ok();
    unsafe { std::env::remove_var(INSTALL_ENV_VAR) };
    let out = body();
    …restore…
}
```

The `env_lock()` mutex serialises callers **that opt into it**. But
`std::env::remove_var` mutates process-global state, so any *other* test running
concurrently that reads `INSTALL_ENV_VAR` without taking the same lock observes
the removed value. The lock protects writers from each other; it does not protect
readers that never acquire it.

The `SAFETY` comment claims the mutation is "serialized by `env_lock`". That is
true only among participants — which is precisely the gap.

## Parity action

Pick ONE; the first is preferred.

1. **Remove the process-global mutation.** Thread the install-env value through
   the resolver as a parameter or a test seam, so no test needs to mutate the
   real environment. This is the only option that makes the tests correct rather
   than merely serialised.
2. Make every reader of `INSTALL_ENV_VAR` acquire `env_lock()`, and state in the
   `SAFETY` comment that the invariant holds only if *all* accessors participate.
3. Last resort: mark the affected tests `#[ignore]` under parallel execution, or
   move them to their own integration binary so they get a dedicated process.

Do not "fix" this by making the whole crate single-threaded in CI — that hides
the race for every future test rather than removing it.

## Definition of done

1. `cargo test -p cyrup-permission-system --lib` passes 20 consecutive runs at
   default parallelism.
2. No test mutates `INSTALL_ENV_VAR` in a way another test can observe, or every
   accessor provably participates in the same lock.
3. The `SAFETY` comment states the real invariant.
4. No production behaviour changes.
