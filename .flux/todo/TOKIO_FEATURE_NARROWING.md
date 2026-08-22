---
stage: new
status: done
updated: 2026-08-22 17:24
---

# Narrow cyrup-core's tokio Dependency to the sync Feature It Uses

## Description

crates/cyrup-core/Cargo.toml:19 and :25 both inherit the full workspace tokio (`rt-multi-thread, macros, sync, fs, process, io-util, time, signal`), but the crate's only non-test tokio path is `use tokio::sync::mpsc;` at src/event_stream.rs:17, and lib.rs:5-6 states the crate does "No I/O, no tokio tasks of its own". The dev-dependency line is byte-identical to the normal one and therefore a no-op. Narrowing requires a direct (non-inherited) declaration: cargo hard-errors on member-side `default-features = false` over an inherited entry. The applied fix shrinks the normal closure from 15 tokio features to `default,sync,time` and drops mio, libc, signal-hook-registry and errno, with an unchanged Cargo.lock and a green test/clippy run. Be honest about the limit: under `resolver = "3"` a workspace build re-unifies the features, so the boundary is only enforced by a per-crate invocation — pair the manifest edit with a guard that someone actually runs.

## Evidence

```
crates/cyrup-core/Cargo.toml:19 and :25 (both `tokio = { workspace = true }`) vs /home/user/cyrup/Cargo.toml:125. Only non-test tokio path: crates/cyrup-core/src/event_stream.rs:17. Before: `tokio v1.52.3|bytes,default,fs,io-util,libc,macros,mio,process,rt,rt-multi-thread,signal,signal-hook-registry,sync,time,tokio-macros`; after the applied-then-reverted fix: `tokio v1.52.3|default,sync,time`, 36 tests pass, clippy exit 0, Cargo.lock byte-identical. Inheritance probe: cargo errors with "`default-features = false` cannot override workspace's `default-features`". Enforcement probe: a `tokio::fs::read` stub fails `cargo check -p cyrup-core` (E0433) but `cargo check --workspace` succeeds under `resolver = "3"` (/home/user/cyrup/Cargo.toml:4).
```

## Acceptance Criteria

- [ ] crates/cyrup-core/Cargo.toml declares tokio directly with `default-features = false, features = ["sync"]` in `[dependencies]` and `["macros", "rt", "sync"]` in `[dev-dependencies]`, with a comment explaining why workspace inheritance cannot express this.
- [ ] The other seven dependencies remain on `workspace = true`.
- [ ] `cargo tree -p cyrup-core -e normal -f "{p}|{f}"` prints `tokio v1.52.3|default,sync,time` with no fs/process/signal/io-util/rt-multi-thread and no mio/libc/signal-hook-registry beneath it.
- [ ] `cargo test -p cyrup-core` passes, `cargo clippy -p cyrup-core --all-targets` exits 0, and `git diff --stat Cargo.lock` is empty.
- [ ] A guard exists so the narrowing is not silently undone: either a `cargo clippy -p cyrup-core --all-targets` step in CI, or a feature-graph test in the style of crates/cyrup-provider/tests/faux_not_in_normal_build.rs that shells out to `cargo tree -p cyrup-core -e normal` and fails if the tokio line contains fs/process/signal/io-util/rt-multi-thread.
- [ ] The task write-up does not claim workspace-wide enforcement — a `cargo check --workspace` still compiles tokio with the union of all members' features.

## Provenance

Found by the cyrup-core hygiene audit workflow (2026-08-22), dimension-fanned and adversarially
verified. Severity **low**, estimated effort **small**.
