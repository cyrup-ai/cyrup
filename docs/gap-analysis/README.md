# cyrup gap analysis

A verified ledger of every behavioral difference between the **cyrup** Rust port and its four
TypeScript upstreams, written to be used as a work-item backlog.

**Start at [`PARITY-GAPS.md`](PARITY-GAPS.md)** — it is organised by gap class (port bug / unwired /
version lag / reverse lag / deletion candidate / open question), opens with **§0a: every item above
medium in one table**, and shows the shape of the remaining distance.
[`00-residual-ledger.md`](00-residual-ledger.md) ranks and suggests an order. The numbered files hold
the evidence.

> **Re-baselined 2026-08-12 against cyrup `04c1ba2`** (last code commit; docs HEAD `a9000b1`, branch
> `david/cyrup`, tree clean). All twelve area files were re-audited against a **named upstream tag**
> on both sides, then reviewed by a completeness critique and repaired against its findings. **117
> items closed, 207 newly filed** — most of the new ones come from auditing what the *closing* code
> actually does. Closed items keep their IDs in each area file's status table so a closure can be
> re-audited later.
>
> **Open set: 448 work items — 6 critical, 22 high, 197 medium, 223 low**, plus **9 `tracker` rows**
> that keep their IDs but propose no schedulable work and are excluded from the count.
>
> **The severity scale is now applied rather than narrated.** The previous edition's headline was "0
> critical" while six items met the definition below on their own text. Four were raised
> (`AGENT-020`, `TUI-027`, `EXT-054`, `PERM-009`), the repair pass filed two more (`TUI-042`,
> `TUI-043`), four items moved up to high (`TOOL-039`, `SEAM-051`, `PERM-023`, `DRIFT-049`), and one
> item held down by an ADR that cannot be read in this workspace (`TUI-019`) was re-rated on
> consequence. Corrections went both ways: `PB-3` and `PB-19` were corrected **down**.
>
> **Two of the four baselines this analysis had been measuring against were wrong.**
> `pi-subagents` latest was recorded as v0.43.0; it is **v0.47.1**, so 151 files / +10 254 / −1 333
> had never been looked at. `pi-intercom` latest was recorded as v0.9.2; it is **v0.10.1**. And the
> `pi-intercom` *ported* baseline was recorded as v0.7.0 for every prior pass — a citation census
> over `crates/cyrup-intercom/src` returns **v0.9.2 × 272** against v0.7.0 × 14, so the true baseline
> is **v0.9.2**. That correction alone moved six items out of "version lag" and into port bugs. See
> structural blind spot 3 below, and `PARITY-GAPS.md` §1d and §7.

## Contents

| file | area | open items | crit | high |
|---|---|---:|---:|---:|
| [`../PARITY-PLAN.md`](../PARITY-PLAN.md) | **the execution plan derived from this directory — 30 batches, the next three moves, deferrals and open questions** | — | — | — |
| [`PARITY-GAPS.md`](PARITY-GAPS.md) | **the same 448 items grouped by gap class — read first** | — | — | — |
| [`00-residual-ledger.md`](00-residual-ledger.md) | ranked cross-cutting view | — | — | — |
| [`01-cyrup-core-and-provider.md`](01-cyrup-core-and-provider.md) | wire APIs, providers, auth, streaming, catalogs, cost | 40 | 0 | 6 |
| [`02-cyrup-agent.md`](02-cyrup-agent.md) | the turn loop, tool dispatch, hooks, abort | 26 | 1 | 1 |
| [`03-cyrup-session.md`](03-cyrup-session.md) | JSONL session tree, compaction, system prompt | 29 | 0 | 1 |
| [`04-cyrup-tools.md`](04-cyrup-tools.md) | the built-in tool set | 29 | 0 | 1 |
| [`05-cyrup-config-and-resources.md`](05-cyrup-config-and-resources.md) | settings, model resolution, trust, skills, packages | 38 | 0 | 1 |
| [`06-cyrup-ext.md`](06-cyrup-ext.md) | extension host, WIT world, event catalog | 50 | 1 | 0 |
| [`07-cyrup-tui.md`](07-cyrup-tui.md) | terminal UI application layer | 56 | 3 | 1 |
| [`08-cyrup-session-svc-and-modes.md`](08-cyrup-session-svc-and-modes.md) | the integration seam, RPC, CLI, print/json modes | 40 | 0 | 7 |
| [`09-cyrup-ext-subagents.md`](09-cyrup-ext-subagents.md) | subagent delegation | 45 | 0 | 2 |
| [`10-cyrup-permission-system.md`](10-cyrup-permission-system.md) | allow / ask / deny gate | 21 | 1 | 1 |
| [`11-cyrup-intercom.md`](11-cyrup-intercom.md) | supervisor↔subagent broker | 44 | 0 | 0 |
| [`12-upstream-drift-pi-core.md`](12-upstream-drift-pi-core.md) | pi core drift since the ported baseline | 30 | 0 | 1 |
| | **total** | **448** | **6** | **22** |

