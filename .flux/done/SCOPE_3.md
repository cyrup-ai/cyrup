---
stage: qa
status: completed
updated: 2026-09-14 06:00
---

# SCOPE_3 — wait completions resolve through the session index (INDEX)

OBJECTIVE: port [`wait-completions.ts`](../../../workspace/pi-subagents/src/runs/background/wait-completions.ts)
**completely** — the projector, its store, its path builders, its return type, and every producer
whose absence would otherwise force a field to be `None` forever.

**This file is now an index.** The work is split across `SCOPE_3a` … `SCOPE_3j`, each independently
executable. This file holds the shared premise, the shared TYPE CONTRACT (§A — read it before
writing a line of any task), the verified anchors and the dependency order that all of them assume.

---

## 0.0 ⚠ RE-AUGMENTED 2026-09-14 — **THIS INDEX'S OBJECTIVE IS LANDED**

> Branch `claude/subagents-scope`, cut from `main` at `d53763b`. Every claim below was re-checked
> against the tree as it is NOW, by opening the files. **No `git` command was run** (flux rule);
> where the old text asserted a commit-vs-working-tree distinction, that distinction is simply gone
> — every anchor named here is an ordinary file on disk.
>
> **Read this section before believing any line number elsewhere in this file. Every line number
> in §0–§4 below predates ~15 k LOC of landed work and almost all of them are now wrong.** They are
> preserved (per flux's "add precision, never remove scope") with a corrected value beside them.

### 0.0.1 What exists now

`wait-completions.ts` (213 LOC) is **fully ported**, in four files under
`crates/cyrup-ext-subagents/src/background/wait_completions/` (675 LOC total):

| file | LOC | contents |
|---|---|---|
| `wait_completions/mod.rs` | 45 | facade + the module narrative; re-exports all seven public items |
| `wait_completions/project.rs` | 365 | `WaitCompletion`, `WaitCompletionChild`, `CompletionUsage`, `CompletionProjectionError`, `to_wait_completion`, `project_structured_output`, `completion_usage` |
| `wait_completions/record.rs` | 152 | `WaitCompletionStore` + its `CompletionObserver` impl |
| `wait_completions/collect.rs` | 113 | `collect_wait_completions` — the three-rung resolution |

Nine of the eleven rows in §1 have executed. **Two remain open, both still in `.flux/todo/`:**

* **`SCOPE_3j`** (`todo/SCOPE_3j.md`, `stage: aug, status: done`) — model-exclusion registry. Its
  insertion point is now a **literal comment block in the landed code**:
  `exec/fallback.rs:1721-1725`, inside `classify_attempt`, reading
  `── SCOPE_3j inserts record_retryable_model_failure HERE ──`. 3b landed the §A types that make it
  a one-line change, exactly as this index predicted — and it also recorded the one thing 3j must
  respect: *"the recording is a FILE WRITE … so it cannot live inside this pure function: 3j must
  carry the decision out on a `LadderStep` payload and let the shell perform it."*
* **`SCOPE_4`** (`todo/SCOPE_4.md`, `stage: aug, status: done`) — durable completion replay, the sole
  producer of `archivePath`. Its absence is **declared in the landed code**, not forgotten:
  `wait_completions/project.rs:44-48` (`WaitCompletion::archive_path`, "Its producer — SCOPE_4's
  durable completion-replay writer — has not landed yet") and `collect.rs:104-107` (pi's third rung
  `readCompletionReplay` is stubbed with a named TODO for SCOPE_4). §0.3's "one legitimate
  dependency" was correct and is still the only one.

Every other `WaitCompletionChild` field has a real producer — see the corrected field map in §2.

### 0.0.2 Landed-status of the nine, with proof

| § 1 row | landed? | proof (file:line, current tree) |
|---|---|---|
| **3a** `SingleResult` completeness | ✅ | `exec/run_result.rs`: `turns:46`, `session_file:187`, `output_state:211`, `structured_output_path:225`, `artifact_paths:234` |
| **3b** `context_overflow` + §A foundation | ✅ | `exec/run_result.rs:97` `context_overflow`; `exec/fallback.rs`: `lower!` macro `:456`, `LowerLiteral` `:490`, `LoweredLine` `:708`, `AttemptNote` `:1038`, `LadderStop` `:1648`, `LadderStep` `:1667`, `classify_attempt` `:1699`, `LadderControl` `:1749` |
| **3c** mutation evidence + timeout recovery | ✅ | `exec/mutation_evidence/` (6 files: `mod/project/repo/snapshot/summary/types.rs`); `run_result.rs:82` `timeout_recovery: Option<TimeoutRecoverySummary>` |
| **3d** workflow foundations | ✅ | `workflows/bounded.rs` (160), `workflows/key.rs` (150), `workflows/child_summary.rs`, `workflows/chat_progress.rs`, `workflows/resources.rs`; `RunMode::Workflow` at `background/state.rs:22-27` |
| **3e** workflow gates | ✅ | `workflows/preflight.rs`, `workflows/checklist.rs`, `workflows/host_command.rs` (1594) |
| **WORKFLOW_1** scripted workflow | ✅ | `workflows/scripted/` (engine.rs ≥ 2949 LOC, types.rs, …) |
| **WORKFLOW_2** the vertical slice | ✅ | `extension/tool/routing.rs:523` `route_workflow_mode`, `:700` calls `crate::workflows::scripted::run_workflow_script`; `extension/tool/params.rs:89` documents the tool parameter |
| **WORKFLOW_3** receipt + settlement | ✅ | `workflows/receipt.rs`, `workflows/settlement.rs`; `WaitCompletion::workflow_receipt_path` populated (`project.rs:38-39`), appended inside `WaitOutcome::new` (`wait.rs:599+`) |
| **WORKFLOW_4** `wait_completions/` + the two `result_index` exports | ✅ | the four files above; `result_index/mod.rs:110-115` now exports **both** `result_payload_path_for_session_run` (`locate.rs:361`) **and** `fallback_result_payload_path_for_session_run` (`locate.rs:397`) |
| **WORKFLOW_5** `WaitOutcome` + rewires | ✅ | `wait.rs:531` `WaitVerdict`, `:578` `WaitOutcome`, `:629` `is_error`, `:653` `details`, `:690` `usage`; `auto_drain.rs:79` `wait_all -> WaitOutcome`; `extension/wait_tool.rs:178-191` keeps text + usage + details |

### 0.0.3 The §A type contract HELD — all five gates pass, with two caveats

Run verbatim from the repo root (`C=crates/cyrup-ext-subagents`), 2026-09-14:

* **gate 1** (key grammar declared once) — **one hit, not a violation**:
  `extension/executor/workflow.rs:995`, a doc comment *quoting* the pattern
  (`` pattern is `/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/` (`prelude.js:87`) ``) while explaining the
  engine's own check. The only *code* copy is `workflows/key.rs`. **Amend the gate** to
  `| grep -v '^\s*//'` or accept a comment-only hit; do not "fix" it by deleting the comment.
* **gate 2** (no `derive(Deserialize)` on a §A newtype) — **empty. Clean.**
* **gate 3** — `workflows/key.rs` has exactly **1** hand-written `impl<'de> serde::Deserialize<'de>`;
  `grep -rn 'deserialize_with' $C/src/workflows/` is **empty**. The AMENDED form of this gate (§A.3's
  2026-09-06 amendment) is the one that passes; the pre-amendment form would fail the compliant
  implementation. `workflows/bounded.rs:59` and `:104` carry the same idiom for `Bounded<N>` /
  `BoundedUtf16<N>`.
* **gate 4** (no `_ =>` on a §A domain enum) — **empty. Clean.**
* **gate 5** (no second bounded-string helper) — **four hits, none a violation**:
  * `workflows/preflight.rs:193` `normalize_display_string` — a *normalizer* that **calls**
    `BoundedUtf16::<256>::parse` as its length predicate; its doc comment (`:183-187`) argues
    exactly this and cites "SCOPE_3e §0.6, SCOPE_3 §3 gate 5". Compliant by construction.
  * `background/watch/observer.rs:119` `bounded_completion_summary` and
    `extension/executor/foreground_history/record.rs:149` `bounded_tail` (+ its test at `:412`) —
    **truncating** helpers, i.e. §A.4's *other* family, which §A.4 explicitly says are free
    functions and NOT `Bounded<N>`.
  * **Amend the gate** to also exclude `workflows/display_text.rs` and to say that a hit whose body
    delegates to `Bounded`/`BoundedUtf16` passes. The truncating family landed as
    `workflows/display_text.rs:153` `truncate_display` (UTF-16 units) and `:175` `truncate_to_bytes`
    (UTF-8 bytes, reserves room for the ellipsis), plus `:82` `sanitize_display_text` — exactly the
    split §A.4 prescribed.

`WorkflowKey` is consumed by **24** files today (`workflows/` × 13, `background/` × 5,
`extension/executor/` × 4, `missions/` × 2, `registration/profiles.rs`) — §A.3's "single
highest-value type" claim is borne out.

### 0.0.4 Upstream has MOVED since these specs were written

The oracle is `/home/user/cyrup/tmp/pi-subagents` (a read-only clone). **The
`../../../workspace/pi-subagents/…` links in this file and its siblings do not resolve on this
machine** — `/home/user/workspace` does not exist; substitute `tmp/pi-subagents`.

| file | LOC in this spec | LOC now |
|---|---|---|
| `background/wait-completions.ts` | 213 | 213 (unchanged) |
| `background/completion-replay.ts` | (SCOPE_4: 287) | 287 (unchanged) |
| `shared/model-exclusions.ts` | **374** | **385** |
| `background/subagent-wait.ts` | — | 842 |
| `background/result-watcher.ts` | — | 795 |
| `background/result-files.ts` | — | 519 |
| `workflows/*.ts` (9 files) | **4418** | **4712** — `scripted-workflow.ts` **2284 → 2553**, `workflow-checklist.ts` 436, `workflow-receipt.ts` 384, `workflow-preflight.ts` 297, `workflow-settlement.ts` 249, `host-command.ts` 235, `workflow-resources.ts` 236, `workflow-child-summary.ts` 170, `chat-progress.ts` 152 |

The `KEY_PATTERN` census in §A.3 is likewise low now: `grep -rn` for the grammar finds **17 sites
across 14 files** upstream, not 11 across 9. §A.3's argument is *strengthened*, not weakened.

### 0.0.5 What a future session should do with this file

1. **Do not re-run this family.** Re-executing any of the nine rows would duplicate landed code and
   would trip the §A gates it was written to satisfy.
2. The only live work this index still points at is **`SCOPE_3j`** and **`SCOPE_4`**, both of which
   have their own augmented task files and their own in-code insertion markers (§0.0.1).
3. Treat §1's dependency graph as **history**. Its one still-binding rule is the WORKFLOW_2 lesson
   (§1's inserted block): *a task's DoD MUST name a command a human can run.* That rule outlived the
   family and applies to `SCOPE_3j`/`SCOPE_4`.

---

## A. THE TYPE CONTRACT — binding on every task in this family

Added 2026-09-06 14:20 after a type-driven design review of SCOPE_3b's landed code. **This section
overrides any sketch in an individual task file that contradicts it.**

### A.0 Why this exists, and why it is not a refactor

Upstream is TypeScript. It validates one identifier grammar in **eleven** places across nine files
and carries **nine** independent bounded-string helpers, because in TS a shared newtype costs more
than a copied regex:

```bash
$ grep -rln "A-Za-z0-9\]\[A-Za-z0-9\._-\]{0,127}" pi-subagents/src | wc -l   # 9 files, 11 sites
$ grep -rn "function bounded\|normalizeDisplayString\|function boundedText\|boundedString" \
    pi-subagents/src/workflows pi-subagents/src/runs/shared | wc -l          # 9 impls
```

In Rust that trade runs the other way. **A transliteration inherits all twenty duplicates.** Most of
this family is unwritten, so adopting shared types now costs nothing and makes the tasks *shorter*;
adopting them later means writing the duplicated version first and paying again to remove it. That
is the whole argument — there is no separate "type cleanup" task in this family and there must never
be one. **Each type below is born in the first task that needs it and consumed by the rest.**

This is explicitly licensed by `CLAUDE.md`: *"Port the BEHAVIOUR; the transport underneath it is
free to differ where it must, and the difference gets stated in the doc comment with the reason."*
Every type here is behaviour-preserving and wire-compatible. None of them change a byte of JSON.

### A.1 The five rules

| Situation | Representation | Never |
|---|---|---|
| A value with a format rule | newtype, private field, fallible constructor at the boundary | `String` + a doc comment saying "must already be validated" |
| A set of mutually exclusive outcomes | named domain enum, exhaustive `match` | a cluster of `bool`s, or a `_ =>` arm |
| An expected business outcome | a named enum variant | `Err(String)`, an empty `Vec`, or a sentinel |
| A technical failure that aborts | `Result<T, E>` | folding it into the outcome enum |
| A decision tangled with `.await`/IO | pure `fn` returning a decision enum, executed by a shell | a decision expressed as source-line order between `break`s |

**Precedence is never expressed as statement order across a long function.** If the order of two
checks is load-bearing, they go in one pure function returning one enum. SCOPE_3b's original DoD
said *"verify by reading the order, not by the compiler: both placements build"* — that sentence is
the defect this contract exists to prevent, and it must not reappear in any task.

### A.2 The shared vocabulary — who declares it, who consumes it

| Type | Declared in | Task | Consumed by |
|---|---|---|---|
| `LowerLiteral` + `lower!()` | `exec/fallback.rs` | **3b** | the two pattern tables |
| `LoweredLine<'_>` | `exec/fallback.rs` | **3b** | `line_matches` |
| `AttemptNote` (enum + `Display`) | `exec/fallback.rs` | **3b** | `attempt_runner`, `exec/mod.rs`, 3j |
| `LadderStep` (enum) | `exec/fallback.rs` | **3b** | `run_fallback_ladder`, **3j** |
| `LadderStop` (enum) | `exec/fallback.rs` | **3b** | `FallbackOutcome` only — **not** `SingleResult` |

> **CORRECTED 14:29.** This table previously read `TerminalReason | exec/run_result.rs | 3b |
> consumed by 3c, WORKFLOW_3, WORKFLOW_4`. That was wrong on every count and would have made §A produce the
> duplication it exists to forbid — **cyrup already has the run-status resolver**:
> `resolve_subagent_result_status` (`tui/intercom.rs:286`) is a complete port of upstream's
> `resolveSubagentResultStatus` (`result-intercom.ts:20-40`), over the existing five-variant
> `SubagentResultStatus` (`:127`), with upstream's exact precedence
> `detached ▸ stopped ▸ paused ▸ completed ▸ failed`.
>
> Two DIFFERENT questions were conflated:
>
> * **"why did the ladder stop trying models?"** — 5 answers, derived from `AttemptSignal` alone,
>   internal to `fallback.rs`. That is `LadderStop`, and it is the only new type.
> * **"what is this run's terminal status?"** — already answered by
>   `resolve_subagent_result_status`, which folds in `stopped`/`interrupted`/`detached` and an
>   unexplained process signal. **3c/WORKFLOW_3/WORKFLOW_4 consume that existing function.** Adding a
>   `SingleResult::terminal_reason()` would be a second, competing resolver.
>
> `LadderStop` deliberately cannot represent `Interrupted` or `Stopped`: `interrupted` lives on
> `AttemptRecord` (`exec/mod.rs:1118` reads `record.interrupted`), not on `AttemptSignal`, and a
> stop is observed outside the ladder entirely. This is a correctness property, not a limitation —
> see 3b §P2.4.
| `assemble_delivered_output` (pure) | `exec/mod.rs` | **3c** | WORKFLOW_3 |
| `Bounded<const N: usize>` | `workflows/bounded.rs` | **3d** | 3d, 3e, WORKFLOW_1, WORKFLOW_3 |
| `WorkflowKey` | `workflows/key.rs` | **3d** | 3d, 3e, WORKFLOW_1, WORKFLOW_3, WORKFLOW_4 |
| `ContainedPath` | `exec/output.rs` | **3e** | 3e, WORKFLOW_1 |

A task that needs a type it does not declare **depends on the task that does**. That dependency is
now part of the family's ordering and is recorded in §1.

### A.3 `WorkflowKey` — the single highest-value type in the family

One grammar, `^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$`, is validated at **eleven** upstream sites:
`workflow-child-summary.ts:65,84,103,156` · `scripted-workflow.ts:1401,1441,1594,1602,2117` ·
`workflow-preflight.ts:11` · `workflow-resources.ts:10` (as `RESOURCE_NAME_PATTERN`) ·
`workflow-receipt.ts` · `workflow-child-permit.ts` · `lane-metadata.ts` · `missions/store.ts` ·
`missions/workflow-state.ts` · and once as a JSON-schema `pattern` at `extension/schemas.ts:111`.

Every one of those is a place a cyrup port could forget the check. Declare it **once**:

```rust
/// A workflow lane / resource / receipt key — pi's `KEY_PATTERN`
/// (`scripted-workflow.ts:12`, and ten identical copies elsewhere; see SCOPE_3 §A.3).
///
/// Grammar: first char `[A-Za-z0-9]`, then 0..=127 of `[A-Za-z0-9._-]`. Parsed ONCE at the
/// boundary; every downstream signature takes `&WorkflowKey` and performs no check, because there
/// is no way to obtain one that has not been checked.
///
/// No `regex` dependency: the grammar is a first-char test plus a `chars().all(..)` over a bounded
/// count. No `Deserialize` derive — see `try_from` / the `deserialize_with` note below, because a
/// derived impl would bypass the parser and reintroduce every one of the eleven sites.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
#[serde(transparent)]
pub struct WorkflowKey(String);

impl WorkflowKey {
    /// The ONLY fallible constructor. `Err` carries upstream's own message verbatim, because four
    /// call sites surface it to the author of a workflow script.
    pub fn parse(value: &str) -> Result<Self, WorkflowKeyError> { /* … */ }
    #[must_use] pub fn as_str(&self) -> &str { &self.0 }
    /// pi `key.startsWith(`${laneKey}.`)` — the lane-root relation, which is a property OF the
    /// grammar and therefore lives with it rather than in `preflight.rs` (SCOPE_3e) and
    /// `child_summary.rs` (SCOPE_3d) separately.
    #[must_use] pub fn is_descendant_of(&self, root: &WorkflowKey) -> bool { /* … */ }
}
```

**Serde is the bypass to watch.** `#[derive(Deserialize)]` on a newtype does *not* call `parse`.

> **AMENDED 2026-09-06 by SCOPE_3d (§0.9) — binding on 3e/WORKFLOW_1/WORKFLOW_3/WORKFLOW_4.** The mechanism is a
> hand-written `impl<'de> serde::Deserialize<'de>` on the newtype itself, routing through `parse`
> (`String::deserialize` → `parse` → `de::Error::custom`) — the idiom `identity/session_id.rs:85`
> and twelve sibling impls established after this section was written. It has no bypass (there is
> no per-field attribute to forget) and exactly one site to maintain, and it is NOT a derive, so
> §3 gate 2 passes either way. The original prescription here —
> `#[serde(deserialize_with = "crate::workflows::key::deserialize")]` on every deserialized
> `WorkflowKey` field — is **superseded**: per-field attributes would reproduce, inside cyrup, the
> eleven-copies-of-one-rule pattern §A.0 exists to eliminate. Do not add `deserialize_with` for a
> §A newtype, and (unchanged) do not derive `Deserialize` on one. §3 gate command 3 is amended to
> match.

### A.4 `Bounded<const N: usize>` — one helper, not nine

Upstream's nine bounded-string helpers split into exactly **two** behaviours, and conflating them is
a real bug class:

* **reject** when over the limit — `bounded` (`workflow-child-summary.ts:41`, returns `undefined`),
  `normalizeDisplayString` (`workflow-preflight.ts:50`, throws), `boundedNonEmptyString`
  (`lane-metadata.ts:20`), `boundedString` (`parallel-handoff.ts:212`);
* **truncate** — `boundedText` (`host-command.ts:45`, appends `"..."`), `text`
  (`workflow-checklist.ts:118`, hard slice to `MAX_TEXT`).

They also disagree on the *unit*: `workflow-preflight.ts:57` compares `.length` (**UTF-16 code
units**) while `:121` and `host-command.ts:41` use `Buffer.byteLength` (**UTF-8 bytes**). A single
`Bounded<N>` that picks one unit would silently diverge from upstream on non-ASCII input.

```rust
/// A non-blank string proven to fit `N` UTF-8 bytes. REJECTS — the `bounded`/`normalizeDisplay`
/// family (SCOPE_3 §A.4). The truncating family is `truncate_to_bytes` / `truncate_display`, which
/// are free functions, NOT this type: a truncated value has no invariant worth carrying.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(transparent)]
pub struct Bounded<const N: usize>(String);

impl<const N: usize> Bounded<N> {
    /// `None` for blank-after-trim or over-length — pi `bounded` (`workflow-child-summary.ts:41`).
    /// Returns the ORIGINAL value, not the trimmed one; the trim is only the emptiness test.
    pub fn parse(value: &str) -> Option<Self> { /* … */ }
}

/// UTF-16 code units, for the ONE limit upstream measures that way
/// (`workflow-preflight.ts:57`). Distinct type so the two units can never be transposed.
pub struct BoundedUtf16<const N: usize>(String);
```

The per-field limits are then type-level and cannot be mixed up:
`Bounded<256>` for `runId`/`agent`/`sessionName`/`model`, `Bounded<32>` for `thinking`,
`Bounded<4096>` for `parentToolCallId`/`workflowRunId`, `BoundedUtf16<256>` for a preflight display
string.

### A.5 What none of this buys

Stated so no task overclaims in its DoD:

* A `WorkflowKey` does not prove the key names a lane that exists.
* `Bounded<N>` does not survive `String::from(k.as_str())` — a task that unwraps to a primitive and
  passes it on has discarded the guarantee. Do not unwrap immediately after parsing.
* `#[derive(Deserialize)]` bypasses every constructor here (§A.3).
* An exhaustive `match` on a domain enum does not prove the *precedence* inside the function that
  produces it — it prevents divergence between decision and execution, not a wrong decision.
* None of this addresses crash/restart, partial external side effects, or a concurrent writer.

---

## 0. Why this is a split, and why nothing in it is optional

### 0.1 The original premise was wrong

The task as first written said *"`wait` resolves a completion's payload at the public path
unconditionally"*. **There is no such place.** cyrup's `wait` reads **no payload at all**:

```
$ grep -n "paths.result" crates/cyrup-ext-subagents/src/background/wait.rs
# only inside #[cfg(test)] mod tests' Fixture::settle helper
```

In production `wait_for_subagents` (`background/wait.rs:434-745`) re-reads each initially-tracked
run's `status.json` (`terminal_runs_for`, `:349-369`), folds it into bucket counts
(`summarize_terminal_runs`, `:372-405`), and returns a rendered sentence. Nothing more.

> **CORRECTED 2026-09-14 — the premise is now HISTORY; `wait` DOES read payloads.**
> `wait.rs` is **2893 lines** (was ~850). Current anchors:
>
> | thing | was | **is now** |
> |---|---|---|
> | `wait_for_subagents` | `:434` | **`wait.rs:704`** |
> | `terminal_runs_for` | `:349` | **`:438`** |
> | `summarize_terminal_runs` | `:372` | **`:461`** |
> | `WaitParams` / `WaitDeps` | `:200`/`:217` | **`:200` / `:217`** (unchanged) |
> | `WaitDeps::stop_on_attention` / `fail_on_failed_runs` / `fail_on_attention` | `:217`/`:221`/`:223` | **`:262` / `:266` / `:268`** |
> | the three builders | `:273`/`:281`/`:289` | **`:362` / `:370` / `:378`** |
> | the two terminal returns | `:697-703`/`:738-744` | folded into `WaitVerdict::Resolved { failed_runs, attention_runs, reported_as_error }` (**`:556-566`**, `reported_as_error` at `:565`) and returned via `WaitOutcome::new` at **`:1095`** and **`:1149`** |
>
> The `grep -n "paths.result"` result still holds *literally* (that spelling appears nowhere), but
> the conclusion drawn from it does not: WORKFLOW_4/5 landed, and production `wait` now calls
> **`super::wait_completions::collect_wait_completions(&terminal_runs, &deps.wait_completions,
> &deps.results_dir)` at `wait.rs:1027-1036`**, whose `Err` becomes
> `WaitOutcome::plain(error.to_string(), WaitVerdict::CompletionsFailed)` — never a propagated
> error (`:1034`). The recovery note 3c/WORKFLOW_5 owed is `format_completion_recovery`
> (**`wait.rs:1190`**), called at **`:1058`**, alongside `format_observed_completions` (**`:1166`**).
> `WaitDeps` gained **`wait_completions: Arc<WaitCompletionStore>`** at **`wait.rs:251`** — and note
> its deliberate non-`Option`-ness, argued in its own doc (`:237-250`): an empty store *is* "this
> process has consumed nothing", unlike `completion_bus` (`:234`) where the two states differ.

### 0.2 The tree moved. SCOPE_2 has EXECUTED — and its output is UNCOMMITTED

Re-verified 2026-09-06 05:06 against cyrup HEAD **`c10e6482`** ("refactor: decompose the subagents
background/ module behind facades") and pi-subagents HEAD `7fe9dee1`. Every line number in this
family is from **the working tree at that commit**, which is NOT the committed tree:

* **SCOPE_2 (terminal-run index + auto-drain) is `stage: exec, status: done` as of 04:55** and its
  entire output sits uncommitted: untracked `background/{auto_drain.rs,result_index/,terminal_run_index/,delivery/}`
  and `src/identity/`, plus ~36 modified tracked files (~1,829 insertions) including `wait.rs`.
  Several §2 anchors (`result_index/locate.rs`, `identity/result_name.rs`, `auto_drain.rs`) **do
  not exist at `git show c10e6482:` at all** — verify against the working tree only.
* **Never `git stash` / `git checkout -- .` / `git clean` / reset in this checkout** while this
  family is open: it would destroy SCOPE_2's landed, unreviewed port work and every untracked
  anchor above. No `git` commands during exec/qa (flux rule) — this is why.
* Multiple flux sessions share this checkout (the 3a–WORKFLOW_5 files were written by one while this index
  was re-verified by another). Per `CLAUDE.md`: before any gate, `pgrep -a cargo`, announce, and
  get an all-clear — a gate over a half-edited tree reports failures that are neither yours nor real.

`WaitDeps` already carries the three flags — fields at `wait.rs:217/:221/:223`, builders
`with_stop_on_attention`/`with_fail_on_failed_runs`/`with_fail_on_attention` at `:273`/`:281`/`:289`
— and both terminal returns already flip `Ok`/`Err` on them (`:697-703`, `:738-744`, each commented
with its pi anchor `:706-710`/`:723-727`). **SCOPE_2 landed first**, so WORKFLOW_5 collapses that
flip into `WaitOutcome::is_error`. Do not leave both mechanisms standing.

**`auto_drain.rs` already exists (372 LOC, SCOPE_2's product).** WORKFLOW_5's `auto_drain` line item is a
**rewire, not a creation**: the module documents at `auto_drain.rs:55-56` that `Err(text)` *is*
pi's `waitResult.isError` + `resultText(waitResult)` — "the shape `wait_for_subagents` already
returns". When WORKFLOW_5 replaces that return with `WaitOutcome`, the consumption seams to update are the
call at `:115`, the `Err(text)` handling at `:169-170`, and that `:55-56` doc contract.

> **CORRECTED 2026-09-14 — §0.2 is entirely superseded, in both halves.**
>
> **(a) The uncommitted-tree warning no longer applies.** There is no `c10e6482` working-tree
> distinction left to defend: `background/{auto_drain.rs,result_index/,terminal_run_index/,delivery/}`
> and `src/identity/` are ordinary files on disk on branch `claude/subagents-scope` (cut from `main`
> at `d53763b`), and `cargo fmt --all --check` / `clippy -D warnings` / `nextest` are all green on
> it. The *rule* it protected — **no `git` command during exec/qa/aug** — still stands and was
> honoured by this augment. `/home/user/cyrup/tmp/pi-subagents` (not `../../../workspace/…`) is the
> oracle on this machine.
>
> **(b) The WORKFLOW_5 rewire has LANDED.** `auto_drain.rs` is now **544 lines**, not 372, and the
> three named seams are all converted:
>
> | seam | old | **now** |
> |---|---|---|
> | the `DrainWaiter` contract | `Result<String,String>` | **`auto_drain.rs:79`** — `async fn wait_all(&self, timeout_ms: u64) -> WaitOutcome` |
> | the `:55-56` doc contract | "the shape `wait_for_subagents` already returns" | **`:67-74`** — now argues the opposite: keeping `Result` here "would reintroduce the `Ok`/`Err` flip … and would re-break the timeout, since a window that merely elapsed is NOT an error upstream" |
> | the call | `:115` | **`:145-158`** (`SubagentDrainWaiter::wait_all`, forwarding a fresh `CancelToken`) |
> | the `Err(text)` handling | `:169-170` | **`:160-200`** — `drain_outstanding_work`'s doc now enumerates the four error sources, and explicitly excludes `WaitVerdict::WindowElapsed` from them |
> | the test doubles | — | **`:292-345`** scripted as whole `WaitOutcome`s ("since WORKFLOW_5") |
>
> Both mechanisms are **not** left standing: the `Ok`/`Err` flip is gone, replaced by the single
> evaluation inside `WaitVerdict::Resolved::reported_as_error` (`wait.rs:565`) that
> `WaitOutcome::is_error` (`:629`) reads.

### 0.3 The completeness rule

**No field is permitted to be a declared-but-never-populated `Option`.** An earlier revision of this
task declared seven `WaitCompletionChild` fields as always-`None` residuals and two more as
"another task's problem". Auditing that claim found the second half false: `workflowReceiptPath` and
`workflowChildren` were attributed to WORKFLOW_6/7/9, and **none of those tasks ports the workflow
runtime** — `pi-subagents/src/workflows/` (9 files, 4,418 LOC) appears in no SCOPE file at all, and
cyrup has zero of it:

```
$ grep -rn "scripted_workflow\|workflow_settlement\|workflow_receipt\|workflow_checklist" \
    crates/cyrup-ext-subagents/src        # 0 matches, all four
$ grep -n "pub enum RunMode" -A 8 crates/cyrup-ext-subagents/src/background/state.rs
# Single | Parallel | Chain    — no Workflow
```

WORKFLOW_6/7/9 port the workflow *control* surfaces (controller registry, foreground steering,
detach/reattach) on top of a runtime nobody ports. That gap is now SCOPE_3d–WORKFLOW_3.

**The one legitimate dependency** is `archivePath`, whose producer is `writeCompletionReplay` —
specified in full by **SCOPE_4** (`completion-replay.ts`, its SUBTASK1 names the constants, the
record shape and `archivePath` explicitly). WORKFLOW_4 leaves the two named seams for it and nothing
else.

> **CORRECTED 2026-09-14 — the workflow-runtime gap §0.3 discovered is CLOSED, and the
> completeness rule HELD.**
>
> * The four-way grep that returned **0** now returns **175 matches**. `pi-subagents/src/workflows/`
>   (9 files, now 4712 LOC) is ported across **18 files** in
>   `crates/cyrup-ext-subagents/src/workflows/`: `bounded.rs chat_progress.rs checklist.rs
>   child_summary.rs display_text.rs host_command.rs host_step.rs key.rs lane_metadata.rs mod.rs
>   permit.rs preflight.rs receipt.rs resources.rs scripted/ settlement.rs stable_json.rs types.rs`.
> * `RunMode` is **no longer** `Single | Parallel | Chain`: `background/state.rs:22-27` adds
>   **`Workflow`**, doc'd as "Produced exclusively by the FOREGROUND arm (`route_workflow_mode` in
>   `extension/tool/routing.rs`)".
> * **The completeness rule was honoured.** `wait_completions/project.rs:63-67` states it in code:
>   *"Every field here has a real producer on `crate::exec::SingleResult` … none is a residual
>   placeholder waiting on a future task. The one exception, `archivePath` …"* — i.e. exactly the one
>   legitimate dependency this section named, and no other. The two seams WORKFLOW_4 was told to leave
>   are `project.rs:44-50` (the declared-but-unset `archive_path`, with the wire-shape argument for
>   declaring it anyway) and `collect.rs:104-107` (the stubbed third rung, `readCompletionReplay`).
> * `workflowReceiptPath` and `workflowChildren` were correctly re-attributed here to WORKFLOW_3, and
>   both are now populated: `WaitCompletion::workflow_receipt_path` (`project.rs:38-39`) and
>   `workflow_children: Option<crate::workflows::WorkflowChildSummary>` (`project.rs:53-54`), whose
>   corruption is the **one** place the tolerant projector refuses to degrade
>   (`CompletionProjectionError`, `project.rs:126-137`).

---

## 1. The nine tasks

| task | subject | upstream LOC | depends on | §A types it DECLARES |
|---|---|---|---|---|
| **3a** | `SingleResult` producer completeness — `turns`, `session_file`, `output_state`, `structured_output_path`, `artifact_paths` | — (cyrup-side plumbing) | — | — *(**done**)* |
| **3j** | model-exclusion registry + `recordRetryableModelFailure` | `model-exclusions.ts` (374) + `model-fallback.ts:311-345,516-530,617-623` | 3b | — *(consumes `LadderStep`, `AttemptNote`)* |
| **3b** | `context_overflow` + the attempt-note channel **+ the §A foundation for `exec/`** | `model-fallback.ts:631-649`, `subagent-runner.ts:1026,1432-1433` | 3a | **`LowerLiteral`, `LoweredLine`, `AttemptNote`, `LadderStep`, `LadderStop`** |
| **3c** | `exec/mutation_evidence/` + `timeout_recovery` + the output-assembly core | `mutation-evidence.ts` (199) | 3a, 3b | **`assemble_delivered_output`** *(consumes `LadderStop` + the EXISTING `resolve_subagent_result_status`)* |
| **3d** | Workflow foundations — `RunMode::Workflow`, child-summary, chat-progress, resources | 547 | 3a | **`WorkflowKey`, `Bounded<N>`, `BoundedUtf16<N>`** |
| **3e** | Workflow gates — preflight, checklist, host-command | 968 | 3d | **`ContainedPath`** *(if 3c has not landed it)* |
| **WORKFLOW_1** | `scripted-workflow.ts` — the workflow executor | 2284 | 3e | — *(heaviest consumer of `WorkflowKey`)* |
| **WORKFLOW_2** | **THE WORKFLOW ARM — the vertical slice that gives 3d/3e/WORKFLOW_1 a caller** | — (cyrup-side wiring) | WORKFLOW_1 | — |
| **WORKFLOW_3** | Workflow terminal writers — receipt + settlement (writes both payload keys) | 615 | **WORKFLOW_2** | — |
| **WORKFLOW_4** | `background/wait_completions/` + the two `result_index` exports | `wait-completions.ts` (213) | 3a, 3b, 3c, WORKFLOW_3 | — |
| **WORKFLOW_5** | `wait.rs` `WaitOutcome`, collector wiring, `wait_tool`, `auto_drain` **rewire** (§0.2) | `subagent-wait.ts:319-360,674-744` | WORKFLOW_4 | — |

> **STATUS 2026-09-14 — nine of eleven rows have EXECUTED.** See §0.0.2 for the per-row proof.
> Open: **3j** (insertion point marked in landed code at `exec/fallback.rs:1721-1725`) and
> **SCOPE_4** (`archivePath`; seams marked at `wait_completions/project.rs:44-48` and
> `collect.rs:104-107`). Their task files are the only two of the eleven still in `.flux/todo/`;
> the other nine are no longer on disk anywhere under `.flux/`. Upstream LOC in the table above are
> stale for two rows: `model-exclusions.ts` is **385** (not 374) and `workflows/*` is **4712** (not
> 4418), with `scripted-workflow.ts` at **2553** (not 2284).

**The §A column is a hard ordering constraint**, not advice. A task may not re-declare a type
another task owns, and may not open-code a grammar or a bound that §A already names. **3d moved
ahead of 3e/WORKFLOW_1/WORKFLOW_3 in importance** for exactly this reason: it is where `WorkflowKey` and
`Bounded<N>` are born, and those two carry the four workflow tasks.

**3j now depends on 3b rather than preceding it.** The original order had 3j first so 3b could
consume its `record_retryable_model_failure` guard; §A inverts that, because 3j's insertion point is
a line inside 3b's `classify_attempt` (§A.1) rather than a position in a 90-line loop. 3b landing
first is what makes 3j a one-line change instead of a hoist.

**All nine task files exist on disk** (`todo/SCOPE_3a.md` … `todo/WORKFLOW_5.md`, written
04:57–05:06 on 2026-09-06). Execute in that order — **3j slots in before 3b**, which consumes its
`record_retryable_model_failure` guard. 3b/3c/3d all edit `SingleResult` and
`runner_main/settle.rs`, so run them sequentially even though they are logically independent.

> ### ⚠ INSERTED 2026-09-07 — [`WORKFLOW_2`](WORKFLOW_2.md) runs BEFORE WORKFLOW_3. Read this before scheduling anything else.
>
> **3d/3e/WORKFLOW_1 landed 15,933 LOC of workflow runtime with ZERO callers.**
> `grep -rn 'run_workflow_script' crates/ --include=*.rs | grep -v 'workflows/scripted/'` returns
> only the facade re-export. Nothing in the crate can start a workflow: there is no
> `workflowScript` tool parameter, no dispatch arm, no producer of `RunMode::Workflow`.
>
> **This is a decomposition defect in §1's axis, not an omission in any one task.** The nine tasks
> are split by UPSTREAM SOURCE FILE — a bookkeeping axis — so every DoD reads *"the port matches
> upstream"* rather than *"a user can do X"*, and the question **does it work** was deferred past
> the last file in the family. WORKFLOW_3's own augment hit the consequence directly: its
> `plan_workflow_settlement` has no caller, so its signature had to be inferred, and inferring
> found `RunStatus` missing five fields the composer writes (WORKFLOW_3 §0.4).
>
> `WORKFLOW_2` is the correction: one vertical slice (tool param → dispatch arm → real
> `WorkflowScriptHost` → `route_workflow_mode`) whose DoD is a runnable `cyrup -p` command. It is
> small because WORKFLOW_1 built the seam correctly — `WorkflowScriptHost` requires only `launch` + `status`
> and `RunWorkflowScriptOptions` only `script` + `host`, and 13 engine tests already drive real
> scripts through a `FakeHost` (`scripted/engine.rs:2877`, `:2949`).
>
> **The binding rule this adds to the family: a task's DoD MUST name a command a human can run.**
> If it cannot, the task is a library fragment and belongs folded into the slice that consumes it.
> `missions/workflow_state.rs` and `missions/goal_driver.rs` both already self-document as having no
> caller — the same defect, and worth an audit before more porting.
>
> #### ✅ RESOLVED 2026-09-14 — WORKFLOW_2 landed, and so did the audit it asked for
>
> * **The zero-caller grep is dead.** `grep -rn 'run_workflow_script' crates/ --include=*.rs | grep
>   -v 'workflows/scripted/'` now returns **seven** hits, and the load-bearing one is a real call:
>   `extension/tool/routing.rs:700` → `crate::workflows::scripted::run_workflow_script(…)`, reached
>   from `route_workflow_mode` (**`routing.rs:523`**). The tool parameter is documented at
>   `extension/tool/params.rs:89`; `workflow_child_stops.rs:537` drives it from a test.
> * **`RunMode::Workflow` has a producer** — `background/state.rs:22-27` names `route_workflow_mode`
>   as the *exclusive* one, and states that `background/runner_main/` never mints it.
> * **The audit's two named files now have callers too**, so that trailing claim is stale:
>   `missions::MissionWorkflowStateStore::create` is called at **`extension/tool/routing.rs:571`**,
>   and `missions::collect_goal_continuation_notices` at
>   **`extension/executor/notices.rs:523`**.
> * Since WORKFLOW_2, the surface kept growing on the same seam — **WORKFLOW_17-21** landed the
>   `workflowScript` runtime (`runs.status`, the child-stop registry at
>   `extension/executor/workflow_child_stops.rs`, `runs.host`, workflow `state.get`/`state.set`, live
>   emit forwarding), and **9aeba76** rewrote the process-group probe
>   (`workflows/host_command.rs:473-560` — `process_group_is_provably_empty` / one-syscall
>   `ESRCH` fast path, then `process_group_is_populated` polled at `VERIFY_INTERVAL`) and **boxed**
>   the engine error: `run_workflow_script` now returns
>   `Result<WorkflowScriptResult, Box<WorkflowScriptError>>` (`workflows/scripted/engine.rs:2663`,
>   with the size rationale at `:2655-2660`). Any future task quoting an engine signature must use
>   the boxed form.

**3j was found while augmenting 3b**, not during the original split:
`pi-subagents/src/runs/shared/model-exclusions.ts` (374 LOC) is unscheduled in every SCOPE file and
absent from cyrup, so `build_model_candidates` (`exec/fallback.rs:146-215`) never drops a model
already known to be down. Same class of omission as the workflow runtime in §0.3.

---

## 2. Verified anchors — shared by all nine

| what | upstream | cyrup @ `c10e6482` |
|---|---|---|
| the module | `wait-completions.ts:1-213` | **absent** |
| call site | `subagent-wait.ts:681`, inside the try at `:674-684` | `wait.rs:642-656` (same five predecessors, same order) |
| the `result()` builder | `subagent-wait.ts:338-353` | `wait.rs` returns `Result<String, String>`; `extension/wait_tool.rs:159-167` discards all but the text |
| usage roll-up | `completionUsage` `:319-336` → `toAgentToolUsage` (`shared/utils.ts:356-365`) | `cyrup_core::ToolResult::usage` (`cyrup-core/src/tool.rs:32`); fold precedent `registration/cost.rs:118-124` |
| recovery note | `formatCompletionRecovery` `:355-360`, interpolated `:707`/`:730` | absent — 3c is its producer, WORKFLOW_5 its consumer |
| index lookup | `resultPayloadPathForSessionRun` (`result-files.ts:334-338`) | **LANDED** — `result_index/locate.rs:187-203`, exported via `result_index/mod.rs:100-103`; note it already folds the pending fallback internally at `:198` |
| EACCES fallback | `fallbackResultPayloadPathForSessionRun` (`result-files.ts:300-304`) | **MISSING** — body exists as `pending_result_location` (`locate.rs:96-112`, `pub(crate)`) |
| errno predicate | `isAccessDenied` (`wait-completions.ts:35-38`) | `result_index/errno.rs:44-48` — `pub(crate)` inside a **private** `mod errno` |
| public path | `resultFilePath` (`result-files.ts:52-54`) | `ResultFileName::for_run` (`identity/result_name.rs:50`) + `resolve_in` (`:96`) |
| the store | `SubagentState.completedResults` (`shared/types.ts:2257`), written `result-watcher.ts:428-435` | **MISSING** — `watch/install.rs:151-159` records nothing |
| TTL | `completionTtlMs` (`result-watcher.ts:203`) | `watch::DEDUP_TTL` (`watch/results_watcher.rs:39`, 10 min) — reuse |
| payload struct | — | `ResultFile` (`background/records.rs:373-419`) |
| child struct | `SingleResult` (`shared/types.ts:1228-1300`) | `exec/run_result.rs:23-196`, `#[serde(rename_all = "camelCase")]` |

### §2 RE-VERIFIED 2026-09-14 — every "MISSING"/"absent" row is now LANDED

| what | cyrup @ `c10e6482` (old) | **cyrup NOW** |
|---|---|---|
| the module | **absent** | `background/wait_completions/` — 4 files, 675 LOC (§0.0.1) |
| call site | `wait.rs:642-656` | **`wait.rs:1027-1036`**, inside `resolve`'s terminal block; `Err` → `WaitVerdict::CompletionsFailed` at `:1034` |
| the `result()` builder | `wait.rs` returns `Result<String,String>`; `wait_tool.rs:159-167` discards all but text | **`WaitOutcome`** (`wait.rs:578-696`): `new` `:599`, `plain` `:620`, `is_error` `:629`, `details` `:653`, `usage` `:690`. `extension/wait_tool.rs:178-191` now keeps **text + usage + details** |
| usage roll-up | `ToolResult::usage`; fold precedent `registration/cost.rs:118-124` | `completion_usage` (`wait_completions/project.rs`) returns `Option<cyrup_core::Usage>` — **no conversion needed**, per `wait_tool.rs:180-183`. `CompletionUsage` (`project.rs:105-122`) is the six-field pi-shaped wire type, deliberately **not** `cyrup_core::Usage` (which has no `turns`). `ToolResult::usage` is `cyrup-core/src/tool.rs:32`; `CostUsage::add_usage` is `registration/cost.rs:118` |
| recovery note | absent — 3c producer, WORKFLOW_5 consumer | **`wait.rs:1190`** `format_completion_recovery`, called `:1058`; sibling `format_observed_completions` `:1166` |
| index lookup | **LANDED** — `locate.rs:187-203`, exported `mod.rs:100-103` | **`locate.rs:361`**, exported `result_index/mod.rs:110-115` |
| EACCES fallback | **MISSING** — body exists as `pending_result_location` (`locate.rs:96-112`, `pub(crate)`) | **LANDED and `pub`** — `fallback_result_payload_path_for_session_run` (**`locate.rs:397-405`**), exported at `result_index/mod.rs:112`; still delegates to `pending_result_location` (**`locate.rs:265`**, `pub(crate)`) |
| errno predicate | `result_index/errno.rs:44-48` — `pub(crate)` inside a **private** `mod errno` | **`errno.rs:47-50`**, and `mod errno` is now reachable from `wait_completions` (`collect.rs:7` does `use crate::background::result_index::{self, errno};`). `is_absent` is the sibling used at `collect.rs:97` |
| public path | `ResultFileName::for_run` + `resolve_in` | **`identity/result_name.rs:50`** / **`:96`** (struct at `:40`) — unchanged, and `collect.rs:50-51` is the caller |
| the store | **MISSING** — `watch/install.rs:151-159` records nothing | **`WaitCompletionStore`** (`wait_completions/record.rs:32-35`), a `Mutex<HashMap<RunId, RecordedCompletion>>` with poison recovery; registered **FIRST** in the watcher's `CompositeCompletionObserver` at **`extension/executor/notices.rs:445-464`** (`self.wait_completions()` at `:453`, ahead of `MissionSyncCompletionObserver` `:459` and the `CompletionBus` `:463`) — the ordering argument is in that comment and in `record.rs`'s own doc |
| TTL | `watch::DEDUP_TTL` (`results_watcher.rs:39`, 10 min) — reuse | **reused**: `results_watcher.rs:40`, imported by `record.rs:8-10` |
| payload struct | `ResultFile` (`records.rs:373-419`) | **`records.rs:483`** — but note `to_wait_completion` deliberately takes `&serde_json::Value`, **not** `ResultFile`, because `ResultFile` is a *validating* type and would turn a best-effort projection into a hard read failure (`project.rs:7-15`) |
| child struct | `exec/run_result.rs:23-196` | **`exec/run_result.rs:25`** (320 LOC total), still `#[serde(rename_all = "camelCase")]` at `:24` |

### The field map — which task supplies each

| `WaitCompletionChild` field | supplied by |
|---|---|
| `agent`, `model`, `error`, `structuredOutput` | already present on `SingleResult` |
| `success` | derived `exit_code == 0` in WORKFLOW_4 — see below |
| `usage` | `SingleResult::usage` + **3a**'s `turns` |
| `sessionFile` | **3a** |
| `outputState` | **3a** |
| `structuredOutputPath` | **3a** |
| `artifactPaths` | **3a** |
| `contextOverflow` | **3b** |
| `timeoutRecovery` | **3c** |
| `runId` (child) | **3d** (`StepStatus::run_id`/`workflow_key`) |
| run-level `workflowChildren`, `workflowReceiptPath` | **WORKFLOW_3** |
| run-level `archivePath` | **SCOPE_4** |

**`success` is derived, not missing.** Upstream copies `child.success`; cyrup's `SingleResult` has
`exit_code: i32` and the crate already uses `exit_code == 0` as the success predicate —
`runner_main/finish.rs:246` computes the run-level `success` as
`terminal_state == Complete && results.iter().all(|r| r.exit_code == 0)`. The derived value is
identical to what upstream stores; adding a redundant field would create two sources of truth for
one fact.

> **FIELD MAP RE-VERIFIED 2026-09-14 — every row has a producer except `archivePath`.**
> `WaitCompletionChild` is `wait_completions/project.rs:69-103`; `WaitCompletion` is `:30-56` (struct at `:32`).
>
> | field | producer as landed |
> |---|---|
> | `agent` `usage` `model` `error` `structuredOutput` | `SingleResult` (`run_result.rs:26/29/47/160/51`) |
> | `success` | **derived, as specified** — `Option<bool>` from the payload's `exitCode`, doc'd at `project.rs:80-84`; `Option` rather than `bool` so a payload with no `exitCode` **omits** the key instead of claiming failure. The run-level predicate it mirrors moved to **`runner_main/finish.rs:338`** (and `:28`), not `:246` |
> | `runId` (child) | `project.rs:86-88` — tries cyrup's `childRunId` **then** pi's `runId` |
> | `sessionFile` `outputState` `structuredOutputPath` `artifactPaths` | 3a: `run_result.rs:187 / :211 / :225 / :234` |
> | `contextOverflow` | 3b: `run_result.rs:97`; on the wire it is `bool` with `skip_serializing_if = "crate::exec::is_false"` (`project.rs:97-99`) — pi writes the key only when `true`. Its value comes from `outcome.stop == LadderStop::ContextOverflow` (**`exec/mod.rs:731-734`**) |
> | `timeoutRecovery` | 3c: `run_result.rs:82` (`TimeoutRecoverySummary`), projected to `TimeoutRecoveryProjection` (`project.rs:102`) |
> | `workflowChildren` / `workflowReceiptPath` (run level) | WORKFLOW_3: `project.rs:53-54` / `:38-39` |
> | `archivePath` (run **and** child level) | **SCOPE_4, still open** — declared and unset at `project.rs:44-50`; see §0.0.1 |
>
> `STRUCTURED_OUTPUT_INLINE_LIMIT_BYTES = 4 * 1024` (pi `wait-completions.ts:45`) is
> `project.rs:22`, and `WaitCompletion` carries **no output text** by design (`project.rs:26-29`,
> pi's own note at `wait-completions.ts:54-59`).

---

## 3. Gates — every task in this family

Per `CLAUDE.md`; check `df -h /` and `df -h /tmp` first, and confirm no other session is mid-build
or mid-edit (`pgrep -a cargo`).

```bash
cargo test  --workspace --features test-fixtures --no-fail-fast
cargo clippy --workspace --all-targets --features test-fixtures; echo "exit=$?"   # MUST be 0
```

`cargo check -p cyrup-ext-subagents` is **not** sufficient for any of these: `SingleResult`,
`StepResult`, `ArtifactPaths`, `RunMode` and `wait_for_subagents` are all `pub` with cross-crate
visibility, and only a `--workspace` run sees every caller.

### Type-contract gate — also every task (§A)

These are greppable and take seconds. A task is not done until all five are clean.

```bash
C=crates/cyrup-ext-subagents

# 1. The key grammar is declared ONCE. Any second copy is a §A.3 violation.
grep -rn 'A-Za-z0-9._-' $C/src --include=*.rs | grep -v 'workflows/key.rs'        # MUST be empty

# 2. No newtype from §A derives Deserialize — it would bypass the parser (§A.3).
grep -rn -B3 'struct \(WorkflowKey\|Bounded\|LowerLiteral\|ContainedPath\)' $C/src \
  | grep -i 'derive.*Deserialize'                                                 # MUST be empty

# 3. Every §A newtype deserializes THROUGH its parser: a hand-written impl exists, and no
#    per-field attribute appears anywhere in workflows/ (AMENDED per SCOPE_3d §0.9 — the old form
#    counted `deserialize_with` attributes, which the correct idiom makes ZERO and would therefore
#    fail a compliant implementation).
grep -c "impl<'de> serde::Deserialize<'de> for" $C/src/workflows/key.rs      # >= 1
grep -rn 'deserialize_with' $C/src/workflows/                                # MUST be empty

# 4. No catch-all on a §A domain enum.
grep -rn -A12 'match .*\(LadderStep\|LadderStop\|AttemptNote\|RunMode\)' $C/src \
  | grep '_ =>'                                                                   # MUST be empty

# 5. No second bounded-string helper (§A.4).
grep -rn 'fn bounded\|fn bounded_text\|fn normalize_display' $C/src \
  | grep -v 'workflows/bounded.rs'                                                # MUST be empty
```

And one that is not greppable and must be read: **no task's DoD may contain a phrase of the form
"verify by reading the order"** or otherwise ask a human to check something the type system was
offered and declined (§A.1).

Baseline at `cf26010`: 4794 passed / 0 failed / 8 ignored. Clippy exit 0.

> **GATES RE-VERIFIED 2026-09-14 — new baseline, and two gate commands need amending.**
>
> **Current green baseline** on `claude/subagents-scope` (cut from `main` at `d53763b`), verified
> on this exact commit — there are **no** pre-existing failures to attribute anything to:
>
> ```bash
> cargo fmt --all -- --check                              # clean (repo-wide no-op; format only
>                                                         # what you touch: cargo fmt -p cyrup-ext-subagents)
> cargo clippy --workspace --all-targets -- -D warnings   # clean
> cargo nextest run --workspace                           # 9913 run, 9913 passed, 9 skipped
> ```
>
> `4794 passed / 8 ignored` at `cf26010` is superseded by **9913 passed / 9 skipped**. Toolchain is
> rustc 1.98.1 + cargo-nextest 0.9.144, `CARGO_INCREMENTAL=0`, already installed — install nothing.
> The `--features test-fixtures` form above is still valid (the feature exists,
> `cyrup-ext-subagents/Cargo.toml:21`, gating the `cyrup-subagent-fixture` bin via
> `required-features` at `:168`/`:178`), and §3's argument for `--workspace` over
> `cargo check -p cyrup-ext-subagents` still holds — `SingleResult`, `RunMode`, `WaitOutcome` and
> `wait_for_subagents` are all `pub` with cross-crate callers.
>
> **Gate 1 and gate 5 now produce non-empty output on a COMPLIANT tree.** Both hits are explained in
> §0.0.3; use these amended forms so a future session does not "fix" compliant code:
>
> ```bash
> C=crates/cyrup-ext-subagents
>
> # 1 (amended). Exclude comment lines — the grammar is quoted in prose at
> #   extension/executor/workflow.rs:995 while explaining pi's own check.
> grep -rn 'A-Za-z0-9._-' $C/src --include=*.rs \
>   | grep -v 'workflows/key.rs' | grep -v ':[0-9]*: *//'                      # MUST be empty
>
> # 5 (amended). The TRUNCATING family is not this type (§A.4) and lives in display_text.rs;
> #   a normalizer that DELEGATES to Bounded/BoundedUtf16 is compliant, not a tenth copy.
> grep -rn 'fn bounded\|fn bounded_text\|fn normalize_display' $C/src \
>   | grep -v 'workflows/bounded.rs' | grep -v 'workflows/display_text.rs'
> #   Current expected hits, all compliant — verify each still delegates rather than re-implements:
> #     workflows/preflight.rs:193          normalize_display_string  -> BoundedUtf16::<256>::parse
> #     background/watch/observer.rs:119    bounded_completion_summary (truncating family)
> #     extension/executor/foreground_history/record.rs:149  bounded_tail (truncating family)
> ```
>
> Gates **2, 3 and 4 are empty/passing as written** — gate 3 in its AMENDED (§A.3, 2026-09-06) form
> only; the pre-amendment form counted `deserialize_with` attributes and would **fail** the correct
> implementation. `workflows/key.rs` carries exactly one hand-written
> `impl<'de> serde::Deserialize<'de>`; `workflows/bounded.rs:59`/`:104` carry the same idiom for
> `Bounded<N>`/`BoundedUtf16<N>`.
>
> **Multi-session caution still applies** (`pgrep -a cargo`, `df -h /`, `df -h /tmp` before any
> gate), and the flux rule that produced §0.2's warning is unchanged: **no `git` command at all**
> during aug/exec/qa.

---

## 4. Shared research notes

* Upstream at HEAD `7fe9dee1`: `wait-completions.ts` (213), `subagent-wait.ts:319-360` (the
  `result`/`completionUsage`/`formatCompletionRecovery` builders), `:674-684` (call site),
  `:704-744` (the two terminal returns), `result-watcher.ts:405-435` (the record point),
  `result-files.ts:52-54,254-257,293-304,334-338` (path builders), `shared/utils.ts:356-365`
  (`toAgentToolUsage`), `shared/types.ts:256-263,399,1228-1368,1516-1522,2257`,
  `model-fallback.ts:589,631-649`, `mutation-evidence.ts` (199), `workflows/*` (4418).
* `result_index::result_payload_path_for_session_run` (`locate.rs:187-203`) rethrows
  `EPERM`/`EACCES`, swallows `ENOENT`, and **promotes a staged payload on the way**
  (`locate.rs:58-85`) — upstream's behaviour too, not a cyrup addition.
* `background/child_identity.rs:14-20` independently records that `StepStatus` carries neither a
  `workflowKey` nor a per-step `runId`; 3d closes exactly that.
* `crate::time::now_epoch_millis()` (`time.rs:18`) and `epoch_millis(SystemTime)` (`:26`) are the
  clock helpers. `SubagentError::Spawn(#[from] std::io::Error)` (`error.rs:205-206`) is the I/O
  variant to wrap into.
* Every dependency these tasks need is already in the workspace, `gix` included
  (`crates/cyrup-resources/Cargo.toml:23`, v0.85.0). No clone into `./tmp` was required —
  `pi-subagents/` is the read-only oracle.
* Re-verified 2026-09-06 05:06 against the working tree (see §0.2): `wait_for_subagents` at
  `wait.rs:434`, `terminal_runs_for` `:349`, `summarize_terminal_runs` `:372`, call-site
  predecessors `:642-656`, terminal flips `:697-703`/`:738-744`; `SingleResult` at
  `run_result.rs:23` (`camelCase`, `:22`) with **none** of the seven new fields yet; `RunMode` is
  still `Single | Parallel | Chain` (`background/state.rs:15-23`); the four workflow greps are
  still 0; `completed_results` store still absent (`grep -rn completed_results` = 0);
  `exec/mutation_evidence` and `background/wait_completions` still absent; `DEDUP_TTL`
  `results_watcher.rs:39`; `ResultFile` `records.rs:373`; `is_access_denied`
  `result_index/errno.rs:45` inside private `mod errno` (`result_index/mod.rs:86`);
  `ResultFileName::for_run`/`resolve_in` `identity/result_name.rs:50`/`:96`; run-level success
  predicate `runner_main/finish.rs:246` (and `:29`); usage fold precedent
  `registration/cost.rs:118-124`; `ToolResult::usage` `cyrup-core/src/tool.rs:32`;
  `wait_tool.rs:159-167` still discards everything but the text.
* `wait.rs` production code reads **no payload** (grep hits for `ResultFile` in it are doc comments
  and the `#[cfg(test)]` fixture at `:836-840`) — §0.1's premise still holds. SUBA-060
  `resume_guidance` + `attention_note` landed with SCOPE_2 (`wait.rs:655-668`, interpolated in the
  two text builders at `:691-694`/`:735-737`); 3c/WORKFLOW_5's recovery note joins them at those builders.

### §4 RE-VERIFIED 2026-09-14 — corrected anchors

Every bullet above dates from the 2026-09-06 05:06 sweep and is superseded where it names a line.

* **Oracle path.** `/home/user/cyrup/tmp/pi-subagents` — **not** `../../../workspace/pi-subagents`,
  which does not exist on this machine. Every `[…](../../../workspace/…)` link in this file and in
  `SCOPE_3j.md` / `SCOPE_4.md` is dead; read the corresponding file under `tmp/pi-subagents/src/`.
  Upstream LOC drift is tabulated in §0.0.4.
* **`result_index::result_payload_path_for_session_run`** is `locate.rs:361-378` (not `:187-203`);
  it still consults the index first and falls through to `pending_result_location`
  (`locate.rs:265`) at `:373`. `fallback_result_payload_path_for_session_run` is
  `locate.rs:397-405`, whose doc (`:382-396`) states it is the *recovery* path — addressed directly
  rather than through an unreadable index directory. `owned_payload_path` is `:174`,
  `owned_candidates` `:253`, `payload_run_id` `:310`.
* **`background/child_identity.rs`'s gap is closed.** Its module doc now reads
  *"`StepStatus::workflow_key` and `StepStatus::run_id` (SCOPE_3d) are what populate the first two
  rungs"* (`child_identity.rs:14-18`). What remains genuinely unrepresentable is only the
  `DynamicGroup` splice residual (SUBA-093), recorded there.
* **Clock/error helpers unchanged**: `crate::time::now_epoch_millis()` (`time.rs:18`),
  `epoch_millis(SystemTime)` (`:26`), `SubagentError::Spawn(#[from] std::io::Error)`
  (`error.rs:205-206`) — and `collect.rs` is the live example of wrapping into `Spawn`, including
  `io::Error::new(InvalidData, …)` for a JSON/projection fault (`collect.rs:83-95`).
* **The observer seam.** `background/watch/install.rs` takes
  `Option<Arc<dyn CompletionObserver>>` (`:70-79`, `:210`, `:305`) and runs the observer band
  **before** the unlink, retiring the cross-session obligation only once every observer ran cleanly
  (`:325-334`, pi `result-watcher.ts:408,425,428`). The `CompositeCompletionObserver` that fans it
  out to three listeners is built at `extension/executor/notices.rs:445-464`.
* **The three-rung resolution as landed** (`collect.rs:37-113`): (1) `WaitCompletionStore::get`;
  (2) session index, EACCES-recovered once via the fallback builder, else the public path;
  (3) public path alone for an unattributed run. A late-arriving record is re-checked after an
  `is_absent` read (`collect.rs:97-103`) — a race the original spec did not name. **Read-only by
  contract**: nothing here mints or touches a `ConsumablePayload`, so "a wait must not be able to
  destroy what it reads" (`collect.rs:16-19`).
* **§A types, as landed**: `LowerLiteral` `exec/fallback.rs:490` (+ the compile-time-checked
  `lower!` macro `:456`), `LoweredLine<'a>` `:708`, `AttemptNote` `:1038`, `LadderStop` `:1648`
  (`pub(crate)`), `LadderStep` `:1667`, plus an unforeseen but contract-compliant sibling
  `LadderControl` `:1749` ("exists so the loop has exactly ONE settle site"). `classify_attempt`
  `:1699` is the pure precedence function §A.1 demanded, with the seven-step order in its doc.
  §A.2's CORRECTED note held: there is no `SingleResult::terminal_reason()`, and
  `resolve_subagent_result_status` (`tui/intercom.rs:286`, over `SubagentResultStatus` `:127`)
  remains the single run-status resolver, pinned by
  `resolve_subagent_result_status_reproduces_pis_branch_order` (`tui/intercom.rs:1190`).
* **`Bounded`/`WorkflowKey` reach**: `workflows/bounded.rs` (160 LOC) is consumed by
  `preflight.rs`, `types.rs`, `lane_metadata.rs`, `receipt.rs`, `child_summary.rs`;
  `workflows/key.rs` (150 LOC) by 24 files across `workflows/`, `background/`,
  `extension/executor/`, `missions/` and `registration/profiles.rs`.

---

/home/d0m17bw/.flux/-home-d0m17bw-workspace-cyrup/todo/SCOPE_3.md
