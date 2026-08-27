---
stage: aug
status: done
updated: 2026-08-27 04:50
---

# Verification Record: Duplicate Type Pairs And The Worktree Legacy Surface (ALREADY COMPLETE)

## Verdict

**This task is already done. Every acceptance criterion below was checked one-by-one against the
working tree (level with `origin/main`, branch `claude/transcript-module-refactor-26arog`, HEAD
`df64e81`) and all seven hold. There is no remaining implementation work. Do not re-open it.**

The work landed in commit `cb7afa5` ("cyrup-ext-subagents hygiene queue, partial remediation, and
exec/mod.rs decomposition"), whose message records `UNIFY_DUPLICATE_TYPE_SHAPES (complete)`. Unlike
the sibling `UNIFY_PATH_AND_CLOCK_HELPERS` — which the same commit message honestly labels
*partial* — this one is complete in fact, not just in the commit message. This file is now a
verification record, not a plan.

Every line number below was read out of the file at the cited line; the original task's citations
had drifted and are superseded by these.

---

## Current state — per-pair evidence

### Pair 1 — `WatchdogLspRequest`: COLLAPSED

One definition remains, in the module that owns the concept.

- Surviving definition: [`watchdog/lsp_diagnostics.rs:87`](../../crates/cyrup-ext-subagents/src/watchdog/lsp_diagnostics.rs)
  — `pub struct WatchdogLspRequest`, `#[derive(Debug, Clone)]` at `:86`, doc comment at `:84-85`
  (`` `WatchdogLspRequest` (`lsp-diagnostics.ts:14-20`) — also the argument bag the runtime's
  [`WatchdogLspDiagnostics::collect`] seam takes (`runtime.ts:727-733`) ``). The surviving shape is
  the *lsp_diagnostics* one, i.e. the stronger typing won: `cwd: PathBuf`, `root: PathBuf`,
  `changed_paths: Vec<String>`, `config: WatchdogLspConfig`, `signal: Option<CancelToken>`.
- The former second definition at `watchdog/runtime.rs:220` (with the drifted `root: String` /
  `cancel: CancelToken`) is gone. `runtime.rs` now *imports* the type:
  [`watchdog/runtime.rs:63`](../../crates/cyrup-ext-subagents/src/watchdog/runtime.rs) —
  `use super::lsp_diagnostics::{TypeScriptLspDiagnostics, WatchdogLspRequest};`
- The `RuntimeLspRequest` alias and its field-copy shim are gone:
  `grep -rn 'RuntimeLspRequest' crates/cyrup-ext-subagents/src/` returns nothing (exit 1).

### Pair 2 — `WatchdogRepoChangeSignature`: COLLAPSED

- Surviving definition: [`watchdog/change_signature.rs:90`](../../crates/cyrup-ext-subagents/src/watchdog/change_signature.rs)
  — `pub struct WatchdogRepoChangeSignature`, `#[derive(Debug, Clone, PartialEq, Eq)]` at `:89`,
  full doc comment at `:80-88` (the one explaining that the runtime uses `key` for two decisions
  and `changed_paths` for two more). Fields unchanged: `root: String`, `key: String`,
  `changed_paths: Vec<String>`.
- The former second definition at `watchdog/runtime.rs:284` is gone; `runtime.rs:59-61` imports it:
  `use super::change_signature::{event_indicates_repo_edit, GitRepoChangeSource,
  WatchdogRepoChangeSignature};`
- The field-copy shim formerly at `change_signature.rs:572-581` is gone. `GitRepoChangeSource`'s
  `WatchdogRepoChangeSource` impl at `change_signature.rs:580-584` now returns
  `compute_watchdog_repo_change_signature(cwd)` directly, with no per-field reconstruction.

### Pair 3 — `HookSpec`: COLLAPSED (via type alias, exactly as the criterion permitted)

- Canonical definition: [`registration/mod.rs:447`](../../crates/cyrup-ext-subagents/src/registration/mod.rs)
  — `pub struct HookSpec`, under the banner `// HookSpec (func-SA §4.7; arch-SA §3.8) — the
  canonical definition` at `:437`, with
  `#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]` at `:445` and
  `#[serde(rename_all = "camelCase")]` at `:446`. The serde surface — the half the old worktree
  copy lacked — is preserved on the survivor, which is the correct direction.
- [`spawn/worktree.rs:1088`](../../crates/cyrup-ext-subagents/src/spawn/worktree.rs) is now
  `pub type HookSpec = crate::registration::HookSpec;`, documented at `:1082-1087` as *"an alias of
  the canonical [`crate::registration::HookSpec`] (arch-SA §2.2 designates `registration/mod.rs` as
  its owner)"*, and retaining the honest caveat that its `args` are ignored by the per-worktree
  invocation.
- The false *"the two shapes are guaranteed identical field-for-field"* deferral comment formerly
  at `registration/mod.rs:489-499` is gone: `grep -rn 'guaranteed identical'` over the crate returns
  nothing (exit 1). `registration/mod.rs:444` now states the accurate relationship instead
  (*"the only one: [`crate::spawn::worktree::HookSpec`] is a type alias to it"*).
- Round-trip coverage survived the collapse: `registration/mod.rs:1180-1190` still serialises and
  deserialises a `HookSpec` under the comment `// HookSpec (aliased as spawn::worktree::HookSpec)`.

### The worktree "legacy compatibility surface": REMOVED

- `grep -rn 'Legacy compatibility surface' crates/cyrup-ext-subagents/src/` returns nothing
  (exit 1). The section banner at [`spawn/worktree.rs:1078-1080`](../../crates/cyrup-ext-subagents/src/spawn/worktree.rs)
  now reads `// Group-level wrappers over the pi-faithful primitives`, and the module doc heading at
  `:32` matches it. Neither text calls the shapes deprecated any more.
- The three doc-link-only items are deleted crate-wide. `grep -rn
  'check_clean_working_tree\|reject_task_level_cwd_overrides\|DEFAULT_HOOK_TIMEOUT'
  crates/cyrup-ext-subagents/src/` returns nothing (exit 1) — so their bodies, their self-tests at
  the old `worktree.rs:1943-1955`, and the `retained for doc-links` justifications (`grep -rn
  'retained for doc-links'` → nothing) are all gone together.
- The three referring doc-links were re-pointed at pi-faithful primitives, not merely deleted:
  - [`spawn/chain_graph.rs:82-83`](../../crates/cyrup-ext-subagents/src/spawn/chain_graph.rs) (was
    the `reject_task_level_cwd_overrides` link) now reads *"enforced by
    [`crate::spawn::worktree::find_worktree_task_cwd_conflict`], called inside
    [`crate::spawn::worktree::setup_worktree_group`]"*.
  - [`spawn/chain_graph.rs:1278-1279`](../../crates/cyrup-ext-subagents/src/spawn/chain_graph.rs)
    (was the `check_clean_working_tree` link) now points at
    [`crate::spawn::worktree::create_worktrees`].
  - [`registration/mod.rs:142-145`](../../crates/cyrup-ext-subagents/src/registration/mod.rs) (was
    the `DEFAULT_HOOK_TIMEOUT` link) now names
    `spawn::worktree::DEFAULT_WORKTREE_SETUP_HOOK_TIMEOUT_MS` — the pi-faithful constant at
    [`spawn/worktree.rs:49`](../../crates/cyrup-ext-subagents/src/spawn/worktree.rs) — and says
    explicitly that the value is "not duplicated here".
- The load-bearing part of the wrapper was correctly *kept*, as the task's own IMPORTANT note
  required: `WorktreeGroupConfig` (`worktree.rs:1092`), `WorktreeAssignment` (`:1105`),
  `WorktreeGroupPlan` (`:1120`) and `setup_worktree_group` (`:1145`) still exist because
  `chain_graph.rs:1891-1899` constructs the config and binds the returned plan by inference. The
  full pi migration of that one caller was the task's *optional* stretch ("if the full pi migration
  is out of scope, the minimum acceptable outcome is…"), not an acceptance criterion, and it is
  correctly absent. Do not treat its absence as unfinished work under this task.

### Remaining crate-wide duplicate names (out of scope — recorded, not actionable here)

`grep -rhoE '^pub struct [A-Za-z0-9_]+' crates/cyrup-ext-subagents/src/ | sort | uniq -d` now
returns **three** names, down from the original seven, and none of them is in this task's scope:

| Name | Sites | Status |
| --- | --- | --- |
| `ProactiveSkillSubagentsConfig` | [`registration/mod.rs:549`](../../crates/cyrup-ext-subagents/src/registration/mod.rs), [`discovery/skills.rs:321`](../../crates/cyrup-ext-subagents/src/discovery/skills.rs) | Explicitly excluded by this task: a documented deliberate split (pi's `Config \| false` vs the recommender's three states). |
| `AcceptanceLedger` | [`exec/acceptance/model/types.rs:535`](../../crates/cyrup-ext-subagents/src/exec/acceptance/model/types.rs), [`exec/acceptance/lattice/mod.rs:129`](../../crates/cyrup-ext-subagents/src/exec/acceptance/lattice/mod.rs) | Named in this task's Evidence list but never in its Acceptance Criteria or Suggested Approach. `cb7afa5` calls this "the known-untracked C12 acceptance collapse". Not this task's work. |
| `NestedRunSummary` | [`tui/mod.rs:190`](../../crates/cyrup-ext-subagents/src/tui/mod.rs), [`spawn/nested_events.rs:187`](../../crates/cyrup-ext-subagents/src/spawn/nested_events.rs) | Same: listed in Evidence only, never in the criteria. Not this task's work. |

`NdjsonLine` is no longer duplicated — a single definition remains at
[`exec/ndjson.rs:328`](../../crates/cyrup-ext-subagents/src/exec/ndjson.rs), collapsed by the
sibling `COLLAPSE_NDJSON_PARSERS` task in the same commit.

---

## Original Acceptance Criteria — checked one by one

- [x] `grep -rhoE '^pub struct [A-Za-z0-9_]+' src/ | sort | uniq -d` no longer lists
      `WatchdogLspRequest`, `WatchdogRepoChangeSignature` or `HookSpec` — **confirmed**, the command
      returns only `AcceptanceLedger`, `NestedRunSummary`, `ProactiveSkillSubagentsConfig`.
- [x] The three field-copy shims are gone: `grep -n 'RuntimeLspRequest'
      src/watchdog/lsp_diagnostics.rs` returns 0 matches, and `change_signature.rs`'s copy block no
      longer exists (the impl delegates directly) — **confirmed**.
- [x] `WatchdogLspDiagnostics::collect`'s trait signature takes the single surviving request type —
      **confirmed** at [`watchdog/runtime.rs:233`](../../crates/cyrup-ext-subagents/src/watchdog/runtime.rs):
      `async fn collect(&self, request: WatchdogLspRequest) -> Result<WatchdogLspResult, String>;`
      with the type imported from `lsp_diagnostics` at `:63`. The production impl at
      [`watchdog/lsp_diagnostics.rs:1101`](../../crates/cyrup-ext-subagents/src/watchdog/lsp_diagnostics.rs)
      matches, and both in-crate test doubles (`runtime.rs:3096`, `:3117`) use the same type.
- [x] `spawn::worktree::HookSpec` is a type alias to the registration shape and the "guaranteed
      identical field-for-field" claim is removed — **confirmed** (`worktree.rs:1088`; the claim
      greps to zero).
