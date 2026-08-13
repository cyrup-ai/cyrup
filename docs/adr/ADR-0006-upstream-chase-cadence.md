# ADR-0006 — Pin to the latest upstream **tag**, re-baseline on the tag event, never on HEAD

**Status** accepted (decided by default under the parity rule — overridable)
**Date** 2026-08-13
**Decides** OQ-4 (`docs/PARITY-PLAN.md:1437-1443`), and answers §6 rows 1 and 13 (`:1381`, `:1393`)
**Blocks released** the existence and position of a rebase batch; the ordering of batches 18 and
24-26; the scope (not severity) of all **74** `upstream-drift` rows across the twelve area tables;
and `SEAM-058`'s tracker status. Adds two mechanical items to batch 3.

---

## Context

### The question as posed rests on a false premise

OQ-4 asks whether cyrup chases the four moving upstreams **before or after** the port bugs. Measured
this pass, against both trees: for all four upstreams the window from the ported baseline to the
**latest tag has already been chased.** There is no "before or after" left to sequence.

| upstream | ported baseline | latest **tag** | window measured? | outcome |
|---|---|---|---|---|
| `pi` | v0.83.0 | v0.84.1 | yes — 627 files, +52 291 / −17 556 | filed: area 12 (34 rows) + drift rows in areas 01-08 |
| `pi-subagents` | ≈v0.43.0 | v0.47.1 | yes — 96 commits, 67 files, all 12 net-new source files read (`00-residual-ledger.md:305-312`) | filed: `SUBA-044`, `SUBA-050`…`SUBA-060`, `SUBA-065`, `SUBA-066` — **17** `upstream-drift` rows in area 09 |
| `pi-permission-system` | v0.7.1 *(recorded)* / **v0.8.0** *(actual)* | v0.8.0 | yes — read per shipping file (`10-cyrup-permission-system.md:37`) | **absorbed into the code.** All five behavioural v0.8.0 changes ported. **Zero** drift rows |
| `pi-intercom` | v0.9.2 | v0.10.1 | yes — 24 files, 14 commits, all accounted (`00-residual-ledger.md:314-320`) | filed: `ICOM-035`…`ICOM-047` — **13** `upstream-drift` rows in area 11 |

Counts re-derived here, not restated: `grep -c "| upstream-drift |"` over `docs/gap-analysis/*.md`
returns **74** rows total — 19 in area 12, 17 in area 09, 13 in area 11, 25 scattered across areas
01-08. That is 16.5% of the 448-row backlog, all of it already filed, severity-bearing and owned by a
batch.

So the plan's word "frozen" describes a state that does not exist. What the plan calls a freeze is:
*the drift has been measured and filed, and the items are not worked yet.* "The items are not worked
yet" is the port backlog. It is not an upstream-chase question.

### What is genuinely unmeasured, and why it is unmeasurable

Exactly two windows, both **untagged**:

```
$ git -C pi describe --tags                     → v0.84.1-117-g581d75a89
$ git -C pi-subagents describe --tags           → v0.47.1-14-g9e9fd13
$ git -C pi-permission-system describe --tags   → v0.8.0        (0 commits past)
$ git -C pi-intercom describe --tags            → v0.10.1       (0 commits past)
```

`pi` v0.84.1..HEAD is 161 files, +9 223 / −4 529. `pi-subagents` v0.47.1..HEAD is 14 commits.

Neither can be filed against, and the reason is a **stated project constraint**, not effort. A
classification turns on which side of the *ported tag* a symbol landed, and a commit hash cannot
answer that (`docs/gap-analysis/README.md:88-90`; structural defect H at
`00-residual-ledger.md:588-600`: *"a commit hash is not evidence of a classification"*, after seven of
nine hash-only items were re-derived and **six** proved misfiled). Rebasing onto pi HEAD `581d75a89`
is therefore not an option that exists. It is not a large option; it is an option the project's own
evidence rule forbids.

### The evidence FOR the plan's freeze, re-checked — one third of it does not hold

**Holds.** `CFG-012` is genuine reverse drift. `deepMergeObjects` / `isMergeableObject` return
**nothing** at `git grep v0.83.0 -- packages` and land at
`v0.84.1:packages/coding-agent/src/core/settings-manager.ts:139-164`, still present at HEAD (3 hits).
cyrup's `deep_merge` at `crates/cyrup-config/src/settings.rs:475-491` is that recursive shape already.
Fixing cyrup toward the retired v0.83.0 spread would have been a regression. Confirmed.

