# cyrup gap analysis

A ledger of the behavioural differences between the **cyrup** Rust port and its upstreams, kept as a
work backlog. Every entry is work: there is no "accepted divergence" category and no "undecided"
status. Mechanism may differ where the language forces it — port the behaviour, state the mechanism
difference and its reason, and if the mechanism difference costs behaviour, it stays on the list.

**Seven upstreams are ported**: the six TypeScript ones — `pi`, `pi-subagents`,
`pi-permission-system`, `pi-intercom`, `pi-mcp-adapter` (as `crates/cyrup-mcp`, area 13) and `pi-acp`
(as `crates/cyrup-acp`, area 15) — plus **`code_puppy_core_plugins`, which is Python**, ported as
`crates/cyrup-flux`. Areas 13 and 15 began as port plans for code that did not exist; both crates now
exist and those areas track what is built. **herdr is not ported.** `cyrup-herdr` is cyrup's own
client of herdr's socket API (<https://github.com/herdrdev/herdr>); area 16 measures whether that
client conforms to the herdr release it targets (see *Baselines measured against*).
The hard rule `git -C <repo> show <tag>:<path>` applies to all of them; only the language differs.

**Where the work is:**

- [`PARITY-GAPS.md`](PARITY-GAPS.md) — every item grouped by gap class (port bug · unwired · version
  lag · reverse lag · deletion candidate · open question). **§0a is every item above medium.**
- [`00-residual-ledger.md`](00-residual-ledger.md) — ranked and cross-cutting. **Start here to pick
  the next item.**
- The numbered area files — the evidence, and the authoritative status of every item.

> **This file carries no counts and no record of past passes.** The open set, its severities and its
> closure record live in the area files' `## Open items` tables; a count copied here is a second
> ledger that goes stale the moment work lands, and it did — this file published a critical that had
> already been closed. Pass narration is worse than useless: it is a scoreboard that grows without
> bound while telling a planner nothing about what is left. Recover any of it from
> `git log -p -- docs/gap-analysis/README.md`.
>
> What is kept here is what does not rot — navigation, the item format, the baselines, the rules for
> working an item, the places this analysis is known to be blind, and the work this directory itself
> still owes.

## Contents

**Each area file's own `## Open items` table is the authority for what is open in that area, and its
severity cells are the authority for what outranks what.** This index says only what each file
covers.

| file | area |
|---|---|
| [`../PARITY-PLAN.md`](../PARITY-PLAN.md) | **the execution plan derived from this directory — batches, next moves, deferrals and open questions** |
| [`../adr/README.md`](../adr/README.md) | **decisions of record — where `PARITY-PLAN.md` §7's open questions were settled, plus the ledger changes those decisions imply** |
| [`PARITY-GAPS.md`](PARITY-GAPS.md) | **the same items grouped by gap class — read first.** §0a is every item above medium; §0's census and the pre-enumeration class sections are historical, while the class taxonomy, the per-entry fix sketches and §7 Method are current |
| [`REPRO-LOG.md`](REPRO-LOG.md) | **what happened when the binary was actually run** — items driven through a real pty or headless, each row carrying a transcript. **Read this before trusting a severity**: most items that survive a static read do not survive a live one unchanged |
| [`00-residual-ledger.md`](00-residual-ledger.md) | ranked cross-cutting view — **start here to pick the next work item** |
| [`01-cyrup-core-and-provider.md`](01-cyrup-core-and-provider.md) | wire APIs, providers, auth, streaming, catalogs, cost |
| [`02-cyrup-agent.md`](02-cyrup-agent.md) | the turn loop, tool dispatch, hooks, abort |
| [`03-cyrup-session.md`](03-cyrup-session.md) | JSONL session tree, compaction, system prompt |
| [`04-cyrup-tools.md`](04-cyrup-tools.md) | the built-in tool set |
| [`05-cyrup-config-and-resources.md`](05-cyrup-config-and-resources.md) | settings, model resolution, trust, skills, packages |
| [`06-cyrup-ext.md`](06-cyrup-ext.md) | extension host, WIT world, event catalog |
| [`07-cyrup-tui.md`](07-cyrup-tui.md) | terminal UI application layer |
| [`08-cyrup-session-svc-and-modes.md`](08-cyrup-session-svc-and-modes.md) | the integration seam, RPC, CLI, print/json modes |
| [`09-cyrup-ext-subagents.md`](09-cyrup-ext-subagents.md) | subagent delegation |
| [`09a-cyrup-ext-subagents-v0.57-drift.md`](09a-cyrup-ext-subagents-v0.57-drift.md) | subagent drift against a later `pi-subagents` tag — a supplement to area 09, with its own item ids and its own severities. **Not covered by area 09's table**; read both |
| [`09b-cyrup-ext-subagents-v0.64-drift.md`](09b-cyrup-ext-subagents-v0.64-drift.md) | subagent drift over `pi-subagents` `v0.57.0..v0.71.0`, the window neither 09 nor 09a covers. Its own ids and its own `## Open items` table; read all three |
| [`10-cyrup-permission-system.md`](10-cyrup-permission-system.md) | allow / ask / deny gate |
| [`11-cyrup-intercom.md`](11-cyrup-intercom.md) | supervisor↔subagent broker |
| [`12-upstream-drift-pi-core.md`](12-upstream-drift-pi-core.md) | pi core drift since the ported baseline |
| [`14-cyrup-flux.md`](14-cyrup-flux.md) | the Flux pipeline — the fifth ported upstream, and the first that is neither pi nor TypeScript |
| [`16-cyrup-herdr.md`](16-cyrup-herdr.md) | **client conformance, not a port** — `crates/cyrup-herdr`, cyrup's own client of herdr's socket API and CLI, checked against herdr v0.9.1. Its kinds (`client-bug`, `protocol-drift`) describe the client, not parity; the counting script folds them into Port bug / Version lag and says so |
| [`17-pi-harness-and-durable.md`](17-pi-harness-and-durable.md) | pi's experimental `packages/agent/src/harness/**` (incl. `pico3`) and the new `packages/durable`, `v0.85.1..v0.87.1`. pi ships none of it to users, so both rows are trackers; it names the existing item that covers each shipped overlap |
| [`15-cyrup-acp.md`](15-cyrup-acp.md) | **the `cyrup-acp` port plan** — the Agent Client Protocol adapter, following `svkozak/pi-acp`. Like area 13 it began as a spec for code that did not exist yet and tracks port units rather than defects, so it is **counted separately from the twelve** for the same structural reason; its own tables are the authority for its unit inventory |

## Reading the area tables

How the area files are written, so that a reader knows which table to believe.

- **Each file carries exactly one `## Open items` table, and that table is the current one. Do not
  re-split one** — findings put in a second table are invisible to enumeration of the first.
- **Where a file ALSO carries a `## Status of every item from prior analyses` table, only the
  `## Open items` table is kept current.** Rows have read "still open" in a status table that the
  same file's open-items table had already closed. Read both before quoting either, believe the
  latter.
