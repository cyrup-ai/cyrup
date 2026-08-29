---
stage: exec
status: done
updated: 2026-08-29 02:14
---



# SEAM-112: /resume Produces A Broken Session

## Objective

`/resume` produces a broken session: **nothing renders, and bash tool calls repeat
endlessly.** Filed 2026-08-15 from live use, `critical`.

The render half is closed (`879eb4e`). The open half was: *why do the bash calls repeat?*

**That is now answered in source, and the fix is a two-line change.** No live reproduction is
required, and none is requested. Read "Root cause" below, apply "The change", update the two
ledger rows, done.

---

## Root cause — SETTLED

**cyrup's one-shot overflow-recovery brake is unarmable for `StopReason::Length`, because the
very message that arms it clears it first. The port dropped one arm of pi's guard.**

### The divergence, verbatim

pi @v0.84.3 [`agent-session.ts:677-680`](../../tmp/pi/packages/coding-agent/src/core/agent-session.ts):

```ts
const assistantMsg = event.message as AssistantMessage;
if (assistantMsg.stopReason !== "error" && assistantMsg.stopReason !== "length") {
    this._overflowRecoveryAttempted = false;
}

// Reset retry counter immediately on successful assistant response
if (assistantMsg.stopReason !== "error" && this._retryAttempt > 0) { /* auto_retry_end */ }
```

cyrup [`session/run.rs:273-279`](../../crates/cyrup-session-svc/src/session/run.rs):

```rust
pub(crate) async fn on_assistant_message_end(&self, assistant: &AssistantMessage) {
    *Self::lock(&self.last_assistant) = Some(assistant.clone());
    if assistant.stop_reason == cyrup_core::StopReason::Error {
        return;                                   // <- covers ERROR only
    }
    *Self::lock(&self.overflow_recovery_attempted) = false;   // <- LENGTH reaches here
    // ... retry-counter reset ...
}
```

The port fused pi's **two independently-guarded** statements into **one** early `return` and, in
doing so, kept only the predicate they share (`!== "error"`). **`&& stopReason !== "length"` was
lost.** The doc comment above the function records the drift in prose — it says "on a non-error
response", where upstream is "on a response that is neither an error nor a length stop".

### Why that makes the loop unbounded

`overflow_recovery_attempted` ([`session/mod.rs:242`](../../crates/cyrup-session-svc/src/session/mod.rs))
is the only brake on the compact-and-retry cycle. It is read and set in
[`auto_compaction.rs:85`,`:100`](../../crates/cyrup-session-svc/src/session/auto_compaction.rs):

```rust
if same_model && is_context_overflow(assistant, Some(window)) {
    let will_retry = assistant.stop_reason != cyrup_core::StopReason::Stop;
    if !will_retry { return self.run_auto_compaction(CompactionReason::Overflow, false).await; }
    if *Self::lock(&self.overflow_recovery_attempted) {
        /* emit CompactionEnd{error_message: "Context overflow recovery failed after one
           compact-and-retry attempt..."} */
        return Ok(false);                                  // <- the brake
    }
    *Self::lock(&self.overflow_recovery_attempted) = true;
    self.drop_trailing_assistant().await;
    return self.run_auto_compaction(CompactionReason::Overflow, will_retry).await;
}
```

[`is_context_overflow`](../../crates/cyrup-provider/src/utils/overflow.rs) (`:79-113`) fires on
exactly three shapes:

| case | stop reason | condition | `will_retry` |
| --- | --- | --- | --- |
| 1 | `Error` | error text matches an overflow pattern | **true** |
| 2 | `Stop` | `input + cache_read > window` | false |
| 3 | `Length` | `usage.output == 0 && input*100 >= window*99` | **true** |

So the brake is consulted on **case 1 and case 3 only**. Now order the two handlers — the
subscriber runs `on_assistant_message_end` on `message_end`, and `check_compaction` runs later
from `handle_post_agent_run` on `agent_end` ([`run.rs:235`](../../crates/cyrup-session-svc/src/session/run.rs)):

