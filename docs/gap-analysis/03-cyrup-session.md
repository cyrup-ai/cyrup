# 03 — cyrup-session (persistence, compaction, prompt)

> **Re-audited 2026-08-12, cyrup HEAD `04c1ba2`** (working tree clean; repo HEAD `a9000b1` is
> docs-only, ~14 commits past the `1806375`/`9219dcd` baseline the previous pass was written at).
> **9 items closed** this pass (SESS-015, SESS-023, SESS-021, SESS-031, SESS-S01, SESS-S02,
> SESS-S03, SESS-S04, SESS-S06), **1 moved to partially-closed** (SESS-S05 — and the refuter cut
> the audit's claim that its render half was done, see the item), **0 reopened**, **8 newly filed**
> (SESS-035 … SESS-038, SESS-040 … SESS-043). One proposed finding, **SESS-039, was rejected and
> folded into SESS-035**; its ID is burned and must not be reused.
>
> **Upstream tags this pass was measured against.** cyrup's ported baseline is pi **v0.83.0**;
> every upstream citation below was taken from `git show v0.83.0:<path>`, not from the checkout on
> disk (which is `main` @ `581d75a89` = **v0.84.1**). The version-lag sweep ran
> `v0.83.0..v0.84.1` over this area's paths and returned **37 files / +4253 / −3**, of which all but
> 8 lines is the brand-new `packages/session-backends/sqlite-node` package — filed as SESS-038.
> `core/compaction/*`, `system-prompt.ts`, `skills.ts`, `session-cwd.ts` and `prompt-templates.ts`
> are **byte-unchanged** across the two tags, which materially narrows this area's drift exposure.
> The two real code changes in the window were both checked and are **not** findings: the
> `AGENTS.override.md` candidate (`resource-loader.ts:71`) is already ported at
> `crates/cyrup-session/src/prompt/context_files.rs:80-81`, and the `listAll` symlink widening
> (`session-manager.ts:1675-1677`) is already equivalent because Rust's `Path::is_dir()` follows
> symlinks where Node's `Dirent.isDirectory()` does not.
>
> The other three upstreams (pi-subagents v0.47.1, pi-permission-system v0.8.0, pi-intercom
> v0.10.1) contain nothing in this area's scope.
>
> **REPAIR PASS 2026-08-12 (completeness critique, findings 9 / 14 / 17c).** Four changes, no item
> renumbered, merged or deleted. (1) **The split-count hazard is gone.** `SESS-S05` lived in a second
> severity table under `## Surface-sweep findings`, making this the one area whose open set could not
> be read off a single table — flagged in three separate documents and never fixed. It is one row; it
> now sits in the main `## Open items` table and the second table is deleted. The
> `## Surface-sweep findings` heading, its provenance prose and the full `SESS-S05` body are
> retained, because the `-S` id is load-bearing. **Count one table in this file now.**
> (2) **`SESS-038` reclassified as a `tracker`** — its Fix is "scope decision first: this is a port
> of the v0.84.1 harness session contract plus one backend, not a bug fix", i.e. it proposes a
> decision rather than work. ID and body kept; excluded from the severity tally.
> (3) **`SESS-044` newly filed** from the cross-area migrations/keybindings surface sweep:
> `encode_cwd` strips *every* leading path separator where pi strips exactly one, and both cyrup
> copies carry a doc comment claiming pi compatibility. (4) **Every cross-tag citation in this file
> was re-verified** at both tags after the critique found a v0.84.1-offset citation asserted as
> tag-invariant elsewhere in the directory; this area's claims held — see `## Coverage`.
>
> Open set after the repair: **29 items — 0 critical, 1 high, 13 medium, 15 low**, in one table,
> plus one `tracker` (SESS-038) and one `partially-closed` residual (SESS-S05, which **is** counted —
> it is open work, unlike area 02's partially-closed entries whose residuals were re-filed under new
> ids).

This area covers `cyrup-session` (JSONL session store, entry model, branch/tree navigation,
compaction and branch summarization, system-prompt assembly) plus the parts of
`cyrup-core/src/message.rs`, `cyrup-session-svc` and `cyrup-tui` that own session state, stats and
the `/tree` flow. It is measured against
`pi/packages/coding-agent/src/core/{session-manager,system-prompt,skills,resource-loader,agent-session}.ts`
and `pi/packages/coding-agent/src/core/compaction/*` at pi **v0.83.0** — note bb301b6 re-aimed
compaction at pi's **live coding-agent fork**, so `coding-agent/src/core/compaction/compaction.ts`
(not the harness copy) is the oracle for everything below.

**Headline finding of this pass.** The two highs the previous pass filed both closed with real,
verified code behind them — `firstKeptEntryId` is now `Option` with all three read sites guarding,
and the `/tree` branch-summary flow is fully wired into the TUI including the Escape rebind and the
`aborted`-before-`cancelled` ordering. But the closure of SESS-023 is exactly what exposed the new
high: **the identical affordance one event handler away — cancelling a compaction — was never
ported at all.** The indicator advertises "(esc to cancel)", `AbortCompaction` has zero production
callers, and `abort_compaction()` cannot reach the auto-compaction token even if it were called.
That is three items (SESS-040/041/042) on a path that bills tokens and mutates the session file.
The second theme is unchanged from the last pass and did not move: the duplicated
`Compactor::run_branch_summary` still carries three divergences (SESS-017/022/034) from the live
`/tree` path that now works correctly, and the rendered-vs-raw token basis is still wrong at the
auto-compaction trigger (SESS-028) even though both `estimatedTokensAfter` sites were fixed.

## Status since the c8bd2ab baseline

| ID | Status | Note |
|---|---|---|
| SESS-001 | closed | Adversarial re-check held. `cyrup-core/src/message.rs:756-763` / `:773-775` normalize null/absent to `[]` with no role-union rejection, matching `session-manager.ts:383-399`. This closure is precisely what makes SESS-027's doc comments false. |
| SESS-002 | closed | Re-checked side by side against compaction.ts:418-435. `cutpoint.rs:130-149` is `est == 0 → continue`, same accumulate, same snap-forward, no type gate on either side. |
| SESS-003 | closed | `prompt/skills_inject.rs:25-29` filters `disable_model_invocation` and early-returns on empty, matching skills.ts:336-340. |
| SESS-004 | **still-open** | Unrecognized fields on *known* entry types still dropped on rewrite. |
| SESS-005 | closed | Back-scan `cutpoint.rs:161-171` breaks on compaction-entry / context-visible / `None` — same two conditions in the same order and the same loop bound as compaction.ts:437-446. |
| SESS-006 | closed | `compaction/branch.rs:130-134` implements `contextWindow \|\| 128000` then `− reserve`. Upstream re-derived by counting: **branch-summarization.ts:312-313**, reserve default `:305`. cyrup's own comments at `branch.rs:117`, `:121`, `mod.rs:360` remain off (SESS-025). |
| SESS-007 | **still-open** | Leading-blank-line session file still fails to open; `load` / `read_header` / `scan_file` still disagree. |
| SESS-008 | closed | `compaction/summarize.rs:245` routes through `retry_assistant_call` — the one choke point both production Summarizers take, as in compaction.ts:562-580. |
| SESS-009 | closed | `summarize.rs:234-239` sets `CacheRetention::None` plus a fresh session id, matching compaction.ts:569-573. Confirms DRIFT-005 is genuinely dead. |
| SESS-010 | closed | `summarize.rs:238` threads `reasoning`, gated at `:288-294` per compaction.ts:539-552. The branch path's `ModelThinkingLevel::Off` (`branch.rs:263`) is **correct**: pi builds branch options inline at branch-summarization.ts:350 and never sets `reasoning`. |
| SESS-011 | closed | `compaction/prepare.rs:42-52` hands the hook raw `AgentMessage`s via `message_for_compaction`, matching `getMessageFromEntryForCompaction` (compaction.ts:80-85); `convert_to_llm` applied later at `summarize.rs:355` per compaction.ts:650-651. |
| SESS-012 | **partially-closed** | Recording correct (`entry.rs:88-89`, `:102-103`; `combine_usage` `summarize.rs:145-174`). The **stats** reader now exists (`state.rs:106-111`, pi agent-session.ts:3120-3123). The **event** reader is still absent — remaining half is exactly SESS-030. |
| SESS-013 | **still-open** | No `findShadowedContextFile`; AGENTS.md double-load in a nested linked worktree. |
| SESS-014 | **still-open** | Whole-file reads for header discovery. Kind stays `upstream-drift`, severity low. |
| SESS-015 | **closed this pass** | `entry.rs:78-79` now `#[serde(default, skip_serializing_if)] first_kept_entry_id: Option<EntryId>`; all three read sites guard on `Some` (`context.rs:171-173`, `context.rs:308-310`, `compaction/prepare.rs:87-90`) and `mod.rs:427-431` fails closed with `CompactionError::MissingEntryId`. Refuter additionally read `migrate.rs` in full: `convert_first_kept_index`'s guards are behaviourally equivalent to pi's `targetEntry && targetEntry.type !== "session"` for idx 0, out-of-range, forward-reference, negative and float indices. Matches session-manager.ts:245-255 + :418-453. |
| SESS-016 | **still-open** | Explicitly-empty `selected_tools` still emits the skills section and the tool guidelines — and the empty case is now confirmed reachable on **both** sides. |
| SESS-017 | **still-open** | `Compactor::run_branch_summary` records `fromId` as the old leaf. |
| SESS-018 | **still-open** | `custom_message` null content renders `"null"`; missing `display` drops the entry. |
| SESS-019 | **still-open** | Footer `Current date` line + extra newline; `project_context` wording drift. `git diff v0.83.0..v0.84.1 -- system-prompt.ts` is empty, so `f4e9ca74`'s removal holds at v0.84.1. |
| SESS-020 | closed | `TokenCache` keyed `(EntryId, EstimateKind)` at `compaction/tokens.rs:200-268`; both projections cached independently and `invalidate` clears both. The two projections genuinely differ, so the split key is load-bearing. |
| SESS-021 | **closed this pass** | `cyrup-session-svc/src/state.rs:93-143` `SessionStats::from_entries(&[Entry], …)` walks ALL entries (sole caller `session.rs:3482` passes `mgr.entries()`), compaction/branch-summary `usage` arm first at `:106-111`, `tool_calls` `:124-129`, `cost` in `add_usage` `:146-152`. Struct `:36-54` carries all ten of pi's `SessionStats` fields (agent-session.ts:260-277, `getSessionStats` :3107-3161). |
| SESS-022 | **still-open** | `run_branch_summary` summarizes even when the user declined, because `skip_prompt` is consulted in the core. Now *doubly* wrong: the TUI consumes `skip_prompt` correctly at `app.rs:3728`. |
| SESS-023 | **closed this pass** | `cyrup-tui/src/app.rs:3719-3754` honours `branch_summary_skip_prompt()`, opens the three-option selector, handles `BranchSummary` / `BranchSummaryInstructions`; `begin_tree_navigation` (`app.rs:2860-2900`) aborts the in-flight stream, passes `NavigateTreeOptions { summarize, custom_instructions }`, sets `IndicatorKind::BranchSummary`; `apply_tree_nav_outcome` (`app.rs:2911-2960`) tests `aborted` **before** `cancelled` and re-shows the tree. Refuter additionally verified the three paths the audit did not cite — Escape on the selector → re-show tree (`app.rs:3356-3369`), pending-target seeding (`:3555-3557`), Escape → `AbortBranchSummary` in flight (`:1886-1897` + `:6597-6599`) — against interactive-mode.ts:4664-4752 at v0.83.0. Regression test `cyrup-tui/tests/tree_branch_summary.rs`. Landed in `cfe351e`. |
| SESS-024 | **still-open** | Skills preamble drops pi's relative-path resolution instruction (skills.ts:345). `skills.ts` byte-unchanged v0.83.0→v0.84.1. |
| SESS-025 | **still-open** | `reserve_tokens` doc and two test names still assert the pre-SESS-006 rule. |
| SESS-026 | **still-open** | `serialize_conversation` drops `[Assistant]: ` when the joined text is empty; pi guards on block presence. |
| SESS-027 | **still-open** | Three `cyrup-core` content-deserializer docs promise per-role validation their bodies deliberately removed. |
| SESS-028 | **still-open** | Auto-compaction threshold fallback still estimates the rendered context. Not fixed by SESS-031 — that landed two functions away. |
| SESS-029 | **still-open** | `estimate_tokens` applies one content-chars function to every role. |
| SESS-030 | **still-open** | `CompactionResult` omits pi's `usage`. (Duplicates area 08's `SEAM-034` — cross-reference, do not double-count.) |
| SESS-031 | **closed this pass** | `session.rs:1520-1526` (manual) and `:4336-4342` (auto) both fold `build_context_raw()` (`manager.rs:739`) through `compaction::tokens::estimate_agent_message`. Upstream verified exactly: agent-session.ts:1876 / :2157 `estimateMessagesTokens(sessionContext.messages)` over `buildSessionContext()` (session-manager.ts:460-468), the raw projection, computed on both sides *after* the compaction entry is appended. |
| SESS-032 | **still-open** | Two branch-summary tests still assert the SESS-017 divergence (lines drifted to `compaction.rs:523` and `:903`). |
| SESS-033 | **still-open** | `inputs_fingerprint` omits `disable_model_invocation`, still has no production caller, doc still claims otherwise. |
| SESS-034 | **still-open** | Before-tree seam has no channel for customInstructions / replaceInstructions / label. |
| SESS-035 | **new this pass** | Docs-pointer section never emitted in any production prompt — sole caller passes `DocsPointers::default()`. |
| SESS-036 | **new this pass** | Context-file ancestor walk canonicalizes cwd where pi uses `path.resolve`. **Corrected down to low** — see the item for why the auditor's mechanism was wrong. |
| SESS-037 | **new this pass** | `SessionManager::open` on a missing/zero-length file falls back to an EMPTY cwd, writing `"cwd": ""` into the header. |
| SESS-038 | **new this pass** | `packages/session-backends/sqlite-node` is a whole new v0.84.1 package with no cyrup counterpart. |
| SESS-039 | **rejected — ID burned** | Proposed as a `test-defect` over `prompt/tests.rs:61-63`. Rejected: that test pins *correct* behaviour of a pure function; the defect is the production wiring, which is SESS-035. Folded into SESS-035's Verify. **Do not reuse this ID.** |
| SESS-040 | **new this pass** | Compaction cannot be cancelled from the shipped binary — the Escape rebind was never ported and `AbortCompaction` has zero callers. |
| SESS-041 | **new this pass** | `abort_compaction()` never cancels an AUTO compaction; its own doc claims it does. |
| SESS-042 | **new this pass** | No abort re-check before `append_compaction` — a cancelled compaction is still written. |
| SESS-043 | **new this pass** | Agent transcript re-seeded from the LLM-flattened context at all three sites pi assigns the raw one. |
| SESS-S01 | **closed this pass** | `session.rs:3306-3308` adds `session_dir()`; `:3323-3329` `list_sessions()` reads `&self.services.session_dir` with `cwd_filter = (*dir != default_dir).then_some(cwd)` — pi's `filterCwd` (session-manager.ts:1638-1643). Refuter chased the field to its source: `builder.rs:1304-1310` derives it as explicit `--session-dir`, else the live manager's own file parent, else the layout dir — matching pi's two constructors (session-manager.ts:1519-1520, :1547-1548) — and `factory.rs:118-136` rebuilds it on every switch. |
| SESS-S02 | **closed this pass** | `runtime.rs:603-628`: `session_dir` derived from `current_file.as_deref().and_then(Path::parent)`, falling back to `factory.session_dir()` only for a non-persisted session; `destination = session_dir.join(file_name)`; `create_dir_all` at `:623`. Matches agent-session-runtime.ts:361-373 by construction. |
| SESS-S03 | **closed this pass** | `cyrup/src/main.rs:1243-1251` selects pi's two `listAll` overloads on `dirs.session_dir_explicit`; cwd filter wired at `main.rs:1261` / `:1274` via `session_list_cwd_filter(dirs)` (`main.rs:1196-1203`) and on the in-TUI path at `session.rs:3327`. Matches session-manager.ts:1653-1690 + :1638-1643. |
| SESS-S04 | **closed this pass** | `cyrup-tui/src/resume_hint.rs:1-110` implements `format_resume_command` / `resume_hint_line` / `ResumeTarget` including the `!usesDefaultSessionDir()` gate and quoting; emitted from `cyrup/src/main.rs:1215-1234`, invoked at `main.rs:580-582` on the clean interactive quit after `runtime.dispose()` — same ordering as interactive-mode.ts:231-244 + :3594-3596. |
| SESS-S05 | **partially-closed** | Default and gate fixed; **render shape and `formatLabelTimestamp` still missing**, and the data half is still open. The audit's "render half CLOSED" claim did not survive refutation — see the item. Moved into the main `## Open items` table by the 2026-08-12 repair pass; still counted, because the residual is real open work. |
| SESS-S06 | **closed this pass** | `listing.rs:214-236` matches `Entry::Unknown(v) if v.get("type") == Some("message")` alongside the typed arm and increments `message_count` on the TAG alone, body read field-by-field below — session-manager.ts:717-718 + :658-671. Pinned by `cyrup-session/tests/listing_unparseable_message.rs`. |

