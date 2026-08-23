---
title: Proactiveskillsinput Doc Still Calls The Dispatch Sync
priority: MEDIUM
stage: qa
status: completed
updated: 2026-08-23 08:11
---

# `ProactiveSkillsInput`'s rustdoc still says `handle_management_action` is sync

## Problem

The async conversion that landed on `main` made `handle_management_action` a `pub async fn`. Two
comments in `cyrup-ext-subagents` carried the clause "`handle_management_action` is sync" as the
*reason* for a design decision. The one inside `SubagentTool::route_management_action`
([`extension/tool/routing.rs`](../../crates/cyrup-ext-subagents/src/extension/tool/routing.rs)) was
corrected at the time. Its twin — in the rustdoc of the **public** `ProactiveSkillsInput` struct in
[`discovery/management.rs`](../../crates/cyrup-ext-subagents/src/discovery/management.rs) — was
not, and now states the opposite of the signature declared further down the same file.

The surrounding paragraph is otherwise **correct and load-bearing**: it explains why
`available_skills` is a pre-resolved slice instead of the lazy
`discoverAvailableSkills: () => AvailableSkill[]` closure upstream passes. That design has not
changed — `route_management_action` still resolves the config first and awaits the scan only when
the feature is enabled. Only the *justifying clause* is false. Correct the clause; keep the
paragraph.

## Verified current state

Every pointer below was re-checked against the tree as it exists on disk at the time of this
augmentation. Prefer the **name** column when navigating; the line numbers are a convenience and
are the part that rots.

| Name / anchor | File | Line (verified) | Fact |
| --- | --- | --- | --- |
| `ProactiveSkillsInput` rustdoc, "Why the availability list is pre-resolved" paragraph | `crates/cyrup-ext-subagents/src/discovery/management.rs` | 1363-1369 | **Contains the false clause. This task's only edit.** |
| the false clause itself | same | 1366 | reads "while [`handle_management_action`] is sync, so the laziness moves one level up: the async caller" |
| `pub struct ProactiveSkillsInput<'a> {` | same | 1370 | line immediately after the doc block |
| `pub async fn handle_management_action(` | same | 1421 | `async` — contradicts the clause |
| `"list" => handle_list(cfg, req),` dispatch arm | same | 1427 | no `.await` |
| `fn handle_list(cfg: &AgentDiscoveryConfig, req: &ManagementRequest) -> Result<ManagementOutcome, SubagentError>` | same | 2545 | still **not** `async` |
| `handle_list` rustdoc, "pre-resolved by the async caller" sentence | same | 2535 | true, no sync claim |
| "this closure / is the sync shim that hands its result to the recommender" | same | **2623-2625** | true — closure passed to `build_proactive_skill_subagent_recommendation_lines` |
| corrected twin, inside `async fn route_management_action` (declared at `:1024`) | `crates/cyrup-ext-subagents/src/extension/tool/routing.rs` | **1053-1059** | clause already removed; this is the wording to mirror |
| `build_proactive_skill_subagent_recommendation_lines` rustdoc, "# Where it is wired" | `crates/cyrup-ext-subagents/src/discovery/skills.rs` | 610-617 | true; "is sync" at `:612` refers to **pi's** closure and to `handle_list` |

### Citation drift corrected during this pass

- "the sync shim" comment: previously cited as `management.rs:2617-2619`; it is now at
  **`:2623-2625`** (`// The availability scan already happened …` / `// is the sync shim …` /
  `// upstream's — is never called when the feature is disabled.`).
- The corrected twin in `routing.rs`: previously cited as `:1053-1058`; the comment block runs
  **`:1053-1059`**, and the two rewritten lines are `:1055-1056`.
- Previously claimed "its twin 300 lines away". The twin is in a **different file**
  (`extension/tool/routing.rs`), not 300 lines down `management.rs`.
- Previously claimed the doc block wraps at "≤ 99 columns". The block's own longest line is
  **100** characters (the offending `:1366`). The file has doc lines up to 115 and there is **no
  `rustfmt.toml` anywhere in the workspace**, so comment width is a hand convention (~95-100), not
  tool output. The replacement below stays within 61-100, i.e. within the block's existing range.
- Anchor `management.rs:1363-1369` re-verified as **still accurate** — the recent `cyrup-config`
  and `crates/cyrup/src/main.rs` churn did not touch this file.

