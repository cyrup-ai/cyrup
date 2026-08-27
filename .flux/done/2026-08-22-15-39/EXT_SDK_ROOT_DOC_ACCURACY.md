---
stage: qa
status: completed
updated: 2026-08-23 01:10
---

# Fix The Stale Event Count And The Incomplete Module Index In The cyrup-ext-sdk Crate-Root Doc

**Severity:** medium · **Effort:** S · **Crate:** `crates/cyrup-ext-sdk`

## What is wrong

Two defects in the crate front page, `crates/cyrup-ext-sdk/src/lib.rs`.

**(a) The event count is wrong — 30 stated, 33 real.** `lib.rs:6` reads "subscribing to any of the 30 lifecycle events with typed `(event, &Ctx) -> Outcome` handlers". Verify the real number three ways:

- `grep -cE '^    pub const [A-Z_]+: u8' crates/cyrup-ext-sdk/src/api.rs` → 33 (`mod kind`, ids 0..=32; the last three are `AGENT_SETTLED = 30` at api.rs:56, `BEFORE_PROVIDER_HEADERS = 31` at :59, `SESSION_INFO_CHANGED = 32` at :63).
- `grep -cE '^    pub fn on_' crates/cyrup-ext-sdk/src/api.rs` → 35, minus the two non-event subscribers `on_terminal_input` (:660) and `on_bus` (:729) = 33.
- `grep -c 'self.handlers.insert' crates/cyrup-ext-sdk/src/api.rs` → 33 (plain `grep -c 'handlers.insert'` returns 34 because of the unrelated `provider_handlers.insert` at api.rs:617).

Every other statement of the count in the crate already says 33: `api.rs:3` ("any of the 33 events"), `api.rs:733` ("the 33 event subscriptions … EXT-072 corrected the count"), `guest.rs:4` ("the 33 lifecycle hooks"), `macros.rs:17` ("all 33 hooks"). The EXT-072 correction landed everywhere except lib.rs. The three missing events are the EXT-009/EXT-011/SEAM-005 additions: agent_settled, before_provider_headers, session_info_changed.

**Do not touch `lib.rs:12`.** Its separate "the 30 typed event payloads" is CORRECT: `grep -c 'pub struct .*Event' crates/cyrup-ext-sdk/src/events.rs` → 30, because e.g. `SessionLifecycleEvent` (events.rs:218) serves both session_start and session_shutdown, and agent_start/agent_settled carry no payload. The adjacent correct 30 two lines below is exactly what makes the wrong one read as authoritative rather than as a typo.

**(b) The `## Modules` index omits half the surface.** `lib.rs:10-16` lists six entries (api, events, ctx, descriptor, example, guest). `grep -nE '^pub mod' crates/cyrup-ext-sdk/src/lib.rs` → api:21, autocomplete:22, ctx:23, descriptor:24, events:25, example:26, macros:27, provider:28, tool_factory:29, widget:30, guest:33 (cfg wasm32), prelude:66. Missing from the index: **autocomplete, macros, provider, tool_factory, widget, prelude** — including the prelude that `macros.rs:5` presents as the author entry point. Consequence: `tool_factory`'s three built-in descriptor builders `bash_descriptor`/`read_descriptor`/`write_descriptor` (`tool_factory.rs:19/:37/:51`, none re-exported at the root — `lib.rs:62` re-exports only `define_tool`) and the whole `provider` module are undiscoverable from the front page.

Also: `crates/cyrup-ext-sdk/src/macros.rs` is `//!` prose (:1-24) plus a single `#[macro_export] macro_rules! export_extension` (:27-29), so `pub mod macros` (lib.rs:27) renders as an item-less page while the macro itself lands at the crate root — a reader looking for `export_extension!` is sent to an empty page.

Neither defect produces a rustdoc warning, so `.flux/todo/CARGO_DOC_WARNINGS.md` will not fix them.

## Fix

1. `lib.rs:6`: `30` → `33`, matching `api.rs:3`. Leave `:12` at 30 and append a clause distinguishing the counts, e.g. "(33 subscribable events; 30 payload structs, because some events share or omit one)".
2. Add the six missing modules to the `## Modules` list at `lib.rs:10-16`, giving `prelude` the first line since it is the documented entry point.
3. For `macros`, either mark it `#[doc(hidden)] pub mod macros;` and move its `//!` authoring guide (macros.rs:1-24, which is genuinely good) into the crate-root doc or onto the macro itself, or keep the module and add a `[export_extension!](crate::export_extension)` pointer at the top of its doc so the empty item list is explained.
4. Optional pin: `crates/cyrup-ext-sdk/src/tests/world_import_coverage.rs` already `include_str!`s SDK sources (see :45/:48) — extend it to assert the digit in the lib.rs doc equals the `mod kind` const count, so the number cannot drift again.

## Acceptance Criteria

- [ ] `grep -n '30 lifecycle events' crates/cyrup-ext-sdk/src/lib.rs` returns nothing; `grep -c '33 lifecycle events' crates/cyrup-ext-sdk/src/lib.rs` returns 1
- [ ] `grep -n '30 typed event payloads' crates/cyrup-ext-sdk/src/lib.rs` still matches (line 12 unchanged in substance)
- [ ] Every name printed by `grep -oE '^pub mod [a-z_]+' crates/cyrup-ext-sdk/src/lib.rs | awk '{print $3}'` plus `prelude` appears in the `## Modules` doc block of lib.rs
- [ ] `crates/cyrup-ext-sdk/src/macros.rs`'s module doc either is `#[doc(hidden)]` at lib.rs or contains a link to `crate::export_extension`
- [ ] `cargo doc -p cyrup-ext-sdk --no-deps` emits no new warnings relative to the 7-warning baseline
- [ ] `cargo test -p cyrup-ext-sdk` passes