| SESS-038 | **tracker** | *(was low; reclassified in the 2026-08-12 repair pass.)* Proposes a scope decision, not work — ID and body kept, excluded from the severity tally. Must be answered together with area 02's AGENT-028: both turn on whether cyrup models pi's v0.84.1 agent-harness. |
| SESS-044 | **new** | *(2026-08-12 repair pass.)* low. `encode_cwd` strips every leading path separator where pi's `/^[/\\]/` strips exactly one; two independent cyrup copies, both carrying a doc comment that claims pi compatibility. |

Nineteen items now closed (ten held under adversarial re-check, nine closed this pass); two are
partially closed. Nine are new: SESS-035 … SESS-038 and SESS-040 … SESS-043 from the 2026-08-12
audit, plus SESS-044 from the 2026-08-12 repair pass. `SESS-039` is burned and `SESS-038` is now a
`tracker`, so the **counted** open set is 29 in a single table.

## Open items

> **✅ THIS TABLE IS NOW THE COMPLETE OPEN SET FOR THIS AREA.** The last `-S` item (`SESS-S05`) was
> moved here from the second table in the 2026-08-12 repair pass and that second table is deleted —
> this area was the final instance of structural defect A in `00-residual-ledger.md`, and all twelve
> area files now carry exactly one open-items table. **Do not re-add a second table**; a surface-sweep
> finding keeps its `-SNN` id for provenance and is listed here like every other item.
> `SESS-038` is a **`tracker`** (a scope decision, not work): it keeps its ID, this row and its full
> body, and is **not counted**. Counted total: **29**.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| SESS-040 | high | not-ported | M | Compaction cannot be cancelled from the shipped binary; the indicator advertises a dead key |
| SESS-007 | medium | parity-bug | S | Session file whose first physical line is blank fails to open |
| SESS-013 | medium | not-ported | M | AGENTS.md loaded twice in a nested git linked worktree |
| SESS-016 | medium | parity-bug | S | Explicitly EMPTY selected-tools list still emits skills and tool guidelines |
| SESS-017 | medium | parity-bug | M | `run_branch_summary` records `fromId` as the OLD leaf |
| SESS-018 | medium | parity-bug | S | `custom_message` null content renders `"null"`; missing `display` drops the entry |
| SESS-022 | medium | parity-bug | S | `run_branch_summary` summarizes even when the user declined |
| SESS-024 | medium | not-ported | S | Skills preamble drops pi's relative-path resolution instruction |
| SESS-028 | medium | parity-bug | S | Auto-compaction threshold fallback estimates the rendered context, not the raw one |
| SESS-032 | medium | test-defect | S | Two branch-summary tests assert the SESS-017 divergence as correct |
| SESS-035 | medium | not-ported | S | Docs-pointer section never emitted in any production system prompt |
| SESS-037 | medium | parity-bug | S | `SessionManager::open` on a missing/empty file writes `"cwd": ""` into the header |
| SESS-041 | medium | parity-bug | S | `abort_compaction()` never cancels an AUTO compaction |
| SESS-042 | medium | parity-bug | S | No abort re-check before `append_compaction`; a cancelled compaction is still written |
| SESS-004 | low | parity-bug | S | Unrecognized fields on known entry types dropped on rewrite |
| SESS-012 | low | not-ported | S | Summarization `usage` recorded, stats reader landed, event reader still absent |
| SESS-014 | low | upstream-drift | S | Session-header discovery reads entire session files into memory |
| SESS-019 | low | upstream-drift | S | Footer `Current date` line plus extra newline; `project_context` wording drift |
| SESS-025 | low | stale-port | S | `reserve_tokens` doc and two test names assert the pre-SESS-006 rule |
| SESS-026 | low | parity-bug | S | `serialize_conversation` omits `[Assistant]: ` for an empty text block |
| SESS-027 | low | stale-port | S | Content-deserializer docs promise validation their bodies removed |
| SESS-029 | low | parity-bug | S | `estimate_tokens` applies one content-chars function to every role |
| SESS-030 | low | not-ported | S | `CompactionResult` omits pi's `usage` |
| SESS-033 | low | cyrup-original | S | `inputs_fingerprint` omits `disable_model_invocation`, has no caller, doc claims otherwise |
| SESS-034 | low | not-ported | M | Before-tree seam has no channel for customInstructions / replaceInstructions / label |
| SESS-036 | low | parity-bug | S | Context-file ancestor walk canonicalizes cwd where pi uses `path.resolve` |
| SESS-043 | low | parity-bug | M | Agent transcript re-seeded from the flattened context, not pi's raw one |
| SESS-044 | low | parity-bug | S | `encode_cwd` strips every leading path separator where pi strips exactly one |
| SESS-S05 | low | not-ported | S | `TreeNode` drops pi's `labelTimestamp`; the `t` toggle switches an always-empty column of the wrong shape |
| SESS-038 | *(tracker)* | upstream-drift | L | `packages/session-backends/sqlite-node` (new at v0.84.1) has no cyrup counterpart — scope decision, not counted |

## SESS-040 — Compaction cannot be cancelled from the shipped binary: the Escape rebind was never ported, `AbortCompaction` has zero callers, and the indicator advertises "(esc to cancel)"

**Kind** not-ported · **Severity** high · **Effort** M · **Confidence** high (both sides read; three independent greps)

**cyrup** — `cyrup/crates/cyrup-tui/src/app.rs:4615-4639` handles `AgentSessionEvent::CompactionStart` by setting `IndicatorKind::Compaction` and nothing else — there is no `defaultEditor.onEscape` equivalent installed — while the comment at `:4624` states "The ` (<key> to cancel)` suffix is appended by the band from the live keymap", and the band at `app.rs:6044` duly renders `(${keyText("app.interrupt")} to cancel)`. `Action::Interrupt` (`app.rs:1886-1913`) checks `branch_summary_in_flight`, then bash, then streaming; its `AppAction::Interrupt` arm (`app.rs:6585-6593`) calls only `session.abort()` + `session.abort_bash()`. `rg -n AbortCompaction crates/` returns exactly two lines — the enum variant at `command.rs:32` and its handler at `command.rs:116-118` — i.e. **no caller anywhere**; `rg -n abort_compaction crates/` finds no production caller of `AgentSession::abort_compaction` (`cyrup-session-svc/src/session.rs:1677-1681`) either, only two tests.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:3074-3085` (v0.83.0): `case "compaction_start": … this.autoCompactionEscapeHandler = this.defaultEditor.onEscape; this.defaultEditor.onEscape = () => { this.session.abortCompaction(); }; this.showStatusIndicator(new CompactionStatusIndicator(…))`, restored in `case "compaction_end"` at `:3088-3095`. The rebind is installed on **every** compaction_start, manual and auto alike.

**Impact** — During `/compact` or an auto-compaction, cyrup shows a cancel key that does nothing. The user presses Escape, the indicator keeps spinning, the provider call runs to completion and bills, and `append_compaction` mutates the session file regardless. This is SESS-023's exact shape one event handler away — a whole user-facing affordance dead in the shipped binary — which is why it carries the same severity. It is also the reason SESS-041 and SESS-042 were invisible: with no caller, neither downstream defect could ever fire.

**Fix** — In `app.rs:4615-4639`, on `CompactionStart` save and replace the default-editor Escape handler with one dispatching `AppAction::AbortCompaction`, and restore it in the `CompactionEnd` arm; route that action to `AgentSession::abort_compaction()` via `command.rs:116-118`. Alternatively extend `Action::Interrupt` (`app.rs:1886-1913`) with a compaction branch ahead of the streaming branch, mirroring the existing `branch_summary_in_flight` check. Land with SESS-041, or the auto case still will not cancel.

**Verify** — Drive the TUI event loop through a `CompactionStart`, deliver Escape, and assert `abort_compaction` was invoked and no compaction entry was appended; assert `rg -n AbortCompaction crates/` shows a production dispatch site.

## SESS-007 — A session file whose first physical line is blank fails to open

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high (line refs exact both sides)

**cyrup** — `cyrup/crates/cyrup-session/src/manager.rs:851` `fn load`: blank-line skip at `:866-868`, but the header test is `if lineno == 0` (from `reader.lines().enumerate()`) at `:869`, so a leading blank line makes the real header line `lineno == 1`; it is parsed as an ordinary `Entry`, `header` stays `None`, and `NotASession` is returned at `:872` and again at `:886` via `header.ok_or_else`. The listing side is self-inconsistent: `listing.rs:176-177` `read_header` takes `text.lines().find(|l| !l.trim().is_empty())`, while `listing.rs:188` `scan_file` takes `lines.next()?` unconditionally.

**upstream** — `pi/packages/coding-agent/src/core/session-manager.ts:502-510` `parseSessionEntryLine` returns null for blank *and* malformed lines; `loadEntriesFromFile` (`:513-554`) validates the first *parsed* entry and returns `[]` rather than throwing (`:546-552`).

**Impact** — A session file with a stray leading newline (a truncated write, an editor, a merge) is unopenable, and the listing view disagrees with the loader about whether it exists at all.

**Fix** — In `manager.rs:851-886`, track "first parsed entry" rather than physical line index, and make `NotASession` a soft-empty return per pi. Share one rule across `load`, `listing.rs:176` and `listing.rs:188` — fix jointly with SESS-014.

**Verify** — Fixture session prefixed with `"\n"`; assert `load` succeeds, `read_header` and `scan_file` agree, and that a genuinely malformed first line yields empty rather than an error.

## SESS-013 — AGENTS.md loaded twice in a nested git linked worktree (no `findShadowedContextFile`)

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high (line refs exact both sides)

**cyrup** — `cyrup/crates/cyrup-session/src/prompt/context_files.rs:107-150` `load`; dedupe is exact-path only — `seen.insert(cf.path.clone())` at `:118` (global scan) and `:136` (ancestor walk). No shadowing predicate; `rg -n worktree crates/cyrup-session/` returns nothing.

**upstream** — `pi/packages/coding-agent/src/core/resource-loader.ts:100-115` `findShadowedContextFile`, both guards load-bearing at `:108` (`worktreeRoot.startsWith(\`${mainRepoRoot}${sep}\`)`) and `:113` (canonical `.git` comparison); call site `:136`, `isShadowed` guard `:140-142`.