### Sweep — every "the dispatch is sync" claim in `cyrup-ext-subagents`

```
grep -rnE '(is|are|was|were|being|stays?|remains?) +(a +)?([`a-z_]+ +)?sync(hronous)?\b' \
  --include=*.rs crates/cyrup-ext-subagents/src
```

Re-run during this pass. 21 hits crate-wide; only the ones below concern the management dispatch:

| Site | Text | Verdict |
| --- | --- | --- |
| `discovery/management.rs:1366` | "while [`handle_management_action`] is sync" | **FALSE — fix (this task)** |
| `extension/tool/routing.rs` (`route_management_action`) | clause already removed | already correct, leave |
| `discovery/skills.rs:612` | "is sync, and `handle_list` is a synchronous dispatcher" | **TRUE — leave alone** |
| `discovery/management.rs:2624` | "is the sync shim that hands its result to the recommender" | TRUE — leave alone |
| `discovery/management.rs:2535` | "pre-resolved by the async caller rather than run lazily here" | TRUE, no sync claim — leave alone |

Confirmations run against the current tree:

- `grep -c 'is sync' crates/cyrup-ext-subagents/src/discovery/management.rs` → `1` (line 1366).
- `grep -c 'is sync' crates/cyrup-ext-subagents/src/extension/tool/routing.rs` → `0`.
- `grep -c 'is sync' crates/cyrup-ext-subagents/src/discovery/skills.rs` → `1` (line 612).
- No `*.md` under `crates/` makes the claim.

## Required change — one edit, one file, one occurrence

**File:** `crates/cyrup-ext-subagents/src/discovery/management.rs`

**Find (verbatim, 7 lines, 653 bytes, `md5sum` = `0eb45a2a60cd647247df66396e4ee28b`).** The only
non-ASCII byte sequence is one U+2014 EM DASH on line 6 (`survive — no scan`); everything else is
ASCII. This block occurs in the file **exactly once** — assert that count is `1` before writing.

```rust
/// **Why the availability list is pre-resolved rather than a closure.** Upstream passes a lazy
/// `discoverAvailableSkills: () => AvailableSkill[]` so that a disabled feature performs no
/// filesystem scan. cyrup's [`crate::discovery::skills::discover_available_skills`] is `async`
/// while [`handle_management_action`] is sync, so the laziness moves one level up: the async caller
/// checks [`crate::discovery::skills::resolve_proactive_skill_subagents_config`]'s `enabled` first
/// and only then awaits the scan, filling this field. Both upstream properties survive — no scan
/// when disabled, and no suggestions when the scan found nothing.
```

**Replace with (verbatim, 7 lines).** One U+2014 EM DASH, now on line 6 (`survive — no scan when`).

```rust
/// **Why the availability list is pre-resolved rather than a closure.** Upstream passes a lazy
/// `discoverAvailableSkills: () => AvailableSkill[]` so that a disabled feature performs no
/// filesystem scan. cyrup's [`crate::discovery::skills::discover_available_skills`] is `async`, so
/// the laziness lives one level up rather than inside the handler: the caller checks
/// [`crate::discovery::skills::resolve_proactive_skill_subagents_config`]'s `enabled` first and
/// only then awaits the scan, filling this field. Both upstream properties survive — no scan when
/// disabled, and no suggestions when the scan found nothing.
```

Lines 1-2 of the block are byte-identical between find and replace and must remain untouched; the
edit is confined to lines 3-7 (file lines 1365-1369). Result: 5 lines out, 5 lines in, file total
stays 5544 lines, every changed line begins with `/// `.

### Required application method

Do not hand-retype the block. Apply it so the byte-exactness and the single-occurrence property are
machine-enforced:

