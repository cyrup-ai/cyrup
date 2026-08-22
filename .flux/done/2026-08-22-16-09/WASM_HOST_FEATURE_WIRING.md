---
stage: exec
status: done
updated: 2026-08-22 23:21
---

# Make The `wasm-host` Feature Actually Gate Wasmtime

**Crate:** `crates/cyrup-session-svc` · **Severity:** medium · **Effort:** small

## Description

`Cargo.toml:20-23` documents `wasm-host` as "Disable for a native-only build (no wasmtime)" and forwards `wasm-host = ["cyrup-ext/wasm-host"]`, but line 33 takes `cyrup-ext = { workspace = true }` and `crates/cyrup-ext/Cargo.toml:74-75` sets `default = ["wasm-host"]`, so wasmtime arrives regardless: `cargo tree -p cyrup-session-svc --no-default-features -e normal | grep -c wasmtime` prints 39 and the inverted tree names cyrup-ext as the sole parent. The feature therefore gates nothing, while `crates/cyrup-tui/Cargo.toml:20` and `crates/cyrup-it/Cargo.toml:61` both forward to it believing it does. The one-line workspace-inheritance fix is rejected by cargo (`default-features = false` cannot override a workspace entry); the scoped variant was measured and works — spelling line 33 as `cyrup-ext = { path = "../cyrup-ext", version = "0.0.0", default-features = false }` drops the no-default-features tree to 0 wasmtime lines and leaves the default tree at 39. Do not instead flip the root `[workspace.dependencies]` entry: 13 crates inherit it and would all silently lose the wasm host. This is the manifest wiring that currently makes the already-queued BUILD_FEATURE_COMBINATIONS work impossible, not that work itself.

## Acceptance Criteria

- [ ] `crates/cyrup-session-svc/Cargo.toml:33` reads `cyrup-ext = { path = "../cyrup-ext", version = "0.0.0", default-features = false }` and line 23 still forwards `wasm-host = ["cyrup-ext/wasm-host"]`.
- [ ] `cargo tree -p cyrup-session-svc --no-default-features -e normal | grep wasmtime` returns nothing (was 39 lines).
- [ ] `cargo tree -p cyrup-session-svc -e normal | grep -c wasmtime` still returns 39 and `cargo test -p cyrup-session-svc` still reports 311 passing.
- [ ] `git diff` shows no change to the root `Cargo.toml` `[workspace.dependencies]` cyrup-ext entry.
- [ ] The feature comment records the caveat that under a whole-workspace build cargo feature unification still enables `cyrup-ext/wasm-host` if any sibling asks for it — the guarantee holds only for `-p cyrup-session-svc --no-default-features`.

## Findings

Each was produced by a survey agent and then adversarially checked by a separate verifier that tried to refute it. `OVERSTATED` means the finding is real but the verifier corrected its scope or severity — the corrected values are the ones below.

### `wasm-host` forwarding is inert — turning the feature off still links wasmtime

`CONFIRMED` · severity **medium** · effort **medium** · dimension `manifest`

**Evidence.** crates/cyrup-session-svc/Cargo.toml:20-23 documents "Disable for a native-only build (no wasmtime)" but line 33 is `cyrup-ext = { workspace = true }`, and crates/cyrup-ext/Cargo.toml:74-75 sets `default = ["wasm-host"]` pulling `dep:wasmtime, dep:wasmtime-wasi, dep:reqwest, dep:bytes, dep:async-compression, dep:tokio-util`. Measured: `cargo tree -p cyrup-session-svc --no-default-features -e normal | grep -c wasmtime` → 39; the inverted tree names cyrup-ext as the sole parent. With the scoped `default-features = false` on line 33 the same command → 0, and the default-feature tree stays at 39.

**Why it matters.** A feature that documents an outcome it cannot produce is worse than no feature. cyrup-tui (crates/cyrup-tui/Cargo.toml:20) and cyrup-it (crates/cyrup-it/Cargo.toml:61) both forward to `cyrup-session-svc/wasm-host` believing it gates wasmtime, and any future native-only CI job would pass while silently testing the wasm graph.

**Fix.** Take the scoped variant (verified): change crates/cyrup-session-svc/Cargo.toml:33 to `cyrup-ext = { path = "../cyrup-ext", version = "0.0.0", default-features = false }`, keeping `wasm-host = ["cyrup-ext/wasm-host"]` on line 23, which then actually gates the edge. Do NOT take the root `[workspace.dependencies]` flip as the first move — 13 crates inherit that entry and would all lose the host silently. Re-verify with `cargo tree -p cyrup-session-svc --no-default-features -e normal | grep wasmtime` returning nothing and the default tree unchanged. Caveat to record in the manifest comment: under a whole-workspace build, Cargo feature unification will still enable cyrup-ext/wasm-host if any sibling asks for it — the guarantee only holds for a `-p cyrup-session-svc --no-default-features` build. Whether that native-only build then compiles is the already-queued BUILD_FEATURE_COMBINATIONS work; this is the manifest wiring that currently makes that work impossible.
