# 07 — cyrup-tui

This area covers `cyrup/crates/cyrup-tui` (the interactive chat UI: transcript, editor, footer, selectors, themes, images, keymap, autocomplete, startup panel, terminal negotiation) plus the TUI wiring in `cyrup/crates/cyrup/src/main.rs`. It is measured against `pi/packages/tui/` (rendering primitives and terminal control) and `pi/packages/coding-agent/src/modes/interactive/` (components and `interactive-mode.ts`).

> **Re-audited 2026-08-12, cyrup HEAD `04c1ba2` (last code commit; working tree clean at `a9000b1`, docs-only), against pi `v0.84.1`.**
>
> The version-lag items in this file were measured against pi **`v0.84.1`**, with pi **`v0.83.0`** — the tag cyrup was ported from — read directly wherever a finding had to be classified `not-ported` (absent at the baseline too) versus `upstream-drift` (landed in the `v0.83.0..v0.84.1` window: 627 files, +52291/−17556). Every `closed` verdict below was reached by reading the Rust at HEAD **and** the TypeScript at the tag; no closure rests on a commit message.
>
> **This pass: 13 items closed · 2 auditor closures overturned · 1 closure re-framed · 15 items newly filed · then a repair pass adding 10 more · 56 items now open (3 critical, 1 high, 26 medium, 26 low).**
>
> ### Repair pass 2026-08-12 (post-critique)
>
> Applied after the completeness critique of the twelve finished area files. Four changes; no item was
> renumbered, merged or deleted.
>
> 1. **TUI-027 raised `high` → `critical`** (critique finding 3). Re-verified both ends at HEAD before
>    raising: the `/tree` inline label editor's confirm arm returns
>    `SelectorOutcome::Apply(format!("{entry_id}{FIELD_SEP}{label}"))` (`tree_selector.rs:540-546`),
>    and the chrome persists that payload through the **live** session path —
>    `app/execute.rs:295-298` `session.services().host_services.set_label(&entry_id, (!label.is_empty()).then_some(label.as_str()))` →
>    `manager.append_label`, the same path a loaded extension's `setLabel` uses (`app/execute.rs:288-298`).
>    So the characters a pi user types expecting a text search are written into the session JSONL as
>    that entry's label. That is corruption of persisted user data, which README:106-107 classes as
>    critical without qualification.
> 2. **TUI-019 re-rated `low` → `medium`, and its ADR-0001 justification struck** (critique finding
>    13). See the item and `## Open questions — decisions required`.
> 3. **Status rows added for TUI-027…TUI-041** (critique finding 17b), replacing the collapsed
>    `new this pass` row that hid fifteen items from a table the README requires to cover every item
>    from every pass.
> 4. **A `packages/tui/src` input-pipeline sweep was run and absorbed as TUI-042…TUI-051** — nine
>    findings off six upstream files (`stdin-buffer.ts`, `editor-component.ts`, `terminal-colors.ts`,
>    `undo-stack.ts`, `word-navigation.ts`, `fuzzy.ts`) that **no file in this directory had ever
>    named**, plus one `/reload` finding routed here from the migrations sweep. Two are critical and
>    both are silent data loss in the prompt editor. This is precisely README structural blind spot 2
>    — `stdin-buffer.ts` is 434 lines that draw nothing, and blind spot 2 exists to catch exactly that
>    class. See `## Coverage` §Surface-driven sweep.
>
> **Baseline shift this file did not previously reflect.** The prior revision was written against cyrup `1806375`/`9219dcd`. Since then `crates/cyrup-tui` took 103 files / +38,826 / −2,362 across ten `fix(tui): batch N` commits (`0aaca00` … `922d90c`) plus `023c778` and `eed28c9`. Fifteen files in `crates/cyrup-tui/src/` did not exist at the old baseline: `panic_hook.rs`, `drain.rs`, `terminal_progress.rs`, `terminal_title.rs`, `tmux.rs`, `keyboard_protocol.rs`, `footer_data.rs`, `resume_hint.rs`, `stray_reply.rs`, `oauth_selector.rs`, `login_dialog.rs`, `user_message_selector.rs`, `native_modifiers.rs`, `markdown/latex.rs`, `component.rs`. Nine of the eleven `-S` surface-sweep items and four main-table items closed as a direct result.
>
> **Closed this pass** — TUI-022, TUI-023, TUI-024, TUI-026, TUI-S01 (as framed), TUI-S03, TUI-S04, TUI-S05, TUI-S06, TUI-S07, TUI-S08, TUI-S09, TUI-S11.
>
> **Auditor closures overturned** — **TUI-020** (`closed` → partially-closed): cyrup emits OSC-8 *nowhere*; `markdown.rs:133-141` documents the omission in its own words, and `image_fallback_text` still pushes a bare filename. **TUI-S02** (`closed` → partially-closed): the `uncaughtCrash` half landed as `panic_hook.rs`, but pi's second mechanism — `DEAD_TERMINAL_ERROR_CODES` → `emergencyTerminalExit` — has no counterpart. **TUI-S01** was re-framed rather than overturned: its sink now exists, so it is closed as framed and carried only as a pointer to TUI-014 and TUI-033, which own the three variants that remain unrendered.
>
> **Newly filed** — TUI-027 … TUI-041 in the main pass, TUI-042 … TUI-051 in the repair pass. Three are `critical`: TUI-027 (`/tree` has no search, its action keys are the characters pi types into that search, and the resulting label edit is **persisted to the session JSONL**), TUI-042 (the undo snapshot omits the paste registry, so undoing a delete over a paste marker silently sends the literal marker text to the model instead of the pasted content) and TUI-043 (Ctrl+W after a large paste orphans the marker and drops the paste). One is `high`: TUI-031 (a prompt typed during compaction is dispatched immediately, against a context the compaction is mid-rewrite of).
>
> ### Repair batch 5 — editor paste / undo / word motion, applied 2026-08-13
>
> **`TUI-042`, `TUI-043`, `TUI-044`, `TUI-049` and `TUI-053` are FIXED; `TUI-048` is partially fixed
> and stays open.** All five fixed items were reproduced live on 2026-08-13 and each now has a test
> that was RED at HEAD `85bc8bd` and is GREEN after (the RED outputs are quoted in the items). The
> work is confined to `crates/cyrup-tui/src/{editor.rs,keymap.rs}` and `tests/editor.rs`.
>
> Three corrections to the items as filed, all from reading pi at the tag rather than from the code:
>
> 1. **TUI-043's Fix asked for the wrong mechanism twice.** `deleteWordBackwards`/`deleteWordForward`
>    do **not** drop the registry entry upstream (`editor.ts:1607-1672`), and upstream has no
>    `marker_covering`-style predicate at all — atomicity is a property of the marker-merging
>    SEGMENTER that motion, deletion and wrapping all share (`:361-363`). Ported that way;
>    `marker_covering` is deleted. This also closed a half of the item nothing had filed: plain
>    **Left/Right arrows could park the caret inside a marker**.
> 2. **TUI-042's `history_draft` half was already benign** — nothing mutates the registry while
>    browsing, in either codebase. The draft path was broken differently: pi pushes an undo snapshot
>    on *entering* history browsing (`:435-438`) and cyrup pushed none.
> 3. **TUI-048 cannot be closed with `unicode-segmentation`.** UAX#29 has no dictionary pass for
>    unspaced scripts, so pi lands at column 2 in `你好世界` where cyrup now lands at 3 (it landed at
>    0 before). Closing it needs `icu_segmenter` + CJK/Thai data — a workspace-level dependency call.
>
> **Four items were filed in the process** — `TUI-058` and `TUI-059` (both **FIXED** in the same
> change: pi renumbers the surviving pastes when a marker is backspaced, `:1293-1315`; and every
> motion clears `lastAction`, which cyrup did on Left/Right only, so two kills separated by `Home`
> merged into one kill-ring entry), plus `TUI-060` and `TUI-061`, which are **open**.

> **Structural note carried forward.** `cyrup/TUI-FIDELITY.md` (464 lines, ~150 presentation findings against v0.84.1) holds no stable IDs and no status table, so nothing in it reaches `00-residual-ledger.md`. That is not a hypothetical: TUI-FIDELITY's C14 recommendation to delete the `{n} queued` footer segment was applied, which is precisely what turned TUI-016 from "wrong surface" into "no surface at all". Merging that backlog into this file with real IDs remains the highest-value follow-up for this area.

> ### Reconciliation 2026-08-14 — sweeps 1 and 2 applied, counts re-derived
>
> **cyrup HEAD `380c713`** (this file was written against `04c1ba2`), tree clean. Two whole-backlog
> parity sweeps have landed since this file was last edited: **sweep 1 — 232 items across 11 crates**,
> and **sweep 2**, run under the same rules. Area agents were forbidden from editing documentation so
> that a single writer could reconcile all sixteen files in one pass; this block, and the dispositions
> written into the `## Open items` rows below, are that reconciliation. **Every status in this file
> that predates this block is stale — including the header notes above it and the
> `## Status of every item…` table.**
>
> **No ID was renumbered, merged or deleted.** A refuted item keeps its ID with the refutation
> recorded in its row, so nobody re-derives it. Refutations are corrections to *this analysis*, not
> failures of the sweep — see `00-residual-ledger.md`, which now publishes the measured error rate.
>
> **The test architecture changed underneath every path citation in this file.** The integration
> tests were relocated into their crates as unit tests (`63d729a` / `c3982b5` / `d973906`), taking the
> suite from **310 integration binaries to 6 + 8 gated** behind a new **`cyrup-it`** harness crate.
> The gate is now **6440 tests / 6440 passed / 8 skipped in 16.4 s**. Any citation of the form
> `crates/<crate>/tests/<x>.rs` in this file is stale unless it names `cyrup-it`, and note that
> `cyrup-it` is `required-features = ["it"]`, so **the gate does not build or run it**.
>
> **Still a static analysis.** Neither sweep executed the suite: area agents were restricted to
> `cargo check -p <crate> [--all-targets]` and the orchestrator ran the gate once over the combined
> work. Every red-before/green-after claim below is a reasoned argument plus a type-check, and every
> `Verify` line in this file remains a design, not an observation.
>
> **Area 07 — recount: 70 rows → 34 open (0 critical · 0 high · 14 medium · 20 low).** The header's
> "62 open, 3 crit, 7 high" is stale in every column, and the 62 itself was off by one: 70 rows minus
> the 7 already marked `FIXED 2026-08-13` is **63** counted, not 62.
>
> **All three criticals and six of the seven highs are closed.** `TUI-027` (crit) closed in sweep 1;
> `TUI-042`, `TUI-043` were already fixed; `TUI-031`, `TUI-054`, `TUI-005` closed in sweep 1; and
> `TUI-016`, `TUI-052`, `TUI-045`, `TUI-055` were **already FIXED at HEAD by commit `c8c86bc`** while
> both their status rows and their item bodies still said still-open/regressed. `TUI-N13` was likewise
> already fixed with its severity cell left at `high`. The only high-severity row left is none.
>
> **THIS AREA HAD NO SWEEP-2 PASS.** Sweep 2 covered areas 01, 02, 03, 05, 06, 08, 09/10 and 11/04;
> area 07 was not assigned. Everything below is sweep 1 plus residuals handed here by other areas.
>
> **RESIDUALS FILED HERE BY OTHER AREAS IN SWEEP 2 — this is now the largest pile of one-crate work in
> the backlog, and none of it has an area-07 item yet:**
>
> - **`EXT-019`** — the markdown render path must call
>   `ExtensionHost::transform_markdown(markdown, message_type, is_streaming, available_width)`. The
>   whole host side (WIT import + guest export, load-ordered owner list, the facade fold, the native
>   trait method, the full SDK surface) is landed; this is the only reader missing.
> - **`EXT-039`** — call `resolve_shortcuts`, invert `app/input.rs:86-94`, thread `shortcut_diagnostics`
>   into `startup_diagnostics.extensions`. The registry half is landed and re-verified.
> - **`EXT-040`** — `cyrup-tui/main.rs` consuming `shortcut_specs` (which exists).
> - **`SEAM-061`** (the backlog's remaining high) — `SessionAction::ToggleScope` in
>   `keymap.rs:888-909` plus its `handle` arm, and making `show_path` FOLLOW the scope. `SessionScope`,
>   `SessionSelector::set_scope` and `scope()` already exist (`session_selector.rs:54`, `:250`,
>   `:255`), so the missing piece has narrowed to the action + handler. **Two sweeps have split this
>   across areas 07 and 08 and neither took it; it needs one agent holding both crates.**
> - **`SESS-S05`** — the `label_timestamp` producer and the inline render + `formatLabelTimestamp`
>   port. `cyrup_session::TreeNode.label_timestamp` is populated and `SessionManager::label_timestamp()`
>   is public, so the 19-line comment at `app/tree_nav.rs:25-43` conceding there is no producer is STALE.
> - **`SESS-013`** — collapse the cyrup-tui copy of `find_git_paths` onto `cyrup_session::git_paths`.
> - **`CFG-038`, `CFG-045`, `CFG-047`** — `keymap.rs` `merge_json` skip-and-continue, `app/input.rs:130-213`
>   Escape branches 3 and 4, and three built-in slash-command metadata divergences in `commands.rs`.
> - **`CFG-048` read-time half** — `keybindings_object` (`:32-40`) must run `migrate_keybindings_config`
>   on the parsed map before the entry loop (pi's keybindings.ts:366). **Ship it with `CFG-038` — both
>   edit the adjacent function.**
> - **`PROV-035`** — the `/session` `Cache Re-billed: $X (N tokens, M misses)` line at `app/execute_session.rs:147`
>   under pi's `stats.cost > 0 || cacheWaste.missedTokens > 0` guard, now that
>   `cyrup_provider::cache_stats` exists. Note the port keys misses by INDEX, not by message
>   reference — the renderer must carry the index.
> - **`CFG-044`** — the dangling doc at `auth_select.rs:39-42`, which still names `auth.rs::get_auth_status`.
>
> **`TUI-028`'s upstream citations are wrong and were not fixed** (area 05 could not edit this file):
> `keybindings.ts:208-270` and `migrateKeybindingsConfig (:294-311)` should be **`:209-269`** and
> **`:289-309`**, identical at both tags. `TUI-028` is now UNBLOCKED in the sense `CFG-048` required —
> the alias table exists and maps to `tui.editor.*` — and **it must NOT delete the `editor.*` arms of
> `EditorAction::from_id`**, which are what keeps shipped-cyrup configs working.
>
> **`TUI-055` is closed but its consequence is not**: `SESS-040` cannot be verified until a compaction
> band actually renders, and `SESS-040`/`041`/`042` now differ only in wiring.


> ### Round-2 refresh 2026-08-19 — cyrup HEAD `4fb5e40`, tree clean
>
> **Twenty-seven commits landed since this file was last touched, and this block is the reconciliation of area 07
> against them.** Every verdict below was reached by opening the Rust at HEAD; three were reached by RUNNING it.
> No id was renumbered, merged or deleted.
>
> **The `app.rs` split is the single largest change under this file.** `40821ed` replaced
> `crates/cyrup-tui/src/app.rs` (10,607 lines) with `crates/cyrup-tui/src/app/` — 33 modules, 10,511 lines — so
> **`crates/cyrup-tui/src/app.rs` NO LONGER EXISTS** and every `app.rs:NNNN` citation in this file was dangling.
> All of them are repaired. Two failure modes were found and both are worth stating, because the next pass will
> meet them again: (a) an automated pass had rewritten a range's START and left its END at the pre-split number,
> producing 77 citations like `app/submit.rs:206–2453` against a 231-line file; (b) a citation that was ALREADY
> WRONG before the split was remapped faithfully into a wrong new location — `TUI-088`'s `handleCtrlC`
> destination pointed at `app/submit.rs`, `TUI-091`'s at share-export code, and the `/tree` label-write chain at
> `app/tree_nav.rs`, which has no part in it. **Two more were found on 2026-08-19, when that pass's own claim
> ("every citation here was re-derived by SYMBOL") was checked and did not hold:** (c) the remap assumed ONE
> authoring revision, but this file's citations were written across ~84 revisions of `app.rs`, so a row whose
> `app.rs:NNNN` meant "line NNNN months ago" was remapped to whatever text happens to sit at line NNNN of
> `40821ed~1` — a pointer that matches TEXT but not MEANING, which is how `TUI-022`'s progress wiring came to
> cite an `ApplySetting` comment and `TUI-046`'s three Kitty pushes came to cite the event fold; and (d) the
> remap's regex matched only the ANCHORED citation, so every continuation in this file's `` `app/mod.rs:N`, `:M`,
> `:P` `` chains kept its PRE-SPLIT number and pointed thousands of lines past the end of a 300-500 line module —
> `TUI-S01` cited `:6915-6919` and `:7098-7102` against a 463-line file. **Repaired 2026-08-19 by symbol: 18
> anchors re-derived and 44 bare continuations re-anchored, plus six version-lag cites outside the `app/` tree
> (`keymap.rs`, `crates/cyrup/src/main.rs`, `startup_ui.rs`) corrected on the same lines — each read with
> `sed -n` at the target before being written, and a continuation whose module differs from its anchor now
> spells the module out instead of reading as the anchor's. Where the code a citation described no longer
> exists, the prose says so rather than carrying an invented pointer.** The one place a bad remap is quoted
> deliberately, as a negative example, is `TUI-091`'s citation-repair note; it is correct as written and must
> not be "fixed".
>
> **Verdicts that moved.**
>
> - **`TUI-092` de-escalated `critical` → `high`, and stays open for ONE reason.** All three clauses that
>   justified `critical` are false at HEAD: `Ctrl+D` is bound and kills, `TUI-088` is closed so `Ctrl+C` is bound,
>   and round 1's reader-thread escape hatch means a wedge is always escapable. Round 2 is **7 of 8 landed** —
>   and **F8 (by-value ingest) was never written at the time of this audit, although `e6f298d` deleted its task file and its subject said it
>   landed**. That is the finding to carry forward: a deleted task file is not a landed fix, and a commit subject
>   is not evidence. **F8 has since landed in `24b6ffe` (2026-08-20)** — `ingest_event_rendered_owned` moves `args` / `partial_result` / `result` / the queue vectors instead of cloning them, because the receiving transcript APIs already consume by value. All of F1-F8 are now in; what remains on this row is one post-round-2 live measurement.
> - **`TUI-091`'s last hypothesis is REFUTED and the row now has zero live hypotheses.** The live reasoning block
>   HAS been in the painted region since 2026-08-02/2026-08-09 (`transcript.rs:1272-1298`, by `git blame`), F2's
>   render cache cannot have hidden it, and `cargo test -p cyrup-tui --lib thinking::` is 6/6 green. What it needs
>   is a live re-observation in a real terminal — `TestBackend` cannot settle it — with `TUI-090` (fixed the same
>   day this was filed) as the leading explanation. **SUPERSEDED IN PART, SAME DAY:** the refutation was
>   then confirmed BY EXECUTION in two harnesses — including the pty-equivalent `CaptureBackend` — and the row
>   is RELOCATED out of the render path into the live event fold (`app/events_fold.rs:121-125`). The live run
>   is still what settles it, but it is now an INSTRUMENTED one against three named projections, not a
>   re-observation. See the row and its detail section; do not re-run the headless work.
> - **`TUI-015` is now PARTIALLY CLOSED.** The render coalescing it asked for arrived as `TUI-092` F3: all three
>   producing arms drain their backlog and paint one frame per wakeup. Only the 16 ms time floor is left.
> - **`TUI-078` is CLOSED** by the `CMDHINT_01` change, with one deliberate, upstream-justified deviation.
>
> **Three ids filed that did not exist here.** `TUI-093` (the mid-session cursor-position query removed, plus
> `InlineBackend`) and `TUI-094` (the `select!` starvation that produced `TUI-092`'s exact symptom) both LANDED
> with real code and real tests and had no row in any area file — `TUI-093` while being cited by name in twelve
> source locations. Both are filed and closed in the same pass. `TUI-095` records the `CMDHINT_01` feature itself,
> which is `cyrup-original` chrome pi does not render and was tracked only by an ad-hoc marker.
>
> **The rule that would have caught all three is already written down and is not being run:** "grep the SOURCE for
> `AREA-NNN` citations at every reconciliation, not just the docs". It must be extended to COMMIT SUBJECTS, which
> is what hid `TUI-094` and `CMDHINT_01`.
>
> **Recount — 104 rows: 44 struck closed/fixed, 60 open (0 critical · 2 high · 18 medium · 40 low).** The two
> highs are `TUI-091` and `TUI-092`. Four of the 60 are partial in their own severity cell (`TUI-015`, `TUI-025`,
> `TUI-048`, `TUI-062`) and one more is partial by its status row (`TUI-030`). **The 60 is two too many:**
> `TUI-N10` and `TUI-N11` say "fixed this pass" in their own titles without their rows being struck, so a
> strike-through pass over those two takes the counted set to **58**. *(Previous edition: 96 rows, 57 open.)*
>
> **Found while repairing citations, NOT acted on, and listed so the next pass does not have to re-find it:** five
> open rows now assert things that are false at HEAD — `TUI-079` (`path_command_argument` exists and is used,
> `app/submit.rs:84-85`), `TUI-080` (`/name` with no argument is a getter, `app/submit.rs:88-95` → `C::ShowName`
> at `app/execute_misc.rs:268`), `TUI-082` (`push_block("Session (HTML)", …)` has zero hits; the branch is
> `app/execute_session.rs:64-104` with one shared status string), `TUI-084` (the normalization warning exists,
> `app/execute_misc.rs:249-257`) and `TUI-N04` (the untrusted-project banner IS rendered —
> `App::render_project_trust_warning_if_needed`, `app/session_bind.rs:125-136`, from `app/run_arms.rs:123`
> and `:272`, with `tests/project_trust_banner.rs` covering it). `TUI-002`'s "no
> `has_visible_content_after` spacer" is also false for the live leg (`transcript.rs:1293-1297`). These
> need a substantive re-audit, not a citation repair, and were left for one.


## Status of every item from prior analyses

> **RECONCILED WITH THE `## Open items` TABLE 2026-08-19, at cyrup HEAD `4fb5e40`.** Twenty-six rows below
> disagreed with their own row in `## Open items` — every one of them a verdict this table had never been
> updated for after the 2026-08-14 sweep reconciliation, which said so itself ("Every status in this file that
> predates this block is stale — including … the `## Status of every item…` table") and then did not fix it. The
> Status cell of each is now struck through and carries the closure marker; **the Evidence prose is left exactly
> as written, because it is the historical record of what was true when the row was audited, not a claim about
> HEAD.** Read a struck row as "this was the finding; the `## Open items` row carries the current verdict".
>
> **STANDING DEFECT, recorded rather than silently carried: `TUI-052` … `TUI-089` have NEVER had rows in this
> table.** They were filed after it was last extended, so a section the README requires to cover every item from
> every pass covers 58 of the area's 103. This pass added rows only for the items it moved — `TUI-078`,
> `TUI-090` … `TUI-095` — rather than back-filling 38 rows it did not audit, which would have been the same
> mistake in the other direction. **Until that back-fill happens the `## Open items` table is the authority for
> any id, and this one is a commentary on the ids it happens to name.**
>
> **Two rows here still disagree with the table for a reason that is the TABLE's defect, not this section's:**
> `TUI-N10` and `TUI-N11` read `**closed** (fixed this pass)` here while their `## Open items` rows carry a live
> `low`/`medium` severity with an unstruck title and a trailing "— **fixed this pass**". Whoever next touches
> those two should strike the table rows rather than re-open these.

| ID | Status | Evidence |
|---|---|---|
| TUI-001 | **closed** | Adversarially re-read at v0.84.1 (`assistant-message.ts` moved 45 lines in the drift window). `crates/cyrup-tui/src/app/event_extract.rs:44-85` `stop_reason_notice` matches `pi/packages/coding-agent/src/modes/interactive/components/assistant-message.ts:172-195` branch-for-branch: `Length` returns *before* the `has_tool_calls` early-return (the same asymmetry as `:177`), `Aborted` special-cases `"Request was aborted"` (`app/event_extract.rs:56` vs `:180-183`), `Error` maps `Error: {m|Unknown error}`, and Pending/Deferred/Stop/ToolUse fall through — `"deferred"` appears nowhere in pi's file either. Wired on the live arm (`app/execute_misc.rs:11`) and the replay walk. Closure holds. |
| TUI-002 | **partially-closed** — open | Markdown half landed: `crates/cyrup-tui/src/transcript.rs:1232` now routes `thinking_lines` through `crate::markdown::render_with_default_style(body, width.max(1), theme, style.fg, true)`, matching `assistant-message.ts:144-162`. Fold-ordering and the `hasVisibleContentAfter` spacer are still absent — see the item. |
| TUI-003 | still-open | `rg 'Session compacted\|compaction_count' crates/ -g '*.rs'` → zero hits at HEAD. pi still emits it at `interactive-mode.ts:3706`. The swap arm (`app/run_arms.rs:260-267`) replays and pushes no compaction status. |
| TUI-004 | still-open | Both halves re-verified. mode 2031: `rg 2031 crates/` hits only the rationale prose at `theme.rs:1483-1485`; pi writes it at `packages/tui/src/tui.ts:701, :731, :749`. `/reload`: `rg 'apply_from_settings\|set_registered_themes' crates/` → zero; pi calls `setRegisteredThemes(...)` then `await themeController.applyFromSettings()` at `interactive-mode.ts:5734-5735`. |
| TUI-005 | ~~still-open~~ **CLOSED 2026-08-14** | Worse than filed — `Action::Interrupt` (`app/input.rs:130-213`) cancels a running bash block in a plain `if`, not an `else if`. See the item. |
| TUI-006 | still-open | One half landed since the baseline: extension **tool** and **flag** conflicts are now recorded (`crates/cyrup-ext/src/registry.rs:222, :604`) and folded into `LoadExtensionsResult::errors`. Slash-command conflicts, built-in shadowing and shortcut conflicts remain invisible; `rg 'command_diagnostics\|shortcut_diagnostics\|builtin_conflicts' crates/ -g '*.rs'` → zero. |
| TUI-007 | **closed** (with cross-reference) | The literal `[image]` placeholder is still absent and tool-result images render (`transcript.rs:1270-1295`). **Do not read this closure as tool-result-image parity** — the capability gate its closing commit claimed still does not exist. See TUI-N01. |
| TUI-008 | still-open | `keymap.rs:96-114` `Action::from_id` recognizes exactly 14 ids; the seven pi wires at `interactive-mode.ts:2804-2814` are absent. `app.session.toggleNamedFilter` is now handled, but only inside the session selector (`keymap.rs:801`). The `app.pageUp`/`app.pageDown` spelling complaint moved to TUI-028. |
| TUI-009 | ~~still-open~~ **CLOSED 2026-08-14** | `rg 'last_escape' crates/ -g '*.rs'` → zero; `AppState` has no escape timestamp (contrast `last_sigint`) and `Action::Interrupt` has no empty-editor branch. The `doubleEscapeAction` settings row is still live at `app/settings_rows.rs:166-169`. |
| TUI-010 | ~~**partially-closed** — open~~ **CLOSED 2026-08-14** | Committed tool blocks now honour the live expand flag (`transcript.rs:2769`, `ImageOpts::tools_expanded` documented at `:1317-1323`) and branch/compaction summaries render collapsed with an expand hint (`transcript.rs:2873, :2876-2891`). The Ctrl+O status echo is still missing. |
| TUI-011 | still-open | `app/submit.rs:111-114` is verbatim `"changelog" => { push_block("What's New", "No changelog entries found."); }`. pi really parses and renders (`interactive-mode.ts:102` imports, `:1178-1188` startup notice, `:6056-6057` the command). |
| TUI-012 | still-open | `autocomplete.rs:28-36` `CompletionContext` is still `{Slash, Path, Mention}`; `has_arg_completion` (`commands.rs:44`) still has no reader. pi installs real argument completions at `interactive-mode.ts:648` and `:674`. |
| TUI-013 | ~~still-open~~ **CLOSED 2026-08-14** | `autocomplete.rs:168` `PATH_DELIMS` and `:202` `before.rfind(PATH_DELIMS)` unchanged; no unclosed-quote scan in that file. **New for the fix:** cyrup already owns an unclosed-quote scanner at `crates/cyrup-tui/src/session_search.rs:149, :179, :183` (`had_unclosed_quote`) — an in-repo precedent the original item did not know about. |
| TUI-014 | ~~still-open~~ **CLOSED 2026-08-14** | The host sink is now installed, so `UiEffect::SetWidget` *reaches* the TUI and is dropped into a field with no reader: `app/extension_ui.rs:336`, declared `app/state.rs:227`, cleared `app/extension_ui.rs:441`, only other references are two test assertions. |
| TUI-015 | ~~still-open~~ **PARTIALLY CLOSED 2026-08-19 — still open** | The run loop's event arm still ends in `self.draw_synchronized()?;` (`app/run_action.rs:242-298`); `rg 'MIN_RENDER\|needs_render\|request_render' crates/cyrup-tui/src` → zero. pi has `MIN_RENDER_INTERVAL_MS = 16` at `packages/tui/src/tui.ts:343`. |
| TUI-016 | ~~still-open — **regressed**, and **misdescribed until 2026-08-13**~~ **CLOSED 2026-08-14** | `QueueUpdate` still discards the texts (`app/events_fold.rs:189-194`) **and** the fidelity work deleted the footer segment that displayed the count: `grep -n 'queued' crates/cyrup-tui/src/status.rs` now returns only the doc lines and the setter at `:149-150`, no render site. **Corrected by live measurement:** the queue is not "entirely invisible" — `dispatch_submission` (`app/submit.rs:13`) echoes each queued message into the CHAT TRANSCRIPT as an ordinary user bubble, so the user is told it was *delivered*. Retitled and raised medium → high; see also the new **TUI-052**. |
| TUI-017 | ~~still-open~~ **CLOSED 2026-08-14** | `image.rs:102-106` still installs `Halfblocks` when `caps.images == None`; `:152-171` takes the placeholder branch only on `!show_images`/zero area/encode error; `:242-247` still emits the invented `🖼 {label} ({w}×{h})`; `app/render.rs:227-236` passes `area.width` with no cap. |
| TUI-018 | **partially-closed** — open | pi's `compactOnboarding` and the standing `onboarding` line now exist (`chrome.rs:98-114`). The **logo/version** line and the whole expanded body are still absent — `chrome.rs:127-130` states outright that cyrup does not draw the logo part, so the app name and version appear nowhere in the UI. |
| TUI-019 | still-open | Absent in cyrup, and the upstream side grew by an order of magnitude in the drift window (`tui-alt-screen.ts` +1047, `tui-main-screen.ts` +586, `scroll-view.ts`, `layout.ts`, `stack.ts`, the eight `tui.altScreen.*` ids at `packages/tui/src/keybindings.ts:43-50`, and the `tui-mode`/`fullscreen-scrollbar` settings rows at `settings-selector.ts:633-643`). Severity stays low as a deliberate ADR-0001 divergence; effort is now L+. |
| TUI-020 | **OVERTURNED** `closed` → partially-closed | The auditor closed this on `markdown.rs:126/:142-148` passing a capability flag. The capability is now *consulted* — but nothing is *emitted*. `markdown.rs:133-141` says so in its own words, and the only `\x1b]8` bytes anywhere in `crates/cyrup-tui/src` are that comment plus two strip-ANSI fixtures. The image-fallback half is untouched. See the item. |
| TUI-021 | still-open | `rg 'cache_miss\|CacheMiss\|showCacheMissNotices' crates/ -g '*.rs'` → one unrelated subagents test name. pi keeps the whole surface at v0.84.1: `settings-selector.ts:547` plus `interactive-mode.ts:3568, :3623, :3670, :4403, :4487-4488`. |
| TUI-022 | **closed** | `crates/cyrup-tui/src/terminal_progress.rs` (363 lines) is a full port of `ProcessTerminal.setProgress`: module docs give the exact byte sequences (`\x1b]9;4;3\x07` / `\x1b]9;4;0\x07`) and the 1000 ms keepalive, and `:47` records that v0.83.0 spelled the clear sequence with a trailing `;` while v0.84.1 (pi `e8a17822d`) dropped it — cyrup tracks the **newer** form. Wiring re-checked live: `app/events_fold.rs:19`/`:28`/`:200`/`:230` set it, `app/shell.rs:84`/`:96`/`:105` flush/keepalive/shutdown (the 1 s keepalive tick is `app/run.rs:309`), `app/crossterm.rs:94` flushes per frame, `app/execute_misc.rs:171` makes the settings row live, `app/run_arms.rs:236-237` re-reads on swap. Test `crates/cyrup-tui/src/tests/terminal_progress.rs`. |
| TUI-023 | **closed** | `status_indicator.rs:149-157` `set_retry` stores `{attempt, max_attempts, initial_seconds}` plus a start `Instant`; `:161-164` `retry_message` recomputes from `started.elapsed()`, so the label ticks. **Both** arms converted together — `app/events_fold.rs:281-292` (`AutoRetryStart`) and `:294-308` (`SummarizationRetryScheduled`) — with a comment naming pi's `CountdownTimer` as the reason not to format inline. Matches `components/status-indicator.ts:46-64` + `countdown-timer.ts:21-30`. |
| TUI-024 | **closed** | `status.rs:343-351` pushes the context segment **unconditionally** with an explicit comment citing `footer.ts:161`, and `:366-380` `context_text` returns `0.0%/0{auto}` when the window is unknown and `?/{window}{auto}` when only the percent is. Matches `footer.ts:107-110, :146-152, :161` including the zero-window case. |
| TUI-025 | ~~still-open~~ **PARTIALLY CLOSED 2026-08-14** | All three literals unchanged: `commands.rs:51` `"<model>"`, `:65` `login` with no hint at all, `:70` `"…prompts, and themes"`. pi v0.84.1 `slash-commands.ts:21` `"<provider/model>"`, `:35` `"<provider>"`, `:40` `"…prompts, themes, and context files"`. `/reload` status still `"reloaded resources"` (`app/extension_ui.rs:323`). |
| TUI-026 | **closed** | Closed by the fidelity work (TUI-FIDELITY X1). `grep -n '"you: "\|"assistant: "' crates/cyrup-tui/src/transcript.rs` returns only historical comments (`:1136` "was budgeting for the deleted `assistant: `", `:2678`, `:2717`) plus regression tests at `:3290-3291` and `:3316` asserting the labels are **absent**. No label spans are constructed. Matches `components/{user-message,assistant-message}.ts`. |
| TUI-N01 | ~~still-open~~ **CLOSED 2026-08-14** | `transcript.rs:1275`'s `inline` gate still consults only `images.show` and decodability; `ImageOpts` (`:1309-1324`) gained `expand_key`, `cwd` and `tools_expanded` but no capability field, so `App::detect_image_support`'s `state.image_renderer` (`app/shell.rs:375-401`) cannot reach it. |
| TUI-N02 | **closed 2026-09-04** | Was `still-open` with the note "`push_loaded_resources` has exactly ONE production call site — `crates/cyrup/src/main.rs:1669`, the boot path. The `session_swapped` arm does nine other things and never pushes the panel." True when written and false since `605f483c`: the panel is now derived from the session (`cyrup_tui::StartupReport::from_session`, `crates/cyrup-tui/src/startup.rs:178`) and the swap arm pushes it at `app/run_arms.rs:338`. See the row and the closure block under `## ~~TUI-N02~~`. |
| TUI-N03 | ~~still-open~~ **CLOSED 2026-08-14** | `confirm_selector`'s `SelectorKind::Theme` arm (`app/selectors.rs:330-336`) still returns `None`, so no `ApplySetting` reaches the persist arm (`C::ApplySetting`, `app/execute_misc.rs:137`). `open_selector` still has exactly one call site (`app/selectors.rs:275`, Theme only). |
| TUI-N04 | still-open | `rg 'This project is not trusted' crates/` → zero. pi has the whole path live: `interactive-mode.ts:3699` calls `renderProjectTrustWarningIfNeeded()`, body at `:3710-3723`. |
| TUI-N05 | still-open | `app/input.rs:86-94` still consults the built-in keymap first; `rg 'RESERVED_KEYBINDINGS\|restrict_override' crates/` → zero. pi's `runner.ts:71-90` + `getShortcuts` `:510-533` inverts the precedence and records a diagnostic. |
| TUI-N06 | still-open | `transcript.rs:631-643` still stamps `hidden` at commit time; `app/session_bind.rs:79-83`'s own comment concedes the divergence from pi's `chatContainer.clear()` + `rebuildChatFromMessages()` (`interactive-mode.ts:4050-4066`). |
| TUI-N07 | still-open | The swap arm (`app/run_arms.rs:138-288`) calls `rebind_session()` (`:160`) then `replay_session_with_extensions` (`app/session_bind.rs:145`, called at `app/run_arms.rs:266`) and appends below the previous session's flushed scrollback; no clear, no boundary rule. |
| TUI-N08 | ~~still-open~~ **CLOSED 2026-08-14** | `crates/cyrup-tui/src/tests/image.rs:56` asserts the glyph is absent on the inline path and `:67-70` asserts `🖼`, the label and `64×48` are present — pinning the invented format while `image_fallback_text` (`image.rs:353-367`) produces pi's real one. |
| TUI-N09 | ~~still-open~~ **CLOSED 2026-08-14** | `tests/extension_dialog_countdown.rs:85` still sleeps 1,100 ms and `:88` asserts the literal `"Proceed? (2s)"`; `tick_extension_dialog_countdown()` at `:86` still takes no argument, so no injectable instant exists. |
| TUI-N10 | **closed** (fixed this pass) | Both `bash_overlay` hotkeys tests now bind `alt` per `cfg!(target_os = "macos")` and interpolate it, matching `keybinding-hints.ts:12-15`. Target 12/12 green on macOS; mutation-checked against `chrome.rs:41-47`. |
| TUI-N11 | **closed** (fixed this pass) | The M7 link mirror arm renders through `render_markdown_with_hyperlinks(…, false)` and gained a capable-branch mirror. `--test markdown` 48/48 under ghostty, vscode, iTerm.app and a scrubbed env; previously only the last passed. |
| TUI-N12 | ~~still-open~~ **CLOSED 2026-08-14** | `image.rs:430/:457-459` is a write-once `OnceLock` carrying only `hyperlinks`; `rg 'set_capabilities\|reset_capabilities' crates/` → zero. The sole both-branches seam is `markdown.rs:142`'s per-call parameter. |
| TUI-N13 | **closed** (fixed this pass) | `crates/cyrup-tui/src/tests/bash_live_run.rs:67` `a_live_bash_run_names_its_spool_file` read the spool path from one wrapped visual line; on macOS's 48-char `temp_dir()` the row is exactly 120 columns and the path wraps, so the parse yielded `""`. Now flattened before parsing. `-p cyrup-tui --lib` 285/285 (was 284/1); mutation-checked against `bash.rs:435`. |
| TUI-S01 | **closed (as framed)** — see TUI-014 + TUI-033 | The item's own Overlap note said "the correct framing is the missing sink, not seven separate items". The sink now exists: `app/extension_ui.rs:137-144` `install_ui_sinks` calls `services.set_ui_effect_sink(effects)`, re-run on every session swap (`app/run_arms.rs:193-197`). Six mutators are live — Notify `app/extension_ui.rs:301`, SetStatus `:311`, SetEditorText/paste `:316`, SetToolsExpanded `:323` (which *does* push the `Tool output: …` status), SetTitle `:324` + the OSC-0 write at `app/run_arms.rs:448-453`. The residue is exactly three variants, and holding S01 open as well would book them a third time: widgets are **TUI-014**, header/footer are **TUI-033**. Carried at low purely as a pointer. |
| TUI-S02 | **OVERTURNED** `closed` → partially-closed | `panic_hook.rs` landed (`:82-89` `install_panic_hook` chains `restore_terminal_best_effort()` before the previous hook; installed at `app/crossterm.rs:48` before `enable_raw_mode`), closing pi's `uncaughtCrash` half. The item's *second* named mechanism did not: pi `interactive-mode.ts:212-220` `DEAD_TERMINAL_ERROR_CODES`/`isDeadTerminalError` → `emergencyTerminalExit()` (`:3816-3823`) has no cyrup counterpart. See the item. |
| TUI-S03 | **closed** | `crates/cyrup-tui/src/footer_data.rs` (358 lines) ports `resolveGitBranchSync`: `HEAD_REF_PREFIX = "ref: refs/heads/"` at `:38`, worktree + reftable handling documented at `:14-26`, `POLL_INTERVAL = 500ms` at `:35` matching pi's debounce. Wired live — `app/state.rs:236` field, `:352` init, `app/shell.rs:235` `FooterGitBranch::discover(cwd)`, `app/shell.rs:237`/`:257-258` `set_branch`/`poll`, `app/run.rs:132` the poll interval in the run loop, production populator at `crates/cyrup/src/main.rs:2180`. Test `crates/cyrup-tui/src/tests/footer_git_branch.rs`. |
| TUI-S04 | **closed** | `crates/cyrup-tui/src/keyboard_protocol.rs`: `KITTY_FLAGS_QUERY = "\x1b[?u"` (`:65`), `MODIFY_OTHER_KEYS_ENABLE = "\x1b[>4;2m"` (`:69`), `MODIFY_OTHER_KEYS_DISABLE = "\x1b[>4;0m"` (`:73`), `NEGOTIATION_TIMEOUT = 100ms` (`:79`), exchange at `:217`; re-exported at `lib.rs:135`. Negotiation runs **before** the reader thread starts (`app/crossterm.rs:64`), with `:126-127` documenting why it is deliberately not re-run after suspend/external-editor. Matches `packages/tui/src/terminal.ts:172, :331, :337`. |
| TUI-S05 | **closed** | `terminal_query.rs:87` `CELL_SIZE_QUERY = "\x1b[16t"` (doc at `:10`, `:84-86` citing pi's `queryCellSize`, `tui.ts:735-742`), consumed at `:376` and fed into `ImageRenderer::from_capabilities_with_cell_size` from `app/shell.rs:375-401` under the same `caps.images.is_some()` gate pi uses at `tui.ts:703`. `image.rs:110-116` `cell_pixels()` documents the measured-vs-default distinction. Test `crates/cyrup-tui/src/tests/cell_size_query.rs`. |
| TUI-S06 | **closed** | `app/input_reader.rs:10` `write_terminal_title` writes `\x1b]0;{safe}\x07`, citing `packages/tui/src/terminal.ts:515`. Re-applied at `app/run_arms.rs:104` (boot), `:179` (session swap), `app/run_action.rs:287` (a rename arriving as `SessionInfoChanged`, recomputed in the fold at `app/events_fold.rs:390`) and `app/run_arms.rs:453` (an extension's `ui.setTitle`) — and recomputed only on change (`app/shell.rs:465-467`). Control-char stripping is a documented CYRUP-DELTA hardening, not a behaviour loss. Sanitization/branding in `terminal_title.rs`; test `crates/cyrup-tui/src/tests/terminal_title.rs`. |
| TUI-S07 | **closed** | `crates/cyrup-tui/src/tmux.rs` (153 lines) ports `checkTmuxKeyboardSetup`: the `$TMUX` gate, the `tmux show -gv extended-keys` / `extended-keys-format` probes, and `:42-47` `EXTENDED_KEYS_OFF_WARNING` / `EXTENDED_KEYS_FORMAT_WARNING` — pi's sentences verbatim with `Pi`→`cyrup`. Called at `app/run_arms.rs:623` (`crate::tmux::check_keyboard_setup().await`). Matches `interactive-mode.ts:1120-1161`, invoked at `:1045`. |
| TUI-S08 | **closed** | `crates/cyrup-tui/src/drain.rs`: `:90` `drain_input<D: InputDrain>(source, max, idle)`, `:134` `drain_stdin_before_exit()`. The run loop exits through `self.drain_and_restore()` (`app/run.rs:360`), with `app/run.rs:355-360` and `app/input_reader.rs:188-196` documenting why the drain must precede the restore ("run disables raw mode on the way out, so a drain after it returns is a guaranteed no-op"). Matches `terminal.ts:377` `drainInput(maxMs = 1000, idleMs = 50)`, called at `interactive-mode.ts:3792`. |
| TUI-S09 | **closed** | `crates/cyrup-tui/src/resume_hint.rs` (241 lines) with the exact output shapes at `:10`/`:17` and `:95` citing `${chalk.dim("To resume this session:")} ${resumeCommand}\n`; exported at `lib.rs:101`; called out as a verified-clean non-drawing helper in `TUI-FIDELITY.md:111`. Matches `interactive-mode.ts:240` `formatResumeCommand` + the post-stop stdout write at `:3808-3810`. |
| TUI-S10 | still-open | `rg 'debug' crates/cyrup-tui/src/keymap.rs` → zero; `rg 'onDebug\|on_debug' crates/ -g '*.rs'` → zero. `/debug` is still reachable only by typing it (`commands.rs:76` `HIDDEN_COMMANDS`, handler `app/session_bind.rs:217`). pi installs the hook globally at `interactive-mode.ts:2803` and checks the chord before dispatching to the focused component (`tui.ts`). |
| TUI-S11 | **closed** (as the item itself scoped it) | `crates/cyrup/src/update_check.rs` (343 lines) ports the git-source update detection (`getGitUpstreamRef` at `:166-175`, `first_sha` at `:255-262`) and is spawned at boot behind the offline gate — `crates/cyrup/src/main.rs:549-558`, with `:552` recording that only the **package** half of pi's pair is ported. The `checkForNewPiVersion` release-feed poll was excluded as a fork/product decision, which is exactly what the item's own Impact paragraph recommended. Receiver threaded into `run_interactive` (`main.rs:1582-1583`, field at `app/mod.rs:219`, moved into the run context at `app/run.rs:182`/`:191` and serviced at `:227-228`). Test `crates/cyrup-tui/src/tests/package_update_notice.rs`. |
| TUI-027 | ~~**new this pass** — open (**critical**, raised from high in the repair pass)~~ **CLOSED 2026-08-14** | `/tree` has no `searchQuery`; `z`/`x`/`e`/`t` are bound to actions where pi accumulates them into a filter. The `e` path opens the inline label editor, which captures all keys and on Enter **persists** the typed text as the entry's label via `app/selectors.rs:201-208` → `app/execute.rs:288-298` `host_services.set_label` → `manager.append_label`. Corruption of persisted user data. |
| TUI-028 | ~~**new this pass** — open (medium)~~ **CLOSED 2026-08-14** | Editor/input keybinding ids use an `editor.*` namespace upstream abandoned — 24 ids inert. Absorbed TUI-008's `app.pageUp`/`app.pageDown` spelling complaint. **Cite correction (repair pass):** the upstream offsets are `keybindings.ts:209-269` and `migrateKeybindingsConfig` `:289-309`, not `:208-270`/`:294-311`; identical at both tags. **Must land after CFG-048** or it silently breaks every `editor.*` config written against shipped cyrup. |
| TUI-029 | **new this pass** — open (medium) | Extension autocomplete providers are never consulted by the interactive editor. The registration half exists in area 06; the consumer does not. Confirmed still the correct framing by the repair pass's `editor-component.ts` read — `setAutocompleteProvider` is one of only two `EditorComponent` members with no `InputEditor` counterpart. |
| TUI-030 | **PARTIALLY CLOSED 2026-08-15** (medium) | The four working-indicator verbs (`setWorkingMessage`/`setWorkingVisible`/`setWorkingIndicator`/`setHiddenThinkingLabel`) are **closed end to end**: four new `UiEffect` variants, four `LiveHostServices` overrides, the matching `App::apply_ui_effect` arms + `resetExtensionUI` restore, and a deliberate non-forward in RPC mirroring `rpc-mode.ts:179-193`. The theme trio `getAllThemes`/`getTheme(name)`/`setTheme` is **also closed** in the same commit, by `cyrup-tui`'s `TuiThemeAccess` behind the new `cyrup_session_svc::ThemeAccess`. **RESIDUAL:** `setEditorComponent`/`getEditorComponent` only (the other of the two missing `EditorComponent` members, needs a WIT-world change reconciled with area 06). `onTerminalInput` was already closed by EXT-021 sweep 6. |
| TUI-031 | ~~**new this pass** — open (high)~~ **CLOSED 2026-08-14** | A prompt typed during compaction is sent immediately instead of queued; the session layer does not serialize behind compaction either. Drain design is area 03's to own. |
| TUI-032 | ~~**new this pass** — open (medium)~~ **CLOSED 2026-08-14** | `/settings` is missing the `Warnings` and `Thinking level` submenus. |
| TUI-033 | ~~**new this pass** — open (medium)~~ **CLOSED 2026-08-14** | `ui.setHeader` / `ui.setFooter` are delivered to the TUI and dropped into fields nothing renders. One of the two residues TUI-S01 was closed in favour of. |
| TUI-034 | **new this pass** — open (medium) | No markdown-transformer hook — extension transformers and pi's Mermaid renderer both absent. Post-baseline drift; guest-facing registration needs area 06. Settings off-switch is CFG-040. |
| TUI-035 | ~~**new this pass** — open (low)~~ **CLOSED 2026-08-14** | `tui.editor.historyPrevious` / `historyNext` are unbound. The single v0.84.1 hunk to `components/editor.ts`. |
| TUI-036 | ~~**new this pass** — open (low)~~ **CLOSED 2026-08-14** | `Show images` / `Image width` rows are offered on terminals with no image protocol. Same capability-gate root as TUI-N01 and TUI-017. |
| TUI-037 | **new this pass** — open (medium) | `/reload` never persists an implicitly-granted project trust (`maybeSaveImplicitProjectTrustAfterReload`). |
| TUI-038 | ~~**new this pass** — open (low)~~ **CLOSED 2026-08-14** | Ctrl+O is an if/else in cyrup and a fan-out upstream — a live bash block blocks tool expansion. |
| TUI-039 | ~~**new this pass** — open (low)~~ **CLOSED 2026-08-14** | Terminal geometry never falls back to `$COLUMNS` / `$LINES`. |
| TUI-040 | **new this pass** — open (low) | No `PI_TUI_WRITE_LOG` equivalent — no escape-sequence write log. Elevated in usefulness by the repair pass: it is the only tractable instrument for the negotiation items (TUI-045, TUI-046) whose verification requires a live terminal. |
| TUI-041 | ~~**new this pass** — open (low)~~ **CLOSED 2026-08-14** | `/settings` shows env-overridden rows with the wrong value. |
| TUI-042 | **FIXED 2026-08-13** | Was: the undo snapshot omits the paste registry — `Snapshot { lines, row, col }` had no `pastes`, so the marker text reappeared on screen while `expanded_text()` could no longer resolve it and Enter sent the literal marker. Now: `Snapshot` carries `pastes` + `paste_counter` (`editor.rs:71-93`), `snapshot()` clones them and `undo()` restores them (`editor.ts:2012-2030`); the undo snapshot moved to the TOP of `handle_paste` so an undone paste rolls the counter back and the next paste re-issues the same id (`editor.ts:1160`); `clear()` resets `paste_counter` (`editor.ts:1264-1266`) and both submit paths clear the undo stack (`:1268`). RED→GREEN: `tests/editor.rs::undo_restores_the_paste_registry_not_just_the_marker_text` (RED output: `left: "[paste #1 1500 chars]"`, right: the 1500 chars), `::undo_rolls_back_the_paste_counter_so_the_next_paste_is_still_marker_one`, `::browsing_history_away_from_a_draft_keeps_its_paste_registry`. |
| TUI-043 | **FIXED 2026-08-13** | Was: word motion and Ctrl+W were not paste-marker atomic — one Ctrl+W deleted the single `]`. Now: `word_left_target`/`word_right_target` are a statement-for-statement port of `findWordBackward`/`findWordForward` (`word-navigation.ts:22-114`) over a marker-merging segmenter (`segmentWithMarkers`, `editor.ts:37-90`), including the `isAtomic` branches at `:44-46`/`:97-99`; `prev_grapheme`/`next_grapheme` route through the same merge, so **cursor motion** is marker-atomic too (pi's `moveCursor`, `editor.ts:1808-1830`) and the caret can no longer be parked inside a marker. `marker_covering` is deleted — upstream has no such predicate. Per `editor.ts:1607-1630`, `deleteWordBackwards` does **not** drop the registry entry, so cyrup no longer does either (see the correction below). RED→GREEN: `::ctrl_w_at_a_marker_end_deletes_the_whole_marker` (RED output: `left: "[paste #1 1500 chars"`), `::alt_d_at_a_marker_start_deletes_the_whole_marker`, `::word_motion_treats_a_paste_marker_as_one_unit`, `::arrow_keys_step_over_a_paste_marker_as_one_grapheme`. |
| TUI-044 | **FIXED 2026-08-13** | `undo()` restores `snap.col` and calls `reset_preferred_col()` (`editor.ts:2019-2022`); `exit_history()` moved ahead of the pop to match `:2017`. RED→GREEN: `::undo_restores_the_snapshot_cursor_column` — the item's own live scenario, asserting `(0, 5)` and that the next keystroke yields `helloZ` rather than `heZllo`. |
| TUI-045 | ~~**new (repair pass)** — open, **raised to high 2026-08-13**~~ **CLOSED 2026-08-14** | An escape sequence split at the ESC byte across `read(2)` boundaries is not reassembled — crossterm emits a spurious `Escape` plus the tail as typed text. `stray_reply.rs` documents observing this exact split and rescues only OSC 11. Escape is not inert: it aborts the turn. **Reproduced live, idle and mid-stream — a 60 ms gap between two writes on a LOCAL pty is enough; the "exposure is over SSH/mosh/tmux" hedge was too conservative.** |
| TUI-046 | ~~**new (repair pass)** — open (medium)~~ **PARTIALLY FIXED 2026-09-04 at `8bb0d22f` — still open (low)** | The flag set is decided and single-sourced: `crate::keyboard_protocol::DESIRED_FLAGS` = `DISAMBIGUATE_ESCAPE_CODES \| REPORT_ALTERNATE_KEYS` (`\x1b[>5u`), written by the one `push_flags()` all three sites call. `REPORT_EVENT_TYPES` is a reasoned `[CYRUP-DELTA]`, not the old silent gap, and withholding it makes the WezTerm hazard **unreachable** rather than merely unguarded. `drain.rs:11-16`'s release-report premise is corrected. **Residual:** pi's `pendingKittyPrintableCodepoint` dedup is still unported and is not portable at cyrup's post-crossterm seam. |
| TUI-047 | **new (repair pass)** — open (low) | A late or unsolicited DCS/APC frame is shredded into ~20 typed characters; `stray_reply.rs` recognises only OSC 11. Reachability is narrow (tmux passthrough), blast radius is not. |
| TUI-048 | **PARTIALLY FIXED 2026-08-13 — still open** | The character-class run is gone: word motion now goes through `unicode-segmentation`'s UAX#29 word iterator plus pi's `PUNCTUATION_REGEX` sub-boundaries (`utils.ts:821`) in pi's three-branch shape. CJK no longer jumps the whole run (`::cjk_word_motion_no_longer_swallows_the_whole_run`, RED at HEAD). **Not parity**: ICU's `Intl.Segmenter` adds a dictionary/LSTM pass, so pi lands at col 2 in `你好世界` where UAX#29 lands at 3. Closing it needs an ICU-class segmenter (`icu_segmenter` + CJK/Thai data) — a new workspace dependency, deliberately not taken here. CYRUP-DELTA recorded on `InputEditor::word_segments`. |
| TUI-049 | **FIXED 2026-08-13** | `marker_at` now matches `PASTE_MARKER_SINGLE` exactly (`editor.ts:24`): id, then either an immediate `]` or one space and exactly one of `+<digits> lines` / `<digits> chars`. RED→GREEN: `::a_hand_typed_marker_shaped_string_is_not_expanded`, which also pins that the bare `[paste #N]` form pi's regex allows still expands. |
| TUI-050 | **new (repair pass)** — open (low) | An 8-bit meta byte (a single byte > 127) is silently dropped instead of becoming `ESC` + char, so every Alt chord is dead under `metaSendsEscape: false`. **Dependent on TUI-045** — without that pre-parser there is no seam. |
| TUI-051 | ~~**new (repair pass)** — open (medium)~~ **CLOSED 2026-08-14** | `/reload` never re-reads `keybindings.json`, while both the command's help string (`commands.rs:70`) and its in-source comment (`app/execute_session.rs:242-244`) claim it does. Routed here from the migrations sweep; the config half is CFG-048. |
| TUI-078 | ~~open (low)~~ **CLOSED 2026-08-17** | Both halves of its Fix are at HEAD: `cyrup-session-svc/src/session.rs:2635-2639` inserts `argumentHint` spread-if-truthy and `cyrup-tui/src/commands.rs:475-479` reads it with pi's empty-string truthiness filter; tested at `crates/cyrup-tui/src/tests/commands.rs:248` and `crates/cyrup-session-svc/src/tests/cmdhint01_argument_hint.rs:58`/`:82`/`:99`. The "(and extension)" half was deliberately declined with the upstream reason in-source (`commands.rs:470-474`). See the row. |
| TUI-090 | **FIXED 2026-08-15** | Re-verified 2026-08-19 at HEAD `4fb5e40`: the flush-synchronized floor release survives the `app.rs` split — `app/draw.rs:66` (`flush_pending`), `:68-70` (the release), `:71` (the grow-only clamp), and `resize_viewport` (`:77-87`) still ordered before `flush_committed` (`:88`). |
| TUI-091 | **still open — but RELOCATED OUT OF THE RENDER PATH 2026-08-19** | Re-audited, then **executed**. The 2026-08-19 refutation of the "the accumulated `thinking` buffer is never in the painted region" candidate is now confirmed by RUNNING it, twice: the reasoning block renders live from delta **#0** and again in committed scrollback, through both the assembled `TestBackend` path and the pty-equivalent `CaptureBackend` escape-capture/VT-replay harness. **It does not reproduce headlessly at HEAD at all.** The defect is therefore in the ASSEMBLED LIVE APP or the terminal, not in `transcript.rs`, the F2 cache or the commit/flush wiring; the one mechanism that reproduces the reported asymmetry (answers render, reasoning does not) is the `MessageEnd` guard at `app/events_fold.rs:121-125`. Next step is instrumentation on three named projections in a real terminal — see the row. |
| TUI-092 | ~~**critical**~~ ~~**high**~~ **CLOSED 2026-08-20 — F8 landed (`24b6ffe`); the epic is complete** | De-escalated 2026-08-19: `Ctrl+D` is bound and kills (`keymap.rs:655` → `app/input.rs:126-129` → `app/run_action.rs:16`), `TUI-088` is closed, and the reader-thread escape hatch (`app/input_reader.rs:147-206`) makes a wedge escapable. Round 2 is 7/8 landed; **F8 (by-value ingest) was never written** though `e6f298d` deleted its task file — `app/run_action.rs:281-282` still passes by reference and `ingest_session_event_owned` has zero hits. |
| TUI-093 | **FIXED 2026-08-17 — filed retroactively 2026-08-19** | Landed at `77dca02` + `743cad8` with no row while being cited by name in twelve source locations. `InlineBackend` (`app/backend.rs:91`) answers `get_cursor_position` from a tracked anchor (`:154-158`) instead of a `CSI 6 n` round-trip that raced the reader thread; the process makes exactly ONE bounded probe (`app/crossterm.rs:64-82`), and a failed viewport reconstruction is now non-fatal (`app/draw.rs:77-87`). |
| TUI-094 | **FIXED 2026-08-18 — filed retroactively 2026-08-19** | Landed at `879eb4e` with no row. The events arm's irrefutable `maybe_ev = events.next()` matched the `None` a session swap produces (`cyrup-session-svc/src/subscriber.rs:89-93`) and, under `biased;`, starved the swap arm below it. Now refutable (`app/run.rs:344`) with the swap arm hoisted above every permanently-ready arm (`:293-297`); pinned by `src/tests/run_loop_swap_arm_reachable.rs:39`. |
| TUI-095 | **new 2026-08-19** — open (low, record-only) | The persistent command-token highlight + argument-hint ghost (`CMDHINT_01`, `0b7c4f4` + `bae24f5`) is `cyrup-original` chrome pi renders neither half of (`editor.rs:211-213`), shipped with thirteen in-source citations and no ledger row. Record-only; its one open consequence is the unrecorded `argumentHint` addition to the `get_commands` RPC payload, routed to area 08. |

## Open items

> The `-S` surface-sweep items are merged into this single table. The prior revision kept them in a
> second table further down the file, and enumerating only the first table undercounted the area by
> 11 — which is how `SEAM-S01` escaped a full audit pass on 2026-08-07. One table now; `-S` ids keep
> their suffix to mark provenance.

> **RECOUNTED 2026-09-04, batch 2 (ledger audit); authoritative over every block below.**
> **104 rows: 74 struck closed/fixed, 30 open — 0 critical, 0 high, 4 medium, 26 low** (`scripts/count_open_items.py`).
> Five rows moved, all on landed code re-read by an independent review: `TUI-037` CLOSED (`0e8c62fa`,
> `/reload` persists an implicit project trust), `TUI-068` CLOSED (`eacd771a`, `app.session.deleteNoninvasive`
> bound), `TUI-081` CLOSED (`84b205a1`, `/import` confirms before replacing the live session), `TUI-089`
> CLOSED — REFUTED (`d685eff1` guard test; cyrup already orders the picker at pi's two points), and
> `TUI-025` CLOSED (its last residual landed with `TUI-037`). The four remaining mediums are `TUI-004`,
> `TUI-046`, `TUI-N02` and `TUI-N11`; the header paragraph below that says "`TUI-081` partially
> re-scoped, left open" is the earlier pass's prose, superseded by the row.
>
> **RECOUNTED 2026-09-04, second pass — `TUI-091` CLOSED; authoritative over every block below.**
> **104 rows: 69 struck closed/fixed, 35 open — 0 critical, 0 high, 8 medium, 27 low.** One row moved:
> `TUI-091` closed as a duplicate of `TUI-090` on an observation in a real pty (tmux 3.4, 120×40,
> HEAD `a4805955`, seven variants including the owner's exact `together`/Kimi-K3/`high` path — see
> the row, its detail section, and `REPRO-LOG.md` §0e). **This area has no above-medium row for the
> first time.** Nothing else was re-read this pass; the eight mediums are as the block below lists
> them. No code in `crates/cyrup-tui` changed.
>
> **RECOUNTED 2026-08-19 (round-2 refresh, fourth edition) — 104 rows: 44 struck closed/fixed, 60 open at
> 0 critical · 2 high · 18 medium · 40 low.** The prior edition read 96 rows / 57 open, and the delta is:
> three rows added (`TUI-093`, `TUI-094`, `TUI-095` — the first two filed retroactively for work that had already
> landed and closed in the same pass), `TUI-078` closed, `TUI-015` moved to partial, and `TUI-092` de-escalated
> `critical` → `high`. **This area no longer has a critical**, and its two highs — `TUI-091` and `TUI-092` — are
> both blocked on the same thing: a live run in a real terminal, which no pass since 2026-08-15 has done.
> Four of the 60 are partial in their own severity cell (`TUI-015`, `TUI-025`, `TUI-048`, `TUI-062`) and `TUI-030`
> is partial by its status row. **`TUI-N10` and `TUI-N11` are counted open but say "fixed this pass" in their own
> titles** — striking those two rows takes the counted set to 58, and is the cheapest correction available here.
> **Every `crates/cyrup-tui/src/app.rs` citation in this table is gone**: that file was deleted by `40821ed` and
> each citation was re-derived by symbol against `crates/cyrup-tui/src/app/` and read at the target.

> **SUPERSEDED 2026-08-19 — RECOUNTED 2026-08-14 (sweeps 7-8 reconciliation, third edition) — counted set UNCHANGED at 0 critical, 0 high, 14 medium, 21 low = 35.** The table carries **71 rows: 36 fully closed, 35 open (3 partially) — still the largest open area by a wide margin**. **No area-07 row moved in sweep 8**, but two foreign routings into this file did: **`CFG-051`'s routed residual is SATISFIED and cyrup-tui now owns the pin** (in `src/transcript.rs`, deliberately not in `tests/` — see the routing table), and **`CFG-045` closed as already-done at `app/input.rs:130-213`**, which also discharges the "a `/settings` row is not a consumer" finding that produced it. **`TUI-062`'s two residuals are unchanged and now have a dependent test.** Area 07 remains the largest open area by a wide margin and is where the next sweep's volume is. *(Previous edition: same counts, 36 closed.)*

> **SUPERSEDED — RECOUNTED 2026-08-14 (sweeps 3-6 reconciliation) — counted set: 0 critical, 0 high, 14 medium, 21 low = 35.** 36 rows are now marked CLOSED. `TUI-062` is new (filed by sweep 6 from another partition, partially closed). This area had no sweep-2 pass; see the reconciliation block above for the residuals other areas filed here.

> **AMENDED 2026-08-14 (documentation audit) — two rows added, `TUI-063` and `TUI-064`; the area's counted set becomes 0 critical, 0 high, 14 medium, 23 low = 37.** Both came from reading the shipped `--help` and the TUI input paths against the code while writing user documentation, not from the backlog. `TUI-064` is a prerequisite for observing `TUI-017` end to end.

> **AMENDED 2026-08-14 (sweep 10 — mechanical surface enumeration) — 23 rows added, `TUI-065` … `TUI-087`; three of them (`TUI-069`, `TUI-070`, `TUI-074`) are FIXED in the same pass and are therefore not counted. The area's counted set becomes 0 critical, 0 high, 17 medium, 40 low = 57, across 96 rows (39 closed-or-fixed, 57 open, 3 partially).** These came from enumerating two finite surfaces MECHANICALLY on both sides and diffing in both directions, not from reading the backlog: **keybinding ids and their default chords** (73 upstream = 31 `tui.*` + 42 `app.*`, confirmed against pi's own shipped `docs/keybindings.md` table) and **slash commands, their argument parsing and their argument completion** (25 dispatch names on each side). Both surfaces were enumerated **completely**, so this filing is exhaustive for them.
>
> Three results are worth carrying forward. (1) **The chord-parse audit sweep 9 prompted came back clean**: all 58 distinct default key-spec strings upstream ships parse under `Key::parse` at HEAD, including `ctrl+-`, `ctrl+]`, `ctrl+alt+]` and the `f1`…`f12` family sweep 9 fixed — so neither newly-found id is a *dead chord*, both are *missing destinations* (`TUI-067`, `TUI-068`), and the only spec-vocabulary hole left is `clear` (`TUI-073`), which no upstream default uses. (2) **The `cyrup-original` class is the one this area had no habit of tracking**: `TUI-065` exists only because TUI-028's closure said the two invented ids "are listed as a new gap" and no row was ever added, and `TUI-066` exists because TUI-028's closure deliberately *preserved* three invented vocabularies whose only tracking row then closed. (3) **The slash surface's 9-missing / 9-differing yield on a 25-command surface** came almost entirely from argument handling — arity, quoting, hints, completion and the argument-less forms — which is the half of a command that no autocomplete list shows.
>
> Filed with no id, each for a stated reason: `/model` and `/login` argument completion → **TUI-012**; the `has_arg_completion` bool-vs-supplier shape and its request-time-vs-item-time consequence → **TUI-012**; extension `getArgumentCompletions` threading → **EXT-013** (routed, area 06); `getBuiltInCommandConflictDiagnostics` → **TUI-006**; the forward-ported `tui.editor.historyPrevious`/`historyNext` ids → **TUI-035** (closed).


> **⚠ ROUTING — 2026-08-14 (sweeps 3-6 reconciliation). ELEVEN OPEN ROWS FILED IN OTHER AREA FILES HAVE THEIR FIX SITE IN `crates/cyrup-tui`.** Their ids stay in their own files (ids are never renumbered or moved) and they are **not counted here**, but a cyrup-tui assignment that ignores them will keep leaving the cheapest work in the backlog undone. Ranked by sweep 6's own assessment:
>
> | id (home file) | what is left, in cyrup-tui |
> |---|---|
> | **`CFG-038`** (05) | *Sweep 6 calls this the single highest-value unlanded item in area 05.* All four `merge_json` bodies in `src/keymap.rs` (`:597-604`, `:684-691`, `:787-794` and siblings) propagate `parse_key_values(&value)?` from **inside** the entry loop, so one typo'd key spec aborts the load AFTER earlier entries were applied — and `crates/cyrup/src/main.rs:1624-1629` then prints "ignoring {path}", which is false. ~30 lines + one test: skip-and-continue per entry with a collected failure list, returned from `App::load_keybindings_json`, named by `main.rs`. The whole-document error must survive ONLY for `keybindings_object`. |
> | **`EXT-013`** (06) | One line: `src/commands.rs:348` hard-codes `has_arg_completion: false`. The cyrup-ext side (`facade.rs::command_completions`, `host/live.rs::argument_completions`) already exists and works. |
> | **`TOOL-015` / `TOOL-022`** (04) | The producer half is done in cyrup-tools and cyrup-ext; **nothing in cyrup-tui branches on `Tool::render_kind()`** (`grep -rn render_kind crates` → zero consuming sites). Wire the transcript's per-tool render dispatch so `SelfRendered` suppresses the generic frame. Needs cyrup-tui + cyrup-core. |
> | **`CFG-045`** (05) | The `Action::Interrupt` arm in `src/app/input.rs:130-213` plus a `last_escape_time` field (pi `interactive-mode.ts:2570-2596`). |
> | ~~**`CFG-051`** (05)~~ **SATISFIED 2026-08-14 (sweep 8) — this routing row is discharged.** | ~~The residual is a rendered-transcript assertion in `crates/cyrup-tui/tests/` plus a live-terminal confirmation~~ — **both landed. cyrup-tui now owns the pin:** `the_migrated_credential_notice_renders_first_and_verbatim_in_the_transcript` at `crates/cyrup-tui/src/transcript.rs:3466`, rendering through `entry_lines` (the production path `app/draw.rs:166` uses) and asserting order, exactly ONE `Warning: ` prefix and the warning colour; mutation-verified. **DEVIATION, stated:** it is **in-src**, not in `crates/cyrup-tui/tests/`, because `entry_lines`/`TranscriptView::lines` are crate-private and widening the public API for a test is the worse trade. Live half observed in a running UI and proven TUI-owned by a mid-session resize (row 29 → row 4 at 90x28) — `REPRO-LOG.md` §0c. |
> | **`CFG-014` / `CFG-015`** (05) | The consumer halves of `show_cache_miss_notices`, `code_block_indent`, `last_changelog_version`, `collapse_changelog`, `npm_command`. The accessors exist; **landing accessors alone is the "a /settings row is not a consumer" failure.** |
> | **`TOOL-017`** (04) | The `docs` classification arm — but a **product decision comes first**: cyrup has no decided shipped-docs root for `getPiDocsClassification`. Blocker recorded in-source at `src/transcript.rs:2305`. |
> | **`EXT-041` / `EXT-053`** (06) | Replay/draw path; autocomplete shadowing diagnostic. |
> | **`TUI-062`** (this file) | The wrong `showWarning` cite at `src/transcript.rs:2965-2971` and the prefix-location design question. **BOTH RESIDUALS UNCHANGED at 2026-08-14 (sweep 8) — but note the new dependency:** the `CFG-051` pin above **deliberately asserts the verbatim / exactly-one-prefix behaviour as it stands today**, so **it will need editing if the design question is ever settled the other way** (prefix moved into the renderer). Sweep 8 also produced the concrete argument for settling it: an injected double prefix left the *producer's* own unit test green and **only the rendered assertion caught it** — a string-level test on the producer side cannot see a renderer that re-prefixes, truncates or drops the line. |


> ### Reconciliation 2026-09-04 — cyrup HEAD `2571969` (baseline was `4fb5e40`, 210 commits, 39 touching `crates/cyrup-tui`)
>
> **Static-read audit only — no pty, no live terminal.** Per this file's own rule, nothing below is
> closed on the strength of a `TestBackend`/unit-test pass alone where the item's own defect was a
> LIVE-rendering claim; those stay open with a note. Every closure below was reached by reading the
> Rust at HEAD against the cited pi source at the tag this file already carries — no closure rests on
> a commit message, and several commit subjects in this window ("the audit backlog is now empty")
> were treated as a hypothesis and checked against the code, not trusted.
>
> **21 items closed**, all with a symbol-anchored citation in their row: **TUI-002** (thinking-block
> fold order + `hasVisibleContentAfter` spacer, `transcript/cache.rs:100-138`), **TUI-006** (all four
> diagnostic sources, `startup.rs:241-244`), **TUI-008** (all seven global ids wired,
> `app/input.rs:426+`), **TUI-012** (`/model`, `/login` argument completion,
> `commands.rs:143-149,168-174`), **TUI-015** (the 16 ms floor, `app/frames.rs:10-11`), **TUI-019**
> (the alternate-screen renderer, `altscreen/`, shipped whole), **TUI-020** (OSC-8 now actually
> emitted, `osc.rs`), **TUI-021** (cache-miss notices, `app/events.rs:231-239`), **TUI-029**
> (extension argument completers, `autocomplete.rs:309`, `commands.rs:570-594`), **TUI-030**
> (`setEditorComponent`/`getEditorComponent`, the one residual, is now a DOCUMENTED deliberate
> non-port — `cyrup-ext/src/lib.rs:54-64` gives the Component Model reason independent of any ADR
> citation), **TUI-034** (the markdown-transformer hook and a real Mermaid renderer,
> `app/events.rs:102-140`, `markdown/mermaid.rs`), **TUI-063** (`share_viewer_url`,
> `app/share.rs:30-46`), **TUI-067** (`EditorAction::PassThrough`, `keymap.rs:354,435`), **TUI-079**
> (`path_command_argument`, `app/submit.rs:90-91`), **TUI-080** (`/name` getter,
> `app/execute_misc.rs:804`), **TUI-082** (bare `/export` writes a file,
> `app/execute_session.rs:92-104`), **TUI-083** (documented deliberate decision — cyrup has no
> config-name-override to template against, `commands.rs:187-192`), **TUI-085**
> (`autocomplete_source_tag` returns `None` on no `sourceInfo`, `commands.rs:425-437`), **TUI-086**
> (`pub(crate)`, and the extra `Builtin` variant is now a documented consequence of unifying two
> upstream types, `commands.rs:19-24,421-424`), **TUI-087** (every citation repaired in place) and
> **TUI-N04** (the untrusted-project banner IS rendered, `app/session_bind.rs:100-129`,
> `app/run_arms.rs:119,289` — this was already flagged false in the 2026-08-19 round-2 note but never
> applied to this table until now).
>
> **Verified still open, unchanged, evidence recorded so the next pass does not re-walk them:**
> **TUI-004** (a related but distinct half landed — `theme.rs`'s `ThemeController::sync_with_terminal`
> now does real OSC-11 boot detection instead of `COLORFGBG`-only — but live colour-scheme sync stays
> off by deliberate documented choice, `theme.rs:1519-1528`, and `/reload` still has no
> `sync_with_terminal`/theme-setting-reapply call site: `rg 'sync_with_terminal' crates/cyrup-tui/src`
> hits only the one boot call and its own doc comments); **TUI-011** (`app/submit.rs:117-118` is
> still the verbatim stub); **TUI-018** (`chrome.rs:126-130` still states outright that no logo is
> drawn); **TUI-025** residual (unchanged, still depends on TUI-037); **TUI-037** (no
> `maybeSaveImplicitProjectTrustAfterReload` write found anywhere in `crates/cyrup`); **TUI-040**,
> **TUI-047**, **TUI-048** (partial, unchanged — no `icu_segmenter`), **TUI-050**, **TUI-056**,
> **TUI-057**, **TUI-060** (`editor/wrap.rs`'s `word_wrap_line` still takes a plain `&[char]` with no
> marker-merged pre-segmentation — the CYRUP-DELTA note added nearby covers CJK/emoji width only, not
> paste-marker atomicity across a wrap point), **TUI-062** (partial, unchanged), **TUI-064**
> (`attach_image`/`attach_image_path` still have zero production callers,
> `rg '\.attach_image\(' crates/cyrup-tui/src` → three test call sites only), **TUI-065**,
> **TUI-066**, **TUI-068** (`rg 'deleteNoninvasive|DeleteNoninvasive' crates/` → zero), **TUI-071**
> (`keymap.rs:704-709` still binds `ctrl+v`/`alt+v` for `pasteImage` unconditionally on every
> platform), **TUI-072**, **TUI-073**, **TUI-075**, **TUI-076**, **TUI-077**, **TUI-089** (checked
> `model_selector.rs`'s `sort_models` — it re-orders the ALREADY-BUILT list so the current/default
> model sorts first, which is a different mechanism from the catalog-assembly-order complaint this
> item files; found no evidence either way in the provider/config catalog-assembly code, which is
> outside this area's owned crate — left open rather than guessed at), **TUI-095** (record-only,
> unchanged), **TUI-N02** (`push_loaded_resources` still has exactly one production call site,
> `crates/cyrup/src/interactive.rs:340`), **TUI-N05**, **TUI-N06** (still explicitly cited as an open
> residual at three call sites, e.g. `status.rs:114`), **TUI-N07**, **TUI-S02**, **TUI-S10**.
>
> **TUI-046 re-verified, NOT closed despite an expanded doc comment that could be misread as a fix.**
> `keyboard_protocol.rs`'s module doc now explains pi's full `CSI > 7 u` push in prose, but the actual
> push at all three call sites (`app/crossterm.rs:55,133,223`) still constructs
> `KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES` alone — bit `1`, not `7`. The gap is
> unchanged; only the documentation explaining pi's side grew.
>
> **TUI-081 partially re-scoped, left open.** The `cancelled` arm this item called unreachable is no
> longer strictly so — `AgentSessionRuntime::import_from_jsonl` (`cyrup-session-svc/src/runtime.rs:697-704`)
> now returns `cancelled: true` when an extension vetoes the swap via `HostEvent::SessionBeforeSwitch`
> — but that is an extension-authored veto, not the interactive user confirmation the item asks for; a
> mistyped `/import <path>` still overwrites the live session with no prompt. Left open with the
> correction recorded so the next pass does not re-file "the `cancelled` arm is dead code".
>
> **TUI-091 (the one remaining `high`) is untouched this pass.** Its own row already says the
> defect can only be settled by a live terminal instrumenting three named projections, and this pass
> had no pty available — re-reading the same static evidence again would not move it. Left exactly as
> written.
>
> **Recount after this pass (by ID, machine-counted from the table below, not by hand): 104 rows, 68
> struck-closed-or-FIXED, 36 open — 0 critical, 1 high (`TUI-091`), 8 medium (`TUI-004`, `TUI-037`,
> `TUI-046`, `TUI-N02`, `TUI-N11`, `TUI-068`, `TUI-081`, `TUI-089`), 27 low.** 21 items were struck
> this pass (listed above); the prior edition's 58-open figure already counted `TUI-N10`/`TUI-N11` as
> open despite their titles saying "fixed this pass" — both are carried forward unstruck, unchanged,
> per that edition's own note.


| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| TUI-042 | **FIXED 2026-08-13** | parity-bug | S | ~~The undo snapshot omits the paste registry — undoing a delete over a `[paste #N …]` marker silently drops the pasted content from the submitted message~~ |
| TUI-043 | **FIXED 2026-08-13** | parity-bug | S | ~~Word motion and Ctrl+W are not paste-marker atomic — one Ctrl+W after a large paste orphans the marker and drops the paste~~ |
| ~~TUI-027~~ | ~~**critical**~~ **CLOSED 2026-08-14** | not-ported | M | `/tree` has no text search, its four action keys are the characters pi types *into* that search, and the resulting label edit is persisted to the session JSONL — **CLOSED 2026-08-14**: sweep 1 — `/tree` gained a real text search, and two corrections to the item are recorded: (1) the `keymap.rs:908-915` offsets for `TreeKeymap::default` have moved; (2) the Fix asks for the digit-filter arm to be REPLACED, which is right, but does not say that `FilterMode::from_digit` becomes dead and must be deleted — upstream has no digit arm anywhere in `handleInput`. pi's help row and the standing `Type to search:` line are part of the surface and were ported with the search. |
| ~~TUI-031~~ | ~~**high**~~ **CLOSED 2026-08-14** | not-ported | M | A prompt typed during compaction is sent immediately instead of queued — **CLOSED 2026-08-14**: sweep 1. |
| ~~TUI-045~~ | ~~**high**~~ **CLOSED 2026-08-14** | not-ported | M | An escape sequence split at the ESC byte across `read(2)` boundaries is not reassembled — a spurious `Escape` aborts the turn and the tail is typed as text — **observed 2026-08-13, raised from medium** — **CLOSED 2026-08-14**: sweep 1 — FIXED at HEAD by commit c8c86bc (`src/pending_messages.rs`; `dispatch_submission` no longer calls `push_user`). Both the status-table row and the item body saying still-open/regressed were stale. |
| ~~TUI-016~~ | ~~**high**~~ **CLOSED 2026-08-14** | parity-bug | M | A queued message is echoed into the transcript as if delivered, and has no queue surface at all — **observed 2026-08-13, retitled and raised from medium** — **CLOSED 2026-08-14**: sweep 1 — FIXED at HEAD by commit c8c86bc (`src/pending_messages.rs`; `dispatch_submission` no longer calls `push_user`). Both the status-table row and the item body saying still-open/regressed were stale. |
| ~~TUI-052~~ | ~~**high**~~ **CLOSED 2026-08-14** | parity-bug | S | A queued message dequeued by Escape stays in the transcript forever as a phantom user message that was never sent — **new, observed 2026-08-13** — **CLOSED 2026-08-14**: sweep 1 — FIXED at HEAD by commit c8c86bc (`src/pending_messages.rs`; `dispatch_submission` no longer calls `push_user`). Both the status-table row and the item body saying still-open/regressed were stale. |
| TUI-053 | **FIXED 2026-08-13** | parity-bug | S | ~~`Ctrl+-` (`editor.undo`) is unreachable from any terminal without the kitty keyboard protocol — pi maps the legacy `0x1F` byte, cyrup does not~~ — fixed by porting `keys.ts:1275-1281`; still wants a live non-kitty run to close by hand |
| ~~TUI-054~~ | ~~**high**~~ **CLOSED 2026-08-14** | parity-bug | S | A failed or aborted compaction is announced to the user as "compaction complete" — `CompactionEnd`'s `aborted`/`error_message` are destructured away — **new, observed 2026-08-13** — **CLOSED 2026-08-14**: sweep 1 — with the residual named: cyrup's `/compact` returns a `CompactOutcome` that `apply_compact_outcome` renders on the COMMAND path, whereas pi's `handleCompactCommand` (:6030-6038) renders nothing and the event is upstream's only renderer. So the event arm handles the automatic reasons only, and a MANUAL abort still reads `compact error: …` where pi reads `Compaction cancelled`. **That residual is folded into SESS-040 rather than left dangling here.** |
| ~~TUI-055~~ | ~~**high**~~ **CLOSED 2026-08-14** | parity-bug | M | No status indicator renders for the entire duration of a compaction — the screen is blank for 10–20 s — **new, observed 2026-08-13** — **CLOSED 2026-08-14**: sweep 1 — FIXED at HEAD by commit c8c86bc (`src/pending_messages.rs`; `dispatch_submission` no longer calls `push_user`). Both the status-table row and the item body saying still-open/regressed were stale. |
| TUI-004 | medium | upstream-drift | M | No live colour-scheme sync; `/reload` does not re-apply themes |
| ~~TUI-005~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | S | Escape branches: bash-mode clear missing; bash child killed while streaming — **CLOSED 2026-08-14**: sweep 1 — the fix also required porting pi's compaction Escape rebind (interactive-mode.ts:3080-3086 / :3094-3097), which no TUI item filed: once the chain is exclusive, `isStreaming` is false during a compaction, so Escape would otherwise fall to the empty-editor branch and abort nothing. Overlaps SESS-040 — 040 must not double-book it. |
| ~~TUI-006~~ | ~~medium~~ **CLOSED 2026-09-04** | not-ported | M | ~~`[Extension issues]` renders 2 of pi's 4 diagnostic sources~~ — all four diagnostic sections (`Skill conflicts`, `Prompt conflicts`, `Extension issues`, `Theme conflicts`) are pushed by `push_diagnostics` calls at `crates/cyrup-tui/src/startup.rs:241-244`. |
| ~~TUI-008~~ | ~~medium~~ **CLOSED 2026-09-04** | not-ported | M | ~~Seven upstream global keybinding ids are unbound~~ — all seven now resolve: `Action::ModelSelect`, `ThinkingToggle`, `MessageCopy` and the rest are wired in `crates/cyrup-tui/src/app/input.rs:426` onward, under a comment naming the item and the exact `interactive-mode.ts:2608-2618` ids. |
| ~~TUI-009~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | S | Double-Escape → tree/fork never implemented although `doubleEscapeAction` ships in `/settings` — **CLOSED 2026-08-14**: sweep 1. |
| ~~TUI-012~~ | ~~medium~~ **CLOSED 2026-09-04** | not-ported | M | ~~No argument autocomplete for `/model <prefix>` or `/login <prefix>`~~ — both commands now carry `ArgumentCompleter::Models` / `ArgumentCompleter::LoginProviders` (`crates/cyrup-tui/src/commands.rs:143-149`, `:168-174`), consumed by `autocomplete.rs:296-309`. |
| ~~TUI-014~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | M | Extension widgets (`ui.setWidget`) now reach the TUI and are stored where nothing renders them — **CLOSED 2026-08-14**: sweep 1 — the CYRUP-DELTA the item did not anticipate is recorded: cyrup's WIT collapsed pi's three-argument `setWidget(key, content, options)` into one opaque JSON payload. **Superseded in sweep 2 — EXT-047 re-signed the import to pi's three arguments, so the widget now arrives keyed, and the item's premise "there is no key to map by" is doubly retired.** |
| ~~TUI-015~~ | ~~medium~~ **CLOSED 2026-09-04** | cyrup-original | M | ~~No render coalescing — one draw per streaming event, no frame budget~~ — the coalescing half closed 2026-08-19 (see history); **the remaining time-floor half is now also closed**: `crates/cyrup-tui/src/app/frames.rs:10-11` defines `MIN_RENDER_INTERVAL = Duration::from_millis(16)` citing pi's `MIN_RENDER_INTERVAL_MS` (`tui.ts:343`), gated at `:64` and consumed by the run loop (`app/run.rs:322`), with input still preempting the cap. |
| ~~TUI-017~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | Attachment image strip: rasterizes without a protocol, invented placeholder, no 60-cell cap — **CLOSED 2026-08-14**: sweep 1. |
| ~~TUI-028~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | Editor/input keybinding ids use an `editor.*` namespace upstream abandoned — 24 ids inert — **CLOSED 2026-08-14**: sweep 1 — one deliberate deviation from the Fix: `app.pageUp`/`app.pageDown` were NOT folded onto the editor map, because `load_keybindings_json` fans one JSON document out to six maps, so accepting the id in both would make a single config entry rebind two different actions. They stay as cyrup-original ids on the global map and are listed as a new gap. The dependency line ("Must land after CFG-048") is satisfied by keeping the `editor.*` spellings as ALIASES rather than renaming them. **Sweep 2 (area 05) correction: this item's upstream cites are wrong — `keybindings.ts:208-270` and `migrateKeybindingsConfig (:294-311)` should be `:209-269` and `:289-309`, identical at both tags — and TUI-028 must NOT delete the `editor.*` arms of `EditorAction::from_id`, which are what keeps shipped-cyrup configs working.** |
| ~~TUI-029~~ | ~~medium~~ **CLOSED 2026-09-04** | not-ported | M | ~~Extension autocomplete providers are never consulted by the interactive editor~~ — `ArgumentCompleter::Extension` reaches the interactive editor via `autocomplete.rs:296-315`, resolved eagerly at catalog-build time from `commands.rs:570-594`'s `argumentCompletions` key. |
| ~~TUI-030~~ | ~~medium~~ **CLOSED 2026-09-04, one residual re-classed as designed** | not-ported | L | ~~Nine `ExtensionUIContext` methods have no cyrup counterpart at all~~ — eight of nine landed in earlier passes (see `## Status of every item…`); the ninth pair, `setEditorComponent`/`getEditorComponent`, is now a documented, reasoned non-port rather than an open gap: `crates/cyrup-ext/src/lib.rs:54-64` states the Component Model cannot hand a guest three live host objects and re-enter a returned object on every keystroke, independent of any ADR citation, and the value-shaped alternative (`get-editor-text`/`set-editor-text`/`paste-editor-text`) already ships. |
| ~~TUI-032~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | S | `/settings` is missing the `Warnings` and `Thinking level` submenus — **CLOSED 2026-08-14**: sweep 1. |
| ~~TUI-033~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | M | `ui.setHeader` / `ui.setFooter` are delivered to the TUI and dropped into fields nothing renders — **CLOSED 2026-08-14**: sweep 1 — CYRUP-DELTA recorded: pi restores the built-in when the FACTORY is `undefined`; cyrup's WIT signature is `set-header(content: string)` with no `undefined`, so the empty string carries "restore the built-in". |
| ~~TUI-034~~ | ~~medium~~ **CLOSED 2026-09-04** | upstream-drift | L | ~~No markdown-transformer hook — extension transformers and pi's Mermaid renderer both absent~~ — `App::apply_markdown_transformers` (`crates/cyrup-tui/src/app/events.rs:102-140`) calls `ExtensionHost::transform_markdown` from three real sites (`session_bind.rs:214`, `events.rs:75,102`), and a real Mermaid renderer (the `mermaid-text` crate) ships in `markdown/mermaid.rs`, live while streaming, gated by the `markdown.mermaid` setting. |
| ~~TUI-037~~ | ~~medium~~ **CLOSED 2026-09-04** | not-ported | S | ~~`/reload` never persists an implicitly-granted project trust~~ — landed in `0e8c62fa`. `App::maybe_save_implicit_project_trust` (`crates/cyrup-tui/src/app/reload_trust.rs:126`) is the shell of pi's `maybeSaveImplicitProjectTrustAfterReload` (`interactive-mode.ts:4921-4941` @v0.84.4) over the pure `implicit_trust_after_reload` (`:85-102`, outcomes `ImplicitTrustReload::{Keep,Disarm,Persist}` `:64`); the `/reload` arm calls it (`app/execute_session.rs:279`) and selects pi's `; saved project trust` status variant (`:299`, `interactive-mode.ts:6000-6003`); the host arms `autoTrustOnReloadCwd` at the composition root (`crates/cyrup/src/main.rs:672-677`, pi `main.ts:701-704`) → `run_interactive` → `App::set_auto_trust_on_reload_cwd` (`interactive.rs:346`); the store failure is pi's `Warning: Could not save project trust after reload: …`, carried post-swap by `LifecycleEffects::warning` (`app/outcome.rs:104`, `app/channels.rs:181`). **[CYRUP-DELTA]** the write runs BEFORE the rebuild is dispatched, not after as pi's does: cyrup's `/reload` rebuilds the session through the factory and re-decides trust from the store, where pi's `AgentSession.reload` preserves `SettingsManager.projectTrusted` (`resource-loader.ts:404`); the inputs are the same, the store ends the same, and the rebuilt session reads the saved `true` back — the one difference is a reload that then FAILS has already written the entry. Tests: `src/app/reload_trust.rs` (4 decision-table cases) and `src/tests/reload_implicit_trust.rs` (5 App tests through the real runtime + `trust.json` store; `reload_persists_an_implicitly_granted_project_trust`, `…_disarms_without_writing` and `a_store_failure_warns_and_keeps_the_plain_status` were RED against the unwired arm). Closes `TUI-025`'s last residual (the `; saved project trust` variant). |
| TUI-044 | **FIXED 2026-08-13** | parity-bug | S | ~~`undo()` discards the snapshot's cursor column — `Snapshot::col` is written and never read~~ |
| TUI-046 | ~~medium~~ **low** | parity-bug | M | ~~cyrup pushes Kitty keyboard flag 1, pi pushes 7 — and neither guard flag 7 requires exists, so raising it alone would duplicate characters and leak CSI-u text~~ — **PARTIALLY FIXED 2026-09-04 at `8bb0d22f`.** The three push sites are now one: `crate::keyboard_protocol::push_flags()` writing `DESIRED_FLAGS` = `DISAMBIGUATE_ESCAPE_CODES \| REPORT_ALTERNATE_KEYS` (`\x1b[>5u`, pi's `\x1b[>7u` minus bit 2). Alternate keys was the half with a user-visible effect (a shifted key on a non-US layout now resolves to the character that layout produces, `crossterm-0.29.0/src/event/sys/unix/parse.rs:597-606`). `REPORT_EVENT_TYPES` is withheld as an argued `[CYRUP-DELTA]` (`keyboard_protocol.rs` module docs): it buys cyrup nothing — `map_event_on` discards every `Release` — and both guards it requires filter RAW BYTES, which cyrup's seam is below. `drain.rs:11-16` corrected. **Stays open (low) for one residual only: the `pendingKittyPrintableCodepoint` dedup, unportable while cyrup filters crossterm events rather than bytes.** |
| ~~TUI-051~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | `/reload` never re-reads `keybindings.json`, while the command's help text and its in-source comment both claim it does — **CLOSED 2026-08-14**: sweep 1. |
| TUI-058 | **FIXED 2026-08-13** | parity-bug | S | ~~Deleting a paste marker does not renumber the pastes that follow it — ids diverge from pi's for the life of the session~~ — **new, found 2026-08-13** |
| TUI-059 | **FIXED 2026-08-13** | parity-bug | S | ~~Only Left/Right clear `lastAction`, so a kill survives every other motion and the next kill accumulates into the same ring entry~~ — **new, found 2026-08-13** |
| ~~TUI-019~~ | ~~medium~~ **CLOSED 2026-09-04** | upstream-drift | L | ~~No alt-screen UI mode, mouse, scrollbars, prompt navigation~~ — `cyrup --tui-mode fullscreen` ships the full renderer tree at `crates/cyrup-tui/src/altscreen/` (thirteen modules: terminal setup, mouse/focus, scroll + scrollbar, wheel, drag, selection/clipboard, the eight `tui.altScreen.*` bindings, prompt navigation, flash, image lifecycle, exit repaint), wired from `crates/cyrup/src/interactive.rs:216` and `cli/args.rs:184-185`. Driven under a live pty by the landing commit (`dbcf59a`), which is closure-adjacent evidence rather than this pass's own live run — no real-terminal re-check was done here; if the alt-screen path regresses it needs a live re-observation, not a static one. |
| ~~TUI-N01~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | Tool-result images rasterize on terminals with no image protocol — **CLOSED 2026-08-14**: sweep 1. |
| ~~TUI-N02~~ | ~~medium~~ **CLOSED 2026-09-04** | not-ported | S | `/reload` did not re-emit the loaded-resources / diagnostics panel — **CLOSED 2026-09-04 at `605f483c`.** `push_loaded_resources` had exactly one production call site (the boot path), so every session swap left the panel describing the replaced session. The panel is now a pure projection of the session it describes — **`StartupReport::from_session(&AgentSession, verbose)`** (`crates/cyrup-tui/src/startup.rs:178`; this row first named it `StartupResourcesPanel::from_session`, a symbol that never existed — corrected 2026-09-05) — reached through `App::push_session_loaded_resources` (`app/session_bind.rs:507-511`) and pushed by the `session_swapped` arm at `app/run_arms.rs:338`, after `install_extension_shortcuts` (`:322`, so EXT-039's reserved-key refusals are recorded before the panel folds them in) and before the replay, the same position the boot path uses (`crates/cyrup/src/interactive.rs:256`, `:282`); `crates/cyrup/src/interactive.rs` drops its duplicate assembly (its private `build_startup_report`, now gone) and calls the shared projection. Upstream at v0.84.4: pi emits the panel from `bindCurrentSessionExtensions` (`interactive-mode.ts:1982`, reached on boot AND on every replacement through `rebindCurrentSession` / the runtime's `setRebindSession` hook at `:576-578`) and again from `handleReloadCommand` (`:5991-5994`), both with the identical `{force: false, showDiagnosticsWhenQuiet: true}`, ungated by swap reason; `setupExtensionShortcuts` precedes `showLoadedResources` at both sites (`:1981-1982`, `:5990-5991`). Deriving the panel from the session removes the "nine other things and never this one" failure the row named, because there is no separate refresh step left to forget. Three new tui tests (`the_panel_is_derivable_from_a_session_and_survives_quiet_startup`, `set_verbose_startup_overrides_quiet_startup_at_the_session_seam`, `the_session_swap_arm_pushes_the_panel_after_the_shortcuts_and_before_the_replay`); all red before the change, and structurally so — none of `push_session_loaded_resources`, `set_verbose_startup` or `StartupReport::from_session` existed, so they could not compile. cyrup-tui 1378 passing, cyrup-session-svc 330, cyrup 236; clippy and rustdoc clean on all three. **Scope defect, recorded rather than hidden:** the same commit also landed EXT-003's `with_wasm` residual (`crates/cyrup-session-svc/src/builder.rs`, plus its test `a_dead_pre_trust_wasm_runtime_does_not_discard_native_votes`) without naming EXT-003 in the subject or body; that row is struck separately in `06-cyrup-ext.md` and the fix belongs to it, not here. **Two recorded deltas from pi, neither a regression.** (a) pi re-renders into a `loadedResourcesContainer` it `clear()`s first (`interactive-mode.ts:1699`) and which is pinned above `chatContainer` (`:594-596`); cyrup's committed entries live in the terminal's own scrollback and cannot be re-rendered, so a second swap APPENDS a second panel where pi replaces one — documented on `push_session_loaded_resources`'s rustdoc. (b) Because the push at `run_arms.rs:338` precedes the arm's re-entrancy generation guard (`:370`), a swap superseded mid-await appends a panel for the abandoned session and the newer generation appends its own; pi cannot show this because both of its emit paths clear the container first. Both follow from (a) — linear scrollback — and are the same class of divergence TUI-N07 already tracks for the replay itself. |
| ~~TUI-N03~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | A theme chosen in `/settings` is applied live but never persisted — **CLOSED 2026-08-14**: sweep 1. |
| ~~TUI-N04~~ | ~~medium~~ **CLOSED 2026-09-04** | not-ported | S | ~~The untrusted-project warning banner is never rendered at startup~~ — it is: `App::render_project_trust_warning_if_needed` (`crates/cyrup-tui/src/app/session_bind.rs:100-129`), called from `app/run_arms.rs:119` (boot) and `:289` (post-replay). This was already flagged false by the 2026-08-19 round-2 note but never struck in this table until now. |
| ~~TUI-002~~ | ~~low~~ **CLOSED 2026-09-04** | parity-bug | M | ~~Thinking blocks: fold-ordering and the visible-content spacer (markdown half closed)~~ — `transcript/cache.rs:100-138` renders the live reasoning block ABOVE the answer with pi's `hasVisibleContentAfter` spacer (`:134-137`, a blank only when visible content follows), and empty/whitespace-only reasoning renders nothing on either leg, matching `assistant-message.ts:122-137`. This was already flagged false for the live leg by the 2026-08-19 round-2 note but never struck here until now. |
| TUI-003 | low | parity-bug | S | Replay omits the compaction-count status — **re-verified 2026-09-04, unchanged: `rg 'Session compacted\|compaction_count' crates/ -g '*.rs'` is still zero hits.** |
| ~~TUI-010~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Ctrl+O pushes no `Tool output: …` status (committed-entry half closed) — **CLOSED 2026-08-14**: sweep 1. |
| TUI-011 | low | not-ported | M | `/changelog` is a hardcoded stub; no "What's New" startup notice — **re-verified 2026-09-04, unchanged: `app/submit.rs:117-118` is still the verbatim `push_block("What's New", "No changelog entries found.")` stub.** |
| ~~TUI-013~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Quoted paths with spaces break `@`-mention autocomplete — **CLOSED 2026-08-14**: sweep 1. |
| TUI-018 | low | not-ported | M | Startup header has no logo/version line and no expanded body — **re-verified 2026-09-04, unchanged: `chrome.rs:126-130` still states outright that no logo is drawn.** |
| ~~TUI-020~~ | ~~low~~ **CLOSED 2026-09-04** | not-ported | S | ~~OSC-8 hyperlinks: capability now consulted, still never emitted~~ — `crates/cyrup-tui/src/osc.rs` is a real emitter now: the escape goes into the `Buffer` cell (not the `Span`, which `ratatui-core` would filter as a control char) with `CellDiffOption::ForcedWidth` restoring the true column count, and `inject` (`osc.rs:145`) writes it into the frame before diffing. The module's own header names this "TUI-020's first landing". |
| ~~TUI-021~~ | ~~low~~ **CLOSED 2026-09-04** | upstream-drift | M | ~~Cache-miss notices not implemented~~ — `App::maybe_show_cache_miss_notice`-equivalent path lands at `app/events.rs:231-239` (`push_cache_miss_notice`) and `app/events_fold.rs:307-309`, gated on the live `show_cache_miss_notices` setting cache the same way pi's `getShowCacheMissNotices()` is. |
| ~~TUI-025~~ | ~~low~~ **CLOSED 2026-09-04** | stale-port | S | Slash-command metadata one baseline behind — **PARTIALLY CLOSED 2026-08-14**: sweep 1 — the three `commands.rs` literals and the `/reload` status sentence are done. ~~**RESIDUAL: the `; saved project trust` variant depends on TUI-037, whose write lives in `crates/cyrup`.** — **re-verified 2026-09-04, residual unchanged (TUI-037 still open).**~~ **CLOSED 2026-09-04 (batch-2 ledger audit): the last residual landed with `TUI-037` at `0e8c62fa` — the `/reload` arm now pushes pi's trust variant `Reloaded keybindings, extensions, skills, prompts, themes, and context files; saved project trust` (`crates/cyrup-tui/src/app/execute_session.rs:299`; pi `interactive-mode.ts:6000-6003` @v0.84.4), pinned by `crates/cyrup-tui/src/tests/reload_implicit_trust.rs` (see the `TUI-037` row). Nothing of this item remains open.** |
| ~~TUI-035~~ | ~~low~~ **CLOSED 2026-08-14** | upstream-drift | S | `tui.editor.historyPrevious` / `historyNext` are unbound — **CLOSED 2026-08-14**: sweep 1. |
| ~~TUI-036~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `Show images` / `Image width` rows are offered on terminals with no image protocol — **CLOSED 2026-08-14**: sweep 1. |
| ~~TUI-038~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Ctrl+O is an if/else in cyrup and a fan-out upstream — a live bash block blocks tool expansion — **CLOSED 2026-08-14**: sweep 1. |
| ~~TUI-039~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Terminal geometry never falls back to `$COLUMNS` / `$LINES` — **CLOSED 2026-08-14**: sweep 1 — but its Verify ("App test with a backend reporting no size and COLUMNS=200 set") CANNOT live in `crates/cyrup-tui/src/`: `std::env::set_var` is unsafe in edition 2024 and the crate is `#![forbid(unsafe_code)]`. It belongs in `crates/cyrup-tui/tests/`, next to `experimental_marker.rs`, which exists for exactly this reason. Same note applies to TUI-041's env half. |
| TUI-040 | low | not-ported | S | No `PI_TUI_WRITE_LOG` equivalent — no escape-sequence write log — **re-verified 2026-09-04, unchanged: `rg 'WRITE_LOG\|write_log' crates/cyrup-tui/src` is zero.** |
| ~~TUI-041~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `/settings` shows env-overridden rows with the wrong value — **CLOSED 2026-08-14**: sweep 1. |
| TUI-047 | low | not-ported | M | A late or unsolicited DCS/APC frame is shredded into ~20 typed characters — `stray_reply.rs` recognises only OSC 11 |
| TUI-048 | low — **partially fixed 2026-08-13** | parity-bug | M | Word navigation classifies by character class instead of Unicode word segmentation — CJK word motion jumps whole runs. Class-run motion replaced by UAX#29 + pi's punctuation sub-boundaries; the ICU dictionary pass for unspaced scripts remains |
| TUI-049 | **FIXED 2026-08-13** | parity-bug | S | ~~`marker_at` accepts any text between `[paste #N ` and `]`, expanding markers pi's regex rejects~~ |
| TUI-050 | low | not-ported | S | An 8-bit meta byte is silently dropped instead of being converted to `ESC` + char (depends on TUI-045) |
| TUI-060 | low | parity-bug | M | The wrap / visual-line map is not paste-marker aware, so a marker can be torn across visual rows — pi passes a marker-merged `preSegmented` to `wordWrapLine` — **new, found 2026-08-13** — **re-verified 2026-09-04, unchanged: `editor/wrap.rs`'s `word_wrap_line` still takes a plain `&[char]`; a nearby CYRUP-DELTA note covers CJK/emoji grapheme width only, not paste-marker atomicity across a wrap boundary.** |
| ~~TUI-061~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `set_text` collapses pi's `setText` and `setTextInternal`, so a programmatic buffer replacement leaves the paste registry live and is not undoable — **new, found 2026-08-13** — **CLOSED 2026-08-14**: sweep 1. |
| TUI-N05 | low | parity-bug | S | Extension shortcuts can never override a built-in key; no conflict reported — **not independently re-checked this pass; left open.** |
| TUI-N06 | low | parity-bug | L | `Entry::Thinking` freezes hide/show at commit time — **re-verified 2026-09-04, still open BY DESIGN and cited as such at three call sites** (`status.rs:114`, `thinking_selector.rs:77`, `tests/footer_chrome_fidelity.rs:938`); `fda714d`'s `hideThinkingBlock` visibility fix explicitly left this residual untouched. |
| TUI-N07 | low | parity-bug | L | Mid-session `/resume` cannot erase the previous session's scrollback — **not independently re-checked this pass; left open.** |
| ~~TUI-N08~~ | ~~low~~ **CLOSED 2026-08-14** | test-defect | S | `tests/image.rs` pins the invented `🖼` placeholder and the rasterize-anyway fallback — **CLOSED 2026-08-14**: sweep 1 — the proposed remedy (annotate + add an `#[ignore]`d companion) was NOT taken: TUI-017 landed in the same pass, so the assertions were retargeted to pi's real `[Image: …]` format outright and the `#[ignore]` is unnecessary. |
| ~~TUI-N09~~ | ~~low~~ **CLOSED 2026-08-14** | test-defect | S | `extension_dialog_countdown` asserts an exact countdown it cannot control — **CLOSED 2026-08-14**: sweep 1. |
| TUI-N10 | low | test-defect | S | `bash_overlay`'s two hotkeys tests hard-code the non-macOS `alt` spelling — **fixed this pass** |
| ~~TUI-N11~~ | ~~medium~~ **CLOSED 2026-09-04** | test-defect | S | ~~`m7_inline_formatting_survives_inside_a_table_cell` asserts a property of the ambient `TERM_PROGRAM`~~ — the fix landed in the pass that filed the item ("fixed this pass"), but the severity cell was never struck, so `scripts/count_open_items.py` kept counting it open (its CLOSED-ROW DETECTION reads the severity strike and nothing else). **Settled 2026-09-04 by re-measurement, not by trusting the note.** Both link arms of the test now render through the explicit-capability entry point — `crates/cyrup-tui/src/tests/markdown.rs:2045` `render_markdown_with_hyperlinks(linked, 40, &theme, false)` and `:2071` the capable mirror `(…, true)` — so neither reads `crate::image::hyperlinks_supported()` (`markdown/mod.rs:148`, the ambient path `render_markdown` takes). `cargo nextest run -p cyrup-tui -E 'test(tests::markdown::)'` is **51/51 under all four terminal identities** — `TERM_PROGRAM=ghostty` with `GHOSTTY_RESOURCES_DIR` set, `=vscode`, `=iTerm.app`, and with those vars scrubbed — and the whole crate is 1375/1375. The pin is load-bearing, not decorative: reverting that one arm to the ambient `render_markdown(linked, 40, &theme)` is RED under the ghostty identity (`1 test run: 0 passed, 1 failed`) and green scrubbed, the exact split the item recorded; revert restored. `f061bf35` refreshed the arm's citations to the ADR-0006 target (pi `packages/tui/src/components/markdown.ts:696-707` fallback and `:692-695` OSC-8 @v0.84.4, `packages/tui/test/markdown.test.ts:597-598` @v0.84.4 for upstream's own "Pin to no-hyperlinks…" line) and corrected a drifted in-file cross-reference; no production line changed. |
| ~~TUI-N12~~ | ~~low~~ **CLOSED 2026-08-14** | not-ported | S | No `setCapabilities` / `resetCapabilitiesCache` seam; only markdown can drive both OSC-8 branches — **CLOSED 2026-08-14**: sweep 1 — the secondary latent hazard ("if any earlier caller latches the lock first that seed is silently discarded for the process") is closed as a consequence: `set_capabilities` replaces rather than first-writer-wins, and `App::detect_image_support` seeds the whole record. |
| ~~TUI-N13~~ | ~~high~~ **CLOSED 2026-08-14** | test-defect | S | `a_live_bash_run_names_its_spool_file` parses one wrapped line, so it is red wherever `TMPDIR` is long — **fixed this pass** — **CLOSED 2026-08-14**: closed pre-sweep (`a_live_bash_run_names_its_spool_file` parsed one wrapped line, red wherever `TMPDIR` is long). |
| TUI-S02 | low | not-ported | S | No dead-terminal (EIO/EPIPE/ENOTCONN) emergency exit path (panic-hook half closed) — **not independently re-checked this pass; no `DEAD_TERMINAL_ERROR_CODES`/`emergencyTerminalExit` counterpart turned up in the symbol sweep, left open rather than guessed at.** |
| TUI-S10 | low | not-ported | S | Shift+Ctrl+D global debug chord absent — `/debug` reachable only by typing into the editor — **re-verified 2026-09-04, unchanged: no `on_debug`/reserved-chord handling in `keymap.rs`.** |
| TUI-056 | low | parity-bug | S | The context-usage meter resets to `0.0%` after an aborted turn while the conversation is still in the transcript — **new, observed 2026-08-13** — **not independently re-checked this pass (needs a live aborted-turn observation, not a static read); left open.** |
| TUI-057 | low | port-divergence | M | Slash-command palette submission is inconsistent — sometimes one Enter, sometimes two, sometimes a trailing space suppresses it — **new, observed 2026-08-13, low confidence** — **not independently re-checked this pass (originally a live-use finding, and this pass had no pty); left open.** |
| TUI-062 | low — **PARTIALLY CLOSED 2026-08-14** | parity-bug | S | `showWarning`'s `Warning: ` prefix lives INSIDE pi's function but OUTSIDE cyrup's renderer, making it a per-caller obligation that one of three callers had dropped — **FILED AND PARTIALLY CLOSED 2026-08-14 (sweep 6, found while fixing `CFG-051`; the work was done from another partition and the design half belongs to whoever owns cyrup-tui).** pi's `showWarning` builds `Warning: ${warningMessage}` inside the function (`interactive-mode.ts:3885-3889` @v0.83.0), while cyrup's `Entry::Warning` renders its text **verbatim**, so every cyrup caller must supply the prefix. Two of three did (`app/extension_ui.rs:307`, `app/run_arms.rs:536`; the trust banner at `app/session_bind.rs:135` is a deliberate no-prefix case, documented in place); `main.rs`'s `modelFallbackMessage` push did not, so **a credential-less first run rendered a bare "No models available…" where pi renders "Warning: No models available…"**. That call site is fixed. **RESIDUAL, two halves, both cyrup-tui's:** (a) the in-tree citation at `crates/cyrup-tui/src/transcript.rs:2965-2971` cites `interactive-mode.ts:3956-3960` for `showWarning`; at v0.83.0 it is `:3885-3889` — a wrong cite of the version-lag kind, uncorrected; (b) the DESIGN question — prefix in the renderer (pi's shape, one place) vs. at each caller (cyrup's current shape, N places, already broken once). **This is the general class "a mechanism ported one level up or down, so the invariant quietly becomes optional"** — see the JS→Rust register in `00-residual-ledger.md`. |
| ~~TUI-063~~ | ~~low~~ **CLOSED 2026-09-04** | cyrup-original | S | ~~`CYRUP_SHARE_VIEWER_URL` is advertised in `cyrup --help` and read by nothing~~ — `app/share.rs:30-46` `share_viewer_url` reads `ENV_SHARE_VIEWER_URL` and the module doc names itself "the ONLY consumer of `ENV_SHARE_VIEWER_URL`. Before it existed, `/share` printed the raw gist URL and the advertised variable was inert." |
| TUI-064 | low | not-ported | M | The attached-image strip has no production callers — **filed 2026-08-14**; `attach_image`/`attach_image_path` are called only from three unit tests, so the strip is dead surface and `TUI-017` cannot be observed end to end. — **re-verified 2026-09-04, unchanged: `rg '\.attach_image\(' crates/cyrup-tui/src` still returns only `tests/image.rs`.** |
| TUI-065 | low | cyrup-original | S | `app.pageUp` / `app.pageDown` are cyrup-invented ids, and each now resolves in **two** keymaps at once, so one config entry rebinds two actions — **filed 2026-08-14** (sweep 10, keybinding-id surface). TUI-028's closure said these "are listed as a new gap"; **no row was ever added** — this is it. — **not independently re-checked this pass; left open.** |
| TUI-066 | low | cyrup-original | S | The cyrup-only keybinding-id and key-spec vocabulary TUI-028's closure deliberately preserved — 5 `tui.autocomplete.*`, 19 bare `editor.*` aliases, 10 `Key::parse` tokens pi's `KeyId` has no word for — is tracked nowhere — **filed 2026-08-14** — **not independently re-checked this pass; left open.** |
| ~~TUI-067~~ | ~~low~~ **CLOSED 2026-09-04** | parity-bug | S | ~~`tui.input.copy` migrates correctly and is then silently dropped by `merge_entries`: `EditorAction::from_id` has no arm, so pi's one "do not consume this key" id is inert~~ — `EditorAction::PassThrough` now exists (`keymap.rs:354`), `from_id` maps `"tui.input.copy" \| "copy" => E::PassThrough` (`:435`), and `handle_key` declines the key ahead of `apply_editor_action` (`editor/keys.rs:49,349`). Landed in `fda714d` ("give `tui.input.copy` a destination"); verified against the actual code, not the commit message. |
| ~~TUI-068~~ | ~~medium~~ **CLOSED 2026-09-04** | not-ported | S | ~~`app.session.deleteNoninvasive` is unbound **and** unbindable — Ctrl+Backspace in `/resume` hits a modifier-blind `Backspace` catch-all and does nothing — **filed 2026-08-14**; the chord parses, so this is a missing destination, not a dead spec — **2026-08-15 (TUI-092 §4): `TUI-088` was checked as a possible THIRD instance of this class and is confirmed NOT one** — Ctrl+C is bound (`keymap.rs:656`), id-resolvable (`:268`) and wired (`app/input.rs:219-231`) at HEAD — so the class stays this row and `TUI-067`, and is **not** re-swept. **Shared root cause with `TUI-067`, stated once:** `merge_entries` silently skips any id `from_id` does not know — `crates/cyrup-tui/src/keymap.rs:128`, `let Some(action) = from_id(&id) else { continue };`, which is upstream's deliberate `if (!(keybinding in this.definitions)) continue` (`packages/tui/src/keybindings.ts:172-179`) — so an id present in a `Default` table but missing from the matching `from_id` is **unbindable and reports nothing**, not even a `KeybindingIssue`. — **re-verified 2026-09-04, unchanged: `rg 'deleteNoninvasive\|DeleteNoninvasive' crates/` is still zero.** |~~ — landed in `eacd771a`: `SessionAction::DeleteNoninvasive` (`crates/cyrup-tui/src/keymap.rs:1199`), the `"app.session.deleteNoninvasive"` arm in `SessionAction::from_id` (`:1211`), default `ctrl+backspace` in `SessionKeymap::default()` (`:1240`), and the handler arm in `SessionSelector::handle` step 4 (`crates/cyrup-tui/src/session_selector.rs:1017`) — empty query → `start_delete_confirmation_for_selected` (`:391`, pi `session-selector.ts:394-403`), non-empty query → forwarded to the search `Input` and re-filtered (pi `:590-600` @v0.84.4). Resolved ahead of the search-Input fallthrough (step 6), so the modifier no longer disappears. Pinned by `tui068_*` (4 in `session_selector.rs`, 1 in `tests/keymap.rs`), all RED before. **Residual (low, new):** pi's `startDeleteConfirmationForSelectedSession` refuses the CURRENT session (`session-selector.ts:398-401`, `Cannot delete the currently active session`); neither cyrup delete path carries that guard. |
| TUI-069 | **FIXED 2026-08-14** | parity-bug | S | ~~`/hotkeys` printed `Shift+Tab/Shift+Shift+Tab/Shift+Tab` for `app.thinking.cycle`, whose middle entry is not a chord and does not round-trip through `Key::parse`~~ |
| TUI-070 | **FIXED 2026-08-14** | parity-bug | S | ~~The page keys rendered as `Pageup` / `Pagedown` where pi renders `PageUp` / `PageDown`; masked inside `/tree` by the `pgup`/`pgdn` rewrite~~ |
| TUI-071 | low | port-divergence | S | Three platform-conditional upstream defaults are bound unconditionally: `app.clipboard.pasteImage` (both `ctrl+v` and `alt+v` everywhere), `app.suspend` (no win32 gate), `app.tree.foldOrUp`/`unfoldOrDown` (key order fixed at alt-first) — **filed 2026-08-14**; none carries the `CYRUP-DELTA` cite a forced difference requires — **re-verified 2026-09-04 for the `pasteImage` clause, unchanged: `keymap.rs:704-709` still binds `ctrl+v`/`alt+v` unconditionally on every platform (an explanatory comment was added, but no platform gate); the `app.suspend`/`foldOrUp` clauses were not independently re-checked.** |
| TUI-072 | low | upstream-drift | S | Four editor key sets carry their v0.84.1 extras (`ctrl+home`, `ctrl+end`, `ctrl+pageup`, `ctrl+pagedown`) against a v0.83.0 baseline, changing the `/hotkeys` cells — **filed 2026-08-14**; disclosed in-code, never in the ledger |
| TUI-073 | low | parity-bug | S | `clear` is a valid pi `KeyId` that `Key::parse` rejects with a generic parse failure — **filed 2026-08-14**; the residual of a chord-parse audit in which **all 58 upstream default key specs parse**, so no default chord is dead |
| TUI-074 | **FIXED 2026-08-14** | parity-bug | S | ~~Dispatch arity: cyrup accepted an argument on all 25 slash commands where pi accepts one on 6, so `/quit now` quit, `/copy that` copied and `/new session` started a new session instead of reaching the agent~~ — also records the registry-vs-if-chain mechanism substitution that caused it |
| ~~TUI-075~~ | ~~low~~ **CLOSED 2026-09-04** | parity-bug | S | ~~The `/` menu lists extension commands before prompt templates; pi lists builtins → prompts → extensions → skills~~ — `commands.rs:311-318` now reorders at the one interactive consumer (deliberately NOT at the catalog, which also serves the RPC `get_commands` payload in pi's RPC order), citing `interactive-mode.ts:625`'s exact ordering. |
| TUI-076 | low | parity-bug | S | The builtin-collision filter keys on `invocation_name`, so a suffixed duplicate (`model:1`) survives into the menu as a command `dispatch_names` will never route — **filed 2026-08-14**; the diagnostic half is TUI-006, do not double-book — **not independently re-checked this pass; left open.** |
| TUI-077 | low | parity-bug | S | A slash-argument context falls through to path completion (pi's slash branch is terminal), and the name/argument split uses Unicode whitespace where pi splits on a literal space — **filed 2026-08-14**; land with TUI-012 — **not independently re-checked this pass; left open.** |
| ~~TUI-078~~ | ~~low~~ **CLOSED 2026-08-17** | parity-bug | M | ~~A prompt template's `argument-hint` is parsed by cyrup-resources and dropped at the `slash_command_catalog` seam, so the `/` menu shows a bare description~~ — **filed 2026-08-14** — **CLOSED 2026-08-17** by `0b7c4f4` + `bae24f5` (the `CMDHINT_01` change; the feature it also shipped is `TUI-095`). Both halves of the Fix are at HEAD: the producer inserts the key spread-if-truthy (`cyrup-session-svc/src/session.rs:2635-2639`, rationale at `:2628-2634`) and the consumer reads it with pi's empty-string truthiness filter (`cyrup-tui/src/commands.rs:475-479`). Verify is satisfied twice — `crates/cyrup-tui/src/tests/commands.rs:248` (catalog row → `SlashCommand::argument_hint`) and `crates/cyrup-session-svc/src/tests/cmdhint01_argument_hint.rs:58`/`:82`/`:99` (the JSON key is emitted for a template with the hint, ABSENT without one, and never for a skill row). **ONE DELIBERATE DEVIATION, so the closure is complete-as-corrected rather than partial:** the "(and extension)" half of the Fix was declined with the upstream reason recorded in-source at `commands.rs:470-474` — pi's `interactive-mode.ts:691-698` forwards a COMPLETER to extension commands, not a hint, and `cyrup-ext/src/registry.rs:94-98` has no such field — so only `source:"prompt"` rows can produce a hint, which is upstream's behaviour. **The residual this row's own Fix asked for is NOT done:** it instructed "note in the RPC area that the field is a cyrup addition to `get_commands`", and `08-cyrup-session-svc-and-modes.md` has zero `argumentHint`/`CMDHINT` hits — routed there via `TUI-095`. |
| ~~TUI-079~~ | ~~low~~ **CLOSED 2026-09-04** | not-ported | S | ~~`/export` and `/import` take the whole remainder as the path; pi parses one quote-aware token (`getPathCommandArgument`), so `/export "my session.html"` writes a file with the quotes in its name~~ — `path_command_argument` (`commands.rs:458`) is a real quote-aware parser, wired at both call sites (`app/submit.rs:90-91`). This was already flagged false by the 2026-08-19 round-2 note but never struck here until now. |
| ~~TUI-080~~ | ~~low~~ **CLOSED 2026-09-04** | not-ported | S | ~~`/name` with no argument is a getter upstream; cyrup always prints usage, never reads the stored name, and echoes the INPUT rather than the normalized stored name~~ — `C::ShowName` (`app/execute_misc.rs:804`) reads and prints the stored session name. This was already flagged false by the 2026-08-19 round-2 note but never struck here until now. |
| ~~TUI-081~~ | ~~medium~~ **CLOSED 2026-09-04** | not-ported | S | ~~`/import` replaces the live session with no confirmation — a mistyped path destroys the in-flight conversation; the `cancelled` arm the code already has is unreachable~~ — **filed 2026-08-14** — **re-scoped 2026-09-04:** the `cancelled` arm was reachable only through an extension veto (`cyrup-session-svc/src/runtime.rs:697-704`), not a user confirmation — **CLOSED 2026-09-04 (`84b205a1`):** `/import <path>` now opens pi's `Import session` / `Replace current session with {path}?` Yes/No prompt (`SelectorKind::ImportConfirm`, `crates/cyrup-tui/src/app/execute_session.rs:575` `open_import_confirm`) BEFORE anything is touched; `No`/Escape push `Import cancelled` and drop the path; only `Yes` reaches `import_from_jsonl` (`:610` `dispatch_import`), whose captions are pi's `Session imported from: {path}` / `Import cancelled` (`interactive-mode.ts:6069-6082` @v0.84.4). Four app tests in `tests/import_confirm.rs`. |
| ~~TUI-082~~ | ~~medium~~ **CLOSED 2026-09-04** | parity-bug | S | ~~Bare `/export` writes no file — it dumps the raw HTML into the transcript where pi writes to the session directory and names the path~~ — bare `/export` now writes a file and reports the path (`app/execute_session.rs:84-104`), with one status string for both branches matching pi's `Session exported to: ${filePath}`. |
| ~~TUI-083~~ | ~~low~~ **CLOSED 2026-09-04, as a documented deliberate decision** | parity-bug | S | ~~`/quit`'s description is the literal `"Quit cyrup"` where pi templates `Quit ${APP_NAME}`; the templating mechanism is unported~~ — `commands.rs:187-192` now records this as a decision, not an oversight: cyrup has no config-name-override feature (pi's `piConfigName`) to template against, so there is nothing to template, and the comment states the line is the one to change if such an override is ever added. |
| ~~TUI-084~~ | ~~low~~ **CLOSED 2026-09-04** | parity-bug | S | ~~The argument-less usage strings for `/import` and `/name` diverge in wording and in severity channel (pi's error/warning → cyrup's neutral status)~~ — `/name` now uses `push_warning("Usage: /name <name>")` (`app/execute_misc.rs:800-807`, pi's exact string and channel) and `/import` uses `push_error("Usage: /import <path.jsonl>")` (`app/execute_session.rs:296-298`, matching pi's `showError`). |
| ~~TUI-085~~ | ~~low~~ **CLOSED 2026-09-04** | parity-bug | S | ~~A dynamic command with no `sourceInfo` is tagged `[t]`; pi leaves the description unprefixed~~ — `autocomplete_source_tag` (`commands.rs:425-437`) is `let info = source_info?;` — returns `None` (unprefixed) on no `sourceInfo`, matching pi's `if (!sourceTag) return description`. |
| ~~TUI-086~~ | ~~low~~ **CLOSED 2026-09-04** | cyrup-original | S | ~~`CommandSource::Builtin` is a fourth variant of an upstream three-value union (pi's builtins are a different type with no `source` field), and `autocomplete_source_tag` is public where pi's is a private method~~ — `autocomplete_source_tag` is now `pub(crate)` (`commands.rs:425`), not `pub`, with the reason stated in place; the extra `Builtin` variant is documented as a necessary consequence of cyrup unifying two upstream types into one struct (`commands.rs:19-24`), not an accidental divergence. |
| ~~TUI-087~~ | ~~low~~ **CLOSED 2026-09-04** | stale-port | S | ~~Eight upstream citations in `commands.rs` do not resolve at v0.83.0, including a **fabricated** `AgentSession::expand_slash_command` and a comment contradicted by the code six lines below it~~ — every cited spot now carries a correction in place (e.g. `commands.rs:197`, `:514-516`) naming what the old citation got wrong and what the right one is. |
| ~~TUI-088~~ | ~~**high**~~ **CLOSED 2026-08-15 — ALREADY IMPLEMENTED / MIS-DIAGNOSED** | ~~not-ported~~ *(mis-filed — ported at HEAD)* | M | ~~**Ctrl+C does not work in the running TUI**~~ — filed 2026-08-15 from live use — **CLOSED 2026-08-15 (TUI-092 §4): its premise, "there is no global Ctrl+C destination in `keymap.rs`", is FALSE at HEAD, and every layer was re-read line by line.** **Binding** — `crates/cyrup-tui/src/keymap.rs:656`, `(Key::ctrl('c'), Action::Clear)` in `Keymap::default()`. **Id** — `keymap.rs:268`, `"app.clear" => Some(Action::Clear)`, so it is rebindable from `keybindings.json` through `merge_json` (`:760`). **Destination** — `crates/cyrup-tui/src/app/input.rs:219-231` implements pi's `handleCtrlC` verbatim: a second press within 500 ms sets `should_quit` and returns `AppAction::Quit`, otherwise the editor is cleared and `last_sigint` stamped — including the asymmetry that the timestamp is updated **only** on the clear branch. **Reach** — `handle_input`'s `defer_to_editor` match (`crates/cyrup-tui/src/app/input.rs:63-80`) special-cases only `Interrupt`/`Quit`/`PageUp`/`PageDown`, so `Action::Clear` falls to `_ => false` (`app/input.rs:79`) and reaches `apply_action` unconditionally. **Affordance** — `crates/cyrup-tui/src/chrome.rs:80-90` renders `ctrl+c/ctrl+d — clear/exit` in the startup hint bar from the LIVE keymap. **Coverage** — `crates/cyrup-tui/src/tests/keymap.rs:75` and `crates/cyrup-tui/src/tests/keybindings.rs:200` both assert `action_for(ctrl+c) == Action::Clear`. **And it is 1:1 with upstream, so the fix this row asked for would have been a divergence, not a fix:** `pi/packages/coding-agent/src/core/keybindings.ts:66-68` binds `app.interrupt`→`escape`, `app.clear`→`ctrl+c`, `app.exit`→`ctrl+d`, and `modes/interactive/interactive-mode.ts:3797-3805` is `handleCtrlC` with no `isStreaming` branch anywhere — **Ctrl+C does NOT bind to interrupt upstream and never aborts a running turn** (aborting is Escape, `keymap.rs:657` → `app/input.rs:130-213`), and rebinding it would also destroy the double-tap exit `TUI-092` §3 depends on. **Mis-diagnosis cause, in one line:** the probe was `grep "Char('c')"`, but the chord table is built from the `Key::ctrl(..)` / `Key::plain(..)` constructors (`keymap.rs:654-696`), so a `KeyCode::Char` grep is structurally blind to **EVERY** global binding in this codebase — the correct probe is `grep "ctrl('c')"`. **The user-visible symptom this row recorded is real and is REASSIGNED TO `TUI-092`:** the key is bound, wired, tested and advertised, and it stops responding because the run loop is starved or wedged and can service NO input at all. **No keybinding code was written for this closure.** **CITATIONS REPAIRED 2026-08-19 (the `app.rs` split, `40821ed`) — and three of them were WRONG BEFORE THE SPLIT, so the automated remap made them worse, not better.** `handleCtrlC`'s destination is `app/input.rs:219-231`, not `app/submit.rs` (231 lines, and its `:206` is `take_compaction_queue`); the `defer_to_editor` match is `app/input.rs:63-80`, not `app/draw.rs:123` (`flush_committed`); and Escape's abort path is `app/input.rs:130-213`, not `app/submit.rs:149` (`debug_markdown`). Re-read and verified good as written: `keymap.rs:656`, `keymap.rs:268` (`"app.clear" => Some(Action::Clear)`), `keymap.rs:760` (`Keymap::merge_json`), `crates/cyrup-tui/src/tests/keymap.rs:75` and `crates/cyrup-tui/src/tests/keybindings.rs:200`, both of which still assert `action_for(ctrl+c) == Action::Clear`. **The closure verdict is unchanged and re-confirmed at HEAD `4fb5e40`.** |
| ~~TUI-089~~ | ~~medium~~ **CLOSED 2026-09-04 — REFUTED** | ~~port-bug~~ *(mis-filed — cyrup already orders at pi's two points, by pi's rules)* | S | **REFUTED 2026-09-04 at HEAD `d685eff1` (guard test) — cyrup orders the catalog at exactly the two points pi does, with the same rules, so a `models.json` model on an EXISTING provider lands at the END of its own provider's block, adjacent to its siblings, and a wholly-new provider trails every built-in; that is pi's order too.** **Where pi orders (v0.84.4):** (1) catalog ASSEMBLY — `packages/coding-agent/src/core/provider-composer.ts:168-206` `applyModelsJson`: a declared id that matches a built-in REPLACES it in place (`:199` `findIndex`, `:202`), a new id is `models.push(model)`ed at the END of that provider's list (`:203`); `core/model-runtime.ts:236-243` `providerIds()` is an insertion-ordered `Set` (builtins → native extension providers → models.json providers → extension providers), so a wholly-new provider block trails every built-in; `:183-189` wraps every built-in in `withRemoteCatalog` BEFORE composition, so pi.dev overlay extras precede the models.json customs inside a block. (2) picker RENDER — `modes/interactive/components/model-selector.ts:225-239` `sortModels`: current first, persisted default second, then `a.provider.localeCompare(b.provider)` **only**, with JS's stable sort, so within a provider the assembly order is what is shown — **pi never sorts by model id inside a block either.** (3) `--list-models` — `cli/list-models.ts:54-58` sorts provider then id (a deliberately different, id-aware order). **cyrup at HEAD, same points, same rules:** `crates/cyrup-config/src/model/compose.rs` `ModelFile::compose` (`:30-68`, provider order = base first-seen then the file's own) and `apply_models_json` (`:128-199`; step 2 `:173-185` replaces in place or `models.push(model)`), called by `crates/cyrup-session-svc/src/session/model.rs` `full_model_registry` (`:242`) over a base that already carries the DRIFT-007 overlay (`:214-231`) — pi's `withRemoteCatalog`-then-compose layering; `crates/cyrup-tui/src/model_selector.rs` `ModelSelector::new` (`:177-181`) and `sort_models` (`:158-174`): current, default, then `a.provider.cmp(&b.provider)` with the stable `Vec::sort_by`; `crates/cyrup/src/actions.rs:132` `list_models` sorts provider then id. The same code was in place at the filing commit `7e2e60cc` (`cyrup-config/src/model.rs:2242`/`:2400`, `cyrup-session-svc/src/session.rs:3151`, `model_selector.rs:97-101`). **Empirical assembly probe** (scratch crate outside the workspace, not committed): the real built-in catalog (1 073 models) + a `models.json` adding `moonshotai/Kimi-K9-probe` to `together` and a new `mycorp` provider → `ModelFile::compose` placed the probe at index 853, immediately after the last built-in `together` entry (852, `zai-org/GLM-5.2`), inside a contiguous 22-entry `together` block, and `mycorp-large` at 1 074 (last). **Scope check answered:** an entry added to an EXISTING provider goes to the end of that provider's block; a wholly-new provider block goes after every built-in — both as in pi. **Why the 2026-08-15 observation read as "bottom of the list":** with the picker grouped by provider, `together` sorts after every configured built-in id except `vercel-ai-gateway`/`xai`, so its block IS the tail of the list and the custom model is the last row OF THAT BLOCK — which is where pi puts it. (`moonshotai/Kimi-K3` has since become a built-in, `crates/cyrup-provider/src/providers/together.rs:286`, so a `models.json` entry with that id now replaces it in place.) **Guard test** `crates/cyrup-tui/src/model_selector.rs` `models_json_appended_model_stays_inside_its_provider_block`: feeds the compose-shaped list through both sort sites and pins provider-contiguous grouping with the custom model last in its block. Passes at HEAD (refutation — no fix precedes it); shown to bite by mutation — replacing the provider tier in both sort sites with `Ordering::Equal` fails it with the custom model detached from its block. **Residual (low, not this row):** neither side sorts by id inside a provider block, so a custom model never sits next to its `moonshotai/` namesakes — pi parity, not a defect. **Tooling residual (medium, not this row):** `cargo clippy -p cyrup-tui --all-targets -- -D warnings` fails at HEAD on a `redundant_closure` at `app/input_reader.rs:443` introduced by the workspace rustfmt commit `3e69ea2a`; with that one lint allowed the crate is clean. **Resolved 2026-09-04 by `9cd2d6f0` (`style(tui): TUI-081 clear the pre-existing redundant_closure denial in input_reader.rs`); `cargo clippy -p cyrup-tui --all-targets -- -D warnings` is clean at HEAD (batch-2 ledger audit).** **The original row, struck:** ~~Models added through `~/.cyrup/agent/models.json` are appended to the picker unsorted instead of being merged into its ordering. Observed live 2026-08-15: the built-in `together` block is ordered (`Qwen…` → `deepseek-ai/…` → `essentialai/…` → `google/…` → `meta-llama/…` → `moonshotai/Kimi-K2.6` → `moonshotai/Kimi-K2.7-Code` → `nvidia/…` → `openai/…`), and a user-declared `moonshotai/Kimi-K3` on the same provider lands at the bottom of the whole 85-entry list, far from its provider group and its `moonshotai/` siblings. So a user who adds a model finds it only by scrolling past every other provider — with no indication it is present at all. Establish where pi sorts (catalog assembly vs. picker render) and sort at the same point; a picker-only sort would still leave `--list-models` and any other consumer inconsistent. Scope check before fixing: confirm whether this affects all `models.json`-declared entries or only ones added to an EXISTING provider, since a wholly new provider block may take a different path. — FILED 2026-08-15 (live use) — checked 2026-09-04, left open: `model_selector.rs::sort_models` re-orders the already-built candidate list so the current/default model sorts first — a different mechanism from this item's catalog-ASSEMBLY-order complaint. No evidence either way was found in the catalog-assembly code (`cyrup-provider`/`cyrup-config`), which sits outside this area's owned crate; left open rather than guessed at.~~ |
| ~~TUI-090~~ | ~~**high**~~ **FIXED 2026-08-15** | ~~port-bug~~ **cyrup-original** *(re-classed at diagnosis: the failing mechanism — `live_floor` × `Terminal::insert_before` — has no upstream counterpart; pi renders one flat line buffer and appends, so it has no viewport to pin)* | ~~M~~ S | ~~**A whole screen of blank rows is emitted after every conversation turn, pushing the agent's output above the fold**~~ — **ROOT CAUSE CONFIRMED by headless VT-replay reproduction AND FIXED.** **Mechanism:** the FLICKER fix's grow-only `live_floor` pins the inline viewport at full-terminal height for the whole turn (`app/draw.rs:56-88` `App::draw`); any turn with ≳ `term_h − chrome` rows saturates it; mid-turn commits (`MessageEnd`/`ToolExecutionEnd`/`AgentEnd`) then flush via `flush_committed` → `Terminal::insert_before` while the floor is stale-full, and ratatui's contract (ratatui-core `inline.rs:66-67` — "if the viewport takes up the whole screen, all lines will be inserted directly into the scrollback buffer") sends every flushed line straight to native scrollback invisibly; the `AgentEnd` shrink then erases the whole screen (`reanchor_inline_region` `erase_top = 0`, `app/backend.rs:41-57`, erase at `:50-53`) with nothing left to put back. Measured pre-fix (80×24, 30-line turn): 0/30 response lines on the visible grid, 30/30 in scrollback, 21/24 visible rows blank, ~31 blank scrollback rows emitted per turn. **Fix (flush-synchronized floor release):** `App::draw` now reads `transcript.pending()` and, on any frame that will flush a commit with `raw < live_floor`, releases the floor to the remaining content height BEFORE the flush — so the shrink (`resize_viewport`, already ordered before `flush_committed`) puts the flushed lines ON screen directly above the live tail. Between commits the floor stays grow-only (FLICKER fix preserved: one shrink per COMMIT, never per tool event); the idle collapse (void-fix), progressive commit (SCREEN-FILL fix), erase-before-reconstruct (stacking fix) and exactly-once insert (ADR-0001 5(a)) are all preserved. The `live_floor` field comment (`app/mod.rs:206`) and the existing floor guard (`crates/cyrup-tui/src/tests/live_floor.rs:3` `live_floor_tests`) were re-stated to the release rule — the guard previously enshrined the bug's precondition. Full diagnosis + spec: `bugs/TUI-090-post-turn-whitespace.md`. **This was also the per-frame cost growth that made TUI-092's wedge reachable.** **TUI-091 must be re-checked in a real terminal before any further work on it:** committed reasoning blocks went to scrollback invisibly under this bug exactly like any other committed content — if reasoning text is present on screen/scrollback after this fix, close TUI-091 as a duplicate. — **FILED 2026-08-15 (live use), FIXED 2026-08-15.** **RE-VERIFIED AND CITATIONS REPAIRED 2026-08-19 at HEAD `4fb5e40`:** the verdict holds and the fix is intact after the `app.rs` split (`40821ed`) — `flush_pending` is `app/draw.rs:66`, the release `if flush_pending && raw < self.live_floor { self.live_floor = raw; }` is `:68-70`, the grow-only clamp `self.live_floor = self.live_floor.max(raw).min(term_h)` is `:71`, and `resize_viewport` still runs BEFORE `flush_committed` (`:77-87` then `:88`), which is the ordering the whole fix depends on. |
| ~~TUI-091~~ | ~~**high**~~ **CLOSED 2026-09-04 — duplicate of `TUI-090`, settled by observation in a real pty** | port-bug | M | **CLOSED 2026-09-04 as a duplicate of `TUI-090`, on observation in a real pty at HEAD `a4805955`** (the five same-day code commits that followed touch only `crates/cyrup-ext-subagents` and `crates/cyrup-it` fixtures; `crates/cyrup-tui` is byte-identical). Driven in tmux 3.4 — a real terminal emulator on a real pty, the `REPRO-LOG.md` instrument — at 120×40 with the debug `cyrup` binary, an isolated `CYRUP_HOME`, no network: a local SSE server (`127.0.0.1:18931`) speaking `openai-completions` with reasoning deltas, reached through a `models.json` `together` `baseUrl` overlay so the owner's exact path — `--provider together --model moonshotai/Kimi-K3 --thinking high`, footer `together/moonshotai/Kimi-K3 • high` — was exercised. **Seven variants all showed the reasoning block live (mid-stream, above the `⠹ Working...` indicator) AND committed in scrollback above the answer**: (1) short `reasoning_content` deltas; (2) Together's alternative `reasoning` delta field; (3) LONG — 90 reasoning lines, taller than the 40-row screen: 0 of 90 `THINKLINE` rows missing from tmux scrollback after commit; (4) TOOL — thinking → bash tool call → result → thinking → answer, both blocks rendered and the replay request carrying `reasoning_content` on the assistant message; (5) `--tui-mode fullscreen`; (6) `hideThinkingBlock: true` collapsing to the italic `Thinking...` label with the footer gaining `(hidden)` — pi's `hiddenThinkingLabel` (`assistant-message.ts:139-142` @v0.84.4); (7) `--continue` replay re-rendering the reasoning from the JSONL. The request body carried `reasoning: {enabled: true}` (`ThinkingFormat::Together`, `crates/cyrup-provider/src/providers/together.rs:286-295`), the decoded `message_end` carried content types `['thinking','text']` with `thinkingSignature: reasoning_content`, every run's stderr was 0 bytes, and the saved session JSONLs carry non-empty `type: thinking` blocks exactly like the owner's evidence. **The three projections the row asked to instrument did not need instrumenting**: `assistant_message_from_event` demonstrably returned `Some` (the committed block is the `Content::Thinking` path through `finalize_assistant_message`, `app/events.rs:297-305`), and `ThinkingDelta` arrival is visible on screen from the first frame. **The timeline that makes it a duplicate**: `TUI-091` was filed at `7e2e60cc` 2026-08-15 16:26 −0700; `TUI-090` was filed at `61804a16` 15:35 and FIXED at `45da9d3b` 19:50 the same day — the owner's observation predates the fix, and `45da9d3b`'s own body names the asymmetry mechanism (mid-turn commits flushed via `insert_before` into native scrollback invisibly while the viewport was stale-full, then erased at `AgentEnd`) and flags `TUI-091` as the likely duplicate symptom. Pane captures and scrollback dumps: `REPRO-LOG.md` §0e. No code changed. **Falsification**: a live `together`/Kimi-K3 turn at HEAD in a real terminal, `hideThinkingBlock` unset, showing the answer and no reasoning block — not `TestBackend`, which this row already showed cannot distinguish "not rendered" from "rendered off-screen". **Observed, recorded, NOT this row (ownerless leads)**: in the LONG run the tail of the live reasoning appears once as scrolled-off live frames and again inside the committed block (121 `THINKLINE` rows for 90 lines) — inline-viewport-taller-than-screen duplication, cosmetic; the 2026-08-19 "lost interior rows" byproduct did NOT reproduce in a real pty (0 rows lost), so it was a harness artifact as suspected; `--no-extensions --no-skills` still lists `[Extensions] cyrup-flux` and its skills/prompts, because `crates/cyrup-flux` is a compiled-in workspace crate the flags do not govern. The strongest possible proof — a pre-`TUI-090` build of `a6ea9ddd` (= `45da9d3b^`) showing the reasoning hidden — was started and not finished; the closure rests on the seven-variant HEAD observation, the filing/fix timeline, and the fix commit's own stated mechanism. **The original row follows unchanged, for the record.** **Reasoning blocks never render, though every layer from provider to setting — INCLUDING the renderer — is wired and correct.** Owner report, live use 2026-08-15, `together`/`Kimi-K3` at `defaultThinkingLevel: high`, confirmed still absent after scrolling. **SIX layers traced and RE-VERIFIED AT HEAD `4fb5e40` — do not re-trace them:** (1) **provider** — Together streams real `reasoning_content` deltas for this model (live request returns `"delta":{"reasoning_content":"The"}`), and `first_reasoning_delta` (`cyrup-provider/src/api/openai_completions.rs:2111`) reads it first from `REASONING_FIELDS` (`:1630`), emitting `StreamEvent::ThinkingStart/Delta/End` (`:1891`, `:1999`); (2) **agent** — `decode_stream` sends `StreamEvent::Start` before any delta and `agent.rs:926` sets `started = true` on it, so the `if started` gate at `:949` is already open when reasoning arrives, and the `_ =>` catch-all re-emits EVERY non-terminal event as `AgentEvent::MessageUpdate` carrying the original event boxed (`:948-956`); (3) **facade** — `cyrup-session-svc/src/event.rs:292-296` clones `assistant_message_event` straight from `AgentEvent::MessageUpdate` onto `AgentSessionEvent::MessageUpdate` with no projection (the `grep 'ThinkingDelta' cyrup-session-svc` miss was a false lead — the facade carries the event *boxed and opaque*, so the variant name never appears there); (4) **TUI route** — `app/events_fold.rs:125-126` destructures exactly that field off `MessageUpdate` and calls `ingest_stream_event`; (5) **TUI handler** — `app/events_fold.rs:471-475` matches `StreamEvent::ThinkingDelta` and calls `push_thinking_delta`, which accumulates into `TranscriptView::thinking` (`transcript.rs:705-711`); (6) **setting** — `hide_thinking_block()` is `get_bool("hideThinkingBlock").unwrap_or(false)` (`cyrup-config/src/settings.rs:580-582`), matching pi's `?? false` (`settings-manager.ts:855`), seeded into the view at boot (`app/run_arms.rs:61-63`) and re-seeded on every session swap (`:227-229`), and the reporter's `settings.json` does not set the key. **MEASURED AGAINST A LIVE SESSION 2026-08-15 — the model IS thinking and cyrup IS capturing it:** the reporter's active transcript (`~/.cyrup/agent/sessions/--Users-davidmaple-cyrup.ai--/*.jsonl`, 682 KB) contains **40 non-empty `"type":"thinking"` content blocks** alongside 106 `text` blocks across 133 messages, with real reasoning prose in each — so reasoning is requested, returned, decoded AND PERSISTED. **THE ONE REMAINING CANDIDATE IS NOW REFUTED — 2026-08-19, at HEAD.** The row's last hypothesis ("the accumulated `thinking` buffer is never included in the painted region") is FALSE, and was false when it was filed: `TranscriptView::lines` puts the LIVE reasoning block ABOVE the answer text at `transcript.rs:1272-1298` — the visibility test at `:1272`, the `thinking_lines` call at `:1282-1289`, and pi's own `hasVisibleContentAfter` spacer at `:1293-1297` — and `git blame` dates that block to `d2c5509` (2026-08-02) and `bc7a538` (2026-08-09), a week BEFORE the filing. **`TUI-092` F2's render cache neither fixed it nor could have broken it:** `cached_render` (`transcript.rs:1228-1249`) is keyed on `render_generation`, and `push_thinking_delta` bumps that on its FIRST statement (`:706`), so a reasoning delta always invalidates. **All six `src/tests/thinking.rs` cases are GREEN at HEAD** — `cargo test -p cyrup-tui --lib thinking::` → 6 passed / 0 failed, including `streaming_thinking_deltas_render_in_the_live_viewport` and `terminal_thinking_blocks_commit_above_the_answer`. **THAT REFUTATION IS NOW CONFIRMED BY EXECUTION, AND THE ROW IS RELOCATED OUT OF THE RENDER PATH ENTIRELY — 2026-08-19.** Every layer including rendering is proven by code and by test, which leaves the confound as the leading explanation: `TUI-090` was FIXED the same day this was filed, and under it a correctly-rendered committed block went to native scrollback invisibly like any other committed content. Re-run the reporter's `together`/Kimi-K3 scenario in a REAL terminal at HEAD; if reasoning is present, close this as a duplicate of `TUI-090`. `TestBackend` cannot distinguish "not rendered" from "rendered off-screen". **HEADLESS REPRODUCTION ATTEMPTED AT HEAD AND IT DOES NOT REPRODUCE — through TWO harnesses, one of them the pty-equivalent.** Two throwaway probes, both deleted afterwards (`git status` clean; `tests/mod.rs` and `tests/inline_stacking.rs` restored byte for byte): (i) the assembled `TestBackend` at 100x30 driven with the real event sequence — `AgentStart` → `MessageStart` → 40 `MessageUpdate{ThinkingDelta}` → 6 `TextDelta` → `MessageEnd` carrying `Content::Thinking` + `Content::Text` → `AgentEnd`, drawing once per event; (ii) the `CaptureBackend` harness from `crates/cyrup-tui/src/tests/inline_stacking.rs:61-152`, with its VT screen model at `:159-340` and `replay` at `:343-347` — a REAL `CrosstermBackend` over a shared byte buffer, replayed through its VT model, i.e. what a user actually sees — same sequence at 80x24. **Both show the reasoning block on screen mid-stream AND in the reconstructed scrollback after the turn, alongside the answer.** **The row's four open questions are now answered by measurement, not by reading.** **(1) The accumulated buffer DOES reach the painted line set:** `push_thinking_delta` (`transcript.rs:705`) → `TranscriptView::thinking` → the `thinking_visible` gate at `:1272` → `thinking_lines` + `pad_lines` extended into the returned `Vec<Line>` at `:1282-1298` → `cached_render` (`:1228`) → `Component::render` clones `cache.lines` into the `Paragraph` (`:3379`) → painted into `msg_area` at `app/render.rs:49`, with layout sized from the same cache (`app/layout.rs:165`). **(2) The F2 cache IS invalidated by thinking deltas:** `push_thinking_delta` bumps `render_generation` as its FIRST statement (`transcript.rs:706`), the key is `(generation, width, theme.generation)`, and the probe measured the sentinel appearing in the live region at **delta #0** and the block growing on every delta after it (`transcript.thinking()` reached 2 158 bytes, viewport grew to 30 rows) — not a first-frame-then-frozen cache. **(3) The commit path is intact and nothing overwrites it:** `finalize_assistant_message` (`app/events.rs:178`) commits `thinking_text(content)` BEFORE the answer, `commit_thinking` (`transcript.rs:724`) pushes `Entry::Thinking`, `entry_lines` renders it (`:2940`), `flush_committed` (`app/draw.rs:128-183`) emits it via `insert_before` (`:178`) — the probe saw EXACTLY ONE committed reasoning block, correctly ordered above the answer, with no double-commit from `AgentEnd`'s `commit_thinking(None)` (`app/events_fold.rs:51`) because finalize had already cleared the buffer. `hide_thinking_block` was `false` throughout and is `false` by default (`cyrup-config/src/settings.rs:580`). **WHERE THE BUG NOW LIVES — the assembled live app or the terminal, NOT `transcript.rs`, the F2 cache or the commit/flush wiring.** The single mechanism found that reproduces the reported ASYMMETRY (answers render, reasoning does not) is the `MessageEnd` guard at `app/events_fold.rs:121-125`: `if self.state.streaming_assistant && let Some(message) = assistant_message_from_event(&ev)`. If either condition is false live, the authoritative `Content::Thinking` blocks are never committed, and the `AgentEnd` fallback (`events_fold.rs:51`, `commit_thinking(None)`) can only commit `self.thinking` — the STREAMED buffer. **The answer has a second source and the reasoning does not** (`commit_assistant(None)` at `:52` recovers the `TextDelta` buffer), which is exactly the asymmetry. So "session JSONL has 40 thinking blocks, screen has none" is fully explained by that guard failing PLUS `ThinkingDelta` not arriving — and by nothing in the render path. **INSTRUMENT THESE THREE, LIVE, AND NOTHING ELSE:** `message_role_from_event` (`app/event_extract.rs:190-193`), `assistant_message_from_event` (`:219-226`), and a `ThinkingDelta`-arrival counter at `app/events_fold.rs:497`. **Both projections go through `serde_json::to_value(ev)` (`event_extract.rs:191`, `:220`) and fail SILENTLY to `None` on any shape mismatch**, which is the failure mode to look for first. **BYPRODUCT, NOT THIS ROW AND NOT FILED:** in the pty replay the commit flush loses interior rows when the inserted block plus the viewport exceeds the screen — 4 of 11 committed rows lost with reasoning, and **2 of 10 lost in a CONTROL run with a long answer and no thinking at all**. The control is what disqualifies it: generic flush geometry (`app/draw.rs:178` `insert_before` on the frame where `resize_viewport` also runs), not a reasoning defect, so it cannot be this row's mechanism. It could not be reproduced analytically from ratatui's `insert_before_no_scrolling_regions` arithmetic and may be an artifact of the harness's deliberately partial VT model, whose declared escape subset discards private modes including `?7h/l` DECAWM auto-wrap (`inline_stacking.rs:156-158`, `:296`). **It needs a real pty before it is worth a row.** **All 1 270 `cyrup-tui` lib tests pass at HEAD.** **Citation repair, same date:** four of this row's original citations were garbage BEFORE the `app.rs` split and the auto-remap carried one forward faithfully — pre-split `app.rs:6007` was `push_tool_start_rendered(`, `:7842` was `env_geometry`, `:5661` was share-export, and the remapped `app/execute_misc.rs:316` is share-export code. All four are replaced above. — **FILED 2026-08-15 (live use); LAST CANDIDATE REFUTED 2026-08-19; REFUTATION CONFIRMED BY EXECUTION AND THE ROW RELOCATED TO THE LIVE EVENT FOLD 2026-08-19** — **2026-09-04: not re-touched.** This pass had no pty available, and the row's own next step is a live instrumentation of three named projections in a real terminal — a static re-read cannot move it further. Left exactly as written. |
| ~~TUI-092~~ | ~~**high**~~ **CLOSED 2026-08-20 — F1-F8 all landed** | port-bug | M | ~~**The TUI degrades from smooth to a total lockup.**~~ **CLOSED 2026-08-20:** the last open contributor, **F8 (by-value ingest)**, landed in `24b6ffe` — `ingest_event_rendered_owned` (`app/events_fold.rs:20`) moves `args`+`tool_name`, `partial_result`, `is_error`+`result` and `(steering, follow_up)` instead of cloning them, since the receiving transcript APIs already consume by value. Scope, recorded honestly: F8 was this row's LOWEST-severity contributor, and one clone per event survives outside it — the fanout's per-subscriber `s.send(ev.clone())` (`cyrup-session-svc/src/subscriber.rs`). What remains is not a defect but a MEASUREMENT: one long post-round-2 live session to confirm the four-phase degradation is gone. Re-open only against a fresh live report. **The TUI degrades from smooth to a total lockup.** **DE-ESCALATED critical → high 2026-08-19, because all three clauses that justified `critical` are FALSE at HEAD `4fb5e40`, and this row now agrees with its own umbrella file (`bugs/TUI-092-progressive-lockup.md:17`, which has said `high` since round 2).** `Ctrl+D` **does** kill it: `keymap.rs:655` binds `(Key::ctrl('d'), Action::Quit)`, `app/input.rs:126-129` sets `should_quit` and returns `AppAction::Quit`, and `app/run_action.rs:16` breaks the loop. `TUI-088` is CLOSED, not open: Ctrl+C is bound (`keymap.rs:656`), wired to pi's `handleCtrlC` double-tap-exit within 500 ms (`app/input.rs:219-231`) and advertised in the startup hint bar (`chrome.rs:80-90`). And round 1's escape hatch means the user is never trapped even in a genuine wedge: three unserviced Ctrl+C/Ctrl+D chords hard-exit **from the reader thread**, the one context a wedged run loop cannot block (`PANIC_PRESSES = 3` at `app/input_reader.rs:147-153`, the chord test at `:176-184`, the escalation counter at `:258`; the exit at `:193-206` also prints `cyrup: run loop wedged in arm {arm} for {elapsed}`). **ROUND 2 IS 7 OF 8 LANDED — F8 IS NOT, AND ITS TASK FILE WAS DELETED ANYWAY.** F1 (scrollback accumulator off in production) `app/state.rs:135-138` + `app/draw.rs:168-169`, feature-gated at `Cargo.toml:21-27`; F2 (render cache) `f14a5db` → `transcript.rs:352` (`render_generation`), `:1228-1249` (`cached_render`), `:1254` (`content_height(&mut self …)`), with `&mut` propagated through `app/layout.rs:48`; F3 (draw coalescing — drain, then ONE frame) `f22efab` → three drains, `app/run_action.rs:217-220` (input), `:267-270` (events, ending in the single `self.draw_synchronized()?` at `:298`) and `app/run_arms.rs:351-355` (bash); F4 (`context_usage` walks once) `2086366` → `cyrup-session-svc/src/session.rs:4146`, whose own doc at `:4144-4145` records the O(session history) allocation it replaced; F5 (DECSTBM `insert_before`) `425ef9f` → `Cargo.toml:19` + `:28-33`; F6 (bounded bash ring) `8f10804` → `bash.rs:29` (`MAX_OUTPUT_LINES = 2000`), `:57` (`VecDeque`), `:155` (`omitted_lines`), `:305-313` (the `… (N earlier lines omitted) …` row); F7 (image protocol cache) `e6f298d` → `image.rs:58` + `:204-224`. **F8 — by-value ingest — WAS NEVER WRITTEN, though `e6f298d` ("…land TUI-092 F5-F8") DELETED `bugs/TUI-092-F8-by-value-ingest.md`.** At HEAD `app/run_action.rs:281` still reads `// F8 swaps this one call to ingest_session_event_owned(ev, &session).` and `:282` is still `self.ingest_session_event(&ev, &ctx.session).await;` — by reference; `ingest_event_rendered` still takes `ev: &AgentSessionEvent` (`app/events_fold.rs:7-12`); `rg 'ingest_session_event_owned' crates/cyrup-tui/src` returns **zero hits**; and every clone F8 named survives — `args.clone()` (`app/events_fold.rs:141`), `partial_result.clone()` (`:163`), `result.clone()` (`:173`) and `(steering.clone(), follow_up.clone())` (`:190`). So the umbrella's DoD property 3 ("one wakeup, one frame … and the events arm moves (not clones) each payload") is HALF-MET: the drain landed, the move did not. **A deleted task file is not a landed fix — do not close this row on `e6f298d`'s subject line.** **All three original candidates are disposed of:** (1) `TUI-090` is FIXED, so it is not the cause; (2) the render-cost half is F2 + F3 + F5 + F7; (3) the per-turn-walk half is F4. **Its "already ruled out" citations have drifted and are repaired here:** `drain_committed`'s `mem::take` is `transcript.rs:596-598`, `commit_tools` `:955`, `commit_finished_leading_tools` `:984`. **What is left, and all this row is now open for:** land F8 (the one-line by-value swap plus the four clone removals), then re-measure frame time and RSS against turn count in a real terminal per `handoff/03-verification.md` — no post-round-2 live observation has been recorded. — **FILED 2026-08-15 (live use); RE-BASELINED AND DE-ESCALATED 2026-08-19** |
| ~~TUI-093~~ | ~~**high**~~ **FIXED 2026-08-17** | ~~port-bug~~ **cyrup-original** *(re-classed at filing: the failing mechanism — ratatui's `CrosstermBackend::get_cursor_position` DSR round-trip racing crossterm's own reader thread — has no upstream counterpart; pi drives its TTY writer directly and never asks the terminal where the cursor is)* | M | ~~**A commit flush, a window resize or a live-region height change stalls and then ENDS THE SESSION with `cursor position could not be read`**~~ — **FILED RETROACTIVELY 2026-08-19.** The work landed at `77dca02` + `743cad8` (2026-08-17) and had NO row in any area file, while `TUI-093` is cited by name in twelve source locations — `terminal_query.rs:24`, `:105`, `:144`, `:712`; `app/backend.rs:7`, `:75`, `:153`, `:213`, `:331`, `:424`; `app/crossterm.rs:65`; `app/draw.rs:78` — and in `src/tests/resize_viewport_failure.rs:1`. The residual ledger's own rule ("grep the SOURCE for `AREA-NNN` citations at every reconciliation, not just the docs") is what should have caught it. **Mechanism.** `CrosstermBackend::get_cursor_position` is a raw `CSI 6 n` round-trip, which crossterm documents as blocking and possibly timing out while `event::read`/`poll` are being called — and `crossterm_input_stream`'s reader thread (`app/input_reader.rs:312-381`) does exactly that for the whole session. ratatui reaches that query from `insert_before` (via `Terminal::clear`), from `autoresize` inside every `Terminal::draw`, and from `Terminal::with_options(Viewport::Inline)`, so all three of cyrup's hot paths — commit flush, window resize, live-region height change — risked the stall and an `Err` that `App::draw` then propagated as a FATAL `TuiError::Backend`. **Fix — the backend ANSWERS instead of ASKING.** `InlineBackend` (`app/backend.rs:91-95`) tracks the anchor cyrup itself set; `get_cursor_position` (`:154-158`) returns it and re-emits the `MoveTo` so the answer is made TRUE rather than merely remembered (`:150-153` states the rule); `reanchor_inline` (`:211-217`) keeps the row `reanchor_inline_region` just moved to instead of discarding it; `rebuild` (`:207-209`) carries the anchor across the re-wrap. The process makes exactly ONE cursor query, in the same pre-reader-thread window as the Kitty negotiation, hard-bounded at `CURSOR_POSITION_TIMEOUT = 100 ms` (`terminal_query.rs:112`) and falling back to the bottom row on a silent terminal (`app/crossterm.rs:64-82`). **Second half — the viewport reconstruction is now NON-FATAL** (`app/draw.rs:77-87`): a failure pushes `viewport resize failed: …` as a status line and leaves `viewport_height` uncommitted so the next frame retries, rather than unwinding ~40 `draw_synchronized()?` call sites out of `App::run`. **New public surface this file had no inventory for:** `InlineBackend` is exported at `crates/cyrup-tui/src/lib.rs:98`, alongside the pre-existing `RebuildBackend` and `reanchor_inline_region`. **Record the DECISION, not just the fix:** cyrup deliberately never probes the terminal for its cursor position after the reader thread starts. A later pass reading ratatui's `Backend` contract must not "restore" the probe as missing behaviour — there is no pi behaviour to restore, because pi has no such query. **Coverage:** `app/backend.rs:339` (`get_cursor_position_returns_the_tracked_anchor_without_querying_the_terminal`, which also asserts the call completes well inside crossterm's 2 s DSR timeout), `:354` (`get_cursor_position_asserts_the_anchor_with_a_moveto`), `:415` (`rebuild_carries_the_anchor_across_the_rewrap`), plus DoD-4 at `src/tests/resize_viewport_failure.rs:171` and `:201`. — **LANDED 2026-08-17; FILED AND CLOSED IN ONE PASS 2026-08-19** |
| ~~TUI-094~~ | ~~**high**~~ **FIXED 2026-08-18** | **cyrup-original** *(a `tokio::select! { biased; … }` starvation hazard; pi's event loop is a JS microtask queue and has no arm-priority concept)* | S | ~~**`/new` prints its receipt and the TUI goes permanently unresponsive, with one worker hot-spinning at 100%**~~ — **FILED RETROACTIVELY 2026-08-19: landed at `879eb4e` (2026-08-18) with no row anywhere.** This is the exact user-visible symptom `TUI-092` was filed for, from a different cause, which is why it needs its own id rather than being folded in. **Mechanism.** The events arm bound `maybe_ev = events.next()` — an IRREFUTABLE pattern, so a `None` from a closed stream matched it as readily as `Some(ev)`. `Fanout::invalidate` (`cyrup-session-svc/src/subscriber.rs:89-93`) drops every sender on a session swap, so the old subscription goes permanently `Ready(None)` the instant a replacement lands; under `biased;` that permanently-ready arm won every poll and starved every arm below it — including `session_swapped`, which sat LAST. No re-subscribe, no `rebind_session()`, and the loop stayed bound to the disposed session. **Fix, in two halves that only work together:** the events arm is now REFUTABLE (`app/run.rs:344`, `Some(ev) = events.next()`), so a closed stream DISABLES the branch; and the swap arm is hoisted directly below the input arm, above every arm that can be permanently ready (`app/run.rs:293-297`, with the whole rationale at `:283-292`). The swap-rebind logic is extracted to `App::on_session_swapped` (`app/run_arms.rs:138-288`) so it can also run from the input arm's pre-dispatch reconcile, with a generation-based re-entrancy guard (`:156`, `:283-285`) mirroring pi's `if (this.session !== session) return;`. **Coverage:** `src/tests/run_loop_swap_arm_reachable.rs:39` (`the_swap_arm_outranks_the_events_arm_and_every_ticker_and_the_events_pattern_is_refutable`) asserts both halves structurally. **AMEND `bugs/TUI-092-progressive-lockup.md`:** its round-1 "Do not touch — the `biased;` arm ordering in `App::run`" is WRONG as written and this fix violated it for a good reason. The invariant round 1 actually pinned is *cancel → input → rest*; the real invariant at HEAD is **cancel → input → SWAP → rest**, stated in-source at `app/run.rs:289-292` ("do NOT 'tidy' it back down"). The umbrella must say so explicitly or the next agent reverts a fix for a 100%-CPU hang. — **LANDED 2026-08-18; FILED AND CLOSED IN ONE PASS 2026-08-19** |
| TUI-095 | low | cyrup-original | — (record-only) | **The persistent command-token highlight and the argument-hint ghost are cyrup-original chrome pi does not render, and until now had no row — only the ad-hoc marker `CMDHINT_01`.** Landed at `0b7c4f4` + `bae24f5` (2026-08-17). **What ships.** While the first buffer line is a real command prefix, the `/name` token stays highlighted after the user types past it, and the command's declared `argument-hint` is drawn as dim ghost text after the space: `InputEditor::command_highlight` (`crates/cyrup-tui/src/editor.rs:238` doc, `:255` fn), the zone splitter `:2196`, the restructured cursor overlay `:2242`, the clipped ghost span `:2311`, and the matcher `autocomplete.rs:177` (`is_command_prefix`). **Verified cyrup-original, not drift.** The code says so in its own words at `editor.rs:211-213` — "CMDHINT_01 — cyrup-original; pi renders neither. Its `argumentHint` reaches exactly one site upstream, the popup description at `tui/src/autocomplete.ts:315`" — corroborated by `spec/CMDHINT_01.md:10-15`, which records that pi's `packages/tui/src/components/editor.ts` and coding-agent's `custom-editor.ts` were both read and do neither half. **One deliberate design choice worth pinning so it is not "fixed" later:** the matcher is NOT the fuzzy one `slash_context` uses. `autocomplete.rs:170-176` states the reason — fuzzy is right for a suggestion LIST and wrong for a signal that claims "what you typed is literally the start of a real command", because `/fa` fuzzy-matches `flux/aug` and must not be highlighted as a real path segment, and a bare `/` returns everything. **Why a row rather than nothing:** `cyrup-original` is a first-class counted class rated by reachability, and this is maximally reachable — it changes what every user sees on every keystroke of every slash command. It is also the `EXT-M03` situation the README legislated for: `CMDHINT_01` is cited thirteen times in shipped source (`cyrup-session-svc/src/session.rs:2628`; `cyrup-tui/src/editor.rs:26`, `:212`, `:238`, `:2196`, `:2242`, `:2311`, `:2945`; `autocomplete.rs:170`; `commands.rs:470`; plus three test files) with no ledger row, so `grep CMDHINT` resolved to nothing. **`CMDHINT_01` is the in-source marker; `TUI-095` is the id** — do not adopt the ad-hoc spelling into the ledger. **Two loose ends, neither a defect in this row:** `spec/CMDHINT_01.md`'s frontmatter still reads `stage: new, status: todo` although both commits landed, and the same change added an `argumentHint` key to the `get_commands` RPC payload that pi's `rpc-mode.ts` never emits — **that belongs in area 08 and is NOT recorded there** (`grep -n 'argumentHint\|CMDHINT' 08-cyrup-session-svc-and-modes.md` → zero hits), which is exactly what `TUI-078`'s own Fix instructed. — **FILED 2026-08-19 (surface sweep of the 27-commit round-2 batch)** |

## TUI-042 — The undo snapshot omits the paste registry — undoing a delete over a `[paste #N …]` marker silently drops the pasted content from the submitted message

> ## FIXED 2026-08-13 — `crates/cyrup-tui/src/editor.rs`
>
> `Snapshot` now carries `pastes: BTreeMap<u32, String>` and `paste_counter: u32` (pi's
> `EditorSnapshot`, `editor.ts:216-220`); `snapshot()` clones both (`:2012-2014`) and `undo()`
> restores both (`:2016-2030`). Three things the item did not name were needed for the fix to
> actually match pi:
>
> 1. **The paste's own snapshot was pushed too late.** cyrup incremented `paste_counter` and inserted
>    into `pastes` *before* `push_undo_for`, so the snapshot already contained the new paste and undo
>    could not roll it back. `handlePaste` pushes the snapshot as its FIRST statement
>    (`editor.ts:1160`). Moved. This is what makes paste → undo → paste re-issue `#1`.
> 2. **`clear()` never reset `paste_counter`.** `submitValue` does `pastes.clear(); pasteCounter = 0`
>    (`editor.ts:1264-1266`) and so does `setText` (`:1018-1020`). Without it, ids climbed for the
>    life of the session, so cyrup's `[paste #7 …]` was pi's `[paste #1 …]`.
> 3. **The undo stack survived a submit.** `submitValue` clears it (`:1268`); both cyrup submit paths
>    now do.
>
> The `history_draft` half the item asks for was already benign — nothing mutates the registry while
> browsing, in either codebase (pi's `historyDraft` is a bare `EditorState`, `:319`). The draft path
> *was* broken in another way: pi pushes an undo snapshot when browsing is entered
> (`editor.ts:435-438`) and cyrup pushed none, so `Ctrl+-` after an `Up` skipped past the draft and
> emptied the buffer. Added; pinned by `::browsing_history_away_from_a_draft_keeps_its_paste_registry`
> (RED at HEAD: `left: ""`, right: `"[paste #1 1500 chars]"`).
>
> **Tests** (all RED at HEAD, GREEN after, `crates/cyrup-tui/src/tests/editor.rs`):
> `undo_restores_the_paste_registry_not_just_the_marker_text` (RED: `left: "[paste #1 1500 chars]"`),
> `undo_rolls_back_the_paste_counter_so_the_next_paste_is_still_marker_one`,
> `browsing_history_away_from_a_draft_keeps_its_paste_registry`.

**Kind** parity-bug · **Severity** critical · **Effort** S · **Confidence** **confirmed — reproduced in a live terminal, with the model's input read out of the session JSONL** · **observed 2026-08-13** (live-terminal; [`REPRO-LOG.md`](REPRO-LOG.md))

> **Reproduced 2026-08-13 end to end, under tmux, via a real bracketed paste** (`tmux paste-buffer -p`,
> which emits `ESC[200~ … ESC[201~`). 719 bytes / 40 lines → `[paste #1 +40 lines]`; one Backspace
> deletes the marker atomically; `Ctrl+-` (sent as the kitty CSI-u form `ESC[45;5u` — see **TUI-053**,
> the legacy byte does not reach `E::Undo`) puts the marker text back on screen; Enter submits the
> **20-character literal string** `[paste #1 +40 lines]`, read out of the session JSONL rather than
> off the screen. The **control** run in the same session — same paste, no edit, straight to Enter —
> submits all 719 characters, so the loss is caused by the undo and not by the paste path. The
> quieter variant reproduced too: paste → undo → paste re-issues `#4` where pi restores
> `pasteCounter` and reissues `#3`.
>
> **One measured correction to the Impact below.** `[paste #1 2000 chars]` is **21** characters, not
> 20 (measured live: pasted 2000 chars, captured the marker, `len=21`). The figure 20 is correct only
> for the `+N lines` form (`[paste #1 +40 lines]`, measured `len=20`). Mechanism, call sites and both
> variants reproduce verbatim.

**cyrup** — `crates/cyrup-tui/src/editor.rs:71-78` defines `struct Snapshot { lines: Vec<Vec<char>>, row: usize, col: usize }` — **no paste registry**. `snapshot()` (`:716-719`) clones only those three fields; `undo()` (`:748-756`) restores `lines` and `row` and nothing else. The registry it fails to snapshot is `pastes: BTreeMap<u32, String>` (`:149`) with `paste_counter: u32` (`:151`), and both are *mutated destructively* on the very paths that push an undo snapshot: `backspace()` at `:814` and `delete()` at `:852` each call `self.pastes.remove(&id)` **after** the `E::DeleteCharBackward` / `E::DeleteCharForward` arms (`:1487-1500`) have already pushed the snapshot. Expansion is gated on the registry: `marker_at` (`:663-694`) ends with `let content = self.pastes.get(&id)?;`, so once the entry is gone the marker text is **no longer a marker**, and `expanded_text` (`:639-659`) — the function `E::Submit` calls at `:1570-1571` to build what the agent actually receives — emits it verbatim.

**upstream** — `pi/packages/tui/src/components/editor.ts:216-220` @v0.83.0 (identical at v0.84.1, same line numbers): `interface EditorSnapshot { state: EditorState; pastes: Map<number, string>; pasteCounter: number }`, commented "Undo snapshot: editor text state plus the paste registry." `:2012-2014` `pushUndoSnapshot()` pushes `{ state, pastes, pasteCounter }` (v0.84.1 `:2024`); `:2016-2030` `undo()` does `Object.assign(this.state, snapshot.state); this.pastes = snapshot.pastes; this.pasteCounter = snapshot.pasteCounter;` (v0.84.1 `:2028`). The deep copy that makes it work is `packages/tui/src/undo-stack.ts:11-13` — `push(state) { this.stack.push(structuredClone(state)) }` — `structuredClone` deep-clones a `Map`, so each snapshot owns its own registry.

**Impact** — paste 2,000 characters (or >10 lines) into the prompt; the buffer shows `[paste #1 2000 chars]` and `pastes[1]` holds the text. Backspace once — the marker is atomic for backspace (`editor.rs:801-816`), so it vanishes and `pastes[1]` is erased. Press Ctrl+- to undo: **the marker text reappears on screen**, so the user sees their paste restored. Press Enter. `expanded_text()` cannot resolve id 1 any more, so the model receives the 21-character literal string `[paste #1 2000 chars]` instead of the 2,000 characters (**length corrected 2026-08-13 from a live measurement**; the `+N lines` form measures 20). Silent data loss with a UI that actively asserts the opposite, on the most ordinary editing sequence there is. pi restores the registry and sends the full text. A second, quieter variant: paste → undo (no delete) leaves an orphan `pastes` entry and never rolls back `paste_counter`, so ids drift from pi's and the map grows for the life of the session.

**Fix** — add `pastes: BTreeMap<u32, String>` and `paste_counter: u32` to `Snapshot` (`editor.rs:71-78`); have `snapshot()` (`:716-719`) clone both and `undo()` (`:748-756`) restore both alongside `lines`/`row`. That is the whole fix — every call site already goes through `snapshot()` / `push_undo_for*`. While there, make `history_draft` (`:93`, `:1199`, `:1218`) carry them too, since it reuses `Snapshot` and browsing history away from a draft containing a marker has the identical failure. **Ships with TUI-044** so `Snapshot` is corrected once. Note cyrup's 500-entry stack bound (`:728-730`, `:742-744`) is a cyrup-original — pi's `UndoStack` is unbounded (`undo-stack.ts:7-28`); keep the bound but state it as a delta rather than leaving it undocumented.

**Verify** — `crates/cyrup-tui/src/tests/editor.rs`: build an `InputEditor`, `handle_paste(&"x".repeat(1500))`, assert `expanded_text()` contains the 1500 chars; send `E::DeleteCharBackward`, then `E::Undo`; assert `text()` again shows `[paste #1 1500 chars]` **and** `expanded_text()` once more contains the 1500 chars (fails today — it returns the literal marker). Second case: paste, `E::Undo`, paste again, assert the second marker is `#1` not `#2`.

## TUI-043 — Word motion and Ctrl+W are not paste-marker atomic — one Ctrl+W after a large paste orphans the marker and drops the pasted content

> ## FIXED 2026-08-13 — `crates/cyrup-tui/src/editor.rs`
>
> Ported the mechanism rather than the symptom. `word_left_target`/`word_right_target` are now
> statement-for-statement ports of `findWordBackward`/`findWordForward`
> (`word-navigation.ts:22-114`), driven by a port of `segmentWithMarkers` (`editor.ts:37-90`) —
> whitespace skip, then one of the three branches: atomic segment (`:44-46` / `:97-99`), word-like
> segment truncated at its last/first `PUNCTUATION_REGEX` match, or a punctuation run. `marker_at`
> supplies the `validIds` gate (`:44`).
>
> **Two corrections to the item's Fix, both from reading upstream at the tag:**
>
> 1. **"make `delete_word_backward`/`forward` drop the registry entry the way `backspace()` does" is
>    not pi's mechanism.** `deleteWordBackwards` (`editor.ts:1607-1630`) and `deleteWordForward`
>    (`:1633-1672`) compute a range from `moveWordBackwards`/`moveWordForwards` and slice the text —
>    they never touch `pastes`. The orphan entry is harmless in both codebases (`expandPasteMarkers`
>    only replaces markers that are *in the text*), and it is what lets the undo snapshot resolve the
>    marker again. cyrup now matches: no registry mutation on either word-delete path.
> 2. **`marker_covering` is the wrong seam and is deleted.** Upstream has no "is this column inside a
>    marker" predicate at all; atomicity is a property of the SEGMENTER, and `moveCursor`
>    (`editor.ts:1808-1830`), `handleBackspace` (`:1287-1290`) and `handleForwardDelete`
>    (`:1687-1690`) all step by `this.segment(text, "grapheme")`, which merges markers. So
>    `prev_grapheme`/`next_grapheme` now merge them too — which is the fix for the half of this item
>    the Verify section could not otherwise reach: **plain Left/Right arrows could park the caret
>    inside a marker**, where the next keystroke destroys it silently.
>
> **Tests** (RED at HEAD, GREEN after): `ctrl_w_at_a_marker_end_deletes_the_whole_marker` (RED:
> `left: "[paste #1 1500 chars"` — exactly the live repro),
> `alt_d_at_a_marker_start_deletes_the_whole_marker`, `word_motion_treats_a_paste_marker_as_one_unit`
> (asserts col 0 from the marker's end, per the REPRO-LOG correction),
> `arrow_keys_step_over_a_paste_marker_as_one_grapheme`.

**Kind** parity-bug · **Severity** critical · **Effort** S · **Confidence** **confirmed — reproduced in a live terminal, with the model's input read out of the session JSONL** · **observed 2026-08-13** (live-terminal; [`REPRO-LOG.md`](REPRO-LOG.md))

> **Reproduced 2026-08-13 verbatim, both halves.** One `Ctrl+W` at the end of a freshly-inserted
> `[paste #1 +40 lines]` (719 bytes of payload) deletes **exactly the single `]`**, leaving the
> 19-character `[paste #1 +40 lines`, and Enter sends that 19-character string to the model instead
> of the paste — read out of the session JSONL, not inferred. The mirror claim reproduced too:
> `Alt+Left` from the marker's end parks the caret between `lines` and `]`, and the next printable
> key writes into the marker, producing `[paste #2 +40 linesX]`.
>
> **One correction, to the Verify line below.** It asks to assert that `E::CursorWordBackward` from
> the marker's end "lands on the marker's **start** column, not one char short". Measured, cyrup
> lands **19 columns short, not one**: from col 20 the class run consumes only the `]` and stops at
> col 19. Read that clause as "lands at col 19 — it consumes only the closing `]` — instead of pi's
> col 0". The cyrup/upstream analysis and the Fix are otherwise exactly right.

**cyrup** — `crates/cyrup-tui/src/editor.rs:1074-1100` `word_left_target()` and `:1102-1128` `word_right_target()` classify purely by `is_word_char` (`:1637-1639`, `c.is_alphanumeric() || c == '_'`) and **never consult `marker_covering()`** — which exists at `:697-712` and is called from exactly two places, `backspace()` `:801-816` and `delete()` `:840-855`. `delete_word_backward()` `:874-884` and `delete_word_forward()` `:886-892` route straight through those two targets, and Ctrl+W / Alt+Backspace are bound to `E::DeleteWordBackward` at `keymap.rs:1002-1003`. Traced concretely on `[paste #1 +42 lines]` with the cursor at the end (col 20): the whitespace skip does not fire (`line[19] == ']'`), `want_word = is_word_char(']') = false`, and the class run consumes only `]` before hitting the word char `s` — so `take_range` (`:921-946`, itself marker-unaware) deletes exactly **one** character and leaves `[paste #1 +42 lines`. `marker_at` (`:663-694`) requires a closing `]`, so the text is no longer a marker and `expanded_text` (`:639-659`) emits it verbatim.

**upstream** — `pi/packages/tui/src/word-navigation.ts:9-14` declares `WordNavigationOptions.isAtomicSegment` — "Predicate identifying atomic segments that should be treated as single units (e.g. paste markers)"; `:44-46` (backward) and `:97-99` (forward) skip exactly one such segment whole, and the whitespace/punctuation loops at `:32-38`, `:59-66`, `:90-93`, `:105-113` all guard with `!isAtomic?.(…)`. Both consumers pass it: `packages/tui/src/components/editor.ts:1869-1889` `moveWordBackwards()` → `findWordBackward(currentLine, cursorCol, { segment: …, isAtomicSegment: isPasteMarker })` (the option at `:1886`), and `:2064-2083` `moveWordForwards()` (`:2080`). `:1588-1631` `deleteWordBackwards()` computes its delete range by calling `moveWordBackwards()`, so Ctrl+W inherits the atomicity. `isPasteMarker` is `:27-29` over `PASTE_MARKER_SINGLE` `:24`. v0.84.1 offsets: `:1881`/`:1898`, `:2076`/`:2092`, `:1600`; code identical.

**Impact** — Ctrl+W ("delete previous word") is the most-used editing key in a readline-style prompt, and the cursor sits at the end of a freshly-inserted marker. One press deletes the single character `]`; the prompt now reads `[paste #1 +42 lines` and the 42-line paste is unreachable — pressing Enter sends that 19-character string to the model instead of the file the user pasted. Nothing warns, and the visible text still says "+42 lines". Alt+D at the start of a marker does the mirror (`[` is eaten). In pi both keys remove the whole marker and drop the registry entry with it. Alt+B / Alt+Left / Alt+F also park the cursor *inside* the marker, where any subsequent keystroke corrupts it the same way.

**Fix** — in `word_left_target()` (`editor.rs:1074-1100`): after the whitespace skip, call `self.marker_covering(i)` and, when it returns `Some((s, _, _))` with `i > s`, return `(self.row, s)` immediately — the port of `findWordBackward`'s `isAtomic` branch (`word-navigation.ts:44-46`). Mirror it in `word_right_target()` (`:1102-1128`) returning `(self.row, e)` for `i < e` (`word-navigation.ts:97-99`). Then make `delete_word_backward()` / `delete_word_forward()` (`:874-892`) drop the registry entry for any marker fully inside the deleted range, as `backspace()` already does at `:814` — otherwise the atomic delete leaves an orphan `pastes` entry. **Land with TUI-042 and TUI-049**: all three are the paste-marker invariant, and fixing the deletion without tightening `marker_at` leaves the partially-chewed-marker case open.

**Verify** — `crates/cyrup-tui/src/tests/editor.rs`: paste 1,500 chars, assert `text() == "[paste #1 1500 chars]"`; send `E::DeleteWordBackward`; assert `text().is_empty()` and `pastes` no longer contains id 1 (today: `text() == "[paste #1 1500 chars"` and the content is orphaned). Same for `E::DeleteWordForward` with the cursor at col 0, and assert `E::CursorWordBackward` from the marker's end lands on the marker's **start** column — measured 2026-08-13, cyrup lands at **col 19** (it consumes only the closing `]`) where pi lands at col 0, so the assertion must name col 0 rather than "one char short".

## TUI-027 — `/tree` has no text search, and its four action keys are the characters pi types into that search

**Kind** not-ported · **Severity** critical · **Effort** M · **Confidence** **confirmed — the persistence half reproduced in a live terminal and read back out of the session JSONL** · **observed 2026-08-13** (live-terminal; [`REPRO-LOG.md`](REPRO-LOG.md))

> **Reproduced 2026-08-13 by typing the ordinary word `text` into `/tree`, one key at a time.** `t`
> toggled the timestamp column, `e` opened the inline label editor, and `x`+`t` were swallowed as
> label text. Enter appended a durable line to the session JSONL. The filter counter stayed at
> `(4/4)` for **every** keystroke, so there is no search state of any kind, and the hint row rendered
> exactly `z/x branch   e label   t label time`.
>
> **The measured artefact, recorded here so nobody re-derives it.** The persisted record is a
> top-level entry:
>
> ```json
> {"type":"label","id":"01dfd155","parentId":"471ccb39",
>  "timestamp":"2026-08-13T12:48:26.225394Z","targetId":"eeb2d0cf","label":"xt"}
> ```
>
> `targetId` is the entry under the cursor at the moment `e` was pressed — on a fresh session that is
> the `model_change` entry, i.e. **not even a message**. Nothing else in the item needed correcting.

> **Raised `high` → `critical` in the 2026-08-12 repair pass**, on the persistence half rather than
> the keybinding half. The mutation was re-verified end to end at HEAD: `handle_label_edit`'s confirm
> arm (`tree_selector.rs:540-546`) returns `SelectorOutcome::Apply(format!("{entry_id}{FIELD_SEP}{label}"))`
> after `update_node_label`, and the chrome does not merely repaint — `app/selectors.rs:201-208` splits that
> payload and `app/execute.rs:288-298` persists it through the **live** session path,
> `session.services().host_services.set_label(&entry_id, &label)` → `manager.append_label`, which is
> the same path a loaded extension's `setLabel` uses (stated in the comment at `:3763-3766`). So the
> characters a pi user types expecting a text search are appended to the session JSONL as that entry's
> label. README:106-107 classes corruption of persisted user data as `critical` with no reachability
> qualifier, and the trigger here is the ordinary act of typing into a picker.

**cyrup** — `crates/cyrup-tui/src/tree_selector.rs:850-889` `TreeSelector::handle`: after the label-edit capture it consults `self.keymap.action_for(key)` (fold / unfold / edit-label / toggle-timestamp), then treats bare characters `1`–`5` as filter-mode switches (`FilterMode::from_digit`, `:867-873`), then falls through to the shared select map. There is **no search state** — `rg 'search|query' crates/cyrup-tui/src/tree_selector.rs` finds only `LabelEdit.query` (`:347-348`), the inline label editor. The four defaults are `z` / `x` / `e` / `t` (`crates/cyrup-tui/src/keymap.rs:908-915`), advertised in the hint row at `tree_selector.rs:841-843` as `z/x branch   e label   t label time`. `TreeAction::from_id` (`keymap.rs:887-895`) knows only the four `app.tree.*` ids and none of the seven `app.tree.filter.*` ids. `app.message.copy` has no tree binding.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/components/tree-selector.ts:113` `private searchQuery = ""`, accumulated by the final `else` of `handleInput` at `:1093-1100` (`if (!hasControlChars && keyData.length > 0) { this.searchQuery += keyData; … }`), filtered at `:337`, cleared by `tui.select.cancel` at `:1032-1035` and by backspace at `:1079-1084`. Fold/unfold are `app.tree.foldOrUp` / `unfoldOrDown` = `["alt+left","ctrl+left"]` / `["alt+right","ctrl+right"]`; edit-label and toggle-timestamp are `shift+l` / `shift+t` (`pi/packages/coding-agent/src/core/keybindings.ts:119-134`). The five filter modes plus two cycle actions are `app.tree.filter.*` on ctrl+d / ctrl+t / ctrl+u / ctrl+l / ctrl+a and ctrl+o / shift+ctrl+o (`keybindings.ts:258-288`, handled at `tree-selector.ts:1039-1076`). `app.message.copy` copies the selected entry (`tree-selector.ts:1029-1031`). **All of this is present at the ported baseline** — `git show v0.83.0:…/tree-selector.ts` has `searchQuery` at `:113` and the filter ids at `:1039-1076`, so this is not drift.

**Impact** On a session with more than a screenful of entries `/tree` cannot be narrowed at all, and typing is pi's primary way to find a branch. The four characters cyrup binds are ordinary search input upstream, so a pi user typing `text` into `/tree` gets a label editor opened (`e`) and the timestamp column toggled (`t`). **The consequence is worse than a wrong keystroke:** once `e` opens the inline editor it captures *all* subsequent keys (`tree_selector.rs:852-854`), and Enter emits the captured text as an `Apply` payload (`:540-546`) which `app/execute.rs:288-298` **persists to the session JSONL** via `host_services.set_label` → `manager.append_label` — silent mutation of durable session data from what the user believed was a search, with the wrong entry (whichever row the cursor happened to be on) receiving it. Rebinding via `keybindings.json` cannot rescue it: the seven `app.tree.filter.*` ids are unknown to `TreeAction::from_id`, so a config carrying pi's defaults is silently ignored and the digit bindings stay. Note the interaction with **CFG-048** — a pi user's legacy keybindings file is not even name-migrated, so nothing they can write fixes this before the search lands.

**Fix** In `crates/cyrup-tui/src/tree_selector.rs`: add `search_query: String` to `TreeSelector`, append printable non-control characters in the fall-through arm (replacing the digit-filter arm), filter `filtered_nodes` on it, clear it on `SelectAction::Cancel` before cancelling, and pop on backspace — mirroring `tree-selector.ts:1079-1100`. In `crates/cyrup-tui/src/keymap.rs`: change `TreeKeymap::default` (`:908-915`) to pi's `alt+left`/`ctrl+left`, `alt+right`/`ctrl+right`, `shift+l`, `shift+t`; add the seven `app.tree.filter.*` ids to `TreeAction` / `TreeAction::from_id` with pi's ctrl defaults and route them to `set_filter` / cycle. Update the hint row at `tree_selector.rs:841-843` to resolve its labels from the live keymap.

**Verify** `crates/cyrup-tui/src/tests/tree_selector.rs`: typing `abc` into the tree filters to matching labels and fires no `TreeAction` and mutates no label; `shift+l` opens the label editor; `ctrl+u` switches to user-only; a `keybindings.json` rebinding `app.tree.filter.noTools` takes effect. Add one assertion specifically for the persistence half — after typing `abc` into `/tree`, no `SelectorOutcome::Apply` carrying `FIELD_SEP` is produced and `host_services.set_label` is never called. Then, per this workspace's standing rule, a **live terminal run**: open `/tree` on a real session, type a word, quit, reopen `/tree`, and confirm no entry gained a label.

## TUI-031 — A prompt typed during compaction is sent immediately instead of queued

**Kind** not-ported · **Severity** high · **Effort** M · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/app/submit.rs:13-25` `dispatch_submission` classifies the line and returns `AppAction::Submit(prompt)` after optimistically echoing it into the transcript; the run-loop arm at `app/run_action.rs:83-104` branches on `session.is_streaming().await` **only** — `steer` when streaming, `prompt_accepted` otherwise. `is_compacting` is never consulted from the TUI: `rg 'is_compacting' crates/ -g '*.rs'` finds the session accessor (`crates/cyrup-session-svc/src/session.rs:4108-4110`), one RPC status field (`crates/cyrup-modes/src/rpc.rs:1428`) and tests — no `cyrup-tui` hit. `rg 'compaction_queued|compactionQueued' crates/ -g '*.rs'` → zero; the concept does not exist.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:3023-3033` — the submit handler tests `if (this.session.isCompacting)` **before** the streaming branch: an extension command runs immediately, anything else goes to `this.queueCompactionMessage(text, "steer")` and returns. `queueCompactionMessage` (`:4230-4236`) pushes onto `compactionQueuedMessages` (declared `:475`), clears the editor, refreshes the pending-messages display and shows `"Queued message for after compaction"`. The queue is folded into `getAllQueuedMessages` (`:4162-4166`) and drained by `clearAllQueues` (`:4177-4183`). Present at v0.83.0.

**Impact** Compaction is a multi-second operation with no input lock in cyrup. **The session layer does not serialize behind it** — `AgentSession::prepare` (`crates/cyrup-session-svc/src/session.rs:849-900`) has no compaction guard either: extension-command dispatch, `input` event, then `if streaming { Prepared::Queued } else { Prepared::Run(...) }` at `:899`, and `prompt_with` (`:658-674`) hands `Prepared::Run` straight to `spawn_run` (`:681-689`). `is_streaming` reads the agent snapshot (`:3202-3204`), which compaction does not set (compaction aborts the active run, `session.rs:1391`). So a message typed during compaction is echoed into the transcript and dispatched as a fresh turn assembled from a context the compaction is in the middle of rewriting, with no status and no queue. pi holds it, tells the user it is held, and releases it afterwards. This also compounds TUI-016: even if the message did land in a queue, cyrup now has no surface that would show it.

**Fix** In the `AppAction::Submit` run-loop arm (`app/run_action.rs:68-82`), check `session.is_compacting()` before `is_streaming()`; when compacting, push the text onto a new `AppState::compaction_queue: Vec<(String, QueueMode)>`, clear the editor, push pi's `Queued message for after compaction` status, and suppress the optimistic transcript echo `dispatch_submission` then did in its `Dispatch::Prompt` arm — removed at HEAD, with the reason recorded in place at `app/submit.rs:22-30`. Drain the queue into `steer`/`follow_up` on `CompactionComplete` (`app/submit.rs:205-211`) and fold it into the `Dequeue`/`InterruptRestoreQueued` drains so Escape restores it too, matching `getAllQueuedMessages` / `clearAllQueues`.

**Verify** App test: with a session reporting `is_compacting = true`, submit `hello` — assert no `prompt_accepted` call is made, the status line reads `Queued message for after compaction`, and the text is not echoed as a user turn; on `CompactionComplete` assert it is delivered exactly once.

## TUI-004 — No live colour-scheme sync; `/reload` does not re-apply themes

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — Mode `2031` is never enabled and nothing consumes an unsolicited `CSI ? 997 ; N n`: `rg 2031 crates/` hits only the rationale prose at `crates/cyrup-tui/src/theme.rs:1483-1485`. Separately, `/reload` never touches the `ThemeController`: `rg 'apply_from_settings|set_registered_themes' crates/` → **zero**. The swap arm (`app/run_arms.rs:138-288`) re-reads `outputPad`, `hideThinking`, `showImages`, `imageWidthCells` and `editorPaddingX` and never touches the controller, which is owned by `crates/cyrup/src/main.rs:1590-1600` and consulted once at boot.

**upstream** — `pi/packages/tui/src/tui.ts:701, :731, :749` enable/disable mode 2031 and re-theme via `onTerminalColorSchemeChange`. `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:5734-5735` calls `setRegisteredThemes(...)` then `await this.themeController.applyFromSettings()` inside `handleReloadCommand`, immediately before `showLoadedResources` at `:5742`.

> The ADR-0001 substrate carve-out does **not** cover this. The probe machinery is faithful in what it *sends*; what it does with what comes back was not, until `stray_reply.rs` landed. Not enabling mode 2031 does not make its hazards moot — cyrup still issues the OSC-11 query and must handle a late reply.

**Impact** (1) Flipping the OS/terminal to dark mode mid-session leaves cyrup on the old palette until restart. (2) Editing a custom theme file and running `/reload` does not pick it up, and newly-registered extension themes are not re-registered.

**Fix** For (2): plumb the `ThemeController` into the app or expose a `ReapplyThemes` command, and in the `session_swapped` arm call `set_registered_themes(...)` then `apply_from_settings()` before the replay — the same place TUI-N02 adds the panel re-emit. Leave (1) as a recorded divergence in `theme.rs:1483-1485` unless crossterm gains an event.

**Verify** App test: `/reload` with a changed `settings.theme` and a newly-registered extension theme repaints to the new palette and lists the new theme.

## TUI-005 — Escape branches: bash-mode clear missing; bash child killed while streaming

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/app/input.rs:130-213` `Action::Interrupt`: `branch_summary_in_flight` early-returns, then `if self.state.transcript.bash_running() { bash_complete_simple(None, true); commit_bash(); }` — a **plain `if`, not an `else if`** — and only afterwards reads `let streaming = self.state.status.streaming`, with no bash-**mode** branch at all — as filed; at HEAD the chain is exclusive, the streaming branch is read first (`:162`) and the bash-mode branch exists (`:182`). The restore half is correct: `AppAction::InterruptRestoreQueued` → `drain_queue`, steering then follow-up, `restore_queued_to_editor`, then `abort()`.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:2766-2792` — four mutually exclusive `else if` branches: streaming → `restoreQueuedMessagesToEditor({abort:true})`; else bash child running → `abortBash()`; else `isBashMode` → clear the editor and exit bash mode (`:2773-2776`); else empty editor → double-escape. Because they are exclusive, pi never touches a bash child while streaming.

**Impact** Escape during a turn that also has a `!`-child kills the child as collateral — destructive, and the branch structure makes it unconditional. Escape in bash mode with a typed-but-unsent `!cmd` does nothing where pi clears it.

**Fix** Restructure `Action::Interrupt` (`app/input.rs:130-213`) into pi's exclusive chain: streaming first (return `InterruptRestoreQueued` without touching bash), then bash-child cancel, then a new bash-mode branch clearing the editor and dropping the `!` prefix, then the empty-editor double-tap (TUI-009). Land with TUI-009 so the precedence is right in one pass, and with TUI-031 so the compaction branch sits ahead of all four.

**Verify** App tests: (a) `!sleep 100` running **and** streaming, press Esc, assert the child is still alive and the queue restored; (b) editor holds `!foo`, nothing streaming or running, press Esc, assert the editor is empty and bash mode off.

## TUI-006 — `[Extension issues]` renders 2 of pi's 4 diagnostic sources

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed

**cyrup** — `rg 'command_diagnostics|shortcut_diagnostics|builtin_conflicts|CommandDiagnostic|ShortcutDiagnostic' crates/ -g '*.rs'` → **zero hits at HEAD**. `crates/cyrup-session-svc/src/services.rs:43-68` `StartupDiagnostics` carries only `{resources, extensions, models, flag errors}`, and `crates/cyrup/src/main.rs:1511-1568` `build_startup_report` maps exactly those. **One half did land since the baseline:** extension **tool** and **flag** conflicts are now recorded (`crates/cyrup-ext/src/registry.rs:222, :604`) and folded into `LoadExtensionsResult::errors` (`facade.rs:1108-1117`), so those reach the panel.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:1788-1792` — `[Extension issues]` is a four-source union: extension load errors, `getCommandDiagnostics()`, `getBuiltInCommandConflictDiagnostics()` (`:623`) and `getShortcutDiagnostics()`. Both remaining sources are live upstream: `core/extensions/runner.ts:296` `commandDiagnostics`, `:295` `shortcutDiagnostics` populated in `getShortcuts` at `:499-533` and exposed at `:539-541`.

**Impact** Two extensions registering `/deploy`, an extension shadowing built-in `/model`, or two extensions claiming the same shortcut are all silent. The user sees an extension "loaded" and one of its commands simply never runs.

**Fix** Add `commands: Vec<CommandDiagnostic>`, `builtin_conflicts: Vec<…>` and `shortcuts: Vec<ShortcutDiagnostic>` to `StartupDiagnostics` (`services.rs:43-68`); emit them from `cyrup-ext`'s registry disambiguation path and from `register_shortcut` (see TUI-N05); fold all three into the existing `[Extension issues]` heading in `build_startup_report` (`main.rs:1511-1568`). Secondary: scope-group the listing bodies and render `diagnostics.models` in the panel. Hand the `StartupDiagnostics` field additions to area 08.

**Verify** `crates/cyrup-tui/src/tests/startup_resources_panel.rs`: two extensions declaring the same slash command produce a conflict line; an extension shadowing a built-in produces the built-in-conflict line; both appear with `quietStartup=true`.

## TUI-008 — Seven upstream global keybinding ids are unbound

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/keymap.rs:96-114` `Action::from_id` recognizes exactly 14 ids; `app.model.select`, `app.thinking.toggle`, `app.message.copy` and `app.session.{new,tree,fork,resume}` are all absent, and `Keymap::default` binds no Ctrl+L / Ctrl+T / Ctrl+X. `app.session.toggleNamedFilter` **is** now handled, but only inside the session selector (`keymap.rs:801`).

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:2804-2814` wires all seven (`onAction("app.model.select", … showModelSelector)`, `"app.thinking.toggle"`, `"app.message.copy"`, `"app.session.new/tree/fork/resume"`), declared at `pi/packages/coding-agent/src/core/keybindings.ts:115-118` with defaults ctrl+l / ctrl+t / ctrl+x at `:85, :87-90, :99-102`.

**Impact** A user's `keybindings.json` carrying any of these ids silently does nothing, and the documented default chords are dead keys.

**Fix** Extend `Action` and `Action::from_id` (`keymap.rs:96-114`) with the seven ids and route them. `app.thinking.toggle` is mechanical — `TranscriptView::set_hide_thinking_block` exists and the settings row already persists `hideThinkingBlock`. `app.model.select` has its destination built with no key routed to it. The `app.pageUp`/`app.pageDown` spelling half of the original item moved to **TUI-028**, which owns the whole namespace question. **And the display half**: add the three `**Other**` rows the `[CYRUP-DELTA]` at `app/hotkeys.rs:131-136` withholds — `| ${selectModel} | Open model selector |`, `| ${toggleThinking} | Toggle thinking block visibility |`, `| ${copyMessage} | Copy last assistant message |` (pi v0.83.0 `interactive-mode.ts:5834-5839`, inside `handleHotkeysCommand`) — to the template at `app/hotkeys.rs:14-59`, deleting that `[CYRUP-DELTA]` note. The delta is legitimate only while the bindings are unported; closing this item without it leaves `/hotkeys` permanently three rows short of upstream with nothing tracking it.

**Verify** Keymap unit tests round-tripping each id, plus app tests asserting Ctrl+T flips `hide_thinking` and Ctrl+L opens the model selector. `/hotkeys` lists **Open model selector**, **Toggle thinking block visibility** and **Copy last assistant message** in the `**Other**` table, with real key cells rather than empty ones.

## TUI-009 — Double-Escape → tree/fork never implemented although `doubleEscapeAction` ships in `/settings`

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `rg 'last_escape|double_escape' crates/ -g '*.rs'` finds only the setting getter (`crates/cyrup-config/src/settings.rs:801-804`), the `/settings` row (`crates/cyrup-tui/src/app/settings_rows.rs:166-169`) and one unrelated test name in `stray_reply.rs`. `AppState` has no `last_escape` field (contrast `last_sigint`, used in the `Action::Clear` arm) and `Action::Interrupt` (`app/input.rs:130-213`) has no empty-editor branch.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:2777-2791`, the fourth `else if`: reads `getDoubleEscapeAction()` and applies a 500 ms window.

**Impact** A live, persisted, documented setting has no consumer: choosing `fork` or `tree` changes nothing.

**Fix** Add `last_escape: Option<Instant>` to `AppState` mirroring `last_sigint`; in the final `Action::Interrupt` branch, on a second Escape inside the 500 ms window dispatch `/tree` or `/fork` per `settings.double_escape_action()`. Must land with TUI-005 so the branch order matches pi.

**Verify** App test: two Escapes on an empty editor within the window emit the tree command; outside the window, nothing; with `none`, nothing.

## TUI-012 — No argument autocomplete for `/model <prefix>` or `/login <prefix>`

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/autocomplete.rs:28-36` `CompletionContext` is still exactly `{Slash, Path, Mention}`; `slash_context` (`:140-142`) bails on any whitespace in `before`. `has_arg_completion` (`crates/cyrup-tui/src/commands.rs:44`) still has only the two `const fn` constructors as writers (`:91`, `:105`) and **no reader**.

**upstream** — `pi/packages/tui/src/autocomplete.ts`'s argument-completion path, fed by `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:648` (`modelCommand.getArgumentCompletions = (prefix) => …`) and `:674` (the same for `loginCommand`), threaded through at `:699`.

**Impact** Typing `/model anth<Tab>` gives nothing; the user must know the exact `provider/model` string or open the selector. `has_arg_completion` is dead metadata claiming a feature that does not exist.

**Fix** Add `CompletionContext::CommandArg { command, prefix }` to `autocomplete.rs`, produced when `before` starts with `/`, names a command with `has_arg_completion`, and has exactly one whitespace run; feed it from the model catalog and provider list already reachable through the session services. Land alongside TUI-029, which needs the same seam.

**Verify** Autocomplete unit tests for `/model anth`, `/login op`, and a negative for a command without `has_arg_completion`.

## TUI-014 — Extension widgets (`ui.setWidget`) now reach the TUI and are stored where nothing renders them

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed

**cyrup** — The host sink is now installed (`crates/cyrup-tui/src/app/extension_ui.rs:137-144` `install_ui_sinks`), so `UiEffect::SetWidget` **does** reach the TUI — and is dropped into a field with no reader: `app/extension_ui.rs:336-346` `UiEffect::SetWidget { widget }`, declared at `app/state.rs:227`, initialised at `app/state.rs:347`, cleared at `app/extension_ui.rs:441`. `rg 'extension_widget' crates/` returns only those sites plus one test assertion (`tests/extension_ui_reset_on_swap.rs:36`) — no render function reads it, and `live_region_height` reserves no rows.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:2109-2160` `setExtensionWidget` mounts widgets as real components into `widgetContainerAbove` / `widgetContainerBelow` (`:492-493`, `:557-558`, `:879-894`), keyed by `key`, with a `placement` option and `MAX_WIDGET_LINES = 10` (`:2197`), updated in place at `:2196-2233`.

**Impact** An extension calling `ui.setWidget` gets a success return and draws nothing. Status widgets — the intended surface for long-running extension state — are unusable. Because the value is stored, a future implementation that starts reading the field would render a stale widget from an extension that has since been unloaded.

**Fix** Render `state.extension_widget` in the live region above the editor, keyed by `key` with pi's `placement` and a 10-line cap, and include its height in `live_region_height`. Land with TUI-033, which needs the same live-region plumbing.

**Verify** App test: an extension emits `ui.setWidget`, assert the text appears in the live region and that a second emit with the same key replaces rather than appends.

## TUI-015 — No render coalescing — one draw per streaming event, no frame budget

**Kind** cyrup-original · **Severity** medium · **Effort** M · **Confidence** confirmed

**cyrup** — **PARTIALLY CLOSED 2026-08-19 at HEAD `4fb5e40`; re-read before planning from the paragraphs below.** The COALESCING this item asked for landed as `TUI-092` F3 (`f22efab`): the session-event arm is now `App::on_session_event` (`crates/cyrup-tui/src/app/run_action.rs:242-299`), which seeds a `VecDeque` with the event that woke it, drains every further event already queued with `now_or_never()` (`:267-270`), folds them all, and ends in exactly ONE `self.draw_synchronized()?` (`:298`). The input arm (`:217-220`) and the bash arm (`app/run_arms.rs:351-355`) do the same. The two per-frame costs this item named are memoised too: the live region renders through `cached_render` (`transcript.rs:1228-1249`), keyed on `(render_generation, width, theme.generation)` (F2), and the image raster is a `protocol_cache` keyed on `ImageCacheKey` (`image.rs:58`, `:204-224`) (F7). **The residual is the TIME floor, and only that:** `rg 'MIN_RENDER_INTERVAL|needs_render|request_render' crates/cyrup-tui/src` still returns nothing, so N deltas arriving in N separate wakeups still cost N draws — the drain bounds a BACKLOG, not a rate.

**upstream** — `pi/packages/tui/src/tui.ts:343` `private static readonly MIN_RENDER_INTERVAL_MS = 16`, with `renderTimer` / `lastRenderAt` / `immediateRenderScheduled` at `:340-342` and `requestRender` coalescing on top of them.

**Impact** On a fast stream cyrup still redraws per wakeup where pi redraws at most every 16 ms. The per-draw cost is no longer the compounding half it was when this was filed — F2 and F7 removed the two recomputations named here — so what remains is the syscall/flush rate rather than unbounded render work.

**Fix** The drain half is done; add only the time floor. A `needs_render` flag plus a `MIN_RENDER_INTERVAL` (16 ms) `tokio::time::interval` arm in `App::run`: the arms above set the flag instead of calling `draw_synchronized` directly, and the timer arm performs the draw. **It must sit BELOW the swap arm** — `TUI-094` (`app/run.rs:283-292`) makes the run loop's arm order `cancel → input → swap → rest` load-bearing, and a new ticker goes in `rest`. The image-memoization clause of the original Fix is discharged by F7.

**Verify** Counter test on the time floor specifically: N streaming deltas delivered in N separate wakeups inside one 16 ms window produce one draw. The drain half already has one — `src/tests/run_loop_draw_coalescing.rs`.

## TUI-016 — A queued message is echoed into the transcript as if delivered, and has no queue surface at all

> **Retitled and restated 2026-08-13 against a live measurement.** This item was filed as
> *"Queued messages are now entirely invisible — texts discarded, footer count deleted"*. The
> absence half is confirmed exactly; the headline is **wrong in the direction that matters**, and a
> fix written to the old text would have made the bug worse. See the `Observed` block.

**Kind** parity-bug · **Severity** **high** *(raised from medium 2026-08-13: the observable is an affirmative wrong signal, not an absence)* · **Effort** M · **Confidence** **confirmed — reproduced in a live terminal against a streaming turn** · **observed 2026-08-13** (live-terminal; [`REPRO-LOG.md`](REPRO-LOG.md))

> **Observed 2026-08-13, tmux, against a genuine streaming turn.** (Harness note, because it was the
> hard part: the faux provider cannot stream from the binary — unscripted ⇒ `No more faux responses
> queued` immediately — so the turn was produced from a **local** fake `openai-completions` endpoint
> declared in the scratch agent dir's `models.json`, 60 SSE deltas one per second, bound to
> `127.0.0.1` only. No network.)
>
> **Confirmed:** across the entire 200-line scrollback there is **no** `{n} queued` footer segment,
> **no** pending-messages region above the editor, **no** `Steering:` / `Follow-up:` labelling and
> **no** dequeue hint. A `grep -in "queue\|steer\|follow"` over the whole scrollback returned only
> the two literal payloads that were typed.
>
> **Corrected:** cyrup does not leave the queue invisible — it **optimistically renders each queued
> message into the CHAT TRANSCRIPT as an ordinary user bubble** (`dispatch_submission`,
> `app/session_bind.rs:140`, calls `transcript.push_user(prompt.clone())` unconditionally before returning
> `AppAction::Submit`, whether or not the session then queues it). The user sees text that looks
> *delivered* while it is still sitting in a queue. Both messages were genuinely held — the first
> stream ran to completion at `tok59` before `QUEUEDMSGONE` was dispatched as a fresh turn.
>
> **Corrected:** "texts discarded" is true only of the **TUI's** copy. The session layer keeps them,
> proved by Escape restoring the still-queued second message verbatim into the editor.
>
> **Consequence for the Fix (load-bearing):** the transcript echo must be **removed at the same time**
> the pending-messages rows are added. Otherwise pi's `Steering: …` row and cyrup's phantom bubble
> both render and the message appears **twice**. See also **TUI-052**, the un-retracted echo, which
> survives this item's fix and `TUI-005`'s unless the echo site itself is removed.

**cyrup** — **Regressed since this item was filed.** `crates/cyrup-tui/src/app/events_fold.rs:189-194` `QueueUpdate` still calls only `status.set_queued(steering.len() + follow_up.len())`, discarding the texts — and the fidelity work then deleted the footer segment that displayed the count (TUI-FIDELITY C14): `grep -n 'queued' crates/cyrup-tui/src/status.rs` now returns only the doc lines `:76`, `:80-81` and the setter at `:149-150`, with **no render site anywhere**. The count is dead state and the queue has no surface of its own. The transcript echo that stands in its place is `app/session_bind.rs:140`.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:4190-4207` `updatePendingMessagesDisplay` renders per-message `Steering: {text}` / `Follow-up: {text}` `TruncatedText` rows above the editor plus the `↳ {key} to edit all queued messages` hint, fed from `getAllQueuedMessages` (`:4192`), which folds `compactionQueuedMessages` (`:4162-4166`).

**Impact** The user is told the opposite of the truth. The queued text appears in the transcript as a delivered user message, while no surface anywhere says it is queued: no count, no `Steering:`/`Follow-up:` distinction, and no notice that the queue can be edited — which matters because TUI-005's Escape restore is the action that hint advertises. A user who queues a message during a long turn and then walks away has no way to learn it was never sent; the screen already told them it was. This is also the concrete instance of the two-document drift called out in the header block: a fidelity fix removed the only surface a gap item depended on, and an optimistic echo — not nothing — took its place.

**Fix** Two halves, and **they must land together**. (1) *Remove* the optimistic echo: `dispatch_submission` (`app/session_bind.rs:140`) must not `push_user` a submission the session will queue — push it when the turn actually starts, as pi's streaming branch does (`interactive-mode.ts:2826-2833` clears the editor and calls `updatePendingMessagesDisplay()`; it never writes the text into the chat container). (2) Carry the message texts on `QueueUpdate` into `AppState`, render truncated per-message rows in the live region above the editor, and append the hint line resolved from the keymap. Fold TUI-031's compaction queue in through the same `getAllQueuedMessages` shape. Landing (2) without (1) renders every queued message **twice**.

**Verify** App test: two steering + one follow-up produce three labelled rows in the right order plus the hint line; the rows clear when the queue drains; and — the acceptance criterion added 2026-08-13 — **the transcript contains no user bubble for a message that is still queued**, at any point. Then a live terminal run against a streaming turn, per the standing rule: queue two messages mid-stream and confirm they appear only in the pending region until they are actually dispatched.

## TUI-017 — Attachment image strip: rasterizes without a protocol, invented placeholder, no 60-cell cap

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — Every site re-read at HEAD. `crates/cyrup-tui/src/image.rs:100-107` `from_capabilities` still installs `ProtocolType::Halfblocks` when `caps.images == None`; `:152-171` `render` takes the placeholder branch only on `!show_images`, a zero area, or an encode error; `:242-247` `placeholder_line` still emits the cyrup-invented `🖼 {label} ({w}×{h})`; `crates/cyrup-tui/src/app/render.rs:227-236` `render_images` passes `area.width` with no `image_width_cells` and no 60-cell cap. `image_fallback_text` (`image.rs:353-367`) — pi's real format — exists in the same file and is used only by the tool-result path.

**upstream** — `pi/packages/tui/src/components/image.ts:65` `Math.max(1, Math.min(width - 2, this.options.maxWidthCells ?? 60))` and `:114-118`, which emit one `imageFallback` line whenever `!caps.images`.

**Impact** Attaching an image on a terminal with no image protocol dumps a coloured half-block raster (unreadable on monochrome, ~20–30 rows of scrollback) instead of one `[Image: …]` line, and on a wide terminal the raster is unbounded where pi caps at 60 cells.

**Fix** In `image.rs`: (a) take the placeholder branch in `render` when `caps.images.is_none()`; (b) replace `placeholder_line`'s `🖼 …` with `image_fallback_text` from the same file; (c) clamp `cell_size` / `render_images` to `min(width - 2, image_width_cells)` with pi's 60 default. The tool-result path's own capability gate is TUI-N01 — different call site, different fix. `crates/cyrup-tui/src/tests/image.rs` currently pins (a) and (b) — see TUI-N08.

**Verify** With a no-protocol capability set, an attachment renders exactly one `[Image: …]` line; with a graphical set, the raster paints and never exceeds 60 cells.

## TUI-028 — Editor and input keybinding ids use an `editor.*` namespace upstream abandoned

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/keymap.rs:157-185` `EditorAction::from_id` matches **24** ids spelled `editor.cursorLeft`, `editor.cursorUp`, … `editor.pageUp`, `editor.newLine`, `editor.submit`, `editor.tab` — none namespaced `tui.`. They are merged from the user's file at `crates/cyrup/src/main.rs:1624-1626` → `crates/cyrup-tui/src/app/shell.rs:159` `load_keybindings_json`, which fans the same JSON out to six maps, each of whose `merge_json` silently ignores ids it does not recognize (`keymap.rs:487-489` and its five twins). cyrup also invents `app.pageUp` / `app.pageDown` (`keymap.rs:102-103`) and a whole `tui.autocomplete.{previous,next,accept,acceptSubmit,cancel}` family (`keymap.rs:606-613`). There is no legacy-name migration table: `rg 'migrat' crates/cyrup-tui/src/keymap.rs` → nothing.

**upstream** — `pi/packages/tui/src/keybindings.ts:9-32` declares `tui.editor.cursorUp` … `tui.editor.undo` (22 ids) and `tui.input.{newLine,submit,tab,copy}`, defaults at `:64-137`. That is already the spelling at the ported baseline (`git show v0.83.0:packages/tui/src/keybindings.ts` → `"tui.editor.cursorUp"` at `:9`, `"tui.editor.pageUp"` at `:19`). pi *migrates the older bare names forward*: `pi/packages/coding-agent/src/core/keybindings.ts:208-270` `KEYBINDING_NAME_MIGRATIONS` maps `cursorUp → tui.editor.cursorUp`, `pageUp → tui.editor.pageUp`, `newLine → tui.input.newLine`, applied by `migrateKeybindingsConfig` (`:294-311`) on every load. pi has no `app.pageUp`/`app.pageDown` and no `tui.autocomplete.*`: autocomplete navigation reuses `tui.select.up/down/cancel/confirm` and `tui.input.tab` (`pi/packages/tui/src/components/editor.ts:664-712`).

> **Refuter's caveat, adopted:** the count is 24, not 26, and the situation is worse than the auditor stated — cyrup's `editor.cursorLeft` matches **neither** pi's current `tui.editor.cursorLeft` **nor** pi's legacy bare `cursorLeft`, so a config written from either era of pi's documentation is inert. `tui.input.copy` also has no cyrup destination at all.

**Impact** A user who writes `~/.cyrup/keybindings.json` from pi's documented id list gets nothing for all 24 editor/input bindings — no error, no diagnostic, the defaults just stay. The autocomplete family is worse than inert: rebinding `tui.select.up` in cyrup moves selector highlights but **not** the autocomplete popup, because cyrup routes the popup through a separate invented map, so one user-visible action needs two different config keys.

**Fix** In `crates/cyrup-tui/src/keymap.rs`, rename the `EditorAction::from_id` arms to `tui.editor.*` / `tui.input.*` (keeping the old spellings as accepted aliases the way pi's migration table does); drop `app.pageUp` / `app.pageDown` in favour of `tui.editor.pageUp` / `pageDown` on the editor map (the deferral logic at `app/input.rs:63-80` stays); delete `AutocompleteAction::from_id`'s invented ids so the popup resolves through `SelectKeymap` + `tui.input.tab` as pi does. Optionally port `KEYBINDING_NAME_MIGRATIONS` into `keybindings_object` (`keymap.rs:32`).

**Verify** `crates/cyrup-tui/src/tests/keybindings.rs`: a JSON containing `{"tui.editor.cursorWordLeft": "alt+h"}` rebinds the editor; `{"tui.select.up": "ctrl+k"}` moves the autocomplete highlight as well as selector highlights; a round-trip test asserting every id in pi's `TUI_KEYBINDINGS` + `KEYBINDINGS` resolves to some cyrup action.

## TUI-029 — Extension autocomplete providers are never consulted by the interactive editor

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed

**cyrup** — The whole guest→host chain exists and terminates in nothing. `ExtensionHost::autocomplete_suggest` (`crates/cyrup-ext/src/host/live.rs:1327-1343`, whose doc names it as the port of pi's `AutocompleteProviderFactory` chain) has **no production caller** — `rg 'autocomplete_suggest' crates/ -g '*.rs'` returns the definition, the SDK guest side (`crates/cyrup-ext-sdk/src/{api.rs:844,guest.rs:312-318,macros.rs:120-124}`) and two tests (`crates/cyrup-ext-sdk/tests/ergonomic.rs:260,:267`, `crates/cyrup-ext/tests/wasm_provider.rs:115`). Host-side `add_autocomplete_provider` (`crates/cyrup-ext/src/host/services.rs:1468-1471`) only increments a counter read back by the guest (`:1474-1476`). The TUI's autocomplete is closed over three sync contexts with no extension seam: `crates/cyrup-tui/src/autocomplete.rs:28-36`, and `rg 'autocomplete' crates/cyrup-tui/src/app/` finds only `set_autocomplete_max_visible`, popup geometry and the Esc-cancel arm.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:2370-2373` `addAutocompleteProvider: (factory) => { this.autocompleteProviderWrappers.push(factory); this.setupAutocompleteProvider(); }`; `setupAutocompleteProvider` (`:723-738`) folds every registered wrapper over the base provider, unions their `triggerCharacters`, and installs the result on both the default and any custom editor (`:734-737`). Re-run at `:1886`, `:2182`, `:2372`, `:4444`, `:5739`. Present at v0.83.0.

**Impact** An extension that registers an autocomplete provider — the documented way to add trigger-character or `@`-style completions — produces zero suggestions in the shipped interactive mode, with a successful registration return. The capability is fully built on both the WIT and SDK sides and dies at the last hop, so the failure looks like a cyrup bug to the extension author and there is no diagnostic.

**Fix** Give `crates/cyrup-tui/src/autocomplete.rs` an extension pass: after `slash_context` / `path_context` / `mention_query` produce a base `Autocomplete` (or `None`), if `ext_host.autocomplete_provider_count() > 0` call `ExtensionHost::autocomplete_suggest(base, {lines, cursorLine, cursorCol, force})` from the run loop — spawned, not inline, following the discipline `AppAction::ExtensionShortcut` already uses at `app/run_action.rs:158`, since a guest can re-enter `ui.*` — and feed the folded result back into the popup through the existing extension→TUI command channel. Re-run the fold on session swap alongside `set_extension_shortcuts` (`app/run_arms.rs:276-277`).

**Verify** App test with a stub extension host whose `autocomplete_suggest` appends one item: typing a trigger character shows that item in the popup and accepting it inserts the guest-supplied text; with zero registered providers the popup is byte-identical to today's.

## TUI-030 — Nine `ExtensionUIContext` methods have no cyrup counterpart at all

**Kind** not-ported · **Severity** medium · **Effort** L · **Confidence** confirmed · **PARTIALLY CLOSED 2026-08-15**

**CLOSED 2026-08-15 — the four working-indicator verbs.** `UiEffect` gained `SetWorkingMessage`/`SetWorkingVisible`/`SetWorkingIndicator`/`SetHiddenThinkingLabel`; `LiveHostServices` overrides all four `HostServices` methods (they had been taking the trait's empty defaults, silently, in every mode — and because `cyrup-ext/src/host/live.rs` forwards the guest imports to those same methods, WASM guests were dead too). `App::apply_ui_effect` routes them onto `StatusIndicator::{set_working_message,set_working_visible,set_working_indicator}` and `TranscriptView::set_hidden_thinking_label`, all four are restored by `App::reset_extension_ui` (pi `resetExtensionUI`, `interactive-mode.ts:2210-2218` @v0.84.2, including its `(… to interrupt)` re-message of a live band), and the run loop re-arms its spinner tick from a custom `intervalMs`. `cyrup_modes::rpc::extension_ui_effect_json` returns `None` for all four **deliberately**: pi's own RPC mode gives them four empty bodies (`modes/rpc/rpc-mode.ts:179-193` @v0.84.2, "requires TUI loader access"). Proof: `crates/cyrup-tui/src/tests/extension_working_indicator.rs` (six tests driving the real `LiveHostServices` → effect sink → `apply_ui_effect` path) plus `the_working_indicator_family_reaches_the_effect_sink` in `crates/cyrup-session-svc/src/host_services.rs`.

**ALSO CLOSED 2026-08-15, in the same commit — the theme trio.** `getAllThemes`/`getTheme(name)`/`setTheme` (plus the `theme` getter) had their WIT imports and their `HostServices` methods, but **no live backend filled them**: `LiveHostServices` overrode none of the four, so all four took the trait defaults in every mode. They are now backed by `cyrup_session_svc::ThemeAccess`, implemented by `crates/cyrup-tui/src/theme_access.rs`'s `TuiThemeAccess` and attached by `App::install_extension_readbacks` — interactive-only, which is upstream's own gating (`createExtensionUIContext`, `interactive-mode.ts:2401-2417` @v0.84.2; every other mode gets `noOpUIContext`, `core/extensions/runner.ts:261-263`). The switch rides a dedicated `theme_switch_rx` run-loop arm rather than the `UiEffect` sink, because RPC installs that sink and pi's RPC `setTheme` is a hard-coded failure (`rpc-mode.ts:298-300`). Proof: `crates/cyrup-tui/src/tests/extension_theme_and_editor_readback.rs`.

**RESIDUAL** — `setEditorComponent` (`types.ts:260`) / `getEditorComponent` (`:263`), which need a WIT-world change reconciled with area 06. `onTerminalInput` was closed by EXT-021 sweep 6. Everything below this line is the ORIGINAL finding text, kept for provenance.

**cyrup** — `rg` across all of `crates/` returns **zero hits** for each of `set_working_message`, `set_working_visible`, `set_working_indicator`, `set_hidden_thinking_label`, `on_terminal_input`, `set_editor_component`, `get_all_themes`. `crates/cyrup-session-svc/src/host_services.rs:142-174` enumerates the complete `UiEffect` surface — Notify, SetStatus, SetWidget, SetHeader, SetFooter, SetTitle, SetEditorText, SetToolsExpanded — eight variants, none of them any of the above. The working-band copy is fixed at `crates/cyrup-tui/src/status_indicator.rs:176-188`; the hidden-thinking label is the constant `HIDDEN_THINKING_LABEL` used at `crates/cyrup-tui/src/transcript.rs:1226`. `set_theme` exists only as the TUI's own internal method (`app/shell.rs:415`), not as an extension capability.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:2344-2396` `createExtensionUIContext()` returns all of them: `onTerminalInput` (`:2350` → `addExtensionTerminalInputListener`, `:2120-2127`, with `rebindExtensionTerminalInputListeners` at `:828` and the clear at `:2174`), `setWorkingMessage` (`:2352-2357`), `setWorkingVisible` (`:2358`), `setWorkingIndicator` (`:2359`), `setHiddenThinkingLabel` (`:2093-2101`, `:2360`), `setEditorComponent`/`getEditorComponent` (`:2374-2375`), `getAllThemes`/`getTheme`/`setTheme` (`:2379-2392`, the last persisting through `settingsManager.setTheme`). All present at the ported baseline.

**Impact** Seven documented extension capabilities are unimplementable in cyrup. An extension cannot replace `Working...` with what it is actually doing, cannot hide the working band while it owns the screen, cannot relabel the hidden-thinking placeholder, cannot observe raw terminal input (the hook a terminal-integration extension needs), cannot supply a custom editor component, and cannot enumerate or switch themes. Because `resetExtensionUI` (`interactive-mode.ts:2175-2193`) restores all of them on reload, an extension written against pi will also leave cyrup in whatever state it reached through the six variants that do exist.

**Fix** Extend `UiEffect` (`crates/cyrup-session-svc/src/host_services.rs:142-174`) with `SetWorkingMessage{Option<String>}`, `SetWorkingVisible{bool}`, `SetWorkingIndicator{Option<Value>}`, `SetHiddenThinkingLabel{Option<String>}` and the theme trio, and handle them in `App::apply_ui_effect` (`crates/cyrup-tui/src/app/extension_ui.rs:299`) against `status_indicator`'s message/visibility, the transcript's hidden label and `set_theme`. Reset all of them from `rebind_session` (`app/extension_ui.rs:439-441`), mirroring `resetExtensionUI`. **`onTerminalInput` and `setEditorComponent` need a WIT/world change and must be scoped with area 06.**

**Verify** `crates/cyrup-tui/src/tests/extension_ui_effects.rs`: an extension setting a working message changes the band text and a later `None` restores `Working...`; `setWorkingVisible(false)` removes the band row; `setHiddenThinkingLabel("redacted")` changes the committed placeholder; all four reset to defaults across a session swap.

## TUI-032 — `/settings` is missing the `Warnings` and `Thinking level` submenus

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — The cyrup settings grid is `crates/cyrup-tui/src/app/settings_rows.rs:7-208`; its ids are `theme`, `compaction.enabled`, `terminal.showImages`, `terminal.imageWidthCells`, `images.autoResize`, `images.blockImages`, `enableSkillCommands`, `showHardwareCursor`, `terminal.clearOnShrink`, `editorPaddingX`, `outputPad`, `autocompleteMaxVisible`, `httpIdleTimeoutMs`, `hideThinkingBlock`, `collapseChangelog`, `quietStartup`, `enableInstallTelemetry`, `terminal.showTerminalProgress`, `steeringMode`, `followUpMode`, `transport`, `doubleEscapeAction`, `treeFilterMode`, `defaultProjectTrust`. There is no `warnings` row and no `thinking` row. `warnings.anthropicExtraUsage` **is** fully parsed and merged (`crates/cyrup-config/src/settings.rs:52, :879-890`, tests at `:2039`, `:2088`) with no editor. `SelectorKind::Thinking`'s confirm arm exists at `app/selectors.rs:343-350` but is **unreachable** — `open_selector` (`app/selectors.rs:7`) has exactly one call site, `app/selectors.rs:275`, which only ever constructs `SelectorKind::Theme`.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/components/settings-selector.ts:596-609` the `warnings` row (`submenu: … new WarningSettingsSubmenu(currentWarnings, …)`, body at `:138-165` with its single `anthropic-extra-usage` item) and `:610-631` the `thinking` row (`label: "Thinking level"`, `submenu: … new SelectSubmenu("Thinking Level", …, config.availableThinkingLevels, …, callbacks.onThinkingLevelChange)`). Both present at the ported baseline.

**Impact** The Anthropic paid-extra-usage warning cannot be turned off from inside cyrup even though the setting is honoured — the only route is hand-editing `settings.json`. And the thinking-level picker cyrup already built has no way in: Shift+Tab cycles blindly through the levels with no list of what they are or what they cost, which is exactly the affordance pi's submenu provides.

**Fix** Add two `SettingRow::submenu` rows to `settings_rows` (`app/settings_rows.rs:7-208`): `"warnings"` opening a toggle list over `eff.warnings()` and persisting through the existing `AppCommand::ApplySetting` path (`app/execute_misc.rs:137`), and `"thinking"` routed to the already-built `SelectorKind::Thinking` via `open_selector` — which also requires giving that arm a persisting return, the same change TUI-N03 needs for `Theme`.

**Verify** `crates/cyrup-tui/src/tests/settings_trust_selectors.rs`: opening `/settings` lists a `Warnings` row whose submenu toggles `anthropicExtraUsage` and writes it to the global layer; a `Thinking level` row opens the thinking selector and a confirm persists the level.

## TUI-033 — `ui.setHeader` / `ui.setFooter` are delivered to the TUI and dropped into fields nothing renders

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/app/extension_ui.rs:330-335` `UiEffect::SetHeader { content } => self.state.extension_header = Some(content)` and `UiEffect::SetFooter { content } => self.state.extension_footer = Some(content)`. Fields declared at `app/state.rs:213-217`, initialised `app/state.rs:345-346`, cleared `app/extension_ui.rs:439-440`. `rg 'extension_header|extension_footer' crates/` returns **only** those sites plus four test assertions (`tests/extension_ui_effects.rs:213-214`, `tests/extension_ui_reset_on_swap.rs:34-35`). Neither name appears in `render`, `region_constraints` or any chrome function — checked across the whole crate, not just `app/`.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:2362-2363` wires `setFooter` / `setHeader` into `setExtensionFooter` (`:2235-2257`) and `setExtensionHeader` (`:2262-2290`). The footer version clears `footerContainer` and swaps the extension component in for the built-in footer, restoring the built-in when the factory is `undefined` (`:2245-2254`); the header version splices the custom header into `headerContainer` in place of `builtInHeader` (`:2273-2290`). Both are reset by `resetExtensionUI` (`:2175-2176`). Present at v0.83.0.

**Impact** An extension replacing the header or footer gets a success return and nothing happens — the built-in footer keeps drawing. Because cyrup stores the value, the state also survives until the next `rebind_session`, so a future implementation that starts reading the field would suddenly render a stale header from an extension that has since been unloaded. This is the residue TUI-S01's closure left behind; TUI-014 scopes itself explicitly to widgets and does not cover these two.

**Fix** In `crates/cyrup-tui/src/app/render.rs`, render `state.extension_header` as the first row of the message region (replacing the compact startup block when present) and `state.extension_footer` in place of `StatusLine`'s rows in the footer region — the same swap semantics pi's containers give, including restoring the built-in when the value is cleared. Include the header's height in the layout constraints at `app/layout.rs:48` so it does not steal editor rows. Land with TUI-014, which needs the same live-region plumbing.

**Verify** `crates/cyrup-tui/src/tests/extension_ui_effects.rs`: an extension emitting `ui.setFooter` paints its text where the model/cwd footer was and clearing it restores the built-in footer; `ui.setHeader` paints above the transcript; both vanish across a session swap.

## TUI-034 — No markdown-transformer hook — extension transformers and pi's Mermaid renderer are both absent

**Kind** upstream-drift · **Severity** medium · **Effort** L · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/markdown.rs` exposes `render(text, width, theme)` / `render_with_hyperlink_support` / `render_with_default_style`, all funnelling into `render_inner(text, width, theme, color, italic, hyperlinks)` (`:126-176`) — no transform parameter and no hook; `rg 'transform' crates/cyrup-tui/src/markdown.rs` → nothing. `rg -i mermaid crates/` finds only two comments citing pi's `mermaid.ts:15` fence test (`markdown.rs:964-965`, `tests/markdown.rs:841`) — no renderer. There is no `mermaidRendering` settings row in the grid at `app/settings_rows.rs:7-208`, and no `MarkdownTransformer` anywhere in the extension host or WIT surface.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/components/markdown-transform.ts:3-10` `createMarkdownTransform(messageType, isStreaming, transformers)`, consumed by `new Markdown(..., { transform: createMarkdownTransform("assistant", …) })` at `components/assistant-message.ts:108-113` and `"assistant-thinking"` at `:146-162`, and inside `Markdown` at `packages/tui/src/components/markdown.ts`. The public API is `registerMarkdownTransformer` at `packages/coding-agent/src/core/extensions/types.ts:1292` (type at `:1153`, re-export `:1703`). Mermaid ships as one of these transformers: `components/mermaid.ts:60` `createMermaidMarkdownTransformer({getMode, theme})`, with the `mermaid-rendering` settings row (`values: ["off","final","streaming"]`) at `components/settings-selector.ts:540-546`. **New in the drift window** — `git show v0.83.0:packages/coding-agent/src/core/extensions/types.ts | rg MarkdownTransformer` → nothing; landed in pi `66534fbdc` / `05e89b418` for v0.84.0.

**Impact** ` ```mermaid ` fences render as a raw code block where pi draws a Unicode diagram, and the `mermaidRendering` setting a pi user expects is absent. More structurally, no extension can rewrite markdown before it is drawn — the single hook pi routes every assistant text and thinking section through, and the mechanism its own diagram support is built on.

**Fix** Add an optional transform callback to `render_inner` (`markdown.rs:157`) and thread it from `transcript.rs`'s assistant-text and `thinking_lines` (`:1232`) call sites, keyed by pi's `messageType`/`isStreaming` context. Then either port a Mermaid renderer as a built-in transformer behind a `mermaidRendering` settings row, or expose the hook to guests via a WIT `markdown-transform` export (**scope the WIT half with area 06**). LaTeX — the sibling v0.84.0 markdown feature — is already ported (`crates/cyrup-tui/src/markdown/latex.rs`, 2242 lines), so the precedent for the built-in half exists.

**Verify** Markdown unit test: a ` ```mermaid ` fence renders box-drawing output with `mermaidRendering=final` and a plain code block with `off`; a registered transformer that uppercases its input changes the rendered assistant text and does not change tool output.

## ~~TUI-037~~ — ~~`/reload` never persists an implicitly-granted project trust~~ **CLOSED 2026-09-04**

> ## CLOSED 2026-09-04 — `0e8c62fa` — `crates/cyrup-tui/src/app/reload_trust.rs`
>
> **What landed.** `App::maybe_save_implicit_project_trust` (`reload_trust.rs:126`) is the shell of pi's
> `maybeSaveImplicitProjectTrustAfterReload` (`interactive-mode.ts:4921-4941` @v0.84.4, upstream commit
> `38f18be44`): it reads the current session's `cwd`/`home`/`project_trusted`, runs
> `cyrup_config::trust::has_trust_requiring_resources`, and takes the pure decision
> `implicit_trust_after_reload(armed, cwd, project_trusted, has_resources, already_saved)`
> (`:85-102`) → `ImplicitTrustReload::{Keep, Disarm, Persist}` (`:64`) — pi's three exits, named by what the
> shell does (`Keep` = pi's two early `return false` with the arm kept, `Disarm` = an entry already exists,
> `Persist` = `trustStore.set(cwd, true)`). The store is opened only after the cheap guards, as pi's is
> (`:4923-4930`); `Persist` writes through `AgentSession::write_project_trust` — the same seam `/trust`
> uses — and disarms. The `/reload` arm (`app/execute_session.rs:279-303`) calls it and sets pi's status
> variant `Reloaded …; saved project trust` (`interactive-mode.ts:6000-6003`), which was `TUI-025`'s last
> residual. A store failure is pi's `catch` (`:4938-4940`): `Warning: Could not save project trust after
> reload: {e}` (framed as `showWarning` frames it, `:4264-4266`), the plain status, the arm kept — carried
> by the new `LifecycleEffects::warning` (`app/outcome.rs:104`) and pushed by `apply_lifecycle_outcome`
> (`app/channels.rs:181`) AFTER the swap, because `rebind_session` resets the transcript.
>
> **Arming.** pi's `autoTrustOnReloadCwd` (`main.ts:701-704`: no `--approve`/`--no-approve` AND no
> trust-requiring resources at boot → the session cwd) is computed at cyrup's composition root
> (`crates/cyrup/src/main.rs:672-677`, off the built session's own `cwd`/`home`), threaded through
> `run_interactive` (`crates/cyrup/src/interactive.rs:210`) to `App::set_auto_trust_on_reload_cwd`
> (`:346`), and held in `AppState::auto_trust_on_reload_cwd` (`app/state.rs:228`).
>
> **[CYRUP-DELTA] — ordering, recorded in the module doc.** pi runs the save AFTER `session.reload()`
> (`interactive-mode.ts:5995`) and can, because `AgentSession.reload` (`agent-session.ts:2811-2826`)
> calls `resourceLoader.reload()` with no trust options and that "preserves
> SettingsManager.projectTrusted" (`resource-loader.ts:404`). cyrup's `/reload` REBUILDS the session
> (`runtime.rs` `reload` → `factory.build` → `SessionBuilder::build`), which re-runs `decide_trust` from
> the store — with resources now present and nothing saved, the rebuilt session would fall to the prompt
> or to untrusted, and a post-rebuild `!isProjectTrusted()` guard would never save. So cyrup decides and
> writes BEFORE dispatching the rebuild: the inputs are identical (pi's post-reload `isProjectTrusted()`
> IS the pre-reload value; the resource scan and the store read are filesystem state the reload does not
> change), the store ends in the same state, and the rebuilt session reads the saved `true` back — the
> nearest cyrup can come to pi's carried in-memory trust. One observable difference: a reload that then
> FAILS has already written the entry, where pi would not have. **Design (DESIGN-GUIDANCE, FC/IS +
> domain enum):** the decision is a pure function over five explicit inputs, the I/O is the shell;
> `Result<bool, SessionServiceError>` keeps pi's boolean (business outcome) apart from the store failure
> (technical, → warning). Rejected: a post-rebuild call (dead in cyrup, above); a `SwapCaption::Deferred`
> caption resolved by the outcome arm (needed only for a post-rebuild call); pushing the warning from the
> `/reload` arm (wiped by the swap's transcript reset).
>
> **Tests.** `src/app/reload_trust.rs` — four decision-table cases. `src/tests/reload_implicit_trust.rs`
> — five App tests driving `AppCommand::Reload` against a real `AgentSessionRuntime` whose factory carries
> the `trust.json` store (the production `build_factory` shape), then reading back with `TrustStore`, the
> rebuilt session's `project_trusted`, and the committed scrollback: the item's own Verify clause
> (`reload_persists_an_implicitly_granted_project_trust`: store `cwd → Trusted`, status carries
> `; saved project trust`, rebuilt session trusted, arm dropped), no grant → store untouched, no
> resources → arm kept, a saved ancestor decision → file byte-identical + disarmed, and a `trust.json`
> directory → pi's warning + plain status + arm kept. **RED→GREEN established by running the App tests
> with `execute_session.rs` at HEAD (everything else in place):** 3 failed — `trust.json holds cwd → true:
> left: None, right: Some(Trusted)`, `pi's warning is missing`, `the arm is dropped once a decision is
> found` — and the two absence tests passed; all 9 pass after.
>
> **Residuals (none blocking).** (1) The pre-rebuild write on a reload that subsequently fails (above,
> low). (2) Whether cyrup's `/reload` should PRESERVE the in-memory trust decision like pi's
> `resourceLoader.reload()` — it re-decides through the builder, and for a project that gains resources
> with no saved decision and the interactive `trust_prompt` wired that could raise the pre-launch
> `TrustSelector` mid-session; not this row's mechanism, not filed here (session-svc/modes territory,
> low). (3) `TUI-025`'s row still names this residual as open; its text is the ledger agent's to strike.

**Kind** not-ported · **Severity** ~~medium~~ closed · **Effort** S · **Confidence** confirmed · **CLOSED 2026-09-04**

**cyrup** — `rg 'implicit_project_trust|implicit_trust|save_implicit' crates/ -g '*.rs'` → **zero hits**, and the `session_swapped` arm (`crates/cyrup-tui/src/app/run_arms.rs:138-288`) touches no trust state at all. `C::Reload` (`app/execute_session.rs:241-264`) only sets `pending_swap_status`.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:5746` calls `const savedImplicitProjectTrust = this.maybeSaveImplicitProjectTrustAfterReload();` inside `handleReloadCommand`. The body at `:4683-4706` checks `this.autoTrustOnReloadCwd === cwd`, then `isProjectTrusted() && hasTrustRequiringProjectResources(cwd)`, and writes `trustStore.set(cwd, true)` through a `ProjectTrustStore` — with an explicit `showWarning("Could not save project trust after reload: …")` on failure. The saved flag also feeds the reload status string variant TUI-025 tracks.

**Impact** A project the user trusted implicitly this session silently loses that decision across `/reload`, so its `.cyrup/` resources go back to being ignored — and with TUI-N04 open there is no banner to say so either. It fails closed, so it is not a security bypass, but it is a persisted security decision being silently dropped.

**Fix** Add a `maybe_save_implicit_project_trust_after_reload()` to `App` mirroring `interactive-mode.ts:4683-4706`: hold the `auto_trust_on_reload_cwd` that granted implicit trust, and in the `session_swapped` arm — the same place TUI-N02 adds the panel re-emit and TUI-004 re-applies themes — write it through the trust store when the cwd matches and the project still has trust-requiring resources (`cyrup::has_trust_requiring_project_resources`, `crates/cyrup/src/startup_ui.rs:340-344`), pushing pi's warning on failure. Land as one S with TUI-N02, TUI-004 and TUI-025, which all touch this call site.

**Verify** App test over a temp cwd with `.cyrup/skills/x.md` and implicit trust granted this session: after `/reload`, the trust store holds `cwd → true` and the reload status carries pi's `; saved project trust` variant; with no implicit grant, the store is untouched.

## TUI-044 — `undo()` discards the snapshot's cursor column — `Snapshot::col` is written and never read

> ## FIXED 2026-08-13 — `crates/cyrup-tui/src/editor.rs`
>
> `undo()` restores `snap.col` (clamped only as a bounds guard), calls `reset_preferred_col()` to
> match `this.preferredVisualCol = null` (`editor.ts:2022`), and calls `exit_history()` *before* the
> pop rather than inside the `if`, matching `:2017`. Shipped with TUI-042 as instructed, so `Snapshot`
> was corrected once. **Test** `undo_restores_the_snapshot_cursor_column` runs the item's own live
> scenario and asserts both readouts it names: the caret at `(0, 5)`, and the next keystroke producing
> `helloZ` (pi) rather than `heZllo` (cyrup at HEAD).

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** **confirmed — reproduced in a live terminal by two independent readouts** · **observed 2026-08-13** (live-terminal; [`REPRO-LOG.md`](REPRO-LOG.md))

> **Reproduced 2026-08-13, running the item's own scenario, twice from a clean editor.** `hello`,
> caret 5, `Ctrl+Y` yanks `world` (snapshot pushed at col 5); eight `Left`s to caret 2; undo. The
> buffer is correctly restored to `hello`, but the caret is at **column 2** — the live pre-undo
> column, clamped — not at the snapshot's column 5. Two readouts agree: the reverse-video caret cell
> in the rendered frame sits on index 2 (`he[SGR7]l[SGR0]lo`), and the next keystroke lands there,
> producing `heZllo` where pi produces `helloZ`. That is the wrong-edit path the item describes, not
> a cosmetic one.
>
> **No correction — the item is exactly right, including the concrete example.** One note for
> whoever writes the regression test, because it will otherwise mislead:
> **`tmux display-message -p '#{cursor_x}'` is not a valid instrument here.** cyrup hides the
> hardware cursor and paints its own caret as a reverse-video cell, so the pane's hardware cursor is
> stale write-position (measured: 6 on an empty editor, 4 with the caret at logical col 2, 10
> immediately after this undo). Read the SGR-7 cell out of `tmux capture-pane -e` instead.

**cyrup** — `crates/cyrup-tui/src/editor.rs:748-756`:

```rust
fn undo(&mut self) {
    if let Some(snap) = self.undo.pop() {
        self.lines = snap.lines;
        self.row = snap.row.min(self.lines.len().saturating_sub(1));
        self.col = self.col.min(self.cur_len());
        self.exit_history();
    }
}
```

The third line reads `self.col`, i.e. the **live pre-undo** column, and merely clamps it. `rg 'snap\.col' crates/cyrup-tui/src/editor.rs` returns nothing — the field populated at `:718` is dead. The `E::Undo` arm (`:1541-1546`) resets `last_action` and refreshes autocomplete but never touches `preferred_visual_col` (`:144`, cleared by `reset_preferred_col()` at `:601-603`), so a stale sticky goal column survives an undo too.

**upstream** — `pi/packages/tui/src/components/editor.ts:2016-2030` @v0.83.0 (v0.84.1 `:2028`): `undo()` does `Object.assign(this.state, snapshot.state)`, and `EditorState` is `{ lines, cursorLine, cursorCol }` (`:209-213`), so **both** cursor coordinates are restored from the snapshot. The same function then explicitly sets `this.lastAction = null; this.preferredVisualCol = null;` before firing `onChange`.

**Impact** — after Ctrl+-, the caret is wherever it happened to be rather than where the undone edit happened. Concrete: text `hello`, caret at 5, Ctrl+Y yanks `world` (snapshot taken at col 5); move the caret to col 2; Ctrl+-. pi puts the caret back at col 5 next to the restored text; cyrup leaves it at col 2. Because undo has already swapped the whole buffer underneath, the very next keystroke inserts or deletes at a position the user did not choose — a wrong-edit path, not merely a cosmetic one. The unreset `preferred_visual_col` compounds it: the first Up/Down after an undo can jump to a column from the pre-undo layout.

**Fix** — `editor.rs:750-754` — restore from the snapshot: `self.row = snap.row.min(self.lines.len().saturating_sub(1)); self.col = snap.col.min(self.cur_len());` (clamping only for safety, since `cur_len()` depends on the just-restored `row`). Add `self.reset_preferred_col();` inside `undo()` to match `editor.ts:2022`. **Fold into TUI-042's change** so `Snapshot` is corrected once.

**Verify** — `crates/cyrup-tui/src/tests/editor.rs`: set text `hello`, caret 5, `E::Yank` with `world` in the kill ring, then `E::CursorLeft` ×8 (caret 2), then `E::Undo`; assert `(row, col) == (0, 5)`. Second case asserting Up/Down immediately after an undo re-seeds the goal column from the restored caret.

## TUI-045 — An escape sequence split at the ESC byte across `read(2)` boundaries is not reassembled

**Kind** not-ported · **Severity** **high** *(raised from medium 2026-08-13: the mid-stream case aborts a live run and the trigger is a 60 ms gap on a LOCAL pty, not a slow transport)* · **Effort** M · **Confidence** **confirmed — reproduced in a live terminal, both idle and mid-stream** · **observed 2026-08-13** (live-terminal; [`REPRO-LOG.md`](REPRO-LOG.md))

> **Reproduced 2026-08-13, deterministically, first attempt, in both states.** Separate
> `tmux send-keys -H` calls with a sleep between them force separate `write(2)`s on the pty, i.e.
> separate `read(2)`s in crossterm — the exact "split at the ESC byte" form.
>
> * **control** (`1b 5b 41` in one write, at idle) → history recall; `Up` works.
> * **split at idle** (`1b`, 60 ms, `5b 41`) → swallowed Escape plus the literal characters `[A`
>   inserted into the prompt.
> * **split mid-stream** → `Operation aborted` — the streaming turn is killed at token 267 of 300 —
>   plus `[A` in the prompt.
>
> The item's own Verify ("hold an arrow key while a turn streams, and confirm no Escape-abort and no
> `[A` text") fails on **both** halves.
>
> **Correction to the reachability hedge below, which is too conservative.** It reads "on a local PTY
> a keypress is normally one write and one read, so the exposure is over SSH/mosh/tmux where the
> transport fragments". No SSH, mosh or throttled pipe was needed: **a 60 ms inter-write gap on a
> *local* pty is sufficient.** Any input source that does not deliver a sequence in a single write is
> exposed — not only slow transports. The crossterm analysis and the Fix hold unchanged.

**cyrup** — `crates/cyrup-tui/src/app/input_reader.rs:312-381` is the whole input pipeline: `event::poll(wait)` → `event::read()` → `StrayReplyFilter::push` → `map_event`. There is **no sequence-reassembly stage of cyrup's own**; the only buffering is crossterm's. crossterm 0.29.0 reassembles a split CSI correctly (`src/event/source/unix/tty.rs:247-268` pushes one byte at a time and keeps `self.buffer` on `Ok(None)`) — **except for a lone ESC**: `src/event/sys/unix/parse.rs:34-41` is `if buffer.len() == 1 { if input_available { Ok(None) } else { Ok(Some(Esc)) } }`, and `input_available` is `read_count == TTY_BUFFER_SIZE` (`tty.rs:149-154`, `TTY_BUFFER_SIZE = 1_024` at `:40`). So any read that does not fill 1,024 bytes and ends on `0x1B` emits `Esc` and clears the buffer; the sequence's tail arrives in the next read and is decoded as literal characters (`\x1b` `[` `A` → `Esc`, `Char('[')`, `Char('A')`). `stray_reply.rs:29-32` documents that cyrup **already observes this exact split form in the wild** ("When the reply is split across `read(2)` calls exactly at the `ESC` byte the opener instead arrives as `Key(Esc)` then `Key(']')`"), but its state machine only rescues an OSC 11 frame — every other sequence class falls through.

**upstream** — this is the entire reason `pi/packages/tui/src/stdin-buffer.ts` exists; its header `:1-18` states it: "stdin data events can arrive in partial chunks… Without buffering, partial sequences can be misinterpreted as regular keypresses", with a worked example of `\x1b` / `[<35` / `;20;5m` arriving as three events. `extractCompleteSequences` (`:192-255`) walks the buffer and returns `{ sequences, remainder }`; `process()` (`:371-386`) keeps the remainder and arms a `setTimeout(this.timeoutMs)` (default 10 ms, `:262`, `:284`) that flushes only if nothing completes it; `flush()` is `:400-414`. `isCompleteSequence` (`:29-78`) is the classifier that says a bare `ESC` is `"incomplete"` (`:34-36`) rather than a key. Identical at v0.84.1.

**Impact** — when it fires the user gets a spurious `Escape` **and** junk characters. Escape is not inert in cyrup: `app/run_action.rs:36` records that "an Esc during a turn silently discards every steering …" message and `app/input.rs:162-176` routes it to abort/interrupt, so a fragmented arrow key while a turn is streaming **aborts the run** and types `[A` into the prompt. Reachability is honest-but-real: on a local PTY a keypress is normally one write and one read, so the exposure is over SSH/mosh/tmux where the transport fragments, and for multi-byte sequences arriving back-to-back with other input. cyrup has no mitigation at all; pi has a 10 ms hold.

**Fix** — extend the machine already sitting on this path. `stray_reply.rs` (`:97-100`, `:165-193`) already holds a bare `Esc` in `State::Esc` and already has an idle flush driven by the reader thread's shortened poll (`app/input_reader.rs:330-336`, `HELD_FLUSH_INTERVAL = 20 ms` at `:25`). Generalise it: when `State::Esc` is followed by any of `[`, `O`, `]`, `P`, `_` with no modifier, do not replay — re-inject the held `Esc` plus the follower into the parser. crossterm exposes no such re-injection, so the tractable form is **a small pre-parser owned by cyrup between `read(2)` and crossterm** — a direct port of `extractCompleteSequences` plus the 10 ms flush, fed by a raw-fd reader like `terminal_query::read_reply` (`terminal_query.rs:408-447`), which is the same shape pi uses. That is a real design decision, hence effort M. **TUI-046, TUI-047 and TUI-050 all land on this pre-parser** — scope the four together.

**Verify** — unit: drive `input_pipeline` (`crates/cyrup-tui/src/tests/input_pipeline.rs:11`) with the two-chunk form and assert one `Up` arrives rather than `Esc` + `[` + `A`. Then, per this workspace's standing rule, a **live terminal run**: `ssh` into a box (or run under tmux with a throttled pipe), hold an arrow key while a turn streams, and confirm no `Escape`-abort and no `[A` text in the prompt. A TestBackend assertion alone does not close this item.

## TUI-046 — cyrup pushes Kitty keyboard flag 1; pi pushes 7 — and neither guard flag 7 requires exists

**Kind** parity-bug · ~~**Severity** medium~~ **Severity low** · **Effort** M · **Confidence** confirmed

> **PARTIALLY FIXED 2026-09-04 at `8bb0d22f` — still open (low). The flags decision is made and
> single-sourced; one residual remains and it is not portable at cyrup's seam.**
>
> **What landed.** `crate::keyboard_protocol::DESIRED_FLAGS` is now the only place the flag set is
> written — `DISAMBIGUATE_ESCAPE_CODES | REPORT_ALTERNATE_KEYS`, wire form `\x1b[>5u` — and
> `keyboard_protocol::push_flags()` is the only function that writes it. The three literals at
> `app/crossterm.rs` (startup `:55`, `App::suspend` `:133`, external editor `:223` as filed) are one
> call each, so a re-entry push can no longer disagree with the set startup negotiated. Fix parts (1)
> and (4) of the item are done; part (2) is the residual below; part (3) is now unreachable rather
> than unported.
>
> **`REPORT_ALTERNATE_KEYS` is the half with a user-visible effect, and it is the half that was
> missing.** crossterm substitutes a CSI-u's alternate (shifted) codepoint for the base keycode and
> clears `SHIFT` (`crossterm-0.29.0/src/event/sys/unix/parse.rs:597-606`), so a shifted key on a
> non-US layout now reports the character that layout produces — which is what a keybinding is
> matched against. The decoder cyrup owns for split sequences already handled the
> `unicode:shifted:base` shape (`escape_reassembly.rs::decode_csi_u_encoded_key_code`); the flag is
> what makes that shape reachable input, and a test now pins it.
>
> **`REPORT_EVENT_TYPES` (bit 2) is deliberately withheld — an argued `[CYRUP-DELTA]` in
> `keyboard_protocol.rs`'s module docs, which the item's own Fix (1) authorises.** Two reasons, both
> checked at both ends:
>
> 1. It buys cyrup nothing. Its only effect in crossterm terms is `Repeat`/`Release` reports
>    (`crossterm-0.29.0/src/event.rs:296-299`) and `map_event_on`
>    (`crates/cyrup-tui/src/app/input_reader.rs`) discards every `KeyEventKind::Release`, so no cyrup
>    code path consumes one.
> 2. Both guards pi needs *because of* it filter **raw bytes** ahead of pi's own key parser, and
>    cyrup's seam is one layer lower — after crossterm has already decoded. That is not a
>    "not-ported-yet"; there is no discriminator to port against:
>    * `pendingKittyPrintableCodepoint` (`stdin-buffer.ts:186-192`, `:399-408` @v0.84.4; pi
>      `bdb416cbc`, issue #3780) drops a raw character duplicating the Kitty CSI-u for the same
>      codepoint. crossterm decodes `\x1b[224u` (`parse.rs:540-568`) and a bare `à`
>      (`parse.rs:118-135`) into **byte-identical** `KeyEvent { Char('à'), NONE, Press, NONE }`
>      values. At the event level the guard degenerates to "drop the second of two identical
>      printable presses", which eats the second `l` of `hello`. The item's Fix (2) — "remember the
>      codepoint … and drop the next event that is exactly that bare character" — is therefore
>      **wrong as written for cyrup**, and that correction is this pass's finding.
>    * the WezTerm `\x1b\x1b`+CSI split (`stdin-buffer.ts:207-232`, test `:258-265`) keys off the
>      Kitty release report WezTerm sends for Escape, which exists only when event types are
>      requested; and crossterm collapses `\x1b\x1b` into one `Esc` event (`parse.rs:77`) before
>      `escape_reassembly` — TUI-045's machine, which operates on events by its own recorded
>      `[CYRUP-DELTA]` — can see it. Not asking for bit 2 makes that hazard **unreachable**: WezTerm
>      sends a plain `\x1b` for Escape and the existing path handles it.
>
> **The item's Impact paragraph was stale in one respect** and is corrected here: it predicted
> "Escape emits `Esc` followed by the literal text `[27;129:3u`" on WezTerm. Since TUI-045 landed
> (2026-08-14) that is not what would happen — `escape_reassembly` would hold the `Esc`, rebuild
> `\x1b[27;129:3u`, decode it to an Esc **release**, and `map_event_on` would drop it, so Escape
> would do *nothing at all*. Either way the conclusion stands and is now moot under the delta.
>
> **`drain.rs:11-16` corrected** (Fix part 4). It asserted that `DISAMBIGUATE_ESCAPE_CODES` makes the
> quit chord generate a *release* report; release reports need `REPORT_EVENT_TYPES`, which cyrup does
> not push. The drain's justification is unchanged — the CSI-u **press** report for `Ctrl+D`/`Ctrl+C`
> leaks into the shell the same way — and pi's protocol-disable-first ordering
> (`terminal.ts:370-377`, `:441-445`) is still what the module ports.
>
> **Design decision (no new type).** The invariant worth encoding is "every push site asks for the
> flag set startup negotiated", which one `const` plus one writer captures; `push_flags` is the seam
> the wire form is asserted at. A newtype over `KeyboardEnhancementFlags` would re-wrap a `bitflags`
> type that already rejects invalid bits; a typestate over push→query→pop would encode an ordering
> the module docs own and that has exactly one production caller. Migration cost: three call sites,
> two removed imports, two added `pub use` names; no serde, no behavioural API change.
>
> **Tests.** `keyboard_protocol.rs::cyrup_asks_for_disambiguate_and_alternate_keys_and_withholds_event_types`
> and `::push_flags_writes_the_csi_push_all_three_sites_share` were RED with `DESIRED_FLAGS` reverted
> to HEAD's `DISAMBIGUATE_ESCAPE_CODES` alone (`left: "\x1b[>1u"`, `right: "\x1b[>5u"`), green after;
> `::a_terminal_echoing_the_flags_cyrup_pushed_is_read_as_kitty` closes the push/read-back loop;
> `escape_reassembly.rs::a_split_kitty_alternate_key_sequence_resolves_the_layout_character` guards
> the newly reachable `unicode:shifted:base` shape (it passes with the flags reverted too, so it is a
> guard, not a red test). `cargo nextest run -p cyrup-tui` 1387 passed; clippy `-D warnings` and
> `RUSTDOCFLAGS='-D warnings' cargo doc` clean.
>
> **Residual — why this stays open at `low`.** The `pendingKittyPrintableCodepoint` dedup is
> unported and, per the byte-identity above, unportable while cyrup filters crossterm events instead
> of the bytes behind them. Its hazard is a terminal/layout quirk (pi observed it on Italian layouts
> at flags 7) rather than a flag-gated one, so withholding bit 2 does not prove it unreachable — only
> unobserved on cyrup. Closing it needs the same thing the item's Fix (3) needed and TUI-045
> deliberately did not build: a byte-level pre-parser owned by cyrup between `read(2)` and
> crossterm. **No live terminal run was performed this pass** (no kitty/ghostty/WezTerm available in
> this environment), so the item's `## Verify` live-run requirement is unmet and is the other reason
> the row is not struck.

**cyrup** — all three push sites push a single flag: `crates/cyrup-tui/src/app/crossterm.rs:55`, `:133`, `:223` — `PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)` (value 1). `rg 'KeyboardEnhancementFlags::' crates/cyrup-tui/src/` returns only those three lines: `REPORT_EVENT_TYPES` (2) and `REPORT_ALTERNATE_KEYS` (4) are pushed nowhere. `keyboard_protocol.rs:8-19` transcribes pi's composite query as `ESC [ > 7 u` and calls the push "the caller's (`PushKeyboardEnhancementFlags`, `crate::App::into_stdout`)" — but the caller pushes 1, so cyrup queries with `CSI ? u` after asking for a different flag set than its own module doc describes. `drain.rs:11-16` compounds it by asserting "With `DISAMBIGUATE_ESCAPE_CODES` pushed, the final Ctrl+D / Ctrl+C … also generates a release report" — release reports require `REPORT_EVENT_TYPES`, which cyrup never pushes. Separately, neither stdin-buffer behaviour that flag 7 makes load-bearing is ported: `rg` across `crates/` finds no equivalent of `pendingKittyPrintableCodepoint`, and crossterm collapses `\x1b\x1b` into a single `Esc` at `crossterm-0.29.0/src/event/sys/unix/parse.rs:76`, clearing the buffer so a following `[27;…u` is decoded character by character as literal text.

**upstream** — `pi/packages/tui/src/terminal.ts:15` @v0.83.0: `const DESIRED_KITTY_KEYBOARD_PROTOCOL_FLAGS = 7;` and `:17` `KITTY_KEYBOARD_PROTOCOL_QUERY = \`\x1b[>${…}u\x1b[?u\x1b[c\`` — disambiguate | report-event-types | report-alternate-keys. The two guards pi needs *because of* that choice both live in `stdin-buffer.ts`: **(a)** `parseUnmodifiedKittyPrintableCodepoint` `:184-190` + the `pendingKittyPrintableCodepoint` field `:280` + `emitDataSequence` `:389-398`, which drops a raw character duplicating the Kitty CSI-u for the same codepoint (pi commit `bdb416cbc` "fix(tui): deduplicate kitty printable input", closes #3780; pinned by `packages/tui/test/stdin-buffer.test.ts:227-246` — `\x1b[224uà` must emit only `\x1b[224u`, including across two chunks, while `\x1b[97ub` and `\x1b[64;3u@` must keep the raw char); and **(b)** the WezTerm split at `:207-230`, which emits a lone `ESC` and restarts from the second `ESC` when `\x1b\x1b` is followed by `[`/`]`/`O`/`P`/`_` (test `:201-213`: `\x1b\x1b[27;129:3u` must emit `["\x1b", "\x1b[27;129:3u"]`). Identical at v0.84.1.

**Impact** — today, with flag 1 only: no alternate-key reports, so on a non-US layout the shifted/base codepoint pi uses to resolve a keybinding is unavailable and bindings can fail to match; `drain.rs`'s stated rationale is unfounded for the release-event half; and `keyboard_protocol.rs` reports a negotiation over a flag set cyrup never asked for. **The moment anyone corrects the flags to 7 to close that — a one-token change that looks obviously right — the two missing guards bite immediately:** every composed/accented character is typed **twice** on a Kitty-protocol terminal (issue #3780's exact symptom), and on WezTerm the Escape key emits `Esc` followed by the literal text `[27;129:3u` into the prompt. Filing the three parts as one item is the point — the flags change is unsafe alone.

**Fix** — decide the flag set explicitly and land the parts as one change. (1) `app/crossterm.rs:55`, `:133`, `:223` → `DISAMBIGUATE_ESCAPE_CODES | REPORT_EVENT_TYPES | REPORT_ALTERNATE_KEYS`, matching `terminal.ts:15`; if any flag is deliberately withheld, say which and why in `keyboard_protocol.rs`'s `[CYRUP-DELTA]` block (`:33-42`) beside the existing `modifyOtherKeys` delta. (2) Port `pendingKittyPrintableCodepoint` (`stdin-buffer.ts:184-190`, `:389-398`) into the reader thread next to `StrayReplyFilter` (`app/input_reader.rs:312-381`): remember the codepoint of the last unmodified Kitty printable event and drop the next event that is exactly that bare character. (3) The `\x1b\x1b`+CSI split needs **TUI-045's pre-parser** — crossterm consumes `\x1b\x1b` before cyrup can see it, so it cannot be fixed at the event level. (4) Correct `drain.rs:11-16`'s premise once the flags are settled.

**Verify** — unit: feed the pipeline `\x1b[224u` then `à` in two chunks and assert one character reaches the editor; feed `\x1b[97ub` and assert two. Then a **live terminal run** on kitty/ghostty (composed characters via a dead key are not doubled; a non-US layout binding resolves) and on WezTerm with `enable_kitty_keyboard` (Escape aborts the turn and types nothing). Cross-check the negotiated value with `keyboard_protocol::current()` in the startup diagnostics.

## TUI-051 — `/reload` never re-reads `keybindings.json`, while both its help text and its in-source comment claim it does

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** (as filed) — `grep -rn load_keybindings_json crates | grep -v /tests/` returned exactly two lines: the definition (`crates/cyrup-tui/src/app/shell.rs:159` at HEAD) and one call on the boot path before `app.run` (`crates/cyrup/src/main.rs:1975`); the sweep-1 fix added the third caller, `App::reload_keybindings_from` (`app/shell.rs:192-209`). The `/reload` handler at `crates/cyrup-tui/src/app/execute_session.rs:241-264` calls only `rt.reload(None).await` — nothing re-reads `<agent_dir>/keybindings.json` and nothing resets the keymaps. **Two in-source statements assert the opposite**: the comment at `app/execute_session.rs:242-244` ("re-reads settings/resources/keybindings") and the command's own description at `crates/cyrup-tui/src/commands.rs:70`, `"Reload keybindings, extensions, skills, prompts, and themes"` — which cyrup prints in the `/` menu. Separately, `Keymap::merge_json` (`keymap.rs:487-493`) only *sets* the ids present in the document and never restores a default for an id the user removed, so even a wired-up reload would leave a deleted entry's old binding live.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:5386` @v0.83.0 — inside `handleReloadCommand`, immediately after `await this.session.reload(...)`, pi calls `this.keybindings.reload()`. That is `core/keybindings.ts:354-357` → `setUserBindings(KeybindingsManager.loadFromFile(this.configPath))` → `loadFromFile` (`:363-367`), which re-reads the file, re-runs `migrateKeybindingsConfig` (`:366`) and hands the result to `packages/tui/src/keybindings.ts:167-192` `rebuild()`. `rebuild()` **replaces** rather than merges: for every id in `definitions`, `userKeys === undefined ? normalizeKeys(definition.defaultKeys) : normalizeKeys(userKeys)` (`:187-191`), so removing an entry from the file restores its default on the next `/reload`.

**Impact** — the single documented way to apply an edited `keybindings.json` does nothing. A user follows the `/reload` help string cyrup itself prints, sees `reloaded resources`, and the new binding is not live — the only remedy is restarting the process, which nothing tells them. The stale comment at `app/execute_session.rs:242-244` means the next auditor reading the handler concludes it works. Note the compounding with **TUI-025/CFG-047**: `/reload`'s description is *also* one baseline behind, so the string is wrong in two independent ways.

**Fix** — give `App` the resolved `keybindings.json` path (it already receives `agent_dir` transitively via `session.services()`), and in the `C::Reload` arm (`app/execute_session.rs:241-264`) re-read and re-apply the file after `rt.reload` succeeds — pi's ordering (session reload first, then `keybindings.reload()`). Add a `reset_to_defaults()` to each of the six maps and call it before merging so a removed entry restores its default, matching `packages/tui/src/keybindings.ts:187-191`'s replace semantics; today's merge-only path cannot un-bind. Correct the comment at `app/execute_session.rs:242-244`. **Must land together with CFG-048** (this is pi's second application site for the name migration) and with **CFG-038** (a bad spec on reload must not wipe the live keymap).

**Verify** — `crates/cyrup-tui/src/tests/keybindings.rs` with a real temp `agent_dir`: boot against a `keybindings.json` binding `app.tools.expand` to `ctrl+e`, assert ctrl+e expands; rewrite the file to `ctrl+y`, dispatch `/reload`, assert ctrl+y expands and ctrl+e does not; delete the entry entirely, `/reload` again, assert the stock ctrl+o default is back. Because this is TUI work, verification must also include a **live terminal run** — edit `~/.cyrup/keybindings.json`, type `/reload`, and observe the new chord working without restarting.

## TUI-N01 — Tool-result images rasterize on terminals with no image protocol

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/transcript.rs:1275` is still `let inline = images.show && !run.images.is_empty() && run.images.iter().all(|i| i.block.is_some());` — the gate consults `terminal.showImages` and decodability only, never a capability. `ImageOpts` (`transcript.rs:1309-1324`) has since gained `expand_key`, `cwd` and `tools_expanded` but still carries no `graphical`/capability field, so `App::detect_image_support`'s `state.image_renderer` (`app/shell.rs:375-401`) cannot reach the gate.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/components/tool-execution.ts:331-334`: `const caps = getCapabilities(); … if (caps.images && this.showImages && img.data && img.mimeType)` — no protocol means no `Image` child at all, and `getTextOutput` supplies the one-line `imageFallback`. The same rule at the component level, `pi/packages/tui/src/components/image.ts:70, :114-118`.

**Impact** On a plain xterm, the Linux console, CI or a pipe, a `read` of a screenshot dumps ~20–30 rows of coloured `▀` into scrollback where pi prints one `[Image: shot.png [image/png] 1920x1080]` line. TUI-007's closing commit asserts a capability fallback that does not exist, so this defect is invisible to anyone reading history.

**Fix** Add `graphical: bool` (or the whole `TerminalCapabilities`) to `ImageOpts` (`transcript.rs:1309-1324`), seed it from `AppState::image_renderer.is_graphical()` wherever `show_images` is already pushed into the transcript, and require it in the `inline` gate at `transcript.rs:1275` so a no-protocol terminal takes the existing `push_image_fallbacks` branch. Keep the half-block raster for the graphical case — that ADR-0001 rationale is sound and orthogonal. Land with TUI-036, which needs the same capability plumbed into the settings grid.

**Verify** Extend `crates/cyrup-tui/src/tests/tool_result_images.rs`: with a no-protocol capability set, a finished `read` whose result carries a PNG commits exactly one `[Image: [image/png] WxH]` line and paints zero coloured cells; with a graphical capability set the raster still paints.

## ~~TUI-N02~~ — ~~`/reload` does not re-emit the loaded-resources / diagnostics panel~~ **CLOSED 2026-09-04**

**Kind** not-ported · **Severity** ~~medium~~ · **Effort** S · **Confidence** confirmed · **Status** **CLOSED 2026-09-04**

> **CLOSED 2026-09-04 in `605f483c`; the body below is kept for traceability and is STALE from its first sentence.**
> The row was struck the same day but this section was not, so for a day the ledger asserted both that the panel is
> pushed on every swap and, here, that `push_loaded_resources` "has exactly **one** production call site" and that the
> swap arm "never pushes the panel". Caught by the batch-3 review; struck 2026-09-05.
>
> **What is true at HEAD.** The panel is a pure projection of the session it describes:
> `StartupReport::from_session(&AgentSession, verbose)` (`crates/cyrup-tui/src/startup.rs:178`), reached through
> `App::push_session_loaded_resources` (`crates/cyrup-tui/src/app/session_bind.rs:507-511`). The `session_swapped` arm
> pushes it at `app/run_arms.rs:338` — after `install_extension_shortcuts` (`:322`), which is what RECORDS the
> EXT-039 reserved-key refusals the panel folds into `[Extension issues]`, and before the replay, which is where the
> boot path already put it (`crates/cyrup/src/interactive.rs:256`, `:282`). The Fix paragraph below asked for
> `build_startup_report` to move out of the binary and named `cyrup_tui::StartupReport::from_session` as one option;
> that is exactly what landed, and no `build_startup_report` exists at HEAD. The dead name nevertheless survived in the
> Open-items row's first draft (as the never-existing `StartupResourcesPanel::from_session`), in `06-cyrup-ext.md`'s
> EXT-039 row and in one `cyrup-it` comment until the review pass corrected all three.
>
> **Upstream re-read at v0.84.4.** `showLoadedResources({force: false, showDiagnosticsWhenQuiet: true})` is called
> from `bindCurrentSessionExtensions` (`interactive-mode.ts:1982`) — boot AND every replacement, via
> `rebindCurrentSession` behind the runtime's `setRebindSession` hook (`:576-578`) — and from `handleReloadCommand`
> (`:5991-5994`) with the identical options object; `setupExtensionShortcuts` immediately precedes it at both sites
> (`:1981-1982`, `:5990-5991`). `showListing = force || verbose || !quietStartup` (`:1702`) is `StartupReport::show_listing`,
> and `force` is dead upstream at this tag (both call sites pass `false`), so `--verbose` is cyrup's only override.
> Nothing gates the emit by swap reason, so cyrup does not either.
>
> **Tests** (`crates/cyrup-tui/src/tests/startup_resources_panel.rs`):
> `the_panel_is_derivable_from_a_session_and_survives_quiet_startup`,
> `set_verbose_startup_overrides_quiet_startup_at_the_session_seam`, and the source-text guard
> `the_session_swap_arm_pushes_the_panel_after_the_shortcuts_and_before_the_replay` (the crate cannot construct a
> `RunCtx`, so the arm's ordering is pinned the way `run_loop_swap_arm_reachable.rs` pins its own). All three were red
> before the change in the strongest sense — none of the three seams they call existed.
>
> **Deltas from pi, recorded not hidden.** pi clears `loadedResourcesContainer` before each render (`:1699`) and pins
> it above `chatContainer` (`:594-596`); cyrup's committed entries are terminal scrollback and cannot be re-rendered,
> so the push is placed before the replay to reproduce the stacking and a second swap APPENDS a second panel. The same
> root cause makes one further case visible: the push at `run_arms.rs:338` precedes the arm's re-entrancy generation
> guard (`:370`), so a swap superseded mid-await leaves a panel for the abandoned session above the newer one's.
> Same class as TUI-N07's append-below-the-previous-session replay; not re-filed.
>
> **Scope defect in the landing commit.** `605f483c` also carried EXT-003's `with_wasm` fallback
> (`crates/cyrup-session-svc/src/builder.rs`) and its test, naming EXT-003 nowhere but an inline comment. That work is
> EXT-003's and is struck under its own row in `06-cyrup-ext.md`; it is noted here only so the commit's contents and
> the ledger agree.

**Superseded text follows.**

**cyrup** — `App::push_loaded_resources` (`crates/cyrup-tui/src/app/session_bind.rs:306-308`) has exactly **one** production call site: `crates/cyrup/src/main.rs:1669`, the boot path. (`rg 'push_loaded_resources' crates/ -g '*.rs'` otherwise returns the transcript sink at `transcript.rs:919` and three test call sites.) The `session_swapped` arm (`app/run_arms.rs:138-288`) re-subscribes, re-titles, refreshes auth and context, re-installs sinks, rebuilds the command registry, re-reads seven settings and replays the conversation — and never pushes the panel. `C::Reload` (`app/execute_session.rs:241-264`) only sets `pending_swap_status`.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:5742-5745` — inside `handleReloadCommand`, after `session.reload`, `keybindings.reload`, `setRegisteredThemes` and `themeController.applyFromSettings`, pi calls `showLoadedResources({ force: false, showDiagnosticsWhenQuiet: true })` — the identical options object it uses at boot (`:1890`).

**Impact** `/reload` is the command a user runs right after editing an extension, skill or prompt. If the edit broke the extension, shadowed a skill name or introduced a prompt conflict, cyrup says only `reloaded resources` and the diagnostics are never shown. The data is re-collected server-side by the factory and discarded.

**Fix** In the `session_swapped` arm, after the settings re-read and before the replay, push the panel for the swapped-in session. `build_startup_report` is private in `crates/cyrup/src/main.rs:1511`; move it to `cyrup-session-svc` or expose it as `cyrup_tui::StartupReport::from_session` behind the existing services accessors. pi does not gate it by swap reason, so neither should we initially. Pair with TUI-004, TUI-025 and TUI-037, which touch the same call site.

**Verify** In `crates/cyrup-tui/src/tests/startup_resources_panel.rs`, drive a `session_swapped` whose services report one extension load error and assert `[Extension issues]` lands in committed scrollback even with `quietStartup=true`.

## TUI-N03 — A theme chosen in `/settings` is applied live but never persisted

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** (as filed) — `crates/cyrup-tui/src/app/selectors.rs:330-336`: `SelectorKind::Theme => { self.set_theme(...); self.state.transcript.push_status(...); None }`. Returning `None` meant no `AppCommand::ApplySetting` reached the persist arm (`C::ApplySetting`, `app/execute_misc.rs:137`). The design was stated in the function's own doc; that block is now the TUI-N03 fix note (`app/selectors.rs:318-329`) and the arm returns `Some(AppCommand::ApplySetting { … })`. The submenu at `app/selectors.rs:273-275` was then the only `open_selector` call site (`rg 'open_selector\(' crates/cyrup-tui/src` → definition at `app/selectors.rs:7`; at HEAD there are two calls, `:275` Theme and `:279` Thinking).

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts` `onThemeChange: (t) => { this.settingsManager.setTheme(t); void this.themeController.applyFromSettings(); }`, with `onThemePreview` kept as the separate, non-persisting hook — pi distinguishes preview from confirm; cyrup treats confirm as a preview that sticks until exit.

**Impact** The only in-app way to change theme does not survive the session. Worse in combination with TUI-004: `ThemeController::sync_with_terminal` persists a high-confidence OSC-11 detection into `settings.theme` only when the setting is **unset** — exactly the state a never-persisted user choice leaves behind — so the next launch writes the auto-detected theme over the user's choice.

**Fix** Give `confirm_selector`'s `Theme` arm an `AppCommand::ApplySetting { id: "theme", value }` return instead of `None`; the arm at `app/extension_ui.rs:123` already persists to Global and pushes the status. Keep `set_theme` for the immediate repaint. TUI-032 needs the same change for `SelectorKind::Thinking` — do both in one pass.

**Verify** App test: open the settings selector, drive the `theme` submenu to `light`, confirm, assert an `ApplySetting{id:"theme", value:"light"}` command is emitted, plus a settings-layer assertion that the global layer holds `"theme": "light"`.

## TUI-N04 — The untrusted-project warning banner is never rendered at startup

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** (as filed) — `rg 'This project is not trusted|project_trusted' crates/cyrup-tui/src crates/cyrup/src/main.rs` returned only the `/trust` dialog's `TrustSelector::new(cwd, saved_label, session.services().project_trusted, …)` — `app/execute.rs:219` at HEAD — with no startup banner anywhere and a boot path that pushed the panel and replayed with no trust check. Both halves already existed unused: `AgentSessionServices::project_trusted` and `cyrup::has_trust_requiring_project_resources` (`crates/cyrup/src/startup_ui.rs:504`). **The banner exists at HEAD**: `App::render_project_trust_warning_if_needed` (`app/session_bind.rs:127`) pushes `PROJECT_UNTRUSTED_WARNING` (`app/settings_rows.rs:288`) from both bind paths (`app/run_arms.rs:123`, `:272`), covered by `crates/cyrup-tui/src/tests/project_trust_banner.rs` — so this open row belongs with the four the header note flags for a substantive re-audit rather than a citation repair.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:3699` — `renderInitialMessages()` calls `renderProjectTrustWarningIfNeeded()`, body at `:3710-3723`, emitting a warning-styled `This project is not trusted. Project {CONFIG_DIR_NAME} resources and packages are ignored. Use /trust to save a trust decision, then restart pi.`

**Impact** Open cyrup in a repo shipping `.cyrup/` skills, prompts, themes or settings that has not been trusted, and those resources are silently ignored with no indication and no pointer to `/trust`. It is the surface that tells the user a security decision is in force.

**Fix** After `push_loaded_resources` and before the replay in `crates/cyrup/src/main.rs:1669`, and in the `session_swapped` arm alongside the other per-session re-reads, evaluate `!services.project_trusted && cyrup::has_trust_requiring_project_resources(cwd)` and push a warning-styled entry with pi's string rebranded (`.cyrup`, `/trust`, `cyrup`). Reuse the existing warning startup role rather than inventing one. Pairs with TUI-037, which owns the other half of the trust lifecycle.

**Verify** App test over a temp cwd containing `.cyrup/skills/x.md` with `project_trusted = false`: committed scrollback contains `This project is not trusted` in warning style; absent when `project_trusted = true` or when the cwd has no trust-requiring resources.

## TUI-002 — Thinking blocks: fold-ordering and the visible-content spacer

**Kind** parity-bug · **Severity** low · **Effort** M · **Confidence** confirmed

> **Half closed.** `thinking_lines` now routes through the markdown renderer — `crates/cyrup-tui/src/transcript.rs:1232` `crate::markdown::render_with_default_style(body, width.max(1), theme, style.fg, true)` — matching pi's `new Markdown(thinkingBlocks.join("\n\n"), …, {color, italic:true})` at `assistant-message.ts:144-162`. Only the ordering and spacer halves remain.

**cyrup** — `crates/cyrup-tui/src/transcript.rs:3176-3186` `thinking_text` `filter_map`s **every** `Content::Thinking` in the message and joins with `\n\n` — no adjacency, no index order — and `app/events.rs:132-140` commits that single blob before `commit_assistant`. The replay path repeats the fold at `app/session_bind.rs:192-196`. There is no `has_visible_content_after` spacer rule.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/components/assistant-message.ts:104-166` walks `message.content` in index order, keeping each **run of adjacent** thinking blocks as its own section (run collector at `:115-127`) interleaved with text sections, and adds a blank only when more visible assistant content follows (`:133-137`, `:163-165`).

**Impact** On interleaved-thinking models all reasoning is hoisted above all prose instead of appearing as the model produced it; spacing differs at the end of a message. `crates/cyrup-tui/src/tests/thinking.rs` exercises only adjacent blocks, so no test pins the divergence either way.

**Fix** Change `thinking_text` to return `Vec<String>` of adjacent-run sections and have `app/events.rs:132-140` (and the replay walk at `app/session_bind.rs:192-196`) commit them interleaved with text sections in content order, keeping the now-correct markdown call per section. Add the `has_visible_content_after` spacer condition.

**Verify** Extend `tests/thinking.rs` with an interleaved `[think, text, think, text]` message and assert commit order and the trailing-blank rule.

## TUI-003 — Replay omits the compaction-count status

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `rg 'Session compacted|compaction_count' crates/ -g '*.rs'` → zero hits at HEAD. The swap arm (`crates/cyrup-tui/src/app/run_arms.rs:260-267`) replays the conversation and pushes no compaction status; the boot replay does the same.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:3706` — `this.showStatus(\`Session compacted ${times}\`)`, emitted from the initial-render path after `renderInitialMessages`.

**Impact** A resumed session that has been compacted gives no indication that earlier context was summarized away, so the user cannot explain why the model has forgotten something.

**Fix** Expose the compaction count on the session accessor already used by the replay, and push a status line after the replay completes when it is non-zero.

**Verify** `crates/cyrup-tui/src/tests/session_replay.rs`: a session whose context carries two compaction summaries produces `Session compacted 2` after the replayed conversation; a fresh session produces nothing.

## TUI-010 — Ctrl+O pushes no `Tool output: …` status

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

> **Half closed.** Committed tool blocks now honour the live expand flag (`crates/cyrup-tui/src/transcript.rs:2769` `tool_lines(run, images.tools_expanded, …)`, with `ImageOpts::tools_expanded` documented at `:1317-1323` as pi's live `toolOutputExpanded`), and branch/compaction summaries render collapsed with an expand hint (`transcript.rs:2873, :2876-2891`).

**cyrup** — `crates/cyrup-tui/src/app/input.rs:234-251` `Action::ToolsExpand` toggles and returns `AppAction::Redraw` with **no status**. The `Tool output: expanded|collapsed` string exists in the codebase only on the extension effect path (`app/extension_ui.rs:323`).

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:2805` routes Ctrl+O through `toggleToolOutputExpansion` (`:4028`) → `setToolsExpanded` (`:4032`), whose body ends at `this.showStatus(\`Tool output: ${expanded ? "expanded" : "collapsed"}\`)` (`:4047`) — the **same** function the extension path uses, so the echo is unconditional upstream apart from a no-op early return. `setToolsExpanded` also calls `setExpanded(expanded)` on the active header (`:4036-4039`), which cyrup has nothing to do (see TUI-N06 for the committed-entry constraint and TUI-038 for the fan-out).

**Impact** Ctrl+O gives no feedback that it did anything, and the same user-visible action produces a status when an extension triggers it and none when a keystroke does.

**Fix** Push `Tool output: expanded|collapsed` from `Action::ToolsExpand` (`app/input.rs:234-251`), reusing the string already built on the extension path so the two cannot drift. Land with TUI-038, which restructures the same arm.

**Verify** App test: Ctrl+O pushes the status string with the correct word in both directions, and the extension path continues to push the identical string.

## TUI-011 — `/changelog` is a hardcoded stub; no "What's New" startup notice

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/app/submit.rs:111-114` is verbatim `"changelog" => { self.state.transcript.push_block("What's New", "No changelog entries found."); AppAction::Redraw }`. `collapseChangelog` is a live settings row with no consumer (`app/execute_misc.rs:321`), and `last_changelog_version` a live accessor with none.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:102` imports `getChangelogPath, getNewEntries, normalizeChangelogLinks, parseChangelog`; the startup notice gated on the last-seen version is at `:1178-1188` and the `/changelog` command at `:6056-6057`.

**Impact** `/changelog` always claims there is nothing, and two settings with persisted state have no effect. Users are never told what changed on upgrade.

**Fix** Embed the changelog at build time (or read it from the install dir), parse per-version sections, render honouring `collapseChangelog`, and at startup compare the current version against `last_changelog_version`, showing the notice and then persisting the version.

**Verify** Unit test on the parser plus an app test: with `last_changelog_version` behind, boot emits the notice and persists the current version; a second boot does not.

## TUI-013 — Quoted paths with spaces break `@`-mention autocomplete

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/autocomplete.rs:168` `const PATH_DELIMS: [char; 5] = [' ', '\t', '"', '\'', '=']`; `:201-206` `trailing_token` splits on the **last** delimiter (`before.rfind(PATH_DELIMS)`), so `see @"my dir/fi` yields `dir/fi` and `mention_query` fails its `strip_prefix('@')`. `rg 'unclosed_quote|find_unclosed' crates/cyrup-tui/src/autocomplete.rs` → zero.

**upstream** — `pi/packages/tui/src/autocomplete.ts` `findUnclosedQuoteStart` / `extractQuotedPrefix` / `extractAtPrefix` — pi scans back for an unclosed quote first and treats everything after it as one token.

**Impact** Any path containing a space cannot be completed via `@`, on project layouts where such paths are common.

**Fix** Port `findUnclosedQuoteStart` into `autocomplete.rs` and consult it in `trailing_token` (`:201-206`) before falling back to `rfind(PATH_DELIMS)`; make `mention_query` accept the quoted form. **There is an in-repo precedent to reuse rather than reinvent:** `crates/cyrup-tui/src/session_search.rs:149, :179, :183` already implements an unclosed-quote scan (`had_unclosed_quote`).

**Verify** Autocomplete unit tests for `@"my dir/fi`, `@'my dir/fi`, and a closed-quote negative.

## TUI-018 — Startup header has no logo/version line and no expanded body

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed

> **Half closed.** pi's `compactOnboarding` and the standing `onboarding` line now exist — `crates/cyrup-tui/src/chrome.rs:98-114` (`compact_onboarding`, `STARTUP_ONBOARDING`) against `interactive-mode.ts:943-951`.

**cyrup** — (a) The **logo** line is absent. pi's collapsed header body is `${logo}\n${compactInstructions}\n${compactOnboarding}\n\n${onboarding}` (`interactive-mode.ts:952-953`) where `logo = theme.bold(theme.fg("accent", APP_NAME)) + theme.fg("dim", \` v${this.version}\`)` (`:910`); cyrup's `compact_hint_entries` (`chrome.rs:154-184`) emits blank / bar / compact-onboarding / blank / onboarding / blank with **no logo entry**, and `chrome.rs:127-130` states outright "cyrup does not draw the `logo` part, so 1 + 4 + 1 = 6". The app name and version therefore appear nowhere in the UI. (b) The **expanded** body (`expandedInstructions`, 19 hints, `interactive-mode.ts:915-934`) has no counterpart and no `ExpandableText` state — `state.show_startup_hints` is a boolean that is only ever cleared (`app/session_bind.rs:135`).

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:910-953`.

**Impact** New users get a terse bar and no orientation, and the version is not visible anywhere in the UI — the first thing anyone is asked for in a bug report. pi's `Press {ctrl+o} to show full startup help` affordance still has nothing to toggle.

**Fix** Add a logo entry to `compact_hint_entries` (`chrome.rs:154-184`) rendering `APP_NAME` bold-accent plus ` v{CARGO_PKG_VERSION}` dim, and add an expanded body carrying pi's 19 hints behind an `expanded` flag in `AppState`, routed from Ctrl+O alongside TUI-010's tool expansion, matching pi's shared toggle.

**Verify** App test: default boot shows the logo line with the crate version; Ctrl+O shows the expanded hints; a second Ctrl+O collapses.

## TUI-019 — No alt-screen UI mode, mouse, scrollbars, prompt navigation

**Kind** upstream-drift · **Severity** medium · **Effort** L · **Confidence** confirmed

> **Re-rated `low` → `medium` in the 2026-08-12 repair pass, and the ADR-0001 justification struck.**
> The prior text read "Severity stays low as a deliberate ADR-0001 divergence". That is not admissible
> here for two independent reasons. **(1) The ADR does not exist in this workspace** —
> `PARITY-GAPS.md:709` records it as unreadable, and README:208-212 says a code comment or item
> invoking an ADR id to justify a divergence is an unverifiable claim, not a decision of record.
> **(2) Even a real ADR would not hold the rating down**, because README:213-215 and PARITY-GAPS both
> state that there is no accepted-divergence category and that a mechanism difference which *costs
> behaviour* stays on the list as work. This one costs behaviour, and the cost is now filed
> concretely in two adjacent items: **SEAM-051** — `--tui-mode regular`, the **default** value of a
> v0.84.1 flag, is not in `crates/cyrup/src/cli.rs`, so it is captured by `partition_extension_flags`,
> becomes an error-severity diagnostic, and makes the binary exit 1 with a message claiming the option
> is unknown; and **CFG-021** — the `tuiMode` / `fullscreenScrollbar` settings keys are modelled
> nowhere. The rating below is therefore judged **on consequence**, per README:106.
>
> **Consequence, stated plainly.** Within this item's own scope the losses are: no fullscreen mode, no
> mouse scrolling, no scrollbar, and no jump-to-previous-prompt — four features a user reaches for on
> a normal path, with native terminal scrollback as the only substitute. That is not data loss, a
> permission bypass or a crash, so it is not critical; it is squarely more than "`get_tree` drops
> `labelTimestamp`", so it is not low. **Medium.** The *launch failure* consequence is deliberately
> **not** counted here — it belongs to SEAM-051, and counting it twice would inflate the ledger.
>
> **This item also carries a genuine open question that is not a severity question** — whether cyrup
> builds an alt-screen mode at all is a scope decision no human has made in a readable document. It is
> recorded in `## Open questions — decisions required` below rather than encoded as a low severity.
> Note the asymmetry that makes the decision non-optional: SEAM-051 and CFG-021 must be fixed
> **whatever** is decided — a flag that exits 1 on its own default value and two settings keys that
> parse nowhere are defects under either answer.

**cyrup** — `rg EnterAlternateScreen crates/cyrup-tui/src` → only the pre-session wizard (`startup_selector.rs:20, :44`), never the chat UI. No `uiMode` / `tuiMode` / `fullscreenScrollbar` settings row in the grid (`app/settings_rows.rs:7-208`), and no `tui.altScreen.*` ids in the keymap.

**upstream** — pi v0.84.1 added `packages/tui/src/tui-alt-screen.ts` (1047 new lines), `tui-main-screen.ts` (586), `components/scroll-view.ts` (195), `components/alt-screen-flash.ts` (51), `layout.ts` (402), `stack.ts` (154), the eight `tui.altScreen.*` keybinding ids (`packages/tui/src/keybindings.ts:43-50`), runtime mode switching (`switchTuiMode`, `interactive-mode.ts:788-841`), and the `tui-mode` (`regular|fullscreen`) + `fullscreen-scrollbar` (`auto|always|hidden`) settings rows (`components/settings-selector.ts:633-643`).

**Impact** No fullscreen mode, no mouse scrolling, no scrollbar, no jump-to-previous-prompt. Users on the inline viewport rely entirely on native terminal scrollback, and there is no in-product way to ask for anything else. The two surfaces through which a user would *request* the missing mode are separately broken and separately owned: the CLI flag makes the binary exit 1 (SEAM-051) and the settings keys parse nowhere (CFG-021).

**Fix** Large and architectural; the drift window is why the effort is L+. Two parts, and the first is not contingent on the decision. **(a) Unconditional, ship regardless:** accept `--tui-mode <regular|fullscreen>` in `crates/cyrup/src/cli.rs` and model `tuiMode` / `fullscreenScrollbar` in `cyrup-config` — SEAM-051 and CFG-021 own these, and until they land a pi command line cannot even start cyrup. Accepting the flag and honouring only `regular` (rejecting `fullscreen` with a message that names the gap) is a legitimate interim that costs no design work and removes the launch failure. **(b) Contingent on the decision below:** add an alt-screen `App` variant behind `tuiMode` owning its own scroll state, with mouse capture, a scrollbar and semantic-prompt navigation, keeping the inline path as default — porting the behaviours of `tui-alt-screen.ts`, `scroll-view.ts`, `layout.ts` and the eight `tui.altScreen.*` ids rather than pi's renderer.

**Verify** For (a): `cyrup --tui-mode regular` starts a session, and a `settings.json` carrying `tuiMode` round-trips through `/settings`. For (b), once scoped: mouse wheel scrolls the transcript, the scrollbar tracks position under each of `auto`/`always`/`hidden`, and the prompt-navigation chord jumps between user turns — each confirmed in a **live terminal run**, since this is TUI work and TestBackend cannot show viewport behaviour. The prior text's "at minimum a decision recorded in `lib.rs`'s ADR notes" is **withdrawn**: an in-source ADR note is not a verification step, and per README:208-212 it would not be a decision of record either.

## TUI-020 — OSC-8 hyperlinks: capability now consulted, still never emitted

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

> **Auditor closure overturned.** The auditor closed this on `markdown.rs:126` passing `crate::image::hyperlinks_supported()` and `:142-148` taking the capability explicitly. The capability is now consulted — but **nothing is emitted**, which is the item's title.

**cyrup** — `crates/cyrup-tui/src/markdown.rs:133-141` says so in its own words: "ratatui … has no channel for an OSC-8 escape … so the capable branch here emits the link text alone, matching upstream's VISIBLE row exactly while omitting the (unrepresentable) clickable wrapper." Confirmed by grep — the only `\x1b]8` bytes anywhere in `crates/cyrup-tui/src` are that comment at `markdown.rs:136` and two strip-ANSI test fixtures at `ansi.rs:247, :292`. The image half is untouched: `crates/cyrup-tui/src/image.rs:353-367` `image_fallback_text` pushes `shorten_image_path(name)` bare.

**upstream** — `pi/packages/tui/src/components/markdown.ts:692-696` `if (getCapabilities().hyperlinks) { result += hyperlink(styledLink, token.href) … }`; `pi/packages/tui/src/terminal-image.ts:646-652` wraps the fallback filename in `hyperlink(display, pathToFileURL(filename).href)` when `getCapabilities().hyperlinks && isAbsolute(filename)`.

**Impact** Links are not clickable on terminals that support it. What *did* land is dropping the noisy ` ({href})` parenthetical — a real improvement, and the visible row now matches pi — but the clickable wrapper is still absent, and per this ledger's no-accepted-divergence rule a mechanism gap that costs behaviour stays on the list.

**Fix** ratatui cannot carry the escape inside a styled `Span`, so emit it the way cyrup already emits other raw sequences outside the buffer (`terminal_title.rs`, `terminal_progress.rs`, `drain.rs` are three in-repo precedents): wrap the link text in `\x1b]8;;{href}\x1b\\ … \x1b]8;;\x1b\\` at flush time for committed rows, gated on `hyperlinks_supported()`. Do the same for `image_fallback_text`'s filename with a `file://` URL.

**Verify** Markdown unit test over the flushed byte stream (not the ratatui buffer): with `hyperlinks = true` the output contains the OSC-8 wrapper and no parenthetical; with false, exactly today's output.

## TUI-021 — Cache-miss notices not implemented

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed

**cyrup** — `rg 'cache_miss|CacheMiss|showCacheMissNotices' crates/ -g '*.rs'` → one hit, an unrelated subagents test name (`crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs:1050`). No `cache-miss-notices` row in the settings grid (`crates/cyrup-tui/src/app/settings_rows.rs:7-208`). The footer computes a per-turn cache-hit rate but nothing warns on a miss.

**upstream** — Live at v0.84.1: the settings row at `pi/packages/coding-agent/src/modes/interactive/components/settings-selector.ts:547`, the notice renderer at `interactive-mode.ts:3568, :3623, :3670, :4403`, and the `onShowCacheMissNoticesChange` hook at `:4487-4488`.

**Impact** A prompt-cache miss — often caused by an edit to a system prompt or a tool-set change, and directly expensive — passes unremarked.

**Fix** Port pi's miss detection into the session layer, add a `showCacheMissNotices` settings row, and push a transcript notice from the usage-update arm in `app/events_fold.rs`.

**Verify** App test: two turns whose usage shows cache creation without a read emit exactly one notice; with the setting off, none.

## ~~TUI-025~~ — ~~Slash-command metadata one baseline behind~~ **CLOSED 2026-09-04**

**Kind** stale-port · **Severity** low · **Effort** S · **Confidence** confirmed

> **CLOSED 2026-09-04 (batch-2 ledger audit).** Sweep 1 (2026-08-14) landed the three `commands.rs` literals and the `/reload` status sentence; the one residual — the `; saved project trust` variant, blocked on `TUI-037` — landed with `TUI-037` at `0e8c62fa`: `crates/cyrup-tui/src/app/execute_session.rs:299` pushes `Reloaded keybindings, extensions, skills, prompts, themes, and context files; saved project trust` when `App::maybe_save_implicit_project_trust` returns `Persist` (pi `interactive-mode.ts:6000-6003` @v0.84.4), pinned by `crates/cyrup-tui/src/tests/reload_implicit_trust.rs`. The text below is the filing, retained.

**cyrup** — All three literals unchanged at HEAD: `crates/cyrup-tui/src/commands.rs:51` `arg_cmd("model", …, "<model>")`, `:65` `cmd("login", "Configure provider authentication", None)` (no argument hint at all), `:70` `"Reload keybindings, extensions, skills, prompts, and themes"`. The `/reload` status string is still `"reloaded resources"` (`app/extension_ui.rs:323`).

**upstream** — pi v0.84.1 `packages/coding-agent/src/core/slash-commands.ts:21` `argumentHint: "<provider/model>"`, `:35` `argumentHint: "<provider>"`, `:40` `"Reload keybindings, extensions, skills, prompts, themes, and context files"`. pi's reload status is the longer wording with a trust variant (`interactive-mode.ts`, `Reloaded keybindings, extensions, skills, prompts, themes, and context files[; saved project trust]`).

**Impact** Cosmetic but misleading: `/model <model>` understates the required `provider/model` form, and `/reload` does not mention context files, which it does reload.

**Fix** Update the three literals in `commands.rs` and the status string in `app/extension_ui.rs:323` including the trust variant — which needs TUI-037's implicit-trust flag to be meaningful. Fold into the same S as TUI-N02, TUI-004 and TUI-037.

**Verify** Snapshot/unit assertions on the command table and the reload status, including the trust variant.

## TUI-035 — `tui.editor.historyPrevious` / `historyNext` are unbound

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/keymap.rs:121-155` `EditorAction` has no history variants and `:157-185` `from_id` no history ids; prompt recall is hard-wired to Up/Down at the buffer edges (documented at `crates/cyrup-tui/src/editor.rs:7`, with `:519`/`:539` noting "history is handled by the caller", exercised by `crates/cyrup-tui/src/tests/editor.rs:221` `prompt_history_recall_with_up_down`).

**upstream** — `pi/packages/tui/src/keybindings.ts:11-12` declares `tui.editor.historyPrevious` / `tui.editor.historyNext`, defaults `[]` at `:67-74`, handled at `pi/packages/tui/src/components/editor.ts:767-777` under the comment `// Dedicated history actions always browse entries instead of moving the cursor`, each cancelling autocomplete then calling `navigateHistory(∓1)`. **Added in the drift window** — absent from `git show v0.83.0:packages/tui/src/keybindings.ts`; the `v0.83.0..v0.84.1` diff to `editor.ts` is exactly this hunk.

**Impact** A user who wants shell-style history on ctrl+p/ctrl+n, or who wants Up/Down to move the caret and never recall, has no way to say so — the two ids pi added for exactly that are silently ignored.

**Fix** Add `EditorAction::{HistoryPrevious, HistoryNext}` with empty default bindings, map `tui.editor.historyPrevious` / `historyNext` in `EditorAction::from_id` (`keymap.rs:157-185`), and in `crates/cyrup-tui/src/editor.rs` route them to the existing history navigation **unconditionally** (not only at the buffer edges), cancelling any open autocomplete first, per `editor.ts:767-777`. Land with TUI-028, which renames the surrounding namespace.

**Verify** `crates/cyrup-tui/src/tests/keybindings.rs`: with `{"tui.editor.historyPrevious": "ctrl+p"}`, ctrl+p recalls the previous prompt from the **middle** of a multi-line buffer while Up still moves the caret.

## TUI-036 — `Show images` / `Image width` rows are offered on terminals with no image protocol

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/app/settings_rows.rs:32-42` pushes `SettingRow::toggle("terminal.showImages", …)` and `SettingRow::choice("terminal.imageWidthCells", …, ["60","80","120"])` unconditionally into `settings_rows`; the function takes no capability argument, and `state.image_renderer` / `state.capabilities` (set by `detect_image_support`, `app/shell.rs:375-401`) are not consulted anywhere in the grid builder.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/components/settings-selector.ts:654-671` — `// Only show image toggle if terminal supports it` / `if (supportsImages) { items.splice(1, 0, {id:"show-images", …}); items.splice(2, 0, {id:"image-width-cells", …}); }`, where `supportsImages` comes from `getCapabilities()`. The neighbouring `auto-resize-images` row is deliberately **not** gated (spliced at `supportsImages ? 3 : 1`), which is exactly the distinction cyrup loses. Present at v0.83.0.

**Impact** On a plain xterm, the Linux console or over a pipe, `/settings` offers two rows that cannot change anything, and the index positions of every row below them differ from pi's — so a user following pi's documentation, or a keyboard macro, lands on the wrong setting. It also makes the TUI-017 / TUI-N01 no-protocol behaviour look configurable when it is not.

**Fix** Give `settings_rows` (`app/execute_session.rs:258`) the capability flag already on `AppState` and push the two image rows only when `state.image_renderer.is_graphical()` (or `state.capabilities.images.is_some()`), keeping `images.autoResize` unconditional exactly as pi does. Land with TUI-N01, which plumbs the same capability into the transcript.

**Verify** App test with a no-protocol capability set: `/settings` contains neither `Show images` nor `Image width` but does contain `Auto-resize images`; with a Kitty capability set all three appear, in pi's order.

## TUI-038 — Ctrl+O is an if/else in cyrup and a fan-out upstream

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/app/input.rs:236-251`: `if self.state.transcript.has_bash() { toggle_bash_expanded() } else { toggle_tool_expanded() }` — mutually exclusive.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:4032-4051` `setToolsExpanded` sets `this.toolOutputExpanded`, then calls `setExpanded(expanded)` on the active header (`isExpandable(activeHeader)`, `:4042-4044`) **and** on every `isExpandable` child of `loadedResourcesContainer` and `chatContainer` (`:4045-4051`). The bash component is one of them — `components/bash-execution.ts:29` `private expanded = false` with `setExpanded(expanded)` at `:70`.

**Impact** Upstream one Ctrl+O expands the bash block and the tool blocks together. In cyrup, while any `!cmd` block is present the tool-expansion flag cannot be moved at all, and afterwards the two flags are out of sync with each other and with what the user last asked for. Distinct from TUI-010, which is about the missing status line.

**Fix** Restructure `Action::ToolsExpand` (`app/input.rs:236-251`) into a fan-out: set one `tools_expanded` flag and apply it to the bash block, the tool blocks and any expandable startup/loaded-resources entry, mirroring `setToolsExpanded`'s container walk. Land with TUI-010, which adds the status echo to the same arm.

**Verify** App test: with a live bash block present, Ctrl+O expands both the bash output and the tool output, and a second Ctrl+O collapses both; the flag observed by a subsequently committed tool entry matches the last Ctrl+O.

## TUI-039 — Terminal geometry never falls back to `$COLUMNS` / `$LINES`

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** (as filed) — `crates/cyrup-tui/src/app/draw.rs:20-30`: `let size = self.terminal.backend().size().ok(); let term_h = size.map(|s| s.height).unwrap_or(self.viewport_height).max(1); let term_w = size.map(|s| s.width).unwrap_or(80);` — the same `.unwrap_or(80)` pattern repeated at three further sites, and `rg 'COLUMNS|"LINES"' crates/ -g '*.rs'` returned only comments and test prose, no environment read anywhere. At HEAD every site routes through the sweep-1 fallback (`app/draw.rs:24-29`, `env_rows` / `fallback_columns`) and the one surviving `.unwrap_or(80)` is that helper's last resort (`app/render.rs:183`).

**upstream** — `pi/packages/tui/src/tui.ts:1730-1736`: `get columns() { return process.stdout.columns || Number(process.env.COLUMNS) || 80; }` and `get rows() { return process.stdout.rows || Number(process.env.LINES) || 24; }` — a two-step fallback, not a constant.

**Impact** Wherever the ioctl gives no size — a pipe, a CI harness, some container PTY setups — cyrup pins 80 columns and silently ignores a `COLUMNS=200` the user or harness set, where pi honours it.

**Fix** Add a `fallback_columns()` / `fallback_rows()` helper reading `$COLUMNS` / `$LINES` (parsed, positive) before the 80/24 constants, and use it at all four `unwrap_or` sites in `app/draw.rs` / `app/render.rs`.

**Verify** App test with a backend reporting no size and `COLUMNS=200` set: the computed width is 200; unset, it is 80; with `COLUMNS=garbage`, it is 80.

## TUI-040 — No `PI_TUI_WRITE_LOG` equivalent — no escape-sequence write log

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `rg 'TUI_WRITE_LOG' crates/ -g '*.rs'` → zero. There is no tee of the terminal write stream anywhere.

**upstream** — `pi/packages/tui/src/tui.ts:1375-1384` resolves `process.env.PI_TUI_WRITE_LOG` (accepting a directory, in which case it derives `tui-{timestamp}-{pid}.log`), and `:1723` `fs.appendFileSync(this.writeLogPath, data)` tees every terminal write.

**Impact** This matters more in cyrup than upstream. This workspace's own rule is that the TUI is not done until it has been run in a real terminal, and every one of the `-S` closures recorded this pass (`keyboard_protocol.rs`, `terminal_query.rs`, `drain.rs`, `terminal_progress.rs`, `terminal_title.rs`) is argued from byte constants and module docs alone. The write log is exactly the instrument that would let those be confirmed against a live kitty/iTerm2/tmux, and it is the one piece of pi's TUI debug surface nobody filed.

**Fix** Add a `CYRUP_TUI_WRITE_LOG` (accepting `PI_TUI_WRITE_LOG` as an alias, consistent with the env aliasing in `crates/cyrup-config/src/env.rs`) resolved once at `App::into_stdout`, and tee every write in the same place `draw_synchronized` and the raw-sequence writers already funnel through. Accept a directory and derive `tui-{timestamp}-{pid}.log` as pi does.

**Verify** With the variable pointing at a temp directory, one session produces a single log file whose bytes contain the OSC-0 title write, the keyboard-protocol negotiation and the drain-time disable sequences, in order.

## TUI-041 — `/settings` shows env-overridden rows with the wrong value

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** (as filed) — `crates/cyrup-tui/src/app/settings_rows.rs:63-75` built the `showHardwareCursor` and `terminal.clearOnShrink` rows from `eff.show_hardware_cursor(&cyrup_session_svc::EnvVars::default())` / `eff.clear_on_shrink(&cyrup_session_svc::EnvVars::default())`, with a comment stating the choice ("a default `EnvVars` yields the persisted setting (else `false`), which is what the grid edits") that is gone at HEAD — the two rows are `app/settings_rows.rs:72-75` and read a threaded `env`, with the sweep-1 rationale at `:63-71`. The **live** value takes the other branch: `crates/cyrup/src/main.rs:1642-1648` uses `EnvVars::from_process()` and feeds the result to `app.set_reserve_status_rows(reserve)`. The getters honour the environment — `crates/cyrup-config/src/settings.rs:849-861` `.unwrap_or(env.hardware_cursor)` / `.unwrap_or(env.clear_on_shrink)`, sourced from `CYRUP_HARDWARE_CURSOR`/`PI_HARDWARE_CURSOR` and `CYRUP_CLEAR_ON_SHRINK`/`PI_CLEAR_ON_SHRINK` at `crates/cyrup-config/src/env.rs:88-90`.

**upstream** — pi's settings selector renders each row from the same resolved value the runtime uses; there is no second, env-blind read path.

**Impact** With either environment variable set and nothing persisted, `/settings` reports `false` for behaviour that is on, and toggling the row to `true` looks like a no-op. Neither id is in the live-apply list at `app/execute_misc.rs:137-190` either, so `terminal.clearOnShrink` additionally does not take effect until the next launch.

**Fix** Thread the process `EnvVars` already built at `crates/cyrup/src/main.rs:1642-1648` into `settings_rows` and use it for both rows so the grid displays the resolved value; when the resolved value comes from the environment rather than the settings layer, mark the row the way pi marks non-editable state rather than silently accepting a write that will not win. Add both ids to the live-apply list at `app/execute_misc.rs:137-190`.

**Verify** App test with `CYRUP_CLEAR_ON_SHRINK=1` and nothing persisted: the `/settings` row reads `true`; toggling it writes the global layer and takes effect within the session without a relaunch.

## TUI-047 — A late or unsolicited DCS/APC frame is shredded into ~20 typed characters — `stray_reply.rs` recognises only OSC 11

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/stray_reply.rs:72-88` enumerates the machine's states, and `:88` fixes the tail to `['1','1',';']` — the filter matches exactly one frame shape, an OSC 11 background-colour reply, and `:53-61` states that as the deliberate scope. There is no DCS (`ESC P … ST`) or APC (`ESC _ … ST`) arm, and `rg 'x1bP|x1b_G|APC|DCS' crates/cyrup-tui/src/` returns only three comment lines in `image.rs`. crossterm has no DCS/APC handling either: `crossterm-0.29.0/src/event/sys/unix/parse.rs:76-88` routes `ESC` + anything other than `O`/`[`/`ESC` to the Alt-key fallback, so `ESC P` becomes `Alt+P`, the payload is decoded one printable character at a time, and the `ESC \` terminator becomes `Alt+\` — all of it delivered to the editor as text.

**upstream** — `pi/packages/tui/src/stdin-buffer.ts:55-63` classifies both families as first-class sequence types — "DCS sequences: ESC P … ESC \\ (includes XTVersion responses)" and "APC sequences: ESC _ … ESC \\ (includes Kitty graphics responses)" — with the completion tests at `:150-161` (`isCompleteDcsSequence`) and `:168-179` (`isCompleteApcSequence`). The consequence upstream is structural: a DCS/APC frame is emitted to `handleTerminalInput` as **one** `data` sequence, so the worst case is a single unrecognised keypress, not a line of garbage. Identical at v0.84.1.

**Impact** — blast radius, not likelihood. Neither pi nor cyrup issues an XTVersion query (`git grep` over `v0.83.0:packages/tui packages/coding-agent` finds no `ESC [ > q`), and cyrup's Kitty image writes go through ratatui-image 11.0.6, which sets `q=2` (`.../ratatui-image-11.0.6/src/protocol/kitty.rs:251`) and therefore suppresses both OK and error responses — so the common APC reply path is closed. What remains open is any **unsolicited** DCS/APC (a tmux passthrough echo, a terminal answering a query cyrup did not send, a `q=2`-ignoring terminal): pi degrades to one bogus keystroke, cyrup degrades to ~20 characters typed into the prompt plus two Alt-chords — the same failure mode `stray_reply.rs` was written for in the OSC 11 case. Filed at low because reachability is genuinely narrow, **not** because the mechanism is acceptable.

**Fix** — generalise `stray_reply.rs`'s state machine from one hard-coded tail to a small family: recognise the three ST-terminated introducers pi does — `ESC P` (DCS), `ESC _` (APC) and the existing `ESC ]` (OSC) — hold until an `ESC \` / BEL terminator or the `MAX_HELD` cap (`:70`), and swallow only on a complete terminator, preserving the replay-on-any-mismatch contract documented at `:34-51`. Keep the OSC 11 payload-alphabet check (`:273-275`) for the OSC arm; DCS/APC payloads are arbitrary, so bound them by `MAX_HELD` and the idle flush alone. **If TUI-045's pre-parser lands instead, port `isCompleteDcsSequence` / `isCompleteApcSequence` there verbatim and delete this arm** — do not build both.

**Verify** — unit in `stray_reply.rs`'s test module: feed the crossterm shredding of `\x1bP>|cyrup 1.0\x1b\\` and assert the output is empty; feed `\x1b_Gi=31;OK\x1b\\` and assert the same; then re-run the existing keystroke-safety battery (`:394-608`) unchanged, especially `every_event_of_a_non_matching_burst_is_accounted_for`, to prove the wider machine still cannot eat a real key. Follow with a **live terminal run** under tmux, since that is the reachable source.

## TUI-048 — Word navigation classifies by character class instead of Unicode word segmentation — CJK word motion jumps whole runs

> ## PARTIALLY FIXED 2026-08-13 — still open · `crates/cyrup-tui/src/editor.rs`
>
> The character-class run is gone. Word motion is now pi's three-branch shape over
> `unicode_segmentation`'s UAX#29 word-boundary iterator, with `PUNCTUATION_REGEX` (`utils.ts:821`)
> ported as its own `matches!` set — explicitly **not** the complement of the old `is_word_char`,
> which the item was right to warn about. `is_word_char` had no other caller and is deleted.
>
> **What is NOT closed, and why the item stays open.** `Intl.Segmenter` is ICU, and ICU adds a
> dictionary/LSTM pass for unspaced scripts that UAX#29 alone has no data for: `你好世界` is two
> segments to pi and four to `unicode-segmentation`. So Alt+B from the end lands at column 2 upstream
> and column 3 here — better than the pre-fix column 0, still not parity. Closing it means taking
> `icu_segmenter` plus its CJK/Thai data as a workspace dependency, which is a call for the workspace
> owner, not this change. Recorded as a CYRUP-DELTA on `InputEditor::word_segments`.
>
> **Tests**: `word_motion_keeps_pis_ascii_boundaries_after_the_segmenter_swap` (GREEN before AND
> after — it is the regression guard the item's Verify asks for: `foo.bar`, `don't`, `3.14`,
> `foo bar`, `a  b`, plus the forward direction) and `cjk_word_motion_no_longer_swallows_the_whole_run`
> (RED at HEAD), which asserts a RANGE rather than a column, precisely because pinning column 3 would
> pin behaviour pi does not have.

**Kind** parity-bug · **Severity** low · **Effort** M · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/editor.rs:1637-1639`: `fn is_word_char(c: char) -> bool { c.is_alphanumeric() || c == '_' }`, and `word_left_target` / `word_right_target` (`:1074-1128`) consume a maximal run of one class (word vs non-word-non-whitespace). There is no segmenter anywhere in the crate: `rg 'segment|Segmenter|unicode_segmentation' crates/cyrup-tui/src/editor.rs` finds only the wrap-related comment at `:1738`. Verified by hand that this **coincides with pi** for ASCII prose, identifiers, `foo'bar`, `3.14` and `foo.bar` — the divergence is confined to scripts where a run of `is_alphanumeric` characters spans several dictionary words.

**upstream** — `pi/packages/tui/src/word-navigation.ts:1-3` imports and instantiates `getWordSegmenter()` (`packages/tui/src/utils.ts:17-19`, an `Intl.Segmenter(undefined, { granularity: "word" })` created at `utils.ts:5`), and both functions iterate `Intl.SegmentData` with `isWordLike` (`:47`, `:100`). Inside a word-like segment they honour ASCII punctuation sub-boundaries via `PUNCTUATION_REGEX` (`utils.ts:821`) — backward takes the last match (`:50-56`), forward the first (`:102`). Whitespace is tested with `isWhitespaceChar` (`utils.ts:826-829`). Identical at v0.84.1.

**Impact** — in CJK text Alt+B / Alt+F / Ctrl+W treat an entire ideograph run as one word: `你好世界` is two segments to `Intl.Segmenter` (so pi's Alt+B from the end lands after `你好`) and one alphanumeric run to cyrup (so it lands at column 0, and Ctrl+W deletes all four characters instead of two). The same applies to Thai and other unspaced scripts. cyrup already depends on Unicode segmentation elsewhere in this file (`grapheme_boundaries`, the wrap code at `:1744`), so the inconsistency is internal as well as against pi.

**Fix** — route `word_left_target` / `word_right_target` (`editor.rs:1074-1128`) through a word segmenter — `unicode-segmentation`'s `UnicodeSegmentation::split_word_bound_indices` is the closest in-ecosystem match for UAX#29 and is very likely already in the graph via ratatui — keeping pi's three-branch shape from `word-navigation.ts:32-67` / `:89-114`: skip a whitespace run, then either one atomic segment (**TUI-043**), or one word-like segment truncated at its last/first `PUNCTUATION_REGEX` match, or a whole non-word non-whitespace run. Port `PUNCTUATION_REGEX` (`utils.ts:821`) as a `matches!` set rather than reusing `is_word_char`'s complement — the two are not the same set. `is_word_char` can stay for non-motion callers. **Sequence after TUI-043**, which touches the same two functions and is critical.

**Verify** — `crates/cyrup-tui/src/tests/editor.rs`: with text `你好世界` and the caret at 4, `E::CursorWordBackward` lands at 2, not 0; `E::DeleteWordBackward` leaves `你好`. Add ASCII regression cases (`foo.bar`, `don't`, `3.14`, `foo bar`) asserting the current, already-correct targets so the segmenter swap cannot silently change them.

## TUI-049 — `marker_at` accepts any text between `[paste #N ` and `]`, so cyrup expands paste markers pi's regex rejects

> ## FIXED 2026-08-13 — `crates/cyrup-tui/src/editor.rs`
>
> `marker_at` is split into a syntactic matcher (`marker_span_at`, a hand-rolled
> `PASTE_MARKER_SINGLE` — `editor.ts:24`) and the registry gate (`validIds`, `:44`). After the digits
> it accepts only an immediate `]`, or one space and exactly one of `+<digits> lines` /
> `<digits> chars`. The ungated matcher is also what the marker renumbering needs, since pi's
> renumber replaces on the bare `PASTE_MARKER_REGEX` with no id filter (`:1308-1314`).
> **Test** `a_hand_typed_marker_shaped_string_is_not_expanded` (RED at HEAD) — and it pins the other
> direction too: the bare `[paste #N]` form pi's regex *does* allow still expands.

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/editor.rs:663-694` `marker_at()` matches the literal prefix `[paste #`, then one or more digits, then scans to the closing `]` on the same marker (a stray `[` aborts, `:682-686`) — **the body between the id and the `]` is unconstrained**. So with paste id 1 live, the user-typed string `[paste #1 see the file above]` satisfies `marker_at`, and `expanded_text()` (`:639-659`) substitutes the full stored paste for it at submit time (`:1570-1571`).

**upstream** — `pi/packages/tui/src/components/editor.ts:21` @v0.83.0: `const PASTE_MARKER_REGEX = /\[paste #(\d+)( (\+\d+ lines|\d+ chars))?\]/g;` and `:24` the anchored single form. `expandPasteMarkers` (`:985-993`, v0.84.1 `:997`) builds its per-id regex from exactly that shape, so only `[paste #1]`, `[paste #1 +42 lines]` and `[paste #1 1500 chars]` are expandable. `isPasteMarker` (`:27-29`) additionally requires `segment.length >= 10`. Identical at v0.84.1.

**Impact** — text the user typed is silently replaced by unrelated content in the message the model receives, with no trace in the visible buffer. Contrived to hit deliberately, which is why it is low, but it is a silent-substitution path — and the looseness widens the surface of TUI-042 and TUI-043, because a partially-chewed marker that still happens to end in `]` keeps expanding. The reverse direction is safe: `marker_covering` is used for atomic delete, so a looser matcher there only over-protects.

**Fix** — tighten `marker_at` (`editor.rs:663-694`) to pi's grammar: after the digit run, accept either an immediate `]`, or a single space followed by `+<digits> lines]` or `<digits> chars]`, and nothing else. A dozen lines of straight-line parsing in the existing bounds-checked `chars.get` style, and it makes `marker_at` agree with the two formats `handle_paste` actually produces (`:619-624`). **Land with TUI-042 / TUI-043** — one invariant, three symptoms.

**Verify** — `crates/cyrup-tui/src/tests/editor.rs`: paste 1,500 chars (creating id 1), then type `[paste #1 see above]` on a second line; assert `expanded_text()` contains that literal string and expands only the real marker (today it expands both).

## TUI-050 — An 8-bit meta byte is silently dropped instead of being converted to `ESC` + char

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — nothing in `crates/cyrup-tui/src/app/input_reader.rs:312-381` inspects raw bytes — crossterm owns them. crossterm 0.29.0 falls through to `parse_utf8_char` for any byte outside its recognised control set (`src/event/sys/unix/parse.rs:115-122`); a lone `0xE1` is a UTF-8 lead byte with no continuation, so with `more == false` the call errors, and `Parser::advance` handles `Err(_)` by `self.buffer.clear()` (`src/event/source/unix/tty.rs:263-266`) — the byte is discarded with no event emitted.

**upstream** — `pi/packages/tui/src/stdin-buffer.ts:294-306` @v0.83.0, in `process()`: "Handle high-byte conversion (for compatibility with parseKeypress) — If buffer has single byte > 127, convert to ESC + (byte - 128)", i.e. `str = \`\x1b${String.fromCharCode(data[0] - 128)}\``, so `0xE1` becomes `ESC a` and reaches the key parser as `Alt+a`. Identical at v0.84.1.

**Impact** — on a terminal configured for 8-bit meta (xterm with `metaSendsEscape` off, some legacy emulators and serial consoles), every `Alt+<letter>` chord is swallowed entirely — no key event at all, so `Alt+B` / `Alt+F` / `Alt+D` / `Alt+Y` and any extension shortcut on Alt are dead. pi handles them. Low because `metaSendsEscape` has been the default for two decades, so almost no user is in this mode.

**Fix** — **dependent on TUI-045.** If that pre-parser lands, port `stdin-buffer.ts:294-306` into it verbatim (a one-byte read whose sole byte is > 0x7F becomes `ESC` + `byte - 128`). Without the pre-parser there is no seam — crossterm eats the byte before cyrup can see it — so this item must be scheduled **with** TUI-045 and never attempted alone.

**Verify** — unit on the pre-parser: feed a one-byte chunk `0xE1` and assert `Alt+a` is produced. **Live run** on `xterm -xrm 'XTerm*metaSendsEscape: false'`: `Alt+B` moves the caret back one word instead of doing nothing.

## TUI-N05 — Extension shortcuts can never override a built-in key; no conflict reported

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/app/input.rs:49-94`: the global `Keymap::action_for(key)` is consulted **first** and returns unless the action is `ClipboardPasteImage` with no image or a deferred `Interrupt`/`Quit`/`PageUp`/`PageDown`; only an unmatched key reaches `state.extension_shortcuts` at `:86-94`. The comment there frames this as protecting Ctrl+D/Esc, but it applies to *every* built-in binding. `rg 'RESERVED_KEYBINDINGS|restrict_override' crates/ -g '*.rs'` → **zero**.

**upstream** — `pi/packages/coding-agent/src/core/extensions/runner.ts:71-90` `RESERVED_KEYBINDINGS_FOR_EXTENSION_CONFLICTS` lists 18 ids; `getShortcuts` (`:510-533`) skips an extension shortcut **only** when the colliding built-in is reserved (`restrictOverride === true`), otherwise the extension **wins** and pi records `Extension shortcut conflict: '{key}' is built-in shortcut for {id} and {path}. Using {path}.` All land in `shortcutDiagnostics` (`:295`, exposed `:539-541`), surfaced under `[Extension issues]`.

**Impact** An extension binding a non-reserved key (any editor motion, page-up/down, history nav) silently never fires — the key does its built-in thing and the guest handler is dead, with no diagnostic to explain it. Two extensions on the same key is likewise silent last-wins.

**Fix** Port `RESERVED_KEYBINDINGS_FOR_EXTENSION_CONFLICTS` as a const set of cyrup `Action`s; in the key path check `state.extension_shortcuts` **before** the built-in keymap when the matched built-in action is not reserved, keeping the current precedence when it is. Have `register_shortcut` record a `ShortcutDiagnostic` on replacement, add a `shortcuts` field to `StartupDiagnostics` and fold it into `[Extension issues]` — the same plumbing TUI-006 needs. Work them together.

**Verify** App test: register an extension shortcut on a non-reserved key, press it, assert `AppAction::ExtensionShortcut` (not the built-in action); register one on `Esc`, press it, assert `Action::Interrupt` still wins **and** a conflict diagnostic is recorded.

## TUI-N06 — `Entry::Thinking` freezes hide/show at commit time

**Kind** parity-bug · **Severity** low · **Effort** L · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/transcript.rs:631-639` `commit_thinking` stamps `hidden: self.hide_thinking` into `Entry::Thinking`, and `:641-643`'s own doc says the setter "affects the live reasoning block and every entry committed afterwards; already-flushed scrollback is immutable". `app/session_bind.rs:79-83` concedes the divergence in a comment. The constraint is structural: committed entries are drained to `Terminal::insert_before` into the terminal's own scrollback.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:4050-4066` `toggleThinkingBlockVisibility` does `this.chatContainer.clear(); this.rebuildChatFromMessages();` then re-adds the streaming component — the toggle retroactively collapses or expands the whole visible conversation.

**Impact** Toggling "Hide thinking" mid-session changes only future turns. On the replay path the asymmetry is sharper: a `/resume` re-commits every historical turn's reasoning at the *current* setting, so the same conversation renders differently depending on when it was resumed.

**Fix** Decide, do not patch. (A) accept it and record it in `lib.rs`'s ADR-0001 notes plus the `/settings` row label; (B) keep the last N committed entries in a re-renderable tail above `insert_before`; (C) on a toggle, re-run the replay so the conversation re-commits below in the new form, accepting duplicated scrollback (which the sibling `/resume` divergence already produces). Mutating already-flushed rows is not achievable under `insert_before`. Per this ledger's no-accepted-divergence rule, option (A) is a decision to record, not a closure.

**Verify** Whatever is chosen, pin it: commit a `Thinking` entry with `hide_thinking = false`, flip `set_hide_thinking_block(true)`, assert the documented outcome so it cannot drift silently again.

## TUI-N07 — Mid-session `/resume` cannot erase the previous session's scrollback

**Kind** parity-bug · **Severity** low · **Effort** L · **Confidence** confirmed

**cyrup** — The swap arm (`crates/cyrup-tui/src/app/run_arms.rs:138-288`) calls `rebind_session()` then `replay_session_with_extensions(&restored, &ext_host)` (`:266`) and appends below the previous session's already-flushed `insert_before` scrollback; there is no clear and no boundary rule.

**upstream** — pi owns its whole viewport, so a clear is a real clear: `this.chatContainer.clear(); this.renderInitialMessages();` on the tree/fork navigate path, and `rebuildChatFromMessages()` (`interactive-mode.ts:4055-4056`) after compaction.

**Impact** After `/resume`, `/fork`, `/tree`, `/import` or `/clone` the terminal shows the old conversation, a `session replaced` status, then the new conversation in full. Scrolling up crosses a session boundary marked only by that one line.

**Fix** Same ADR-0001 family as TUI-N06; decide rather than patch. Cheapest honest improvement: replace the plain `session replaced` status with a full-width rule plus the new session's id/name/branch so the seam reads as deliberate. A true clear needs either the alternate screen (TUI-019) or a raw `ESC[3J` outside the ratatui buffer, which destroys the user's pre-cyrup scrollback and must be opt-in at most.

**Verify** App test: commit two entries, drive a session swap with a different message set, assert `scrollback_lines()` still contains the pre-swap text plus the chosen boundary marker — pinning the behaviour instead of leaving it in a commit message.

## TUI-N08 — `tests/image.rs` pins the invented `🖼` placeholder and the rasterize-anyway fallback

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/tests/image.rs:56` asserts the buffer does **not** contain `🖼` when inline rendering happens, and `:67-70` asserts `🖼`, the label and `64×48` are present when `show_images = false` — both pinning `crates/cyrup-tui/src/image.rs:242-247`'s invented `🖼 {label} ({w}×{h})` as correct. The same crate contains `image_fallback_text` (`image.rs:353-367`) producing pi's real form, so the test and the code disagree about what the placeholder should be. The inline test's own precondition makes a backend with **no image protocol** explicitly the tested case.

**upstream** — `pi/packages/tui/src/components/image.ts:114-118`: when `!caps.images`, exactly one line, `truncateToWidth(this.theme.fallbackColor(imageFallback(...)), width)`; `pi/packages/tui/src/terminal-image.ts` defines that string as `[Image: {shortened path} [{mime}] {w}x{h}]`. pi has no half-block rasterizer anywhere.

**Impact** A green test pinning current-but-wrong behaviour. Anyone implementing TUI-017 hits two failing assertions and must decide whether the test or this document is authoritative, while the suite reports parity on a path that has none.

**Fix** Retarget both to state the divergence rather than the format: assert that *some* placeholder line is emitted, or annotate the two assertions `// TUI-017: pins the current cyrup format; flip to image_fallback_text when the attachment strip is ported` plus an `#[ignore]`d companion asserting pi's `[Image: …]` form. Delete the comment and un-ignore when TUI-017 lands.

**Verify** `grep -n '🖼' crates/cyrup-tui/tests/` returns nothing load-bearing, and `attached_image_fallback_matches_pi_format` passes once TUI-017 is fixed and fails loudly if the format regresses.

## TUI-N09 — `extension_dialog_countdown` asserts an exact countdown it cannot control

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/tests/extension_dialog_countdown.rs:85` `std::thread::sleep(Duration::from_millis(1_100))`, `:86` `app.tick_extension_dialog_countdown()` (no argument, so no injectable instant), `:88` `assert!(text.contains("Proceed? (2s)"))` — a wall-clock-exact assertion against a 3 s budget with ~900 ms of scheduler slack, in a workspace of ~3,180 tests. The deterministic alternative already exists in-repo: `StatusIndicator::retry_message` reads `started.elapsed()` (`crates/cyrup-tui/src/status_indicator.rs:161-164`), landed by TUI-023's fix this pass.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/components/countdown-timer.ts:21-30` drives the same countdown from an injected timer rather than a wall-clock sleep.

**Impact** A CI or loaded-laptop stall past 900 ms turns it red with a message pointing at the countdown logic rather than at the scheduler, which is how a suite starts being ignored.

**Fix** (a) weaken to a monotone assertion — after the sleep the title shows fewer seconds than at open and more than zero; or better (b) give the countdown an injectable instant (`tick_extension_dialog_countdown_at(Instant)`), mirroring the `started.elapsed()` pattern `status_indicator.rs` now uses, and drive it synthetically. (b) also removes 1.1 s of real sleep from the suite.

**Verify** With (b): `tick_extension_dialog_countdown_at(open + 1_100ms)` renders `(2s)` and `+ 2_100ms` renders `(1s)`, with no `thread::sleep` and no dependence on wall time.

## TUI-N10 — `bash_overlay`'s two hotkeys tests hard-code the non-macOS `alt` spelling, so they are red on every darwin host

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** confirmed · **Status** fixed this pass

**cyrup** — `crates/cyrup-tui/src/tests/bash_overlay.rs:309` asserted ``| `Ctrl+X/Alt+E` | Edit message in external editor |`` and `:368` asserted ``| `Alt+Shift+K` | Kill the ring |``. On macOS the renderer emits `Ctrl+X/Option+E` and `Option+Shift+K`, because `crates/cyrup-tui/src/chrome.rs:41-47` `format_key_part` rewrites `alt`→`option` under `cfg!(target_os = "macos")` — reached from `app/hotkeys.rs:9`/`:12` (the `keyDisplayText` closures) and `app/hotkeys.rs:106-115` (the Extensions rows). Measured before the fix: `cargo test -p cyrup-tui --test bash_overlay` → 10 passed / 2 failed in 0.01 s. Every other assertion in both tests was already satisfied by the actual output — the rebind half (`Ctrl+T`, no surviving `Ctrl+O`, the `/`-joined pair, the untouched `Ctrl+D` mirror) and the whole Extensions section including ordering and the `AppAction::ExtensionShortcut` dispatch.

**upstream** — pi v0.83.0 `packages/coding-agent/src/modes/interactive/components/keybinding-hints.ts:12-15`: `const displayPart = process.platform === "darwin" && part.toLowerCase() === "alt" ? "option" : part;`, on the path `formatKeyText` (`:17-27`) → `formatKeys` (`:29-32`) → `keyDisplayText` (`:38-40`), used by `interactive-mode.ts:5743-5752` and directly at `:5856` for the Extensions table. pi on macOS prints exactly what cyrup prints; the tests pinned pi's Linux/Windows spelling.

**Impact** Two permanently-red tests on the platform this workspace develops on, pinning a spelling upstream does not produce there, while the behaviour they exist to guard — global cells resolving from the live keymap, and the Extensions table — is in fact correct. A red that is not a defect is how a suite starts being ignored; the direct sibling of TUI-N09.

**Not** a symptom of the ADR-0011 / CFG-048 keybindings-name migration. Both tests use the modern ids (`app.tools.expand`, `app.editor.external`) and both resolved correctly; `KEYBINDING_NAME_MIGRATIONS` plays no part in `/hotkeys` rendering, and TUI-028's `editor.*` namespace question is untouched. Batch 8 inherits nothing from this item.

**Fix** — *applied.* Bound `let alt = if cfg!(target_os = "macos") { "Option" } else { "Alt" };` in each test and interpolated it into the expected cell, mirroring the in-repo precedent `crates/cyrup-tui/src/tests/chrome.rs:36-43` `format_key_text_rewrites_alt_to_option_on_macos`. Deliberately **not** sourced from `cyrup_tui::format_key_text`, which would let a broken formatter satisfy its own assertion.

**Verify** — done. `cargo test -p cyrup-tui --test bash_overlay` → 12 passed / 0 failed on macOS. Mutation-checked: disabling `chrome.rs:41-47`'s darwin arm returns the target to 10 passed / 2 failed, so the assertions are not vacuous.

## TUI-N11 — `m7_inline_formatting_survives_inside_a_table_cell` asserted a property of the developer's `TERM_PROGRAM`

**Kind** test-defect · **Severity** ~~medium~~ · **Effort** S · **Confidence** confirmed · **Status** **CLOSED 2026-09-04**

> **CLOSED 2026-09-04 — the fix was real; only the bookkeeping was open.** The `## Open items` row said
> "fixed this pass" while leaving `medium` unstruck, and `scripts/count_open_items.py` decides closure on the
> severity strike alone, so the census carried a repaired test as an open medium for three weeks. The row is now
> struck.
>
> **Re-measured at HEAD rather than re-read.** Both link arms render through the explicit-capability entry point —
> `crates/cyrup-tui/src/tests/markdown.rs:2045` `render_markdown_with_hyperlinks(linked, 40, &theme, false)` and
> `:2071` the capable mirror — so the test never reaches `crate::image::hyperlinks_supported()`, which is what
> `render_markdown` resolves the gate from (`crates/cyrup-tui/src/markdown/mod.rs:148`). `cargo nextest run -p
> cyrup-tui -E 'test(tests::markdown::)'` is 51/51 under `TERM_PROGRAM=ghostty` (+`GHOSTTY_RESOURCES_DIR`),
> `=vscode`, `=iTerm.app` and with the terminal-identity vars scrubbed; `-p cyrup-tui` as a whole is 1375/1375.
> **Counterfactual, so the pin is not taken on faith:** reverting that one arm to `render_markdown(linked, 40,
> &theme)` gives `1 test run: 0 passed, 1 failed` under the ghostty identity and stays green scrubbed — the split
> the item described. Revert restored; the only committed change is `f061bf35`, comments.
>
> **Upstream re-read at the ADR-0006 target (pi v0.84.4), and the citations refreshed to it.** The gate and its two
> arms are byte-identical to the v0.83.0 text this item was written against; only the offsets moved, because the
> `link` case starts at `markdown.ts:689` @v0.84.4 against `:537` @v0.83.0 — fallback ` (url)` suffix `:696-707`
> (was `:544-554`), OSC-8 branch `:692-695` (was `:540-543`). Upstream's own pin, quoted in the test, is unchanged
> too: `packages/tui/test/markdown.test.ts:597-598` @v0.84.4 (`:469-470` @v0.83.0), "Pin to no-hyperlinks so width
> checks work on plain text without OSC 8 sequences." `f061bf35` also fixed a drifted in-file cross-reference (the
> convention note is at `tests/markdown.rs:181-183`, not `:132-134`).
>
> **The structural half is not this row's.** `set_capabilities` / `reset_capabilities_cache` — the seam TUI-N12
> asked for — now exist over the whole `TerminalCapabilities` record (`crates/cyrup-tui/src/image.rs:661-707`), and
> TUI-N12 is closed separately. Nothing of TUI-N11 remains open.

**cyrup** — `crates/cyrup-tui/src/tests/markdown.rs:1496` and `:1505` rendered the link mirror arm through `render_markdown`, which resolves the OSC-8 gate from `crate::image::hyperlinks_supported()` (`crates/cyrup-tui/src/image.rs:450-452`) — a write-once `OnceLock` sniff of the ambient environment. `:1501` then demanded the row contain `doc (https://ex.com)`, a string that by design cannot exist when the terminal is hyperlink-capable (`src/markdown.rs:1105` gates the suffix on `!self.hyperlinks`). Measured: with `TERM_PROGRAM=ghostty` + `GHOSTTY_RESOURCES_DIR` set (`image.rs:516` → `hyperlinks: true`) the target was 47 passed / 1 failed; with those five vars unset, 48 passed / 0 failed; forcing `TERM_PROGRAM=vscode` reproduced the identical panic. The red therefore fired on ghostty, kitty, iTerm2, WezTerm, Warp, vscode, alacritty, Windows Terminal and forwarding tmux, and hid only on an unidentified terminal — which is why it had been recorded as an unexplained failure.

**upstream** — pi never has this exposure: `packages/tui/test/markdown.test.ts` @v0.83.0 imports `{ resetCapabilitiesCache, setCapabilities }` at `:6` and pins the branch at eight sites (`:470, :1263, :1276, :1289, :1301, :1313, :1334, :1348`). `:469` states the reason inside its own table block, verbatim: "Pin to no-hyperlinks so width checks work on plain text without OSC 8 sequences" — the same family of width/border-sensitive table assertion as the cyrup test that failed. The renderer itself is a faithful port: `packages/tui/src/components/markdown.ts:537-557`, where the ` (url)` suffix exists only in the `else` of `if (getCapabilities().hyperlinks)`.

**Impact** A red on nearly every real terminal, blamed on the renderer rather than the harness. Neither TUI-FIDELITY M7 nor M14 had regressed — both fixes are intact and their dedicated assertions were green throughout. Belongs to the test-hermeticity class opened by 5d2bc0b / ce0bf8c ("stop asserting a property of the ambient shell"). cyrup's own file states the convention at `tests/markdown.rs:132-134` and honours it at twelve other call sites; this one arm was the sole exception.

**Fix** — *applied.* Routed the mirror arm through `render_markdown_with_hyperlinks(linked, 40, &theme, false)` — the explicit-capability entry point the crate exports for exactly this purpose (`src/markdown.rs:142`, "Exists so tests can drive both branches without touching the global cache") — hoisting the previously-duplicated render into one binding reused by both halves. **Strengthened rather than merely pinned**: appended a capable-branch mirror asserting that on the OSC-8 path the cell holds `doc`, the top border is unpolluted, and the URL is *not* printed inline (`markdown.ts:540-543`). The production gate at `src/markdown.rs:1105` was left untouched.

**Verify** — done. Green under all four terminal identities, where previously only the last passed: ambient `TERM_PROGRAM=ghostty` 48/48; `TERM_PROGRAM=vscode` 48/48; `TERM_PROGRAM=iTerm.app` 48/48; fully scrubbed 48/48. Mutation-checked: dropping the `!self.hyperlinks` gate fails 3 tests including `m7` — and now fails it under the *scrubbed* environment too, which the pre-fix arm could not detect.

## TUI-N12 — No counterpart to pi's `setCapabilities` / `resetCapabilitiesCache`: only the markdown renderer has a both-branches test seam

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — the only counterpart is `seed_hyperlink_support` (`crates/cyrup-tui/src/image.rs:457-459`), a first-writer-wins `OnceLock::set` (`:430`) that (a) cannot be overwritten or reset once read and (b) carries only the `hyperlinks` field, not `images` / `true_color`. Because there is no way to pin the global, a test wanting the non-ambient branch must use a per-call override, and that override exists for exactly one consumer — `render_with_hyperlink_support` (`src/markdown.rs:142`). Any other present or future consumer of `hyperlinks_supported()` has no seam at all and can only be tested against whatever terminal the developer happens to be running, which is precisely the failure mode TUI-N11 records.

**upstream** — pi @v0.83.0 exports two capability-cache mutators alongside the getter: `resetCapabilitiesCache()` (`packages/tui/src/terminal-image.ts:137-139`) and `setCapabilities(caps)` (`:142-144`), the latter doc-commented "Override the cached capabilities. Useful in tests to exercise both code paths". Both are pure state mutation and draw nothing, so ADR-0001 rule 2 puts them in scope with no substrate defence.

**Impact** The hermeticity hole that produced TUI-N11 is structural, not incidental: it is closed for markdown only, by a per-call parameter. Secondary and latent, flagged rather than claimed: `App::detect_image_support` (`app/shell.rs:375`) seeds the lock at boot, and if any earlier caller latches it first that seed is silently discarded for the process — including a real tmux session whose `client_termfeatures` probe positively confirmed forwarding, which would then print pi-suppressed ` (url)` noise on every link for the rest of the session. Both call sites (`crates/cyrup/src/main.rs:1840`, `:1957`) currently run before the event loop, so this is an ordering hazard rather than an observed bug.

**Fix** Port both mutators over a capability struct carrying all three fields, replacing the `OnceLock` with a resettable cache; keep `render_with_hyperlink_support` as the per-call override.

**Verify** A test that calls `set_capabilities(hyperlinks: true)`, renders through the *ambient* entry point, asserts the OSC-8 shape, then `reset_capabilities_cache()` and re-asserts the fallback — with no dependence on `TERM_PROGRAM`.

## TUI-N13 — `a_live_bash_run_names_its_spool_file` read the spool path from a single rendered line, so it was red on every host whose `TMPDIR` is long (i.e. every macOS host)

**Kind** test-defect · **Severity** high · **Effort** S · **Confidence** confirmed · **Status** fixed this pass

**cyrup** — `crates/cyrup-tui/src/tests/bash_live_run.rs:67-107` `a_live_bash_run_names_its_spool_file` (pre-fix) did `out.lines().find(|l| l.contains("Output truncated. Full output:"))` and then `row.split("Full output:").nth(1).unwrap().trim()`. The status block is rendered word-wrapped at the `TestBackend::new(120, 24)` width set at `:70`, and the spool path comes from `std::env::temp_dir()` (`crates/cyrup-session-svc/src/bash.rs:258`, `cyrup-bash-{suffix}.log`). On macOS `temp_dir()` is `/var/folders/<2>/<30>/T/` (48 chars), so the rendered row ` Output truncated. Full output: /var/folders/d9/6l395nfj7cz0mgc37c_1dxjr0000gn/T/cyrup-bash-180e4-18cb58e67518da38-0.log` is 120 columns and the path wraps onto the **next** visual line. The `find` therefore matched a line ending at `Full output:`, `nth(1)` yielded the empty string, and the assertion at `:93` failed with an empty path. Reproduced 5/5 in isolation and once in the full-workspace run — deterministic on this host, not flaky. On Linux with `TMPDIR` unset the same path is `/tmp/cyrup-bash-….log` (~43 chars) and the row fits, which is why the test was ever green.

**upstream** — the wrap is faithful, not a defect. pi @v0.83.0 `packages/coding-agent/src/modes/interactive/components/bash-execution.ts:195-199` pushes `Output truncated. Full output: ${this.fullOutputPath}` into `statusParts`, and `:201` emits the whole block as `this.contentContainer.addChild(new Text(`\n${statusParts.join("\n")}`, 1, 0))` — a `Text` node with padding-left 1 that word-wraps to the terminal width exactly as cyrup's does. pi's own bash tests never parse a rendered line for the path. So the renderer matches upstream and only the fixture was wrong.

**Impact** A permanently-red unit test in `cyrup-tui --lib` on the platform this workspace develops on, invisible to every prior pass because the first suite measurement piped `cargo test` through `tail` and kept only the last 120 lines. It sat inside the same 285-test target as the rest of `app.rs` (pre-split; the tree is now `app/`), so `-p cyrup-tui --lib` could never be green here — the direct sibling of TUI-N10 (macOS-only red) and TUI-N11 (ambient-environment assertion), and the third instance of the same class in one pass.

**Fix** — *applied.* Flatten the wrap before parsing: `let flat = out.split_whitespace().collect::<Vec<_>>().join(" ");` then `flat.split_once("Output truncated. Full output: ")` and take the first whitespace-delimited token. Width- and `TMPDIR`-independent, and the assertion is unchanged — the RENDERED scrollback must still name the executor's own `cyrup-bash-` spool file, and the test still opens that file and requires `line-number-1-padding` and `line-number-3000-padding` to both be present. Neither the renderer (`crates/cyrup-tui/src/bash.rs:435`) nor `transcript.rs` was touched.

**Verify** — done. `-p cyrup-tui --lib` 285 passed / 0 failed (was 284/1); the x13 family 3/3. Mutation-checked so the fix is not vacuous: rewriting `bash.rs:435` to emit `MUTANT-` in place of `cyrup-bash-` turns the test red again with the full mutated path in the message (`…/T/MUTANT-18234-….log`), proving the new parse both reads the path across the wrap and still discriminates on it. Mutation reverted; `git status` clean for `bash.rs` and `transcript.rs`.

## TUI-S02 — No dead-terminal (EIO/EPIPE/ENOTCONN) emergency exit path

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

> **Auditor closure overturned — half only.** The `uncaughtCrash` half landed: `crates/cyrup-tui/src/panic_hook.rs:82-89` `install_panic_hook` chains `restore_terminal_best_effort()` before the previous hook, with a test double at `:132`, installed at `crates/cyrup-tui/src/app/crossterm.rs:48` **before** `enable_raw_mode` — and `app/crossterm.rs:44-48` documents that ordering and cites pi's install site. The item's *second* named mechanism did not land.

**cyrup** — `grep -n 'EIO\|emergency\|BrokenPipe\|unhandled' crates/cyrup-tui/src/panic_hook.rs crates/cyrup-tui/src/app/` → nothing relevant. A write failure to a dead terminal runs the full normal restore path straight back into the dead fd and exits with an ordinary code.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:212-220` `DEAD_TERMINAL_ERROR_CODES = new Set(["EIO","EPIPE","ENOTCONN"])` plus `isDeadTerminalError`, feeding `emergencyTerminalExit()` at `:3816-3823`, which deliberately **skips** normal shutdown ("TUI and extension cleanup can write restore sequences and re-trigger EIO") and exits 129.

**Impact** A dropped SSH connection or a killed tmux pane sends cyrup through the restore path it must not take: each restore write hits the dead fd, and the exit code does not signal a terminal death to the parent. Low because it needs the terminal to die mid-session, but the failure mode is exactly the one pi added a dedicated path for.

**Fix** Classify write errors from the draw/flush and restore paths against pi's three codes in `crates/cyrup-tui/src/drain.rs` or `panic_hook.rs`, and on a match take an emergency path that skips `drain_and_restore()` and extension cleanup entirely and exits 129 — mirroring `emergencyTerminalExit`. Coordinate the exit code with `SEAM-008` in area 08, which owns cyrup's signal exit codes.

**Verify** Unit test over a writer that returns `ErrorKind::BrokenPipe`: the emergency path is taken, no restore sequences are written, and the process exit code is 129; a `PermissionDenied` write error still takes the normal path.

## TUI-S10 — Shift+Ctrl+D global debug chord absent

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `rg 'debug' crates/cyrup-tui/src/keymap.rs` → **zero** at HEAD; `rg 'onDebug|on_debug' crates/ -g '*.rs'` → **zero**. `/debug` is reachable only by typing it (`crates/cyrup-tui/src/commands.rs:76` `HIDDEN_COMMANDS`, handler at `crates/cyrup-tui/src/app/session_bind.rs:217`).

**upstream** — `pi/packages/tui/src/tui.ts:850`, inside `handleTerminalInput` and **before** dispatch to the focused component: `if (matchesKey(data, "shift+ctrl+d") && this.onDebug) { this.onDebug(); return; }`. Wired at `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:2803` `this.ui.onDebug = () => this.handleDebugCommand();` — "works regardless of focus".

**Impact** When a selector, dialog or overlay has focus — precisely when a diagnostic dump is wanted — there is no route to it, because the only route is the editor that no longer has focus.

**Fix** Add a pre-dispatch chord check in the key path (`app/session_bind.rs:50`), ahead of the focused-component dispatch, matching shift+ctrl+d to the existing `/debug` handler at `app/session_bind.rs:217`. Structurally outside TUI-008/TUI-028, which cover configurable ids; pi hardcodes this one.

**Verify** App test: with a selector focused, shift+ctrl+d emits the debug dump and the selector keeps its selection; with the editor focused, the same.

## TUI-052 — A queued message dequeued by Escape stays in the transcript forever as a phantom user message that was never sent

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** **confirmed — observed in a live terminal, cross-checked against the session JSONL** · **observed 2026-08-13** (live-terminal; [`REPRO-LOG.md`](REPRO-LOG.md))

> **Filed 2026-08-13 from the `TUI-016` repro run.** Not covered by `TUI-016` (which is about the
> *missing queue surface*) nor by `TUI-005` (which is about the Escape *restore*), and it **survives
> both of their fixes** unless the echo site itself is removed.

**cyrup** (as filed) — `crates/cyrup-tui/src/app/submit.rs:13` — `dispatch_submission`'s `Dispatch::Prompt` arm called `self.state.transcript.push_user(prompt.clone())` **unconditionally**, before returning `AppAction::Submit(prompt)`, with no knowledge of whether the session will run the prompt or queue it. The arm is `app/submit.rs:31` at HEAD and pushes nothing; the removal is recorded in place at `:22-30`. There is no compensating retraction anywhere: `rg 'remove_user\|retract\|pop_user' crates/cyrup-tui/src/transcript*` finds nothing, and the `QueueUpdate` handler (`app/events_fold.rs:189-194`) touches only `status.set_queued`. So when `TUI-005`'s Escape path restores a still-queued message to the editor, the transcript keeps the bubble that was pushed when it was typed.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:2826-2833` @v0.83.0 — the streaming branch clears the editor and calls `updatePendingMessagesDisplay()`; it **never** writes the text into the chat container. The text lives only in `pendingMessagesContainer`, which `restoreQueuedMessagesToEditor` (`:3993-4005`) clears via `updatePendingMessagesDisplay()` at the moment the text moves back to the editor. pi cannot reach this state.

**Impact** — Measured live: two messages queued during a streaming turn were both echoed into the transcript. Escape then interrupted the second turn and correctly restored the still-queued `QUEUEDMSGTWO` into the **editor** — while its transcript echo was never retracted, so the same text was on screen twice, once as a delivered user turn and once as unsent editor content:

```
47: count slowly
51: QUEUEDMSGONE
55: QUEUEDMSGTWO        <-- still shown in the transcript as a DELIVERED user message
72: Operation aborted
74: QUEUEDMSGTWO        <-- and simultaneously sitting UNSENT in the editor
```

The session JSONL proves it was never sent — the only user messages recorded are `count slowly` and `QUEUEDMSGONE`. **The transcript permanently disagrees with the session**: it shows a turn the model never saw. A user scrolling back sees a question they believe was asked and answered.

**Fix** — Do not push a submission into the transcript until the turn it belongs to actually starts. Move the `push_user` out of `dispatch_submission` (`app/session_bind.rs:140`) and onto the event that begins the run, so a queued message is rendered by the pending-messages region (**TUI-016**) and by nothing else. **Land with TUI-016** — that item's Fix adds the pending rows, and adding them without removing this echo renders every queued message twice.

**Verify** — App test: submit during a streaming turn, assert the transcript gains **no** user entry while the message is queued, then interrupt and assert the transcript still has no entry for it and the editor holds it. Then a live terminal run against a streaming turn: queue a message, press Escape, and confirm the text appears **once**, in the editor only.

## TUI-053 — `Ctrl+-` (`editor.undo`) is unreachable from any terminal that does not implement the kitty keyboard protocol

> ## FIXED 2026-08-13 — `crates/cyrup-tui/src/keymap.rs`
>
> Ported `keys.ts:1275-1281` as `normalize_legacy_control_byte`, applied at the head of
> `EditorKeymap::action_for`: `Char('4'|'5'|'7') + CONTROL` — which is how crossterm 0.29.0 renders
> the bytes `0x1C`/`0x1D`/`0x1F` (`event/sys/unix/parse.rs:110-113`) — is rewritten to
> `Char('\\'|']'|'-') + CONTROL`, preserving ALT so pi's `\x1b\x1f` → `ctrl+alt+-` forms map too.
> `0x1E` is left alone because pi maps no chord to it. The rewrite is **gated on
> `keyboard_protocol::current() != Kitty`**: under the kitty protocol `Ctrl+7` is genuinely `Ctrl+7`
> (`CSI 55;5u`) and pi's byte branch is unreachable, so the alias must be too. Chosen over adding a
> second binding to the table so `/hotkeys` still reports `ctrl+-`, and over waiting for TUI-045's
> pre-parser, which does not exist yet — if that lands, this function is what moves into it.
>
> **Test** `ctrl_undo_is_reachable_from_a_terminal_without_the_kitty_protocol` (RED at HEAD) covers
> undo *and* the `0x1D` → `ctrl+]` char-jump of the same class. **Still wants the live half**: a run
> on a real non-kitty terminal (Terminal.app or plain xterm), which this pass could not perform.

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** **confirmed — measured live, and pi's legacy mapping read at the tag** · **observed 2026-08-13** (live-terminal; [`REPRO-LOG.md`](REPRO-LOG.md))

**cyrup** — `crates/cyrup-tui/src/keymap.rs:1010` binds `(ctrl('-'), E::Undo)` and nothing else; `:176` maps the id `"editor.undo"` to `E::Undo`. Matching `Char('-') + CONTROL` requires a CSI-u report. On a legacy terminal `Ctrl+-` arrives as the single byte `0x1F`, and crossterm 0.29.0 decodes the whole `0x1C..=0x1F` range arithmetically — `src/event/sys/unix/parse.rs:110-113`, `c @ b'\x1C'..=b'\x1F' => KeyCode::Char((c - 0x1C + b'4') as char) + CONTROL` — so `0x1F` becomes **`Char('7') + CONTROL`** and can never match `Char('-') + CONTROL`. There is no fallback binding and no diagnostic. Note the keymap already carries an explicit `(Kitty-gated)` comment for the char-jump keys directly below this line: the gating hazard was understood there and not applied to undo.

**upstream** — `pi/packages/tui/src/keys.ts:1277` @v0.83.0, read this pass: **`if (data === "\x1f") return "ctrl+-";`**, with `:1281` `if (data === "\x1b\x1f") return "ctrl+alt+-";`. pi's default binding is the same single `ctrl+-` (`packages/tui/src/keybindings.ts:117`), but pi decodes the **legacy control byte explicitly**, so undo works on every terminal. This is a genuine parity gap, not merely a robustness one: the mechanism pi ports specifically to make `ctrl+-` reachable was not ported.

**Impact** — On Terminal.app, iTerm2's default profile, gnome-terminal, plain xterm and anything else without the kitty protocol, **undo simply does not exist** — no error, no hint, nothing happens. Measured live under tmux with extended-keys on: `send-keys C--` did nothing to the editor, while the raw kitty CSI-u form `ESC[45;5u` fired the undo:

```
$ tmux send-keys "abc"                         -> editor: abc
$ tmux send-keys "C--"                         -> editor: abc      (nothing happened)
$ tmux send-keys -H 1b 5b 34 35 3b 35 75       -> editor: (empty)  (undo fired)
```

This also raises the cost of **TUI-042** and **TUI-044**, both of which are *about* undo restoring state correctly: on most terminals their user-visible surface is unreachable, so a fix to either is untestable by hand without a kitty-protocol terminal.

**Fix** — Add the legacy alias. In `keymap.rs` bind `E::Undo` to `Char('7') + CONTROL` alongside `ctrl('-')` (the byte crossterm actually delivers), or — cleaner, and the right home if **TUI-045**'s pre-parser lands — normalise `0x1F` to `ctrl+-` at the input boundary, which is the literal shape of pi's `keys.ts:1277`. Prefer the pre-parser form so the mapping lives in one place with `\x1b\x1f` → `ctrl+alt+-`. Audit the rest of the `0x1C..=0x1F` range for the same class while there.

**Verify** — Unit: feed the input pipeline the single byte `0x1F` and assert `E::Undo` is produced. Then a live terminal run **on a non-kitty terminal** (Terminal.app or plain xterm): type text, press `Ctrl+-`, confirm the edit is undone. A tmux run with `extended-keys on` does not close this — it is the configuration in which the bug is invisible.

## TUI-054 — A failed or aborted compaction is announced to the user as "compaction complete"

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** **confirmed — observed three times in live scrollback, and the discarded fields read at HEAD** · **observed 2026-08-13** (live-terminal; [`REPRO-LOG.md`](REPRO-LOG.md))

**cyrup** (as filed) — the `CompactionEnd` arm destructured the event as `AgentSessionEvent::CompactionEnd { .. }` — **discarding every field** — and ended with an unconditional `self.state.transcript.push_status("compaction complete");`. The arm is `crates/cyrup-tui/src/app/events_fold.rs:224` at HEAD, where the sweep-1 fix destructures `{ reason, result, aborted, error_message, .. }` and branches; its own `TUI-054` comment at `:237-242` records the old shape. The fields thrown away are exactly the ones that decide what to say: `crates/cyrup-session-svc/src/event.rs:173-181` declares `CompactionEnd { reason, result, aborted, will_retry, error_message }`, and the crate's own doc comment at `:170-172` describes `error_message` as carrying "an `error_message` on the failure paths".

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:3089-3100` @v0.83.0, read this pass: `case "compaction_end"` restores the escape handler, clears the indicator, and then branches — `if (event.aborted) { if (event.reason === "manual") { this.showError("Compaction cancelled"); …` — i.e. pi reports cancellation and failure distinctly and never claims success for either.

**Impact** — Observed three times in live scrollback, each an error immediately followed by a success claim:

```
1069: compact error: Nothing to compact (session too small)
1073: compaction complete

 346: compact error: compaction: summarization failed: http 400: {
 348:   "error": {
 351:     "type": "invalid_request_error",
 359: compaction complete
```

In the second case the summarization provider call failed outright and **no compaction entry was written to the session file** — yet the last line the user sees is "compaction complete". Anyone who scrolls past the error blob, or whose error blob has scrolled off, is told the context was compacted when it was not, and will then reason about their remaining context window from a false premise. Compounded by **TUI-055**: with no indicator during the compaction, this status line is the *only* feedback the whole operation produces.

**Fix** — Destructure the event: `CompactionEnd { reason, aborted, error_message, .. }`, and branch as pi does — `error_message` ⇒ render the failure, `aborted && reason == Manual` ⇒ "Compaction cancelled", else "compaction complete". Lands naturally with **SESS-040** (which makes `aborted` reachable at all) and **TUI-055**.

**Verify** — App test: drive the event loop with `CompactionEnd { aborted: true, reason: Manual, .. }` and assert the transcript says "Compaction cancelled" and **not** "compaction complete"; repeat with `error_message: Some(…)` and assert the message is rendered. A live run where compaction is refused ("Nothing to compact") must not print a success line.

## TUI-055 — No status indicator renders for the entire duration of a compaction — the screen is blank for 10–20 s

**Kind** parity-bug · **Severity** high · **Effort** M · **Confidence** **confirmed — sampled every 200 ms across a full compaction in a live terminal** · **observed 2026-08-13** (live-terminal; [`REPRO-LOG.md`](REPRO-LOG.md))

> **Filed 2026-08-13 from the `SESS-040` repro run**, which it corrects: `SESS-040` assumes the
> opposite ("the indicator keeps spinning"). No item describes the non-render.

**cyrup** — `crates/cyrup-tui/src/app/events_fold.rs:195-222` handles `CompactionStart` by calling `self.state.indicator.set(IndicatorKind::Compaction, Some(msg))` with `msg = "Compacting context..."`, and `app/render.rs:87-90` appends `(${keyText("app.interrupt")} to cancel)`. **None of it reaches the screen.** The source is real and correct-looking; what is missing is whatever makes that indicator state render during a compaction, which is why no amount of source reading found this — it is a defect of the assembled application, visible only when run.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:3076-3078` @v0.83.0 — `case "compaction_start"` calls `this.showStatusIndicator(new CompactionStatusIndicator(…))`, and pi's indicator is on screen for the whole operation.

**Impact** — A manual `/compact` measured at 10.5 s, sampled every 200 ms for the first 4 s and every 1 s thereafter, with **no keys sent** during the window:

```
[t=0.2s] complete=0 ind=          [t=5s]  complete=0 ind=
[t=0.4s] complete=0 ind=          [t=8s]  complete=0 ind=
   ...                            [t=10s] complete=0 ind=
[t=4.0s] complete=0 ind=          [t=11s] complete=1 ind=

# ind = `tmux capture-pane -p | sed -n '/cancel\|Compact/p'` — EMPTY at every sample.
```

A full pane dump at t=3 s of a second compaction likewise contained no spinner and no indicator text; the transcript region was literally blank lines. So for ten to twenty seconds the shipped TUI gives the user **no spinner, no progress, no key hint and no indication anything is happening** — during an operation that is billing a provider call and rewriting the session. The natural user response is to assume a hang and kill the process, which is exactly when the session file is being mutated.

**Fix** — Establish why `IndicatorKind::Compaction` does not reach the frame (the most likely cause is the indicator being gated on the streaming state, which a manual `/compact` outside a turn does not set — start there), and make the band render for the duration with pi's text and the cancel hint. **Schedule with SESS-040**: that item wires Escape, and Escape wired to an invisible band is still an invisible band; this item makes the band visible, and a visible band advertising a dead key is `SESS-040`'s original complaint. Neither alone is shippable.

**Verify** — App test asserting the rendered frame contains `Compacting context...` for every frame between `CompactionStart` and `CompactionEnd`. Then, per the standing rule, a **live terminal run**: issue `/compact`, sample the pane at 200 ms intervals, and assert the indicator text is present in every sample until the completion line.

## TUI-056 — The context-usage meter resets to `0.0%` after an aborted turn while the conversation is still in the transcript

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** medium — observed live; the cyrup-side cause was not isolated · **observed 2026-08-13** (live-terminal; [`REPRO-LOG.md`](REPRO-LOG.md))

> **Scope note, because half of the original observation was struck.** The same session also showed
> the meter rendering a literal `?` after a successful compaction. That is **not** a defect: it is a
> faithful port of pi's `contextPercent === "?"` branch (`footer.ts:151-152`), implemented at
> `crates/cyrup-tui/src/status.rs:377` and documented at `:364-365` as the intended rendering "when
> a compaction has left it unknown". Only the aborted-turn behaviour below is filed. Do not re-file
> the `?`.

**cyrup** — `crates/cyrup-tui/src/status.rs:366-377` `context_text()` renders `{pct}%/{window}` from `set_context_usage`. After an aborted turn the segment was observed at `0.0%/131k` — window known, percent computed as **zero** — which is a different state from the `?` branch (`percent: None`) and is therefore not covered by it. The producing side was not isolated; the item is filed on the observation.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/components/footer.ts:148-153` @v0.83.0 renders the same segment from `getContextUsage()`. pi's abort path does not reset the usage accounting; the transcript is unchanged by an abort, so the occupancy is unchanged.

**Impact** — Measured on the same session, one abort apart:

```
before abort: ↑26k ↓1.6k R31k CH94.3% $0.005 4.5%/131k (auto)
after  abort: ↑26k ↓1.6k R31k          $0.005 0.0%/131k (auto)
```

The conversation is still in the transcript and still in the session file, but the meter reads "the context is empty". A user watching the meter to decide when to compact is given a number that says they have just been handed a fresh window. Low severity because it is a display value with no downstream consumer, and because it self-corrects on the next turn.

**Fix** — Trace the `set_context_usage` call on the abort path (start from the `Interrupt` arm at `app/input.rs:130-213` and the `AgentEnd`/abort event handling) and stop it re-seeding the meter from a zero or absent usage snapshot; the transcript is unchanged by an abort, so the last known usage should stand. Note the cache-hit segment (`CH94.3%`) also disappears at the same moment, which suggests one shared reset rather than two.

**Verify** — App test: drive a turn, abort it, and assert the context segment retains its pre-abort percentage and the `CH` segment is unchanged. Live run to confirm, since this was only ever seen assembled.

## TUI-057 — Slash-command palette submission is inconsistent: sometimes one Enter runs the command, sometimes two, sometimes a trailing space suppresses it

**Kind** port-divergence · **Severity** low · **Effort** M · **Confidence** **low — observed live but driven through tmux, so an instrument race cannot be excluded** · **observed 2026-08-13** (live-terminal; [`REPRO-LOG.md`](REPRO-LOG.md))

> **Filed at low confidence, deliberately, with its caveat intact.** This is a lead, not a
> characterised defect. It is filed rather than dropped because it was seen repeatedly across one
> session and because the alternative explanation is itself a defect — see Impact.

**cyrup** — Not isolated. The behaviour spans the palette's completion-acceptance path and the editor's `E::Submit` arm; no single call site was identified, and no static claim is made here.

**upstream** — Not re-read for this item; pi's palette submission semantics were not established this pass. **This must be settled before the item is worked**, or the fix risks pinning cyrup-original behaviour.

**Impact** — Across roughly eight `/compact` invocations driven identically via `tmux send-keys`, submission behaviour varied: sometimes a single Enter after typing `/compact` ran the command; sometimes the first Enter only accepted the completion and a second was needed; and in one case `"/compact "` (with a trailing space) followed by Enter left the text sitting in the editor with the command unrun:

```
────────────────────────────────────────────────────────
/compact
────────────────────────────────────────────────────────
↑15k ↓2.6k R8.9k CH87.6% $0.004 4.3%/131k (auto)
```

— captured *after* Enter had been sent. **The caveat is load-bearing and is not a reason to dismiss the item:** driven through tmux, a race between the palette's async population and the Enter keypress cannot be excluded — but a human types at comparable speed, so **that race would itself be the defect**, not an artefact of the harness.

**Fix** — First reproduce deterministically: drive the palette from a test harness rather than a terminal, with the completion source stubbed to resolve on a controllable schedule, and establish whether Enter-before-population is the trigger. Then read pi's palette submission path and port its ordering. Do not change behaviour before both steps.

**Verify** — Once characterised: a test that submits `/compact` with the completion source resolving after the keypress and asserts the command runs exactly once; plus the trailing-space case. Then a live run issuing the same command ten times and asserting ten executions.

## TUI-058 — Deleting a paste marker does not renumber the pastes that follow it

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed — **new, found 2026-08-13 while porting TUI-042/043** · **FIXED 2026-08-13**

**cyrup (at HEAD)** — `backspace()` (`editor.rs:801-816` at HEAD) did `self.pastes.remove(&id)` and nothing else.

**upstream** — `handleBackspace` (`editor.ts:1291-1315` @v0.83.0) does four things when the deleted grapheme is a paste marker: `pastes.delete(targetId)`, `pasteCounter--`, shift every registry entry with a higher id down by one ("`[paste #3]` becomes `[paste #2]` when `[paste #1]` is removed"), and rewrite the buffer text with `line.replace(PASTE_MARKER_REGEX, …)` so the visible ids follow. The text rewrite is **not** filtered by `validIds` — it renumbers any syntactic marker.

**Impact** — after deleting an earlier marker, cyrup's remaining marker ids diverged permanently from pi's: the user saw `[paste #2 …]` where pi shows `[paste #1 …]`, and the next paste took an id pi had already recycled. Cosmetic on its own; it compounds TUI-042, because the ids in the buffer and the ids in the registry are the *same namespace* the undo snapshot restores.

**Fixed** — ported literally as `InputEditor::drop_paste` + `renumber_markers` (`editor.rs`). A `BTreeMap` gives the ascending iteration upstream buys with `.sort()`. **One upstream hazard is reproduced rather than corrected, and is flagged in the code**: upstream computes the deletion offsets *before* the rewrite and re-reads the line *after* it (`:1317-1322`), so renumbering a two-digit id (`#10` → `#9`) on the same line ahead of the caret shifts the deletion by one column. Reachable only with ≥10 live pastes in one prompt. Deviating there would have been an un-signed-off design change; it is called out here so the decision is visible.

**Verify** — `crates/cyrup-tui/src/tests/editor.rs::deleting_a_marker_renumbers_the_pastes_that_follow_it` (RED at HEAD): paste two large blocks, backspace over the first, assert the survivor reads `[paste #1 …]` **and** that `expanded_text()` returns the second block's content.

---

## TUI-059 — Only Left/Right clear `lastAction`, so a kill survives every other motion and the next kill accumulates into the same ring entry

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed — **new, found 2026-08-13** · **FIXED 2026-08-13**

**cyrup (at HEAD)** — `apply_editor_action` set `last_action = LastAction::None` in the `CursorLeft`/`CursorRight` arms only. `CursorUp`, `CursorDown`, `CursorWordLeft`, `CursorWordRight`, `CursorLineStart`, `CursorLineEnd` and the history-recall path left it untouched.

**upstream** — every motion clears it: `moveCursor` (`editor.ts:1791`), `moveToLineStart`/`moveToLineEnd` (`:1783`/`:1787`), `moveWordBackwards`/`moveWordForwards` (`:1870`/`:2065`), `navigateHistory` (`:430`), `jumpToChar`, `pageScroll`.

**Impact** — measured: `hello world`, Ctrl+W, Home, Ctrl+K produced the single kill-ring entry `"worldhello "` where pi produces two entries and leaves `"hello "` on top, so Ctrl+Y pasted a mangled concatenation of two unrelated kills. The same stale flag left Alt+Y (yank-pop) armed after a vertical motion, where pi disarms it.

**Fixed** — the six arms now clear it, each citing its upstream function. **Verify** — `::a_motion_between_two_kills_starts_a_new_kill_ring_entry` (RED at HEAD: `left: Some("worldhello ")`).

---

## TUI-060 — The wrap/visual-line map is not paste-marker aware, so a marker can be split across visual rows

**Kind** parity-bug · **Severity** low · **Effort** M · **Confidence** confirmed — **new, found 2026-08-13, open**

**cyrup** — `word_wrap_line` (`editor.rs`) segments with plain extended grapheme clusters, and its `[CYRUP-DELTA]` block states the divergence as a fact about cyrup's segments ("never composite") without noting the consequence.

**upstream** — both wrap callers pass the marker-merging segmentation explicitly: `layoutText` at `editor.ts:928` and `buildVisualLineMap` at `:1745` are each `wordWrapLine(line, width, [...this.segment(line, "grapheme")])`. A `[paste #N …]` marker is therefore one indivisible segment for wrapping, and `wordWrapLine:163-178` exists precisely to re-wrap a segment wider than the line.

**Impact** — a marker that straddles the right edge is torn across two visual rows upstream keeps whole. Because the same map drives vertical motion and the caret's screen position ([`Self::cursor_in`]), the divergence is not purely cosmetic. Low because the marker is ~20 columns and the tear needs a narrow terminal or a long prefix.

**Fix** — thread a marker-merged segment list into `word_wrap_line` as pi's `preSegmented` parameter. Note the recursion at `:163-178` is what upstream needs for an over-wide *composite* segment; cyrup's existing delta comment about an over-wide single cluster stays valid and should be kept alongside it.

**Verify** — a marker positioned to straddle the wrap width occupies one visual line, not two, and `visual_line_map()` reports its whole span on that line.

---

## TUI-061 — `set_text` collapses pi's `setText` and `setTextInternal` into one function, so the paste registry outlives a programmatic buffer replacement

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed — **new, found 2026-08-13, open**

**cyrup** — one `pub fn set_text` (`editor.rs`) serves both the programmatic replacement (`app/session_bind.rs:333`, the queued-message restore) and history browsing (`history_older`/`history_newer`). It clears neither `pastes` nor `paste_counter` and pushes no undo snapshot.

**upstream** — two functions. `setText` (`editor.ts:1010-1021`) cancels autocomplete, clears `lastAction`, exits history browsing, **pushes an undo snapshot when the content actually differs**, then `this.pastes.clear(); this.pasteCounter = 0;` before delegating. `setTextInternal` (`:1043-1056`) — "Internal setText that doesn't reset history state - used by navigateHistory" — does none of it.

**Impact** — after a programmatic buffer replacement the registry keeps entries whose markers are no longer in the buffer, so a subsequently typed `[paste #1 1500 chars]` still expands (TUI-049's surface, narrowed but not closed by that fix), and the replacement is not undoable. Low: both call sites replace the buffer with text the user can retype.

**Fix** — split the two, exactly as upstream does, and route `history_older`/`history_newer` to the internal form; the external form gets the snapshot + registry reset. Small, but it changes a `pub` method's contract, so it is filed rather than folded into the paste-registry batch.

**Verify** — `set_text` after a large paste leaves `expanded_text()` unable to resolve a hand-retyped marker; a `set_text` that changes the content is undoable with Ctrl+-.


## TUI-063 — `CYRUP_SHARE_VIEWER_URL` is advertised in `--help` and read by nothing

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup/src/cli.rs:1077` prints `CYRUP_SHARE_VIEWER_URL           - Base URL for /share command` in the environment section of `cyrup --help`. A grep over `crates/` for `CYRUP_SHARE_VIEWER_URL` or `SHARE_VIEWER` returns **that line and nothing else**; the `/share` handler (`crates/cyrup-tui/src/app/execute_misc.rs:306`) never consults it.

**upstream** — pi exposes a share-viewer base URL; the cyrup-side consumer was never built. The upstream symbol and tag were **not re-read this pass**.

**Impact** — a user who sets the variable to point `/share` at a self-hosted viewer gets no effect and no diagnostic; the gist is still produced against the default. This is precisely the *advertised but unimplemented* shape `README.md` blind spot 5 names, and it was invisible to area 12's env-var sweep because that sweep is delta-scoped to v0.83.0→v0.84.1 and cannot see a cyrup-invented variable.

**Fix** — either consume the variable in the `/share` handler when composing the viewer link, or delete the help line. Deleting is the smaller change and is exactly what `crates/cyrup/src/subcommands.rs:297-302` did for the dead npm install example.

**Verify** — set `CYRUP_SHARE_VIEWER_URL` to a sentinel host, run `/share`, and assert the emitted link uses it; or assert the help text no longer names it.


## TUI-064 — the attached-image strip has no production callers

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed

**cyrup** — `App::attach_image` (`crates/cyrup-tui/src/app/shell.rs:294`) and `App::attach_image_path` (`:300`) are `pub` and populate `pending_images`, which the layout renders as its own region. The only callers in the workspace are three unit tests (`crates/cyrup-tui/src/tests/image.rs:54`, `:79`, `:115`). No input path reaches them: `Ctrl+V` writes the clipboard image to a temp `.png` and inserts the **path as text** (`app/shell.rs:318-358`), and an `@file` mention likewise passes a path.

**upstream** — pi populates its attachment strip from the paste and mention paths. Tag and line **not re-read this pass**; establish before fixing.

**Impact** — the strip is dead surface in the shipped binary. `TUI-017` audits how that strip *renders* — Halfblocks without a protocol, the invented placeholder glyph, the missing 60-cell cap — and implicitly assumes it is populated. **`TUI-017` is marked CLOSED, and this item is the falsification condition its closure did not write down:** with no production caller, that closure can only have been validated against unit tests driving `attach_image` directly, never against a user pasting an image. That is the third failure mode the third edition names — *a closure validated against the wrong signal* — and it is the same shape that reopened `TOOL-042` by measurement. This item does **not** reopen `TUI-017` on its own; it records what would. Filed separately rather than folded in because the fix sites differ.

**Fix** — call `attach_image_path` from the clipboard-paste handler, and from `@`-mention acceptance when the resolved path is an image, **alongside** (not instead of) inserting the path text — the current text-path behaviour is deliberate, since it keeps the raster out of context until the agent reads it.

**Verify** — paste an image and assert `pending_images` is non-empty and the strip occupies rows; keep `TUI-017`'s rendering assertions on top. Per this directory's own caveat, TUI work is not done until run in a real terminal.


## TUI-065 — `app.pageUp` / `app.pageDown` are cyrup-invented ids, and each now resolves in **two** keymaps at once

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/keymap.rs:270-271` (`"app.pageUp" => Some(Action::PageUp)`, `"app.pageDown" => …`), `Action` variants at `:191-194`, default-bound at `:647-648` (`Key::plain(KeyCode::PageUp) → Action::PageUp`); the **same two strings** are also accepted as `EditorAction` aliases at `:403-404`.

**upstream** — pi has no `app.*` page id at either v0.83.0 or v0.84.1. Paging is `tui.editor.pageUp` / `tui.editor.pageDown` (`pi/packages/tui/src/keybindings.ts:89-90`), and pi's own `/hotkeys` reads it off the editor map (`getEditorKeyDisplay("tui.editor.pageUp")`).

**Impact** — half of this was known: TUI-028's closure note says it "absorbed TUI-008's `app.pageUp`/`app.pageDown` spelling complaint" and that the two ids "stay as cyrup-original ids on the global map and are listed as a new gap" — **but no row was ever added, so nothing tracks them.** This item is that row. The **new** half is a consequence of TUI-028's alias work landing: `app.pageUp` now resolves in TWO maps at once, so `{"app.pageUp": "f5"}` rebinds both the global transcript scroll and the editor caret page, and stock `PageUp` is bound in both `Keymap::default` and `EditorKeymap::default`. Which one fires is decided by `App` routing order rather than by any upstream rule, and `load_keybindings_json` fans one document out to six maps, so a single config entry moves two different actions.

**Fix** — decide one owner for the id. Either (a) accept `app.pageUp`/`app.pageDown` on the **global** map only and delete the `EditorAction::from_id` aliases at `:403-404`, leaving `tui.editor.pageUp`/`pageDown` as the editor spelling pi uses; or (b) delete the `app.*` spelling outright and let the deferral logic at `app/input.rs:63-80` route the editor id to the transcript when the editor is empty. (a) is the smaller change and keeps shipped configs working. Whichever is chosen, the surviving cyrup-only id must be recorded in TUI-066's list and kept out of user documentation.

**Verify** — a keymap test asserting that one JSON document containing `app.pageUp` changes exactly one map, and that `Keymap::default()` and `EditorKeymap::default()` do not both claim bare `PageUp`.


## TUI-066 — the cyrup-only keybinding-id and key-spec vocabulary that TUI-028's closure deliberately preserved is tracked nowhere

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — three invented vocabularies survive at HEAD, all of them by a deliberate decision recorded in TUI-028's closure ("must NOT delete the `editor.*` arms … which are what keeps shipped-cyrup configs working"), and none of them carried by an open row once TUI-028 closed:

1. **`tui.autocomplete.previous` / `.next` / `.accept` / `.acceptSubmit` / `.cancel`** — `crates/cyrup-tui/src/keymap.rs:845-881` (`AutocompleteAction` + `from_id`), defaults at `:895-901`. Upstream has no `tui.autocomplete.*` family at all; pi's popup reuses `tui.select.up/down/confirm/cancel` and `tui.input.tab` (`pi/packages/tui/src/components/editor.ts:664-712` @v0.83.0). The doc comment at `keymap.rs:862-869` states this honestly, and TUI-028 added pi's ids alongside, so a pi-shaped config now works — the five originals survive as do-not-break-shipped-config aliases.
2. **The bare `editor.*` namespace, 19 aliases** (`editor.cursorUp` … `editor.tab`) — `keymap.rs:360-412`, the middle alternative of each `|`-arm. These match **neither** pi's modern `tui.editor.*` **nor** pi's legacy bare `cursorUp`, so they are a cyrup-only third spelling.
3. **`Key::parse` spec tokens pi's `KeyId` does not contain** — `backtab`, `del`, `ins`, `pgup`, `pgdn`, `control`, `option`, `meta`, `cmd`, `command` (`keymap.rs:502-532`: modifier arms `:502-505`, key arms `:508`, `:518`, `:521-522`, `:532`).

**upstream** — pi's `ModifierName` is exactly `ctrl|shift|alt|super` (`keys.ts:143`) and `SpecialKey` (`keys.ts:109-139`) has no `backtab`/`del`/`ins`/`pgup`/`pgdn`.

**Impact** — cyrup accepts a strict superset of pi's id and key-spec vocabulary, so a `keybindings.json` that works in cyrup can be rejected by pi's TS types and would silently never match at pi runtime. Low impact in behaviour — nobody writes `pgdn` — but this is invented surface that ships, and the class matters: an id or token a user learns from cyrup is not portable, and the ledger had no row for any of it after TUI-028 closed. Not a defect to remove; a defect to leave **unrecorded**.

**Fix** — no code change is proposed. Record the three lists in one place (this item is that place) and add a single-line assertion in `keymap.rs` next to each alias block naming this id, so a future pass reading the code finds the ledger row. If cyrup ever publishes a user-facing keybinding-id reference, it must list **only** the 73 upstream ids; the aliases stay accepted and undocumented.

**Verify** — a keymap test enumerating the accepted-id set and asserting that every id outside pi's 73 appears in this item's list, so a new invented id fails loudly.


## TUI-067 — `tui.input.copy` migrates correctly and is then silently dropped: it has no `EditorAction` destination

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `EditorAction::from_id` (`crates/cyrup-tui/src/keymap.rs:356-415`) has no arm for `tui.input.copy`; all 24 arms were checked. The entry migrates correctly — `crates/cyrup-config/src/keybindings.rs:70` and `:142` both carry it — and is then dropped by `merge_entries`' `let Some(action) = from_id(&id) else { continue }` (`keymap.rs:128`) **with no diagnostic**.

**upstream** — `pi/packages/tui/src/keybindings.ts:34` (id) and `:121` (`defaultKeys: "ctrl+c"`, "Copy selection"); consumed at `pi/packages/tui/src/components/editor.ts:654` `if (kb.matches(data, "tui.input.copy")) { return; }`; doc row `pi/packages/coding-agent/docs/keybindings.md:73`.

**Impact** — this is the ONE id whose whole job is to tell the editor *not* to consume a key, so the app tier gets it. Upstream, rebinding `tui.input.copy` to `ctrl+q` makes the editor forward Ctrl+Q to the parent. In cyrup the rebind is accepted, migrated, and discarded. Stock behaviour is accidentally right — `EditorKeymap::default()` binds no `ctrl+c`, so it falls through to `Action::Clear` — so this is **config-inertness, not a default-chord break**, which is why it stayed invisible: TUI-028 named it only inside its prose ("`tui.input.copy` also has no cyrup destination at all", `07-cyrup-tui.md:692`) and its Fix section never listed it, so TUI-028's closure did not cover it and no step ever owned it.

**Fix** — add `EditorAction::PassThrough` (name it for what it does upstream: the editor declines the key) with a `"tui.input.copy" | "input.copy"` arm in `from_id`, bound to nothing by default, and have `InputEditor::handle_input` return early when it resolves — the port of `editor.ts:654`'s bare `return`.

**Verify** — editor test: with `{"tui.input.copy": "ctrl+q"}` merged, Ctrl+Q is not consumed by the editor and reaches the global keymap; without the rebind, nothing changes.


## ~~TUI-068~~ — ~~`app.session.deleteNoninvasive` is unbound **and** unbindable: Ctrl+Backspace does nothing in `/resume`~~ **CLOSED 2026-09-04**

> ## CLOSED 2026-09-04 — `eacd771a` — `crates/cyrup-tui/src/{keymap.rs,session_selector.rs}`
>
> **What landed.** `SessionAction::DeleteNoninvasive` (`crates/cyrup-tui/src/keymap.rs:1199`), the
> `"app.session.deleteNoninvasive"` arm in `SessionAction::from_id` (`:1211`) — so `merge_entries` no
> longer skips the id — and the default binding `ctrl+backspace` in `SessionKeymap::default()` (`:1240`),
> upstream's `core/keybindings.ts:177-180` @v0.84.4 (`:151-154` @v0.83.0, identical). The handler arm sits
> in `SessionSelector::handle` step 4, the `app.session.*` table (`crates/cyrup-tui/src/session_selector.rs:1017`),
> and is therefore resolved **before** the search-`Input` fallthrough in step 6 that used to swallow the
> modifier. It is the port of `session-selector.ts:590-600` @v0.84.4: with a non-empty query the event is
> handed to `Input::handle_key` (pi's `this.searchInput.handleInput(keyData)`) and, if the input edited,
> the highlight resets to the top (pi's `filterSessions`); with an empty query it calls
> `start_delete_confirmation_for_selected` (`:391`), the shared port of
> `startDeleteConfirmationForSelectedSession` (`session-selector.ts:394-403`) that `app.session.delete`
> now also uses. The hint row is unchanged: pi's `hint2Parts` (`:171-179`) does not name the alias.
> `docs/guide/reference/keybindings.md:146` gains the `/resume` row.
>
> **Tests** (all RED against the pre-commit code, GREEN after): `session_selector::tests::`
> `tui068_ctrl_backspace_with_an_empty_query_arms_the_delete_confirmation` (arms, and Enter then deletes
> `/s/a.jsonl`), `tui068_ctrl_backspace_with_a_query_forwards_to_the_input_and_keeps_the_list` (`Redraw`,
> not the old `Ignored`; no confirmation; filter intact), `tui068_ctrl_backspace_with_a_query_honours_an_editor_rebind`
> (`tui.editor.deleteWordBackward` bound to the chord edits the query and re-filters; the now-empty query
> then arms on the next press), `tui068_delete_noninvasive_is_rebindable_from_json`
> (`{"app.session.deleteNoninvasive": "ctrl+k"}` moves it, no `KeybindingIssue`), and
> `tests::keymap::tui068_session_delete_noninvasive_resolves_and_defaults_to_ctrl_backspace` (a compile
> failure before: the variant did not exist). Full crate: 1368 passed.
>
> **Design.** No new type (DESIGN-GUIDANCE applied): the empty/non-empty branch is a runtime property of the
> live search box, and the outcome already has its domain state (`confirming_delete`) and enum
> (`SelectorOutcome`). A typestate or a forward-outcome enum was rejected as ceremony pi's `void` return
> gives nothing to consume.
>
> **Residual — low, new, not this row.** pi's `startDeleteConfirmationForSelectedSession` refuses the
> **current** session (`session-selector.ts:398-401` @v0.84.4: `onError("Cannot delete the currently active
> session")` when `isCurrentSessionPath`); neither cyrup delete path (`Ctrl+D` or `Ctrl+Backspace`) carries
> that guard, so the live session's file can be trashed from `/resume`. `SessionSelector` already holds
> `current_path`, so the guard is a one-line check plus the error status.
>
> Original item text, for the record:


**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `SessionAction` has 5 variants and `SessionAction::from_id` (`crates/cyrup-tui/src/keymap.rs:1055-1064`) 5 arms — ToggleSort / ToggleNamedFilter / Delete / TogglePath / Rename — and `SessionKeymap::default()` (`:1078-1084`) 5 bindings. `deleteNoninvasive` is in none of them, so the default `ctrl+backspace` is unbound AND unbindable. The chord itself is **not** the f1..f12 failure mode sweep 9 found: `Key::parse("ctrl+backspace")` resolves through the `"ctrl"` modifier arm (`:502`) and the `"backspace"` key arm (`:518`), so the spec parses and only the destination is missing.

**upstream** — `pi/packages/coding-agent/src/core/keybindings.ts:42` (id) and `:151-154` (`defaultKeys: "ctrl+backspace"`, "Delete session when query is empty"); consumed at `pi/packages/coding-agent/src/modes/interactive/components/session-selector.ts:592-600`; rename entry `deleteSessionNoninvasive` at `:268`; doc row `pi/packages/coding-agent/docs/keybindings.md:105`.

**Impact** — in `/resume` with an empty search query, upstream Ctrl+Backspace opens the delete-confirmation for the highlighted session (`session-selector.ts:599` `startDeleteConfirmationForSelectedSession()`). In cyrup the event falls to the catch-all at `session_selector.rs:1013` — `if key.code == KeyCode::Backspace { self.backspace(); }`, which **does not check modifiers** — so Ctrl+Backspace runs a no-op backspace on an already-empty query and the user sees nothing happen. The non-empty-query half (forward to the input, re-filter) is also absent. The id was invisible to nine sweeps because it *does* appear in `crates/cyrup-config/src/keybindings.rs:106,178` and in CFG-048's prose at `05-cyrup-config-and-resources.md:381` — but only as evidence for the **rename table**, never as an unbound action.

**Fix** — add `SessionAction::DeleteNoninvasive` with the `"app.session.deleteNoninvasive"` arm in `from_id`, default `ctrl+backspace` in `SessionKeymap::default()`, and port `session-selector.ts:592-600`: on an empty query start the delete confirmation for the selected session; otherwise fall through to the query input and re-filter. Resolve it **before** the `KeyCode::Backspace` catch-all at `session_selector.rs:1013`, which today swallows the modifier.

**Verify** — session-selector test: Ctrl+Backspace with an empty query opens the delete confirmation; with a non-empty query it edits the query and leaves the list alive; a `{"app.session.deleteNoninvasive": "ctrl+k"}` rebind moves the behaviour.


## TUI-069 — `/hotkeys` printed `Shift+Tab/Shift+Shift+Tab/Shift+Tab` for `app.thinking.cycle`

> ## FIXED 2026-08-14 — `crates/cyrup-tui/src/keymap.rs`
>
> Two changes, both in the label path only; no binding was added or removed. (1) `Key::label` no
> longer prepends `shift+` when the code is `KeyCode::BackTab`, because `BackTab`'s own base label is
> already `shift+tab` — the doubled form is not a chord any terminal reports and `Key::parse` reads it
> back as plain `Tab`+SHIFT, so the label did not round-trip. (2) The five `keys_label` bodies now go
> through one `join_key_labels` helper that skips a label already emitted, which is what makes the
> three-way binding render as pi's single cell.
>
> **RED before, at HEAD:** `Keymap::default().keys_label(Action::ThinkingCycle)` was
> `Some("shift+tab/shift+shift+tab/shift+tab")` and the `/hotkeys` cell
> `Shift+Tab/Shift+Shift+Tab/Shift+Tab`. **GREEN after:** `Some("shift+tab")` → `Shift+Tab`, byte-for-byte
> pi's cell. Test: `hotkey_cells_match_pis_key_text_for_shift_tab_and_the_page_keys`
> (`crates/cyrup-tui/src/tests/keybindings.rs`), which also pins the `BackTab`+SHIFT label directly.
>
> **Execution caveat, stated rather than implied:** the RED values above are derived by reading the
> code at HEAD (the label path is a pure function of the binding table, so they are mechanically
> certain), and the new test compiles under `cargo check -p cyrup-tui --all-targets`. This pass's
> test rules forbid `cargo test`, so the suite was **not executed** here — a reviewer with a green
> light to run tests should do so before treating the row as verified.

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/keymap.rs:667-669` binds three keys to `Action::ThinkingCycle` (`BackTab`, `BackTab`+SHIFT, `Tab`+SHIFT) — defensible, since a terminal reports Shift+Tab three ways depending on the keyboard protocol. But `/hotkeys` renders through `keys_label` (all keys, `/`-joined — `app/hotkeys.rs:11-12` `g = |a| format_key_text(&km.keys_label(a)…, true)`, row at `app/hotkeys.rs:47`, argument at `:88`), and `Key::label` rendered `KeyCode::BackTab` as the literal `"shift+tab"` (`keymap.rs:608`) while **also** prepending its own `shift+` for the SHIFT modifier — the guard that now suppresses the second prefix is `keymap.rs:597-598`.

**upstream** — `pi/packages/coding-agent/src/core/keybindings.ts:73-76` — `defaultKeys: "shift+tab"`, a SINGLE key; rendered by `keyDisplayText` → `formatKeys(getKeys(id))` → `keys.join("/")` (`keybinding-hints.ts:29-40`), i.e. exactly `Shift+Tab`.

**Impact** — the `/hotkeys` cell read `Shift+Tab/Shift+Shift+Tab/Shift+Tab` where pi prints `Shift+Tab`. The middle entry is not a real chord, and a label that does not round-trip through `Key::parse` is a label that lies everywhere hints are drawn. No backlog row existed: a grep for `shift+shift` / `Shift+Shift` across `docs/gap-analysis/` returned nothing.

**Verify** — done, above. Per this directory's standing caveat the cell should also be read once in a real terminal, since `/hotkeys` is a rendered surface.


## TUI-070 — the page keys render as `Pageup` / `Pagedown` where pi renders `PageUp` / `PageDown`

> ## FIXED 2026-08-14 — `crates/cyrup-tui/src/keymap.rs`
>
> `Key::label` now emits upstream's camelCase `pageUp` / `pageDown`. `Key::parse` lowercases every
> token before matching (`:504`), so the label still round-trips — which is the only property the
> old in-code comment at `:519-520` was defending; it never considered display. The two
> `TreeKeymap::first_key_label` rewrite arms (`"pageup" => "pgup"`) were retargeted in the same edit.
>
> **RED before:** `Key::plain(KeyCode::PageUp).label()` was `"pageup"` and `format_key_part`
> capitalizes only the first character, so the `/hotkeys` "Scroll by page" cell read `Pageup`.
> **GREEN after:** `"pageUp"` → `PageUp`. Test:
> `hotkey_cells_match_pis_key_text_for_shift_tab_and_the_page_keys`
> (`crates/cyrup-tui/src/tests/keybindings.rs`), which asserts the label, the parse round-trip and the
> rendered cell.
>
> **Execution caveat, stated rather than implied:** the RED values above are derived by reading the
> code at HEAD (the label path is a pure function of the binding table, so they are mechanically
> certain), and the new test compiles under `cargo check -p cyrup-tui --all-targets`. This pass's
> test rules forbid `cargo test`, so the suite was **not executed** here — a reviewer with a green
> light to run tests should do so before treating the row as verified.

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `Key::label` emitted the lowercased `"pageup"` / `"pagedown"` (`crates/cyrup-tui/src/keymap.rs:615-616`); `format_key_part` (`crates/cyrup-tui/src/chrome.rs:41-56`) then capitalizes only the first character.

**upstream** — `pi/packages/tui/src/keybindings.ts:89-90` `defaultKeys: "pageUp"` / `"pageDown"`; `formatKeys` joins the raw `KeyId` strings and `formatKeyPart` only upper-cases the FIRST character (`keybinding-hints.ts:12-15`), so pi's `/hotkeys` cell is `PageUp`.

**Impact** — every hint naming a page key was one character wrong. Inside `/tree` the divergence was masked by `TreeKeymap::first_key_label`, whose `"pageup" => "pgup"` rewrite (`:1240-1241`) replaced the token before it was ever capitalized — which is why the defect survived: the one surface with a test was the one surface that could not show it.

**Verify** — done, above.


## TUI-071 — three platform-conditional upstream defaults are bound unconditionally in cyrup

**Kind** port-divergence · **Severity** low · **Effort** S · **Confidence** confirmed

Upstream computes three default key sets from `process.platform`. cyrup binds all three unconditionally, so on some platform each one differs from pi's. None carries a `CYRUP-DELTA` row naming pi's file:line, which the hard rules require for a forced difference.

1. **`app.clipboard.pasteImage` — one platform key upstream, two keys on every platform in cyrup.** `crates/cyrup-tui/src/keymap.rs:670-671` binds BOTH `ctrl+v` and `alt+v`. Consequences: (a) on non-Windows, `alt+v` is stolen from the editor, which upstream never does; (b) `/hotkeys` shows `Ctrl+V/Alt+V` (via `keys_label`, `app/input.rs:281`) where pi shows one key. The comment at `keymap.rs:666-669` declares the intent but cites no upstream line.
   **upstream** — `pi/packages/coding-agent/src/core/keybindings.ts:111-114` — `defaultKeys: process.platform === "win32" ? "alt+v" : "ctrl+v"` — exactly one key is ever bound.
2. **`app.suspend` — no win32 gate.** `keymap.rs:644` binds `ctrl+z` unconditionally, and `grep -rn 'Action::Suspend' crates/` shows no `cfg(windows)` guard anywhere on the path (`keymap.rs:269,644`; `app/input.rs:254`, `app/hotkeys.rs:87`, `app/run_action.rs:17`). On Windows the chord is bound and its `/hotkeys` row is populated where upstream leaves both empty.
   **upstream** — `pi/packages/coding-agent/src/core/keybindings.ts:69-72` — `defaultKeys: process.platform === "win32" ? [] : "ctrl+z"`; doc row `packages/coding-agent/docs/keybindings.md` reads "`ctrl+z` (none on Windows)".
3. **`app.tree.foldOrUp` / `app.tree.unfoldOrDown` — key ORDER is platform-dependent upstream, fixed in cyrup.** `keymap.rs:1197-1200` always puts `alt` first. The key SET is right on both platforms, so behaviour is unaffected; only display diverges, and only on Linux/Windows — `TreeKeymap::first_key_label` (`:1231`) takes `getKeys(id)[0]`, matching pi's `formatHelpKeys` (`tree-selector.ts:1238-1253`), so `/tree`'s help row shows `Option+←` where pi shows `Ctrl+←`.
   **upstream** — `pi/packages/coding-agent/src/core/keybindings.ts:119-126` — `process.platform === "darwin" ? ["alt+left","ctrl+left"] : ["ctrl+left","alt+left"]` (and the mirror for right). The shipped doc table lists the NON-darwin order: `packages/coding-agent/docs/keybindings.md` rows read "`ctrl+left`, `alt+left`".

**Impact** — (2) and (3) are display-only or Windows-only. (1) is behavioural on the primary target: Alt+V is taken from the editor on macOS and Linux, which upstream never does. Filed as one item because the three share a fix site (`Keymap::default` / `TreeKeymap::default`) and one decision: does cyrup key off `cfg!(windows)` / `cfg!(target_os = "macos")` the way pi keys off `process.platform`?

**Fix** — port the conditionals literally: `if cfg!(windows) { alt+v } else { ctrl+v }`; `if !cfg!(windows) { ctrl+z }`; and order the tree keys `darwin ? [alt, ctrl] : [ctrl, alt]`. If the pasteImage double-binding is kept deliberately, it needs a `CYRUP-DELTA` comment naming `core/keybindings.ts:111-114` and an item saying so — the current comment is an intent statement with no citation.

**Verify** — keymap tests asserting the bound set per `cfg!`, plus `keys_label(Action::ClipboardPasteImage)` returning one key on non-Windows and `keys_label(Action::Suspend)` returning `None` under `cfg(windows)`.


## TUI-072 — four editor key sets carry their v0.84.1 extras against a v0.83.0 baseline

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/keymap.rs` adds `ctrl+home`, `ctrl+end` (`:1302`, `:1305`) and `ctrl+pageup`, `ctrl+pagedown` (`:1312`, `:1314`) to `tui.editor.cursorLineStart` / `cursorLineEnd` / `pageUp` / `pageDown`. These are the **v0.84.1** sets. The in-code comment at `:1298-1310` already discloses this as version lag rather than a port bug.

**upstream** — `pi/packages/tui/src/keybindings.ts:73-80` @v0.83.0 — `["home","ctrl+a"]` / `["end","ctrl+e"]`; `:89-90` — `"pageUp"` / `"pageDown"` (single key).

**Impact** — it changes the `/hotkeys` cells (`Home/Ctrl+Home/Ctrl+A` vs pi's `Home/Ctrl+A`) and it inflates any cyrup-side key count taken against the ported tag. Nothing is broken; the reason for the row is that a *forward*-ported default is exactly as invisible as a missing one, and the ledger had no record of these four. Recorded for completeness with the same disposition as TUI-035's forward-ported `tui.editor.historyPrevious` / `historyNext` ids, which are the other half of the same drift and are already closed.

**Fix** — none required while cyrup deliberately tracks the newer sets. When the baseline moves to v0.84.1 this row closes with no code change; if the baseline stays at v0.83.0, drop the four extra keys and the `/hotkeys` cells match again.

**Verify** — a keymap test pinning the four key sets against whichever tag is declared the baseline, so the next upstream change to them fails loudly.


## TUI-073 — `clear` is a valid pi `KeyId` that `Key::parse` rejects

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/keymap.rs:530-531` deliberately omits it ("crossterm's `KeyCode` has no counterpart"). Consequence: `{"app.interrupt": "clear"}` is accepted by pi and produces a `KeybindingIssue` in cyrup — the multi-character token falls to the `_ => Err(TuiError::KeySpec)` arm at `:546`.

**upstream** — `pi/packages/tui/src/keys.ts:119` (`SpecialKey` member), with real sequence tables at `:379`, `:399`, `:413`, `:429-430` and a real `matchesKey` arm at `:990-994`.

**Impact** — spec-vocabulary only: **no upstream DEFAULT uses `clear`**, so no default chord is dead, and the other 39 `SpecialKey`s all parse — including `insert` and `f1`…`f12` (`:532-541`, sweep 9's fix, verified present at HEAD). The failure is confined to a hand-written config that names the key: pi binds it, cyrup reports the entry as an issue and leaves the action on its previous binding. This is the residual of the chord-parse audit run over this surface: **all 58 distinct default key-spec strings pi ships parse under `Key::parse`**, including `ctrl+-`, `ctrl+]` and `ctrl+alt+]` (which reach the single-char fallback at `:542-548`), so `clear` is the only spec-vocabulary hole left.

**Fix** — either map it to the closest crossterm code and document the approximation, or keep rejecting it and say so in the diagnostic (`unsupported key "clear"` rather than a generic parse failure) so the user is not left guessing whether they typo'd. The second is honest and is one line.

**Verify** — `Key::parse("clear")` yields the specific unsupported-key diagnostic, and a config naming it reports exactly one issue while every other entry still applies (CFG-038's contract).


## TUI-074 — dispatch arity: cyrup accepted an argument on all 25 slash commands where pi accepts one on 6, so `/quit now` quit

> ## FIXED 2026-08-14 — `crates/cyrup-tui/src/commands.rs`
>
> `match_command` now takes the `"name "`-prefix branch only for the six names in the new
> `ARGUMENT_DISPATCH_NAMES` const; every other name is exact-match, which is upstream's guard. The
> registry abstraction is kept — this is the data the uniform matcher was missing, not a rewrite.
>
> **RED before, at HEAD:** `CommandRegistry::new().dispatch("/quit now")` returned
> `Dispatch::Command { name: "quit", arg: Some("now") }`; likewise `/copy that`, `/new session`,
> `/trust me`, `/tree left`, `/debug on`. **GREEN after:** all six are `Dispatch::Prompt`, and the six
> argument-taking commands are unchanged. Test:
> `only_pis_six_argument_commands_accept_trailing_text` (`crates/cyrup-tui/src/tests/commands.rs`).
> The existing suite exercised only prefix-accepting commands (`/model`, `/compact`), which is why
> nothing pinned the nineteen exact-only guards.
>
> **Execution caveat, stated rather than implied:** the RED values above are derived by reading the
> code at HEAD (the label path is a pure function of the binding table, so they are mechanically
> certain), and the new test compiles under `cargo check -p cyrup-tui --all-targets`. This pass's
> test rules forbid `cargo test`, so the suite was **not executed** here — a reviewer with a green
> light to run tests should do so before treating the row as verified.

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `match_command` (`crates/cyrup-tui/src/commands.rs:240-253`) applied exact-or-`"name "`-prefix **uniformly** to all 25 `dispatch_names`, and the crate's own doc at `:205` claimed this matched `interactive-mode.ts`.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:2666-2793` @v0.83.0 — compare `:2666` `if (text === "/settings")` against `:2676` `if (text === "/model" || text.startsWith("/model "))`. pi accepts an argument on exactly 6 of its 25 dispatch names: `/model`, `/export`, `/import`, `/name`, `/login`, `/compact`. The other 19 — `/settings`, `/scoped-models`, `/share`, `/copy`, `/session`, `/changelog`, `/hotkeys`, `/fork`, `/clone`, `/tree`, `/trust`, `/logout`, `/new`, `/reload`, `/resume`, `/quit`, `/debug`, `/arminsayshi`, `/dementedelves` — are strict equality, so `/quit now` is NOT a command and is sent to the model as a prompt.

**Impact** — `/quit now` quit, `/copy that` copied, `/new session` started a new session and `/trust me` opened the trust selector, every one of which pi forwards to the agent verbatim. The user-visible cost is a destroyed prompt: text the user typed and expected the model to see was swallowed by a command.

**Also recorded here, per the port-fidelity rule** — the *mechanism* was substituted. pi has no registry object and no dispatch enum; `setupEditorSubmitHandler` (`interactive-mode.ts:2660-2793`) is 25 hand-written `if (text === "/x" …)` guards in source order. `CommandRegistry` / `Dispatch` (`commands.rs:122-254`) is reasonable Rust and is kept, but it is the direct cause of this divergence: a uniform matcher cannot express a per-command mix of exact-only and prefix-accepting guards without carrying the arity as data, which is what the fix adds. No separate id is filed for the substitution; this paragraph is its record.

**Verify** — done, above.


## TUI-075 — the `/` menu lists extension commands before prompt templates; pi lists prompts first

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `CommandRegistry::with_dynamic` (`crates/cyrup-tui/src/commands.rs:184-188`) appends `slash_command_catalog` rows in catalog order, and the catalog emits extensions first (`crates/cyrup-session-svc/src/session.rs:2503` extensions, `:2517` prompts, `:2526` skills). Result: builtins → extension commands → prompt templates → skill commands.

**upstream** — `pi/…/interactive-mode.ts:625` @v0.83.0 — `[...slashCommands, ...templateCommands, ...extensionCommands, ...skillCommandList]`, i.e. builtins → prompt templates → extension commands → skill commands.

**Impact** — order is user-visible: both sides' fuzzy filter returns `items` unchanged for an empty query (pi `fuzzy.ts:100-102`; cyrup `fuzzy.rs::filter`), so pressing `/` alone shows the two middle blocks swapped. A user with several extensions has to scroll past them to reach their own prompt templates, which upstream puts first.

**Fix** — sort the merged dynamic rows by source into pi's order (prompt → extension → skill) inside `with_dynamic`, or emit the catalog in that order in `session.rs:2503-2526`. Prefer `with_dynamic`: the catalog is also the RPC `get_commands` payload, whose order is pi's RPC order and should not be changed to fix an interactive-mode display.

**Verify** — a registry test with one prompt, one extension and one skill row asserting the emitted order, and an autocomplete test asserting the `/`-with-empty-query list.


## TUI-076 — the builtin-collision filter keys on `invocation_name`, so a suffixed duplicate survives into the menu and never dispatches

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `CommandRegistry::with_dynamic` (`crates/cyrup-tui/src/commands.rs:182-188`) filters the catalog row's `name` against the builtin set — but that field is already the **invocation name** (`crates/cyrup-session-svc/src/session.rs:2506`).

**upstream** — `pi/…/interactive-mode.ts:600-608` @v0.83.0 — `.filter((cmd) => !builtinCommandNames.has(cmd.name))` **then** `.map((cmd) => ({ name: cmd.invocationName, … }))`: pi filters on the extension command's ORIGINAL `name` and only then maps to `invocationName`.

**Impact** — if two extensions both register `model` (invocation names `model:1` and `model:2` per `runner.ts:598-641`), pi drops BOTH from autocomplete; cyrup keeps both, so the `/` menu offers two commands that `dispatch_names` will never route — they fall through to `Dispatch::Prompt` and are sent to the model as literal text. An advertised command that cannot run is the same shape as TUI-063, one level in.

**Fix** — carry the original `name` alongside `invocation_name` on the catalog row (pi has both on the object it filters), and filter on the original before the merge. Lands next to TUI-006, which owns the *diagnostic* for the same collision — this item is the *filtering*; do not book them twice.

**Verify** — registry test: two dynamic rows whose original name is `model` and whose invocation names are `model:1`/`model:2` are both absent from `commands()`.


## TUI-077 — a slash-argument context falls through to path completion, and the name/argument split rule is not pi's

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `slash_context` (`crates/cyrup-tui/src/autocomplete.rs:141`) returns `None` on any whitespace **or** on an empty fuzzy result, and `Autocomplete::compute` (`:88-93`) then falls into `path_context`. Two divergences follow:

1. **Fallthrough.** pi's slash branch is terminal: once `!force && textBeforeCursor.startsWith("/")`, every path out of the branch returns — a filtered command list, argument suggestions, or `null` — and it never reaches `extractPathPrefix`. Observable: with a popup already open (`was_open`), typing `/export ./sr` pops a FILE list in cyrup where pi shows nothing until Tab; and `/Users/dav` eventually pops a path list in cyrup where pi closes the popup.
2. **The split rule.** pi splits on `textBeforeCursor.indexOf(" ")` — a literal SPACE only, so a TAB character typed into the editor keeps pi in the command-name branch. cyrup uses `before.contains(char::is_whitespace)`, which also catches tab, no-break space and the rest of Unicode whitespace.

**upstream** — `pi/packages/tui/src/autocomplete.ts:308-359` @v0.83.0 — the whole `if (!options.force && textBeforeCursor.startsWith("/"))` block, whose no-completions exit is `return null` at `:347` and `:352`; the split itself at `:309`, `:339-340` (`slice(1, spaceIndex)` / `slice(spaceIndex + 1)`).

**Impact** — (1) is visible today: a path popup appears under a slash line that upstream leaves closed. (2) is cosmetic today and is filed with it because it is the same three lines and because TUI-012's argument-completion port must not inherit a different split rule than pi's.

**Fix** — make the slash branch terminal in `compute`: when `before` starts with `/` and `!force`, return whatever the slash path produced (including `None`) without trying `path_context`. Split on `' '` rather than `char::is_whitespace`. Land with TUI-012, which adds the argument branch this makes room for.

**Verify** — autocomplete tests: `/export ./sr` and `/Users/dav` produce no path completions while `./sr` alone still does; a tab-containing `/mo\tdel` stays in the command-name branch.


## ~~TUI-078~~ — a prompt template's `argument-hint` is parsed, then dropped at the catalog seam — **CLOSED 2026-08-17**

> **CLOSED 2026-08-17 (`0b7c4f4` + `bae24f5`, the `CMDHINT_01` change), verified line by line at HEAD `4fb5e40`.**
> The producer half landed at `crates/cyrup-session-svc/src/session.rs:2635-2639` — `if let Some(hint) = t.argument_hint.as_deref()`
> then `obj.insert("argumentHint".into(), …)`, i.e. pi's spread-if-truthy, with the key ABSENT rather than `null` when there is no
> hint; its own rationale at `:2628-2634` records why the RPC catalog was the right seam ("cyrup's TUI builds its registry from THIS
> catalog … so without the key the hint is unreachable in the one mode pi shows it in"), which is exactly the structural point this
> item's Impact paragraph made. The consumer half is `crates/cyrup-tui/src/commands.rs:475-479`, reading the key through pi's
> empty-string truthiness filter into `SlashCommand::argument_hint`.
>
> **Verify is satisfied twice over.** `crates/cyrup-tui/src/tests/commands.rs:248` proves a catalog row's `argumentHint` becomes
> `SlashCommand::argument_hint`; `crates/cyrup-session-svc/src/tests/cmdhint01_argument_hint.rs:58`/`:82`/`:99` prove the JSON key is
> emitted for a template that declares a hint, omitted for one that does not, and never written for a skill row.
>
> **ONE DELIBERATE DEVIATION FROM THIS ITEM'S OWN FIX, which is why the closure is complete-as-corrected rather than partial:** the
> Fix said "add `argumentHint` to the catalog row for prompt **(and extension)** sources". The extension half was declined, with the
> upstream reason written into the code that declines it (`commands.rs:470-474`): pi's `interactive-mode.ts:691-698` forwards a
> COMPLETER to an extension command, not a hint, and `cyrup-ext/src/registry.rs:94-98` has no such field — so only `source:"prompt"`
> rows can produce one, which IS upstream's behaviour. Porting the extension half would have been a divergence, not a fix.
>
> **The one residual is the half of the Fix nobody wrote.** It instructed: "note in the RPC area that the field is a cyrup addition to
> `get_commands` so it is not read as parity drift there." `grep -n 'argumentHint\|CMDHINT' docs/gap-analysis/08-cyrup-session-svc-and-modes.md`
> returns **zero hits**. That routing is carried by **`TUI-095`**, which also owns the user-visible feature the same commits shipped.

**Kind** parity-bug · **Severity** low · **Effort** M · **Confidence** confirmed

**cyrup** — the TUI gets prompt templates through `slash_command_catalog` (`crates/cyrup-session-svc/src/session.rs:2517-2523`), which never emits an `argumentHint` key, and `dynamic_commands_from_catalog_gated` (`crates/cyrup-tui/src/commands.rs:356`) hardcodes `argument_hint: None`. The data **exists and is parsed** — `crates/cyrup-resources/src/prompt.rs:35`, `:72` `argument_hint`, asserted at `tests/resources.rs:342` as `Some("<pr> [focus]")` — it is dropped at the seam between them.

**upstream** — `pi/…/interactive-mode.ts:596` @v0.83.0 — `...(cmd.argumentHint && { argumentHint: cmd.argumentHint })` on `templateCommands`, sourced from `prompt-templates.ts:125` (`frontmatter["argument-hint"]`).

**Impact** — a user's `/review-pr` template shows a bare description in the `/` menu where pi shows `<pr> [focus] — …`. The user cannot see what the template expects without opening the file. Root cause is structural, and worth stating because it decides the fix: cyrup feeds the interactive menu from the **RPC catalog** (pi's RPC shape carries no `argumentHint` either), whereas pi builds it from `session.promptTemplates` directly.

**Fix** — add `argumentHint` to the catalog row for prompt (and extension) sources and read it in `dynamic_commands_from_catalog_gated`. Adding a field to the RPC payload is the smaller deviation than giving the TUI a second data path; note in the RPC area that the field is a cyrup addition to `get_commands` so it is not read as parity drift there.

**Verify** — a catalog→registry test asserting a template with `argument-hint: "<pr> [focus]"` reaches `SlashCommand::argument_hint`, plus the rendered `/` row.


## TUI-079 — `/export` and `/import` take the whole remainder as the path; pi parses one quote-aware token

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `CommandRegistry::dispatch`'s whole trimmed remainder is passed straight through (`crates/cyrup-tui/src/commands.rs:248-249` → `app/submit.rs:53-56`). No port of pi's helper exists anywhere: `grep -rn 'path_command_argument' crates/` is empty.

**upstream** — `pi/…/interactive-mode.ts:5449-5476` @v0.83.0 `getPathCommandArgument`: strips a leading `"`/`'` to its matching close; otherwise truncates at the first `/\s/`; returns `undefined` on an unterminated quote. Called at `:5435` and `:5480`.

**Impact** — `/export "my session.html"` writes a file whose name literally includes the quote characters; `/export a.html junk` writes to the path `a.html junk`; an unterminated quote that pi **rejects** is accepted as a path. Quoting a path with spaces is the obvious thing a user tries, and it silently produces a wrongly-named file rather than an error.

**Fix** — port `getPathCommandArgument` verbatim into `commands.rs` (it is a pure string function) and apply it in the `/export` and `/import` arms only — it is not a general dispatch rule, and `/name`, `/compact`, `/model`, `/login` take their remainder whole upstream.

**Verify** — unit tests over the helper for all four cases (`"a b.html"`, `a.html junk`, `'a b'`, unterminated `"a b`), plus an app test that `/export "my session.html"` writes exactly that filename.


## TUI-080 — `/name` with no argument is a getter upstream; cyrup always prints usage, and never reports the stored name

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/app/submit.rs:88-95` unconditionally prints `usage: /name <session name>` and never reads the current name, so the only way to see the session's display name is `/session`. The normalization warning is also absent: `C::SetName` (`app/execute_misc.rs:245-263`) prints `session name → {name}` echoing the **input**, not what was actually stored, so a normalized name is reported back wrongly.

**upstream** — `pi/…/interactive-mode.ts:5632-5644` @v0.83.0 `handleNameCommand`: empty arg + a name set ⇒ appends `Session name: ${currentName}` to the chat container; only when NO name is set does it warn `Usage: /name <name>`. It also warns when the name is normalized (`:5648-5650`).

**Impact** — a user who types `/name` to check the session's name is told they used the command wrong. Worse, after `/name My Session!` the transcript claims the name is `My Session!` even when the store normalized it, so the value the user believes is set is not the value `/resume` will show.

**Fix** — port the three branches: empty arg + name set → push `Session name: {current}`; empty arg + no name → the usage warning; non-empty → set, then echo **the stored name** and warn when it differs from the input. The getter needs the current name on the session handle, which `/session` already reads.

**Verify** — app tests for all three branches, including a name that normalizes, asserting the echoed string is the stored one.


## ~~TUI-081~~ — ~~`/import` replaces the live session with no confirmation~~ **CLOSED 2026-09-04**

> ## CLOSED 2026-09-04 — `84b205a1` — `crates/cyrup-tui/src/{selector/mod.rs,app/state.rs,app/execute_session.rs,app/execute.rs,app/selectors.rs}`
>
> **What landed.** The `/import <path>` arm (`crates/cyrup-tui/src/app/execute_session.rs:331`) no longer
> calls `import_from_jsonl`; it calls `App::open_import_confirm` (`:575`), which parks the typed path on
> `AppState::pending_import` (`app/state.rs:283`, `PendingImport` `:554`) and opens
> `SelectorKind::ImportConfirm` (`selector/mod.rs:367`, title `Import session` `:452`) — a first-party
> Yes/No `ListSelector::prompt` whose title is pi's `${title}\n${message}` join, `Replace current session
> with {path}?`, with `Yes` highlighted and the `ExtensionSelectorComponent` chrome (`draws_hint_row`,
> `insets_rows`, `envelope_spacers`) the extension `ui.confirm` uses. It is the port of
> `handleImportCommand`'s `await this.showExtensionConfirm("Import session", `Replace current session with
> ${inputPath}?`)` (`interactive-mode.ts:6069` @v0.84.4) via `showExtensionConfirm` = `showExtensionSelector(…,
> ["Yes","No"])`, `result === "Yes"` (`:2557-2565`). The answer is an ordinary
> `ConfirmSelection { kind: ImportConfirm }` (routed at `app/execute.rs:738`, handled in
> `execute_session_switch` at `app/execute_session.rs:373`): the parked path is `take()`n either way; anything
> but `CONFIRM_YES` (`app/state.rs:563`) pushes pi's `Import cancelled` (`:6071`); `Yes` runs
> `App::dispatch_import` (`:610`) — the former arm body, now captioned with pi's `Session imported from:
> {path}` (`:6082`) and pi's second `Import cancelled` for a runtime `cancelled` (an extension veto, `:6079`).
> Escape on the prompt is `App::cancel_pending_import` (`:597`) from the selector-cancel arm
> (`app/selectors.rs:522`) — pi's Esc resolves `undefined`, read as a decline (`:2564`). The `cancelled` arm
> this row called unreachable is now the veto path behind a user confirmation, which is pi's shape. Same
> strings and order at v0.83.0 (`:5485-5493`).
>
> **Tests** (`crates/cyrup-tui/src/tests/import_confirm.rs`, a faux-provider `AgentSessionRuntime` driven
> through the editor → `handle_input` → `execute_command` path; all four RED at HEAD — the variant did not
> exist, and `/import typo.jsonl` produced `import error: …` with no selector — GREEN after):
> `import_opens_pis_confirm_prompt_before_touching_the_session` (`:112` — prompt open, pi's title and body
> on screen, generation still 0, no `import error`), `declining_the_prompt_cancels_the_import` (`:151` —
> Down+Enter on `No`: `Import cancelled`, no swap, same session), `escaping_the_prompt_cancels_the_import`
> (`:180` — Esc: same, plus a later stray `ConfirmSelection{ImportConfirm,"yes"}` imports nothing because the
> path died with the decline), `confirming_the_prompt_imports_and_swaps` (`:222` — Enter on `Yes`: generation
> 0→1, the install is the copy in the sessions dir, `Session imported from: {path}` surfaces on re-bind).
> Full crate: 1372 passed, 1 skipped; clippy `-D warnings`, rustdoc `-D warnings`, `cargo check -p cyrup` clean.
>
> **Design** (DESIGN-GUIDANCE applied, recorded in the commit body). `Option<PendingImport>` on `AppState`,
> taken on the answer, mirrors `pending_tree_nav`: the invariant is "the path imported is the path the user
> was asked about, asked exactly once", and `take()` on both answers makes a stale path un-importable. Rejected:
> a typestate (the transition is an external key event on a `Box<dyn Selector>`), carrying the path in the
> selector row value (a user path on the `Confirm(String)` wire), and reusing the extension `pending_ui_reply`
> one-shot (would make `/import` deny a guest dialog arriving while the prompt is open, and vice versa).
>
> **Residuals — low, not this row.** (1) pi's v0.84.4 `MissingSessionCwdError` re-prompt
> (`interactive-mode.ts:6084-6095`: `promptForMissingSessionCwd` → a second confirm offering
> `issue.fallbackCwd`, then `importFromJsonl(inputPath, selectedCwd)`) is not ported — cyrup's
> `import_from_jsonl` returns `SessionServiceError::MissingSessionCwd` and the arm shows `import error: …`;
> the runtime already accepts a `cwd_override`, so the port is a second `ImportConfirm`-style prompt.
> (2) The failure wording stays `import error: {e}` where pi shows `Failed to import session: {message}`
> through `showError` (`:6098-6101`); string + channel only.
>
> ---
>
> **Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/app/execute_session.rs:284-294` calls `rt.import_from_jsonl(path, None)` immediately. Its own `Ok(r) if r.cancelled` arm handles a cancellation the TUI never offers a way to trigger.

**upstream** — `pi/…/interactive-mode.ts:5485` @v0.83.0 — `await this.showExtensionConfirm("Import session", \`Replace current session with ${inputPath}?\`)`, and `showStatus("Import cancelled")` on decline.

**Impact** — a mistyped `/import` destroys the live session with no prompt. The user's in-flight conversation is replaced by the imported one; there is no undo surface. Rated medium rather than low because the loss is of user data and the guard exists upstream precisely to prevent it.

**Fix** — call the existing confirm dialog (the same one extension confirms use) before `import_from_jsonl`, with pi's title and body strings, and push `Import cancelled` on decline — which finally makes the dead `cancelled` arm reachable.

**Verify** — app test: `/import p.jsonl` opens a confirm dialog; declining leaves the session untouched and pushes `Import cancelled`; accepting imports.


## TUI-082 — bare `/export` writes no file: it dumps the raw HTML into the transcript

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — the no-path branch (`crates/cyrup-tui/src/app/execute_session.rs:64-104`) calls `push_block("Session (HTML)", html)`.

**upstream** — `pi/…/interactive-mode.ts:5435-5446` @v0.83.0 (`exportToHtml(outputPath)` with `outputPath === undefined`) → `pi/packages/coding-agent/src/core/agent-session.ts:3215-3231`, documented `@param outputPath Optional output path (defaults to session directory)`; the handler then shows `Session exported to: ${filePath}`.

**Impact** — bare `/export`, the most likely invocation, produces **no artifact** and floods scrollback with HTML source. The user is given no path, so they cannot find a file that was never written. Distinct from `DRIFT-041`, which is about the quality of the HTML that gets rendered, not about whether a file is written — do not fold them.

**Fix** — default the output path to the session directory the way `agent-session.ts:3215-3231` does, write the file, and push `Session exported to: {path}`. The `push_block` fallback should go entirely; there is no upstream branch it corresponds to.

**Verify** — app test: bare `/export` writes a `.html` under the session directory and the status names that path; the transcript contains no HTML source.


## TUI-083 — `/quit`'s description is a hardcoded literal where pi templates the app name

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-tui/src/commands.rs:82` hardcodes `"Quit cyrup"`; `grep -rn 'APP_NAME|app_name' crates/cyrup-config/src crates/cyrup-core/src` finds no counterpart constant.

**upstream** — `pi/packages/coding-agent/src/core/slash-commands.ts:41` @v0.83.0 + `config.ts:489` — the description is built as `Quit ${APP_NAME}` where `APP_NAME = piConfigName || "pi"`, i.e. a user running under a renamed config sees their own app name.

**Impact** — low today, because cyrup has no config-name override. The row exists because the **mechanism** is unported, not the string: the moment a config-name override is added, this line is wrong and nothing will point at it. TUI-025 already fixed three sibling literals in this table by re-reading pi's strings; it did not notice that this one is not a string upstream at all.

**Fix** — introduce the `APP_NAME` constant (config-name override falling back to `"cyrup"`) where cyrup's config name would live, and build the description from it. If the override is not wanted, say so at the call site and cite `config.ts:489`, so the difference is a decision rather than an oversight.

**Verify** — a registry test asserting the description tracks the constant rather than a literal.


## TUI-084 — the argument-less usage strings for `/import` and `/name` diverge in wording and in severity channel

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** (as filed) — `/import` with no arg pushed `usage: /import <path>` and `/name` with no arg pushed `usage: /name <session name>`, both through `push_status`. **Neither string exists in-tree at HEAD**: `/import` dispatches at `app/submit.rs:85` and its no-argument branch pushes pi's `Usage: /import <path.jsonl>` through `push_error` (`app/execute_session.rs:297-303`), while `/name` with no argument is a getter whose empty branch pushes `Usage: /name <name>` through `push_warning` (`app/submit.rs:92-95` → `app/execute_misc.rs:268-272`) — this is the row the header note already flags as asserting things false at HEAD, and it needs a substantive re-audit rather than a citation repair.

**upstream** — `pi/…/interactive-mode.ts:5482` and `:5639` @v0.83.0 — `/import`: `Usage: /import <path.jsonl>` via `showError`; `/name` with no arg **and no name set**: `Usage: /name <name>` via `showWarning`.

**Impact** — three string divergences (casing, the dropped `.jsonl` constraint, `<session name>` vs `<name>`) plus a **severity-channel** divergence: upstream's error/warning become a neutral status, so the line is not coloured or prefixed as a problem. That channel question is the same one `TUI-062` records for `showWarning`'s `Warning: ` prefix — this item is a caller-side instance of it, and the two should be settled together.

**Fix** — match pi's strings verbatim and route them through the error/warning entry kinds rather than `push_status`. `/name`'s branch also depends on TUI-080: upstream only reaches the usage warning when **no name is set**.

**Verify** — app tests asserting the exact strings and the entry kind for both commands.


## TUI-085 — a dynamic command with no `sourceInfo` is tagged `[t]`; pi leaves the description unprefixed

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `dynamic_commands_from_catalog_gated` defaults a missing `sourceInfo.scope` to `"temporary"` → tag `t` and **always** prefixes (`crates/cyrup-tui/src/commands.rs:344-355`), so a `sourceInfo`-less row renders `[t] desc`.

**upstream** — `pi/…/interactive-mode.ts:497-528` @v0.83.0 — `getAutocompleteSourceTag` returns `undefined` for a missing `sourceInfo` (`:498-500`), and `prefixAutocompleteDescription` then returns the description **unprefixed** (`:523-526`).

**Impact** — unreachable through cyrup's own catalog, which always emits `sourceInfo`. Recorded because the divergence lives in a `pub fn` any future caller can hit, and because a wrong provenance tag is worse than none: `[t]` claims the command came from a temporary scope it may not have.

**Fix** — make the scope `Option`, return `None` from the tag helper when `sourceInfo` is absent, and skip the prefix in that case.

**Verify** — a unit test over `dynamic_commands_from_catalog_gated` with a row carrying no `sourceInfo`, asserting the description is unprefixed.


## TUI-086 — two slash-command structures have no upstream counterpart: `CommandSource::Builtin` and a public `autocomplete_source_tag`

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** —

1. **`CommandSource::Builtin`** (`crates/cyrup-tui/src/commands.rs:21`). pi's `SlashCommandSource` is exactly `"extension" | "prompt" | "skill"` (`core/slash-commands.ts:4`), and `BuiltinSlashCommand` (`:13-17`) has **no `source` field at all** — builtins are a different type. cyrup unified the two into one struct, which forces a fourth variant.
2. **`autocomplete_source_tag` is exported from the crate** (`commands.rs:263`). pi's `getAutocompleteSourceTag` is a private method on `InteractiveMode` (`interactive-mode.ts:497`).

**Impact** — (1) means `CommandSource` is **not** the port of `SlashCommandSource`: anything that serializes it will emit a value pi has no word for, which matters the moment a command list crosses the RPC or WIT boundary. The unification is defensible and internally consistent; the hazard is treating the enum as pi's. (2) is harmless in itself but widens the public surface past pi's and invites callers upstream has no counterpart for — the same shape as `ToolInfo::source`, a field pi has no word for.

**Fix** — no behavioural change proposed. Document on `CommandSource` that `Builtin` is cyrup-only and must never be serialized as a `SlashCommandSource`; make `autocomplete_source_tag` `pub(crate)` unless a cross-crate caller exists (there is none at HEAD).

**Verify** — a compile-level check that no serializer reaches `CommandSource`, and `cargo check -p cyrup-tui --all-targets` after the visibility narrowing.


## TUI-087 — `commands.rs`'s upstream citations do not resolve at v0.83.0, and one names a symbol that does not exist

**Kind** stale-port · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — the file mixes v0.84.1-era offsets into a v0.83.0 port, and one citation is to a function that was never written. All verifiable with `git -C pi show v0.83.0:<path> | grep -n`:

| cite | at `commands.rs` | actually, at v0.83.0 |
|---|---|---|
| `core/slash-commands.ts:18-41` | `:3-5` | the table is `:19-42`, path `packages/coding-agent/src/core/slash-commands.ts` |
| `interactive-mode.ts:2549-2734` (`setupEditorSubmitHandler`) | `:3-5` | `:2660-2846` |
| `interactive-mode.ts:2657-2671` (hidden commands) | `:86` | `:2769-2783` |
| `interactive-mode.ts:536-559` (`getAutocompleteSourceTag`) | `:256` | `:497-520` |
| `interactive-mode.ts:561-567` (`prefixAutocompleteDescription`) | `:279` | `:522-528` |
| `rpc-mode.ts:676-690` | `:311` | path is `modes/rpc/rpc-mode.ts` |
| "only `/model` in Pi, `:498-528`" | `:43` | **wrong on both halves** — pi has TWO commands with `getArgumentCompletions` at v0.83.0 (`:555` model, `:582` login, and the code six lines below the comment already gives `/login` an `arg_cmd`), and `:498-528` lands on `getAutocompleteSourceTag`/`prefixAutocompleteDescription`, not on either completer |
| `AgentSession::expand_slash_command` | `:317` | **no such function.** `grep -rn 'expand_slash_command' crates/ --include='*.rs'` returns only this comment; the real functions are `expand_input_text` and `expand_skill_command` (`crates/cyrup-session-svc/src/session.rs:1208`, `:1216`) |

The `:610-622` skill-gate cite is the one that does resolve.

**Impact** — a fabricated symbol reference inside a doc comment that is otherwise making a correct behavioural claim is the same class as the fabricated pi citation on `working-start`/`working-stop`: the claim reads as verified and cannot be checked, and the next reader either trusts it or re-derives the whole seam. The stale offsets are the milder half — they cost a reader one `grep` each — but they are how a v0.84.1 reading silently becomes the baseline.

**Fix** — retarget all eight to v0.83.0, and replace `expand_slash_command` with the two real function names. Pure comment edit, no behaviour.

**Verify** — every cite in `commands.rs` resolves under `git -C pi show v0.83.0:<path>`; ideally a script that extracts `path:line` pairs from the crate's comments and checks them, since this is the third area to hit the same class.


## ~~TUI-089~~ — ~~medium~~ **CLOSED 2026-09-04 — REFUTED** — Models added through `models.json` are appended to the picker unsorted

**Kind** ~~port-bug~~ *mis-filed* · **Severity** ~~medium~~ · **Effort** S · **Confidence** confirmed (both sides read at HEAD `d685eff1` and pi `v0.84.4`; assembly probed empirically)

**Verdict** — REFUTED. cyrup orders the catalog at the same two points pi does and by the same rules: `models.json` composition pushes a new id at the END of its provider's block (and replaces a matching id in place), and the `/model` picker sorts by provider only, stably. A user-declared model therefore lands at the end of its own provider's block, adjacent to that block, on both sides. No behaviour was changed; a guard test was added.

**pi v0.84.4** — assembly: `packages/coding-agent/src/core/provider-composer.ts:168-206` `applyModelsJson` (`:198-204` loop: `:199` `findIndex` by id → `:202` replace in place, else `:203` `models.push(model)`); provider order: `core/model-runtime.ts:236-243` `providerIds()` insertion-ordered `Set`; overlay-before-compose: `:183-189` `withRemoteCatalog(provider, …)` on every built-in (except `radius`) before `rebuildProviders`. Picker: `modes/interactive/components/model-selector.ts:225-239` `sortModels` — current, default, then `a.provider.localeCompare(b.provider)` only; the snapshot it sorts is `modelRuntime.getAvailableSnapshot()` (`:156`), i.e. composition order. `--list-models`: `cli/list-models.ts:54-58`, provider then id.

**cyrup HEAD** — `crates/cyrup-config/src/model/compose.rs` `ModelFile::compose` `:30-68` and `apply_models_json` `:128-199` (step 2 `:173-185`); `crates/cyrup-session-svc/src/session/model.rs` `full_model_registry` `:200-243` (overlay in the base at `:214-231`, compose at `:242`), `available_model_catalog` `:252-257`; `crates/cyrup-tui/src/app/event_extract.rs` `model_entries` `:296-318`; `crates/cyrup-tui/src/model_selector.rs` `new` `:177-181`, `sort_models` `:158-174`; `crates/cyrup/src/actions.rs` `list_models` `:132`. Filing-commit `7e2e60cc` had the same three sites (`cyrup-config/src/model.rs:2242`, `:2400`; `cyrup-session-svc/src/session.rs:3151`; `cyrup-tui/src/model_selector.rs:97-101`), so the observation was never a cyrup divergence.

**Probe** — scratch crate outside the workspace (not committed) over the real built-in catalog (1 073 models) with a `models.json` adding `moonshotai/Kimi-K9-probe` to `together` and a wholly-new `mycorp` provider: probe at index 853 directly after the last built-in `together` row (852), `together` block contiguous (22 rows), `mycorp-large` at 1 074. **Scope check:** existing-provider entries go to the end of that block; new-provider blocks go after every built-in — both pi's behaviour.

**Why it looked wrong live** — grouped by provider, `together` is the last configured block unless `vercel-ai-gateway`/`xai` are configured, so its final row is the bottom of the list. Pi shows the same. Neither side sorts by id inside a block, so the custom model is not beside its `moonshotai/` namesakes on either side — parity, not a defect (low residual, not this row).

**Test** — `crates/cyrup-tui/src/model_selector.rs::tests::models_json_appended_model_stays_inside_its_provider_block` (commit `d685eff1`). Passes at HEAD; mutation-checked: with the provider tier of both sort sites replaced by `Ordering::Equal` it fails with the custom model detached from its block.

**Checks** — `cargo fmt --all -- --check` clean; `cargo nextest run -p cyrup-tui` 1 348 passed / 1 skipped; `RUSTDOCFLAGS='-D warnings' cargo doc -p cyrup-tui --no-deps` clean; `cargo clippy -p cyrup-tui --all-targets -- -D warnings` fails on a pre-existing `redundant_closure` at `app/input_reader.rs:443` from the workspace rustfmt commit `3e69ea2a` (outside this item), clean with that one lint allowed.

## ~~TUI-091~~ — ~~high~~ **CLOSED 2026-09-04 — duplicate of `TUI-090`, settled in a real pty** — Reasoning blocks never render — ~~**DOES NOT REPRODUCE HEADLESSLY AT HEAD; RELOCATED OUT OF THE RENDER PATH 2026-08-19**~~

**Kind** port-bug · **Severity** ~~high~~ closed · **Effort** M · **Confidence** confirmed (the report), **duplicate of `TUI-090`** (the mechanism) · **filed 2026-08-15 (live use), CLOSED 2026-09-04 (live pty)**

> **CLOSED 2026-09-04 — the row's own Verify clause was executed and took its first branch.** The
> clause reads: *the row closes when a live `together`/Kimi-K3 turn at `defaultThinkingLevel: high`
> in a real terminal either shows the reasoning block (close as a duplicate of `TUI-090`) or
> produces one of the three instrumented signals.* It showed the block — in seven variants, live and
> committed — so the three signals were never needed.
>
> **Instrument.** tmux 3.4 (a real terminal emulator over a real pty) at 120×40, the debug
> `target/debug/cyrup` at HEAD `a4805955`, an isolated `CYRUP_HOME`, no network. A local python3 SSE
> server on `127.0.0.1:18931` speaking `openai-completions` with reasoning deltas, reached through a
> `models.json` `together` `baseUrl` overlay, so the launch was the owner's own —
> `--provider together --model moonshotai/Kimi-K3 --thinking high`, footer
> `together/moonshotai/Kimi-K3 • high`. The request body the server logged carried
> `reasoning: {enabled: true}` (`ThinkingFormat::Together`, `crates/cyrup-provider/src/providers/together.rs:286-295`).
> A headless `--mode json -p "say hello" < /dev/null` control through the same overlay exited 0 with
> 28 `thinking_delta` updates and a `message_end` whose assistant content types were
> `['thinking','text']`, `thinkingSignature: reasoning_content` — the exact shape
> `assistant_message_from_event` deserializes.
>
> **Observation, pane captured at T+1s (mid-stream) and T+2s (committed), then the full scrollback
> via `capture-pane -p -S -`.** T+1s: ` say hello` / ` THINKHEAD The user wants a greeting. Let me
> reason carefully about the best short reply. I should keep it` / ` ⠹ Working...` / footer
> `0.0%/1.0M (auto)   together/moonshotai/Kimi-K3 • high`. T+2s: the same first two lines, then
> ` friendly. Considering tone, length and clarity. THINKTAIL` / ` FINALANSWER Hello there, this is the
> final answer text.` / footer `↑10 ↓50 $0.001 …`. Scrollback: line 70 ` say hello`, 80-81
> `THINKHEAD…THINKTAIL`, 83 `FINALANSWER` — reasoning committed ABOVE the answer, once. Six more
> variants, all the same result: Together's alternative `reasoning` delta field; LONG (90 reasoning
> lines, taller than the screen — committed block `THINKLINE001` at scrollback line 148 through
> `THINKLINE090` at 237, `FINALANSWER` at 239, **missing `THINKLINE`s = []**); TOOL (scrollback 73
> ` THINKHEAD1 … THINKTAIL1`, 82 ` $ echo TOOLCANARY`, 84 ` TOOLCANARY`, 95 ` THINKHEAD2 …
> THINKTAIL2`, 97 ` FINALANSWER2 …` — both blocks, and the replay request carried
> `reasoning_content` on the assistant message); `--tui-mode fullscreen`; `hideThinkingBlock: true`
> (pane ` Thinking...` / ` FINALANSWER …`, footer `… • high (hidden)` — pi's `hiddenThinkingLabel`,
> `assistant-message.ts:139-142` @v0.84.4, cyrup `transcript/stream.rs:105-139`); and `--continue`
> replay of a saved session, re-rendered from the JSONL. All six `tui_*.stderr` files are 0 bytes;
> the four saved session JSONLs carry 1/2/1/1 non-empty `type: thinking` blocks — the owner's
> evidence shape exactly, and here it is on screen.
>
> **Why a duplicate of `TUI-090` and not a new mechanism.** `TUI-091` was filed at `7e2e60cc`,
> 2026-08-15 16:26 −0700. `TUI-090` was filed at `61804a16` (15:35) and FIXED at `45da9d3b` (19:50)
> the same day. The owner's observation predates the fix, and `45da9d3b`'s own commit body describes
> the asymmetry this row spent three passes hunting — mid-turn commits flushed through
> `insert_before` into native scrollback invisibly while the viewport was stale-full, then erased at
> `AgentEnd` — and says *"TUI-091 noted for re-check as a likely duplicate symptom"*. At HEAD the
> symptom does not reproduce in a real pty under any of seven shapes. The `MessageEnd` guard at
> `app/events_fold.rs:121-125` this row relocated to is not implicated: the committed reasoning is
> the authoritative `Content::Thinking` path through `finalize_assistant_message`
> (`app/events.rs:297-305`), which only runs when that guard passes.
>
> **Falsification** — a live `together`/Kimi-K3 turn at HEAD, real terminal, `hideThinkingBlock`
> unset, showing the answer and no reasoning block. Not `TestBackend`, not the capture harness.
>
> **Residuals, none of them this row.** (a) The strongest possible duplicate proof — a pre-`TUI-090`
> binary (`git archive` of `45da9d3b^` = `a6ea9ddd`) showing the reasoning hidden — was building
> (310/727 crates) when the pass ended; the closure rests on the HEAD observation, the timeline and
> the fix commit's own mechanism, which together satisfy the Verify clause as written. (b) Untested:
> the real Together wire for Kimi-K3 (the owner's JSONL already proves decoding produced 40 thinking
> blocks, so the wire is not where the symptom lived) and macOS terminals vs tmux/xterm-256color. (c)
> Observed, ownerless lead: in the LONG run the tail of the live reasoning appears once as
> scrolled-off live frames and again inside the committed block (121 `THINKLINE` rows for 90 lines)
> — inline-viewport-taller-than-screen duplication, cosmetic. The 2026-08-19 "lost interior rows"
> byproduct did NOT reproduce (0 of 90 rows lost), confirming it as a harness artifact. (d)
> Observed, ownerless lead: `--no-extensions --no-skills` still lists `[Extensions] cyrup-flux` and
> the flux skills/prompts — `crates/cyrup-flux` is compiled in, so the flags do not govern it. (e)
> Instrument caveats for the next pass: `pkill -f <script>` matches the invoking shell's own command
> line (the `REPRO-LOG.md` warning, hit again); `-p` mode blocks on an inherited open non-TTY stdin
> (pi-faithful, `main.ts:75-93` @v0.84.4) — use `< /dev/null`.
>
> Everything below is the 2026-08-19 analysis, unchanged; read it as the record of where the
> defect was NOT.

> **The row is NOT refuted — the owner saw it, and the session JSONL proves the model was thinking and
> cyrup was capturing it (40 non-empty `"type":"thinking"` content blocks). What is refuted is the
> last remaining hypothesis about WHERE it happens.** The 2026-08-19 re-audit refuted "the accumulated
> `thinking` buffer is never in the painted region" by reading; this pass refuted it by RUNNING, in
> two harnesses, one of which is the pty-equivalent. The render path is now proven by execution, and
> the defect moves to the assembled live app or the terminal. **Nothing below is a reason to re-trace
> the six layers the row already pins.**

**Reproduction attempted, and it does not reproduce.** Two throwaway probes, both deleted afterwards — `git status` clean, `crates/cyrup-tui/src/tests/mod.rs` and `crates/cyrup-tui/src/tests/inline_stacking.rs` restored byte for byte, all **1 270** `cyrup-tui` lib tests green.

1. **Assembled `TestBackend`, 100x30**, driven with the full real event sequence — `AgentStart` → `MessageStart` → 40 `MessageUpdate{ThinkingDelta}` → 6 `TextDelta` → `MessageEnd` carrying both `Content::Thinking` and `Content::Text` → `AgentEnd` — drawing once per event. Reasoning visible in the live viewport from **delta #0**; committed reasoning and answer both in scrollback afterwards, in the right order.
2. **The `CaptureBackend` harness** (`crates/cyrup-tui/src/tests/inline_stacking.rs:61-152`, VT screen model at `:159-340`, `replay` at `:343-347`) — a REAL `CrosstermBackend` writing into a shared byte buffer, replayed through the VT model, which is what a user actually sees rather than what a fixed grid reports. Same sequence at 80x24. `THINKHEAD`/`THINKTAIL` sentinels on screen mid-stream, and still present in the reconstructed scrollback + grid after commit alongside `FINALANSWER`.

**The row's four open questions, answered by measurement.**

**(1) Does the accumulated `thinking` buffer reach the painted line set? YES.** Traced end to end and confirmed by execution: `push_thinking_delta` (`crates/cyrup-tui/src/transcript.rs:705`) → `TranscriptView::thinking` → the `thinking_visible` gate (`:1272`) → `thinking_lines` + `pad_lines` extended into the returned `Vec<Line>` (`:1282-1298`, including pi's `hasVisibleContentAfter` spacer at `:1293-1297`) → `cached_render` (`:1228`) → `Component::render` clones `cache.lines` into the `Paragraph` (`:3379`) → painted into `msg_area` at `crates/cyrup-tui/src/app/render.rs:49`. Layout sizes that region from the same cache (`app/layout.rs:165` → `content_height` → `cached_render`).

**(2) Is the F2 render cache invalidated for thinking deltas? YES.** `push_thinking_delta` bumps `render_generation` as its **first** statement (`transcript.rs:706`), and the cache key is `(generation, width, theme.generation)`. The probe measured the sentinel entering the live region at **delta #0** and the block growing on every subsequent delta — `transcript.thinking()` reached 2 158 bytes and the viewport grew to 30 rows. This is not a first-frame-then-frozen cache, and `TUI-092` F2 could neither have caused nor cured it.

**(3) Is the commit path intact, and does anything overwrite it? INTACT, AND NO.** `finalize_assistant_message` (`app/events.rs:178`) commits `thinking_text(&message.content)` **before** the answer; `commit_thinking` (`transcript.rs:724`) pushes `Entry::Thinking`; `entry_lines` renders it (`transcript.rs:2940`); `flush_committed` (`app/draw.rs:128-183`) emits it through `insert_before` (`:178`). The probe saw **exactly one** committed reasoning block, ordered above the answer, present in the reconstructed pty scrollback after the turn — no double-commit from `AgentEnd`'s `commit_thinking(None)` (`app/events_fold.rs:51`), because finalize had already cleared the buffer. `hide_thinking_block` was `false` throughout and is `false` by default (`crates/cyrup-config/src/settings.rs:580`).

**(4) Reproduction:** the two probes above. Neither reproduces the report.

**Where the bug now lives.** Not `transcript.rs`, not the F2 cache, not the commit/flush wiring — the assembled live app or the terminal. **The single mechanism found that reproduces the reported ASYMMETRY (answers render, reasoning does not) is the `MessageEnd` guard at `crates/cyrup-tui/src/app/events_fold.rs:121-125`:**

```rust
if self.state.streaming_assistant
    && let Some(message) = assistant_message_from_event(&ev)
{
    self.finalize_assistant_message(&message);
}
```

If either condition is false live, the authoritative `Content::Thinking` blocks are never committed, and the `AgentEnd` fallback (`events_fold.rs:51`, `commit_thinking(None)`) can only commit `self.thinking` — the **streamed** buffer. **The answer has a second source and the reasoning does not**: `commit_assistant(None)` (`:52`) recovers the `TextDelta` buffer, so a turn in which `ThinkingDelta` never arrives AND the guard fails loses the reasoning while keeping the answer. That is the reported shape exactly, and "session JSONL has 40 thinking blocks, screen has none" needs nothing from the render path to explain it.

**Fix** — do not write code yet; instrument, in a real terminal, exactly three things and nothing else. (a) `message_role_from_event` (`crates/cyrup-tui/src/app/event_extract.rs:190-193`) — what role does a live `MessageEnd` project to? (b) `assistant_message_from_event` (`:219-226`) — does it return `Some`? (c) a `ThinkingDelta`-arrival counter at `app/events_fold.rs:497`. **Both projections go through `serde_json::to_value(ev)` (`event_extract.rs:191`, `:220`) and fail SILENTLY to `None` on any shape mismatch** — that is the first failure mode to look for, and it is invisible to every existing test because the tests construct the events the projection expects. One live turn with those three logged decides the row.

**Byproduct observation — NOT this row, deliberately NOT filed.** In the pty replay the commit flush loses interior rows when the inserted block plus the viewport exceeds the screen: 4 of 11 committed rows lost with reasoning, and **2 of 10 lost in a control run with a long answer and no thinking at all**. The control run is what disqualifies it as this row's mechanism — it is generic flush geometry (`app/draw.rs:178` `insert_before` on the same frame `resize_viewport` runs), not a reasoning defect. It could not be reproduced analytically from ratatui's `insert_before_no_scrolling_regions` arithmetic, and it may be an artifact of the harness's deliberately partial VT model: its module doc declares the subset it implements — `CUP H`, `ED J`, `EL K`, LF scroll, SGR and private modes ignored (`inline_stacking.rs:156-158`) — and `?7h/l`, DECAWM auto-wrap, is explicitly among the private modes it discards (`:296`). **It needs a real pty to confirm before it is worth a row**, and it is recorded here so the next pass does not re-derive it from scratch.

**Verify** — the row closes when a live `together`/Kimi-K3 turn at `defaultThinkingLevel: high` in a real terminal either shows the reasoning block (close as a duplicate of `TUI-090`, which was fixed the same day this was filed) or produces one of the three instrumented signals above. **`TestBackend` cannot settle it and neither can the capture harness — both now say the code is correct.**

## Coverage

**Method.** Read-only and static, at cyrup HEAD `04c1ba2` (last code commit; tree clean at the docs-only `a9000b1`) against pi `v0.84.1`, with `v0.83.0` read directly wherever a finding had to be classified `not-ported` versus `upstream-drift`. No cargo, no npm, no execution of cyrup or pi. Every `closed`, `partially-closed` and `overturned` verdict was reached by opening the named Rust function at HEAD **and** the cited TypeScript at the tag; commit messages were treated as hypotheses throughout, and two auditor closures (TUI-020, TUI-S02) were overturned on exactly that basis.

**Read first-hand on the cyrup side.** `crates/cyrup-tui/src/`: `app.rs` (targeted, ~1.5k of ~7,100 lines — **pre-split; that file was replaced by the 33-module `app/` tree at `40821ed` and the paths in this section are historical**), `transcript.rs`, `keymap.rs`, `commands.rs`, `autocomplete.rs`, `chrome.rs`, `image.rs`, `status.rs`, `status_indicator.rs`, `tree_selector.rs`, `markdown.rs`, `footer_data.rs`, `terminal_query.rs`, `terminal_progress.rs`, `terminal_title.rs`, `keyboard_protocol.rs`, `tmux.rs`, `drain.rs`, `panic_hook.rs`, `resume_hint.rs`, `startup.rs`, `session_search.rs`, `editor.rs` (keybinding ids and prompt history only), `theme.rs` (targeted). Plus `crates/cyrup/src/{main.rs, update_check.rs, startup_ui.rs}`; `crates/cyrup-session-svc/src/{host_services.rs, services.rs, session.rs}` (including the compaction/streaming lifecycle traced for TUI-031); `crates/cyrup-ext/src/{registry.rs, facade.rs, host/services.rs, host/live.rs}`; `crates/cyrup-config/src/{settings.rs, env.rs}`; the 85-file test inventory plus `image.rs`, `extension_dialog_countdown.rs`, `extension_ui_effects.rs`, `extension_ui_reset_on_swap.rs`, `turn_interleaving.rs` read directly.

**Read first-hand on the upstream side.** At v0.84.1: `packages/coding-agent/src/modes/interactive/interactive-mode.ts` (escape chain, `createExtensionUIContext`, `setExtensionWidget`/`Footer`/`Header`, `setupAutocompleteProvider`, `setToolsExpanded`, `updatePendingMessagesDisplay`, `queueCompactionMessage`, `maybeSaveImplicitProjectTrustAfterReload`, `handleReloadCommand`, the startup header, `switchTuiMode`, `renderProjectTrustWarningIfNeeded`, `emergencyTerminalExit`), `components/{assistant-message,user-message,tool-execution,footer,status-indicator,countdown-timer,settings-selector,tree-selector,bash-execution,markdown-transform,mermaid}.ts`, `core/{keybindings,slash-commands,extensions/runner,extensions/types}.ts`, `packages/tui/src/{tui,keybindings,terminal,terminal-image,autocomplete}.ts`, `packages/tui/src/components/{editor,image,markdown}.ts`. At v0.83.0 specifically, to classify: `tree-selector.ts`, `keybindings.ts` (both packages), `extensions/types.ts`, `interactive-mode.ts`. Plus the `v0.83.0..v0.84.1` diffstat and log for `packages/tui` and `packages/coding-agent/src/modes/interactive`.

**Version-lag sweep result.** Four drift-relevant clusters were examined. (a) Alt-screen / `tui-mode` / scroll-view / mouse / semantic-prompt navigation — folded into the **TUI-019** re-audit rather than filed separately, since TUI-019 already owns it and only its size changed. (b) LaTeX — **already ported** (`crates/cyrup-tui/src/markdown/latex.rs`, 2242 lines); no item. (c) Mermaid plus the markdown-transformer API — filed as **TUI-034**. (d) The `scrollbarThumb` theme token — **already ported** with pi's optional-with-fallback semantics (`crates/cyrup-tui/src/theme.rs:480-483, :1032-1033`); no item. Two small v0.84.x coding-agent fixes were checked and found already correct in cyrup: `fc4a3d99b` (errors honour `outputPad` — `transcript.rs:2933`) and `e8a17822d` (progress clear sequence — `terminal_progress.rs:47` explicitly tracks the v0.84.1 spelling).

**Surface-driven sweep — the axis used, and the one that had never been run (repair pass).** The critique was right on both counts: this file never used the term, and the specific sweep README blind spot 2 prescribes had not been performed. What the main pass ran was a **version-lag** sweep (the four clusters above) plus item-driven verification — neither of which can see a `packages/tui/src` file that draws nothing and that no item names.

The repair pass ran the missing one. Axis: **enumerate every non-drawing file under `pi/packages/tui/src`, read it at both v0.83.0 and v0.84.1, and trace every exported symbol to its cyrup consumer by ripgrep over `crates/`.** Six files, 863 lines: `stdin-buffer.ts` (434), `editor-component.ts` (74), `terminal-colors.ts` (73), `undo-stack.ts` (28), `word-navigation.ts` (117), `fuzzy.ts` (137). **Zero mentions of any of these six basenames existed anywhere in `docs/gap-analysis` before this pass** — a grep over all fifteen files returned nothing, which is the confirmation of blind spot 2 rather than an inference about it. Only `terminal-colors.ts` differs between the tags (`COLOR_SCHEME_REPORT_PATTERN` widened to `(?:…)+`); the other five are byte-identical at both.

Yield: **nine items, two of them critical** — TUI-042, TUI-043, TUI-044, TUI-045, TUI-046, TUI-047, TUI-048, TUI-049, TUI-050. The two criticals are both silent data loss in the prompt editor on ordinary keystrokes, in code that has shipped and that no pass had ever opened.

**Substrate note, load-bearing for four of the nine.** cyrup routes input through crossterm 0.29.0, whose `Parser::advance` (`crossterm-0.29.0/src/event/source/unix/tty.rs:247-268`) feeds `parse_event` byte-at-a-time and retains a partial buffer — so **most of `stdin-buffer.ts`'s reassembly duty is genuinely covered**, and this was verified in crossterm's own source rather than assumed. A CSI/SS3/mouse sequence split at any byte *after* the introducer is reassembled; bracketed paste spanning many reads comes out as one `Event::Paste` (`parse.rs:198-200` + `parse_csi_bracketed_paste`, routed at `app/input_reader.rs:427` → `app/input.rs:106-108` → `InputEditor::handle_paste`); the old-style `ESC[M`+3-byte mouse completion is handled by `parse_csi_normal_mouse`. Four behaviours are **not** covered and are the items above.

**Confirmed covered by this sweep — do not re-file.**
- **`fuzzy.ts` — both exports ported faithfully and widely consumed.** Every constant was re-derived against upstream: consecutive-run reward `-5·run` (`fuzzy.ts:37` / `fuzzy.rs:52`), gap penalty `+2·gap` (`:42` / `:56`), word-boundary bonus `-10` over `/[\s\-_./:]/` (`:32`,`:48` / `is_boundary_sep` `:19-21`, `:61`), later-match penalty `+0.1·i` (`:52` / `:65`), whole-string-exact `-100` (`:63-65` / `:75-77`), the empty-query-before-length-check ordering (`:17-23` / `:26-31`), and the alphanumeric-swap retry at `+5` with pi's `[a-z]`-then-`[0-9]` precedence (`:75-92` / `:83-117`). `fuzzyFilter` (`:99-137`) → `filter` (`:143-174`): same `/[\s/]+/` tokenisation, all-tokens-must-match with summed scores, empty-query passthrough, stable ascending sort. Nine consumers verified.
- **`terminal-colors.ts` — all four exports ported, including the only inter-tag change in the six.** `parseOscHexChannel` → `terminal_query.rs:188-201`; `parseOsc11BackgroundColor` → `:143-184`; `parseTerminalColorSchemeReport` → `:221-239`, and cyrup **already carries** the v0.84.1 widening of `COLOR_SCHEME_REPORT_PATTERN` (pi `0e633790c`) with both load-bearing properties (last-frame-wins, the all-or-nothing `^…$` anchor) and transcribes pi's own test case at `terminal_query.rs:528-559`. `isOsc11BackgroundColorResponse`'s consumer role is `stray_reply.rs` in full. The remaining live-resync consumer is TUI-004.
- **`undo-stack.ts` — the container is ported.** `UndoStack<S>` → `undo: Vec<Snapshot>` with push/pop/clear/length; `structuredClone` is mechanism-N/A (`Vec<Vec<char>>: Clone` is already deep). pi's fish-style coalescing is ported correctly on both arms (`editor.ts:1092-1097` → `push_undo_for_type` `:739-746`; the non-typing always-push → `push_undo_for` `:723-732`). pi has no redo and neither does cyrup, so `editor.rs:748`'s parity comment is correct. **Only the snapshot PAYLOAD diverges** — TUI-042 and TUI-044.
- **`editor-component.ts` — every interface member has a concrete `InputEditor` counterpart**, so there is no missing *behaviour*, only missing *pluggability*: `getText`/`setText`, `handleInput`, `onSubmit`/`onChange`, `addToHistory`, `insertTextAtCursor`, `getExpandedText`, `setPaddingX`, `setAutocompleteMaxVisible`, `borderColor` all map. The two genuinely absent members are already owned — `setEditorComponent`/`getEditorComponent` is **TUI-030**, `setAutocompleteProvider` is **TUI-029**. Do not re-file either.
- **`utils.ts:826-829 isWhitespaceChar`** — ported with its delta stated at `editor.rs:1655-1660` (JS `\s` vs `char::is_whitespace` differ on U+FEFF and U+0085; both unreachable because `sanitize_paste` `:1834-1836` strips them). `sanitize_paste` was re-read and the claim holds.

**Explicitly N/A, recorded so the absence is not read as an oversight.** `stdin-buffer.ts`'s `EventEmitter` plumbing (`:20`, `:265-268`, `:274`) — cyrup's transport is the `tokio::sync::mpsc` channel at `app/input_reader.rs:312-313` with `InputEvent::Key`/`Paste` standing in for `data`/`paste`. `StdinBufferOptions` (`stdin-buffer.ts:257-263`) — a constructor knob with no object to configure; **the 10 ms default's *behaviour* is not N/A and is TUI-045**. `flush()`/`clear()`/`getBuffer()`/`destroy()` (`:400-433`) — public API servicing pi's own buffer instance; crossterm owns the equivalent and exposes none of it, and `clear()`'s teardown role is served by `drain.rs`. The zero-length-chunk guard (`:308-311`) — crossterm's reader never surfaces a zero-byte event (`tty.rs:88-101`). The SGR-mouse completion logic (`:43-46`, `:102-120`) — cyrup enables no mouse reporting and `map_event_on` drops `Event::Mouse(_)` (`app/input_reader.rs:431`); already owned by TUI-019, and crossterm parses both forms correctly regardless. `word-navigation.ts:11`'s `segment?` option **as a pluggable option** — the two behaviours it delivers are TUI-043 and TUI-048, filed rather than dismissed.

**Rejected, with reasons — do not re-derive these.**
- **TUI-S01 re-filed as a separate open item.** Rejected. Its own Overlap note says the correct framing is the missing sink, and the sink now exists; the three unrendered variants are TUI-014 (widgets) and TUI-033 (header/footer). Holding S01 open as well would book the same defect three times. It is closed as framed and carried in the status table as a pointer.
- **TUI-008's `app.pageUp`/`app.pageDown` spelling complaint as part of TUI-008.** Rejected as scoped there. `app/input.rs:63-80` now self-documents the deferral behaviour as deliberate, and the spelling question belongs to the whole-namespace problem — moved to **TUI-028**.
- **TUI-020 as `closed`.** Rejected; overturned to partially-closed. Dropping the ` ({href})` parenthetical made the visible row match pi, but the item's title is "never emitted" and cyrup still emits OSC-8 nowhere.
- **TUI-S02 as `closed`.** Rejected; overturned to partially-closed. The `uncaughtCrash` half landed; `DEAD_TERMINAL_ERROR_CODES` → `emergencyTerminalExit` did not.
- **TUI-S11's release-feed half as an open item.** Rejected. The item's own Impact paragraph recommended scoping to the extension-package check, and `crates/cyrup/src/main.rs:552` records the fork/product decision explicitly. Closed as scoped.
- **`crates/cyrup-tui/src/tests/terminal_theme_query.rs:220-224`** as a `test-defect` sibling of TUI-N09. Rejected: a `< 1s` upper bound on a bounded probe is monotone-safe.
- **`crates/cyrup-tui/src/terminal_title.rs`'s control-char stripping** as a parity divergence from `terminal.ts:515`. Rejected: it is a documented CYRUP-DELTA hardening that removes no behaviour.

**Blind spots — where the next pass should look first.**

1. **Rendering fidelity is not covered by this file.** `cyrup/TUI-FIDELITY.md`'s ~150 presentation divergences live in a document with no stable IDs and no status table, and this pass did not re-audit which of its rows batches 4–10 actually closed. Any row still open there is invisible to `00-residual-ledger.md`. **TUI-016 is already an instance of the two documents drifting apart** — a fidelity fix (C14) removed the only surface a gap item depended on. Merging that backlog into this file with real IDs is the single highest-value follow-up for area 07.
2. **Selector internals were sampled, not swept.** `tree_selector.rs` was read end to end because a lead pointed there, and TUI-027 (high) was found in the first selector opened. `session_selector.rs`, `config_selector.rs`, `settings_selector.rs`, `model_selector.rs`, `auth_select.rs`, `user_message_selector.rs`, `oauth_selector.rs`, `login_dialog.rs`, `session_search.rs`, `export.rs`, `diff.rs`, `fuzzy.rs`, `overlay.rs` and `component.rs` were **not** compared against their upstream components for behaviour. In particular `/resume`'s `app.session.*` keymap and `/scoped-models`' `app.models.*` keymap carry cyrup-chosen defaults never diffed against `core/keybindings.ts:135-176`. The base rate suggests more of TUI-027's class.
3. **The `app` tree is 10,511 lines across 33 modules and roughly a fifth was read.** *(Restated 2026-08-19: this said "`crates/cyrup-tui/src/app.rs` is now ~7,100 lines"; `40821ed` split that file into `crates/cyrup-tui/src/app/`, which does not shrink the blind spot — it makes it enumerable. `wc -l crates/cyrup-tui/src/app/*.rs` is the inventory this pass did not have when the number was written.)* The run loop (`app/run.rs`, `app/run_arms.rs`, `app/run_action.rs`), `ingest_session_event`'s ~60 event arms (`app/events_fold.rs`, 494 lines) and `run_command`'s ~40 command arms (`app/submit.rs` → `app/execute*.rs`) were sampled only at the points items pointed to. An event arm that is a silent no-op where pi does something would not have surfaced.
4. **Editor internals unexamined.** `editor.rs` (visual motion, wrapping, undo, kill-ring, paste, bracketed paste, hardware cursor) was touched only for keybinding ids and prompt history. pi's `packages/tui/src/components/editor.ts` is ~1,400 lines; the v0.84.1 diff to it was one hunk (TUI-035), but the v0.83.0 baseline port has never been audited behaviourally by any pass.
5. **The seven new `-S` closures are now unaudited new code.** `panic_hook.rs`, `drain.rs`, `tmux.rs`, `keyboard_protocol.rs`, `footer_data.rs`, `resume_hint.rs`, `update_check.rs`, `terminal_progress.rs`, `terminal_title.rs` each exist, are wired, and cite the correct upstream sites in their docs — enough to overturn "ABSENT", but per this method's own rule a closure means the subsystem now **exists**, not that it is **correct**. TUI-S02's overturned closure is the first instance found; the PROV-005 precedent (a closure that hid three new highs) says to expect more. None of the nine was read line-for-line against pi.
6. **Runtime behaviour is unverifiable statically.** Terminal negotiation (`keyboard_protocol.rs`, `terminal_query.rs`, `drain.rs`) is argued from module docs and byte constants matching pi's. Whether a real kitty/iTerm2/tmux answers within the timeouts, and whether the OSC-0 / OSC-9;4 / OSC-8 writes interleave correctly with ratatui's buffer flush, cannot be shown by reading. Per this workspace's rule that the TUI is not done until run in a real terminal, every `-S` closure recorded here should be confirmed live before being trusted — which is also the argument for **TUI-040**.
7. **Cross-area handoffs opened this pass.** TUI-030's `onTerminalInput` / `setEditorComponent` halves and TUI-034's guest-facing transformer registration need WIT-world changes and must be reconciled with **area 06**. TUI-031's queue drain touches the session's compaction lifecycle in **area 03** (the compaction/streaming interaction was traced in `session.rs` for this pass, but the drain design is area 03's to own). TUI-006's and TUI-N05's remaining diagnostic sources need new fields on `StartupDiagnostics` in `cyrup-session-svc`, **area 08**, which also owns the exit code TUI-S02 needs. **Added in the repair pass:** TUI-051 and **CFG-048** are one behaviour split across two areas (the keybinding name migration has a write-time site in `migrations.rs` and a read-time site on `/reload`), and CFG-048 must land before **TUI-028** or the namespace rename breaks every `editor.*` config written against shipped cyrup. TUI-019's launch-failure and settings halves are **SEAM-051** (area 08) and **CFG-021** (area 05) and are deliberately not counted here.
8. **NEW (repair pass) — a startup-path sweep landed on `crates/cyrup-tui` code that area 08 owns, and nothing here duplicates it.** A separate sweep over `pi/packages/coding-agent/src/cli/` (`session-picker.ts`, `startup-ui.ts`, `config-selector.ts`, `list-models.ts`, `file-processor.ts`, `initial-message.ts`) produced ten findings routed to **area 08** and this area jointly. Several of them edit files in this crate — `crates/cyrup-tui/src/session_selector.rs` (the `--resume` picker merges current-folder and all-projects sessions into one list headed "Current Folder", and advertises a `tab scope` toggle that has no `SessionAction`; the pre-launch picker also offers a rename it silently discards), and the pre-launch surfaces that hardwire `UiTheme::default()` and `SelectKeymap::default()` instead of the user's theme and `keybindings.json`. **No TUI id is filed for any of them**, because the defects live in the pre-launch chrome (`crates/cyrup/src/{main,startup_ui,subcommands}.rs`) that area 08 owns, and double-filing would inflate the count. Recorded here per README blind spot 4 so the work is not orphaned: **if area 08's file does not carry these, they are unowned.** Note two of them are direct siblings of TUI-027's class — typed text accepted, echoed, and thrown away — and the pre-launch keymap gap is a sibling of TUI-051's.
9. **NEW (repair pass) — the six `packages/tui/src` non-drawing files are now read, but the drawing-adjacent ones still are not.** The sweep in `## Coverage` was scoped to files that draw nothing. `packages/tui/src/` also holds `autocomplete.ts`, `components/editor.ts` (~1,400 lines) and `components/markdown.ts`, which draw *and* carry portable behaviour, and blind spot 4 above already flags `editor.rs` as never behaviourally audited. TUI-042/043/044/049 were all found in `editor.rs` from the *outside* — by reading pi's helpers and tracing their consumers — which strongly implies more inside it. **`editor.rs` read line-for-line against `components/editor.ts` at v0.83.0 is the highest-value remaining target in this area**, ahead of the selector sweep in blind spot 2.

## Open questions — decisions required

Recorded separately because they are **not** severity judgements and must not be encoded as one. Per
the no-unapproved-deferrals rule, each names what is blocked, what is *not* blocked by it, and who
must decide.

**OQ-07-1 — Does cyrup build an alt-screen / fullscreen TUI mode at all? (TUI-019)**

- **Status:** undecided in any readable document. The prior "deliberate ADR-0001 divergence"
  justification is withdrawn — `PARITY-GAPS.md:709` records ADR-0001 as unreadable in this workspace,
  and README:208-212 forbids resting an item on an unverifiable ADR reference. **No decision of record
  exists**, so the previous `low` was encoding a decision nobody made.
- **Blocked on the answer:** the alt-screen `App` variant, mouse capture, the scrollbar, semantic
  prompt navigation, and the eight `tui.altScreen.*` keybinding ids — jointly effort L+, the largest
  single unit of unscheduled work in this area.
- **NOT blocked on the answer, and must be fixed either way:** **SEAM-051** (`--tui-mode`, whose
  *default* value `regular` currently makes the binary exit 1 with "unknown option") and **CFG-021**
  (`tuiMode` / `fullscreenScrollbar` modelled nowhere). A flag that rejects its own default is a
  defect under both answers. Do not let these wait on the decision.
- **Who decides:** a human, in a document in this workspace. If the answer is "no", it belongs in a
  readable ADR that this file can cite — at which point TUI-019 becomes a *scoped* mechanism
  difference whose behavioural cost (no mouse, no scrollbar, no jump-to-prompt) still stays on the
  list as work, because there is no accepted-divergence category.
- **Interim rating:** `medium`, judged on consequence within the item's own scope, as recorded in the
  item. It is not a placeholder for the decision and should not be re-lowered without one.

**OQ-07-2 — Does `cyrup/TUI-FIDELITY.md` get merged into this file with real IDs?**

- **Status:** raised by every recent pass (see blind spot 1) and never answered. 464 lines, ~150
  presentation findings against v0.84.1, no stable IDs, no status table, therefore invisible to
  `00-residual-ledger.md`.
- **Why it is a decision and not just work:** merging it would add on the order of 150 rows to a
  426-item ledger, most of them low. The alternative — leaving it out — has already cost real
  behaviour once: TUI-FIDELITY C14 deleted the `{n} queued` footer segment, which is exactly what
  turned **TUI-016** from "wrong surface" into "no surface at all", and no ledger reader could have
  seen that coming.
- **Who decides:** whoever owns backlog shape. Either answer is defensible; silence is not, because
  the two documents are actively drifting into each other.
