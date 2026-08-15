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
> **RECONCILED 2026-08-14 (second edition) against cyrup HEAD `bdcb0d0`.** **Six** whole-backlog
> sweeps have now landed. The first edition of this block reconciled sweeps 1-2; **sweeps 3, 4, 5 and
> 6 ran before any doc writer did**, and this edition reconciles them. Every count below is
> re-derived from the twelve `## Open items` tables, nothing carried forward.
>
> **Open set: 153 work items — 0 critical, 2 high, 63 medium, 88 low** (was 173 = 0/3/75/95), plus
> **10 `tracker` rows** excluded from the count as always (`PERM-017` was re-classified this
> edition). **338 of 500 rows now carry a closure marker.** Eight rows are new since the first
> edition and **four were filed AND closed in the same pass** — `TOOL-042`, `EXT-M01`, `EXT-M02`,
> `PERM-033` — which is what a *hunting* sweep produces. `TUI-062` was filed and partially closed;
> `CFG-052`, `CFG-053` and `ICOM-053` were filed open. **No ID was renumbered, merged or deleted.**
>
> **The two remaining highs are `SESS-040` and `PROV-047`.** `SEAM-061`, ranked #1 for two editions,
> **is closed as REFUTED — it was already landed at HEAD in both crates.** Areas 08, 09 and 10 now
> have zero open criticals and zero open highs between them.
>
> **The measured error rate is unchanged at ≈12% (≈53 refutations across ~430 items worked), but the
> failure mode has shifted, and this is the finding to act on.** Sweep 6 recorded ~39
> `refuted-not-fixed` outcomes; **roughly 32 of them were DOC STALENESS, not analysis error** — the
> fix had landed in a sweep between 3 and 5 and no writer had reconciled it. In `06-cyrup-ext.md`,
> eighteen rows read "still open" in one table while the *same file's* `## Open items` table had
> already marked them CLOSED. **Reconcile documentation every sweep, not every four.** The remaining
> ~7 are genuine analysis errors, including three fabricated citations and one refutation that was
> itself wrong (`PERM-008`).
>
> **ORCHESTRATION, recorded for whoever runs sweep 7:** per-crate partitioning **stalled at sweep 4,
> which landed 15 items**, because an agent owning the crate where a defect is *observed* rarely owns
> the crate where the fix *lands*. **Sweep 5 repartitioned by FEATURE — each agent owning every crate
> its feature needs — and landed all five assigned items**; sweep 6 kept that shape and landed ~15,
> including `PROV-011`, which five consecutive provider-side re-verifications had called "clean"
> because both of its remaining defects were plumbing frames in the middle. **Route sweep 7 by FIX
> SITE, not by area number**: not one open row in `04-cyrup-tools.md` has a fix site inside
> `crates/cyrup-tools/**` any more, and `07-cyrup-tui.md` now carries a routing table of eleven
> foreign-filed rows that land in it.
>
> **The test architecture changed**: 310 integration binaries → **6 + 8 gated** behind the
> `cyrup-it` harness crate, gate now at **6699 tests, 7 skipped, in 16.3 s** (was 6440 in 16.4 s).
> Every `crates/<crate>/tests/<x>.rs` citation in this directory is stale unless it names `cyrup-it`
> — **and `cyrup-it` is `required-features = ["it"]`, so the gate does not build or run it**
> (structural defect J, now filed in its own right as `ICOM-053`; it also blocks `EXT-025` and hides
> an un-instantiated 0.7 `HOST_WORLD` guest fixture).
>
> *Superseded first-edition figures, retained for provenance:*

