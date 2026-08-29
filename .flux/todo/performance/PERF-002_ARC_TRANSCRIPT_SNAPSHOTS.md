---
stage: new
status: pending
updated: 2026-08-29 02:33
---

# Deep-copying the transcript per turn where pi copies pointers

> **This is a parity regression, not an optimisation.** pi's working-transcript snapshot is
> a JS `.slice()` — an O(n) *pointer* copy. cyrup's port spells it `Vec::clone()`, which is
> a `.slice()` **plus a deep copy of every message's content**. The port comment at
> [`agent/run/mod.rs:63-64`](../../../crates/cyrup-agent/src/agent/run/mod.rs) says
> `.slice()` and means it; the type chosen underneath does something stronger and slower.

---

## 0. READ THIS FIRST — why this is not just "clone is slow"

`Vec::clone()` looks like the obvious translation of `.slice()` and it is *semantically*
correct: both produce an independent snapshot that a later mutation of the source cannot
disturb. The port is not wrong about behaviour. It is wrong about **cost**, and only
because `AgentMessage` owns its content inline:

| | pi `.slice()` | cyrup `Vec<AgentMessage>::clone()` | cyrup `Vec<Arc<AgentMessage>>::clone()` |
| --- | --- | --- | --- |
| copies | n pointers | n messages, **all bytes** | n pointers + n refcount bumps |
| cost | O(n) | **O(total transcript bytes)** | O(n) |
| allocations | 1 | 1 + one per string in every message | 1 |
| snapshot isolation | yes | yes | yes |

The third column is what the doc comment already describes. Getting there is a type change,
not a semantics change: `AgentMessage` is only ever *read* through these snapshots — the
loop mutates `self.messages` by `push`, never by editing a message in place — so shared
ownership is safe and no `Arc::make_mut` is needed.

