# 09b — cyrup-ext-subagents: the v0.57.0 → v0.71.0 drift window

**This file adds to `09-cyrup-ext-subagents.md` and `09a-cyrup-ext-subagents-v0.57-drift.md`. It
replaces neither of them.** `09` remains the area's file of record for `SUBA-001`…`SUBA-071` and its
Trackers. `09a` remains the file of record for `SUBA-072`…`SUBA-106`, including the ids it filed past
its own scope line, which stay where they are. This file adds **`SUBA-107`…`SUBA-113`** (first pass)
and **`SUBA-114`…`SUBA-143`** (pass 2, 2026-09-24); the next free id is **`SUBA-213`** (`SUBA-212` filed 2026-10-10 closing `SUBA-131`; `SUBA-210`…`SUBA-211` filed 2026-10-10 closing `SUBA-162`; `SUBA-204`…`SUBA-209` filed 2026-10-09 as `v0.76.1` follow-ups; `SUBA-176`…`SUBA-203` filed 2026-10-09 by the pi v1.1.0 drift triage; `SUBA-164`…`SUBA-173` filed 2026-10-03; `SUBA-174` filed 2026-10-04, closed 2026-10-05; `SUBA-175` filed 2026-10-09). Ids are
never renumbered, so an item never moves between the three files.

This file was created because README's pin table (*Which area file is pinned where*, row `09a`)
names a `09b` as the structurally correct home for the pi-subagents window that neither `09`'s nor
`09a`'s `## Scope` covers. That was `v0.57.0..v0.67.0` when the table was written. Upstream has since
tagged v0.68.0, v0.69.0, v0.70.0, v0.70.1 and v0.71.0, so the window is now **`v0.57.0..v0.71.0`**.
The file name keeps the `v0.64` stem the task brief gave it. The scope below is the authority, not
the name.

**Do not add up this file with the census blocks in `09` and `09a` without reading them.** Those
blocks hold UNVERIFIED leads for `v0.57.0..v0.67.0` (2026-09-14). By scope, this file now owns them.
They were **not** re-verified here and are not restated. `## Leads` below says which of them this pass
happened to settle.

> ### PIN 2026-10-09 — pi v1.1.0 drift triage: cyrup `6b14575` × pi-subagents **`ad11b7ab`** (= `v0.76.1-29-gad11b7ab`)
>
> **Window read:** `git log --no-merges v0.74.0..ad11b7ab` = **92** commits (the lane recorded 88; 92 is the
> measured figure), split `v0.74.0..v0.75.0` = 31 (already dispositioned commit by commit by the 2026-10-03 pass,
> `## Findings filed 2026-10-03`; spot-checked, no new evidence against them) and `v0.75.0..ad11b7ab` = **61**
> (the lane recorded 57; reconciling all 61 against the filed rows and the list below, every one has a
> disposition). Upstream read through `git -C tmp/pi-subagents show` only; cyrup at `6b14575`. Nothing was run.
> **This file's window is now `v0.57.0..ad11b7ab`.** The pin is untagged, deliberately (README *CURRENT PINS*).
>
> **Filed (28):** `SUBA-176`…`SUBA-203` — three medium (`SUBA-176` the `deprecated` schema keyword, `SUBA-177` the
> paused-run stop seal, `SUBA-178` runner launchers failing open), the rest low. Kind corrections applied at
> filing: `SUBA-177`, `SUBA-181`, `SUBA-196` and `SUBA-203` are `upstream-drift`, not `parity-bug` (upstream had
> the same behaviour until a commit after the baseline). `SUBA-187` is PLAUSIBLE (read, not run). `SUBA-185`
> supersedes `09-cyrup-ext-subagents.md:901`; `SUBA-188` is the row the ledger's 2026-10-08 UPDATE asked for;
> `SUBA-186` is blocked on `TUI-171` (area 07).
>
> **Read in scope and deliberately NOT filed (`v0.75.0..ad11b7ab`):**
> * *Folded into `SUBA-162`* (progressive widget tier; **CLOSED 2026-10-10**, both ported): `8d804895` / #2662, `588d2cfd` / #2738.
> * *Folded into filed rows:* `ed942575` / #2746 (`SUBA-186`), `620cefa3` / #2670 (`SUBA-179`), `4c63aa07` /
>   #2747 (`SUBA-180`), `d67d3173` / #2675 (`SUBA-182`), `58c7f613` / #2722 (RPC half already matched,
>   `extension/rpc/mod.rs:62-68`; paused half is `SUBA-177`).
> * *Not applicable by construction:* `50412418` / #2683 and `7fd88df8` / #2687 (pi-web liveness, N/A per 09a:112);
>   `33f427fc` / #2671 (Pi's `builtin:mcp` selector; cyrup ships cyrup-mcp); `48f7e28c` / #2732 (in-process
>   foreground children; cyrup spawns processes; `SUBA-142`); `3be46a22` / #2723 (background workflow usage; cyrup
>   refuses async workflows, `extension/tool/routing.rs:851`); `3c729c4f` / #2710 (HerdrPlacedRun bridge backlog;
>   no herdr-placed pi bridge session); `542cf504` / #2739 (cyrup's schedule root is `<cwd>/.cyrup-subagents`,
>   `artifacts.rs:43`, `:162`); `5855164a` / #2686 and `1edc2b20` / #2684 (Windows file ids as JS numbers; Rust
>   `u64` is exact); `6826b054` / #2697 (Node module cache).
> * *Already matched:* `f738f855` / #2700 (doctor and admission key capacity by `services.session_id()`,
>   `extension/executor/reports.rs:57-66`); `dbd40216` / #2743 (stage → index → promote,
>   `background/result_index/write.rs:205-227`).
> * *Perf only:* `1ec410b3` / #2708, `e44fe0df` / #2709.
> * *Docs / tests / CI / release / chore:* `8323c6d5`, `1a255e91`, `448a395a`, `0c33ec7c`, `655986a0`, `9a40e7fb`,
>   `026f6ee0`, `7d072b91` (v0.76.1), `99ccd391` (v0.76.0), `700c91bc`.
>
> **Leads recorded, not filed:** `ad11b7ab` / #2757 (per-run result lease vs a consumer of a paused result; cyrup's
> runner publishes once, `runner_main/finish.rs:257-265`; `workflow_detach`'s reconcile write not traced);
> `5da80816` / #2712 (output-less workflow resume reusing an earlier stage's report path; cyrup scoped out
> `resumeContract`, `foreground_history/record.rs:73`, so the collision cannot be located; whether a cyrup resume
> overwrites the earlier report was not determined); `8fb89814`'s first commit (async-workflow-child release; no
> async workflow children; the namespace half is `SUBA-194`).

> ### CLOSURES 2026-09-28 (second batch) — five lows (`SUBA-133`, `SUBA-136`, `SUBA-137`, `SUBA-138`, `SUBA-140`); two partials (`SUBA-134`, `SUBA-141`)
>
> The suba-surface lane. This work is on `claude/lows-next` and not yet committed. Each row and body section
> carries its evidence. All five closures are fixes. **Unlike the block below, no test in this batch was
> shown red without its fix, and none has been seen to run on the combined tree.** The verifier could not
> build anything, and the integration pass ran only `cargo fmt --all -- --check` (passed, no files changed).
> Clippy and `cargo nextest` were not run because `/` had 2.9 GB free, below the pass's 4 GB floor. Every
> named test exists in the tree; that is all this block claims for them.
> **Counted set after this pass (`count_open_items.py`): 0 critical · 0 high · 8 medium · 7 low = 15 open,
> 2 trackers, 20 closed** (was 8 medium · 12 low = 20 open, 15 closed after the block below; this supersedes
> that block's counted-set line). Open: `SUBA-107`, `SUBA-108`, `SUBA-111`, `SUBA-116`, `SUBA-118`,
> `SUBA-119`, `SUBA-124`, `SUBA-143` (medium); `SUBA-109`, `SUBA-129`, `SUBA-130`, `SUBA-131`, `SUBA-134`,
> `SUBA-139`, `SUBA-141` (low). By row:
>
> - `SUBA-133`: `advertise` is a strict boolean frontmatter field, editable through management and written
>   back. The bounded, XML-escaped `<advertised_subagents>` block is on the parent system prompt at
>   `before_agent_start` while `subagent` is selected, and it is replaced each turn. **Remaining delta:** name
>   ordering is lowercased byte order, not ICU root collation. `_` versus `-`/`.`/digits sorts differently,
>   and that can change which 16 agents make the cut. The fix is a CLDR-root primary-weight map for ASCII
>   punctuation.
> - `SUBA-136`: surfaced requests carry upstream's details, `tui/supervisor_ui.rs` ports `supervisor-ui.ts`,
>   and each reply journals one `subagent_supervisor_reply` entry. Two unfiled neighbours are outside this
>   row: v0.71.0 does not inject no-reply (`progress_update`) requests, and cyrup emits no
>   `INTERCOM_DETACH_REQUEST_EVENT`.
> - `SUBA-137`: the Ghostty gate also requires the trimmed `__CFBundleIdentifier` to be `com.mitchellh.ghostty`.
> - `SUBA-138`: RPC `cost` (the ninth method) and `ping.capabilities.cost` are in, and `/subagent-cost` shares
>   the same v0.71.0 collector. **Ledger correction:** at HEAD, `/subagent-cost` was an older transcript walk, not
>   upstream's collector. The unreferenced R-SA-140 accumulator in `registration/cost.rs` is still to delete.
> - `SUBA-140`: a non-empty `CYRUP_SUBAGENT_CACHE_RETENTION` becomes the child's `CYRUP_CACHE_RETENTION`.
> - `SUBA-134` (partial): derivation, the env hand-off, the child's `set_session_name` and `sessionName` on
>   results are in. The label arm (workflow node / chain step / dynamic task) is not: every caller passes
>   `label: None`. **Ledger correction:** the lane's "nothing is missing within the row" is wrong.
> - `SUBA-141` (partial): the runner-exit half is in (observed close, `exited with code N (signal S)`,
>   upstream's stderr-tail bounds). The Fleet external-CLI tails and step elapsed time are still open, because
>   `StepStatus` has no `externalProcess` and `exec/external_cli` has no live process hook.
>
> No other area file needed a note. `09a`'s pointers to `SUBA-133`/`134`/`136`/`137` (`09a:100-108`) are
> promotion pointers and still hold. `11`'s `ICOM-064` names `SUBA-134` as the claimant, and the child now
> sends that claim.

> ### CLOSURES 2026-09-28 — six lows (`SUBA-112`, `SUBA-126`, `SUBA-127`, `SUBA-128`, `SUBA-132`, `SUBA-135`); no partials
>
> Landed on `claude/lows-next`, not yet committed. Each row and body section carries its evidence. All six
> are fixes, and every closure has a test that the lane showed red without its fix. **The combined tree has
> not passed the gates.** The integration pass ran only `cargo fmt --all -- --check`, which passed and changed
> no files. Clippy and `cargo nextest` were not run on the combined diff, because `/` had 2.9 GB free, below
> the pass's 4 GB floor (`target/` is 24 GB). Until they run, treat "green" as per-lane only.
> **Counted set after this pass (`count_open_items.py`): 0 critical · 0 high · 8 medium · 12 low = 20 open,
> 2 trackers, 15 closed** (was 8 medium · 18 low = 26 open, 9 closed). By row:
>
> - `SUBA-112` — the bundled `worker.md` is byte-identical to `agents/worker.md` @v0.71.0: `defaultContext:
>   fresh` and the three prose drifts. **Ledger correction:** `acceptanceRole: writer` was already at HEAD
>   (`282b9fa2`, with the `SUBA-108` work); `SUBA-108`'s section carries a dated note.
> - `SUBA-126` — a structured-only answer is saved, delivered and chained as pretty JSON. "Blank" is measured
>   after `strip_acceptance_report`, as upstream measures it, and acceptance still reads the raw prose.
> - `SUBA-127` — the capture is read whenever `structured_output` was invoked, a valid value survives a failed
>   or timed-out run, and a rejected call gets the ported, bounded, redacted rejection summary. An interrupted
>   run is still not read (upstream's runner reads it; its foreground does not).
> - `SUBA-128` — `checkpointBeforeDeadlineMs` is ported as a tool parameter and a config key, resolved on the
>   async single launch, and delivered by the runner as upstream's `deadline-checkpoint` steer. The bounds
>   refusal covers every call shape. `SUBA-113` carries a dated note: `UNPORTED_CONFIG_KEYS` is 12 → 11.
> - `SUBA-132` — `SingleResult::tool_budget_blocked` is set from the child's own block message and settles a
>   workflow child `budget_exhausted`. Neighbouring gaps (upstream runner's looser match, the status-step
>   flag, `AbortRecoveryInput::tool_budget_exhausted`) are listed in the row, outside its scope.
> - `SUBA-135` — `preflightLaunchCwd` runs at the tool entry ahead of the mission binding, at the head of
>   `run_sync`, and before any async run exists. **Ledger correction:** the impact was understated: the mission
>   binding created the typo'd cwd and the child ran silently in it.
>
> No other area file needed a note: `09a`'s `SUBA-135` mention (`09a:109`) is a promotion pointer, and its
> claim still holds.

> ### RE-MEASURE — 2026-09-24 (pass 2) — the unread windows, read
>
> **Pins unchanged:** cyrup **`ea23ca2`**, pi-subagents **`v0.71.0`** (clone HEAD
> `v0.71.0-10-g6f1027f7`). Upstream read only as `git -C tmp/pi-subagents diff <tag>..<tag> -- <path>`,
> `git show <tag>:<path>` or `git show <sha>` for a commit inside a tagged window. No cargo run.
>
> **What this pass read.**
> 1. **`v0.67.0..v0.71.0`, line by line:** `src/runs/background/subagent-runner.ts` (the whole
>    1 914-line diff), `src/runs/foreground/execution.ts` (864 lines), `src/runs/foreground/subagent-executor.ts`
>    (1 411 lines), `src/runs/shared/structured-output.ts`, `src/runs/foreground/async-stop-action.ts`.
>    **Read with the Herdr-placement / model-ladder / completion-guard hunks filtered out** (those
>    are `SUBA-100`, `SUBA-109`, `SUBA-107` ground): `src/runs/background/async-execution.ts` and
>    `src/agents/agents.ts`. **Read by commit, hunk by hunk for the behaviour-bearing commits:** every
>    other `src/` file with >100 changed lines — `slash-commands.ts`, `notify.ts`, `child-session.ts`,
>    `pruned-fork.ts`, `subagent-wait.ts`, `wait-completions.ts`, `process-terminal.ts`,
>    `extension/index.ts`, `agent-management.ts`, `async-status-projection.ts`, `child-tool-plan.ts`,
>    `scripted-workflow.ts`, `acceptance.ts`, `shared/types.ts`, and the new `subagent-cost.ts`,
>    `tool-activation.ts`, `runner-http-dispatcher.ts`, `subagent-runner-bootstrap.ts`,
>    `herdr-*.ts`. The CHANGELOG was used as an index only.
> 2. **`v0.57.0..v0.67.0`:** every lead in `09a`'s census and every lead in this file was resolved
>    by reading both sides (dispositions: `09a` `## UNVERIFIED census 2026-09-14` → *Resolution*
>    table; this file's `## Leads`).
> 3. **The ten untagged commits `v0.71.0..6f1027f7`**, read as post-tag leads (`## Post-tag leads`).
>
> **Filed:** `SUBA-114`…`SUBA-143` (two high, eleven medium, sixteen low, one tracker). **Closed:**
> none. `SUBA-144`…`SUBA-148` were filed later by the low-severity batches, and the
> `v0.71.0..v0.74.0` pass filed `SUBA-150`…`SUBA-163`, and batch 6 filed `SUBA-149` in the gap that pass
> left. **Next free id: `SUBA-204`** *(2026-10-09: the pi v1.1.0 drift triage filed `SUBA-176`…`SUBA-203`. 2026-10-09: `SUBA-175` filed reviewing `SUBA-163`. 2026-10-03: the `v0.74.0..v0.75.0` pass filed `SUBA-164`…`SUBA-173`; before that the counter read `SUBA-164`)*. `SUBA-113`'s tracker escalated two of its keys
> (`checkpointBeforeDeadlineMs` → `SUBA-128`, `modelResponseAliases` → `SUBA-119`).
>
> **Still unread, stated exactly.** (a) The **`v0.57.0..v0.67.0` `src/` diff was NOT read line by
> line** (207 files, +24 658 / −9 124): this pass resolved the leads the 2026-09-14 census drew from
> it, not the diff itself. A behaviour change in a *modified* file of that window that no census lead
> names is still invisible. Reason: size — it is the largest unread block left in area 09, and a
> leads-first pass could not also walk it. (b) In `v0.67.0..v0.71.0`, the hunks of
> `async-execution.ts` and `agents.ts` that belong to Herdr placement were skipped (`SUBA-100`'s
> closure owns them), and the Node-runtime/jiti/Bun launch-resolution hunks of `async-execution.ts`
> and `subagent-runner-bootstrap.ts` were read but not compared (no cyrup counterpart: cyrup's runner
> is its own binary). (c) The async-workflow-only surfaces (`onChildSettled`, `workflowTerminalProof`,
> public workflow lifecycle events, routine intermediate wake suppression) were not compared hunk by
> hunk, because cyrup refuses async workflows outright (`extension/tool/routing.rs:587`); that
> refusal is `09`'s lead at `09:192`.

## Provenance and pins

| side | pin | how obtained |
|---|---|---|
| cyrup | **`ea23ca2`** (2026-09-24). Merge of #152; the code commit is `df3e9a8` | `git log -1` |
| pi-subagents | **`v0.71.0`**, the newest tag. Clone HEAD is `v0.71.0-10-g6f1027f7`; that past-the-tag range was read by pass 2 as leads only (`## Post-tag leads`) | `git -C tmp/pi-subagents tag --sort=-v:refname \| head -1`; `git describe --tags` |

Every upstream claim here was settled with `git -C tmp/pi-subagents show <tag>:<path>` at a named
tag, or with `git -C tmp/pi-subagents show <sha>` for a commit inside a named-tag window. None was read
from the working tree. Every cyrup claim was read at `ea23ca2`. **No cargo command was run**, because
the machine was busy with a coverage run, so every item here is a static reading.

**The crate's own baseline census at `ea23ca2`** (`grep -rhoE 'v0\.[0-9]+\.[0-9]+'
crates/cyrup-ext-subagents/src | sort | uniq -c | sort -rn`): **v0.68.0 × 840**, v0.43.0 × 668,
v0.64.0 × 377, v0.34.0 × 330, v0.57.0 × 128, v0.47.1 × 97, v0.66.0 × 25, v0.67.0 × 9, and **zero**
citations of v0.69.0, v0.70.x or v0.71.0. The crate is now ported *to* v0.68.0 on most of its
surface; it was at v0.43.0 plus patches on 2026-09-14. **So everything in `v0.68.0..v0.71.0` is
genuine lag. For `v0.67.0..v0.68.0`, check the `@v0.68.0` citations before filing anything as
absent.**

## Scope

**Port measured:** `crates/cyrup-ext-subagents/` at `ea23ca2`, which is 489 `.rs` files and 401 210
lines under `src/` (tests included).

**Upstream windows.** Each was measured in this clone with `git diff --shortstat` and
`git log --oneline --no-merges`:

| window | whole tree | `src/` | non-merge commits | `src/` files added / deleted | read by |
|---|---|---|---|---|---|
| `v0.57.0..v0.67.0` | 496 files, +73 195 / −25 830 | 207 files, +24 658 / −9 124 | 341 | 54 / 5 | **leads only** (the `09` and `09a` census blocks, 2026-09-14). One item, `SUBA-092`, is filed in `09a`. *Pass 2: every lead resolved on both sides; the diff itself still not walked line by line* |
| `v0.67.0..v0.68.0` | 252 files, +11 943 / −8 502 | 97 files, +4 139 / −2 491 | 78 | 11 / 3 | **mostly already ported by cyrup** (its v0.68.0 citations). `09a` filed and closed `SUBA-099`…`SUBA-105` against `@v0.68.0` surfaces: machine placement, `inheritGlobalContext`, `mutationTools`, `fast`, capability rows and `acceptance.report`. It holds `SUBA-106` (agent `outputSchema`) open, and that row was re-read by this pass (see below). Apart from that, this pass read only the three deletions and the `Removed` changelog entry |
| `v0.68.0..v0.71.0` | 256 files, +13 752 / −6 103 | 95 files, +3 285 / −2 442 | 80 | 8 / 4 | **this pass**: the CHANGELOG entries for 0.69.0, 0.70.0, 0.70.1 and 0.71.0; every `src/` add and delete; bundled `agents/` diffs |
| **`v0.67.0..v0.71.0`** (the brand-new window) | 386 files, +25 038 / −13 948 | 147 files, +7 377 / −4 886 | 158 | — | the two rows above |
| **`v0.57.0..v0.71.0`** (this file's scope) | 601 files, +90 655 / −32 200 | 238 files, +29 914 / −11 889 | 499 | 70 / 9 | — |

**The `src/` deletions in `v0.67.0..v0.71.0` matter most.** cyrup ported every one of these files, so
each is a candidate `stale-port`:

| deleted upstream | removed at | commit | cyrup still carries |
|---|---|---|---|
| `src/runs/shared/model-exclusions.ts` | v0.68.0 | `f58dfcb5` *refactor: remove automatic model fallback (#2270)* | `exec/model_exclusions/` (5 modules) → `SUBA-109` |
| `src/runs/shared/model-fallback.ts` (renamed to `model-resolution.ts`, 62 % similar) | v0.68.0 | `f58dfcb5` | `exec/fallback.rs::run_fallback_ladder` → `SUBA-109` |
| `src/runs/shared/readonly-model-continuation.ts` | v0.68.0 | `f58dfcb5` | not traced |
| `src/runs/shared/completion-guard.ts` | v0.70.1 | `7c98a696` *refactor: remove inferred no-edit completion failures (#2356)* | `exec/completion_guard.rs` → `SUBA-107` |
| `src/runs/shared/task-intent.ts` | v0.70.1 | `7c98a696` | `exec/task_intent.rs` → `SUBA-107`, `SUBA-108` |
| `src/runs/shared/llm-intent-arbiter.ts` | v0.70.1 | `7c98a696` | nothing: `llm_intent` / `LLM_INTENT` have zero hits in `crates/`. It was never ported, so nothing is stale |
| `src/runs/shared/completion-evidence.ts`, `readonly-session-evidence.ts` | `v0.68.0..v0.71.0` | not attributed | not traced |

Presence was settled with `git cat-file -e <tag>:<path>` at v0.67.0, v0.68.0, v0.69.0, v0.70.0,
v0.70.1 and v0.71.0.

## Methodology

1. `git diff --name-status v0.67.0..v0.71.0 -- src` gave the adds, deletes and the one rename. Each
   delete was dated by tag with `git cat-file -e`, then attributed with
   `git log --diff-filter=D <window> -- <path>`.
2. The CHANGELOG sections `0.68.0`…`0.71.0` came from `git show v0.71.0:CHANGELOG.md`. They were
   used **only as an index**. Each item below re-reads the source at a tag. A changelog line is a
   claim, not evidence.
3. For each candidate: a cyrup grep for the upstream symbol, config key and env name, then a read
   of the Rust that the grep landed on. An item is filed only when both sides were read. Everything
   else is under `## Leads`.
4. No commit message on either side was taken as evidence. That includes #152's "delegation that
   works end to end" (see `09`'s `SUBA-022` row).

## Open items

> Kept under the heading `## Open items`, with the standard `ID | Severity | Kind | Effort | Title`
> table, as README's *Item format* requires. **`09a`'s `## Summary — confirmed items` shape is NOT
> copied**, because README requires one `## Open items` table per file. `scripts/count_open_items.py`
> reads this file as area `09b` since the 2026-09-24 synthesis pass.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| SUBA-174 | ~~medium~~ **CLOSED 2026-10-05** | parity-bug | M | **Every workflow script can call `runs.host`; upstream grants it only to a resource-provenance run, gated per key** — upstream registers the `runs.host` op ONLY for a run whose script came from a named workflow resource (`scripted-workflow.ts:2060-2110` @v0.75.0; a raw `workflowScript` isolate never links it) and authorizes each key against that resource's `WorkflowResourcePermit`. In cyrup `WorkflowRunHost::supports_host` returns `true` **unconditionally** (`extension/executor/workflow.rs:1328`, and its own doc comment says so), and `extension/executor/workflow_launch.rs:253` passes `one_use_permit: None` with the comment *"No resource provenance is wired in this build"* — so a named resource's host grant is not the gate it is upstream, and any workflow script reaches the host-command op. **FILED 2026-10-04** by the `SUBA-150` lane, which found it while porting the `workflow` field and could not fix it (the file belongs to another lane's territory in that pass). **Pre-existing and structural, not introduced by `SUBA-150`:** a raw `workflow: "./x.js"` script could already call `runs.host` before that change, and the two builtin resources' expansions are bounded by the resolver's own whitelist (`resolve_run_ci` accepts only `npm test` / `npm run typecheck`), so making resources reachable added no authority. **Fix** — thread the resolved resource's permit from the boundary to `drive_workflow_run` (upstream keys a `WeakMap` on the params object; cyrup needs an explicit side channel) and make `supports_host` provenance-gated rather than constant, then authorize per key. **Verify** — a raw-script workflow is refused `runs.host`; a named resource is granted only the keys its permit lists; the existing whitelist still bounds what a granted key may run. **CLOSED 2026-10-05.** All three Verify clauses now hold, each proved by a mutation. `supports_host` is provenance-gated on a CONSUMED permit whose authority declares a host grant list (`extension/executor/workflow.rs:1385`), `host_command` authorizes per key (`:1433-1434`), and the resolved permit is threaded from the boundary (`extension/tool/routing.rs:515`) to `drive_workflow_run` as a move rather than upstream's params-keyed `WeakMap`. **This row's own upstream cite was wrong and is superseded:** `scripted-workflow.ts:2060-2110` is `isZeroUsage` and `setupAbortResumeParams`. The real linkage is a NESTED conditional — `workflowResource ? (authority.host ? runHostCommand : undefined) : (publicExecution ? undefined : runHostCommand)` (`src/runs/foreground/subagent-executor.ts:5965-5967` @v0.75.0; note the v0.75.0 path, not `src/extension/`) — with per-key authority at `shared/workflow-child-permit.ts:205-212`. Reading only its first arm suggests a raw script keeps the op; it does not, because BOTH of cyrup's `drive_workflow_run` callers are upstream `executePublic` runs (`executeScheduled` delegates to `executePublic` at `:8011`, which adds to the `publicExecutions` set at `:7960`), so the second arm is `undefined` for every run cyrup can produce. The quoted cyrup line numbers above (`workflow.rs:1328`, `workflow_launch.rs:253`) are pre-fix and have moved. **Three reachability proofs were preserved, not rewritten away:** the receipt's upsert-by-id, the failure arm carrying `error.partial` host steps, and `host_steps_for_run` read back off a real `status.json` by `collect_fleet_history`. The three dispatch tests that previously drove a RAW script and asserted `runs.host` succeeded now run on a session-registered resource (binding host services is required: resource lookup is scoped by `current_session_id()`, and an unbound executor resolves BUILTINS ONLY, so a custom registration is invisible — and `run-ci` cannot substitute, its whitelist being `npm test` / `npm run typecheck` while those tests need exit codes 0, 7 and 9). New at this altitude: `a_raw_script_file_is_refused_runs_host_at_the_dispatch`, which closes a genuine hole — the engine's own gate (`scripted/engine.rs`'s `if !shared.host.supports_host()`) had NO test, the two existing unit cases calling `host_command` directly reaching only the defence-in-depth arm INSIDE the method. It asserts the refusal's shape, not just its text: `typeof runs.host` stays `"function"` (the member is installed unconditionally, `prelude.js:574`'s delete branch never taken) and `hostSteps` is ABSENT — the latter being the only assertion that catches a revert of `supports_host`, since the inner arm emits the identical sentence one layer further in. Gates: clippy clean on a real `Checking` run (2m36s, not a fingerprint hit); crate suite 4999 run / 4999 passed / 0 failed, up from 4998 / 4995 / 3. |
| ~~SUBA-110~~ | ~~high~~ **CLOSED 2026-09-24** | upstream-drift | S | Git routing variables (`GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`, `GIT_CONFIG_*`, …) are not removed from the background runner's environment or from an inherited external-CLI environment — **CLOSED 2026-09-24** (`c936d8c`): new `spawn/git_env.rs` ports `git-environment.ts` (the 16 names plus `GIT_CONFIG_{KEY,VALUE}_<n>`, case-insensitive); applied before the overlay in `background/spawn_detached.rs` and on the inherited arm of `exec/external_cli/run.rs`; an adapter allowlist is left as written. Verify: `spawn::git_env::tests` (predicate; removals-then-overlay; a real `git rev-parse` resolves the parent repo through an inherited `GIT_DIR` and the child's own repo once filtered). The two call sites are not driven by a test (`#![forbid(unsafe_code)]` rules out setting `GIT_DIR` on the test process). |
| ~~SUBA-107~~ | ~~medium~~ **CLOSED 2026-09-29** | stale-port | M | The completion-mutation guard still fails a successful run because its task wording "looked like" an implementation task. Upstream deleted the guard at v0.70.1 — **CLOSED 2026-09-29**: the guard is retired, matching upstream: `pi-subagents 7c98a696` deletes `src/runs/shared/{completion-guard,task-intent,completion-evidence,llm-intent-arbiter}.ts` and first appears at tag v0.70.1, present at v0.71.0. `apply_completion_guard` is gone from `run_sync`'s gate sequence, `exec/completion_guard.rs` and `exec/task_intent.rs` are deleted, `completionGuard` is out of `KNOWN_FIELDS`, `apply_agent_config`, `RecoveryDescriptor` and `AgentDefinition`, and the acceptance `>= Checked` rung no longer caps on `completion_guard.triggered`. The two helper families that SURVIVE upstream were re-homed rather than deleted, to the files upstream keeps them in: `is_mutating_bash_command`/`is_mutating_tool` to `exec/control.rs` (upstream's `src/runs/shared/long-running-guard.ts:138,155`) and the output write-capability predicate to `exec/output.rs` (`src/runs/shared/single-output.ts:6-14`, twelve names in source order). Verify: `cyrup-it subagents::completion_guard_retirement::an_implementation_task_with_no_edits_finishes_clean`, `::the_retired_completion_guard_symbols_have_zero_hits_in_the_workspace`. |
| ~~SUBA-108~~ | ~~medium~~ **CLOSED 2026-09-29** | stale-port | M | Acceptance-level inference still reads task wording and agent-name regexes. At v0.70.1 upstream infers only from the declared `acceptanceRole` — **CLOSED 2026-09-29**: inference reads the declared `acceptanceRole` and nothing else: `exec/acceptance/model/level.rs::infer_level` is verbatim `pi-subagents v0.71.0 src/runs/shared/acceptance.ts:81-122`, diffed branch by branch. `AcceptanceContract::resolve_effective_for_role` now runs upstream's whole `:505-517` resolution -- the policy-presence upgrade, the MAX, the evidence union against the UPGRADED inferred level, criteria re-normalized against the merged evidence, the review fallback and `:521-522`'s none-level clear -- rather than a bare level MAX, and `AcceptanceConfig` gained `report` so `explicit_acceptance_requests_policy` reads all seven modeled non-level keys (report, criteria, evidence, verify, review, stop_rules, reason). An explicit policy therefore INHERITS the inferred branch's criteria and `requiredEvidenceForLevel`, which is what `resolveEffectiveAcceptance` gives for `{level:"checked"}` with `acceptanceRole: undefined` and what `evaluateAcceptance` (`:1476-1633`) then gates on, where cyrup's explicit contract previously gated on nothing. Verify: `exec::acceptance::model::level::tests::inference_is_invariant_across_task_text_and_agent_name` (3 roles x 3 launch shapes x 3 task texts x 2 agent names), `cyrup-it subagents::read_only_acceptance_inference::a_lowered_policy_survives_a_declared_read_only_role_at_the_live_seam`, `::a_report_only_policy_upgrades_a_declared_read_only_role`, `::an_empty_policy_does_not_upgrade_a_declared_read_only_role`. |
| ~~SUBA-111~~ | ~~medium~~ **CLOSED 2026-09-29** | upstream-drift | M | Agent `allowedAgents` (frontmatter and `agentOverrides`, v0.70.0) is unported. The frontmatter key is silently kept as an extra field, so a declared delegation restriction fails open — **CLOSED 2026-09-29**: `allowedAgents` is ported on both declaration paths and enforced on the child. `discovery/frontmatter.rs:989-1017` parses the frontmatter key through the shared capability-ceiling normalizer with its verbatim message -- `pi-subagents v0.71.0 src/runs/shared/capability-ceiling.ts:66-95`: trim, `^[A-Za-z0-9_.:-]+$`, <=128 UTF-8 bytes per entry, <=256 entries, deduplicated, SORTED -- as `src/agents/agents.ts:2118-2119` does, and `allowedAgents` joined `KNOWN_FIELDS` so it stops leaking to `extra_fields`; `AgentOverrideConfig::allowed_agents` is an `OverrideField` whose `ExplicitClear` is pi's `delete next.allowedAgents` (`agents.ts:1140-1141`, where a JSON `false` clears the bound); `exec/spawn_plan.rs::env_orchestration` builds the agent-sourced ceiling `{version:1, allowedAgents:[...], denyExtensions:false, sources:["agent:<name>"]}` and intersects it into what crosses the process boundary AFTER planning (`src/runs/shared/child-launch.ts:185-187,:211`), so an agent's own bound never narrows its own tool plan; and the bound rides the `RecoveryDescriptor` and its launch-binding projection and is emitted by `discovery/management/frontmatter_write.rs` so a management rewrite cannot delete it. Verify: `exec::spawn_plan::tests::an_agents_own_allowed_agents_becomes_an_agent_sourced_ceiling_on_its_child`, `::an_agent_sourced_ceiling_can_only_narrow_the_inherited_one`, `::an_agent_naming_other_agents_must_still_launch_itself`, `discovery::frontmatter::tests::allowed_agents_frontmatter_parses_normalized_and_is_never_demoted`, `discovery::merge::tests::an_allowed_agents_override_sets_a_bound_and_a_json_false_clears_one`. |
| SUBA-109 | low | stale-port | M | `fallbackModels`, the same-launch model ladder and persistent model exclusions are still live. Since v0.68.0 upstream rejects the key by name |
| ~~SUBA-112~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): `crates/cyrup-ext-subagents/resources/agents/worker.md` is now byte-identical to `agents/worker.md` @v0.71.0 (`diff` against `git show v0.71.0:agents/worker.md` is empty): `:11 defaultContext: fresh`, plus the three prose drifts the Fix listed (the `First read the provided context…` paragraph, the `contact_supervisor`-unavailable fallback, the source-discoverability bullet). A worker launched with no `context` now resolves to fresh even when a fork is available. Upstream `agents/worker.md:5,:11` @v0.71.0, CHANGELOG 0.71.0 (#2384). Verify: `cyrup-ext-subagents tests::discovery_integration::the_bundled_worker_starts_fresh_and_is_a_writer` (red without the fix). **Ledger correction:** `acceptanceRole: writer` (`:5`) was already at HEAD `48da124d`; it landed in `282b9fa2` with the `SUBA-108` work. Only `defaultContext: fork` and the three prose drifts were still stale. — *Original:* The bundled `worker` still has `defaultContext: fork` (upstream: `fresh`, v0.71.0) and has no `acceptanceRole: writer` (v0.70.1) |
| SUBA-113 | tracker | tracker | — | Thirteen `config.json` keys are declared unported in source (`registration/mod.rs::UNPORTED_CONFIG_KEYS`), which says "the ledger carries them". No ledger item did until this row — **2026-09-28:** `checkpointBeforeDeadlineMs` left `UNPORTED_CONFIG_KEYS` with `SUBA-128`'s closure (on `claude/lows-next`); the const now holds 11 entries |
| ~~SUBA-114~~ | ~~high~~ **CLOSED 2026-09-26** | stale-port | M | Child tool plans are still pruned to the PARENT session's tool registry, and reviewer/scout launches are refused when the parent was started with a narrow `--tools`. Upstream removed the prediction at v0.70.0 — **CLOSED 2026-09-26** (`850eb70`, #156): `b12496b8` ported — `exec/tool_surface.rs` has no parent-registry input (`host_builtin_tool_names`, the partition, the host `read` throw, the review/scout lane refusal, the omission warning and `unavailableHostBuiltins`/`warnings` are gone), and `RunOptions`/`RunnerConfig`/`ExecSingleStepExecutor` no longer carry `host_available_builtins` (an old runner config carrying the key still decodes). The child-side guard is brought to v0.71.0 (#1356): no builtin floor, and the child ABORTS its run at `agent_start` when its real registry lacks a required tool (`prompt_runtime::refresh_tool_diagnostic`). Verify: `cyrup-it --test subagents child_tool_plan_not_predicted_from_parent::*` (5, real parent `AgentSession`). See the section. |
| ~~SUBA-115~~ | ~~high~~ **CLOSED 2026-09-24** | upstream-drift | S | A nested run's stop / interrupt / timeout cascade reaches every live run on the ROOT route, including sibling subtrees it did not launch (v0.68.0 confines it to the issuing subtree) — **CLOSED 2026-09-24** (`7ba9e03`): `background/cascade.rs::is_control_descendant` ports `isNestedControlDescendant`; `settle.rs::cascade_to_descendants` passes this run's id as issuer when `nested_self` is set, for stop, interrupt and timeout. Verify: `background::cascade::tests::a_nested_issuer_reaches_only_its_own_subtree` (red with the filter removed). Reach note: cyrup does not yet mint a nested route for its own children (`exec/spawn_plan.rs:993`), so today the cascade only has a route when cyrup was itself launched under an inherited one. |
| ~~SUBA-116~~ | ~~medium~~ **CLOSED 2026-09-29** | upstream-drift | M | A paused async run cannot be stopped (refused as "No running or queued async run"), so it keeps its active-capacity slot until resumed — **CLOSED 2026-09-29**: a paused async run can be stopped and releases its slot. `background/control.rs::seal_paused_run` (:1090-1259) ports `sealPausedRun` from `pi-subagents v0.71.0 src/runs/foreground/async-stop-action.ts:31-101`, behind upstream's widened `pausedWholeRun` guard at `:120-121` (`state == Paused && child_id.is_none()`, so a child-scoped stop is still refused): the five refusal rungs in upstream's order -- `status.processTerminal?.runnerProcessInstanceId`, the sidecar proof matched on state+runId+instance with upstream's `unknown (<reason>)` / state-word / `missing` interpolation, `sessionId`, `resultPayloadPathForSessionRun`, then parse and identity -- delivery before seal (`:149-151`), the step rewrite whose `!is_terminal()` is byte-equal to upstream's `pending\|running\|paused` with `endedAt` assigned rather than filled, the per-entry result rewrite indexed against the step list with `exitCode: 1`, the run-level rewrite, and `updateActiveRunIndex(..., "stopped", ...)`. `Paused -> Stopped` is a first-class terminal transition (`background/state.rs:213`), so the run stops holding its active-capacity slot until resumed, and the refusal sentence an operator actually hits (`:154`) is now pinned end to end at the action boundary. No runner change was needed: `runner_main/finish.rs:60-67` maps `LoopOutcome::Interrupted` to `Paused` and then EXITS, so a cyrup `Paused` run has no live runner for upstream's widened `stopRunner` to reach. Verify: `background::control::tests::stopping_a_paused_run_seals_it_stopped_and_releases_its_capacity_slot`, `::a_child_scoped_stop_of_a_paused_run_is_still_refused`, `::a_paused_run_whose_terminal_proof_is_not_observed_keeps_the_stop_request_and_refuses`, `background::state::tests::stopped_is_a_first_class_terminal_run_state_not_an_alias`, `extension::executor::foreground_actions::stop::tests::control_stop_admits_a_paused_run_and_reports_the_seal_that_is_not_ready_yet`. |
| ~~SUBA-117~~ | ~~medium~~ **CLOSED 2026-09-27** | parity-bug | S | The worktree clean-tree check does not exclude the crate's own `.cyrup-subagents/` project directory, so the crate's own chain-run / refinement / schedule files make `worktree: true` refuse a clean repository — **CLOSED 2026-09-27**: the clean-tree probe excludes the crate's own project artifact root (`crates/cyrup-ext-subagents/src/spawn/worktree.rs:495-498`, `:491-493` recording the pathspec resolution). Verify: `cyrup-ext-subagents spawn::worktree::tests::create_worktrees_ignores_the_project_artifact_root`. |
| ~~SUBA-118~~ | ~~medium~~ **CLOSED 2026-09-29** | upstream-drift | M | Abort recovery is unported: a child whose provider/transport aborted after compaction settled, with useful progress, fails instead of being resumed once — **CLOSED 2026-09-29**: abort recovery is dispatched, not merely decided -- the 27-line comment at `exec/fallback.rs` asserting it could not be wired is deleted with its false premise. `DriveOutcome` now carries `after_compaction_settlement` (pi's `AFTER_COMPACTION_SETTLEMENT` stamp, `pi-subagents v0.71.0 src/runs/foreground/execution.ts:1418-1420`) and a `tool_budget_blocked` latch out of every settle path; `AttemptRecord` carries both plus a per-attempt `structured_output_failed` computed at upstream's exact position in the diagnosis chain, between `:1445-1447` and `:1472-1493`; `AttemptRunner` gains `plan_abort_recovery`/`apply_abort_recovery` on the existing `wait_startup_retry`/`apply_startup_outcome` split, with a default that settles on upstream's own `retained session unavailable`; and `classify_attempt` gains a `ResumeAbort` rung at upstream's precedence -- below the startup ladder, because upstream's startup loop lives INSIDE `runSingleAttempt` at `:1518` and is fully spent before the abort loop wrapping it gets a say, and above the context-overflow classification, which upstream decides only after its loop breaks (`:1905`). The shell latches `abort_resumed` per candidate (pi's `attemptIndex > 0`), pushes `AttemptNote::AbortRecovery` whose `Display` is `:1881` verbatim, swaps the attempt's task text to `ABORT_RECOVERY_PROMPT` while the same `--session <file>` argv makes it a resume, resets the startup counter, relaunches the same candidate, and joins the plan's diagnostic onto the attempt's error and the serialized `ModelAttempt` row (`:1886-1888`); `usage_budget_exhausted` and `acceptance_failed` are spelled out as the foreground literals at `:1885` and `:1887` rather than silently omitted. Verify: `tests::abort_recovery_integration::a_compaction_induced_abort_is_resumed_once_with_the_recovery_prompt_and_the_run_succeeds`, `::the_same_abort_without_the_compaction_evidence_settles_with_no_relaunch`, `::a_child_its_own_tool_budget_stopped_is_not_resumed`, `::a_declared_schema_with_no_captured_value_refuses_the_resume_and_says_why`, `exec::fallback::tests::the_resume_rung_sits_below_startup_retry_and_above_context_overflow`, `::a_timeout_a_detach_and_a_success_all_outrank_the_abort_recovery_rung`. |
| ~~SUBA-119~~ | ~~medium~~ **CLOSED 2026-09-29** | not-ported | M | A native child that reports a different model than the launch candidate is accepted silently: no `model_verification_failed`, no `modelResponseAliases` — **CLOSED 2026-09-29**: a child reporting a different model than the launch candidate now latches the verification error. `exec/model_verification.rs::format_subagent_model_verification_error` reproduces every clause of `pi-subagents v0.71.0 src/runs/shared/model-resolution.ts:13-33` in order -- the empty-registry off-switch, the alias match against the RAW observed id, base equality, the `fullId` lookup, `id` equality and both id leaves -- and the `verifyModel` gate is `(!model_override_from_parent).then(\|\| plan.model_arg.clone())` at `exec/attempt_runner.rs:671` (upstream's extra `Boolean(candidate)` conjunct is unreachable because `spawn_plan.rs:465` builds `model_arg` from a resolved non-empty `ModelId`). `modelResponseAliases` reaches all three production seams and each has its own test -- `extension/executor/chain.rs:110`, `extension/executor/background.rs:821`, `background/runner_main/turn_loop.rs:423` -- and `BackgroundStepsSpec.model_response_aliases` is the two-variant `RetainedModelResponseAliases`, matched exhaustively with NO fallback on the `Retained` arm, so `Retained(None)` is upstream's meaningful absence (`src/runs/foreground/subagent-executor.ts:2135-2136`) rather than a silent fall-through to the live config. Verify: `exec::drive_attempt::tests::a_child_reporting_a_different_model_latches_the_verification_error`, `::a_declared_alias_on_the_run_options_clears_the_verification_error`, `::a_message_end_carrying_a_tool_call_is_not_verified`, `extension::executor::background::tests::recovery_descriptor::a_revive_whose_descriptor_declared_no_alias_gets_none_even_when_the_live_config_declares_one`, `background::runner_main::turn_loop::tests::the_runner_configs_alias_map_reaches_the_step_executor`, `extension::executor::chain::tests::a_foreground_chain_walk_carries_the_config_alias_map`. |
| ~~SUBA-120~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | M | Watchdog findings have no `importance`; every finding above threshold is delivered into the parent model's context, where upstream sends only `high` — **CLOSED 2026-09-27**: `importance` is required with no default and `route_warning` is wired; the false claim that upstream's notice path has no importance filter is corrected at `crates/cyrup-ext-subagents/src/watchdog/types.rs:118-148`, and `parse_test_command` now requires a non-empty remainder after ONE separator (`watchdog/register_main.rs:662`). Verify: `cyrup-ext-subagents watchdog::register_main::tests::exactly_one_trailing_space_is_not_a_test_command_but_two_are`. |
| ~~SUBA-121~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | S | Runtime-registered agents never receive `subagents.defaultModel` / `defaultProvider` / `defaultThinking` or `agentOverrides.<name>` model / provider / fast / thinking — **CLOSED 2026-09-27**: the one line that makes `subagents.defaultModel` / `agentOverrides.<name>` reach a runtime-registered agent (`crates/cyrup-ext-subagents/src/discovery/mod.rs:2074-2076`) is now covered end to end by a real discovery integration test. Verify: `cyrup-ext-subagents tests::runtime_agent_registration_integration::settings_defaults_and_narrowed_overrides_reach_a_runtime_agent_through_discovery`, plus the four `discovery::merge::tests` unit tests. |
| ~~SUBA-122~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | S | Agent-name resolution does not prefer a canonical name over a packaged short name, so `scout` beside `code-analysis.scout` is refused as ambiguous — **CLOSED 2026-09-27**: `resolve_agent_name` is split into a canonical pass and a local-name pass, in that order, each with its own ambiguity message (`crates/cyrup-ext-subagents/src/discovery/mod.rs:578-613`). Verify: `cyrup-ext-subagents discovery::tests::{a_canonical_name_beats_another_agents_local_name_of_the_same_string,two_distinct_agents_sharing_a_local_name_are_a_local_ambiguity_error}`. |
| ~~SUBA-123~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | S | Top-level `subagents.agentScanDirs`, `agentExcludeDirs` and `defaultSubagentOnlyExtensions` are silently dropped: the key census walks only `agentOverrides.<name>` — **CLOSED 2026-09-27**: all three keys are parsed, validated with upstream's message shape, trimmed and applied — `agentScanDirs` (tracked as SUBA-123a), `defaultSubagentOnlyExtensions` (SUBA-123b) and `agentExcludeDirs` (SUBA-123c) — and the key census now warns on an unported top-level key (`crates/cyrup-ext-subagents/src/discovery/{types.rs:955-974,mod.rs:962-966,1283-1288,1342-1366,1500-1501},agent_dirs.rs:186-277`). Verify: `cyrup-ext-subagents discovery::agent_scan_and_exclude_dir_tests::*` (6), `discovery::merge::tests::*default_subagent_only_extensions*` (5), `discovery::tests::{an_unknown_top_level_subagents_key_warns,a_ported_top_level_subagents_key_does_not_warn}`. |
| ~~SUBA-124~~ | ~~medium~~ **CLOSED 2026-09-29** | upstream-drift | M | `mcpDirectTools` cannot resolve servers contributed by settings `packages` or `agentPluginPaths`; a grant naming one resolves to no tools, silently — **CLOSED 2026-09-29**: new `exec/mcp_config_sources.rs` is a line-referenced port of `pi-subagents v0.71.0 src/runs/shared/mcp-config-sources.ts` following upstream's own file split: `isMcpServerDefinition`, `loadPackageMcpServers` with `getConfiguredPackageRoots`/`resolvePackageRoot` (`npm:`, `git:`/scp/URL, `file:`/`~`/absolute/relative), `parseNpmPackageName`, `parseGitPackagePath`, `resolvePackageConfigPath` with both lexical and realpath containment, `loadAgentPluginMcpServers` with the manifest/config `$schema` equality gates, `PLUGIN_NAME_PATTERN`, the per-transport field allowlists, stdio translation (`./` command contained in the plugin root, `${PLUGIN_ROOT}`/`${PLUGIN_DATA}` expansion, the injected vars and their shadow refusal, `resolvePluginCwd`), http translation (URL safety, loopback-only plaintext, WHATWG header name/value and case-collision refusal) and `formatName`. It is wired into `load_mcp_config` at upstream's exact precedence, `mergeConfigs({packageOnly}, mergeConfigs({plugin}, config))` (`src/runs/shared/mcp-direct-tool-allowlist.ts:243-262,:302-321`) -- mcp.json > plugin > package, with a package server whose normalized name a plugin claims dropped outright. `cyrup-mcp` deliberately stays a dev-dependency so resolving `mcp:` selectors cannot drag rmcp/reqwest/oauth2 into a spawn; the two ports are instead held together by a cross-crate conformance test that drives `cyrup_mcp::agent_plugin::load_agent_plugins_in` over the same plugin directory and asserts identical namespaced names and an identical 15-key `configHash` pre-image. Verify: `exec::mcp_direct_tools::tests::an_agent_plugin_server_resolves_its_direct_tools`, `::a_settings_package_server_resolves_its_direct_tools`, `::a_plugin_server_beats_a_package_server_of_the_same_normalized_name`, `::an_mcp_json_server_beats_a_plugin_server_of_the_same_name`, `::a_project_settings_block_does_not_erase_a_global_tool_prefix`, `::a_non_string_agent_plugin_path_is_skipped_and_its_neighbours_still_load`, `::the_plugin_translation_agrees_with_the_adapters_own_loader`. |
| ~~SUBA-125~~ | ~~medium~~ **CLOSED 2026-09-27** | upstream-drift | S | Skills marked `disable-model-invocation: true` are still injected into a child that names them — **CLOSED 2026-09-27**: `disable_model_invocation` is carried onto `ResolvedSkill` and filtered at the injection chokepoint (`crates/cyrup-ext-subagents/src/discovery/skills.rs:78,96-100,158,274,302`), and the proactive recommender skips hidden skills too (`:571`). Verify: `cyrup-ext-subagents discovery::skills::tests::{an_agent_naming_a_hidden_skill_launches_without_it_in_the_child_prompt,build_skill_injection_omits_a_hidden_skill,a_hidden_skill_is_not_proactively_recommended}`. |
| ~~SUBA-143~~ | ~~medium~~ **CLOSED 2026-09-29** | upstream-drift | L | The runtime-agent registration EVENT bridge (`pi-subagents:runtime-agent-register:v1`) is unported; only native Rust code can register a runtime agent — **CLOSED 2026-09-29**: the registration event bridge is ported. New `discovery/runtime_agent_events.rs` holds the whole of `pi-subagents v0.71.0 src/agents/runtime-agent-events.ts`: `RUNTIME_AGENT_REGISTER_EVENT` and `_VERSION` (`:4-5`, upstream's exact constant), the request builders, `read_registration_reply`/`read_disposal_reply` as the reading half of `registerAgentViaEvents` (`:35-46`, carrying pi's two refusal sentences verbatim), and `RuntimeAgentEventBridge::dispatch` as the body of `registerRuntimeAgentEventListener` (`:49-70`) -- routing to the existing `RuntimeAgentRegistry::register_value`, so every refusal a client sees is the registry's, i.e. upstream's, sentence. Because `SharedBus::emit` queues and fans out BY VALUE (`cyrup-ext/src/bus.rs:80-93`), upstream's mutated-payload return channel does not exist, so the result travels on `...:reply:<requestId>` in upstream's shapes, and the handle upstream returns as `dispose()` (`src/agents/runtime-agent-registry.ts:386-397`) is a `Uuid::new_v4` token plus a disposal topic -- random rather than a counter so one client cannot dispose another's registration, naming exactly one RECORD (upstream's `entry !== record` identity, not name equality), idempotent and infallible like `dispose(): void`, and removed rather than tombstoned so a register/dispose loop cannot grow the bridge. The `requestId` is validated FIRST because the reply topic is built from it, and only then the version, refused with upstream's sentence including its `String(request.version)` interpolation. Both topics are subscribed in the `RegistrationMode::Full` arm only, and the token map is cleared in the `SessionShutdown` arm right after `teardown_session()` runs pi's `clearRuntimeAgentsForPi` (`src/extension/index.ts:1042`). Verify: `tests::runtime_agent_event_bridge_integration::a_sibling_extension_registers_an_agent_over_the_bus_and_discovery_finds_it`, `::the_registration_token_disposes_the_agent_and_is_idempotent`, `::session_shutdown_clears_the_registry_and_the_outstanding_tokens`, `::a_forged_reply_topic_is_never_written_to`, `::a_child_safe_registration_answers_no_runtime_agent_event_at_all`, `discovery::runtime_agent_events::tests::an_unsupported_version_is_refused_with_upstreams_sentence`, `::a_stale_token_never_removes_a_later_registration_of_the_same_name`. |
| ~~SUBA-126~~ | ~~low~~ **CLOSED 2026-09-28** | not-ported | S | **CLOSED 2026-09-28** (on `claude/lows-next`): `run_sync` reads the structured value before the output handoff and, when the prose is blank, substitutes `serde_json::to_string_pretty(value)`, so the value is saved to the output file, delivered, and passed on as `{previous}` (`crates/cyrup-ext-subagents/src/exec/mod.rs:739-758`, ahead of `resolve_saved_output` at `:772`). Acceptance still reads the child's raw prose (`:786-790`). Upstream `execution.ts:1513-1516`, `subagent-runner.ts:1372-1376` @v0.71.0. Verify: `cyrup-ext-subagents tests::structured_output_results_integration::{a_structured_only_answer_is_saved_and_delivered_as_pretty_json,prose_that_is_only_an_acceptance_report_still_delivers_the_structured_value}` (red without the fix). The verifier fixed one gap: "blank" is measured after `strip_acceptance_report`, as upstream measures it after `stripAcceptanceReport`; before that, prose that was only an acceptance-report block saved the machine report to the output file and delivered nothing. — *Original:* A child that answers only through `structured_output` delivers an empty output and saves an empty output file; upstream substitutes the pretty-printed JSON |
| ~~SUBA-127~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): the capture is read whenever the winning attempt's events show a `tool_execution_start` for `structured_output` (`crates/cyrup-ext-subagents/src/exec/structured.rs:401` `structured_output_tool_invoked`; `exec/mod.rs:1716-1777` `GateState::apply_structured_output_value`), and a valid value survives a failed or timed-out run. An invoked call that captured nothing fails with the ported `formatStructuredOutputRejectionError` (`exec/structured.rs:526`): bounded to 4096 bytes (`:392`, `utf8_prefix` `:413`) and redacted by `sanitize_structured_output_rejection` (`:485`) with upstream's regexes, marker and fallbacks. The missing-call error is kept for a child that never invoked the tool, and either error is reported only on an otherwise-clean run. `validate_structured_output` (`:73`) now returns the bare message, which removes the doubled prefix from the child tool and from dynamic collection. The verifier also corrected the `AgentProgress::all_events` doc (`exec/progress.rs:62-66`), which still said structured output never reads it. Upstream `structured-output.ts:11-80,388-405`, `execution.ts:1036-1038,1449-1471`, `subagent-runner.ts:1245-1270` @v0.71.0. Verify: `cyrup-ext-subagents tests::structured_output_results_integration::{a_rejected_structured_output_call_fails_with_the_rejection_summary,a_valid_structured_value_survives_a_later_failure,a_child_that_never_invokes_structured_output_gets_the_missing_call_error}` plus the `exec::structured` unit tests (bare message, capitalised read-back); red without the fix. Two shape differences, neither a gap in this row: an interrupted run is not read (as upstream's foreground, `execution.ts:1421-1435`; upstream's runner still reads an invoked capture on an interrupted run, `subagent-runner.ts:1252`), and the parent reads the capture file where upstream's foreground uses an in-process capture. — *Original:* Structured output is read only on a clean exit: a rejected call is reported as "Missing structured_output call", and a valid value is discarded when a later provider error fails the run |
| ~~SUBA-128~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): ported end to end. Tool parameter `checkpointBeforeDeadlineMs` with upstream's bounds and description (`crates/cyrup-ext-subagents/src/extension/tool/schema.rs:553`, `params.rs:227`). Config key carried raw and validated with upstream's sentence (`registration/mod.rs:551,877-886`, checked first in `validate_raw_config`, `:840`), and dropped from `UNPORTED_CONFIG_KEYS` (12 → 11, `:559`). `params ?? config` is resolved on the async single launch only (`extension/executor/background.rs:389-393`, upstream `executeAsyncSingle`). The detached runner arms `remainingMs - checkpoint` when that is at least 1 s and queues upstream's verbatim `source: "deadline-checkpoint"` steer, unless the run already timed out, was stopped or interrupted, or has no running step (`background/runner_main/control_watcher.rs:593,630`, armed from `entry.rs:186`). The verifier moved the bounds refusal (0 or above 2147483647) from the async single path to the tool entry, ahead of every mode (`extension/tool/mod.rs:254-264`), because pi's argument validation refuses an out-of-range value for every call shape. Upstream `schemas.ts:363`, `config.ts:145-151,215-230`, `subagent-executor.ts:3460`, `async-execution.ts:2104`, `subagent-runner.ts:210,3379-3406` @v0.71.0. Verify: `cyrup-ext-subagents tests::deadline_checkpoint_runner_integration::a_running_child_receives_the_deadline_checkpoint_and_hands_off_before_the_kill`, `background::runner_main::control_watcher::tests::{the_deadline_checkpoint_reaches_the_running_child_before_the_deadline,an_ended_or_idle_run_gets_no_deadline_checkpoint,the_checkpoint_delay_is_the_remaining_time_less_the_checkpoint_and_at_least_a_second}`, `extension::executor::background::tests::checkpoint_before_deadline_launch::the_call_wins_the_config_default_fills_in_and_a_bad_value_is_refused` (extended with foreground single and chain refusals), `registration::tests::checkpoint_before_deadline_ms_is_read_and_validated`; red without the fix. Outside this row: an invalid config warns and falls back to defaults (`crates/cyrup/src/subagent_config.rs:68-70`, the warning names the key) where upstream `loadConfig` throws, and the out-of-range tool message is cyrup's own sentence, not TypeBox's. — *Original:* `checkpointBeforeDeadlineMs` (async single tool param and `config.json` default) is unported, so a deadline kills a child with no handoff steer |
| SUBA-129 | low | upstream-drift | M | Typed gates (`gate: { command, output: "json", schema?, timeoutMs? }`, `verify[].output`/`schema`) and `acceptance.preserveStagedIndex` are refused |
| ~~SUBA-130~~ | ~~low~~ **CLOSED 2026-09-30** | upstream-drift | M | **CLOSED 2026-09-30** (on `claude/lows-batch4`): (1) New shared runner `spawn/bounded_argv.rs::run_bounded_argv` (`:130`), the Rust port of `worktree-setup-command.ts::runSetupCommand`: `process_group(0)`, a 1 MiB combined output cap whose overflow kills the program and FAILS the call (`ENOBUFS`; never a truncated result), stop token (`ABORT_ERR`) and absolute deadline (`ETIMEDOUT`) both kill through `signal::terminate_on_timeout` plus a `SIGKILL` of the whole group, and an already-expired bound never spawns. (2) `spawn/worktree.rs`: `GitBounds` (`:265`, stop + deadline + byte budget) rides `CreateWorktreesOptions::bounds` and `WorktreeGroupConfig::bounds`; `run_git_bounded` / `run_git_env_bounded` (`:396,:412`) replace the unbounded `Command::output()` at every call in create, preflight-shaped probe (`resolve_repo_state`), hook validation, diff, patch validation, cleanup and the cleanup gate's probes; `run_git` stays the unbounded default ONLY for the operator-invoked cleanup-plan builder (`cleanup_plan/git.rs`, `classify.rs`) and `worktree.discard` (`handoff/write.rs`), each marked at the site as having no run context. `chain_graph::assign_worktree_cwds` passes `ctx.cancel` + `ctx.deadline_at` (pi `worktree.ts:239-252`). (3) The setup hook now runs through the same runner (`run_worktree_setup_hook`, `:928`), with upstream's `Math.min(deadlineAt, now + hookTimeoutMs)` clamp, so it obeys stop and the run deadline as well as its own timeout (its own timeout keeps its `timed out after Nms` text). (4) Rollback (`GitBounds::for_compensation`, `:322`) keeps only the original deadline and not the launch signal, per `worktree.ts:1305-1325`. **[CYRUP-DELTA, two]** (a) diff capture and cleanup are bounded too (`capture_worktree_diff`, `cleanup_worktrees`, `GitBounds::for_harvest` `:343`, wired in `chain_graph::publish_worktree_handoff`): upstream's `runGit`/`runGitChecked` (`:277-296`) are unbounded `spawnSync`, and a hung git that holds a finished run open past its deadline or ignores stop is strictly worse. Because a run that ended by stop or timeout is exactly the one whose work most needs capturing, the harvest deadline gets a 10 s floor and an already-cancelled token is not obeyed (a stop arriving DURING the harvest still ends a hung git). Patch capture has a 256 MiB budget (`PATCH_CAPTURE_MAX_BYTES`) and an overflow fails the capture (worktree preserved) instead of upstream's silent 1 MiB truncation. (b) `for_compensation` floors the rollback deadline at 10 s from now: upstream hands compensation an already-expired deadline verbatim and reports the leftovers through `writeWorktreeSetupHandoff`, which cyrup does not have (the rollback report is discarded), so an exact port leaks a worktree and a branch every time the deadline fires mid-setup. Verify (all in `cyrup-ext-subagents`, no cyrup-it, no PATH faking; a real hung git is made with a `core.fsmonitor` hook that sleeps): `spawn::bounded_argv::tests::{a_hung_program_is_killed_at_the_deadline, the_stop_token_ends_a_hung_program_promptly, output_past_the_cap_fails_and_kills, an_exhausted_bound_never_spawns, a_descendant_dies_with_its_leader}` and `spawn::worktree::tests::{create_worktrees_with_an_expired_deadline_creates_nothing, a_stop_during_the_setup_hook_aborts_creation_and_rolls_back, the_run_deadline_clamps_the_hook_timeout_and_rollback_still_cleans_up, a_hung_git_in_the_diff_capture_is_ended_at_the_deadline, a_hung_git_in_cleanup_is_ended_by_stop_and_the_worktree_is_preserved, a_harvest_after_stop_and_deadline_still_captures_the_work}`; with the bounds disabled 9 fail (hung programs ran to `sleep 30`: `stop must be prompt: 30.03s`; expired-deadline create returned `Ok(WorktreeSetup{..})`; diff capture `the capture must fail at the deadline: None`; `for_harvest` obeying the run's stop: `an already-ended run's stop is not obeyed`), and with only the compensation floor removed the clamp test fails `only the main checkout may remain` (leaked worktree). *Not closed, filed as `SUBA-147`/`SUBA-148`:* the `preflightWorktreeSource` admission pass and the operator-action/turn-lock bounds. *Earlier text:* Worktree git commands run with no deadline, stop signal or output bound, so a hung `git` outlives the run's deadline and ignores `stop` |
| ~~SUBA-131~~ | ~~low~~ **CLOSED 2026-10-10** | upstream-drift | M | **CLOSED 2026-10-10** (checked against pi-subagents ad11b7ab; the mechanism is byte-identical from v0.71.0 to the pin and untouched at clone HEAD `bc078467`): a local external-CLI step now has a `ControlMonitor` and raises `needs_attention` like a native child. New `exec/external_cli/activity.rs` (`ExternalActivityTracker`) ports `prepareExternalActivity` (`subagent-runner.ts:2628-2634`; `prepare` `:204`: no git with control off, else the baseline `readGitFingerprint` before launch/preflight/spawn), `readGitFingerprint` + `expectedMissingGitEvidence` (`:627-657`; `read_git_fingerprint` `:120`, `rev-parse --verify HEAD` then `status --porcelain=v1 -z --untracked-files=normal` through `spawn::bounded_argv::run_bounded_argv`, 16 MiB, 30s, cancellable, raw bytes instead of base64), `onExternalOutput`/`recordExternalStreamActivity` (`:925-928`, `:2635-2639`; `on_chunk` `:242` stamps without re-deriving via the new `ControlMonitor::touch_activity`, `exec/control.rs:2178`) and the idle-boundary gate of `updateRunnerActivityState` (`:3178-3205`; `on_tick` `:252` / `on_probe_result` `:275`: already past the window with a baseline, wait on an in-flight probe or start one at most every `EXTERNAL_GIT_PROBE_MIN_INTERVAL_MS = 2_000`, a changed fingerprint is activity, an unchanged one raises). `exec/external_cli/run.rs::run_external_cli_process_tracked` (`:192`) feeds every chunk of both streams (`:415`) and adds a 1s tick arm (`:394`) and a probe-completion arm (`:402`), both below the verbs, above the chunk arm and gated `!reaped`; an in-flight probe is cancelled through a child token and awaited after the loop (`:454`). `run_external_cli` (`exec/external_cli/mod.rs:356`) builds the monitor as `exec::attempt_runner` does, ports the `:872-876` early return (a stop or timeout already in force never spawns, `:385`, and, like upstream, its result carries no `runner` descriptor) and returns the monitor's events in `control_events` (`:602`, was `Vec::new()`). `ControlMonitor::idle_due` (`control.rs:2190`) documents why cyrup needs no forced `turnCount: 1` (`:3173`): `derive_activity_state` has no turn guard. **Ledger corrections:** the probe runs once the step is already past the idle window, not "near" it; the stale cites are now `drive_attempt.rs:1076-1087` (tick), `run.rs:179-515`, upstream `:3173`, `:925-928`, `:627-657`, `:3178-3220`. **Deviations [CYRUP-DELTA]:** `PROCESS_TREE_UNVERIFIED` is not ported. Upstream `runSetupCommand` raises it (`worktree-setup-command.ts:106-131`) when termination begins after git already exited: git ended by a signal, or git exited while a descendant still holds stdout/stderr when the 30s deadline or an abort fires; `readGitFingerprint` rethrows it (`subagent-runner.ts:653`) and a periodic probe fails the step with `PROCESS_TREE_UNVERIFIED: <msg>` (`:2609-2627`). `run_bounded_argv` reaches the same conditions but reports a `None` status, `DeadlineExceeded` or `Aborted` (its group SIGKILL is fire-and-forget and unverified, so a `setsid` descendant escapes it); cyrup warns once (or stays silent on an abort), treats the read as no fingerprint and, for a periodic probe, raises `needs_attention` instead of failing the step (module doc, `activity.rs:23-37`); probes are single-flight per step, not coalesced per cwd across a run (`gitProbesByCwd`); the once-per-cwd warning goes to `tracing` without the `[pi-subagents]` prefix; the raised event also carries `turns`/`tokens`/`tool_count` (`Some(0)`) from the shared `emit_needs_attention`. **Split out:** placed (Herdr machine) external steps still get no idle rule, filed as `SUBA-212`. **Leads, not filed here:** the background runner writes no live `activityState` to `status.json` for any child (native or external; pre-existing); `derive_activity_state` lacks upstream's `turnCount === 0` guard and `scaledNeedsAttentionAfterMs` thinking scaling (native path). Verify: `exec::external_cli::tests::run_sync_flags_a_silent_external_cli_needs_attention`, `::run_sync_does_not_flag_an_external_cli_that_writes_stderr_periodically` (stderr and stdout), `::run_sync_credits_new_git_worktree_changes_as_external_activity`, `::run_sync_does_not_credit_unchanged_preexisting_git_dirtiness`, `::run_sync_raises_nothing_for_a_silent_external_cli_with_control_disabled`, `::run_sync_never_spawns_an_external_cli_stopped_before_launch`, `exec::external_cli::activity::tests::*` (9: strict idle window and chunk stamping, probe interval, in-flight wait, changed-only credit, disabled control, warn-once, real-git fingerprint, silent/unexpected git failures, cancelled baseline); each red-proved by mutation (the missing-evidence test by making `expected_missing_git_evidence` always false, the cancelled-baseline test by reading the baseline under a fresh `CancelToken`); the no-`runner` assertion on the stopped-before-launch test was red against the old `external_failure` early return. — *Original:* An external-CLI step is never flagged `needs_attention` when idle; upstream now tracks its stdout/stderr and Git fingerprint as activity |
| ~~SUBA-132~~ | ~~low~~ **CLOSED 2026-09-28** | not-ported | S | **CLOSED 2026-09-28** (on `claude/lows-next`): `is_tool_budget_blocked_message` is ported as a whole-message match against this run's own hard limit and tool name (`crates/cyrup-ext-subagents/src/exec/tool_budget.rs:282`). `run_sync` sets `SingleResult::tool_budget_blocked` (`exec/run_result.rs:277`) from the winning attempt's `tool_execution_end` events, by each event's own tool name (`exec/mod.rs:799-810`). The flag crosses the runner's `StepResult` waist (`spawn/chain_graph.rs:1311`, `background/runner_main/executor.rs:1301`, `settle.rs:627`), and `WorkflowBudgetSignals::from_single_result` reads it (`workflows/settlement.rs:452,462`). The verifier also moved a misplaced doc comment in `executor.rs` back onto its SUBA-3c test. Upstream `tool-budget.ts:74-94`, `execution.ts:1144-1156`, `subagent-runner.ts:1077,1325-1329,1544`, `workflow-settlement.ts:169-174` @v0.71.0. Verify: `cyrup-ext-subagents tests::structured_output_results_integration::{a_tool_budget_block_reaches_the_result_and_settles_budget_exhausted,the_blocked_message_counts_only_against_the_childs_own_budget}`, `background::runner_main::executor::tests::tool_budget_blocked_survives_the_step_result_waist_and_settles_budget_exhausted`, `exec::tool_budget::tests::only_this_runs_own_whole_blocked_message_is_a_budget_block`; red without the fix. Neighbouring gaps outside this row: upstream's runner detects the block more loosely (`includes("Tool budget hard limit reached")` over every toolResult, `subagent-runner.ts:1325-1329`) where cyrup's shared `run_sync` applies the strict v0.70.0 (#2302) foreground matcher on both paths; the `toolBudget` state and the status-step `toolBudgetBlocked` are not carried, so `workflows/checklist.rs` still reads None; `AbortRecoveryInput::tool_budget_exhausted` has no production caller. — *Original:* `SingleResult` carries no `toolBudgetBlocked`, so a tool-budget-blocked workflow child never settles `budget_exhausted` (self-declared in `workflows/settlement.rs`) |
| ~~SUBA-133~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): `advertise` is a typed frontmatter field, strictly `true`/`false`; any other value skips the file with upstream's message (`crates/cyrup-ext-subagents/src/discovery/frontmatter.rs:1362-1384`). Management create/update can set it (`""` clears it; `discovery/management/config_parse.rs:108-114`, `agent_crud.rs:52-54,262,352`), and it is written back as upstream writes it (`frontmatter_write.rs:60`). New `discovery/advertised.rs` ports `advertised-agent-prompt.ts` (16 agents, 12 288 bytes, 512-byte descriptions, XML escaping, the omitted-count line, strip-then-append; `:17-31,95,157`). `extension/executor/advertised.rs` holds the session state and refresh; `extension/host/native_impl.rs:465-467` starts it at session start, and `:810-838` appends the block at `before_agent_start` while `subagent` is in `selectedTools` (active tools as the fallback) and strips a stale block otherwise. Management mutations refresh it (`extension/tool/routing.rs:2210-2212`). Runtime, disabled and ceiling-denied agents are left out. Verify: `tests::rpc_bridge_integration::advertised_agents_ride_the_parent_system_prompt`, `discovery::advertised::tests::*` (4), `discovery::frontmatter::tests::advertise_is_a_strict_boolean_frontmatter_field`. Remaining delta, larger than the in-source `[CYRUP-DELTA]` note says: `locale_order` (`discovery/advertised.rs:82-111`) lowercases and compares bytes, which is not ICU root collation (`_` sorts before `-`, `.`, digits and letters under `localeCompare`, after digits in ASCII). So `code_review`/`code-review`/`code1` sort differently, and with more than 16 advertised agents the 16 that make the cut can differ. — Agent `advertise: true` and the `<advertised_subagents>` catalog in the parent's system prompt are unported |
| ~~SUBA-134~~ | ~~low~~ **CLOSED 2026-09-30** | upstream-drift | S | **CLOSED 2026-09-30** (on `claude/lows-batch3`): the nested-run display name and run-status's external-runner block landed. **Nested exact-status view:** `NestedRunSummary`/`NestedStepSummary` carry the typed `session_name` (`spawn/nested_events.rs:206,283`; pi `shared/types.ts:1606,1659`), kept by the sanitizer every relayed event passes through (`nested_events.rs:499,597`; `nested-events.ts:311,352`); upstream's start event carries none, and cyrup's matches (`extension/executor/background.rs:1096`). `format_nested_exact_status` ports `formatNestedExactStatus` (`background/run_status.rs:734`; `run-status.ts:322-345`) with `nested_run_display_name` (`run_status.rs:711`; `:171-176`: session name, else agent, else agents joined, else id) and the step names `step.sessionName?.trim() || step.agent`; the descendant tree under it is a new port of `nested-render.ts` (`spawn/nested_render.rs:81,115,267,386` — `formatNestedAggregate`, `nestedRunLabel` with its session-name rung, `formatNestedRunStatusLines`). `status` with an id that is no exact async run resolves an exact nested descendant across the projected registries and renders the view, or upstream's `Nested run id '…' is ambiguous across authorized registries.` error (`extension/executor/status.rs:611`, `nested_control.rs:365`, `run_status.rs:991`; `run-status.ts:435-448`, `run-id-resolver.ts:152-161`). **External-runner block** (`run-status.ts:641-664`, unported as a whole and outside SUBA-141's Fleet scope): under each external-CLI step line, after the timeout-recovery lines, `format_external_cli_runner_lines` renders `Runner: external-cli (<command> <args>)`, `Adapter: <id> v<version> (<executionMode>)`, the per-family `Safety:` line (Codex/Cursor/Claude Code/legacy fallback), `Capabilities:`, the three `Unsupported …:` reasons and `Context handoff: fresh only (…)` off the re-normalized descriptor (`normalizeExternalCliRunnerStatus`), `Runner: external-cli (invalid persisted runner metadata)` when it does not normalize, then `Process:` (only with a pid), `Stdout:`, `Stderr:` and `Final output:` off SUBA-141's live `StepStatus.external_process` (`run_status.rs:501,575,635`). Verify: `background::run_status::tests::{an_external_cli_step_renders_the_runner_block_and_its_live_process, each_adapter_family_renders_its_own_safety_line_and_a_pidless_receipt_its_logs, an_unnormalizable_runner_is_invalid_metadata_and_a_native_step_has_no_block, the_nested_exact_status_names_the_run_its_steps_and_descendants_by_session_name, the_nested_display_name_falls_back_agent_then_agents_then_id}`, `extension::executor::status::tests::status_by_id_renders_a_nested_run_by_its_child_session_name` — the block, sanitizer, display-name and dispatch fixes each shown red when reverted. **Not in this row (recorded, not claimed):** cyrup's runner emits no `subagent.nested.updated`/`completed` events (`nestedSummaryFromAsyncStatus`, `nested-events.ts:1002-1050`, from `subagent-runner.ts:2128`, `async-execution.ts:794`, `stale-run-reconciler.ts:344`) and mints no root route (SUBA-115's reach note), so a cyrup-produced summary carries `session_name` only once that relay is ported; the nested lookup lists every route under `Roots::nested_events` (as `resolves_to_nested_run` already did) rather than upstream's state-scoped routes, and skips the nested prefix rung, `reconcileNestedAsyncDescendants` and the nested transcript view; the nested summary has no `turnBudget`/`model`/`thinking`, so those segments stay empty; run-status's per-step nested tree, the external runner's `Steer: unavailable; external runners do not accept live messages.` line and the `external-job` block (no cyrup external-job runner) are unported. *Earlier text:* **PARTIALLY CLOSED 2026-09-30** (on `claude/lows-batch3`): every remaining item but the nested-run display name landed. **Typed synchronous intercom claim** (replaces the JSON reply topic the project owner rejected): cyrup-ext gains a typed native bus — `InitApi::subscribe_typed_bus` (`crates/cyrup-ext/src/native.rs:622`), `NativeExtension::on_typed_bus_event` (`native.rs:709`), `SharedBus::subscribe_typed`/`emit_typed` (`bus.rs:100,120`, listeners run inline before the emit returns, held `Weak`), registered at load (`facade.rs:674`) and reached by natives through `HostServices::emit_typed_event` (`host/services.rs:578`; live impl `cyrup-session-svc/src/host_services.rs:1829`). The request is the typed `IntercomSessionIdentityRequestV1 { claim(&str), claimed() }` (`cyrup-ext-subagents/src/tui/intercom.rs:815-851`, re-exported from `cyrup-intercom/src/identity.rs:66`); the child runtime subscribes (`prompt_runtime.rs:2399`) and claims its routing name inline (`prompt_runtime.rs:2520`); intercom emits it at `session_start` and reads `claimed()` right after the emit, before any connect is scheduled (`cyrup-intercom/src/extension.rs:905-913`), so the first registration is already under the claim (`connect.rs:556`) — the reply topic, claim window and late-claim re-register are deleted. **Label arm:** `SingleStepSpec` carries `label`/`session_name` (`spawn/chain_graph.rs:205-213`) and `child_session_name` is pi's `step.sessionName ?? deriveChildSessionName({agent, task, label})` (`:221`); labels are threaded from chain files (`discovery/chains.rs:1248`), tool `tasks[]`/`chain[]` items (`extension/tool/task_items.rs:336`), slash `label=` (`registration/slash_commands.rs:1300`) and chain-append (`extension/executor/control.rs:770`); dynamic fan-out members are named from their own item's label or task at expansion (`chain_graph.rs:2180-2210`, `subagent-runner.ts:3561-3567`); a workflow row whose child never launched gets pi's label placeholder from the key's first `run` trace entry (`workflows/child_summary.rs:711`, wired at `extension/executor/workflow_launch.rs:634`; `subagent-executor.ts:5774`). **Pending statuses:** single/parallel entries carry the label-aware name and `label`, an attached root is `<agent>: Attached <runId>` (`background/flat_index.rs:118`, `StepStatus.label` in `records.rs`); settled names land on status entries and results (`runner_main/status.rs:520,574`, `settle.rs:630`, reconciler `reconcile.rs:896`). A dynamic group keeps upstream's unnamed `expand:` placeholder; its members have no status entries in cyrup (the recorded SUBA-093 splice residual) but each member's child and result are named. **Launcher-assigned branch now live:** the async runner puts the step's name into `RunOptions::child_env` (`runner_main/executor.rs:885`), which `resolve_child_session_name` reads for the child env and the result. **Displays:** `StepStatus::display_name`/`child_display_name` (`records.rs:241,254`) drive Fleet step rows, the transcript child hint and step line (`fleet_view.rs:529,690,711`), run-status and run-list step lines (`run_status.rs:480,1003`); the Fleet foreground row prefers the live child's session name (`fleet_view.rs:412`, registered at `extension/executor/foreground.rs:1476`), and a remembered foreground child keeps and shows it (`foreground_history/record.rs:257`, `extension/executor/status.rs:1045`). Verify: cyrup-ext `tests::native_dispatch::a_typed_bus_emit_runs_every_listener_before_it_returns`; cyrup-it `session_identity_claim::{a_claiming_extension_sets_the_intercom_id_over_the_stable_id, the_first_non_blank_claim_wins_and_does_not_outlive_its_runtime}`; subagents `tests::child_prompt_runtime_integration::the_child_names_its_session_and_claims_its_intercom_route`, `background::flat_index::tests::pending_entries_are_named_from_their_label_and_an_attached_root_from_its_attachment`, `background::runner_main::executor::tests::a_runner_step_names_its_child_from_its_label_and_the_foreground_walk_does_not`, `background::runner_main::status::tests::{a_settled_childs_session_name_lands_on_its_status_entry, a_step_result_carries_the_name_its_child_ran_under_else_its_declared_one}`, `spawn::chain_graph::tests::dynamic_group_names_each_member_from_its_own_item`, `extension::executor::control::tests::an_appended_step_is_named_from_its_label`, `extension::tool::routing::tests::a_workflow_child_that_never_launched_is_named_from_its_label`, `workflows::child_summary::tests::step_rows_are_named_by_their_child_session_else_by_the_trace_label`, `background::fleet_view::tests::{fleet_step_lines_show_the_child_session_name_over_the_agent, a_foreground_row_shows_the_live_childs_session_name_over_its_agent}`, `background::run_status::tests::active_run_step_lines_show_the_child_session_name_over_the_agent`, `extension::executor::foreground::tests::register_foreground_controls_derives_the_entry_and_stamps_workflow_identity`, `extension::executor::status::tests::status_by_id_finds_a_remembered_foreground_run_that_is_no_longer_live`, `background::reconcile::tests::genuinely_dead_pid_reconciles_to_failed_and_writes_both_files` — each red with its fix reverted. **CYRUP-DELTA:** a typed-bus listener downcasts to the request type instead of duck-typing `typeof request.claim === "function"` (`native.rs:709`, `tui/intercom.rs:815`). **Remaining:** `nestedRunDisplayName` (`run-status.ts:171-175`) — cyrup has no nested exact-status view (`formatNestedExactStatus`, `run-status.ts:322-345`) and its `NestedRunSummary` (`spawn/nested_events.rs:236`) has no `sessionName`, so there is no surface to name; it lands with that view's port. *Earlier text:* Child sessions get no derived human-readable name (`deriveChildSessionName` → `setSessionName`, `sessionName` in results, the `intercom:session-identity` claim) — **2026-09-27:** the intercom half is in (`ICOM-064`, on `claude/intercom-lows`): answer the `intercom:session-identity` request by emitting `intercom:session-identity-claim` `{version:1, stableId:<routing name>}` before the first `agent_start` — **PARTIALLY CLOSED 2026-09-28** (on `claude/lows-next`): the base naming landed. `crates/cyrup-ext-subagents/src/exec/child_session_name.rs:35` ports `deriveChildSessionName` (60/80 bounds, `[prompt redacted]` skipped). `exec/attempt_runner.rs:541-555` passes the name to the child as `CYRUP_SUBAGENT_SESSION_NAME`. The child's `prompt_runtime.rs:2719-2733` calls `set_session_name` at `before_agent_start`, using the readable name once intercom has claimed the routing name and the routing name otherwise (`subagent-prompt-runtime.ts:532-550`). The name is carried on `SingleResult.sessionName` (`exec/mod.rs:343`, `background/runner_main/settle.rs:607`, `extension/executor/foreground.rs:1712`) and on pending status steps (`background/flat_index.rs:107`). Verify: `tests::structured_output_results_integration::the_derived_child_session_name_reaches_the_child_env_and_the_result`, `tests::child_prompt_runtime_integration::the_child_names_its_session_and_claims_its_intercom_route`, `exec::child_session_name::tests::*` (2). **Remaining:** (1) the label arm. Every caller passes `label: None`, so workflow children (`subagent-executor.ts:5774`), labelled chain steps (`subagent-runner.ts:828,1913,1967`) and dynamic parallel tasks (`:3562`) get the task excerpt instead of the label. (2) DynamicGroup and ImportAsyncRoot pending statuses carry no `sessionName`, and chain-append does not name appended steps. (3) `resolve_child_session_name`'s launcher-assigned branch (`child_session_name.rs:66-79`) reads `RunOptions::child_env`, which has no production caller, so the branch runs only in tests. (4) Fleet and run-status displays do not yet prefer `session_name` (`fleet-view.ts:79-84`, `run-status.ts:167-172`). **Ledger correction:** the lane's "nothing is missing within the row" is wrong, because this row names "agent name + task or workflow node label" and the label arm is unported. |
| ~~SUBA-135~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): `preflightLaunchCwd` is ported verbatim (`crates/cyrup-ext-subagents/src/exec/launch_cwd.rs:19`: does not exist / is not a directory / could not be accessed, plus `(resolved from "<typed>")`). It runs in three places: at the tool entry for a typed `cwd`, ahead of the mission binding (`extension/tool/mod.rs:446-464`); at the head of `run_sync` (`exec/mod.rs:357-366`), which covers the foreground and every runner step; and in `spawn_background_steps`, before any async root, run directory or runner exists (`extension/executor/background.rs:515-521`). Upstream `launch-cwd.ts:3-16`, `execution.ts:1604`, `async-execution.ts:648-650`, `subagent-runner.ts:1061` @v0.71.0. Verify: `cyrup-ext-subagents extension::executor::background::tests::launch_cwd_preflight::{a_missing_single_launch_cwd_is_refused_before_launch_with_its_typed_spelling,a_non_directory_cwd_is_refused_on_the_chain_paths_too}`, `extension::executor::background::tests::an_async_launch_into_a_missing_cwd_is_refused_before_any_run_exists`, `tests::structured_output_results_integration::a_run_into_a_missing_cwd_is_refused_by_name_before_spawning`, the `exec::launch_cwd` unit tests; red without the fix. **Ledger correction:** the impact was understated. cyrup's mission store sits under the project root (`<projectRoot>/.cyrup-subagents/missions`, CYRUP-DELTA), so before the fix the mission binding created the typo'd cwd and the child then ran silently in that new empty directory. Shape differences, all forced by that store location: a typed top-level `cwd` on a chain or parallel call refuses the whole call at the tool entry, even when every task names its own valid cwd (upstream fails each task); no mission record is written for the refused launch (upstream binds first, `subagent-executor.ts:5230`, and records a failed mission); the access-failure arm appends Rust's `io::Error` text rather than Node's. — *Original:* No launch-cwd preflight: a missing or non-directory `cwd` fails at spawn with an OS error instead of upstream's named refusal before launch |
| ~~SUBA-136~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): a surfaced request's details now carry upstream's fields: id, requestId, reason, expectsReply, runId, agent, childIndex, requestBody, replyHint, and childTarget or interview when present (`crates/cyrup-ext-subagents/src/native_supervisor.rs:1233-1234`; `native-supervisor-channel.ts:757-767`). Each reply appends one `subagent_supervisor_reply` custom entry, and a journal failure is logged, not fatal (`:1117-1140`). New `tui/supervisor_ui.rs` ports `supervisor-ui.ts`: the shape checks, the 512/8 000/4 000 UTF-16 bounds with ` [truncated]`, `safeTerminalText`, the headings, a 36-row collapsed cap with a marker row, and heading-only output below 3 columns (`:26-41,380,394`). `extension/host/native_impl.rs:213-221` registers both renderers through the host's live-component tier. Verify: `tests::rpc_bridge_integration::the_supervisor_request_and_reply_cards_are_registered_with_the_host`, `native_supervisor::tests::a_surfaced_request_carries_its_details_and_a_reply_is_journalled`, `tui::supervisor_ui::tests::*` (4). Outside this row and not yet filed: v0.71.0 does not inject no-reply requests (`progress_update`, `native-supervisor-channel.ts:740-744`) and cyrup still does; cyrup emits no `INTERCOM_DETACH_REQUEST_EVENT` per surfaced ask. — No renderer for `subagent_supervisor_request` messages and no `subagent_supervisor_reply` session entry |
| ~~SUBA-137~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): `available()` requires macOS, `TERM_PROGRAM` equal to `ghostty` ignoring case, and a trimmed `__CFBundleIdentifier` exactly `com.mitchellh.ghostty` (`crates/cyrup-ext-subagents/src/inspectors/ghostty/plugin.rs:111-116,134-142`; `ghostty/plugin.ts:20-24` @v0.71.0). The env map is the real process env (`inspectors::actions::process_env`), so the bundle id reaches the check in production, and a cmux-style embedder exporting `TERM_PROGRAM=ghostty` is refused. Verify: `inspectors::ghostty::plugin::tests::available_requires_the_standalone_ghostty_bundle_id`. Cosmetic: `plugin.rs:236` still cites `ghostty/plugin.ts:13, both conjuncts`; the case fold is ASCII-only and `trim` does not strip U+FEFF, which no real value reaches. — The Ghostty inspector activates on `TERM_PROGRAM=ghostty` alone; v0.69.0 also requires the macOS host bundle id, so cmux-style embedders stop misfiring |
| ~~SUBA-138~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): `/subagent-cost` and the new RPC `cost` method share one port of v0.71.0 `collectSubagentCost` (`crates/cyrup-ext-subagents/src/registration/cost.rs:995`, camelCase `SubagentCostReport` `:732`, `format_subagent_cost_report` `:1243`; called from `extension/executor/reports.rs:172-224`). It counts parent assistant turns and compaction usage; `subagent`/`bg_wait` tool-result details and slash-result messages, deduplicated by `run:`/`session:` identity; and workflow children resolved through the run-dir status, the receipt and artifact `_meta.json` (2 MiB bound, run-id charset check), plus `unresolvedAsyncChildren`. `SUBAGENT_RPC_METHODS` is nine, in upstream's order (`extension/rpc/mod.rs:110-120`, dispatch `:322`); `cost` refuses non-object params, `null` included, with upstream's text; `ping.capabilities.cost = {version: 1}` (`extension/rpc/ping.rs:122`). Verify: `tests::rpc_bridge_integration::{rpc_cost_returns_the_versioned_report_over_the_live_branch,the_bridge_announces_ready_on_session_start}`, `registration::cost::tests::{cost_report_counts_compactions_bg_wait_completions_and_dedupes_by_run_id,cost_report_resolves_workflow_children_through_receipt_and_metadata}`, `extension::executor::reports::tests::run_cost_report_walks_the_session_transcript`. **Ledger correction:** this row said `/subagent-cost` already computed the data. At HEAD it was an older transcript walk (`build_subagent_cost_report`, citing `slash-commands.ts:377-416`), not v0.71.0's collector. Remaining in the same file: the cyrup-only R-SA-140 dual-recursion accumulator (`CostUsage`, `RunMetadata`, `accumulate_meta_tree`, `compute_recursive_cost`, `build_cost_report`, `format_cost_report`, …; `cost.rs:90-600,1282-1310`) has no production caller and no upstream counterpart, and the module doc (`:1-58`) still describes it as `/subagent-cost`. It should be deleted, keeping `find_latest_session_file_by_mtime` (`:611`). — The in-process RPC `cost` method and `ping.capabilities.cost` are unported |
| ~~SUBA-139~~ | ~~low~~ **CLOSED 2026-10-09** | upstream-drift | M | **CLOSED 2026-10-09** (with `SUBA-153`): the `subagents_enable` loader is ported from `src/extension/tool-activation.ts` @v0.76.1 (unchanged since v0.74.0) as `crates/cyrup-ext-subagents/src/extension/tool_activation.rs`. It is registered in the Full arm beside `subagent` (`native_impl.rs`, pi `extension/index.ts:1193-1199`), model-only, with upstream's label, description, snippet, `Cannot enable unavailable tools: subagent.` and success text verbatim apart from "Start cyrup with --exclude-tools subagents_enable" (product name, as `bg_wait` says `cyrup -p`). `session_start` and the newly subscribed `session_tree` run `applyRecordedSelection` through `HostServices::set_active_tools`; `before_agent_start` keeps a selected loader in `selectedTools`, and that edit and the SUBA-133 catalog rewrite now leave in one `Mutate` instead of the catalog arm's early return dropping the edit. **Prerequisite corrected:** the row's "needs `MCP-037a`" was wrong. Nothing registers late: both tools register at `init`, and activation toggles the active set, which the session drains at every turn boundary (`AgentSession::next_turn_tools`) and before the first request. **CYRUP-DELTA:** the loader is `default_active() == false`, so a host with no live dynamic-tool view (`active_tools() == None`) never selects it and keeps today's eager `subagent`, logging one warning per process (upstream's unsupported-host fallback, `:88-94`). Verify: `subagents::tool_activation_integration::dynamic_offers_the_loader_first_and_subagent_after_it_is_called` (cyrup-it; the first request has the loader and not `subagent`, and after the loader runs the next prompt's request has `subagent` and the call dispatches. Across two prompts because the test harness's session is never `into_shared`, so its in-run turn-boundary drain is inert; a bound session drains inside the run), `extension::tool_activation::tests::the_loader_enables_subagent_keeps_other_tools_and_is_idempotent`, `::on_event_keeps_the_loader_edit_and_the_catalog_rewrite_in_one_mutate`, `::a_host_without_a_dynamic_tool_view_keeps_subagent_and_never_selects_the_loader`. — The `subagents_enable` lazy loader is unported; the full `subagent` tool is always in the prompt (upstream's unsupported-host fallback) |
| ~~SUBA-140~~ | ~~low~~ **CLOSED 2026-09-28** | upstream-drift | S | **CLOSED 2026-09-28** (on `claude/lows-next`): when the parent env (read through the extension's env seam) has a non-empty `CYRUP_SUBAGENT_CACHE_RETENTION`, the child's spawn plan sets `CYRUP_CACHE_RETENTION` to it. Unset or empty writes nothing, so the child keeps inheriting the parent's retention (`crates/cyrup-ext-subagents/src/exec/spawn_plan.rs:141-151,594-603`; `child-cache-retention.ts:14-25` @v0.71.0, whose `\|\|` treats empty as unset). Every cyrup child is a spawned process, so the env form covers both of upstream's arms. Verify: `exec::spawn_plan::tests::the_child_only_cache_retention_reaches_the_child_env`. — `PI_SUBAGENT_CACHE_RETENTION` (child-only prompt-cache tier) is unported |
| ~~SUBA-141~~ | ~~low~~ **CLOSED 2026-09-30** | upstream-drift | S | **CLOSED 2026-09-30** (on `claude/lows-batch3`): the Fleet half landed. `StepStatus.external_process` is the typed `externalProcess` (`background/records.rs:179`). The external CLI runner reports its process through a typed live hook — once at spawn (pid, start, log paths) and again at close (`exec/external_cli/run.rs:81,250,497`; pi `onProcess`, `external-cli-runner.ts:340-348,386-399`) — which `run_external_cli` forwards with the launch's runner descriptor as a typed `ExternalProcessUpdate` on the live sink (`exec/external_cli/mod.rs:326`, `exec/agent_config.rs:941,1004`). The async runner routes it through its telemetry channel (`TelemetryMsg::ExternalProcess`, `runner_main/executor.rs:800`) onto the step in `status.json` (`runner_main/status.rs:85,150`; pi `updateExternalProcess`, `subagent-runner.ts:2247-2251`). Pending external-CLI steps declare their runner as upstream does (`background/flat_index.rs:61`, called from `runner_main/entry.rs` and the chain-append path in `turn_loop.rs`). The logs now go into the run's own directory (`RunOptions::external_log_dir`, `exec/agent_config.rs:739`, set at `runner_main/executor.rs:1032`, read at `exec/external_cli/mod.rs:204`; pi `asyncDir: path.dirname(ctx.outputFile)`, `subagent-runner.ts:924`), inside Fleet's containment root — before this they sat in the per-cwd scratch dir, where two runs in one cwd overwrote each other's `external-0.*.log`. Fleet renders `external-cli · <elapsed>` on the step row (`background/fleet_view.rs:465`; `fleet-view.ts:342-349,365`), reads an adapter's final-output file first, and tails `External stderr tail` then `External stdout tail` ahead of the transcript while the step runs and after it once settled (`fleet_view.rs:1158`; `fleet-view.ts:569,590-609`). The stderr-tail bound now counts UTF-16 code units, JS `length` (`background/reconcile.rs:1046,1066`). Verify: `background::runner_main::executor::tests::an_external_cli_step_publishes_its_process_live_and_logs_into_the_run_dir`, `background::runner_main::status::tests::an_external_process_report_is_published_onto_its_step_in_status_json`, `background::flat_index::tests::a_pending_external_cli_step_declares_its_runner`, `background::fleet_view::tests::fleet_shows_an_external_cli_steps_elapsed_time_and_its_stream_tails`, `background::reconcile::tests::the_stderr_tail_bound_counts_utf16_units_like_upstream` — each red with its fix reverted. **CYRUP-DELTA:** a UTF-16 cut through a surrogate pair renders the orphaned half as U+FFFD instead of emitting a lone surrogate (`reconcile.rs:1066`). **Ledger correction:** "revived runners still spawn unobserved" was already untrue — a revive goes through `spawn_background_steps` (`extension/executor/control.rs:512`), whose only spawn is the observed one (`extension/executor/background.rs:1009`); no production caller of the unobserved spawn remains (only the orchestrator-sim bin). **Not here:** run-status's external-runner block (`Runner:`/`Adapter:`/`Process:`/`Stdout:` lines, `run-status.ts:640-663`) is unported as a whole and is not this row's Fleet scope — ported 2026-09-30 under `SUBA-134` (`background/run_status.rs:635`). *Earlier text:* Observability drift: external-CLI stdout/stderr tails are not shown in Fleet, and a runner that dies without a result is not reported with its exit code and signal — **PARTIALLY CLOSED 2026-09-28** (on `claude/lows-next`): the runner-exit half landed. The launching process now watches the detached runner (`crates/cyrup-ext-subagents/src/background/spawn_detached.rs:302`, called from `extension/executor/background.rs:964-967`; pi `proc.once("close")`, `async-execution.ts:759-770`) and finalizes the proof with the observed exit code and signal name. An `unknown` proof carries the runner instance, and a sticky unknown gains the exit only if it has none and is not proof-write-failed (`background/process_terminal/finalize.rs:326,347,383`). The dead-pid repair reads `Async runner process {pid} exited with code N (signal S)` or `exited or disappeared`, and a live stalled pid gets its own sentence (`background/reconcile.rs:397,664-712`; `stale-run-reconciler.ts:231-238,434-443`). The stderr tail uses upstream's bounds (last 64 KiB, last 30 lines, at most 4 000 characters plus `[stderr tail truncated]`) under `Runner stderr tail:` (`reconcile.rs:741,1065`). Verify: `background::reconcile::tests::{a_runner_killed_by_a_signal_reconciles_with_its_exit_code_and_signal,an_unobserved_dead_runner_exited_or_disappeared_with_its_stderr_tail}`. **Remaining:** the Fleet half. Upstream reads `step.externalProcess` from status.json, which its runner publishes from the external runner's live process hook (`subagent-runner.ts:2248`). cyrup's `StepStatus` (`background/records.rs:24`) has no such field (only `SingleResult.external_process` does), and `exec/external_cli` has no live hook. That also blocks the step-row elapsed time (`fleet-view.ts:344`). Smaller: the 4 000 bound counts chars, not UTF-16 units, and revived runners (`background/control.rs`) still spawn unobserved. |
| SUBA-142 | tracker | tracker | — | In-process child sessions: upstream builds children in-process; cyrup spawns a process. The design question, plus the in-process-only features parked behind it **FOLD-IN 2026-10-03 (v0.75.0):** `07946874`/#2636 and `1fe508f1`/#2638 make children launch and verify on extension-registered virtual models (`pi-virtual` api); the child's `virtualModelId` (`src/runs/shared/child-session.ts:118,589` @v0.75.0) replaces the router's physical model in `formatSubagentModelVerificationError` (`run-child-session.ts:517`, `foreground/execution.ts:1114`). In-process only, so parked here, no row. When `SESS-067` (area 03, the session half of virtual models) lands, `exec/model_verification.rs` must compare the child's selected model rather than the router's physical model, or a correct child fails `model_verification_failed`. |
| SUBA-144 | low | upstream-drift | M | **NEW 2026-09-30** (filed by lane A2 of `claude/lows-batch3` while closing SUBA-134). The runner emits no `subagent.nested.updated`/`subagent.nested.completed` events (upstream `nestedSummaryFromAsyncStatus`, pi-subagents v0.71.0) and never creates a nested route for a root run (SUBA-115's reach note), so `NestedRunSummary.session_name` and the nested exact-status view added under SUBA-134 have no production producer — they are fed only by relayed events from a writer that sets them. Same area, same port: the nested lookup lists every route under `Roots::nested_events` rather than upstream's state-scoped routes; there is no nested prefix matching, no `reconcileNestedAsyncDescendants` and no nested transcript view; the nested summary carries no `turnBudget`/`model`/`thinking`, so those status segments stay empty. |
| ~~SUBA-145~~ | ~~low~~ **CLOSED 2026-09-30** | upstream-drift | S | **CLOSED 2026-09-30** (on `claude/lows-batch4`): the row named three differences; the drift was wider and all of it, from the same upstream lines, is closed. (1) `fleet_view::format_activity_label` (`background/fleet_view.rs:~280-292`) now prints `active but long-running · last activity now` (no `ago`), per `status-format.ts:19-20`; test `fleet_view::tests::activity_label_matches_pis_five_branches` gained the `now` cases (red before: `last activity now ago`). (2) `format_status` (`background/run_status.rs`) is now `format_status_with(status, paths, deps, nested, now)` (`:474`) with `format_status` a thin wrapper that reads the clock and the nested registry. It renders, at upstream's positions: the run-level `Activity:` and `Steering:` lines between `Error:` and `Mode:` (`run-status.ts:589-590`; `Activity:` only for a running run), the per-step `, <activity>` text of a running step (`:624,635`), each step's nested tree with `NestedLinesOptions { indent "  ", command_hints true, max_lines 20 }` (`:676`), the unattached tail with indent `""` (`:691-693`), the `Warning:` line (`:703`; a failed nested lookup, joined with `; ` to an unreadable mission binding, which moved here from its old position under `Run:`, `:557`), the running-step `  Intercom target: … (if registered)` + `  Steer: subagent({ action: "steer", … index })` for a local non-workflow step (`:680-681`), `  Steer: unavailable; external runners do not accept live messages.` for a running external-cli step (`:683`, const `EXTERNAL_RUNNER_STEER_UNAVAILABLE`), and the run-level `Steer running child:` hint for a running, not-all-external, non-workflow run (`:707`, which the row did not list). (3) `spawn/nested_events.rs`: `attach_root_children_to_steps` (`:1629`, port of `nested-events.ts:964-975`; returns the per-step attachment because a persisted `StepStatus` has no `children` field; `MAX_CHILDREN = 16`), plus `find_nested_route_for_root_id_in` / `project_nested_registry_for_root_in` (`:1566,:1606`) over an explicit events root. (4) CYRUP-DELTA: upstream reads the nested registry from a process-global route lookup; `RunStatusRenderDeps::nested_events_root` (`run_status.rs:324`) carries the `Roots::nested_events()` tree instead (set by `extension/executor/status.rs` from the executor's roots; `None` = no lookup), which is what makes the whole path testable end to end. (5) The module header (`run_status.rs:10-30`) now lists what is and is not rendered. Verify (all in `cyrup-ext-subagents`, no cyrup-it): `run_status::tests::{a_running_local_step_gets_its_intercom_target_and_steer_hint, a_running_external_runner_step_says_steer_is_unavailable, activity_labels_ride_the_run_and_its_running_steps, root_children_attach_to_the_step_they_name, nested_runs_render_under_their_step_and_the_unattached_tail_after, a_nested_lookup_failure_and_a_bad_mission_binding_share_one_warning_line, the_nested_registry_is_projected_from_the_deps_events_root}` and `fleet_view::tests::activity_label_matches_pis_five_branches` are red with their fix lines disabled (8 failures; the attach test red with the dedupe/cap lines removed: `left: ["a", "b", "a"] right: ["b", "a"]`); `a_running_workflow_step_gets_no_intercom_target` is a guard (green both ways). `a_non_running_workflow_renders_no_steer_lines` was updated: its running-single-run half asserted no `Steer` at all, which upstream's `Steer running child:` hint contradicts. *Not closed, filed as `SUBA-146`:* upstream's `reconcileNestedAsyncDescendants` pass before the projection, the `Session:` line, the all-external `Resume:` sentence. Nothing produces nested events in production until `SUBA-144` lands, so the nested tree renders only what a test or an external writer puts in the events tree. *Earlier text:* **NEW 2026-09-30** (filed by lane A2 of `claude/lows-batch3`). `run-status` @v0.71.0 still differs in three places after SUBA-134/141: the per-step nested tree in the main report is unported; the `Steer: unavailable; external runners do not accept live messages.` line is absent; and `fleet_view::format_activity_label` prints `last activity now ago` where upstream prints `last activity now`. |
| SUBA-146 | low | upstream-drift | S | **NEW 2026-09-30** (filed while closing `SUBA-145` on `claude/lows-batch4`). **RE-SCOPED 2026-10-02** (`claude/lows-batch5`): parts (b) and (c) are DONE and no longer open — see below; (a) and (d) remain. `run-status` @v0.71.0 differences left after `SUBA-145` (line numbers in (b)/(c) are @v0.71.0; the closure below cites @v0.75.0, where they are `:711` and `:716`): (a) `reconcileNestedAsyncDescendants(route, { resultsDir, kill, now })` (`stale-run-reconciler.ts:325`) runs before the nested projection in the main report (`run-status.ts:541`) and before the nested exact-status view (`:436`); cyrup projects the registry as-is, so a nested async descendant whose runner died still shows `running`. (b) ~~The `Session: <status.sessionFile>` line (`run-status.ts:705`) is not rendered (`RunStatus::session_file` is carried); the stale comment at the `Workflow receipt:` site says cyrup has no such line.~~ **DONE 2026-10-02.** (c) ~~For an all-external run (every step external-cli/external-job) a non-running report ends in `Resume: unavailable; external runners do not persist Pi sessions.` (`:709-710`) instead of cyrup's `formatResumeGuidance` result; the external-job follow-up variant needs the unmodelled `external-job` runner.~~ **DONE 2026-10-02**, except the external-job follow-up variant, which still needs that runner and stays open under this row. (d) Per-step `, acceptance: <status>` and `, turn budget: n/m+g (outcome)` suffixes (`:631-632`) and the run-level `Turn budget:` line have no source field on `StepStatus`/`RunStatus`. Fix direction: port (a) beside `cascade.rs`'s registry walk, add (b) and (c), and decide whether (d) waits on the turn-budget/acceptance status fields. **(b)+(c) closure evidence:** `run_status.rs` now pushes `Session: <path>` right after `Workflow receipt:` (an empty path prints no line, as JS truthiness does) and, for a non-running run whose every step is `external-cli`, `Resume: unavailable; external runners do not persist Pi sessions.` (`EXTERNAL_RUNNERS_NOT_RESUMABLE`) in place of `format_resume_guidance` (`run-status.ts:711`, `:714-716` @v0.75.0). The module doc and the stale comment at the `Workflow receipt:` site were corrected. Tests (`background::run_status::tests`): `a_report_names_the_session_file_after_the_receipt_and_before_the_steer_hint` and `a_finished_all_external_run_says_external_runners_do_not_persist_sessions` (both FAILED before the fix, committed red in `d2d1bc68`), plus the guards `a_report_has_no_session_line_without_a_session_file` and `the_external_resume_sentence_needs_every_step_external_and_a_non_running_run` (pass before and after: they pin what must not change). Not verified: the `external-job` variant, and the mixed-runner and running cases beyond those guards. Remaining fix direction: port (a) beside `cascade.rs`'s registry walk, and decide whether (d) waits on the turn-budget/acceptance status fields. |
| ~~SUBA-147~~ | ~~low~~ **CLOSED 2026-10-03** | not-ported | M | **CLOSED 2026-10-03** (`claude/lows-batch6`) as a WRONG PREMISE, superseded by `SUBA-149`; no code change. Read at `tmp/pi-subagents` v0.75.0 (the row's lines were @v0.71.0 and have moved): `preflightWorktreeSource` is `src/runs/shared/worktree.ts:359-369` and runs `rev-parse --is-inside-work-tree`, `--show-toplevel` and `status --porcelain`, reporting `worktree isolation requires a git repository` or `... a clean git working tree. Commit or stash changes first.` It has exactly TWO call sites, not the row's description: (1) workflow admission, `preflightWorkflowWorktrees` (`src/runs/foreground/subagent-executor.ts:4941-4968`), invoked from the engine's `admit:` callback (`:6130` async, `:6442` foreground) once per `runs.run` / `runs.all` batch, with the `Worktree admission failed for '<keys>' at <cwd>: <reason> Select the correct cwd or arrange an operator-approved commit/stash.` wrapper (keys sharing a cwd are listed together); (2) the direct async single call (`:7454-7459`), which reports the RAW probe message with no wrapper. Upstream has no preflight for sync single, `tasks[]` or chain groups. The row's premise fails for cyrup: a `worktree: true` single run or workflow child never gets a worktree here. `worktree` is parsed (`extension/tool/params.rs:216`) but consumed only by the parallel `tasks[]` shape (`routing.rs:2717`) and chain parallel groups (`spawn/chain_graph.rs:1827`, `assign_worktree_cwds`); `WorkflowScriptHost::admit` has only its default `Ok(())` (`workflows/scripted/engine.rs:124`, no override). Porting the probe now would reject dirty-tree launches for children that would never use a worktree, an invented failure. "Earlier group members may have run" is also false inside one group: `assign_worktree_cwds` sets up the whole group before dispatch, so a dirty or non-git source fails before any member of that group runs. This is established by reading source and grepping every consumer of the field; the silent drop was NOT observed by running it. The real gap is filed as `SUBA-149`. Original finding: ~~**NEW 2026-09-30** (filed while closing `SUBA-130` on `claude/lows-batch4`). `preflightWorktreeSource(cwd, { signal, deadlineAt })` (`worktree.ts:359-368`) is unported: upstream's executor runs a read-only admission probe of every distinct `worktree: true` source cwd BEFORE launch (`subagent-executor.ts:4739` for workflows, `:7120` for a plain call) and fails the whole launch with `Worktree admission failed for '<key>' at <cwd>: <reason> Select the correct cwd or arrange an operator-approved commit/stash.`; cyrup discovers a dirty or non-git source only inside `create_worktrees`, after earlier group members may have run. The probe's git calls are the same bounded ones `SUBA-130` added (`resolve_repo_state` with a `GitBounds`), so the port is the admission loop plus the error text.~~ |
| SUBA-148 | low | not-ported | S | **NEW 2026-09-30** (filed while closing `SUBA-130` on `claude/lows-batch4`). Three places where worktree git still has no stop or deadline because the caller has no run context: `handoff::discard_preserved` (`worktree.discard`), the `worktree.cleanup` plan builder (`spawn/cleanup_plan/git.rs::run`, `classify.rs::is_patch_captured`) and `resolve_expected_worktree_agent_cwd` take `GitBounds::unbounded()`; and `create_worktrees` / the harvest wait on `worktree_turn()` (an unbounded `tokio::sync::Mutex`) without honouring stop or deadline, so a hung discard holding the turn blocks every later launch. Upstream v0.71.0 is equally unbounded in the git calls (`spawnSync`), so this is not drift; it is room to do better. Fix direction: thread the tool call's cancel token into `discard_preserved` and the plan builder, and race the turn lock against the run's stop/deadline. |
| ~~SUBA-149~~ | ~~medium~~ **CLOSED 2026-10-05** | not-ported | L | **NEW 2026-10-03** (filed while closing `SUBA-147` on `claude/lows-batch6`; established by reading source, not by running it). Single-agent and scripted-workflow-child `worktree: true` isolation is unported. Upstream allocates a managed worktree for a single run (`worktreeSetup` and `retainSingleWorktreeHandoff`, `subagent-executor.ts:3998`, the single path in `worktree.ts`) and for each workflow child whose effective `worktree` resolves true. cyrup accepts the flag (`extension/tool/params.rs:216`, `workflows/scripted/engine.rs:832` whitelist and `:1519-1522` boolean validation) but only the `tasks[]` shape (`routing.rs:2717`) and chain parallel groups (`spawn/chain_graph.rs:1827`) consume it, so the request is silently dropped on every other shape and the child runs in the shared cwd. Impact: a caller who asks for isolation gets none, and no error says so. **Fix** — allocate and hand off a worktree for a single run and for each workflow child that resolves `worktree: true`; THEN add the admission probe `SUBA-147` described: (a) `WorkflowRunHost::admit` (`extension/executor/workflow.rs:750`, currently not overridden) calling a `preflight_worktree_source(cwd, GitBounds)` (the repo and clean-tree half of `resolve_repo_state`, `spawn/worktree.rs:645`, without the HEAD resolve) and reporting `Worktree admission failed for '<keys>' at <cwd>: <reason> Select the correct cwd or arrange an operator-approved commit/stash.`, keys sharing a cwd listed together, children with `resume` skipped; (b) the same probe with the RAW message on the async-single launch (`subagent-executor.ts:7454-7459`). Check how the admission changes the order of the engine's existing claim logic before wiring it. **Verify** — a single run and a workflow child with `worktree: true` each run in a managed worktree; a dirty source fails the whole workflow batch before any child launches. **PARTIAL 2026-10-04** (`b9e35f5d`) — **NOT closed, and the reason is the default shape.** Landed: `WorktreeRequest{Shared,Isolated}` (no `Default`, so nothing can forget to answer it) and `ManagedLaunch` with private fields and one fallible async constructor, so no value of the type says "isolation requested" while `child_cwd()` points at the shared cwd — the original bug made unrepresentable rather than re-checked. Allocation + pi's preserve-and-publish hand-off (diff captured to `<artifactsDir>/worktree-diffs/<runId>` BEFORE removal, `Retained` on a write failure, detached runs exempt per `subagent-executor.ts:4387`) for the **foreground** single run and for **every scripted-workflow child**. `WorkflowRunHost::admit` (Fix 2a) probes once per batch, memoised on the batch id, with upstream's wrapper sentence, keys grouped by cwd and `resume` children skipped. `preflight_worktree_source` is shared with `resolve_repo_state` so admission and allocation cannot drift on what a usable source is. **WHAT REMAINS, and it is the common case:** `cfg.async_by_default` is `true` (`registration/mod.rs:684`; its own doc says "absent and `true` both mean background"), so a bare `subagent({agent, task, worktree: true})` takes the ASYNC path — and `SingleStepSpec` (`spawn/chain_graph.rs:71`) has **no `worktree` field at all**, `worktree` being consumed only on `ParallelGroupSpec`. So the silent drop this row is about STILL HAPPENS on the default shape for a single call, and on a sequential chain step, where upstream's background runner isolates both (`runs/background/async-execution.ts:1336-1340`). Fix 2(b), the raw-message probe at the async-single launch, is deliberately NOT done because probe-without-allocation would reject dirty-tree launches for children that can never use a worktree — the invented failure `SUBA-147` was closed as a wrong premise for. Owed, in order: `SingleStepSpec::worktree` plus allocation and hand-off in `walk_chain`'s `SingleStep` arm wired from `BackgroundSingleRequest`, THEN Fix 2(b). A second L-sized piece, not a tail. No `[CYRUP-DELTA]`: the remainder is a lost guarantee, which no delta can cover. **EVIDENCE CORRECTED:** (i) this row has NO body section — `09b` jumps `## SUBA-110`…`## SUBA-173` with no heading for 149, so it is table-only; (ii) `workflows/scripted/engine.rs:832` is the `"worktree"` entry of `AUTO_RESUME_PARAM_KEYS` (the 18 keys carried across a setup-abort auto-resume relaunch, pi `:1679`), NOT the acceptance whitelist — the flag IS preserved there, so the cite is not false, but it misdescribes what the line is; acceptance is the boolean validator at `:1517-1522` (row says `:1519-1522`); (iii) `extension/executor/workflow.rs:750` pointed at `impl WorkflowScriptHost for WorkflowRunHost`, not at an `admit` — there was none in that impl; the trait default lived at `engine.rs:124`. Confirmed accurate: `params.rs:216`, `routing.rs:2717`, `chain_graph.rs:1827`, and the central claim that the request was silently dropped on every other shape. Pinned by 25 new tests; the ordering is proven message-independently by `one_childs_dirty_source_stops_a_sibling_that_wanted_no_isolation`, where the shared-cwd sibling writes its `pwd` outside any worktree and the test asserts the file does not exist BEFORE any message check — without the admission that sibling launched and worked against a tree the operator was never warned about. — **CLOSED 2026-10-05** (`acf72ecc`, completing `b9e35f5d`). All three parts done. `SingleStepSpec::worktree` is a MANDATORY `WorktreeRequest` with no `Default`, which surfaced **46 construction sites, 12 of them production** — the compiler named the two the previous lane found by grepping, plus the async single at `extension/executor/background.rs:250` that kept this row open. `run_single_step` allocates through `ManagedLaunch::resolve`, dispatches with `child_cwd()` and hands back via `finalize`; an unallocatable request aborts the walk rather than degrading to the shared cwd. Wired from `BackgroundSingleRequest::worktree` and `route_single_background`. Part 2 covers all three sequential authoring surfaces — the authored chain file (`discovery/chains.rs`), the tool's `chain[]` (`extension/tool/task_items.rs`, where the key **was not parsed at all**, so `chain: [{agent, task, worktree: true}]` was accepted and ran un-isolated) and the walker itself. Part 3, Fix 2(b)'s probe, landed AFTER allocation as the row's own ordering requires. `SingleHandoffBinding` gains `mode`/`step_index`/`flat_start_index`, so the diffs dir is `worktree-diffs-step-{n}` and two isolated steps of one chain cannot overwrite each other's patches. Detached runs are exempt from the settle (pi `subagent-runner.ts:4732`), matching the foreground path's `:4387`. **The headline test does not pin `async: false`** — payload is exactly `{agent, task, worktree: true}`, it asserts `cfg.async_by_default` is still `true` so a flipped default moves the test rather than silently retesting foreground, it panics unless pi's `Async: {agent} [{id}]` headline appears, and only then reads the real `runner-config.json` hop 1 writes. That is the trap the first attempt at this row fell into. 9 mutations, 9 verbatim reds. **A THIRD dropped-isolation surface the row never names, found and fixed:** `/prompt-workflow --worktree` and a recipe's `worktree: true` frontmatter were folded into one effective request by `workflow_params` and then read NOWHERE — both `slash.rs` dispatch arms hard-coded the shared cwd. Literally this row's sentence on a surface it does not mention. It carries a `[CYRUP-DELTA]` because v0.75.0 has **no such surface at all**: `slash/prompt-workflows.ts` has zero `worktree` mentions and `slash/prompt-template-bridge.ts:232-238` REFUSES a legacy one, so the field's own doc cite is stale — ported from an older baseline. With no upstream behaviour either way the choice was honour-or-delete, and honouring what the usage string advertises is the smaller change. **Recorded as NOT a defect:** `slash_commands.rs:873`'s `GroupConfig::worktree` is parsed and unread, but `ParsedChainElement`/`parse_group_segment`/`step_token_to_spec` have no production caller at all — upstream deleted `/chain`, `/parallel`, `/run-chain` and `/chain-prompts` at v0.41.0 and the grammar helpers are retained deliberately. Nothing is dropped because nothing reaches them; recorded so a later pass does not file it. Cite drift: the `tasks[]` consumer is now `routing.rs:2944` (row says `:2717`). Confirmed accurate: `async_by_default: true` at `registration/mod.rs:684`, `SingleStepSpec` at `chain_graph.rs:71` with no `worktree`, `params.rs:216`, `chain_graph.rs:1827`, and that this row has NO body section. **Three adjacent residuals, each another row's:** the `subagents.worktree` CONFIG rung (pi `:7334-7335`) is unported and is already census-tracked by `SUBA-113`; `worktreeSetupHook`/`worktreeProvider`/`baseRef`/`worktreeBranchPrefix` never reach hop 2 (`RunnerConfig` carries `worktree_base_dir` alone), so background runs no setup hook where foreground does; and `resolve_expected_worktree_agent_cwd` has no production caller, so an isolated child's prompt text still names the shared cwd — a prompt-fidelity gap, harmless for output resolution. |
| ~~SUBA-150~~ | ~~medium~~ **CLOSED 2026-10-04** | upstream-drift | L | **`workflow: true` reply-fenced scripts replaced `workflowScript`/`workflowScriptPath`, which v0.74.0 deleted from the tool** — one `workflow` field now takes `true` \| a path \| a resource name (`src/extension/schemas.ts:221` @v0.74.0, `src/extension/reply-workflow-script.ts:14`); cyrup still advertises both removed params at `extension/tool/schema.rs:345`. **FILED 2026-10-02**; body below. **FOLD-IN 2026-10-03 (v0.75.0):** (a) `df3b6df1`/#2610 makes the string `"true"` equal `workflow: true` for MCP clients that stringify booleans (`src/extension/reply-workflow-script.ts:29`, `rpc.ts:527`, `index.ts:744` @v0.75.0); it is part of the `workflow` field this row ports. (b) `cfb6f9a8`/#2611: `action: "validate"` now checks workflow `args` against the same limits as a launch, exports `MAX_ARGS_FIELDS/ITEMS/DEPTH/BYTES` and states them in the `args` schema description (`src/extension/schemas.ts:7,227` @v0.75.0). Cyrup already enforces 16 fields, 64 items, depth 8 and 16 KiB in `workflows/resources.rs` (`validate_plain_json` and `normalize_args`, `:385-445`; they are literals, only `MAX_ARGS_BYTES` is a private const at `:29`), but its `validate` arm (`extension/tool/routing.rs:1870`) takes only `workflowScript` and never runs `normalize_args`, and `args` is advertised and plumbed only for scheduled runs (`extension/tool/schema.rs:742`, `background/scheduled_runs/tool.rs:355`). Port: export the four limits as constants, state them in the `args` description, and run `normalize_args` in `validate`, reporting its error beside the script errors. — **CLOSED 2026-10-04** (`8fa93e66`). One `workflow` field taking `true` | a path | a resource name, with `workflowScript` kept as the INTERNAL carrier so the mode gate, `route_workflow_mode`, the `validate` arm and `schedule.create` needed no rename — upstream's own move (`subagent-executor.ts:7928-7958` @v0.75.0). New `extension/tool/workflow_field.rs` holds the functional core: `WorkflowSource::{ReplyBlock, ScriptFile, Resource}` parsed ONCE at the boundary, because upstream re-derives that classification four times and that duplication is exactly how `"true"` became a resource lookup (#2600). Both fold-ins done: the string `"true"` is coerced before any branching, including in the one-per-reply count; and the four arg limits are promoted to named consts used by the advertised description, `maxProperties`, the `validate` arm and the schedule parse, so the stated and enforced bounds cannot drift. **The decision the row left open is resolved as full parity:** both removed parameters are gone from the advertised schema AND refused at dispatch with upstream's two verbatim sentences, so the `[CYRUP-DELTA]` the row anticipated is NOT needed. **EVIDENCE STALE:** the row's "zero hits for `js workflow` / `reply_workflow`" was true when filed but `extension/reply_workflow_script.rs` landed 2026-10-04 in `81200403`, so half of Fix step (b) was already done and only the session walk was missing. Cite drift: `workflowScript` was at `schema.rs:353` not `:345`, `args` at `:750` not `:742`. Pinned by 12 tests in `workflow_field_tests.rs` plus 3 in `schema.rs`, each proven red; `validate_and_the_launch_path_agree_on_the_exact_boundary_of_every_limit` is the one that fails if the two paths ever diverge. **Recorded alongside the closure:** cyrup's workflow launch does not consume `args` at all (`cyrup-workflow-runtime`'s prelude installs no `args` global), so `validate` checks them against the same normalizer `schedule.create` uses — one implementation, three call sites — while upstream additionally hands the normalized args to the script deeply frozen. That exposure belongs to the scheduled-runs Part-B residual, not here. |
| SUBA-151 | ~~medium~~ **CLOSED 2026-10-05** | upstream-drift | L | **`chain`, `tasks` and the whole dynamic-fanout schema are gone from v0.74.0's default tool** — they survive only in the reduced schema used when `disabledFeatures` lists `workflow-scripts`, and are lowered into package-owned scripts (`src/workflows/structured-workflow-scripts.ts:175`, `src/extension/schemas.ts:286,299`); cyrup advertises the full v0.71.0 shape (`extension/tool/schema.rs:259,520,527`). **FILED 2026-10-02**; body below. **PREMISE CORRECTED 2026-10-04 — the row's upstream claim is false, and following it literally would have DELETED a working capability.** The row says v0.71.0's `SubagentParamProperties` carried `chain`/`tasks` and that v0.74.0 deleted them. Measured at the pin: that property set has **zero** top-level `chain`/`tasks`/`chainDir`/`concurrency` at v0.71.0, v0.73.0, v0.74.0 AND v0.75.0, and v0.71.0's `public-execution.ts:155` ALREADY refuses them — *"Legacy top-level chain and parallel inputs were removed; use workflowScript."* The data-shaped surface left pi's default tool before v0.71.0, at the v0.43.0 public-execution cutover. `ChainItem`/`ParallelTaskSchema` were exported at v0.71.0 but never referenced by the tool's properties, so what `71d042f0`/#2596 did at v0.74.0 was (i) delete types already dead for the tool and (ii) RE-INTRODUCE a deliberately reduced `tasks`/`chain` pair for the `workflow-scripts`-disabled case, with a lowering into a package-owned script. cyrup's `chain`/`tasks` are therefore a port of the PRE-cutover (≤ v0.43.0) shape, not of v0.71.0. The row is not "upstream deleted these, cyrup still advertises them" but "upstream moved these behind a config flag years ago; cyrup never followed the cutover" — same severity, different fix. **DECISION OF RECORD (option b): keep the native engine and the full schema.** pi still supports these shapes by another route and cyrup offers both routes, so removing them would delete a working capability for nothing — that would be the lost guarantee, not keeping them. Pinned by `chain_and_tasks_still_dispatch_to_the_native_graph_rather_than_a_removal_refusal`, which fails loudly if a later pass deletes the shapes. **PARTIAL 2026-10-04** (`8fa93e66`): the two narrowings portable without a hinge are ported — `toolBudget`'s `soft <= hard` and the `timeoutMs`/`maxRuntimeMs` "must agree", each stating a bound the dispatcher already refused by name while the schema called it legal. **SECOND EVIDENCE ERROR:** the row's "three narrowings landed in the same diff" is the wrong release — all three are present at v0.73.0 (`schemas.ts:123`, `:133`, `:361`), so they predate #2596 and are not in its diff; and `usageBudget.minProperties: 1`, still listed as to-port, was already ported (`schema.rs:108`, citing this row). **REMAINS:** option (c) only — port `createSubagentParamsSchema`'s reduction so `disabledFeatures: ["workflow-scripts"]` yields the reduced pair. **BLOCKED on `SUBA-152`** (`disabledFeatures` unported), which is rated `low` while gating this `medium`; three of the five properties the reduction drops are not advertised by cyrup at all. **CLOSED 2026-10-05.** Option (c) is ported: `subagent_tool_parameters_for(&DisabledFeatureSurface)` (`extension/tool/schema.rs`), pi `createSubagentParamsSchema` (`src/extension/schemas.ts:301-312` @v0.75.0 — NOT `:286-312`; `:287` is `StructuredTask` and `:288-299` is `StructuredWorkflowProperties`). The guard is upstream's own `disabled.params.size === 0` identity door, not a `features` test. Wired via a `SubagentTool::with_disabled_features` builder at the two registration arms that already hold the config — **including ChildSafe, because upstream reduces in the fanout child too** (`extension/fanout-child.ts:219` calls `createSubagentParamsSchema` itself rather than inheriting the parent's object). No signature widened. **The reduction is generic over all 15 groups plus the synthetic `schedules` surface, which is where most of this row's value sits** — framing it as the `workflow-scripts` path alone would have under-delivered. **This row's ‘three of five’ count was right but its membership was wrong:** `args` IS advertised (`schema.rs:805`), so `workflow-scripts` drops TWO properties here, not one; the three absent are `preflight`, `globalConcurrencyLimit`, `maxSubagentSpawnsPerRun`. **Option (b) is preserved and pinned:** cyrup's `chain`/`tasks` entries are strict supersets of pi's minimal reduced pair (same names, same type, same `required` on the `tasks[]` item; cyrup's `chain[]` item carries all four of upstream's members plus eight more), so emitting pi's pair would REMOVE advertised capability the dispatcher still accepts — in the one session shape where a script-less orchestrator needs them most. They pass through untouched, pinned byte-for-byte against the default entries. The `task` re-description and the `tasks`/`chain` emission are therefore **evidenced no-ops**: pi's reduced `task` sentence introduces `chain`/`tasks` and `{task}` only because in pi they exist solely in the reduced schema, whereas here both are unconditional (`extension/executor/chain.rs:403`), so saying it only when reduced would make advertised text vary on something that does not vary. **`props.shift_remove`, never `remove`:** under workspace-wide `serde_json/preserve_order` (`Cargo.toml:253`) `remove` is `swap_remove` and drags the last property into each hole — the set stays right while the ORDER silently changes, which no set-based assertion can see. The default (no-disabled-features) schema is pinned byte-identical by a measured digest taken from `59186c60`. **Two narrowings found and deliberately NOT ported** (each would move that digest, which this row forbids): `minItems: 1` on `tasks`/`chain`, which the dispatcher already refuses (`routing_tests.rs:751-759`), and `additionalProperties: false` + `agent.minLength: 1` on the `tasks[]` item — same defect class as the already-ported `usageBudget.minProperties`, and worth their own row. **BOTH LANDED 2026-10-06** alongside `SUBA-152`'s wiring: `minItems: 1` on `tasks` (`schema.rs:625`) and `chain` (`:635`), and `minLength: 1` on the `tasks[]` item's `agent` (`:198`). The FOURTH candidate, `additionalProperties: false` on that item, was deliberately NOT added — the dispatcher does not refuse an unknown key there, so advertising it would claim a bound that does not exist; it is asserted ABSENT with its reason, so a reader who notices the asymmetry with the `chain[]` item finds the measurement instead of repeating it. The default schema's pinned digest was therefore **deliberately re-baselined** (`beaab01c…` → `b0e48cee…`), and the pin now records three distinct mutation digests, one per narrowing, so it discriminates between them rather than merely noticing that something moved. Gates: `-p` and workspace clippy clean on real `Checking` runs; crate suite 5016 → **5022**; workspace **13954 passed, 0 failed**. |
| SUBA-152 | ~~low~~ **CLOSED 2026-10-06** | not-ported | M | **`config.disabledFeatures` is unported** — 15 named feature groups an operator removes from the `subagent` tool, each mapping actions and params to the setting that disabled them (`src/shared/disabled-features.ts:8,82,98` @v0.74.0, validated `src/extension/config.ts:182`); `disabled_features` has zero hits in `crates/`. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** the claim that cyrup accepts `disabledFeatures` and "drops it with no warning" is false. `SubagentExtensionConfig::config_warnings` (`registration/mod.rs:927-945`) runs `discovery::key_census` and emits `unknown key 'disabledFeatures' (ignored)` (`:944`); `crates/cyrup/src/subagent_config.rs:75-77` prints it to stderr on load (only when `validate_raw_config` passes). What is true is narrower: the key is not in `UNPORTED_CONFIG_KEYS` (`registration/mod.rs:572`, 10 entries), so the message is the generic typo wording rather than the "is not supported by this port (...); it has no effect" wording an unported key gets, and nothing says the key is a real upstream feature. The upstream cites (`disabled-features.ts:8/82/98`) hold; v0.75.0 only widens upstream's fail-closed list (see `SUBA-166`). **PARTIAL 2026-10-05 — recorded as CLOSED earlier the same day IN ERROR, and reopened.** The strike was premature: this row's **Fix** requires the surface to be consulted in THREE places — the advertised schema (`extension/tool/schema.rs`), the tool description (`registration/tool_description.rs`, prepending the notice) and the dispatch boundary (`extension/tool/routing.rs`, before any action runs). Only the FIRST is wired, and it landed under `SUBA-151`, not here. `disabled_feature_use_error` and `disabled_feature_notice` have **zero production callers** (verified: every non-test mention in `crates/` is a doc link), so a session that disables a group now stops ADVERTISING its params but still DISPATCHES a caller that sends one anyway. Upstream's own SAFETY note (`schemas.ts:310`) states both halves — *"the executor rejects disabled options"* — and that half is absent. The row's **Verify** clauses are all satisfied at function level, which is what misled the close; the Fix is not. **CLOSED 2026-10-06** — both remaining wirings landed, and **this row's Fix was WRONG about where the notice goes.** The Fix says `registration/tool_description.rs`. Upstream calls `disabledFeatureNotice` in exactly ONE place, `runs/foreground/subagent-executor.ts:6883` @v0.75.0, gated on `topic === "tool-reference"` — the **`guide` topic**, which is what the function's own doc means by *"for prepending to static reference docs that describe the full tool"*. It is NOT on the tool description; wiring it where this row said would have been a divergence, and putting it on the description is a separate, larger port. Pinned by `the_disabled_feature_notice_leads_the_tool_reference_guide_topic_only`. **The dispatch gate** is `disabled_feature_use_error` in the `subagent` tool's execute prologue (`extension/tool/mod.rs:290`) — ONE call site where upstream needs two, because cyrup's tool and RPC paths funnel into this entry, so a second copy would only add a way for the two to disagree; the RPC path is proved to share it by `rpc_spawn_is_refused_by_the_same_disabled_feature_gate_as_the_tool`. Three properties of that position are load-bearing and documented there: it sees the RAW request (a declared `{"gate": null}` IS a use of `gate`, which the typed params cannot carry because serde folds an explicit `null` into the same `None` an absent key gives); it runs BEFORE the `workflow`→`workflowScript` lowering (upstream: *"Callers check the original request, before the package lowers chain/tasks into its own script"*), so a `workflow: "./x.js"` call is refused on the param it sent rather than as a carrier; and it precedes every side effect, so the refusal is the tool error the caller sees with no partial work done. Upstream's own call sites are `subagent-executor.ts:5288`/`:5294` and `rpc.ts:535` — not `:5286`. Crate suite 5022 → **5029**; workspace **13961 passed, 0 failed**, the whole workspace green in a single pass for the first time this week. What landed earlier, and stands: ported to `crates/cyrup-ext-subagents/src/disabled_features.rs` with all four upstream behaviours — `validate_disabled_features` (four distinct refusals, each upstream's sentence verbatim, including the dedicated `"schedules"` message pointing at `config.scheduledRuns.enabled`), `resolve_disabled_feature_surface`, `disabled_feature_use_error` and `disabled_feature_notice` — plus the `disabled_features` config field, its validator call in `validate_raw_config`, and the `disabled_feature_surface()` accessor. All 15 groups are VERBATIM, in upstream's declaration order, verified mechanically by diffing the parsed upstream table against the parsed port (every group, every action, every param, order included). **Two cites in this row are wrong and are superseded:** `config.ts:182` is `validateAuthorityPolicy` — `validateDisabledFeatures` is at **`config.ts:185`** (an off-by-three, not a near-miss); and `registration/mod.rs:927-945` is inside `invalid_config_disposition`, while `config_warnings` was at **`:1004`** pre-port with its unknown-key `format!` at **`:1021`** (the row says `:944`). The stderr print is `crates/cyrup/src/subagent_config.rs:154-156`, not `:75-77` (which is `impl std::error::Error`). **Key-census end state, the row's actual defect:** `config_warnings` on a declared `disabledFeatures` is now EMPTY — the key is genuine config, not an unknown key wearing the generic typo wording. It is deliberately NOT added to `UNPORTED_CONFIG_KEYS` (asserted negatively), and `FAIL_CLOSED_CONFIG_KEYS`'s pre-port entry now has teeth: a bad value refuses the whole file instead of handing the operator back the full tool. **Two fidelity traps, both pinned:** (i) `preflight` is a SHARED param, owned by the `preflight` group AND `workflow-scripts`, and upstream attributes it to `workflow-scripts` whatever order the config lists them in (`:88-90`) — wrong only when both are disabled, which no casual test covers; (ii) MAP ORDER IS OBSERVABLE — `disabled_feature_use_error` returns the FIRST disabled param, iterating JS `Map` insertion order, which follows the operator's config array order with `schedules` appended last, so a `HashMap` destroys it and a `BTreeMap` silently reorders it alphabetically. Ported as an insertion-ordered pair vector whose `set` reproduces `Map.prototype.set` (a re-set key updates IN PLACE without moving). Both held by tests that go red under an independent `features.reverse()` mutation (five tests, incl. the two order tests), not only under the lane's own. **Three groups name no surface this port advertises** — `preflight`, `gates`, `extension-bindings`: each has an empty upstream `actions` list and its one param is not in cyrup's schema, so disabling one is a no-op for a caller following cyrup's advertised schema. That is correct, not a bug, and is documented in the module so a later pass does not 'fix' it into a refusal; validation still accepts all 15 names so a config stays portable from pi. Every action the 15 groups and the schedule surface name IS a verb this port dispatches (59 verbs, zero misses), pinned by `every_group_action_is_a_verb_this_port_dispatches`. Gates: `-p` clippy clean on a real `Checking` run; crate suite 4999 → **5016**; workspace clippy clean and workspace suite **13930 → 13948 passed, 0 failed** (+18: 15 + 2 + 1). Unblocked `SUBA-151`'s option (c) via `SubagentFeature::WorkflowScripts`, `DisabledFeatureSurface::contains` and `disabled_feature_surface()`. **One error shipped in this work and was fixed by the `SUBA-151` lane:** the new module's doc listed nine advertised params as unadvertised (`args`, `missionStatus`, `missionId`, `runMode`, `runStatus`, `summary`, `laneId`, `supersession`, `planId`) — all nine ARE advertised, as `props.insert("<name>".to_string(), …)`. Measured mechanically: of the 32 params the 15 groups own, **27 are advertised and 5 are not** (`extensionBindings`, `gate`, `globalConcurrencyLimit`, `maxSubagentSpawnsPerRun`, `preflight`). The prose is replaced by a test. |
| ~~SUBA-153~~ | ~~low~~ **CLOSED 2026-10-09** | upstream-drift | M | **CLOSED 2026-10-09** (with `SUBA-139`): `config.toolActivation` is `SubagentExtensionConfig::tool_activation: Option<ToolActivationMode>` (`auto`/`dynamic`/`eager`, absent = `auto`), validated on the raw JSON with upstream's sentence between `artifactDir` and `missions` (pi `extension/config.ts:178-180` @v0.76.1) and already fail-closed. `eager` registers no loader; `dynamic` always selects it; recorded sessions replay their declared selection and `auto` never adds the loader to a transcript that did not declare it; legacy history keeps `subagent`; a model switch changes nothing. Host and provider additions: `HostServices::current_model_info()` (pi `ctx.model`, answered by `LiveHostServices` from the model `update_model` pushes) and `ModelCompat::supports_additional_tools` (catalog key now kept; the two stale "no catalog carries it" doc comments are fixed). `addsToolsWithoutCheckpoint` is ported verbatim as `compat_adds_tools_without_checkpoint`. **CYRUP-DELTA (decided 2026-10-09):** `auto` also requires `cyrup_provider::api::emits_native_tool_additions(api)`, which is `false` for every api until PROV-133 / "PROV-083b" give an adapter a native mid-conversation tool emitter. Every cyrup adapter rebuilds the request tool list today, so enabling `subagent` mid-conversation would miss the prompt cache on every api, and a verbatim `auto` would pay that on exactly the models upstream protects. Until then `auto` behaves as `eager` for a fresh session on every model. The gate flips per api with the emitter, and `api::tests::no_adapter_emits_native_tool_additions_yet` must change with it. Verify: `extension::tool_activation::tests::upstreams_predicate_matches_its_fourteen_row_table` (upstream's 14 rows, predicate alone), `::the_cyrup_gate_refuses_every_row_until_an_adapter_emits_native_tool_additions`, `::auto_starts_every_fresh_session_eager_while_no_adapter_emits_tool_additions`, `::auto_replays_recorded_sessions_without_adding_or_removing_tools`, `::dynamic_always_offers_the_loader_even_to_a_recorded_session_without_it`, `::auto_with_an_incapable_model_starts_eager_and_a_model_switch_changes_nothing`, `::the_loader_is_registered_except_under_eager_and_in_a_child`, `::tool_activation_config_validates_like_upstream`, `subagents::tool_activation_integration::auto_on_a_capable_anthropic_model_stays_eager_until_the_adapter_emits_tool_additions` (cyrup-it), `host_services::tests::current_model_info_reports_the_full_model_and_follows_update_model` (cyrup-session-svc), `api::compat::tests::supports_additional_tools_round_trips_and_stays_absent_when_unset`, `providers::openai_codex::tests::codex_spark_is_the_odd_row_out` (cyrup-provider). — **`SUBA-139`'s port target moved: the loader now has three `config.toolActivation` modes and a per-API cache-miss gate** — `auto`/`dynamic`/`eager` plus `addsToolsWithoutCheckpoint` (`src/extension/tool-activation.ts:36,85` @v0.74.0, `src/shared/types.ts:2595,2672`); cyrup has neither the loader (`SUBA-139`, open) nor the key. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** the statement that the compat flags the gate reads "do exist port-side ... plus `supports_tool_search`/`supports_additional_tools`" is wrong for the second flag. `ModelCompat` (`crates/cyrup-provider/src/api/compat.rs`) has `supports_mid_convo_system_messages` (`:456`), `supports_mid_convo_tool_additions` (`:466`), `supports_mid_convo_tool_changes` (`:520`) and `supports_tool_search` (`:569`), but no `supports_additional_tools` field: a grep of `crates/**/*.rs` for `supports_additional_tools` finds nothing and for `supportsAdditionalTools` finds only a test comment (`providers/openai_codex.rs:779`). `supportsAdditionalTools` appears in the catalog JSON (`opencode-go.json`, `openai-codex.json`, `github-copilot.json`), and `ModelCompat` sets no `deny_unknown_fields`, so the catalog key is ignored. The Responses-API arm of `addsToolsWithoutCheckpoint` (`tool-activation.ts:36-53` @v0.74.0) therefore needs a new compat field and catalog plumbing first; "the predicate is portable today" holds only for the `anthropic-messages` and `openai-completions` arms. |
| ~~SUBA-154~~ | ~~medium~~ **CLOSED 2026-10-04** | upstream-drift | S | **CLOSED 2026-10-04**: `spawn/worktree.rs`'s `MACHINE_DIFF_OPTIONS` and `MACHINE_PATCH_OPTIONS` both spell upstream's seven-element list with `--src-prefix=a/ --dst-prefix=b/` in place of `--default-prefix`, so a capture no longer needs git ≥ 2.43 and a diff that fails no longer costs the child's patch outright; the doc paragraph says why and the citations are retagged `@v0.74.0`. Verify: `machine_diff_argv_matches_upstream_and_carries_no_default_prefix`, `the_two_machine_option_lists_move_together`, `a_hostile_prefix_configuration_still_yields_a_p1_applicable_patch` (captures through the real `diff_worktrees`, then `git apply -p1`s into a fresh checkout and compares bytes). **Ledger correction:** `1cf63c18` is `v0.74.0~36`; v0.73.1 was the last release still carrying `--default-prefix`, so the provenance reads as the swap landing *at* v0.74.0. — *Original:* **`--default-prefix` in cyrup's worktree diff/patch options needs git ≥ 2.43, and a failed capture destroys the child's work** — upstream replaced it with `--src-prefix=a/ --dst-prefix=b/` (`src/runs/shared/worktree.ts:23` @v0.74.0, `1cf63c18`/#2527); cyrup still passes it at `spawn/worktree.rs:79,95`. **FILED 2026-10-02**; body below. |
| ~~SUBA-155~~ | ~~medium~~ **CLOSED 2026-10-04** | upstream-drift | M | **`modelScope` is still the three-key v0.33 shape: no `agents.<name>` rules, no `inherit`, no `scoped`** — so an operator copying pi's documented `allow: ["inherit"]` under `enforce` has every model rejected (`src/runs/shared/model-scope.ts:51,53,93` and `resolveModelScopesForAgent` @v0.74.0; cyrup `exec/model_scope.rs:40`). **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** line cite: `ModelScopeConfig` is at `exec/model_scope.rs:43` (the row's `:40` is its doc comment); it has the three fields `enforce`, `strict`, `allow` and no `inherit` or `agents`. Read in addition: `parse_model_scope_config` (`exec/model_scope.rs`, from `:274`) reads only `enforce`, `strict` and `allow` and emits no warning for another subkey, which supports the "silently dropped" wording for `modelScope.agents`; a separate key census was not checked. — **CLOSED 2026-10-04** (`1409cede`, completing `81200403`). **EVIDENCE CORRECTED — this row's `CORRECTED 2026-10-03` note is itself stale:** `81200403` (on `main`) added `ModelScopeRule`, `ModelScopeConfig.agents` (four fields, not three, at `exec/model_scope.rs:66`), `resolve_model_scopes_for_agent`, `expand_reserved_patterns`, the 8-pattern render cap and the `agents` parser arm — verified absent in `81200403^` and present in `81200403`. What that commit left were two behaviours unreachable from any launch path, and those are what `1409cede` closes. **(a)** `scoped` never resolved to anything: both production call sites passed `scoped_model_ids: None` (`exec/mod.rs:1048`, `fallback.rs:442`), citing a `RunOptions` field that does not exist, so `allow: ["scoped"]` could only degrade to `inherit` — admitting the parent's one current model and refusing every other model the operator deliberately kept in scope. Now a two-phase expansion: `ModelScopeConfig::with_scoped_snapshot` substitutes the snapshot (read once per launch from `HostServices::scoped_models()`) into the global and every per-agent `allow`, then `expand_reserved_patterns` resolves `inherit`. Because the snapshot lands inside the `model_scope` that `RunnerConfig` already serializes, a background run enforces the set its parent held at launch with no new field to drift. **(b)** An armed reserved token with no parent model failed **OPEN**. `unresolved_enforced_reserved_scope_message` was defined at `model_scope.rs:347` but `#[cfg(test)]` begins at `:693` and all four callers sat after it — ported, tested, never wired. With no parent session model the only check that ran was warn-severity, so the launch proceeded on the persona's model with a log line where pi refuses (`model-resolution.ts:347`). `resolve_model_inheritance` now consults the gate first, returning `ModelScopeRefusal::{OutOfScope, UnresolvableReservedToken}`. **Two further corrections.** The Fix's "one non-trivial piece" named the wrong seam: `cyrup-tui/src/app/selectors.rs:225` is the TUI's own selector state, not a seam a subagent launch can reach; the seam is `HostServices::scoped_models()` (`cyrup-ext/src/host/services.rs:1091`, already filled by the live backend per EXT-045), and it is ~50 lines with no struct change. And the row understated the fail-open: it treats fail-closed as a property of `expand_reserved_patterns`, where upstream's guarantee actually lives in the uncalled refusal — that belonged in Impact as a third concrete failure. Cite drift at the pin: `resolveModelScopesForAgent` is `model-scope.ts:161-188` and `expandReservedPatterns` `:107-119` (the row's `:160` and `:105-114` are v0.74.0's). Pinned by `the_documented_inherit_policy_admits_the_parent_model_and_still_refuses_another`, `an_armed_reserved_token_with_no_parent_session_refuses_the_launch_instead_of_warning`, `a_per_agent_rule_refuses_at_launch_a_model_the_global_block_admits`, `the_launch_time_policy_captures_this_sessions_scoped_model_snapshot` and three more, each proven red by reverting production code. |
| ~~SUBA-156~~ | ~~medium~~ **CLOSED 2026-10-04** | parity-bug | S | **CLOSED 2026-10-04** (`ae115d74`, one commit with `MCP-594` as both rows require): `exec/mcp_direct_tools.rs`'s `ServerEntry` carries `inheritEnv` and `literalEnv`, its pre-image gates `interpolate_env_record` on `literal_env` exactly as the writer does, both sides compute `literal_env` with the identical expression and push the same two members under the same `is_stdio` condition, and `exec/mcp_config_sources.rs::translate_plugin_stdio_server` injects `literal_env: Some(true)` as the adapter's own loader does. The agreement tests assert the reader's pre-image *string* equals the writer's, not merely the digest. Verify: `pre_image_matches_the_upstream_generated_golden_vector`, `the_two_stdio_identity_keys_agree_reader_writer_and_upstream`, `reader_and_writer_agree_on_the_fifteen_field_pre_image`, `reader_writer_and_upstream_agree_across_the_edge_cases`, `the_plugin_translation_agrees_with_the_adapters_own_loader`. — *Original:* **The MCP direct-tool reader models no `literalEnv`, so every agent-plugin MCP server's identity hash disagrees with the writer's and its `mcp:` selectors silently resolve to nothing** — upstream put `literalEnv`/`inheritEnv` into the identity at `src/runs/shared/mcp-direct-tool-allowlist.ts:470` @v0.74.0 (`847ee4de`/#2539); cyrup's reader is `exec/mcp_direct_tools.rs:1012`, its writer `cyrup-mcp/src/dirs.rs:1294`. **FILED 2026-10-02**; body below. |
| ~~SUBA-157~~ | ~~low~~ **CLOSED 2026-10-08** | upstream-drift | S | **`subagents.agentOverrides.<name>.advertise` is unported** — v0.74.0 lets settings opt a builtin or custom agent into the parent-prompt catalog without editing its file (`src/agents/agents.ts:88,141`, `8dc90dca`/#2534); cyrup has `AgentDefinition::advertise` (`discovery/types.rs:1352`, `SUBA-133`) but `AgentOverrideConfig` (`discovery/types.rs:686`) has no such field. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** line cite: `AgentOverrideConfig` is at `discovery/types.rs:673` (the row's `:686` is inside the struct); `advertise` exists only on `AgentDefinition` (`:1352`). **CLOSED 2026-10-08** (on `claude/subagents-robustness-sweep`, checked against pi-subagents ad11b7ab): `AgentOverrideConfig::advertise` is an `OverrideField<bool>` beside `description` (`discovery/types.rs:697`, cited to `src/agents/agents.ts:91` @ad11b7ab); a non-boolean is refused before serde with upstream's text, `Builtin override '<name>' has invalid 'advertise'; expected a boolean.` (`discovery/mod.rs:1273`, port of `agents.ts:1047-1050`; the file half comes from the reader's `settings file '<path>': ` prefix); `apply_builtin_override` applies it for builtin and custom agents alike (`discovery/merge.rs:792`, port of `:1500`) and records `advertise` in `fields`. Runtime agents are untouched: `runtime_agent_overrides` still narrows to model/defaultProvider/fast/thinking (`agents.ts:1637-1647`) and the catalog builder still drops `AgentSource::Runtime` (`advertised-agent-prompt.ts:43`). The v0.68.0 census test now pins 27 upstream keys. Tests: `discovery::merge::tests::advertise_override_opts_a_builtin_in_and_a_custom_agent_out`, `::an_advertise_override_does_not_reach_a_runtime_agent`, `discovery::tests::override_advertise_is_read_and_a_non_boolean_is_refused_with_pis_text`. |
| ~~SUBA-158~~ | ~~medium~~ **CLOSED 2026-10-04** | not-ported | S | **A spawned subagent child does not follow the parent session's project trust**, so a child in a session-trusted project loads it as untrusted and drops its project-level config — upstream threads `projectTrusted` into the child's settings manager (`src/runs/shared/child-session.ts:59,373` @v0.74.0, `b2718fb8`/#2570); cyrup's child argv carries no trust (`exec/spawn_plan.rs:317`) and a fresh `cyrup` boots `project_trusted: false` (`crates/cyrup/src/bootstrap.rs:93`). **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** the cyrup evidence is mis-aimed. `crates/cyrup/src/bootstrap.rs:93` (`load_startup_settings`) is the process's startup settings manager, used for settings diagnostics and the `sessionDir` lookup; it does not decide the project trust a session runs under. Real trust resolution is `SessionBuilder` in `crates/cyrup-session-svc/src/builder.rs:801-853`: settings loaded with the project untrusted, `default_project_trust`, `has_trust_requiring_resources`, the trust-store `nearest` lookup, then `TrustInputs { trust_override, saved, default_trust, mode, .. }` into `decide_trust_with_extension`. `cyrup-ext-subagents` passes no `--approve`/`--no-approve` and no trust env to the child (no hits in the crate outside unrelated tests), so a child resolves trust itself from the trust store and `defaultProjectTrust`. Direction caveat: upstream's v0.74.0 changelog describes the opposite symptom ("a child in an untrusted project still loaded that project's settings"), so the row's "child less trusting than the parent after an in-session grant" is the mirror case of the same missing hand-off. Neither was reproduced end to end; the defect is plausible but unproven. Recommended re-rating (not applied): medium to low until a spawned child is shown to resolve a different trust than its parent. — **CLOSED 2026-10-04**: A child follows the launching session's project trust through the `PARENT_PROJECT_TRUSTED_ENV` ladder, which prefers explicit then inherited then nothing. Pinned by `the_parent_trust_ladder_prefers_explicit_then_inherited_then_nothing` and `an_untrusted_parent_hands_its_child_the_no_approve_flag`. |
| ~~SUBA-159~~ | ~~low~~ **CLOSED 2026-10-08** | upstream-drift | S | **CLOSED 2026-10-08** (on `claude/subagents-robustness-sweep`, checked against pi-subagents ad11b7ab): the runner stamps `RunStatus.pid_namespace_scope` (`pidNamespaceScope`, `background/records.rs:391`) from a cached `readlink("/proc/self/ns/pid")` (`background/reconcile.rs:165` `current_pid_namespace_scope`; pi `runs/background/pid-namespace.ts:6-18`) beside its own pid at both status sites (`background/runner_main/entry.rs:392,655`). `reconcile` takes the observer's scope and downgrades a `Dead` probe to `Unknown` when a recorded scope differs from it, including when the observer has none (`reconcile.rs:453`; pi `stale-run-reconciler.ts:446-452`), so a cross-namespace `ESRCH` waits out the stale window. The stale sentence is upstream's current one, `Async runner PID <pid> is still live` or `... cannot be probed from this process; status has not updated for <n>ms, so stale-run reconciliation marked the run failed because PID ownership is unverified.` (`reconcile.rs:480-494`; pi `:457-458`). The #2606 fold-in is ported too: `check_pid_liveness_probing_zombie` (`reconcile.rs:197`; pi `checkPidLiveness`, `:360-379`) reads `/proc/<pid>/stat` after a successful `kill(pid, 0)` and reports state `Z` as `Dead`, asked for only when the recorded scope equals the observer's; `reconcile_now` uses it. Verify: `background::reconcile::tests::{a_cross_namespace_esrch_is_not_death_inside_the_stale_window, a_cross_namespace_run_fails_only_once_stale_and_says_it_could_not_be_probed, a_matching_or_absent_scope_still_fails_a_dead_pid_at_once, the_stat_state_field_is_read_after_the_last_close_paren, the_runner_scope_is_persisted_as_pid_namespace_scope, this_process_reads_its_own_pid_namespace, an_unreaped_zombie_is_dead_only_when_the_zombie_probe_is_asked_for}`; the first three fail on the old code. **Not in this row (recorded, not claimed):** upstream applies the same scope gate in two more readers: the active-async capacity release (`active-async-capacity.ts:256`), whose cyrup counterpart still probes with the bare `check_pid_liveness` (`background/active_async_capacity/config.rs:151`), and `runnerExitedWithoutResult` (`await-async-run.ts:18-26`), whose cyrup counterpart was not checked. *Earlier text:* **Runner liveness probes are not scoped to the PID namespace, so an observer in another namespace reads `ESRCH` and fails a live run** — upstream records `pidNamespaceScope` and downgrades a cross-namespace `dead` to `unknown` (`src/runs/background/pid-namespace.ts:7`, `src/runs/background/stale-run-reconciler.ts:439,441` @v0.74.0); cyrup's probe is a bare `kill(pid, 0)` and `Dead` fails immediately (`background/reconcile.rs:144,383`). **FILED 2026-10-02**; body below. **FOLD-IN 2026-10-03 (v0.75.0, `806e3678`/#2606, Linux zombie runners):** `checkPidLiveness(pid, kill, probeZombie)` (`src/runs/background/stale-run-reconciler.ts:360-370` @v0.75.0) now reads `/proc/<pid>/stat` after a successful `kill(pid, 0)` and returns `dead` when the state field is `Z`. `probeZombie` is true only when the status's recorded `pidNamespaceScope` equals the observer's (`stale-run-reconciler.ts:446-451`; the same gate in `await-async-run.ts:15-24`). Cyrup's `check_pid_liveness` (`background/reconcile.rs:144`) is a bare `kill(pid, 0)` and reports a zombie as `Alive`. `background/spawn_detached.rs:391` drops the runner's `Child` (`None => drop(child)`), so a runner that outlives its launcher is reparented, and where PID 1 does not reap (a container without an init) it stays a zombie that reads as alive; that is the triager's reading, not run, and it needs no cross-namespace mount. Scope widens: port the `/proc/<pid>/stat` zombie probe together with `pid_namespace_scope`. Recommended re-rating (not applied): low to medium if the init-less-container zombie is judged reachable. |
| ~~SUBA-160~~ | ~~low~~ **CLOSED 2026-10-08** | upstream-drift | S | **CLOSED 2026-10-08** (on `claude/subagents-robustness-sweep`, checked against pi-subagents ad11b7ab): `timer_delay_overflow_error` (`extension/tool/params.rs:565`) ports pi `timerDelayOverflowError` (`runs/foreground/subagent-executor.ts:2999-3003`) and reuses `exec::tool_timeout`'s cap and message builder (`invalid_tool_timeout_message`, now `pub(crate)`, `exec/tool_timeout.rs:222`). `resolve_foreground_timeout` refuses a call-site `timeoutMs`/`maxRuntimeMs` above 2147483647 per name, before the alias clash (`params.rs:592`; pi `:3028-3029`); single, parallel, chain and workflow launches all resolve their call-site deadline there, which covers upstream's workflow-launch check (`:5459-5463`) too. An agent's frontmatter `timeoutMs` is a separate rung: upstream's `applySingleAgentLaunchDefaults` (`:2938-2958`, applied at `:7423`) copies it into `params.timeoutMs` only for a SINGLE launch (it returns early on `chain`/`tasks`) when neither alias is set, so the same overflow check refuses it as `timeoutMs`. Cyrup keeps that rung out of the params, so the two single-launch surfaces check it themselves: `route_single` (`extension/tool/routing.rs:702-714`) and `/run` (`extension/host/slash.rs:478-489`, after the depth guard and before the spawn charge, as upstream's `:7371`/`:7558`/`:7719`). Parallel and chain launches get no such check upstream either: their per-agent default reaches the async child builder unchecked (`runs/background/async-execution.ts:1304`). The `action: "resume"` arm refuses an oversized `timeoutMs` first, before its acceptance check, leaving a call with neither message nor chain to `control_resume`'s `requires message` refusal (`routing.rs:2966-2991`; pi `:1917-1940`, order requires-message, overflow, model, acceptance). The Fix's pointer to `extension/executor/background.rs` was stale: the resume dispatch is in `routing.rs`, and `checkpointBeforeDeadlineMs` is checked at the tool entry (`extension/tool/mod.rs:358`). Verify: `extension::tool::params::tests::an_explicit_timeout_above_the_timer_delay_cap_is_refused`, `extension::tool::routing::tests::{resume_refuses_a_timeout_above_the_timer_delay_cap, an_agent_frontmatter_timeout_above_the_timer_delay_cap_refuses_a_single_launch}`, `extension::host::tests::run_refuses_an_agent_frontmatter_timeout_above_the_timer_delay_cap`; all fail on the old code. **Not in this row (recorded, not claimed):** upstream's workflow children re-enter `execute` as single launches (`prepareWorkflowChildLaunchParams`, `:5088-5106`), so an agent's `timeoutMs` applies to them and an oversized one refuses the child; cyrup's workflow child launch does not apply the agent's `timeoutMs` at all (`extension/executor/workflow.rs:1003`, `child.timeout_ms.or(child.max_runtime_ms)`), so there is nothing there to cap until that rung is ported. *Earlier text:* **`timeoutMs`/`maxRuntimeMs` accept any `u64` although `toolTimeoutMs` and `checkpointBeforeDeadlineMs` are capped at `MAX_TIMER_DELAY_MS`** — upstream now rejects an oversized value on launch and on `action: "resume"` (`src/runs/foreground/subagent-executor.ts:2948,2950` @v0.74.0, `5655f9bb`/#2517); cyrup's `resolve_foreground_timeout` (`extension/tool/params.rs:566`) checks only `0` and the alias clash. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** the Impact paragraph's claim that "a Rust/tokio deadline built from a huge `u64` saturates" is unverified; `Instant + Duration` arithmetic can panic on overflow, so the real effect of an oversized value may be a panic rather than "no deadline". The defect and the cites (`params.rs:566-588` checks only zero and the alias clash) stand. |
| SUBA-161 | low | not-ported | S | **The `council` guide topic and the multi-file guide body are unported** — v0.74.0 adds an eleventh topic that concatenates three bundled files behind `<!-- path -->` markers so `/council` survives `--no-skills` (`src/extension/subagent-guide.ts:16,25,39`, `a0fd73df`/#2469); cyrup's `SUBAGENT_GUIDE_TOPICS` is ten (`registration/guide.rs:44`) and `council` has zero hits in `crates/` or the ledger. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** effort S is optimistic (S to M): a working `council` topic needs the three skill files bundled (`SUBA-161`'s own Fix says whether they ship is a separate decision), and v0.75.0 changes eight guide docs (`docs/agents.md`, `configuration.md`, `extension-api.md`, `missions.md`, `models.md`, `observability.md`, `tool-reference.md`, `workflows.md`). Recommended re-rating (not applied): effort S to M. |
| ~~SUBA-162~~ | ~~low~~ **CLOSED 2026-10-10** | not-ported | M | **CLOSED 2026-10-10** (checked against pi-subagents ad11b7ab): the progressive tier is ported into `tui/render.rs` as pure functions with the layout session threaded as a `&mut Option<WidgetLayoutSession>` parameter, so the module keeps no state. `running_leaf_agent_count` + `progressive_header_line` port `render.ts:2634-2666` (`7e07a22d`/#2584): a workflow counts its loaded children recursively through `WidgetJobTree` (`widgetJobTree`, `:2869-2884`), any other running job counts its running/pending steps, synthesised from `AsyncJobSnapshot::agents` when it has none, with a sequential chain's non-current pending steps excluded. `fit_adaptive_widget_lines` / `build_progressive_widget_lines` port `:2706-2820`: lock to the rows content fills, grow up to `collapsed_widget_line_budget` when a job would be hidden, shrink only when root jobs leave (`8d804895`/#2662, which superseded the row's "never shrinks"), and fill spare rows with workflow lane rows (`9a5a2d5e`/#2583). `render_async_jobs_widget(jobs, &AsyncWidgetRender, &mut session, tick)` (`tui/events.rs`) drops the session on an empty list (`:3093`); the extension owns it (`extension/host/mod.rs` `async_widget_session`) and `slash.rs` clears it on both empty edges. Fleet side (`tui/fleet_status.rs`; the row's `tui/fleet_view.rs` does not exist): a `Workflow` tracked run now yields one `workflow_wrapper` entry (`fleet-status.ts:439-464`) and the collapsed label counts `active_leaf_agent_count` (`:346-348`), wrappers excluded from count and tokens. FOLD-IN: `asyncWidgetCollapsed` and `asyncWidgetLayout` (`588d2cfd`/#2738, folded by the PIN note) are raw config keys validated with upstream's sentences between `artifactDir` and `toolActivation` (`extension/config.ts:187-191`), resolved by `async_widget_collapsed()` / `async_widget_layout()`, and drive the published widget. **Deviations:** the header-click fold (#2235) is not ported: extension widgets get no pointer events (`cyrup-tui/src/app/pointer.rs` module doc) and `set_widget` carries lines only, so the folded state is the config value for the widget's life — split to **`SUBA-210`**. The extension has no terminal-size accessor, so pi's `|| 30` / `|| 120` fallbacks are passed and the default `adaptive` layout never leaves the full tier (cyrup's full block is at most 5 lines); the progressive card is reached with `asyncWidgetLayout: "rows"` — split with the empty `agents` roster to **`SUBA-211`**. `parent_workflow_run_id` and `workflow_lanes` have no async producer (the runner never emits `RunMode::Workflow`), so in production every job is a root and no lane row renders; `fitWidgetLineBudget`, the `stageProgress` exception, lane signals/stats and inline-fleet coverage are not ported. Verify: `a_four_lane_workflow_beside_one_run_reads_the_same_count_in_fleet_and_widget`, `widget_header_and_fleet_report_the_same_agent_count`, `a_workflow_counts_its_loaded_children_not_itself`, `a_sequential_chain_counts_one_in_fleet_and_widget`, `a_sequential_chain_counts_only_its_current_step`, `a_job_with_no_step_detail_counts_its_agents`, `the_locked_card_keeps_its_height_as_jobs_finish`, `the_locked_card_grows_up_to_the_cap_when_a_job_starts_while_jobs_are_hidden`, `the_progressive_card_locks_to_its_content_rows`, `the_card_shrinks_when_root_jobs_leave`, `workflow_lanes_fill_spare_rows_and_the_lock_holds_across_content_only_updates`, `an_empty_job_list_resets_the_session`, `the_adaptive_layout_keeps_the_full_tier_when_it_fits`, `the_collapsed_widget_is_one_line_over_every_job`, `async_widget_collapsed_and_layout_are_parsed_validated_and_censused`, `async_widget_collapsed_publishes_the_one_line_card`, `async_widget_rows_layout_publishes_the_progressive_header`. — *Original:* **The progressive async-widget tier is unported: no height lock, no workflow lane rows, and no running-agent header count** — upstream's widget locks the card to the rows its content fills and counts leaf agents the way Fleet does (`src/tui/render.ts:2635,2648,2704` @v0.74.0, `9a5a2d5e`/#2583 and `7e07a22d`/#2584); cyrup renders one full block per run with no header line (`tui/render.rs:386`, `tui/events.rs:874`). **FILED 2026-10-02**; body below. **FOLD-IN 2026-10-03 (v0.75.0, `53aee6d8`/#2621):** also unported are upstream's header-click fold (`93d47c0c`/#2235, v0.68.0; `buildWidgetComponent`, `src/tui/render.ts:2903` @v0.75.0) and v0.75.0's boolean `asyncWidgetCollapsed` (validated `src/extension/config.ts:175-176`, passed as `initiallyCollapsed` at `render.ts:3099`). `grep -rn 'asyncWidgetCollapsed' crates/` is empty, so a user who sets it gets only the generic unknown-key stderr warning. Not verified: whether the cyrup TUI can deliver clicks to an extension widget, which decides whether the click half is portable. |
| ~~SUBA-163~~ | ~~low~~ **CLOSED 2026-10-08** | stale-port | S | **`/subagent-cost` reports no child usage for an async single, chain or parallel launch** — `SUBA-138` ported v0.71.0's collector, and `d556bb01`/#2490 (v0.72.0) then added the `asyncId`-with-empty-`results` and `completions` arms (`src/slash/subagent-cost.ts:163,164` @v0.74.0); cyrup's collector tracks only `workflow_run_ids` (`registration/cost.rs:1002`). **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03 (stale by v0.75.0):** the port target moved again; `ae9c9d77`/#2612 and `4998ceca`/#2615 change the same function (`git diff v0.74.0 v0.75.0 -- src/slash/subagent-cost.ts`). (a) A workflow id joins `workflowRunIds` only when `details.mode === "workflow" && details.runId && details.asyncId`: foreground workflow child usage is already in `results`, and only async workflows persist a receipt. (b) The child `runId` is read from the typed `result.runId`, with no cast. (c) Every round of a resumed foreground workflow child counts (#2612). (d) A missing or unreadable receipt now adds to `unresolvedAsyncChildren` (an `ENOENT`, also via `error.cause`, is not logged; other errors are), so an async workflow with no receipt yet is listed as `Async child usage unavailable` (#2615). Cyrup diverges on both ends (read, not run): `collect_subagent_cost` (`registration/cost.rs:995`) adds a workflow id on `mode == "workflow"` plus `runId` (`:1014-1020`), but cyrup's foreground workflow details carry `mode`, `children`, `workflowRunId` and no `results`, `runId` or `asyncId` (`extension/executor/workflow_launch.rs:456-470`, `:697`), so foreground workflow child usage is never counted; and a `NotFound` receipt is a silent `continue` (`cost.rs:1102`) instead of an unresolved count. Scope of the fix therefore widens beyond the `asyncId`/`completions` arms: add the arm, gate the workflow arm on `asyncId`, count foreground workflow `children` usage, and count unresolved on a missing receipt. **CLOSED 2026-10-08** (on `claude/subagents-robustness-sweep`, checked against pi-subagents ad11b7ab, where `collectSubagentCost` is `src/slash/subagent-cost.ts:153-288`): `collect_subagent_cost` (`crates/cyrup-ext-subagents/src/registration/cost.rs:1074`) now keeps upstream's three id sets. (1) An `asyncId` with empty `results` joins `async_run_ids` (`:1110-1124`; pi `:193`), resolved through the run's `status.json` steps and each step's artifact `_meta.json` (`:1300`; pi `:259-283`): a one-step run reads unindexed then `_0`, under `run:<id>`; a multi-step run reads `_<flatIndex>` under `run:<id>:<index>`. A missing status or empty steps is unresolved, and so is a settled step with no metadata; a pending or running step is not. `metadata_usage` takes upstream's `indexes` (`:943`, default `DEFAULT_METADATA_INDEXES` `:988`), and `add_child` takes its `identity` override (`:1004`). (2) Every `completions[].runId` joins `completed_run_ids`, and the async arm skips it, so a `bg_wait` completion does not double-count (pi `:197-199,261`). (3) The workflow arm is gated on `asyncId` (pi `:191`). A `NotFound`/`MayStillBeActive` receipt, an unreadable one, or an id that is not a run dir name now adds to `unresolved_async_children`, and only the non-missing errors are logged (`:1193`; pi `:250-256`). (4) **[CYRUP-DELTA, representation]** Foreground workflow usage: upstream flattens `workflowDetailsResults` into `details.results` (`src/runs/foreground/subagent-executor.ts:4758-4764`, at `:6625`/`:6657`). Cyrup keeps each child's `results` on `details.children`, so `subagent_details` also accepts `mode: "workflow"` with `children` (`:826`), and `foreground_workflow_results` (`:849`) does the same flatten: each round keeps its own run id (#2612) and otherwise falls back to the child's. The failure arm now carries `mode` + `children` too (`extension/executor/workflow_launch.rs:580`; pi `:6657`); before this it was `{}`, so a failed workflow's children were invisible. Tests: `an_async_single_launch_contributes_its_child_usage`, `an_async_chain_and_parallel_launch_each_contribute_every_steps_usage`, `a_bg_wait_completion_does_not_double_count_an_async_launch`, `a_foreground_workflow_run_id_is_not_resolved_through_a_receipt`, `an_async_workflow_without_a_receipt_is_listed_as_unavailable`, `a_foreground_workflow_counts_every_childs_usage`, and `extension::tool::routing::tests::a_failed_foreground_workflow_keeps_its_partial_children_for_subagent_cost`, which drives a real foreground workflow into the failure arm (a script that throws after one `runs.run`) and feeds its returned details to `collect_subagent_cost`; it fails with the failure arm's details reverted to `{}`. The existing `cost_report_resolves_workflow_children_through_receipt_and_metadata` now carries `asyncId`, so "a workflow run still resolves through its receipt" still holds. Residual: cyrup has no async workflow launch (`routing.rs` refuses `async: true` for a workflow), so in production the workflow arm is reached only through a `bg_wait` completion with `mode: "workflow"`. |
| ~~SUBA-164~~ | ~~medium~~ **CLOSED 2026-10-04** | not-ported | M | **`tool_open_threshold` attention is unported, so a child stuck in one long tool call never raises `needs_attention`** — upstream emits `reason: "tool_open_threshold"` for each tool call left open past `activeNoticeAfterMs` (`shouldEmitOpenToolAttention`, `src/runs/shared/subagent-control.ts:114-122`; `src/runs/foreground/execution.ts:915-933`; `src/runs/background/subagent-runner.ts:2709-2781`; reason enum `src/shared/types.ts:387` @v0.75.0); cyrup's `ControlEventReason` has seven variants (`exec/control.rs:301-322`) and `derive_activity_state` returns `None` while a tool is open (`:475`). **FILED 2026-10-03** from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0); body below. — **CLOSED 2026-10-04** (`a26aaa74`). `ControlEventReason::ToolOpenThreshold` + `ControlEvent::tool_call_id`; `should_emit_open_tool_attention` as a pure `(now, open-since, threshold)` decision reusing `is_tool_timeout_exempt`; `ControlMonitor` now tracks the open calls (`ActiveToolCall`, keyed by the existing `tool_timeout_call_key`) with a per-call `attention_emitted`, derives `current_tool`/`current_path` from them, and runs the open-tool branch between the idle and long-running branches as both upstream runners do; `control_notification_key` is keyed per call for this reason, which also makes the TUI notice dedupe per call; `drive_attempt` clears the open set on a terminal assistant stop. One monitor serves both the foreground and detached-background paths, covering both of upstream's producers. The bash nudge wording stays deferred to `SUBA-165`, as this row specified. 10 new tests, none of which sleeps or reads a clock — `now` is an explicit epoch-millis argument throughout, which is what lets the inclusive `>=` boundary be pinned at 999 vs 1000 ms against a 1000 ms threshold; all proven red by surgical reverts, and three PRE-EXISTING tests also go red when the threshold is forced true, which is what shows the branch is wired into the real fold rather than sitting beside it. **Cite corrections:** `shouldEmitOpenToolAttention` is `subagent-control.ts:114-124` — the row's `:114-122` stops one line short of the `>=` comparison at `:123`, i.e. omits the single line that fixes both the clock and the inclusive boundary; foreground `updateActivityState` is `execution.ts:910-936` with the branch at `:924-936`; the background pair is `subagent-runner.ts:2709-2711` + `:2760-2791`, **driven from `:3204`** — that driver line, which the row does not cite, is what establishes the idle → open-tool → long-running ordering and is what anyone verifying "do the two runners agree?" needs. **Upstream's own v0.75.0 CHANGELOG miscredits this fix** to `#2598` (`72682254`, opt-in command supervision); the actual change is `478f871c`/`#2613`, exactly as this row said — verified at the pin, so reconciling against pi's CHANGELOG would turn a correct row into a wrong one. Incidental fix found while porting: `pending_tool_result.path` took the derived `current_path` where upstream uses the call's own `activeTool?.path` (`execution.ts:1061`), so with two calls open a mutating failure could be attributed to the wrong path. |
| SUBA-165 | low | not-ported | L | **Opt-in `subagent_command` supervision (`command.status`, `command.yield`, `command.cancel`, `toolCallId`, bash `yieldTimeMs`) is unported** — upstream wraps a native child's `bash` so the parent can query, yield or cancel one command (`createChildCommandRuntime`, `src/runs/shared/child-commands.ts:49`; `commandAction`, `src/runs/foreground/command-action.ts:24`; actions in `SUBAGENT_ACTIONS` `src/shared/types.ts:2865`; `toolCallId` param `src/extension/schemas.ts:184` @v0.75.0, `72682254`/#2598); cyrup's `SUBAGENT_ACTIONS` (`extension/tool/text.rs:265`) has no `command.*` and no code mentions `yieldTimeMs`. **FILED 2026-10-03** from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0); body below. |
| ~~SUBA-166~~ | ~~medium~~ **CLOSED 2026-10-04** | parity-bug | S | **Any invalid config value makes cyrup load all-defaults with only a stderr warning, dropping `authorityPolicy` and `permissions`** — upstream's `loadConfig` rethrows when the file holds any `FAIL_CLOSED_CONFIG_KEYS` key (`src/extension/config.ts:17,226-240` @v0.75.0, which `9f1c2552`/#2624 widened from 8 keys to 11 by adding `authorityPolicy`, `permissions`, `toolBudget`); cyrup returns defaults on any `validate_raw_config` failure (`crates/cyrup/src/subagent_config.rs:68-73`) and on a typed-parse error (`:91-97`), and has no fail-closed key list at all. **FILED 2026-10-03** from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0); body below. — **CLOSED 2026-10-04** (`f194471b`, completed by `a3786e3e`): `FAIL_CLOSED_CONFIG_KEYS` (11 keys, upstream order) plus `InvalidConfigDisposition::{DefaultWithWarning, Refuse}` consulted on both the raw-validation and typed-parse arms, so a file declaring `authorityPolicy` or `permissions` is refused instead of replaced by the all-defaults config; a file declaring no listed key still warns and defaults. `f194471b` alone left the refusal escaping `attach_native_extensions`, aborting the whole launch where pi discards one extension and starts anyway; `a3786e3e` closes that with `cyrup_ext::QuarantinedNative`, so the cost is exactly the extension upstream loses. Pinned by `a_typo_beside_an_authority_policy_refuses_the_file_instead_of_lifting_the_policy`, `every_fail_closed_config_key_forces_a_refusal`, `a_refused_subagents_config_costs_the_launch_only_that_extension` and `the_attach_gate_is_consulted_before_the_config_is_read`. |
| ~~SUBA-167~~ | ~~low~~ **CLOSED 2026-10-09** | not-ported | M | **CLOSED 2026-10-09** (checked against pi-subagents v0.76.1; `claude-code-adapter.ts` is byte-identical from v0.75.0, and the `agents.ts` exemption is now at `:2070-2080`). `exec/external_cli/adapters/claude_code.rs` ports `resolveClaudeCodeOverride` (`:116-172`) with `CLAUDE_CODE_EFFORT_BY_THINKING`, `CLAUDE_CODE_MODEL_PATTERN` (hand-rolled, no regex dependency), `agentPinnedModel`, `combineClaudeCodeModel`, `assertClaudeCodeModelScope` (`:174-191`) and `assertClaudeCodeOverrideIsLocal` (`async-execution.ts:891-896`), every refusal verbatim; `launch_args` appends the tokens after `--no-chrome` and the `--version`/`--help` probes stay prefix-only. `exec::run_sync` resolves them once, ahead of the runner dispatch (`external_cli::resolve_claude_code_launch_override`), from the new `RunOptions::launch_model` (the caller's or step's model as typed: foreground `req.model_override`, hop-2 `step.model`; never the post-inheritance `model_override`), `agent.thinking` (caller param over persona) and the ceiling folded as Step 2b folds it; any refusal fails the run before a probe or process starts. `subagents.defaultModel` provenance travels as `AgentConfig`/`ResolvedAgentPersona::model_is_settings_default` (serde default, omitted when false, so existing personas and their recovery digests are unchanged; a revive keeps it only while the descriptor's model equals the persona's, matching upstream's `modelSource.model === model`). Both launch sites (foreground `resolve_run_agent`, hop-2 `build_step_agent_config`) skip the parent-session thinking rung for an external runner (`async-execution.ts:1117,1934`), and skip native model resolution (`resolve_model_inheritance`) for one (`primaryModel = externalRunner ? undefined`, `:1091,1905`), so the Claude Code scope check is the only scope check that fires for these agents. `validate_external_runner_profile` exempts `model`/`thinking` for `claude-code`/`claude-code-writer` (`agents.ts:2072-2076`). The result keeps `model: None`, as upstream's external result does (`subagent-runner.ts:977-994`), which answers the row's open question. **Deviations:** (i) the session-thinking and native-model skips key on any external runner (upstream's `externalRunner`), a superset of the Claude Code adapters with no effect on the generic runner, which reads neither; (ii) `[CYRUP-DELTA]` the saved-machine refusal names the resolved machine's display name (label, else id) because `RunOptions` carries no requested selector string; (iii) the preflight reason code `model_scope` (`api/preflight.ts:49,214-218`) is **N/A**: cyrup has no port of the launch-contract `validate` surface (see `09a` `SUBA-101`); (iv) cyrup agents carry no per-agent `maxThinking`, so the ceiling is the settings/inherited one (pre-existing). Verify: `exec::external_cli::adapters::claude_code::tests::{the_override_derives_model_and_effort_from_the_suffix, a_bare_level_runs_on_the_agents_model_and_a_settings_default_never_becomes_model, the_override_rejects_values_it_cannot_pass_as_one_argv_element, the_ceiling_checks_the_requested_level_not_the_effort, an_enforced_scope_refuses_an_unnamed_or_out_of_scope_claude_model, a_claude_code_override_cannot_run_on_a_saved_machine}`, `exec::external_cli::tests::the_override_is_appended_after_the_fixed_argv_and_kept_out_of_preflight`, `exec::tests::{run_sync_passes_a_claude_code_launch_model_and_effort_to_the_cli, run_sync_maps_a_claude_code_agents_thinking_to_effort_and_keeps_a_default_model_out, run_sync_refuses_an_unusable_claude_code_override_before_anything_spawns}`, `background::runner_main::executor::tests::a_claude_code_step_takes_its_own_model_and_never_the_sessions_thinking_or_model`, `extension::executor::foreground::tests::a_claude_code_launch_takes_the_callers_model_and_never_the_sessions_thinking`, `runner::tests::claude_code_adapters_accept_model_and_thinking_frontmatter`, `discovery::frontmatter::tests::a_claude_code_profile_loads_with_model_and_thinking_frontmatter`. — **Claude Code adapters accept no per-launch model or thinking level (`--model`, `--effort`)** — upstream's `resolveClaudeCodeOverride` (`src/runs/shared/claude-code-adapter.ts:132` @v0.75.0, `aafcfb31`/#2603) maps `model: "id:level"` to `--model` and `--effort`, and frontmatter `model`/`thinking` are allowed for these adapters (`src/agents/agents.ts:2091-2093`); cyrup's `launch_args` is fixed (`exec/external_cli/adapters/claude_code.rs:75-101`) and `PI_ONLY_FIELDS` rejects both fields (`runner/mod.rs:190-207`). **FILED 2026-10-03** from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0); body below. |
| SUBA-168 | low | not-ported | M | **`schedule.create` still refuses `missionId`: schema-version-2 mission-bound schedules are unported** — upstream accepts an existing `missionId` at create, writes `schemaVersion: 2` exactly when it is present and omits `mission: false` when bound (`src/runs/background/scheduled-runs.ts:313,326,456-467,476,641` @v0.75.0, `9240e7e2`/#2616); cyrup returns "Mission attachment is deferred" (`background/scheduled_runs/tool.rs:468-478`) and parses only version 1 (`schedule.rs:57,618`). **FILED 2026-10-03** from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0); body below. |
| ~~SUBA-169~~ | ~~low~~ **CLOSED 2026-10-08** | upstream-drift | S | **One unreadable goal mission aborts continuation notices for every healthy goal mission** — upstream wraps each mission in try/catch with an `onError(missionId, err)` callback (`collectGoalContinuationNotices`, `src/missions/goal-driver.ts:127-175` @v0.75.0, `36081c57`/#2604), and `readLinkedRun` names the path of a malformed `status.json` (`:30-40`); cyrup's `collect_goal_continuation_notices` (`missions/goal_driver.rs:486-554`) uses `?` per mission and the caller turns that error into `return 0` (`extension/executor/notices.rs:735-745`). **FILED 2026-10-03** from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0); body below. — **CLOSED 2026-10-08** (on `claude/subagents-robustness-sweep`, checked against pi-subagents ad11b7ab, where the code is unchanged at `goal-driver.ts:127-175`): `collect_goal_continuation_notices` evaluates each goal mission in its own `evaluate_goal_mission` and hands a failure to an `on_error(mission_id, err)` callback (default: a `Failed to evaluate goal mission <id>` warning) before moving on, so the executor no longer drops every notice on one bad mission; `read_linked_run` now names the path in `Failed to read linked run status '<path>': …` and refuses a non-object status as upstream does. Pinned by `a_damaged_goal_mission_is_reported_and_a_healthy_one_still_notifies`, `a_non_object_linked_run_status_is_reported_with_its_path` and `an_unreadable_mission_state_is_scoped_to_its_own_mission`. |
| ~~SUBA-170~~ | ~~low~~ **CLOSED 2026-10-08** | upstream-drift | S | **The global mission list shows the index entry, not the mission record** — upstream's `listGlobalMissions` overlays `title`, `status`, `updatedAt` and `lastRunId` from the readable record (`src/missions/store.ts:554-575` @v0.75.0, `d6726aea`/#2618); cyrup's `list_global_missions` (`missions/store.rs:1372`) parses the record only to set `stale` and returns the index entry unchanged (`:1424-1446`). **FILED 2026-10-03** from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0); body below. — **CLOSED 2026-10-08** (on `claude/subagents-robustness-sweep`, checked against pi-subagents ad11b7ab, where the projection is at `store.ts:563-572`): `list_global_missions` overlays the readable, id-matching record's `title`, `status`, `updated_at` and last run id on the listed entry (no `last_run_id` when the record has no runs), sorts after the overlay, and leaves the pointer file untouched. Pinned by `a_stale_global_pointer_lists_the_records_title_status_and_last_run`, `a_record_with_no_runs_lists_no_last_run_id` and `the_global_list_is_sorted_by_the_records_updated_at`. |
| ~~SUBA-171~~ | ~~low~~ **CLOSED 2026-10-08** | parity-bug | S | **Settings save replaces a symlinked or restricted `settings.json` with a default-mode regular file** — upstream's `writeSettingsFile` (`src/agents/agents.ts:947` @v0.75.0, `63ac9c2a`/#2627) resolves the real target through symlinks (`resolveSettingsWriteTarget`, `:977`), checks `W_OK`, keeps the existing mode and writes via temp and rename; cyrup's `write_settings_file` (`discovery/settings_write.rs:102-111`) calls `cyrup_config::lock::write_atomic(path, bytes, false)` (`lock.rs:337-377`), which creates the temp at the umask default and renames over the path itself. **FILED 2026-10-03** from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0); body below. — **CLOSED 2026-10-08** (on `claude/subagents-robustness-sweep`, checked against pi-subagents ad11b7ab, where the writer is `src/agents/agents.ts:950-978` and the resolver moved to `src/shared/settings-file-lease.ts:6-30`): `discovery/settings_write.rs` ports `resolveSettingsWriteTarget` (realpath, or a dangling link's text when its physical parent exists; a trailing separator is refused) and, like `withSettingsFileLease`, makes the parent, resolves once and takes the lock on the physical file; `write_settings_file` stats the target's `mode & 0o7777`, refuses an existing file it cannot open for writing (the `accessSync(W_OK)` step), and writes through the new `cyrup_config::lock::write_atomic_with_mode` (temp created `mode | 0o200`, set to exactly `mode` before the rename). `write_atomic`'s own defaults are unchanged. Verify: `a_save_through_a_symlink_updates_the_target_and_keeps_the_link`, `a_save_through_a_dangling_symlink_creates_the_link_target`, `a_save_keeps_the_existing_file_mode_exactly`, `a_save_through_a_symlink_keeps_the_targets_mode`, `a_read_only_settings_file_is_refused_and_left_untouched` (skips itself as root, where `W_OK` always passes, upstream included), `the_write_access_probe_refuses_before_any_rename`, and `cyrup-config`'s `lock::tests::write_atomic_with_mode_gives_the_renamed_file_exactly_that_mode` / `write_atomic_defaults_are_unchanged`. |
| ~~SUBA-172~~ | ~~low~~ **CLOSED 2026-10-09** | stale-port | M | **CLOSED 2026-10-09** (checked against pi-subagents v0.76.1; `src/runs/shared/run-history.ts` is unchanged since v0.75.0, so the `@v0.75.0` cites in this row still hold). `background/run_history.rs` is a 1:1 port of `run-history.ts`: `RunHistoryEntry` gains `taskHash` and `outcome` in upstream's on-disk key order (`:240-249`); `record_run` (`recordRun`, `:223-260`) writes `task: "[redacted]"` plus the sha256 of the FULL task, derives the outcome by upstream's ladder (`:231-239`, unexplained-signal rung included), hardens the storage (0700 dir, 0600 file, chmod errors swallowed, unix only), re-sanitizes legacy and externally written lines in place (`sanitizeHistoryLine`: plaintext tasks hashed then redacted, non-JSON dropped, unknown keys kept, no outcome synthesised), keeps upstream's stat cache, and appends with one `O_APPEND` write. `load_runs_for_agent` (`:262-288`) is ported with its rotation (keep 1000 past 1200) and, like upstream, has no production caller: **`record_run` does not rotate, faithfully**, so the file still grows unbounded in both runtimes. `plan_background_run_history` (`:136-221`) is ported, and the runner's terminal tail now calls it with a real launch fact: `ExecSingleStepExecutor::launched` is marked immediately before `exec::run_sync` (pi `launchedFlatIndices.add`, `subagent-runner.ts:3725,4130,4519`), after the child-stop gate, and an attached root is marked too (upstream's `:4519` precedes `runSingleStepInner`'s `importAsyncRoot` arm). Per-step duration is `ended_at - started_at`; the run-wide stopped/interrupted/timed-out flags are read off the loop outcome before `settle_loop_outcome` folds `TimedOut` into `Failed`. Early-failure `finish_run` callers pass no plan, so the synthesized placeholder result (agent = first step, or the run id) is no longer recorded as a row. The foreground single path records too: the attached settle tail records every terminal result (pi `:4396`; covers a refused detach and a workflow-awaited detach exactly once) and the plain-detach continuation records the child's real exit (pi `onDetachedExit`, `:4371`), never the receipt; path is the config snapshot's `roots.agent_dir()`. **Premise corrected:** this row's "a chain or parallel run is one row" was already stale when filed — since SCOPE_17 `ResultFile::results` is one entry per flat step, so rows were already per step; the real defects were rows for never-launched steps, the run-wide duration on every row, the missing outcome and the placeholder row. **Recorded deviations:** (i) a `DynamicGroup` owns one flat slot, so it records ONE row under its display agent (the SUBA-093 residual), where upstream records one per child; (ii) `[CYRUP-DELTA]` the foreground hash is of the AUTHORED task — upstream hashes `wrapForkTask(task)` for a forked agent (`subagent-executor.ts:4172-4175`) and cyrup has no such wrapper; (iii) `StepState::Partial` is in neither of upstream's sets (`run-history.ts:155,157`), so it records exit 1 / `failed` and takes run-wide flags, as upstream's rule says; (iv) an intercom-detached foreground result is recorded, because cyrup's drive loop returns it at the child's real exit and upstream records that exit through `onDetachedExit` with `detached` cleared (`execution.ts:2212-2218`); (v) per-step `timedOut` comes from the step's own result, since cyrup's `StepStatus` has no `timedOut` and `mark_remaining_timed_out` only relabels undispatched steps. The false doc claim that `--force`/staleness checks and the cost report read this file is removed: nothing reads it. Verify: `background::run_history::tests::{record_run_redacts_and_hashes_the_task_in_upstreams_key_order, record_run_creates_and_hardens_private_modes, outcome_ladder_matches_upstream_and_legacy_outcomes_are_not_guessed, existing_history_is_redacted_and_hardened_while_recording, an_external_write_is_resanitized_on_the_next_record, load_runs_for_agent_rotates_only_past_the_read_threshold, load_runs_for_agent_tolerates_the_legacy_shape_and_a_missing_file, the_history_task_is_the_sole_single_task_else_the_mode_word, steps_the_runner_never_launched_get_no_row, a_single_step_run_is_one_foreground_shaped_row, a_fan_out_records_one_row_per_child, pending_steps_get_no_row, a_self_terminal_step_keeps_its_own_outcome_when_the_run_is_interrupted, own_and_run_level_terminal_flags, a_partial_step_is_a_failure_that_run_flags_relabel}`, `background::runner_main::finish::tests::{a_chain_records_one_row_per_launched_step_with_its_own_duration, a_stop_before_the_first_dispatch_records_no_rows, run_wide_terminal_flags_reach_only_the_steps_that_did_not_finish_on_their_own, a_single_step_run_hashes_its_task_until_an_append_widens_it, control_inbox_dir_creation_failure_still_reaches_a_terminal_failed_state_via_finish_run}`, `background::runner_main::tests::an_attached_async_root_becomes_a_chains_first_step`, `extension::executor::foreground::detach_producer_tests::{a_detached_plain_single_records_its_terminal_result_once_the_child_exits, a_detached_workflow_childs_settle_reaches_the_step_that_launched_it}`; cyrup-it `subagents::background_runner_main_integration::{a_finished_background_chain_writes_one_redacted_private_history_row_per_step, a_child_scoped_stop_for_a_pending_step_is_queued_and_skips_it_when_reached}`, `subagents::subagents_detach_integration::{an_attached_foreground_single_records_one_redacted_private_history_row, a_live_detach_returns_the_receipt_and_leaves_its_child_running, a_detached_workflow_child_settles_back_into_the_step_that_launched_it}`. — **`run-history.jsonl` has the pre-hardening shape, and foreground runs are never recorded** |
| ~~SUBA-173~~ | ~~low~~ **CLOSED 2026-10-08** | parity-bug | S | **A saved profile's `machine` string is not validated before `/subagents-load-profile` writes settings** — upstream's `validateSubagentProfile` (`src/profiles/profiles.ts:125,148` @v0.75.0, `8a577c27`/#2619) calls the now-exported `validateOptionalMachine` (`src/agents/agents.ts:1035`) per override, so a bad value is rejected before any write; cyrup's `load_profile` (`registration/profiles.rs:211`) only serde-parses, and `placement/resolve.rs:33` already has `validate_optional_machine`. **FILED 2026-10-03** from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0); body below. — **CLOSED 2026-10-08** (on `claude/subagents-robustness-sweep`, checked against pi-subagents ad11b7ab, where the check is `src/profiles/profiles.ts:148-150` and `validateOptionalMachine` is `src/agents/agents.ts:1012-1019`): `load_profile` (`registration/profiles.rs`) now parses to a JSON value and runs `validate_profile_machines` before the typed parse: every override's `machine` other than absent or `false` goes through `placement::resolve::validate_optional_machine` with upstream's label `Profile '<file>' has invalid machine for '<name>'`, and the trimmed value replaces the raw one. `/subagents-load-profile` and `/subagents-check-profile` both call `load_profile` first, so a bad value is refused before `apply_profile_to_settings_file` writes. Verify: `an_invalid_profile_machine_is_refused_before_settings_are_written` (blank, 129 characters, a control character, `true`; settings byte-identical), `a_false_profile_machine_still_clears_the_pin`, `a_valid_profile_machine_is_applied_trimmed`. |
| ~~SUBA-175~~ | ~~low~~ **CLOSED 2026-10-09** | port-divergence | S | **CLOSED 2026-10-09** (checked against pi-subagents v0.76.1): `crate::artifacts::run_artifact_metadata` (`crates/cyrup-ext-subagents/src/artifacts.rs:617-629`) now inserts `turns`, taken from `SingleResult::turns`, into the `usage` object it writes to `_meta.json`, which is upstream's shape (the async runner folds `run.usage.turns` into the usage it writes, `src/runs/background/subagent-runner.ts:1238`, `:1509`; the foreground writer persists `target.usage`, `src/runs/foreground/execution.ts:154` @v0.76.1). Both cyrup producers go through that one builder, so no reader changed: `metadata_usage` (`registration/cost.rs`) reads `usage.turns` as before, and a `_meta.json` written before the fix still resolves with 0 turns. Verify: `registration::cost::tests::an_async_childs_turns_come_from_the_metadata_the_runner_writes`; red-proved, since reverting the insert fails it with `left: 0` / `right: 3`. — **`/subagent-cost` reports 0 turns for every async child resolved from `_meta.json`** — upstream's `metadataUsage` reads `turns` off the artifact metadata's `usage` (`src/slash/subagent-cost.ts:137` @ad11b7ab), and both producers write a `Usage` that carries `turns` (`src/shared/types.ts:261-268`; async runner `src/runs/background/subagent-runner.ts:1241,1512`; foreground `src/runs/foreground/execution.ts:154`). Cyrup's `_meta.json` writes `cyrup_core::Usage`, which has no `turns` (`crates/cyrup-ext-subagents/src/artifacts.rs:577`; the count lives beside it on `SingleResult::turns`, `exec/run_result.rs:120`), so `metadata_usage` (`registration/cost.rs:943`, called at `:1292` and `:1346`) always yields `turns: 0`. **FILED 2026-10-09** while reviewing `SUBA-163`; body below. |
| SUBA-176 | medium | upstream-drift | S | **The `subagent` tool schema still emits `deprecated: true`, which strict tool-schema validators reject with HTTP 400, so every request carrying the tool fails on those providers** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-177 | medium | upstream-drift | S | **A paused async run cannot be stopped once its paused result was delivered: the seal refuses with "paused result is missing" and the run keeps its capacity slot (extends the closed `SUBA-116`)** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-178 | medium | not-ported | L | **Runner launchers (`runnerLaunchers` config, agent `launcher:` frontmatter) are unported, and an agent that names a launcher silently runs unwrapped instead of failing** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-179 | low | upstream-drift | S | **`workflow: true` still fails "found 0" when the model fences its script as plain ```js, and says nothing when the reply's text never reached the session** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-180 | low | upstream-drift | M | **The child boundary instructions are prepended ahead of the base prompt, and the child runtime returns a frozen full prompt instead of filtering the prompt options** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-181 | low | upstream-drift | S | **Saving an agent whose description has a newline writes invalid frontmatter** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-182 | low | upstream-drift | S | **A schedule claim whose lock write fails wedges the schedule, and a lost claim overwrites the owner's schedule record from a stale snapshot** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-183 | low | upstream-drift | M | **Schedule `history.json` is rewritten from each session's own snapshot with no lease, so concurrent sessions lose runs from the index** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-184 | low | upstream-drift | S | **Watchdog settings writes and `/subagents-load-profile` read-modify-write `settings.json` with no lock, so concurrent sessions lose each other's changes** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-185 | low | not-ported | L | **Daily and weekly zoned calendar schedules (`every: "day"\|"week"`, `at: "HH:mm"`, `on`, `timezone`) are still refused (supersedes `09-cyrup-ext-subagents.md:901`)** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-186 | low | not-ported | M | **Subagent run state is not reported to the terminal with OSC 7501 (`programStatus`); blocked on `TUI-171`, which owns the root record** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-187 | low | upstream-drift | S | **In headless mode, results that finished during the `agent_end` drain still wait out the completion batch window, so a print-mode parent can exit before they are delivered** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-188 | low | upstream-drift | M | **Subagent notices wake an idle parent with a triggered custom message, which starts a run without `before_agent_start`, so extension-set prompt sections are missing from the woken run** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-189 | low | not-ported | M | **A parent that ends its turn without acting on a completion wake or a supervisor ask gets no reminder: the bounded `agent_before_settle` continuation is unported** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-190 | low | upstream-drift | M | **Subagent messages in the main chat still use per-type cards and glyph lines instead of Pi's collapsible `[subagent]` block** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-191 | low | upstream-drift | S | **Subagent model displays strip the provider, so the same model id from two providers is indistinguishable** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-192 | low | upstream-drift | S | **A blocking `bg_wait` ignores a steer or follow-up the operator types, holding the message until the wait window ends** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-193 | low | not-ported | M | **Async runs cannot be found by the tool-call id that launched them: no async status records `toolCallId` and the run-id resolver has no tool-call lookup** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-194 | low | upstream-drift | S | **The abandoned-slot release reads a runner PID from another PID namespace as dead, so capacity can be reclaimed while the runner is alive (the residual `SUBA-159` recorded)** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-195 | low | upstream-drift | M | **Text still streaming when a child times out or errors is lost: the partial-output tracker is unported** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-196 | low | upstream-drift | S | **A child ended by a per-tool timeout is reported as "Subagent timed out after {run budget}ms" with the real cause shown as partial output** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-197 | low | upstream-drift | S | **With `outputSchema`, a bound output file receives the child's closing prose instead of the structured result** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-198 | low | upstream-drift | S | **The main watchdog never records mid-run user input in scope, so a steer typed while the agent streams is later flagged as scope drift** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-199 | low | not-ported | L | **`worktree.cleanup` is still plan-only: reviewed plans cannot be applied** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-200 | low | upstream-drift | S | **The herdr bridge marks the pane `blocked` when a child needs attention, though that attention is for the parent agent, not the user** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-201 | low | upstream-drift | S | **Inspector open and close are not serialized per run, so concurrent opens can create two panes and one overwrites the other's binding** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-202 | low | not-ported | M | **The bundled tmux inspector plugin is unported** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-203 | low | upstream-drift | S | **Workflow `emit()` rejects objects with undefined fields, failing the whole workflow where `return` accepts the same value** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| SUBA-204 | low | parity-bug | S | **A generic external-CLI agent silently ignores a per-call `model` or `thinking`; upstream refuses the launch** — upstream adds `model override` and `thinking override` to its `does not support:` list for every external runner except the Claude Code adapters (`src/runs/background/async-execution.ts:1788,1790,1798`, chain steps `:1025,1031` @v0.76.1); cyrup refuses only fast mode (`exec/external_cli/mod.rs:324-329`, `extension/executor/background.rs:1185`), and `resolve_claude_code_launch_override` returns no argv for any other runner (`exec/external_cli/mod.rs:212-218`), so the requested model is dropped without a word. **FILED 2026-10-09**; body below. |
| SUBA-205 | low | upstream-drift | S | **The Claude Code version probe refuses the calendar-style version Claude Code now prints** — upstream accepts `YYYY.M.D <platform> (YYYY-MM-DD)` beside semver since `02cb5b6a` / #1828 (v0.65.0; `src/runs/shared/claude-code-adapter.ts:274-280` @v0.76.1); cyrup's `validate_version` (`exec/external_cli/adapters/claude_code.rs:388-421`) is the semver-only pattern, so a `claude` that prints the calendar form fails preflight with "Unsupported Claude Code version response". **FILED 2026-10-09**; body below. |
| SUBA-206 | low | parity-bug | S | **`_meta.json` and `_input.md` store the child's task in plaintext; upstream writes `[prompt redacted]`** — upstream's metadata writers put `task: PROMPT_REDACTED` (`src/runs/foreground/execution.ts:151`, `src/runs/background/subagent-runner.ts:1504` @v0.76.1) and the input artifact holds only `[prompt redacted]; live Prompt Audit only.` (`execution.ts:1812`, `subagent-runner.ts:850`); cyrup writes `"task": result.task` (`artifacts.rs:575`) and the full task into `_input.md` (`background/runner_main/executor.rs:1189`, `extension/executor/foreground.rs:2109`). **FILED 2026-10-09**; body below. |
| SUBA-207 | low | upstream-drift | S | **An external-runner profile may declare `allowedAgents`; upstream refuses it as a Pi-only field** — upstream's `validateExternalRunnerProfile` list gained `allowedAgents` in v0.70.0 (`7ba72ee4`; `src/agents/agents.ts:2075` @v0.76.1); cyrup's `PI_ONLY_FIELDS` (`runner/mod.rs:204-221`) does not have it, so the key is parsed (`SUBA-111`) and accepted on a foreign-CLI profile without a word. **FILED 2026-10-09**; body below. |
| SUBA-208 | low | upstream-drift | S | **`subagent`, `subagent_supervisor`, `contact_supervisor` and `structured_output` are `direct` tools, so codemode scripts can call them** — upstream marks all of them `exposure: "model-only"` (`MODEL_ONLY_TOOL`, `src/shared/extension-context.ts:9`; `09585988` / #2586, v0.74.0); in cyrup only the `subagents_enable` loader overrides `Tool::exposure` (`extension/tool_activation.rs:374`), and the other four keep the `Direct` default (`cyrup-core/src/tool.rs:410`). **FILED 2026-10-09**; body below. |
| SUBA-209 | low | upstream-drift | M | **The advertised-agents catalog rewrites the whole system prompt each turn instead of riding its own `advertised_subagents` prompt section** — upstream sets `event.systemPromptOptions.sections.advertised_subagents` (`src/extension/index.ts:793-802` @v0.76.1; `e583ea6a` / #2519, v0.73.1) so the host appends a transcript delta and the cached prefix survives `subagents_enable`; cyrup's `before_agent_start` returns a rewritten `system` string (`extension/host/native_impl.rs:920-931`), which the host records as `forceSystemPrompt` (`cyrup-ext/src/contract.rs:216-219`). Portable now that `EXT-084` (closed 2026-10-09) gives cyrup typed prompt-option sections. **FILED 2026-10-09**; body below. |
| SUBA-210 | low | not-ported | M | **The async-jobs widget cannot be folded by clicking its header** — upstream's mounted widget toggles its one-line card on a left click on row 0 with no modifier (`handleMouse`, `src/tui/render.ts:2938-2944` @ad11b7ab, `93d47c0c`/#2235) and resets its layout session; cyrup routes a press on an extension widget to text selection (`cyrup-tui/src/app/pointer.rs` module doc) and `HostServices::set_widget` carries lines only, so the folded state is `asyncWidgetCollapsed` for the widget's life. **FILED 2026-10-10** (split from `SUBA-162`); body below. |
| SUBA-211 | low | not-ported | M | **The async-jobs widget never sees the real terminal size or a launch roster** — upstream sizes its tiers from `process.stdout.rows`/`columns` (`src/tui/render.ts:2536-2547` @ad11b7ab) and counts a step-less job's `agents` (`:2642-2644`); cyrup's extension has no terminal-size accessor, so `slash.rs` passes pi's `30`/`120` fallbacks and the default `adaptive` layout never leaves the full tier, and no producer fills `AsyncJobSnapshot::agents`. **FILED 2026-10-10** (split from `SUBA-162`); body below. |
| SUBA-212 | low | upstream-drift | S | **A placed (Herdr machine) external-CLI step is never flagged `needs_attention` when idle** — upstream's runner tick derives idle state for every running step, counting an external-cli step as one turn (`subagent-runner.ts:3173` @ad11b7ab), so a machine step gets the plain idle rule (it skips only the git baseline, `:871`); cyrup's `exec/external_cli/placed.rs::run_placed_external_cli` builds no `ControlMonitor` and returns no `control_events`. Split from `SUBA-131` (2026-10-10). |

---

## SUBA-110 — Git routing variables reach the background runner and inherited external CLIs

**Kind** upstream-drift · **Severity** high · **Effort** S · **Confidence** confirmed (both sides read; not observed live) · **CLOSED 2026-09-24** (`c936d8c`): new `spawn/git_env.rs` ports `git-environment.ts` (the 16 names plus `GIT_CONFIG_{KEY,VALUE}_<n>`, case-insensitive); applied before the overlay in `background/spawn_detached.rs` and on the inherited arm of `exec/external_cli/run.rs`; an adapter allowlist is left as written. Verify: `spawn::git_env::tests` (predicate; removals-then-overlay; a real `git rev-parse` resolves the parent repo through an inherited `GIT_DIR` and the child's own repo once filtered). The two call sites are not driven by a test (`#![forbid(unsafe_code)]` rules out setting `GIT_DIR` on the test process).

**cyrup** — The detached background runner is spawned at
`crates/cyrup-ext-subagents/src/background/spawn_detached.rs:213-228`. It uses
`tokio::process::Command::new(&spawn_command.binary)` with `.envs(env_overlay)` and never clears or
filters anything; the comment at `:225-227` says *"overlay (never `env_clear`)"*. The generic
external-CLI path (no adapter) is `ExternalEnv::Inherited`
(`exec/external_cli/env.rs:56-63`), which is a plain inherit. `exec/external_cli/run.rs:182-189` calls
`env_clear` only for the `Allowlisted` variant. `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE` and
`local-env-vars` have **zero hits** in `crates/`. (The `GIT_INDEX_FILE` hits in `spawn/cleanup_plan/`
are cyrup setting its own temporary index for its own commands. They do not filter anything.)

**upstream** — `src/runs/shared/git-environment.ts` @v0.71.0 (new in the window) is a 16-name set
(`GIT_ALTERNATE_OBJECT_DIRECTORIES`, `GIT_COMMON_DIR`, `GIT_CONFIG`, `GIT_CONFIG_COUNT`,
`GIT_CONFIG_PARAMETERS`, `GIT_DIR`, `GIT_GRAFT_FILE`, `GIT_IMPLICIT_WORK_TREE`, `GIT_INDEX_FILE`,
`GIT_NAMESPACE`, `GIT_NO_REPLACE_OBJECTS`, `GIT_OBJECT_DIRECTORY`, `GIT_PREFIX`,
`GIT_REPLACE_REF_BASE`, `GIT_SHALLOW_FILE`, `GIT_WORK_TREE`) plus `/^GIT_CONFIG_(?:KEY|VALUE)_\d+$/`.
The match is case-insensitive for Windows. `omitGitRoutingEnv` is applied in two places: the
background runner's env (`src/runs/background/async-execution.ts:729` @v0.71.0,
`...omitGitRoutingEnv(omitExtensionBindingsEnv(process.env))`) and the external CLI's inherited
default (`src/runs/shared/external-cli-runner.ts:91` @v0.71.0). An adapter allowlist is left
unfiltered on purpose (`:90` comment). CHANGELOG 0.71.0 *Fixed*, #2437/#2440.

**Impact** — Suppose cyrup is started from a Git hook, or under `git --git-dir=…`, or under anything
else that exports these variables. Then a background child's `git add` / `git commit` / `git
checkout` works on **the parent's repository and index**, not on the child's cwd or managed worktree.
The change is silent and lands in the wrong repository. Blast radius, recorded as scheduling
information and not as a rating: this only happens when the orchestrator's own environment carries
the variables.

**Fix** — Add a `git_routing_env_names()` predicate in `spawn/` (or `exec/external_cli/env.rs`) that
ports the set and the regex verbatim, including the case-insensitive match. In `spawn_detached.rs`,
call `command.env_remove(name)` for every matching inherited key before `.envs(env_overlay)`. This
keeps the crate's "never `env_clear`" invariant. Build `ExternalEnv::Inherited` the same way at
spawn. Leave `Allowlisted` alone, as upstream does.

**Verify** — An integration test sets `GIT_DIR=<parent>/.git` and `GIT_INDEX_FILE=<tmp>` on the
orchestrator process, launches an async child whose task runs `git rev-parse --git-dir`, and asserts
that the child reports its own worktree. A unit test covers `GIT_CONFIG_KEY_0` and a lower-case
`git_dir`.

## SUBA-107 — The completion-mutation guard is still live after upstream deleted it

**Kind** stale-port · **Severity** medium · **Effort** M · **Confidence** confirmed (both sides read)

**cyrup** — The guard runs on the production run path: `exec/mod.rs:725` calls
`gates.apply_completion_guard(agent, task, &progress, &mut control)`. The body
(`exec/mod.rs:1695-1712`) calls `evaluate_completion_mutation_guard` and, when it triggers, sets
**`self.exit_code = 1`** and pushes `COMPLETION_GUARD_ERROR_MESSAGE`
(`exec/completion_guard.rs:760`: *"completion-mutation guard: task appeared to require an
implementation change, but no mutating edit/write/bash tool call was observed before the run
completed"*). It also raises the `completion_guard` notice. The classifier is
`exec/task_intent.rs`, which says it is a port of `task-intent.ts` @v0.43.0. #152 (`df3e9a8`) *added*
to it: its body says `mutationTools` is "counted as mutating by the completion guard".

**upstream** — `src/runs/shared/completion-guard.ts` and `src/runs/shared/task-intent.ts` exist at
v0.70.0 (`git cat-file -e`), where `execution.ts:1554` calls `evaluateCompletionMutationGuard`. Both
are **deleted at v0.70.1** by `7c98a696` (*refactor: remove inferred no-edit completion failures
(#2356)*). `completionGuard` has zero hits under `src/` at v0.71.0, and at v0.71.0 it is gone from
the override-field list (`src/agents/agents.ts:2029`). CHANGELOG 0.70.1 *Changed*: *"Stop guessing
whether task wording requires file edits. Successful tasks now follow their process result and
explicitly configured output and acceptance checks. The `completionGuard` setting and
`PI_SUBAGENTS_LLM_INTENT_ARBITER` switch have been removed."*

**Impact** — A delegated task can finish correctly without editing files: the fix was already
there, the investigation found nothing to change, or the wording ("implement…") was loose. In cyrup
that run is still reported **failed** with exit code 1 and a failure notice. Upstream reports it by
its process result. This is a wrong outcome on a normal path, and it is exactly the defect upstream
removed the guard to fix (#2351).

**Fix** — Remove `apply_completion_guard` from the `exec/mod.rs` gate sequence. Remove the
`completion_guard` frontmatter/override key and its management rendering, or accept it as a no-op
with a deprecation warning. Decide whether to keep `completion_guard::is_mutating_bash_command` and
`has_mutation_tool_call`: `exec/control.rs` (long-running guard) and startup evidence use them, so
move them rather than delete them. `SUBA-108` removes `task_intent.rs`'s last consumer. Land the two
items together.

**Verify** — A run whose task says "Implement the fix", with a writer agent that makes no edit and
exits 0, finishes with `exit_code == 0` and no `completion_guard` notice.
`grep -rn COMPLETION_GUARD_ERROR_MESSAGE crates/` is empty.

## SUBA-108 — Acceptance-level inference still reads task wording and agent names

**Kind** stale-port · **Severity** medium · **Effort** M · **Confidence** confirmed (both sides read) · **STILL OPEN 2026-09-27**: DEFECT 2 (`acceptance.ts:521-522`) is ported and red-without, DEFECT 3's cyrup-it restatement is done, the MINOR doc is corrected, and the invariance-table test the row's Verify asks for exists — keep all of it. TWO blocking items remain, both empirically confirmed at HEAD. (A) Port `acceptance.ts:505`, where the explicit policy is actually COMBINED, not only in the model helper. `crates/cyrup-ext-subagents/src/exec/acceptance/lattice/contract.rs::resolve_effective_for_role` (`:378-400`) must upgrade the INFERRED floor from `NotRequired` to `Attested` when an explicit policy is present and requests one, BEFORE its MAX — i.e. the lattice seam, reached from `crates/cyrup-ext-subagents/src/exec/mod.rs:269`, has to consult the same predicate. The cleanest shape is to stop duplicating the rule: route the lowered explicit policy through `resolve_effective_acceptance` (`explicit: Some(...)`) instead of re-implementing level arithmetic, since as it stands the new `:505`/`:509` code is DEAD on every production path. Note that the lattice's `explicit_floor(NotRequired)` + `is_no_op()` encoding is what swallows the policy: after the upgrade the contract must be non-no-op so `inject_acceptance_contract` emits the block and `evaluate_acceptance` does not short-circuit at `gate.rs:205`. Proof it is still broken (scratch test, since removed): `lower_acceptance_input(json!({"evidence":["review-findings"]}))` then `AcceptanceContract::resolve_effective_for_role(lowered, "scout", Some(AcceptanceRole::ReadOnly), "Audit the flow")` gives `required_level=NotRequired`, `is_no_op()=true` and an unchanged injected task, where upstream gives `attested`, the read-only branch's findings criterion, evidence `[review-findings, residual-risks]`, an emitted prompt and an armed gate. A test at the LIVE seam (lowering → `resolve_effective_for_role` → `inject_acceptance_contract`) is required, red without the change; the existing `a_caller_supplied_policy_reaches_a_read_only_child` only exercises the model function. (B) Make the presence predicate cover every key upstream's `Object.keys` sees, then either delete the `CYRUP-DELTA` at `level.rs:215-220` or restate it truthfully. `report` is a supported acceptance key on BOTH sides (upstream `acceptance.ts:53` + `shared/types.ts:1065`; cyrup `validate_input.rs:71-79`), so `{report: "on"}` — and `{report: "off"}` — must make `explicit_acceptance_requests_policy` true; today it cannot, because `AcceptanceConfig` (`model/types.rs:269-277`) has no `report` field, so either carry `report` on `AcceptanceConfig` (letting the `AcceptanceReportMode` that `resolve_acceptance_report_mode` resolves keep its own path) or thread `acceptance_declares_report` into the predicate. Needs a test: declared `read-only` role + `{report:"on"}` resolves `attested`, red without the change. The delta may only be KEPT if it stops claiming full parity on an input set that in fact diverges; `preserveStagedIndex` (an upstream config key cyrup rejects outright) is a separate pre-existing gap and is NOT this row's to fix, but it must not be papered over by the same delta either. (Non-blocking leftover, still true: `exec/tool_surface.rs` routes through `task_intent::is_review_or_scout_lane_agent`; that is not acceptance behaviour and does not block.)

**cyrup** — `exec/acceptance/model/level.rs::infer_level` (`:234`) still calls
`task_intent::classify_task_mutation_intent` (`:245`), `task_may_mutate` (`:287`) and
`strip_severity_compounds` (`:283`). In its comment it cites `acceptance.ts:90` **@v0.57.0**. So the
inferred level (`none` / `attested` / `checked`, and the required reviewer for async or dynamic
writes) still depends on task wording, on `reviewer|oracle|scout|researcher|analyst` /
`worker` agent-name regexes, and on the risky-keyword pattern (`release|migration|…`).

**upstream** — `7c98a696`'s first half (*refactor: base acceptance inference on declared roles*)
removes all of that from `inferLevel`. At v0.71.0 (`src/runs/shared/acceptance.ts:81-122`), the
level comes from `acceptanceRole` alone:

- `writer` + async/dynamic → `checked` with a required `reviewer` (`:90`);
- `writer` → `checked` (`:101`);
- `read-only` → `none` (`:109`);
- anything else → `attested`, "default lightweight attestation" (`:119`).

CHANGELOG 0.70.1: *"Custom implementation agents must declare `acceptanceRole: writer` to receive
writer acceptance defaults."*

**Impact** — In cyrup, the same agent and task get different acceptance gates from upstream. A task
that mentions "migration" or "security" still becomes `checked` with required evidence, and a scout
given write-sounding wording is gated as a writer. The reverse also happens: once this is ported,
any cyrup agent without an explicit role drops to `attested`. That is why `SUBA-112` (the bundled
worker's missing `acceptanceRole: writer`) has to land with this item. *2026-09-28: `acceptanceRole: writer` was already in `worker.md` at HEAD (`282b9fa2`); `SUBA-112` closed on `claude/lows-next` with the rest of the frontmatter and prose sync.*

**Fix** — Port `inferLevel` @v0.71.0 verbatim into `level.rs::infer_level`. Delete
`task_intent.rs` once `SUBA-107` has removed its other consumer. `exec/tool_surface.rs:1038` uses
`is_review_or_scout_lane_agent`, which is not acceptance, so re-home that helper first. Update the
`read_only_acceptance_inference` tests in `cyrup-it` to state roles rather than wording.

**Verify** — There is one table test over (role ∈ {writer, read-only, none}) × (async, dynamic, plain)
× (task text containing "migration" / "review only" / "implement"), and the level is independent of
the task text in every row.

## SUBA-111 — Agent `allowedAgents` is unported, so a declared delegation restriction fails open

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed (both sides read)

**cyrup** — `allowedAgents` exists only as an axis of the session **capability ceiling**
(`exec/capability_ceiling.rs:11,90,185-203`). It has **zero hits under `discovery/`**, so agent
frontmatter does not read it. `discovery/frontmatter.rs:1325` and the test at `:2185` say that an
unknown frontmatter key is "round-tripped verbatim into `extra_fields`". It is kept, and nothing
enforces it or warns about it. `agentOverrides.<name>.allowedAgents` lands in the settings key census
(`discovery/key_census.rs`) as an *unknown* key. That produces a warning and nothing more.

**upstream** — Added at v0.70.0 (#2312; `git show v0.67.0:src/agents/agents.ts | grep -c
allowedAgents` → 0). At v0.71.0, `src/agents/agents.ts:2118-2119` parses frontmatter
`allowedAgents` and normalises it with `normalizeCapabilityCeilingAllowedAgents`. `:1140-1141` parses
the override form (`string[] | false`, and `false` clears it). The foreground path
(`src/runs/foreground/execution.ts:403`) and the background path
(`src/runs/background/runner-child-launch.ts:53`) pass it as `descendantAllowedAgents`.
`src/runs/shared/child-launch.ts:185-187` turns that into an agent-sourced capability ceiling
`{ version: 1, allowedAgents: [...], denyExtensions: false, sources: ["agent:<name>"] }` on the
child. Management, serialisation (`agent-serializer.ts:82-83`) and async resume
(`async-resume.ts:374-375,622`) carry it.

**Impact** — Suppose an author writes `allowedAgents: reviewer` on an agent to limit what it may
delegate to. Under cyrup, that agent's child can launch any agent the session allows. The restriction
is written in the file, looks accepted, and is ignored. A declared narrowing that fails open is the
same shape as `SUBA-072` (critical when filed). This row is rated medium because it narrows
*delegation targets*, not tool capability, and the session ceiling still applies.

**Fix** — Add `allowed_agents: Option<Vec<String>>` to `AgentDefinition`, parsed in
`discovery/frontmatter.rs` with the ceiling's normaliser, and add the `false`-clears override form to
`AgentOverrideConfig`. At child launch (`exec/spawn_plan.rs`, background `runner_main/`), intersect
it into the child's `CapabilityCeiling` with source `agent:<name>`. Carry it in the recovery
descriptor and in management create/update/render.

**Verify** — An agent with `allowedAgents: reviewer` whose task asks it to delegate to `worker` gets
the ceiling refusal. The same agent delegating to `reviewer` succeeds. An `agentOverrides` value of
`false` restores unrestricted delegation.

## SUBA-109 — `fallbackModels` and persistent model exclusions are still live after upstream removed them

**Kind** stale-port · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read)

**cyrup** — `discovery/frontmatter.rs:1006` parses `fallbackModels`. `exec/fallback.rs` builds the
ladder (`build_model_candidates`, model override → primary → `fallback_models`), and
`exec/mod.rs:1021` drives it through `run_fallback_ladder(candidates, &mut runner,
opts.model_exclusions.as_deref())`. `exec/model_exclusions/` (`store`, `persist`, `filter`, `auth`) is
the persistent exclusion store that SCOPE batch 1 (`2bd76ac`) ported. The crate knows upstream moved
on. `exec/spawn_plan.rs:226-230` says *"Upstream checks its ONE launch model (v0.68.0 has no fallback
ladder). cyrup's ladder can reach…"* and marks that as a `[CYRUP-DELTA]` about fast-mode granularity.
It is not recorded as a decision to keep the ladder.

**upstream** — At v0.68.0, `model-exclusions.ts` is deleted and `model-fallback.ts` is renamed to
`model-resolution.ts`, which has no ladder (`f58dfcb5`, #2270). The key is now **refused by name** in
five places at v0.68.0:

- frontmatter (`src/agents/agents.ts:2059`, *"uses removed frontmatter field 'fallbackModels'.
  Configure one model instead."*);
- builtin overrides (`:998-999`);
- runtime definitions (`src/agents/runtime-agent-registry.ts:226`);
- profiles (`src/profiles/profiles.ts:147`);
- management config (`src/agents/agent-management.ts:459`).

The child watchdog config refuses it too (`src/watchdog/child-status.ts:131-132`). CHANGELOG 0.68.0
*Removed*: *"Remove `fallbackModels`, all same-launch model switching (including read-only HTTP 429
continuation), and persistent model exclusions. Retry another model only with a later explicit
launch."*

**Impact** — An agent file written for current upstream never contains the key. So the visible
divergence goes the other way: a cyrup agent that uses `fallbackModels` is refused when it is copied
to upstream, and in cyrup one launch can silently finish on a different model from the one the
caller asked for. `SUBA-089` already stops a re-dispatch after tools have run, and that bounds the
damage. The row is low because the ladder is cyrup behaviour working as designed, just against a
design upstream has retired. **This needs a decision, not only effort**: follow upstream, or record
a `[CYRUP-DELTA]` decision of record and re-kind this row `port-divergence`.

**Fix** — To follow upstream: refuse the key at all five parse sites with upstream's text. Collapse
`run_fallback_ladder` to one attempt. Retire `exec/model_exclusions/` and its persisted store (with a
migration that ignores an existing file).

*Pass 2 note:* the same removal replaced `attemptedModels`/`modelAttempts` with `requestedModel` in
run results and status (`2b64ced0`, v0.68.0; `subagent-runner.ts:1497,1528` @v0.71.0), and
`exec/fallback.rs:1336-1338`'s "UNPORTED" note for `ReadonlyContinuation` is now stale. Both go with
this fix.

**Verify** — An agent file with `fallbackModels:` fails discovery with upstream's message. A retryable
failure on the primary model ends the run and does not start a second attempt.

## SUBA-112 — The bundled `worker` agent's frontmatter has drifted

> **CLOSED 2026-09-28.** (on `claude/lows-next`) The bundled `resources/agents/worker.md` is byte-identical to `agents/worker.md` @v0.71.0: `defaultContext: fresh` and the three prose drifts are synced. Test: `the_bundled_worker_starts_fresh_and_is_a_writer`. **Ledger correction:** `acceptanceRole: writer` was already at HEAD (landed in `282b9fa2` with the `SUBA-108` work), so the `cyrup` paragraph below was half stale. Evidence is in the row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**cyrup** — `crates/cyrup-ext-subagents/resources/agents/worker.md:10` is `defaultContext: fork`, and
the frontmatter has no `acceptanceRole`.

**upstream** — `agents/worker.md` @v0.71.0: `:5 acceptanceRole: writer` (added at v0.70.1 by
`7c98a696`; absent at v0.70.0) and `:11 defaultContext: fresh` (still `fork` at v0.70.1). CHANGELOG
0.71.0 *Changed*: *"Packaged `worker` agents now start with fresh context instead of forking the
parent's conversation"* (#2384). The same commit added `acceptanceRole: writer` to
`claude-code-writer.md`, `codex-exec-writer.md` and `cursor-agent-writer.md`. cyrup does not bundle
those three. `oracle.md` stays `fork` on both sides.

**Impact** — cyrup's bundled worker inherits the parent's full conversation on every call. That costs
more tokens and exposes more context than upstream now intends. Without `acceptanceRole: writer`, the
worker falls to `attested` as soon as `SUBA-108` lands.

**Fix** — Set `defaultContext: fresh` and add `acceptanceRole: writer` in `resources/agents/worker.md`.
The prose also drifted: the `First read the provided context…` paragraph, the
`contact_supervisor`-unavailable fallback, and the source-discoverability bullet. Take those as a
separate text sync.

**Verify** — `/subagents` shows `worker` with a fresh context and the writer role. A worker launched
with no `context` argument gets a fresh child session.

## SUBA-113 — tracker: thirteen `config.json` keys are self-declared unported, and until now no item carried them

**Kind** tracker · **Severity** tracker · **Effort** — · **Confidence** confirmed (cyrup side read; upstream side not re-read per key)

`crates/cyrup-ext-subagents/src/registration/mod.rs:525-553`, `UNPORTED_CONFIG_KEYS` (13 entries,
cited against `shared/types.ts:2596-2690` @v0.68.0), lists these keys: `forkContext`,
`modelResponseAliases`, `mainWindowRenderer`, `orcaProgressTabs`, `toolTimeoutMs`,
`checkpointBeforeDeadlineMs`, `toolBudget`, `usageBudget`, `worktree`, `worktreeProvider`,
`worktreeBranchPrefix`, `intercomBridge` and `resultScanLogging`. Its doc says: *"Each is a real
feature gap, not a decision that it is out of scope; **the ledger carries them**."* Before this row,
none of the thirteen had a `SUBA-` id. `modelResponseAliases` appears only as an UNVERIFIED lead in
`09a`'s census, and `mainWindowRenderer` only as a `SUBA-061` aside. The keys are **reported, not
silently dropped** (`SubagentExtensionConfig::config_warnings`), so none of them is a silent-ignore
defect. That is why this row is a tracker and not an item.

*Pass 2 (2026-09-24): `checkpointBeforeDeadlineMs` escalated to `SUBA-128`, `modelResponseAliases` to
`SUBA-119`. `worktreeProvider` stays here (the `worktrunk` provider is the 09a census lead this row
absorbs).*

*2026-09-28 (on `claude/lows-next`): `SUBA-128` closed, so `checkpointBeforeDeadlineMs` is no longer
in `UNPORTED_CONFIG_KEYS`. The const now holds 11 entries (`registration/mod.rs:559`), and
`modelResponseAliases` is not among them either.*

**Escalates to an item** when a key's feature is shown to change behaviour for a user who sets it.
The likely first candidates are `checkpointBeforeDeadlineMs` (the per-call form was added at v0.68.0
for async single runs) and `worktreeBranchPrefix`. Each needs its own row, with both sides read.

## SUBA-114 — Child tool plans are still pruned to the parent session's registry

**Kind** stale-port · **Severity** high · **Effort** M · **Confidence** confirmed (both sides read; not observed live) · **CLOSED 2026-09-26** (`850eb70`, #156)

> **Closure evidence.**
>
> * **The prediction is gone, everywhere it reached** (pi `b12496b8`). `exec/tool_surface.rs`:
>   `host_builtin_tool_names`, `HOST_BUILTIN_TOOL_NAMES`, `REPOSITORY_INSPECTION_TOOLS`,
>   `NATIVE_COORDINATION_TOOL_NAMES` (no other consumer), the host partition, the host `read`
>   throw, `is_review_or_scout_lane_agent` / `missing_permitted_repository_inspection_tools` /
>   `format_review_lane_tool_contract_failure`, the omission warning, and the
>   `ResolvedToolSurface::{unavailable_host_builtins, warnings}` wire fields (their only producer)
>   are deleted; `resolve_tool_surface{,_in}` lost the host parameter. `RunOptions`,
>   `RunnerConfig`, `ExecSingleStepExecutor` (field + `foreground` argument) and every launch site
>   (`extension/executor/{foreground,background,chain}.rs`) no longer carry
>   `host_available_builtins`; `task_intent::is_review_or_scout_lane_agent` went with its only
>   caller; `SubagentError::ToolContractUnsatisfiable` keeps one producer (the fanout refusal).
>   Old payloads stay readable: neither `RunnerConfig` nor `ResolvedToolSurface` denies unknown
>   fields (`RecoveryDescriptor` does, but never carried the key) —
>   `session_state::tests::a_runner_config_carrying_the_removed_host_observation_still_decodes`,
>   `tool_surface::tests::a_payload_carrying_the_removed_host_fields_still_decodes`.
> * **The child-side guard, brought to v0.71.0.** Upstream deleted `PI_CORE_CHILD_TOOLS` in #1356
>   (`51cca33e`, v0.55.0) and throws from `agent_start`; cyrup still carried the v0.43.0 floor
>   (which would have waved a missing `bash`/`edit` through) and only wrote a file. Now
>   `tool_availability::write_child_tool_diagnostic` diffs against the registry alone, the MCP
>   line is v0.71.0's wording, and `prompt_runtime::refresh_tool_diagnostic` aborts the child's own
>   run (`ControlOp::Abort`) when anything is missing; the parent reports the file as the run's
>   error (`attempt_runner::diagnose_attempt_error`, unchanged rank). Upstream's `host:"parent"`
>   diagnostic arm (in-process foreground child) is not ported: a cyrup child is always its own
>   process.
> * **Verify (production path, real parent `AgentSession`)** —
>   `crates/cyrup-it/tests/subagents/child_tool_plan_not_predicted_from_parent.rs`: a parent built
>   with `--tools subagent,read` AND a registry of only those two (asserted: no `bash`/`edit`/`grep`
>   definition) launches `worker` (`read, bash, edit`), `reviewer` and `scout` in the foreground
>   (`a_narrow_parent_launches_foreground_children_with_every_declared_tool`), a `/chain` step
>   (`…_a_chain_step_…`) and a detached background run through the written `runner-config.json`
>   and production `run_with` (`…_a_detached_background_child_…`); each child's REAL argv carries
>   the full `--tools`. `a_child_whose_registry_lacks_a_required_tool_is_refused_at_agent_start`:
>   a real child session (`--tools read,bash,edit`, registry `read` only, runtime from
>   `prompt_runtime_from_env`) ends its turn ABORTED without the model request ever being polled,
>   with upstream's five-line message; `the_parent_reports_the_childs_agent_start_refusal_as_the_run_error`
>   carries it back as the subagent run's error. Unit: `tool_surface::tests::{every_declared_core_tool_is_kept_so_the_child_registry_decides,
>   review_and_scout_lanes_resolve_like_any_other_agent, a_capability_ceiling_is_still_honoured}`,
>   `spawn_plan::tests::declared_tools_reach_the_child_argv_and_required_env_for_every_lane`,
>   `tool_availability::tests::a_missing_core_builtin_is_reported_there_is_no_floor`,
>   `prompt_runtime::tests::agent_start_aborts_the_run_when_the_real_registry_lacks_a_required_tool`.
> * **Correction to the reach stated below.** The CLI `--tools` bounds the ACTIVE set; cyrup's
>   registry (`DynamicToolState`, what `all_tools` reads) is bounded by `tool_availability`
>   (`Availability`, the SDK's `allowedToolNames`), which the CLI never sets. So on the CLI the
>   prediction mostly saw all eight built-ins and pruned little; it bit when the registry was
>   narrowed (SDK/harness) and for review/scout lanes there. The fix is the same either way; the IT
>   parent is narrowed on both axes so the deleted code would have pruned it.
> * **Tests deleted or rewritten** (they pinned the removed behaviour): `tool_surface` — the 6
>   `host_builtin_tool_names_*`/`a_malformed_tool_row_*` observer tests, the vocabulary guards
>   (`host_builtin_tool_names_track_the_tool_registry`, `repository_inspection_tools_match_upstream`,
>   `native_coordination_names_match_the_crate_constants`,
>   `every_intercom_persona_is_covered_by_the_coordination_exemption`), the 7 host-intersection
>   tests (two rewritten host-free as `a_capability_ceiling_is_still_honoured` and
>   `an_intentionally_empty_review_allowlist_launches_with_no_tools`),
>   `the_host_read_throw_precedes_the_ceiling_read_throw`,
>   `a_host_that_omits_subagent_does_not_revoke_the_supervisor_tool`, all 11 review-lane/warning
>   tests (the port of upstream's deleted `child-tool-plan-diagnostics.test.ts`) and the two wire
>   tests for the removed fields; `spawn_plan` —
>   `a_reviewer_launch_with_a_host_missing_read_is_refused_through_run_options` and
>   `the_same_reviewer_launch_proceeds_when_the_host_is_unknown` (replaced by
>   `declared_tools_reach_the_child_argv_and_required_env_for_every_lane`); `runner_main::executor`
>   — `the_step_executor_forwards_the_observation_to_every_step`,
>   `the_foreground_chain_executor_carries_the_observation`; `extension::executor` —
>   `the_observation_seam_reads_the_live_host`; `session_state` — the two round-trip tests
>   (replaced by the legacy-decode test); `tool_availability::tests::the_core_tool_floor_is_unioned_into_available`
>   (replaced by `a_missing_core_builtin_is_reported_there_is_no_floor`).

**cyrup** — `exec/tool_surface.rs::host_builtin_tool_names` (`:130-160`) reads the LAUNCHING session's
`HostServices::all_tools`, and production passes it into every launch: `extension/executor/foreground.rs:1177`,
`extension/executor/background.rs:744`, `extension/executor/chain.rs:161`, `extension/executor/mod.rs:1030,1042`
(and on to the detached runner via `background/runner_main/config.rs:235`). `resolve_tool_surface_in`
then partitions the agent's declared built-ins by that set (`:584-593`) and DROPS every one the parent
does not have, with a warning (`:975-990`); for a `reviewer`/`scout` it REFUSES the launch outright
(`:947-958`). The parent's registry is itself bounded by the parent's own `--tools`/`--no-tools`:
`cyrup-session-svc/src/builder.rs:1128,1566-1570` filter both built-ins and extension tools through
`resolve_allowed_tool_names`, and `session/tools.rs:94-99` filters late tools the same way — so
`all_tools` (`host_services.rs:2178-2199`, reading that registry) reports only what the parent was
allowed.

**upstream** — `b12496b8` (*fix: stop predicting a child's tools from its parent's session*, #2289,
in v0.70.0) deleted the prediction: `hostAvailableBuiltins` and `getHostBuiltinToolNames` exist at
v0.68.0 (`src/runs/shared/child-tool-plan.ts:198,354,384`) and are gone at v0.71.0
(`git show v0.71.0:src/runs/shared/child-tool-plan.ts | grep -c hostAvailable` → 0), together with
the review/scout lane check and its audit fields. The commit's reasoning: a child is a separate
session that builds its own tools, and it already validates its real registry against required tools
at `agent_start`.

**Impact** — Start the orchestrator as a narrow dispatcher (`--tools subagent,read`) and every child
silently loses the `bash`/`edit`/`grep` its agent declares; a `reviewer` or `scout` is refused
outright. Because a cyrup child is launched with its effective allowlist as its own `--tools`, a
grandchild loses more at each hop. Upstream fixed exactly this compounding loss.

**Fix** — Stop threading `host_available_builtins` into the plan (drop it from `RunOptions`, the
runner config and the recovery path), delete the partition at `tool_surface.rs:584-593`, the lane
refusal and the omission warning, and keep the child-side check (the child's own registry vs. its
required tools). Keep `NATIVE_COORDINATION_TOOL_NAMES` only if another consumer needs it.

**Verify** — A parent built with `tools: ["subagent"]` launches a `worker` declaring
`read, bash, edit`; the child's `--tools` carries all three. A `reviewer` launch from the same parent
succeeds.

## SUBA-115 — Nested control cascades escape the issuing subtree

**Kind** upstream-drift · **Severity** high · **Effort** S · **Confidence** confirmed (both sides read) · **CLOSED 2026-09-24** (`7ba9e03`): `background/cascade.rs::is_control_descendant` ports `isNestedControlDescendant`; `settle.rs::cascade_to_descendants` passes this run's id as issuer when `nested_self` is set, for stop, interrupt and timeout. Verify: `background::cascade::tests::a_nested_issuer_reaches_only_its_own_subtree` (red with the filter removed). Reach note: cyrup does not yet mint a nested route for its own children (`exec/spawn_plan.rs:993`), so today the cascade only has a route when cyrup was itself launched under an inherited one.

**cyrup** — `background/cascade.rs::cascade_to_nested_async_descendants` (`:167-230`) projects the
whole registry of the ROOT route (`project_nested_events_in(&roots.nested_events(), route)`),
flattens every descendant (`flatten_nested_runs`, `:117-131`) and delivers the verb to every
`running`/`queued` one (`is_live_state`, `:137`). It never consults this run's own address, although
the runner carries it: `RunnerConfig::nested_self` (`background/runner_main/config.rs:297`), set in
production at `extension/executor/background.rs:805`. Called from `runner_main/settle.rs:481-491` for
stop, interrupt and timeout.

**upstream** — `bd03854a` (#2243, v0.68.0): `subagent-runner.ts:2384-2385` @v0.71.0 defines
`isNestedControlDescendant = (run) => !config.nestedSelf || run.path.some((entry) => entry.runId === id)`
and applies it in all three fan-outs (`:2401`, `:2432`, and the stop loop). Root-wide control is kept
for a root run (`nestedSelf` absent). The commit's test drives stop/interrupt/timeout from `A` and
asserts sibling tree `B`/`B1` is untouched.

**Impact** — A nested child that is stopped, interrupted or times out stops or pauses every live run
under the same root, including sibling subtrees it never launched. Their work is lost or parked for
no reason, and nothing tells the user why.

**Fix** — In `cascade_to_nested_async_descendants`, take `nested_self: Option<&NestedParentAddress>`
plus this run's id and skip any `run` whose `path` does not contain this run id when `nested_self` is
`Some`. Thread it from `settle.rs::cascade_to_descendants`.

**Verify** — Seed a registry with `root→{A→A1, B→B1}`; a cascade issued by `A` (with `nested_self`)
writes control files into `A1` only; issued by the root, into all four.

## SUBA-116 — A paused async run cannot be stopped

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed (both sides read)

**cyrup** — `background/control.rs::stop` refuses anything not `Running`/`Queued` (`:950-952`,
`StopOutcome::NotStoppable`), and its doc (`:851-855`) says a stop of a `Paused` run is an error "as
upstream". A paused run keeps its active-capacity slot (`background/active_async_capacity/inspect.rs:266-273`,
pinned by `tests.rs::a_paused_run_keeps_its_slot`).

**upstream** — `03b92ee7` (#2176, v0.68.0). `src/runs/foreground/async-stop-action.ts:120-121`
@v0.71.0 admits a whole-run stop of a `paused` run, and `sealPausedRun` (`:31-101`) — after checking
the runner's process-terminal proof and the paused result's identity — rewrites status and result to
`stopped`, marks paused/pending steps stopped, and releases the active-run index. The runner side
(`subagent-runner.ts` `stopRunner`) now also acts on `paused` and converts interrupted results to
stopped.

**Impact** — A run the user paused and no longer wants can only be resumed or left alone. It stays
resumable and holds a capacity slot, so it can block new async launches when capacity is tight.

**Fix** — Port `sealPausedRun` into `control.rs::stop` for the whole-run, `Paused` case (child-scoped
stop stays pending/running-only), gated on the runner's observed process-terminal proof; release the
capacity slot and update the active-run index to `stopped`.

**Verify** — Interrupt a run, wait for `Paused`, `stop` it: status and result read `stopped`,
`resume` refuses it, and the capacity slot is released.

## SUBA-117 — The worktree clean check counts the crate's own project directory as dirt

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed (both sides read; not observed live) · **CLOSED 2026-09-27**: upstream's `probeWorktreeSource` excludes its own directory from the status call: `await tx.gitChecked(toplevel, ["status", "--porcelain", "--", `:!${PROJECT_SUBAGENTS_RELATIVE_DIR}`])` (`src/runs/shared/worktree.ts:351` @v0.71.0, read at the tag, under the comment *"pi-subagents writes durable runtime state under .pi/subagents/ by default; that state must not make managed isolation unusable for later runs"*, with the same refusal string cyrup carries). Ported at `crates/cyrup-ext-subagents/src/spawn/worktree.rs:495-498`: `resolve_repo_state` builds `format!(":!{}", crate::artifacts::PROJECT_ARTIFACT_ROOT)` and passes `["status", "--porcelain", "--", &artifact_root_exclude]`, with `:491-493` recording that the pathspec resolves against the TOPLEVEL while `PROJECT_ARTIFACT_ROOT` is `<cwd>`-relative — upstream's own asymmetry, kept rather than silently corrected. The per-worktree status probes are left alone (`:1519`), as upstream does. A chain run, refinement, schedule or cleanup plan under `.cyrup-subagents/` no longer makes a later `worktree: true` run refuse as dirty, so the user no longer has to discover and git-ignore that directory. Verify: `cyrup-ext-subagents spawn::worktree::tests::create_worktrees_ignores_the_project_artifact_root` (real temp git repo, state written under the artifact root), with the untracked-file-elsewhere refusal still asserted alongside.

**Window note.** This is in-baseline (upstream has the exclusion at v0.43.0), so by scope it is
`09`'s. It is filed here because this pass read both sides and `09` was not open for items in this
pass; ids never move.

**cyrup** — `spawn/worktree.rs::resolve_repo_state` runs `git status --porcelain` over the whole
toplevel (`:489-495`) and refuses with "worktree isolation requires a clean git working tree". The
crate writes its own project state under `<cwd>/.cyrup-subagents/` (`artifacts.rs:42,162-176`): chain
runs by DEFAULT (`resolve_chain_runs_dir`'s `project` arm, used by `extension/tool/task_items.rs:812`),
agent refinements (`exec/agent_refinements.rs:254`), schedules (`background/scheduled_runs/store.rs:137`)
and cleanup plans (`spawn/cleanup_plan/model.rs:421`). Nothing writes a `.gitignore` for it.

**upstream** — the same check excludes its own directory: `["status", "--porcelain", "--", ":!.pi-subagents"]`
at v0.43.0 (`src/runs/shared/worktree.ts:140`), `:!${PROJECT_SUBAGENTS_RELATIVE_DIR}` at v0.47.1
(`:141`) and v0.71.0 (`:351`, with the comment that this state "must not make managed isolation
unusable for later runs").

**Impact** — After a chain run (or a refinement, schedule or cleanup plan) in an untracked-clean
repository, a later `worktree: true` run is refused as dirty, and committing or stashing does not
help. The user must discover and git-ignore `.cyrup-subagents/`.

**Fix** — Append `"--", ":!.cyrup-subagents"` (the `PROJECT_ARTIFACT_ROOT` constant, relative to the
toplevel) to the status call in `resolve_repo_state`. Leave the per-worktree status probes alone, as
upstream does.

**Verify** — In a temp repo, create `.cyrup-subagents/chain-runs/x`, then `create_worktrees`
succeeds; an untracked file elsewhere is still refused.

## SUBA-118 — Abort recovery (resume once after a compaction-induced abort) is unported

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed (both sides read)

**cyrup** — `exec/fallback.rs:1336-1338` names `AbortRecovery` in its "UNPORTED upstream kinds" list;
`ABORT_RECOVERY`, `after_compaction` and `compaction_settlement` have zero hits in the crate. The child
stream already carries `compaction_start` (`exec/ndjson.rs:199,267`), so the evidence exists.

**upstream** — `src/runs/shared/abort-recovery.ts:3,67-116` @v0.71.0 (entered in `v0.57.0..v0.67.0`
via `40b20b70`, `feb24a40`; still live after the v0.68.0 ladder removal). `planAbortRecovery` resumes
the retained session ONCE with `ABORT_RECOVERY_PROMPT` only when: a session file exists, no stop /
interrupt / timeout / budget / structured-output failure / in-flight tool / unresolved tool call, the
child settled after compaction (`afterCompactionSettlement`), the terminal assistant message is an
empty, zero-output abort (`stopReason: "aborted"` or a provider-abort error), and there was useful
progress. Otherwise it settles, with the diagnostic `Compaction-induced child abort could not be
resumed safely: <reason>.` when the compaction case applies. Sites: `subagent-runner.ts:1341-1362`
and `execution.ts:1855-1903` (two-attempt loop); the runner also appends "Child failure followed
session compaction and agent settlement." (`formatChildFailureDiagnostic`, `:490-510`).

**Impact** — A long child that auto-compacts and then hits a provider/transport abort fails, and
its work so far is reported as a failed run. Upstream resumes it once on the same model.

**Fix** — Track `compaction_start` / `compaction_end{willRetry}` / `agent_settled` in the attempt
driver to derive `after_compaction_settlement`, port `planAbortRecovery` verbatim, and in `run_sync`
allow ONE `--session <file>` re-dispatch with the recovery prompt when it says `resume`. Carry the
settle diagnostic into the error.

**Verify** — A fake child that emits `compaction_start`, `agent_settled`, then an empty aborted
assistant message after a successful tool call is re-launched once with the recovery prompt and the
run succeeds; without the compaction events it settles with no re-launch.

## SUBA-119 — A child served by a different model than requested is accepted silently

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed (both sides read) · **STILL OPEN 2026-09-27**: everything else in the row is closed; TWO items remain. (1) THREE of the five production wiring seams have NO test that fails without them, and they are exactly the ones carrying the background/detached path the descriptor half is about. Reverting all three simultaneously — `crates/cyrup-ext-subagents/src/extension/executor/chain.rs:171` (drop `.with_model_response_aliases(cfg.model_response_aliases.clone())`), `crates/cyrup-ext-subagents/src/extension/executor/background.rs:802-803` (`requested_model_response_aliases.and(None)`, so `RunnerConfig.model_response_aliases` is never populated at the launch site) and `crates/cyrup-ext-subagents/src/background/runner_main/turn_loop.rs:400` (`config.model_response_aliases.clone().and(None)`, so it never reaches `ExecSingleStepExecutor`) — leaves the full suite passing 4767/4767, and `grep -rn model_response_aliases --include=*.rs crates/ | grep -v ": None"` is empty outside `crates/cyrup-ext-subagents/src`, so with those three lines dead a background/detached child gets no declared alias at all and the descriptor always carries `None` in production, silently. The two green tests only cover hops DOWNSTREAM of the seam they sit at (`executor.rs:1706` sets `runner.model_response_aliases` by hand; `recovery_descriptor.rs`'s test sets `config.model_response_aliases` by hand) — a test that hands the function its own input cannot notice that nothing in production supplies it. Needed: a test at the `spawn_background_steps` seam that a declared `cfg.model_response_aliases` lands on `RunnerConfig` (and so on the descriptor `for_single_launch` writes), one at the `turn_loop` seam for the `RunnerConfig` → `ExecSingleStepExecutor` hop, and one at the `chain.rs` construction seam — each proven red by reverting its OWN line. (2) The revive does not reproduce `subagent-executor.ts:2136`; it is adjacent and MORE PERMISSIVE. `background.rs:802-803` is `requested_model_response_aliases.or_else(|| cfg.model_response_aliases.clone())` and `BackgroundStepsSpec.model_response_aliases` is a single `Option`, so "a revive whose descriptor declared nothing" is indistinguishable from "an ordinary launch" — `requests.rs:534`'s own doc admits it ("`None` from every ordinary producer, which takes the LIVE `config.json` value"), and `control.rs:598` hands over `descriptor.model_response_aliases.clone()`, which is `None` for every run launched before an alias was declared. Upstream is `recoveryDescriptor ? recoveryDescriptor.modelResponseAliases : foregroundContract?.modelResponseAliases`, whose revive never touches `deps.config.modelResponseAliases`, under the comment *"Absence in the retained contract is meaningful; never acquire current aliases"* — the descriptor's absence is LOAD-BEARING, not a hole to fill. Concrete divergence: a router starts substituting, run A fails `model_verification_failed`, the operator declares the alias in `config.json` and revives A — pi still fails A (a new independent run is required), cyrup now passes it. That leaves the shipped error text's own sentence ("Configuration changes affect new independent native runs; resumed native runs retain their launch-time declaration") still FALSE in exactly the case DEFECT 2 was refused over, and the code comment at `background.rs:800-801` asserting parity with `:2136` is not something the `or_else` has. Fix: let the spec distinguish a retained contract from no contract (`Option<Option<ModelResponseAliases>>`, or a separate `retained_model_response_aliases: bool`/enum set only by `control_resume`) so a revive uses the descriptor's value verbatim INCLUDING its absence, plus a test that a revive whose descriptor declares nothing gets `None` on the runner config while `cfg.model_response_aliases` is `Some`.

**Window note.** The verification entered in `v0.47.1..v0.57.0` (`model_verification_failed` is in
`src/runs/shared/model-fallback.ts` at v0.57.0, absent at v0.47.1); `modelResponseAliases` entered in
`v0.57.0..v0.67.0`. `09a`'s pass over its own window missed the first half.

**cyrup** — no counterpart: `model_verification_failed`, `modelResponseAliases` and
`response_alias` have zero hits in production code; `modelResponseAliases` is only listed in
`registration/mod.rs::UNPORTED_CONFIG_KEYS` (`:525-553`, `SUBA-113`). The child's reported model
reaches `progress.model` and the result without comparison.

**upstream** — `src/runs/shared/model-resolution.ts:14-34` @v0.71.0: when the launch model was chosen
by the child's own configuration (`verifyModel = Boolean(candidate) && !options.modelOverrideFromParent`,
`execution.ts:1836`), each non-tool-call assistant message's `model` is compared with the candidate
(base id, registry `id`, and the id leaf all accepted) and a mismatch not declared in
`config.modelResponseAliases[expected]` fails the run with `model_verification_failed: native Pi
child reported a different model than the launch candidate. Expected '…' but observed '…'.`
(`execution.ts:1100-1102`, `run-child-session.ts:513`). The alias map is validated at config load
(`extension/config.ts:176`) and carried in recovery descriptors.

**Impact** — A provider or router that silently substitutes a different model (a fallback tier, an
alias that moved) produces a "successful" run on a model nobody chose. Upstream fails it loudly.

**Fix** — Port `formatSubagentModelVerificationError` and apply it in the attempt driver on each
assistant message when the model came from the agent/config; add `modelResponseAliases` to
`config.json` parsing (leave `UNPORTED_CONFIG_KEYS`) and to the recovery descriptor.

**Verify** — A fake child reporting `other/model` for a launch of `prov/model` fails with the
upstream text; the same run with `modelResponseAliases: {"prov/model": ["other/model"]}` succeeds.

## SUBA-120 — Watchdog findings have no importance, so all of them reach the model

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed (both sides read) · **CLOSED 2026-09-27**: **Items 1, 2 and 4 of the Fix were already in the tree and independently proven** (`importance` required with no default at `exec/acceptance/.../review.rs:326`/`:336` with rejected-not-defaulted tests at `:1241`/`:1257`; `route_warning` at `watchdog/runtime.rs:1477-1485`, the port of upstream's `:548-550`; both usage strings updated). This pass fixed the record and one predicate. (1) `crates/cyrup-ext-subagents/src/watchdog/types.rs:118-148` no longer claims `notify.ts` has no importance filter. Read at the tag: there are FOUR importance filters upstream, one ported and three not — `collectWatchdogFindings` inside `buildSubagentNotifyPayload` is `if (warning.importance !== "high") continue;` (`src/runs/background/notify.ts:716` @v0.71.0) with `:743` rendering the survivors into the completion notice's `resultPreview` under `High-importance watchdog concerns:`; `childWatchdogProgressForModel` is `.filter((warning) => warning.importance === "high")` (`src/watchdog/child-status.ts:223`); and `withAggregatedToolUsage` appends the surviving findings to the subagent tool RESULT (`src/runs/foreground/subagent-executor.ts:3021`). A tag-wide grep for BOTH spellings returns exactly those four and nothing more, so the new paragraph's enumeration is exhaustive and every line number in it resolves. The paragraph also carries the explicit correction of the old claim WITH the reason the narrow grep missed `:716` (that site is spelled `!== "high"`), and a MUST for whoever ports the projection. The stated reason the three filters stay unported is established, not asserted: all three read `ChildWatchdogProgress.warnings`, and cyrup's `ChildWatchdogStateSnapshot` (`watchdog/child_status.rs:176-193`) has `phase`, `seq`, `last_update`, `follow_up_pending`, `reason` and `timed_out` and NO `warnings` field, while `grep -rn WatchdogWarning crates/cyrup-ext-subagents/src/ | grep -v '/watchdog/'` returns nothing — so there is no list to filter at any of the three sites and adding the filter without the field would be dead code. **What is missing is the PROJECTION, not the filter**, which is a finding about the row rather than a shortfall in the change: the row's third Fix item ("filter the notice and tool-result projections to high") cannot be satisfied as written because those projections do not exist in cyrup, and that is now written down at the type with a MUST instead of being denied by a false claim. The `CYRUP-DELTA` label was correctly DROPPED from that paragraph — an unported behaviour is never a delta — while the genuine delta below it keeps its label: upstream's `warningMeetsThreshold` (`runtime.ts:543-545`) re-validates `warning.importance` with `WATCHDOG_WARNING_IMPORTANCES.includes(...)` because a TypeScript string can carry an out-of-union value at runtime, which a Rust enum makes unrepresentable — a mechanism difference at identical behaviour for every well-typed input. (2) `parse_test_command` (`watchdog/register_main.rs:638`, guard at `:662`) now requires a non-empty remainder after consuming ONE separator character, which is what upstream's `^test\s+(concern|blocker)\s+(low|medium|high)\s+([\s\S]+)$` (`register-main.ts:158`) means: `\s+` is greedy but backtracks and `[\s\S]` matches whitespace, so TWO trailing spaces DO match with a whitespace capture that `.trim()` at `:160` empties, while ONE leaves the capture empty and the match fails — which is what selects the specific usage error over the general one. Proven red-without by reverting the guard to `&& true`: `1 test run: 0 passed, 1 failed`, `left: Some((Blocker, High, ""))` / `right: None`; restored, and the whole watchdog scope is 339/339. The neighbouring inputs (`test blocker `, `test `, `test blocker  `, `test concernx`, `test blocker lowish x`) were checked against the regex too and the port agrees on all of them. The function's doc records the honest scope note that both sides' handlers `args.trim()` first (`register-main.ts:249`), so no slash-command input reaches the function with trailing whitespace and the divergence was at the function contract, not at the operator. Scope: the reconcile diff is 42 lines in `types.rs` and 51 in `register_main.rs`, all under `crates/cyrup-ext-subagents/src/watchdog/`, with no `#[ignore]`/`should_panic` added. Verify: `cyrup-ext-subagents watchdog::register_main::tests::exactly_one_trailing_space_is_not_a_test_command_but_two_are`.

**cyrup** — `watchdog/types.rs:104` still has `WatchdogConfidence { Medium, High }` and
`WatchdogWarning::confidence` (`:303-305`); `watchdog/review.rs:346-348` advertises `confidence` on
`watchdog_warn`. There is no `importance` anywhere under `watchdog/`, and delivery steers every
threshold-passing finding into the parent (`WatchdogDelivery::Steer`, `watchdog/types.rs:276-282`,
`runtime.rs:1480`).

**upstream** — `52fc6b4b` (*feat: route watchdog findings by importance*, v0.68.0). `watchdog_warn`
requires `importance: low|medium|high` (`src/watchdog/review.ts:30` @v0.71.0; `confidence` removed);
`runtime.ts:548-550` `routeWarning` sends only `high` to the model (`displayWarning`) and appends
`low`/`medium` as a user-only session entry (`displayUserWarning`); completion notices and tool
results project only high-importance findings (`notify.ts` `collectWatchdogFindings`,
`subagent-executor.ts` `withAggregatedToolUsage`); `/subagents-watchdog test` takes an importance.

**Impact** — Every watchdog concern steers the parent model and costs a turn; upstream keeps low and
medium findings visible to the user and out of the model's context.

**Fix** — Replace `confidence` with a required `importance` in `WatchdogWarning`, the tool schema and
the prompt text; route by importance in `runtime.rs` (user-only entry for low/medium); filter the
notice and tool-result projections to `high`; update the test command's grammar.

**Verify** — A review emitting one `low` and one `high` finding steers only the `high` one; the `low`
one is rendered as an entry and absent from the next model request.

## SUBA-121 — Runtime-registered agents never receive settings defaults or overrides

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed (both sides read) · **CLOSED 2026-09-27**: No behavioural defect was alleged in the ported logic and none was changed: the narrowing (`runtime_agent_overrides`, four fields), the fill-if-unset defaults and the `defaultExtensions` exclusion still match upstream, read at the tag — `runtimeAgentOverrides` keeps only `model`, `defaultProvider`, `fast` and `thinking` and drops an entry that narrows to empty, and `applyRuntimeAgentSettings` runs `applySubagentDefaultModel` then `applySubagentDefaultThinking` then `applyCustomAgentOverrides(withDefaults, runtimeAgentOverrides(user), runtimeAgentOverrides(project), …)` and NEVER `applySubagentDefaultExtensions` (`src/agents/agents.ts:1596-1627` @v0.71.0); cyrup's `discovery/merge.rs:385-407` narrows the identical four fields with the identical empty-drop and `:438-465` runs the three passes per agent, which is equivalent because every pass is per-agent independent. What was missing was a test through the real seam: all seven SUBA-121 tests lived in `discovery::merge::tests` and called `merge::apply_runtime_agent_settings` on a hand-built `Vec`, so the ONE line that makes settings reach a runtime-registered agent — `crates/cyrup-ext-subagents/src/discovery/mod.rs:2074-2076`, `if let Some(runtime_slice) = agents.get_mut(before_runtime..) { merge::apply_runtime_agent_settings(runtime_slice, &cfg.override_settings); }` — was covered by nothing. It now is: `crates/cyrup-ext-subagents/src/tests/runtime_agent_registration_integration.rs:601` builds a real `tempfile::tempdir()` user tree, reads a real `settings.json` through `crate::discovery::load_layered_override_settings`, registers one agent through `RuntimeAgentRegistry`, and drives `discover_agents(&cfg, None)`, asserting on the DISCOVERED agent that `model == "p/m"`, `model_source == SettingsDefault` and `thinking == Some("high")`. Red-without was reproduced twice with two independent sabotages: replacing line 2075 with `let _ = runtime_slice;` fails at `:637` with ``"`subagents.defaultModel` must reach a model-less runtime agent" / left: None / right: Some("p/m")``, and disabling the narrowing in `merge.rs::runtime_agent_overrides` fails at `:661` with ``"the override narrowing keeps `tools` out: Some([Builtin("Read")])"`` — so the NEGATIVE assertions have teeth and are not vacuous, which matters because the same `agentOverrides.runtime-scout` entry is proven to have arrived by the POSITIVE `thinking == Some("high")` assertion in the same test. The `defaultExtensions` exclusion is likewise paired with a CONTROL: a second, on-disk agent `disk-scout` at the same scope IS asserted to take both `defaultModel` and `defaultExtensions`, which rules out "discovery silently applied nothing to anyone" — the failure mode this row is about. All three of the row's Verify clauses are now asserted THROUGH discovery, with `defaultProvider`/`defaultThinking`/`fast` still covered by the seven `discovery::merge::tests` unit tests already confirmed red-without and at parity. Scope: one file touched (+114 lines and one `use cyrup_core::ModelId;`), no production code changed by this pass, nothing skipped or quarantined. No `CYRUP-DELTA` was added; the one in the diffed discovery files belongs to `SUBA-123` (`discovery/types.rs`, `subagent_only_extensions_from_default`) and correctly records a mechanism at full parity. Verify: `cyrup-ext-subagents tests::runtime_agent_registration_integration::settings_defaults_and_narrowed_overrides_reach_a_runtime_agent_through_discovery`; 19/19 for `-E 'test(runtime_agent)'`. (Caveat recorded for the next reader, NOT attributable to this row: a crate-wide run during this window transiently showed `discovery::agent_scan_and_exclude_dir_tests::a_malformed_default_subagent_only_extensions_aborts_discovery_with_upstreams_text` failing, which was a CONCURRENT lane's in-flight revert in `discovery/merge.rs`; it and its four siblings pass once that lane restored, so no single full-suite number from that window is reliable.)

**cyrup** — `discovery/mod.rs::run_discovery` appends runtime agents AFTER `merge::discover_and_merge`
has applied the settings (`:1826-1846`), and `discovery/runtime_registry.rs::merge_runtime_agents`'s
doc says so: they "never take part in the four-tier precedence merge and never receive settings
overrides" (`:1249-1256`).

**upstream** — `bbb30096` (#2369, v0.70.1). `applyRuntimeAgentSettings` (`src/agents/agents.ts:1619-1627`
@v0.71.0) applies `defaultModel`/`defaultProvider`/`defaultThinking` and a narrowed `agentOverrides`
(`runtimeAgentOverrides`, `:1597-1610`: `model`, `defaultProvider`, `fast`, `thinking` only).

**Impact** — A runtime agent silently runs on the parent session's model even when the user
configured a default model, provider or thinking level for subagents, or an override for that agent.

**Fix** — After `merge_runtime_agents`, apply the same default-model/provider/thinking passes and an
override pass restricted to those four fields.

**Verify** — With `subagents.defaultModel: "p/m"`, a runtime agent without a model launches on `p/m`;
`agentOverrides.<name>.thinking` reaches it; `agentOverrides.<name>.tools` does not.

## SUBA-122 — A canonical agent name does not win over a packaged short name

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed (both sides read) · **CLOSED 2026-09-27**: upstream `resolveAgentName` tries canonical `name` matches first — one hit wins, several go through `effectiveAgentMatch` and otherwise raise `Ambiguous agent name '<name>': …` — and only when NONE matched does it try `localName`, with its own `Ambiguous local agent name '<name>': …` (`src/agents/agents.ts:707-724` @v0.71.0, read at the tag; the alias pass follows at `:725`). Ported at `crates/cyrup-ext-subagents/src/discovery/mod.rs:578-613`: the single combined `a.name == raw || a.local_name == raw` filter is gone; `:595` is the canonical ambiguity message and `:600`/`:611` the local pass and its distinct message, with `:557-566` documenting the three passes and the specific case the old code got wrong. A user agent `scout` beside an installed package agent `code-analysis.scout` whose local name is `scout` is no longer ambiguous — the canonical pass resolves it — and `scout` still resolves to the package agent when it is the only one. Verify: `cyrup-ext-subagents discovery::tests::a_canonical_name_beats_another_agents_local_name_of_the_same_string` and `…::two_distinct_agents_sharing_a_local_name_are_a_local_ambiguity_error`, the second pinning that a real local collision raises the LOCAL message, not the canonical one.

**cyrup** — `discovery/mod.rs::resolve_agent_name` (`:568-591`) collects `a.name == raw || a.local_name == raw`
into one set; two hits with different names make `effective_agent_match` (`:517-533`) return `None`,
and the call fails with `Ambiguous agent name '<name>'`.

**upstream** — `src/agents/agents.ts:709-724` @v0.71.0 (v0.68.0, #2214): canonical `name` matches are
tried first; only if none match are `localName` matches tried, with their own
`Ambiguous local agent name` error.

**Impact** — A user agent `scout` beside an installed package agent `code-analysis.scout` makes
`scout` unusable by name.

**Fix** — Split the first pass into a canonical pass and a local-name pass, in that order, each with
its own ambiguity message.

**Verify** — Agents `scout` (user) and `code-analysis.scout` (package, local `scout`): `scout`
resolves to the user agent; with only the package agent, `scout` resolves to it.

## SUBA-123 — Three top-level subagent settings keys are silently dropped

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed (both sides read) · **CLOSED 2026-09-27**: **All three sub-parts of this row are closed together, because one change covers them:** `agentScanDirs` (SUBA-123a), `defaultSubagentOnlyExtensions` (SUBA-123b) and `agentExcludeDirs` (SUBA-123c). Upstream validates all three with the SAME rule and the same message shape — `if ("<key>" in subagentsObject)`, refused when `!Array.isArray(...) || some(item => typeof item !== "string" || !item.trim())`, then `.map(item => item.trim())` (`src/agents/agents.ts:1218-1241` @v0.71.0, read at the tag: `defaultSubagentOnlyExtensions` at `:1219-1225`, `agentScanDirs` at `:1226-1233`, `agentExcludeDirs` at `:1234-1241`) — with `agentScanDirs` adding discovery roots (`readConfiguredAgentScanDirs` `:2392`, `settingsAgentScanDirs` `:2948`), `agentExcludeDirs` removing trees including nested plugin sources and symlink aliases, and `defaultSubagentOnlyExtensions` filling `subagentOnlyExtensions` for agents that set none (`resolveSubagentDefaultSubagentOnlyExtensions` `:1392-1399`, `applySubagentDefaultSubagentOnlyExtensions` `:1401-1410`). cyrup: the three keys are typed on the settings struct (`crates/cyrup-ext-subagents/src/discovery/types.rs:955-974`, with `agent_scan_dirs` and `default_subagent_only_extensions` each carrying their own doc block rather than one orphaned above the other), validated by ONE shared loop rather than three near-copies (`discovery/mod.rs:1342-1366`, whose `:1345-1348` records why `defaultSubagentOnlyExtensions` joins it), trimmed at `:962-966` and `:955-958`, resolved per scope at `:1500-1501`, and applied through `discovery/agent_dirs.rs:186` (`is_excluded`, reached from `mod.rs:844-845` and `:1607`, and consulted at every scan tier `:221`/`:230`/`:251`/`:269`) and `:277` (`settings_agent_scan_dirs`). The row's fourth Fix clause is done too: `override_key_warnings` (`mod.rs:1283-1288`) now runs over the top-level `subagents` object, so the next unported key warns instead of vanishing — which is the half the file's own earlier lead got wrong. Three reverts were each proven red independently (drop the validator key → the malformed-shape test; `let _ = list;` for the trim → the trim test; drop `project_settings_path.is_some() &&` → the pathless-project-scope test), each re-checked with a post-run presence check because a concurrent lane was rewriting the same files. The one `CYRUP-DELTA` in the region is a mechanism note at full parity, verified at the tag rather than taken on faith: `withDeclaredExtensionPaths` (`agent-management.ts:238-248`) re-reads the agent file with `parseFrontmatter(fs.readFileSync(filePath))` and restores only the frontmatter literals, and neither it nor `readAgentFrontmatterFields` (`:330-337`, called from `editableAgentConfig` `:250`/`:299`/`:327`) consults the `agentFrontmatterFields` WeakMap that the merge pass maintains (`agents.ts:1406-1407`) — cyrup's `management::handlers` `editable_base` (`handlers.rs:146-148`) clears the list and the provenance flag and reaches the same answer without the file read. A configured exclusion now actually hides agents under it, configured scan dirs and default child-only extensions take effect, and a bad value aborts discovery with upstream's text. Verify: `cyrup-ext-subagents discovery::agent_scan_and_exclude_dir_tests::{agent_exclude_dirs_hides_an_agent_under_an_excluded_root,agent_exclude_dirs_hides_an_agent_reached_through_a_symlink_alias,agent_scan_dirs_adds_a_discovery_root,agent_scan_dirs_expands_one_wildcard_segment,a_two_wildcard_agent_scan_dir_entry_contributes_nothing,a_scan_dir_sits_after_the_extras_and_before_the_ordinary_user_dirs,a_non_array_agent_scan_dirs_aborts_discovery_with_upstreams_text,a_malformed_default_subagent_only_extensions_aborts_discovery_with_upstreams_text,default_subagent_only_extensions_entries_are_stored_trimmed}`, `discovery::merge::tests::{default_subagent_only_extensions_fills_only_agents_that_declared_none,a_project_default_subagent_only_extensions_beats_the_user_one,a_project_default_subagent_only_extensions_needs_a_project_settings_path,an_explicitly_empty_subagent_only_extensions_is_not_overwritten_by_the_default,a_default_supplied_subagent_only_extensions_is_not_written_back_by_editable_base}` and `discovery::tests::{an_unknown_top_level_subagents_key_warns,a_ported_top_level_subagents_key_does_not_warn}`; crate 4767/4767.

**Correction to this file's earlier lead.** The 2026-09-24 first pass said these keys "would reach
the key census as *unknown*, which gives a warning". That is false for top-level keys:
`override_key_warnings` (`discovery/mod.rs:1179-1203`) censuses only `agentOverrides.<name>`, and
`parse_subagent_settings` (`:837-856`) deserializes the rest with serde's tolerance.

**cyrup** — `agentScanDirs`, `agentExcludeDirs` and `defaultSubagentOnlyExtensions` have zero hits in
the crate; the settings struct ignores them without a word.

**upstream** — `src/agents/agents.ts:1219-1241` @v0.71.0 validates all three (arrays of non-empty
strings, a named error otherwise). `agentScanDirs` adds discovery roots (`readConfiguredAgentScanDirs`,
`:2392`; `settingsAgentScanDirs`, `:2948`); `agentExcludeDirs` (v0.68.0, #2131) removes trees from
discovery, including nested plugin sources and symlink aliases; `defaultSubagentOnlyExtensions`
(v0.70.0, #2284) fills `subagentOnlyExtensions` for agents that do not set it.

**Impact** — A configured exclusion is ignored, so agents the user excluded are still discovered and
can shadow or collide by name; configured scan dirs and default child-only extensions have no effect.
None of it is reported.

**Fix** — Parse and validate the three keys with upstream's messages; apply scan and exclude roots in
`scan_agent_tiers_scoped`, and default `subagent_only_extensions` after the existing
`default_extensions` pass. Separately, run the key census over the top-level `subagents` object so the
next unported key warns instead of vanishing.

**Verify** — An `agentExcludeDirs` entry hides an agent under it; a bad value aborts discovery with
upstream's text; an unknown top-level key produces a warning.

## SUBA-124 — `mcpDirectTools` cannot reach package or agent-plugin MCP servers

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed (both sides read)

**cyrup** — `exec/mcp_direct_tools.rs::load_mcp_config` / `get_config_paths` (`:514-541`) read only
the mcp.json files and their imports. `resolve_direct_tool_names` (`:662-700`) skips any selection
whose server is not in that config, with no warning. The MCP adapter itself does load plugin servers
(`crates/cyrup-mcp/src/agent_plugin.rs`), so the servers exist at runtime and the direct-tool
resolver cannot name them.

**upstream** — `src/runs/shared/mcp-config-sources.ts` @v0.71.0 (new in `v0.57.0..v0.67.0`):
`loadPackageMcpServers` (`:83-118`, a settings `packages` entry's `package.json` `pi.mcp` config
paths, servers named `<package>__<server>`) and `loadAgentPluginMcpServers` (`:120-146`,
`agentPluginPaths` → `plugin.json` + `mcp.json`, named `<plugin>__<server>`), merged in
`mcp-direct-tool-allowlist.ts:253-254`.

**Impact** — A child granted direct tools from a package- or plugin-provided MCP server gets none of
them, silently. The child then fails at the task or improvises with other tools.

**Fix** — Port both loaders (reuse `cyrup-mcp`'s plugin reader where the shapes agree) and merge
their servers under upstream's names before direct-tool resolution. Coordinate with `13c`, which owns
the adapter's config sources.

**Verify** — A plugin at an `agentPluginPaths` entry with `mcp.json` server `db`, and a metadata cache
for `<plugin>__db`: `mcpDirectTools: ["<plugin>__db"]` resolves its tools.

## SUBA-125 — Model-hidden skills are still injected into children

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed (both sides read) · **CLOSED 2026-09-27**: upstream filters at the injection chokepoint — `const visibleSkills = skills.filter((skill) => !skill.disableModelInvocation);` with `if (visibleSkills.length === 0) return "";` (`src/agents/skills.ts:712-713` @v0.71.0, read at the tag, under the comment that this is *"the chokepoint every prompt-injection path funnels through, so filtering here closes the leak regardless of which caller assembled the list"*) — and `proactive-skills.ts:146` stops suggesting them. cyrup already had the flag on `cyrup_resources::Skill` (`crates/cyrup-resources/src/skill.rs:38`) but nothing in this crate read it; it is now carried through `crates/cyrup-ext-subagents/src/discovery/skills.rs:78` and `:96-100` (on both the discovered and resolved shapes), populated at `:158` and `:274`, and filtered at the injection chokepoint at `:302` (`.filter(|skill| !skill.disable_model_invocation)`) — upstream's placement, so a skill a named-skill path resolved is still dropped. The proactive recommender skips them at `:571`. A skill its author marked `disable-model-invocation: true` no longer reaches a child's prompt when an agent lists it, so the declared restriction no longer fails open. Verify: `cyrup-ext-subagents discovery::skills::tests::{an_agent_naming_a_hidden_skill_launches_without_it_in_the_child_prompt,build_skill_injection_omits_a_hidden_skill,a_hidden_skill_is_not_proactively_recommended}` — the first covering the row's Verify clause that a normal skill beside the hidden one is still injected.

**cyrup** — `discovery/skills.rs::resolve_skills_in` (`:156-190`) resolves each named skill from
`discover_skills` (`:122`), whose `cyrup_resources::Skill` already carries `disable_model_invocation`
(`crates/cyrup-resources/src/skill.rs:38`). Nothing in `cyrup-ext-subagents` reads the flag, so a
hidden skill named by an agent is resolved and injected.

**upstream** — `a1eaa728` (#2400, v0.71.0). `src/agents/skills.ts:712` @v0.71.0 filters
`visibleSkills = skills.filter((skill) => !skill.disableModelInvocation)` at the injection chokepoint
(so a requested hidden skill is dropped), and `proactive-skills.ts:146` stops suggesting them.

**Impact** — A skill its author marked `disable-model-invocation: true` (a user-only command, say)
reaches a child's prompt whenever an agent lists it. That is a declared restriction failing open.

**Fix** — Drop `disable_model_invocation` skills in `resolve_skills_in` (report them as missing, as
upstream's chokepoint does) and in the proactive-skill recommender.

**Verify** — An agent listing a hidden skill launches without it in the child prompt; a normal skill
beside it is injected.

## SUBA-126 — Structured-only answers are delivered and saved empty

> **CLOSED 2026-09-28.** (on `claude/lows-next`) A structured-only answer is saved, delivered and chained as the pretty-printed value; blank is measured after `strip_acceptance_report`, and acceptance still reads the raw prose (`exec/mod.rs:739-790`). Tests: `a_structured_only_answer_is_saved_and_delivered_as_pretty_json`, `prose_that_is_only_an_acceptance_report_still_delivers_the_structured_value`. The line numbers in the `cyrup` paragraph below describe the pre-fix code. Evidence is in the row.

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**cyrup** — `exec/mod.rs::resolve_saved_output` (`:1520-1560`, called at `:706`) persists the captured
prose, empty or not, BEFORE the structured value is read (`apply_structured_output`, `:723`), and
`assemble_delivered_output` never substitutes the value for empty prose.

**upstream** — foreground since v0.41.0: `src/runs/foreground/execution.ts:1515` @v0.71.0,
`if (!fullOutput.trim() && result.structuredOutput !== undefined) fullOutput = JSON.stringify(result.structuredOutput, null, 2);`;
background since v0.68.0 (`a525db4d`, #2163): `subagent-runner.ts:1373-1374`.

**Impact** — A child that finishes with only a `structured_output` call leaves an empty output file
and an empty delivered output (and an empty `{previous}` in a chain), although the answer exists.

**Fix** — Read the structured value first, and when the prose is blank substitute
`serde_json::to_string_pretty(value)` before `resolve_saved_output` and assembly.

**Verify** — A structured-only child with `output: "out.json"` writes the pretty JSON there and
returns it.

## SUBA-127 — Structured-output evidence is read only on a clean exit

> **CLOSED 2026-09-28.** (on `claude/lows-next`) The capture is read whenever `structured_output` was invoked, a valid value survives a failed or timed-out run, and a rejected call fails with the ported rejection summary (`exec/structured.rs:401,526`; `exec/mod.rs:1716-1777`). Tests: `a_rejected_structured_output_call_fails_with_the_rejection_summary`, `a_valid_structured_value_survives_a_later_failure`, `a_child_that_never_invokes_structured_output_gets_the_missing_call_error`. An interrupted run is still not read (upstream's runner reads it). Evidence is in the row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**cyrup** — `exec/mod.rs::apply_structured_output_value` reads the capture file only when
`self.gate().is_clean()` (`:1633`), and `exec/structured.rs::read_structured_output` (`:354-366`) maps
a missing file to `STRUCTURED_OUTPUT_MISSING_ERROR` whether or not the child called the tool.

**upstream** — v0.71.0 (#2407, #2411; `b9a0ce83`, `789c6856`): `execution.ts:1449-1471` reads the
value whenever the tool was invoked and keeps it as evidence even when the run later failed; a
rejected call gets `formatStructuredOutputRejectionError` (`structured-output.ts:62-80`, a bounded,
redacted summary of the latest failed `structured_output` result) instead of the missing-call error;
the runner mirrors this (`subagent-runner.ts:1248-1265`) and stops clearing the value on timeout or
stop.

**Impact** — A child whose output was rejected is told it never called the tool, which misdirects the
fix; a valid value produced before a provider error is lost.

**Fix** — Read the capture whenever the tool was invoked; set the missing-call error only when it was
not; port the rejection summary; keep the value on a failed run.

**Verify** — A child that calls `structured_output` with an invalid value fails with the validation
summary; a child that produced a valid value and then hit a provider error keeps `structured_output`.

## SUBA-128 — `checkpointBeforeDeadlineMs` is unported

> **CLOSED 2026-09-28.** (on `claude/lows-next`) Tool parameter, config key, async-single `params ?? config` resolution and the runner's `deadline-checkpoint` steer are all ported; the bounds refusal sits at the tool entry for every call shape (`extension/tool/mod.rs:254-264`, `background/runner_main/control_watcher.rs:630`). Test: `a_running_child_receives_the_deadline_checkpoint_and_hands_off_before_the_kill`, plus the runner, launch and config tests in the row. An invalid config still warns and defaults where upstream throws. Evidence is in the row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

Escalated from `SUBA-113`.

**cyrup** — only `registration/mod.rs:541` (`UNPORTED_CONFIG_KEYS`); no tool parameter, no runner
timer.

**upstream** — v0.68.0 (#2141): tool param `checkpointBeforeDeadlineMs` (`src/extension/schemas.ts:363`
@v0.71.0), config default validated at `extension/config.ts:145-151`, resolved at
`subagent-executor.ts:3460`; the runner arms a timer `remainingMs - checkpointBeforeDeadlineMs`
(≥ 1 s) and delivers a `source: "deadline-checkpoint"` steer asking the child to finish the current
tool call and reply with a handoff (`subagent-runner.ts:3383-3406`).

**Impact** — A long async run with a deadline is killed with no chance to report changed files and
remaining work. The config key warns; the per-call parameter is simply not advertised.

**Fix** — Add the parameter and config key; in the detached runner, arm the timer and route the steer
through the existing steer inbox with upstream's text.

**Verify** — A run with `timeoutMs: 10000, checkpointBeforeDeadlineMs: 5000` receives the checkpoint
steer about five seconds before the timeout.

## SUBA-129 — Typed gates and `preserveStagedIndex` are refused

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read)

**cyrup** — `workflows/scripted/engine.rs:1506-1511` accepts `gate` only as a non-empty string;
`exec/acceptance/model/types.rs::AcceptanceVerifyCommand` (`:177-191`) has no `output`/`schema`; and
`validate_input.rs:71-82` rejects `preserveStagedIndex` and `verify[].output` as unknown keys.

**upstream** — v0.69.0: `36eb7660` (typed gates, #2315): `parseGateInput`/`normalizeGateAcceptance`
(`src/runs/shared/acceptance.ts:172-236` @v0.71.0), the stdout contract (`:1259-1290`, empty,
truncated, non-JSON or schema-invalid output fails the gate), never memoized, conflict with
`outputSchema` refused (`TYPED_VERIFY_OUTPUT_SCHEMA_CONFLICT`, `:212`), and a passing value becomes
`structuredOutput` (`subagent-runner.ts:1458-1463`, `execution.ts:1998-2003`). `c29a1dcf` (#2280/#2286):
`preserveStagedIndex` (checked/verified only, `:282-287`) captures the launch index tree and checks
it unchanged instead of `no-staged-files` (`:1111-1148,1600-1602`).

**Impact** — Workflow scripts and calls written for current upstream fail validation. Nothing is
silently wrong; the features are absent.

**Fix** — Extend the verify command with `output: "json"` and `schema`, port the stdout contract and
the conflict rule, bridge the value into `structured_output`; port the staged-index baseline.

**Verify** — `gate: { command: "echo '{\"ok\":true}'", output: "json" }` yields
`structuredOutput: {"ok": true}`; invalid JSON fails the gate.

## SUBA-130 — Worktree git commands are unbounded

> **CLOSED 2026-09-30** (on `claude/lows-batch4`): worktree allocation, the setup hook, rollback, diff capture and cleanup now run through `spawn/bounded_argv.rs::run_bounded_argv` under a `GitBounds` (stop + deadline + output cap). Two `[CYRUP-DELTA]`s: diff/cleanup are bounded although upstream's are not, and rollback has a 10 s deadline floor. See the table row for the cites, the tests and the remainder (`SUBA-147`, `SUBA-148`). The `Fix`/`Verify` below were written before the port: the "fake `git` on `PATH`" verify was replaced by a `core.fsmonitor` hook that really hangs git.

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read)

**cyrup** — `spawn/worktree.rs::run_git_env` (`:265-281`) is `Command::output().await` with no
deadline, no stop signal, no output cap and no process-group kill. Every worktree create / diff /
cleanup git call goes through it. (The setup HOOK is bounded, `:734-822`, `SUBA-027`.)

**upstream** — `src/runs/shared/worktree-setup-command.ts` (new in `v0.57.0..v0.67.0`): one runner
with `signal`, `deadlineAt`, `maxBuffer` and an owned process-tree controller. At v0.71.0 worktree
setup is a transaction whose every git command honours the run's signal and deadline
(`worktree.ts:239-252`), compensation keeps only the original deadline (`:1305-1325`), and
`preflightWorktreeSource` bounds the source probe (`:359-368`).

**Impact** — A git call that hangs (an index lock, a credential helper waiting on a terminal, a slow
network filesystem) hangs worktree setup past the run's deadline, and `stop` cannot end it. This is
the third instance of the unbounded-spawn class after `SUBA-069` and `SUBA-095`.

**Fix** — Give `run_git_env` a deadline and a cancellation token, spawn in its own process group and
kill it on expiry through `spawn::signal::terminate_on_timeout`, and cap captured output.

**Verify** — A fake `git` on `PATH` that sleeps forever makes worktree creation fail at the run
deadline, and `stop` ends it promptly.

## SUBA-131 — An idle external-CLI step is never flagged

> **CLOSED 2026-10-10** (checked against pi-subagents ad11b7ab). A local external-CLI step now raises `needs_attention` through a `ControlMonitor` driven by stream chunks, a 1s tick and the throttled git-fingerprint probe (`exec/external_cli/activity.rs`, `exec/external_cli/run.rs:192-454`, `exec/external_cli/mod.rs:356-385,600`). Placed (Herdr machine) steps split to `SUBA-212`. Deviations, corrected cites and tests are in the row. The `cyrup`/`upstream` paragraphs below are the pre-fix text with stale line numbers.

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read)

**cyrup** — idle detection lives only in the native attempt driver (`exec/drive_attempt.rs:807`,
`ControlMonitor::update_activity_state`); `exec/external_cli/run.rs::run_external_cli_process`
(`:159-330`) feeds no activity to any monitor, so an external-CLI step never gets `needs_attention`.

**upstream** — `c7938169` (#2167, v0.68.0), `subagent-runner.ts` @v0.71.0: external steps count as one
turn for idle derivation (`:3138`), stdout/stderr chunks refresh activity (`onExternalStreamActivity`,
`:914-918`), and near the attention boundary a coalesced, cancellable `git` fingerprint probe
(`readGitFingerprint`, `:626-649`; `:3143-3166`) credits a changed worktree as activity; a probe
failure fails the step `PROCESS_TREE_UNVERIFIED`.

**Impact** — A stalled claude/codex/cursor child gives no attention notice at all; the user finds out
at the deadline.

**Fix** — Give the external runner a `ControlMonitor`, note activity on every chunk, and port the
throttled fingerprint probe for the attention boundary.

**Verify** — An external script that sleeps with no output past `needsAttentionAfterMs` raises
`needs_attention`; one that writes to stderr periodically does not.

## SUBA-132 — Tool-budget blocks never reach the result

> **CLOSED 2026-09-28.** (on `claude/lows-next`) `SingleResult::tool_budget_blocked` is set from the child's own budget-block message and reaches `WorkflowBudgetSignals::from_single_result` across the runner's `StepResult` waist (`exec/tool_budget.rs:282`, `exec/mod.rs:799-810`, `workflows/settlement.rs:462`). Test: `a_tool_budget_block_reaches_the_result_and_settles_budget_exhausted`. The `workflows/settlement.rs:444-467` quote in the `cyrup` paragraph below is the pre-fix text. Neighbouring gaps are listed in the row. Evidence is in the row.

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**cyrup** — `workflows/settlement.rs:444-467` says it in source: "`tool_budget_blocked` has NO carrier
on `SingleResult` yet … always `false`". The block itself happens (`prompt_runtime.rs:2678`).

**upstream** — `result.toolBudgetBlocked` since v0.43.0 (`execution.ts`); at v0.70.0 (#2302) the
foreground matches the blocked message by the result event's own tool name and records a count above
`hard` (`execution.ts:1144-1156` @v0.71.0), so `workflow-settlement.ts` settles `budget_exhausted`
and the delegation adapter reports `tool_budget_exhausted` (`slash/delegation-adapters.ts:364`).

**Impact** — A workflow child stopped by its tool budget settles as an ordinary failure or success,
not `budget_exhausted`, so a script cannot branch on it.

**Fix** — Carry `tool_budget_blocked` on `SingleResult` from the attempt driver (match upstream's
`isToolBudgetBlockedMessage`) and read it in `WorkflowBudgetSignals::from_single_result`.

**Verify** — A workflow child with `toolBudget.hard: 1` that tries a second tool settles
`budget_exhausted`.

## SUBA-133 — Agent advertisement to the parent prompt is unported

> **CLOSED 2026-09-28** (on `claude/lows-next`). `advertise` is parsed strictly, editable through management and written back; `discovery/advertised.rs` builds upstream's bounded, XML-escaped `<advertised_subagents>` block, and `extension/host/native_impl.rs:810-838` puts it on the parent system prompt at `before_agent_start` while `subagent` is selected, replacing the previous block each turn. Evidence and the one remaining ordering delta (`locale_order` is not ICU root collation) are on the `## Open items` row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**cyrup** — no `advertise` frontmatter field (the key round-trips into `extra_fields`) and no
`<advertised_subagents>` block anywhere.

**upstream** — `src/agents/advertised-agent-prompt.ts:28-60` @v0.71.0 (new in `v0.57.0..v0.67.0`):
file-defined agents with `advertise: true` (`agents.ts:2107-2110`), not disabled, not runtime, and
allowed by the capability ceiling are listed (≤ 16 agents, ≤ 12 KiB, descriptions ≤ 512 bytes,
XML-escaped) in the parent's system prompt at `before_agent_start` (`extension/index.ts:535,825-827`).

**Impact** — Opt-in: without it the parent model never sees agents their authors chose to advertise.

**Fix** — Parse `advertise`, build the block with upstream's bounds, append it in the host's
system-prompt hook, and refresh it when discovery changes.

**Verify** — One advertised agent appears in the parent system prompt; a disabled or ceiling-denied
one does not.

## SUBA-134 — Child sessions get no human-readable name

> **CLOSED 2026-09-30** (on `claude/lows-batch3`). The nested exact-status view is ported and names a nested run by its typed `session_name`, and run-status renders the external-CLI runner block. See the `## Open items` row for the residual nested-relay reach.
>
> **PARTIALLY CLOSED 2026-09-30** (on `claude/lows-batch3`). The intercom claim is now a typed, synchronous request on a typed native bus; the label arm, attached-root and chain-append naming, the launcher-assigned branch and the Fleet/run-status/foreground displays landed. Only the nested-run display name remains (no nested exact-status view exists to name). See the `## Open items` row.
>
> **PARTIALLY CLOSED 2026-09-28** (on `claude/lows-next`). The base naming is ported: derivation (`exec/child_session_name.rs`), the env hand-off (`exec/attempt_runner.rs:541-555`), the child's `set_session_name` at `before_agent_start` (`prompt_runtime.rs:2719-2733`), and `sessionName` on results and single/parallel pending steps. **Still open:** the workflow-node / chain-step / dynamic-task label arm (every caller passes `label: None`), DynamicGroup/ImportAsyncRoot pending statuses, chain-append naming, the launcher-assigned branch that has no production producer, and Fleet/run-status preferring the name. See the `## Open items` row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**cyrup** — `derive_child_session_name` / `child_session_name` have zero hits; a child's session is
never named, and results carry no `sessionName`. (The `session_name` hits in `exec/spawn_plan.rs` are
intercom routing env, `spawn/intercom_target.rs:61-65`.)

**upstream** — `src/shared/child-session-name.ts:27` @v0.71.0 (v0.57.0..v0.67.0, `5ed2a4d4`):
derived from agent name + task or workflow node label, applied with `pi.setSessionName` in the child,
carried as `sessionName` in progress, status and results; v0.71.0 (`55b79828`, #2433) keeps it while
the intercom bridge is active by claiming the route through `intercom:session-identity`.

**Impact** — Child sessions are indistinguishable in `/resume` and session lists.

**Fix** — Port the derivation, pass it to the child (a `--session-name` flag or env the child applies
at start), and thread `session_name` into status and results. The intercom half overlaps area 11.

**Verify** — A `worker` child for "fix auth refresh" lists as `worker: fix auth refresh`.

## SUBA-135 — No launch-cwd preflight

> **CLOSED 2026-09-28.** (on `claude/lows-next`) `preflightLaunchCwd` is ported (`exec/launch_cwd.rs`) and runs at the tool entry ahead of the mission binding, at the head of `run_sync`, and in `spawn_background_steps` before any run exists. Tests: `a_missing_single_launch_cwd_is_refused_before_launch_with_its_typed_spelling`, `an_async_launch_into_a_missing_cwd_is_refused_before_any_run_exists`. **Ledger correction:** the Impact below understated it: the mission binding created the typo'd cwd and the child ran silently in it. Evidence is in the row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (cyrup side read; the failure text was not reproduced)

**cyrup** — `extension/tool/routing.rs::resolve_requested_cwd` (`:367-372`) joins the path and never
checks it; a missing cwd surfaces as the OS error from `Command::current_dir` at spawn (in the
detached runner, after a receipt was returned).

**upstream** — `src/runs/shared/launch-cwd.ts:3-16` @v0.71.0: "Subagent launch aborted: cwd is not a
directory: <p>" / "… cwd does not exist: <p>", suffixed `(resolved from "<requested>")` when the
typed cwd was rewritten; applied in both the foreground and the runner before launch.

**Impact** — A typo'd `cwd` produces an unhelpful OS error, and for async runs only after a receipt.

**Fix** — Port `preflightLaunchCwd` and call it before any run state is written.

**Verify** — `cwd: "nope"` fails before launch with upstream's text naming the resolved path.

## SUBA-136 — Supervisor requests have no renderer and replies leave no entry

> **CLOSED 2026-09-28** (on `claude/lows-next`). `tui/supervisor_ui.rs` ports `supervisor-ui.ts` with its bounds and sanitizer, both renderers are registered (`extension/host/native_impl.rs:213-221`), surfaced requests carry upstream's details, and each reply appends one `subagent_supervisor_reply` entry (`native_supervisor.rs:1117-1140`). See the `## Open items` row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**cyrup** — `native_supervisor.rs:1179` injects requests under `subagent_supervisor_request`, but no
renderer is registered for it (the host registers only the watchdog renderer,
`extension/host/native_impl.rs:308`), and `subagent_supervisor_reply` has zero hits.

**upstream** — `src/intercom/supervisor-ui.ts` @v0.71.0 (new in `v0.57.0..v0.67.0`): a bounded,
sanitized renderer for requests (reason header, body ≤ 8 000 chars, interview ≤ 4 000, reply hint)
registered at `extension/index.ts:652`, and a `subagent_supervisor_reply` session entry recording
each reply (`:6`, `SupervisorReplyEntryData`).

**Impact** — Requests show as raw message content, and the session keeps no record of what the parent
answered. Per README blind-spot 3, the drawing is substrate but the bounds, sanitization and entry
contract are portable.

**Fix** — Register a renderer for the request type with upstream's bounds, and append the reply entry
when `subagent_supervisor` replies.

**Verify** — A request renders with its reason header and truncation marker; a reply appends one
`subagent_supervisor_reply` entry.

## SUBA-137 — The Ghostty inspector misfires inside Ghostty embedders

> **CLOSED 2026-09-28** (on `claude/lows-next`). `available()` also requires `__CFBundleIdentifier` (trimmed) to be `com.mitchellh.ghostty` (`inspectors/ghostty/plugin.rs:134-142`). Verify: `inspectors::ghostty::plugin::tests::available_requires_the_standalone_ghostty_bundle_id`.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**cyrup** — `inspectors/ghostty/plugin.rs:96-100` gates on `TERM_PROGRAM` (case-insensitive
`ghostty`) and macOS only.

**upstream** — v0.69.0: `src/inspectors/ghostty/plugin.ts:5,20-24` @v0.71.0 also requires
`__CFBundleIdentifier === "com.mitchellh.ghostty"`; embedders such as cmux set `TERM_PROGRAM=ghostty`
too and previously got AppleScript `-1728`/`-2741` errors or the wrong window.

**Impact** — macOS users in a Ghostty-embedding terminal get failed or misdirected inspector opens.

**Fix** — Add the bundle-id check to the availability gate.

**Verify** — `TERM_PROGRAM=ghostty` without the bundle id is unavailable; with it, available.

## SUBA-138 — The RPC `cost` method is unported

> **CLOSED 2026-09-28** (on `claude/lows-next`). `cost` is the ninth RPC method and `ping` advertises `capabilities.cost.version = 1`; it and `/subagent-cost` share a port of v0.71.0 `collectSubagentCost` (`registration/cost.rs:995`). **Ledger correction:** the **cyrup** paragraph below implied `/subagent-cost` already computed upstream's data; it was an older transcript walk. The dead R-SA-140 accumulator in `registration/cost.rs` is still to delete (see the row).

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**cyrup** — `extension/rpc/mod.rs:108-118`, `SUBAGENT_RPC_METHODS` has eight methods, no `cost`, and
`ping.rs` advertises no cost capability. `/subagent-cost` exists (`registration/cost.rs`).

**upstream** — `858661af` (#2378, v0.71.0): `src/extension/rpc.ts:35,770` @v0.71.0 adds `cost`,
returning `{ version: 1, parent, children, childTotal, total, unresolvedAsyncChildren }`, and
`ping.capabilities.cost = { version: 1 }`.

**Impact** — Other extensions must scrape `/subagent-cost` text to read spend.

**Fix** — Add the method over the data `/subagent-cost` already computes, and advertise it.

**Verify** — An RPC `cost` request returns the versioned object; `ping` lists `cost`.

## SUBA-139 — No lazy `subagents_enable` loader

> **CLOSED 2026-10-09** (with `SUBA-153`). The loader is ported from `tool-activation.ts` @v0.76.1 as `crates/cyrup-ext-subagents/src/extension/tool_activation.rs`, registered beside `subagent` in the Full arm and driven from `session_start`, `session_tree` and `before_agent_start` (`extension/host/native_impl.rs`). The **Fix** line below named the wrong prerequisite: the loader needs no late registration (`MCP-037a`), because both tools register at `init` and activation only toggles the active set through `HostServices::set_active_tools`, which the session drains at every turn boundary (`AgentSession::next_turn_tools`, `cyrup-session-svc/src/session/tools.rs`) and before the first request (`session/run.rs`). The `before_agent_start` arm now returns the loader edit and the SUBA-133 catalog rewrite in one `Mutate`. CYRUP-DELTA: the loader is `default_active() == false`, so a host without a live dynamic-tool view never selects it, which is the cyrup form of upstream's unsupported-host fallback. A second CYRUP-DELTA: the selection starts unselected rather than pi's `true`, so a session that never emits `session_start` (a host that skips `bind_extensions`) never gets the loader. Verify: `subagents::tool_activation_integration::dynamic_offers_the_loader_first_and_subagent_after_it_is_called` (cyrup-it), `extension::tool_activation::tests::before_agent_start_without_a_session_start_adds_no_loader`, `extension::tool_activation::tests::the_loader_enables_subagent_keeps_other_tools_and_is_idempotent`, `::on_event_keeps_the_loader_edit_and_the_catalog_rewrite_in_one_mutate`.

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read)

**cyrup** — the `subagent` tool is registered unconditionally; `subagents_enable` has zero hits.

**upstream** — `5b679875` (#2380, v0.71.0): `src/extension/tool-activation.ts` registers a small
`subagents_enable` loader and activates the full tool through `setActiveTools` on demand; hosts
without dynamic tools (`unsupportedDynamicToolsReason`, `:22-27`) keep the always-loaded tool, which
is what cyrup does today.

**Impact** — Every prompt carries the full `subagent` tool schema; upstream keeps unrelated prompts
smaller. Cost only.

**Fix** — Needs a working late-activation path in the host (see `MCP-037a`), then the loader.

**Verify** — A fresh session's first request carries `subagents_enable` and not `subagent`; calling
the loader activates `subagent` for the next turn.

## SUBA-140 — No child-only prompt-cache retention

> **CLOSED 2026-09-28** (on `claude/lows-next`). A non-empty `CYRUP_SUBAGENT_CACHE_RETENTION` becomes the child's `CYRUP_CACHE_RETENTION` (`exec/spawn_plan.rs:594-603`). Verify: `exec::spawn_plan::tests::the_child_only_cache_retention_reaches_the_child_env`.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**cyrup** — `PI_SUBAGENT_CACHE_RETENTION` and a cyrup-named equivalent have zero hits in the crate;
children inherit the parent's retention.

**upstream** — `ce3cff20` (#2190, v0.68.0): `src/shared/child-cache-retention.ts:22` @v0.71.0 maps
the variable onto the child's `PI_CACHE_RETENTION` (spawned children) or a per-request pin
(in-process children).

**Impact** — A parent on the 1-hour tier writes its short-lived children at the same, more expensive
tier. Cost only.

**Fix** — Read `CYRUP_SUBAGENT_CACHE_RETENTION` and set the child's retention env in `spawn_plan`.

**Verify** — With the variable set, a child's spawn env carries the requested tier.

## SUBA-141 — Observability drift: external-CLI logs and runner exit status

> **CLOSED 2026-09-30** (on `claude/lows-batch3`). The Fleet half landed: a typed live process hook from the external CLI runner publishes `StepStatus.external_process` into `status.json`, the logs live in the run dir, and Fleet shows `external-cli · <elapsed>` plus the stderr/stdout tails; the stderr bound counts UTF-16 units. See the `## Open items` row.
>
> **PARTIALLY CLOSED 2026-09-28** (on `claude/lows-next`). The runner-exit half is ported: the launcher observes the detached runner's close, and the reconciler reports `exited with code N (signal S)` with upstream's stderr-tail bounds (`background/reconcile.rs:664-741`). **Still open:** the Fleet external-CLI stdout/stderr tails and step elapsed time, blocked because cyrup's `StepStatus` has no `externalProcess` and `exec/external_cli` publishes no live process hook. See the `## Open items` row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**cyrup** — `background/fleet_view.rs:1088-1106` offers only "Transcript tail"; an external-CLI
step's stdout/stderr logs are not shown. `background/reconcile.rs::synthesize_failure` (`:727-732`)
reports "reconciled as stale/dead: <reason>" plus the runner stderr tail, without the runner's exit
code or signal.

**upstream** — `be6e0ca6` (#2375, v0.71.0): `fleet-view.ts:584-605` @v0.71.0 shows
`External stderr tail` / `External stdout tail` (first while the step runs) and elapsed time on the
step row. `2e280687` (#2427, v0.71.0): `stale-run-reconciler.ts:235-238` reports
`exited with code N (signal S)` from the recorded runner exit.

**Impact** — A failing external agent's output has to be found by hand; a dead runner's diagnosis
lacks its exit status.

**Fix** — Add the two log sources to the Fleet tail; record the runner exit where the launching
process observes it and include it in the synthesized failure.

**Verify** — Fleet on a running external step shows its stderr tail; a runner killed with SIGKILL
reconciles with `signal SIGKILL` in the message.

## SUBA-143 — Runtime agents can be registered only through the native Rust API

**Kind** upstream-drift · **Severity** medium · **Effort** L · **Confidence** confirmed (both sides read)

Promoted from `09a`'s residual-lead list (the `SUBA-084` closure).

**cyrup** — runtime registration exists only as native methods (`extension/host/mod.rs:432`,
`extension/executor/mod.rs:396` `register_agent`); `runtime-agent-register` has zero hits, and the bus
cannot carry upstream's contract as written: `crates/cyrup-ext/src/bus.rs:80-93` QUEUES emits
(`[CYRUP-DELTA]`, to avoid re-entering a WASM guest's store) and passes payloads by value.

**upstream** — `src/agents/runtime-agent-events.ts:4-70` @v0.71.0 (v0.64.0): another extension emits
`pi-subagents:runtime-agent-register:v1` with `{ version: 1, name, definition }`; the owner's listener
registers the agent synchronously and writes `request.result` (`{ ok, registration }` or
`{ ok: false, error }`) into the same object; `registerAgentViaEvents` reads it back. Re-exported at
`src/api/agents.ts:3-10`; the listener is installed at `extension/index.ts:25`.

**Impact** — A guest (WASM) extension cannot contribute agents at runtime, which upstream extensions
do through this event; cyrup can host only built-in native registrants.

**Fix** — Design a request/response bus topic (a reply topic keyed by request id, as the RPC bridge in
`extension/rpc/` already does) that carries the same request and result shapes, and route it to
`register_agent`. Registration disposal needs a matching topic.

**Verify** — A guest extension registers an agent through the topic, sees `{ ok: true }`, and the
agent appears in discovery; a malformed definition returns `{ ok: false }` with upstream's message.

## SUBA-142 — tracker: in-process child sessions

**Kind** tracker · **Severity** tracker · **Effort** — · **Confidence** confirmed (both sides read)

Promoted from `09a`'s census lead. At v0.71.0 upstream builds every native child in-process
(`src/runs/shared/child-launch.ts::buildInProcessChildLaunch`, `src/runs/shared/child-session.ts`),
over a shared model runtime; cyrup spawns a `cyrup` process per child (`spawn/mod.rs`,
`exec/spawn_plan.rs`), and no `[CYRUP-DELTA]` records that as a decision. This is a scope question,
not schedulable work, so it is a tracker.

Features upstream built on the in-process mechanism and that this pass therefore did not file as
items: usage reconciliation from the session's own messages (`usage-reconciliation.ts`, #2295), the
parent-provider-registry inheritance and its model-not-found diagnostic (#2276,
`model-resolution-diagnostic.ts`), the per-request cache-retention pin, and the Pi-0.86/0.87 SDK
ownership fixes (`child-session.ts`). `SUBA-110`'s scrub applies only where upstream still spawns
(the runner and external CLIs), and that remains correct.

**Escalates to an item** when a decision of record says cyrup follows upstream, or when one of the
parked features is shown to change a result for a spawned cyrup child.

> **FOLD-IN 2026-10-03 (v0.75.0):** `07946874`/#2636 and `1fe508f1`/#2638 make children launch and verify on extension-registered virtual models (`pi-virtual` api); the child's `virtualModelId` (`src/runs/shared/child-session.ts:118,589` @v0.75.0) replaces the router's physical model in `formatSubagentModelVerificationError` (`run-child-session.ts:517`, `foreground/execution.ts:1114`). In-process only, so parked here, no row. When `SESS-067` (area 03, the session half of virtual models) lands, `exec/model_verification.rs` must compare the child's selected model rather than the router's physical model, or a correct child fails `model_verification_failed`.

---

## Findings filed 2026-10-02 — the `v0.71.0..v0.74.0` window

`pi-subagents` **v0.74.0** (88 commits since v0.71.0), read only through `git -C tmp/pi-subagents
show v0.74.0:<path>` and `git diff v0.71.0..v0.74.0`; cyrup read at `fe875569`. This file's window
is therefore now `v0.57.0..v0.74.0`, and its title's `v0.64` is the tag the first drift pass opened
against, not the current one.

## SUBA-150 — `workflow: true` reply-fenced scripts; `workflowScript` / `workflowScriptPath` are gone from the tool

**Kind** upstream-drift · **Severity** medium · **Effort** L · **Confidence** confirmed (both sides read)

**upstream** — `0538e14d` (#2588, `feat(workflows)!`, v0.74.0) is a declared breaking change to the
tool surface. The `subagent` tool now has exactly one workflow field
(`src/extension/schemas.ts:221` @v0.74.0):

- `workflow: true` runs the single ` ```js workflow ` fenced block in the **same assistant reply**
  that issued the tool call. `readReplyWorkflowScript`
  (`src/extension/reply-workflow-script.ts:14`) walks `sessionManager.getBranch()` backwards for the
  assistant message carrying this `toolCallId` and reads the script out of its text blocks. Pi
  persists the whole assistant message before running its tool calls, which is what makes this
  work.
- a string containing `/` or `\` is a script **file** read from the request cwd.
- any other string is a named workflow **resource**.

`workflowScript` and `workflowScriptPath` were **removed from the tool**; RPC spawn takes inline
text as `script` (`src/extension/rpc.ts`). The fence scanner (`workflowBlocks`,
`reply-workflow-script.ts:41`) skips the bodies of other fences, accepts ``` and `~~~` markers of
three or more characters, and has four distinct refusals: no `workflow: true` outside a model tool
call, more than one `workflow: true` call in one reply, an unclosed block, and a block count that is
not exactly one. `src/extension/public-execution.ts` and `src/runs/background/scheduled-runs.ts`
were rewritten around the single field; `workflowScript` survives only as the package's internal
carrier (`disabled-features.ts:106-110` names it as such).

**cyrup** — `extension/tool/schema.rs:345` advertises `workflowScript` with its own long description,
and `extension/tool/mod.rs:190` records that `SubagentToolParams` carries "neither
`workflowScriptPath`, nor `workflow`, nor the marker key". The whole scripted-workflow engine is
built around the name: `workflows/scripted/engine.rs:259` ("The workflowScript body"), `:1505`
(`params.contains_key("workflowScript")`), `:2160` (`NESTED_WORKFLOW_REFUSAL`, whose text says
`workflowScript cannot be started from inside a workflow child.`), `:2184`. Zero hits in `crates/`
for `js workflow`, `reply_workflow` or `readReplyWorkflowScript`, and zero in
`docs/gap-analysis/` — this is not filed anywhere.

**Impact** — Two things, and the second is the reason this is `medium` rather than `low`. (1) A model
trained on or prompted from pi's current tool reference will call `workflow: true` and cyrup will
reject it as an unknown parameter with no hint. (2) The reason upstream made the change is that a
script passed as a JSON string has every quote and newline escaped, which is a real authoring tax on
the model and a real source of malformed scripts; cyrup has that tax today.

**Fix** — This is `L` and should be split. (a) Add `workflow: true | string` and keep
`workflowScript` as the internal carrier, exactly as upstream does, so the engine's internals and
`NESTED_WORKFLOW_REFUSAL` need no rename. (b) Port `workflowBlocks` and `scriptFromReply` as a pure
function over the branch's assistant content, with upstream's four refusal strings verbatim — the
fence grammar is the load-bearing part and is pure, so it tests without a session. (c) Decide
whether to remove `workflowScript`/`workflowScriptPath` from the advertised schema. Removing them is
a breaking change to cyrup's own tool; keeping them is an advertise-vs-pi divergence. Either way the
decision belongs in a `[CYRUP-DELTA]`, because this crate's rule is that the schema and the dispatch
agree, and both would still dispatch.

**Verify** — A reply containing one ` ```js workflow ` block and a `subagent({ workflow: true })`
call runs that script; a reply with two such calls, with two blocks, with an unclosed block, and a
non-model caller each get upstream's distinct message; a ` ```js ` block without the ` workflow `
tag is ignored; a `~~~~`-fenced block closes only on four or more `~`.

> **FOLD-IN 2026-10-03 (v0.75.0):** (a) `df3b6df1`/#2610 makes the string `"true"` equal `workflow: true` for MCP clients that stringify booleans (`src/extension/reply-workflow-script.ts:29`, `rpc.ts:527`, `index.ts:744` @v0.75.0); it is part of the `workflow` field this row ports. (b) `cfb6f9a8`/#2611: `action: "validate"` now checks workflow `args` against the same limits as a launch, exports `MAX_ARGS_FIELDS/ITEMS/DEPTH/BYTES` and states them in the `args` schema description (`src/extension/schemas.ts:7,227` @v0.75.0). Cyrup already enforces 16 fields, 64 items, depth 8 and 16 KiB in `workflows/resources.rs` (`validate_plain_json` and `normalize_args`, `:385-445`; they are literals, only `MAX_ARGS_BYTES` is a private const at `:29`), but its `validate` arm (`extension/tool/routing.rs:1870`) takes only `workflowScript` and never runs `normalize_args`, and `args` is advertised and plumbed only for scheduled runs (`extension/tool/schema.rs:742`, `background/scheduled_runs/tool.rs:355`). Port: export the four limits as constants, state them in the `args` description, and run `normalize_args` in `validate`, reporting its error beside the script errors.

## SUBA-151 — `chain`, `tasks` and the dynamic-fanout schema left the default tool; they are now lowered into package-owned workflow scripts

**Kind** upstream-drift · **Severity** medium · **Effort** L · **Confidence** confirmed (both sides read)

**upstream** — `71d042f0` (#2596, v0.74.0) finished what #2588 started. At v0.71.0
`SubagentParamProperties` carried `chain` (built from `ChainItem`), `tasks`, `ParallelTaskSchema`,
`DynamicExpandSchema`, `DynamicParallelTemplateSchema`, `DynamicCollectSchema`, `OutputOverride`,
`ReadsOverride` and `ChainGateOverride`. **At v0.74.0 every one of those is deleted**
(`git -C tmp/pi-subagents diff v0.71.0..v0.74.0 -- src/extension/schemas.ts`), and
`git -C tmp/pi-subagents show v0.74.0:src/extension/schemas.ts | grep -n 'chain'` finds `chain` only
inside `StructuredWorkflowProperties` (`:286`) and `createSubagentParamsSchema` (`:299,305,308`).

The data-shaped surface now exists in exactly one place: when `disabledFeatures` lists
`workflow-scripts`, `createSubagentParamsSchema` drops the `workflow` family and substitutes a
deliberately small `tasks` / `chain` pair — `{agent, task}` items, and steps that are
`{agent, task?, as?}` or `{parallel: [{agent, task}]}`, with `additionalProperties: false` and no
`phase`, `label`, `cwd`, `machine`, `count`, `output`, `reads`, `progress`, `skill`, `model`,
`fast`, `toolBudget`, `acceptance`, `agentContract`, `gateOn`, `expand`, `collect`, `concurrency`,
`failFast` or `worktree`. Those inputs are then **compiled into a package-owned workflow script**
(`buildStructuredWorkflowScript`, `src/workflows/structured-workflow-scripts.ts:175`): every caller
string is embedded through `JSON.stringify` and never becomes code, `{task}`/`{previous}`/
`{outputs.<name>}` compile to string concatenation over script variables in a single
non-rescanning pass, and the generated script runs on the ordinary workflow runtime with a `settle`
/`failure` prelude. Three narrowings landed in the same diff: `usageBudget` gained
`minProperties: 1`, `toolBudget`'s description now states `soft <= hard`, and the `timeoutMs`
description says the aliases "must agree".

**cyrup** — `extension/tool/schema.rs` is a faithful port of the **v0.71.0** shape: `sj_chain_item`
(`:259`, whose doc cites `schemas.ts:190-229`), `sj_parallel_task` (`:166`, citing `:133-152`),
`sj_dynamic_parallel_template` (`:194`), `tasks` (`:520`), `concurrency` (`:525`), `worktree`
(`:526`), `chain` (`:527`), `chainDir` (`:537`). `buildStructuredWorkflowScript`,
`structured_workflow` and `StructuredWorkflow` have zero hits in `crates/`, and
`structured-workflow-scripts` has zero hits in `docs/gap-analysis/`. cyrup's chain/parallel
orchestration is native (`spawn/chain_graph.rs`, `extension/executor/chain.rs`), not a lowering.

**Impact** — This is the largest single piece of pi-subagents drift in the window, and it is a
**design** divergence, not a missing feature: upstream has decided that one execution engine
(workflow scripts) runs everything, and that the data-shaped `chain`/`tasks` surface is a
reduced-capability fallback for operators who turn scripts off. cyrup has two engines. Severity is
`medium` and not higher because cyrup's surface works and is a superset of upstream's reduced one —
nothing breaks for a cyrup user. What breaks is parity reasoning: every future `chain`/`tasks`
upstream change now lands in a generated script, so a line-for-line comparison of the two
`chain` paths stops being meaningful.

**Fix** — Needs a decision of record before any code. The options, with what each costs: (a) follow
upstream — lower `chain`/`tasks` into generated scripts and keep the native graph only as the
script runtime's executor. This is the only option that restores line-level parity, and it is a
large, risky change to the most heavily tested code in the crate. (b) Keep the native engine and the
full schema, and record a `[CYRUP-DELTA]` saying so — then the schema narrowings in this diff
(`usageBudget.minProperties`, the two descriptions) are still worth porting on their own, and future
upstream `chain` work has to be back-translated by hand. (c) Keep the native engine but port
`createSubagentParamsSchema`'s reduction so `disabledFeatures: ["workflow-scripts"]` behaves as
upstream does — this depends on `SUBA-152` and is the cheapest way to stop the surfaces diverging
further. Recommend (b) plus (c), and say so in the file rather than leaving it implied.

**Verify** — For (c): with `workflow-scripts` disabled the advertised schema has `tasks`/`chain` in
upstream's reduced shape and no `workflow`/`args`/`preflight`/`globalConcurrencyLimit`/
`maxSubagentSpawnsPerRun`; with it enabled the schema is unchanged. For the narrowings: an empty
`usageBudget: {}` is refused.

## SUBA-152 — `config.disabledFeatures` is unported, so an operator cannot trim the `subagent` tool

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read)

**upstream** — `60905d10` (#2543, v0.73.0) added `src/shared/disabled-features.ts`.
`SUBAGENT_FEATURES` (`:8` @v0.74.0) is 15 groups, each owning the actions and the params that only
it needs: `agent-management`, `watchdog`, `panes`, `missions`, `lane-management`,
`spawn-budget-grants`, `preflight`, `lane-metadata`, `gates`, `usage-budgets`, `tool-budgets`,
`control-overrides`, `extension-bindings`, `external-machines`, `workflow-scripts`.
`resolveDisabledFeatureSurface` (`:82`) builds the disabled param/action maps and folds
`scheduledRuns.enabled: false` in as a sixteenth pseudo-feature `schedules`;
`disabledFeatureUseError` (`:98`) rejects a request that uses a disabled action or param with
`subagent action '<a>' is disabled by config disabledFeatures "<f>".` /
`subagent option '<p>' is disabled by config …`; `disabledFeatureNotice` (`:115`) prepends a
"Disabled by config in this session" block to the static reference docs. `validateDisabledFeatures`
(`:51`) refuses a non-array, an unknown name (listing all 15), a duplicate, and the literal
`"schedules"` with a message naming `scheduledRuns.enabled` instead. `disabledFeatures` is in
`FAIL_CLOSED_CONFIG_KEYS` (`src/extension/config.ts:17`) and validated at `:182`. The surface
reaches the tool at `src/extension/index.ts:705,711,715,720,721` and the executor at `:803`
(`workflowScriptsDisabled`). Declared at `src/shared/types.ts:2674`.

**cyrup** — `grep -rn 'disabled_features\|disabledFeatures\|DisabledFeature' crates/ --include='*.rs'`
is empty, and so is the same grep over `docs/gap-analysis/`. The key is not in
`UNPORTED_CONFIG_KEYS` (`registration/mod.rs:572`, 10 entries) either, so
`SubagentExtensionConfig`'s `#[serde(default)]` with no `deny_unknown_fields` accepts it and drops it
with no warning — the same advertise-and-ignore shape `SUBA-061` was filed for.

**Impact** — Low on its own: an operator who sets the key gets the full tool and no error. It is
listed because it is the hinge for `SUBA-151`'s option (c) — upstream's reduced `chain`/`tasks`
schema exists only when this key disables `workflow-scripts` — and because the `subagent` tool
description is the single largest prompt cost this crate imposes, so letting an operator cut 15
groups out of it is a real context win.

**Fix** — `S` for the data and the validator, `M` for the wiring. Port `SUBAGENT_FEATURES` as a
`const` table with upstream's names and member lists verbatim; add `disabled_features: Option<Vec<String>>`
to `SubagentExtensionConfig` with `validate_disabled_features` in `validate_raw_config` beside
`validate_missions`; build the surface once per session and consult it in three places — the
advertised schema (`extension/tool/schema.rs`), the tool description
(`registration/tool_description.rs`, prepending the notice) and the dispatch boundary
(`extension/tool/routing.rs`, before any action runs). The `workflow-scripts`-owns-`preflight` rule
at `disabled-features.ts:91` is load-bearing: a param shared with `workflow-scripts` is always
attributed to it whatever order the config lists features in.

**Verify** — Each of the 15 names disables exactly its own actions and params and nothing else; a
disabled action and a disabled param each get upstream's message; `"schedules"` in the array is
refused by name; a duplicate and an unknown name are refused; `scheduledRuns.enabled: false` alone
disables the nine `schedule.*` actions and their ten params.

> **CORRECTED 2026-10-03:** the claim that cyrup accepts `disabledFeatures` and "drops it with no warning" is false. `SubagentExtensionConfig::config_warnings` (`registration/mod.rs:927-945`) runs `discovery::key_census` and emits `unknown key 'disabledFeatures' (ignored)` (`:944`); `crates/cyrup/src/subagent_config.rs:75-77` prints it to stderr on load (only when `validate_raw_config` passes). What is true is narrower: the key is not in `UNPORTED_CONFIG_KEYS` (`registration/mod.rs:572`, 10 entries), so the message is the generic typo wording rather than the "is not supported by this port (...); it has no effect" wording an unported key gets, and nothing says the key is a real upstream feature. The upstream cites (`disabled-features.ts:8/82/98`, `config.ts:17/182`) hold; v0.75.0 only widens upstream's fail-closed list (see `SUBA-166`).

## SUBA-153 — `SUBA-139`'s port target moved: three activation modes and a per-API cache-miss gate

> **CLOSED 2026-10-09** (with `SUBA-139`). `tool-activation.ts` is byte-identical from v0.74.0 to v0.76.1, so the target below still holds; its `:36-53`/`:85` cites are also correct at v0.76.1, and the types cites are now `shared/types.ts:2608,2686-2687`, the validator `extension/config.ts:178-180`. Ported: `config.toolActivation` (`ToolActivationMode`, raw validation in upstream's order, fail-closed), `HostServices::current_model_info()` (pi `ctx.model`), `ModelCompat::supports_additional_tools` (the 2026-10-03 correction's missing field; the catalog key is now kept), and `addsToolsWithoutCheckpoint` verbatim as `compat_adds_tools_without_checkpoint`, tested alone against upstream's 14-row table. **CYRUP-DELTA, decided 2026-10-09:** `auto` additionally requires `cyrup_provider::api::emits_native_tool_additions(api)`, `false` for every api until an adapter gains its native mid-conversation tool emitter (PROV-133 for Anthropic; the "PROV-083b" completions/Responses emitters). Today every cyrup adapter rebuilds the request tool list, so a verbatim `auto` would pick the loader exactly where enabling it misses the prompt cache. `auto` therefore equals `eager` for a fresh session until then, and switches on per api by itself when the emitters land. The **Verify** clause "with a model whose compat says it can add tools mid-conversation selects the loader" is pinned the other way on purpose (`auto_starts_every_fresh_session_eager_while_no_adapter_emits_tool_additions`, `auto_on_a_capable_anthropic_model_stays_eager_until_the_adapter_emits_tool_additions`); every other clause holds as written. Verify: `extension::tool_activation::tests::upstreams_predicate_matches_its_fourteen_row_table`, `::the_cyrup_gate_refuses_every_row_until_an_adapter_emits_native_tool_additions`, `::auto_replays_recorded_sessions_without_adding_or_removing_tools`, `::dynamic_always_offers_the_loader_even_to_a_recorded_session_without_it`, `::the_loader_is_registered_except_under_eager_and_in_a_child`, `::tool_activation_config_validates_like_upstream`, `api::tests::no_adapter_emits_native_tool_additions_yet` (cyrup-provider).

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read)

**This row does not duplicate `SUBA-139`.** `SUBA-139` (open, low, `09b:242`) says the
`subagents_enable` lazy loader is unported and cyrup behaves as upstream's unsupported-host
fallback. That is still true. This row records that the thing `SUBA-139` asks to be ported **changed
shape** in this window, so anyone picking `SUBA-139` up now would port a surface that no longer
exists. Close them together.

**upstream** — `92a8a1f5` (#2595, v0.74.0) adds `config.toolActivation`
(`src/shared/types.ts:2595,2672`; validated at `src/extension/config.ts:175-176` with
`config.toolActivation must be "auto", "dynamic", or "eager"`, and listed in
`FAIL_CLOSED_CONFIG_KEYS` at `:17`):

- `eager` returns immediately from `registerSubagentToolActivation`
  (`src/extension/tool-activation.ts:85-87`) — no loader at all, `subagent` always active. Upstream
  says this equals `--exclude-tools subagents_enable`.
- `dynamic` always selects the loader.
- `auto` (the default) goes eager **only** for an empty session **and only when the model cannot add
  tools mid-conversation without a cache checkpoint**. That last predicate is
  `addsToolsWithoutCheckpoint` (`:36-53`), a per-API read of the model's compat flags:
  `anthropic-messages` needs `supportsMidConvoToolChanges`, `openai-completions` needs
  `supportsMidConvoToolAdditions`, the three Responses APIs need `supportsAdditionalTools ||
  supportsToolSearch`, and all of them first need `supportsMidConvoSystemMessages`; any other API is
  `false`. The point is stated in the comment: without those flags a mid-conversation tool change
  makes Pi resend the conversation under a new leading system message and miss the prompt cache.

Three further changes in the same file: `setSelection` now takes `includeLoader` as well as
`includeSubagent` and can deselect the loader; the recorded-selection replay tracks the whole
recorded tool set rather than `subagent`'s membership alone, so `auto` never adds the loader to a
transcript that did not declare it; and the decision is taken at `session_start`/`session_tree`
only, so switching models mid-session keeps the session's tools. Earlier commits in the window round
the loader out: `3bd1892f`/#2525 (tell the model to check its tool list instead of waiting),
`32ceaab0`/#2492 (accept stray `subagents_enable` arguments), `2f39ae2c`/#2531 (enable on in-process
hosts), `2f8c55a5`/#2514 (tell the model when enabled tools arrive on bridged providers).

**cyrup** — `toolActivation`, `tool_activation`, `ToolActivationMode` and `subagents_enable` all have
zero hits in `crates/`. The compat flags the gate reads **do** exist port-side
(`crates/cyrup-provider/src/api/compat.rs:456` `supports_mid_convo_system_messages`, `:466`
`supports_mid_convo_tool_additions`, `:520` `supports_mid_convo_tool_changes`, plus
`supports_tool_search`/`supports_additional_tools`), so the predicate is portable today.

**Impact** — Low, and it is `SUBA-139`'s impact: the full `subagent` tool is always in the prompt.
The addition here is that the fix is now three-valued, and that the `auto` arm's whole purpose is a
prompt-cache property — which is also what the v1.0.0 release post calls "deferred tool loading"
and "cache warming". A port that implements only the v0.71.0 two-state loader would cost cyrup the
cache on exactly the providers upstream protects.

**Fix** — Port with `SUBA-139`, not after it. Add `toolActivation` to `SubagentExtensionConfig` with
upstream's validator message; port `addsToolsWithoutCheckpoint` against
`crates/cyrup-provider/src/api/compat.rs`'s fields, keeping the per-API switch and the
`supportsMidConvoSystemMessages` precondition; register the loader only for `auto`/`dynamic`.

**Verify** — `eager` registers no loader; `dynamic` always selects it; `auto` on an empty session
with a model whose compat says it can add tools mid-conversation selects the loader, and with a
model that cannot, goes eager; `auto` on a session with history keeps whatever the transcript
declared; a model switch mid-session changes nothing.

> **CORRECTED 2026-10-03:** the statement that the compat flags the gate reads "do exist port-side ... plus `supports_tool_search`/`supports_additional_tools`" is wrong for the second flag. `ModelCompat` (`crates/cyrup-provider/src/api/compat.rs`) has `supports_mid_convo_system_messages` (`:456`), `supports_mid_convo_tool_additions` (`:466`), `supports_mid_convo_tool_changes` (`:520`) and `supports_tool_search` (`:569`), but no `supports_additional_tools` field: a grep of `crates/**/*.rs` for `supports_additional_tools` finds nothing and for `supportsAdditionalTools` finds only a test comment (`providers/openai_codex.rs:779`). `supportsAdditionalTools` appears in the catalog JSON (`opencode-go.json`, `openai-codex.json`, `github-copilot.json`), and `ModelCompat` sets no `deny_unknown_fields`, so the catalog key is ignored. The Responses-API arm of `addsToolsWithoutCheckpoint` (`tool-activation.ts:36-53` @v0.74.0) therefore needs a new compat field and catalog plumbing first; "the predicate is portable today" holds only for the `anthropic-messages` and `openai-completions` arms.

## SUBA-154 — `--default-prefix` needs git ≥ 2.43, and a failed worktree capture destroys the child's work

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed (both sides read)

**upstream** — `1cf63c18` (#2527, v0.73.1) changed `MACHINE_DIFF_OPTIONS`
(`src/runs/shared/worktree.ts:23` @v0.74.0) from
`["--no-color","--no-ext-diff","--no-textconv","--default-prefix","--line-prefix=","--no-relative"]`
to `["--no-color","--no-ext-diff","--no-textconv","--src-prefix=a/","--dst-prefix=b/","--line-prefix=","--no-relative"]`.
The changelog entry says why: explicit `a/`/`b/` prefixes "work on older Git releases while still
overriding `diff.noprefix`". `MACHINE_PATCH_OPTIONS` is `[...MACHINE_DIFF_OPTIONS, "--binary"]`, so
both move together.

**cyrup** — `spawn/worktree.rs` has the v0.68.0 list twice, spelled out rather than composed:
`MACHINE_DIFF_OPTIONS` at `:75-82` and `MACHINE_PATCH_OPTIONS` at `:89-97`, both containing
`"--default-prefix"` (`:79`, `:95`). `git diff --default-prefix` was added in git **2.43**
(Nov 2023); older git exits non-zero with `error: unknown option 'default-prefix'` and writes no
diff. `--src-prefix=`/`--dst-prefix=` have been accepted for well over a decade.

**Impact** — This is `medium`, not `low`, and the reason is in cyrup's own doc at
`spawn/worktree.rs:71-74`: "a harvested worktree is REMOVED right after capture
([`crate::spawn::chain_graph`]'s `publish_worktree_handoff`), so the patch is all that survives".
On git < 2.43 every managed-worktree child's diff and patch capture fails, and then the worktree is
removed — the child's work is gone with no patch to recover it. `SUBA-130`'s closure added a 256 MiB
`PATCH_CAPTURE_MAX_BYTES` rule precisely so that an overflow **fails the capture and preserves the
worktree**; an `unknown option` failure needs the same treatment and, better, should not happen.
`validate_worktree_patch_represents_current_worktree`'s byte-for-byte re-capture comparison also
fails on both sides, so the failure mode is total rather than partial.

**Fix** — Two one-line edits in `spawn/worktree.rs` (`:79` and `:95`) replacing `--default-prefix`
with `--src-prefix=a/` and `--dst-prefix=b/`, and the doc comment at `:67` updated (it currently
explains `--default-prefix` as the answer to `diff.mnemonicPrefix`/`diff.noPrefix`; the explicit
pair answers both as well, and the doc should say so). While there, check that a capture failure
preserves the worktree rather than letting `publish_worktree_handoff` proceed to cleanup.

**Verify** — A repo with `diff.noprefix=true` and `diff.mnemonicPrefix=true` still produces a
`-p1`-applicable patch; the argv contains no `--default-prefix`; a capture failure (any cause)
leaves the worktree on disk. The existing `spawn::worktree::tests` harvest cases cover the first.

> **CORRECTED 2026-10-03:** (1) "a failed capture destroys the child's work" is wrong. `cleanup_worktrees` is called with `WorktreeCleanupIntent::Preserve(PreserveEvidence)` (`spawn/chain_graph.rs:2001`), and `refuse_uncaptured_preserve` (`spawn/worktree.rs:1864`) refuses to remove a worktree whose capture row has an error, whose patch file is missing, or whose patch is not recorded in the handoff manifest, leaving it on disk with `preserved: true` (doc at `worktree.rs:1575-1582`). So an `unknown option` failure costs a missing patch and a kept worktree, not the work; the Impact paragraph below and the "better, should not happen" hedge overstate it, and the Fix's last sentence ("check that a capture failure preserves the worktree") is already satisfied. (2) The "git >= 2.43" figure is unsourced: the v0.73.1 changelog says only that worktree diffs "now work with older Git versions that lack the `--default-prefix` option" (`CHANGELOG.md` @v0.74.0), and the version number was not verified. The two edits at `spawn/worktree.rs:79,95` and the upstream cite (`worktree.ts:23` @v0.74.0) stand. Recommended re-rating (not applied): medium to low, because the preserve gate keeps the worktree and the effect is a missing patch.

## SUBA-155 — `modelScope` is the three-key v0.33 shape: no per-agent rules, no `inherit`, no `scoped`

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed (both sides read)

**Not in-baseline, and not previously filed.** `git -C tmp/pi-subagents show v0.43.0:src/runs/shared/model-scope.ts`
has neither `agents` nor an `inherit` expansion, so none of this belongs to `09`. The ledger's
`modelScope` rows are `SUBA-003` (closed, "ported and enforced"), `SUBA-035` (closed, the doctor and
models surfaces) and `SUBA-050` (closed, `strict`); none of them mentions `agents`, `inherit` or
`scoped`. The `agents`/`inherit` half is therefore pre-v0.74.0 drift this file's earlier passes
missed; `scoped` is new in this window.

**upstream** — At v0.74.0 `ModelScopeConfig extends ModelScopeRule` with
`agents?: Record<string, ModelScopeRule>` — "Additional restrictions keyed by canonical agent name"
— and `resolveModelScopesForAgent(config, agentName, parentModel, scopedModelIds)` returns a global
scope plus, when present, an `modelScope.agents.<name>` scope whose `enforce`/`strict` fall back to
the global ones. Allow patterns go through `expandReservedPatterns`: `inherit` becomes the parent
session's `provider/id`, and `scoped` (`SCOPED_PATTERN`, `src/runs/shared/model-scope.ts:51`,
added by `c305d4bb`/#2538, v0.73.0) becomes the parent session's scoped-model snapshot — pi's
`/scoped-models` — degrading to `inherit` semantics when the parent is unscoped. A token that
cannot be expanded is returned unexpanded so the enforced-inherit path in `model-resolution.ts`
fails **closed**. `checkModelScope` also gained a render cap: more than `MAX_RENDERED_PATTERNS = 8`
(`:53`) patterns render as `a, b, …, h, … (N patterns total)` (`:93`). Every launch path passes the
snapshot; background runs keep the set captured at start.

**cyrup** — `exec/model_scope.rs:40` `ModelScopeConfig` has exactly three fields: `enforce`,
`strict`, `allow`. There is no `agents`, no `resolve_model_scopes_for_agent`, no reserved-token
expansion (`grep -n 'inherit\|agents' exec/model_scope.rs` finds only prose and test names) and no
render cap. `allow` is matched literally with only `*` special (field doc at `:58-59`).
`settings.json`'s `subagents.modelScope.agents` is accepted by serde and dropped.

**Impact** — Two concrete failures, which is why this is `medium`. (1) An operator who copies pi's
documented `"modelScope": {"enforce": true, "allow": ["inherit"]}` gets a cyrup that matches the
literal string `inherit` against `provider/id`, matches nothing, and **rejects every explicit
model** — the run is refused with `SubagentError::ModelOutOfScope` naming an allowlist of one
meaningless pattern. (2) `modelScope.agents.<name>` is silently dropped, so a per-agent restriction
an operator believes is in force is not; that is the failure direction that matters for a policy
knob.

**Fix** — `M`. Add `agents: Option<BTreeMap<String, ModelScopeRule>>` and split the three shared
fields into a `ModelScopeRule` as upstream does; port `resolve_model_scopes_for_agent` with
upstream's `enforce`/`strict` fallback and its two origins (`modelScope`,
`modelScope.agents.<name>`); port `expand_reserved_patterns` for `inherit` and `scoped`, keeping the
unexpanded-token-fails-closed rule; add the 8-pattern render cap to the violation message. The
`scoped` arm needs the parent session's scoped-model set threaded to each launch and captured at
start for background runs — cyrup already has the set (`cyrup-tui/src/app/selectors.rs:225`
`CheckboxSelector::scoped_models`, `event_extract.rs:355`), but `RunnerConfig` does not carry it,
which is the one non-trivial piece.

**Verify** — `allow: ["inherit"]` with `enforce` admits the parent session's model and refuses
another; `allow: ["scoped"]` admits every scoped model and, with no scoping configured, behaves as
`inherit`; an enforced `scoped`/`inherit` with no parent model fails closed; `agents.<name>` adds a
restriction and inherits `enforce`/`strict` from the global block; a nine-pattern allowlist renders
eight plus `… (9 patterns total)`.

> **CORRECTED 2026-10-03:** line cite: `ModelScopeConfig` is at `exec/model_scope.rs:43` (the row's `:40` is its doc comment); it has the three fields `enforce`, `strict`, `allow` and no `inherit` or `agents`. Read in addition: `parse_model_scope_config` (`exec/model_scope.rs`, from `:274`) reads only `enforce`, `strict` and `allow` and emits no warning for another subkey, which supports the "silently dropped" wording for `modelScope.agents`; a separate key census was not checked.

## SUBA-156 — the MCP direct-tool reader models no `literalEnv`, so agent-plugin servers' digests diverge and their `mcp:` selectors resolve to nothing

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed (both sides read)

**Scope note.** Upstream's `847ee4de` (#2539) and `14dcf975`/`fa1de042` (#2573/#2576) together move
pi-subagents onto pi-mcp-adapter 3.x and onto Pi's own built-in MCP: `getConfigPaths` now reads
`mcp-adapter.json` instead of `mcp.json`, and `usesBuiltinMcp`/`resolveBuiltinMcpSelections`/
`extensionOnlyMcpServers` add a second resolution path for when the adapter is not loaded. **None of
that is filed here**: cyrup ships its MCP port as the default and has no adapter-versus-built-in
split (`EXT-092` records the one place the distinction surfaces), so there is no second path to
resolve against and no file to rename. What is filed is the one half of `#2539` that is a defect in
cyrup on its own terms.

**upstream** — `computeMcpServerHash` (`src/runs/shared/mcp-direct-tool-allowlist.ts:470` @v0.74.0)
now makes `literalEnv` part of the server identity, and skips env interpolation when it is set:

    const isStdio = definition.command !== undefined;
    const literalEnv = isStdio && definition.literalEnv === true;
    … env: literalEnv ? definition.env : interpolateEnvRecord(definition.env),
      ...(isStdio ? { inheritEnv: definition.inheritEnv !== false, literalEnv } : {}),

with the comment "A mismatch leaves that server's cached direct-tool selectors unresolved." It also
hashes `resolveConfigPath(definition.command)` rather than the raw command.

**cyrup** — the reader is `exec/mcp_direct_tools.rs::server_identity_pre_image_with` (`:1012`); the
writer is `cyrup-mcp/src/dirs.rs::server_identity_pre_image` (`:1294`). Both emit the same 15 keys
and **neither** has `inheritEnv` or `literalEnv`, so for an ordinary server they agree — that is why
this is not already broken everywhere. They diverge for `literalEnv` servers:

- `cyrup-mcp` sets `literal_env: Some(true)` on every agent-plugin stdio server
  (`cyrup-mcp/src/agent_plugin.rs:883`, and `runtime.rs:5733`), and its resolver takes those env
  values **verbatim** — no `$VAR` interpolation and no `!secret` resolution
  (`cyrup-mcp/src/secrets.rs:341,355-359,388`). The writer hashes `resolved.env`, i.e. the verbatim
  values.
- the reader's `ServerEntry` has no `literal_env` field at all
  (`grep -n 'literal_env\|inherit_env' exec/mcp_direct_tools.rs` is empty), so it always runs
  `interpolate_env_record(definition.env, env)` (`:1028`).

**Impact** — Any agent-plugin MCP server whose `env` values contain a `$` sequence or a `!`/`!!`
secret token hashes differently on the two sides. The reader's `is_server_cache_valid` then rejects
that server's cache entry, and the agent's `mcp:<plugin>__<server>/<tool>` selectors resolve to
nothing — **silently**, with no diagnostic. This is exactly the failure the module header at
`exec/mcp_direct_tools.rs:32` was written about ("the two disagreed **silently**: every cached entry
failed hash validation (so `mcp:` selectors resolved to nothing)"), reintroduced through a field the
reader does not model. The secondary divergence is `command`: the writer hashes the plugin's
already-absolutised `resolved_command` (`agent_plugin.rs:871`), the reader hashes
`definition.command` unresolved (`:1020`), and upstream now resolves it on both sides.

**Fix** — `S`, and it is a reader-side change plus one shared test. Add `literal_env` and
`inherit_env` to the reader's `ServerEntry`; gate `interpolate_env_record` on `literal_env`; add the
`inheritEnv`/`literalEnv` pair to the stdio identity on **both** sides in the same commit so they
cannot drift again; hash the resolved command on both sides. The crate already has the right place
for the guard: extend
`mcp_direct_tools`' `reader_and_writer_agree_once_every_resolver_actually_runs` (named in the module
header at `:60`) to cover a `literalEnv` server.

**Verify** — A plugin-registered stdio server with `env: { TOKEN: "$HOME/x" }` produces the same
digest from `cyrup_mcp::dirs::compute_server_hash` and
`cyrup_ext_subagents::exec::mcp_direct_tools::compute_mcp_server_hash`, and an agent selecting
`mcp:<plugin>__<server>/<tool>` resolves it; the same server with `literalEnv` honoured only on one
side is red.

---

**Paired with `MCP-594` (reconciled 2026-10-02).** The writer side of the same digest contract is
filed as **`MCP-594`** (`13c-…`, medium, `upstream-drift`): `cyrup-mcp`'s
`server_identity_pre_image` is missing the same two stdio keys and hashes an interpolated `env` for a
`literalEnv: true` entry. It also names the cost this row does not: adding the keys invalidates every
stdio server's metadata-cache entry once, and voids the golden pre-image vectors pinned at
`cyrup-mcp/src/dirs.rs:1694`, `:1734` and `:1808`, which must be regenerated from `v5.0.0` rather than
hand-edited. **Land both sides in one commit**, as both rows require.

> **CORRECTED 2026-10-03:** effort reads S+, not S: the fix touches two crates (`cyrup-ext-subagents` reader and `cyrup-mcp` writer, see `MCP-594`), regenerates golden pre-image vectors and invalidates every stdio server's metadata cache once. Recommended re-rating (not applied): effort S to M. **FOLD-IN 2026-10-03 (v0.75.0):** `3f367e75`/#2607 sanitises `-` to `_` in `resolveBuiltinMcpSelections` (`src/runs/shared/mcp-direct-tool-allowlist.ts:183-189` @v0.75.0) so hyphenated `mcp:` names match Pi's built-in MCP. It belongs to the built-in-MCP path this row's scope note leaves unfiled; cyrup's direct-tool matcher already registers both the hyphen and the `_` spellings of a tool name (`exec/mcp_direct_tools.rs:1576-1590`), and has no built-in/adapter split, so nothing is owed for it.

## SUBA-157 — `subagents.agentOverrides.<name>.advertise` is unported

> **CLOSED 2026-10-08** (on `claude/subagents-robustness-sweep`, checked against pi-subagents ad11b7ab): all three Verify clauses hold. `agentOverrides.reviewer.advertise = true` puts a bundled agent in the `<advertised_subagents>` block (and `false` takes a custom agent with `advertise: true` frontmatter out); `"yes"` is refused with upstream's message; a runtime-registered agent is still not advertised. Upstream cites at ad11b7ab: field `src/agents/agents.ts:91`, parse `:1047-1050`, apply `:1500`, runtime narrowing `:1637-1647`. See the row for the cyrup sites and tests.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**upstream** — `8dc90dca` (#2534, v0.73.0) adds `advertise?: boolean` to
`BuiltinAgentOverrideConfig` (`src/agents/agents.ts:88` @v0.74.0; the agent's own frontmatter field
is `:141`), parsed in `parseBuiltinOverrideEntry` with
`Builtin override '<name>' in '<file>' has invalid 'advertise'; expected a boolean.` and applied in
`applyBuiltinOverride` (`if (override.advertise !== undefined) next.advertise = override.advertise;`).
The commit message states the boundary: settings can now opt a builtin **or custom** agent into the
parent-prompt catalog without copying the agent file, and runtime-registered agents still cannot be
advertised.

**cyrup** — the frontmatter half is in: `AgentDefinition::advertise` at `discovery/types.rs:1352`,
cited to `agents.ts:140` @v0.71.0, feeding `discovery::advertised::build_advertised_agent_prompt`
(`SUBA-133`, closed 2026-09-28). The override half is not: `AgentOverrideConfig`
(`discovery/types.rs:686`) has 20 fields and no `advertise` (`grep -n advertise` over
`discovery/types.rs` and `discovery/merge.rs` finds only `AgentDefinition::advertise` and two
`advertise: None` literals in fixtures).

**Impact** — Low. A setting pi documents is accepted and dropped, so the only way to advertise a
bundled agent in cyrup is to eject and edit it. `SUBA-096` is this row's neighbour: it carries the
three override fields (`acceptance_role`, `output_mode`, `fast`) that `SUBA-081`'s partial closure
left, and `advertise` is a fourth of the same kind. If `SUBA-096` is picked up, do both in one pass.

**Fix** — One `OverrideField<bool>` on `AgentOverrideConfig` beside `description`, parsed with
upstream's message verbatim and applied in `discovery/merge.rs` where the other booleans are.

**Verify** — `subagents.agentOverrides.reviewer.advertise = true` puts a bundled agent in the
`<advertised_subagents>` block; `= "yes"` is refused with upstream's message; a runtime-registered
agent is still not advertised.

> **CORRECTED 2026-10-03:** line cite: `AgentOverrideConfig` is at `discovery/types.rs:673` (the row's `:686` is inside the struct); `advertise` exists only on `AgentDefinition` (`:1352`).

## SUBA-158 — a spawned subagent child does not follow the parent session's project trust

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed (both sides read)

**This escalates `SUBA-142`, by that tracker's own rule.** `SUBA-142` (tracker, `09b:245`) parks
upstream's in-process-child features and says it "escalates to an item … when one of the parked
features is shown to change a result for a spawned cyrup child". Project trust is such a feature:
upstream's fix is in-process (`SettingsManager.create(..., { projectTrusted })`), but the *result*
it corrects — the child loading the project as untrusted while the parent has it trusted — is
reproduced by cyrup's spawned child. The same applies to `4cd43cae`/#2541's `skillsOverride`, which
is **not** filed here because it corrects a Pi-internal path (`extendResources` merging
`resources_discover` skill paths past `noSkills`) that a cyrup child, launched with `--no-skills`
(`exec/spawn_plan.rs:862`), does not take.

**upstream** — `b2718fb8` (#2570, v0.73.1) threads the launching session's trust to every child:
`ChildSessionLaunch.projectTrusted` (`src/runs/shared/child-session.ts:59` @v0.74.0) reaches
`pi.SettingsManager.create(launch.cwd, agentDir, { projectTrusted: launch.projectTrusted })` (`:373`),
and `sessionProjectTrust(ctx)` (`src/runs/foreground/subagent-executor.ts:645`,
`typeof ctx.isProjectTrusted === "function" ? ctx.isProjectTrusted() : undefined`) is passed at
every launch site — `runSinglePath`, `runAsyncPath`, `resumeAsyncRun` (both arms) and
`resumeExternalJobFollowUp`. `undefined` keeps Pi's default for a host with no trust concept.

**cyrup** — nothing trust-shaped reaches the child. `exec/spawn_plan.rs:300-340` documents the whole
argv and env overlay and names no trust flag; `grep -n trust exec/spawn_plan.rs` finds one unrelated
comment at `:490`. The `cyrup` binary's own startup posture is
`project_trusted: false` (`crates/cyrup/src/bootstrap.rs:93`, "cyrup's standing pre-trust posture
(R-07-002)"), and the only override is the CLI `--approve`/`-a` pair
(`crates/cyrup/src/subcommands.rs:57-67`), which the child is not given.

**Impact** — A parent session whose project trust was granted in-session (the ordinary interactive
path — `crates/cyrup/src/interactive.rs:197-198` carries `auto_trust_on_reload_cwd` precisely
because that grant is not yet persisted; `TUI-037` is open for the same reason) spawns children that
load the project as **untrusted**. The child therefore resolves a different settings and
project-resource set than the parent: project-local settings and project extension discovery are
suppressed where the parent has them, and `mcp-direct-tool-allowlist.ts`' own comment in this window
("the project file is read only when the project is trusted") shows upstream treating the project
MCP file the same way. A child quietly running under a narrower configuration than its parent is a
behaviour divergence a user cannot see, which is why this is `medium` and not `low`.

**Fix** — `S`. Decide the channel — a `--approve`-shaped argv flag is the smallest, an env var
beside the existing `INHERIT_PROJECT_CONTEXT_ENV`/`INHERIT_SKILLS_ENV` pair is the most consistent
with how every other inherit flag reaches a cyrup child — then set it from the parent's
`AgentSessionServices::project_trusted` (the field `crates/cyrup-tui/src/app/session_bind.rs:131`
reads) at each launch site, with "absent means the child decides for itself" as the default so a
host with no trust state is unchanged. Record the decision as a `[CYRUP-DELTA]` against
`child-session.ts:373`, since the mechanism differs even though the result must not.

**Verify** — In a project trusted only in the parent session, a foreground child, an async child and
a resumed async child each load the project's local settings; with the parent untrusted, none of
them does; a launch with no trust information behaves exactly as today.

> **CORRECTED 2026-10-03:** the cyrup evidence is mis-aimed. `crates/cyrup/src/bootstrap.rs:93` (`load_startup_settings`) is the process's startup settings manager, used for settings diagnostics and the `sessionDir` lookup; it does not decide the project trust a session runs under. Real trust resolution is `SessionBuilder` in `crates/cyrup-session-svc/src/builder.rs:801-853`: settings loaded with the project untrusted, `default_project_trust`, `has_trust_requiring_resources`, the trust-store `nearest` lookup, then `TrustInputs { trust_override, saved, default_trust, mode, .. }` into `decide_trust_with_extension`. `cyrup-ext-subagents` passes no `--approve`/`--no-approve` and no trust env to the child (no hits in the crate outside unrelated tests), so a child resolves trust itself from the trust store and `defaultProjectTrust`. Direction caveat: upstream's v0.74.0 changelog describes the opposite symptom ("a child in an untrusted project still loaded that project's settings"), so the row's "child less trusting than the parent after an in-session grant" is the mirror case of the same missing hand-off. Neither was reproduced end to end; the defect is plausible but unproven. Recommended re-rating (not applied): medium to low until a spawned child is shown to resolve a different trust than its parent.

## SUBA-159 — runner liveness probes are not scoped to the PID namespace, so a cross-namespace observer fails a live run

> **CLOSED 2026-10-08** (on `claude/subagents-robustness-sweep`, checked against pi-subagents ad11b7ab). The runner records its PID namespace (`pidNamespaceScope`), `reconcile` turns a cross-namespace `Dead` into `Unknown` and uses upstream's current `is still live` / `cannot be probed from this process` stale sentence, and a same-namespace `/proc/<pid>/stat` zombie reads as `Dead` (the #2606 fold-in). The capacity-release and await-run readers still probe without the scope gate; see the row.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**upstream** — `d5d3b9ff` (#2498, v0.72.0) adds `src/runs/background/pid-namespace.ts:7`
`currentPidNamespaceScope()` — the `readlink("/proc/self/ns/pid")` value on Linux, cached,
`undefined` elsewhere — stamps it on `AsyncStatus.pidNamespaceScope`
(`src/shared/types.ts`, "Linux PID namespace identity used to scope liveness probes") at both
status-writing sites (`spawnRunner`, `buildStartedStatus`), and consults it in
`reconcileAsyncRun`:

    const pidScopeMismatch = effectiveStatus.pidNamespaceScope !== undefined && effectiveStatus.pidNamespaceScope !== observedScope;
    const liveness = observedLiveness === "dead" && pidScopeMismatch ? "unknown" : observedLiveness;

(`src/runs/background/stale-run-reconciler.ts:439,441` @v0.74.0). A cross-namespace `dead` becomes
`unknown`, which falls through to the existing 24-hour stale rule instead of failing the run, and the
failure sentence gained a second probe text: `could not be probed from this process` where the
observed liveness was not `alive`. The commit names the two shapes it fixes: another container, and
a macOS/Windows host sharing a container's `PI_SUBAGENTS_TEMP_ROOT`.

**cyrup** — `background/reconcile.rs:144` `check_pid_liveness` is a bare `kill(pid, 0)` with
`ESRCH => Liveness::Dead`, and the decision at `:383` is `Liveness::Dead => true` — fail
immediately, no stale window. `AsyncStatus` has no `pidNamespaceScope`
(`grep -rn 'pidNamespaceScope\|pid_namespace\|/proc/self/ns/pid' crates/` is empty), and the
`Liveness::Alive | Liveness::Unknown` arm still prints only `still has a live PID` (`:406`), so
upstream's second sentence has no counterpart. cyrup does have `processDemonstrablyGone`'s
start-identity check (`:166-174`) against PID **reuse**, which is a different hazard: a PID that
does not exist in this namespace has no start identity to compare.

**Impact** — `low`, honestly scoped: cyrup exposes no documented shared-temp-root env (the run tree
hangs off cyrup's own `TEMP_ROOT_DIR`, `exec/mod.rs:1365`), so reaching this needs a deployment that
shares the state directory across PID namespaces by mount. In that deployment the consequence is not
cosmetic — a **live** run is marked failed and a `synthesize_failure` record is written over it. The
row is worth having because the fix is small and because the reconciler's "dead means dead" rule is
the one place cyrup is strictly less careful than upstream about a probe it cannot trust.

**Fix** — `S`. Add `pid_namespace_scope: Option<String>` to the async status record, populated from a
cached `readlink("/proc/self/ns/pid")` on Linux and `None` elsewhere; stamp it wherever `pid` is
stamped; in `reconcile`, downgrade `Dead` to `Unknown` when a recorded scope is present and differs
from the observed one (including when the observer has none), and add upstream's second probe
sentence.

**Verify** — A status carrying a recorded scope that differs from the observer's, with a PID that is
absent locally, is **not** failed until the stale window elapses, and then fails with the
`could not be probed from this process` sentence; a matching scope, and an absent recorded scope,
both behave exactly as today.

> **FOLD-IN 2026-10-03 (v0.75.0, `806e3678`/#2606, Linux zombie runners):** `checkPidLiveness(pid, kill, probeZombie)` (`src/runs/background/stale-run-reconciler.ts:360-370` @v0.75.0) now reads `/proc/<pid>/stat` after a successful `kill(pid, 0)` and returns `dead` when the state field is `Z`. `probeZombie` is true only when the status's recorded `pidNamespaceScope` equals the observer's (`stale-run-reconciler.ts:446-451`; the same gate in `await-async-run.ts:15-24`). Cyrup's `check_pid_liveness` (`background/reconcile.rs:144`) is a bare `kill(pid, 0)` and reports a zombie as `Alive`. `background/spawn_detached.rs:391` drops the runner's `Child` (`None => drop(child)`), so a runner that outlives its launcher is reparented, and where PID 1 does not reap (a container without an init) it stays a zombie that reads as alive; that is the triager's reading, not run, and it needs no cross-namespace mount. Scope widens: port the `/proc/<pid>/stat` zombie probe together with `pid_namespace_scope`. Recommended re-rating (not applied): low to medium if the init-less-container zombie is judged reachable.

## SUBA-160 — `timeoutMs`/`maxRuntimeMs` have no timer-delay cap although two neighbouring params do

> **CLOSED 2026-10-08** (on `claude/subagents-robustness-sweep`, checked against pi-subagents ad11b7ab). `timeoutMs` and `maxRuntimeMs` above 2147483647 are refused with upstream's `timerDelayOverflowError` sentence on every launch shape, and `action: "resume"` refuses an oversized `timeoutMs`; 2147483647 is accepted. See the row for cites and tests.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**upstream** — `5655f9bb` (#2517, v0.73.0) added `timerDelayOverflowError`
(`src/runs/foreground/subagent-executor.ts:2950` @v0.74.0, over
`MAX_TIMER_DELAY_MS = 2_147_483_647` at `:2948`) returning
`<name> must be a positive integer no larger than 2147483647.`, and applies it in three places: in
`resolveForegroundTimeout`, at the executor entry for both `timeoutMs` and `maxRuntimeMs`, and in
`resumeAsyncRun` for `action: "resume"`'s own `timeoutMs`. The changelog states Node's failure mode:
a larger delay was shortened to about 1 ms, so the run timed out almost immediately.

**cyrup** — the constant and the message already exist, for other params:
`exec/tool_timeout.rs:24` `MAX_TIMER_DELAY_MS`, its error at `:221`
(`{label} must be a positive integer no larger than {MAX_TIMER_DELAY_MS}.`, upstream's string), and
`registration/mod.rs:588` `MAX_CHECKPOINT_BEFORE_DEADLINE_MS` with `SUBA-128`. But
`resolve_foreground_timeout` (`extension/tool/params.rs:566-588`) checks only `value == Some(0)` and
the `timeoutMs`/`maxRuntimeMs` alias clash; both fields are `Option<u64>`
(`extension/tool/params.rs:222-223`), so any value is accepted. `action: "resume"` has no such
check either.

**Impact** — Node's specific corruption does not reproduce: a Rust/tokio deadline built from a huge
`u64` saturates rather than wrapping to ~1 ms, so the practical effect is "no deadline" rather than
"instant timeout". The defect is the inconsistency: the tool refuses an oversized `toolTimeoutMs`
and `checkpointBeforeDeadlineMs` with a specific message and accepts an oversized `timeoutMs`, which
is the opposite of this crate's advertise-and-dispatch-agree rule. `low`, and it is worth doing
only because it is three lines against a constant that is already in the tree.

**Fix** — Apply `exec::tool_timeout`'s existing overflow check to `timeout_ms` and `max_runtime_ms`
in `resolve_foreground_timeout`, and to the `resume` arm's `timeoutMs` in
`extension/executor/background.rs` beside the existing `checkpointBeforeDeadlineMs` check. Reuse the
existing message builder rather than restating the string.

**Verify** — `timeoutMs: 2_147_483_648` and `maxRuntimeMs: 2_147_483_648` are each refused with
upstream's message before any launch; `action: "resume"` with the same value is refused;
`2_147_483_647` is accepted.

> **CORRECTED 2026-10-03:** the Impact paragraph's claim that "a Rust/tokio deadline built from a huge `u64` saturates" is unverified; `Instant + Duration` arithmetic can panic on overflow, so the real effect of an oversized value may be a panic rather than "no deadline". The defect and the cites (`params.rs:566-588` checks only zero and the alias clash) stand.

## SUBA-161 — the `council` guide topic and the multi-file guide body are unported

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**upstream** — `a0fd73df` (#2469, v0.72.0) makes `council` an eleventh guide topic
(`src/extension/subagent-guide.ts:16` @v0.74.0) and gives `readSubagentGuide` a multi-file arm
(`:39`): a topic may name several files, and when it does the body is their contents joined with a
`<!-- <path> -->` marker before each. `COUNCIL_FILES` (`:25`) is
`skills/council-mode/SKILL.md`, `skills/council-mode/references/pass-contracts.md` and
`skills/pi-subagents/references/execution-controls.md`, and the comment says why the topic exists at
all: "`/council` must work under `pi --no-skills`, which drops the package skills from context, so
this topic serves the council skill together with the references it tells the model to read."

**cyrup** — `registration/guide.rs:44` `SUBAGENT_GUIDE_TOPICS` is the ten-topic v0.47.1 list, and the
doc above it records that the order is load-bearing twice (the unknown-topic message joins it, and
`overview` first makes it the default). `council` has **zero** hits in `crates/` and zero in
`docs/gap-analysis/` — neither the topic nor the `council-mode` skill it serves has ever been filed.

**Impact** — Low and bounded: `guide topic: "council"` answers
`Unknown subagents guide topic 'council'. Valid topics: …`. Behind it is the larger question of
whether cyrup ships pi-subagents' bundled skills at all — upstream carries nine files under
`skills/` at v0.74.0 — which is a bundled-asset decision, not this row's work.

**Fix** — `S` for the mechanism, and the mechanism is what to port: add the multi-file arm and its
`<!-- path -->` marker to the guide reader, since that is reusable, and add `council` as the
eleventh topic **last** in the list, preserving the order rule. Whether the three files ship is a
separate decision; if they do not, the topic should not be advertised, because this crate does not
advertise a topic it cannot answer.

**Verify** — `guide` with `topic: "council"` returns the three files in order, each preceded by its
`<!-- path -->` marker; the unknown-topic message lists eleven topics in upstream's order;
`overview` is still the default.

> **CORRECTED 2026-10-03:** effort S is optimistic (S to M): a working `council` topic needs the three skill files bundled (`SUBA-161`'s own Fix says whether they ship is a separate decision), and v0.75.0 changes eight guide docs (`docs/agents.md`, `configuration.md`, `extension-api.md`, `missions.md`, `models.md`, `observability.md`, `tool-reference.md`, `workflows.md`). Recommended re-rating (not applied): effort S to M.

## SUBA-162 — the progressive async-widget tier is unported: no height lock, no lane rows, no running-agent header count

> **CLOSED 2026-10-10** (checked against pi-subagents ad11b7ab = v0.76.1-29; clone HEAD differs in `src/tui/render.ts` only by #2808's removal of `buildWidgetComponent`'s `layout = "adaptive"` default, no behaviour change). Upstream cites at the pin: `runningLeafAgentCount` `render.ts:2637`, `progressiveHeaderLine` `:2650`, `buildProgressiveWidgetLines` `:2706`, `fitAdaptiveWidgetLines` `:2765-2820`, `buildWidgetComponent` `:2911`, `handleMouse` `:2938-2944`, `renderWidget` `:3091`; `asyncWidgetCollapsed`/`asyncWidgetLayout` validated `extension/config.ts:187-191`, resolved `extension/index.ts:294-295`; fleet `activeLeafAgentCount` `fleet-status.ts:346-348`, workflow wrapper `:439-464`. Two corrections to the text below: "never shrinks between relocks" was superseded before the pin by `8d804895`/#2662 (the lock shrinks to content when root jobs leave, and still holds while a finished job is listed), and the cyrup fleet file is `tui/fleet_status.rs` (`N active agents`), not `tui/fleet_view.rs`. Each Verify clause holds and is red-proved: the four-lane workflow beside one run reads 5 in both the fleet line and the widget header (the old running-root rule reads 2 or 6); a sequential chain counts 1 (proved against the pending-step exclusion mutation, not the old rule: one running chain is one running root, so the pre-#2584 header also reads 1); a running job with no step detail counts its `agents` (1 with none, as fleet does); the locked card keeps its height as jobs finish and grows to the cap, not past it, when a job starts while jobs are hidden. The FOLD-IN is split: `asyncWidgetCollapsed` is ported (with `asyncWidgetLayout`); the click half is not portable inside this crate, because cyrup routes a press on an extension widget to text selection (`cyrup-tui/src/app/pointer.rs`) and `HostServices::set_widget` has no callback, so it is filed as **`SUBA-210`**. Production reach (terminal size, `agents` roster) is filed as **`SUBA-211`**. See the row for the cyrup sites, deviations and tests. Review follow-up: the collapsed fleet line now drops its leading separator when the label is empty and, with a workflow wrapper present, reads `usage on child rows` (or `standalone: <tokens> · workflow usage on child rows`) as pi does (`fleet-status.ts:812-817` @ad11b7ab), tested by `tui::fleet_status::tests::a_collapsed_fleet_of_only_a_workflow_wrapper_reads_usage_on_child_rows`; cyrup's fleet entries have no `external`/`parentKey`, capacity or window fields, so pi's other summary parts are not modelled. The four-lane parity test also asserts the widget with the lanes attached under the workflow (`parent_workflow_run_id`), which exercises the recursive child count.

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read)

**upstream** — `src/tui/render.ts` at v0.74.0 has a progressive widget tier that cyrup's port does
not: `buildProgressiveWidgetLines` (`:2704`) locks the card height to the rows its content fills at
lock time, never shrinks between relocks, grows up to the compact cap when a job starts while jobs
are hidden, and fills the rows left after the visible job lines with the visible **workflows' lane
rows** (`9a5a2d5e`/#2583 — before it, the tier padded with blank rows and a workflow's lanes stayed
hidden). `progressiveHeaderLine` (`:2648`) prints a running/queued/failed summary line, and
`runningLeafAgentCount` (`:2635`, `7e07a22d`/#2584) makes its count agree with FleetView's
"N active agents": a workflow counts its loaded child runs recursively, any other running job counts
its active steps, synthesised from `agents` when it has no step detail, with a sequential chain's
non-current `pending` steps excluded. Before the fix a four-lane workflow beside one other run read
"2 agents running" next to "5 active agents".

**cyrup** — `tui/render.rs` is 841 lines against upstream's ~2 700 and is a port of the earlier
shape: `render_background_region` (`:386`) renders one full header-plus-nested block per run up to a
cap and folds the overflow into an aggregate line (`background_region_details_up_to_cap_then_folds_overflow`),
`render_progress_header` (`:356`) is the per-run block, and `render_async_jobs_widget`
(`tui/events.rs:874`) is a thin wrapper that maps snapshots and calls it. There is no header summary
line (`grep -rn 'agents running' crates/cyrup-ext-subagents/src/` is empty), no height lock, no
progressive tier and no lane rows. `SUBA-061`'s closure wired the `asyncWidget` config key to this
renderer, so the slot and its plumbing exist; the tier does not.

**Impact** — Low: a cosmetic difference in an under-editor widget. It is filed because the two
upstream fixes in this window both describe the symptom in user terms (blank rows where a
workflow's lanes belong; a count that disagrees with the Fleet view a key away), and because the
leaf-counting rule is the kind of thing that is cheap to port with the fix in hand and expensive to
re-derive later.

**Fix** — `M`, and splittable. The `runningLeafAgentCount` rule is pure and portable on its own
against cyrup's `AsyncJobSnapshot`/`StepStatus`, and is the half worth doing first: it makes the
widget and `tui/fleet_view.rs` agree. The height lock and lane rows need a locked-rows value
threaded through `render_async_jobs_widget`, which cyrup's `data in -> Vec<Line>` contract
(`tui/render.rs:7`) can carry as a parameter without giving the module state.

**Verify** — A four-lane workflow beside one single run reports the same agent count in the widget
header as `tui/fleet_view.rs` reports active agents; a sequential chain counts one; a job with no
step detail counts its agents; the locked card keeps its height as jobs finish and grows, up to the
cap, when a job starts while jobs are hidden.

> **FOLD-IN 2026-10-03 (v0.75.0, `53aee6d8`/#2621):** also unported are upstream's header-click fold (`93d47c0c`/#2235, v0.68.0; `buildWidgetComponent`, `src/tui/render.ts:2903` @v0.75.0) and v0.75.0's boolean `asyncWidgetCollapsed` (validated `src/extension/config.ts:175-176`, passed as `initiallyCollapsed` at `render.ts:3099`). `grep -rn 'asyncWidgetCollapsed' crates/` is empty, so a user who sets it gets only the generic unknown-key stderr warning. Not verified: whether the cyrup TUI can deliver clicks to an extension widget, which decides whether the click half is portable.

## SUBA-163 — `/subagent-cost` reports no child usage for an async single, chain or parallel launch

> **CLOSED 2026-10-08** (on `claude/subagents-robustness-sweep`, checked against pi-subagents ad11b7ab): the `asyncId`-with-empty-`results` arm, the `completions` set, the `indexes` parameter and the `identity` override are ported. The workflow arm is gated on `asyncId`, and a missing receipt counts as unresolved. Foreground workflow `children` usage (success and failure arms) is counted per round. See the table row for the cites and the tests.

**Kind** stale-port · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**Why `stale-port`.** `SUBA-138` (closed 2026-09-28, `09b:241`) ported v0.71.0's
`collectSubagentCost` as `registration/cost.rs:995`, and that port is faithful to its tag.
`d556bb01` (#2490) landed in **v0.72.0**, one tag later, and fixes a hole the v0.71.0 collector has.
So this is not a defect in `SUBA-138`'s work; it is the port being measured against a tag that has
moved.

**upstream** — `collectSubagentCost` at v0.74.0 tracks three id sets, not one
(`src/slash/subagent-cost.ts:162-164`): `workflowRunIds`, and the new `asyncRunIds` (`:163`) and
`completedRunIds` (`:164`). The new arm is

    if (details.mode === "workflow" && details.runId) workflowRunIds.add(details.runId);
    // An async launch result has no child results; its usage lands in run artifacts.
    else if (details.asyncId && details.results.length === 0) asyncRunIds.add(details.asyncId);

plus `completedRunIds.add(completion.runId)` for every `bg_wait` completion, an `identity` override
on `addChild` so a step's usage dedupes by its own identity, and an `indexes` parameter on
`metadataUsage` so a run's per-step artifact metadata is read rather than only index `0` and the
unindexed form. The changelog states the symptom: `/subagent-cost` "only read child usage from
workflow receipts and returned results, so an async `subagent` call with an empty `results` list
reported no child usage".

**cyrup** — `registration/cost.rs:1002` declares `workflow_run_ids` and nothing else;
`add_workflow_run_id` is called at `:1022` and `:1052`, the resolution loop at `:1087` walks
`workflow_run_ids` alone, and `unresolved_async_children` (`:1086`) counts only failures inside that
loop. `grep -n 'async_run_ids\|completed_run_ids' registration/cost.rs` is empty. The artifact
metadata read is likewise the two-index form the pre-fix upstream had.

**Impact** — `low`, because it is a report rather than an execution path, but it is a wrong number
shown to the user, not a missing feature: an `async: true` single, chain or parallel launch is the
common case, its tool result carries an `asyncId` and an empty `results` array, and
`/subagent-cost` attributes zero child usage to it. The run's usage is on disk in its artifact
metadata the whole time.

**Fix** — `S`. Add the `asyncId`-with-empty-`results` arm and the `completions` id set to the
collector's transcript walk, resolve those run ids through the same run-dir status / artifact
`_meta.json` path `SUBA-138` already built (the status's steps give the indices), and add the
`indexes` parameter so per-step metadata is read. Keep the existing `run:`/`session:` dedupe and add
upstream's `identity` override so a step does not collide with its run.

**Verify** — An `async: true` single launch, an async chain and an async parallel launch each
contribute their children's usage to `/subagent-cost` and to the RPC `cost` report; a `bg_wait`
completion for the same run does not double-count it; a workflow run still resolves through its
receipt exactly as it does today.

> **CORRECTED 2026-10-03 (stale by v0.75.0):** the port target moved again; `ae9c9d77`/#2612 and `4998ceca`/#2615 change the same function (`git diff v0.74.0 v0.75.0 -- src/slash/subagent-cost.ts`). (a) A workflow id joins `workflowRunIds` only when `details.mode === "workflow" && details.runId && details.asyncId`: foreground workflow child usage is already in `results`, and only async workflows persist a receipt. (b) The child `runId` is read from the typed `result.runId`, with no cast. (c) Every round of a resumed foreground workflow child counts (#2612). (d) A missing or unreadable receipt now adds to `unresolvedAsyncChildren` (an `ENOENT`, also via `error.cause`, is not logged; other errors are), so an async workflow with no receipt yet is listed as `Async child usage unavailable` (#2615). Cyrup diverges on both ends (read, not run): `collect_subagent_cost` (`registration/cost.rs:995`) adds a workflow id on `mode == "workflow"` plus `runId` (`:1014-1020`), but cyrup's foreground workflow details carry `mode`, `children`, `workflowRunId` and no `results`, `runId` or `asyncId` (`extension/executor/workflow_launch.rs:456-470`, `:697`), so foreground workflow child usage is never counted; and a `NotFound` receipt is a silent `continue` (`cost.rs:1102`) instead of an unresolved count. Scope of the fix therefore widens beyond the `asyncId`/`completions` arms: add the arm, gate the workflow arm on `asyncId`, count foreground workflow `children` usage, and count unresolved on a missing receipt.

## Findings filed 2026-10-03 — the `v0.74.0..v0.75.0` window

`pi-subagents` **v0.75.0** (31 non-merge commits since v0.74.0), read through `git -C tmp/pi-subagents show v0.75.0:<path>` and `git diff v0.74.0 v0.75.0`; cyrup read at `11664557`. This file's window is therefore now `v0.57.0..v0.75.0`. Nothing was run. Every commit has a disposition. **Existing rows extended** (see the `FOLD-IN`/`CORRECTED` notes on the rows): `SUBA-022` (#2643, #2640), `SUBA-142` (#2636, #2638), `SUBA-150` (#2610, #2611), `SUBA-156` (#2607), `SUBA-159` (#2606), `SUBA-162` (#2621, and the earlier #2235), `SUBA-163` (#2612, #2615), and `09`'s refuted-`SUBA-042` note (#2628). **New rows:** `SUBA-164` (#2613), `SUBA-165` (#2598), `SUBA-166` (#2624), `SUBA-167` (#2603), `SUBA-168` (#2616), `SUBA-169` (#2604), `SUBA-170` (#2618), `SUBA-171` (#2627 residual), `SUBA-172` (#2620), `SUBA-173` (#2619). **Already matched:** #2605 (`extension/host/native_impl.rs:690-712` logs the drain failure, then still raises the goal notices) and the atomicity half of #2627 (`SUBA-029`). **Nothing owed:** #2649 (refactor), #2648 (CI), #2647 (Windows `.cmd` launch), #2634 (JS module resolution), #2633 and #2626 (tests), the release commit, and #2630 (background `expand` steps with an external-runner agent: cyrup takes the runner from the persona, `runner_main/status.rs:665`; whether materialised `expand` status rows show the runner was not checked). The `v0.71.0..v0.74.0` commits with no row (about 25 PR numbers, mostly workflow and retention work: #2572, #2547, #2536, #2523, #2507-#2510, #2566, #2568) were noted by the review as untriaged and still are.

## SUBA-164 — `tool_open_threshold` attention is unported, so a child stuck in one long tool call never raises `needs_attention`

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed (both sides read; nothing run)

Filed 2026-10-03 from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0, cyrup `11664557`). 
**upstream** — The reason has been in pi-subagents since v0.50.0 (`a660ea30`, "add smart tool timeout defaults"); it is unledgered. `shouldEmitOpenToolAttention` (`src/runs/shared/subagent-control.ts:114-122` @v0.75.0) is true when control is enabled, a tool is open, it is not timeout-exempt and it has been open at least `activeNoticeAfterMs`. The foreground `updateActivityState` (`src/runs/foreground/execution.ts:915-933`) and the runner's (`src/runs/background/subagent-runner.ts:2709-2781`) emit `needs_attention` with `reason: "tool_open_threshold"`, the tool name, path and duration. v0.75.0 (`478f871c`/#2613) makes this per call: each active call carries an `attentionEmitted` flag (`subagent-runner.ts:2646,2710,2765`), the event carries `toolCallId`, and the dedupe key includes it (`subagent-control.ts:193`), so two long calls in one child give two notices instead of one per child. For a `bash` call the nudge text (`subagent-control.ts:253-259`) points at `command.status/yield/cancel` (`SUBA-165`).

**cyrup** — `ControlEventReason` has seven variants and no `ToolOpenThreshold` (`exec/control.rs:301-322`). `derive_activity_state` returns `None` whenever a tool is in flight (`:475`, `current_tool.is_some_and(...)`), so idle detection is off while a call is open. `update_activity_state` (`:1897-1919`) has only the idle branch and the elapsed/turn/token long-running triggers; there is no open-tool branch. `ControlEvent` (`:345-395`) has no `tool_call_id`.

**Impact** — A child hung inside one `bash` call raises no `needs_attention`; the parent hears only the generic elapsed/turn/token long-running notice, which names no call and does not say that a tool, not the model, is what is stuck. Medium because it is the notice upstream added specifically for the hung-command case, and v0.75.0 just fixed its per-call behaviour.

**Fix** — Add the `tool_open_threshold` variant and a `tool_call_id` field on `ControlEvent`; track an `attention_emitted` flag per active call; add the open-tool branch to `update_activity_state` after the idle check, with upstream's timeout-exempt rule; key the notification dedupe on the call id. Port the `bash` nudge wording only together with `SUBA-165`.

**Verify** — Two tools open past `activeNoticeAfterMs` in one child give two `needs_attention` notices, one per call, each with its own `toolCallId`; a call that finishes before the threshold gives none; the same call does not notify twice.


## SUBA-165 — Opt-in `subagent_command` supervision (`command.status`, `command.yield`, `command.cancel`, `toolCallId`, bash `yieldTimeMs`) is unported

**Kind** not-ported · **Severity** low · **Effort** L · **Confidence** confirmed (both sides read; nothing run)

Filed 2026-10-03 from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0, cyrup `11664557`). 
**upstream** — v0.75.0 `72682254`/#2598 adds opt-in command supervision for native Pi children granted both `bash` and `subagent_command`. `createChildCommandRuntime(channelDir)` (`src/runs/shared/child-commands.ts:49`) owns command lifetimes, not shell execution: it wraps the child's `bash` with an optional `yieldTimeMs` (`:119-139`, 0 to 30 000 ms; the command keeps running and a handle is returned), persists `commands.json` in the channel directory and keeps the last 20 commands (`RECENT_COMMANDS`, `:34`). `commandAction` (`src/runs/foreground/command-action.ts:24`) serves `command.status`, `command.yield` and `command.cancel` from the parent, addressed by the new `toolCallId` parameter (`src/extension/schemas.ts:184`); all three are in `SUBAGENT_ACTIONS` (`src/shared/types.ts:2865`). The `tool_open_threshold` nudge for `bash` (`subagent-control.ts:253-259`) tells the parent which command to inspect (`SUBA-164`).

**cyrup** — `SUBAGENT_ACTIONS` (`extension/tool/text.rs:265` onward) has no `command.*` verbs. `grep -rn 'yield_time\|yieldTime' crates --include=*.rs` is empty, so a child's `bash` cannot yield. The control inbox carries one request type, `interrupt` (`background/control.rs:378`, `InterruptRequest`), and its presence is the whole state.

**Impact** — The parent cannot cancel or yield one runaway command; the only lever is interrupting the whole run, which upstream's own hint says is run-scoped and may affect siblings. Low because it is opt-in upstream and needs both tools granted. Upstream builds it on the in-process child (`SUBA-142` family); a spawned child needs the wrapper on its own side.

**Fix** — A child-side `bash` wrapper, a `command` request type on the control inbox next to `interrupt`, and three actions gated like `stop`/`steer`. This needs a bash yield seam in area 04 (built-in tools), which is the larger half and the reason for `L`.

**Verify** — A yielded command returns a handle and keeps running; `command.status` reports it; `command.cancel` stops only that call and the run continues.


## SUBA-166 — Any invalid config value makes cyrup load all-defaults with only a stderr warning, dropping `authorityPolicy` and `permissions`

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed (both sides read; nothing run)

Filed 2026-10-03 from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0, cyrup `11664557`). 
**upstream** — `FAIL_CLOSED_CONFIG_KEYS` (`src/extension/config.ts:17` @v0.75.0) is `worktreeProvider`, `worktreeBranchPrefix`, `modelResponseAliases`, `modelExclusions`, `checkpointBeforeDeadlineMs`, `disabledFeatures`, `scheduledRuns`, `toolActivation`, `authorityPolicy`, `permissions`, `toolBudget`; v0.74.0 had the first eight. `loadConfig` (`:226`) catches a validation failure, re-reads the file, and rethrows if it contains any of those keys (`:234`); only a file with none of them falls back to `{}` with a logged error. The comment above the list: policies "must not be silently discarded and replaced by the built-in defaults after validation fails". The v0.75.0 changelog states the failure it fixes: an invalid value for any key "silently drops `authorityPolicy`, `permissions`, or `toolBudget`".

**cyrup** — `crates/cyrup/src/subagent_config.rs:68-73` runs `SubagentExtensionConfig::validate_raw_config` (`registration/mod.rs:855-865`) and, on any `Err`, prints `cyrup: warning: ... is invalid (...); using defaults` and returns the all-defaults config. The typed-parse error arm does the same (`:91-97`). There is no list of keys whose presence turns a failure into a hard error. `validate_authority_policy` exists (`registration/mod.rs:702`) and so a malformed `authorityPolicy` is caught, but its consequence is the defaults. `permissions` is carried raw and validated at use, not at load (`registration/mod.rs:526-536`); `toolBudget` is an unported key (`UNPORTED_CONFIG_KEYS`, `:576`).

**Impact** — The stderr warning does exist, but the whole file is discarded. A typo in `artifactDir` in a `config.json` that also sets `{"authorityPolicy":{"stopRun":"forbid"}}` or permission rules lifts those restrictions for the session, and a one-line stderr message at startup is the only trace. Medium because it fails open on a policy surface.

**Fix** — Port the 11-key list. When `validate_raw_config` or the typed parse fails and the raw object contains any listed key, abort the load with the same message (a hard error naming the path) instead of defaulting. Unported keys in the list (`toolBudget`, `disabledFeatures`, `toolActivation`) still count as present, so the list is complete before they are ported.

**Verify** — A bad `artifactDir` together with `authorityPolicy` makes the load fail with the path; a bad `artifactDir` alone still defaults with the warning; a bad value beside `permissions` or `toolBudget` fails.


## SUBA-167 — Claude Code adapters accept no per-launch model or thinking level (`--model`, `--effort`)

> **CLOSED 2026-10-09** (checked against pi-subagents v0.76.1). The resolver, scope check and saved-machine refusal are ported into `exec/external_cli/adapters/claude_code.rs`, and `exec::run_sync` applies them before the runner dispatch from the launch's model as typed (`RunOptions::launch_model`) and the agent's level, appending `--model`/`--effort` after `--no-chrome`. Both launch sites stop inheriting the parent session's thinking into, and stop resolving a native model for, an external runner, and the two Claude Code adapters may declare `model`/`thinking` frontmatter. The result's `model` stays `None`, as upstream's does. The preflight `model_scope` reason code has no cyrup surface (N/A). See the table row for deviations and the Verify tests.

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read; nothing run)

Filed 2026-10-03 from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0, cyrup `11664557`). 
**upstream** — `resolveClaudeCodeOverride` (`src/runs/shared/claude-code-adapter.ts:132-172` @v0.75.0) splits a known thinking suffix off `model` (`claude-opus-5.5:high` sets both, `:high` sets only the effort), validates the model against `CLAUDE_CODE_MODEL_PATTERN`, checks the level against the thinking ceiling, and returns `--model X` and `--effort Y`. `CLAUDE_CODE_EFFORT_BY_THINKING` (`:58-66`) maps `off` to no flag, `minimal` and `low` to `low`, and `medium`, `high`, `xhigh`, `max` to themselves. The tokens are appended after the fixed argv (`resolveClaudeCodeLaunch`'s `overrideArgs`, `:233,263`), so the `--version`/`--help` probes never see them. `assertClaudeCodeModelScope` (`:180`) fails closed under an enforced `modelScope`. Frontmatter `model` and `thinking` are exempt from the external-runner Pi-only-field refusal for these adapters (`src/agents/agents.ts:2091-2093`). A new preflight code `model_scope` reports a scope failure. An unknown model or level fails the launch and names it.

**cyrup** — `launch_args` (`exec/external_cli/adapters/claude_code.rs:75-101`) builds a fixed argv with no model or effort. `PI_ONLY_FIELDS` (`runner/mod.rs:190-207`) lists `model` and `thinking` among the fields an external runner refuses. `exec/external_cli/mod.rs:402-403` sets `model: None` for an external-runner result (comment: "Upstream resolves no model for an external runner at all"); whether a Claude Code result should now carry the override model was not checked.

**Impact** — The Claude model and effort for a `claude-code` child can only be set in the user's global Claude Code settings, never per agent or per launch. Low.

**Fix** — Port `resolveClaudeCodeOverride`, the effort map, the thinking-ceiling and `modelScope` checks and the `model_scope` preflight code; let `PI_ONLY_FIELDS` skip `model`/`thinking` for the two Claude Code adapters, as `agents.ts:2091-2093` does; append the override tokens after the fixed argv.

**Verify** — A fake `claude` records `--model X --effort high` for `model: "X:high"`; a bare `:high` sets only `--effort high`; `off` passes no effort flag; an unknown level fails the launch; under an enforced `modelScope` an out-of-scope model is refused.


## SUBA-168 — `schedule.create` still refuses `missionId`: schema-version-2 mission-bound schedules are unported

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read; nothing run)

Filed 2026-10-03 from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0, cyrup `11664557`). 
**upstream** — `ScheduleTarget` gains `missionId?` (`scheduled-runs.ts:45`); the record's `schemaVersion` is `1 | 2` (`:48,313`) and version 2 is required exactly when `target.missionId` is present (`:326`). `schedule.create` validates the id (`:456-459`), checks the mission exists with `readMission` (`:640-641`) and stores `schemaVersion: 1` or `2` accordingly (`:661`); `schedule.list` and `schedule.show` print the mission (`:685,690`). At fire time `executionParams` omits `mission: false` when the schedule is bound (`:476`), so the workflow's `state` persists across fires. The changelog: other schedules keep the old format, and older pi-subagents versions reject version 2 "instead of silently dropping the mission".

**cyrup** — `schedule.create` returns `Mission attachment is deferred from this first schedule slice.` (`background/scheduled_runs/tool.rs:468-478`, message at `:475`) for `missionId`, `mission` and the other mission params. The record codec accepts only `SCHEDULE_VERSION` (1): the serializer writes the constant (`schedule.rs:57`) and `parse_schedule` rejects anything else (`:618`).

**Impact** — A scheduled workflow cannot keep `state` across fires. Low.

**Fix** — Add `mission_id` to the target; write version 2 exactly when it is set and make the reader accept both versions with upstream's equivalence check; validate with `read_mission` at create; skip `mission: false` at fire when bound; show the mission in `list` and `show`.

**Verify** — A version-2 record round-trips; a version-1 reader rejects version 2; a missing mission fails at create; a bound schedule's workflow sees the same mission state on the next fire.


## SUBA-169 — One unreadable goal mission aborts continuation notices for every healthy goal mission

> **CLOSED 2026-10-08** (on `claude/subagents-robustness-sweep`, checked against pi-subagents ad11b7ab). Each goal mission is evaluated on its own (`missions/goal_driver.rs` `evaluate_goal_mission`); a failure goes to `on_error` with the mission id and the scan continues, and `extension/executor/notices.rs` no longer returns `0` on the first bad mission. `read_linked_run` wraps a read or parse failure as `Failed to read linked run status '<path>': …`, leaves the link unchanged when the file is missing, and refuses a non-object (`goal-driver.ts:33-40` @ad11b7ab). Verify: `missions::goal_driver::tests::a_damaged_goal_mission_is_reported_and_a_healthy_one_still_notifies` (one notice plus one report), plus `a_non_object_linked_run_status_is_reported_with_its_path`, `an_unreadable_linked_run_status_is_wrapped_and_a_missing_one_is_not_an_error` and `an_unreadable_mission_state_is_scoped_to_its_own_mission`.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; nothing run)

Filed 2026-10-03 from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0, cyrup `11664557`). 
**upstream** — `collectGoalContinuationNotices` (`src/missions/goal-driver.ts:127` @v0.75.0) iterates `listMissions(...).records` and wraps the per-mission refresh, budget update and notice build in `try { ... } catch (error) { onError(listed.id, error); }`, with a default `onError` that logs `Failed to evaluate goal mission <id>`. `readLinkedRun` (`:30-40`) wraps a malformed `status.json` parse in `Failed to read linked run status '<path>': ...`. The changelog: one mission with a damaged run status or state no longer stops notices for healthy goal missions, and the damaged mission is reported separately.

**cyrup** — `collect_goal_continuation_notices` (`missions/goal_driver.rs:486-554`) calls `read_mission(...)?`, `refresh_goal_mission(...)?`, `update_mission(...)?` and `next_ready_action(...)?` inside the loop, so the first failing mission returns `Err` for the whole call. The caller (`extension/executor/notices.rs:735-745`) logs `Failed to evaluate goal missions` and returns `0`. `read_linked_run` (`goal_driver.rs:125-146`) maps a JSON parse failure to `MissionError::invalid(err.to_string())` without the status path. (The sibling `#2605`, notices after an auto-drain failure, is already matched: `extension/host/native_impl.rs:690-712` logs the drain failure and still calls `raise_goal_continuation_notices`.)

**Impact** — One damaged mission silences every goal notice for the session. Low.

**Fix** — Evaluate each mission under a `match`: on error log with the mission id and `continue`. Wrap the status parse error with its path, as upstream does.

**Verify** — A healthy and a corrupt goal mission in one store yield one notice plus one report.


## SUBA-170 — The global mission list shows the index entry, not the mission record

> **CLOSED 2026-10-08** (on `claude/subagents-robustness-sweep`, checked against pi-subagents ad11b7ab). `missions/store.rs` `list_global_missions` now projects the record over its pointer as `listGlobalMissions` does (`store.ts:563-572` @ad11b7ab): title, status, `updatedAt` and `lastRunId` come from the record, `lastRunId` is dropped when the record has no runs, the sort runs over the projection, and the pointer is not rewritten. Verify: `missions::store::tests::a_stale_global_pointer_lists_the_records_title_status_and_last_run`, `a_record_with_no_runs_lists_no_last_run_id`, `the_global_list_is_sorted_by_the_records_updated_at`.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; nothing run)

Filed 2026-10-03 from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0, cyrup `11664557`). 
**upstream** — `listGlobalMissions(globalIndexDir)` (`src/missions/store.ts:554` @v0.75.0) parses each index entry, then reads and parses the record it points at, checks the id and builds a projection `{ ...entry, title: record.title, status: record.status, updatedAt: record.updatedAt }` with `lastRunId` from the record's last run (`:565-575`). The changelog: a mission no longer looks out of date because its index entry is stale.

**cyrup** — `list_global_missions` (`missions/store.rs:1372`) reads the record only to produce a verdict (`parse_mission_record` plus the id check, `:1424-1435`) and pushes `GlobalMissionIndexRecord { entry, stale, stale_reason }` with the index `entry` as read (`:1437-1446`). The list is then sorted by the entry's `updated_at` (`:1449`), so a lagging pointer also misorders it.

**Impact** — A mission shows an old status, title or last run when its index pointer lags the record. Low.

**Fix** — On a readable, id-matching record overlay the four fields (and drop `last_run_id` when the record has no runs, as upstream does); do not rewrite the pointer. Sort after the overlay.

**Verify** — A stale pointer lists the record's status and title; a record with no runs lists no `lastRunId`.


## SUBA-171 — Settings save replaces a symlinked or restricted `settings.json` with a default-mode regular file

> **CLOSED 2026-10-08** (on `claude/subagents-robustness-sweep`, checked against pi-subagents ad11b7ab). Settings saves resolve the physical target through symlinks (`resolve_settings_write_target`, port of `src/shared/settings-file-lease.ts:6-30`), lock that file, refuse an existing file that cannot be opened for writing, and keep its exact mode across the atomic rename (`cyrup_config::lock::write_atomic_with_mode`, per `src/agents/agents.ts:950-978`). See the `## Open items` row.

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; nothing run)

Filed 2026-10-03 from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0, cyrup `11664557`). 
**upstream** — `writeSettingsFile` (`src/agents/agents.ts:947`) first resolves the write target (`resolveSettingsWriteTarget`, `:977`: `realpath`, or the link text of a dangling link when its physical parent exists), stats the target for its mode, `accessSync(W_OK)`, and passes the existing mode (plus owner write) to the atomic writer, `chmod`ing the temp to the existing mode before the rename. #2627 is the atomicity change; `SUBA-029` covers the atomic half (cyrup already writes temp then rename). The changelog: "if the save is interrupted, the previous settings stay readable".

**cyrup** — `write_settings_file` (`discovery/settings_write.rs:102-111`) serialises and calls `cyrup_config::lock::write_atomic(path, bytes, false)`. `write_atomic` (`cyrup-config/src/lock.rs:337-377`) opens `<name>.tmp.<pid>` in the target's parent with `create(true)` and no mode (the umask default when `secret` is false), `sync_all`s it and `std::fs::rename`s it over `path` itself. It never resolves a symlink, never stats the old mode and never checks writability.

**Impact** — Saving through a dotfile-managed symlink replaces the link with a plain file; a `0600` settings file becomes `0644` (umask-dependent); a read-only file is overwritten. Low, and only reachable through the builtin-override save path.

**Fix** — Resolve the target first (follow symlinks, as `resolveSettingsWriteTarget` does), stat its mode, check `W_OK`, and give the temp the existing mode before the rename. `write_atomic` is shared with other writers, so add a variant or parameter rather than changing its defaults.

**Verify** — Saving through a symlink updates the link's target and leaves the link in place; a `0600` file stays `0600`; a read-only file is refused.


## SUBA-172 — `run-history.jsonl` has the pre-hardening shape, and foreground runs are never recorded

> **CLOSED 2026-10-09** (checked against pi-subagents v0.76.1). `background/run_history.rs` ports `run-history.ts` 1:1 — redacted task + sha256 `taskHash`, explicit `outcome`, 0700/0600 hardening, in-place sanitizing of legacy lines, and `loadRunsForAgent`'s rotation (on read only, as upstream) — and `plan_background_run_history` drives the async runner's tail off a real per-dispatch launch fact; the foreground single path now records its terminal result (attached settle and the plain-detach continuation). **Premise corrected:** chain and parallel runs were already one row per flat step (SCOPE_17), not one row per run, so the `cyrup` paragraph's last sentence below is stale; the defects were rows for never-launched steps, a run-wide duration on every row, no outcome, and the placeholder row early failures wrote. A dynamic group still records one row. See the table row for deviations and the Verify tests.

**Kind** stale-port · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read; nothing run)

Filed 2026-10-03 from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0, cyrup `11664557`). 
**upstream** — `RunEntry` carries `taskHash` and `outcome` (`run-history.ts:12,15`); `recordRun(agent, task, exitCode, durationMs, terminal)` (`:223`) derives `outcome` (`stopped`, `interrupted`, `timed_out`, `stopped` on an unexplained process signal, else `completed` or `failed`), writes `task: "[redacted]"` plus `taskHash: hashTask(task)` (`:24,243`), and uses private modes (`PRIVATE_DIR_MODE = 0o700`, `PRIVATE_FILE_MODE = 0o600`, `:22-23`). `loadRunsForAgent` (`:262`) hardens the storage, sanitises lines and rotates past a threshold. The foreground executor calls it (`src/runs/foreground/subagent-executor.ts:4357,4382`). v0.75.0 adds `planBackgroundRunHistory` (`:180`): one row per launched step, a paused run recorded as `interrupted`, and a step that finished before a sibling was interrupted keeping its own outcome instead of the run-wide flags. The changelog: before, only foreground runs reached the file, so per-agent lookups missed every background launch.

**cyrup** — `record_run_history` (`background/run_history.rs:79`, core `record_run_history_at` `:86-135`) appends one `RunHistoryEntry { agent, task, ts, status, duration, exit }` per top-level result: `task` is `result.task.chars().take(200)` in plaintext, `duration` is the run-wide duration, the file is opened `create(true).append(true)` with the umask default, and there is no hash, `outcome`, hardening or rotation. Its only caller is the async runner's finish path (`background/runner_main/finish.rs:523`); `grep` finds no other writer of `run-history.jsonl` (the foreground executor's `persist_foreground_run_history_for` is a different file, `extension/executor/foreground_history/persist.rs`). Both halves are therefore behind: the entry shape and the foreground path. Its rows are per top-level result, so a chain or parallel run is one row.

**Impact** — Prompt text lands in a default-mode file in the clear, and `run-history.jsonl` lacks foreground runs. Low.

**Fix** — Port `RunEntry` with `taskHash` and `outcome`, the redacted task, `0600`/`0700` creation and rotation on load; port `planBackgroundRunHistory` so the async runner records one row per launched step with a per-step outcome; call a `recordRun` equivalent from the foreground path.

**Verify** — A row has `task: "[redacted]"`, a `taskHash`, an `outcome` and mode `0600`; a fail-fast chain has no row for the skipped sibling; a foreground single run appears in the file.


## SUBA-173 — A saved profile's `machine` string is not validated before `/subagents-load-profile` writes settings

> **CLOSED 2026-10-08** (on `claude/subagents-robustness-sweep`, checked against pi-subagents ad11b7ab). `load_profile` validates each override's `machine` with `validate_optional_machine` and upstream's label (`src/profiles/profiles.ts:148-150`) before the typed parse, so load and check refuse a bad value before any settings write; `machine: false` passes and a valid name is stored trimmed. See the `## Open items` row.

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; nothing run)

Filed 2026-10-03 from the post-pin triage (pi v1.0.1 / pi-subagents v0.75.0, cyrup `11664557`). 
**upstream** — `validateSubagentProfile(filePath, parsed)` (`src/profiles/profiles.ts:125`) runs `override.machine = validateOptionalMachine(override.machine, "Profile '<file>' has invalid machine for '<name>'")` for every override (`:148`); `validateOptionalMachine` (`src/agents/agents.ts:1035`) is exported for the purpose. The changelog: a profile with an invalid `machine` is rejected "when loaded or checked, before any settings are written"; `machine: false` still clears a pin.

**cyrup** — `load_profile` (`registration/profiles.rs:211`) is `serde_json::from_str::<NamedProfile>` and nothing else; `apply_profile_to_settings_file` (`:554`) merges the profile's `subagents` block into settings and carries a string `machine` through (`:606-622`) without checking it. The function that would check it exists: `validate_optional_machine` (`placement/resolve.rs:33`: non-empty string or `false`, at most 128 characters, no control characters). Not read end to end: the slash command's own pre-checks.

**Impact** — A hand-edited profile with a blank or oversized `machine` is accepted and written to settings, and the failure shows up later when the agent is loaded. Low.

**Fix** — Call `validate_optional_machine` per override in `load_profile` (and in any "check" path) with upstream's label, before `apply_profile_to_settings_file` writes.

**Verify** — A blank `machine` is rejected with the settings file untouched; `machine: false` still clears the pin; a valid name is applied.


## SUBA-175 — `/subagent-cost` reports 0 turns for every async child resolved from `_meta.json`

> **CLOSED 2026-10-09** (checked against pi-subagents v0.76.1). `run_artifact_metadata` (`crates/cyrup-ext-subagents/src/artifacts.rs:617-629`) inserts `turns` from `SingleResult::turns` into the `usage` object it writes to `_meta.json`, upstream's shape (`subagent-runner.ts:1238`, `:1509`; `execution.ts:154` @v0.76.1). This is the Fix's first option, so `_meta.json` stays byte-compatible with upstream's and no reader changed; a pre-fix file still resolves with 0 turns. Verify: `registration::cost::tests::an_async_childs_turns_come_from_the_metadata_the_runner_writes`, red-proved (reverting the insert fails it with `left: 0` / `right: 3`).

**Kind** port-divergence · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; nothing run)

Filed 2026-10-09 while reviewing the `SUBA-163` closure on `claude/subagents-robustness-sweep` (pi-subagents ad11b7ab). `SUBA-163` made async single, chain, parallel and workflow children resolve their usage through artifact `_meta.json`; the token and cost columns are right, but the turn count is not.

**upstream** — `metadataUsage` (`src/slash/subagent-cost.ts:124-145`) returns `usageFromValue(metadata.usage)` (`:137`), and `usageFromValue` takes `turns` from the record itself when no override is given (`:59`). Every producer writes a full `Usage`, whose interface has `turns` (`src/shared/types.ts:261-268`): the async runner folds each run's `usage.turns` into `aggregateUsage` (`src/runs/background/subagent-runner.ts:1241`) and writes it as `metadata.usage` (`:1512`); the foreground writer persists `target.usage` (`src/runs/foreground/execution.ts:154`). An async child's row in `/subagent-cost` therefore shows its real turn count.

**cyrup** — `run_artifact_metadata` (`crates/cyrup-ext-subagents/src/artifacts.rs:553`) writes `"usage": serde_json::to_value(&result.usage)` (`:577`), a `cyrup_core::Usage` (`crates/cyrup-core/src/message/usage.rs:6`), which has no `turns` field; the turn count is kept beside it on `SingleResult::turns` (`exec/run_result.rs:120`) and is not written to `_meta.json`. The async runner (`background/runner_main/executor.rs:1166`) and the foreground writer (`extension/executor/paths.rs:275`) both go through that one builder. `metadata_usage` (`registration/cost.rs:943`) calls `usage_from_value(metadata.get("usage"), None)`, so `turns` is always 0 for a child resolved at `:1292` (async workflow receipt children) or `:1346` (async single/chain/parallel steps). A foreground child is unaffected: `add_result_child` falls back to the result's own `turns`.

**Impact** — The per-child `turns` column and the child total's turn count under-report every async child as 0. Tokens and cost are correct. Low.

**Fix** — Write the turn count into `_meta.json`'s `usage` as `turns` (upstream's shape) in `run_artifact_metadata`, so `metadata_usage` reads it with no reader change; alternatively read a top-level `turns` beside `usage` as the override, mirroring `add_result_child`. The first keeps `_meta.json` byte-compatible with upstream's.

**Verify** — An async single run whose child took 3 turns reports `turns: 3` in `/subagent-cost` (extend `registration::cost::tests::an_async_single_launch_contributes_its_child_usage` to write `_meta.json` through `run_artifact_metadata` and assert the turns); a `_meta.json` written before the fix still resolves with 0 turns.

## Findings filed 2026-10-09 — pi-subagents `v0.74.0..ad11b7ab` (the pi v1.1.0 drift triage)

Upstream read through `git -C tmp/pi-subagents show` only, at `ad11b7ab` (= `v0.76.1-29-gad11b7ab`); cyrup read at `6b14575`. Nothing was run. The window record is the PIN 2026-10-09 block at the top of this file.

## SUBA-176 — The `subagent` tool schema still emits `deprecated: true`, which strict tool-schema validators reject with HTTP 400, so every request carrying the tool fails on those providers

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `50c280f1` / #2721 (related #2713): drops `deprecated: true` from the `reviewed` branch of `AcceptanceOverride` (`src/extension/schemas.ts:64-70` @ad11b7ab; `git show ad11b7ab:src/extension/schemas.ts | grep -c deprecated` = 0) and adds `deprecated` to the test's provider-rejected keyword list (`test/unit/schemas.test.ts` "does not emit provider-rejected schema shapes"). The CHANGELOG (@ad11b7ab `:33`) states the 400.

**cyrup** — `crates/cyrup-ext-subagents/src/extension/tool/schema.rs:63` (`sj_acceptance_override`: the `reviewed` branch still carries `"deprecated": true`), plus two `CYRUP-DELTA` uses upstream never had: `:488` (the `runId` alias) and `:657` (the `maxRuntimeMs` alias). No provider adapter strips the keyword (`grep -rn '"deprecated"' crates/cyrup-provider/src` has no schema hit). `exec/acceptance/lattice/lowering.rs:603` reads the flag in a test as the sanctioned-exception marker.

**Impact** — On a provider whose tool-schema validator is strict, the parent's whole request (the tool list rides every turn) is rejected with 400, so the session cannot run at all while the subagent tool is registered. Medium: a broken user-facing flow on those providers.

**Fix** — Remove `"deprecated": true` from all three schema sites; keep the `reviewed` branch and its description (upstream keeps it so preflight can explain). Re-key `lowering.rs`'s test off the branch's enum value instead of the flag. Add a schema test that walks the whole tool schema and rejects `allOf` / `const` / `if` / `then` / `not` / `deprecated`, as upstream's does.

**Verify** — `grep -rn '"deprecated"' crates/cyrup-ext-subagents/src/extension/tool/schema.rs` is empty; the keyword-walk test passes; `acceptance: "reviewed"` is still refused with the explanatory preflight sentence.

## SUBA-177 — A paused async run cannot be stopped once its paused result was delivered: the seal refuses with "paused result is missing" and the run keeps its capacity slot (extends the closed `SUBA-116`)

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `15757b00` / #2701: `sealPausedRun` (`src/runs/foreground/async-stop-action.ts:32` @ad11b7ab) falls back to `resultPayloadFileForSessionRun` and, when no payload exists at all, seals from `status.json` (`:45`: "Delivery deletes the paused result, and an interrupted parent may never have received one; seal from status"). `58c7f613` / #2722 routes the RPC stop through the same `deliverAsyncRunStop` (`:124`). Until `15757b00` upstream had the same refusal.

**cyrup** — `crates/cyrup-ext-subagents/src/background/control.rs:1134-1147` (`seal_paused_run`): `result_payload_path_for_session_run` returning `Ok(None) | Err(_)` is `return Some("paused result is missing")`. The delivered payload is unlinked by `background/watch/results_watcher.rs:648` (`consume`), so after the ordinary paused-result notification the seal can never succeed. `SUBA-116` (closed 2026-09-29) ported the v0.71.0 `sealPausedRun`, which ended at this same rung; this row extends that port. The RPC half of #2722 is already matched: RPC `stop` routes through the tool's `control_stop` (`extension/rpc/mod.rs:62-68`).

**Impact** — `stop` on a paused run whose pause notice already reached the parent (the normal case) returns an error; the run stays paused and holds its `maxActiveAsyncRunsPerSession` slot until resumed. `SUBA-116`'s symptom, back on the common path.

**Fix** — Port the fallback: when no validated payload resolves, try the unvalidated pending / public file, and when none exists build the stopped result from `RunStatus` (id, mode, sessionId, asyncDir, completionOwnerId, toolCallId, per-step agent / success / error / exitCode) and continue the seal.

**Verify** — Pause an async run, let the watcher deliver and consume its paused result, then `subagent({action:"stop", id})` succeeds: `status.json` reads `stopped`, a stopped result is published and the capacity slot is released. Change the `seal_paused_run` test near `control.rs:4600-4620`, which asserts the refusal sentence today, to expect success.

## SUBA-178 — Runner launchers (`runnerLaunchers` config, agent `launcher:` frontmatter) are unported, and an agent that names a launcher silently runs unwrapped instead of failing

**Kind** not-ported · **Severity** medium · **Effort** L · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `e4b52b4b` / #2720: the `runnerLaunchers` user-config key, validated fail-closed (`src/extension/config.ts:108-111`, `:211`; added to `FAIL_CLOSED_CONFIG_KEYS` at `:17`, now 12 keys); `resolveAgentRunnerLauncher` / `runnerLauncherPlacementError` (`src/runs/shared/runner-launcher.ts:5-27` @ad11b7ab): an undefined name fails before launch and never falls back; a launcher forces background, refuses `machine` / external runners, records `launcher:{name,argv}` in status and survives resume. `docs/agents.md` "Sandboxing background children with a launcher".

**cyrup** — `grep -rn 'runnerLaunchers\|launcher' crates --include=*.rs` has no hit. `FAIL_CLOSED_CONFIG_KEYS` is the 11-key list (`crates/cyrup-ext-subagents/src/registration/mod.rs:630-642`). An unknown frontmatter key is kept verbatim in `extra_fields` and binds nothing (`discovery/frontmatter.rs:54`, `:78`), so `launcher: net` is dropped and the runner spawns directly.

**Impact** — An agent file written to run in a dedicated sandbox runs in the parent's environment instead, with no error: a declared isolation boundary fails open. Medium only for that fail-open; the feature itself is low.

**Fix** — Minimum: type `launcher` in `KNOWN_FIELDS` and refuse a launch whose agent names one (upstream's "not defined in runnerLaunchers" sentence) until ported, and add `runnerLaunchers` to `FAIL_CLOSED_CONFIG_KEYS`. Full port: parse and validate the key from user config only, prefix the runner argv in `spawn_detached`, force background, refuse machine / external-runner combinations and mixed-launcher chains, record `launcher` in `RunStatus`, reuse the recorded launcher on resume.

**Verify** — An agent with `launcher: nope` and no config entry fails before any run dir or fan-out slot is created; with `runnerLaunchers.net = ["env","X=1","--"]` the runner's process tree shows the wrapper and `status.json` records `launcher`; a config with an invalid `runnerLaunchers` value refuses the whole file.

## SUBA-179 — `workflow: true` still fails "found 0" when the model fences its script as plain ```js, and says nothing when the reply's text never reached the session

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `7cb20ce9` / #2660: `PLAIN_JS_FENCE` (`src/extension/reply-workflow-script.ts:5` @ad11b7ab); a reply with no tagged block and exactly one plain js block runs it (`:38-41`); the found-0 error names the exact opener and the file fallback (`:8`, `:40-41`); tagged / plain collection at `:49-58`. `620cefa3` / #2670: when the issuing assistant message has no text blocks, a dedicated error says the text never reached the session (`:33`).

**cyrup** — `crates/cyrup-ext-subagents/src/extension/reply_workflow_script.rs:91` (`workflow_blocks` collects tagged blocks only) and `:146-168` (`script_from_reply`: the old "requires exactly one ```js workflow fenced block ... found {n}" sentence, no plain-block fallback); its own test at `:217` asserts a plain block is invisible. `extension/tool/workflow_field.rs:226-240` joins text blocks with no check that any exist.

**Impact** — A model that drops the `workflow` tag (common) gets a failed tool call and must retry; a reply whose text was stripped gets a misleading "found 0".

**Fix** — Port the tagged / plain split and the selection rule, the new error sentences (including the multiple-untagged hint and the file-path fallback), the "not closed" / "empty" rewording, and the no-text-block check in `script_from_branch` before joining.

**Verify** — A reply with one ```js block and `workflow: true` runs it; two untagged blocks give the "untagged js blocks" sentence; an assistant message with only a tool call gives the never-reached-the-session error; update the `:217` test.

## SUBA-180 — The child boundary instructions are prepended ahead of the base prompt, and the child runtime returns a frozen full prompt instead of filtering the prompt options

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `17e17673` / #2693: `appendChildBoundary` puts the base prompt first (`src/runs/shared/subagent-prompt-runtime.ts:224-227` @ad11b7ab), so providers that recognise Pi's opening still do. `4c63aa07` / #2747: the runtime filters `systemPromptOptions` in place (`filterChildPromptOptions`, `:236`) and a last-position hook appends the boundary (`registerSubagentPromptBoundary`, `:250`; ordering `orderChildPromptHooks`, `src/runs/shared/child-session.ts:197`), so sections later extensions add in `before_agent_start` reach the provider request. `stripChildBoundaryInstructions` (~`:199-205`) strips each instruction in both its `\n\n`-prefixed and bare form, `STRUCTURED_OUTPUT_INSTRUCTIONS` included.

**cyrup** — `crates/cyrup-ext-subagents/src/prompt_runtime.rs:1238` returns `format!("{boundary}{structured}\n\n{rewritten}")` (boundary first); `:1169-1178` strips only the two boundary texts, not `STRUCTURED_OUTPUT_INSTRUCTION` in either form; the `BeforeAgentStart` arm (`:2763-2777`) returns a whole rewritten `system` string. cyrup's dispatcher re-renders the prompt from `options` after any later handler that edits options (`crates/cyrup-ext/src/dispatch.rs:712-722`), which would discard the rewrite.

**Impact** — Providers and extensions that key on the base prompt's first sentence mis-handle every child; a later child extension that edits prompt options re-renders from unfiltered options, dropping both the stripping and the boundary.

**Fix** — Append the boundary after the rewritten prompt; strip `STRUCTURED_OUTPUT_INSTRUCTION` too, in both its prefixed and bare forms. Then split the hook as upstream does: filter `options` (context files, skills, customPrompt, appendSystemPrompt) in place in the first handler, and append the boundary from a handler registered last.

**Verify** — `rewrite_subagent_prompt("BASE", ..)` starts with `BASE`; a child with a second extension adding a prompt section in `before_agent_start` sends a request whose system prompt holds that section, the stripped context is absent and the boundary is last.

## SUBA-181 — Saving an agent whose description has a newline writes invalid frontmatter

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `49a3ec7f` / #2681: `serializeAgent` writes `description: |-` with each line indented when the description contains `\n` (`src/agents/agent-serializer.ts:67-71` @ad11b7ab). Upstream's serializer had the same single-line write until this commit.

**cyrup** — `crates/cyrup-ext-subagents/src/discovery/management/frontmatter_write.rs:59`: `lines.push(format!("description: {}", def.description))` unconditionally. The parser accepts `|` / `|-` literal blocks (`discovery/frontmatter.rs:610-613`), so a multiline description can be read in and then written back broken.

**Impact** — An update or eject of an agent with a multiline description corrupts its file (the continuation lines become stray keys or a parse failure).

**Fix** — Port the branch: `description: |-` plus two-space-indented lines when the text contains `\n`.

**Verify** — Round trip: an agent with `description: |-\n  a\n  b` saved through `serialize_agent` and re-parsed yields `"a\nb"`.

## SUBA-182 — A schedule claim whose lock write fails wedges the schedule, and a lost claim overwrites the owner's schedule record from a stale snapshot

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `cccba0cf` / #2679: when writing the run id into `active.lock` fails, the claim removes the lock only if it is still its own inode, closes it and rethrows (`src/runs/background/scheduled-runs.ts:943-958` @ad11b7ab; the bigint inode compare comes from `1edc2b20`). `d67d3173` / #2675: on EEXIST the loser re-reads the record (`store.find`), writes only the skipped run and re-arms locally without advancing the owner's cursor (`:962`: "Losing the claim gives this snapshot no authority to change the owner").

**cyrup** — `crates/cyrup-ext-subagents/src/background/scheduled_runs/store.rs:618-639` (`acquire_active_lock`): a `write_all` / `flush` failure after `create_new` returns via `?` and leaves an empty `active.lock`; `restore_one`'s stale-claim recovery runs only when `active_run_id` is set (`trigger.rs:807`, `:839`), so nothing removes it. `trigger.rs:567-591` (`skip_overlap`, reached from the claim caller at `:498`) writes the caller's `schedule` snapshot (`store.write(schedule)`) after advancing its trigger.

**Impact** — After an ENOSPC-style failure every later fire is skipped forever; two sessions racing one schedule can clobber the owner's `activeRunId` / cursor with the loser's stale copy.

**Fix** — In `acquire_active_lock`, on a post-create write failure remove the file if its (dev, ino) still matches the handle, then return the error. In `skip_overlap`, re-read the record, write only the skipped run against it, and re-arm without persisting a cursor change (once-triggers clear their timer).

**Verify** — A fault-injected write failure leaves no `active.lock` and the next tick launches; a two-store race shows the loser's skip leaves the owner's `active_run_id` and `next_run_at` unchanged.

## SUBA-183 — Schedule `history.json` is rewritten from each session's own snapshot with no lease, so concurrent sessions lose runs from the index

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `650244c3` / #2682: `writeRun` holds `withFileLease(history.json)` around the read-modify-write and prefers a running entry's own receipt (`src/runs/background/scheduled-runs.ts:420`; `getRun` `:407`; `withFileLease`, `src/shared/file-lease.ts:61`); completion, restore and delete read the run receipt through `activeRun` (`:1134`); an unlaunched claim is released when its first records cannot be saved (`:990`); completion and a failed launch release the lock and re-arm before writing history. @ad11b7ab.

**cyrup** — `crates/cyrup-ext-subagents/src/background/scheduled_runs/store.rs:526-566` (`write_run`) reads `history()`, prepends, and writes `history.json` with no lock. `restore_one` and completion matching find the active run only in `history()` (`trigger.rs:794-800`).

**Impact** — Two sessions recording runs for one project schedule can drop an entry; restore then misreads a live claim as stale (or never settles it), leaving the schedule claimed or failing a live run.

**Fix** — Put a pid-owned lease (or `cyrup_config::lock::FileLock`, as `SUBA-029` does) around `write_run`'s read-modify-write; add `get_run` reading `runs/<id>.json` and use it for active-run lookup in completion / restore / delete; reorder completion and failed launch so lock release and re-arm precede the history write.

**Verify** — Two stores appending different runs concurrently keep both in `history.json`; a restore whose history entry is missing still finds the run via its receipt.

## SUBA-184 — Watchdog settings writes and `/subagents-load-profile` read-modify-write `settings.json` with no lock, so concurrent sessions lose each other's changes

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `2d3f6471` / #2680: all seven settings writers hold `withSettingsFileLease` from read to save: agent overrides (`src/agents/agents.ts:1722`, `:1744` @ad11b7ab), profile application (`src/profiles/profiles.ts:487`), watchdog settings (`src/watchdog/settings.ts:465`, `:477`); the lease is `src/shared/settings-file-lease.ts` over `src/shared/file-lease.ts:61`.

**cyrup** — Agent overrides are already locked (`discovery/settings_write.rs:87-99`, `:270`, `:301`, `:330`, `:379`; `SUBA-029`, and `SUBA-171`, closed 2026-10-08, which hardened that writer only). **Not a duplicate of `SUBA-171`:** the other two writers are unlocked: `watchdog/settings.rs:1036-1054` (`edit_watchdog_settings`: read, edit, `write_settings_file` `:1013-1024` = plain `std::fs::write`) and `registration/profiles.rs:600-687` (`apply_profile_to_settings_file`: read, merge, `std::fs::write` at `:686`).

**Impact** — A watchdog toggle or profile load racing another session's settings save drops one change; a crash mid-`fs::write` truncates `settings.json`.

**Fix** — Route both writers through `settings_write_target` + `lock_settings_file` (the `SUBA-029` / `SUBA-171` path) and write atomically, keeping each writer's merge logic.

**Verify** — Concurrent `write_user_watchdog_enabled` and `merge_builtin_agent_override` on one file both survive; a symlinked `settings.json` is locked on its target's sidecar.

## SUBA-185 — Daily and weekly zoned calendar schedules (`every: "day"|"week"`, `at: "HH:mm"`, `on`, `timezone`) are still refused (supersedes `09-cyrup-ext-subagents.md:901`)

**Kind** not-ported · **Severity** low · **Effort** L · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `a4b1fb9d` / #2656: `src/runs/background/calendar-schedule.ts:13-80` @ad11b7ab (`normalizeCalendarRule`, `nextCalendarOccurrence`, `latestCalendarOccurrence`, `calendarDateAfter`, `restoreCalendarTrigger`); `scheduled-runs.ts:678-704` creates `kind: "calendar"` triggers, and `:448`, `:467`, `:473`, `:777-780`, `:829` advance, restore, describe and manually run them; the schema (`src/extension/schemas.ts:199-203`) takes an `on` weekday array and an IANA / UTC `timezone`; `docs/missions.md` gives the calendar rules (skipped local times; a repeated time fires once).

**cyrup** — `crates/cyrup-ext-subagents/src/background/scheduled_runs/tool.rs:492-500` refuses `on` / `timezone` and `every` in `day|week|month|year` with "Calendar schedules are deferred from this first safe slice"; `extension/tool/schema.rs:844-845` still advertises `on` as `string|integer` "Reserved calendar selector"; `schedule.rs:1240` asserts `"day"` is malformed. `09-cyrup-ext-subagents.md:901` recorded calendar triggers as "NOT a residual" because upstream also refused them then (`scheduled-runs.ts:622`); `a4b1fb9d` is what makes them a gap, so this row supersedes that note.

**Impact** — An operator cannot schedule "weekdays at 09:00 America/New_York"; a pi-written calendar schedule in a shared project store is an unknown trigger kind to cyrup.

**Fix** — Port the calendar trigger (a `Calendar` variant beside interval / once) with a tz database (`jiff` or `chrono-tz`), the schema change, create-time validation, next / latest occurrence, manual-run consumption rules and the restore-time UTC cache refresh.

**Verify** — `every:"week", on:["mon","fri"], at:"09:00", timezone:"America/New_York"` creates a schedule whose `nextRunAt` is the next Mon/Fri 09:00 local across a DST change; a nonexistent local time is skipped; `every:"day"` with `on` is refused.

## SUBA-186 — Subagent run state is not reported to the terminal with OSC 7501 (`programStatus`); blocked on `TUI-171`, which owns the root record

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `ac848775` / #2725 and `ed942575` / #2746: `registerProgramStatusReporter` (`src/integrations/program-status.ts:131-215` @ad11b7ab) writes one `subagents/<run>` (or `subagents/<workflow-run>.<key>`) record per async run: working / blocked (kind=question) / done / error / idle; title = agents or key; msg = current tool or outcome; at most 64 records; resend on SIGCONT; clear on non-quit dispose. Gated on interactive TUI, a TTY, `TERM != dumb`, `PI_PROGRAM_STATUS != 0` and the config key `programStatus` (default true, `src/extension/config.ts`); wired at `src/extension/index.ts:462-478`, `:1037`, `:1044`, `:1092`.

**cyrup** — `grep -rn 7501 crates --include=*.rs` has no hit and no `programStatus` config key exists (`registration/mod.rs` key list). pi core's own OSC 7501 root record (pi `503c60552`, `packages/tui/src/program-status.ts`) is also absent; it is filed in this triage as `TUI-171` (area 07).

**Impact** — Terminals that render the Program Status Protocol show nothing for cyrup's background runs.

**Fix** — After `TUI-171` lands the root record and its negotiation, port the reporter over the async job tracker and the supervisor pending set, with the config key and the env gates; coordinate with area 07, which owns the root record and the suspend / editor clears.

**Verify** — With a TTY and `programStatus` unset, an async run emits `ESC]7501;state=working:id=subagents/<12 chars>:app=...` then `state=done`; `CYRUP_PROGRAM_STATUS=0` and `programStatus:false` emit nothing; an unanswered supervisor ask after the parent settles reports `blocked:kind=question`.

## SUBA-187 — In headless mode, results that finished during the `agent_end` drain still wait out the completion batch window, so a print-mode parent can exit before they are delivered

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** plausible (static read; not run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `3bb9b203` / #2666: after `drainOutstandingWork`, headless `agent_end` calls `resultWatcher.deliverPendingResults` (`src/extension/index.ts:728` @ad11b7ab), which flushes the notifier's batchers and hands every on-disk result to it with no coalescing (`src/runs/background/result-watcher.ts:698`; `notify.ts` `flush()`).

**cyrup** — `crates/cyrup-ext-subagents/src/extension/host/native_impl.rs:706-731` awaits the drain and returns; nothing flushes the batcher (`background/watch/batch.rs:240`, `:249`, `:560` flush only on their own timers; default debounce 650 ms / max-wait 1500 ms, `:93-110`). `cyrup-modes/src/print.rs:109-118` ends after `wait_for_idle`, which returns while a completion is still held in the batch. cyrup's results watcher also polls every 500 ms (`RESULTS_DIR_POLL_INTERVAL`) before the debounce, so the gap is likely wider than upstream's was.

**Impact** — `cyrup -p` orchestrations whose async children finish near the end of the turn can exit without the parent seeing those results (they stay on disk). Confirmed by reading, not by running.

**Fix** — Expose a `flush_now` on the batching sink and a `deliver_pending_results` on the results watcher that rescans the results dir and delivers immediately; call it after a successful drain in the headless `AgentEnd` arm.

**Verify** — Headless test: an async child completes during the drain; the parent's queued completion turn runs before `run_print` returns (a `subagent-notify` message is in the transcript).

## SUBA-188 — Subagent notices wake an idle parent with a triggered custom message, which starts a run without `before_agent_start`, so extension-set prompt sections are missing from the woken run

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `65dff229` / #2689: `createParentWake` (`src/shared/parent-wake.ts:29-66` @ad11b7ab): to an idle parent a turn-triggering notice is appended with `triggerTurn:false` and the run is started with `sendUserMessage(PARENT_WAKE_TEXT, {deliverAs:"steer"})` (`:43-47`), because Pi starts a `sendMessage`-triggered run without `before_agent_start` (earendil-works/pi#5581, still true in pi `agent-session.ts:2314-2319`); every waking notice (completions, supervisor asks, control / steering, wait subscriptions, compaction resume) goes through it (`src/extension/index.ts:347`).

**cyrup** — Same core behaviour: `crates/cyrup-session-svc/src/session/run.rs:330-366` (`run_injection`) prompts the agent directly, while `before_agent_start` and the run's prompt options are built only in `assemble_run_inputs` (`:1135`). The subagent notices inject with `trigger_turn` (`background/watch/batch.rs`; `session/inject.rs:659-664` puts them on `plan.turn`). `00-residual-ledger.md:49-54` (UPDATE 2026-10-08) recorded exactly this drift and said it "Needs a `SUBA-` row when area 09b re-pins".

**Impact** — A parent woken by a completion runs without the prompt sections other extensions add per run (and without the watchdog's per-run additions), so its behaviour differs between user-started and completion-started turns.

**Fix** — Port parent-wake on the seam that now exists, as intercom did for `ICOM-084` (closed 2026-10-08, `11-cyrup-intercom.md:740`): `HostServices::wake_user_prompt` (`crates/cyrup-ext/src/host/services.rs:861`), whose session-svc `InjectItem::WakePrompt` runs the prompt lifecycle. Append the notice without a turn, then wake an idle parent with a short steer-delivered prompt and upstream's 10 s pending reservation. No core change is needed.

**Verify** — With a test extension adding a prompt section in `before_agent_start`, an idle parent woken by an async completion sends a provider request whose system prompt contains that section.

## SUBA-189 — A parent that ends its turn without acting on a completion wake or a supervisor ask gets no reminder: the bounded `agent_before_settle` continuation is unported

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `3d4764de` / #2716: completion wakes carry `Parent action: …` (`src/runs/background/notify.ts:621`, `COMPLETION_ACTION` at `:615` @ad11b7ab); `handleBeforeSettle` (`:925-926`, registered through `pi.on("agent_before_settle")` at `:975`) finds a wake the assistant never answered with text or a tool call, continues once with `subagent-completion-unanswered` (`:957`), then ends with `subagent-completion-unhandled` (`:968`). The supervisor channel does the same per pending request (`src/intercom/native-supervisor-channel.ts:854` registration, the unanswered notice at `:827`, BLOCKED at `:845`). State survives reload per session UUID.

**cyrup** — No subagents handler subscribes `agent_before_settle` (`grep -rn 'BeforeSettle' crates/cyrup-ext-subagents/src` is empty), though the host dispatches the boundary (`crates/cyrup-ext/src/dispatch.rs:62-75`, `EXT-078`). `grep -rn 'Parent action' crates` is empty.

**Impact** — A model that silently yields on a completion or a supervisor ask leaves the work unacknowledged and the child blocked, with no visible marker.

**Fix** — Append the `Parent action` line to triggered completion content; register a `before_settle` handler in the notifier and in the native supervisor channel with upstream's one-reminder-then-warn budget and its acted-on test (a later assistant message with text or a tool call).

**Verify** — A scripted parent that replies empty to a completion wake gets exactly one continuation with the unanswered notice, then an UNHANDLED entry, and settles; a parent that calls a tool after the wake gets neither.

## SUBA-190 — Subagent messages in the main chat still use per-type cards and glyph lines instead of Pi's collapsible `[subagent]` block

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `699429b4` / #2753: one registry renders every pi-subagents message and entry type as a single `[subagent]` headline (kind-coloured label, status words in status colours, `ctrl+o to expand` hint) that expands to the model-visible text as markdown (`src/tui/subagent-messages.ts` @ad11b7ab, 311 lines). The per-type card renderers are removed from `src/intercom/supervisor-ui.ts` (−176 lines), `src/watchdog/render.ts` (reworked) and `src/watchdog/register-main.ts` (−20); the files still exist. Collapsed watchdog lines keep the stalemate / stale / failed-review labels.

**cyrup** — Per-type renderers remain: the supervisor request / reply renderers registered at `crates/cyrup-ext-subagents/src/extension/host/native_impl.rs:235-240`, `:339` (`tui/supervisor_ui.rs`), and the watchdog warning renderer at `:1199` (`watchdog/register_main.rs:1054`).

**Impact** — Visual drift only: cyrup's chat looks like pre-#2753 pi.

**Fix** — Add one message-renderer registry mirroring `subagent-messages.ts` (headline plus markdown body per custom type, collapsed by default, expand on click or the expand key) and register it for every subagent custom type, retiring the per-type renderers.

**Verify** — Snapshot tests: a completion, a supervisor ask, a reply and a watchdog blocker each render as one collapsed `[subagent]` line and the model text when expanded.

## SUBA-191 — Subagent model displays strip the provider, so the same model id from two providers is indistinguishable

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `f318c82e` / #2734: `formatModelThinking` no longer cuts at the last `/` (`src/shared/formatters.ts:35` @ad11b7ab); observed models are qualified with the serving provider via `qualifyModelWithProvider` (`src/shared/model-info.ts:90`) in the foreground and background child loops; cards prefer the observed `progress.model`; Fleet transcripts show `provider/model`.

**cyrup** — `crates/cyrup-ext-subagents/src/formatters.rs:31-36` (`format_model_thinking`) keeps only the text after the last `/`.

**Impact** — Operators running two logins or providers of one model cannot tell which served a child.

**Fix** — Drop the slash-stripping, add a `qualify_model_with_provider` over the model registry, apply it where the child's assistant `message_end` model is recorded, and prefer the observed model in compact rows.

**Verify** — A child served by `opencode-go/deepseek-v4.1-flash` renders that full id in the card and in Fleet; an id already qualified for its provider is unchanged.

## SUBA-192 — A blocking `bg_wait` ignores a steer or follow-up the operator types, holding the message until the wait window ends

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `ecc9bc41` / #2736: the wait tool listens to `input` events (non-extension, with `streamingBehavior`) and aborts open waits (`src/runs/background/wait-tool.ts:31-52` @ad11b7ab); `waitForSubagents` then yields a non-error `user_input` result listing still-active work (`src/runs/background/subagent-wait.ts:411-422`, `:595`, `:794`).

**cyrup** — `crates/cyrup-ext-subagents/src/extension/wait_tool.rs` and `background/wait.rs` have no input signal (no `steer` / `user_input` reference; only `window_elapsed` yields, `wait.rs:1043`). The host publishes `Input` with `streaming_behavior` (`crates/cyrup-ext/src/event.rs:580-587`).

**Impact** — An operator correction typed during a long wait reaches the model only after the window (default 30 min) or a run change.

**Fix** — Subscribe the wait tool to `Input` (skip extension-sourced and non-streaming inputs), cancel a per-call token, and return the `user_input` yield with upstream's text and active ids.

**Verify** — A blocking wait on a long run returns within one poll of a steer typed by the user, with `details.wait.reason == "user_input"` and not an error; an extension-sent input does not end it.

## SUBA-193 — Async runs cannot be found by the tool-call id that launched them: no async status records `toolCallId` and the run-id resolver has no tool-call lookup

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `6a6675c9` / #2717: direct async singles carry `toolCallId` into status, result and launch details (`src/runs/background/async-execution.ts:265`, `:2196` @ad11b7ab); terminal indexing keeps an `@tool-calls` alias (`src/runs/background/terminal-run-index.ts:14`, `:127`); the resolver merges live and terminal aliases and reports a shared alias as ambiguous (`src/runs/background/run-id-resolver.ts:155-159`). Documented as RPC lost-reply recovery via `id: "rpc-spawn-<requestId>"` (`docs/extension-api.md` "Direct async launch correlation"). The live-index tool-call lookup predates the window (`run-id-resolver.ts:105-109`).

**cyrup** — `RunStatus.tool_call_id` exists (`crates/cyrup-ext-subagents/src/background/records.rs:466-472`) but no launch path sets it: the only production writer is the `active_run_index.rs:478` helper path, and `runner_main/finish.rs:569` hard-codes `None` for the result. `read_active_run_tool_call_index` (`background/active_run_index.rs:437`) has no production caller; `run_id_resolver.rs:263` resolves by run-id prefix only. cyrup's RPC bridge already mints the call id as `rpc-<method>-<requestId>` (`extension/rpc/mod.rs:577`), so `rpc-spawn-R` is the right alias once stamped.

**Impact** — An RPC client whose spawn reply was lost cannot recover the run id from its own request id, and must not redispatch blind.

**Fix** — Stamp the executor tool-call id into `RunStatus` and the result write's `tool_call_id` for async launches, add the terminal `@tool-calls` alias, and resolve exact tool-call ids (live and terminal) before prefix matching, with upstream's ambiguity error.

**Verify** — An RPC spawn with requestId R whose reply is dropped: `status` with `id: "rpc-spawn-R"` returns the run with `toolCallId` in details, before and after its result is delivered; two runs sharing the alias give the ambiguity error.

## SUBA-194 — The abandoned-slot release reads a runner PID from another PID namespace as dead, so capacity can be reclaimed while the runner is alive (the residual `SUBA-159` recorded)

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `8fb89814` / #2663 (second commit): `abandonedRunnerReleaseVerdict` retains the slot when `status.pidNamespaceScope` differs from the current namespace (`src/runs/background/active-async-capacity.ts:255` @ad11b7ab). The workflow-child half (`abandonedWorkflowReleaseVerdict`, `:291`) is for async workflow children, which cyrup does not have.

**cyrup** — `crates/cyrup-ext-subagents/src/background/active_async_capacity/inspect.rs:386-440` (`abandoned_runner_release_verdict`) and the workflow fallback `child_pid_is_gone` (`:643-647`) call `options.pid_liveness(pid)` with no namespace check, although `RunStatus.pid_namespace_scope` is now stamped (`background/records.rs:391`) and used only by `reconcile.rs:452`. The closed `SUBA-159` (09b:264) recorded this exact gap under "Not in this row (recorded, not claimed)" (the capacity release still probes with bare liveness, `active_async_capacity/config.rs:151`), together with a second unchecked reader, upstream `runnerExitedWithoutResult` (`await-async-run.ts:18-26`).

**Impact** — A failed run whose runner lives in another PID namespace (container, sandbox) can have its slot released after the threshold while still running, over-admitting async work.

**Fix** — Before the liveness probe, retain with upstream's reason when `status.pid_namespace_scope` is set and differs from `reconcile::current_pid_namespace_scope()`; apply the same guard in `child_pid_is_gone`. Check cyrup's counterpart of `runnerExitedWithoutResult` and either guard it here or record why it is excluded.

**Verify** — A failed status with a foreign `pid_namespace_scope` and a dead-looking pid past the threshold is `retained` with the namespace reason.

## SUBA-195 — Text still streaming when a child times out or errors is lost: the partial-output tracker is unported

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `9bc8f2d1` / #2653: `createPartialOutputTracker` keeps the latest unfinished assistant message from `message_update` (`src/runs/shared/partial-output.ts` @ad11b7ab); on timeout or a thrown child error it becomes the output (`Partial output before child error:` for errors), with `outputPartial: true`, never satisfying acceptance or an output file (`src/runs/foreground/execution.ts:566`, `:1547`; runner `src/runs/background/run-child-session.ts`).

**cyrup** — `crates/cyrup-ext-subagents/src/exec/ndjson.rs:114` parses `MessageUpdate` but nothing consumes it (`grep -rn 'SubagentEvent::MessageUpdate' src` hits only `ndjson.rs`); `exec/mod.rs:1566-1572` builds "Partial output before timeout" only from completed output; no `output_partial` field exists. cyrup's child json mode emits `message_update` as a delta-only projection, `{type, assistantMessageEvent}`, with no cumulative `message` snapshot (`exec/ndjson.rs:99-116`).

**Impact** — A child that times out mid-answer returns only the timeout sentence, discarding the visible partial answer.

**Fix** — Track the streaming text per attempt by accumulating `text_delta` events per content index and resetting on `message_start` / `message_end` (cyrup cannot read a partial message snapshot the way upstream does); clear on a completed reply, keep on an errored one; use it as the partial body on timeout or child error; add `output_partial` to `SingleResult` and keep it out of acceptance and output-file saves.

**Verify** — A child killed by `timeoutMs` while streaming "half an answer" returns that text under the timeout preamble with `outputPartial: true`; a tool-only completed reply before the timeout clears it.

## SUBA-196 — A child ended by a per-tool timeout is reported as "Subagent timed out after {run budget}ms" with the real cause shown as partial output

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `ba008223` / #2694: `timeoutCause` records whichever timer fired (`src/runs/foreground/execution.ts:1215`, `:1267` @ad11b7ab); the terminal preamble uses it (`:1557`), so a tool timeout leads with the tool's own message; the run timer and the tool timer cannot both fire. Upstream emitted the same "timed out after <run timeout>ms" preamble until this commit.

**cyrup** — `crates/cyrup-ext-subagents/src/exec/attempt_runner.rs:1170-1193` sets `timed_out: true` and `final_output = tool_timeout_error`; `exec/mod.rs:1556-1572` (`apply_terminal_preamble`) then prepends `format_timeout_message(opts.timeout_ms.unwrap_or(0))` and puts the tool message under "Partial output before timeout:".

**Impact** — Operators read a run-deadline timeout (often "after 0ms" when no run timeout was set) for what was a single tool exceeding its limit.

**Fix** — Carry the timeout cause (tool vs run) from the attempt and lead the preamble with it; do not treat the tool message as partial output.

**Verify** — A child whose `bash` exceeds its tool timeout with no `timeoutMs` reports the tool-timeout sentence first and no "timed out after 0ms".

## SUBA-197 — With `outputSchema`, a bound output file receives the child's closing prose instead of the structured result

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `8983754b` / #2673 (#2672): when a structured value exists the output file is written from `structuredText ?? fullOutput` (`src/runs/foreground/execution.ts:1582` @ad11b7ab; runner `src/runs/background/subagent-runner.ts:1389`). `docs/tool-reference.md`: "the runtime persists the structured result as indented JSON instead of the final prose". After saving, upstream sets `fullOutput = stripAcceptanceReport(resolvedOutput.fullOutput)` (`:1582-1583`), so the reply text may change too.

**cyrup** — `crates/cyrup-ext-subagents/src/exec/mod.rs:745-768`: the structured value replaces `final_output` only when the prose is blank (`SUBA-126`), and `resolve_saved_output` saves `final_output`.

**Impact** — A workflow step that binds `output` and `outputSchema` gets prose in the file where later steps expect the JSON contract.

**Fix** — Pass `serde_json::to_string_pretty(value)` to the output-file save whenever a structured value exists. For the reply text, follow whatever upstream's post-save assignment produces rather than assuming it is unchanged.

**Verify** — A child that calls `structured_output({a:1})` then says "Done." with `output: "r.json"` writes `{\n  "a": 1\n}` to `r.json`. Pin the reply text to what upstream's tests assert (`test/integration/async-execution.part-2` / `part-3` in `8983754b`), not to an unchanged "Done." by assumption.

## SUBA-198 — The main watchdog never records mid-run user input in scope, so a steer typed while the agent streams is later flagged as scope drift

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `ff2e00bd` / #2661: `pi.on("input", (event) => runtime.handleUserInput(event))` (`src/watchdog/register-main.ts:406` @ad11b7ab); `handleUserInput` adds non-extension input with `streamingBehavior` to scope (`src/watchdog/runtime.ts:513`); changed paths are labelled as every dirty path with unverified authorship (`:855`), and `watchdog_diff`'s description says it includes other sessions' edits (`src/watchdog/diff-tool.ts:80-81`).

**cyrup** — No `Input` handler in the watchdog (`grep -rn 'HostEvent::Input' crates/cyrup-ext-subagents/src` is empty); scope prompts are added only at agent start (`watchdog/runtime.rs:867`). The review input header is the bare "Changed repo paths:" (`watchdog/runtime.rs:1939`).

**Impact** — False scope-drift warnings for work the user asked for mid-run; reviews treat other sessions' dirty files as this session's.

**Fix** — Handle `Input` in the main watchdog: skip extension input, reset clarification as upstream does, and `scope.add_prompt(text)` when `streaming_behavior` is set; change the changed-paths header and the diff tool description to upstream's wording.

**Verify** — A steer typed during a run appears in the next review's scope block; the review input carries the authorship caveat.

## SUBA-199 — `worktree.cleanup` is still plan-only: reviewed plans cannot be applied

**Kind** not-ported · **Severity** low · **Effort** L · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `3ae772c9` / #2655: `mode: "apply"` with an explicit `repo` and a saved `planId` (`src/runs/shared/worktree-cleanup-apply.ts:30`, `:63`, `:157` @ad11b7ab; dispatch `src/runs/foreground/subagent-executor.ts:6701`), under `authorityPolicy.discardWorktree`, with a plan hash and 30-minute expiry, a repository lock shared with retained-resume admission (`src/runs/shared/worktree-lock.ts`), the recorded creation base, and a one-shot claim with a durable receipt.

**cyrup** — `crates/cyrup-ext-subagents/src/extension/tool/lane_actions.rs:133`, `:137` refuse `mode='apply'` and `planId` ("apply/removal is not available yet"); `spawn/cleanup_plan/mod.rs:14-24` documents the plan-only state; the schema (`extension/tool/schema.rs:548-555`) says "Reserved; cleanup is plan-only".

**Impact** — Stale managed worktrees must be removed by hand.

**Fix** — Port the apply path behind the existing authority gate: plan persistence with hash and expiry, the shared repo lock with resume admission, a per-candidate recheck, creation-base proof, claim plus receipt, and non-forced `git worktree remove` only.

**Verify** — Plan then apply on a repo with one clean terminal managed worktree removes it and writes a receipt; a dirty, locked or resumed tree is kept; re-applying the plan shows the receipt and removes nothing.

## SUBA-200 — The herdr bridge marks the pane `blocked` when a child needs attention, though that attention is for the parent agent, not the user

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `1faa0c08` / #2703: the bridge never raises `herdr:blocked` for child attention and only emits `herdr:busy` while async work remains (`src/integrations/herdr-status.ts` @ad11b7ab; `docs/extension-api.md` Herdr section).

**cyrup** — `crates/cyrup-ext-subagents/src/herdr/state.rs:317-321` (`StateModel::desired`): any `attention` entry returns `PaneAgentState::Blocked` with the attention message, ranked above `Working`.

**Impact** — herdr's sidebar tells the human a pane needs them when only the parent model does.

**Fix** — Keep `Blocked` only for `human_waiting`; report attention through the label / metadata (the `⚠` suffix) with state `Working` while work is active.

**Verify** — Update the `state.rs` tests at `:553`, `:574`, `:642`: an attention entry with runs active yields `Working` plus the attention label; a permission dialog still yields `Blocked`.

## SUBA-201 — Inspector open and close are not serialized per run, so concurrent opens can create two panes and one overwrites the other's binding

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `5096985c` / #2728: the dispatcher holds a file lease in the run directory around open and close per target (`src/inspectors/actions.ts` @ad11b7ab, `tryLease` from `src/shared/file-lease.ts`), across processes; status and command stay unlocked. A held lease is waited for, not refused: the dispatcher polls `tryLease` every 50 ms for up to `INSPECTOR_LEASE_WAIT_MS = 30 s` (`actions.ts:22`, `:149-155`), and only an abort ends the wait, with "was cancelled while waiting for another inspector open or close to finish".

**cyrup** — `crates/cyrup-ext-subagents/src/inspectors/actions.rs:622` (`plugin.open`) and `:645` (`owner.close`) run with no lock; the herdr plugin writes a binding per target.

**Impact** — Double-clicking inspect, or two sessions inspecting one run, can leave an orphaned herdr pane and a binding that close cannot reach.

**Fix** — Take a per-target, pid-owned lock file in the run directory (dead-owner reclaim) around open and close, and wait for it as upstream does (50 ms polls, 30 s cap), ending early only on abort with upstream's cancellation sentence.

**Verify** — Two concurrent `inspector.open` calls on one target open one pane; a racing close finds the binding the open wrote; an aborted waiter returns the cancellation sentence.

## SUBA-202 — The bundled tmux inspector plugin is unported

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `cceb676c` / #2719: `createTmuxInspectorPlugin` (`src/inspectors/tmux/plugin.ts` @ad11b7ab; actions `src/inspectors/tmux/actions.ts`, client `src/inspectors/tmux/client.ts`): available when `TMUX` is set and not on Windows; splits the focused window and records a binding so status and close work. Dispatcher order Herdr → Ghostty → tmux → external, and `tmux` joins the reserved names (`docs/extension-api.md`).

**cyrup** — `crates/cyrup-ext-subagents/src/inspectors/plugins.rs:67-74` registers herdr and ghostty only; `grep -rn tmux src/inspectors` is empty.

**Impact** — Inside tmux, Fleet's inspect / `H` has no pane backend.

**Fix** — Port the tmux plugin (split-window, pane-id binding, status via `display-message`, close via `kill-pane`), add it third in the built-in list and to the reserved external names.

**Verify** — With `TMUX` set and a fake tmux client: open writes a binding and splits; status reports the pane; close kills it; an external plugin named `tmux` is rejected.

## SUBA-203 — Workflow `emit()` rejects objects with undefined fields, failing the whole workflow where `return` accepts the same value

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi-subagents `v0.74.0..ad11b7ab`, cyrup `6b14575`).

**upstream** — `1325e95a` / #2755: `emit` normalizes with `omitUndefinedWorkflowValues` (and `undefined` → `null`) before `assertJsonValue` (`src/workflows/scripted-workflow.ts` sandbox `emit`, @ad11b7ab); the offline validator stops rejecting undefined emit values; syntax-error lines are reported relative to the user's script. Upstream's `emit` had the same un-normalized assertion until this commit.

**cyrup** — `crates/cyrup-workflow-runtime/src/js/prelude.js:559-562`: `assertJsonValue(emittedValue)` runs on the raw value, so `emit({a: undefined})` throws "emit.a must contain only JSON values"; the return path already normalizes (`:590`).

**Impact** — Emitting a child result with an absent optional field (for example `outputPathMapping`) kills the workflow.

**Fix** — In `emit`, map `undefined` to `null` and apply `omitUndefinedWorkflowValues` before the assertion; mirror the analyzer relaxation; subtract the wrapper line from reported syntax-error positions if cyrup wraps the script.

**Verify** — A script calling `emit({a: 1, b: undefined})` emits `{a:1}` and completes; `emit(undefined)` emits `null`.

## Findings filed 2026-10-09 — pi-subagents `v0.76.1` follow-ups (`SUBA-204`…`SUBA-209`)

Upstream read through `git -C tmp/pi-subagents show v0.76.1:<path>` only; cyrup read at HEAD (`c487d16`). Nothing was run. Each item was checked against `09`, `09a` and `09b` first and has no earlier row.

## SUBA-204 — A generic external-CLI agent silently ignores a per-call `model` or `thinking`; upstream refuses the launch

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

**upstream** — Both async launch paths build a `does not support:` list for an external runner (`external-cli` or `external-job`). The single path pushes `"model override"` when `params.modelOverride` is set and `"thinking override"` when `params.thinkingOverride` is set, each unless the runner is a Claude Code adapter (`src/runs/background/async-execution.ts:1788`, `:1790` @v0.76.1), and fails with `Agent '<name>' uses runner.type='<type>' and does not support: model override, thinking override.` (`:1798`). The chain path does the same for a step's `model` (`:1025`, refusal `:1031`). The refusal dates from `fa6f51f0` / #738 (v0.41.0); the Claude Code exemption is v0.75.0's. External runners are async-only upstream (`src/runs/foreground/subagent-executor.ts:7490`), so these two paths are every launch.

**cyrup** — The only launch-time refusal for a foreign runner is fast mode: `exec/external_cli/mod.rs:324-329` (all paths) and `refuse_external_runner_fast` (`extension/executor/background.rs:1185`, called at `:547`). Its doc comment says so: "this check owns only `fast`". `resolve_claude_code_launch_override` (`exec/external_cli/mod.rs:208`) returns an empty argv for every non-Claude-Code runner (`:212-218`), so a generic runner never reads `RunOptions::launch_model`. Neither does it read the `thinking` the tool schema offers (`extension/tool/schema.rs:483`). Frontmatter `model`/`thinking` on a generic profile is already refused at load (`PI_ONLY_FIELDS`, `runner/mod.rs:204-221`); only the per-call parameter slips through.

**Impact** — `subagent({agent: "my-cli-agent", model: "x"})` runs the CLI's own default model and reports success, so the caller believes the override applied. Low.

**Fix** — In the shared external-runner check, refuse a per-call model (`launch_model`) and thinking level for any runner that is not a Claude Code adapter, with upstream's list wording (`model override`, `thinking override`), and fold fast mode into the same list. The other upstream list entries (structured output, acceptance, tool budget, fork context, skills) were not compared.

**Verify** — A generic `external-cli` agent launched with `model` is refused before anything spawns with `does not support: model override`; with `thinking`, `thinking override`; a `claude-code` agent with both still launches with `--model`/`--effort`.

## SUBA-205 — The Claude Code version probe refuses the calendar-style version Claude Code now prints

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

**upstream** — `02cb5b6a` / #1828 ("fix: accept calendar-style Claude Code versions", first tag v0.65.0) widens the preflight `validate` of the Claude Code adapter to `/^(?:\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)? \(Claude Code\)|\d{4}\.\d{1,2}\.\d{1,2} [A-Za-z0-9._-]+ \(\d{4}-\d{2}-\d{2}\))$/` (`src/runs/shared/claude-code-adapter.ts:280` @v0.76.1). The comment above it (`:274-279`) says Claude Code publishes both semver (`2.1.259 (Claude Code)`) and calendar/platform (`2026.4.24 macos-arm64 (2026-04-27)`) versions, and that the launch flags are checked from `--help` instead. The CHANGELOG: "Accept calendar/platform `claude --version` output during Claude Code adapter preflight while retaining required launch-flag validation."

**cyrup** — `validate_version` (`exec/external_cli/adapters/claude_code.rs:388-421`, wired at `exec/external_cli/mod.rs:175`) hand-rolls only the first alternative: it requires the ` (Claude Code)` suffix (`:396-398`) and a three-part numeric core. Its doc comment still cites the pre-#1828 pattern (`:380`). `2026.4.24 macos-arm64 (2026-04-27)` has no such suffix and is refused.

**Impact** — On a machine whose `claude --version` prints the calendar form, every `claude-code` / `claude-code-writer` agent fails preflight with `Unsupported Claude Code version response: "…"`. Low.

**Fix** — Accept the second alternative too: four-digit year, one- or two-digit month and day, one `[A-Za-z0-9._-]+` platform token, then a parenthesised `YYYY-MM-DD`. Keep it hand-rolled (the crate has no regex dependency) and update the doc cite.

**Verify** — `validate_version("2026.4.24 macos-arm64 (2026-04-27)")` is `Ok`; `2026.4.24 (2026-04-27)` (no platform) and `2026.4.24 macos-arm64` (no date) are refused; the existing semver cases are unchanged.

## SUBA-206 — `_meta.json` and `_input.md` store the child's task in plaintext; upstream writes `[prompt redacted]`

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

**upstream** — `PROMPT_REDACTED = "[prompt redacted]"` (`src/shared/utils.ts:13` @v0.76.1). Every artifact metadata writer stores `task: PROMPT_REDACTED`: foreground `writeRunMetadata` (`src/runs/foreground/execution.ts:151`, also `:238`, `:248`), the async runner (`src/runs/background/subagent-runner.ts:1504`, and the external paths `:974`, `:1045`). The input artifact holds `# Task for <agent>\n\n[prompt redacted]; live Prompt Audit only.` (`execution.ts:1812`, `subagent-runner.ts:850`). Upstream has redacted since `d3d1ac4c` ("feat: add live prompt audit drawer", v0.48.0); the prompt is kept only in memory for the live Prompt Audit.

**cyrup** — `run_artifact_metadata` writes `"task": result.task` (`artifacts.rs:575`), and both input writers write the full task: `background/runner_main/executor.rs:1189` and `extension/executor/foreground.rs:2109` (`# Task for <agent>\n\n<task>`). The constant exists in cyrup for another purpose (`exec/child_session_name.rs:28`). Nothing in the crate reads `task` back from `_meta.json`. `run-history.jsonl` already redacts (`SUBA-172`).

**Impact** — Every task the parent delegates, which can include pasted secrets, lands in the artifacts dir in the clear and survives the run. Low.

**Fix** — Write `PROMPT_REDACTED` as `_meta.json`'s `task`, and `[prompt redacted]; live Prompt Audit only.` as the `_input.md` body, as upstream does. The Prompt Audit drawer itself stays cut (`09a`, "Prompt Audit drawer" in the ratio cut list).

**Verify** — After a run with artifacts on, `_meta.json` has `task: "[prompt redacted]"`, `_input.md` does not contain the task text, and `/subagent-cost` and the artifact readers are unaffected.

## SUBA-207 — An external-runner profile may declare `allowedAgents`; upstream refuses it as a Pi-only field

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

**upstream** — `validateExternalRunnerProfile` (`src/agents/agents.ts:2070-2080` @v0.76.1) refuses `tools`, `excludeTools`, `allowNestedSubagents`, `allowedAgents`, `model`, `thinking`, `extensions`, `subagentOnlyExtensions`, `mutationTools`, `maxSubagentDepth`, `skills`, `skill`, `skillPath`, `toolBudget`, `permission`, `permissions` (`:2075`), with `Agent '<name>' uses runner.type='<type>' and declares unsupported Pi-only fields: …`. `allowedAgents` joined the list in v0.70.0 with the feature (`7ba72ee4`, "add descendant agent allowlists"); v0.68.0's list did not have it.

**cyrup** — `PI_ONLY_FIELDS` (`runner/mod.rs:204-221`) has the other fifteen and no `allowedAgents`; `validate_external_runner_profile` (`:236-258`) filters only that array. Since `SUBA-111` (closed 2026-09-29), `allowedAgents` is parsed from frontmatter (`discovery/frontmatter.rs:1009`), so an external profile that declares it loads, although the allowlist cannot constrain what a foreign CLI does. The array's doc comment is stale too: it says "seventeen" and cites `agents.ts:1906` @v0.64.0 (`:194-195`). The list also still has `fallbackModels`, which upstream dropped when it removed the field at v0.68.0. That is `SUBA-109`'s, not this row's.

**Impact** — An author who adds `allowedAgents` to an external-runner agent believes delegation is restricted, but nothing is enforced and no error says so. Low.

**Fix** — Add `"allowedAgents"` after `"allowNestedSubagents"` (upstream's order reaches the user in the refusal), and refresh the doc comment's count and cite.

**Verify** — An `external-cli` profile with `allowedAgents: reviewer` is refused at load with `declares unsupported Pi-only fields: allowedAgents`; a native agent with the same key still loads.

## SUBA-208 — `subagent`, `subagent_supervisor`, `contact_supervisor` and `structured_output` are `direct` tools, so codemode scripts can call them

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

**upstream** — `09585988` / #2586 (v0.74.0) adds `MODEL_ONLY_TOOL = { exposure: "model-only" }` (`src/shared/extension-context.ts:9` @v0.76.1) and spreads it into `subagent` (`src/extension/index.ts:745`, and the child-safe fanout variant `src/extension/fanout-child.ts:222`), `subagents_enable` (`src/extension/tool-activation.ts:100`), `contact_supervisor` (`src/intercom/native-supervisor-channel.ts:264`, `src/extension/herdr-pi-bridge.ts:79`), `subagent_supervisor` (`native-supervisor-channel.ts:591`) and `structured_output` (`src/runs/shared/subagent-prompt-runtime.ts:427`). The CHANGELOG: with `codemode` active, scripts could call these as nested tools, where a nested `subagent` shows no progress card and is cancelled if not awaited, `contact_supervisor` blocks the script, and `structured_output` neither ends the step nor reaches the transcript; codemode also copied the full `subagent` schema into its description (about 2,200 tokens per request).

**cyrup** — `Tool::exposure` defaults to `ToolExposure::Direct` (`crates/cyrup-core/src/tool.rs:410`). In this crate only `SubagentsEnableTool` overrides it (`extension/tool_activation.rs:374`, citing #2586). `SubagentTool` (`extension/tool/mod.rs:177`), `SubagentSupervisorTool` (`native_supervisor.rs:1358`), `NativeContactSupervisorTool` (`:1568`) and `StructuredOutputTool` (`prompt_runtime.rs:1558`) do not, so they are `direct`. The host honours the setting (`crates/cyrup-ext/src/wrapper.rs:191` forwards it; `crates/cyrup-ext/src/tests/tool_exposure.rs` pins codemode against model-only), so the fix needs no host work.

**Impact** — With codemode on, a script can launch an untracked subagent, block on a supervisor ask, or call `structured_output` without ending the step; and every request carries the `subagent` schema twice. Low.

**Fix** — Override `exposure()` to `ToolExposure::ModelOnly` on the four tools, as `SubagentsEnableTool` does. `NativeChildIntercomTool` (`native_supervisor.rs:1707`) has no counterpart in #2586 and is out of scope.

**Verify** — A session with codemode active lists none of the four in the codemode tool declarations and still offers them to the model directly; a unit test asserts each tool's `exposure()`.

## SUBA-209 — The advertised-agents catalog rewrites the whole system prompt each turn instead of riding its own `advertised_subagents` prompt section

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

**upstream** — `e583ea6a` / #2519 (v0.73.1; fixes #2518). The `before_agent_start` handler (`src/extension/index.ts:793-802` @v0.76.1) sets `event.systemPromptOptions.sections.advertised_subagents = catalog` on every turn `subagent` is selected, with the comment "Structured sections let Pi append a transcript delta instead of replacing the cached system prompt … an unset turn records a removal". `buildAdvertisedAgentCatalog` (`src/agents/advertised-agent-prompt.ts:38`) returns the body without the `<advertised_subagents>` wrapper, because the section key supplies the tag (`:34-36`). The CHANGELOG: "Turning on `subagent` no longer throws away the prompt cache … Before, pi-subagents rewrote the whole system prompt, so the first message after `subagents_enable` resent the entire conversation to the cache."

**cyrup** — The `BeforeAgentStart` arm (`extension/host/native_impl.rs:890-931`) calls `discovery::advertised::append_advertised_agent_prompt` (`:920`; `discovery/advertised.rs:157`) on the system prompt string and returns it as `EventPatch::SystemPromptAndInject { system, .. }` (`:926`). The host records a returned `system` as `forceSystemPrompt` (`crates/cyrup-ext/src/contract.rs:216-219`), which replaces the base prompt for the run. This is upstream's pre-#2519 shape. The port is now possible: `EXT-084` (closed 2026-10-09, `06-cyrup-ext.md`) gave `before_agent_start` a typed `SystemPromptOptions` with named custom sections, and the same patch already carries `options` (`loader_options`, `:929`). Not checked: whether cyrup's section path emits the transcript delta that makes the change cache-safe on each provider. Related: `SUBA-180` (the child prompt's handler also returns a whole string), `SUBA-188` (woken runs miss extension sections), and `SUBA-133` (closed; the catalog itself).

**Impact** — Each turn the catalog changes the forced system prompt, and the first request after `subagents_enable` turns `subagent` on resends the conversation uncached. Cost, not correctness. Low.

**Fix** — Write the unwrapped catalog into `options.sections.advertised_subagents` on every turn `subagent` is selected and leave it unset otherwise, merging into the loader's edited options. Stop returning `system`. Split `append_advertised_agent_prompt` into a body builder, keeping the byte budget on the wrapped form as upstream does.

**Verify** — With an advertised agent and `subagent` selected, the `before_agent_start` patch carries `sections.advertised_subagents` and no `system`; the provider request's system prompt still contains the `<advertised_subagents>` block; the turn after `subagents_enable` does not force-replace the base prompt.

## SUBA-210 — The async-jobs widget cannot be folded by clicking its header

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read) · **Filed** 2026-10-10 (split from `SUBA-162`)

**upstream** — `buildWidgetComponent` (`src/tui/render.ts:2911` @ad11b7ab) mounts a component whose `handleMouse` (`:2938-2944`) answers a left `click` at `event.y === 0` with no shift, alt or ctrl by flipping `collapsed` and calling `invalidate()`, which also resets the layout session (`:2920-2924`). `collapsed` starts from `asyncWidgetCollapsed` (`:2919`); the folded state lasts until the widget unmounts.

**cyrup** — `asyncWidgetCollapsed` is ported (`SUBA-162`): `AsyncWidgetRender::collapsed` in `tui/events.rs`, set from config in `extension/host/slash.rs`. Nothing can toggle it. `HostServices::set_widget(key, Option<&[String]>, placement)` (`cyrup-ext/src/host/services.rs`) carries lines, not a component, and `cyrup-tui/src/app/pointer.rs`'s module doc states that a press on an extension widget is not claimed and falls through to text selection. Overlays do get mouse events (`cyrup-ext/src/host/overlay.rs` `handle_mouse`); widgets do not.

**Impact** — Low: a user cannot fold the card by clicking; the config key still sets the folded state.

**Fix** — Needs a widget pointer route in `cyrup-tui` and a host-to-extension callback (a `HostServices`/WIT addition carrying the widget key and row), then a handler here that flips the collapsed state, drops `async_widget_session` and republishes. Out of reach inside `cyrup-ext-subagents` alone.

**Verify** — A left click on row 0 of the async-jobs widget folds it to `subagents (…)`, a second click unfolds it, a modified click or a click on another row does nothing, and each toggle relocks the progressive card.

## SUBA-211 — The async-jobs widget never sees the real terminal size or a launch roster

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read) · **Filed** 2026-10-10 (split from `SUBA-162`)

**upstream** — `fitAdaptiveWidgetLines` (`src/tui/render.ts:2765-2820` @ad11b7ab) chooses the full, one-line or progressive tier from `process.stdout.rows` (`estimateAvailableWidgetRows`, `:2536-2539`) and relocks when rows or columns change (`widgetSessionMatches`, `:2549-2553`). `runningLeafAgentCount` (`:2642-2644`) synthesises steps from `job.agents` when a running job has no step detail, as Fleet does (`fleet-status.ts:467-473`).

**cyrup** — `SUBA-162` ported both rules, but `cyrup_ext::host::HostServices` has no terminal-size accessor, so `publish_async_status_snapshot_widget` (`extension/host/slash.rs`) passes `ASYNC_WIDGET_FALLBACK_ROWS`/`_COLUMNS` (pi's own `|| 30` / `|| 120`). With 11 available rows and a full block of at most 5 lines, the default `adaptive` layout always stays on the full tier; the progressive card is reached only with `asyncWidgetLayout: "rows"`, and the one-line tier (`availableRows <= 2`) and the resize relock are unreachable. `AsyncJobSnapshot::agents` has no producer: `RunStatus`, `TrackedJob` and `AsyncRunView` carry no launch roster, so a step-less running job counts 1 in both the widget and the fleet.

**Impact** — Low: on a short terminal the full block can crowd the editor where upstream would switch to the locked card; a running job whose `status.json` has no steps yet counts 1 instead of its agents (how often that window occurs was not measured).

**Fix** — Give extensions the terminal size (a `HostServices`/WIT addition, outside this crate) and pass it as `AsyncWidgetViewport`; thread the launch-time agent list into `RunStatus` or the tracker so `AsyncJobSnapshot::from_run_status` and the fleet's run-level entry can both use it.

**Verify** — On a 20-row terminal the default layout publishes the progressive card; resizing relocks it; a running parallel launch whose `status.json` has no steps yet counts its agents in both the widget header and the fleet line.

## SUBA-212 — A placed external-CLI step is never flagged

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read) · **Filed** 2026-10-10 (split from `SUBA-131`)

**upstream** — `updateRunnerActivityState` (`src/runs/background/subagent-runner.ts:3156-3238` @ad11b7ab) runs on the 1s `activityTimer` (`:3240-3246`) over every running step and passes `turnCount: 1` for an external-cli runner (`:3173`), so a step placed on a Herdr machine is flagged by the plain idle rule. Only the git baseline is skipped for it (`if (!step.machine && externalAbortSignal) await ctx.prepareExternalActivity?.(…)`, `:871`); its activity comes from `stepOutputActivityAt` (`:2640-2652`).

**cyrup** — `SUBA-131` gave the local external path a monitor (`exec/external_cli/activity.rs`), but `exec/external_cli/placed.rs::run_placed_external_cli` builds none and its result carries no `control_events`.

**Impact** — Low: a stalled agent in a Herdr pane gives no attention notice; the user finds out at the deadline.

**Fix** — Build an `ExternalActivityTracker` with no git baseline for the placed path, tick it while the pane runs, and credit whatever pane output the placement transport observes.

**Verify** — A placed external step that stays silent past `needsAttentionAfterMs` raises `needs_attention`.

## Items in this window held by `09a`

- **`SUBA-106`** (low, `09a`, agent frontmatter `outputSchema`) was **re-read 2026-09-24 at
  `ea23ca2`: still open.** `exec/agent_config.rs::AgentConfig` has no schema field. The only
  `outputSchema` readers under `discovery/` are chain-step parsers (`discovery/types.rs:1455-1458`,
  `discovery/management/config_parse.rs:455-460`). Upstream still parses it at v0.71.0
  (`src/agents/agents.ts:2220-2224`, `assertJsonSchemaObject`, emitted at `:2273`). `09a`'s row cites
  `:2152-2157` **@v0.68.0**, and that citation is correct at that tag. The line moved because the
  file grew. The row stays in `09a`.

## Leads — `v0.67.0..v0.71.0` — RESOLVED 2026-09-24 (pass 2)

*The first pass listed these as unverified. Each was read on both sides by pass 2; the list below is
now a disposition record, not a lead list.*

| lead (first-pass wording) | disposition | evidence |
|---|---|---|
| Typed gates (0.69.0) | **promoted → `SUBA-129`** | `workflows/scripted/engine.rs:1506-1511` string-only; `acceptance.ts:172-236,1259-1290` @v0.71.0 |
| `details.workflowTerminalProof` (0.71.0) | **struck — subsumed** | upstream emits it for ASYNC workflows only (CHANGELOG 0.71.0; `5f326611`); cyrup refuses async workflows (`extension/tool/routing.rs:587`), which is `09`'s lead at `09:192`. Re-open with that lead |
| RPC `cost` / `ping.capabilities.cost` | **promoted → `SUBA-138`** | `extension/rpc/mod.rs:108-118` has eight methods; `rpc.ts:35,770` @v0.71.0. **2026-09-28:** nine now (`SUBA-138` closed) |
| `subagents_enable` lazy loader | **promoted → `SUBA-139`** | cyrup behaves as upstream's unsupported-host fallback (`tool-activation.ts:22-27` @v0.71.0) |
| Required child extensions host API (0.68.0) | **struck — subsumed by `09`'s `SUBA-022`** | upstream's API registers JS module PATHS a child must import (`shared/required-child-extensions.ts:1-60` @v0.71.0); cyrup has no JS-module child extensions, and `background/recovery_descriptor.rs:34-38` already lists `requiredExtensions` as a field cyrup cannot carry. It is a leaf of the typed extension API `SUBA-022` owns **FOLD-IN 2026-10-03 (v0.75.0):** `552f1dd7`/#2643 `requireForAllRunners` (`src/shared/required-child-extensions.ts:20,24-25` @v0.75.0: reject launches whose runner or machine placement cannot load the required extensions instead of dropping them) and `dbb92db1`/#2640 (a required extension that throws during `session_start` stops the launch) are further leaves of the same JS-module child-extension API owned by `09`'s `SUBA-022`; no separate row. |
| `agentExcludeDirs` / `defaultSubagentOnlyExtensions` | **promoted → `SUBA-123`**, with `agentScanDirs` | the first pass's "a warning, so neither is silent" was **wrong**: the census covers only `agentOverrides.<name>` (`discovery/mod.rs:1179-1203`) |
| `PI_SUBAGENT_CACHE_RETENTION` | **promoted → `SUBA-140`** | `shared/child-cache-retention.ts:22` @v0.71.0 |
| Child session naming via `intercom:session-identity` | **promoted → `SUBA-134`** (with the base naming) | `55b79828`; area 11 owns the intercom half |
| Structured-output error summaries / keep valid output | **promoted → `SUBA-127`** | `exec/mod.rs:1633`; `structured-output.ts:62-80`, `execution.ts:1449-1471` @v0.71.0 |
| Hidden skills (`disable-model-invocation`) | **promoted → `SUBA-125`** | `discovery/skills.rs:156-190`; `skills.ts:712` @v0.71.0 |
| Duplicate completion notices (#2389) | **struck — not applicable by construction** | upstream's fix (`9b8356d4`) dedupes sends when the npm package is loaded twice into one session; `cyrup-ext-subagents` is a native built-in registered once by the host. *Falsified if* a session can register this native extension twice |
| MCP direct-tool names vs pi-mcp-adapter (#2395) | **struck — the invariant holds against cyrup's own adapter** | `exec/mcp_direct_tools.rs::format_tool_name` (`:1315-1330`) is written as the twin of `cyrup_mcp::registration::format_tool_name`; whether cyrup-mcp's names match pi-mcp-adapter's is area 13's question |
| External-CLI logs in Fleet (#2375) | **promoted → `SUBA-141`** | `background/fleet_view.rs:1088-1106`; `fleet-view.ts:584-605` @v0.71.0 |
| Usage reconciliation (0.70.0) | **parked under tracker `SUBA-142`** | reconciles from the in-process session's `messages` (`execution.ts:1320-1325` @v0.71.0), which a spawned child does not expose |
| Retained agent resuming despite its own allowlist (#2379) | **struck — subsumed by `SUBA-111`** | the exception exists only once `allowedAgents` does; cyrup has none (`SUBA-111`) |
| Removed `readonly-model-continuation.ts` | **struck — never ported** | `exec/fallback.rs:1336-1338` lists `ReadonlyContinuation` as UNPORTED; that comment is now stale and should go with `SUBA-109`'s fix |
| Removed `completion-evidence.ts` | **struck — covered by `SUBA-107`** | its plan wrapped the guard `SUBA-107` removes; cyrup's `exec/completion_guard.rs` / `exec/mod.rs:1489` go with that item |
| Removed `readonly-session-evidence.ts` | **struck — never ported** | zero hits for `readonly_session\|ReadonlySession` in the crate |

**Negative results from pass 2's line-by-line read (read, nothing owed, recorded so no pass re-derives them):**
`chain-append`'s `flatSteps.push` fix (`subagent-runner.ts:2519`) — cyrup derives the flat layout as a
pure function of the current step list (`background/flat_index.rs:1-35`), so there is no cached flat
list to go stale after `turn_loop.rs::append_steps`; the `httpIdleTimeoutMs` runner
dispatcher (`1a7101e6`, `runner-http-dispatcher.ts`) — a cyrup runner child is a full `cyrup` process
reading its own settings, so there is no bare dispatcher to configure; the runner startup handshake
rewrite (`ack`/`confirm`/`proceed`, `085d8605`, `9003911d`) — Node-specific, cyrup's detached runner
has its own lease/startup path (`background/session_lease/`); the `requestedModel` field and removal
of `attemptedModels`/`modelAttempts` (`2b64ced0`, `f58dfcb5`) — part of `SUBA-109`'s ladder removal;
authority `inspectorOpen`/`projectOpen` (#2269) — ported (`registration/authority.rs:54-94`); the
home-directory-is-not-a-project rule (#2214, second half) — ported (`discovery/mod.rs:186-230`).

**Read on one side only, left as ownerless leads (outside this file's scope):** the top-level `gate`
tool parameter exists at v0.43.0 (`src/extension/schemas.ts`, one `gate:` property at v0.43.0,
v0.47.1, v0.57.0) and is absent from cyrup's tool schema (`extension/tool/schema.rs` has `gate` only
as a lane `mode`), so it belongs to `09`'s scope; foreground prompt audit (`runs/foreground/prompt-audit.ts`,
present from v0.57.0) has no cyrup module and sits in `09a`'s window. Neither was compared further.

## Post-tag leads (pi-subagents `v0.71.0..6f1027f7`)

README cites upstream only at tags, so these are leads, not items. All ten commits were read.

- `965dd4b8` *fix(supervisor): show question text in pending requests* — applies: cyrup's
  `native_supervisor.rs::format_pending_line` (`:738-752`) prints the header only.
- `5794f52d` *fix(fast): accept any native openai-codex model* — applies: `exec/spawn_plan.rs:207-212`
  still pins `FAST_MODE_ALLOWED_MODELS` to two models.
- `d44b514e` *fix(worktree): keep UTF-8 naming labels within byte cap* — no counterpart found: cyrup's
  `spawn/worktree.rs` has no byte-capped naming label. Not compared further.
- `ffeec95c` *fix(external-job): service bridges of child-launched runs* — cyrup's external-job bridge
  servicing was not traced; lead.
- `d8591e80` *fix: pair awaited workflow child lifecycle events* — async-workflow event surface; see the
  async-workflow refusal above.
- `4b28f863` *fix(status): report the child's actual context limit* — in-process `created.contextWindow`;
  parked with `SUBA-142`.
- `d74dc57c` (package metadata), `68cea36f`, `255f66a0`, `6f1027f7` (tests only) — nothing to port.

### Settled by this pass — negative results about the 2026-09-14 census leads

- **`09a` census, *"Watchdog bounded model-fallback chains (`fallbackModels`)"*: DEAD.** Upstream
  removed watchdog fallback at v0.68.0, and `src/watchdog/child-status.ts:131-132` now **refuses**
  the key. `grep -rln fallback_models crates/cyrup-ext-subagents/src/watchdog` is empty, so cyrup
  never ported it. Nothing is owed. Do not re-file it.
- **`09a` census, *"In-process pi child sessions replace the spawned-child-process model"*:
  STILL OPEN AS A LEAD** *(pass 2: promoted to tracker `SUBA-142`)*. At v0.71.0, upstream still builds children in-process
  (`src/runs/shared/child-launch.ts::buildInProcessChildLaunch`). cyrup still spawns subprocesses.
  The design question that lead raises has not been answered. One consequence that is now visible:
  `SUBA-110`'s scrub only applies where upstream still spawns a process. Upstream's in-process
  foreground children see the parent's environment, so cyrup's subprocess foreground child inheriting
  it is *not* a divergence.

## Blind spots — read before the next pass

*Updated by pass 2 (2026-09-24). The first pass's items 1, 2 and 4 are rewritten below; the
originals are in `git log -p` of this file.*

1. **`v0.57.0..v0.67.0` has had its leads resolved, not its diff read.** Every census lead in `09a`
   and here now has a disposition, but the 207-file `src/` diff was not walked line by line. The
   crate's `@v0.68.0` citations close much of it; a modified-file behaviour change that no lead names
   is still invisible. This is the next pass's job.
2. **`v0.67.0..v0.71.0`:** the five largest modified files were read line by line (runner, executor,
   execution in full; async-execution and agents with the Herdr/ladder hunks filtered out); every
   other `src/` file over 100 changed lines was read commit by commit. Node-runtime launch plumbing
   (`jiti`, Bun, native TypeScript hooks) was read and deliberately not compared.
3. **Nothing was observed live.** Every mechanism is a static reading, per README *Where this analysis
   is blind* §2. `SUBA-114`, `SUBA-115` and `SUBA-117` are the three worth a live repro first.
4. **`v0.71.0..HEAD`** (10 commits) was read as post-tag leads (`## Post-tag leads`); none is an item
   until a tag cuts it.
