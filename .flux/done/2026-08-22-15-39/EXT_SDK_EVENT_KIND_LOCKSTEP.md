---
stage: qa
status: completed
updated: 2026-08-23 01:10
---

# Guard The Three-Way Event-Kind Discriminant Lockstep (Host EventKind / SDK kind::* / export_extension! Literals)

**Severity:** medium · **Effort:** M · **Crate:** `crates/cyrup-ext-sdk`

## What is wrong

The event-kind numbering exists in three hand-maintained copies with zero checks between them:

1. `EventKind` — `crates/cyrup-ext/src/event.rs:13-62`, 33 variants, `ToolCall = 0` … `SessionInfoChanged = 32`.
2. `mod kind` — `crates/cyrup-ext-sdk/src/api.rs:21-64`, 33 `pub const … : u8`.
3. Bare numeric literals in `export_extension!` — `crates/cyrup-ext-sdk/src/macros.rs`, e.g. `guest::notify(13/14/15)` at :309-335 and `notify(32)` at :356.

The only thing tying them together is prose at `crates/cyrup-ext-sdk/src/api.rs:20`: "Event-kind discriminants — kept in lockstep with the host `EventKind` (cyrup-ext/src/event.rs)." `rg -n 'lockstep|kind::TOOL_CALL' crates/cyrup-ext/src/tests/ crates/cyrup-ext-sdk/src/tests/ crates/cyrup-it/tests/` returns **nothing** — there is no guard at all.

All three copies agree today (verified by machine diff: EventKind vs `mod kind` is 33/33 identical, and every numbered `fn on_*` in the macro matches its const), so this is a guard gap, not a live defect.

Both coupling legs are real: `crates/cyrup-ext-sdk/src/guest.rs:123` sends `registration::subscribe(&api.subscription_kinds())`; the host at `crates/cyrup-ext/src/host/live.rs:253-259` does `if let Some(kind) = EventKind::from_u8(k) { … }` **with no else branch** — an unknown number is silently dropped; and dispatch looks up `self.handlers.get(&kind)` at `crates/cyrup-ext-sdk/src/api.rs:1037-1041`.

The macro body is `#[cfg(target_arch = "wasm32")]` (`macros.rs:31`), so its literals are not even parsed on the host target where the default suite runs. `world_import_coverage.rs:166-168` deliberately exempts the `events` interface via `NON_IMPORT_INTERFACES`, so it does not cover this either.

## Why it matters

Adding an event mid-enum or reordering compiles clean on every target, passes `cargo test --workspace`, and produces a guest whose subscriptions are silently dropped by `EventKind::from_u8` or whose handlers receive another event's argument strings. The symptom is "my extension's hook never fires" or a decode of the wrong payload, with no diagnostic pointing at the numbering. This is the guest-side twin of the class `src/tests/world_import_coverage.rs` was written for.

## Fix

Two tests, each in the crate that can see both sides.

**(1) SDK-internal (macro literals vs `mod kind`).** In `crates/cyrup-ext-sdk/src/tests/world_import_coverage.rs` — which already `include_str!`s `../api.rs` at line 48 — add `include_str!("../macros.rs")` and a test that pairs each `fn (on_[a-z_]+)\s*\(` in the macro body with the `guest::(hook|notify)\(\s*(\d+)` that follows it, asserting the number equals the `pub const <NAME>: u8 = N;` for the matching event in `mod kind`. Assert a count floor of 33 for non-vacuity, the way `every_ctx_submodule_is_in_sdk_sources` asserts `scanned >= 13`. No new dependency needed.

**Important**: the pairing is NOT a mechanical snake-case→SCREAMING_CASE map. `crates/cyrup-ext-sdk/src/api.rs:35-37` declares `TOOL_EXEC_START`/`TOOL_EXEC_UPDATE`/`TOOL_EXEC_END` while the macro exports `on_tool_execution_start`/`_update`/`_end` (`macros.rs:311/318/331`), and `fn on_terminal_input` (`macros.rs:98`) is an export with no kind at all. The test needs an explicit name map plus an allowlist of non-kind exports.

**(2) Cross-crate (`mod kind` vs `EventKind`).** In `crates/cyrup-ext/src/tests/` — which already reads SDK sources by relative path in `wit_world_sync.rs::cited_files()` — parse `../cyrup-ext-sdk/src/api.rs`'s `mod kind` block and assert each `NAME = N` matches `EventKind::from_u8(N)`'s variant, using the existing `EventKind::name`/`from_u8` pair (`crates/cyrup-ext/src/event.rs:65-120`) to map.

Then replace the unenforced prose at `api.rs:20` with a pointer to the two tests.

## Acceptance Criteria

- [ ] A test in `crates/cyrup-ext-sdk/src/tests/world_import_coverage.rs` cross-checks every numbered `on_*` export in `src/macros.rs` against `mod kind` in `src/api.rs`, and asserts a non-vacuity floor of at least 33 pairs
- [ ] A test in `crates/cyrup-ext/src/tests/` cross-checks every `mod kind` const in `../cyrup-ext-sdk/src/api.rs` against `EventKind::from_u8`
- [ ] Temporarily changing `SESSION_INFO_CHANGED: u8 = 32` to `42` in `crates/cyrup-ext-sdk/src/api.rs` makes both `cargo test -p cyrup-ext-sdk` and `cargo test -p cyrup-ext` fail; reverting restores green
- [ ] Temporarily changing one `guest::notify(N)` literal in `crates/cyrup-ext-sdk/src/macros.rs` makes `cargo test -p cyrup-ext-sdk` fail; reverting restores green
- [ ] `crates/cyrup-ext-sdk/src/api.rs:20`'s prose names the two tests by file
- [ ] `cargo test -p cyrup-ext-sdk` and `cargo test -p cyrup-ext` pass at or above their baseline counts (17 / 293)