- **Case 1 (`Error`)** — `run.rs:275` returns early, the latch survives, `:85` can see `true`.
  The brake works. Parity-correct.
- **Case 3 (`Length`)** — `run.rs:275` does **not** return, `:278` sets the latch to `false`,
  and *then* `:85` reads it. **The read is `false` on every pass, unconditionally.** `:85-98` is
  unreachable code on this path; `:100` re-sets a latch that the next `Length` message will
  clear again.

```
Length overflow -> latch CLEARED at run.rs:278 -> :85 reads false -> :100 set true
   -> drop_trailing_assistant -> compact -> will_retry -> continue_run
   -> the interrupted turn re-runs, re-issuing the SAME bash call
   -> next assistant is Length again -> latch CLEARED again -> ... forever
```

pi's cycle terminates after **exactly one** attempt. cyrup's has **no termination condition at
all** — not "usually loops", not "loops if compaction is weak": the guard can never be observed
`true`, so the loop is structurally unbounded. Every pass re-drives the interrupted turn, so
every pass re-executes the identical bash command. **That is the reported symptom, exactly, and
it is the only unbounded loop in the audited surface.**

A resumed session is the natural way in: it starts at or near the context limit, so the very
first turn can land in case 3 (`input >= 99% of window`, output truncated to nothing).

### Two independent corroborations in this repo

1. **cyrup already knows `Error | Length` is the retriable pair.**
   [`auto_compaction.rs:399-404`](../../crates/cyrup-session-svc/src/session/auto_compaction.rs)
   re-drops the trailing message when `matches!(a.stop_reason, StopReason::Error | StopReason::Length)`,
   with the in-source comment "remove the retriable error **or truncated-length** response again
   before continuing the interrupted turn." The compaction side of the port carries both arms;
   the latch side carries one. **The asymmetry is the bug.**
2. **The failure mode has already been observed here.** `PROV-069`'s closure note
   ([`01-cyrup-core-and-provider.md:330`](../../docs/gap-analysis/01-cyrup-core-and-provider.md))
   records that a port of pi's `isRecoverableLength` "was drafted and REVERTED because it routes
   a truncated turn into `run_auto_compaction`, which ... trades truncation for **compaction
   spam**." Compaction spam is what an unbrakeable `Length` → compact-and-retry cycle produces.
   That note is describing this defect from the other side.

---

## Settled — do not re-investigate

**Candidate (1), "the rebuilt session's tool-result path is not re-wired." DEAD in both halves.**
Tool results reach agent state and the session file synchronously, in the same awaited call:
[`state.rs:173`](../../crates/cyrup-agent/src/state.rs) pushes into `st.messages` on `MessageEnd`
"while the state lock is held, BEFORE subscribers are awaited"; all three emit sites
([`exec.rs:249-250`](../../crates/cyrup-agent/src/agent/run/tools/exec.rs), `exec.rs:361-362`,
[`mod.rs:110-111`](../../crates/cyrup-agent/src/agent/run/tools/mod.rs)) `.await?` a
`MessageStart`/`MessageEnd` pair, and [`subscriber.rs:171`](../../crates/cyrup-session-svc/src/subscriber.rs)
appends to the session tree on `MessageEnd`. There is no window. The resume seed is likewise
intact: [`builder.rs:1499`](../../crates/cyrup-session-svc/src/builder.rs) seeds through
`raw_message_to_agent` ([`event.rs:418`](../../crates/cyrup-session-svc/src/event.rs)), whose
`Core` arm has an explicit `Message::ToolResult → AgentMessage::ToolResult` preserving
`tool_call_id`.

**Candidate (2), the generation bump racing the install. FALSE, structurally impossible.**
[`runtime.rs`](../../crates/cyrup-session-svc/src/runtime.rs) `install_inner` assigns session
(`:445`) and generation (`:446`) under one write lock, notifying only after (`:449`).

