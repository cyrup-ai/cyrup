---
title: Proactiveskillsinput Doc Still Calls The Dispatch Sync
priority: MEDIUM
stage: aug
status: done
updated: 2026-08-23 03:38
---

# `ProactiveSkillsInput`'s rustdoc still says `handle_management_action` is sync

## Problem

This branch converted `handle_management_action` to `pub async fn`. Two comments in
`cyrup-ext-subagents` carried the clause "`handle_management_action` is sync" as the *reason* for a
design decision. The one in
[`extension/tool/routing.rs`](../../crates/cyrup-ext-subagents/src/extension/tool/routing.rs) was
corrected in the same commit. Its twin — in the rustdoc of the **public**
`ProactiveSkillsInput` struct in
[`discovery/management.rs`](../../crates/cyrup-ext-subagents/src/discovery/management.rs) — was
not, and now states the opposite of the signature 55 lines below it in the same file.

The surrounding paragraph is otherwise **correct and load-bearing**: it explains why
`available_skills` is a pre-resolved slice instead of the lazy
`discoverAvailableSkills: () => AvailableSkill[]` closure upstream passes. That design has not
changed — the caller still resolves the config first and awaits the scan only when the feature is
enabled. Only the *justifying clause* is false. Correct the clause; keep the paragraph.

## Evidence

### 1. The stale clause (verified present, unchanged by this branch)

`crates/cyrup-ext-subagents/src/discovery/management.rs:1363-1369`:

```rust
/// **Why the availability list is pre-resolved rather than a closure.** Upstream passes a lazy
/// `discoverAvailableSkills: () => AvailableSkill[]` so that a disabled feature performs no
/// filesystem scan. cyrup's [`crate::discovery::skills::discover_available_skills`] is `async`
/// while [`handle_management_action`] is sync, so the laziness moves one level up: the async caller
/// checks [`crate::discovery::skills::resolve_proactive_skill_subagents_config`]'s `enabled` first
/// and only then awaits the scan, filling this field. Both upstream properties survive — no scan
/// when disabled, and no suggestions when the scan found nothing.
pub struct ProactiveSkillsInput<'a> {
```

`git diff <merge-base> -- crates/cyrup-ext-subagents/src/discovery/management.rs | grep 'is sync'`
returns nothing: the branch rewrote 243 lines of this file and never touched this one.

### 2. What it contradicts

Same file, `:1421` — written by this branch:

```rust
pub async fn handle_management_action(
```

The diff hunk that produced it:

```
-pub fn handle_management_action(
+pub async fn handle_management_action(
```

### 3. The sibling that WAS fixed (the pattern to mirror)

`crates/cyrup-ext-subagents/src/extension/tool/routing.rs:1053-1058`, from this branch's diff:

```
-        // (`agent-management.ts:765-770` @v0.43.0). cyrup's skill scan is `async` and
-        // `handle_management_action` is sync, so the laziness lives here instead: the config is
+        // (`agent-management.ts:765-770` @v0.43.0). cyrup's skill scan is `async`, so the
+        // laziness lives here rather than inside the handler: the config is
         // resolved first and the scan is awaited ONLY when the feature is enabled, which is the
```

The false clause was removed; the rationale was kept intact. Do exactly that in `management.rs`.

## Impact

Documentation only, no runtime effect — but it is a `pub` item's rustdoc, so it ships in generated
docs, and it asserts a constraint that no longer exists. A reader deciding where to put the next
lazily-resolved input is told the dispatch cannot await, and will keep pre-resolving in the caller
for a reason that has been gone since this commit. It is also precisely the statement a `git blame`
of this commit would be expected to have already corrected, since its twin 300 lines away was.

## Sweep — every "the dispatch is sync" claim in `cyrup-ext-subagents`

Ran across the whole crate — `grep -rn handle_management_action`, a copula-plus-`sync(hronous)`
regex over every `.rs`, and `cannot await` / `async caller` / `lazy` / `pre-resolv` variants — plus
every `*.md` in the repo:

```
grep -rnE '(is|are|was|were|being|stays?|remains?) +(a +)?([`a-z_]+ +)?sync(hronous)?\b' \
  --include=*.rs crates/cyrup-ext-subagents/src
