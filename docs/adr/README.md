# Decisions of record

This directory is cyrup's decision of record. Eleven ADRs settle the nine open questions the port
stopped at (`../PARITY-PLAN.md` §7) plus two decisions that were made in code and never written
down; one further file is evidence, not a decision.

Before this batch, `docs/adr/` did not exist while ~2 195 in-source citations pointed at it and at a
`spec/` tree that is unrecoverable. That is the condition these files end.

## The eleven decisions

| ADR | decides | the decision, in one line | unblocks | status |
|---|---|---|---|---|
| [ADR-0001](ADR-0001-tui-substrate.md) — TUI substrate | batch 2's "write it or delete every reference"; the TUI half of OQ-6 | ratatui + crossterm is the substrate, and the carve-out covers **drawing only** — if an upstream line does not draw, it is in scope | batches 5, 6, 30; TUI-004/015/019/042–050/N06/N07; 20 in-source citations now resolve | accepted |
| [ADR-0002](ADR-0002-extension-io-is-serde.md) — extension I/O is serde | the ADR-0002 half of OQ-6 (6 citation sites) | every value crossing the extension boundary crosses **as a value**, on the WASM *and* native tiers; the encoding never licenses dropping a serde-representable field | batches 17, 18, 19 (member list now derivable), 20; splits EXT-013/021/045/050 | accepted |
| [ADR-0003](ADR-0003-bash-scope.md) — what `bash` is | **OQ-1** (`TOOL-039` + `TOOL-007` as one) | option (i): delete the `CYRUP_SHELL` arm, add nothing to compensate, and default `protect_paths` to `false` | batch 9 in full (14 items) and batch 10 transitively (15) | accepted |
| [ADR-0004](ADR-0004-agent-harness-scope.md) — agent-harness scope | **OQ-2** | port the behaviour the harness pins, not the harness: harness-v2 is a published SDK pi's own binary does not consume, so it contributes **zero** behaviour to be 1:1 with | batch 2; batch 18 loses its measurement task; the `PARITY-PLAN.md:262` branch does **not** fire; closes 4 trackers | accepted |
| [ADR-0005](ADR-0005-alt-screen-tui-mode.md) — alt-screen TUI mode | **OQ-3** / `OQ-07-1` / `PARITY-GAPS` §6 q8 | **port it** — the mechanism-impossibility argument is refuted by cyrup's own code; batch 30 splits into 30a (21 items, L) and 30b (`TUI-019`, L+, units B-1…B-14) | batch 30; TUI-019; CFG-021; SEAM-051's interim; closes `DRIFT-022` | accepted |
| [ADR-0006](ADR-0006-upstream-chase-cadence.md) — upstream chase cadence | **OQ-4**; §6 rows 1 and 13 | pin each upstream to its latest **tag**, re-baseline on the tag event; split "baseline" into three fields; re-baseline permission-system and intercom today | batch 2; batch 3 (+2 items); batches 18, 21–22, 24–26 keep position; deletes the post-26 rebase batch | accepted |
| [ADR-0007](ADR-0007-windows-scope.md) — Windows scope | **OQ-5** / `PARITY-GAPS` §6 q3 | Windows is **in scope** — pi gates its releases on Windows binaries; 17 of 18 crates already cross-compile and one file blocks the rest | batch 2; batch 9 (TOOL-036/038, DRIFT-046); PB-19 splits forward; a new area 13 | accepted |
| [ADR-0008](ADR-0008-requirement-ids-and-sdk-surface.md) — requirement ids & SDK surface | **OQ-6**, both halves | the `spec/` tree is gone and unrecoverable; all ~2 195 citations are a **grep index with no authority**; SDK parity is by **capability**, not export list | batches 2, 3, 7 (PERM-009 deletes cleanly), 19; every batch gains a reviewer's ground to reject a citation-only justification | accepted |
| [ADR-0009](ADR-0009-tui-fidelity-doc.md) — `TUI-FIDELITY.md` | **OQ-7** / `OQ-07-2` | it is an **executed audit, not a backlog** — all ten batches shipped, 46 of 117 rows re-read at HEAD found 46 landed / 0 open; archive it, merge nothing | batch 30's scope; the plan's coverage claim; `TUI-016`'s fix shape; area 07 blind spot 1 | accepted |
| [ADR-0010](ADR-0010-oauth-acquisition.md) — `CFG-005` credential acquisition | **OQ-8**, the `CFG-005` half only | withdraw the deprioritisation and schedule **four** (not two) api-key login bodies into batch 11 beside `PROV-003`; the providers are not un-loginnable, they **lie** | batch 11; batch 12's `PROV-030` precondition | accepted |
| [ADR-0011](ADR-0011-first-run-wizard.md) — first-run wizard | **OQ-9** / `PARITY-GAPS` §6 q6 (`UW-2`) | **wire it** — pi ships and invokes the wizard, cyrup has a complete unit-tested port and no caller; the "deliberately unreachable" trap is inverted | batch 14; batch 2; move 1's repro row; the known-traps list | accepted |

## Decisions added since

| ADR | decides | the decision, in one line | unblocks | status |
|---|---|---|---|---|
| [ADR-0028](ADR-0028-cyrup-acp-type-design.md) — `cyrup-acp` type design | the Rust type-design question for the unwritten `crates/cyrup-acp`, raised by area 15 | **explicit domain enums and a functional core / imperative shell split first, boundary newtypes second, and typestate essentially not at all** — an ACP agent is driven by an editor sending JSON-RPC in whatever order it likes, so every candidate lifecycle is inspected dynamically, which is the decision rule's own signal for an enum | every area-15 unit whose Rust mechanism names one of its types; `docs/gap-analysis/15-cyrup-acp.md` links to it per-unit rather than repeating the argument | accepted |

