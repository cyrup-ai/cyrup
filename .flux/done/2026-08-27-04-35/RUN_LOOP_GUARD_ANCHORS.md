---
stage: qa
status: completed
updated: 2026-08-27 04:35
---

# Repair The Dead Run-Loop Select! Guards And Make Anchor Misses Fail Loud

> **Closed by verification, not by a flux session.** The work landed via PR #58 and PR #68;
> this file was parked in `backlog/` with the `/task`-authored `status: done` frontmatter, which
> records only that the file was written. Every acceptance criterion above was re-checked against
> `main` by command before filing, plus the shared gate: `cargo build -p cyrup-tui` 0 warnings,
> `cargo test -p cyrup-tui` 1271 passed, `cargo clippy -p cyrup-tui --all-targets` reporting only
> the pre-existing `escape_reassembly.rs:972`. The timestamp is a filing date, not authorship.


> Identified by the `cyrup-tui` hygiene audit (6-dimension fan-out, adversarially verified).
> **Priority:** high · **Effort:** small

## Description

Two source-reading guards over the `app/` run loop silently stopped checking what they were written to check when `app.rs` was split into the `app/` module tree at `40821ed`. Both failures are of the same shape — a string needle that no longer matches, swallowed by a fallback arm — and both live in `crates/cyrup-tui/src/tests/`. Fix them in one session.

**(1) Dead spinner needle.** `crates/cyrup-tui/src/tests/run_loop_input_priority.rs:56-65` iterates five ticker spellings and does `let Some(ticker_pos) = block.find(ticker) else { continue; };`. The first entry, `"_ = spinner.tick()"` (`:57`), matches nothing: the live arm is `_ = ctx.spinner.tick(),` at `crates/cyrup-tui/src/app/run.rs:310`. The ordering assertion at `:66-72` therefore never runs for the spinner — which is the one ticker the guard exists for, per the module doc at `:6-9` (`SPINNER_INTERVAL = 80 ms`, armed for the whole of every streaming turn). `checked >= 1` at `:75-80` keeps the test green off the other four. The post-split sibling `crates/cyrup-tui/src/tests/run_loop_swap_arm_reachable.rs:82` already uses the correct `"_ = ctx.spinner.tick()"`. Change `:57` to that spelling, then replace the `else { continue; }` at `:63-65` with `.unwrap_or_else(|| panic!("ticker `{ticker}` not found in the run-loop select! — if the arm was renamed, rename it here rather than losing the check"))`. Apply the identical `continue` -> `panic` change to the parallel ticker array in `run_loop_swap_arm_reachable.rs:88`, which has the same latent hole with currently-correct spellings. The fixed assertion should pass as-is: `maybe_in = input.next()` is at `run.rs:290`, above the spinner at `:310`.

**(2) Unanchored `arm_body` terminator.** `crates/cyrup-tui/src/tests/run_loop_draw_coalescing.rs:39-46` defines `arm_body(src, arm, next_arm)`, which panics on a missing start anchor but does `let end = rest.find(next_arm).unwrap_or(rest.len());` for the terminator. `crates/cyrup-tui/src/tests/render_cache_tick.rs:33-40` is a byte-identical copy. The call sites at `run_loop_draw_coalescing.rs:85` and `:197-198` pass `(ACTION_SRC, "fn on_session_event(", "ok = theme_changed")`, where `ACTION_SRC` is `include_str!("../app/run_action.rs")` (`:37`) but `ok = theme_changed` exists only in `crates/cyrup-tui/src/app/run.rs:362` — one hit crate-wide. The slice silently runs to EOF and is correct only because `on_session_event` is the last of the three fns in `run_action.rs` (`:10`, `:193`, `:242`; file is 304 lines). Change the fallback in both copies to `.unwrap_or_else(|| panic!("terminator `{next_arm}` not found after `{arm}` — if the loop was re-split, re-anchor this guard rather than reading to EOF"))`, then fix the two now-failing call sites by adding an explicit `arm_body_to_end(src, arm)` variant and using it for `on_session_event`, so slice-to-EOF is stated rather than reached by accident. Add a one-line comment at each `arm_body` call naming the file its terminator is expected to live in. All other terminators already resolve in-file and need no change (`run_loop_draw_coalescing.rs:69`, `:74`, `:131`, `:159`, `:175`; `render_cache_tick.rs:47`, `:55`, `:60`, `:78`, `:84`).

## Acceptance Criteria

- [ ] `grep -rn -F '"_ = spinner.tick()"' crates/cyrup-tui/src/tests/` returns no hits outside module-doc prose; `grep -rn -F '_ = ctx.spinner.tick()' crates/cyrup-tui/src/app/run.rs` still returns line 310
- [ ] No `unwrap_or(rest.len())` remains in `run_loop_draw_coalescing.rs` or `render_cache_tick.rs`; both `arm_body` copies panic with the needle name on a miss
- [ ] Temporarily corrupting any one ticker needle or `arm_body` terminator makes `cargo test -p cyrup-tui` FAIL rather than pass (demonstrate once, then revert)
- [ ] `cargo test -p cyrup-tui` passes (1270 tests) and `cargo build -p cyrup-tui` emits 0 warnings
- [ ] `cargo clippy -p cyrup-tui --all-targets` shows only the known pre-existing warning at escape_reassembly.rs:972

## Evidence

crates/cyrup-tui/src/tests/run_loop_input_priority.rs:56-65 (needle :57, swallowing else :63-65, assertion :66-72, checked>=1 :75-80, module doc :6-9); crates/cyrup-tui/src/app/run.rs:290,305,310,315,321,325,328,362; crates/cyrup-tui/src/tests/run_loop_swap_arm_reachable.rs:82,88,101-108; crates/cyrup-tui/src/tests/run_loop_draw_coalescing.rs:37,39-46,69,74,85,131,159,175,197-198; crates/cyrup-tui/src/tests/render_cache_tick.rs:33-40,47,55,60,78,84; crates/cyrup-tui/src/app/run_action.rs:10,193,242 (304 lines)
