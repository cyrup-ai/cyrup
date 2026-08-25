---
severity: medium
file: crates/cyrup-ext-subagents/src/formatters.rs
lines: 1-66
introduced: true
---

# formatters.rs is unused — six duplicate implementations remain live, contradicting its own "single port" claim

## Problem
`formatters.rs` is added and documented (both in its own module doc and in `lib.rs`'s doc comment on `pub mod formatters`) as "the crate's SINGLE port of pi's `shared/formatters.ts`" for `format_tokens`, `format_model_thinking`, and `run_mode_label`, with the explicit rationale that these render "human-facing strings that appear side by side in the same views... so a divergence between two copies is directly visible to the user."

In reality, nothing in the crate calls into `formatters::` — there is not a single `use crate::formatters` or `formatters::` reference anywhere outside the module itself. All of the private, byte-for-byte duplicate implementations the module was meant to replace are still in place and still what every call site actually uses:

- `format_tokens`: still separately defined in `registration/cost.rs:735`, `tui/fleet.rs:607`, and `background/fleet_view.rs:246`.
- `format_model_thinking`: still separately defined in `tui/fleet_status.rs:355`, `tui/fleet.rs:618`, and `background/fleet_view.rs:260`.
- `run_mode_label`: still separately defined in `tui/fleet_status.rs:546`, `tui/render.rs:394`, and `background/run_status.rs:58`.

This is the opposite outcome from `paths.rs` and `time.rs` in the same PR, which really are wired in everywhere (dozens of live `crate::paths::` / `crate::time::` call sites across the crate) — `formatters.rs` is a module that exists, is public, is documented as authoritative, but is dead code that nothing depends on. The exact divergence risk the module's own doc comment warns about (citing the `missions/store.rs` `CYRUP_HOME` incident as a cautionary tale) is still fully present for these three functions.

## Evidence
```
$ grep -rn "formatters::" crates/cyrup-ext-subagents/src/ | grep -v src/formatters.rs
(no output — zero call sites)

$ grep -rn "fn format_tokens" crates/cyrup-ext-subagents/src/
crates/cyrup-ext-subagents/src/registration/cost.rs:735:fn format_tokens(n: u64) -> String {
crates/cyrup-ext-subagents/src/tui/fleet.rs:607:fn format_tokens(n: u64) -> String {
crates/cyrup-ext-subagents/src/background/fleet_view.rs:246:fn format_tokens(n: u64) -> String {
crates/cyrup-ext-subagents/src/formatters.rs:13:pub fn format_tokens(n: u64) -> String {   # <- the "single" copy, unreferenced
```
(same pattern for `format_model_thinking` in `tui/fleet_status.rs`, `tui/fleet.rs`, `background/fleet_view.rs`, and for `run_mode_label` in `tui/fleet_status.rs`, `tui/render.rs`, `background/run_status.rs`.)

## Impact
- Misleading documentation: `lib.rs` and `formatters.rs` both assert single-source-of-truth status that does not hold, which will mislead future contributors (and reviewers) into believing the consolidation happened and that editing `formatters.rs` alone is sufficient.
- The exact bug class the module was created to prevent (silent divergence between copies rendering the same value differently in different views) remains fully possible — none of the six existing duplicates were replaced with calls into the new module.
- Dead public API: `formatters::format_tokens`, `format_model_thinking`, `format_model_thinking_opt`, and `run_mode_label` currently have no callers in the crate.

## Suggested Fix
Either finish the consolidation in this PR (replace the six duplicate `fn` definitions in `registration/cost.rs`, `tui/fleet.rs`, `tui/fleet_status.rs`, `tui/render.rs`, `background/fleet_view.rs`, and `background/run_status.rs` with calls to `crate::formatters::*`, deleting the local copies), or — if that's intentionally deferred to a follow-up — soften the "SINGLE port" / "one definition each" claims in `lib.rs` and `formatters.rs` so they don't assert a consolidation that hasn't happened yet, and leave a TODO/tracking note pointing at the follow-up.
