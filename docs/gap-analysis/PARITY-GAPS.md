# PARITY-GAPS

**Regenerated 2026-08-12 (repair pass) from the twelve repaired area files.** Supersedes the earlier
2026-08-12 edition, whose bookkeeping was exact and whose *content* carried four defects a
completeness critique found and this pass fixed:

1. **The severity scale was not being applied.** The previous edition's headline was "0 critical"
   while six open items met this project's own definition (`README.md` — data loss, silent wrong
   output, a permission bypass, or a crash on a normal path) **on their own text**. The definition
   was applied, not amended: `AGENT-020`, `TUI-027`, `EXT-054`, `PERM-009` were raised to critical
   — **`AGENT-020`'s raise was subsequently REFUTED by measurement on 2026-08-13 and the item is now `low`;**
   `TUI-027`, `EXT-054` and `PERM-009` were all confirmed in the shipped binary (`REPRO-LOG.md`) —
   and the repair pass filed two more (`TUI-042`, `TUI-043`). Four items were re-rated up to high
   (`TOOL-039`, `SEAM-051`, `PERM-023`, `DRIFT-049`) and one down-held rating (`TUI-019`) had its
   unverifiable-ADR justification struck.
2. **Nine items proposed no work.** They are now `tracker` rows — ID and body retained, excluded from
   the count, listed in §0.
3. **Citations were asserted tag-invariant without being read at the tag.** A sweep across the area
   files re-resolved every `@v0.83.0` / `@v0.84.1` / "identical at both tags" claim by opening the
   file at the tag. It found wrong offsets on a critical, on three highs, and on ~25 items overall.
4. **Three files contradicted each other about whether pi's catalog generator exists.** It does, at
   both tags — see §4 and OQ-5.

Two baselines were corrected in the previous edition and still hold: `pi-subagents` latest is
**v0.47.1** (not v0.43.0) and `pi-intercom` latest is **v0.10.1** (not v0.9.2); `pi-intercom`'s
**ported** baseline is **v0.9.2**, not v0.7.0 — see the §1d note.

This document is the work-facing companion to `00-residual-ledger.md`. The ledger ranks; the area
files hold the evidence; this file is organised by **gap class**, so someone doing the work sees the
shape of the remaining distance rather than a per-crate walk.

**There is no "accepted divergence" category.** The project's goal is behavioural equivalence with
the four upstreams. Mechanism may differ where the language forces it — WASM Component Model guests
where pi runs TypeScript through `jiti`; ratatui where pi hand-rolls a renderer. Port the BEHAVIOUR
and state the mechanism difference with its reason. That is not an exemption: **where a mechanism
difference costs behaviour, the entry says so and stays on the list as work.**

| | |
|---|---|
| cyrup HEAD | **`e5c6933`** — last code commit as of the third edition, 2026-08-14 (docs HEAD `0097149`, branch `david/cyrup`). *Superseded: `04c1ba2` (docs `a9000b1`) when this file was regenerated; `bdcb0d0` at the second edition.* |
| `pi` | ported baseline **v0.83.0** → latest **v0.84.1** · delta 627 files, +52 291 / −17 556 |
| `pi-subagents` | ported baseline **≈v0.43.0** (inferred — the crate records no version string) → latest **v0.47.1** · delta 151 files, +10 254 / −1 333 |
| `pi-permission-system` | ported baseline **v0.7.1** → latest **v0.8.0** · delta 28 files, +4 023 / −1 851 |
| `pi-intercom` | ported baseline **v0.9.2** *(prior docs said v0.7.0 — wrong, see §1d)* → latest **v0.10.1** · true drift window `v0.9.2..v0.10.1` = 24 files, +2 495 / −700 |

Read upstream with `git -C <repo> show <tag>:<path>`, never from a working tree — clone-HEAD line
numbers and file existence both mislead. §7 says how much of this was first-hand.

---

## 0. Census — every open item in the twelve area files, by class

> **CORRECTED 2026-08-19 against cyrup HEAD `4fb5e40`, and NOT recounted — read why.** The
> `0 critical / 5 high` headline below is false: all five of those rows closed on 2026-08-15, and the
> twelve area tables carried **three unstruck `critical` severity cells** when this correction was
> measured (`SEAM-112`, `PERM-034`, `TUI-092`) and three unstruck `high` ones (`PROV-068`, `TUI-091`,
> `SEAM-113`) — **2 + 4** after `TUI-092`'s de-escalation landed later in the same batch. See
> §0a, which tables them. **The 237/606 figures are stale by roughly a hundred rows** — five closing
> batches landed on 2026-08-15 without reconciling this file — and no replacement total is published
> here **because the twelve tables were being edited by other writers in the same batch that produced
> this correction**, which is the same reason `00-residual-ledger.md`'s fourth edition declines to
> restate area 05's count mid-batch ("*deliberately NOT restated by one slice mid-batch; recount the
> table*"). Recount from the tables, with the rule stated below, once the batch settles.
> **A second reason a recount must be run rather than adjusted:** `crates/cyrup-flux` — 9 files,
> 1 513 lines, shipped, with a spec and a rustbook chapter — had **no area file at all** until this
> batch opened [`14-cyrup-flux.md`](14-cyrup-flux.md) with 7 rows, so every figure in this file
> predates a whole shipped surface. The denominator moved, not just the numerator.
>
> **SUPERSEDED — FOURTH EDITION 2026-08-14 (after the surface enumeration). Every census below this
> block, including the third edition's, enumerates a set that no longer exists.**
>
> **Open set: 237 work items — 0 critical, 5 high, 88 medium, 144 low** (was 145 = 0/2/61/82), plus
> the same **10 `tracker` rows**. **606 rows across the twelve tables: 360 carry a full closure marker
> and 36 more a partial one — 396 of 606 (65%).** Derived row by row from the twelve `## Open items`
> tables **in the working tree** — the four surface writers' filings are not committed yet; the last
> code commit is `5990e86`. The counting rule is the third edition's, and it was validated by
> re-running it against the same twelve files at commit `e5c6933`, where it reproduces
> **503 / 349 / 35 / 145 = 0/2/61/82** exactly. **`13-cyrup-mcp.md`, `13a`–`13i` and
> `MCP-PORT-METHODOLOGY.md` are owned by another team and are excluded from every figure in this
> file** — as they always have been, though it was never written down.
>
> **The ninth pass was not a sweep. It enumerated nine finite pi SURFACES mechanically and diffed
> both directions: 191 findings — 67 missing in cyrup, 66 cyrup-original, 58 differing in shape —
> against the 10-25 a late-stage sweep produces. 93 ids were filed, 11 closed on arrival.** Five of
> the nine surfaces were walked completely; four state exactly what they could not reach. See
> `00-residual-ledger.md`'s fourth-edition block for the re-run recipe, the incomplete-surface list
> and the `cyrup-original` count. **No ID was renumbered, merged or deleted; `SEAM-087`…`SEAM-099` are
> deliberately unallocated — do not "recover" them.**
>
> **THE ABOVE-MEDIUM SET IS NOW FIVE ROWS, NOT TWO. §0a below is stale in a way that costs work.**
> Joining `PROV-047` and `SESS-040`: **`PROV-054`** (xai `grok-4.5` routed over `openai-completions`
> where pi uses `openai-responses` — and it is the xai *default* model), **`PROV-055`** (opencode's
> `sessionAffinityFormat: "openai-nosession"` missing on all 16 `openai-responses` rows, so cyrup
> leaks a `session_id` header pi suppresses) and **`PROV-056`** (kimi-coding's `forceAdaptiveThinking`
> ×3 and `allowEmptySignature` ×1 — two wire divergences per request on every model of the provider).
> **All three are catalog DATA and share ONE fix site with `PROV-057`…`PROV-059`:** they close through
> `PROV-018`/`PROV-060`'s bulk regeneration in the commit that rewrites `catalog_manifest.json`.
> **Do not schedule them individually** — that produces six agents each hand-patching one catalog row
> and each invalidating the manifest.
>
> **§6 q5 (OQ-5) IS REFUTED and is corrected in place below.** Catalog accuracy IS statically
> auditable; the "two-line re-export" premise holds only from `a9f6a3159` onward, and its direct
> parent `b0c2a90e` — cyrup's own stated provenance floor — still carries the full data literals.
> Filed as `PROV-060`.
>
> **§5's "21 `cyrup-original` items in the census" is likewise stale: the class is now 46 open rows
> (68 filed), and it has its own section in the ledger.** 66 of this pass's 191 findings are surfaces
> cyrup has and pi does not — the class through which divergence enters while everyone is looking at
> parity.
>
> *Superseded third-edition census follows.*

> **SUPERSEDED — THIRD EDITION 2026-08-14 (after sweeps 7-8). The census below enumerates a set that
> no longer exists, and so does the second-edition block beneath it.** **Eight** whole-backlog sweeps
> have landed. The current set, re-derived row by row from the twelve `## Open items` tables, is
> **145 open work items — 0 critical, 2 high, 61 medium, 82 low**, plus the same **10 `tracker`
> rows**. **503 rows across the twelve tables: 349 carry a full closure marker and 35 more a partial
> one — 384 of 503 (76%).** *(The second edition's "338 of 500" and its 63/88 medium/low split were
> derived by a different method; this edition states its counting rule in `00-residual-ledger.md` so
> the two can be compared.)*
>
> **THREE rows are new since the second edition and all three are closed: `PROV-M01` (area 01) and
> `TOOL-M01` (area 04), filed and closed in the same pass, plus `EXT-M03` (area 06), filed
> RETROACTIVELY because the ID was cited five times in `crates/cyrup-ext/src/host/live.rs` and had no
> row anywhere** — both produced by one assigned audit of
> hand-written delegating trait impls rather than by the backlog. **One row was REOPENED by
> measurement: `TOOL-042`** (area 04), whose closure rested on an argument that a 286-run experiment
> refuted for the one occurrence it instrumented. **The two highs are unchanged: `SESS-040` and
> `PROV-047`.** **No ID was renumbered, merged or deleted.**
>
> **THREE ENTRIES IN THIS FILE ARE NOW WRONG AND ARE CORRECTED HERE, because a work-facing document
> that mis-sizes an item costs a whole agent-pass:**
>
> - **`PB-10` (`turnBudget` = `SUBA-008`) IS CLOSED (sweep 8), and the second-edition note calling it
>   "the cheapest remaining medium … wiring plus a schema key, not a port" was measurably wrong.**
>   cyrup's `Usage` carried **no `turns` counter at all**, so there was no turn count to wire; the
>   port is ~888 lines of new module plus a drive-loop arm, a signal ladder, three new `SingleResult`
>   fields, a frontmatter field, a serializer arm and a config key. **And the mechanism it prescribed
>   was inverted** — the turn budget has no env handoff and no child-side enforcement, unlike the tool
>   budget it was told to mirror. See the ledger's mechanism register, entry 1.
> - **The `CFG-052` entry's premise about upstream is FALSE and the row is closed as REFUTED.** pi's
>   `parseGitUrl` returns `null` before reaching `hostedGitInfo.fromUrl` unless there is a `git:`
>   prefix or an explicit `://` (`utils/git.ts:172-179` @v0.83.0, and its own doc comment says so).
>   Upstream stores the shorthand as a local path exactly as cyrup does.
> - **`TOOL-042` is not a same-pass win.** It was filed, largely fixed, and reopened.
>
> *Second-edition block, retained for provenance:*

> **SUPERSEDED — SECOND EDITION 2026-08-14 (after sweeps 3-6). The census below enumerates a set that
> no longer exists.** **Six** whole-backlog sweeps have landed. The current set, re-derived from the
> twelve `## Open items` tables, is **153 open work items — 0 critical, 2 high, 63 medium, 88 low**,
> plus **10 `tracker` rows** (`PERM-017` re-classified 2026-08-14). **338 of 500 rows carry a closure
> marker.** Eight rows are new since the first edition, four of them filed AND closed in the same
> pass — `TOOL-042`, `EXT-M01`, `EXT-M02`, `PERM-033`; `TUI-062` was filed and partially closed, and
> `CFG-052`, `CFG-053` and `ICOM-053` were filed open. **The two remaining highs are `SESS-040` and `PROV-047`;
> `SEAM-061` closed as REFUTED (already landed at HEAD in both crates).**
>
> *First-edition figures, superseded: 173 open = 0 / 3 / 75 / 95 after sweeps 1-2, which closed 290
> rows; eight rows new (`PROV-053`, `AGENT-034`, `AGENT-035`, `SESS-045`…`SESS-048`, `EXT-060`).*
> The class *taxonomy* below is unchanged and still the right way to read the backlog; only the
> per-class counts are dead, and they have not been re-derived because the disposition is recorded
> per row in the area files rather than per class. See `00-residual-ledger.md`, top section.
>
> **Two class corrections landed that this file's §3 must absorb:** `DRIFT-013` and `DRIFT-029` were
> filed as **version lag** and are **port omissions inside the ported baseline** (`isZai` is at
> openai-completions.ts:1435 @v0.83.0; `_bashAbortControllers` is present in full at v0.83.0). With
> `DRIFT-014`/`018`/`019`/`030`/`031`/`032`, that is **eight** rows moved out of §3 by re-derivation.
> **Re-derive every remaining §3 entry at `v0.83.0` before scheduling it.**
>
> **`PB-13` is closed** with `SUBA-048`, as its own text instructed. **`PB-5`** is down to the
> subagent re-exec half only — and as of 2026-08-14 (sweep 6) **both** of its non-subagent halves are
> landed: the immediate-bash half in `cyrup-session-svc/src/bash.rs:107-109` **and the bash-TOOL half
> in `cyrup-tools/src/tools/bash.rs:154-165`**, pinned by `cyrup-tools/src/tests/bash_session_env.rs:200-221`.
> **`PB-5`'s remaining fix site is `crates/cyrup-ext-subagents/**` — route it there, not to area 04.**
> ~~**`PB-10`** (`turnBudget`) is `SUBA-008`, re-verified open at HEAD and rated the cheapest remaining
> medium in area 09: the three consumers already exist and read a hard-coded `false`, so it is wiring
> plus a schema key, not a port.~~ **CLOSED 2026-08-14 (sweep 8) with `SUBA-008`; the sizing and the
> mechanism in this sentence were both wrong — see the third-edition block above.**
> **`VL-P22`** is half-addressed: `DiskStore::rewrite`'s temp-sibling-and-rename now carries a
> `[CYRUP-DELTA]` naming pi's `_rewriteFile` (session-manager.ts:979-988) and the reason; the
> torn-tail half is untouched.
>
> **One sweep-1 doc instruction against this file is REFUTED:** it asked for line 19's `pi-intercom`
> ported baseline to be corrected from v0.7.0 to v0.9.2. The repair pass had already done it — `:26`
> and the §1d baseline table at `:44` both say **v0.9.2**. No change was needed.

**448 open work items: 6 critical, 22 high, 197 medium, 223 low.** Plus **9 `tracker` rows**, which
keep their IDs and bodies but propose no schedulable work and are deliberately outside the
arithmetic. Counted mechanically from the single `## Open items` table each area file now carries —
**all twelve have exactly one table as of this pass**; area 03's second table (`SESS-S05`) was the
last one and is gone.

Arithmetic from the previous edition: **426 + 31 filed by the repair pass − 9 reclassified as
trackers = 448.** Nothing was renumbered, merged or deleted to produce it.

| PARITY-GAPS class | area-file `Kind` values it covers | n |
|---|---|---|
| **Port bug** (§1) — upstream had it at the ported tag; cyrup does not | `not-ported` 146 + `parity-bug` 176 + `port-divergence` 1 | **323** |
| **Version lag** (§3) — landed upstream after the ported baseline | `upstream-drift` 66 | **66** |
| **Reverse lag** (§1e) — cyrup carries behaviour upstream changed or deleted | `stale-port` | **14** |
| **Test defect** — a test pinning wrong behaviour or an uncontrollable timing outcome | `test-defect` | **23** |
| **Invented surface** — behaviour with no upstream basis; delete it or justify it | `cyrup-original` | **21** |
| **Tooling** — audit/generation debt, not a user-visible gap | `tooling` | **1** |
| | | **448** |

The `tracking` kind no longer appears in the counted set: every row that carried it is now a tracker.

**§2 (unwired) is a lens, not a bucket.** Every unwired item also carries one of the kinds above; it
is called out separately because it is the project's most common defect shape and by far the cheapest
to fix. Do not add §2 to the census total.

| area | open | crit | high | med | low | trackers | closed this pass | new this pass |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| [01 core + provider](01-cyrup-core-and-provider.md) | 40 | 0 | 6 | 14 | 20 | 1 | 10 | 22 |
| [02 agent](02-cyrup-agent.md) | 26 | 1 | 1 | 6 | 18 | 1 | 3 | 14 |
| [03 session](03-cyrup-session.md) | 29 | 0 | 1 | 13 | 15 | 1 | 9 | 9 |
| [04 tools](04-cyrup-tools.md) | 29 | 0 | 1 | 10 | 18 | 0 | 14 | 11 |
| [05 config + resources](05-cyrup-config-and-resources.md) | 38 | 0 | 1 | 19 | 18 | 0 | 16 | 17 |
| [06 ext host](06-cyrup-ext.md) | 50 | 1 | 0 | 28 | 21 | 0 | 6 | 21 |
| [07 tui](07-cyrup-tui.md) | 56 | 3 | 1 | 26 | 26 | 0 | 13 | 25 |
| [08 session-svc + modes](08-cyrup-session-svc-and-modes.md) | 40 | 0 | 7 | 19 | 14 | 1 | 6 | 24 |
| [09 subagents](09-cyrup-ext-subagents.md) | 45 | 0 | 2 | 23 | 20 | 1 | 22 | 24 |
| [10 permission system](10-cyrup-permission-system.md) | 21 | 1 | 1 | 6 | 13 | 0 | 10 | 7 |
| [11 intercom](11-cyrup-intercom.md) | 44 | 0 | 0 | 22 | 22 | 0 | 3 | 24 |
| [12 pi core drift](12-upstream-drift-pi-core.md) | 30 | 0 | 1 | 11 | 18 | 4 | 5 | 9 |
| **total** | **448** | **6** | **22** | **197** | **223** | **9** | **117** | **207** |

**The nine trackers**, so nobody mistakes them for backlog: `PROV-004` (catalog field-diff coverage —
its whole Fix is `PROV-018`), `AGENT-028` + `SESS-038` (both turn on whether cyrup models pi's
v0.84.1 agent-harness — **answer them together**, OQ-7), `SEAM-058` (pi's experimental server/client
tree; escalate the moment `main()` references `experimentalCli`), `SUBA-005` (the management-verb
census — "this item is the ledger, not the work"), `DRIFT-022`, `DRIFT-023`, `DRIFT-032`,
`DRIFT-040`. Two of those — `DRIFT-023` and `DRIFT-040` — are additionally **leads, not items**:
neither side was ever re-read, and area 12 records the two commands that would settle each.

**Treat 448 as a floor.** Four area files say so explicitly with reasons: area 09 (`workflowScript`
is a whole execution model nobody has decomposed), area 11 (`broker/mod.rs` read in ranges only),
areas 05/10 (the surface sweep is the only counter to structural blind spot 1 and it is not
exhaustive), and area 08 (the inner RPC payload shapes are unswept).

**And 448 contains known duplication, so it is a floor with a soft ceiling.** The only deduplication
anyone has actually computed is area 12's: **16 of its 30 counted rows carry a `duplicate-of`** naming
the area that owns the same defect, so **432 is the largest defensible deduplicated figure today**.
The ledger's cluster F4 lists roughly twenty further defects carrying two-to-four IDs each
(`AGENT-019`/`DRIFT-039` are literally the same test); nobody has reduced that to a number, and until
someone does, no single figure here is both a floor and a total. Do the F4 reduction before any plan
books the same fix twice.

**Stable IDs are load-bearing.** `PB-N` / `UW-N` / `VL-*` ids below are never renumbered or deleted.
Where the re-audit moved an item to a different class, the id moves section and keeps its number.
`PB-32`…`PB-41` and `UW-19`/`UW-20` are new in this edition.

---

## 0a. Everything above medium, in one table

> **SUPERSEDED — FIFTH EDITION 2026-08-19, against cyrup HEAD `4fb5e40`. ALL FIVE ROWS OF THE
> FOURTH EDITION'S ABOVE-MEDIUM SET ARE CLOSED, AND THE SET THAT REPLACED THEM OPENED WITH THREE
> CRITICALS — a class the fourth edition published as empty.** Read this block before planning; the
> two below it name work that no longer exists.
>
> **Closed, each verified in its area table and, for the two the cross-cutting files kept alive, in
> the code:** `PROV-047` (`01-cyrup-core-and-provider.md`, CLOSED 2026-08-15 —
> `cyrup-session-svc/src/builder.rs:296-299`
> calls `cyrup_provider::configure_http_proxy(proxy.clone())` unconditionally, including with `None`,
> reached from `:1516`, and `crates/cyrup/src/main.rs:177` is the bootstrap call deliberately ABOVE
> the package/credential pre-dispatches that can egress before a session exists — so the "inert until
> one line lands" residual is DISCHARGED); `SESS-040` (`03-…`, REFUTED 2026-08-15 — see §2's UW-12
> entry for the dispatch chain); and `PROV-054`/`PROV-055`/`PROV-056`, all three CLOSED 2026-08-15
> in area 01, through exactly the one bulk catalog regeneration this block predicted.
>
> **The current above-medium set is SIX rows — 2 `critical` + 4 `high` — and it is entirely disjoint
> from the old one.** It opened 3 + 3 and became 2 + 4 when `TUI-092` was de-escalated inside this
> batch. Every one of the six was filed from LIVE USE on 2026-08-15 or later — the first cohort in
> this directory's history that no reading pass produced:
>
> | id | area | sev | one line |
> |---|---|---|---|
> | `SEAM-112` | 08 | **crit** | `/resume` produces a broken session: nothing renders and bash tool calls repeat endlessly — the repetition means tool results are not reaching the model, so it is at minimum a context/session-rebuild fault, not the display bug it resembles |
> | `PERM-034` | 10 | **crit** | *(renumbered from `PERM-033` on 2026-08-19 — id collision; see area 10.)* "Allow Always" does not stick — the same tool/command is re-prompted within one session, which makes the permission gate unusable. Both halves are wired (`extension.rs:1717`/`:2023` write, `:1464`/`:1634`/`:2211` read), so it is **not** the dead-seam class |
> | `TUI-092` | 07 | ~~crit~~ **high** | The TUI degrades from smooth to a total lockup. **Its severity cell was corrected `critical` → `high` inside this batch**; its own bug file's `**Severity**` header has said `high` since round 2 (`bugs/TUI-092-progressive-lockup.md`); the three clauses that justified `critical` are all false at HEAD — Ctrl+D is bound (`keymap.rs:655` → `app/input.rs:126-129` → `app/run_action.rs:16`), Ctrl+C is bound (`keymap.rs:656` → `app/input.rs:219-231`), and `TUI-088` is CLOSED |
> | `PROV-068` | 01 | high | An explicit `null` in `thinkingLevelMap` reads as UNSUPPORTED, collapsing most reasoning models to two rungs (`cyrup-provider/src/collection.rs:794-807`) |
> | `TUI-091` | 07 | high | Reasoning blocks never render although every layer is wired — and as of 2026-08-19 the row has **zero live hypotheses**; its last named candidate is refuted in the area file |
> | `SEAM-113` | 08 | high | A model chosen with `/model` does not survive into the next session |
>
> **The lesson is the one this directory keeps re-learning from the other side.** Nine reading passes
> and one nine-surface enumeration produced a five-row above-medium set of which every row was a
> *wire or wiring* defect a reader can see; four days of live use produced three rows rated
> `critical` on arrival, none of which any reading pass had a row for. `README.md`'s caveat — "no `TUI-*` item is done until it has been run in a real
> terminal" — generalises past the TUI: **the above-medium set is the part of this backlog a static
> method is worst at populating.**