- **`tracker` rows and `*(partially-closed)*` rows are not work** and sit outside any tally — see
  *Item format*. A figure mixing work with bookkeeping cannot be planned against.
- **Whatever is open is a floor, never a total**, and it contains known duplication in the other
  direction — see *Where this analysis is blind* and *Work this directory owns*.
- **If you need a number, derive it with a committed script**, not by hand and not from prose.

Numbering follows the convention already referenced in cyrup's source
(`spec/gap-analysis/03-cyrup-agent.md`, `12-cyrup-tui.md`, `00-residual-ledger.md`). That `spec/`
tree is not in this workspace, so exact alignment with it is unverified.

## Area 13 — the MCP adapter port

A **fifth upstream**, `pi-mcp-adapter`, planned at v2.25.0 (~24k lines of TypeScript, 203 paths)
before any of it was ported. `crates/cyrup-mcp` now exists and area 13 is re-measured at v2.37.0.
**Area 13 is scoped and counted separately from the twelve areas, and the separation is
load-bearing**: the twelve measure drift in code that exists, while area 13 tracks port units (implemented /
partial / missing) against a plan, so a forward-looking port unit and a backward-looking defect cannot be added together
into anything a planner can act on. **The exclusion runs both ways: nothing in `README.md`,
`00-residual-ledger.md` or `PARITY-GAPS.md` speaks for `13-cyrup-mcp.md`, `13a`–`13i` or
`MCP-PORT-METHODOLOGY.md` — and those files' own tables are the authority for their unit inventory
and its status.** *Correction 2026-09-04: earlier editions said another team owned area 13. That was
never true — every upstream port, the MCP adapter included, is this repository's own work and is
schedulable from this directory. The count separation above stands on the structural reason only.*

| file | area |
|---|---|
| [`13-cyrup-mcp.md`](13-cyrup-mcp.md) | **the port — thesis, scope, seam map, architecture, and the one canonical table of every unit. Start here.** |
| [`13-cyrup-mcp-STATUS.md`](13-cyrup-mcp-STATUS.md) | **what is actually BUILT** — per-unit implementation status against the plan, audited at a named `pi-mcp-adapter` tag |
| [`MCP-PORT-METHODOLOGY.md`](MCP-PORT-METHODOLOGY.md) | **how it is executed and verified** — fidelity rules, thirteen phases, the ADR docket |
| [`13a-mcp-activation.md`](13a-mcp-activation.md) | activation, lifecycle and the host seam |
| [`13b-mcp-config.md`](13b-mcp-config.md) | configuration, the type model and errors |
| [`13c-mcp-servers.md`](13c-mcp-servers.md) | server manager, transports and the metadata cache |
| [`13d-mcp-proxy-modes.md`](13d-mcp-proxy-modes.md) | proxy modes and search ranking |
| [`13e-mcp-tools.md`](13e-mcp-tools.md) | tool registration, approval, output guard and rendering |
| [`13f-mcp-credentials.md`](13f-mcp-credentials.md) | credential storage, keychain and consent |
| [`13g-mcp-oauth.md`](13g-mcp-oauth.md) | the OAuth 2.1 flow and the callback server |
| [`13h-mcp-tui.md`](13h-mcp-tui.md) | the TUI panels, slash commands and prompts |
| [`13i-mcp-protocol-and-verification.md`](13i-mcp-protocol-and-verification.md) | sampling, elicitation, tracing and verification |

**The port is an extension and changes nothing in cyrup's core** — `crates/cyrup-mcp` is a native
built-in crate, the same shape as `cyrup-ext-subagents`, linking `rmcp` 3.1.2 (client-only) directly.
**Four surfaces are cut by owner decision** — the legacy HTTP+SSE transport, MCP Apps, the raw
unix-socket transport, and `mcpScript`/the JavaScript worker — which is why there is **no section 09**
(it would have held MCP Apps) and why the port contains no hand-written protocol code and no
JavaScript engine question.

**Area 13 cites cyrup by symbol and file only — no line numbers, no commit shas — and the rest of
this directory is why.** Line citations written during an analysis are stale before it ships, because
the repository advances underneath it. Caveat inherited from the whole directory: it is a static
analysis, nothing was built or run, and every `verify` line is a design rather than an observation.

**Area 13 also produced a finding about cyrup rather than about the port.**
`ExtensionHost::refresh_tools` returns the *guest* materializer's verdict, and under the default
`wasm-host` feature that materializer reads a different map than a natively late-registered tool is
written into — so the tool never reaches the running agent, and `take_tools_dirty`'s `swap` destroys
the signal rather than deferring it. Dormant only because `register_late_tool` has zero callers
anywhere in the workspace. Filed as `MCP-037a`.

A **seventh upstream**, `svkozak/pi-acp` v0.0.33 (4 238 lines of TypeScript across 17 `src/` files),
planned before any of it was ported. `crates/cyrup-acp` now exists. `cyrup-acp` is the Agent Client Protocol adapter that lets an
editor — Zed is the reference client — drive cyrup over JSON-RPC on stdio. **Area 15 is scoped and
counted separately from the twelve for the same structural reason as area 13**: it began as a spec
for code that did not exist yet, and its port units cannot be added to a backward-looking defect count. The
exclusion runs both ways, and [`15-cyrup-acp.md`](15-cyrup-acp.md)'s own tables are the authority
for its unit inventory and status.

**Area 15's defining decision is an inversion of its upstream, and it is worth knowing before
reading any unit.** pi-acp is an out-of-process adapter by necessity — a separate npm package that
spawns `pi --mode rpc` and rebuilds the agent's state from untyped NDJSON. `cyrup-acp` is a
workspace crate and binds to `AgentSession` in-process, so the entire subprocess surface —
`PiRpcProcess`, its ENOENT/EACCES diagnostics, the ANSI prelude scraping, and the defensive
key-probing in `translate/bash.ts` and `translate/pi-tools.ts` — has **no counterpart to port**, and
`AgentSessionEvent` supplies typed variants (`QueueUpdate`, `BashExecutionUpdate`,
`SessionInfoChanged`, `EntryAppended`) that upstream had to infer. Much of the area is therefore
recorded as *already present* or *cut* rather than as work; that record is the point, not an
omission.

## Item format

Every item is a `##` section with a stable id (`AREA-NNN`):

```
**Kind** parity-bug · **Severity** critical · **Effort** S · **Confidence** confirmed
**cyrup**    — cyrup/crates/…:LINE — what the code actually does
**upstream** — pi/packages/…:LINE — what upstream does
**Impact**   — the user-visible consequence
**Fix**      — concrete sketch naming files and functions
**Verify**   — how to prove it is fixed
```

**Kind** — `parity-bug` (ported but drifted) · `not-ported` (predates the baseline, never built) ·
`upstream-drift` (landed after the baseline; expected lag) · `stale-port` (cyrup carries behaviour
upstream changed or deleted) · `cyrup-original` (no upstream basis) · `test-defect` (a test pinning
wrong behaviour, or asserting a timing outcome it cannot control) · plus a small tail of `tooling` /
`port-divergence`. `PARITY-GAPS.md` §0 maps every kind onto its gap class.

