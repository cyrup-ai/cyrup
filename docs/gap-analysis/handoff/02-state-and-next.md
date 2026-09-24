# 02 — State, and what to do next

State section rewritten 2026-09-24 against cyrup code HEAD `ea23ca2`. The
sections after it (*Read this before planning*, *The queue after that*, the open question, MCP) were
written at `e815e08` and are **not** re-verified; read them as history. In particular, MCP is no
longer "not started": `crates/cyrup-mcp` exists and area 13 tracks what is built.

---

## Where the project is

| | |
|---|---|
| workspace | **24 crates, 1,044,991 lines of Rust under `crates/`**; 28,706 `.ts:N` upstream citations naming 16,976 distinct upstream locations; 974 `CYRUP-DELTA` markers |
| unit gate | **11,311 passed, 0 failed, 9 skipped**, line coverage 86.3% (per the root `README.md`, measured at `ea23ca2`; not re-run by this pass) |
| since the last re-measure | `9aeba769..ea23ca2`: 18 non-merge commits, 485 files, +164,754 / −4,932 under `crates/`: subagents SCOPE/lanes/runner identity, the 18-command slash surface, the UW-7 fleet roster, and the new `cyrup-herdr` crate (herdr v0.9.1, no area file, upstream not cloned) |
| upstream tags measured against | pi **v0.87.1**, pi-subagents **v0.71.0**, pi-intercom **v0.14.0**, pi-mcp-adapter **v2.37.0**, code_puppy_core_plugins **v0.0.62**, pi-acp **v0.0.33**, pi-permission-system **v0.8.0**. Window stats are in `../README.md` *Baselines measured against* |

### Open ledger rows

Derive the number; do not copy it from here. `python3 docs/gap-analysis/scripts/count_open_items.py`
at the end of the 2026-09-24 pass printed **124 open (1 critical, 2 high, 24 medium, 97 low), 7
trackers, 621 closed** across areas 01–12, 09a, 09b and 14. The script now reads `09b`. Areas 13 and
15 are outside that count by the standing rule.

**The set above medium, empty since 2026-09-05, has refilled with three rows**, all
`upstream-drift`, all filed in this pass:

- `SEAM-122` (critical, area 08): importing a session whose file name already exists in the session
  dir overwrites the stored session. pi v0.85.0 renames the copy.
- `TOOL-047` (high, area 04): a shell command killed by a signal is reported to the model as a
  success. pi v0.86.0 reports `128 + signo` as a failure.
- `SUBA-110` (high, area 09b): `GIT_DIR`, `GIT_INDEX_FILE`, `GIT_CONFIG_*` and similar are not
  removed before the background runner or an external CLI starts (pi-subagents v0.71.0).

Outside the count: `MCP-540` (high, area 13), a higher-precedence config that switches a server
between `command` and `url` keeps the old transport's fields.

### What the 2026-09-24 pass did

- **Re-pulled every upstream into `tmp/`** and re-read every area file at `ea23ca2` against the new
  tags. Each file carries a 2026-09-24 pin block. `09b` is new and owns pi-subagents
  `v0.57.0..v0.71.0`.
- **Closed:** `CFG-073` (refuted). In area 15, `ACP-121`, `145`, `209`, `219`, `291` (critical) and
  `ACP-005`, `056`, `122`, `140`, `221` (high), all built in `0aefd08`/`cb290d1`.
- **Filed:** area 01/12 `PROV-073`…`082`, `DRIFT-056`/`057`; area 02/03/06 `AGENT-038`…`041`,
  `SESS-051`…`055`, `EXT-077`…`080`; area 04/05/08 `TOOL-046`…`050`, `CFG-081`…`085`,
  `SEAM-120`…`122`; area 07 `TUI-098`…`103`; area 09b `SUBA-107`…`113`; area 11 `ICOM-062`…`067`;
  area 13 `MCP-540`…`550` (next id `MCP-551`).
- **`user_bash` fails open in cyrup and closed in pi v0.86.0.** It is filed twice, as `DRIFT-056`
  and `EXT-077`; fix it once.

### Windows still unmeasured

- pi `v0.84.1..v0.85.1`: the 2026-09-14 census leads are still mostly unverified.
- pi `v0.85.1..v0.87.1`: `packages/agent/src/harness/**` (incl. ~9k-line `pico3`), `packages/durable`,
  `packages/chord`, the `packages/ai` anthropic/openai-responses/codex adapters, and most of
  `agent-session.ts`, `interactive-mode.ts`, `runner.ts`, `loader.ts`.
- pi-subagents `v0.57.0..v0.67.0` (leads only) and the large modified files in `v0.67.0..v0.71.0`.
- pi-intercom: no surface sweep of `v0.10.1..v0.14.0`.
- pi-mcp-adapter `v2.32.1..v2.33.0` (leads only), and 159 area-13 rows never re-checked against the
  TypeScript.
- Area 15: the rows not closed this pass were not re-read.
- `cyrup-herdr`: unmeasured entirely.

---

## Read this before planning anything