Counts are the `## Open items` table of each file. **Every file now carries exactly one such table**
— area 03's second table was the last one and was folded in during the repair pass — so a single
enumeration is complete. Nine `tracker` rows sit in those tables (or, in areas 08 and 09, in a
separate `## Trackers` table) and are deliberately outside the arithmetic: one each in areas 01, 02,
03, 08 and 09, and four in area 12.

**Every one of these is a floor, not a total** — see blind spot 1. It is also not a clean total in
the other direction: area 12 marks **16 of its 30** rows `duplicate-of` an item another area owns, so
432 is the largest deduplicated figure anyone has actually computed, and the ledger's F4 cluster
lists further multi-ID defects nobody has reduced to a number.

Numbering follows the convention already referenced in cyrup's source
(`spec/gap-analysis/03-cyrup-agent.md`, `12-cyrup-tui.md`, `00-residual-ledger.md`). That `spec/`
tree is not in this workspace, so exact alignment with it is unverified.

## Baselines measured against

| repo | HEAD | cyrup ported baseline | latest tag | delta |
|---|---|---|---|---|
| `cyrup/` | **`04c1ba2`** (last code commit; docs `a9000b1`, branch `david/cyrup`, clean) | — | — | 18 crates, ~482k lines of Rust under `crates/` |
| `pi/` | `581d75a89` = `v0.84.1-117-g581d75a89` | **v0.83.0** | **v0.84.1** | 627 files, +52 291 / −17 556 |
| `pi-subagents/` | `9e9fd13` | **≈v0.43.0** (inferred — the crate records no version string) | **v0.47.1** | 151 files, +10 254 / −1 333 |
| `pi-permission-system/` | `9affcc9` | **v0.7.1** | **v0.8.0** | 28 files, +4 023 / −1 851 |
| `pi-intercom/` | `30dcbdd` | **v0.9.2** — *not v0.7.0; every prior doc had this wrong* | **v0.10.1** | true window `v0.9.2..v0.10.1` = 24 files, +2 495 / −700 |

Three standing hazards in this table. **(a)** The intercom baseline is the one that bites in both
directions: diffing from v0.6.0 or v0.7.0 reports a pile of already-done work as debt, and
`crates/cyrup-intercom/src/lib.rs:2` still says v0.6.0 (tracked as `ICOM-012`). Diff
`v0.9.2..v0.10.1`. **(b)** pi HEAD is **117 commits past v0.84.1**, so that range is unanalysed by
construction — items in it are deliberately not filed, because the hard rules require citing a named
tag. **(c)** A classification turns on which side of the **ported** tag a symbol landed, and a commit
hash does not answer that. Settle presence with `git cat-file -e <tag>:<path>` before writing
`upstream-drift`; six area-12 items were misfiled as lag until someone did.

Read upstream with `git -C <repo> show <tag>:<path>`, never from a working tree. Clone-HEAD line
numbers and file existence both mislead, and at least one item in a prior pass named a file that has
never existed at any tag.

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
`upstream-drift` (landed after the baseline; expected lag) · `stale-port` (cyrup carries behavior
upstream changed or deleted) · `cyrup-original` (no upstream basis) · **`test-defect`** (a test
pinning wrong behavior, or asserting a timing/scheduling outcome it cannot control) · plus a small
tail of `tooling` / `port-divergence`. `PARITY-GAPS.md` §0 maps every kind onto its gap class and
shows the census.

