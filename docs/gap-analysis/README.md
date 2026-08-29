# cyrup gap analysis

A verified ledger of every behavioral difference between the **cyrup** Rust port and its upstreams,
written to be used as a work-item backlog.

**Five upstreams are ported, not four** *(corrected 2026-08-19)*: the four TypeScript ones — `pi`,
`pi-subagents`, `pi-permission-system`, `pi-intercom` — plus **`code_puppy_core_plugins`, which is
Python**, ported as `crates/cyrup-flux` and measured in [`14-cyrup-flux.md`](14-cyrup-flux.md). A
sixth, `pi-mcp-adapter`, is TypeScript and **not** ported (area 13, below). The hard rule
`git -C <repo> show <tag>:<path>` applies to all of them; only the language differs.

**Start at [`PARITY-GAPS.md`](PARITY-GAPS.md)** — it is organised by gap class (port bug / unwired /
version lag / reverse lag / deletion candidate / open question), opens with **§0a: every item above
medium in one table**, and shows the shape of the remaining distance.
[`00-residual-ledger.md`](00-residual-ledger.md) ranks and suggests an order. The numbered files hold
the evidence.

> **UPDATE 2026-08-29 — `SEAM-112` is CLOSED, so the above-medium set is 1 `critical` + 3 `high`.** The dated editions below are left as written; they are records of what was true when each was measured, not current counts. `SEAM-112` (`/resume` produces a broken session: nothing renders, bash calls repeat endlessly) is closed in both halves — the render half at `879eb4e` (2026-08-18), the repetition at the port divergence described in area 08's row: pi guards the overflow-latch clear with `stopReason !== "error" && stopReason !== "length"` and the port kept only the shared arm, so every `Length` message cleared the latch immediately before the compaction check read it and the compact-and-retry cycle had no termination condition. `TUI-092` also closed (2026-08-20) after the fifth edition was written.
>
> **RECONCILED 2026-08-19 (fifth edition) against cyrup HEAD `4fb5e40`.** This edition corrects two
> things and deliberately recounts nothing. **(1) The above-medium set turned over completely.** All
> five rows the fourth edition named — `SESS-040`, `PROV-047`, `PROV-054`, `PROV-055`, `PROV-056` —
> closed on 2026-08-15, and what replaced them is six rows it had no entry for, **three of them rated
> `critical` on arrival**: `SEAM-112` (`/resume` yields a broken session and bash calls repeat
> endlessly), `PERM-034` ("Allow Always" does not stick), `TUI-092` (progressive TUI lockup —
> **de-escalated to `high` inside this batch**, because `Ctrl+D` and `Ctrl+C` are both bound at
> HEAD), plus the highs `PROV-068`, `TUI-091` and `SEAM-113`. The set stands at **2 critical +
> 4 high**, and **`0 critical, 5 high` below is false in both directions.**
>
> **Every one of the six was filed from LIVE USE, not from reading.** Nine sweeps and a nine-surface
> enumeration produced an above-medium set of five wire-or-wiring defects a reader can see; four days
> of running the binary produced three rows rated `critical` on arrival, none of which any reading
> pass had a row for. The Caveats section's TUI rule generalises: **the above-medium set is what a
> static method is structurally worst at populating**, and the counter is hours in a real terminal,
> not a better sweep.
>
> **(2) No recount is published, and the third reason is the important one.** The twelve tables were
> being edited by other writers in the same batch; `crates/cyrup-flux` had no area file until this
> batch opened [`14-cyrup-flux.md`](14-cyrup-flux.md), so the denominator was wrong too; and **the
> counting rule stated in `00-residual-ledger.md` is not reproducible by a second reader** — an
> independent implementation of it, run over the twelve files at the fixed commit `e5c6933`, returns
> `500 rows / 153 open = 0/2/63/88`, which is the SECOND edition's figure, not the third's
> `503 / 145 = 0/2/61/82` that the same commit is supposed to reproduce. **Publish the script, not
> the rule.** Four editions have rested on a validation a prose rule cannot carry.
>
> **Two structural items this edition adds.** `crates/cyrup-tui/src/app.rs` was deleted by `40821ed`,
> so every `app.rs:NNNN` citation in this directory is unresolvable rather than stale; and a full
> citation audit measured **78.6% of the directory's `.rs` citations dangling** — see Caveats for the
> numbers, the cause and the CI guard that ends the class. **And five pieces of landed work carried
> no row at all until this batch filed them** — `TUI-093`, `TUI-094`, `CMDHINT_01` (which closed
> `TUI-078` and became `TUI-095`), the `npt_*` recursive prompt scan (`CFG-077`) and the Kimi K3
> catalog addition (`PROV-070`), plus the whole `cyrup-flux` crate. The fourth edition's "grep the
> SOURCE for `AREA-NNN` citations" rule does not catch an id that lives only in a **commit subject**,
> which is what hid every one of them. **Extend the rule to commit subjects** — and do not read a
> commit subject as landing evidence either: `e6f298d` says "land TUI-092 F5-F8" and deleted F8's
> task file without ever making the code change.
>
> *Superseded fourth-edition figures and the earlier stack, retained for provenance:*