**The ledger overstates remaining work, substantially, and this is now the most important fact about
it.** In the last two batches, refutations roughly equalled fixes — **29 refuted / 29 fixed**, then
**15 refuted / 11 fixed** — and independent reviewers confirmed those refutations row by row. Almost
all of them read "already closed at HEAD": earlier sweeps fixed the code and never marked the row.

Consequences you must plan around:

1. **A count of open rows is not a count of remaining work.** Do not report progress as "N of 118".
2. **Every batch currently spends its first third re-verifying finished work.** That is pure waste.
3. **Area headers disagree with their own contents.** Area 04's header said 11 items; the file's own
   sweep-9 recount block said 8, and the recount was right. **Trust the recount block over the
   header, and say which you used.**

### Therefore: do the reconciliation pass first

This is the highest-value next move, and it is cheap because it needs no fixes — only reading.

For each area file, walk every non-struck row in `## Open items`, check it against the code at HEAD,
and mark the ones already closed with the evidence. Do not fix anything; just make the ledger true.
Partition by area, run it wide, and finish with a corrected census.

You will get a real number to work against, and every subsequent batch gets a third cheaper.

---

## The queue after that

**1. Area 07 — `cyrup-tui` (47 rows, plus 8 routed `CFG-*`).** The largest remaining area.

There is a hard house rule here, learned expensively: **a TUI change is not done until it has been
run in a real terminal.** `TestBackend` unit tests pass while the assembled application has layout
and empty-state bugs they cannot see. Plan for a live run, and if you cannot do one, say so plainly
rather than reporting the area closed.

The 8 routed `CFG-*` rows that land in `cyrup-tui`: `CFG-014` (consumer half), `CFG-015`, `CFG-021`,
`CFG-038`, `CFG-063`, `CFG-064`, `CFG-065`, `CFG-066`.

**2. The re-unblocked subagents work.** Two items were blocked on reasoning that turned out to be
wrong, and re-reading them removed most of the cost:

- **`SUBA-016` — scheduled runs, nine `schedule.*` verbs.** Blocked as "needs `workflowScript`,
  which is a Node VM sandbox." **Wrong for the scheduler**: `ScheduledRunManagerDeps.launch` is an
  *injected dependency*, and the manager never compiles, parses or executes a script.
  **~750 of 753 upstream lines are JS-free** — relative/absolute time parsing, interval math,
  `catchUp: 'latest'` skip-ahead, `MAX_TIMER_DELAY_MS` chunking, `overlap: 'skip'`, a 100-entry
  history ring, stale-launch-claim reclaim, and the nine-arm dispatch. It ports onto tokio.
  `AuthorityAction::ScheduleCreate` is already pre-wired in `registration/authority.rs`.
- **`SUBA-026` — the subagents admin picker.** Blocked as "a TUI subsystem, not a verb." The row has
  **three defects**: it cites `tui/selector.ts`, which **exists at no tag** (the real file is
  `slash/selector.ts`); that file is **147 lines** whose own doc says it is *"composed from pi's own
  TUI primitives"*; and `subagents-admin.ts` has exactly one UI primitive, everything else being
  config shaping and persistence that cyrup already has in `discovery/management.rs`. cyrup ships
  `ListSelector` (1,484 lines, three test files) and `fleet_overlay.rs` already hosts a subagents
  component through the same `InteractiveOverlay` seam. **Real size M, not L.**

**3. The ~44 rows blocked with measured sizes.** These are genuine feature work, each already sized
in its row by the agent that blocked it. Schedule them individually with room, not as part of a
wide sweep — a sweep with a dozen items cannot land a 400-line subsystem.

---

## The one open question that is the owner's, not yours

**`workflowScript`'s authoring surface.** pi's workflow script is *source text the model emits inline
at tool-call time* — `sanitizeTarget` literally instructs it to write
`workflowScript: "return runs.run('main', { agent, task })"`. cyrup's guest unit is an authored crate
built with `cargo build --target wasm32-wasip2`, which a model cannot emit inline and which costs a
cold cargo build.

For **confinement**, cyrup's WASM host is strictly stronger than `node:vm` and the substitution is a
free host-idiom translation. For **authoring**, it is user-observable, so it is a real `CYRUP-DELTA`
that needs the owner's decision.

Two things make it cheaper than it looks when it is taken: the outer mechanism is a `Worker` with
message-passing RPC whose entire host surface is `runs.run/all/status`, `state.get/set`,
`prompts.render`, `emit`, `console` — and **cyrup has already ported the `state` half in full**, in
`missions/workflow_state.rs`, whose header records that it was done ahead of time "so that the
workflowScript port is a call-site change rather than a second port of this file."

**Do not decide this yourself, and do not let it block anything else.** It is one item; the nine
`schedule.*` verbs do not depend on it.

---

## MCP is out of scope until parity closes

`docs/gap-analysis/13*.md` and `MCP-PORT-METHODOLOGY.md` are a delivered, verified plan for porting
`pi-mcp-adapter` into a new `crates/cyrup-mcp` — 433 port units, sequenced into 13 phases. It is
owned separately, excluded from every count in this ledger, and **explicitly deferred by the owner
until the parity gaps above are closed.** Do not start it. Do not re-audit the plan; it has been
verified at length already.