**Candidate (3), nothing drives the new subscription. FALSE.** `on_session_swapped`
([`run_arms.rs:143-315`](../../crates/cyrup-tui/src/app/run_arms.rs)) re-subscribes (`:163`),
repoints the loop handle (`:164`) and replays the conversation (`:285`,`:290`).

**The previous augmentation's proposed fix — "widen the early return to cover `ToolUse`" — is
WRONG. Do not apply it.** Three reasons:
1. `is_context_overflow` never returns `true` for a `ToolUse` message (table above), so a
   `ToolUse` turn can never be the turn that consults the latch. Suppressing its clear fixes
   nothing.
2. pi clears the latch on `ToolUse` (`"toolUse" !== "error" && !== "length"`). Suppressing it
   would *introduce* a divergence into code that is currently correct.
3. It would leave the actual divergence — `Length` — in place.

---

## The change

### A. The fix — [`crates/cyrup-session-svc/src/session/run.rs:269-279`](../../crates/cyrup-session-svc/src/session/run.rs)

**Split the two guards.** Do **not** widen the early `return`: pi resets the retry counter on a
`length` stop (its second `if` is guarded by `!== "error"` alone), so returning early for
`Length` would fix the latch and break the retry counter in one stroke.

Replace:

```rust
    /// The subscriber's `message_end` handler for an ASSISTANT message (Pi `_handleAgentEvent` tail,
    /// agent-session.ts:562-577): track the last assistant message (drives the post-run loop) and — on
    /// a non-error response — clear the overflow latch and reset the retry counter, emitting
    /// `auto_retry_end{success:true}` if a retry sequence was in flight.
    pub(crate) async fn on_assistant_message_end(&self, assistant: &AssistantMessage) {
        *Self::lock(&self.last_assistant) = Some(assistant.clone());
        if assistant.stop_reason == cyrup_core::StopReason::Error {
            return;
        }
        *Self::lock(&self.overflow_recovery_attempted) = false;
```

with:

```rust
    /// The subscriber's `message_end` handler for an ASSISTANT message (Pi `_handleAgentEvent` tail,
    /// agent-session.ts:673-694 @v0.84.3): track the last assistant message (drives the post-run
    /// loop), clear the overflow latch on a response that is neither an error NOR a length stop, and
    /// reset the retry counter on any non-error response, emitting `auto_retry_end{success:true}` if
    /// a retry sequence was in flight.
    ///
    /// SEAM-112 — the two clears carry DIFFERENT predicates upstream and must not be fused into one
    /// early return. pi guards the latch with `stopReason !== "error" && stopReason !== "length"`
    /// (`agent-session.ts:678`) and the retry counter with `stopReason !== "error"` alone (`:684`).
    /// cyrup kept only the shared arm, so every `Length` message cleared the latch here — i.e.
    /// immediately BEFORE `check_compaction` reads it (`auto_compaction.rs:85`) for the overflow
    /// case a `Length` message triggers (`is_context_overflow` case 3, `overflow.rs:101-109`). The
    /// read was therefore always `false`, the one-shot brake at `:85-98` was unreachable, and
    /// overflow recovery re-compacted and re-drove the interrupted turn without bound — re-running
    /// the same tool call on every pass. `Length` is a retriable, NOT-completed response here for
    /// the same reason it is one at `auto_compaction.rs:399-404`.
    pub(crate) async fn on_assistant_message_end(&self, assistant: &AssistantMessage) {
        *Self::lock(&self.last_assistant) = Some(assistant.clone());
        if assistant.stop_reason == cyrup_core::StopReason::Error {
            return;
        }
        if assistant.stop_reason != cyrup_core::StopReason::Length {
            *Self::lock(&self.overflow_recovery_attempted) = false;
        }
```

Everything below that line (the `attempt` read and the `AutoRetryEnd` emit) is unchanged and
must keep running for `Length`.

Resulting truth table, which is pi's exactly:

| stop reason | latch cleared | retry counter reset |
| --- | --- | --- |
| `Error` | no | no |
| `Length` | **no** *(was: yes)* | yes |
| `Stop` / `ToolUse` / `Aborted` / `Deferred` | yes | yes |

