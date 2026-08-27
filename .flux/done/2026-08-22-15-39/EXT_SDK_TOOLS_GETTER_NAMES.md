---
stage: qa
status: completed
updated: 2026-08-23 01:10
---

# Strip The get_ Prefix From The Four ctx/tools.rs Accessors

**Severity:** low · **Effort:** S · **Crate:** `crates/cyrup-ext-sdk`

## What is wrong

`crates/cyrup-ext-sdk/src/ctx/tools.rs` defines four `impl Ctx` accessors that keep a `get_` prefix — `get_active_tools` (:11), `get_all_tools` (:23), `get_commands` (:40), `get_flag` (:52) — while every other WIT `get-*` wrapper on the SAME type strips it. Side by side, all wrapping identically-shaped WIT imports:

- `src/ctx/base.rs:187` `pub fn cwd(&self)` wraps `ctx_state::get_cwd()`
- `src/ctx/base.rs:216` `pub fn system_prompt(&self)` wraps `ctx_state::get_system_prompt()`
- `src/ctx/ui.rs:303` `pub fn editor_text(&self)` wraps `ui::get_editor_text()`
- but `src/ctx/tools.rs:12` `pub fn get_active_tools(&self)` wraps `ext_tools::get_active_tools()`

The pi source they cite is `get`-prefixed in every case (`getCwd`/`getSystemPrompt`/`getEditorText`/`getActiveTools`), so upstream naming does not explain the split. `get_active_tools` (:11) is also paired with `set_active_tools` (:32), making the asymmetry visible in one screen.

Confirm the scope: `rg -nE 'pub fn (get_|set_)' crates/cyrup-ext-sdk/src | grep -v guest.rs` → 4 `get_` (all in ctx/tools.rs) and 17 `set_`; `grep -nE '^    pub fn ' crates/cyrup-ext-sdk/src/ctx/*.rs | grep -c 'pub fn get_'` → 4, against ~15 bare-noun accessors on the same types. The prefix is applied to exactly one of the thirteen ctx submodules.

## Why it matters

Rust API Guidelines C-GETTER; more concretely, an author who has learned `ctx.cwd()` and `ctx.system_prompt()` will reach for `ctx.active_tools()` and get a compile error on the one module that broke the pattern.

## Fix

Rename to `active_tools`, `all_tools`, `commands`, `flag` in `crates/cyrup-ext-sdk/src/ctx/tools.rs`. In-crate callers are only two: `crates/cyrup-ext-sdk/src/example.rs:843` (`ctx.ctx().get_active_tools()`) and `:970` (`.get_flag("demo-flag")`). Nothing outside this crate calls them — the `get_active_tools`/`get_flag` hits in `crates/cyrup-ext/src/host/live.rs:1007`/`:194` are the HOST-side WIT import implementations, unaffected by a guest-wrapper rename, and the `cyrup-session-svc` hit at `src/command.rs:84` is a doc comment.

The rename touches only Rust wrapper names, not the literal `ext_tools::get_active_tools(` / `registration::get_flag(` WIT call paths that `src/tests/world_import_coverage.rs` scans, so that test is unaffected.

If EXT_SDK_DECODE_FAILURE_DOCS is also changing `get_flag`'s signature, do both edits in one pass.

## Acceptance Criteria

- [ ] `rg -nE 'pub fn get_' crates/cyrup-ext-sdk/src --glob '!guest.rs'` returns nothing
- [ ] `grep -n 'active_tools\|all_tools\|fn commands\|fn flag' crates/cyrup-ext-sdk/src/ctx/tools.rs` shows the four bare-noun accessors
- [ ] `grep -n 'get_active_tools\|get_flag' crates/cyrup-ext-sdk/src/example.rs` returns nothing
- [ ] `grep -n 'ext_tools::get_active_tools(\|registration::get_flag(' crates/cyrup-ext-sdk/src/ctx/tools.rs` still matches (WIT call paths unchanged) and `cargo test -p cyrup-ext-sdk` passes, including world_import_coverage
- [ ] `cargo check -p cyrup-ext-sdk --target wasm32-wasip2` reports 0 warnings, 0 errors and `cargo clippy -p cyrup-ext-sdk --all-targets` reports 0 warnings