- [x] `check_clean_working_tree`, `reject_task_level_cwd_overrides` and `DEFAULT_HOOK_TIMEOUT` are
      deleted along with their self-tests, and the three referring doc-links point at the
      pi-faithful primitives instead — **confirmed** (see the four sites cited above).
- [x] `grep -n 'Legacy compatibility surface' src/spawn/worktree.rs` returns 0 — **confirmed**.
- [x] The crate still compiles — **confirmed**: `cargo check -p cyrup-ext-subagents --lib` exits 0
      with no warnings emitted for this crate (run at HEAD `df64e81`, 1m21s, clean finish). The
      full `cargo test -p cyrup-ext-subagents` run was not executed under this augmentation pass's
      read-only constraint; see Definition of Done for the exact command to confirm it.

---

## Definition of Done

All commands are run from the repository root `/home/user/cyrup`. Every one of these already passes
at HEAD `df64e81`; they are recorded so any future reviewer can re-confirm in under two minutes.

One-definition greps — each MUST return exactly `1`:

```sh
grep -rn '^pub struct WatchdogLspRequest'          crates/cyrup-ext-subagents/src/ | wc -l    # -> 1  (lsp_diagnostics.rs:87)
grep -rn '^pub struct WatchdogRepoChangeSignature' crates/cyrup-ext-subagents/src/ | wc -l    # -> 1  (change_signature.rs:90)
grep -rn '^pub struct HookSpec'                    crates/cyrup-ext-subagents/src/ | wc -l    # -> 1  (registration/mod.rs:447)
grep -rn '^pub type HookSpec'                      crates/cyrup-ext-subagents/src/ | wc -l    # -> 1  (spawn/worktree.rs:1088, the alias)
```