```

| Site | Text | Verdict |
| --- | --- | --- |
| `discovery/management.rs:1366` | "while [`handle_management_action`] is sync" | **FALSE — fix (this task)** |
| `extension/tool/routing.rs:1053-1058` | clause already removed | already correct, leave |
| `discovery/skills.rs:611-612` | "`handle_list` is a synchronous dispatcher" | **TRUE — leave alone** |
| `discovery/management.rs:2617-2619` | "this closure is the sync shim that hands its result to the recommender" | TRUE — leave alone |
| `discovery/management.rs:2535` | "the availability scan is pre-resolved by the async caller rather than run lazily here" | TRUE, no sync claim — leave alone |

Verification that the two `handle_list` claims are still true:
`crates/cyrup-ext-subagents/src/discovery/management.rs:2545` is
`fn handle_list(cfg: &AgentDiscoveryConfig, req: &ManagementRequest) -> Result<…>` — no `async`
— and the dispatch arm at `:1427` is `"list" => handle_list(cfg, req),` with no `.await`.
`handle_list` was **not** converted by this branch.

No other occurrence exists. `management.rs:1366` is the only surviving instance in the crate, and
there are none in any `*.md`.

## Required change — one edit, one file

**File:** `crates/cyrup-ext-subagents/src/discovery/management.rs`

Replace lines 1363-1369 exactly.

**Find (verbatim, 7 lines; the only non-ASCII character is U+2014 `—` on the sixth line):**

```rust
/// **Why the availability list is pre-resolved rather than a closure.** Upstream passes a lazy
/// `discoverAvailableSkills: () => AvailableSkill[]` so that a disabled feature performs no
/// filesystem scan. cyrup's [`crate::discovery::skills::discover_available_skills`] is `async`
/// while [`handle_management_action`] is sync, so the laziness moves one level up: the async caller
/// checks [`crate::discovery::skills::resolve_proactive_skill_subagents_config`]'s `enabled` first
/// and only then awaits the scan, filling this field. Both upstream properties survive — no scan
/// when disabled, and no suggestions when the scan found nothing.
```

**Replace with (verbatim, 7 lines; same U+2014 em dash, now on the sixth line's "survive — no scan"):**

```rust
/// **Why the availability list is pre-resolved rather than a closure.** Upstream passes a lazy
/// `discoverAvailableSkills: () => AvailableSkill[]` so that a disabled feature performs no
/// filesystem scan. cyrup's [`crate::discovery::skills::discover_available_skills`] is `async`, so
/// the laziness lives one level up rather than inside the handler: the caller checks
/// [`crate::discovery::skills::resolve_proactive_skill_subagents_config`]'s `enabled` first and
/// only then awaits the scan, filling this field. Both upstream properties survive — no scan when
/// disabled, and no suggestions when the scan found nothing.
```

### What changed and why

- **Removed:** the clause ``while [`handle_management_action`] is sync`` — the sole false statement.
- **Removed:** the qualifier `async` from "the async caller". Still true, but with the contrast
  clause gone it no longer carries information, and `routing.rs`'s corrected twin does not use it.
- **Kept verbatim:** the bold lead sentence, the upstream closure citation, the `async` scan fact,
  the `resolve_proactive_skill_subagents_config` gating order, and the two-properties closer. These
  describe the design, which is unchanged and still correct.
- **Phrasing "one level up rather than inside the handler"** deliberately mirrors `routing.rs`'s
  "the laziness lives here rather than inside the handler", so the two halves of the same rationale
  read as one voice from either end.
- **Re-wrapped** lines 3-7 of the block to absorb the deletion. Every line stays ≤ 99 columns,
  matching the block's existing hand-wrap (the crate has no `rustfmt.toml`, and rustfmt's
  `wrap_comments` is off by default, so comment width is a hand convention, not a tool output).

## Explicitly OUT of scope

- **`discovery/skills.rs:610-617`.** Its "`handle_list` is a synchronous dispatcher" is verified
  TRUE above. Do not touch this paragraph, and do not "harmonise" it with the new `management.rs`
  wording.
- **`discovery/management.rs:2617-2619`** ("the sync shim") — describes the closure passed to
  `build_proactive_skill_subagent_recommendation_lines`, still sync. Leave it.
- **The stale `extension.rs::route_management_action` path references** at
  `discovery/skills.rs:613`, `discovery/management.rs:1286` and `discovery/management.rs:4799`.
  There is no `extension.rs` — the routing lives in `extension/tool/routing.rs`, and it did at the
  merge base too. Pre-existing, unrelated to the async conversion, not this task.
- **Any behavioural change.** No signature, no control flow, no test. `available_skills` stays a
  pre-resolved `&[AvailableSkill]`; the scan stays in `route_management_action`.

## Definition of Done

1. `crates/cyrup-ext-subagents/src/discovery/management.rs` contains the replacement block above,
   byte-for-byte, at what was lines 1363-1369.
2. `grep -rn 'handle_management_action.*is sync' crates/cyrup-ext-subagents/` returns nothing.
3. `grep -c 'is sync' crates/cyrup-ext-subagents/src/discovery/skills.rs` still returns `1` — the
   `handle_list` sentence at `:612` is untouched.
4. `git diff --stat` for the whole task shows exactly one file changed with **5 insertions and
   5 deletions** — the first two lines of the 7-line anchor are unchanged and must stay unchanged.
   Every changed line begins with `///`. No source line outside the doc comment moves. (Dry-run
   applied against the current file confirms exactly this diff; the 7-line anchor matches the file
   in exactly one place.)
5. `git diff -w` on `discovery/skills.rs` is empty.
6. `cargo doc -p cyrup-ext-subagents --no-deps` resolves both intra-doc links in the replaced block
   (`crate::discovery::skills::discover_available_skills` and
   `crate::discovery::skills::resolve_proactive_skill_subagents_config`) — both are unchanged from
   the original text, so this is a no-new-warnings check, not a new link to validate.
7. Crate still builds; no test changes are expected or permitted.