> **RECONCILED 2026-08-14 (first edition) against cyrup HEAD `380c713`.** Two whole-backlog parity sweeps have
> landed — **sweep 1, 232 items across 11 crates, plus sweep 2** — and every count in this file has
> been re-derived from the twelve `## Open items` tables. **Open set: 173 work items — 0 critical,
> 3 high, 75 medium, 95 low**, plus **9 `tracker` rows** excluded from the count as always. **290 rows
> moved to closed**; eight rows are new (`PROV-053`, `AGENT-034`, `AGENT-035`, `SESS-045`…`SESS-048`
> filed and closed on arrival, `EXT-060` filed open). **No ID was renumbered, merged or deleted** — a
> refuted item keeps its ID with the refutation recorded in its row. Start at
> [`00-residual-ledger.md`](00-residual-ledger.md), whose top section is the reconciliation.
>
> ~~**The three remaining highs are `SEAM-061`, `SESS-040` and `PROV-047`**, and all three are blocked
> on coordination across two or more crates rather than on analysis.~~ **Superseded: two highs, see above.**
>
> **The analysis's own error rate is now measured: ≈12%.** Sweep 1 refuted 31 of ~290 items it
> worked; sweep 2 recorded 16 further `refuted-not-fixed` outcomes plus about a dozen in-body factual
> corrections. **Refuting is a success, not a shortfall** — but it means a status in this directory is
> evidence, not fact, until it has been re-read at HEAD. See the ledger's error-rate section.
>
> **The test architecture changed**: 310 integration binaries → **6 + 8 gated** behind a new
> `cyrup-it` harness crate, gate at ~~**6440 tests in 16.4 s**~~ **6699 tests in 16.3 s (2026-08-14)**. Every `crates/<crate>/tests/<x>.rs`
> citation in this directory is stale unless it names `cyrup-it` — **and `cyrup-it` is
> `required-features = ["it"]`, so the gate does not build or run it** (structural defect J).
>
> *The superseded figures, retained for provenance: 458 / 5 / 29 after the 2026-08-13 repro pass;
> 448 / 6 / 22 at the 2026-08-12 re-baseline. Neither matches what the tables actually contained
> (463) — see the ledger.*
>
> **Amended 2026-08-13 by [`REPRO-LOG.md`](REPRO-LOG.md), the first pass that ran the binary.**
> Seventeen items were driven through a real pty or headless: **16 CONFIRMED, 1 REFUTED, 0 BLOCKED**
> — but only **3 of 17 survived unchanged**. Ten items were corrected in place and carry an
> `observed 2026-08-13` marker; ten new items were filed from behaviour the binary was *seen* doing.
> Severity movements: `AGENT-020` **critical → low** (its Impact was refuted — measured 5/5, no
> message lost), `TUI-016` and `TUI-045` **medium → high**, and four new highs in area 07.
> **The medium/low split is not re-derived here** — that arithmetic predates this pass and two
> passes have filed items since; recount from the tables before planning against those two numbers.
> The critical and high figures are current.
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
| [`../adr/README.md`](../adr/README.md) | **decisions of record — where the nine open questions of `PARITY-PLAN.md` §7 were settled (eleven ADRs), plus the ledger changes those decisions imply** | — | — | — |
| [`PARITY-GAPS.md`](PARITY-GAPS.md) | **the same items grouped by gap class — read first.** Its §0 census and §0a above-medium table are **superseded 2026-08-14** (they enumerate the 448-item set); the class taxonomy, the per-entry fix sketches and §7 Method are current | — | — | — |
| [`REPRO-LOG.md`](REPRO-LOG.md) | **the first execution of this binary — 17 items driven through a real pty or headless, 16 confirmed / 1 refuted / 0 blocked, plus the real suite numbers (6387, not the inherited 3932) and 9 new items filed from what the binary was seen doing.** Every row carries a transcript. **Read this before trusting a severity: only 3 of 17 items survived a live run unchanged.** | — | — | — |
| [`00-residual-ledger.md`](00-residual-ledger.md) | ranked cross-cutting view | — | — | — |
| [`01-cyrup-core-and-provider.md`](01-cyrup-core-and-provider.md) | wire APIs, providers, auth, streaming, catalogs, cost | **11** | 0 | 1 |
| [`02-cyrup-agent.md`](02-cyrup-agent.md) | the turn loop, tool dispatch, hooks, abort | **2** | 0 | 0 |
| [`03-cyrup-session.md`](03-cyrup-session.md) | JSONL session tree, compaction, system prompt | **8** | 0 | 1 |
| [`04-cyrup-tools.md`](04-cyrup-tools.md) | the built-in tool set | **5** | 0 | 0 |
| [`05-cyrup-config-and-resources.md`](05-cyrup-config-and-resources.md) | settings, model resolution, trust, skills, packages | **12** | 0 | 0 |
| [`06-cyrup-ext.md`](06-cyrup-ext.md) | extension host, WIT world, event catalog | **24** | 0 | 0 |
| [`07-cyrup-tui.md`](07-cyrup-tui.md) | terminal UI application layer | **35** | 0 | 0 |
| [`08-cyrup-session-svc-and-modes.md`](08-cyrup-session-svc-and-modes.md) | the integration seam, RPC, CLI, print/json modes | **7** | 0 | 0 |
| [`09-cyrup-ext-subagents.md`](09-cyrup-ext-subagents.md) | subagent delegation | **20** | 0 | 0 |
| [`10-cyrup-permission-system.md`](10-cyrup-permission-system.md) | allow / ask / deny gate | **4** | 0 | 0 |
| [`11-cyrup-intercom.md`](11-cyrup-intercom.md) | supervisor↔subagent broker | **9** | 0 | 0 |
| [`12-upstream-drift-pi-core.md`](12-upstream-drift-pi-core.md) | pi core drift since the ported baseline | **16** | 0 | 0 |
| | **total** | **153** | **0** | **2** |

