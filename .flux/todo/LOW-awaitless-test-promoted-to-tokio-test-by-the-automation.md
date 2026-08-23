---
title: Awaitless Test Promoted To Tokio Test By The Automation
priority: LOW
stage: aug
status: done
updated: 2026-08-23 03:38
---

# Revert the one `#[tokio::test]` that has no `.await` back to a plain `#[test]`

## Description

`fe86c7f` ("refactor: decompose ops/local.rs and fix the config-lock hang") made
`handle_management_action` async and then applied
[`CFGLOCK_3`](_backlog/CFGLOCK_3.md)'s step 4 recipe — *"promote any `fn` whose body now contains
`.await` to `async fn`, and any `#[test]` immediately above it to `#[tokio::test]`"* — via a
compiler-span-driven script. The script fired one extra time, on a test whose body contains no
`.await` and never touches the async surface.

[`crates/cyrup-ext-subagents/src/discovery/management.rs:4245-4246`](../../crates/cyrup-ext-subagents/src/discovery/management.rs):

```rust
    #[tokio::test]
    async fn serialize_agent_round_trips_memory_and_tool_budget() {
```

The whole body (`:4247-4283`) is synchronous. Every callee it touches is a plain `fn` on this
branch:

| Callee | Definition | Signature |
| --- | --- | --- |
| `sample_agent` | [`management.rs:3711`](../../crates/cyrup-ext-subagents/src/discovery/management.rs) | `fn sample_agent(source: AgentSource, file_path: PathBuf) -> AgentDefinition` |
| `serialize_agent` | [`management.rs:646`](../../crates/cyrup-ext-subagents/src/discovery/management.rs) | `fn serialize_agent(def: &AgentDefinition, preserve_fields: Option<&HashSet<String>>) -> String` |
| `parse_agent_file` | [`frontmatter.rs:791`](../../crates/cyrup-ext-subagents/src/discovery/frontmatter.rs) | `pub fn parse_agent_file(content: &str, source: AgentSource, file_path: &Path) -> Option<AgentDefinition>` |
| `validate_tool_budget_config` | [`tool_budget.rs:69`](../../crates/cyrup-ext-subagents/src/exec/tool_budget.rs) | `pub fn validate_tool_budget_config(...)` |

## Evidence

A body-scoped scan of every `#[tokio::test]` in `management.rs` — 30 of them — finds exactly one
with no `.await`, and it is the same one:

```
total tokio tests: 30
NO AWAIT: serialize_agent_round_trips_memory_and_tool_budget
tokio tests calling handle_management_action: 29
```

The mirror check is clean: of the 42 remaining plain `#[test]` fns in `management.rs`, **zero**
contain `.await` or call `handle_management_action`. The other two touched files are clean too —
all 10 `#[tokio::test]`s in
[`settings_write.rs`](../../crates/cyrup-ext-subagents/src/discovery/settings_write.rs) (one of
them `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]` at `:270`) and all 15 in
[`management_actions_integration.rs`](../../crates/cyrup-ext-subagents/src/tests/management_actions_integration.rs)
genuinely await.

So the true promotion set was **29**, not the 30 that `CFGLOCK_3`'s scope table predicted
(*"30 of its 72 sync `#[test]` fns call it"*). The totals only appear to reconcile — 30 + 42 = 72 —
because this spurious promotion filled the off-by-one.

The pre-branch form is on record at the merge base
(`4902cddf8ce7d4723e41b4a7bf652361a584f905:crates/cyrup-ext-subagents/src/discovery/management.rs:4244-4245`):

```rust
    #[test]
    fn serialize_agent_round_trips_memory_and_tool_budget() {
```

and the branch diff for this hunk is a clean two-line swap with no body changes:

```diff
-    #[test]
-    fn serialize_agent_round_trips_memory_and_tool_budget() {
+    #[tokio::test]
+    async fn serialize_agent_round_trips_memory_and_tool_budget() {
```

## Decision: revert it

Reverting is the required path, not "leave it".

