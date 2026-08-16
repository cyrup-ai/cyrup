# TUI-092 — The TUI wedges and cannot be exited: input shares fate with the work loop

> **Status 2026-08-15 (QA review): IMPLEMENTED — 9/10. One residual item remains (below).**
>
> Every required change in the original spec landed and is production quality:
>
> * **§5c** — the `input.next()` arm is now select! position #2, directly under the cancel arm
>   (`app.rs:8402`), with the `biased;` comment extended to the stronger *cancel → input →
>   everything else* ordering rule. Culprit A (spinner-tick starvation of the input arm) is closed.
> * **§3** — the reader-thread liveness watchdog is fully landed: `INPUT_SERVICED` beacon bumped
>   after the draw in the input arm (`app.rs:8583`), `TERMINAL_RELEASED` guards in `suspend`
>   (`:7929`) and `edit_in_external_editor` (`:8017`), `PANIC_PRESSES`/`PANIC_MIN_GAP`,
>   `is_escalate_chord`, `hard_exit_from_reader` (drain → restore → name the wedged arm → reap
>   children → `exit(130)`), the press-driven `Escalation` ladder, and all three marked changes to
>   `crossterm_input_stream`'s reader loop.
> * **§3.1** — `restore_terminal_best_effort` closes any open synchronized update as its first
>   statement (`panic_hook.rs:57`).
> * **§5a** — `ArmGuard`/`ARM_BUDGET`/`ACTIVE_ARM`/`OVER_BUDGET_ARM` landed; all five awaiting arms
>   are guarded and an overrun surfaces in the transcript on the next healthy iteration.
> * **§5b.1** — `drain_queue` is off the loop task behind `queue_drain_tx` at all three sites
>   (`Escape`, `Alt+Up`, `/tree`), with the `None` inline fallback preserved. The abort deliberately
>   sits *after* the restore inside `apply_queue_drain` — pi's own order — a judged improvement over
>   the spec's call-site placement.
> * **§5b.2** — all five session-lifecycle `execute_command` arms run through `dispatch_lifecycle`
>   with optimistic `pending_swap_status` and the `None` inline fallback; the KNOWN-RESIDUAL
>   paragraph is gone and `host_services.rs:1055` now states the invariant the run loop upholds.
> * **§4** — `TUI-088` is closed in `07-cyrup-tui.md:367` as already-implemented/mis-diagnosed with
>   the full evidence; `TUI-067`/`TUI-068` (`:346-347`) carry the shared `merge_entries` root cause
>   and the not-a-third-instance note. No keybinding code was written.
>
> Verified: workspace `cargo check` clean; 1175/1175 `cyrup-tui` tests pass (incl.
> `run_loop_cancel_bias` and the `Ctrl+C` double-tap tests); clippy clean on touched code. The
> `cyrup-session-svc` parallel-run failures are pre-existing flakiness (pass isolated and
> single-threaded; the only session-svc change was a comment).

## Residual — the one outstanding item

### Stale upstream line citations for `handleCtrlC` (3 sites in `app.rs`)

The original spec's §9.4 carried an explicit instruction:

> `app.rs:2436` cites `interactive-mode.ts:3361-3369` for `handleCtrlC`; at this checkout it is
> `:3797-3805`. Line drift only — the body is identical. **Fix the comment while you are in the
> file.**

It was not done. Verified against the upstream checkout
([`pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts`](../../../../pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts)):
`private handleCtrlC(): void {` is at **line 3797**, so the correct citation is
`interactive-mode.ts:3797-3805`. Three comments still cite the stale `:3361-3369`:

| Site | Context |
| --- | --- |
| `crates/cyrup-tui/src/app.rs:437-438` | `last_sigint` field doc — "(Pi `handleCtrlC`, interactive-mode.ts:3361-3369)" |
| `crates/cyrup-tui/src/app.rs:2532` | `Action::Clear` arm comment in `apply_action` |
| `crates/cyrup-tui/src/app.rs:9848` | F10 test doc (`double_ctrl_c_within_500ms_exits_regardless_of_editor_contents`) |

**Fix:** replace `interactive-mode.ts:3361-3369` with `interactive-mode.ts:3797-3805` at all three
sites. Comment-only change; no behaviour, no tests affected.

## Definition of done (residual)

1. `grep -n "3361-3369" crates/cyrup-tui/src/app.rs` returns nothing.
2. All three sites cite `interactive-mode.ts:3797-3805`.
3. `cargo test -p cyrup-tui` still passes (comment-only change expected to be inert).