### B. Ledger citations

Repoint the drifted references in the `SEAM-112` rows
([`08-cyrup-session-svc-and-modes.md:410`](../../docs/gap-analysis/08-cyrup-session-svc-and-modes.md),
[`00-residual-ledger.md:24`](../../docs/gap-analysis/00-residual-ledger.md)), strike candidates
(1)–(3) as disproved, and record the root cause and fix.

| Row cites | Reality at HEAD |
| --- | --- |
| `run.rs:344` (`Some(ev) = events.next()`) | **`cyrup-tui/src/app/run.rs:397`** |
| `run.rs:293` (swap arm) | **`:331`** (`swapped = session_swapped`) |
| `run_arms.rs:158` (re-subscribe) | **`run_arms.rs:163`/`:164`** |
| `agent.rs:1985-2026` (`continue_run`) | **file split** — [`agent/lifecycle.rs:209`](../../crates/cyrup-agent/src/agent/lifecycle.rs), [`loop_fn.rs:200`,`:280`](../../crates/cyrup-agent/src/loop_fn.rs) |
| `session.rs:5550-5562`, `session.rs:4868`, `:4775-4781` | **no such file** — the split is `session/run.rs`, `session/auto_compaction.rs`, `session/retry.rs` |
| `subscriber.rs:89-93`, `runtime.rs:513`, `session_bind.rs:4` | correct |

Note in the row that `auto_compaction.rs`'s in-source comments still cite the pre-split
`session.rs:4868` / `:4775-4781` offsets (`:314`, `:326`, `:383`, `:392`); repoint them to
`session/auto_compaction.rs:78` and `session/retry.rs:140` while editing that file.

---

## Out of scope — with reasons, so they are not re-attempted

- **Porting pi's `isRecoverableLength`** ([`overflow.ts:171-173`](../../tmp/pi/packages/ai/src/utils/overflow.ts),
  joined into the Case-1 `if` at `agent-session.ts:2135`). It is **deliberately absent**: tracked
  as PARITY-GAPS **VL-P10** ([`PARITY-GAPS.md:839`](../../docs/gap-analysis/PARITY-GAPS.md)) and
  explicitly drafted-and-reverted under `PROV-069`. It widens the compact-and-retry trigger; it is
  a different ticket, and landing it here would enlarge the very cycle this task is closing.
- **The `ToolUse` latch clear** — upstream-faithful, see above.
- **The swap path** (`on_session_swapped`, `install_inner`, `Fanout::invalidate`) — correct at HEAD.
- **The resume seed's tool-result fidelity** — proved intact.
- **The "nothing renders" half** — closed by `879eb4e`.
- **Persisting or de-duplicating tool results** — already synchronous.
- **Any cap, counter, or circuit-breaker on the retry loop.** The brake already exists at
  `auto_compaction.rs:85`; it was simply never reachable. Make it reachable, add nothing.

---

## Definition of done

1. `on_assistant_message_end` no longer clears `overflow_recovery_attempted` for
   `StopReason::Length`, and still resets the retry counter for it — the truth table above holds.
2. The change is the guard split shown in A, not a widened early `return`, and nothing else in
   the function moved.
3. `auto_compaction.rs:85`'s `if *Self::lock(&self.overflow_recovery_attempted)` arm is reachable
   for a `Length`-triggered overflow: a second consecutive `Length` overflow emits
   `CompactionEnd { will_retry: false, error_message: Some("Context overflow recovery failed
   after one compact-and-retry attempt. …") }` and `check_compaction` returns `Ok(false)`, so
   `handle_post_agent_run` returns `false` and the run settles instead of re-driving the turn.
4. The stale in-source citations in `auto_compaction.rs` (`:314`, `:326`, `:383`, `:392`) point
   at the post-split paths.
5. The two `SEAM-112` ledger rows carry the corrected citations, the three struck candidates, and
   the root cause + fix.
6. `cargo check --workspace --all-targets` clean.