**`tracker`** is not a severity and not a kind — it is a row that proposes **no schedulable work**,
either because it indexes items other files own or because it asks a scope question. A tracker keeps
its ID, its status row and its full body, and is excluded from every count, because a number that
mixes work with bookkeeping cannot be planned against. Each tracker records what would escalate it
back into the counted set. Two of the nine (`DRIFT-023`, `DRIFT-040`) are further marked **leads**:
neither side was ever re-read, so they are held to a lower evidence standard than an item and must
not be counted as one.

Each area file also opens with a **status table** covering every item from every prior pass:
`closed` · `partially-closed` · `still-open` · `misdescribed` · `superseded`. **IDs are never
renumbered or deleted** — closed items keep theirs so a closure can be re-audited, and where an item
changes class it keeps its number and moves section. A gap in a number range is not evidence of a
deletion: `SEAM-035`…`SEAM-046` never existed, and area 08 records the check that establishes it.

**Severity** is judged by user-visible consequence, not code size: `critical` = data loss, silent
wrong output, a permission bypass, or a crash on a normal path. **That definition carries no
reachability qualifier** — `EXT-054` is critical although no WASM guest ships today, and the blast
radius is recorded inside the item as scheduling information rather than as a rating. Severity is
also never held down by an unverifiable justification: an in-source ADR or requirement id that cannot
be read from this workspace is not a decision of record (see Caveats).

**Effort** — `S` under a day · `M` a few days · `L` a week+ or needs design.

## How this was produced

Twelve areas, each run through three independent passes: an analyst enumerating gaps with two-sided
evidence, an adversarial verifier instructed to **refute** every item and to default to rejection
when it could not personally re-read both sides, then a writer rendering only the survivors. Each
file's `## Coverage` section lists what was read, the blind spots, and every rejected item with its
reason — so a later reader can see what was already considered and dismissed rather than re-deriving
it.

A fourth stage was added on 2026-08-12: a **completeness critique** read all fifteen finished files
as a set and returned 17 findings, which five repair agents then applied area by area. It found what
a per-area pass structurally cannot — three cross-cutting files each declaring a different one of
them current, a severity scale quietly unenforced, one factual contradiction about upstream that had
produced a wrong Fix, and four upstream subtrees no file claimed to have read. **Regenerate the
cross-cutting files last, and have someone read the whole set afterwards.**

On the re-baseline passes the verifier's primary duty is **inverted**: rather than confirming
findings, it is told to **refute every `closed` claim**, on the grounds that a wrongly-closed item
deletes a real defect from the backlog and nobody looks again. Closure requires reading the Rust at
HEAD and the TypeScript at the named tag; a commit message asserting a fix is explicitly treated as a
hypothesis, not evidence. That scepticism keeps paying: on 2026-08-12 area 01 found four follow-on
defects (`PROV-027`/`028`/`029`/`030`) inside the code that closed `PROV-005`, and area 11 found two
inside the code that closed `ICOM-022` and `ICOM-002`.

The refresh also mines `git log` for debt that exists **only in commit messages** (deferred
subsystems, a deliberate WIT ABI break, known limitations), and runs a systematic hunt for the
`test-defect` class after three instances were found by accident. That hunt returned 27; 23 remain
open.

Known traps are fed to every pass so they are not re-reported as discoveries: the `loop_fn.rs`
facade, pi's two forked compaction implementations, the provider `fleet!` macro hiding ~20
registrations, `wasm-host` being default-on, the out-of-scope pi packages, and the deliberately
unreachable first-run wizard. **Two of those traps are now contested by evidence** — see
`PARITY-GAPS.md` UW-2 / OQ-6 for the wizard, and blind spot 6 for the out-of-scope package list.

## Structural blind spots, all found the hard way

Each was found because something the analysis had looked straight at and blessed turned out to be
wrong. They are properties of the *method*, so they will keep producing misses until the method
changes.

**1. An item-driven analysis cannot see behaviour nobody wrote an item for.** Every pass starts from
a list and asks "is this item real?". A pi function with no corresponding item is invisible to all
three passes, including the adversarial one — the verifier refutes claims, and there is no claim to
refute. The fix is the **surface-driven sweep**: walk upstream itself, and for each exported symbol /
event / config key / CLI flag / env var ask "what in cyrup consumes this?". One such sweep added 58
items, 6 of them high; the 2026-08-12 sweeps added far more, and the repair pass's five new sweeps
added 31 more including four criticals. One sweep is unlikely to have exhausted the class; **treat
every open count as a floor, not a total.**