```bash
python3 - <<'PY'
p = 'crates/cyrup-ext-subagents/src/discovery/management.rs'
find = (
"/// **Why the availability list is pre-resolved rather than a closure.** Upstream passes a lazy\n"
"/// `discoverAvailableSkills: () => AvailableSkill[]` so that a disabled feature performs no\n"
"/// filesystem scan. cyrup's [`crate::discovery::skills::discover_available_skills`] is `async`\n"
"/// while [`handle_management_action`] is sync, so the laziness moves one level up: the async caller\n"
"/// checks [`crate::discovery::skills::resolve_proactive_skill_subagents_config`]'s `enabled` first\n"
"/// and only then awaits the scan, filling this field. Both upstream properties survive — no scan\n"
"/// when disabled, and no suggestions when the scan found nothing.\n"
)
repl = (
"/// **Why the availability list is pre-resolved rather than a closure.** Upstream passes a lazy\n"
"/// `discoverAvailableSkills: () => AvailableSkill[]` so that a disabled feature performs no\n"
"/// filesystem scan. cyrup's [`crate::discovery::skills::discover_available_skills`] is `async`, so\n"
"/// the laziness lives one level up rather than inside the handler: the caller checks\n"
"/// [`crate::discovery::skills::resolve_proactive_skill_subagents_config`]'s `enabled` first and\n"
"/// only then awaits the scan, filling this field. Both upstream properties survive — no scan when\n"
"/// disabled, and no suggestions when the scan found nothing.\n"
)
src = open(p, encoding='utf-8').read()
n = src.count(find)
assert n == 1, f'expected exactly 1 match, found {n} - STOP, do not edit'
open(p, 'w', encoding='utf-8').write(src.replace(find, repl))
print('applied')
PY
```

### What changed and why

- **Removed:** the clause ``while [`handle_management_action`] is sync`` — the sole false
  statement, and with it the now-pointless intra-doc link to `handle_management_action` (the item
  is still linked from the `ManagementAction` rustdoc at `management.rs:1395`, so nothing is
  orphaned).
- **Removed:** the qualifier `async` from "the async caller". Still true, but with the contrast
  clause gone it carries no information — "awaits the scan" already implies it — and `routing.rs`'s
  corrected twin does not use it either.
- **Kept verbatim:** the bold lead sentence, the upstream closure citation, the `async` scan fact,
  the `resolve_proactive_skill_subagents_config` gating order, and the two-properties closer. These
  describe the design, which is unchanged and still correct.
- **Both intra-doc links are carried over byte-identically** —
  `[`crate::discovery::skills::discover_available_skills`]` and
  `[`crate::discovery::skills::resolve_proactive_skill_subagents_config`]` — so no link resolution
  changes by construction.
- **Phrasing "one level up rather than inside the handler"** deliberately mirrors `routing.rs`'s
  "the laziness lives here rather than inside the handler", so the two halves of the same rationale
  read as one voice from either end.
- **Re-wrapped** lines 3-7 to absorb the deletion, staying inside the block's existing 61-100
  column range.

## Explicitly OUT of scope

- **`discovery/skills.rs:610-617`.** Its `:612` "is sync, and `handle_list` is a synchronous
  dispatcher" is verified TRUE above (pi's closure is sync; `handle_list` at `management.rs:2545`
  has no `async`). Do not touch this paragraph, and do not "harmonise" it with the new
  `management.rs` wording.
- **`discovery/management.rs:2623-2625`** ("the sync shim") — describes the closure passed to
  `build_proactive_skill_subagent_recommendation_lines`, still sync. Leave it.
- **The stale `extension.rs::route_management_action` path references** at `discovery/skills.rs:613`,
  `discovery/management.rs:1286` and `discovery/management.rs:4799`, and the bare `extension.rs`
  references at `discovery/management.rs:1336`, `:1396` and `discovery/mod.rs:113`, `:319`. There is
  no `extension.rs` — `extension/` is a directory and the routing lives in
  `extension/tool/routing.rs`. Pre-existing, unrelated to the async conversion, not this task.
- **Any behavioural change.** No signature, no control flow. `available_skills` stays a pre-resolved
  `&[AvailableSkill]`; the scan stays in `route_management_action`.
- **Tests, benchmarks and new documentation.** Another team owns those. Do not add, rename, or
  modify any `#[test]`/`#[tokio::test]`, and do not write new doc pages.

## Definition of Done

All checks are run from the repo root, `/home/user/cyrup`. No git command is required or permitted.

1. **The edit applied cleanly.** The apply script above printed `applied` (its assertion guarantees
   the anchor matched exactly once and byte-for-byte).
2. **The false clause is gone crate-wide:**
   `grep -rn 'handle_management_action.*is sync' crates/cyrup-ext-subagents/` returns nothing
   (exit status 1).
3. **`management.rs` has no `is sync` left:**
   `grep -c 'is sync' crates/cyrup-ext-subagents/src/discovery/management.rs` returns `0`.
