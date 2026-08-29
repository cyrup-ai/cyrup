---
stage: new
status: done
updated: 2026-08-29 01:41
---

# SEAM-112: /resume Produces A Broken Session

## Description

`/resume` produces a broken session: **nothing renders, and bash tool calls repeat
endlessly.** Owner report, filed from live use 2026-08-15. Rated `critical` on arrival.
Ledger rows: `docs/gap-analysis/08-cyrup-session-svc-and-modes.md:410`,
`docs/gap-analysis/00-residual-ledger.md:24`.

### The repetition is the load-bearing clue

A pure RENDER fault would run each command **once** and fail to display it. Running the
same command **over and over** means the model is not receiving its tool results. So this
is at minimum a context/session-rebuild fault — possibly two faults sharing one cause in
the swap path — **not the display bug it resembles.** Do not let the investigation drift
back into rendering.

### Wiring already verified at HEAD — do not begin by re-deriving it

- `AgentSessionRuntime::switch_session_with` (`crates/cyrup-session-svc/src/runtime.rs:513`)
  emits `session_before_switch` with reason `"resume"`, builds a fresh session via
  `factory.build(SessionTarget::Resume(path), cwd)`, and calls `install` → `install_inner`
  (`:387`/`:398`), which bumps the runtime `generation`.
- The TUI watches that generation, re-subscribes via `*events = new_session.subscribe()`
  (`crates/cyrup-tui/src/app/run_arms.rs:158`) and re-binds at `App::rebind_session`
  (`crates/cyrup-tui/src/app/session_bind.rs:4`).

The mechanism exists, so the defect is inside it.

### Half the symptom is already explained and fixed

A defect of exactly this shape was found and fixed on 2026-08-18 at `879eb4e`. The
`session_swapped` arm sat LAST under `biased;` while the events arm bound an IRREFUTABLE
`maybe_ev = events.next()`. `Fanout::invalidate`
(`crates/cyrup-session-svc/src/subscriber.rs:89-93`) drops every sender the instant a
replacement lands, so the disposed session's stream went permanently `Ready(None)`, won
every poll, and starved the swap arm — no re-subscribe, no `rebind_session()`, the loop
still bound to the OLD session.

At HEAD the events arm is refutable (`app/run.rs:344`, `Some(ev) = events.next()`) so a
closed stream DISABLES the branch; the swap arm is hoisted directly below the input arm
(`app/run.rs:293`); the rebind is extracted to `App::on_session_swapped`
(`app/run_arms.rs:138`) with a generation guard and ALSO runs from the input arm's
pre-dispatch reconcile; `src/tests/run_loop_swap_arm_reachable.rs` pins both structurally.
The in-source rationale at `app/run.rs:284-292` names this row's first symptom verbatim —
"the TUI up, dead".

**That accounts for "nothing renders". It does NOT obviously account for the repeated bash
calls.** That is why this row stays open.

### Candidates, cheapest first — the one explaining BOTH symptoms first

1. The rebuilt session's tool-result path is not re-wired, so results never re-enter the
   message list.
2. The generation bump fires before the new session is fully installed, so the TUI
   subscribes to a stream that is then replaced.
3. `rebind_session` resets the transcript but nothing drives the new subscription.

### Read these three tests first — none of them caught it

- `crates/cyrup-tui/src/tests/runtime_swap.rs`
- `crates/cyrup-tui/src/tests/extension_ui_reset_on_swap.rs`
- `crates/cyrup-session-svc/src/tests/session_start_lifecycle.rs`

Understanding what they assume is the point — the gap between them is where this lives.

### How to reproduce — read this before starting

Log at **tool-result append** and at **TUI subscribe/rebind**, run **ONE** `/resume`, then
read the log. The ledger is explicit: **do not characterise this by re-running it**, and do
not open with more static tracing. One live observation drives the investigation.

## Acceptance Criteria

- [ ] One live `/resume` reproduced with logging at tool-result append and at TUI
      subscribe/rebind; the captured log is the evidence, not a re-derivation from reading.
- [ ] Root cause of the **repeated bash tool calls** identified and stated — specifically
      where a tool result fails to re-enter the rebuilt session's message list.
- [ ] Established whether "nothing renders" and the repeated calls share one cause or are
      two faults; if `879eb4e` fully closed the render half, say so with evidence.
- [ ] Fix applied at the root cause, not at the symptom.
- [ ] A test pins the behaviour across a resume/swap — covering the seam the three existing
      tests each miss — and is revert-proven: neutralise the fix and show the test fails.
- [ ] `cargo check --workspace --all-targets` clean; `cyrup-tui` and `cyrup-session-svc`
      test suites pass.
- [ ] The `SEAM-112` ledger rows updated to reflect the outcome.

## Source

- **Ledger:** `SEAM-112` — severity `critical`, class `port-bug`, size `M`
- **Filed:** 2026-08-15 (live use, owner report)
- **Related fix:** `879eb4e` (2026-08-18) — closed the starved-swap-arm fault