It is an **opportunity review written before its subject exists**, which no earlier ADR is. Its §5,
*Deliberately rejected opportunities*, is load-bearing rather than decorative: it is where the file
argues against applying typestate in the four places it looks attractive.


**Not a decision:** [`LEADS-SETTLED.md`](LEADS-SETTLED.md) is an **evidence record** — the two-sided
reads that closed `DRIFT-023` (superseded by `CFG-020`; kind corrected to `not-ported`) and
`DRIFT-040` (out of scope, corroborating ADR-0004 from upstream's own non-goal statement). It carries
no `Status: accepted` line, decides nothing, and holds no ADR number.

**OQ-8's other half is still open.** ADR-0010 decides the `CFG-005` question only. *"Are the ~163
non-user-observable lows ordinary work items or a mechanically-executed conformance suite?"* is
**undecided** and unowned. It is the one §7 question this batch does not close.

## The convention going forward

**Numbering.** ADRs are `docs/adr/ADR-NNNN-<slug>.md`, numbered sequentially from the highest
existing number — but **0012 through 0027 are claimed and are not free**. `docs/gap-analysis/MCP-PORT-METHODOLOGY.md`
reserves that whole block for the MCP port and cites `ADR-0012` by name in four places, so the block
is spent whether or not the files exist yet. `ADR-0028` is written (below); **the next free number is
ADR-0029.** Numbers are never reused and never renumbered, including
`ADR-0001` and `ADR-0002`, which this batch deliberately re-used because 26 in-source tokens already
spend them on the same two subjects (ADR-0008 §A.5 records that as a live ambiguity closed per-site
by reading). `LEADS-SETTLED.md` holds no number. A slug that turns out to be wrong stays put —
`ADR-0010-oauth-acquisition.md` decides an `ApiKeyAuth` question with no OAuth in it, and the path is
kept stable because it is the address other documents cite.

**Citing one.** Always by **path** (`docs/adr/ADR-0001-tui-substrate.md`), never by the bare token
`ADR-0001`, which is now ambiguous between a lost document and a present one. New `R-NN-NNN`,
`R-ARCH-*`, `arch-NN`, `func-NN` and `spec/…` ids are **never minted** (ADR-0008 §A.4).

**`OQ-N` disambiguation — two independent namespaces.** `PARITY-GAPS.md` §6 (`:826-836`) carries its
own nine numbered open questions whose numbers do **not** match `PARITY-PLAN.md` §7's. Four ADRs hit
this collision independently. **Unqualified `OQ-N` means `PARITY-PLAN.md` §7**; the other list is
always cited as `PARITY-GAPS §6 q<N>`. The mapping:

| `PARITY-GAPS` §6 | q1 | q2 | q3 | q4 | q5 | q6 | q7 | q8 | q9 |
|---|---|---|---|---|---|---|---|---|---|
| `PARITY-PLAN` §7 | — (PB-7) | — (PB-4) | **OQ-5** | ⊂ **OQ-6** | — (catalogs) | **OQ-9** | **OQ-2** | **OQ-3** | **OQ-1** |

**What `Status: accepted (decided by default under the parity rule — overridable)` means.** Every ADR
here carries that line. It means: nobody was asked. The project's standing rule is 1:1 behavioural
parity with pi with **no accepted-divergence category**
([`../gap-analysis/README.md:274-276`](../gap-analysis/README.md)), so the default answer to "is X in
scope?" is **yes, port it**, and a decision not to port must rest on an actual impossibility or a
stated project constraint — never on effort. Where the evidence did not force a different answer,
these ADRs took the default and said so, rather than escalating and blocking thirty batches. The
maintainer delegated these and asked not to be blocked; "overridable" is not a hedge, it is the
statement that a maintainer sentence outranks the default.

**Overturning one.** Every ADR ends with a *How to reverse this* section naming (a) the sentence a
maintainer would have to say, (b) what would have to change in the tree, and (c) the cost of the
option that was rejected. To overturn: say the sentence, and the ADR's own reversal section is the
work order. Two of them reverse **automatically on evidence** — ADR-0004 when its tripwire fires
(pi's `coding-agent/src` importing `AgentHarness`), and ADR-0006's pins the moment an upstream cuts a
new tag. An ADR is never silently amended: it is superseded by a higher-numbered ADR that names it.

**The three governing anchors**, re-resolved by reading `../gap-analysis/README.md` at HEAD (all
fifteen gap-analysis documents cite two of them at stale offsets, and the third not at all):

- **`:130-135`** — severity is never held down by an unverifiable justification.
- **`:268-273`** — a code comment invoking an `R-NN-NNN` id or an ADR to justify a divergence is an
  unverifiable claim, not a decision of record.
- **`:274-276`** — there is no "accepted divergence" category.

## Contradictions resolved in this pass

The twelve files were written by twelve agents that could not see each other's work. These are the
conflicts that existed between them, and what was changed. Nobody needs to re-derive these.

| # | the conflict | resolution — and what was edited |
|---|---|---|
| 1 | **Where the governing rules live.** ADR-0001 cited `README:268-273` / `:274-276`; ADR-0008 cited `:133-135` / `:271-277`; ADR-0005 cited `:274-276`. | Read at HEAD: there are **three** rules, at `:130-135`, `:268-273`, `:274-276`. ADR-0008's `:271-277` straddles two bullets and is superseded; the two-anchor habit loses the severity rule entirely. **Edited ADR-0008** (canonical block + two downstream sites) and **ADR-0001** (adds the third anchor). |
| 2 | **Two `OQ-N` namespaces.** ADR-0003 wrote "`PARITY-GAPS` §5 item 9 / OQ-9" for a question that is OQ-1; ADR-0004 flagged an OQ-2/OQ-7 clash; ADR-0005 flagged OQ-8; ADR-0011 called `PARITY-GAPS.md:508`'s "OQ-6" simply wrong. | Root-caused: `PARITY-GAPS` §6 runs its own numbering. Convention above is now binding, with the mapping table. **Edited ADR-0003** (§5→§6, OQ-9→q9=OQ-1), **ADR-0004** (flagged→root-caused), **ADR-0005** (local→general), **ADR-0011** (the token is *ambiguous*, wrong under either reading, same repair). |
| 3 | **`SESS-038`: closed or held?** ADR-0004 closes it as out of scope on a measurement; ADR-0008 said "tracker **held**, re-classify the reason". | ADR-0004 wins the disposition — it made the measurement, and `03-cyrup-session.md:124` required `SESS-038` be answered *together with* `AGENT-028`, which only OQ-2 could do. ADR-0008's contribution survives as the *reason* (not an SDK question). **Edited ADR-0008** in three places; **ADR-0004** states the meeting point. `SEAM-058` is unaffected and stays a tracker. |
| 4 | **Batch 30's shape.** ADR-0009 wrote "batch 30's items list is unchanged … `TUI-019`-after-OQ-3"; ADR-0005 splits batch 30 into 30a/30b and makes `TUI-019` unconditional. | Compatible once scoped: OQ-7 changes no count, OQ-3 changes no membership. **Edited ADR-0009** to say "unchanged *by this decision*", drop the dead contingency, and defer the split to ADR-0005; **ADR-0005** gained a read-with-ADR-0009 note (the presentation tail gains zero rows, so 30a's 21 items are the whole of it). |
| 5 | **`CYRUP-DELTA`: mandated by one ADR, linted as a violation by another.** ADR-0002 rule 7 *requires* a `CYRUP-DELTA` note naming the pi `file:line`; ADR-0008 §A.3 lists `CYRUP-DELTA` as a divergence marker its lint **fails** unless the block names a `docs/adr/` or `docs/gap-analysis/` file. A conforming ADR-0002 note would have failed ADR-0008's lint. | Harmonised: a `CYRUP-DELTA` carries **both** halves — the *tagged* two-sided upstream citation **and** the owning ADR path or gap item id — and the two checks ship as **one** `cargo xtask lint-citations` pass, not two. **Edited ADR-0002** (rule 7 + its batch-3 bullet) and **ADR-0008** (§A.3 interaction clause). |
| 6 | **Batch 18: "loses its measurement task" vs "keeps its content".** ADR-0004 discharges the harness measurement and strikes it; ADR-0006 said batch 18 "keeps its position and content". | Both true of different things — ADR-0004 removes a *task*, ADR-0006 corrects a *justification* (`8902b4f` is a cyrup commit). **Edited ADR-0006** to name the removal explicitly. |
| 7 | **Batch 3 is five ADRs deep and none knew.** ADR-0002 (CYRUP-DELTA check), ADR-0003 (`CYRUP_SHELL` repo guard), ADR-0006 (`upstream-watch` + the `check-citations.py` repair), ADR-0007 (two Windows `cargo check` gates), ADR-0008 (`lint-citations`) each add a deliverable to the same batch, each describing it as "small". | Not a contradiction but an unrecorded collision that would have under-sized the batch. Consolidated list below; **edited ADR-0002, ADR-0006, ADR-0007, ADR-0008** to cross-reference it. |
| 8 | **`TOOL-038` / `ShellConfig::detect` decided twice.** ADR-0003 D4 and ADR-0007 §5 independently order the same fix — delete the `cmd.exe` arm, port pi's throw — but ADR-0003 alone carries the `try_detect()` fallibility consequence, and ADR-0007 cited the arm as `:140-143`. | Same instruction, no conflict; the risk was an implementer reading only one. **Edited ADR-0007** (line range → `:140-144`, verified in-tree; plus the `try_detect` note and the fact that ADR-0003 deletes `CYRUP_SHELL` in the same file and batch) and **ADR-0003** (records that OQ-5 is now answered, so its "holds under either Windows answer" hedge is discharged, not pending). |
| 9 | **ADR-0003 left `TOOL-036`'s ranking "following OQ-5"** while ADR-0007 answered OQ-5. | **Edited ADR-0003**: discharged, not pending — Windows in scope, strike refused, `TOOL-036` stays `low` in batch 9. |
| 10 | **ADR-0001's citation triage vs ADR-0008's citation policy.** ADR-0001 pre-declares 17 of its 20 sites as resolving; ADR-0008 §A.5 requires per-site triage by reading and forbids the bare token. | Not in conflict — ADR-0001's triage *is* §A.5 performed for its own subject. **Edited ADR-0001** rule 6 to defer to ADR-0008 on citation policy explicitly and to require the path form. |
| 11 | **ADR-0008 §C could be read as pulling the harness back in** ("every capability pi exports … must exist"). | Scoped: §C ranges over `packages/coding-agent/src/index.ts` (the product surface), not `packages/agent`'s published `pi-agent-core` SDK, which is ADR-0004's. **Edited both** to state the boundary and the single point where they meet (ADR-0004's tripwire). |
| 12 | **ADR-0004's tripwire is lagging; LEADS-SETTLED found a leading one.** | **Edited ADR-0004** §6 to add upstream's own §20 checkbox count (10 done / 30 open at v0.84.1) as a third signal that fires *before* the other two. |
| 13 | **"448 stands" read as a global claim.** ADR-0009's ledger-counts paragraph is scoped to OQ-7, but reads as a post-batch figure while five other ADRs move the count. | **Edited ADR-0009** to scope it and point at the aggregate below. |
| 14 | **`LEADS-SETTLED.md` sits in `docs/adr/` without a status contract**, and opened with a stray `<title>` tag. | **Edited** its header: explicitly not an ADR, unnumbered, no number may be taken from it, and a list of what consumes it. Tag removed. |
| 15 | **ADR-0010's filename says `oauth-acquisition`; its subject has no OAuth in it.** | Path kept (it is the citable address); **edited** the header with a *Filename note* so nobody "fixes" the subject to match the slug. |

**Left standing, with the reason.** ADR-0001 reports `rg -c 'spec/' crates/` as **216 lines** and
ADR-0008 reports **220 occurrences** of `spec/…` paths. Different patterns counted differently
(matching lines vs matches); neither figure is load-bearing for either decision, and re-running one
regex to make two prose numbers agree would be a change with no reader.

**Not reconciled, because it is not this batch's to reconcile.** The `~163 non-user-observable lows`
half of OQ-8 has no ADR and no owner. It is stated as open in ADR-0010 and again above.

## Batch 3, consolidated

Five ADRs add deliverables to the `crates/xtask` batch. It is not small, and no single ADR sees it:

| deliverable | from |
|---|---|
| `cargo xtask lint-citations` — a line-oriented regex pass; fails any line pairing a citation token with a divergence marker outside a reviewed ~45-line allow-list | ADR-0008 §A.3 |
| …carrying, in the **same** pass, the `CYRUP-DELTA` two-part conformance check (tagged upstream citation **+** owning ADR/gap id) | ADR-0002 rule 7 |
| `cargo xtask upstream-watch` — per upstream: comparison tag, latest tag, commits between, symbol-watch verdicts; non-zero on a newer tag | ADR-0006 §4.1 |
| fix `.workflows/check-citations.py:23`'s hardcoded `WS = /home/d0m17bw/workspace`, and widen it to lint `docs/gap-analysis/*.md` | ADR-0006 |
| `cargo check --target x86_64-pc-windows-msvc` and `--target aarch64-pc-windows-msvc`, workspace-wide, as required checks (red today; no Windows host needed) | ADR-0007 §2 |
| a repo-guard test that the literal `CYRUP_SHELL` appears nowhere under `crates/` | ADR-0003 D8(2) |

## Backlog changes these decisions imply

Every ledger change the eleven decisions produce, in one place. **The next workflow applies these to
the area files**; this table exists so nothing is lost between phases. `→` means "changes to".

### Severity, kind and effort changes

| item | change | ADR |
|---|---|---|
| `CFG-005` | severity `medium` → **`high`**; kind `not-ported` → **`parity-bug`**; effort `L` → **`M`**; scope 2 flows → **4 bodies / 4 provider ids** (adds `cloudflare-ai-gateway`, `amazon-bedrock`); the "Maintainer has DEPRIORITISED" line at `05-…:432` and `00-residual-ledger.md:379-383` are **withdrawn**; title becomes wrong as written | 0010 |
| `PB-19` = `ICOM-015` | severity `low` → **`high`** (measured: the **whole binary** fails to build for Windows, 6 errors, all in one crate); `PARITY-GAPS.md:440`'s "(severity corrected down)" is **reversed**; kind/effort hold; scope narrows to the **listen half**; **split out of batch 24 and moved forward** | 0007 |
| `TUI-019` | severity `medium` **held**, kind `upstream-drift` **held**, effort `L` → **`L+`**; becomes **unconditional** — strike "conditional" at `PARITY-PLAN.md:206`, `:1190-1194` and the dead branch at `:1212-1214`; the `low` ADR-0001 justification is **permanently struck**; scope is units B-1…B-14, to be decomposed (~8 new area-07 rows) before 30b is scheduled | 0001, 0005, 0008 |
| `CFG-021` | severity `low` held; effort `L` → **`S`** for the settings half (30a), renderer half → `TUI-019`/B-5; its "**Impact** — none today" line is **wrong and must be rewritten** (a pi-written `tuiMode` is lost on a cyrup write); kind `upstream-drift` confirmed two-sided; **not** a substrate question | 0001, 0005, 0006 |
| `TOOL-007` | effort `M` → **`S`**; the three "keep it on" limbs struck; closes fully with the `builder.rs:208` flip + two doc corrections + `[CYRUP-DELTA]` stamp + tests | 0003 |
| `TOOL-039` | severity/kind hold; scope collapses to a **five-line deletion** (`ops/shell.rs:101-105`) + three tests; option (ii) limbs (a)–(d) struck from the item text | 0003 |
| `UW-2` | kind *contested / deliberately unreachable* → **`not-ported` (wiring)**; effort `S` holds; **scope grows** by the missing `cli.list_models.is_none()` conjunct and the call-site position; evidence line drops "Escalated to OQ-6" | 0011 |
| `TUI-016` | severity/kind/effort **unchanged** (medium · parity-bug · M); **fix shape corrected** — port `updatePendingMessagesDisplay` (`interactive-mode.ts:4190-4207`), **not** a footer segment (pi has none); stop discarding the texts at `app.rs:4612-4614`; delete the dead `queued` field/setter | 0009 |
| `DRIFT-047` / `VL-P5` | **kind change**: out of "SDK surface, decision pending" into ordinary in-scope port work; `DRIFT-047` keeps `duplicate-of: VL-P5` | 0008 |
| `DRIFT-023` | kind `upstream-drift` → **`not-ported`**; premise refuted (the facade is permanent; the refactor shipped v0.80.8) | LEADS |
| `PROV-033` | kind **confirmed `stale-port`**, not reverse drift — removal landed at v0.80.7, three minors *before* the baseline | 0006 |
| all **74** `upstream-drift` rows | **scope** clarified, no severity change: ordinary parity work at filed severity in the owning batch, **never** "deferred until the next bump" | 0006 |

### Closures

| item | change | ADR |
|---|---|---|
| `AGENT-028` | **split, then closed.** Harness-telemetry half out of scope (zero emission sites); live half is `packages/ai`, already owned by `VL-P5`/`DRIFT-047` — no new id. Stays tally-excluded, now for a decided reason | 0004 |
| `SESS-038` | **closed — out of scope.** No pi package depends on `@earendil-works/pi-session-backend-sqlite-node`. Also leaves OQ-6's dependent list (never an SDK question) | 0004, 0008 |
| `DRIFT-040` | **evidenced, then closed**; ceases to be a lead. Two figures confirmed, sqlite figure **refuted** (`+4,010/−1,494`, not `+12598/−3479`). Absorbed `DRIFT-037` residue closes with it | 0004, LEADS |
| `VL-P22` | **refuted on both halves and closed.** It measured cyrup against harness-v2, which pi does not run. **Do not implement** the torn-tail repair or the atomic fork publish — both would introduce divergence. `PARITY-GAPS.md:651-660` needs rewriting and its "should be fixed now" scheduling withdrawn | 0004 |
| `DRIFT-022` | **close the tracker** as `duplicate-of: SEAM-051`, resolution "decided by ADR-0005 — port it"; remove from `00-residual-ledger.md:89` and `:287` | 0005 |
| `DRIFT-023` | **closed as superseded by `CFG-020`**; keep the ID; `## Leads — not yet evidenced` is now empty and can be removed | LEADS |
| `PERM-009` | **critical held**, scope narrowed to **deletion** of `extension.rs:1651-1653` + the false parenthetical at `:1631`. The "produce the mandate" branch is **struck, not deferred** — the site cites no id at all. The *Taken on trust* caveat at `10-…:475` is resolved | 0003, 0008 |
| `SEAM-058` | **tracker held** on unreachable-upstream (not SDK scope); trigger re-verified **not fired** at pi HEAD `581d75a89` | 0006, 0008 |

### Scope, evidence and citation corrections (no severity change)

| item | change | ADR |
|---|---|---|
| `TUI-042`…`TUI-050` (9 items) | confirmed **in scope**, no substrate defence available; ratings stand. `TUI-045` explicitly: `StdinBufferOptions` as a config object is mechanism-N/A, but its 10 ms default's **behaviour** is the item. Batches 5 and 6 | 0001 |
| `TUI-N06`, `TUI-N07` | Fix option **(A) struck** ("record it in `lib.rs`'s ADR-0001 notes") — an in-source ADR note is not a closure. Both stay open at `low`/`L`; choose (B) or (C). `TUI-N07`'s session-boundary improvement is available now | 0001 |
| `TUI-014`, `TUI-029`, `TUI-030`, `TUI-033` | commitment 5(c) is a **layering** rule, not a behaviour waiver — `setWidget`/`setHeader`/`setFooter`/`setAutocompleteProvider` must still reach the screen. Ratings stand; the WIT-world work is in scope | 0001 |
| `TUI-N01`, `TUI-017`, `TUI-036` | split: the half-block raster **is** a drawing decision the ADR covers; the **capability gate** is not. No rating change | 0001 |
| `TUI-004` | both halves in scope; the mode-2031 half is a genuine crossterm mechanism gap whose behavioural cost **stays filed** — `theme.rs:1483-1492` is a note, not a closure | 0001 |
| `TUI-015` | boundary call resolved **in scope**: `MIN_RENDER_INTERVAL_MS` schedules drawing but does not draw. Kind/severity unchanged | 0001 |
| `TUI-020`, `TUI-039`, `TUI-040`, `TUI-051` | recorded as never substrate, so no future pass re-raises the defence | 0001 |
| `SEAM-051` | severity/kind/effort/batch unchanged (high · upstream-drift · S · batch 14). The `fullscreen` rejection message must be explicitly **temporary**, name ADR-0005, be grep-able, and be deleted by B-13; "not supported in this build" → **"not built yet"** | 0001, 0005 |
| `EXT-009`, `EXT-023`, `EXT-006`, `EXT-019`, `EXT-022`, `EXT-052`, `EXT-S04` | kind gains **`adr-0002 consequence`** — each needs a round-trip *designed* (an export, a bump, a fixture) before it can be estimated. Every one is a **batch-19 member by construction** | 0002 |
| `EXT-034`, `EXT-057` | **re-labelled**: consequences of single-instance **reentrancy**, not of the encoding. Deferred fan-out is legal; **silent loss at the round bound is not** | 0002 |
| `EXT-021`, `EXT-045`, `EXT-050`, `EXT-013` | **SPLIT into ADR and plain halves** — this is what lets batch 19 *shed* work rather than absorb it. `EXT-021`'s getter direction has **no representation at all** and must be re-specified or delta'd | 0002 |
| `EXT-047`, `EXT-024`, `EXT-044`/`043`/`016`, `EXT-035` | **scope corrections — explicitly NOT ADR-0002 consequences.** `EXT-047` is a mis-shaped port; `EXT-024` moves to the unrelated class; the `cwd` family is a plain omission (the host holds it four lines from the dispatch); `EXT-035` is rule 10 violated in the **native** direction | 0002 |
| `EXT-054` | **evidence correction**: `06-cyrup-ext.md:178` cites ADR-0002 for the capability-scoped sandbox; that citation is **wrong**. Re-point at `manifest.rs:2` and `host/store_state.rs:1-3`. Severity unchanged (critical) | 0002 |
| `TOOL-038` | scope **specified** (delete the `cmd.exe` arm at `ops/shell.rs:140-144`, port pi's three-option throw, add **no** opt-in setting); dependence on OQ-5 removed; detection becomes fallible → new `try_detect()` at `registry.rs:54`, `builder.rs:635`, `session.rs:4500` | 0003, 0007 |
| `TOOL-036` / `DRIFT-046` | **unblocked, not struck** — the "Prerequisite decision" holds at `04-…:593`/`:733` and the "all moot" note at `12-…:999` are discharged **against** striking. `low` holds; batch 9. `shellPath` is now the *only* user-side interpreter lever | 0003, 0007 |
| `SEAM-015` | meaning sharpens: the per-call `operations` override is the **only** sanctioned way anything but the locally resolved shell executes a bash call — the migration target for anyone reaching for `CYRUP_SHELL` | 0003 |
| `PROV-003` | Fix amended on three points: signature is `Result<Credential, OAuthError>` (**not** `AuthError`); the member must be **consumed** at `login.rs:786-802`; `api_key_strategy_supports_login` must be **deleted** in the same diff. Hard same-diff dependency of `CFG-005` | 0010 |
| `PROV-030` | batch-12 precondition relaxed — no hand-provisioned `auth.json`; the `gcloud auth application-default login` step still applies | 0010 |
| `PROV-029` | **no change**, recorded as a do-not-conflate: one field assignment per provider; must not be merged into the `CFG-005` diff or `lint-unwired`'s signal becomes unreadable | 0010 |
| `PROV-031` | `low` held; confirmed the **only** genuine SDK/crate-boundary member of the four `PARITY-GAPS.md:831` grouped; no longer blocked on a scope answer | 0008 |
| `SEAM-067` | scope clarification: `UW-2` does **not** depend on it — the wizard's own gate requires that no `settings.json` exists, so pi resolves to detected terminal polarity there | 0011 |
| `TUI-002`, `TUI-034`, `TUI-026`, `TUI-023`, `TUI-024`, `TUI-010`, `TUI-031` | **evidence pointers only** — repoint at `docs/audits/2026-08-09-tui-presentation-fidelity.md`; closures stand; `TUI-031`'s dangling note resolves through `TUI-016`'s corrected fix | 0009 |
| `ICOM-012` | scope widened: the `lib.rs:2` `v0.6.0` banner is now also the **ported-baseline census artefact**. Fix to v0.9.2 in batch 24, then v0.10.1 when 24–26 close | 0006 |
| `CFG-034` | kind `upstream-drift` **confirmed**; second counter-example to the ledger's "zero moved in" claim | 0006 |
| `CFG-012` | stays **superseded** — and is the **only** sound reverse-drift example | 0006 |
| `CFG-048` | its `migrations.rs:9-10` "intentionally NOT ported" justification is covered by the general citation rule and enforced by the lint, rather than adjudicated per item | 0008 |
| `PB-7` | its "cannot be checked from this workspace" caveat on `R-09-021` is resolved **by rule**: the id carries no authority, so the npm-channel drop is an **undecided** question to settle on its merits. Its 5 sites go on the lint allow-list | 0008 |
| `PROV-021` | note only: if it replaces anthropic's `env_key` with a bespoke strategy, that strategy needs a one-line `login` calling `env_api_key_login` | 0010 |
| `CFG-020` | absorbs `DRIFT-023` and takes three corrections: "+356 lines" → **+274/−82, net +192**; add `utils/abort.ts` (new at v0.84.1) as a prerequisite; point **Verify** at `test/model-runtime-credential-sync.test.ts` (375 lines, new at v0.84.1) instead of an invented assertion | LEADS |
| `VL-P24` | same "+356" correction; its `model-runtime.ts:94-111` and `:494` citations are exact and stand | LEADS |
| `VL-P19` | mechanism note replaced (crossterm's `EnableMouseCapture` is **not** a drop-in — three deltas: forced any-motion, rxvt `?1015h`, no focus reporting `?1004h`, which pi's selection-cancel depends on); its two "OQ-8" references disambiguated | 0005 |

### Document defects to repair (numbers, anchors, stale claims)

| where | correction | ADR |
|---|---|---|
| `PARITY-GAPS.md:654`, `:834`; `PARITY-PLAN.md:1382`, `:1419` | harness churn is **4,977 / 2,936 across 32 files**, not "~11.4k / ~10.9k" — that figure is the whole `packages/agent/`, ~70 % docs and tests. Subtree: 28 files / 7,783 lines @v0.83.0 → 41 / 9,824 @v0.84.1 | 0004 |
| `PARITY-PLAN.md:38`, `:1381` | "358 commits" for pi-subagents `v0.43.0..v0.47.1` → **96** (110 to HEAD). A 3.7× overstatement, and the load-bearing number in §6 row 1's argument against rebasing | 0006 |
| `PARITY-PLAN.md:38` | delete the `8902b4f` justification — it is a **cyrup** commit, not upstream; batch 18 is a port-side audit | 0006 |
| `PARITY-PLAN.md:39`; `gap-analysis/README.md:80` | pi-permission-system ported baseline `v0.7.1` → **`v0.8.0`** (area 10 already read the whole surface there) | 0006 |
| `gap-analysis/README.md:75-82` | split the single "cyrup ported baseline" column into **ported baseline / comparison tag / upstream HEAD** | 0006 |
| `PARITY-PLAN.md:1338-1340` | stop citing `sendSessionIdHeader` as a reverse-drift example; use `CFG-012` | 0006 |
| `00-residual-ledger.md:323-325` | "twelve items out … and **zero** in" → **fourteen named out, at least two in** (`CFG-021`, `CFG-034`). The asymmetry survives; the word "zero" does not | 0006 |
| `PARITY-GAPS.md:830`; `PARITY-PLAN.md:1447` | "161 `cfg(unix)` vs 6 `cfg(windows)`" → **162 unix sites vs 62 Windows-aware sites** (6 attribute + 30 runtime `cfg!` + 26 `cfg(not(unix))`); one of the six is a doc comment | 0007 |
| `PARITY-PLAN.md:263` | the OQ-5 branch risk is **partially discharged** — true in kind, materially smaller: 17/18 crates cross-compile, 62 sites already branch. Re-size it; do not re-plan around it | 0007 |
| `PARITY-GAPS.md:830` item 3 | rewrite: `PB-19` does **not** reduce to its second half; it grows | 0007 |
| `PARITY-PLAN.md:245`, `:1199`, `:1384`, `:1468-1474`; `07-…:46`, `:1078`, `:1114-1123`; `00-residual-ledger.md:396-400` | the "~150 presentation findings excluded from every count" caveat is **false** and must be struck; strike "add on the order of 150 rows" | 0009 |
| `07-cyrup-tui.md:70` | the stale status-row sentence "Severity stays low as a deliberate ADR-0001 divergence" contradicts `:158` and `:718-726`; delete it | 0001 |
| `PARITY-PLAN.md:259-260` | batch 2's *Verified by* (`rg` returning only resolvable references) is **unachievable and withdrawn**; substitute "`lint-citations` passes, allow-list has one reason per entry" | 0008 |
| `PARITY-PLAN.md:480-483` | batch 7's "if the mandate is later produced…" hedge is **struck** — no requirement id is cited at the site | 0008 |
| `PARITY-GAPS.md:508` | `UW-2`'s "Escalated to **OQ-6**" is ambiguous and wrong under either namespace → **`PARITY-PLAN` §7 OQ-9 / `PARITY-GAPS` §6 q6, decided by ADR-0011** | 0011 |
| `gap-analysis/README.md:168-172` | **delete** the "deliberately unreachable first-run wizard" trap — it is a gap, not a trap. Two of six entries are now known wrong; the remaining four need the same two-sided re-check (the `fleet!` entry is already smoking against `PARITY-GAPS.md:787`) | 0011 |
| seven sites (`PARITY-PLAN.md:479`, `05-…:284`, `07-…:722`/`:754`/`:1098`, `10-…:132`/`:475`) | "`README:208-212`" is about censusing baselines and says nothing of the kind → re-anchor to **`:130-135` / `:268-273` / `:274-276`** | 0001, 0005, 0008 |
| four sites citing `PARITY-GAPS.md:709` | `:709` is a pi-subagents paragraph; the ADR-0001-unreadable record is at **`:914`** | 0001 |
| `README.md:64-71` (repo root) | still presents `../spec` as available; rewrite — the tree is lost, the ids are a grep index with no authority, decisions live in `docs/adr/` | 0008 |
| eight in-source `spec/gap-analysis/…` paths | repoint (`12-cyrup-tui.md` → `07-cyrup-tui.md`, `03-cyrup-agent.md` → `02-cyrup-agent.md`); where the target was never rescued, strike the path and state inline what the reader needs | 0008 |
| `crates/cyrup-tui` — 3 of 20 `ADR-0001` sites | `transcript.rs:59` (that is `TUI-N06`, not an ADR grant), `app.rs:1281` (keep the retraction, repoint at `docs/adr/`), `theme.rs:1492` (mechanism note, **not** a closure). The other 17 need only the path | 0001 |
| `stray_reply.rs:4` | cites `tui.ts:788-794` for pi's OSC-11 guard — the guard at neither tag (v0.83.0 `:765-771`; v0.84.1 `:819-825`, method renamed `handleTerminalInput`) | 0001 |
| `crates/cyrup-ext/Cargo.toml:70` | `R-ARCH-EXT-010` resolves to nothing; replace with a pointer to `docs/adr/ADR-0002-extension-io-is-serde.md` or delete | 0002 |
| 26 `model-registry.ts:NNN` citations across 9 crates | every one is **out of range** (file is 145 lines @v0.83.0); the port was written against pre-v0.80.8 pi. Correct counterpart is now `model-runtime.ts` | LEADS |
| `00-residual-ledger.md:246`, `:288`, `:290`, `:590`; `gap-analysis/README.md:120` | the alt-screen family row is "decided by ADR-0005"; the "two leads" language retires | 0005, LEADS |
| `theme.rs:1011-1017`, `tests/theme_fidelity.rs:835` | accurate until 30b, but should cite ADR-0005 rather than read as an open question | 0005 |
| `main.rs:215-217` | three false claims in three lines (predicate "faithfully false"; the wizard "is the ext-UI dialog host"; `Pi main.ts:557` for a gate at `:615`) — delete and replace | 0011 |
| `crates/cyrup-intercom/src/paths.rs:6-8`, `broker/mod.rs:1253-1254` | in-source Windows deferrals **superseded** by ADR-0007 | 0007 |

### New work these decisions imply that no ledger row covers

Each needs an id assigned by the owning area file. **None is optional** — under the parity rule a
mechanism difference that costs behaviour stays as work.

| what | suggested rating | area / batch | ADR |
|---|---|---|---|
| **Silent encode/decode degradation** — `serde_json::to_string(…).unwrap_or_else(|_| "[]")` at `host/live.rs:1472/:1490/:1494/:1551/:1562-1563` (+`:744`/`:748`/`:765`) makes a failed encode indistinguishable from an empty payload; `decode_outcome`'s unparseable arm (`:1645-1651`) makes a malformed patch indistinguishable from a decline. pi structurally cannot have this failure mode | medium · S · cyrup-original | 06 / batch 20 | 0002 |
| **Cloudflare `resolveValue` ambient fallback dropped** — pi returns `fromCredential ?? await ctx.env(name)` (`cloudflare-auth.ts:18-23`); cyrup returns `None` (`providers/cloudflare.rs:48-65`), so a key-only credential **permanently shadows** `CLOUDFLARE_ACCOUNT_ID` and the only exit is `/logout`. Fix lands in batch 11 regardless of where it is filed | confirmed parity bug | 01 / batch 11 | 0010 |
| **`cyrup-sdk` re-exports nothing from `sdk.ts:114-126`** (`withFileMutationQueue` + nine tool factories) while `lib.rs:7` promises embedders need no internal-crate dependency — a self-contradiction at HEAD. Free to fix: `cyrup-session-svc` already pulls `cyrup-tools` | low · S | 08 | 0008 |
| **`cyrup_sdk::Session` surfaces neither `modelFallbackMessage` nor the extensions result** from `CreateAgentSessionResult`; both reachable one hop down — ergonomics, not capability | low · S | 08 | 0008 |
| **The citation lint + allow-list + the §A.5 triage of 26 `ADR-000N` sites + the stale-path repairs** as one mechanical unit | low · M (98 % mechanical) | 05 / cross-cutting | 0008 |
| **Transcript retention (B-1)** — `drain_committed` (`transcript.rs:505-511`) takes committed entries out of memory; a scrollable fullscreen document requires retaining them. cyrup-only machinery, no upstream counterpart, so **no drift sweep would ever produce it**. Largest structural change in 30b; contends with batches 16 and 30a in `transcript.rs`/`app.rs` | part of `TUI-019` | 07 / batch 30b | 0005 |
| **The mouse enable sequence (B-4)** — three crossterm-vs-pi deltas; the implementer must `queue!` literal escapes, not take `EnableMouseCapture`, or silently lose multiplexer performance and focus reporting inside an L+ item | part of `TUI-019` | 07 / batch 30b | 0005 |
| **~8 further `TUI-019` decompositions** — B-5, B-7, B-8, B-11, B-12, B-14 each deserve an id; `TUI-019` alone under-counts 30b | — | 07 | 0005 |
| **A new area file `13-windows-platform.md` + its own late batch** — sweep the ~106 `cfg(unix)` `src/` sites with **no** non-unix arm (only 26 of 132 have one). Six concrete holes already found: clipboard no-op vs pi's `clip`; subagent `send_sigint`/`sigterm`/`sigkill` no-ops so **termination kills nothing** (pi's `taskkill /F /T /PID`, which cyrup **already ports for the bash tool**); `drain.rs` returning 0; `terminal_query.rs` returning `None`; `ctrl+z`→Suspend bound where pi binds nothing on win32; **both** `ctrl+v` and `alt+v` bound where pi binds one per platform (a divergence on **every** platform, not just Windows) | unknown until swept | 13 | 0007 |
| **A small Windows bring-up batch** — `PB-19`'s listen half + the two cross-target check gates + the `rust-toolchain.toml` target declaration; acceptance is a green workspace `check` for both Windows targets | S–M | new batch | 0007 |
| **A real Windows CI runner** — neither pi nor cyrup has one; cyrup has no CI at all (no `.github/` at HEAD) | — | eventually | 0007 |
| **`onThemePreview` live recolour unported** — `ListSelector::prompt` constructs `preview: false` and `run_startup_selector` no-ops `SelectorOutcome::Preview`; `startup.rs:250-255` admits it in-tree and no id cites it | low · S — new `UW`-family row | beside `SEAM-067` | 0011 |
| **Migrate `TUI-FIDELITY.md` §8's 15 killed claims into the README traps list** *before* the archive goes non-normative — a non-normative document cannot discharge an anti-re-filing function. Load-bearing: F22, F90, "one dark palette" (false), "hardcodes colours" (false), F40's inverted direction, S3/S28's generality, and above all **S24**, which read a guard backwards, **deleted a working feature** and inverted two tests into asserting its absence | documentation | batch 2 | 0009 |
| **Read `harness/session/testing/conformance.ts` once** and extract only the assertions that hold for the coding-agent JSONL format cyrup ships — expect a handful of test items, not 993 lines. **If the honest yield is zero, record that explicitly rather than leaving silence** | small | 03 | 0004 |
| **A standing sweep instruction** — when a sweep excludes a `packages/tui/src` file it must record the rule-2 draws-test result with the exclusion. An unstated exclusion is how 863 lines and nine items stayed invisible | process | every sweep | 0001 |
| **A one-line note in areas 03 and 12** that `packages/agent/src/harness/…` is the wrong upstream side for any cyrup session/tool defect | process | 03, 12 | 0004 |
| **cyrup has no published environment-variable contract** where pi does (`packages/coding-agent/docs/environment-variables.md`) — cyrup reads `CYRUP_SHELL`, `CYRUP_SUBAGENT_BINARY`, `CYRUP_INTERCOM_BROKER_BINARY`, `CYRUP_TELEMETRY` and the `CYRUP_*`/`PI_*` session family, and no item owns it. Whether pi's user-facing `docs/` tree is in scope at all is an **unfiled question** adjacent to OQ-6/OQ-7 — flagged, not pitched | unfiled | — | 0003 |

### Structural moves

- **Batch 30 splits** into **30a** (TUI presentation, 21 items, L — deps 3, 6, 16; its batch-2
  dependency is discharged) and **30b** (the fullscreen renderer, `TUI-019`, L+ — deps 14, 16, 30a;
  30a and 30b both rewrite `app.rs`/`transcript.rs` and **must not run concurrently**). The plan goes
  to 31 batches. *(ADR-0005)*
- **The post-batch-26 re-baseline batch is deleted** and replaced by an event-triggered procedure.
  *(ADR-0006)*
- **Batch 18 loses its harness-measurement task** (discharged in full) and keeps everything else.
  *(ADR-0004, ADR-0006)*
- **`PB-19`'s listen half splits out of batch 24** and moves forward as the compile gate's
  prerequisite. *(ADR-0007)*
- **`cyrup/TUI-FIDELITY.md` → `docs/audits/2026-08-09-tui-presentation-fidelity.md`**, stamped
  EXECUTED AND CLOSED / non-normative. `docs/audits/` is created. The 15 in-source citations name
  sections, not paths, so the move breaks nothing. *(ADR-0009)*
- **Batch 3 gains six deliverables from five ADRs** — see the consolidated table above.