**Impact** — Inside a linked worktree nested under its main repo, the same AGENTS.md content is injected twice — wasted context and duplicated instructions the model may weight more heavily.

**Fix** — Port `findShadowedContextFile` into `context_files.rs`, call it before the `seen` insert at `:118`/`:136`, and skip shadowed files. This is also the one place a realpath helper is legitimately needed — see SESS-036.

**Verify** — Create a linked worktree under the main repo with AGENTS.md at both roots and assert the built prompt contains the content once. Impact is still reasoned from code, not reproduced.

## SESS-016 — An explicitly EMPTY selected-tools list still emits the skills section and the tool guidelines

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high (both sides re-read; reachability now confirmed on both)

**cyrup** — `cyrup/crates/cyrup-session/src/prompt/builder.rs:272-274`: `fn is_selected(selected, name) -> bool { selected.is_empty() || selected.iter().any(...) }`. `selected_tools: Vec<Arc<str>>` at `builder.rs:38`, whose comment at `:36-37` explicitly says "Empty = default set", so "unset" and "explicitly empty" are indistinguishable. Gates at `:131-132` (`read_available`), `:172`, `:196-198`, `:202` all route through that predicate.

**upstream** — `pi/packages/coding-agent/src/core/system-prompt.ts:80` `const tools = selectedTools || ["read","bash","edit","write"]` — an empty array is truthy in JS, so `hasBash`/`hasGrep`/`hasFind`/`hasLs`/`hasRead` (`:97-101`) are all false and the skills gate at `:155` skips; the customPrompt branch does the same at `:63`. **Reachable on both sides**: pi's live caller always passes an array — `selectedTools: validToolNames` where `validToolNames = toolNames.filter(n => this._toolRegistry.has(n))` (agent-session.ts:1022, :1052) — and cyrup's production feed sets `selected_tools` from `base_tools` at `cyrup-session-svc/src/builder.rs:1048-1049`, so an empty base tool set lands squarely on this predicate.

**Impact** — A caller that deliberately restricts the agent to zero tools still gets skills advertised and tool-usage guidelines describing tools the model cannot call — instructions to use capabilities that do not exist.

**Fix** — Make `selected_tools` an `Option<Vec<Arc<str>>>` at `builder.rs:38`; `is_selected` becomes `match selected { None => DEFAULTS.contains(name), Some(v) => v.iter().any(...) }`. Update `read_available` and the three gates, plus the tool hash in the fingerprint (SESS-033).

**Verify** — Add a `prompt/tests.rs` case with `selected_tools: Some(vec![])` asserting no `<available_skills>` and no tool guidelines. Nothing currently pins the wrong behavior — all existing sites pass a non-empty vec.

## SESS-017 — `run_branch_summary` records `fromId` as the OLD leaf; pi records the navigation TARGET

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session/src/compaction/mod.rs:384` `session.branch(&target_id)?;` then `:388` `let from_id = old_leaf.clone().unwrap_or_else(|| EntryId::from("root"));` then `:389` `session.append_branch_summary(from_id, ...)`. The sibling does it pi's way: `manager.rs:642-663` `branch_with_summary`. The live `/tree` path is now confirmed correct too — `cyrup-session-svc/src/session.rs:1925-1935` calls `guard.branch_with_summary(new_leaf.as_ref(), ...)` — so the divergent path is exclusively the embedder-facing `Compactor::run_branch_summary`, and cyrup's two branch-summary implementations contradict each other.

**upstream** — `pi/packages/coding-agent/src/core/session-manager.ts:1381-1404` `branchWithSummary`: `this.leafId = branchFromId` (`:1391`), `parentId: branchFromId` (`:1395`), `fromId: branchFromId ?? "root"` (`:1397`) — one value for all three. Call site `agent-session.ts:3036-3042`, with the comment "Summary is attached at the navigation target position (newLeafId), not the old branch".

**Impact** — Summaries produced through the SDK/embedder path carry a `fromId` pointing at the abandoned leaf. Consumers reconstructing branch provenance from `fromId` get a different graph than pi's, and the same session gets different metadata depending on which entry point produced it.

**Fix** — Collapse `Compactor::run_branch_summary` onto `session.branch_with_summary(Some(&target_id), ...)` (`manager.rs:642-663`), deleting the local `branch` + `append_branch_summary` pair at `mod.rs:384-389`. This also removes SESS-022 and SESS-034.

**Verify** — Assert `entry.from_id == entry.parent_id` (pi's invariant) for summaries from both entry points. Requires flipping the two assertions filed as SESS-032.

## SESS-018 — `custom_message` null content renders literal `"null"`; missing `display` drops the entry

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high (line refs verified both sides)

**cyrup** — `cyrup/crates/cyrup-session/src/entry.rs:119` `content: Value,` and `:120` `display: bool,` — neither carries `#[serde(default)]` (contrast `:121` `details`, which does), so an absent `display` fails `from_value::<KnownEntry>` and `entry.rs:277-279` demotes the entry to `Entry::Unknown`. `agent_message.rs:252-260` `custom_to_message` ends `other => vec![Content::text(other.to_string())]` at `:257`, so `Value::Null` injects the four characters `null` into a user turn.

**upstream** — `pi/packages/coding-agent/src/core/session-manager.ts:396-399` `createCustomMessage(entry.customType, entry.content ?? [], entry.display, entry.details, entry.timestamp)` — `?? []` normalizes null/undefined and a missing `display` is simply falsy; `createCustomMessage` itself (`messages.ts:123-136`) does no validation.

**Impact** — Two ways a foreign or extension-written session degrades: a custom entry without `display` silently vanishes from context, and one with null content feeds the model the token `null`.

**Fix** — `#[serde(default)]` on `entry.rs:119-120`, and a `Value::Null => Vec::new()` arm before the catch-all at `agent_message.rs:252-260`.

**Verify** — Round-trip a custom entry with `content: null` and one with no `display`; assert the first contributes zero content blocks and the second survives as `KnownEntry::CustomMessage` with `display == false`.

## SESS-022 — `Compactor::run_branch_summary` generates and appends a summary even when the user declined one

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session/src/compaction/mod.rs:352`: `if !user_wants_summary && settings.skip_prompt { (None, false) } else if collection.entries.is_empty() { ... } else { ...generate at :369-378... }`. `skip_prompt` defaults false (`compaction/settings.rs:40-41`, `:46`), so `run_branch_summary(..., user_wants_summary = false, ...)` falls through to the generator and appends at `:389`.

**upstream** — `pi/packages/coding-agent/src/core/agent-session.ts:2983` gates strictly on the user's choice: `if (options.summarize && entriesToSummarize.length > 0 && !extensionSummary)`. `skipPrompt` is UI-only upstream — it appears nowhere in `agent-session.ts`, and its sole consumer repo-wide is `interactive-mode.ts:4672`.

**Impact** — An embedder navigating with `summarize: false` still pays for a summarization LLM call and gets an unwanted `branch_summary` entry written into the session — the inverse of the setting's documented meaning. Now doubly wrong: cyrup's own TUI consumes `skip_prompt` correctly at `app.rs:3728` (SESS-023's closure), which makes the core-level consult redundant as well as divergent.

**Fix** — Replace the condition at `mod.rs:352` with a plain `if !user_wants_summary`, and leave `skip_prompt` as a front-end setting consumed only by the TUI. Or fold into the SESS-017 collapse.

**Verify** — Call `run_branch_summary(..., user_wants_summary = false, ...)` with a non-empty collection and a recording summarizer; assert the summarizer was never invoked and no entry appended. Nothing pins the wrong behavior today — all five `skip_prompt` sites in tests pass `false`.

## SESS-024 — `<available_skills>` preamble drops pi's relative-path resolution instruction

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** high (upstream line verified verbatim)

**cyrup** — `cyrup/crates/cyrup-session/src/prompt/skills_inject.rs:17-18`: the entire preamble is the single const `SKILLS_PREAMBLE = "Available skills (open the SKILL.md with the read tool to use one):"`, pushed at `:31`.

**upstream** — `pi/packages/coding-agent/src/core/skills.ts:342-347` builds five preamble strings. The behavioural one is `skills.ts:345` verbatim: "When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands." `git diff v0.83.0..v0.84.1 -- packages/coding-agent/src/core/skills.ts` is empty, so this is not drift — it was never ported.

**Impact** — Skills whose SKILL.md references sibling files by relative path (`references/palette.md`, `scripts/run.sh`) resolve against the agent's cwd instead of the skill directory, so the model issues reads that miss. Silent skill failure that looks like a badly written skill.

**Fix** — Add the resolution sentence as a second preamble line at `skills_inject.rs:17-18`. The section is already gated on `read` availability, so the instruction is only emitted when actionable.

**Verify** — `prompt/tests.rs` assertion that a prompt with at least one skill contains "resolve it against the skill directory".

## SESS-028 — Auto-compaction threshold fallback estimates the RENDERED context, not pi's raw AgentMessage context

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session-svc/src/session.rs:4194-4195` — the Case-2 threshold fallback is still `let messages = self.messages().await; let estimate = estimate_context_tokens(&messages);`. `messages()` (`session.rs:3408-3410`) is `manager.build_context().messages`, the `convertToLlm`-flattened `Vec<Message>`; its own doc at `:3405-3407` says so. The raw twin `build_context_raw()` (`manager.rs:739`) is now used two functions away at `session.rs:1520` and `:4336` (SESS-031's fix) but **not here**. `Compactor::should_compact` (`compaction/mod.rs:114-128`), which does it correctly, still has exactly one caller workspace-wide and it is a test (`cyrup-session/tests/compaction.rs:241`).

**upstream** — `pi/packages/coding-agent/src/core/agent-session.ts:2020-2021` reads `this.agent.state.messages` — `AgentMessage[]` — into `estimateContextTokens`, whose per-message dispatch is over raw roles (`compaction.ts:266-301`, including `bashExecution`, `custom`, `branchSummary`/`compactionSummary`).

**Impact** — On the persistent-API-error path (stopReason `error`, or all-zero usage) cyrup measures a different number than pi: the rendered projection drops `excludeFromContext` bash messages and pads summaries with LLM wrapper prose, so auto-compaction fires early or late. cyrup's own test proves the bases differ (`tests/compaction.rs` asserts the raw estimate is pi's captured 77 and `assert_ne!(rendered_tokens, 77)`). Secondary hazard: a correct raw-basis `should_compact` sits dead beside the live wrong-basis one, so a future editor may "fix" the dead copy and observe no behaviour change.