- **Nothing references the name.** A repo-wide grep (excluding `target/` and `.git/`) for
  `serialize_agent_round_trips_memory` returns the definition at `management.rs:4246` and this task
  file — nothing else. No CI filter, no `nextest` selector, no doc, no `mod` re-export. The test is
  a private `fn` inside `mod tests`; changing its signature cannot break a caller because it has
  none.
- **No lint will ever catch it.** `[workspace.lints.clippy]` in the root
  [`Cargo.toml`](../../Cargo.toml) sets `unwrap_used`, `expect_used`, `panic`,
  `indexing_slicing` and `return_self_not_must_use`. `clippy::unused_async` is not enabled, and
  `#[tokio::test]` would suppress it here anyway. If this is not fixed by hand it stays forever.
- **The split is load-bearing documentation.** Within this one `mod tests`, `#[test]` vs
  `#[tokio::test]` is the only marker of which tests exercise the newly-async management dispatch.
  One false positive in 30 makes the marker unreliable, and the next reader has to re-derive it.
- **It costs a runtime per run for nothing.** `#[tokio::test]` builds and tears down a
  current-thread `Runtime` around a pure string round-trip.
- **The precedent is 46 lines below.** The sibling round-trip test
  `serialize_agent_round_trips_the_turn_budget_launch_default`
  ([`management.rs:4291-4292`](../../crates/cyrup-ext-subagents/src/discovery/management.rs)) is
  structurally identical — same `sample_agent` fixture, same `serialize_agent` →
  `parse_agent_file` shape — and correctly remained `#[test] fn`. Reverting makes the two agree.

## Required Change

Exactly one edit, in exactly one file. Replace lines 4245-4246 of
[`crates/cyrup-ext-subagents/src/discovery/management.rs`](../../crates/cyrup-ext-subagents/src/discovery/management.rs):

```rust
    #[tokio::test]
    async fn serialize_agent_round_trips_memory_and_tool_budget() {
```

with:

```rust
    #[test]
    fn serialize_agent_round_trips_memory_and_tool_budget() {
```

Keep the four-line doc comment at `:4241-4244` and the entire body at `:4247-4283` byte-identical.
Indentation is four spaces (the fn sits inside `mod tests`). Do not touch the other 29
`#[tokio::test]`s in this file, and do not touch `settings_write.rs` or
`management_actions_integration.rs`.

## Out Of Scope

- Do **not** run a workspace-wide `cargo fmt`. This edit shortens two lines and introduces no
  rustfmt violation; the branch's separate formatting debt is tracked in
  [`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md`](LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md).
- Do **not** enable `clippy::unused_async` workspace-wide as a "systemic" fix. It fires on real
  async trait impls and on `async fn`s that are async for signature-compatibility reasons across
  the workspace; that is a separate, much larger decision.
- Do **not** rewrite, rename, split, or extend the test body, its doc comment, or its assertions.
- No new tests and no new docs: correcting this attribute *is* the deliverable.

## Acceptance Criteria

- [ ] `grep -n -A1 'async fn serialize_agent_round_trips_memory_and_tool_budget' crates/cyrup-ext-subagents/src/discovery/management.rs` returns nothing
- [ ] `sed -n '4245,4246p' crates/cyrup-ext-subagents/src/discovery/management.rs` prints `    #[test]` followed by `    fn serialize_agent_round_trips_memory_and_tool_budget() {`
- [ ] `grep -c '#\[tokio::test\]' crates/cyrup-ext-subagents/src/discovery/management.rs` is **29** (was 30) and `grep -c '^\s*#\[test\]' …` is **43** (was 42); 29 + 43 = 72, unchanged
- [ ] Every remaining `#[tokio::test]` in `management.rs` has `.await` in its body — the body-scoped scan reports no `NO AWAIT` entries
- [ ] `git diff` for this task touches one file and is exactly the two-line swap shown above — no body, doc-comment, or whitespace changes anywhere else
- [ ] `cargo test -p cyrup-ext-subagents serialize_agent_round_trips_memory_and_tool_budget` passes and reports 1 test run
- [ ] `cargo clippy -p cyrup-ext-subagents --all-targets` is clean (no new warnings introduced)
- [ ] `cargo fmt -p cyrup-ext-subagents --check` reports no *new* violations attributable to this edit
