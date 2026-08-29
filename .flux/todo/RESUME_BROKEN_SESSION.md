---
stage: aug
status: done
updated: 2026-08-29 01:59
---

# SEAM-112: /resume Produces A Broken Session

## Objective

`/resume` produces a broken session: **nothing renders, and bash tool calls repeat
endlessly.** Filed 2026-08-15 from live use, `critical`.

The render half is closed. **The open question is narrowly: why do the bash calls repeat?**

---

## Audit results — settled, do not re-investigate

### The ledger's three candidates: two dead, one narrowed to nothing

**Candidate (2) — the generation bump racing the install. FALSE, structurally impossible.**
[`runtime.rs`](../../crates/cyrup-session-svc/src/runtime.rs) `install_inner` assigns the
session and the generation under the **same write lock** (`:445`, `:446`) and only then sends
the watch notify (`:449`). No window exists.

**Candidate (3) — nothing drives the new subscription. FALSE.** `on_session_swapped`
([`run_arms.rs:143-315`](../../crates/cyrup-tui/src/app/run_arms.rs)) re-subscribes (`:163`),
repoints the loop's handle (`:164`), and replays the conversation (`:285`, `:290`), plus ui
sinks, read-backs, shortcuts, auth, context usage and the title. This is why "nothing renders"
is closed.

**Candidate (1) — "the rebuilt session's tool-result path is not re-wired." DEAD in both
halves.** The previous augmentation narrowed this to the live path and proposed that a tool
result present in agent state but not yet persisted is erased by a wholesale re-seed. **That
hypothesis is now disproved.** Two independent proofs:

1. **Agent state gets it synchronously.** [`state.rs:173`](../../crates/cyrup-agent/src/state.rs)
   — `reduce` pushes every message into `st.messages` on `MessageEnd`, and its own doc states
   it is "called while the state lock is held, BEFORE subscribers are awaited."
2. **The session file gets it in the same awaited call.** All three tool-result sites emit
   `MessageStart` then `MessageEnd` with `.await?` —
   [`exec.rs:249-250`](../../crates/cyrup-agent/src/agent/run/tools/exec.rs), `exec.rs:361-362`,
   [`mod.rs:110-111`](../../crates/cyrup-agent/src/agent/run/tools/mod.rs) — and the subscriber
   appends the finalized message to the session tree on `MessageEnd`
   ([`subscriber.rs:171`](../../crates/cyrup-session-svc/src/subscriber.rs)).

There is no window. **Stop looking for a lost tool result.**

The resume seed is likewise fine: [`builder.rs:1499`](../../crates/cyrup-session-svc/src/builder.rs)
seeds through `raw_message_to_agent`
([`event.rs:418`](../../crates/cyrup-session-svc/src/event.rs)), whose `Core` arm carries an
explicit `Message::ToolResult → AgentMessage::ToolResult` arm preserving `tool_call_id`.

### The row's citations have drifted — navigate by symbol

| Row cites | Reality at HEAD |
| --- | --- |
| `run.rs:344` (`Some(ev) = events.next()`) | **`:397`** |
| `run.rs:293` (swap arm) | **`:331`** (`swapped = session_swapped`) |
| `run_arms.rs:158` (re-subscribe) | **`:163`/`:164`** |
| `agent.rs:1985-2026` (`continue_run`) | **file split** — [`agent/lifecycle.rs:209`](../../crates/cyrup-agent/src/agent/lifecycle.rs), [`loop_fn.rs:200`,`:280`](../../crates/cyrup-agent/src/loop_fn.rs) |
| `session.rs:5550-5562` | **no such file** |
| `subscriber.rs:89-93`, `runtime.rs:513`, `session_bind.rs:4` | correct |

---

## The leading hypothesis — the overflow latch is not one-shot

Every in-source `SEAM-112` marker sits on compaction / re-seed / retry, not on the swap path.
[`round8_postrun.rs:177`](../../crates/cyrup-session-svc/src/tests/round8_postrun.rs) states it
outright: *"after a successful OVERFLOW compaction the interrupted turn must actually be
RETRIED."*

The overflow guard is `overflow_recovery_attempted`
([`session/mod.rs:242`](../../crates/cyrup-session-svc/src/session/mod.rs)). It is read and set
in [`auto_compaction.rs:85`,`:100`](../../crates/cyrup-session-svc/src/session/auto_compaction.rs)
to make overflow recovery one-shot. It is **cleared in two places**
([`run.rs:247`,`:278`](../../crates/cyrup-session-svc/src/session/run.rs)) — and the second is
the problem:

```rust
// run.rs:273 — on_assistant_message_end
if assistant.stop_reason == cyrup_core::StopReason::Error {
    return;
}
*Self::lock(&self.overflow_recovery_attempted) = false;
```