**Fix** — At `session.rs:4194-4195` estimate over `build_context_raw()`, or route the whole Case-2 check through `Compactor::should_compact` so there is one implementation. Note the fix is **incomplete without SESS-043**: pi reads the agent's live state, which cyrup also seeds from the flattened projection, so the basis is wrong in two places.

**Verify** — Build a branch with an `excludeFromContext:true` bash entry plus a compaction summary, hand `check_compaction` an assistant turn with `stop_reason = Error`, and assert the trigger decision follows the raw estimate the test already pins rather than the rendered one.

## SESS-032 — Two branch-summary tests assert `from_id` is the OLD leaf, pinning the SESS-017 divergence

**Kind** test-defect · **Severity** medium · **Effort** S · **Confidence** high (both line numbers re-verified at HEAD)

**cyrup** — `cyrup/crates/cyrup-session/tests/compaction.rs:523` `assert_eq!(entry.from_id, l1, "from_id is the leaf navigated from (R-05-016)");` (drifted from `:519`) and `:903` `assert_eq!(entry.from_id, abandoned, "from_id is the abandoned leaf navigated from");`, each paired with a `parent_id` assertion naming the navigation target — two assertions that contradict each other under pi's model. Both drive `Compactor::run_branch_summary`, the path whose `from_id` disagrees with the sibling `SessionManager::branch_with_summary` the live `/tree` uses.

**upstream** — `pi/packages/coding-agent/src/core/session-manager.ts:1391-1397` sets `this.leafId = branchFromId`, `parentId: branchFromId` and `fromId: branchFromId ?? "root"` — one value for all three.

**Impact** — The classic "test pins current-but-wrong behavior" shape. The suite makes SESS-017 look intentional: an auditor grepping for coverage finds two green assertions citing a spec id, while the contradicting sibling implementation has no assertion against it at all. Any SESS-017 fix reads as a regression until these flip.

**Fix** — With SESS-017, change both assertions to expect the navigation target and drop the R-05-016 justification from the comments — `spec/` is not in this workspace and cannot be cited to defend behavior contradicting pi's code.

**Verify** — After the fix, `rg -n 'from_id is the .*navigated from' crates/cyrup-session/tests/` returns nothing and both tests assert `entry.from_id == entry.parent_id`.

## SESS-035 — The docs-pointer section is never emitted in any production system prompt; the sole caller passes `DocsPointers::default()`

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** high (both sides read; grep run twice independently)

**cyrup** — `cyrup/crates/cyrup-session-svc/src/builder.rs:1059` `docs: DocsPointers::default(),` is the **only** production site that populates `PromptInputs.docs` (`rg -n DocsPointers crates --type rust` returns the definition/export/doc lines in `cyrup-session`, this one, and two in `prompt/tests.rs`). `DocsPointers` derives `Default` over three `Option<PathBuf>` (`prompt/builder.rs:18-23`), so all three fields are `None`, `emit_docs_section` (`prompt/builder.rs:288-291`) hits `if docs.is_empty() { return; }` on every build, and the whole block is dropped. There is no `get_readme_path` / `get_docs_path` / `get_examples_path` anywhere in `cyrup-config` or `cyrup-session-svc`. Beyond the wiring, the block cyrup *would* emit is 3 lines (`prompt/builder.rs:295-312`) against pi's 8.

**upstream** — `pi/packages/coding-agent/src/core/system-prompt.ts:74-76` computes `getReadmePath()` / `getDocsPath()` / `getExamplesPath()` unconditionally, and `:131-138` sits inside the default-body template literal with **no guard**, so every default-body prompt carries it. The three behavioural lines are `:135` (resolve `docs/…` under Additional docs and `examples/…` under Examples, not the cwd), `:137` (read the docs and examples and follow `.md` cross-references before implementing) and `:138` (read `.md` files completely and follow links). The path helpers are `pi/packages/coding-agent/src/config.ts:427-439`, each a plain `resolve(join(getPackageDir(), …))`. Byte-identical at v0.83.0 and v0.84.1.

**Impact** — The model is never told where cyrup's own documentation lives. Every question about cyrup itself — extensions, themes, skills, prompt templates, TUI components, keybindings, SDK, custom providers, adding models, environment variables — is answered from training data (which contains nothing about cyrup) instead of the shipped docs, and the model has no instruction to follow `.md` cross-references. Silently wrong output on exactly the class of question a coding agent shipping its own docs most needs to get right, and invisible from inside the prompt builder because the feature *looks* implemented. Severity is internally consistent with SESS-024, which is medium for dropping one preamble instruction.

**Fix** — Add path helpers mirroring `pi/packages/coding-agent/src/config.ts:427-439` (resolve README.md / docs / examples relative to the installed package dir) in `cyrup-config`, and populate them at `cyrup-session-svc/src/builder.rs:1059` instead of `DocsPointers::default()`. Separately, extend `DEFAULT_TEMPLATE.docs_header` / `emit_docs_section` (`prompt/builder.rs:102-103`, `:288-312`) with cyrup-equivalents of pi's `:135`, `:137` and `:138` — those are behavioural instructions, not branding.

**Verify** — Assert over a prompt built through `SessionBuilder` (not a hand-constructed `PromptInputs`) that the output contains the docs header and a `README:` line, and that `rg -n DocsPointers crates/cyrup-session-svc/src` no longer returns a `::default()` construction. **Includes the rejected SESS-039**: the existing assertions at `prompt/tests.rs:61-63` / `:77-78` fabricate a populated `DocsPointers` no production caller produces, so they stay green either way — re-point them at the real producer in the same change, or this fix ships with its test still passing for the wrong reason.

## SESS-037 — `SessionManager::open` on a missing or zero-length file falls back to an EMPTY cwd, writing `"cwd": ""` into the new session header

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high (both sides read; reachability chain traced through four files, not executed)

**cyrup** — `cyrup/crates/cyrup-session/src/manager.rs:87-106` `open_with_cwd`: the missing/zero-length branch at `:96-105` does `let cwd = cwd_override.map(Path::to_path_buf).unwrap_or_default();` (`:99`) — `PathBuf::default()` is the **empty** path — then `SessionHeader::new(id, cwd.to_string_lossy(), now_ts())` (`:100`) persists `"cwd": ""`. `SessionManager::open` (`:77-79`) passes `None`. Reachable from the CLI: `cyrup/src/session_resolve.rs:71-75` `match_session_arg` returns `SessionLookup::Path` for any arg containing `/` or ending `.jsonl` **without checking existence**, `:199` maps it to `SessionTarget::Resume(p)` with `cwd_override` left at its `None` default (`cyrup-session-svc/src/builder.rs:178`), and `builder.rs:568` calls `open_with_cwd(path, None)`. Neither `main.rs:1052` nor `:1117` — the only two `cwd_override` assignments in the bin — is on that branch. Also reachable from `cyrup-session-svc/src/runtime.rs:503` and `:646` (`SessionManager::open(&path)?.cwd()`).

**upstream** — `pi/packages/coding-agent/src/core/session-manager.ts:1530-1549` `static open`: `:1534` guards `if (cwdOverride === undefined && existsSync(resolvedPath))` before reading a header at all, and `:1546` is `const cwd = cwdOverride ?? (header ? getSessionHeaderCwd(header) : undefined) ?? process.cwd();`; the constructor then does `this.cwd = resolvePath(cwd)` (`:876`), and `newSession` (`:930-953`) writes it into the header at `:941`.

**Impact** — `cyrup --session ~/notes/new-session.jsonl` — a path pi accepts as "start a session here" — creates a session whose header records `"cwd": ""`. That session is thereafter unattributable: `session_cwd_matches` never matches it, so it is filtered out of every cwd-filtered listing (`cyrup-session/src/listing.rs:168-171`, `cyrup-session-svc/src/session.rs:3327`, `cyrup/src/main.rs:1261`), and `SessionInfo.cwd` reads as the "old session, unknown cwd" sentinel. On the `switch_session` / `import` paths a zero-length target produces the nonsense error `MissingSessionCwd("")` where pi opens the session normally — and cyrup's two guards disagree with each other as well as with pi: `runtime.rs:505` and `:648` do a bare `if !cwd.exists()` with none of pi's short-circuits (`session-cwd.ts:22-25` returns no-issue when `!sessionCwd`), while the CLI twin `session_resolve.rs:275` `session_cwd_is_missing` **does** carry the `!session_cwd.is_empty() &&` guard.

**Fix** — At `manager.rs:99` replace `unwrap_or_default()` with pi's `?? process.cwd()` equivalent, or — better, since `cyrup-session` avoids ambient state — give `open_with_cwd` a `fallback_cwd: &Path` parameter and thread `cfg.cwd` from `builder.rs:568` and `services.cwd` from `runtime.rs:503`/`:646`. Separately add the `!is_empty()` short-circuit to `runtime.rs:505`/`:648` so the two cyrup paths agree.

**Verify** — Call `SessionManager::open(<nonexistent>.jsonl)` and assert `manager.cwd()` is the supplied fallback and the flushed header's `cwd` is non-empty; repeat with a `touch`-created zero-byte file; then assert the resulting file appears in `list_in_dir(dir, Some(cwd), None)`.

## SESS-041 — `abort_compaction()` never cancels an AUTO compaction, and its own doc comment claims it does

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session-svc/src/session.rs:1677-1681`: `pub fn abort_compaction(&self) { if let Some(c) = Self::lock(&self.compaction_cancel).as_ref() { c.cancel(); } }` — it touches only `compaction_cancel`, installed by the **manual** path at `session.rs:1397-1398`. The auto path installs a different token at `session.rs:4239-4240` (`*Self::lock(&self.auto_compaction_cancel) = Some(cancel.clone())`) that `abort_compaction` never reaches. `is_compacting` (`session.rs:4110-4114`) correctly checks all three tokens, and the doc block at `session.rs:4232` asserts the auto run is "tracked under `auto_compaction_cancel` so `is_compacting`/`abort_compaction`" see it — the second half of that sentence is false.

**upstream** — `pi/packages/coding-agent/src/core/agent-session.ts:1929-1933`, docstring literally "Cancel in-progress compaction (manual or auto)": `abortCompaction(): void { this._compactionAbortController?.abort(); this._autoCompactionAbortController?.abort(); }`.

**Impact** — Even once SESS-040 wires a caller, Escape during an **auto** compaction still does nothing — and auto compaction (threshold/overflow) is the slow one the user most wants to escape, because it fires unprompted mid-turn. Both cyrup tokens are children of `session_cancel`, so `dispose_with` (`session.rs:2410-2424`) still tears them down; the gap is user-initiated cancellation only. The false doc comment is what makes this invisible to review.

**Fix** — Extend `abort_compaction` (`session.rs:1677-1681`) to cancel `auto_compaction_cancel` (and the branch-summary token if pi's shape warrants it), mirroring agent-session.ts:1929-1933. Land with SESS-040.

**Verify** — Start an auto compaction against a slow stub summarizer, call `abort_compaction()`, and assert the auto token is cancelled and no compaction entry is appended; assert the same for the manual path so the test covers both controllers.

## SESS-042 — No abort re-check before `append_compaction`: an extension- or hook-supplied compaction is written even after the user cancelled

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session/src/compaction/mod.rs:206-296` `finish_compaction` resolves the summary from one of three sources — an external extension override (`Some(ov)`, `:219-230`), the internal `before_compact` hook's `Custom` decision (`:253-263`), or the default summarizer (`:265-289`) — and then calls `session.append_compaction(...)` at `:291` **with no cancellation test in between**. Only the default branch is abort-aware, and only indirectly, via `complete_summarization`'s `cancel.run_until_cancelled(call)` (`summarize.rs:245-249`), which returns `Some(msg)` — success — when the call had already completed.

**upstream** — `pi/packages/coding-agent/src/core/agent-session.ts:1866-1868` (manual): `if (this._compactionAbortController.signal.aborted) { throw new Error("Compaction cancelled"); }` immediately before `this.sessionManager.appendCompaction(...)` at `:1870`. The auto path guards the same window differently at `:2142-2151`, emitting `compaction_end { result: undefined, aborted: true }` and returning `false` before its `appendCompaction` at `:2153`. Both guards are unconditional and cover all summary sources.

