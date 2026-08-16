# 02 — State, and what to do next

Written at HEAD `e815e08`, branch `david/cyrup`.

---

## Where the project is

| | |
|---|---|
| unit gate | **7,112 tests, 7,112 passed, 8 skipped, ~18s** |
| `cargo check --workspace --all-targets` | clean |
| `cargo clippy --workspace --all-targets` | clean; 79 warnings, all pre-existing |
| `await_holding_lock` | **0** (was 5) |
| integration suite | **not run recently — see `03-verification.md` before trusting its numbers** |

### Open ledger rows

**118 open — 2 high, 43 medium, 73 low**, across areas 01–12. Area 02 (`cyrup-agent`) is fully
closed at every severity. MCP (`13*`) is excluded from every count and is not started.

**Treat 118 as an upper bound, not a work estimate.** See "read this before planning" below.

### What landed most recently

```
e815e08  batch C — ext, session-svc, intercom, permission, and the locks
831321b  batch B — config, resources, tools, session, drift
68bbd39  batch A — subagents and provider
646e739  docs: state the upstream rule instead of recommending against the project
4dfdd03  fix(plumbing): close the six seams that needed more than one crate
37c2833  fix(plumbing): the seams that compiled, returned a plausible value, and lied
320a1a2  docs: re-true the user-facing docs against HEAD
```

The two `plumbing` commits are worth reading before you start. They closed a whole defect
class — **seams that compile, return a plausible value, and are wrong, because nothing ever called
them.** Examples: the entire *read* half of the session interface (`entries`/`branch`/`tree`)
returned `[]`/`[]`/`null` forever while the *write* half worked; `oauth_prompt`/`oauth_select`
denied against a capability nothing grants, with test doubles as their only implementors; on Windows
two of three termination signals were no-ops, so the ladder waited out both grace periods for
signals it never sent and then orphaned the child's whole descendant subtree.

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