The early return covers **only** `StopReason::Error`. A normal tool-calling turn ends with
`StopReason::ToolUse`, falls through, and **clears the latch**. So in a tool-calling loop the
"one-shot" guard is re-armed on every single turn.

Combine that with the retry tail (`auto_compaction.rs:375`, `will_retry` → `continue_run`
re-driving the interrupted turn) and the cycle is:

```
overflow -> latch was false -> compact -> will_retry -> continue_run
   -> turn re-runs the SAME bash call -> assistant message_end (ToolUse) -> latch CLEARED
   -> overflow -> compact -> ... unbounded
```

Each pass re-executes the identical bash command. **This is the reported symptom exactly.**

A resumed session is the natural way to enter it: it starts at or near the context limit, so
the very first turn can overflow.

**The cycle terminates only if compaction actually shrinks the context.** The same file
documents a prior bug of precisely that shape — compaction "reported success while the very
next turn still shipped the ENTIRE pre-compaction history to the provider: zero token
reduction, full cost" (`auto_compaction.rs:327`). If any resumed-session path leaves the
re-seed ineffective, the loop never exits.

**This is a hypothesis with a specific numeric check, not a conclusion.** It is unproven until
the live run below confirms it. It is recorded because it explains an *endless* repeat — which
none of the row's three candidates does, and which the previous augmentation's hypothesis
cannot, now that both halves of candidate (1) are dead.

---

## The work

### 1. Instrument, reproduce ONCE, read

Log at exactly these points, then run **one** `/resume` on a session large enough to overflow,
let the bash call repeat two or three times, and stop. Do not characterise by re-running.

- **latch cleared** — `run.rs:277`: log `assistant.stop_reason` on every clear. This is the
  load-bearing line; a `ToolUse` here is the hypothesis firing.
- **latch read / set** — `auto_compaction.rs:85` and `:100`.
- **compaction effectiveness** — `auto_compaction.rs:340`: log `estimated_tokens_after`
  alongside the model's context window, and the seeded message count.
- **retry decision** — `auto_compaction.rs:375`: log `will_retry`.
- **tool result** — `turn.rs:76`: log `tool_call_id` and `tool_name`, to correlate the repeats.

### 2. Read the log against this decision tree

- **`ToolUse` appears on a latch clear, and compaction repeats** → the hypothesis is
  confirmed. Fix at `run.rs:277`: the clear must not re-arm overflow recovery for a turn that
  merely ended in a tool call. Restrict it to a genuinely completed response — pi's own
  intent at `agent-session.ts:562-577` is a *non-error response*, and a `ToolUse` turn is not
  a completed response. Widen the early return to cover `ToolUse` (and any stop reason that
  implies the turn is still in flight) rather than removing the clear, which would break the
  legitimate reset after a real answer.
- **`estimated_tokens_after` does not drop, or drops but stays above the window** → the
  compaction is ineffective on this path; the latch is then a symptom and the fix is at the
  re-seed. Follow the ordering already established at `auto_compaction.rs:307` (success path
  only) and `:375` (re-drop the retriable tail).
- **The latch stays set and the loop still repeats** → the driver is not overflow recovery;
  fall back to logging at `agent/lifecycle.rs:209` and `loop_fn.rs:200`/`:280` to see what is
  re-entering the run.
- **No compaction at all in the log** → the repeat is not compaction-driven; the remaining
  surface is the request projection, where `AgentMessage::ToolResult` maps back at
  [`hooks.rs:185`](../../crates/cyrup-agent/src/hooks.rs) — check `tool_call_id` correlation.

### 3. Fix at the root cause

Do not suppress the repeat. A guard that merely caps the loop leaves a session that silently
burns the context window.

### 4. Correct the row's stale citations

Repoint the drifted references in the `SEAM-112` rows
([`08-cyrup-session-svc-and-modes.md:410`](../../docs/gap-analysis/08-cyrup-session-svc-and-modes.md),
[`00-residual-ledger.md:24`](../../docs/gap-analysis/00-residual-ledger.md)) to the symbols in
the drift table, and strike candidates (1)–(3), all now disproved.

---

## Out of scope

- The swap path (`on_session_swapped`, `install_inner`, `Fanout::invalidate`) — correct at HEAD.
- The resume seed's tool-result fidelity — proved intact.
- The "nothing renders" half, closed by `879eb4e`.
- Persisting or de-duplicating tool results — they are already persisted synchronously.

---

## Definition of done

1. One instrumented `/resume` captured, and the log names which decision-tree branch fired.
2. The reason the model re-issues the identical call is stated in one sentence grounded in
   that log.
3. The fix is applied at that branch; if it is the latch, the clear no longer re-arms overflow
   recovery on a turn that ended in a tool call, while still resetting after a real response.
4. A second `/resume` under the same conditions no longer repeats the call.
5. Temporary instrumentation removed; `cargo check --workspace --all-targets` clean.
6. The `SEAM-112` rows carry corrected citations and the three disproved candidates struck.
