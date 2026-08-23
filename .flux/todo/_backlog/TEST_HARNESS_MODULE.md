---
stage: new
status: done
updated: 2026-08-22 18:31
---

# Add src/tests/harness.rs, Migrate The 46 Duplicated Scrape And Key Helpers, And Document Placement

> Identified by the `cyrup-tui` hygiene audit (6-dimension fan-out, adversarially verified).
> **Priority:** medium · **Effort:** medium

## Description

`crates/cyrup-tui/src/tests/mod.rs` is 109 lines containing only a doc comment (`:1-12`) and 96 `mod` declarations — zero shared code across 97 files / 29,151 lines. The consequence is 47 hand-rolled buffer-scrape helpers totalling 754 lines under 8 different names, and they have already drifted semantically.

**The drift is the reason this matters, not DRY.** `fn buf_text(app: &App<TestBackend>) -> String` appears in 21 files (`assembled_render.rs:32`, `auth_selector.rs:35`, `bash_overlay.rs:26`, `extension_dialog_countdown.rs:17`, `extension_dialog_wrapping.rs:19`, `extension_editor_dialog.rs:25`, `footer_chrome_fidelity.rs:46`, `footer_subscription.rs:182`, `fork_selector.rs:20`, `image.rs:23`, `model_selector_assembled.rs:17`, `render.rs:19`, `scoped_models.rs:36`, `selector.rs:53`, `selector_wiring.rs:70`, `settings_inert_keys.rs:98`, `settings_trust_selectors.rs:21`, `status_indicator.rs:15`, `transport_live_apply.rs:69`, `tree_and_chrome.rs:30`, `tree_label_timestamp.rs:32`); 18 are byte-identical. The 18-copy majority (`status_indicator.rs:15-27`) pushes `'\n'` after every row, leaving a trailing newline; `footer_chrome_fidelity.rs:46-49` builds the same string with `.join("\n")` and has none. `buf_text(&app).contains("foo\n")` against the bottom row therefore means different things depending on which file it was written in. `fn key(code: KeyCode) -> InputEvent { InputEvent::Key(KeyEvent::new(code, KeyModifiers::NONE)) }` appears in 20 files, all byte-identical, with a 5th spelling returning `KeyEvent` in 5 more.

The crate has already paid for the missing harness and documented it: `src/tests/image_capabilities.rs:222-236` had to promote a file-private lock to `pub(crate) fn caps_lock()` so `src/tests/markdown.rs:154` could serialize against it — "A lock the reader cannot name does not serialize anything."

**The work (scoped to one session).** Add `crates/cyrup-tui/src/tests/harness.rs`, declared in `mod.rs` as `pub(crate) mod harness;`, exporting: `buf_text`, `rows_text(app, y0, y1)`, `row_text(app, y)`, `live_text`, `find_row`, `row_with`, `col_of`, `fg_at` (lift the richer forms from `assembled_render.rs:32-56` and `footer_chrome_fidelity.rs:41-76`), plus `key`, `ctrl`, `alt`, `esc`. Pin ONE trailing-newline convention in the harness doc comment and state it explicitly. Migrate the 21 `buf_text` copies and the 25 `key`-family copies to `use super::harness::*;`, adjusting any assertion in `footer_chrome_fidelity.rs` that depended on the old no-trailing-newline join. Move `caps_lock` from `image_capabilities.rs:222-236` into the harness and re-point `markdown.rs:154`.

**Explicitly out of scope**, to keep this one session: (a) `src/tests/chrome.rs:12-23` — its `buf_string` takes `&Terminal<TestBackend>` because the file never constructs an `App` at all (`chrome.rs:5-10` tests `compact_hints`/`format_key_text`/`BorderedLoader` directly), so it cannot use an `&App`-shaped helper and is not drift; leave it. (b) The 30 zero-arg `new_app()`/`app()` constructors across 10 distinct TestBackend dimensions — a follow-up, saving roughly one line per file. (c) `escalation.rs:19`'s `GLOBAL_STATE_LOCK`, which has no second reader and correctly sits next to its process-global statics, per the pattern at `src/drain.rs:204-207`.

