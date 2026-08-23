---
title: Awaitless Test Promoted To Tokio Test By The Automation
priority: LOW
stage: qa
status: completed
updated: 2026-08-23 08:20
---

# Revert the one `#[tokio::test]` that has no `.await` back to a plain `#[test]`

## Description

The ops/local.rs decomposition / config-lock work made
`crates/cyrup-ext-subagents/src/discovery/management.rs::handle_management_action` async and then
applied [`CFGLOCK_3`](_backlog/CFGLOCK_3.md)'s step 4 recipe — *"promote any `fn` whose body now
contains `.await` to `async fn`, and any `#[test]` immediately above it to `#[tokio::test]`"* — via
a compiler-span-driven script. The script fired one extra time, on a test whose body contains no
`.await` and never touches the async surface.

The offending item is the test function **`serialize_agent_round_trips_memory_and_tool_budget`**,
inside `mod tests` in
[`crates/cyrup-ext-subagents/src/discovery/management.rs`](../../crates/cyrup-ext-subagents/src/discovery/management.rs)
(currently at `:4245-4246` for the attribute + signature, body through `:4283` — line numbers are a
navigation hint only; the function **name** is the authoritative pointer):

```rust
    #[tokio::test]
    async fn serialize_agent_round_trips_memory_and_tool_budget() {
```

The whole body is synchronous. Every callee it touches is a plain `fn` in the current tree:

| Callee | Defined in | Signature (verified in the current tree) |
| --- | --- | --- |
| `sample_agent` | `discovery/management.rs`, inside `mod tests` | `fn sample_agent(source: AgentSource, file_path: PathBuf) -> AgentDefinition` |
| `serialize_agent` | `discovery/management.rs`, module scope | `fn serialize_agent(def: &AgentDefinition, preserve_fields: Option<&HashSet<String>>) -> String` |
| `parse_agent_file` | `discovery/frontmatter.rs` | `pub fn parse_agent_file(content: &str, source: AgentSource, file_path: &Path) -> Option<AgentDefinition>` |
| `validate_tool_budget_config` | `exec/tool_budget.rs` | `pub fn validate_tool_budget_config(...)` (sync) |

The body also contains **no** `tokio::`, `spawn`, `Handle`, `block_on`, `Runtime`, `sleep`, or
`channel` token, and neither `sample_agent` nor `serialize_agent` reaches any of those. There is
therefore no "needs a reactor without awaiting" justification for the attribute — the runtime is
pure overhead here.

By contrast, `handle_management_action` really is `pub async fn` in the current tree
(`management.rs:1421`), so the other 29 promotions in this file are correct.

## Evidence (re-verified against the current tree, 2026-08-23)

A **body-scoped** scan of every `#[tokio::test]` in `management.rs` finds exactly one with no
`.await`, and it is the same one:

```
total tokio tests: 30
NO AWAIT: serialize_agent_round_trips_memory_and_tool_budget   (attr line 4245, body ends 4283)
tokio tests calling handle_management_action: 29
```

