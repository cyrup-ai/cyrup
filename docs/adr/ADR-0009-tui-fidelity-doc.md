# ADR-0009 — `TUI-FIDELITY.md` is an executed audit, not a backlog: archive it, merge nothing, and gate actionability on the ledger

**Status** accepted (decided by default under the parity rule — overridable)
**Date** 2026-08-13
**Decides** OQ-7 (`docs/PARITY-PLAN.md` §7) = `OQ-07-2` (`docs/gap-analysis/07-cyrup-tui.md` §Open questions, `docs/gap-analysis/00-residual-ledger.md:396`)
**Blocks released** Batch 30 (its scope no longer depends on this question); `docs/PARITY-PLAN.md`'s coverage claim; area 07's coverage blind spot 1; `TUI-016`'s fix shape; any future "the TUI is at parity" statement

---

## Context

### The question as posed rests on a false premise

OQ-7 asks whether `cyrup/TUI-FIDELITY.md` — 106,185 bytes, 464 lines — is "merged with real IDs" or
"formally retired", and tells the decider to "expect the medium/low counts to rise materially". Every
statement of the question in the tree describes the document as a live backlog of open work:

- `docs/PARITY-PLAN.md:1384` — "~150 presentation **divergences** … invisible to the ledger and to
  this plan's coverage claim".
- `docs/gap-analysis/07-cyrup-tui.md:46` — "Merging that backlog into this file with real IDs remains
  the highest-value follow-up for this area."
- `docs/gap-analysis/07-cyrup-tui.md:1114-1118` — "merging it would add on the order of 150 rows to a
  426-item ledger, most of them low."
- `docs/gap-analysis/00-residual-ledger.md:396-400` — the same framing again.

**It is not a backlog. It is a work order that was executed in full, and none of the four documents
that ask the question checked.** Area 07 admits the gap in its own coverage section
(`07-cyrup-tui.md:1078`): *"this pass did not re-audit which of its rows batches 4–10 actually
closed."* That re-audit is what this ADR performed.

### What the document actually is

