# 02 — State, and what to do next

Rewritten 2026-09-24 after the second re-measure pass, against cyrup code `ea23ca2` (HEAD `8d93b0e`
is docs-only). The earlier text of this file (the "reconcile first" advice, the area-07 queue, the
claim that MCP is out of scope) is in `git log -p -- docs/gap-analysis/handoff/02-state-and-next.md`.
It was written at `e815e08` and no longer describes the project.

---

## Where the project is

| | |
|---|---|
| workspace | **24 crates, 1,044,991 lines of Rust under `crates/`**; 28,706 `.ts:N` upstream citations naming 16,976 distinct upstream locations; 974 `CYRUP-DELTA` markers |
| unit gate | **11,311 passed, 0 failed, 9 skipped**, line coverage 86.3% (per the root `README.md`, measured at `ea23ca2`; not re-run by either 2026-09-24 pass) |
| ported upstreams, at their latest tags | pi **v0.87.1**, pi-subagents **v0.71.0**, pi-intercom **v0.14.0**, pi-mcp-adapter **v2.37.0**, code_puppy_core_plugins **v0.0.62**, pi-acp **v0.0.33**, pi-permission-system **v0.8.0** |
| herdr — **a client, not a port** | `crates/cyrup-herdr` is cyrup's own client of herdr's socket API (<https://github.com/herdrdev/herdr>). It is now measured, as client conformance against herdr **v0.9.1**, in area 16 (`../16-cyrup-herdr.md`). Do not plan "port herdr" work |
| what is unread | **every upstream window up to each latest tag has been read.** The remainder (post-tag commits, pi's unshipped harness/durable design material, a few component diffs and one cyrup call site) is listed with reasons in `../README.md` *Where this analysis is blind → What is still unread* |

### Open ledger rows

Derive the number; do not copy it from here: `python3 docs/gap-analysis/scripts/count_open_items.py`.
The script now reads areas 16 (herdr client; its `client-bug`/`protocol-drift` kinds fold into Port
bug / Version lag) and 17 (all trackers), and no longer counts `DRIFT-056`, a duplicate of `EXT-077`.
Areas 13 and 15 stay outside the count by the standing rule.

**The set above medium is now ten rows: 2 critical, 8 high.** The first pass found three; the second
pass added seven, one of them a reopened closure. The ranked table with reasons is at the top of
`../00-residual-ledger.md` and of `../PARITY-GAPS.md`.

### What the second pass did

- Read every window the first pass left unmeasured: pi `packages/ai` (anthropic, openai-responses,
  codex, types, generator), the agent/session/extension and tools/config/session-svc windows, the TUI
  lead lists, pi `harness/**` and `packages/durable` (new area 17), pi-subagents `v0.57.0..v0.71.0`,
  pi-intercom `v0.10.1..v0.14.0` (a surface sweep), pi-mcp-adapter `v2.32.1..v2.33.0` plus every open
  area-13 row, pi-acp's full `src/`, and herdr (new area 16).
- Every lead in every area file now has a written disposition: promoted to an item, or struck with
  evidence.
- **Reopened:** `ICOM-035`, a regression. `8de7460`'s injection pump waits for idle, so a busy
  session is no longer steered.
- **Closed:** no counted row. Area 13 re-ruled `MCP-137` and a large share of its open units to
  implemented; area 15 closed most of its open rows on landed code (`0aefd08`, `cb290d1`). `CFG-067`
  and `CFG-074` were narrowed.
- **Filed:** see the fourteenth-edition block of `../00-residual-ledger.md` for the full id ranges.
  Next free ids: `SUBA-144`, `MCP-586`, `HERDR-004`.

---

## Recommended next work — highest severity first

Every item below is a static read of both sides. None was reproduced. Reproduce each one before
fixing it (see `../REPRO-LOG.md`), and record the run.

**1. Session data loss — the two criticals. Both are S effort.**

- `SESS-056` (area 03): repair a session file's unterminated last line before appending, as pi
  v0.84.4 does. Today the next entry is glued onto the partial line and lost, and every later entry
  loses its parent chain.
- `SEAM-122` (area 08): session import must not overwrite an existing file. Give the copy a unique
  name and copy with create-new semantics (pi v0.85.0).

**2. Intercom delivery — three highs, one change in area 08's `cyrup-session-svc`.** Take `ICOM-035`
(the pump parks busy-session messages until idle), `ICOM-068` (a message delivered without a turn
never reaches the model's transcript) and `ICOM-062` (a peer message during `/compact` starts a run
under the compaction) together. `ICOM-062` needs `SEAM-125` (`is_idle` true during manual
compaction). Area 08 has not filed the `ICOM-035`/`068` code as its own rows; file or cross-reference
them there when the work starts. Note that the existing tests use a `HostServices` double that never
runs the pump, which is why they passed.

**3. Tree navigation during compaction — `TUI-104` (high) with `SEAM-124` (medium).** One guard in
`navigate_tree` plus the UI refusal. Settle the rating disagreement when fixing the pair. The same
seam carries `SEAM-126`, `SEAM-127`, `SESS-061` and `SESS-062`; batch them if the agent has room.

**4. `TOOL-047` (high, area 04, S).** Report a signal-killed shell command as a failure with exit code
`128 + signo`.

**5. Subagents — three highs in `09b`.** `SUBA-115` (a nested stop/interrupt/timeout cascades into
sibling subtrees; S), `SUBA-110` (strip git routing variables before the background runner and
allowlist-less external CLIs; S), `SUBA-114` (stop pruning child tools to the parent's start-up tool
set; M, `stale-port`). The fail-open `SUBA-111` belongs in the same batch.

**6. Area 13's highs, outside the count but user-facing.** `MCP-585` (an Agent Plugin header's
`!command` runs through `/bin/sh`: an HTTP-only plugin can run a local command), `MCP-553`
(`inheritEnv:false` ignored) and `MCP-576` (a rotating bearer token is never re-resolved) first, then
critical `MCP-500` with `MCP-501`, as `13-cyrup-mcp-STATUS.md` already sequences them.

**7. The medium tier.** Start with the fail-open `EXT-077` (`DRIFT-056` closes with it), `HERDR-001`
(project panes may open beside the human's focused pane in another workspace), and area 01's
provider mediums (`PROV-083` first: pi v0.86.0's transcript-based system prompt and tool changes).

**Directory work that blocks nothing but should not slip:** check pi `7fd564cbb` (pi.dev chooses a
catalog version by user agent) against the live server; open the summarization-auth call site; and
re-run the area-13 census by script rather than trusting its hand arithmetic.

---

## The one open question that is the owner's, not yours

*Carried from the `e815e08` edition and not re-verified in 2026-09-24's passes. The mention of the
nine `schedule.*` verbs predates their landing; the second pass found cyrup does have scheduled runs
(see `../09a-cyrup-ext-subagents-v0.57-drift.md`). The authoring question itself is still open.*

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