**Severity** is judged by user-visible consequence, not code size: `critical` = data loss, silent
wrong output, a permission bypass, or a crash on a normal path. **The definition carries no
reachability qualifier** — an unreachable defect is still rated on what it does when reached, and the
blast radius is recorded inside the item as scheduling information rather than as a rating. Severity
is never held down by an unverifiable justification: an in-source ADR or `R-NN-NNN` id that cannot be
read from this workspace is not a decision of record.

**Effort** — `S` under a day · `M` a few days · `L` a week+ or needs design.

**`tracker`** is not a severity and not a kind — it is a row proposing **no schedulable work**, either
because it indexes items other files own or because it asks a scope question. A tracker keeps its id,
its status row and its body, and stays outside any tally, because a figure mixing work with
bookkeeping cannot be planned against. Each tracker records what would escalate it into the work set.
A tracker further marked **lead** has had neither side re-read and is held to a lower evidence
standard than an item.

**IDs are never renumbered or deleted.** Closed items keep theirs so a closure can be re-audited, and
an item changing class keeps its number and moves section. A gap in a number range is not evidence of
a deletion — `SEAM-035`…`SEAM-046` never existed, and area 08 records the check that establishes it.

## Baselines measured against

Re-measured **2026-09-24**, every figure below from `git diff --shortstat` / `git rev-list --no-merges --count`
run in `tmp/<repo>`. Earlier values are in `git log -p -- docs/gap-analysis/README.md`.