**2. The ADR-0001 substrate carve-out was applied far too broadly.** "cyrup delegates rendering to
ratatui + crossterm, so pi's hand-rolled `render(width): string[]` framework is out of scope" is
correct — for the *drawing* layer. It was silently extended to everything living in pi's
`packages/tui/src/tui.ts`, including behaviour that draws nothing: input sanitation, terminal-reply
handling, mode negotiation, paste and focus semantics. Those are portable and in scope. **Before
invoking ADR-0001 on a `tui.ts` line, check whether it actually draws anything.** The repair pass
paid this debt directly: sweeping `packages/tui/src`'s non-drawing files — `stdin-buffer.ts`,
`word-navigation.ts`, `undo-stack.ts` — produced nine items, two of them **critical**.

A corollary worth stating separately, because it generalises past the TUI: **not enabling a feature
does not make its hazards moot.** `TUI-004` reasoned that mode 2031 is off, so unsolicited terminal
pushes cannot arrive — ignoring that cyrup *does* issue an OSC-11 query and therefore must handle its
reply, including a reply that arrives late. Ask what the code *sends*, not only what it *enables*.

**3. NEW (2026-08-12) — the recorded baseline is itself an unverified claim, and a wrong one silently
reclassifies work.** `pi-intercom` was recorded as ported-from-v0.7.0 by every document for months.
It is v0.9.2: a citation census over the crate returns v0.9.2 × 272 against v0.7.0 × 14, and
load-bearing v0.8.0/v0.9.x code is present *and tested*. The consequence is not cosmetic — six items
sat in "version lag" (out of scope until the next bump) that were in-baseline **port bugs** (top
priority), and the drift window was a whole minor version too wide. The same error in the other
direction hid the entire `pi-subagents` v0.43.0..v0.47.1 range, because "latest" was recorded as
v0.43.0 and nobody re-checked. Counters, both cheap:
- **Census the baseline, do not inherit it.** Count in-tree `vX.Y.Z` citations per crate and compare
  against the recorded number before trusting any `not-ported` vs `upstream-drift` call.
- **Re-measure "latest" every pass.** `git -C <repo> describe --tags` and
  `git diff --stat <baseline>..<latest>` are the first commands of an audit, not the last.
- Where the classification actually turns on it, settle presence directly with
  `git cat-file -e <tag>:<path>` rather than by date reasoning. That is how three `pi-subagents`
  items were re-classified, how two named upstream files were struck as never having existed, and how
  six area-12 items moved out of `upstream-drift` in the repair pass.

**4. NEW (2026-08-12) — a cross-cutting document can orphan work no area file owns.** `PARITY-GAPS.md`
carries entries that predate the area files; when the areas were re-audited, four version-lag rows
(`VL-P12`, `VL-P13`, `VL-P15`, `VL-P17`) and the harness-v2 half of `VL-P22` were claimed by nobody.
They read as verified because they carry citations, but nothing re-derived them at the current HEAD
and no area owns the fix. **Every cross-cutting entry must name an owning area item or be explicitly
marked ownerless** — an unowned entry with a citation is more dangerous than no entry at all. Those
same four rows went unclaimed a second time in the repair pass, which is what an unowned row does.

**5. NEW (2026-08-12) — "has a consumer" is too weak a test for the unwired class.** Area 05's sweep
let `doubleEscapeAction` through on a previous pass because it *was* consumed — by a `/settings`
display row, and by nothing else. **A settings row is not a consumer.** The same shape recurs as
*advertised but unimplemented* (`grant-spawn-budget`), *implemented but unadvertised* (`toolBudget`,
`outputSchema`), and *delivered but never rendered* (`ui.setWidget`/`setHeader`/`setFooter`). The
durable countermeasure is a test rather than a sweep: **a schema/dispatch guard asserting that every
advertised property has a consumer**, which would have caught three area-09 items as a class.

Note that the mechanical fix for the *previous* enumeration defect is now complete: the surface-sweep
`-S` items used to live in a second table and were invisible to first-table enumeration, which cost
`SEAM-S01` an entire audit pass. **All twelve files now carry exactly one open-items table** — area
03 was the last exception and was folded in during the repair pass. Do not re-split one.

