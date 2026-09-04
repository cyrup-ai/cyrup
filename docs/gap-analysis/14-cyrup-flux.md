# 14 — cyrup-flux

Covers `cyrup/crates/cyrup-flux/` — the Flux pipeline (`new → ask → split → aug → exec → qa → tests →
commit → create-pr`) as a single `cyrup_ext::native::NativeExtension`: fifteen bundled prompt
templates, one bundled skill, three native renderers (`/flux/status`, `/flux/cheatsheet`,
`/flux/about`), the `ctrl+f` status overlay, and the `ask_user_question` tool — measured against
**`code_puppy_core_plugins/flux_bootstrap` at tag `v0.0.6`** (checkout at
`/Users/davidmaple/cyrup.ai/code_puppy_core_plugins`, HEAD `8de5184`, latest tag also `v0.0.6`), read
with `git show v0.0.6:<path>` and never from that checkout's working tree — **its working tree is
missing `flux_bootstrap/bundled/commands/` entirely even though `v0.0.6` has all 22 files**, so a
working-tree read here silently reports the upstream command set as absent.

**This is the FIFTH ported upstream in this directory and the first that is neither pi nor
TypeScript.** `README.md:3-4` still says cyrup is measured against "four TypeScript upstreams";
code-puppy is Python, and the `git show <tag>:<path>` hard rule applies to it unchanged. The second
upstream this crate needs is `code_puppy` itself (`/Users/davidmaple/cyrup.ai/code_puppy`, HEAD
`757db1cf`, latest tag `v0.0.720`) — the `invoke_agent` tool the templates were renamed off lives in
core code-puppy, not in the plugin, and `FLUX-002` turns on that fact.