**Impact** — A cancel that lands while a `session_before_compact` guest is producing a compaction — or in the window after the summarization stream settles but before the append — still appends the compaction entry, rewrites the branch and reports success, where pi refuses and reports `aborted`. Durable session mutation after the user said stop. Latent until SESS-040 lands a caller, which is precisely why it must land with it.

**Fix** — In `compaction/mod.rs:206-296`, re-test the cancellation token immediately before `append_compaction` at `:291` and return a cancelled/aborted outcome on all three summary paths; surface it as `aborted: true` on the `compaction_end` payload per agent-session.ts:2142-2151.

**Verify** — Script a `before_compact` hook returning a `Custom` summary, cancel the token while the hook is in flight, and assert no compaction entry was appended and the emitted result carries `aborted`. Repeat with an extension override.

## SESS-004 — Unrecognized fields on known entry types dropped on rewrite

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session/src/entry.rs:18-26` `EntryBase` declares only `id`/`parent_id`/`timestamp` with no `#[serde(flatten)]` catch-all. `impl Deserialize for Entry` (`entry.rs:264-287`) preserves the original JSON verbatim only on the `Err(_) => Ok(Entry::Unknown(v))` arm at `:279`.

**upstream** — `pi/packages/coding-agent/src/core/session-manager.ts:502-510` `parseSessionEntryLine` is a bare `JSON.parse` with a blank/malformed→null guard and no schema check, so unknown keys survive any rewrite.

**Impact** — A session written by a newer cyrup, a fork, or an extension that annotates entries loses those annotations the first time this cyrup rewrites the file. Silent data loss, bounded to non-modelled keys.

**Fix** — Add `#[serde(flatten)] extra: Map<String, Value>` to `EntryBase` (`entry.rs:18-26`) and re-emit it on serialize.

**Verify** — Round-trip a session entry carrying an unknown top-level key and assert byte equality. Standing caveat: keys nested inside `AgentMessage`/`Content` have still not been enumerated.

## SESS-012 — Summarization `usage` recorded, stats reader landed, event reader still absent

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — Recording is done and correct: `cyrup/crates/cyrup-session/src/entry.rs:88-89` (Compaction) and `:102-103` (BranchSummary) carry `usage: Option<Usage>`, `skip_serializing_if`-elided; `combine_usage` (`compaction/summarize.rs:145-174`) matches pi field-for-field. The **stats** reader now exists — `cyrup-session-svc/src/state.rs:106-111` destructures `usage` off both variants and folds it through `add_usage`. The **event** reader is still absent: `CompactionResult` (`state.rs:195-203`) has no `usage` field.

**upstream** — `pi/packages/coding-agent/src/core/compaction/compaction.ts:99-122` `combineUsage`, consumed at `agent-session.ts:3120-3123` (stats — now matched) and `:1894-1901` (the `compaction_end` payload — still unmatched).

**Impact** — Summarization spend now reaches stats but still never reaches the event stream.

**Fix** — Nothing further is owed by this item; the remaining half is exactly SESS-030.

**Verify** — Grep that a non-test site destructures `usage` off a compaction/branch-summary entry and that its value reaches **both** stats and the event payload. Half of that assertion passes today.

## SESS-014 — Session-header discovery reads entire session files into memory

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session/src/listing.rs:176` `read_header` → `std::fs::read_to_string(path).ok()?`; `listing.rs:187` `scan_file` → the same. `read_header` is called per-candidate from `newest_session`.

**upstream** — pi already has the bounded reader **at cyrup's ported tag**: the buffer constants at `session-manager.ts:489-493` (`SESSION_READ_BUFFER_SIZE`, `SESSION_HEADER_READ_BUFFER_SIZE`, `MAX_SESSION_HEADER_SCAN_BYTES`), `SessionHeaderScanLimitError` at `:494-499`, its catch inside `static open` (`:1535-1543`), and the chunked `readSync` loop in `loadEntriesFromFile` (`:513-554`).

**Impact** — Listing N sessions reads N whole files; on a directory of large sessions the picker stalls and peak memory tracks the largest file. It stays classified `upstream-drift` because pi's buffered reader landed in `f1c587dd` (2026-07-19), after cyrup's 2026-07-10 baseline — but note it is present at v0.83.0, so this is not "not yet released upstream", it is unported.

**Fix** — Replace both `read_to_string` calls with a bounded reader that stops at the first newline or a scan-byte cap. Do it with SESS-007 so `load`, `read_header` and `scan_file` share one first-entry rule.

**Verify** — Point `read_header` at a multi-megabyte session and assert bytes read stay under the cap.

## SESS-019 — Footer emits a `Current date` line pi has since REMOVED (plus an extra leading newline); `project_context` wording drifts

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** high (diff run on both tags)

**cyrup** — `cyrup/crates/cyrup-session/src/prompt/builder.rs:330-341` `emit_footer` writes `"\n\nCurrent date: {:04}-{:02}-{:02}"` — a blank line before the date — then `"\nCurrent working directory: {}"`. Wording deltas: `builder.rs:104` `project_context_open: "<project_context>\n\nProject-specific instructions follow.\n\n"` and `:105` `project_context_close: "</project_context>"` (no trailing newline).

**upstream** — at **both** tags: `pi/packages/coding-agent/src/core/system-prompt.ts:146-147` (`"\n\n<project_context>\n\n"` + `"Project-specific instructions and guidelines:\n\n"`), `:151` (`"</project_context>\n"`), `:159` (`\nCurrent working directory:`), with **no date line**. `git diff v0.83.0..v0.84.1 -- packages/coding-agent/src/core/system-prompt.ts` is empty, so `f4e9ca74`'s removal is already in the ported-baseline successor and holds at v0.84.1.

**Impact** — Byte-level prompt divergence from pi. The date line is a *faithful port* of the version cyrup ported — not a cyrup invention, and it owes no `[CYRUP-DELTA]` marker — but pi has removed it, so keeping it is a deliberate drift decision. The extra newline and the `project_context` wording are genuine port deltas independent of that decision.

**Fix** — Two separable pieces. (a) Drop the date line at `builder.rs:330-341` to follow `f4e9ca74` — upstream drift, so decide it jointly with DRIFT-016/DRIFT-035 in `12-upstream-drift-pi-core.md`. (b) Independently, fix the extra leading `\n` and align the `project_context` open/close strings at `builder.rs:104-105`.

**Verify** — Byte-compare a built prompt against a pi capture for the `project_context` region. The `prompt/tests.rs` `Current date:` assertions are **not** test-defects — they correctly pin the ported baseline and go stale only if (a) is taken.

## SESS-025 — `reserve_tokens` doc comment asserts the pre-SESS-006 budget rule; two test names repeat it

**Kind** stale-port · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session/src/compaction/settings.rs:35-36` still reads "Message budget (newest-first) for the branch summary (default 16384). Per the corrected R-05-016 this IS the message budget, not `contextWindow − reserve`." — contradicted by `branch.rs:130-134` `branch_token_budget`, which does exactly `contextWindow − reserve`. Stale test names survive: `tests/compaction.rs:533 fn branch_budget_is_reserve_tokens_newest_first` and `tests/parity.rs:122 fn gap5_cut_point_validity_excludes_settings_and_summaries`. Both test bodies assert correctly.

**upstream** — re-derived by counting from `branch-summarization.ts:295`: `reserveTokens = 16384` at `:305`, `const contextWindow = model.contextWindow || 128000;` at `:312`, `const tokenBudget = contextWindow - reserveTokens;` at `:313`. cyrup's own inline citations at `branch.rs:117`, `:121` and `mod.rs:360` still say `:315` / `:315-317` and are off by three to seven lines.

**Impact** — Documentation-only, but it is precisely the claim SESS-006 disproved; a reader trusting it will re-introduce the bug. The two misleading test names hide correct coverage.

**Fix** — Rewrite `settings.rs:35-36` to state the `contextWindow || 128000 − reserve` rule; rename `compaction.rs:533` and `parity.rs:122`; correct the three upstream line citations to `:312-313` / `:305`.

**Verify** — Grep that no doc under `compaction/` claims `reserve_tokens` is the budget.