Zero-match greps — each MUST return `0`:

```sh
grep -rc 'RuntimeLspRequest'              crates/cyrup-ext-subagents/src/ | grep -v ':0$' | wc -l   # -> 0
grep -rc 'guaranteed identical'           crates/cyrup-ext-subagents/src/ | grep -v ':0$' | wc -l   # -> 0
grep -rc 'Legacy compatibility surface'   crates/cyrup-ext-subagents/src/ | grep -v ':0$' | wc -l   # -> 0
grep -rc 'retained for doc-links'         crates/cyrup-ext-subagents/src/ | grep -v ':0$' | wc -l   # -> 0
grep -rEc 'check_clean_working_tree|reject_task_level_cwd_overrides|DEFAULT_HOOK_TIMEOUT[^_]' \
     crates/cyrup-ext-subagents/src/ -r | grep -v ':0$' | wc -l                                    # -> 0
```

Whole-crate duplicate census — MUST list exactly the three out-of-scope names and nothing else:

```sh
grep -rhoE '^pub struct [A-Za-z0-9_]+' crates/cyrup-ext-subagents/src/ | sort | uniq -d
# AcceptanceLedger
# NestedRunSummary
# ProactiveSkillSubagentsConfig
```

Build and existing-test verification (no new tests are to be written; these only confirm the
existing suite is unaffected):

```sh
cargo check -p cyrup-ext-subagents --lib      # exits 0  [verified 2026-08-27]
cargo test  -p cyrup-ext-subagents            # no new failures vs. the ~2483-test baseline
cargo build -p cyrup-it --tests               # still compiles (cross-crate consumers unaffected)
cargo doc   -p cyrup-ext-subagents --no-deps --lib   # 0 broken intra-doc links; the workspace
                                                     # pins broken_intra_doc_links = "deny", so the
                                                     # re-pointed doc-links above are gated here
```

## Source

- Identified by the `subagents-hygiene-survey` workflow (13 agents, 21 raw findings, 16 confirmed
  after adversarial verification).
- Implemented in commit `cb7afa5`; verified complete by this `/aug` pass on 2026-08-27.