> **SUPERSEDED — SECOND EDITION 2026-08-14 (after sweeps 3-6). The current above-medium set is TWO
> rows — `SESS-040` and `PROV-047` — tabled at the top of `00-residual-ledger.md`.** `SEAM-061`
> closed as REFUTED: sweep 6 found it already landed at HEAD in **both** crates
> (`cyrup-tui/src/session_selector.rs:154`/`:276`/`:313`/`:1918`/`:1985`; `crates/cyrup/src/main.rs:1354`
> + `startup_ui.rs:191-201`), which also retires the "one agent, both crates" coordination note that
> ranked it #1 for two editions. `PROV-030` (row 7 below) is likewise closed and was re-verified at
> HEAD by sweep 6 — `api/google_vertex.rs` is 717 lines with a real `ApiImpl::run`.
>
> *First edition, retained:* **SUPERSEDED 2026-08-14 — every row in this table is dispositioned.** All six criticals and 31 of
> the 34 highs are closed. Three of the highs (`PROV-027`, `PROV-028`, `PROV-029`) turned out to have
> been fixed before either sweep and were closed by **refutation**; four more (`SEAM-047`, `SEAM-051`,
> `SEAM-064`, `SEAM-072`) plus `DRIFT-049` had been marked fixed in their *kind* cell while their
> *severity* cell still read `high`, which is how this table published phantom highs across two
> recounts. **The current above-medium set is three rows — `SEAM-061`, `SESS-040`, `PROV-047` —
> tabled at the top of `00-residual-ledger.md`. Do not plan from the ranking below**; it is retained
> because each row is still the best one-line statement of what the work was.

A planner should not have to read six sections to find the twenty-eight items that outrank the rest.
Port bugs still rank above everything at equal severity (§1); the two `cyrup-original` highs are here
because a severity is a consequence, not a class.

| id | sev | area item | entry | effort | one line |
|---|---|---|---|---:|---|
| — | ~~crit~~ **low** | `AGENT-020` (02) | PB-25 | S | **⚠ REFUTED 2026-08-13 — no longer belongs in this table.** `continue_run` does drain before the run-active check, but the predicted loss does not occur on the normal path: typing during a live stream queued and delivered the message **5/5 times** (`REPRO-LOG.md`). Latent race only, reachable via `AGENT-030`. Severity critical → low; **this table's ranking is stale until someone re-ranks it.** |
| 2 | **crit** | `TUI-042` (07) | PB-33 | S | The undo snapshot omits the paste registry — one undo turns a `[paste #N …]` marker into 20 literal characters sent to the model |
| 3 | **crit** | `TUI-043` (07) | PB-34 | S | Word motion / Ctrl+W are not paste-marker atomic — one Ctrl+W after a large paste orphans the marker and drops the paste |
| 4 | **crit** | `TUI-027` (07) | PB-28 | M | `/tree` has no text search; typing a filter word triggers `e`, and Enter **persists** the typed text as that entry's label in the session JSONL |
| ~~5~~ | ~~**crit**~~ | `EXT-054` (06) | UW-13 | M | **FIXED 2026-08-13** — `capabilities` reaches instantiation via `load_wasm_with_caps`; enforced host-side at the import boundary; `EXT-055` (`ext-fs`) closed in the same change. Evidence in `06-cyrup-ext.md` |
| 6 | **crit** | `PERM-009` (10) | PB-32 | S | `should_expose_tool`'s cyrup-only bash branch keeps `bash` advertised under a tool-level deny **and the allow-listed command runs** |
| 7 | high | `PROV-030` (01) | PB-22 | L | `google-vertex` registered with 10 catalog models and **no wire API** — every request dies at dispatch |
| 8 | high | `PROV-027` (01) | PB-23 | S | Copilot's 9 anthropic-messages models send `x-api-key`; pi sends `Authorization: Bearer` — all unauthenticated |
| 9 | high | `PROV-028` (01) | PB-24 | S | `github-copilot-headers.ts` unported on all three routes — Copilot image turns are rejected outright |
| 10 | high | `PROV-029` (01) | UW-11 | S | Copilot + Codex login flows ship complete and unreachable; `/login` dead-ends on `LoginUnsupported` |
| 11 | high | `PROV-047` (01) | PB-35 | M | `httpProxy` reaches only the streaming wire APIs — five OAuth flows, the agent proxy and extension HTTP all bypass it |
| 12 | high | `PROV-048` (01) | PB-36 | S | A lone-surrogate `\uXXXX` escape in an SSE frame kills the whole assistant turn (and blocks resuming a pi-written session) |
| 13 | high | `AGENT-030` (02) | PB-26 | M | `AgentSession::prompt` gates on the agent's per-run flag, so a prompt in the post-run gap starts a **second** run |
| 14 | high | `SESS-040` (03) | UW-12 | M | Compaction cannot be cancelled from the shipped binary while the indicator advertises "(esc to cancel)" |
| 15 | high | `TOOL-039` (04) | §5 | S | `CYRUP_SHELL` silently redirects every model-issued `bash` call to an arbitrary interpreter; pi has no shell env var |
| 16 | high | `CFG-035` (05) | PB-27 | M | `.cyrup/SYSTEM.md` / `APPEND_SYSTEM.md` never discovered — the trust gate prompts about files cyrup never reads |
| 17 | high | `TUI-031` (07) | PB-29 | M | A prompt typed during compaction is dispatched into a context the compaction is mid-rewrite of |
| 18 | high | `SEAM-051` (08) | VL-P19 | S | `--tui-mode regular` — the flag's **default** value — makes the binary exit 1 claiming the option is unknown |
| 19 | high | `SEAM-047` (08) | PB-30 | M | First SIGTERM/SIGHUP neither tears down nor exits; `cyrup --mode rpc` cannot be stopped by a supervisor |
| 20 | high | `SEAM-065` (08) | PB-41 | M | Trust is resolved pre-launch, inverting pi's tier order — the extension `project_trust` hook is skipped entirely |
| 21 | high | `SEAM-064` (08) | PB-40 | S | The pre-launch trust prompt drops both "(this session only)" options — every answer is persisted, including a lockout |
| 22 | high | `SEAM-063` (08) | PB-39 | M | Session delete permanently unlinks where pi routes through `trash`, and the failure is swallowed |
| 23 | high | `SEAM-061` (08) | PB-37 | M | `--resume` lists every project's sessions under "Current Folder" with no cwd column and a dead `tab scope` hint |
| 24 | high | `SEAM-062` (08) | PB-38 | S | Pre-launch rename is invited, accepted, echoed on screen — and discarded |
| 25 | high | `SUBA-014` (09) | PB-31 | S | `requireReadTool` unported — a skill-carrying agent is told to `read` a skill it has no `read` tool for |
| 26 | high | `SUBA-043` (09) | UW-14 | S | SINGLE-mode `outputSchema` unadvertised and hardcoded `None`, so the structured-output channel is unreachable |
| 27 | high | `PERM-023` (10) | §5 | S | The install probe never consults `agents_dir`, which the manager enforces — an agent-frontmatter deny is silently inert |
| 28 | high | `DRIFT-049` (12) | PB-30 | M | **duplicate of `SEAM-047`** — schedule once, in area 08; area 12's body carries the RPC-mode analysis |

Six of the top ten are effort **S**. Three pairs must ship together or the defect moves rather than
closes: `AGENT-020`+`AGENT-030`, `TUI-042`+`TUI-043`+`TUI-044`, `SEAM-047`+`SEAM-059`(+`SEAM-008`).

---

## 1. Port bugs — upstream had it at the tag cyrup ported; cyrup does not

> **⚠ INCOMPLETE AS OF 2026-08-14 (fourth edition), and stated rather than implied. The class
> sections §1–§4 were NOT regenerated for the 93 ids the surface enumeration filed.** They enumerate
> the pre-enumeration set. The 93 new ids carry no `PB-nn` / `UW-nn` / `VL-nn` entry number and are
> **not** listed below — they are in their area files, with the counts and the cross-cutting reading
> in `00-residual-ledger.md`'s fourth-edition block. Regenerating these sections is a pass of its own
> (67 of the 191 findings are `missingInCyrup`, i.e. §1 material; 58 are `differingShape`, which this
> taxonomy has no home for at all — **that is itself a finding about the taxonomy**). **Do not read
> the absence of an id from §1 as evidence that no port bug was filed for it.**
>
> **AND DO NOT READ AN ENTRY'S PRESENCE AS EVIDENCE THAT IT IS OPEN — added 2026-08-19.** §1–§4 are
> a class-organised RENDERING of the area files at the edition that produced them; the owning area
> file's `## Open items` table is the only authority on status. Repairing this file's citations at
> `4fb5e40` walked past three specimens in a single pass — **PB-39** (`SEAM-063`, session delete via
> `trash`) closed 2026-08-14 and the seam now reports pi's own three strings from
> `app/execute_session.rs:24-32`; **PB-29** (`TUI-031`) closed 2026-08-14 with the compaction guard at
> `app/run_action.rs:68-82`; **UW-12** (`SESS-040`) closed 2026-08-15. Each entry now carries a dated
> `CLOSED` bullet, but they were found incidentally and **the rest of §1–§4 was not audited for
> status**. Check the area file before scheduling any entry here.

**These rank above everything else in this document at equal severity.** They are not version lag:
the behaviour was available to be ported and was not.

### 1a. From `pi` v0.83.0

**PB-1 · `radius` provider is not registered** — *medium* · area 01 `PROV-014` (re-confirmed at HEAD)
- upstream: `pi/packages/ai/src/providers/all.ts:117` @v0.83.0 (`radiusProvider()`; :121 @v0.84.1); definition `providers/radius.ts:20`; `env-api-keys.ts` @v0.83.0 already maps `RADIUS_API_KEY`
- cyrup: `crates/cyrup-provider/src/providers/all.rs:140-240` — no radius push; `env_api_keys.rs:34-73` has no such arm; `providers/builtin_oauth.rs:17` documents the hole in-tree
- observable: `--provider radius` resolves to no provider. The Radius OAuth flow is ported (`auth/oauth/radius.rs`, id registered at `auth/oauth/load.rs:59`) and the wire API it streams over exists (`api/pi_messages.rs`), so a working credential can never be attached to a streamable provider.

**PB-2 · `qwen-token-plan` and `qwen-token-plan-cn` are not registered, but the resolver advertises them** — *medium* · area 01 `PROV-014`, area 12 `DRIFT-019` (**kind corrected this pass**: `DRIFT-019` was `upstream-drift`; `git cat-file -e v0.83.0:` proves the upstream files predate the ported tag, so it is a port bug on both sides of the pair)
- upstream: `providers/all.ts:115-116` @v0.83.0; definition `providers/qwen-token-plan.ts:6-15`; `env-api-keys.ts` @v0.83.0 maps `QWEN_TOKEN_PLAN_API_KEY` / `QWEN_TOKEN_PLAN_CN_API_KEY`
- cyrup: `providers/all.rs:140-240` (no push, no catalog) versus `crates/cyrup-config/src/model.rs:1022-1023` (both in `KNOWN_PROVIDERS`) and `model.rs:973-974` (both given the default model `qwen3.7-max`)
- observable: cyrup accepts `--provider qwen-token-plan` at argument validation and resolves a default model for it, then fails at stream time with no such provider.

