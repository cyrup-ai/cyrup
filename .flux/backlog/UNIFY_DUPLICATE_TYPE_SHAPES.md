---
stage: exec
status: done
updated: 2026-08-23 02:15
---

# Finish The Deferred Port Migrations: Duplicate Type Pairs And The Worktree Legacy Surface

## Description

Several port migrations were explicitly deferred and never done, leaving duplicate type names bridged by hand-written field-copy shims and a self-declared 'legacy compatibility surface' that is mostly unreachable.

Seven `pub struct` names are defined twice in the crate. Three pairs are unexamined duplication with concrete cost. `WatchdogLspRequest` exists in both `watchdog/runtime.rs` and `watchdog/lsp_diagnostics.rs` and has already drifted in field types (`root: String` vs `root: PathBuf`, `cancel: CancelToken` vs `signal: Option<CancelToken>`), so lsp_diagnostics.rs imports the other under an alias and copies it field-by-field just to call the collector — a field added to one is silently absent from the other. `WatchdogRepoChangeSignature` is structurally identical in both places and bridged by a pure field-copy. `HookSpec` has a deferred-migration comment at registration/mod.rs:489-499 that asserts 'the two shapes are guaranteed identical field-for-field', which is already untrue: registration's derives `Serialize`/`Deserialize` with `rename_all = "camelCase"`, worktree's derives neither. Because the names are identical, a mismatch reads as `expected WatchdogLspRequest, found WatchdogLspRequest` and a reader must check the `use` line to know which type a call site means.

The same half-crossed migration shows up in `spawn/worktree.rs`, whose section header literally reads 'Legacy compatibility surface (thin wrappers over the pi-faithful primitives)' and whose module doc says these exist so `chain_graph::assign_worktree_cwds` keeps compiling 'while the crate converges on pi's `create_worktrees`/`diff_worktrees`/`cleanup_worktrees` contract'. Three of its public items have no caller at all beyond a single rustdoc reference — their own docs say 'retained for doc-links' — and those doc-links actively steer readers from `chain_graph.rs` and `registration/mod.rs` toward the deprecated shapes rather than the pi-faithful ones.

## Evidence

`grep -rhoE '^pub struct [A-Za-z0-9_]+' src/ | sort | uniq -d` returns 7 names: AcceptanceLedger, HookSpec, NdjsonLine, NestedRunSummary, ProactiveSkillSubagentsConfig, WatchdogLspRequest, WatchdogRepoChangeSignature. Pair sites and shims read directly: WatchdogLspRequest at src/watchdog/runtime.rs:220 (`root: String`, `cancel: CancelToken`) and src/watchdog/lsp_diagnostics.rs:86 (`root: PathBuf`, `signal: Option<CancelToken>`), aliased import at lsp_diagnostics.rs:56 and field-copy shim at :1099-1108. WatchdogRepoChangeSignature at runtime.rs:284 and change_signature.rs:82, identical (`root: String`, `key: String`, `changed_paths: Vec<String>`), field-copy at change_signature.rs:572-581. HookSpec at registration/mod.rs:502 (derives Serialize/Deserialize + `#[serde(rename_all="camelCase")]` at :500-501) and spawn/worktree.rs:1098-1104 (derives only Debug/Clone/PartialEq/Eq; its doc says its `args` 'are currently ignored by the per-worktree invocation'); the deferral comment is at registration/mod.rs:489-499. Worktree legacy section header at src/spawn/worktree.rs:1090-1092, module doc at :32-38, 8 items: HookSpec:1099, WorktreeGroupConfig:1108, WorktreeAssignment:1121, WorktreeGroupPlan:1136, setup_worktree_group:1161, check_clean_working_tree:1218, reject_task_level_cwd_overrides:1242, DEFAULT_HOOK_TIMEOUT:54. Per-name grep excluding worktree.rs: check_clean_working_tree -> only chain_graph.rs:1288 (doc-link), reject_task_level_cwd_overrides -> only chain_graph.rs:82 (doc-link), DEFAULT_HOOK_TIMEOUT -> only registration/mod.rs:191 (doc-link); their own docs say 'retained for doc-links' at :1210-1211, :1235-1236, :55-56. IMPORTANT: WorktreeAssignment and WorktreeGroupPlan are NOT dead — chain_graph.rs:1907-1911 binds the returned `WorktreeGroupPlan` and iterates `plan.assignments` reading `assignment.path` via type inference, so a name-grep misses them and deleting the section wholesale would break the build. Excluded from scope: the ProactiveSkillSubagentsConfig pair is a documented deliberate split (discovery/skills.rs:328-341 explains pi's `Config | false` vs the recommender's three states), and NdjsonLine is covered by COLLAPSE_NDJSON_PARSERS.

## Suggested approach

For each pair, pick the module that owns the concept and delete the other definition: move WatchdogRepoChangeSignature to change_signature.rs and import it from runtime.rs; move WatchdogLspRequest to lsp_diagnostics.rs and change the collector trait signature; do the alias registration/mod.rs:494 already specifies for HookSpec. For the worktree surface, migrate the one live caller (chain_graph.rs:1900-1911) onto pi's `create_worktrees` contract FIRST — it consumes `WorktreeGroupPlan`/`WorktreeAssignment` by inference — then delete the doc-link-only items and repoint the three comments. If the full pi migration is out of scope, the minimum acceptable outcome is deleting the three doc-link-only items and fixing their referring comments, so the section shrinks to the wrapper that is actually load-bearing.

## Acceptance Criteria

- [ ] `grep -rhoE '^pub struct [A-Za-z0-9_]+' src/ | sort | uniq -d` no longer lists WatchdogLspRequest, WatchdogRepoChangeSignature or HookSpec
- [ ] The three field-copy shims are gone: `grep -n 'RuntimeLspRequest' src/watchdog/lsp_diagnostics.rs` returns 0, and change_signature.rs:572-581's copy block no longer exists
- [ ] `WatchdogLspDiagnostics::collect`'s trait signature takes the single surviving request type
- [ ] `spawn::worktree::HookSpec` is a type alias to the registration shape (or is deleted with worktree's callers using registration's), and the 'guaranteed identical field-for-field' claim at registration/mod.rs:489-499 is removed
- [ ] `check_clean_working_tree`, `reject_task_level_cwd_overrides` and `DEFAULT_HOOK_TIMEOUT` are deleted along with their self-tests at worktree.rs:1943-1955, and the three referring doc-links (chain_graph.rs:1288, chain_graph.rs:82, registration/mod.rs:191) point at the pi-faithful primitives instead
- [ ] `grep -n 'Legacy compatibility surface' src/spawn/worktree.rs` returns 0
- [ ] `cargo test -p cyrup-ext-subagents` passes with no new failures and `cargo build -p cyrup-it --tests` still compiles

## Source

- Identified by the `subagents-hygiene-survey` workflow (13 agents, 21 raw findings, 16 confirmed after adversarial verification).
- Effort: medium · survey priority: 5 of 6
