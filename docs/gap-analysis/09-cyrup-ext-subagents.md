# 09 — cyrup-ext-subagents

Covers `cyrup/crates/cyrup-ext-subagents/` (the largest crate) — subagent discovery, registration,
foreground/background execution, chain/parallel orchestration, acceptance gating and the subagent TUI
surface — measured against `pi-subagents/` at the ported baseline **v0.43.0** (the crate still records
no version string; v0.43.0 is the `PARITY-GAPS.md` inference, and every upstream claim below was
settled with `git show v0.43.0:<path>` or `git show v0.47.1:<path>`, never clone HEAD, because
clone-HEAD line numbers and file existence both mislead here).

> ### RECONCILIATION 2026-09-16 — the ten open rows re-read against `cc7818b`; three closed, three partially closed
>
> **Pins for THIS pass, and only this pass.** cyrup code HEAD **`cc7818b`** (branch
> `claude/subagents-next`, cut from `main` at the same sha). Upstream **pi-subagents v0.68.0** —
> re-measured here with `git -C tmp/pi-subagents tag --sort=-v:refname | head -1`, not inherited from
> a LOC figure recorded elsewhere. Every upstream claim added by this pass was settled with
> `git -C tmp/pi-subagents show v0.68.0:<path>` (or `v0.43.0:` where the question is in-baseline-ness);
> **no pre-existing `@v0.43.0` / `@v0.47.1` citation in this file was rewritten to a newer tag.**
>
> **What this pass covers that no prior pass did:** PRs **#137** (`21d1acc` / `2bd76ac`), **#139**
> (`b2fdc7e` / `e61ff44`) and **#140** (`cc7818b` / `7e41cf9`) — the whole SCOPE sequence, ~31k lines,
> landed after the 2026-09-14 provenance correction and after the `d53763b` re-read it records.
>
> **Disposition of the ten rows this file carried open** (`SUBA-016`, `054`, `056` medium;
> `017`, `022`, `023`, `024`, `026`, `061`, `063` low):
>
> | id | was | now | why |
> |---|---|---|---|
> | `SUBA-016` | still-open · XL · **BLOCKED** | **CLOSED** `7e41cf9` (#140) | all nine verbs dispatch, tick fires, gate consulted, 100/100 green |
> | `SUBA-056` | still-open · M | **CLOSED** `2bd76ac` (#137) + `e61ff44` (#139) | replay + archive written on the live terminal path, read back by `wait` |
> | `SUBA-063` | still-open · low | **PARTIALLY CLOSED** | zero-budget DECODE and the events-cap override landed; no parent-side writer, no ack path, no truncation marker |
> | `SUBA-054` | still-open · M | **PARTIALLY CLOSED** | foreground SINGLE landed; async SINGLE still pins `reads: None` |
> | `SUBA-023` | still-open · low | **PARTIALLY CLOSED** | signal-name attribution landed with production consumers; `process-terminal` / `session-lease` unported |
> | `SUBA-017` `022` `024` `026` `061` | open | **open, evidence refreshed** | every grep re-run at `cc7818b`; `026` stays partial |
>
> **THE REAL LEDGER DEFECT IS THAT THIS FILE CARRIES TWO TABLES THAT DISAGREE — and an earlier draft
> of this very block blamed the wrong one. Recorded rather than quietly fixed.** The `## Open items`
> table is the maintained one and the one `scripts/count_open_items.py` reads; it had already caught
> most of what this pass re-derived — `SUBA-023` and `SUBA-054` were marked PARTIALLY CLOSED there in
> 2026-08-14, and `SUBA-063`'s row records at `9aeba769` that part (a)'s zero-hit grep was REFUTED and
> part (c) CLOSED. **`## Status table (every item from every prior pass)` is the stale one**: it still
> called `SUBA-054` and `SUBA-056` "new this pass", still carried `SUBA-016`'s zero-hit line, and its
> `SUBA-063` row still read "unported". Every one of those rows is rewritten by this pass.
> **So the 2026-09-14 provenance block's "none closed" is accurate as written** — no row closed FULLY
> in that window, and each row does carry its own honest `RE-READ … at d53763b` line. What it could not
> know is that #137/#139/#140 were about to close two of them outright.
>
> **Landing shas, stated precisely.** `SUBA-016` and `SUBA-056` are attributable to this window and are
> cited to it. The landed HALVES of `SUBA-023`, `SUBA-054` and `SUBA-063` are not: all three were
> already present at `d53763b` (`git show d53763b:…` — `spawn/signal.rs` has `pub fn signal_name`,
> `extension/executor/foreground.rs` has `reads: agent.default_reads.clone()`, `exec/tool_budget.rs`
> has `TOOL_BUDGET_ZERO_AUTH_ENV`), and `git log -S` on each bottoms out at `da970e8`, which is this
> repo's FIRST commit (`git show da970e8^` → *"invalid object name"*). **No finer landing sha exists
> for them and none should be invented.**
>
> **Two code defects found while verifying, filed as rows rather than left in prose:** `SUBA-097`
> (an in-source claim of integration coverage for a file that does not exist, over the one seam
> `SUBA-016`'s own tests stub out) and `SUBA-098` (a stray CJK token inside an English doc comment).
>
> **Counted set after this pass: 0 critical · 0 high · 1 medium · 9 low = 10 open** for area 09, and
> **1 medium = 1 open** for `09a` (`scripts/count_open_items.py`, re-run after these edits; `SUBA-005`
> stays under `## Trackers`, uncounted). The area's only remaining medium is **`SUBA-054`'s async
> half**, and it needs a DECISION — which of upstream's two cwds a runner step's reads resolve against
> — not effort. `SUBA-016` and `SUBA-056` leave the medium set entirely. The open count happens to
> stay at ten because the two lows filed from the code (`SUBA-097`, `SUBA-098`) replace them; the
> medium set went **3 → 1**. Two ids added, none renumbered, none deleted.

> ### PROVENANCE CORRECTION 2026-09-14 — pins re-derived; this file was NOT re-read
>
> **Audited at (history — unchanged, and correct as written):** pi-subagents **v0.43.0** (the
> inferred ported baseline) and **v0.47.1** (then-latest), cyrup HEAD `04c1ba2`, reconciled at
> `380c713`. Every `git show v0.43.0:<path>` / `git show v0.47.1:<path>` citation in this file records
> what somebody actually read at that tag. **None of them may be rewritten to a newer tag** — doing so
> fabricates verification that never happened.
>
> **Current pins:** pi-subagents **v0.67.0**. cyrup code HEAD **`d53763b`** (PR #134 / `e5a0789`,
> "the `workflowScript` runtime surface", +3993/−121 across 18 files, merged AFTER the row-level
> audit). **The row-level audit below was performed at `9aeba769` — that pin is history and stays.**
> `9aeba769..b28d3ff` is docs-only, so `b28d3ff` and `9aeba769` are byte-identical under `crates/`
> (the README baselines table records `9aeba769` as the last CODE commit and marks `824a539e`
> superseded); rows that predate the `9aeba769` pass are pinned to code at or before `824a539e`.
> `b28d3ff..d53763b` is #134 alone, confined to `extension/executor/`, `extension/tool/`, `missions/`
> and `workflows/scripted/`. **Every still-open row in this file was re-checked against that window on
> 2026-09-14 and none closed** — each carries its own `RE-READ … at d53763b` line.
>
> **Therefore unmeasured by this file:** upstream **`v0.47.1..v0.67.0`** entire — of which
> `v0.47.1..v0.57.0` is covered by `09a` and **`v0.57.0..v0.67.0` is covered by no file** — and the
> cyrup side **`824a539e..b28d3ff`**, which is where the whole `workflowScript` runtime and the
> net-new `cyrup-workflow-runtime` crate landed.
>
> **This correction re-read nothing.** No row was re-verified, no severity re-derived, no count
> changed, no item opened or closed. Only the pins moved. The census below is a worklist of
> **UNVERIFIED leads**, not findings.
>
> **A second pin correction, recorded and not applied:** the "≈v0.43.0" ported baseline is
> contradicted by this crate's own in-source citations —
> `grep -rhoE 'v0\.[0-9]+\.[0-9]+' crates/cyrup-ext-subagents/src | sort | uniq -c | sort -rn` at
> `b28d3ff` returns v0.43.0=611, v0.34.0=334, **v0.64.0=330**, v0.57.0=120, v0.47.1=98, v0.62.0=6,
> v0.63.0=5, v0.66.0=1, and zero citations of v0.65.x or v0.67.0. **This crate has no single baseline
> and the README's one-number cell cannot express that.** The practical consequence for every row
> below: a surface absent from cyrup but stamped `@v0.64.0` elsewhere in the same crate is a **miss**,
> not lag. Other upstream pins re-checked and unchanged: `pi-permission-system` v0.8.0,
> `pi-intercom` v0.13.0, `pi-acp` v0.0.33.

## UNVERIFIED census 2026-09-14 — leads against the current pins

**Nothing in this section is a finding.** These are leads collected by a census pass reading source on
both sides at the pins above; none has been through an adversarial refutation, none carries a `SUBA-`
id, and id assignment belongs to a pass that verifies both sides. **No open-item row, severity or
count elsewhere in this file was changed by this census.** Where a lead bears on an existing item it
names that item; the item's row is untouched.

Sizes are the census's own estimate on the same `S`/`M`/`L` scale this file uses.

**Self-citations by line number are as-read before this block was inserted.** A `09:NNN` offset
below is short by **209 lines** against this file at HEAD, and a `09a:NNN` offset by **352 lines**
against `09a` at HEAD; both files gained a provenance block and a census ahead of their bodies in
the same pass. Follow the item id, not the offset.

### Corpus health and provenance

- **243 in-source cyrup citations now name upstream files that do not exist at v0.67.0.** `S` —
  `git cat-file -e v0.67.0:<path>` fails for all of `src/runs/shared/pi-args.ts`,
  `child-protocol.ts`, `subagent-startup-retry.ts`, `turn-budget.ts` and `src/shared/post-exit-stdio-guard.ts`;
  cyrup-side counts by `grep -rc` over `crates/cyrup-ext-subagents/src` are pi-args 171, child-protocol 44,
  subagent-startup-retry 14, turn-budget 14, post-exit-stdio-guard 0. Four were deleted by `d9bc62f8`
  (the in-process migration), `turn-budget.ts` by `94ecb662`. **Tag-stamped citations (`@v0.64.0`) stay
  resolvable and are not the problem**; bare ones become *unresolvable, not merely stale* — the same
  trap `09a`'s restructure-trap section documents for `extension.rs`, running in the opposite
  direction. This is the citation-lint case the README already owns.
- **The recorded ≈v0.43.0 baseline is contradicted by the crate's own citations.** `S` — see the
  provenance block above. Counter-evidence for the README's first standing hazard ("census the
  baseline, do not inherit it").
- **The `workflowScript` port is pinned to v0.66.0 and nothing in the code says so.** `S` — recovered
  only by line-number archaeology: `workflows/scripted/engine.rs:1` and `:2610` cite
  `scripted-workflow.ts:1717-2284`; that file is 2 284 lines at v0.66.0 with `:1717` =
  `export async function runWorkflowScript(`, and 2 514 lines at v0.67.0 where `:1717` is
  `dynamic = true;`. Zero `@vX.Y.Z` markers exist anywhere in `src/workflows/` or
  `cyrup-workflow-runtime/`. Two leads: the crate should record its tag, and **`v0.66.0..v0.67.0` on
  `src/workflows/` is an unmeasured drift window over the freshest code in the crate**.

### workflowScript — ported, and mostly ungranted

The subsystem `09:1306` (blind spot 2) called "the biggest unaudited mass" now exists:
`crates/cyrup-ext-subagents/src/workflows/` is 27 files / 19 957 lines, `crates/cyrup-workflow-runtime`
is 926 lines of src, the tool advertises `workflowScript` (`extension/tool/schema.rs:337`) and
`route_workflow_mode` (`extension/tool/routing.rs:523`) mints a real `RunMode::Workflow` run dir.
Upstream v0.66.0's `src/workflows/` is 9 files / 4 414 lines; all nine have counterparts.

- **`SUBA-016`'s `BLOCKED on workflowScript` rating is stale.** `M` — the stated hard prerequisite is
  satisfied. `schedule.*` is still undispatched and the code says so twice
  (`extension/executor/foreground_actions/stop.rs:215`, `registration/authority.rs:66`), so the item
  stays open, but its **XL / BLOCKED** rating and its whole blocking paragraph (`09:517-523`) are wrong
  and must be rewritten before anyone plans against them. `scheduled-runs.ts` was not read this pass,
  so the residual size is unmeasured.
  **[SETTLED 2026-09-16 — `SUBA-016` is CLOSED at `7e41cf9`.]** The lead was right about the rating and
  wrong about the residual: `scheduled-runs.ts` @v0.68.0 is 1 012 lines and `background/scheduled_runs/`
  ports it in 6 669 lines across 8 files. The two "`schedule.*` is still undispatched" citations this
  bullet quotes are themselves stale — `stop.rs` now OFFERS an armed schedule as a stop target and
  prints `subagent({ action: "schedule.pause", id })` (`foreground_actions/stop.rs:324`), and
  `registration/authority.rs`'s `ScheduleCreate` mapping is consulted for real at
  `extension/tool/routing.rs:1274-1304`. See the row.
- **Async/detached workflows are permanently refused; upstream's `workflowScript` default IS async.** `L`
  — `routing.rs:544-549` rejects `async: true` outright and `:537` calls the refusal permanent (§0.6);
  upstream `extension/schemas.ts:348` @v0.66.0 reads "Normally async unless `asyncByDefault:false`",
  `config.ts:235-236` returns `config.asyncByDefault !== false`, `tool-description.ts:35` says
  "Async/background runs are the normal default". **This is the root of the group: `children.list`
  retention, durable completion replay, `schedule.*` and the WORKFLOW_3/WORKFLOW_13 forward references
  are all downstream of detachment.** The ledger has no "accepted divergence" category for it.
- **`runs.host` ported (1 594 lines) but never granted.** `M` — `workflows/host_command.rs` is fully
  ported with one production consumer (`exec/output.rs`), but `WorkflowRunHost` never overrides
  `supports_host()` and `routing.rs:691` passes `on_host_step: None`;
  `cyrup-workflow-runtime/src/js/prelude.js:574` deletes `surface.host`, so `runs.host` is `undefined`
  rather than refusing. `workflows/host_step.rs` is in the same state. Upstream:
  `src/workflows/host-command.ts` @v0.66.0 (235L, net-new since v0.57.0).
- **Mission workflow `state.get`/`state.set` unwired.** `M` — `routing.rs:672` passes `state: None` and
  both the run arm and the `validate` arm hard-code `state_enabled: false` (`:553`, `:1497`), so
  `globalThis.state` is never installed. `missions/workflow_state.rs` exists.
  `PARITY-GAPS.md:1385` names `src/missions/workflow-state.ts` as unported *because its only consumer
  was unported* — the consumer now exists and declines the capability, which moves the item from
  **not-ported to unwired**.
- **Workflow RESOURCE provenance unwired.** `M` — `workflows/resources.rs` (883L) and `permit.rs` (574L)
  are ported and the executor owns a `WorkflowResourceRegistry` (`extension/executor/mod.rs:227`), but
  `routing.rs:659` passes `one_use_permit: None` and `:863` `resource: None`, and cyrup's 48 advertised
  keys (`extension/tool/schema.rs`) contain `workflowScript` and none of `workflow` / `args` /
  `workflowScriptPath` (upstream `extension/schemas.ts:282-395` @v0.66.0 has all four). This is also
  why `runs.host` stays off — `lib.rs`'s op doc says host is registered only for `workflow`-provenance
  runs.
- **Seven more upstream workflow-level params unadvertised while their modules are ported.** `M` —
  `globalConcurrencyLimit`, `maxSubagentSpawnsPerRun`, `preflight`, `chatProgress`, `isolation`,
  `baseRef`, `lane` all sit at top level in `extension/schemas.ts:282-395` @v0.66.0; none is in cyrup's
  advertised list. Yet `workflows/preflight.rs` is 1 306 lines, `chat_progress.rs` 652,
  `lane_metadata.rs` 16 122 bytes, `scripted/git_ref.rs` is `valid_git_ref` (= `baseRef`), and
  `exec/run_fanout_budget.rs` is 1 667 lines whose config key exists (`registration/mod.rs:108`) but is
  never a tool param. **One cause, so probably one row with a sub-list, not seven rows.**
- **`checklist.rs` (2 301 lines) and `chat_progress.rs` (652 lines) have NO production caller.** `M` —
  `git grep project_workflow_checklist` and `resolve_workflow_chat_progress` at `b28d3ff` return only
  the module and its `mod.rs` re-export. ~2 950 lines of ported, tested, dead code, distinct from the
  param gap because wiring `chatProgress`/`preflight` as params would still not reach the checklist
  projection. Exactly the class `09:1085-1099` blind spot 6 predicted would recur.
- **Three live `RunWorkflowScriptOptions` callbacks passed `None`, each with its reason in-line.** `S` —
  `routing.rs:674` `register_stop_child: None`, `:682` `on_lane_plan: None` (`runs.lanes` still works;
  only the advisory plan callback is undelivered), `:689` `on_emit: None` with a two-part reason
  (quadratic re-forwarding; emit is the one callback whose failure is fatal). `on_trace: None` is
  deliberate and correct. **`on_emit` is the one with real cost** — live emit forwarding is absent and
  the code says it needs an engine change, not a wiring change; `register_stop_child` becomes
  load-bearing the moment async workflows exist.
- **`extension/executor/workflow.rs`'s module doc contradicts its own code.** `S` — `:4-11` says the
  file implements "the two REQUIRED trait methods and nothing else" and that keyed resume is "PRESENT
  AND REFUSING"; `:578-583` says keyed receipt resume **is** available, `supports_resolve_resume`
  returns `true` with a full `resolve_resume` at `:585-617`, and `supports_steer` returns `true` at
  `:624`. Documentation-only, but it is the header a later pass reads first and it under-reports the
  port. cyrup side only.
- **Does `cyrup-workflow-runtime` need its own area file?** `M` — the net-new crate is `Cargo.toml` (23),
  `src/lib.rs` (322: the `WorkflowOpsBridge` trait, 12 `#[op2]` ops, `build_snapshot`) and
  `src/js/prelude.js` (604: the guest sandbox and every guest-side validation error string), consumed
  twice by `cyrup-ext-subagents` (a `[build-dependencies]` edge from `build.rs`, and `engine.rs:2166`
  `include_bytes!` / `:2301` `startup_snapshot`). **The census's read, offered as a lead not a ruling:
  no peer area file.** It has no upstream of its own — it is one half of ONE upstream file
  (`scripted-workflow.ts`'s 960-line `WORKER_SOURCE`, `:34-995` @v0.66.0), split purely by a Cargo
  constraint the crate states explicitly, and a peer file would split one upstream unit across two
  areas. The shape that fits is a `## workflowScript runtime` section here, or a
  `09b-cyrup-workflowscript.md` covering BOTH `src/workflows/` and this crate — which the ~20 900-line
  size does justify. `prelude.js`'s error strings are product surface (`scripted/mod.rs:20` says so)
  and need a home either way.

### Tool surface and verb census

- **`SUBA-005`'s verb census is stale in both directions: 32 advertised vs 57 upstream.** `S` — upstream
  `src/shared/types.ts:2757` @v0.66.0 has **57** `SUBAGENT_ACTIONS`; `extension/tool/text.rs:195-258`
  has **32** (adds `guide`, `validate`, `dismiss`, `grant-spawn-budget`, `mission.resolve-decision`
  since the recorded 27). `09:353`'s "27 … 50 @v0.43.0 and 53 @v0.47.1" and `09a:3344`'s "30 vs 52" are
  both superseded. Newly-unowned verb families visible at v0.66.0: `worktree.cleanup`, `lane.status`,
  `lane.recordMerge`, `lane.recordSupersession`, `refine`, `refine.show`, `inspector.open/status`,
  `project.open/status`, `debug.run`, plus the nine `schedule.*`. **Tracker maintenance, not new work —
  but `SUBA-005` is the index other rows route to, so a stale count there mis-sizes the area.**
  *(2026-09-19: `debug.run` landed — `background/run_lifecycle_debug.rs`, see `SUBA-005`'s row.)*
- **`append-step` is still dispatched by cyrup; upstream deleted it between v0.47.1 and v0.57.0.** `S` —
  `SUBAGENT_ACTIONS` contains it at v0.43.0 and v0.47.1 and not at v0.57.0, v0.64.0 or v0.66.0;
  `extension/tool/text.rs:195-258` still lists it and `schema.rs` advertises the whole array as the
  `action` enum, so the verb is live on the model-facing surface. **CAUTION:** `09a:3344` already notes
  `−append-step … via 7ece6f35` and routed it to `SUBA-005` as census rather than work, so this may be
  knowingly-unfiled — check `SUBA-005`'s body before filing.
- **cyrup still advertises the v0.34-era execution surface upstream deleted: `tasks`, `chain`,
  `concurrency`, `chainDir`, `clarify`.** `L` — none is in `extension/schemas.ts:282-395` @v0.66.0;
  `extension/tool/schema.rs` inserts all five and `route_chain_mode`/`route_parallel_mode` are still
  live arms. `09:1306` predicted exactly this but framed it as blocked behind the unported runtime;
  **the runtime is now ported and the two surfaces coexist**, so the deferred question — retire, keep
  both, or document the divergence — is answerable and schedulable. A scope/decision lead, not a defect
  claim.
- **`turnBudget`: cyrup carries a live implementation upstream deleted inside the window — bears on
  `SUBA-008`.** `M` — `94ecb662` "refactor: remove turn budget controls (#1579)" (after v0.57.0) deleted
  `src/runs/shared/turn-budget.ts` (98L) and stripped it from `agents.ts`, `agent-management.ts`,
  `agent-serializer.ts`, `runtime-agent-registry.ts`, `api/delegation.ts`, `api/preflight.ts`,
  `extension/schemas.ts`, `launch-contract.ts`, `slash/delegation-*.ts`, `execution.ts` (−95) and
  `subagent-runner.ts` (−217); `git cat-file -e v0.67.0:src/runs/shared/turn-budget.ts` fails. cyrup's
  `exec/turn_budget.rs:166`/`:326` is live on the production path (`exec/spawn_plan.rs:807`) with result
  fields at `exec/run_result.rs:126,134`. **`stale-port` class — but "delete `turn_budget.rs`" is NOT
  the obvious fix**: upstream removed the CONTROLS and kept residual `turnBudget`/`turnBudgetExceeded`
  STATE through `async-job-tracker.ts` and `async-resume.ts` at v0.67.0. The right question is which
  half of `SUBA-008`'s surface still has an upstream counterpart, and that needs a field-by-field read
  of those two files at the tag, which this census did not do. `SUBA-008`'s row is unchanged.

### Ledger maintenance — paragraphs that now point the wrong way

- **Blind spot 4 is closed: `run-fanout-budget.ts` is ported.** `S` — `09:1085-1099` blind spot 4 said
  it was "deliberately not filed, because the hard rules require citing a named tag. Pick it up on the
  next tag." It was picked up: `exec/run_fanout_budget.rs:1-20` is 1 667 lines under the port's own id
  `CFG-067`, with a directory-backed claims ledger crossing the spawn boundary via
  `RUN_FANOUT_BUDGET_ENV`, config key at `registration/mod.rs:108` and doctor surface at
  `extension/executor/reports.rs:110`. The blind-spot entry should be retired; the residual (the
  `maxSubagentSpawnsPerRun` TOOL PARAM is still unadvertised) is the seven-params lead above. **Upstream
  side not read — this is a code-side observation plus the port's own citation.**
- **`## Coverage` and `## Blind spots` are materially stale on this subsystem.** `S` — blind spot 2
  ("the biggest unaudited mass is `workflowScript` … treat this area's open count as a floor by a wide
  margin") no longer describes the crate; blind spot 3 lists `scripted-workflow.ts` (502 lines) as "not
  read at all on the upstream side" where it is now the crate's largest ported unit; blind spot 4 is
  closed; and `09:929`'s `SUBA-055` closure note declines `children.list` as "part of the unported
  `workflowScript` shape" where `extension/tool/text.rs:203-213` has **already retired that wording in
  the code** and replaced it with the real reason (no retention — the foreground host's `settled` list
  dies with the tool call, so the listing would always be empty). Pure ledger maintenance, but these
  are the paragraphs a planner reads to size the area.

### Where the cross-file work lands

The census's `v0.57.0..v0.67.0` leads are filed in `09a` for want of a better home, and `09a`'s scope
line binds it to `v0.47.1..v0.57.0`. **A `09b-cyrup-ext-subagents-v0.67-drift.md` is the structurally
correct home**, on the precedent that created `09a`. `PARITY-GAPS.md`'s `VL-S2` (`:1174-1175`,
`workflowScript` + `chatProgress` as a large unported port bug citing `extension.rs:5327-5338`) and
`PARITY-PLAN.md:1033`/`:1396` are the cross-file owners of the workflow rows above and are stale in
the same direction; `PARITY-GAPS.md:1562` still lists `workflows/scripted-workflow.ts` as unread
upstream. None of those files was touched by this census.


> **Re-audited 2026-08-12, cyrup HEAD `04c1ba2`** (last code commit; tree clean at docs-only
> `a9000b1`), against **pi-subagents `v0.43.0`** (ported baseline) and **`v0.47.1`** (latest tag).
>
> **22 items closed**, 4 partially closed, 2 re-written as misdescribed, **24 newly filed**
> (`SUBA-043` … `SUBA-066`), 1 new lead refuted and recorded in `## Coverage` so it is not
> re-derived. Open set after the audit pass was **46 items: 0 critical, 2 high, 24 medium, 20 low**;
> the repair pass below reclassifies one of those (`SUBA-005`) as a tracker, leaving **45 counted
> items + 1 tracker**.
>
> What actually changed on the cyrup side: batches 8–10 landed the child-side prompt runtime, the
> structured-output capture channel, the control/activity pipeline, the async deadline + cascade,
> the watchdog subtree, FleetView, agent memory, prompt workflows, the native supervisor channel and
> the tool-budget enforcer. **Every one of those closures was audited for what the new code does, not
> that it exists** — and that audit is where most of the 24 new items come from. The recurring shape
> is a capability that is *implemented and unreachable*: `toolBudget` is enforced but not advertised
> (`SUBA-047`), `defaultReads` is parsed and never used on a single run (`SUBA-054`), `outputSchema`
> is captured on `tasks[]` but unadvertised on SINGLE (`SUBA-043`).
>
> **Version lag.** The prior pass recorded pi-subagents "latest v0.43.0"; latest is now **v0.47.1**.
> The workspace brief measures the full range at 151 files / +10254 / −1333; the src-only sweep run
> for this pass covered **96 non-merge commits, 67 files, +4696/−769, 12 net-new source files**, none
> of which any prior pass had seen. Eleven of the new items come out of that range
> (`SUBA-044`, `SUBA-050` … `SUBA-060`, `SUBA-065`, `SUBA-066`).
>
> **The two-table split is gone.** All seven `-S` surface-sweep items closed this pass, so the open
> table below is now the complete open set for this area. The `-S` ids are retained in the status
> table so their closures can be re-audited.
>
> ---
>
> **REPAIR PASS, same day (2026-08-12), applying the completeness critique.** No item was renumbered,
> merged or deleted; no new items were filed (no sweep digest routes to this area). Three changes:
> - **`SUBA-005` is now a `tracker`, not a backlog item.** Its own Fix says "this item is the ledger,
>   not the work" — the 27-vs-50/53 verb count is an index over other people's subsystems. It keeps
>   its ID, its severity annotation and its whole body, and it moves to a separate **Trackers** table
>   below so it is excluded from the item count. What it owes (owners for the seven still-unowned
>   verbs, and a completeness assertion pinning the enum against upstream's array) is bookkeeping the
>   next pass performs, not code someone schedules.
> - **The status table now carries a row per ID.** The previous edition collapsed this pass's new
>   items into one `SUBA-043 … SUBA-066` range row, which satisfies the letter of "every item from
>   every prior pass" and defeats its purpose: a reader looking up `SUBA-057` found no row. All 24 are
>   enumerated.
> - **Severities were re-derived** against `README.md:106-107` rather than left as filed; two were
>   examined and both stand. Recorded in `## Coverage` → *Severity re-derivation*.
>
> **Open set after the repair pass: 45 items — 0 critical, 2 high, 23 medium, 20 low — plus 1
> tracker (`SUBA-005`) excluded from that count.**

> ### Reconciliation 2026-08-14 — sweeps 1 and 2 applied, counts re-derived
>
> **cyrup HEAD `380c713`** (this file was written against `04c1ba2`), tree clean. Two whole-backlog
> parity sweeps have landed since this file was last edited: **sweep 1 — 232 items across 11 crates**,
> and **sweep 2**, run under the same rules. Area agents were forbidden from editing documentation so
> that a single writer could reconcile all sixteen files in one pass; this block, and the dispositions
> written into the `## Open items` rows below, are that reconciliation. **Every status in this file
> that predates this block is stale — including the header notes above it and the
> `## Status of every item…` table.**
>
> **No ID was renumbered, merged or deleted.** A refuted item keeps its ID with the refutation
> recorded in its row, so nobody re-derives it. Refutations are corrections to *this analysis*, not
> failures of the sweep — see `00-residual-ledger.md`, which now publishes the measured error rate.
>
> **The test architecture changed underneath every path citation in this file.** The integration
> tests were relocated into their crates as unit tests (`63d729a` / `c3982b5` / `d973906`), taking the
> suite from **310 integration binaries to 6 + 8 gated** behind a new **`cyrup-it`** harness crate.
> The gate is now **6440 tests / 6440 passed / 8 skipped in 16.4 s**. Any citation of the form
> `crates/<crate>/tests/<x>.rs` in this file is stale unless it names `cyrup-it`, and note that
> `cyrup-it` is `required-features = ["it"]`, so **the gate does not build or run it**.
>
> **Still a static analysis.** Neither sweep executed the suite: area agents were restricted to
> `cargo check -p <crate> [--all-targets]` and the orchestrator ran the gate once over the combined
> work. Every red-before/green-after claim below is a reasoned argument plus a type-check, and every
> `Verify` line in this file remains a design, not an observation.
>
> **Area 09 — recount: 48 rows → 26 open (0 critical · 0 high · 14 medium · 12 low).** All five of the
> area's highs are closed: `SUBA-014` and `SUBA-043` in sweep 1, `SUBA-067` and `SUBA-068` before it,
> and `SUBA-069` in sweep 2.
>
> **`SUBA-069` closed only after its own premise was refuted**, and both errors are worth recording
> because they were checkable in seconds with `git show`: (1) "pi's default is the same 5000 ms" —
> pi's `DEFAULT_WORKTREE_SETUP_HOOK_TIMEOUT_MS` is **30000** at both v0.43.0:113 and v0.47.1:114, and
> cyrup's already matched it; (2) "these use the production DEFAULT … so unlike SUBA-068 they cannot
> simply be re-budgeted" — all three fixtures passed `timeout_ms: Some(5_000)` explicitly at HEAD, so
> they were always re-budgetable. The fix is real; the diagnosis was not.
>
> **`SUBA-048` closes PARITY-GAPS `PB-13` with it, as PB-13's own text instructs.** `project_chain_runs_dir`
> had ZERO references crate-wide before sweep 2; `resolve_chain_runs_dir` now gives it a live caller
> and upstream's `project` default takes effect on all three surfaces. Sweep 1 had already REWRITTEN
> this item's Impact — its central claim was measurably wrong, since both run-artifact sites called
> `temp_artifacts_dir(cwd)` directly, so the defect was the inverse of the one filed.
>
> **`SUBA-054` is handed back partial on a decision, not on effort.** The foreground SINGLE half is
> complete and is the whole of the item's Verify. The async half needs a choice the sweep would not
> guess — which of upstream's two cwds (`effectiveCwd` vs the chain dir) a runner step's reads resolve
> against — and guessing would double-emit the instruction on every chain step. **The blocker is
> written into `background/runner_main.rs` at the field rather than left as a silent `None`.** Its
> Fix line is also corrected: upstream has NO top-level `reads` param, so `defaultReads` is the entire
> SINGLE precedence chain and no new advertised param is owed.
>
> **`SUBA-028` was consciously declined and the reason is on the record**: threading a `CancelToken`
> through `evaluate_acceptance` → `run_verify_commands_memoized` → `model::run_memoized_verify_command`
> plus a field on `model::EvaluateAcceptanceInput` touches 40+ call sites, and three other crates
> broke the shared build in sequence during the pass.
>
> **AN ADJACENT REPAIR, flagged for the integration phase:** cyrup-ext's `HostServices::set_widget`
> changed signature mid-pass (`EXT-047`) from one opaque `&Value` to pi's three arguments, and its new
> doc names THIS crate as the defect's victim — "the shipped subagents extension hand-rolled
> `{"key": …, "content": null}` for it and the slot stayed occupied". All four call sites in
> `extension.rs` were adapted, `SubagentFleetStatus::widget_lines` was added beside the retained
> `widget_payload`, `FleetViewPlacement` maps onto `WidgetPlacement`, and the dispose/inspector-open
> paths now issue a real REMOVAL (`lines: None`).
>
> **COVERAGE NOTE:** no item in `12-upstream-drift-pi-core.md` lands in this crate. `DRIFT-014` is the
> only row that names cyrup-ext-subagents and its Fix explicitly says "Do not follow the original
> filing's instruction to add these to `cyrup-ext-subagents/src/exec/fallback.rs`" — recorded here so
> a future pass does not re-derive it.
>
> **`SUBA-030`'s persona half should carry a cross-area dependency marker** rather than reading as
> in-crate work: `crates/cyrup/src/cli.rs` must accept a path form for `--system-prompt` first.


## Status table (every item from every prior pass)

| ID | Status | Evidence |
|---|---|---|
| SUBA-001 | **closed** (7a44aec) | Persona reaches the child as one argv element; residual deltas live in SUBA-030. Not re-derived this pass. |
| SUBA-002 | **closed** (513e45a) — re-audited | `reserve_subagent_spawns` (`extension.rs:760`) now has **four** production call sites — `:8317`, `:10042`, `:10454`, `:10625` — covering tool, slash and chain/graph entries; per-session reset at `:9422` inside the SessionStart handler citing pi's `resetSessionState` (`extension/index.ts:695-803`). Tests at `:13699/:13716/:13753` (the prior note's `:13338-13470` range was wrong). |
| SUBA-003 | **closed** (46c3868) | `modelScope` ported and enforced. Reporting surface remains as SUBA-035; the new `strict` knob is SUBA-050. |
| SUBA-004 | **closed** (46c3868) | `wait` tool present and registered in the `Full` arm. Residuals SUBA-031, SUBA-034; new residual SUBA-056. |
| SUBA-005 | **misdescribed → still-open** | The "15 of 20" framing is stale in *both* directions. cyrup advertises **27** verbs (`extension.rs:6557`); upstream has **50** @v0.43.0 (`shared/types.ts:1885`) and **53** @v0.47.1 (`:1968`). Restated below. |
| SUBA-006 | **closed** | `exec/mod.rs:1463-1478`: `let explicit_tool_allowlist = agent.tools.is_some();` then `if allowlist.is_empty() { --no-tools } else { --tools <csv> }`, matching `pi-args.ts:389-392` @v0.43.0. Test at `exec/mod.rs:4499-4535`. |
| SUBA-007 | **partially-closed** | Enforcement landed: `exec/tool_budget.rs` (388 lines), `TOOL_BUDGET_ENV` written at `exec/mod.rs:1837-1846`, frontmatter key at `discovery/frontmatter.rs:850`. The **tool-param half did not** — residual filed as **SUBA-047**. |
| SUBA-008 | **still-open** | `turn_budget` appears only as a hard-coded `false` consumer in three files (`tui/intercom.rs:348-352`, `exec/fallback.rs`, `exec/mod.rs:2354-2360`), each commented as having no source; no schema key. Duplicate of PARITY-GAPS PB-10. |
| SUBA-009 | **closed** | `registration/slash_commands.rs:11-14` records the removal; the 16-variant `as_str` match at `:127-145` has no `SubagentsCompanions`; `registration/doctor.rs:787-788` records the deleted Companion-packages section. `git ls-tree v0.47.1 -- src \| grep -i companion` is empty upstream. The surviving `tests/companions_*.rs` files are the intercom permission COMPANION — a different subsystem. |
| SUBA-010 | **closed** | `src/native_supervisor.rs` (2251 lines) is a first-class typed channel; `ENV_SUPERVISOR_CHANNEL_DIR` written at `exec/mod.rs:3240-3243`; doctor check in `registration/doctor.rs`. |
| SUBA-011 | **closed** (severity was low, closed at medium) | `src/watchdog/` is 18 modules (~18k lines), registered at `extension.rs:9055` with nine subscriptions at `:9338-9352`. **Existence is not correctness** — PARITY-GAPS UW-3/UW-4/UW-5 record three no-op holes *inside* it and remain open there. |
| SUBA-012 | **closed** | `tui/fleet.rs` (3040 lines) + `fleet_state/fleet_status/fleet_transcript/fleet_overlay/fleet_theme`; `view`/`lines` are advertised properties; `/subagents-fleet` is in the 16-name table at `registration/slash_commands.rs:127-145`. Keystroke half remains PARITY-GAPS UW-7. |
| SUBA-013 | **partially-closed** | Inbox + verb landed: `CYRUP_SUBAGENT_STEER_INBOX` at `exec/mod.rs:1857-1868`, child-side `SteeringInbox` at `prompt_runtime.rs:157-290`, `steer` in the enum (`extension.rs:6557`) dispatched at `:7825-7837`. The **ack/mode half did not** — residual filed as **SUBA-049**. (The audit's cross-reference to "SUBA-045" for this residual was a mislabel; SUBA-045 is the tool-availability diagnostic.) |
| SUBA-014 | **still-open — severity RAISED to high** | Restated below. |
| SUBA-015 | **closed** | `discovery/agent_memory.rs` exists; the memory block is folded onto the persona at `exec/mod.rs:1544-1553` (`persona_with_memory`) before the refinement overlay. Live production path. |
| SUBA-016 | **closed 2026-09-16** (`7e41cf9`, PR #140) — *was* still-open/XL/BLOCKED | **The prior evidence line was false at `cc7818b` in every clause.** `background/scheduled_runs/` is 8 files / 6 669 lines (`schedule.rs` 1 712, `trigger.rs` 1 587, `store.rs` 1 419, `tool.rs` 1 040, `ceiling_gate.rs` 407, `manager.rs` 266, `mod.rs` 158, `test_fixtures.rs` 80) against upstream's 1 012-line `scheduled-runs.ts` @v0.68.0. All nine verbs are in the advertised `action` enum (`extension/tool/schema.rs:905-913`) AND dispatch (`extension/tool/routing.rs:1246-1339`), not merely present as strings: `ScheduledRunAction::from_wire` (`background/scheduled_runs/tool.rs:92-106`) is the guard and `every_scheduled_run_action_dispatches` (`extension/tool/scheduled_runs_tests.rs:101`) drives all nine through `Tool::execute` asserting none lands on the unknown-action arm. **The tick fires without a caller** — `ScheduledRunManager::ensure_tick_timer` (`manager.rs:167-191`) spawns the loop from `restore()`, armed on `SessionStart` at `extension/host/native_impl.rs:406` and disposed on shutdown at `:521`, pinned by `the_armed_tick_fires_a_due_schedule_with_nobody_asking` (`scheduled_runs_tests.rs:1365`). `AuthorityAction::ScheduleCreate` is consulted through the same three-arm gate as `stop`/`steer` (`routing.rs:1274-1304`); the six mutating verbs are refused from child-safe fanout (`:1260-1265`). **Verified by RUNNING, not reading:** `cargo nextest run -p cyrup-ext-subagents -E 'test(scheduled_run) or test(schedule) or …'` = **100 tests run, 100 passed**. Residual recorded in the section, not filed. PARITY-GAPS PB-11 should be re-read against this. |
| SUBA-017 | **still-open — evidence refreshed at `cc7818b` 2026-09-16** | Re-run, still zero: `git grep -c 'completion_batch\|batcher\|CompletionBatch' -- crates/cyrup-ext-subagents/src` returns **no matching file**. The old "18-field `SubagentExtensionConfig`" count is stale — it is **34** fields now, and none of them is `completion_batch`. `background/watch.rs` is now `background/watch/` (7 files); the sink is `watch/sink.rs` and `watch/install.rs:304` still hands it one message per result. Upstream re-confirmed at **v0.68.0**: `completion-batcher.ts` is 168 lines, wired at `extension/index.ts:495` (`registerSubagentNotify(pi, state, { batchConfig: config.completionBatch, … })`), with the key at `shared/types.ts:2663` and the grouping at `notify.ts:544/692/723/790`. In-baseline (`v0.43.0:…/completion-batcher.ts` exists), so `not-ported` stands. `09a`'s `SUBA-090` closure explicitly parked the grouped `formatGroupedCompletion` form on this row. |
| SUBA-018 | **closed** | `registration/prompt_workflows.rs` (831 lines); `/prompt-workflow` and `/chain-prompts` at `registration/slash_commands.rs:141-142`. |
| SUBA-019 | **closed** | `discovery/frontmatter.rs:301` `fold_block`, `:351` `parse_frontmatter_list` (block `- item` lists, preserving pi's absent-vs-empty distinction), used at `:780-799`; tests `:1677-1710`. **The LITERAL block-scalar half never came across** — new item **SUBA-052**. |
| SUBA-020 | **closed** | `exec/fallback.rs:559-575` defines the `TOOL_FAILURE_PREFIX` equivalent citing `model-fallback.ts:316-323`, short-circuited first inside `is_retryable_model_failure` (`:642-651`), matching pi's ordering at `:326`. |
| SUBA-021 | **misdescribed → still-open, severity raised to medium** | The "post-baseline, out of scope" framing is **dead**: `capability-ceiling.ts`, `usage-budget.ts` and `spawn-budget.ts` all pass `git cat-file -e` at **both** v0.43.0 and v0.47.1. `launch-contract.ts` is struck — it is absent at both tags at every path (it historically lived at `src/shared/launch-contract.ts`), so it was never in either baseline. Restated below; the grant half is **SUBA-046**. |
| SUBA-022 | **still-open — evidence refreshed at `cc7818b` 2026-09-16** | Re-run, unchanged: `ls crates/cyrup-ext-subagents/src` has **no `api/`** among its 23 entries, and `git grep -c 'prompt-template:subagent' -- crates/` = 0. Upstream re-confirmed at **v0.68.0**: `src/api/delegation.ts` exists (and at `v0.43.0`, so in-baseline), exporting the same five constants at `:6-10` — `SUBAGENT_DELEGATION_{REQUEST,STARTED,UPDATE,RESPONSE,CANCEL}_EVENT`, values `prompt-template:subagent:{request,started,update,response,cancel}`. Its header says the API deliberately reuses the existing extension-to-extension transport rather than adding a second protocol, which is the shape a cyrup port should follow over `cyrup-ext`'s `SharedBus`. Nothing in #137/#139/#140 touched it. |
| SUBA-023 | **partially-closed 2026-09-16** — *was* still-open | **The signal-attribution half is IN and the prior evidence line is false.** `TerminationOutcome` (`spawn/signal.rs:112-127`) now carries a third field, `signal_name: Option<&'static str>`, filled by `signal_name(i32)` (`:136-`) — and the doc at `:123-125` records the right design call: the name is derived from the OBSERVED status, not from `Self::stage`, so an external `kill`, an OOM kill or a `SIGSEGV` is not misattributed to the rung the ladder happened to be on. It has **production consumers**, not just a field: `exec/attempt_runner.rs:150` puts it on every attempt outcome through `process_signal_name` (`:787-791`), and `exec/external_cli/run.rs:403,569` uses the same single mapping. Two tests, both green: `spawn::signal::tests::a_killed_child_reports_its_signal_name_and_a_clean_exit_reports_none` and `exec::attempt_runner::tests::a_crashed_child_reports_the_posix_signal_name_not_a_number`. **~~Still open:~~ CLOSED 2026-09-20 — every clause of the sentence this cell carried is now false and is struck rather than softened.** It read *"`process_terminal` and `session_lease` remain zero-hit as SYMBOLS — and are now explicitly unported in two new call sites' docs, `background/active_async_capacity/inspect.rs:145-172` and `background/active_run_index.rs:56-59`, each substituting for the missing record."* Both modules exist (`background/session_lease/`, `background/process_terminal/`), `active_async_capacity/inspect.rs` reads the real proof as the first rung of its own artifacts with the pid ladder BENEATH it, and `active_run_index.rs` applies the `observed`-proof disjunct of the staleness rung. See this row's own closure body below and `PARITY-GAPS.md` VL-S3/VL-S4. The `Kind` correction the cell also owed (`not-ported`, since both upstream files exist at **v0.43.0 and v0.68.0**) was applied on closure. This half landed at or before the history root `da970e8` — **not** from #137/#139/#140 — and the `## Open items` row has recorded it as PARTIALLY CLOSED since 2026-08-14; only this table was stale. |
| SUBA-024 | **partially-closed — evidence refreshed at `cc7818b` 2026-09-16** | `task_intent` and the struck `chain_validation` are unchanged. `parallel_handoff` / `agent_contract` are still zero-hit as IMPLEMENTATIONS, and the three matches that now exist are all the code saying it does not have them: `background/async_retention/scan.rs:425` `[CYRUP-DELTA] RunStatus has no parallel_handoff field`; `exec/acceptance/lattice/inject.rs:51-54` and `spawn/chain_graph.rs:2047-2051` both hard-code `report_optional: false` because `isAgentContractV1` can never be true with `agent-contract.ts` unported — and both say so at the seam, which is the right shape. **One consumer-side slice DID land and it is worth naming so nobody re-derives it:** `has_unresolved_run_handoff` (`async_retention/scan.rs:423-440`) ports the `<run_dir>/handoff.json` reader in full — but its own doc says *"no cyrup writer produces the file today"*, so it is a reader with no producer and the row does not close on it. Upstream re-confirmed at **v0.68.0**: both `src/runs/shared/parallel-handoff.ts` and `src/runs/shared/agent-contract.ts` exist, and at **v0.43.0** too — so `upstream-drift` is the wrong kind here; it is `not-ported`. The blind spot below (chain pre-walk validation / `ChainStepConfig` unknown keys) is **still not read** — four passes now. |
| SUBA-025 | **still-open — severity raised to medium** | Restated below. |
| SUBA-026 | **partially-closed — evidence refreshed and one citation CORRECTED at `cc7818b` 2026-09-16** | The `as_str` table is now **17** variants at `registration/slash_commands.rs:129-149` (`/subagents-stop` at `:146`, `/subagents-guide` at `:147` — `SUBA-066` closed). Still absent: `subagents` (the admin surface) and any selector; `git grep -c 'subagents_admin\|SubagentsAdmin\|subagents-admin'` = 0 and `git grep -c 'selector\|Selector' -- …/src/tui/` = 0. **Correction — this row cited a path that exists at no tag.** `src/tui/selector.ts` fails `git cat-file -e` at v0.43.0 AND v0.68.0, and `git log --all -- src/tui/selector.ts` is empty; the real file is **`src/slash/selector.ts`** (147 lines @v0.68.0), present at both tags. `src/slash/subagents-admin.ts` is **460** lines @v0.68.0 (the recorded 432 was the v0.47.1 figure) and `subagents` registers at `src/slash/slash-commands.ts:869`. Both files exist at v0.43.0, so the `upstream-drift` kind is wrong — `not-ported`. PARITY-GAPS VL-S11 carries the same wrong selector path and should be corrected with it. |
| SUBA-027 | **closed** (513e45a) — re-audited adversarially | `exec/acceptance.rs:2346-2385` carries two real process-death regression tests: `a_timed_out_verify_command_is_killed_not_abandoned` (publishes a pid via `exec sleep 300` — the `exec` is load-bearing and the test says so — and asserts `wait_for_pid_gone`) and `a_timed_out_verify_command_kills_its_whole_process_group` (asserts the **descendant** pid dies). `cmd.process_group(0)` at `:6805`, group-kill doc at `:6853`. |
| SUBA-028 | **still-open** | Restated below. |
| SUBA-029 | **still-open** | `discovery/settings_write.rs:70-81` unchanged. |
| SUBA-030 | **still-open** | Both halves confirmed at HEAD. Restated below. |
| SUBA-031 | **still-open** | Restated below. |
| SUBA-032 | **still-open** | Restated below. |
| SUBA-033 | **still-open** | Restated below, with a corrected line citation for the second instance. |
| SUBA-034 | **closed** (sweep 11 — REFUTED / already-done, landed by `844e25f`) | The `CompletionBus` wake is at HEAD; see the Open-items row. |
| SUBA-035 | **still-open** | `rg 'model_scope\|modelScope' registration/doctor.rs` = 0 across all 1803 lines. Enforcement itself is live at `exec/model_scope.rs:170-188`. |
| SUBA-036 | **closed** (a340b56) | The dual implementation was collapsed: `ls src/exec/` shows a single `acceptance.rs` with no `acceptance/` submodule, and `exec::acceptance::model` is the **live** implementation — reached from `discovery/chains.rs:714`, `extension.rs:8555`, `exec/mod.rs:3527/:3561/:3588`, `exec/acceptance.rs:739/:1152-1160`, `spawn/chain_graph.rs:1614-1618`. No orphaned ~3000-line submodule remains. |
| SUBA-037 | **still-open** | Restated below. |
| SUBA-038 | **partially-closed** | The child-safe UNAVAILABLE text is now verbatim. **Three** residual texts remain — restated below. |
| SUBA-039 | **still-open** | Restated below. |
| SUBA-040 | **closed** | The weak `sleep 5` assertion is gone; `exec/acceptance.rs:2346-2385` asserts process death, which is exactly the strengthened assertion this item asked for. |
| SUBA-041 | **closed** (513e45a), residuals now also closed | Residuals SUBA-N03/N04/N05/N06 all closed this pass. |
| SUBA-N03 | **closed** | `extension.rs:14635-14685` is a table test over `artifacts`/`acceptance`/`control`/`includeProgress`/`timeoutMs`/`maxRuntimeMs`/`share`/`sessionDir` with `async:true`, asserting the message does **not** contain `only supported for foreground`, that it reaches agent resolution (`agent not found`), and that each name is an advertised property. That assertion is now the only crate-wide occurrence of the string. Async SINGLE threads `output_path`/`output_mode`/`skills`/`session_dir` at `extension.rs:2295-2310`. |
| SUBA-N04 | **closed** | `background/runner_main.rs:2375-2385` lowers `step.acceptance` through `exec::acceptance::lower_acceptance_input` and returns `StepResult::failure` with pi's `validateAcceptanceInput` message on an invalid policy, instead of degrading to `None`. The old hard-drop at `~:1734` is gone. |
| SUBA-N05 | **closed** | `extension.rs:2924-2928`: `let chain_dir = resolve_chain_dir(chain_dir_override, cwd, &foreground_run_id);`, resolver at `:6539-6540`, tests at `:12719-12733`. PB-13's separate complaint — that the **fallback** root is temp rather than the project — is untouched and stays open there. |
| SUBA-N06 | **closed** | `extension.rs:6640` inserts `control` via `sj_control_overrides()` and `includeProgress` sits at `:6628` between `artifacts` and `share`, matching pi's ordering; `include_progress` is threaded at `:1963`, `:2179`, `:2342`, `:2393`, `:2541`, `:2773`. |
| SUBA-S01 | **closed** | `exec/mod.rs:3248-3250` creates the capture runtime, `:1818-1832` writes `STRUCTURED_OUTPUT_SCHEMA_ENV`/`_CAPTURE_ENV` into the child env overlay, `:3389-3398` reads the capture **file** back (the fence-scrape survives only on the runtime-creation-failed arm); child side `prompt_runtime.rs:1613-1620` gates `StructuredOutputTool` on both vars, attached in production at `crates/cyrup/src/main.rs:489,638,732`. Item scope was "a chain step or fanout task with `outputSchema`" — the path now wired (`extension.rs:5849`). The **SINGLE-mode** residual is **SUBA-043**. |
| SUBA-S02 | **closed** | `exec/control.rs:464` `derive_activity_state`, `:1260-1293` `update_activity_state` driven from the live monitor (`:1293/:1399`), 12 transition tests at `:1841-2039`; sink + notice formatting wired at `extension.rs:1071-1130`; `ControlEvent` aggregated through chain runs at `spawn/chain_graph.rs:1055,1906`. A real producer, which is precisely what the item said was missing. |
| SUBA-S03 | **closed** | `background/runner_main.rs:302-330` (`timeout_ms`, `deadline_at_ms`, documented against `async-execution.ts:924/983` and `subagent-runner.ts:126`), `:1260-1270` arms the deadline with pi's `Math.max(0, deadlineAt-now)` conversion, `:1653-1661` fires `timeout_message`; external `control/timeout.json` verb consumed at `:1340`. The **missing default** is split out as **SUBA-051**. |
| SUBA-S04 | **closed** | `background/cascade.rs:1-31` (module doc naming the pi function), `:45-80` `CascadeVerb::{Interrupt,Timeout,Stop}` with literal `ancestor-interrupt`/`ancestor-timeout`/`ancestor-stop` sources, `:163` `cascade_to_nested_async_descendants`. cyrup also ported the `stop` twin the item did not ask for. |
| SUBA-S05 | **closed** | `prompt_runtime.rs` is a full port of `subagent-prompt-runtime.ts` — `rewrite_subagent_prompt`, `strip_parent_only_subagent_messages` (`:660-700`, reproducing the `isParentOnlySubagentMessage` filter, the `SUBAGENT_FANOUT_CHILD` gate at `:142`, `stripAssistantSubagentToolCallBlocks` and the "return None when nothing changed" contract at `:319`), `INHERIT_PROJECT_CONTEXT`/`INHERIT_SKILLS` env readers — attached in production at `crates/cyrup/src/main.rs:489,638,732`. One omitted sub-behaviour (`sanitizeToolIds`) was investigated and **refuted** as a finding; see `## Coverage`. |
| SUBA-S06 | **closed** | `exec/mod.rs:2826-2856`: a `ChildStep::Exited` arm arms `exit_drain_at = now + POST_EXIT_DRAIN_MS` and deliberately does *not* break (so buffered stdout is not dropped), with an `exit_drain_arm` at `:2852` that breaks the `select!` so the status flows through `wait_final_drain`. Reasoning cites SUBA-S06 and the surviving-grandchild hazard verbatim. |
| SUBA-S07 | **closed** | `spawn/mod.rs:459-464`: the `Err(err)` arm of `SpawnedChild::spawn` calls `cleanup_temp_files(&temp_files)` before returning, with the R-SA-067 rationale at `:426-434` explaining that `spec` is taken by value so this is the only cleanup opportunity. No other early return bypasses it. |
| SUBA-042 | **refuted — never filed** | Inherited tool-id sanitation. See `## Coverage` → rejected with reason. The ID is burned, not reusable. |
| SUBA-043 | **new this pass** | high · SINGLE-mode `outputSchema` unadvertised and hardcoded `None`. Residual of the closed SUBA-S01. |
| SUBA-044 | **new this pass** | medium · Bundled `reviewer` agent still grants `bash`/`edit`/`write`; upstream made the lane read-only. From the v0.43.0..v0.47.1 range. |
| SUBA-045 | **new this pass** | medium · Child tool-availability diagnostic unported. From sweep 1 (child env vars, `TOOL_DIAGNOSTIC_PATH`). |
| SUBA-046 | **new this pass** | medium · `grant-spawn-budget` unported *and advertised*. Depends on SUBA-064 for the authority gate. |
| SUBA-047 | **new this pass** | medium · `toolBudget` honoured but never advertised. Residual of the partially-closed SUBA-007. |
| SUBA-048 | **new this pass** | medium · `artifactDir` config key unported; `session`/`temp` unreachable. From sweep 3. |
| SUBA-049 | **partially-closed** (sweep 11) | Ack path + `mode` + capability landed; `steeringRecovery` handed back as L. |
| SUBA-050 | **new this pass** | medium · `subagents.modelScope.strict` unported. From the version-lag range. |
| SUBA-051 | **new this pass** | medium · Async child runs have no default wall-clock timeout. Split out of the closed SUBA-S03. |
| SUBA-052 | **new this pass** | medium · YAML literal block scalars parse to the literal `"\|"`. Split out of the closed SUBA-019. |
| SUBA-053 | **new this pass** | medium · `~` never expanded in chain read/write paths. |
| SUBA-054 | **partially-closed 2026-09-16** | medium · `defaultReads` reaches a FOREGROUND single run and not an ASYNC one. The foreground half is in and is the whole of the item's Verify: `extension/executor/foreground.rs:827` `reads: agent.default_reads.clone()`, consumed by `build_task_text` (`exec/spawn_plan.rs:1414-1426`) through the shared `spawn::chain_graph::build_single_reads_instruction` (`:775-796`), which carries `~`-expansion and the existence filter with it — so **SUBA-053 and SUBA-058 are discharged for this path**. Green: `build_task_text_prepends_the_default_reads_instruction_for_a_single_run` (`spawn_plan.rs:1991`). The async half is still `reads: None`, at TWO sites, and the second one states the blocker rather than defaulting silently (`background/runner_main/executor.rs:670-677`, `extension/executor/background.rs:253`). **This half was already true at `d53763b`, and the `## Open items` row has said PARTIALLY CLOSED since 2026-08-14** — it is only THIS table that was stale. No landing sha finer than the history root `da970e8` exists for it. |
| SUBA-055 | **closed** (sweep 11) | `guide` + `resources/docs/` landed; `children.list` returned to SUBA-005 with its reason. |
| SUBA-056 | **closed 2026-09-16** (`2bd76ac`, PR #137; third rung `e61ff44`, PR #139) | **The prior evidence line's `rg -c … = 0` is false at `cc7818b`.** `background/completion_replay/` is 5 files / 1 810 lines (`archive.rs` 548, `store.rs` 422, `retention.rs` 376, `record.rs` 301, `mod.rs` 163). Written from the LIVE terminal path, not a test: `background/wait_completions/record.rs:186-209` persists the record inside `WaitCompletionStore::record`, before the payload is unlinked, with the ordering pinned by `watch::install::tests::the_replay_record_is_written_before_the_payload_is_deleted` (`background/watch/install.rs:550`) — written precisely because a refactor that moved the persist into the spawned task would still compile. Read back as the third rung of `collect_wait_completions` (`background/wait_completions/collect.rs:41`), which `background/wait.rs:1226` calls on every wait, landing on `WaitOutcome::completions` and from there on the tool result's `details.completions` (`wait.rs:784-788`). The 64 KiB tail is char-boundary-correct through `exec::child_protocol::utf8_tail` (`completion_replay/archive.rs:292,319`) with `an_archive_over_64_kib_is_truncated_and_flagged` (`:366`) using a two-byte `é` so the cut lands mid-character. The archive's rung order (output artifact → session file → bounded tail → run `summary`) is at `archive.rs:227-320`, which is the item's second Verify clause. Retention sweep wired at `extension/executor/notices.rs` (`cleanup_completion_replay_if_due`) and a second production writer at `background/wait_subscriptions/manager.rs:1421`. |
| SUBA-057 | **new this pass** | medium · `dismiss` unported — a recovered workflow with no live controller is stuck "running". Owns one of SUBA-005's unowned verbs. |
| SUBA-058 | **new this pass** | low · Chain read instructions not filtered by existence. |
| SUBA-059 | **new this pass** | low · `artifactConfig.cleanupDays` parsed but never wired. |
| SUBA-060 | **new this pass** | low · "Resume-first" guidance for failed async runs unported. |
| SUBA-061 | **still-open on THREE keys — evidence refreshed at `cc7818b` 2026-09-16** | low · All four names are still zero-hit port-side (`git grep -c 'asyncWidget\|async_widget\|inlineToolDisplay\|inline_tool_display\|fleetKeybindings\|fleet_keybindings\|legacyChainControls\|legacy_chain_controls' -- crates/cyrup-ext-subagents/src` → no matching file), but **`legacyChainControls` LEAVES the item and stays struck: `git -C tmp/pi-subagents grep -c legacyChainControls v0.68.0 -- src` is EMPTY**, where v0.47.1 had it in five files — upstream deleted the key, so porting it now would port a surface that no longer exists. The `## Open items` row established this at `9aeba769` against v0.67.0; re-confirmed here at **v0.68.0**. Surviving three, all still at `shared/types.ts` @v0.68.0: `fleetKeybindings` `:2609`, `asyncWidget` `:2611`, `inlineToolDisplay` `:2617` — so the mixed kind is **2 in-baseline + 1 drift**. Port-side field count refreshed again: `SubagentExtensionConfig` (`registration/mod.rs:79`) is **34** fields (the `## Open items` row's 30, itself correcting this table's 18, has drifted by four more — `model_exclusions`, `max_active_async_runs_per_session`, `capacity`, `scheduled_runs`), still `#[serde(rename_all = "camelCase", default)]` with no `deny_unknown_fields`, so the three are still accepted and silently dropped. |
| SUBA-062 | **new this pass** | low · Bundled `researcher` cannot do web research; root cause hands off to areas 04/12. |
| SUBA-063 | **partially-closed 2026-09-16** | low · **Two of the three sub-parts landed. The `## Open items` row recorded both at `9aeba769` — (a)'s zero-hit grep REFUTED, (c) CLOSED — and only THIS table still read "unported"; the correction below is a re-verification at `cc7818b`, not a new finding.** (1) The zero-budget DECODE half is in: `TOOL_BUDGET_ZERO_AUTH_ENV = "CYRUP_SUBAGENT_TOOL_BUDGET_ZERO_AUTH"` (`exec/tool_budget.rs:39`) with `HardMinimum::{One,Zero}` and `from_env` (`:53-64`), read in production child-side at `prompt_runtime.rs:2332` inside `prompt_runtime_from_env`. (3) The events-cap override is in AND wired: `ASYNC_EVENTS_MAX_BYTES_ENV` (`background/runner_main/events.rs:14`), `resolve_async_events_cap_bytes` (`:24`), consumed at `:49-50` by the run's real `events.jsonl` writer. **Still open, and this is the sharp half: nothing WRITES the zero-auth variable.** `git grep 'TOOL_BUDGET_ZERO_AUTH'` over the whole workspace returns 12 hits, all in the two reader files; `exec/spawn_plan.rs:1173` writes `TOOL_BUDGET_ENV` and no sibling. So the child can honour `hard: 0` and no production path can ask it to — upstream writes it at `pi-args.ts:1032` from `input.allowZeroToolBudget`. With `SUBA-047` closed the caller surface now EXISTS (`toolBudget` advertised at `extension/tool/schema.rs:546`) and refuses `hard: 0` with *"toolBudget.hard must be an integer >= 1."* (`extension/tool/routing_tests.rs:604`), so the blocker that row carried is discharged and only the writer is owed. Also still open: the whole ack path (`RUNTIME_ACKNOWLEDGED` = 0 hits) and the truncation marker (upstream `TRUNCATED_EVENT_TYPE = "subagent.events.truncated"`, `subagent-runner.ts:320` @v0.68.0 / `:266` @v0.43.0 — in-baseline). Both landed parts were already true at `d53763b`; no landing sha finer than the history root `da970e8` exists for either. |
| SUBA-064 | **new this pass** | medium · The whole `authorityPolicy` subsystem unported; the `stop`/`steer` gate it drives is live-reachable. From sweep 4 (denial paths). |
| SUBA-065 | **new this pass** | low · `unknownSubagentActionMessage` did-you-mean recovery and its destructive-action gate unported. From sweep 4. |
| SUBA-066 | **closed** (sweep 11) | `/subagents-guide`, landed with SUBA-055. |
| SUBA-067 | **new — FIXED** | high · descendant-termination fixture exec-collapsed to one pid, so the test never exercised group-kill. Found by RUNNING the suite. |
| SUBA-068 | **new — FIXED** | high · setup-hook timeout fixture raced macOS's ~200 ms first-exec verification with a 200 ms budget. Found by RUNNING the suite. |
| SUBA-069 | **new — OPEN** | high · the whole setup-hook test family is wall-clock-budgeted and goes red under machine load; `-p cyrup-ext-subagents` is not a reliable gate. |
| SUBA-097 | **new 2026-09-16 — OPEN** | low · `background/scheduled_runs/trigger.rs:887` claims an integration suite that does not exist, over the one seam its unit tests stub. Found while verifying `SUBA-016`. |
| SUBA-098 | **new 2026-09-16 — OPEN** | low · A stray CJK token inside an English doc comment at `extension/tool/scheduled_runs_tests.rs:31`. Found while verifying `SUBA-016`. |

Closed this pass: **22**. Partially closed: **4** (SUBA-007, SUBA-013, SUBA-024, SUBA-026).
Newly filed: **24**. Refuted and recorded: **1**.

## Open items

> **RECOUNTED 2026-09-16 — authoritative over every block below, including the 2026-09-04 one.
> Counted set: 0 critical · 0 high · 1 medium · 9 low = 10 open** (`SUBA-005` stays under
> `## Trackers`, uncounted). The table carries **53 rows: 43 closed, 10 open** — the numbers are
> `scripts/count_open_items.py`'s, re-run after these edits, not hand-tallied. Two ids were added this
> pass (`SUBA-097`, `SUBA-098`), none renumbered, none deleted. **The open COUNT is unchanged at ten
> and that is a coincidence, not a no-op:** two rows left the set (`SUBA-016`, `SUBA-056` — both
> mediums) and two entered it (`SUBA-097`, `SUBA-098` — both lows found in the code while verifying),
> so the medium set went 3 → 1 and the low set 7 → 9.
> **The 2026-09-04 block's closing sentence — "The remaining mediums are `SUBA-016`, `SUBA-054`,
> `SUBA-056`" — is now wrong in two thirds.** `SUBA-016` is CLOSED (`7e41cf9`, PR #140) and
> `SUBA-056` is CLOSED (`2bd76ac`, PR #137 + `e61ff44`, PR #139); **the area's only remaining medium
> is `SUBA-054`'s async half, and it needs a decision (which cwd a runner step's reads resolve
> against), not effort.** Three further rows moved to partially-closed — `SUBA-023`, `SUBA-054`,
> `SUBA-063` — and their landed halves were already true at `d53763b`, so the 2026-09-14 provenance
> block's "none closed" is false; the reconciliation block at the top of this file has the greps.
> Still fully open and re-verified at `cc7818b` with their evidence lines refreshed: `SUBA-017`,
> `SUBA-022`, `SUBA-024`, `SUBA-026` (partial, as before), `SUBA-061`. `09a` has no open rows in its
> table but carries `SUBA-081`'s live residual, now filed there as **`SUBA-096`** — read that file's
> table too.
>
> **RECOUNTED 2026-09-04, batch 2 (ledger audit); superseded by the block above — counted set — counted set: 0 critical, 0 high, 3 medium, 8 low = 11 open** (`SUBA-005` stays under `## Trackers`, uncounted). The table carries **51 rows: 40 closed, 11 open** (`scripts/count_open_items.py`). One row moved: **`SUBA-072` CLOSED** at `7791b26a` (the per-attempt scratch dir now lives under the crate's run-scratch root, not the project tree; `bugs/SUBA-072-scratch-dir-project-scope.md` carries the detail; citations refreshed to HEAD in `b36c72e3`). The remaining mediums are `SUBA-016`, `SUBA-054`, `SUBA-056`. `09a` closed or partially closed its five carried mediums the same day and filed `SUBA-093`/`SUBA-094` from their residuals — read that file's table too.
>
> **RECOUNTED 2026-08-14 (sweeps 7-8 reconciliation, third edition) — counted set: 0 critical, 0 high, 8 medium, 9 low = 17** (`SUBA-005` remains under `## Trackers` and is not counted). The table carries **48 rows: 31 fully closed, 17 open (2 partially)**. Sweep 8 closed three: **`SUBA-008`** — the whole turn-budget subsystem, the top-ranked remaining medium, landed end to end; **`SUBA-030`** and **`SUBA-035`** as already-done, both of which had been *partially* closed for an edition with residuals that were in fact already in (and `SUBA-035`'s residual named the wrong file). Four rows were re-confirmed as **decisions rather than work** — `SUBA-025` (second decline), `SUBA-054`'s async half, plus `PERM-032`-style blockers routed elsewhere. *(Previous edition: 0 / 0 / 10 / 10 = 20, 30 closed.)*
>
> **THE AREA'S REMAINING SET IS GENUINELY LARGE AND IN-AREA, and it partitions cleanly — record this so sweep 9 does not re-derive it.** `SUBA-016` (`scheduledRuns` + nine `schedule.*` verbs; **L**, and the gate is free — `AuthorityAction::ScheduleCreate` is already pre-wired by `SUBA-064`); `SUBA-021` (`capability-ceiling` + `usage-budget`; **L** — **the spawn-budget third is already closed with `SUBA-046`, so any plan quoting "all three" is stale**); and the guide/replay/dismiss trio `SUBA-055`+`SUBA-056`+`SUBA-057` (**M/L/M** — but `SUBA-055`'s guide action additionally requires AUTHORING a `resources/docs/` set, the same invention hazard as `SUBA-025`, so it needs the same sign-off, and **`SUBA-066` is its last mile, not an independent item**).
>
> **UPDATED 2026-08-15 (sweep 10) — two of the three items the routing plan above called the area's
> remaining large set are now CLOSED, and one of them was blocked on a premise that was simply
> wrong.** `SUBA-021` landed both remaining halves (capability-ceiling + usage-budget, ~1030 new
> lines, consumed at `build_attempt_spawn_plan` and at `run_sync`'s settle respectively).
> `SUBA-025` landed after **refuting its own blocker**: the routing note said porting it required
> *"AUTHORING cyrup-specific compact and safety-guidance blocks — inventing model-facing text"*,
> and that is false. `extension/tool-description.ts` exists at **v0.34.0** — the tag cyrup's own
> `SUBAGENT_TOOL_DESCRIPTION` was ported from — and its COMPACT/SAFETY constants there are written
> around SINGLE/PARALLEL/CHAIN, so both came across byte-identical with nothing authored. **Two
> sweeps declined this item, and a third routed around it, because all three read only the
> v0.43.0 copy of the file.** The general lesson, worth carrying: *when an item is blocked because
> an upstream constant describes a subsystem cyrup has not ported, check whether an EARLIER tag
> has the same symbol written around the surface cyrup actually has* — `git log --oneline -- <path>`
> costs seconds and here it was the difference between a decline and a close.
> `SUBA-016` remains BLOCKED exactly as sweep 9 recorded it.
>
> **CORRECTED 2026-08-15 (sweep 9), because the paragraph above is a routing plan and two thirds of it are now wrong.** `SUBA-057` is **CLOSED** — and it was not an M: the read half was already at HEAD, so only the writer, the enum entry, the dispatch arm and the child-safe gate were owed. `SUBA-016` is **BLOCKED, not L**: "the gate is free" is true and irrelevant — the gate was never the cost. A schedule's only legal target is `workflowScript` at **both** baselines (`scheduled-runs.ts:38` @v0.43.0), and `workflowScript` is a 916-line `node:vm` JS sandbox (`workflows/scripted-workflow.ts:8,388,392`) this crate documents as unported at `extension.rs:5990-6020`. **Do not hand `SUBA-016` to an agent as ordinary work** — it will either invent a schedule target upstream deleted or stall. It needs an owner decision, exactly like `SUBA-025`.

> **SUPERSEDED — RECOUNTED 2026-08-14 (sweeps 3-6 reconciliation) — counted set: 0 critical, 0 high, 10 medium, 10 low = 20** (`SUBA-005` remains under `## Trackers` and is not counted). 28 rows are now marked CLOSED, including all five of the area's highs. **Sweep 6 closed six as REFUTED — every one was already closed at HEAD by a sweep between 3 and 5** (`SUBA-046`, `SUBA-028`, `SUBA-031`, `SUBA-039`, `SUBA-045`, `SUBA-060`) — and discharged two recorded blockers (`SUBA-021`'s spawn-budget third, `SUBA-030`'s `cli.rs` prerequisite). *(Previous edition: 0 / 0 / 14 / 12 = 26, 22 closed.)*
>
> **ONE OPEN ROW NEEDS A DECISION, NOT AN AGENT — `SUBA-025`.** Its Fix (port `toolDescriptionMode`, the 50 KiB file override and `withMandatorySafetyGuidance`) is mechanically portable, but **two of the three constants it selects between are not**: pi v0.43.0's FULL/COMPACT/SAFETY texts are written around `workflowScript`, which cyrup deliberately does not implement and documents as such (`extension.rs:5701-5716`), while cyrup's `SUBAGENT_TOOL_DESCRIPTION` is the v0.34.0-era SINGLE/CHAIN/PARALLEL text and correctly describes what cyrup can do. Landing the mechanism therefore requires **AUTHORING cyrup-specific compact and safety-guidance blocks — inventing model-facing text**, which is the class this directory blames for 29 of sweep 1's 32 fallout failures. Sweep 6 declined rather than invent it silently. It needs either owner sign-off on cyrup-original constants under a `CYRUP-DELTA`, or a decision to port `workflowScript` first.

> **AMENDED 2026-08-14 (documentation audit) — two rows added, `SUBA-070` and `SUBA-071`; the area's counted set becomes 0 critical, 0 high, 9 medium, 10 low = 19.** Both were found while writing user documentation for the subagents extension, and both were predicted by this area's own Coverage blind spots — 6 for the unwired class and 5 for `registration/profiles.rs` never being opened. A blind spot that names a file is a work item, not a disclaimer.

> **AMENDED 2026-08-18 — one row added, `SUBA-072`.** Filed from a real project checkout: after
> ordinary `/flux/*` subagent-tool use in this repo, `git status` on the project itself showed an
> untracked `.cyrup-subagent-scratch/` directory in the **working tree**. Full diagnosis + fix:
> [`bugs/SUBA-072-scratch-dir-project-scope.md`](bugs/SUBA-072-scratch-dir-project-scope.md). Note
> that file also clears `.cyrup-subagents/` (missions/artifacts) of any suspicion — that one is a
> correct, faithful port of pi-subagents' own project-scoped `.pi-subagents` default, not a bug.

> **RE-AUDITED 2026-09-04, cyrup HEAD `2571969`** (baseline for this pass `4fb5e40`, 210 commits
> ahead; combined 09/09a pass — see `09a-cyrup-ext-subagents-v0.57-drift.md` for that file's own
> closures this pass, nine confirmed and one partial). `git log --oneline 4fb5e40..HEAD -- crates/cyrup-ext-subagents`
> lists 25 non-merge commits; the ones that touch this area's own open items (as opposed to 09a's)
> are refactor/decomposition work (`extension.rs` → `extension/`, `exec/mod.rs` split into six files,
> `exec/acceptance.rs` split) that moves code without changing behaviour. **Every currently-open row
> in this file (`SUBA-005` tracker aside) was re-checked against the code at HEAD — by reading the
> actual current implementation, not by trusting that "nothing mentions it" — and none closed.**
> Specifically re-verified unchanged, each by grep-for-the-behaviour plus a read of the relevant
> function: `SUBA-016` (`schedule.*` still not in `SUBAGENT_ACTIONS`; the `DESTRUCTIVE_MANAGEMENT_ACTIONS`
> list carries the string `"schedule.delete"` but that is SUBA-065's verbatim did-you-mean table, not
> a dispatched verb — do not mistake it for progress), `SUBA-017`, `SUBA-022`, ~~`SUBA-023`'s residual~~
> (**CLOSED 2026-09-20** — `process_terminal` and `session_lease` are both real modules with
> production callers, so the zero-hit finding this list rested on is refuted), `SUBA-024`'s residual (`chain_graph.rs:1900`'s
> own comment still states "this crate has no agent-contract concept at all yet"), `SUBA-026`,
> `SUBA-049`'s `steeringRecovery` residual, `SUBA-054`'s async-cwd residual (still an explicit blocker
> comment in `background/runner_main.rs`, not a silent default), `SUBA-056`, `SUBA-061` (all four keys
> still zero-hit), `SUBA-063`, `SUBA-070` (`interactive` is parsed at `discovery/types.rs:1020` and
> `discovery/frontmatter.rs:847` but `grep -rn '\.interactive\b' src/exec/ src/extension/ src/spawn/`
> is still 0 — no consumer), and this file's own `SUBA-072` (`exec/mod.rs:757` is still
> `opts.cwd.join(".cyrup-subagent-scratch")`, byte-identical to the citation in
> `bugs/SUBA-072-scratch-dir-project-scope.md`). **One cross-file update**: `SUBA-085` in
> `09a-cyrup-ext-subagents-v0.57-drift.md` (filed and left open this pass) discharges
> `mission.resolve-decision` from `SUBA-005`'s unowned-verb list below — noted at that tracker row.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| ~~SUBA-014~~ | ~~**high**~~ **CLOSED 2026-08-14** | not-ported | S | `requireReadTool` unported — a skill-carrying agent can be told to `read` a skill it has no `read` tool for — **CLOSED 2026-08-14**: sweep 1 — the seam is now `exec::build_attempt_spawn_plan_with_read_requirement`; the 7-arg `build_attempt_spawn_plan` survives as pi's `requireReadTool: undefined` form so no external caller broke. |
| ~~SUBA-043~~ | ~~**high**~~ **CLOSED 2026-08-14** | not-ported | S | SINGLE-mode `outputSchema` is unadvertised and hardcoded `None` on both single paths — **CLOSED 2026-08-14**: sweep 1 — the schema/dispatch guard the Verify asked for already existed (`every_advertised_schema_property_is_read_outside_provided_keys`) and now covers `outputSchema` and `toolBudget` automatically — narrows blind spot 6. |
| ~~SUBA-008~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | **M/L** *(re-rated from M — see the body; sweep 6's "cheapest remaining medium … WIRING plus a schema key, not a port" is measurably wrong)* | `turnBudget` unported; the only consumers read a hard-coded `false` — **CLOSED 2026-08-14 (sweep 8): the whole assistant-TURN-budget subsystem ported end to end**, ~888 lines of new module (`crates/cyrup-ext-subagents/src/exec/turn_budget.rs`, verified at HEAD) plus a new drive-loop arm, a signal ladder, three new `SingleResult` fields, a frontmatter field, a serializer arm and a config key. **14 new tests, incl. two `cyrup-it` end-to-end tests against real OS subprocesses.** Closes PARITY-GAPS PB-10. **FOUR ERRORS IN THIS ROW'S OWN BODY ARE CORRECTED THERE — read them before reading the body as history; one of them would have shipped a non-functioning feature.** |
| ~~SUBA-016~~ | ~~medium~~ **CLOSED 2026-09-16** | not-ported | **XL, and BLOCKED** *(re-rated from L)* | `scheduledRuns` unported (+ **nine** `schedule.*` verbs, not four) — **BLOCKED 2026-08-15**: sweep 9 read `pi-subagents/src/runs/background/scheduled-runs.ts` at **both** baselines and found a hard prerequisite the item never recorded. **The verb count is CONFIRMED at nine** (`SCHEDULED_RUN_ACTIONS`, `:15-25` @v0.43.0 and HEAD; `shared/types.ts:2084` @v0.47.1) — the row was right and the body's "four" was stale. **But a schedule's only legal target is `workflowScript`**: `ScheduleTarget = { workflowScript: string }` (`:38` @v0.43.0, `:39` @HEAD) is the entire union, `sanitizeTarget` (`:319`/`:327`) refuses an `agent`/`task` target with *"schedule.create requires workflowScript. Use workflowScript: \"return runs.run('main', { agent, task })\"."*, and `parseScheduleTarget` (`:196`) refuses a persisted one as a *"removed legacy agent target"*. `workflowScript` is `pi-subagents/src/workflows/scripted-workflow.ts` — **916 lines whose mechanism is a `node:vm` JS sandbox** (`:8` `require("node:vm")`, `:388` `vm.createContext`, `:392` `new vm.Script`) — which this crate documents as entirely unported at `extension.rs:5990-6020` (*"the identifier appears nowhere in it"*). Porting the nine verbs against cyrup's `agent`/`task` shape instead would **invent a schedule-target union upstream deleted**, which is the mechanism-substitution the port rules forbid; porting them against `workflowScript` needs a **scripting-host decision** (and a Rust JS engine), which is an owner call, not agent work. **TWO ERRORS IN THIS ITEM'S OWN TEXT, corrected here:** its Verify recipe `{action:"schedule.create", agent, task, cron}` describes a shape upstream **explicitly refuses**, and names a **`cron` parameter that exists nowhere upstream** — the two triggers are `at` (one-shot, `parseScheduledRunTime` `:96`) and `every` (fixed interval, `parseScheduleInterval` `:125`), and `create` refuses calendar forms outright (`:459`). **NEEDS: owner sign-off on either (a) porting `workflowScript` first, or (b) a `[CYRUP-DELTA]` schedule target carrying `agent`/`task`, explicitly diverging from both baselines.** The SUBA-064 pre-wiring the row below advertises (`AuthorityAction::ScheduleCreate`) is real but is not the expensive half. — **RE-READ 2026-09-14 at `9aeba769` (verified re-audit, verdict still-open): the claim holds on both sides, but the row's `BLOCKED 2026-08-15` header and its whole blocking paragraph are now WRONG and must be rewritten.** The claim itself: nine `schedule.*` verbs are still undispatched and unadvertised — `extension/tool/text.rs:195-258` (`SUBAGENT_ACTIONS`, 39 entries, nothing beginning `schedule.`), `extension/tool/routing.rs:523-549` (`route_workflow_mode`), `registration/authority.rs:66-73`, `extension/tool/params.rs:45` — while upstream still ships all nine (`SCHEDULED_RUN_ACTIONS`, `src/runs/background/scheduled-runs.ts:19-30` and `:38-64` @v0.67.0, and the same nine at the tail of `SUBAGENT_ACTIONS`, `src/shared/types.ts:2760` @v0.67.0). `Kind: not-ported` re-confirmed by PRESENCE, not by date: `git cat-file -e v0.43.0:src/runs/background/scheduled-runs.ts` succeeds, so it is in-baseline. **THE BLOCKER HAS MOVED, NOT VANISHED.** `workflowScript` IS ported — `workflows/` is 19 modules, `extension/tool/schema.rs:337` advertises it, and `routing.rs:600-603` mints a real `RunMode::Workflow`. What the closing code actually DOES was audited rather than assumed: `routing.rs:544-549` refuses `async: true` OUTRIGHT and `:537` calls that refusal permanent, whereas upstream's `ScheduleRunRecord` (`scheduled-runs.ts:64-78` @v0.67.0) carries `asyncId`/`asyncDir` — a schedule fires a DETACHED run. So the residual prerequisite is **async/detached `workflowScript`**, not `workflowScript`. Residual size, previously recorded as unmeasured: `scheduled-runs.ts` is **1006 lines** @v0.67.0 — large enough that even `L` would be generous, so the recorded `XL` is not overstated. — **RE-READ 2026-09-14 at `d53763b` (post-#134 delta check, both sides): still open, and the residual prerequisite this row identified is UNCHANGED.** PR #134 (`e5a0789`, the `workflowScript` runtime surface) rewrote `extension/tool/routing.rs` and `extension/executor/workflow*.rs` without moving either limb: `SUBAGENT_ACTIONS` (`extension/tool/text.rs:195-258`, 32 entries) still advertises no `schedule.*` verb — `schedule.delete` appears only in `DESTRUCTIVE_MANAGEMENT_ACTIONS` (`:263-282`), carried verbatim on purpose — `scheduled_runs` is zero-hit crate-wide, and `routing.rs:545-557` still refuses `async: true` outright with `:545` calling that refusal *permanent* ("Async would buy only detachment, which this build does not implement for Workflow"), so the async/detached `workflowScript` a `ScheduleRunRecord` needs is still absent; #134's own new code writes the gap down at `extension/executor/foreground_actions/stop.rs:249` ("The `schedule.*` family is unported") and `extension/tool/text.rs:205-210` (`children.list` still withheld for want of retention). Upstream re-read at v0.67.0: all nine verbs still in `SCHEDULED_RUN_ACTIONS`, `src/runs/background/scheduled-runs.ts:19-30`. — **CLOSED 2026-09-16 at cyrup `cc7818b` (`7e41cf9`, PR #140), upstream re-measured at v0.68.0.** The blocker this row spent two passes narrowing was DISCHARGED, not refuted, and the narrowing was right: sweep 9 said the prerequisite was `workflowScript`; the `9aeba769` re-read corrected that to **async/detached** `workflowScript` because a `ScheduleRunRecord` carries `asyncId`/`asyncDir`. Both are now in. `background/scheduled_runs/` is 8 files / 6 669 lines against upstream's 1 012-line `scheduled-runs.ts` @v0.68.0. All nine verbs are advertised (`extension/tool/schema.rs:905-913`) **and** dispatched (`extension/tool/routing.rs:1246-1339`, guarded by `ScheduledRunAction::from_wire`), with `every_scheduled_run_action_dispatches` driving all nine through `Tool::execute` to prove no verb lands on the unknown-action arm — the advertise-vs-dispatch trap this row's `SUBAGENT_ACTIONS` evidence was watching for. The tick fires unasked (`manager.rs:167-191`, armed on `SessionStart` at `extension/host/native_impl.rs:406`, disposed at `:521`, pinned by `the_armed_tick_fires_a_due_schedule_with_nobody_asking`). `AuthorityAction::ScheduleCreate` — the SUBA-064 pre-wiring this row correctly called the cheap half — is consulted through the same three-arm gate as `stop`/`steer` (`routing.rs:1274-1304`). The two errors this row recorded in its own Verify (`cron`, `{agent, task}`) are preserved as refusals: `schedule.create` refuses a non-`workflowScript` target with upstream's sentence verbatim, and the triggers are `at`/`every`. **Verified by RUNNING:** `cargo nextest run -p cyrup-ext-subagents -E 'test(scheduled_run) or test(schedule) …'` = 100 tests run, 100 passed. Residual (`baseRef` accepted-and-refused where v0.68.0 honours it) is recorded in the body and deliberately NOT given an id — it lives in the unowned `v0.57.0..v0.68.0` window. **PARITY-GAPS PB-11 is the duplicate and is now stale.** |
| ~~SUBA-021~~ | ~~medium~~ **CLOSED 2026-08-15** | not-ported | L | `capability-ceiling` / `usage-budget` unported — both are **in-baseline** — **CLOSED 2026-08-15 (sweep 10): both remaining halves ported end to end.** **(a) `crates/cyrup-ext-subagents/src/exec/capability_ceiling.rs`** (~560 lines) — `normalizeCeiling`/`parseSubagentCapabilityCeiling`/`intersectSubagentCapabilityCeilings`/`resolveSubagentCapabilityCeiling`/`isAgentAllowedByCapabilityCeiling`/`assertAgentAllowedByCapabilityCeiling`/`capabilityCeilingAgentRestrictionMessage`/`…Sources`/`encode`/`decode` plus the per-session registry, vs `runs/shared/capability-ceiling.ts:12`/`:58`/`:65`/`:95`/`:106`/`:140`/`:159`/`:168`/`:172`/`:176`/`:183`/`:188`/`:192`/`:197` @v0.43.0, every refusal byte-identical. **CONSUMED, not merely present**: `exec/mod.rs`'s `build_attempt_spawn_plan_with_read_requirement` resolves the ceiling FIRST (before any argv/env) and refuses an out-of-ceiling agent through the new `SubagentError::CapabilityCeilingViolation`, then encodes it into `CYRUP_SUBAGENT_CAPABILITY_CEILING_V1` beside the `TOOL_BUDGET_ENV` encoder — **which is exactly where this item's Fix line said to put it** — so the bound tightens monotonically across the re-exec. Closes the env-var half of PARITY-GAPS VL-S1. **(b) `exec/usage_budget.rs`** (~470 lines) — `validateUsageBudgetConfig`/`validateLimit`/`usageBudgetState`/`metricState`/`usageBudgetExceededMessage` vs `runs/shared/usage-budget.ts:3`/`:14`/`:35`/`:44`/`:61`, threaded end to end: `usageBudget` is now an advertised tool param (`extension/schemas.ts:330`), validated at BOTH dispatch boundaries with upstream's text, carried on `SingleRunOverrides`/`BackgroundSingleRequest`/`BackgroundStepsSpec`/`RunnerConfig`/`ExecSingleStepExecutor`/`RunOptions`, and enforced at `run_sync`'s settle (pi `subagent-runner.ts:4403-4411`) — an exhausted budget becomes the run's `error` and is published on the new `SingleResult::usage_budget`. **Two `CYRUP-DELTA`s, both in-source**: the registry token is a `u64` not a JS `Symbol`, and the handle disposes on `Drop` (a Rust future can be dropped at any `.await`, so an explicit-only `dispose()` would strand a live ceiling). **11 new unit tests** plus two wiring tests that were red at HEAD (`the_capability_ceiling_refuses_an_out_of_ceiling_agent_and_reaches_the_child_env`, and the `usageBudget` block inside `subagent_tool_schema_exposes_the_full_pi_parameter_union`). **ONE ERROR IN THIS ROW'S OWN HISTORY, corrected:** the item's Fix said to consult usage-budget in `build_attempt_spawn_plan` too — upstream does not; `usageBudget` is a run-level, settle-time check (`subagent-runner.ts:4403`), never a spawn-plan input, and wiring it into the plan would have enforced nothing. — *(superseded)* ~~**RESTATED 2026-08-14**: sweep 6 — the `spawn-budget.ts` third is CLOSED with `SUBA-046`; what remains is the capability-ceiling half and the usage-budget half only.~~ |
| ~~SUBA-025~~ | ~~medium~~ **CLOSED 2026-08-15 — the blocking premise REFUTED** | not-ported | S | `toolDescriptionMode`, the file override, and the mandatory safety-guidance appender unported — **CLOSED 2026-08-15 (sweep 10). The two prior declines rested on a premise that is measurably false, and the refutation is one `git cat-file`: `src/extension/tool-description.ts` is present at **v0.34.0**, not only v0.43.0.** v0.34.0's `FULL_SUBAGENT_TOOL_DESCRIPTION` (`:17-66`) is the very text `extension.rs`'s `SUBAGENT_TOOL_DESCRIPTION` was ported from, and its `COMPACT_SUBAGENT_TOOL_DESCRIPTION` (`:68-88`) and `SUBAGENT_SAFETY_GUIDANCE` (`:9-15`) siblings are written around the SAME SINGLE/PARALLEL/CHAIN surface — they name `list/get/models/create/update/delete/eject/disable/enable/reset/status/interrupt/resume/steer/append-step/doctor`, every one of which cyrup dispatches today. Only **v0.43.0's** rewrite of those two constants is `workflowScript`-shaped, and v0.43.0 was never the revision cyrup's full description came from. **So NO model-facing text was authored: both constants are byte-identical v0.34.0 upstream, and the `full` arm is passed IN rather than duplicated, so the crate still has exactly one full description.** New `crates/cyrup-ext-subagents/src/registration/tool_description.rs` (755 lines) ports `resolveToolDescriptionMode` (`:104`), `customDescriptionPaths` (`:112`), `renderCustomTemplate` (`:121`, all eight placeholders, hand-scanned rather than regex), `loadCustomToolDescription` (`:143`, the 50 KiB `CUSTOM_TOOL_DESCRIPTION_MAX_BYTES` gate and all six warnings verbatim), `withMandatorySafetyGuidance` (`:180`) and `buildSubagentToolDescription` (`:191`). Wired at registration: new `SubagentExtensionConfig::tool_description_mode` (carried RAW like `turn_budget`, because upstream WARNS on a bad value rather than throwing), resolved in `init`'s Full arm and applied through the new `SubagentTool::with_description` — the `description` field is now `String` because a custom description's bytes come off disk. The ChildSafe arm is deliberately NOT resolved: upstream's fanout child builds its own literal (`extension/fanout-child.ts:159` @v0.34.0) and never calls the resolver. **ONE `[CYRUP-DELTA]`, in-source and mechanically enforced**: the compact text drops upstream's single *"Opt-in schedule actions: …"* bullet (`:80`) because `scheduledRuns` is unported (SUBA-016) and advertising four unroutable verbs is the SUBA-046 defect class; `the_compact_description_advertises_no_verb_cyrup_cannot_dispatch` asserts it rather than prose. **10 new tests**, one of which (`the_advertised_description_honours_the_configured_mode_and_the_file_override`, in `extension.rs`) is red at HEAD against the pre-existing `Tool::description()` surface. **ONE ERROR IN THIS ITEM'S OWN TEXT, corrected:** its Verify says the compact form must apply to *"both `subagent` and `wait`"* — upstream calls `buildSubagentToolDescription` for the `subagent` tool ONLY (`extension/index.ts:458` @v0.34.0, `:540` @v0.43.0); `wait` carries its own literal (`:512`), so applying the mode to it would have been a divergence. — *(superseded)* ~~**NOT TAKEN by a SECOND sweep (2026-08-14, sweep 8) … NEEDS: owner sign-off on cyrup-original constants under a `CYRUP-DELTA`, or a decision to port `workflowScript` first.**~~ |
| ~~SUBA-028~~ | ~~medium~~ **CLOSED 2026-08-14 — REFUTED** | parity-bug | S | Acceptance verification cannot be aborted — **REFUTED, CLOSED 2026-08-14**: sweep 6 — closed at HEAD. **The sweep-2 decline paragraph that stood here is deleted: the 40+ call sites of churn it was based on were AVOIDED, not paid.** `exec/acceptance.rs` now has BOTH `evaluate_acceptance` (`:1366`, the unchanged 7-arg entry) and `evaluate_acceptance_with_cancel` (`:1416`), so the cancellation seam exists without breaking the entry shape. vs pi-subagents v0.43.0 `runs/shared/acceptance.ts:1073`, `:1181-1182`, `:1290`, `:1295`. |
| ~~SUBA-030~~ | ~~medium~~ **CLOSED 2026-08-14 — already-done** | parity-bug | S | Persona inline on argv; task spill written with the default umask under a doc asserting 0600 — **CLOSED 2026-08-14**: sweep 8 read the crate at HEAD and found the persona/E2BIG residual — the half sweep 6 called "now ordinary in-crate work" — **already closed**. `spawn/mod.rs:262-289` `ChildSpawnSpec::resolve_system_prompt_arg` writes the composed persona to a `0600` `<stem>.md` in the run scratch dir via `write_private`, with `sanitize_prompt_file_stem` reproducing upstream's `[\w.-]`→`_` rule (pi `runs/shared/pi-args.ts:570-585` @v0.43.0 — note upstream spills **unconditionally**, unlike the task spill at `:588`, so this is the literal mechanism and not a large-persona fallback). **And it is CALLED:** `exec/mod.rs:1802` pushes the flag and the PATH as two argv elements, with an in-source block at `:1783-1801` naming **both** halves the item filed — the `/proc/<pid>/cmdline` disclosure and the `MAX_ARG_STRLEN`/`E2BIG` spawn failure. The 0600 task spill closed earlier and is pinned by `spawn/mod.rs:1197`. **Superseded partial-closure text follows.** — ~~**PARTIALLY CLOSED 2026-08-14**~~: sweeps 1 + 2 closed the task-spill 0600 half. **PREREQUISITE DISCHARGED 2026-08-14 (sweep 6): the row's hard blocker — "`crates/cyrup/src/cli.rs` must accept a path form for `--system-prompt` before an over-threshold persona can be spilled to a file" — is DONE.** `cli.rs`'s module doc at `:8` states it reads the `--system-prompt`/`--append-system-prompt` token to decide path-vs-literal, with `resolve_prompt_input` at `:419`/`:451`. **The persona/E2BIG residual is now ordinary in-crate work for area 09, not a cross-area dependency.** |
| ~~SUBA-031~~ | ~~medium~~ **CLOSED 2026-08-14 — REFUTED** | parity-bug | M | `wait` scopes runs by cwd, not by session, and says "in this session" — **REFUTED, CLOSED 2026-08-14**: sweep 6 — closed at HEAD. `background/wait.rs:42-55` no longer states the delta; it carries a `# Scoping (SUBA-031)` section recording that the session filter landed via `WaitDeps::session_id` as the INNER partition under the cwd-derived `async_root`, and explicitly records that **this is what makes the "in this session" empty-set string true** — the exact contradiction the item was filed on. The in-tree delta comment the item quotes verbatim no longer exists. vs pi-subagents v0.43.0 `subagent-wait.ts:265` (`sessionId: deps.state.currentSessionId ?? undefined`). |
| ~~SUBA-032~~ | ~~medium~~ **CLOSED 2026-08-14** | test-defect | S | Notice-debounce test asserts a wall-clock outcome with ~15 ms margin — **CLOSED 2026-08-14**: sweep 1 — `tokio` gained a `test-util` dev-feature in this crate's Cargo.toml, so `start_paused`/`advance` are available to any other wall-clock-marginal test here. |
| ~~SUBA-044~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | S | Bundled `reviewer` agent still grants `bash`/`edit`/`write`; upstream made the lane read-only — **CLOSED 2026-08-14**: sweep 1 — including the second correction (the delegate/worker strict-allowlist paragraph, in-baseline at v0.43.0). All six bundled agents now diff clean against v0.47.1 except researcher.md, whose divergence carries a `[CYRUP-DELTA]` header per SUBA-062. |
| ~~SUBA-045~~ | ~~medium~~ **CLOSED 2026-08-14 — REFUTED** | not-ported | M | Child tool-availability diagnostic unported — a silently missing tool reports nothing — **REFUTED, CLOSED 2026-08-14**: sweep 6 — closed at HEAD. `exec/tool_availability.rs` exists and `TOOL_DIAGNOSTIC` is threaded through `exec/mod.rs` and the child-side `prompt_runtime.rs`, which is the whole item. |
| ~~SUBA-046~~ | ~~medium~~ **CLOSED 2026-08-14 — REFUTED** | not-ported | M | `grant-spawn-budget` unported *and advertised*, so an exhausted cap is terminal for the session — **REFUTED, CLOSED 2026-08-14**: sweep 6 — closed at HEAD by a sweep between 3 and 5. `crates/cyrup-ext-subagents/src/exec/spawn_budget.rs` exists; `extension.rs:805-870` carries `reserve_subagent_spawns` on `preflight_spawn_budget`, a `spawn_budget_snapshot` accessor for the refusal details, and `grant_subagent_spawn_budget`; the verb is in the advertised action list at `extension.rs:5543`; `route_grant_spawn_budget` exists with its first gate documented at `:5491`. **The dependency this row records is DISCHARGED: the SUBA-064 authority consult is live** (`registration/authority.rs:42`/`:55`/`:83`/`:114`/`:130`/`:245`, `AuthorityAction::SpawnBudgetGrant` → `"spawnBudgetGrant"`). vs pi-subagents v0.43.0 `runs/shared/spawn-budget.ts:30`/`:50`/`:55`/`:59`/`:73`/`:85`/`:107`, `shared/types.ts:1885`. **`SUBA-021`'s spawn-budget third closes with it.** |
| ~~SUBA-047~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | S | `toolBudget` honoured but never advertised — **CLOSED 2026-08-14**: sweep 1 — the top-level half; SUBA-007 becomes fully closed. Residual filed: the PER-ITEM `toolBudget` override on `tasks[]`/`chain[]` (schemas.ts:148, :178) is still unadvertised and unconsumed, deliberately, because a `SingleStepSpec`/`RunnerStep` field is needed first and advertising without a consumer is the defect class. |
| ~~SUBA-048~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | S | `artifactDir` config key unported — `session` and `temp` are unreachable — **CLOSED 2026-08-14**: sweep 1 + 2 — sweep 1 REWROTE the Impact (its central claim was measurably wrong: both run-artifact sites called `artifacts::temp_artifacts_dir(cwd)` directly, so runs already landed in temp, NOT in `<cwd>/.cyrup-subagents` — the defect was the inverse, an unreachable `project` default). Sweep 2 closed it: new `artifacts::resolve_chain_runs_dir` (pi `getChainRunsDir`), and both run-artifact sites — `run_foreground_impl` and `spawn_background` — routed through `artifacts::resolve_artifacts_dir(session_file, Some(cwd), cwd, cfg.artifact_dir_preference())`. **Closes PARITY-GAPS PB-13 with it, as PB-13's own text instructs**: `project_chain_runs_dir` had ZERO references crate-wide before this. |
| ~~SUBA-049~~ | ~~medium~~ **PARTIALLY CLOSED 2026-08-15** | not-ported | M | Steer ack, delivery `mode` and `steeringRecovery` unported — a steer is fire-and-forget — **THE ACK PATH AND `mode` ARE CLOSED 2026-08-15 (sweep 11); `steeringRecovery` is handed back with its measured size.** The channel is no longer one-way. **New in `background/control.rs`** (~330 lines): `SteerDeliveryMode` (+`parse`/`as_str`/`next`), `SteerDeliveryStatus`, `SteerAckState`, `SteerAck`, `SteerCapability`, `MAX_STEER_QUEUE_SIZE = 20`, `steer_capabilities_dir`/`steer_capability_path`/`steer_acks_dir`/`steer_ack_file_name`/`steer_ack_write_path`/`write_steer_ack_at`/`write_steer_ack`/`write_steer_capability_at`/`read_steer_capability`/`consume_steer_acks`/`take_steer_acks`, plus `SteerRequest.mode` and `request_async_steer_with_mode` (which RETURNS the minted id — the correlation key the wait needs). vs pi `runs/background/control-channel.ts:63-104,154-253,304-343,396-415` @v0.43.0, every path and file-name format ported including the `(ts, state-order, state)` ack name that makes a `queued`-then-`delivered` pair replay in lifecycle order. **Parent env** — `RunOptions::steer_ack_dir`/`steer_capability_path` → `CYRUP_SUBAGENT_STEER_ACK_DIR`/`CYRUP_SUBAGENT_STEER_CAPABILITY` in `build_attempt_spawn_plan` (pi `pi-args.ts:101-102,764-768`), derived by `runner_main.rs`'s new `steer_ack_dir_for`/`steer_capability_path_for` off the SAME flat index as `steer_inbox_for`, so the request hop and the answer hop cannot address different children. **Child** — `SteeringInbox` gained `acknowledge`, `publish_capability`, `on_turn_start`, `on_turn_end`, an async `dispose` and a follow-up queue; it now writes exactly one ack per consumed request on EVERY path (delivered / queued / failed-no-host / failed-queue-full / failed-inject), publishes a capability record carrying its own pid, subscribes to `TurnStart`, and honours `mode` — `follow_up` parks and is released ONE per turn boundary, `auto` parks only mid-turn. **Parent wait** — `control_steer` takes `mode`, validates it with its own refusal, and polls `take_steer_acks` for 3 s (pi's `ackTimeoutMs ?? 3_000`) before answering pi's `Steering {delivered\|queued\|failed\|pending} for async run {id} (request {rid}).` **Closes `fleet.rs` delta 1 as a side effect**: the FleetView `Tab` mode cycle was logged and then dropped, and the file carried a SECOND `SteerDeliveryMode` enum for want of an argument to pass it to — that duplicate is deleted and re-exported from the wire type. **6 new tests** in `src/tests/steer_delivery_integration.rs` incl. both halves of the item's own Verify, and **ONE PRE-EXISTING TEST WAS CORRECTED**: `a_failed_injection_returns_the_undelivered_guidance_to_the_inbox` required the FAILED request to be re-delivered; upstream writes back only `requests.slice(index + 1)` (`subagent-prompt-runtime.ts:390-391`) and acknowledges the failed one instead — retrying it now would deliver it late and out of order behind newer guidance. **RESIDUAL, sized rather than stubbed: `steeringRecovery`.** It is not a parameter, it is a subsystem — `runs/background/steering.ts` (steering status, per-target state, `claimSteeringRecovery`/`remainingSteeringRecoveryLimits`/`waitForSteeringAction`/`actionResultFromSteeringStatus`) plus the pause-and-revive half of `runs/foreground/async-steering-action.ts:135-215` (interrupt, confirmed-pause poll, late-ack reconciliation, `resolveAsyncResumeTarget`, replacement launch, `subagent.steering.notice` events) — **~450 upstream lines and a new `status.json` sub-record**, i.e. L, not the M this row was filed at. It is deliberately NOT advertised on the schema: an unconsumed `steeringRecovery` boolean is exactly the advertised-and-inert defect class this area keeps filing (`SUBA-047`, `SUBA-054`, `SUBA-061`). **SWEEP-11 REVIEW FIX (adversarial verification):** the async port of `SteeringInbox::flush` reproduced upstream's `try` but not its `finally`. Upstream's `flush` is a SYNCHRONOUS `(): void` whose body is wrapped in `try { … } finally { flushing = false; }` (`subagent-prompt-runtime.ts:381-413`), so its re-entrancy latch cannot outlive the call; cyrup's is `async`, awaits at `consume_steer_requests_from_dir`, at every `acknowledge` and at the write-back, and is driven both from the poll task and from the turn-lifecycle handlers — so a dropped future latched `flushing = true` PERMANENTLY, every later flush took the early return, and the inbox went silently deaf for the rest of the run (requests keep being consumed off disk but never acknowledged, so the parent's `await_steer_ack` only ever times out to `pending`). Fixed with a `FlushGuard` RAII type that clears the latch in `Drop`, pinned by `prompt_runtime.rs::a_dropped_flush_future_does_not_wedge_the_steering_inbox` — verified RED before the guard (polls `flush` once, drops it, then asserts both the latch and that a following flush still drains the inbox) and green after. Also corrected the PRE-EXISTING `extension.rs::steer_action_writes_a_control_inbox_request_for_a_running_run`, which still pinned the cyrup-original "Steering queued … Delivery requires a live Cyrup child session" text this item replaced with upstream's own sentence. |
| ~~SUBA-050~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | S | `subagents.modelScope.strict` unported — inherited/fallback out-of-scope models can only warn — **CLOSED 2026-08-14**: sweep 1. |
| ~~SUBA-051~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | S | Async **child** runs have no default wall-clock timeout; upstream bounds them at 30 minutes — **CLOSED 2026-08-14**: sweep 1. |
| ~~SUBA-052~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | S | YAML literal block scalars (`\|`, `\|-`) parse to the literal string `"\|"` — **CLOSED 2026-08-14**: sweep 1 — the parser half. The second sentence (route `parseSkillDescription` through the shared parser, as `a4fc59a` did) is NOT done and is cross-crate: skill descriptions come from `cyrup_resources::Skill.front.description`, not from this crate's parser. Filed against the resources area. |
| ~~SUBA-053~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | S | `~` never expanded in chain read/write paths — **CLOSED 2026-08-14**: sweep 1. |
| SUBA-054 | medium — **PARTIALLY CLOSED 2026-08-14** | upstream-drift | M | `defaultReads` never reaches a single run — no `[Read from: …]` outside chains — **PARTIALLY CLOSED 2026-08-14**: sweep 2 — the FOREGROUND SINGLE half is closed, which is the item's headline and its whole Verify recipe: `RunOptions::reads` (the declared, unfiltered list, pi's `reads` binding) populated from `agent.default_reads`, and `build_task_text` prepending `[Read from: …]\n\n` through two new shared helpers in `spawn/chain_graph.rs` (`resolve_existing_read_paths`, `build_single_reads_instruction`); `build_chain_instructions` was refactored onto the same helper so the two paths cannot drift. The separator difference is deliberate and both forms are upstream's. **FIX LINE CORRECTED: "add the `reads` param on the async path" — upstream has NO top-level `reads` param.** `extension/schemas.ts`'s `SubagentParamProperties` has no such key and the three `reads` entries at :144/:174/:204 are all per-ITEM, so `defaultReads` is the entire SINGLE precedence chain and no new advertised param is owed. **RESIDUAL, and the blocker is written into `background/runner_main.rs` at the field rather than silently defaulted: the ASYNC half needs a decision — a runner step already gets its read line from `build_chain_instructions` resolved against the CHAIN dir, while upstream's async single resolves against `effectiveCwd`, so setting `RunOptions::reads` there would double-emit.** **RE-CONFIRMED UNCHANGED 2026-08-14 (sweep 8): this needs a DECISION on which cwd an async single step's read instruction resolves against — closing it means teaching the step builder which of the two cwds applies. It is not agent work as filed** (`pi-subagents/src/runs/background/async-execution.ts:1300-1302` @v0.43.0). The SUBA-044 interaction is moot — reviewer.md no longer carries `defaultReads`. — **RE-READ 2026-09-14 at `d53763b` (post-#134 delta check, both sides): still open, unchanged.** #134 touched nothing under `exec/`, `background/`, `registration/`, `discovery/` or `spawn/` (`git diff --name-only b28d3ff..d53763b` over those five trees is empty), so the async half is exactly as filed: `background/runner_main/executor.rs:663-670` still pins `reads: None` with the double-emit blocker stated at the field, and `runner_main/mod.rs:124` likewise. Upstream re-read at v0.67.0 — the SINGLE precedence chain survives the file's growth: `src/runs/background/async-execution.ts:1669-1671`, `const reads = params.reads !== undefined ? params.reads : agentConfig.defaultReads ?? false;` (the row cites the same code at `:1300-1302` @v0.43.0). — **RE-READ 2026-09-16 at `cc7818b` (post-#137/#139/#140): STILL PARTIALLY CLOSED, unchanged in substance, and this row remains the area's ONLY open medium.** The foreground half is intact and green — `extension/executor/foreground.rs:827` `reads: agent.default_reads.clone()`, `build_task_text` at `exec/spawn_plan.rs:1414-1426`, `build_task_text_prepends_the_default_reads_instruction_for_a_single_run` passing. The async half is still pinned `None` at two sites, with the double-emit blocker stated at the field — citation drift only: `background/runner_main/executor.rs:670-677` (was `:663-670`) and `extension/executor/background.rs:253` (the `SingleStepSpec` constructor; the old `runner_main/mod.rs:124` citation no longer resolves). Upstream re-read at **v0.68.0**: the SINGLE precedence chain survives unchanged. **The decision this row needs has not been made and no pass should close it without making it.** |
| ~~SUBA-055~~ | ~~medium~~ **CLOSED 2026-08-15** | upstream-drift | M | The `guide` action and its packaged version-matched docs unported — **CLOSED 2026-08-15 (sweep 11), together with its last mile `SUBA-066`.** New `crates/cyrup-ext-subagents/src/registration/guide.rs` ports `SUBAGENT_GUIDE_TOPICS` (`extension/subagent-guide.ts:5-16` @v0.47.1, all ten in upstream's order), `isGuideTopic` (`:22`) and `readSubagentGuide` (`:26-38`) with the unknown-topic sentence byte-identical — *"Unknown subagents guide topic '<t>'. Valid topics: … . No files were changed."* — returned as ORDINARY TEXT, not an error, exactly as upstream returns it, because the caller is a model and an error costs it a turn. `guide` added to `SUBAGENT_ACTIONS` in pi's own position (after `models`, `shared/types.ts:1968` @v0.47.1), dispatched from `route_action`, with `topic` advertised as pi declares it (`schemas.ts:281`: a bare optional string, **no enum and no description** — reproduced including the absence, since an invented description would be cyrup-original model-facing text). **The docs are cyrup's own, describing cyrup's surface** — a new `resources/docs/` set (README + nine topics, ~35 KB) written from `docs/guide/extensions/subagents.md` and the crate's real surface, NOT pi's 130 KB `docs/` tree, which documents verbs and config keys this build does not have. **ONE `[CYRUP-DELTA]`, in-source**: `include_str!` instead of `fs.readFileSync` off a resolved `packageRoot`, which makes the version-matching STRUCTURAL rather than conventional (upstream's on-disk docs can be edited after install and drift from the binary; these cannot) and removes upstream's only throw, so the function returns `String` rather than `Result`. **A missing doc file is now a compile error.** **4 new tests**, one of which — `the_tool_reference_topic_names_every_dispatched_verb` — pins the packaged tool-reference page against `SUBAGENT_ACTIONS`, so a later verb added without documenting it goes red. **RESIDUAL: `children.list`**, the second verb this row claimed. NOT ported and the reason is recorded on the enum rather than left to inference: upstream's `children.list` lists RETAINED children (completed single runs held open under a `parentWorkflowRunId`, `subagent-executor.ts:4993-5000` @v0.47.1), which is part of the unported `workflowScript` shape — cyrup has no `parentWorkflowRunId` and no retained-child concept, so the verb would advertise a listing that is always empty. It returns to `SUBA-005`'s unowned-verb list. — **RESIDUAL CLOSED 2026-09-19** (`.flux/done/CHILDREN_LIST_AND_DEBUG_RUN.md`): `children.list` is advertised at pi's index and dispatched from `route_action` via the new `background/retained_children.rs` (upstream `retained-children.ts` @v0.68.0). The "always empty" premise was stale: a workflow's settled children ARE retained on disk, as step rows of the workflow's own `status.json` (`WorkflowRunHost::publish_steps` + the terminal write), which is what the listing reads — `RunStatus` still has no `parentWorkflowRunId` (that is by design; a cyrup workflow child is foreground and is addressed as `(workflow id, step index)`, which is how the resume hint names it). One residual stays open with a true premise: no producer writes a recovery descriptor for a workflow launch, so in production every row lists `not resumable (missing recovery descriptor)` — pi's reason sentence; `resume` refuses the same absence with `RecoveryDescriptorError::Missing`'s own, different sentence (`recovery_descriptor.rs`, raised at `extension/executor/control.rs`) — until a per-child descriptor location is chosen. |
| ~~SUBA-056~~ | ~~medium~~ **CLOSED 2026-09-16** | upstream-drift | L | Durable completion replay and output archives unported — **RE-READ 2026-09-14 at `d53763b` (post-#134 delta check, both sides; note this row was NOT part of the `9aeba769` re-audit): still open on its headline half.** #134 touched nothing under `background/`, and the durable half is still unported: `completion_replay`/`output_archive` are zero-hit as implementations and `background/wait_completions/collect.rs:105` says so in source — *"this build has no durable-replay reader yet: until SCOPE_4 lands `read_completion_replay`"*. **One evidence line of this row is now stale and is recorded rather than acted on:** the `wait-completions.ts` half IS ported (`background/wait_completions/{mod,project,record,collect}.rs`, landed pre-window in `c55a990` and present at `b28d3ff`, so not attributable to #134), which refutes the row's `rg -c '…\|wait_completion' = 0`; the replay + archive half is untouched. Upstream re-read at v0.67.0: `src/runs/background/completion-replay.ts` is present at 287 L — `expiresAt` `:36`, `completionReplayPath` `:45`, `completionArchivePath` `:49`, `writeCompletionArchive` `:73`, TTL stamp `:202`. — **CLOSED 2026-09-16 at cyrup `cc7818b` (`2bd76ac`, PR #137; the third `collect_wait_completions` rung at `e61ff44`, PR #139), upstream re-read at v0.68.0.** The in-source promissory note this row quoted at `9aeba769` — *"this build has no durable-replay reader yet: until SCOPE_4 lands `read_completion_replay`"* — is discharged: SCOPE_4 landed it. `background/completion_replay/` is 5 files / 1 810 lines. The write is on the LIVE terminal path, not a test seam: `background/wait_completions/record.rs:186-209` persists inside `WaitCompletionStore::record` before the payload is unlinked, with the ORDER (not merely the outcome) pinned by `watch::install::tests::the_replay_record_is_written_before_the_payload_is_deleted` — written, its own comment says, because a refactor moving the persist into the spawned task would still compile and still "record the completion". Read back as the third rung of `collect_wait_completions` (`wait_completions/collect.rs:41`), called by `background/wait.rs:1226` on every wait and landing on `details.completions` (`wait.rs:784-788`) — the structured-result half this row asked for. The Fix's named hazard was taken: the 64 KiB tail goes through `exec::child_protocol::utf8_tail` (`archive.rs:292,:319`) with `an_archive_over_64_kib_is_truncated_and_flagged` cutting a two-byte `é` mid-character. Archive rung order (output artifact → session file → bounded tail → run `summary`) at `archive.rs:227-320` satisfies the second Verify clause. The Fix's sequencing ("after SUBA-034") held. This row's own stale-evidence note stands corrected in full: `wait_completion` was already ported, and now the replay half is too. |
| ~~SUBA-057~~ | ~~medium~~ **CLOSED 2026-08-15** | upstream-drift | M | `dismiss` unported — a recovered workflow with no live controller is stuck "running" forever — **CLOSED 2026-08-15**: sweep 9 — **the READ half was already at HEAD and only the PRODUCER was missing.** `RunStatus::display_dismissed_at`, `ReconcileAction::DisplayDismissed`, `list_active_runs`'s `continue` and `format_display_dismissed_status`'s `State: display-dismissed` report all existed, and `background/mod.rs:933` already `[]`-linked a `SubagentExecutor::control_dismiss` that **did not exist** — a broken intra-doc link over a field with zero writers. Landed: `SubagentExecutor::control_dismiss` (`extension.rs:4222`) porting all five refusals in upstream's order with byte-identical sentences, the result-file re-reconcile, the stamp + `write_atomic_json`, the post-write re-reconcile (testing `ReconcileAction::DisplayDismissed` as the carrier of upstream's `status: null`, per that variant's own doc contract) and the `JobTracker::untrack` eviction; `dismiss` added to `SUBAGENT_ACTIONS` at pi's own position (between `stop` and `append-step`, `shared/types.ts:2084` @v0.47.1) and to `route_control_action`; the child-safe refusal (`subagent-executor.ts:5865-5870`, `dismiss` ∈ `MUTATING_MANAGEMENT_ACTIONS` `:175`) spelled on the arm rather than by widening `discovery::management::MUTATING_MANAGEMENT_ACTIONS`, which gates a different dispatcher; `background::control::read_status_file` raised to `pub(crate)` so the refusals judge the RAW record as pi's `readStatus` does. **4 new tests**, incl. the item's own Verify end to end through a real tool call. **TWO `[CYRUP-DELTA]`s recorded on `control_dismiss`**: (a) upstream's `status.mode !== "workflow"` half is NOT ported — cyrup's `RunMode` (`background/mod.rs:242`) has no `Workflow` variant because that fourth `SubagentRunMode` member (`shared/types.ts:231`) belongs to the unported `workflowScript` shape, and adding a variant nothing constructs would make the gate refuse every run and leave the verb unreachable; only the `!status` half is ported. (b) upstream's `state.workflowControllers.has(runId)` is an in-process `AbortController` map; cyrup drives every background run from a **detached process**, so the live-controller test is a zero-signal liveness probe of the recorded pid — which also identifies cyrup's exact analogue of the reload-orphaned run the verb exists for: a `Running` status with `pid: None`, which `reconcile`'s step 3 falls through as `NoneNeeded` and can therefore never advance. vs pi-subagents v0.47.1 `runs/foreground/async-dismiss-action.ts:11-85`, `runs/foreground/subagent-executor.ts:5872-5885`. |
| ~~SUBA-064~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | M | The whole `authorityPolicy` subsystem is unported, and the `stop`/`steer` gate it drives is live-reachable — **CLOSED 2026-08-14**: sweep 1 — the subsystem + the stop/steer gate. Hard prerequisite carried verbatim into SUBA-005's unowned-verb list: whoever lands `worktree.discard` or `destructiveCleanup` must route it through `registration::authority::resolve_authority_decision` in the same change. `spawnBudgetGrant` is now pre-wired for SUBA-046 and `scheduleCreate` for SUBA-016, so both are cheaper than filed. |
| SUBA-017 | low | not-ported | M | Completion batching unported (**in-baseline**, not drift) — **RE-READ 2026-09-14 at `9aeba769` (verified re-audit): still open, unchanged on both sides.** cyrup still has no batcher, no debounce between `CompletionObserver`/`CompletionSink` and the notice, and no `completionBatch` config key. Upstream still ships `src/runs/background/completion-batcher.ts` (168 L @v0.67.0; `DEFAULT_COMPLETION_BATCH_CONFIG` debounce 150 / maxWait 1000 / straggler 75/400/2000) and still wires it at `src/extension/index.ts:504` @v0.67.0 — `registerSubagentNotify(pi, state, { batchConfig: config.completionBatch, ownership: resultDeliveryOwnership })`, i.e. the call gained an `ownership` argument since `v0.43.0` but KEPT `batchConfig`. `Kind: not-ported` re-confirmed by presence: `git cat-file -e v0.43.0:src/runs/background/completion-batcher.ts` succeeds, so it is in-baseline, not drift. **Two cosmetic corrections to the row's own evidence line:** the config struct is now **30** fields, not 18 (`registration/mod.rs:78-425`, still no `deny_unknown_fields`), and `background/watch.rs` is now the directory `background/watch/` (`message.rs:30-75`, `sink.rs`, `observer.rs`, `results_watcher.rs`). — **RE-READ 2026-09-14 at `d53763b` (post-#134 delta check): still open, unchanged.** #134 touched no file under `background/` or `registration/` (the diff is confined to `extension/executor/`, `extension/tool/`, `missions/` and `workflows/scripted/`), and `completion_batch`/`batcher` remain zero-hit crate-wide, so the `9aeba769` evidence carries over verbatim. — **RE-READ 2026-09-16 at `cc7818b` and upstream `v0.68.0`: still open, unchanged.** #137/#139/#140 added a great deal under `background/` but no batcher and no debounce: `completion_batch\|batcher\|CompletionBatch` is still zero-hit, `watch/install.rs:304` still hands the sink one message per result, and the config struct has grown again — **34** fields now (the row's corrected 30 has drifted by four), still with no `completion_batch` and still no `deny_unknown_fields`. Upstream at v0.68.0: `completion-batcher.ts` still 168 L, still wired at `extension/index.ts:495` with `batchConfig: config.completionBatch` kept, key at `shared/types.ts:2663`, grouping at `notify.ts:544/692/723/790`. **`09a`'s `SUBA-090` closure parked upstream's grouped `formatGroupedCompletion` form on this row — it is in scope here, not a separate item.** |
| SUBA-022 | low | not-ported | L | Typed extension delegation API unported (**in-baseline**, not drift) — **RE-READ 2026-09-14 at `9aeba769` (verified re-audit): still open, unchanged.** No `api/` module exists in the crate at `9aeba769` (`registration/` holds `authority.rs`, `cost.rs`, `doctor.rs`, `guide.rs`, `mod.rs`, `profiles.rs`, `prompt_workflows.rs`, `resources.rs`, `slash_commands.rs`, `tool_description.rs` and nothing else), and the five `prompt-template:subagent:*` event names are zero-hit crate-wide. Upstream still exports all five at `src/api/delegation.ts:6-10` @v0.67.0, plus the `SubagentDelegationRequest` shape at `:27-45`. `Kind: not-ported` re-confirmed by presence: `git cat-file -e v0.43.0:src/api/delegation.ts` succeeds. **Note for the fixer:** at 116 L @v0.67.0 the file has grown well past a five-constant module — it now carries the typed request/response contract (`toolBudget`, `thinking`, structured-result request, per-launch intercom-bridge config), so `Effort L` still reads correctly. — **RE-READ 2026-09-14 at `d53763b` (post-#134 delta check): still open, unchanged.** #134 added no module outside `extension/executor/`, `extension/tool/`, `missions/` and `workflows/scripted/` — there is still no `api/` in the crate — and `prompt-template:subagent` is still zero-hit crate-wide. — **RE-READ 2026-09-16 at `cc7818b` and upstream `v0.68.0`: still open, unchanged.** Still no `api/` module among the crate's 23 `src/` entries, and `prompt-template:subagent` is still zero-hit workspace-wide; the SCOPE sequence added `background/scheduled_runs/`, `background/completion_replay/`, `background/wait_subscriptions/`, `background/inspect_rpc/` and `background/async_retention/` and no `api/`. Upstream's five constants unchanged at `src/api/delegation.ts:6-10` @v0.68.0. **One line of its header is the design instruction and was not recorded before:** *"This is the established extension-to-extension transport. The structured delegation API intentionally reuses it instead of adding a second event protocol."* — so the port is a typed façade over `cyrup-ext`'s `SharedBus`, which queues emits and passes payloads by value, the same request/response-topic problem `09a` recorded for the v0.64.0 runtime-agent event bridge. Design the two together. |
| ~~SUBA-023~~ | ~~low~~ **CLOSED 2026-09-20** | not-ported *(corrected on closure: both upstream files pass `git cat-file -e v0.43.0`, so this was never drift — the correction this row owed itself)* | L | Async lifecycle hardening unported; no signal-name attribution — **PARTIALLY CLOSED 2026-08-14**: sweep 1 + 2 — the signal-name attribution half closed in sweep 1; sweep 2 closed the consumer half sweep 1 left open. `exec/mod.rs::process_signal_name` carried its OWN three-entry table (SIGINT/SIGKILL/SIGTERM) plus a `SIG{n}` fallback, so a child that segfaulted reported `SIG11` and one that aborted `SIG6` on `SingleResult.process_signal`, where pi passes Node's signal NAME through and reports SIGSEGV/SIGABRT. It now delegates to `spawn::signal::signal_name_of`, the single crate-wide mapping; the numeric form survives only as the fallback for a signal that table does not name. The dead `libc_signal` module is deleted. **RESIDUAL: the two unported upstream subsystems only — `process-terminal.ts` and `session-lease.ts` (= VL-S3 / VL-S4).** — **RE-READ 2026-09-14 at `9aeba769` (verified re-audit): still open — and the row's own evidence line and Verify clause are STALE and MUST be rewritten before anyone uses them, because the Verify now PASSES on an item that is not closed (a closure trap).** The "no `ExitStatus::signal()` name mapping anywhere in the module" observation is REFUTED: `spawn/signal.rs:112-173` carries `signal_name: Option<&'static str>` on `TerminationOutcome` (`:126`), maps the number at `:136-158`, and derives it from the OBSERVED status at `:162-173` (deliberately not from the escalation stage); it reaches the terminal record as `SingleResult.process_signal` (`exec/run_result.rs:113-125`), populated at `exec/mod.rs:1225` (see also `exec/external_cli/run.rs:403,:569-577` and `exec/mod.rs:468,:742`). So the Verify sentence "a child killed at the SIGKILL rung must report the signal name in its run record" is already satisfied and cannot be used as the closure test. **What is NOT ported, and is the whole remaining item, is exactly the RESIDUAL this row already records:** `src/runs/background/process-terminal.ts` (310 L @v0.67.0 — the writers/`expectedWriters` terminal-candidate reconciliation and `releaseActiveRunIndex`) and `src/runs/shared/session-lease.ts` (293 L @v0.67.0 — `SESSION_LEASES_DIR` and the token/pid/hostname/`processStartIdentity` owner record). Both are zero-hit crate-wide. Severity stays `low` only because it is already the floor; the residual is materially larger than the signal half that got done. — **RE-READ 2026-09-14 at `d53763b` (post-#134 delta check): still open, unchanged.** `process_terminal` and `session_lease` are still zero-hit crate-wide and `spawn/` was not touched by #134. The one thing that could be mistaken for the residual is #134's net-new `extension/executor/workflow_child_stops.rs`, and it is a DIFFERENT upstream surface — pi's `state.workflowChildStops` map (`shared/types.ts:2315-2316`, its own module doc), which stops one child of a live workflow; it carries no terminal-candidate reconciliation and no lease record. — **RE-READ 2026-09-16 at `cc7818b` and upstream `v0.68.0`: still PARTIALLY CLOSED, and the residual has become MORE expensive, not less.** The signal half is intact (`spawn/signal.rs:112-127`, consumers at `exec/attempt_runner.rs:150` and `exec/external_cli/run.rs:403,:569`, two tests green). The residual is still `process-terminal.ts` + `session-lease.ts`, both zero-hit — **but the SCOPE sequence built two new production call sites ON TOP of their absence**, each with a documented substitute: `background/active_async_capacity/inspect.rs:145-172` replaces upstream's `processTerminal`-`state:observed` + `runnerProcessInstanceId` release proof with the runner pid, explaining that a verbatim port would retain every SUCCESSFUL run forever and so be strictly worse than no cap; and `background/active_run_index.rs:56-59` records the `readProcessTerminal` disjunct of the staleness rung (`async-status.ts:575`) as unappliable, leaving the age disjunct alone. **A later port is now a replacement of two live substitutes, not an addition.** The kind correction this row's body already implies is still owed: both upstream files pass `git cat-file -e v0.43.0`, so `Kind` should read `not-ported` — the same correction `SUBA-017`/`SUBA-022`/`SUBA-024` carry. — **CLOSED 2026-09-20.** The RESIDUAL this row restated across four re-reads — *"the two unported upstream subsystems only — `process-terminal.ts` and `session-lease.ts` (= VL-S3 / VL-S4)"* — is DISCHARGED. `background/session_lease/` (7 modules, 2,343 LOC) and `background/process_terminal/` (8 modules, 3,569 LOC) port both files whole, each reachable from a production caller: the orchestrator mints the `RunnerProcessInstanceId` and writes the candidate + `pending` sidecar before the spawn (`extension/executor/background.rs:665`, `:880`), the runner acquires the revival lease (`runner_main/entry.rs:118`), writes its candidate, releases the lease and finalizes its own proof in upstream's order (`:273`, `:289`, `:299`), and the capacity release rung reads that proof before the pid ladder beneath it. Three `--features it` files carry the proof, and what each one actually drives matters: `session_lease_revival_integration.rs` and `process_terminal_lifecycle_integration.rs` drive `run_with` — the real hop-2 runner main loop — IN PROCESS with an injected `spawn_command`, so every CHILD is a genuine OS process in its own process group while the runner itself is this test's task; `debug_run_lifecycle_integration.rs` is the one that spawns a genuinely DETACHED hop-1 process through `spawn_background_steps` and reads its slot back after that pid is really dead. The lease IT refuses a second revival of one session file against an incumbent whose pid is genuinely alive on this genuine hostname; `process_terminal_lifecycle_integration.rs::a_killed_runner_leaves_a_pending_proof_where_a_clean_one_leaves_observed` pins the crash-vs-clean discriminator by ABORTING a runner mid-child and comparing it against the identical run allowed to finish — and pins the ORDERING with it, because the clean proof's `resumeDisposition` is `resumable` only if `finish_run` wrote the terminal status BEFORE the finalize; `debug_run_lifecycle_integration.rs` reads the same `pending` sidecar under a genuinely dead OS pid and asserts the capacity ladder's fallback sentence over it. **The two substitutes this row's last re-read warned had been built ON TOP of the absence are replaced, not left standing:** `active_async_capacity/inspect.rs`'s release rung now reads the real proof (the pid ladder stays BENEATH it, because a runner killed before finalizing writes no proof and its slot would otherwise be retained forever), and `active_run_index.rs`'s staleness rung has its `readProcessTerminal` disjunct back (`async-status.ts:575`), so a marker is released on an observed proof instead of waiting out 24 hours. The signal half was already intact and was NOT touched: `spawn/signal.rs:112-127`, `:136-158`, `:162-170`, exactly as this row's own REFUTED note said. See `PARITY-GAPS.md` VL-S3/VL-S4 for the full closure evidence. |
| SUBA-024 | low | upstream-drift | L | `parallel-handoff` / `agent-contract` unported (`task-intent` closed; `chain-validation` struck) — **RE-READ 2026-09-14 at `9aeba769` (verified re-audit): still open, and the three-pass blind spot is now settled as a NEGATIVE.** Neither symbol is implemented: `agent_contract`/`parallel_handoff` are zero-hit as implementations, and the four apparent hits are (a) two comments quoting pi's `isAgentContractV1` predicate — one of which, `exec/acceptance/lattice/inject.rs:49-58`, states outright that `runs/shared/agent-contract.ts` is unported and that `report_optional` is therefore hardcoded `false` for every run; (b) `workflows/scripted/engine.rs:757` and `extension/tool/schema.rs:509`, which only NAME `agentContract` as a positional landmark (it is not in cyrup's advertised key set); and (c) `missions/lifecycle.rs:432-444` and `:893-904`, which defensively READ a `parallelHandoff.path` out of foreign JSON to file a `MissionArtifact` — nothing in the crate WRITES that key, and `lane.recordMerge`/`lane.recordSupersession` are likewise unadvertised (see also `spawn/chain_graph.rs:2047-2048`). **KIND CORRECTION OWED (deliberately not spent as a `reclassify` verdict this pass):** both upstream files pass `git cat-file -e v0.43.0`, so `upstream-drift` is wrong on the same grounds `SUBA-017`/`SUBA-022`/`SUBA-026` were argued — and the row's own body already records them as present at BOTH `v0.43.0` and `v0.47.1`, i.e. the body and the `Kind` cell already disagree. Whoever edits this row next should set `Kind: not-ported` to match its own evidence. **Size for planning:** `src/runs/shared/parallel-handoff.ts` is 741 L @v0.67.0 (lane bindings, merge/supersession evidence, worktree cleanup) and `src/runs/shared/agent-contract.ts` only 38 L — they should be split, not landed together. — **RE-READ 2026-09-14 at `d53763b` (post-#134 delta check): still open, unchanged.** `agent_contract`/`parallel_handoff` are still zero-hit as implementations. #134 rewrote `workflows/scripted/engine.rs`, which moved the `agentContract` hit from `:757` to `:794`, but it is still a NAME only — an entry in `AUTO_RESUME_PARAM_KEYS` (`:791-792`, ported verbatim from pi `:1679` as a key list), not a contract concept — and `exec/acceptance/lattice/inject.rs:49-59` still passes a hardcoded `false` for `report_optional` and still states in source that `runs/shared/agent-contract.ts` is unported. The kind correction this row records as owed is still owed. — **RE-READ 2026-09-16 at `cc7818b` and upstream `v0.68.0`: still open.** Both symbols still unimplemented, and the hits are still the crate declaring the gap — `exec/acceptance/lattice/inject.rs:49-55` and `spawn/chain_graph.rs:2047-2051` both pass `report_optional: false` and both keep it a PARAMETER precisely so that porting `agent-contract.ts` is a change at one seam. **One consumer-side slice HAS landed since and the row must not be closed on it:** `has_unresolved_run_handoff` (`background/async_retention/scan.rs:423-440`) is a full port of pi's `hasUnresolvedRunHandoff` + `unresolvedHandoff` for the `<run_dir>/handoff.json` half, version-gated and fail-closed — and its own doc says *"no cyrup writer produces the file today"*. A reader with no producer is not closure; the neighbouring `scan.rs:425` `[CYRUP-DELTA]` records that `RunStatus` still has no `parallel_handoff` field, so upstream's first path source is not simulated. Both upstream files present at v0.68.0 and at v0.43.0, so **the `Kind: not-ported` correction this row records as owed is still owed.** **The chain pre-walk / `ChainStepConfig` blind spot is now carried for a FOURTH pass** — two functions would settle it and no pass has opened them. |
| SUBA-026 | low | upstream-drift | L | Interactive admin UI, selector and `/subagents` unported (`/subagents-stop` landed) — **RE-READ 2026-09-14 at `d53763b` (post-#134 delta check, both sides; note this row was NOT part of the `9aeba769` re-audit): still open, unchanged.** #134 touched neither `registration/` nor `tui/`: `SLASH_COMMANDS` (`registration/slash_commands.rs:184-281`) registers ten `/subagents-*` commands and no bare `/subagents`, and there is no selector. Upstream re-read at v0.67.0: `src/slash/subagents-admin.ts` is still present, now 456 L. — **RE-READ 2026-09-16 at `cc7818b` and upstream `v0.68.0`: still open, with ONE CITATION IN THE BODY CORRECTED — it names a file that exists at no tag.** The body cites `src/tui/selector.ts`; `git cat-file -e` fails for it at v0.43.0 AND v0.68.0 and `git log --all -- src/tui/selector.ts` is empty. The real file is **`src/slash/selector.ts`** (147 L @v0.68.0, present at v0.43.0). **PARITY-GAPS VL-S11 carries the same wrong path.** `src/slash/subagents-admin.ts` is **460** L @v0.68.0 (456 at v0.67.0, 432 at v0.47.1 — it grows every tag) and `subagents` registers at `src/slash/slash-commands.ts:869`. Port side: the `as_str` table is 17 variants (`registration/slash_commands.rs:129-149`), with `/subagents-guide` at `:147` now closed as `SUBA-066`; still no bare `subagents` and `selector\|Selector` is zero-hit under `src/tui/`, so the Fix's premise that FleetView "now supplies the rendering primitives this needs" remains an unchecked assumption. Both upstream files are at v0.43.0 → `Kind` should read `not-ported`. |
| ~~SUBA-029~~ | ~~low~~ **CLOSED 2026-08-14** | cyrup-original | S | Management actions read-modify-write subagents `settings.json` unlocked — **CLOSED 2026-08-14**: sweep 1. |
| ~~SUBA-033~~ | ~~low~~ **CLOSED 2026-08-14** | test-defect | S | Tests assert a lower bound on observed concurrency — **CLOSED 2026-08-14**: sweep 1 — both lower bounds removed; overlap is enforced by a `tokio::sync::Barrier` rendezvous inside the worker with a bounded wait, so a serialization regression fails loudly instead of hanging. |
| ~~SUBA-034~~ | ~~low~~ **CLOSED 2026-08-15 — REFUTED / already-done** | not-ported | M | `wait`'s event-bus wake unported; pure polling at a 1 s floor — **REFUTED, CLOSED 2026-08-15**: sweep 11 read the file at HEAD and the whole item is already in, landed by **sweep 9 (`844e25f`)** and never marked. `background/wait.rs` carries a `# Wake mechanism (SUBA-034)` module section, `WaitDeps::completion_bus` threads `watch::CompletionBus`, and the loop `select!`s a `broadcast::Receiver` against the sleep with `biased;` (cancellation first, then the wake, then the timer) — the `Lagged`-is-a-wake / `Closed`-retires-the-subscription handling is there too, which is the part a naive port gets wrong (a closed receiver returns instantly forever and spins the loop at 100% CPU). The subscription is taken BEFORE the first listing, with the race argument written out at the call. The surviving delta is a LATENCY FLOOR, not a mechanism gap, and is recorded in-source: cyrup's publisher is `ResultsWatcher`, bounded below by its own 500 ms `RESULTS_DIR_POLL_INTERVAL`, so what the bus removes is the SECOND 1 s interval stacked on top. `completion_bus: None` is upstream's own no-bus degradation. **No fix was manufactured and no test was added** — the item's Verify is already covered by the code's own tests. |
| ~~SUBA-035~~ | ~~low~~ **CLOSED 2026-08-14 — REFUTED / already-done, and its residual's stated LOCATION was wrong too** | not-ported | S | Active `subagents.modelScope` policy not surfaced by doctor/models — **CLOSED 2026-08-14**: sweep 8. **Both halves are in at HEAD.** Doctor: `registration/doctor.rs:646-676` `model_scope_check` (four arms), called at `:603`, tests at `:1609`-`:1642`. Models report: `crates/cyrup-ext-subagents/src/extension.rs` — the single-agent view (~`:3565-3572`) and the all-agents view (~`:3579-3585`) each push a `Model scope:` line built by `crate::exec::model_scope::model_scope_summary_line`, and each **cites SUBA-035 by name in-source**. **CORRECTION THAT MUST SURVIVE THE CLOSURE: the residual's stated location — `registration/mod.rs` / `profiles.rs` — is wrong.** The models report is `extension.rs::run_models_report`, so anybody following this row would have hunted in two files that never carried the surface. Port target `pi-subagents/src/runs/shared/model-scope.ts` @v0.43.0. |
| ~~SUBA-037~~ | ~~low~~ **CLOSED 2026-08-14** | cyrup-original | S | Doctor's `--version` probe leaks the probe process on timeout — **CLOSED 2026-08-14**: sweep 1 — no test was added and the reason is recorded: `VERSION_PROBE_TIMEOUT` is not injectable and the flag's effect is a tokio guarantee. |
| ~~SUBA-038~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Three denial/unknown-action messages still diverge from pi's text — **CLOSED 2026-08-14**: sweep 1 — residual 2 was closed by porting the v0.47.1 message (SUBA-065) rather than the v0.43.0 text, since the two items rewrite the same three strings and the richer form supersedes the bare one. |
| ~~SUBA-039~~ | ~~low~~ **CLOSED 2026-08-14 — REFUTED** | cyrup-original | M | `SpawnedChild` has no `Drop` guard, so a dropped drive future orphans a group — **REFUTED, CLOSED 2026-08-14**: sweep 6 — closed at HEAD. `spawn/mod.rs:849` is `impl Drop for SpawnedChild`, and `:394` documents the `Option` field that exists specifically to make that `Drop` work. **This is the JS→Rust guarantee-gap class (a dropped future orphaning a detached process group), which is why sweep 6 checked it first among the lows.** No upstream counterpart — pi's async functions always settle. |
| ~~SUBA-058~~ | ~~low~~ **CLOSED 2026-08-14** | upstream-drift | S | Chain read instructions not filtered by existence — **CLOSED 2026-08-14**: sweep 1 — citation drift recorded: the pinned expectation is at `spawn/chain_graph.rs` in `build_chain_instructions_emits_reads_output_prefix_and_previous_suffix`, not at the stated `:2522`. |
| ~~SUBA-059~~ | ~~low~~ **CLOSED 2026-08-14** | upstream-drift | S | `artifactConfig.cleanupDays` never wired to the type that already parses it — **CLOSED 2026-08-14**: sweep 1 — one correction: the item says to add `artifact_config: Option<ArtifactConfig>`; upstream's type is `Pick<ArtifactConfig,"cleanupDays">`, so the new field is `Option<ArtifactRetentionConfig>` (one key). The full struct would advertise five per-run switches upstream does not read from config. |
| ~~SUBA-060~~ | ~~low~~ **CLOSED 2026-08-14 — REFUTED** | upstream-drift | S | "Resume-first" guidance for failed async runs unported — **REFUTED, CLOSED 2026-08-14**: sweep 6 — closed at HEAD. `background/resume_guidance.rs` exists and is referenced from `background/wait.rs` and `background/mod.rs`. vs pi-subagents v0.47.1. |
| SUBA-061 | low | not-ported | M | Four config keys silently ignored: `asyncWidget`, `inlineToolDisplay`, `fleetKeybindings`, `legacyChainControls` — **RE-READ 2026-09-14 at `9aeba769` (verified re-audit): still open for THREE of the four keys; the fourth must be struck from this row.** `legacyChainControls` was live at `v0.47.1` (`types.ts`, `extension/config.ts`, `schemas.ts`, `tool-description.ts`, `fanout-child.ts` — five files) and is GONE from the whole of `src` at `v0.67.0`: `git grep legacyChainControls v0.67.0 -- src` is EMPTY. Upstream DELETED the key, so porting it now would port a surface that no longer exists — it LEAVES the item rather than staying as work. Surviving: `asyncWidget` (`src/shared/types.ts:2574` @v0.67.0, `:1750` @v0.43.0) and `inlineToolDisplay` (`:2582` @v0.67.0, `:1754` @v0.43.0), both in-baseline, so `not-ported`; plus `fleetKeybindings` (`:2572` @v0.67.0), post-baseline, so drift. The mixed `Kind` is therefore still right, but for **2+1** keys rather than 2+2. The cyrup half was re-verified by observation rather than inherited: `registration/mod.rs:78-425` is now **30** fields (the row's "18" is stale — it gained `authority_policy`, `artifact_dir`, `artifact_config`, `max_subagent_spawns_per_run`, `permissions`, `tool_description_mode`, `turn_budget`, `timeout_ms`, `default_subagent_context`, `spawn_command` and friends), carries `#[serde(rename_all = "camelCase", default)]` at `:78` with **no** `deny_unknown_fields` and no unknown-key validator beside `validate_missions`/`validate_authority_policy`/`validate_artifact_dir`/`validate_artifact_config` — so the three remaining keys are still accepted and silently dropped. Severity `low` already at the floor. — **RE-READ 2026-09-14 at `d53763b` (post-#134 delta check): still open, unchanged.** #134 did not touch `registration/mod.rs`, and all four keys (`asyncWidget`, `inlineToolDisplay`, `fleetKeybindings`, `legacyChainControls`) are still zero-hit crate-wide, so the three surviving keys are still accepted and silently dropped exactly as recorded at `9aeba769`. — **RE-READ 2026-09-16 at `cc7818b` and upstream `v0.68.0`: still open on the same THREE keys; `legacyChainControls` stays struck.** Re-confirmed at the current tag rather than carried: `git -C tmp/pi-subagents grep -c legacyChainControls v0.68.0 -- src` is EMPTY, so upstream's deletion held across another five tags. The three survivors are still declared at `shared/types.ts` @v0.68.0 — `fleetKeybindings` `:2609`, `asyncWidget` `:2611`, `inlineToolDisplay` `:2617` — so the mixed kind is 2+1. Port side the struct has grown again to **34** fields (`registration/mod.rs:79`; the row's 30 gained `model_exclusions`, `max_active_async_runs_per_session`, `capacity`, `scheduled_runs`), still `#[serde(rename_all = "camelCase", default)]` with no `deny_unknown_fields`, so all three are still accepted and silently dropped. **The Fix's `SUBA-025` sequencing clause is moot, not discharged:** `tool_description_mode` is on the struct so the builder exists, but the key it was to feed is the deleted one. |
| ~~SUBA-062~~ | ~~low~~ **CLOSED 2026-08-14** | cyrup-original | L | Bundled `researcher` cannot do web research — the crate's target has no web tools — **CLOSED 2026-08-14**: sweep 1 — the in-crate half (the `[CYRUP-DELTA]` header). The handoff to areas 04/12 for `web_search`/`fetch_content`/`get_search_content` stands. |
| SUBA-063 | low | not-ported | M | Zero-tool-budget authorisation and the runtime-extension acknowledgement path unported — **RE-READ 2026-09-14 at `9aeba769` (verified re-audit): still open — but BOTH of the row's cyrup evidence lines are now WRONG and the SHAPE of part (a) has changed; recorded here so the next reader does not close this on the grep alone.** (a) `rg -c 'ZERO_AUTH' = 0` is REFUTED: `TOOL_BUDGET_ZERO_AUTH_ENV` exists (`exec/tool_budget.rs:39`), `HardMinimum::{One,Zero}` exists (`:31-72`), and the CHILD side fully honours it (`HardMinimum::from_env` at `:56-64`, exact-string `"1"` equality as upstream; `prompt_runtime.rs:2301,:2598-2680` decodes with it and blocks the first call). But what that code DOES was audited rather than that it exists, and it is a dead end: **no site anywhere writes `TOOL_BUDGET_ZERO_AUTH_ENV` into the child env** — `exec/spawn_plan.rs:1163-1176` writes `TOOL_BUDGET_ENV` and stops — and the parent-side validator `validate_tool_budget_config` (`tool_budget.rs:111-115`) hardcodes `HardMinimum::One`, as do `spawn_plan.rs:3128` and `:3391`, so a declared `hard: 0` is refused parent-side before it can ever be authorised. `HardMinimum::Zero` is reachable only from tests and from the child's own env read. **The defect is therefore now `unwired`, not `not-ported`, and its failure mode has INVERTED relative to `CFG-080`:** an unauthorised zero no longer runs unbudgeted (fail-open) but refuses the child runtime (fail-closed). Upstream has meanwhile grown the parent half considerably — a `delegatedAllowZeroToolBudget` private param and a `delegatedZeroToolBudgets` `WeakSet` (`src/runs/foreground/subagent-executor.ts:7283-7289`, `:4891`) gating `{ minimumHard: 0 }` at `:6688`; see also `:513`, `:3996` and `src/shared/types.ts:2386` (`allowZeroToolBudget`) @v0.67.0. (b) The runtime-extension acknowledgement path is unchanged and genuinely unported, and upstream has grown it well past a path env var into a `subagent:acknowledge-extension` RPC event surfaced on `AsyncStatus` (`src/extension/rpc.ts:457`, `src/runs/background/subagent-runner.ts:1224,:1368`, `async-status.ts:87,:140,:386,:441` @v0.67.0). (c) The row's own "separately and cheaply" residual — the `ASYNC_EVENTS_MAX_BYTES` override — **IS closed**: `background/runner_main/events.rs:9-36` ports it with upstream's `Number()` coercions and one documented divergence. The `subagent.events.truncated` marker (`TRUNCATED_EVENT_TYPE`, `subagent-runner.ts:329` @v0.67.0) is not. — **RE-READ 2026-09-14 at `d53763b` (post-#134 delta check): still open, unchanged.** #134 touched nothing under `exec/` or `background/`: `TOOL_BUDGET_ZERO_AUTH_ENV` (`exec/tool_budget.rs:39`) still has no parent-side writer and `validate_tool_budget_config` (`:115`) still hardcodes `HardMinimum::One`, so part (a) is still the `unwired`, fail-closed dead end this row describes; `acknowledge-extension`/`acknowledge_extension` are still zero-hit crate-wide, so part (b) is untouched. — **RE-READ 2026-09-16 at `cc7818b` and upstream `v0.68.0`: still open, and this pass CONFIRMS the `9aeba769` re-read's sharpest observation by re-running it rather than inheriting it.** Part (a) is still a reader with no writer: `git grep TOOL_BUDGET_ZERO_AUTH` over the WHOLE workspace returns 12 hits, every one in `exec/tool_budget.rs` or `prompt_runtime.rs`; `exec/spawn_plan.rs:1173` writes `TOOL_BUDGET_ENV` and no sibling. **The `unwired` reclassification that re-read proposed is correct and should be applied to the `Kind` cell.** One thing HAS changed in its favour: the row's stated dependency on `SUBA-047` for a caller surface is discharged — `toolBudget` is advertised (`extension/tool/schema.rs:546`) and validated (`extension/tool/routing.rs:736,:825`), and `hard: 0` is refused parent-side with *"toolBudget.hard must be an integer >= 1."* (`routing_tests.rs:604,:1801`), so only the writer and the explicit-zero plumbing are owed. Upstream's parent half at v0.68.0 is where the row said it was, line-shifted: `delegatedZeroToolBudgets` `subagent-executor.ts:4998`, the `{ minimumHard: 0 }` gate `:6870`, `delegatedAllowZeroToolBudget` `:7469-7475`, `allowZeroToolBudget` on the async input `async-execution.ts:319` and `shared/types.ts:2427`. Part (b) unchanged and now has its own upstream module — `src/runs/shared/runtime-acknowledged-extensions.ts` @v0.68.0 beside the `acknowledge-extension` RPC — still zero-hit here. Part (c)'s marker (`TRUNCATED_EVENT_TYPE = "subagent.events.truncated"`, `subagent-runner.ts:320` @v0.68.0, `:266` @v0.43.0 — in-baseline) is still unported; the cap override stays closed. |
| SUBA-097 | low | cyrup-original | S | `scheduled_runs`' production launcher is claimed to be integration-tested by a file that does not exist — **FILED 2026-09-16** while verifying `SUBA-016`'s closure. `background/scheduled_runs/trigger.rs:884-888` justifies stubbing the launcher in its unit tests with *"the integration suite (`tests/scheduled_runs_integration.rs`) covers the production one"*. **There is no such file:** the crate has no `tests/` directory at all (its tests are at `src/tests/`, 15 files, none about schedules) and `find . -name 'scheduled_runs_integration*'` is empty workspace-wide. The seam it promises is covered — `ExecutorScheduleLauncher` (`extension/executor/scheduled_runs.rs:39-51`), the only thing that actually spawns a `RunMode::Workflow` run — has exactly three references in the crate: its declaration, its `impl ScheduleLauncher`, and its one construction at `:317`. No test names it. The surrounding suite is unusually strong (all nine verbs through `Tool::execute`, 100/100 green), which makes the one gap easier to miss, not harder. **Fix:** write the row the comment promises, or correct the sentence to say the production launcher is not covered — a false coverage claim is worse than an acknowledged gap because it stops the next reader looking. |
| SUBA-098 | low | cyrup-original | S | A stray CJK token inside an English doc comment — **FILED 2026-09-16** while verifying `SUBA-016`'s closure. `extension/tool/scheduled_runs_tests.rs:31` reads *"a trigger that read it live would观察 a different identity after the swap"*; the verb is replaced by 观察 with no surrounding space, in the comment that justifies `SwappableSessionHost`'s mutability for two session-pinning tests. A workspace Unicode-script sweep (`rg '[\p{Han}\p{Hiragana}\p{Katakana}\p{Hangul}]' crates/ -g '*.rs'`) shows every other match is deliberate test DATA — CJK width fixtures in `cyrup-tui`, `"日"` in the schedule-interval rejection table, `"あ"` in the output-truncation test — with exactly one other accidental instance, `crates/cyrup-tui/src/tests/selector.rs:41` (*"an empty/末-position caret"*), which belongs to **area 07** and is named here only so the sweep is not re-run. Cosmetic; filed because ids are never deleted here, so it can be closed in one line whenever the file is next touched. |
| ~~SUBA-065~~ | ~~low~~ **CLOSED 2026-08-14** | upstream-drift | S | `unknownSubagentActionMessage` — did-you-mean recovery and its destructive-action gate — unported — **CLOSED 2026-08-14**: sweep 1 — `SUBAGENT_ACTIONS` is now a single const feeding BOTH the schema enum and the message, which is the structural half of SUBA-005's owed "completeness assertion": the enum can no longer drift from the advertised list, only from upstream's. |
| ~~SUBA-066~~ | ~~low~~ **CLOSED 2026-08-15** | upstream-drift | S | `/subagents-guide` slash command unported (outside both VL-S11 and SUBA-055) — **CLOSED 2026-08-15 (sweep 11), landed with SUBA-055 as that item's Fix line instructed.** `SlashCommandName::SubagentsGuide` + its descriptor (`description` upstream's verbatim, `slash-commands.ts:707` @v0.47.1) and a `dispatch_slash` arm routing to the SAME `registration::guide::read_subagent_guide` the `guide` action uses — two readers over one document set cannot disagree about the current contract. Upstream's multi-word-argument refusal (`ctx.ui.notify("Usage: /subagents-guide [topic]", "error")`, `:712-715`) is ported as a pre-dispatch refusal rather than falling through to the unknown-topic message, because a two-word argument is a usage mistake and the topic list is the wrong answer to it. **2 tests**, one of which is the pre-existing table-count assertion moved from `+ 4` to `+ 5` — red before the variant existed. |
| ~~SUBA-067~~ | ~~high~~ **CLOSED 2026-08-14** | test-defect | S | Descendant-termination fixture exec-collapses to one pid; test never exercised group-kill (FIXED) — **CLOSED 2026-08-14**: closed pre-sweep — the descendant-termination fixture exec-collapsed to one pid, so the test never exercised group-kill. Found by RUNNING the suite. |
| ~~SUBA-068~~ | ~~high~~ **CLOSED 2026-08-14** | test-defect | S | Setup-hook timeout fixture races macOS ~200 ms first-exec verification with a 200 ms budget (FIXED) — **CLOSED 2026-08-14**: closed pre-sweep — the setup-hook timeout fixture raced macOS's ~200 ms first-exec verification with a 200 ms budget. Found by RUNNING the suite. |
| ~~SUBA-069~~ | ~~high~~ **CLOSED 2026-08-14** | test-defect | M | Setup-hook test family is wall-clock-budgeted; 3 siblings go red under machine load (OPEN) — **CLOSED 2026-08-14**: sweep 2 — **and TWO FACTUAL ERRORS IN THE ITEM ARE CORRECTED FIRST.** (1) "pi's default is the same 5000 ms" is wrong: pi's constant is **30000** at both v0.43.0:113 and v0.47.1:114 (`DEFAULT_WORKTREE_SETUP_HOOK_TIMEOUT_MS`), and cyrup's already matched it. (2) "These use the production DEFAULT 5000 ms hook timeout rather than a fixture constant, so unlike SUBA-068 they cannot simply be re-budgeted" is wrong: all three fixtures passed `timeout_ms: Some(5_000)` explicitly at HEAD, so they were always re-budgetable. Fix landed options (a)+(b) together: `write_hook_script` emits a `[ -n "$CYRUP_HOOK_WARMUP" ] && exit 0` guard as line 2 and a new `warm_hook_exec` helper pays macOS's one-off first-`exec` verification (measured 197-242 ms) OUTSIDE any timeout budget, and the four non-timeout fixtures now use the SHIPPED 30 s default. The production constant is untouched. **The Verify (green under deliberate load ≥8, 3×) was NOT executed — the warm-up mechanism is unit-pinned, the flake reduction is not measured.** |
| SUBA-070 | ~~low~~ **CLOSED 2026-09-14** | not-ported | M | The `interactive` frontmatter key is parsed into a typed field but never enforced — **filed 2026-08-14** by a documentation audit; deliberate for v1 per an in-source note, recorded here so a later pass does not re-derive it as a discovery. — **CLOSED 2026-09-14 (verified re-audit at `9aeba769`) — REFUTED, not fixed.** The row's upstream limb was an explicit unverified assertion ("pi-subagents honours `interactive` when deciding whether a child may prompt. Tag and line NOT re-read this pass; establish before fixing"). It was established this pass and it is FALSE at **both** `v0.43.0` and `v0.67.0`. READ ON BOTH SIDES. Upstream: an exhaustive sweep of `interactive` across all of `src` (49 hits, 18 files @v0.67.0) finds exactly FOUR uses of the AGENT-level key and not one of them changes spawn behaviour — parsed off frontmatter (`src/agents/agents.ts:2154` @v0.67.0, `:1634` @v0.43.0), typed on `AgentConfig` (`agents.ts:173` @v0.67.0, `:147` @v0.43.0), re-serialized on write (`src/agents/agent-serializer.ts:39,:137` @v0.67.0), validated and round-tripped by the runtime registry (`src/agents/runtime-agent-registry.ts:205,:243,:279,:361` @v0.67.0), and folded into the definition digest (`src/shared/launch-contract.ts:73` @v0.67.0, `:64` @v0.43.0 — cache identity, not behaviour). Every run-path occurrence — `async-execution.ts:150,:1538,:2092`, `subagent-executor.ts:1341,:1723`, `subagent-wait.ts` — is `ctx.interactive`/`ctx.hasUI`, the PARENT SESSION's interactivity, an unrelated concept. cyrup @`9aeba769` does exactly what upstream does: parses (`discovery/frontmatter.rs:111,:1022,:1331,:2106-2116`), types, writes back (`discovery/management/frontmatter_write.rs:232-233`, `discovery/management/agent_crud.rs:51,:274,:377`) and exposes it in the runtime registry (`discovery/runtime_registry.rs:103,:193,:300`). It is therefore at PARITY, not behind — there is no behaviour to port and no user-visible divergence, so this is not an open defect, and the crate's own test name `interactive_is_parsed_into_typed_field_and_never_dropped_from_extra_fields_expectation` turns out to describe upstream's behaviour too, not a cyrup v1 shortcut. **This closure rests on an ARGUMENT (an exhaustive negative over upstream's uses), so per this directory's rule it carries its falsification condition:** reopen if any consumer of the AGENT-level `interactive` key is found at a tag between `v0.43.0` and `v0.67.0` that alters child spawn or prompting — specifically (i) any read of `agent.interactive` / `agentConfig.interactive` outside `agents/`, `agent-serializer.ts`, `runtime-agent-registry.ts` and `launch-contract.ts`; (ii) any bracket or quoted access of the `"interactive"` key on an agent object (zero @v0.67.0); (iii) any destructuring `const { interactive } = agent` (zero @v0.67.0); or (iv) `projectAgentDefinition`'s output being spread into a launch/spawn argument object whose `interactive` member is then read child-side. The concrete measurement that reopens it: `git grep -n '\.interactive' <tag> -- src` returning a hit under `src/runs/` or in `src/runs/shared/pi-args.ts` that is not `ctx.`/`input.ctx.`-qualified. **One genuine residual belongs to a DIFFERENT row, not this one:** upstream's digest projection `projectAgentDefinition` (`launch-contract.ts`) has no cyrup counterpart at all, which is `SUBA-021` territory — and note that `SUBA-021`'s inline caveat wrongly struck `launch-contract.ts` as "absent at both tags at every path" when `git cat-file -e` succeeds at `v0.43.0`, `v0.47.1`, `v0.57.0` and `v0.67.0`. |
| ~~SUBA-072~~ | ~~medium~~ **CLOSED 2026-09-04** | port-bug | S | `.cyrup-subagent-scratch/` (the per-attempt raw-stdout tee) is written under the PROJECT working tree (`opts.cwd.join(".cyrup-subagent-scratch")`, `exec/mod.rs:3858`) instead of the crate's own established global root (`crate::background::temp_root_dir()`, `<CYRUP_HOME\|HOME>/.cyrup/subagents` or OS temp, matching pi's `os.tmpdir()`-scoped `TEMP_ROOT_DIR`) — **filed 2026-08-18**. Full diagnosis + fix: [`bugs/SUBA-072-scratch-dir-project-scope.md`](bugs/SUBA-072-scratch-dir-project-scope.md). **CLOSED 2026-09-04, cyrup `7791b26a`.** The one call site now resolves through a new `background::attempt_scratch_dir(cwd)` / `attempt_scratch_dir_in(&Roots, cwd)` pair (`background/mod.rs:1493`/`:1501`, leaf `SCRATCH_SUBDIR = "scratch"` at `:1237`) — `<Roots::run_scratch>/scratch/<cwd_key>`, a third sibling of the async/results (`run_artifact_roots_in`) and artifacts/chain-runs (`crate::artifacts`) trees under the crate's ONE run-scratch root; `exec/mod.rs:827` (`prepare_ladder`) is the consumer. Upstream re-read at **v0.64.0** (ADR-0006): every per-spawn scratch file is `os.tmpdir()`-rooted (`src/runs/shared/pi-args.ts:787`/`:802`/`:826`/`:841`/`:855`, `fs.mkdtempSync(path.join(os.tmpdir(), "pi-subagent-"))`; `cleanupTempDir` `:1052-1059`, invoked from `src/runs/foreground/execution.ts:491`/`:560`/`:602`/`:635`/`:1387`/`:1426`) and every persisted run tree hangs off `TEMP_ROOT_DIR` (`src/shared/types.ts:2689-2695`). Only the directory moved: the tee is still created once per run and still survives it (R-SA-058, `extension/executor/foreground.rs:326`), and the structured-output guard is unchanged. Design (functional core / imperative shell): pure `_in(&Roots, cwd)` core + env-reading wrapper, the `run_artifact_roots` shape; rejected a `RunArtifactRoots` field, threading `Roots` through `RunOptions`, and an inline `temp_root_dir().join(..)` — reasons in the commit body. **Tests (red before / green after):** `exec::tests::prepare_ladder_makes_the_scratch_dir_under_the_run_scratch_root_not_the_project_tree` (`exec/mod.rs:1956`; run against the pre-fix call site and failed at the `assert_eq!` on `setup.scratch_dir`), `background::tests::attempt_scratch_dir_is_a_cwd_keyed_leaf_of_the_run_scratch_root_never_the_project_tree` (`background/mod.rs:3497`; new symbol, uncompilable before), `temp_root_dir_lives_under_the_os_temp_dir_and_never_under_home` (`:3440`) now also covers the scratch leaf, and `cyrup-it` `artifacts_run_integration.rs:210` asserts a REAL run leaves no `.cyrup-subagent-scratch/` in the project tree. The five in-crate absence assertions and the eight `cyrup-it` tee readers (§3's amended list, plus `child_protocol_stream_integration`, `background_runner_main_integration` and intercom `child_bridge_activation` which the bug file's list missed) now resolve through the public `attempt_scratch_dir`; `nextest -p cyrup-ext-subagents` 2668/2668, `-p cyrup-it --features it --test subagents` 195/195, intercom `child_bridge_activation` 1/1. **Residuals, both low:** (1) `runner_main::run_with`'s test-only `roots` override does not reach `exec::run_sync`, which reads the process env — pre-existing (the pre-fix path read `opts.cwd`, no roots at all), and every in-process runner test reads the tee through the same env-reading resolver, so nothing observes the difference; (2) `.gitignore:20`'s `.cyrup-subagent-scratch/` entry is now dead (nothing writes that name) and can be dropped by whoever next touches the root ignore file. The bug file's Verify (`ls -R $CYRUP_HOME/.cyrup/subagents/scratch` → `<cwd_key>/attempt-N.jsonl`) is the exact layout landed. (Line citations refreshed to HEAD by the 2026-09-04 review fix `6cf2cb9f` — same-day commits shifted the two files; every citation also names its symbol.) |
| ~~SUBA-071~~ | ~~medium~~ **CLOSED 2026-08-15 — REFUTED** | cyrup-original | M | Subagent settings are read from two files that can disagree, with no merge and no precedence rule — **REFUTED, CLOSED 2026-08-15 (sweep 11). There is exactly ONE settings store in this crate, and its precedence is upstream's.** The item's premise rests on a claim that `SubagentsSettingsView` is *"read via `SettingsManager::effective().get(\"subagents\")`"*. **That read does not exist and never has**: `rg 'SettingsManager|effective\(\)' crates/cyrup-ext-subagents/src` matches PROSE ONLY — a stale doc comment on the struct — and the struct's only data-carrying constructor is `SubagentsSettingsView::from_subagent_settings`, whose input is the discovery-side result. The item's own cyrup citation is wrong too: `registration/mod.rs:625-640` is `ProactiveSkillSubagents`, not a settings read. **What actually exists is pi's own arrangement**: `~/.cyrup/agents/settings.json` (user) and `<project_root>/.cyrup/agents/settings.json` (project), layered by `discovery::load_layered_subagent_settings` with **project beating user** on every scalar and every per-agent override — pi resolves `defaultModel` at `agents/agents.ts:924-931` @v0.43.0, `defaultThinking` at `:949-951`, `defaultExtensions` at `:969-971`, from `getUserAgentSettingsPath()` (`:674-676`) and `getProjectAgentSettingsPath(cwd)` (`:678-681`). So there is nothing to merge and no precedence to invent, and the item's Verify (*"set the same key to different values in both files, assert the documented winner"*) is ALREADY pinned at `discovery/mod.rs:2008` and `:2025`. `~/.cyrup/agent/settings.json` — `cyrup_config::Dirs::settings_path()` — is a different file this crate never reads; `profiles.rs:227-232`, the in-source note the filing cites, says exactly that, and it is a record of a store-based writer being DELETED for aiming at it, i.e. the divergence being closed, not open. **What was fixed is the documentation that caused the filing**: the stale sentence is deleted rather than softened (a doc describing a read path that does not exist is indistinguishable, to a reader, from one that does), the struct doc now names the real pair with its upstream citation, and `registration/mod.rs`'s R-SA-133 tier-2 header no longer says *"`cyrup-config`'s effective (CLI ▷ project ▷ global) settings view"*. **No test was added and the reason is stated rather than papered over: there is no defect to make red.** |

**45 items — 0 critical, 2 high, 23 medium, 20 low.** Per structural defect A in
`00-residual-ledger.md`, treat the count as a floor.

> **SWEEP 11, 2026-08-15 — the steering/guide/settings cluster.** Five rows moved: `SUBA-055` and
> `SUBA-066` closed together (the `guide` action, a packaged `resources/docs/` set embedded with
> `include_str!`, and the slash command over the same reader); `SUBA-049` partially closed (the
> acknowledgment path, the delivery `mode`, and the capability record are in end to end — a steer is
> no longer fire-and-forget — with `steeringRecovery` handed back as **L**, not the **M** it was
> filed at, and deliberately NOT advertised on the schema until it has a consumer); and **two were
> REFUTED against the code at HEAD** — `SUBA-034` (the `CompletionBus` wake landed in sweep 9 and the
> row was never marked) and `SUBA-071` (the "two settings files" premise cites a
> `SettingsManager::effective()` read that exists nowhere in the crate; the real arrangement is pi's
> own user◁project pair, already layered and already tested). That is **2 refutations in 5 rows**,
> against this ledger's published ~12% error rate — both were checkable in under a minute with `rg`.
>
> **NOT TAKEN in this cluster, with measured sizes rather than stubs**, because each is a subsystem
> and not a fix: `SUBA-056` (completion replay + output archives — upstream is `completion-replay.ts`
> 267 lines + `wait-completions.ts` 146, plus a `wait`-result projection: **L**), `SUBA-017`
> (completion batching: **M**), `SUBA-061` (four config keys, three of which need a consumer built
> first: **M**), `SUBA-063` (zero-tool-budget auth + runtime-extension acknowledgement: **M**),
> `SUBA-070` (`interactive` enforcement — needs its upstream tag/line ESTABLISHED first, which the
> row itself says was never read: **M**), `SUBA-022` (`api/delegation.ts`: **L**), `SUBA-024`
> (`parallel-handoff.ts` + `agent-contract.ts`: **L**), `SUBA-026` (`slash/subagents-admin.ts`, 432
> lines, + `tui/selector.ts`: **L**). None of them was stubbed, half-wired or advertised.
>
> **One documentation guarantee this sweep took on:** the packaged `resources/docs/` set is
> load-bearing now — it is what `{action:"guide"}` returns — so it must not describe behaviour this
> build lacks. Draft text claiming completion replay and notice batching was removed before landing
> for exactly that reason, and `the_tool_reference_topic_names_every_dispatched_verb` makes the
> verb half of that guarantee mechanical.

## Trackers (excluded from the item count)

These keep their IDs and their bodies but propose no schedulable work: they are indexes over work
other items own. A planner should not pick one up; the next audit pass maintains them.

| ID | Kind | Owner of the actual work | Note |
|---|---|---|---|
| SUBA-005 | tracking | the subsystem items it indexes (SUBA-016, SUBA-046, SUBA-055, SUBA-057, VL-S6, VL-S13, and now SUBA-085 in `09a`) plus two still-unowned verbs | **Counts re-derived 2026-09-04 against cyrup HEAD `2571969` and upstream `v0.57.0`** (independently counted, not copied from `09a`'s own re-measurement, which agrees): cyrup's `SUBAGENT_ACTIONS` (`crates/cyrup-ext-subagents/src/extension/tool/text.rs:187-230`) advertises **30** verbs against upstream's **52** (`git show v0.57.0:src/shared/types.ts:2397`) — **23 unadvertised**: `debug.run`, `inspector.{close,open,status}`, `mission.resolve-decision`, `project.{close,open,status}`, `refine`, `refine.rollback`, `refine.show`, `schedule.{create,delete,history,list,pause,resume,run,run-due,show}`, `validate`, `worktree.discard`. **`append-step` is cyrup-only** — upstream deleted it from the array between v0.47.1 and v0.57.0 (commit `7ece6f35`, per `09a`'s "Already tracked" table), so it advertises a verb upstream no longer has; not a defect, just stale relative to the old 27-vs-50 framing. Of the 23, all but two now have a named owner: `schedule.*` (9) → `SUBA-016`; `refine`/`refine.show`/`refine.rollback` → `VL-S13`+`VL-S11`; the six `inspector.*`/`project.*` verbs → `VL-S6`+`PB-8`; `worktree.discard` → `SUBA-024`/`VL-S10`/`SUBA-064`; **`mission.resolve-decision` → `SUBA-085`, filed and left open in `09a-cyrup-ext-subagents-v0.57-drift.md` this pass.** The original "seven unowned verbs" framing is stale in two more ways: `approve-checkpoint` and `reject-checkpoint` no longer exist at v0.57.0 either (same commit), so they were dropped from cyrup's list along with upstream's rather than being a residual gap. **Genuinely unowned as of this pass: `debug.run` and `validate`** — neither is named by any open item in area 09 or 09a; `debug.run` is on `09a`'s own "cut at the twenty-item cap" list (confirmed absent, filed next pass) but has no id yet. What this tracker still owes: (a) an owner for `debug.run` and `validate`, and (b) the completeness assertion pinning the enum against a checked-in copy of upstream's array, unchanged from the original filing. — **2026-09-19: `children.list` leaves this list** — ported at pi's index (`SUBA-055`'s residual, closed above); the "no retained-child concept" reason it was parked under was stale, see that row. **Same day: `debug.run` leaves it too** — `background/run_lifecycle_debug.rs` + `SubagentExecutor::control_debug_run`, at pi's index directly after `status`, printing the pid-probe substitute for the process-terminal proof honestly while VL-S4 was open. (**VL-S4 closed 2026-09-20**: the verb now prints upstream's real `Process terminal file:` / `Status process terminal:` / `Sidecar process terminal:` trio and the substitute line is deleted.) Of the "genuinely unowned" pair, `validate` landed under WORKFLOW_2 (`schema.rs`'s index note) and `debug.run` here; nothing on this row is unowned now. |

---

## SUBA-014 — `requireReadTool` unported, so a skill-carrying agent can be told to `read` a skill it has no `read` tool for

**Kind** not-ported · **Severity** high *(raised from medium this pass)* · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/exec/mod.rs:1463-1491` builds the allowlist verbatim from declared builtins plus resolved MCP tool names, with no `read` head-injection; `rg 'require_read_tool' crates/cyrup-ext-subagents/src` = 0. The same block is SUBA-006's `explicit_tool_allowlist` fix, read in full — the injection is genuinely absent from it.
**upstream** — `pi-subagents/src/runs/shared/pi-args.ts:355-372` @v0.43.0 injects `["read", ...requestedBuiltinTools]` whenever `requireReadTool && requestedBuiltinTools.length > 0 && !includes("read")`. `git grep requireReadTool v0.43.0` shows **seven** live setters, all deriving it from `Boolean(resolvedSkills.length)`: `async-execution.ts:731,1324`, `subagent-runner.ts:1328,1366`, `execution.ts:322,357`, `preflight.ts:277`.
**Impact** — Severity raised because cyrup's own proactive-skill block tells the child *"Use the read tool to load a skill's file"* (`discovery/skills.rs:273`) while the allowlist may not contain `read`. An agent with an explicit `tools:` list that omits `read` plus any resolved skill silently cannot load that skill: the child is instructed to do something it has no tool for, and the failure surfaces as a model apology rather than a config error.
**Fix** — In `exec/mod.rs:1463-1491`, compute `require_read_tool` from `!resolved_skills.is_empty()` (the value is already in scope at the skill-resolution site) and inject `"read"` at the head of the builtin list under pi's exact three-way condition. Add the config/param surface only if a caller needs to force it independently of skills — upstream never sets it any other way.
**Verify** — An agent with `tools: [bash]` and one resolved skill must spawn with `--tools read,bash`; the same agent with no skills must spawn with `--tools bash`. Table-test the three-way condition including the already-contains-`read` case.

## SUBA-043 — A SINGLE-mode subagent call can never declare `outputSchema` — the param is unadvertised and the field is hardcoded `None` on both single paths

**Kind** not-ported · **Severity** high · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/extension.rs:6543-6690`: `subagent_tool_parameters()` emits 45 properties and `outputSchema` is not among them — it exists only on the `tasks[]`/`chain[]` **item** schemas (`:6300`, `:6326`, `:6371`, `:6389`). Both single-run construction sites pin the field: `structured_output_schema: None` at `:1934` (foreground SINGLE `RunOptions`) and at `:2295` (async SINGLE `SingleStepSpec`, where only `output_path`/`output_mode`/`skills`/`session_dir` are threaded). The only populated path is the `tasks[]` item lowering at `:5849`.
**upstream** — `pi-subagents/src/extension/schemas.ts:349` @v0.43.0 — `outputSchema: Type.Optional(JsonSchemaObject)` is a **top-level** `SubagentParamsSchema` property, under the "Workflow defaults forwarded to each child" comment; `runs/foreground/subagent-executor.ts:3515` (`runSinglePath`) reads `params.outputSchema` at `:3651` and `:3671`, and `pi-args.ts:759-762` writes the resulting schema/capture pair into the child env.
**Impact** — The mechanism SUBA-S01 was closed to deliver is unreachable from the surface a model actually calls. `subagent({agent, task, outputSchema:{…}})` is accepted (the root schema is `additionalProperties: true` and `SubagentToolParams` has no `deny_unknown_fields`), the schema is dropped without error, the run completes as free prose, and the orchestrator that asked for typed output gets untyped text. The only workaround is wrapping a one-item `tasks:[{…}]` — the exact SUBA-041-era shape the maintainer rejected as a workaround.
**Fix** — Add `outputSchema` to `subagent_tool_parameters()` using the existing `sj_json_schema_object()` helper the item schemas already use, deserialize it onto `SubagentToolParams`, and thread it into `RunOptions::structured_output_schema` at `extension.rs:1934` and onto the async step at `:2295` (the runner already carries the field; only the constructors pin it).
**Verify** — `{agent:"x", task:"y", outputSchema:{type:"object",properties:{n:{type:"number"}},required:["n"]}}` must set both structured-output env vars on the child, and a child that never calls `structured_output` must fail with `STRUCTURED_OUTPUT_MISSING_ERROR` rather than returning prose. Add the schema/dispatch guard test asserting every advertised property has a consumer — the single test that would have caught this, SUBA-N05 and SUBA-047 together.

## SUBA-005 — Management actions: 27 advertised against upstream's 50 (v0.43.0) / 53 (v0.47.1)

**`tracker`** — not counted in this area's 45 open items. **Kind** tracking · **Severity** n/a *(was medium)* · **Effort** n/a · **Confidence** confirmed

> **Reclassified in the 2026-08-12 repair pass.** The item's own Fix line reads "this item is the
> ledger, not the work", which is the definition of bookkeeping: every schedulable half belongs to a
> subsystem item (`SUBA-016`, `SUBA-046`, `SUBA-055`, `SUBA-057`, PARITY-GAPS VL-S6/VL-S13), and the
> two things this ID actually owes — owners for the seven unowned verbs, and a completeness assertion
> — are maintenance the next audit pass performs. Body retained unchanged below so the census is not
> lost; the ID is retained per the never-delete rule.

*(Restated: the prior "15 of 20" framing was stale in both directions. Counts below were re-derived by enumerating both arrays element by element.)*
**cyrup** — `crates/cyrup-ext-subagents/src/extension.rs:6557` — the action enum lists **27** verbs.
**upstream** — `pi-subagents/src/shared/types.ts:1885` @v0.43.0 has **50**; `:1968` @v0.47.1 has **53**.
**Impact** — Set-differencing the two arrays: missing **and covered by another item** — `schedule.*` (9, SUBA-016 / PARITY-GAPS PB-11), `refine*` (3, VL-S13), `inspector.*` (3, VL-S6). Missing and covered by **no** item until this pass — `children.list`, `worktree.discard`, `grant-spawn-budget`, `approve-checkpoint`, `reject-checkpoint`, `project.open`/`project.status`/`project.close`, `guide`, `dismiss`, `mission.resolve-decision`; all confirmed zero-hit by `rg` over `crates/cyrup-ext-subagents/src`. Four of those now have owners: **SUBA-046** (`grant-spawn-budget`), **SUBA-055** (`guide`, `children.list`), **SUBA-057** (`dismiss`). The rest are unowned. Because `SubagentToolParams` carries no `deny_unknown_fields`, accompanying params are dropped *before* the unknown-action error fires, so the error never explains what was actually wrong.
**Fix** — This item is the ledger, not the work: each subsystem absorbs its own dispatch half (enum entry at `extension.rs:6557`, arm in `route_management_action`/`route_control_action`, and the completeness assertion). What is owed *here* is (a) owners for the seven still-unowned verbs, and (b) restoring a completeness assertion that pins the enum against a checked-in copy of upstream's array so the count cannot silently drift again.
**Verify** — The completeness assertion holds the same name vector as `shared/types.ts` at the pinned tag, and every advertised verb dispatches to something other than the unknown-action arm.

## SUBA-008 — `turnBudget` unported; the only consumers read a hard-coded `false` — **CLOSED 2026-08-14**

> **CLOSED 2026-08-14 (sweep 8). FOUR ERRORS IN THE BODY BELOW ARE CORRECTED HERE FIRST, because
> following one of them would have shipped a budget that nothing enforced.**
>
> **(1) The `Fix` line's mechanism is WRONG, and it is the most valuable finding on this row.** It
> says *"Port as `exec/turn_budget.rs` mirroring `exec/tool_budget.rs`'s env-handoff shape"*. The tool
> budget is env-var plus **child-side refusal** (`tool-budget.ts:70-80`, `PI_SUBAGENT_TOOL_BUDGET`).
> **The turn budget is the OPPOSITE shape.** `git grep -n TURN_BUDGET v0.43.0 -- src/` matches **only
> `runs/shared/turn-budget.ts` itself** — there is no env var and no child-side enforcement. The child
> is only **told**, via a system-prompt block; the **SUPERVISOR** enforces, by counting assistant
> `message_end` events off the child's NDJSON stdout and signalling it down
> (`foreground/execution.ts:910-924`, `:733-757`). A faithful-*looking* env-shaped port would have
> advertised a budget nobody enforced.
>
> **(2) The `Verify` line's key is WRONG.** It says `turnBudget:{hard:2}`. **`hard` is the TOOL
> budget's key.** The turn budget takes `{maxTurns, graceTurns}` (`extension/schemas.ts:104-107`
> @v0.43.0), and upstream **rejects `hard` by name** — `turnBudget.hard is not supported.`
>
> **(3) The effort rating was WRONG, and sweep 6's recount doubled down on it** ("the cheapest
> remaining medium in this area … WIRING plus a schema key, not a port"). Measurably false: **cyrup's
> `Usage` carries no `turns` counter at all** — `exec/fallback.rs:919` says so in its own doc — so
> there was no turn count to wire. **Re-rated M/L.**
>
> **(4) The "three consumers reading a hard-coded `false`" count was WRONG, and the miscount hid a
> quieter failure.** Only **two** were reachable (`tui/intercom.rs:355`, `:448`).
> `exec/fallback.rs:915` was **already reading the field correctly** and merely had no producer —
> which means `is_retryable_subagent_startup_failure` would have **RELAUNCHED the very model that
> blew its turn budget**. A correct consumer starved of a value is a different and quieter defect
> than a consumer reading a constant.
>
> **WHAT LANDED.** New `crates/cyrup-ext-subagents/src/exec/turn_budget.rs` (888 lines, verified at
> HEAD) plus its wiring. **Module:** `resolve_turn_budget_config` (upstream's verbatim error strings,
> its first-unknown-key scan, its `?? DEFAULT_TURN_BUDGET_GRACE_TURNS`, and `Number.isInteger(2.0)
> === true` so a fractionless float is accepted as upstream accepts it),
> `append_turn_budget_system_prompt`, `turn_budget_soft_note` / `_exceeded_message` /
> `_deferred_note`, `format_turn_budget_output`, `prepend_turn_budget_note`,
> `initial_turn_budget_state` / `turn_budget_state` / `turn_budget_deferred_state`,
> `turn_budget_decision`, and a `TurnBudgetTracker` gathering pi's four `updateTurnBudget` locals.
> **Enforcement (`exec/mod.rs`):** `drive_attempt` counts assistant `message_end` events
> (`progress.turn_count()` is this port's `result.usage.turns`), computes pi's
> `terminalAssistantStop || terminalStructuredOutputCall` and `hasToolCall ||
> Boolean(progress.currentTool)`, raises the one-time soft note into `progress.recent_output`, and on
> abort walks `SpawnedChild::terminate_with_graces`. **Composition:** the budget block is appended
> LAST onto the system prompt, after persona→skills→memory→refinement→output-path, matching
> `execution.ts:326` reading `shared.systemPrompt` from `:1443`. The abort message is the run's error
> at the TOP of the diagnosis chain (upstream sets `result.error` at abort time and its close handler
> only fills an unset one, `:1099`). The terminal output fold is pi's `else if` chain off `if
> (result.timedOut)`. **Surface:** `turnBudget` tool param + `TurnBudgetOverride` schema (in
> upstream's own property slot, immediately above `toolBudget`), `turnBudget:` agent frontmatter →
> `AgentDefinition::default_turn_budget` (+ `KNOWN_FIELDS` + a `serialize_agent` arm, so a management
> rewrite cannot silently delete an author's budget), and the `subagents.turnBudget` config key,
> resolving caller > frontmatter > config at one point per path (`subagent-executor.ts:4928` after
> `applySingleAgentLaunchDefaults`). **Result shape:** `SingleResult` gained `turn_budget` /
> `turn_budget_exceeded` / `wrap_up_requested`, all `skip_serializing_if`, so pre-existing
> `status.json` round-trips byte-for-byte.
>
> **`[CYRUP-DELTA]`, recorded at the call site rather than silently matching the observable
> outcome.** pi ARMS two `setTimeout`s inside `requestTurnBudgetAbort` and **keeps reading the
> child's stdout during the window**, so a child that wraps up inside it still delivers output.
> cyrup's `SpawnedChild::terminate` **CONSUMES** the child — there is no seam that signals without
> taking it — so the ladder blocks the drive loop for the same wall-clock window instead. A late final
> message written after the SIGINT is **dropped here where upstream would have read it**, which is why
> the abort message doubles as `final_output`. **The graces are pinned to reproduce upstream's
> ABSOLUTE instants: SIGINT, SIGTERM at +1 s, SIGKILL at +4 s** — pi arms both timers from the same
> moment, so the real SIGTERM→SIGKILL gap is **3 s, not 4 s**. Reading `execution.ts:752` alone gives
> the wrong number.
>
> **VERIFY — 14 new tests, all green.** `exec::turn_budget::tests` (10): the resolver's defaults and
> every verbatim rejection message; the system-prompt block's exact text with both pluralisations;
> the decision table including that a TERMINAL assistant stop is never aborted however far past the
> hard limit; the tracker's within-budget→wrap-up→abort walk asserting the soft note fires exactly
> ONCE and that `wrapUpRequestedAtTurn` (the THRESHOLD) differs from `exceededAtTurn` (the OBSERVED
> turn); a repeated deferral keeping the FIRST deferral turn; an unarmed tracker being wholly inert;
> the timed-out guard; and the wire shape omitting unset turn fields.
> `exec::tests::the_turn_budget_notice_reaches_the_child_through_the_spilled_system_prompt_file`
> reads back the SPILLED prompt file — **not the argv, which `SUBA-030` moved the persona off, so an
> argv assertion would have passed vacuously** — and asserts absence before presence.
> `extension::tests`: the schema shape incl. `graceTurns.minimum == 0` and upstream's verbatim
> description, plus the refusal path asserting `turnBudget.hard is not supported.`
> `discovery::management::tests::serialize_agent_round_trips_the_turn_budget_launch_default` pins the
> `KNOWN_FIELDS`/serializer pair against silent deletion. **cyrup-it (2, real OS subprocesses):**
> `a_turn_budget_wraps_up_at_max_turns_and_aborts_the_child_after_the_grace_turn` scripts a child that
> emits four NON-terminal turns then sleeps 30 s, and asserts the run ends on turn 3 with upstream's
> exact error string, `exceededAtTurn: 3` / `wrapUpRequestedAtTurn: 2`, the partial output preserved
> under upstream's heading, and that the post-sleep turn was NEVER observed;
> `a_child_that_finishes_inside_its_turn_budget_is_untouched` is the adversarial twin — the SAME
> budget must be completely inert for a child that stops on its own, which an always-abort bug fails.
>
> **ONE VERIFY NOT PERFORMED, stated rather than smoothed.** The abort is proven against a real OS
> subprocess, but **the soft-note text was never observed rendering in a live TUI.** Per this
> directory's standing rule that a UI surface is not done until it has been RUN, the
> `progress.recent_output` wrap-up note is **mechanism-verified, not eye-verified.**
>
> **Everything below is the filing text, retained for provenance and wrong on the four points above.**

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed
**cyrup** — `turn_budget` appears in exactly three files, all as consumers of a value with no source: `tui/intercom.rs:348-352` and `exec/mod.rs:2354-2360` (both with in-tree comments saying the flag has no producer) and `exec/fallback.rs`. No `turnBudget` key among the 45 properties at `extension.rs:6543-6690`.
**upstream** — `pi-subagents/src/runs/shared/turn-budget.ts` present at v0.43.0 and v0.47.1; `turnBudget: Type.Optional(TurnBudgetOverride)` is a top-level tool param (`extension/schemas.ts:353` @v0.43.0), and `appendTurnBudgetSystemPrompt` composes the budget notice into the child's system prompt.
**Impact** — No per-run turn cap, and the child is never told how many turns it has, so it cannot self-pace. Same unbounded-loop exposure as the tool budget, minus the enforcement half that SUBA-007 already landed. Duplicate of PARITY-GAPS PB-10 — fix once, close both.
**Fix** — Port as `exec/turn_budget.rs` mirroring `exec/tool_budget.rs`'s env-handoff shape, count assistant turns in the drive loop, wire the three existing consumers to the real value, and compose `appendTurnBudgetSystemPrompt` into the persona channel at `exec/mod.rs:1597-1608`. Add the schema key at the same time — do not repeat SUBA-047.
**Verify** — `turnBudget:{hard:2}` must terminate the run after two assistant turns with pi's budget-exhausted result shape, and the child's system prompt must contain the budget notice.

## ~~SUBA-016~~ — `scheduledRuns` unported (+ nine `schedule.*` verbs, not four) — **CLOSED 2026-09-16 (`7e41cf9`, PR #140)**

> **CLOSED 2026-09-16, cyrup `cc7818b` — see the table row for the full evidence. Everything below
> this box, INCLUDING the 2026-08-15 BLOCKED box, is the filing text and its history; both are kept
> because the blocker's refutation is the useful part.**
>
> **The blocker was real when written and is discharged, not refuted.** Sweep 9 was correct that a
> schedule's only legal target is `workflowScript` (`ScheduleTarget = { workflowScript: string; args;
> baseRef? }`, `scheduled-runs.ts:44` @v0.68.0 — still true, and `schedule.create` refuses `{agent,
> task}` BY NAME). It was correct that cyrup had no `workflowScript` runtime. What changed is that the
> runtime landed (`crates/cyrup-ext-subagents/src/workflows/`, 27 files / ~20k lines, advertised at
> `extension/tool/schema.rs:337`), and the port then followed it. The refusal is preserved verbatim,
> pinned by `schedule_create_refuses_a_non_workflow_script_target` (`scheduled_runs_tests.rs:122`),
> which asserts the exact sentence *"schedule.create requires workflowScript. Use workflowScript:
> \"return runs.run('main', { agent, task })\"."*.
>
> **The Verify line below is dead and stays dead.** Sweep 9 already flagged it: there is no `cron`
> parameter anywhere upstream (the triggers are `at` and `every`), and `{agent, task}` is refused.
> The executable form is the test module at `extension/tool/scheduled_runs_tests.rs`, whose own header
> states the bar this closure was held to — *"Every row here drives `Tool::execute` or the installed
> `ScheduledRunManager` — never a module function directly — because the defect this task exists to
> close is a well-tested surface that nothing could reach."*
>
> **Residual, recorded and deliberately NOT filed as a row.** `schedule.create` accepts and then
> REFUSES `baseRef` (`extension/tool/schema.rs:630`, test at `scheduled_runs_tests.rs:186`), where
> upstream HONOURS it through `normalizeWorktreeBaseRef` (`scheduled-runs.ts:289-297`, `:446-456`
> @v0.68.0). `baseRef` is absent from `scheduled-runs.ts` at **v0.43.0 and v0.47.1** and present only
> at v0.68.0, so it belongs to the `v0.57.0..v0.68.0` window that **no file in this directory owns** —
> `09a`'s scope line binds it to `v0.47.1..v0.57.0`. Minting an id for one arbitrary member of an
> unmeasured window would misrepresent that window as measured. It belongs to the `09b` the census
> above already argues for. Calendar triggers (`on`/`timezone`/`every:"day"`) are NOT a residual:
> upstream refuses them too, at `scheduled-runs.ts:622`, and cyrup reproduces the sentence.
>
> **BLOCKED 2026-08-15 (sweep 9) — see the table row for the full evidence. Everything below is the
> filing text and its Verify line is WRONG.** The nine-verb count is confirmed. The blocker is that
> a schedule's only legal target is `workflowScript` at **both** baselines
> (`ScheduleTarget = { workflowScript: string }`, `scheduled-runs.ts:38` @v0.43.0), and
> `workflowScript` is a 916-line `node:vm` JS sandbox (`workflows/scripted-workflow.ts:8,388,392`)
> that this crate documents as unported at `extension.rs:5990-6020`. The Fix line below —
> *"then the nine dispatch arms"* — is therefore the cheap half; the target runtime is the item.
> The Verify line below is **not executable against upstream**: `schedule.create` refuses
> `agent`/`task` outright, and there is no `cron` parameter anywhere in upstream — the triggers are
> `at` and `every`.

**Kind** not-ported · **Severity** medium · **Effort** L · **Confidence** confirmed
**cyrup** — Zero hits for `scheduled_runs` crate-wide; the 27-verb enum at `extension.rs:6557` has nothing beginning `schedule.`.
**upstream** — `pi-subagents/src/runs/background/scheduled-runs.ts` present at v0.43.0 and v0.47.1; **nine** verbs in `shared/types.ts:1968` — `schedule.create`, `.list`, `.show`, `.history`, `.pause`, `.resume`, `.run`, `.run-due`, `.delete`. The item's prior count of four was stale.
**Impact** — No recurring or deferred subagent runs. A caller attempting one gets the unknown-action error with its schedule parameters silently discarded first, so the error does not explain the failure. Duplicate of PARITY-GAPS PB-11.
**Fix** — Port the job store as `background/scheduled.rs` (persisted jobs under the subagents home plus a tick loop in the extension's background task), then the nine dispatch arms and the enum entries at `extension.rs:6557`.
**Verify** — `{action:"schedule.create", agent, task, cron}` then `{action:"schedule.list"}` must round-trip, and `schedule.run-due` must fire the job on its interval.

## SUBA-021 — `capability-ceiling` / `usage-budget` / `spawn-budget` unported — all three are in-baseline

**Kind** not-ported *(re-classified from `upstream-drift`)* · **Severity** medium *(raised from low)* · **Effort** L · **Confidence** confirmed
*(Corrected this pass. The refuter's caveat, stated inline: `launch-contract.ts` is struck not because upstream deleted it but because it is **absent at both tags at every path** — it historically lived at `src/shared/launch-contract.ts` — so it was never in either baseline and the item was wrong to name it.)*
**cyrup** — `rg 'capability_ceiling|usage_budget' crates/cyrup-ext-subagents/src` = 0. The only spawn-budget analogue is the per-session counter at `extension.rs:760` with no grant path, no snapshot, and no ceiling concept.
**upstream** — `capability-ceiling.ts`, `usage-budget.ts` and `spawn-budget.ts` all pass `git cat-file -e` at **both** v0.43.0 and v0.47.1, so the prior "post-baseline, out of scope" framing is dead — this is in-baseline unported work.
**Impact** — No pre-spawn capability ceiling and no usage budget: a child can be granted a capability set wider than its parent's, and there is no token/cost bound on a run beyond the model's own limits. `CAPABILITY_CEILING_V1` is separately tracked as PARITY-GAPS VL-S1 on the env-var surface.
**Fix** — Port `capability-ceiling.ts` and `usage-budget.ts` as `exec/capability_ceiling.rs` and `exec/usage_budget.rs`, consulted in `build_attempt_spawn_plan` beside the existing tool-budget encoder. The spawn-budget half is **SUBA-046** and should land first, since it is the one with a live user-facing dead end.
**Verify** — A child requesting a capability outside its parent's ceiling must be refused at preflight with pi's message; a run exceeding its usage budget must terminate with pi's budget result shape.

## SUBA-025 — `toolDescriptionMode`, the file override, and the mandatory safety-guidance appender unported

**Kind** not-ported · **Severity** medium *(raised from low)* · **Effort** S · **Confidence** confirmed
**cyrup** — `rg 'description_mode|toolDescriptionMode' crates/cyrup-ext-subagents/src` = 0. Descriptions are code constants selected by registration mode. `rg 'SAFETY-CRITICAL' extension.rs` hits only unrelated R-SA-055 depth-guard doc comments.
**upstream** — `pi-subagents/src/extension/tool-description.ts` present at v0.43.0 with **three** surfaces cyrup has none of: `resolveToolDescriptionMode` (`:68`); a user/project `subagent-tool-description.md` override capped at `CUSTOM_TOOL_DESCRIPTION_MAX_BYTES = 50 * 1024` (`:6-7`, `:80-81`); and `withMandatorySafetyGuidance` (`:144`) which appends `SUBAGENT_SAFETY_GUIDANCE` (`:9`). Refuter's precision note, stated inline: `withMandatorySafetyGuidance` is applied on the **`custom` branch only** (`:160`) — for `full`/`compact` the guidance is baked into the constants — so "every description including custom ones" overstates it; the load-bearing case is that a deployment *can* replace the description and cannot drop the safety guidance.
**Impact** — Deployments cannot trim the (long) subagent tool description to save context, cannot steer the orchestrator with a project-specific description, and — the reason for the severity raise — there is no mechanism guaranteeing the safety guidance survives a custom description, because there is no custom-description path at all. Severity medium rather than low because this is the surface `SUBA-046`'s advertise-vs-refuse defect and `SUBA-061`'s `legacyChainControls` both attach to.
**Fix** — Add `toolDescriptionMode` to `SubagentExtensionConfig`, resolve the description at registration rather than selecting a constant, port the 50 KiB-capped file override with the same search order, and port `withMandatorySafetyGuidance` applied to the custom branch.
**Verify** — With `compact` configured, the registered description must be the short form for both `subagent` and `wait`; with a `subagent-tool-description.md` present, the registered description must be its contents **plus** the safety guidance; a 60 KiB override must be rejected with pi's error.

## SUBA-028 — Acceptance verification cannot be aborted

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/exec/acceptance.rs:1337-1352`: `evaluate_acceptance(contract, gate, final_output, completion_guard, verify_cwd, memo, file_output)` takes no cancellation argument, and the live `model::EvaluateAcceptanceInput` (`:6964-6989`) has no `signal` field either. Grep for `signal`/`cancel`/`abort` in the file returns only `has_generic_acceptance_report_signal` (`:5355`), an unrelated JSON-shape helper.
**upstream** — `pi-subagents/src/runs/shared/acceptance.ts:1073` @v0.43.0 takes `signal?: AbortSignal`; `:1181-1182` is `if (options.signal?.aborted) abortVerification(); else addEventListener(...)`; `:1290` passes `signal: input.signal` down and `:1295` breaks the command loop on `input.signal?.aborted`.
**Impact** — Cancelling a subagent run (Ctrl-C, orchestrator cancel, parent timeout) does not stop acceptance verification; the caller can wait a full per-command timeout after asking to stop. SUBA-027's fix means the timed-out child is now killed, so the leak is gone — the latency is not.
**Fix** — Thread `CancelToken` from the `exec/mod.rs` caller into `evaluate_acceptance` → `run_verify_commands_memoized` → `model::run_memoized_verify_command`, check it before each command in the loop, and `select!` it against the per-command wait alongside the existing timeout. Add the field to `model::EvaluateAcceptanceInput` so both entry shapes carry it.
**Verify** — Start a run whose verify command sleeps 60 s, cancel after 1 s; `evaluate_acceptance` must return within ~1 s and the child must be gone.

## SUBA-030 — Persona passed inline on argv; task spill written with the default umask under a doc asserting 0600 — **CLOSED 2026-08-14 (already-done)**

> **CLOSED 2026-08-14 (sweep 8) as already-done — see the table row for the evidence. Everything
> below is the filing text.** Both halves are in at HEAD: `spawn/mod.rs:262-289` spills the composed
> persona to a `0600` `<stem>.md`, and `exec/mod.rs:1802` calls it, pushing the flag and the PATH as
> two argv elements with an in-source block at `:1783-1801` naming the `/proc/<pid>/cmdline`
> disclosure half and the `MAX_ARG_STRLEN`/E2BIG availability half by name.


**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed (E2BIG half reasoned, not observed)
**cyrup** — Both halves confirmed at HEAD. **Persona:** `crates/cyrup-ext-subagents/src/exec/mod.rs:1597-1608` pushes `format!("{flag}={persona_body}")` as one argv element with no size check; the in-tree comment explains that the `=`-form is required by clap and says nothing about length. **Task spill:** `crates/cyrup-ext-subagents/src/spawn/mod.rs:248-260` — `resolve_task_arg` is a plain `std::fs::write(&path, task)` with the default umask, while `spawn/mod.rs:428`'s own doc calls these *"the 0600 task/system-prompt temp files"*. The code's documentation asserts a mode the code never sets; that internal contradiction is the strongest evidence in this item.
**upstream** — `pi-subagents/src/runs/shared/pi-args.ts:571-593` @v0.43.0 writes **both** the system prompt and the task overflow with `{ mode: 0o600 }` into an `mkdtempSync` dir and passes paths.
**Impact** — (a) Any local user can read a subagent's full persona from `/proc/<pid>/cmdline`, and the spilled task file is world-readable; personas routinely carry project context and occasionally credential-adjacent instructions. (b) A persona above Linux `MAX_ARG_STRLEN` (131072) makes `execve` fail with E2BIG and the spawn dies with an opaque OS error rather than a diagnosable message.
**Fix** — Set mode `0600` unconditionally on the task spill at `spawn/mod.rs:248-260` (two lines, and it makes the existing doc true). For the persona, add the same threshold guard the task path already has — above a limit, write to a 0600 file in a per-run `mkdtemp` dir and pass a path — which first requires teaching `crates/cyrup/src/cli.rs` to accept a path form for `--system-prompt`.
**Verify** — `stat -c %a` on the task spill must be `600`. Spawn with a 200 KB persona: the run must succeed and `/proc/<child>/cmdline` must not contain the body.

## SUBA-031 — `wait` scopes runs by cwd, not by session, and says "in this session"

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/background/wait.rs:42-48` still states the delta in-tree ("pi filters runs by `state.currentSessionId`; cyrup's `RunStatus` carries no session id"), and the empty-set message at `:325-330` reads *"No active async runs in this session. Nothing to wait for."* while the actual scope is the cwd. The in-tree comment and the user-facing string disagree with each other.
**upstream** — `pi-subagents/src/runs/background/wait.ts` passes `sessionId` from `state.currentSessionId`; `subagent-wait.ts` @v0.43.0 keeps the same filter.
**Impact** — Two cyrup sessions in the same repo see each other's background runs. `wait {all:true}` in session A blocks on session B's runs and reports their results; a stalled run in an unrelated session hangs an unrelated wait — and the message tells the user the opposite of what happened.
**Fix** — Record a session id on `RunStatus` at spawn and filter `list_active_runs` by it (or add a session component to the artifact roots). Fix the empty-set string in the same change so it matches whichever scope survives.
**Verify** — Two sessions, same cwd, one background run each: `wait {all:true}` in each must return only its own run.

## SUBA-032 — Notice-debounce test asserts a wall-clock outcome with ~15 ms margin

**Kind** test-defect · **Severity** medium · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/tui/notices.rs:925-943`: three real-clock `tokio::time::sleep(Duration::from_millis(20/45/40))` calls around a 60 ms debounce. The load-bearing assertion lands 15 ms inside the deadline, and overshoot on the *second* sleep is fatal. `start_paused` / `time::pause` / `time::advance` are zero-hit across the whole crate.
**upstream** — No counterpart; the debounce is cyrup-original over `pi-subagents/src/tui/render.ts`-derived surfaces. The in-repo precedent is commit `1806375`, which removed a structurally identical assertion from `cyrup-ext/src/caps/proc.rs`.
**Impact** — On a loaded CI box the second sleep overshoots and the test flakes; flaky tests get `#[ignore]`d and the debounce loses coverage entirely.
**Fix** — `#[tokio::test(start_paused = true)]` plus `tokio::time::advance` for each interval, making the assertion exact rather than marginal.
**Verify** — Deterministic under `--test-threads=1` on a machine loaded to 100% CPU, with runtime near zero once the clock is paused.

## SUBA-044 — cyrup's bundled `reviewer` agent still grants `bash`, `edit` and `write`; upstream made the reviewer lane read-only

**Kind** upstream-drift · **Severity** medium *(corrected down from the auditor's high — see caveat)* · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/resources/agents/reviewer.md:4` — `tools: read, grep, find, ls, bash, edit, write, intercom`, with `defaultReads: plan.md, progress.md` at `:9` and a prose-only restraint at `:56` (*"Use `bash` only for read-only inspection…"*) over a grant that permits arbitrary shell and arbitrary writes. Registered as a builtin through `registration/resources.rs` / `discovery/mod.rs`.
**upstream** — `pi-subagents/agents/reviewer.md:4` @v0.47.1 — `tools: read, grep, find, ls, intercom`; `defaultReads` removed; the rule is now *"Do not use shell commands or write files. Report any test or Git command that a supervisor must run."* Changed by `0b1976b` ("fix: make reviewer lanes read-only by default", #1008), released v0.47.1. v0.43.0 still carried the write grant, so this is drift, not a stale port.
**Impact** — A user delegating to the shipped `reviewer` gets a lane that can edit the working tree and run shell during what pi users expect to be an inspection-only pass. **Refuter's caveat, stated inline:** severity is medium rather than high because cyrup's own shipped prose contradicts the "user believes it is read-only" premise — `reviewer.md:58` says *"Prefer small corrective edits over broad rewrites"*, i.e. this agent is documented to edit. The defect is a documented behavioural divergence from upstream, not a silent capability escalation.
**Fix** — Set `tools: read, grep, find, ls, intercom` in `resources/agents/reviewer.md`, drop `defaultReads`, and take upstream's two prose lines verbatim. **Second correction:** the "strict tool allowlist / does not inherit ambient extension tools" paragraph the audit attributed to a v0.47.1 addition in `agents/delegate.md:12-13` and `agents/worker.md:21-22` is present at **v0.43.0 too** — cyrup's copies lack an *in-baseline* paragraph, so port it as a not-ported gap in the same pass.
**Verify** — Diff every file under `crates/cyrup-ext-subagents/resources/agents/` against `git -C pi-subagents show v0.47.1:agents/<name>.md`; only the `researcher.md` divergence (SUBA-062) may remain, and only with a recorded `[CYRUP-DELTA]`.

## SUBA-045 — The child tool-availability diagnostic is entirely unported: a child that silently lacks a declared tool reports nothing

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/exec/mod.rs:1849-1855` writes only `CYRUP_SUBAGENT_REQUIRED_TOOLS` into the child env; there is no diagnostic-path var and `rg 'TOOL_DIAGNOSTIC|tool_diagnostic' crates/cyrup-ext-subagents/src` = 0. The single consumer of the required-tools list is the intercom fallback gate (`native_supervisor.rs:1639` `read_required_child_tools`, used at `:1742`), so **nothing compares required against available**.
**upstream** — `pi-subagents/src/runs/shared/tool-availability.ts` @v0.43.0 — `:6` `CHILD_TOOL_DIAGNOSTIC_PATH_ENV`, `:18-44` `writeChildToolDiagnostic` (child side: computes `missing` against a `PI_CORE_CHILD_TOOLS` floor, writes 0600 JSON, deletes the file when nothing is missing), `:47-61` `readChildToolDiagnostic`, `:68-70` the `missingMcpDirectTools` message. Written into the env at `pi-args.ts:610-616` beside the required-tools var; the child writes it from `subagent-prompt-runtime.ts:99`; the **parent** reads it back and folds it into the run's terminal error at `foreground/execution.ts:1072-1079` (`closeError = result.error ?? toolDiagnosticError ?? assistantError`) and `background/subagent-runner.ts:1442`.
**Impact** — An agent whose `tools:` names an MCP tool the child's host never registered (stopped MCP server, renamed tool, extension that failed to load) runs to completion producing a model apology instead of a diagnosis. Upstream turns exactly that case into the run's error text, naming the missing tools and distinguishing *"a host/pi-mcp-adapter registration problem, not a tool-call failure"*.
**Fix** — Add `exec/tool_availability.rs` porting the three functions; write `CYRUP_SUBAGENT_TOOL_DIAGNOSTIC_PATH` beside `CYRUP_SUBAGENT_REQUIRED_TOOLS` at `exec/mod.rs:1849`, pointing into the run's scratch dir; have `prompt_runtime.rs`'s init write the diagnostic from the live registry; read it back in `run_attempt`'s error composition so it takes precedence over the assistant error, matching `execution.ts:1079`.
**Verify** — Declare `tools: read, mcp__nonexistent__x` and run; the run must fail with text naming `mcp__nonexistent__x` as missing from the child registry. With all tools present, no diagnostic file may survive the run.

## SUBA-046 — `grant-spawn-budget` is unported *and advertised*, so an exhausted per-session spawn cap is terminal for the whole session

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed
**cyrup** — All six `grant-spawn-budget` hits under `crates/cyrup-ext-subagents/src` are prose: `extension.rs:5268-5273` states outright that *"cyrup does not implement a `grant-spawn-budget` action at all"*, while `:5290` and `:12915` reproduce pi's child-safe tool description **advertising the verb to the model**. The action enum at `:6557` does not contain it, so a model that reads the description and calls it lands on the unknown-action arm. The counter exists (`reserve_subagent_spawns`, `:760`; reset only at SessionStart, `:9422`) with no grant path and no snapshot in tool-result details (`rg 'spawnBudget' extension.rs` = 0).
**upstream** — `pi-subagents/src/runs/shared/spawn-budget.ts` @v0.43.0 — `:30` `getSpawnBudgetSnapshot`, `:50/:55` the two formatters, `:59` `preflightSpawnBudget`, `:73` `reserveSpawnBudget`, `:85` `preflightSpawnBudgetGrant`, `:107` `grantSpawnBudget`. Dispatch at `runs/foreground/subagent-executor.ts:4457-4505`: root-interactive only, requires a session id, refuses while children are queued/running, previews, consults `resolveAuthorityDecision({action:"spawnBudgetGrant"})`, then `ctx.ui.confirm`. Param `additional` at `extension/schemas.ts:283`. `grant-spawn-budget` is in `SUBAGENT_ACTIONS` at `shared/types.ts:1885` @v0.43.0 and in `MUTATING_MANAGEMENT_ACTIONS` at `subagent-executor.ts:167` @v0.47.1. `spawnBudget` is returned in `details` on every refusal.
**Impact** — Once `maxSubagentSpawnsPerSession` is reached, a cyrup session can do no further delegation until it is restarted; there is no in-session escape hatch and no visibility into remaining budget, because the snapshot is never reported. Upstream's design is that the cap is a speed bump with a confirmed grant behind it. cyrup additionally **advertises the verb while refusing it** — the SUBA-041 defect class recurring on the description surface.
**Fix** — Port `spawn-budget.ts` as `exec/spawn_budget.rs` around the existing counter (snapshot type, `preflight_spawn_budget_grant`, `grant_spawn_budget`); add `grant-spawn-budget` to the enum at `extension.rs:6557` plus the `additional` param; gate on root-interactive (`allow_mutating_management` + a host-services UI) and route the confirm through `HostServices` (`crates/cyrup-ext/src/host/services.rs`). Attach the snapshot to the budget-refusal `ToolError` details so the cap is observable even without the grant. Depends on **SUBA-064** for the authority gate; land the counter/snapshot half first and wire the authority consult when that lands.
**Verify** — Set the cap to 1, spawn once, then `{action:"grant-spawn-budget", additional:2}` from the root session must confirm and permit two more; the same call from a fanout child must be refused with *"available only from the root interactive parent session."*

## SUBA-047 — `toolBudget` is honoured but never advertised, so the model cannot set a per-run tool cap

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed
*(Residual of SUBA-007, which is otherwise closed.)*
**cyrup** — `crates/cyrup-ext-subagents/src/exec/tool_budget.rs` (388 lines) and the env hand-off at `exec/mod.rs:1837-1846` show the enforcement half is complete, and the frontmatter key is read at `discovery/frontmatter.rs:850` — but `toolBudget` is **not** among the 45 keys emitted by `subagent_tool_parameters()` (`extension.rs:6543-6690`), and `rg 'tool_budget' extension.rs` returns exactly one hit (`tool_budget: None` at `:18993`, a test fixture). The only way to set a budget is to edit the agent file on disk.
**upstream** — `pi-subagents/src/extension/schemas.ts:279` @v0.34.0 and `:354` @v0.43.0 — `toolBudget: Type.Optional(ToolBudgetOverride)` is a top-level param (shape at `:116-120`: `soft?`, `hard`, `block?`), and also a per-item override on `ParallelTaskSchema:148` and the dynamic template at `:178`. In-baseline since before the ported tag.
**Impact** — A caller cannot bound a single delegation's tool spend without editing the agent on disk, and a per-call budget passed by an orchestrator is silently discarded. This is the mirror of the closed SUBA-N06: honoured but unadvertised, so the capability exists and is unreachable. It also blocks the per-task `toolBudget` override on `tasks[]`, which cyrup's item schema likewise omits.
**Fix** — Add `toolBudget` to `subagent_tool_parameters()` and to the `tasks[]`/`chain[]` item schemas beside `acceptance`, deserialize onto `SubagentToolParams`, and lower it into `RunOptions`/`SingleStepSpec` so it reaches the existing encoder at `exec/mod.rs:1837`. Precedence must be caller > frontmatter > extension config, matching pi.
**Verify** — `{agent:"x", task:"y", toolBudget:{hard:3}}` against an agent that would make ten calls must stop after three with the budget message; the same override inside `tasks:[{…}]` must apply per task.

## SUBA-048 — The `artifactDir` config key is unported — `resolve_artifacts_dir` has no preference parameter, so "session" and "temp" are unreachable

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/artifacts.rs:170-186` — `resolve_artifacts_dir(session_file, project_cwd, temp_cwd)` takes three path arguments and no preference: a `Some(project_cwd)` always wins, else the session sibling, else temp. Its two production callers (`extension.rs:9403`, `tui/fleet.rs:1032`) pass paths only, and `tui/fleet.rs:1030` records the delta in-tree ("pi passes `state.artifactDirPreference ?? \"project\"`"). `SubagentExtensionConfig` (`registration/mod.rs`) has 18 fields and `artifact_dir` is not one.
**upstream** — `pi-subagents/src/shared/artifacts.ts:160-183` @v0.43.0 — `getArtifactsDir(sessionFile, projectCwd?, dirPreference: ArtifactDirPreference = "project")` with distinct `session`/`temp`/`project` arms and a `throw` on an unsupported value; `getChainRunsDir` (`:145-158`) takes the same preference. The key is on `ExtensionConfig` at `shared/types.ts:1777` and **validated at `extension/config.ts:9,22-24`, which throws** on anything outside `ARTIFACT_DIR_PREFERENCES`.
**Impact** — Setting `"artifactDir": "temp"` or `"session"` does nothing — every run writes `<cwd>/.cyrup-subagents/…` into the user's repository. Users who chose `temp` specifically to keep generated transcripts, inputs and outputs out of a git working tree get them written there anyway. Upstream **errors** on a bad value where cyrup silently ignores a good one. Distinct from PARITY-GAPS PB-13, which is about the chain-runs *default* root going to temp instead of the project.
**Fix** — Add `artifact_dir: Option<ArtifactDirPreference>` to `SubagentExtensionConfig` with pi's three-variant enum and its validation error, add the parameter to `artifacts::resolve_artifacts_dir` and `artifacts::chain_runs_dir`, and thread the config snapshot through both callers. Landing it alongside PB-13 makes `project` the correct default for both.
**Verify** — With `"artifactDir": "temp"`, a foreground run must leave `<cwd>/.cyrup-subagents` untouched; with `"session"` and an active session file, artifacts must land in that file's sibling `subagent-artifacts` dir; an invalid value must be rejected at config load.

## SUBA-049 — Steer acknowledgment, delivery `mode` and `steeringRecovery` are unported, so a queued steer is fire-and-forget — **PARTIALLY CLOSED 2026-08-15**

> **PARTIALLY CLOSED 2026-08-15 (sweep 11) — see the table row for the evidence.** The ack path,
> the delivery `mode` and the capability record are in end to end; a steer is no longer
> fire-and-forget. **`steeringRecovery` is NOT ported and is not advertised**: it is `steering.ts`
> plus the pause-and-revive half of `async-steering-action.ts` (~450 upstream lines and a new
> `status.json` sub-record), i.e. **L**, not the **M** this item was filed at. The sequencing note
> below still stands and was NOT discharged: cyrup's inbox targets the v0.43.0 shape and the
> v0.44–v0.47 steering hardening (safety poll, settle fallback, `awaitingSettlement`) has still not
> been diffed line by line — schedule it WITH `steeringRecovery`, since both live in the same file.

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed
*(Residual of SUBA-013, which is otherwise closed.)*
**cyrup** — `rg 'STEER_ACK|steer_ack|STEER_CAPABILITY|steer_capability' crates/cyrup-ext-subagents/src` = 0. cyrup writes only `CYRUP_SUBAGENT_STEER_INBOX` (`exec/mod.rs:1857-1868`); the child-side inbox at `prompt_runtime.rs:157-290` consumes and deletes requests but writes no ack; the dispatch arm at `extension.rs:7825-7837` calls `control_steer` and returns its text with no ack poll. Neither `mode` nor `steeringRecovery` is among the 45 advertised params.
**upstream** — `pi-subagents/src/runs/shared/pi-args.ts:101-102` @v0.43.0 defines `SUBAGENT_STEER_CAPABILITY_ENV` and `SUBAGENT_STEER_ACK_DIR_ENV`, written at `:766-768`; the child acknowledges each request with `delivered`/`queued`/`failed` plus a message via `writeSteerAckAt(steerAckPathFromDir(ackDir, request.id), …)` (`subagent-prompt-runtime.ts:334-335` reads both vars; the `acknowledge` closure writes). Params `mode` (`steer`/`follow_up`/`auto`) and `steeringRecovery` are top-level at `extension/schemas.ts:283-284`.
**Impact** — `{action:"steer", id, message}` returns success as soon as the request file is written. The caller is never told whether the child took it, queued it behind a full follow-up queue (upstream caps at `MAX_STEER_QUEUE_SIZE` and answers `failed`), or could not deliver at all. A steer aimed at a child that is mid-tool and never reaches a turn boundary looks identical to one that landed. Without `mode` the caller cannot ask for the non-interrupting follow-up delivery upstream makes explicit.
**Fix** — Add an ack directory per run beside the existing inbox, export `CYRUP_SUBAGENT_STEER_ACK_DIR` and the capability path from `build_attempt_spawn_plan`, have `prompt_runtime::SteeringInbox` write an ack record per consumed/failed request, and have the `steer` dispatch arm poll for the ack before answering. Add `mode` and `steeringRecovery` to the schema with upstream's descriptions verbatim. **Sequencing note:** cyrup's inbox targets the v0.43.0 shape and upstream hardened steering across v0.44–v0.47 (safety poll, settle fallback, `awaitingSettlement`); that drift is an unfiled blind spot recorded in `## Coverage` and should be diffed in the same pass.
**Verify** — Steer a child whose follow-up queue is full; the tool must answer `failed` with upstream's *"Follow-up queue is full (N messages)."* text rather than success. Steer with `mode:"follow_up"` and assert the child receives it at the next turn boundary, not mid-turn.

## SUBA-050 — `subagents.modelScope.strict` is unported, so inherited and fallback out-of-scope models can never be hard-rejected

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/exec/model_scope.rs:43-50` — `ModelScopeConfig` has exactly two fields, `enforce` and `allow`; the severity decision at `:175-185` is an unconditional `match source { Explicit => Error, Inherited => Warn }` with no config input. `rg 'strict' exec/model_scope.rs` returns nothing.
**upstream** — `pi-subagents/src/runs/shared/model-scope.ts` @v0.47.1 — `:20` adds `strict?: boolean` ("Reject inherited and fallback models outside the allowlist instead of warning"), `:73` becomes `source === "explicit" || scope.strict === true ? "error" : "warn"`, and `:108-113` validates the key with a typed error. `git show v0.43.0:…/model-scope.ts | grep strict` is empty, confirming drift. Landed in `94b0cb1` ("feat: enforce strict subagent model scope", closes #995), released v0.47.0.
**Impact** — An operator who configures a model allowlist to keep subagents off expensive or non-compliant models cannot make it binding: an agent whose frontmatter names an out-of-scope model, or whose fallback ladder walks onto one, only warns and then runs on it. The policy is advisory for exactly the sources that are hardest to audit.
**Fix** — Add `strict: Option<bool>` to `ModelScopeConfig` with the same serde shape and validation error text, and change the severity computation at `:175-185` to `Explicit | (_ if strict) => Error`. The `Err(violation)` propagation path already exists from SUBA-003, so nothing downstream changes.
**Verify** — With `"modelScope": {"enforce": true, "strict": true, "allow": ["anthropic/*"]}`, an agent whose frontmatter names `openai/gpt-5` must fail the run with the out-of-scope error rather than warning and running.

## SUBA-051 — Async child runs have no default wall-clock timeout; upstream bounds every async CHILD at 30 minutes

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/background/runner_main.rs:310-330` — `timeout_ms: Option<u64>` and `deadline_at_ms: Option<u64>`, both serde-defaulted to `None` unless the caller supplied `timeoutMs`; `:1255` states in-tree that the **default** remains "no wall-clock timeout". The deadline arm at `:1653-1661` only exists when `deadline_at` is `Some`.
**upstream** — `pi-subagents/src/runs/background/async-execution.ts:131` @v0.47.1 — `export const DEFAULT_ASYNC_TIMEOUT_MS = 30 * 60 * 1000;` and `:782` `timeoutMs: a.defaultTimeoutMs ?? DEFAULT_ASYNC_TIMEOUT_MS` inside the child-step builder, with the composite **parent** deliberately left unbounded. Landed in `635c1bd` ("fix: add default async child timeouts", fixes #978), released v0.47.0; the split is echoed in the tool description.
**Impact** — A background child that wedges — a hung `cargo test`, a non-terminating model, a retry loop — burns tokens and CPU until a human notices and issues `interrupt`. The stopping machinery now exists in cyrup (SUBA-S03 closed); only the default is missing, so every async fan-out is unbounded by default exactly as it was before that fix.
**Fix** — Add `DEFAULT_ASYNC_CHILD_TIMEOUT_MS: u64 = 30 * 60 * 1000` and apply it when building each async **child** step (the `RunnerConfig`/step construction reached from `extension.rs:2295`), leaving composite parents (`chain`/`tasks`/graph roots) unbounded to match upstream's split. Do not apply it to foreground runs, which already have their own default.
**Verify** — `{agent, task, async:true}` with no `timeoutMs` against a non-terminating child must flip to `failed` with the timeout message after 30 minutes, while an async chain **parent** with running children must not.

## SUBA-052 — YAML literal block scalars (`|`, `|-`) in agent frontmatter parse to the literal string `"|"`

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed
*(Residual of SUBA-019, which is otherwise closed.)*
**cyrup** — `crates/cyrup-ext-subagents/src/discovery/frontmatter.rs:509-519` — the only block-scalar branch is `let is_folded = !is_quoted && (raw_value == ">" || raw_value == ">-");` followed by `if value.is_empty() || is_folded`. For `description: |`, `strip_matching_quotes` yields `(false, "|")`, which is neither empty nor folded, so `:518` executes `fields.push((key, "|"))`; the indented body lines then fail the `^([\w-]+):` match at `:503` and are silently discarded. The blank-line continuation predicate at `:503` (`current_folded && line.trim().is_empty()`) has no literal arm either.
**upstream** — `pi-subagents/src/agents/frontmatter.ts` @v0.47.1 — `currentLiteral` at `:86`, reset at `:113`, `const isLiteral = !isQuoted && (rawValue === "|" || rawValue === "|-")` at `:124`, folded into the defer condition at `:126` and into the blank-line continuation test at `:91`. `git show v0.43.0:…/frontmatter.ts | grep -n 'isLiteral\|currentLiteral'` is empty. Landed in `a4fc59a` ("fix: parse block scalar skill descriptions", #952), released v0.46.0, which also routed `parseSkillDescription` (`agents/skills.ts:398`) through the shared parser.
**Impact** — `|` is the most common YAML idiom for a multi-line description or prompt, and cyrup turns it into the one-character string `"|"` with the whole body dropped and no warning. An agent authored that way lists with a description of `|`, matches nothing in proactive-skill selection, and — for any multi-line key that feeds behaviour — runs with an empty value. Silent wrong value, not a hard error.
**Fix** — Mirror the upstream change in `parse_frontmatter_block`: add a `current_literal` flag beside `current_folded`, set it for `|`/`|-`, include it in the defer condition at `:513` and the blank-line continuation test at `:503`, and store the dedented block verbatim (no folding) on flush. Route skill-description parsing through the same parser as upstream did.
**Verify** — An agent whose `description: |` spans three indented lines must yield those lines joined by newlines, and `description: >` must still fold — one table test covering `|`, `|-`, `>`, `>-` and a plain scalar.

## SUBA-053 — `~` is never expanded in chain read/write paths, so `reads: ["~/notes.md"]` resolves to `<chain_dir>/~/notes.md`

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/spawn/chain_graph.rs:693-700` — `resolve_chain_path(file, chain_dir)` is `if file.is_absolute() { … } else { chain_dir.join(file) }` with no home expansion; `rg 'expand_home' crates/cyrup-ext-subagents/src` = 0. It feeds both the `[Read from: …]` prefix (`:730-736`) and the `[Write to: …]` line (`:737-739`).
**upstream** — `pi-subagents/src/shared/settings.ts` @v0.47.1 — `:341-345` `expandHomePath` (`~` → homedir, `~/x` → `join(homedir, x)`, `~user/` deliberately untouched), called at the head of `resolveChainPath` (`:351-354`) before the `isAbsolute` test. At v0.43.0 `resolveChainPath` (`:335`) had no expansion. Landed in `87420e5` ("fix(reads): expand home paths and wire reads into single runs"), released v0.45.0; the commit message names the exact symptom.
**Impact** — A chain step declaring `reads: ["~/.config/project.toml"]` gets an instruction pointing at a path that does not exist, so the child either reports the file missing or fabricates content; a `~`-prefixed `output` writes into a literal `~` directory under the chain dir. The failure is silent at the orchestrator — the instruction line looks well-formed.
**Fix** — Add an `expand_home` helper in `spawn/chain_graph.rs` (or `artifacts.rs` for reuse) matching upstream's three cases exactly, called at the head of `resolve_chain_path` before the `is_absolute` test. `~user/` must **not** be expanded, matching upstream.
**Verify** — Table-test `resolve_chain_path` over `~`, `~/`, `~/file`, `/abs/path`, `rel/path` and `~user/file` against upstream's eight cases in `test/unit/reads-resolution.test.ts`.

## SUBA-054 — `defaultReads` is parsed and rendered but never reaches a single run — no `[Read from: …]` instruction outside chains — **PARTIALLY CLOSED 2026-09-16**

> **PARTIALLY CLOSED 2026-09-16, cyrup `cc7818b`. Everything below is the filing text; its cyrup line
> is now half false and its Fix line was already corrected once (see the sweeps-1-and-2 block under
> `## Where the cross-file work lands`).**
>
> **IN — the foreground SINGLE half, which is the whole of the Verify line below.**
> `extension/executor/foreground.rs:827` passes `reads: agent.default_reads.clone()` into `RunOptions`
> (field declared at `exec/agent_config.rs:443`), and `build_task_text` (`exec/spawn_plan.rs:1414-1426`)
> prepends the instruction through `spawn::chain_graph::build_single_reads_instruction` (`:775-796`).
> Reusing the chain path's formatter rather than writing a second one means **`SUBA-053` (`~`
> expansion) and `SUBA-058` (the existence filter) are both discharged for this path** —
> `resolve_existing_read_paths` (`chain_graph.rs:767-773`) expands, resolves and filters, and an
> all-missing list emits no line at all rather than an empty one. Note also the separator: the SINGLE
> path ends `]\n\n`, the chain path joins with single newlines and adds the blank line once; both are
> upstream's and they are not interchangeable. Green:
> `build_task_text_prepends_the_default_reads_instruction_for_a_single_run` (`spawn_plan.rs:1991`),
> plus two siblings covering the existence filter and the all-missing case.
>
> **NOT IN — the async SINGLE half.** `reads: None` survives at two sites. The step-executor one
> STATES its blocker rather than defaulting silently (`background/runner_main/executor.rs:670-677`):
> a step dispatched through the runner already gets its line from `build_chain_instructions` resolved
> against the CHAIN dir, so populating `RunOptions::reads` here as well would emit it twice for every
> chain step; upstream's async single path resolves against `effectiveCwd`
> (`async-execution.ts:1300-1302`), so closing this means teaching the step BUILDER which of the two
> cwds applies — not setting that field. The constructor site is
> `extension/executor/background.rs:253`. **That is a decision, not effort, and it is what the row is
> now open for.**
>
> **Corrections to the filing text below, both already established and repeated here because the text
> is left as written:** (a) the Fix's "add the `reads` param on the async path" is wrong — upstream has
> NO top-level `reads` param (`extension/schemas.ts`'s `SubagentParamProperties` has no `reads` key;
> its three `reads` entries are all per-ITEM), so the persona's own `defaultReads` is the entire SINGLE
> precedence chain and no new advertised param is owed; (b) the cyrup line's `exec/mod.rs:3735` and
> `extension.rs:2310` citations predate the executor split and no longer resolve.
>
> **This half was already true at `d53763b`**, the pin at which the 2026-09-14 pass wrote "none
> closed" — see the 2026-09-16 block at the top of this file.

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/discovery/frontmatter.rs:782` parses `defaultReads` and `discovery/management.rs:779` renders it in agent listings, but `rg 'Read from' crates/cyrup-ext-subagents/src` matches only `spawn/chain_graph.rs:705,734` (the chain-instruction builder), `registration/slash_commands.rs:1255` (the `/run reads=` caller override) and `exec/task_intent.rs` (a stripper). `exec/mod.rs` never composes a read instruction — `default_reads` appears there once, at `:3735`, inside a test fixture — and the async `SingleStepSpec` at `extension.rs:2310` pins `reads: None`. So `defaultReads` is inert for every non-chain invocation.
**upstream** — `pi-subagents/src/runs/foreground/subagent-executor.ts:3867-3869` @v0.47.1 — *"Reads: caller override > agent defaultReads > none"* with `const reads = readsOverride !== undefined ? readsOverride : agentConfig.defaultReads ?? false;`, matched at `runs/background/async-execution.ts:1300-1302`; the instruction is built through `resolveExistingReadInstructionPaths` (`shared/settings.ts:356-362`). `git show v0.43.0:…/subagent-executor.ts | grep defaultReads` is empty. Landed in `87420e5`, released v0.45.0; the commit states that single-run launches "silently dropped a top-level `reads` value".
**Impact** — cyrup's own bundled `reviewer` ships `defaultReads: plan.md, progress.md` (`resources/agents/reviewer.md:9`) and, invoked as an ordinary single subagent, is never told to read either file — the frontmatter key is documentation. Any user agent relying on `defaultReads` to pre-seed context behaves as if the key were absent, and the failure looks like the model ignoring instructions.
**Fix** — Build the `[Read from: …]` prefix in `exec/mod.rs`'s task assembly (beside `build_task_text`) from `caller reads > agent.default_reads > none`, reusing `spawn/chain_graph.rs`'s formatter, and add the `reads` param on the async path. Land **SUBA-053** first so `~` paths resolve, and **SUBA-058**'s existence filter in the same pass. Note the interaction with SUBA-044: upstream removed `defaultReads` from `reviewer.md`, so fixing this without fixing that changes the bundled reviewer's behaviour.
**Verify** — Run the bundled `reviewer` against a repo containing `plan.md`; the child's task text must open with `[Read from: <abs>/plan.md]`. An agent with `defaultReads` plus an explicit caller `reads` must use the caller's list only.

## SUBA-055 — The `guide` action and the packaged version-matched docs it serves are unported — **CLOSED 2026-08-15**

> **CLOSED 2026-08-15 (sweep 11) — see the table row for the evidence.** `registration/guide.rs`
> plus a cyrup-authored `resources/docs/` set embedded with `include_str!`, the verb in pi's own
> enum position, and `topic` advertised exactly as pi declares it. The `children.list` half named in
> the Impact below is NOT ported — it lists retained children, part of the unported `workflowScript`
> shape — and returns to `SUBA-005`'s unowned-verb list.

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/extension.rs:6557` — the 27-verb enum has no `guide`; `rg 'children.list|"guide"' crates/cyrup-ext-subagents/src` = 0. The unknown-action arm at `:7610-7614` would answer `unknown subagent action 'guide'`. cyrup ships no `docs/` tree beside the crate's `resources/`.
**upstream** — `pi-subagents/src/extension/subagent-guide.ts` @v0.47.1 (new file; `git cat-file -e v0.43.0:…` fails) — `:5-16` `SUBAGENT_GUIDE_TOPICS` (overview, workflows, agents, missions, observability, tool-reference, configuration, models, watchdog, extension-api), `:26-38` `readSubagentGuide` resolving `README.md` for overview and `docs/<topic>.md` otherwise, with an explicit unknown-topic message. Dispatched at `runs/foreground/subagent-executor.ts:4979` and exempted from the strict-action throw at `:4797`. Landed in `0677ac0` ("feat: add subagents guide action"), released v0.46.0, and advertised in `extension/tool-description.ts:28`.
**Impact** — An orchestrator model that has drifted from the tool surface has no in-band way to re-read the current contract; upstream lets it call `{action:"guide", topic:"tool-reference"}` and get shipped, version-matched documentation. It also blocks the companion `children.list` retained-child listing that the same description paragraph advertises. The **slash-command** half is separately unported as **SUBA-066**.
**Fix** — Embed the crate's own documentation set (README plus per-topic markdown) with `include_str!` under `resources/docs/`, add `guide` to the action enum plus a `topic` param, and route it to a `registration::guide::read_subagent_guide` reproducing upstream's unknown-topic message verbatim. Embedding rather than reading from disk is the mechanism difference forced by shipping a single binary; record it as a `[CYRUP-DELTA]`.
**Verify** — `{action:"guide"}` must return the overview; `{action:"guide", topic:"bogus"}` must return upstream's exact *"Unknown subagents guide topic … Valid topics: … No files were changed."* string.

## ~~SUBA-056~~ — Durable completion replay and output archives are unported, so an async result consumed once cannot be re-read — **CLOSED 2026-09-16 (`2bd76ac`, PR #137; third rung `e61ff44`, PR #139)**

> **CLOSED 2026-09-16, cyrup `cc7818b` — see the table row for the evidence. Everything below is the
> filing text and its cyrup line is now false in its first clause.** `background/completion_replay/`
> exists (5 files / 1 810 lines) and is wired as the third rung of `collect_wait_completions`, which
> every `wait` runs. Both of the Fix's named hazards were taken: the 64 KiB tail is char-boundary
> correct (`utf8_tail`, not a byte slice) and the write sits on the terminal-transition path ahead of
> the unlink, with a test asserting the ORDER rather than the outcome. The Fix's sequencing note
> ("after `SUBA-034`") held — `SUBA-034` closed first and the same seam carries both.
>
> **`PARITY-GAPS.md` was updated for this closure and area 09's own table was not**, which is how the
> row survived two passes as open. Recorded so the next pass checks both.
>
> **One half of the filing did NOT come across and is not owed by this row:** the Impact's claim that
> the record should also feed `children.list` belongs to `SUBA-055`'s closure note, which returned
> `children.list` to `SUBA-005`'s unowned-verb list. Nothing here changes that.

**Kind** upstream-drift · **Severity** medium · **Effort** L · **Confidence** confirmed
**cyrup** — `rg -c 'completion_replay|output_archive|wait_completion' crates/cyrup-ext-subagents/src` = 0 across the whole crate. `background/watch.rs` invokes `CompletionSink` once per result file and nothing persists a replay record or an archive; `background/wait.rs` returns text only and carries no structured completion in tool-result details.
**upstream** — `pi-subagents/src/runs/background/completion-replay.ts` @v0.47.1 (new, 267 lines; absent at v0.43.0) — `completionReplayPath`/`completionArchivePath` (`:41,:46`), `writeCompletionArchive` (`:68`) preferring the child's output artifact, then its session file, then a 64 KiB `utf8Tail` of the result text, and `CompletionReplayRecord` with `expiresAt` (`:28-36`). Paired with `runs/background/wait-completions.ts` (new, 146 lines; `toWaitCompletion` at `:27` projecting the slim per-child shape into `details`) and wired through `result-watcher.ts` and `wait-subscriptions.ts`. Landed in `c2add95` and `a1e3fc8`, released v0.44.0/v0.45.0; hardened by `e55f083` and `0e06cb3`.
**Impact** — A background result that arrives while the orchestrator is mid-turn, or that is consumed by one `wait` and then needed again after a compaction, is unrecoverable — the notice fires once and the structured outcome never enters the tool result's `details`. The orchestrator has to re-run the child. Upstream's whole point is that the completion survives the turn that missed it.
**Fix** — Add `background/completion_replay.rs` porting the record + archive shapes (including the 64 KiB UTF-8-safe tail, which needs a char-boundary-correct truncation, not a byte slice), write both from `background/watch.rs`'s terminal-transition path, and project a `WaitCompletion` into the `wait` tool's structured result. Sequence after **SUBA-034** so the same terminal-transition seam carries both.
**Verify** — Complete a background run while no `wait` is outstanding, then call `wait {id}`; it must return the child's outcome and artifact paths from the replay record rather than "no active runs". Assert the archive prefers the output artifact path over inline text when the artifact exists.

## SUBA-057 — The `dismiss` action is unported, so a recovered async workflow with no live controller is stuck "running" in the fleet forever — **CLOSED 2026-08-15**

> **CLOSED 2026-08-15 (sweep 9) — see the table row for the evidence. Everything below is the
> filing text and its cyrup line is now stale.** By the time this was taken, the whole READ half
> was already at HEAD (the `display_dismissed_at` field, `ReconcileAction::DisplayDismissed`,
> `list_active_runs`'s `continue`, the `State: display-dismissed` report) — what was missing was
> the only WRITER, `SubagentExecutor::control_dismiss`, which `background/mod.rs:933` already
> intra-doc-linked as if it existed. It now exists, with the enum entry, the dispatch arm and the
> child-safe gate.

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/extension.rs:6557` — no `dismiss` in the 27-verb enum; `rg 'dismiss' crates/cyrup-ext-subagents/src` = 0. cyrup's reconciliation path is `background/reconcile.rs` + `background/run_status.rs`, which can mark a run stale but has no user-driven display dismissal and no `displayDismissedAt` field.
**upstream** — `pi-subagents/src/runs/foreground/async-dismiss-action.ts` @v0.47.1 (new, 85 lines; absent at v0.43.0) — `dismissRecoveredWorkflow` refuses when the run is not a recovered workflow, not in the active session, still has a live controller, or is not `running`; otherwise stamps `displayDismissedAt`, re-reconciles, calls `updateActiveRunIndex(asyncDir, "complete")` and drops the run from `asyncJobs`/`fleetJobs`. Dispatched at `subagent-executor.ts:5266`; `dismiss` is in `SUBAGENT_ACTIONS` (`shared/types.ts:1968`) and `MUTATING_MANAGEMENT_ACTIONS` (`:167`). Landed in `ade35ea` ("fix: dismiss recovered workflow displays", #1011), released v0.47.1.
**Impact** — After a crash or restart, a workflow whose runner process is gone but whose `status.json` still says `running` is displayed as live in the fleet widget and overlay indefinitely. The operator has no supported way to clear it short of deleting files under the async root by hand.
**Fix** — Add `dismiss` to the enum at `extension.rs:6557` and a `route_control_action` arm porting the five refusal conditions verbatim (each with upstream's exact message), stamping a `display_dismissed_at` on the status record, re-running `background::reconcile`, and evicting the run from `tui/fleet_state.rs`. Add it to the child-safe mutating denylist alongside the existing seven.
**Verify** — Kill a background runner mid-flight, restart the session, then `{action:"dismiss", id}`; the run must disappear from `/subagents-fleet` and `{action:"status"}`, while a run with a live controller must be refused with *"still has a live controller and cannot be dismissed."*

## SUBA-064 — The entire `authorityPolicy` subsystem is unported, and the `stop`/`steer` gate it drives is live-reachable in cyrup today

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed
*(Filed from the refuter's independent denial-path sweep; the audit cited `resolveAuthorityDecision` inside SUBA-046's upstream evidence and did not file the subsystem.)*
**cyrup** — `rg -n 'authority|Authority' crates/cyrup-ext-subagents/src` returns exactly two hits, neither an implementation: `extension.rs:7574`'s own comment places the mission arm *"after the management/control arms, before the authority-policy arm"* — naming a gate that does not exist — and `exec/task_intent.rs:7` is unrelated prose. `stop` and `steer` **are** both implemented and dispatched (`extension.rs:7805`, `:7825`), and `registration/mod.rs`'s only config validator is `validate_missions` (`:256`), so an `authorityPolicy` in `config.json` is silently dropped with no error and the action executes.
**upstream** — `pi-subagents/src/policy/authority.ts` present at **both** v0.43.0 and v0.47.1 — `:1-8` `AUTHORITY_ACTIONS` (`discardWorktree`, `destructiveCleanup`, `spawnBudgetGrant`, `scheduleCreate`, `stopRun`, `steerRun`), `:11` `AuthorityDecision = auto|confirm|forbid`, `:14-21` `DEFAULT_AUTHORITY_POLICY` (three actions defaulting to `confirm`), `:23` `resolveAuthorityDecision`, `:30` `validateAuthorityPolicy` throwing typed errors for both an unknown action and a bad decision. Four consumers at v0.43.0: `subagent-executor.ts:4358` (discardWorktree), `:4491` (spawnBudgetGrant), `worktree.ts:607`, `herdr/actions.ts:205-206` — plus, critically, `subagent-executor.ts:4412-4423`, which maps `stop`→`stopRun` and `steer`→`steerRun` and then forbids with `Authority policy forbids action '${action}'.` or requires `ctx.ui.confirm` with `Authority policy requires user confirmation for action '${action}'…`, including an explicit no-UI refusal branch.
**Impact** — An operator who sets `"authorityPolicy": {"stopRun": "forbid"}` has it silently dropped and the action executes anyway — the config is accepted, unvalidated, and inert. Unlike most items here the gated actions are already live in cyrup, so this is not a dormant gap: it is a policy surface that a user can configure and that does nothing. It is also the prerequisite for **SUBA-046**'s grant path and for the `worktree.discard` / `destructiveCleanup` verbs SUBA-005 lists as unowned.
**Fix** — Port `authority.ts` as `registration/authority.rs`: the six-action enum, the three-way decision, `DEFAULT_AUTHORITY_POLICY`, `resolve_authority_decision`, and `validate_authority_policy` with both typed errors wired into config load beside `validate_missions` (`registration/mod.rs:256`). Consult it in the `stop` and `steer` dispatch arms (`extension.rs:7805`, `:7825`) with pi's exact forbid/confirm/no-UI messages, routing the confirm through `HostServices`. **Hard prerequisite, recorded so it is not rediscovered:** this item is held at `medium` only because the four destructive `AUTHORITY_ACTIONS` (`discardWorktree`, `destructiveCleanup`, `spawnBudgetGrant`, `scheduleCreate`) have no dispatch in cyrup to bypass. Whoever lands `worktree.discard` or `destructiveCleanup` (two of `SUBA-005`'s unowned verbs) must land the authority gate **in the same change** and raise this item to `critical` if they do not — shipping a destructive verb behind a config key that is parsed and ignored is a permission bypass by `README.md:106-107`'s definition.
**Verify** — `"authorityPolicy": {"stopRun": "forbid"}` must make `{action:"stop", id}` fail with `Authority policy forbids action 'stop'.`; `{"steerRun": "confirm"}` with no UI available must refuse with pi's no-UI message; an unknown action key or a bad decision value must fail config load with pi's typed error rather than being ignored.

## SUBA-017 — Completion batching unported

> **RE-READ 2026-09-16 at cyrup `cc7818b` and upstream `v0.68.0` — STILL OPEN, evidence refreshed.**
> Port side, re-run and still zero: `git grep -c 'completion_batch|batcher|CompletionBatch' --
> crates/cyrup-ext-subagents/src` returns no matching file. Two citations below are stale and are
> corrected here rather than in place: `background/watch.rs` is now the directory
> `background/watch/` (7 files — `classify.rs`, `install.rs`, `message.rs`, `mod.rs`, `observer.rs`,
> `results_watcher.rs`, `sink.rs`), with the one-message-per-result hand-off at `watch/install.rs:304`;
> and `SubagentExtensionConfig` is **34** fields (`registration/mod.rs:79`), not 18 — still with no
> `completion_batch` among them. Upstream re-read at v0.68.0: `completion-batcher.ts` is 168 lines,
> the key is `completionBatch?: CompletionBatchConfig` at `shared/types.ts:2663`, the wiring is
> `extension/index.ts:495`, and the grouping logic is `notify.ts:544` (`completionBatchKey`), `:692`,
> `:723`, `:790`. Still in-baseline (`v0.43.0:src/runs/background/completion-batcher.ts` exists), so
> the `not-ported` re-classification stands. **`09a`'s `SUBA-090` closure explicitly parked upstream's
> grouped `formatGroupedCompletion` form on this row** — it is part of this item's scope, not a
> separate one. Nothing in #137/#139/#140 touched the notify path's batching.

**Kind** not-ported *(re-classified from `upstream-drift`)* · **Severity** low · **Effort** M · **Confidence** confirmed
**cyrup** — Zero hits for `completion_batch`/`batcher` crate-wide; no `completionBatch` key on the 18-field `SubagentExtensionConfig` (`registration/mod.rs`). `background/watch.rs` invokes `CompletionSink` once per result with no debounce, and its own test pins the one-notify-per-result contract.
**upstream** — `pi-subagents/src/runs/background/completion-batcher.ts` passes `git cat-file -e v0.43.0` — **in-baseline**, so this is unported work, not expected lag — and is wired at `extension/index.ts:376` @v0.43.0 (`registerSubagentNotify(pi, state, { batchConfig: config.completionBatch })`).
**Impact** — Ten background runs finishing together produce ten separate notices instead of one batched summary; in a large fan-out the notice stream buries the actual results.
**Fix** — Port as `background/batcher.rs` between `CompletionWatcher` and `CompletionSink`, with the debounce window and aggregate notice, plus the `completionBatch` config key. Same seam as SUBA-034 and SUBA-056 — do them together.
**Verify** — Complete five runs within the window; exactly one aggregate notice must be emitted.

## SUBA-022 — Typed extension delegation API unported

> **RE-READ 2026-09-16 at cyrup `cc7818b` and upstream `v0.68.0` — STILL OPEN, evidence refreshed and
> unchanged in substance.** `ls crates/cyrup-ext-subagents/src` has no `api/` among its 23 entries
> (`artifacts.rs background bin discovery error.rs exec extension fork_context.rs formatters.rs
> identity jsonl.rs lib.rs missions native_supervisor.rs paths.rs prompt_runtime.rs registration
> runner spawn tests time.rs tui watchdog workflows`), and `git grep -c 'prompt-template:subagent' --
> crates/` = 0 workspace-wide. Upstream `src/api/delegation.ts` re-read at v0.68.0: the five constants
> are at `:6-10`, unchanged in name and value since v0.43.0. **One line of its header is the design
> instruction for the port and was not recorded before:** *"This is the established
> extension-to-extension transport. The structured delegation API intentionally reuses it instead of
> adding a second event protocol."* — so the cyrup port is a typed façade over `cyrup-ext`'s existing
> `SharedBus`, not a new channel. `SharedBus` queues emits and passes payloads by value
> (`crates/cyrup-ext/src/bus.rs`, `[CYRUP-DELTA]`), which is the same request/response-topic design
> problem `09a` recorded for the v0.64.0 runtime-agent event bridge; the two should be designed
> together. Effort **L** stands.

**Kind** not-ported *(re-classified from `upstream-drift`)* · **Severity** low · **Effort** L · **Confidence** confirmed
**cyrup** — `ls crates/cyrup-ext-subagents/src` at HEAD has no `api/` module; `rg -c 'prompt-template:subagent' crates/` = 0.
**upstream** — `pi-subagents/src/api/delegation.ts` passes `git cat-file -e v0.43.0` — **in-baseline** — and is present at v0.47.1, exporting five event names (`prompt-template:subagent:request|started|update|response|cancel`, `:4-8`).
**Impact** — Other extensions cannot delegate to subagents through a typed API; any integration must go through the tool surface. Low because no in-tree consumer exists yet.
**Fix** — Port as `api/delegation.rs` over the crate's existing event surface, emitting the five event names verbatim. Sequence after SUBA-018 (now closed), whose `prompt_workflows.rs` is the natural first consumer.
**Verify** — A second extension subscribing to `prompt-template:subagent:response` must receive a payload for a delegation it requested.

## SUBA-023 — Async lifecycle hardening unported; no signal-name attribution — **PARTIALLY CLOSED 2026-09-16**

> **PARTIALLY CLOSED 2026-09-16, cyrup `cc7818b`. The cyrup line below is false in its first clause
> and the Verify line is satisfied.**
>
> **IN — signal-name attribution, exactly the half the Fix called "independently useful now and
> cheap".** `TerminationOutcome` (`spawn/signal.rs:112-127`) carries `signal_name:
> Option<&'static str>`, mapped by `signal_name(i32)` (`:136-`) over this ladder's own three signals,
> the terminal/job-control set and the fault signals. The doc at `:120-125` records the call that
> makes it correct rather than merely present: the name is derived from the OBSERVED status, never
> from `Self::stage`, because a child can die of a signal nobody in this ladder sent — an external
> `kill`, an OOM kill, a `SIGSEGV` — and reporting the rung we happened to be on would misattribute
> exactly the cases worth debugging. An unrecognised number returns `None` rather than a fabricated
> name. **It has production consumers, which is the bar this directory sets:**
> `exec/attempt_runner.rs:150` puts it on every attempt outcome via `process_signal_name` (`:787-791`),
> and `exec/external_cli/run.rs:403,569` routes through the same single mapping — the doc at
> `attempt_runner.rs:845-852` records that this function previously carried its OWN three-entry table
> (SIGINT/SIGKILL/SIGTERM only), so a crashed child reported a number. Green:
> `spawn::signal::tests::a_killed_child_reports_its_signal_name_and_a_clean_exit_reports_none` and
> `exec::attempt_runner::tests::a_crashed_child_reports_the_posix_signal_name_not_a_number`.
>
> **NOT IN — the two upstream subsystems, and the code now says so at two new seams.**
> `process_terminal` and `session_lease` remain zero-hit as implementations. The two matches that
> exist are both the crate recording the absence: `background/active_async_capacity/inspect.rs:145-172`
> explains that upstream releases a runner slot on a `processTerminal` record whose `state ===
> "observed"` matches the owner's `runnerProcessInstanceId`, that **cyrup has neither input**, and that
> porting it verbatim would retain every SUCCESSFUL run forever — so it substitutes the runner pid as
> its start-proof and drops two unrepresentable rungs deliberately; `background/active_run_index.rs:56-59`
> records `readProcessTerminal(asyncDir)?.state === "observed"` as the other disjunct of the reader's
> staleness rung (`async-status.ts:575`) that cyrup cannot apply. **Both substitutions are now
> load-bearing, which RAISES the cost of a later port** — it is no longer additive.
>
> **Kind correction.** Both `src/runs/background/process-terminal.ts` and `src/runs/shared/session-lease.ts`
> pass `git cat-file -e` at **v0.43.0** as well as v0.68.0, so they are in-baseline: this row's
> `upstream-drift` kind is wrong and should read `not-ported`, the same correction `SUBA-017`/`SUBA-022`
> already took. PARITY-GAPS VL-S3/VL-S4 carried the same wrong kind; all three closed on
> 2026-09-20, and SUBA-023's `Kind` cell was corrected to `not-ported` as part of that closure.
>
> **The landed half was already true at `d53763b`** and is not attributable to #137/#139/#140 — nor to
> any sha, since `git log -S` bottoms out at the history root `da970e8`. The `## Open items` row has
> carried PARTIALLY CLOSED since 2026-08-14; this section and the `## Status table` row are what lagged.

**Kind** upstream-drift · **Severity** low · **Effort** L · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/spawn/signal.rs:90-106` — `TerminationOutcome` carries only `status: ExitStatus` and `stage: EscalationStage`; no `ExitStatus::signal()` name mapping anywhere in the module. `process_terminal` and `session_lease` are zero-hit crate-wide.
**upstream** — `src/runs/background/process-terminal.ts` and `src/runs/shared/session-lease.ts` both present at v0.47.1 (PARITY-GAPS VL-S3/VL-S4, **both CLOSED 2026-09-20** — this paragraph is the kind-correction argument and is kept as history).
**Impact** — Signal attribution in run results stays coarse ("failed" rather than "killed by SIGKILL"), which makes escalation-ladder debugging harder, and there is no session lease to arbitrate two hosts touching the same run tree.
**Fix** — Independently useful now and cheap: map `ExitStatus::signal()` to a name in `TerminationOutcome`. The two upstream subsystems port after that.
**Verify** — A child killed at the SIGKILL rung must report the signal name in its run record.

## SUBA-024 — `parallel-handoff` / `agent-contract` unported

> **RE-READ 2026-09-16 at cyrup `cc7818b` and upstream `v0.68.0` — STILL OPEN. The zero-hit greps now
> return three matches and all three are the code declaring the gap.**
> `background/async_retention/scan.rs:425` — `[CYRUP-DELTA] RunStatus has no parallel_handoff field,
> so upstream's first path source (status.parallelHandoff.path, :270-273) has nothing to read on this
> side and is not simulated`. `exec/acceptance/lattice/inject.rs:49-55` and
> `spawn/chain_graph.rs:2047-2051` both pass `report_optional: false` into `format_acceptance_prompt`,
> each stating that `isAgentContractV1` is `contract?.version === 1` (`shared/agent-contract.ts:3-5`)
> and that no cyrup agent can declare one — and both keep it a PARAMETER rather than hardcoding the
> `false` inside the prompt builder, "so that porting `agent-contract.ts` is a change at THIS seam
> only". That is the right shape and it means the agent-contract half is a one-seam port, not an L.
>
> **One consumer-side slice of `parallel-handoff` HAS landed, and the row does not close on it.**
> `has_unresolved_run_handoff` (`async_retention/scan.rs:423-440`) is a full port of pi's
> `hasUnresolvedRunHandoff` (`:268-277`) + `unresolvedHandoff` (`:258-266`) for the local
> `<run_dir>/handoff.json` half, version-gated and fail-closed on an unreadable manifest. Its own doc
> says why it exists anyway: *"no cyrup writer produces the file today, the probe is one `stat`, and a
> guard written as a constant `false` is the one that fails silently later."* **A reader with no
> producer is not closure** — this is the fourth instance in this crate of tested machinery with no
> production caller, and it is recorded here so a future pass does not mistake the grep hit for the
> feature.
>
> **Kind correction.** Both `src/runs/shared/parallel-handoff.ts` and `src/runs/shared/agent-contract.ts`
> pass `git cat-file -e` at **v0.43.0** and v0.68.0 — in-baseline, so `upstream-drift` is wrong and the
> kind should read `not-ported`.
>
> **The blind spot below is now carried for the FOURTH pass and is not a disclaimer.**
> `spawn/chain_graph.rs`'s pre-walk validation and `ChainStepConfig`'s unknown-key handling have still
> not been read; either could already cover part of the handoff surface, and until one pass opens them
> this row's effort estimate is unfounded in both directions. Reading two functions would settle it.

**Kind** upstream-drift · **Severity** low · **Effort** L · **Confidence** confirmed *(raised from probable on the two surviving names; the two named blind spots below are unchanged)*
**cyrup** — `task_intent` **is** ported (`exec/task_intent.rs`, 1967 lines, consumed from `completion_guard.rs`, `acceptance.rs`, `output.rs`, `mod.rs`) and is struck from this item. `parallel_handoff` and `agent_contract` remain zero-hit crate-wide.
**upstream** — `src/runs/shared/parallel-handoff.ts` and `src/runs/shared/agent-contract.ts` are present at **both** v0.43.0 and v0.47.1. `chain-validation.ts` is **struck from the item entirely**: `git log --all -- src/runs/shared/chain-validation.ts` is empty — the file never existed at any path, so the original item named a symbol that was never real.
**Impact** — No structured handoff between parallel siblings and no agent-contract validation, so a fan-out's per-child expectations are unenforced.
**Fix** — Port both as `spawn/parallel_handoff.rs` and `discovery/agent_contract.rs`.
**Verify** — N/A until scheduled. **Blind spot carried forward for the third pass running:** `spawn/chain_graph.rs`'s pre-walk validation and `ChainStepConfig`'s unknown-key handling have still not been re-read; either could already cover part of the handoff surface.

## SUBA-026 — Interactive admin UI, selector and `/subagents` unported

> **RE-READ 2026-09-16 at cyrup `cc7818b` and upstream `v0.68.0` — STILL PARTIALLY CLOSED, with one
> citation in the filing below CORRECTED because it names a file that exists at no tag.**
>
> **Correction, and it matters because it would waste a porter's first hour.** The cyrup/upstream lines
> below cite `src/tui/selector.ts`. `git cat-file -e v0.43.0:src/tui/selector.ts` and
> `…v0.68.0:src/tui/selector.ts` both FAIL, and `git log --all -- src/tui/selector.ts` is empty — the
> file has never existed at that path. The real file is **`src/slash/selector.ts`**, 147 lines at
> v0.68.0 and present at v0.43.0. `src/slash/subagents-admin.ts` is **460** lines at v0.68.0 (the
> recorded 432 was its v0.47.1 size) and `subagents` registers at `src/slash/slash-commands.ts:869`.
> **PARITY-GAPS VL-S11 carries the same wrong path** and should be corrected alongside this row.
>
> **Port side, refreshed.** The `as_str` table is 17 variants at `registration/slash_commands.rs:129-149`
> — the closed `/subagents-stop` at `:146` and `/subagents-guide` (`SUBA-066`) at `:147`. Still absent:
> `subagents` itself and any selector. `git grep -c 'subagents_admin|SubagentsAdmin|subagents-admin' --
> crates/cyrup-ext-subagents/src` = 0 and `git grep -c 'selector|Selector' -- …/src/tui/` = 0, so the
> Fix's premise that FleetView "now supplies the rendering primitives this needs" is an assumption no
> pass has checked — `tui/fleet*.rs` contains no selector primitive.
>
> **Kind correction.** `src/slash/subagents-admin.ts` and `src/slash/selector.ts` are both at v0.43.0,
> so this row is `not-ported`, not `upstream-drift`.

**Kind** upstream-drift · **Severity** low · **Effort** L · **Confidence** confirmed
*(Partially closed: `/subagents-stop` now exists at `registration/slash_commands.rs:144`.)*
**cyrup** — The 16-variant `as_str` match at `registration/slash_commands.rs:127-146` has neither `subagents` nor a selector surface; `rg 'subagents-admin|SubagentsAdmin' crates/cyrup-ext-subagents/src` = 0.
**upstream** — `src/slash/subagents-admin.ts` (432 lines) and `src/tui/selector.ts`; `subagents` is registered at `slash-commands.ts:655` @v0.47.1. Matches PARITY-GAPS VL-S11.
**Impact** — No interactive agent picker and no admin surface; every management operation must go through the tool's action verbs.
**Fix** — Port alongside the existing FleetView surface (`tui/fleet*.rs`), which now supplies the rendering primitives this needs.
**Verify** — `/subagents` must open the picker and list the same agents `{action:"list"}` returns.

## SUBA-029 — Management actions read-modify-write subagents `settings.json` unlocked

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/discovery/settings_write.rs:70-81`: `write_settings_file` is `create_dir_all` then a bare `std::fs::write` — no lock, no temp-then-rename; `read_settings_file_strict` is a separate unlocked call, so the disable/enable/reset handlers perform an unsynchronised read-modify-write.
**upstream** — `pi-subagents/src/agents/agents.ts` is likewise unlocked, which is why this stays `cyrup-original` rather than a parity gap: the bar cyrup fails is its **own** (`cyrup-config/src/settings.rs` uses `FileLock` + `write_atomic`), not pi's.
**Impact** — Two concurrent disable/enable/reset actions can lose one another's write, or leave a truncated `settings.json` if the process dies mid-write, disabling every agent until it is hand-repaired.
**Fix** — Hold one lock across read+write in `settings_write.rs` and route the write through the crate's own `background/atomic.rs::write_atomic_json`.
**Verify** — Two concurrent `disable` calls on different agents must both persist; kill mid-write and the file must remain parseable.

## SUBA-033 — Tests assert a lower bound on observed concurrency

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/spawn/parallel.rs:739-742` asserts `observed_peak >= 2` (the `<= cap` upper bound at `:734` is the real invariant and is unaffected). **Citation corrected this pass:** the second instance is at **`:1133`** (`peak.load(Ordering::SeqCst) >= 2`), not at `:802-810` — `:798-812` contains only the `<= global_cap` assertion.
**upstream** — No counterpart; `spawn/parallel.rs` is a documented cyrup-original bounded-concurrency primitive. The precedent is commit `1806375`, which deleted an `orphaned > 0` lower bound for exactly this reason.
**Impact** — On a single-core or heavily loaded runner these flake, and a flaky concurrency test is the fastest route to an ignored concurrency test.
**Fix** — Make the overlap deterministic with a `tokio::sync::Barrier` sized to the expected concurrency inside the worker, then keep only the `<= cap` assertions.
**Verify** — Tests pass deterministically under `--test-threads=1` on a single-core cgroup.

## SUBA-034 — `wait`'s event-bus wake unported; pure polling at a 1 s floor — **CLOSED 2026-08-15 (REFUTED / already-done)**

> **REFUTED, CLOSED 2026-08-15 (sweep 11).** The wake landed in sweep 9 (`844e25f`) and the row was
> never marked. Everything below is the filing text and its cyrup line is stale: `background/wait.rs`
> carries a `# Wake mechanism (SUBA-034)` section, `WaitDeps::completion_bus`, and a `biased;`
> `select!` over a `broadcast::Receiver`. No fix was manufactured.

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/background/wait.rs:30-34` documents the missing subscription as a deliberate delta; `DEFAULT_POLL_INTERVAL_MS = 1000` (`:67-69`) is the only interval used (`:179-184`); no broadcast/subscribe in the file.
**upstream** — `pi-subagents/src/runs/background/wait.ts` subscribes to the completion/control channels and wakes the instant either fires, keeping the poll as reconciliation. Upstream additionally gained `wait-subscriptions.ts` at v0.41.0 (PARITY-GAPS VL-S8).
**Impact** — Up to one second of added latency per wait, paid repeatedly by a fan-out of short runs. Low because the polling fallback is functionally correct.
**Fix** — Have `CompletionWatcher` (`background/watch.rs`) publish terminal transitions on a broadcast channel the wait loop `select!`s against, keeping the poll as reconciliation. Same seam as SUBA-017 and SUBA-056.
**Verify** — A run that completes 50 ms into a wait must return in ~50 ms, not ~1 s.

## SUBA-035 — Active `subagents.modelScope` policy not surfaced by doctor/models — **CLOSED 2026-08-14 (REFUTED / already-done)**

> **CLOSED 2026-08-14 (sweep 8). Everything below is the filing text.** Doctor half:
> `registration/doctor.rs:646-676` `model_scope_check`, called at `:603`, tests `:1609`-`:1642`.
> Models-report half: `extension.rs::run_models_report`, both views, each citing SUBA-035 in-source.
> **The residual's stated location (`registration/mod.rs` / `profiles.rs`) was wrong** — that
> correction is the part of this row worth keeping.


**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed
**cyrup** — `rg 'model_scope|modelScope' crates/cyrup-ext-subagents/src/registration/doctor.rs` returns nothing across all 1803 lines; the models report reads only the current model. Enforcement itself is live (`exec/model_scope.rs:170-188`).
**upstream** — `pi-subagents/src/runs/shared/model-scope.ts` surfaces warn-severity violations and validates the config as part of its settings surface.
**Impact** — An operator debugging "why did my model choice not apply" gets no hint from `/subagents-doctor` or the models report that a scope policy is filtering it. Compounds with **SUBA-050**: once `strict` exists, an unsurfaced policy becomes an unexplained hard failure rather than an unexplained warning.
**Fix** — One diagnostic in `registration/doctor.rs` reading the resolved `ModelScopeConfig`, plus the same line in the models-report header. Include `strict` once SUBA-050 lands.
**Verify** — With a scope configured, `/subagents-doctor` must print the active scope and its severity.

## SUBA-037 — Doctor's `--version` binary probe leaks the probe process on timeout

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/registration/doctor.rs:340-352`: the probe builder sets stdin/stdout/stderr to null and calls `.status()`, then is raced by `tokio::time::timeout(VERSION_PROBE_TIMEOUT, probe)`; there is no `.kill_on_drop(true)`, so on timeout the future is dropped and the child is leaked. The crate now has three `kill_on_drop` sites (`extension.rs:11553`, `watchdog/lsp_diagnostics.rs:907`, plus the acceptance ladder), so the pattern is well established here — this one site was missed.
**upstream** — `pi-subagents/src/extension/doctor.ts` spawns no subprocess, so the probe is cyrup-original; the in-codebase contract for a timed-out child is the acceptance ladder (SIGTERM then a hard SIGKILL), now enforced after SUBA-027.
**Impact** — `/subagents-doctor` on a misconfigured install — exactly what doctor exists for — can leave a hung `cyrup --version` behind on every invocation, and the report says the probe timed out without saying anything survived.
**Fix** — Add `.kill_on_drop(true)` to the probe builder. The probe does not set `process_group(0)`, so the pid-targeted SIGKILL suffices and no group logic is needed.
**Verify** — Point the subagent binary at a script that `exec sleep 300`, run the check with a 100 ms `VERSION_PROBE_TIMEOUT`; after it returns Timeout, `kill(probe_pid, 0)` must fail with ESRCH.

## SUBA-038 — Three denial / unknown-action messages still diverge from pi's text

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed
*(Partially closed: the child-safe UNAVAILABLE text is now verbatim — `extension.rs:7585`, `:7636`, `:17190` emit `Action '{action}' is not available from child-safe subagent fanout mode.`, matching `subagent-executor.ts:4380` @v0.43.0.)*
**cyrup** — Three residuals, the third found by the refuter in the same sweep:
1. `extension.rs:7698-7702` — the MUTATING denial still emits cyrup's own *"subagent management action '{action}' is blocked in child-safe fanout mode; {list} are not permitted here."*, pinned by substring tests at `:14194` and `:18227`.
2. `extension.rs:7610-7614` — the unknown-action text is *"unknown subagent action '{other}'; valid actions are …"* against upstream's `Unknown action: ${action}. Valid: …`, **and cyrup's hand-written list omits the four `watchdog.*` verbs that do dispatch.**
3. `extension.rs:7851-7854` — the CONTROL arm's unknown-action message lists *"status, interrupt, resume, steer, append-step"* and omits `stop`, which is advertised in the enum and dispatched at `:7805`. Same advertise-vs-message drift, same fix.
**upstream** — `subagent-executor.ts:4861` @v0.43.0 for the unknown-action text; `:167` for the denylist. Note the v0.47.1 message is richer still — that is **SUBA-065**, filed separately.
**Impact** — A fanout child attempting a mutating or unknown action sees different text than pi emits, and in two of three cases a *wrong* list of valid actions, so a model recovering from the error is steered away from verbs that exist. Model-visible; no behavioural difference in what is blocked.
**Fix** — Replace all three format strings with pi's verbatim text, derive the valid-action list from the enum rather than hand-writing it (which is what let two of them drift), and rewrite the two substring assertions at `:14194`/`:18227` to equality against the new text.
**Verify** — `{action:"delete", agent:"x"}` in child-safe mode, `{action:"bogus"}`, and `{action:"bogus"}` on the control arm must all produce text byte-identical to upstream's strings, with every dispatching verb present in the list.

## SUBA-039 — `SpawnedChild` has no `Drop` guard, so a dropped drive future orphans a detached process group

**Kind** cyrup-original · **Severity** low · **Effort** M · **Confidence** confirmed on mechanism, probable on reachability
**cyrup** — `rg 'impl Drop' crates/cyrup-ext-subagents/src` returns exactly five impls — `background/runner_main.rs:2712` (`SigUsr2Guard`), `:3003` (`ControlWatcherHandle`), `background/control.rs:1684` (`AppendLockGuard`), `background/watch.rs:762` (`CompletionWatcherHandle`), `registration/profiles.rs:1006` (`RestorePerms`) — and **none is for `SpawnedChild`**, while `spawn/mod.rs` sets `command.process_group(0)` on every child. `spawn/signal.rs:205` now names the exact hazard in its own doc: *"with no kill_on_drop leaves the whole process group running for the machine's uptime"*.
**upstream** — `pi-subagents` never passes `detached`, so its children stay in pi's own process group and a terminal signal reaches the whole tree regardless of how a promise unwinds. The guard upstream gets for free must be written here.
**Impact** — An orphaned subagent subtree — a re-exec'd `cyrup` plus whatever cargo/npm/git it is blocked in — runs for the machine's uptime, unreachable by Ctrl-C. Low because neither in-crate driver drops today; this is a missing safety invariant one careless `select!`/`timeout`/`JoinHandle::abort` away from firing.
**Fix** — Add `impl Drop for SpawnedChild` that on Unix best-effort SIGKILLs `-pgid` when the child leads its group, reusing the `getpgid(pid) == pid` guard from `spawn::signal::send_signal`. The existing `exited` flag makes the guard a no-op on the normal paths. `kill_on_drop(true)` alone is **not** adequate: it targets the bare pid and leaves the descendants this item is about.
**Verify** — Construct a `SpawnedChild` running `sh -c 'sleep 300 & echo $! > gpid; wait'`, drop it without `terminate`/`finish`, and assert `kill(descendant_pid, 0)` fails with ESRCH.

## SUBA-058 — Chain read instructions are not filtered by existence, so children are told to read files that are not there

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/src/spawn/chain_graph.rs:726-736` — the reads list is mapped straight through `resolve_chain_path` into `[Read from: {}]` with no filesystem check; the test at `:2522` pins the unfiltered output.
**upstream** — `pi-subagents/src/shared/settings.ts:356-362` @v0.47.1 — `resolveExistingReadInstructionPaths(reads, instructionCwd, existenceCwd)` resolves each path twice and emits only those where `fs.existsSync(existencePath)` holds. At v0.43.0 `settings.ts:359` was still the unfiltered `.map(resolveChainPath)`. Landed in `bc1b689` ("fix: omit missing child read files"), released v0.47.1.
**Impact** — A chain step whose upstream sibling produced no output still receives `[Read from: <chain_dir>/analysis.md]`. The child burns a turn on a failing read and often narrates the missing file as a finding, polluting the chain's `{previous}` text for every later step. Low because it degrades quality rather than correctness.
**Fix** — Add the existence filter to the instruction builder at `spawn/chain_graph.rs:730`, taking pi's two-cwd form (instruction path vs existence path) so a worktree child checks the right tree. Update the pinned expectation at `:2522`. Land with **SUBA-053** and **SUBA-054**, which touch the same builder.
**Verify** — A chain step declaring two reads where only one file exists must emit `[Read from: <the existing one>]`; a step where none exists must emit no read line at all.

## SUBA-059 — `artifactConfig.cleanupDays` is never wired to the type that already parses it

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed
*(Corrected this pass. The refuter's caveat, stated inline: the gap is **narrower** than the audit described — `ArtifactConfig` already exists in cyrup and already deserializes `cleanupDays`; the whole defect is the missing wire from `SubagentExtensionConfig` to it. Do not "add a `cleanup_days` field" — it is there.)*
**cyrup** — `crates/cyrup-ext-subagents/src/artifacts.rs:85-113` already defines `#[serde(rename_all = "camelCase")] pub struct ArtifactConfig { pub cleanup_days: u64, … }` defaulting to 7 and citing pi's `DEFAULT_ARTIFACT_CONFIG`. But both call sites pass the hardcoded constant instead: `extension.rs:9235-9238` and `:9403-9406` call `cleanup_all_artifact_dirs(&self.cwd, crate::artifacts::DEFAULT_CLEANUP_DAYS)` (`artifacts.rs:50` = 7), and the 18-field `SubagentExtensionConfig` has no `artifact_config`. Confirmed separately: `cleanup_old_artifacts` (`artifacts.rs:277-302`) computes `cutoff = now - max_age_days * ONE_DAY_MS`, so a literal `0` would mean "delete everything", not "disable".
**upstream** — `pi-subagents/src/extension/index.ts:369-370` @v0.47.1 — `const artifactCleanupDays = config.artifactConfig?.cleanupDays ?? DEFAULT_ARTIFACT_CONFIG.cleanupDays; cleanupAllArtifactDirs(artifactCleanupDays);`, validated at `extension/config.ts:41-47` (non-negative integer), semantics at `shared/types.ts:1859` ("Set cleanupDays to 0 to disable cleanup"). Landed in `b69aafb` ("fix: honor artifact cleanup retention config", #1013), released v0.47.1.
**Impact** — A user who wants subagent transcripts kept for audit (or deleted sooner) cannot say so, and there is no way to disable the sweep — cyrup silently deletes run inputs, outputs and JSONL older than a week on every extension load, including where those files are the record of what a fan-out actually did.
**Fix** — Add `artifact_config: Option<ArtifactConfig>` to `SubagentExtensionConfig` (the struct already exists), validate as a non-negative integer with upstream's error text, pass it at `extension.rs:9236` and `:9404`, and add an explicit `0 ⇒ skip` arm to `cleanup_all_artifact_dirs`/`cleanup_old_artifacts` so zero disables rather than purges.
**Verify** — With `"artifactConfig": {"cleanupDays": 0}`, an artifact with an mtime a year old must survive an extension load; with `30`, a 40-day-old artifact must be removed and a 20-day-old one kept.

## SUBA-060 — "Resume-first" guidance for failed async runs is unported, so the parent relaunches work a persisted session could continue

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed
**cyrup** — `rg -c 'Resume-first|resume_first|resume_guidance' crates/cyrup-ext-subagents/src` = 0. `background/run_status.rs` and the completion-notice path emit failure text with no revive suggestion, even though cyrup **has** the revive machinery (`ResumeOutcome::RespawnFromTranscript`, `background/control.rs:1214`, routed at `extension.rs:4210-4212`).
**upstream** — `pi-subagents/src/runs/background/resume-guidance.ts` @v0.47.1 (new, 33 lines; absent at v0.43.0) — `formatAsyncReviveCommand` (`:5`) picks the failed step with an existing `sessionFile` and formats a literal `subagent({ action: "resume", id: "…", index: N, message: "Continue from the persisted child session…" })`; `formatResumeFirstFailedRunDetail` (`:16`) and `formatResumeFirstFailedRunsNote` (`:25`) fold it into status output with *"Launch a replacement only if revive fails or the user explicitly asks for one."* Landed in `b283d27`, released v0.45.2.
**Impact** — When a background run fails, cyrup's status/notice text says only that it failed. An orchestrator model's default response is to spawn a fresh child, discarding the failed child's persisted session and re-paying for work already done.
**Fix** — Port the three formatters as `background/resume_guidance.rs` over `RunStatus`/step records (the `session_file` field already exists — `background/wait.rs:543` shows it on the summary shape), and append the note in `run_status`'s failure rendering and the completion-notice text. The `resume` verb and `index` param already exist. **Precision note:** upstream omits `index` entirely when `steps.length === 1` (`:12-13`) — reproduce that, do not always emit `index: 0`.
**Verify** — A failed two-step async run whose step 1 has a persisted session file must produce a status body containing the literal `subagent({ action: "resume", id: "…", index: 0, message: … })` call, byte-identical to upstream's format; a failed single-step run must omit `index`.

## SUBA-061 — Four documented config keys are silently ignored

> **RE-READ 2026-09-16 at cyrup `cc7818b` — STILL OPEN on all four keys, but the field inventory below
> is badly stale and one of the Fix's blockers is discharged.**
>
> All four names are still zero-hit port-side: `git grep -c 'asyncWidget|async_widget|inlineToolDisplay|inline_tool_display|fleetKeybindings|fleet_keybindings|legacyChainControls|legacy_chain_controls'
> -- crates/cyrup-ext-subagents/src` returns no matching file. **But the item is THREE keys, not four.**
> `legacyChainControls` was struck by the `9aeba769` re-read and stays struck, re-confirmed here at the
> current tag: `git -C tmp/pi-subagents grep -c legacyChainControls v0.68.0 -- src` is **empty**, where
> `v0.47.1` carried it in five files (`extension/config.ts`, `extension/fanout-child.ts`,
> `extension/schemas.ts`, `extension/tool-description.ts`, `shared/types.ts`). Upstream DELETED the key;
> porting it would port a surface that no longer exists. The surviving three are all still declared at
> `shared/types.ts` @v0.68.0 — `fleetKeybindings` `:2609`, `asyncWidget` `:2611`, `inlineToolDisplay`
> `:2617` — so the mixed `Kind` is right for **2 in-baseline + 1 drift**.
>
> **`SubagentExtensionConfig` (`registration/mod.rs:79`) is now 34 fields, not the 18 enumerated
> below.** The sixteen added since: `max_subagent_spawns_per_run`, `model_exclusions`,
> `max_active_async_runs_per_session`, `capacity`, `spawn_command`, `roots`, `env_overrides`,
> `scheduled_runs`, `artifact_config`, `artifact_dir`, `authority_policy`, `turn_budget`, `timeout_ms`,
> `default_subagent_context`, `tool_description_mode`, `permissions`. The list below should be read as
> a v0.43.0-era snapshot, not as the struct.
>
> **The Fix's `SUBA-025` sequencing clause is now moot rather than discharged.** It says to wire
> `legacy_chain_controls` "into the description builder **SUBA-025** introduces — so sequence after it".
> `tool_description_mode` IS on the struct, so that builder exists — but the key it was to feed is the
> one upstream deleted, so the clause has no subject. The remaining three wiring targets in the Fix cite
> `extension.rs:9489/:9889/:9978` and `:9616/:9646`, all of which predate the executor/tool split and no
> longer resolve — re-derive them before planning.

**Kind** not-ported *(mixed — see below)* · **Severity** low · **Effort** M · **Confidence** confirmed
**cyrup** — `SubagentExtensionConfig` (`crates/cyrup-ext-subagents/src/registration/mod.rs`) has exactly 18 fields: `async_by_default`, `force_top_level_async`, `global_concurrency_limit`, `max_subagent_spawns_per_session`, `parallel`, `control`, `chain`, `proactive_skill_subagents`, `default_session_dir`, `single_run_output_base_dir`, `max_subagent_depth`, `worktree_base_dir`, `worktree_setup_hook`, `worktree_setup_hook_timeout_ms`, `fleet_view`, `fleet_view_placement`, `wait_tool`, `missions`. `rg -c 'fleetKeybindings|legacyChainControls|asyncWidget|inlineToolDisplay'` (and the snake_case forms) = 0 for all four.
**upstream** — Two are **in-baseline** (hence the `not-ported` kind): `asyncWidget?: boolean` at `shared/types.ts:1750` @v0.43.0 ("Show the under-editor async runs widget. Defaults to true") and `inlineToolDisplay?: InlineToolDisplay` at `:1754` ("Inline chat rendering for the subagent tool. Defaults to rich"). Two are **drift**: `fleetKeybindings` (`:1827` @v0.47.1, validated at `extension/config.ts:26-36` against `FLEET_KEYBINDING_ACTIONS`, from `da98baa`, v0.46.0) and `legacyChainControls` (`:1833`, validated at `config.ts:52-54`, used at `tool-description.ts:156-170` to trim the append-step/checkpoint guidance, from `889a798`, v0.47.0).
**Impact** — Four documented keys are accepted into `config.json` and dropped with no validation error. Concretely: a user cannot hide the under-editor async widget while keeping FleetView, cannot switch the subagent tool's inline chat rendering, cannot rebind any Fleet key, and cannot trim the legacy chain-control guidance out of the tool description to save context. The same advertised-and-ignored shape as the closed SUBA-N05.
**Fix** — Add all four to `SubagentExtensionConfig` with upstream's validation errors verbatim; wire `async_widget` into the `set_widget` calls at `extension.rs:9489/:9889/:9978`, `fleet_keybindings` into `tui/fleet.rs`'s key dispatch, `inline_tool_display` into the renderers at `extension.rs:9616/:9646`, and `legacy_chain_controls` into the description builder **SUBA-025** introduces — so sequence after it.
**Verify** — Each key must round-trip through `config.json` and change observable behaviour; an invalid value for any of them must produce upstream's exact error text rather than being dropped.

## SUBA-062 — cyrup's bundled `researcher` agent cannot do web research because the crate's target has no web tools

**Kind** cyrup-original · **Severity** low · **Effort** L · **Confidence** confirmed
**cyrup** — `crates/cyrup-ext-subagents/resources/agents/researcher.md:4` declares `tools: read, grep, find, ls, write, intercom` and its body rewrites every web step into a filesystem step ("use `find`/`ls` to locate them, `grep` to search across them"), with a "Note on web access" section replacing upstream's workflow. The cause is upstream of this crate: `ls crates/cyrup-tools/src/tools/` is `bash, edit, edit_diff, find, globmatch, grep, ls, read, write` — no `web_search`, `fetch_content` or `get_search_content` — and `rg -l 'web_search|fetch_content|get_search_content' crates/` matches only two files inside cyrup-ext-subagents itself (a test and a doc string).
**upstream** — `pi-subagents/agents/researcher.md:3-4` @v0.47.1 — `description: Autonomous web researcher — searches, evaluates, and synthesizes a focused research brief`, `tools: read, write, web_search, fetch_content, get_search_content, intercom`, with body rules built on `web_search`.
**Impact** — A user who delegates to the shipped `researcher` expecting pi's behaviour gets a working-tree grep instead of web research, and the divergence is invisible until the brief comes back sourced entirely from local files. Filed as `cyrup-original` rather than a bundled-agent bug because the agent file is a faithful adaptation to a real missing capability.
**Fix** — **Handoff:** file the missing `web_search` / `fetch_content` / `get_search_content` tools against the tools area (04 built-in tools / 12 pi drift) — they are not this crate's to build. Inside this crate the only change owed now is a `[CYRUP-DELTA]` header in `resources/agents/researcher.md` naming the upstream file it diverges from and why, so **SUBA-044**'s bundled-agent diff does not read it as drift; restore upstream's text the moment the tools land.
**Verify** — Once the web tools exist, `diff <(git -C pi-subagents show v0.47.1:agents/researcher.md) crates/cyrup-ext-subagents/resources/agents/researcher.md` must be empty.

## SUBA-063 — Zero-tool-budget authorisation and the runtime-extension acknowledgement path are unported — **PARTIALLY CLOSED 2026-09-16**

> **PARTIALLY CLOSED 2026-09-16, cyrup `cc7818b`. The cyrup line below — "`rg -c 'ZERO_AUTH|RUNTIME_ACKNOWLEDGED'`
> = 0 for both" — is false for the first name: it is 12 hits.**
>
> **IN (1) — the zero-budget DECODE half.** `exec/tool_budget.rs:39` defines
> `TOOL_BUDGET_ZERO_AUTH_ENV = "CYRUP_SUBAGENT_TOOL_BUDGET_ZERO_AUTH"` under the crate's
> `CYRUP_SUBAGENT_*` rename, with `HardMinimum::{One,Zero}` (`:44-71`) as a two-variant enum rather
> than an integer — upstream's `minimumHard?: 0 | 1` admits exactly those. `HardMinimum::from_env`
> (`:57-64`) is exact string equality on `"1"`, no trim, as upstream. It is read in PRODUCTION
> child-side at `prompt_runtime.rs:2332`, inside `prompt_runtime_from_env`, which is the child's real
> entry point.
>
> **IN (3) — the events-cap override, the sub-part this row's own correction called "separately and
> cheaply".** `background/runner_main/events.rs:14` defines `ASYNC_EVENTS_MAX_BYTES_ENV`, `:24`
> `resolve_async_events_cap_bytes` parses it with upstream's own numeric tolerance (`1e6`, `50.9`,
> rejecting `""`/`lots`/`-1`), and `:49-50` feeds the result to `BoundedJsonlWriter::create_with_cap`
> for the run's real `events.jsonl`. Re-exported at `runner_main/mod.rs:88`.
>
> **STILL OPEN, and the first of these is the sharp one — a reader with no writer.** Nothing anywhere
> in the workspace WRITES the zero-auth variable: all 12 hits are in the two reader files, and
> `exec/spawn_plan.rs:1173` writes `TOOL_BUDGET_ENV` with no sibling. Upstream writes it at
> `pi-args.ts:1032` from `input.allowZeroToolBudget`. So the child can honour `hard: 0` and no
> production path can ask it to — the capability is built and unreachable, the exact shape this area's
> audit note names as its recurring defect. **The blocker the Fix recorded is discharged:** it made the
> zero flag depend on `SUBA-047` "for a caller surface", and `SUBA-047` is closed — `toolBudget` is
> advertised at `extension/tool/schema.rs:546` and validated at `extension/tool/routing.rs:736,825`,
> where `hard: 0` is currently refused with *"toolBudget.hard must be an integer >= 1."*
> (`routing_tests.rs:604`, `:1801`) because the parent always validates at `HardMinimum::One`. Only
> the writer and the caller's explicit-zero plumbing are owed.
> Also still open: the entire acknowledgement path — `git grep -c 'RUNTIME_ACKNOWLEDGED|runtime_acknowledged|acknowledged_extensions'`
> = 0 — and the truncation marker, upstream `TRUNCATED_EVENT_TYPE = "subagent.events.truncated"`
> (`subagent-runner.ts:320` @v0.68.0, `:266` @v0.43.0 — in-baseline, so the marker was portable at the
> measured baseline and the cap override was the drift half).
>
> **Both landed parts were already true at `d53763b`, and the `## Open items` row recorded them at
> `9aeba769`** — this section and the `## Status table` row are what lagged. The `BoundedJsonlWriter`
> refutation recorded in the parenthetical below remains correct.

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed
*(Corrected this pass. The refuter's caveat, stated inline: the audit's third part — "the async events log is unbounded" — is **refuted and struck**. `crates/cyrup-ext-subagents/src/jsonl.rs` is a shared `BoundedJsonlWriter` with `DEFAULT_JSONL_CAP_BYTES = 50 * 1024 * 1024` at `:51`, the same 50 MB as pi's `DEFAULT_MAX_ASYNC_EVENTS_BYTES`, and `runner_main.rs:711` opens the run's `events.jsonl` through it. `events.jsonl` **is** capped. What is missing is only the `PI_SUBAGENT_ASYNC_EVENTS_MAX_BYTES` override and the `subagent.events.truncated` marker event — a much smaller gap, noted here so nobody builds a cap that already exists.)*
**cyrup** — An exhaustive diff of the child-env surface — `rg --only-matching '"CYRUP_SUBAGENTS?_[A-Z0-9_]*"' crates/cyrup-ext-subagents/src | sort -u` (48 names) against the upstream inventory (42 `PI_SUBAGENT*` names at v0.43.0) — shows no counterpart to `PI_SUBAGENT_TOOL_BUDGET_ZERO_AUTH` or `PI_SUBAGENT_RUNTIME_ACKNOWLEDGED_EXTENSIONS`; `rg -c 'ZERO_AUTH|RUNTIME_ACKNOWLEDGED'` = 0 for both.
**upstream** — @v0.43.0: `PI_SUBAGENT_TOOL_BUDGET_ZERO_AUTH` is defined at `runs/shared/tool-budget.ts:5`, written at `pi-args.ts:771` (`input.allowZeroToolBudget ? "1" : undefined`) and read child-side at `subagent-prompt-runtime.ts:476` (`decodeToolBudgetEnv(…, { allowZero: … === "1" })`, `{minimumHard: 0}`); `RUNTIME_EXTENSION_ACK_PATH_ENV` is written at `pi-args.ts:604-609` and returned as `runtimeAcknowledgedExtensionsPath` at `:787` for the parent to read back.
**Impact** — (a) A `toolBudget` of zero — "this child may make no tool calls at all" — cannot be distinguished from "no budget", so a deliberately tool-less lane cannot be expressed. (b) The parent has no record of which runtime extensions the child actually acknowledged, so an extension that silently failed to load in the child looks identical to one that loaded — the same blind spot as **SUBA-045**, one layer up.
**Fix** — Add the zero-authorisation flag to `exec/tool_budget.rs`'s encode/decode pair, set from the caller's explicit `0` (depends on **SUBA-047** for a caller surface); write an acknowledgement path into the child env in `build_attempt_spawn_plan` and read it back in `run_attempt` alongside SUBA-045's tool diagnostic (same temp dir, same read-back point). Separately and cheaply: add the `ASYNC_EVENTS_MAX_BYTES` override and the truncation marker over the existing `BoundedJsonlWriter`.
**Verify** — `toolBudget:{hard:0}` must block the child's first tool call rather than being treated as unset; a child asked to load a nonexistent extension must leave an acknowledgement file that omits it and the parent must report it.

## SUBA-065 — `unknownSubagentActionMessage` — the did-you-mean recovery and its destructive-action safety gate — is unported

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed
*(Filed from the refuter's sweep. Note it comes out of `28b9222`, a commit the audit's coverage claims to have diffed line by line — evidence that "diffed the commit" and "derived every behaviour in it" are different acts.)*
**cyrup** — `crates/cyrup-ext-subagents/src/extension.rs:7610-7614` returns a static hand-written list with no suggestion machinery, and there is no destructive-action set anywhere in the crate (`rg DESTRUCTIVE` = 0; `MUTATING_MANAGEMENT_ACTIONS` at `discovery/management.rs:1387` is a different 7-element set serving the child-safe denylist).
**upstream** — `pi-subagents/src/runs/foreground/subagent-executor.ts:167-208` @v0.47.1 — `DESTRUCTIVE_MANAGEMENT_ACTIONS` (13 verbs incl. `delete`, `eject`, `reset`, `stop`, `interrupt`, `reject-checkpoint`, `schedule.delete`), `editDistance` (`:170`), `hasSingleAdjacentTransposition` (`:186-192`), and `unknownSubagentActionMessage` (`:195-208`) which suggests the closest `SUBAGENT_ACTIONS` candidate but applies a **deliberately stricter** rule to destructive candidates — `distance === 1 && requested.length >= candidate.length - 1` (`:200`) — so a loose typo is never nudged toward `delete`. It also appends a fixed nextStep. At v0.43.0 the message was the bare `Unknown action: ${action}. Valid: …` at `:4861`, so this landed in range via `28b9222`.
**Impact** — A model that mistypes an action gets a wall of 27 names and no suggestion, so recovery costs a turn. More interesting is the half cyrup could get *wrong* later: porting a naive did-you-mean without the destructive gate would nudge a typo toward `delete`, which is exactly what upstream's asymmetric rule exists to prevent. Low today, but it is a landmine for whoever improves the message casually.
**Fix** — Port `DESTRUCTIVE_MANAGEMENT_ACTIONS`, `edit_distance`, `has_single_adjacent_transposition` and `unknown_subagent_action_message` into `extension.rs` beside the unknown-action arm, **including the asymmetric destructive rule verbatim**, and derive the candidate list from the enum. Land with **SUBA-038**, which rewrites the same three messages.
**Verify** — `{action:"statu"}` must suggest `status`; `{action:"delet"}` must **not** suggest `delete` (distance 1 but `requested.length < candidate.length - 1` fails the gate is the wrong direction — assert against upstream's own table); an unknown action with no near candidate must fall back to the plain list.

## SUBA-066 — The `/subagents-guide` slash command is unported, and falls outside both VL-S11 and SUBA-055 — **CLOSED 2026-08-15**

> **CLOSED 2026-08-15 (sweep 11)**, landed with `SUBA-055` as this item's own Fix line instructs.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed
**cyrup** — The complete slash set is the 16-variant `as_str` match at `crates/cyrup-ext-subagents/src/registration/slash_commands.rs:127-146`; `subagents-guide` is not in it.
**upstream** — `git show v0.47.1:src/slash/slash-commands.ts | grep 'registerCommand('` yields 15: `subagents`(`:655`), `run`(`:662`), `subagent-cost`(`:692`), `subagents-doctor`(`:699`), **`subagents-guide`(`:706`)**, `subagents-refine`(`:721`), `subagents-fleet`(`:734`), `subagents-detach`(`:744`), `subagents-stop`(`:771`), `subagents-models`(`:822`), `subagents-profiles`(`:845`), `subagents-load-profile`(`:857`), `subagents-refresh-provider-models`(`:910`), `subagents-generate-profiles`(`:944`), `subagents-check-profile`(`:982`).
**Impact** — The user-facing half of the guide feature has no owner: PARITY-GAPS VL-S11 names exactly three missing commands (`/subagents`, `/subagents-refine`, `/subagents-detach`) and **SUBA-055** scopes itself to the `guide` *action* plus the packaged docs. A user who reads pi's docs and types `/subagents-guide` gets an unknown command. Low severity, but this is precisely the kind of item that silently survives a "guide is filed" checkmark on the next pass.
**Fix** — Add the `SubagentsGuide` variant and descriptor to `registration/slash_commands.rs:127-146`, routing to the same `read_subagent_guide` **SUBA-055** introduces. Land the two together; this is the last mile of that item, not an independent subsystem.
**Verify** — `/subagents-guide` must render the overview topic, and `/subagents-guide tool-reference` the tool-reference topic, identical to the corresponding `{action:"guide"}` output.

---

## SUBA-067 — The descendant-termination fixture exec-collapsed to a single process, so the test never exercised group-kill

**Kind** test-defect · **Severity** high · **Effort** S · **Confidence** confirmed · **Status** FIXED
**cyrup** — `spawn/mod.rs:1633-1636` built `sh -c 'echo $$ > …; mv …; exec sleep 300'` and handed it to `sh_command()` (`mod.rs:1098-1110`), which runs `/bin/sh -c <script>`. A `-c` script that is a SINGLE simple command triggers the shell's exec-through optimization: the outer shell replaces itself with the inner `sh`, which then `exec`s `sleep` — one pid, never a tree. Measured directly: `ps -o pid,ppid,pgid,command` on the direct child showed that pid IS `sleep`, and the `assert_ne!` at `mod.rs:1653` reported `left: 22802 / right: 22802`.
**upstream** — Not an upstream-behaviour question: the fixture never constructed the scenario the test names, so the assertion could not have been evaluated either way.
**Impact** — The test is the ONLY coverage of the claim that the escalation ladder reaches a child's descendants, and it was testing nothing: it died in its own precondition before `terminate()` ran. The production mechanism it targets is in fact correct, and was verified independently — `Command::process_group(0)` (`mod.rs:515`) plus the leader-only negation in `send_signal` (`signal.rs:353-364`, `getpgid(pid)==pid` → target `-pgid`); with a fixture that really forks, one `kill(-pgid, SIGTERM)` killed both child and grandchild. A permanently-red test that also covers nothing is the worst of both.
**Fix** — LANDED. Appended a trailing `; :` so the outer shell is no longer running a single simple command and must stay resident to fork the inner one. Verified on this machine: outer `/bin/sh` 22900, descendant 22902, distinct. Deliberately NOT `… & wait`, which would make the descendant ASYNCHRONOUS and therefore `SIG_IGN` for SIGINT under POSIX — exactly the confound the test's own comment warns against.
**Verify** — `cargo test -p cyrup-ext-subagents --lib spawn::tests::terminate_reaches_the_childs_own_descendants_not_just_the_direct_child` green, and the `assert_ne!` still fires if the `; :` is removed.

---

## SUBA-068 — The worktree setup-hook timeout fixture raced macOS first-exec verification latency with a 200 ms budget

**Kind** test-defect · **Severity** high · **Effort** S · **Confidence** confirmed · **Status** FIXED
**cyrup** — `spawn/worktree.rs:1632` failed inside the `wait_for_published_pid` PRECONDITION helper ("the hook never published its pid … within 5s"), not on the kill assertion; the `timed out` assertion above it passed, so the kill path had already run. Root cause is ambient: macOS pays a one-off verification cost on the first `exec` of a freshly written executable whose exact content it has not seen. Measured here with a standalone harness — unique script content: 242.38 / 197.67 / 206.91 / 207.63 / 203.26 / 206.77 ms to first write (6/6 runs); identical content reused: ~130 ms. `tempfile::tempdir()` randomizes the path, and the path is embedded in the script body, so the content is unique on EVERY run and the cost recurs every run. Against `timeout_ms: 200` (`worktree.rs:1576`) the SIGTERM always landed before `echo $$ > pid` could execute.
**upstream** — Production is already exact parity and was NOT touched: pi's `runWorktreeSetupHook` is `spawnSync(hook.hookPath, [], { …, timeout: hook.timeoutMs, shell: false })` (`pi-subagents v0.47.1 src/runs/shared/worktree.ts:328-334`), and Node's `spawnSync` timeout kills the direct pid with the default `SIGTERM`. The 1000 ms SIGTERM→SIGKILL grace in `signal.rs:190` is pi's own number from `acceptance.ts:1164-1177` (`child.kill("SIGTERM")` then `setTimeout(() => child.kill("SIGKILL"), 1000)`), read at the tag.
**Impact** — The test could not pass on this machine regardless of implementation correctness, because it asserted a property of process-startup latency that it does not control.
**Fix** — LANDED, in two parts, because raising the budget alone was NOT enough: at `timeout_ms: 3_000` the test still failed under the full suite's parallel load. The dominant term is removed instead of guessed around — the fixture now `exec`s the SAME hook file once with `$WARMUP` set (a new first-line short-circuit that exits before the pid is published), paying macOS's verification cost before the clock starts, and keeps a 3000 ms budget to absorb ordinary scheduling jitter. The assertion's meaning is unchanged: the hook still `exec sleep 300`, so it still blows its budget and the timeout arm still fires.
**Verify** — `cargo test -p cyrup-ext-subagents --lib` = **2218 passed / 0 failed**, run twice with all 24 cores saturated by spinners (load average 25-32); `spawn::worktree::tests::` alone 20/20, 3× under the same load.

---

## SUBA-069 — The whole worktree setup-hook test family is wall-clock-budgeted, and goes red under machine load

**Kind** test-defect · **Severity** high · **Effort** M · **Confidence** confirmed · **Status** OPEN
**cyrup** — Three further tests in the family — `runs_a_repo_relative_setup_hook_and_records_synthetic_paths` (`worktree.rs:1544`), `rejects_tracked_synthetic_paths_from_hook_output` (`:1691`), `excludes_hook_created_synthetic_files_from_captured_patch` (`:1717`) — all fail with `WorktreeSetup("worktree setup hook timed out after 5000ms")` when the machine is loaded, and all pass in isolation. These use the production DEFAULT 5000 ms hook timeout rather than a fixture constant, so unlike SUBA-068 they cannot simply be re-budgeted.
**Measured** — Controlled A/B on this box, varying ONLY `worktree.rs` and holding everything else constant. With two other agents running cargo concurrently (load ≈8-10): 4 failed / 2214 passed — the three above plus SUBA-068's — reproduced twice. At load ≈3-5: HEAD `worktree.rs` → 1 failed / 2217 passed (SUBA-068 only, the three siblings green); fixed `worktree.rs` → 2218 passed / 0 failed. So the three sibling failures are load-induced and are NOT caused by the SUBA-068 fix.
**Note on the load that matters** — Synthetic CPU pressure does not reproduce it: with all 24 cores saturated by spinners (load 25-32) the full `--lib` was 2218/2218 green, twice. The failures appear under CONCURRENT CARGO — compilation's I/O and memory pressure, which delays `fork`/`exec` far more than pure CPU contention does. Any reproduction attempt must use a competing build, not a busy-loop.
**upstream** — pi's default is the same 5000 ms (`worktree.ts` `DEFAULT_SETUP_HOOK_TIMEOUT_MS`, applied through `spawnSync(…, { timeout })` at `:328-334` @v0.47.1), so the CONSTANT is parity-correct and must not be raised in production to make tests pass. The defect is that the tests exercise it with real `/bin/sh` scripts under `cargo test`'s full parallelism (2218 tests), where a freshly-written script's first `exec` alone costs ~200 ms (SUBA-068) and scheduling latency adds the rest.
**Impact** — "`cargo test -p cyrup-ext-subagents` is green" is not a reliable gate today: the result depends on what else the machine is doing. An intermittent red is more corrosive than a consistent one, because it trains readers to re-run rather than investigate — and it is what let SUBA-067 and SUBA-068 sit unexamined behind a quoted, never-executed "3932 passed" figure.
**Fix** — Do NOT raise the production default. Options, in preference order: (a) inject the timeout in these three fixtures the way SUBA-068's does, so the budget is a test constant rather than the shipped default; (b) give the hook fixtures stable content at a stable path so macOS's first-exec verification is paid once per suite, not once per test; (c) serialize this module (a shared mutex, as `native_supervisor_channel_integration.rs` already does with `ENV_LOCK`) so hook tests do not compete with 2200 siblings. Re-measure under deliberate load after any of them.
**Verify** — `cargo test -p cyrup-ext-subagents --lib spawn::worktree::tests::` green while the box is held at load ≥8 (e.g. a concurrent `cargo build` of the workspace), repeated 3×.

---

## SUBA-097 — `scheduled_runs`' production launcher is claimed to be integration-tested by a file that does not exist

**Kind** cyrup-original *(test-coverage claim)* · **Severity** low · **Effort** S · **Confidence** confirmed
*(Filed 2026-09-16 while verifying `SUBA-016`'s closure, per this directory's rule that a defect found while auditing is a finding.)*
**cyrup** — `crates/cyrup-ext-subagents/src/background/scheduled_runs/trigger.rs:884-888` justifies stubbing the launcher in its unit tests with: *"The production launcher spawns a real `RunMode::Workflow` run; these rows are about the state machine AROUND that launch … so a stub is the right seam and the integration suite (`tests/scheduled_runs_integration.rs`) covers the production one."* **There is no such file.** `crates/cyrup-ext-subagents/` has no `tests/` directory at all — its tests live at `src/tests/` (15 files, none named for schedules) — and `find . -name 'scheduled_runs_integration*' -not -path './tmp/*'` returns nothing workspace-wide. The seam the sentence promises is covered is `ExecutorScheduleLauncher` (`extension/executor/scheduled_runs.rs:39-51`), which has **exactly three references in the crate**: its declaration, its `impl ScheduleLauncher`, and its one construction at `:317`. No test names it.
**upstream** — N/A; this is a cyrup-side coverage claim, not a parity gap.
**Impact** — The one component of the `scheduled_runs` subsystem that actually spawns a `RunMode::Workflow` run — the boundary between a correct state machine and a schedule that fires into nothing — is untested, and the code says the opposite in the place a reader looks to check. A reviewer who greps for the named file finds nothing; a reviewer who trusts the sentence approves an uncovered seam. That the surrounding suite is unusually strong (`scheduled_runs_tests.rs` drives all nine verbs through `Tool::execute`, 100/100 green) makes the one gap easier to miss, not harder.
**Fix** — Either write the integration row the comment promises — arm a schedule whose target is a trivial `workflowScript`, let `ExecutorScheduleLauncher::launch` run for real, assert a run directory with the schedule's `ScheduleOrigin` — or, if that is deferred, correct the sentence to say the production launcher is *not* covered and why. Do not leave the claim standing; a false coverage claim is worse than an acknowledged gap because it stops the next reader from looking.
**Verify** — `git grep -n 'scheduled_runs_integration'` must either resolve to a real test file or return nothing at all.

## SUBA-098 — A stray CJK token inside an English doc comment in the scheduled-runs test module

**Kind** cyrup-original *(corpus health)* · **Severity** low · **Effort** S · **Confidence** confirmed
*(Filed 2026-09-16 while verifying `SUBA-016`'s closure.)*
**cyrup** — `crates/cyrup-ext-subagents/src/extension/tool/scheduled_runs_tests.rs:31` reads *"so a trigger that read it live would观察 a different identity after the swap"* — the verb has been replaced by the Chinese 观察 with no surrounding space. A Unicode-script sweep of the workspace (`rg '[\p{Han}\p{Hiragana}\p{Katakana}\p{Hangul}]' crates/ -g '*.rs'`) shows every other match is deliberate test DATA — CJK width fixtures in `cyrup-tui`, a `"日"` in the schedule-interval parser's non-ASCII rejection table, an `"あ"` repeat in the output-truncation test — with exactly **one** other accidental instance, `crates/cyrup-tui/src/tests/selector.rs:41` (*"an empty/末-position caret"*), which belongs to area 07 and is named here only so the sweep is not re-run.
**upstream** — N/A.
**Impact** — Cosmetic in isolation; recorded because it is in the doc comment that explains why `SwappableSessionHost` is mutable, which is the load-bearing justification for two of the session-pinning tests, and because a corrupted token in a shipped doc comment is the kind of thing that survives indefinitely once no one files it. Trivial to fix, trivial to verify, and this directory's rule is that ids are never deleted — so it can be closed in one line whenever the file is next touched.
**Fix** — Replace 观察 with `observe` at `scheduled_runs_tests.rs:31`. Area 07 owns the `selector.rs:41` twin.
**Verify** — `rg '[\p{Han}\p{Hiragana}\p{Katakana}\p{Hangul}]' crates/cyrup-ext-subagents/src -g '*.rs'` must return only the two deliberate test-data lines (`scheduled_runs/schedule.rs`, `inspect_rpc/read_output.rs`).

## SUBA-070 — the `interactive` frontmatter key is parsed and typed but never enforced

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed
**cyrup** — `interactive` is in the recognised key list (`crates/cyrup-ext-subagents/src/discovery/frontmatter.rs:96`), parsed at `:869`, and stored in its own typed field at `:1024`. Nothing reads that field to change spawn behaviour. The deferral is deliberate and pinned by a test: `:1595-1602`, named `interactive_is_parsed_into_typed_field_and_never_dropped_from_extra_fields_expectation`, whose comment states *"func-SA §4.1: `interactive` is parsed but unenforced in v1 — it MUST still be typed"*.
**upstream** — `pi-subagents` honours `interactive` when deciding whether a child may prompt. Tag and line **not re-read this pass**; establish before fixing.
**Impact** — an agent file declaring `interactive: true` gets default behaviour with no warning. Because the key *is* recognised, it does not fall through to `extra_fields` either, so no other layer — including the permission system, which consumes `extra_fields` — can act on it. Same advertised-but-unenforced shape as `SUBA-061`, which names four different keys (`asyncWidget`, `inlineToolDisplay`, `fleetKeybindings`, `legacyChainControls`).
**Fix** — consume the typed field where the child's prompt capability is decided; leave the parse and its test as they are.
**Verify** — an agent with `interactive: true` and one without produce different spawn behaviour on the same task, and the existing parse test still passes.
**Note** — filed because this area's `## Coverage` blind spot 6 predicts exactly this class (*"the unwired class is unsampled … a type-resolved pass over this crate specifically would very likely return more"*). The v1 rationale is recorded here so the item is not mistaken for an unnoticed defect.

---

## SUBA-071 — subagent settings are read from two files that can disagree — **CLOSED 2026-08-15 (REFUTED)**

> **REFUTED, CLOSED 2026-08-15 (sweep 11).** There is ONE settings store and its precedence is
> upstream's (project beats user, pi `agents/agents.ts:924-931` @v0.43.0). The
> `SettingsManager::effective().get("subagents")` read this item is built on **exists nowhere in the
> crate** — it was a stale doc comment, now deleted — and the item's cyrup citation
> (`registration/mod.rs:625-640`) points at `ProactiveSkillSubagents`. Everything below is the filing
> text; see the table row for the full refutation.

**Kind** cyrup-original · **Severity** medium · **Effort** M · **Confidence** confirmed
**cyrup** — the `subagents` block is read from the layered `settings.json` via `effective().get("subagents")` (`crates/cyrup-ext-subagents/src/registration/mod.rs:625-640`) and, separately, from `~/.cyrup/agents/settings.json` and `<project>/.cyrup/agents/settings.json` on the discovery path (`crates/cyrup-ext-subagents/src/extension.rs:1580-1596`, `discovery/mod.rs:359-368`). `crates/cyrup-ext-subagents/src/registration/profiles.rs:227-232` documents the divergence **in-source**. The two are never merged and no precedence rule exists between them. Failure modes also differ: a malformed discovery-side file **aborts** discovery (`extension.rs:1590-1594`), while the layered side degrades.
**upstream** — `pi-subagents` reads one location. Tag and line **not re-read this pass**.
**Impact** — a user who sets `subagents.defaultModel` in `~/.cyrup/agent/settings.json` — the file every other part of cyrup calls *the* settings file — does not affect discovery-side behaviour, and vice versa. Which half of the crate honours a given key is not discoverable without reading the source. The two paths do not even share a root: the layered file lives under `~/.cyrup/agent` while the discovery one lives under `~/.cyrup` (`extension.rs:5138-5143`), so they are not siblings and a user correcting one will not find the other.
**Fix** — pick one location; read the other for backward compatibility behind a deprecation diagnostic that names both paths and states which won. Do not merge silently — a silent merge makes the precedence question permanently unanswerable from behaviour.
**Verify** — set the same key to different values in both files, assert the documented winner, and assert the diagnostic is emitted naming both paths.
**Note** — `registration/profiles.rs` is named in this area's `## Coverage` blind spot 5 as one of the cyrup-side files that pass never opened, which is why an in-source divergence note went unfiled.

---

## Coverage

### Read first-hand at cyrup HEAD `04c1ba2` (tree clean; docs-only `a9000b1` on top)

In full or in the cited regions: `extension.rs` (schema builder `6543-6690` — **all 45 `props.insert` names enumerated, twice, independently**; action enum `6557` — **all 27 verbs counted**; `route_management`/`route_control` `7560-7712` and `7780-7860`; dispatch tests `12580-12960`, `14120-14200`, `14640-14690`; init `9200-9420`; spawn budget `760-800`, `8317`, `10042`, `10454`, `10625`), `exec/{mod,structured,acceptance,fallback,control,model_scope,tool_budget,mcp_direct_tools}.rs`, `prompt_runtime.rs` (module doc, `660-740`, `1590-1730`), `spawn/{mod,signal,parallel,chain_graph}.rs`, `background/{wait,watch,control,cascade,runner_main}.rs`, `discovery/{frontmatter,skills,management,settings_write}.rs`, `registration/{mod,doctor,slash_commands}.rs`, `artifacts.rs`, `native_supervisor.rs`, `jsonl.rs`, `tui/notices.rs`, and all six files under `crates/cyrup-ext-subagents/resources/agents/`. Outside the crate, for the receiving and producing ends: `crates/cyrup/src/main.rs:489,638,732`, `crates/cyrup-provider/src/api/{openai_responses,anthropic_messages,google_generative_ai,openai_completions}.rs`, and `ls crates/cyrup-tools/src/tools/`.

### Read first-hand upstream, at tags only

`git show v0.43.0:<path>` and `git show v0.47.1:<path>`, never clone HEAD: `extension/{schemas,config,tool-description,subagent-guide,index}.ts`, `shared/{types,settings,artifacts,utf8,prompt-resources}.ts`, `runs/shared/{pi-args,acceptance,model-scope,model-fallback,tool-availability,tool-budget,turn-budget,subagent-prompt-runtime,spawn-budget}.ts`, `runs/foreground/{subagent-executor,execution,async-dismiss-action}.ts`, `runs/background/{async-execution,completion-replay,wait-completions,resume-guidance,active-run-index,scheduled-runs}.ts`, `agents/{frontmatter,skills}.ts`, `policy/authority.ts`, `api/delegation.ts`, `slash/slash-commands.ts`, and all six `agents/*.md`. `not-ported` vs `upstream-drift` was decided by `git cat-file -e v0.43.0:<path>` in every case where it mattered — which is how SUBA-017, SUBA-021 and SUBA-022 were re-classified.

### Version-lag sweep (new territory)

The full `v0.43.0..v0.47.1` range was swept — the workspace brief measures it at 151 files / +10254 / −1333; the src-only sweep covered 96 non-merge commits, 67 files, +4696/−769 and **12 net-new source files**, none of which any prior pass had seen (`PARITY-GAPS.md` records v0.43.0 as "latest"). All 12 new files were read: `subagent-guide.ts`, `active-run-index.ts`, `resume-guidance.ts`, `utf8.ts`, `prompt-resources.ts`, `completion-replay.ts`, `wait-completions.ts`, `async-dismiss-action.ts`, `foreground-history.ts`, `workflow-foreground-steering.ts`, `display-text.ts`, `api/project-panes.ts`. Commits diffed line by line: `94b0cb1`, `635c1bd`, `a4fc59a`, `87420e5`, `b69aafb`, `ade35ea`, `0b1976b`, `0677ac0`, `c2add95`, `28b9222`, `38bc408`, `889a798`, `b283d27`, `bc1b689`.

### Surface-driven sweeps run (three, diffed as sets, not spot-checked)

1. **Child env vars** — 42 `PI_SUBAGENT*` names @v0.43.0 vs 48 `CYRUP_SUBAGENT*` names at HEAD. Six upstream names have no cyrup counterpart; five became findings (`SUBA-045` TOOL_DIAGNOSTIC_PATH, `SUBA-049` STEER_ACK_DIR/STEER_CAPABILITY, `SUBA-063` ZERO_AUTH/RUNTIME_ACKNOWLEDGED); `CAPABILITY_CEILING_V1` is PARITY-GAPS VL-S1.
2. **Tool schema** — upstream's 66 top-level `SubagentParamsSchema` keys @v0.43.0 vs cyrup's 45. Of the ~25 with no counterpart, `outputSchema` (`SUBA-043`) and `toolBudget` (`SUBA-047`) are new; `mode`/`steeringRecovery` fold into `SUBA-049`, `additional` into `SUBA-046`; `workflowScript`/`chatProgress`/`resume`/`handoffPath`/`usageBudget`/`turnBudget`/`agentContract`/`gate`/`schedule.*` are already VL-S2/VL-S8/VL-S9/VL-S10/PB-10/PB-11.
3. **Action enum + extension config** — 27 verbs vs 50 (v0.43.0) / 53 (v0.47.1); 18 config fields vs 30 (v0.43.0) / 33 (v0.47.1). Yielded `SUBA-046`, `SUBA-048`, `SUBA-055`, `SUBA-057`, `SUBA-059`, `SUBA-061` and the `SUBA-005` restatement.
4. **Denial/gating paths** (refuter-only, a fourth lens the audit did not run) — swept every refusal site rather than every advertise site. Yielded `SUBA-064` (`authorityPolicy`), `SUBA-065` (unknown-action recovery), `SUBA-066` (`/subagents-guide`), and the third residual inside `SUBA-038`.

### Severity re-derivation (repair pass, 2026-08-12)

The completeness critique's finding 3 (`critical` = data loss, silent wrong output, a permission
bypass, or a crash on a normal path — `README.md:106-107`) was applied to this file's own items
rather than only to the ones it named elsewhere. Two candidates were examined; **both stand where
they are**, and the reasoning is recorded so the next pass does not re-litigate:

- **`SUBA-064` (`authorityPolicy`) stays `medium`, not raised.** It has the shape of a permission
  bypass — an operator writes `"authorityPolicy": {"stopRun": "forbid"}`, the key is silently
  dropped by the only config validator (`registration/mod.rs:256` validates missions and nothing
  else) and the action runs. What holds it at medium is *which* actions are reachable: of upstream's
  six `AUTHORITY_ACTIONS`, only `stopRun` and `steerRun` are implemented in cyrup at all
  (`extension.rs:7805`, `:7825`); `discardWorktree`, `destructiveCleanup`, `spawnBudgetGrant` and
  `scheduleCreate` have no dispatch to bypass. Stopping or steering a subagent run against policy is
  a control-surface divergence, not a destructive one. **If `worktree.discard` /
  `destructiveCleanup` ever land (they are two of `SUBA-005`'s unowned verbs), this item becomes
  `critical` on the day they do, and it must be raised in the same change** — noted in `SUBA-064`'s
  Fix as a hard prerequisite rather than left to be rediscovered.
- **`SUBA-043` (SINGLE-mode `outputSchema`) stays `high`, not raised to `critical`.** The dropped
  parameter is silent, but the *output* is not silently wrong: the run returns free prose where typed
  JSON was requested, which the caller's own parse rejects. It is an unreachable capability, not a
  corrupted result. Kept at the top of the table with `SUBA-014`.
- `SUBA-014` was re-read and stays `high` for the reason the audit gave: the child is *instructed* to
  use a tool it may not have, so the failure surfaces as a model apology rather than a config error —
  bad, but loud at the point of use.

### Rejected with reason — do not re-derive

- **Inherited tool-id sanitation (proposed `SUBA-042`, claimed high).** The claim was that cyrup's `strip_parent_only_subagent_messages` (`prompt_runtime.rs:670-700`) has no counterpart to pi's `portableToolId`/`sanitizeToolHistoryMessage` (`subagent-prompt-runtime.ts:208-266`, selected at `:557`), so a `context:"fork"` child on a non-composite API inherits `call_id|item_id` ids and its first request 400s. **Refuted on mechanism:** cyrup normalizes composite ids at the **provider** boundary, which is where the 400 would occur — `crates/cyrup-provider/src/api/anthropic_messages.rs:809` `normalize_tool_call_id` maps every non-`[A-Za-z0-9_-]` char (including `|`) to `_` and truncates to 64, with a unit test at `:2437`; `google_generative_ai.rs:586` does the same; both feed `transform_messages_with_source` (`openai_completions.rs:892-935`), which rewrites the assistant toolCall id **and** the paired `tool_call_id` consistently via `tool_call_id_map`, so pairing survives. `mistral_conversations.rs:313` and `bedrock_converse_stream.rs:1399` also pass normalizers. Pi has the identical provider-level guard (cyrup's own comment cites `anthropic-messages.ts:1006-1009`), so pi's subagent-level `portableToolId` is belt-and-braces, not the load-bearing defence. What remains is that inherited ids are *persisted* in non-portable form in the child's own history rather than rewritten at the context event — cosmetic, and not worth a work batch as written. **If someone re-derives this, the burden is to exhibit a provider path with no normalizer.**
- The code fact behind it is still true and unfiled: cyrup has no per-API composite gate, so it cannot *preserve* composite ids where they are required. No known consumer needs that today.

### Handoffs to other areas

- **`SUBA-062`'s root cause** — the absent `web_search` / `fetch_content` / `get_search_content` tools — belongs to area 04 (built-in tools) / area 12 (pi drift), not here. Only the `[CYRUP-DELTA]` header is owed in this crate.
- **PARITY-GAPS ids not restated as findings**, confirmed still accurate at HEAD by spot-check: PB-8 (RPC bridge), PB-9 (clarify UI), PB-10 (turnBudget, = SUBA-008), PB-11 (schedule, = SUBA-016), PB-12..PB-14, UW-3..UW-8, VL-S1..VL-S15. The `subagent_wait` rename, wait subscriptions and auto-drain are VL-S8 and were not re-derived.
- **PARITY-GAPS is stale in its header** — it records pi-subagents latest as v0.43.0 and pi-intercom latest as v0.9.2. Both are wrong (v0.47.1 / v0.10.1). Not fixed here; flagged for whoever owns that file.

### Blind spots — read this before the next pass

1. **Static only.** Nothing was executed — no cargo, no npm, no spawned process. Every `Verify` line is a design, not an observation.
2. **The biggest unaudited mass is `workflowScript`, and everything above sits *around* it, not inside it.** Upstream deleted the entire `task`/`tasks`/`chain`/`concurrency`/`chainDir` execution surface at v0.41.0 and replaced it with `workflowScript`; at v0.43.0 the top-level schema has **no `task` key at all** and the whole model-facing tool description is workflowScript-centric. cyrup implements the v0.34.0-era surface. PARITY-GAPS files this as one `large` item (VL-S2), but it is not one item — it is a different execution model whose per-behaviour consequences (mixed sequential/parallel phases, `runs.ref`, `emit`, per-child gates, `prompts.render`, `chatProgress`, retained-child `resume`, `children.list`) were **not** decomposed by this pass or any prior one. **Treat this area's open count as a floor by a wide margin.**
3. **Not read at all on the upstream side**, so anything in them is invisible here: `src/workflows/scripted-workflow.ts` (502 lines, +231 in range), `src/inspectors/herdr/project-panes.ts` (+524), `src/runs/background/async-job-tracker.ts` (+426 — the v0.47.0 event-driven rewrite), `src/runs/foreground/foreground-history.ts` and `workflow-foreground-steering.ts` (new, 137 + 187), `src/shared/display-text.ts` (new, 100), `src/tui/{render,fleet}.ts` (+343 combined), `src/missions/workflow-state.ts` (+209), `src/extension/rpc.ts`. Related and specifically unfiled: the **steering-recovery hardening across v0.44–v0.47** in `subagent-prompt-runtime.ts` (safety poll, settle fallback, `awaitingSettlement`) was read but **not** compared line by line against cyrup's `prompt_runtime.rs` `SteeringInbox`, which targets the v0.43.0 shape. Drift there is likely; diff it when SUBA-049 is scheduled.
4. **`run-fanout-budget.ts`** (257 lines — a whole new per-run logical fan-out cap with config, doctor check and status surface) landed on pi-subagents `main` at `17b4078`/`668c587` **after v0.47.1**. Deliberately not filed, because the hard rules require citing a named tag. Pick it up on the next tag.
5. **cyrup-side files not opened this pass:** `spawn/{chain_graph beyond the instruction builder, dynamic_fanout, nested_events, worktree}.rs`, all of `missions/`, all of `watchdog/` beyond confirming the subtree exists and is registered, `tui/{fleet,fleet_transcript,fleet_status,render}.rs`, `registration/{cost,profiles,resources}.rs`, `exec/{output,ndjson,task_intent,agent_refinements}.rs`. **SUBA-024's two named blind spots (`chain_graph.rs` pre-walk validation, `ChainStepConfig` unknown-key handling) are still not re-read — third pass running.**
6. **The unwired class is unsampled.** PARITY-GAPS §7 states its unwired sweep was identifier-based and incomplete by construction, with ~120 flagged items untriaged. This pass found the class again *by accident* twice — `SUBA-047` (`toolBudget` fully implemented, unadvertised) and `SUBA-054` (`defaultReads` parsed, rendered, never used) — without running a systematic hunt. A type-resolved pass over this crate specifically would very likely return more, since it is the largest and where batches 8–10 landed the most code. **The single highest-leverage test to write in this area is the schema/dispatch guard asserting every advertised property has a consumer** (named in SUBA-043's Verify); it would have caught SUBA-N05, SUBA-043 and SUBA-047 as a class.
7. **The v0.43.0 baseline is inherited, not re-derived** — the crate still records no version string. Several items are classified `not-ported` vs `upstream-drift` on that assumption. Where it mattered, first-tag presence was re-checked directly with `git cat-file -e <tag>:<path>`, which is how SUBA-021's "post-baseline, out of scope" framing was found to be dead and SUBA-017/SUBA-022 were re-classified.
8. **`spec/` and ADR-0001 are absent from this workspace**, as documented. `R-SA-*` ids in cyrup's comments were used only as grep anchors; no finding rests on one, and where a comment invokes one to justify a divergence it was treated as an unverifiable claim — relevant to **SUBA-030**, whose `[CYRUP-DELTA]` justifies inline-argv delivery while the code's own doc at `spawn/mod.rs:428` asserts a 0600 mode the code never sets.
9. **Closure quality.** Twenty-two items closed this pass and every closure was re-derived from code on both sides, not from a commit message. The audit's own citations were wrong in three places, caught by re-reading: SUBA-002's test range (`:13699/:13716/:13753`, not `:13338-13470`), SUBA-033's second instance (`:1133`, not `:802-810`), and SUBA-008's consumer count (three files, not two). Assume a similar residue in the citations above and treat each as a lead to verify.