464 lines carrying **117 ID-bearing table rows** (T1–T9, L1–L7, X1–X18, C1–C15, S1–S36, E1–E15,
M1–M17 — counted mechanically), tagged against **152 raw audit finding ids** `F1`…`F152` plus five
`missed#N`. It also carries four non-row sections that are not findings at all: §6 (19 "missing
element" rows, most of which duplicate a table row), §7/§7a (a **ten-batch execution schedule**), §8
(**15 killed claims** — hypotheses disproved with evidence), and a closing §Confidence notes listing
four genuinely unaudited pairs. "~150 findings" conflates the 117 rows with the 152 `F` tags.

### It was executed — ten commits, matching §7's ten batches, one per batch

`git log --oneline --all | grep 'fix(tui): batch'` at cyrup HEAD `72cd292`:

```
0aaca00 fix(tui): batch 1 — colour accessors (T1-T9), the first presentation-fidelity batch
a090e81 fix(tui): batch 2 — footer, status band, loaders and hints (C1-C15)
d6d31cc fix(tui): batch 3 — SYS-4 selectedBg inversion + SelectList geometry
99e9908 fix(tui): batch 4 — dialog envelopes, and pi's real layout contract
bc7a538 fix(tui): batch 5 — transcript vertical rhythm (SYS-3 transcript half)
677d594 fix(tui): batch 6 — markdown rendering (M1-M4, M6, M8, M11, M14, M16, M17)
b4bcc06 fix(tui): batch 7 — editor and input dialogs (E1-E4, E8-E15)
b54dd75 fix(tui): batch 8 — SYS-2, the wrap rewrite
8834df0 fix(tui): batch 9 — per-selector completeness (26 items across 9 dialogs)
922d90c fix(tui): batch 10 + closeout — the TUI fidelity backlog, actually finished
```

A commit message is a hypothesis. **The evidence is the source at HEAD**, read two-sided below.

### Sampled verification — 46 of 117 rows (39%), read at HEAD against pi v0.84.1

Two disjoint samples were taken. Sample A is mechanical and unbiased: **every fifth row in document
order** (positions 1, 6, 11 … 116) = 24 rows. Sample B is **adversarial** — 22 further rows chosen
*because* their ID appears nowhere in `crates/cyrup-tui/src/`, i.e. selected to maximise the chance of
finding unfinished work.

| | rows | acted on at HEAD | still open |
|---|---|---|---|
| **A** (every 5th row) | T1, T6, L2, L7, X5, X10, X15, C4, C9, C14, S5, S10, S15, S20, S25, S30, S35, E4, E9, E14, M2, M7, M12, M17 — 24 | **24** | **0** |
| **B** (adversarial: no ID citation in source) | T2, T4, C2, C5, C6, C7, C10, C12, C15, L1, S1, S4, S6, S34, E1, M1, M3, M4, M6, M8, M11, M16 — 22 | **22** | **0** |
| **total** | **46 / 117 (39%)** | **46** | **0** |

Representative two-sided reads (cyrup at HEAD `72cd292`, pi at `v0.84.1`):

| row | the document's "cyrup" column | **cyrup at HEAD** | pi @ v0.84.1 |
|---|---|---|---|
| **T1** | `theme.rs:292-299` `add_modifier(Modifier::DIM)` over the `text` role | `theme.rs:339-341` `role_style("dim", "#666666", "#767676")`; `:332` names T1 and says "this **used to** resolve the `text` role" | `theme.ts:372-376` `fg()`, colour-only |
| **T2** | `theme.rs:35-49` reads two env vars, no terminal-program table | `theme.rs:59-67` `detect_from` delegates to `image::detect_capabilities_from` — the port of `detectCapabilities` | `terminal-image.ts:68-135` |
| **T4** | `error_style()` bakes in `Modifier::BOLD` | `theme.rs:325-327` `Style::default().fg(self.error…)` — no bold | `theme.ts:372-376` colour-only |
| **L1** | `finalize_block` pushes no `paddingY` rows | `transcript.rs:1551` `padding_y`, `:1571-1579` `apply_bg(Line::default(), …)` top and bottom | `box.ts:106-119` |
| **L2 / SYS-2** | wrapping happens after padding, at full frame width | `markdown.rs:705-745` `flush_line` wraps to `content_width()` **then** re-prefixes every produced row, with `continuation_prefix()` | `markdown.ts:322` → `:329-340` |
| **E1** | `editor.rs:42 const PROMPT: &str = "› "` | **no `PROMPT` const exists**; `editor.rs:1966` is a comment on its removal | `editor.ts:482-601` — no prompt glyph |
| **S1 / SYS-4** | `select_list.rs` paints `selected_bg_style()` on every selected row | `grep selected_bg_style select_list.rs` → **nothing** | `theme.ts:1293-1294` foreground only |
| **M3** | `markdown.rs:217-220` `"  ".repeat(depth)` | `markdown.rs:673` `"    ".repeat(depth)` | `markdown.ts:758` four spaces |
| **M12** | "`grep -rin latex crates/cyrup-tui/src/` → **zero**" | `crates/cyrup-tui/src/markdown/latex.rs`, **2242 lines**; `latex_prepass` called from `render_inner` | `packages/tui/src/latex.ts` |
| **S15** | "`body_lines` returns after the row loop; no such format exists" | `session_selector.rs:530-532` emits `format!("  ({}/{})", …)` | `session-selector.ts:512-516` |
| **X15** | "`renderer failed` appears nowhere in `crates/cyrup-tui/src/`" | `transcript.rs:2814` `format!("[{label}] renderer failed: {message}")` | `custom-entry.ts:47-52` |

77 of the 117 IDs are cited **by name** inside `crates/cyrup-tui/src/`, and the file is named as an
authority 15 times across seven source files (`theme.rs` ×9, `transcript.rs` ×2, `editor.rs`,
`extension_editor.rs`, `markdown.rs`, `select_list.rs`) in the form `T1 (TUI-FIDELITY §2)`. Sample B
demonstrates that the absence of a citation does **not** imply the finding is open — all 22 uncited
rows were landed too.

**Confidence.** 0 of 46 open. By the rule of three the 95% upper bound on the open rate is 3/46 ≈
6.5%, i.e. **at most ~8 of the 117 rows could still be open, with a point estimate of 0.** This is a
source read, not a render check: it establishes that every sampled finding was *acted on* and that
the document's "cyrup" column is now false. It does **not** establish that the TUI renders correctly
— that still requires the live-terminal runs batch 30 already schedules.

### The one row that cost behaviour, read two-sided

**C14** (`{n} queued` footer segment) is the row that produced `TUI-016`, and it is the only sampled
row whose application was harmful. The finding itself was **correct**:

- pi @ v0.84.1 `packages/coding-agent/src/modes/interactive/components/footer.ts:129-165` builds
  `statsParts` as `↑ ↓ R W CH% $cost`, the context segment and `xp`. **There is no queue segment.**
- cyrup at HEAD: `status.rs:76-81` retains the `queued` field with a doc comment reciting C14's
  reasoning, `:149-150` retains `set_queued`, and there is **no render site**.

What C14 did not say is where pi *does* draw the queue, and nobody looked:

- pi `interactive-mode.ts:4190-4207` `updatePendingMessagesDisplay()` clears
  `pendingMessagesContainer`, then for a non-empty queue adds a `Spacer(1)`, one
  `TruncatedText(theme.fg("dim", \`Steering: ${message}\`), 1, 0)` per steering message, the same for
  `Follow-up: `, and a final dim `↳ ${dequeueHint} to edit all queued messages`
  (`app.message.dequeue`, also hinted at `:932`).
- cyrup at HEAD `app.rs:4612-4614`: `AgentSessionEvent::QueueUpdate { steering, follow_up } =>
  self.state.status.set_queued(steering.len().saturating_add(follow_up.len()))` — the **texts are
  discarded** and the count is written to dead state.

So the deletion shipped alone and the replacement never did. That is not a flaw in the finding; it is
a flaw in the *form* of the finding — a delete-only instruction with no paired add, in a document
with no row for the paired add to live in.

### What is genuinely open in the file

Not the 117 rows. Its §Confidence notes list four pairs nobody finished reading:
`startup.rs`/`startup_selector.rs` vs `interactive-mode.ts:1480-1690 showLoadedResources`
("**unaudited**, not clean"), `login_dialog.rs` ↔ `login-dialog.ts` ("**unaudited**"), and two
questions needing a live terminal. Area 07 already carries both as coverage, not as items:
`07-cyrup-tui.md:1079` names `login_dialog.rs` among fourteen selectors "**not** compared against
their upstream components for behaviour", and `TUI-018`/`TUI-N02` hold the startup-panel behaviour.

---

## Decision

**Retire `TUI-FIDELITY.md` from the actionable path. Merge none of its 117 rows into area 07. Do not
delete it. Do not leave it where it is.** Concretely, and in this order:

1. **Archive it, stamped.** Move `cyrup/TUI-FIDELITY.md` → `docs/audits/2026-08-09-tui-presentation-fidelity.md`,
   preserving its section numbering (`§2`, `§3D`, `§5`, `§8` — the 15 in-source citations name
   sections, not paths, so the move breaks nothing). Prepend, verbatim:

   > **Status: EXECUTED AND CLOSED — non-normative. Evidence only.**
   > All ten batches of §7 were applied (`0aaca00` … `922d90c`). A 46-of-117 sample re-read at cyrup
   > `72cd292` against pi `v0.84.1` found 46 of 46 rows landed and 0 open (ADR-0009).
   > **The "cyrup" column of every table below describes a tree that no longer exists.** No work may
   > be scheduled from this file and no commit may cite it as authority for a change. It is retained
   > as the two-sided evidence record for the changes already in `crates/cyrup-tui/`, which cite it by
   > section in 15 places, and for §8's killed claims.

2. **Change no count.** Area 07 stays at **56 open (3 critical, 1 high, 26 medium, 26 low)**; the
   ledger stays at **448 raw IDs / ~420 distinct**. The medium and low counts **do not rise** — the
   premise that they would is the false one this ADR overturns. Strike the "add on the order of 150
   rows" sentence at `07-cyrup-tui.md:1116-1117` and `00-residual-ledger.md:396-400`.

3. **Correct `TUI-016`'s fix shape** — the one item the document damaged. Its fix is **not** "restore
   the footer segment"; pi has none (`footer.ts:129-165`, verified). Its fix is: stop discarding the
   texts at `app.rs:4612-4614`, and port `updatePendingMessagesDisplay`
   (`interactive-mode.ts:4190-4207`) as a region above the editor — `Spacer(1)`, one dim
   `Steering: {msg}` / `Follow-up: {msg}` row per queued message inset one column, then a dim
   `↳ {app.message.dequeue} to edit all queued messages`. Severity, kind and effort are unchanged
   (**medium · parity-bug · M**); what changes is that the row now names its upstream surface. Delete
   the dead `queued` field and `set_queued` (`status.rs:76-81`, `:149-150`) in the same change so no
   future reader mistakes dead state for a rendered one.

4. **Rescue §8 before the archive goes non-normative.** Its 15 killed claims exist for exactly one
   purpose — to stop the next pass re-filing a disproved hypothesis — and a non-normative document
   cannot discharge that. Migrate them into the README's known-traps list as trap rows, with their
   evidence: F22 (unreachable hex fallbacks), F90 (unreachable empty-state string), "cyrup ships one
   dark palette" (false), "cyrup hardcodes colours" (false), F134's scope, F40's inverted effect
   direction, S3/S28's per-component (not `SelectList`) generality, and above all **S24** — a row
   that read a `!showsFoldInConnector` guard backwards, *deleted a working feature*, and inverted two
   tests into asserting its absence. That one already cost behaviour a second time and its lesson
   must outlive the archive.

5. **Answer area 07's coverage blind spot 1.** Replace `07-cyrup-tui.md:1078` ("this pass did not
   re-audit which of its rows batches 4–10 actually closed") with this ADR's sampled result, and
   carry forward the archive's two genuinely unaudited pairs — `startup.rs`/`startup_selector.rs` vs
   `showLoadedResources`, and `login_dialog.rs` ↔ `login-dialog.ts` — as named entries in that same
   coverage section, folded into the existing unswept-selector list at `:1079`. **File no new IDs for
   them.** "Nobody has looked" is a coverage statement; it becomes items when the sweep runs and finds
   something.

6. **Batch 30 keeps its scope unchanged.** OQ-7 adds nothing to it. Add one line to its *Verified by*:
   the live-terminal runs must confirm the ten fidelity batches actually render as claimed, because a
   source read is not a render check — and record the result as the archive's closeout, not as new
   items.

### The rule: how a finding becomes actionable in this project

A finding is actionable **only** when all five hold, in `docs/gap-analysis/`:

1. a **stable ID** in an area file's `## Open items` table;
2. a **severity**;
3. a **kind** (`parity-bug` / `not-ported` / `upstream-drift` / `stale-port` / `cyrup-original` / `test-defect`);
4. a **status row** in that file's status table; and
5. a **batch** in `docs/PARITY-PLAN.md`.

Everything else — audit archives, `F`-tags, in-source comments, commit messages, this ADR's own prose
— is **evidence**. Evidence justifies an item; it never substitutes for one. **No commit may cite a
document outside `docs/gap-analysis/` and `docs/adr/` as its authority for a behaviour change.** A
finding that is worth acting on is worth an ID; if it does not earn an ID it does not get executed.

Three corollaries, each of which would have prevented a failure that actually happened:

- **The C14 corollary — a deletion is half a finding.** A row whose fix is "delete X" is not
  actionable until the same row names the upstream surface that replaces X. pi does not draw *nothing*
  where cyrup draws something; it draws something else, somewhere else. One item, both halves, filed
  together and landed together — or not filed.
- **The staleness corollary — an executed findings document must be stamped at the commit that
  executed it, or archived.** Its "current state" column becomes false the moment it is acted on, and
  an unstamped one is a revert waiting to happen: read literally today, this document instructs a
  contributor to delete a `PROMPT` const that no longer exists and to re-add `Modifier::DIM` reasoning
  to a `dim_style()` that has already been fixed.
- **The S24 corollary — a disproved claim needs a home in the ledger too.** Killed claims are load
  bearing. If they live only in a document that goes non-normative, the next pass re-files them.

---

## Consequences

**Ledger — counts.** No change **from this decision**; the 448 raw IDs / ~420 distinct / 9 trackers
figure stands against OQ-7 specifically, which is the claim being tested here (the premise was that
it would rise by ~150). Other decisions in this same batch do move it — ADR-0004 closes four
trackers, ADR-0005 expects ~8 new area-07 rows when `TUI-019` is decomposed, ADR-0007 opens a
thirteenth area file, ADR-0008 and ADR-0010 add items — and the aggregate is tabulated in
`docs/adr/README.md`. Nothing here licenses quoting "448" as a post-batch figure. Area 07 stays 56 open
(3 · 1 · 26 · 26). The `00-residual-ledger.md` severity table (`:66-68`) and the area table (`:654`,
`:660`) are untouched by this decision. Any future statement that the ledger "excludes ~150
presentation findings" is now false and must be struck wherever it appears
(`PARITY-PLAN.md:245`, `:1199`, `:1384`, `:1468-1474`; `07-cyrup-tui.md:46`, `:1078`, `:1114-1123`;
`00-residual-ledger.md:396-400`).

**Ledger — items.**

| ID | change |
|---|---|
| `TUI-016` | **Scope corrected, severity/kind/effort unchanged (medium · parity-bug · M).** The fix is `updatePendingMessagesDisplay` (`interactive-mode.ts:4190-4207`), **not** a footer segment — pi has no footer queue segment (`footer.ts:129-165`). Add: stop discarding the texts (`app.rs:4612-4614`); delete the dead `queued` field/setter (`status.rs:76-81`, `:149-150`). Cross-reference the archive as the origin of the regression. |
| `TUI-002` | **Evidence pointer only.** Its markdown half was closed by archive row X5; the note at `07-cyrup-tui.md:53` already records this. Repoint the citation at the archived path. No severity, kind or scope change. |
| `TUI-034` | **Evidence pointer only.** Archive rows M13 (transformer chain / mermaid) and M12 (LaTeX) are the same finding and its landed sibling; `07-cyrup-tui.md:481` already cites `markdown/latex.rs` as the precedent. No change. |
| `TUI-026`, `TUI-023`, `TUI-024`, `TUI-010` | **Closure evidence repointed** to the archived path. All four were closed by fidelity rows (X1, C8, C1, X14 respectively); the closures stand. |
| `TUI-031` | Unchanged, but its "no surface would show a queued message" note (`00-residual-ledger.md:118`) now resolves through `TUI-016`'s corrected fix. |
| **no new IDs** | The 117 rows produce **zero** new open items. The two unaudited pairs become coverage entries, not items. |

**Plans.** `PARITY-PLAN.md` §7's OQ-7 entry is replaced by a pointer to this ADR. Batch 30's items
list is unchanged **by this decision** — OQ-7 adds nothing to it and removes nothing from it
(TUI-002, -004, -010, -012, -015, -017, -020, -025, -032, -036, -038, -041, N01–N03,
N06–N09, DRIFT-041, CFG-021, TUI-019). **The batch's *shape* is changed by another decision, and that
one governs:** `docs/adr/ADR-0005-alt-screen-tui-mode.md` answers OQ-3 "port it", which splits batch
30 into **30a** (the 21 presentation items above, effort L) and **30b** (`TUI-019`, now
**unconditional**, effort L+, decomposed into fourteen work units). The trailing entry in the list
above is therefore `TUI-019`, not "`TUI-019`-after-OQ-3": the contingency it named is discharged, and
it lives in 30b. The two decisions are independent and compatible — OQ-7 moves no count, OQ-3 moves
no membership. Its *Why here* paragraph must drop the claim that
grouping the presentation tail "forces OQ-7" — it does not, because there is no presentation tail
left to schedule. Its *Verified by* gains the render-confirmation line from decision item 6.

**Repo layout.** `cyrup/TUI-FIDELITY.md` ceases to exist at the repo root; `docs/audits/` is created.
The 15 in-source citations continue to resolve — they name sections, not paths — but a one-line
pointer at the top of `docs/gap-analysis/07-cyrup-tui.md` should give the new path so a reader
following `TUI-FIDELITY §3D` from `select_list.rs:251` can find it.

**Work this creates that no item covers.** One: migrating §8's 15 killed claims into the README traps
list (decision item 4). It is documentation work with no code change and belongs in batch 2 alongside
the other decision-of-record repairs, not in batch 30.

---

## Rejected alternatives

**1. Merge all 117 rows into area 07 with real IDs** — the option the question expected, and the one
that does real damage. It would book ~115 **already-implemented** rows as open work, taking area 07
from 56 to ~171 and the ledger from 448 to ~563, and every one of those rows carries a "cyrup" column
that describes a tree deleted between `0aaca00` and `922d90c`. Batch 30 would then be handed
instructions to remove a `PROMPT` const that does not exist (`editor.rs:42` — gone), to stop
`select_list.rs` painting `selected_bg_style()` (already gone), to change `"  ".repeat(depth)` to four
spaces (already four, `markdown.rs:673`) and to port a LaTeX renderer that is already 2242 lines in
the tree. The realistic outcome of scheduling stale rows as open work is not wasted effort — it is a
**revert**, because the fastest way to "close" a row whose evidence no longer matches is to make the
code match the evidence. Merging only the *residue* is the same option minus the pretence: on the
measured evidence the residue is empty, and the two rows that do connect to open work (`TUI-016`,
`TUI-002`) already exist.

**2. Keep it as-is at the repo root** — the option that has already caused one regression, and the one
the assignment requires be justified rather than assumed. It cannot be justified. The recurrence
mechanism is unchanged and now *worse*: the document reads as a live instruction sheet, its
prescriptions have all been carried out, and its "cyrup" column is false in 46 of 46 sampled rows. The
next contributor who opens it and follows §4's 84-item quick-win checklist — which is written as
imperatives, not as findings — will undo batches 1–10. C14 is the proof of concept for exactly that
failure at scale of one.

**3. Delete the file outright** — the literal reading of "formally retire it as non-normative and
delete it" in the plan's option list. Cost: it orphans the 15 in-source citations that name it as
their authority (`theme.rs:332` "T1 (TUI-FIDELITY §2): this used to resolve the `text` role…" becomes
unresolvable), it destroys the only two-sided evidence record for ~117 landed behaviour changes —
which under the project's "a commit message is not evidence" rule means those changes become
unverifiable without redoing the entire audit — and it destroys §8, whose whole function is to stop
re-filing disproved claims. §8's S24 entry documents a case where misreading one guard deleted a
working feature and inverted two tests; deleting that lesson is how it happens a third time. Archiving
costs one `git mv` and preserves all of it.

**4. Convert it into a "closed items" appendix inside area 07** — 117 closed rows appended to a file
that is already 1,126 lines, to satisfy the README's rule that the status table covers every item from
every pass. Cost: it doubles the length of the area file the parity effort reads most often, in
service of rows that will never be worked again, and it invites exactly the confusion this ADR exists
to end — closed rows and open rows in one table, distinguished only by a status word. The archive with
a status stamp gives the same auditability at the cost of one hyperlink.

---

## How to reverse this

**"TUI-FIDELITY's rows are open work — file them in area 07 and schedule them."**

For that to be right, the sampled result would have to be wrong: someone must re-read rows at HEAD and
show that a material fraction are still divergent from pi v0.84.1 — starting with the 71 rows this ADR
did not sample. Two disjoint samples totalling 46 of 117, one of them adversarially selected against
the conclusion, returned 46 landed and 0 open; overturning that needs counter-evidence at file:line on
both sides, not a re-reading of the document. If such rows are found, they are filed **individually
with fresh IDs, fresh two-sided evidence read at HEAD, and a batch** — never by bulk-importing the
archive's tables, whose cyrup column is stale by construction. The archive's non-normative status and
the five-condition actionability rule survive that reversal unchanged; only the count moves.

A second, narrower reversal: **"keep the file at the repo root."** That would require a reason the
delete-only-instruction failure (C14) and the stale-instruction-sheet failure cannot recur — and the
only such reason is a status column on every row, which is the merge option under a different name.
