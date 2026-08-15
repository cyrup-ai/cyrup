# 04 — cyrup-tools (the built-in tool set)

This area covers `crates/cyrup-tools` — the seven built-in tools (`read`, `write`, `edit`, `bash`, `grep`, `find`, `ls`), their registry, the `ops` filesystem/process/shell seam, the glob matchers, the file-mutation lock and the `isolation/` decorators — plus the places tool metadata crosses into `cyrup-core`, `cyrup-ext`, `cyrup-tui` and `cyrup-session-svc`, which are the tool *surface* and are routed explicitly per item. It is measured against `pi/packages/coding-agent/src/core/tools/` at **pi v0.84.1** (the coding-agent copy is pi's live path; the thinner `packages/agent/src/harness/tools/` fork is not the reference). Headline: **both prior highs are gone** — `write`/`edit` now write in place (TOOL-004) and `bash` now injects and scrubs the session environment (TOOL-008) — and the entire 2026-08-03 surface sweep (`TOOL-S01`…`TOOL-S06`) is closed. **A new high replaced them**, from the same first-ever audit of `ops/shell.rs`: **TOOL-039**, the ambient `CYRUP_SHELL` override with no pi analogue, which sits ahead of `/bin/bash` in the shell resolution every model-issued command goes through, is unscrubbed so it propagates into subagent re-execs, and is invisible in the transcript. It ships with **TOOL-007** as one shell-surface decision. The rest is a different shape of debt: two streaming/limit defects in `grep` that mirror the known one in `find`, a guest-tool metadata surface that is still dead end-to-end, a silent `cmd.exe` substitution on Windows, and five tests that still assert timing or nothing at all.

> **Re-audited 2026-08-12, cyrup HEAD `04c1ba2`** (working tree clean; `a9000b1` is docs-only on top of it), against **pi v0.84.1**, ~40 commits past the `1806375`/`9219dcd` baseline this file was last written at. Every item below was re-read at source on both sides rather than trusted from the prior text.
>
> **This pass: 14 items closed, 4 partially closed, 0 reopened, 11 newly filed.**
> Closed: TOOL-004, TOOL-008, TOOL-009, TOOL-010, TOOL-012, TOOL-013, TOOL-027, TOOL-028 and all six surface-sweep items TOOL-S01…TOOL-S06.
> Partially closed (still listed open, with the residual named): TOOL-015, TOOL-017, TOOL-019, TOOL-025.
> New: TOOL-031…TOOL-041. Four of those (TOOL-038…TOOL-041) came from the *refuter*, not the auditor — `crates/cyrup-tools/src/ops/shell.rs` and `write`'s cancellation bracketing were absent from the audit's own coverage list, which is exactly the structural blind spot the README's surface-driven sweep exists to counter.
> Two auditor severities were **corrected downward** by the refuter and are written here at the corrected value with the caveat stated inline: TOOL-024 (medium→low), TOOL-031 (medium→low), TOOL-032 (medium→low), TOOL-037 (medium→low). One item's *upstream mechanism* was refuted and rewritten while the substantive gap survived: TOOL-021.
>
> **Version lag: none filed for this area.** `git diff --stat v0.83.0..v0.84.1 -- packages/coding-agent/src/core/tools/` is 7 files / +68 −35 and the whole diff was read. Four changes; three are already carried at HEAD and the fourth is a no-op for cyrup — details in `## Coverage`. The other three upstreams (`pi-subagents` v0.47.1, `pi-permission-system` v0.8.0, `pi-intercom` v0.10.1) do not touch this area.
>
> **REPAIR PASS 2026-08-12 (completeness critique, findings 5 / 9 / 14).** Three changes, no item
> renumbered, merged or deleted. (1) **TOOL-039 raised low → high**, and cross-linked to **TOOL-007**
> as one shell-surface decision that must be taken once. `CYRUP_SHELL` is an ambient-authority
> override with **no upstream analogue**, it sits ahead of `/bin/bash` in the `ShellConfig::detect()`
> that both `ToolRegistry::with_builtins` and `Backend::default()` use, and it is absent from
> `session_env_scrub_keys()` so it propagates into every nested re-exec including the subagent
> runner. Any process in the environment chain silently substitutes the interpreter for every command
> the model runs, with nothing in the transcript recording which shell executed. All five limbs were
> independently re-verified this pass. It had been rated below "`get_tree` drops pi's
> `labelTimestamp`". (2) **The cross-tag citation sweep** the critique demanded (finding 9) was run
> over this file and produced a real correction: **TOOL-036's Windows shell-path half is v0.84.1
> drift, not a baseline parity bug** — `normalizeWindowsShellPath` does not exist at v0.83.0. Six
> more upstream files shift between the tags; the offset table is in `## Coverage`. (3) Every item
> was tested against finding 14's tracker rule; **none in this area proposes a decision instead of
> work**, so nothing was reclassified — recorded so the check is not re-run blind.
>
> **Open count is a floor, not a total.** 29 open here (0 critical, **1 high**, 10 medium, 18 low),
> 0 trackers.

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
> **Area 04 — recount: 29 rows → 6 open (0 critical · 0 high · 1 medium · 5 low).** The area's only
> high (`TOOL-039`) closed in sweep 1 by deleting the `CYRUP_SHELL` arm outright — option (i), the
> item's own default recommendation — which also settles `TOOL-007` and `TOOL-038` and invalidates
> handoff (g): they were one shell-surface decision and it has been taken.
>
> **Sweep 2 changed no code in this crate, and that is the correct outcome.** Every remaining id has
> its fix site outside `crates/cyrup-tools/**`: `TOOL-015`/`TOOL-022` are cluster C5 (`render_kind`
> needs a consumer), `TOOL-016` and `TOOL-017` are cross-crate, `TOOL-024` is
> `crates/cyrup-ext/src/wrapper.rs:183`/`:333` (verified at HEAD — **it should be routed to area 06
> rather than left in this table as if a tools agent could take it**), and `TOOL-031`'s residual is
> the subagent re-exec half of PARITY-GAPS PB-5.
>
> **`TOOL-021` closed on the back of a refutation in another area.** Sweep 2's `EXT-007` work
> established that the claimed blocker was stale: `Tool::prompt_guidelines` already returns
> `Vec<&str>` (`crates/cyrup-core/src/tool.rs:130`) and `impl Tool for WasmTool` already overrides it
> (`cyrup-ext/src/host/live.rs:1690`), with an in-source note recording that TOOL-021/EXT-007
> unblocked it. Three separate places in the record were still planning around that blocker.
>
> **STRUCTURAL:** every path of the form `crates/cyrup-tools/tests/<x>.rs` in this file is stale —
> only `tests/bash_env_scrub.rs` and `tests/shell_interpreter.rs` remain out-of-crate. Affects
> `TOOL-004`, `-007`, `-008`, `-012`, `-019`, `-024`, `-025`, `-026` and the Coverage section. Add
> `crates/cyrup-tools/src/tests/pi_tool_semantics.rs` to the Coverage test list — it carries sweep 1's
> regressions for `TOOL-006`/`011`/`014`/`018`/`023`/`029`/`033`/`034`/`041` and the `render_kind`
> assertion. Blind spot 3 is updated by `TOOL-036`'s Windows-scope answer (ADR-0007) and blind spot 4
> is cleared by `TOOL-035`.


## Status since the 1806375 / 9219dcd baseline

| ID | Status | Note |
|---|---|---|
| TOOL-001 | closed | Re-attacked a third time. `crates/cyrup-tools/tests/pi_schema.rs:139-215` asserts `description()`/`prompt_snippet()`/`prompt_guidelines()` for all seven **through the `Arc<dyn Tool>` vtable** against verbatim pi constants (`pi_schema.rs:95-136`). Six of seven byte-compare against v0.84.1 directly; the seventh is covered by the whole-diff read (no description literal changed v0.83.0→v0.84.1). Reachability re-verified at the real site: `crates/cyrup-agent/src/agent.rs:714` `description: t.description().to_string()` inside the `ToolDef` build — the older `:656` cite is stale. |
| TOOL-002 | closed | `EditTool::prepare_arguments` at `crates/cyrup-tools/src/tools/edit.rs:132-134` → `normalize_args` (`:83-109`). Re-attacked at the *semantic* level this pass, not just presence: pi appends the legacy pair to a **copy** of any existing `edits` array (`edit.ts:124-125`) and strips `oldText`/`newText` via `{...rest, edits}` (`:126-127`); `edit.rs:95-106` does exactly that, pinned by four unit tests at `edit.rs:249-328`. |
| TOOL-003 | closed | `prompt_snippet` (`crates/cyrup-core/src/tool.rs:112-114`) and `prompt_guidelines` (`:120-122`) on the vtable; all seven built-ins override them (`read.rs:67-72`, `write.rs:65-70`, `edit.rs:143-159`, `bash.rs:88-126`, `grep.rs:105-107`, `find.rs:86-88`, `ls.rs:61-63`). The one hole this item recorded — bash's guideline — is closed by TOOL-008 (`bash.rs:120-126`, gated on `expose_session_environment` == `bash.ts:334`). The `&[&str]` return type remains the guest-side defect, tracked as TOOL-021. |
| TOOL-004 | **closed this pass** | `cfe351e`. `FsOps::write_in_place` declared at `crates/cyrup-tools/src/ops/mod.rs:299` (contract doc `:279-298`), implemented at `ops/local.rs:73-96` (`OpenOptions::write(true).create(true).truncate(true)` — no temp file, no rename), called from `write.rs:89` and `edit.rs:221`. `write_atomic` is gone from the mutator path. Upstream is `edit.ts:95` / `write.ts:223`. Proven by `crates/cyrup-tools/tests/write_semantics.rs:76`, `:105`, `:132`, `:165`, `:197`, `:232`, `:314`. |
| TOOL-005 | closed | `crates/cyrup-tools/src/tools/grep.rs:222-225` `BinaryDetection::quit(b'\x00')`, with the `convert`-vs-`quit` `[CYRUP-DELTA]` at `:208-221`. Upstream `grep.ts:220-223` builds rg's argv with no `--text`/`-a`, so rg's default quit-on-NUL applies. |
| TOOL-006 | still-open | `write`/`edit` still declare `ExecMode::Sequential`; the whole batch is still routed serially at `agent.rs:888`. |
| TOOL-007 | still-open | `protect_paths: true` still hardcoded at `builder.rs:208`, still bypassed by `bash` (`:646` leaves `proc` undecorated), still no flag or setting, and `isolation/mod.rs:3-6` still asserts the opposite of the wiring. **Cross-linked to TOOL-039 by the 2026-08-12 repair pass** — both are decisions about what `bash` is allowed to be, and taking them separately produces an incoherent shell surface. |
| TOOL-008 | **closed this pass** | `cfe351e`. Injection at `bash.rs:152-179` from a live `SessionEnvHandle` read at spawn time; scrub at `config.rs:41-48` set **unconditionally** at `bash.rs:153` (pi deletes at `bash.ts:171-175` *before* consulting the flag at `:176`); default `true` at `config.rs:229` == `bash.ts:327`; guideline at `bash.rs:120-126` == `bash.ts:334`; wiring at `builder.rs:660-670`; tests `bash_session_env.rs`, `bash_env_scrub.rs`. All three wrong comments fixed — `pi_schema.rs:114-123` now states the default correctly and pins the non-empty guideline. |
| TOOL-009 | **closed this pass** | `ModelVisionHandle` (`config.rs:119-138`), `ReadOpts.model_vision` (`:151`), `supports_images_now()` (`:169-171`) consulted **per execution** at `read.rs:257` == pi's per-call `getNonVisionImageNote(ctx?.model)`. Production wiring is real: `builder.rs:658-659` seeds from `resolved_model.supports_image_input()` and re-pushes on `/model`; threaded at `:676-677`. Test `read_model_vision.rs:54`, `:112`. |
| TOOL-010 | **closed this pass** | `RgGlob` (`globmatch.rs:73-152`) ports `GitignoreBuilder::add_line`: `!` sets `negated` (`:105-108`), `keeps_file` is `hit != self.negated` (`:146-151`); compiled at `grep.rs:149-152`, applied at `:188-191`. Pinned by `globmatch.rs:214-219`. |
| TOOL-011 | still-open | `find` still matches path-globs against the search-root-relative path. Must NOT be swept in with grep's rule, which is now correctly split out (TOOL-027). |
| TOOL-012 | **closed this pass** | `registry.rs:20` `BUILTIN_NAMES = ["read","bash","edit","write","grep","find","ls"]` with the inserts at `:62-78` in that order == `index.ts:156-166`; derived sets match too (`registry.rs:121-126` == `index.ts:138-145`; `:129-134` == `:147-154`). Guarded by `tests/builtin_tool_order.rs:30/:46/:58/:69`. |
| TOOL-013 | **closed this pass** | `read.rs:110` is the bare `self.fs.access(&abs, Access::Read).await?;` with the rationale at `:100-109`; the propagated error is built at `ops/local.rs:119` from the real `access(2)` at `:117`. Test `read_access_errno.rs:43` and `:64`. |
| TOOL-014 | still-open | `edit`'s access-failure body still interpolates the whole `ToolError`; the wrong comment survives verbatim at `edit.rs:192-193`. TOOL-013's closure supplies **no** reusable errno mapping — it propagates `io::Error` Display rather than naming the errno — so this needs its own. |
| TOOL-015 | **partially-closed** | The user-visible half is closed: `crates/cyrup-tui/src/transcript.rs:1802-1840` renders `edit` with pi's own component shape (preview → settled `details.diff` **replacing** it, plus `edit_header_preview` porting `getEditHeaderBg`), so the redundant outer frame is gone. The mechanism half is not: `EditTool` still overrides no `render_kind`, and zero sites in `crates/` branch on the value. The residual is exactly TOOL-022. |
| TOOL-016 | still-open | `constrainedSampling` still unrepresented in `cyrup_core::Tool` and in the WIT `tool-descriptor`; upstream still plumbs it at `tool-definition-wrapper.ts:14`/`:42`. |
| TOOL-017 | **partially-closed** | Two of pi's three arms are ported (`transcript.rs:2265-2305` + the compact formatter below it, selected at `:1701`). The v0.84.1 addition is already carried: `transcript.rs:2235` lists `AGENTS.override.md` first, byte-matching the widened `read.ts:42`. Residual = PARITY-GAPS **PB-4**: the `docs` arm is absent, documented as absent in-source at `transcript.rs:2258-2264`, and blocked on deciding what cyrup's shipped-docs root is. |
| TOOL-018 | still-open | Empty-normalized-needle divergence unchanged at `edit_diff.rs:298`/`:310-312`, still with no `[CYRUP-DELTA]`. |
| TOOL-019 | **partially-closed** | Primary closed by `7fd0d9c`: the map is now a process-global `static LazyLock` (`lock.rs:29-30`, attached at `:78-80`, with `Default` hand-written at `:44-51` so a derived `Default` cannot silently re-create per-owner domains), revert-proved at `lock.rs:164-187` and by `tests/cross_registry_mutation_lock.rs:144/:208/:262`. Secondary survives: `key()` still calls the **blocking** `std::fs::canonicalize` from inside the async `guard()`. That residual is written up in full as TOOL-032 — **it is one fix, not two.** |
| TOOL-020 | still-open | Forced-SIGKILL drain test unchanged at `ops/local.rs:992-1060`; still asserts scheduling and shell-buffering outcomes. |
| TOOL-021 | still-open, **evidence corrected** | The gap survives; the auditor's upstream mechanism did not. `tool-definition-wrapper.ts` does **not** copy `promptGuidelines` — pi reads them off the definition registry in `agent-session.ts`. Rewritten below on the corrected evidence. Severity medium stands. |
| TOOL-022 | still-open | WIT `tool-descriptor` is still the 8-field record (`world.wit:39-48`); `WasmTool` still overrides none of `label`/`prompt_guidelines`/`render_kind`/`prepare_arguments`. |
| TOOL-023 | still-open | `find` still drains the whole walk then sorts and truncates. Now has a sibling in `grep` (TOOL-033) — land them together. |
| TOOL-024 | still-open, **severity corrected medium→low** | The vacuity is exact and unchanged (9 of 11 assertions compare a default against itself). Downgraded because a vacuous assertion has no user-visible consequence, which is the README's severity axis, and because the sibling test-defect TOOL-025 was rated low on identical reasoning. |
| TOOL-025 | **partially-closed** | The vacuity is repaired — the racing payloads now differ in length (`tests/tools.rs:395`, `:407`) and the rationale is recorded at `:380-388`; the two requested cases landed as separate files (`cross_registry_mutation_lock.rs:144/:208/:262`). Residual: the test still names "serializes" while observing no ordering, and its guarantee is probabilistic rather than structural. |
| TOOL-026 | still-open | `bash_timeout_fractional_seconds` wall-clock ceiling unchanged at `tests/tools.rs:785-789`. |
| TOOL-027 | **closed this pass** | The two glob rules are now separate types: `PatternMatcher` (fd's rule, `globmatch.rs:20-53`) is used by `find` only, `RgGlob` (ripgrep's rule, `:73-152`) by `grep` only, and `RgGlob` prepends `**/` only when the pattern contains **no** `/` (`:125-127`) — the exact inverse. Proven by `globmatch.rs:178-188` and end-to-end at `grep.rs:523`/`:540`. Leading-`/` anchoring fixed in the same move. |
| TOOL-028 | **closed this pass** | `BashSpawnContext.env_remove` (`config.rs:20-26`, rationale `:15-19`), carried through `ExecSpec` (`ops/mod.rs:218-225`, ordering contract `:213-217`) and applied at `ops/local.rs:253-255` **before** the overrides at `:256-258` — matching pi's delete-then-repopulate order (`bash.ts:170-186`). The hook receives and can extend it (`bash.rs:179-183`). |
| TOOL-029 | still-open | `ls.rs:86` still propagates the raw `FsOps::read_dir` error; its two siblings in the same function are pi-shaped. |
| TOOL-030 | still-open | `exec_pre_cancelled_never_spawns` still carries the 200ms ceiling (`ops/local.rs:827-831`). |
| TOOL-S01 | **closed this pass** | `crates/cyrup-tools/src/jsnum.rs` adds `to_integer` (ECMA-262 §7.1.5) and `to_count`; every numeric param now deserializes as `f64` (`read.rs:21-22`, `grep.rs:29-30`, `find.rs:24`, `ls.rs:19`, `bash.rs:24`) and every clamp matches upstream one-for-one. |
| TOOL-S02 | **closed this pass** | `crates/cyrup-tui/src/ansi.rs:20-27` `sanitize_display_text` = `sanitize_binary_output(&strip_ansi(text)).replace('\r', "")` == `render-utils.ts:48`, with the two halves ported at `:37` and `:61`, tests at `:238-300` and a fuzz guard at `:552`. A second copy covers the immediate-bash seam (`cyrup-session-svc/src/bash.rs:293-330`). |
| TOOL-S03 | **closed this pass** | `edit_diff.rs:545-576` `compute_edits_diff`, consumed by the TUI's pre-execution preview (`transcript.rs:222`, `:729`, `:1802-1830`) with pi's replace-not-append dedup. Tests `edit_preview_diff.rs:45`, `:83`. |
| TOOL-S04 | **closed this pass** | `ReadOpts.auto_resize_images` (`config.rs:154-163`, default true at `:183`), threaded at `read.rs:265-270`, false-branch at `read.rs:396-408` returns normalized original bytes with no resize and no dimension note. Wired at `builder.rs:681` from `settings.rs:787`. Deliberately static rather than a live handle, with the pi justification recorded at `config.rs:159-162`. |
| TOOL-S05 | **closed this pass** | `transcript.rs:1963-1979` ports `bash.ts:309-313` (`Elapsed` from call start, flipping to `Took` on settle); the per-second repaint predicate is documented at `transcript.rs:711`. |
| TOOL-S06 | **closed this pass** | `grep.rs:297-303` emits one `(unable to read file)` marker row per match when the format-pass re-read fails == `grep.ts:258`. Pinned by `grep.rs:445-470` and, for the other half of pi's condition, `grep.rs:475-498`. |
| TOOL-031 | new | `bash` stamps no agent-identity variable into the child env. |
| TOOL-032 | new | `FileMutationLocks::key` blocks a runtime thread on `std::fs::canonicalize`. Same fix as TOOL-019's residual. |
| TOOL-033 | new | `grep` completes the whole walk before searching; `limit` bounds neither cost nor selection. |
| TOOL-034 | new | `grep` materializes every candidate file in memory, twice on the context path. |
| TOOL-035 | new | `read`'s image sniff scans the whole file; pi sniffs 4100 bytes. |
| TOOL-036 | new | `resolve_to_cwd` omits the whole win32 leg of pi's `normalizePath`. |
| TOOL-037 | new | `output-guard`'s protocol-writer half is unported. |
| TOOL-038 | new (refuter) | On Windows with no bash, `bash` silently falls back to `cmd.exe /C`; pi refuses to run. |
| TOOL-039 | new (refuter) | **high** *(raised from low in the 2026-08-12 repair pass)*. `CYRUP_SHELL` silently redirects every `bash` call; pi has no shell env var. One shell-surface decision with TOOL-007. |
| TOOL-040 | new (refuter) | `find_bash_on_path` runs `which bash` with no timeout; pi bounds it at 5s. |
| TOOL-041 | new (refuter) | `write` never re-checks cancellation after the write lands. |

Fourteen items closed, four partially closed, nothing overturned, no previously-closed item reopened. TOOL-031 through TOOL-041 are new this pass.

## Open items

> **⚠ THIS TABLE IS NOW THE COMPLETE OPEN SET FOR THIS AREA.** The separate `-S` table under
> `## Surface-sweep findings` is retained for traceability but every item in it is **closed** as of
> this pass, so it no longer adds to the count. Do not delete it: the `-S` ids are load-bearing and
> a closure must remain re-auditable. See structural defect A in `00-residual-ledger.md`.

> **RECOUNTED 2026-08-14 (sweeps 3-6 reconciliation) — counted set: 0 critical, 0 high, 1 medium, 4 low = 5.** 25 rows are now marked CLOSED (`TOOL-016` closed by sweep 6 under `PROV-011`; `TOOL-042` filed and closed in sweep 6). The `-S` table below remains fully closed. *(Previous edition: 0 / 0 / 1 / 5 = 6, 23 closed.)*

> **RECOUNTED 2026-08-14 (sweeps 7-8 reconciliation, third edition) — counted set: 0 critical, 0 high, 2 medium, 3 low = 5, unchanged in total but NOT in composition.** The table now carries **31 rows: 26 fully closed, 5 open (2 of them partially closed, including the reopened `TOOL-042`)**. Sweep 8: **`TOOL-024` closes as already-done** (all 13 `Tool` methods delegated at HEAD; its stated evidence was itself wrong — see the row); **`TOOL-042` is REOPENED as PARTIALLY CLOSED (medium)** after 286 measured runs refuted the mechanism its closure rested on; and **`TOOL-M01` is filed and closed in the same pass** (the `FsOps` decorator audit's one real residual). `TOOL-022`/`TOOL-015`'s fix site is corrected below — it was incomplete in a way that would have produced a no-op fix. *(Previous edition: 0 / 0 / 1 / 4 = 5, 26 closed.)*

> **RECOUNTED 2026-08-14 (sweep 9, fourth edition) — counted set: 0 critical, 0 high, 2 medium, 6 low = 8.** The table now carries **34 rows: 26 fully closed, 8 open (3 of them partially closed)**. *(Previous edition: 0 / 0 / 2 / 3 = 5, 26 closed, 31 rows.)* Three ids filed — **`TOOL-043`, `TOOL-044`, `TOOL-045`** — from a mechanical enumeration of the built-in tool surface (names, parameter schemas, result shapes) on both sides.
>
> **The result on this surface is mostly a clean bill, and that is worth recording as strongly as the three findings.** 7/7 tools walked in both directions with nothing sampled: **zero** `missingInCyrup` findings, names and order identical, all 7 descriptions / `promptSnippet`s / schemas byte-matching, all 6 `details` structs matching field-for-field, result envelopes matching. Details are in the section preamble under `## Findings filed 2026-08-14`.
>
> **A fourth finding from the same sweep was fixed inline and committed before this filing and is deliberately NOT re-filed here** — `additionalProperties: false` on `edit`'s parameter schema, which no pi built-in emits at all. It matters as precedent rather than as a bug: the in-source comment asserted `edit` "is the ONLY tool whose source passes `{ additionalProperties: false }`" and the test file's own `PI_EDIT` constant was documented as "the EXACT output ... ground truth, not a paraphrase" — **both false, so the test suite was certifying the divergence.** A ground-truth constant that was never mechanically re-derived is indistinguishable from an assertion of the current behaviour.
>
> **`TOOL-044`'s residual needs a DECISION, not an agent.** Two of its three limbs landed; the third (pi duplicates the whole truncated output into `details.truncation.content`) is a size-vs-literal-fidelity tradeoff with no reachable consumer on either side. Route it to whoever can say "port it" or "tag it `[CYRUP-DELTA]`" — scheduling it as work will produce another pass that re-derives the same tradeoff.
>
> **The partitioning note above needs one amendment.** All three new rows have fix sites INSIDE `crates/cyrup-tools/**` (`tools/bash.rs`, `truncate.rs`, all seven `tools/*.rs`), so "area 04 is finished as a crate" is no longer true. It was true of the rows that existed when it was written; it was never a property of the crate.

> **⚠ PARTITIONING NOTE — 2026-08-14 (sweep 6). AREA 04 IS FINISHED AS A CRATE: after sweep 6 re-verified every remaining row at HEAD, NOT ONE open row has a fix site inside `crates/cyrup-tools/**`.** `TOOL-015` and `TOOL-022` need a **consumer** in `crates/cyrup-tui` (plus `cyrup-core`); `TOOL-017` needs `crates/cyrup-tui` **and a product decision** (cyrup has no decided shipped-docs root for `getPiDocsClassification` to resolve against — the blocker is recorded in-source at `cyrup-tui/src/transcript.rs:2305`); `TOOL-024`'s two fix sites are both `crates/cyrup-ext/src/wrapper.rs`; `TOOL-031`'s residual is `crates/cyrup-ext-subagents/**`. Every row below now names its fix site. **Do not schedule a "finish area 04" assignment against `crates/cyrup-tools`** — sweep 6 was the second sweep to be routed at a `cyrup-ext` defect from this table, and it produced an agent with no reachable work in its own crate. Route these rows by FIX SITE, not by area number.

> **AMENDED 2026-08-14 (sweep 8): the "not one open row has a fix site inside `crates/cyrup-tools/**`" claim now has exactly one exception — `TOOL-042`, reopened.** Its residual is a harness-level question about `cargo nextest`'s 500 ms leak tripwire and, secondarily, `crates/cyrup-tools`' own `exec`/`exec_argv` fixtures (`ops/local.rs:1135`, `:1568`, `:1812`). Everything else in the partitioning note stands.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| ~~TOOL-M01~~ | ~~low~~ **FILED AND CLOSED 2026-08-14** | test-defect | S | Both `FsOps` decorators forwarded `detect_image_mime` but no test could see it — deleting either forward was silent — **FILED AND CLOSED 2026-08-14**: sweep 8, the one real residual of the assigned `FsOps` decorator audit. See the body below. |
| ~~TOOL-039~~ | ~~**high**~~ **CLOSED 2026-08-14** | cyrup-original | S | `CYRUP_SHELL` silently redirects every `bash` call to an arbitrary interpreter; pi has no shell env var *(ships with TOOL-007)* — **CLOSED 2026-08-14**: sweep 1 — the area's only `high` was stale. The ADR-0003 bash-surface landing deleted the `CYRUP_SHELL` arm entirely (option (i), the item's own default recommendation); `grep -rn CYRUP_SHELL crates/` now returns only the regression test `crates/cyrup-tools/tests/shell_interpreter.rs`. This also invalidates handoff (g) — TOOL-007/-038/-039 were one shell-surface decision and it has been taken. |
| ~~TOOL-006~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | `write`/`edit` declare `Sequential`, serializing the whole batch — **CLOSED 2026-08-14**: sweep 1 — both `execution_mode` overrides deleted; pinned by `mutators_do_not_declare_sequential_execution` in the new `src/tests/pi_tool_semantics.rs`. The item's second Verify limb (an agent-level test that one edit + two reads yields `sequential == false`) is NOT done and belongs to whoever owns crates/cyrup-agent. |
| ~~TOOL-007~~ | ~~medium~~ **CLOSED 2026-08-14** | cyrup-original | M | Protected-path write block is on by default, has no pi analog, and `bash` bypasses it *(ships with TOOL-039)* — **CLOSED 2026-08-14**: sweep 1 — `builder.rs:239` now sets `protect_paths: false` (the item cites `:208` = true) and `isolation/mod.rs:11-17` is rewritten to match the wiring. All three cited facts were stale. |
| ~~TOOL-019~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | File-mutation-lock key is computed by a blocking canonicalize inside `guard()` (residual only) — **CLOSED 2026-08-14**: sweep 1 — closed together (one fix, as both items state). `key` is async over `tokio::fs::canonicalize` with pi's narrow ENOENT/ENOTDIR catch; the ENOTDIR half required a raw-errno match (`libc::ENOTDIR`) because stable Rust exposes no `ErrorKind` for it. Handoff (d) discharged. |
| ~~TOOL-020~~ | ~~medium~~ **CLOSED 2026-08-14** | test-defect | M | Forced-SIGKILL drain test asserts scheduling and shell-buffering outcomes — **CLOSED 2026-08-14**: sweep 1 — both defective assertions are gone: the run window is 250 ms decoupled from the 15 ms kill grace, and the shell-flush claim is REFUTED in-source and independently re-verified (`exec_argv` runs the argv it is handed, so `ShellConfig::detect()` is not consulted on this path at all). |
| ~~TOOL-021~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | `Tool::prompt_guidelines` returns `&[&str]`, so guest tools lose their guidelines — **CLOSED 2026-08-14**: sweep 2 (area 06, via the EXT-007 refutation) — the blocker is gone: `Tool::prompt_guidelines` returns `Vec<&str>` (`crates/cyrup-core/src/tool.rs:130`), `ToolDescriptor.prompt_guidelines: Vec<String>` exists (`cyrup-ext/src/registry.rs:27`), and `impl Tool for WasmTool` overrides it (`cyrup-ext/src/host/live.rs:1690`) with an in-source note recording that TOOL-021/EXT-007 unblocked it. Consumed by EXT-038's `promptGuidelines` half. The one remaining producer gap is EXT-007's, not this item's. |
| TOOL-022 | medium | not-ported | L | `renderShell`, `prepareArguments` and `label` never reach a guest tool's behavior — **FIX SITE: `crates/cyrup-tui/**` + `crates/cyrup-core`, NOT `crates/cyrup-tools`.** **2026-08-14, sweep 6 — re-verified: the PRODUCER half is done** (`WasmTool` overrides `render_kind`/`prompt_guidelines`/`prepare_arguments` at `cyrup-ext/src/host/live.rs:1774-1808`, and `cyrup-ext/src/wrapper.rs:110` delegates `render_kind`). What is missing is a CONSUMER: `grep -rn 'render_kind' crates` shows **zero** sites in `cyrup-tui` that branch on the value. ~~Needs one agent owning cyrup-tui + cyrup-core~~ **CORRECTED 2026-08-14 (sweep 8): needs `cyrup-session-svc` TOO — `ToolRun` carries only `name: String` and no `tool_info`/`tool_catalog`/`set_tools` accessor exists on `App`, so there is nothing for a TUI branch to read.** See the correction block under the body. |
| ~~TOOL-023~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | `find` walks the whole tree then sorts and truncates; pi passes `--max-results` — **CLOSED 2026-08-14**: sweep 1 — option (i) chosen (bounded walk, sort DELETED), landed with TOOL-033, discharging handoff (e). Citations corrected: the empty-result check is find.ts:311 (not :172/:297) and `--max-results` is :252. |
| ~~TOOL-033~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | `grep` walks the whole tree before searching; `limit` bounds neither cost nor selection — **CLOSED 2026-08-14**: sweep 1 — walk and search fused; sort deleted. Citations corrected to the re-derived v0.84.1 offsets: the early-return guard is grep.ts:278 (the item implies :288) and the limit block is :292-295 (the item says :288-295). |
| ~~TOOL-034~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | M | `grep` materializes every candidate file in memory, twice on the context path — **CLOSED 2026-08-14**: sweep 1 — the seam gained `FsOps::read_stream` with a whole-read default (decorators untouched) and a real-`File` override on `LocalFs`; grep drives `search_reader` from `spawn_blocking`. The item's premise that the FsOps seam "offers no alternative" is now false. The context-path re-read was NOT changed and needed no change. |
| ~~TOOL-038~~ | ~~medium~~ **CLOSED 2026-08-14** | cyrup-original | S | On Windows with no bash, `bash` silently falls back to `cmd.exe /C`; pi refuses to run — **CLOSED 2026-08-14**: sweep 1 — `ops/shell.rs` `try_detect()`'s Windows arm is pi's verbatim `No bash shell found.` throw with the searched-paths list; no `cmd.exe` arm exists. `detect()` survives only as the infallible `Default` path, degrading to `bash -c` with a `[CYRUP-DELTA]`. |
| ~~TOOL-011~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `find` path-globs match the relative path; pi/fd match the absolute path — **CLOSED 2026-08-14**: sweep 1 — and FIX THE ITEM'S VERIFY rather than copying it forward: "assert `/src/**/*.ts` returns the same set as `src/**/*.ts`" is wrong about fd, which compares the ABSOLUTE candidate path, so a leading-slash pattern anchors at the filesystem root and matches nothing under a tmp repo. The genuine divergence is case (a) — a pattern naming an ancestor above the search root — and that is what the landed test pins. The confidence-medium caveat (fd not vendored) still applies. |
| ~~TOOL-014~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `edit`'s access-failure body diverges from pi's `Error code: <ERRNO>` — **CLOSED 2026-08-14**: sweep 1 — the errno mapping exists in `error.rs` (`errno_name`/`io_errno`/`errno_code_of`) and is consumed by `ops/local.rs`'s `access` AND `read_dir`, so TOOL-029 reuses it as the item anticipated. The factually-wrong comment at `edit.rs` is deleted. |
| TOOL-015 | low | not-ported | M | `edit` does not declare `renderShell: "self"`; nothing reads `render_kind` (residual only) — **FIX SITE: `crates/cyrup-tui/**` + `crates/cyrup-core`, NOT `crates/cyrup-tools`.** **2026-08-14, still open**: sweep 1 — kept open as a pure TOOL-022 residual; the built-in half is done (`EditTool::render_kind()` returns `SelfRendered` at `cyrup-tools/src/tools/edit.rs:131-132`, pinned at `src/tests/pi_tool_semantics.rs:66-78`). Sweep 6 re-verified: what remains is only "give the value a consumer" (cluster C5), and `grep -rn 'render_kind' crates` still shows zero branching sites in cyrup-tui. **FIX SITE CORRECTED 2026-08-14 (sweep 8): `cyrup-tui` + `cyrup-core` is NOT sufficient — `cyrup-tui` has no tool-metadata channel at all, so a producer must be added in `crates/cyrup-session-svc` in the same commit. See the correction block under TOOL-022.** |
| ~~TOOL-016~~ | ~~low~~ **CLOSED 2026-08-14** | upstream-drift | M | `constrainedSampling` has no representation in cyrup's tool model — **CLOSED 2026-08-14**: sweep 6 — **closed under `PROV-011`, not under this row, which is why no tools pass noticed.** `cyrup_core::Tool::constrained_sampling()` is on the vtable (`cyrup-core/src/tool.rs:156`, type in `cyrup-core/src/constrained_sampling.rs`), the WIT `tool-descriptor` carries `constrained-sampling`, and `cyrup-agent/src/agent.rs:829` copies it onto the runtime tool; pinned end-to-end by `cyrup-agent/src/tests/agent_loop.rs::prov011_a_tools_constrained_sampling_declaration_reaches_the_provider`. **Do not add opt-ins to cyrup-tools' built-ins**: no pi built-in declares `constrainedSampling` (three grep hits at v0.83.0, all in `extensions/types.ts:463` and `tool-definition-wrapper.ts:14`/`:42`), so that would be a divergence *from* pi. |
| TOOL-017 | low | not-ported | M | `read`'s compact `docs` classification arm is unported (residual only) — **FIX SITE: `crates/cyrup-tui/**`, and it needs a PRODUCT DECISION first.** **2026-08-14, sweep 6 — re-verified still open**: `cyrup-tui/src/transcript.rs:2305` documents the missing arm in-source and names the blocker precisely — cyrup has no decided shipped-docs root for `getPiDocsClassification` to resolve against. Once that root is decided the arm is mechanical. |
| ~~TOOL-018~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `edit` fuzzy matcher returns not-found where pi returns duplicate-occurrences — **CLOSED 2026-08-14**: sweep 1 — the CODE option was taken, not the documentation option: the divergence was not forced (Rust `str::find("")` is `Some(0)`, identical to JS), so a `[CYRUP-DELTA]` would have been an accepted-divergence in disguise. The one genuinely unobservable difference (empty content + empty needle: pi -1 vs cyrup 0) is documented in-source. |
| ~~TOOL-024~~ | ~~low~~ **CLOSED 2026-08-14 — already-done, and its own evidence was wrong** | test-defect | S | `every_surface_method_delegates` proves nothing for 9 of its 11 assertions — **CLOSED 2026-08-14**: sweep 8 re-read `crates/cyrup-ext/src/wrapper.rs` at HEAD rather than the row. All **13** `Tool` methods are delegated (`:88-158`: name, parameters, execution_mode, description, label, prompt_snippet, prompt_guidelines, render_kind, constrained_sampling, prepare_arguments, render_call, render_result, execute); the `Fixed` fixture (`:202-272`) carries a **distinct non-default for every one**, including a *mutating* `prepare_arguments` (an identity default would be indistinguishable from a dropped delegation); and `every_surface_method_delegates` (`:362`) asserts each against **both** the inner and a literal, with presence-before-absence on `constrained_sampling` (`assert!(inner.constrained_sampling().is_some(), "fixture must declare it")`). **CORRECTION TO THIS ROW, recorded because it is the kind of claim a later reader would trust:** the text below asserts the `Fixed` double at `:229-230` "returns the same `SelfRendered` the real default returns". **That is false** — `Tool::render_kind`'s trait default is `ToolRenderKind::Default` (`cyrup-core/src/tool.rs:137-139`), so the fixture's `SelfRendered` **is** distinct. The row's standing lesson survives its closure and is promoted to `PROV-M01`'s body: fixing the fixture once does not immunise it. **Superseded fix-site text follows.** — **FIX SITE: `crates/cyrup-ext/src/wrapper.rs`. RE-HOMED TO AREA 06 for routing (the ID stays here so it is neither renumbered nor double-counted; `06-cyrup-ext.md` carries a pointer row note).** **2026-08-14, still open**: sweep 2 re-routed it and sweep 6 re-verified at HEAD — both fix sites are `crates/cyrup-ext/src/wrapper.rs` (the `Fixed` double at `:229-230`, returning the same `SelfRendered` the real default returns, and `every_surface_method_delegates` at `:379-380`). It is not actionable from `crates/cyrup-tools/**` at all. **Sweep 6 also showed the item is not a one-off:** `constrained_sampling` was added to `cyrup_core::Tool` *after* TOOL-024's original nine were fixed and immediately reintroduced a tenth vacuous assertion (fixed under `PROV-011`). Fixing the fixture once does not immunise it — the "every fixture value is DISTINCT and non-default" invariant has to be re-established in the same commit as every new trait method. |
| ~~TOOL-025~~ | ~~low~~ **CLOSED 2026-08-14** | test-defect | S | The mutation-lock concurrency test is probabilistic and misnamed (residual only) — **CLOSED 2026-08-14**: sweep 1 — renamed to `write_creates_dirs_and_holds_one_mutator_per_path` and made structural via a `MutexProbeFs` asserting max-concurrency 1 inside the guarded region — the exact "shared counter" fix the item specifies. |
| ~~TOOL-026~~ | ~~low~~ **CLOSED 2026-08-14** | test-defect | S | `bash_timeout_fractional_seconds` asserts a wall-clock upper bound — **CLOSED 2026-08-14**: sweep 1 — the ceiling is `< 15s`, not the cited 4000 ms; the `>= 2300ms` lower bound and the message assertion are retained. |
| ~~TOOL-029~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `ls` swallows pi's `Cannot read directory: <message>` — **CLOSED 2026-08-14**: sweep 1 — upstream citation corrected to ls.ts:147-152 (the item says :145/:150). |
| ~~TOOL-030~~ | ~~low~~ **CLOSED 2026-08-14** | test-defect | S | `exec_pre_cancelled_never_spawns` carries a 200ms wall-clock ceiling — **CLOSED 2026-08-14**: sweep 1 — the 200 ms bound was replaced by the structural non-existent-cwd sentinel proof (only pi's ordering can yield `Ok(Killed)`), with the marker check explicitly retained as non-sufficient — the item's own "do NOT merely delete the timing assertion" warning was honoured. |
| TOOL-031 | low — **PARTIALLY CLOSED 2026-08-14** | not-ported | S | `bash` stamps no `AI_AGENT` / `PI_CODING_AGENT` into the child environment — **FIX SITE OF THE RESIDUAL: `crates/cyrup-ext-subagents/**`, NOT `crates/cyrup-tools`.** **PARTIALLY CLOSED 2026-08-14**: sweep 1 + 2 landed the immediate-bash half (`cyrup-session-svc/src/bash.rs:107-109` pushes `PI_CODING_AGENT=true` and `AI_AGENT=cyrup` onto the `ExecSpec` env vector, OUTSIDE the `expose_session_environment` gate and outside the scrub list, with the `[CYRUP-DELTA, value only]` note on `AI_AGENT`). **Sweep 6 narrowed it further: the BASH-TOOL half is also done** — `cyrup-tools/src/tools/bash.rs:154-165` pushes both variables, pinned by `cyrup-tools/src/tests/bash_session_env.rs:200-221`. **RESIDUAL: the subagent re-exec half ALONE (PARITY-GAPS PB-5).** |
| ~~TOOL-032~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `FileMutationLocks::key` blocks on `std::fs::canonicalize` and swallows every error kind — **CLOSED 2026-08-14**: sweep 1 — closed together (one fix, as both items state). `key` is async over `tokio::fs::canonicalize` with pi's narrow ENOENT/ENOTDIR catch; the ENOTDIR half required a raw-errno match (`libc::ENOTDIR`) because stable Rust exposes no `ErrorKind` for it. Handoff (d) discharged. |
| ~~TOOL-035~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `read`'s image sniff scans the whole file where pi sniffs 4100 bytes — **CLOSED 2026-08-14**: sweep 1 — clears blind spot 4: the >4100-byte pre-`acTL` PNG the prior pass only reasoned about is now constructed in `image_sniff_window_matches_pis_4100_bytes`. |
| ~~TOOL-036~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `resolve_to_cwd` omits the win32 leg of `normalizePath` (`~`, Git Bash / WSL / Cygwin paths) — **CLOSED 2026-08-14**: sweep 1 + 2 — closed with its duplicate DRIFT-046. Both halves ported in `crates/cyrup-tools/src/path.rs`. **Sweep 2 found a SECOND live instance** in `crates/cyrup-config/src/paths.rs` — the 1:1 port of the very function upstream applies the rule inside — created AFTER this item was written, by CFG-025/CFG-036; that copy is now fixed too. See DRIFT-046. |
| ~~TOOL-037~~ | ~~low~~ **CLOSED 2026-08-14** | not-ported | S | `output-guard`'s protocol-writer half (retry, serialized tail, flush) is unported — **CLOSED 2026-08-14**: sweep 1 — location corrected: the retrying writer is `crates/cyrup-modes/src/raw_stdout.rs`, not `crates/cyrup/src/output_guard.rs` (the consumers are the protocol writers and `cyrup` depends on `cyrup-modes`, not the reverse); `output_guard.rs` re-exports it. |
| ~~TOOL-040~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `find_bash_on_path` runs `which bash` with no timeout; pi bounds it at 5s — **CLOSED 2026-08-14**: sweep 1 — `BASH_PROBE_TIMEOUT = 5s` (citing shell.ts:47) with a deadline-polled `try_wait()` loop that kills, reaps and returns `None` on expiry — pi's `sh -c` fall-through. |
| ~~TOOL-041~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `write`/`edit` never re-check cancellation after the write lands — **CLOSED 2026-08-14**: sweep 1. |
| TOOL-042 | medium — **REOPENED 2026-08-14; PARTIALLY CLOSED** | test-defect | M | An intermittent `LEAK` fails the `cyrup-tools` suite; the source-scan pin closed one real class but **not** the failure — **REOPENED 2026-08-14 (sweep 8), superseding sweep 6's "filed and closed … pinned so it cannot return".** Refuted by measurement, not by reading: **3 LEAK-FAILs in 286 scrubbed runs (~1.0%, down from the historical ~4 in 33 ≈ 12%), on three DIFFERENT tests**, one of them on an idle box. `fail-fast` CANCELS the run (`243 tests run: 242 passed, 1 failed`), so it is a hard gate red. **The stated mechanism is refuted for the instrumented occurrence**: it named `ops::bash_operations_tests::local_bash_operations_forwards_command_cwd_and_env_onto_the_proc_seam`, which drives `RecordingProc` — an in-memory double (`ops/mod.rs:605`) — and whose only possible child (`which bash`, `ops/shell.rs:78-84`) names all three stdio handles and is reaped in-loop, so **no candidate holder of that test process's fd 1/2 exists in its subtree**, and the wait ran the full 500 ms. Corroborating: 69 distinct orphan pipe addresses vs 244 sampled `cargo-nextest` pipe addresses — **zero intersection**. **KEEP the source-scan pin: it closed a real class.** RESIDUAL: a harness-level question. See the body. |
| TOOL-043 | low | cyrup-original | S | `bash`'s `promptGuidelines` string diverges from the ported tag TWICE — a v0.84.0 rewording cyrup is ahead of, and a deliberate `PI_*` → `CYRUP_*` rename — and neither carries a `[CYRUP-DELTA]` tag. Model-facing: this lands in the system prompt's Guidelines section. **FILED 2026-08-14** (sweep 9, mechanical built-in-tool surface enumeration) |
| TOOL-044 | low — **PARTIALLY CLOSED 2026-08-14** | parity-bug | S | The serialized `details.truncation` payload diverges from pi's `TruncationResult` on three fields — **FILED AND PARTIALLY CLOSED 2026-08-14** (sweep 9): `truncatedBy` now serializes as an explicit `null` instead of vanishing, and the byte-only `maxLines` sentinel is pi's `Number.MAX_SAFE_INTEGER` instead of `usize::MAX`. **RESIDUAL: pi's `content` field**, deliberately not ported — see the body. |
| TOOL-045 | low | not-ported | S | No built-in overrides `Tool::label`, so all seven return the trait default `None` where pi sets an explicit `label` on every built-in `ToolDefinition`. Behaviourally equivalent today ONLY because pi's seven values are identical to the names. **FILED 2026-08-14** (sweep 9) |

## TOOL-039 — `CYRUP_SHELL` silently redirects every `bash` tool call to an arbitrary interpreter; pi has no shell env var

**Kind** cyrup-original · **Severity** high · **Effort** S · **Confidence** high (all five limbs re-verified independently in the 2026-08-12 repair pass)

> **Severity raised low → high, 2026-08-12 repair pass** (completeness critique finding 5, verified
> rather than accepted). The rating was indefensible: this item was filed below `get_tree` dropping
> pi's `labelTimestamp`. The five facts that carry the rating were each re-read at source this pass
> and all five hold — see **cyrup** below. What makes it `high` rather than `medium` is the
> combination: the override is **first** in resolution order, it is reachable **by default** with no
> setting and no flag, it **propagates** to children because it is not scrubbed, it is **silent** in
> the transcript, and it has **no upstream analogue at all**, so there is no pi behaviour it is
> faithfully porting. It stops short of `critical` only because it requires an attacker or a
> misconfiguration to already control cyrup's environment — at which point a great deal else is also
> reachable — so it is an ambient-authority surface rather than a defect that fires on its own.
>
> **Ships with TOOL-007.** Both items are decisions about what `bash` is permitted to be, and taken
> separately they produce an incoherent surface: TOOL-007 concedes that cyrup's protected-path guard
> is "security theatre" precisely *because* `bash` is undecorated (`builder.rs:646` passes
> `base.proc` through untouched), while this item shows that the same undecorated `bash` will run
> under whatever interpreter the ambient environment names. Deciding "cyrup constrains what the model
> can do through `bash`" and "cyrup does not control which shell `bash` is" cannot both be true.
> Schedule them as one change with one written answer.

**cyrup** — Five limbs, each re-verified at source this pass.
(1) **The override is first.** `crates/cyrup-tools/src/ops/shell.rs:101-105` — `ShellConfig::detect()`
opens with `if let Some(explicit) = std::env::var_os("CYRUP_SHELL") { return
get_bash_shell_config(PathBuf::from(explicit)); }`, **ahead of** the `/bin/bash` probe (`:108-110`),
`find_bash_on_path` (`:111-113`) and the `sh` fallback (`:114-118`); the in-source comment at
`:101-102` concedes "it has no Pi analogue".
(2) **It is the default path.** `detect()` is what `ToolRegistry::with_builtins` calls
(`registry.rs:54` `let shell = ShellConfig::detect();`) and what `Backend::default()` calls
(`ops/mod.rs:359-361` `Self::local(ShellConfig::detect())`), so with no `shellPath` setting this env
var **is** the interpreter for every model-issued `bash` call.
(3) **It is not scrubbed.** `session_env_scrub_keys()` (`config.rs:41-48`) is built solely from
`SESSION_ENV_SUFFIXES` (`config.rs:31-32`: `SESSION_ID`, `SESSION_FILE`, `PROVIDER`, `MODEL`,
`REASONING_LEVEL`) crossed with the `CYRUP_` and `PI_` prefixes — ten keys, none of them
`CYRUP_SHELL`. A parent that exports it therefore reaches every nested cyrup re-exec, and the doc at
`config.rs:38-40` states that a subagent run *is* a real re-exec of the `cyrup` binary, so that
inheritance path is live rather than theoretical.
(4) **It is silent.** Nothing writes the resolved program to the transcript, the event stream or a
log line; `get_bash_shell_config` returns a `ShellConfig` and no caller reports it.
(5) **There is no `shellPath` precedence problem to hide behind** — the settings override is handled
one function up (`shell.rs:95-96` returns `Ok(Self::detect())` only after the explicit path arm), so
`CYRUP_SHELL` beats every *automatic* source and loses only to an explicit user setting.

**upstream** — `getShellConfig` (`pi/packages/coding-agent/src/utils/shell.ts:67-120`, read in full,
**byte-identical at v0.83.0 and v0.84.1** — `git diff v0.83.0 v0.84.1 -- …/utils/shell.ts` is empty,
so these offsets are valid at either tag) reads **no** environment variable as a shell selector. The
only `process.env` reads in the whole file are `ProgramFiles` / `ProgramFiles(x86)` at `:79` / `:83`,
which are Windows *installation-location* lookups feeding the Git Bash candidate list, and the `PATH`
handling in `getShellEnv` at `:124-131`. The only override is the explicit `customShellPath` argument
(`:69-74`), fed from the `shellPath` setting, and pi *throws* on a `customShellPath` that does not
exist (`:73`) rather than silently accepting it.

**Impact** — Any process in cyrup's environment chain — a wrapper script, a CI runner, a shell
profile, a parent agent, a compromised dependency's `postinstall` — silently substitutes the
interpreter for **every** command the model runs, and neither the model, the user, the transcript nor
the session file records which shell executed. The substitute need not be a shell: it is passed
straight to `get_bash_shell_config`, so `CYRUP_SHELL=/path/to/anything` makes that binary the
executor of every model-issued command, with the command text as its argument. Because it is not
scrubbed, one export at the top of a session silently governs the parent session, every subagent
re-exec beneath it, and any `bash` those subagents run. And because the resolution order puts it
first, it beats a perfectly good `/bin/bash` on a healthy machine — this is not a fallback that fires
only when detection fails. This is live production code reachable by default, unlike the dead
`isolation/policy.rs` helpers rejected in `## Coverage` item 7.

**Fix** — Decide deliberately, **in the same change as TOOL-007**, and write the answer down. Option
(i), pi's shape and the default recommendation: delete the `CYRUP_SHELL` arm at `shell.rs:101-105`
and require the `shellPath` setting, which is a configured, auditable, session-scoped input rather
than an ambient one — this is a three-line deletion. Option (ii), keep it: then all four of (a) stamp
a `[CYRUP-DELTA]` at `shell.rs:101-105` naming the divergence and the reason pi has no counterpart,
(b) report the resolved interpreter once at session start and in the `bash` tool's own result details
so the transcript records what actually ran, (c) add `CYRUP_SHELL` to `SESSION_ENV_SUFFIXES`' scrub
set at `config.rs:41-48` — note it does not fit the `{CYRUP,PI}_<SUFFIX>` shape, so the scrub list
needs a second, explicitly-named group — and (d) validate the path exists and is executable and fail
loudly if not, matching `shell.ts:73`. Half of option (ii) is not an option.

**Verify** — Option (i): a unit test asserting `ShellConfig::detect()` ignores `CYRUP_SHELL`
(set the var, assert the returned `program` is `/bin/bash`), plus a grep assertion that
`CYRUP_SHELL` appears nowhere in `crates/`. Option (ii): assert the resolved program appears in the
session-start diagnostics **and** in a `bash` tool result's details; assert
`session_env_scrub_keys()` contains `CYRUP_SHELL`; and assert end-to-end that a subagent re-exec
launched with `CYRUP_SHELL` set in the parent's environment does **not** see it in its own
`ShellConfig::detect()`. Under either option, add the negative test that a non-existent
`CYRUP_SHELL` path fails loudly rather than producing a config that fails at first `bash` call.

## TOOL-006 — `write`/`edit` declare `ExecMode::Sequential`, serializing the whole tool batch

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-tools/src/tools/write.rs:56-58` and `crates/cyrup-tools/src/tools/edit.rs:119-121` both still `fn execution_mode(&self) -> ExecMode { ExecMode::Sequential }`, with no `[CYRUP-DELTA]`. `crates/cyrup-agent/src/agent.rs:880-882` computes `any_seq` over every call in the batch, `:883` ORs it with the session mode, and `:888` routes the WHOLE batch to `execute_sequential` (`:1325`). Per-file serialization is already provided independently by `FileMutationLocks` (`write.rs:83`, `edit.rs:189`).

**upstream** — `git grep -n executionMode` at v0.84.1 over `core/tools/` and `core/extensions/` hits only `tool-definition-wrapper.ts:16`, `:44` and `extensions/types.ts:477` — the plumbing. No built-in sets it: `edit.ts:305-314` and `bash.ts:191-197` declare no `executionMode`, and upstream serialization is `withFileMutationQueue` alone (`edit.ts:316`, `write.ts:212`).

**Impact** — A batch containing one `edit` plus several `read`s or `grep`s runs entirely serially, adding latency proportional to batch size on the most common multi-tool turn shape.

**Fix** — Delete both `execution_mode` overrides.

**Verify** — Assert `ExecMode::Parallel` for both in `crates/cyrup-tools/tests/pi_schema.rs`, plus an agent-level test that a batch of one `edit` and two `read`s yields `sequential == false` at `agent.rs:880-883`.

## TOOL-007 — `write`/`edit` to `.env`, `.git/`, `node_modules/` blocked by default, no pi analog, and `bash` bypasses it

**Kind** cyrup-original · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `crates/cyrup-session-svc/src/builder.rs:208` still sets `protect_paths: true` in the default `SessionConfig`; `:643-644` `if cfg.protect_paths { fs = Arc::new(ProtectedFs::with_defaults(fs)); }`; `:646` `let backend = Backend { fs, proc: base.proc.clone() };` — only `fs` is decorated, so `bash 'echo K=v >> .env'` bypasses the guard entirely. `grep -rn protect_paths crates/` returns only `builder.rs:152` (doc), `:153` (decl), `:208`, `:643` — still no CLI flag, no setting, no override. `ProtectedPaths::defaults()` is unchanged at `crates/cyrup-tools/src/isolation/protected.rs:30-32` with path-COMPONENT equality matching at `:51-58`. `crates/cyrup-tools/src/isolation/mod.rs:3-6` still asserts the opposite of the wiring ("by default nothing here is in the call path").

**upstream** — No protected-path concept exists anywhere under `pi/packages/coding-agent/src/core/tools/` at v0.84.1; `write.ts:203-231` and `edit.ts:305-360` write whatever path they are given.

**Impact** — A silent, undocumented, unoverridable refusal on three common paths. The model is told nothing about the restriction (no description text, no guideline), so it retries or routes around it via `bash` — which succeeds, making the guard security theatre while still costing a failed turn.

**Fix** — Decide deliberately: either flip `builder.rs:208` to `false` and expose a flag/setting, or keep it on and (a) surface it in the `write`/`edit` descriptions plus a `prompt_guidelines` entry, (b) decorate `ProcOps` so `bash` is covered, (c) correct `isolation/mod.rs:3-6`. Sibling `confine_to_cwd` is correctly `false` and needs no change. Note `default_bash_rm_rf_runs_without_any_gate` (`crates/cyrup-tools/tests/isolation.rs`) is NOT a test defect — it builds `Backend::default()` directly and is accurate for `bash`.

**Verify** — With the guard on: assert `write` to `.env` is refused AND `bash 'echo x >> .env'` is refused. With it off: assert both succeed and no `ProtectedFs` is in the chain.

## TOOL-019 — File-mutation-lock key is computed by a blocking canonicalize inside the async `guard()` (residual)

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

> **Partially closed.** The item's primary defect — a per-`ToolRegistry` mutation queue — is closed by `7fd0d9c`. The ID stays open for the secondary defect it also recorded. **The residual is written up in full as TOOL-032; fix it once, not twice.** Severity is retained at the auditor's rating; note that the residual considered on its own is rated low under TOOL-032.

**cyrup** — Closed half: `crates/cyrup-tools/src/lock.rs:29-30` `static FILE_MUTATION_LOCKS: LazyLock<Arc<DashMap<..>>>`, attached by `new()` at `:78-80`, with `Default` hand-written as an alias for `new()` at `:44-51` precisely so a derived `Default` cannot silently re-create per-owner domains. Revert-proved by `lock.rs:164-187` `independent_handles_share_one_lock_per_path` (`Arc::ptr_eq` on the maps AND `via_b.try_lock().is_err()` while `a` holds it) and by `crates/cyrup-tools/tests/cross_registry_mutation_lock.rs:144`, `:208`, `:262`. Open half: `lock.rs:85-87` `fn key()` still calls the BLOCKING `std::fs::canonicalize`, invoked at `:96` from inside the async `guard()` (`:91-107`), which both mutators await (`write.rs:83`, `edit.rs:189`).

**upstream** — `pi/packages/coding-agent/src/core/tools/file-mutation-queue.ts:4` keeps `fileMutationQueues` at MODULE level (the half cyrup now matches), and `getMutationQueueKey` (`:16-26`) is `async` with `await realpath(...)` — never blocking (the half cyrup does not).

**Impact** — Every `write`/`edit` performs a synchronous `realpath(2)` on a tokio worker thread; on an NFS/SSHFS/FUSE mount or a deep symlink chain that stalls the whole runtime, including unrelated in-flight tool calls and the provider stream.

**Fix** — See TOOL-032. Do not re-derive it here.

**Verify** — See TOOL-032. The process-global half is already covered and must stay green: `lock.rs:164-187` and `tests/cross_registry_mutation_lock.rs`.

## TOOL-020 — Forced-SIGKILL stdout-drain test asserts scheduling and shell-buffering outcomes it cannot control

**Kind** test-defect · **Severity** medium · **Effort** M · **Confidence** high (analytic)

**cyrup** — `crates/cyrup-tools/src/ops/local.rs:992-1060`, `exec_argv_forced_sigkill_does_not_drop_buffered_stdout_already_sitting_in_the_pipe`, unchanged at HEAD. Eight trials (`:1013`); each builds `LocalProc::with_kill_grace(ShellConfig::detect(), Duration::from_millis(15))` (`:1017`) and runs a trapped-TERM `sh -c` loop under a 15ms timeout (`:1032`). Two assertions lie outside its control. (a) `:1046-1050` `assert!(gt_last >= 0, …)` requires fork+exec of `/bin/sh` plus one full loop iteration inside roughly 30ms. (b) `:1051-1057` `assert!(gt_last - stdout_last <= 1, …)` assumes the host `ShellConfig::detect()` shell flushes stdout once per iteration. The doc comment at `:991-1009` still concedes the timing is "inherently racy" and defends it with per-host trial counts.

**upstream** — Not a parity question: the ported behavior mirrors `pi/packages/coding-agent/src/core/tools/exec.ts` `killProcess` correctly. Only the assertion strategy is defective.

**Impact** — Intermittent failure in the repo's only gate — cyrup has no CI, so `cargo test` is all there is. Once a file is known to flake, a genuine regression in it gets dismissed.

**Fix** — Make the invariant deterministic: (1) replace the fixed 15ms timeout with a barrier — the child writes a `ready` marker after its first iteration and the timeout starts only once the marker exists, so `gt_last >= 0` holds by construction; (2) force line-buffered stdout independently of the host shell (`stdbuf -oL`, or a small Rust helper instead of a shell) so the lag bound stops depending on `ShellConfig::detect()`; (3) failing both, delete the trial loop as `1806375` did. The house technique already exists: `bash_trailing_edge_flush_emits_midstream` uses `#[tokio::test(start_paused = true)]`.

**Verify** — After the rewrite the test must pass under `cargo test --workspace` with artificial load and `--test-threads=32`. Caveat recorded by the refuter and worth keeping: the doc comment at `local.rs:1006-1010` states the current shape was empirically revert-validated (deterministic failure pre-fix across 3 runs, 40 clean trials post-fix). That mitigates the flake risk but does not remove the host dependence, so the item stands. Do NOT sweep in the sibling `< Duration::from_secs(2)` bounds elsewhere in the file — each guards a ~100-200ms path against a 5s grace alternative and is load-bearing.

## TOOL-021 — `Tool::prompt_guidelines` returns `&[&str]`, so guest tools silently lose their declared guidelines

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

> **Upstream mechanism corrected this pass.** The prior text (and the auditor's re-statement) claimed `tool-definition-wrapper.ts` copies `promptGuidelines` onto every AgentTool. It does not — the refuter read the file in full at v0.84.1. The corrected path is given below; the substantive gap is unaffected and the severity stands.

**cyrup** — `crates/cyrup-core/src/tool.rs:120-122` still declares `fn prompt_guidelines(&self) -> &[&str] { &[] }`. A `&[&str]` cannot be produced from a `Vec<String>` without an allocation the borrow cannot outlive, so `WasmTool` does not implement it: `crates/cyrup-ext/src/host/live.rs:1386-1420`, read in full, implements `name` (`:1387`), `parameters` (`:1390`), `execution_mode` (`:1393`), `description` (`:1399`), `prompt_snippet` (`:1406`) and `execute` (`:1409`), then closes at `:1420`. The data DOES reach the host — `crates/cyrup-ext/wit/world.wit:45` carries `prompt-guidelines: list<string>`, `register_tool` copies it at `live.rs:84`, it is stored at `crates/cyrup-ext/src/registry.rs:27` — but that field has no reader anywhere in `crates/`, and the vtable is the only surface the prompt builder reads (`crates/cyrup-session-svc/src/builder.rs:1606`).

**upstream** — `pi/packages/coding-agent/src/core/extensions/types.ts` declares `promptGuidelines?: string[]` on `ToolDefinition`. `wrapToolDefinition` (`tool-definition-wrapper.ts:9-19`) copies name, label, description, parameters, `constrainedSampling`, `prepareArguments`, `executionMode` and `execute` — and NOT `promptSnippet`/`promptGuidelines`. pi instead reads them off the definition registry: `agent-session.ts:2505-2512` builds `_toolPromptGuidelines` from `definition.promptGuidelines` over `definitionRegistry`, which at `:2490-2495` includes `allCustomTools` (extension tools), and `:1026-1038` folds them into `buildSystemPrompt`. Either way an extension tool's guidelines reach the prompt; in cyrup they do not.

**Impact** — Every WASM extension tool's declared usage guidance is dropped between the guest and the system prompt, with no warning. Extension authors get a field that appears to work and does nothing.

**Fix** — Widen the trait return type rather than working around it: `fn prompt_guidelines(&self) -> Vec<&str>` (or `Cow<'_, [&str]>`) in `crates/cyrup-core/src/tool.rs:120-122`. The built-ins that declare guidelines keep their const arrays with a trivial `.to_vec()`; `RegisteredTool` (`crates/cyrup-ext/src/wrapper.rs:107-111`) forwards unchanged; `WasmTool` becomes `self.descriptor.prompt_guidelines.iter().map(String::as_str).collect()`. The prompt builder at `builder.rs:1606` already maps into owned values, so nothing downstream changes.

**Verify** — Register a guest tool declaring two guidelines, assert both appear in the built system prompt. `crates/cyrup-tools/tests/pi_schema.rs:139-215` must stay green unchanged.

## TOOL-022 — `renderShell`, `prepareArguments` and `label` never reach a guest tool's behavior

**Kind** not-ported · **Severity** medium · **Effort** L · **Confidence** high

**cyrup** — There are two `ToolDescriptor` types and they disagree. The host-facing WIT record is still the 8-field shape — `crates/cyrup-ext/wit/world.wit:39-48`: name / label / description / parameters-json / exec-mode / prompt-snippet / prompt-guidelines / has-renderer — with no `render-shell` and no `prepare-arguments`, so a guest's declaration has no wire to arrive on. `impl Tool for WasmTool` (`crates/cyrup-ext/src/host/live.rs:1386-1420`) overrides neither `label`, `prompt_guidelines`, `render_kind` nor `prepare_arguments`, so all four fall to the `cyrup_core::Tool` defaults (`tool.rs:105-107`, `:120-122`, `:126-128`, `:133-135`). The guest-side type still declares what the host cannot receive: `crates/cyrup-ext-sdk/src/descriptor.rs:19` `RenderShell`, `:45` `render_shell`, `:65` the default.

**upstream** — `pi/packages/coding-agent/src/core/extensions/types.ts:453` `label: string` (REQUIRED), `:465` `renderShell?: "default" | "self"`, `:468` `prepareArguments?`. `tool-definition-wrapper.ts:11` copies `label` and `:15` copies `prepareArguments` onto every AgentTool, extension tools included.

**Impact** — Three losses. (1) A guest tool declaring an argument-normalizing shim never gets it run — precisely TOOL-002, un-fixed, for every WASM tool. (2) A `renderShell: "self"` guest tool is double-framed with no diagnostic; this is now the *entire* residual of TOOL-015, since the built-in `edit` path was fixed in the TUI. (3) A guest's distinct display name is unreachable, harmless today only because `Tool::label` has no consumer at all.

**Fix** — Add `render-shell: option<render-shell>` and `prepare-arguments: bool` to the WIT `tool-descriptor` record in BOTH copies (`f777e44` established that both must move together and that this breaks the guest ABI), mirror them onto `crates/cyrup-ext/src/registry.rs`, map them in `register_tool` (`live.rs:71-86`), then implement `render_kind` and `prepare_arguments` on `WasmTool` — the latter via a guest-side `prepare-arguments` export the host calls only when the flag is set. Add `fn label` and `fn prompt_guidelines` (TOOL-021) at the same time; these are one WIT bump, cluster **C5** in the residual ledger.

**Verify** — A fixture guest tool whose `prepare_arguments` renames a key: assert the renamed key reaches `execute` and passes schema validation. A second declaring `renderShell: "self"`: assert no outer frame in the TUI. Note the ABI break — any component built against the old world must be rebuilt.

> **FIX SITE CORRECTED 2026-08-14 (sweep 8) — applies to this row, to `TOOL-015`, and to `EXT-024`
> in `06-cyrup-ext.md`. The recorded site "`crates/cyrup-tui/**` + `crates/cyrup-core`" is
> INCOMPLETE, and landing only that half is a no-op.** Re-verified at HEAD:
> `grep -rn 'render_kind|ToolRenderKind|SelfRendered' crates/cyrup-tui/src/` returns **zero**, exactly
> as the rows say — but the reason is deeper than "nobody wrote the branch". **`cyrup-tui` has no
> tool-metadata channel at all:** `ToolRun` (`cyrup-tui/src/transcript.rs:701-720`) holds only
> `name: String`; there is no `tool_info` / `tool_catalog` / `set_tools` accessor anywhere on `App`;
> and the extension-renderer siblings `rendered_call` / `rendered_result` are supplied **per call by
> the caller**, not looked up. **A producer must publish a `name → ToolRenderKind` map from
> `cyrup-session-svc` first.** A TUI-side branch with nothing to read would be the "declared surface
> with no consumer" failure this directory repeatedly names. **Route to one agent owning
> `cyrup-session-svc` + `cyrup-tui` + `cyrup-core`.**
>
> **And record pi's actual mechanism, which none of the three rows states:** `renderShell: "self"`
> does not mean "the tool draws itself" in the abstract — `ToolExecutionComponent` resolves
> `getRenderShell()` **at construction** and adds a bare `selfRenderContainer` instead of the framed
> `contentBox` (`modes/interactive/components/tool-execution.ts:65-76` @v0.83.0), with three-way
> precedence `toolDefinition.renderShell ?? builtInToolDefinition.renderShell ?? "default"` at
> `:105-113`. The field is `core/extensions/types.ts:465`; the one built-in that declares it is
> `core/tools/edit.ts:306`.

## TOOL-023 — `find` walks the whole tree then sorts and truncates; pi passes `--max-results` to fd

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-tools/src/tools/find.rs:124-152` drains the ENTIRE walk into `results` — the `loop { tokio::select! { … } }` breaks only on `None` from the walker at `:148` — then `:154-157` `results.sort(); if results.len() > limit { results.truncate(limit); }`, and `:173` computes `limit_reached` off the truncated vector.

**upstream** — `pi/packages/coding-agent/src/core/tools/find.ts:252` `args.push("--max-results", String(effectiveLimit))` — fd stops after N results in its own parallel, unordered traversal; pi relativizes only the lines it received (`find.ts:321-326`, via the v0.84.1 `relativizeFindResultPath` helper) and never sorts.

**Impact** — On a large repo, `find` with a small `limit` still pays the full-tree walk (seconds where pi returns in milliseconds) and returns a different result SET — the alphabetically-first N rather than the first N discovered.

**Fix** — Decide and document: either (i) break out of the walk at `find.rs:148` once `results.len() == limit` and drop the `sort()`, matching fd's early-exit semantics and removing the full-tree cost; or (ii) keep sort-then-truncate for determinism and add a `[CYRUP-DELTA]` at `find.rs:154`. Option (ii) plus an early break at some multiple of `limit` is not equivalent and must not be presented as such. **`grep` has the identical defect (TOOL-033) — land the two together so one strategy is chosen once.**

**Verify** — Build a tree of 10k files, run `find` with `limit=5`, assert wall time is not proportional to tree size (option i) or assert the documented determinism (option ii). Assert `limit_reached` matches the chosen semantics.

## TOOL-033 — `grep` completes the entire directory walk before searching anything, so `limit` bounds neither traversal cost nor which matches are chosen

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-tools/src/tools/grep.rs:168-200` drains the whole `self.fs.walk(...)` stream into `files` — the `loop { tokio::select! { … } }` breaks only on `None` from the walker at `:196` and has no count-based exit — then `:201` `files.sort_by(|a, b| a.1.cmp(&b.1))`, and only then does the search loop at `:231-234` stop with `if count >= limit { break; }`. So every `grep` call walks the complete tree regardless of `limit`, and the 100-match window is taken from the alphabetically-first files rather than from the first matches discovered.

**upstream** — `pi/packages/coding-agent/src/core/tools/grep.ts:226` `const child = spawn(rgPath, args, …)` streams rg's `--json` events, and the line handler at `:288-295` sets `matchLimitReached` and calls `stopChild(true)` (`:240-245`) the instant `matchCount >= effectiveLimit`. rg's traversal is parallel and unordered, and `grep -n sort grep.ts` is empty — pi never reorders it. Both the traversal and the match set stop at the limit.

**Impact** — Two silent effects on any large repository. (1) Latency: a `grep` that pi answers in milliseconds after 100 hits still costs cyrup a full-tree walk on every call — seconds per tool call on a monorepo, on the single most-used search tool. (2) Sampling: when the 100-match cap fires, cyrup reports matches from the alphabetically-first files, a systematic `a*`-biased sample, where pi reports rg's discovery order. Nothing in the output distinguishes the two, so the model draws conclusions about the codebase from a differently-selected 100.

**Fix** — Fuse the walk and the search instead of staging them. In `grep.rs` replace the collect-then-sort at `:168-201` with a single streaming loop that applies the glob filter, searches each candidate as it arrives, and breaks out of the walk as soon as `count >= limit` — the same restructure TOOL-023 needs in `find.rs:124-157`, so land the two together. If determinism is preferred over pi's discovery order, keep the sort but bound the walk and stamp a `[CYRUP-DELTA]` at the sort site naming the divergence; sorting a bounded prefix is NOT equivalent to sorting the full set and must not be presented as such.

**Verify** — Build a tree of ~10k files each containing the needle, run `grep(pattern, limit=5)`, and assert wall time is not proportional to tree size (option i) or assert the documented determinism (option ii). Assert the `N matches limit reached` notice (`grep.rs:345-350`) still fires with the same `limit` under either strategy.

## TOOL-034 — `grep` materializes every candidate file in memory (twice on the context path) where pi's ripgrep streams

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `crates/cyrup-tools/src/tools/grep.rs:238-245`: `let bytes = match self.fs.read(file).await { Ok(b) => b, Err(_) => continue };` then `searcher.search_slice(&matcher, &bytes, sink)` — one full-file `Vec<u8>` allocation per candidate, taken BEFORE binary detection can reject it (the `BinaryDetection::quit` at `:222-225` only shortens the search, not the read). When any match on that file needs a context block, `:265-277` performs a SECOND full `self.fs.read(file)`, then `String::from_utf8_lossy` + two `replace` passes + `.split('\n').map(str::to_owned).collect()` into a `Vec<String>` — roughly three more copies of the file. The seam offers no alternative: `FsOps::read` (`crates/cyrup-tools/src/ops/mod.rs:277`, trait read in full at `:274-312`) returns `Result<Vec<u8>, ToolError>` and there is no streaming variant.

**upstream** — `pi/packages/coding-agent/src/core/tools/grep.ts:177` `const rgPath = await ensureTool("rg", true);` and `:226` `spawn(rgPath, args, { stdio: ["ignore","pipe","pipe"] })` — the search runs in a separate process with ripgrep's bounded read buffer / mmap strategy, so file size is decoupled from the agent process's heap entirely. pi's own in-process read, `getFileLines` (`:206-218`), runs only for files that ACTUALLY MATCHED, only on the `contextValue > 0` / non-UTF-8 path (`:333`), and is memoized per file by `fileCache` (`:205`, `:215`).

**Impact** — A `grep` over a tree containing a large uncommitted artifact — a multi-GB log, a database dump, a vendored tarball, a core file — allocates that whole file inside the agent process. On a memory-limited host that is an OOM kill of the entire session; short of that it is a multi-second stall and a large RSS spike per call. pi returns normally because rg never holds the file. The hazard applies to files that do not match at all, since the read precedes the search. Peak RSS is one file at a time, so this is a spike rather than an unbounded leak — which is why it is medium, not high.

**Fix** — Add a streaming read to the `FsOps` seam (`crates/cyrup-tools/src/ops/mod.rs`) — e.g. `fn read_stream(&self, path) -> EventStream<Result<Vec<u8>, ToolError>>` or an `AsyncRead` handle — and drive `grep_searcher`'s `search_reader` from it at `grep.rs:238-245` instead of `search_slice`, keeping `LocalFs`'s implementation on `tokio::fs::File`. As a cheap interim guard, stat the file first and skip (or fall back to a bounded prefix) above a size threshold, matching ripgrep's practical behaviour on huge files. The context-path re-read at `:265-277` should reuse the same streaming handle and cache per file as pi's `fileCache` does.

**Verify** — Create a 512 MB file containing the needle plus several small ones, run `grep` with a memory cap on the test process, and assert it completes without exceeding a bounded RSS. Assert the existing grep tests (`grep.rs:445-584`) stay green, in particular `context_block_emits_unable_to_read_marker_when_reread_fails`, which depends on the two-read shape.

## TOOL-038 — On Windows with no bash available, `bash` silently falls back to `cmd.exe /C` where pi refuses to run at all

**Kind** cyrup-original · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-tools/src/ops/shell.rs:140-145`: after the Git Bash probes (`:126-136`) and `find_bash_on_path` (`:137-139`) both miss, `ShellConfig::detect` returns `ShellConfig { program: "cmd.exe", args: ["/C"], transport: Argv }`, labelled in-source at `:123-124` as a "(cyrup pragmatic fallback)". `detect()` is what `ToolRegistry::with_builtins` uses (`registry.rs:54`) and what `Backend::default()` uses (`ops/mod.rs:359`).

**upstream** — `pi/packages/coding-agent/src/utils/shell.ts:100-106` does the opposite at exactly that point: it throws `No bash shell found. Options:\n  1. Install Git for Windows: …\n  2. Add your bash to PATH (Cygwin, MSYS2, etc.)\n  3. Set shellPath in settings.json\n\nSearched Git Bash in:\n${paths…}` — a hard stop with a repair recipe.

**Impact** — A model-authored bash command — `A=1 ./x`, `ls | grep -v foo`, `echo $HOME`, a heredoc, `2>&1` — is executed by `cmd.exe` with different or no meaning, and neither the model nor the user is told the interpreter is not bash. Quoting and redirection silently change semantics; `$VAR` expansion does not happen at all. pi never reaches that state.

**Fix** — Replace the fallback arm at `shell.rs:140-145` with pi's error, including the searched-paths list the probe block already has in hand at `:126-136`, and surface it at session construction (`registry.rs:54` / `ops/mod.rs:359`) rather than at first `bash` call. If a `cmd.exe` escape hatch is wanted, it must be an explicit opt-in setting with a `[CYRUP-DELTA]`, never the default.

**Verify** — `#[cfg(windows)]` unit test with `PATH` cleared and the Git Bash probe paths absent: assert `ShellConfig::detect()` returns an error naming all three remediation options, not a `cmd.exe` config. Assert the unix path is unaffected.

## TOOL-011 — `find` path-globs match the relative path; pi/fd match the absolute path

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** medium

**cyrup** — `crates/cyrup-tools/src/tools/find.rs:136-137` computes `rel = to_posix(w.path.strip_prefix(&search_root))` and `:142` calls `matcher.is_match(&rel, &basename)`; `PatternMatcher::is_match` (`crates/cyrup-tools/src/tools/globmatch.rs:46-52`) tests `rel_posix` in full-path mode. The `pattern.starts_with('/')` guard at `globmatch.rs:29` is dead against a relative candidate, since a relative posix path can never begin with `/`.

**upstream** — `pi/packages/coding-agent/src/core/tools/find.ts:254-256` documents in-source that fd's `--full-path` "matches against the absolute candidate path"; the `**/`-prepend block is `:257-266`, and `:267` `args.push("--", effectivePattern, searchPath)` passes the ABSOLUTE search path as fd's root. pi relativizes only for OUTPUT (`find.ts:321-326`).

**Impact** — Because both sides prepend `**/`, the two agree for the common case. The divergence is confined to (a) patterns naming an ancestor directory ABOVE the search root and (b) leading-`/` patterns such as `/src/**/*.ts`, which silently return an empty set in cyrup where fd would match.

**Fix** — Match against the absolute POSIX path in `find.rs:136-142` (keeping relativization for output only) and revisit `globmatch.rs:29` so the `starts_with('/')` arm becomes live. **The grep side is now a separate type (`RgGlob`, TOOL-027) and must NOT be changed with it** — ripgrep anchors override globs at the search root and that is already correct.

**Verify** — Under a search root `<tmp>/repo`, assert `pattern="/src/**/*.ts"` returns the same set as `pattern="src/**/*.ts"`; red today. Confidence stays medium because fd is not vendored in this workspace — fd's actual matching target is taken from pi's own in-source comment plus its argv construction. This is the only glob-family claim still resting on an external binary.

## TOOL-014 — `edit`'s access-failure body diverges from pi's `Error code: <ERRNO>`; the in-source comment is wrong

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-tools/src/tools/edit.rs:194-196` still formats `Could not edit file: {}. {e}.` where `e` is the whole `ToolError`. The comment at `edit.rs:192-193` survives verbatim and is factually wrong about upstream: "The `${errorMessage}` body itself (a Node errno string) is irreducible".

**upstream** — `pi/packages/coding-agent/src/core/tools/edit.ts:332-333` `const errorMessage = error instanceof Error && "code" in error ? \`Error code: ${error.code}\` : String(error);` and the throw at `:334` — the bare `Error code: EACCES` form, never the full Node message. Access mode matches on both sides (`edit.ts:96` `constants.R_OK | constants.W_OK`; `crates/cyrup-tools/src/ops/local.rs:114` `libc::R_OK | libc::W_OK`).

**Impact** — A read-only or missing file yields a Rust-flavoured body instead of `Error code: EACCES` / `Error code: ENOENT`. Models trained on the pi phrasing lose the machine-readable errno token, and the wrong comment will cause the next reader to close this item as unfixable.

**Fix** — Map the `ToolError` to an errno name and emit `Could not edit file: {path}. Error code: {ERRNO}.`; DELETE the incorrect comment at `edit.rs:192-193`. **TOOL-013's closure supplies nothing to reuse** — `read.rs:110` propagates the `io::Error` Display rather than naming the errno — so the mapping must be written here, in `ops/local.rs` next to the `access` implementation, where TOOL-029 can also reuse it.

**Verify** — `edit_access_error_has_trailing_period` checks only the prefix and the trailing period, so it survives unchanged; add an assertion for the `Error code: ENOENT` body and an EACCES case.

## TOOL-015 — `edit` does not declare `renderShell: "self"`; nothing in the workspace reads `render_kind` (residual)

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** high

> **Partially closed.** The user-visible half is fixed; only the mechanism remains, and it is exactly TOOL-022's WIT bump. Severity drops from medium to low because the transcript no longer double-frames `edit`.

**cyrup** — Closed half: `crates/cyrup-tui/src/transcript.rs:1802-1840` now renders `edit` with pi's own component shape — the pre-execution preview from `compute_edits_diff`, the settled `details.diff` REPLACING it (not appended), and `edit_header_preview` porting `getEditHeaderBg`. Open half: `impl Tool for EditTool` (`crates/cyrup-tools/src/tools/edit.rs:111-159`) still does not override `render_kind`, and `grep -rn 'render_kind\|ToolRenderKind' crates/` resolves to exactly the enum and default (`cyrup-core/src/tool.rs:67`, `:126-128`, `:198`), the re-export (`cyrup-core/src/lib.rs:33`) and the forwarder plus its assertion (`cyrup-ext/src/wrapper.rs:24`, `:110-111`, `:287`). ZERO sites branch on the value.

**upstream** — `pi/packages/coding-agent/src/core/tools/edit.ts:310` `renderShell: "self"` — the only built-in that sets it; the field is declared at `core/extensions/types.ts:465`.

**Impact** — No built-in consequence today. The residual cost is entirely on guests: a `renderShell: "self"` extension tool is double-framed with no diagnostic, because the value has no wire and no consumer.

**Fix** — `EditTool::render_kind` returns `ToolRenderKind::SelfRendered` (trivial, and makes the declaration honest), then give the value a consumer as part of TOOL-022's WIT bump — `cyrup-tui` should branch on the kind rather than on the run name, so guests get the same treatment `edit` now gets by hard-coding.

**Verify** — Unit assertion that `EditTool::render_kind()` is `SelfRendered` and every other built-in is `Default`; a TUI snapshot over a guest tool declaring `renderShell: "self"` showing no outer frame (blocked on TOOL-022).

## TOOL-016 — `constrainedSampling` has no representation in cyrup's tool model

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — `grep -rni constrained crates/` returns only doc comments: `crates/cyrup-provider/src/api/bedrock_converse_stream.rs:44` explicitly states "cyrup's [`ToolDef`] has no such field", plus four unrelated hits (`cyrup-ext-subagents/src/prompt_runtime.rs:25`, `:882`, `:2091` and `cyrup-ext/src/host/engine.rs:30`). `cyrup_core::Tool` (`crates/cyrup-core/src/tool.rs:88-160`, read in full) has slots for execution_mode / description / label / prompt_snippet / prompt_guidelines / render_kind / prepare_arguments / render_call / render_result / execute — none for constrained sampling. The WIT `tool-descriptor` record (`crates/cyrup-ext/wit/world.wit:39-48`) is still the 8-field shape.

**upstream** — `pi/packages/coding-agent/src/core/extensions/types.ts` `constrainedSampling?: false | ConstrainedSamplingConfig`, plumbed at `tool-definition-wrapper.ts:14` and `:42`.

**Impact** — Zero built-in behavior gap today — no pi built-in sets `constrainedSampling`. It becomes real only when an extension wants grammar-constrained tool output, which cyrup then cannot express at all.

**Fix** — Add the config type to `cyrup-core/src/tool.rs`, a `constrained-sampling` field to BOTH WIT copies and both descriptors, and honour it where tool schemas reach the provider (`crates/cyrup-agent/src/agent.rs:714`). Cheapest if folded into TOOL-022's ABI break rather than paid twice.

**Verify** — A guest tool declaring a constrained-sampling config; assert the config appears in the emitted `ToolDef` reaching the provider. Correctly deprioritized until an extension needs it.

## TOOL-017 — `read`'s compact call rendering: the `docs` classification arm is unported (residual)

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** high

> **Partially closed.** Two of pi's three classification arms are ported, and cyrup is already ahead of its own baseline on the v0.84.1 name-set widening. Owner remains cyrup-tui; retained here for traceability. The residual is **PARITY-GAPS PB-4**.

**cyrup** — `crates/cyrup-tui/src/transcript.rs:2265-2305` `compact_read_classification` ports `getCompactReadClassification` with the `skill` arm (`:2283-2295`) and the `resource` arm (`:2296-2305`); `compact_read_call` below it ports `formatCompactReadCall`; it is selected at `transcript.rs:1701` only when not expanded. `COMPACT_RESOURCE_FILE_NAMES` at `transcript.rs:2234-2235` lists `AGENTS.override.md` first, byte-matching v0.84.1's widened set. The missing arm is `docs`: `grep -rn 'readme_path\|getReadmePath' crates/` is empty, and the absence is documented in-source at `transcript.rs:2258-2264`.

**upstream** — `pi/packages/coding-agent/src/core/tools/read.ts:103-120` `getPiDocsClassification`, resolving the candidate against `dirname(getReadmePath())`; `:122-143` `getCompactReadClassification`; `:145-167` `formatCompactReadCall`; selected at `:336`. `read.ts:42` is the five-name `COMPACT_RESOURCE_FILE_NAMES` (four at v0.83.0 — cyrup already carries the fifth).

**Impact** — A collapsed `read` of a file inside cyrup's own shipped documentation shows a raw path where pi shows a `docs` label. Cosmetic, and confined to one of three arms.

**Fix** — Blocked on a decision, not on code: cyrup needs a `getReadmePath()` analog — a packaged-docs root locator — before the arm can be written. Once that exists, the arm is ~20 lines next to `compact_read_classification` and the in-source note at `transcript.rs:2258-2264` should be deleted.

**Verify** — TUI snapshot over a collapsed `read` of a file under the packaged-docs root showing the `docs` label, and of an ordinary source file showing the plain path.

## TOOL-018 — `edit` fuzzy matcher returns not-found where pi returns duplicate-occurrences

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-tools/src/tools/edit_diff.rs:296-298` guards with `match (fuzzy_old.is_empty(), fuzzy_content.find(&fuzzy_old))`, falling to the not-found arm at `:302` when the normalized needle is empty; `count_occurrences` returns 0 for an empty needle at `:310-312`. Neither carries a `[CYRUP-DELTA]`.

**upstream** — `pi/packages/coding-agent/src/core/tools/edit-diff.ts:222` `const fuzzyIndex = fuzzyContent.indexOf(fuzzyOldText);` — JS `indexOf("")` returns 0 (found); `:254` `return fuzzyContent.split(fuzzyOldText).length - 1` yields one element per code unit, far above 1, so pi raises the DUPLICATE-occurrences error at `:332`.

**Impact** — Both sides reject a literally-empty `oldText` up front, so this is reachable only when `oldText` is non-empty but NORMALIZES to empty — i.e. entirely whitespace. There cyrup returns `Could not find the exact text in {path}…` where pi returns the duplicate-occurrences error, giving the model different remediation advice in a rare case.

**Fix** — The cheapest resolution is documentation, not code: keep the guards and mark them an intentional `[CYRUP-DELTA]` in the `edit_diff.rs` module comment, naming the reachability condition, so this is not re-filed a fifth time. Otherwise mirror pi's semantics exactly at `:296-312`.

**Verify** — `edit` with `oldText = "   "` on a file containing whitespace: assert the chosen error (documented delta, or the pi-shaped duplicate message).

## TOOL-024 — `every_surface_method_delegates` proves nothing for 9 of its 11 assertions

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** high

> **Severity corrected medium→low this pass.** The facts are exact and unchanged; the rating is not. A vacuous assertion has no user-visible consequence, which is the README's severity axis, and the sibling test-defect TOOL-025 was rated low on identical reasoning. The auditor's amplifying claim — that `wrapper.rs:107-111` is "the only forwarding path" by which built-in metadata reaches the prompt builder — **could not be confirmed**: the prompt builder reads `tool.prompt_guidelines()` straight off the registered `Arc<dyn Tool>` at `crates/cyrup-session-svc/src/builder.rs:1606`, and it was not established that built-ins pass through `wrap_registered_tool` at all. Establishing that is a prerequisite for re-raising the severity.

**cyrup** — `crates/cyrup-ext/src/wrapper.rs:278-294`. The inner tool is `Fixed` (`wrapper.rs:180-209`), which overrides only `name` (`:187`), `parameters` (`:190`) and `execute` (`:193`); every other method falls to the `cyrup_core::Tool` default. Of exactly eleven assertions, nine compare a default against itself — `description()`, `label()`, `prompt_snippet()`, `prompt_guidelines()`, `render_kind()`, `execution_mode()`, `render_call()`/`render_result()` against literal `None`, and `prepare_arguments()` asserted to be the identity. Only `name` and `parameters` discriminate. Nine of eleven pass identically whether `RegisteredTool` forwards to `self.inner` (`wrapper.rs:87-121`) or omits the override entirely.

**upstream** — `pi/packages/coding-agent/src/core/extensions/wrapper.ts` `wrapRegisteredTool` spreads the whole definition (`{...tool, execute: instrumented}`), so forwarding is structural and cannot regress. cyrup's hand-written per-method delegation can, which is why the guard matters here and not upstream.

**Impact** — No user-visible consequence today. The cost is future: a regression that silently drops a forwarder is invisible to the suite, and the forwarder set is exactly the metadata TOOL-001/TOOL-003/TOOL-008 spent three passes restoring.

**Fix** — Give `Fixed` non-default metadata: distinct `description`, `label`, `prompt_snippet`, `prompt_guidelines`, `render_kind: SelfRendered`, `execution_mode: Sequential`, `render_call`/`render_result` returning `Some(..)`, and a `prepare_arguments` that mutates the value.

**Verify** — After the change, comment out any single forwarder in `wrapper.rs:87-121` and confirm the test goes red; today deleting nine of them leaves it green.

## TOOL-025 — The mutation-lock concurrency test is probabilistic and misnamed (residual)

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** high

> **Partially closed.** The vacuity this item was filed for is genuinely repaired; what remains is a naming and strength problem.

**cyrup** — Repaired half: `crates/cyrup-tools/tests/tools.rs:375-421` — the two racing payloads are now `"AAAA"` (`:395`) and `"BBBBBBBBBBBBBBBB"` (`:407`), DIFFERENT lengths, and the comment at `:380-388` records why this now bites (the backend is an in-place `O_TRUNC` write per TOOL-004, so any interleaving leaves a short read, a tail, or a truncated prefix and the disjunction assertion at `:417-420` can genuinely fail). The two cases the item asked for landed as separate files rather than folded in: `crates/cyrup-tools/tests/cross_registry_mutation_lock.rs:208` is the EDIT read-modify-write case, `:144` and `:262` the cross-registry and different-file cases. Residual: the test still names "serializes" while observing no ordering, and its guarantee is probabilistic — an interleaving that happens not to occur on a given run leaves it green.

**upstream** — `pi/packages/coding-agent/src/core/tools/file-mutation-queue.ts:4` module-level map; `write.ts:212` / `edit.ts:316` wrap the whole read-modify-write in `withFileMutationQueue`. The property that actually needs guarding is mutual exclusion, not the surviving byte pattern.

**Impact** — None to a user. The risk is a false green: the lock could be weakened and this particular test might still pass.

**Fix** — Either rename it to describe what it observes (`write_creates_dirs_and_leaves_one_whole_payload`) or make it structural — a shared counter incremented on entry and decremented on exit inside the guarded region, asserting max concurrency 1, the technique `lock.rs:164-187` already uses at the primitive level.

**Verify** — The rewritten test must fail with the `guard()` call removed from `write.rs:83` on every run, not merely on unlucky ones.

## TOOL-026 — `bash_timeout_fractional_seconds` asserts a wall-clock upper bound it cannot control

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** medium (analytic)

**cyrup** — `crates/cyrup-tools/tests/tools.rs:769-790`, unchanged. Runs `sleep 30` with `"timeout": 2.5` (`:776`) and asserts at `:785-789` `elapsed >= Duration::from_millis(2300) && elapsed < Duration::from_millis(4000)`. The lower bound is safe. The upper bound leaves only ~1.5s of slack for scheduling, the SIGTERM→grace→SIGKILL escalation and process reaping, under an arbitrarily loaded `cargo test --workspace`. The load-independent part — `msg.contains("Command timed out after 2.5 seconds")` at `:784` — is what actually pins the float-seconds parsing (`resolve_timeout_ms`, `crates/cyrup-tools/src/tools/bash.rs:36-47`) and needs no timing at all.

**upstream** — Not a parity question: the ported behavior is correct. Same class as TOOL-020 and TOOL-030.

**Impact** — A low-probability intermittent failure in the repo's only gate. Materially less likely to trip than TOOL-020 (1.5s slack versus ~15ms), which is why it is low — do not let it displace TOOL-020 in priority.

**Fix** — Keep the message assertion and the `>= 2300ms` lower bound (which proves the 2.5s value was honoured rather than a default), and either drop the upper bound or widen it to something no realistic load can exceed (e.g. 15s, still far below the 30s sleep, so it still proves the timeout fired).

**Verify** — Run the file under `--test-threads=32` alongside a loaded workspace build; analytic only this pass, since cargo was forbidden.

## TOOL-029 — `ls` swallows pi's `Cannot read directory: <message>` and emits the raw io-error wrapper instead

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-tools/src/tools/ls.rs:86` is still `let mut entries = self.fs.read_dir(&abs).await?;` — the `?` propagates whatever `FsOps::read_dir` produced, which for `LocalFs` is `error::io(&error::show(path), &e)`, formatting as `"<path>: <io::Error Display>"`. The two sibling failure modes in the same function ARE pi-shaped: `Path not found: {}` at `ls.rs:81` and `Not a directory: {}` at `:83`. The readdir branch is the only one that is not.

**upstream** — `pi/packages/coding-agent/src/core/tools/ls.ts:147-152`: `try { entries = await ops.readdir(dirPath); } catch (e: any) { reject(new Error(\`Cannot read directory: ${e.message}\`)); return; }` — a third distinct stable prefix the model can pattern-match on, separate from `Path not found:` and `Not a directory:`.

**Impact** — An `ls` of a directory that exists and is a directory but cannot be enumerated (mode `0300`, an unmounted/EIO path, a permissions-stripped `.git/objects`) returns a Rust-flavoured message with no `Cannot read directory:` prefix. The model reads it as an unclassified failure rather than a permissions problem. Narrow reachability keeps this low.

**Fix** — Wrap the call at `ls.rs:86`: `self.fs.read_dir(&abs).await.map_err(|e| error::invalid(format!("Cannot read directory: {e}")))?`, matching the surrounding style. If TOOL-014's errno mapping lands first, reuse it so `{e}` renders Node-shaped rather than as the Rust wrapper.

**Verify** — Create a directory, `chmod 0300` it (readdir denied, traversal allowed), run `LsTool`, assert the error contains `Cannot read directory:` and not the bare path-prefixed form. Skip on non-unix and when running as root.

## TOOL-030 — `exec_pre_cancelled_never_spawns` carries a 200ms wall-clock upper bound it does not control

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** medium (analytic)

**cyrup** — `crates/cyrup-tools/src/ops/local.rs:815-831`. The attribute at `:815` is a plain `#[tokio::test]` (single-threaded current-thread runtime), so the test task shares one thread with the runtime — if anything more exposed, not less. After asserting the pre-cancelled `exec` returns `ExitStatus::Killed`, it asserts at `:827-831` `started.elapsed() < Duration::from_millis(200)` with the message "must short-circuit before spawning, not pay real process start/teardown latency" — a wall-clock ceiling on work the test cannot schedule.

**upstream** — Not a parity question: the behaviour under test is correct — `LocalProc::exec`'s pre-spawn `if cancel.is_cancelled() { return Ok(ExitStatus::Killed); }` at `local.rs:439-441` mirrors pi's `bash.ts:91-93`. Filed purely under the defect class the project keeps finding.

**Impact** — A low-probability intermittent failure in the repo's only gate. Kept low deliberately (200ms for two awaits is far more slack than TOOL-020's ~15ms for a fork+exec).

**Fix** — Prove no-spawn deterministically rather than by latency. Note that simply deleting the timing assertion and keeping `!marker.exists()` is NOT a sufficient substitute: a run that DID spawn `sh -c 'touch …'` and was killed before the `touch` completed also leaves the marker absent, so that would strictly weaken the test. Instead (a) instrument the path — have `exec` return a distinguishable pre-spawn short-circuit, or use a `ProcOps` test double that records spawn attempts and assert zero; or (b) if a latency guard is still wanted alongside, widen it to a figure no realistic load can reach.

**Verify** — With the test double in place the no-spawn invariant is assertable with zero timing. Until then, only running the suite under artificial load can observe whether the 200ms bound trips; analytic only, since cargo was forbidden this pass.

## TOOL-031 — `bash` stamps no agent-identity variable into the child environment — neither `PI_CODING_AGENT` nor v0.84.1's `AI_AGENT`

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** high

> **Severity corrected medium→low this pass.** The two-sided facts survive intact. The rating does not: neither variable is consumed anywhere inside pi itself (`git grep` at v0.84.1 finds only the two assignment sites), so this is a third-party-compatibility marker rather than behaviour the agent's own output depends on, and the "scripts block on a prompt no one can answer" impact is speculative — the conventional guards are `CI`/tty checks.

**cyrup** — `crates/cyrup-tools/src/tools/bash.rs:152-179` assembles the ENTIRE child environment vector explicitly — `let mut env = shell_env(self.opts.bin_dir.as_deref());` at `:152`, the scrub list at `:153`, then five conditional `env.push(("CYRUP_SESSION_ID", …))`-style pushes at `:161-177` — and never pushes an agent-identity key. `crates/cyrup-tools/src/config.rs:31-48` covers only the five session-metadata names. `grep -rn 'PI_CODING_AGENT\|AI_AGENT' crates/` returns only `crates/cyrup-config/src/env.rs:68-69`, the unrelated `PI_CODING_AGENT_DIR` / `PI_CODING_AGENT_SESSION_DIR` directory fallbacks. `crates/cyrup/src/main.rs:53-57` documents declining to replicate it because `std::env::set_var` is `unsafe` under edition 2024 — a rationale that covers only process-GLOBAL mutation, not a per-child env vector.

**upstream** — `pi/packages/coding-agent/src/cli.ts:13` @v0.83.0 `process.env.PI_CODING_AGENT = "true";` — present at the ported tag, so this is not version lag. @v0.84.1 the same file adds `:14` `process.env.AI_AGENT = "pi";`, mirrored in `rpc-entry.ts:7-8`. Because pi sets them on `process.env` before `main()`, every child pi spawns inherits both — including every `bash` child, since `createLocalBashOperations` passes `env: env ?? getShellEnv()` (`bash.ts:105`) and `getShellEnv()` is `{...process.env, PATH}` (`utils/shell.ts:130-133`).

**Impact** — A shell hook, npm script, git hook, Makefile or MCP server launched from the model's `bash` tool cannot detect that it is running inside an agent. Scripts written against the `$AI_AGENT` / `$PI_CODING_AGENT` convention — the documented way to suppress interactive prompts, pagers and colour — behave as if launched by a human. Silent: nothing in the output says the variable is missing.

**Fix** — Push the keys in `BashTool::execute` right after `let mut env = shell_env(...)` (`bash.rs:152`), unconditionally and independent of `expose_session_environment` (pi sets them process-wide, not behind that flag): `CYRUP_CODING_AGENT=true` plus `PI_CODING_AGENT=true` for script compatibility, and `AI_AGENT=cyrup`. No `unsafe` is required — this is the per-child env vector, not `std::env::set_var`. pi does not scrub them, so do not add them to `session_env_scrub_keys()`. The same two lines belong on the immediate-bash seam (`run_bash`'s `ExecSpec` at `crates/cyrup-session-svc/src/bash.rs:96-102`) and on the subagent re-exec, since pi's process-global set covers those too — that second half is **PARITY-GAPS PB-5** and is owned by whoever owns `crates/cyrup/src/main.rs` and the subagent runner.

**Verify** — Assert `bash 'echo ${AI_AGENT:-ABSENT} ${PI_CODING_AGENT:-ABSENT}'` returns neither `ABSENT`, with `BashOpts { expose_session_environment: false, ..Default::default() }` as well as the default, to pin that the flag does not gate them. Red today (both ABSENT).

## TOOL-032 — `FileMutationLocks::key` blocks a runtime thread on `std::fs::canonicalize` and swallows every error kind

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

> **Severity corrected medium→low this pass, and this item IS TOOL-019's residual** — one fix, counted twice only for traceability. The auditor's second impact claim (divergent lock keys silently interleaving two in-place writes into a file matching neither payload) was **refuted**: every realpath failure that is not ENOENT/ENOTDIR — EACCES on a parent's search bit, ELOOP, ENAMETOOLONG — also fails the subsequent `open(2)` in `write_in_place` (`ops/local.rs:81-87`), so cyrup takes a differently-keyed lock and then fails anyway rather than corrupting the file. What survives is the blocking call plus an error-message shape difference.

**cyrup** — `crates/cyrup-tools/src/lock.rs:85-87` `fn key(path: &Path) -> PathBuf { std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()) }` — the BLOCKING std call, with `unwrap_or_else` swallowing every error kind. It is invoked at `lock.rs:96`, the first statement of the async `pub async fn guard(&self, path, cancel)` (`:91-107`), which both mutators await on every call (`crates/cyrup-tools/src/tools/write.rs:83`, `crates/cyrup-tools/src/tools/edit.rs:189`). `tokio::fs::canonicalize` appears nowhere in the crate.

**upstream** — `pi/packages/coding-agent/src/core/tools/file-mutation-queue.ts:16-26` `getMutationQueueKey` is `async` and `await realpath(resolvedPath)` — non-blocking. Its catch is narrow: `:21-22` `if (isMissingPathError(error)) return resolvedPath;` where `isMissingPathError` (`:7-14`) tests only `ENOENT` / `ENOTDIR`; `:24` `throw error` rethrows everything else, which propagates out of `withFileMutationQueue` (`:34`) before `fn()` ever runs, so the write/edit fails loudly and with the realpath errno rather than the later open errno.

**Impact** — Every `write`/`edit` performs a synchronous `realpath(2)` on a tokio worker thread; on an NFS/SSHFS/FUSE mount or a deep symlink chain that stalls the whole runtime — unrelated in-flight tool calls and the provider stream included — for the duration of the syscall. pi never blocks its loop. Secondary and cosmetic: on a non-ENOENT realpath failure the model sees the `open(2)` error rather than pi's realpath error.

**Fix** — In `crates/cyrup-tools/src/lock.rs` make `key` an `async fn` using `tokio::fs::canonicalize` and await it inside `guard()` at `:96`. Narrow the fallback to pi's two error kinds — `ErrorKind::NotFound` and the ENOTDIR equivalent — and return `Err(error::io(...))` for anything else so `guard()` propagates it and `write`/`edit` fail before touching the file, matching `file-mutation-queue.ts:24`. Update the two call sites in `lock.rs`'s own tests (`:171`, `:196`) which use `FileMutationLocks::key` directly. **This closes TOOL-019's residual — do not schedule both.**

**Verify** — Unit: create `<tmp>/dir/f.txt`, `chmod 0000` the parent, call `guard()` and assert it returns `Err` naming the realpath failure rather than silently proceeding to a later open error (skip on non-unix and when running as root). The blocking half is not directly assertable without a slow filesystem; the code-level proof is that `key` is `async` and calls `tokio::fs::canonicalize`.

## TOOL-035 — `read`'s image sniff scans the whole file where pi sniffs only the first 4100 bytes, inverting the APNG verdict for large-header PNGs

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-tools/src/tools/read.rs:118` `let bytes = self.fs.read(&abs).await?;` reads the ENTIRE file, and `:121` `if let Some(mime) = crate::ops::ImageMime::from_magic(&bytes)` hands all of it to the sniffer. `ImageMime::from_magic` (`crates/cyrup-tools/src/ops/mod.rs:85-103`) applies no length bound, and `is_animated_png` (`ops/mod.rs:141-159`) walks the PNG chunk chain to the end of that buffer, returning `true` on the first `acTL` seen anywhere in the file. The doc comment at `ops/mod.rs:80-84` acknowledges the mechanism difference ("Pi opens the file and reads the header; cyrup sniffs the bytes returned by the `FsOps::read` seam") without modelling the window.

**upstream** — `pi/packages/coding-agent/src/utils/mime.ts:3` `const IMAGE_TYPE_SNIFF_BYTES = 4100;` and `:25-34` `detectSupportedImageMimeTypeFromFile` opens the file, reads exactly that many bytes and calls `detectSupportedImageMimeType(buffer.subarray(0, bytesRead))`. `isAnimatedPng` (`:42-55`) therefore walks a ≤4100-byte buffer and bails `return false` at `:51` the moment `nextOffset > buffer.length` — any `acTL` beyond the window is invisible and the file is reported as `image/png`.

**Impact** — A PNG whose chunks before `acTL` exceed the 4100-byte window is classified `image/png` by pi — the image branch runs and the model receives an image block — while cyrup finds the `acTL`, returns `None`, falls through to the TEXT branch at `read.rs:126`, and hands the model `String::from_utf8_lossy` of the binary. Silent wrong output on a normal `read` call: no error, just replacement-character garbage where pi shows a picture. The same window difference can flip the JPEG-LS and BMP structural checks for pathological headers, though those read fixed low offsets and are far less reachable.

**Fix** — Bound the sniff, not the read: at `read.rs:121` pass `bytes.get(..4100).unwrap_or(&bytes)` to `ImageMime::from_magic`, and record the constant next to the existing citation at `ops/mod.rs:80-84` as `IMAGE_TYPE_SNIFF_BYTES` so it is greppable against `mime.ts:3`. Do not bound the read itself — the image branch needs the full bytes to encode.

**Verify** — Construct a PNG with a >4100-byte ancillary chunk (e.g. a large `iTXt`) placed before an `acTL` and before the first `IDAT`. Assert `read` returns an image block with the `Read image file [image/png]` note. Red today (returns the text branch). Keep the existing APNG case — `acTL` inside the first 4100 bytes — asserting the text branch, so the fix does not simply disable the animation check.

## TOOL-036 — `resolve_to_cwd` omits the whole win32 leg of pi's `normalizePath`: `~` resolves off `$HOME` only, and Git Bash / MSYS / Cygwin / WSL drive paths are not converted

**Kind** parity-bug *(the `~`/`homedir` half; the shell-path half is `upstream-drift` — see below)* · **Severity** low · **Effort** S · **Confidence** high

> **Classification corrected, 2026-08-12 repair pass.** The cross-tag sweep this file ran was scoped
> to `packages/coding-agent/src/core/tools/`; `utils/paths.ts` is not under that path and was
> therefore never diffed across the tags. It is **not** tag-invariant: `git diff --numstat v0.83.0
> v0.84.1 -- packages/coding-agent/src/utils/paths.ts` is `22 1`, and **`normalizeWindowsShellPath`
> does not exist at v0.83.0** — at the ported tag the only `process.platform === "win32"` test in the
> file is the `~\` spelling check inside `normalizePath` (v0.83.0 `:69`). So this item is **two
> defects with two different classifications**, and only one of them is baseline debt:
> • the **`~` / `os.homedir()`** half **is** a `parity-bug` — `homedir()` is present at v0.83.0
> (`paths.ts:67`, v0.84.1 `:88`) and cyrup never ported it. This half carries the item's kind.
> • the **Git Bash / MSYS / Cygwin / WSL drive-path conversion** half is **`upstream-drift`**, landed
> in the v0.83.0..v0.84.1 window (v0.84.1 `paths.ts:67-73` `normalizeWindowsShellPath`, called from
> `:83-85`). Do not schedule it as a port regression; it is a v0.84.1 feature cyrup has not caught
> up to, and it belongs with whatever else that window brings in.
> Severity is unaffected — both halves land on the same Windows-only surface and the whole item is
> still gated on the Windows-scope question in **Verify**.

**cyrup** — `crates/cyrup-tools/src/path.rs:91-93` `fn home_dir() -> Option<PathBuf> { std::env::var_os("HOME").map(PathBuf::from) }` — the only home source. `expand_home` (`:73-89`) explicitly handles the Windows `~\rest` form at `:80-83`, a branch that can essentially never fire usefully because `HOME` is normally unset on Windows, so `~` and `~\...` fall through to `:88`. `resolve_to_cwd` (`:161-178`) then joins that literal `~` onto the cwd at `:176`. There is no counterpart to `normalizeWindowsShellPath` anywhere in the file — the pipeline at `:164-171` is unicode-spaces → strip `@` → expand `~` → `file://` → lexical resolve, with no platform branch. Every built-in routes through this: `read.rs:90`, `write.rs:81`, `edit.rs:188`, `ls.rs:75`, `grep.rs:119`, `find.rs:100`.

**upstream** — `pi/packages/coding-agent/src/utils/paths.ts:2` imports `homedir` from `node:os` and uses it at **`:67` at v0.83.0** (v0.84.1 `:88`) `const home = options.homeDir ?? homedir();` — `os.homedir()` resolves `USERPROFILE` on Windows, not `HOME`. That half is present at the ported tag. The platform branch, by contrast, exists **only at v0.84.1**: `:83-85` `if (process.platform === "win32") { normalized = normalizeWindowsShellPath(normalized); }`, whose body at `:67-73` matches `/^\/(?:mnt\/|cygdrive\/)?([a-z])(?:\/(.*))?$/i` and converts `/c/...`, `/cygdrive/c/...` and `/mnt/c/...` into `C:\...`. `git show v0.83.0:packages/coding-agent/src/utils/paths.ts | grep -n normalizeWindowsShellPath` returns nothing, and the file's only v0.83.0 `win32` test is the `~\`-prefix check at `:69`. `resolveToCwd` (`path-utils.ts:48-50`) is a thin wrapper over that same `normalizePath` at both tags, so all seven pi tools get whichever behaviours the tag has.

**Impact** — On Windows every tool that takes a path is affected. `read("~/notes.md")` resolves to a literal `<cwd>\~\notes.md` and fails with a not-found error where pi reads the file; likewise for `write`, `edit`, `ls`, and the `path` argument of `grep`/`find`. Separately, a path pasted from a Git Bash / WSL / Cygwin shell — the normal way a Windows user of a bash-flavoured agent produces paths, and exactly what the `bash` tool's own output looks like there — is passed through unconverted and fails, where pi rewrites it to a native drive path. Both are hard, loud failures on a target the crate otherwise compiles `cfg!(windows)` branches for.

**Fix** — In `crates/cyrup-tools/src/path.rs` replace `home_dir()` (`:91-93`) with a resolver that mirrors `os.homedir()`: `USERPROFILE`, then `HOMEDRIVE`+`HOMEPATH`, falling back to `HOME` on unix. Add a `normalize_windows_shell_path` port of `paths.ts:67-73` and call it from `resolve_to_cwd` between the `@`-strip at `:165-167` and the tilde expansion at `:168`, guarded by `cfg!(windows)` to match pi's placement exactly — the order matters, pi converts the drive path BEFORE expanding `~`.

**Verify** — `#[cfg(windows)]` unit tests in `path.rs`'s existing module: assert `resolve_to_cwd("~/notes.md", cwd)` starts with the `USERPROFILE` value; assert `resolve_to_cwd("/c/Users/x/f.txt", cwd) == PathBuf::from("C:\\Users\\x\\f.txt")` and the same for the `/cygdrive/c/` and `/mnt/c/` spellings. Assert the existing unix tests at `path.rs:218-351` are unaffected. **Prerequisite decision:** nothing in this workspace can execute the win32 branch and it was not established whether cyrup ships Windows CI or claims the target formally. If Windows is out of scope by policy, strike this item rather than downgrading it — and strike TOOL-038/TOOL-039's Windows halves with it.

## TOOL-037 — `output-guard`'s protocol-writer half is unported: no blocked-write retry, no serialized write tail, no backpressure gate

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** medium

> **Severity corrected medium→low this pass, and the impact story was refuted.** pi needs its retry loop because libuv puts Node's stdout in non-blocking mode for pipes. Rust's `std::io::Stdout` is blocking, `write_all` already loops partial writes and retries `Interrupted`, and a blocking write IS the backpressure gate — so "macOS pipes routinely emit a truncated JSON line" is not reachable on a blocking fd. Concurrent interleaving is likewise impossible: the modes hold the sink as `&mut W`. What survives is hardening against an **inherited `O_NONBLOCK` stdout**, which a parent process can hand cyrup.

**cyrup** — `crates/cyrup/src/output_guard.rs` (read in full, 114 lines) ports only the takeover half — `take_over_stdout` (`:35`), `restore_stdout` (`:42`), `is_stdout_taken_over` (`:47`), `emit_stray`/`emit_stray_line` (`:72`, `:81`) — and its module doc at `:18-22` states the protocol path is instead "an injected `Write` sink which `main` binds to `std::io::stdout()`". That sink is a plain synchronous `std::io::Write`: `crates/cyrup-modes/src/json.rs:58-59` and `:80-81` are `writeln!` + `flush()?`. `grep -rn 'write_raw_stdout\|flush_raw_stdout\|backpressure' crates/` is empty. `is_stdout_taken_over` has no non-test consumer (only `output_guard.rs`'s own tests at `:96-110` and the re-export at `crates/cyrup/src/lib.rs:49`).

**upstream** — `pi/packages/coding-agent/src/core/output-guard.ts:9` `RAW_STDOUT_RETRY_DELAY_MS = 10`; `:20-43` `writeRawStdoutChunk` loops forever, retrying on `ENOBUFS`/`EAGAIN`/`EWOULDBLOCK` after a 10 ms sleep (`:37-41`) and rethrowing anything else (`:38`); `:11` + `:85-93` `writeRawStdout` chains every write onto the module-level `rawStdoutWriteTail` promise and `process.exit(1)` on a genuine failure; `:95-103` `waitForRawStdoutBackpressure` and `:105-108` `flushRawStdout`, awaited at `print-mode.ts:116`/`:167` and `rpc-mode.ts:362`/`:738`/`:760`/`:785`.

**Impact** — If a parent hands cyrup a stdout fd that is already `O_NONBLOCK` (a supervisor using a non-blocking pipe, some test harnesses, an inherited descriptor), a large `--mode json` or print event returns `WouldBlock` from `write_all` mid-line, emitting a truncated JSON line the peer cannot parse and then propagating an error out of the mode. pi retries until the byte lands regardless of fd mode. On a normal blocking fd there is no difference.

**Fix** — Add `write_raw_stdout(text)` / `flush_raw_stdout()` to `crates/cyrup/src/output_guard.rs` wrapping the real stdout handle: loop on `ErrorKind::WouldBlock` (and the `ENOBUFS` raw-os-error) with a 10 ms sleep, treat `Interrupted` as retryable, and return an error for anything else — the port of `output-guard.ts:20-43`. Route the injected sink `main` binds for the print/JSON arms through it, and call the flush before each mode returns, matching `print-mode.ts:167`. A mutex around the handle (the analog of `rawStdoutWriteTail`) is optional today since the sink is `&mut W`, but is cheap insurance if a second writer is ever added.

**Verify** — Give the writer a pipe whose read end is not drained and whose write end has `O_NONBLOCK` set, emit an event larger than the pipe buffer, and assert the complete line is eventually written rather than an error being returned. Assert the existing takeover tests (`output_guard.rs:93-113`) stay green.

## TOOL-040 — `find_bash_on_path` runs `which bash` with no timeout; pi bounds the same probe at 5s and degrades to `sh`

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-tools/src/ops/shell.rs:64` `let output = std::process::Command::new(cmd).arg(arg).output().ok()?;` — a synchronous, unbounded wait, reached from `ShellConfig::detect()` (`:112`) whenever `/bin/bash` is absent (Termux, distroless, NixOS images), and `detect()` runs on the async runtime during `ToolRegistry::with_builtins`.

**upstream** — `pi/packages/coding-agent/src/utils/shell.ts:47` `spawnSync("which", ["bash"], { encoding: "utf-8", timeout: 5000 })`, with the Windows `where` equivalent at `:28-32` also carrying `timeout: 5000`; pi gives up after 5s and falls through to `{ shell: "sh", args: ["-c"] }` at `:119`.

**Impact** — A `which` that hangs on a stale NFS/automount PATH entry wedges cyrup's session construction indefinitely, where pi starts with `sh` after five seconds. Additionally the blocking `output()` runs on a runtime thread, the same class of defect as TOOL-032.

**Fix** — Bound the probe: spawn with `std::process::Command` + `wait_timeout`-style polling, or move the whole `detect()` probe behind `tokio::task::spawn_blocking` with a `tokio::time::timeout(Duration::from_secs(5), …)`, falling through to the `sh` arm on expiry exactly as `shell.ts:119` does.

**Verify** — Point `PATH` at a directory containing a `which` shim that sleeps 30s, call `ShellConfig::detect()`, assert it returns the `sh` configuration within ~5s. Skip on non-unix.

## TOOL-041 — `write`/`edit` never re-check cancellation after the write lands; pi's `throwIfAborted()` runs after `writeFile`

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-tools/src/tools/write.rs` checks cancellation exactly once, at `:84-86`, between acquiring the mutation guard (`:83`) and `self.fs.write_in_place(&abs, bytes)` (`:89`); there is no check between `:89` and the success `ToolResult` at `:94-104`. `edit` has the same asymmetry (`edit.rs:214-216` checks before the write at `:221`, never after).

**upstream** — `pi/packages/coding-agent/src/core/tools/write.ts` brackets the same sequence three times — `:217` before `ops.mkdir(dir)` (`:219`), `:220` before `ops.writeFile` (`:223`), and `:224` immediately after it — with `throwIfAborted` defined at `:213-215` to throw `"Operation aborted"`. `edit.ts` carries the matching post-write check.

**Impact** — A cancel that arrives while the bytes are being written yields a `Successfully wrote N bytes` result in cyrup and an aborted tool error in pi, even though both leave the same file on disk. The transcript and the session record then disagree with the user's own cancellation, and a supervising subagent sees a success it did not get.

**Fix** — Add the post-write check in both mutators: after `write_in_place` at `write.rs:89` and `edit.rs:221`, `if cancel.is_cancelled() { return Err(error::aborted()) }` before constructing the success result, matching `write.ts:224`. Keep the pre-write checks; pi has both.

**Verify** — Fire a `write` whose backing `FsOps` blocks inside `write_in_place` until the test cancels, then completes. Assert the tool returns the aborted error rather than a success, and assert the file on disk still contains the payload (pi's semantics: the write is not undone, only the result is reported as aborted).

## TOOL-M01 — Both `FsOps` decorators forwarded `detect_image_mime` but nothing could see it — **FILED AND CLOSED 2026-08-14**

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed and closed** 2026-08-14 (sweep 8)

**Provenance** — the one real residual of the `FsOps` half of sweep 8's assigned audit of hand-written
delegating trait impls (the audit that produced `PROV-M01`). **Both decorators were already complete
(8/8 methods) and well-documented**, and `read_stream` — the more dangerous of the trait's two
defaulted methods — was already pinned by a distinct-value probe. This row is the other one.

**cyrup** — `detect_image_mime` was forwarded by both decorators (`isolation/protected.rs:145-147`,
`isolation/traversal.rs:123-125`) but had **no test**, so deleting either forward would have been
silent. The trait default is extension-based (`cyrup-tools/src/ops/mod.rs:363-365`), which is a
*plausible* answer for every fixture whose inner leaves it alone — the `PROV-M01` hazard exactly.

**upstream** — pi's operation decorators are object literals (`{ ...ops, writeFile }`,
`core/tools/write.ts:32-35` / `edit.ts:83-87` @v0.83.0), so the member survives by construction and
upstream has no equivalent exposure. The existing `read_stream` test's doc block already cites this
correctly.

**Fix — LANDED.** Extended the existing `DistinctStreamFs` probe in
`crates/cyrup-tools/src/tests/isolation.rs` with a `detect_image_mime` override that **contradicts the
default in BOTH directions**: a `.txt` path (default `None`) reports `Some(Png)`, and a `.png` path
(default `Some(Png)`) reports `None`.

**Verify — DONE.**
`fs_decorators_forward_detect_image_mime_instead_of_inheriting_the_extension_default` — presence
before absence on the probe itself (it must disagree with the default in both directions before any
decorator is asserted), then all three decorator configurations: `TraversalFs`, `ProtectedFs`, and the
stacked composition.

## TOOL-042 — An intermittent nextest `LEAK` fails the `cyrup-tools` suite — **REOPENED 2026-08-14 (sweep 8); PARTIALLY CLOSED**

**Kind** test-defect · **Severity** medium · **Effort** M · **Confidence** confirmed by measurement · **Filed** 2026-08-14 (sweep 6, as closed) · **REOPENED** 2026-08-14 (sweep 8)

> **READ THIS FIRST. Sweep 6's caveat below set its own falsification condition — "Confirming it
> needs ~35 runs … If a LEAK still appears the fd-inheritance theory is wrong" — and sweep 8 ran the
> experiment. The LEAK still appears.** 286 runs of `cargo nextest run -p cyrup-tools` with the
> ambient env scrubbed, in five blocks:
>
> | block | shape | runs | LEAKs | named test |
> |---|---|---:|---:|---|
> | A | idle box, first after build | 1 | **1** | `tests::tools::bash_timeout_at_maximum_is_valid` |
> | B | sequential, idle (the row's own ask) | 35 | 0 | — |
> | C | 25 concurrent pairs | 50 | **1** | `ops::local::tests::terminate_pid_reports_true_and_the_real_process_dies` |
> | D | + a 100 ms orphan/`lsof` sampler | 80 | 0 | — |
> | E | + a snapshot fired reactively on the LEAK line | 120 | **1** | `ops::bash_operations_tests::local_bash_operations_forwards_command_cwd_and_env_onto_the_proc_seam` |
> | | | **286** | **3 (~1.0%)** | **three DIFFERENT tests** |
>
> **The rate fell (from ~4 in 33 ≈ 12%), so the source-scan pin closed a real class — keep it. The
> failure did not go away, so the row does not close.** `fail-fast` CANCELS the run on a LEAK
> (block A: `243 tests run: 242 passed, 1 failed`), so this is a hard gate red, not a warning.
>
> **The mechanism is refuted for the one occurrence that was instrumented.** Block E's LEAK names
> `local_bash_operations_forwards_command_cwd_and_env_onto_the_proc_seam`, which drives
> `RecordingProc`, an **in-memory double** (`ops/mod.rs:605`). Its only possible child is
> `find_bash_on_path`'s `which bash` (`ops/shell.rs:78-84`), which names all three stdio handles and
> is reaped in-loop. **There is no candidate holder of that test process's fd 1/2 anywhere in its
> subtree**, and the wait ran the full 500 ms (`LEAK-FAIL [0.513s]`). Corroborating over block D: **69
> distinct orphan pipe addresses vs 244 sampled `cargo-nextest` pipe addresses — zero intersection.
> No orphan was ever seen holding a harness pipe.**
>
> **Stray children DO exist, and they are a separate finding.** Under two concurrent suites the box
> carries orphaned `sleep 30`s (ppid 1, fd0 `/dev/null`, fd1+fd2 PIPE), one alive for its full 00:30
> — the `exec`/`exec_argv` fixtures' backgrounded descendants (`ops/local.rs:1135`, `:1568`, `:1812`)
> holding **the tool's** pipes, not the harness's. After a single idle run there are **zero** orphan
> sleeps at t+1 s … t+32 s. Hygiene lead, not the leak.
>
> **RESIDUAL — a harness-level question, and the next instrument is named so nobody re-derives it.**
> Does `cargo nextest`'s leak detector time out under saturation? `.config/nextest.toml`'s 500 ms
> tripwire was **deliberately not touched** — raising it would hide the signal and is the user's call.
> **Any future closure of this row has to name a holder or name the harness.** Nothing was weakened to
> reach green: leak-timeout untouched, no `#[ignore]`, no retries.
>
> **MEASUREMENT HYGIENE, disclosed rather than smoothed:** the runs straddled other agents' in-flight
> edits — `crates/cyrup-tools/src/tests/isolation.rs` gained a test mid-measurement (243 → 244 tests
> between blocks C and D) and the binary was built from a working tree carrying 34 modified files,
> though the two files the live rows measure were unmodified vs HEAD.
>
> **Everything below is sweep 6's closure argument, retained unedited.** Its fd algebra and its two
> pins are still correct and still load-bearing; only its *sufficiency* is refuted.

**Mechanism — fd algebra, not a bad assertion.** nextest hands each test process a pipe for fd 1/2 and waits `leak-timeout` (500 ms, `.config/nextest.toml:42`) for EOF *after the process exits*; EOF requires every copy of the WRITE end to be closed. `std::process::Command` defaults every **unnamed** handle to `Stdio::inherit()`, and naming a handle `dup2`s over the harness's copy — so **only a spawn that omits a handle can hold the harness pipe open**. `git show c8c86bc:crates/cyrup-tools/src/ops/shell.rs` shows exactly one such site in the whole crate: `path_probe_is_bounded` spawned `sleep 30` naming only `.stdout(piped())`, handing the child the harness's **stdin and stderr**, and its `Err(_) => break` arm left that child alive, unkilled and unreaped for 30 s. Sweep 5 (`bdcb0d0`) fixed both halves at `ops/shell.rs:339-378`.

**Re-audit at HEAD** — all 8 `Command::new(` sites in the crate (`shell.rs:80`, `:348`; `local.rs:275`, `:321`, `:458`, `:488`, `:653`, `:1434`) either pin all three handles or terminate in `.output()`.

**Regression pins (new)** — `crates/cyrup-tools/src/tests/no_inherited_harness_stdio.rs`: `every_command_in_this_crate_pins_all_three_stdio_handles` is a deterministic source scan over `crates/cyrup-tools/{src,tests}` that fails if any `Command::new(` window omits `.stdin(`/`.stdout(`/`.stderr(` without ending in `.output()`; `shell_probe_loops_reap_on_the_error_arm_not_just_the_deadline` pins the reap on the ERROR arm. Both assert presence before absence (≥10 files scanned, ≥6 spawn sites matched) so a broken walk cannot pass vacuously. **`.status()` is deliberately NOT exempted — it inherits stdout/stderr; only `.output()` overrides all three.** Any future lint that exempts "terminal builder methods" as a class will re-open the leak through `.status()`.

**REFUTED sub-claim** — the original report's premise that "the named victim is ARBITRARY and changes every run" is false as stated: nextest runs each test in its own process, so a leaked child holds the pipe of the test that spawned it and the victim can only be that test. The intermittency is explained by `path_probe_is_bounded`'s `Err(_)` arm firing only sometimes (the normal path kills and reaps inside 200 ms), or by the observation having been taken from a `--workspace` run where a different crate leaked.

**CAVEAT — this closure is a static argument plus two pins, NOT an observation.** Sweep 6 was forbidden `cargo nextest`, so the `LEAK` was never re-run. Confirming it needs ~35 runs of `cargo nextest run -p cyrup-tools` with the LEAK count reported. If a LEAK still appears the fd-inheritance theory is wrong and the next instrument is `lsof -p <pid>` on any surviving `sleep`/`bash` — **not** `pgrep -f <pattern>`, which matches its own pattern in other shells' command lines and already fabricated a "22 orphaned brokers" measurement once (`REPRO-LOG.md`, the orphaned-broker entry).


## Findings filed 2026-08-14 (sweep 9 — the mechanical built-in-tool surface enumeration)

Filed by the surface sweep that enumerated **built-in tool names, parameter JSON schemas and result
shapes** on both sides by command rather than by eye — pi `packages/coding-agent/src/core/tools/*.ts`
@`v0.83.0` vs `crates/cyrup-tools/src/tools/`.

> **This surface came back 7/7 CLEAN on almost everything, and that is the useful headline.** Names
> identical and in identical order on both sides (`read`, `bash`, `edit`, `write`, `grep`, `find`,
> `ls`; pi `index.ts:156-166` vs cyrup `registry.rs:20,62-78`, with `createCodingTools` /
> `createReadOnlyTools` reproducing by filtering that order). **Zero** `missingInCyrup` findings. All
> 7 descriptions, all 7 `promptSnippet`s and all 3 non-empty `promptGuidelines` sets byte-match
> except `bash`'s guideline (`TOOL-043`). All 7 schemas byte-match the evaluated TypeBox output.
> Every Rust input struct's field types match its advertised schema type (all numerics are `f64`
> against TypeBox `Type.Number`, so `limit: -1` / `offset: 10.0` deserialize as pi accepts them).
> Result envelopes match (`ToolResult` == `AgentToolResult`: content, details, usage,
> addedToolNames, terminate). All 6 `details` structs match pi's interfaces field-for-field in name
> and optionality; `write` correctly has none. Result-content strings match verbatim, including the
> edit-diff error family (pi `edit-diff.ts:260,264,271,275,281,283,351` vs cyrup
> `edit_diff.rs:462,464,470,472,478,480,546`). **`ToolName` is closed at the same seven on both
> sides** (pi `index.ts:79`), so the ten `impl Tool for` sites outside `cyrup-tools` are a different
> surface with a different ported tag (`pi-subagents`, `pi-intercom`) and are not gaps here.
>
> **One finding from this sweep is already fixed and committed and is deliberately NOT re-filed** —
> `additionalProperties: false` on `edit`'s schema, which pi does not emit, and which the test
> suite's own `PI_EDIT` "ground truth" constant carried too, so the suite was certifying the
> divergence. It was closed inline at all four sites plus the two stale comments that repeated the
> claim.

## TOOL-043 — `bash`'s `promptGuidelines` diverges from the ported tag twice, and neither delta carries a `[CYRUP-DELTA]` tag

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-08-14 (sweep 9)

**upstream** — `packages/coding-agent/src/core/tools/bash.ts:328-331` @`v0.83.0` (snippet `:328`, guideline `:330`). The guideline is the bare imperative `"Inspect PI_* environment variables for current model and session details."`

**cyrup** — `crates/cyrup-tools/src/tools/bash.rs:120-122` emits `"You can inspect CYRUP_* environment variables for current model and session details."`

**Impact** — **Two independent deltas in one string, and the string is model-facing** — it lands in the system prompt's Guidelines section, so it is part of the prompt every bash-capable session ships.

* **(a) `"You can "` is a v0.84.0 upstream rewording** — cyrup is *ahead* of the ported tag here, not behind. It is documented as such at `bash.rs:104-110`, and the reasoning is sound (v0.84.0 softened an instruction into a statement of availability and hoisted the pair into an exported const; only the wording is model-facing).
* **(b) `PI_*` → `CYRUP_*` is a deliberate rename**, justified in-tree at `bash.rs:117-126`: cyrup's `resolveSpawnContext` port injects `CYRUP_SESSION_ID` / `CYRUP_SESSION_FILE` / `CYRUP_PROVIDER` / `CYRUP_MODEL` / `CYRUP_REASONING_LEVEL` and `config::session_env_scrub_keys` unconditionally deletes the five `PI_*` names from the child, so naming `PI_*` would point the model at variables cyrup guarantees are absent. That reasoning is correct and the rename should stand.

**This item is not asking for either to be reverted.** It exists because **the sweep rules admit no accepted-divergence category**: the rationale is present and cites pi's file:line, but neither delta carries a `[CYRUP-DELTA]` tag, which is the mechanism the project uses to *find* accepted divergences. A future parity sweep diffing prompt strings will re-derive this analysis from scratch, exactly as this one did. The gating (`exposeSessionEnvironment`, default true) and the `promptSnippet` are byte-identical, so nothing else on this tool is in question.

**Fix** — Add two `[CYRUP-DELTA]` tags at `bash.rs:104-126`: one marked *version-lag, ahead* naming v0.84.0 for the wording, one marked *deliberate, value only* naming the `PI_*`→`CYRUP_*` rename and the scrub list that forces it. This is the same treatment `TOOL-038` gave `ShellConfig::detect()`'s degradation and `TOOL-031` gave `AI_AGENT`'s value.

**Verify** — `grep -rn '\[CYRUP-DELTA' crates/cyrup-tools/src/tools/bash.rs` finds both; and a test pinning the exact guideline string, so the next edit to it is a deliberate one. (A string-equality test here is not busywork: this string is prompt content, and prompt content that drifts silently is the failure `pi_schema.rs` exists to prevent for schemas.)

## TOOL-044 — The serialized `details.truncation` payload diverges from pi's `TruncationResult` on three fields — **FILED AND PARTIALLY CLOSED 2026-08-14**

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed and partially closed** 2026-08-14 (sweep 9)

**upstream** — `packages/coding-agent/src/core/tools/truncate.ts:15-38` @`v0.83.0` — `TruncationResult` carries eleven fields (incl. `content` `:17` and `truncatedBy` `:21`) and is placed into `details.truncation` **whole**. Byte-only call sites: `grep.ts:335`, `find.ts:189`, `find.ts:324`, `ls.ts:182`, each passing `{ maxLines: Number.MAX_SAFE_INTEGER }`.

**cyrup** — `crates/cyrup-tools/src/truncate.rs:34-46` `Truncation`. Three divergences, on `read` / `bash` / `grep` / `find` / `ls`:

1. **`truncatedBy` was omitted when null.** `skip_serializing_if = "Option::is_none"` (`truncate.rs:36`) made the key VANISH on an untruncated result, where pi always emits `truncatedBy: null`.
2. **`maxLines` differed numerically for the three byte-only callers.** `TruncOpts::bytes_only` (`truncate.rs:61`) passed `usize::MAX`, so the record read `maxLines: 18446744073709551615` where pi's reads `9007199254740991`.
3. **`content` is ABSENT in cyrup.** pi's object carries the entire truncated output text a second time inside `details`.

**Impact** — `details` is persisted on `ToolResultMessage` (pi `packages/ai/src/types.ts:415-420`), so **all three reach the session file** — this is session-record interop, not a model-facing difference. Impact is genuinely bounded and the bound was measured, not assumed: `git grep -n 'truncation\.content' v0.83.0 -- packages/` shows every hit is a LOCAL variable inside an `execute()` body — **no pi consumer reads `details.truncation.content`** — and the renderers read only `truncated` / `truncatedBy` / `outputLines` / `totalLines` / `maxBytes`. What survives that bound is a reader of a cyrup session file who distinguishes an absent key from an explicit null (1), and any consumer that compares the two implementations' records byte-for-byte (1, 2, 3).

**Fix — LANDED for limbs 1 and 2.**
* `skip_serializing_if` removed from `truncated_by`; `None` now serializes as `null`.
* A `MAX_SAFE_INTEGER` constant (2⁵³ − 1) added beside the other truncation caps, and `TruncOpts::bytes_only` uses it. It is "effectively unbounded" to the truncation arithmetic exactly as `usize::MAX` was, so no behaviour outside the serialized record changes.
* Both are documented at their sites with pi's file:line and with the reasoning, so the values read as ported rather than arbitrary.

**RESIDUAL — limb 3, `content`, deliberately not ported, and this is the part that needs a decision rather than a patch.** Porting it duplicates the entire truncated output text into `details` on every truncated `read`/`bash`/`grep`/`find`/`ls` result, roughly doubling the session-file cost of the largest tool results cyrup writes — and no pi consumer reads it. Porting it blind would honour the letter of the port-the-literal-mechanism rule at a real cost with no reachable benefit; NOT porting it is an accepted divergence and needs to be recorded as one. The reasoning is written into `Truncation`'s doc comment so the next reader inherits the analysis instead of re-deriving it. **Take the decision explicitly**: either port `content` and accept the size, or tag it `[CYRUP-DELTA]` and close this row.

**Verify — DONE for limbs 1 and 2.** `crates/cyrup-tools/src/tests/pi_tool_semantics.rs` →
`truncation_details_serialize_in_pis_shape`. RED before on both: the untruncated case had no
`truncatedBy` member at all, and `maxLines` serialized as `18446744073709551615`. The test asserts
over the SERIALIZED value (`serde_json::to_value`), not the struct, because the serialized form is
what reaches the session file; and it re-asserts the truncated case still reports `"lines"`, so limb
1 cannot be satisfied by blanket-nulling the field. `cargo check -p cyrup-tools --all-targets` green.

## TOOL-045 — No built-in overrides `Tool::label`; all seven inherit the trait default `None`

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed · **Filed** 2026-08-14 (sweep 9)

**upstream** — pi sets an explicit `label` on every built-in `ToolDefinition`, always equal to `name`: `read.ts:210-211` @`v0.83.0`, and the parallel `name`/`label` pair at `bash.ts:325-326`, `edit.ts:293-294`, `write.ts:187-188`, `grep.ts:129-130`, `find.ts:115-116`, `ls.ts:101-102`.

**cyrup** — no `cyrup-tools` built-in overrides `Tool::label()`, so all seven return the trait default `None` (`crates/cyrup-core/src/tool.rs:102-104`), documented as "the runtime falls back to the tool name".

**Impact** — **Behaviourally equivalent for these seven**, because pi's seven values are identical to the names and cyrup's fallback produces the name. Filed for completeness and for one specific hazard: the field is **declared on the trait and left unimplemented across the entire built-in set**, so the fallback has never been exercised against a value that differs from the name. A future built-in — or a ported subagent/intercom tool — whose label legitimately differs from its name inherits the wrong fallback **silently**, and there is no test anywhere that would notice. That is the `TOOL-024` / `PROV-M01` dropped-delegation shape one step earlier: not a delegation that drops a value, but a value nothing ever sets, so nothing downstream is proven to read it.

**Relationship to `TOOL-022`/`TOOL-015`** — those are about `label` (with `renderShell` and `prepareArguments`) never reaching a **guest** tool's behaviour, i.e. a missing CONSUMER. This is the built-in PRODUCER half, and it is independent: closing this one does not close those, and closing those does not close this.

**Fix** — Override `label()` on all seven built-ins returning the tool name, matching pi's literal `ToolDefinition`. Trivial, and its value is that it makes the seven declarations *data* rather than an inference from a default.

**Verify** — Extend `crates/cyrup-tools/src/tests/pi_tool_semantics.rs`: assert `label() == Some(name())` for all seven (RED today — all seven are `None`). Separately, and more valuable, assert in `cyrup-ext`'s wrapper tests that a tool whose `label` differs from its `name` has that label survive to the consumer — the `Fixed` fixture's "every value DISTINCT and non-default" invariant `TOOL-024` established.


## Coverage

**Read first-hand at cyrup HEAD `04c1ba2`** (tree clean; `a9000b1` on top is docs-only). This file had been baselined at `1806375`/`9219dcd` and was ~40 commits stale, so every item was re-read at source rather than trusted. `crates/cyrup-tools` in full: all seven tools under `src/tools/` plus `globmatch.rs`, `edit_diff.rs`, `registry.rs`, `config.rs`, `lock.rs`, `error.rs`, `jsnum.rs`, `path.rs`, `truncate.rs` (`truncate_line`/`format_size` only), `ops/mod.rs` (the `FsOps`/`ProcOps` traits in full, `ImageMime`, `WalkOpts`, `ExecSpec`), `ops/local.rs` (`build_command`, `build_argv_command`, `LocalProc::exec`, `write_in_place`, and the inline `#[cfg(test)]` block at `:740-1173`), `ops/shell.rs` (**new to this area's coverage — it produced TOOL-038/039/040**), `isolation/mod.rs` and `isolation/protected.rs`. Tests: `tests/pi_schema.rs`, `tests/tools.rs`, `tests/isolation.rs`, and the ten test files that did not exist at the last pass — `builtin_tool_order.rs`, `bash_env_scrub.rs`, `bash_session_env.rs`, `cross_registry_mutation_lock.rs`, `edit_preview_diff.rs`, `grep_context_zero_line_text.rs`, `read_access_errno.rs`, `read_model_vision.rs`, `write_semantics.rs`. Off-crate tool surface: `crates/cyrup-core/src/tool.rs:60-160` in full, `crates/cyrup-ext/{wit/world.wit, src/registry.rs, src/wrapper.rs, src/host/live.rs::WasmTool}`, `crates/cyrup-ext-sdk/src/descriptor.rs`, `crates/cyrup-agent/src/agent.rs:700-720` and `:880-890`, `crates/cyrup-session-svc/src/builder.rs:630-690` and `:1600-1610`, `crates/cyrup-session-svc/src/bash.rs`, `crates/cyrup/src/output_guard.rs` in full, `crates/cyrup-modes/src/json.rs`, `crates/cyrup-tui/src/ansi.rs` and `transcript.rs:1700-1850`/`:2230-2320`.

**Upstream read at tags.** pi @v0.84.1: `core/tools/{read,write,edit,grep,find,ls,bash,index,truncate,output-accumulator,file-mutation-queue,path-utils,render-utils,tool-definition-wrapper,edit-diff}.ts`, `core/bash-executor.ts`, `core/output-guard.ts`, `core/extensions/{types,wrapper}.ts`, `core/agent-session.ts` (the tool-prompt assembly only), `utils/{mime,shell,paths,ansi}.ts`, `cli.ts`, `rpc-entry.ts`; and `cli.ts` + `read.ts` @v0.83.0 for the PB-5 and `COMPACT_RESOURCE_FILE_NAMES` baseline checks.

**Version-lag sweep, v0.83.0..v0.84.1, scoped to this area.** `git diff --stat` is 7 files / +68 −35 and the whole diff was read. Four changes, **three already ported, one a no-op**, so no `upstream-drift` item is filed. (1) Every tool hoists its snippet/guidelines into an exported `*SystemPromptContribution` const — a pure TS refactor; the one wording change (bash's guideline gaining a leading "You can ") is already carried at `bash.rs:122` and pinned at `tests/pi_schema.rs:122-123`, with the version provenance documented at `bash.rs:104-110`. (2) `read.ts:42` adds `AGENTS.override.md` to `COMPACT_RESOURCE_FILE_NAMES` — already first in `transcript.rs:2235`. (3) `find.ts` extracts `relativizeFindResultPath` (`:16-26`), fixing a prefix-slice bug where a sibling directory sharing the search-path prefix relativized wrongly — cyrup was never exposed to it, since `find.rs:136` uses component-wise `Path::strip_prefix` and already appends the trailing `/` for directories at `:143`. (4) `find.ts:263-265` rewrites `/` to `[/\\]` in the pattern on win32 — a no-op for cyrup, whose candidates are posix-normalized by `to_posix` (`globmatch.rs:155-162`) before matching. `bash-executor.ts` and `output-guard.ts` are byte-identical across the two tags.

**Repair pass 2026-08-12 — what it covered, what it rejected, what is still blind.**

*Covered — TOOL-039's severity (critique finding 5), verified rather than accepted.* All five limbs
of the claim were re-read at source before the raise, and each is recorded inside the item so the
rating can be audited without re-deriving it: the override is first in `ShellConfig::detect()`
(`ops/shell.rs:101-105`, ahead of the `/bin/bash` probe at `:108-110`); `detect()` is the default
path (`registry.rs:54`, `ops/mod.rs:359-361`); `session_env_scrub_keys()` (`config.rs:41-48`) is
built from `SESSION_ENV_SUFFIXES` (`config.rs:31-32`) crossed with `CYRUP_`/`PI_` and therefore
cannot contain `CYRUP_SHELL`; nothing reports the resolved program; and `getShellConfig`
(`utils/shell.ts:67-120`, **byte-identical at both tags**) reads no environment variable as a shell
selector — its only `process.env` reads are the `ProgramFiles` installation-location lookups at
`:79`/`:83` and `getShellEnv`'s `PATH` handling at `:124-131`. Raised **low → high** and cross-linked
to **TOOL-007** as one shell-surface decision; the coupling is stated in both items and in the open
table.

*Covered — the cross-tag citation sweep (critique finding 9), and it found a real defect.* This file
declares pi **v0.84.1** as its oracle (line 3) while `README.md:224-225` requires classification
against the ported tag v0.83.0. The prior pass justified that with a version-lag sweep — but that
sweep was scoped to `packages/coding-agent/src/core/tools/`, and **four of this area's upstream files
live outside it**. Every file cited by an open item was re-diffed across the tags:

| upstream file | v0.83.0 → v0.84.1 | items citing it | effect |
|---|---|---|---|
| `utils/shell.ts` | **identical** | TOOL-038, TOOL-039, TOOL-040 | offsets valid at either tag |
| `utils/mime.ts` | **identical** | TOOL-035 | valid at either tag |
| `core/tools/file-mutation-queue.ts` | **identical** | TOOL-019, TOOL-025, TOOL-032 | valid at either tag |
| `core/tools/index.ts`, `edit-diff.ts`, `tool-definition-wrapper.ts`, `extensions/wrapper.ts` | **identical** | TOOL-012 (closed), TOOL-018, TOOL-021, TOOL-024 | valid at either tag |
| `core/extensions/types.ts` | +29/−3, but `:453`/`:463`/`:465`/`:468` **unmoved** | TOOL-015, TOOL-016, TOOL-022 | cited offsets valid at either tag |
| `core/bash-executor.ts`, `core/output-guard.ts` | **identical** | TOOL-037 | valid at either tag |
| `core/tools/edit.ts` | +10 from `:86` on (`:86`→`:96`, `:306`→`:310`, `:312`→`:316`, `:329`→`:333`) | TOOL-006, TOOL-014, TOOL-015 | cited v0.84.1 offsets; v0.83.0 equivalents given here |
| `core/tools/write.ts` | +5 from `:203` on (`:203`→`:208`, `:208`→`:213`, `:212`→`:217`, `:218`→`:223`, `:219`→`:224`) | TOOL-006, TOOL-041 | ditto — and the post-write `throwIfAborted` **does** exist at v0.83.0 (`:219`), so TOOL-041 is confirmed baseline debt, not drift |
| `core/tools/ls.ts` | +5 (`:145`→`:150`) | TOOL-029 | ditto |
| `core/tools/grep.ts` | +5 (`:172`→`:177`, `:221`→`:226`, `:253`→`:258`, `:288`→`:293`) | TOOL-033, TOOL-034, TOOL-S06 | ditto |
| `core/tools/find.ts` | +11 to +14 (`:241`→`:252`, `:243`→`:254`, `:248`→`:259`, `:253`→`:267`) | TOOL-011, TOOL-023 | ditto |
| `utils/paths.ts` | +22/−1, and **`normalizeWindowsShellPath` is absent at v0.83.0** | TOOL-036 | **classification corrected** — see the item |

The one substantive result is **TOOL-036**: its Windows shell-path half is `upstream-drift`, not a
`parity-bug`, because the upstream function it names did not exist at the ported tag. Its `~` /
`os.homedir()` half remains a genuine baseline parity bug. Kind, severity and effort are unchanged
(the item keeps `parity-bug` for the half that carries it); what changed is that the drift half is
now labelled and will not be scheduled as a port regression. The seven purely-numeric shifts changed
no finding — in each case the code was byte-identical and only the addressing moved — and the
v0.83.0 equivalents are recorded above rather than rewritten into every item, because this file's
declared oracle is v0.84.1 and re-anchoring it wholesale would introduce more error than it removes
(`README.md:224-225`: do not fix a citation by shifting it).

*Covered — the tracker test (critique finding 14).* Every open item was re-read against "does this
propose a change with a named fix site, or only a decision?". **None in this area is a tracker**, so
nothing was reclassified. The three nearest calls, all kept as counted work: **TOOL-017** ("blocked
on a decision, not on code") — but it names the ~20-line arm and the file it goes in, so it is
blocked work, not bookkeeping; **TOOL-036** — carries a "Prerequisite decision" on Windows scope, but
proposes the concrete resolver and the concrete port; **TOOL-007** and **TOOL-039** — both say
"decide deliberately", but each enumerates the code changes each branch of the decision requires,
which is a design choice inside a work item, not a deferral.

*Rejected in this repair pass.* (a) **Raising TOOL-007 alongside TOOL-039** — rejected: pi has no
protected-path concept at all, so TOOL-007 is an over-restriction plus an ineffective guard, not a
configured deny being defeated; it stays `medium` and ships with TOOL-039. (b) **Raising TOOL-035 to
critical** under finding 3's "silent wrong output" clause — rejected: the clause is real (the model
gets `from_utf8_lossy` garbage where pi shows an image) but it needs a PNG with >4100 bytes of
pre-`acTL` chunks, which is not an ordinary path; the rule applied across all three of this repair
agent's files is "meets a README:106-107 condition **and** the triggering path is one a user takes in
ordinary use". (c) **Re-anchoring every v0.84.1 citation in this file to v0.83.0** — rejected in
favour of the offset table above, for the shifting hazard `README.md:224-225` names.

*Still blind after this pass.* The sweep diffed the upstream files **cited by open items**; it did
not enumerate `packages/coding-agent/src/utils/` as a surface, which is how `paths.ts` was missed in
the first place and is a live instance of blind spot 3 below. `utils/ansi.ts` and `utils/mime.ts`
were read but `utils/` was never swept as a directory. The eight pre-existing blind spots below are
otherwise unchanged.

**Rejected this pass, with reasons — do not re-derive these.**
1. *"A realpath failure gives two aliases different lock keys and interleaves their writes into a corrupt file"* (TOOL-032's second impact, filed at medium) — **rejected**: every non-ENOENT/ENOTDIR realpath failure also fails the subsequent `open(2)` in `write_in_place` (`ops/local.rs:81-87`), so the call errors rather than corrupting. TOOL-032 kept at low for the blocking call alone.
2. *"cyrup emits truncated JSON lines on macOS pipes because it lacks pi's ENOBUFS retry"* (TOOL-037 at medium) — **rejected**: pi needs the retry because libuv sets Node's stdout non-blocking; Rust's `Stdout` is blocking, `write_all` loops partial writes, and a blocking write is itself the backpressure gate. Kept at low, rescoped to an inherited `O_NONBLOCK` fd.
3. *"`tool-definition-wrapper.ts` copies `promptGuidelines` onto every AgentTool"* — **rejected as a factual claim**; `wrapToolDefinition` (`:9-19`) copies eight fields and neither `promptSnippet` nor `promptGuidelines`. TOOL-021 rewritten on the real path (`agent-session.ts:2490-2512`, `:1026-1038`); the gap itself is unaffected.
4. *"`wrapper.rs:107-111` is the only forwarding path by which built-in metadata reaches the prompt builder"* (TOOL-024's severity amplifier) — **not confirmed**: the prompt builder reads the vtable directly at `builder.rs:1606`, and it was not established that built-ins pass through `wrap_registered_tool` at all. TOOL-024 downgraded to low; establishing this is the prerequisite for re-raising it.
5. *`AI_AGENT`/`PI_CODING_AGENT` as a medium* — **rejected**: pi consumes neither variable itself, so it is a third-party marker, not agent behaviour. TOOL-031 kept at low.
6. *The `CYRUP_*`-for-`PI_*` session-variable rename* — **not filed**: deliberate and self-consistent (`bash.rs:112-119` explains it, `config.rs:41-48` scrubs BOTH families).
7. *`isolation/policy.rs`'s `dangerous_bash_rule` / `is_dangerous_command` / `protected_path_rule`* — **not filed here**: zero production consumers (only `lib.rs:40`'s re-export and tests), cyrup-original dead code with no behavioural difference. Belongs in PARITY-GAPS §5 deletion candidates. Note the contrast with TOOL-039, which is live.
8. *The in-process `ignore`/`grep-searcher`/`globset` substitution for the rg and fd binaries* — a declared mechanism delta (`grep.rs:1-3`, `find.rs:1-2`); only its behavioural costs are filed (TOOL-033, TOOL-034). `ensureTool()`'s binary download has no cyrup analog by construction.

**Handoffs.** (a) PARITY-GAPS **PB-4** is TOOL-017's residual and is owned by cyrup-tui; it needs a packaged-docs locator to exist before the `docs` arm can be written. (b) TOOL-031 fixes only the `bash` half of **PB-5**; the process-global half (subagent re-exec, MCP server spawn, extension processes) belongs to whoever owns `crates/cyrup/src/main.rs` and the subagent runner. (c) TOOL-015's residual, TOOL-016, TOOL-021 and TOOL-022 are all blocked on the same WIT bump — residual-ledger cluster **C5**; schedule them as one change. (d) TOOL-019 and TOOL-032 are the same fix. (e) TOOL-023 and TOOL-033 are the same restructure and must pick one limit strategy between them. (g) **TOOL-039 and TOOL-007 are one shell-surface decision** and must land together — "cyrup constrains what the model can reach through `bash`" (TOOL-007's premise) and "cyrup does not control which interpreter `bash` is" (TOOL-039's finding) cannot both be true, and TOOL-038's `cmd.exe` fallback is the Windows face of the same question, so read all three before answering any of them. (f) v0.84.1's `*SystemPromptContribution` consts exist so `pi/packages/coding-agent/src/server/create-harness.ts` can reuse them; whether cyrup has a counterpart to that server harness is outside this area.

**Blind spots — read this before the next pass.**
1. **No tests were run** (cargo forbidden). TOOL-020, TOOL-024, TOOL-025, TOOL-026 and TOOL-030 remain ANALYTIC judgements about defective assertion design, not observed flakes; every "red today" claim about a proposed test is likewise analytic. TOOL-025's downgrade rests on reading the payload lengths, not on watching the test fail with the lock removed. It was not verified that the ten new test files pass — only that they exist and assert the properties claimed.
2. **Neither fd nor ripgrep is vendored as a binary.** TOOL-011 and TOOL-023 still rest on pi's in-source comment (`find.ts:254-256`) plus its argv construction rather than on fd's matching code. The ripgrep side is better off — the `ignore` crate IS in the workspace and TOOL-027's closure was built against `ignore-0.4.33`'s `gitignore.rs:513-522` directly. TOOL-033's claim that rg stops traversing at the match limit is inferred from pi killing the child (`grep.ts:288-295`), not from rg's own source.
3. **Windows is unexercised.** TOOL-036, TOOL-038 and TOOL-039's propagation half are derived entirely from reading `paths.ts`/`shell.ts` against `path.rs`/`shell.rs`; nothing here can execute the win32 branch, and **it was not established whether cyrup ships Windows CI or claims the target formally**. Settle that question first — if Windows is out of scope by policy, strike those items rather than downgrading them.
4. **TOOL-035's reachability was reasoned, not demonstrated.** A >4100-byte pre-`acTL` chunk follows from `mime.ts`'s loop bound; no such PNG was constructed.
5. **Not re-audited, no evidence gathered this pass.** `crates/cyrup-tools/src/{output.rs, details.rs}` were not compared line-by-line against `output-accumulator.ts` — that file was read in full upstream but cyrup's accumulator was only spot-checked through `bash.rs`'s call sites, so the rolling-tail trim, the `tailStartsAtLineBoundary` partial-first-line rule and the temp-file threshold are UNVERIFIED. Same for `src/truncate.rs` beyond `truncate_line`/`format_size` (`truncateHead`/`truncateTail`'s `+1 for newline` accounting and the re-classification at `truncate.ts:140`/`:221` were not diffed), for `isolation/{policy,sandbox,traversal}.rs`, and for the image resize/encode ladder at `read.rs:332-561`.
6. **`renderCall`/`renderResult` parity is sampled, not systematic.** All seven pi bodies were read, but their cyrup counterparts live in `cyrup-tui` and only `render_read`/`render_write`/`render_edit`/the bash footer were checked. TOOL-S02/S03/S05 were closed on targeted evidence, not a full read of `transcript.rs`. A systematic transcript-vs-pi comparison is area 07's.
7. **`LocalProc::exec_argv` and the WASM `exec` grant** were read only far enough to confirm the TOOL-020/TOOL-030 test sites and the deliberate single-pid-vs-process-group split; their parity against pi's `exec.ts` was not re-derived.
8. **`ops/shell.rs` was audited for the first time this pass and immediately yielded three items.** Treat the rest of the `ops/` seam — `local.rs`'s walk implementation, `mod.rs`'s `WalkOpts` defaults against `.gitignore`/hidden-file handling in rg and fd — as the most likely place the next sweep finds more.

---

## Surface-sweep findings (2026-08-03, HEAD `9219dcd`) — ALL CLOSED as of 2026-08-12

Found by a **surface-driven** sweep that walked pi asking what has NO cyrup counterpart at all,
rather than checking a list of known items. That inversion exists because the item-driven method
missed pi's stray-OSC-reply swallow (`pi/packages/tui/src/tui.ts:788-794`) — a real, user-reported
bug — and by construction cannot see behaviour nobody wrote an item for. IDs keep their `-SNN`
suffix to mark their provenance. **All six are closed at HEAD `04c1ba2`**; the evidence is in the
status table above. The section is retained so each closure remains re-auditable, and as a record
that the sweep paid for itself — six items, all real, all now shipped.

| ID | Severity | Kind | Effort | Title | Status |
|---|---|---|---|---|---|
| TOOL-S01 | medium | not-ported | S | Six of seven built-ins model numeric parameters as `usize`, so pi-legal JSON floats/negatives hard-error at deserialization instead of being coerced | **closed** — `jsnum.rs`, all five param sites on `f64` |
| TOOL-S02 | low | not-ported | S | Tool-result text is rendered with no `stripAnsi` and no `sanitizeBinaryOutput` | **closed** — `cyrup-tui/src/ansi.rs:20-27` |
| TOOL-S03 | low | not-ported | M | `computeEditsDiff` — pi's pre-execution edit preview — has no counterpart | **closed** — `edit_diff.rs:545-576` + `transcript.rs:1802-1830` |
| TOOL-S04 | low | not-ported | S | `images.autoResize` is a live toggle with no consumer — `read` always downsizes | **closed** — `config.rs:154-163`, wired at `builder.rs:681` |
| TOOL-S05 | low | not-ported | S | `bash` shows no live elapsed timer | **closed** — `transcript.rs:1963-1979` |
| TOOL-S06 | low | not-ported | S | `grep` with context silently drops a file it cannot read | **closed** — `grep.rs:297-303` |

A note for the next sweep: the six `-S` items were written with empty `**Impact**` / `**Fix**` /
`**Verify**` bodies ("port the upstream behaviour named above"), which is why they took a full extra
pass to action. A surface-sweep finding is not finished until it carries the same six fields as
every other item in this file.
