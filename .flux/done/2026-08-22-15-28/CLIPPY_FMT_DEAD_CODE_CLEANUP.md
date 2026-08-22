---
stage: qa
status: completed
updated: 2026-08-22 21:05
---

# Clear cyrup-config's Clippy Warnings, rustfmt Drift, And Dead Field

## Description

Three small, purely mechanical hygiene items. All verified against real command output; none change
behavior.

### 1. Three clippy warnings

`cargo clippy -p cyrup-config --all-targets` reports `cyrup-config (lib) generated 3 warnings` and
`(lib test) generated 3 warnings (3 duplicates)` — no extra warnings in test targets. (The run also
compiles path-dependencies, so unrelated lines like `cyrup-provider (lib) generated 23 warnings`
appear; those are not this crate.)

```
src/config_value.rs:556:5   match can be simplified with `.unwrap_or_default()`
src/model/validate.rs:67:20  redundant guard
src/model/validate.rs:207:24 redundant guard
```

The two `validate.rs` hits sit inside `src/model/` but are ordinary lint fixes, not decomposition
work, so they belong here.

### 2. `FileLock::path` is dead and silenced with an unexplained `#[allow]`

[`src/lock.rs:14-18`](../../crates/cyrup-config/src/lock.rs):

```rust
pub struct FileLock {
    file: File,
    #[allow(dead_code)]
    path: PathBuf,
}
```

Nothing reads `path`. The `#[allow(dead_code)]` is unconditional on production code and carries no
rationale — contrast `src/env_keys.rs:33`, where a `#[cfg_attr(not(test), allow(dead_code))]` is
preceded by three lines explaining itself (`env_keys.rs:30-32`).

The case for deleting is hygienic, not performance: `lock_path` is built unconditionally at
`lock.rs:25` because `OpenOptions::open(&lock_path)` at `:31` needs it, so the `PathBuf` is
allocated either way — the field only keeps the buffer alive for the lock's lifetime. What deletion
buys: the `lock_path.clone()` at `lock.rs:32` (error path only, inside
`map_err(|_| ConfigError::Lock { path: lock_path.clone() })`) becomes a move, and a correct compiler
diagnostic stops being suppressed.

### 3. rustfmt drift: 19 hunks across 5 files

`cargo fmt -p cyrup-config -- --check | grep -c '^Diff in'` returns **19**:

```
models_store.rs  9  (:355 :390 :406 :413 :424 :441 :497 :504 :581)
login.rs         4  (:297 :793 :818 :1664)
keybindings.rs   3  (:336 :355 :500)
env.rs           2  (:524 :571)
env_keys.rs      1  (:370)
```

Sample: the `login.rs:297` hunk is a stray double blank line — the offending blank is physically
line 300 (`:297` is rustfmt's context anchor).

For context, cyrup-config is already the second-cleanest of the workspace's 22 crates by hunk count
(cyrup-sdk 7, **cyrup-config 19**, xtask 22, … cyrup-tui 2688, cyrup-ext-subagents 2768). Nineteen
hunks is a ten-minute fix that makes this crate the cleanest in the repo.

`cargo fmt -p cyrup-config -- --check | grep -c 'src/model/'` returns **0** — the PR #38
decomposition is already fmt-clean and out of scope.

## Acceptance Criteria

- [ ] `cargo clippy -p cyrup-config --all-targets` reports no warnings attributed to `cyrup-config`
- [ ] `cargo fmt -p cyrup-config -- --check` exits 0 with no `Diff in` output
- [ ] `grep -n 'allow(dead_code)' crates/cyrup-config/src/lock.rs` returns no match, and `path: PathBuf` is gone from `struct FileLock`
- [ ] `crates/cyrup-config/src/lock.rs:32` uses `lock_path` by move rather than `lock_path.clone()`
- [ ] `cargo build -p cyrup-config` and `cargo test -p cyrup-config` show no failures beyond those in `TEST_FAILURES.md`

## Outcome — completed

Landed in `9ae42e8`, merged to `main` via #46 (squashed into `7e221a3`).

All five acceptance criteria met: `cargo clippy -p cyrup-config --all-targets` reports **0** warnings
for this crate (was 3), `cargo fmt -p cyrup-config -- --check` exits clean with **0** hunks (was 19),
and `FileLock`'s dead `path` field is gone along with the unexplained `#[allow(dead_code)]`, with
`lock.rs`'s error path now moving `lock_path` rather than cloning it.

Running last in the remediation chain was load-bearing: three earlier tasks had edited the crate by
the time this ran, so the "19 hunks across 5 files" figure was already stale. The fix was to run
`cargo fmt` and verify `--check` clean, not to chase the recorded line numbers.

Every falsifiable claim in this task file checked out against the tree when it was written.
