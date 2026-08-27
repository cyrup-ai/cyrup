---
stage: qa
status: completed
updated: 2026-08-23 01:10
---

# Reconcile lib.rs's Flat Re-Export List With The Prelude And Pin Them With A Set-Equality Test

**Severity:** medium · **Effort:** S · **Crate:** `crates/cyrup-ext-sdk`

## What is wrong

`crates/cyrup-ext-sdk/src/lib.rs` maintains two hand-written export lists that have drifted, and nothing checks them against each other.

**(a) Ten names are in the flat block but not the prelude.** Flat block is lib.rs:38-63; prelude is lib.rs:65-95. Missing from the prelude: `RawOutcome` (flat :40), `RegisteredCommand`/`RegisteredRenderer`/`RegisteredShortcut`/`RegisteredTool` (flat :41), `ExecResult` (flat :48), `ExecOptions`/`ModelCost`/`ModelCostTier`/`RenderShell` (flat :52-55).

Those are not incidental types — they are in the signatures of prelude APIs:

- `crates/cyrup-ext-sdk/src/ctx/exec.rs:10`: `pub fn exec(&self, cmd: &str, args: &[&str], opts: &ExecOptions) -> Result<ExecResult, String>` — an author who follows the documented import and calls `ctx.exec(...)` can name neither the parameter nor the return type.
- `crates/cyrup-ext-sdk/src/tool_factory.rs:13`: `pub fn define_tool(...) -> RegisteredTool`
- `crates/cyrup-ext-sdk/src/descriptor.rs:94`: `pub render_shell: RenderShell`; `descriptor.rs:367`: `pub cost: ModelCost`

Live proof: `crates/cyrup-it/tests/ext/ergonomic.rs:7-8` is `use cyrup_ext_sdk::prelude::*;` immediately followed by `use cyrup_ext_sdk::RawOutcome;`, and `crates/cyrup-ext-sdk/src/example.rs:9-14` imports `ExecOptions` from `crate::{...}` rather than the prelude. `macros.rs:5` documents the prelude as the author entry point, so the crate's own reference extension cannot be written through its documented import.

**(b) Seven signature types of root-exported APIs are in neither list.** They are reachable only via the module paths `cyrup_ext_sdk::api::` / `cyrup_ext_sdk::descriptor::`:

- `ArgCompleter` (api.rs:265, the type of `pub completions: Option<ArgCompleter>` at api.rs:272), `TerminalInputResult` (api.rs:375), `TerminalInputHandler` (api.rs:402) — all reachable from `on_terminal_input` (api.rs:660) / `handle_terminal_input` (api.rs:666). The sibling handler trait `MarkdownTransformer` IS in the flat list (lib.rs:40), which makes the omission internally inconsistent.
- `ConstrainedSampling` (descriptor.rs:69), `ConstrainedSamplingConfig` (:31), `GrammarVariants` (:57), `StrictSampling` (:46) — reachable from the `constrained_sampling` field (descriptor.rs:105) and builder (:163).

So `api.on_terminal_input(|d| Some(TerminalInputResult::consume()))` and `ToolDescriptor::new(..).constrained_sampling(ConstrainedSamplingConfig::JsonSchema { strict: StrictSampling::Require })` cannot be written from the crate root.

Note: `crate::widget` is NOT drift — `pub mod widget;` (lib.rs:30) already makes it nameable at the root and lib.rs:63 re-exports `WidgetPlacement`.

## Why it matters

The prelude is the documented entry point yet cannot express the crate's own reference extension, and because nothing checks the two lists against each other, every future export added to one silently skips the other.

## Fix

1. Add `ArgCompleter, TerminalInputHandler, TerminalInputResult` to the `pub use api::{...}` block (lib.rs:38-43) and `ConstrainedSampling, ConstrainedSamplingConfig, GrammarVariants, StrictSampling` to the `pub use descriptor::{...}` block (lib.rs:51-57).
2. Make the prelude the flat list: define the per-module re-exports once inside `pub mod prelude`, then collapse the flat block to `pub use prelude::*;` plus any deliberate root-only extras. If the two must stay separate, add all ten missing names (plus the seven above) to `prelude`.
3. Either way, add a unit test under `crates/cyrup-ext-sdk/src/tests/` that fails on divergence — e.g. a module doing `use crate::prelude::*;` that also names every symbol in the flat list, so a name present in one and absent from the other fails the build rather than a user's.

## Acceptance Criteria

- [ ] A module compiling `use crate::prelude::*;` can name `RawOutcome`, `RegisteredCommand`, `RegisteredRenderer`, `RegisteredShortcut`, `RegisteredTool`, `ExecResult`, `ExecOptions`, `ModelCost`, `ModelCostTier`, `RenderShell` without any additional `use`
- [ ] `ArgCompleter`, `TerminalInputHandler`, `TerminalInputResult`, `ConstrainedSampling`, `ConstrainedSamplingConfig`, `GrammarVariants`, `StrictSampling` are all nameable as `cyrup_ext_sdk::<Name>` (grep them in the `pub use api::{...}` / `pub use descriptor::{...}` blocks of lib.rs)
- [ ] A test in `crates/cyrup-ext-sdk/src/tests/` fails when a name is added to the flat re-export block and not to the prelude (verify by adding one temporarily, running `cargo test -p cyrup-ext-sdk`, then reverting)
- [ ] `cargo test -p cyrup-ext-sdk` passes and `cargo clippy -p cyrup-ext-sdk --all-targets` reports 0 warnings
- [ ] `cargo check -p cyrup-ext-sdk --target wasm32-wasip2` reports 0 warnings, 0 errors
