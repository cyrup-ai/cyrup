# 00 — Residual ledger

Ranked, cross-cutting view. The per-area files hold the evidence; this file is for **picking the
next work item**.

---

# RECONCILED 2026-09-04 → 2026-09-05 (ninth edition) — batch 3: nineteen rows closed, two partially, one refuted; **the set above `medium` is EMPTY for the first time**, and this pass filed seven new rows against the code the batch itself landed

> **Read this block before planning.** cyrup **code** HEAD **`824a539e`**, branch
> `claude/parity-batch3`, cut from `main` = `3e9633c4`. That is the last CODE commit of the batch —
> `fix(tui,session-svc,ext,intercom): TUI-046 DRIFT-041 EXT-006 ICOM-054 review fixes`. The docs
> commit carrying this block cannot cite its own sha, and a status is measured against code, not
> against the doc commit that describes it. 51 commits off the base, 23 of them touching
> `crates/`/`xtask`.
>
> **Dating.** The batch's brief was written 2026-09-04 and the first half of the work carries that
> date; a container restart pushed the second half into 2026-09-05, and every closure mark in the
> area files matches its own commit's date. This edition is therefore dated across both days rather
> than to the one the brief named.
>
> **Counts are `scripts/count_open_items.py`'s, re-run after every correction below was written:**
> **91 open = 0 critical, 0 high, 15 medium, 76 low; 583 closed; 6 trackers** (eighth edition:
> 103 = 0/1/28/74, 564 closed). §0 of `PARITY-GAPS.md` carries the per-area table and §0a the
> above-medium set; neither is duplicated here.
>
> **The arithmetic, stated so the headline cannot flatter itself.** 583 − 564 = **19 rows closed**.
> Open went 103 → 91, which is −12 and not −19, because **this ledger pass filed seven NEW rows** —
> six of them against defects in the code this very batch landed, found by reviews that blocked and
> were then not answered before the batch ended. A batch that closes nineteen rows and opens seven
> has closed twelve, and that is the number to plan against.

## The above-medium set is EMPTY — what that does and does not mean

`SUBA-074` was the last row above `medium` in the ledger, and it is genuinely closed: stage 1 (the
refusal) at `bf8b0f9`, stage 2 (the capability/status contract, the hardened external-CLI runner, the
generic no-adapter path and the `claude-code` adapter) at `af1a8a76`, review-fixed at `95a55ea1`.
`codex-exec`, `cursor-agent` and the whole `external-job` protocol stay deferred and are refused BY
NAME through an exhaustive `RunnerDispatch`, never silently widened. The row now writes out what
landed, what is deferred and why, and the design decision with its rejected alternatives.

`scripts/count_open_items.py` prints `Above-medium open rows (0)` across all fourteen tables. **No
prior edition has been able to print that line.** What it means: no row currently carries a
`critical` or `high` severity cell. What it does NOT mean:

* **It is not "the port is finished."** 91 rows are open, 15 of them `medium`, and `README.md`'s
  *Where this analysis is blind* is unchanged — an item-driven analysis cannot see behaviour nobody
  filed an item for, and **whatever is open is a floor, never a total**.
* **It is not "nothing serious is left."** Severity is a per-row judgement made when the row was
  written; six of this edition's fifteen mediums were filed in the last two days, which is a
  statement about how recently anyone looked, not about how bad the remainder is.
* **It is not durable.** The fifth edition recorded the above-medium set turning over *completely* in
  one pass, twice acquiring criticals that had not existed a week earlier. Expect this line to be
  false again the next time a surface is walked.

## What closed, and on what evidence — every row re-opened in its area file, not taken from a commit subject

Nineteen rows, each with a landing sha that exists and whose subject matches the claim (all 27 shas
cited across the batch were resolved with `git rev-parse` this pass):

* **`SUBA-074`** (was the last `high`) — external-CLI runner + capability contract + `claude-code`
  adapter, `af1a8a76` / `95a55ea1`. See above.
* **`SUBA-093`** `07f2df0d` — parallel-group members flattened into `RunStatus::steps` so a
  child-scoped stop can address one. **`SUBA-094`** `f9de9fe7` — `display` carried on the custom
  message, so a hidden completion notice is no longer drawn.
* **`EXT-003`** `605f483c` — the pre-trust `with_wasm` fallback. **Landed under `TUI-N02`'s subject
  and body**, which is recorded as a scope defect on both rows rather than tidied away.
  **`EXT-006`** `1739fcc4` — renderers get display options + theme and are re-invoked when either
  moves. **`EXT-039`** `9c25f603` — the reserved-keybinding gate wired into the TUI, precedence
  inverted to match `custom-editor.ts`.