**Does not hold — `PROV-033` is not drift in either direction.** §5 cites *"pi deleted
`sendSessionIdHeader`"* as reverse drift. It was deleted at **v0.80.7** — three minors *before* the
ported baseline. `git show v0.83.0:packages/ai/CHANGELOG.md` puts the removal entry at `:168` under
the `## [0.80.7] - 2026-07-14` heading at `:164`, and `SessionAffinityFormat` is already the
three-valued union at `v0.83.0:packages/ai/src/types.ts:109`. cyrup still branches on
`compat.send_session_id_header` at `crates/cyrup-provider/src/api/openai_responses.rs:441`. Area 01
already has the kind right — `stale-port`, `01-cyrup-core-and-provider.md:148`. The item is fine; §5's
use of it as a drift argument is not, and it is doing rhetorical work there.

**Does not hold — "twelve out, and *zero* in the other direction"** (`00-residual-ledger.md:323-325`).
Both halves are wrong.

- The ledger names **fourteen** items out, not twelve: `PROV-021`, `PROV-023`, `PROV-024`, `PROV-025`,
  `SUBA-017`, `SUBA-021`, `SUBA-022`, `DRIFT-014` from the first pass (`:325-326`), plus `DRIFT-016`,
  `DRIFT-018`, `DRIFT-019`, `DRIFT-030`, `DRIFT-031`, `DRIFT-032` from the repair pass (`:327-334`).
- At least **two moved IN**, by the same tag-accurate re-measurement: `CFG-021` moved `not-ported` →
  `upstream-drift` (`05-cyrup-config-and-resources.md:69`, and it sits in the open table at `:152` with
  kind `upstream-drift`), and `CFG-034` was closed with its kind corrected to `upstream-drift`
  (`:82`). Verified two-sided: `tuiMode` and `fullscreenScrollbar` return **nothing** from
  `git grep -n "tuiMode\|fullscreenScrollbar" v0.83.0 -- packages/coding-agent/src` and are present at
  `v0.84.1:packages/coding-agent/src/core/settings-manager.ts:135-136`.

The **direction** of the asymmetry survives — 14 out against ≥2 in is still lopsided, and it still
says HEAD-relative measurement over-reports lag. The word "zero" does not survive, and a decision
built on "zero" would be built on a number this ADR just falsified.

### Two load-bearing figures in the plan are wrong

**"358 commits"** (`PARITY-PLAN.md:38` and `:1381`). The real window is **96**:

```
$ git -C pi-subagents rev-list --count v0.43.0..v0.47.1   → 96
$ git -C pi-subagents rev-list --count v0.43.0..HEAD      → 110
$ git -C pi-subagents rev-list --count --no-merges v0.43.0..HEAD → 99
```

The ledger already says so in prose — *"96 non-merge commits, 67 files, +4 696/−769"*
(`00-residual-ledger.md:307-308`). 358 is a 3.7× overstatement, and it is the single number carrying
§6 row 1's argument against rebasing pi-subagents. (The nearest real figure is
`v0.38.0..v0.47.1` = 353 — five minors below the actual baseline. It appears to be a wrong-tag
artefact.)

**"`8902b4f` … read against v0.43.0"** as the reason pi-subagents is frozen (`PARITY-PLAN.md:38`).
`8902b4f` is a **cyrup** commit, not an upstream one:

```
$ git -C pi-subagents cat-file -t 8902b4f  → fatal: Not a valid object name
$ git -C pi           cat-file -t 8902b4f  → fatal: Not a valid object name
$ git -C cyrup        log -1 8902b4f
  8902b4fb565e54e501326ed664750192f8f535d9  Tue Aug 11 20:00:43 2026
  feat(parity): port watchdog/, missions/ and tui/fleet* (batch 10)   [47 files, +34 231 / −74]
```

Batch 18 is a **port-side** audit of 34 337 lines of cyrup code against pi-subagents v0.43.0. It has
no bearing on which upstream tag cyrup compares against. pi-subagents' pinning must not be justified
by it — the actual reason is that its HEAD is untagged.

### The evidence AGAINST freezing, re-checked — it holds, and it is cheaper than stated