| repo | HEAD | cyrup ported baseline | latest tag | delta |
|---|---|---|---|---|
| `cyrup/` | **`ea23ca2`** (2026-09-23), the last code commit every area file was re-read against on 2026-09-24. *Superseded: `9aeba769`, `824a539e`, `6cf2cb9f`.* `9aeba769..ea23ca2` under `crates/` is **18 non-merge commits, 485 files, +164 754 / −4 932**: the subagents SCOPE/lanes/runner-identity work, the 18-command slash surface, the UW-7 fleet roster, and the new `cyrup-herdr` crate. **Re-measure before trusting any status: the port moves faster than this directory does** | — | — | **24 crates, 1 044 991 lines of Rust under `crates/`** (`find crates -name '*.rs' -type f \| xargs cat \| wc -l`). **28 706** `.ts:N` upstream citations in `.rs` files, naming **16 976** distinct upstream locations; **974** `CYRUP-DELTA` markers in `.rs` files. Gates were not re-run by this pass |
| `pi/` | `b45597504` = `v0.87.1-16-gb45597504` | **v0.83.0** | **v0.87.1** (`f07218c4d`, 2026-09-22) *(was v0.85.1)* | `v0.83.0..v0.87.1` = **1 392 files, +217 038 / −40 736**, 1 248 non-merge commits. **New window `v0.85.1..v0.87.1` = 667 files, +79 789 / −22 639, 201 non-merge commits**: `packages/agent` 125 files +36 468 / −1 316 (mostly `harness/**`, of which `harness/pico3/**` is ~9k new lines), `packages/coding-agent` 242 files +18 411 / −15 390, `packages/durable` (new package) 27 files +8 533, `packages/ai` 127 files +4 910 / −2 058, `packages/chord` 26 files +4 228 / −529, `packages/tui` 48 files +2 665 / −397. Breaking changes: v0.86.0 (`TranscriptContext`, `user_bash` fails closed) and v0.87.0 (`finishTurn`, `context_edit`, actionable `turn_end`). The previous window measures `v0.84.1..v0.85.1` = 883 files, +111 832 / −27 415, **708** non-merge commits, and `v0.84.4..v0.85.1` = 708 files, 426 non-merge commits (the 428 recorded before was off by two). `v0.87.1..HEAD` = 240 files, 15 commits, untagged: read by the 2026-09-24 second pass and recorded in each area only as post-tag leads (the hard rule forbids filing against an untagged commit) |
| `pi-subagents/` | `6f1027f7` = `v0.71.0-10-g6f1027f7` | **≈v0.68.0 by citation census** (v0.68.0 × 840, v0.43.0 × 668, v0.64.0 × 377, nothing at v0.69 or later). The crate records no version string; the old ≈v0.43.0 figure is where the port started, not where it stands | **v0.71.0** (`4af5e85a`) *(was v0.67.0)* | `v0.43.0..v0.71.0` = **690 files, +138 768 / −35 061**, 878 non-merge commits. **New window `v0.67.0..v0.71.0` = 386 files, +25 038 / −13 948, 158 non-merge commits**, of which `src/` is 147 files, +7 377 / −4 886. `v0.71.0..HEAD` = 10 commits, read on 2026-09-24; two apply to cyrup (`965dd4b8`, `5794f52d`) and are recorded in `09b` as post-tag leads |
| `pi-permission-system/` | `9affcc9` = `v0.8.0`, **re-checked 2026-09-24, unchanged** | **v0.7.1** | **v0.8.0**, still the newest tag; `v0.8.0..HEAD` is empty | `v0.7.1..v0.8.0` = 28 files, +4 023 / −1 851, 9 non-merge commits. Absorbed into the code: zero drift rows |
| `pi-intercom/` | `6c15527` = `v0.14.0` | **v0.9.2** (*not v0.7.0; older docs had this wrong*) | **v0.14.0** *(was v0.13.0)* | `v0.9.2..v0.14.0` = 32 files, +9 176 / −3 677, 36 non-merge commits. **New window `v0.13.0..v0.14.0` = 20 files, +4 451 / −2 677, 8 non-merge commits**, mostly `package-lock.json` churn |
| `code_puppy_core_plugins/` | `6d4e26a` = `v0.0.62-1-g6d4e26a` | **v0.0.6**, *not recorded anywhere in `crates/cyrup-flux`; see `FLUX-007`* | **v0.0.62** (`720a885`) *(was v0.0.50)* | `v0.0.50..v0.0.62` = 65 files, +2 499 / −452, 33 non-merge commits across the whole repo, **none of it on the ported surface**: `git diff --quiet v0.0.6 <tag> -- code_puppy_core_plugins/flux_bootstrap/ tests/test_flux_bootstrap.py` holds at every tag from v0.0.50 to v0.0.62 (re-checked 2026-09-24). The ported surface is `flux_bootstrap/`: 18 bundled commands, 4 `_docs` files, 3 renderer scripts. cyrup ships 15 templates + 3 native renderers = the same 18. The second upstream, `code_puppy` itself, is at v0.0.867 and unpinned past v0.0.720 |
| `pi-acp/` | `42926cc` = `v0.0.33-8-g42926cc` | **v0.0.33**, ported as `crates/cyrup-acp` (area 15) | **v0.0.33**, still the newest tag | 17 `.ts` under `src/` = 4 238 lines. `v0.0.33..HEAD` = 15 files, +859 / −52, 6 non-merge commits, of which `src/` is 3 files, +155 / −43 (context-window usage, stats timeout, Windows session path). Untagged, so a lead only |
| `herdr/` — **a client, not a port** (<https://github.com/herdrdev/herdr>) | `9c96f7dd` = `preview-2026-09-21-0ff0f27e2226-21-g9c96f7dd`, cloned 2026-09-24 | **none: nothing is ported.** `crates/cyrup-herdr` is cyrup's client of herdr's socket API (area 16). The crate pins itself to "herdr v0.9.1 (`d59d060`)", but `d59d060` is an untagged `main` commit 25 commits past the v0.9.1 branch point, and `v0.9.1` is not its ancestor (`HERDR-002`) | **v0.9.1** (`065ef9d6`, 2026-09-16), the newest release tag. Newest tag of any kind: `preview-2026-09-21-0ff0f27e2226` | `v0.9.1..HEAD` = 307 files, +32 344 / −3 248, 55 non-merge commits. The client was checked against v0.9.1's API surface (104 methods; the 28 cyrup calls and all 29 event kinds agree). Post-v0.9.1 API changes the client already handles, and four post-tag leads, are recorded in area 16 |
| `pi-mcp-adapter/` | `86f3e20` = `v2.37.0-2-g86f3e20` | **v2.37.0**, ported as `crates/cyrup-mcp` (area 13). Its counted unit census at this tag lives in `13-cyrup-mcp-STATUS.md` | **v2.37.0** (`28049de`) *(was v2.33.0)* | `v2.26.1..v2.37.0` = 260 files, +38 861 / −4 364, 158 non-merge commits. **New window `v2.33.0..v2.37.0` = 173 files, +15 550 / −4 169, 57 non-merge commits.** Production TypeScript is 81 files / 31 379 lines by area 13's filter (73 / 28 109 at v2.33.0; the old 72/84 figures used a different filter and do not compare). `v2.37.0..HEAD` = 7 files, 2 commits, untagged |

**`cyrup-herdr` is a client of herdr, not a port of it.** herdr is cloned at `tmp/herdr` from
<https://github.com/herdrdev/herdr.git>, and area 16 checks the client against the v0.9.1 release
tag. Do not file "herdr feature not ported" items: a herdr method cyrup has no reason to call is not
a gap. What is a gap is the client disagreeing with herdr (`client-bug`) or not following a herdr API
change (`protocol-drift`).

**Read upstream with `git -C <repo> show <tag>:<path>`, never from a working tree.** Clone-HEAD line
numbers and file existence both mislead; items have named files that never existed at any tag.

### Which area file is pinned where, and what each is blind to

The table above is the state of the **upstreams**. This one answers the question a planner actually
asks: *how stale is the file I am about to open?* Every area file carries its own pin block; this is
those blocks, collected on 2026-09-24. Older pins stay in each file's header and in git history.

**Read it as staleness, not as work.** A blind window is a range in which this directory has no
opinion; it is not a backlog and it is not a count. **Where an area file states its own per-module
range, that range wins over the single tag in this column.** Nothing in this table re-verifies any
file; it only says how old each one is.

| area file(s) | pinned at (2026-09-24) | still unread after the second pass | 2026-09-24 second pass |
|---|---|---|---|
| `01` · `12` | cyrup `ea23ca2` × pi **v0.87.1** (ported v0.83.0) | nothing in `packages/ai`; every census lead has a disposition. `v0.87.1..HEAD` is post-tag leads (`7fd564cbb`, catalog version chosen by user agent, first) | `PROV-083`…`100`, `DRIFT-058`/`059` filed; `DRIFT-056` marked duplicate of `EXT-077` |
| `02` · `03` · `06` | cyrup `ea23ca2` × pi **v0.87.1** | `harness/**` (area 17; `AGENT-028` covers scope); `agent-session.ts` `v0.84.1..v0.85.1` hunks beyond abort/compaction/model (area 08's); chord `delta`/`facets` and `experimental/**` (tracker `EXT-088`) | `AGENT-042`…`044`, `SESS-056`…`063`, `EXT-081`…`088` filed; every lead dispositioned |
| `04` · `05` · `08` | cyrup `ea23ca2` × pi **v0.87.1** | cyrup's summarization-auth call site (pi v0.86.0 `getAuth` cancellation); `experimental/micro` `runtime.ts`/`tui.ts` (nothing in pi launches it) | `TOOL-051`, `CFG-086`…`093`, `SEAM-123`…`133` filed (`SEAM-132` tracker); `CFG-067` and `CFG-074` narrowed |
| `07` | cyrup `ea23ca2` × pi **v0.87.1**. The crate's module docs still cite a per-module range of v0.84.1–v0.84.3; believe that over this row | component diffs outside the assigned leads (model, scoped-models, thinking, trust, settings, extension selectors, `session-share`); `packages/tui/test/**` | `TUI-104`…`122` filed (`TUI-118` tracker); every lead dispositioned |
| `09` · `09a` | cyrup `ea23ca2` × pi-subagents **v0.71.0** | the windows they declare in `## Scope`; everything later is `09b`'s. Every census and residual lead now has a disposition | dispositions only; nothing filed |
| `09b` | cyrup `ea23ca2` × pi-subagents **v0.57.0..v0.71.0** | the `v0.57.0..v0.67.0` `src/` diff line by line (207 files; its leads are resolved); Herdr-placement hunks (`SUBA-100`); Node launch plumbing and async-workflow-only surfaces (no cyrup counterpart / refused) | `SUBA-114`…`143` filed; next id `SUBA-144` |
| `10` | cyrup `ea23ca2` × pi-permission-system **v0.8.0** = the latest tag | none | not re-measured in the second pass |
| `11` | cyrup `ea23ca2` × pi-intercom **v0.14.0** | none; `v0.14.0..HEAD` is empty | `ICOM-035` reopened (regression); `ICOM-068`…`070` filed; `ICOM-062` raised to high |
| `13` · `13a`–`13i` · `13-cyrup-mcp-STATUS.md` | cyrup `ea23ca2` × pi-mcp-adapter **v2.37.0** | the non-theme hunks of the two panel files in `977577f` (sampled); the `implemented` rows as a regression set; `MCP-522`/`525`/`531` never had their own TypeScript read | `MCP-551`…`585` filed; every open unit re-ruled against v2.37.0 (closures recorded in STATUS); **numbering resumes at `MCP-586`** |
| `14` | code_puppy_core_plugins **v0.0.6** ported / **v0.0.62** measured | none (the ported surface is byte-identical). `code_puppy` itself past v0.0.720 is `FLUX-007` | not re-measured in the second pass |
| `15` | cyrup `ea23ca2` × pi-acp **v0.0.33** = the latest tag | `@agentclientprotocol/sdk` 0.26 is not in `tmp/pi-acp` (bears on `ACP-014`) | every open row re-read, most closed on landed code (recorded in its `## 6. Open items`); `ACP-297`/`298` filed |
| `16` (**new**) | cyrup `ea23ca2` × herdr **v0.9.1** (`065ef9d6`) — **a client, not a port** | `relay.rs` and the ssh-runner half of `remote.rs` (cyrup's own relay protocol; their herdr surface was read); `cli.rs` beyond its verbs; herdr's non-API changes after v0.9.1 | first measurement; `HERDR-001`…`003` filed |
| `17` (**new**) | cyrup `ea23ca2` × pi **v0.85.1..v0.87.1**, `harness/**` and `packages/durable` | `pico-v5.md` §2–§11, the durable tests and harness conformance suite, `packages/agent/docs/**`, the v0.85.1 bodies of 27 `harness/runtime` files. None reaches a pi user; read the first and last if a `HARN-*` escalation fires | trackers `HARN-001`/`002` |

**What this table shows that no single file could.** (1) **Every pi area is pinned at v0.87.1, and
after the second pass every upstream window up to each latest tag has been read.** What is left in
the third column is either off pi's shipped path, a cyrup-side call site, or test and design
material; *Where this analysis is blind* lists it with reasons. (2) **pi-subagents `v0.57.0..v0.71.0` now has an owner** (`09b`), which ends
the problem of leads filed into `09a` under a scope that excluded them. (3) **Everything now has an
owner.** `packages/durable` and `harness/**` are area 17's, `cyrup-herdr` is area 16's, and
`packages/chord` is held by area 06's watch-only tracker `EXT-088` (checked by importer only: nothing
cyrup ports depends on it).

> **LINE-CITATION SHIFT, stated so nobody re-verifies against the wrong line.** Inserting this
> section moved **43 lines** into this file immediately above *Three standing hazards*. Every
> `README.md:<line>` citation elsewhere in this directory that named a line **at or after old line
> 203** is now short by 43 — `:208-212` → `:251-255`, `:224-225` → `:267-268`, `:509-512` →
> `:552-555`. Citations above 203 (`:3-4`, `:68`, `:106-107`, `:133`, `:196-199`) are unaffected.
> Those citations live in files this pass does not own and were **not** edited; this note is the
> repair instruction, and it is one more instance of the dangling-citation problem *Work this
> directory owns* already names. **The 2026-09-24 rewrite of this section moved lines again; cite
> this file by heading, not by line.**

Three standing hazards:

- **A recorded baseline is an unverified claim, and a wrong one silently reclassifies work.**
  `pi-intercom` was recorded as ported-from-v0.7.0 for months; it is v0.9.2, and the error parked
  in-baseline **port bugs** in "version lag", where they were out of scope until the next bump. The
  same error in the other direction hid an entire `pi-subagents` release range because "latest" was
  never re-checked. **Census the baseline, do not inherit it** — count in-tree `vX.Y.Z` citations per
  crate and compare against the recorded number before trusting any `not-ported` vs `upstream-drift`
  call — and **re-measure "latest" every pass**: `git -C <repo> describe --tags` and
  `git diff --stat <baseline>..<latest>` are the first commands of an audit, not the last.
- **A classification turns on which side of the *ported* tag a symbol landed, and a commit hash does
  not answer that.** Settle presence with `git cat-file -e <tag>:<path>` before writing
  `upstream-drift`. Date reasoning has misfiled items in both directions.
- **pi HEAD is past its latest tag**, so that range is unanalysed by construction — items in it are
  deliberately not filed, because the hard rules require citing a named tag.

## Working an item

**Evidence means both sides, personally read: the Rust at HEAD and the TypeScript at the named tag.**
Anything less is a lead. Default to rejection when you cannot re-read both.

- **A commit message asserting a fix is a hypothesis, not evidence.** So is a task file, and so is a
  deleted task file — one commit subject claimed to land a change whose code was never written.
- **Refute closures rather than confirming them.** A wrongly-closed item deletes a real defect from
  the backlog and nobody looks again. Closing code is itself unaudited code: audit what a closure
  actually does, not that it exists.
- **A closure resting on an argument rather than an observation must write down its own falsification
  condition**, so it can be reopened by measurement instead of by luck.
- **Every cross-cutting entry must name an owning area item or be explicitly marked ownerless.** An
  unowned entry with a citation reads as verified and is more dangerous than no entry at all —
  several have gone unclaimed through consecutive passes.
- **Publish what you excluded, with a reason per entry, and record negative results.** "Read, nothing
  found" is worth as much as an item; a silent exclusion is invisible to every later pass, because
  each one inherits the last one's scope.
- **Regenerate the cross-cutting files last**, and have someone read the whole set afterwards. A
  per-area pass structurally cannot see three cross-cutting files each declaring a different one of
  them current.

**Known traps — do not re-report these as discoveries.** `loop_fn.rs` is a facade over
`RunCtx::run_loop`; pi carries two forked compaction implementations; the provider `fleet!` macro
hides ~20 registrations; `wasm-host` is default-**ON**, an opt-out; and the out-of-scope pi package
list is contested rather than settled. **A wrong trap is worse than no trap** — "the deliberately
unreachable first-run wizard" was fed to every pass and was simply false, which converted a real
finding into a non-finding across all of them. Verify a trap before feeding it forward, and re-open
one when the code beneath it moves.

## Where this analysis is blind

Properties of the method, not of any one pass. They will keep producing misses until the method
changes.

1. **An item-driven analysis cannot see behaviour nobody wrote an item for.** Every pass starts from
   a list and asks "is this item real?"; a pi function with no item is invisible even to the
   adversarial reader, because there is no claim to refute. The counter is the **surface-driven
   sweep** — walk upstream itself and, for each exported symbol / event / config key / CLI flag / env
   var, ask what in cyrup consumes it. **Treat whatever is open as a floor, never a total.**
2. **A static read recovers what the code does, not what the user sees — and the picture of the
   screen is what a fix gets written against.** Items have been filed as an absent surface that is
   really an affirmative wrong one, or against a spinner that never renders. **Treat an unobserved
   item's mechanism as a hypothesis even when its existence is well evidenced.**
3. **The substrate carve-out generalises too far.** "cyrup delegates rendering to ratatui + crossterm,
   so pi's hand-rolled `render(width): string[]` framework is out of scope" is correct for the
   *drawing* layer only. It has been extended to everything in `packages/tui/src/tui.ts`, including
   behaviour that draws nothing — input sanitation, terminal-reply handling, mode negotiation, paste
   and focus semantics — all of which is portable and in scope. **Before invoking it on a `tui.ts`
   line, check whether that line draws anything.** Corollary, past the TUI: **not enabling a feature
   does not make its hazards moot** — ask what the code *sends*, not only what it *enables*.
4. **"Has a consumer" is too weak a test for the unwired class.** A `/settings` display row is not a
   consumer. The same shape recurs as advertised-but-unimplemented, implemented-but-unadvertised, and
   delivered-but-never-rendered. The durable countermeasure is a test, not a sweep — see *Work this
   directory owns*.
5. **A dismissal is never re-examined.** An exclusion is written once, with a plausible reason, and
   from then on no pass looks at it. That is how a 2 733-line generator present at both tags and
   exposed as an npm script came to be declared non-existent in three files, one of which built a
   whole Fix on the false premise. **"No runtime effect" licenses skipping a directory's *behaviour*,
   never its *provenance*** — a gitignored path is evidence that a generator exists, not that the
   artifact is absent.
6. **The axis, not the diligence, is the variable.** Where a pass changes what it walks rather than
   how hard it looks, the yield changes with it. Prefer a new surface over a re-read.

### What is still unread (2026-09-24, second pass, cyrup `ea23ca2`)

**Every upstream window up to each latest tag has been read.** What follows is everything left, each
with the reason it was left. Nothing else is known to be unread; the method's limits above still
apply to what was read.

| unread | why | owner |
|---|---|---|
| every upstream's commits after its latest tag (pi `v0.87.1..b45597504`, pi-subagents `v0.71.0..6f1027f7`, pi-mcp-adapter `v2.37.0..86f3e20`, pi-acp `v0.0.33..42926cc`, herdr after `preview-2026-09-21-0ff0f27e2226`) | untagged, so recorded as post-tag leads only; the hard rules forbid filing against them. Start with pi `7fd564cbb` (pi.dev picks a catalog version by user agent; cyrup's `cyrup/…` agent is not checked against the live server) | each area's post-tag lead list |
| pi `harness/**` and `packages/durable` design and test material: `pico-v5.md` §2–§11, `packages/agent/docs/**`, the durable tests, the harness conformance suite, the v0.85.1 bodies of 27 `harness/runtime` files | pi ships none of it to users; area 17 records the grep that would change that | `HARN-001`/`002`, `AGENT-028` |
| pi `packages/chord` line by line | nothing cyrup ports imports it; checked by importer only | `EXT-088` (tracker) |
| pi `packages/tui` component diffs outside the assigned leads (model, scoped-models, thinking, trust, settings and extension selectors, `session-share`), and `packages/tui/test/**` | outside the second pass's assignment | area 07 |
| pi `agent-session.ts` `v0.84.1..v0.85.1` hunks beyond the abort, compaction and model hunks | area 08's file; not in either pass's brief for 02/03/06 | area 08 |
| pi `experimental/micro` `runtime.ts`/`tui.ts` | nothing in pi launches it | area 17 |
| cyrup's summarization-auth call site, against pi v0.86.0's `getAuth(model, {signal})` cancellation | not opened | area 05/08 |
| pi-subagents `v0.57.0..v0.67.0` `src/` line by line (207 files) | size; every lead from it has a disposition instead | `09b` |
| pi-subagents Herdr-placement hunks of `async-execution.ts`/`agents.ts` | already owned | `SUBA-100` |
| pi-mcp-adapter `977577f` non-theme panel hunks | sampled, not read line by line | area 13 |
| area 13's 347 `implemented` rows as a regression set; `MCP-522`/`525`/`531`'s TypeScript | never re-read against v2.33.0+ as a set | area 13 |
| `@agentclientprotocol/sdk` 0.26 (not in `tmp/pi-acp`) | what upstream answers for unimplemented methods is unreadable here | `ACP-014` |
| `cyrup-herdr`'s `relay.rs`, the ssh-runner half of `remote.rs`, `cli.rs` beyond verbs and error mapping; herdr's non-API changes after v0.9.1 | cyrup's own ssh relay protocol and herdr internals no cyrup call depends on; the herdr-facing surface was read | area 16 |
| runtime behaviour of every item | this is a static analysis: no item filed in either 2026-09-24 pass was run | *How much to trust an item* |

## Work this directory owns

Work with no area file to hold it. It is real work and it blocks or corrupts the rest.

- **Repair the dangling `.rs` citations, then guard them.** Most absolute `<file>.rs:<line>` citations
  in this directory point at a line that no longer holds what the prose says; the cause is
  concentrated, since the bulk were written in a single commit and the tree has moved by six figures
  of lines since. The repair is mechanical — recover the cited line's TEXT from the commit that last
  touched the doc line and re-find it at HEAD. **The standing fix is a CI check that resolves every
  `<file>.rs:<line>` and fails on any line or range end past EOF.** Until it exists, treat a citation
  as a lead, not an address.
- **`crates/cyrup-tui/src/app.rs` does not exist** — it was split into `crates/cyrup-tui/src/app/`,
  so every `app.rs:NNNN` is unresolvable rather than merely stale. The only honest repair is to
  re-find the symbol. **Do not repair a citation by shifting it**: only a small minority of same-file
  citation groups share a single offset, so a per-file `sed` corrupts more than it fixes. And **never
  write "identical at both tags"** — byte-identical bodies do not imply identical line numbers.
- **Publish a counting script.** The counting rule is prose, and two readers implement it
  differently: an independent implementation returned a different total than the figure it was meant
  to reproduce, at the same fixed commit. No total is trustworthy until the script is committed
  alongside it.
- **Re-run the duplicate census.** Area 12 marks many rows `duplicate-of` an item another area owns,
  and the ledger's F4 cluster lists further multi-ID defects nobody has reduced. **Do the reduction
  before a plan books the same fix twice.**
- **Build the schema/dispatch guard** asserting that every advertised property has a consumer. It
  would catch the whole unwired class as a class rather than one row at a time.
- **Claim or kill the ownerless `PARITY-GAPS.md` rows** — the version-lag entries `VL-P12`, `VL-P13`,
  `VL-P15`, `VL-P17` and the harness-v2 half of `VL-P22` predate the area files, carry citations,
  have never been re-derived at HEAD, and no area owns the fix.
- **Walk the surfaces below.** Each is finite, each has a one-command extraction on the pi side, and
  none has been walked end to end. A surface qualifies only if it is **finite and mechanically
  extractable on both sides**; anything else is a sweep, not an enumeration.

### Surfaces left to walk

**Partially walked — the residue:**

| residual | what is unwalked | extraction |
|---|---|---|
| env vars — reverse direction | the `CYRUP_SUBAGENT_*` / `CYRUP_INTERCOM_*` names have never been walked back to pi-subagents / pi-intercom, so `CFG-074`'s confirmed cyrup-originals **may not be all of them** | `grep -rhoE '"(CYRUP\|PI)_[A-Z0-9_]+"' crates/ \| sort -u` against each sibling upstream at its tag |
| env vars — pi-mcp-adapter | extracted, **never diffed** | **belongs to area 13, which is this repository's own work (there is no "MCP team") — diff it when area 13 is next audited** |
| extension API — citations | the non-`types.ts` citations (`agent-session.ts`, `tui.ts`, `event-bus.ts`, `exec.ts`, `agent/types.ts`, `project-trust.ts`, `tool-definition-wrapper.ts`) were spot-checked, not resolved | the citation-lint test `EXT-072` and `EXT-073` both specify — resolve every `<file>:N` against the checked-out tag and assert the cited line contains the cited symbol. **Land the guard, not just the rewrite** |
| RPC — payload shapes | commands, events, envelopes and `RpcSessionState` are 1:1; the **response DATA shapes behind the commands** were checked only where a finding was suspected | extract each `case "<cmd>"` return object from `rpc-mode.ts` at the ported tag vs each arm of `crates/cyrup-modes/src/rpc.rs` |
| providers — request bodies | the compat matrix is exhaustive; **request-body fields beyond compat** are not | per wire API, diff the assembled request object against `crates/cyrup-provider/src/api/*.rs` |
| providers — catalog residue | the catalogs are measured at `b0c2a90e`, before the ported tag, and the data is genuinely not in git after `a9f6a3159` | unfixable by reading; needs `PROV-018`'s generator run |

**Never walked at all:**

- **Session JSONL entry types and their fields** — every `type` discriminant and every field pi writes
  into a session file, vs `crates/cyrup-session`'s `Entry` enum. Area 03 has closed items on
  individual fields; nobody has diffed the *set*. The `cwd`-writing bug (`SESS-037`) is what this
  finds.
- **System-prompt sections and their exact text** — `core/system-prompt.ts` assembles a fixed list of
  blocks; wording drift has been found one item at a time (`SESS-019`, `SESS-024`, `SESS-035`).
- **User-visible error messages and exit codes across the binary** — pi's throw/exit sites vs cyrup's.
  `SEAM-101` (config exits 0 where pi exits 1) and `SEAM-104` (a bare `-` became a prompt) were both
  incidental finds.
- **Tool-result `details` payload shapes** — the serialized payloads that reach the session file are
  unwalked past the handful already diffed (`TOOL-044` came out of the seventh look).
- **Theme tokens and colour roles** — a closed finite list on both sides; `EXT-066` says the seam is
  already thin.
- **Autocomplete providers and their trigger characters** — `@`, `/`, and the extension-registered
  tier; `TUI-077` found the slash half by accident.
- **The three sibling upstreams' own CLI/env/config surfaces** — `pi-subagents`, `pi-intercom`,
  `pi-permission-system`. **Every surface so far was walked against `pi` only**, and not one open row
  in areas 09, 10 or 11 came from an enumeration.
- **Agent frontmatter / `agents.md` schema keys** and the permission rule grammar (action names,
  policy-file keys, match syntax) — finite, authoritative, never diffed as sets.
- **Markdown block types and the transform pipeline** — `EXT-019` landed the mechanism; the block-type
  set was never enumerated.
- **pi's shipped docs as a surface** — `docs/settings.md`, `docs/keybindings.md`, `docs/rpc.md`. **A
  shipped doc is an independent enumeration of an implementation surface and is the cheapest
  cross-check available**; the keybinding walk settled its own scope with
  ``git -C pi show <tag>:packages/coding-agent/docs/keybindings.md | grep -c '^| `'``.

**Two rules for whoever walks one.** Emit the extraction commands as a first-class field of the
artifact — a previous `surfaces.json` lost them and its parser has to be rewritten. And **diff both
directions explicitly**: the `cyrup-original` class exists only because someone did, and it is the
class through which divergence enters while everyone is looking at parity.

## How much to trust an item

**Every item is a lead to verify, not a fact.** Items have been wrong about the mechanism rather than
merely stale — a claimed palette that never existed, providers named as missing that were always
implemented, CLI flags named that exist at neither tag. Expect a similar residue in what is open now.

- **This is a static analysis except where an item says otherwise.** Unless a row carries an
  `observed` marker, nothing was built, run or reproduced: it is evidenced by reading both sources,
  and its `Verify` line is a design rather than an observation.
- **A severity raise must cite an observation, or say plainly that it does not.** Raises made on
  *predicted* consequence have been refuted by a single measurement. The procedure applies the
  severity definition to an item's own Impact prose; where that prose is a prediction, it faithfully
  promotes a prediction into a rating.
- **For TUI work this is not a formality.** ratatui `TestBackend` tests pass while the assembled
  application has layout and empty-state bugs, and the defects that matter most are invisible to a
  static read — an indicator whose source looks correct and that never reaches the screen. **No
  `TUI-*` item is done until it has been run in a real terminal.**
- **Validate your instrument as a first-class step.** Measurement errors here have included `tail`
  hiding a failure, `pgrep -f` matching its own pattern and inventing orphaned processes, and
  `tmux display-message` reporting a stale hardware cursor while cyrup paints its caret as an SGR-7
  cell.
- **Severity and effort are judgements, not measurements.** Treat any suggested ordering as a
  starting proposal.
- **There is no `spec/` tree and no readable ADR set in this workspace.** Where code invokes an
  `R-NN-NNN` id or an ADR to justify a divergence, that is an unverifiable claim, not a decision of
  record — one item sat at `low` for months on an ADR citation nobody could read.

## Do not re-file — claims already disproved

Each of these was investigated and killed with evidence. They are here for one reason: to stop the
next pass re-filing a hypothesis that has already been refuted. **Re-file one only by first refuting
the evidence quoted here.**

**The one that cost behaviour, twice over:**

- **S24 — "pi never draws `⊟` in the tree connector." WRONG, and acting on it deleted a working
  feature.** The row read the guard at `tree-selector.ts:734` backwards.
  `foldMarker = isFolded && !showsFoldInConnector ? theme.fg("accent","⊞ ") : ""` does **not** mean
  "pi only ever shows a folded marker". `showsFoldInConnector` is
  `flatNode.showConnector && !flatNode.isVirtualRootChild`, so `!showsFoldInConnector` reads *"the
  connector did NOT already show the fold state"*: `:734` is the FALLBACK for a node with no
  connector to put the cell in (depth 0 / virtual-root children). The general case is `:722`, inside
  the connector — `prefixChars.push(isFolded ? "⊞" : foldable ? "⊟" : "─")`. pi draws `⊟` on every
  foldable, expanded node that has a connector. A batch acting on the wrong row deleted `⊟` from
  cyrup **and inverted two tests into asserting its absence**; both are restored.
  **The general rule: when a marker is gated on a `!shows…InX` predicate, find X and read what X
  emits before concluding the marker's site is the only site.** An inverted guard plus an inverted
  test is invisible to the gate — the suite is green in exactly the state where the feature is gone.

**Unreachable code mistaken for a rendered defect** (the trap: quoting a literal proves the literal,
not that anything reaches it):

- **F22 — hex fallback defaults in `role_style(key, default_hex)` not matching `dark.json`.** The
  hexes are correctly quoted on both sides, but the code is **unreachable**. `role_style`/
  `role_color` fall back only when `self.roles` lacks the key; the synthetic-empty-roles constructor
  is called only from `UiTheme::dark()`/`light()` with names `builtin_themes()` always resolves; and
  a custom theme omitting a token is blocked by `cyrup-resources/src/theme.rs:109-121`, which
  hard-errors on any missing `REQUIRED_COLOR_TOKEN`. **No real session renders `#666666` fence
  borders because of this.** The residual — a syntactically valid string that is neither a hex nor a
  defined var, dropped by `from_theme_data`'s `filter_map` — is a real but far narrower bug.
- **F21 / F142's hex-fallback clause.** Dead for exactly the same reason as F22. The live half of
  those rows is the `Modifier::DIM` alone.
- **F90 — the data-selector empty-state string.** The literals are as quoted, but
  `SelectList::with_no_match` only renders when `items.is_empty()`, and every production
  `open_data_selector` call site guards emptiness first (Logout, UserMessage, Login);
  BranchSummary uses `ListSelector::prompt` with three hardcoded rows. **No user ever sees the
  string.**

**Claims about cyrup's colour handling that are simply false:**

- **"cyrup ships one palette that assumes dark."** False. `cyrup-resources/src/theme.rs` matches
  `dark.json` *and* `light.json` token-for-token across all 50 shared tokens, `vars` and `export`
  blocks included, and `ThemeController` ports `resolveThemeSetting` / `parseAutoThemeSetting` /
  terminal-background detection. The light-theme damage was entirely downstream of SYS-1.
- **"cyrup hardcodes colours where pi uses tokens."** Essentially false. `grep "Color::"` over
  `crates/cyrup-tui/src/*.rs` yields two files: `image.rs` (two `Color::Reset` *equality tests*) and
  `theme.rs` (`unwrap_or(...)` fallbacks + hex-parse plumbing). **There is no `Color::Rgb` literal in
  any renderer.** All colour drift is accessor-level or call-site-level, never a magic number.

**Rows whose scope, severity or direction was wrong as first written** — re-file only the corrected
half:

- **F134's scope ("the single most common shape, nearly every turn").** Wrong. With a blank line
  before a list (the normal case) marked's `space` token pushes `""` (`markdown.ts:619-622`), so pi
  and cyrup both produce exactly one blank row. The divergence occurs **only** for a list that
  interrupts a paragraph with *no* blank line in the source.
- **F40's effect direction — inverted.** `chars().count()` **under**-counts wide glyphs, so
  `width - len` is too **large**: the line is **over**-padded and spills past the frame into a
  spurious extra tinted row. (It was filed as under-padding.) Corrected as L6.
- **F31 / F88 / F124's "2 columns off" clause.** Fires only when the clamp binds
  (`primary_min`/`primary_max`); when `primary_min <= widest` and `widest + GAP <= primary_max` both
  sides land the description at `2 + widest + 2` and agree exactly. **The scroll-window jitter is the
  always-on half** — that is the part worth fixing.
- **F19's `scrollbarThumb` sub-claim ("cannot even be expressed").** Overstated — pi itself defaults
  it (`theme.ts:164`, `withThemeColorFallbacks` at `:330`), so a palette omitting the key is harmless
  upstream. The real defect is only that no cyrup renderer reads it.
- **F39's severity.** Lowered: pi gates the scrollbar on `getFullscreenScrollbar()`
  (`interactive-mode.ts:873`), an alt-screen-only feature, and cyrup runs an inline viewport by
  design.
- **S3 / S28 are NOT properties of the shared `SelectList`.** Neither the hint row nor the
  one-column inset is generic; both are built per-component upstream. A cyrup "fix" putting either on
  `ListSelector` unconditionally reaches ~10 dialogs where pi draws them on 4 (hints) and 6 (inset).
  Both are opt-in via `SelectorKind::draws_hint_row()` / `insets_rows()`, whose doc comments carry
  the per-component evidence. **Generalising a per-caller behaviour onto a shared widget is a
  divergence, not a port.**

**Live-use rows closed by measurement on 2026-09-04 — refute the transcript in `REPRO-LOG.md` §0e
before re-filing any of these:**

- **"cyrup never reads `defaultProvider`/`defaultModel` back — rank 4's input is permanently
  empty"** (`SEAM-113`). Wrong at HEAD: `resolve_default_launch_model`
  (`crates/cyrup/src/bootstrap.rs:247-275`) reads both keys at `:269-270` into
  `default_launch_model` → `find_initial_model`; seeding the pair into
  `<CYRUP_HOME>/.cyrup/agent/settings.json` changes what a fresh launch resolves. What is true is
  that only the picker's Ctrl+S writes them — which is pi v0.84.4's contract, not a gap.
- **"pi has a typed `--default` flag on `/model` and `/thinking` that cyrup lacks."** It lived one
  day on pi `main` (added 2026-08-19, deleted by `5133c9284` on 2026-08-20, both inside the v0.84.3
  window) and shipped in no tag; `git -C tmp/pi grep parseDefaultFlagArgs v0.84.4` is empty and the
  v0.84.4 hints are `<provider/model>` / `<level>`, byte-identical to `crates/cyrup-tui/src/commands.rs:147,165`.
- **"Reasoning blocks never render"** (`TUI-091`). At HEAD `a4805955`, in a real pty, they render
  live and committed in seven variants including the reporter's exact launch; the report predates
  `TUI-090`'s fix by three hours and its commit body names the mechanism. Reopen only on a new live
  observation in a real terminal — never on `TestBackend`.

**Verified-matching pairs — do not re-derive:**

- **"`config_selector.rs:372`'s group line is accent+BOLD vs pi's plain accent."** Wrong:
  `config-selector.ts:418-419` is `theme.fg(inherited ? "dim" : "accent", theme.bold(label))` —
  accent **and** bold, exactly what cyrup does. The config-selector defects are S17-S19.
- **"`selectedBg` is used in exactly one place upstream."** Wrong — `session-selector.ts:507` uses it
  too. See SYS-4.
- **"`overlay.rs` has no pi counterpart, so there is nothing to audit."** Wrong —
  `interactive-mode.ts:6090-6204 handleHotkeysCommand` is the counterpart, rendering the same content
  as a transcript markdown block. It was filed as S36.
- **`h-stack` / `v-stack` / `stack` gap handling.** Not a finding: `git grep "gap:"` at v0.84.1 over
  `packages/coding-agent/src packages/tui/src` returns only the declarations and CSS. No caller ever
  passes a non-zero gap, so `this.gap` is always 0 — matching ratatui's gapless layout.
- **Spinner frames and interval.** Byte- and millisecond-identical (`loader.ts:11-12` ↔
  `status_indicator.rs:28-30`).
- **`markdown.rs`'s code-block indent, HR glyph, H1 underline, `### ` prefix rule, blockquote prefix
  and `trim_partial_closing_fence`; and `ansi.rs` entire.** All verified matching.
- **`session_search.rs` ↔ `session-selector-search.ts`.** A pure query DSL with no render surface on
  either side. pi does no match-highlighting anywhere in these components (`fuzzyFilter` returns
  items, not spans), so the absence of highlight styling in cyrup is **correct**, not a gap.

Two §8 entries were deliberately **not** rescued as traps, because they are live coverage rather than
killed claims, and ADR-0009 item 5 routes them to area 07's `## Coverage` instead:
`startup.rs`/`startup_selector.rs` vs `interactive-mode.ts:1480-1690 showLoadedResources`, and
`login_dialog.rs` ↔ `login-dialog.ts` — both **unaudited, not clean**. "Nobody has looked" is a
coverage statement, not a trap and not an item.