**PB-3 · `Models::refresh` accepts no options and returns no per-provider result** — *low* **(severity corrected down)** · area 01 `PROV-S05`
- upstream: `pi/packages/ai/src/models.ts:46-56` @v0.83.0 (`ModelsRefreshOptions{allowNetwork,force,signal}` + `ModelsRefreshResult{aborted,errors}`), refresh at `:276`; v0.84.1 adds `providers?: readonly string[]` (`models.ts:67`) and generation-checked publication (`:320-361`)
- cyrup: `crates/cyrup-provider/src/collection.rs:317-337` — `refresh(&self, provider: Option<&str>)`, `join_all` at `:335` with every result discarded, unconditional `Ok(())`
- **corrected by the re-audit**: most of what pi's options buy is already reproduced by a different mechanism this entry missed. `crates/cyrup/src/provider.rs:71-130` splits pi's `refresh({allowNetwork:false})` restore from the network refresh, gates the network path on mode (mirroring pi's rpc/interactive-only triggers) and restricts the fetch to configured providers exactly as pi's `resolveRefreshCredential` bail does. **What genuinely remains is the `errors`/`aborted` result shape, `force`, and the abort signal** — API-shape and error-reporting residue. The proposed raise to medium was rejected in area 01; it is *low*.
- observable: no force past a freshness check, no cancellation, and no report of which providers failed. Two concurrent refreshes of one provider both publish, last-writer-wins.

**PB-4 · Compact-read classification has no `docs` arm** — *low* · area 04 `TOOL-017`
- upstream: `pi/packages/coding-agent/src/core/tools/read.ts:98` @v0.83.0 (`getPiDocsClassification`, resolving against `dirname(getReadmePath())`), called from `:130` — **present at the ported tag**, so this is not lag
- cyrup: `crates/cyrup-tui/src/transcript.rs:2468` (`compact_read_classification` — `skill` and `resource` arms complete, no `docs` arm; the doc at `:2461-2467` states why, and names the missing seam: `getReadmePath` has no counterpart in `crates/`) *(citation re-resolved by symbol 2026-08-19; `:2265` is now a closing brace)*
- observable: reading cyrup's own shipped README/docs/examples renders as an ordinary file read. **Blocked on a decision, not on code** — see OQ-2. Area 04 confirms it is TOOL-017's residual and that it needs a packaged-docs locator to exist first.

**PB-5 · `PI_CODING_AGENT` is never stamped into the environment (and `AI_AGENT` is the v0.84.1 half)** — *low* · area 04 `TOOL-031`
- upstream: `pi/packages/coding-agent/src/cli.ts:13` @v0.83.0 (`process.env.PI_CODING_AGENT = "true"`); `AI_AGENT = "pi"` is new at v0.84.1, `cli.ts:14` and `rpc-entry.ts:8`
- cyrup: `crates/cyrup/src/main.rs:53-57` — an explicit comment declines to replicate it because `std::env::set_var` is `unsafe` under edition 2024; `crates/cyrup-tools/src/tools/bash.rs:158-175` assembles the child env explicitly and adds neither key
- observable: a shell hook, npm script or MCP server that branches on `$AI_AGENT` / `$PI_CODING_AGENT` cannot tell it is inside cyrup. The unsafe-`set_var` rationale covers *process-global* mutation only — `bash.rs` already builds a per-child env vector, so both keys can be added there with no `unsafe`. Area 12 rejected a duplicate filing (`DRIFT-044`) because **PB-5 is strictly broader**. Its sibling — `process.title`'s role suffix, which is a syscall against the current process and carries none of the `set_var` hazard — is `DRIFT-051` / `SEAM-070`, filed this pass.

**PB-6 · Changelog-on-upgrade is absent; `lastChangelogVersion` is never read or written** — *medium* · area 05 `CFG-015`, area 07 `TUI-011`
- upstream: `modes/interactive/interactive-mode.ts:997` @v0.83.0 (`getLastChangelogVersion`), `:998-999` (`getChangelogPath` / `parseChangelog`), `:1003` and `:1010` (`setLastChangelogVersion(VERSION)`); getter/setter at `core/settings-manager.ts:660` and `:664`
- cyrup: `crates/cyrup-config/src/settings.rs:994` (`last_changelog_version`) has zero callers workspace-wide and no setter exists; `/changelog` is hardcoded at `crates/cyrup-tui/src/app/submit.rs:111-113` to `push_block("What's New", "No changelog entries found.")` *(was `app.rs:1824` before the `40821ed` split)*
- observable: after upgrading, pi shows the new entries once and records the version; cyrup shows nothing. The `collapseChangelog` settings row (`app/settings_rows.rs:115`) toggles a value nothing reads. (`enableInstallTelemetry`, the row beside it, **does** have live consumers — `cyrup-config/src/policy.rs:25-27`, `cyrup-session-svc/src/builder.rs:1145` — so it is not part of this claim.)

**PB-7 · The npm package channel is unported, and `npmCommand` is inert** — *large* · area 05 `CFG-009` / `CFG-015`
- upstream: `core/package-manager.ts:1720` (`getNpmCommand`) with install/update/list through `:1740` (`runNpmCommand`), `:1745` (`getGitDependencyInstallArgs`), `:1753` (`runNpmCommandSync`); manifest kinds at `core/pi-manifest.ts:3-9` — a package may ship `extensions`, **`skills`, `prompts` and `themes`**
- cyrup: `crates/cyrup-resources/src/package/source.rs:81-83` returns `Err(Unsupported)` for any `npm:` spec **with the misleading message "unsupported source (OCI deferred)"** (area 05 `CFG-009`); `crates/cyrup-config/src/settings.rs:742` (`npm_command`) has zero callers anywhere
- observable: `cyrup install npm:<pkg>` fails outright, and setting `"npmCommand": ["pnpm","--silent"]` does nothing. The *extension* half is genuinely mechanism-forced (WASM guests cannot load a TypeScript extension); skills, prompts and themes are plain files needing no runtime and are unreachable purely because the channel is gone. See OQ-1. Downstream of this: pi's `.pi-update-incomplete` marker has nowhere to attach (area 05 records it as deliberately not filed for that reason).

**PB-22 · `google-vertex` is registered with 10 catalog models and has no wire API — every request dies at dispatch** — *high* · area 01 `PROV-030`
- upstream: `pi/packages/ai/src/providers/google-vertex.ts` @v0.83.0 registers the provider **and** its api implementation together
- cyrup: `crates/cyrup-provider/src/providers/all.rs:187` pushes `google-vertex` with a 10-row catalog (`providers/catalog/google-vertex.json`) and resolved auth including the ADC arm, and it appears in `/model`. But `api/mod.rs:130-163` `register_builtins` registers **9** factories and none of them is google-vertex, and there is no `api/google_vertex.rs`. All 10 rows die at `wire.rs:158-166` with `no API implementation for google-vertex`
- observable: selecting any Vertex model fails at stream time with an internal error after the model list has already offered it. **This is the exact failure mode `PROV-005`'s own Fix text warned about for `bedrock-converse-stream`** — it was fixed there and shipped here in the same sweep. `PROV-005` stays closed; this defect is new and carries its own id.
- **Mandatory in the same change (added this pass):** rewrite the port-status doc table at `providers/all.rs:12-47`, which still calls `amazon-bedrock` / `google-vertex` / `openai-codex` "**pending**" and names all four including `github-copilot` at `:46-47` as "Pending (NOT registered)" — while `:176-197` pushes all four and `:21` already marks copilot ported. The table is self-contradictory *and* it flatly denies this item's premise; it is the first thing an engineer opening the file reads. `PROV-038`'s roster rewrite should then assert `all_providers()` matches the set the table marks registered.

**PB-23 · GitHub Copilot's Claude models send `x-api-key`; pi sends `Authorization: Bearer`** — *high* · area 01 `PROV-027`
- upstream: `pi/packages/ai/src/api/anthropic-messages.ts:867-888` @**both** v0.83.0 and v0.84.1 (re-read at both tags this pass, offsets verified equal) branches on `model.provider === "github-copilot"` **before** the OAuth test
- cyrup: `api/anthropic_messages.rs:470-536` `build_headers` has **no provider branch** — the scheme is chosen solely by `is_oauth`, derived at `:434-437` from `api_key.contains("sk-ant-oat")`, and the non-OAuth arm at `:524-531` emits `x-api-key`
- observable: every request on that route arrives unauthenticated. Blast radius measured by parsing the catalog: `github-copilot.json` has 28 rows, exactly **9** on `anthropic-messages`.

**PB-24 · `github-copilot-headers.ts` is unported on all three routes** — *high* · area 01 `PROV-028`
- upstream: `pi/packages/ai/src/api/github-copilot-headers.ts` @v0.83.0 exports `inferCopilotInitiator` / `hasCopilotVisionInput` / `buildCopilotDynamicHeaders`, applied under the Copilot guard at `anthropic-messages.ts:867-871`, `openai-completions.ts:638-645` and `openai-responses.ts:223-230`. **Citation corrected this pass**: the previously recorded `openai-completions.ts:646-652` is the v0.84.1 offset, not v0.83.0
- cyrup: `rg -i 'X-Initiator|Copilot-Vision|Openai-Intent' crates/cyrup-provider/src` returns only the login flow's unrelated `openai-intent: chat-policy` (`auth/oauth/github_copilot.rs:666`); there is no `api/github_copilot_headers.rs` and no dynamic-header call in any api impl
- observable: Copilot image turns are rejected outright (no `Copilot-Vision-Request` — a loud failure on a normal path) and every agent-loop request is misreported for quota (no `X-Initiator` / `Openai-Intent`).

**PB-25 · `continue_run` drains the steering queue before claiming the run latch** — ~~***critical***~~ ***low*** **(raised to critical 2026-08-12 on a predicted consequence; LOWERED to low 2026-08-13 after that consequence was measured and refuted — typing during a live stream delivered the message 5/5 times, see `REPRO-LOG.md`)** · area 02 `AGENT-020`
- upstream: `pi/packages/agent/src/agent.ts:350` @v0.83.0 — `async continue()`, with the run-active guard at **`:351-353`** running **before both drains** (`:361` steering, `:367` follow-ups). **Citation corrected this pass**: the previous edition cited `:362-364`/`:361-388` and asserted tag-invariance; those are the **v0.84.1** offsets. The bodies are byte-identical across the tags, the line numbers are not
- cyrup: `crates/cyrup-agent/src/agent.rs:1637` `continue_run` drains steering at `:1646` and follow-ups at `:1650`, then `start_run` (`:1659`) claims the latch at `:1672-1682` and returns `Err(AgentError::RunActive)` at `:1681`; `queue.rs:51` `drain()` removes
- observable: an `Err(RunActive)` silently destroys a user-typed steering message — no error, no log, no retry. **Typing while a turn streams is the normal path**, which is why this is critical rather than high. Fix has two halves and needs both: hoist the guard as a fast path, *and* push the drained vec back with a new `PendingQueue::push_front` on `Err` (the fast path is racy in Rust, where pi gets atomicity from single-threaded JS).

**PB-26 · `AgentSession::prompt` gates on the agent's per-run flag, so a prompt in the post-run gap starts a SECOND run** — *high* · area 02 `AGENT-030`
- upstream: `pi/packages/coding-agent/src/core/agent-session.ts` @v0.83.0 — `_isAgentRunActive` set `:1062`, cleared `:582`, consulted `:876-877` and `:1159`; it spans `_handlePostAgentRun` and every `agent.continue()`, so a submission during the post-run driver loop is routed to steering
- cyrup: `crates/cyrup-session-svc/src/session.rs:627` (and `prepare`, `:854`) gate on `agent.snapshot().is_streaming` (`:3202-3204`), a per-run flag `SettlementGuard::drop` clears at `cyrup-agent/src/agent.rs:1441` the moment each individual run settles. The session already owns the right latch — `driver_tx`, set in `spawn_run` at `:686` and dropped after the whole post-run loop at `:739` — but it is consulted only by `is_idle()` (`:601-603`)
- observable: a prompt landing in the post-run gap (auto-retry, auto-compaction, queued continuation) starts a second run and races `continue_run`. **Must land in the same change as PB-25** or the loss just moves to the other branch.

**PB-27 · `.cyrup/SYSTEM.md` and `APPEND_SYSTEM.md` are never discovered — the trust gate prompts about files cyrup will never read** — *high* · area 05 `CFG-035`
- upstream: `core/resource-loader.ts:1022-1034` @v0.83.0 `discoverSystemPromptFile()` (project `.pi/SYSTEM.md` when trusted, else `<agentDir>/SYSTEM.md`), `:1036-1048` the identical pair for `APPEND_SYSTEM.md`; consumed in `reload()` at `:525` and `:533-535`. Unchanged at v0.84.1
- cyrup: `grep -rn 'SYSTEM\.md' crates/` returns five hits and **not one reads a file** — a doc comment (`cyrup-session/src/prompt/overrides.rs:12-16`), two trust-gate MARKERS (`cyrup-config/src/trust.rs:194`, `:203-204`) and a test. The only producers of the two override fields are the CLI flags (`cyrup-session-svc/src/builder.rs:1051`, `:1055` ← `cyrup/src/cli.rs:456-463`)
- observable: a project shipping `.cyrup/SYSTEM.md` gets the DEFAULT system prompt with no diagnostic — silent wrong output on a normal path. Made worse by the half-port: `has_trust_requiring_resources` prompts the user to trust the project *because* the file exists, then loads nothing from it. cyrup ported the gate and not the thing it gates. Note pi's `??` semantics: the CLI flag **replaces** the discovered append file, it does not accumulate — `overrides.rs:15-16` documents the opposite and must be corrected in the same change.

**PB-28 · `/tree` has no text search, and its four action keys are the characters pi types INTO that search — `e` persists the typed text as a label** — ***critical*** **(raised from high this pass)** · area 07 `TUI-027`
- upstream: `modes/interactive/components/tree-selector.ts:113`, `:1079-1100` — `z`/`x`/`e`/`t` are ordinary characters typed into the tree's text filter
- cyrup: `crates/cyrup-tui/src/tree_selector.rs:850-889` binds them as actions, and `e` opens the inline label editor, which captures all keys. **Corrected trace this pass** — the persistence is two hops past the local star update (`update_node_label`, `:529-533`): the confirm arm returns `SelectorOutcome::Apply(entry_id + FIELD_SEP + label)` (`:540-546`) → `app/selectors.rs:201-208` splits it on `FIELD_SEP` into `AppCommand::SetEntryLabel` → `app/execute.rs:288-298` calls `session.services().host_services.set_label(&entry_id, (!label.is_empty()).then_some(label.as_str()))` → `manager.append_label`, the same live path an extension's `setLabel` uses. *(Trace re-resolved by symbol 2026-08-19: the middle hop was cited as `app.rs:3306-3307` → `app/tree_nav.rs`, and `tree_nav.rs` has no part in it — the `40821ed` remap carried the wrong module forward.)* The seven `app.tree.filter.*` ids are unknown to `TreeAction::from_id` (`:887-895`), so a pi-shaped `keybindings.json` cannot fix it
- observable: a pi user typing a filter word into `/tree` silently renames a session branch **in the session JSONL**. Corruption of persisted user data on a normal path.

**PB-29 · A prompt typed during compaction is dispatched immediately instead of queued** — *high* · area 07 `TUI-031` · **supersedes VL-P11**, which filed the same defect as lag and at *small*
- upstream: `modes/interactive/interactive-mode.ts:3023-3033`, `:4230-4236` — `queueCompactionMessage` with a visible status; the session-level throw is `core/agent-session.ts:1133-1137`
- cyrup: the TUI never consults `is_compacting` (the `AppAction::Submit` arm, now `app/run_action.rs:83-103`, branched on `is_streaming` only) **and** `AgentSession::prepare` has no compaction guard either (`session.rs:849-900`); `is_compacting` exists at `session.rs:4110` and its one production consumer is an RPC status field (`cyrup-modes/src/rpc.rs:1428`)
- **CLOSED — verify before scheduling.** Area 07 struck `TUI-031` on 2026-08-14 and the guard is at HEAD (`4fb5e40`): `app/run_action.rs:68-82` is a `Submit` arm guarded `if ctx.session.is_compacting() && !is_extension_command(…)` sitting **above** the streaming arm, routing to `queue_compaction_message(text, false)`, with pi's identical follow-up gate at `:116-117`
- observable: the turn is assembled from a context the compaction is mid-rewrite of. Not "rejected instead of accepted" — **wrong context, silently**. Note `TUI-016`: there is currently no surface that would *show* a queued message, so the queue and its indicator ship together.

**PB-30 · First SIGTERM/SIGHUP neither tears down nor exits 143/129 — `--mode rpc` cannot be stopped by a supervisor** — *high* · area 08 `SEAM-047`, area 12 `DRIFT-049` (**duplicate — schedule once, in area 08**)
- upstream: `modes/rpc-mode.ts:366-383` @v0.83.0 registers SIGTERM plus SIGHUP off-win32 and calls `killTrackedDetachedChildren()` then `shutdown(SIGHUP?129:143, signal)`; `shutdown` at `:724-741` runs `runtimeHost.dispose()` then `process.exit`. `print-mode.ts:50-66` is the same shape
- cyrup: `crates/cyrup/src/signals.rs:88-101` does only `session.abort()` + `cancel.cancel()` on the first delivery, and the token it fires is `main.rs:367`'s **TUI input** `CancelToken`; neither `run_rpc` (`cyrup-modes/src/rpc.rs:575-579`) nor `run_print`/`run_json` takes a cancel token at all, and `rpc_driver`'s `select!` (`rpc.rs:717-842`) has no cancellation arm
- observable: `cyrup --mode rpc` never returns, `runtime.dispose()` never runs, and no `session_shutdown` is ever emitted. Interactive/print/json survive only incidentally, because the mode loop returns and `main.rs:575` / `run.rs:39` / `run.rs:60` then dispose. The repeat force-exit path *is* implemented with pi's exact 130/143/129 codes (`signals.rs:97-100`) — which is why `SEAM-S02` should be re-audited as **closed**. **Ships with `SEAM-059`** (the watcher holds the startup session `Arc` and aborts a disposed session after any `new_session`/switch/fork), because both rewrite the same function.

**PB-33 · The undo snapshot omits the paste registry — one undo sends the literal `[paste #N …]` marker to the model** — ***critical*** · area 07 `TUI-042` (new this pass)
- upstream: `pi/packages/tui/src/editor.ts:216-220` — `EditorSnapshot` carries `pastes` **and** `pasteCounter`, restored at `:2012-2030`. Byte-identical at v0.83.0 and v0.84.1 **at the same line numbers** (checked at both tags, not asserted)
- cyrup: `crates/cyrup-tui/src/editor.rs:71-78` — `Snapshot { lines, row, col }`, no pastes. `backspace()` (`:814`) and `delete()` (`:852`) erase `pastes[N]` *after* the snapshot was pushed, and `undo()` (`:748-756`) restores only the visible text, so `marker_at` (`:663-694`, which ends `self.pastes.get(&id)?`) no longer matches. `history_draft` (`:93`, `:1199`, `:1218`) reuses the same struct and inherits the defect
- observable: undo restores the marker text and Enter sends ~20 literal characters instead of the pasted content — silent wrong output, on a keystroke pair every user makes. Ships with `TUI-044` (the same `undo()` discards `Snapshot::col`, a field written and never read).

**PB-34 · Word motion and Ctrl+W are not paste-marker atomic — one Ctrl+W after a large paste drops the paste** — ***critical*** · area 07 `TUI-043` (new this pass)
- upstream: `pi/packages/tui/src/word-navigation.ts:9-14` declares `isAtomicSegment` with pi's own paste-marker comment; `findWordBackward`/`findWordForward` take the atomic branch at `:44-46` and `:97-99` (present at v0.83.0)
- cyrup: `editor.rs:1074-1128` `word_left_target`/`word_right_target` classify only by `is_word_char` (`:1637-1639`) and never call `marker_covering` (`:697-712`, which has just two callers); `delete_word_backward`/`delete_word_forward` (`:874-892`) never drop the registry entry the way `backspace()` does at `:814`
- observable: one Ctrl+W at the end of `[paste #1 +42 lines]` deletes the single `]`, the marker stops matching, and Enter sends the 19-character fragment. Ships with `TUI-042`; `TUI-049` (`marker_at` accepts text pi's regex rejects) is the same code and should be folded in.

**PB-35 · `httpProxy` reaches only the streaming wire APIs — OAuth, the agent proxy transport and extension HTTP all bypass it** — *high* · area 01 `PROV-047` (new this pass)
- upstream: `packages/ai/src/utils/http-dispatcher.ts:43-48`, `:79-103` @v0.83.0 — pi installs a **process-global** undici dispatcher, so every `fetch` in the process is proxied
- cyrup: `cyrup-session-svc/src/builder.rs:229-239` turns the setting into a `ProviderEnv` overlay read solely by `sse.rs:181-192` `build_client_for_target`. Every other egress path calls `build_client()` (`sse.rs:140-144`), which has no proxy handling: five OAuth flows (`auth/oauth/{anthropic:443, openai_codex:552, xai:525, openrouter:372, radius:468}`), `cyrup-agent/src/proxy.rs:455`, and `cyrup-provider/src/wire.rs:472`; `cyrup-ext/src/caps/http.rs:599` is a bare `reqwest` builder with reqwest's own competing env detection
- observable: on a proxied network, streaming works and logging in does not — and the failure is a connection error with no mention of the proxy. Fix: `configure_http_proxy()` beside `configure_http_idle_timeout`, `build_client_for(target_url)` running the already-ported resolver, the URL threaded through the seven call sites, and `.no_proxy()` on `caps/http.rs` so reqwest's detection is retired process-wide.

**PB-36 · A lone-surrogate `\uXXXX` escape in a provider SSE frame kills the whole assistant turn** — *high* · area 01 `PROV-048` (new this pass)
- upstream: `packages/ai/src/utils/sanitize-unicode.ts` @v0.83.0 — `JSON.parse` accepts the escape and `sanitizeSurrogates` strips it on the way back out. (The **outbound** half is a correct documented no-op in Rust: a `String` cannot hold a lone surrogate. Only the inbound half is a gap.)
- cyrup: `serde_json` rejects it; `repair_json` re-emits it verbatim (`utils/json_parse.rs:67-75`) so `repaired == json` and `parse_json_with_repair` returns `None`; both SSE callers treat `None` as fatal (`anthropic_messages.rs:1439-1449`, `google_generative_ai.rs:975-985`)
- observable: one malformed escape from a provider ends the turn. The same weakness breaks **resuming a pi-written session JSONL**. Fix is one arm: in `repair_json`'s `Some('u')` valid-hex branch, drop an escape decoding to an unpaired surrogate so `repaired != json` and the retry succeeds. Ships with `PROV-049` / `PROV-050` (the other two defects in the same repair path).

**PB-37 · `--resume` lists every project's sessions under "Current Folder", and the `tab scope` hint it prints is dead** — *high* · area 08 `SEAM-061` (new this pass)
- upstream: `pi/packages/coding-agent/src/cli/session-picker.ts:15-19` @v0.83.0 (byte-identical at v0.84.1) — `selectSession(currentSessionsLoader, allSessionsLoader, settingsManager)` takes **two** loaders and passes both to the component, which starts at `scope: "current"` (`components/session-selector.ts:704`), loads only the current set (`:859`), and toggles on Tab (`:551-556`); `main.ts:419-421` supplies `SessionManager.list(cwd, …)` and `SessionManager.listAll(…)`
- cyrup: `crates/cyrup/src/main.rs:1259-1268` `gather_session_infos` concatenates the cwd listing and the cross-project listing into **one** vector, handed to a `SessionSelector` defaulting to `scope: Current` (`cyrup-tui/src/session_selector.rs:204`); no `SessionAction::ToggleScope` exists, so the advertised hint cannot fire and `show_path` never flips
- observable: on any machine with more than one cyrup project the picker is headed "Current Folder" over every session on disk, with no cwd column and rows labelled only by their first message; picking a foreign row resumes another project's session with no guard (`main.rs:1124`). Both halves must land together or the screen keeps lying. **Verification requires a live run in a real terminal** with two project dirs.

**PB-38 · The pre-launch rename is invited, accepted, echoed, and discarded** — *high* · area 08 `SEAM-062` (new this pass)
- upstream: `cli/session-picker.ts:48` @v0.83.0 (byte-identical at v0.84.1) passes `showRenameHint: false` and **no** `renameSession` callback, so `canRename` is false (`components/session-selector.ts:771`), the hint is hidden (`:772`) and the handler bails before entering rename mode (`:807-808`). pi's pre-launch picker cannot rename at all
- cyrup: `session_selector.rs:214` defaults `show_show_rename_hint: true` and `startup_ui.rs:126-127` never disables it; `SessionAction::Rename` is ungated (`:833-837`), the row is mutated in place (`:798-801`) and an `Apply(rename_payload)` is returned (`:802`) — but `run_resume_picker`'s `on_apply` (`startup_ui.rs:129-138`) matches only `Delete`
- observable: complete positive feedback for an operation that is never persisted. Same class as PB-28 (typed text accepted and thrown away), on a surface nobody had read. Minimum fix is two lines; the preferred fix reuses `session.rs:3355-3365`'s existing rename sequence.

**PB-39 · Session delete permanently unlinks where pi routes through `trash`, and the failure is swallowed** — *high* · area 08 `SEAM-063` (new this pass)
- upstream: `modes/interactive/components/session-selector.ts:645-680` @v0.83.0 (identical at v0.84.1) — `deleteSessionFile` runs `spawnSync("trash", …)` **first** with a `["--", path]` guard (`:649`), treats exit-0 **or** the file having vanished as success with `method:"trash"` (`:666-668`), falls back to `unlink` (`:672-674`), and on failure returns `{ok:false,error}` with a `trash: …` hint (`:675-679`); the caller reports which happened
- cyrup: two bare `std::fs::remove_file` sites — `startup_ui.rs:133-137`, which additionally `let _`s the `io::Result`, and `cyrup-session-svc/src/session.rs:3343-3347`, whose caller (the `C::DeleteSession` arm, now `app/execute_session.rs:15-33`) prints "deleted session" either way. `rg -ni 'trash' crates/` returns nothing
- **CLOSED — do not schedule.** Area 08 struck `SEAM-063` on 2026-08-14 and the seam is at HEAD: `delete_session_file` returns a method, and `app/execute_session.rs:24-32` pushes pi's own `"Session moved to trash"` / `"Session deleted"` / `Failed to delete: {e}` strings (`session-selector.ts:846`/`:849` @v0.83.0). The pre-launch `--resume` half — the residual `REPRO-LOG.md` §3 measured — is the part to re-check
- observable: for every user with `trash` installed, pi's delete is recoverable and cyrup's is not — one confirmed keypress destroys a conversation JSONL with no undo, on the same screen whose *reversible* action (PB-38) is the broken one. A failed delete on a read-only volume looks identical to a successful one.

**PB-40 · The pre-launch trust prompt drops both "(this session only)" options — every answer is persisted** — *high* · area 08 `SEAM-064` (new this pass)
- upstream: `core/project-trust.ts:32` @v0.83.0 (identical at v0.84.1) — the **pre-launch** path calls `getProjectTrustOptions(cwd, { includeSessionOnly: true })`; `core/trust-manager.ts:82-84`, `:91-93` append the two ephemeral options with an empty `updates`, which `saveProjectTrustPromptResult` skips writing (`project-trust.ts:40-44`). pi's **in-app** selector passes no flag — so the asymmetry is deliberate upstream
- cyrup: `crates/cyrup/src/main.rs:1155` passes `false`. That flag gates exactly both ephemeral rows (`cyrup-config/src/trust.rs:356-363`, `:370-377`), so the prompt renders three rows, every one with a non-empty `updates`, and `run_trust_prompt` persists unconditionally (`startup_ui.rs:266-268`). cyrup's other call site (`session.rs:3255`) is correct and must be left alone
- observable: a user cannot answer a security prompt about someone else's repository without recording a permanent verdict in `trust.json` — including "Do not trust", which then locks the folder out with no prompt offered to reverse it. One-line production change plus a test rewrite (`startup_ui.rs:504-537`).

**PB-41 · Trust is resolved pre-launch, inverting pi's tier order — the extension `project_trust` hook never runs** — *high* · area 08 `SEAM-065` (new this pass)
- upstream: `core/project-trust.ts:46-95` @v0.83.0 (identical at v0.84.1) — `resolveProjectTrusted` orders the tiers: `trustOverride` (`:47`), no-trust-requiring-resources (`:50`), then **`emitProjectTrustEvent` at `:54-70`**, which returns before anything else and persists when `remember === true`; only then the store (`:72-75`), the default policy (`:77-84`), `hasUI` (`:86-88`), and last the prompt (`:90-94`)
- cyrup: `main.rs:325-329` calls `resolve_startup_ui` **before** any runtime exists; `main.rs:1142-1162` resolves trust from store + default policy, prompts, and sets `config.trust_override` (`:1159`) — which short-circuits `cyrup-session-svc/src/builder.rs:495-499`, so `pre_trust_extension_verdict` never runs whenever the user answered
- observable: an extension implementing `on-project-trust` (declared at `cyrup-ext-sdk/wit/world.wit:237`) is defeated on the interactive path: the human is asked first and wins, the hook is suppressed, and its `remember` half never fires. **Reachability caveat, stated because it bounds the severity**: no in-tree extension implements the hook today, so the bypass is latent — but the seam the builder was written for is dead on the path that matters. Also retires `builder.rs`'s `saved: None` and its "no trust store is wired" warning.

### 1b. From `pi-subagents` v0.43.0

> **Reclassification, and it is large.** The previous edition filed `VL-S1…VL-S15` as version lag on
> the strength of "baseline ≈v0.34.0 with holes". The recorded baseline is **v0.43.0**, and every one
> of those fifteen has a first tag **at or before v0.43.0** — so by this document's own baseline they
> are in-baseline **port bugs**, not lag. Area 09 proved three of them directly with
> `git cat-file -e v0.43.0:<path>` (`capability-ceiling.ts`, `usage-budget.ts`, `spawn-budget.ts` all
> present at both v0.43.0 and v0.47.1) and re-classified its own `SUBA-021`, `SUBA-017` and
> `SUBA-022` the same way. **The ids do not change; the section does.** The genuine
> `v0.43.0..v0.47.1` lag is §3b, and it is 17 items nobody had looked at.
>
> Area 09 deliberately does **not** restate `PB-8…PB-14`, `UW-3…UW-8` or `VL-S1…VL-S15` as findings —
> it confirmed them still accurate at HEAD by spot-check and left them owned here. **This section is
> therefore the only record for that work; do not compress it away.**

**PB-8 · Subagent RPC bridge is entirely absent** — *large*
- upstream: `src/extension/rpc.ts:622` @v0.43.0 (`registerSubagentRpcBridge`, 653-line file; method list `:29`; event names `:25-27`), registered from `src/extension/index.ts:529`. First tag **v0.33.0**
- cyrup: `crates/cyrup-ext-subagents/src/extension.rs:9313-9352` (the whole `init` registration/subscription block) — no bridge; `grep -ri 'subagents:rpc' crates/` returns 0
- observable: no host, embedder or sibling extension can drive subagents programmatically. Upstream answers `ping`/`status`/`spawn`/`steer`/`interrupt`/`stop`/`resume` over `subagents:rpc:v1:request` with a `subagents:rpc:v1:reply:<id>` envelope; cyrup emits no ready event and answers nothing. *(Area 09 blind spot 3: `src/extension/rpc.ts` was not read on the upstream side this pass either.)*

**PB-9 · `clarify: true` is advertised but shows no preview/edit UI** — *large*
- upstream: `src/runs/foreground/chain-clarify.ts:199` (`ChainClarifyComponent`, 1350-line file), dispatched at `subagent-executor.ts:3190`, `:3572` and `chain-execution.ts:692`, all three via `await ctx.ui.custom<ChainClarifyResult>(...)`. First tag **v0.21.2**
- cyrup: `extension.rs:6634` declares the param with the description "Show TUI to preview/edit before execution."; the flag is read at `:5576-5578` (the async→foreground downgrade) and at `:9678` (suppressing the `[async]` badge) — **neither read produces a UI**
- observable: cyrup accepts `clarify: true`, forces the run foreground, and launches immediately with the model's unmodified prompt. The tool description promises a UI that does not exist. The seam it needs is live: `HostServices::open_overlay` (`cyrup-ext/src/host/services.rs:224`) is already consumed in production by this same crate at `extension.rs:9908`.

**PB-10 · `turnBudget` — no soft assistant-turn budget for children** — *medium* · = area 09 `SUBA-008`
- upstream: `src/runs/shared/turn-budget.ts:5` (`resolveTurnBudgetConfig`) and `:26` (`appendTurnBudgetSystemPrompt`); tool param `src/extension/schemas.ts:328`. First tag **v0.33.0**
- cyrup: the tool schema at `extension.rs:6634` has 45 `props.insert` keys and none is `turnBudget`; the flag has **three** hard-coded `false` consumers, each commented as having no source — `tui/intercom.rs:348-352`, `exec/fallback.rs`, `exec/mod.rs:2354-2360` *(the previous count of two was wrong)*
- observable: no "## Turn budget" wrap-up block in the child's system prompt, no abort past `maxTurns+graceTurns`, and the result always reports `turnBudgetExceeded: false`, so an unexplained process signal is misattributed. (Frontmatter `toolBudget` **is** read, `discovery/frontmatter.rs:850` — this is the turn half only.)

**PB-11 · Scheduled subagent runs (`schedule.*`) are unported — and it is NINE verbs, not four** — *large* · = area 09 `SUBA-016`
- upstream: `src/runs/background/scheduled-runs.ts:14` (`SCHEDULED_RUN_ACTIONS`) and `:358` (`class ScheduledRunManager`), 753-line file, present at v0.43.0 and v0.47.1. **Nine** verbs in `shared/types.ts:1968` — `schedule.create`, `.list`, `.show`, `.history`, `.pause`, `.resume`, `.run`, `.run-due`, `.delete`. First tag **v0.33.0**
- cyrup: zero hits for `scheduled_runs`; the 27-verb enum at `extension.rs:6557` has nothing beginning `schedule.`; `extension.rs:3909` states "The `schedule.*` family is unported"
- observable: `subagent({action:"schedule.create", at:…})` is refused as an unknown action **after its schedule parameters are silently discarded**, so the error does not explain the failure. The in-tree note at `extension.rs:12572` pinning the enum as "pi's SUBAGENT_ACTIONS union minus the deferred schedule* four" is stale on the count.

**PB-12 · No live child transcript writer; the `transcriptPath` artifact is missing** — *medium*
- upstream: `src/shared/child-transcript.ts:102` (`createChildTranscriptWriter`, per-record `fs.appendFileSync` at `:133`), created at `runs/background/subagent-runner.ts:1200-1201`; the field is the **fourth** `ArtifactPaths` member (`src/shared/types.ts:1048`, interface opens `:1044`); reported by `runs/background/run-status.ts:128`. First tag **v0.33.0**
- cyrup: `crates/cyrup-ext-subagents/src/artifacts.rs:61-70` — `ArtifactPaths` has four fields (input/output/jsonl/metadata) and `:58` says so; the substitute `.jsonl` is written only after the run settles (`extension.rs:4925-4928` foreground, `background/runner_main.rs:2611-2614` background). A live NDJSON stream exists but goes elsewhere: `exec/mod.rs:2113-2118` writes `<cwd>/.cyrup-subagent-scratch/attempt-N.jsonl`
- observable: the FleetView transcript pane for a RUNNING foreground child points at `paths.jsonl_path` (`tui/fleet.rs:1041-1058`), a file that does not exist until the child finishes, so it renders empty where upstream's fills in real time; `status`/`run-status` never print a `Transcript:` line.

**PB-13 · Chain-run artifacts default to the temp root, not the project** — *small*
- upstream: `runs/foreground/subagent-executor.ts:2022` @v0.34.0 (`chainDir: params.chainDir ?? getProjectChainRunsDir(effectiveCwd)`), helper `src/shared/artifacts.ts:16`. At v0.43.0 the same slot is `subagent-executor.ts:2623` via `getChainRunsDir`, whose "project" default still resolves to `getProjectChainRunsDir` (`shared/artifacts.ts:141-143`)
- cyrup: `artifacts.rs:146` (`project_chain_runs_dir`) has **zero references of any kind**; the live resolver `resolve_chain_dir` (`extension.rs:6539`) falls back to `chain_runs_dir(cwd)` = `temp_root_dir()/chain-runs/<cwd_key>` (`artifacts.rs:164-166`)
- observable: a chain run's artifacts land under `$TMPDIR/.../chain-runs/<cwd_key>/<runId>` instead of `<cwd>/.cyrup-subagents/chain-runs/<runId>` — invisible to the project, not committable, swept by OS tmp cleanup. The `[CYRUP-DELTA]` at `extension.rs:6536-6538` documents only the added per-run subdirectory and is silent on the root change. **Land with area 09 `SUBA-048`** (the `artifactDir` preference key, which is the same resolver and makes `project` the correct default for both).

**PB-14 · The "skills not found" warning is unported on BOTH surfaces** — *small*
- upstream, run side: `runs/foreground/execution.ts:1112` @v0.34.0 — `skillsWarning: missingSkills.length > 0 ? …` declared on the shared result shape at `:179` (v0.43.0: `execution.ts:1524`)
- upstream, management side: `agents/agent-management.ts:773` and `:823` @v0.34.0 call `skillsWarning(ctx.cwd, …)`, helper `:190` (v0.43.0: `:971`, `:1023`, helper `:206`)
- cyrup: `exec/mod.rs:3190-3193` keeps `resolution.resolved` and **discards `resolution.missing`**; `SingleResult` has no `skills_warning` field and `artifacts.rs:427` documents omitting it. On the management side `discovery/skills.rs:149` (`resolve_skills`) has zero callers, and the stale deferral note at `discovery/management.rs:1276-1277` still claims the skills subsystem is "entirely absent today"
- observable: `subagent({action:"create", config:{skills:"typo"}})` reports success with no warning, **and** a run with the same typo produces no warning either. (The `Skills not found:` string at `exec/mod.rs:3180` is a different thing: the hard failure for a missing *orchestration* skill, exit 1.)

**PB-31 · `requireReadTool` unported — a skill-carrying agent is told to `read` a skill it has no `read` tool for** — *high* · = area 09 `SUBA-014` (severity raised from medium)
- upstream: `src/runs/shared/pi-args.ts:355-372` @v0.43.0 — `requireReadTool` head-injects `read` into the allowlist under `requireReadTool && requestedBuiltinTools.length > 0 && !includes("read")`, with **seven** live setters, all deriving it from `Boolean(resolvedSkills.length)`
- cyrup: `exec/mod.rs:1463-1491` builds the tool allowlist with no `read` head-injection, while `discovery/skills.rs:273` tells the child to "use the read tool to load a skill's file"
- observable: an agent with an explicit `tools:` list plus any resolved skill silently cannot load it. The child is instructed to use a tool it does not have and the failure surfaces as a model apology rather than a config error.

**VL-S1 · No capability ceiling on child tools/agents/extensions** — *medium* · id retained, **class corrected to port bug** · area 09 `SUBA-021`
`src/runs/shared/capability-ceiling.ts:5`, `:95`, `:106` (209 lines) — present at **both** v0.43.0 and v0.47.1; env write and the `MCP_DIRECT_TOOLS="__none__"` forcing at `src/runs/shared/pi-args.ts:741-756` — vs `exec/mod.rs:1428`, the single workspace-wide mention, a comment reading "no capability ceiling in this port". `CAPABILITY_CEILING_V1` is one of the six upstream child env names with no cyrup counterpart (area 09 sweep 1). **Observable**: a grandchild inherits its parent's full tool/extension surface; upstream clamps monotonically and stamps the ceiling so the child cannot re-widen.

**VL-S2 · `workflowScript` runtime (and `chatProgress`)** — *large* · id retained, **class corrected to port bug** (first tag v0.41.0 ≤ baseline)
`src/workflows/scripted-workflow.ts:311` (`runWorkflowScript`, 502 lines) plus `src/workflows/chat-progress.ts` (140); tool params `src/extension/schemas.ts:317`, `:318` — vs `extension.rs:5327-5338` ("the identifier appears nowhere in this crate") and `missions/workflow_state.rs:26-30`. **Observable**: the model cannot express a dynamic workflow (`runs.run`/`runs.all`/`emit`/`state.get`/`state.set`); conversely cyrup still exposes the `tasks`/`chain`/`concurrency`/`chainDir` shapes v0.41.0 removed (that half is VL-S12). **Area 09's blind spot 2 is a warning about this entry specifically:** upstream deleted the entire task/chain execution surface at v0.41.0 and replaced it with `workflowScript`; at v0.43.0 the top-level schema has **no `task` key at all** and the whole model-facing tool description is workflowScript-centric. cyrup implements the v0.34.0-era surface. **This is not one item — it is a different execution model**, and its per-behaviour consequences (`runs.ref`, `emit`, per-child gates, `prompts.render`, `chatProgress`, retained-child `resume`, `children.list`) have never been decomposed by any pass. Treat area 09's count as a floor "by a wide margin" because of it.

**VL-S3 · Session lease — two runners can own one session file** — *medium* · id retained, class corrected (v0.35.0)
`src/runs/shared/session-lease.ts:9`, `:59`, `:208` (299 lines); acquired `subagent-runner.ts:4618`, released `:4648`; present at v0.47.1 — vs zero lease machinery anywhere in `crates/cyrup-ext-subagents/src` (area 09 `SUBA-023` re-confirms zero-hit). **Observable**: nothing prevents two runner processes writing one async session file concurrently, and there is no dead-owner reclaim on the next revival.

**VL-S4 · Process-terminal record — a killed runner leaves an ambiguous run** — *medium* · id retained, class corrected (v0.37.0)
`src/runs/background/process-terminal.ts:52`, `:163`, `:216` (280 lines); present at v0.47.1 — vs zero hits crate-wide; run state comes from `background/run_status.rs` and `background/reconcile.rs`. Area 09 `SUBA-023` adds the missing half: `TerminationOutcome` (`spawn/signal.rs:90-106`) carries only `status` + `stage`, with no `ExitStatus::signal()` name mapping. **Observable**: when a runner dies without writing a result, upstream still reports a definite terminal cause; cyrup can only report the reconciled "stale" guess, so `status` cannot distinguish a crash from a slow start.

**VL-S5 · Revival does not restore the child's effective config** — *small* · id retained, class corrected (v0.35.0)
`runs/background/async-execution.ts:1358` builds a `SteeringRecoveryDescriptor` and `:1401` persists it as `recovery-descriptor.json`; `async-resume.ts:276` reads it back and `:501-524` re-applies model, fallbackModels, thinking, tools, extensions, mcpDirectTools, systemPrompt, skills, completionGuard, memory, output, toolBudget and maxSubagentDepth — vs cyrup, which writes no descriptor and rebuilds the revived step with `model: None, tools: None, extensions: None` at `extension.rs:4269-4285`. **Observable**: a run launched with per-call `model`/`tools`/`toolBudget` overrides revives without them. *(Revival ITSELF is ported and works — `ResumeOutcome::RespawnFromTranscript` at `background/control.rs:1214` → `revive_from_transcript` at `extension.rs:4232`.)*

**VL-S6 · Herdr inspector subsystem** — *large* · id retained, class corrected (v0.41.0)
`src/inspectors/herdr/actions.ts:15` (`HERDR_INSPECTOR_ACTIONS`) and `:158`, plus `client.ts` (130), `inspector-runner.ts` (141), `project-panes.ts` (154), `src/integrations/herdr-status.ts` (330) — vs `tui/fleet.rs:1654` ("Herdr inspector controls are unavailable in this context."), the hard-coded `false` at `extension.rs:9863`, and no `inspector.*` verb in the enum at `extension.rs:6557`. **Observable**: the FleetView's advertised `H` key (footer at `tui/fleet.rs:2025`) always answers "unavailable".

**VL-S7 · Authority policy (confirm/forbid gates)** — *medium* · id retained, class corrected (v0.41.0) · now also area 09 `SUBA-064`
`src/policy/authority.ts:1-8` (`AUTHORITY_ACTIONS`), `:14-21` (defaults — discardWorktree/destructiveCleanup/spawnBudgetGrant default to `confirm`), `:23`, `:30`; consumed by `inspectors/herdr/actions.ts:205-206` (`allowSteer`/`allowStop`) and validated at `src/extension/config.ts:26` — vs `extension.rs:7574` (a doc line naming upstream's dispatch arm) and no `authorityPolicy` config key anywhere. **Area 09 sharpened this**: the `stop`/`steer` gate it drives is **live-reachable** in cyrup today, so the missing policy is not merely unconfigurable — it is an unguarded live path. Scope caveat: upstream's `discardWorktree` gate hangs off a `worktree.discard` action cyrup does not have, so that arm has nothing to attach to yet. **`SUBA-064` stays medium only because of that caveat — it becomes critical the day `worktree.discard` or `destructiveCleanup` lands**, and its Fix now carries that as a hard prerequisite.

**VL-S8 · Wait tool is still `wait`; no non-blocking subscriptions, no auto-drain** — *large* · id retained, class corrected (v0.35.0/v0.41.0)
`src/runs/background/wait-tool.ts:9` (`name: "subagent_wait"`), backed by `subagent-wait.ts` (651), `wait-config.ts` (36) and `auto-drain.ts` (67) at v0.35.0 plus `wait-subscriptions.ts` (253) at v0.41.0 — vs `extension.rs:6704` (`WAIT_TOOL_NAME: &str = "wait"`) and zero auto-drain/subscription hits. **Observable**: a child prompted by upstream's tool description calls `subagent_wait` and gets "unknown tool"; there is no `{id, nonBlocking:true}` wake subscription, and a headless run does not auto-drain at `agent_end`, so results can be lost when the turn ends. Related residuals now filed in area 09: `SUBA-034` (event-bus wake unported; pure polling at a 1 s floor), `SUBA-031` (`wait` scopes by cwd not session), `SUBA-056` (durable completion replay).

**VL-S9 · `usageBudget`** — *small* · id retained, **class corrected to port bug** · area 09 `SUBA-021`
`src/runs/shared/usage-budget.ts:14`, `:44`, `:61` (65 lines) — present at **both** v0.43.0 and v0.47.1; tool param `src/extension/schemas.ts:330` — vs zero hits in the crate and no such key among the 45 schema properties. **Observable**: a run cannot be capped by cost/token spend.

**VL-S10 · Parallel worktree handoff manifests** — *medium* · id retained, class corrected (v0.36.0) · area 09 `SUBA-024`
`src/runs/shared/parallel-handoff.ts:74`, `:158`, `:162`, `:183` (238 lines), present at v0.47.1; `handoffPath` tool param at `src/extension/schemas.ts:274` — vs `spawn/parallel.rs` (no manifest writer) and three incidental mentions only. **Observable**: after a parallel run with `worktree: true` there is no handoff manifest, no `handoffPath` to hand preserved worktrees to a follow-up, and no `discardPreservedWorktrees` cleanup — the branches are left for the user to find by hand.

**VL-S11 · Three slash commands missing: `/subagents`, `/subagents-refine`, `/subagents-detach`** — *medium* · id retained, class corrected (v0.35.0/v0.43.0/v0.39.0) · area 09 `SUBA-026` (partially closed: `/subagents-stop` landed)
`src/slash/slash-commands.ts:651`, `:701`, `:724`; the admin surface is `src/slash/subagents-admin.ts` (432 lines) — vs the 16-variant match at `registration/slash_commands.rs:127-145`, which has none of the three. **A fourth is now known**: `/subagents-guide`, filed separately as area 09 `SUBA-066` because it sits outside both this entry and `SUBA-055`. **Observable**: no interactive admin surface for an agent's model/thinking/prompt, no way to detach a live foreground run from a slash command, no refinement overlay generation.

**VL-S12 · Four slash commands upstream deleted at v0.41.0 are still registered** — *small* · **reverse lag**, not a port bug and not lag
`git grep -oh 'registerCommand("[a-z-]*"' <tag> -- src` gives 19 unique names at v0.40.0 including `chain`, `parallel`, `run-chain`, `chain-prompts`, and 15 at v0.41.0 with all four gone (still gone at v0.43.0 and v0.47.1) — vs `registration/slash_commands.rs:128` (`Chain`), `:129` (`Parallel`), `:130` (`RunChain`), `:142` (`ChainPrompts`). **Observable**: cyrup's palette advertises four commands upstream no longer has, whose function moved into `workflowScript` (VL-S2). Do not delete them before VL-S2 lands or the capability disappears entirely.

**VL-S13 · Agent refinement WRITE half** — *medium* · id retained, class corrected (v0.43.0)
`src/agents/agent-refinements.ts:349` (`collectBoundedRefinementEvidence`), `:448` (`validateRefinementProposal`), `:546` (`handleRefinementAction`) — vs `exec/agent_refinements.rs:12-20`, which states the port is the read half only, and no `refine*` verb in the enum at `extension.rs:6557` (area 09 counts three such verbs missing). **Observable**: an overlay written by upstream (or by hand) is applied correctly at spawn (`exec/mod.rs:1565`), but cyrup can never generate or roll one back.

**VL-S14 · `runner: external-cli` agents unsupported** — *medium* · id retained, class corrected (v0.41.0)
`src/runs/shared/external-cli-runner.ts:12`, `:26`; `src/api/external-runs.ts` (129 lines); refusal text `runs/foreground/subagent-executor.ts:5023` — vs `discovery/frontmatter.rs` (no `runner` key) and `discovery/types.rs` (no `runner`/`external` field); the sole trace is a doc citation at `background/runner_main.rs:4020`. **Observable**: `runner: {type:'external-cli'}` in frontmatter parses as if absent and the agent is launched as an ordinary cyrup re-exec instead of shelling out to the declared CLI (or being refused, as upstream does for foreground/clarify).

**VL-S15 · Native extensions cannot register a keyboard shortcut** — *small* · **host-seam gap, not upstream lag** · see also §2 UW-7, area 06 `EXT-039`
`src/slash/slash-commands.ts:719-722` (`pi.registerShortcut(Key.ctrlAlt("f"), … showFleet(ctx))`) — vs `crates/cyrup-ext/src/native.rs:240-297` (`InitApi` exposes `subscribe`, `register_tool`, `register_command` and three renderer registrations; no `register_shortcut`). The WASM-guest path HAS one (`cyrup-ext/src/host/live.rs:98`), which proves the seam can carry it. **Observable**: the fleet inspector opens only by typing `/subagents-fleet`; Ctrl+Alt+F has no counterpart, and the same limit blocks every other native-extension shortcut (`crates/cyrup-intercom/src/extension.rs:465` records the identical complaint). Note area 11's correction: `ui/mod.rs:12-19`'s rationale is now **half stale** — `register_message_renderer` DOES exist at `native.rs:270`; only `register_shortcut` is missing.

### 1c. From `pi-permission-system` v0.7.1

**PB-32 · `should_expose_tool` keeps `bash` advertised under a tool-level deny — and the allow-listed command executes** — ***critical*** · = area 10 `PERM-009` (raised from medium this pass; **now the first row of area 10's table**)
- upstream: `shouldExposeTool` has a read/skills bypass and **nothing else** at **both** tags — `src/index.ts:2049-2075` @v0.7.1 (the ported baseline, which governs the classification) and `:1790-1816` @v0.8.0. There is no bash branch at either. cyrup's in-tree citation of `index.ts:2049-2075` turns out to be the correct v0.7.1 offset
- cyrup: `crates/cyrup-permission-system/src/extension.rs:1651-1653` adds `if tool_name == "bash" && mgr.get_bash_permissions(agent_name).any_allow() { return true; }`, with a justification comment at `:1624-1631`. And `manager.rs:205-215` resolves a bash **command** rule above the tool-level state — its own comment says "command rules OUTRANK the tool-level bash fallback"
- observable: `tools.bash: deny` + `bash: {"git status": allow}` hides bash in pi; in cyrup it leaves bash **advertised to the model AND executes the command**. A configured deny is defeated silently, in both directions. This is an **in-baseline parity bug**, not drift. No test pins the divergence (`tests/context_hygiene.rs:128-152` denies `write`), so the suite goes green on the deletion. Effort **S**: delete the branch and its comment, refresh the citation.

**PB-15 · Model-option compatibility guard (temperature stripping) is entirely unported** — *medium* · = area 10 `PERM-012`
- upstream: `src/model-option-compatibility.ts:62` @v0.8.0 (`getUnsupportedTemperatureReason`), `:126` (`ensureModelOptionGuardForApi`), `:164` (`registerModelOptionCompatibilityGuard`); wired as the **first statement** of the `session_start` handler, `src/index.ts:1829`. `git diff v0.7.1..v0.8.0 -- src/model-option-compatibility.ts` is **empty** and v0.7.1 `index.ts:2088` makes the same call — this predates the ported baseline
- cyrup: `crates/cyrup-permission-system/src/extension.rs:1991` (the `HostEvent::SessionStart` arm) registers no guard; `cyrup-provider/src/api/openai_responses.rs:359-361`, `openai_codex_responses.rs:707-708` and `azure_openai_responses.rs:386-387` insert `temperature` unconditionally; `grep -rn "does not support temperature" crates/` = 0
- observable: with the permission system installed, pi strips `temperature` for openai-codex-responses, the openai-codex provider, any `codex`-tokened model id, and any reasoning model on openai-/azure-openai-responses; cyrup sends it (its own test at `openai_codex_responses.rs:1515` asserts `body["temperature"] == 0.25`), so those requests are rejected or silently mis-parameterised. Not blocked: `cyrup-session-svc/src/guest_providers.rs` already exposes `register_provider`/`unregister_provider`.

**PB-16 · Permission-request events are never emitted — native extensions cannot reach the event bus** — *medium* · = area 10 `PERM-011` half B (area 10 merged PB-16 and UW-9 into one item)
- upstream: `src/index.ts:150` @v0.8.0 (`PERMISSION_REQUEST_EVENT_CHANNEL`), `:1518-1529` (`emitPermissionRequestEvent`), `:1531-1548` (`emitPermissionStateEvent`), fired `:1606`, `:1612`, `:1626`. Present at v0.7.1 (`:137`, `:1753-1755`, fired `:1825`/`:1844`/`:1871`)
- cyrup: `grep -rn "events.emit|emit_event|permission-request" crates/cyrup-permission-system/src` = 0. **The bus itself exists** — `SharedBus` at `cyrup-ext/src/host/services.rs:988` (`subscribe` `:1002`, `emit` `:1010`, `take_pending` `:1018`), fanned out by `cyrup-ext/src/facade.rs:1003-1026` — but it is wired to WASM guests only (`host/live.rs:642`, `:650`) and `cyrup-ext/src/native.rs` contains **zero** `bus` references
- observable: in pi any extension can subscribe and observe every waiting/approved/denied transition with its requestId, tool, command, target and agent; in cyrup no such stream exists. The work is a native-extension bus accessor (area 06 owns the seam) plus three emit sites at `extension.rs:1384-1469` — **not** a new subsystem.

**PB-17 · The forwarding half of the security-review audit trail is unwritten** — *medium* · = area 10 `PERM-008`, which sharpened the count to **8 review + 3 debug sites** — **CLOSED; and the census was still short.** `PERM-008` landed the mechanism and **eleven** call sites; upstream has **~28**, and the sixteen missing ones (the fs helpers, both reader diagnostics, and **both response-binding rejections**) were ported by sweep 6 as **`PERM-033`** — filed and closed 2026-08-14. The security-relevant one: a forged or misaddressed forwarded response was discarded leaving only an all-null `response_received` entry, so nothing named it.
- upstream: `src/index.ts` @v0.8.0 has the `writeReviewEntry` definition at `:200` plus 17 call sites; the forwarding path is `:735`, `:1032`, `:1058`, `:1080`, `:1173`, `:1184`, `:1187`, `:1228`. All eight exist at v0.7.1 (`:1011`, `:1019`, `:1298`, `:1324`, `:1346`, `:1417`, `:1428`, `:1473`)
- cyrup: `crates/cyrup-permission-system/src/forwarding.rs` is 1125 lines and `grep -n "logger|review|tracing"` returns **zero**; both entry points (`wait_for_forwarded_approval` `:398`, `process_forwarded_requests` `:528`) log nothing. `write_review_entry` is at `extension.rs:930` with 6 calls
- observable: a forwarded child ask that times out, expires, is auto-approved or is denied leaves no audit record, and every forwarding I/O failure is silent where pi writes `permission_forwarding.error`.

**PB-18 · `/permission-system` prints text instead of opening the settings modal** — *medium* · = area 10 `PERM-007`
- upstream: `src/config-modal.ts:63-122` @v0.8.0 (`openPermissionSystemSettingsModal` — `ctx.ui.custom<void>` overlay at `:66` with a live `onChange`→`setConfig` loop), registered at `src/index.ts:1502-1512`. Same shape at v0.7.1
- cyrup: `extension.rs:1038` (`run_permission_system_command`) parses `<setting> <value>` and returns a `String`. **The in-tree rationale ("HostServices exposes no custom-overlay seam") was STALE and has since been corrected in place — `extension.rs:998` now says "One exists:" and names it.** The seam: `HostServices::open_overlay` is `cyrup-ext/src/host/services.rs:254`, implemented at `cyrup-session-svc/src/host_services.rs:1043`, driven by `App::on_overlay_request` (`cyrup-tui/src/app/run_arms.rs:406-430`, which pushes onto `state.overlays` at `:427`), and already consumed in production by `cyrup-ext-subagents/src/extension.rs:11376` — with no `cyrup-tui` dependency in the consuming crate *(all five citations re-resolved by symbol 2026-08-19; the `app/events_fold.rs:441` one was a bad `40821ed` remap and lands on a doc comment)*
- observable: `/permission-system` opens a live two-row modal in pi; in cyrup it prints a status paragraph and the user must retype `/permission-system debug on`. A straight port onto an existing seam. Area 10 adds the missing companion: `PERM-024` (config not refreshed on `before_agent_start`) is what makes the modal feel instantaneous once it lands.

> **Area 10's second fail-open is not a port bug and so is not in this section**: `PERM-023` (*high*,
> `cyrup-original`) — `is_installed` (`extension.rs:2159-2175`) probes env, policy file and
> `config.json` and never `agents_dir`, which `manager_paths_for` (`:390-401`) wires and
> `manager.rs:500-503` enforces. An operator whose only policy artifact is agent-markdown
> frontmatter gets no extension attached and silently inert deny rules. See §5.

### 1d. From `pi-intercom` v0.9.2

> **The ported baseline was recorded wrong in every prior document.** A citation census over
> `crates/cyrup-intercom/src` returns **v0.9.2 × 272**, v0.7.0 × 14, v0.8.0 × 3, v0.6.0 × 1 (the
> `lib.rs` banner), v0.10.x × 0. Load-bearing v0.8.0/v0.9.x code is present **and tested**:
> `broker/runtime_claim.rs`, `/intercom-id`, `format_context.rs`, the 16-tag `BrokerMessage` union,
> the v0.9.2 envelope with `#[serde(flatten)] extra`, and `transport/target.rs` + `stream.rs`.
> **The true baseline is v0.9.2.** Two consequences: (a) the drift window is two minor versions, not
> three — §3c; (b) **`VL-I1…VL-I6` were never version lag.** They are in-baseline port bugs and move
> here, keeping their ids. Area 11 maps all six and confirms all six still open.

**PB-19 · The broker binds a Unix socket unconditionally and never consults its own listen-target resolver** — *low* **(severity corrected down)** · = area 11 `ICOM-015`, now correctly **partial**
- upstream: `broker/broker.ts:21` @v0.7.0 (`const LISTEN_TARGET = getBrokerListenTarget();`) with the two-branch listen at `:176-179`; helper `broker/paths.ts:107`, Windows named pipe `paths.ts:65-74`, TCP predicate `paths.ts:44-59`. At v0.9.2 the broker additionally publishes its endpoint (`broker/broker.ts:252-256`, `stateId: BROKER_STATE_ID`) and enforces it at `:408-409`
- cyrup: `crates/cyrup-intercom/src/broker/mod.rs:1243` — `let listener = UnixListener::bind(&socket_path)?;`, unconditional; `:24` imports only `tokio::net::UnixListener`; `paths.rs:6-8` records the deferral. The ported resolver `broker_listen_target` (`transport/target.rs:278`) has **zero callers of any kind**. **The CLIENT half is fully live**: `broker_connect_target` (`target.rs:254`) is called from `transport/spawn.rs:226` and `:299`, and `transport/client.rs:202` handles all three transports
- observable: with `CYRUP_INTERCOM_TRANSPORT=tcp` (or on Windows) a cyrup client resolves a TCP/pipe endpoint while a cyrup broker only ever listens on a Unix socket and never writes `broker.port.json` — the two halves cannot meet. Severity is *low* because the reachable configuration is Windows or an explicit env override; see OQ-3, which decides how much of this survives.

**PB-20 · The bundled `pi-intercom` skill is not shipped or registered** — *medium* · = area 11 `ICOM-004`
- upstream: `pi-intercom/skills/pi-intercom/SKILL.md` — 514 lines at v0.9.2 (513 at v0.7.0; **rewritten by 164 lines at v0.10.0**), declared at `package.json:26-28` (`"pi": { "skills": ["./skills"] }`)
- cyrup: `find crates/cyrup-intercom -type f ! -name '*.rs'` returns only `Cargo.toml`; `init` at `extension.rs:457-495` registers 2 tools and 2 commands, subscribes 8 event kinds, never subscribes `EventKind::ResourcesDiscover`, and never registers a skill
- observable: a pi session with intercom installed gets a coordination-protocol skill the model can load; a cyrup session has none. Not blocked: `EventKind::ResourcesDiscover` exists (`cyrup-ext/src/event.rs:20`), is dispatched via `facade.rs:485`, and `cyrup-permission-system` already subscribes to it (`extension.rs:1900`). **Port the v0.10.0 text, not the v0.9.2 text.**

**PB-21 · The session-name poll timer is unported; `CYRUP_INTERCOM_NAME_POLL_MS` is inert** — *medium* **(severity corrected up)** · = area 11 `ICOM-006`
- upstream: `index.ts:598-611` @v0.7.0 (`startNamePoll`, `setInterval` `:601`, interval from `getNamePollMs()` `:609`; helper `:421-429`). Same shape at v0.9.2 (`index.ts:461`, used `:829`)
- cyrup: `identity.rs:24` declares `ENV_INTERCOM_NAME_POLL_MS` and **its declaration is its only occurrence**; `transport/client.rs:368` (`update_presence`) has no production caller; the only live presence path is `update_presence_with_context` from `sync_presence` (`extension.rs:205`), which **hard-codes `name: None`**. The label is sent once at connect (`connect.rs:444`)
- observable: renaming a session never updates its presence label for peers — other sessions' `/intercom` listings keep the old name until the client reconnects. Setting the env var does nothing.

**VL-I1 · Broker has no mailbox** — *medium* · id retained, **class corrected to port bug** · = area 11 `ICOM-010`
`broker/broker.ts:40-41` (24 h retention / 256 messages), `:219` (`mailboxMessages`), `:775`/`:1002` (`queueMailboxMessage`), `:992` (prune), `:1020` (`flushMailboxForSession`, called on register at `:510`), `:1110` (`findDisconnectedSessions`) — vs `broker/mod.rs:792-797`, which replies `DeliveryFailed{reason:"Session not found"}` the moment no LIVE session resolves; absence documented at `:590`, `connect.rs:46`, `reply_tracker.rs:268`. **Observable**: sending to a named session that just restarted is lost; pi queues and delivers on reconnect.

**VL-I2 · Message receipts, receiver-side dedupe and delivery metadata** — *large* · id retained, class corrected · = area 11 `ICOM-017` (+ `ICOM-048`, `ICOM-050`)
`types.ts:49-56` (`MessageReceipt`); `index.ts:446` (`formatInboundDeliveryMetadata`), `:503`, `:515` (dedupe), `:533` (`emitMessageReceipt`), `:564`, `:588`, fired `:880`–`:974`; `broker/client.ts:773`; broker routes `broker/broker.ts:698`, `:773`, `:809`, `:1053` — vs `inbound.rs:347-386` (no timestamps stamped, no `(from.id, message.id)` dedupe, no receipt), `transport/client.rs:635-640` (decodes and `tracing::debug!`s only), `broker/mod.rs:447-456`, `session_state.rs:295-298`. The envelope fields themselves ARE modelled (`transport/protocol.rs:301-347`). **Observable**: a pi peer sending to cyrup gets no receipts, so its `ask` timeout reports an unknown delivery state; a duplicate message id is injected twice; and **cyrup's injected body omits the `_deliveryMetadata_` line entirely, which is why `replyTo` is unreachable without a second tool call** (area 11 `ICOM-048` — land it with `ICOM-043`).

**VL-I3 · No cancel / supersede / retry controls** — *medium* · id retained, class corrected · = area 11 `ICOM-017`
`index.ts:1795` (8-action enum including `cancel`), `:1813`/`:1816`/`:1819` (`messageId`/`supersedes`/`retryOf`), `:1927` (the `cancel` case), `:551-562` (`handleMessageControl`); `broker/client.ts:738` (`cancelMessage`); `broker/broker.ts:642` (supersede validation), `:822-866` (cancel) — vs `tools/intercom.rs:388` (six actions), `:26-36` (`IntercomParams`), `:315` (unknown-action error), `transport/client.rs:46-57` (`SendOptions` without supersedes/retry_of/sender_sequence), and `broker/mod.rs:595-607`, where `handle_cancel_message` **always** answers `DeliveryFailed{"Message cannot be cancelled by this session"}`. **Observable**: the model cannot cancel or supersede an in-flight message, and a pi peer's cancel/supersede against a cyrup session is silently discarded.

**VL-I4 · Extension bus: frames validated then dropped** — *large* · id retained, class corrected · = area 11 `ICOM-016`
`types.ts:1` (`EXTENSION_BUS_FEATURE`), `:80`/`:88`/`:96`, `:115`-`:131`; `extension-api.ts` (44 lines); `broker/extension-state.ts` (186 lines — persisted, sha256-checksummed, 64 KiB-capped, optimistic revisions); `broker/broker.ts:505` (`features: [EXTENSION_BUS_FEATURE]`), `:509` (owner election); `broker/client.ts:216` (`supportsFeature`) with gates at `:648`/`:817` — vs `broker/mod.rs:419-425`, routing the three frames to validation-only handlers (rationale `:460-463`), and `transport/client.rs:575`, which discards the `features` field the protocol models at `protocol.rs:767`. **Observable**: a pi extension registering an intercom namespace gets owner election, cross-session publish and a durable revisioned state store; against a cyrup broker it is told no feature is supported and any forced frame is dropped with no reply. *(Area 11 blind spot 5: neither upstream file was read this pass, so the fix sketch is directional.)*

**VL-I5 · No restart-stable intercom session id** — *small* · id retained, class corrected · = area 11 `ICOM-011`
`config.ts:38-39` (`stableId`) with fail-closed validation `:141-150`; `index.ts:39` (`STABLE_INTERCOM_SESSION_ID_ENV`), `:409-411`, consumed `:1264` — vs `config.rs:33-48` (7 fields, no `stable_id`) and `connect.rs:377-382`. `grep -rni 'stable_id|stableId'` over `src/` and `tests/` returns **zero**. `/intercom-id` itself IS ported (`extension.rs:474-481`). **Observable**: a cyrup session's intercom address changes on every restart, so peers holding the old id can no longer reach it. *(Area 11's repair-pass spot-checks confirm this is the **only** behavioural absentee in both the config-key diff — pi's 9 `IntercomConfig` members vs cyrup's 7 — and the env-var diff. Warning recorded there: the nine `INTERCOM_*` identifiers in `broker/paths.ts` and `extension-api.ts` are internal constants, not env vars; a name-grep that treats them as such reports a false gap.)*

**VL-I6 · No `list-cwd` action** — *medium* · id retained, class corrected · = area 11 `ICOM-018` (shares `cwd.rs` with `ICOM-042` — port it once)
`cwd.ts:13-27` (`normalizeCwd`) and `:29-31` (`sameCwd`); `index.ts:25`, `:1783-1784`, `:1795`, `:1822-1824`, `:1874-1925` (the case, with the "your session's cwd has N peers" fail-loud note at `:1901-1908`) — vs `tools/intercom.rs:388` and `:26-36`; no `cwd.rs` in the crate. **Observable**: a cyrup agent cannot ask for peers in a given working directory; it must call `list` and eyeball paths. *(Do NOT also claim symlink-normalization breakage: pi's own session list compares raw strings exactly as `ui/session_list.rs:153` does.)*

### 1e. Reverse lag — cyrup carries behaviour upstream changed or deleted (14 items, `stale-port`)

Not port bugs and not lag; a third shape that still costs behaviour. The named instances:
`VL-S12` (four deleted slash commands), area 01 `PROV-033` (`sendSessionIdHeader`, which pi **deleted**
in #6496 with a documented migration to `sessionAffinityFormat: "openai-nosession"` —
`packages/ai/CHANGELOG.md:168` — so `x-session-id` is now unreachable on openai-responses),
`PROV-016` / `PROV-019` (validation and `max_output_tokens` behaviour that drifted from a byte-identical
upstream), `PROV-039` / `PROV-041` (provenance comments asserting things that are no longer true),
`AGENT-017`, `SESS-025` / `SESS-027`, `EXT-028` / `EXT-036`, `TUI-025`, `SEAM-029`, `ICOM-012`, and —
new to this class this pass — area 12 `DRIFT-016` (`Current date:` still injected into the system
prompt; `git grep 'Current date' v0.83.0 -- packages/coding-agent/src` returns **nothing**, so it is
cyrup carrying something upstream does not have, not lag). The recurring sub-shape is **an in-tree
comment that documents a divergence the code no longer has, or a rationale that a later commit
invalidated** — six of the fourteen. Fix the comment in the same change as the code, always.

---

## 2. Unwired — the code exists in cyrup and has no production caller

**This is the project's most common defect class and the cheapest to fix.** Batches 8–10 shipped
roughly forty such items — all with green tests, because the tests called the functions directly. **A
green suite is not evidence that a subsystem runs.** Every entry here is a wiring job, not a port.

A refinement this pass earned, recorded by area 05 and worth generalising: **a `/settings` row is not
a consumer.** The previous sweep's "has a consumer" test let `doubleEscapeAction` through because it
was rendered in the settings list — and nothing else read it.

**UW-1 · The native modifier probe has no production caller, so the Apple-Terminal Shift+Enter rescue never fires** — *medium*
- upstream: `pi/packages/tui/src/native-modifiers.ts:21-56` (`loadNativeModifiersHelper` loads the prebuilt darwin/win32 addon), consumed at `packages/tui/src/terminal.ts:6` and used at `:324`
- cyrup: `crates/cyrup-tui/src/native_modifiers.rs:62` (`set_native_modifier_probe`) — the only call workspace-wide is `crates/cyrup-tui/src/tests/native_shift_enter.rs:138`. The consumer side IS wired: `app/input_reader.rs:403` calls `is_native_modifier_pressed` on the production `map_event_on` path *(`app/settings_rows.rs:110` was a bad `40821ed` remap — that line is an idle-timeout description)*
- observable: with no probe installed the predicate always answers false, so on macOS Apple Terminal Shift+Enter still submits instead of inserting a newline — the exact defect the ported code exists to fix. Mechanism note: pi `require`s a prebuilt `.node` addon; cyrup needs an OS query (`CGEventSourceKeyState`/`GetKeyState`), which is FFI and cannot live inside `#![forbid(unsafe_code)]` `cyrup-tui` — the injectable seam exists precisely for that and is fed by nothing. *(Area 07 did not restate this item; `native_modifiers.rs` is one of fifteen files that did not exist at the older baseline. The citations above are at `04c1ba2`.)*

**UW-2 · The first-run setup wizard is gated but never invoked — the `if` body is empty** — *small* · **observed 2026-08-13** (live-terminal; [`REPRO-LOG.md`](REPRO-LOG.md)) · **FIXED 2026-08-13**
- **FIXED 2026-08-13, per ADR-0011 (which decided `OQ-9` / `PARITY-GAPS` §6 q6 — *not* `OQ-6`; the escalation below is one of the two mis-citations ADR-0011 records).** `crates/cyrup/src/main.rs` now calls `cyrup::run_first_time_setup(&theme, &mut startup_settings, detected)?` at **pi's position** — after `startupSettingsManager` + its diagnostics, **before** `apply_settings_session_dir` — mirroring `main.ts:610 → 615-617 → 625-630` and pi's stated reason ("Runs before any runtime services are created so the chosen settings apply everywhere"). The condition carries pi's `listModels` conjunct (`mode == Interactive && cli.list_models.is_none() && should_run_first_time_setup(...)`); `!parsed.help` needs none because `main.rs` prints help and returns upstream of the gate, and the comment says so. `detected` is `detect_terminal_theme_for_auto(&StdinTerminalProbe, 100ms, $COLORFGBG)` — pi's `detectTerminalThemeForAuto({ ui, timeoutMs: 100 })`, startup-ui.ts:180 — and the theme is that detected polarity (`UiTheme::light()`/`dark()`), not `UiTheme::default()`, because on a first run there is by definition no `settings.json` for pi's `createStartupTui` to resolve. Nothing was deleted. The false comment at the old `main.rs:215-217` is gone.
- **Test** `crates/cyrup/tests/first_time_setup.rs` — its header no longer claims the wizard is unexercised-and-unreachable; it now states that the production caller exists and lists exactly what a live pty run must show (Light + "Don't share" → `"theme":"light"`, `"enableAnalytics":false`, **no** `trackingId`; opt-in → non-empty `trackingId`; Escape at either step → **no** `settings.json`; relaunch → no wizard; `--list-models gpt` on a TTY → the model list, no wizard; `CYRUP_AGENT_DIR` set → no wizard). The one clause a unit test CAN pin is added: `a_list_models_run_is_still_interactive_so_the_gate_needs_the_list_models_conjunct` proves `resolve_app_mode` answers `Interactive` for `--list-models gpt` on a TTY, which is why the conjunct is not optional. **The wizard APPEARING cannot be asserted from `cargo test`** — it is a `CrosstermBackend` surface — so this fix is *not* live-verified, and that live run is the outstanding evidence.
- **Also strike the trap-list entry** at `docs/gap-analysis/README.md` (done in the same pass): a wrong trap is removed, not downgraded.
- upstream: `pi/packages/coding-agent/src/main.ts:615-616` @v0.83.0; unchanged at v0.84.1 `main.ts:663-664`
- cyrup: `crates/cyrup/src/main.rs:218-223` evaluates `should_run_first_time_setup` and the body (`:221-222`) is comment-only; `crates/cyrup/src/startup.rs:256` (`run_first_time_setup`) has zero callers
- observable: on a first run with `CYRUP_EXPERIMENTAL=1`, no `settings.json` and no agent-dir override, pi presents the theme + analytics wizard and persists the answers; cyrup does nothing. **The gate can fire**: `OFFICIAL_PACKAGE_NAME`/`APP_NAME`/`CONFIG_DIR_NAME` (`startup.rs:32-34`) name cyrup itself and match the live values at `:38-43`, so `is_official_distribution()` (`:71-73`) is **true** for this build. This is the one place where the standing "deliberately unreachable first-run wizard" trap and the code disagree — the comment at `main.rs:215-217` and CLAUDE.md's "compile-time constant `false`" are both stale. Escalated to **OQ-6**.
- **observed 2026-08-13, live terminal, and the trap list is now settled on evidence rather than on a read.** With `CYRUP_EXPERIMENTAL=1`, no `settings.json` and the *default* agent dir, the binary goes straight to the interactive TUI: no theme picker, no analytics question, and no `settings.json` written. Two independent corroborations that the gate's inputs were **all true in that very process**: the footer printed the `xp` experimental badge (`crates/cyrup-tui/src/status.rs:356`, pi `footer.ts:162-164`), proving `CYRUP_EXPERIMENTAL=1` was read; and the agent dir ended the run containing only `models-store.json.lock`, proving `settings_path` did not exist. The wizard is also **not** broken-but-invisible: the sibling pre-launch selectors (trust prompt, resume picker) rendered fine on the same pty in the same pass, so a wizard that ran would have been seen. **The standing trap-list entry "the deliberately unreachable first-run wizard" is wrong and should be struck**; OQ-6 is a live product decision, not a documentation cleanup.

**UW-3 · Child-watchdog NDJSON status events are never read by the parent** — *medium* · confirmed at HEAD by area 09
- upstream: `pi-subagents/src/runs/foreground/execution.ts:846`, `:848`, `:857`, `:585`; `runs/background/subagent-runner.ts:626`, `:628`, `:640`, `:831`, `:2711-2712`; definitions `src/watchdog/child-status.ts:167`, `:181`, `:186`
- cyrup: `watchdog/child_status.rs:480`, `:497`, `:516` — production callers zero. The two readers that should call them (`exec/ndjson.rs`, `background/`) contain no watchdog reference. The child EMIT side is wired (`prompt_runtime.rs:1701`; `exec/mod.rs:1727-1737`)
- observable: a child mid-watchdog-review when its agent settles is killed by the ordinary final-drain timer instead of held open by the watchdog tail timer, so its blocker/concern warnings are lost. cyrup emits `subagent.watchdog.status` frames its own parent discards as an unknown event type. `child_status.rs:461-473` states this in-tree.

**UW-4 · Watchdog review never runs a model turn — every review is silently clean** — *medium* · confirmed at HEAD
- upstream: `src/watchdog/review.ts:295` — `await agent.prompt(buildReviewPrompt(request, selection))` inside `createMainWatchdogReview` (`:249`)
- cyrup: `watchdog/review.rs:871` (`NoTurnReviewAgent`) whose `run` returns `Ok(Vec::new())` at `:876`; it is the agent bound in BOTH production paths — `watchdog/register_main.rs:169` and `prompt_runtime.rs:1761`
- observable: the whole `watchdog/` subtree is wired — 18 modules, `register_main_watchdog` at `extension.rs:9055`, nine subscriptions `:9338-9352`, `/subagents-watchdog` `:9326`, four `watchdog.*` verbs `:7567-7570` — and a review model is resolved on every agent-end boundary, but **no warning can ever be emitted**. `/subagents-watchdog status` reports "real model review" (`register_main.rs:191`) over a machine that cannot produce a finding. **This is the canonical example of "closing a not-implemented item means the subsystem exists, not that it is correct."**

**UW-5 · Watchdog permission arbiter never runs a model turn — every `ask` denies** — *medium* · confirmed at HEAD
- upstream: `src/watchdog/permission-arbiter.ts:41` (`createWatchdogPermissionArbiter`) constructing `new Agent({… streamFunction })` at `:102`, exported as `requestWatchdogPermission` at `:145`
- cyrup: `watchdog/permission_arbiter.rs:587` (`NoDecisionPermissionAgent`, returning `Ok(None)` at `:595`), bound at `prompt_runtime.rs:1734` — the sole production construction of the gate
- observable: a child tool whose policy tier is `ask` is denied with the `malformed` reason ("Watchdog permission arbiter returned no decision.", `permission_arbiter.rs:734`). Fail-closed is the right direction, but no `ask`-tier tool can ever succeed inside a subagent.

**UW-6 · Nothing ever ships a permission policy to a child — the child-side gate is inert** — *medium* · confirmed at HEAD
- upstream: `src/runs/shared/permissions.ts:40` (`resolvePermissionRules`) and `:51` (`encodePermissionRules`), written into the child env at `src/runs/shared/pi-args.ts:730` and `:758`
- cyrup: `exec/mod.rs:1376` (`build_attempt_spawn_plan`) builds the child env overlay through `~:1874` — structured-output vars, `TOOL_BUDGET_ENV` (`:1840-1846`), steer inbox, supervisor channel, required child tools — but never `CYRUP_SUBAGENT_PERMISSION_POLICY` / `…_PERMISSION_AUDIT_PATH` (defined at `watchdog/permission_arbiter.rs:358`, `:361`). The only writer workspace-wide is a test stub at `prompt_runtime.rs:1949-1950`; the reader is `prompt_runtime.rs:1716-1717`
- observable: `with_permission_gate` always receives `None`, so no child tool is ever checked against agent/config permission rules and no `permission.request`/`permission.decision` audit record is ever appended. `permission_arbiter.rs:56-63` admits this in-tree.

**UW-7 · The fleet-status widget receives no keystrokes** — *medium* · confirmed at HEAD
- upstream: `src/tui/fleet-status.ts:282-283` (`ui.onTerminalInput((data) => this.handleKey(data))`), handler `:352`
- cyrup: `tui/fleet_status.rs:764` (`handle_key`) has zero production callers, as do `press` (`:1169`) and `is_widget_registered` (`:711`). The host has no seam to wire it to: `cyrup-ext/src/host/services.rs` has `set_widget` (`:260`) and `open_overlay` (`:224`) but nothing resembling `on_terminal_input`. The PUBLISH half is live (`extension.rs:9489`, `:9889`, `:9978`)
- observable: while subagents run, pressing ↓ or ← on an empty editor expands pi's widget into a selectable roster whose Enter opens the fleet inspector; in cyrup the widget is display-only and those keys fall through to the editor. Same missing seam family as VL-S15 and area 06 `EXT-021`.

**UW-8 · Mission workflow state is never written** — *small* · confirmed at HEAD
- upstream: `src/missions/workflow-state.ts:17` (`missionStatePath`); its only consumer is the `workflowScript` runtime at `runs/foreground/subagent-executor.ts:4139`, which exposes `state.get`/`state.set`
- cyrup: `missions/workflow_state.rs:209` (`get`) and `:222` (`set`) have no non-test caller. The **read** half IS live — `mission_state_path` is called from `missions/goal_driver.rs:372` and `missions/actions.rs:908`
- observable: `<missionDir>/<missionId>/state.json` is never written, so `goal_driver`'s production read always finds a missing file and falls through to the decisions list, and `mission.show` advertises a path that never exists. **Closes for free the moment VL-S2 lands.**

**UW-9 · The yolo-mode runtime API has no publish seam and no caller** — *medium* · = area 10 `PERM-011` half A
- upstream: `src/index.ts:1480-1484` @v0.8.0 (`registerPiPermissionSystemRuntimeApi({getYoloMode,setYoloMode,toggleYoloMode})`); `src/yolo-mode-api.ts:23-29` publishes it on `globalThis.__piPermissionSystem`, `:40-43` reads it back. `git diff v0.7.1..v0.8.0 -- src/yolo-mode-api.ts` is empty
- cyrup: `extension.rs:608` (`yolo_mode`), `:628` (`set_yolo_mode`), `:693` (`toggle_yolo_mode`). `set_yolo_mode` has exactly one caller — `:694`, inside `toggle_yolo_mode` — and `toggle_yolo_mode` has none. The `/permission-system` yoloMode row deliberately routes through `save_extension_config` instead (`:783-790`). `crates/cyrup-ext/src/native.rs:318` (`trait NativeExtension`) exposes `id`/`init`/`on_event`/`execute_command` — no way to publish a callable API object
- observable: the three methods compile, are documented and tested, and cannot be invoked in production. **And `yolo_api.rs:16` claims they are "reached through the `/permission-system` command", which `extension.rs:721-728` contradicts** — the same doc-asserts-wiring-that-does-not-exist pattern as `PERM-014`. Unlike PB-16 this needs a new seam: `SharedBus` is an event bus, not a callable-API registry.

**UW-10 · The intercom compose and session-picker overlays are render-only** — *medium* · confirmed at HEAD by area 11
- upstream: `pi-intercom/index.ts:1857` (`new SessionListOverlay(...)`) and `:1874` (`new ComposeOverlay(...)`), both handed to `ctx.ui.custom`; classes at `ui/session-list.ts:44` and `ui/compose.ts:13`
- cyrup: `ui/compose.rs:86` (`handle_input`), `:74`, `:79` and `ui/session_list.rs:75` have zero production callers; `open_overlay` is never called from this crate. The only production use is a one-shot `render` at `extension.rs:404-407` whose output ends "Type `/intercom {target} <message>` to send."
- observable: `/intercom <target>` prints a picture of a compose box and asks the user to retype the whole command with a body. **Rationale correction**: `ui/mod.rs:12-19` blames a missing `register_message_renderer` AND `register_shortcut`; the first now exists (`cyrup-ext/src/native.rs:270`), so only the `alt+m` path stays blocked by VL-S15 — the slash-command path is reachable today. Fix the comment with `ICOM-024`/`ICOM-028`.

**UW-11 · Copilot and Codex login flows are fully written and unreachable** — *high* · = area 01 `PROV-029`
- upstream: `providers/github-copilot.ts:16` and `openai-codex.ts:**13**` @v0.83.0 both carry `lazyOAuth({… load: load*OAuth })`. **Citation corrected this pass**: the previous edition cited `openai-codex.ts:15` (that line is `models:` at v0.83.0), and elsewhere quoted an `isSubscription: true` property from `github-copilot.ts:16` that **does not exist at v0.83.0 at all** — it is a v0.84.1 addition. Both sides re-read at the tag
- cyrup: two Copilot OAuth types exist — `GitHubCopilotLogin` (`auth/oauth/github_copilot.rs`, real `login` at `:821`) and `GitHubCopilotOAuth` (`providers/github_copilot.rs:410`, refresh/to_auth only) — and `github_copilot_auth()` (`providers/github_copilot.rs:142-146`) wires the **second**. Same shape for Codex: `openai_codex_auth()` (`providers/openai_codex.rs:129-131`) wires `OpenAiCodexOAuth`, not `OpenAiCodexOAuthFlow` (`auth/oauth/openai_codex.rs:516`). `/login` resolves through `provider.provider_auth().oauth` (`cyrup-config/src/login.rs:784`), so both dead-end on `LoginUnsupported` (`auth/mod.rs:124-131`). `providers/builtin_oauth.rs:37-56` has four arms and a prose exemption at `:14-16`; `register_bundled_oauth_flow_loaders` (`auth/oauth/load.rs:111`) has **zero production callers**
- observable: `/login` advertises both providers — with the subscription marker, since both `is_subscription` returns true — and dead-ends. Two complete, tested login flows ship in the binary and cannot be reached. Fix is one field assignment per provider; separately, either populate the flow registry or delete it.

**UW-12 · Compaction cannot be cancelled from the shipped binary, and the indicator advertises the dead key** — *high* · = area 03 `SESS-040` (+ `SESS-041`, `SESS-042`)
- upstream: `modes/interactive/interactive-mode.ts:3074-3085` @v0.83.0 rebinds Escape on `compaction_start`, restoring it at `:3088-3095`
- cyrup: that rebind was never ported — the `CompactionStart` arm (now `app/events_fold.rs:195-223`) handled it by setting the indicator and nothing else; `AbortCompaction` (`cyrup-session-svc/src/command.rs:32`, `:116-118`) had **zero production callers**, `AgentSession::abort_compaction` likewise; and the indicator band (now `app/render.rs:86-90`) prints "(esc to cancel)"
- **CLOSED — do not schedule.** Area 03 struck `SESS-040` as REFUTED on 2026-08-15 and the whole chain is at HEAD (`4fb5e40`): `app/events_fold.rs:222` arms `state.compacting` beside the band at `:220`; `app/input.rs:144-146` returns `AppAction::AbortCompaction` on Escape ahead of pi's four-branch chain, exactly as `interactive-mode.ts:3080-3086` shadows it; `app/run_action.rs:53-54` calls `ctx.session.abort_compaction()` (`session.rs:1900`). Pinned at `cyrup-tui/src/tests/escape_chain.rs:233`/`:244`, band at `tests/compaction_status.rs`. `REPRO-LOG.md`'s "one of seventeen still open" is corrected in the same direction
- observable: the provider call bills and `append_compaction` mutates the session file regardless of what the user presses. Two adjacent defects make it worse and **must ship together**: `abort_compaction()` never cancels an AUTO compaction (`SESS-041`), and there is no abort re-check before `append_compaction`, so even a cancelled compaction is written (`SESS-042`) — both are latent *only* because this has no caller. **Verification must include a live terminal run**, not a driven event loop (standing TUI rule).

**UW-13 · `ExtensionManifest.capabilities` is parsed and never read — the per-extension WASM sandbox grant is inert** — ***critical*** **(raised from high this pass)** · = area 06 `EXT-054` (+ `EXT-055`)

> **FIXED 2026-08-13.** See `06-cyrup-ext.md` `EXT-054`/`EXT-055` for the full evidence block. Summary: `load_discovered` passes `disc.manifest.capabilities` into a new `ExtensionHost::load_wasm_with_caps`; `GuestState` carries the grant as DATA and the host enforces it at the `exec`/`proc`/`http-client`/`ui` import boundary and through `FsCaps` for `ext-fs`; `load_wasm` keeps its signature as the manifest-less host-internal entry. 9 new tests, 5 RED before a one-line revert.
- upstream: **none, and that is the point.** pi has no capability model; every TypeScript extension runs with the whole process's authority. This is a divergence from **cyrup's own** security design, which is why a pi-anchored item-driven sweep cannot see it
- cyrup: `manifest.rs:20` declares the field and `:23-35` the `Capabilities { fs, exec, net, ui }` shape; `loader.rs:213`/`:259` synthesise defaults; **there is no consumer anywhere.** Re-verified at HEAD this pass: `load_discovered` (`facade.rs:1166-1184`) holds `disc.manifest` and calls `self.load_wasm(id, &bytes, services)`, whose signature (`facade.rs:1063-1070`) is `(id, bytes, services)` — the manifest provably cannot reach instantiation, so `capabilities.{exec,net,ui}` narrow nothing. `FsCaps::with_fs_root` likewise has zero callers (`EXT-055`), so `ext-fs` is permanently denied for every guest
- observable: every loaded guest gets the full host surface, gated only by the coarse `origin.is_pre_trust() || project_trusted` check (`loader.rs:56-59`) — while `manifest.rs:2`, `host/store_state.rs:1-3`/`:20-22` and ADR-0002 all document a capability-scoped sandbox. **Critical because README's definition of critical carries no reachability qualifier**: a permission bypass is a permission bypass. Blast radius *is* bounded — zero WASM guests ship today — and that is scheduling information, not a rating: **land it before the first third-party component, not after.** Deny-by-default is part of the fix: the loader's two `capabilities: Default::default()` synthesis sites must stay the EMPTY grant.

**UW-14 · SINGLE-mode `outputSchema` is unadvertised, so the structured-output channel is unreachable** — *high* · = area 09 `SUBA-043`
- upstream: `src/extension/schemas.ts:349` @v0.43.0 has `outputSchema` **top-level**
- cyrup: it is not among the 45 props at `extension.rs:6543-6690`, and `structured_output_schema` is hardcoded `None` at `:1934` and `:2295` — the runner already carries the field; only the two constructors pin it
- observable: the schema is silently dropped and the run returns prose. **`SUBA-S01` was closed to deliver exactly this channel** — the transport exists and the surface a model calls does not expose it. Area 09 names the general fix: a schema/dispatch guard asserting every advertised property has a consumer, which would have caught this, `SUBA-047` and `SUBA-N05` as a class.

**UW-15 · Concurrent-duplicate ask collapse is implemented in `dedup.rs` and never wired** — *medium* · = area 10 `PERM-014`
- cyrup: the module exists and is tested; nothing in the live gate path calls it, and the in-tree docs read as though it were wired
- observable: two concurrent identical asks each prompt the user, where pi collapses them onto one decision. (Examined and deliberately **not** raised this pass: a double-ask is not an unasked approval.)

**UW-16 · Implemented-and-unadvertised, subagents edition** — *medium* · area 09 `SUBA-047`, `SUBA-054`, `SUBA-046`
`toolBudget` is fully enforced (`exec/tool_budget.rs`, 388 lines; `TOOL_BUDGET_ENV` written at `exec/mod.rs:1837-1846`) and **not advertised** on the tool schema, so a per-call budget from an orchestrator is silently discarded (`SUBA-047`). `defaultReads` is parsed and rendered and **never reaches a single run**, so `[Read from: …]` never appears outside chains (`SUBA-054`). And the inverse shape: `grant-spawn-budget` is **advertised and unported**, so an exhausted spawn cap is terminal for the session (`SUBA-046`). Area 09 found the first two *by accident*, without running a systematic hunt — which is the argument for doing one.

**UW-17 · Extension widgets, headers and footers reach the TUI and are stored where nothing renders them** — *medium* · area 07 `TUI-014`, `TUI-033`
The delivery half of `ui.setWidget` / `ui.setHeader` / `ui.setFooter` is live all the way into `cyrup-tui`, and the values land in fields no draw path reads. Related on the wire: `SEAM-011` sends `setWidget` with a cyrup-invented `{widget}` blob, and `SEAM-028` is the test pinning it.

**UW-18 · Settings rows that toggle values nothing reads** — *low* · area 05 `CFG-045`, `CFG-015`, `CFG-044`
`doubleEscapeAction` is offered in `/settings` and the Escape handler has no double-escape and no bash-mode-exit branch (`CFG-045`, and `TUI-009` is its TUI half). `CFG-015` carries five unconsumed settings accessors including `lastChangelogVersion` and `collapseChangelog` (PB-6's home). `CFG-044`'s `get_auth_status` is dead. Adjacent: `AGENT-031` / `CFG-006` (`websocketConnectTimeoutMs` parsed, never reaching the HTTP layer), `AGENT-S03` (`StreamOptions.metadata` unreachable from the agent loop), `SESS-033` (`inputs_fingerprint` has no caller and its doc claims otherwise), `PROV-032` (`filter_github_copilot_models`, zero production callers), `SEAM-048` (pi's `name:N` command disambiguation is dead code), `TOOL-015` / `EXT-024` (nothing reads `render_kind`), `EXT-025` (`reload()` plus four `emit_*` facade methods).

**UW-19 · `keybindings.json` is read exactly once, at boot, and no other surface ever reads it** — *medium* · **new this edition** · area 07 `TUI-051`, area 08 `SEAM-067`, area 05 `CFG-048`
Three findings, one wiring hole. `/reload` (`cyrup-tui/src/app/execute_session.rs:241-268`, `rt.reload(None).await` at `:264`) calls only `rt.reload`; `load_keybindings_json` has exactly **one** non-test caller — `crates/cyrup/src/main.rs:1626`, at boot — while both the command's help text and its in-source comment claim keybindings are re-read (`TUI-051`). The **pre-launch** selectors (`--resume` picker, trust prompt, config selector) never load it at all and print hint rows naming the built-in keys (`SEAM-067`). And pi's sixth startup migration, `migrateKeybindingsConfigFile` → `migrateKeybindingsConfig` (`core/keybindings.ts:289-309`, **59** legacy names, also applied at read time at `keybindings.ts:366`), is not ported at write time or read time, so every legacy name is silently inert (`CFG-048`; `crates/cyrup/src/migrations.rs:26-33` makes four calls, pi's `runMigrations` six). **Ordering matters: `CFG-048` must precede `TUI-028`**, or the `editor.*`→`tui.editor.*` namespace rename breaks every config written against shipped cyrup.

**UW-20 · The faithful fuzzy matcher is ported and unused; `--list-models` hand-rolls a lossier one** — *low* · **new this edition** · area 08 `SEAM-068`
The port of pi's `fuzzy.ts` exists and has no caller on the `--list-models <search>` path, which uses a hand-written filter that drops matches pi returns. Same shape as UW-1: a correct port sitting beside the code that should call it. Ships naturally with `SEAM-020` (the same command prints the whole compiled catalog rather than the auth-configured one, and its no-models-available branch is therefore unreachable).

---

## 3. Version lag — upstream added it after cyrup's ported baseline

Still work. An item here is in scope for the next version bump, not out of scope.
**66 items: 36 against `pi`, 17 against `pi-subagents`, 13 against `pi-intercom`, 0 against
`pi-permission-system`.**

**Why this fell from 78.** Nothing was closed and nothing was deleted. Area 12 ran
`git cat-file -e v0.83.0:<path>` before trusting any inherited `upstream-drift` kind and **six items
proved to be in-baseline work misfiled as lag** — `DRIFT-018`, `DRIFT-019`, `DRIFT-030`, `DRIFT-031`
and `DRIFT-032` moved to `not-ported`, `DRIFT-016` to `stale-port`. Separately, the `tracking`-kind
rows and the two `upstream-drift` scope questions (`AGENT-028`, `SESS-038`) are now trackers outside
the count. **Lag shrank because the port-bug pile grew**, which is the direction that matters: a
port bug is owed now, a lag item is owed at the next bump.

### 3a. `pi` v0.83.0 → v0.84.1 — 36 items (627 files, +52 291 / −17 556)

The `VL-P` rows below are the previous edition's cross-cutting entries; the re-audit assigned owners
to most of them. Where an area item owns the row, **the area file carries the two-sided evidence at
HEAD and is the place to work from** — the citation here is the load-bearing pair only.

| id | one line | upstream (v0.84.1 unless noted) | cyrup | owner |
|---|---|---|---|---|
| VL-P1 | `baseten` provider, `thinkingFormat:"baseten"`, `compat.chatTemplateArgs` | `providers/baseten.ts:6-14`; `api/openai-completions.ts:779-795`; `types.ts:565`, `:574-575` | `api/compat.rs:28-39` (10 variants, no `Baseten`), `:73-168` (no `chat_template_args`); `grep -rni baseten crates/` = 0 | 12 `DRIFT-009`; env half `BASETEN_API_KEY` |
| VL-P2 | `qwen-token-plan-individual` provider | `providers/all.ts:120` (absent at v0.83.0) | `providers/all.rs:141-243` — cyrup registers **35** built-ins against v0.84.1's **40** | 12 `DRIFT-019` (only this one of its four is genuine lag — the other three predate the tag) |
| VL-P3 | `samplingParams` carried nowhere | `api/simple-options.ts:27-33`; `types.ts:189`, `:802`; applied `openai-completions.ts:885-886`, `openai-responses.ts:331-332`, `azure-openai-responses.ts:325-326`; composed `provider-composer.ts:123-125` | `utils/simple_options.rs:61-98` threads 20 fields, none of them sampling params | 05 `CFG-039` (config tier), 02 `AGENT-026` (proxy body) |
| VL-P4 | vLLM `thinking_token_budget` via `compat.supportsThinkingTokenBudget` | `types.ts:583`; `openai-completions.ts:851-866` with a `MIN_ANSWER_TOKENS` floor | `api/compat.rs:73-168` — no such field; and `PROV-015` means it has nowhere to land | 01 `PROV-015` |
| VL-P5 | `telemetryContext` on request options | `packages/ai/package.json:65` (telemetry became a runtime dep at v0.84.1), `types.ts:122-123`, `api/simple-options.ts:36` | `stream.rs` `StreamOptions` has no telemetry field; zero `TelemetryContext` hits | 12 `DRIFT-047` (which extends it to the whole `packages/telemetry` + `pi.ai.request` span contract), 02 `AGENT-028` *(now a tracker — see OQ-7)* |
| VL-P6 | Auth ops take no cancellation signal; OAuth refresh has no 15 s bound | `auth/types.ts:45-48` threaded onto every `CredentialStore` method `:70`, `:76`, `:86-90`, `:93`; `DEFAULT_OAUTH_REFRESH_TIMEOUT_MS = 15_000` at `auth/resolve.ts:120`, raced `:149-153` | `auth/store.rs:24-54` (no options arg); `auth/resolve.rs:198` refresh unbounded | unowned at item level; 08 confirmed the CLI call sites pass no deadline |
| VL-P7 | Copilot Individual: no policy-state fallback | `auth/oauth/github-copilot.ts:92-113`, gate `:115-133` | `providers/github_copilot.rs:310-326`, `:331` — single list, no fallback, no gate | 01 (re-confirmed open) |
| VL-P8 | A blocked tool call cannot terminate the batch | `packages/agent/src/types.ts:61-69` (`BeforeToolCallResult.terminate`), consumed `agent-loop.ts:636-646` | `cyrup-agent/src/hooks.rs:49-52`; blocked result hardcodes `terminate:false` at `agent.rs:1030` | 02 `AGENT-022`, 06 `EXT-049` (the extension-facing `ToolCallEventResult.terminate`) |
| VL-P9 | `Agent::reset()` does not reject mid-run | `agent.ts:333-336` (no guard at v0.83.0 `:326`) | `agent.rs:1604-1616`, whose own doc at `:1601` says it clears "unconditionally, even mid-run" | 02 `AGENT-023` |
| VL-P10 | No compact-and-retry after a recoverable `length` stop | `core/agent-session.ts:1993-1994`; predicate `utils/overflow.ts:171-173` | `cyrup-session-svc/src/session.rs:4160` — term absent though the recovery scaffold `:4161-4182` is ported; `rg is_recoverable_length crates/` = 0 | unowned at item level; area 01 re-confirmed the predicate absent |
| VL-P11 | *(superseded)* prompt during compaction | — | — | **→ PB-29** (promoted to high; it is a port bug, and the defect is wrong context, not a missing rejection) |
| VL-P12 | Tool-result images never normalized/auto-resized | `utils/tool-result-images.ts:22-62`, called after the `tool_result` hook at `core/agent-session.ts:518-520` | `cyrup-ext/src/hooks.rs:58-113` — `after_tool_call` is a pure field diff. Primitive exists (`cyrup-tools/src/tools/read.rs:265`); setting plumbed (`builder.rs:681`) | **unowned** — see below |
| VL-P13 | Ambiguous bare `--model` silently picks the first catalog match | `core/model-resolver.ts:469-503` (errors `Model "…" is ambiguous across providers: …`) | `cyrup-config/src/model.rs:1139-1143` — `all.iter().find(...)`, first match wins | **unowned** — see below |
| VL-P14 | `auth check` subcommand unrecognized | `cli/auth-command.ts:51` (usage `:18`, `:42`), impl `cli/auth-check.ts:22-73`, result shape `:16-20` | `crates/cyrup/src/credential_print.rs:149-155` matches only the two print verbs | 08 `SEAM-050`, which widens it to the whole v0.84.1 auth-command surface (per-kind unknown-option errors, tri-state 0/1/2 exit) |
| VL-P15 | A malformed `pi` block in `package.json` hard-fails the install | `core/pi-manifest.ts:16-34` (whole body in try/catch → `null`; `:26` skips non-string-array fields) | `cyrup-resources/src/package/manifest.rs:87` — `serde_json::from_str(&text)?`; same at `:80` for the `cyrup.toml` branch | **unowned** — see below |
| VL-P16 | Management HTTP fetches are not retried | `utils/management-http.ts:25-68` (2 extra attempts, retryable set `:3`), callers `remote-catalog-provider.ts:81`, `version-check.ts:57`, `tools-manager.ts:109`/`:127` | `cyrup-provider/src/remote_catalog.rs:544-547` — one `send()`; any transport error or 5xx is terminal | 01 (re-confirmed open) |
| VL-P17 | Terminal colour-scheme and background probes run sequentially | `modes/interactive/theme/theme.ts:796-810` (both promises started, then awaited) | `cyrup-tui/src/theme.rs:1334-1343` — early return, then fall through | **unowned** — see below |
| VL-P18 | `tui.editor.historyPrevious` / `historyNext` not rebindable | `packages/tui/src/keybindings.ts:68-75` (both `defaultKeys: []`), consumed `components/editor.ts:768-777` | `cyrup-tui/src/keymap.rs:157-186` — 24 ids, neither history id | 07 `TUI-035` (verified absent at v0.83.0 — genuine lag) |
| VL-P19 | The fullscreen (alternate-screen) TUI program is entirely absent | `packages/tui/src/tui-alt-screen.ts` (1047 lines, new at v0.84.1) + `components/scroll-view.ts` (195) + `alt-screen-flash.ts` (51); flag `cli/args.ts:180-193`; settings `settings-manager.ts:135-136`; switch `interactive-mode.ts:345`; 8 `tui.altScreen.*` bindings `keybindings.ts:43-50` | no alt-screen module; `app/input_reader.rs:431` is `Event::Mouse(_) => None`; no `tui_mode` anywhere | 08 **`SEAM-051` — now *high*, effort S, and the most urgent row in this table**: `--tui-mode regular`, the flag's *default* value, is captured by `partition_extension_flags` and every mode exits 1. 07 `TUI-019` (**re-rated low → medium**; the ADR-0001 justification is struck — see OQ-8), 05 `CFG-021` (`tuiMode`/`fullscreenScrollbar` settings), 12 `DRIFT-022` *(now a tracker)*. **`SEAM-051` and `CFG-021` must be fixed under either answer to OQ-8; they do not wait on it.** **Mechanism note**: ratatui supports the alternate screen and mouse capture natively; the gap is the application layer. |
| VL-P20 | Mermaid fences not rendered; `markdown.mermaid` is not a setting | `modes/interactive/components/mermaid.ts:14-30`; setting `settings-manager.ts:61` (getter `:1251`) | `cyrup-tui/src/markdown.rs:964-965` — the only two "mermaid" occurrences are a comment quoting upstream's predicate | 07 `TUI-034`, 05 `CFG-040` |
| VL-P21 | `registerMarkdownTransformer` and the transform pipeline are absent | `core/extensions/types.ts:1153`, `:1292`, `:1703`; pipeline `components/markdown-transform.ts:3-29` (fail-open per transformer, width-aware) | `cyrup-ext/wit/world.wit` has no transformer import/export; `markdown.rs` has no transform seam | 06 `EXT-019` (re-scoped to the v0.84.1 `MarkdownTransformContext` shape), 07 `TUI-034`, 12 `DRIFT-015`. **VL-P20 has nowhere to attach until this lands** — upstream ships mermaid AS a registered transformer. |
| VL-P22 | A torn session-JSONL tail is never repaired; fork is not published atomically | `packages/agent/src/harness/session/jsonl/storage.ts:33-46`, `:83-90`, `:93-95`, `:99-109` | `cyrup-session/src/manager.rs:851-888` (`load` skips malformed lines, returns `recovered`) and `:114-117`, where the rewrite is gated `if migrated && !recovered` — a recovered file is provably never rewritten. `store.rs:68-82` stages+renames for full rewrites; `create_exclusive` (`:86-116`) writes straight to the destination fd | **ownerless and growing** — see below |
| VL-P23 | `packages/protocol` + `packages/client` entered the closure — no remote-session wire format | `coding-agent/package.json:48-49` now depends on `pi-client`/`pi-protocol` (neither at v0.83.0), consumed `client/remote-session.ts:7`, `:17` | no `cyrup-protocol`/`cyrup-client` crate; nothing decodes framed CBOR. **Do not conflate** `cyrup-intercom/src/transport/framing.rs` — a line/JSON protocol capped at 1 MiB (`:19`) | 08 `SEAM-058` *(a tracker: its Fix is "track, do not build"; the reachability re-check confirms v0.84.1's `main()` still does not reference `experimentalCli`)* |
| VL-P24 | `CredentialSynchronizationError` and serialized, cancellable credential operations | `core/model-runtime.ts:94-111`, `:494` (`enqueueCredentialOperation`) | zero `CredentialSynchronizationError` hits; `/login`/`/logout` have no per-provider operation queue | 05 `CFG-020` — **note the target grew +356 lines at v0.84.1; read `model-runtime.ts` at v0.84.1, not v0.83.0.** Depends on PB-3's `providers:[id]` refresh scope. Area 12's `DRIFT-023` is a duplicate **and a lead** — its diffstat was never re-derived |
| VL-P25 | Catalog set trails the provider set | 39 `*.models.ts` at v0.84.1 (Together's is hand-written Rust) | 35 embedded catalogs; the four with no counterpart are `baseten`, `qwen-token-plan`, `qwen-token-plan-cn`, `qwen-token-plan-individual` — closes with PB-2, VL-P1, VL-P2 | 12 `DRIFT-009` (rewritten this pass), 01 `PROV-018` / `PROV-038` / `PROV-039`, `PROV-004` *(tracker)*. Per-model accuracy IS statically auditable and **has now been audited**: all 35 catalogs are generated from `b0c2a90e` by `cargo run -p xtask -- gen-catalogs` (**CLOSED 2026-08-15**, `PROV-018`/`PROV-060`), which needs no generator run and no network. See OQ-5's refutation above |

**Four rows no area file has claimed across two passes** — nobody has re-derived them at `04c1ba2`,
and nobody owns the fix. They are the highest-risk rows in this table because they read as verified:
**VL-P12** (tool-result image normalization — the `images.autoResize` *toggle* is now wired at
`builder.rs:681`, which is a different thing from running pi's normalizer after the `tool_result`
hook), **VL-P13** (ambiguous `--model`; `--model glm-4.7` is offered by six providers in cyrup's own
catalogs), **VL-P15** (`"pi": {"extensions":[1,2]}` aborting a whole install — and its blast radius
was already flagged unconfirmed), **VL-P17** (double-timeout `auto` theme detection on a terminal
answering neither DSR ?996 nor OSC 11; the remedy is larger than the symptom because cyrup's
`TerminalProbe` is a synchronous `&dyn` trait). *(Do not mistake area 08's new `SEAM-066` for an
owner of VL-P17: that item is the pre-launch surfaces hardwiring the dark palette, a different
defect in a different file.)*

**VL-P22 is still the single largest ownerless surface in the port.** Three area files hand the same
mass to it and none owns it: area 03 (`packages/agent/src/harness/session` retree — 21 files,
+3070/−1147, adding seq/lanes/records/retainedTail plus a 993-line conformance suite), area 02
(`AGENT-028` — `packages/agent/src/harness/**` is ~11.4k insertions / ~10.9k deletions in this window,
including the `agent-harness.ts` rewrite, a new 667-line `reducer.ts` and a new typed telemetry layer,
**not measured and owned by no area file**), and area 12 (`DRIFT-040`, a tracker **and** a lead —
area 12 states plainly which of its claims were confirmed first-hand, the `packages/storage` →
`packages/session-backends` rename and `harness/session/types.ts:44-58`, and which were never read).
The *torn-tail* half of VL-P22 is a concrete, small, provable bug and should be fixed now; the
*harness-v2* half is a scope decision — **OQ-7**.

**Additional pi-drift items filed since the VL-P rows were written**: area 12's `DRIFT-041`
(session HTML export is a 131-line text dump against pi's 5 021-line templated document across 8
files — tool-result *text* survives, call arguments and result metadata do not), `DRIFT-045`
(Ctrl+V with text on the clipboard inserts nothing — the `wl-paste` text branch), `DRIFT-046`
(`normalizeWindowsShellPath` — **duplicate of `TOOL-036`**, which owns the same body plus the
`~`/`os.homedir()` half; note area 04's correction that the `~` half is a v0.83.0 parity bug while
`normalizeWindowsShellPath` itself landed *inside* the drift window), `DRIFT-048` (Google converter
picks the tool-call-id rule off the SOURCE message's model — a defect inside the code that closed
`DRIFT-026`), `DRIFT-050` (`CYRUP_TELEMETRY=` empty is an explicit OFF upstream and a silent no-op
here), `DRIFT-051` (`process.title`'s role suffix — RPC, `__subagent-runner` and `__intercom-broker`
children are all bare `cyrup` in `ps`; the base title is satisfied by accident in Rust, which is why
the item claims only the suffix); area 01's `PROV-040` (`fetchDeferred`/`cancelDeferred` — cyrup
ports the deferred data model but no handle can be redeemed); area 05's `CFG-041` / `CFG-042`;
area 06's `EXT-049`…`EXT-052`; area 07's `TUI-046` (Kitty keyboard flag 1 vs pi's 7).

**Systematic delta sweeps, so nobody re-runs them.** Across the whole v0.83.0..v0.84.1 delta: exactly
**1** CLI flag added (`--tui-mode` → VL-P19), exactly **3** settings keys added (all area 07), **7**
env vars added (`AI_AGENT` → PB-5, `BASETEN_API_KEY` → VL-P1, `CC`/`STY`/`ZELLIJ` → area 07). Areas
03 and 04 both returned near-empty: `core/compaction/*`, `system-prompt.ts`, `skills.ts`,
`session-cwd.ts` and `prompt-templates.ts` are **byte-unchanged** across the two tags (all five
re-verified this pass), and `packages/coding-agent/src/core/tools/` is 7 files / +68 −35 with all
four changes already carried or provably no-ops. `bash-executor.ts` and `output-guard.ts` are
byte-identical. That is a real, narrowing result — it means this delta's risk is concentrated in
`packages/agent/src/harness/**` (VL-P22 / OQ-7), `packages/tui` (VL-P19) and the provider tier.
**One caution the repair pass earned**: area 04's version-lag sweep was scoped to
`packages/coding-agent/src/core/tools/`, and `utils/paths.ts` is not under that path — which is how
`normalizeWindowsShellPath` was missed. A "near-empty diff" result is only as wide as the paths it
walked, so state them.

**Window limit.** pi HEAD is `581d75a89` = `v0.84.1-117-g581d75a89`, so **117 commits past the tag are
unanalysed**. One concrete item is known to sit in there and was deliberately not filed because the
hard rules require a named tag: `getExperimentalToolSampling()`'s constrained-sampling request on the
four built-in tools, absent at both diffed tags.

### 3b. `pi-subagents` v0.43.0 → v0.47.1 — 17 items (151 files, +10 254 / −1 333)

**This range was unanalysed territory until the 2026-08-12 re-baseline** — the previous edition
recorded v0.43.0 as "latest". The src-only sweep covered **96 non-merge commits, 67 files,
+4 696/−769 and 12 net-new source files, all 12 read**; fourteen commits were diffed line by line.
All 17 items live in area 09 with two-sided evidence:

- **medium** — `SUBA-044` (the bundled `reviewer` agent still grants `bash`/`edit`/`write`; upstream made the lane read-only), `SUBA-050` (`subagents.modelScope.strict`), `SUBA-051` (async **child** runs have no default wall-clock timeout; upstream bounds them at 30 min), `SUBA-052` (YAML literal block scalars `|`/`|-` parse to the literal string `"|"`), `SUBA-053` (`~` never expanded in chain read/write paths), `SUBA-054` (`defaultReads` never reaches a single run — also UW-16), `SUBA-055` (the `guide` action and its packaged version-matched docs), `SUBA-056` (durable completion replay and output archives), `SUBA-057` (`dismiss` — a recovered workflow with no live controller is stuck "running" forever)
- **low** — `SUBA-023` (async lifecycle hardening; no signal-name attribution), `SUBA-024` (`parallel-handoff` / `agent-contract`; `task-intent` closed, `chain-validation` **struck — the file never existed at any tag**), `SUBA-026` (interactive admin UI and selector; `/subagents-stop` landed), `SUBA-058` (chain read instructions not filtered by existence), `SUBA-059` (`artifactConfig.cleanupDays` never wired to the type that already parses it), `SUBA-060` ("resume-first" guidance for failed async runs), `SUBA-065` (`unknownSubagentActionMessage` — did-you-mean recovery and its destructive-action gate), `SUBA-066` (`/subagents-guide`)

**Not filed by rule**: `run-fanout-budget.ts` (257 lines — a whole per-run logical fan-out cap with
config, doctor check and status surface) landed on `main` at `17b4078`/`668c587` **after v0.47.1** and
has no named tag to cite. Pick it up on the next tag.

**Unread on the upstream side in this range**, so anything inside is invisible: `workflows/scripted-workflow.ts`
(+231), `inspectors/herdr/project-panes.ts` (+524), `runs/background/async-job-tracker.ts` (+426 — the
v0.47.0 event-driven rewrite), `foreground-history.ts` / `workflow-foreground-steering.ts` (new),
`shared/display-text.ts` (new), `tui/{render,fleet}.ts` (+343), `missions/workflow-state.ts` (+209),
`extension/rpc.ts` (PB-8's file). Specifically unfiled and likely drifted: the **steering-recovery
hardening across v0.44–v0.47** in `subagent-prompt-runtime.ts` was read but not compared line by line
against cyrup's `prompt_runtime.rs` `SteeringInbox`, which targets the v0.43.0 shape. Diff it when
`SUBA-049` is scheduled.

### 3c. `pi-intercom` v0.9.2 → v0.10.1 — 13 items (24 files, +2 495 / −700, 14 commits)

The window is **two** minor versions, not three, because the ported baseline is v0.9.2 (§1d). All 14
commits are accounted for commit-by-commit in area 11. The 13 items are `ICOM-035`…`ICOM-047`:

- **medium** — `ICOM-035` (busy inbound **parked until idle instead of steered**; upstream deleted the
  entire park branch at v0.9.3 — this one also keeps `ICOM-023`/`ICOM-032` alive, and its fix does
  **not** need a `HostServices` change: `AgentSession::inject_message` already routes to `agent.steer`
  when `is_streaming()` at `session.rs:3752-3754`), `ICOM-036` (reply targeting by sender-ID prefix +
  four disambiguation errors; upstream **replaced** the function `ICOM-001` closed against, at
  v0.9.3), `ICOM-037` (a `send` to the sole pending asker is not treated as its reply), `ICOM-038`
  (no client liveness heartbeat — a half-open broker socket strands a session invisibly), `ICOM-039`
  (`list` prints a fixed 8-char id, not a distinguishing prefix), `ICOM-040` (unnamed-session alias
  uses 8 id characters, not 18 — **collides for UUIDv7 subagents spawned milliseconds apart, making
  peers unaddressable**; coupled to area 09's `orchestrator_presence_target`, change both or neither),
  `ICOM-042` (cwd-scoped `send`/`ask` and `openProjectPaneIfMissing`), `ICOM-044` (malformed config
  fails closed silently), `ICOM-046` (`reply` silently drops attachments — a one-line fix upstream
  shipped a point release for)
- **low** — `ICOM-041` (`runtimeFallbackAlias`), `ICOM-043` (v0.10.0 copy revision; emoji still
  present — land with `ICOM-048`), `ICOM-045` (blocking `ask` not refused up front when the target is
  offline), `ICOM-047` (broker startup failures discard the broker's stderr)

### 3d. `pi-permission-system` v0.7.1 → v0.8.0 — **zero**

Independently corroborated by area 10, which read the diff per shipping file: of 28 changed files, 11
are non-shipping, and `permission-manager.ts`, `jsonc-config.ts`, `permission-dialog.ts`,
`system-prompt-sanitizer.ts`, `permission-forwarding.ts` and `before-agent-start-cache.ts` are
extract/export refactors with no behavioural delta. The five genuinely behavioural v0.8.0 changes are
all ported and were closed or confirmed this pass — see §4. The five permission-system entries in
this document (PB-32, PB-15…PB-18) are all port bugs against **v0.7.1**, and PB-32 was verified at
**both** tags precisely so it could not be re-litigated as drift.

---

## 4. Closure record — what has actually landed, verified at `04c1ba2`

**117 items closed across the twelve area files in the re-baseline**, every one re-derived from code
on both sides rather than from a commit message. The largest single blocks: area 09 (22), area 05
(16), area 04 (14), area 07 (13), areas 01 and 10 (10 each). **207 new items were filed in the same
pass** (176 in the audit, 31 more in the repair pass that followed the completeness critique), and
most of them come out of auditing what the *closing* code does — which is the standing lesson:
**closing a "not implemented" item means the subsystem now exists, not that it is correct.**

**Subsystems that were "absent" and are now ported and wired**
- `watchdog/` — 18 modules under `crates/cyrup-ext-subagents/src/watchdog/` (~18k lines), including a real stdio LSP client. Wiring: `register_main_watchdog` at `extension.rs:9055`, nine subscriptions `:9338-9352`, `/subagents-watchdog` `:9326`, four `watchdog.*` verbs `:7567-7570`. *(Two no-op agents remain — UW-4, UW-5.)*
- `missions/` — 7 modules; six `mission.*` verbs, params parsed `:5423-5432`, goal-continuation notices from the `agent_end` handler. *(Write half of workflow state remains — UW-8.)*
- The interactive fleet inspector — `tui/fleet_overlay.rs:177` implements `InteractiveOverlay`; `extension.rs:11366`/`:11376` construct it and call `services.open_overlay(Box::new(overlay))`, driven by `App::on_overlay_request` (`cyrup-tui/src/app/run_arms.rs:406-430`). **This is the seam PB-9 and PB-18 should now use.**
- The persistent fleet status widget (`set_widget` at `extension.rs:9489`, `:9889`, `:9978`); subagent tool renderers (consumed through the ONE dispatch site, `run_renderer` at `app/extension_render.rs:153-166`; `Which::ToolCall`/`Which::ToolResult` at `:162`/`:163`); terminal-run **revival** (`background/control.rs:1214` → `extension.rs:4232`); agent **aliases** end to end with the verbatim `Ambiguous agent alias '…': …` message.
- The child-side prompt runtime, the structured-output capture channel, the control/activity pipeline, the async deadline + cascade, agent memory, prompt workflows, the **native supervisor channel** (`native_supervisor.rs`, 2251 lines) and the tool-budget enforcer (`exec/tool_budget.rs`, 388 lines).
- All 11 OAuth login flows (`crates/cyrup-provider/src/auth/oauth/`), driven from `/login`. *(Two of them are unreachable — UW-11. And nine of the eleven were not audited against upstream at all; area 01 flags `PROV-003`'s closure as deliberately weak.)*
- The 2-model `seed.json` stub is **physically gone** (`PROV-007`): the six production sites now go through `cyrup-provider/src/catalog.rs:38-44` `builtin_catalog()`, guarded at `:52-75`.
- Provider retry + idle timeout (`utils/provider_retry.rs`, consumed by **seven** api impls); provider error-body truncation (`utils/error_body.rs`); the full 25-pattern `OVERFLOW_PATTERNS` set, byte-compared; `StopReason::{Pending,Deferred}` + `raw_stop_reason` verified by **producer sweep** across all six wire APIs; `supports_finish_reason` including its consumer; the WIT world at `@0.4.0` with a real cross-copy test.
- `crates/cyrup-modes/src/json_event.rs` — `modes/json-event.ts` is **new at v0.84.1 and already ported**, wired at `json.rs:79` and `rpc.rs:300`.

**`pi-permission-system` v0.8.0, fully absorbed** — `PermanentApprovalStore` removed (regression test `tests/permanent_approvals_file_is_inert.rs`) · review stream un-gated from `debug` (`logging.rs:168-170`) · wildcard 500-char cap → never-match (`wildcard.rs:24`, `:81-83`, `:57`) · forwarded-request id path containment, the critical one (`forwarding.rs:592`, `:596-597`) · `enabled` master switch (`ext_config.rs:51`, `:324`; early return `extension.rs:2221-2223`) · merge-preserving config save (`ext_config.rs:408`) · prototype-pollution key skip (`common.rs:173-175`).

**`pi-intercom` v0.9.2 interop** — re-verified line by line by area 11, and **every claim holds**: the full 16-tag `BrokerMessage` union decodes instead of tearing down the connection (`transport/protocol.rs:857-875`, `client.rs:635-651`) · the v0.9.2 envelope round-trips with a `#[serde(flatten)] extra` capture (`protocol.rs:301-347`) · the broker refuses to replace a live broker (`broker/runtime_claim.rs`, called at `broker/mod.rs:1238` before the stale-socket unlink) · live context-window usage in presence/session lists (`format_context.rs:70` → `tools/intercom.rs:377`).

**`pi` core** — Gemini 3 tool-call ids · Responses terminal-error fallback and length-stop mapping · `OAuthAuth::isSubscription` and the footer `(sub)` marker · `--model` cycling filtered to authenticated models · LaTeX rendering (`markdown/latex.rs`, 2242 lines) · `ctrl+home`/`ctrl+end` and editor page actions · batched colour-scheme reports settling on the last frame · OSC 9;4 terminal progress (tracking the **newer** v0.84.1 spelling) · searchable settings list · `AGENTS.override.md` as first context-file candidate · the `scrollbarThumb` theme token with pi's optional-with-fallback semantics · the bash session-env scrub in pi's exact delete→repopulate→hook order · the Google signed-empty-block hoist.

**Corrections this pass forced, all of them to claims that read as settled**
- **pi's model-catalog generator EXISTS, at both tags.** `git cat-file -e` passes at v0.83.0 and v0.84.1 for `packages/ai/scripts/{generate-models.ts (2733 lines), model-data.ts, models-dev-reasoning-options.ts, check-model-data.ts}` and `scripts/{diff-model-catalog.mjs, publish-model-catalog.mjs}`; pi's root `package.json:24-30` exposes `generate:models` / `hydrate:model-data` / `generate:model-catalog` / `diff:model-catalog` / `check:model-catalog`, and `packages/ai/package.json:52-54` gives the concrete invocations. **Only the OUTPUT is gitignored** (`.gitignore:11` → `packages/ai/src/providers/data/`). `DRIFT-009`'s "no in-tree regeneration source" sentence and the ledger's "catalog lag is unresolvable from this workspace" bullet were both **false** and are struck; `DRIFT-009` now defers to `PROV-018`, because seeding from the published pi.dev artifact is strictly lossier — the artifact *is* the published catalog, so it cannot reproduce what the generator computes from `models-dev-reasoning-options.ts` or the per-provider compat overrides, and it yields no reproducible build step. **The lesson generalises**: "no runtime effect" licenses skipping a directory's *behaviour*, never its *provenance* — a gitignored path is evidence that an artifact is generated, hence that a generator exists.
- **`SEAM-035`…`SEAM-046` never existed.** `git show a9000b1:docs/gap-analysis/08-…md | grep -o 'SEAM-0[34][0-9]'` returns 030-034 only; the 2026-08-12 pass simply started its new ids at `SEAM-047`. Recorded in area 08 so the twelve-wide hole is not read as a deletion under a hard rule about deletion. Honest caveat: `docs/gap-analysis/` has exactly **one** commit in cyrup's history, so an id dropped before the directory came under source control is invisible to that check — to this pass and to every pass.
- **`PROV-030` is contradicted by the file it points at.** `providers/all.rs:176-197` pushes `amazon_bedrock`, `openai_codex`, `google_vertex` and `github_copilot`, while the same file's status table at `:12-47` still calls three of them "pending" and its summary at `:46-47` names all four — including one the table's own row at `:21` marks ported. The doc fix is now mandatory inside PB-22 rather than a separate id, because a separate id lets the code fix land without it.
- **Nine `upstream-drift` claims in area 12 rested on a commit hash rather than a two-sided read.** Seven were re-derived cheaply and six proved misclassified (§3 preamble). The two expensive ones are now leads outside the count. The rule that falls out: **a commit hash answers "when did this land upstream", but classification turns on "before or after the tag cyrup ported from" — run `git cat-file -e <tag>:<path>` before assigning `upstream-drift`.**

**Corrections to CLAUDE.md that fall out of this pass** — worth folding back:
- "first-run predicate is a compile-time constant `false`" is **wrong**; it is `true` for this build (UW-2, OQ-6).
- "cyrup registers the 38 built-ins pi shipped at v0.83.0" is **wrong**; it registers **35** of upstream's **40**, and three of the five missing are port bugs (PB-1, PB-2).
- `packages/telemetry` is **inside** the port's dependency closure as of v0.84.1 (VL-P5 / `DRIFT-047`); `packages/{server,storage,evals}` remain outside it — **though area 08 could not re-verify that against v0.84.1's `package.json` and flags it**; `packages/{protocol,client}` newly entered (VL-P23).
- The `pi-subagents` baseline is not v0.33.x–v0.34.0; it is **≈v0.43.0**.
- The `pi-intercom` baseline is **v0.9.2**, not v0.7.0 (§1d). `crates/cyrup-intercom/src/lib.rs:2` still says v0.6.0 — `ICOM-012` carries the fix.
- **There is no `CLAUDE.md` in this workspace.** The file this document and the README repeatedly cite for the deliberate-divergence list and the out-of-scope pi package list does not exist here. Every claim sourced from it is unverifiable — which is why `SEAM-058` is a tracker and why OQ-7 exists.

**Bookkeeping actions for the ledger owner**: the promoted `PROV-010`/`AGENT-014`/`DRIFT-012` high row
is **stale** (`DRIFT-012` is closed); `SEAM-S02` (`08-…:520`) should be re-audited as **closed** —
`signals.rs:97-100` implements the repeat force-exit with pi's exact 130/143/129 codes — with
`DRIFT-049` replacing it; and the ledger's own "catalog lag is unresolvable from this workspace"
bullet is false (above). None of these changes an item count.

---

## 5. Deletion candidates — dead code, no behavioural difference

Not gaps. Recorded so nobody mistakes them for unimplemented features and "finishes" them.

- `crates/cyrup-ext/src/facade.rs:766` (`render_message_result`) and `:772` (`render_message_result_outcome`) — zero references outside their definitions and two doc-links (`native.rs:264`, `registry.rs:179`). pi has no message-RESULT renderer surface at all: `MessageRenderer` (`core/extensions/types.ts:1153`) is a single call-side function consumed only by `getMessageRenderer` (`runner.ts:579`) at `interactive-mode.ts:3471`. This is invented surface wider than pi's. Scope check before deleting: `RenderKind::Result` and the routing behind it also exist on the extension trait (and area 06 `EXT-024` records that `render_kind` has zero consumers, so the two should be settled together).
- `crates/cyrup-provider/src/session_resources.rs:48` / `:67` — a faithful port of `packages/ai/src/session-resources.ts` with no registrant and no dispose caller. pi's only registrant is the codex WebSocket cleanup (`api/openai-codex-responses.ts:927`), and cyrup has no WS transport by design-note (`api/openai_codex_responses.rs:39-46`). It is pre-wiring for a transport that does not exist; the real item is that transport.
- `crates/cyrup-permission-system/src/jsonc.rs:33` (`parse_ordered`) — zero references including tests; every caller goes through `parse_ordered_config`. Upstream has no counterpart behaviour to lose.

Also dead on **both** sides, so purely cosmetic: `crates/cyrup-provider/src/legacy_api_aliases.rs`
(mirrors pi's own deprecated shim) and `nested_events.rs`'s `is_top_level_async_dir` /
`nested_artifact_env` (upstream's `nested-events.ts` definitions have no call sites at v0.43.0 either).

**Do not confuse this section with the `cyrup-original` items in the census.** Those are invented
*behaviour*, not dead code — they run, they diverge from pi, and each needs a decision (delete, or
justify with a `[CYRUP-DELTA]` note).

> **UPDATED 2026-08-14 (fourth edition): the class is no longer 21 items. It is 46 open rows out of
> 68 filed, and it now has its own section and its own count in `00-residual-ledger.md`.** 66 of the
> surface enumeration's 191 findings are surfaces cyrup has and pi does not — 31 of them newly filed
> and open. **Rate them by REACHABILITY**: an advertised-but-dead surface (`TUI-063` —
> `CYRUP_SHARE_VIEWER_URL` is in `--help` and read by nothing; `EXT-071` — a WIT comment advertising a
> `ToolInfo.source` field that was already removed from the emitted object) outranks an internal
> helper, and a mechanism port the language forces (a WASM guest cannot hand a function back, so
> pi's callback values necessarily become WIT exports) is not divergence at all. **The most dangerous
> members are the ones other analysis has already built on**: `SEAM-080`'s cyrup-invented
> `model_changed` RPC line is reasoned about as upstream by two existing backlog items, exactly as
> `SEAM-100`'s missing `cyrup update --models` was. By area the open set is 05 → 14, 08 → 11,
> 06 → 8, 01 → 5, 07 → 5, 04 → 1, 09 → 1, 11 → 1.

Three of them are the highest-severity invented surface in
the port, and two of those are the whole reason this class needs a decision rather than a backlog
slot:
- **`EXT-054` (UW-13) — *critical***: an advertised per-extension sandbox that grants nothing.
- **`TOOL-039` — *high***: `CYRUP_SHELL` is the FIRST arm of `ShellConfig::detect()` (`ops/shell.rs:101-105`, ahead of the `/bin/bash` probe at `:108-110`), which is the default path for both `ToolRegistry::with_builtins` (`registry.rs:54`) and `Backend::default()` (`ops/mod.rs:359-361`); it is structurally excluded from `session_env_scrub_keys()` (built from `SESSION_ENV_SUFFIXES` × `{CYRUP_,PI_}`, `config.rs:31-48`), so it survives into subagent re-execs; nothing records the resolved interpreter anywhere; and the substitute need not be a shell, since the value goes straight to `get_bash_shell_config`. pi's `getShellConfig` (`utils/shell.ts:67-120`, byte-identical at both tags) reads **no** env var as a shell selector. **Decide it in ONE change with `TOOL-007`** (protected-path write block, on by default, no pi analog, bypassed by `bash`) — see OQ-9.
- **`PERM-023` — *high***: the permission extension's install probe never consults `agents_dir`, which the manager enforces, so an operator whose only policy artifact is agent-markdown frontmatter gets no gate attached and silently inert deny rules.

Also in this class: `TOOL-038` (on Windows with no bash, `bash` silently falls back to `cmd.exe /C` where pi refuses to run), `AGENT-016` / `AGENT-033` (panic handling that diverges from pi's failure semantics — both bounded by `[profile.release] panic="abort"`, which is why they stay medium/low), `SUBA-039` (`SpawnedChild` has no `Drop` guard, so a dropped drive future orphans a process group).

---

## 6. Open questions — need a human decision, not more analysis

1. **The npm package channel (PB-7).** The extension half is genuinely blocked — WASM guests cannot load a TypeScript extension. The skills/prompts/themes half is not blocked. Do we (a) support `npm:` for resource-only packages, (b) support it fully by treating `extensions` entries as unsupported-but-skipped, or (c) delete `npmCommand` from settings so it stops advertising a capability? Today it is (d): the setting exists and does nothing, and the error message blames OCI. The code comment at `package/source.rs:81` cites a requirement id (`R-09-021`) that **cannot be checked from this workspace** — not a decision of record.
2. **The compact-read `docs` arm (PB-4).** Upstream resolves against `dirname(getReadmePath())` — the shipped npm package tree. A Rust binary ships no such tree beside it. Porting the behaviour requires deciding what cyrup's "shipped docs root" is (embedded? install-relative? nothing?). Area 04 states the blocker precisely: **a packaged-docs locator has to exist before the arm can be written.**
3. **Windows (PB-19).** The broker's Unix-only bind is unambiguous, but `crates/` carries 161 `cfg(unix)` sites against 6 `cfg(windows)`, so this may be a property of the whole port. If Windows is out of scope for the binary, PB-19 reduces to its second half (the client resolves TCP targets a cyrup broker never serves, and the ported listen resolver is dead) — still real, but smaller. **This question also governs `DRIFT-046` (`normalizeWindowsShellPath`), `TOOL-036` (the win32 leg of `normalizePath`) and `TOOL-038`** (the `cmd.exe` fallback), so answer it once for all four. Note that `TOOL-036`'s `~`/`os.homedir()` half is a v0.83.0 parity bug on **every** platform and does not wait on this answer.
4. **SDK-surface parity vs behavioural parity.** `VL-P5`/`DRIFT-047` (telemetry), `SESS-038` (`session-backends/sqlite-node`), `SEAM-058` (`packages/{server,protocol,client}`) and `PROV-031` are all embedder-facing with no user-visible symptom in the cyrup binary. Is SDK-surface parity in scope, or do we track it separately? **Four separate area files asked this independently**, which is the signal that it needs deciding rather than re-litigating per item. `SESS-038` and `SEAM-058` are now trackers, as is `AGENT-028` (the other owner of the telemetry half), precisely because the answer — not more analysis — is what unblocks them.
5. **REFUTED 2026-08-14 (fourth edition) — CATALOG ACCURACY *IS* STATICALLY AUDITABLE, and this question is no longer open on its stated premise.** Filed as **`PROV-060`**. The "every `*.models.ts` is a two-line re-export" premise below is true only from **`a9f6a3159`** onward — the single commit that both gitignored `providers/data/` and converted the files to re-exports. Its **direct parent `b0c2a90e` still carries the full data literals, and `b0c2a90e` is the revision cyrup's own `catalog_manifest.json` names as its provenance floor.** `git log --oneline b0c2a90e..a9f6a3159` returns exactly one commit. So the whole catalog is checkable with `git show b0c2a90e:packages/ai/src/providers/<name>.models.ts` plus a short parser — no generator run, no `npm install`, no network. **It was done in this pass: 35/35 catalogs parsed, 1072 upstream models vs 1078 cyrup, 1027 compared field-by-field, yielding 25 missing models (`PROV-057`), 16 extra (`PROV-058`), 28 compat-flag differences and 119 non-compat field differences (`PROV-059`) — including the wire-API mismatch that is now the high `PROV-054`.** Nine sweeps inherited the "unverifiable" verdict; the data was one `git show` away the whole time. **`PROV-018`'s `xtask` generator is still the right fix site** — the same recipe is the drift check it needs — but it is no longer blocked on an unanswerable question. **Two caveats travel with every catalog claim derived this way:** `b0c2a90e` is **13 days earlier than v0.83.0**, so a clean refresh still leaves an unmeasurable residue and *"catalog parity at v0.83.0" is a claim about `b0c2a90e` plus an unbounded delta*; and a regeneration will **re-introduce** groq `qwen/qwen3-32b`'s deliberately-removed `thinkingLevelMap` (`PROV-064`) unless the generator carries a named exception list. *The original question is retained below for provenance; its factual premise is dead.* ~~**Catalog accuracy (VL-P25 / `PROV-004` / `DRIFT-009`) is not *statically* auditable — but it IS auditable.**~~ pi generates `providers/data/*.json` and gitignores the output (`pi/.gitignore:11`); every `*.models.ts` at v0.84.1 is a two-line re-export, so no pricing, context-window, `maxTokens` or compat-flag claim about the 35 embedded catalogs can be checked by reading this workspace. **The generator itself is committed at both tags** (§4), so the honest options are: accept structural parity only, or run pi's `generate:models` / `diff:model-catalog` and diff the result. **`PROV-018`'s `xtask` generator is the highest-leverage tooling item in the port** for exactly this reason, and `DRIFT-009` now defers to it rather than proposing the lossier pi.dev-artifact path.
6. **The first-run wizard (UW-2).** The standing trap list says it is "deliberately unreachable"; the code says `is_official_distribution()` is **true** for this build and the gate can fire into an empty `if` body. One of the two is wrong — and **as of the 2026-08-13 live run it is the trap list**: the gate was measured firing (the `xp` badge proves `CYRUP_EXPERIMENTAL=1` was read, the empty agent dir proves `settings_path` did not exist) straight into the empty body, on a pty where the sibling selectors rendered fine. Treat this as a product decision awaiting an answer, not as a documentation discrepancy. Decide whether the wizard ships (wire `startup.rs:256`) or does not (delete the predicate and the dead function, and correct the trap list) — it cannot stay in this state, because the current shape is the worst of both. **ANSWERED 2026-08-13 by ADR-0011: it ships.** `startup.rs`'s wizard is wired at pi's call position with pi's full condition, nothing was deleted, and the trap-list entry was struck from `README.md`. See `UW-2` above for the code, the tests, and the live run that is still owed. This question is closed; note for the record that the escalation token in `UW-2` used to read "OQ-6", which is wrong under both namespaces — this is `PARITY-PLAN` §7 **OQ-9** / `PARITY-GAPS` §6 q6.
7. **pi's agent-harness v2 (VL-P22 / `AGENT-028` / `DRIFT-040` / `SESS-038`).** ~11.4k insertions / ~10.9k deletions in `packages/agent/src/harness/**` in this delta — an `agent-harness.ts` rewrite, a new 667-line `reducer.ts`, a new `session/` subtree with its own JSONL codec/repo/storage/state and a 993-line conformance suite, and a new typed telemetry layer. **No area file owns it and nobody has measured it.** Do we (a) absorb it, (b) track interop only (read harness-v2-written sessions, keep writing the coding-agent format), or (c) declare it out of scope until pi's own `coding-agent` migrates? The torn-tail bug inside VL-P22 is small and should be fixed regardless of the answer. **`AGENT-028` and `SESS-038` are the same question in two files and must be answered together.**
8. **NEW — the alt-screen/TUI-mode scope decision (VL-P19 / `TUI-019`, filed in area 07 as `OQ-07-1`).** The previous edition held `TUI-019` at *low* "as a deliberate ADR-0001 divergence". That justification is dead twice over: ADR-0001 is **unreadable in this workspace** (§7), and even a real ADR would not hold it down, because a mechanism difference that costs behaviour stays as work. It is now rated on consequence — *medium*, for no fullscreen mode, no mouse scroll, no scrollbar and no jump-to-prompt, four normal-path features — and the underlying question is what needs a human: **does cyrup ship a fullscreen TUI mode at all?** (a) port it (effort **L+**, an application layer on ratatui's native alt-screen/mouse support), (b) support the flag and settings key as accepted no-ops with an explicit not-supported message, or (c) declare it out of scope and say so in the flag's error text. **`SEAM-051` and `CFG-021` must be fixed under every one of those answers and must not wait on it** — today the flag's default value makes the binary refuse to start.
9. **NEW — what is `bash` allowed to be? (`TOOL-039` + `TOOL-007`, one decision).** Both are `cyrup-original` behaviour on the same surface and taking them separately produces an incoherent shell: either (i) delete the `CYRUP_SHELL` arm and require the `shellPath` setting — a three-line deletion, pi's shape, **recommended** — or (ii) keep it and do **all four** of stamp a `[CYRUP-DELTA]`, report the resolved interpreter at session start and in bash result details, add `CYRUP_SHELL` to the scrub set (it does not fit the `{CYRUP,PI}_<SUFFIX>` shape, so a second explicitly-named group is needed), and validate the path exists and is executable per `shell.ts:73`. **Half of (ii) is not an option.** The same decision governs whether `TOOL-007`'s protected-path block — on by default, no pi analog, and bypassed by `bash` — stays, is made configurable, or goes.

---

## 7. Method, and what to trust

**How this edition was produced.** Two stages. First, twelve areas were independently re-audited at
cyrup `04c1ba2` against a **named upstream tag** on both sides — never a working tree, never a
floating HEAD. Then a completeness critique read all fifteen finished files and returned 17 findings;
five repair agents applied them area by area, and this document and the README were regenerated from
the result. The class counts in §0 are mechanical: every row of every `## Open items` table tallied
by `Kind` and `Severity`, with `tracker` rows excluded by construction. Where an area file and this
document disagree, **the area file wins** — it is the one that re-read the code.

**Closure required reading the Rust at HEAD and the TypeScript at the tag.** A commit message
asserting a fix was treated as a hypothesis. That scepticism paid twice: area 01 found four follow-on
defects (`PROV-027`/`028`/`029`/`030`) *inside* the code that closed `PROV-005`, and area 11 found two
(`ICOM-027`/`043` inside `ICOM-022`'s fix, `ICOM-035` inside `ICOM-002`'s).

**Severity is now applied, not narrated.** The definition (README — data loss, silent wrong output, a
permission bypass, or a crash on a normal path) carries **no reachability qualifier**, and this pass
stopped adding one: `EXT-054` is critical although no WASM guest ships today, and its blast radius is
recorded inside the item as *scheduling* information. Two counter-cases are recorded with reasons so
they are not re-opened: `SEAM-051` is **high, not critical** — the failure is deterministic, loud,
diagnosed and one token from working, so it is a launch refusal rather than the silent/unrecoverable
class; and six items in areas 02/03/04 were tested against the definition and deliberately left where
they were (`AGENT-030` loss is race-conditional, `AGENT-016`/`AGENT-033` are bounded by
`panic="abort"`, `SESS-004` needs a fork/extension-written key, `SESS-042` is latent until `SESS-040`
lands a caller, `TOOL-035` needs a PNG with >4100 bytes of pre-`acTL` chunks). **A severity is a
consequence, not a class**, which is why two `cyrup-original` items sit in the high list.

**Citations, and the failure mode that made this pass necessary.** Every "identical at both tags"
claim in the fifteen files was re-resolved by opening the file at the named tag. The class defect —
quoting the **v0.84.1** offset while asserting it holds at **v0.83.0** — was found on the
number-one-ranked item in the backlog (`AGENT-020`: `agent.ts:350`/`:351-353` at v0.83.0, not
`:361`/`:362-364`) and on roughly twenty-five others: nine wrong-at-the-named-tag citations in area 01
alone, including a `high` that quoted a property (`isSubscription: true`) that **does not exist at
v0.83.0 at all**; thirteen more in area 02, where the `agent-loop.ts` shift is **not uniform** (0
through `:636`, +4 from `:642` on, because the block arm was rewritten); `TUI-028`'s two keybindings
offsets; `SEAM-006`'s `print-mode.ts` set; `PROV-024`'s four cites, which matched **neither** tag.
The rules that follow, and they are cheap:
- **Never write "identical at both tags".** Give the offsets per tag, each labelled with the tag it
  was read at. Byte-identical bodies do not imply identical line numbers.
- **Do not "fix" a citation by shifting it.** A previous renumber-by-uniform-shift pass introduced
  errors at 15% while looking verified.
- Area 01 proposes widening `PROV-041`'s in-tree citation lint to cover `docs/gap-analysis/*.md`,
  which would make this class machine-checkable instead of pass-dependent.

**Trackers, and why the count is cleaner for having them.** Nine rows propose no schedulable work —
they index other items, or they ask a scope question. They keep their IDs, their status rows and
their full bodies, and they are excluded from the severity census, because a number that mixes work
with bookkeeping cannot be planned against. Each records what would escalate it back (for
`SEAM-058`: the moment pi's `main()` references `experimentalCli`). Two of them are additionally
**leads** — `DRIFT-023` and `DRIFT-040`, where neither side was ever re-read — and area 12 lists the
exact commands that would settle each. **Items are held to a two-sided read; leads are not, which is
precisely why they sit outside the count.**

**Exhaustive vs sampled.**
- **Exhaustive**: the `v0.7.1..v0.8.0` permission-system diff, per shipping file; the `v0.9.2..v0.10.1` intercom window, commit by commit (14 of 14); the `v0.43.0..v0.47.1` subagents src sweep (96 commits, 12 net-new files, all read); the `v0.83.0..v0.84.1` diffs over `packages/ai/src` (52 files), `packages/coding-agent/src/core/tools/` (7 files, whole diff read), the area-03 session scope (37 files) and `packages/{server,protocol,client}` + the coding-agent CLI/modes tree (81+39 files); the CLI flag set, RPC verb set, RPC payload field set, settings key set and env var set, each diffed as a **set** rather than spot-checked.
- **Newly swept in the repair pass, because the critique proved nobody had read them**: `packages/ai/src/utils/` (`sanitize-unicode.ts`, `http-dispatcher.ts`, `json-parse.ts`, `abort-signals.ts`, `provider-env.ts`, `event-stream.ts`, `hash.ts`, `typebox-helpers.ts`) → `PROV-047`…`PROV-051` plus 9 confirmed-covered symbols and 6 mechanism-N/A carve-outs, each with its reason; `packages/coding-agent/src/bun/` (`cli.ts`, `register-bedrock.ts`, `restore-sandbox-env.ts`) → `DRIFT-051`, the rest carved out explicitly; `packages/tui/src`'s **input pipeline** (`stdin-buffer.ts`, `word-navigation.ts`, `undo-stack.ts`, `editor-component.ts`, `terminal-colors.ts`, `fuzzy.ts`) → `TUI-042`…`TUI-050`, including two criticals; `packages/coding-agent/src/cli/` (`session-picker.ts`, `startup-ui.ts`, `list-models.ts`, `config-selector.ts`, `initial-message.ts`, `file-processor.ts`) → `SEAM-061`…`SEAM-070`, **five of them high**; `packages/coding-agent/src/migrations.ts` → `CFG-048`…`CFG-051`. That five of ten items from one previously-unread directory came back `high` is the evidence for the point: **the axis, not the diligence, was the variable.**
- **Still sampled**: everything else in the 627-file pi delta. `packages/agent/src/harness/**` is **not measured at all** (OQ-7). Nine of eleven OAuth flows unread against upstream. The four large SSE decoders (~8k lines) read only along the paths each item required. `broker/mod.rs` (1559 lines) read in ranges; `transport/client.rs` (1268) and `framing.rs` (367) grepped, not read — and upstream rewrote the framing state machine at v0.9.1. Area 11's **surface-driven axis has never been run at all**: 68 top-level export declarations across the 17 non-test `.ts` files at v0.9.2, plus the tool action enum, the 8 subscribed event kinds and the 16-tag `BrokerMessage` vocabulary. Its commit-driven and citation-census axes are bounded by the drift window, so **none of them can see a symbol that existed at v0.9.2 and was never ported** — run it against `broker/` first.
- **The unwired sweep is incomplete by construction**: it indexed bare identifiers, not resolved paths, so any method whose name collides with a live method elsewhere was silently excluded (`SubagentFleetStatus::handle_key` was found only by reading). Of ~7 087 pub items it flagged 385, of which ~120 remain untriaged. **The true unwired set is larger than §2.** Closing it properly needs a type-resolved pass (`cargo +nightly rustdoc --output-format json` or rust-analyzer), not grep.

**Record what you EXCLUDED.** This is the method lesson of the repair pass and it is now README blind
spot 6. A surface dismissed as out of scope becomes invisible to every later pass, and the dismissal
is never re-examined: area 12's one-line rejection of pi's root `scripts/` as "dev/release tooling
with no runtime effect" is how the catalog generator came to be declared non-existent, and four whole
upstream subtrees went unread by anybody until this workflow. **Every sweep must publish its
exclusion list with a reason per entry**, so exclusions are auditable rather than silent. The repair
pass did this in each area's Coverage section — including the negative results, so nobody redoes
them.

**Static only.** Nothing here was built, run, tested or reproduced. Every `Verify` line in every area
file is a design, not an observation. **For TUI items this is not a formality**: ratatui `TestBackend`
unit tests pass while the assembled application has layout and empty-state bugs, so `TUI-*`,
`SEAM-061`/`062`/`063`/`066`/`067` and `SESS-040` are not done until they have been **run in a real
terminal**.

**Finally: `spec/`, `ADR-0001` and `CLAUDE.md` do not exist in this workspace.** Doc comments across
the codebase cite `spec/architecture/arch-NN-*.md`, `R-NN-NNN` ids and ADR numbers thousands of times.
Those are a useful search index and nothing more. **No entry in this document rests on one, and none
should.** Where a code comment invokes one to justify a divergence — as `package/source.rs:81` does
for the npm channel, `spawn/mod.rs:428` does for a 0600 mode the code never sets, and `TUI-019` did
for the whole alt-screen cluster until this pass — treat it as an unverifiable claim, not as a
decision of record.