**Two small companions, same session.** First, extend the `src/tests/mod.rs` doc comment (`:1-12`, which today documents only the migration history) with the placement rule the crate already follows but states only in leaves: inline `#[cfg(test)] mod tests` when the tests need private items/fields or must sit beside a process-global static and its lock (33 production files do this, 19 provably needing private access); `src/transcript/tests/` for the transcript module's private-access tests (7 files, 1,975 lines, declared at `src/transcript/mod.rs:50-51` and never mentioned from the index); `src/tests/` for App-level tests that drive `App<TestBackend>` and assert on rendered output. Cite `src/transcript/tests/mod.rs:1-3` and `src/app/backend.rs:230-234` as the existing statements of the rule. No test files move for this part.

Second, delete `crates/cyrup-tui/tests/zzz_scratch_perf_probe.rs`, whose own line 1 reads "THROWAWAY perf probe (TUI-092 round 2). Not part of the suite — delete after measuring." It is 286 lines, self-gated `#![cfg(feature = "scrollback-accumulator")]` (`:4`), hand-rolls a 60-line `impl Backend for CaptureBackend` (`:62-117`) duplicating `src/tests/inline_stacking.rs:60-135`, and has already blocked a merge gate once (`docs/gap-analysis/00-residual-ledger.md:200-206`) after a ratatui bump. Its `scroll_region_up`/`scroll_region_down` at `:75-80` are NOT gated on `#[cfg(feature = "scrolling-regions")]` while the sibling harness gates both (`inline_stacking.rs:126-131`), so it alone fails to compile under `--no-default-features --features scrollback-accumulator`. Trim the reference to it from the feature doc comment at `crates/cyrup-tui/Cargo.toml:22-27`, leaving `cyrup-it`'s `wasm_renderer_screen.rs:119,144` as the feature's sole consumer. Confirm TUI-092 round 2 is closed first; if the measurement is still wanted, re-add it as an `#[ignore]`d test in `src/tests/` reusing `inline_stacking.rs`'s existing `CaptureBackend`.

## Acceptance Criteria

- [ ] `crates/cyrup-tui/src/tests/harness.rs` exists and is declared from `mod.rs`; its doc comment states the trailing-newline convention explicitly
- [ ] `grep -c 'fn buf_text' crates/cyrup-tui/src/tests/*.rs | grep -v ':0'` matches only `harness.rs`; same for `fn key(code: KeyCode) -> InputEvent`
- [ ] `grep -rn 'fn caps_lock' crates/cyrup-tui/src/tests/` returns only `harness.rs`, and `markdown.rs` reaches it through the harness
- [ ] `crates/cyrup-tui/tests/zzz_scratch_perf_probe.rs` is deleted and no longer referenced from `crates/cyrup-tui/Cargo.toml`
- [ ] `src/tests/mod.rs`'s doc comment names all three test locations and the rule for choosing between them, citing `src/transcript/tests/mod.rs:1-3` and `src/app/backend.rs:230-234`
- [ ] `cargo test -p cyrup-tui` passes 1270 tests (count unchanged — this is a refactor, not a coverage change), `cargo build -p cyrup-tui` emits 0 warnings, `cargo clippy -p cyrup-tui --all-targets` shows only escape_reassembly.rs:972

## Evidence

crates/cyrup-tui/src/tests/mod.rs:1-12 (109 lines, 96 mod decls, 0 fns); 21 buf_text copies at the listed file:line pairs, 18 byte-identical; trailing-newline divergence status_indicator.rs:15-27 vs footer_chrome_fidelity.rs:46-49; 20 identical `key` copies (bash_overlay.rs:9, assembled_render.rs:24, auth_selector.rs:20, command_exec.rs:12, editor_page_actions.rs:19, extension_editor_dialog.rs:21, …); richer helpers assembled_render.rs:32-56, footer_chrome_fidelity.rs:41-76; cross-file lock image_capabilities.rs:222-236 consumed at markdown.rs:154; out-of-scope chrome.rs:5-23, escalation.rs:19, drain.rs:204-207; placement rule src/transcript/tests/mod.rs:1-3, src/app/backend.rs:230-234, src/transcript/mod.rs:50-51; probe crates/cyrup-tui/tests/zzz_scratch_perf_probe.rs:1,4,62-117,75-80 (286 lines), Cargo.toml:22-27, docs/gap-analysis/00-residual-ledger.md:200-206, inline_stacking.rs:126-131
