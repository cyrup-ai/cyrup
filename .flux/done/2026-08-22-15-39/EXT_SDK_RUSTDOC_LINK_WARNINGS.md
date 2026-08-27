---
stage: qa
status: completed
updated: 2026-08-23 01:10
---

# Take cyrup-ext-sdk's 7 Rustdoc Warnings To Zero, Including Four Links Naming APIs That Do Not Exist

**Severity:** medium · **Effort:** S · **Crate:** `crates/cyrup-ext-sdk`

## What is wrong

`cargo doc -p cyrup-ext-sdk --no-deps` emits 7 warnings; `cargo doc -p cyrup-ext-sdk --no-deps --document-private-items` emits the identical 7 (the private `src/ctx/*` submodules have no accumulated debt). Four of them name a callable API that does not exist, two of those misdirecting to the wrong type — the failure mode a reader cannot detect without grepping.

**Broken links to nonexistent items (4):**

| warning | reality |
|---|---|
| `descriptor.rs:196:67` unresolved link to `crate::Ctx::abort_signal` | `grep -rn 'fn abort_signal' crates/cyrup-ext-sdk/src` → one hit, `src/ctx/ui.rs:112`, a `Ui` method reached via `ctx.ui()`. The prose ("references a signal it already registered by ID via …") points at the wrong type. |
| `descriptor.rs:224:53` unresolved link to `crate::Ctx::abort_signal` | same |
| `descriptor.rs:425:8` unresolved link to `crate::events::SessionCompact` | `grep -rn 'SessionCompact' crates/cyrup-ext-sdk/src` → the real type is `SessionCompactEvent` at `src/events.rs:323` |
| `events.rs:89:23` unresolved link to `UserBashResult` | `grep -rn 'UserBashResult' crates/cyrup-ext-sdk/src` returns only the doc line itself. The `operations`/`result` override is returned as an `Outcome` — `src/api.rs:790` is `pub fn on_user_bash(&mut self, f: impl Fn(UserBashEvent, &Ctx) -> Outcome + 'static)`, and the handled arm is documented at `api.rs:81`. |

**Mechanical link warnings (3):**

- `lib.rs:18:83` unresolved link to `guest` and `macros.rs:19:72` unresolved link to `crate::guest` — both because `pub mod guest;` is behind `#[cfg(target_arch = "wasm32")]` (`src/lib.rs:33-34`), so rustdoc's host-target build cannot resolve it.
- `api.rs:659:11` public documentation for `on_terminal_input` links to the private item `ExtensionApi::terminal_input_handler` (field declared `pub(crate)` at `src/api.rs:454`).

## Why it matters

All four broken links sit on public items an author reads first (exec signal binding, compaction options, the user_bash event), and the house standard treats an inaccurate comment as worse than none. Seven warnings is small enough to reach zero in one sitting, and this crate's docs are the SDK's contract for external extension authors.

**This is not covered by `.flux/todo/CARGO_DOC_WARNINGS.md`**: that task is `status: done` in its front-matter with all four criteria unchecked, its per-crate table still says cyrup-ext-sdk 8, and its only SDK example cites `crates/cyrup-ext-sdk/src/ctx.rs:213` — a path deleted by the ctx decomposition. All 7 warnings remain.

## Fix

- `descriptor.rs:196` and `:224` — retarget to `[`crate::ctx::Ui::abort_signal`]` and adjust the prose to say the signal is registered through `ctx.ui()`.
- `descriptor.rs:425` — `[`crate::events::SessionCompactEvent`]`.
- `events.rs:89` — name `[`crate::Outcome`]` (the handled arm, `api.rs:81`) instead of the nonexistent `UserBashResult`; keep the `Pi UserBashEventResult` citation as plain backticks since it is an upstream name, not an in-crate item.
- `lib.rs:18` and `macros.rs:19` — drop the brackets to plain `` `guest` `` / `` `crate::guest` ``, matching `lib.rs:16`'s existing treatment of the same module ("- `guest` (wasm32) — the `wit-bindgen` glue…"). A `cfg_attr` doc alias is not worth it for two prose mentions.
- `api.rs:659` — plain `` `terminal_input_handler` `` backticks, or promote the field doc's content into the method doc. Do not make the field `pub`.
- Add `#![deny(rustdoc::broken_intra_doc_links, rustdoc::private_intra_doc_links)]` to `src/lib.rs` so the checked surface cannot regrow. Note this does not cover the private `src/ctx/*` submodules (rustdoc never visits them without `--document-private-items`); a documented note in `src/ctx/mod.rs`'s `## Submodules` section saying the flag is required when editing those files is sufficient — there is no `.github/` directory in this repo (`ls -d /home/user/cyrup/.github` → No such file or directory) and no existing rustdoc lint anywhere (`grep -rn 'rustdoc' Cargo.toml crates/cyrup-ext-sdk/Cargo.toml crates/cyrup-ext-sdk/src/lib.rs` → nothing).

## Acceptance Criteria

- [ ] `cargo doc -p cyrup-ext-sdk --no-deps 2>&1 | grep -c '^warning'` returns 0
- [ ] `cargo doc -p cyrup-ext-sdk --no-deps --document-private-items 2>&1 | grep -c '^warning'` returns 0
- [ ] `grep -rn 'crate::Ctx::abort_signal\|events::SessionCompact\]\|UserBashResult' crates/cyrup-ext-sdk/src` returns nothing
- [ ] `grep -n 'deny(rustdoc::broken_intra_doc_links' crates/cyrup-ext-sdk/src/lib.rs` matches
- [ ] `crates/cyrup-ext-sdk/src/api.rs:454`'s `terminal_input_handler` field is still `pub(crate)`
- [ ] `cargo check -p cyrup-ext-sdk` and `cargo check -p cyrup-ext-sdk --target wasm32-wasip2` both report 0 warnings, 0 errors