4. **Neighbours untouched:**
   `grep -c 'is sync' crates/cyrup-ext-subagents/src/discovery/skills.rs` still returns `1`, and
   `grep -n 'is the sync shim' crates/cyrup-ext-subagents/src/discovery/management.rs` still
   returns line `2624`.
5. **Structure preserved:**
   `wc -l crates/cyrup-ext-subagents/src/discovery/management.rs` returns `5544`;
   `sed -n '1370p' crates/cyrup-ext-subagents/src/discovery/management.rs` returns
   `pub struct ProactiveSkillsInput<'a> {`; and
   `sed -n '1363,1369p' … | grep -cv '^/// '` returns `0` (every line of the block is still a doc
   comment).
6. **The replacement block is present byte-for-byte:**
   `sed -n '1363,1369p' crates/cyrup-ext-subagents/src/discovery/management.rs | md5sum` returns
   `c729a53ac60f529f6dac92eb3bb29ce2`. *(Recompute rather than trust this digest if any other change
   to the same block has landed since; checks 2-5 and 7 are the authoritative ones.)*
7. **Rustdoc still resolves both links:**
   `cargo doc -p cyrup-ext-subagents --no-deps 2>&1 | grep -i 'broken.intra.doc\|unresolved link'`
   returns nothing. Both links are unchanged from the original text, so this is a
   no-new-warnings check, not a new link to validate.
8. **Crate still compiles:** `cargo check -p cyrup-ext-subagents` succeeds. Expected to be a
   near-no-op — only comment bytes changed.

## QA verdict — 2026-08-23 08:11

**Rating: 9/10. PASS.**

The defect is fixed and nothing false was introduced. Verified against the tree on disk:

- `management.rs:1363-1369` now reads the replacement block byte-for-byte
  (`sed -n '1363,1369p' | md5sum` = `c729a53ac60f529f6dac92eb3bb29ce2`). The false clause
  ``while [`handle_management_action`] is sync`` is gone.
- Every DoD check passes: `grep -rn 'handle_management_action.*is sync' crates/cyrup-ext-subagents/`
  → no hits; `grep -c 'is sync' …/discovery/management.rs` → 0; `…/discovery/skills.rs` → 1
  (untouched); `grep -n 'is the sync shim' …/management.rs` → 2624; `wc -l` → 5544;
  `sed -n '1370p'` → `pub struct ProactiveSkillsInput<'a> {`; all 7 block lines still `/// `.
- Every factual claim in the *new* text checked against source, not taken on the comment's word:
  `discover_available_skills` is `pub async fn` (`discovery/skills.rs:239`); the caller
  `route_management_action` (`extension/tool/routing.rs`) does resolve
  `resolve_proactive_skill_subagents_config(...).enabled` (a plain `fn`, `skills.rs:419`) *before*
  `.await`ing the scan; "no scan when disabled" holds via that `&&` guard; "no suggestions when the
  scan found nothing" holds because `recommend_proactive_skill_subagents` filters on
  `available_by_name.contains_key(skill)` with `Some(&[])`, so an empty scan yields an empty list.
- The clause that *was* false is now absent rather than merely reworded, and the sentence no longer
  makes any sync/async claim about `handle_management_action` (still `pub async fn` at `:1421`).
- Out-of-scope neighbours were correctly left alone: `skills.rs:612` ("is sync, and `handle_list` is
  a synchronous dispatcher") is still TRUE — `handle_list` at `management.rs:2545` has no `async`;
  `management.rs:2624` "the sync shim" is still TRUE.
- Both intra-doc links carried over unchanged; `cargo doc -p cyrup-ext-subagents --no-deps` emits no
  broken-intra-doc/unresolved-link warnings. `cargo check -p cyrup-ext-subagents` succeeds.
- Dropping the `handle_management_action` intra-doc link orphans nothing: the item is still linked
  from the `MANAGEMENT_ACTIONS` rustdoc at `management.rs:1395`.

Not deducted (explicitly out of scope, pre-existing, unrelated to the async conversion): the stale
`extension.rs::route_management_action` path references at `skills.rs:613`, `management.rs:1286`
and `:4799`, and the bare `extension.rs` mentions at `management.rs:1336`, `:1396`,
`discovery/mod.rs:113`, `:319`. Worth a follow-up task, not a blocker here.
