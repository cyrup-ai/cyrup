# cyrup gap analysis

A ledger of the behavioural differences between the **cyrup** Rust port and its upstreams, kept as a
work backlog. Every entry is work: there is no "accepted divergence" category and no "undecided"
status. Mechanism may differ where the language forces it — port the behaviour, state the mechanism
difference and its reason, and if the mechanism difference costs behaviour, it stays on the list.

**Five upstreams are ported**: the four TypeScript ones — `pi`, `pi-subagents`,
`pi-permission-system`, `pi-intercom` — plus **`code_puppy_core_plugins`, which is Python**, ported
as `crates/cyrup-flux`. A sixth, `pi-mcp-adapter`, is TypeScript and **not** ported (area 13, below).
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
| [`10-cyrup-permission-system.md`](10-cyrup-permission-system.md) | allow / ask / deny gate |
| [`11-cyrup-intercom.md`](11-cyrup-intercom.md) | supervisor↔subagent broker |
| [`12-upstream-drift-pi-core.md`](12-upstream-drift-pi-core.md) | pi core drift since the ported baseline |
| [`14-cyrup-flux.md`](14-cyrup-flux.md) | the Flux pipeline — the fifth ported upstream, and the first that is neither pi nor TypeScript |

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

A **fifth upstream**, `pi-mcp-adapter` v2.25.0 (~24k lines of TypeScript, 203 paths), which has never
been ported. **Area 13 is scoped and counted separately from the twelve areas, and the separation is
load-bearing**: the twelve measure drift in code that exists, while area 13 specifies code that does
not exist yet, so a forward-looking port unit and a backward-looking defect cannot be added together
into anything a planner can act on. **The exclusion runs both ways: nothing in `README.md`,
`00-residual-ledger.md` or `PARITY-GAPS.md` speaks for `13-cyrup-mcp.md`, `13a`–`13i` or
`MCP-PORT-METHODOLOGY.md`, which another team owns — and those files' own tables are the authority
for their unit inventory and its status.**

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

| repo | HEAD | cyrup ported baseline | latest tag | delta |
|---|---|---|---|---|
| `cyrup/` | **`275c1f85`** — last CODE commit as of the 2026-09-04 second pass (`feat(subagents): SUBA-086 …`, branch `claude/beautiful-feynman-odz1v5`, five commits off `main` = `a4805955`, which merged the `2571969` full re-audit). *The docs commit that wrote this row cannot cite its own sha; the code sha is the one a status is measured against.* **Re-measure before trusting any status: the port moves faster than this directory does.** | — | — | 21 crates, ~714k lines of Rust under `crates/` (`find crates -name '*.rs' \| xargs wc -l \| tail -1` = 714 265; `for d in crates/*/; do [ -f "$d/Cargo.toml" ] && echo "$d"; done \| wc -l` = 21) |
| `pi/` | `6aedd1066` = `v0.84.3-453-g6aedd1066` | **v0.83.0** | **v0.84.4** | 775 files, +68 885 / −20 827 |
| `pi-subagents/` | `a5f401e8` | **≈v0.43.0** (inferred — the crate records no version string) | **v0.64.0** | 485 files, +92 664 / −18 069 |
| `pi-permission-system/` | `9affcc9` — **re-checked this pass, unchanged from the prior record** | **v0.7.1** | **v0.8.0** — **re-checked this pass, unchanged from the prior record** | 28 files, +4 023 / −1 851 (re-measured identical to the prior record) |
| `pi-intercom/` | `199279a` | **v0.9.2** — *not v0.7.0; every prior doc had this wrong* | **v0.13.0** | true window `v0.9.2..v0.13.0` = 26 files, +4 701 / −976 |
| `code_puppy_core_plugins/` | `8c6f852` | **v0.0.6** — *not recorded anywhere in `crates/cyrup-flux`; see `FLUX-007`* | **v0.0.40** | 139 files, +11 071 / −3 822 across the whole repo — but the ported surface is unchanged: `git diff --stat v0.0.6..v0.0.40 -- code_puppy_core_plugins/flux_bootstrap/ tests/test_flux_bootstrap.py` is empty, byte-identical across all 34 intervening tags. Ported surface is `flux_bootstrap/` — 18 bundled commands, 4 `_docs` files, 3 renderer scripts. cyrup ships 15 templates + 3 native renderers = the same 18 |
| `pi-mcp-adapter/` | `6ba7d36` = `v2.32.1-3-g6ba7d36` — **clone re-pulled 2026-09-04; area 13 NOT re-audited against it** (the MCP team owns area 13; its files were not opened this pass). *Superseded: `14c0e6c` = `v2.25.0-4-g14c0e6c`.* | **not ported** — area 13 is the plan | **v2.32.1** *(was v2.25.0; the `v2.25.0..v2.32.1` window is unmeasured here)* | 203 paths / 164 `.ts` at the tag, ~24 200 lines; drift to HEAD is 17 files, +543 / −69 |

**Read upstream with `git -C <repo> show <tag>:<path>`, never from a working tree.** Clone-HEAD line
numbers and file existence both mislead; items have named files that never existed at any tag.

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
| env vars — pi-mcp-adapter | extracted, **never diffed** | **routed to the MCP team's files, not to this directory** |
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
