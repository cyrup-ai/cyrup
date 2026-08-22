---
stage: new
status: done
updated: 2026-08-22 18:31
---

# Fix The lib.rs Crate Doc's Dead Pointer And Stale Gap List, And Annotate The Pre-Split Citations

> Identified by the `cyrup-tui` hygiene audit (6-dimension fan-out, adversarially verified).
> **Priority:** medium · **Effort:** small

## Description

Two cheap documentation repairs left behind by the `app.rs` and `transcript.rs` splits. One session, prose only, no code movement.

**(1) The crate doc points nowhere and describes shipped features as gaps.** `crates/cyrup-tui/src/lib.rs:31` reads "# Remaining gaps (see `spec/gap-analysis/12-cyrup-tui.md`)". `spec/` exists but contains only `CMDHINT_01.md`, `flux/`, `flux.md` — there is no `spec/gap-analysis/`. The real document is `docs/gap-analysis/07-cyrup-tui.md`; `docs/gap-analysis/12-…` is `12-upstream-drift-pi-core.md`, a different area. So the pointer is wrong twice. Fix it to `docs/gap-analysis/07-cyrup-tui.md`.

Then delete the demonstrably-stale clause at `:32`: "The five unsourced data-bound selectors + their bespoke layouts (tree/session/settings/trust/oauth)" — all are declared modules (`lib.rs:63` oauth, `:72` session, `:73` settings, `:87` tree) and publicly re-exported (`:175`, `:186`, `:187` incl. `TrustSelector`, `:193`), and `07-cyrup-tui.md` discusses them as shipped-but-divergent (TUI-027 at `:308`/`:647` is about `/tree`'s missing search filter *inside* a working selector). Leave the `clipboard-image paste` clause alone: `docs/gap-analysis/07-cyrup-tui.md:1873` confirms `App::attach_image`/`attach_image_path` still have no input-path caller and Ctrl+V inserts the temp `.png` path as text, so that entry is at worst partially stale and resolving it is TUI-064's job, not this task's. Do NOT attempt to re-derive the whole gap list from `07-cyrup-tui.md` — it is 2,000+ lines of live rows and that is a separate exercise.

Also replace the meaningless round label at `:25` ("# Built this round (L6 round-4)") with a plain capability heading or fold it into the "Built in-crate" section at `:18`, and add two lines to the `# Layering` section (`:9-16`) naming the two module trees the crate is now organised around: `app/` (the `App` shell — `run.rs`/`run_arms.rs`/`run_action.rs` select! skeleton, `draw.rs`/`render.rs`/`layout.rs` frame path) and `transcript/`. Leave the `pub use` block's ordering alone.

**(2) Annotate the 22 pre-split self-citations instead of churning them.** 9 comments cite `app.rs:NNN` (`transcript/tests/output_pad.rs:183,200`; `transcript/render.rs:295`; `panic_hook.rs:58`; `model_selector.rs:224`; `tests/markdown.rs:1545`; `tests/bash_overlay.rs:346`; `tests/clipboard.rs:236,271`) and 13 cite `transcript.rs:NNN` (`transcript/cache.rs:16,38`; `transcript/tests/output_pad.rs:132`; `app/events_fold.rs:11,107,159`; `tests/render_cache_tick.rs:4`; `tests/turn_interleaving.rs:13`; `markdown.rs:71,72,73,74,76`). Both files are deleted. These are distinct from the crate's deliberate upstream citations (`interactive-mode.ts:3500`, `truncate.ts:177`), which name a pinned external tag — these name this repo's own removed files.

The project already has the precedent for handling this, at `docs/gap-analysis/07-cyrup-tui.md:161-162`: annotate, do not churn. Add a short paragraph to the module doc of `crates/cyrup-tui/src/app/mod.rs` and `crates/cyrup-tui/src/transcript/mod.rs` naming the pre-split file, the commit it was split at (`40821ed` for `app.rs`; the equivalent commit for `transcript.rs`), and stating that any surviving `app.rs:NNN` / `transcript.rs:NNN` citation elsewhere in the crate is historical and refers to that pre-split file.

Then fix only the two places where drift actively misdirects about a **symbol**, not merely a line: `src/tests/render_cache_tick.rs:3-4` cites "`BashExecution::render_lines` -> `started.elapsed()`, bash.rs:204" but `bash.rs:204` is inside the doc for the unrelated `context_truncated` (`fn context_truncated` at `bash.rs:206`), and the same line's "`render_bash`, transcript.rs:2157" is now `src/transcript/tool_builtin.rs:214`; `src/panic_hook.rs:58` cites `app.rs:7593-7606` for `draw_synchronized`, which is `src/app/crossterm.rs:87`. Explicitly do NOT rewrite the six-row citation table at `src/markdown.rs:71-76` — the note covers it and the table's value is the width arithmetic, not the line numbers.

## Acceptance Criteria

- [ ] `grep -rn 'spec/gap-analysis' crates/cyrup-tui/src/lib.rs` returns no hits; the doc points at `docs/gap-analysis/07-cyrup-tui.md`
- [ ] The `Remaining gaps` list no longer claims the tree/session/settings/trust/oauth selectors are unbuilt; the `L6 round-4` heading is gone; `# Layering` names the `app/` and `transcript/` trees
- [ ] `crates/cyrup-tui/src/app/mod.rs` and `crates/cyrup-tui/src/transcript/mod.rs` module docs each contain a historical-paths paragraph naming the pre-split file and its split commit
- [ ] `src/tests/render_cache_tick.rs:3-4` and `src/panic_hook.rs:58` cite live symbols (`transcript/tool_builtin.rs:214`, `app/crossterm.rs:87`); `src/markdown.rs:71-76` is unchanged
- [ ] `cargo build -p cyrup-tui` emits 0 warnings and `cargo test -p cyrup-tui` passes 1270 tests (the source-scraping guards in `src/tests/` must still find their anchors)

## Evidence

crates/cyrup-tui/src/lib.rs:9-16,18,25,31-34 (verified verbatim: `spec/gap-analysis/12-cyrup-tui.md`), module decls :63,:72,:73,:87, re-exports :175,:186,:187,:193; `ls spec` -> CMDHINT_01.md, flux, flux.md (no gap-analysis); docs/gap-analysis/07-cyrup-tui.md:1 (title), :161-162 (annotate-don't-churn precedent), :308,:647 (TUI-027), :1873,:1879 (clipboard/attach_image still open); docs/gap-analysis/12-upstream-drift-pi-core.md:1; 22 dead self-citations at the listed file:line pairs; symbol drift tests/render_cache_tick.rs:3-4 vs bash.rs:204/206 and transcript/tool_builtin.rs:214; panic_hook.rs:58 vs app/crossterm.rs:87
