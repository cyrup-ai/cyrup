---
stage: new
status: done
updated: 2026-08-22 18:31
---

# Close The &str Slicing Hole In The No-Panic Policy With deny(clippy::string_slice)

> Identified by the `cyrup-tui` hygiene audit (6-dimension fan-out, adversarially verified).
> **Priority:** high · **Effort:** medium

## Description

The crate's entire declared safety posture is the four workspace lints denied at `/home/user/cyrup/Cargo.toml:97-101` (`unwrap_used`, `expect_used`, `panic`, `indexing_slicing`), reinforced by `#![forbid(unsafe_code)]` at `crates/cyrup-tui/src/lib.rs:35` and a release profile that sets `panic = "abort"` (recorded at `src/panic_hook.rs:18` — no unwind, no Drop). But `clippy::indexing_slicing` only fires on `ty::Slice`/`ty::Array` receivers; `str`/`String` range indexing is `ty::Str` and is exempt, tracked instead by the non-enabled restriction lint `clippy::string_slice`. So `cargo clippy -p cyrup-tui --all-targets` passes clean while 19 production sites range-slice a `&str` and would panic on a non-char-boundary byte offset — in widgets that take arbitrary Unicode keystrokes.

The 19 sites (reproduce with `cargo clippy -p cyrup-tui --all-targets -- -W clippy::string_slice`): `text_input.rs:113,120,126,132`; `session_selector.rs:487`; `model_selector.rs:194`; `tree_selector.rs:635`; `commands.rs:376,379,382`; `keymap.rs:536,538`; `theme.rs:1351,1354,1355`; `transcript/content.rs:66,67`; `transcript/tool_result.rs:166,168`. All 19 are boundary-safe **today**: the `find`/`rfind`/`strip_prefix`-derived offsets are char boundaries by construction, and the four cursor-driven families maintain a hand-rolled byte-offset invariant (`text_input.rs:108-109` `insert` then `cursor += c.len_utf8()`, `:114` `cursor - ch.len_utf8()`, with the only absolute assignments `self.cursor = 0` at `:221` and `= self.buffer.len()` at `:225`). This is unenforced discipline invisible to every tool — the point of the task is to make it mechanical, not to fix a live crash.

**The work.** Add `#![deny(clippy::string_slice)]` to `crates/cyrup-tui/src/lib.rs` beside the existing `#![forbid(unsafe_code)]` at `:35`. Keep it crate-scoped: promoting it to `[workspace.lints.clippy]` is out of scope (the same clippy run shows ~80 more sites in other crates, ~20 in `cyrup-resources/src/package/git_url.rs` alone). Then convert the 19 sites. The `find`-derived ones are mechanical `.get(..)` + `let ... else`, e.g. `transcript/tool_result.rs:168` `output[..idx]` -> a `let Some(head) = output.get(..idx) else { ... }` that falls into the module's existing degrade path. Prefer `let ... else` returning the existing fallback over `unwrap_or(output)`-style silent substitutions, which convert a boundary bug into wrong output instead of surfacing it. The four cursor families are better fixed at the source: replace the raw slice with `self.buffer.get(..self.cursor)` feeding the `else { return }` arm that `text_input.rs:113` already has the shape for. Use `login_dialog.rs:440-441`'s `.get(..cursor).and_then(|s| s.chars().next_back())` as the template — it is the form that already satisfies the workspace deny.

The deny also covers `src/tests/`, where the same clippy run flags 24 additional sites across 13 files: `run_loop_cancel_bias.rs:34,37,38,67,68,69`; `run_loop_swap_arm_reachable.rs:42,43,44`; `run_loop_input_priority.rs:36,39,40`; `run_loop_draw_coalescing.rs:44,46`; `render_cache_tick.rs:37,39`; `thinking.rs:173`; `settings_trust_selectors.rs:257`; `selector_wiring.rs:101`; `scoped_models.rs:93`; `footer_chrome_fidelity.rs:982`; `editor.rs:706`; `dialog_envelope_spacers.rs:380`; `auth_selector.rs:262`. These are source-scraping guards over `include_str!` bodies, where slicing is the point — add `string_slice` to the existing per-module `#[allow(...)]` blocks (which today list only `unwrap_used`/`expect_used`/`indexing_slicing`/`panic`), matching the established pattern, rather than rewriting them. Note `text_input.rs:242` already carries such an allow block; do not let it mask the production fixes at `:113-132`.

## Acceptance Criteria

- [ ] `crates/cyrup-tui/src/lib.rs` contains `#![deny(clippy::string_slice)]`
- [ ] `cargo clippy -p cyrup-tui --all-targets` shows only the known pre-existing warning at escape_reassembly.rs:972 — no `string_slice` diagnostics
- [ ] `grep -rn 'string_slice' crates/cyrup-tui/src/ | grep allow` shows allows only inside `#[cfg(test)]` modules / test files, never on a production item
- [ ] No production `&str` range slice remains at the 19 cited sites (spot-check `text_input.rs:113`, `transcript/tool_result.rs:168`, `theme.rs:1351`)
- [ ] `cargo test -p cyrup-tui` passes (1270 tests) and `cargo build -p cyrup-tui` emits 0 warnings

## Evidence

> **Correction applied during task creation:** the audit cited
> `crates/cyrup-tui/src/package/git_url.rs`, which does not exist — that file lives in
> `crates/cyrup-resources` and is out of scope for this crate-scoped task. The real
> `&str`-slice surface in cyrup-tui is 7 sites across 3 production files (see below).

/home/user/cyrup/Cargo.toml:97-101; crates/cyrup-tui/src/lib.rs:35; crates/cyrup-tui/src/panic_hook.rs:18; production sites text_input.rs:113,120,126,132, session_selector.rs:487, model_selector.rs:194, tree_selector.rs:635, commands.rs:376,379,382, keymap.rs:536,538, theme.rs:1351,1354,1355, transcript/content.rs:66,67, transcript/tool_result.rs:166,168; invariant text_input.rs:108-109,114,221,225; lint-clean template login_dialog.rs:440-441; existing allow block text_input.rs:242
