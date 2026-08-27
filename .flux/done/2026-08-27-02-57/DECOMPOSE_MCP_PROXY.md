---
stage: qa
status: completed
updated: 2026-08-27 05:30
---

# Decompose cyrup-mcp proxy.rs Into Submodules

## COMPLETED

`proxy.rs` (7,594 lines, the workspace's largest Rust file) is now `proxy/` — 14
files, largest 1,070 — split along the 17 section banners the file already
declared, verified against a traced dependency graph.

Gates: `cargo check --workspace --all-targets` exit 0 zero warnings;
`cargo doc --workspace --no-deps --bins` exit 0 with the rustdoc lints still
`deny` and zero `#[allow(rustdoc::…)]`; `cargo nextest run --workspace`
7863/7863; 90 proxy tests.

The QA finding that held this at 9/10 is closed: all 13 module-doc headers cited
`Split out of proxy.rs (lines N–M)` plus a bare `(§N` banner number, both
pointing into the file this task deleted. Headers now carry only durable
anchors — the `MCP-###` port-unit IDs and the `13d §N` gap-analysis references —
and `See [crate::proxy] for the module overview.` Both rework greps return zero;
`13d §2/§7/§10/§12/§13` are preserved.

Three findings worth carrying forward:

* **A latent security bug**, found because the split surfaced it. `approval.rs`
  inherited `Some(APPROVE_ONCE_OPTION) => Approved` without importing the
  constant, which silently turns a constant pattern into a catch-all binding —
  every tool call would have been approved, including denied ones. rustc cannot
  error on this; it showed only as `unreachable pattern` + `unused variable` +
  `non_snake_case`, and it passed 7863 tests both before and after the fix
  because no test covered that arm's negative case.
* **The plan's own numbers were wrong twice.** 89 tests was really 90 (the count
  regex missed `#[tokio::test(start_paused = true)]`); 106 at-risk doc links was
  really 21, because the explicit imports the compiler demanded also brought
  most link targets into scope.
* **`suggestion_text` never needed widening**, confirming the plan's warning
  that its 14-item visibility list carried false positives. All 43 widenings are
  `pub(crate)`, so nothing new is public.

---

## What was verified


- `proxy.rs` (7,594 lines) is now `proxy/` with 14 files, largest 1,070; no
  file exceeds the original's worst case by any measure
- Production logic byte-identical to its original section ranges, modulo only
  visibility widening, doc-link requalification and added imports
- Public API unchanged: `lib.rs`, `oauth.rs`, `registration.rs` and
  `extension.rs` are untouched, held by 12 glob re-exports in `mod.rs`
- No public-API widening — all 43 widenings are `pub(crate)` (23 free items,
  20 methods), applied only where the compiler demanded; `suggestion_text`,
  which the plan predicted would need it, correctly did not
- No duplicate top-level definitions anywhere — the slices did not overlap
- Realised import graph matches the planned layering, including the
  anticipated `env ↔ call` and `env ↔ approval` mutual references
- All 90 tests preserved (the plan's "89" undercounted by missing
  `#[tokio::test(start_paused = true)]`) and each group placed with the code it
  exercises
- `testsupport.rs` is `#[cfg(test)]`-gated, matching all three sibling
  `testsupport` modules in `cyrup-ext-subagents`
- Test modules import from `testsupport` by name, not by glob; the eleven
  `#[allow(unused_imports)]` suppressions were removed
- `use super::*` appears only in test modules, matching 432 of this crate's 453
  test modules (95%) and both named exec precedents
- Gates: `cargo check --workspace --all-targets` exit 0 zero warnings;
  `cargo doc --workspace --no-deps --bins` exit 0; `cargo nextest run
  --workspace` 7863/7863
- A latent security bug was found and fixed en route: `approval.rs` inherited
  `Some(APPROVE_ONCE_OPTION) => Approved` without the constant, silently turning
  a constant pattern into a catch-all binding that would approve every tool call
