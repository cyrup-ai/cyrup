---
stage: new
status: done
updated: 2026-08-22 18:30
severity: medium
effort: small
category: lint-policy
---

# Consolidate The Crate's 18 Lint Suppressions Into One Test Waiver And Retire The 4 Doc-Link Import Shims

## Description
`cyrup-agent` carries two clusters of lint suppressions that exist only because of how the code was assembled, not because of anything the code does.

**(1) One workspace policy waiver, re-declared 14 times in four spellings.** The workspace denies exactly four lints ([Cargo.toml:97-101](../../Cargo.toml): `unwrap_used`, `expect_used`, `panic`, `indexing_slicing`). Every one of the 13 modules under `src/tests/` re-declares its own waiver for them, in four different forms:

- 5-lint multi-line, adding a fifth lint `clippy::print_stdout`: [agent_loop.rs:3-9](../../crates/cyrup-agent/src/tests/agent_loop.rs), [hook_failure_text.rs:22-28](../../crates/cyrup-agent/src/tests/hook_failure_text.rs), [model_boundary.rs:9-15](../../crates/cyrup-agent/src/tests/model_boundary.rs), [preflight_validation.rs:3-9](../../crates/cyrup-agent/src/tests/preflight_validation.rs), [round2_parity.rs:11-17](../../crates/cyrup-agent/src/tests/round2_parity.rs), [tool_result_model.rs:12-18](../../crates/cyrup-agent/src/tests/tool_result_model.rs), [untracked_misses.rs:13-19](../../crates/cyrup-agent/src/tests/untracked_misses.rs)
- 4-lint multi-line: [area02_backlog.rs:7-12](../../crates/cyrup-agent/src/tests/area02_backlog.rs), [pending_containment.rs:21-26](../../crates/cyrup-agent/src/tests/pending_containment.rs)
- 4-lint single-line, order `unwrap, expect, panic, indexing_slicing`: [agent_message_role_key.rs:13](../../crates/cyrup-agent/src/tests/agent_message_role_key.rs), [settlement_latch.rs:19](../../crates/cyrup-agent/src/tests/settlement_latch.rs)
- 4-lint single-line, order `unwrap, expect, indexing_slicing, panic`: [proxy_live_turn.rs:13](../../crates/cyrup-agent/src/tests/proxy_live_turn.rs), [turn_tool_refresh.rs:16](../../crates/cyrup-agent/src/tests/turn_tool_refresh.rs)

The 14th site is the item-level `#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]` at [proxy.rs:805](../../crates/cyrup-agent/src/proxy.rs), on the inline `#[cfg(test)] mod tests`. [src/tests/mod.rs](../../crates/cyrup-agent/src/tests/mod.rs) carries no waiver of its own (verified: 16 lines, a module doc plus 13 `mod` declarations), so every child re-declares the full list.

The `clippy::print_stdout` entry in 7 of those files is dead twice over: it is not in the workspace deny list, and `grep -rnE 'println!|eprintln!|print!' src/tests/*.rs` returns **0** hits.

This cluster also explains the crate's reported "27 unwrap/expect in non-test src". `grep -rn "unwrap()\|expect(" --include=*.rs src | grep -v /tests/ | awk -F: '{print $1}' | sort | uniq -c` returns exactly one line: `27 src/proxy.rs`, and every one of those 27 sits between proxy.rs:824 and proxy.rs:1123 — inside the inline test module that spans proxy.rs:804 to EOF at 1175. A path-based filter cannot see an inline test module, so the 27 is a measurement artifact, not 27 policy violations: `unwrap_used`/`expect_used` are `deny` (hard errors), and `cargo clippy -p cyrup-agent --all-targets` emits 3 warnings and zero `error:` lines.

**(2) Four unused-import shims, all of them in this crate.** `grep -rn 'allow(unused_imports)' crates/*/src/` returns exactly four hits workspace-wide, all inside the freshly split `src/agent/` tree: [agent/mod.rs:45](../../crates/cyrup-agent/src/agent/mod.rs) (`use lifecycle::SettlementGuard;`), [agent/prompt.rs:5](../../crates/cyrup-agent/src/agent/prompt.rs) (`use super::Agent;`), and [agent/run/mod.rs:10 and :12](../../crates/cyrup-agent/src/agent/run/mod.rs) (`use super::lifecycle::emit_standalone;`, `use super::Agent;`). Each is annotated as existing only so a bare intra-doc link keeps resolving after the item moved files — mod.rs:43-44 reads "Scope-only import: `SettlementGuard` appears in the `running_tx` doc below and resolved implicitly while it lived in the same file as `Agent`." The four dependent links are mod.rs:72 (`[`SettlementGuard`]`), prompt.rs:15 (`[`Agent::prompt`]`), run/mod.rs:87, :141, :196 (`[`Agent::start_run`]`, `[`Agent::set_headers`]`, `[`Agent::run`]`) and run/mod.rs:197 (`[`emit_standalone`]`).

