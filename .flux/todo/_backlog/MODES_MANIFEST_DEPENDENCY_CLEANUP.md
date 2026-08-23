---
stage: new
status: done
updated: 2026-08-22 18:45
---

# Drop Unused tokio-stream and Move async-trait to Dev-Dependencies

## Problem

`crates/cyrup-modes/Cargo.toml` declares two dependencies incorrectly.

**1. `tokio-stream` is unused.** `crates/cyrup-modes/Cargo.toml:26` declares `tokio-stream.workspace = true`, and a recursive grep for `tokio_stream`/`tokio-stream` across the whole crate directory returns exactly one hit — that manifest line itself. The crate has no `build.rs` and no `tests/`, `benches/` or `examples/` directories (only `Cargo.toml` and `src/`), so there is no target that could be consuming it. The streaming code the crate does have goes through `futures::StreamExt` instead: `src/json.rs:30`, `src/print.rs:16`, `src/rpc.rs:33-34`. The entry is dead weight in the dependency graph and misleads readers about what the adapters are built on.

**2. `async-trait` is a production dependency used only by test code.** `crates/cyrup-modes/Cargo.toml:15` puts `async-trait.workspace = true` in `[dependencies]`, but all four use sites are inside the test tree:

- `src/tests/rpc_agent_settled.rs:42`
- `src/tests/rpc_host_seam.rs:197`
- `src/tests/modes/rpc_bash.rs:139`
- `src/tests/modes/rpc_extension_errors.rs:31`

All are reached only through `#[cfg(test)] mod tests;` at `src/lib.rs:29-30`. No non-test module under `src/` mentions `async_trait`. That makes it a production dependency of `cyrup-modes` — and therefore of `crates/cyrup`, `cyrup-sdk` and `cyrup-it`, which all depend on this crate — purely to satisfy test-only trait impls. The workspace already treats the dev-only placement as the rule: `crates/cyrup-tui/Cargo.toml:114` carries an explicit note that `async-trait` *was* a dev-dependency and was promoted only because non-test `src/` code needed it, and `crates/cyrup-ext-subagents/Cargo.toml:135-141` documents the same pattern in reverse (`filetime` kept dev-only precisely because it is used from `src/`-module `#[cfg(test)]` tests).

## Fix

In `crates/cyrup-modes/Cargo.toml`:

1. Delete line 26 (`tokio-stream.workspace = true`). Since Rust 2018 there is no `extern crate` linkage, so an unused `[dependencies]` entry has no compile-time effect and removal cannot change behaviour.
2. Delete line 15 (`async-trait.workspace = true`) from `[dependencies]` and add `async-trait.workspace = true` under `[dev-dependencies]` (after the existing entries around line 35), with a one-line comment in the crate's existing style — e.g. that the RPC seam tests declare `NativeExtension` impls. Dev-dependencies are in scope for a lib's own `#[cfg(test)]` modules, so the four `#[async_trait::async_trait]` attribute sites keep compiling unchanged.

No source file needs to change.

## Acceptance Criteria

- [ ] `crates/cyrup-modes/Cargo.toml` no longer contains `tokio-stream`
- [ ] `async-trait` appears under `[dev-dependencies]` and not under `[dependencies]` in `crates/cyrup-modes/Cargo.toml`, with a one-line rationale comment
- [ ] `cargo check -p cyrup-modes` succeeds (non-test build)
- [ ] `cargo check -p cyrup-modes --all-targets` succeeds
- [ ] `cargo test -p cyrup-modes` compiles and runs with the same result set as before
- [ ] `cargo check -p cyrup -p cyrup-sdk -p cyrup-it` still succeeds (downstream crates unaffected)

## Source

- Identified by the cyrup-modes hygiene audit (workflow `cyrup-modes-hygiene-audit`)
- Severity: medium | Size: small