Counts are the `## Open items` table of each file, re-derived 2026-08-14 (second edition, after
sweeps 3-6). **Every file now carries exactly one such table** — area 03's second table was the last
one and was folded in during the repair pass — so a single enumeration is complete. **Ten** `tracker`
rows sit in those tables (or, in areas 08 and 09, in a separate `## Trackers` table) and are
deliberately outside the arithmetic: one each in areas 01, 02, 03, 08, 09 and **10** (`PERM-017`,
re-classified 2026-08-14), and four in area 12.

**A count in this table is a floor for a second reason as of this edition: eighteen rows in
`06-cyrup-ext.md`, six in `09-cyrup-ext-subagents.md` and five in `11-cyrup-intercom.md` were found by
sweep 6 to be CLOSED at HEAD while a *second* table in the same file still called them open.** Where a
file carries both a `## Status of every item from prior analyses` table and an `## Open items` table,
**only the latter is counted, and only the latter was kept current between reconciliations.** Read
both before quoting either.

**Every one of these is a floor, not a total** — see blind spot 1. It is also not a clean total in
the other direction: area 12 marks **16 of its 30** rows `duplicate-of` an item another area owns
(**2026-08-14: 14 of those 30 rows are now closed, so the duplication census needs re-running before
any deduplicated figure is quoted against 173**), so
432 was the largest deduplicated figure anyone had actually computed against the old set, and the ledger's F4 cluster
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
registrations, `wasm-host` being default-on, and the out-of-scope pi packages. **One trap was
removed on 2026-08-13: "the deliberately unreachable first-run wizard".** It was not downgraded, it
was **wrong** — `is_official_distribution()` is a compile-time `true` for this build, the gate was
measured firing on a live pty, and the wizard was a complete, unit-tested port with no caller
(`UW-2`, decided by ADR-0011, wired and closed the same day). A wrong trap is worse than no trap: it
converted a real finding into a non-finding across every pass, which is the only mechanism this
project has for finding anything. The out-of-scope package list remains contested — see blind
spot 6.

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

- This is a **static** analysis **except for the seventeen items marked `observed 2026-08-13`**
  ([`REPRO-LOG.md`](REPRO-LOG.md)). For everything else: nothing was built, run, tested or
  reproduced; items are evidenced by reading both sources, not by observing behavior, and every
  `Verify` line is a design rather than an observation.
  **The repro pass measured what that costs, and the number is not reassuring.** 16 of 17 items were
  confirmed to exist — reading finds real defects — but only **3 of 17** survived a live run
  unchanged. The recurring failure is that an item recovers *what the code does* and not *what the
  user sees*: `TUI-016` was filed as an absent surface and is an affirmative wrong one; `SESS-040`
  assumed a spinner that never renders; `SEAM-063` assumed a success message that is never printed.
  In each case the verdict was right and the picture of the screen was wrong — **and the picture is
  what a fix gets written against.** Treat any unobserved item's *mechanism* as a hypothesis even
  when its *existence* is well evidenced.
- **A severity raise must cite an observation, or say plainly that it does not.** Two of the four
  `high → critical` raises made on 2026-08-12 were made on *predicted* consequences: `AGENT-020`'s
  "data loss on the normal path" was refuted by measurement (5/5 attempts, no message lost) and the
  item is now `low`, and `EXT-054`'s reassuring blast-radius note ("zero WASM guests ship") was wrong
  — the in-tree SDK guest reproduces the mis-grant in under a second. The raise procedure applied the
  severity definition to an item's own Impact prose; where that prose was a prediction, the procedure
  faithfully promoted a prediction into a rating.
- **For TUI work this is not a formality.** ratatui `TestBackend` unit tests pass while the assembled
  application has layout and empty-state bugs. No `TUI-*` item — nor `SESS-040`, nor the pre-launch
  surfaces in `SEAM-061`…`SEAM-067` — is done until it has been **run in a real terminal**.
  **Vindicated 2026-08-13.** `TUI-055` (no indicator renders for the entire 10–20 s of a compaction)
  is invisible to every static read: the source at `app.rs:4615-4639` sets the indicator and looks
  correct. Only running it shows the band never reaches the screen. `TUI-N13` is the mirror image —
  a deterministic macOS-only test failure that four passes missed because the first measurement was
  piped through `tail`. **Validate your instrument as a first-class step:** that pass produced three
  instrument errors (`tail` hiding a red, `pgrep -f` matching its own pattern and inventing 22
  orphaned processes, and `tmux display-message '#{cursor_x}'` reporting a stale hardware cursor
  while cyrup paints its caret as an SGR-7 cell).
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
