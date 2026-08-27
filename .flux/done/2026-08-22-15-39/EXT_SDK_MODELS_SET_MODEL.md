---
stage: qa
status: completed
updated: 2026-08-23 01:10
---

# Expose Models::set_model And Fix ctx/models.rs's Stale "set_model Is Command-Tier" Claim

**Severity:** medium · **Effort:** S · **Crate:** `crates/cyrup-ext-sdk`

## What is wrong

`sed -n '1,2p' crates/cyrup-ext-sdk/src/ctx/models.rs` prints:

> The `models` WIT import: the model registry view. `set_model` is command-tier and lives in `command`; `set_thinking_level` is not tier-gated (EXT-074) and lives here.

The first half is false, and three in-tree sources say so:

1. `crates/cyrup-ext-sdk/src/ctx/base.rs:58-62`, on `Ctx::models()`: "EXT-074: `set_model`/`set_thinking_level` are NOT command-only — this line said they were, and the host dropped that gate at GAP-11".
2. `crates/cyrup-ext-sdk/wit/world.wit:778-786`, above `set-model: func(model-json: string);`: "Callable from ANY tier, matching pi … EXT-074: this line read 'COMMAND-only at the host' … both were STALE".
3. The host itself, `crates/cyrup-ext/src/host/live.rs:565-577`: `async fn set_model` opens `let Ok(guest) = guest_of(self) else { return };` with no tier check and an explicit GAP-11 comment ("We therefore do NOT gate this on the command tier").

This text is **new**, not inherited rot: `git log --oneline -3 -- crates/cyrup-ext-sdk/src/ctx/models.rs` → only `725b047` "Decompose cyrup-ext-sdk ctx.rs into per-interface submodules", and `git show HEAD~1:crates/cyrup-ext-sdk/src/ctx.rs | grep -n 'command-tier'` shows no such claim in the predecessor.

The gap is behavioural too, not just documentary. `rg -n 'set_model' crates/cyrup-ext-sdk/src` shows the only SDK wrapper is `CommandCtx::set_model` at `src/ctx/command.rs:162`; `Models` has none. The crate's own bundled reference extension proves the consequence at `src/example.rs:251-260`: a four-line apology ("The ergonomic SDK exposes `set_model` only on `CommandCtx`; to exercise the HOST's event-tier `set_model` import we call the raw WIT binding directly") followed by `crate::guest::bindings::cyrup::ext::models::set_model(...)` under a `#[cfg(target_arch = "wasm32")]` guard. `grep -n 'bindings::' crates/cyrup-ext-sdk/src/example.rs` returns that one line (:258) — the only raw-ABI reach-around in the file, into the layer `lib.rs:19-20` documents as where unsafe is confined.

## Why it matters

An event-tier author reading the module doc beside the type is told they cannot set the model, while the world comment, the sibling ctx doc and the host all say they can. They either believe the doc and give up, or do what the example had to do and bypass the typed `impl Serialize` encoding for a call site that only compiles on wasm32. And a freshly written doc reasserting exactly the claim EXT-074 retracted is the resurfacing-stale-citation class the repo maintains `crates/cyrup-ext/src/tests/wit_world_sync.rs` for.

## Fix

1. Add `pub fn set_model(&self, model: impl Serialize)` to `Models` in `src/ctx/models.rs`, next to `set_thinking_level` (declared at `src/ctx/models.rs:76`; the impl closes at :87), with the same dual body `CommandCtx::set_model` already has (`src/ctx/command.rs:162-171`: serialize, wasm arm `models::set_model(&m)`, host arm `let _ = m;`).
2. Make `CommandCtx::set_model` a one-line delegation to `Models::set_model` so no caller breaks (`src/example.rs:737` does `match ctx.set_model(target) {`).
3. Rewrite `src/ctx/models.rs:1-2` to state the EXT-074/GAP-11 position: neither `set_model` nor `set_thinking_level` is tier-gated; both are queued and applied at the store-free turn-boundary drain (`base.rs:58-62`, `live.rs:565-577`). `set_model` remains on `CommandCtx` only for source compatibility.
4. Replace `src/example.rs:257-260` with `ctx.models().set_model(json!({"provider":"faux","model":"faux-2"}))` and drop the four-line apology and the `#[cfg(target_arch = "wasm32")]` guard.

The `world_import_coverage` guard is unaffected: `models::set_model(` still appears literally in `src/ctx/models.rs` and `src/ctx/command.rs`, both in `SDK_SOURCES` (`src/tests/world_import_coverage.rs:32-50`), and `example.rs` is not in that list at all.

If `CommandCtx::set_model` is being reworked at the same time as EXT_SDK_SERIALIZE_ERROR_SWALLOW, do that task's `map_err` change on the delegating method.

## Acceptance Criteria

- [ ] `grep -n 'pub fn set_model' crates/cyrup-ext-sdk/src/ctx/models.rs` matches, and `crates/cyrup-ext-sdk/src/ctx/command.rs`'s `set_model` delegates to it
- [ ] `grep -n 'command-tier' crates/cyrup-ext-sdk/src/ctx/models.rs` returns nothing, and the module doc's claim matches `crates/cyrup-ext-sdk/src/ctx/base.rs:58-62`
- [ ] `grep -n 'bindings::' crates/cyrup-ext-sdk/src/example.rs` returns nothing
- [ ] `cargo test -p cyrup-ext-sdk` passes at its baseline count (world_import_coverage still finds `models::set_model(` in SDK_SOURCES)
- [ ] `cargo check -p cyrup-ext-sdk --target wasm32-wasip2` reports 0 warnings, 0 errors, and `cargo build -p cyrup-ext-sdk --target wasm32-wasip2` still emits the component