> **Scanner caveat — read this before re-running the scan.** A naive brace-counting body extractor
> gives the WRONG answer on this file and reports zero awaitless tests. The body of
> `serialize_agent_round_trips_memory_and_tool_budget` contains the string literal
> `"toolBudget: {"` — an unbalanced `{` inside a string — so brace-depth tracking never returns to
> zero and every subsequent "body" swallows the rest of the file. Delimit bodies by **indentation**
> (from the `fn` line to the first line that is exactly the `fn`'s indent followed by `}`), not by
> counting braces. The verification script in *Definition of Done* does this correctly.

The mirror check is clean: of the 42 plain `#[test]` fns in `management.rs`, **zero** contain
`.await` and **zero** call `handle_management_action`.

The other two files the automation touched are clean too — every `#[tokio::test]` in
[`settings_write.rs`](../../crates/cyrup-ext-subagents/src/discovery/settings_write.rs) (there are
**11**, one of them `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]`) and all 15 in
[`management_actions_integration.rs`](../../crates/cyrup-ext-subagents/src/tests/management_actions_integration.rs)
genuinely await.

> Correction to the previous revision of this task file: it said "all 10 `#[tokio::test]`s in
> `settings_write.rs`". The current count is **11**. The conclusion is unchanged — all 11 await.

So the true promotion set was **29**, not the 30 that `CFGLOCK_3`'s scope table predicted
(`CFGLOCK_3.md:143` — *"**30 of its 72 sync `#[test]` fns** call it and must become
`#[tokio::test]`"*). The totals only appear to reconcile — 30 + 42 = 72 — because this spurious
promotion filled the off-by-one.

## Decision: revert it

Reverting is the required path, not "leave it".

- **Nothing references the name.** A repo-wide grep (excluding `target/`) for
  `serialize_agent_round_trips_memory_and_tool_budget` returns the definition in `management.rs`
  and this task file — nothing else. No CI filter, no `nextest` selector, no doc, no `mod`
  re-export. The test is a private `fn` inside `mod tests`; changing its signature cannot break a
  caller because it has none.
- **No lint will ever catch it.** `[workspace.lints.clippy]` in the root
  [`Cargo.toml`](../../Cargo.toml) sets exactly `unwrap_used = "deny"`, `expect_used = "deny"`,
  `panic = "deny"`, `indexing_slicing = "deny"`, `return_self_not_must_use = "warn"`.
  `clippy::unused_async` appears nowhere in the repo, and `#[tokio::test]` would suppress it here
  anyway. If this is not fixed by hand it stays forever.
- **The split is load-bearing documentation.** Within this one `mod tests`, `#[test]` vs
  `#[tokio::test]` is the only marker of which tests exercise the newly-async management dispatch.
  One false positive in 30 makes the marker unreliable, and the next reader has to re-derive it.
- **It costs a runtime per run for nothing.** `#[tokio::test]` builds and tears down a
  current-thread `Runtime` around a pure string round-trip.
- **The precedent is 46 lines below.** The sibling round-trip test
  `serialize_agent_round_trips_the_turn_budget_launch_default` (same file, currently `:4291-4292`)
  is structurally identical — same `sample_agent` fixture, same `serialize_agent` →
  `parse_agent_file` shape, same `use crate::discovery::frontmatter::parse_agent_file;` first line —
  and correctly remained `#[test] fn`. Reverting makes the two agree.

## Required Change

Exactly one edit, in exactly one file:
`crates/cyrup-ext-subagents/src/discovery/management.rs`.

**Replace this exact text** (two lines, four-space indent, LF endings, no trailing whitespace —
verified to occur **exactly once** in the file):

```
    #[tokio::test]
    async fn serialize_agent_round_trips_memory_and_tool_budget() {
```

**with this exact text:**

```
    #[test]
    fn serialize_agent_round_trips_memory_and_tool_budget() {
```

Constraints on the edit:

- The match count for the old text must be **1** before the edit. Assert this. If it is not 1,
  stop and re-derive the anchor from the function name — do not edit by line number.
- The four-line doc comment immediately above (`/// A \`memory:\`/\`toolBudget:\` agent must survive
  a serialize -> re-parse round-trip. …`) stays byte-identical.
- The entire body, from `use crate::discovery::frontmatter::parse_agent_file;` down to the closing
  `    }`, stays byte-identical. No reflow, no reordering, no assertion changes.
- Indentation is four spaces (the fn sits inside `mod tests`). Do not convert to tabs.
- Do not touch the other 29 `#[tokio::test]`s in this file, and do not touch `settings_write.rs`
  or `management_actions_integration.rs`.

## Out Of Scope

- **The 29 other awaitless `#[tokio::test]`s elsewhere in `cyrup-ext-subagents`.** A crate-wide
  body-scoped scan finds 666 `#[tokio::test]`s, of which 30 have no `.await` — this one plus 29
  others spread across `exec/acceptance/lattice/{lowering,report_source}.rs`,
  `registration/cost.rs`, `tui/fleet_overlay.rs`, `spawn/{chain_graph,signal}.rs`,
  `background/spawn_detached.rs`, `tests/watchdog_wiring.rs`,
  `extension/executor/resolve.rs`, `extension/tool/mod.rs`, and
  `watchdog/{runtime,review}.rs`. Their provenance is unrelated to the management-dispatch
  promotion, and several plausibly need a reactor for non-await reasons (constructing a runtime-
  bound handle, a channel, a watchdog). **Do not touch any of them in this task.** This task's
  scope is `management.rs` only; a crate-wide sweep is a separate decision on separate evidence.
- Do **not** run a workspace-wide `cargo fmt`. This edit shortens two lines and introduces no
  rustfmt violation; the separate formatting debt is tracked in
  [`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md`](LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md).
- Do **not** enable `clippy::unused_async` workspace-wide as a "systemic" fix. It fires on real
  async trait impls and on `async fn`s that are async for signature-compatibility reasons across
  the workspace; that is a separate, much larger decision.
- Do **not** rewrite, rename, split, or extend the test body, its doc comment, or its assertions.
- **No new tests, no benchmarks, no new documentation.** Correcting this attribute *is* the
  deliverable. Another team owns test and doc changes.

## Definition of Done

All checks are textual or compile-only. None requires writing a test, running a test, or using git.
Run from the repo root.

1. **The old form is gone.**

   ```
   grep -c 'async fn serialize_agent_round_trips_memory_and_tool_budget' \
     crates/cyrup-ext-subagents/src/discovery/management.rs
   ```
   prints `0`.

2. **The new form is present, exactly once, correctly indented.**

   ```
   grep -n -B1 '^    fn serialize_agent_round_trips_memory_and_tool_budget() {$' \
     crates/cyrup-ext-subagents/src/discovery/management.rs
   ```
   prints exactly two lines: `    #[test]` followed by
   `    fn serialize_agent_round_trips_memory_and_tool_budget() {`.

3. **The counts moved by exactly one, in opposite directions, and still total 72.**

   ```
   grep -c '#\[tokio::test\]' crates/cyrup-ext-subagents/src/discovery/management.rs   # 29 (was 30)
   grep -c '^[[:space:]]*#\[test\]$' crates/cyrup-ext-subagents/src/discovery/management.rs  # 43 (was 42)
   ```
   29 + 43 = 72, unchanged.

4. **No awaitless `#[tokio::test]` remains in `management.rs`.** Run this indentation-based scan
   (the brace-counting variant is wrong here — see the Scanner caveat above):

   ```
   python3 - <<'EOF'
   import re
   p = 'crates/cyrup-ext-subagents/src/discovery/management.rs'
   lines = open(p, encoding='utf-8').read().split('\n')
   total = 0
   bad = []
   for i, l in enumerate(lines):
       if not l.strip().startswith('#[tokio::test'):
           continue
       j = i + 1
       while j < len(lines) and ' fn ' not in lines[j]:
           j += 1
       name = re.search(r'fn (\w+)', lines[j]).group(1)
       indent = len(lines[j]) - len(lines[j].lstrip())
       close = ' ' * indent + '}'
       k = j + 1
       while k < len(lines) and lines[k].rstrip() != close:
           k += 1
       total += 1
       if '.await' not in '\n'.join(lines[j:k + 1]):
           bad.append((name, i + 1))
   print('total tokio tests:', total)
   print('awaitless:', bad)
   EOF
   ```
   must print `total tokio tests: 30` → **`29`** and `awaitless: []`.

5. **The body and doc comment are untouched.** The 38 lines from
   `    fn serialize_agent_round_trips_memory_and_tool_budget() {` through its closing `    }`
   still contain, in order: `use crate::discovery::frontmatter::parse_agent_file;`,
   `sample_agent(AgentSource::Project, PathBuf::from("/w.md"))`,
   `validate_tool_budget_config(`, `serialize_agent(&def, None)`,
   `serialized.contains("memory:\n  scope: project\n  path: security-reviewer")`,
   `serialized.contains("toolBudget: {")`,
   `parse_agent_file(&serialized, AgentSource::Project, Path::new("/w.md"))`, and the four final
   `assert` lines on `reparsed.memory`, `reparsed.tool_budget`, and the two `extra_fields` keys.
   The doc comment above still begins
   ``/// A `memory:`/`toolBudget:` agent must survive a serialize -> re-parse round-trip.``

6. **It still type-checks under `cfg(test)`.**

   ```
   cargo check -p cyrup-ext-subagents --all-targets
   ```
   completes with no new errors or warnings. (`--all-targets` type-checks test code without
   executing anything; do not run `cargo test` for this task.)

7. **Nothing else changed.** The only file modified in the working tree for this task is
   `crates/cyrup-ext-subagents/src/discovery/management.rs`, and within it only the two lines
   named in *Required Change*. Verify by comparing file mtimes / by inspection, not with git.