* **`TUI-N02`** `605f483c` · **`TUI-N11`** `f061bf35` · **`TUI-004`** `1f53b8bd` (the `/reload`
  half; the mode-2031 half stays the reasoned divergence the item's own Fix asks for).
* **`SEAM-015`** `abb8b5e3` / `8f1b5e76` · **`ICOM-054`** `87421cd7` · **`ICOM-055`** `c440c038`.
* **`DRIFT-004`** `8b688401` (the WASM guest bash-operations tier) · **`DRIFT-041`** `ce81dba9` /
  `824a539e` (pi's templated HTML export, five byte-identical assets) · **`DRIFT-053`** `9de4254b`
  (`/share`'s Radius-first path).
* **`CFG-078`** `69f6c4e1` · **`CFG-079`** `0a697d62` · **`CFG-080`** `9a7c0fdb`.

**Partial, and correctly left OPEN with severity intact so the script still counts them:**
`DRIFT-009` (`9e2bfda5` — the four unembeddable catalogs accounted for and pi's own differ ported;
the catalog floor itself stands) and `TUI-046` (`8bb0d22f`, then **bit 4 REVERTED** at `824a539e`
because it broke every shift chord — the row is now `low` and still open).

**Refuted, not closed by work:** `TOOL-022`. All three limbs (`renderShell`, `prepareArguments`,
`label`) already reached a guest tool at HEAD; the missing consumer had landed in `75532cee`
(`EXT-024`), which `git merge-base --is-ancestor` confirms reached `main` before this branch was cut.
Docs-only closure at `5ad1a419`. Its `label` half is **parity, not a gap** — pi copies `label` at
`core/tools/tool-definition-wrapper.ts:11`/`:39` and reads it nowhere.

## Seven rows filed BY this pass, all against code this batch landed

Six came out of batch-3 reviews that blocked and were not answered before the batch ended; one is a
residual whose own closure asked for an id. Every one was re-derived on both sides by this pass
before it was written — the Rust at HEAD `ba380e58`, the TypeScript with `git -C tmp/<repo> show`.

| id | sev | what | why it is a new id rather than a re-opening |
|---|---|---|---|
| `SUBA-095` | medium | `exec/external_cli/run.rs`'s select loop still has two escapes from its own deadline and stop arms: a post-loop unguarded `child.wait()` reached when both pipes EOF before the child exits, and `biased` starvation of the deadline/stop arms by an always-ready `rx.recv()`. Upstream's `registerTimeout`/`registerStop` are event-loop callbacks independent of stream state (`external-cli-runner.ts:361-362` @v0.64.0) | `SUBA-074`'s own gap — external runners unported — is genuinely closed. This is a defect in the code that closed it |
| `DRIFT-054` | medium | The templated HTML export drops `systemPrompt` and `tools`, so **every** exported document silently loses its System Prompt and Available Tools sections, on `/export`, `/share` and RPC `export_html` alike. pi always passes `this.state` (`agent-session.ts:3438` @v0.84.4) into `exportSessionToHtml`, which sets both keys (`export-html/index.ts:263-269`), and the byte-identical `template.js` this batch shipped renders both blocks from them (`:1405-1435`) | `DRIFT-041`'s subject — a 131-line text dump vs a templated document — is closed. The payload being two keys short is a new defect, and the row asserted the opposite |
| `EXT-076` | medium | A non-lowercase extension shortcut key is normalized on install but dispatched by exact, case-sensitive match, so its handler is never found. A **regression**: before `9c25f603` the production line installed the raw keys from `shortcut_specs()` | `EXT-039`'s subject — the reserved-key gate and the precedence inversion — is closed |
| `TUI-096` | low | A theme that fails to load repaints dark in silence and `active_name()` keeps the broken name. pi's `applyThemeName` also seats `"dark"` AND surfaces ``Failed to load theme "…": …\nFell back to dark theme.`` under `showError`, which is `true` on both naming branches (`theme-controller.ts:126-135`, `:64`, `:70`) | `TUI-004`'s `/reload` re-apply is ported; its row claimed the whole failure path and had ported half |
| `TUI-097` | low | Ctrl+X and `/copy` are one `AppCommand::Copy`, so `/copy` prefers a live selection where pi always copies the last assistant message (`interactive-mode.ts:2910` vs `:3022`, the selection leg gated at `:6117-6127`) | `CFG-078`'s residual 2, which asked for its own row and its own red-before test |
| `SEAM-118` | low | `--no-builtin-tools` leaves `grep`/`find`/`ls`/`powershell` active where pi's `options.noTools ? []` starts a session with **no** built-in active (`sdk.ts:261-262`) | `CFG-079`'s residual 2, filed in area 08 because that area owns `cyrup-session-svc/src/builder.rs` |
| `DRIFT-055` | low | `/share` uploads the whole session tree with original `parentId`s and the stored header timestamp, where pi uploads `getBranch()` linearised with a fresh one (`core/session-export.ts:21-38`) | `DRIFT-053`'s residual (d), which asked for its own id |

## Corrections this audit made in place — two of them were FALSE claims in CLOSED rows

The batch's own reviews found these and the batch ended without answering them; the reviews'
findings were re-derived here rather than trusted, and **one review claim was itself refuted**.

1. **`DRIFT-041`'s residual list was false.** It said "one residual, low: `renderedTools`" in three
   places. Corrected, and the real second residual filed as `DRIFT-054`. Two in-source claims carry
   the same false statement and need a CODE pass:
   `crates/cyrup-session-svc/src/session/transcript.rs`'s *"Its one residual is `renderedTools`"* and
   `src/export/mod.rs`'s `SessionData` doc, which excuses the absence as *"what `JSON.stringify`
   produces for pi's own `exportFromFile`"* — but `exportFromFile` is `cyrup --export`'s path, not
   the path any front-end takes.
2. **`TUI-004`'s row equated half of `applyThemeName` with all of it.** Corrected; filed as
   `TUI-096`.
3. **`EXT-039`'s `ExtensionShortcut` citation did not resolve.** `types.ts:1547-1552` @v0.84.4 is the
   OAuth provider block; the interface is at **`:1611-1616`**. Fixed in both area-06 sites. Four
   in-source copies remain (`crates/cyrup-ext/src/registry.rs`,
   `crates/cyrup-ext/src/tests/payload_and_seam_parity.rs`, `crates/cyrup/src/interactive.rs`,
   `crates/cyrup-tui/src/app/state.rs`) and need a code pass.
4. **Two area-13 cyrup citations did not resolve, one of them on the re-audit's only new `critical`.**
   `URL_BOUND_AUTH_FIELDS` is at `config.rs:2274`, not `:1702` (`:1702` is the `AuthMode` derive);
   `ConfigContext::sources()` is at `:3231`, not `:2578` (`:2578` is `read_imported_config`). The
   substance of both rows re-derives exactly — upstream really is five fields at
   `v2.32.1:config.ts:525` with the delete loop at `:572`, and the ladder really has no manifest
   rung.
5. **`05-cyrup-config-and-resources.md` carried TWO `## CFG-079` sections**, the second unstruck and
   reading as open work: the closure block had been inserted as a new section rather than in front of
   the original. Demoted to a "superseded text follows" marker, so the file carries one section per
   id again.
6. **Six closed detail sections had headings that did not carry a closure mark**, so a reader
   scanning `## ` headings saw open items — `SUBA-074`, `EXT-006`, `EXT-039`, `TUI-004`, `TUI-N11`,
   `DRIFT-004` (plus `DRIFT-041`/`DRIFT-053`/`CFG-078`/`CFG-079`/`CFG-080` normalised to the
   `~~sev~~ **CLOSED date**` form their siblings use).
7. **Stale-by-a-day cyrup line citations repaired by re-finding each SYMBOL, never by shifting an
   offset** — `TOOL-022`'s (rotted by `EXT-006`, `DRIFT-004` and `CFG-078` rewriting `world.wit`,
   `host/live.rs` and `app/state.rs` after it was written), `TUI-N02`'s (rotted by `TUI-004`
   inserting 27 lines into `run_arms.rs`), `SEAM-015`'s and `EXT-003`'s. Each carries a dated note
   saying so. `SEAM-015`'s `agent-session.ts:2782` is corrected to `:2993` — the expression moved
   between v0.83.0 and the ADR-0006 target v0.84.4, and **17 in-tree copies of the stale number
   remain** across six crates.
8. **`SEAM-015`'s guest-tier residual was discharged and read as open** — `DRIFT-004` landed
   `GuestBashOperations` later in the same batch. Struck.
9. **`ICOM-055`'s "`E_SENDER_NOT_FOUND` belongs to a different gap"** was true when written and stale
   by the end of the batch: `ICOM-054` *was* the different gap and every refusal now carries its
   code. Struck.
10. **One review claim REFUTED rather than applied.** The batch-3 second review reported `DRIFT-009`'s
    three `scripts/diff-model-catalog.mjs` citations as each off by one. All three were re-derived at
    `v0.84.4` and are **exact**: `sortJsonKeys` opens at `:96` and `canonicalizeJson` closes at
    `:114`, `THINKING_LEVEL_ORDER` is `:93`, the no-parent-key array arm is `:106`. The row was right
    and is left alone; the refutation is recorded in it.

## The container restart, and what it cost

The batch was interrupted by a container restart with **nine code commits already landed and
unreviewed**, and one agent was killed mid-item. Three consequences are on the record because they
are the shape a restart leaves behind, not because they were unusual:

* **`TUI-N02` was recovered from a killed agent's uncommitted work** and landed at `605f483c` with
  its row already closed. That recovery is also how `EXT-003`'s whole fix came to land under
  `TUI-N02`'s subject and body with `EXT-003`'s row still asserting the opposite — a scope defect
  caught by review and now recorded on both rows.
* **`1739fcc4` (`EXT-006`) left the tree unbuildable for `cargo nextest run -p cyrup-session-svc`**
  for two commits: it re-signed `render_tool_call_outcome`/`render_tool_result_outcome` with a third
  parameter and missed five call sites in `crates/cyrup-session-svc/src/tests/custom_tool_render.rs`.
  Repaired at `b0f9fbe5`. Bisect-hostile, and the commit's own check list named four crates and not
  the one it broke.
* **Reviews that blocked were not all answered before the batch ended.** Six of the seven rows this
  pass filed are review findings that survived to the end of the batch unfixed. The restart did not
  create them; it removed the slack that would otherwise have closed them.

The **disk** is the other standing cost: it hit 100% three times across the batch. Agents recovered
by deleting `target/debug/incremental` and `~/.cargo/registry/cache` — both regenerable caches,
never `target/` itself — and several checks were skipped for it, which is recorded honestly per row.

## Residual leads this batch produced — recorded, NOT counted

Ownerless unless an id is named. These are leads, not items: neither side has been re-read to the
standard *Working an item* sets.

* **`cargo nextest run -p cyrup-it --features it` was not run for most of the batch**, and the
  `492/493/494` figures several rows quote are taken from commit bodies rather than measured by any
  reviewer. `EXT-006`'s agent did run the `ext` target (46/46) and `ICOM-054`'s ran
  `-E 'binary(intercom)'` (80/80); everything else is unverified. **This is the single largest
  unmeasured claim in the batch.**
* **17 in-tree `agent-session.ts:2782` citations** across `cyrup-ext`, `cyrup-ext-sdk`, `cyrup-tools`,
  `cyrup-session-svc`, `cyrup-modes` and `cyrup-it` should be `:2993` at the ADR-0006 target. One
  sweep, no behaviour.
* **`crates/cyrup-tui/src/tests/startup_resources_panel.rs`** cites `(:1986, :5993)` for
  `showDiagnosticsWhenQuiet: true`; `:5993` is right, `:1986` should be `:1982`.
* **The `[Extension issues]` panel omits pi's MIDDLE tier.** Upstream assembles load errors →
  `getCommandDiagnostics()` → `getBuiltInCommandConflictDiagnostics()` → `getShortcutDiagnostics()`
  (`interactive-mode.ts:1872-1886` @v0.84.4); `StartupReport::from_session` folds the first and last
  only. Area 07 already owns a row for the command tier, whose body still names the now-deleted
  `build_startup_report` in the present tense.
* **Dead-but-shipped surface in `cyrup-ext-subagents`**: `ClaudeCodeParser::AFTER_TERMINAL` and the
  `AfterTerminal` enum have no production reader, and their one test asserts a constant against its
  own definition. `af1a8a76`'s body claims the divergence is "a named constant rather than an `if`";
  at HEAD the `if` is what is there.
* **`runner_to_json_string`'s `capabilities` comment states the opposite of what the code does** —
  it claims upstream's capability ORDER survives, but the values are collected into a
  `serde_json::Map` and this workspace deliberately does not enable `preserve_order`, so the emitted
  object is alphabetical.
* **The shortcut key is three stringly-typed copies with three normalization policies**
  (registry-as-registered, `ShortcutSpec.id`-as-normalized, guest `Shortcut.key`-as-registered).
  `EXT-076` fixes the symptom; a `KeyId` newtype whose only constructor normalizes would make the
  third copy unrepresentable.
* **`ExtensionRegistry::resolve_shortcuts`** (the owner-only projection) has no production caller
  left after `EXT-039` — it survives only as one test's drift-check counterpart.
* **A two-lock TOCTOU on the export leaf**: `AgentSession::export_to_html` takes the manager lock
  once for `export_jsonl` and again for `export_leaf_id`, where pi reads entries and leaf in one
  synchronous object literal. An append between them yields a leaf absent from the entries, and
  `template.js` walking an unknown id produces an EMPTY document — the exact failure the leaf fix
  existed to prevent.
* **29 markdown table rows across the area files have the wrong cell count**, because a `|` inside a
  cell (usually a JS `||` or a Rust closure `|s|` in an inline code span) was never escaped, so the
  row renders short and its trailing columns vanish. Measured this pass with a five-line script
  over every `|---|` separator and the rows beneath it; **none was introduced by batch 3** (the
  same script found zero new ones in this pass's own diff). It is mechanical and belongs with the
  citation-resolver CI check `README.md`'s *Work this directory owns* already asks for.
* **pi's two `exportSessionToHtml` guards are unported** — `"Cannot export in-memory session to
  HTML"` and `"Nothing to export yet - start a conversation first"` (`export-html/index.ts:245-250`).
  cyrup writes a valid-but-empty document and reports success.

## Area 13 — the MCP port, re-audited at `pi-mcp-adapter` v2.32.1

Folded in here from `11b9994a` because no cross-cutting file had recorded it, and because the eighth
edition's *Recommended next batch* put area 13 first. **Area 13 remains counted separately from the
fourteen tables above** and nothing in this file speaks for its unit inventory — the structural
reason is in `README.md`'s *Area 13* section: the fourteen measure drift in code that exists, area 13
specifies code that does not exist yet, and the two cannot be added.

* **The clone was re-pulled to `v2.32.1` and the area files were opened against it** — the prior
  record said "clone re-pulled, area 13 NOT re-audited". That is no longer true, and `README.md`'s
  Baselines row is updated to say so.
* **The upstream delta**: `git diff --stat v2.26.1..v2.32.1` = **147 files, +16,014 / −1,001**;
  `git rev-list --count` = **72 commits** (68 `--no-merges`). *The re-audit first said 79; corrected
  at `b18fe2ff`, and the file/line figures on either side of it re-derive exactly.* 66 of the 147
  are tests; production TypeScript changed in 38 files, six of them new.
* **The census, re-derived by re-parsing the unit table** (not carried from prose): the pre-re-audit
  baseline is **214 implemented / 98 partial / 98 missing / 27 n-a of 437**, and the counted figure
  after this pass is **244 / 82 / 84 / 27 of 437** — 166 open. Thirty rows moved to `implemented`,
  each carrying a citation.
* **That counted column is a FLOOR, not the answer**: 159 open rows were not opened this pass and are
  counted exactly as they stood. The extrapolation — a 1-in-6 systematic sample closing 26 of 33,
  p̂ = 0.788, Wilson 95% CI [0.622, 0.893] — puts roughly **76 open of 477 units, 95% interval ~59 to
  102**, once the 40 new units this pass filed (35 of them open work) are added. **It is an
  extrapolation from a sample; do not quote it as a count.**
* **Four new criticals/highs were filed against `crates/cyrup-mcp`**, of which the load-bearing one is
  `MCP-500`: `URL_BOUND_AUTH_FIELDS` is five fields upstream and four in cyrup. It is not a live leak
  *today* only because `bearerTokenStore` (`MCP-501`) is unported — **land the two together, never
  `MCP-501` first**. `crates/cyrup-mcp/src/config.rs`'s own header comment asserts *"the symbol
  appears at no upstream tag"*, which is **false at v2.32.1** and must be corrected in the same
  change.
* **The 13 `TODO(MCP-NNN)` markers in `crates/cyrup-mcp/src` are a floor on the open set, not its
  size** — six ids are open at HEAD with no marker. `README.md`'s "9 `TODO(MCP-NNN)` ids remain" is
  corrected to 13 with that caveat.

## Recommended next batch

Ranked by what a wrong entry costs, not by effort.

1. **The six rows this pass filed against batch-3's own code, as one batch** — `SUBA-095`,
   `DRIFT-054`, `EXT-076` (all `medium`), then `TUI-096`, `TUI-097`, `SEAM-118`, `DRIFT-055`. They
   are small, they are all in code landed in the last two days while the context is still recoverable,
   and three of them are things a closed row currently denies. Doing them first also closes the
   review-debt loop the restart opened, instead of letting it compound into a fourth batch.
2. **Run `cargo nextest run -p cyrup-it --features it` and publish the number.** It is the batch's
   largest unmeasured claim, three rows quote figures nobody re-derived, and two of the commits
   under those rows add tests to that suite. `CYRUP_IT_BIN_DIR=/home/user/cyrup/target/debug` is what
   makes it fit on a full disk.
3. **Area 13, `MCP-500` + `MCP-501` together**, with the false `config.rs` header comment corrected in
   the same change. It is the only credential-shaped item in the directory, and the eighth edition
   already put area 13 first.
4. **The remaining twelve mediums** — areas 01, 05 (4), 06 (2 after `EXT-076`), 09 (3), 12 (1 after
   `DRIFT-054`). Rank port bugs above version lag above test defects, as the eighth edition did.
5. **The citation rot, once, mechanically.** Two of this pass's ten corrections and three of its
   repairs were line numbers that had rotted within a *day*. `README.md`'s *Work this directory owns*
   has proposed a CI check that resolves every `<file>.rs:<line>` and fails past EOF for three
   editions running; until it exists, every pass will keep spending its budget here. **Prefer a
   `file::symbol` citation to a `file:line` one wherever the symbol is unique** — this pass repaired
   in that form where it could.


# RECONCILED 2026-09-04 (eighth edition) — batch 2: twenty-five medium rows worked in one day on landed code; 18 closed, 6 partially closed, 1 refuted; the set above medium is still ONE row

> **Read this block before planning.** cyrup **code** HEAD **`6cf2cb9f`**, branch
> `claude/beautiful-feynman-odz1v5` (68 commits off `main` = `a4805955`, 35 of them touching
> `crates/`/`xtask`; the last code commit is `6cf2cb9f fix(subagents): SUBA-087 review fixes`). The docs
> commit carrying this block cannot cite its own sha. Counts are `scripts/count_open_items.py`'s, re-run
> after every closure and correction below was written: **103 open = 0 critical, 1 high, 28 medium,
> 74 low; 564 closed** (seventh edition: 125 = 0/1/49/75; 539 closed). §0 of `PARITY-GAPS.md` carries
> the per-area table (tenth edition) and §0a the above-medium table (seventh edition, with a batch-2
> note); neither is duplicated here. **One census correction landed with this edition:** the script's
> hand-enumerated `carried_medium` list (`SUBA-087..091`) was emptied — all five are table rows now —
> so the interim figure of "8 open / 7 medium" for `09a` was a double count; the true figure is 3 / 2.

## How this edition was produced, and what it checked

Each implementer wrote its own row and section in the owning area file in the same batch as the code
(new this batch); five independent review groups then re-read both sides (the Rust at the landing sha
and at HEAD, the TypeScript at the named tag via `git -C tmp/<repo> show`) and blocked on anything
false; review-fix commits followed; and this ledger audit re-opened every one of the twenty-five rows
and sections, confirmed the closure mark, date, landing sha, residual and severity against the reports
and against HEAD, and spot-checked citations with `rg` at HEAD and `git show` at the tag (every one
resolved; the audit's own corrections are listed below). Repo checks at HEAD from the last review:
`cargo fmt --all -- --check` clean, `cargo clippy -p <crate> --all-targets -- -D warnings` clean for
every touched crate, per-crate `cargo nextest run` green (`cyrup-ext-subagents` 2725/2725,
`cyrup-tui` 1372/1372, `cyrup-config` 226/226). **The armed seam suite HAS now been run, and it
found two real test-side breaks this edition originally shipped unmeasured.** `cargo nextest run
-p cyrup-it --features it` (with `CYRUP_IT_BIN_DIR` pointed at pre-built `--features faux` /
`--features test-fixtures` binaries, which is what makes it fit on a 96%-full disk) failed
`background_cascade_integration::a_delivered_stop_request_stops_the_run_and_cascades_to_descendants`
(asserted the LEGACY single-file `control/stop.json` on the descendant; since `SUBA-087` the
cascade writes one file per request under `control/stop-requests/`, pi `requestAsyncStop`,
`runs/background/control-channel.ts:297-310` @v0.64.0) and
`background_runner_main_integration::a_child_scoped_stop_stops_one_chain_step_and_the_next_step_still_completes`
(its second chain step named agent `second`, absent from the shared `all_personas()` fixture, so
the step failed pre-spawn as `Unknown agent: second`). Both are FIXTURE defects, not port defects —
the runner behaviour each test describes is correct at HEAD — and both are fixed at `1a559d33`.
After that fix: **492/492 passing**, `cargo clippy -p cyrup-it --features it --all-targets -D
warnings` clean. Workspace at the same head: `cargo fmt --all -- --check` clean, `RUSTDOCFLAGS='-D
warnings' cargo doc --workspace --no-deps` clean, `cargo clippy --workspace --all-targets -D
warnings` clean, `cargo nextest run --workspace` 8710/8710. **The lesson stands even though the
port survived it: `SUBA-072`/`087`/`088`/`090` edited ~30 `cyrup-it` files that nothing in this
batch executed until now, and a `cargo check` is not a run.**

## What closed, and on what evidence — each row re-read in its area file, not taken from a commit subject

| id | area | landing commit | what landed (see the area row for citations) |
|---|---|---|---|
| `SESS-049` | 03 | `23abca0f` | one pure gate `check_summarization_response` at all FOUR summarization sites (the fourth, `session-svc/session/forking.rs`, the row had not named); pi v0.84.4 `getSummarizationFailure` — first tagged v0.84.4, not v0.84.2 as filed |
| `TOOL-042` | 04 | *measurement* | 300 concurrent `cargo nextest` runs of the archived `cyrup-tools` binaries at a 100 ms tripwire (5× tighter than the gate) under a 10 ms `/proc` pipe sampler: 0 LEAK, 0 FAIL; plus 4 amplified main-tree and 34 stock runs. The harness is named clean from `nextest-runner 0.122.1` source; `2daaa32d` (2026-08-27, post-sweep-8) is recorded as the fix + pins. Closure states its own falsification condition; macOS unmeasured (low residual) |
| `EXT-024` | 06 | `75532cee` | `Tool::render_kind` consumed in the TUI tool row (pi `renderShell: "self"`); **`75532cee`/`651bac70` do not build checked out alone** (another track's hunks were swept in) — disclosed in the row; step to `0e8c62fa` |
| `TUI-037` | 07 | `0e8c62fa` | `/reload` persists an implicitly-granted project trust (`maybeSaveImplicitProjectTrustAfterReload`), pure decision + shell |
| `TUI-068` | 07 | `eacd771a` | `app.session.deleteNoninvasive` (Ctrl+Backspace) bound in `/resume` |
| `TUI-081` | 07 | `84b205a1` | `/import` asks before replacing the live session (first-party `ImportConfirm` selector) |
| `TUI-025` | 07 | `0e8c62fa` | its one residual (`; saved project trust` status variant) landed with `TUI-037`; closed by this audit |
| `SEAM-115` | 08 | `c6142d01` | three RED/GREEN tests over a branched session pin the `context_usage` fixes |
| `SEAM-116` | 08 | `4481e807` | RPC `clear_queue` verb + shared `ClearedQueue` wire struct |
| `SEAM-117` | 08 | `1d9d422f` | `message_update` wire shape: `usage` + `toolcall_start.{id,toolName}` (v0.84.4) |
| `SUBA-072` | 09 | `7791b26a` | per-attempt scratch dir moved out of the project tree to `<run_scratch>/scratch/<cwd_key>` |
| `SUBA-088` | 09a | `ba24e5e5` | `subagents.defaultProvider` / agent `modelProvider` through the launch ladder (promoted from `## Carried`, confirmed with three corrections) |
| `SUBA-089` | 09a | `cde2ddfc` | `isRetryableModelFailureAttempt` — no re-dispatch after tools ran; the paired `connection\s+(error|reset|closed|aborted)` pattern ported with it |
| `SUBA-091` | 09a | `681f6255` | fleet inspector passes pi's two trusted session roots to the transcript fallback |
| `ICOM-053` | 11 | `1d2b4418` | the feature-matrix gate now RUNS the `cyrup-it` seam suite; two seam targets that had stopped compiling fixed on the way |
| `ICOM-060` | 11 | `a91e3c41` | pi-intercom v0.12.1 `5fe0ee3` misdirected-reply guard, line for line |
| `FLUX-001` | 14 | `03f3add0` | bundled prompt/skill tree embedded at build time and materialised under the agent dir (port of `installer.py`) |
| `FLUX-002` | 14 | `c7d21bbb` | the four multi-task templates check for `subagent` before calling it |
| `FLUX-003` | 14 | `4bb3569c` | 37 tests whose every expectation is the upstream Python's output at v0.0.40; six real defects fixed by the pins |
| `FLUX-004` | 14 | `c846ff97` | status overlay moved off the editor's `ctrl+f` to `ctrl+alt+f`, one constant, cross-crate pin against the real default keymaps |

**Refuted (1):** `TUI-089` (07) — cyrup already orders the picker at pi's two points by pi's rules
(`applyModelsJson` replace-in-place / push-at-end, provider-only stable sort); guard test `d685eff1`;
the review corrected the row's `provider-composer.ts` line numbers to `:168-206`/`:199`/`:202-203`.

**Partially closed (6), each with its residual stated at its severity in the row:** `PROV-014` (01,
`1471a16f` — radius + qwen-token-plan ×3 registered; re-rated low: dynamic catalogs, no shell trigger
for `refresh_models`, `configureRadiusProviders` unported), `CFG-067` (05, `91ca02e5` — three of nine
vars ported; still medium for the six mechanism-less ones, chiefly `TOOL_TIMEOUT_MS` and the
run-fan-out ledger), `EXT-041` (06, `a0134787` — tool surface done; custom-ENTRY replay open, low, fix
site area 08), `SUBA-087` (09a, `2d9d0d0a` + `6cf2cb9f` — child-scoped stop; group members not
individually addressable → **filed as `SUBA-093`**, medium), `SUBA-090` (09a, `79ee7eff` — the
`display` predicate; session-svc drops it on the trigger-turn path → **filed as `SUBA-094`**, medium,
fix site areas 08/03), `FLUX-005` (14, with `4bb3569c`, low).

## The above-medium set: ONE row, unchanged

| id | area | sev | why it is above medium |
|---|---|---|---|
| `SUBA-074` | 09a | high | Agent `runner:` frontmatter — stage 1 (the refusal path) closed 2026-09-04; **stage 2, the external-runner adapter protocol itself, is the open residual under this id.** Effort L, needs design; unchanged this edition. |

## Corrections this audit made in place (docs only)

`TOOL-042` closed with the finished measurement (the implementer's note was written at 110 of the 300
runs); `TUI-025` closed (residual landed); `TUI-089`'s "tooling residual" marked resolved (`9cd2d6f0`
cleared the `input_reader.rs:443` lint `3e69ea2a` introduced); `09a` `SUBA-090` `notify.ts:444` →
`:440`; `14` `FLUX-003`'s quotation of the retracted "another team owns tests" line annotated; `CFG-080`
filed (below); batch-2 recount blocks added to 06/07/09/11/14's `## Open items` headers; the census
script's `carried_medium` list emptied. Everything else in the twenty-five rows was found as reported.

## Residual leads the closures produced — recorded, ownerless unless an id is named, NOT counted

(1) `SEAM-115`: pi's footer occupancy is `estimateContextTokens` — `usage.totalTokens` preferred plus a
trailing-messages estimate (`compaction.ts:202-230`, `:146-148` @v0.84.4); cyrup's
`ContextUsage::from_last_assistant` is the four-field sum alone (low, area 08, unfiled). (2) `PROV-014`
review: `baseten` (`all.ts:95` @v0.84.4) is now the one unregistered v0.84.x built-in, with no row
(area 01). (3) `EXT-041`: `ReplayItem` has no custom-entry variant, so `cyrup-intercom`'s inbound card
is lost on `/resume` (area 08 producer). (4) `TUI-068`: pi refuses to delete the CURRENT session from
`/resume` (`session-selector.ts:398-401` @v0.84.4); neither cyrup delete path has that guard (low, 07).
(5) `TUI-081`: pi's `MissingSessionCwdError` re-prompt (`interactive-mode.ts:6084-6095`) unported;
`import error:` vs `Failed to import session:` channel (low, 07). (6) `FLUX-001`: the three sibling
`CARGO_MANIFEST_DIR` resolvers in `cyrup-ext-subagents` (`registration/resources.rs:46`, plus the
agents-dir twin the `FLUX-001` report names) and `cyrup-intercom` (`resources.rs:41`) are untouched — the `build.rs` +
`install.rs` mechanism is reusable as-is (medium class, areas 09/11, unfiled). (7) `SUBA-091`: the
`trustedSessionFiles`/`trustedSessionFileRoot` second containment rung and `trackedJob.sessionRoot`
are unported; `subagent status view:transcript` trusts a different root triple than pi (low, 09a row).
(8) `SUBA-088`: a bare model id offered only by a provider other than the preferred one is forced
onto the preferred provider and fails in the child (`resolveExactIdMatches` unique-match fallback
absent; low). (9) `SUBA-087`: cross-process `ts` ties in stop-request names still drain arbitrarily
(identical to upstream); a pi-shaped request file without `source` is DROPPED by `parse_stop_request`
because cyrup's `StopRequest` requires `ts`/`source` (upstream optional, `control-channel.ts:56-57`) —
an undeclared interop `[CYRUP-DELTA]`, low. (10) `SEAM-116`: `data` key order `followUp, steering`
(BTreeMap) vs pi's literal order — meaningless on the wire, documented. (11) `SEAM-117`:
`exec/ndjson.rs:100-106`'s "two-key object" doc comment is stale (area 09). (12) `ICOM-053`:
`crates/cyrup-it/tests/misc/main.rs` is an empty `[[test]]` target; nothing runs the gate for anyone
(no CI, no schedule — README says so). (13) `SUBA-072`: `.gitignore:20`'s `.cyrup-subagent-scratch/`
entry is dead. (14) Git history: `75532cee` and `651bac70` do not build alone (see `EXT-024`); the
brief's "row in the SAME commit as the code" rule was followed by no track — every closure is a code
commit plus a `docs(gap-analysis): <id> row` commit. (15) `TUI-N11` (07) reads "**fixed this pass**"
in its title cell while its severity cell is unstruck, so the script counts it open medium — settle it
next pass (not touched here: neither side was re-read).

## Workspace hygiene and retractions this batch

- **`3e69ea2a style: apply rustfmt across the workspace`** — the seventh edition's lead (6) is
  discharged: the workspace is `cargo fmt --all -- --check` clean and every track kept it so. The
  reflow introduced one `clippy::redundant_closure` denial at `crates/cyrup-tui/src/app/input_reader.rs:443`
  that blocked `-D warnings` for every crate depending on `cyrup-tui` for most of the day; `9cd2d6f0`
  (with `TUI-081`) cleared it. Every "pre-existing lint" note in a batch-2 row refers to that window.
- **`d0d601c9`** retracted "another team owns area 13" in `README.md`, `PARITY-GAPS.md` and this file;
  **`f239fc3d`** retracted "another team owns tests / benchmarks" from the `/split` template, its
  `.claude/commands` copy, `spec/flux/README.md`, three `.flux/backlog` tasks and the three ledger
  files. This audit grepped `docs/gap-analysis/**` for any surviving ownership claim: the only hit was
  `14-cyrup-flux.md`'s `FLUX-003` Fix paragraph QUOTING the retracted spec line, now annotated as such.
  No text in this directory says another team owns anything.
- Untracked `docs/gap-analysis/scripts/__pycache__/` was deleted and is not committed.

## Recommended next batch — area 13 first, then the mediums ranked port bugs → test defects → S-effort drift

1. **Area 13 — the MCP port. Re-audit `13-cyrup-mcp-STATUS.md` (and `13a`–`13i`) against
   `tmp/pi-mcp-adapter` at v2.32.1 FIRST**: `git -C tmp/pi-mcp-adapter diff --stat v2.25.0..v2.32.1`
   and `git -C tmp/pi-mcp-adapter show v2.32.1:<path>` for every file the STATUS file cites, per
   `README.md`'s baseline hazards. No edition has opened those files since the clone was re-pulled;
   area 13 is this repository's work (the "MCP team" claim was retracted) and is outside the census
   only by counting rule.
2. **Port bugs (medium):** `TOOL-022` (04; fix site `cyrup-tui` + `cyrup-core`), `EXT-006` (06),
   `TUI-046` (07, Kitty flag 7 — needs the guard flags first), `TUI-N02` (07), `SEAM-015` (08),
   `SUBA-093` (09a, new — status-model change), `SUBA-094` (09a → areas 08/03, new — `display` through
   `AgentMessage::Custom`), `ICOM-054`, `ICOM-055` (11), the `EXT-003`/`EXT-039`/`EXT-064` partials
   (`EXT-039`'s `resolve_shortcuts` caller is what `FLUX-004` proved live), `EXT-041`'s custom-entry
   half (08), `CFG-067`'s six remaining vars (`TOOL_TIMEOUT_MS` and the run-fan-out ledger want
   area-09 items of their own), `PROV-042` (01), `DRIFT-041` and `DRIFT-015` (12; the latter a
   duplicate of `EXT-019`). `CFG-020` and `SUBA-016` are L and stay parked. `CFG-068`/`CFG-074` are
   invented-surface decisions, not ports.
3. **Test defects:** `TUI-N11` (07 — probably already fixed; settle the row), then the `cyrup-it
   --features it` run (not a row, a gate).
4. **S-effort drift:** `TUI-004` (07), `SUBA-054`, `SUBA-056` (09), `DRIFT-004`/`DRIFT-009`/`DRIFT-053`
   (12), the low `CFG-078`/`CFG-079` (05, v0.84.4 additions) and `CFG-080` (05).
5. Leads (1), (2) and (6) above are one both-sides read from being rows; file them before the batch.

---

# RECONCILED 2026-09-04 (seventh edition) — seven of the sixth edition's eight above-medium rows closed the same day, on landed code and on two live runs; the set is now ONE row

> **Read this block before planning.** cyrup **code** HEAD **`275c1f85`**, branch
> `claude/beautiful-feynman-odz1v5` (five code commits off `main` = `a4805955`, itself the merge of the
> sixth edition's `claude/gap-analysis-refresh`). The docs commit carrying this block cannot cite its
> own sha; `275c1f85` is the last commit that changed `crates/`. Everything below this block is the
> sixth edition and earlier. Counts are `scripts/count_open_items.py`'s, re-run over the fourteen
> files after every closure below was written: **125 open = 0 critical, 1 high, 49 medium, 75 low;
> 539 closed** (sixth edition: 132 = 0/8/49/75). §0 of `PARITY-GAPS.md` carries the per-area table
> (ninth edition) and §0a the above-medium table (seventh edition); neither is duplicated here.

## What closed, and on what evidence — each row re-read in its area file, not taken from a commit subject

Five `09a` rows closed on **landed code, both sides read** (the Rust after the landing commit, the
TypeScript at v0.57.0 and v0.64.0 via `git -C tmp/pi-subagents show`, the commit diff rather than its
subject, plus an independent review that re-read both sides again; `cargo clippy … -D warnings`
clean and `cargo nextest run -p cyrup-ext-subagents` 2666/2666 at `275c1f85`):

| id | landing commit | what landed (see the `09a` section for citations) |
|---|---|---|
| `SUBA-085` | `5e3aa1c8` | `mission.resolve-decision` — seventh verb, store transition, upstream's open-decision status gate; goal driver moves past a resolved decision (pinned) |
| `SUBA-092` | `247ff97b` | `excludeTools:`/`allowNestedSubagents:` — frontmatter, override, serializer, spawn-plan subtraction and nested-fanout grant, `--exclude-tools` to the child |
| `SUBA-082` | `5a4ae4ed` | `acceptanceRole:`/`acceptance:` in the schema; role is the primary `infer_level` input; single-agent `acceptance:` launch default — **promoted out of `## Carried` first** (upstream re-read at both tags, confirmed as filed) |
| `SUBA-084` | `dee8b9d0` | `RuntimeAgentRegistry`, `AgentSource::Runtime` (rank 4), collision checks, merge in `run_discovery`, clear on shutdown, public `register_agent` — **promoted first**; effort corrected L → M |
| `SUBA-086` | `275c1f85` | `AgentDiscoveryDiagnostic`, `parse_agent_file_checked`, `find_blocking_agent_diagnostic`; rendered by `list`/`get`/`models`/doctor, enforced at both delegation seams — **promoted first**, with three corrections to the filed text |

Two live-use rows closed on **observation** (`REPRO-LOG.md` §0e), per this file's own rule that no
`TUI-*`/live item closes on a static read:

- **`TUI-091`** (07) — **duplicate of `TUI-090`.** Driven in tmux 3.4 on a real pty at 120×40, HEAD
  `a4805955`, the owner's exact `together`/Kimi-K3/`high` path through a local SSE server: the
  reasoning block rendered live and committed above the answer in **seven variants** (short and
  alternative delta fields, 90-line LONG with 0 rows lost, a TOOL round-trip, fullscreen,
  `hideThinkingBlock`, `--continue` replay). `TUI-091` was filed 16:26 on 2026-08-15; `TUI-090` was
  fixed 19:50 the same day with a commit body naming this exact asymmetry. No code changed.
- **`SEAM-113`** (08) — **REFUTED as an open bug: stale under ADR-0006.** The row settled its contract
  against v0.83.0; at the parity target v0.84.4 the contract is opt-in Ctrl+S persist and cyrup matches
  it path for path (re-read on both sides). The row's "rank 4 input is permanently empty" claim is
  false at HEAD — `crates/cyrup/src/bootstrap.rs:247-275` reads `defaultProvider`/`defaultModel` back,
  proven by a headless `--mode rpc` run that reproduced the symptom and then cleared it by seeding the
  keys. The `--default` flag residual never shipped in any pi tag (`5133c9284`, 2026-08-20, deleted it
  inside the v0.84.3 window). The matched `set_thinking_level` sibling is dispositioned in the row.

## The above-medium set: ONE row

| id | area | sev | why it is above medium |
|---|---|---|---|
| `SUBA-074` | 09a | high | Agent `runner:` frontmatter — stage 1 (the refusal path) closed 2026-09-04; **stage 2, the external-runner adapter protocol itself, is the open residual under this id.** Effort L, needs design; unchanged this edition. |

## Residual leads the closures produced — recorded, ownerless, NOT counted

Each is cited in the closing row and collected in `09a`'s second 2026-09-04 summary blockquote; none
was read on the confirmed bar by this edition, so none is a row: (1) pi-subagents v0.63.0 `0128385f`
— `inferLevel` returns `none` for read-only reviewers and gates dynamic escalation on
`dynamicResolvesReadOnly` (`v0.64.0:src/runs/shared/acceptance.ts:105,107,110-111,137`); cyrup's
`infer_level` is deliberately the v0.57.0/v0.62.0 body. (2) v0.63.0 `31562d76` — custom-agent
overrides no longer gated on frontmatter presence (`v0.64.0:src/agents/agents.ts:1476`); cyrup's
`apply_custom_override` still fill-unset for all 20 fields. (3) v0.64.0's runtime-agent EVENT bridge
(`src/agents/runtime-agent-events.ts`) — needs a request/response topic over cyrup's queued
`SharedBus`. (4) Five `RuntimeAgentDefinition` fields refused with a marked `[CYRUP-DELTA]` pending
their `AgentDefinition` landings. (5) `TUI-091`'s two cosmetic observations (live-tail duplication
when the inline viewport exceeds the screen; `--no-extensions` not governing the compiled-in
`cyrup-flux`). (6) Tooling: the workspace is not `rustfmt`-clean at `a4805955` under the pinned
toolchain (no `rustfmt.toml`); every implementer hit it — a repo-level decision.

## What this edition explicitly did NOT do

- Did not re-walk any medium/low row in any area; the 49/75 are exactly the sixth edition's.
- Did not re-audit area 13: `tmp/pi-mcp-adapter` was re-pulled to **v2.32.1** (`README.md` Baselines
  records the new HEAD); its files were not opened. *Correction: this edition said the MCP team owns
  that area. No such team exists — area 13 is this repository's work and belongs in the next batch.*
- Did not file the six leads above as rows — each needs both sides read on the confirmed bar first.

---

# RECONCILED 2026-09-04 (sixth edition) — a full re-audit of all fourteen files, `09a` merged into this table for the first time, and the count is now a committed script rather than a hand-count

> **Read this block before planning.** cyrup HEAD **`2571969`**, branch `claude/gap-analysis-refresh`
> — 210 commits ahead of the `4fb5e40` baseline every area file (01–12, `09a`, 14) was independently
> re-audited against this pass, each with both sides personally re-read (the Rust at HEAD, the
> upstream at its named tag via `git show`, never a working tree), per `README.md`'s evidence rule.
> Everything below this block is the fifth edition and earlier.
>
> This edition is built directly from **`PARITY-GAPS.md`'s §0 and §0a**, which for the first time are
> produced by a committed script (`scripts/count_open_items.py`) rather than derived by hand — the
> fourth edition's own complaint, three editions ago, that "a prose rule two readers implement
> differently cannot validate an edition" is answered by this, not by a better prose rule. Running the
> script myself against the fourteen files' current `## Open items` tables reproduces §0's table
> exactly (132 open = 0/8/49/75, 8 above-medium) — see the run recorded in the sanity-check section
> below. **Do not duplicate §0's by-area census here**; `00` ranks, `PARITY-GAPS.md` classifies by gap
> class and carries the counts, and copying a count into a second file is exactly the drift this
> directory has paid for before (area 12 was 50% false-positive on exactly this failure — see the
> fourth edition's note below).

## The above-medium set: still eight rows, zero critical — but SIX of the eight are new to this table

**Of the fifth edition's three high-severity survivors, one closed this pass.** `PROV-068` (area 01)
was refuted and closed 2026-09-04 — re-read at the ported tag `v0.83.0` itself, not a later citation:
`mapped === null` really does mean UNSUPPORTED on both sides (`packages/ai/src/models.ts:668`;
`crates/cyrup-provider/src/collection.rs::get_supported_thinking_levels`); the item's own live-use
report (a two-rung Kimi K2.6 ladder) turns out to be pi's own catalog data
(`packages/ai/test/together-models.test.ts:24` @v0.83.0), not a cyrup bug. `TUI-091` and `SEAM-113`
remain open, both re-confirmed directly against area 07 and area 08's current rows, not carried
forward on trust.

**The other six rows are new to THIS TABLE, not new to the backlog.**
[`09a-cyrup-ext-subagents-v0.57-drift.md`](09a-cyrup-ext-subagents-v0.57-drift.md) is a same-tier
supplement to area 09 (`README.md`'s Contents table), and no prior edition of this table — not the
fourth, not the fifth — ever drew from its severities. `PARITY-GAPS.md` §0a folded it in for the first
time this pass; this edition does the same. Nine of `09a`'s own confirmed-schedulable items closed
outright this pass (see area 09/09a's summary), which is why only six of its rows still clear `high`.

| id | area | sev | why it is above medium |
|---|---|---|---|
| `TUI-091` | 07 | high | Reasoning blocks never render although every layer — provider through renderer — is wired and correct. Live-use report 2026-08-15; its last named live-render candidate is REFUTED, by execution through two harnesses including a pty-equivalent, so the row is relocated into the live event fold (`app/events_fold.rs:121-125`). Needs three named projections instrumented in a real terminal, not more static tracing. **Not re-touched 2026-09-04** — this pass had no pty available; left exactly as written, per this file's own rule that no `TUI-*`/live item closes on a static read. |
| `SEAM-113` | 08 | high | A model chosen with `/model` does not survive into the next session — it reverts to the catalog/settings default. **Evidence substantially expanded 2026-09-04, not just re-confirmed**: `82f40d3` (2026-08-28) landed pi's LATER opt-in Ctrl+S persist contract — a new `ConfirmSelectionAsDefault` path (`crates/cyrup-tui/src/app/execute_misc.rs:416-462`) that writes the settings default only when the user explicitly presses Ctrl+S — instead of the unconditional persist inside `apply_model_change` (`crates/cyrup-session-svc/src/session/model.rs:473-539`) this ledger settled on 2026-08-19 as the actual fix site. Plain `Enter` in the picker and typed `/model <pattern>` both still resolve through `apply_model_change` with no settings write, so the originally reported symptom is unchanged. Stays open, unreduced, at high. |
| `SUBA-074` | 09a | high | Agent `runner:` frontmatter is ignored entirely, so a sandboxed foreign-CLI profile runs as a full-capability native child. **Stage 1 (the refusal path) CLOSED 2026-09-04** — `AgentRunnerConfig::refusal_reason`, called before the model ladder in `exec/mod.rs` Step 0b, refuses the run with pi's own message. **Stage 2 (the external-runner adapter protocol itself) is the open residual under this same id** — confirmed still absent. |
| `SUBA-085` | 09a | high | `mission.resolve-decision` unported: a mission decision is write-once and permanently open, so the goal driver proposes the same next action forever. Re-verified open 2026-09-04 — `resolve_decision`/`ResolveDecision` is 0 hits anywhere in `crates/`, and `MissionUpdateInput` has no such field. |
| `SUBA-092` | 09a | high | **New 2026-09-04.** Agent-level `excludeTools:`/`allowNestedSubagents:` (frontmatter and settings-override) are entirely unported — a declared per-agent tool exclusion has no effect, and a nested-subagent grant can only ever come from an explicit `tools:` allowlist. Both sides personally read at `pi-subagents` v0.62.0, inside the v0.57.0..v0.64.0 window that sits past `09a`'s own original v0.57.0 scope. |
| `SUBA-082` | 09a | high | Agent `acceptanceRole:`/`acceptance:` frontmatter is not in the schema, so the acceptance classifier is driven purely by an agent-name regex. *(Carried, NOT adversarially verified — `09a`'s own lower evidence bar for this section: the port-side zero-hit grep was re-run and still returns zero at HEAD, but the upstream line numbers were not re-read, this pass or the original filing.)* |
| `SUBA-084` | 09a | high | Runtime agent registration is entirely absent — no `registerAgent` API, no `runtime` source tier, no runtime/configured collision checks. *(Carried, NOT adversarially verified — same caveat as `SUBA-082`.)* |
| `SUBA-086` | 09a | high | Per-agent parse diagnostics are absent: a malformed agent file is silently degraded to defaults instead of being reported by name and blocking that one agent. *(Carried, NOT adversarially verified — same caveat as `SUBA-082`.)* |

**Read `09a`'s own `## Summary — confirmed items` table and its `## Carried — NOT adversarially
verified` section before scheduling any of the three carried rows.** They are held to a lower evidence
bar than the other five by `09a`'s own header (port-side re-checked, upstream not re-read), and this
table does not launder that difference away — it is stated in each row above rather than left implicit.

**Six of the eight rows are effort M or smaller per their area files; none is blocked on a decision**
the way several prior above-medium rows were (`PB-7`'s npm channel, `PB-19`'s Windows question). The
two live-use rows (`TUI-091`, `SEAM-113`) are held to this directory's highest evidence bar — no
`TUI-*`/live item is done until it has been observed in a real terminal — while the six `09a` rows are
static-read findings at a named tag, three of them at `09a`'s own lower "carried" standard.

## Closures verified this edition, area by area — not taken from any area summary's prose alone

Every closure named below was independently re-checked against the cited area file's current row
before being treated as fact here, per the "refute closures rather than confirm them" rule:

- **`PROV-068`** (01) — closed, see above.
- **`SEAM-112`, `PERM-034`, `TUI-092`** — all three closed **before** this pass (2026-08-29 /
  2026-08-20) and re-confirmed unchanged by their owning areas' 2026-09-04 re-audits (area 08, area
  10, area 07 respectively); no new evidence this edition, carried forward correctly.
- **`CFG-021`, `DRIFT-022`** (fullscreen/alt-screen TUI mode) — both closed 2026-09-04 in areas 05 and
  12, on the same shipped code (`crates/cyrup-tui/src/altscreen/`, `crates/cyrup-config/src/settings/`).
  Neither was ever above-medium, but both directly answer `PARITY-GAPS.md` §6 OQ-8, which is now
  marked ANSWERED there.
- A further **~50 rows closed across areas 01, 02, 03, 04, 05, 06, 07, 09a, 11, 12** this pass (see
  each area's own summary and `PARITY-GAPS.md` §0's per-area table) — none of them were above-medium
  before this edition, so none change the ranked table; they are not re-listed here to avoid the exact
  duplicate-census drift this file's own fourth edition documents at length below.

## What this edition explicitly did NOT do

- **Did not re-derive the full by-area open-item census** (crit/high/medium/low per file). That table
  now lives in `PARITY-GAPS.md` §0, is script-derived, and was independently re-run against the
  fourteen files by this pass to confirm it (132 open = 0/8/49/75 — matches exactly). Duplicating it
  here would create a second hand-maintained copy of a number a script already owns.
- **Did not re-walk any area file's medium/low rows for ranking purposes.** The above-medium table is
  the whole of this edition's ranked content, per this file's own stated purpose ("picking the next
  work item") — everything at medium or below is read from the owning area file directly, not from
  this ledger.
- **Did not touch `README.md`'s "carries no counts, no record of past passes" note.** The Baselines
  table there is data (upstream tags, HEAD SHAs), refreshed separately; this file's own historical
  editions below remain the only place a running pass-count lives, unchanged.

---

# RECONCILED 2026-08-19 (fifth edition) — the above-medium set turned over COMPLETELY, two criticals are open, and `crates/cyrup-tui/src/app.rs` no longer exists

> **Read this block before planning. It does not recount the backlog — it corrects the two things a
> planner reads first and cannot get from the tables: which rows outrank the rest, and whether a
> citation can be followed.** cyrup HEAD **`4fb5e40`**, branch `david/cyrup`. Everything below this
> block is the fourth edition and earlier.

## The above-medium set: all five fourth-edition rows are closed, and CRITICALS replaced them

**`0 critical, 5 high` is false at HEAD, in both directions.** The five rows the fourth edition
named — `SESS-040`, `PROV-047`, `PROV-054`, `PROV-055`, `PROV-056` — all closed on 2026-08-15. What
sits above medium now is **six rows the fourth edition had no entry for — 2 `critical` + 4 `high`**,
and it was 3 + 3 until `TUI-092` was de-escalated inside this batch:

| id | area | sev | why it is above medium |
|---|---|---|---|
| `SEAM-112` | 08 | ~~**crit**~~ **FIXED 2026-08-29** | `/resume` produced a broken session — nothing rendered and bash tool calls repeated endlessly. **Both halves are now closed.** Render: `879eb4e`. Repetition: **the one-shot overflow brake was unarmable for `StopReason::Length`.** pi guards the latch clear with `stopReason !== "error" && stopReason !== "length"` (`agent-session.ts:678` @v0.84.3) and the retry counter with `!== "error"` alone (`:684`); the port fused the two statements into one early `return` and kept only the arm they share, so `on_assistant_message_end` (`cyrup-session-svc/src/session/run.rs:285`) cleared `overflow_recovery_attempted` on every `Length` message — immediately BEFORE `check_compaction` reads it (`session/auto_compaction.rs:85`) for the one overflow case a `Length` message triggers (`is_context_overflow` case 3, `cyrup-provider/src/utils/overflow.rs:101-109`). The read was therefore always `false`, the brake at `:85-98` was unreachable code, and compact-and-retry re-drove the interrupted turn without bound — re-running the identical tool call on every pass. **Fix:** split the guard so the latch clear skips `Length` while the retry-counter reset still runs for it, restoring pi's truth table exactly. **Candidates (1)-(3) are all struck as disproved** — see area 08. |
| ~~`PERM-034`~~ | 10 | ~~**crit**~~ **CLOSED 2026-08-29 — REFUTED** | "Allow Always" does not stick. **Not a gap: the port is faithful and no code changed.** Every mechanism producing the symptom was traced to behaviour pi has too — both sides clear `sessionApprovals` unconditionally from `session_start` AND `session_shutdown` (`pi-permission-system` `index.ts:1828-1831`/`:1862-1865` @v0.8.0), so a reload wiping always-grants is upstream behaviour, now pinned by a test. The subject round-trips byte-identically for simple, reported and COMPOUND commands (three new tests), and the per-instance-store suspect is structurally impossible — `SessionFactory::build_with_parent` clones the `Vec<Arc<dyn NativeExtension>>`, so one instance is shared process-wide. **Falsification condition recorded in area 10:** reopen only on a re-prompt inside one session with no reload and no session switch between the prompts. |
| ~~`TUI-092`~~ | 07 | ~~crit~~ ~~**high**~~ **CLOSED 2026-08-20** | **F1-F8 all landed; F8 (by-value ingest) closed it in `24b6ffe`.** What remains is a measurement, not a defect: one long post-round-2 live session to confirm the four-phase degradation is gone. Progressive lockup of the TUI. **De-escalated `critical` → `high` in this batch**, because all three clauses that justified `critical` are false at HEAD: `Ctrl+D` IS bound (`keymap.rs:655` → `app/input.rs:126-129` → `app/run_action.rs:16` returns `RunFlow::Break`), `Ctrl+C` IS bound (`keymap.rs:656` → `app/input.rs:219-231`), and `TUI-088` is CLOSED. |
| `PROV-068` | 01 | high | An explicit `null` in `thinkingLevelMap` reads as UNSUPPORTED (`cyrup-provider/src/collection.rs:794-807`), collapsing most reasoning models to two rungs. |
| `TUI-091` | 07 | high | Reasoning blocks never render although every layer is wired. **It does not reproduce headlessly at HEAD** — proven by RUNNING both the assembled `TestBackend` path and the pty-equivalent `CaptureBackend` harness, so the row is now RELOCATED out of the render path into the live event fold (`app/events_fold.rs:121-125`). Needs three named projections instrumented in a real terminal, not more tracing. |
| `SEAM-113` | 08 | high | A model chosen with `/model` does not survive into the next session. **Contract settled: it is (a) — pi persists unconditionally at v0.83.0 from four sites, cyrup ports the transcript half and drops the settings half.** One edit, in `cyrup-session-svc/src/session.rs:4532` (`apply_model_change`), plus three decisions and a matched sibling (`set_thinking_level`) that must land with it. |

**Every one of the six was filed from LIVE USE on 2026-08-15 or later.** That is the finding, not the
turnover. Nine reading sweeps plus a nine-surface enumeration produced a five-row above-medium set of
which every row was a wire-or-wiring defect a reader can see; four days of running the binary
produced three rows rated `critical` on arrival, none of which any reading pass had a row for.
**`README.md`'s TUI caveat generalises:
the above-medium set is the part of this backlog a static method is structurally worst at
populating**, and the fix is not a better sweep — it is more hours in a real terminal.

## Why this edition publishes NO recount, and what to do instead

The `237 = 0/5/88/144` and `606 rows / 396 closed` figures below are stale by roughly a hundred rows:
five closing batches landed on 2026-08-15 and none of them reconciled this file. **A replacement
total is deliberately not published, for three reasons, and the third is the one that matters.**

1. **The twelve tables were being edited by other writers in the same batch that produced this
   block.** This file already refuses to restate one count mid-batch for exactly this reason — see
   the fourth edition's *"deliberately NOT restated by one slice mid-batch; recount the table"* —
   and the same discipline applies to twelve at once.
2. **The denominator moved in the same batch.** `crates/cyrup-flux` had no area file at all until
   [`14-cyrup-flux.md`](14-cyrup-flux.md) was opened here with 7 rows, so **every total below this
   block excludes a shipped 1 513-line crate**, and every total above it must include one.
3. **THE COUNTING RULE AS WRITTEN IS NOT REPRODUCIBLE BY A SECOND READER, and this edition measured
   that.** An independent implementation of the rule restated in the fourth edition's
   *"Rule, restated: …"* parenthesis, run over the twelve files
   *as they stand at `e5c6933`*, returns **500 rows / 153 open = 0/2/63/88** — the SECOND edition's
   figure, not the third's `503 / 145 = 0/2/61/82` that the same commit is supposed to reproduce. The
   gap is 8 rows and it is not attributable to file drift, because the input is a fixed git object.
   **Publish the script, not the rule.** A prose rule that two readers implement differently cannot
   validate an edition, and this file has rested four editions on the claim that it can.

**What a planner should do:** take the six rows above as the ranking, and recount from the tables
with a committed script once the batch settles.

## `crates/cyrup-tui/src/app.rs` was deleted, and 78.6% of this directory's citations do not resolve

**`40821ed` split the 10 607-line `app.rs` into `crates/cyrup-tui/src/app/` (33 modules).** Every
`app.rs:NNNN` in this directory therefore names a file that does not exist. Across the four
cross-cutting files — this one, `PARITY-GAPS.md`, `README.md` and `REPRO-LOG.md` — all eight were
re-resolved by symbol against HEAD and rewritten in place. **Three had additionally been mis-remapped
by an earlier automated pass into ranges whose START was rewritten and whose END kept the pre-split
number**, so they ended thousands of lines past EOF: `app/submit.rs` ending at 2343 against a
230-line file, `app/run.rs` at 8091 against 362, `app/execute.rs` at 4639 against 447. **And one was
routed through the wrong module entirely** — the `/tree` label-write trace (`TUI-027`'s row here,
`PB-28` in `PARITY-GAPS.md`) was carried forward as `app/tree_nav.rs`, and `tree_nav.rs` has no part
in it. The real chain at HEAD is `app/selectors.rs:201-208` (split the `FIELD_SEP` payload into
`AppCommand::SetEntryLabel`) → `app/execute.rs:288-298` (`host_services.set_label`) →
`manager.append_label`. **A mechanically-correct remap of a citation that was already wrong is still
wrong; verify each remap by reading the target, not by trusting the match.**

**The wider measurement, because it changes how every row in this directory should be read.** A
citation audit at `4fb5e40` scored the whole corpus: **6 249 `.rs` citations, of which 4 119 of the
5 241 scoreable non-`app.rs` ones point at a line that no longer holds what the prose says — 78.6%.**
The cause is not neglect: **3 336 of the 4 335 absolute citations (77%) were written in ONE commit,
`72cd292` (2026-08-13)**, and 105 commits / +137 184 lines have landed in `crates/` since. Three
consequences:

- **Never renumber by offset.** Only 14% of same-file citation groups share a single shift;
  `transcript.rs` drifts in six distinct bands, `cyrup-session-svc/src/session.rs` in 65. The
  previous uniform-shift pass introduced errors at 15% *while looking verified* — the caveat
  `README.md` already carries, now with a number behind it.
- **It is mechanically fixable anyway.** Recovering each cited line's TEXT from the commit that last
  touched the doc line and re-finding that text at HEAD resolves **2 862 of the 4 119 (69%)** to a
  unique new line; 499 are ambiguous and 705 are unrecoverable because the cited code is gone.
- **Add the guard, not just the rewrite.** Extract every `<file>.rs:<line>` and `` `:<line>` ``,
  resolve, and fail on any line or range end past EOF. That one check catches every range-vs-EOF
  breakage and the six citations in this directory that are impossible under *any* file of that name,
  with no judgement call — and it is the standing countermeasure this file has been missing.

## Do not trust a commit subject as landing evidence

`e6f298d`'s subject reads *"…land TUI-092 F5-F8"* and its diff **deletes**
`bugs/TUI-092-F8-by-value-ingest.md`. F8's code change was never written. At HEAD
`crates/cyrup-tui/src/app/run_action.rs:281` still carries the future-tense comment
``// F8 swaps this one call to `ingest_session_event_owned(ev, &session)`.`` and `:282` is still
`self.ingest_session_event(&ev, &ctx.session).await;` — by reference;
`grep -rn 'ingest_session_event_owned' crates/` returns exactly that comment and no function.
**A deleted task file is not a landed fix.** Before closing any sub-defect row, grep for the artifact
the fix was supposed to create, not for the commit that claims it.

## Landed work that had no row — five ids and one whole crate, every one filed in this batch

The fourth edition's own rule (*"grep the SOURCE for `AREA-NNN` citations at every reconciliation,
not just the docs"*) caught `EXT-M03`. **It does not catch work whose id lives only in a commit
subject**, and six instances had accumulated by 2026-08-19 — including an entire new crate. They
were filed in the same batch that wrote this block; the table records what each was and where it
landed, so the *class* stays auditable rather than merely closed.

| what landed | where it is cited | disposition |
|---|---|---|
| `TUI-093` — the terminal is never asked for its cursor position mid-session; `InlineBackend` answers from a tracked anchor (`crates/cyrup-tui/src/app/backend.rs:91`) and is a **new public export** (`cyrup-tui/src/lib.rs:98`) | **16 occurrences across 5 files** — `app/backend.rs`, `app/crossterm.rs`, `app/draw.rs`, `terminal_query.rs`, and `src/tests/resize_viewport_failure.rs` | **FILED AND CLOSED 2026-08-19 as `TUI-093`** (`07-…`, FIXED 2026-08-17), in one row, per the established `TOOL-042`/`EXT-M01` pattern. The row has to keep the *decision* — the terminal is never asked where its cursor is — or a later pass restores the removed probe as missing pi behaviour |
| `TUI-094` (`879eb4e`) — a `select!` starvation that produced the exact `TUI-092` symptom: the events arm's irrefutable `maybe_ev =` binding matched a closed stream's `None` and, under `biased;`, starved the swap arm below it. Fixed by a refutable `Some(ev)` pattern (`app/run.rs:344`, rationale at `:335-343`) **plus** hoisting the swap arm above every permanently-ready arm (`:286-296`), with the rebind extracted to `App::on_session_swapped` (`app/run_arms.rs:138`) and pinned by `src/tests/run_loop_swap_arm_reachable.rs` | commit subject only | **FILED AND CLOSED 2026-08-19 as `TUI-094`** (`07-…`, FIXED 2026-08-18). It had no id at all — **and the fix violates `bugs/TUI-092-progressive-lockup.md`'s own "do not touch the `biased;` arm ordering" instruction, which must be amended to state the real invariant (cancel → input → SWAP → rest) or the next agent reverts a fix for a 100%-CPU hang** |
| `CMDHINT_01` (`0b7c4f4` + `bae24f5`) — persistent command-token highlight + argument-hint ghost text, plus an `argumentHint` key added to the `get_commands` RPC payload (`cyrup-session-svc/src/session.rs:2628-2640`, read at `cyrup-tui/src/commands.rs:470-479`) | **13 occurrences across 7 files**, four of them shipped source (`session.rs`, `autocomplete.rs`, `commands.rs`, `editor.rs`) | an ad-hoc id outside the `AREA-NNN` vocabulary. **Resolved 2026-08-19 in three parts:** it CLOSED `TUI-078` (`07-…`, CLOSED 2026-08-17), and the highlight/ghost feature itself is now `TUI-095` (`cyrup-original`, low). **The third part — the `argumentHint` key on `get_commands` — belongs in area 08, `TUI-078`'s own Fix asked for it, and it must be confirmed there.** `README.md` names "RPC payload shapes" as one of the four surfaces the enumeration could not reach, so an unrecorded addition there is invisible to the one method that would find it |
| `npt_*` (`3b022f2` + `fea333d`) — recursive namespaced prompt scan (`cyrup-resources/src/discovery.rs:1764`/`:1780`, name derivation moved to `prompt.rs:60`) | in-source only | **FILED 2026-08-19 as `CFG-077`** (`05-…`, `cyrup-original`, low) — a verified divergence, not drift: pi's `packages/coding-agent/src/core/prompt-templates.ts:136` @v0.83.0 says **"Scan a directory for .md files (non-recursive)"** in `loadTemplatesFromDir`'s own docstring. Unrecorded, the next surface sweep files the recursion as a defect and removes it — **taking every `/flux/*` command with it** |
| Kimi K3 (`2add245`) — `moonshotai/Kimi-K3` hand-added to `cyrup-provider/src/providers/together.rs`, with the guard widened to `20 + ADDITIONS.len()` (`:447-448`) | in-source only | **FILED 2026-08-19 as `PROV-070`** (`01-…`, `cyrup-original`, low). The **first deliberate divergence from the pinned `b0c2a90e` catalog**, and `PROV-018`/`PROV-060`'s whole design is REGENERATION — a row living only in `together.rs` is exactly what the next regeneration drops |
| `crates/cyrup-flux` (`67b73a0`) | `spec/flux.md`, `docs/guide/extensions/flux.md` | **AREA OPENED 2026-08-19 — [`14-cyrup-flux.md`](14-cyrup-flux.md), 7 rows (4 medium, 3 low).** An entire 1 513-line shipped crate that appeared in no count in this directory; the scope-gap note below records why it needed its own numbered file rather than a row in area 06 |

## The suite was de-flaked 2026-08-19 — an unattended pipeline now depends on it, and two hazards remain OUTSIDE that work

**Recorded here because it is not a parity finding and belongs to no area file, but every overnight
run in this repo now rests on it.** The changes are all `#[cfg(test)]`-gated or in `tests/` files; no
production path was altered, nothing was `#[ignore]`d, and the working tree carries them at the time
of writing. **Do not "clean up" a lock that looks unused — each one below is the only thing
serialising a writer against a reader that has no idea it exists.**

**Both rows that were FILED as flakes are refuted as stated, and one of them hid a real flake with a
different cause.** (a) `cyrup-ext` / `caps::http::tests::*` was **already fixed at HEAD** —
`PROXY_SETTING_GUARD` (`crates/cyrup-ext/src/caps/http.rs:1077`) is taken through
`proxy_setting_guard()` (`:1080-1082`) by **36 of that module's 37 tests**; the one that does not is
`build_request_defaults_accept_encoding_scheme_conditionally_and_identity_for_range` (`:1684`), a
pure unit test that touches no client. Baseline before anything was changed: **5/5 clean, 289
passed**; no change made. (b) `cyrup-provider`'s *"`configure_http_proxy` shared across parallel
tests"* **cannot** be the cause — the only test that calls it lives in a separate integration binary
(`crates/cyrup-provider/tests/oauth_http_proxy.rs`), i.e. a separate OS process, deliberately, and
that file's module doc says so at `:8-30`. **The flake is real, though, and was reproduced: 6 of 25
lib-binary runs failed**, every time as `1117 passed; 1 failed` — the exact filed symptom — always
`utils::refresh::tests::a_lost_publish_race_does_not_start_a_second_fetch`
(`crates/cyrup-provider/src/utils/refresh.rs:227`), `left: 2, right: 1`. The cause is the TEST's
release timing, not process-global state and not a defect in `RefreshDedup`: the `Barrier` ordered
the two callers' ENTRY into the publish region, but the winner's fetch was released as soon as it
signalled "started", so the winner could complete and empty the memo while the loser was still
descheduled between `gate.wait()` and its `self.inflight.lock()` — at which point starting a second
fetch is *correct*, per pi's `finally`. Fixed by releasing only after both callers have reached their
first `Poll::Pending`, which is the one signal observable for the LOSER too.

**Six unguarded sites were found by matching every process-global writer against its READERS — which
is where all six gaps were — and all six are fixed.** The worst two are worth naming because neither
is a flake in the ordinary sense. **`crates/cyrup-ext-subagents/src/spawn/signal.rs` +
`spawn/mod.rs` failed 100% deterministically under a shell BACKGROUND job**, which is how an
overnight pipeline launches a suite: POSIX makes `SIG_IGN` survive `execve`, and a shell running a
non-interactive *asynchronous* command sets `SIGINT`/`SIGQUIT` to `SIG_IGN` for it (XCU 2.11), so the
test binary and every `sleep`/`sh` child inherited an ignored SIGINT and stage 1 of the escalation
ladder physically could not end the child. Measured on one binary at one commit: foreground
`2 passed, 0 failed, 0.00s`; backgrounded `0 passed, 2 failed, 30.01s` (`left: Sigterm, right:
Sigint`), burning the full injected grace twice. The repair is `ensure_sigint_reaches_children`
(`spawn/signal.rs:545`), which **probes first** and only registers a real handler when SIGINT is not
deliverable, so an interactive `cargo test` does not lose Ctrl-C. **`crates/cyrup-tui/src/tests/markdown.rs`
was a TOCTOU on `image::CAPABILITIES`**: the writer held `CAPS_LOCK`, the *reader* did not, and it
reads the global twice and asserts the two agree — a pin landing between them fails a correct
renderer. The remaining four: `crates/cyrup-provider/src/stream/sse.rs` held a 1 s
`HTTP_IDLE_TIMEOUT_MS` (`:56`) across a multi-second leg, which every `build_client` in the crate
reads; `crates/cyrup-tui/src/panic_hook.rs:75-76` reads and clears `PROGRESS_ARMED` under a
different mutex than `terminal_progress`'s tests used; and `crates/cyrup-tools/tests/shell_interpreter.rs`
carried a `SAFETY:` comment claiming it was the only test in its binary, untrue since a sibling
landed. The two new locks follow the existing in-repo idiom (`panic_hook::HOOK_LOCK`,
`cyrup_permission_system::ext_config::env_lock`) rather than inventing one:
`caps_lock()` (`crates/cyrup-tui/src/tests/image_capabilities.rs:236`) and `lock_progress_armed()`
(`crates/cyrup-tui/src/terminal_progress.rs:109-110`). **21 further global-state clusters were
audited and are already correct** — including all 384 `cyrup-it` env sites under `ENV_MUTATION_LOCK`,
which `cargo test --workspace` does not even build (`required-features = ["it"]`).

**Evidence: `-p cyrup-provider` went 6/25 FAILED → 30/30 ok; `-p cyrup-ext-subagents` backgrounded
went 2/2 FAILED (30.0 s) → 5/5 ok (~6 s); `--workspace --exclude cyrup-tui --no-fail-fast`
backgrounded went 2/2 FAILED → 5/5 with zero test failures; foreground `--workspace` final: 5 996
passed, 0 failed, 48 binaries.**

**TWO HAZARDS FOUND AND DELIBERATELY NOT FIXED — both block the merge gate, and neither is in the
de-flaking class.**

1. **`cyrup-session-svc` aborts its whole 311-test binary with SIGABRT, and when it fires 311
   passing tests are reported as ZERO.** **This is the single worst remaining pipeline hazard.** It
   is **not** a parallelism race: it reproduces in the foreground (3 of 10 runs, `rc=134`) and at
   `--test-threads=1` (2 of 3). The faulting frame is third-party, on a runtime thread of its own —
   `wasmtime::runtime::vm::sys::unix::machports::handler_thread` → `abort`, stderr
   `mach_msg failed with 268451845 (10004005)`, i.e. the macOS Mach exception-handler thread for wasm
   traps. Minimal reproducer (~0.09 s, roughly 2 in 6): the two tests
   `tests::round3::clone_at_creates_new_file_and_runtime_surfaces_fallback` and
   `tests::round3::drift029_abort_bash_cancels_every_in_flight_command` run together at
   `--test-threads=1`; either alone is 8/8 and 3/3 clean. **No lock this repo can add fixes it — it
   needs its own investigation.**
2. **`crates/cyrup-tui/tests/zzz_scratch_perf_probe.rs` does not compile at HEAD**, so
   `cargo test -p cyrup-tui` and `cargo test --workspace` cannot build. Its `impl Backend for
   CaptureBackend` (`:62`) is missing `scroll_region_up`/`scroll_region_down` after a ratatui bump —
   the sibling harness has both (`crates/cyrup-tui/src/tests/inline_stacking.rs:128`, `:132`).
   Pre-existing and unrelated, so it was left under the no-unrelated-fixes rule; it is a one-line
   fix and it is blocking the gate. Verification therefore ran as `-p cyrup-tui --lib --test
   experimental_marker --test share_viewer_url` plus `--workspace --exclude cyrup-tui`.

**One stale citation found while auditing and left in place** (documentation only, no behaviour):
`crates/cyrup-ext/src/caps/http.rs:1059` cites `cyrup-provider/src/tests/proxy_setting.rs`, which no
longer exists — it is `crates/cyrup-provider/tests/oauth_http_proxy.rs` — and the same comment's
claim at `:1070-1071` that it *"does not retrofit every one of this file's other `request`/
`request_stream` tests onto the guard"* is no longer true: 36 of 37 now hold it.

---

# RECONCILED 2026-08-14 (fourth edition) — the ninth pass was an ENUMERATION, not a sweep: nine finite pi surfaces walked end to end, 191 findings, 93 ids filed

> **This block changes how the rest of this file should be read, and that matters more than the
> numbers in it.** Everything below it is the third edition and is superseded on every count.
>
> **cyrup last commit `5990e86`** (`fix(tools): edit/read schemas emit no additionalProperties, matching pi`),
> branch `david/cyrup`. **The area files are ahead of that commit — every figure here is derived from
> the WORKING TREE**, because the four surface writers' filings are not committed yet. The third
> edition's `e5c6933` figures are still reproducible from git and are used as this edition's baseline.
>
> **SCOPE EXCLUSION, stated so the numbers stay interpretable: `13-cyrup-mcp.md`, `13a`–`13i` and
> `MCP-PORT-METHODOLOGY.md` are excluded from every count in this file.** *(Corrected 2026-09-04: the
> original text said they were "owned by another team"; that was false. The exclusion is structural — a
> forward-looking port plan cannot be summed with backward-looking defects — not an ownership boundary,
> and area 13 is schedulable work.)* They always have been — "the twelve `## Open items` tables" has meant areas 01–12 since the
> first edition — but it was never written down, and a reader who counts the directory gets a
> different answer. No figure in this edition, and no figure in `PARITY-GAPS.md` or `README.md`,
> includes an MCP row.
>
> **SECOND SCOPE GAP, FOUND 2026-08-19 — and unlike the MCP one it was never declared.**
> `crates/cyrup-flux` (9 source files, 1 513 lines; workspace member at `Cargo.toml:24`) has **no
> area file and no row anywhere**: `grep -rn 'cyrup-flux' docs/gap-analysis/` returns zero. It landed
> at `67b73a0`, with `spec/flux.md` + `FLUX_01`–`13` at `adb3bba` and a rustbook chapter at
> `0c0367d`. It is a **port**, not a `cyrup-original` — upstream is `code_puppy_core_plugins`'
> `flux_bootstrap` plugin (checkout at `/Users/davidmaple/cyrup.ai/code_puppy_core_plugins`, HEAD
> `8de5184`, latest tag **v0.0.6**), which makes it the **fifth ported upstream and the first that is
> neither pi nor TypeScript** (`pyproject.toml` — it is Python). House convention gives it its own
> numbered file: area 09 exists for `cyrup-ext-subagents` on exactly the basis that an
> extension-shaped crate gets an area, while area 06 is the extension HOST. **RESOLVED the same day —
> [`14-cyrup-flux.md`](14-cyrup-flux.md) was opened with 7 rows (4 medium, 3 low), and `README.md`'s
> Contents and Baselines tables both carry it.** It is recorded here rather than silently fixed
> because a shipped 1 513-line crate appearing in zero counts is precisely the failure the fourth
> edition's own "enumeration, not sweeps" lesson was written about, and the *mechanism* that hid it is
> still live. Same class, smaller: `CMDHINT_01` (`0b7c4f4` +
> `bae24f5`) is an id load-bearing in 13 shipped source locations with no row and no place in the
> `AREA-NNN` vocabulary, so **extend this file's own "grep the SOURCE for `AREA-NNN` citations at
> every reconciliation" rule to commit subjects**, which is what hides it.

## The open set — re-derived row by row from the twelve `## Open items` tables (areas 01–12), nothing carried forward

| | critical | high | medium | low | **counted open** |
|---|---:|---:|---:|---:|---:|
| first edition (after sweeps 1-2) | 0 | 3 | 75 | 95 | **173** |
| second edition (after sweeps 3-6) | 0 | 2 | 63 | 88 | **153** |
| third edition (after sweeps 7-8) | 0 | 2 | 61 | 82 | **145** |
| **this edition (after the surface enumeration)** | **0** | **5** | **88** | **144** | **237** |

**606 rows across the twelve tables. 360 carry a full closure marker and 36 more a partial one — 396
of 606 (65%).** The closure *rate* fell from 76% to 65% while eleven more rows closed: the
denominator grew by 103 rows in one pass. That is what an enumeration does to a backlog, and it is
not a regression.

**The counting rule is unchanged from the third edition and was validated rather than assumed.** Run
against the twelve area files *as they stand at commit `e5c6933`*, the script that produced the table
above reproduces the third edition exactly — **503 rows, 349 full closures, 35 partial, 8 in-table
`tracker` rows, 145 open = 0/2/61/82.** Every number in this edition therefore differs from the
third's because the *files* changed, not because the method did. *(Rule, restated: a row counts fully
closed if its ID is struck through **or** its severity cell says `CLOSED`/`FIXED` without
`PARTIALLY`; `PARTIALLY CLOSED`/`REOPENED` rows count **open** at their current severity — and where
a severity cell reads `~~medium~~ low`, the surviving severity is the one counted. `tracker` rows are
excluded: 8 in the area tables plus `SEAM-058` and `SUBA-005` in their areas' separate `## Trackers`
tables = 10, unchanged. `AGENT-S04` still carries `*(partially-closed)*` in place of a severity and is
in neither total — four editions now, and still left alone rather than silently rated.)*

### By area

| file | crit | high | medium | low | **open** | Δ vs third edition | rows |
|---|---:|---:|---:|---:|---:|---|---|
| `01-cyrup-core-and-provider.md` | 0 | **4** | 8 | 11 | **23** | **+12** (`PROV-054`…`PROV-067`; `PROV-061`/`PROV-062` filed+closed) | 43 → 57 |
| `02-cyrup-agent.md` | 0 | 0 | 0 | 2 | **2** | — | 31 |
| `03-cyrup-session.md` | 0 | 1 | 1 | 6 | **8** | — | 34 |
| `04-cyrup-tools.md` | 0 | 0 | 2 | 6 | **8** | **+3** (`TOOL-043`…`TOOL-045`) | 31 → 34 |
| `05-cyrup-config-and-resources.md` | 0 | 0 | 10 | 20 | **30** | **+21** (`CFG-054`…`CFG-076`; `CFG-056`/`CFG-057` filed+closed) | 40 → 63 |
| `06-cyrup-ext.md` | 0 | 0 | 18 | 17 | **35** | **+12** (`EXT-061`…`EXT-073`; `EXT-071` filed+closed) | 57 → 70 |
| `07-cyrup-tui.md` | 0 | 0 | 17 | 40 | **57** | **+22** (`TUI-063`…`TUI-087`; `TUI-069`/`070`/`074` filed+closed) | 71 → 96 |
| `08-cyrup-session-svc-and-modes.md` | 0 | 0 | 9 | 17 | **26** | **+20** (`SEAM-076`…`079`, `080`…`086`, `100`…`111`; three filed+closed) | 45 → 68 |
| `09-cyrup-ext-subagents.md` | 0 | 0 | 9 | 10 | **19** | **+2** (`SUBA-070`/`SUBA-071` — a documentation audit, **not** this enumeration) | 48 → 50 |
| `10-cyrup-permission-system.md` | 0 | 0 | 2 | 2 | **4** | — | 23 |
| `11-cyrup-intercom.md` | 0 | 0 | 5 | 4 | **9** | — | 46 |
| `12-upstream-drift-pi-core.md` | 0 | 0 | **2** | **1** | **3** *(was 16)* | **+1** (`DRIFT-052`, filed+closed) | 34 → 35 |
| | **0** | **5** | **88** | **144** | **237** | **+92** | **503 → 606** |

**CORRECTION 2026-08-15 — area 12 is no longer 16 open, and 8 of the 13 it shed were shed by
OTHER areas' work.** An area-12 pass at HEAD `68bbd39` re-derived all sixteen rows against pi
`v0.84.2` (the header's `v0.84.1` and pi-subagents `v0.47.1` were both stale; re-measure with
`git tag --sort=-v:refname`). Result: **8 REFUTED at HEAD** — `DRIFT-014`, `DRIFT-018`, `DRIFT-020`,
`DRIFT-025`, `DRIFT-027`, `DRIFT-028`, `DRIFT-030`, `DRIFT-031` — every one of them closed by
provider/resources work that landed after area 12 was last reconciled, and **not one closed by the
area-12 pass itself**; **2 CLOSED by the pass** (`DRIFT-042` half (a), `DRIFT-045`); **1 FILED AND
CLOSED** (`DRIFT-052`); **5 BLOCKED with measured sizes** (`DRIFT-004`, `DRIFT-009`, `DRIFT-015`,
`DRIFT-019`, `DRIFT-041`, `DRIFT-047` — six ids, five distinct blocks, since `DRIFT-019` is blocked
by `DRIFT-009`). Remaining open: 2 medium (`DRIFT-004`, `DRIFT-041`) + `DRIFT-009`/`DRIFT-015` (also
medium) and 2 low (`DRIFT-019`, `DRIFT-047`) — recounted in the area file's own table, which is
authoritative; the row above records the shape, not a re-derived severity census.

**The measured lesson, and it is a scheduling one.** Area 12 is 20-of-34 a duplicate index. Its rows
therefore go stale from work done in the OWNING area, and this pass measured a **50% false-positive
rate** on its open set — higher than the directory's published ~12%. A duplicate row is not evidence
of a live defect; re-check at HEAD before scheduling one. The corollary for the ledger: when an
owning area closes an item, the duplicate rows in area 12 should be closed in the same pass, or the
next planner schedules work that no longer exists.

**One methodological result worth generalising, from `DRIFT-014`.** That row was the file's stated
example of a Verify that "cannot be satisfied under the standing no-test-execution rule", because it
needed to know what string reqwest/hyper emits on a failed DNS lookup. It was settled by **reading
the dependency's source** — `ConnectError::dns` at
`hyper-util-0.1.20/src/client/legacy/connect/http.rs:668` — and the resulting `"dns error"` literal
is marked `[CYRUP-DELTA]` in `cyrup-provider/src/utils/retry.rs` with that citation. A claim about
what a Rust dependency emits is a source-reading question, not a test-execution question.

**Where the +92 came from, so none of it is unattributed.** **93 ids were filed by the four surface
writers; 11 of them closed on arrival** (`CFG-056`, `CFG-057`, `SEAM-104`, `TUI-069`, `TUI-070`,
`TUI-074`, `EXT-071`, `SEAM-085`, `SEAM-086`, `PROV-061`, `PROV-062`), leaving **82 open — 3 high,
24 medium, 55 low**. **Eight more rows were already in the working tree, unreconciled, before this
pass started** — `CFG-054`, `CFG-055`, `TUI-063`, `TUI-064`, `SEAM-076`…`SEAM-079` — all eight open;
the surface writers cite several of them as pre-existing ids, which is how they were caught.
**CORRECTION 2026-08-15: two of those eight were NOT open.** `CFG-054` and `CFG-055` were both
already fixed, with tests, at HEAD `68bbd39`, and are closed as REFUTED in
`05-cyrup-config-and-resources.md` — see their rows. The arithmetic above is left as written because
it counts what was *filed*, not what was open; this is a correction to the "all eight open" clause
only, and it is exactly the measured-error-rate phenomenon this file publishes. (The area-05 count
in the table above is stale for the same pass — `CFG-003` also closed — and is deliberately NOT
restated by one slice mid-batch; recount the table.) **Two
more, `SUBA-070` and `SUBA-071`, came from a documentation audit of area 09 and are not part of this
enumeration at all.** 82 + 8 + 2 = 92. **No ID was renumbered, merged or deleted; `SEAM-087`…`SEAM-099`
are deliberately unallocated** — two writers were minting SEAM ids concurrently and the second block
moved to `SEAM-100` rather than risk a collision. **Do not "recover" that range.**

~~**The above-medium set is no longer two rows. It is FIVE**, and three are new: `PROV-054` (xai
`grok-4.5` routed over the wrong wire API — and it is the xai *default* model), `PROV-055` (opencode's
`openai-nosession` affinity format missing on all 16 `openai-responses` rows, so cyrup leaks a
`session_id` header pi suppresses) and `PROV-056` (kimi-coding's `forceAdaptiveThinking` ×3 and
`allowEmptySignature` ×1 — two wire divergences per request on every model of the provider), joining
the unchanged `PROV-047` and `SESS-040`.~~ **STRUCK 2026-08-19 — ALL FIVE ARE CLOSED, and the set
that replaced them is tabled in the fifth-edition block at the top of this file.** `PROV-054`,
`PROV-055` and `PROV-056` closed 2026-08-15 (`01-cyrup-core-and-provider.md:293`-`:295`) through
exactly the bulk regeneration this paragraph predicted; `SESS-040` closed as REFUTED the same day;
and **`PROV-047` closed 2026-08-15** (`01-cyrup-core-and-provider.md`) — the one line that paragraph said the fix waited
on is `cyrup-session-svc/src/builder.rs:297`, `cyrup_provider::configure_http_proxy(proxy.clone())`
inside `apply_http_proxy_settings` (`:296-299`, reached from `:1516`), called **unconditionally,
including with `None`**, so clearing the setting clears the process-global; the second production
site is `crates/cyrup/src/main.rs:177`, deliberately ABOVE the package/config and credential-print
pre-dispatches, both of which can egress before a session exists. **The "inert until one line lands"
residual recorded at `:652` and `:836` below is DISCHARGED.** **All three new highs are catalog DATA
and share one fix site**: they close through `PROV-018`/`PROV-060`'s bulk regeneration, in the commit
that rewrites `catalog_manifest.json`. Scheduling them as three items produces three agents
hand-patching one row each and each invalidating the manifest — which is what did *not* happen, and
is why all three closed together.

## What this pass actually was, and the lesson — which is NOT that the analysis was blind

**Nine whole-backlog sweeps read the BACKLOG against the code and closed 384+ of ~503 rows.** This
pass did something different: it enumerated **nine finite pi surfaces mechanically and diffed both
directions**, and produced **191 findings — 67 missing in cyrup, 66 cyrup-original, 58 differing in
shape.** For scale, a late-stage sweep produces 10–25 items.

**pi's source was fully available the entire time** — the whole tree is checked out at
`/Users/davidmaple/cyrup.ai/pi` and every sweep could read it. Nothing was hidden. **The limit is
structural: an item-driven pass can only close what someone already wrote down.** Under that method
*"we have stopped finding things"* and *"there is nothing left"* produce identical evidence, and
nine sweeps' worth of falling yield was read as the second when it was the first.

**Enumeration removes the ambiguity.** When all 39 CLI flags are walked, all 73 keybinding ids, all
25 slash commands, all 7 built-in tools — **the diff IS the answer**, and the residual is stated as a
named gap rather than inferred from a quiet result. The two specimens worth remembering:

- **`cyrup update --models` does not exist** (`SEAM-100`) — verified by running the shipped binary,
  which prints `Unknown option --models for "update".` There is no CLI route to refresh model
  catalogs. **And the backlog already assumed it shipped**: `05-cyrup-config-and-resources.md:779`
  reasons about lock contention "against any concurrent `cyrup update --models`". *A gap that other
  analysis has already built on top of cannot be revealed by closing the items above it.*
- **The `edit` tool emitted `"additionalProperties": false` where pi emits none — and the test
  suite's own `PI_EDIT` "ground truth" constant carried the same keyword**, so the suite was
  certifying the divergence. Fixed and committed inline (`5990e86`) before the filing, and recorded
  in area 04 as precedent rather than backlog. **A ground-truth constant that was never mechanically
  re-derived is indistinguishable from an assertion of the current behaviour.**

**The catalog refutation belongs in the same list and is routed, not just filed.** `PROV-004` and
`PARITY-GAPS.md` §6 q5 both recorded as SETTLED that catalog accuracy is "not statically auditable".
**That premise is false.** pi's `*.models.ts` files are two-line re-exports only from `a9f6a3159`
onward; at its direct parent `b0c2a90e` — the revision cyrup's own manifest names as its provenance
floor — they are full data literals. `git log --oneline b0c2a90e..a9f6a3159` returns exactly one
commit. The whole catalog is checkable with `git show b0c2a90e:…` plus a ~12-line script, and that is
what made `PROV-054`…`PROV-059` measurable. Filed as `PROV-060`; the `PARITY-GAPS.md` entry is
corrected by this reconciliation.

## The re-run recipe — enumeration must be a RE-RUN, not a re-derivation

**This matters more here than anywhere else in the record, because this analysis's own measured error
rate is ≈12% and has not moved in six editions.** A finding that can only be reproduced by a human
walking a file again inherits that rate every time. A finding whose extraction is a command does not.
The commands below are recorded verbatim from
`scratchpad/surfaces.json`; upstream is always read out of the tag, never from a working tree.

| surface | extraction, as run |
|---|---|
| CLI flags | `git -C pi show v0.83.0:packages/coding-agent/src/cli/args.ts` (parseArgs + printHelp) · `git -C pi grep -n 'registerFlag' v0.83.0 -- packages/coding-agent/src/extensions/` (returns **zero** — pi's shipped default flag surface has no extension entries) · `git -C pi grep -nE 'argv\[2\]\|process\.argv' v0.83.0` |
| settings keys | `git -C pi grep -nE 'get(Global\|Project)Settings\(\)\s*[.\[]' v0.83.0` · `git -C pi grep -n 'applyOverrides' v0.83.0 -- 'packages/**/*.ts'` · `git -C pi grep -n mermaid v0.83.0 -- packages/coding-agent/src/core/settings-manager.ts` · cyrup side `grep -rn 'cli_settings(' crates/`, `grep -rn '\.packages()' crates/` |
| env vars | `git -C pi show v0.83.0:packages/coding-agent/src/cli/args.ts \| sed -n '342,390p' \| grep -oE '^  [A-Za-z_{][A-Za-z0-9_{}:<]*'` (the help block) · `git -C pi grep -n '<NAME>' v0.83.0 -- packages/` per name · cyrup side `grep -rn '"<NAME>"' crates --include='*.rs'` |
| keybinding ids | `git -C pi show v0.83.0:packages/tui/src/keybindings.ts` + `…coding-agent/src/core/keybindings.ts` · **independent check that settles the count:** ``git -C pi show v0.83.0:packages/coding-agent/docs/keybindings.md \| grep -c '^\| ` ' `` ⇒ **73** (the grep pattern is a pipe, a space, then a backtick — the table-row prefix of pi's shipped keybinding doc) |
| slash commands | `git -C pi show v0.83.0:packages/coding-agent/src/core/slash-commands.ts` + `modes/interactive/interactive-mode.ts` · cyrup side `grep -rn 'has_arg_completion' crates/ --include='*.rs'`, `grep -rn 'expand_slash_command' crates/ --include='*.rs'` |
| extension API | `grep -c '^\ton('` over `types.ts` @v0.83.0 ⇒ **33** overloads · `git -C pi grep -n 'startWorking\|stopWorking' v0.83.0` (⇒ empty; this is what exposed the fabricated `working-start`/`working-stop` citation) · every `types.ts:N` citation in both `world.wit` copies (109) and `ctx.rs` (57) resolved against the tag |
| built-in tools | `git -C pi grep -n additionalProperties v0.83.0 -- packages/coding-agent/src/core/tools/ packages/agent/src/harness/tools/` · `git -C pi grep -n 'truncation\.content' v0.83.0 -- packages/` · cyrup side `grep -rn 'impl Tool for'` |
| RPC protocol | `git -C pi grep -cn 'model_changed\|modelChanged' v0.83.0 -- packages/` (⇒ zero — the line is cyrup-invented) · `git -C pi grep -n toJsonEvent v0.83.0 -- packages/` (⇒ absent at v0.83.0, present at v0.84.1: this is how four citations were caught presenting v0.84.1 lines against v0.83.0 paths) |
| providers/catalogs | `git -C pi show b0c2a90e:packages/ai/src/providers/<name>.models.ts` for all 35, parsed and field-diffed against `crates/cyrup-provider/src/providers/catalog/*.json` · `git -C pi grep supportsFinishReason v0.83.0 -- packages/ai` · registration list read at v0.83.0 **and** at `91585d9a` side by side |

**One honest gap in the artifact itself: `surfaces.json` records no `extractionCommands` field.** Its
own prose refers to "the extractionCommands above", and surface 8's narrative describes a ~12-line
node script that parsed all 35 catalogs — **that script is not in the artifact, and the `surface`
strings are truncated at ~200 characters.** The table above is what could be recovered verbatim from
the finding bodies. **The catalog parser and the env-var cross-product recipe must be rewritten by
whoever re-runs those two surfaces.** Recording this is the point: the pass that argues enumeration
should be re-runnable did not fully make its own re-runnable, and the next one should emit the
commands as a first-class field.

## Which surfaces are COMPLETE, and exactly what the four incomplete ones could not reach

**Five of nine were enumerated completely on both sides.** For these, the diff is the answer and a
future pass re-runs rather than re-walks:

| surface | upstream | cyrup | findings |
|---|---:|---:|---:|
| CLI flags, subcommands, value enums, help text | 79 | 86 | 21 |
| settings.json keys — key, type, default, precedence | 66 | 67 | 10 |
| keybinding ids + default chords (**73**, not the ~40 the brief assumed) | 73 | 71 | 14 |
| slash commands + argument parsing + completion | 25 | 25 | 25 |
| built-in tool names, parameter schemas, result shapes | 7 | 7 | 4 |

**Four are INCOMPLETE and say so. Do not read coverage into them:**

1. **Environment variables (130 upstream / 233 cyrup — 62 findings).** Three named gaps. (a)
   **pi-mcp-adapter's env surface was extracted but NOT diffed** — `BROWSER`, `GLIMPSE_BINARY`,
   `MCP_DIRECT_TOOLS`, the five `MCP_HASH_*`, `MCP_OAUTH_DIR`, `MCP_OAUTH_CALLBACK_PORT`,
   `MCP_UI_DEBUG`, `MCP_UI_VIEWER`, `NPM_CONFIG_CACHE`, the five `PI_MCP_ADAPTER_*`,
   `PI_PACKAGE_DIR`; spot-checks show ten of those return zero hits in `crates/`. **Routed, not
   filed — that surface belongs to area 13's files.** *(Corrected 2026-09-04: this originally said
   "the MCP team's files, which this reconciliation may not touch"; there is no such team and area 13
   is schedulable work.)*
   (b) The **cyrup→pi-subagents direction is only partly walked**: ~110 `CYRUP_SUBAGENT_*` /
   `CYRUP_INTERCOM_*` names were not all walked back, so `CFG-074`'s nine confirmed cyrup-originals
   in that family **may not be all of them**. (c) ~110 of cyrup's 233 names come from the three
   sibling ports, so `cyrup-original` there was assigned against pi-subagents / pi-intercom /
   pi-permission-system, **not** against `pi/packages` — **anyone re-deriving must do the same or the
   cyrup-original count is fiction.**
2. **Extension API (134 / 182 — 28 findings).** Complete for pi's 33 `on(event:)` overloads (1:1,
   zero gaps both directions), all 28 `ExtensionUIContext` members, all 18 `ExtensionContext`, all 24
   non-`on` `ExtensionAPI` members, all 13 `ToolDefinition` fields, all 157 WIT functions, and every
   `types.ts:N` citation in both `world.wit` copies. **Not reached: the non-`types.ts` citations** —
   `agent-session.ts`, `tui.ts`, `event-bus.ts`, `exec.ts`, `agent/types.ts`, `project-trust.ts`,
   `tool-definition-wrapper.ts`, `agent-session-runtime.ts` were spot-checked, not exhaustively
   resolved, and `tui.ts:773-788` / `:784-786` remain **"plausible" — nobody has resolved them.**
   `EXT-072`'s ~40-site citation rewrite is filed, not landed.
3. **RPC protocol (68 / 72 — 9 findings).** Commands 32 v 32 (`diff` of the extracted lists is
   empty), event types 23 v 27, `extension_ui_request` methods 9 v 9, `RpcSessionState` 12 v 12,
   `RpcClient` surface complete with constants matching. **Not reached: the response DATA shapes
   behind each command** were verified only where a finding was already suspected —
   `bash`'s envelope and `get_commands`' entries are filed (`SEAM-083`, `SEAM-084`); the rest of the
   32 payloads were not walked field-by-field.
4. **Providers / wire APIs / compat flags (88 / 86 — 18 findings).** The compat-interface, wire-api-id
   and registration halves were read at v0.83.0 and carry no caveat. **The catalog half carries a
   provenance caveat that must travel with every claim derived from it:** pi gitignores
   `packages/ai/src/providers/data/`, so the 35 catalogs were read at **`b0c2a90e`, 13 days EARLIER
   than v0.83.0.** Every catalog claim in `PROV-054`…`PROV-061` is measured against `b0c2a90e`, **not**
   the ported baseline, and a clean refresh to `b0c2a90e` still leaves an unmeasurable 13-day
   residue. **Any future claim of "catalog parity at v0.83.0" is a claim about `b0c2a90e` plus an
   unbounded delta.** Also not reached: request-body fields beyond the compat matrix.

## `cyrup-original` — promoted to a first-class class, with its own count

**66 of the 191 findings are surfaces cyrup HAS and pi does NOT.** This project has tracked parity
gaps for four editions and has never tracked this class systematically — and it is the class through
which divergence enters *while everyone is looking at parity*. It gets its own count here rather than
being folded into the medium/low totals unexamined.

| | rows | open | medium | low |
|---|---:|---:|---:|---:|
| at the third edition (commit `e5c6933`) | 28 | **7** | — | — |
| **this edition** | **68** | **46** | **13** | **33** |

**31 of the 46 open rows were filed by this pass** (32 filed, `PROV-061` closed on arrival), and 8
more are the pre-existing working-tree rows. By area: 05 → 14, 08 → 11, 06 → 8, 01 → 5, 07 → 5,
04 → 1, 09 → 1, 11 → 1.

**These are not automatically defects. Every one must become KNOWN.** Rate them by **reachability**:
an advertised-but-dead surface outranks an internal helper, and a mechanism port forced by the
language is not divergence at all. The precedents that define the class:

- **`TUI-063` — `CYRUP_SHARE_VIEWER_URL` is advertised in `--help` and read by nothing.** The failure
  mode with a name: the binary documents a control that does not exist.
- **`EXT-021` — `working-start` / `working-stop` were documented in the WIT against a FABRICATED pi
  citation.** `git -C pi grep -n 'startWorking|stopWorking' v0.83.0` is empty. This pass found the
  same class twice more: a spliced `ctx.abort()` doc quote (fixed, `EXT-073`) and nine citations
  naming a line band that exists in **no** version (open). **Three fabrications in one area file is a
  test-infrastructure gap, not three mistakes** — both `EXT-072` and `EXT-073` specify the same
  guard (resolve every `types.ts:N` citation against the checked-out tag); **land the guard, not just
  the rewrite.**
- **`EXT-071` — `get-all-tools`' shape comment advertised a `source` field on `ToolInfo` that
  `EXT-060` had already REMOVED from the emitted object.** An advertised surface nothing produces, in
  the guest's only contract. Fixed this pass.
- **`SEAM-080` — `model_changed` is a cyrup-invented line on the RPC stdout stream, and two backlog
  items already reason about it as upstream.** Same shape as `SEAM-100`: invented surface that other
  analysis has already built on.
- **`CFG-068` — `CYRUP_HOME` is invented, live in shipped builds, and outranks `$HOME` at four sites
  at once.**

**Two scheduling hazards inside this class, recorded because they outlive the pass.** (1)
**`SEAM-047`'s Verify line would pin a cyrup-invented wire line (`session_shutdown` on stdout) as
required behaviour** — flagged LOAD-BEARING in `SEAM-081`'s body, but nothing forces someone to read
it before landing `SEAM-047`. (2) A catalog regeneration from `b0c2a90e` will **re-introduce** groq
`qwen/qwen3-32b`'s `thinkingLevelMap` and turn `PROV-064`'s guard test red; the generator needs a
named exception list or that deliberate delta is silently reverted.

**And one anti-pattern the writers got right, worth copying:** several `cyrup-original` findings were
examined and given **no id on purpose**, with the reasoning recorded — the 16 closure-to-export
inversions and the 22 authority imports are *forced* by the Component Model (a WASM guest cannot hand
back a function), `NO_PROXY`'s absence is a false positive of a literal grep because both sides
case-fold the key at runtime, and `supportsStrictMode`'s three different upstream defaults were
checked individually and are all correct in cyrup. **Verified non-divergence is a result. Write it
down or the next pass re-derives it.**

## What this edition does NOT change

The third edition's reasoning is intact and is still what this file rests on: the two-failure-modes
analysis of "refuted", the **dropped-delegation class** (three instances, the third a live behaviour
defect), the JS→Rust mechanism register, the orchestration rules, and the ≈12% measured error rate.
**Nothing in this pass was executed** — the standing test rules permit only
`cargo check -p <crate> --all-targets`, so all eleven fixes are type-checked arguments, not
observations, and the three TUI fixes additionally want a live terminal run per this workspace's
standing rule. Read those sections below as current; read only the *counts* below as superseded.

---

# RECONCILED 2026-08-14 (third edition) — EIGHT sweeps applied, every count re-derived from the twelve area tables

> **cyrup HEAD `e5c6933`** (docs HEAD `0097149`), branch `david/cyrup`. **Both gates green:**
> `cargo nextest run --workspace` = **6740 tests, 6740 passed, 17.9 s**;
> `cargo nextest run -p cyrup-it --features it` = **473 tests, 473 passed, 92 s**.
>
> **Everything below this block is the previous edition and is superseded on every number.** It is
> retained because its *reasoning* — the two-failure-modes-of-"refuted" analysis, the orchestration
> finding, the JS→Rust register — is still the argument this file rests on.

## The open set — re-derived row by row from the twelve `## Open items` tables, nothing carried forward

| | critical | high | medium | low | **counted open** |
|---|---:|---:|---:|---:|---:|
| first edition (after sweeps 1-2) | 0 | 3 | 75 | 95 | **173** |
| second edition (after sweeps 3-6) | 0 | 2 | 63 | 88 | **153** |
| **this edition (after sweeps 7-8)** | **0** | **2** | **61** | **82** | **145** |

**503 rows across the twelve tables. 349 carry a full closure marker and 35 more a partial one — 384
of 503 (76%).** *(Method, stated so it is reproducible and so the drift from the second edition's
"338 of 500" is visible rather than mysterious: a row counts as fully closed if its ID is struck
through **or** its severity cell says `CLOSED`/`FIXED` without `PARTIALLY`; a `PARTIALLY CLOSED` /
`REOPENED` row is still counted **open** and carries its severity into the arithmetic above. Ten
`tracker` rows are excluded from every figure — eight in the area tables plus `SEAM-058` and
`SUBA-005` in their areas' separate `## Trackers` tables. One row, `AGENT-S04`, carries
`*(partially-closed)*` in place of a severity and is in neither total; that has been true for three
editions and is left alone rather than silently rated.)*

**THREE rows are new since the second edition, and all three are closed** — `PROV-M01` (area 01) and
`TOOL-M01` (area 04), both filed and closed in the same pass; and `EXT-M03` (area 06), **filed
retroactively by this reconciliation because the ID was cited FIVE times in
`crates/cyrup-ext/src/host/live.rs` and had no row in any area file.** The work landed in sweep 6;
the bookkeeping never did. An ID citable from source but absent from the backlog is the orphan
condition `README.md` blind spot 4 names as *more dangerous than no entry at all* — and it is a
reason to grep the SOURCE for `AREA-NNN` citations at every reconciliation, not just the docs. Both came out of one assigned audit rather than from
the backlog, which is the point: see "The dropped-delegation class" below. **One row was REOPENED:
`TOOL-042`, by measurement.** **No ID was renumbered, merged or deleted.**

### By area

| file | crit | high | medium | low | **open** | Δ vs second edition |
|---|---:|---:|---:|---:|---:|---|
| `01-cyrup-core-and-provider.md` | 0 | 1 | 4 | 6 | **11** | — (`PROV-M01` filed+closed) |
| `02-cyrup-agent.md` | 0 | 0 | 0 | 2 | **2** | — |
| `03-cyrup-session.md` | 0 | 1 | 1 | 6 | **8** | — |
| `04-cyrup-tools.md` | 0 | 0 | 2 | 3 | **5** | — net (`TOOL-024` closed, `TOOL-042` **reopened**, `TOOL-M01` filed+closed) |
| `05-cyrup-config-and-resources.md` | 0 | 0 | 4 | 5 | **9** | **−3** (`CFG-045`, `CFG-051`, `CFG-052`) |
| `06-cyrup-ext.md` | 0 | 0 | 11 | 12 | **23** | **−1** (`EXT-060`; `EXT-M03` row filed retroactively, closed) |
| `07-cyrup-tui.md` | 0 | 0 | 14 | 21 | **35** | — |
| `08-cyrup-session-svc-and-modes.md` | 0 | 0 | 3 | 3 | **6** | **−1** (`SEAM-017`) |
| `09-cyrup-ext-subagents.md` | 0 | 0 | 8 | 9 | **17** | **−3** (`SUBA-008`, `SUBA-030`, `SUBA-035`) |
| `10-cyrup-permission-system.md` | 0 | 0 | 2 | 2 | **4** | — |
| `11-cyrup-intercom.md` | 0 | 0 | 5 | 4 | **9** | — |
| `12-upstream-drift-pi-core.md` | 0 | 0 | 7 | 9 | **16** | — (no sweep since 3 has owned it) |
| | **0** | **2** | **61** | **82** | **145** | **−8** |

**The two highs are unchanged — `SESS-040` and `PROV-047`.** Areas 08, 09 and 10 still have zero open
criticals and zero open highs between them, which is why sweep 8's tail-a agent correctly reported
that its *entire* remaining set was medium/low and then landed the top-ranked medium end to end.

## What eight sweeps produced, honestly

| outcome | count | what it means |
|---|---:|---|
| **closed** | ~424 of 448 originally-worked rows, +8 this edition | the fix landed and was verified at HEAD |
| **refuted-not-fixed** | **≈56 across all eight sweeps** | the row was wrong about the code or about upstream, **or** the fix had landed and no writer had reconciled it |
| **already-done** | 6 this edition alone | verified in place; a first-class outcome, not a shortfall |
| **reopened** | **1** (`TOOL-042`) | a closure was refuted by *measurement* |
| **blocked / not-taken** | 13 this edition | needs an owner decision, a cross-crate seam, or a live provider call |

**The measured error rate is unchanged at ≈12%** (≈56 refutations against ~465 rows worked across
eight sweeps). **It has not improved in six editions, and the honest reading is that this is the
method's floor, not a defect to be driven out** — refuting is how a static analysis corrects itself.
What *did* change is which failure the word "refuted" names, and the second edition's split still
holds: the majority is **doc staleness** (the fix landed; nobody reconciled), the minority is
**genuine analysis error**. Sweep 8 produced clean instances of each. Doc staleness: `SEAM-017` read
*"sweep 2 — not started"* while `crates/cyrup-modes/src/rpc_client.rs` was **1262 lines** at HEAD;
`CFG-045` read *"sweeps 2 and 6 — unchanged at HEAD"* while both branches it called missing were in
`app/input.rs:147-210` (pi's four mutually-exclusive `defaultEditor.onEscape` arms, with the
`doubleEscapeAction` window at `:191-210`). Genuine analysis error: **`CFG-052`'s entire premise about upstream is false** —
pi's `parseGitUrl` returns `null` before reaching `hostedGitInfo.fromUrl` unless there is a `git:`
prefix or an explicit `://`, and its own doc comment says so (`utils/git.ts:165-179` @v0.83.0), so
the "internally inconsistent state" the row filed is **upstream's, faithfully ported**.

**And this edition adds a third failure mode, which is the sharpest thing in it: a closure validated
against the wrong signal.** `TOOL-042` was closed on a static fd-inheritance argument plus two pins.
The argument was correct and the pins closed a real class — the LEAK rate fell from ~12% to ~1.0%.
**But the failure did not stop**, and the one occurrence that was instrumented cannot be explained by
inherited handles at all: it names a test driving an **in-memory** `RecordingProc` whose only possible
child names all three stdio handles and is reaped. **A tripwire's RED was read as naming its own
cause.** Sweep 6's own caveat set the falsification condition — *"If a LEAK still appears the
fd-inheritance theory is wrong"* — and sweep 8 ran the 286 runs and met it. **That caveat is why this
row could be reopened at all. Write the falsification condition into every closure that rests on an
argument rather than an observation.**

## The dropped-delegation class — now three instances, and the invariant is wider than anyone wrote

**This is the highest-value finding of sweep 8 and it produced a live behaviour defect.** pi composes
by **object spread** — `return { ...provider, getModels, refreshModels }`
(`core/remote-catalog-provider.ts:52-54` @v0.83.0) — so every member of the interface survives **by
construction**. Rust has no spread, so a hand-written delegating impl silently drops any method it
forgets, **and the drop is invisible precisely because the trait default is a plausible answer**:
`name`→id, `base_url`/`headers`→`None`, `filter_models`→identity, `render_kind`→`Default`,
`constrained_sampling`→`None`, `read_stream`→a `Cursor` over the whole file,
`detect_image_mime`→extension-based.

| # | site | trait | found by | consequence |
|---|---|---|---|---|
| 1 | `RegisteredTool` (`cyrup-ext/src/wrapper.rs`) | `Tool` | `TOOL-024` | 9 of 11 assertions vacuous |
| 2 | `WasmTool` (`cyrup-ext/src/host/live.rs`) | `Tool` | `EXT-M03` *(row filed retroactively this edition — it was cited in source and existed nowhere in the backlog)* | a guest's declared `label` crossed the whole ABI **write-only**; a native tool could express one, a WASM guest could not |
| 3 | **`RemoteCatalogProvider` + `ConfigProvider`** (`cyrup-provider`) | **`Provider`** — the first NON-`Tool` trait | **`PROV-M01`, sweep 8** | **LIVE:** `github-copilot` is the one built-in installing a `filter_models`, and `all_providers_with_overlay` maps every built-in through `CatalogOverlay::apply` — so `Models::get_available` got the identity default and **offered all 29 Copilot models regardless of what the OAuth credential entitled.** Proven by running the new test against the pre-fix code: 29 ids, not 1. |

**The invariant is not "audit `Tool` impls".** It is: **every hand-written same-trait decorator, every
defaulted method, and a fixture value that CONTRADICTS the default — ideally in both directions.**
`TOOL-M01` is that rule applied: both `FsOps` decorators forwarded `detect_image_mime` correctly, but
nothing could see it, so the probe now reports `Some(Png)` for a `.txt` path *and* `None` for a
`.png` path.

**Sweep 8 also published the complete enumeration of same-trait decorators in areas 01-07 and 11-12,
so the next sweep does not re-derive it:** `RegisteredTool`/`Tool` (13/13, pinned);
`WasmTool`/`Tool` (complete, source-level-pinned at `host/live.rs:2323`); `ProtectedFs`/`FsOps` and
`TraversalFs`/`FsOps` (8/8, both defaults now pinned); `OverlayEnvContext`/`AuthContext` (the trait
has 2 methods and **no defaults**, so it is structurally immune); `RemoteCatalogProvider` and
`ConfigProvider`/`Provider` (fixed this pass). **Checked and dismissed as not decorators despite
matching the `impl Trait for Wrapper` grep:** `RecordingServices`/`HostServices` (records into its own
state, no inner), `DeferredSandbox`/`OsSandbox` (no inner), `NativeHandle`/`Extension` (adapts a
*different* trait, so the compiler catches omissions).

## JS→Rust mechanism gaps found in sweeps 7-8 — nine more, and one of them was in a BRIEF

These are appended to the register further down this file, which now stands at its original entries
plus these.

1. **A budget the child is told about and nobody enforces.** `SUBA-008`'s own `Fix` line instructed
   *"port as `exec/turn_budget.rs` mirroring `exec/tool_budget.rs`'s env-handoff shape"*. The tool
   budget is env-var + **child-side refusal** (`PI_SUBAGENT_TOOL_BUDGET`). **The turn budget is the
   opposite shape** — `git grep -n TURN_BUDGET v0.43.0 -- src/` matches only `turn-budget.ts` itself;
   there is no env var and no child-side enforcement. The child is only **told**, via a system-prompt
   block; the **supervisor** enforces by counting assistant `message_end` events off the child's
   NDJSON stdout (`foreground/execution.ts:910-924`). **A faithful-looking env-shaped port would have
   advertised a budget nothing enforced.**
2. **`setTimeout` + keep reading vs. a `terminate` that CONSUMES the child.** pi arms two timers
   inside `requestTurnBudgetAbort` and keeps reading stdout during the window, so a child that wraps
   up inside it still delivers output. cyrup's `SpawnedChild::terminate` takes the child; there is no
   signal-without-taking seam. Recorded as an explicit `[CYRUP-DELTA]` at the call site rather than
   silently matching the observable outcome, **because a late final message is dropped here where
   upstream would have read it**. The graces reproduce upstream's **absolute instants** — SIGINT,
   SIGTERM at +1 s, SIGKILL at +4 s — since pi arms both timers from the same moment: **the real
   SIGTERM→SIGKILL gap is 3 s, not 4 s, and reading `execution.ts:752` alone gives the wrong number.**
3. **A JS DEFAULTED parameter fed its own previous value.** `terminationDeferredAtTurn = turnCount`
   (`execution.ts:773-777`). A naive Rust port taking `u64` renumbers the deferral point on every
   later deferral; the port takes `Option<u64>` + `unwrap_or(turn_count)`, and a test pins that a
   second deferral still names the FIRST turn.
4. **Two fields that look like synonyms and differ exactly when the feature matters.**
   `wrapUpRequestedAtTurn` is upstream's literal `budget.maxTurns` (the THRESHOLD); `exceededAtTurn`
   is the OBSERVED `turnCount`. Equal in the common case, different whenever a grace turn is
   configured — **precisely the case the feature exists for.** Both tests assert they differ (2 vs 3).
5. **A stale reassurance in a comment is a defect the moment the thing it reassures about changes.**
   `tui/intercom.rs:348-352` justified a hard-coded `false` with two claims, and the turn-budget
   landing falsified **both**: "a turn-budget stop is not signal-killed in this port" (it is now) and
   "`false` can only WIDEN `isUnexplainedProcessSignal`, never narrow it" (**widening is the bug** — a
   deliberate budget kill has `process_signal: Some` and a non-zero exit, so `false` routed it down
   the unexplained-signal branch and reported `stopped` rather than a budget abort).
6. **A correct consumer starved of a value is a different and quieter failure than a consumer reading
   a constant.** `is_retryable_subagent_startup_failure` was **already** reading
   `evidence.turn_budget_exceeded` correctly (`exec/fallback.rs:915`); it just had no producer — so
   the ladder would have **relaunched the very model that blew its turn budget.** The item counted it
   as one of three consumers "reading a hard-coded `false`". It was not.
7. **`Entry::Warning` renders verbatim, so `Warning: ` is a per-caller obligation** (`TUI-062`).
   Demonstrated concretely this pass: an injected double prefix left the **producer's** own unit test
   green and **only the rendered assertion caught it.** A string-level test on the producer side
   cannot see a renderer that re-prefixes, truncates or drops the line.
8. **Assert presence before absence — twice, and both would have been vacuous tests.** (a) The first
   draft of the turn-budget system-prompt test asserted on the **argv**; `SUBA-030` had moved the
   persona off the command line into a spilled `0600` file, so it would have passed whatever the file
   contained. (b) The existing `message_end_line` fixture helper sets `stopReason: "stop"`, which
   makes every turn a **terminal** assistant stop — and `turnBudgetDecision` returns `continue` for a
   terminal stop **however far past the hard limit** (`turn-budget.ts:94`). **An enforcement test
   built on that helper can never abort.** A separate `working_message_end_line` with
   `stopReason: "toolUse"` was added, and its doc comment says why.
9. **A live-fixture null run reads as a product bug.** Planting `hooks/` at `$HOME/.cyrup/hooks`
   collects zero warnings because the agent dir is `$HOME/.cyrup/agent`
   (`cyrup-config/src/env.rs:178`). The `CFG-049` agent's first run **looked exactly like "the
   keypress gate was dropped"**. **Any live row whose fixture plants files under a resolved directory
   must assert PRESENCE — that the effect fires — before it is allowed to report ABSENCE.**

## ORCHESTRATION — what sweeps 7-8 confirmed, and the one new rule

**The second edition's finding held and should be treated as settled: per-crate partitioning stalled
at sweep 4 (15 items); repartitioning by FEATURE unblocked it (sweep 5 landed 5 of 5, sweep 6 ~15).**
Sweep 8 kept the feature shape and it worked again — its tail-a agent held five crates and landed
`SUBA-008` end to end including two `cyrup-it` tests, which no per-crate agent could have done
because the change touches `cyrup-ext-subagents` *and* 15 `cyrup-it` fixture files.

**THE NEW RULE, and it cost this edition a correction of its own: TREAT THE BRIEF AS A LEAD.** Two of
sweep 8's three agents found a load-bearing error **in their own assignment text**, not in the code:

- `SUBA-008`'s `Fix` line prescribed the wrong mechanism (register entry 1 above). An agent following
  it faithfully ships a non-functioning feature and reports success.
- **A pi citation in the orchestrator's own brief was FABRICATED**, and an agent caught it by opening
  the file. That is the same class as the three fabricated citations the second edition recorded
  inside items — **it is not confined to the area files.**
- Sweep 6 had already refuted `PROV-030` rather than rewriting it, on the ground that the area file
  said it was done and the code agreed.

**So: no brief, no item and no ledger line — including this one — is evidence until the file is
open.** `already-done` is a first-class outcome and must be reported with the evidence that closed it.

**Three fix sites in the area files are recorded WRONG in the same way, and each would have produced
a no-op or a half-landed fix.** All three name `crates/cyrup-tui` (± `cyrup-core`) and all three need
a **producer in `crates/cyrup-session-svc`** that does not exist:

| row | what the TUI cannot reach | what must be added first |
|---|---|---|
| `EXT-013` (*"NOW A ONE-LINER"*) | `has_arg_completion` at `cyrup-tui/src/commands.rs:358` | `AgentSession::slash_command_catalog()` emits only name/description/source/sourceInfo (`session.rs:2504-2514`) — **no arg-completion signal arrives** |
| `TOOL-022` / `TOOL-015` / `EXT-024` | any branch on `render_kind` | **there is no tool-metadata channel at all** — `ToolRun` (`transcript.rs:701-720`) holds only `name: String`; no `tool_info`/`tool_catalog`/`set_tools` accessor exists on `App` |
| ~~`PROV-036`~~ **CLOSED — producer landed** | ~~the per-model cost breakdown~~ | ~~the consumer is `SessionStats::from_entries` in `cyrup-session-svc/src/state.rs`~~ — **the channel exists at HEAD (`4fb5e40`): `usage_cost_breakdown` (`cyrup-session-svc/src/state.rs:187`, exposed as `AgentSession::usage_cost_breakdown` at `session.rs:4029`) is awaited by `C::SessionInfo` at `app/execute_session.rs:155` and rendered under pi's own `usageBreakdown.length > 1` gate at `:184-191`, with `Cache Re-billed` at `:195-210` under pi's `stats.cost > 0 \|\| cacheWaste.missedTokens > 0` guard (`:181`). This row's premise — "the TUI cannot reach it" — is refuted.** |

**`SEAM-020` is a fourth of the same shape but fails differently: its recorded residual — "one line at
`main.rs:215`" — is a TYPE ERROR.** `render_help` takes cyrup's *parsed argv* `ExtensionFlag`; pi's
`printHelp` takes flag **declarations** carrying `type`/`description`/`extensionPath`
(`cli/args.ts:212-222` @v0.83.0). And the renderer is already lossy: `cli.rs:900-910` drops the whole
description column that upstream emits via `.padEnd(30)`. **Three parts, two crates, and a live
terminal.**

**Route sweep 9 by FIX SITE, and the specific pairings that must go to one agent:**
`SEAM-015` **+** `DRIFT-004` (both need the same `BashOperations` trait in `crates/cyrup-tools`);
`SEAM-S03` (the `cyrup-tools` half only); `EXT-013` **+** `TOOL-022`/`TOOL-015`/`EXT-024` **+**
`PROV-036` (all three need `cyrup-session-svc` + `cyrup-tui`, so one owner closes five rows);
`SEAM-048` → `cyrup-ext/src/facade.rs`; `SEAM-073` half (a) → `cyrup-config/src/lock.rs`, half (b)
**must be re-scoped away from `cyrup-session-svc` before anyone hunts there.**

**Six rows are NOT agent work and must stop being scheduled as such.** `SEAM-057` (declined by three
sweeps — deleting `--json`/`--rpc`/`--output-format` breaks `cyrup-it`'s `--rpc` fixture);
`SUBA-025` (declined by two — requires **authoring model-facing text**, because upstream's
FULL/COMPACT/SAFETY constants are written around `workflowScript`, which cyrup deliberately does not
implement); `SUBA-055`'s guide action (same hazard — requires authoring a `resources/docs/` set; and
**`SUBA-066` is its last mile, not an independent item**); `SUBA-054`'s async half (needs a decision
on which cwd an async single step's read instruction resolves against, or it double-emits);
`PERM-032` (needs a live provider call this environment cannot safely make —
`TOGETHER_API_KEY`/`TOGETHER_AI_API_KEY` are exported here — **and its request-body diff must be
re-baselined against the current block shape, since `HookOutcome::Block` gained a `terminate` field
via `EXT-049` after it was filed**); and `CFG-049`'s `MIGRATION_GUIDE_URL`/`EXTENSIONS_DOC_URL`
rebrand, which still points cyrup users at `github.com/earendil-works/pi-mono` and is **visible
verbatim in a live transcript**.

**Two operational hazards recorded because they will recur.** (a) **Ambient credentials must be
scrubbed when spawning the binary.** `TOGETHER_API_KEY`, `TOGETHER_AI_API_KEY`, `CYRUP_INTERCOM`,
`CYRUP_SUBAGENTS`, `CYRUP_PERMISSION_SYSTEM` and `GITHUB_TOKEN` are exported on this machine; the
integration suite has **guard tests that FAIL when they leak in**, and seven "failures" in one run
were those guards doing their job. (b) **A concurrent agent's red crate blocks `cargo clippy` for
everyone downstream.** Sweep 8's tail-a agent could not lint its own crate because
`crates/cyrup-provider` was mid-edit and clippy-RED, which aborts the dependency graph before
`cyrup-ext-subagents` is linted — `--keep-going` does not get past it. `cargo check --all-targets`
was clean and both nextest gates green, but **the clippy gate on that change is UNVERIFIED and should
be re-run centrally.**

## Test architecture and the gates — current numbers

- **310 integration binaries → 6 + 8 gated**, behind the `cyrup-it` harness crate (`63d729a` /
  `c3982b5` / `d973906`). **Every `crates/<crate>/tests/<x>.rs` citation in this directory is stale
  unless it names `cyrup-it`.**
- **Unit gate: `cargo nextest run --workspace` = 6740 tests in 17.9 s** (was 6699 in 16.3 s at the
  second edition; 6440, 6387 and 3932 are older still).
- **Integration gate: `cargo nextest run -p cyrup-it --features it` = 473 tests in 92 s.** This is
  new information for `ICOM-053`: the harness **does** run, as a second gate. What `ICOM-053` still
  names is that the 17.9 s merge gate does not invoke it, so a breakage inside `cyrup-it` lands
  silently — which is why `EXT-025` still cannot be closed by deleting its dead methods without one
  agent owning `cyrup-ext` + `cyrup-session-svc` + `cyrup-it` in a single commit.
- **The integration suite carries guard tests that FAIL when ambient credentials leak in.** Scrub the
  environment before running it; the suite is 473/473 once scrubbed.
- **`.config/nextest.toml`'s 500 ms `leak-timeout` was deliberately NOT raised** while investigating
  `TOOL-042`. Raising it would hide the signal and is the user's call.

---

# RECONCILED 2026-08-14 (second edition) — six sweeps applied, every count re-derived from the twelve area tables

> **cyrup HEAD `bdcb0d0`** (was `380c713` when the first edition below was written), branch
> `david/cyrup`, tree clean. Gate: **`cargo nextest run --workspace` = 6699 tests, 6699 passed,
> 7 skipped, 16.3 s**; `cargo check --workspace --all-targets` clean.
>
> **Everything below this block — including the `RECONCILED 2026-08-14 — two parity sweeps applied`
> header that follows it — is the previous edition and is superseded on every number.** It is
> retained unedited because its *reasoning* is still the argument this file rests on. Only the
> arithmetic is dead. Where a sentence in it is wrong about the CODE or about UPSTREAM (not merely
> out of date), it has been struck in place; those strikes are listed under
> "Corrections to the previous edition" below.

## What happened: sweeps 3, 4, 5 and 6

The first edition reconciled sweeps 1-2. **Four more sweeps ran before any doc writer did**, and that
delay is itself the largest finding of this edition — see "Why sweep 6 'refuted' 39 rows" below.

| sweep | shape | landed | note |
|---|---|---|---|
| 1 | per-crate, 11 agents | 232 items | 184 `docUpdatesNeeded`; reconciled in the first edition |
| 2 | per-crate | (see first edition) | reconciled in the first edition |
| 3 | per-crate | — | **never reconciled into these files** |
| 4 | per-crate | **15 items only** | the stall; see the orchestration finding |
| 5 | **by FEATURE** | **5 of 5 assigned, incl. `SUBA-S01`** | the unblock |
| 6 | **by FEATURE**, 5 agents | **~15 landed, ~39 rows refuted** | reconciled here |

## The open set — derived from the twelve `## Open items` tables, nothing carried forward

| | critical | high | medium | low | **counted open** |
|---|---:|---:|---:|---:|---:|
| first edition (after sweeps 1-2) | 0 | 3 | 75 | 95 | **173** |
| **this edition (after sweeps 3-6)** | **0** | **2** | **63** | **88** | **153** |

**338 of 500 rows across the twelve tables now carry a closure marker** (was 311 of 492). **Ten
`tracker` rows are excluded from every figure**, as always — eight in the area tables (`PERM-017`
was re-classified this edition; its own body says "No action while the middle levels remain
meaningless", which is the Trackers contract) plus `SEAM-058` and `SUBA-005` in their areas'
separate `## Trackers` tables.

**Eight rows are new since the first edition, and FOUR of them were filed AND closed in the same
pass** — which is what a *hunting* sweep produces when it finds a defect the backlog never named:
~~`TOOL-042` (the nextest `LEAK`, root-caused, area 04)~~ — **CORRECTED (third edition): `TOOL-042` was filed, LARGELY fixed, and its residual REOPENED by measurement in sweep 8. It is not one of the four same-pass wins; three of the four are.** — `EXT-M01` and `EXT-M02` (two JS→Rust mechanism
gaps, area 06) and `PERM-033` (16 unported forwarding audit sites, area 10). One more was filed and
**partially** closed: `TUI-062` (area 07, still counted open). Three were filed open: `CFG-052`
(`github:` shorthand), `CFG-053` (test debt re-filed out of `CFG-006`) and `ICOM-053` (re-filed out
of `ICOM-026`'s structural half).

### By area

| file | crit | high | medium | low | **open** | Δ vs first edition |
|---|---:|---:|---:|---:|---:|---|
| `01-cyrup-core-and-provider.md` | 0 | 1 | 4 | 6 | **11** | −1 (`PROV-011`) |
| `02-cyrup-agent.md` | 0 | 0 | 0 | 2 | **2** | — |
| `03-cyrup-session.md` | 0 | 1 | 1 | 6 | **8** | — |
| `04-cyrup-tools.md` | 0 | 0 | 1 | 4 | **5** | −1 (`TOOL-016`; `TOOL-042` filed+closed) |
| `05-cyrup-config-and-resources.md` | 0 | 0 | 5 | 7 | **12** | −6 (8 closed, 2 filed) |
| `06-cyrup-ext.md` | 0 | 0 | 11 | 13 | **24** | — (`EXT-M01`/`EXT-M02` filed+closed) |
| `07-cyrup-tui.md` | 0 | 0 | 14 | 21 | **35** | +1 (`TUI-062` filed) |
| `08-cyrup-session-svc-and-modes.md` | 0 | 0 | 3 | 4 | **7** | −1 (`SEAM-061`) |
| `09-cyrup-ext-subagents.md` | 0 | 0 | 10 | 10 | **20** | −6 |
| `10-cyrup-permission-system.md` | 0 | 0 | 2 | 2 | **4** | −1 (`PERM-017` → tracker; `PERM-033` filed+closed) |
| `11-cyrup-intercom.md` | 0 | 0 | 5 | 4 | **9** | −5 |
| `12-upstream-drift-pi-core.md` | 0 | 0 | 7 | 9 | **16** | — (no sweep-6 agent owned it) |
| | **0** | **2** | **63** | **88** | **153** | **−20** |

**The two remaining highs — the whole actionable high set:**

| # | ID | area | why it is still the top of the backlog |
|---|---|---|---|
| 1 | **`SESS-040`** | 03 + 07 + 08 | A shipped control that bills tokens and rewrites the session file still has no dispatch site, and the UI advertises it. Its two siblings (`SESS-041`, `SESS-042`) are closed, so **the moment `SESS-040` lands a caller the abort actually takes effect.**  **STRUCK 2026-08-15 (batch B, area 03): CLOSED as REFUTED — the Escape dispatch landed in `380c713`.** `rg -n AbortCompaction crates/` returns a production dispatch at `cyrup-tui/src/app/input.rs:144-146` (an `Action::Interrupt` branch on `state.compacting`, ahead of the four-branch chain and behind the `branch_summary_in_flight` check) routed to `ctx.session.abort_compaction()` at `app/run_action.rs:53-54` (`session.rs:1900`), plus assertions at `cyrup-tui/src/tests/escape_chain.rs:233,244`; the band half (`TUI-055`) is pinned by `cyrup-tui/src/tests/compaction_status.rs`. **The row was already false when it was last counted — see `03-cyrup-session.md`.** |
| 2 | **`PROV-047`** | 01 + 02 + 06 + 08 | `httpProxy` reaches OAuth and the ADC minting path, but **the fix stays inert in production until one line lands**: `configure_http_proxy(...)` beside the existing `configure_http_idle_timeout(timeout_ms)` in `cyrup-session-svc/src/builder.rs`. Two further one-liners (`cyrup-agent/src/proxy.rs:468`, `cyrup-ext/src/caps/http.rs:599`) complete it.  **STRUCK 2026-08-19: CLOSED 2026-08-15 (area 01) — the line landed.** `builder.rs:296-299` (`apply_http_proxy_settings`, called from `:1516`) and `crates/cyrup/src/main.rs:177` are both live production call sites; `cyrup-agent/src/proxy.rs:473` records the transport half in-tree. |

`SEAM-061` — ranked #1 in the first edition — **is closed as REFUTED**: sweep 6 found it already
landed at HEAD in *both* crates (`cyrup-tui/src/session_selector.rs:154`/`:276`/`:313`/`:1918`/`:1985`
and `crates/cyrup/src/main.rs:1354` + `startup_ui.rs:191-201`). **Areas 08, 09 and 10 now have zero
open criticals and zero open highs between them.**

## Why sweep 6 "refuted" 39 rows — two different failures wearing one word

**This is the correction a reader most needs, and it cuts the other way from the first edition's
headline.** Sweep 6 recorded ~39 `refuted-not-fixed` outcomes across five agents. They are **not** 39
analysis errors. Separate them:

- **(b) DOC STALENESS — roughly 32 of the 39.** The analysis was right, a sweep between 3 and 5
  landed the fix, and **no writer ever reconciled it**, so the row still said "still open". Area 06
  alone accounts for eighteen: every one of `EXT-007`, `-009`, `-011`, `-016`, `-018`, `-023`, `-030`,
  `-032`, `-033`, `-034`, `-036`, `-044`, `-045`, `-046`, `-049`, `-052`, `-056`, `-057` still read
  "still open" in `## Status of every item from prior analyses` **while that file's own
  `## Open items` table had already marked them CLOSED.** Two tables in one file disagreed for four
  sweeps. Areas 09 and 11 each contributed six more of the same kind.
- **(a) GENUINE ANALYSIS ERROR — roughly 7.** Fabricated or wrong citations (`PROV-011`'s claim that
  pi's built-in Edit/Write/Read/Bash declare `constrainedSampling` — **no pi built-in does, three
  grep hits total at v0.83.0**; three fabrications and four wrong-offset clusters in `EXT-036`),
  a wrong premise (`LEAK-FAIL`'s "the victim is arbitrary" — nextest runs each test in its own
  process, so the victim can only be the test that spawned the leak), a misclassification
  (`PERM-017` is a tracker, not work), stale headline evidence (`CFG-014`'s "grep returns ZERO",
  `CFG-048`'s "~30-name rename table" — it is **59**, diffed pair-for-pair), and **one refutation
  that was itself wrong** (`PERM-008-R2`, below).

**Combined across all six sweeps: ≈53 recorded refutations against ~430 items worked ≈ 12%** — the
same rate the first edition measured, and it did not improve. **But the failure mode has shifted:**
sweeps 1-2's refutations were mostly type (a); sweep 6's are mostly type (b). **Type (b) is cheaper
per instance and far more expensive in aggregate, because it costs a whole agent-pass to rediscover
and it is entirely preventable by reconciling documentation every sweep instead of every four.**

**One refutation was itself wrong, and this is the sharpest lesson in the file.** Sweep 2 refuted a
third of `PERM-008`'s Verify recipe on the ground that pi emits no warning for a malformed forwarded
request: `if (!request) { safeDeleteFile(...); continue; }` at `index.ts:1144-1147` has no log call
above it. **The reading of the CALLER is correct; the conclusion is not** — upstream logs one frame
down, inside `readForwardedPermissionRequest` (`:942` catch, `:928` field-ladder). The test that
resulted **pinned cyrup-invented silence as if it were parity**, and sweep 6 had to un-pin it.
**A test that pins ABSENCE is only as good as the frame depth of the citation behind it. Absence
claims require the callee to be opened too.**

## ORCHESTRATION — per-crate partitioning stalled at sweep 4; partitioning by FEATURE unblocked it

**Record this for whoever runs sweep 7; it is the highest-leverage finding of this edition and it is
about the process, not the code.**

- **Sweep 4, partitioned per-crate, landed 15 items.** Per-crate ownership strands everything
  cross-cutting: an agent that owns the crate where the defect is *observed* usually does not own the
  crate where the fix *lands*, so the honest outcome is "blocked", every pass, forever.
- **Sweep 5 repartitioned by FEATURE — each agent owning every crate its feature needs — and landed
  all five assigned items, including `SUBA-S01`,** which three per-crate passes had left blocked.
- **Sweep 6 kept the feature partition and landed ~15**, including `PROV-011`, which had been
  reported "clean" by five consecutive provider-side re-verifications **because both of its two
  remaining defects were plumbing frames in the middle** (`cyrup-agent/src/agent.rs:818` and
  `cyrup-ext/src/wrapper.rs`) — sites no per-crate provider agent would ever have opened.

**Three limits of the feature partition, all observed in sweep 6, all worth designing around:**

1. **Ownership partitions the WRITE set; it says nothing about the BUILD set,** which is the
   dependency closure. `crates/cyrup-intercom` cannot type-check while `crates/cyrup-ext` is
   mid-edit, and cyrup-ext's WIT was being changed by a concurrent agent for ~20 minutes of the pass.
2. **Exclusive crate ownership was violated twice, both times for good reason.** The provider agent
   edited `crates/cyrup-ext/src/wrapper.rs` (one delegating override) because landing only its own
   half would have shipped a feature no tool could use — the deferral pattern the standing rules
   forbid. `crates/cyrup/src/main.rs` was edited by two agents in the same window, one of them via
   **whole-file scripted rewrites** — a clobbering risk that agent itself flagged and said it would
   not repeat. **A shared bin like `crates/cyrup/src/main.rs` should be assigned to exactly one agent
   per sweep, or edited only through anchored patches.**
3. **A "feature" must actually be one.** Area 04's tail was paired with area 11's, and they have
   nothing in common: **not one open row in `04-cyrup-tools.md` has a fix site inside
   `crates/cyrup-tools/**` any more** (`TOOL-015`/`-022` → cyrup-tui + cyrup-core, `TOOL-017` →
   cyrup-tui + a product decision, `TOOL-024` → cyrup-ext, `TOOL-031`'s residual →
   cyrup-ext-subagents). **Area 04 is finished as a crate; what remains is five rows filed under the
   wrong area.** Every affected row now names its FIX SITE, and `07-cyrup-tui.md` carries a routing
   table of the eleven foreign-filed rows that land in it. **Route sweep 7 by fix site, not by area
   number** — or it will again spawn an agent with no reachable work.

## Test architecture and the gate — updated numbers

- The integration tests were relocated into their crates as unit tests (`63d729a` / `c3982b5` /
  `d973906`): **310 integration binaries → 6 + 8 gated**, behind the `cyrup-it` harness crate.
- **The gate is now 6699 tests in 16.3 s** (was 6440 in 16.4 s at the first edition; 3932 and 6387
  are older still). **Every `crates/<crate>/tests/<x>.rs` citation in this directory is stale unless
  it names `cyrup-it`.**
- **Structural defect J is unchanged and now has a second and third instance.** `crates/cyrup-it` is
  `required-features = ["it"]`, so the 16-second gate builds and runs **none** of it. Consequences
  now on the record: (i) `EXT-025` cannot be closed by deleting its dead methods, because the
  breakage would land silently in the un-built crate — it needs **one** agent owning cyrup-ext +
  cyrup-session-svc + cyrup-it in a single commit; (ii) the 0.6 and 0.7 `HOST_WORLD` bumps have
  **never been instantiated against a real guest**, because that fixture lives there too, and the
  failure mode is an opaque wasmtime LINK error; (iii) the four `cyrup-it` assertions the first
  edition cited as contradicting production **no longer do** — sweep 6 re-read them and they match
  (`ICOM-026`, closed as refuted). The gap is now filed as **`ICOM-053`** so it is tracked as a
  statement about the GATE rather than hidden inside a closed test-defect row.

## Corrections to the previous edition — wrong about the CODE or about UPSTREAM, not merely stale

1. **`CFG-042`/`CFG-048`'s "needs `indexmap` in the workspace dependency table" is VOID.** The
   insertion-ordered map landed in sweep 6 as a ~60-line local `OrderedObject` in
   `crates/cyrup-config/src/models_store.rs` with **no new dependency**. The mechanism claim in
   register entry **D** is right; its prescription was wrong.
2. **`PERM-008`'s "REFUTED ONE THIRD OF THE RECIPE" is struck** — see above. The observation about the
   caller stands; the conclusion and the test it produced were wrong.
3. **`SEAM-061`'s ranking note ("two sweeps have split this across areas 07 and 08 and neither took
   it") is struck** — it was already closed at HEAD in both crates.
4. **The claim that pi's built-in tools declare `constrainedSampling` is struck.** Three grep hits
   exist at v0.83.0, all of them the `ToolDefinition` field (`extensions/types.ts:463`) and the two
   `tool-definition-wrapper.ts` copies (`:14`, `:42`). **Adding opt-ins to cyrup-tools' built-ins
   would be a divergence FROM pi.**
5. **`SUBA-N03`'s closure is wider than recorded**: it covers **eleven** `async:true` single-mode
   parameters, not eight, and the load-bearing half — that the parameters reach the **detached hop-2
   runner** at the `runner-config.json` boundary, not merely past the router — is separately pinned.


---

# RECONCILED 2026-08-14 — two parity sweeps applied, every count re-derived

> **cyrup HEAD `380c713`** (was `04c1ba2` when the edition below was written), branch `david/cyrup`,
> tree clean. Gate: **`cargo nextest run --workspace` = 6440 tests, 6440 passed, 8 skipped, 16.4 s**;
> `cargo check --workspace --all-targets` clean.
>
> **Everything below this block — including the `REGENERATED 2026-08-12` header that follows it — is
> the previous edition, and is superseded on every number.** It is
> retained unedited because its *reasoning* — the severity-scale correction, the duplication census,
> the structural-defect list — is still the argument this file rests on. Only the arithmetic is dead.

## What happened, honestly

Two whole-backlog parity sweeps ran against this directory's twelve area files. Area agents were
**forbidden from editing documentation** so that a single writer could reconcile all sixteen files in
one pass without sixteen partial edits racing each other; this section, and the per-row dispositions
now written into every `## Open items` table, are that reconciliation.

**Sweep 1** landed **232 items across 11 crates** (commit `380c713`) and handed back **184
`docUpdatesNeeded` entries**. **Sweep 2** ran the same shape over what remained. Both were restricted
to `cargo check -p <crate> [--all-targets]` — **neither executed the test suite**; the orchestrator
ran the gate once over the combined work.

### The open set — before and after, derived from the twelve tables, nothing carried forward

| | critical | high | medium | low | **counted open** |
|---|---:|---:|---:|---:|---:|
| **before** (the twelve tables as this reconciliation found them) | 1 | 34 | 195 | 233 | **463** |
| **after** | **0** | **3** | **75** | **95** | **173** |

**290 rows moved to closed.** 21 rows already carried a closure marker before either sweep, so
**311 of the 492 rows across the twelve tables now carry one**. Eight rows are new: seven filed and
closed in the same pass (`PROV-053`, `AGENT-034`, `AGENT-035`, `SESS-045`…`SESS-048`) and one filed
open (`EXT-060`). **Nine `tracker` rows** are excluded from every figure above, as always — seven in
the area tables, plus `SEAM-058` and `SUBA-005` in their areas' separate `## Trackers` tables.

Note that the "before" row does not match this file's own previous headline of 448 raw / ~420
distinct, nor `README.md`'s 458. Both were computed before the repair pass finished growing the
tables, and neither was re-derived afterwards. **463 is what the tables actually contained**; every
figure in this section was produced by parsing them, and the parse is reproducible.

### By area

| file | crit | high | medium | low | **open** |
|---|---:|---:|---:|---:|---:|
| `01-cyrup-core-and-provider.md` | 0 | 1 | 5 | 6 | **12** |
| `02-cyrup-agent.md` | 0 | 0 | 0 | 2 | **2** |
| `03-cyrup-session.md` | 0 | 1 | 1 | 6 | **8** |
| `04-cyrup-tools.md` | 0 | 0 | 1 | 5 | **6** |
| `05-cyrup-config-and-resources.md` | 0 | 0 | 11 | 7 | **18** |
| `06-cyrup-ext.md` | 0 | 0 | 11 | 13 | **24** |
| `07-cyrup-tui.md` | 0 | 0 | 14 | 20 | **34** |
| `08-cyrup-session-svc-and-modes.md` | 0 | 1 | 3 | 4 | **8** |
| `09-cyrup-ext-subagents.md` | 0 | 0 | 14 | 12 | **26** |
| `10-cyrup-permission-system.md` | 0 | 0 | 2 | 3 | **5** |
| `11-cyrup-intercom.md` | 0 | 0 | 6 | 8 | **14** |
| `12-upstream-drift-pi-core.md` | 0 | 0 | 7 | 9 | **16** |
| | **0** | **3** | **75** | **95** | **173** |

**The three remaining highs, and they are the whole actionable set:**

| # | ID | area | why it is still the top of the backlog |
|---|---|---|---|
| ~~1~~ | ~~**`SEAM-061`**~~ **CLOSED 2026-08-14 — REFUTED (sweep 6): already landed at HEAD in BOTH crates — `cyrup-tui/src/session_selector.rs:154`/`:276`/`:313`/`:1918`/`:1985` and `crates/cyrup/src/main.rs:1354` + `startup_ui.rs:191-201`. The "blocking evidence" below, and the "two sweeps split this and neither took it" note, are both wrong at HEAD.** | ~~08 + 07~~ | The `--resume` picker lists every project's sessions under a header that says "Current Folder", with a `tab scope` hint bound to nothing. The blocking evidence has NARROWED to one crate: `SessionScope`, `SessionSelector::set_scope` and `scope()` all exist (`session_selector.rs:54`, `:250`, `:255`); what is missing is `SessionAction::ToggleScope` in `keymap.rs:888-909`, its `handle` arm, and making `show_path` follow the scope. **Two sweeps have split this across areas 07 and 08 and neither took it. It must go to one agent holding both crates.** |
| 2 | **`SESS-040`** | 03 + 07 + 08 | A shipped control that bills tokens and rewrites the session file still has no dispatch site. Its two siblings are now closed — `SESS-041` (auto-compaction token) and `SESS-042` (the `aborted: true` payload) — so **040, 041 and 042 now differ only in wiring: the moment 040 lands a caller the abort actually takes effect.** Blocked on `TUI-055`'s consequence, not on `TUI-055`: the band renders now, but nobody has watched it.  **STRUCK 2026-08-15 (batch B, area 03): CLOSED as REFUTED — the Escape dispatch landed in `380c713`.** `rg -n AbortCompaction crates/` returns a production dispatch at `cyrup-tui/src/app/input.rs:144-146` (an `Action::Interrupt` branch on `state.compacting`, ahead of the four-branch chain and behind the `branch_summary_in_flight` check) routed to `ctx.session.abort_compaction()` at `app/run_action.rs:53-54` (`session.rs:1900`), plus assertions at `cyrup-tui/src/tests/escape_chain.rs:233,244`; the band half (`TUI-055`) is pinned by `cyrup-tui/src/tests/compaction_status.rs`. **The row was already false when it was last counted — see `03-cyrup-session.md`.** |
| 3 | **`PROV-047`** | 01 + 02 + 06 + 08 | `httpProxy` now reaches OAuth and the ADC minting path, but **the fix is inert in production until one line lands**: `configure_http_proxy(...)` beside the existing `configure_http_idle_timeout(timeout_ms)` in `cyrup-session-svc/src/builder.rs`. Two further one-liners (`cyrup-agent/src/proxy.rs:468`, `cyrup-ext/src/caps/http.rs:599`) complete it.  **STRUCK 2026-08-19: CLOSED 2026-08-15 (area 01) — the line landed.** `builder.rs:296-299` (`apply_http_proxy_settings`, called from `:1516`) and `crates/cyrup/src/main.rs:177` are both live production call sites; `cyrup-agent/src/proxy.rs:473` records the transport half in-tree. |

## The analysis's own error rate is now measured, not estimated

This is the most useful thing the two sweeps produced, and it is a property of *this directory*, not
of the port.

- **Sweep 1 refuted 31 items** out of ~290 it worked — **≈11%** — including 9 of 23 in
  `cyrup-tools` and 8 of 41 in `cyrup-session-svc`.
- **Sweep 2 recorded 16 further `refuted-not-fixed` outcomes** and roughly a dozen more in-body
  factual corrections to items it *did* fix (a wrong upstream constant, a wrong line range, a wrong
  premise about what blocks the work).
- Combined: **≈47 recorded refutations against ~380 items worked across the two sweeps ≈ 12%.**
  **Fifteen rows carry an explicit `REFUTED, CLOSED 2026-08-14` marker**; the rest are recorded
  inside the closure note of an item that was also fixed, or as a correction on a row that stays
  open. No refuted ID was renumbered or deleted.

**Refuting is a success, not a shortfall** — but a 12% error rate has three consequences a planner
must act on:

1. **A stale-closed item costs a whole pass.** `PROV-027`, `PROV-028` and `PROV-029` — three of area
   01's six highs — were already fixed and were closed by *neither* sweep's code work; the file had
   simply never been re-read. `SEAM-047`, `SEAM-051`, `SEAM-064`, `SEAM-072` and `DRIFT-049` were all
   marked fixed in their *kind* cell while their *severity* cell still read `high`, which is how two
   consecutive recounts published phantom highs.
2. **Verify a deferral's stated blocker before accepting the deferral.** `PROV-030` sat open through
   sweep 1 on the ground that "cyrup-provider has no crypto/JWT dep". `ring` 0.17 was already in
   `Cargo.lock` via rustls — checkable in thirty seconds. The same deferral was masking a second,
   independent defect (`PROV-053`) that would have kept the feature dead anyway. Likewise
   `EXT-007`'s "blocked on the `Tool::prompt_guidelines` signature", which three separate places in
   the record were still planning around after the signature had been widened.
3. **A handoff field is not a record.** `SESS-037` appears in none of sweep 1's structured
   `fixedIds` / `partial` / `notReached` buckets because its area's free-text `unresolved` string was
   **truncated at exactly 2500 characters mid-sentence**. It was re-verified from scratch and would
   have been re-worked. That truncation is systematic across areas in the handoff file. Area 01's
   sweep-2 agent independently reported the same thing: its structured fields said "1 not-reached, 2
   partial" while the prose named six not-reached and mentioned none of the three highs it found
   already closed. **Plan from the prose, not the buckets — or fix the buckets.**

## JS→Rust mechanism gaps — the register

This is the highest-value class in this port, and it is not "cyrup does X where pi does Y". It is
**pi relying on a guarantee Rust does not give**. Sweep 1's dedup deadlock came from this class; so
did the two worst near-misses of sweep 2. Every entry below is recorded on the item it belongs to;
they are collected here because the *class* is what a future pass should hunt, not the instances.

**A. A future can be dropped at any `.await`; a JS `async` function always settles.**

- Sweep 1's dedup deadlock: a `PendingOwner` died at an `.await` without settling, and its cache
  entry's live `Sender` kept the channel open forever. pi cannot reach it.
- `DRIFT-029`: pi's `executeBash` removes its `AbortController` in a `finally`. A plain
  `let … = …; …; remove()` sequence leaks a live `CancelToken` on a dropped future and makes
  `is_bash_running()` **permanently true**. Closed with an RAII `BashCancelGuard`.
- `EXT-034`: a re-entrancy latch cleared only on the success path stays set forever after an aborted
  run and permanently disables bus delivery. `bus::DrainLatch` is therefore RAII, not a
  `store(false)` at the end.
- `AGENT-035`: a cancelled Rust stream can end **cleanly** where a JS read loop always throws, so a
  cancel landing after the last `None` was invisible and the consumer saw a well-formed stream end
  with no terminal event.
- **Generalisation, cheap to run:** `grep -n 'finally {' packages/coding-agent/src/core/agent-session.ts`
  enumerates every pi site where a `finally` mutates session state. Each one is a place the Rust port
  needs either a guard or a provably-reached statement.

**B. A `tokio::Mutex` has no deadlock detection, so the failure mode is a HANG, not a red test.**

`EXT-034`'s first cut drained the bus unconditionally at every dispatch seam. `dispatch_*_excluding`
with `exclude = Some(id)` exists precisely because that guest is **suspended inside its own
`provider-stream.on-payload` host import, holding its single-instance store guard** — so a queued
event addressed to it would have re-taken a lock it already held: an infinite await. pi cannot reach
it: its runner is one JS process, `emit` runs listeners synchronously, and a re-entered handler is an
ordinary nested call with no lock. **The invariant — never drain while a dispatch carries an
`exclude` — is now a comment, a test, and a line in `EXT-034`'s row, because removing the argument
reintroduces a hang rather than a failure.**

**C. TypeScript field types are compile-time only; pi's readers re-validate by hand.**

`SESS-045`: `interface SessionHeader` declares `timestamp` and `cwd` required, but pi's two runtime
validators check `type` and `id` and nothing else, and every consumer carries a
`typeof … ? … : fallback` ternary. Rust's serde turns the declared type into an **enforced** one, so
cyrup hard-failed the whole session file where pi opens it and falls back to `process.cwd()`.
**Anywhere cyrup mirrored a pi `interface` field-for-field as a required Rust field, check whether
pi's readers re-validate it.** `SESS-001` and `SESS-027` are the same mechanism one layer down.

**D. JS coercion has no Rust counterpart, in four distinct shapes.**

- `SESS-048`: `typeof x === "number"` is true for negatives and fractions, so pi's `delete` runs for
  every number where an `as_u64` match returns early. **The Rust idiom for a JS number guard is
  `as_f64`.** And `entries[2.0]` is `entries["2"]` — a JS array index is stringified, so pi resolves
  an integral float where cyrup resolved nothing.
- `SESS-046`: `Array.prototype.sort()` with no comparator is **UTF-16 code-unit order**, which is not
  Rust's `Ord for str` (UTF-8 byte order). They disagree for every pair where one string carries an
  astral code point and the other a code point in U+E000..=U+FFFF. **Any `BTreeSet`/`.sort()` that
  reproduces a pi `.sort()` over user-supplied strings — file paths, model ids, session names, skill
  names — is a candidate.** This one reached persisted summary text.
- `PROV-053`: `std::env::var(name).ok()` returns `Some("")` for a blank variable where pi's ambient
  accessor collapses it to `undefined`. Every precedence chain ported from a JS `??` then reads a
  blank var as CONFIGURED — **a silent inversion of the upstream semantics at every such site, and
  the bug is in the adapter, not in any call site, so a per-item review of the call sites cannot see
  it.**
- `CFG-042` / `CFG-048`: `serde_json::Map` is a `BTreeMap` in this workspace, so it cannot represent
  the insertion order a JS object literal carries and `JSON.stringify` writes. A straight port of
  `orderKeybindingsConfig` would have **silently alphabetised the user's keybindings file**. Handled
  with `Vec<(String, Value)>` plus a hand-written two-space printer; the same gap blocks `CFG-042`'s
  remaining half and needs `indexmap` in the workspace dependency table.

**E. pi keys maps on object identity; Rust has none for a value type.**

`PROV-035`: `collectCacheMisses` returns `Map<AssistantMessage, CacheMiss>` keyed on the message
**reference**, and its consumer looks it up by the same reference while re-rendering. Two structurally
equal `AssistantMessage`s are the same key in Rust. Resolved by keying on the entry index, which
pushes an obligation onto the renderer. `DRIFT-029` is the same shape: pi's Set keys on
`AbortController` identity, **not** on the optional-and-repeatable `options.id` the item proposed.

**F. Rust ports keep inventing defensive clamps upstream does not have.**

`SESS-047`'s `.max(1)` on the summarization `maxTokens` had no counterpart in pi's
`Math.min(Math.floor(frac * reserveTokens), …)`. This is the *inverse* of the usual gap — not a JS
guarantee Rust lacks, but a Rust-side clamp invented to avoid a zero pi is perfectly happy to send.

**G. A faithful port would sometimes reproduce an upstream bug, and that decision needs recording.**

pi's `generateId(byId)` inside `migrateV1ToV2` guards against a set the function never `.add()`s to,
so **pi's collision check is a no-op and can mint the same 8-hex id twice**. cyrup's working dedup
can only differ in the case pi gets wrong. Recorded in-source rather than ported. The same question
will recur.

**H. Two cyrup-original surfaces are wearing fabricated pi citations.**

`EXT-036` was filed for this class in the event catalog. Sweep 2 found a second instance in
`interface ui`: `working-start` / `working-stop` are documented "(Pi startWorking/stopWorking,
types.ts:265-275)" and **neither function exists upstream at v0.83.0** — the cited range is
getAllThemes/getTheme/setTheme/getToolsExpanded. Worse, the invented shape is strictly weaker than
pi's: it welds the message to the visibility, so `setWorkingVisible(false)` with the message intact
was unexpressible. `EXT-060` is the third instance, filed by this reconciliation: cyrup's
`registry.tool_info()` emits a `source: "extension"|"guest"` discriminator that pi's `ToolInfo` does
not have — **the WASM-vs-native tier leaking into a guest-facing introspection API**, a distinction
pi's one-extension-kind model has no word for. **Re-run the `EXT-036` sweep over the import surfaces,
not only the event catalog.**


**I. Instance-scoped host state substituted for a call-scoped pi parameter must be torn down in `Drop`. (`EXT-M01` — the sixth real bug from this class.)**

`LiveExtension::execute_tool` binds the tool's `CancelToken` on `GuestState` before awaiting the guest
call and cleared it on both arms of a `tokio::select!`. Upstream's `signal` is a **parameter** of
`ToolDefinition.execute` (`extensions/types.ts:483` @v0.83.0), so it is call-scoped by the language and
a started `async` function always settles. A Rust future has a **third** exit neither `select!` arm
covers — being dropped at the await by an outer `select!`, a `timeout`, or an aborted `JoinHandle` —
and on that path the cancelled token stayed bound forever. `host-tool.is-cancelled` is **not gated to
tool calls** (`world.wit:773`), so every later poll from every guest handler answered `true`: a guest
that checks it to decide whether to keep working silently stopped working. Fixed with an RAII
`ToolCancelBinding` declared *after* the instance mutex guard, so it drops *before* it and no other
call can observe the gap. **This generalises to the whole WIT seam:** every pi surface where a
parameter cannot cross the Component Model boundary is re-expressed as host-side instance state plus a
guest poll import (`signal` → `is-cancelled`, `AbortSignal` → `is-run-cancelled`, the `onUpdate`
callback → `emit-update` + `take_tool_updates`). **It is a bug generator, not a one-off.**

**J. An unbiased `tokio::select!` picks at RANDOM; a JS race cannot. (`EXT-M02`.)**

`EpochDriver::spawn` raced `token.cancelled()` against `iv.tick()` with no `biased;`, and a freshly
built `tokio::interval` fires its **first tick immediately** — so whenever the token was already
cancelled at spawn, both arms were ready on the very first iteration and shutdown depended on a coin
flip (expected ~2 iterations: a flake, not a hang). **Audit result, recorded so it is not redone:**
this was the only unbiased `select!` in cyrup-ext / cyrup-ext-sdk / cyrup-sdk; the other eleven carry
`biased;`, and **`caps/proc.rs:297`'s unbiased one is CORRECT** — its loop re-evaluates the
authoritative condition at the top of every iteration and both arms are no-ops. **Do not "fix" it.**

**K. Rust has no object spread, so every wrapper is a hand-written list that silently rots — and the rot is INVISIBLE to the obvious test.**

pi's `wrapRegisteredTool` is `return { ...tool, execute }` (`core/extensions/wrapper.ts:21-22`): every
field survives **by construction**, including fields added years later. `impl Tool for RegisteredTool`
must name each method, and its list of eleven omitted `constrained_sampling` — so a WASM guest's
declaration was read off the descriptor and discarded one frame later (`PROV-011`). **Because a
dropped delegation returns exactly the trait default, `assert_eq!(w.x(), inner.x())` compares the
default against the default and passes with the delegation deleted.** `TOOL-024` fixed nine instances
of that vacuity; `constrained_sampling` was added to the trait afterwards and made a tenth. **Fixing
the fixture once does not immunise it — the "every fixture value is DISTINCT and non-default"
invariant must be re-established in the same commit as every new trait method.**

**L. A JS `Map` is TWO data structures, and this port keeps translating it as one.**

`seenInboundMessages` (`ICOM-017`) uses `has(key)` (hash) **and** `keys().next().value` (insertion
order) inside the same sixteen-line function: the cap eviction is "forget the OLDEST", and a bare
`HashMap` silently makes it "forget one at random". The file had already recorded this trap for
`activeTools` (`v0.10.1 index.ts:677` reading `activeTools.values().next().value`) and it was hit
again. **Standing rule: any pi `Map` whose code touches `.keys()`, `.values()` or `.entries()` needs
an explicit order carrier in Rust.** Register entry **D**'s `serde_json::Map` case is the same
mechanism at the serialization layer — and worth a dedicated sweep rather than one item at a time,
since every user-authored JSON file this port round-trips through `serde_json::Map` gets its key order
rewritten (`models-store.json` was `CFG-042`; `keybindings.json` already carries a bespoke ordered
type).

**M. `Iterator::all` short-circuits; three effectful calls ANDed together do not.**

pi's `ensurePermissionForwardingLocation` evaluates three `ensureDirectoryExists` calls as
unconditional bindings and ANDs the results at the end (`:803-805` → `:808`), so a spool with three
broken directories reports **three** causes. The natural Rust transcription
(`[..].into_iter().all(..)`) reports one and stops. **The RETURN VALUE is identical, which is exactly
why this survives review — only the side effects differ.** Any port of a JS expression that ANDs
several effectful calls needs a fold. (`PERM-033`.)

**N. A boolean predicate has nowhere to put a diagnostic; the JS guard function it replaced logged before returning `false`.**

`forwarding::response_is_bound` had a real caller and correct constant-time semantics — but the caller
was `Option::filter`, which **discards the reason**. Upstream cannot lose it, because the same check is
a function that logs first. **Any Rust `bool` predicate standing in for a JS guard that also logged is
a place to look for dropped diagnostics**, and the security-relevant instance is exactly this one: a
forged or misaddressed forwarded response was discarded leaving only an all-null `response_received`
entry. (`PERM-033`.)

**O. `serde_json::from_str::<T>` collapses two upstream error classes into one.**

pi separates `JSON.parse` failure (catch → `Failed to read …`, **with** the cause) from a field-shape
rejection (→ `Ignoring invalid … format in …`, **without** a cause, because it holds a parsed object
and not an error). A single typed deserialize cannot tell them apart, so a faithful port must go
through `serde_json::Value` first. **Every ported `readX()` that has both a catch and a validation
ladder has this trap, and it is silent:** one plausible message where upstream emits two distinct ones,
with the `error` key wrongly present or wrongly absent. (`PERM-033`.)

**P. A correlation table keyed by id is deliberately REUSED across operations upstream; "tidying" it into two tables type-checks and then hangs.**

pi's `cancelMessage` resolves through the **same** `pendingSends` map as `send`, keyed by the
**cancelled** message's id, because the broker answers a cancel with `delivered { messageId }` naming
that id. A Rust port that introduces a separate `pending_cancels` table compiles and then waits
forever on every cancel — the ack arrives and matches nothing. (`ICOM-017`.)

**Q. A mechanism ported one level up or down turns an invariant into an obligation.**

pi's `showWarning` builds `Warning: ${message}` **inside** the function
(`interactive-mode.ts:3885-3889`); cyrup's `Entry::Warning` renders verbatim, so the prefix became a
**per-caller** duty. Two of three callers complied; `main.rs`'s `modelFallbackMessage` push did not, so
a credential-less first run rendered a bare sentence where pi renders a labelled warning. (`TUI-062`.)
The same shape appears in `findAutoloadDeltaBase` (`CFG-026`): pi computes each scope's identity
against **its own** base (`package-manager.ts:1307` vs `:1311`), so a relative local path never pairs —
cyrup's raw-string comparison paired them, i.e. **cyrup was more permissive than pi**, and the in-code
doc comment asserted the opposite of upstream's behaviour.

**R. `??` and `||` differ on the empty string, and pi uses both within sixty lines.**

`result.reason ?? "Message may not exist..."` (`v0.10.1 index.ts:1955`) **keeps** an empty-string
reason, where the `||` fallbacks nearby replace it. `unwrap_or_else` on `Option` is `??`; reproducing
`||` needs an extra `.filter(|s| !s.is_empty())`. (`ICOM-017`.)

**S. `std::process::Command` inherits every UNNAMED stdio handle, so a test fixture can hold the harness's pipe open.**

nextest waits `leak-timeout` for EOF on fds 1/2 after a test process exits, and EOF needs every copy of
the write end closed. Naming a handle `dup2`s over the harness's copy — **only an omitted handle can
leak**. One test (`path_probe_is_bounded`) named `.stdout(piped())` only, handed its child the
harness's stdin and stderr, and left it alive for 30 s on an error arm — producing an intermittent
`LEAK` on the very test whose purpose was proving the production probe reaps (`TOOL-042`). **Reviewing
a spawn for what it SETS can never catch this; only a rule about what it must not leave UNSET does.**
`.output()` is safe (it overrides all three); **`.status()` is NOT** — any future lint that exempts
"terminal builder methods" as a class re-opens the leak through `.status()`.

> **AMENDED, third edition — this entry is CORRECT and INSUFFICIENT, and the distinction is the
> lesson.** The rule it states closed a real class: 286 measured runs put the LEAK rate at ~1.0%,
> down from ~4 in 33 (~12%). **But the failure still fires**, and the one occurrence that was
> instrumented cannot be an inherited handle: it names a test driving an in-memory `RecordingProc`
> whose only possible child (`which bash`) names all three handles and is reaped, and over 80
> sampled runs **no orphan was ever seen holding a harness pipe** (69 orphan pipe addresses vs 244
> harness pipe addresses, zero intersection). **The generalisable form: a LEAK-FAIL is not proof of
> an inherited handle. A tripwire's RED does not name its own cause, and a fix validated against the
> wrong signal looks exactly like a fix.** See `TOOL-042` in `04-cyrup-tools.md`.

**T. Two functions ported from two different upstream files can leave a value classified by neither.**

`is_local_path` (from `paths.ts:41-55`) treats `github:user/repo` as **non-local**, while
`parse_git_url` (from `git.ts`) rejects it because the prefix is not `git:` and the scheme is not
https/http/ssh/git — so the source is simultaneously "not a local path" and stored as one.
~~Upstream's `parseGitUrl` reaches `hostedGitInfo.fromUrl`, which resolves the shorthands.~~ (`CFG-052`.)

> **STRUCK, third edition — this entry's premise about upstream is FALSE and the entry is withdrawn
> as a mechanism gap.** `parseGitUrl` opens with
> `if (!hasGitPrefix && !/^(https?|ssh|git):\/\//i.test(url)) return null;` (`utils/git.ts:172-179`
> @v0.83.0) and its own doc comment says verbatim *"Without git: prefix, only accept explicit
> protocol URLs"* (`:165-171`), so **upstream returns null before ever reaching `fromUrl`** and its
> `parseSource` stores the shorthand through the final local arm (`package-manager.ts:1459`).
> **The "two files whose domains no longer meet" condition is UPSTREAM's, and cyrup ports it
> faithfully.** The *shape* of the observation — two functions ported from two different upstream
> files can leave a value classified by neither — remains a real hazard worth watching for; it just
> has no instance here. `CFG-052` is closed as REFUTED. **This is the third edition's clearest
> reminder that a citation is only as good as the frame you opened.**

## Structural defect J — the merge gate does not cover the `cyrup-it` harness

New this pass, and it is a property of the gate rather than of any item.

`crates/cyrup-it` is `required-features = ["it"]` (its own `Cargo.toml:26-34` says so), so
`cargo test --workspace` / `cargo nextest run --workspace` **does not build or run it**. The 6440-test
figure therefore gives **zero coverage of the broker-socket seam tests**. **UPDATED 2026-08-14 (sweep 6).** The four `cyrup-it` assertions this section cited as contradicting
production (`tests/intercom/tool_actions.rs:319`, `:372`, `:502`,
`tests/intercom/intercom_command_transcript.rs:144`) **no longer do** — they were re-read at HEAD and
match production with no trailing period (`ICOM-026`, closed as REFUTED). **The structural defect is
unchanged and is now filed in its own right as `ICOM-053`**, because it is a statement about the GATE
rather than about those tests, and burying it inside a closed test-defect row is how it would be lost.
Two further consequences are now on the record: **`EXT-025` cannot be closed by deleting its dead
methods** — the only callers are tests in this un-built crate, so the breakage would land silently, and
it needs ONE agent owning cyrup-ext + cyrup-session-svc + cyrup-it in a single commit; and **the 0.6
and 0.7 `HOST_WORLD` bumps have never been instantiated against a real guest**, because the Tier-1
fixture component lives here too, with an opaque wasmtime LINK error as the failure mode. `PERM-022`
moved into the same crate and inherits the problem. **Either the gate gains a second invocation with
`--features it`, or every assertion in that crate should be treated as unverified.**

## Test architecture — recorded because it invalidates path citations everywhere

The integration tests were relocated into their crates as unit tests (`63d729a` / `c3982b5` /
`d973906`): **310 integration binaries → 6 + 8 gated**, behind the new `cyrup-it` harness crate, with
the gate now at **6699 tests, 6699 passed, 7 skipped, in 16.3 s at HEAD `bdcb0d0`** (**6440 in 16.4 s**
was this section's own previous figure; the inherited 3932 and the repro pass's 6387 are older still —
all three are superseded). **Every `crates/<crate>/tests/<x>.rs` citation in this directory
is stale unless it names `cyrup-it`** — the affected items are enumerated in each area file's
reconciliation block. Areas 02, 03 and 04 additionally have no out-of-crate `tests/` directory at all.

## What a planner should do next, in order

1. ~~**`SEAM-061`** — one agent, both crates (07 + 08). It is the only high whose fix is fully~~
   ~~understood and fully blocked on coordination.~~ **STRUCK 2026-08-14 (sweep 6): closed as REFUTED — it was already landed at HEAD in both crates. The ranked actionable set is now `SESS-040` and `PROV-047`.**
2. **The one-line residuals**, which are cheap and are what makes three landed features actually
   reachable by a user: `PROV-047`'s `configure_http_proxy` call in the session-svc builder;
   `EXT-037`/`EXT-038`'s two `LiveHostServices` impls (`commands()` off
   `AgentSession::slash_command_catalog`, `all_tools()` off the dynamic tool registry, so built-ins
   appear); `SESS-035`'s doc line. Each is small, mechanical, and named exactly in its row.
3. **Build and instantiate the Tier-1 WASM fixture.** `HOST_WORLD` moved `0.5 → 0.6` in sweep 2 and
   **nothing has proven it against a real guest** — host and guest agree at the type level and no
   further. A failure will present as an opaque wasmtime link error.
4. **Move the items that cannot be closed where they are filed**, rather than letting a fourth pass
   re-derive the same blocked plan: `EXT-025`, `EXT-003`, `EXT-059`, `EXT-013`, `EXT-041`, `EXT-053`,
   the residuals of `EXT-039`/`040`/`019`, `TOOL-024`, `PERM-011`, `PERM-022`, `SEAM-073`(b).
5. **Read `caps/{http,fs,proc}.rs` and the `is_trusted` gate** (area 06, blind spot 4). Roughly 1000
   lines, never read by any pass, and now the only part of `host/services.rs` nobody has walked —
   in the same file where `EXT-054` found the entire manifest capability model inert.
6. **Re-derive every remaining `upstream-drift` row at `v0.83.0` before scheduling it.** Four more
   kind corrections landed this pass (`DRIFT-013`, `DRIFT-029`, `DRIFT-046`, plus `DRIFT-033`'s
   scope), all in the same direction: filed as version lag, actually a port omission inside the
   baseline. **This file's default assumption about that class is wrong more often than it is right.**

---

> **REGENERATED 2026-08-12 (second edition, same day) — cyrup HEAD `04c1ba2`** (last code commit;
> repo HEAD `a9000b1` is docs-only, branch `david/cyrup`), against **pi `v0.84.1`**,
> **pi-subagents `v0.47.1`**, **pi-permission-system `v0.8.0`**, **pi-intercom `v0.10.1`**.
>
> **This edition is derived from the twelve area files AFTER the repair pass** that applied the
> 17-finding completeness critique. Severities moved, six items were reclassified as trackers,
> duplicates gained machine-readable `duplicate-of:` markers, and 31 further items landed from four
> upstream surfaces no file in this directory had ever named. **Every count in the previous edition
> is wrong.** Nothing below is carried forward; every figure was recomputed from the twelve
> `## Open items` tables at their current contents.
>
> **The four headline results.**
>
> 1. **The "zero criticals" headline is gone. There are six**, and they were not created by new
>    code — they were created by applying `README.md:106-107`'s own definition to items already in
>    the backlog. `AGENT-020`, `TUI-027`, `EXT-054` and `PERM-009` were re-rated on their own text;
>    `TUI-042` and `TUI-043` are new. Four of the six are silent data loss in the prompt editor or
>    the agent seam. **The previous edition's result #1 was an artifact of not applying the scale.**
> 2. **Highs went 14 → 22**, mostly from one axis nobody had run: area 08's sweep of
>    `pi/packages/coding-agent/src/cli/` alone produced five highs on the pre-launch startup surface
>    (`SEAM-061`…`SEAM-065`). Area 08 now carries seven open highs, more than any other area.
> 3. **The open count is 448 raw IDs / ~420 distinct defects / 9 trackers** — see *The open set*.
>    The previous edition published 426 as a floor while it was simultaneously inflated by known
>    duplication; both figures are now published separately and labelled.
> 4. **117 closed against 207 filed.** The port converges by *severity*, not by item count — and
>    this pass it did not even converge by severity, because the severity scale was being applied
>    incorrectly. That correction, not new breakage, is what produced the six criticals.
>
> **Claims struck from the previous edition** (each checked at the source this pass):
>
> - ~~"`PARITY-GAPS.md`'s own header is stale — it records `pi-subagents` latest v0.43.0 and
>   `pi-intercom` baseline v0.7.0 / latest v0.9.2"~~ — **FALSE, struck.** `PARITY-GAPS.md:3-8` and its
>   baseline table at `:20-25` record **v0.47.1** and **v0.10.1**, and the file *opens* by explaining
>   that correction and the `pi-intercom` v0.9.2 baseline. This ledger was the stale file, in both
>   places it claimed otherwise (previously at `:282-284` and in structural defect D).
> - ~~"Catalog lag is unresolvable from this workspace… there is no in-tree regeneration source"~~ —
>   **FALSE, struck.** `packages/ai/scripts/generate-models.ts` (2,733 lines) plus `model-data.ts`,
>   `models-dev-reasoning-options.ts`, `check-model-data.ts`, `scripts/diff-model-catalog.mjs` and
>   `scripts/publish-model-catalog.mjs` all exist at **both** tags, and pi's root `package.json:24-30`
>   exposes `generate:models` / `hydrate:model-data` / `generate:model-catalog` / `diff:model-catalog`
>   / `check:model-catalog`. Only the generated **output** (`packages/ai/src/providers/data/`) is
>   gitignored (`.gitignore:11`). The work is `PROV-018` (tooling) and it is tractable today;
>   `DRIFT-009` has been rewritten to defer to it.
> - ~~"Zero critical items remain open"~~ — see headline 1.
>
> **This is a STATIC analysis.** Nothing in this file or in any area file was reproduced by running
> cyrup or pi. No binary was built, launched or tested for this pass or the repair pass; no Rust or
> TypeScript source was modified. Every item is evidenced by reading both sources at a named tag.
> Severity and effort are judgements, not measurements. Every `Verify` line in every area file is a
> design, not an observation — and for any TUI item, a design that is **not satisfied by a
> `TestBackend` unit test**: the standing rule is that TUI work is not done until it has been run in
> a real terminal.

---

## The open set at `04c1ba2` — two figures, both published

> **SUPERSEDED 2026-08-14.** Both figures below were computed before the repair pass finished
> growing the tables and neither was re-derived afterwards; the tables actually contained **463**
> counted rows, not 448. The current figure is **173 open — 0 critical, 3 high, 75 medium, 95 low**.
> See *RECONCILED 2026-08-14* at the top of this file. The **deduplication reasoning** below (the
> `duplicate-of` census and the F4 cluster) is still valid and was not re-run; a deduplicated figure
> for 173 has not been computed.

The previous edition called 426 a floor while its own cluster tables documented ≥25 IDs of
double-count inside it. Both numbers are now stated, each labelled, so the count can be used.

| | critical | high | medium | low | **total** |
|---|---|---|---|---|---|
| **raw ID count** (severity-bearing rows in the twelve `## Open items` tables) | **6** | **22** | **197** | **223** | **448** |
| less area-12 rows carrying a machine-readable `duplicate-of:` an in-census ID | — | −1 | −3 | −11 | **−15** |
| less cross-area cluster excess in areas 01–11 (cluster F4, editorial) | — | — | −6 | −7 | **−13** |
| **deduplicated distinct defects** | **6** | **21** | **188** | **205** | **~420** |

**Plus 9 `tracker` rows, excluded from every figure above** (see *Trackers*), and 2 `partially-closed`
rows in area 02 (`AGENT-S01`, `AGENT-S04`) that are listed for provenance and counted nowhere.
Total rows across the twelve tables: **457**.

The deduplication is exact for the 15 area-12 rows — each carries `duplicate-of: <ID>` in its row,
its status row and its body's `Kind` line, and the owning area is named. The 13 cross-area
subtractions are **editorial**: they come from cluster F4 below, where a defect that is genuinely one
piece of work carries two or three IDs in different areas. Some F4 members are separable halves
rather than true duplicates (`SEAM-051` is a flag, `CFG-021` is a settings key, `TUI-019` is the
renderer), so ~420 is itself a soft figure. **Use 448 for "how many rows must be dispositioned" and
~420 for "how many distinct things are wrong."** Neither is a total — see structural defect C.

**Effort profile: S 286 · M 130 · L 32.** 64% of the backlog is effort `S`. Of the 28 criticals and
highs: **15 `S`, 12 `M`, 1 `L`** — the top of the backlog is unusually cheap.

**Movement since the first 2026-08-12 edition: 426 − 9 + 31 = 448.** Nine items left the count as
trackers — `PROV-004`, `AGENT-028`, `SESS-038`, `SEAM-058`, `SUBA-005`, and area 12's `DRIFT-022`,
`DRIFT-023`, `DRIFT-032`, `DRIFT-040` — and 31 items entered it. **No ID was renumbered, merged or
deleted to produce any of this**, in any of the twelve files; every tracker keeps its ID, its row and
its full body.

---

## The actionable set — 6 criticals + 22 highs

> **SUPERSEDED 2026-08-14 — every row below is dispositioned.** All six criticals and 31 of the 34
> highs are closed; the actionable set is now three items (`SEAM-061`, `SESS-040`, `PROV-047`) and is
> tabled at the top of this file. The section is retained because each row's *fix sketch* is still the
> best short statement of what the work was, and because several rows record a refutation that must
> not be lost — in particular row 1 (`AGENT-020`, refuted by measurement) and row 6 (`EXT-054`).
> **Do not plan from the ranking below.**

Ranked by the criterion stated in *Ranking proposal*. One row per item, written so a planner can
schedule without opening the area file. Every row names the file and the fix.

| # | ID | area | sev · effort | why it matters, and what to do |
|---|---|---|---|---|
| 1 | **AGENT-020** | 02 | ~~**crit**~~ **low** · S | **⚠ REFUTED AS RANKED, 2026-08-13 — this row's premise was measured and is false; the ranking below it is stale.** Typing during a live stream queued and delivered the message **5/5 times**, including four attempts timed at the settle boundary (`REPRO-LOG.md`). The code path is real and unchanged at HEAD, but the TUI's steering path does not enter the drain-before-latch window; the loss is reachable only through `AGENT-030`'s sub-millisecond race and was not observed. Severity lowered critical → low. **Whoever next re-ranks this ledger must promote a different item into position 1.** Original text follows. ~~**Silently destroys a user-typed steering message.**~~ `Agent::continue_run` (`agent.rs:1637`) drains the steering queue at `:1646` and the follow-up queue at `:1650` **before** `start_run` (`:1659`) claims the latch at `:1672-1682`; on `Err(RunActive)` at `:1681` the drained `Vec<AgentMessage>` is dropped with no error, log or retry. pi guards first — `agent.ts:351-353` **@v0.83.0** throws before the drains at `:361`/`:367` (`:362-364` is the v0.84.1 offset; the code is byte-identical, the line numbers are not). **Fix:** hoist the `is_running()` check to the top as a fast path, AND — the load-bearing half, since the fast path is racy in Rust where pi gets atomicity from single-threaded JS — capture each drained vec and restore it via a new `PendingQueue::push_front` (`queue.rs`) before propagating. Test that fails today: `steer('keep-me')`, hold the latch, `continue_run()`, assert `Err(RunActive)` **and** `has_queued_messages()`. |
| 2 | **TUI-042** | 07 | **crit** · S | **Undo silently sends the literal marker text to the model instead of the pasted content.** `Snapshot` (`editor.rs:71-78`) carries `lines`/`row`/`col` and **not** `pastes`; backspace (`:814`) and delete (`:852`) erase the registry entry after the snapshot is pushed, so `undo()` (`:748-756`) restores the visible `[paste #N +42 lines]` text while `pastes[N]` stays gone, `marker_at` (`:663-694`, ends `self.pastes.get(&id)?`) no longer matches, and Enter ships ~20 characters of marker. **Fix:** add `pastes: BTreeMap<u32,String>` + `paste_counter` to `Snapshot`, clone in `snapshot()` (`:716-719`), restore in `undo()`; carry the same on `history_draft` (`:93`, `:1199`, `:1218`), which reuses `Snapshot`. Upstream `editor.ts:216-220` + `:2012-2030`, byte-identical at both tags **at the same line numbers** (checked). |
| 3 | **TUI-043** | 07 | **crit** · S | **One Ctrl+W after a large paste drops the paste.** `word_left_target`/`word_right_target` (`editor.rs:1074-1128`) classify only via `is_word_char` (`:1637-1639`) and never call `marker_covering` (`:697-712`), so Ctrl+W at the end of `[paste #1 +42 lines]` deletes the single `]`, the marker stops matching, and Enter sends the 19-char fragment. **Fix:** port `findWordBackward`/`findWordForward`'s `isAtomic` branches (`word-navigation.ts:44-46`, `:97-99`, present at v0.83.0, whose `isAtomicSegment` declaration at `:9-14` carries pi's own paste-marker comment) and make `delete_word_backward`/`delete_word_forward` (`:874-892`) drop the registry entry the way `backspace()` does at `:814`. Ship with #2 and `TUI-044`. |
| 4 | **PERM-009** | 10 | **crit** · S | **A configured `tools.bash: deny` is defeated and the allow-listed command executes.** `extension.rs:1651-1653` adds a cyrup-only `bash` bypass to `should_expose_tool`; upstream's `shouldExposeTool` has a read/skills bypass and nothing else at **both** `index.ts:2049-2075` @v0.7.1 (the ported baseline, so this is an in-baseline parity bug, not drift) and `:1790-1816` @v0.8.0. Because `manager.rs:205-215` resolves a bash **command** rule above the tool-level state — its own comment says "command rules OUTRANK the tool-level bash fallback" — `tools.bash: deny` + `bash: {"git status": allow}` leaves bash advertised **and** runs it. **Fix:** delete `:1651-1653` and the justification comment at `:1624-1631`; refresh the citation. No test pins the divergence (`tests/context_hygiene.rs:128-152` denies `write`), so the suite goes green on the deletion. |
| 5 | **TUI-027** | 07 | **crit** · M | **A mistyped key writes persisted user data.** `/tree` has no text search, so `z`/`x`/`e`/`t` are bound to characters pi types *into* that search; `e` opens the inline label editor, which captures every key, and Enter emits `SelectorOutcome::Apply(entry_id + FIELD_SEP + label)` (`tree_selector.rs:540-546`) → `app/selectors.rs:201-208` → `app/execute.rs:288-298` → `host_services.set_label` → `manager.append_label` — the same live path an extension's `setLabel` uses, into the session JSONL. **Fix:** add `search_query` to `TreeSelector`, accumulate printable non-control chars in the fall-through arm (replacing the digit-filter at `:867-873`), filter, clear on Cancel, pop on backspace (`tree-selector.ts:1079-1100`); rebind `TreeKeymap::default` (`keymap.rs:908-915`) from `z/x/e/t` to alt+left / alt+right / shift+l / shift+t; add the seven `app.tree.filter.*` ids to `TreeAction::from_id` (`:887-895`). Depends on `CFG-048`. |
| 6 | ~~**EXT-054**~~ **FIXED 2026-08-13** | 06 | **crit** · M | **Every WASM guest gets the full host surface; the documented per-extension sandbox is inert.** `load_discovered` (`facade.rs:1166-1184`) holds `disc.manifest` and calls `load_wasm(id, &bytes, services)` — whose signature (`facade.rs:1063-1070`) has no manifest parameter — so `capabilities.{fs,exec,net,ui}` provably cannot reach instantiation and narrows nothing. Gated only by the coarse `origin.is_pre_trust() \|\| project_trusted` check. **Fix:** take `&ExtensionManifest` (or a resolved `Capabilities`) in `load_wasm`, pass `disc.manifest`, seed `ProcCaps`/`HttpCaps`/`FsCaps` in `GuestState` construction from the grant instead of `Default`, and make the exec/net/ui host imports in `host/live.rs` return a typed denial when the bit is false. Deny-by-default: the loader's two `capabilities: Default::default()` synthesis sites (`loader.rs:213`, `:259`) must stay the EMPTY grant. Ships with `EXT-055`. **Blast radius today is zero shipping guests — which is the argument for doing it *before* the first third-party component, not after.** **FIXED 2026-08-13 with EXT-055** — `load_discovered` now calls `load_wasm_with_caps(.., &disc.manifest.capabilities)` (`facade.rs:1223`); the grant crosses as data into `GuestState::with_capabilities` and is enforced host-side at the `exec`/`proc`/`http-client`/`ui` import boundary (`host/live.rs:53-88`) plus `FsCaps` for `ext-fs`. Both `loader.rs` synthesis sites are now the explicit `Capabilities::none()`. `load_wasm` keeps its signature (manifest-less host-internal entry, `Capabilities::host_granted()`), so no caller changed. Evidence: `crates/cyrup-ext/tests/manifest_capabilities.rs`, 9 tests, 5 RED before the one-line revert. **Residual filed:** `AgentSession::load_wasm_extension` (`cyrup-session-svc/src/session.rs:3956-3962`) is still a full-authority manifest-less load reachable as public API — outside this pass's file ownership. |
| 7 | **AGENT-030** | 02 | high · M | **Two concurrent runs on one session.** `AgentSession::prompt` (`session.rs:627`) and `prepare` (`:854`) gate on `is_streaming()` → the agent's **per-run** flag, which `SettlementGuard::drop` clears at `cyrup-agent/src/agent.rs:1441` the moment each run settles — so a prompt landing in the post-run gap (auto-retry, auto-compaction, queued continuation) starts a SECOND run where pi queues it as steering. The session already owns the right latch (`driver_tx`, set at `:686`, dropped after the post-run loop at `:739`) and consults it only in `is_idle()` (`:601-603`). pi's `_isAgentRunActive` spans `_handlePostAgentRun` and every `agent.continue()` (`agent-session.ts:1062`/`:582`/`:876-877`/`:1159` @v0.83.0). **Fix:** add `is_run_active()` reading `driver_tx`, switch `:627` and `:854` to it, route post-run-gap submissions to `queue_steer`/`queue_follow_up`. **Must land in the same change as #1** or the loss just moves to the other branch. |
| 8 | **PERM-023** | 10 | high · S | **The gate never attaches, so configured denies are inert.** `is_installed` (`extension.rs:2159-2175`) probes env var, policy file and `config.json` and nothing else, while `manager_paths_for` (`:390-401`) wires `agents_dir` and `manager.rs:500-503` loads `<agents_dir>/<agent>.md` as an **enforced** policy layer. An operator whose only artifact is a persona's `permission:` frontmatter gets `is_installed == false`, no extension attached, and silently inert deny rules. **Fix:** return true when `<agent_dir>/agents/` or `<cwd>/.cyrup/agent/agents/` exists and is non-empty. Neither directory is ever written by this crate, so it carries none of the self-footprint hazard that produced `PERM-002`. Verify serialized on `ext_config::env_lock()`. |
| 9 | **TOOL-039** | 04 | high · S | **`CYRUP_SHELL` silently redirects every model-issued `bash` call to an arbitrary interpreter.** `ops/shell.rs:101-105` makes it the FIRST arm of `ShellConfig::detect()`, ahead of the `/bin/bash` probe (`:108-110`), `which_bash` (`:111-113`) and the `sh` fallback (`:114-118`); `detect()` is the default path for both `ToolRegistry::with_builtins` (`registry.rs:54`) and `Backend::default()` (`ops/mod.rs:359-361`); `session_env_scrub_keys()` (`config.rs:41-48`) is built from `SESSION_ENV_SUFFIXES` (`config.rs:31-32`) crossed with `CYRUP_`/`PI_`, so it **structurally cannot** contain `CYRUP_SHELL`, and a subagent run is a real re-exec; nothing records the resolved interpreter anywhere. The value goes straight to `get_bash_shell_config`, so the substitute need not be a shell. pi's `getShellConfig` (`utils/shell.ts:67-120`, byte-identical at both tags) reads **no** env var as a shell selector. **Decide in ONE change with `TOOL-007`:** (i) delete the arm and require the `shellPath` setting — three lines, pi's shape, recommended; or (ii) keep it and do **all four** of stamp a `[CYRUP-DELTA]`, report the resolved interpreter at session start and in bash result details, add a second explicitly-named scrub group (it does not fit the `{CYRUP,PI}_<SUFFIX>` shape), and validate the path per `shell.ts:73`. Half of (ii) is not an option. |
| 10 | **SEAM-065** | 08 | high · M | **Trust is resolved pre-launch, inverting pi's tier order, so the extension `project_trust` hook is skipped.** `main.rs:325-329` calls `resolve_startup_ui` before any runtime exists; `main.rs:1142-1162` resolves trust from store + default policy, prompts, and sets `config.trust_override` (`:1159`) — which short-circuits `builder.rs:495-499` (`if cfg.trust_override.is_none() && has_resources`), so `pre_trust_extension_verdict` never runs and the hook only gets a say when the user **cancels**. pi orders the extension tier above the store, the default policy and the prompt (`project-trust.ts:46-95` @v0.83.0, identical at v0.84.1). **Fix:** delete the trust block from `resolve_startup_ui` and give `SessionServiceBuilder` a `with_trust_prompt` callback invoked only on `TrustOutcome::NeedsPrompt`. Also retires `builder.rs`'s `saved: None` and its "no trust store is wired" warning. Latent until someone ships a trust-policy extension — and silent when it fires. |
| 11 | **SEAM-064** | 08 | high · S | **A user cannot answer a security prompt without recording a permanent verdict.** `main.rs:1155` passes `trust_options(&dirs.cwd, false)`; the flag gates both "(this session only)" rows (`trust.rs:356-363`, `:370-377`), so the startup prompt renders three options, every one with a non-empty `updates`, and `run_trust_prompt` persists them unconditionally (`startup_ui.rs:266-268`) — including a permanent lockout. pi's **pre-launch** path passes `includeSessionOnly: true` (`project-trust.ts:32`) while its in-app selector does not, so cyrup's other call site (`session.rs:3255`) is correct and must be left alone. **Fix:** one production character — `true`. Update `startup_ui.rs:504-537` to assert the five-option order and that a session-only index yields empty `updates`. |
| 12 | **SEAM-062** | 08 | high · S | **The pre-launch `--resume` picker invites a rename, accepts it, repaints the row with the typed name, and drops it.** `run_resume_picker`'s `on_apply` (`startup_ui.rs:129-138`) matches only `Delete`; the rename payload falls through. pi disables rename entirely on this surface (`session-picker.ts:48` passes `showRenameHint:false` and no `renameSession` callback, so `canRename` is false and the handler bails). **Minimum fix:** `set_show_rename_hint(false)` plus a new `SessionSelector::set_rename_enabled(bool)` gating `SessionAction::Rename`. **Preferred:** handle the outcome by opening the target and appending `session_info`, reusing `session.rs:3355-3365`. Same class as #5 — typed text accepted, echoed, discarded. Verify by relaunching after a rename **in a real terminal**. |
| 13 | **SEAM-063** | 08 | high · M | **Session delete permanently unlinks where pi routes through `trash`, and the failure is swallowed.** `rg -ni trash crates/` returns zero; both `startup_ui.rs:133-137` and `cyrup-session-svc/src/session.rs:3343-3347` bare-unlink, and the startup site discards the `io::Result` so a failed delete still reports success. **Fix:** one `delete_session_file(path) -> Result<DeleteMethod, String>` helper — spawn `trash` first with pi's `["--", path]` guard, success on exit-0 **or** the file having vanished, else `std::fs::remove_file` — called from both sites, propagating the method so the `C::DeleteSession` arm (`app/execute_session.rs:15-33`) can say "moved to trash" vs "deleted". Verify with a stub `trash` on PATH for all three arms, then a live run. |
| ~~14~~ | ~~**SEAM-061**~~ **CLOSED 2026-08-14 — REFUTED (sweep 6)** | 08 | ~~high · M~~ | **The `--resume` picker lists every project's sessions under a header that says "Current Folder", with a `tab scope` hint that does nothing.** `gather_session_infos` (`main.rs:1259-1268`) concatenates the cwd listing and the cross-project listing into one vector; `run_resume_picker` hands it to a `SessionSelector` defaulting to `scope=Current`, so the cwd column is off and the advertised toggle is inert — no `SessionAction::ToggleScope` exists. **Fix:** take pi's two loaders separately, add `ToggleScope` bound to Tab, flip `show_path` with the scope (pi's `showCwd`), make the hint conditional, thread `SessionListProgress`. Both halves must land together or the screen keeps lying. Verify with two project dirs **in a real terminal**. |
| 15 | **SESS-040** | 03 | high · M | **A shipped control that bills tokens and rewrites the session file does nothing, and the UI advertises it.** The indicator band renders "(esc to cancel)" at `app/render.rs:86-90`, but the `CompactionStart` arm (`app/events_fold.rs:195-223`) handled it by setting `IndicatorKind::Compaction` and nothing else — no `defaultEditor.onEscape` equivalent is installed; `rg AbortCompaction crates/` returns only the enum variant (`command.rs:32`) and its handler (`:116-118`), **no caller**, and `AgentSession::abort_compaction` (`session.rs:1677-1681`) has no production caller either. pi rebinds Escape on every `compaction_start` (`interactive-mode.ts:3074-3085` @v0.83.0) and restores it at `:3088-3095`. **Fix:** save and replace the default-editor Escape handler on `CompactionStart`, restore in the `CompactionEnd` arm, route through `command.rs:116-118`. `SESS-041` (auto-compaction still uncancellable) and `SESS-042` (a cancelled compaction is still written) are latent **only** because this has no caller — all three ship together. Verification must include a live terminal run.  **STRUCK 2026-08-15 (batch B, area 03): CLOSED as REFUTED — the Escape dispatch landed in `380c713`.** `rg -n AbortCompaction crates/` returns a production dispatch at `cyrup-tui/src/app/input.rs:144-146` (an `Action::Interrupt` branch on `state.compacting`, ahead of the four-branch chain and behind the `branch_summary_in_flight` check) routed to `ctx.session.abort_compaction()` at `app/run_action.rs:53-54` (`session.rs:1900`), plus assertions at `cyrup-tui/src/tests/escape_chain.rs:233,244`; the band half (`TUI-055`) is pinned by `cyrup-tui/src/tests/compaction_status.rs`. **The row was already false when it was last counted — see `03-cyrup-session.md`.** |
| 16 | **TUI-031** | 07 | high · M | **A turn is assembled from a context that is being rewritten under it.** A prompt typed during compaction dispatches immediately: the `AppAction::Submit` arm (`app/run_action.rs:83-103`) branches on `is_streaming()` only and never consults `is_compacting()`, and `AgentSession::prepare` (`session.rs:849-900`) has no compaction guard either. pi checks compaction **first** (`interactive-mode.ts:3023-3033`). **Fix:** check `session.is_compacting()` before `is_streaming()`, push onto a new `AppState::compaction_queue`, clear the editor, push pi's `Queued message for after compaction` status, suppress the optimistic echo `dispatch_submission` does, drain on `CompactionComplete`. The session-layer serialization is area 03's to own; note `TUI-016` means there is currently no surface that would show a queued message.  **STRUCK 2026-08-19: CLOSED 2026-08-14 (area 07), and the Fix landed as written.** `AppState::compaction_queue` is `app/state.rs:85` holding `CompactionQueued` (`app/outcome.rs:4-6`); the guarded arm is `app/run_action.rs:68-82`, ABOVE the streaming arm, with pi's identical follow-up gate at `:116-117`; `queue_compaction_message` pushes at `app/submit.rs:195-196`; the drain is `take_compaction_queue` in `app/events.rs:63` (and `app/channels.rs:87`); the optimistic echo is gone — `app/submit.rs:22-28` records that `dispatch_submission` "used to `push_user` unconditionally right here". `TUI-016` closed too, so the queue does now have a surface. |
| 17 | **SEAM-051** | 08 | high · S | **The DEFAULT value of a v0.84.1 flag makes the binary refuse to start.** `rg tui_mode crates/*.rs` returns nothing; `--tui-mode` is absent from `KNOWN_LONG_FLAGS` (`cli.rs:757-799`), so `partition_extension_flags` (`:701-753`) captures `--tui-mode regular` as an extension flag (the value does not start with `-`/`@`), `report_runtime_diagnostics` returns fatal, and all three modes return `Ok(1)` (`main.rs:514-517`, `:662-666`, `:770-774`). No pi command line or wrapper script can launch cyrup. **Fix:** add the flag to `KNOWN_LONG_FLAGS` + `KNOWN_VALUE_LONG_FLAGS`, add a `TuiMode {Regular, Fullscreen}` value-enum to `Cli`, add pi's two error diagnostics to `apply_arg_leniency` (`diagnostics.rs:90-152`), add the help line. Accepting `regular` as a no-op and rejecting `fullscreen` with an explicit not-supported message is a legitimate interim; the rendering half is `TUI-019`/`OQ-07-1` and **must not block this**. |
| 18 | **PROV-027** | 01 | high · S | **All 9 GitHub Copilot rows on the anthropic-messages route arrive unauthenticated.** `anthropic_messages.rs:470-536` `build_headers` has no provider branch; the scheme comes solely from `is_oauth = api_key.contains("sk-ant-oat")` (`:434-437`), which a Copilot `tid=…;exp=…` token never matches, so `:524-531` emits `x-api-key`. pi branches on `model.provider === "github-copilot"` **before** the OAuth test (`anthropic-messages.ts:867-888`, verified clean at both tags this pass) and sends `Authorization: Bearer` with only the selective betas, deliberately without the Claude-Code identity headers. **Fix:** test the provider first and take the bearer path; the betas half is already correct. **One edit with #19.** |
| 19 | **PROV-028** | 01 | high · S | **`github-copilot-headers.ts` is entirely unported** — no `X-Initiator`, `Openai-Intent` or `Copilot-Vision-Request` on any of the three routes. Without `Copilot-Vision-Request` an image turn against Copilot is **rejected** (loud, normal path); without `X-Initiator` every agent-loop request is silently misreported for quota. **Fix:** port as `api/github_copilot_headers.rs` (three pure fns over `&[Message]`) and apply at `anthropic_messages.rs:470`, `openai_completions.rs` and `openai_responses.rs:412`, guarded on the provider, ordered after `model.headers` and before `opts.headers`. Upstream **@v0.83.0**: `anthropic-messages.ts:867-871`, `openai-completions.ts:638-645`, `openai-responses.ts:223-230` — the previously recorded `:646-652` was the v0.84.1 offset, corrected this pass. Needs the exact provider guard #18 introduces, so **doing them separately is strictly more work.** |
| 20 | **PROV-048** | 01 | high · S | **A lone-surrogate `\uXXXX` escape in a provider SSE frame kills the entire assistant turn.** `serde_json` rejects it, `repair_json` re-emits it verbatim (`json_parse.rs:67-75`) so `repaired == json` and `parse_json_with_repair` returns `None`, and both SSE callers treat `None` as fatal (`anthropic_messages.rs:1439-1449`, `google_generative_ai.rs:975-985`). pi's `JSON.parse` accepts it and `sanitizeSurrogates` strips it on the way out. The same weakness breaks resuming a pi-written session JSONL. **Fix:** in `repair_json`'s `Some('u')` valid-hex arm, drop an escape decoding to an unpaired surrogate (D800-DBFF not followed by DC00-DFFF, or DC00-DFFF not preceded) so `repaired != json` and the retry succeeds. Ships with `PROV-049` + `PROV-050`. |
| 21 | **PROV-029** | 01 | high · S | **Copilot and Codex login flows are fully ported and unreachable.** Each provider has a runtime half (refresh/`to_auth` only) and a login-capable flow half, and `ProviderAuth` wires the **runtime** half (`providers/github_copilot.rs:142-146`, `openai_codex.rs:129-131`), so `/login` — which resolves via `provider.provider_auth().oauth` (`cyrup-config/src/login.rs:784`) — dead-ends on the `LoginUnsupported` default at `auth/mod.rs:124-131`. Both render with the subscription marker (`github_copilot.rs:597`, `openai_codex.rs:451`). **Fix:** one field assignment per provider — add the two arms to `providers/builtin_oauth.rs:37` and delete the prose exemption at `:14-16`. Separately: either populate the flow registry (`auth/oauth/load.rs:111`, zero production callers) or delete it. |
| 22 | **SUBA-043** | 09 | high · S | **A caller's `outputSchema` is dropped without error and the run returns prose.** `subagent_tool_parameters()` (`extension.rs:6543-6690`) emits 45 properties and `outputSchema` is not among them — it exists only on the `tasks[]`/`chain[]` item schemas — and both single-run sites pin `structured_output_schema: None` (`:1934` foreground, `:2295` async). The root schema is `additionalProperties: true` and `SubagentToolParams` has no `deny_unknown_fields`, so the call is accepted. The capture mechanism `SUBA-S01` was closed to deliver is unreachable from the surface a model calls. Upstream has it top-level at `schemas.ts:349` @v0.43.0. **Fix:** add the property with the existing `sj_json_schema_object()` helper, deserialize onto `SubagentToolParams`, thread into both constructors — the runner already carries the field. Land the advertise-vs-consume guard test with it. |
| 23 | **SUBA-014** | 09 | high · S | **A skill-carrying agent is told to use a `read` tool it does not have.** `exec/mod.rs:1463-1491` builds the child's builtin allowlist verbatim with no head-injection; `rg require_read_tool` = 0. Meanwhile cyrup's own proactive-skill block (`discovery/skills.rs:273`) instructs the child to "use the read tool to load a skill's file", so an agent with an explicit `tools:` list omitting `read` plus any resolved skill silently cannot load it — and the failure surfaces as a model apology, not a config error. pi injects under an exact three-way condition (`pi-args.ts:355-372` @v0.43.0, seven live setters, all deriving it from `Boolean(resolvedSkills.length)`). **Fix:** compute `require_read_tool` from `!resolved_skills.is_empty()` (already in scope at the skill-resolution site) and inject `read` at the head. |
| 24 | **PROV-047** | 01 | high · M | **The `httpProxy` setting reaches only the streaming wire APIs.** `builder.rs:229-239` turns it into a `ProviderEnv` overlay read solely by `sse.rs:181-192` `build_client_for_target`; every other egress path uses `build_client()` (`sse.rs:140-144`), which has no proxy handling — five OAuth flows (`auth/oauth/{anthropic:443, openai_codex:552, xai:525, openrouter:372, radius:468}`), `cyrup-agent/src/proxy.rs:455` and `cyrup-provider/src/wire.rs:472` — while `cyrup-ext/src/caps/http.rs:599` is a bare `reqwest` builder with reqwest's own competing env detection. pi proxies **every** fetch via a process-global undici dispatcher (`http-dispatcher.ts:43-48`/`:79-103` @v0.83.0). **Fix:** add `configure_http_proxy()` beside `configure_http_idle_timeout`, set it at `builder.rs:1200`, turn `build_client()` into `build_client_for(target_url)` running the already-ported resolver, thread the URL through the seven call sites, add the resolver + `.no_proxy()` to `caps/http.rs`. |
| 25 | **CFG-035** | 05 | high · M | **The trust gate asks the user to trust a file cyrup will never read.** `has_trust_requiring_resources` (`trust.rs:194`, `:203-204`) prompts *because* `.cyrup/SYSTEM.md` exists, and nothing ever discovers it — the CLI flags are the only producers of the two override fields. **Fix:** add a discovery step in `builder.rs` (~`:1045-1060`) mirroring `resource-loader.ts:1022-1048` — `custom_prompt` = CLI `--system-prompt` else `<cwd>/.cyrup/SYSTEM.md` when project-trusted else `<agent_dir>/SYSTEM.md`; `append_system_prompt` = CLI (which **replaces**, per pi's `??`) else the single discovered `APPEND_SYSTEM.md` under the same trust rule; route the discovered path through `resolve_prompt_input` (`cli.rs:456`). Also correct `prompt/overrides.rs:15-16`, which documents accumulation where pi picks exactly one. |
| 26 | **SEAM-047** | 08 | high · M | **`cyrup --mode rpc` cannot be stopped by a supervisor.** `signals.rs:88-101`'s first-signal body is `session.abort(); cancel.cancel();`; the 130/143/129 codes it computes are used only on the **second** delivery at `:98-99`, and the token it fires is `main.rs:367`'s TUI input token, observed by the interactive arm only. `rpc_driver`'s `select!` (`rpc.rs:717-842`) has no cancellation arm, so `runtime.dispose()` (`run.rs:113`) never runs and no `session_shutdown` reaches extensions. pi does all of it on the first delivery (`rpc-mode.ts:366-383` → `shutdown` `:724-741`; `print-mode.ts:50-66` the same shape). **Fix:** publish the `ShutdownSignal` on a watch/oneshot, add a `cancel.cancelled()` arm to `rpc_driver` that sets `reader_open=false` so the drain-and-break at `:851` runs, add a between-message check to print/json, return 143/129 from `run.rs:22`/`:50`/`:101` **after** `runtime.dispose()`. **Ships with `SEAM-059`** (which rewrites the same function: pass `Arc<AgentSessionRuntime>` so the handler aborts the CURRENT session, not the one live at startup), `SEAM-008`, and `DRIFT-049`. |
| 27 | **DRIFT-049** | 12 | high · M | **Same defect as #26, filed in area 12** — `duplicate-of: SEAM-047`, raised medium → high this pass because a defect cannot carry two severities and a high that reads as medium falls off a planner's list. **Schedule once, in area 08.** The body is retained because it carries analysis area 08's item does not: `run_rpc` is parked on a stdin read no signal disturbs, and `cyrup-modes/src/run.rs:101-116` disposes only *after* `run_rpc` returns, so interactive/print/json survive only incidentally. It also records that `SEAM-S02` is stale — `signals.rs:97-100` does implement the repeat force-exit with pi's exact codes. |
| 28 | **PROV-030** | 01 | high · L | **A registered provider that cannot serve a single request.** `google-vertex` ships with 10 catalog models and full auth including the ADC arm, appears in `/model`, and has no wire API — every request terminates at `wire.rs:158-166` with `no API implementation for google-vertex`. This is the exact failure mode `PROV-005`'s own Fix text warned about, shipped by the sweep that closed `PROV-005`. **Fix:** port `pi/packages/ai/src/api/google-vertex.ts` as `api/google_vertex.rs` (factor the google-shared converters out of `google_generative_ai.rs` first), add `known_api::GOOGLE_VERTEX`, register the factory at `api/mod.rs:130-163`. S-sized mitigation if the port cannot land: refuse at construction time to push a provider whose catalog names an api the registry does not `contains()` (`api/mod.rs:116-119` already has the predicate). **MANDATORY in the same change:** rewrite the port-status table at `providers/all.rs:12-47`, which still calls amazon-bedrock / google-vertex / openai-codex "**pending** (NOT registered)" at `:12`/`:23`/`:34` and names all **four** at `:46-47` including github-copilot — contradicting its own table row at `:21` and flatly denying this item's premise. It is the first thing an engineer opening the file reads. Ranked last among the highs because it fails loudly and immediately. |

**Areas with zero open criticals or highs: 06 has one critical and no high; 11 and 04's tail are the
only genuinely quiet surfaces.** Area **11 (44 items, 0 high)** is quiet because its baseline was
only just corrected and its surface-driven axis has **never** been run — treat the zero as
un-swept, not clean (area 11 blind spot 10).

---

## Closure rates — the mechanism that produced 3.9% is still gone

> **SUPERSEDED 2026-08-14.** Two sweeps closed **290 rows** in one commit cycle. Whatever this
> section concluded about closure rate is answered: the mechanism that was missing was a
> whole-backlog sweep with per-item pi re-derivation, and it works. What it did *not* fix is
> verification — nothing below the gate was executed, and the analysis's own 12% error rate is now
> the binding constraint. See *The analysis's own error rate is now measured* at the top.

> The 2026-08-07 pass measured highs closing at 82% and mediums at **3.9%**, and correctly attributed
> the gap to *every commit being explicitly high-targeted*. That diagnosis was right and it is what
> predicted the change: **the rate moved when the commits stopped being item-targeted.**

**This pass: 117 items closed**, plus 4 retired by other means (`CFG-012` superseded, `SEAM-019`
superseded, `DRIFT-037` withdrawn, `DRIFT-038` → `CFG-018`). Entering open set ≈ 371.

| band | entering | outcome | rate |
|---|---|---|---|
| critical | 2 (`SEAM-021`, `SEAM-022`) | both closed | **100%** |
| high | ~17 | 14 retired, 3 carried (`PROV-027`/`028`/`029`) | **~82%** |
| medium | 178 (the 2026-08-07 audit's own figure) | ~52 closed | **~29%** |
| low | ~181 | ~50 closed | **~28%** |

The cause is structural and visible in the git log: the 64 commits in `1806375..04c1ba2` are
**area-targeted subsystem batches**, not item-targeted fixes — `8902b4f` (watchdog/missions/fleet,
39 files, ~28k lines), the ten `fix(tui): batch N` commits (`0aaca00`…`922d90c`, 103 files,
+38,826/−2,362, fifteen source files that did not exist at the old baseline), four sequential
pi-subagents batches, `0a5742d` (permission gate → v0.8.0) and `911dd59` (intercom → v0.9.2).
**Zero areas came back 100% open**; the previous pass had seven.

**The counter-fact a planner must hold alongside it, and it got worse this edition: 117 closed
against 207 filed.** 371 − 117 − 4 + 207 = 457 rows (448 counted + 9 trackers). The 31 items the
repair pass added came from four upstream surfaces — `packages/ai/utils/`, `packages/tui/src`'s
non-drawing files, `packages/coding-agent/src/cli/`, and `migrations.ts` + `core/keybindings.ts` —
that **no file in this directory had ever named**, and they yielded 2 criticals and 8 highs. That is
structural defect C behaving exactly as the README predicts. It is not evidence the work is going
backwards; it is evidence the count was never a measure of remaining work.

**The severity trend, which is the measure, is now honest and is worse than last edition claimed:**
2 criticals → 0 (real closures) → **6** (correct application of the scale). Four of the six existed
in the backlog before this pass, mis-rated.

---

## The medium/low picture — 197 medium + 223 low

> **SUPERSEDED 2026-08-14 — the current split is 75 medium + 95 low.** The clustering below is still
> the right way to read that population and was not re-derived; treat the cluster membership as a map
> and the counts as dead.

### Item kinds, re-derived from the twelve open tables

| kind | n | change vs the first 2026-08-12 edition | note |
|---|---|---|---|
| `parity-bug` | 176 | 153 → 176 | ported, then drifted — still the largest bucket by a wide margin |
| `not-ported` | 146 | 135 → 146 | predates the baseline, never built. **+11 net, but the real move is +4 reclassified INTO it from `upstream-drift`** |
| `upstream-drift` | 66 | 75 → 66 | **−9**, almost entirely area 12's kind corrections — see *Version lag* |
| `test-defect` | 23 | 23 → 23 | unchanged; still a coherent one-sweep target |
| `cyrup-original` | 21 | 21 → 21 | no upstream basis — includes the whole "advertised but inert" class |
| `stale-port` | 14 | 13 → 14 | carries behaviour upstream changed or deleted |
| `tooling` / `port-divergence` | 2 | 3 → 2 | `PROV-004` left the count as a tracker |
| `tracking` | **0** | 3 → 0 | **every `tracking`-kind item is now a tracker and outside the count** — which is what the kind always meant |

### The clusters — 420 medium+low items collapse to a small number of moves

**F1 — "advertised but inert": at least 31 ids across ten areas, one mechanism, mechanically
detectable.** Code that exists, compiles, has tests, and has **zero production callers** — or a
control the UI advertises and no code implements. This is the dominant recurring shape of the whole
analysis and it grew again this pass.

> `EXT-054` (**critical** — manifest capabilities never read) · `EXT-055` (`FsCaps::with_fs_root`,
> zero callers) · `EXT-025` · `EXT-013` · `SESS-040` (high — `AbortCompaction`, zero callers) ·
> `SESS-041` · `SESS-035` (docs-pointer section never emitted) · `SESS-033` · `PROV-029` (high —
> flow registry, zero production callers) · `PROV-032` · `PROV-040` · `SUBA-043` (high) · `SUBA-047`
> · `SUBA-054` · `SUBA-046` · `SUBA-059` · `SUBA-049` · `TUI-014` / `TUI-033` (widgets, header,
> footer stored where nothing reads them) · `TUI-044` (`Snapshot::col` written and never read) ·
> `TUI-051` (`/reload` claims to re-read `keybindings.json` in both its help text and its own source
> comment; `load_keybindings_json` has exactly one non-test caller, `main.rs:1626`, at boot) ·
> `CFG-045` · `CFG-015` · `CFG-044` · `CFG-006` · `AGENT-031` · `PERM-014` · `PERM-027` · `ICOM-041`
> · `SEAM-061` (a `tab scope` hint with no `SessionAction`) · `SEAM-062` (a rename accepted and
> discarded).

  A **schema/dispatch drift test** plus a **"no production caller" lint** would have caught most of
  these before they were written. That was suggested-order item 0 in the previous two editions, it
  has still not been built, and the class has grown every pass. **It remains the highest
  leverage-per-hour move in the backlog.**

**F2 — one WIT/ABI bump unlocks 27 ids.** The world is at `cyrup:ext@0.4.0` with a byte-identity test
tying both copies and a compile-time `ABI_FINGERPRINT`. The next bump is `0.5.0`, and `EXT-028`'s
contract says *any* export change bumps the minor — so doing these separately means twenty-seven
minor bumps and twenty-seven guest-refusal cliffs.

> `EXT-009` (+ its provider twin `PROV-042`) · `EXT-014` · `EXT-015` · `EXT-016` · `EXT-021` ·
> `EXT-023` · `EXT-024` · `EXT-035` · `EXT-037` · `EXT-040` · `EXT-042` · `EXT-043` · `EXT-044` ·
> `EXT-045` · `EXT-046` · `EXT-047` · `EXT-048` · `EXT-049` · `EXT-S04` · `SEAM-011` · `SEAM-012` ·
> `SEAM-025` · `TOOL-015` · `TOOL-016` · `TOOL-021` · `TOOL-022`.
>
> **The host-side prerequisite is unchanged and still unbuilt:** `cyrup-core/src/tool.rs::prompt_guidelines(&self) -> &[&str]`
> must return owned strings before `impl Tool for WasmTool` can ever carry guidelines. That is
> `TOOL-021` (medium, `S`). Land it in the same move.

**F3 — the test-defect sweep: 23 ids, one pass.** The suite is at zero failures, so any new failure
is signal. **Deduplicate `AGENT-019` and `DRIFT-039` first — they are literally the same test**
(`crates/cyrup-agent/src/tests/agent_loop.rs:327`); `DRIFT-039`'s body carries the better fix sketch (the
`agent-loop.test.ts:589-612` rendezvous), so fold that into `AGENT-019` rather than working both.
Members: `PROV-038`, `AGENT-019`, `SESS-032`, `TOOL-020`, `TOOL-024`, `TOOL-025`, `TOOL-026`,
`TOOL-030`, `EXT-032`, `TUI-N08`, `TUI-N09`, `SEAM-028`, `SEAM-030`, `SUBA-032`, `SUBA-033`,
`PERM-020`, `PERM-021`, `PERM-022`, `ICOM-025`, `ICOM-026`, `DRIFT-035`, `DRIFT-036`, `DRIFT-039`.

**F4 — 50 counted ids cover 21 defects; 28 of them are excess.** A planner scheduling by area will
book the same work twice. Area-12 pairings are now machine-readable (`duplicate-of:` in the row, the
status row and the body's `Kind` line) and account for 15 of the 28; the areas 01–11 pairings are
editorial and account for the other 13. Both are subtracted in *The open set*. Where a row lists two
owners, the ids are **separable halves**, not true duplicates — schedule both, but as one move.

| one defect | ids | owner |
|---|---|---|
| constrained sampling | `PROV-011` · `TOOL-016` · `EXT-024` · `DRIFT-018` | `PROV-011` |
| TUI / alt-screen mode | `SEAM-051` · `TUI-019` · `CFG-021` · `DRIFT-022` *(tracker)* | `SEAM-051` (flag), `TUI-019` (renderer), `CFG-021` (settings key) |
| signal teardown | `SEAM-047` · `SEAM-008` · `SEAM-059` · `DRIFT-049` | `SEAM-047` |
| cache-miss notices | `PROV-035` · `CFG-014` · `TUI-021` | `PROV-035` |
| markdown transformers | `EXT-019` · `TUI-034` · `DRIFT-015` | `EXT-019` |
| `sessionAffinityFormat` | `PROV-024` · `PROV-033` · `DRIFT-020` | `PROV-024` |
| `Current date:` footer | `SESS-019` · `DRIFT-016` · `DRIFT-035` | `SESS-019` |
| `CompactionResult.usage` | `SESS-030` · `SEAM-034` | `SESS-030` |
| `websocketConnectTimeoutMs` | `CFG-006` · `AGENT-031` | `CFG-006` |
| `labelTimestamp` on tree nodes | `SESS-S05` · `SEAM-060` | `SESS-S05` |
| parallel-tool test | `AGENT-019` · `DRIFT-039` | `AGENT-019` — *the same test* |
| Windows shell paths | `TOOL-036` · `DRIFT-046` | `TOOL-036` — **found this pass, missing from the previous F4 table** |
| embedded catalog floor | `PROV-018` · `DRIFT-009` · `PROV-039` · `PROV-004` *(tracker)* | `PROV-018` (tooling half) + `PROV-039` (provenance half) — **pairing found this pass** |
| `ANTHROPIC_AUTH_TOKEN` · radius/qwen · `deferredToolsMode:"kimi"` · usage cost · AGENTS.md double-load · `${@:-default}` · `ModelRuntime` · llama.cpp | `PROV-021`·`DRIFT-030` / `PROV-014`·`DRIFT-019` / `PROV-025`·`DRIFT-027` / `PROV-036`·`DRIFT-031` / `SESS-013`·`DRIFT-024` / `CFG-017`·`DRIFT-025` / `CFG-020`·`DRIFT-023` *(tracker)* / `EXT-027`·`DRIFT-032` *(tracker)* | the non-`DRIFT` id in each pair |

  **Area 12 is partly a duplicate index, and the census is now exact: 20 of its 34 rows duplicate an
  item another area owns; 14 are uniquely its own** (8 medium — `DRIFT-041` HTML export, `DRIFT-048`,
  `DRIFT-004`, `DRIFT-013`, `DRIFT-014`, `DRIFT-028`, `DRIFT-029`, `DRIFT-033`; 6 low — `DRIFT-010`,
  `DRIFT-036`, `DRIFT-042`, `DRIFT-045` clipboard text, `DRIFT-050`, `DRIFT-051` process title).
  **A finding worth the planner's attention:** area 12 rated its duplicates *lower* than the owning
  area in six cases, so "area 12 has no highs" was partly a bookkeeping effect rather than a property
  of the surface. `DRIFT-049` was corrected medium → high for exactly that reason.

**F5 — the closed clusters, for the record.** `C2` (no HTTP timeout or retry budget) is closed;
residual `PROV-043` (bedrock alone has no retry). `C10` (the subagent second-hop config boundary) is
closed. Do not re-schedule either.

---

## Trackers — 10 ids that propose no work, excluded from every count

These keep their IDs, their rows and their full bodies, per the stable-id rule. **A planner should
not pick one up.** They are here so the backlog stops mixing work with bookkeeping — and so that
each one's escalation condition is written down instead of implied.

| ID | area | what it is | what escalates it back into the count |
|---|---|---|---|
| `PROV-004` | 01 | The five newest catalogs were never field-diffed. Its entire Fix is "this is `PROV-018`'s `xtask gen-catalogs` and nothing else — do not re-derive by hand". | Scheduling `PROV-018`. It schedules nothing `PROV-018` does not. |
| `AGENT-028` | 02 | pi's v0.84.1 agent-harness. Its own body says "filed as scope-defining, not as loop debt"; its Fix opens "Do not port speculatively. First decide whether cyrup models pi's harness at all." | A decision that cyrup models the harness. **Answer it together with `SESS-038` — both turn on the same question.** |
| `SESS-038` | 03 | `packages/session-backends/sqlite-node`, new at v0.84.1. Nothing in `coding-agent/src` or `agent/src` imports it. | Upstream wiring it into a shipped path, or the harness decision above. |
| `SEAM-058` | 08 | pi's experimental `server`/`client` tree + `packages/protocol`/`packages/client`. Fix: "track, do not build, until upstream wires it into `main()`". Re-checked: at v0.84.1 `git grep experimentalCli` matches only the file itself and its test. | **The moment pi's `main()` references `experimentalCli`.** Its Verify line is a re-diff at the next tag, not an implementation. |
| `SUBA-005` | 09 | 27 advertised management actions against upstream's 50 (v0.43.0) / 53 (v0.47.1). Its own Fix: "this item is the ledger, not the work." | It owes two things: owners for `worktree.discard`, `approve-checkpoint`, `reject-checkpoint`, `project.open`/`status`/`close`, `mission.resolve-decision`; and a completeness assertion pinning the enum against a checked-in copy of upstream's array. Filing those makes it redundant. |
| `DRIFT-022` | 12 | TUI mode / alternate screen. Fix: "Do **not** implement yet"; Verify: "n/a while tracking". `duplicate-of: SEAM-051`. | `OQ-07-1` being answered. The behavioural cost is already carried as work by `SEAM-051`, `CFG-021` and `TUI-019`. |
| `DRIFT-023` | 12 | `ModelRegistry` → `ModelRuntime`. **Also a LEAD — neither side was re-read, in this pass or the repair pass.** `duplicate-of: CFG-020`. | Someone spending the two-sided read. The area file records the exact commands. |
| `DRIFT-032` | 12 | llama.cpp router / HF model search. Fix: "Defer until DRIFT-019 and DRIFT-009 are settled". Kind corrected `upstream-drift` → `not-ported` this pass (all files exist at **v0.83.0**), confidence medium → high. `duplicate-of: EXT-027`. | `EXT-027` being scoped — upstream ships it as a bundled **extension**, which is why area 06 owns it. |
| `PERM-017` | 10 | **Re-classified 2026-08-14 (sweep 6)** — forwarding-root agent-dir env overrides. Its own Fix is "No action while the middle levels remain meaningless" and its Verify is "n/a until triggered", which is the Trackers contract; it was counted as open work and overstated area 10's remaining set by 20%. Re-derived at the tag: `permission-forwarding.ts:62-92` @v0.8.0 resolves five levels, the three middle ones all guarded by `options.isSubagent`, and the v0.7.1→v0.8.0 diff of that file is a pure `normalizeAgentName` extraction — no level added or removed. | cyrup grows a delegated or multi-auth runtime dir, i.e. the middle levels acquire a meaning. |
| `DRIFT-040` | 12 | pi's agent-harness v2 rearchitecture. Fix: "Do **not** port now". **Also a LEAD** — its three load-bearing claims (the `agent-harness.ts` rewrite, `docs/harness-v2.md`, the sqlite-node rebuild) are still carried forward unverified. `duplicate-of: PARITY-GAPS VL-P22`. | The same harness decision as `AGENT-028`. |

---

## Version lag — four upstreams

| upstream | cyrup ported baseline | latest tag | drift window | owner | result this pass |
|---|---|---|---|---|---|
| `pi` | **v0.83.0** | **v0.84.1** | 627 files, +52,291/−17,556 | 01–08 + 12 | swept per area; every area reports its own scoped diffstat |
| `pi-subagents` | v0.43.0 *(inferred; the crate records no version string)* | **v0.47.1** | 151 files, +10,254/−1,333 | 09 | **NEWLY ANALYSED** — 11 of area 09's 24 new items |
| `pi-permission-system` | v0.7.1 | **v0.8.0** | 28 files, +4,023/−1,851 | 10 | **fully absorbed — ZERO drift items.** Its two open items are in-baseline parity bugs, one of them the workspace's only permission bypass |
| `pi-intercom` | **v0.9.2** *(corrected — every prior doc said v0.7.0)* | **v0.10.1** | true window `v0.9.2..v0.10.1` = 24 files, +2,495/−700, 14 commits | 11 | **NEWLY ANALYSED** — 13 of area 11's 24 new items |

### The two ranges no prior pass had seen

**`pi-subagents` v0.43.0..v0.47.1.** The previous docs recorded "latest v0.43.0" — i.e. no gap at
all. The src-only sweep covered **96 non-merge commits, 67 files, +4,696/−769 and 12 net-new source
files, all 12 read**, 14 commits diffed line by line. It produced `SUBA-044`, `SUBA-050`…`SUBA-060`,
`SUBA-065`, `SUBA-066`. Highlights: upstream made the bundled `reviewer` lane **read-only** (cyrup
still grants it `bash`/`edit`/`write` — `SUBA-044`); upstream bounds every async **child** at 30
minutes (cyrup has no default at all — `SUBA-051`); `subagents.modelScope.strict` now hard-rejects
an out-of-scope inherited model rather than warning (`SUBA-050`). *Not filed by rule:*
`run-fanout-budget.ts` landed on `main` after v0.47.1 with no named tag to cite.

**`pi-intercom` v0.9.2..v0.10.1.** Two corrections at once — the prior docs recorded latest v0.9.2
(one version stale) *and* baseline v0.7.0 (two versions **too pessimistic**). A citation census over
`crates/cyrup-intercom/src` returns v0.9.2 ×272, v0.7.0 ×14, v0.8.0 ×3, v0.6.0 ×1 (the `lib.rs`
banner), v0.10.x ×0, and the load-bearing v0.8.0/v0.9.x code is present **and tested**. All 14
commits accounted for; it produced `ICOM-035`…`ICOM-047`. `ICOM-012` carries the banner fix.

### Two systematic corrections that change how lag should be measured

1. **Measuring against a floating upstream HEAD over-reports lag and under-reports port omissions.**
   Re-measuring against the *named ported tag* has now reclassified **twelve** items out of
   `upstream-drift`, and **zero** in the other direction. The first pass moved `PROV-021`,
   `PROV-023`, `PROV-024`, `PROV-025`, `SUBA-017`, `SUBA-021`, `SUBA-022`, `DRIFT-014`. The repair
   pass re-derived nine of area 12's commit-hash-only items and found **six more misclassified** —
   `DRIFT-016` (`git grep 'Current date' v0.83.0 -- packages/coding-agent/src` returns nothing → the
   removal predates the ported baseline, so cyrup carries deleted behaviour: `stale-port`);
   `DRIFT-018` (`constrained-sampling.ts`, 148 lines, 7 exports, exists at v0.83.0 and is already
   imported there by five wire APIs → `not-ported`); `DRIFT-019` (three of four provider files exist
   at v0.83.0 → `not-ported`); `DRIFT-030` (`env-api-keys.ts:29`/`:76`/`:147` + `providers/anthropic.ts:5`/`:21`
   all at v0.83.0 → `not-ported`); `DRIFT-031` (`usage-totals.ts:37` at v0.83.0 with a live consumer
   at `interactive-mode.ts:5665` → `not-ported`); `DRIFT-032` (the whole `extensions/llama/` tree at
   v0.83.0 → `not-ported`, and it belongs to `cyrup-ext`, not `cyrup-provider`, because upstream
   ships it as a bundled extension). **None of these will be swept up by a rebase.**
2. **Some drift runs backwards.** `PROV-033`: cyrup carries `sendSessionIdHeader`, which pi
   **deleted** in #6496 with a documented migration to `sessionAffinityFormat` — now a three-valued
   union (`types.ts:112`) a bool cannot express. `CFG-012`: pi **adopted cyrup's** recursive settings
   merge at v0.84.1, so "fixing" cyrup toward the retired v0.83.0 spread would be a regression — the
   item is superseded, not open. `CFG-034` / the TUI scrollbar tokens: cyrup anticipated a v0.84.1
   addition.

### Known limits of the version sweep

- **117 commits past the diffed tag are unanalysed everywhere.** pi HEAD is
  `v0.84.1-117-g581d75a89` (`581d75a89`, 2026-08-13) — the count has not moved since the first
  edition, but the window is now a day older. `pi-subagents` HEAD is `v0.47.1-14-g9e9fd13`: **14
  commits past its diffed tag**, previously recorded only as the one `run-fanout-budget.ts` file.
  Area 05 names one concrete item known to sit inside pi's window and deliberately not filed:
  `getExperimentalToolSampling()`'s constrained-sampling request on the four built-in tools.
- **`packages/agent/src/harness/**` is owned by NO area file.** ~11.4k insertions / ~10.9k deletions
  in this window — the `agent-harness.ts` rewrite, a new 667-line `reducer.ts`, a new `session/`
  subtree (jsonl codec / repo / storage / state) with a 993-line conformance suite, and a new typed
  telemetry layer. `AGENT-028`, `SESS-038`, `DRIFT-040` and `PARITY-GAPS` VL-P22 all point at it, and
  **all three of the first are now trackers precisely because none of them proposes work.** This
  needs a scope decision, not another item.
- **The embedded catalog gap is tractable and mis-recorded as unresolvable.** cyrup ships 35
  embedded catalogs against upstream's 39. pi gitignores only the generated **output**
  (`.gitignore:11` → `packages/ai/src/providers/data/`); the generator and its five npm scripts are
  committed at **both** tags. `PROV-018` (medium, tooling, `M`) is the real work — an `xtask
  gen-catalogs` plus the drift check whose absence is the reason nobody noticed. `DRIFT-009` now
  defers to it, with the argument recorded for why seeding from the published pi.dev artifact is
  strictly lossier: the artifact is the *published* catalog, so it cannot reproduce what
  `generate-models.ts` computes from `models-dev-reasoning-options.ts` or from the per-provider
  compat overrides, and it yields no reproducible build step.
- **Two sibling docs must be corrected in the same edit as this one, and this time the claim is
  checked:** `PARITY-GAPS.md` §0 publishes the now-superseded census ("**426 open items: 0 critical,
  14 high, 189 medium, 223 low**") and `README.md:27-30`/`:37` declare *this* file "stale — one pass
  behind" and quote counts ("~7 actionable highs", "169 open mediums") that appear in no current
  edition. `PARITY-GAPS.md`'s **baselines** at `:20-25` are correct and were never stale — the
  previous edition of this ledger said twice that they were, and it was wrong.

---

## Deliberately out of scope — and which deferrals are now stale

### Still genuinely deferred, by decision

- **`CFG-005` — the OAuth *acquisition* cluster. Maintainer-deprioritised.** Scope has narrowed:
  `login.rs` (1,721 lines) now ports login / logout / env-key login / status / selectors, and refresh
  lives at `cyrup-provider/src/auth/resolve.rs:146-239`. The residual is **two multi-prompt api-key
  login flows** (cloudflare, google-vertex). Filed at medium / `L`, not scheduled.
- **ADR-0001 mechanism divergences** — ratatui + crossterm where pi hand-rolls a renderer; WASM
  Component Model guests where pi runs TypeScript through `jiti`. **Mechanism only.** Per the hard
  rules, where a mechanism difference *costs behaviour* it stays on the list as work.

### Decisions required — NOT deferrals, and not encoded as severities

- **`OQ-07-1` — does cyrup build an alt-screen / fullscreen TUI mode at all?** Recorded in
  `07-cyrup-tui.md`'s new `## Open questions` section. **`TUI-019` was re-rated low → medium** this
  pass: its `low` rested on "a deliberate ADR-0001 divergence", and `PARITY-GAPS.md:709` records
  ADR-0001 as **unreadable in this workspace** — which `README:208-212` forbids resting an item on,
  and which `README:213-215` would not license anyway. The `low` was encoding a decision nobody made.
  **`SEAM-051` and `CFG-021` must be fixed under either answer and must not wait on it** — a flag
  that rejects its own default value is a defect regardless.
- **`OQ-07-2` — does `cyrup/TUI-FIDELITY.md` get merged into this file with real IDs?** 464 lines,
  ~150 presentation findings against v0.84.1, no stable IDs, no status table, therefore invisible to
  this ledger. It has already cost behaviour once: its C14 recommendation to delete the `{n} queued`
  footer segment was applied, which is exactly what turned `TUI-016` from "wrong surface" into "no
  surface at all". Either answer is defensible; silence is not.
- **The agent-harness question** — `AGENT-028` + `SESS-038` + `DRIFT-040`, now all trackers. Not a
  deferral by decision; a scope decision nobody has made. They exist to force it.

### STALE — struck, and still struck

- ~~**`steer`**~~ — **STALE.** `SUBA-013` is partially closed: `CYRUP_SUBAGENT_STEER_INBOX`
  (`exec/mod.rs:1857-1868`), the child-side `SteeringInbox` (`prompt_runtime.rs:157-290`), the
  `steer` verb at `extension.rs:7825-7837`. The residual is filed work, not a deferral: **`SUBA-049`**
  (ack, delivery `mode`, `steeringRecovery` — a steer is currently fire-and-forget).
- ~~**`watchdog/`**~~ — **STALE.** `8902b4f` ported it (18–22 modules, ~18k lines, a real stdio LSP
  client, nine subscriptions, four `watchdog.*` verbs). `SUBA-011` closed. **Existence is not
  correctness** — `PARITY-GAPS` UW-3 / UW-4 / UW-5 record three no-op holes *inside* it (child NDJSON
  status never read; review never runs a model turn; the permission arbiter never runs a model turn,
  so every `ask` denies). Those are work.
- ~~**FleetView**~~ — **STALE.** `8902b4f` ported `tui/fleet*` (`fleet.rs` 2,863 lines plus five
  siblings, `/subagents-fleet` registered). `SUBA-012` closed. Residual: `PARITY-GAPS` **UW-7** — the
  fleet-status widget receives no keystrokes.
- **The same commit also ported `missions/`** (~7k lines, six modules) which no deferral list ever
  mentioned. **The deferral list was not tracking what was actually deferred.**

### Survives, but was recorded wrong

- **The "four `schedule*` verbs"** — still deferred, but it is **nine** verbs, not four:
  `schedule.create` / `list` / `show` / `history` / `pause` / `resume` / `run` / `run-due` / `delete`
  (`shared/types.ts:1968` @v0.47.1). `SUBA-016`, medium / `L`. It feeds tracker `SUBA-005`.

---

## Ranking proposal

**Criterion, stated so it can be disagreed with:** rank by *user-visible consequence when the code is
wrong*, in this order —

1. **silently destroys or corrupts something the user produced** (typed input, a paste, a label, a
   session file);
2. **silently returns a wrong result** the user cannot distinguish from a right one;
3. **a security or sandbox default that is inert** — fails open with no signal;
4. **an advertised control that does nothing** on a path that bills tokens or mutates state;
5. **a whole capability dead on a normal path** (loud, so the user knows, but blocked);
6. **cost, hygiene, and diagnostics.**

Effort breaks ties *within* a band only — never across one. A cheap loud bug does not outrank an
expensive silent one.

**0. Build the two guards before fixing anything else. Effort `S`. Highest leverage-per-hour.**
   (a) a **schema/dispatch drift test** — nothing today fails when a property is added to a tool's
   parameter schema without being wired into its dispatcher; (b) a **"no production caller" check**.
   Together they are the mechanical countermeasure to cluster **F1**, now ≥31 ids across ten areas
   and including two of the six criticals (`EXT-054`, and `TUI-027`'s label-persist path) plus four
   highs (`SESS-040`, `SUBA-043`, `PROV-029`, `SEAM-061`/`SEAM-062`). This was suggested-order 0 in
   both previous editions, it was not built, and the class has grown every pass. That is the argument
   for building it now.

**1. The silent-destruction criticals, as two shipments.**
   (a) **`AGENT-020` (S) → `AGENT-030` (M)** — one latch, two branches; fixing either alone moves the
   loss to the other. (b) **`TUI-042` (S) + `TUI-043` (S) + `TUI-044` (S)** — three small edits to
   `editor.rs` that stop the prompt editor silently substituting marker text for a paste. Both
   shipments are band 1 and both are mostly `S`.

**2. Audit what the recent closures shipped, not that they shipped.** *(Raised from position 12 —
   see structural defect **E**, and the critique finding that produced the move.)* Both audited
   provider closures produced highs: `PROV-005` → `PROV-027`/`028`/`029`/`030`. **The named,
   unaudited scope is:** `amazon-bedrock` and `openai-codex`, which arrived in the same sweep as
   `google-vertex` and `github-copilot` with **no read-against-upstream pass at all**; `cf26010`'s
   **other nine OAuth flows** (only the Copilot/Codex halves have been read); and the **~28k lines
   `8902b4f` landed in one commit** (watchdog/, missions/, tui/fleet*), of which only the three
   `PARITY-GAPS` UW holes are documented. Expected yield exceeds most of what was previously ranked
   above it, and the two provider names alone are the same shape that produced `PROV-030`.

**3. The three fail-open security defaults, plus the shell surface.** `PERM-009` (S — delete three
   lines and a comment) · `PERM-023` (S) · `EXT-054` (M) with `EXT-055` (S) in the same edit ·
   `TOOL-039` + `TOOL-007` as **one** shell-surface decision. Band 3. `EXT-054` has zero blast radius
   today, which is exactly why it should land before the first third-party guest rather than after.

**4. `TUI-027` (M).** The remaining item where a mistyped key **writes** persisted data. Needs
   `/tree` text search, the keymap rebind, and the seven `app.tree.filter.*` ids in
   `TreeAction::from_id`. **`CFG-048` must land first** or the namespace rename breaks every
   `editor.*` config written against shipped cyrup.

**5. The pre-launch startup surface — `SEAM-064` (S) + `SEAM-062` (S) + `SEAM-063` (M) + `SEAM-065`
   (M) + `SEAM-061` (M).** Five highs, one surface, all from the `packages/coding-agent/src/cli/`
   sweep that had never been run. `SEAM-064` is a one-character production change. Every one of these
   needs a **live terminal run** to verify, not a driven event loop.

**6. The compaction-safety trio `SESS-040` + `SESS-041` + `SESS-042` (M, one path), then `TUI-031`
   (M).** One afternoon on a path that bills tokens and rewrites the session file. Do `TUI-031`
   immediately after — same subsystem, and it is the reason a cancelled compaction matters.

**7. `SEAM-051` (S).** Band 5, but first within it: the default value of a v0.84.1 flag makes the
   binary exit 1, and it is one token from working. No pi migrant can launch cyrup today.

**8. The provider wire cluster.** `PROV-027` + `PROV-028` in **one** edit (PROV-028 needs the exact
   provider guard PROV-027 introduces — separately is strictly more work), `PROV-029` alongside as
   one field assignment per provider; then `PROV-048` + `PROV-049` + `PROV-050` (the SSE
   surrogate/astral trio, all `S`, all in `json_parse.rs`).

**9. `SUBA-043` (S) + `SUBA-014` (S).** Two head-injections in the same crate, both silent, both
   under a day. Cheapest high-value pair in the backlog.

**10. `PROV-047` (M) + `CFG-035` (M).** Egress that ignores the proxy setting on seven paths; a trust
   gate that asks about a file cyrup will never read.

**11. Signal teardown — `SEAM-047` (M) with `SEAM-059` (S, same function), `SEAM-008` and
   `DRIFT-049`.** Four ids, one defect. Blocks any supervised deployment of `--mode rpc`. **Schedule
   once, in area 08.**

**12. `PROV-030` (L).** A whole wire API, plus the mandatory `all.rs:12-47` doc correction. Correctly
   last among the highs: it fails loudly and immediately.

**13. The WIT/ABI batch — cluster F2, one `cyrup:ext@0.5.0` bump, 27 ids.** Land `TOOL-021` (owned
   `prompt_guidelines`) in the same move; without it `impl Tool for WasmTool` can never carry
   guidelines. `L`-once versus 27 × `M` and 27 guest-refusal cliffs.

**14. The test-defect sweep — cluster F3, 23 ids, one pass.** The suite is at zero failures, so any
   new failure is signal. Deduplicate `AGENT-019` / `DRIFT-039` first.

**15. A third surface-driven sweep, on the axes the area files now name as unrun.** This pass's two
   most productive axes were both new: **cyrup's own asserted invariants, inverted** (grep the claim,
   then grep for a reader — this produced `EXT-054` and `EXT-055`, and a pi-anchored sweep is
   structurally blind to it because pi has no capability model), and **the non-drawing files of an
   upstream package** (`stdin-buffer.ts` is 434 lines that draw nothing, and it produced two
   criticals). The named unrun targets, in the files' own words: **`crates/cyrup-tui/src/editor.rs`
   read line-for-line against `components/editor.ts` @v0.83.0** — `TUI-042`/`043`/`044`/`049` were all
   found from *outside* that file, which strongly implies more inside it (area 07 blind spot 9);
   **`pi-intercom`'s 68 top-level exports at v0.9.2, restricted to `broker/` first** — no pass has
   ever walked that surface, because every axis used there was bounded by the drift window or by what
   a prior pass happened to close (area 11 blind spot 10); and **symbol-by-symbol enumeration of
   `core/extensions/{types,runner,loader,index}.ts`** (area 06 blind spot 8).

---

## Structural defects in this analysis — status update

**A. Split open-items tables hid items from every enumeration. — RETIRED.** Area 03's second table
was the last instance and the repair pass deleted it: `SESS-S05` moved into the main table as a low
row with its id, severity, kind, effort and status untouched, the `## Surface-sweep findings` heading
and body retained for provenance, and an explicit instruction not to re-add a second table.
**All twelve files now have exactly one open-items table**, verified mechanically (one severity-table
header per file; every row has a body and every body has a row). Areas 08, 09 and 12 additionally
carry a separate `## Trackers` table — that is deliberate and is *outside* the count, not a split.

**B. Surface-sweep items carried no verified upstream trace. — RETIRED.**

**C. An item-driven analysis cannot see behaviour nobody wrote an item for. — CONFIRMED, HARD, and
demonstrated twice in one day.** The first pass filed 176 against 117 closures. The repair pass then
read four upstream surfaces nobody had named and filed **31 more, including 2 criticals and 8
highs**, without any new code being written. **Treat 448 as a floor, not a total**, and treat any "we
are N items from done" claim as unsupported.

**D. Files contradict themselves. — the named instances are fixed; a NEW class was found and it is
worse.** Resolved: area 06's `EXT-028` row, area 12's `DRIFT-026` contradiction, area 10's
`PERM-001`/`PERM-005` prose. **Struck as false:** this file's own claim that `PARITY-GAPS.md`'s
header was stale (see the header block). **The new class is code contradicting the analysis:**
`providers/all.rs:12-47`'s port-status table calls four registered providers "pending (NOT
registered)" and contradicts its own table row at `:21`, so an engineer opening `PROV-030` reads a
denial of the item before reading the item. The doc correction is now a mandatory part of
`PROV-030`'s Fix rather than a separate id — a separate id would let the code fix land without it.

**E. A closure reliably ships defects inside the code that closed it. — CONFIRMED, and now a rule.**
`PROV-005`'s closure shipped `PROV-030` **and** `PROV-027`/`028`/`029` · `ICOM-022`'s closure shipped
`ICOM-027`, `ICOM-043` and `ICOM-048` · `ICOM-002`'s closure shipped `ICOM-035` · `AGENT-S01`'s fix
shipped `AGENT-021` and `AGENT-029` · `SESS-023`'s closure is what exposed `SESS-040`/`041`/`042` ·
`TOOL-004`'s fix shipped `TOOL-019` · `EXT-S02`'s closure sharpened `EXT-013`, `EXT-017`, `EXT-053` ·
`SUBA-S01`'s closure left `SUBA-043` (high) · `SUBA-S03`'s closure left `SUBA-051` · `SUBA-007`'s
closure left `SUBA-047`.

  **The rule: closing a "not implemented" item means the subsystem now EXISTS, not that it is
  CORRECT.** A closure is not done until the closing code has been read against upstream. This is
  ranking item 2.

**F. The "advertised but inert" class is mechanically detectable and nobody is detecting it. —
CONFIRMED and growing.** See cluster F1 and ranking item 0.

**G. NEW — citation drift between the two tags is systemic, not incidental, and it reached the #1
item in the backlog.** `AGENT-020` cited `agent.ts:361-388` "(identical at v0.83.0 and v0.84.1)"
when `async continue()` is at `:350` at the ported baseline. The repair pass swept the class and
found **twenty-plus further instances across areas 01, 02, 04 and 07** — nine wrong in area 01 alone,
the worst on a high (`PROV-029` quoted `isSubscription: true` from `github-copilot.ts:16` "@v0.83.0",
a property that **does not exist at v0.83.0 at all**), `PROV-023` cited a line holding a *different
flag*, `PROV-024`'s four cites matched **neither** tag, and `TOOL-036` proved to be half
upstream-drift because `normalizeWindowsShellPath` landed *inside* the window. The `agent-loop.ts`
shift is **not uniform** (0 through `:636`, +4 from `:642` on, because the block arm was rewritten),
which is exactly the hazard `README:224-225` warns about. **Method, now written into three area
files: never write "identical at both tags" — give per-tag offsets, each labelled with the tag it was
read at, and re-resolve by opening the file rather than shifting.** Area 01 proposes widening
`PROV-041`'s citation lint to cover `docs/gap-analysis/*.md`; that is the mechanical fix.

**H. NEW — a commit hash is not evidence of a classification.** Nine area-12 items rested on a hash
rather than a two-sided read. Seven were re-derived cheaply this pass and **six proved
misclassified** (see *Version lag*). The remaining two (`DRIFT-023`, `DRIFT-040`) are now trackers in
an explicit `## Leads — not yet evidenced` section outside the count, each carrying the exact commands
that would settle it. **Rule: `git cat-file -e v<ported-tag>:<path>` must run before any
`upstream-drift` kind is assigned** — a hash answers "when did this land upstream", but the
classification turns on "before or after the tag cyrup was ported from". The generalisation of the
same error: *"no runtime effect" licenses skipping a directory's behaviour, never its provenance — a
gitignored path is evidence that an artifact is generated, hence that a generator exists, not
evidence that the generator is absent.* That sentence is why the catalog generator went unfound for
three editions.

---

## Corrections to carry forward — these are wrong ABOUT THE CODE or ABOUT UPSTREAM

1. **DISCHARGED.** The false `subagent-executor.ts:3022` "pi precedent" is fixed in place; all five
   surviving occurrences in `cyrup-ext-subagents/src/extension.rs` are now the **correction**.
2. **STANDS.** `EXT-S01`'s original Impact was wrong about pi in a way that could have inverted a
   security default. The lesson stands: an item's Fix text is a hypothesis.
3. **STANDS.** `CFG-002`'s prescribed Fix was wrong and the implementer correctly ignored it. pi
   throws unconditionally (`provider-composer.ts:167-169`).
4. **DISCHARGED.** `PROV-026` is struck — `seed.json`, `seed_catalog()` and `seed_catalog_parses` no
   longer exist.
5. **`SEAM-019` is unworkable as written and is superseded by `SEAM-051`.** Its premise
   (`--ui-mode` / `--alt`) is false at **both** tags — pi has never had either flag.
6. **`CFG-021` was misdescribed twice.** The key is `tuiMode`, not `uiMode`; both it and
   `fullscreenScrollbar` are v0.84.1 additions, so its kind is `upstream-drift`.
7. **`DRIFT-028` is halved.** The `~anthropic/*` alias claim is refuted: pi's own runtime detector is
   byte-equivalent to cyrup's. A shared upstream bug is not a cyrup divergence.
8. **`SUBA-024` and `SUBA-021` cite files that never existed.** `chain-validation.ts` has no history
   at any tag; `launch-contract.ts` is absent at both v0.43.0 and v0.47.1.
9. **NEW — `DRIFT-009`'s "no in-tree regeneration source" was false**, and it produced a Fix
   (seeding from the pi.dev artifact) that is strictly lossier than the correct one. Rewritten to
   defer to `PROV-018`. **This ledger repeated the error as "catalog lag is unresolvable from this
   workspace"; both are struck.**
10. **NEW — this ledger asserted twice that `PARITY-GAPS.md`'s header was stale. It was not.**
    `PARITY-GAPS.md:3-8` records v0.47.1 and v0.10.1 and *opens* by explaining the correction. Struck
    from *Version lag* and from structural defect D.
11. **NEW — `TOOL-036` is half upstream-drift.** `normalizeWindowsShellPath` returns nothing at
    v0.83.0; it landed inside the window. The `~`/`os.homedir()` half **is** at v0.83.0 (`paths.ts:67`)
    and remains a genuine baseline parity bug, so the item keeps `kind=parity-bug` with the drift half
    labelled. Root cause: area 04's version-lag sweep was scoped to `core/tools/`, and `utils/paths.ts`
    is not under that path.
12. **NEW — `SEAM-035`…`SEAM-046` never existed.** `git show a9000b1:docs/gap-analysis/08-….md | grep -o 'SEAM-0[34][0-9]'`
    returns 030–034 only, and the missing ids appear in no file. It is a numbering artifact of this
    pass starting its new ids at `SEAM-047`, **not a deletion**. Honest caveat: `docs/gap-analysis`
    has exactly **one** commit in cyrup's history (`a9000b1`), so an id dropped before the directory
    came under source control is invisible to that check — to this pass and to every pass.

---

## By area

> **SUPERSEDED 2026-08-14** — see the *By area* table at the top of this file.

Highs, mediums **and** lows have all been audited against code at HEAD `04c1ba2`. Counts are derived
from each file's own single `## Open items` table; the `trk` column is excluded from every other
column and from the total.

| file | open | crit | high | medium | low | trk | closed | filed | open criticals + highs |
|---|---|---|---|---|---|---|---|---|---|
| [01-cyrup-core-and-provider](01-cyrup-core-and-provider.md) | 40 | 0 | 6 | 14 | 20 | 1 | 10 | 22 | `PROV-027` `PROV-028` `PROV-029` `PROV-030` `PROV-047` `PROV-048` |
| [02-cyrup-agent](02-cyrup-agent.md) | 26 | 1 | 1 | 6 | 18 | 1 | 3 | 14 | **`AGENT-020`** `AGENT-030` |
| [03-cyrup-session](03-cyrup-session.md) | 29 | 0 | 1 | 13 | 15 | 1 | 9 | 9 | `SESS-040` |
| [04-cyrup-tools](04-cyrup-tools.md) | 29 | 0 | 1 | 10 | 18 | 0 | 14 | 11 | `TOOL-039` |
| [05-cyrup-config-and-resources](05-cyrup-config-and-resources.md) | 38 | 0 | 1 | 19 | 18 | 0 | 16 | 17 | `CFG-035` |
| [06-cyrup-ext](06-cyrup-ext.md) | 50 | 1 | 0 | 28 | 21 | 0 | 6 | 21 | **`EXT-054`** |
| [07-cyrup-tui](07-cyrup-tui.md) | 56 | 3 | 1 | 26 | 26 | 0 | 13 | 25 | **`TUI-027`** **`TUI-042`** **`TUI-043`** `TUI-031` |
| [08-cyrup-session-svc-and-modes](08-cyrup-session-svc-and-modes.md) | 40 | 0 | 7 | 19 | 14 | 1 | 6 | 24 | `SEAM-047` `SEAM-051` `SEAM-061` `SEAM-062` `SEAM-063` `SEAM-064` `SEAM-065` |
| [09-cyrup-ext-subagents](09-cyrup-ext-subagents.md) | 45 | 0 | 2 | 23 | 20 | 1 | 22 | 24 | `SUBA-014` `SUBA-043` |
| [10-cyrup-permission-system](10-cyrup-permission-system.md) | 21 | 1 | 1 | 6 | 13 | 0 | 10 | 7 | **`PERM-009`** `PERM-023` |
| [11-cyrup-intercom](11-cyrup-intercom.md) | 44 | 0 | 0 | 22 | 22 | 0 | 3 | 24 | — |
| [12-upstream-drift-pi-core](12-upstream-drift-pi-core.md) | 30 | 0 | 1 | 11 | 18 | 4 | 5 | 9 | `DRIFT-049` *(dup of `SEAM-047`)* |
| **total** | **448** | **6** | **22** | **197** | **223** | **9** | **117** | **207** | |

**Where the backlog actually sits.** Areas **07 (56)** and **06 (50)** carry 24% of it between them,
and 06's 28 mediums are still the largest single medium block anywhere. **Area 08 is the new
concentration of *severity*** — seven open highs, more than the next three areas combined, and every
one of them from a single unrun axis. Area **11 (44)** is third by volume and is almost entirely new,
because its true baseline was only just established; its zero highs should be read as *un-swept*, not
clean.

---

# Archive — superseded provenance

> **DO NOT PLAN FROM THIS SECTION.** Everything below is retained as the historical record required
> by the stable-id rule. It was true when written and is **not** true at `04c1ba2` after the repair
> pass. Read it for *how the analysis got here*, not for what to do.

### Archived — the first 2026-08-12 edition's headline claims (superseded the same day)

> Its three headline results were: **(1)** "Zero critical items and zero pre-2026-08-11 highs remain
> open" — **superseded**, an artifact of not applying `README.md:106-107`'s own definition; there are
> six criticals, four of which were already in that edition's backlog, mis-rated. **(2)** "The medium
> closure rate went from 3.9% to roughly 29%" — **stands**, and the analysis behind it stands; it is
> reproduced above. **(3)** "The open count went UP anyway: ~311 → 426; treat 426 as a floor" —
> **superseded by 448 raw / ~420 deduplicated**, and the reason it was superseded is that 426 was
> simultaneously called a floor and inflated by ≥25 ids of documented double-count. Both figures are
> now published separately.
>
> Its 14-row actionable table ranked `AGENT-020` first (correct, and the item is now critical),
> `TUI-027` second (now critical), `EXT-054` seventh (now critical) and did not contain `PERM-009`,
> `TOOL-039`, `SEAM-051` or any of `SEAM-061`…`SEAM-065` at all — the first three because they were
> mis-rated, the last five because the axis that found them had not been run. Its ranking put "audit
> what the recent closures shipped" at **position 12**; it is now position 2.
>
> Two of its statements were false about other files and are struck in the corrections list above:
> that `PARITY-GAPS.md`'s header was stale, and that catalog lag is unresolvable from this workspace.

> **UPDATE — GitHub Copilot, 2026-08-11, cyrup HEAD `097bdde`.** `PROV-005` (four providers
> unimplemented) is **CLOSED** — `amazon-bedrock`, `openai-codex`, `google-vertex` and
> `github-copilot` are all constructed in `providers/all.rs:177-197`, and `register_builtins`
> (`api/mod.rs:131-163`) now registers 9 factories including the formerly dangling
> `bedrock-converse-stream`. **But the Copilot port that closed it carries three new highs**, filed
> as `PROV-027`/`028`/`029`:
>
> * **PROV-027** (parity-bug, S) — Copilot's 9 Claude models send `x-api-key`; pi has an explicit
>   Copilot branch sending `Authorization: Bearer`.
> * **PROV-028** (not-ported, S) — `github-copilot-headers.ts` has no counterpart, so `X-Initiator`,
>   `Openai-Intent` and `Copilot-Vision-Request` are absent on **all three** wire routes.
> * **PROV-029** (parity-bug, S) — the Copilot **and** Codex login flows are fully ported and
>   unreachable.
>
> Two things generalise past Copilot. **(1) A "not implemented" item closing means the subsystem now
> exists, not that it is right.** **(2) `PROV-003`'s "deprioritised, out of scope" status was stale**:
> `cf26010` landed all 11 OAuth flows under `auth/oauth/`.

> **UPDATE `6104dfa` + `7fd0d9c` (2026-08-07).** `control` and `includeProgress` are ported for real
> and live in the schema. `SUBA-N04` closed (the field was dropped in THREE places, not one).
> `TOOL-019` closed and revert-proved twice. `PROV-006` closed as a process-global **idle** timeout
> mirroring pi's `configureHttpDispatcher` — deliberately not a total-request deadline, which would
> kill a healthy model streaming a token every 20 s. `PROV-008` capped at pi's 4000 chars.
> **3932 passed / 0 failed / 8 ignored across 225 suites.**

> **REWRITTEN 2026-08-07 after a code-level audit of every high/critical item at HEAD `8d00f06`.**
> The previous version of this file was baselined at `1806375`, **eight commits behind**, and an
> auditor called it *"the most dangerous file in the directory"* — a planner reading it would have
> scheduled about twelve already-completed items.

### Archived — "The real number: 4 actionable highs, not 17 — now 7" (2026-08-07 / 2026-08-11)

38 high/critical items were audited on 2026-08-07. **31 CLOSED, 5 OPEN, 2 PARTIAL.**

| ID | area | state *(as of 2026-08-11)* | why it mattered |
|---|---|---|---|
| **SUBA-S01** | 09 | OPEN — **now CLOSED** | A declared `outputSchema` never reached the child in any form. *(Its residual is `SUBA-043`, still high.)* |
| **SUBA-N03** | 09 | **CLOSED** | Overrides refused on the async/background branch. ~~**EIGHT params, not seven.**~~ **CORRECTED 2026-08-14 (sweep 6): the closure covers ELEVEN params** — output, outputMode, skill, share, sessionDir, artifacts, acceptance, control, includeProgress, timeoutMs, maxRuntimeMs — each asserted (a) not to hit the refusal, (b) to reach agent resolution, (c) to be an advertised property; **and the load-bearing half is separately pinned: two companion tests assert the params reach the DETACHED hop-2 runner at the `runner-config.json` boundary, not merely past the router.** Do not re-open it with the eight-param framing. |
| **EXT-S02** | 06 | OPEN — **now CLOSED** | Extension slash commands never reached the TUI `/` autocomplete. `SlashCommand::name` is `Cow<'static, str>` at `commands.rs:36`. |
| **TUI-S01** | 07 | PARTIAL — **now CLOSED as framed** | 6 of 9 `UiEffect` mutators wired. Residue is `TUI-014` + `TUI-033`. |
| **PROV-027 / 028 / 029** | 01 | OPEN (2026-08-11) — **still open** | See the Copilot update above. All three remain in the current actionable table. |

**Archived deferral note (2026-08-07/11), now superseded:** *"Deliberately out of scope: `CFG-005` —
the OAuth acquisition cluster… Also deferred as subsystems: `steer`, the four `schedule*` verbs,
`watchdog/`, FleetView."* — **three of those four subsystem deferrals are stale**, and the `schedule*`
count was nine, not four; see *Deliberately out of scope* above.

### Archived — "The mediums were audited too — 169 genuinely open" (2026-08-07)

> **Medium audit, 2026-08-07, HEAD `8d00f06`. 178 items read against code. 166 OPEN, 3 PARTIAL,
> 7 CLOSED, 2 WRONG. Closure rate 3.9%** — against **82%** for the highs.

The mechanism is plain once stated: every commit since the analysis was written was *explicitly*
high-targeted (`513e45a`'s own message: "close the 8 remaining **high-severity** gap items"). **All 7
closures were collateral of a high fix**, never independent work. `git log 1806375..HEAD` returned
**zero** commits over `retry.rs`, `compat.rs`, `cyrup-core/src/message.rs`, `env_keys.rs`, `hooks.rs`,
`openai_completions.rs`. Seven whole areas came back **100% open**.

*(Superseded 2026-08-12: the medium rate is ~29% and no area came back 100% open. The 3.9% figure and
its diagnosis are retained because the diagnosis was correct and is what predicted the change — the
rate moved when the commits stopped being item-targeted.)*

### Archived — promoted medium → high (2026-08-07)

| ID | promoted because | status at `04c1ba2` |
|---|---|---|
| `TOOL-019` | A regression one of OUR OWN fixes introduced: `cfe351e`'s `TOOL-004` fix moved to `write_in_place`, so an unlocked cross-registry race interleaved bytes. | **CLOSED** (`7fd0d9c`, process-global `static LazyLock`, revert-proved). Residual `TOOL-032`. |
| `PROV-006` | **No request timeout at ANY layer**, and a silent no-op on a shipped control. | **CLOSED.** Real retry module + idle timeout, seven api impls. Residual `PROV-043`. |
| `PROV-010` / `AGENT-014` / `DRIFT-012` | One defect, three ids: a truncated stream transcribed as a cleanly completed turn and persisted to JSONL. | **ALL THREE CLOSED.** `StopReason::Pending`+`Deferred`+`raw_stop_reason`, verified across all six producer sites. |
| `SUBA-S06` | `drive_attempt`'s `tokio::select!` had no `child.wait()` arm — a child exiting without closing inherited stdio hung the orchestrator forever. | **CLOSED** (`exec/mod.rs:2826-2856`). |
| `TUI-N02` | `/reload` silently fails the permission gate **OPEN**. | **STILL OPEN** (medium, area 07). |

### Archived — clusters as stated on 2026-08-07

**C5 — "the extension ABI is lossy; batch every WIT change into ONE bump" — 15 ids.** *(Superseded:
the world reached `@0.4.0`; the cluster is now 27 ids against a `0.5.0` bump — see F2. The
`prompt_guidelines` prerequisite is unchanged and still unbuilt.)*

**C10 — the subagent second-hop config boundary** — `SUBA-006/007/008/014` plus `SUBA-N03/N04/N05/N06`.
*(Superseded: all four `-N` residuals closed; `SUBA-006` closed; `SUBA-007` partially closed;
`SUBA-014` is still open and is now **high**.)*

**C2 — no HTTP timeout or retry budget.** *(Superseded: closed, residual `PROV-043`.)*

### Archived — open work that appeared in no high row (2026-08-07)

- `includeProgress` and `control` were UNTRACKED and de-advertised. *(Closed — `SUBA-N06`.)*
- `SUBA-N04` filed medium, should have been high. *(Closed.)*
- `chainDir` advertised and dead. *(Closed — `SUBA-N05`.)*
- The schema is `additionalProperties: true` — 30 advertised, 32 parsed. *(The class survives as
  cluster F1 and as `SUBA-043`/`SUBA-047`.)*
- The `PROV-007` closure residual: `registry_models()` binds to `builtin_catalog()` (pi's
  credential-blind `getModels()`) where pi's six call sites use `getAvailable()`. *(Survives as
  `PROV-031` — and the `cli/` sweep found the same defect a second time on the `--list-models` path,
  which is why `SEAM-020` was re-rated low → medium.)*