The assignment's counter-argument is that two of four upstreams are nearly current so freezing them
buys nothing. Measured: it is stronger than that. Both have **zero commits past their latest tag** —
their HEAD *is* the tag. `pi-permission-system` HEAD `9affcc9` == v0.8.0; `pi-intercom` HEAD `30dcbdd`
== v0.10.1. Neither is "moving". For those two the plan's table of record is simply stale:
`PARITY-PLAN.md:39` and `docs/gap-analysis/README.md:80` record the permission-system ported baseline
as **v0.7.1** while area 10 read the whole surface at **v0.8.0** (`10-…:453`, *"every `export` /
module-level const / `process.env` reference across all 24 upstream `src/*.ts` files at v0.8.0"*) and
confirmed all five behavioural v0.8.0 changes ported (`10-…:37`). The recorded baseline is a fiction
that area 10 already ignored.

That is the exact defect the analysis has now made twice. `README.md:203-207` records it: the intercom
baseline was recorded as v0.7.0 when a citation census over `crates/cyrup-intercom/src` returns
v0.9.2 × 272 against v0.7.0 × 14 — and the consequence was **six items parked in "version lag" (out of
scope until the next bump) that were in-baseline port bugs.** A wrong recorded baseline does not fail
loudly. It silently reclassifies work as not-yet-due.

### The cost of one re-baseline, measured rather than asserted

The mechanical half — intersect the new window's file list with the docs' citation set — was run this
pass for pi v0.84.1..HEAD:

- 161 files in the window; **56** match a `dir/file.ts` two-segment path already cited in
  `docs/gap-analysis/*.md`; **83** match on bare basename (the loose bound — `index.ts` / `types.ts`
  match trivially). That is the re-anchor worklist, computable in seconds.
- Tag-anchored citations in the fifteen documents plus the plan: v0.83.0 × 564, v0.84.1 × 530,
  v0.43.0 × 116, v0.47.1 × 89, v0.9.2 × 93, v0.10.1 × 71, v0.8.0 × 66, v0.7.1 × 32. A pi re-baseline
  touches ~1 094 of them.

The judgement half — the tag-to-tag read — has three precedents with a measurable yield: v0.7.1..v0.8.0
(28 files, 9 commits) → 0 items; v0.9.2..v0.10.1 (24 files, 14 commits) → 13 items; v0.43.0..v0.47.1
(67 src files, 96 commits) → ~17 items. Roughly one item per 2-5 changed source files. A pi *minor*
(627 files at v0.83.0..v0.84.1) is a batch. A satellite minor is a day.

### The one tool for this is broken in this workspace

`.workflows/check-citations.py` is the existing re-anchor tool. Line 23 is
`WS = pathlib.Path("/home/d0m17bw/workspace")`, which does not exist here (`ls` → `No such file or
directory`). Its own docstring at `:11-13` states the requirement this ADR formalises: *"a BARE
`index.ts:1447` is correct when the citing code ports v0.7.1 and wrong when it ports v0.8.0 … Citations
should name their tag."* It also only checks crate doc-comments against `src/<file>` and only proves a
citation is past-EOF. `crates/xtask` does not exist either — `crates/` has 18 members, none named
xtask, as batch 3 already states.

### The standing escalation trigger — measured this pass, and it has NOT fired

`PARITY-PLAN.md:1363-1364`: *"the moment pi's `main()` references `experimentalCli`, `SEAM-058` stops
being a tracker."* At pi HEAD `581d75a89`, `experimentalCli` is defined at
`packages/coding-agent/src/cli/experimental/cli.ts:7` and the only importer anywhere is
`packages/coding-agent/test/experimental-cli-command.test.ts:2`. **Nothing under `src/` imports it** —
`git grep -rn "experimental/cli" HEAD -- packages/coding-agent/src` returns empty, at v0.84.1 and at
HEAD alike. `SEAM-058` stays a tracker (`08-cyrup-session-svc-and-modes.md:131`, `:149`).

---

## Decision

**Pin each upstream to its latest *tag*. Re-baseline on the tag event, never on a commit. Split the
one "baseline" field into three fields that move on three different triggers.**

### 1. Three tags per upstream, recorded separately

Every upstream carries three, and conflating them is what produced both baseline errors:

| field | what it means | what moves it | citable? |
|---|---|---|---|
| **ported baseline** | what cyrup's code actually implements | the items closing a window **landing** | yes — this is the tag an `upstream-drift` / `not-ported` / `stale-port` kind is decided against |
| **comparison tag** | what the docs cite upstream at | upstream cutting a **new tag** | yes — every citation names it |
| **upstream HEAD** | where `main` is | every push | **no** — cited for nothing; watched only by the symbol watch list |

Establish the ported baseline by **census, never by inheritance**: count in-tree `vX.Y.Z` citations per
crate (`README.md:211-212`). That is how intercom's was found to be v0.9.2, and it is the check that
would have caught permission-system's v0.7.1 row.

### 2. Re-baseline these two **today**; it is bookkeeping, and it is free

Both have HEAD == latest tag, 0 commits past.

- **`pi-permission-system`: record the ported baseline as v0.8.0, not v0.7.1.** All five behavioural
  v0.8.0 changes are ported (`10-…:37`); area 10 already reads upstream at v0.8.0. Zero items move,
  zero severities change, zero citations re-anchor (area 10 contains **0** occurrences of `v0.84.1`
  and cites `pi-permission-system` at v0.8.0 throughout). Comparison tag = ported baseline = v0.8.0.
  This upstream is **done**, not frozen.
- **`pi-intercom`: record the comparison tag as v0.10.1** — which it already is in practice, since
  `ICOM-035`…`ICOM-047` were filed against it. The **ported** baseline stays v0.9.2 and moves to
  v0.10.1 when those 13 items land in batches 24-26. Nothing re-orders; batches 24-26 keep their
  positions and their content.

The plan's "partial concession" option is therefore not a concession and not a middle path. It is a
correction to two wrong rows, it costs two table edits, and holding it back for 26 batches only
preserves the error.

### 3. `pi` stays at v0.84.1 and `pi-subagents` at v0.47.1 — *pinned*, not frozen

Not because rebasing is expensive. Because **there is no newer tag to pin to.** `git describe` returns
`v0.84.1-117-g581d75a89` and `v0.47.1-14-g9e9fd13`; an untagged commit cannot answer "which side of the
ported tag did this land on", so items in those windows are unfileable by the project's own rule. When
pi cuts v0.85.0, that is the event. Not batch 26. Not a date.

Delete the justification *"frozen until batches 18/21/22 read `8902b4f` against v0.43.0"* from
`PARITY-PLAN.md:38` — `8902b4f` is a cyrup commit and batch 18 is a port-side audit. Batch 18 keeps
its position and its content; it just is not an upstream-tag argument.

### 4. There is no rebase batch. There is a **re-baseline procedure**, triggered by an event

Six steps. Run by whoever opens the first batch that names a file in the changed set — not by a sweep
team. The ledger's own §6 row 15 gives the reason: *"a sweep run by someone who is not about to edit
the code files items and fixes nothing"*, and this backlog already went 117 closed against 207 filed.

1. **Watch.** At the top of every batch that names an upstream file, and weekly regardless:

   ```sh
   for r in pi pi-subagents pi-permission-system pi-intercom; do
     git -C "$WS/$r" fetch --tags --quiet
     latest=$(git -C "$WS/$r" describe --tags --abbrev=0)
     echo "$r  recorded=<comparison-tag>  latest=$latest  \
   commits=$(git -C "$WS/$r" rev-list --count "<comparison-tag>..$latest")"
   done
   ```

   Plus the **symbol watch list** — greps over upstream HEAD that force an out-of-band read even with
   no new tag. It has exactly one entry today, already written into the plan: `experimentalCli`
   referenced from anything under `pi/packages/coding-agent/src/`. Verdict as of `581d75a89`:
   **not fired**. Add an entry whenever an item's Fix says "track, do not build".

2. **Trigger.** A new tag on any of the four. Only that. `git rev-list --count <recorded>..<latest>`
   going non-zero is *information*; a new tag is the *trigger*.

3. **Census before trusting.** Count in-tree `vX.Y.Z` citations for the affected crate before writing
   any new baseline number. Do not inherit the previous row.

4. **Compute the re-anchor worklist mechanically.** Intersect `git diff --name-only <old>..<new>` with
   the file paths cited in `docs/gap-analysis/*.md`. Report both the two-segment (strict) and basename
   (loose) counts — this pass they were 56 and 83 of 161 for pi's unfiled window. That set, and only
   that set, needs citations re-resolved.

5. **Re-anchor by ADDING, never by rewriting.** A re-baseline does **not** convert v0.83.0 offsets into
   v0.85.0 offsets. It adds the new tag's offsets alongside, each labelled with the tag it was read at,
   re-resolved by opening the file. Never write "identical at both tags" — the `agent-loop.ts` shift is
   documented as non-uniform (`00-residual-ledger.md:580-583`). ~25 citations already quote one tag's
   offset while asserting another's.

6. **File only what the diff shows**, then move the **comparison** tag. The **ported** baseline moves
   only when the items close.

### 5. Reclassify what `upstream-drift` means

The 74 `upstream-drift` rows are **ordinary parity work at their filed severity**, scheduled in their
owning batch. `upstream-drift` names *when a behaviour landed upstream*. It never means "out of scope
until the next bump" — that reading is precisely what parked six in-baseline intercom port bugs
(`README.md:203-207`). Under the no-accepted-divergence rule there is no scope difference between a
`parity-bug` and an `upstream-drift` row of equal severity; there is only a difference in which tag you
cite when you fix it.

---

## Consequences

**Batch by batch.**

- **Batch 2** (force the decisions): OQ-4 is answered here. It no longer blocks.
- **Batch 3** gains two mechanical items, both prerequisites for any future re-baseline:
  (a) fix `.workflows/check-citations.py:23`'s hardcoded `WS = /home/d0m17bw/workspace` to take the
  workspace root from `argv`/`$CYRUP_WS`, and widen it to lint `docs/gap-analysis/*.md` — area 05
  already proposes exactly this as PROV-041's mechanical fix (`00-residual-ledger.md:584-586`);
  (b) add `cargo xtask upstream-watch` to the `crates/xtask` batch 3 creates, implementing step 1
  above and exiting non-zero when any of the four has a tag newer than its recorded comparison tag.
  **Batch 3 is where five ADRs in this batch collide** — it also gains `cargo xtask lint-citations`
  (ADR-0008, carrying ADR-0002's `CYRUP-DELTA` conformance check), the two
  `cargo check --target *-pc-windows-msvc` gates (ADR-0007) and the `CYRUP_SHELL` repo-guard test
  (ADR-0003 D8(2)). The consolidated list is in `docs/adr/README.md`; size batch 3 against all of it.
- **Batch 18** keeps its position, and keeps its content *as far as this ADR is concerned*. Its
  justification changes: it is a **cyrup-side** audit of commit `8902b4f`'s 34 337 lines against
  pi-subagents v0.43.0, and it is not evidence about the upstream tag. **One task does leave the
  batch, by another decision:** `docs/adr/ADR-0004-agent-harness-scope.md` discharges batch 18's
  "measure `pi packages/agent/src/harness/**`" obligation (`PARITY-PLAN.md:879`) in full, so that
  line is struck rather than re-run. Nothing in this ADR restores it, and the two changes are
  independent — ADR-0004 removes a task, this ADR corrects a justification.
- **Batches 21-22** unchanged. The 17 area-09 drift rows are ordinary work at their filed severity.
- **Batches 24-26** unchanged and unre-ordered. The 13 ICOM drift rows *are* the work, as the plan
  already says; when they land, intercom's ported baseline becomes v0.10.1.
- **After batch 26**: the plan's scheduled re-baseline batch is **deleted**. It is replaced by step 1's
  watch, which runs continuously and fires on an event. If pi has cut a tag by then, the procedure runs
  because of the tag, not because of the batch number.
- **`SEAM-058`** stays a tracker, excluded from the item count. Re-verify the trigger at every watch
  run.

**Ledger changes — gap items whose severity, kind or scope this decision changes.** No severity moves.

- All **74** `upstream-drift` rows: **scope** clarified — ordinary parity work at filed severity in the
  owning batch, never "deferred until the next bump".
- `PROV-033`: **kind confirmed `stale-port`, not drift.** Removal landed at v0.80.7, three minors
  before the ported baseline. Area 01 is already correct at `:148`; `PARITY-PLAN.md:1338-1340` must
  stop citing it as a reverse-drift example.
- `CFG-012`: stays **superseded** — this is the sound reverse-drift example, and the only one.
- `CFG-021` / `CFG-034`: kind `upstream-drift` **confirmed** two-sided. They are the counter-examples
  to "zero in the other direction" and must be named wherever that claim is repeated.
- `SEAM-058`: stays **tracker**; escalation trigger re-verified as not fired at `581d75a89`.
- `ICOM-012` (the `crates/cyrup-intercom/src/lib.rs:2` v0.6.0 banner): now also the **ported-baseline
  census artefact** — it is the string a future census will trip over. Fix it in batch 24 with the
  correct value v0.9.2, and again to v0.10.1 when batches 24-26 close.

**Document corrections implied** (a later cross-link phase applies them; this ADR touches no other
file):

- `PARITY-PLAN.md:38` — "358 commits" → **96** (110 to HEAD); delete the `8902b4f` justification.
- `PARITY-PLAN.md:39` and `docs/gap-analysis/README.md:80` — permission-system ported baseline
  v0.7.1 → **v0.8.0**.
- `PARITY-PLAN.md:1338-1340` — replace the `sendSessionIdHeader` reverse-drift example with `CFG-012`.
- `00-residual-ledger.md:323-325` — "twelve items out … and **zero** in" → **fourteen named out, at
  least two in (`CFG-021`, `CFG-034`)**; the asymmetry stands, the "zero" does not.
- `docs/gap-analysis/README.md:75-82` — split the "cyrup ported baseline" column into **ported
  baseline / comparison tag / upstream HEAD**.

---

## Rejected alternatives

**Rebase onto pi HEAD `581d75a89` now (chase first).** Cost: it is not expensive, it is
*self-defeating*. There is no tag to cite, so every item filed in the 117-commit window is unfileable
under `README.md:88-90` and structural defect H, and HEAD-relative measurement is exactly what got the
six recovered `DRIFT-0xx` items wrong on the first pass. It would also burn the 56-83 file re-anchor
against a target that moves the next day.

**Freeze all four for the plan's duration, one re-baseline after batch 26** (the plan's position).
Cost: it keeps two provably-wrong baseline rows in the table of record for ~26 batches. That defect has
already cost six items misfiled as lag once, and it fails silently by construction. And "after batch
26" is a batch number, not an event — if pi cuts v0.85.0 during batch 4, the position has no answer at
all, which is how the analysis went stale twice.

**A scheduled rebase batch at position 3.** Cost: two of the four have literally nothing to do (0
commits past tag) and the other two *cannot* be done (no newer tag exists). A batch whose work is
"wait for upstream to tag" is not a batch, and scheduling it teaches the next reader that re-baselining
is calendar-driven — the exact belief this ADR removes.

**Track pi `main` continuously (the mergeability goal).** Cost: it is a legitimate goal and it is
incompatible with a stated project constraint, not merely expensive. A classification must cite a named
tag; pi `main` is not a release, so "1:1 parity with HEAD" has no verifiable meaning and
`upstream-drift` stops being a kind. Concretely, it would have had cyrup "fix" toward v0.83.0's
single-level settings spread months before v0.84.1 adopted cyrup's recursive merge —
`settings-manager.ts:139-164` is the receipt.

**Keep one "baseline" field per upstream instead of three.** Cost: this is the defect itself. One
field cannot be simultaneously true of the code (v0.8.0 for permission-system) and of the docs
(v0.7.1), and when it disagrees it silently reclassifies in-baseline port bugs as not-yet-due. Two
recorded baselines were wrong the last time this was tried.

---

## How to reverse this

> *"Staying mergeable with pi `main` matters more than being correct for today's users — track HEAD."*

For that to be executable, three things must change first, in this order: the named-tag citation rule
(`README.md:88-90` and structural defect H at `00-residual-ledger.md:588-600`) is repealed, because
tracking HEAD and citing tags are incompatible; `upstream-drift` is retired as a kind, because it is
defined relative to a tag; and the ~1 094 tag-anchored pi citations across the fifteen documents become
commit-anchored — the anchoring that the repair pass measured as producing misfiled items **six times
in nine**. A narrower reversal that needs none of that: *"re-baseline `pi` the moment v0.85.0 is cut,
as a scheduled batch rather than by whoever opens the next batch"* — that changes only step 1's owner,
and the rest of the mechanism stands.
