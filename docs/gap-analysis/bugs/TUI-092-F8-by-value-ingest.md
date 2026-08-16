# TUI-092-F8 — Move (don't clone) event payloads through the run-loop ingest

> **Part of** [`TUI-092-progressive-lockup.md`](TUI-092-progressive-lockup.md) (the umbrella audit).
> A pure refactor of the ingest path; the production call site is the events arm, which [F3](TUI-092-F3-draw-coalescing.md) owns.
>
> **Kind** `cyrup-original` · **Severity** high · **Effort** S · **Phase driven** 3

## Coordinates with

**Natural pairing with [F3](TUI-092-F3-draw-coalescing.md):** F3's events arm owns each dequeued
event and is the only production call site for the new by-value variant. **Either lands
independently.** If F3 is not yet landed, add the owned variant and the reference wrappers now; the
existing per-event `self.ingest_session_event(&ev, &session)` call keeps using the reference wrapper
until F3 swaps it for the owned call. Nothing else changes.

---

## Evidence

[`ingest_event_rendered`](../../../crates/cyrup-tui/src/app.rs#L5871) borrows the event and
therefore clones its way through the three tool arms: `args.clone()` (`ToolExecutionStart`),
`partial_result.clone()` (`ToolExecutionUpdate`), `result.clone()` (`ToolExecutionEnd`), plus
`steering.clone()`/`follow_up.clone()` on `QueueUpdate`. The transcript APIs being called —
[`push_tool_start_rendered`](../../../crates/cyrup-tui/src/transcript.rs#L731),
[`push_tool_end_rendered`](../../../crates/cyrup-tui/src/transcript.rs#L826) — already take those
values **by value**; the clone exists only because the ingest path borrows `&ev`. The run loop owns
every event it dequeues.

**Verified in the tree:** the clones are at
[`app.rs:6005`](../../../crates/cyrup-tui/src/app.rs#L6005)/[`:6027`](../../../crates/cyrup-tui/src/app.rs#L6027)/[`:6037`](../../../crates/cyrup-tui/src/app.rs#L6037)/[`:6054`](../../../crates/cyrup-tui/src/app.rs#L6054);
the borrow is `ev: &AgentSessionEvent` at
[`app.rs:5871`](../../../crates/cyrup-tui/src/app.rs#L5871). The transcript APIs already consume by
value: `push_tool_start_rendered(…, args: Value, …)` at
[`transcript.rs:731`](../../../crates/cyrup-tui/src/transcript.rs#L731),
`push_tool_update(…, partial: Option<Value>)` at
[`:760`](../../../crates/cyrup-tui/src/transcript.rs#L760), `push_tool_end_rendered(…, result:
Option<Value>, …)` at [`:826`](../../../crates/cyrup-tui/src/transcript.rs#L826). The clones exist
**only** because the ingest path borrows `&ev`.

**Cost shape.** CPU/event ∝ payload size — a `ToolExecutionEnd` carrying a large `result` JSON is
cloned in full on every tool completion.

---

## FIX — make the ingest path by-value end-to-end, with reference wrappers kept for tests

* `ingest_event_rendered` becomes `ingest_event_rendered_owned(&mut self, ev: AgentSessionEvent,
  rendered, entry_rendered)` — the same match, but the tool/queue arms **move** `args`, `result`,
  `partial_result`, `steering`, `follow_up` into the transcript and `session_queue`.
* `ingest_event_with_extensions` computes `rendered`/`entry_rendered` from `&ev` first (unchanged),
  then moves `ev` into the owned fold.
* The existing reference signatures (`ingest_event`, `ingest_event_with_extensions` as called from
  `src/tests/`) stay as thin wrappers that `ev.clone()` — tests keep compiling untouched and pay a
  clone no production path pays.
* The run loop's events arm calls the owned variant (see [F3](TUI-092-F3-draw-coalescing.md)'s loop,
  which already owns each `ev`).

---

## Definition of done

* The production events-arm path (whether F3's drained loop or the pre-F3 single-event call)
  invokes `ingest_event_rendered_owned` and moves — never clones — `args`/`partial_result`/
  `result`/`steering`/`follow_up` into the transcript and `session_queue`.
* The reference signatures remain for `src/tests/`; in-crate tests compile untouched.

## Do not touch

The `ingest_event`/`ingest_event_with_extensions` reference signatures and their test call sites —
they are the thin-clone wrappers that keep the 253 in-crate test call sites compiling.