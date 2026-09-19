---
stage: qa
status: completed
updated: 2026-09-19
---

# Parallel handoff + lanes — the coordination layer over worktree fan-out

OBJECTIVE: make a parallel worktree fan-out **converge**. Today cyrup can spawn N isolated git
worktrees and has nothing that records what each lane claimed, what it produced, whether it merged,
or whether another lane superseded it — and no verb to clean the worktrees up. The user is left
finding orphan branches by hand.

This is the coordination layer for **graph patterns of inter-agent communication**: it is the
feature, not a parity exercise.

## The governing directive (from the user, this batch)

> "we ship features ... adapted to Rust best practices ... pi names can change ... it's the features
> that matter"

So:

- **Port the BEHAVIOUR, not the spelling.** Upstream's names, file layout and function signatures
  are evidence of intent, not a specification to mirror. Where Rust has a better shape, take it.
- **Use Rust's type system where TypeScript could not.** Concretely, upstream's string unions
  (`WorktreeCleanupPlanState = "safe" | "ineligible" | "stale" | "dirty" | "active" | "unknown"`,
  `WorktreeCleanupPlanDecision`, `WorktreeCleanupPlanSource`, `ForegroundRunOwnership`,
  `WorkflowLaneMode = "mutation" | "review" | "scout" | "gate"`) are **enums**. Upstream's
  hand-rolled `boundedNonEmptyString` / `assertKnownFields` validation is a **newtype with a
  fallible constructor** plus `#[serde(deny_unknown_fields)]`. Upstream's `throw new Error(string)`
  is a **typed error enum**. Do not port a stringly-typed shape into Rust just because upstream has
  one.
- **Keep the on-disk format compatible** — the manifest and plan files are the interface, and
  cyrup's own retention reader already parses one (see below). Field names in the JSON stay
  upstream's; Rust identifiers do not have to.
- Naming in the tool surface is cyrup's call. Match the repo's existing conventions.

## Why this is landable: four seams already wait for it

1. **Worktree isolation WORKS.** `spawn/worktree.rs` is a faithful port of `runs/shared/worktree.ts`,
   and the tool schema advertises the flag — `extension/tool/schema.rs:430`, *"Create isolated git
   worktrees for parallel tasks; requires clean git state."*
2. **A complete READER already exists** with no writer: `background/async_retention/scan.rs:423`
   (`has_unresolved_run_handoff`) parses `<run_dir>/handoff.json`, and its own `[CYRUP-DELTA]` says
   *"no cyrup writer produces the file today"*. It also records that `RunStatus` has no
   `parallel_handoff` field, so upstream's first path source is unreadable here. **Both halves are
   this task's to close.**
3. **The authority gate is waiting by name:** `registration/authority.rs:20` — *"Whoever lands
   `worktree.discard` or `destructiveCleanup` must wire them through"*.
4. **The destructive-action gate already carries the verb:** `extension/tool/text.rs:323` lists
   `worktree.discard` in `DESTRUCTIVE_MANAGEMENT_ACTIONS`, ported verbatim ahead of the dispatch.

## Scope: FIVE verbs, one substrate

`worktree.discard`, `worktree.cleanup`, `lane.status`, `lane.recordMerge`, `lane.recordSupersession`
— 5 of the 17 verbs in cyrup's action-list gap. **The ledger files them as two unrelated items;
they are one feature.** Verified at `runs/foreground/subagent-executor.ts:6270-6290` @v0.68.0: all
five dispatch through `readParallelHandoffManifest` / `recordParallelHandoffMerge` /
`recordParallelHandoffSupersession`.

Counted directly, not from the ledger: upstream `SUBAGENT_ACTIONS` (`src/shared/types.ts:2801`) is
**57**; cyrup's (`extension/tool/text.rs:215`) is **42**; 17 missing, 2 cyrup-only
(`append-step`, `inspect`).

## Upstream, pinned at v0.68.0

Read **only** via `git -C /home/user/cyrup/tmp/pi-subagents show v0.68.0:<path>`. Never a working
tree, never unpinned HEAD.

- `src/runs/shared/parallel-handoff.ts` (741 lines) — the manifest. Key entry points:
  `readParallelHandoffManifest:122`, `resolveParallelHandoffChild:128`,
  `resolveRetainedWorktreeCwd:162`, `isTerminalParallelHandoffChildStatus:295`,
  `formatStoredParallelHandoffCleanup:411`, `recordParallelHandoffMerge:437`,
  `recordParallelHandoffSupersession:455`, `writeParallelHandoffGroup:497` (**the missing writer**),
  `parallelHandoffPath:614`, `writeWorktreeSetupHandoff:619`, `discardPreservedWorktrees:684`.
- `src/runs/shared/worktree-cleanup-plan.ts` (869 lines) — the **two-phase, plan-then-execute**
  cleanup. `buildWorktreeCleanupPlan:742`, `createWorktreeCleanupPlan:825`,
  `worktreeCleanupPlanPath:821`, `formatWorktreeCleanupPlan:837`, `parseGitWorktreeList:269`.
  `WORKTREE_CLEANUP_PLAN_TTL_MS:21` is 30 minutes.
  **This design is safety-critical and must survive the port**: a plan is built by cross-checking
  git against the manifest metadata (`WorktreeCleanupPlanSource = git | metadata | both`), each
  entry gets a state and a decision, and only then can `cleanup` execute it. A worktree with
  uncommitted work must never be removed.
- `src/runs/shared/lane-metadata.ts` (126 lines) — launch-declared lane metadata:
  `key`, `mode` (`mutation`/`review`/`scout`/`gate`), `sourceRef`, `claims`, `outputPaths`, with
  bounds at `:3-11` (`KEY_MAX_BYTES` 128, `SOURCE_REF_MAX_BYTES` 128, `CLAIM_MAX_BYTES` 160,
  `CLAIMS_MAX` 20, `OUTPUT_PATH_MAX_BYTES` 256, `OUTPUT_PATHS_MAX` 10) and a key pattern at `:13`.
- `src/extension/schemas.ts` — the params cyrup does not advertise:
  `handoffPath:298`, `laneId:301`, `merge:302`, `supersession:303`, `lane:355`
  (`WorkflowLaneMetadata`), and `lanes:147` (display-only preflight hints, max 64).
  **`grep '"handoffPath"\|"lane"\|"laneId"' extension/tool/schema.rs` returns 0 today.**
- Dispatch: `src/runs/foreground/subagent-executor.ts:6270-6290`. Note `lane.status` is READ-ONLY
  and the other two are gated by `allowMutatingManagementActions` — preserve that split.

## Definition of done

A human or an agent can fan out parallel work into worktrees, then:

1. **see the graph** — which lanes exist, their mode, what each claimed, what it produced, and which
   are still unresolved;
2. **record convergence** — a lane merged, or was superseded by another, with evidence;
3. **clean up safely** — a plan that never proposes removing a worktree with uncommitted work, and a
   discard path for preserved worktrees.

All five verbs reachable from the production tool surface AND, where it makes sense, from the RPC
bridge's `manage` method (`extension/rpc/`, landed in PR #142) — the bridge already routes
`schedule.*`, so a delegating host should reach lanes the same way. Decide this deliberately and say
why either way.

`has_unresolved_run_handoff` (`async_retention/scan.rs:423`) must find real files, and its
`[CYRUP-DELTA]` about there being no writer must be deleted because it has stopped being true.
Close the `RunStatus::parallel_handoff` half too, or state precisely why not.

**No `allow(dead_code)`. No landing a writer with no caller. A test must drive the production path.**
This programme has shipped tested-but-unreachable machinery five times; PR #142 was the first batch
that did not.

## Rules

- Every deliberate divergence gets a `[CYRUP-DELTA]` whose stated reason is **true**. Two defects in
  the last batch came from a delta justifying itself on a false premise — verify the premise before
  you write it.
- Workspace stays `cargo fmt` clean and clippy clean under every README gate, including
  `cargo clippy -p cyrup-it --features it --all-targets` and the wasm32-wasip2 one.
- Baseline: **10 474** workspace tests passing, 9 skipped; `cyrup-it --features it` **551** passing.
  Do not regress either.

## [AUG — manifest]

Area: **the handoff manifest and the lane graph** — upstream `src/runs/shared/parallel-handoff.ts`
(741 lines @v0.68.0, read in full) plus `src/runs/shared/lane-metadata.ts` (126 lines) for the lane
shape the manifest embeds. Research only; no source file touched.

### 0. Anchor re-verification (seed spec)

| Seed anchor | Verdict |
|---|---|
| `parallel-handoff.ts` 741 lines, `readParallelHandoffManifest:122`, `resolveParallelHandoffChild:128`, `resolveRetainedWorktreeCwd:162`, `isTerminalParallelHandoffChildStatus:295`, `formatStoredParallelHandoffCleanup:411`, `recordParallelHandoffMerge:437`, `recordParallelHandoffSupersession:455`, `writeParallelHandoffGroup:497`, `parallelHandoffPath:614`, `writeWorktreeSetupHandoff:619`, `discardPreservedWorktrees:684` | **all exact** |
| `lane-metadata.ts` 126 lines, bounds `:3-11`, key pattern `:13` | **exact** |
| `subagent-executor.ts:6270-6290` dispatch for the five verbs | **exact** (`:6242` `worktree.discard`, `:6270` the `lane.*` arm) |
| `shared/types.ts:2801` `SUBAGENT_ACTIONS` = 57 | **exact** |
| `extension/schemas.ts` `handoffPath:298`, `laneId:301`, `merge:302`, `supersession:303`, `lane:355`, `lanes:147` | **all exact** |
| cyrup `extension/tool/schema.rs:430` worktree flag | **exact** |
| cyrup `extension/tool/text.rs:323` `"worktree.discard"` in `DESTRUCTIVE_MANAGEMENT_ACTIONS` | **exact** |
| cyrup `registration/authority.rs:20` "Whoever lands `worktree.discard` or `destructiveCleanup` must wire them through" | **exact** (`:20-22`) |
| cyrup `background/async_retention/scan.rs:423` `has_unresolved_run_handoff` | **SLIGHTLY STALE**: `:423` is the first line of its doc comment; the `[CYRUP-DELTA]` is `:425-429`; the `async fn` itself is **`:430`**. The path constant is `HANDOFF_MANIFEST_FILE = "handoff.json"` at **`:53`**, and the single production call is **`:282`**. |
| `grep '"handoffPath"\|"lane"\|"laneId"' extension/tool/schema.rs` returns 0 | **verified 0** (also 0 in `extension/tool/params.rs`) |

### 1. What cyrup ALREADY has (grepped hard; this is the reuse list)

1. **The reader.** `crates/cyrup-ext-subagents/src/background/async_retention/scan.rs:430`
   `has_unresolved_run_handoff(run_dir)`. Reads `<run_dir>/handoff.json`, requires
   `version == 1` (u64) and a **non-empty** `groups` array, and returns `true` if **any** group's
   `cleanup.state != "complete"`. Unreadable/absent-object → `true` (keep). Called once, from
   `scan.rs:282`, feeding `RunScanCandidate`. **This pins the on-disk shape.** Note the reader's
   emptiness rule is *stricter* than pi's `hasValidStoredLaneShape`: an empty `groups` is
   "unresolved", so a writer must never publish `groups: []`.
2. **`stable_json_digest`** — `crates/cyrup-ext-subagents/src/workflows/stable_json.rs:54`, an exact
   port of pi `stableJsonDigest` (`launch-contract.ts:21-23`), the function
   `manifestFactsDigest` (`parallel-handoff.ts:232-241`) needs. Already carries its own
   `[CYRUP-DELTA]` about byte-order vs `localeCompare` key sorting — **that delta already covers
   the manifest digest**; do not write a second one, and do not "fix" it (cyrup is self-consistent;
   mixed-case keys do not occur in `manifestFacts`, whose keys are `version/runId/mode/source/cwd/groups`,
   all lowercase-initial, so pi and cyrup digests actually AGREE for this payload — verified by
   inspection of the six key names).
3. **`WorkflowKey`** — `crates/cyrup-ext-subagents/src/workflows/key.rs:18`, `parse()` at `:30`
   implements *exactly* `WORKFLOW_LANE_KEY_PATTERN` (`lane-metadata.ts:13`:
   `^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$`) and deserializes THROUGH `parse`. **`WorkflowLaneMetadata::key`
   is a `WorkflowKey`. Do not write a second newtype.** It also already has
   `is_descendant_of` for lane-root relations.
4. **Atomic JSON writes** — `background/atomic.rs:75` `write_atomic_json`, `:114`
   `write_atomic_json_creating_parent` (the one to use: pi's `writeAtomicJson` mkdir -p's,
   `shared/atomic-json.ts:56-58`, and `<artifactsDir>/handoffs/` will not exist).
5. **`RunDir`** — `background/run_paths.rs:27`, with `status()` `:64` and `events()` `:74`. The
   manifest path belongs here as a third accessor (see §3.4).
6. **Worktree isolation** — `spawn/worktree.rs`: `WorktreeSetup:63`, `WorktreeInfo:74`,
   `WorktreeDiff:94`, `create_worktrees:833`, `capture_worktree_diff:958`, `diff_worktrees:1010`,
   `cleanup_worktrees:1045`, `format_worktree_diff_summary:1072`.
7. **`crate::time::now_epoch_millis()`** (`time.rs:18`) → `i64` — the `createdAt`/`updatedAt` clock.
8. **`SubagentError`** (`error.rs:16`, thiserror) with `WorktreeSetup(String)` at `:47`.
9. `#[serde(deny_unknown_fields, rename_all = "camelCase")]` is already the crate idiom —
   `workflows/child_summary.rs:420,441`, `workflows/types.rs:559`.

**What cyrup does NOT have (proven by grep):**
- No `WorkflowLaneMetadata`, no lane mode, no lane key/claims/outputPaths. The 908 `lane` hits are
  all `exec/tool_surface.rs`'s *review/scout agent lane* vocabulary — a different concept entirely.
- No manifest type, no writer, no `ParallelHandoff*` anything.
- `diff_worktrees`, `capture_worktree_diff` and `cleanup_worktrees` have **zero non-test callers**
  outside `spawn/worktree.rs` itself. cyrup creates worktrees and then never diffs, never cleans up,
  never records. That is the whole hole.
- No `WorktreeCleanupReport` / `WorktreeCleanupTask` / `WorktreeCleanupIntent`. `cleanup_worktrees`
  (`worktree.rs:1045`) takes `&WorktreeSetup` only, reports nothing, and has no `preserve`/`discard`/
  `setup-rollback` intent. **The manifest's `group.cleanup` object cannot be populated until that
  report type exists — this is my hard dependency on the cleanup sibling.**
- `RunStatus` (`background/records.rs:210`) has **no** `parallel_handoff` field. Confirmed by
  reading the full field list `:211-350`.

### 2. The load-bearing upstream blocks (quoted, with line numbers)

**Manifest validation — `:70-120`.** `validateManifestIdentity` enforces, in order: `version === 1`
and `Array.isArray(groups)` (`:71`); non-empty trimmed `runId` (`:74`); per group, `children` is an
array, `cleanup` present, `cleanup.tasks` an array (`:80`); `laneBindings` (if present) is an array
whose every `taskIndex` is **unique within the group** and **has a matching `cleanup.tasks[].index`**
(`:85-91`); per child, non-negative-safe-integer `index`/`taskIndex` (`:97-98`), `taskIndex` unique
**within the group** (`:99`), `index` unique **across the whole manifest** (`:101`), a matching
cleanup task for the child's `taskIndex` (`:103`), and `workflowKey`/`runId` unique **across the
whole manifest** (`:108-111`), with `lane.key === workflowKey` when both are present
(`assertWorkflowLaneKey`, `lane-metadata.ts:71-74`).

**Cleanup eligibility — `:359-385` (`cleanupEligibilityForEvidence`), the safety core.**
```
359  if (hasActiveChildren(manifest)) return { state: "active" };
366     catch → { state: "terminal-blocked", reason: "stored merge evidence is invalid" }
372     supersession.manifestDigest !== manifestFactsDigest → "stored supersession evidence is stale"
376     → { state: "superseded-eligible" }
378  if (!merge) → "no merge or supersession evidence recorded"
379  merge.manifestDigest !== manifestFactsDigest → "stored merge evidence is stale"
380-382 merge.treeEquivalent !== true → "merged tree equivalence was not attested"
                                      / "merged tree is not equivalent to the reviewed head"
383  merge.postMergeChecks !== "recorded" → "required post-merge checks were not recorded"
384  → { state: "terminal-eligible" }
```
Note the **supersession-beats-merge precedence** (`:369` runs before `:378`) and that
`recordParallelHandoffMerge` **deletes** any stored supersession (`:449`) — merge evidence supersedes
supersession, not the other way round.

**Trust laundering — `:313-324` (`trustedStoredCleanupEligibility`).** A *stored* eligibility string
is never believed on its own: the manifest must first pass `hasValidStoredLaneShape` (`:299-311`),
active children force `"active"` (`:316`), a stored `"active"` is **re-derived** from evidence
(`:317`), and a stored `terminal-eligible`/`superseded-eligible` is only honoured if re-derivation
agrees or downgrades to a *stale* blocker (`:318-322`); anything else collapses to `"unknown"`.
**This is the anti-forgery rule: an attacker who edits `cleanupEligibility` to `terminal-eligible`
gets `unknown`, which formats as "removal is not safe" (`:424`).**

**Active-children rule — `:326-333`.** A group with **zero children but ≥1 cleanup task** is ACTIVE
(`:330`) — an allocated-but-unsettled worktree blocks removal. Any child whose status is not terminal
is also active (`:331`).

**Terminal statuses — `:295-297`.** `completed | complete | failed | paused | stopped | rejected`.
`pending`, `running` and **`detached`** are NOT terminal, though all nine are accepted on read
(`STORED_CHILD_STATUSES`, `:293`).

**The writer — `:497-612`.** Merge-by-`stepIndex`: existing groups are filtered to drop the same
`stepIndex` (`:587`), the new group is pushed and the list re-sorted by `stepIndex` (`:588-589`).
Identity is pinned: an existing manifest with a different `runId`/`mode`/`source` is a hard error
(`:514-516`). `laneBindings` are only persisted when the group has **no** child results yet
(`:570`) — they are launch-time identity that child rows replace. `merge`/`supersession` survive a
rewrite (`:599-600`), and `cleanupEligibility` is recomputed **only if the existing manifest already
carried cleanup metadata** (`:602-608`) — a fresh manifest deliberately has **no** `cleanupEligibility`
key at all.

**Path — `:614-616`.** `runId ? <base>/handoffs/<runId>.json : <base>/handoff.json`.

**Discard — `:684-741`.** Only tasks with `preserved && (!worktreeRemoved || !branchRemoved)` are
touched (`:693`); a task whose report row does not show BOTH removals is forced back to
`preserved: true` with a reason (`:710-712`); `cleanup.state` becomes `"complete"` only if every task
is doubly-removed **and** `report.pruned` (`:714-717`); remaining worktrees are printed with manual
`git status --short` / `worktree remove --force` / `branch -D` commands (`:730-738`).

### 3. The Rust design

New module **`crates/cyrup-ext-subagents/src/handoff/`**, registered in `lib.rs` between `fork_context`
and `identity`. It sits beside `spawn/` and `background/` rather than inside either, because the
foreground executor, the background runner and `background::async_retention` all touch it — putting
it under `spawn/` would make `async_retention`'s reader import from `spawn`, and putting it under
`background/` would make the foreground `/parallel` path import from `background`.

```
handoff/
  mod.rs        re-exports; module docs naming pi parallel-handoff.ts @v0.68.0
  lane.rs       LaneMetadata, LaneMode, LaneClaim, LaneOutputPath, LaneSourceRef   (lane-metadata.ts)
  model.rs      Manifest, Group, Child, LaneBinding, Patch, CleanupTask, CleanupReport-ref
  evidence.rs   MergeEvidence, SupersessionEvidence, CleanupEligibility, CommitSha, ManifestDigest,
                AttestationActor, AttestationTimestamp, LaneId
  read.rs       read_manifest / resolve_child / resolve_retained_worktree_cwd
  write.rs      write_group / write_setup_handoff / record_merge / record_supersession
  path.rs       manifest_path helpers (or fold into RunDir, see §3.4)
  error.rs      HandoffError
  format.rs     format_stored_cleanup / format_reference / format_error
```

#### 3.1 Enums (upstream string unions → Rust)