## SESS-026 — `serialize_conversation` omits `[Assistant]: ` for a turn whose only text block is empty

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session/src/compaction/serialize.rs:30-32`: `let text = join_text(&a.content, "\n"); if !text.is_empty() { parts.push(format!("[Assistant]: {text}")); }` — guards on the *joined* text, so a single empty text block joins to `""` and the line disappears.

**upstream** — `pi/packages/coding-agent/src/core/compaction/utils.ts:135-137` guards on *presence*: `if (msg.content.some((block) => block.type === "text")) { parts.push(\`[Assistant]: ${contentText(msg.content)}\`); }`. The asymmetry is deliberate upstream — the user arm (`utils.ts:113-115`) and toolResult arm (`:141-145`) both guard on emptiness, which `serialize.rs:19-23` and `:38-43` correctly mirror.

**Impact** — The transcript handed to the summarizer loses an assistant turn marker, shifting the interleaving pi's summary prompt sees. Rare and low-consequence, but it changes summary input bytes.

**Fix** — Change the guard at `serialize.rs:30-32` to `a.content.iter().any(|c| matches!(c, Content::Text { .. }))`. Leave the user and toolResult arms alone.

**Verify** — Serialize an assistant message whose only content is `Content::text("")` and assert the output contains `[Assistant]: `. Nothing currently pins the wrong behavior.

## SESS-027 — Three `cyrup-core` content deserializers' docs promise per-role validation their bodies removed

**Kind** stale-port · **Severity** low · **Effort** S · **Confidence** high (all doc/body pairs read)

**cyrup** — `cyrup/crates/cyrup-core/src/message.rs:741-744` documents `de_user_content` as "validating the array to `Text|Image` only … a `Thinking`/`ToolCall` block is rejected" while its own body comment at `:756-758` says "with no role-union rejection". The same contradiction holds for `de_tool_result_content` (doc `:766-767` vs body `:773-775`) and `de_assistant_content` (doc `:778-780` vs its body). The field doc at `:634-641` repeats the false claim.

**upstream** — `pi/packages/coding-agent/src/core/session-manager.ts:502-510` `parseSessionEntryLine` is a bare `JSON.parse`; pi's per-role content unions are compile-time TypeScript only, never enforced at read time. The tolerant behaviour is the correct port and is exactly what SESS-001 closed.

**Impact** — Documentation-only. An auditor reading these docs concludes cyrup validates content-block roles on read; it does not, by design. Someone may "restore" the validation and break SESS-001.

**Fix** — Rewrite the three fn docs and the field doc (`message.rs:634-641`, `:741-744`, `:766-767`, `:778-780`) to state the actual read-tolerant contract and cite `session-manager.ts:502-510`.

**Verify** — Grep that no doc in `message.rs` claims a block type is "rejected" on deserialize.

## SESS-029 — `cyrup-session`'s `estimate_tokens` applies one content-chars function to every role

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session/src/compaction/tokens.rs:26-35` binds `blocks` uniformly for `User | ToolResult | Assistant` and sums `content_chars` over all three. `content_chars` (`tokens.rs:86-96`) charges `Content::Image => ESTIMATED_IMAGE_CHARS` (4800) and counts `Thinking`/`ToolCall` regardless of role. The workspace already holds a correct port of the same pi function: `cyrup/crates/cyrup-provider/src/utils/estimate.rs:80-101` gives the Assistant arm `Content::Image { .. } => 0`.

**upstream** — `pi/packages/coding-agent/src/core/compaction/compaction.ts:266-301` — the `assistant` arm (`:276-288`) accumulates only text, thinking and toolCall; an image contributes **nothing**. `estimateTextAndImageContentChars` (`:246-260`), used by the `user` (`:270-275`) and `custom`/`toolResult` (`:289-293`) arms, counts only text and image.

**Impact** — An assistant message carrying an image block costs 1200 tokens in cyrup and 0 in pi; a user/toolResult message carrying a thinking or toolCall block (reachable through the SESS-001 read tolerance on a foreign or hand-edited session) is counted where pi counts 0. This estimate feeds the persisted `tokensBefore`, the keep-recent cut-point walk and the raw trigger, so cyrup can cut at a different entry than pi on the same file. Two ports inside this repo disagree, and the provider one matches pi.

**Fix** — Split `estimate_tokens` at `tokens.rs:26-35` into role-dispatched arms mirroring `cyrup_provider::estimate_message_tokens`: user/toolResult → text+image only; assistant → text/thinking/toolCall only.

**Verify** — Add an assistant message of one text block plus one image block and assert the estimate equals `ceil(text_len/4)`, citing `compaction.ts:276-288`. Severity stays low because no cyrup code path currently places a `Content::Image` into an assistant message — reachable only via a foreign session or an extension.

## SESS-030 — `CompactionResult` omits pi's `usage`

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session-svc/src/state.rs:195-203` `CompactionResult { summary, first_kept_entry_id, tokens_before, estimated_tokens_after, details }` — no `usage`. Constructed at `session.rs:1552-1558` (manual) and `:4368-4374` (auto), where `entry.usage` is in scope and unread.

**upstream** — `pi/packages/coding-agent/src/core/compaction/compaction.ts:88-97` declares `/** Usage from the LLM call(s) that generated this summary, if available */ usage?: Usage;` at `:89-90`; `agent-session.ts:1894-1901` puts it on the object emitted as `compaction_end`.

**Impact** — An extension or RPC client watching `compaction_end` cannot see what the compaction cost, even though cyrup wrote that usage into the session file one line earlier. Any host doing its own cost accounting off the event stream silently under-reports every compaction. This is the remaining half of SESS-012.

**Fix** — Add `#[serde(default, skip_serializing_if = "Option::is_none")] pub usage: Option<Usage>` to `state.rs:195-203` and populate from the returned entry at `session.rs:1552` and `:4368`.

**Verify** — Run a compaction against a stub summarizer returning a known usage and assert the `CompactionEnd` payload's `usage` equals it and equals the usage on the appended compaction entry. **Cross-reference**: area 08 tracks the same defect as `SEAM-034` — fix once, close both, do not double-count.

## SESS-033 — `SystemPromptBuilder::inputs_fingerprint` omits `disable_model_invocation` and has no production caller, though its doc claims the agent caches on it

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session/src/prompt/builder.rs:222-223`: "The agent caches `(fingerprint -> prompt)` for the session and only rebuilds when it changes." The skills loop at `builder.rs:248-252` hashes only `s.name`, `s.description`, `s.path` — not `disable_model_invocation`, which `skills_inject.rs:25-26` makes output-affecting. The caching claim is false at HEAD: `rg -n inputs_fingerprint crates/` returns exactly three lines — the definition at `builder.rs:224` and two test call sites at `prompt/tests.rs:369-370`. No production caller.

**upstream** — pi has no prompt fingerprint. `buildSystemPrompt` (`system-prompt.ts:28`) is a pure function, rebuilt explicitly at `agent-session.ts:1021` `_rebuildSystemPrompt`. There is no upstream contract to inherit; the divergence is that cyrup's doc promises a cache that does not exist and the hash it would use is already incomplete.

**Impact** — Latent today (no caller). If the fingerprint is ever wired up as documented, flipping a skill's `disable-model-invocation` frontmatter and reloading leaves the stale prompt in place, re-advertising a skill the author just hid — re-opening SESS-003. Meanwhile the doc misleads anyone auditing prompt-cache correctness.

**Fix** — Hash `s.disable_model_invocation` in the skills loop at `builder.rs:248-252`, then either wire the fingerprint into a real cache or correct the doc at `builder.rs:222-223` to say it is currently unused. Also reconsider `inp.today.hash(&mut h)` at `builder.rs:255`, which forces a daily rebuild only because of the SESS-019 date line.

**Verify** — Build two `PromptInputs` differing only in a skill's `disable_model_invocation` and assert their fingerprints differ — the same shape as the existing with_tool/without_tool assertion at `prompt/tests.rs:369-370`.

## SESS-034 — `Compactor::run_branch_summary`'s before-tree seam has no channel for the hook's customInstructions / replaceInstructions / label

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session/src/compaction/hooks.rs:147-155`: `pub enum BeforeTreeDecision { Proceed, Cancel, CustomSummary { summary, details } }` — three variants only. `compaction/mod.rs:344-381` consumes exactly those, and the generator it calls, `generate_branch_summary` (`compaction/branch.rs:233-249`), hardcodes `BRANCH_SUMMARY_PROMPT` at `:249` with no instruction slot. The live `/tree` path supports all three through a different seam: `cyrup-session-svc/src/session.rs:1725-1731` threads the effective instructions into `generate_branch_summary_with_instructions`.

**upstream** — `pi/packages/coding-agent/src/core/agent-session.ts:2965-2975` reads `result.customInstructions`, `result.replaceInstructions` and `result.label` off the single `session_before_tree` hook result; `generateBranchSummary` honours the first two at `branch-summarization.ts:326-334` (`if (replaceInstructions && customInstructions) instructions = customInstructions; else if (customInstructions) instructions = \`${BRANCH_SUMMARY_PROMPT}\n\nAdditional focus: ${customInstructions}\`;`).

**Impact** — An extension that steers or replaces the branch-summary prompt, or labels the resulting entry, is silently ignored when the summary is produced through the embedder/SDK path — the same orphaned path that already carries SESS-017 and SESS-022. Three defects on one duplicated code path.

**Fix** — Fix with SESS-017/SESS-022 by collapsing `Compactor::run_branch_summary` onto the session-svc flow; alternatively widen `BeforeTreeDecision` with `custom_instructions` / `replace_instructions` / `label` and call `generate_branch_summary_with_instructions`. Collapsing removes all three at once.

**Verify** — Script a before-tree hook returning custom instructions and assert the recording summarizer saw a prompt containing "Additional focus:"; with `replace_instructions`, assert the prompt is the custom text alone.

## SESS-036 — Context-file ancestor walk canonicalizes the cwd where pi uses `path.resolve`

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** medium (code difference certain; blast radius inferred, and the auditor's mechanism was wrong — see below)

> **Refuter caveat, stated inline as required.** This was filed as medium with a `~/work/proj` and a
> macOS `/tmp → /private/tmp` scenario. Both scenarios are **wrong**, and the proposed fix is a
> no-op. (1) The cwd is already canonicalized long before it reaches the loader:
> `crates/cyrup-config/src/env.rs:171` does `let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);`
> and `cyrup-session-svc/src/builder.rs:1021` passes that value straight to
> `ContextFileLoader::new`, so `context_files.rs:124` is a **second, no-op** canonicalize on the
> production path — the real site is `env.rs:171`, and it affects far more than context files
> (session header cwd, the cwd-encoded session dir, trust keys). (2) `std::env::current_dir()` and
> Node `process.cwd()` both call `getcwd(3)` and return the **physical** path, so `cd` through a
> symlink gives pi and cyrup the identical realpath. The divergence requires a cwd from a
> non-`getcwd` source — an explicit `--cwd`, a resumed session header string, or a `cwd_override`.
> Hence **low**, not medium.

**cyrup** — `cyrup/crates/cyrup-session/src/prompt/context_files.rs:124` `let start = std::fs::canonicalize(&self.cwd).unwrap_or_else(|_| self.cwd.clone());`, then the ancestor loop at `:128-143` walks the parents of the **realpath**. The discovered `ContextFile.path` (`:163`) is likewise the realpath, and that string is interpolated verbatim into the prompt at `prompt/builder.rs:321-323` (`<project_instructions path="…">`). Upstream of it, `cyrup-config/src/env.rs:171` already canonicalized the cwd for every consumer.

**upstream** — `pi/packages/coding-agent/src/core/resource-loader.ts:122` `const resolvedCwd = resolvePath(options.cwd);` and `:137` `let currentDir = resolvedCwd;`, with the ancestor loop at `:139-152` using `dirname(currentDir)`. `resolvePath` (`pi/packages/coding-agent/src/utils/paths.ts:81-85`) is `node:path.resolve` — lexical normalization, **no** symlink resolution. pi deliberately keeps a separate `canonicalizePath` (`paths.ts:26-32`, `realpathSync`) and uses it in exactly one place in this file: the `findShadowedContextFile` worktree comparison (`:102-103`, `:113`), precisely because that comparison needs realpaths and the walk does not.

**Impact** — When the cwd arrives from a non-`getcwd` source (`--cwd <symlinked path>`, a resumed session header, a `cwd_override`), cyrup walks the target's ancestors and pi walks the link's: pi picks up `~/work/AGENTS.md`, cyrup picks up `/Volumes/ext/AGENTS.md`. Project instructions the user can see in their own directory tree silently do not reach the model, and the `path=` attribute the model is shown differs from the path the user supplied.

**Fix** — The real site is `cyrup-config/src/env.rs:171` — decide there whether cyrup normalizes lexically (pi's rule) or physically, since that value also keys the session dir and trust store; changing it is a wider decision than context files. Then drop the redundant `canonicalize` at `context_files.rs:124`. Keep a realpath helper available for the SESS-013 `findShadowedContextFile` port, which is the only place pi canonicalizes.

**Verify** — Create `<tmp>/real/proj` with `<tmp>/real/AGENTS.md`, symlink `<tmp>/link → <tmp>/real`, load with an **explicit** `cwd = <tmp>/link/proj` (not an inherited process cwd, which cannot reproduce it), and assert the discovered file's path is `<tmp>/link/AGENTS.md` — pi's answer.

## SESS-038 — `packages/session-backends/sqlite-node` is a whole new v0.84.1 package with no cyrup counterpart

**Kind** upstream-drift · **Severity** *(tracker — not counted)* · **Effort** L · **Confidence** high (both sides verified; counts corrected)

> **Reclassified `low` → `tracker`, 2026-08-12 repair pass** (completeness critique finding 14).
> This item proposes **no work**: its Fix opens "Scope decision first: this is a port of the v0.84.1
> harness session contract plus one backend, not a bug fix", and its stated purpose is to stop the
> package going unmentioned. An item whose output is a decision is bookkeeping, so it keeps its ID,
> its row and this body but leaves the severity tally. **It is not deferred and it is not closed** —
> the decision is genuinely outstanding, and if it comes back "in scope" this becomes a real `L`
> item and returns to the count. It is the sibling of area 02's AGENT-028: both turn on whether
> cyrup models pi's v0.84.1 agent-harness at all, and **they must be answered together** — the
> harness session contract SESS-038 would port is the same subtree AGENT-028's telemetry lives in.

**cyrup** — No crate under `crates/` implements a session repository other than the JSONL one: `crates/cyrup-session/src/store.rs` is 150 lines defining `trait SessionStore` at `:13` with exactly two impls (`DiskStore` `:42`, `MemStore` `:122`); `SessionManager` binds one at construction (`manager.rs:71`, `:101`, `:302`). There is no search index, no lane/record model, no writer lease, and no `seq` field anywhere (`entry.rs:18-26` has `id`/`parentId`/`timestamp` only). `rg -ni sqlite crates/cyrup-session crates/cyrup-session-svc` returns nothing.

**upstream** — `git diff --stat v0.83.0..v0.84.1 -- packages/session-backends` shows the entire tree as new: **37 files, +4253** (the audit's 39/+4258 was slightly off), including `sqlite-node/src/sqlite/repo.ts` (958 lines), `search-backend.ts`, `branch-cache.ts`, `migrations/001_initial.sql`, and `storage/{entries,records,lanes,branch-entries,branch-tips,facts,sessions,session-sequences,session-stats,writer-leases}.ts`. It is a first-class workspace member (`v0.84.1:package.json:7` `"packages/session-backends/*"`), built in the release chain (`:16`), published as `@earendil-works/pi-session-backend-sqlite-node`. It backs the harness session contract retreed in the same release (`packages/agent/src/harness/session`: 21 files, +3070/−1147, adding `seq`, `lane`, `RecordBase`, `OperationStartedRecord`, `retainedTail`, and a 993-line `testing/conformance.ts`).

**Impact** — An embedder cannot swap cyrup's JSONL store for a database-backed one, and none of what the SQLite backend buys — indexed full-text session search, per-session stats without a full re-parse, branch tip caching, and **writer leases that stop two processes owning one session** — is available. The lease gap is the sharpest: it is the same hazard `VL-S3` records for pi-subagents, and cyrup has no lease machinery on either path. Rated low because nothing in `packages/coding-agent/src` or `packages/agent/src` imports the package at v0.84.1 (`git grep -l` over both trees returns nothing) — it is an SDK-facing option today, not a path the pi CLI takes.

**Fix** — Scope decision first: this is a port of the v0.84.1 harness session contract plus one backend, not a bug fix. If taken, widen `crates/cyrup-session/src/store.rs`'s `SessionStore` to the repository shape the conformance suite pins and add a `cyrup-session-sqlite` crate behind a feature flag. If deferred, the deferral is recorded **here** rather than leaving the package unmentioned — before this pass the area file did not name `packages/session-backends` at all.

**Verify** — Port `packages/agent/src/harness/session/testing/conformance.ts` as a Rust integration suite and run it against both the JSONL store and any new backend; assert identical entry/branch/stat results.

## SESS-043 — The agent transcript is re-seeded from the LLM-flattened context, not pi's raw `AgentMessage` context, at all three sites pi assigns `agent.state.messages`

**Kind** parity-bug · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session-svc/src/session.rs:1542-1544` (manual compaction), `:1957-1960` (navigate_tree) and `:4358-4360` (auto compaction) all do `ctx.messages.iter().map(core_message_to_agent).collect()` where `ctx = guard.build_context()` — the `convertToLlm`-flattened `Vec<cyrup_core::Message>` (`manager.rs:689-720`; its doc at `:726-733` says `build_context` **is** the LLM boundary and names `build_context_raw` as the raw twin). `core_message_to_agent` (`cyrup-session-svc/src/event.rs:398-425`) can only produce User/Assistant/ToolResult, because `cyrup_agent::AgentMessage` (`crates/cyrup-agent/src/event.rs:15-30`) has no BashExecution / BranchSummary / CompactionSummary variants at all.

**upstream** — pi assigns the **raw** list at the three matching lines: `agent-session.ts:1874-1875`, `:2155-2156` and `:3067-3068`, each `this.agent.state.messages = sessionContext.messages`, where `buildSessionContext` (`session-manager.ts:460-468`) returns `buildContextEntries(...).flatMap(sessionEntryToContextMessages)` with every role intact.

**Impact** — An `excludeFromContext` (`!!`) bash message is present in pi's agent state and absent from cyrup's, so index arithmetic like pi's `messages.slice(0, -1)` (`agent-session.ts:2008`, `:2188`, `:2703`) operates on different lists; and every agent-state token estimate is taken over wrapper prose. Rated low because the LLM request itself is unaffected (the flattening would happen at the boundary anyway) and the estimate consequence is already tracked as SESS-028 — but **the SESS-028 fix is incomplete without this**, because switching that one call site still leaves the transcript basis flattened.

**Fix** — Give `cyrup_agent::AgentMessage` the missing variants (or a passthrough carrying the raw entry projection) and seed the transcript from `build_context_raw()` at `session.rs:1542`, `:1957` and `:4358`, mirroring the three pi assignments. The `M` effort is the enum widening, not the call sites.

**Verify** — Seed a branch containing an `excludeFromContext` bash entry and a compaction summary, run a compaction, and assert the resulting agent transcript length and roles match a pi capture of `agent.state.messages` at the same point.

## SESS-044 — `encode_cwd` strips EVERY leading path separator where pi strips exactly one, and both cyrup copies claim pi compatibility in their doc comments

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high (both sides read at v0.83.0; both cyrup copies read)

> **New this pass (2026-08-12 repair).** Routed here from the cross-area
> `migrations.ts` / `core/keybindings.ts` surface sweep, whose primary findings land in areas 05 and
> 07. Both sides were independently re-derived here before filing.

**cyrup** — Two independent copies of the encoder, diverging from pi the same way. (a)
`cyrup/crates/cyrup-session/src/layout.rs:97-105` `pub fn encode_cwd(cwd: &Path) -> String` opens
`let trimmed = raw.trim_start_matches(['/', '\\']);` — `str::trim_start_matches` removes **all**
leading matches, not one — and its doc comment at `:95` states "Pi-compatible encoding: **strip a
leading separator**, map `/` `\` `:` → `-`, wrap in `--…--`", which is what pi does and not what the
line below it does. It is the live encoder: `SessionLayout` calls it at `layout.rs:59`
(`self.root.join(encode_cwd(&self.cwd))`). (b) `cyrup/crates/cyrup/src/migrations.rs:160-173` is a
second, private copy of the same function with the same `trim_start_matches(['/', '\\'])` and a doc
comment making the same claim; it is called from `migrate_sessions_from_agent_root` at
`migrations.rs:143`. The migration's own test only covers single-separator inputs
(`migrations.rs:302` `--Users-x-proj--`, `:304` `--C--a-b--`), so it cannot catch this.

**upstream** — `pi/packages/coding-agent/src/migrations.ts:112` **at v0.83.0**:
`` const safePath = `--${cwd.replace(/^[/\\]/, "").replace(/[/\\:]/g, "-")}--`; ``. The first
`replace` uses `/^[/\\]/` — anchored, **no `g` flag** — so exactly one leading separator is removed;
only the second carries `g`. `pi/packages/coding-agent/src/core/session-manager.ts:479` is the
byte-identical expression, which is why pi's migration and its session manager agree with each other.
Both files are byte-identical at v0.83.0 and v0.84.1 (`git diff v0.83.0 v0.84.1 --` on both paths is
empty), so this is a baseline port bug, not drift. Worked divergences: for `//net/x` pi yields
`---net-x--` and cyrup `--net-x--`; for `\\srv\share\proj` pi yields `---srv-share-proj--` and cyrup
`--srv-share-proj--`.

**Impact** — Bounded, which is why this is `low` and not higher: cyrup's two copies agree with **each
other**, so no cyrup-written session is lost by cyrup. The costs are (a) a session tree written by pi
under a UNC path or a double-slash cwd resolves to a different directory name under cyrup and vice
versa, so the sessions are mutually invisible — the same class of silent listing miss as SESS-037's
empty-cwd header, reachable on Windows/SMB and anywhere a cwd arrives with a doubled leading
separator; and (b) an in-source comment asserting pi parity that is untrue, sitting in the file the
next auditor opens to decide the encoding is fine. That is exactly the self-certifying-comment class
`README.md:208-212` warns about, and it is why this was invisible for five passes.

**Fix** — In `layout.rs:97-105` replace `trim_start_matches(['/', '\\'])` with a single-separator
strip — `raw.strip_prefix('/').or_else(|| raw.strip_prefix('\\')).unwrap_or(&raw)` — and **delete**
the duplicate in `crates/cyrup/src/migrations.rs:160-173` in favour of `cyrup_session::encode_cwd`,
which is already `pub`. The duplication is what let two drift-free copies both be wrong; collapsing
to one definition is the durable half of this fix. Correct the doc comment at `layout.rs:95` either
way. Note the ordering hazard: changing the encoder changes directory names, so any session already
written under a multi-separator cwd becomes unreachable — either accept that (no such session can
exist unless the user ran cyrup from such a cwd) or add a one-shot fallback lookup on the old
encoding, and say which was chosen.

**Verify** — Extend `migrations.rs:300-305` `encodes_cwd_like_session_manager` with pi's exact outputs
for `//net/x` (`---net-x--`) and `\\srv\share` (`---srv-share--`), and add an assertion that
`cyrup_session::encode_cwd` and the migration's encoder return byte-identical strings for the same
input — a property test over generated paths is cheap here and would have caught the divergence when
the second copy was written.

## Coverage

**Read first-hand at cyrup HEAD `04c1ba2`** (repo HEAD `a9000b1`, docs-only; tree clean). Nothing was
executed — no cargo, no npm, no test run. Every `closed` verdict is a read of the Rust function body
at HEAD against the corresponding pi function opened **in full**, by two independent passes (an
auditor and a refuter instructed to default to rejection). A test cited as evidence is evidence of
intent, not of passing.

**Repair pass 2026-08-12 — what it covered, what it rejected, what is still blind.**

*Covered — the split-count fix (critique finding 17c).* `SESS-S05` moved from a second severity table
into `## Open items` and that table was deleted. This was the last instance in the directory of the
enumeration defect that cost `SEAM-S01` an entire audit pass; all twelve area files now carry one
table. Nothing about the item changed — same id, same severity, same body, same `partially-closed`
status row — only where it is enumerated. **It is counted**, and the header now says so explicitly,
because unlike area 02's partially-closed entries (whose residuals were re-filed under new ids and
are therefore counted elsewhere) SESS-S05's residual has no other home.

*Covered — the cross-tag citation sweep (critique finding 9).* The critique found a v0.84.1 offset
asserted as tag-invariant in area 02. Every cross-tag claim in **this** file was therefore re-run
against both tags rather than trusted, and all of them held:

| claim in this file | re-verified |
|---|---|
| SESS-035: `system-prompt.ts:74-76` / `:131-138`, `config.ts:427-439` "byte-identical at v0.83.0 and v0.84.1" | ✅ `git diff v0.83.0 v0.84.1 --` on both paths is **empty**; `:74-76` and `:131-138` re-read at v0.83.0 and carry the cited text |
| SESS-019: "`git diff v0.83.0..v0.84.1 -- system-prompt.ts` is empty, so `f4e9ca74`'s removal holds at v0.84.1" | ✅ empty |
| SESS-024: "`skills.ts` byte-unchanged v0.83.0→v0.84.1" | ✅ empty |
| header + Coverage: "`core/compaction/*`, `system-prompt.ts`, `skills.ts`, `session-cwd.ts`, `prompt-templates.ts` byte-unchanged" | ✅ re-run; unchanged |
| SESS-044 (new): `migrations.ts` and `session-manager.ts` byte-identical at both tags | ✅ empty on both paths |

So every upstream line number in this file is a **v0.83.0** line number, as `README.md:224-225`
requires, and the handful of items that deliberately cite v0.84.1 (SESS-014, SESS-038) say so in
their own text. No citation was "fixed by shifting"; each was re-resolved by opening the file at the
tag.

*Covered — the tracker test (critique finding 14).* Every open item was re-read against "does this
propose a change with a named fix site?". **SESS-038 alone fails it** and is now a `tracker`. The
nearest calls, both **kept as counted work**: SESS-034 (proposes either a collapse onto the session
flow or a widened `BeforeTreeDecision` — two concrete alternatives, not a decision to be taken
elsewhere) and SESS-030 (concrete field addition, cross-referenced to area 08's `SEAM-034`).

*Covered — a severity re-test against `README.md:106-107`.* The critique's finding 3 argues the
"zero criticals" headline survived only because the definition was not applied. It was applied here,
using the rule "meets one of the four conditions **and** the triggering path is one a user takes in
ordinary use". Three items were tested and **deliberately left where they are**, recorded so the
question is not re-opened blind: **SESS-004** ("silent data loss" in its own Impact, but bounded to
non-modelled keys written by a fork/newer cyrup/extension — not an ordinary path — stays `low`);
**SESS-042** (durable session mutation after a cancel, but latent until SESS-040 lands a caller, so
unreachable in the shipped binary today — stays `medium`, ships with SESS-040); **SESS-035** (the
model is never told where cyrup's docs live — a missing instruction degrading answer quality, not a
mechanism emitting wrong output — stays `medium`). **SESS-040 stays `high`**: it is a dead
affordance and a wasted provider call, not data loss.

*Rejected in this repair pass.* (a) **Raising SESS-004 or SESS-035 to critical** — rejected above.
(b) **Filing the `encode_cwd` divergence twice**, once here and once against area 05's copy in
`crates/cyrup/src/migrations.rs` — rejected: it is one defect with two code sites, so SESS-044 owns
both and names the migrations copy explicitly, and area 05 should cross-reference rather than
re-file. (c) **Re-numbering SESS-S05 to a plain `SESS-NNN` now that it is in the main table** —
rejected outright; ids are never renumbered, and the `-S` suffix is the provenance record that the
surface-driven sweep found it.

*Still blind after this pass.* The sweep verified **upstream** tag-invariance and the two sides of
SESS-044 only. It did not re-verify the cyrup-side line numbers of the other 28 items, which still
rest on the 2026-08-12 audit. The four pre-existing blind spots below are unchanged, and the
citation-refresh sweep noted under **Method residue** (cyrup's own in-source pi citations on the
branch-summary path being stale by ~200 lines) is still not filed as an item — SESS-044 is the same
class of defect caught in a doc comment rather than a citation, which is an argument for finally
filing it.

**cyrup files read.** `crates/cyrup-session/src/{entry,context,manager,listing,header,ids,store,migrate,agent_message}.rs`;
`src/compaction/{mod,settings,serialize,summarize,hooks,tokens,cutpoint,prepare,branch,files}.rs`;
`src/prompt/{builder,skills_inject,context_files}.rs` in full plus `prompt/tests.rs` regions;
`crates/cyrup-session-svc/src/{state,session,runtime,builder,factory}.rs` regions;
`crates/cyrup-core/src/message.rs` regions; `crates/cyrup-tui/src/{app,tree_selector,resume_hint,keymap,command}.rs` regions;
`crates/cyrup/src/{main,session_resolve}.rs` regions; `crates/cyrup-config/src/env.rs` (via the
SESS-036 refutation); tests `crates/cyrup-session/tests/{compaction,parity}.rs` at grep level and
`tests/listing_unparseable_message.rs` at existence level.

**Upstream read at the PORTED tag, not from the working tree.** The pi checkout on disk is `main` =
v0.84.1, so every upstream citation was taken from `git show v0.83.0:<path>` extracted to a
scratchpad: `core/{session-manager,system-prompt,skills,resource-loader,session-cwd,prompt-templates,messages,agent-session}.ts`,
`core/compaction/{compaction,branch-summarization,utils}.ts`, `modes/interactive/interactive-mode.ts`,
`modes/interactive/components/tree-selector.ts`, `main.ts`, `config.ts`, `utils/paths.ts`. Where a
v0.84.1 line is cited it is marked as such.

**Version-lag sweep (v0.83.0..v0.84.1), scoped to this area.** `git diff --stat v0.83.0..v0.84.1 --
packages/session-backends packages/coding-agent/src/core/{session-manager,session-cwd,system-prompt,prompt-templates,skills,resource-loader}.ts
packages/coding-agent/src/core/compaction` = **37 files / +4253 / −3**, and all of it except 8 lines
is the new `session-backends/sqlite-node` package (SESS-038). The whole of `core/compaction/`,
`system-prompt.ts`, `skills.ts`, `session-cwd.ts` and `prompt-templates.ts` is **byte-unchanged**
across the two tags — a clean result that materially narrows this area's drift exposure. The two
real code changes were both run to ground and are **not** findings: (a) `resource-loader.ts:71`
added `AGENTS.override.md` as the first candidate — already ported at `context_files.rs:80-81` with
a provenance note at `:70-75` citing `8ecf8a988`; (b) `session-manager.ts:1675-1677` widened
`listAll`'s directory filter to `entry.isDirectory() || entry.isSymbolicLink()` — cyrup is already
equivalent because `listing.rs:83` uses `Path::is_dir()`, which follows symlinks where Node's
lstat-based `Dirent.isDirectory()` does not.

**Surface-driven sweep.** Walked every `export` in `core/compaction/{compaction,branch-summarization,utils}.ts`
(37 symbols) and every public method of `class SessionManager` (46) against `rg` over `crates/`,
asking "what in cyrup consumes this?" rather than checking a list. Ports verified line-by-line and
found faithful — recorded here so the next pass does not re-derive them: `findCutPoint` /
`findValidCutPoints` / `findTurnStartIndex` / `isCutPointMessage` / `isTurnStartMessage` /
`isTurnStartEntry`; `estimateContextTokens` and its no-usage branch; `calculateContextTokens`;
`completeSummarization`; `createSummarizationOptions`; `generateSummaryWithUsage` prompt assembly
(`<conversation>` / `<previous-summary>` / `Additional focus:`); `prepareBranchEntries` including the
`fromHook` first-pass guard and the 0.9-budget force-include; `collectEntriesForBranchSummary`;
`extractFileOpsFromMessage` / `computeFileLists` / `formatFileOperations`;
`sessionEntryToContextMessages`; `assertValidSessionId`; `getSessionDir`'s `safePath` encoding
(**re-opened by the repair pass — `session-manager.ts:479` is where SESS-044's oracle lives, and the
original sweep read it as matching**); the `SessionHeader` / `NewSessionOptions`
shape; `_persist`'s deferred-flush rule (`manager.rs:423-440` matches `session-manager.ts:1015-1041`
including the resumed-but-no-assistant append branch); `createBranchedSession`'s deferred write;
`continueRecent`'s `filterCwd`; `forkFrom`. The sweep's yields were SESS-035, SESS-036 and SESS-037.
The cancellation cluster (SESS-040/041/042) came from a different lens — auditing the code that
*closed* SESS-023 rather than the item itself, which is the "closing a not-implemented item means
the subsystem exists, not that it is correct" rule paying off a second time.

**Rejected, with reason — do not re-derive.**
- **SESS-039** (proposed `test-defect` over `prompt/tests.rs:61-63` hand-building a `DocsPointers`
  no production caller produces). The facts check out, but the ledger's own vocabulary defines
  `test-defect` as "a test pinning wrong behavior, or asserting a timing/scheduling outcome it
  cannot control". This test pins *correct* behaviour of `SystemPromptBuilder::build`, a pure
  function whose entire contract is "given these inputs, emit this text"; unit-testing it with a
  populated input is normal. What is wrong is the production wiring — SESS-035. Filing both
  inflates the count. **Folded into SESS-035's Verify clause; the ID is burned.**
- `getLastAssistantUsage` (`compaction.ts:172-181`) has no cyrup counterpart, but it is exported
  only through `coding-agent/src/index.ts:46` with zero internal callers — an SDK-surface gap judged
  too thin to file.
- `DEFAULT_COMPACTION_SETTINGS` has no named cyrup equivalent; cyrup spells the same values as
  `Default` impls at `compaction/settings.rs:21-28`. Not a gap.
- Sort-order divergence in `computeFileLists`: pi sorts with JS `.sort()` (UTF-16 code units),
  cyrup with `BTreeSet<String>` (UTF-8 bytes). These differ for non-ASCII paths. Judged below the
  filing bar; **not confirmed unreachable**, so it is a candidate if anyone hits a path-ordering
  diff.
- `compute_max_tokens_frac` (`summarize.rs:307-313`) applies a `.max(1)` floor pi does not have
  (`compaction.ts:622-625` can yield 0). Only reachable with `reserve_tokens == 0`. Not filed.
- pi's `BranchPreparation.totalTokens` (`branch-summarization.ts:51-58`) has no cyrup counterpart
  (`branch.rs:165-168`); no upstream reader was found, so not filed — but pi was not exhaustively
  grepped for one.

**Corrections applied to the auditor's own claims** (recorded so the pattern is visible):
SESS-S05's "render half CLOSED" was **cut** — pi composes the timestamp *inline* between `[label] `
and the entry text (`tree-selector.ts:739-748`), while cyrup emits a right-aligned trailing column
(`tree_selector.rs:738-752`), and `formatLabelTimestamp`'s three formats (`:854-878`) have no cyrup
counterpart at all; only the gate and the `show_time: false` default are done, so wiring a producer
will **not** reproduce pi's row. SESS-036 was cut from medium to low for a wrong mechanism (see the
item). Several inherited pi line citations in the SESS-023 evidence match neither tag and were
replaced with v0.83.0 lines; the substance was unaffected.

**Blind spots — what the next pass should attack first.**
- `crates/cyrup-session/src/migrate.rs` was flagged by the auditor as the one file most likely to
  hold an unfiled defect. The refuter then read it in full for SESS-015 and found
  `convert_first_kept_index` behaviourally equivalent to pi across idx 0, out-of-range,
  forward-reference, negative and float indices. **`migrateV2ToV3` and the rest of the v1/v2 legacy
  paths were still not walked statement-by-statement.**
- Session-file locking under concurrent writers, and the torn-tail repair `VL-P22` describes
  (`manager.rs:114-117` gating the rewrite on `migrated && !recovered`). The gate still reads that
  way; the item is owned by PARITY-GAPS and was not re-derived here. SESS-038's writer-lease gap is
  the same hazard from the upstream side.
- `crates/cyrup-session/tests/{compaction,parity,sessions}.rs` (~4529 lines) were grepped for the
  SESS-025/SESS-032 shapes, not read. The 2026-08-03 test-defect hunt was **not** re-run from
  scratch, so tests added since `1806375` have had no test-defect pass. The class is not exhausted.
- Nothing in the TUI's compaction-indicator lifecycle beyond the two arms cited in SESS-040 was
  audited; if the Escape rebind is missing, the restore path and the interaction with
  `branch_summary_in_flight` deserve the same scrutiny.

**Claims that could not be personally proved.**
- SESS-036's blast radius is reasoned from `std::fs::canonicalize` vs `path.resolve` semantics, not
  reproduced. The code difference is certain; the reachability now depends on a non-`getcwd` cwd
  source, which was traced but not executed.
- SESS-037's CLI reachability chain (`match_session_arg` → `SessionLookup::Path` → `Resume` →
  `open_with_cwd(path, None)`) was traced by reading four files, not executed. `cwd_override` is
  `Some` on the factory path (`factory.rs:126`), so the exposure is specifically the FIRST build
  from `main.rs`.
- SESS-013's doubled-load impact still needs a real `git worktree add`; unchanged from the prior
  pass.
- SESS-040/041/042 are read-and-grep verdicts. Two of them (041, 042) are latent until 040 lands a
  caller, so no runtime observation is possible today even in principle.

**Method residue.** `spec/` is still absent from this workspace, so `R-05-016` / `R-05-018` /
`R-06-017` remain grep anchors only. This matters most for SESS-017 and SESS-022, where cyrup's own
doc comments invoke spec text to justify behaviour contradicting pi; those verdicts rest on pi's
code plus the fact that cyrup's two branch-summary paths contradict **each other**. Separately,
cyrup's inline pi citations on the branch-summary path are still stale by ~200 lines (`mod.rs:356`
cites agent-session.ts:2787 for a gate now at `:2983`; `mod.rs:360` and `branch.rs:117`/`:121` cite
branch-summarization.ts:315-317 for code at `:312-313`) — worth a citation-refresh sweep, still not
filed as an item.

---

## Surface-sweep findings (2026-08-03, HEAD `9219dcd`) — provenance only, NO second table

Found by a **surface-driven** sweep that walked pi asking what has NO cyrup counterpart at all,
rather than checking a list of known items. IDs use an `-SNN` suffix to mark their provenance.
**Five of the six closed on 2026-08-12** (SESS-S01, S02, S03, S04, S06 — evidence in the status
table above). One remains open: `SESS-S05`, whose body follows.

> **This section no longer carries a severity table.** Until the 2026-08-12 repair pass `SESS-S05`
> lived in a second table here, which made this area the last place in the directory where the open
> count could not be read off one heading — the miss recorded as structural defect A in
> `00-residual-ledger.md` and flagged unfixed in `README.md:196-199`. `SESS-S05` is now a row in the
> single `## Open items` table above and the second table is deleted; only this heading, this
> provenance note and the item body remain. A future surface sweep files its findings **into the
> main table** with an `-SNN` id, never into a table of their own.

## SESS-S05 — `TreeNode` drops pi's `labelTimestamp`; the ported `t` toggle now switches an always-empty column, and the column itself is the wrong shape

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** high · **Status** partially-closed

> **Refuter caveat, stated inline as required.** The audit reported the render half **closed**. It
> is not. Only two narrow sub-claims hold: the gate at `tree_selector.rs:741-744`
> (`show_time && node.has_label && Some(t)`) and the corrected default `show_time: false` at `:375`
> (pi's `showLabelTimestamps = false`, `tree-selector.ts:116`). What remains is structural — see
> **cyrup** below.

**cyrup** — Data half open: `cyrup-tui/src/app.rs:5570` sets `time_label: None` with a 22-line
comment at `:5553-5569` conceding that `cyrup_session::TreeNode` (`manager.rs:29-34`) still has no
timestamp field, even though `SessionManager::labels` holds `(label, ts)` (`manager.rs:44-45`) and
`label()` discards the second element. Render shape also open: `tree_selector.rs:738-752` emits the
timestamp as a **right-aligned trailing column** (`pad = width − (left_len + right_width + 1)`, then
a padded span). `formatLabelTimestamp`'s three formats have no cyrup counterpart anywhere.

**upstream** — `pi/.../components/tree-selector.ts:739-748` composes the timestamp **inline**:
`body = prefixPart + label + labelTimestamp + content`, i.e. between `[label] ` and the entry text.
pi has no right-hand column anywhere in that render. `formatLabelTimestamp` (`:854-878`) renders
`HH:MM` today, `M/D HH:MM` this year, else with a 2-digit year. Source data:
`session-manager.ts:159-167` `SessionTreeNode { entry; children; label?; labelTimestamp? }`.

**Impact** — `t` now toggles an always-empty column instead of rendering the literal string
`"current"`, which is a strict improvement (the previous nonsense output is gone and the default
matches pi). But the feature is still absent: a user cannot see when a bookmark was set, which is
the entire point. **Wiring a producer will not finish this** — the row would then differ from pi's
in position and format.

**Fix** — Three pieces, in order: (1) add `label_timestamp: Option<String>` to
`cyrup_session::TreeNode` (`manager.rs:29-34`) and populate it from `labels` at the build site;
(2) produce it at `app.rs:5570`; (3) rewrite `tree_selector.rs:738-752` to compose inline after the
label rather than as a right column, and port `formatLabelTimestamp`'s three-branch format from
`tree-selector.ts:854-878`.

**Verify** — Render a tree with a label applied at a known timestamp and byte-compare the row
against a pi capture — position and format, not merely the presence of a non-empty string.