> **Re-baselined 2026-08-12 against cyrup `04c1ba2`** (last code commit; docs HEAD `a9000b1`, branch
> `david/cyrup`, tree clean). All twelve area files were re-audited against a **named upstream tag**
> on both sides, then reviewed by a completeness critique and repaired against its findings. **117
> items closed, 207 newly filed** — most of the new ones come from auditing what the *closing* code
> actually does. Closed items keep their IDs in each area file's status table so a closure can be
> re-audited later.
>
> **RECONCILED 2026-08-14 (fourth edition) — the ninth pass was an ENUMERATION, not a sweep.**
> Counts derived from the twelve `## Open items` tables **in the working tree** (last code commit
> `5990e86`; the four surface writers' filings are not committed yet), by the third edition's rule,
> which was validated by re-running it at `e5c6933` where it reproduces `145 = 0/2/61/82` exactly.
>
> **Open set: 237 work items — 0 critical, 5 high, 88 medium, 144 low** (was 145 = 0/2/61/82), plus
> the same **10 `tracker` rows**. **606 rows now carry 360 full closure markers and 36 partial ones —
> 396 of 606 (65%).** The closure *rate* fell while eleven more rows closed, because the denominator
> grew by 103 in one pass. **93 ids were filed, 11 closed on arrival; no ID was renumbered, merged or
> deleted, and `SEAM-087`…`SEAM-099` are deliberately unallocated — do not "recover" them.**
>
> **What changed is the method, and it is the reason to read `00-residual-ledger.md`'s fourth-edition
> block before planning.** Nine sweeps read the BACKLOG against the code and closed 384+ rows. This
> pass enumerated **nine finite pi SURFACES mechanically and diffed both directions**, producing
> **191 findings — 67 missing in cyrup, 66 cyrup-original, 58 differing in shape** — an order of
> magnitude more than a late-stage sweep. **The lesson is not that the analysis was blind:** pi's
> source was fully available the entire time. It is that **an item-driven pass can only close what
> someone already wrote down**, so *"we stopped finding things"* and *"there is nothing left"* are
> indistinguishable under it. Enumeration removes the ambiguity — when all 39 CLI flags and all 73
> keybinding ids are walked, **the diff IS the answer**. Specimen: **`cyrup update --models` does not
> exist** (verified by running the binary), while the backlog already reasoned about lock contention
> "against any concurrent `cyrup update --models`".
>
> **`cyrup-original` is now a first-class class with its own count: 46 open rows of 68 filed** (was 7
> of 28). It is the class through which divergence enters *while everyone is looking at parity*, and
> it is rated by **reachability** — an advertised-but-dead surface (`CYRUP_SHARE_VIEWER_URL` in
> `--help`, read by nothing) outranks an internal helper; a mechanism port the language forces is not
> divergence at all.
>
> **Five of the nine surfaces were walked completely; four state exactly what they could not reach**
> (env vars, extension API, RPC payload shapes, provider catalogs) — see the ledger, and see
> **"Surfaces not yet enumerated"** below for what the next pass should walk.
>
> **Three new highs, all catalog data, all one fix site:** `PROV-054` (xai `grok-4.5` on the wrong
> wire API — and it is the xai default model), `PROV-055` (opencode leaks a `session_id` header pi
> suppresses, on all 16 `openai-responses` rows), `PROV-056` (kimi-coding, two wire divergences per
> request). They close through `PROV-018`/`PROV-060`'s bulk regeneration, **not individually.** And
> **`PARITY-GAPS.md` §6 q5 is REFUTED** — catalog accuracy IS statically auditable at `b0c2a90e`,
> which is what made those highs measurable (`PROV-060`).
>
> *Superseded third-edition figures, retained for provenance:*

> **RECONCILED 2026-08-14 (third edition) against cyrup HEAD `e5c6933`** (docs HEAD `0097149`,
> branch `david/cyrup`). **EIGHT** whole-backlog sweeps have now landed. The second edition below
> reconciled sweeps 3-6; this one reconciles **sweeps 7 and 8**, and — following that edition's own
> instruction to *reconcile documentation every sweep, not every four* — the gap is now two, not six.
> Every count is re-derived row by row from the twelve `## Open items` tables.
>
> **Open set: 145 work items — 0 critical, 2 high, 61 medium, 82 low** (was 153 = 0/2/63/88), plus
> the same **10 `tracker` rows**, excluded as always. **503 rows now carry 349 full closure markers
> and 35 partial ones — 384 of 503 (76%).** Three rows are new and all three are closed: `PROV-M01`
> and `TOOL-M01` were **filed AND closed in the same pass**, both from one assigned audit rather than
> from the backlog; **`EXT-M03` was filed RETROACTIVELY by this reconciliation, because the ID is
> cited five times in `crates/cyrup-ext/src/host/live.rs` and had no row in any area file** — the
> work landed in sweep 6 and only the bookkeeping was missing. **Grep the SOURCE for `AREA-NNN`
> citations at every reconciliation, not just the docs.**
> **One row was REOPENED by measurement: `TOOL-042`.** **No ID was renumbered, merged or deleted.**
>
> **The two highs are unchanged — `SESS-040` and `PROV-047`.** Areas 08, 09 and 10 still have zero
> open criticals and zero open highs between them.
>
> **`PROV-M01` is the finding to read first.** It is the **third** instance of the dropped-delegation
> class (after `RegisteredTool` and `WasmTool`) and the **first on a non-`Tool` trait** — and unlike
> the other two it is a **live behaviour defect**: `github-copilot`'s credential filter was discarded
> in the overlay configuration, so `Models::get_available` offered all 29 Copilot models regardless of
> what the OAuth credential entitled. Proven by running the new test against the pre-fix code.
> **The invariant is not "audit `Tool` impls": it is every hand-written same-trait decorator, every
> defaulted method, and a fixture value that CONTRADICTS the default.**
>
> **The measured error rate is unchanged at ≈12%** (≈56 refutations against ~465 rows worked). It has
> not moved in six editions and the honest reading is that **this is the method's floor, not a defect
> to be driven out.** Sweep 8 produced clean instances of both failure modes the second edition
> separated — doc staleness (`SEAM-017` read "not started" while the port was **1262 lines** at HEAD)
> and genuine analysis error (**`CFG-052`'s entire premise about upstream is false**) — plus a **third
> mode this edition names for the first time: a closure validated against the wrong signal.**
> `TOOL-042`'s fix cut the LEAK rate from ~12% to ~1.0% but did not stop it, and the one instrumented
> occurrence cannot be an inherited handle at all. **It was reopenable only because its closure wrote
> down its own falsification condition. Do that on every closure resting on an argument rather than
> an observation.**
>
> **ORCHESTRATION, for whoever runs sweep 9.** The feature partition still works — sweep 8's tail-a
> agent held five crates and landed the turn-budget subsystem end to end, which no per-crate agent
> could have done. **The new rule is: TREAT THE BRIEF AS A LEAD.** Two of three agents found a
> load-bearing error in their own assignment text rather than in the code — one was a prescribed
> mechanism that would have shipped a budget nothing enforced, and **one was a FABRICATED pi citation
> in the orchestrator's own brief, caught by an agent opening the file.** Fabricated citations are not
> confined to the area files. **Four fix sites recorded in the area files are wrong in the same
> direction** — `EXT-013`, `TOOL-022`/`TOOL-015`/`EXT-024`, `PROV-036` all name `cyrup-tui` but need a
> **producer in `cyrup-session-svc`** that does not exist, and `SEAM-020`'s "one line" is a **type
> error**. Route sweep 9 by fix site; one owner spanning `cyrup-session-svc` + `cyrup-tui` closes five
> rows. **Six rows are owner decisions, not agent work** (`SEAM-057`, `SUBA-025`, `SUBA-055`'s guide
> action, `SUBA-054`'s async half, `PERM-032`, and `CFG-049`'s URL rebrand) — three sweeps have now
> declined `SEAM-057` and two have declined `SUBA-025`.
>
> **The test architecture now has TWO gates, and both are green:** unit gate **6740 tests in 17.9 s**;
> integration gate **473 tests in 92 s behind `cargo nextest run -p cyrup-it --features it`**. That is
> new information for `ICOM-053`: the harness **does** run — what the row still names is that the
> 17.9 s merge gate does not invoke it. **The integration suite carries guard tests that FAIL when
> ambient credentials leak in** (`TOGETHER_API_KEY`, `CYRUP_INTERCOM`, `GITHUB_TOKEN`, …); scrub the
> environment before running it, and it is 473/473.
>
> *Superseded second-edition figures, retained for provenance:*

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
> `cyrup-it` harness crate, gate now at ~~**6699 tests, 7 skipped, in 16.3 s**~~ **6740 tests in
> 17.9 s (2026-08-14, third edition), plus a second gate: 473 integration tests in 92 s behind
> `--features it`** (was 6440 in 16.4 s).
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

> **THE `open items` COLUMN IS STALE AS OF 2026-08-19 AND THE `crit`/`high` COLUMNS ARE CORRECTED IN
> PLACE.** The counts are the fourth edition's, derived 2026-08-14; five closing batches landed on
> 2026-08-15 and none reconciled this table, so treat the middle column as a fourth-edition record
> rather than a current figure and recount from the tables. The `crit` and `high` cells were re-derived
> by NAME on 2026-08-19 at `4fb5e40` — six rows, each read in its area table — because a planner acts
> on those two columns first and every cell in them was wrong: `PROV-047`, `PROV-054`, `PROV-055`,
> `PROV-056` and `SESS-040` all closed on 2026-08-15, while `SEAM-112`, `PERM-034` and `TUI-092`
> opened as `critical` in a class this table published as empty. `TUI-092` is counted here at the
> `high` its severity cell was corrected to inside this batch; it read `critical` when this
> correction was first measured, and if that de-escalation is ever reverted, area 07 and the total
> each gain a critical and lose a high.

| file | area | open items *(2026-08-14, stale)* | crit *(2026-08-19)* | high *(2026-08-19)* |
|---|---|---:|---:|---:|
| [`../PARITY-PLAN.md`](../PARITY-PLAN.md) | **the execution plan derived from this directory — 30 batches, the next three moves, deferrals and open questions** | — | — | — |
| [`../adr/README.md`](../adr/README.md) | **decisions of record — where the nine open questions of `PARITY-PLAN.md` §7 were settled (eleven ADRs), plus the ledger changes those decisions imply** | — | — | — |
| [`PARITY-GAPS.md`](PARITY-GAPS.md) | **the same items grouped by gap class — read first.** Its §0 census and §0a above-medium table are **superseded 2026-08-14** (they enumerate the 448-item set); the class taxonomy, the per-entry fix sketches and §7 Method are current | — | — | — |
| [`REPRO-LOG.md`](REPRO-LOG.md) | **the first execution of this binary — 17 items driven through a real pty or headless, 16 confirmed / 1 refuted / 0 blocked, plus the real suite numbers (6387, not the inherited 3932) and 9 new items filed from what the binary was seen doing.** Every row carries a transcript. **Read this before trusting a severity: only 3 of 17 items survived a live run unchanged.** | — | — | — |
| [`00-residual-ledger.md`](00-residual-ledger.md) | ranked cross-cutting view | — | — | — |
| [`01-cyrup-core-and-provider.md`](01-cyrup-core-and-provider.md) | wire APIs, providers, auth, streaming, catalogs, cost | **23** | 0 | ~~4~~ **1** |
| [`02-cyrup-agent.md`](02-cyrup-agent.md) | the turn loop, tool dispatch, hooks, abort | **2** | 0 | 0 |
| [`03-cyrup-session.md`](03-cyrup-session.md) | JSONL session tree, compaction, system prompt | **8** | 0 | ~~1~~ **0** |
| [`04-cyrup-tools.md`](04-cyrup-tools.md) | the built-in tool set | **8** | 0 | 0 |
| [`05-cyrup-config-and-resources.md`](05-cyrup-config-and-resources.md) | settings, model resolution, trust, skills, packages | **30** | 0 | 0 |
| [`06-cyrup-ext.md`](06-cyrup-ext.md) | extension host, WIT world, event catalog | **35** | 0 | 0 |
| [`07-cyrup-tui.md`](07-cyrup-tui.md) | terminal UI application layer | **57** | 0 | ~~0~~ **2** |
| [`08-cyrup-session-svc-and-modes.md`](08-cyrup-session-svc-and-modes.md) | the integration seam, RPC, CLI, print/json modes | **26** | ~~0~~ **1** | ~~0~~ **1** |
| [`09-cyrup-ext-subagents.md`](09-cyrup-ext-subagents.md) | subagent delegation | **19** | 0 | 0 |
| [`10-cyrup-permission-system.md`](10-cyrup-permission-system.md) | allow / ask / deny gate | **4** | ~~0~~ **1** | 0 |
| [`11-cyrup-intercom.md`](11-cyrup-intercom.md) | supervisor↔subagent broker | **9** | 0 | 0 |
| [`12-upstream-drift-pi-core.md`](12-upstream-drift-pi-core.md) | pi core drift since the ported baseline | **16** | 0 | 0 |
| [`14-cyrup-flux.md`](14-cyrup-flux.md) | the Flux pipeline — the fifth ported upstream, and the first that is neither pi nor TypeScript. **Opened 2026-08-19; no prior edition's count includes it** | **7** | 0 | 0 |
| | **total** | ~~**237**~~ *recount* | ~~**0**~~ **2** | ~~**5**~~ **4** |

## Area 13 — the MCP adapter port

A **fifth upstream**, `pi-mcp-adapter` v2.25.0 (~24k lines of TypeScript, 203 paths), which has never
been ported. Its count is deliberately **kept out of the total above**: the twelve areas measure drift
in code that exists, while area 13 specifies code that does not exist yet, and adding 433
forward-looking port units to 237 backward-looking defects would produce a number nobody can plan
against. **The exclusion runs both ways and is now stated in every cross-cutting file: no count in
`README.md`, `00-residual-ledger.md` or `PARITY-GAPS.md` includes a row from `13-cyrup-mcp.md`,
`13a`–`13i` or `MCP-PORT-METHODOLOGY.md`, which another team owns.**

| file | area | port units | crit | high |
|---|---|---:|---:|---:|
| [`13-cyrup-mcp.md`](13-cyrup-mcp.md) | **the port — thesis, scope, seam map, architecture, and the one canonical table of every unit. Start here.** | **433** | 21 | 146 |
| [`13-cyrup-mcp-STATUS.md`](13-cyrup-mcp-STATUS.md) | **what is actually BUILT — per-unit implementation status against the plan, audited 2026-08-21 at v2.26.1. 198 of 437 units carry open work.** | 437 | | |
| [`MCP-PORT-METHODOLOGY.md`](MCP-PORT-METHODOLOGY.md) | **how it is executed and verified — fidelity rules, thirteen phases, the ADR docket** | — | — | — |
| [`13a-mcp-activation.md`](13a-mcp-activation.md) | activation, lifecycle and the host seam | 50 | | |
| [`13b-mcp-config.md`](13b-mcp-config.md) | configuration, the type model and errors | 50 | | |
| [`13c-mcp-servers.md`](13c-mcp-servers.md) | server manager, transports and the metadata cache | 50 | | |
| [`13d-mcp-proxy-modes.md`](13d-mcp-proxy-modes.md) | proxy modes and search ranking | 36 | | |
| [`13e-mcp-tools.md`](13e-mcp-tools.md) | tool registration, approval, output guard and rendering | 53 | | |
| [`13f-mcp-credentials.md`](13f-mcp-credentials.md) | credential storage, keychain and consent | 41 | | |
| [`13g-mcp-oauth.md`](13g-mcp-oauth.md) | the OAuth 2.1 flow and the callback server | 49 | | |
| [`13h-mcp-tui.md`](13h-mcp-tui.md) | the TUI panels, slash commands and prompts | 54 | | |
| [`13i-mcp-protocol-and-verification.md`](13i-mcp-protocol-and-verification.md) | sampling, elicitation, tracing and verification | 50 | | |

**The port is an extension and changes nothing in cyrup's core** — `crates/cyrup-mcp` is a native
built-in crate, the same shape as `cyrup-ext-subagents`, linking `rmcp` 3.1.2 (client-only) directly.
**Four surfaces are cut by owner decision** — the legacy HTTP+SSE transport, MCP Apps, the raw
unix-socket transport, and `mcpScript`/the JavaScript worker — which is why there is **no section 09**
(it would have held MCP Apps) and why the port contains no hand-written protocol code and no
JavaScript engine question.

**Area 13 carries no line numbers and no commit shas, deliberately, and the rest of this directory is
the cautionary tale that produced that rule.** Its first edition pinned cyrup line citations; the
repository advanced a commit *during* the analysis, and a completeness critique then measured **37% of
those citations sitting on already-drifted files**, with 21 of 25 hand-checked citations wrong for the
revision a reader would actually use. Area 13 references cyrup by **symbol and file** only. Caveat
inherited from the whole directory: it is a static analysis, nothing was built or run, and every
`verify` line is a design rather than an observation.

**Area 13 also produced a finding about cyrup rather than about the port.**
`ExtensionHost::refresh_tools` returns the *guest* materializer's verdict, and under the default
`wasm-host` feature that materializer reads a different map than a natively late-registered tool is
written into — so the tool never reaches the running agent, and `take_tools_dirty`'s `swap` destroys
the signal rather than deferring it. Dormant only because `register_late_tool` has zero callers
anywhere in the workspace. Filed as `MCP-037a`.

*Third edition, 2026-08-14 (after sweeps 7-8). Changed from the second edition: area 05 12 → 9
(`CFG-045`, `CFG-051`, `CFG-052`), area 06 24 → 23 (`EXT-060`), area 08 7 → 6 (`SEAM-017`), area 09
20 → 17 (`SUBA-008`, `SUBA-030`, `SUBA-035`). Area 04 holds at 5 in a changed composition —
`TOOL-024` closed, `TOOL-M01` filed+closed, and `TOOL-042` **reopened as medium**. Areas 01, 02, 03,
07, 10, 11 and 12 did not move; `PROV-M01` was filed and closed inside area 01 without changing its
count.* **Area 12 is now the least-worked file in the directory by closure rate (14 of 34 = 41%,
against a directory average of 76%) — no sweep since 3 has owned it, which is a scheduling fact
rather than a difficulty one.**

Counts are the `## Open items` table of each file, re-derived 2026-08-14 (**third edition, after
sweeps 7-8**), row by row with the counting rule stated in `00-residual-ledger.md`. **They were NOT
re-derived on 2026-08-19 and the fifth edition explains why** — the tables were being edited in the
same batch, area 14 did not exist when they were taken, and **the counting rule is not reproducible
by a second reader**: an independent implementation of it returns the *second* edition's figures at
the *third* edition's commit. Publish the script with the next recount. **One row,
`AGENT-S04`, carries `*(partially-closed)*` in place of a severity and is therefore in neither the
open total nor the closed one** — that has been true for three editions and is recorded rather than
silently rated. **Every file now carries exactly one such table** — area 03's second table was the last
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

## Surfaces not yet enumerated — named future work

**Added 2026-08-14 (fourth edition).** The ninth pass enumerated nine finite pi surfaces and filed 93
ids. This section names what is **left to walk**, so the next pass picks a surface off a list instead
of guessing — which is the whole point of the method. A surface qualifies if it is **finite and
mechanically extractable on both sides**; anything else is a sweep, not an enumeration.

**Residuals of the four surfaces that were walked INCOMPLETELY** (details and citations in
`00-residual-ledger.md`'s fourth-edition block):

| residual | what is unwalked | extraction |
|---|---|---|
| env vars — reverse direction | ~110 `CYRUP_SUBAGENT_*` / `CYRUP_INTERCOM_*` names never walked back to pi-subagents / pi-intercom, so `CFG-074`'s nine confirmed cyrup-originals **may not be all of them** | `grep -rhoE '"(CYRUP\|PI)_[A-Z0-9_]+"' crates/ \| sort -u` against each sibling upstream at its tag |
| env vars — pi-mcp-adapter | extracted, **never diffed** (25+ names) | **routed to the MCP team's files, not to this directory** |
| extension API — citations | the non-`types.ts` citations (`agent-session.ts`, `tui.ts`, `event-bus.ts`, `exec.ts`, `agent/types.ts`, `project-trust.ts`, `tool-definition-wrapper.ts`) were spot-checked, not resolved; `tui.ts:773-788` is still only "plausible" | the citation-lint test both `EXT-072` and `EXT-073` specify — resolve every `<file>:N` against the checked-out tag and assert the cited line contains the cited symbol. **Land the guard, not just the rewrite** |
| RPC — payload shapes | commands, events, envelopes and `RpcSessionState` are 1:1; the **response DATA shapes behind the 32 commands** were only checked where a finding was suspected | extract each `case "<cmd>"` return object from `rpc-mode.ts` @v0.83.0 vs each arm of `crates/cyrup-modes/src/rpc.rs` |
| providers — request bodies | the compat matrix is exhaustive; **request-body fields beyond compat** are not | per wire API, diff the assembled request object against `crates/cyrup-provider/src/api/*.rs` |
| providers — catalog residue | the catalogs are measured at **`b0c2a90e`, 13 days before v0.83.0**, and the data is genuinely not in git after `a9f6a3159` | unfixable by reading; needs `PROV-018`'s generator run |

**Surfaces never enumerated at all.** Each is finite, each has a one-command extraction on the pi
side, and none has ever been walked end to end:

- **Session JSONL entry types and their fields** — every `type` discriminant and every field pi
  writes into a session file, vs `crates/cyrup-session`'s `Entry` enum. Area 03 has closed items on
  individual fields; nobody has diffed the *set*. The `cwd`-writing bug (`SESS-037`) is the kind of
  thing this finds.
- **System-prompt sections and their exact text** — `core/system-prompt.ts` assembles a fixed list of
  blocks; the port has already produced three separate wording-drift items (`SESS-019`, `SESS-024`,
  `SESS-035`) found one at a time.
- **User-visible error messages and exit codes across the binary** — pi's throw/exit sites vs
  cyrup's. `SEAM-101` (config exits 0 where pi exits 1) and `SEAM-104` (a bare `-` became a prompt)
  were both found incidentally by the CLI walk; the surface itself was never enumerated.
- **Tool-result `details` payload shapes** — the tools surface diffed 6 of them and found `TOOL-044`
  on the seventh look; the remaining serialized payloads that reach the session file are unwalked.
- **Theme tokens and colour roles** — a closed finite list on both sides, and `EXT-066` ("the live
  theme is the one theme a guest cannot read the colours of") says the seam is already thin.
- **Autocomplete providers and their trigger characters** — `@`, `/`, and the extension-registered
  tier; `TUI-077` found the slash half by accident.
- **The three sibling upstreams' own CLI/env/config surfaces** — `pi-subagents` v0.47.1,
  `pi-intercom` v0.10.1, `pi-permission-system` v0.8.0. **Every surface in the ninth pass was walked
  against `pi` only.** Areas 09/10/11 have 32 open rows between them and not one of them came from an
  enumeration.
- **Agent frontmatter / `agents.md` schema keys** and the permission rule grammar (action names,
  policy-file keys, match syntax) — both finite, both authoritative, both never diffed as sets.
- **Markdown block types and the transform pipeline** — `EXT-019`'s forward-port landed the
  mechanism; the block-type set was never enumerated.
- **pi's shipped docs as a surface** — `docs/settings.md`, `docs/keybindings.md`, `docs/rpc.md`. The
  keybinding walk settled its own count with
  ``git -C pi show v0.83.0:packages/coding-agent/docs/keybindings.md | grep -c '^| `'`` ⇒ 73. **A
  shipped doc is an independent enumeration of an implementation surface and is the cheapest
  cross-check available.**

**Two rules for whoever runs the next one.** Emit the extraction commands as a first-class field of
the artifact — this pass's own `surfaces.json` lost them, and its catalog parser now has to be
rewritten. And **report the reverse direction explicitly**: `cyrup-original` findings only exist
because both directions were diffed, and they were 66 of 191.

## Baselines measured against

| repo | HEAD | cyrup ported baseline | latest tag | delta |
|---|---|---|---|---|
| `cyrup/` | **`4fb5e40`** — fifth edition, 2026-08-19, branch `david/cyrup`. *Superseded: `e5c6933` at the third edition (docs `0097149`), `bdcb0d0` at the second, `380c713` at the first, `04c1ba2` at the re-baseline.* | — | — | 18 crates, ~482k lines of Rust under `crates/` |
| `pi/` | `581d75a89` = `v0.84.1-117-g581d75a89` | **v0.83.0** | **v0.84.1** | 627 files, +52 291 / −17 556 |
| `pi-subagents/` | `9e9fd13` | **≈v0.43.0** (inferred — the crate records no version string) | **v0.47.1** | 151 files, +10 254 / −1 333 |
| `pi-permission-system/` | `9affcc9` | **v0.7.1** | **v0.8.0** | 28 files, +4 023 / −1 851 |
| `pi-intercom/` | `30dcbdd` | **v0.9.2** — *not v0.7.0; every prior doc had this wrong* | **v0.10.1** | true window `v0.9.2..v0.10.1` = 24 files, +2 495 / −700 |
| `code_puppy_core_plugins/` | `8de5184` | **v0.0.6** — *not recorded anywhere in `crates/cyrup-flux`; see `FLUX-007`* | **v0.0.6** | Python, not TypeScript. Ported surface is `flux_bootstrap/` — 18 bundled commands, 4 `_docs` files, 3 renderer scripts. cyrup ships 15 templates + 3 native renderers = the same 18 |
| `pi-mcp-adapter/` | `14c0e6c` = `v2.25.0-4-g14c0e6c` | **not ported** — area 13 is the plan | **v2.25.0** (tagged 2026-08-13) | 203 paths / 164 `.ts` at the tag, ~24 200 lines; drift to HEAD is 17 files, +543 / −69 |

Three standing hazards in this table. **(a)** The intercom baseline is the one that bites in both
directions: diffing from v0.6.0 or v0.7.0 reports a pile of already-done work as debt, and
~~`crates/cyrup-intercom/src/lib.rs:2` still says v0.6.0 (tracked as `ICOM-012`).~~ **CORRECTED
2026-08-19: that claim is FALSE and was numerically valid, which is why no renumber pass could see
it.** `crates/cyrup-intercom/src/lib.rs:1-3` reads *"a 1:1 source port of `pi-intercom` **v0.9.2**,
with the v0.9.3/v0.10.x deltas listed in `docs/gap-analysis/11-cyrup-intercom.md` ported
item-by-item"* — the crate agrees with this table. Re-verify `ICOM-012`'s premise in area 11 before
scheduling it. **The class this belongs to is the one a citation audit cannot reach: a true line
number carrying an untrue claim.** Diff `v0.9.2..v0.10.1`. **(b)** pi HEAD is **117 commits past v0.84.1**, so that range is unanalysed by
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

### Killed claims — disproved hypotheses about `crates/cyrup-tui`, do not re-file

Rescued 2026-08-14 from §8 of `docs/audits/2026-08-09-tui-presentation-fidelity.md`, which
**ADR-0009 stamped non-normative** — a non-normative document cannot discharge the one job these
rows exist for, which is to stop the next pass re-filing a hypothesis that has already been
refuted with evidence. Each row is a claim that was *investigated and killed*; the evidence is the
part that matters, so it is carried across verbatim in substance. Re-file any of these only by
first refuting the evidence quoted here.

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
  is invisible to every static read: the source — the `CompactionStart` arm, now
  `app/events_fold.rs:195-223` — sets the indicator and looks correct. Only running it shows the band never reaches the screen. `TUI-N13` is the mirror image —
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
- **Citations in this directory are 78.6% dangling, measured 2026-08-19 at `4fb5e40`** — 4 119 of
  the 5 241 scoreable non-`app.rs` `.rs` citations (of 6 249 total, counting the 1 914 relative
  `` `:NNN` `` continuations no pass has ever touched) point at a line that no longer holds what the
  prose says. The cause is concentrated, not diffuse: **3 336 of the 4 335 absolute citations (77%)
  were written in one commit, `72cd292` on 2026-08-13**, and `git rev-list --count 72cd292..HEAD` is
  105 with `+137 184 / −33 380` across `crates/`. Treat a citation as a lead, not an address. The
  repair is mechanical — recover the cited line's TEXT from the commit that last touched the doc line
  and re-find it at HEAD, which resolves 69% uniquely — and the standing guard is a CI check that
  resolves every `<file>.rs:<line>` and fails on any line or range end past EOF.
- **`crates/cyrup-tui/src/app.rs` does not exist.** `40821ed` split it into
  `crates/cyrup-tui/src/app/` (33 modules), so every `app.rs:NNNN` in this directory is
  unresolvable rather than merely stale, and the only honest repair is to re-find the symbol.
- Do not "fix" a citation by shifting it. A previous renumber-by-uniform-shift pass introduced errors
  at 15% while looking verified. **Now measured: only 14% of same-file citation groups share a single
  offset — `transcript.rs` drifts in six distinct bands, `cyrup-session-svc/src/session.rs` in 65 —
  so a per-file `sed` corrupts more citations than it fixes.** Re-resolve the line by reading the file at the named tag — and
  **never write "identical at both tags"**: the repair pass found ~25 citations quoting a v0.84.1
  offset while asserting it held at v0.83.0, including one on the highest-ranked item in the backlog.
  Byte-identical bodies do not imply identical line numbers, and the shift is often non-uniform
  within a single file.
- The count is a floor (blind spot 1) *and* contains known duplication (see Contents). Do the
  duplicate reduction before a plan books the same fix twice.