**6. NEW (2026-08-12, repair pass) — a surface the sweep dismissed as out of scope is invisible to
every later pass, and the dismissal is never re-examined.** An exclusion is written once, in one
line, usually with a plausible reason — and from then on no pass looks at it, because every pass
inherits the previous pass's scope. Area 12 dismissed pi's root `scripts/` as "dev/release tooling
with no runtime effect", which is how `packages/ai/scripts/generate-models.ts` — 2733 lines, present
at **both** tags, exposed as `npm run generate:models` — came to be declared non-existent in three
files, one of which built a whole Fix on the false premise. Four upstream subtrees were unread by
anybody until the repair pass: `packages/tui/src`'s input pipeline, `packages/coding-agent/src/cli/`,
`packages/ai/src/utils/` and `packages/coding-agent/src/bun/`. **Five of the ten items from
`cli/` alone came back `high`** — the axis, not the diligence, was the variable. Two counters:
- **Every sweep must publish what it EXCLUDED, as an explicit list with a reason per entry**, so the
  exclusions are auditable rather than silent and the next pass can re-open one cheaply. Record the
  negative results too — "read, nothing found" is worth as much as an item.
- **"No runtime effect" licenses skipping a directory's *behaviour*, never its *provenance*.** A
  gitignored path is evidence that an artifact is generated — hence that a generator exists — not
  evidence that it is absent.

## Caveats

- This is a **static** analysis. Nothing here was built, run, tested or reproduced. Items are
  evidenced by reading both sources, not by observing behavior. Every `Verify` line is a design, not
  an observation.
- **For TUI work this is not a formality.** ratatui `TestBackend` unit tests pass while the assembled
  application has layout and empty-state bugs. No `TUI-*` item — nor `SESS-040`, nor the pre-launch
  surfaces in `SEAM-061`…`SEAM-067` — is done until it has been **run in a real terminal**.
- Severity and effort are judgements, not measurements. Treat any suggested ordering as a starting
  proposal.
- **There is no `CLAUDE.md` in this workspace**, and no `spec/` tree or ADR documents. Earlier
  editions cited a workspace `CLAUDE.md` for a "deliberate divergences" list and an out-of-scope pi
  package list; that file cannot be read here, so every claim sourced from it is unverifiable. No
  item may rest on one, and where a code comment invokes a `R-NN-NNN` id or an ADR to justify a
  divergence, treat it as an unverifiable claim rather than a decision of record. This is not
  hypothetical: `TUI-019` was held at *low* for months on an ADR-0001 citation nobody could read.
- **There is no "accepted divergence" category.** The goal is behavioural equivalence. Mechanism may
  differ where the language forces it — port the behaviour, state the mechanism difference and its
  reason, and if the mechanism difference costs behaviour, it stays on the list as work.
- The upstreams keep moving, and two of four "latest" figures were stale within days. Re-run the
  version diffs before trusting any `upstream-drift` count (blind spot 3).
- Several items in past editions were **wrong about the mechanism**, not merely stale, and were
  corrected in place — `DRIFT-005` was already fixed before anyone worked it; `DRIFT-001`'s
  `addedToolNames` is a cache-*placement* record; `TUI-002`'s claimed `thinkingText` palette never
  existed; `PROV-005` named xAI/Groq/DeepSeek as missing when they were always implemented; `SEAM-019`
  named two CLI flags (`--ui-mode`, `--alt`) that exist at neither tag. Expect a similar residue.
  **Treat every item as a lead to verify, not a fact.**
- Do not "fix" a citation by shifting it. A previous renumber-by-uniform-shift pass introduced errors
  at 15% while looking verified. Re-resolve the line by reading the file at the named tag — and
  **never write "identical at both tags"**: the repair pass found ~25 citations quoting a v0.84.1
  offset while asserting it held at v0.83.0, including one on the highest-ranked item in the backlog.
  Byte-identical bodies do not imply identical line numbers, and the shift is often non-uniform
  within a single file.
- The count is a floor (blind spot 1) *and* contains known duplication (see Contents). Do the
  duplicate reduction before a plan books the same fix twice.