> **Audited 2026-08-19, cyrup HEAD `4fb5e40`** (merge of `david/cyrup`), against
> `code_puppy_core_plugins v0.0.6` and `code_puppy v0.0.720`.
>
> **FIRST PASS. Nothing in this area has ever been audited.** The crate landed at `67b73a0`
> (+ `adb3bba` `spec/flux.md`, + `0c0367d` `docs/guide/extensions/flux.md`), 9 source files /
> **1,513 lines**, and `grep -rn 'cyrup-flux' docs/gap-analysis/` returned **zero hits** before this
> file existed — so the directory's headline count ("606 rows across the twelve tables") silently
> excluded an entire shipped, unconditionally-attached surface. That is the same class as the MCP
> exclusion declared at `00-residual-ledger.md:18-23`, except this one was never declared.
>
> **Why a numbered area file rather than rows in `06-cyrup-ext.md`.** Area 06 is the extension
> **host** — WIT world, event catalog, registry, dispatcher. Area 09 is the standing precedent that
> an extension-shaped **crate** gets its own area: `cyrup-ext-subagents` is a `NativeExtension` and
> has `09-cyrup-ext-subagents.md`. `cyrup-flux` is the same shape (one crate, one
> `NativeExtension`, its own upstream, its own bundled content tree), and it additionally has an
> upstream no other area file measures against. Filing it into 06 or 09 would put a code-puppy
> baseline inside a file whose header pins a pi tag.
>
> **Open set after this pass: 7 items — 0 critical, 0 high, 4 medium, 3 low.** No trackers. Four of
> the seven are cross-cutting rather than local: `FLUX-001` names three sibling instances outside
> this crate, `FLUX-002`'s fix site is arguably `crates/cyrup/src/main.rs`, `FLUX-004` is the first
> LIVE instance of `EXT-039`'s open residual, and `FLUX-007` is a documentation-integrity defect that
> makes every other row in this file harder to re-audit.
>
> **Content coverage is COMPLETE and was verified by enumeration, not by sampling.** Upstream
> `v0.0.6` ships **18** `bundled/commands/flux/*.md`; cyrup ships **15** templates
> (`resources/prompts/flux/*.md`) plus **3** native renderers, and the three that became renderers
> are exactly upstream's three `exec:`-directive commands (`about.md`, `cheatsheet.md`,
> `status.md`). 15 + 3 = 18. `FLUX_11`'s marker sweep is genuinely complete: `grep -rn 'FLUX-GAP'
> crates/cyrup-flux/` is **0**, and the sites those markers held are live — **25 `ask_user_question`
> occurrences across 11 template files**, exactly the census `spec/flux.md:863-864` recorded. **Do
> not file a content gap here without re-running that enumeration.**
>
> **Static only.** Nothing was executed — no `cargo`, no binary, no `/flux/*` command run. Every
> `Verify` line below is a design, not an observation. That matters more in this area than in most,
> because the crate has no tests at all (`FLUX-003`), so there is nothing to read a green result off
> either.

> **Re-audited 2026-09-04, cyrup HEAD `2571969`** (210 commits ahead of `4fb5e40`).
> `git log --oneline 4fb5e40..HEAD -- crates/cyrup-flux` returns exactly **one** commit, `254bb48`,
> and its only touch on this crate is two rustdoc intra-doc-link fixes in doc comments
> (`ask_tool.rs`, `overlay.rs`) — no behavior changed. **`FLUX-001` through `FLUX-006` were each
> re-read against the code cited above and are unchanged: still open, same severity, same cited
> lines.** Re-confirmed this pass: `bundled_dir()` still resolves through `CARGO_MANIFEST_DIR` with
> no loud-failure path (`FLUX-001`); the four multi-task templates still name `subagent`
> unconditionally with no availability fallback (`FLUX-002`); `grep -rn '#\[cfg(test)\]\|#\[test\]'
> crates/cyrup-flux/` is still **0** (`FLUX-003`); `resolve_shortcuts` still has no production caller
> (`FLUX-004`); the four `_docs`/`reference` pairs are still byte-identical with no test
> (`FLUX-005`); `state.rs`'s `parse_frontmatter` still uses `read_to_string` (UTF-8-strict), not
> `read` + `from_utf8_lossy` (`FLUX-006`).
>
> **`FLUX-007` is substantially revised below.** Both upstreams named in this file's own header are
> now cloned in this workspace — `tmp/code_puppy_core_plugins` (HEAD `8c6f852`, latest tag now
> **v0.0.40**, up from the recorded baseline `v0.0.6`) and `tmp/code_puppy` (HEAD `38f74d4`, latest
> tag now **v0.0.819**, up from the recorded baseline `v0.0.720`) — neither was available in this
> workspace as of the first pass. A diff-stat sweep of the ported surface across that gap found
> **zero content drift**: `git -C tmp/code_puppy_core_plugins diff --stat v0.0.6 v0.0.40 --
> code_puppy_core_plugins/flux_bootstrap/ tests/test_flux_bootstrap.py` is empty —
> `flux_bootstrap/` is byte-identical at both tags despite 34 tags of distance, so **no new
> version-lag item is filed from that range; do not re-file one without re-running this diff.**
> `code_puppy`'s `tools/__init__.py` and `tools/subagent_invocation.py` — the two files `FLUX-002`
> cites — DID change between `v0.0.720` and `v0.0.819` (+170/−42 combined), but `invoke_agent` is
> still registered in `TOOL_REGISTRY` unchanged at the CITED tag `v0.0.720` (confirmed by re-reading
> it), so `FLUX-002`'s citation still holds exactly as written. The `v0.0.720..v0.0.819` range itself
> was read only far enough to confirm that one fact and is otherwise unswept — see Blind spots below;
> nothing in the diff (a Codex-patch tool-selection rewrite and a subagent recursion-guard refactor,
> neither touching Flux) looked close enough to `FLUX-002` to file with confidence.

## Status table (every item from every prior pass)

**There are no prior passes.** This section is declared, and empty, per the item format
(`README.md:434-435`): every area file carries a status table covering every item from every prior
analysis, and for area 14 that set is empty. The seven `FLUX-NNN` ids below are all filed by this
pass. Nothing in this area has ever been closed, refuted, superseded or misdescribed, so there is
nothing to re-audit — the next pass audits the rows in `## Open items` directly.

| ID | Status | Evidence |
|---|---|---|
| — | — | First pass; no prior items exist. |

**ID-SCHEME RESERVATION, recorded so nobody later "recovers" the missing numbers.** This file uses
the HYPHEN form `FLUX-001`…, because `spec/flux/` already owns the UNDERSCORE form
`FLUX_01`…`FLUX_12` as **task-file** names (`spec/flux/README.md:56-69`), and the crate source cites
those task ids **13 times** (`FLUX_01`×2, `FLUX_06`×1, `FLUX_07`×3, `FLUX_08`×1, `FLUX_09`×4,
`FLUX_10`×2). Two schemes one underscore apart will be conflated. `FLUX-001` … `FLUX-007` are item
ids and have no relationship to `FLUX_01` … `FLUX_12`, which are build tasks — the same distinction
area 08 records for `SEAM-035`…`SEAM-046`, which never existed. **`CMDHINT_01` (`spec/CMDHINT_01.md`)
is a third instance of the ad-hoc-task-id-in-shipped-source class and is NOT this area's** — it lands
in areas 07 and 08.

## Open items

> **Counted set: 0 critical, 0 high, 4 medium, 3 low = 7.** No trackers. Every row was filed from
> reading both sides at the named revisions; none is inherited, and none is a sweep digest routed
> here. **`FLUX-001` is the one to schedule first** — it is the only row that can make the whole
> feature silently disappear for a user, it is `S`, and one fix shape closes the three sibling
> instances it names in `cyrup-ext-subagents` and `cyrup-intercom` as well.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| ~~FLUX-001~~ | ~~medium~~ **CLOSED 2026-09-04** | cyrup-original | S | The bundled prompt/skill tree is resolved through a build-machine `CARGO_MANIFEST_DIR` path, so a binary without an intact source tree loses all 15 `/flux/*` templates and the skill — silently, while the three native commands, `ctrl+f` and `ask_user_question` still register. **CLOSED 2026-09-04, cyrup `03f3add0`.** Landed as the row's **Fix (a)** with **(b)** on top: `crates/cyrup-flux/build.rs` embeds `resources/**` as a generated `include_bytes!` table (`src/bundle.rs` `bundled_files`/`bundled_file`/`bundle_fingerprint`); `src/install.rs` is the port of upstream's `installer.py` (`decide` → `FileAction`, `install_pass`, `ensure_installed` → `InstallOutcome::{UpToDate, Installed, SkippedLocked}`, SHA-256 manifest `.flux_bootstrap_manifest.json`, marker `.flux_bootstrap_version`, unique `.bak`/`.bak.N`, atomic tmp+rename, `fs4` non-blocking flock on `.flux_bootstrap.lock`); `src/resources.rs` replaces `bundled_dir()` with `BundledRoot::{Vendored(CYRUP_FLUX_RESOURCES_DIR), Managed(<agent_dir>/flux/resources)}` resolved once at construction (`resolve_from`, `managed_root`), and the `CARGO_MANIFEST_DIR` fallback is gone; `src/extension.rs::materialise_bundle` runs on every `ResourcesDiscover` before the contribution (upstream `register_callbacks.py:47-68`, notice wording kept) and a miss on either root is now a `notify` Warning naming the path instead of `HookOutcome::Noop`; `lib.rs` `flux_extension(agent_dir)` / `flux_extension_for_env(agent_dir)` / `flux_extension_with_root`, wired at `crates/cyrup/src/session_launch.rs:138` with the same `agent_dir` MCP and the permission system receive. Upstream re-read at **v0.0.40** (`flux_bootstrap/` byte-identical to v0.0.6): `installer.py:9-20` (goals), `:47`, `:113-127`, `:139-149`, `:152-167`, `:170-222`, `:225-256`; `register_callbacks.py:47-71`. **Tests (red before / green after):** `crates/cyrup-flux/tests/flux_001_embedded_bundle.rs`, 12 tests — a probe of `resources_discover_contributes_the_managed_root_not_the_build_tree` against the pre-change crate failed with `contributed the build-machine source tree: /home/user/cyrup/crates/cyrup-flux/resources/prompts`; `cargo nextest run -p cyrup-flux` 18/18 after (FLUX-002's 6 re-pointed at the embedded bundle, assertions unchanged). **Residual (medium, unfiled here):** the three sibling `CARGO_MANIFEST_DIR` resolvers this row names in `cyrup-ext-subagents` (`registration/resources.rs:46`, `extension/…builtin_agents_dir`) and `cyrup-intercom` (`resources.rs:41`) are NOT touched — other tracks own those crates this batch; the mechanism is reusable as-is. **Residual (low):** the row's full Verify (release binary moved off-tree, `/flux/new` resolves, `/skill:flux` loads) was not executed live; the unit-level Verify it names is `a_fresh_install_materialises_every_file_then_is_a_no_op`. Labelled inference: the marker is `<crate version>+<bundle sha256>`, not the version alone as upstream (`register_callbacks.py:38-44`), so a same-version rebuild with an edited template re-installs. |
| ~~FLUX-002~~ | ~~medium~~ **CLOSED 2026-09-04** | parity-bug | S | Four templates instruct the model to call the `subagent` tool, which is default-OFF, while Flux itself is default-ON — the rename `invoke_agent` → `subagent` ported the name and dropped the availability. **CLOSED 2026-09-04, cyrup `c7d21bbb`.** Landed as the row's prescribed template fix, not the rejected gate-arming: each of the four multi-task branches now opens with an availability pre-condition — check the tool list for `subagent` BEFORE calling it; if absent, do NOT call it or substitute another tool, tell the user ONCE (naming the `CYRUP_SUBAGENTS=1` / `subagents/config.json` opt-in) and take the sequential single-task path — at `resources/prompts/flux/exec.md:48-49` (dispatch `ELSE:`) + `:181` (MULTI-TASK MODE), `aug.md:50-51` + `:158`, `qa.md:48-49` + `:163`, and `review.md:109` (STEP 6, degrade = review each group in-line, extended to STEP 7's second launch at `:192`); the skill's `N` row (`resources/skills/flux/SKILL.md:56`), `docs/guide/extensions/flux.md:52-55` and `spec/flux.md` §0.3 (next to the rename map, as the row required) record the same. The armed path is unchanged (one `subagent` call, `tasks:[]`, `concurrency: $ARGUMENTS`). The gate itself is untouched: `crates/cyrup-ext-subagents/src/extension/host/registration.rs:99-129` `is_installed`/`is_installed_with`, tool name at `extension/mod.rs:104`; flux still attaches unconditionally at `crates/cyrup/src/session_launch.rs:136` (the `main.rs:726/930/1060` seams this row cited have since moved there). Upstream re-read at **v0.0.40** (ADR-0006; `flux_bootstrap/` is byte-identical to v0.0.6): `bundled/commands/flux/exec.md:181`, `aug.md:158`, `qa.md:163`, `review.md:110` name `invoke_agent` unconditionally, which is correct there because it is a core `TOOL_REGISTRY` tool. **Tests (red before / green after):** `crates/cyrup-flux/tests/flux_002_subagent_fallback.rs` — 4 of 6 failed against the unedited templates (`every_multi_task_template_checks_tool_availability_before_calling_subagent`, `the_three_n_argument_templates_route_a_missing_tool_to_the_sequential_path_in_dispatch`, `review_degrades_in_line_and_covers_its_second_subagent_launch_in_step_7`, `the_skill_tells_the_model_the_n_mode_needs_the_subagent_tool`), 6/6 after; the two that held both times pin the armed path and the four-template census. **Residual (low):** the row's Verify is a live-model observation (`/flux/exec 3` on a three-todo fixture with the gate off completing sequentially with one notice) and was NOT executed — what is pinned is the prompt contract, and whether a given model honours a tool-list check is not testable here; the harness has no template-time signal of tool availability (prompt templates are static files contributed via `ResourcesDiscover`), so a wiring-level degrade would need a new seam and is not filed. |
| ~~FLUX-003~~ | ~~medium~~ **CLOSED 2026-09-04** | test-defect | M | The crate has zero tests at 1,513 lines — a state parser, three renderers, an overlay and a tool with no red-before evidence for anything, and no pin on the cross-harness state contract the port calls a requirement. **CLOSED 2026-09-04, cyrup `4bb3569c`.** Three test binaries, 37 tests, every expectation produced by RUNNING the upstream Python at **v0.0.40** (`git -C tmp/code_puppy_core_plugins show v0.0.40:code_puppy_core_plugins/flux_bootstrap/bundled/scripts/{flux_status,flux_cheatsheet,flux_about}.py`; `flux_bootstrap/` is byte-identical v0.0.6..v0.0.40, so no tag drift), with the fixture trees and regeneration commands in each file's header: `crates/cyrup-flux/tests/flux_003_state_and_status.rs` (14 — minimum-set items (1)-(6): `flatten_cwd` 13-row table incl. leading/trailing/consecutive separators and non-ASCII; `format_timestamp`; `parse_frontmatter` on missing/no-frontmatter/unterminated/`a: b: c`/BOM/CRLF/NBSP-terminator, on invalid UTF-8, and on `str.splitlines` boundaries; `collect_todos`/`collect_done`/`collect_reviews` on a 19-file tree; `derive_base_from`; `parse_sections`; and the item-(5) GOLDEN — `render()` byte-equal to `flux_status.py --no-color` for the full panel, six section subsets, `--sections ""`, an empty base, a 57-char name past the `min(name_w, 50)` cap and the missing-base line), `tests/flux_003_renderers.rs` (9 — `PIPELINE_HEADING` 18-row and `SLASH_CMD` tables, `parse_arg`, `render_doc` goldens on a synthetic `pipeline.md` hitting every `parse_pipelines` branch plus both empty-state lines, the VENDORED `pipeline.md` golden, `SLASH_CMD_RE` 19-row table, the `/flux/about` body, and item (8) — the four `_docs`/`reference` pairs byte-equal), `tests/flux_003_host_surfaces.rs` (14 — overlay text == `/flux/status` text line-for-line, span colours, `tick`/ESC, `open_status_overlay`'s three outcomes, `ask_user_question` metadata/refusals/single/multi/pre-cancelled, `execute_command` routing incl. both Python error wordings). Item (7) was already pinned by FLUX-001's `flux_001_embedded_bundle.rs`. Seams added, no behaviour change: `state::derive_base_from(env, cwd)` (`derive_base` is now the shell), `overlay::FluxStatusOverlay::with_base`, `render_cheatsheet::render_doc` + `match_pipeline_heading`/`strip_slashes` `pub`, `render_about::normalize_slash_cmd` `pub`. **Six defects the pins exposed, fixed in the same commit — 6 of 59 red at the pre-fix tree (seams in, fixes out), 59/59 after, three consecutive runs, no LEAK:** (a) FLUX-006 — `parse_frontmatter` now `fs::read` + `from_utf8_lossy` (`flux_status.py:100` `errors="replace"`); (b) `state::splitlines` ports `str.splitlines()`'s boundary table (`:105`; `\r`, `\x0b`, `\x0c`, `\x1c`-`\x1e`, U+0085, U+2028/9) — a lone-`\r` task file parsed to `{}` here and to a full map upstream; (c) `render_about::is_word` was ASCII where Python `\w` is Unicode (`//é`/`é//x` inverted); (d) `/flux/cheatsheet`'s error printed `"e"` where `flux_cheatsheet.py:208` `{...!r}` prints `'e'`; (e) the overlay padded review rows to full width where `flux_status.py:267` `rstrip()`s — it now stops at the dot, same text as `/flux/status` span-for-span; (f) **FLUX-005 had already materialised**: `f239fc3d` (2026-09-04, after this file's re-audit) edited `_docs/README.md:140` and not `skills/flux/reference/README.md:140` — re-synced here; the byte-equality test is what caught it. Upstream lines: `flux_status.py:84-86`, `:89-93`, `:96-112`, `:129-178`, `:181-270`, `:309-338`; `flux_cheatsheet.py:64-65`, `:76-82`, `:85-131`, `:144-164`, `:203-241`; `flux_about.py:53`, `:58-61`. **Residual (low):** `/flux/status`'s valid path and `flux_extension_for_env` read the process environment (`derive_base()`, `CYRUP_SUBAGENT_CHILD`) and are pinned through their pure seams, not end-to-end (edition-2024 `set_var` is unsafe and the workspace forbids it); the `ask_user_question` lock HOLD across a dialog (a second prompt must wait) is not exercised; `\s`/`\w`/`str.strip()` are approximated by `char::is_whitespace`/`is_alphanumeric`/`trim`, which agree with Python on every table value but not on every code point (U+001C-U+001F are `isspace()` in Python, not `White_Space`). `cyrup-sdk` (940 lines) remains the workspace's one untested crate — not this area's. |
| ~~FLUX-004~~ | ~~medium~~ **CLOSED 2026-09-04** | cyrup-original | S | `ctrl+f` silently takes the editor's `tui.editor.cursorRight` away from the user, with no diagnostic and no rebind path — the first LIVE instance of `EXT-039`'s open residual. **CLOSED 2026-09-04, cyrup `c846ff97`.** Landed as the row's **Fix (2)**, the in-crate half: the overlay chord is now `ctrl+alt+f`, held in ONE constant `crates/cyrup-flux/src/extension.rs` `STATUS_OVERLAY_SHORTCUT` (+ `STATUS_OVERLAY_SHORTCUT_DESCRIPTION`) that both `FluxExtension::init`'s `register_shortcut` and `execute_shortcut`'s match read, so the two sites cannot drift. `flux_bootstrap/` @v0.0.40 registers no keybinding (the chord is a cyrup design choice — labelled inference), so the invariant is "bound by no default keymap" and it is pinned against the REAL tables, not a copied list: `crates/cyrup-flux/tests/flux_004_status_shortcut.rs` (4 tests; `cyrup-tui` added as a DEV-dependency only) parses the constant with `Key::parse` (the same filter `App::set_extension_shortcuts` applies) and asserts `Keymap`/`EditorKeymap`/`SelectKeymap`/`AutocompleteKeymap`/`ModelsKeymap`/`SessionKeymap`/`TreeKeymap`/`AltScreenKeymap` `::default().action_for(..)` are all `None`, that `ExtensionRegistry::resolve_shortcuts` against `tui.editor.cursorLeft/Right`'s pi defaults yields no diagnostic, that `ExtensionHost::load_native` registers exactly the constant with its `/hotkeys` description, and that `execute_shortcut` opens the overlay for the constant and ignores the retired `ctrl+f`. **Red before / green after:** the constant was introduced at the OLD value first, so 3 of 4 failed as assertions (`EditorKeymap::default() already binds "ctrl+f" to Some(CursorRight)`; the registry's own rule-3 text `'ctrl+f' is built-in shortcut for tui.editor.cursorRight and cyrup-flux. Using cyrup-flux.`; the retired chord reaching the overlay); `cargo nextest run -p cyrup-flux` 22/22 after. Why `ctrl+alt+f`: no cyrup default table binds any `ctrl+alt+<letter>`; pi v0.84.4's only `ctrl+alt` default is `ctrl+alt+]` (`packages/tui/src/keybindings.ts:110-112`); `ctrl+shift+f` rejected because it IS pi's `tui.altScreen.search` (`:192-195`; `core/keybindings.ts:88-91` makes it plain `ctrl+f` under `windowsKeybindings`) and needs the kitty protocol, while `ctrl+alt+f` arrives as `ESC 0x06` on a legacy terminal like the `alt+b`/`alt+f` motions already do. Docs follow (`docs/guide/extensions/flux.md`, `spec/flux.md` §3.4.3). **Residual — NOT re-filed here (medium, `EXT-039`, areas 06/07):** **Fix (1)** is untouched — `resolve_shortcuts` still has no production caller (`crates/cyrup/src/interactive.rs` `set_extension_shortcuts(..shortcut_specs())`, `crates/cyrup-tui/src/app/run_arms.rs` swap arm), so the next bundled extension can still take a key silently; this row's **Verify** line belongs to that half and was not executed. **Observation (not this crate's):** `cargo clippy -p cyrup-flux --all-targets -- -D warnings` now checks `cyrup-tui` too and trips a pre-existing HEAD lint, `crates/cyrup-tui/src/app/input_reader.rs:443` `clippy::redundant_closure`; with only that lint allowed (`-A clippy::redundant_closure`) the same deny-mode command is clean, `cargo clippy -p cyrup-flux --lib -- -D warnings` is clean, and `cargo check -p cyrup` (the sole consumer) builds. |
| ~~FLUX-005~~ | ~~low~~ **PARTIALLY CLOSED 2026-09-04** | cyrup-original | S | The four `_docs/*.md` reference files are byte-identical duplicates of `skills/flux/reference/*.md` with no sync mechanism, and one of them is additionally compiled into `/flux/cheatsheet`. **PARTIALLY CLOSED 2026-09-04, cyrup `4bb3569c` (with FLUX-003).** The row's "cheapest credible version" landed: `crates/cyrup-flux/tests/flux_003_renderers.rs::the_four_docs_reference_pairs_are_byte_identical` asserts the four pairs byte-equal through the embedded bundle (`bundle::bundled_file`) and pins the census (`_docs` = 5 files, `reference` = 4, `about.md` the odd one). It was RED on its first run: the pairs were NOT identical any more — `f239fc3d` (2026-09-04, after this file's 2026-09-04 re-audit said "still byte-identical") edited `_docs/README.md:140` alone ("No tests, no benchmarks" -> "Tests are in scope"); `skills/flux/reference/README.md:140` is now re-synced to the `_docs` text, so `/skill:flux` and the installed tree say the same thing again. Verify satisfied: a one-character difference in either copy fails the test. **Open (low):** the duplication itself stands (no build-time mirror; the test is the sync mechanism), and the fifth-file note the Fix asks for (`about.md`'s missing twin, `spec/flux.md:441-442`) was not written. |
| ~~FLUX-006~~ | ~~low~~ **CLOSED 2026-09-04** | parity-bug | S | `parse_frontmatter` is UTF-8-strict where the Python is `errors="replace"`, so one bad byte turns a task file's whole frontmatter into an empty map instead of a parsed one. **CLOSED 2026-09-04, cyrup `4bb3569c` (with FLUX-003).** Exactly the row's one-line Fix: `crates/cyrup-flux/src/state.rs` `parse_frontmatter` now reads bytes (`std::fs::read`) and decodes with `String::from_utf8_lossy` — `flux_status.py:100` `errors="replace"` @v0.0.40 (byte-identical to the v0.0.6 line cited below). The row's Verify is `tests/flux_003_state_and_status.rs::parse_frontmatter_decodes_invalid_utf8_lossily_like_errors_replace` (a bad byte inside the block -> `caf\u{fffd}`, a bad byte after the block leaves both keys intact, a truncated multi-byte sequence -> one U+FFFD; every expected value is the Python's), red before (`{}`) / green after. The same table exposed a second tolerance gap the row did not name — `str::lines` vs `str.splitlines()` (`:105`) — closed in the same commit by `state::splitlines`, pinned by `parse_frontmatter_splits_lines_where_python_splitlines_does`. |
| FLUX-007 | low | tooling | S | The crate still records no upstream baseline version in-source, and its four `tmp/code-puppy` doc-comment links still resolve nowhere — confirmed this pass to be wrong on two axes (directory name and a missing path segment) even now that both upstreams are cloned in-workspace — and 13 `FLUX_NN`/`§5.6` citations still point at spec files that do not exist |

---

## ~~FLUX-001~~ — The bundled prompt/skill tree resolves through a build-machine path, so a distributed binary silently ships half a feature — **CLOSED 2026-09-04**

**Kind** cyrup-original · **Severity** ~~medium~~ · **Effort** S · **Confidence** confirmed
**CLOSED 2026-09-04, cyrup `03f3add0`** (`fix(flux): FLUX-001 embed the bundled prompt/skill tree and materialise it under the agent dir`).
Landed as **Fix (a)** below — embed, then materialise on first `ResourcesDiscover` — with **(b)** layered on
top rather than instead, exactly as the row prescribes. **Embed:** `crates/cyrup-flux/build.rs` walks
`resources/` at build time (no hand-maintained list; `rerun-if-changed` on the directory and every file)
and generates `$OUT_DIR/bundled.rs`, which `src/bundle.rs` includes as `BUNDLED_FILES: &[BundledFile
{rel, bytes}]`, sorted; `bundled_files()`, `bundled_file(rel)`, `sha256_hex`, `bundle_fingerprint()`
(sha256 over every rel+payload, once per process). **Materialise:** `src/install.rs` is the port of
`code_puppy_core_plugins/flux_bootstrap/installer.py` @v0.0.40 that `spec/flux.md` §0 had recorded as
"deleted, replaced by `cyrup install`" — `decide(current, recorded, payload) -> FileAction {Install,
Unchanged, PreserveForeign, Overwrite, BackupThenOverwrite}` is the pure per-file table of `:186-218`;
`install_pass(root, files, marker)` is `_install_pass` (`:170-222`) with tmp+rename atomic writes
(`:83-91`, `:94-110`), `unique_backup_path` (`:113-127`), manifest `.flux_bootstrap_manifest.json`
(`:139-149`) and marker `.flux_bootstrap_version` (`:152-167`) written last; `ensure_installed(root)
-> InstallOutcome {UpToDate, Installed(InstallReport), SkippedLocked}` is `install_bundled_commands`
(`:225-256`) plus the `needs_install` gate its caller applies, with an `fs4` non-blocking flock on
`.flux_bootstrap.lock` (`:244-256`); `InstallReport::summary()` keeps `:68-72`'s wording. **Resolve
once:** `src/resources.rs` replaces `bundled_dir()` / `bundled_prompts_dir()` / `bundled_skill_md()`
with `BundledRoot::{Vendored(PathBuf), Managed(PathBuf)}` decided at construction by `resolve_from(agent_dir,
env)` — `CYRUP_FLUX_RESOURCES_DIR` (non-blank) names a vendored tree read as-is and never written;
otherwise `managed_root(agent_dir) = <agent_dir>/flux/resources`, an extension-owned sibling of
`<agent_dir>/intercom` and `<agent_dir>/subagents`, deliberately NOT the scanned `<agent_dir>/prompts`
+ `skills` (upstream's literal config-dir-root layout) because cyrup's user-scope loader already walks
those and every template would register twice. The `CARGO_MANIFEST_DIR` fallback no longer exists in
this crate. **Loud miss:** `src/extension.rs` `on_event(ResourcesDiscover)` now calls
`materialise_bundle()` first (`register_callbacks.py:47-68` `_install_flux_commands`, notices
"Flux commands installed -> …" Info / "Backed up locally-modified Flux files (see *.bak): …" Warning /
"Flux bootstrap skipped (install failed): …" Warning, minus the command-cache rescan `:64-66` which has
no cyrup counterpart), then contributes the prompt DIRECTORY and skill FILE as before — and when either
is missing it emits a Warning naming the path tried (`flux: bundled prompt templates not found at …`,
`flux: bundled skill not found at …`) instead of the silent `Noop` cited below. **Seam:** `lib.rs`
`flux_extension(agent_dir)`, `flux_extension_for_env(agent_dir)`, `flux_extension_with_root(root)`;
`crates/cyrup/src/session_launch.rs:138` passes `dirs.agent_dir` — the same value `cyrup_mcp` and
`cyrup_permission_system` receive at `:128`/`:132` — so no second home/agent-dir ladder was added.
**Upstream** re-read at **v0.0.40** (ADR-0006; `git -C tmp/code_puppy_core_plugins diff --stat v0.0.6
v0.0.40 -- code_puppy_core_plugins/flux_bootstrap/` is empty, so every `installer.py` citation below
holds at both tags). **Design decisions** (recorded in the commit body): invariant = the contributed
directory comes from exactly one of two rules whose inputs are known at construction → `BundledRoot`
enum at the boundary; per-file decision as a pure domain enum (FC/IS); `InstallOutcome` names what
upstream encodes as an early return and an empty report. Rejected: (b) alone; installing into the
scanned `prompts/`/`skills/` roots; a hand-listed `include_str!` table; a typestate over on-disk
state; a runtime `include_dir` dependency. **Labelled inference, not an upstream rule:** the marker is
`<CARGO_PKG_VERSION>+<bundle sha256>` rather than the version string alone
(`register_callbacks.py:38-44`) — same steady-state cost, and a same-version rebuild with an edited
template (every from-source development build) re-installs. Mode-bit preservation (`:94-110`) has no
counterpart (markdown only, embedded bytes carry no mode). **Tests, red before / green after:**
`crates/cyrup-flux/tests/flux_001_embedded_bundle.rs` (12) —
`the_embedded_bundle_is_exactly_the_on_disk_resources_tree` (byte-equal to `resources/**`, sorted; the
one place a test still reads `CARGO_MANIFEST_DIR`, as the reference not the runtime),
`the_embedded_bundle_holds_fifteen_templates_and_the_skill`,
`the_root_is_managed_under_the_agent_dir_unless_a_vendored_tree_is_named` (incl. blank override, vendored
never written), `the_per_file_decision_matches_installer_py`,
`a_fresh_install_materialises_every_file_then_is_a_no_op` (the row's cheaper Verify: 15 `.md` under
`prompts/flux/` + `skills/flux/SKILL.md` resolved with no reference to the build tree; manifest + marker;
`UpToDate` on the second run; idempotent forced pass),
`a_hand_edited_managed_file_is_backed_up_and_a_foreign_file_is_preserved` (`.bak` then `.bak.1`; foreign
file never claimed), `a_changed_bundle_overwrites_untouched_managed_files_without_backups`,
`a_held_install_lock_skips_the_pass_instead_of_racing`,
`resources_discover_contributes_the_managed_root_not_the_build_tree` (one Info notice, then steady-state
with none), `a_missing_vendored_tree_is_reported_not_silently_dropped` (Noop + two Warnings naming the
path), `an_unwritable_managed_root_is_reported_and_the_session_survives`,
`a_subagent_child_still_gets_no_flux_extension`. Red before: a probe of the seam test against the
pre-change crate (old `flux_extension()`, same event, same assertion) failed with `contributed the
build-machine source tree: /home/user/cyrup/crates/cyrup-flux/resources/prompts`; the rest name types
that did not exist. Green after: `cargo nextest run -p cyrup-flux` **18/18** (12 new + FLUX-002's 6,
whose helpers now read `bundle::bundled_file` — assertions unchanged). `cargo fmt --all -- --check`,
`cargo clippy -p cyrup-flux --all-targets -- -D warnings`, `cargo clippy -p cyrup --no-deps
--all-targets -- -D warnings` and `RUSTDOCFLAGS='-D warnings' cargo doc -p cyrup-flux --no-deps` all
clean (the full-deps `cyrup` clippy fails only on a concurrent track's
`crates/cyrup-tui/src/app/input_reader.rs:443`, untouched here). `spec/flux.md` (the `bundled_dir()`
note and the installer row of the rename map) and `docs/guide/extensions/flux.md` record the change.
**Residual (medium — the WORKSPACE-WIDE CLASS below, still unfiled elsewhere):** the three sibling
resolvers in `cyrup-ext-subagents` and `cyrup-intercom` are untouched — other tracks own those crates
this batch; the build.rs table + `install.rs` shape is reusable for them as-is, so "one mechanism
closes all four" remains available but is not claimed. **Residual (low):** the row's full **Verify**
(release binary moved off-tree, `/flux/new` resolves, `/skill:flux` loads) was not executed live; what
is pinned is the extension seam's contributed path plus the materialised tree's content. Note the
`CYRUP_FLUX_RESOURCES_DIR` census below is now stale by design: `resources.rs` still declares it, and
it is now the `Vendored` arm.
**cyrup** — `crates/cyrup-flux/src/resources.rs:19-23` — `bundled_dir()` reads
`CYRUP_FLUX_RESOURCES_DIR` (`:14`) and otherwise falls back to
`PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources")`, i.e. **the absolute path of the source
tree on whatever machine compiled the binary**. `bundled_prompts_dir()` (`:27-29`) and
`bundled_skill_md()` (`:33-35`) both hang off it. The contribution is then guarded and **silent**:
`extension.rs:135` inserts `promptPaths` only `if prompts.is_dir()`, `:146` inserts `skillPaths` only
`if skill.is_file()`, and `:152-153` returns `HookOutcome::Noop` when the payload came out empty — no
error, no `notify`, no startup diagnostic. Nothing at HEAD sets the env var: `grep -rn
'CYRUP_FLUX_RESOURCES_DIR' .` over the whole tree (less `target/` and the gitignored `tmp/`) returns
exactly three hits — `resources.rs:4` and `:14`, both declaring it, and `spec/flux.md:629`,
describing it. No build script, packaging step, installer or CI job sets it.
**upstream** — `code_puppy_core_plugins/flux_bootstrap/installer.py` @v0.0.6 is a real install
mechanism, and its own docstring enumerates the guarantees cyrup dropped: it copies
`bundled/commands` + `bundled/scripts` into `~/.code_puppy` so "`/flux/...` commands are available out
of the box", and it is **idempotent** (SHA-256 manifest `.flux_bootstrap_manifest.json`),
**non-destructive** (`<name>.bak` before overwriting a hand-edited file), **version-gated**
(`.flux_bootstrap_version` marker) and **fails closed, never fatal**, under a best-effort `fcntl`
cross-process lock. `spec/flux.md:847` records the decision to delete it — "replaced by `cyrup
install` (Phase 1) + built-in registration (Phase 2) | no copy/manifest/version-gate" — but **`cyrup
install` is the EXTENSION installer** (`crates/cyrup/src/cli.rs:946`, "Install extension source and
add to settings"); it does not vendor `resources/` anywhere and does not set the env var. The
replacement named in the spec does not perform the function the spec assigns it.
**Impact** — On any binary whose build-time `CARGO_MANIFEST_DIR` is not present at run time — a
release artifact copied to another machine, a container image, a `cargo install` from a registry
source dir that is later cleaned — **the fifteen `/flux/*` commands and the `flux` skill simply are
not there**, and the user is told nothing. What makes this worse than a plain missing feature is that
the failure is ASYMMETRIC, so the product actively lies about itself: `/flux/status`,
`/flux/cheatsheet`, `/flux/about`, the `ctrl+f` overlay and `ask_user_question` all still register
(they are `include_str!`-compiled or code — `render_cheatsheet.rs:13`, `render_about.rs:12`), and
`/flux/cheatsheet` renders its full command table naming `/flux/config`, `/flux/new`, `/flux/ask`,
`/flux/split`, `/flux/aug`, `/flux/exec`, `/flux/qa`, `/flux/tests`, `/flux/commit`,
`/flux/create-pr` and `/flux/review`
(`crates/cyrup-flux/resources/prompts/flux/_docs/pipeline.md:21-31`) — **every one of which is now a
"command not found"**. This directly falsifies two shipped statements: the repository README
(`../../README.md:85-87`, not this directory's) ("Flux's
whole point is to work with no install step at all") and `docs/guide/extensions/flux.md:8-12` ("It is
a native extension compiled into the binary and attached unconditionally at startup — the whole point
of moving it into cyrup is that it works with no install step"). Flux is the instance that matters
because, per `../../README.md:88-91`, it is the ONLY one of the four native extensions that is
unconditionally on; the other three are opt-in and a user who never armed them never notices.
**WORKSPACE-WIDE CLASS, unfiled anywhere:** the identical `unwrap_or_else(|| PathBuf::from(env!(
"CARGO_MANIFEST_DIR")).join("resources"))` shape is at
`crates/cyrup-ext-subagents/src/extension.rs:5750`,
`crates/cyrup-ext-subagents/src/registration/resources.rs:46` and
`crates/cyrup-intercom/src/resources.rs:41`. `grep -rn 'CARGO_MANIFEST_DIR' docs/gap-analysis/*.md`
returns only `01-cyrup-core-and-provider.md:985`, an unrelated test-time suggestion.
**Fix** — Embed rather than resolve. `crates/cyrup-flux/Cargo.toml:13-17` already declares
`include = ["src/**/*.rs", "resources/**", "Cargo.toml"]`, so the tree is in the package; make it in
the BINARY too. Either (a) `include_dir!` the `resources/` tree and materialise it under
`dirs.agent_dir` on first `ResourcesDiscover` — which restores upstream's install-once shape and its
`~/.code_puppy` analogue in one move — or (b) keep the path resolution but make the miss LOUD: when
neither `prompts.is_dir()` nor `skill.is_file()` holds, push a startup diagnostic naming the path
tried, instead of `HookOutcome::Noop` at `extension.rs:152-153`. **(b) alone is not sufficient** and
should not be landed as the whole fix — a diagnostic the user cannot act on still leaves the feature
gone. Do (a) here and reuse the shape for the three sibling sites; one mechanism closes all four.
**Verify** — Build a release binary, move it to a directory tree with no `crates/cyrup-flux/`
anywhere above it, unset `CYRUP_FLUX_RESOURCES_DIR`, run it, and assert `/flux/new` resolves and
`/skill:flux` loads. A cheaper unit-level version: assert `bundled_prompts_dir()` resolves to
15 `.md` files and `bundled_skill_md()` to an existing file with the env var unset and the manifest
dir renamed. Neither test exists today — see `FLUX-003`.

## ~~FLUX-002~~ — Four templates instruct a tool that is off by default, in an extension that is on by default — **CLOSED 2026-09-04**

**Kind** parity-bug · **Severity** ~~medium~~ · **Effort** S · **Confidence** confirmed
**CLOSED 2026-09-04, cyrup `c7d21bbb`** (`fix(flux): FLUX-002 gate the multi-task fan-out on `subagent` tool availability`).
Landed exactly as **Fix** below prescribes — the four-file template edit — and the alternative it says to
REJECT (arming subagents from flux) was rejected. What each template now says, first, in its multi-task
branch: check the tool list for `subagent` BEFORE calling it; if it is absent, do NOT call it and do NOT
substitute another tool, tell the user ONCE that it is not available (naming the opt-in:
`CYRUP_SUBAGENTS=1` or a `subagents/config.json` at user or project scope), then run every
`$FLUX_BASE/todo/*.md` task in SINGLE-TASK MODE one after another exactly as `all` does
(`exec`/`aug`/`qa`) or review each group in-line, one group at a time, with the same sub-agent prompt
template (`review`, whose STEP 7 second launch is covered too). The dispatch block's pure-integer `ELSE:`
branch carries the same condition so the model never commits to fan-out before checking. Landed lines:
`crates/cyrup-flux/resources/prompts/flux/exec.md:48-49` + `:181`, `aug.md:50-51` + `:158`,
`qa.md:48-49` + `:163`, `review.md:109` + `:192`; `resources/skills/flux/SKILL.md:56` (the system-prompt
table's `N` row); `docs/guide/extensions/flux.md:52-55`; `spec/flux.md` §0.3 (the note the **Fix** asks
for, next to the rename map). The armed path — one `subagent` call with `tasks: [...]` and
`concurrency: $ARGUMENTS` — is unchanged. Nothing in the wiring moved: `is_installed` /
`is_installed_with` at `crates/cyrup-ext-subagents/src/extension/host/registration.rs:99-129`,
`TOOL_NAME = "subagent"` at `extension/mod.rs:104`, and flux still attaches unconditionally — the seam
is now `crates/cyrup/src/session_launch.rs:136` (`attach_native_extensions`, item 7 of its doc comment),
not the three `main.rs` lines cited below, which moved in the interim. **Upstream re-read at v0.0.40**
(ADR-0006; `flux_bootstrap/` is byte-identical to the v0.0.6 baseline): `code_puppy_core_plugins/`
`flux_bootstrap/bundled/commands/flux/exec.md:181`, `aug.md:158`, `qa.md:163`, `review.md:110` name
`invoke_agent` with no availability check, which is correct there and only there. **Tests, red before /
green after:** `crates/cyrup-flux/tests/flux_002_subagent_fallback.rs` (read through
`resources::bundled_prompts_dir()` / `bundled_skill_md()`, the same resolver `ResourcesDiscover`
contributes) — against the unedited templates 4 of 6 failed:
`every_multi_task_template_checks_tool_availability_before_calling_subagent` (the shared trigger
sentence, the once-only notice, the no-call/no-substitute rule, the named opt-in),
`the_three_n_argument_templates_route_a_missing_tool_to_the_sequential_path_in_dispatch`,
`review_degrades_in_line_and_covers_its_second_subagent_launch_in_step_7`,
`the_skill_tells_the_model_the_n_mode_needs_the_subagent_tool`; 6/6 after. The two that passed both
times — `every_multi_task_template_still_names_the_subagent_tool_for_the_armed_path` and
`only_the_four_multi_task_templates_name_the_subagent_tool` (the `4 — aug, exec, qa, review` census) — are
regression pins, not closure evidence. `cargo nextest run -p cyrup-flux` 6/6, clippy `--all-targets -D
warnings` and `RUSTDOCFLAGS='-D warnings' cargo doc` clean, `cargo check -p cyrup` clean. This is the
crate's first test binary; `FLUX-003` is NOT claimed by it. **Residual (low):** the **Verify** below is a
live-model observation and was not executed — what is pinned is the prompt contract; whether a given
model honours a tool-list check is not testable here. A harness-level degrade (flux observing the
subagent gate and rewriting or selecting templates) would need a new seam — prompt templates are static
files contributed by directory via `ResourcesDiscover` — and is not filed: the row's own **Fix** names the
template edit as the correct shape.
**cyrup** — Flux attaches unconditionally at all three `AppMode` seams:
`crates/cyrup/src/main.rs:726`, `:930`, `:1060`, each `if let Some(ext) =
cyrup_flux::flux_extension_for_env()`, whose only `None` is a subagent CHILD
(`crates/cyrup-flux/src/lib.rs:49-53`). The `subagent` tool does not: it is
`cyrup_ext_subagents::extension::TOOL_NAME` (`crates/cyrup-ext-subagents/src/extension.rs:97`),
registered by `subagent_extension_for_env` (`main.rs:676`/`:684`) behind the opt-in gate the comment
at `main.rs:651-658` spells out — "a plain TOP-LEVEL session attaches the orchestrator surface ONLY
when opted in (`is_installed`: `CYRUP_SUBAGENTS` truthy, or a `subagents/config.json` at user/project
scope)". Four of the fifteen templates then tell the model to call it as a plain fact:
`resources/prompts/flux/exec.md:180` and `:199` ("Use the `subagent` tool — parallel foreground calls
only; NEVER background", then "Issue **one** `subagent` call with `tasks: [...]` … and `concurrency:
$ARGUMENTS`"), `aug.md:157`, `qa.md:162`, `review.md:109`.
**upstream** — code-puppy's counterpart is a CORE tool, not an opt-in subsystem:
`code_puppy/tools/__init__.py:36-40` @`v0.0.720` puts `"invoke_agent": register_invoke_agent` and
`"invoke_agent_with_model"` in the module-level `TOOL_REGISTRY`, implemented in
`code_puppy/tools/subagent_invocation.py:650`/`:696`. Upstream's `exec.md`/`aug.md`/`qa.md`/`review.md`
therefore name a tool that is always present.
**Impact** — On a default install — no `CYRUP_SUBAGENTS`, no `subagents/config.json` — `/flux/exec 3`,
`/flux/aug 2`, `/flux/qa 2` and `/flux/review` reach their multi-task branch and instruct the model to
call a tool that is not in its tool list. The model gets a tool-not-found (or, worse, hallucinates a
substitute), and the user sees the flagship parallel-execution feature fail in the middle of a
pipeline run with no explanation naming the real cause. `/flux/exec`'s own single-task and `all`
branches (`exec.md:30-49`) explicitly describe a no-subagent sequential path, so a correct degrade
EXISTS and is simply not selected — the template branches on `$ARGUMENTS`, never on tool
availability. **This is a defect the port created, not one it inherited:** `spec/flux.md:118-129`
records the rename map (`| invoke_agent | subagent | 4 — aug, exec, qa, review |`, `:125`) and
`spec/flux.md:380` calls the two "equivalent, richer", but neither the rename map nor
`spec/flux.md:524`'s "Default-on is the decision (not an option)" ever mentions that the tool being
renamed ONTO is default-off. Two correct decisions taken independently compose into a broken one.
**The spec's own acceptance step would have caught it and cannot run:** `spec/flux.md:932-933`
requires "`/flux/exec 2` on a two-task fixture runs both tasks via foreground `subagent` fan-out",
which passes only with `CYRUP_SUBAGENTS` set — a condition the DoD never states, so the check reads
as green on the one machine that had the gate armed.
**Fix** — Cheapest correct fix is in the templates, not the wiring: add an availability
pre-condition to the multi-task branch of `exec.md`, `aug.md`, `qa.md` and `review.md` — "if the
`subagent` tool is not available, fall back to the sequential single-task path above and say so
once" — which is a four-file edit and reuses a path each template already documents. The alternative
is to arm subagents whenever flux needs them, and it should be REJECTED unless an owner signs it off:
`main.rs:651-658` gates an OS-process-spawning subsystem, and having a default-on extension quietly
flip that gate is a bigger change than the defect. Whichever is chosen, record it in
`spec/flux.md:118-129` next to the rename map, because that table is where the next reader will look.
**Verify** — With `CYRUP_SUBAGENTS` unset and no `subagents/config.json`, `/flux/exec 3` on a base
with three todo files must complete all three sequentially and emit one notice explaining the
degrade — not attempt a `subagent` call. With the gate armed, the same command must issue exactly one
`subagent` call carrying `tasks: [...]` and `concurrency: 3`.

## ~~FLUX-003~~ — The crate has no tests: 1,513 lines, nine modules, and nothing pinning the cross-harness state contract the port calls a requirement — **CLOSED 2026-09-04**

**Kind** test-defect · **Severity** ~~medium~~ · **Effort** M · **Confidence** confirmed
**CLOSED 2026-09-04, cyrup `4bb3569c`** (`test(flux): FLUX-003 pin the crate against the upstream Python
and fix what the pins exposed`). The minimum set below landed as three test binaries (37 tests) whose
every expected value is the OUTPUT of the upstream scripts at **v0.0.40** — extracted with `git -C
tmp/code_puppy_core_plugins show v0.0.40:code_puppy_core_plugins/flux_bootstrap/bundled/scripts/<name>.py`
and run (`--no-color`, `--base`/`--docs` at the fixture trees the tests write) or imported (the pure
tables); each file's header carries the regeneration recipe. Items: (1)-(6) in
`crates/cyrup-flux/tests/flux_003_state_and_status.rs`, with (5) — the one the Verify says to write
first — as `render_status_matches_flux_status_py_no_color_on_the_small_tree` (+ the 50-cap, empty-base
and missing-base goldens); (6) also covers the cheatsheet's `parse_arg`; (7) was already pinned by
FLUX-001's `flux_001_embedded_bundle.rs` (`BundledRoot::resolve_from`); (8) in
`tests/flux_003_renderers.rs` (`the_four_docs_reference_pairs_are_byte_identical`), which is where
FLUX-005 turned out to have ALREADY materialised (`f239fc3d`, see its row). The overlay and the tool —
named in the title, not in the list — are in `tests/flux_003_host_surfaces.rs` through a scripted
`HostServices`. Testability seams (no behaviour change): `state::derive_base_from(env, cwd)`,
`overlay::FluxStatusOverlay::with_base`, `render_cheatsheet::render_doc` (+ `match_pipeline_heading`,
`strip_slashes` `pub`), `render_about::normalize_slash_cmd` `pub`. **Red before / green after:** with the
seams present and the fixes absent, 6 of 59 tests failed — FLUX-006 (`read_to_string`), `str::lines` vs
`str.splitlines()`, ASCII `is_word` vs Unicode `\w`, the `"e"`/`'e'` cheatsheet wording, the overlay's
un-`rstrip()`ped review rows, and the diverged README pair — and every one is fixed in the same commit
(details in the `## Open items` row); 59/59 after, three consecutive `cargo nextest run -p cyrup-flux`
runs, no LEAK. `cargo test -p cyrup-flux` now runs a non-zero number of tests, as the Verify asks.
**Residual (low):** see the row — the two env-reading paths are pinned through pure seams only, the
interaction-lock hold is not exercised, and the `\s`/`\w`/`strip()` approximations agree on every
table value but not on every code point.
**cyrup** — `grep -rn '#\[cfg(test)\]\|#\[test\]\|#\[tokio::test\]' crates/cyrup-flux/` returns
**0**; there is no `crates/cyrup-flux/tests/` directory; and `crates/cyrup-it/tests/` (120 files
across 8 subdirectories) has no flux entry. Counting files containing a test per crate,
`cyrup-flux` is **0** against `cyrup-tui` 126, `cyrup-ext-subagents` 116, `cyrup-provider` 96,
`cyrup-session-svc` 49, `cyrup-intercom` 35, `cyrup-tools` 31, `cyrup-ext` 29 — down to
`cyrup-resources` 6 and `cyrup-test-support` 3. **One other crate is also zero — `cyrup-sdk`, at 940
lines** — so the accurate statement is that these are the workspace's only two untested crates, and
`cyrup-flux` is the larger, the one that renders user-visible output and the one that parses files
another harness writes.
**upstream** — `code_puppy_core_plugins` ships `tests/test_flux_bootstrap.py` @v0.0.6, covering the
half cyrup deleted (the installer). It has no counterpart for the renderers, which are `exec:`
scripts upstream.
**Impact** — Untested surfaces here are not incidental: `state.rs`'s five collectors and
`parse_frontmatter` are the shared read model for BOTH `/flux/status` and the overlay
(`state.rs:1-10`), `render_status.rs`'s column arithmetic reimplements six Python layout constants by
hand (`:16-31`, `:123-125`), `render_cheatsheet.rs` reimplements two regex parses as character
predicates (`:22-30`), `render_about.rs:20` reimplements a regex LOOKBEHIND as a hand-rolled
predicate, and `state.rs:20-38` reimplements `re.sub(r"[^a-zA-Z0-9]+", "-", cwd)` — which decides the
DIRECTORY NAME every task file lands in, so an off-by-one there silently splits a project's pipeline
state into two trees. Nothing pins any of it, and nothing pins the interop contract `lib.rs:13-14`
states as the crate's purpose ("State on disk (`~/.flux/<flattened-cwd>/`) stays byte-identical to
code-puppy's so one project's task tree is readable by both harnesses"). `FLUX-005` and `FLUX-006`
below are both defects a single table test would have caught.
**Fix** — `spec/flux/README.md:73` says "**No tests to be written** — another team owns tests", and
`:76` makes every definition of done manual ("one manual run-through, not a test suite"), which
`spec/flux.md:920-933` then spells out as a list of things a human types. **That is a disclosure, not
a closure**, and it is exactly the class `00-residual-ledger.md` calls out: a deferral nobody signed
off on is an incomplete build. Note the manual DoD is also partly unrunnable as written — its last
line, "**Parallel exec**: `/flux/exec 2` on a two-task fixture runs both tasks via foreground
`subagent` fan-out" (`spec/flux.md:932-933`), silently assumes the gate `FLUX-002` names is armed. The
minimum set is small and needs no fixtures beyond a `tempfile` tree: (1) `flatten_cwd` against the
Python regex on leading/trailing/consecutive-separator cases; (2) `parse_frontmatter` on
missing-file / no-frontmatter / unterminated-block / `value: with: colons` / non-UTF-8 (`FLUX-006`);
(3) `format_timestamp` 5-part and pass-through; (4) `collect_done` reverse-sort + `"completed"`
default; (5) a golden `render_status` panel against `flux_status.py --no-color` output for one fixed
tree; (6) `parse_sections` valid / invalid / empty; (7) `bundled_dir` env override (`FLUX-001`); (8)
byte-equality of the four `_docs` / `skills/flux/reference` pairs (`FLUX-005`).
**Verify** — `cargo test -p cyrup-flux` runs a non-zero number of tests and the count appears in the
merge gate. Item (5) is the one that matters most and the one to write first: it is the only test
that can fail when the hand-ported layout arithmetic drifts from the Python.

## ~~FLUX-004~~ — `ctrl+f` silently takes the editor's forward-char away, with no diagnostic and no rebind path — **CLOSED 2026-09-04**

**Kind** cyrup-original · **Severity** ~~medium~~ · **Effort** S · **Confidence** confirmed
**CLOSED 2026-09-04, cyrup `c846ff97`** (`fix(flux): FLUX-004 move the status overlay off the editor's ctrl+f onto ctrl+alt+f`).
Landed as **Fix (2)** below — the in-crate half — and NOT **Fix (1)**, which is `EXT-039`'s residual in
areas 06/07 and which this row itself says must not be re-filed here. **What changed:** the chord is
`ctrl+alt+f`, held in one constant, `crates/cyrup-flux/src/extension.rs` `STATUS_OVERLAY_SHORTCUT` (and
`STATUS_OVERLAY_SHORTCUT_DESCRIPTION`, the EXT-040 `/hotkeys` label), read by both
`FluxExtension::init`'s `api.register_shortcut(..)` and `execute_shortcut`'s `if key ==
STATUS_OVERLAY_SHORTCUT` — the register site and the dispatch site can no longer drift, which the
two `"ctrl+f"` literals cited below could. `crates/cyrup-flux/src/overlay.rs` and `build.rs` doc
comments point at the constant; `docs/guide/extensions/flux.md:21`/`:92` and `spec/flux.md` (§3.4.3
bullet, which records the reason) name the new chord. **Design decision** (cyrup-original row; the
brief's design guidance applied): `flux_bootstrap/` @v0.0.40 registers no keybinding at all (`git -C
tmp/code_puppy_core_plugins grep -i 'ctrl\|keybind\|shortcut\|hotkey' v0.0.40 -- flux_bootstrap` is
empty), so the chord is a cyrup choice and the invariant is "bound by no default keymap", first known
at chord-choice time — a compile-time constant, checked by a test against the real tables. Rejected:
a validated-chord newtype in `cyrup-ext` refusing built-in collisions (pi's rule 3 is warn-and-accept
by design, `extensions/runner.ts:522-528` @v0.83.0, ported at `registry.rs` `resolve_shortcuts`; a
type-level refusal would change what the user sees, and that seam is `EXT-039`'s); consulting the
keymap from the extension at `init` (it has no keymap access — that is the host's job, Fix (1));
changing the literal alone (captures nothing). What still rests on a test, not the compiler: the
cross-crate "not a default binding" property itself. Migration cost: none (additive constant; no
call site outside the crate names the chord); `cyrup-tui` is a DEV-dependency only, respecting the
production crate-boundary rule `cyrup-ext-subagents/Cargo.toml` states. **Why `ctrl+alt+f`**
(labelled inference — a design choice, not an upstream rule): keeps the mnemonic; no cyrup default
table binds any `ctrl+alt+<letter>` (`crates/cyrup-tui/src/keymap.rs`, every `impl Default`); pi
v0.84.4's only `ctrl+alt` default is `ctrl+alt+]` (`tui.editor.jumpBack`,
`packages/tui/src/keybindings.ts:110-112`), so a later port cannot collide; `ctrl+shift+f` was
rejected because it IS pi's `tui.altScreen.search` (`packages/tui/src/keybindings.ts:192-195`;
`core/keybindings.ts:88-91`, where `windowsKeybindings` makes it plain `ctrl+f`) — unported today,
the next collision tomorrow — and because it is invisible without the kitty protocol, whereas
`ctrl+alt+f` arrives as `ESC 0x06` (CONTROL|ALT + `f`) on a legacy terminal, the path the `alt+b`/
`alt+f` word motions already rely on; it is also the chord the workspace's EXT-035 fixture models
(`crates/cyrup-ext/src/tests/native_dispatch.rs:1328`). **Upstream re-read at v0.84.4** for the
collision itself: `packages/tui/src/keybindings.ts:82-89` (`tui.editor.cursorLeft` `["left",
"ctrl+b"]`, `tui.editor.cursorRight` `["right", "ctrl+f"]`), ported at `crates/cyrup-tui/src/keymap.rs`
`impl Default for EditorKeymap` (`ctrl('b') => CursorLeft`, `ctrl('f') => CursorRight`); the tier
order is unchanged at `crates/cyrup-tui/src/app/input.rs` (extension shortcuts "before the editor
(so the key never leaks in as text)"). **Tests, red before / green after:**
`crates/cyrup-flux/tests/flux_004_status_shortcut.rs`, 4 tests. The constant was introduced at the
OLD value `"ctrl+f"` first so the failures are assertions, not a missing symbol — 3 of 4 failed:
`the_status_shortcut_is_bound_by_no_default_keymap` (`EditorKeymap::default() already binds "ctrl+f"
to Some(CursorRight)`), `the_status_shortcut_resolves_with_no_conflict_against_the_editor_defaults`
(the registry's own rule-3 diagnostic, `Extension shortcut conflict: 'ctrl+f' is built-in shortcut for
tui.editor.cursorRight and cyrup-flux. Using cyrup-flux.` — i.e. exactly the `[Extension issues]`
line this row's **Verify** predicts, produced by the registry the host does not yet call), and
`execute_shortcut_routes_the_constant_and_ignores_the_retired_chord` (the retired chord reached the
overlay's `notify` fallback); `the_extension_registers_exactly_the_constant_chord` (real
`ExtensionHost::load_native` → `shortcut_specs()`) passes on both values and pins the contract.
After the flip: `cargo nextest run -p cyrup-flux` **22/22** (the 18 existing untouched);
`cargo fmt --all -- --check` clean; `RUSTDOCFLAGS='-D warnings' cargo doc -p cyrup-flux --no-deps`
clean. **Observation, not this crate's:** `cargo clippy -p cyrup-flux --all-targets -- -D warnings`
now lints `cyrup-tui` too (dev-dep) and trips a PRE-EXISTING HEAD lint there,
`crates/cyrup-tui/src/app/input_reader.rs:443` `clippy::redundant_closure` (last touched by the
workspace rustfmt commit; untouched here); cyrup-flux's own lib and test targets produce no
diagnostic: `cargo clippy -p cyrup-flux --lib -- -D warnings` is clean, and `cargo clippy -p
cyrup-flux --all-targets -- -D warnings -A clippy::redundant_closure` (only that foreign lint allowed)
is clean. `cargo check -p cyrup` (the sole consumer, `crates/cyrup/src/session_launch.rs`, which
names no chord) builds; the change is API-additive (one `pub const`, no signature change). **Residual — NOT re-filed here (medium; it is `EXT-039`, areas 06/07):** **Fix (1)**
is untouched: `resolve_shortcuts` still has no production caller — the install sites are now
`crates/cyrup/src/interactive.rs` (`app.set_extension_shortcuts(session.services().ext_host.shortcut_specs())`)
and `crates/cyrup-tui/src/app/run_arms.rs` (the swap arm's `set_extension_shortcuts`), not the
`main.rs:2017` / `run_arms.rs:276-277` lines cited below, which moved — so the next bundled extension
can still take a key silently and nothing reaches `[Extension issues]`; this row's **Verify** line
belongs to that half and was not executed.
**cyrup** — `crates/cyrup-flux/src/extension.rs:53` — `api.register_shortcut("ctrl+f", Some("Flux
status overlay".into()))`, in an extension that attaches unconditionally (`main.rs:726`/`:930`/`:1060`).
cyrup binds the same chord to `EditorAction::CursorRight` at
`crates/cyrup-tui/src/keymap.rs:1298`, inside `impl Default for EditorKeymap` (`:1282`), a faithful
port of pi. `Key::parse("ctrl+f")` (`keymap.rs:493-502`) produces exactly the `Key { code: Char('f'),
mods: CONTROL }` the editor table holds, so the match is exact. The extension tier fires FIRST:
`crates/cyrup-tui/src/app/input.rs:86-94` finds the registered shortcut and returns
`AppAction::ExtensionShortcut` "before the editor (so the key never leaks in as text)"; the shortcuts
are sourced at boot from `crates/cyrup/src/main.rs:2017` and re-sourced on session swap at
`crates/cyrup-tui/src/app/run_arms.rs:276-277`.
**upstream** — the PRECEDENCE is correct, and this row is not asking for it to change. pi's
`RESERVED_KEYBINDINGS_FOR_EXTENSION_CONFLICTS` (`extensions/runner.ts:70-89` @v0.83.0, ported
verbatim at `crates/cyrup-ext/src/registry.rs:1200-1219`) lists eighteen ids and
`tui.editor.cursorRight` is not among them, so pi's rule 3 says the extension wins. pi's
`tui.editor.cursorRight` is `{ defaultKeys: ["right", "ctrl+f"] }`
(`packages/tui/src/keybindings.ts:61-64` @v0.83.0, `defaultKeys` at `:62`). **What upstream also does
and cyrup does not is WARN:** rule 3 is warn-but-accept, and cyrup ported the warning —
`registry.rs:938-947` builds `"Extension shortcut conflict: '{key}' is built-in shortcut for {} and
{owner}. Using {owner}."` — but the function that emits it, `ExtensionRegistry::resolve_shortcuts`
(`registry.rs:908`), **has no production caller.** `grep -rn 'resolve_shortcuts' crates/
--include='*.rs'` returns hits only in `registry.rs`, its facade wrapper
`crates/cyrup-ext/src/facade.rs:1825-1830` (itself uncalled), and one test at
`crates/cyrup-ext/src/tests/payload_and_seam_parity.rs:470`.
**Impact** — Every cyrup user who uses emacs-style motion silently loses forward-char to a status
overlay the moment they type it, on a default install, and there is no way back: an extension
shortcut is not a rebindable `Keybinding`, which `crates/cyrup-tui/src/app/hotkeys.rs:102-105` states
in so many words ("the key cell is `formatKeyText(key, …)` over the REGISTERED id, not a keymap
lookup — an extension shortcut is not a rebindable `Keybinding`, so there is nothing to resolve it
against"), so no `keybindings.json` edit reaches it. **There is no supported way to turn flux off
either:** `flux_extension_for_env` (`crates/cyrup-flux/src/lib.rs:49-53`) has exactly one `None`
branch, `CYRUP_SUBAGENT_CHILD=1`, which is the subagent-child signal and not a user setting.
**This makes flux the FIRST LIVE INSTANCE of an already-open residual** — `06-cyrup-ext.md:251`
(`EXT-039`, PARTIALLY CLOSED 2026-08-14) reads "**RESIDUAL (area 07): call `resolve_shortcuts`,
invert `app/input.rs:81-93`, thread `shortcut_diagnostics` into
`startup_diagnostics.extensions`.**" — *quoted verbatim, and its middle citation is the right site:
`app/input.rs:81-93` spans the built-in dispatch (`:81-83`) and the extension-shortcut lookup
(`:86-93`) this row anchors above at `:86-94` — the tier boundary itself. The install sites that
residual means are `crates/cyrup/src/main.rs:2017` and
`crates/cyrup-tui/src/app/run_arms.rs:276-277`.* Until flux shipped, that residual cost nothing,
because no bundled extension registered a shortcut that collided. **This row is a cross-reference
and a severity input for `EXT-039`, NOT a duplicate filing** — the registry work is area 06/07's and
must not be re-filed here.
**Fix** — Two independent halves, and both are cheap. (1) Discharge `EXT-039`'s residual — call
`resolve_shortcuts` at the two sites that install shortcuts (`crates/cyrup/src/main.rs:2017` and
`crates/cyrup-tui/src/app/run_arms.rs:276-277`) and surface `shortcut_diagnostics` in the
`[Extension issues]` panel. That turns the theft into a visible, explained one. (2) In this crate,
pick a chord that is not already an editor motion — `ctrl+f` was chosen for "flux", and it is the one
emacs binding in the table a user is most likely to hold down. Landing (1) without (2) is acceptable
and is the higher-value half; landing (2) without (1) leaves the next bundled extension free to do
the same thing silently.
**Verify** — With flux attached, a `[Extension issues]` entry must read `Extension shortcut conflict:
'ctrl+f' is built-in shortcut for tui.editor.cursorRight and cyrup-flux. Using cyrup-flux.` The
registry side is already pinned — `shortcut_resolution_refuses_reserved_keys_warns_on_the_rest_and_records_every_diagnostic`
(`crates/cyrup-ext/src/tests/payload_and_seam_parity.rs:456-480`) exercises rules 2/3/4 including a
non-reserved built-in collision the extension wins. What is missing is an assertion that the TUI ever
CALLS it — a structural test over `main.rs`/`run_arms.rs` in the shape of
`crates/cyrup-tui/src/tests/run_loop_swap_arm_reachable.rs`.

## ~~FLUX-005~~ — The four reference docs are shipped twice, byte-identical, with nothing keeping them in sync — **PARTIALLY CLOSED 2026-09-04**

**Kind** cyrup-original · **Severity** ~~low~~ · **Effort** S · **Confidence** confirmed
**PARTIALLY CLOSED 2026-09-04, cyrup `4bb3569c` (with FLUX-003).** The Fix's "cheapest credible version"
— item (8) of FLUX-003's set — is `crates/cyrup-flux/tests/flux_003_renderers.rs::the_four_docs_reference_pairs_are_byte_identical`,
and its first run was RED: the "byte-identical pairs at HEAD" this section and the 2026-09-04 re-audit
note both record were no longer true, because `f239fc3d` had edited `_docs/README.md:140` without its
`reference/` twin — precisely the Impact paragraph's scenario, with `/skill:flux` and the installed
`_docs` disagreeing on whether tests are in scope. The reference copy is re-synced in the same commit.
Verify holds (a one-character difference in either copy fails the test). **Still open (low):** the
duplication itself, and the fifth-file note for `about.md` in `spec/flux.md:441-442`.
**cyrup** — `crates/cyrup-flux/resources/prompts/flux/_docs/{README,pipeline,cheatsheet,synopsis}.md`
and `crates/cyrup-flux/resources/skills/flux/reference/{README,pipeline,cheatsheet,synopsis}.md` are
**byte-identical pairs at HEAD** (verified with `diff -q` on all four). The `_docs` copy carries a
fifth file, `about.md`, with no `reference/` twin. Neither copy is generated from the other: there is
no build script, no `include!`, no test. The `_docs` copy is additionally NOT reachable as a command —
`scan_prompt_dir` skips `_`-prefixed directories
(`crates/cyrup-resources/src/discovery.rs:1810`, `if name.starts_with('.') || name.starts_with('_')
|| name == "node_modules" { continue; }`) — so three of its five files are pure runtime dead weight,
while two are load-bearing at COMPILE time: `render_cheatsheet.rs:13` is
`include_str!("../resources/prompts/flux/_docs/pipeline.md")` and `render_about.rs:12` is
`include_str!("../resources/prompts/flux/_docs/about.md")`.
**upstream** — one copy only. `code_puppy_core_plugins/flux_bootstrap/bundled/commands/flux/_docs/`
@v0.0.6 holds exactly four files (`README`, `cheatsheet`, `pipeline`, `synopsis`) and code-puppy
registers them as `//flux/_docs/*` slash commands. cyrup deliberately does not (`spec/flux.md:74`,
`:241`, `:400`, `:486`) and substitutes the skill's `reference/` set (`spec/flux.md:441-442`,
mapping table row at `:846`). **The non-registration is a signed-off delta and must not be re-filed
— the DUPLICATION is the defect.**
**Impact** — Edit `_docs/pipeline.md` and `/flux/cheatsheet` changes while `/skill:flux` does not;
edit `skills/flux/reference/pipeline.md` and the reverse. The two surfaces then describe the same
pipeline differently, and because `_docs/pipeline.md` is `include_str!`d the divergence is baked into
the binary at build time and cannot be seen by inspecting the installed tree. It is low severity only
because both copies agree today; there is no mechanism preventing them from disagreeing tomorrow, and
per `FLUX-003` there is no test to notice.
**Fix** — Delete one copy. The `_docs` copy has to stay (two `include_str!`s point at it and
`render_cheatsheet.rs:4-7` names it "SINGLE SOURCE OF TRUTH"), so make `skills/flux/reference/` a
build-time or check-time mirror of it rather than a hand-maintained second original. Cheapest
credible version: keep both trees and add the byte-equality assertion listed as item (8) of
`FLUX-003`'s minimum set — that converts a silent divergence into a failing build for the cost of one
test. While there, resolve the fifth file: either give `about.md` a `reference/` twin or say in
`spec/flux.md:441-442` why the skill's reference set is four and the `_docs` set is five.
**Verify** — A test asserting all four pairs are byte-equal must exist and pass; introducing a
one-character difference in either copy must fail it.

## ~~FLUX-006~~ — `parse_frontmatter` is UTF-8-strict where the Python is `errors="replace"`, so one bad byte empties a whole task file's frontmatter — **CLOSED 2026-09-04**

**Kind** parity-bug · **Severity** ~~low~~ · **Effort** S · **Confidence** confirmed
**CLOSED 2026-09-04, cyrup `4bb3569c` (with FLUX-003).** Landed as the one-line Fix below —
`std::fs::read` + `String::from_utf8_lossy` at `crates/cyrup-flux/src/state.rs` `parse_frontmatter`
(the cited `state.rs:67` moved down with the new `derive_base_from` seam above it) — and the Verify's
table test is `tests/flux_003_state_and_status.rs::parse_frontmatter_decodes_invalid_utf8_lossily_like_errors_replace`,
red (`{}`) before, green after, with the Python's own output as the expected values. Re-read at
**v0.0.40**: `flux_status.py:96-112` is byte-identical to the v0.0.6 lines cited below. One more
tolerance gap in the same function, not named here, closed alongside: `:105` is `str.splitlines()`,
not `str::lines` — `state::splitlines` ports its boundary table.
**cyrup** — `crates/cyrup-flux/src/state.rs:67` — `let Ok(text) = std::fs::read_to_string(path) else
{ return data };`. `read_to_string` returns `Err(InvalidData)` on any byte sequence that is not valid
UTF-8, so the whole function returns the empty map. Every other tolerance in the port is faithful:
the `starts_with("---")` gate (`:68`), the terminator break (`:74`), the split-on-first-colon
(`:77`), the missing-file case (`:67`).
**upstream** — `flux_status.py:96-112` @v0.0.6 — `text = path.read_text(encoding="utf-8",
errors="replace")` at `:100`, inside a `try/except OSError`. `errors="replace"` substitutes U+FFFD
for each undecodable byte and **parsing continues**, so the frontmatter is read and only the damaged
characters are mangled. The `except` catches OS errors only, not decode errors — because there are
none.
**Impact** — A task file with one stray byte (a Latin-1 apostrophe pasted from a ticket, a truncated
write, a file that spent a round-trip through a non-UTF-8 tool) renders in cyrup with a blank `STAGE`
and `(unknown)` `STATUS` — `render_status.rs:62-63` maps the resulting empty status straight to
`(unknown)` — while code-puppy renders the same file correctly. That is precisely the failure the
module's own doc forbids: `state.rs:8-10` says "Tolerant parsing is a requirement, not politeness
(port doc §5.6): this parser serves BOTH cyrup's own `/flux/*` prompt-written trees and
code-puppy's", and `spec/flux.md:669-672` says "port the Python tolerance exactly … missing/malformed
→ empty map (never error). This is what lets one renderer serve both code-puppy and cyrup state
trees". The severity is low because the failure is visible (a row appears, with wrong cells) rather
than silent, and because the pipeline's own writers emit UTF-8.
**Fix** — One line at `state.rs:67`: read bytes and decode lossily —
`let Ok(bytes) = std::fs::read(path) else { return data }; let text =
String::from_utf8_lossy(&bytes);` — which is exactly `errors="replace"`. Nothing else in the function
changes; `str::lines()` and `split_once(':')` operate on the `Cow` unchanged.
**Verify** — A table test writing `---\nstage: exec\nstatus: in-progress\n---\n` followed by the
invalid byte `0xFF` must return a two-key map, not an empty one. Listed as item (2) of `FLUX-003`'s
minimum set.

## FLUX-007 — Nothing in the crate records which upstream it was ported from, and its citations still resolve nowhere even now that both upstreams are cloned in this workspace

**Kind** tooling · **Severity** low · **Effort** S · **Confidence** confirmed
**REVISED 2026-09-04** — re-derived against `tmp/code_puppy_core_plugins` and `tmp/code_puppy`,
neither of which was in this workspace when the row below was first filed (2026-08-19). The premise
that made this "unresolvable off this machine" no longer holds — both repos are now clonable
coordinates any holder of this workspace can fetch, the same as every other upstream in this
directory — but **the three underlying cyrup-side defects are unchanged**, so the item stays open.
**cyrup** — Three compounding defects in the crate's evidence trail, each re-confirmed at HEAD
`2571969` this pass. **(1) No baseline version.** `grep -rn 'v0\.0\.' crates/cyrup-flux/src/
Cargo.toml` still returns **0**; `Cargo.toml` and `lib.rs:1-4` still name the upstream only in prose
("port of code-puppy flux", "ported from code-puppy's `flux_bootstrap` plugin"), no repository, no
tag. Same defect class area 09 records against `cyrup-ext-subagents`, except here there is still not
even an inference on file. **(2) The four upstream doc-comment links are wrong, and now precisely so.**
`state.rs:2`, `render_status.rs:2`, `render_cheatsheet.rs:2` and `render_about.rs:2` all cite
`../../../tmp/code-puppy/flux_bootstrap/bundled/scripts/<f>.py`. Now that the real clone exists, that
path is confirmed wrong on **two independent axes**: the clone directory is `tmp/code_puppy_core_plugins`
(underscored, not `tmp/code-puppy`), and inside it the plugin package is one level deeper than the
comment assumes — the real path is
`tmp/code_puppy_core_plugins/code_puppy_core_plugins/flux_bootstrap/bundled/scripts/<f>.py` (the repo
name and the top-level Python package share a name, and the comment's three `../` + `code-puppy/`
skips that package segment entirely). So the citation was never resolvable, at any point, on any
machine — not even the original author's, unless a differently-shaped checkout existed. `spec/flux.md`
still cites `../tmp/code-puppy/` **21** times, same wrong shape. **(3) Thirteen citations still point
at spec task files that do not exist.** Recounted this pass: `FLUX_01`×2, `FLUX_06`×1, `FLUX_07`×3,
`FLUX_08`×1, `FLUX_09`×4, `FLUX_10`×2 = 13, unchanged from the first pass — `extension.rs`,
`overlay.rs`, `render_about.rs`, `render_cheatsheet.rs`, `resources.rs` and `state.rs` all still carry
them, and `spec/flux/` still contains **only `README.md`**; all twelve `FLUX_NN.md` files its task
table links (`spec/flux/README.md:56-69`) are still absent. The "§5.6" instance is also unchanged:
`state.rs:8` and `spec/flux.md:672` both still cite it, and `spec/flux.md` still has no §5.6 heading.
**upstream** — n/a for the defect itself; this remains a cyrup bookkeeping problem, not a parity one.
**Baseline now independently re-derivable, and re-derived this pass:** `code_puppy_core_plugins`
ported baseline **`v0.0.6`** (repo now cloned at `tmp/code_puppy_core_plugins`, HEAD `8c6f852`,
**latest tag is now `v0.0.40`** — real tag-distance now exists where the first pass correctly recorded
none). `git -C tmp/code_puppy_core_plugins diff --stat v0.0.6 v0.0.40 --
code_puppy_core_plugins/flux_bootstrap/ tests/test_flux_bootstrap.py` is **empty** — the entire ported
surface is byte-identical across all 34 intervening tags, so the new tag-distance carries **zero**
content lag; nothing here should be filed as a version-lag item without re-running that diff first.
`code_puppy` ported baseline **`v0.0.720`**, **latest tag now `v0.0.819`** (repo cloned at
`tmp/code_puppy`, HEAD `38f74d4`). `TOOL_REGISTRY["invoke_agent"]` (`FLUX-002`'s citation) is
unchanged at the cited tag `v0.0.720`, re-confirmed by direct read this pass; the `v0.0.720..v0.0.819`
range is otherwise unswept for this area (see Blind spots).
**Impact** — The directory's central rule is that upstream claims are settled with `git show
<tag>:<path>` and never from a working tree (`README.md`'s hard rules; `09-cyrup-ext-subagents.md:5-8`
restates it). **That is now achievable for this area** — both repos are cloned in `tmp/` and their
tags confirmed — which retires the "impossible for anyone but the author" half of the original
Impact. What is NOT retired: the crate's own source still cites nothing a reader can resolve without
first independently discovering the correct clone shape, so a reader who trusts the doc comments as
written is still led to a path that does not exist. `README.md`'s Caveats section flags exactly this
hazard, and it already produced one wrong belief this pass had to correct by hand: the "Wibey"
attribution in `overlay.rs:1-2` reads as an unexplainable internal name until `flux_status.py:4-6`
@v0.0.6 is read directly, where it turns out to be upstream's own wording. **Do not re-derive that as
a leak.**
**Fix** — Three small edits, one now more precisely specified than before. (a) Record the baseline in
the crate: a `//!` line in `lib.rs` naming `code_puppy_core_plugins @ v0.0.6` and `code_puppy @
v0.0.720`. (b) Re-point the four doc-comment links at the REAL path —
`../../../tmp/code_puppy_core_plugins/code_puppy_core_plugins/flux_bootstrap/bundled/scripts/<f>.py`
— or, better, drop the link syntax entirely and cite `git show
v0.0.6:code_puppy_core_plugins/flux_bootstrap/bundled/scripts/<f>.py` in prose, which is what a
citation that must survive a `git show`-only reading rule should look like; a link that renders as
resolvable but silently is not is worse than plain prose. (c) Either restore
`spec/flux/FLUX_01.md`…`FLUX_12.md` or rewrite the 13 in-source `FLUX_NN` citations to point at
`spec/flux.md` sections that exist, and fix the two "§5.6" references. **This is still the row that
gates re-auditing every other row in this file**, which is why it stays filed despite being
bookkeeping.
**Verify** — `grep -rn 'tmp/code-puppy' crates/cyrup-flux/ spec/flux.md` returns 0; a corrected link
resolves with `git -C tmp/code_puppy_core_plugins show v0.0.6:<path>` from the exact path the comment
names; `grep -rnE 'FLUX_[0-9]+' crates/cyrup-flux/src/` resolves to files that exist; and `lib.rs`
names tags that `git -C tmp/code_puppy_core_plugins rev-parse v0.0.6` and `git -C tmp/code_puppy
rev-parse v0.0.720` both accept.

## Coverage

### Read first-hand at cyrup HEAD `4fb5e40`

In full: all nine files of `crates/cyrup-flux/src/` (`lib.rs`, `extension.rs`, `resources.rs`,
`state.rs`, `render_status.rs`, `render_cheatsheet.rs`, `render_about.rs`, `overlay.rs`,
`ask_tool.rs` — 1,513 lines), `crates/cyrup-flux/Cargo.toml`, and the full file listing of
`crates/cyrup-flux/resources/` (25 files: 15 templates, 5 `_docs`, 5 skill files). In the cited
regions, outside the crate: `crates/cyrup/src/main.rs:651-700` (the subagents opt-in gate), `:719-728`
/ `:926-932` / `:1056-1062` (the three flux attach seams, read in full and confirmed
identical and unconditional), `:2012-2017` (shortcut sourcing);
`crates/cyrup-tui/src/app/input.rs:80-100`, `app/run_arms.rs:268-280`, `app/hotkeys.rs:100-115`;
`crates/cyrup-tui/src/keymap.rs:493-505` (`Key::parse`), `:1282-1302` (`impl Default for
EditorKeymap`); `crates/cyrup-ext/src/registry.rs:905-965` (`resolve_shortcuts` + rules 3/4),
`:1196-1219` (`RESERVED_KEYBINDINGS_FOR_EXTENSION_CONFLICTS`, all eighteen ids counted),
`crates/cyrup-ext/src/facade.rs:1818-1832`; `crates/cyrup-resources/src/discovery.rs:1764-1830`
(`scan_prompt_root`/`scan_prompt_dir`, the `_`-prefix skip); `crates/cyrup-ext-subagents/src/
extension.rs:96-97`; `crates/cyrup/src/cli.rs:946`; `spec/flux.md` (§0.4 rename map `:115-129`,
§3.4.1-§3.4.3 `:547-715`, §5 `:852-883`, the mapping table `:828-848`), `spec/flux/README.md`,
`docs/guide/extensions/flux.md`, the repository `../../README.md:80-95`, and this directory's
`README.md:406-445`
(item format, kind vocabulary, severity rubric), `docs/gap-analysis/06-cyrup-ext.md:251`
(`EXT-039`), and `docs/gap-analysis/09-cyrup-ext-subagents.md` in full as the structural template.

### Read first-hand upstream, at tags only

`git -C /Users/davidmaple/cyrup.ai/code_puppy_core_plugins show v0.0.6:<path>`, never the working
tree — **that checkout's working tree has no `flux_bootstrap/bundled/commands/` at all**, while
`v0.0.6` has all 22 files under it, so a working-tree read reports the entire upstream command set as
missing. Read: `flux_bootstrap/bundled/scripts/flux_status.py` (in full, 345 lines),
`flux_bootstrap/installer.py` (docstring + install contract), `bundled/commands/flux/new.md:14-19`
(the `FLUX_ROOT`/`FLUX_BASE` derivation block), and
`git ls-tree -r --name-only v0.0.6` enumerated in full for the 18-command / 4-`_docs` / 3-script
census. `FLUX_ROOT` presence was counted across all 18 command files individually, not sampled.
From `code_puppy` @`v0.0.720` (`/Users/davidmaple/cyrup.ai/code_puppy`, HEAD `757db1cf`):
`code_puppy/tools/__init__.py:36-40` (`TOOL_REGISTRY`) and
`code_puppy/tools/subagent_invocation.py:650`/`:696`. From pi @`v0.83.0`, for `FLUX-004`'s upstream
half only: `packages/tui/src/keybindings.ts:54-70`.

### Re-audited 2026-09-04 — what was actually re-read

`crates/cyrup-flux/src/*.rs` in full again (all nine files), diffed mentally against the first pass's
quotes — unchanged. `crates/cyrup/src/main.rs` symbols for the subagent opt-in gate (grep-confirmed
present, same shape). `crates/cyrup-ext/src/registry.rs` / `facade.rs` `resolve_shortcuts` callers
(grep-confirmed still test-only). The four `_docs`/`reference` pairs (`diff -q`, still byte-identical).
`git -C tmp/code_puppy_core_plugins show v0.0.6:code_puppy_core_plugins` (tree listing, confirms
`flux_bootstrap/` at the expected shape) and `v0.0.40` (same 29-file listing, confirming no added or
removed files); `git -C tmp/code_puppy_core_plugins diff --stat v0.0.6 v0.0.40 --
code_puppy_core_plugins/flux_bootstrap/ tests/test_flux_bootstrap.py` (empty). `git -C tmp/code_puppy
show v0.0.720:code_puppy/tools/__init__.py` (re-confirms `invoke_agent`/`TOOL_REGISTRY` unchanged from
the first-pass quote) and `git -C tmp/code_puppy diff --stat v0.0.720 v0.0.819` (324 files,
+35128/−24793 workspace-wide; only `tools/__init__.py` and `tools/subagent_invocation.py` read in
detail, both unrelated to Flux — a Codex-patch tool-selection rewrite and a subagent recursion-guard
refactor). `git log --oneline 4fb5e40..HEAD -- crates/cyrup-flux` (one commit, doc-comment-only).

### Surface-driven sweeps run (diffed as sets, not spot-checked)

1. **Content census** — upstream's 18 `bundled/commands/flux/*.md` @v0.0.6 against cyrup's 15
   templates + 3 native renderers. Complete; the three that became renderers are exactly upstream's
   three `exec:` commands. **No content gap exists — do not re-derive one.**
2. **`FLUX-GAP` marker sweep** — `grep -rn 'FLUX-GAP' crates/cyrup-flux/` = **0**, against 8
   remaining references in `spec/`. FLUX_11's sweep is genuinely complete.
3. **Tool-name sweep over all 15 templates** — the identifiers actually named are `bash` (83),
   `ask_user_question` (25), `subagent` (18), `grep` (12), `write` (11), `read` (11), `edit` (5).
   All resolve to real cyrup tools (`crates/cyrup-tools/src/tools/` + the flux-registered
   `ask_user_question` at `extension.rs:54-56`) **except for the availability defect in `FLUX-002`** —
   no template names a nonexistent tool. The rename map at `spec/flux.md:118-129` is fully applied
   in BOTH directions: `grep -rnoE '\b(create_file|replace_in_file|read_file|invoke_agent)\b'`
   over the 15 templates returns **zero** — no code-puppy tool name survived the port.
4. **Resource-root sweep** — `grep -rn 'CARGO_MANIFEST_DIR' crates/ --include='*.rs'` across the
   workspace, which is how `FLUX-001`'s three sibling instances were found.
5. **Test-presence sweep** — per-crate count of files containing a test, all 20 crates under `crates/`,
   which is how the `cyrup-sdk` correction in `FLUX-003` was found (the crate is not the *only*
   untested one).

### Rejected with reason — do not re-derive

- **The `exec:` frontmatter directive is DELIBERATELY unported.** `spec/flux.md:859-862` records the
  decision and the reasoning: "Shell-out-from-frontmatter is a trust/sandbox question cyrup answers
  with the extension capability model instead; the three exec commands become native renderers. This
  is a **CYRUP-DELTA** relative to code-puppy, by design." It is why upstream's 18 commands became
  15 templates + 3 renderers. **Not a gap.**
- **`prompts/flux/_docs/` never registering as commands is DELIBERATE**, not a consequence of an
  unnoticed scanner rule. `spec/flux.md:74`, `:241`, `:400`, `:438`, `:486` all state it, and the
  substitute — the skill's `reference/` set — is complete at four files. Upstream registers
  `//flux/_docs/*` as commands and cyrup does not; that trade is on the record. What IS filed is the
  DUPLICATION it produced (`FLUX-005`).
- **`derive_base`'s `FLUX_ROOT` support is a deliberate correction of an upstream inconsistency, and
  removing it would be a regression.** `crates/cyrup-flux/src/state.rs:42-60` honours
  `${FLUX_ROOT:-$HOME/.flux}`; upstream's `flux_status.py:89-93` does **not** — it is
  `Path.home()/".flux"/flatten_cwd(...)` with no env var. But upstream's PROMPTS do: `FLUX_ROOT` is
  read in **10 of the 18** command files (`aug`, `exec`, `qa`, `review` ×4 each; `address-feedback`,
  `ask`, `config`, `new`, `split`, `tests` ×2 each), e.g. `new.md:14`,
  `FLUX_ROOT="${FLUX_ROOT:-$HOME/.flux}"`. So code-puppy's own renderer reads a different tree from
  its own commands whenever `FLUX_ROOT` is set, and cyrup picked the commands' behaviour, which
  `spec/flux.md:667-668` specifies. **A later pass reading only `flux_status.py:89-93` will see
  cyrup's env override as invented drift and try to delete it — this is the record that stops that.**
- **"Wibey" is upstream's own word, not a leaked internal build-host name.**
  `crates/cyrup-flux/src/overlay.rs:1-2` and `render_about.rs:10` inherit it verbatim from
  `flux_status.py:4-6` @v0.0.6 ("This is the code-puppy equivalent of Wibey's native `ui-mode:
  flux-status` renderer. Wibey draws an interactive Ink overlay; code-puppy can't host an interactive
  UI"). It names no repository anyone can read, which is a real weakness of the citation — but the
  weakness is upstream's, and the cyrup comment is a faithful port. Do not file it as a cyrup defect.
- **`ask_user_question` is not a capability gap in either direction.** It is registered
  unconditionally by flux itself (`extension.rs:54-56`), so unlike `subagent` it is present whenever
  the templates that name it are present. Its `HostServices::select` bridge and the
  description-folding workaround are documented in-source at `ask_tool.rs:71-74` against the
  `oauth_select` CYRUP-DELTA precedent.

### Handoffs to other areas

- **`FLUX-004`'s missing half belongs to `EXT-039`** (`06-cyrup-ext.md:251`, PARTIALLY CLOSED
  2026-08-14), whose open residual is literally "call `resolve_shortcuts` … thread
  `shortcut_diagnostics` into `startup_diagnostics.extensions`". Flux is the first shipped extension
  that makes that residual cost something. **`EXT-039` should be re-read for severity now that it has
  a live consumer**; this row is the input, not a re-filing.
- **`FLUX-001`'s three sibling instances** — `cyrup-ext-subagents/src/extension.rs:5750`,
  `cyrup-ext-subagents/src/registration/resources.rs:46`, `cyrup-intercom/src/resources.rs:41` —
  belong to areas 09 and 11 and are unfiled in both. They are lower severity there because both
  subsystems are opt-in (`../../README.md:88-91`), but the fix shape is shared; whoever lands `FLUX-001`
  should land all four.
- **`FLUX-002`'s alternative fix** (arming subagents when flux needs them) would edit
  `crates/cyrup/src/main.rs:651-700`, which is area 09's gate. **It needs an owner decision, not
  agent work** — it flips the default for an OS-process-spawning subsystem. The template-side fix
  proposed in the row stays entirely inside this crate.
- **`README.md` and `00-residual-ledger.md` need three edits this file cannot make**, since neither is
  in this area's partition: (a) a `[14-cyrup-flux.md]` row in the Contents table
  (`README.md:227-240`) with the totals re-derived; (b) a `code_puppy_core_plugins/` row in
  `## Baselines measured against` (`README.md:384-391`) — **updated this pass**: HEAD `8c6f852`
  (was `8de5184`), ported baseline `v0.0.6`, **latest tag now `v0.0.40`** (was recorded as `v0.0.6`
  with "no version lag to sweep yet" — that statement is now false as a tag-distance claim, though
  the diff-stat sweep above found the content delta is still empty), plus `code_puppy` HEAD `38f74d4`,
  ported baseline `v0.0.720`, **latest tag now `v0.0.819`** (was recorded with no latest-tag column
  entry); (c) `README.md:3-4`, which says cyrup is measured against "four TypeScript upstreams" —
  code-puppy is the fifth ported upstream and the first that is not TypeScript, which changes the
  scope of the `git show <tag>:<path>` hard rule. The open counts in `00-residual-ledger.md` and
  `PARITY-GAPS.md` are unchanged by this pass (**+7**, same as before — nothing closed) and should
  still be re-derived from the tables, not adjusted arithmetically.

### Blind spots — read this before the next pass

1. **Static only, and this area has no test suite to fall back on.** Nothing was executed. Every
   `Verify` line is a design. Unlike every other area file, there is not even an existing green test
   to reason from — see `FLUX-003`.
2. **`overlay.rs` (347 lines) and `ask_tool.rs` (219 lines) were read but NOT diffed against an
   upstream, because they have none.** The overlay restores a Wibey behaviour code-puppy itself could
   not host (`flux_status.py:4-6`), and `ask_user_question` is cyrup-original by the crate's own
   admission (`ask_tool.rs:1-5`, "Closes the only real capability gap between code-puppy flux and
   cyrup flux"). They are therefore unmeasured in both directions — no parity claim is made about
   them here, and none should be inferred from their absence from the open table. **The overlay is the
   single largest unaudited surface in this crate.**
3. **The 15 templates were swept for tool names and `FLUX_ROOT`, not read as prose.** Only
   `exec.md`'s branch logic (`:30-49`, `:180`, `:199`), `aug.md:157`, `qa.md:162`, `review.md:109`,
   `new.md` (upstream) and `_docs/pipeline.md:21-31` were read line by line. Upstream's other 17
   command files were enumerated and grepped, not read. **A prose-level diff of all 15 pairs has never
   been run** and is where a `parity-bug` would hide — `spec/flux.md:828-848`'s per-file mapping table
   asserts what was carried "verbatim" for each, and none of those assertions has been checked.
4. **The three renderers' OUTPUT was never compared against the Python's.** `render_status.rs` was
   read against `flux_status.py:181-270` statement by statement and the six layout constants match
   (`:16-20`, `:23-31`), but no golden output was produced from either side. `render_cheatsheet.rs`'s
   two hand-rolled parses and `render_about.rs:20`'s hand-rolled regex lookbehind were read for
   plausibility only. `FLUX-003` item (5) is the test that would settle it.
5. **`cargo package` / `cargo install` behaviour for `FLUX-001` was reasoned, not run.**
   `Cargo.toml:13-17`'s `include` list puts `resources/**` in the package, which is what makes the
   failure mode SUBTLE rather than universal — the files exist in the registry source dir, so a
   `cargo install` on the build machine may work while a copied binary does not. The exact matrix
   (`cargo install` from crates.io vs `--path` vs a copied release artifact vs a container layer) was
   not measured, and the row's severity would move if it turned out one of those paths is fine. **Run
   it before down-rating the row.**
6. **No `AREA-NNN` id has ever been grepped out of this crate's source, because none existed.** The
   residual-ledger rule "Grep the SOURCE for `AREA-NNN` citations at every reconciliation, not just
   the docs" now applies here: `grep -rnE 'FLUX-[0-9]{3}' crates/cyrup-flux/` should start returning
   hits as these rows get fix comments, and `FLUX_NN`/`§5.6`-style dangling citations
   (`FLUX-007`) must not be allowed to accumulate alongside them.
7. **`code_puppy`'s `v0.0.720..v0.0.819` range (324 files, +35128/−24793) is unswept beyond the two
   files `FLUX-002` already cites.** This pass read only enough of `tools/__init__.py` and
   `tools/subagent_invocation.py` to re-confirm `invoke_agent`'s registration is unchanged at the
   CITED tag `v0.0.720` — it did not read forward to `v0.0.819` for new tools, renamed tools, or
   subagent-behavior changes that might bear on `FLUX-002` or on tool names the 15 templates assume.
   The two files' `v0.0.720..v0.0.819` diff itself (a Codex-patch tool-selection rewrite and a
   subagent recursion-guard refactor) was skimmed and neither hunk looked Flux-relevant, but it was
   not read closely enough to file or refute anything from it — left open rather than guessed at.
   Whoever next re-audits this area should read that range in full before trusting `FLUX-002`'s
   severity against current upstream, not just against the recorded baseline tag.
8. **`flux_bootstrap/`'s zero-diff finding (`v0.0.6..v0.0.40`) covers only that directory and
   `tests/test_flux_bootstrap.py`, not `code_puppy_core_plugins` as a whole.** No claim is made here
   about drift elsewhere in that repo (other plugins, shared install machinery `flux_bootstrap`
   might come to depend on) — out of scope for this area regardless.