**Do not "fix" this by taking references instead.** The snapshot must survive an
independent mutation of `self.messages` (that is the whole point of `.slice()`, and
[`stream.rs:37-40`](../../../crates/cyrup-agent/src/agent/run/stream.rs) states the case:
a `prepare_next_turn` context override or a mid-run external `set_messages` "must not cross
between the two"). A borrow cannot express that. `Arc` can.

---

## 1. Where it happens

**Four deep clones per turn**, all of the full working transcript:

| site | binding | why it exists |
| --- | --- | --- |
| [`agent/run/stream.rs:40`](../../../crates/cyrup-agent/src/agent/run/stream.rs) | `base_messages` | the loop's own working copy for the request (pi `context.messages`, `agent-loop.ts:283`) |
| [`agent/run/tools/mod.rs:63`](../../../crates/cyrup-agent/src/agent/run/tools/mod.rs) | `ctx_messages` | per-call hook context view (pi `currentContext.messages`, `agent-loop.ts:691`) |
| [`agent/run/turn.rs:99`](../../../crates/cyrup-agent/src/agent/run/turn.rs) | `ctx_messages` | `prepare_next_turn` context |
| [`agent/run/turn.rs:160`](../../../crates/cyrup-agent/src/agent/run/turn.rs) | `ctx_messages_after` | `should_stop_after_turn` re-snapshot |

Plus the accumulator and state copies on the same path:

| site | what |
| --- | --- |
| [`agent/run/turn.rs:52,177,197`](../../../crates/cyrup-agent/src/agent/run/turn.rs) | `self.new_messages.clone()` into `AgentEvent::AgentEnd` (three sites) |
| [`agent/run/mod.rs:245`](../../../crates/cyrup-agent/src/agent/run/mod.rs) | `self.new_messages.clone()` |
| [`agent/lifecycle.rs:180,270`](../../../crates/cyrup-agent/src/agent/lifecycle.rs) | `state.messages.clone()` |
| [`state.rs:122`](../../../crates/cyrup-agent/src/state.rs) | `messages: self.messages.clone()` |
| [`loop_fn.rs:120`](../../../crates/cyrup-agent/src/loop_fn.rs) | `working_messages` |
| [`session-svc/subscriber.rs:197`](../../../crates/cyrup-session-svc/src/subscriber.rs), [`event.rs:329`](../../../crates/cyrup-session-svc/src/event.rs) | `messages.clone()` crossing the session seam |

The declaration to change is
[`agent/run/mod.rs:62,69`](../../../crates/cyrup-agent/src/agent/run/mod.rs):

```rust
    new_messages: Vec<AgentMessage>,
    …
    messages: Vec<AgentMessage>,
```

## 2. What pi does

[`agent-loop.ts:163,186,219,229,234`](../../../../pi/packages/agent/src/agent-loop.ts):

```ts
	let currentContext = initialContext;
	…
					currentContext.messages.push(message);
	…
				currentContext.messages.push(result);
	…
					context: currentContext,
	…
				currentContext = nextTurnSnapshot.context ?? currentContext;
```

`currentContext` is passed by reference everywhere; the only copy in the loop is the
initial `.slice()` at `agent-loop.ts:104-107`. Nothing in pi's turn deep-copies message
content, because JS has no way to.

---

## 3. Required implementation

1. **Change the element type to `Arc<AgentMessage>`** on `RunCtx::messages`,
   `RunCtx::new_messages`, and `AgentState::messages`
   ([`state.rs`](../../../crates/cyrup-agent/src/state.rs)). Every `.clone()` listed in §1
   then becomes a pointer copy with no other edit.

2. **Push sites wrap once.** `self.messages.push(m)` → `self.messages.push(Arc::new(m))`.
   The message is constructed exactly once per turn, so this adds one allocation where
   the old code performed one per *snapshot*.

3. **Read sites deref.** `AgentMessage` is consumed by value in a handful of places
   (`AgentMessage::Assistant(partial.clone())` and the `AgentEnd` payloads). Where an owned
   value is genuinely required, `Arc::unwrap_or_clone` gives the old behaviour at the one
   site that needs it instead of at every snapshot.

4. **Carry the `Arc` across the session seam.** `AgentEvent::AgentEnd`'s `messages` and
   `AgentSessionEvent`'s equivalent
   ([`event.rs:329`](../../../crates/cyrup-session-svc/src/event.rs)) should hold
   `Vec<Arc<AgentMessage>>` too, so `Fanout::emit`'s per-subscriber `ev.clone()`
   ([`subscriber.rs:67,71`](../../../crates/cyrup-session-svc/src/subscriber.rs)) stops
   deep-copying the whole transcript once per subscriber per `agent_end`.

5. **Update the doc comment at
   [`agent/run/mod.rs:63-66`](../../../crates/cyrup-agent/src/agent/run/mod.rs)** to record
   that the `Arc` element type is what makes this snapshot pi's `.slice()` rather than
   something stronger — so the next reader does not "simplify" it back to
   `Vec<AgentMessage>`.

**Do not** reach for `im::Vector` or another persistent-collection crate. The snapshot is
whole-vector and the mutation is append-only, so `Vec<Arc<_>>` already gives O(n) pointer
copies; a persistent vector would add a dependency to turn O(n) into O(1) on a value of n
in the hundreds. Not worth it. (If profiling later shows the O(n) pointer copy itself
mattering, that is a separate task with its own evidence.)

---

## 4. Honest sizing — read this before prioritising

**This was not measured end to end, and it is much smaller than
[PERF-001](PERF-001_STREAM_SNAPSHOT_QUADRATIC.md).** The structural cost is
`4 × sizeof(transcript)` of `memcpy` plus one allocation per string per copy, per turn.
For a 400 KB transcript that is ~1.6 MB and a few hundred microseconds; for a 10 MB
transcript (a long session with large tool outputs) it is ~40 MB and single-digit
milliseconds per turn.

Two reasons it is still worth doing despite the modest absolute number:

- **It grows without bound with session length.** The cost is proportional to history, so
  it is worst exactly when the user has the most invested in the session — the opposite of
  where you want a cost curve.
- **It is nearly free to fix and it removes a way to be slower than the thing we port.**

If a measurement contradicts the estimate above, put the measurement in this file and
re-prioritise. Do not let the estimate stand in for a number once a number is available.

---

## 5. Definition of Done

1. **A turn's cost no longer scales with transcript size.** Prompting in a session with a
   10 MB transcript costs the same per-turn snapshot work as one with a 100 KB transcript.
2. **Snapshot isolation is intact.** A `prepare_next_turn` context override, and a mid-run
   external `set_messages`, still do not cross between the loop's working copy and the
   agent's observable `state.messages` — the invariant
   [`stream.rs:37-40`](../../../crates/cyrup-agent/src/agent/run/stream.rs) documents.
3. **`AgentEnd` and `MessageUpdate` payloads are unchanged** in content and ordering, for
   every subscriber, including a session with more than one live subscriber.
4. **No `Arc::make_mut` anywhere on this path.** If one is needed, a message is being
   mutated in place after being snapshotted, and that is a bug in the change, not a
   requirement of it.
5. **The suite is green under the real gate:**
   `cargo test --workspace --features test-fixtures --no-fail-fast`, and
   `cargo clippy --workspace --all-targets --features test-fixtures` exits **0**.