Why it matters: 14 copies of one waiver in four spellings means nobody can tell at a glance whether a given file's waiver is deliberate or copy-pasted, and changing the workspace deny list requires editing 14 files. The four shims are decomposition scar tissue — dead `use` statements kept alive by the workspace's only four suppressions of their kind, purely so doc links can stay in their pre-split short form. Anyone later moving `SettlementGuard` or `emit_standalone` must discover those imports are load-bearing for rustdoc rather than for code, and the compiler cannot warn them because the warning is already suppressed.

## Scope
In scope: the 13 `#![allow(...)]` blocks in `src/tests/*.rs`, a new hoisted waiver in `src/tests/mod.rs`, and the four `#[allow(unused_imports)]` shims plus the doc links that depend on them.

Explicitly out of scope:
- The 3 existing clippy warnings (`needless_return`, `err_expect`) — not part of this item.
- The 6 existing `cargo doc -p cyrup-agent --no-deps` warnings. This task must not fix, add to, or otherwise collide with the queued **CARGO_DOC_WARNINGS** task; the doc-warning count must be unchanged at 6 when this lands.
- `proxy.rs:805` and any other inline `#[cfg(test)]` module waiver. Those are separate modules, and inline unit tests under `crates/*/src/` are the documented house convention (README.md:159).
- Any change to `[workspace.lints.clippy]` in the root Cargo.toml, to test assertions, or to runtime behavior.

## Approach
1. Add to [src/tests/mod.rs](../../crates/cyrup-agent/src/tests/mod.rs), directly under the existing module doc and above the `mod` list, a single inner attribute with a one-line comment pointing at `Cargo.toml:97-101`:
   `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]`
   Lint levels set on a parent module propagate into file-loaded submodules, so one declaration covers all 13.
2. Delete the `#![allow(...)]` block from each of the 13 files listed in the Description, at the exact line ranges given. Leave each file's module doc comment intact.
3. Drop `clippy::print_stdout` entirely rather than hoisting it — it is neither denied by the workspace nor triggered by any code in `src/tests/`.
4. For each of the four shims, rewrite the dependent doc link to a path-qualified form and then delete both the `use` and its `#[allow(unused_imports)]`:
   - agent/mod.rs:72 → `[`lifecycle::SettlementGuard`]`; delete mod.rs:43-46.
   - agent/prompt.rs:15 → `[`super::Agent::prompt`]`; delete prompt.rs:3-6.
   - agent/run/mod.rs:87, :141, :196 → `[`super::Agent::start_run`]`, `[`super::Agent::set_headers`]`, `[`super::Agent::run`]`; run/mod.rs:197 → `[`super::lifecycle::emit_standalone`]`; delete run/mod.rs:8-13.
5. If a qualified link trades the unused-import warning for a new `private_intra_doc_links` warning (checked by the doc-warning diff below), demote that one link to a plain code span instead of adding a suppression back.

## Acceptance Criteria
- [ ] `grep -rn 'allow(clippy::unwrap_used' crates/cyrup-agent/src/tests/` returns exactly one hit, in `src/tests/mod.rs`.
- [ ] `grep -rn 'print_stdout' crates/cyrup-agent/src/` returns 0 hits.
- [ ] `grep -rn 'allow(unused_imports)' crates/*/src/` returns 0 hits.
- [ ] `grep -n 'allow(clippy::unwrap_used' crates/cyrup-agent/src/proxy.rs` still returns the hit at line 805 (unchanged).
- [ ] `cargo clippy -p cyrup-agent --all-targets` emits zero `error:` lines and no more than the current 3 warnings (`needless_return` x2, `err_expect` x1) — no new lint fires from the removed per-file waivers.
- [ ] `cargo doc -p cyrup-agent --no-deps 2>&1 | grep -c '^warning:'` is 7 (6 warnings plus the summary line), i.e. the doc-warning baseline is unchanged.
- [ ] `cargo test -p cyrup-agent` is 140/140 passing.
- [ ] `git diff --stat` touches only the 17 files listed in this task; no test assertion or runtime code is modified.