```rust
/// pi `ParallelHandoffManifest["mode"]` (`shared/types.ts:509-511`).
/// [CYRUP-DELTA] deliberately NOT `crate::background::RunMode`: that enum has a fourth variant,
/// `Workflow` (`background/state.rs:37`), which upstream's manifest union does not admit.
/// Reusing it would let `"mode":"workflow"` into a file pi refuses to parse.
#[derive(…, Serialize, Deserialize)] #[serde(rename_all = "camelCase")]
pub enum HandoffMode { Single, Parallel, Chain }

/// pi `ParallelHandoffManifest["source"]`.
#[serde(rename_all = "camelCase")] pub enum HandoffSource { Foreground, Async }

/// pi `SubagentResultStatus` (`types.ts:400`) widened to `STORED_CHILD_STATUSES`
/// (`parallel-handoff.ts:293`) on the READ side.
#[serde(rename_all = "camelCase")]
pub enum ChildStatus {
    Pending, Running, Completed, Complete, Failed, Paused, Stopped, Detached, Rejected,
}
impl ChildStatus {
    /// pi `isTerminalParallelHandoffChildStatus` (`:295-297`). NOTE: `Detached` is NOT terminal.
    pub fn is_terminal(self) -> bool { matches!(self,
        Self::Completed|Self::Complete|Self::Failed|Self::Paused|Self::Stopped|Self::Rejected) }
}
```
`Complete` and `Completed` are both kept because pi's reader accepts both and cyrup's own
`StepState` serializes `"complete"` (`background/state.rs:236`) while pi's writer emits `"completed"`.
The cyrup **writer emits `Completed`** (upstream's write-side value) so a pi reader of a cyrup file
sees the value pi itself writes; the reader accepts both. `[CYRUP-DELTA]` not needed — this is
upstream's own read/write asymmetry, ported.

```rust
/// pi `CleanupEligibility` (`types.ts:469-474`) — a tagged union, so a Rust enum with the reason
/// living ONLY on the variant that has one. Upstream must test `state` before touching `reason`;
/// here it is unrepresentable to have one without the other.
#[derive(…, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CleanupEligibility {
    Active,
    TerminalEligible,
    TerminalBlocked { reason: BlockReason },
    SupersededEligible,
    Unknown,
}
```
`kebab-case` gives `terminal-eligible` / `terminal-blocked` / `superseded-eligible`; `active` and
`unknown` are unaffected. **Verify this in a round-trip test against a literal JSON string** —
the on-disk spelling is the interface.

`BlockReason`: pi produces exactly seven blocker strings (`:366,372,374,378,379,381,383`) and
`trustedStoredCleanupEligibility:320` **regex-matches one of them** (`/ evidence is stale$/`). A
free `String` would make that a stringly test in Rust too. Model it as an enum with a `Display`
that renders pi's exact text, plus an `Other(BoundedReason)` arm for a reason read from an
already-stored manifest:
```rust
pub enum BlockReason {
    StoredMergeEvidenceInvalid,        // "stored merge evidence is invalid"
    StoredMergeEvidenceStale,          // "stored merge evidence is stale"
    StoredSupersessionEvidenceInvalid, // "stored supersession evidence is invalid"
    StoredSupersessionEvidenceStale,   // "stored supersession evidence is stale"
    NoEvidenceRecorded,                // "no merge or supersession evidence recorded"
    TreeEquivalenceNotAttested,        // "merged tree equivalence was not attested"
    TreeNotEquivalent,                 // "merged tree is not equivalent to the reviewed head"
    PostMergeChecksNotRecorded,        // "required post-merge checks were not recorded"
    Other(BoundedReason),              // a reason string read back from disk (≤256 bytes, no newlines)
}
impl BlockReason { pub fn is_stale(self) -> bool { matches!(self, Self::StoredMergeEvidenceStale
    | Self::StoredSupersessionEvidenceStale) } }   // replaces pi's `/ evidence is stale$/` regex
```
That `is_stale` predicate is the whole point: `:320`'s regex becomes a match arm, and the delta
comment should say so.

```rust
/// pi `merge.treeEquivalent: boolean | "unknown"` (`types.ts:480`) — a three-state, so a
/// three-variant enum, not `Option<bool>` (which would let `null` in).
#[serde(rename_all = "camelCase", untagged)] // see note
pub enum TreeEquivalence { Yes, No, Unknown }
```
The wire values are `true`, `false`, `"unknown"` — a mixed bool/string union. Implement
`Serialize`/`Deserialize` **by hand** (the crate's established idiom, `workflows/key.rs:73-78`
names it): serialize `Yes→Value::Bool(true)`, `No→Bool(false)`, `Unknown→String("unknown")`;
deserialize the mirror and reject everything else. Do not reach for `#[serde(untagged)]` on a
fieldless enum — it cannot express bool-or-string.

```rust
/// pi `merge.postMergeChecks: "recorded" | "unknown"`.
#[serde(rename_all = "camelCase")] pub enum PostMergeChecks { Recorded, Unknown }

/// pi `WorkflowLaneMode` (`types.ts:167`).
#[serde(rename_all = "camelCase")] pub enum LaneMode { Mutation, Review, Scout, Gate }

/// pi group `cleanup.state: "complete" | "partial"` (`types.ts:502`).
#[serde(rename_all = "camelCase")] pub enum CleanupState { Complete, Partial }
```

#### 3.2 Newtypes (hand-rolled `boundedString` → fallible constructors)

Every one gets `parse(&str) -> Result<Self, HandoffError>`, `as_str()`, `Display`, and a
**hand-written `Deserialize` that goes through `parse`** (idiom pinned at `workflows/key.rs:73-78`:
"NOT a derive, and NOT a per-field serde attribute … a new deserialized field cannot forget an
attribute that does not exist").

| Newtype | Rule | Upstream |
|---|---|---|
| `CommitSha` | exactly 40 lowercase hex; input lowercased then matched | `commitString:220-224` (`COMMIT_PATTERN:206`) |
| `ManifestDigest` | exactly 64 lowercase hex | `manifestDigestString:226-230` (`:207`) |
| `AttestationActor` | trimmed, non-empty, no `\r`/`\n`, ≤128 UTF-8 bytes | `boundedString:212-218` + `MAX_ATTESTATION_ACTOR_LENGTH:208` |
| `AttestationTimestamp` | trimmed, non-empty, no newline, ≤64 bytes, **and parses as a date** | `attestationTimestamp:243-247` |
| `LaneId` | trimmed, non-empty, no newline, ≤128 bytes | `MAX_SUPERSESSION_ID_LENGTH:210`, used for both `laneId` and `supersession.supersededBy` |
| `BoundedReason` | trimmed, non-empty, no newline, **truncated to 256** | `:288` |
| `LaneSourceRef` | ≤128 bytes, non-empty, no `\n`/`\r`/NUL | `lane-metadata.ts:4,20-26` |
| `LaneClaim` | ≤160 bytes, same charset rule; `Vec<LaneClaim>` capped at 20 | `lane-metadata.ts:5,6` |
| `LaneOutputPath` | ≤256 bytes; `Vec` capped at 10 | `lane-metadata.ts:7,8` |
| lane `key` | **reuse `crate::workflows::WorkflowKey`** | `lane-metadata.ts:13` ≡ `workflows/key.rs:30-45` |

`AttestationTimestamp`: pi uses `Date.parse` (accepts ISO-8601 and a pile of legacy formats). cyrup
should accept **RFC 3339** via `time`/`chrono` (check which the workspace already depends on;
`registration/cost.rs` and `background/` both timestamp) and carry a `[CYRUP-DELTA]`:
*narrower than `Date.parse` by design — `Date.parse("not a date at all")` is NaN but
`Date.parse("Dec 2 1999")` is not, and accepting locale-ish formats into an attestation record
makes two readers disagree about when it happened.* A cyrup-written timestamp is always RFC 3339,
so cyrup files round-trip; a pi file with a legacy timestamp is refused, which is a **behavioural**
divergence and therefore earns the delta.

Numeric bounds: pi's `nonNegativeIndex` (`:45-48`) is "safe integer ≥ 0" → **`u32`** for `index`/
`taskIndex` (matching `WorktreeInfo::index: u32`, `worktree.rs:83`), and `prNumber` is "integer ≥ 1"
→ a `PrNumber(NonZeroU32)` newtype. Both make pi's runtime check a parse-time one.

#### 3.3 The records

```rust
#[derive(Serialize, Deserialize)] #[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub version: ManifestVersion,          // a ZST-ish newtype that serializes 1 and refuses other
    pub run_id: LaneId,                    // pi trims + requires non-empty (`:74-75`)
    pub mode: HandoffMode,
    pub source: HandoffSource,
    pub cwd: PathBuf,
    pub created_at: i64,
    pub updated_at: i64,
    pub groups: Vec<Group>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub merge: Option<MergeEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub supersession: Option<SupersessionEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub cleanup_eligibility: Option<CleanupEligibility>,
}
```
`cleanup_eligibility` stays `Option` **because its ABSENCE is meaningful** — `writeParallelHandoffGroup:602-608`
omits the key entirely on a fresh manifest, and `referenceFor:200` uses `hasOwnProperty` to decide
whether to project it. `merge`/`supersession` are genuinely optional. Everything else is required.

```rust
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Group {
    pub step_index: u32,
    pub base_commit: String,               // pi only requires non-empty (`:303`); NOT a CommitSha —
                                           // `WorktreeSetup::base_commit` is `git rev-parse HEAD`
                                           // output and pi never length-checks it here.
    pub repo_root: PathBuf,
    pub children: Vec<Child>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub lane_bindings: Option<Vec<LaneBinding>>,
    pub cleanup: GroupCleanup,
}

#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupCleanup {                  // pi's inline `group.cleanup` (`types.ts:501-506`)
    pub state: CleanupState,
    pub tasks: Vec<CleanupTask>,
    pub pruned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub errors: Option<Vec<String>>,
}

#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Child {
    pub index: u32, pub task_index: u32, pub agent: String,
    #[serde(default, skip_serializing_if="Option::is_none")] pub workflow_key: Option<WorkflowKey>,
    #[serde(default, skip_serializing_if="Option::is_none")] pub run_id: Option<LaneId>,
    #[serde(default, skip_serializing_if="Option::is_none")] pub lane: Option<LaneMetadata>,
    pub status: ChildStatus,
    pub summary: String,
    #[serde(default, skip_serializing_if="Option::is_none")] pub output_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if="Option::is_none")] pub structured_output: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if="Option::is_none")] pub structured_output_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if="Option::is_none")] pub session_path: Option<PathBuf>,
    pub patch: Patch,
}
```
Careful: `structured_output` is `unknown` upstream and `undefined` is dropped while `null` is kept
(`:555` tests `!== undefined`). `Option<Value>` + `skip_serializing_if = "Option::is_none"` gives
exactly that — `Some(Value::Null)` still serializes `null`.

`CleanupTask` mirrors `types.ts:454-467`: `index: u32`, `path: PathBuf`, `branch: String`,
optional `provider: WorktreeProvider` (`native|worktrunk`) and `naming: WorktreeNaming`,
`worktree_removed: bool`, `branch_removed: bool`, optional `preserved: bool`, optional
`reason: String`, optional `errors: Vec<String>`. **This type is owned by the cleanup sibling**
(it is `WorktreeCleanupTask` in `worktree.ts:82-93`); I depend on it and should not define it twice.
Recommendation for the executor: **define it once in `handoff::model` and have `spawn::worktree`'s
cleanup report use it**, because it is the thing that is *serialized*, and the serialization rules
(camelCase, `deny_unknown_fields`, optional-elision) belong with the on-disk model.

`LaneMetadata` (`lane-metadata.ts:49-69`):
```rust
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LaneMetadata {
    pub version: LaneVersion,                                   // literal 1
    pub key: WorkflowKey,
    #[serde(default, skip_serializing_if="Option::is_none")] pub mode: Option<LaneMode>,
    #[serde(default, skip_serializing_if="Option::is_none")] pub source_ref: Option<LaneSourceRef>,
    #[serde(default, skip_serializing_if="Option::is_none")] pub claims: Option<Claims>,        // ≤20
    #[serde(default, skip_serializing_if="Option::is_none")] pub output_paths: Option<OutputPaths>, // ≤10
}
```
`Claims`/`OutputPaths` are bounded-vec newtypes so the 20/10 caps are parse-time, not a runtime
`if length >`. `assertWorkflowLaneKey` (`lane-metadata.ts:71-74`) is **not** a newtype — it is a
cross-field relation, so it belongs in the validating constructor of the *containing* record
(`Child::new`, `LaneBinding::new`), which is where `deny_unknown_fields` cannot reach.

#### 3.4 Error enum

```rust
#[derive(thiserror::Error, Debug)]
pub enum HandoffError {
    #[error("{label} must be a non-empty string.")]                      Empty { label: &'static str },
    #[error("{label} must be at most {max} bytes.")]                     TooLong { label: &'static str, max: usize },
    #[error("{label} must not contain newlines.")]                       Newline { label: &'static str },
    #[error("{label} must be a full 40-character commit SHA.")]          NotACommit { label: &'static str },
    #[error("{label} must be a full 64-character SHA-256 digest.")]      NotADigest { label: &'static str },
    #[error("attestedAt must be a valid timestamp.")]                    BadTimestamp,
    #[error("merge.prNumber must be a positive integer.")]               BadPrNumber,
    #[error("{label} has unsupported fields: {fields}.")]                UnknownFields { .. },
    #[error("Invalid parallel handoff manifest '{path}': {detail}")]     InvalidManifest { path: PathBuf, detail: ManifestDefect },
    #[error("Parallel handoff manifest not found: {0}")]                 NotFound(PathBuf),
    #[error("Managed worktree handoff belongs to run '{found}', not '{expected}'.")] RunMismatch { .. },
    #[error("Lane '{lane}' does not match manifest run '{run}'.")]       LaneRunMismatch { .. },
    #[error("Lane has an active child owner; {what} must be recorded after local reconciliation.")] ActiveChildOwner { what: &'static str },
    #[error("Lane manifest is stale: reviewed head is already recorded as {existing}.")] StaleReviewedHead { existing: CommitSha },
    #[error("Lane manifest already contains different merge evidence for this reviewed head.")] ConflictingMerge,
    #[error("Lane manifest already contains different supersession evidence.")] ConflictingSupersession,
    #[error("supersession.supersededBy must identify a different replacement lane.")] SelfSupersession,
    #[error("Parallel handoff manifest belongs to a different run: {0}")] ForeignManifest(PathBuf),
    #[error(transparent)] Io(#[from] std::io::Error),
    #[error("{0}")] Json(#[source] serde_json::Error),
}
```
Every message is pi's verbatim text, so the crate's existing parity-of-wording convention
(`error.rs:80-88` `Management(String)` records it) holds without a `String` variant. Bridge into
`SubagentError` with `#[from]` on a new `SubagentError::Handoff(#[from] HandoffError)` variant
rather than stringifying.

`ManifestDefect` is a sub-enum carrying pi's per-defect wording
(`groups[{i}] is malformed`, `duplicate child index {n}`, `duplicate workflow key '{k}'`, …) so the
`InvalidManifest` message reproduces `:80-111` exactly.

#### 3.5 Path resolution

Add to `RunDir` (`background/run_paths.rs:29`), beside `status()`/`events()`:
```rust
/// `<run_dir>/handoff.json` — pi `parallelHandoffPath(asyncDir)` (`parallel-handoff.ts:614-616`).
/// The SAME literal `background::async_retention::scan`'s `HANDOFF_MANIFEST_FILE` (`scan.rs:53`)
/// reads; move that constant here so writer and reader cannot drift.
pub fn handoff(&self) -> PathBuf
```
and a free function in `handoff::path` for the foreground shape:
```rust
/// pi `parallelHandoffPath(baseDir, runId)` (`:614`) — `<base>/handoffs/<run_id>.json`.
pub fn handoff_manifest_path(base_dir: &Path, run_id: &RunId) -> PathBuf
```
The reader's `HANDOFF_MANIFEST_FILE` constant at `scan.rs:53` should be **deleted and replaced by
`RunDir::handoff()`** — that is one of the two "delete the delta" moves this item owes.

#### 3.6 Function surface (behaviour, not spelling)

| pi | Rust |
|---|---|
| `readParallelHandoffManifest:122` | `read_manifest(&Path) -> Result<Option<Manifest>, HandoffError>` (async, `tokio::fs`) |
| `resolveParallelHandoffChild:128` | `resolve_child(&Path, &RunId, ChildSelector) -> Result<Option<(&Group,&Child)>, _>` where `ChildSelector` is an **enum** (`ByWorkflowKey`, `ByRunId`, `Both`) — pi's "requires workflowKey or childRunId" throw (`:139`) becomes unrepresentable |
| `resolveRetainedWorktreeCwd:162` | `resolve_retained_worktree_cwd(&Path, &RunId, u32) -> Result<Option<PathBuf>, _>` |
| `isTerminalParallelHandoffChildStatus:295` | `ChildStatus::is_terminal` |
| `trustedStoredCleanupEligibility:313` | `Manifest::trusted_cleanup_eligibility(&self) -> CleanupEligibility` |
| `formatStoredParallelHandoffCleanup:411` | `format_stored_cleanup(&Path, Option<&Manifest>) -> String` |
| `recordParallelHandoffMerge:437` | `record_merge(RecordMerge<'_>) -> Result<EvidenceOutcome, _>` |
| `recordParallelHandoffSupersession:455` | `record_supersession(RecordSupersession<'_>) -> Result<EvidenceOutcome, _>` |
| `writeParallelHandoffGroup:497` | `write_group(WriteGroup<'_>) -> Result<HandoffReference, _>` |
| `writeWorktreeSetupHandoff:619` | `write_setup_handoff(WriteSetupHandoff<'_>) -> Result<Option<HandoffReference>, _>` |
| `discardPreservedWorktrees:684` | `discard_preserved(&Path, DiscardAuthorization) -> Result<DiscardOutcome, _>` — **cleanup sibling's call, my writer** |

The `input: {...}` object literals become **named parameter structs** (`WriteGroup<'_>` etc.), which
is both the Rust idiom and what keeps `write_setup_handoff`'s `Omit<Parameters<…>[0], …>` trick
(`:619`) expressible — in Rust it is a struct that *contains* the shared half rather than a mapped
type.

`resolve_retained_worktree_cwd`'s containment check (`:157-160`, `pathInside`) must NOT be
reimplemented: `spawn/worktree.rs:234` already has `lexical_normalize` and `:254`
`normalize_comparable_cwd`. Reuse or lift them; a second path-escape predicate in this crate is a
security bug waiting to happen.

### 4. PRODUCTION call sites — the reachability spine

**This is the part the last five failures got wrong. There is exactly ONE place in cyrup where a
worktree fan-out exists today, and the writer must land inside it.**

1. **`crates/cyrup-ext-subagents/src/spawn/chain_graph.rs:1655`** — inside `run_parallel_group`
   (`:1635`), `if spec.worktree { assign_worktree_cwds(&mut resolved_steps, ctx).await?; }`.
   This is the *only* non-test caller of any worktree function in the crate (verified:
   `grep -rn "create_worktrees\|cleanup_worktrees\|diff_worktrees\|setup_worktree_group" crates/
   --include=*.rs` outside `spawn/worktree.rs` yields only `chain_graph.rs:2085` and doc comments).
   The `dispatch_group(...)` call at `:1669` is where the group *completes*. **The manifest write
   goes immediately after it**, mirroring pi `subagent-runner.ts:4389-4434`.
2. **`crates/cyrup-ext-subagents/src/spawn/chain_graph.rs:2085`** — `assign_worktree_cwds` calls
   `setup_worktree_group`, which **throws away the `WorktreeSetup`** (`worktree.rs:1170-1207`
   returns only `WorktreeGroupPlan { assignments, base_commit }`). The full `WorktreeSetup`
   (repo `cwd`, every `WorktreeInfo` with `path`/`agent_cwd`/`branch`/`synthetic_paths`) is required
   by `write_group` (it reads `setup.cwd` → `repoRoot`, `setup.baseCommit`, and
   `setup.worktrees[].{index,path,branch}` → the cleanup tasks, `:530-531,:574-584`).
   **`assign_worktree_cwds` must return the `WorktreeSetup`** (or a struct containing it), and
   `run_parallel_group` must hold it across the dispatch.
3. **`crates/cyrup-ext-subagents/src/background/runner_main/turn_loop.rs:437-465`** — builds the
   `ChainRunContext` for the detached background runner. **Add `handoff_manifest_path:
   Option<PathBuf>` to `ChainRunContext` (`chain_graph.rs:1362`) and set it here to
   `RunDir::for_existing(&run_paths.run_dir).handoff()`.** `run_paths.run_dir` is already in scope
   at `:417` (`run_dir: Some(run_paths.run_dir.clone())`). This is the field that makes
   `has_unresolved_run_handoff` (`scan.rs:430`, reading `<run_dir>/handoff.json`) find a real file.
4. **`crates/cyrup-ext-subagents/src/extension/executor/chain.rs:190-215`** — builds the
   `ChainRunContext` for the FOREGROUND `/chain` / `/parallel` walk and for the tool's
   `route_parallel_mode`. Set `handoff_manifest_path` to
   `handoff_manifest_path(&artifacts_dir, &run_id)` — pi's foreground shape
   (`subagent-executor.ts:3718`, `parallelHandoffPath(input.artifactsDir, input.runId)`).
   `artifacts_dir` comes from `crate::artifacts::resolve_artifacts_dir(session_file, Some(cwd), cwd,
   cfg.artifact_dir_preference)` — the same call `extension/executor/background.rs:169` makes.
   The `RunId` currently minted inline at `chain.rs:135` (`Some(RunId::new())`) must be **hoisted
   above** the executor construction so both the executor and the manifest path use one id.
5. **`crates/cyrup-ext-subagents/src/extension/tool/routing.rs:1940-1947`** — `route_parallel_mode`
   builds `RunnerStep::ParallelGroup { worktree: p.worktree.unwrap_or(false) }`. This is the
   `subagent({tasks:[…], worktree:true})` entry point that reaches (4) or, when `async`, (3).
   No change needed here; naming it because it is the caller that makes the feature *user-visible*.
6. **The five verbs** (`worktree.discard`, `worktree.cleanup`, `lane.status`, `lane.recordMerge`,
   `lane.recordSupersession`) attach at `extension/tool/text.rs:215` (`SUBAGENT_ACTIONS`),
   `extension/tool/schema.rs` (`handoffPath`/`laneId`/`merge`/`supersession` props, currently 0),
   `extension/tool/params.rs:138`-region (`SubagentToolParams` fields), and the management dispatch
   in `extension/executor/foreground_actions/`. `lane.status` is READ-ONLY; the other four go
   through `MUTATING_MANAGEMENT_ACTIONS` (`discovery/management/mod.rs:184`) and, for the two
   destructive ones, `registration::authority::resolve_authority_decision`
   (`registration/authority.rs`) — the gate whose own doc comment at `:20-22` names this task.
7. **RPC bridge**: `extension/rpc/params.rs:122` `manage_params` accepts exactly seven
   `SUBAGENT_RPC_MANAGEMENT_ACTIONS` (`rpc/mod.rs:112`), all `schedule.*`. **Recommendation: do NOT
   widen it.** Upstream's `rpc.ts:65-73` list is seven and does not contain the lane verbs, and
   `manage_params`' own doc comment already records "that narrowing is upstream's own choice". The
   seed asks for a deliberate decision: the decision is *no*, on upstream-parity grounds, and the
   reason belongs as a one-line note at `rpc/params.rs:117-121` rather than as a `[CYRUP-DELTA]`
   (there is no divergence to flag — matching upstream is the default).

**`RunStatus::parallel_handoff` — the decision.** **Add it.** Three reasons, each checked:
- `scan.rs:425-429`'s `[CYRUP-DELTA]` exists *only* because the field is absent; adding it lets that
  delta be deleted in the same change as the writer's, which is what the item asks for.
- pi `statusPayload.parallelHandoff = writeParallelHandoffGroup(...)` at `subagent-runner.ts:3293`,
  `:3308`, `:4427`, `:4810` — four sites — and `types.ts:920` / `:1468` / `:1994` declare it on
  `AsyncStatus`, `Details` and the result record. It is not decorative: it is how a caller learns
  the `handoffPath` to pass to `worktree.cleanup`/`lane.*` (`schemas.ts:298` literally says
  *"Existing manifest for worktree/lane actions"*, and `subagent-executor.ts:6247` errors with
  *"requires handoffPath from parallelHandoff.path or async status"*).
- Shape: `#[serde(default, skip_serializing_if = "Option::is_none")] pub parallel_handoff:
  Option<HandoffReference>` on `RunStatus` (`background/records.rs`, beside
  `workflow_receipt_path` at `:315`), plus the same field on `ResultFile`/`Details` if the sibling
  needs it for the tool result. `RunStatus::queued` (`:357`) gains `parallel_handoff: None`.
  `HandoffReference` = pi `ParallelHandoffReference` (`types.ts:523-531`): `version`, `path`,
  `groupCount`, `childCount`, `changedPatches`, `cleanupState`, optional `cleanupEligibility`.

Without (a)+(6), the writer lands with a caller but the *user* still cannot address the manifest.
Do not skip them.

### 5. On-disk JSON — the pinned contract

`<run_dir>/handoff.json` (async) or `<artifactsDir>/handoffs/<runId>.json` (foreground):

```json
{
  "version": 1,
  "runId": "0198f2…",
  "mode": "parallel",
  "source": "async",
  "cwd": "/repo/packages/app",
  "createdAt": 1758153600000,
  "updatedAt": 1758153611000,
  "groups": [{
    "stepIndex": 0,
    "baseCommit": "a1b2c3…",
    "repoRoot": "/repo",
    "children": [{
      "index": 0, "taskIndex": 0, "agent": "worker",
      "workflowKey": "lane.a", "runId": "child-1",
      "lane": { "version": 1, "key": "lane.a", "mode": "mutation",
                "sourceRef": "main", "claims": ["src/a.rs"], "outputPaths": ["out/a.json"] },
      "status": "completed", "summary": "done",
      "outputPath": "…", "structuredOutput": {}, "structuredOutputPath": "…", "sessionPath": "…",
      "patch": { "path": "…/task-0.patch", "branch": "subagents/…-0", "changed": true,
                 "diffStat": " a.rs | 2 +-", "filesChanged": 1, "insertions": 1, "deletions": 1 }
    }],
    "laneBindings": [{ "index": 0, "taskIndex": 0, "workflowKey": "lane.a", "runId": "…", "lane": {…} }],
    "cleanup": { "state": "partial", "pruned": false,
      "tasks": [{ "index": 0, "path": "/tmp/wt-0", "branch": "subagents/…-0",
                  "provider": "native", "naming": {…},
                  "worktreeRemoved": false, "branchRemoved": false, "preserved": true,
                  "reason": "cleanup pending durable handoff capture" }],
      "errors": ["…"] }
  }],
  "merge": { "prNumber": 42, "reviewedHead": "<40 hex>", "mergeCommit": "<40 hex>",
             "treeEquivalent": true, "postMergeChecks": "recorded",
             "attestedBy": "…", "attestedAt": "2026-09-18T00:00:00Z", "manifestDigest": "<64 hex>" },
  "supersession": { "supersededBy": "lane-b", "attestedBy": "…", "attestedAt": "…",
                    "manifestDigest": "<64 hex>" },
  "cleanupEligibility": { "state": "terminal-eligible" }
}
```

**Who already reads it:** `crates/cyrup-ext-subagents/src/background/async_retention/scan.rs:430`
`has_unresolved_run_handoff`, via `read_json_object` (`:459`), called from `:282`. It touches
`version`, `groups`, and `groups[].cleanup.state` only — so those three are the compatibility floor.
Its `[CYRUP-DELTA]` at `:425-429` (two clauses: "`RunStatus` has no `parallel_handoff` field" and
"no cyrup writer produces the file today") **becomes deletable in full** once §4's items (3) and
the `RunStatus` field land. Deleting it is part of the definition of done, not a nicety.

Format rules that MUST hold: camelCase keys; absent-not-null for every optional (`skip_serializing_if`);
`cleanupEligibility` **absent** on a fresh manifest; `laneBindings` present **only** when the group
has no child rows; timestamps epoch-millis integers; `treeEquivalent` is `true`/`false`/`"unknown"`
(never `null`); `manifestDigest` computed over `{version, runId, mode, source, cwd, groups}` ONLY
(`manifestFactsDigest:232-241` — note `createdAt`/`updatedAt`/`merge`/`supersession`/
`cleanupEligibility` are deliberately excluded, so recording evidence does not invalidate it).

Writes go through `background::atomic::write_atomic_json_creating_parent` (`:114`), not
`write_atomic_json` — the foreground `handoffs/` subdirectory will not exist. pi's own
`writeAtomicJson` mkdir -p's (`shared/atomic-json.ts:56-58`), so this matches rather than diverges.

### 6. The reachability test

**Primary (the one that proves the bar).** A new `#[tokio::test]` in
`crates/cyrup-it/tests/subagents/background_runner_main_integration.rs`, built on that file's
existing `run_against_fixture(dir, &script, config)` harness (`:~230-246`), which drives the REAL
detached-runner `run()`:

1. Build a real git repo with one commit (copy `spawn/worktree.rs`'s `make_real_git_repo`,
   `worktree.rs:~1248`).
2. `RunnerConfig { cwd: <repo>, worktree_base_dir: Some(<tmp>), steps: vec![
   RunnerStep::ParallelGroup(ParallelGroupSpec { steps: [two single steps], concurrency: 2,
   fail_fast: false, worktree: true })], mode: RunMode::Parallel, … }` — every other field as the
   existing `happy_path_…` test sets it (`:264-308`).
3. The scripted child fixture writes a file into its own cwd so at least one patch is `changed: true`.
4. Assert, **through production readers only**:
   - `RunDir::for_existing(&run_paths.run_dir).handoff()` **exists**;
   - `handoff::read_manifest(...)` returns a `Manifest` with `version: 1`, `runId == status.run_id`,
     `source: Async`, `mode: Parallel`, `groups.len() == 1`, `groups[0].children.len() == 2`,
     `groups[0].cleanup.tasks.len() == 2`, and `changedPatches >= 1`;
   - **`scan_run_candidates(...)`** (the production retention scan, `scan.rs:~250`) reports
     `has_unresolved_run_handoff == true` for the run while cleanup is `partial`, and `false` once
     the group's cleanup reaches `complete`;
   - `status.parallel_handoff` is `Some(reference)` whose `path` equals the manifest path.

**Why it FAILS if gutted.** Every assertion is downstream of a real fan-out: the file only exists if
`run_parallel_group` actually called `write_group` after `dispatch_group`; `children.len() == 2`
only holds if the real `StepResult`s were folded in; `cleanup.tasks` only holds if the real
`WorktreeSetup` survived `assign_worktree_cwds`; `changedPatches >= 1` only holds if
`diff_worktrees` ran against the real git worktrees; and the retention assertion is made by a
function this change does not touch. A stub writer, a hard-coded `groups: []`, an unwired module,
or a writer behind a config flag all fail at least two of these. **The test constructs no handoff
type directly** — it hands a `RunnerConfig` to `run()` and reads files off disk.

**Secondary (cheap, same file as the code).** A unit test in `handoff/model.rs` that
serializes a fixture `Manifest` and asserts the literal JSON **string** contains
`"cleanupEligibility":{"state":"terminal-blocked","reason":…}` and `"treeEquivalent":true` — the
on-disk spelling of the two hand-written serde impls is the interface and a `#[serde(rename_all)]`
typo would otherwise pass every round-trip test.

**Tertiary.** A unit test that mutates a written manifest's `cleanupEligibility` to
`terminal-eligible` by hand and asserts `trusted_cleanup_eligibility()` returns `Unknown` — pi
`:318-322`, the anti-forgery rule. This is the one whose absence would be silently exploitable.

### 7. Safety invariants (what must NEVER happen)

1. **A worktree holding uncommitted work is never removed.** The manifest's role in this: a cleanup
   task is written with `preserved: true` and `reason: "cleanup pending durable handoff capture"`
   (`:583`) BEFORE any removal is attempted, so a crash leaves the worktree recorded and preserved
   rather than orphaned-and-forgotten.
2. **The manifest is written TWICE around cleanup** — once before (`:4425`/`:3753`), once with the
   cleanup report (`:4427`/`:3755`). Never collapse these into one write. A crash between them
   leaves a complete, `partial`-state manifest that `has_unresolved_run_handoff` correctly reports;
   a single write after cleanup leaves *nothing* if the process dies during removal.
3. **`cleanupEligibility` read off disk is never trusted** (`trustedStoredCleanupEligibility:313`).
   It must be re-derived; disagreement collapses to `Unknown`, and `Unknown` formats as
   "removal is not safe" (`:424`). Never short-circuit this for speed.
4. **Active children block everything.** A group with cleanup tasks and zero children is ACTIVE
   (`:330`). Recording merge or supersession evidence while any child is non-terminal is refused
   (`:441`, `:460`). Do not weaken to "no *running* children".
5. **Evidence is digest-bound.** `manifestDigest` is stamped by the recorder (`:442`, `:461`) and
   re-checked on read (`:372`, `:379`); a mismatch is `…evidence is stale`, which blocks cleanup.
   Never accept caller-supplied `manifestDigest` (upstream overwrites it — `{...normalize, digest}`).
6. **Merge evidence is append-once per reviewed head.** A second merge with a different
   `reviewedHead` is `Lane manifest is stale` (`:445`); same head with different details is a
   conflict (`:446`). Idempotent re-record of *identical* evidence is allowed.
7. **Self-supersession is refused** (`:462`).
8. **Writing a manifest for a different run is refused** (`:514-516`, `:136`, `:165`) — `runId`,
   `mode` and `source` are all identity.
9. **`resolveRetainedWorktreeCwd` must not escape the worktree root** (`:172-188`): the cwd's
   repo-relative prefix must not be absolute or start with `..`; `cleanup.path` must be a directory
   and not a symlink (`lstat` check, `:179-181`); the realpath'd target must be inside the realpath'd
   worktree root (`pathInside`, `:187`). All three checks are load-bearing against a symlinked
   worktree redirecting a child's cwd outside the sandbox.
10. **`groups` is never written empty.** cyrup's own reader treats an empty `groups` as
    unresolved-forever (`scan.rs:444-446`), so an empty-group manifest would pin the run directory
    against retention permanently.
11. **`writeWorktreeSetupHandoff` must never copy argv, environment, `Error` objects, or captured
    stdout/stderr into the artifact** (`:628`, explicit upstream comment) and truncates diagnostics
    to 512 bytes (`:627`). This file is written into a project directory; it is not a log.
12. **Never regress the retention reader's contract**: `version: 1`, non-empty `groups`, and
    `groups[].cleanup.state` must keep the exact spellings at `scan.rs:438-454`.

### 8. Blockers / dependencies

1. **HARD BLOCKER — `worktree: true` cannot succeed in a default install today.**
   `chain_graph.rs:2069-2074` requires `ctx.worktree_base_dir` to be `Some`, but
   `registration/mod.rs:520` defaults it to `None` and **nothing computes a fallback** (verified:
   `grep -rn "worktree_base_dir" crates/ --include=*.rs` — every non-test assignment is a pass-through,
   and `registration/mod.rs:1440` asserts the default is `None`). So `subagent({tasks:[…],
   worktree:true})` currently returns `worktree: true group requires ChainRunContext::worktree_base_dir
   to be configured` unless the operator set `subagents.worktreeBaseDir`. Meanwhile
   `spawn/worktree.rs:284` `resolve_worktree_base_dir(None, repo_root)` already implements the
   env-var → `temp_dir()` fallback, and `create_worktrees` (`:843`) calls it. **Fix:
   `assign_worktree_cwds` must pass `ctx.worktree_base_dir` through as an `Option` instead of
   erroring on `None`** (which means `WorktreeGroupConfig::worktree_base_dir` at `worktree.rs:1121`
   becomes `Option<&Path>`, or `assign_worktree_cwds` calls `create_worktrees` directly). Without
   this the manifest writer is reachable only from a non-default config, which fails the standing bar.
2. **`WorktreeCleanupReport` does not exist** (`spawn/worktree.rs:1045` `cleanup_worktrees(&WorktreeSetup)`
   returns `()`). The manifest's `group.cleanup` **cannot be populated** until the cleanup sibling
   lands `WorktreeCleanupTask` / `WorktreeCleanupReport` / `WorktreeCleanupIntent`
   (`worktree.ts:82-110`). **Sequencing: cleanup types first, manifest model second, writer third.**
   Proposal to avoid a duplicate definition: `WorktreeCleanupTask`/`WorktreeCleanupReport` are
   *defined in* `handoff::model` (they are the serialized shape) and *used by* `spawn::worktree`.
3. **`assign_worktree_cwds` discards the `WorktreeSetup`** (`worktree.rs:1170-1207` →
   `WorktreeGroupPlan`). It must be widened to return the setup. This touches
   `chain_graph.rs:2065-2091` and `worktree.rs:1145-1207`.
4. **`ChainRunContext` has no run identity.** It carries `cwd` but no `run_id` and no run dir
   (`chain_graph.rs:1362-1419`). A `handoff_manifest_path: Option<PathBuf>` field must be added and
   populated at both construction sites (`turn_loop.rs:437`, `executor/chain.rs:190`) plus the
   test constructors (`chain_graph.rs:2344,2365,2583,3501`, `runner_main/executor.rs:1128,1290`).
5. **`StepResult` has no `stopped` flag** (`chain_graph.rs:1127-1246`: `success`, `interrupted`,
   `timed_out`, `exit_code`, but no stop marker). pi's status mapping
   (`subagent-runner.ts:4411-4416`) is `stopped → "stopped"`, `interrupted → "paused"`,
   `exitCode === 0 → "completed"`, else `"failed"`. cyrup can produce the first three of four;
   the `stopped` arm needs either a new `StepResult` field or a mapping from the run-level
   `RunState::Stopped`. **Decide explicitly and record it** — silently collapsing `stopped` into
   `failed` in the manifest would make a stopped lane look like a crashed one to `lane.status`.
6. **`DynamicGroup` has no worktree support.** `ParallelGroupSpec` has `worktree: bool`;
   `DynamicGroupSpec` does not (`chain_graph.rs`, `run_dynamic_group`). pi writes handoffs for
   dynamic fan-outs too (`subagent-runner.ts:4776-4810`). **Out of scope for this item** — but say so,
   rather than letting a reviewer think it was missed.
7. **Timestamp parsing dependency**: `AttestationTimestamp` needs an RFC 3339 parser. Check whether
   `time` or `chrono` is already a `cyrup-ext-subagents` dependency before adding one; if neither is,
   a hand-rolled RFC 3339 shape check is preferable to a new dependency for one field.
8. **`extension/tool/text.rs`'s `the_tool_reference_topic_names_every_dispatched_verb` test**
   (referenced at `text.rs:207-210`) will fail the moment the five verbs are added to
   `SUBAGENT_ACTIONS` without the packaged guide page being updated. The guide resource lives under
   `registration/resources.rs`. Budget for it.

### 9. Files expected to be touched (for sequencing)

**New**
- `crates/cyrup-ext-subagents/src/handoff/{mod,lane,model,evidence,read,write,path,error,format}.rs`

**Modified — model/plumbing (mine)**
- `crates/cyrup-ext-subagents/src/lib.rs` (register `pub mod handoff;`)
- `crates/cyrup-ext-subagents/src/error.rs` (`SubagentError::Handoff(#[from] HandoffError)`)
- `crates/cyrup-ext-subagents/src/background/run_paths.rs` (`RunDir::handoff()`, own the
  `handoff.json` literal)
- `crates/cyrup-ext-subagents/src/background/records.rs` (`RunStatus::parallel_handoff`,
  `RunStatus::queued`)
- `crates/cyrup-ext-subagents/src/background/async_retention/scan.rs` (**delete** `HANDOFF_MANIFEST_FILE`
  `:53` in favour of `RunDir::handoff()`; **delete** the `[CYRUP-DELTA]` `:425-429`)
- `crates/cyrup-ext-subagents/src/spawn/chain_graph.rs` (`ChainRunContext::handoff_manifest_path`;
  `assign_worktree_cwds` returns `WorktreeSetup`; `run_parallel_group` writes the manifest)
- `crates/cyrup-ext-subagents/src/background/runner_main/turn_loop.rs:437` (populate the field)
- `crates/cyrup-ext-subagents/src/extension/executor/chain.rs:135,190` (hoist `RunId`, resolve the
  artifacts dir, populate the field)

**Modified — shared with the cleanup sibling (coordinate, do not both edit blind)**
- `crates/cyrup-ext-subagents/src/spawn/worktree.rs` — `cleanup_worktrees` gains an intent and
  returns a report; `WorktreeGroupConfig::worktree_base_dir` becomes optional; `WorktreeInfo` gains
  `provider`/`naming` if the sibling ports them.

**Modified — tool surface (likely the other sibling)**
- `extension/tool/text.rs` (`SUBAGENT_ACTIONS`), `extension/tool/schema.rs`
  (`handoffPath`/`laneId`/`merge`/`supersession`/`lane`), `extension/tool/params.rs`
  (`SubagentToolParams`), `extension/executor/foreground_actions/` (dispatch),
  `registration/authority.rs` (wire `DiscardWorktree`/`DestructiveCleanup`),
  `discovery/management/mod.rs` (`MUTATING_MANAGEMENT_ACTIONS`),
  `registration/resources.rs` (guide page).

**Tests**
- `crates/cyrup-it/tests/subagents/background_runner_main_integration.rs` (the reachability test)
- `crates/cyrup-ext-subagents/src/handoff/*` unit tests (serde spelling, anti-forgery, validation)
- Every `ChainRunContext { … }` literal: `chain_graph.rs:2344,2365,2583,3501`,
  `runner_main/executor.rs:1128,1290`, `tests/dynamic_collect_record_fidelity.rs:110`,
  `tests/dynamic_group_acceptance_parity.rs:101` — each needs the new field.


## [AUG — cleanup]

Research output for the **two-phase worktree cleanup plan** (`src/runs/shared/worktree-cleanup-plan.ts`,
869 lines @`v0.68.0`, read in full). Research only — no source file was touched.

### 0. Anchor re-verification (every file:line the seed cites, re-checked)

Upstream, via `git -C /home/user/cyrup/tmp/pi-subagents show v0.68.0:<path>`:

| seed claim | verified |
| --- | --- |
| `WORKTREE_CLEANUP_PLAN_TTL_MS:21` = 30 min | ✅ `:21` `30 * 60 * 1000` |
| `WorktreeCleanupPlanPreconditions:31` | ✅ |
| `GitResult:90` | ✅ |
| `__testables:176` | ✅ `export const __testables = { realpathExisting }` |
| `parseGitWorktreeList:269` | ✅ |
| `buildWorktreeCleanupPlan:742` | ✅ |
| `worktreeCleanupPlanPath:821` | ✅ |
| `createWorktreeCleanupPlan:825` | ✅ |
| `formatWorktreeCleanupPlan:837` | ✅ |
| state/decision/source unions | ✅ `:26`, `:27`, `:28`; `ForegroundRunOwnership:29` |
| dispatch `subagent-executor.ts:6270-6290` | ✅ the `lane.*` arm; `worktree.cleanup` is `:6213-6241`, `worktree.discard` `:6242-6268` |

cyrup side:

| seed claim | verified |
| --- | --- |
| `extension/tool/schema.rs:430` advertises `worktree` | ✅ exact line 430 |
| `extension/tool/text.rs:323` lists `worktree.discard` | ✅ exact line 323 |
| `registration/authority.rs:20` "Whoever lands …" | ✅ exact line 20 |
| `background/async_retention/scan.rs:423` | ✅ the doc comment is `:423`; the fn `has_unresolved_run_handoff` is `:430` |

**No stale anchors.** One seed/task-text claim is WRONG and it matters — see §1.

### 1. THE CORRECTION: upstream `worktree.cleanup` is PLAN-ONLY at v0.68.0

The computed task says *"the plan is persisted with a TTL, and only then can `worktree.cleanup`
execute it."* **That second half does not exist upstream.** `subagent-executor.ts:6217-6222`
@v0.68.0:

```ts
6217  if (paramsWithResolvedCwd.mode !== "plan") {
6218      return { … text: "worktree.cleanup currently supports mode='plan' only; apply/removal is not available yet." …, isError: true … };
6219  }
6220  if (paramsWithResolvedCwd.planId !== undefined) {
6221      return { … text: "worktree.cleanup plan mode does not accept planId; apply is not available yet." …, isError: true … };
6222  }
```

and `schemas.ts:300`: `planId: Type.Optional(Type.String({ description: "Reserved; cleanup is plan-only." }))`.
`formatWorktreeCleanupPlan:866` ends every render with `"Plan-only mode: no worktrees or branches were removed."`
`git grep v0.68.0 -- src` finds exactly two consumers of this module: the import at `:154` and the
one call at `:6224`. **There is no apply path, no plan reader, and no TTL enforcement anywhere in
upstream.**

**Therefore the port is plan-only too.** `expiresAt`/`contentHash`/`preconditions` are the
*forward contract* a later apply phase will check; they must be written correctly and completely
(they are the evidence that makes a later apply safe), but implementing an executor is OUT OF
SCOPE and would be inventing behaviour, not porting it. The refusals above are ported verbatim,
which is what keeps `mode='apply'` from being silently accepted later.

**What "stale" means, since the task asked:** two different things, and neither is TTL-checked today.
1. A plan is stale when `now > expiresAt` (`createdAt + 30min`, `:816`). Nothing reads it yet.
2. `CleanupState::Stale` on an ENTRY means something else entirely: metadata and git disagree —
   the path is gone from disk (`:619`, `:623`), the manifest already records the worktree as removed
   (`:633`), the active-run marker is older than 24 h (`:652`), or metadata names a worktree git has
   never heard of (`:723`). **`Stale` always decides `Unknown`, never `Remove`.**

### 2. What cyrup ALREADY has (grepped hard; assume-it-exists discipline applied)

**Absent.** `grep -rn "cleanup_plan\|CleanupPlan\|worktree\.cleanup\|prune_candidates\|worktree list --porcelain"`
over `crates/**/*.rs` returns **two** hits, both of them the pre-placed gates the seed names
(`authority.rs:20`, `text.rs:323`). There is no plan builder, no porcelain parser, no plan file.

**Present and reusable — this is the half that matters:**

| cyrup seam | what it gives this work |
| --- | --- |
| `crates/cyrup-ext-subagents/src/spawn/worktree.rs:195` `run_git(cwd,&[&str]) -> Result<GitResult, SubagentError>` | THE git idiom. `tokio::process::Command`, `String::from_utf8_lossy`, `status: Option<i32>`. Private — must be promoted `pub(crate)`. |
| `spawn/worktree.rs:209` `run_git_checked` | pi `runGitChecked`, incl. the `stderr→stdout→"<cmd> failed"` message ladder = pi `gitFailure:88`. |
| `spawn/worktree.rs:184` `struct GitResult` | already pi's shape. Promote `pub(crate)`. |
| `spawn/worktree.rs:284` `resolve_worktree_base_dir(configured, repo_root)` | **the create-side base dir. THE plan must call THIS.** See the delta in §4.1. |
| `spawn/worktree.rs:279` `build_worktree_path` → `<base>/cyrup-worktree-<runId>-<index>` | the leaf-name shape the containment tightening in §4.1 keys off. |
| `spawn/worktree.rs:254` `normalize_comparable_cwd` | pi `comparablePath`'s core (absolute → canonicalize → fallback). Private; promote or re-implement with the missing-segment walk (§4.2). |
| `spawn/worktree.rs:236` `lexical_normalize` | the `..`-collapsing half `comparablePath` needs for a path whose tail does not exist. |
| `background/active_run_index.rs:319` `active_run_marker_age_ms(async_dir, now) -> Option<i64>` (async) | pi `activeRunMarkerAgeMs`, exact. |
| `background/active_run_index.rs:76` `DEFAULT_STALE_TERMINAL_ACTIVE_MARKER_MS` = 24 h | pi's constant, exact. |
| `background/atomic.rs:75` `write_atomic_json` / `write_atomic_json_creating_parent` | pi `writeAtomicJson`. The plan dir will not exist → use the `_creating_parent` form. |
| `background/run_paths.rs:52,62` `RunDir::for_existing(dir).status()` → `<dir>/status.json` | pi `readStatusBesideManifest:368`. |
| `background/state.rs:70` `RunState` {Queued,Running,Paused,Complete,Failed,Stopped}, `#[serde(rename_all="camelCase")]` | pi `AsyncStatus["state"]`. **cyrup has no `partial`/`rejected`** — see §4.4. |
| `background/records.rs:210` `RunStatus` (`run_id`, `state`, …) | the `status.json` shape. |
| `artifacts.rs:155` `project_subagents_dir(cwd)` → `<cwd>/.cyrup-subagents`, `:161` `project_artifacts_dir` | pi `PROJECT_SUBAGENTS_RELATIVE_DIR`. cyrup's root is `.cyrup-subagents`, pi's `.pi-subagents` — already an established crate-wide delta. |
| `background/async_retention/scan.rs:53` `HANDOFF_MANIFEST_FILE = "handoff.json"`, `:430` `has_unresolved_run_handoff` | the existing READER of the manifest this plan cross-checks. |
| `error.rs:17` `SubagentError` (thiserror) | the crate error taxonomy to extend. |
| `extension/tool/routing.rs:1084` `route_action` | THE production dispatch seam. |
| `extension/tool/scheduled_runs_tests.rs` | the exact reachability-test pattern to copy (§8). |

**Absent and needed, that upstream puts in `worktree.ts` not in my file** (so nobody assumes the
sibling has it): `MACHINE_DIFF_OPTIONS` (`worktree.ts:23`), `validateWorktreePatch` (`:297`),
`currentWorktreePatch` (`:303`), `validateWorktreePatchRepresentsCurrentWorktree` (`:329`).
`grep -rn "MACHINE_DIFF_OPTIONS\|validate_worktree_patch"` over cyrup → **0 hits.** These three are
called from `isPatchCaptured:591`, which is in MY file, so this area owns porting them into
`spawn/worktree.rs` beside `capture_worktree_diff`.

### 3. The state machine, enumerated exactly (`buildManagedEntry:596-711`, in evaluation order)

Every arm below is `blockedEntry(entry, state, decision, reason)` and returns immediately. Line
numbers are `worktree-cleanup-plan.ts` @v0.68.0.

| # | condition | state | decision | line |
| -- | --- | --- | --- | --- |
| 1 | manifest `runId` missing/blank | Unknown | Unknown | 613 |
| 2 | manifest `source` ∉ {foreground, async} | Unknown | Unknown | 614 |
| 3 | group `repoRoot` missing/blank | Unknown | Unknown | 615 |
| 4 | metadata path ≠ git worktree path (`samePath`) | Unknown | Unknown | 616 |
| 5 | git path un-inspectable | Unknown | Unknown | 618 |
| 6 | git path missing from disk | **Stale** | Unknown | 619 |
| 7 | git path is a symlink | Unknown | Unknown | 620 |
| 8 | metadata path un-inspectable | Unknown | Unknown | 622 |
| 9 | metadata path missing from disk | **Stale** | Unknown | 623 |
| 10 | metadata path is a symlink | Unknown | Unknown | 624 |
| 11 | not a directory / no realpath | Unknown | Unknown | 625 |
| 12 | realpath inside `<agentDir>/extensions` | **Ineligible** | Keep | 627 |
| 13 | containment invalid, or realpath not a STRICT child of baseDir | **Ineligible** | Keep | 628 |
| 14 | realpath == repo root | Ineligible | Keep | 630 |
| 15 | detached worktree (no branch) | Unknown | Unknown | 631 |
| 16 | metadata branch ≠ git branch | Unknown | Unknown | 632 |
| 17 | `task.worktreeRemoved` already true | **Stale** | Unknown | 633 |
| 18 | `task.preserved !== true` | Unknown | Unknown | 634 |
| 19 | `group.cleanup.state !== "partial"` | Unknown | Unknown | 635 |
| 20 | `task.reason` contains `"retained child resume"` (lowercased) | Ineligible | Keep | 637 |
| 21 | `task.reason` contains `"cleanup pending durable handoff capture"` | Ineligible | Keep | 638 |
| 22 | `group.repoRoot` realpath ≠ requested repo root | Unknown | Unknown | 639 |
| 23 | same branch checked out in ANOTHER git worktree | Unknown | Unknown | 642 |
| 24 | branch is what the repo ROOT has checked out | Ineligible | Keep | 645 |
| 25 | `inspectRunState` == unknown | Unknown | Unknown | 650 |
| 26 | `inspectRunState` == active | **Active** | Keep | 651 |
| 27 | `inspectRunState` == stale | **Stale** | Unknown | 652 |
| 28 | `group.baseCommit` missing/blank | Unknown | Unknown | 655 |
| 29 | base commit not a resolvable local commit | Unknown | Unknown | 657 |
| 30 | `refs/heads/<branch>` tip unresolvable | Unknown | Unknown | 661 |
| 31 | branch tip ≠ git worktree HEAD | Unknown | Unknown | 664 |
| 32 | `git status --porcelain=v1 --untracked-files=all` errored | Unknown | Unknown | 667 |
| 33 | **status output non-empty ⇒ uncommitted or untracked work** | **Dirty** | **Keep** | **669** |
| 34 | `git diff --quiet <MACHINE_DIFF_OPTIONS> <base> --` exit ∉ {0,1} | Unknown | Unknown | 672 |
| 35 | `git merge-base --is-ancestor <head> <targetHead>` exit ∉ {0,1} | Unknown | Unknown | 674 |
| 36 | diverged AND captured patch present but invalid | Ineligible | Keep | 679 |
| 37 | diverged, no usable patch, tip NOT an ancestor of HEAD | Ineligible | Keep | 685 |
| 38 | manifest or any report path lies INSIDE the worktree | Ineligible | Keep | 694 |
| 39 | a durable report path is missing | Unknown | Unknown | 695 |
| 40 | a durable report path is a symlink | Unknown | Unknown | 696 |
| 41 | any output/patch path lies inside the worktree | Ineligible | Keep | 702 |
| ✔ | everything above passed | **Safe** | **Remove**, `willDeleteBranch = branchTipIsAncestor` | 705-709 |

Two entry kinds never reach that machine:
* `buildUnknownGitEntry:713` — a git worktree with no (or >1) matching metadata record →
  `source: git`, Unknown/Unknown, *"no matching extension-owned handoff metadata was found"*.
* `buildMissingMetadataEntry:719` — metadata with no git worktree → `source: metadata`,
  Stale/Unknown, *"handoff metadata records a worktree that is not present in Git worktree state"*.
  **Returns `undefined` (no entry at all) when `worktreeRemoved && branchRemoved`** (`:720`).

`inspectRunState:485-519`, expanded:
* `statusError` present → Unknown.
* `status.runId !== manifest.runId` → Unknown.
* `status.state` ∈ {queued, running} → **Active**.
* `status.state` ∉ terminal set {complete, failed, partial, paused, stopped, rejected} → Unknown.
* no status at all AND `manifest.source === "async"` → Unknown (*"async status is missing beside …"*).
* group has **zero** children, or any child status is non-terminal → **Active**.
* `manifest.source === "foreground"` → consult the injected `foregroundRunOwnership(runId)`:
  `active`→Active, `terminal`→Terminal, anything else (incl. a throw) → **Unknown**. *Absence of
  proof is never treated as termination.*
* otherwise → `activeRunMarkerAgeMs(dirname(statusPath), now)`: `Some(age) && age <= 24h` → Active;
  `Some(age)` older → **Stale**; `None` → Terminal.

`pruneCandidates:797-800`: entries that are `decision == unknown && state == stale` **and** whose
path git itself reports `prunable`. Reported only; nothing is pruned (`:862`).

Caps and warnings: `MAX_DISCOVERED_HANDOFFS = 256` (`:22`), `MAX_PLAN_ENTRIES = 512` (`:23`, applied
to groups `:387`, tasks `:395` and the final entry list `:793`), `MAX_METADATA_FILE_BYTES = 2 MiB`
(`:24`, on both manifest `:315` and status `:372`). Warnings are deduped and sorted (`:364`, `:440`,
`:808`).

### 4. The RUST design

Module: **`crates/cyrup-ext-subagents/src/spawn/cleanup_plan/`** — a directory module beside
`spawn/worktree.rs`, because (a) it is the only consumer of `run_git`/`GitResult`/
`resolve_worktree_base_dir`, which are private to that file and get promoted to `pub(crate)`, and
(b) at ~900 upstream lines a single file would be the crate's largest leaf.

```
spawn/cleanup_plan/
  mod.rs        public API + build_worktree_cleanup_plan orchestration (pi :742-830)
  model.rs      the serde types, the enums, the newtypes, the JSON contract
  verdict.rs    Verdict + reason enums + the state→decision total function
  gitwt.rs      GitWorktreeRecord + parse_git_worktree_list + list_git_worktrees (pi :269-301)
  metadata.rs   handoff discovery/read/join + ManifestMetadataRecord (pi :303-441)
  paths.rs      comparable_path / same_path / path_inside / inspect_path (pi :158-232, :443-454)
  classify.rs   buildManagedEntry's 41 gates as ONE exhaustive match (pi :596-724)
  render.rs     format_worktree_cleanup_plan (pi :832-869)
```

#### 4.0 The headline type win: `Verdict` makes state and decision inseparable

Upstream pairs `state` and `decision` by hand at **fourteen** call sites. I checked all forty-one
rows in §3: the mapping is a **total function** — `safe→remove`; `ineligible|dirty|active→keep`;
`stale|unknown→unknown`. Nothing else occurs. So in Rust it is one value, not two fields:

```rust
/// pi's (state, decision) pair, collapsed. The mapping is total (verified over all 41 arms of
/// `buildManagedEntry` @v0.68.0), so carrying two independent fields would let a future arm
/// spell `state: "dirty", decision: "remove"` — the single worst bug this module can have.
pub(crate) enum Verdict {
    /// Every gate passed. `delete_branch` is pi `willDeleteBranch` (`:707`) and exists ONLY here,
    /// so no non-removable entry can carry a branch-deletion flag.
    Safe { delete_branch: bool, note: Option<SafeNote> },
    Ineligible(IneligibleReason),
    Dirty,                       // pi `:669` — the one that must never be Remove
    Active(ActiveReason),
    Stale(StaleReason),
    Unknown(UnknownReason),
}

impl Verdict {
    pub(crate) fn state(&self) -> CleanupState { … }
    /// The total function. `match` is exhaustive; adding a Verdict arm without a decision is a
    /// compile error, which is the whole point.
    pub(crate) fn decision(&self) -> CleanupDecision {
        match self {
            Self::Safe { .. } => CleanupDecision::Remove,
            Self::Ineligible(_) | Self::Dirty | Self::Active(_) => CleanupDecision::Keep,
            Self::Stale(_) | Self::Unknown(_) => CleanupDecision::Unknown,
        }
    }
    pub(crate) fn reasons(&self) -> Vec<String> { /* Display of the typed reason, pi's wording */ }
}
```

`IneligibleReason` / `StaleReason` / `UnknownReason` / `ActiveReason` are `thiserror`-free plain
enums with `Display` impls carrying **pi's verbatim sentences** (the ones in §3), because those
strings are the model-facing explanation. Variants that interpolate carry their data:
`UnknownReason::BranchCheckedOutElsewhere`, `UnknownReason::MetadataBranchMismatch { metadata: String, git: String }`,
`IneligibleReason::OutsideBaseDir { base_dir: PathBuf }`, `StaleReason::ActiveMarkerStale { age_secs: u64 }`, etc.

#### 4.1 [CYRUP-DELTA] the base directory — **the single highest-risk item in this area**

Upstream `resolveCleanupBaseDir:213-227` reproduces upstream's CREATE-side layout:
`<dirname(repoRoot)>/worktrees/<basename(repoRoot)>` (or `$PI_SUBAGENTS_WORKTREE_DIR` + the
`basename` level). Verified against the create side: `worktree.ts:648-651` builds the same
`dedicatedRoot`, and `buildNativeWorktreePath:684-686` =
`path.join(buildNativeProjectPath(dedicatedRoot, repoRoot), "pi-worktree-…")`. The two agree, which
is what makes the strict-child containment check at `:628` pass.

**cyrup's create side is different and a transliteration of `resolveCleanupBaseDir` would make this
feature inert by construction.** `spawn/worktree.rs:284-300` resolves
`configured_base_dir → $CYRUP_SUBAGENTS_WORKTREE_DIR → std::env::temp_dir()` with **no `worktrees/`
rung and no `basename(repoRoot)` level**, and `build_worktree_path:279-281` puts the leaf directly at
`<base>/cyrup-worktree-<runId>-<index>`. Under the ported resolver, `pathInside(baseDir, realpath, strict)`
would be false for every real cyrup worktree → every entry `Ineligible/Keep` → a cleanup verb that
can never propose removing anything.

**Decision:** the plan calls **`spawn::worktree::resolve_worktree_base_dir(config.worktree_base_dir, repo_root)`
— the SAME function the create side calls.** `[CYRUP-DELTA]`: *the containment check's premise is
"this is where THIS build puts managed worktrees", and cyrup's layout is `<base>/cyrup-worktree-*`,
not pi's `<dedicatedRoot>/<repoName>/pi-worktree-*` (`spawn/worktree.rs:279-300` vs
`worktree.ts:648-686`). Porting pi's resolver would check containment against a directory this build
never writes to.*

**Tightening, because cyrup's default base is the shared system temp dir.** With nothing configured,
`resolve_worktree_base_dir` returns `std::env::temp_dir()`; "contained in `/tmp`" is a near-vacuous
guarantee. So containment gains a second conjunct, and it is a **behavioural divergence, flagged**:
the worktree's realpath must be a strict child of the base dir **and** its final component must
start with `cyrup-worktree-` (the prefix `build_worktree_path:280` writes). This only ever narrows
the Remove set; it cannot widen it. `[CYRUP-DELTA]`, premise verified at `spawn/worktree.rs:279-281`.

`cleanupContainmentInvalid:229-234` ports as-is (baseDir == repoRoot, or strictly inside repoRoot, or
inside `<agent_dir>/extensions`), using `crate::paths::resolve_agent_dir` for the extensions root.

#### 4.2 Newtypes with fallible constructors

```rust
/// pi `validatePlanId:187-192`. The regex `^[A-Za-z0-9._-]+$` plus the `.`/`..` carve-out is a
/// PATH-TRAVERSAL guard, not a style rule: the value is interpolated into a filename at `:822`.
/// As a newtype, `worktree_cleanup_plan_path` cannot be called with an unvalidated string AT ALL —
/// the guard moves from "remember to call the validator" to "does not typecheck".
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PlanId(String);
impl PlanId {
    pub fn parse(raw: &str) -> Result<Self, CleanupPlanError>;   // pi's verbatim sentence on Err
    pub fn generate() -> Self;                                    // uuid v4 (pi `randomUUID()`), already a dep
}
impl<'de> Deserialize<'de> for PlanId { /* re-validates on read — the file is untrusted input */ }

/// sha-256 hex. Newtype so it cannot be transposed with a commit id, which is also 40-64 hex chars.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentHash(String);

/// A git object name that `git rev-parse --verify <x>^{commit}` accepted. Exists so
/// `preconditions.base_commit` / `.branch_tip` / `.worktree_head` / `.target_ref` cannot be filled
/// with an unresolved ref by a later edit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CommitId(String);

/// pi `sha256(status output)` (`:561`). A DIGEST, deliberately not the status text: the plan is
/// world-readable under the repo and must not carry a listing of the user's untracked filenames.
#[serde(transparent)] pub struct StatusDigest(String);
```

`comparablePath` (`:178-195`) becomes `fn comparable_path(&Path) -> PathBuf`: canonicalize the
longest existing ancestor, re-join the missing tail, lexically normalize. `spawn/worktree.rs:236`
`lexical_normalize` + `:254` `normalize_comparable_cwd` are the two halves; promote both to
`pub(crate)` rather than writing a third copy.
`samePath`'s Windows dev/ino fallback (`:203-212`) is **not ported** — `[CYRUP-DELTA]`: the crate
targets Linux/macOS and wasm32-wasip2; `cfg(windows)` code with no gate that builds it is code that
rots. Premise verified: `rust-toolchain.toml` + the README gates name no `*-pc-windows-*` target.

#### 4.3 The error enum

Upstream throws in exactly six places, all of them *fatal to the whole plan* (not per-entry):
`:743` no repo path, `:746` non-finite `now`, `:189` bad plan id, `:129` `runGitChecked`
(`rev-parse --show-toplevel` `:220`, `rev-parse HEAD` `:750`), `:221` empty base dir, `:298`
`worktree list --porcelain` failure. Everything else is a `Verdict` or a warning. So:

```rust
#[derive(thiserror::Error, Debug)]
pub enum CleanupPlanError {
    #[error("worktree cleanup plan requires a repository path")]
    MissingRepo,
    #[error("worktree cleanup plan timestamp must be finite")]      // see note
    NonFiniteTimestamp,
    #[error("worktree cleanup plan id must contain only letters, numbers, dots, underscores, or hyphens")]
    InvalidPlanId,
    #[error("worktree base directory cannot be empty")]
    EmptyBaseDir,
    #[error("{0}")]                                                  // pi `gitFailure` text, verbatim
    Git(String),
    #[error("failed to write worktree cleanup plan {path}: {source}")]
    Write { path: PathBuf, #[source] source: std::io::Error },
}
```
wired into the crate taxonomy as `SubagentError::WorktreeCleanupPlan(#[from] CleanupPlanError)` with
`#[error("{0}")]`, matching the existing `Management(String)` convention at `error.rs:88` — upstream
surfaces these as `isError: true` prose the model reads, so no prefix may be added.

`NonFiniteTimestamp` is retained as a variant but is **structurally unreachable in Rust**, because
`now: i64` epoch-millis cannot be NaN. Rather than `allow(dead_code)` it, the timestamp seam is
typed `now: i64` and the variant is **dropped**; the doc on the `now` parameter records that pi's
`:746` guard is discharged by the type. (Nothing else in this spec is deleted on those grounds.)

#### 4.4 Run-state mapping — cyrup's `RunState` has four of pi's six terminal words

`background/state.rs:70`: `Queued | Running | Paused | Complete | Failed | Stopped`
(`rename_all = "camelCase"`). pi's terminal set (`:481-483`) is
`complete | failed | partial | paused | stopped | rejected`. cyrup has no `partial` and no `rejected`.

The status is read as `RunStatus` via serde, so an unknown state string is a **deserialize failure**,
not an "unknown state" verdict — which would collapse pi's *"owning run has unknown state 'X'"*
(Unknown) into *"invalid async status"* (also Unknown). Same verdict, different sentence. Port it as:
read the status with `serde_json::from_slice::<RunStatus>`; on failure, fall back to reading the raw
object and, if it has a string `state` and a string `runId`, emit
`UnknownReason::UnknownRunState(state)` — pi's sentence. `[CYRUP-DELTA]` with the true premise:
*cyrup's `RunState` is a closed enum and pi's is an open string union; the fallback keeps pi's
diagnostic instead of degrading every unrecognised state to "invalid status file".*

`terminalRunState` therefore = `matches!(state, Complete | Failed | Paused | Stopped)`. `Queued |
Running` → Active. The mapping is exhaustive over `RunState`, so a seventh state cannot be added
without this `match` going red.

#### 4.5 `parseGitWorktreeList` (`:269-301`) → `parse_git_worktree_list`

Pure string→`Vec<GitWorktreeRecord>`; **make it `pub(crate)` and unit-test it directly** (it is pi's
own `export`, and `__testables:176` shows upstream keeps a test seam in this module too). Rules,
verbatim: blank line flushes; `worktree <path>` starts a record with `head: ""`; `HEAD <sha>`;
`branch refs/heads/<name>` (note: the prefix is stripped, so `record.branch` is the SHORT name and is
compared against `task.branch` at `:632` — the manifest stores short names);
`detached` **clears** any branch; `prunable <reason>`; final flush. A record with an empty path is
dropped. Lines that are none of the above, and any line before the first `worktree `, are ignored —
so `bare`, `locked`, `locked <reason>` are silently skipped, which is upstream's behaviour and must
be preserved (a `locked` worktree gets no special treatment; it fails later on a real gate).

```rust
pub(crate) struct GitWorktreeRecord {
    pub path: PathBuf,
    pub head: String,                  // "" for a bare main worktree; pi keeps the record
    pub branch: Option<String>,        // short name, no refs/heads/ prefix
    pub prunable: Option<String>,
}
```

#### 4.6 Serde discipline

* `CleanupState`/`CleanupDecision`/`CleanupSource`: `#[serde(rename_all = "lowercase")]` →
  `"safe"|"ineligible"|"stale"|"dirty"|"active"|"unknown"`, `"remove"|"keep"|"unknown"`,
  `"git"|"metadata"|"both"`. Serialized from `Verdict::state()`/`::decision()`; never stored as
  independent fields in memory.
* The PLAN types (`WorktreeCleanupPlan`, `CleanupPlanEntry`, `CleanupPreconditions`):
  `#[serde(rename_all = "camelCase", deny_unknown_fields)]` + `skip_serializing_if = "Option::is_none"`.
  cyrup both writes and (later) reads these, so an unexpected key is corruption.
* The MANIFEST mirror types (`ParallelHandoffManifest` &c., owned by the sibling area):
  `rename_all = "camelCase"` but **NOT** `deny_unknown_fields`. `[CYRUP-DELTA]`, premise verified —
  upstream has ADDED optional keys to these records across versions (`types.ts:459` `provider?`,
  `:461` `naming?`, `:432` `workflowKey?`, `:435` `lane?`, `:500` `laneBindings?`); denying unknown
  fields would make cyrup refuse to plan against a newer manifest, i.e. fail **closed on a read**,
  which loses the safety analysis entirely rather than gaining any.
* `ForegroundRunOwnership` is an enum but is **never serialized** — it is the return type of an
  injected closure (`:81`), not a JSON field.

#### 4.7 The injected ownership seam

pi `foregroundRunOwnership?: (runId) => ForegroundRunOwnership` (`:81`), supplied at
`subagent-executor.ts:6230-6235`:

```ts
if (deps.state.foregroundControls.has(runId)) return "active";
const remembered = deps.state.foregroundRuns?.get(runId);
if (!remembered || remembered.children.length === 0 || remembered.children.some(c => c.status === "detached")) return "unknown";
return "terminal";
```

In Rust: `pub type ForegroundOwnershipProbe<'a> = &'a dyn Fn(&str) -> ForegroundRunOwnership;`
on `BuildCleanupPlanInput`, **not** `Option` — pi's `?? "unknown"` means the absent case is
indistinguishable from an unknown answer, so making it required removes a way to be wrong. The
production caller passes a closure over the executor's live foreground state (see §5); a test passes
a closure returning a fixed verdict.

#### 4.8 The public API

```rust
pub struct BuildCleanupPlanInput<'a> {
    pub repo: &'a Path,                              // required — pi's `:743` guard is the type
    pub handoff_path: Option<&'a Path>,
    pub handoff_paths: &'a [PathBuf],                // pi's `:74` test/migration seam
    pub worktree_base_dir: Option<&'a str>,
    pub now: i64,                                    // epoch ms, required (discharges pi `:746`)
    pub plan_id: Option<PlanId>,
    pub foreground_ownership: ForegroundOwnershipProbe<'a>,
}

pub async fn build_worktree_cleanup_plan(input: BuildCleanupPlanInput<'_>) -> Result<WorktreeCleanupPlan, CleanupPlanError>;
pub async fn create_worktree_cleanup_plan(input: BuildCleanupPlanInput<'_>) -> Result<CreatedCleanupPlan, CleanupPlanError>;
pub fn worktree_cleanup_plan_path(repo_root: &Path, plan_id: &PlanId) -> PathBuf;
pub fn format_worktree_cleanup_plan(created: &CreatedCleanupPlan) -> String;
pub struct CreatedCleanupPlan { pub plan: WorktreeCleanupPlan, pub plan_path: PathBuf }
```

`async` throughout, because `run_git` is `tokio::process` — the crate's established idiom. pi's
`spawnSync` is not a contract.

### 5. PRODUCTION CALL SITES — where this must attach to be reachable

This is the bar. Five edits, all required; the feature is unreachable if any is missing.

1. **`crates/cyrup-ext-subagents/src/extension/tool/text.rs`** — add `"worktree.cleanup"` to
   `SUBAGENT_ACTIONS` (the const at `:215`). **Position:** upstream `shared/types.ts:2801` @v0.68.0
   — the executor must place it at pi's own index, exactly as the `schedule.*` and `mission.*`
   entries in that slice document doing, and record the verified index in the comment.
   `DESTRUCTIVE_MANAGEMENT_ACTIONS` at `:311` already carries `worktree.discard` but **not**
   `worktree.cleanup`, and upstream's list does not either (`subagent-executor.ts:168`) — leave it
   out; plan-only mode removes nothing.

2. **`crates/cyrup-ext-subagents/src/extension/tool/routing.rs`** — a new `"worktree.cleanup"` arm in
   `route_action` (the `match` at `:1084`), placed like the `schedule.*` arm. It must, in order:
   * refuse when `!self.allow_mutating_management`, with the crate's existing sentence
     `"Action 'worktree.cleanup' is not available from child-safe subagent fanout mode."`
     (pi `:6215`, and the identical wording the `mission.*`/`schedule.*` arms already emit);
   * refuse `mode != Some("plan")` with pi's verbatim `:6218` sentence;
   * refuse `plan_id.is_some()` with pi's verbatim `:6221` sentence;
   * resolve `repo` against `effective_cwd` when relative, else `effective_cwd` (pi `:6225-6227`);
   * resolve `handoff_path` the same way (pi `:6228`);
   * pass `config_snapshot().await.worktree_base_dir` (`registration/mod.rs:304`) (pi `:6229`);
   * pass the foreground-ownership closure (§4.7);
   * render with `format_worktree_cleanup_plan` and return
     `details: json!({"mode":"management","results":[]})` — the shape every other management arm uses.
   **Not** behind `AuthorityAction` — `for_tool_action` (`registration/authority.rs:64-74`) must keep
   returning `None` for it. pi gates `worktree.discard` on `discardWorktree` (`:6249`) and gates
   `worktree.cleanup` on **nothing**, because plan-only mode mutates nothing but one plan file. The
   `authority.rs:20` note is discharged by the SIBLING's `worktree.discard`, not by this area; this
   spec says so explicitly so the executor does not double-gate a read.

3. **`crates/cyrup-ext-subagents/src/extension/tool/params.rs`** — add `repo: Option<String>`
   and `plan_id: Option<String>` to `SubagentToolParams` (`:85`, `rename_all = "camelCase"` already
   maps `planId`), and **add both to `provided_keys()` (`:583`)**. `handoffPath` and `mode` already
   exist (`mode` at `:132`). Missing `provided_keys` entries fail
   `every_advertised_schema_property_is_read_outside_provided_keys` (`schema.rs:1093`).

4. **`crates/cyrup-ext-subagents/src/extension/tool/schema.rs`** — advertise `repo` (pi
   `schemas.ts:299`) and `planId` (pi `:300`, *"Reserved; cleanup is plan-only."*), and widen the
   existing `mode` enum at `:415` from `["steer","follow_up","auto"]` to upstream's five
   `["steer","follow_up","auto","plan","apply"]` with pi's `:313` description
   (*"…worktree.cleanup supports plan only, no apply/removal."*). Safe to widen: `SteerDeliveryMode::parse`
   (`background/control.rs:1416`) is a closed three-arm match returning `None` for anything else, so
   `steer` still refuses `plan`/`apply` at the boundary. `handoffPath` also needs advertising if the
   sibling has not already added it (`grep '"handoffPath"' schema.rs` → 0 today — coordinate).
   The `action` enum is DERIVED from `SUBAGENT_ACTIONS` (`:357`), so edit 1 feeds it automatically.

5. **`crates/cyrup-ext-subagents/resources/docs/tool-reference.md`** — must name
   `worktree.cleanup`, or `the_tool_reference_topic_names_every_dispatched_verb`
   (`registration/guide.rs:215`) goes red.

**RPC bridge (`extension/rpc/`), decided deliberately, as the seed asks:** `worktree.cleanup` SHOULD
route through `manage` alongside `schedule.*`. It is a read-only, repo-scoped diagnostic whose whole
output is text, it takes no session identity, and a delegating host that can fan out into worktrees
through the bridge has no other way to see what it left behind. If the bridge dispatches by
forwarding to `route_action`, edit 2 gives it the verb for free and this reduces to confirming the
bridge's own action allow-list (if it has one) carries the name.

### 6. THE ON-DISK JSON — pinned, and who reads it

**Plan file:** `<repoRoot>/.cyrup-subagents/cleanup-plans/<planId>.json` (pi `:822`:
`<repoRoot>/<PROJECT_SUBAGENTS_RELATIVE_DIR>/cleanup-plans/<planId>.json`). cyrup's project root is
`.cyrup-subagents` via `artifacts::project_subagents_dir` (`artifacts.rs:155`) — an established
crate-wide delta, not a new one. Written with
`background::atomic::write_atomic_json_creating_parent` (the directory will not exist).

```jsonc
{
  "version": 1,
  "planId": "…",                 // ^[A-Za-z0-9._-]+$, never "." or ".."
  "repoRoot": "/abs/path",       // realpath of `git rev-parse --show-toplevel`
  "createdAt": 1750000000000,    // epoch ms
  "expiresAt": 1750001800000,    // createdAt + 1_800_000  (WORKTREE_CLEANUP_PLAN_TTL_MS)
  "baseDirs": ["/abs/base"],     // always exactly one element (pi `:804`)
  "metadataPaths": ["/abs/handoff.json", …],   // sorted by comparable path
  "entries": [ {
      "path": "/abs/worktree",
      "branch": "cyrup-parallel-run-0",        // "" for a detached/unknown git entry
      "decision": "remove" | "keep" | "unknown",
      "state": "safe" | "ineligible" | "stale" | "dirty" | "active" | "unknown",
      "reasons": ["…"],
      "source": "git" | "metadata" | "both",
      "willDeleteBranch": true,                // ONLY on a removable entry
      "runId": "…", "handoffPath": "/abs/handoff.json", "taskIndex": 0,
      "baseCommit": "<sha>", "patchPath": "/abs/x.patch", "targetRef": "<sha>",
      "preconditions": {
        "path": "/abs/worktree", "branch": "…",
        "worktreeHead": "<sha>", "branchTip": "<sha>", "baseCommit": "<sha>",
        "statusDigest": "<sha256 hex of the porcelain output>",
        "recordedBaseDir": "/abs",             // dirname(entry.path) — pi `:529`
        "targetRef": "<sha>"
      }
  } ],
  "pruneCandidates": ["/abs/worktree"],
  "warnings": ["…"],                            // key OMITTED when empty (pi `:808`)
  "contentHash": "<sha256 hex>"
}
```

JSON field names are upstream's, verbatim; Rust identifiers are snake_case under
`#[serde(rename_all = "camelCase")]`. `willDeleteBranch`, `runId`, `handoffPath`, `taskIndex`,
`baseCommit`, `patchPath`, `patchPath`, `warnings` and every `preconditions` field except `path`,
`branch` and `recordedBaseDir` are `skip_serializing_if = "Option::is_none"`, matching pi's
conditional-spread construction (`:531-550`).

`contentHash` = sha-256 of the canonical JSON of `{version, repoRoot, baseDirs, metadataPaths,
entries, pruneCandidates, warnings?}` — i.e. the plan **minus** `planId`/`createdAt`/`expiresAt`/
`contentHash` (pi `contentPayload:726-736`). Declaration order must match that key order; the
workspace's `serde_json` carries `preserve_order` (root `Cargo.toml:181`), so struct order is wire
order. This is an *internal* recomputability guarantee for a future apply phase, **not** a
byte-parity claim against pi — say so in the doc comment rather than implying parity.

**Manifest files this READS (the interface with the sibling area), unchanged:**
`<run_dir>/handoff.json`, `<repoRoot>/.cyrup-subagents/artifacts/handoff.json`, and
`<repoRoot>/.cyrup-subagents/artifacts/handoffs/*.json` (pi `discoverHandoffPaths:328-365`;
`parallelHandoffPath` at `parallel-handoff.ts:614-615`). **Already read today by
`background/async_retention/scan.rs:430` `has_unresolved_run_handoff`**, which parses
`version == 1`, `groups[]`, and `groups[].cleanup.state != "complete"` — so the manifest shape is
already load-bearing in production on this side, and this area must not change it.

**Beware the default `reason`.** `parallel-handoff.ts:583` writes every freshly-preserved task with
`reason: "cleanup pending durable handoff capture"`, and gate #21 (`:638`) makes exactly that string
`Ineligible/Keep`. A plan built immediately after a group is written therefore has **zero** removable
entries by design. The executor must not read that as a broken implementation, and the reachability
test in §8 must set a different reason to exercise the Safe path.

### 7. THE REACHABILITY TEST

**Location:** a new `#[cfg(test)]` module `crates/cyrup-ext-subagents/src/extension/tool/worktree_cleanup_tests.rs`,
wired from `extension/tool/mod.rs` the way `scheduled_runs_tests.rs` is. In-crate, **not** `cyrup-it`:
`cyrup-it` is `required-features = ["it"]` and off the `cargo test --workspace` merge gate
(`crates/cyrup-it/Cargo.toml`, "THE GATE"), so a reachability test that lives only there does not
actually gate anything. `scheduled_runs_tests.rs:1-7` is the precedent and states the reasoning
("A `schedule.create` that works only when called from Rust is a database, not a feature").

**Production entry point driven:** `SubagentTool::execute(…)` — i.e. `cyrup_core::Tool::execute`,
the same seam both model-facing registrations call — with raw JSON params, exactly as
`scheduled_runs_tests.rs:82-89`'s `dispatch` helper does. **No module function is called directly.**

**The rows:**

1. `worktree_cleanup_is_advertised_and_dispatches` — `{"action":"worktree.cleanup","mode":"plan"}`;
   assert the reply does not contain "unknown subagent action". This is the advertise-vs-dispatch
   invariant for this verb (copied from `every_scheduled_run_action_dispatches`).
2. `a_worktree_with_uncommitted_work_is_never_proposed_for_removal` — **the load-bearing row.**
   Fixture, all real: `git init` a tempdir repo, one commit; `git worktree add -b lane-0 <base>/cyrup-worktree-t-0`;
   write `<worktree>/dirty.txt` (untracked, so `--untracked-files=all` sees it); write a manifest at
   `<repo>/.cyrup-subagents/artifacts/handoff.json` with `version:1`, `source:"foreground"`,
   `runId`, one group (`repoRoot`, `baseCommit` = the commit, `cleanup.state:"partial"`, one child
   with a terminal status and `taskIndex:0`, one cleanup task `{index:0, path:<worktree>, branch:"lane-0",
   worktreeRemoved:false, branchRemoved:false, preserved:true, reason:"ready"}`); point
   `CYRUP_SUBAGENTS_WORKTREE_DIR` at `<base>`. Dispatch. Assert: the rendered text lists the
   worktree under **"Will keep, with reasons"** with *"worktree has uncommitted or untracked changes"*,
   and **not** under "Will remove"; and the persisted
   `<repo>/.cyrup-subagents/cleanup-plans/<planId>.json` has that entry
   `state == "dirty" && decision == "keep"` and **no `willDeleteBranch` key**.
3. `a_clean_preserved_worktree_is_proposed_for_removal` — identical fixture minus the untracked file.
   Assert `state == "safe" && decision == "remove"` and that the plan file exists and round-trips
   through `serde_json::from_str::<WorktreeCleanupPlan>` (which exercises `deny_unknown_fields` and
   `PlanId`'s re-validating `Deserialize`).
4. `an_active_owning_run_keeps_its_worktree` — same fixture, plus `<manifest dir>/status.json` with
   `state:"running"` and the matching `runId`. Assert `state == "active" && decision == "keep"`.
5. `apply_mode_and_plan_id_are_refused` — `{"action":"worktree.cleanup","mode":"apply"}` and
   `{"action":"worktree.cleanup","mode":"plan","planId":"x"}`; assert pi's two verbatim sentences.
6. `child_safe_fanout_refuses_worktree_cleanup` — a tool built with `allow_mutating_management = false`.
7. Pure unit rows in `gitwt.rs` for `parse_git_worktree_list`: detached clears the branch, `prunable`
   is captured, a `locked` line is ignored, a trailing record without a blank line still flushes.

**Why row 2 fails if the implementation is gutted.** It is negative *and* positive on the same run:
it asserts the entry appears (so deleting the classifier, returning an empty plan, or refusing the
verb fails it) **and** that its verdict is `dirty/keep` (so returning `remove` for everything, or
dropping the `git status` gate at `:669`, fails it). Returning a canned string fails the plan-file
assertion; writing a plan with no entries fails the presence assertion. There is no way to satisfy
it without a real `git worktree list --porcelain` parse, a real manifest join, a real
`git status --porcelain=v1 --untracked-files=all`, and a real state→decision mapping — and it
reaches all of that only through `Tool::execute`, so an unwired module fails it at the first assert.

### 8. SAFETY INVARIANTS — what must NEVER happen

1. **A worktree with uncommitted or untracked changes must never be `Remove`.** Gate #33 (`:669`),
   `git status --porcelain=v1 --untracked-files=all`. `--untracked-files=all` is not optional: with
   the default `normal`, an untracked directory is summarized and a worktree holding only new files
   inside a new directory could read clean.
2. **Absence of proof is never proof of safety.** Every unreadable/ambiguous condition is
   `Unknown/Unknown` or `Ineligible/Keep`, never `Safe/Remove`. `Verdict` has no fallible
   `Default`; the classifier's only path to `Safe` is falling off the end of all 41 gates.
3. **Building a plan must never mutate anything but the plan file.** The only writes are the plan
   and, inside `is_patch_captured`, a temp `GIT_INDEX_FILE` (`worktree.ts:307-311`) — never the
   worktree's real index, never `git add` into it, never `git worktree remove`, never `git branch -D`,
   never `git worktree prune`. `pruneCandidates` is a REPORT (`:862`). `cyrup-ext-subagents`'s
   existing `spawn::worktree::cleanup_worktrees` (`worktree.rs:1045-1060`) is exactly the
   force-removing primitive this module must not call, and the plan module must not import it.
   **cyrup's `run_git` takes no env (`worktree.rs:195-208`) — a `run_git_env` variant is required
   so the patch re-capture can set `GIT_INDEX_FILE`.** Without it a transliteration would
   `git add -A` into the live worktree index, which is a mutation during a *planning* operation.
4. **The plan id can never escape its directory.** `PlanId` newtype + `worktree_cleanup_plan_path`
   taking `&PlanId` (§4.2). A `../../etc/passwd` plan id must not typecheck.
5. **A worktree outside the managed base dir is never removable** (#13), nor is the repository root
   (#14), nor anything inside `<agent_dir>/extensions` (#12), nor a symlink (#7, #10). Symlinks are
   rejected rather than followed because a removal would otherwise act through an attacker-chosen link.
6. **Never remove a worktree whose branch is checked out elsewhere** (#23) or at the repo root (#24).
7. **Never remove a worktree whose owning run is not provably terminal** — active status (#26),
   any non-terminal child (`inspectRunState`), a fresh active marker, or a foreground run whose
   ownership probe answers anything other than `terminal` (#25). `foreground_ownership` is required,
   not `Option`, so "no probe supplied" cannot silently read as "terminal".
8. **Never destroy unmerged work.** #36/#37: committed divergence from base is removable only if a
   *validated* durable patch preserves it, or the branch tip is already an ancestor of the target
   HEAD. `willDeleteBranch` is `branchTipIsAncestor` — so an un-merged branch survives even when its
   worktree is proposed for removal (`:709`'s second reason line says so to the operator).
9. **Never remove a worktree that contains its own durable evidence** — the manifest, the output,
   the structured output, the session transcript or the patch (#38, #41). Removing it would delete
   the record of what was lost.
10. **Never propose removal against a different repository** (#22) or a metadata/git path mismatch (#4).
11. **A stale plan must not be executed.** No executor exists (§1), so the invariant is discharged
    structurally: `mode='apply'` and `planId` are REFUSED at the dispatch boundary, with pi's own
    sentences. The refusals are part of the deliverable precisely because they are the guard.
12. **The plan must not leak the user's untracked filenames.** `statusDigest` is a sha-256 of the
    porcelain output, never the output (`:561`, `:668`). The plan file sits inside the repo.
13. `formatWorktreeCleanupPlan` must keep emitting *"Plan-only mode: no worktrees or branches were
    removed."* (`:866`) and *"none (plan-only mode; no branch will be deleted)"* (`:855`). A human
    reading the output must not believe anything was deleted.

### 9. Blockers / dependencies / open coordination

* **Hard dependency on the sibling (parallel-handoff) area** for the manifest mirror types
  (`ParallelHandoffManifest/Group/CleanupTask/Child`, `types.ts:427-521`) and for
  `isTerminalParallelHandoffChildStatus` (`parallel-handoff.ts:295-296`:
  `completed|complete|failed|paused|stopped|rejected`, note `pending|running|detached` are NOT
  terminal). If the sibling does not land them, this area must define them and the two will collide
  on the same file. **Proposed split: the sibling owns `spawn/parallel_handoff/model.rs`; this area
  imports it and defines nothing manifest-shaped.**
* **Hard dependency on the sibling for anything removable to ever exist.** Until a writer produces a
  manifest with `cleanup.state == "partial"` and `preserved: true`, every plan is empty in
  production. The test fixture writes one by hand, so this area is testable independently, but the
  *feature* is only useful once the sibling's `writeParallelHandoffGroup` lands. State this in the
  PR rather than letting it look like an inert module.
* **`run_git` needs an env-carrying variant** (invariant 3). That edits `spawn/worktree.rs`, which
  the sibling may also be editing — sequence this area's `worktree.rs` edits after theirs.
* `MACHINE_DIFF_OPTIONS` includes `--default-prefix`, which requires **git ≥ 2.41**. cyrup's existing
  `capture_worktree_diff` (`worktree.rs:965-978`) uses plain `git diff --cached <base>` with none of
  these options, so a byte-equality check between a cyrup-captured patch and a machine-options
  re-capture would **always** fail → `Ineligible/Keep`. Fail-safe, but it makes gate #36's
  patch-preservation path dead. **Recommendation:** the re-capture in `is_patch_captured` must use
  the identical argv to `capture_worktree_diff` (adding only the temp `GIT_INDEX_FILE`), and the
  create side should adopt `MACHINE_DIFF_OPTIONS` in a follow-up so both sides are locale- and
  config-independent. Note also that a cyrup-captured patch is taken from a `git add -A`-staged
  index, and such a worktree is `Dirty/Keep` at gate #33 before #36 is ever reached — so #36 is
  reachable today only for a worktree whose divergence is COMMITTED. Port it anyway: its failure
  mode is Keep.
* `sha2 = "0.11.0"` (crate manifest `:130`) and `uuid` with `v4` (`:112`) are already dependencies;
  no new dependency is needed.
* `randomUUID()` → `uuid::Uuid::new_v4()`. Not v7: pi's ids are opaque and a v7 id would leak the
  plan's creation time into a filename that is already timestamped by `createdAt` inside.

### 10. Files this area expects to touch (for sequencing against the two siblings)

**New:**
* `crates/cyrup-ext-subagents/src/spawn/cleanup_plan/{mod,model,verdict,gitwt,metadata,paths,classify,render}.rs`
* `crates/cyrup-ext-subagents/src/extension/tool/worktree_cleanup_tests.rs`

**Modified (collision risk noted):**
* `crates/cyrup-ext-subagents/src/spawn/mod.rs` — declare the new module. *(sibling may also edit)*
* `crates/cyrup-ext-subagents/src/spawn/worktree.rs` — promote `run_git`/`run_git_checked`/`GitResult`/
  `resolve_worktree_base_dir`/`normalize_comparable_cwd`/`lexical_normalize` to `pub(crate)`; add
  `run_git_env`; add `MACHINE_DIFF_OPTIONS` + `validate_worktree_patch_represents_current_worktree`.
  ***(high collision risk — sequence after the sibling)*
* `crates/cyrup-ext-subagents/src/error.rs` — one `SubagentError` variant.
* `crates/cyrup-ext-subagents/src/extension/tool/text.rs` — `SUBAGENT_ACTIONS` += `worktree.cleanup`.
  **(all three areas will edit this const — coordinate one ordered insert per verb)**
* `crates/cyrup-ext-subagents/src/extension/tool/routing.rs` — the `worktree.cleanup` arm. *(sibling adds `worktree.discard` + `lane.*`)*
* `crates/cyrup-ext-subagents/src/extension/tool/params.rs` — `repo`, `planId` + `provided_keys`. *(sibling adds `laneId`, `merge`, `supersession`, `lane`)*
* `crates/cyrup-ext-subagents/src/extension/tool/schema.rs` — `repo`, `planId`, widen `mode`; possibly `handoffPath`. *(shared with sibling)*
* `crates/cyrup-ext-subagents/src/extension/tool/mod.rs` — `mod worktree_cleanup_tests;`.
* `crates/cyrup-ext-subagents/resources/docs/tool-reference.md` — document the verb. *(shared)*
* `crates/cyrup-ext-subagents/src/extension/rpc/` — allow-list `worktree.cleanup` for `manage`, if
  that surface has an explicit list.

### 11. Addendum — reconciliation with `## [AUG — manifest]` (appended after reading it)

The manifest sibling's section landed while this one was being written. Two of my statements above
need correcting, and I am correcting them here rather than rewriting the section:

1. **§5's RPC-bridge recommendation was WRONG and is withdrawn.** I recommended routing
   `worktree.cleanup` through the bridge's `manage`. I have now checked the premise the sibling
   cites, and it holds: `extension/rpc/mod.rs:120` `SUBAGENT_RPC_MANAGEMENT_ACTIONS` is a closed
   seven-entry list, and upstream `rpc.ts:65-73` @v0.68.0 is the identical seven `schedule.*` verbs
   with nothing else in it. Widening it for `worktree.cleanup` would be **inventing** a surface,
   not porting one. **Decision: do NOT widen the bridge.** Same conclusion as the sibling, same
   reason, and — per the seed's request for a deliberate decision either way — the reason is
   upstream parity, not an oversight. No `[CYRUP-DELTA]` is needed for matching upstream.

2. **§9's proposed module path for the manifest types is superseded.** I proposed
   `spawn/parallel_handoff/model.rs`; the sibling has chosen a **top-level `handoff/`** module, with
   the (correct) argument that `background::async_retention` and the foreground path both read these
   types and neither should have to import from `spawn/`. This area therefore imports
   `crate::handoff::model::{Manifest, Group, Child, CleanupTask}` and
   `crate::handoff::is_terminal_child_status`, and **defines nothing manifest-shaped of its own.**
   `spawn/cleanup_plan/` (§4) stays where it is: it is the only consumer of `spawn/worktree.rs`'s
   private git helpers, and nothing outside the tool dispatch imports it.

Everything else in §1-§10 stands, including the two collision warnings in §10
(`spawn/worktree.rs` and `extension/tool/text.rs`'s `SUBAGENT_ACTIONS`), which the sibling's own
file list confirms — their §10 names `spawn/worktree.rs` too.

## [AUG — surface]

Area: **lane metadata, the tool params, and the dispatch** — upstream `src/runs/shared/lane-metadata.ts`
(126 lines @v0.68.0, read in full), `src/extension/schemas.ts` (the six unadvertised params), and
`src/runs/foreground/subagent-executor.ts:6213-6293` (the three dispatch blocks, read in full).
Research only; no source file touched.

### 0. Anchor re-verification — and ONE anchor that is badly stale

Upstream, via `git -C /home/user/cyrup/tmp/pi-subagents show v0.68.0:<path>`:

| anchor | verdict |
|---|---|
| `lane-metadata.ts` 126 lines; bounds `:3-11`; key pattern `:13`; `normalizeWorkflowLaneMetadata:49` | **all exact** (`:3-8` are the six lane bounds, `:9-11` the three `WORKTREE_STATUS_*` ones; `WORKFLOW_LANE_MODES` set is `:14`) |
| `schemas.ts` `handoffPath:298`, `laneId:301`, `merge:302`, `supersession:303`, `lane:355`, `lanes:147` | **all exact.** Also pinned, because this area needs them and the seed does not name them: `repo:299`, `planId:300`, `mode:313` (the enum that must grow `plan`/`apply`), `preflight:350`, `WorkflowLaneMetadata` TypeBox at `:103-110`, `WorkflowPreflightLane` at `:135-142`, `WorkflowPreflightOverride` at `:144-148` |
| `subagent-executor.ts:6270-6290` the `lane.*` arm | **exact.** Full three-block span is `:6213-6293`: `worktree.cleanup` `:6213-6241`, `worktree.discard` `:6242-6269`, `lane.*` `:6270-6293` |
| `shared/types.ts:2801` `SUBAGENT_ACTIONS` = 57 | **exact**, 57 counted |
| cyrup `extension/tool/text.rs:215` `SUBAGENT_ACTIONS`, 42 entries | **exact** — `:215` is the `pub(crate) const`; 42 counted |
| cyrup `extension/tool/text.rs:323` `"worktree.discard"` in `DESTRUCTIVE_MANAGEMENT_ACTIONS` | **exact** |
| cyrup `registration/authority.rs:20` *"Whoever lands `worktree.discard` or `destructiveCleanup` must wire them through"* | **exact** (`:20-22`) — but **half of it is unlandable; see §5.2** |
| `grep '"handoffPath"' …/extension/tool/schema.rs` returns 0 | **verified 0.** Also 0 for `laneId`, `"merge"`, `"supersession"`, `"lane"`, `"repo"`, `"planId"`, `"preflight"`, `"isolation"` — in **both** `schema.rs` and `params.rs`. Nine properties, zero advertised. |

**STALE ANCHOR — and it is the biggest single finding in this area.**
`## [AUG — manifest]` §1 states, under *"What cyrup does NOT have (proven by grep)"*:

> *"No `WorkflowLaneMetadata`, no lane mode, no lane key/claims/outputPaths. The 908 `lane` hits are
> all `exec/tool_surface.rs`'s review/scout agent lane vocabulary — a different concept entirely."*

**That is wrong, and building on it would duplicate ~250 lines of landed, tested code.**
`lane-metadata.ts:1-74` — the entire lane half of my first upstream file — **is already ported,
completely, and is already reachable from production**:

- `crates/cyrup-ext-subagents/src/workflows/lane_metadata.rs` (431 lines) —
  `normalize_workflow_lane_metadata:127` (pi `:49-70`), `assert_workflow_lane_key:224` (pi `:71-74`),
  all six bound constants `:14-24` (pi `:3-8`), the field allow-list `LANE_METADATA_FIELDS:29`
  (pi's `assertKnownFields` call, `:48`), `lane_bounded_string:66` (pi `boundedNonEmptyString:20-26`),
  `lane_bounded_string_array:87` (pi `boundedStringArray:36-44`), `parse_lane_mode:106`,
  `has_forbidden_control_character:60` (pi's `\n`/`\r`/NUL rule, `:23`).
- `crates/cyrup-ext-subagents/src/workflows/types.rs:557-584` — `WorkflowLaneMetadata`, already
  `#[serde(rename_all = "camelCase", deny_unknown_fields)]`, already `key: WorkflowKey`, already
  `Bounded<128>/<160>/<256>` for `source_ref`/`claims`/`output_paths`.
- `workflows/types.rs:540-548` — `WorkflowLaneMode { Mutation, Review, Scout, Gate }`, `camelCase`.
- `workflows/types.rs:504-532` — `LaneMetadataVersion`, a unit type that serializes `1` and refuses
  anything else.
- **Production caller:** `extension/executor/workflow.rs:754`
  `let lane = Self::parse_child_lane(key, params.get("lane"))?;` inside
  `impl WorkflowScriptHost for WorkflowRunHost::launch`, via `parse_child_lane:372`, which calls
  `normalize_workflow_lane_metadata` **and then** `assert_workflow_lane_key`. A `runs.run(key, {lane})`
  from a workflow script already validates lane metadata before spawn.
- It is also already on the wire twice: `workflows/types.rs:292` and `:343`
  (`lane: Option<WorkflowLaneMetadata>`).

**Consequence for the executor: do NOT create `handoff::lane`.** The manifest sibling's §3 module
layout lists `handoff/lane.rs  LaneMetadata, LaneMode, LaneClaim, LaneOutputPath, LaneSourceRef`
— that file must not be written. `handoff::model::Child::lane` and `LaneBinding::lane` are
`Option<crate::workflows::WorkflowLaneMetadata>`, imported, not redeclared. Its serde shape is
already byte-identical to the manifest's requirement (camelCase, absent-not-null optionals,
`deny_unknown_fields`, `version: 1`). The manifest sibling's own §3.2 table row *"lane `key` →
**reuse `crate::workflows::WorkflowKey`**"* is right; it just did not notice the whole record
above the key was reusable too.

Also already landed, and relevant because the seed calls it out as missing: **`lanes:147`
(the display-only preflight hints, max 64) is ported in full** —
`crates/cyrup-ext-subagents/src/workflows/preflight.rs` (1306 lines), with
`WORKFLOW_PREFLIGHT_MAX_LANES = 64` at `:30` (pi `:4`), `WORKFLOW_PREFLIGHT_MAX_CLAIMS = 16` at `:36`
(pi `:6`), `LANE_FIELDS:55-62` matching `WorkflowPreflightLane`'s six keys exactly, and
`WorkflowPreflightLane`/`WorkflowPreflightMode`/`WorkflowPreflightCoverage`/`PreflightVersion` at
`workflows/types.rs:387-502`. What is missing is **only the tool-surface `preflight` property**
(`schemas.ts:350`) — the normalizer has no tool-param caller. See §5.6; it is cheap and it is in
this area's blast radius, but it is a genuinely separable decision.

**What of `lane-metadata.ts` is genuinely NOT ported:** `:76-126` — the worktree half.
`WORKTREE_STATUS_PATH_MAX_BYTES=4096`/`WORKTREE_STATUS_BRANCH_MAX_BYTES=256`/
`WORKTREE_STATUS_NAMING_LABEL_MAX_BYTES=256` (`:9-11`), `WorktreeStatusReference` (`:76-81`),
`normalizeWorktreeNaming` (`:83-96`), `normalizeWorktreeStatusReference` (`:99-110`),
`validateAsyncStatusLaneMetadata` (`:113-126`). `lane_metadata.rs`'s own module doc says so, at
`:1-3`: *"The worktree-reference half (`:99-126`, `normalizeWorktreeStatusReference`/
`validateAsyncStatusLaneMetadata`) belongs to a different family and has no consumer here."*
**It has a consumer now** — see §2.3.

### 1. The three upstream dispatch blocks, quoted

`subagent-executor.ts` @v0.68.0. These are the behaviour this area ports.

**`worktree.cleanup` — `:6213-6241`.**
```ts
6213  if (action === "worktree.cleanup") {
6214    if (deps.allowMutatingManagementActions === false) {
6215      return { … "Action 'worktree.cleanup' is not available from child-safe subagent fanout mode." … isError: true …};
6216    }
6217    if (paramsWithResolvedCwd.mode !== "plan") {
6218      return { … "worktree.cleanup currently supports mode='plan' only; apply/removal is not available yet." … isError: true …};
6219    }
6220    if (paramsWithResolvedCwd.planId !== undefined) {
6221      return { … "worktree.cleanup plan mode does not accept planId; apply is not available yet." … isError: true …};
6222    }
6224    const created = createWorktreeCleanupPlan({
6225-27    repo: <params.repo trimmed ? (absolute ? repo : resolve(requestCwd, repo)) : requestCwd>,
6228      ...(params.handoffPath ? { handoffPath: <same absolute-or-resolve> } : {}),
6229      ...(deps.config.worktreeBaseDir ? { worktreeBaseDir: deps.config.worktreeBaseDir } : {}),
6230-35  foregroundRunOwnership: (runId) => { … "active" | "unknown" | "terminal" … },
6236    });
6237    return { content:[{type:"text", text: formatWorktreeCleanupPlan(created)}], details:{ mode:"management", results: [] } };
6238-40 catch (error) → text = error.message, isError: true
6241  }
```
Note the **order**: child-safe gate FIRST, then mode, then planId, then the build. And note there is
**NO `resolveAuthorityDecision` call in this block** — this is load-bearing for §5.2.

**`worktree.discard` — `:6242-6269`.**
```ts
6243  if (deps.allowMutatingManagementActions === false) → child-safe refusal
6246  if (!params.handoffPath?.trim()) → "worktree.discard requires handoffPath from parallelHandoff.path or async status."
6249  const decision = resolveAuthorityDecision({ action: "discardWorktree", ...(config.authorityPolicy ? {policy} : {}) });
6250  if (decision === "forbid") → "Authority policy forbids worktree discard."
6253  let confirmed = decision === "auto";
6254  if (decision === "confirm") {
6255    if (!ctx.hasUI) → "Authority policy requires user confirmation for worktree discard, but this
                          session has no interactive UI. Preserved worktrees were not changed."
6256    confirmed = await ctx.ui.confirm("Discard preserved subagent worktrees?",
              `This permanently removes preserved worktrees and temporary branches recorded in:\n${handoffPath}`);
6257  }
6258  if (!confirmed) → "Worktree discard canceled; preserved worktrees were not changed."  (NOT isError)
6260  const manifestPath = <absolute-or-resolve against requestCwd>;
6261  const discarded = await withWorktreeTransaction(() => discardPreservedWorktrees(
6263      manifestPath, { kind: decision === "confirm" ? "confirmed" : "policy", ...(policy ? {policy} : {}) }));
6265  return { content:[{text: discarded.text}], details:{ mode:"management", results: [] } };
```
Three things a transliteration loses. (a) The authority messages here are **verb-specific prose**
("Discard preserved subagent worktrees?", "…Preserved worktrees were not changed."), NOT the generic
`Authority policy forbids action '<x>'.` the `stop`/`steer`/`schedule.create` path uses — cyrup's
`registration::authority::{forbidden_message, no_ui_message, confirm_prompt, confirm_message,
declined_message}` (`authority.rs:196,202,211,217,225`) are the **generic** family and are the wrong
strings for this verb. (b) The `DiscardAuthorization` passed down carries `kind: "confirmed" | "policy"`
— *how* the discard was authorized is recorded into the manifest, so it is data, not control flow.
(c) `withWorktreeTransaction` (`worktree.ts:207-216`) serializes the discard against any in-flight
worktree setup; **cyrup has no equivalent** — `grep -rn 'with_worktree_transaction|worktree_transaction|
WORKTREE_MUTEX'` over `crates/**/*.rs` is **0**, and `spawn/worktree.rs` has **no `static`, no `Mutex`,
no `Semaphore`, no `OnceLock`** at all. See §7.4.

**`lane.status` / `lane.recordMerge` / `lane.recordSupersession` — `:6270-6293`.**
```ts
6270  if (action === "lane.status" || action === "lane.recordMerge" || action === "lane.recordSupersession") {
6271    if (action !== "lane.status" && deps.allowMutatingManagementActions === false) → child-safe refusal
6274    const laneId = params.laneId?.trim();
6275    if (!laneId) → `${action} requires laneId.`
6276    const handoffPath = params.handoffPath?.trim();
6277    if (!handoffPath) → `${action} requires handoffPath for the existing parallel handoff manifest.`
6278    const manifestPath = <absolute-or-resolve against requestCwd>;
6280    if (action === "lane.status") {
6282      try { manifest = readParallelHandoffManifest(manifestPath); } catch { manifest = undefined; }
6283      if (manifest && manifest.runId !== laneId) throw `Lane '${laneId}' does not match manifest run '${manifest.runId}'.`
6284      return { content:[{text: formatStoredParallelHandoffCleanup(manifestPath, manifest)}], details:{mode:"management", results:[]} };
6285    }
6286-88 const recorded = action === "lane.recordMerge"
            ? recordParallelHandoffMerge({ manifestPath, laneId, merge: params.merge })
            : recordParallelHandoffSupersession({ manifestPath, laneId, supersession: params.supersession });
6289    return { content:[{text: recorded.text}], details:{ mode:"management", results: [], parallelHandoff: recorded.reference } };
6290-92 catch (error) → text = error.message, isError: true
```

**THE SPLIT THE TASK ASKS ME TO PRESERVE, stated precisely.** `:6271` is
`action !== "lane.status" && allowMutatingManagementActions === false`. So:
- `lane.status` is reachable from a **child-safe fanout tool**. It is a pure read.
- `lane.recordMerge` / `lane.recordSupersession` are **refused** there.
- All three are in the same `if` and share the laneId/handoffPath validation, so a child that calls
  `lane.recordMerge` with no `laneId` gets the child-safe refusal, not the missing-laneId one —
  the gate is checked **before** `laneId`. Order matters and must be ported in order.
- **None of the three consults the authority policy.** Only `worktree.discard` does.

**`lane.status` swallows read errors, `lane.record*` does not.** `:6282`'s bare `catch { manifest =
undefined; }` means an unreadable/corrupt manifest renders as "no manifest" prose, not an error;
`:6283`'s lane/run mismatch is thrown and becomes `isError: true`. In Rust that is
`read_manifest(..).unwrap_or(None)` for `lane.status` specifically — the one place in this feature
where discarding an error is correct, and it needs a comment saying it is upstream's, not a slip.

### 2. What cyrup ALREADY has (grepped hard — the reuse list)

**2.1 The lane record, entire.** §0 above. `crate::workflows::{WorkflowLaneMetadata, WorkflowLaneMode,
LaneMetadataVersion, normalize_workflow_lane_metadata, assert_workflow_lane_key, LaneMetadataError,
WORKFLOW_LANE_*}` — all `pub`, all re-exported from `workflows/mod.rs:109-113,161-162`.

**2.2 The preflight record, entire.** `workflows/preflight.rs` + `workflows/types.rs:387-502`.

**2.3 The worktree-status-reference half is MISSING and now has a consumer.**
`grep -rn 'ManagedWorktreeProvider|WorktreeNaming|worktrunk'` over `crates/**/*.rs` → **0 hits.**
`spawn/worktree.rs`'s `WorktreeInfo` carries no `provider` and no `naming`. This matters to this
area because the manifest's `cleanup.tasks[]` carries optional `provider` and `naming`
(`shared/types.ts:454-467`), and `lane.status`'s renderer prints them. Port
`lane-metadata.ts:9-11,76-110` — the three `WORKTREE_STATUS_*` bounds, `WorktreeNaming`,
`ManagedWorktreeProvider`, `normalizeWorktreeStatusReference` — **into the existing
`workflows/lane_metadata.rs`**, beside the half that is already there, and delete that file's
module-doc sentence claiming the half "has no consumer here". `validateAsyncStatusLaneMetadata`
(`:113-126`) stays unported: it validates `AsyncStatus.steps[].{lane,worktreePath,branch,provider,naming}`,
and `RunStatus`'s step record (`background/records.rs`) has none of those fields; porting the
validator with nothing to validate is the exact "landed with no caller" failure this programme keeps
shipping. Say that in the doc comment rather than leaving it to inference.

**2.4 The dispatch seam.** `extension/tool/routing.rs:1084` `route_action(&self, action: &str,
p: &SubagentToolParams, cwd: &Path)`. The `cwd` argument is **already the resolved request cwd**, so
pi's `path.isAbsolute(x) ? x : path.resolve(requestCwd, x)` is `if Path::new(x).is_absolute() { x.into() }
else { cwd.join(x) }` with no new plumbing.

**2.5 The exact arm to copy.** `routing.rs:1240-1320`, the `schedule.*` guard arm: matcher guard →
child-safe refusal (`!self.allow_mutating_management`) → authority consult via
`AuthorityAction::for_tool_action(action)` → `resolve_authority_decision` → three-arm
`Auto`/`Forbid`/`Confirm` with `self.executor.host_services()` and `services.confirm(prompt, message,
&DialogOptions::default())`, and **a decline returns `Ok`, not `Err`** (`:1295-1300`) — which is
exactly upstream `:6258`'s non-`isError` cancel. This arm is the template; do not invent a fourth
authority-gate shape.

**2.6 The authority module.** `registration/authority.rs`: `AuthorityAction` `:38-46` (already has
`DiscardWorktree` and `DestructiveCleanup` variants), `for_tool_action:72-80` (the mapping table to
extend), `default_decision:83-92` (`DiscardWorktree` → `Confirm`), `resolve_authority_decision:145`,
and the five generic message helpers `:196-225`.

**2.7 The child-safe flag.** `self.allow_mutating_management` on the routing struct, and
`crate::discovery::management::MUTATING_MANAGEMENT_ACTIONS` (`discovery/management/mod.rs:184`,
seven entries: create/update/delete/eject/disable/enable/reset). Note the generic gate at
`routing.rs:1630-1640` is applied only inside `route_management_action`; every other verb family
does its own explicit check. Ours does too (upstream does).

**2.8 `DESTRUCTIVE_MANAGEMENT_ACTIONS`** — `extension/tool/text.rs:313-326`, already carries
`worktree.discard` at `:323`. Its job is to make the did-you-mean suggester **stricter** for these
names; nothing to change, it simply starts applying to a verb that now exists.

**2.9 `details.parallelHandoff` ALREADY HAS TWO PRODUCTION READERS.** This is the reachability gift
of this area and nobody has named it:
- `missions/lifecycle.rs:431-442`, inside `artifacts_for_result` (`:379`), reads
  `outcome.details["parallelHandoff"]["path"]` and pushes a
  `MissionArtifact { kind: MissionArtifactKind::Manifest, .. }`.
- `missions/lifecycle.rs:893-903`, inside `pub fn sync_mission_from_async_completion` (`:836`), reads
  `map["parallelHandoff"]["path"]` from an async-completion event and does the same.

Both are production, non-`#[cfg(test)]`, and **both are dead today because nothing in cyrup ever
writes that key.** `grep -rn 'parallelHandoff|parallel_handoff' crates/` returns exactly four hits:
these two, plus the two lines of `async_retention/scan.rs:425-426`'s `[CYRUP-DELTA]` saying the
field does not exist. Emitting `details.parallelHandoff` from `lane.recordMerge`/
`lane.recordSupersession` (upstream `:6289`) revives a landed mission-artifact path with **zero**
changes to `missions/`. That is a second, independent reachability proof for this area — use it.

**2.10 THE STRUCTURAL REACHABILITY ENFORCER.** `extension/tool/schema.rs:1093`
`every_advertised_schema_property_is_read_outside_provided_keys`. It walks **every `.rs` file under
`src/extension/`**, excises every `fn provided_keys()` body, derives the advertised property set from
`subagent_tool_parameters()` itself, camelCase→snake_case's each name, and asserts a literal
`.<field>` read (not followed by an identifier char) survives somewhere. Its failure message:
*"ADVERTISED but never read outside provided_keys(), so a caller that sets one is silently ignored…
Do NOT satisfy this test by adding a mention to provided_keys()."*
**This means the schema properties this area adds CANNOT land unwired.** The moment `handoffPath`
is advertised, the build is red until `routing.rs` (or a sibling under `src/extension/`) contains
`p.handoff_path`. The standing bar is enforced by the compiler here, for free. Note the corollary:
the *read* must live under `src/extension/` — `params.rs:277` records that exact trap for the
`schedule.*` projection. A read that lives in `handoff/` or `spawn/` does **not** count.

**2.11 The action enum is derived, not hand-written.** `schema.rs:359` `"enum": SUBAGENT_ACTIONS`.
Appending to `text.rs`'s list advertises the verb automatically.

**2.12 The two other advertise-vs-dispatch guards** that will go red on the same edit:
`registration/guide.rs:215` `the_tool_reference_topic_names_every_dispatched_verb` (every
`SUBAGENT_ACTIONS` entry must appear in `resources/docs/tool-reference.md`), and
`schema.rs:845-900`, which asserts the action enum's **exact values AND order**. Both are budgeted
work, not surprises.

**2.13 `WorkflowKey`** — `workflows/key.rs:18`, `parse:30`, hand-written `Deserialize` `:73-78`.

**What cyrup does NOT have, proven:** no `handoffPath`/`laneId`/`merge`/`supersession`/`lane`/`repo`/
`planId`/`preflight`/`isolation` in `schema.rs` **or** `params.rs` (0/0, all nine); no `worktree.*`
or `lane.*` verb in `SUBAGENT_ACTIONS`; no `withWorktreeTransaction` analogue; no
`ManagedWorktreeProvider`/`WorktreeNaming`; `schema.rs:414-422`'s `mode` enum is
`["steer","follow_up","auto"]` — **three** of upstream's five (`schemas.ts:313` has
`["steer","follow_up","auto","plan","apply"]`).

### 3. The RUST design

#### 3.1 Module layout

**No new top-level module.** This area is edits to five existing files plus one new leaf:

```
extension/tool/text.rs        +5 SUBAGENT_ACTIONS entries (at pi's own indices)
extension/tool/schema.rs      +7 properties; widen the `mode` enum; update the two order assertions
extension/tool/params.rs      +6 fields on SubagentToolParams; +2 projection methods
extension/tool/routing.rs     +1 `worktree.*` arm, +1 `lane.*` arm on route_action
extension/tool/lane_actions.rs   NEW — the two arms' bodies, param projections, and their tests
registration/authority.rs     extend for_tool_action; CORRECT the :20-22 note (§5.2)
workflows/lane_metadata.rs    + the worktree-status-reference half (§2.3)
```

`lane_actions.rs` sits under `src/extension/tool/` **deliberately**: §2.10's guard only scans
`src/extension/`, so the `p.handoff_path` / `p.lane_id` / `p.merge` / `p.supersession` reads must
live there to count. Putting them in `handoff/` would leave all six properties reported as
advertised-but-unwired. This is the same placement rule `params.rs:277` already records for the
schedule projection, and it is the single highest-value constraint in this section.

#### 3.2 The action enum — one type, not five string comparisons

Upstream dispatches on raw strings in a 14-deep `if` chain. cyrup's own idiom is a `from_wire`
parse (`ScheduledRunAction::from_wire`, `routing.rs:1241`; `MissionAction`, `:1209`). Follow it:

```rust
/// The five convergence verbs — pi `subagent-executor.ts:6213,6242,6270` @v0.68.0.
/// ONE type instead of upstream's five string comparisons, so the three orthogonal properties
/// below are decided by a `match` the compiler checks for exhaustiveness rather than by five
/// independent `if`s that can disagree. `is_mutating` in particular IS the child-safe split
/// (`:6214`, `:6243`, `:6271`) and `lane.status` is its one `false` — a boolean field on a struct
/// could be set wrong; a match arm on a five-variant enum cannot be forgotten.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LaneAction {
    WorktreeCleanup,
    WorktreeDiscard,
    LaneStatus,
    LaneRecordMerge,
    LaneRecordSupersession,
}

impl LaneAction {
    pub(crate) fn from_wire(action: &str) -> Option<Self> { /* the five names */ }
    pub(crate) fn as_str(self) -> &'static str { /* round-trips from_wire */ }

    /// pi's child-safe gate. `lane.status` is a pure read (`:6271`'s `action !== "lane.status"`);
    /// every other verb writes the manifest or the filesystem.
    pub(crate) fn is_mutating(self) -> bool { !matches!(self, Self::LaneStatus) }

    /// pi consults the authority policy for `worktree.discard` ONLY (`:6249`). `worktree.cleanup`
    /// is plan-only and removes nothing (`:6217`), and the three `lane.*` verbs edit a JSON file.
    /// Verified: `git grep destructiveCleanup v0.68.0 -- src` returns exactly two hits, both in
    /// `policy/authority.ts` (`:3` the list, `:18` the default) — upstream declares that action
    /// and never consults it. See `registration/authority.rs`.
    pub(crate) fn authority_action(self) -> Option<AuthorityAction> {
        matches!(self, Self::WorktreeDiscard).then_some(AuthorityAction::DiscardWorktree)
    }

    /// Whether this verb requires `handoffPath`. `worktree.cleanup` takes it OPTIONALLY (`:6228`
    /// — a spread, so absent means "discover manifests under repo"); the other four require it
    /// (`:6246`, `:6277`).
    pub(crate) fn requires_handoff_path(self) -> bool { !matches!(self, Self::WorktreeCleanup) }
}
```

`from_wire` returning `Option` is what lets `route_action` add ONE guard arm
(`lane_action if LaneAction::from_wire(lane_action).is_some() => …`) exactly as the `schedule.*` arm
does, rather than five `|`-joined literals.

#### 3.3 The params — six new fields, typed as narrowly as the dispatch allows

`SubagentToolParams` (`params.rs:86`) is `#[serde(rename_all = "camelCase")]`, permissive, no
`deny_unknown_fields`. Its house rule, stated at `:131-135` and `:148-152`: **a value that upstream
rejects with an actionable sentence stays a raw `String`/`Value`, so the model gets the sentence and
not a serde error.** Apply it honestly, field by field — this is where "newtypes with fallible
constructors" meets a real constraint, and the answer differs per field:

```rust
/// pi `handoffPath` (`extension/schemas.ts:298`) — "Existing manifest for worktree/lane actions."
/// Raw `String`: upstream trims it and answers an empty one with a verb-specific sentence
/// (`:6246`, `:6277`), and it is resolved against the request cwd at `:6260`/`:6278`, so a
/// `PathBuf` here would lose the blank-vs-absent distinction the two messages depend on.
pub(crate) handoff_path: Option<String>,

/// pi `repo` (`:299`) — "worktree.cleanup repo; default cwd." Raw, same resolution at `:6225-6227`.
pub(crate) repo: Option<String>,

/// pi `planId` (`:300`) — "Reserved; cleanup is plan-only." Declared so a model that tries an
/// apply gets upstream's own `:6221` sentence instead of a schema rejection, exactly as `on` and
/// `timezone` are declared-and-refused (`params.rs:170-177`).
pub(crate) plan_id: Option<String>,

/// pi `laneId` (`:301`, `minLength:1, maxLength:128`) — "Exact manifest run id for lane actions."
/// Raw: upstream trims then answers a blank with `${action} requires laneId.` (`:6275`), and the
/// manifest comparison at `:6283` is a plain string equality against `manifest.runId`.
pub(crate) lane_id: Option<String>,

/// pi `merge` (`:302`) / `supersession` (`:303`) — `Type.Unsafe({type:"object",
/// additionalProperties:true})`. Raw `Value`, because `recordParallelHandoffMerge` owns the
/// validation and its ~12 rejections are the product surface (`parallel-handoff.ts:437-455`).
/// The TYPED shape is the handoff sibling's `MergeEvidence`/`SupersessionEvidence`; this field is
/// the untyped carrier that reaches its fallible constructor, which is where the newtypes live.
pub(crate) merge: Option<serde_json::Value>,
pub(crate) supersession: Option<serde_json::Value>,
```

**`lane` and `preflight` are the two that do NOT stay raw**, and that is the point of the directive:

```rust
/// pi `lane` (`:355` → the TypeBox at `:103-110`). Typed, not raw, because cyrup already has the
/// validated record and its normalizer — `crate::workflows::WorkflowLaneMetadata` +
/// `normalize_workflow_lane_metadata`. Carried as `Value` on the params struct and normalized at
/// the dispatch boundary (NOT by serde), for this struct's own stated reason: upstream's eight
/// lane rejections are sentences a model can act on and `LaneMetadataError` already carries them
/// verbatim, where a `serde::de::Error` would not.
pub(crate) lane: Option<serde_json::Value>,
```

so the boundary call is
`crate::workflows::normalize_workflow_lane_metadata(p.lane.as_ref(), "lane")?` — landed code, one
line, upstream's messages.

**`mode` needs no new field**, only a widened enum (§3.5) and a second reader. It is already
`Option<String>` at `params.rs:129` for exactly this reason.

**Two projection methods**, in `params.rs` beside `mission_action_params:259` and
`schedule_action_params:279`, and for the same documented reason (`params.rs:277`: the guard scans
only `src/extension/`):

```rust
pub(crate) fn lane_action_params(&self) -> LaneActionParams;      // handoff_path, lane_id, merge, supersession
pub(crate) fn cleanup_plan_params(&self) -> CleanupPlanParams;    // repo, handoff_path, plan_id, mode
```

#### 3.4 The refusal set — a typed enum, not `format!` at seven call sites

Upstream's seven refusals in these blocks are `throw`/early-return strings. They are a closed set,
they are the product surface, and three of them interpolate the action. Model them once:

```rust
/// Every non-`Err` refusal these five verbs can produce, with pi's exact wording. A typed enum
/// rather than seven `format!`s: `text.rs`'s parity-of-wording convention means these strings ARE
/// the contract, and five of the seven are shared by more than one verb — a `Display` impl in one
/// place is one place to get them right.
#[derive(Debug, thiserror::Error)]
pub(crate) enum LaneActionRefusal {
    #[error("Action '{0}' is not available from child-safe subagent fanout mode.")]
    ChildSafe(&'static str),                                    // :6215, :6244, :6272
    #[error("worktree.cleanup currently supports mode='plan' only; apply/removal is not available yet.")]
    CleanupPlanOnly,                                            // :6218
    #[error("worktree.cleanup plan mode does not accept planId; apply is not available yet.")]
    CleanupNoPlanId,                                            // :6221
    #[error("worktree.discard requires handoffPath from parallelHandoff.path or async status.")]
    DiscardRequiresHandoffPath,                                 // :6247
    #[error("{0} requires laneId.")]
    LaneRequiresLaneId(&'static str),                           // :6275
    #[error("{0} requires handoffPath for the existing parallel handoff manifest.")]
    LaneRequiresHandoffPath(&'static str),                      // :6277
    #[error("Authority policy forbids worktree discard.")]
    DiscardForbidden,                                           // :6251
    #[error("Authority policy requires user confirmation for worktree discard, but this session \
             has no interactive UI. Preserved worktrees were not changed.")]
    DiscardNoUi,                                                // :6255
}
```

`Lane '{lane}' does not match manifest run '{run}'.` (`:6283`) is **not** here — it is
`HandoffError::LaneRunMismatch`, owned by the manifest sibling (its §3.4 already lists it verbatim).
Do not declare it twice.

**Two of these are `Ok`, not `Err`, and getting it wrong is a real defect.** `:6258`'s
*"Worktree discard canceled; preserved worktrees were not changed."* has **no `isError`** — a user
declining a confirm is a choice. `routing.rs:1295-1300` already does exactly this for
`schedule.create`; copy it. So the decline message is a plain `ToolResult`, not a refusal variant.
Everything else in the enum above is `isError: true`.

#### 3.5 The `mode` enum widening — the one schema change with a behavioural edge

`schema.rs:414-422` advertises `["steer","follow_up","auto"]`. Upstream's `:313` is
`["steer","follow_up","auto","plan","apply"]`, described *"steer delivery mode; worktree.cleanup
supports plan only, no apply/removal."*

The trap: `mode` is read by **two** dispatches. `foreground_actions/steer.rs:92` passes it to
`control::SteerDeliveryMode::parse(raw)` and answers `None` with a sentence. Widening the advertised
enum means `subagent({action:"steer", mode:"plan"})` becomes schema-legal and lands on that
sentence. **That is upstream's own behaviour** — `schemas.ts:313` is one shared enum across both
verbs and pi's `steer` rejects `plan` at runtime identically — so it is a port, not a regression, and
the `SteerDeliveryMode::parse` refusal message is already the right answer. Say so in a comment; do
not "fix" it by splitting the property, which would diverge the wire shape.

`apply` is advertised **and refused** (`:6218`), for the same reason `planId`, `on` and `timezone`
are: a model that reaches for it gets the sentence *"apply/removal is not available yet"*, which
tells it the feature's status, where a schema rejection tells it nothing. The `[AUG — cleanup]`
sibling's §1 correction (upstream `worktree.cleanup` is plan-only at v0.68.0) is what makes this the
right call, and I independently confirm it from `:6217-6222` above.

#### 3.6 `SUBAGENT_ACTIONS` — the five entries, at pi's own indices

Upstream's 57-entry list (`shared/types.ts:2801`) reads
`… "mission.attach-run", "mission.close", "worktree.discard", "worktree.cleanup", "lane.status",
"lane.recordMerge", "lane.recordSupersession", "refine", …`.

So the block goes **immediately after `"mission.close"`**, which in cyrup's list (`text.rs:215`) is
followed today by `"watchdog.status"` — cyrup omits `refine*`/`inspector.*`/`project.*`, so the five
land contiguously between `mission.close` and `watchdog.status`. Same for the `action` enum
assertion at `schema.rs:845-900` (exact values AND order). The existing entries all carry a comment
naming pi's index; match that convention or the next reader will assume the position was guessed.

Count after: **47**, and the 17-verb gap becomes 12.

#### 3.7 What the arms actually call

Both arms are thin — the work is the siblings':

| arm | calls |
|---|---|
| `worktree.cleanup` | `spawn::cleanup_plan::create_worktree_cleanup_plan(CreatePlan{ repo, handoff_path, worktree_base_dir, foreground_run_ownership })` then `format_worktree_cleanup_plan(&plan)` — `[AUG — cleanup]` §4.8 |
| `worktree.discard` | `handoff::write::discard_preserved(&manifest_path, DiscardAuthorization{ kind, policy })` — `[AUG — manifest]` §3.6 |
| `lane.status` | `handoff::read::read_manifest(&p).unwrap_or(None)`, the run-id equality check, then `handoff::format::format_stored_cleanup(&p, m.as_ref())` |
| `lane.recordMerge` | `handoff::write::record_merge(RecordMerge{ manifest_path, lane_id, merge })` |
| `lane.recordSupersession` | `handoff::write::record_supersession(RecordSupersession{ … })` |

The `foregroundRunOwnership` closure (`:6230-6235`) reads `deps.state.foregroundControls` /
`deps.state.foregroundRuns`. In cyrup that is `self.executor`'s foreground registry; the sibling's
§4.7 already designs the injected trait/closure seam. **This arm is the only production site that
can supply it** — it is the sole holder of an executor reference on the path. Flagged so the
sibling does not design a seam with no injector.

### 4. PRODUCTION CALL SITES — the reachability spine

Ranked by what actually makes the feature reachable.

1. **`crates/cyrup-ext-subagents/src/extension/tool/routing.rs:1084` `route_action`** — the one and
   only tool dispatch. Two new arms, placed with the same discipline the existing arms use: the
   `worktree.*`/`lane.*` guard arm goes **after** the `mission.*` arm and **before** the `schedule.*`
   arm, matching upstream's own dispatch order (`:6213` sits above `:6294`'s policy-action chain,
   which is where `schedule.create` is reached). Without this, nothing else in this feature is
   addressable by a model or a human.
2. **`crates/cyrup-ext-subagents/src/extension/tool/text.rs:215` `SUBAGENT_ACTIONS`** — feeds
   `schema.rs:359`'s `action` enum directly. A verb missing here is not advertised, so a model
   cannot name it; a verb here without an arm at (1) is the advertise-vs-dispatch defect the crate
   has three tests against.
3. **`crates/cyrup-ext-subagents/src/extension/tool/schema.rs`** — the seven properties
   (`handoffPath`, `repo`, `planId`, `laneId`, `merge`, `supersession`, `lane`) plus the widened
   `mode`. **Each one is load-bearing under `schema.rs:1093`**: advertise it and the build is red
   until `src/extension/` reads `p.<field>`. This is the single strongest reachability guarantee
   available to this batch and it costs nothing to use.
4. **`crates/cyrup-ext-subagents/src/extension/tool/params.rs:86` `SubagentToolParams`** — the six
   fields plus the two projections at `:259`-region. Without the projections living in
   `src/extension/`, (3)'s guard reports every property unwired even when the dispatch works.
5. **`crates/cyrup-ext-subagents/src/registration/authority.rs:72` `AuthorityAction::for_tool_action`**
   — add `"worktree.discard" => Some(Self::DiscardWorktree)`. This is the in-tree note at `:20-22`
   being paid off. `default_decision:83` already returns `Confirm` for it, so a default install
   prompts before discarding, which is upstream's `DEFAULT_AUTHORITY_POLICY` (`policy/authority.ts:14-21`).
   **Do not also map `worktree.cleanup` — see §5.2.**
6. **`crates/cyrup-ext-subagents/src/missions/lifecycle.rs:431` (`artifacts_for_result`) and `:893`
   (`sync_mission_from_async_completion`)** — **no edit required.** They already read
   `details.parallelHandoff.path`. Emitting that key from the `lane.recordMerge`/
   `lane.recordSupersession` results (upstream `:6289`) is what makes two landed, currently-dead
   production readers live. **Do not omit `parallelHandoff` from the details object** as an
   "unused field" — it has consumers today.
7. **`crates/cyrup-ext-subagents/resources/docs/tool-reference.md`** — `### Actions` (`:13`) and
   `### Parameters` (`:64`). `registration/guide.rs:215` fails until every new verb appears. This is
   also the surface `action:"guide"` returns, i.e. how a model discovers these verbs exist at all —
   so it is reachability, not documentation hygiene.
8. **`crates/cyrup-ext-subagents/src/workflows/lane_metadata.rs`** — the worktree-status-reference
   half (§2.3), consumed by the manifest's `cleanup.tasks[].{provider,naming}`.
9. **RPC bridge — `extension/rpc/params.rs:122` `manage_params` / `rpc/mod.rs:121`
   `SUBAGENT_RPC_MANAGEMENT_ACTIONS`. RECOMMENDATION: do NOT widen. See §5.1 for the argument.**

### 5. The three deliberate decisions

#### 5.1 Should the five verbs be reachable from the RPC bridge's `manage`? — **NO.**

Verified, not assumed: `git show v0.68.0:src/extension/rpc.ts` → `SUBAGENT_RPC_MANAGEMENT_ACTIONS`
is `:65-73`, seven entries, all `schedule.*`. And `git show v0.68.0:src/extension/rpc.ts | grep -n
'lane\|worktree\|handoff'` returns **nothing** — the whole RPC surface, all eight methods, has no
concept of a lane or a worktree at v0.68.0.

The seed's argument for yes is real and I want to state it fairly: *a delegating host that fanned
work out over the bridge arguably should be able to converge it the same way.* Three things answer it.

1. **The bridge cannot fan out into worktrees in the first place.** `rpc.ts`'s `spawn` and cyrup's
   `SUBAGENT_RPC_METHODS` (`rpc/mod.rs:109-119`) have no `worktree` parameter and no
   `ParallelGroup` shape. A host that cannot create the manifest over the bridge has nothing to
   converge over the bridge. The symmetry argument is appealing and the premise is false — exactly
   the "delta justifying itself on a false premise" the rules warn about, which is why I checked it.
2. **`worktree.discard` is confirm-gated by default** (`authority.rs:83-92`). The RPC bridge is a
   bus-to-bus channel; `resolve_authority_decision`'s `Confirm` arm needs `host_services()`, and a
   bridge caller is not the session with the UI. Widening the list would either bypass the gate or
   route a destructive confirm prompt to whichever session happens to hold the host — the worse of
   two bad outcomes, and a permission bypass of the kind `authority.rs:20-22` exists to prevent.
3. **`manage_params`' own doc comment already records the rule** (`rpc/params.rs:117-121`): *"That
   narrowing is upstream's own choice … the list stays at seven rather than being widened to
   whatever this side happens to dispatch."* That sentence was written to survive exactly this
   question. Widening now would falsify a comment that is currently true.

**No `[CYRUP-DELTA]` for this** — matching upstream is the default and a delta marks divergence.
What the change SHOULD carry is one sentence appended at `rpc/params.rs:121`: *"Re-checked when the
five lane/worktree verbs landed: `rpc.ts:65-73` @v0.68.0 still names seven, and the bridge has no
`worktree` spawn parameter to converge, so the list is unchanged."* — so the next person does not
re-litigate it from scratch.

#### 5.2 The authority gate — wire `worktree.discard`; do NOT invent a gate for `worktree.cleanup`

`registration/authority.rs:20-22` says *"Whoever lands `worktree.discard` or `destructiveCleanup`
must wire them through [`resolve_authority_decision`] in the same change."* Half of that is
payable and half of it is not, and the note itself is what is wrong.

- **`worktree.discard` → `AuthorityAction::DiscardWorktree`: wire it.** Upstream `:6249` does exactly
  this. `default_decision` already returns `Confirm`. §4.5.
- **`destructiveCleanup`: there is nothing to wire.** `git grep -n 'destructiveCleanup' v0.68.0 -- src`
  returns **exactly two hits, both in `policy/authority.ts`** — `:3` (the `AUTHORITY_ACTIONS` list)
  and `:18` (the `confirm` default). Upstream declares the action and **never consults it, anywhere**.
  And the verb it would plausibly gate, `worktree.cleanup`, is **plan-only** (`:6217-6222`) and
  removes nothing — gating a read-only plan build behind a destructive-cleanup confirm would prompt
  a user to authorize a deletion that cannot occur. Wiring it would be inventing behaviour under a
  note's authority, which is worse than leaving the note unpaid.

**So the deliverable is to CORRECT the note, with the grep as its evidence**, not to satisfy it
literally. Replace `:20-22` with, in substance: *"`worktree.discard` is wired through
[`resolve_authority_decision`] as of <this change> (`extension/tool/routing.rs`'s lane arm),
mirroring `subagent-executor.ts:6249` @v0.68.0. `destructiveCleanup` remains declared and
unconsulted **because upstream never consults it either** — `git grep destructiveCleanup v0.68.0 --
src` is two hits, both in `policy/authority.ts` itself — and the only candidate verb,
`worktree.cleanup`, is plan-only (`:6217`) and removes nothing. It stays parsed-and-inert by parity,
not by omission. Anyone landing a cleanup APPLY phase must wire it through here in that change."*

This is a correction, not a `[CYRUP-DELTA]`: the note made a claim about upstream, and the claim is
checkable and false. That failure mode — *"two defects in the last batch came from a delta justifying
itself on a false premise"* — is precisely what this paragraph is guarding against, on a note that
already exists in-tree.

#### 5.3 `preflight` (`schemas.ts:350`) — advertise it, in this change

The normalizer is landed and complete (`workflows/preflight.rs`, 1306 lines) with **no tool-param
caller**: `grep -c '"preflight"' extension/tool/schema.rs` is 0. That is a fully-built, fully-tested
subsystem that no tool call can reach — the exact failure this programme has shipped five times,
already sitting in the tree. This area is the one that touches `schema.rs`/`params.rs`, so it is the
cheap moment to close it: one property, one field, one
`crate::workflows::normalize_workflow_preflight(...)` call at the launch boundary.

**But it is separable** — it is not one of the five verbs, it is a launch-time param on a different
path, and if closing it properly needs more than the boundary call, it should be split out rather
than half-landed. Recommendation: attempt it; if the launch boundary turns out to need real
plumbing, file it and say so explicitly rather than advertising a property the dispatch drops
(which `schema.rs:1093` would catch anyway, so the failure mode is loud, not silent).

### 6. On-disk JSON — what this area pins

This area writes **no** file. It pins the **wire** shapes on both sides of the dispatch.

**Inbound — `lane` (`schemas.ts:103-110`), and it must round-trip with the manifest's embedded copy:**
```json
{ "version": 1, "key": "lane.a", "mode": "mutation",
  "sourceRef": "main", "claims": ["src/a.rs"], "outputPaths": ["out/a.json"] }
```
`version` integer `1` only; `key` `^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$`; `mode` ∈
`mutation|review|scout|gate`; `sourceRef` 1..=128 bytes; `claims` ≤20 × ≤160 bytes; `outputPaths`
≤10 × ≤256 bytes; every string non-blank after trim and free of `\n`/`\r`/NUL;
`additionalProperties: false`. **Already produced and consumed by
`crate::workflows::WorkflowLaneMetadata`** (`workflows/types.rs:557-584`) — serializes camelCase
with `skip_serializing_if = "Option::is_none"`, so absent-not-null. **Who already reads it:**
`extension/executor/workflow.rs:754` via `parse_child_lane:372`. The manifest embeds this exact
record at `children[].lane` and `laneBindings[].lane`; **one type, one spelling, no second
serializer** — that is the compatibility guarantee, and it is free because the type already exists.

**Inbound — `merge` / `supersession`:** open objects at this layer
(`Type.Unsafe({type:"object", additionalProperties:true})`, `:302-303`). Their pinned field shapes
(`prNumber`, `reviewedHead`, `mergeCommit`, `treeEquivalent`, `postMergeChecks`, `attestedBy`,
`attestedAt`, `manifestDigest` / `supersededBy`, `attestedBy`, `attestedAt`, `manifestDigest`) are
`[AUG — manifest]` §5's contract; this area only guarantees the object arrives **unmodified** —
no trimming, no key filtering, no camelCase round-trip through an intermediate struct, because
`manifestDigest` is stamped by the recorder and any re-serialization risk is asymmetric.

**Outbound — the `details` object.** `{ "mode": "management", "results": [] }` for
`worktree.cleanup`, `worktree.discard`, `lane.status`; and for the two recorders
(upstream `:6289`):
```json
{ "mode": "management", "results": [],
  "parallelHandoff": { "version": 1, "path": "…/handoffs/<runId>.json", "groupCount": 1,
                       "childCount": 2, "changedPatches": 1, "cleanupState": "partial",
                       "cleanupEligibility": "terminal-blocked" } }
```
**Who already reads it:** `missions/lifecycle.rs:431-442` (`artifacts_for_result`) and `:893-903`
(`sync_mission_from_async_completion`), both reading `parallelHandoff.path` and emitting a
`MissionArtifactKind::Manifest`. Both landed, both currently dead. `details.results: []` must be a
present empty array, not omitted — upstream emits it on every one of these arms and cyrup's
management details shape (`routing.rs:1297`) already does.

### 7. The reachability test

**Primary — drives the real tool dispatch, not the subsystem.** A `#[tokio::test]` in
`crates/cyrup-ext-subagents/src/extension/tool/lane_actions.rs`'s test module, built on the harness
`extension/tool/scheduled_runs_tests.rs` already establishes for exactly this shape (that file is
the crate's own worked example of "a management verb, tested through `route_action`"):

1. Build a real git repo with one commit and run
   `subagent({ tasks: [t1, t2], worktree: true, async: false })` **through the tool's `execute`**,
   so `route_parallel_mode` → the foreground chain → the manifest writer run for real.
2. Read `details.parallelHandoff.path` **off the tool result** — not off disk, not off a constructed
   type. If `write_group` never ran, this key is absent and the test dies here.
3. `subagent({ action: "lane.status", laneId: <run id>, handoffPath: <that path> })` through
   `execute`. Assert the rendered text names both children and reports the cleanup state.
4. `subagent({ action: "lane.recordMerge", laneId, handoffPath, merge: {…valid evidence…} })`.
   Assert `details.parallelHandoff` is present on the RESULT and its `path` is the same file —
   which is the assertion that revives `missions/lifecycle.rs`'s two readers.
5. Re-run `lane.status`; assert the merge evidence is now reflected.
6. `subagent({ action: "worktree.cleanup", mode: "plan", repo: <repo> })`; assert the plan names the
   worktrees and ends in the plan-only line.
7. **The three negative assertions, which are the ones that prove the split:**
   - with `allow_mutating_management = false`, `lane.status` **succeeds**;
   - with the same flag, `lane.recordMerge` returns exactly
     `Action 'lane.recordMerge' is not available from child-safe subagent fanout mode.`;
   - `worktree.cleanup` with `mode: "apply"` returns exactly
     `worktree.cleanup currently supports mode='plan' only; apply/removal is not available yet.`

**Why it FAILS if gutted.** Every step drives `Tool::execute`/`route_action` with a JSON params
object — no step constructs a `LaneAction`, a `Manifest` or a plan directly. A verb missing from
`SUBAGENT_ACTIONS` fails the schema's `action` enum before dispatch. A property missing from
`schema.rs` is dropped by the permissive deserializer and step 3 fails with "requires handoffPath".
An arm that reads `p.handoff_path` but drops it fails at step 3. A `details` object without
`parallelHandoff` fails step 4. A child-safe gate that forgot `lane.status`'s exemption fails 7a;
one that forgot the other two fails 7b. **And the crate's own
`every_advertised_schema_property_is_read_outside_provided_keys` (`schema.rs:1093`) fails at compile-
time-equivalent for any property advertised without a read — a guard this area does not have to
write, only avoid defeating.**

**Secondary (mechanical, already-existing guards that must be made to pass, not added):**
`registration/guide.rs:215` `the_tool_reference_topic_names_every_dispatched_verb`;
`schema.rs:845-900`'s exact action enum values-and-order assertion;
`registration/tool_description.rs:744` `the_compact_description_advertises_no_verb_cyrup_cannot_dispatch`
(only if the compact description is edited to mention `lane.*`/`worktree.*` — it scans tokens out of
the text, so mentioning `worktree.*` without the verbs existing goes red, and mentioning them after
they exist is free).

**Tertiary (unit, in `lane_actions.rs`):** `LaneAction::from_wire` round-trips `as_str` for all five;
`is_mutating()` is `false` for `LaneStatus` and `true` for the other four; `authority_action()` is
`Some(DiscardWorktree)` for `WorktreeDiscard` and `None` for the other four. Three tiny tests that
pin the three orthogonal properties §3.2 folds into the enum, because those are the three things a
future sixth verb can silently get wrong.

### 8. Safety invariants — what must NEVER happen

1. **`lane.status` must NEVER become mutating.** `:6271`'s condition is
   `action !== "lane.status" && allowMutatingManagementActions === false`. If a future refactor
   routes all five through one `is_mutating()` that returns `true` for `LaneStatus`, a child-safe
   fanout silently loses the ability to read its own lane graph — the whole point of the feature for
   delegated work. `LaneAction::is_mutating` is the one place this lives; test it directly (§7).
2. **The child-safe gate runs BEFORE param validation, always.** Upstream checks it at `:6214`,
   `:6243`, `:6271` — above `mode`, above `planId`, above `laneId`, above `handoffPath`. Reordering
   leaks the shape of the refusal to a caller that is not permitted to invoke the verb at all.
3. **`worktree.discard` must NEVER run without the authority consult.** `:6249-6258`. Not
   `unwrap_or(Auto)` on a missing policy — `resolve_authority_decision` falls back to
   `default_decision`, which is `Confirm`. A `Confirm` with no UI is a **refusal** (`:6255`), never
   an implicit yes.
4. **A discard must NEVER race a worktree setup.** Upstream wraps it in `withWorktreeTransaction`
   (`:6261`, `worktree.ts:207-216`), which also re-checks `setupPoison` inside the turn. **cyrup has
   no such serialization** — verified: 0 hits for `with_worktree_transaction`/`worktree_transaction`/
   `WORKTREE_MUTEX`, and `spawn/worktree.rs` declares no `static`/`Mutex`/`Semaphore`/`OnceLock`.
   Landing `discard_preserved` without one lets a discard remove a worktree another task is
   mid-`git worktree add` into. **This is a hard prerequisite, it belongs to `spawn/worktree.rs`
   (the cleanup sibling's file), and it must be built before the discard arm dispatches.** Flagged
   here as §9.1 because this arm is the caller that makes the race reachable.
5. **A user declining a confirm must NEVER be an error.** `:6258` has no `isError`. Returning `Err`
   would make a deliberate "no" look like a failure to an orchestrator retry loop, which would
   re-prompt. `routing.rs:1295-1300`'s existing decline branch is the shape.
6. **`handoffPath` and `repo` must be resolved against the REQUEST cwd**, never `std::env::current_dir()`
   (`:6226`, `:6260`, `:6278`). `route_action` already receives `cwd: &Path`; use it. A verb that
   silently resolved against the process cwd could be pointed at a manifest outside the project.
7. **The `merge`/`supersession` objects must reach the recorder unmodified.** No trimming, no
   filtering, no round-trip through an intermediate struct. `manifestDigest` is overwritten by the
   recorder and every other field is evidence; a helpful normalization here would change what an
   attestation attests to.
8. **`lane.status`'s swallowed read error must NOT spread.** `:6282`'s bare `catch` is scoped to
   `lane.status` alone; the run-id mismatch at `:6283` and every `lane.record*` failure are errors.
9. **A verb must never be advertised without an arm, or wired without being advertised.** Three
   existing tests enforce it (§2.12, §2.10). Do not satisfy `schema.rs:1093` by adding a name to
   `provided_keys()` — its own failure message names that as the forbidden move, and it is how
   `chainDir` stayed broken.
10. **Never map `worktree.cleanup` to `AuthorityAction::DestructiveCleanup`** (§5.2). It has no
    upstream consult and removes nothing; a confirm prompt for a no-op teaches users to click
    through prompts.

### 9. Blockers / dependencies

1. **HARD, and it is mine to flag though not mine to fix: no worktree-mutation serialization.**
   §8.4. `discardPreservedWorktrees` upstream is only ever called inside `withWorktreeTransaction`.
   The cleanup sibling owns `spawn/worktree.rs`; this must land there before the discard arm does.
2. **The manifest sibling's §3 module layout must drop `handoff/lane.rs`** (§0). If both land,
   the crate gets two incompatible `LaneMetadata` serializations for the same JSON object and the
   `deny_unknown_fields` round-trip between them is a coin flip. **This is the highest-value
   correction in my section; it needs to reach the executor before either sibling writes code.**
   Same for the manifest sibling's §3.2 rows `LaneSourceRef` / `LaneClaim` / `LaneOutputPath` —
   `Bounded<128>/<160>/<256>` already carry those bounds on the landed struct.
3. **`ManagedWorktreeProvider` / `WorktreeNaming` do not exist** (§2.3, 0 hits). The manifest's
   `cleanup.tasks[].{provider,naming}` and `lane.status`'s render of them need the
   `lane-metadata.ts:76-110` half. Mine to port, into `workflows/lane_metadata.rs`.
4. **The three landed guards will go red on the first commit** and must be fixed in the same change,
   not after: `guide.rs:215` (tool-reference page), `schema.rs:845-900` (action enum order),
   `schema.rs:1093` (property reads). Budget the `tool-reference.md` edit — `### Actions` at `:13`
   and `### Parameters` at `:64` — as real work, because it is also the model's discovery surface.
5. **Ordering against the siblings.** This area's arms cannot compile until
   `handoff::{read_manifest, format_stored_cleanup, record_merge, record_supersession,
   discard_preserved}` and `spawn::cleanup_plan::{create_worktree_cleanup_plan,
   format_worktree_cleanup_plan}` exist. **Suggested sequence:** (a) worktree-status-reference half
   + `LaneAction` + params + schema + `SUBAGENT_ACTIONS` + authority, with the arms returning the
   refusals only — every guard green, nothing advertised-but-unread, because the refusal paths
   already read every property; (b) siblings land their subsystems; (c) the arms call them; (d) the
   end-to-end test. Step (a) is independently landable and independently green, which is worth a
   lot given the collision risk on `text.rs` and `schema.rs`.
6. **`foregroundRunOwnership` has exactly one possible injector** and it is this arm (§3.7). The
   cleanup sibling should be told, so it does not design a seam nothing can fill.
7. **Collision warning, echoing both siblings' §10s:** `extension/tool/text.rs`'s `SUBAGENT_ACTIONS`
   is named by all three of us. It should be edited **once**, by whoever lands step 5(a), and the
   other two should not touch it.

### 10. Files this area expects to touch

**New**
- `crates/cyrup-ext-subagents/src/extension/tool/lane_actions.rs` — `LaneAction`,
  `LaneActionRefusal`, `LaneActionParams`, `CleanupPlanParams`, the two arm bodies, the tests.
  (Under `src/extension/` deliberately — §3.1.)

**Modified — mine alone**
- `crates/cyrup-ext-subagents/src/extension/tool/params.rs` (six fields on `SubagentToolParams:86`;
  two projections beside `:259`/`:279`; `provided_keys:583` gains the six names)
- `crates/cyrup-ext-subagents/src/extension/tool/schema.rs` (seven properties; widen the `mode` enum
  at `:414-422`; update the action-enum assertion `:845-900` and the property list `:966-1000`)
- `crates/cyrup-ext-subagents/src/extension/tool/routing.rs` (two arms in `route_action:1084`;
  `mod lane_actions;` in `extension/tool/mod.rs`)
- `crates/cyrup-ext-subagents/src/registration/authority.rs` (`for_tool_action:72`; **correct** the
  `:20-22` note per §5.2)
- `crates/cyrup-ext-subagents/src/workflows/lane_metadata.rs` (the `:76-110` worktree half; delete
  the stale "has no consumer here" sentence at `:1-3`)
- `crates/cyrup-ext-subagents/src/workflows/mod.rs` (re-export the new items; the module map at `:53`)
- `crates/cyrup-ext-subagents/resources/docs/tool-reference.md` (`### Actions` `:13`,
  `### Parameters` `:64`)
- `crates/cyrup-ext-subagents/src/extension/rpc/params.rs:121` — **comment only**, the §5.1 note

**Modified — shared, coordinate**
- `crates/cyrup-ext-subagents/src/extension/tool/text.rs` — `SUBAGENT_ACTIONS:215`, +5 entries
  between `"mission.close"` and `"watchdog.status"`. **All three areas name this file; one editor.**

**NOT touched, and deliberately**
- `crates/cyrup-ext-subagents/src/missions/lifecycle.rs` — its two `parallelHandoff` readers
  (`:431`, `:893`) already do the right thing; this area feeds them (§4.6).
- `crates/cyrup-ext-subagents/src/extension/rpc/mod.rs` — `SUBAGENT_RPC_MANAGEMENT_ACTIONS:121`
  stays at seven (§5.1).
- `crates/cyrup-ext-subagents/src/workflows/preflight.rs` — complete; only its tool-surface
  *caller* is missing (§5.3).

**Tests**
- `crates/cyrup-ext-subagents/src/extension/tool/lane_actions.rs` (the §7 primary + tertiary)
- existing, must be updated: `registration/guide.rs:215`, `extension/tool/schema.rs:845-900` and
  `:1093`, `registration/tool_description.rs:744`

## [EXEC — manifest]

Step 1 of 3 (the substrate). The manifest, its safety core, its writer, and the production spine
that makes a `worktree: true` fan-out publish one. Upstream read only via
`git -C tmp/pi-subagents show v0.68.0:<path>`.

### What landed

**New module `crates/cyrup-ext-subagents/src/handoff/`** (registered at `src/lib.rs:39`):

| file | contents |
|---|---|
| `mod.rs` | module doc + the re-export surface; the safety-rule table |
| `error.rs` | `HandoffError` (thiserror) — one variant per pi `throw`, each `#[error(...)]` pi's verbatim text; `ManifestDefect` sub-enum carries pi's per-defect wording (`:80-111`) |
| `model.rs` | every newtype, enum and record; the on-disk contract |
| `evidence.rs` | `manifest_facts_digest`, `has_active_children`, `cleanup_eligibility_for_evidence`, `trusted_cleanup_eligibility`, `validate_manifest_for_lane_evidence`, `reference_for` |
| `read.rs` | `read_manifest`, `validate_manifest_identity`, `resolve_child` (+ `ChildSelector`), `resolve_retained_worktree_cwd` |
| `write.rs` | `write_group`, `record_merge`, `record_supersession`, `CLEANUP_PENDING_REASON` |
| `path.rs` | `handoff_manifest_path(base, run_id)` |
| `format.rs` | `format_stored_cleanup`, `format_reference`, `format_error` |
| `tests.rs` | 22 unit tests (serde spelling, anti-forgery, evidence rules, writer) |

**`handoff/lane.rs` was NOT written.** The `[AUG — surface]` correction is right:
`crate::workflows::WorkflowLaneMetadata` (`workflows/types.rs:557`) is already the complete port of
`lane-metadata.ts:49-74` — camelCase, `deny_unknown_fields`, `key: WorkflowKey`, the 128/160/256
bounds. `handoff::model::{Child,LaneBinding}::lane` import it. `LaneSourceRef`/`LaneClaim`/
`LaneOutputPath` were likewise not written: `Bounded<128>/<160>/<256>` already carry those bounds.

### PRODUCTION call sites (file:line, post-`cargo fmt`)

1. `crates/cyrup-ext-subagents/src/spawn/chain_graph.rs:1743` — `run_parallel_group` calls
   `publish_worktree_handoff` immediately after `dispatch_group` (pi `subagent-runner.ts:4389-4434`).
   `crates/cyrup-ext-subagents/src/spawn/chain_graph.rs:1782` is that function: capture diffs →
   **write** → `cleanup_worktrees` → **write again**. This is THE site; there is no other worktree
   fan-out in cyrup.
2. `crates/cyrup-ext-subagents/src/spawn/chain_graph.rs:1703` — `assign_worktree_cwds` now RETURNS
   the `WorktreeSetup` (`spawn/worktree.rs:1281` `WorktreeGroupPlan::setup`), which the writer needs
   for `repoRoot`, `baseCommit` and the cleanup tasks (pi `:530-531`, `:574-584`).
3. `crates/cyrup-ext-subagents/src/background/runner_main/turn_loop.rs:468` — the detached runner's
   `ChainRunContext` binds `RunDir::for_existing(&run_paths.run_dir).handoff()`, source `Async`.
   THIS is what makes `has_unresolved_run_handoff` find a real file.
4. `crates/cyrup-ext-subagents/src/extension/executor/chain.rs:238` — the foreground walk binds
   `handoff_manifest_path(&artifacts_dir, &run_id)`, source `Foreground` (pi
   `subagent-executor.ts:3718`). The `RunId` minted inline at `chain.rs:135` is hoisted so the
   executor and the manifest path share one id.
5. `crates/cyrup-ext-subagents/src/background/runner_main/status.rs:344` —
   `record_step_outcome` sets `status.parallel_handoff` from the settled group (pi `:4427`).
6. `crates/cyrup-ext-subagents/src/background/records.rs:360` — `RunStatus::parallel_handoff:
   Option<HandoffReference>` added; `RunStatus::queued` and `control.rs`'s terminal-repair
   constructor initialise it.
7. `crates/cyrup-ext-subagents/src/background/run_paths.rs:89` — `RunDir::handoff()` owns the
   `handoff.json` literal; `HANDOFF_MANIFEST_FILE` deleted from `async_retention/scan.rs:53`.
8. `crates/cyrup-ext-subagents/src/background/async_retention/scan.rs:427` —
   `has_unresolved_run_handoff(run_dir, status)` now consults BOTH of pi's path sources
   (`status.parallelHandoff.path` and the local file) as a set, `:456` is the per-file half. **The
   `[CYRUP-DELTA]` at the old `:425-429` is deleted in full** — both of its clauses stopped being
   true.
9. `crates/cyrup-ext-subagents/src/error.rs:264` — `SubagentError::Handoff(#[from] HandoffError)`,
   never stringified.
10. `crates/cyrup-ext-subagents/src/spawn/worktree.rs:1069` — `cleanup_worktrees` takes a
    `WorktreeCleanupIntent` and RETURNS a `WorktreeCleanupReport`. It had zero non-test callers;
    it now has one, and its report is the manifest's `group.cleanup`.
11. `crates/cyrup-ext-subagents/src/extension/tool/routing.rs` `route_parallel_mode` — unchanged,
    and named because it is the `subagent({tasks:[…], worktree:true})` entry that reaches (3)/(4).

**The HARD BLOCKER is fixed.** `WorktreeGroupConfig::worktree_base_dir` is now `Option<&Path>`
(`spawn/worktree.rs:1242`) and `assign_worktree_cwds` passes `ctx.worktree_base_dir` THROUGH instead
of erroring on `None`, so `resolve_worktree_base_dir`'s env-var → `temp_dir()` ladder runs. Before
this, `subagent({tasks:[…], worktree:true})` failed in every default install. The reachability test
runs with `worktree_base_dir: None` precisely to pin that.

**RPC bridge — the deliberate non-site.** `extension/rpc/params.rs` `manage_params` is NOT widened.
Upstream `rpc.ts:65-73` @v0.68.0 is seven `schedule.*` verbs and contains no lane verb; matching
upstream is not a divergence, so no `[CYRUP-DELTA]`.

### Rust shapes taken over upstream's

* string unions → enums: `HandoffMode`, `HandoffSource`, `ChildStatus` (+`is_terminal`, `Detached`
  NOT terminal), `CleanupState`, `PostMergeChecks`, `TreeEquivalence`, `BlockReason`,
  `WorktreeCleanupIntent`;
* `CleanupEligibility` is `#[serde(tag="state", rename_all="kebab-case", deny_unknown_fields)]`, so
  "blocked without a reason" is unrepresentable;
* pi's `/ evidence is stale$/` regex (`:320`) is `BlockReason::is_stale`, a match arm;
* `boundedNonEmptyString` → newtypes with `parse` + a HAND-WRITTEN `Deserialize` that goes through
  it (`workflows/key.rs:73-78`'s idiom): `CommitSha`, `ManifestDigest`, `AttestationActor`,
  `AttestationTimestamp`, `LaneId`, `BoundedReason`, `PrNumber(NonZeroU32)`, `ManifestVersion`;
* pi's `input:{…}` literals → named parameter structs `WriteGroup`/`RecordMerge`/`RecordSupersession`;
* pi's "requires workflowKey or childRunId" throw (`:139`) → `ChildSelector`, unrepresentable;
* `throw new Error(string)` → `HandoffError` + `ManifestDefect`;
* `ChainRunContext` carries a `HandoffBinding` (`chain_graph.rs:1456`), not a bare path — `runId`,
  `mode` and `source` are all manifest identity (`:514-516`), and a context holding only the path
  would force the fan-out to re-derive the other three.

### `[CYRUP-DELTA]`s written (premises grepped first)

1. **`deny_unknown_fields` on every record** (`handoff/model.rs` module doc). pi applies an
   allow-list only to lane bindings (`normalizeLaneBinding:52-53`); its other records silently carry
   an unknown key through a rewrite. cyrup fails closed. *Noted risk:* the `[AUG — cleanup]` sibling
   argued the other way (forward-compat with a newer pi manifest). The computed task for this area
   directed `deny_unknown_fields`, and a file cyrup both writes and reads is the case where strict
   wins — but if the cleanup sibling needs to plan against a pi-written manifest, this is the
   decision to revisit.
2. **`AttestationTimestamp` is RFC 3339 only**, narrower than `Date.parse`. Hand-rolled: neither
   `time` nor `chrono` is a dependency of this crate (verified in its `Cargo.toml`).
3. **`HandoffMode` is not `background::RunMode`** — that enum has a `Workflow` variant
   (`background/state.rs:37`) pi's union does not admit. A workflow run's manifest records `Chain`.
4. **The `stopped` child status is never WRITTEN** (`chain_graph.rs`'s `handoff_child_status`).
   `StepResult` carries no stop marker; a stop in cyrup is a run-level verdict that tears the walk
   down. Synthesizing `Stopped` from `!success` would report a crashed lane as an operator-stopped
   one. The variant stays in `ChildStatus` because pi's reader emits it.
5. **`missing_patch` does not create an empty `.patch` file** (pi `:479-483` does).
   `diff_worktrees` already writes one on a capture failure, so this arm is only reached when no
   capture was attempted — where a zero-byte file would fabricate evidence of a harvest.
6. ~~**`WorktreeCleanupIntent` carries no payload.**~~ **WITHDRAWN — both halves were false in the
   landed tree.** `Discard { authorization }` carried a payload and was read the moment it landed
   (`handoff/model.rs`, `spawn/worktree.rs`'s second authority consult), and `preserve` acquired a
   production caller in the same batch (`chain_graph.rs`'s `publish_worktree_handoff`). See
   `## [FIX — safety]`, which ports the preserve payload and the gate that reads it.

`stable_json_digest`'s existing byte-order `[CYRUP-DELTA]` already covers `manifestFactsDigest` — no
second one written, as `[AUG — manifest]` §1.2 instructs.

### Tests

**Reachability (the bar):**
`crates/cyrup-it/tests/subagents/background_runner_main_integration.rs` →
`a_worktree_fan_out_publishes_a_real_parallel_handoff_manifest`. Hands a `RunnerConfig`
(`worktree: true`, `worktree_base_dir: None`) to the REAL `run_with` against the REAL scripted
fixture in a REAL git repo, then reads back **only through production readers**:
`handoff::read_manifest`, `RunStatus::parallel_handoff`, and
`async_retention::scan_run_candidates`. It constructs no `handoff::` value by hand.

Each lane declares a RELATIVE `output:` path, which `runner_main/executor.rs:616` resolves against
that step's effective cwd — the lane's own worktree — so the delivered output lands inside the
checkout and the captured patch is non-empty. That is what proves `assign_worktree_cwds` pointed
each child at a real, distinct worktree, with no test-double change.

**Mutations run, and the assertion each one broke:**

| mutation | failing assertion |
|---|---|
| delete the phase-1 (pre-cleanup) `write_group` | `updated_at > created_at` (`:2957`) — "the pre-cleanup write must precede the post-cleanup one" |
| gut `handoff::write::write_manifest` to `Ok(())` | `manifest_path.exists()` (`:2936`) |
| make `assign_worktree_cwds` discard the `WorktreeSetup` again | `read_manifest(...).expect("manifest validates")` (`:2945`) — children with no cleanup task |
| delete `status.parallel_handoff = …` in `record_step_outcome` | `status.parallel_handoff` (`:3053`) |
| restore the `worktree_base_dir` hard error | `status.state == Complete` (`:2925`) — the group is refused in a default install |
| point the retention reader at `status()` instead of `handoff()` | `!facts.unresolved_handoff` (`:3075`) |

All six restored. Unit tests: 22 in `handoff/tests.rs`, including the literal on-disk JSON spelling
of the two hand-written serde impls, the anti-forgery rule (a hand-edited `terminal-eligible`
collapses to `unknown`), the stale-downgrade exception, the append-once merge rules, and
self-supersession.

### NOT done in this step (for the two siblings)

* **The five verbs and every tool-surface property** (`worktree.discard`, `worktree.cleanup`,
  `lane.status`, `lane.recordMerge`, `lane.recordSupersession`; `handoffPath`/`laneId`/`merge`/
  `supersession`/`lane`) — the `[AUG — surface]` area. `record_merge`/`record_supersession`/
  `read_manifest`/`format_stored_cleanup` are the functions those arms call, and they are landed
  and tested here.
* **`discard_preserved`** (pi `:684-741`) — it is `worktree.discard`'s body. Its dependency,
  `cleanup_worktrees(setup, Discard)`, is landed; the arm that calls it is the surface sibling's.
* **`write_setup_handoff`** (pi `:619-674`) — allocation-progress evidence. `create_worktrees` has
  no `onProgress` seam in cyrup, so there is nothing to project. Stating it rather than stubbing it.
* **The two-phase cleanup PLAN** (`spawn/cleanup_plan/`) — the `[AUG — cleanup]` area.
* **`DynamicGroupSpec` has no `worktree` flag**, so a dynamic fan-out writes no handoff whereas pi
  does (`subagent-runner.ts:4776-4810`). Out of scope, as `[AUG — manifest]` §8.6 states — recorded
  so a reviewer does not read it as an oversight.

### Gates run

| gate | result |
|---|---|
| `cargo fmt --all` / `--check` | clean |
| `cargo clippy --workspace --all-targets --features test-fixtures -- -D warnings` | clean |
| `cargo nextest run --workspace --features test-fixtures` | **`Summary [109.772s] 10496 tests run: 10496 passed, 9 skipped`** (baseline 10474 passing / 9 skipped; the +22 are `handoff/tests.rs`) |
| `cargo nextest run -p cyrup-it --features it -E 'binary(subagents)'` | **`Summary [24.060s] 210 tests run: 210 passed, 0 skipped`** (baseline 209; the +1 is the reachability test) |
| `cargo nextest run -p cyrup-it --features it` (all 552) | **NOT completed in this environment.** Two attempts stalled around 110/552 inside `cyrup-it::ext`, whose wasm/`cargo build` tests run 15–85 s each on this 4-core box at load ~12 (`build_tier1::tier1_cargo_build_emits_a_component_that_caches_and_instantiates` alone took 82 s). Nothing in this change touches `cyrup-it::ext`/`::bin`; the `subagents` binary — which holds every `it` test this change affects — is green above. Stated rather than claimed. |

Disk note: `target/doc` (26 MB) and three stale `target/debug/build/cyrup-it-*` output trees (~1.9 GB of
regenerable `it-bins`/`it-wasm`) were deleted to get under the ~2.5 GB budget; `CYRUP_IT_BIN_DIR`
pointed at the surviving `cyrup-it-87b505e3149e7da5/out/it-bins/debug` so the 7.7 GB relink was skipped.

## [EXEC — cleanup]

Step 2 of 3. The **two-phase worktree cleanup PLAN** — the read-only half of `worktree.cleanup` —
plus the tool-surface wiring that makes it reachable. Upstream read only via
`git -C tmp/pi-subagents show v0.68.0:<path>`.

### The correction in `[AUG — cleanup]` §1 holds, and it shaped the whole deliverable

`worktree.cleanup` is **plan-only at v0.68.0**. `subagent-executor.ts:6217-6222` refuses
`mode != "plan"` and refuses `planId` outright; `schemas.ts:300` says *"Reserved; cleanup is
plan-only."*; `git grep v0.68.0 -- src` finds exactly two consumers of the module (the import at
`:154`, the one call at `:6224`). There is **no apply path, no plan reader and no TTL enforcement
anywhere upstream**. So the port is plan-only too, and the safety invariant *"a stale plan must
never be executed"* is discharged **structurally**: the two refusals at the dispatch boundary ARE
the guard, and they are pinned verbatim by a test.

`expiresAt`, `contentHash` and `preconditions` are written completely — they are the forward
contract that makes a later apply phase safe — and are read by nobody. That is stated in the
module doc rather than left for a reviewer to discover.

### What landed

**New module `crates/cyrup-ext-subagents/src/spawn/cleanup_plan/`** (registered at
`src/spawn/mod.rs:30`):

| file | contents |
|---|---|
| `mod.rs` | `BuildCleanupPlanInput`, `build_worktree_cleanup_plan`, `create_worktree_cleanup_plan`, the git↔metadata join, the content hash, the base-dir `[CYRUP-DELTA]` |
| `model.rs` | the plan's on-disk contract, the four newtypes, the four wire enums, `CleanupPlanError` |
| `verdict.rs` | `Verdict` + the four reason enums — the headline type win |
| `git.rs` | the plan's whole borrowed `git` surface, deliberately small |
| `gitwt.rs` | `GitWorktreeRecord` + `parse_git_worktree_list` (pure, `pub(crate)`, 5 unit tests) |
| `metadata.rs` | handoff discovery, the bounded manifest/status reads, `inspect_run_state` |
| `paths.rs` | `comparable_path` / `same_path` / `path_inside` / `inspect_path` (4 unit tests) |
| `classify.rs` | all 41 gates of `buildManagedEntry`, in upstream's evaluation order |
| `render.rs` | `format_worktree_cleanup_plan` (1 unit test pinning the ISO-8601 conversion) |

`crates/cyrup-ext-subagents/src/extension/tool/worktree_cleanup_tests.rs` is new: 12 rows, all
driving `Tool::execute`.

### PRODUCTION call sites (file:line, post-`cargo fmt`)

1. `crates/cyrup-ext-subagents/src/extension/tool/routing.rs:1251` — the `"worktree.cleanup"` arm
   in `route_action`. Child-safe gate → `mode != "plan"` refusal → `planId` refusal → resolve
   `repo`/`handoffPath` against the request cwd → `config_snapshot().worktree_base_dir` → the
   foreground-ownership closure → `create_worktree_cleanup_plan` →
   `format_worktree_cleanup_plan` + `details {mode:"management", results:[]}`. **This is THE site;
   there is no other way to reach the module.**
2. `crates/cyrup-ext-subagents/src/extension/tool/text.rs:299` — `"worktree.cleanup"` added to
   `SUBAGENT_ACTIONS` at pi's own index (`shared/types.ts:2801` @v0.68.0 reads
   `… "mission.close", "worktree.discard", "worktree.cleanup", "lane.status", …`; cyrup has none
   of the other four yet, so it follows `mission.close`). **Not** added to
   `DESTRUCTIVE_MANAGEMENT_ACTIONS` — upstream's list (`subagent-executor.ts:168`) does not carry
   it either, because plan-only mode removes nothing.
3. `crates/cyrup-ext-subagents/src/extension/tool/schema.rs:442,443,445` — `handoffPath`, `repo`
   and `planId` advertised, descriptions verbatim from `extension/schemas.ts:298-300`.
   `schema.rs:426` widens the `mode` enum from three to upstream's five
   (`["steer","follow_up","auto","plan","apply"]`, `schemas.ts:313`). Safe, and checked:
   `SteerDeliveryMode::parse` is a closed three-arm match returning `None` otherwise, so
   `action='steer'` with `mode='plan'` is still refused at the boundary.
4. `crates/cyrup-ext-subagents/src/extension/tool/params.rs:137,140,148` — `handoff_path`, `repo`,
   `plan_id` on `SubagentToolParams`; `:634,637,640` add all three to `provided_keys()`.
5. `crates/cyrup-ext-subagents/src/extension/executor/mod.rs:544`
   `SubagentExecutor::foreground_run_ownership` — the ONLY possible injector for pi's
   `foregroundRunOwnership` seam (`:6230-6235`), reading the live `foreground_controls` /
   `foreground_runs` pair.
6. `crates/cyrup-ext-subagents/src/error.rs:274` —
   `SubagentError::WorktreeCleanupPlan(#[from] CleanupPlanError)` with `#[error("{0}")]`, matching
   the `Management(String)` verbatim-prose convention; upstream renders these as `isError` prose
   the model reads, so no prefix is added.
7. `crates/cyrup-ext-subagents/resources/docs/tool-reference.md:45,103-106,122` — the verb, its
   four parameters, and a `### Worktree cleanup` section. This is also the model's discovery
   surface (`action:"guide"`), so it is reachability, not documentation hygiene.
8. `crates/cyrup-ext-subagents/src/spawn/worktree.rs` — `GitResult`/`run_git`/`run_git_checked`
   promoted `pub(crate)`; `:212` **new `run_git_env`**; `:315` **new
   `resolve_worktree_base_dir_path`** (the non-creating half of `resolve_worktree_base_dir`);
   `:1100` **new `validate_worktree_patch_represents_current_worktree`** with its
   `validate_worktree_patch`/`current_worktree_patch` helpers.
9. **RPC bridge — the deliberate non-site.** `extension/rpc/mod.rs`'s
   `SUBAGENT_RPC_MANAGEMENT_ACTIONS` is unchanged at seven. Upstream `rpc.ts:65-73` @v0.68.0 is
   the identical seven `schedule.*` verbs and the whole RPC surface has no concept of a worktree,
   so widening it would be inventing a surface, not porting one. `[AUG — cleanup]` §5 recommended
   the opposite and §11 withdrew it; that withdrawal is what shipped. No `[CYRUP-DELTA]` —
   matching upstream is the default.
10. **Authority — the deliberate non-gate.** `registration/authority.rs`'s `for_tool_action` still
    returns `None` for this verb, on purpose. pi gates `worktree.discard` on `discardWorktree`
    (`:6249`) and gates `worktree.cleanup` on **nothing**; a confirm prompt for a deletion that
    cannot occur teaches operators to click through prompts. The `authority.rs:20` note is the
    `worktree.discard` sibling's to discharge.

### Rust shapes taken over upstream's

* **`Verdict` collapses pi's `(state, decision)` pair.** All 41 arms of `buildManagedEntry` were
  enumerated; the mapping is a total function (`safe→remove`; `ineligible|dirty|active→keep`;
  `stale|unknown→unknown`). `Verdict::decision()` is an exhaustive match, so a future arm cannot
  spell `state: "dirty", decision: "remove"` — the single worst bug this module can have.
  `will_delete_branch` lives ONLY on `Verdict::Safe`, so a kept entry cannot carry a
  branch-deletion flag; the test asserts the key is absent from the JSON.
* String unions → enums: `CleanupState`, `CleanupDecision`, `CleanupSource`,
  `ForegroundRunOwnership` (`rename_all = "lowercase"`; the last is never serialized).
* `boundedNonEmptyString`/`validatePlanId` → newtypes with fallible constructors: `PlanId`
  (hand-written `Deserialize` through `parse`, because a plan file is untrusted input),
  `ContentHash`, `CommitId` (constructible only from a `rev-parse --verify` that exited 0),
  `StatusDigest`.
* `throw new Error(string)` → `CleanupPlanError`, five variants.
* pi's per-arm interpolated reason strings → four reason enums carrying their data
  (`UnknownReason::MetadataBranchMismatch{metadata,git}`, `IneligibleReason::OutsideBaseDir
  {base_dir}`, `StaleReason::ActiveMarkerStale{age_secs}`, …) with `Display` impls rendering pi's
  verbatim sentences.
* `Verdict` has **no `Default`**: the only path to `Safe` is falling off the end of all 41 gates.
* `foreground_ownership` is `&dyn Fn(...)`, **not `Option`** — pi's `?? "unknown"` makes an absent
  probe indistinguishable from an unknown answer, so the `Option` carried no information and only
  added a way for "nobody supplied a probe" to read as "terminal".
* `resolve_worktree_base_dir` was **split** into a pure `_path` half and a `mkdir`-ing half, so a
  planning operation can ask where managed worktrees live without creating that directory.

### `[CYRUP-DELTA]`s written (premises grepped before writing)

1. **The base directory** (`cleanup_plan/mod.rs`, `resolve_cleanup_base_dir`). pi's
   `resolveCleanupBaseDir:213-227` reproduces pi's own CREATE layout
   (`<dirname(repoRoot)>/worktrees/<basename(repoRoot)>`, verified against `worktree.ts:648-651`
   and `buildNativeWorktreePath:684-686`). cyrup's create side is
   `configured → $CYRUP_SUBAGENTS_WORKTREE_DIR → temp_dir()` with no `worktrees/` rung and no
   per-repo level, leaf at `<base>/cyrup-worktree-<runId>-<index>`. Transliterating pi's resolver
   would make strict-child containment FALSE for every real cyrup worktree → every entry
   `Ineligible/Keep` → a verb that can never propose anything. The plan calls the **same function
   the create side calls**.
2. **Containment tightening** (`verdict.rs`, `IneligibleReason::NotAManagedWorktreeName`).
   Behavioural, and it only ever NARROWS the removable set. With nothing configured cyrup's base
   IS the shared system temp dir, so "contained in /tmp" is near-vacuous; containment gains a
   second conjunct, the `cyrup-worktree-` leaf prefix `build_worktree_path` itself writes.
3. **`samePath`'s Windows `dev`/`ino` fallback is not ported** (`paths.rs`). Premise verified in
   this repository: the README gates name `wasm32-wasip2` and the host triple only, no
   `*-pc-windows-*` target is built anywhere, and `cfg(windows)` code nothing compiles is code
   that rots into false coverage.
4. **The plan reads manifests through `crate::handoff::read_manifest`** (`metadata.rs`), not
   through a second laxer parser as pi does (`:315-326` + per-group warnings at `:387-399`). Two
   reasons, both stated: reuse (a second parser for the same file is exactly the drift the sibling
   module's strictness exists to prevent), and identical safety (pi's per-record warnings and this
   reader's whole-file rejection both end with no `Safe` entry for the affected worktrees). What
   differs is granularity, and the warning says so.
5. **`RunState` is a closed enum where pi's is an open string union** (`metadata.rs`,
   `read_status_beside_manifest`). cyrup has no `partial`/`rejected`, so a plain serde read would
   turn pi's *"owning run has unknown state 'X'"* into *"invalid async status"*. The read falls
   back to the raw object and keeps pi's sentence when it carries a string `state` + `runId`. The
   closed enum stays, because it is what makes `terminal_run_state` exhaustive.
6. **`current_worktree_patch` uses the IDENTICAL argv to `capture_worktree_diff`**
   (`spawn/worktree.rs`), not pi's `MACHINE_PATCH_OPTIONS`. The caller compares the two byte for
   byte (pi `:341`); adding `--default-prefix`/`--binary` on the re-capture side only would make
   that comparison fail for every patch cyrup ever captured, turning gate #36 into a permanent
   `Ineligible/Keep` — fail-safe, but dead. Adopting pi's machine options on BOTH sides is a
   follow-up. Also recorded: a cyrup-captured patch comes from a `git add -A`-staged index, and
   such a worktree is `Dirty/Keep` at gate #33 before #36 is reached, so #36 is reachable today
   only for COMMITTED divergence.

### The one place this area and `[EXEC — manifest]` disagree, stated rather than papered over

`[AUG — cleanup]` §4.6 argued the manifest MIRROR types should NOT carry
`#[serde(deny_unknown_fields)]`, with a verified premise: upstream has added optional keys to
these records across versions (`types.ts:459` `provider?`, `:461` `naming?`, `:432` `workflowKey?`,
`:435` `lane?`, `:500` `laneBindings?`). The manifest sibling landed `deny_unknown_fields`
uniformly and flagged it as revisitable.

**I did not churn the sibling's landed module**, and here is the consequence, precisely: a
manifest written by pi (or by a newer cyrup) carrying `cleanup.tasks[].provider`/`naming` is
refused by `read_manifest`, the plan emits a warning for it if the caller named it explicitly, and
it contributes **no entries** — i.e. nothing is proposed for removal. That is fail-safe, not
unsafe. It is also narrow in practice: cyrup writes `.cyrup-subagents/` and pi writes
`.pi/subagents/` (`shared/artifacts.ts:6` @v0.68.0), so the only real scenario is an older cyrup
reading a newer cyrup's file. If
`provider`/`naming` land on `WorktreeCleanupTask` (the `[AUG — surface]` sibling owns
`ManagedWorktreeProvider`/`WorktreeNaming`), this stops mattering entirely. Flagging, not fixing,
was the call.

### Tests

**Reachability (the bar):** `crates/cyrup-ext-subagents/src/extension/tool/worktree_cleanup_tests.rs`
— 12 rows, every one of them through `Tool::execute` with raw JSON params. **No row constructs a
`spawn::cleanup_plan` value directly.** In-crate rather than in `cyrup-it`, because `cyrup-it` is
`required-features = ["it"]` and off the `cargo test --workspace` merge gate, so a reachability
test that lived only there would not gate anything.

The load-bearing row is `a_worktree_with_untracked_work_is_never_proposed_for_removal`. It builds a
real repo, a real `git worktree add`, and a real manifest, drops an untracked file in the
worktree, **and sets `status.showUntrackedFiles = no` in it** — an ordinary operator setting under
which a bare `git status --porcelain=v1` prints nothing at all. It then asserts, on one run, that
the entry appears under *"Will keep"* with pi's sentence, does NOT appear under *"Will remove"*,
that the persisted plan says `dirty`/`keep` with **no `willDeleteBranch` key**, and that the plan
file does not contain the untracked filename. Its positive control,
`a_clean_preserved_worktree_is_proposed_for_removal`, proves the verb can still say `remove` — so
"never propose removal" is not satisfiable by proposing nothing — and additionally asserts that the
worktree and branch both still exist afterwards.

**Mutations run, and the assertion each one broke:**

| mutation | failing assertion |
|---|---|
| drop `--untracked-files=all` from the status probe | `a_worktree_with_untracked_work_is_never_proposed_for_removal` — the fixture reads clean and the entry goes `Safe/Remove` |
| `Verdict::decision()` maps `Dirty → Remove` | same row, on `state`/`decision` |
| replace the routing arm's body with a canned plan-only string | 8 of 12 rows (no plan file, no entries) |
| `foreground_run_ownership` always answers `Terminal` | `an_unowned_foreground_run_is_not_provably_terminal` |
| delete the strict-child base-dir containment gate | `a_worktree_outside_the_managed_base_dir_is_never_removable` |
| delete the `!allow_mutating_management` refusal | `child_safe_fanout_refuses_worktree_cleanup_before_validating_anything` |
| `inspect_run_state` always answers `Terminal` | `an_active_owning_run_keeps_its_worktree`, `a_non_terminal_child_keeps_the_worktree`, `an_unowned_foreground_run_is_not_provably_terminal` |
| delete the `write_atomic_json_creating_parent` call | 6 of 12 rows (no persisted plan to read back) |

All eight restored; the suite is green afterwards.

**Unit rows (10):** `parse_git_worktree_list` (detached clears a branch, `prunable` captured,
`locked` ignored, a trailing record flushes, `refs/heads/` stripped), `path_inside` (strictness,
a sibling with a shared prefix, an escape through `..`), `comparable_path` on a missing tail, and
the epoch-millis → ISO-8601 conversion against three `new Date().toISOString()` values including a
leap day. Plus `a_traversing_plan_id_does_not_deserialize`, which pins that `PlanId` refuses
`../../etc/passwd`, `.`, `..`, `a/b` and `""` on the way in off disk.

### NOT done in this step

* **`MACHINE_DIFF_OPTIONS` was not adopted on the CAPTURE side.** Delta 6 above explains why
  adopting it on one side only would be worse than not adopting it; doing both sides is a
  follow-up that changes what `capture_worktree_diff` writes, which is the manifest sibling's file
  and out of this step's blast radius.
* **No apply/execute phase**, by design — see the top of this section.
* **`worktree.discard`, `lane.status`, `lane.recordMerge`, `lane.recordSupersession`** and the
  `laneId`/`merge`/`supersession`/`lane` properties remain the `[AUG — surface]` sibling's. This
  step added only `handoffPath`/`repo`/`planId`, because this arm reads all three and
  `every_advertised_schema_property_is_read_outside_provided_keys` would otherwise go red.
* **In production, every plan is empty until a fan-out has actually run.** A freshly written
  manifest marks each task `reason: "cleanup pending durable handoff capture"`
  (`handoff::CLEANUP_PENDING_REASON`), and gate #21 makes exactly that string `Ineligible/Keep` —
  by design, upstream's design. The test fixture writes a settled manifest by hand so this area is
  testable independently. **This is not an inert module; it is a module whose input arrives one
  fan-out later.**

### Gates run

| gate | result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets --features test-fixtures -- -D warnings` | clean |
| `cargo clippy -p cyrup-ext-sdk --target wasm32-wasip2` | clean |
| `cargo clippy -p cyrup-it --features it --all-targets` | clean |
| `cargo nextest run --workspace --features test-fixtures` | **`Summary [108.341s] 10518 tests run: 10518 passed, 9 skipped`** (the step-1 baseline was 10496; the +22 are this area's) |
| `cargo nextest run -p cyrup-it --features it -E 'binary(subagents)'` | **`Summary [22.070s] 210 tests run: 210 passed, 0 skipped`** |

Disk note: the box hit ENOSPC twice. Deleted, both times, only regenerable intermediates under
`target/`: `target/debug/build/cyrup-it-95e9b8598c2df3fe` (a stale 188 MB build dir),
`target/doc`, and `target/debug/build/cyrup-it-87b505e3149e7da5/out/it-bins/debug/deps` (~7 GB of
intermediate rlibs). The linked `it-bins` BINARIES were left intact and
`CYRUP_IT_BIN_DIR=/home/user/cyrup/target/debug/build/cyrup-it-87b505e3149e7da5/out/it-bins/debug`
was exported for the two `cyrup-it` gates so the 7.7 GB relink was skipped.

## [EXEC — surface]

Step 3 of 3. The four remaining convergence verbs, the tool surface that advertises them, the
authority gate that guards the destructive one, and the worktree-mutation serialization that makes
a discard safe to run at all. Upstream read only via `git -C tmp/pi-subagents show v0.68.0:<path>`.

### What landed

**The verbs.** `SUBAGENT_ACTIONS` goes **43 -> 47**. `worktree.discard`, `lane.status`,
`lane.recordMerge` and `lane.recordSupersession` join `worktree.cleanup` (step 2) at pi's own
indices — contiguously between `mission.close` and `watchdog.status`, matching
`shared/types.ts:2801` @v0.68.0. All five dispatch through **one** `route_action` guard arm.

| new file | contents |
|---|---|
| `src/extension/tool/lane_actions.rs` | `LaneAction`, `LaneActionRefusal`, `LaneActionParams`, `CleanupPlanParams`, the shared resolvers, 7 unit tests |
| — | **+31 tests overall**: the workspace suite goes 10 518 -> 10 549 |
| `src/extension/tool/lane_actions_tests.rs` | 20 rows, every one through `Tool::execute`, against a real `git worktree add` and a real manifest |

**`handoff::discard_preserved` + `DiscardOutcome`** (`handoff/write.rs:539`) — pi
`parallel-handoff.ts:684-741`. `[EXEC — manifest]` left it for this step because this arm is its
only caller; it now has one.

**`spawn::worktree::worktree_turn`** (`spawn/worktree.rs:918`) — the worktree-mutation
serialization cyrup had none of, and the hard blocker `[AUG — surface]` §9.1 flagged.

**`workflows::{ManagedWorktreeProvider, WorktreeNaming, WorktreeNamingCollision}` + the three
`WORKTREE_STATUS_*` bounds** (`workflows/lane_metadata.rs:234-345`) — pi
`lane-metadata.ts:9-11,76-96`, now carried on `handoff::WorktreeCleanupTask`. That file's stale
*"has no consumer here"* sentence is deleted, because it has one.

**A SECOND authority consult** (`spawn/worktree.rs`'s `refuse_unauthorized_dirty_discard`) — pi
`worktree.ts:1171-1252`: the gate that refuses to `--force`-remove a worktree holding uncommitted
work, or one it cannot inspect at all.

### PRODUCTION call sites (file:line, post-`cargo fmt`)

1. **`extension/tool/routing.rs:1245`** — the `LaneAction::from_wire` guard arm in `route_action`,
   after the `mission.*` arm and before `schedule.*`, which is upstream's own dispatch order
   (`:6213-6293` sits above the policy-action chain at `:6294`). **THE dispatch**; without it
   nothing else in this feature is addressable. Child-safe gate first, then `laneId`, then
   `handoffPath` — pi's order, and the order is the point.
2. **`extension/tool/routing.rs:1870` `route_worktree_discard`** — the authority consult
   (`:6249`), the forbid refusal (`:6251`), the no-UI refusal (`:6255`), the confirm (`:6256`),
   the `Ok`-not-`Err` decline (`:6258`), the worktree turn (`:6261`), then `discard_preserved`.
3. **`extension/tool/routing.rs:1971` `route_lane_evidence`** — `lane.status`'s swallowed read
   (`:6282`) and run-id check (`:6283`); the two recorders and their **`details.parallelHandoff`**
   (`:6289`).
4. **`extension/tool/text.rs:302-306`** — `SUBAGENT_ACTIONS`, +4 entries. Feeds `schema.rs:359`'s
   action enum directly, so this is what lets a model NAME the verbs.
5. **`extension/tool/schema.rs:458,466,472,484`** — `laneId`, `merge`, `supersession`, `lane`.
   Each is load-bearing under `schema.rs:1093`: advertise it and the build is red until
   `src/extension/` reads `p.<field>`.
6. **`extension/tool/params.rs:156,167,171,180`** — the four fields on `SubagentToolParams`;
   `:372` `lane_action_params`, `:386` `cleanup_plan_params`; `provided_keys` gains four names.
7. **`extension/tool/routing.rs:2297`** — `route_parallel_mode` normalizes `p.lane` through the
   LANDED `crate::workflows::normalize_workflow_lane_metadata` and binds it on the group spec.
8. **`spawn/chain_graph.rs:226` `ParallelGroupSpec::lane`** -> **`:1765`** ->
   `publish_worktree_handoff` — the lane reaches `children[].lane` in the manifest, so what a lane
   CLAIMED and where it wrote survive the fan-out as durable evidence. On the spec rather than on
   `ChainRunContext` so one field serves the foreground walk and the detached runner (a spec is
   what crosses the process boundary).
9. **`registration/authority.rs:98`** — `"worktree.discard" => Some(Self::DiscardWorktree)`.
   `default_decision` already returned `Confirm`, so a default install prompts.
10. **`spawn/worktree.rs:918` `worktree_turn`**, taken by **`create_worktrees` (`:940`)** — pi's
    `setupActive` guard — by **`chain_graph.rs:1817` `publish_worktree_handoff`** — pi's "one
    turn" finalization rule — and by **`routing.rs:1931`**, the discard arm (pi `:6261`).
11. **`handoff/read.rs:83`** — a cleanup task's retained `naming` is validated on every manifest
    read, so a branch name carrying a newline cannot reach the manual `git branch -D` lines a
    discard renders.
12. **`missions/lifecycle.rs:431` / `:893` — NOT EDITED.** They already read
    `details.parallelHandoff.path`; emitting that key from the two recorders revives both.
13. **`resources/docs/tool-reference.md:45,47-49,110-115,128-182`** — the verbs, their parameters,
    and two new sections. This is the surface `action:"guide"` returns, i.e. how a model discovers
    the verbs exist at all.
14. **`extension/rpc/params.rs:122` — COMMENT ONLY**, the re-check note (§5.1).

### The three landed guards, all green

`registration/guide.rs:215` (every dispatched verb appears in the tool-reference page),
`schema.rs:845-900` (the action enum's exact values AND order) and `schema.rs:1093` (every
advertised property is read outside `provided_keys()`) all went red on the first edit and are
green now. **No name was added to `provided_keys()` to satisfy the third** — its own failure
message names that as the forbidden move.

### A latent defect the mutation testing found, and the fix

The first cut gave `LaneAction::authority_action()` its own `matches!(self, WorktreeDiscard)` AND
added `"worktree.discard"` to `AuthorityAction::for_tool_action`. Unwiring the table then broke
exactly one test — the one comparing the two tables — and **no end-to-end row**, because the
dispatch read the enum and never the table. Two tables for one question, with the dispatch reading
the half that could not be turned off. `authority_action()` now DELEGATES to `for_tool_action`, so
there is one table on the live path; re-running the same mutation breaks **8** tests.

### Rust shapes taken over upstream's

* **`LaneAction`** — five string comparisons in a 14-deep `if` chain become one enum and three
  exhaustive matches. `is_mutating()` IS pi's child-safe split; `authority_action()` IS pi's
  single consult; `requires_handoff_path()` IS pi's spread-vs-required distinction. A boolean
  field can be set wrong; a match arm on a five-variant enum cannot be forgotten.
* **`LaneActionRefusal`** — eight `throw`/early-return strings become one `thiserror` enum with
  pi's verbatim wording; three of them interpolate the verb and are shared across verbs.
* **`ManagedWorktreeProvider`** — a two-variant enum, so `"auto"` (a REQUEST, never an outcome) is
  unrepresentable in a record describing something already allocated.
* **`WorktreeNamingCollision`** — `"branch" | "path" | "both"` becomes an enum; pi's three-way
  `!==` chain (`lane-metadata.ts:89`) becomes the `Deserialize` derive.
* **`WorktreeNaming`** — pi's `assertKnownFields` (`:86`) is `deny_unknown_fields`; its six
  `boundedNonEmptyString` calls are `Bounded<N>` fields deserializing through `parse`. Only the
  `\n`/`\r`/NUL rule stays a function, because it belongs to pi's lane-metadata helper and not to
  the bound — folding it into `Bounded` would silently change nine other fields' contracts.
* **`DiscardAuthorization` / `DiscardAuthorizationKind`** — pi's inline `{kind, policy}` union
  becomes a struct + enum, and `kind` is two variants rather than a `bool` because the question is
  not "yes/no" but *which gate said yes*.
* **`worktree_turn()` is an RAII guard, not a callback.** pi needs `worktreeTurn` AND a
  `setupActive` flag because a caller can forget to await the turn; here the turn is a value a
  caller must hold, and dropping it — including on unwind — releases it, exactly as pi's `finally`
  does. pi's `setupPoison` latch is NOT ported and the reason is stated in the doc: it is set only
  from `WorktreeSetupError`'s settlement-unknown path, which cyrup's straight-line rollback has no
  analogue of, and a latch nothing can set is a gate that only looks like one.
* **Evidence stays a raw `Value` on the params struct** and is parsed at the recorder, because the
  ~12 rejections ARE the product surface. `parse_evidence` strips serde's synthetic
  `" at line N column M"` so the verbatim sentence stays verbatim; a unit test pins that.

### `[CYRUP-DELTA]`s written (premises grepped before writing)

1. **`ParallelGroupSpec::lane` is recorded onto EVERY child row**, where pi attaches `params.lane`
   to the one result its single path produces (`subagent-executor.ts:3734`) and derives per-child
   lanes from workflow children (`:4394`). cyrup's entry point for a run-level lane is a fan-out
   with N children; attaching it to child 0 alone would make `lane.status` report the other N-1 as
   belonging to no lane at all.
2. **The discard confirm BODY names the RESOLVED manifest path**, where pi interpolates the raw
   `handoffPath` (`:6256` uses `paramsWithResolvedCwd.handoffPath`; the path it acts on is
   absolutized at `:6260`). A relative path in a destructive prompt is ambiguous, and showing
   something other than what will be touched is the wrong half of the pair to show. The prompt
   TITLE and every other sentence stay verbatim.

Not deltas, and said so the next reader does not re-derive them: the enums, the newtypes, the
error enum and the RAII turn are the batch directive's own instruction, not divergences.

### The three deliberate decisions

**RPC — NOT widened.** Re-verified: `rpc.ts:65-73` @v0.68.0 is seven `schedule.*` entries and
`git show v0.68.0:src/extension/rpc.ts | grep -n 'lane\|worktree\|handoff'` returns nothing. The
symmetry argument's premise is FALSE — neither `rpc.ts`'s `spawn` nor `SUBAGENT_RPC_METHODS` has a
`worktree` parameter, so a bridge caller cannot create a manifest here and has nothing to converge.
Separately, `worktree.discard` is confirm-gated by default and a bridge caller is not the session
with the UI. One sentence appended at `rpc/params.rs:122` recording the re-check. **No
`[CYRUP-DELTA]`** — matching upstream is the default.

**Authority — the note itself was wrong, and is CORRECTED rather than satisfied.**
`registration/authority.rs:16-33` used to say *"whoever lands `worktree.discard` or
`destructiveCleanup` must wire them through"*. The first half is now discharged. The second was
never payable: `git grep -n destructiveCleanup v0.68.0 -- src` returns **exactly two hits, both
inside `policy/authority.ts`** (`:3` the list, `:18` the default) — upstream declares the action
and never consults it anywhere — and the only candidate verb, `worktree.cleanup`, is plan-only
(`:6217`) and removes nothing, so gating it would prompt a user to authorize a deletion that
cannot occur. The note now says that, with the grep as its evidence.

**`preflight` — NOT landed, filed rather than half-landed.** See "NOT done".

### Tests

**Reachability (the bar):** `extension/tool/lane_actions_tests.rs`, 20 rows, **every one through
`Tool::execute` with raw JSON params**. No row constructs a `LaneAction`, a `handoff::Manifest` or
a `DiscardAuthorization`. In-crate rather than in `cyrup-it`, for the reason
`worktree_cleanup_tests.rs` states: `cyrup-it` is `required-features = ["it"]` and off the
`cargo test --workspace` merge gate.

The load-bearing pair is `a_child_safe_fanout_may_read_its_lane_graph_but_not_record_evidence`:
on ONE fixture it asserts that `lane.status` **succeeds** from a child-safe tool and that all four
mutating verbs are refused with pi's verbatim sentence — so "refuse everything" cannot pass it.
The destructive verb has the same shape: `an_authorized_discard_removes_the_real_worktree_and_branch`
proves it really removes a real worktree and branch and rewrites the ledger, while
`a_forbidden_discard_refuses_and_touches_nothing`,
`the_default_policy_confirms_and_a_session_with_no_ui_refuses`,
`declining_the_confirm_is_ok_not_an_error` and
`a_worktree_the_manifest_can_no_longer_inspect_is_preserved_not_removed` prove it declines to.

**Mutations run, and the assertion each one broke:**

| mutation | failing assertion(s) |
|---|---|
| `LaneAction::is_mutating()` returns `true` for every verb | `only_lane_status_is_non_mutating`; `a_child_safe_fanout_may_read_its_lane_graph_but_not_record_evidence` (lane.status refused) |
| remove `"worktree.discard"` from `AuthorityAction::for_tool_action` | **8 rows**, incl. `the_default_policy_confirms_and_a_session_with_no_ui_refuses`, `an_authorized_discard_removes_the_real_worktree_and_branch`, `declining_the_confirm_is_ok_not_an_error` |
| the confirm decline returns `Err` instead of `Ok` | `declining_the_confirm_is_ok_not_an_error` |
| drop `details.parallelHandoff` from the two recorders | `record_merge_makes_the_lane_eligible_and_reports_the_manifest` — the key `missions/lifecycle.rs` reads |
| delete the five verbs from `SUBAGENT_ACTIONS` | `every_lane_verb_is_advertised_and_dispatches`, `subagent_tool_schema_exposes_the_full_pi_parameter_union` |
| delete the discard dirty/probe gate in `cleanup_worktrees` | `a_worktree_the_manifest_can_no_longer_inspect_is_preserved_not_removed` |
| `discard_preserved` never rewrites the manifest | `an_authorized_discard_removes_the_real_worktree_and_branch`, `a_second_discard_is_a_no_op_that_says_so`, `a_worktree_the_manifest_can_no_longer_inspect_is_preserved_not_removed` |

All seven restored; the suite is green afterwards.

**Unit rows:** 7 in `lane_actions.rs` (`from_wire`/`as_str` round-trip; the three orthogonal
predicates, each asserted for ALL five variants; the authority table keys on this family's wire
names; blank-vs-absent path resolution; the evidence sentence surviving serde), and 4 in
`workflows/lane_metadata.rs` (the naming record's literal on-disk spelling both ways, unknown
fields and an invalid `collision` refused, `"auto"` unrepresentable as a provider, the byte bound
enforced at parse time, and the control-character rule).

### Safety invariants, and what pins each

| invariant | pinned by |
|---|---|
| `lane.status` never becomes mutating | `only_lane_status_is_non_mutating` + the child-safe e2e pair |
| the child-safe gate runs BEFORE param validation | the same e2e row — none of its four refusal calls carries `laneId` or `handoffPath`, so a dispatch that validated first would answer differently |
| `worktree.discard` never runs without the authority consult | `the_default_policy_confirms_and_a_session_with_no_ui_refuses` (NO policy configured at all, so the fallback is exercised) |
| a `Confirm` with no UI is a refusal, never an implicit yes | same row |
| a decline is `Ok`, not `Err` | `declining_the_confirm_is_ok_not_an_error` |
| a discard never races a worktree setup | `worktree_turn`, taken by `create_worktrees` and by both finalization owners |
| paths resolve against the REQUEST cwd | `a_blank_handoff_path_resolves_to_none_not_to_the_cwd` + every e2e row |
| evidence reaches the recorder unmodified | `record_merge_makes_the_lane_eligible_and_reports_the_manifest` |
| a stored eligibility is never trusted | `a_forged_stored_eligibility_is_not_believed` |
| `lane.status`'s swallowed read does not spread | `lane_status_renders_a_missing_manifest_as_not_safe` (Ok) vs `lane_status_refuses_a_lane_that_is_not_this_manifests_run` (Err) |
| a worktree that cannot be inspected is never removed | `a_worktree_the_manifest_can_no_longer_inspect_is_preserved_not_removed` |
| a verb is never advertised without an arm | `every_lane_verb_is_advertised_and_dispatches` + the three landed guards |
| `worktree.cleanup` is never mapped to `DestructiveCleanup` | `only_worktree_discard_consults_the_authority_policy`, `the_authority_table_keys_on_this_familys_wire_names` |

### NOT done in this step, and why

* **The `preflight` tool-surface property** (`schemas.ts:350`). `workflows/preflight.rs` (1306
  lines) is landed and complete with no tool-param caller, and closing that was worth attempting
  here. It is NOT one line: upstream's `preflight` REQUIRES `workflowScript`
  (`subagent-executor.ts:5035-5036`), is normalized at `:5038`, attached to `details.preflight`
  (`:4921`) and to the async job state (`:5420`), and fed to
  `workflowPreflightWarnings(preflight, trace)` at `:5605`. That is real plumbing on the workflow
  launch path, which this area does not otherwise touch. Advertising the property with only a
  normalize-and-drop would be the exact "accepted, validated, ignored" failure this programme keeps
  shipping — so it is filed, not half-landed, exactly as `[AUG — surface]` §5.3 said to do if the
  boundary needed plumbing. **It remains a landed subsystem with no tool caller.**
* ~~**pi's `preserve`-intent captured-patch gate**~~ — **LANDED, see `## [FIX — safety]`.** The
  premise recorded here ("cyrup's harvest path has never had the `preserve` half") was true of
  `ee1f844` and false of this batch, which gave the harvest path its first destructive call to
  `cleanup_worktrees`. The gate is ported, with the `Preserve` payload it reads.
* **The dirty-worktree AUTHORIZATION arm is defence-in-depth, not a reachable refusal.** Reached
  from the `worktree.discard` dispatch, `Auto` always authorizes and `Confirm` always arrives as
  `Confirmed` (a `Confirm` that was not confirmed returns earlier), so only the PROBE arm can
  refuse there — and that one is reachable and tested. (The `preserve` intent's own gate, added in
  `## [FIX — safety]`, refuses on the harvest path and is separately tested.) The authorization arm exists because
  `cleanup_worktrees(setup, Discard{..})` is a `pub` primitive and upstream gates it there too.
  Stated rather than left for a reviewer to discover.
* **`write_setup_handoff`** (pi `:619-674`) — still unported, as `[EXEC — manifest]` recorded.
* **`DynamicGroupSpec` has no `worktree` flag**, so a dynamic fan-out still writes no handoff and
  carries no lane. Out of scope, as `[AUG — manifest]` §8.6 states.

### Gates run

| gate | result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets --features test-fixtures -- -D warnings` | clean |
| `cargo clippy -p cyrup-ext-sdk --target wasm32-wasip2` | clean |
| `cargo clippy -p cyrup-it --features it --all-targets` | clean |
| `cargo nextest run --workspace --features test-fixtures` | **`Summary [106.679s] 10549 tests run: 10549 passed, 9 skipped`** (the step-2 baseline was 10 518; the +31 are this area's) |
| `cargo nextest run -p cyrup-it --features it -E 'binary(subagents)'` | **`Summary [21.744s] 210 tests run: 210 passed, 0 skipped`** |

`cargo nextest run -p cyrup-it --features it` in full (all ~552) was NOT completed here, for the
reason `[EXEC — manifest]` recorded: the `cyrup-it::ext` wasm/`cargo build` rows take 15-85 s each
on this box. Nothing in this change touches `cyrup-it::ext`/`::bin`, and the `subagents` binary —
which holds every `it` test this change affects — is green above. Stated rather than claimed.

`CYRUP_IT_BIN_DIR=/home/user/cyrup/target/debug/build/cyrup-it-87b505e3149e7da5/out/it-bins/debug`
was exported for the two `cyrup-it` gates so the 7.7 GB relink was skipped. No `target/` artifacts
were deleted in this step.

### The five verbs, end to end, in one paragraph

`subagent({tasks:[…], worktree:true, lane:{key:"lane.a", claims:["src/a.rs"]}})` fans out into
isolated worktrees, publishes a manifest that records what each lane claimed, and reports its path.
`subagent({action:"lane.status", laneId, handoffPath})` renders whether removing those worktrees is
safe and why not — and a delegated, child-safe child can call it.
`subagent({action:"lane.recordMerge", …})` attests convergence with digest-bound evidence and files
a mission artifact. `subagent({action:"worktree.cleanup", mode:"plan"})` cross-checks git against
the manifest and proposes what is safe. `subagent({action:"worktree.discard", handoffPath})` asks
first, then removes what is left — and preserves anything it cannot prove is empty.

---

## [FIX — safety]

A `worktree: true` fan-out could force-delete a worktree holding uncommitted or untracked work, on
the ordinary production settle path, with nobody asked. The batch that added
`publish_worktree_handoff`'s `cleanup_worktrees` call created that reachability; this step closes
it, and closes the three defects that would have made the gate useless on its own.

### The defect, as it stood

`spawn/chain_graph.rs`'s `publish_worktree_handoff` — reached on EVERY `worktree: true` settle —
called `cleanup_worktrees(setup, Preserve { handoff_captured: true })`. Inside `cleanup_worktrees`,
the ONLY dirty-worktree gate was `if let WorktreeCleanupIntent::Discard { .. } = intent`, so
`Preserve` skipped it entirely: no `git status --porcelain`, no base diff, no captured-patch
validation. Control fell straight into `git worktree remove --force` and then `git branch -D`, both
unconditional.

The comment that justified omitting the gate read *"cyrup's harvest path has never had that gate and
giving it one is the harvest owner's change, not this one."* True at `ee1f844`, where
`cleanup_worktrees`'s only non-test caller was `create_worktrees`'s allocation-rollback arm. False
of this batch, which created the harvest caller. **That comment is deleted, not reworded**, and
`[CYRUP-DELTA] 6` above is withdrawn for the same reason: it stated the preserve payload was read
only by a gate "cyrup does not have", which was circular — cyrup did not have the gate BECAUSE the
payload was not carried.

### (a) The preserve gate — ported

Upstream: `src/runs/shared/worktree.ts:1196-1231` @v0.68.0, inside `cleanupSingleWorktree`.
Here: `spawn/worktree.rs:1551` `refuse_unsafe_cleanup` / `:1640` `refuse_uncaptured_preserve`.

`cleanupSingleWorktree`'s straight-line body is split into three Rust functions because the three
questions it interleaves are independent: *is there work here* (`probe_worktree_work`, `:1507`),
*may a harvest remove it* (`refuse_uncaptured_preserve`), *may an operator discard it*
(`refuse_unauthorized_discard`, `:1696`). The probe's result is the enum `WorktreeWorkProbe`
(`:1493`) rather than two loose exit codes, so the third state — **the probe did not answer** — is a
variant a `match` must handle instead of a pair of booleans a reader must remember to check.
`git diff --quiet` exits 1 for *differs*, so anything but 0 or 1 is unanswered, and unanswered is
never collapsed into clean.

The refusal is a typed `WorktreeCleanupTask` (`preserved: true`, `worktreeRemoved: false`,
`branchRemoved: false`) carrying upstream's two reason strings verbatim — *"worktree contains
changes that are not represented by a captured handoff patch"* and *"captured handoff patch failed
validation: &lt;err&gt;"* — because the manifest is a cross-implementation on-disk contract and a
reason string is part of it.

Removal of a worktree that holds work is allowed only when ALL of upstream's five conditions hold:
a capture row exists for it; that row carries no `error`; the `.patch` exists and is non-empty; the
MANIFEST records that patch (`handoff::handoff_records_patch`, `handoff/read.rs:395`, pi
`worktree.ts:1131-1149`); and a fresh re-capture is byte-identical to it
(`validate_worktree_patch_represents_current_worktree`). Each closes a distinct way the work
vanishes; the manifest check is the one that distinguishes *recorded durably* from *written to a
path nothing names*.

`SetupRollback` skips all of it (pi `:1171`): allocation failed before any child ran, so the only
thing in those worktrees is the allocation being unwound. The gate also re-runs
`remove_synthetic_paths_before_diff` first (pi `:1172-1176`), so cyrup's own `node_modules` symlink
cannot be the reason a worktree reads dirty.

### (b) The `Preserve` payload — restored

`handoff/model.rs:966` `Preserve(PreserveEvidence)`, `:994` the struct. Two fields,
`captured_diffs` and `handoff_manifest_path`, passed at `spawn/chain_graph.rs:1887` — the call site
that corresponds to pi's `{ kind: "preserve", capturedDiffs, handoffManifestPath }` at
`src/runs/background/subagent-runner.ts:4426` @v0.68.0.

`handoff_captured: bool` is **removed**, not kept alongside. It had no `false` caller in production,
and its doc's claim — that `false` "protects against orphaning" — was false: `cleanup_worktrees`
removed first and used the flag only to pick a reason string afterwards. What it was reaching for
is now real and is upstream's own mechanism: `handoff_manifest_path: None` means no durable record
exists, `handoff_records_patch` therefore answers `false`, and every worktree holding work is
preserved. The intent is no longer `Copy`, so `cleanup_worktrees` takes `&WorktreeCleanupIntent`.

Not ported: pi's third `preserve` field, `cleanupBlocker`. Its only upstream setter is
`subagent-runner.ts:4807`, the retained-child-resume path (*"retained child resume requires managed
worktree cwd"*), which cyrup has no analogue of. A field nothing can ever set is a gate that only
looks like one. Stated here rather than as a `[CYRUP-DELTA]` on a field that does not exist.

### (c) `git branch -D` no longer runs after a failed removal

`spawn/worktree.rs:1415`, `if worktree_removed { … }` — pi `worktree.ts:1253-1263`. Previously the
branch delete ran unconditionally, so a worktree that could not be removed could still lose the
branch holding the only reference to its commits. git normally refuses to delete a branch a worktree
has checked out, but a child that detached HEAD inside its worktree removes that protection —
exactly the case where the commits matter.

### (d) A failed capture is no longer written as a clean patch

`WorktreeDiff` gains `error: Option<String>` (pi `worktree.ts:79`); `empty_diff` takes it;
`diff_worktrees` fills it from the real capture error instead of discarding it; and
`handoff/write.rs:280` carries it into `Patch.error` instead of hardcoding `None`. Without this the
gate in (a) passes on precisely the worktrees it exists to protect, because
`captured.error.is_none()` is one of its five conditions and a failed capture was indistinguishable
from an empty one.

`capture_worktree_diff` also gained pi's post-capture `validate_worktree_patch` check
(`worktree.ts:1103-1104`): a patch that cannot be applied back is not a capture, and the harvest
removes the worktree immediately afterwards.

### Also fixed, same lens

- **`MACHINE_DIFF_OPTIONS` / `MACHINE_PATCH_OPTIONS`** (`spawn/worktree.rs:75`, `:91`; pi
  `worktree.ts:23-24`) now wrap every `git diff` this module takes — the stat, the numstat, the
  stored patch, the re-capture, and the base-diff probe. `--binary` is the difference between a
  patch and a note saying a patch was not taken; the other five make the output a function of the
  repository rather than of the running user's git config. Verified supported on this box's
  `git 2.43.0`, `--default-prefix` included. The `[CYRUP-DELTA]` on `current_worktree_patch` that
  deferred this is deleted: it said the capture and re-capture sides "must move together", which was
  correct, and they have now moved together.
- **`Preserve`'s dead `handoff_captured: false` branch** — gone with the field (see (b)).
- **`discard_preserved`'s pending filter** (`handoff/write.rs`) is left at upstream's behaviour and
  its doc corrected instead. Narrowing it would break the case it exists for: a crash between the
  two phase writes leaves exactly those `CLEANUP_PENDING_REASON` rows, and acting on them is the
  point. The doc now states what the `worktree_turn` mutex does and does not establish — it is
  process-local, as pi's chained promise is — and names what actually protects the work across
  processes: `cleanup_worktrees` re-probes each worktree at removal time rather than trusting the
  ledger's snapshot.
- **`registration/authority.rs`'s mapping test** was named
  `only_stop_steer_and_schedule_create_map_to_a_policy_action` and its doc said "anything else is
  ungated" — both falsified by this batch's `worktree.discard` → `DiscardWorktree`. Renamed
  `only_the_four_privileged_verbs_map_to_a_policy_action`, and its negative list now enumerates
  every remaining `LaneAction` wire name, so it pins the completeness its name claims.

### Tests, and the mutations that prove them

Three of the four decisive tests drive the REAL production harvest — `publish_worktree_handoff`
itself, not a hand-built intent — because the defect was as much in what that call site passes as in
what `cleanup_worktrees` does with it. A test that built the intent itself would keep passing while
the call site reverted.

| test | what it pins |
|---|---|
| `chain_graph::tests::a_harvest_whose_capture_failed_preserves_the_dirty_worktree_and_its_branch` | Real repo, real worktree, dirtied BOTH ways (an uncommitted modification AND an untracked file), capture made to fail the way it fails in the field — a stale `index.lock` in the worktree's own git dir. Asserts the directory, the modification, the untracked file and the branch all survive, and that the manifest records `preserved: true` with the refusal reason, `state: partial`, and `patch.error` present. |
| `chain_graph::tests::a_harvest_with_a_validated_captured_patch_removes_the_dirty_worktree` | The gate is not a blanket refusal: a captured, manifest-recorded, validated patch still clears it, and both kinds of work reach the durable patch. Without this, "preserve always" would pass the row above. |
| `chain_graph::tests::a_patch_no_manifest_records_does_not_clear_the_preserve_gate` | The manifest half of the evidence, checked independently of a SUCCESSFUL in-memory capture. |
| `worktree::tests::a_failed_worktree_removal_never_deletes_the_branch_holding_the_work` | (c). A committed child, HEAD detached inside the worktree, the worktree `git worktree lock`ed so removal fails. Asserts the branch still exists and still points at the child's commit. |
| `worktree::tests::a_captured_patch_carries_binary_content_not_a_note_about_it` | `--binary`. Asserts `GIT binary patch` in the patch AND applies it to a fresh checkout of the base commit, comparing the recovered bytes. |

Every existing dirty-worktree test exercised only the `Discard` intent, which is why none of them
caught this.

**Mutations run** (each reverted afterwards; no mutation is in the tree):

| mutation | result |
|---|---|
| `publish_worktree_handoff`'s `cleanup_worktrees` call → the ungated intent (`SetupRollback`) | `a_harvest_whose_capture_failed_…` **FAILS**: *"the worktree holding uncaptured work must still be on disk: /tmp/…/cyrup-worktree-harvestgate-0"* |
| `git branch -D` back to unconditional | `a_failed_worktree_removal_…` **FAILS**: *"the branch delete must not even be attempted: … worktree_removed: false, branch_removed: true"* |
| `--binary` dropped from `MACHINE_PATCH_OPTIONS` | `a_captured_patch_carries_binary_content_…` **FAILS** |
| `Patch.error` back to hardcoded `None` | `a_harvest_whose_capture_failed_…` **FAILS** |
