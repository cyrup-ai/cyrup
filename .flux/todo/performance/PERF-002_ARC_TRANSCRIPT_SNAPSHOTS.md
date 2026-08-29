---
stage: aug
status: in-progress
updated: 2026-08-29 04:16
aug_against: cyrup HEAD f3bf9f0 · pi v0.84.2 (agent-loop.ts unchanged from ported baseline)
aug_reverified: cyrup HEAD 8f49433 (= f3bf9f0 + 1 commit; zero diff in cyrup-agent/session-svc/ext)
---

# Deep-copying the transcript per turn where pi copies pointers

> **This is a parity regression, not an optimisation.** pi's working-transcript snapshot is
> a JS `.slice()` — an O(n) *pointer* copy. cyrup's port spells it `Vec::clone()`, which is
> a `.slice()` **plus a deep copy of every message's content**. The port comment at
> [`agent/run/mod.rs:63-68`](../../../crates/cyrup-agent/src/agent/run/mod.rs) says
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

> **[AUG] A THIRD whole-transcript deep-copy seam is missing from this table.** Besides the loop
> accumulator (§1) and the session seam (§4 step 4), `cyrup-ext` copies the entire transcript on
> every `agent_end` too, live whenever any extension subscribes to it:
> [`cyrup-ext/src/event.rs:536`](../../../crates/cyrup-ext/src/event.rs) —
> `AgentEvent::AgentEnd { messages } => HostEvent::AgentEnd { messages: messages.clone() }` —
> reached from [`cyrup-ext/src/subscriber.rs:65`](../../../crates/cyrup-ext/src/subscriber.rs)
> (`ExtSubscriber::on_event`, gated on `no_subscribers`). `HostEvent::AgentEnd` is declared
> `Vec<AgentMessage>` at [`event.rs:314`](../../../crates/cyrup-ext/src/event.rs) and is then
> serialized to a JSON string for the guest at
> [`cyrup-ext/src/host/live.rs:2146`](../../../crates/cyrup-ext/src/host/live.rs)
> (`serde_json::to_string(messages)`). Because `Arc<T>` serializes identically to `T`, this field
> can become `Vec<Arc<AgentMessage>>` with the guest payload byte-unchanged — see §6.

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

> **[AUG] Correction — step 1's "Every `.clone()` … then becomes a pointer copy with no other
> edit" is FALSE.** Three of the four per-turn clones do not feed a bare `Vec::clone`; they feed
> the **hook seam**, which is typed on `Vec<AgentMessage>` / `&[AgentMessage]`. Changing only the
> field types makes the compiler demand an `unwrap_or_clone` right back at the seam boundary,
> re-introducing the deep copy on the hot path. Realizing the win requires changing the hook-seam
> element type too — a cross-crate change (`cyrup-agent` → `cyrup-session-svc` → `cyrup-ext`). The
> exact sites and the recommended plan are in §6; read §6 before starting.

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
   ([`subscriber.rs:68,72`](../../../crates/cyrup-session-svc/src/subscriber.rs)) stops
   deep-copying the whole transcript once per subscriber per `agent_end`.

5. **Update the doc comment at
   [`agent/run/mod.rs:63-68`](../../../crates/cyrup-agent/src/agent/run/mod.rs)** to record
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
  **[AUG] “nearly free” is optimistic.** The field-type change is trivial, but it drags the
  hook-seam element type and three `agent_end` seams with it across three crates (§6). The change
  is mechanical and low-risk, but it is not a one-liner; budget it as a contained cross-crate
  refactor, not a five-minute edit.

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
6. **[AUG] Neither per-turn transcript snapshot deep-copies.** After the change BOTH inputs to
   `transform_context` are pointer copies: `base_messages` at
   [`stream.rs:40`](../../../crates/cyrup-agent/src/agent/run/stream.rs) **and** the extension
   seam's `HostEvent::Context` build at
   [`cyrup-ext/src/hooks.rs:135`](../../../crates/cyrup-ext/src/hooks.rs), which runs every turn
   via `PolicyHooks`' unconditional delegation (§6.8). This is the criterion that catches a
   “changed the field, kept a seam typed on `AgentMessage`” half-fix, which compiles and passes
   1–5 while quietly re-cloning at the `transform_context` or `HostEvent::Context` boundary.
   **[AUG-2] §6.8 is mandatory for this DoD** — leaving `HostEvent::Context` as `Vec<AgentMessage>`
   either fails to compile or invites a `(**m).clone()` that silently restores the per-turn copy.

---

## 6. [AUG] Research pass — verified sites, corrections, and the ordered plan

*All line numbers in §1–§4 were re-checked against cyrup HEAD `f3bf9f0` and are accurate. pi
`agent-loop.ts` is byte-identical to the ported baseline at v0.84.2 (`messages: [...context.messages,
...prompts]` at `:106`, `let messages = context.messages;` at `:284`, `newMessages.push(…)` /
`{ type:"agent_end", messages: newMessages }` at `:187/:194/:274`) — the parity claim holds.*

### 6.1 The `no Arc::make_mut` precondition is CONFIRMED

cyrup never mutates a snapshotted message in place. Grepping `\.messages\[` across
`cyrup-agent/src` finds exactly one hit, a test assertion
([`tests/round2_parity.rs:404`](../../../crates/cyrup-agent/src/tests/round2_parity.rs)). Every
write to `self.messages` / `self.new_messages` / `state.messages` is a `push` of a freshly built
message ([`turn.rs:35-36,43-44,76-77`](../../../crates/cyrup-agent/src/agent/run/turn.rs),
[`mod.rs:259-260`](../../../crates/cyrup-agent/src/agent/run/mod.rs),
[`state.rs` reducer `MessageEnd` arm](../../../crates/cyrup-agent/src/state.rs)). Notably cyrup
does **not** replicate pi's in-place `context.messages[len-1] = finalMessage`
(`agent-loop.ts:346/359/374`); cyrup pushes the final assistant message in `turn.rs` after
`stream_assistant` returns instead. So DoD #4 is structurally satisfiable and `Arc::make_mut` is
genuinely never needed. **Push sites wrap once**: `self.messages.push(Arc::new(m))`.

### 6.2 The hot path runs THROUGH the hook seam — this is the real scope

The four per-turn clones split by how they are consumed:

| clone | consumed by | element type today | to stay cheap |
| --- | --- | --- | --- |
| [`stream.rs:40`](../../../crates/cyrup-agent/src/agent/run/stream.rs) `base_messages` | `Hooks::transform_context(msgs: Vec<AgentMessage>)` **by value** | `Vec<AgentMessage>` | seam must take `Vec<Arc<AgentMessage>>` |
| [`tools/mod.rs:63`](../../../crates/cyrup-agent/src/agent/run/tools/mod.rs) `ctx_messages` | `AgentContextView.messages: &[AgentMessage]` **by borrow** | `&[AgentMessage]` | view must be `&[Arc<AgentMessage>]` |
| [`turn.rs:99`](../../../crates/cyrup-agent/src/agent/run/turn.rs) `ctx_messages` | `AgentContextView.messages` (prepare_next_turn) | `&[AgentMessage]` | " |
| [`turn.rs:160`](../../../crates/cyrup-agent/src/agent/run/turn.rs) `ctx_messages_after` | `AgentContextView.messages` (should_stop_after_turn) | `&[AgentMessage]` | " |

The seam is defined in [`cyrup-agent/src/hooks.rs`](../../../crates/cyrup-agent/src/hooks.rs):
`transform_context` takes `Vec<AgentMessage>` (`:220-226`), `convert_to_llm` takes
`&[AgentMessage]` (`:215`), `AgentContextView.messages: &'a [AgentMessage]` (`:24`), and `PostTurn`
carries the same (`:114-117`). `convert_to_llm` is fed the **output** of `transform_context`
(`stream.rs:43-49`), so its element type follows whatever `transform_context` returns.

**Real implementations that move with the seam** (each is small and mechanical — mostly `m.as_ref()`
in match arms):
- [`cyrup-agent/src/hooks.rs`](../../../crates/cyrup-agent/src/hooks.rs) — trait defaults +
  `default_convert_to_llm`.
- [`cyrup-session-svc/src/hooks.rs:179,223`](../../../crates/cyrup-session-svc/src/hooks.rs) — the
  live `convert_to_llm` / `transform_context`.
- [`cyrup-ext/src/hooks.rs:130`](../../../crates/cyrup-ext/src/hooks.rs) — the extension
  `transform_context` wrapper. **[AUG-2] NOT mechanical** — it routes the transcript through its
  own owned-`Vec` `HostEvent::Context`, a distinct per-turn deep-copy seam that needs the same Arc
  treatment as the `agent_end` trio; see **§6.8**.

### 6.3 The three `agent_end` seams (step 4 was one-third complete)

`AgentEvent::AgentEnd.messages` flows to three deep-copy sites, all fixed by making the payload
`Vec<Arc<AgentMessage>>`:

1. **Loop accumulator** — constructed at
   [`turn.rs:52,177,197`](../../../crates/cyrup-agent/src/agent/run/turn.rs),
   [`mod.rs:221`](../../../crates/cyrup-agent/src/agent/run/mod.rs) and
   [`lifecycle.rs:375`](../../../crates/cyrup-agent/src/agent/lifecycle.rs) (`vec![fm.clone()]`).
   Extractor at [`loop_fn.rs:217`](../../../crates/cyrup-agent/src/loop_fn.rs).
2. **Session seam** — [`event.rs:328-329`](../../../crates/cyrup-session-svc/src/event.rs) and
   [`subscriber.rs:196-197`](../../../crates/cyrup-session-svc/src/subscriber.rs); the field is
   declared at [`event.rs:154`](../../../crates/cyrup-session-svc/src/event.rs). Also removes the
   per-subscriber `ev.clone()` transcript copy at
   [`subscriber.rs:68,72`](../../../crates/cyrup-session-svc/src/subscriber.rs).
3. **Extension seam** — [`cyrup-ext/src/event.rs:314,536`](../../../crates/cyrup-ext/src/event.rs)
   (missed by §1; see the §1 [AUG] note). `HostEvent::AgentEnd.messages` → `Vec<Arc<AgentMessage>>`;
   [`live.rs:2146`](../../../crates/cyrup-ext/src/host/live.rs) `serde_json::to_string` is
   unchanged (Arc is transparent to serde — precondition confirmed in §6.9). This is **not** the
   only extension transcript seam — the `transform_context` path (`HostEvent::Context`) is a
   separate per-turn one; see §6.8.

**One signature ripple:**
[`retry.rs:75`](../../../crates/cyrup-session-svc/src/session/retry.rs)
`will_retry_after_agent_end(&self, messages: &[AgentMessage])` is called at `subscriber.rs:197`
with the `AgentEnd` payload; change its parameter to `&[Arc<AgentMessage>]` and deref in the
`.iter().rev()` match arm (`AgentMessage::Assistant(a)` → match on `m.as_ref()`).

**Downstream is safe:** every front-end consumer of `AgentSessionEvent::AgentEnd` matches
`{ .. }` and ignores `messages`
([`cyrup-tui/.../event_extract.rs:14`](../../../crates/cyrup-tui/src/app/event_extract.rs),
[`events_fold.rs:41`](../../../crates/cyrup-tui/src/app/events_fold.rs),
[`cyrup-modes/.../json_event.rs:144`](../../../crates/cyrup-modes/src/json_event.rs)), and
`--json`/RPC serialization is byte-identical because `Arc<T>` serializes as `T`. **DoD #3 holds.**

### 6.4 Scope decision: LEAVE `StateInner::messages` / `AgentStateSnapshot` alone (Phase 2, optional)

Step 1 lumps `AgentState::messages` ([`state.rs`](../../../crates/cyrup-agent/src/state.rs)) into
the core change. **It does not belong there for DoD #1**, and it is the single highest-blast-radius
piece:

- The four *per-turn* clones are all off `RunCtx::{messages,new_messages}` — none off
  `StateInner::messages`. `StateInner::messages` grows one message at a time in the reducer
  (O(message), not O(transcript)) and is deep-copied only in
  [`snapshot()` at `state.rs:122`](../../../crates/cyrup-agent/src/state.rs), which is an
  **accessor-path** call (`retry.rs:141`, `bash.rs:280`, `auto_compaction.rs:396`,
  `accessors.rs:316`), **not** the per-turn loop. So leaving it as `Vec<AgentMessage>` satisfies
  “a *turn's* cost no longer scales.”
- `AgentStateSnapshot.messages` is **public** (`Vec<AgentMessage>`) and consumed by ~8 session-svc
  sites that take it by value and mutate (`let mut msgs = …snapshot().messages`). Converting it
  either forces an Arc ripple across all of them or a deref-copy in `snapshot()` that defeats the
  purpose.
- **Cost of leaving it:** `RunCtx` is seeded from `state.messages.clone()`
  ([`lifecycle.rs:270`](../../../crates/cyrup-agent/src/agent/lifecycle.rs)); with `RunCtx` Arc'd
  and `StateInner` not, the seed becomes `st.messages.iter().map(|m| Arc::new(m.clone())).collect()`
  — **one** deep copy at *run start* (once per user prompt), off the per-turn path. That is a large
  reduction from `4 × turns × sizeof` to `1 × sizeof` per run and it is honest to say the per-run
  seed still scales with history; making it a pointer copy is exactly what Phase 2 buys, at the
  price of the public-snapshot ripple. **Recommend Phase 1 only; open Phase 2 as its own task with
  the snapshot-API decision recorded.**

### 6.5 Public return surfaces: keep `Vec<AgentMessage>` (one deref-clone per run)

`RunCtx::run` returns `self.new_messages.clone()`
([`mod.rs:245`](../../../crates/cyrup-agent/src/agent/run/mod.rs)); this flows to
`RunHandle::finished() -> Vec<AgentMessage>`
([`lifecycle.rs:26`](../../../crates/cyrup-agent/src/agent/lifecycle.rs)),
`run_agent_loop -> Vec<AgentMessage>` and `AgentLoopStream = FinalizingStream<AgentEvent,
Vec<AgentMessage>>` ([`loop_fn.rs`](../../../crates/cyrup-agent/src/loop_fn.rs)). Recommendation:
**keep the public return type `Vec<AgentMessage>`** and pay one `Arc::unwrap_or_clone`/`(*m).clone()`
deref-map at the single return site — a once-per-run cost, off the hot path, that avoids widening
the public API. Every in-tree caller ignores the value anyway (`let _ = handle.finished().await`,
[`run.rs:151,166`](../../../crates/cyrup-session-svc/src/session/run.rs)). Making the loop's
`agent_end` extractor (`loop_fn.rs:217`) do the same deref-map keeps the finalizing-stream result
type unchanged.

### 6.6 Cost model, confirmed

`AssistantMessage` ([`cyrup-core/src/message/assistant.rs:30`](../../../crates/cyrup-core/src/message/assistant.rs))
owns `content: Vec<Content>`, provider/model/api strings, `usage`, optional
`Vec<AssistantMessageDiagnostic>`, boxed `DeferredHandle`, `error_message`, etc. — so a `Vec` clone
deep-copies every content block's bytes, exactly the cost the task describes. `Content` blocks
(text/tool-call args/tool-result payloads) are where the bytes live; the estimate in §4 stands.

### 6.7 Ordered implementation plan for `exec`

1. **`cyrup-agent` field + seam types.** `RunCtx::{messages,new_messages}` →
   `Vec<Arc<AgentMessage>>` ([`run/mod.rs:62,69`](../../../crates/cyrup-agent/src/agent/run/mod.rs)).
   Change the hook seam element type in
   [`hooks.rs`](../../../crates/cyrup-agent/src/hooks.rs): `transform_context(Vec<Arc<AgentMessage>>)
   -> Vec<Arc<AgentMessage>>`, `convert_to_llm(&[Arc<AgentMessage>])`,
   `AgentContextView.messages: &'a [Arc<AgentMessage>]`, `PostTurn.messages`. Wrap at push sites;
   the `ctx_messages` locals become pointer-cheap `self.messages.clone()`.
2. **`AgentEvent::AgentEnd.messages` → `Vec<Arc<AgentMessage>>`**
   ([`event.rs:271`](../../../crates/cyrup-agent/src/event.rs)); update `loop_fn.rs:217` extractor
   (deref-map to keep the public stream result `Vec<AgentMessage>`, per §6.5) and the failure-path
   constructors (`mod.rs:221`, `lifecycle.rs:375`, `turn.rs:52/177/197`).
3. **Hook impls follow the seam:** `cyrup-session-svc/src/hooks.rs`, `cyrup-ext/src/hooks.rs`
   (mechanical `m.as_ref()` edits).
4. **Session seam:** `AgentSessionEvent::AgentEnd.messages`
   ([`event.rs:154`](../../../crates/cyrup-session-svc/src/event.rs)) → `Vec<Arc<AgentMessage>>`;
   fix `event.rs:328-329`, `subscriber.rs:196-197`, and `will_retry_after_agent_end`'s signature
   (`retry.rs:75`) + its match arm.
5. **Extension seam:** `HostEvent::AgentEnd.messages`
   ([`cyrup-ext/src/event.rs:314`](../../../crates/cyrup-ext/src/event.rs)) →
   `Vec<Arc<AgentMessage>>`; `event.rs:536` becomes a pointer clone; `live.rs:2146` unchanged.
   **Then also do the `transform_context` seam (§6.8):** `HostEvent::Context.messages`
   ([`event.rs:300`](../../../crates/cyrup-ext/src/event.rs)) and `EventPatch::Context.messages`
   ([`contract.rs:58`](../../../crates/cyrup-ext/src/contract.rs)) → `Vec<Arc<AgentMessage>>`; the
   build at `hooks.rs:135` becomes a pointer clone; `contract.rs:114` / `live.rs:2077` unchanged.
6. **Update the doc comment** at
   [`run/mod.rs:63-68`](../../../crates/cyrup-agent/src/agent/run/mod.rs) (step 5 of §3) to record
   that the `Arc` element type is what makes the snapshot pi's `.slice()`.
7. **Do NOT touch** `StateInner::messages` / `AgentStateSnapshot` / the public
   `RunHandle::finished` return type (§6.4/§6.5).

**Test blast radius** (for the green-suite DoD, not new tests): construction/match sites over
`AgentEnd { messages }` and `snapshot().messages` live in ~11 `cyrup-agent` test files
(`agent_loop.rs`, `pending_containment.rs`, `round2_parity.rs`, `untracked_misses.rs`, …) and a
few `cyrup-session-svc`/`cyrup-ext` tests; most match `{ .. }` or `.iter()` and need only
`m.as_ref()` derefs or `Arc::new(…)` wraps. Per flux rules this task adds **no** tests or
benchmarks — `/flux/tests` owns fixing any that the type change disturbs.

### 6.8 [AUG-2] The extension `transform_context` seam deep-copies the transcript EVERY turn — a per-turn site §6.2/§6.3 missed

§6.2 files `cyrup-ext/src/hooks.rs:130` under "mechanical `m.as_ref()` edits." It is not
mechanical. The extension `transform_context` wrapper carries the transcript through its **own
owned-`Vec` HostEvent** — `HostEvent::Context` — which is a per-turn full-transcript deep-copy in
exactly the same class as `base_messages`, on exactly the hot path DoD #6 targets. Getting
`stream.rs:40` to a pointer copy while leaving this alone means DoD #6 is *not* met with any
extension wired.

**The path, verified at HEAD `8f49433`:**

1. The loop calls `self.hooks.transform_context(base_messages)` at
   [`stream.rs:40`](../../../crates/cyrup-agent/src/agent/run/stream.rs).
2. In the wired stack that resolves to `PolicyHooks::transform_context`
   ([`cyrup-session-svc/src/hooks.rs:223`](../../../crates/cyrup-session-svc/src/hooks.rs)), whose
   body **unconditionally delegates** to the extension seam —
   `self.inner.transform_context(msgs, cancel).await`
   ([`hooks.rs:228`](../../../crates/cyrup-session-svc/src/hooks.rs)) — every turn, no gate.
3. `ExtensionHooks::transform_context`
   ([`cyrup-ext/src/hooks.rs:130`](../../../crates/cyrup-ext/src/hooks.rs)) then builds
   `HostEvent::Context { messages: msgs.clone() }`
   ([`hooks.rs:135`](../../../crates/cyrup-ext/src/hooks.rs)) — **an unconditional deep copy**,
   because `msgs.clone()` runs BEFORE the `no_subscribers` gate that lives inside
   `dispatch_block_mutate` ([`dispatch.rs:415`](../../../crates/cyrup-ext/src/dispatch.rs)). Even a
   session with no `context`-subscribing extension pays the whole-transcript copy on every turn.

**Owned-`Vec` seams that must move with the element type (identical treatment to the `agent_end`
trio in §6.3):**

| site | today | after |
| --- | --- | --- |
| [`cyrup-ext/src/event.rs:300`](../../../crates/cyrup-ext/src/event.rs) `HostEvent::Context.messages` | `Vec<AgentMessage>` | `Vec<Arc<AgentMessage>>` |
| [`cyrup-ext/src/contract.rs:58`](../../../crates/cyrup-ext/src/contract.rs) `EventPatch::Context.messages` | `Vec<AgentMessage>` | `Vec<Arc<AgentMessage>>` |
| [`cyrup-ext/src/hooks.rs:135`](../../../crates/cyrup-ext/src/hooks.rs) build | deep copy | pointer clone |
| [`cyrup-ext/src/hooks.rs:138`](../../../crates/cyrup-ext/src/hooks.rs) return `Ok(messages)` | — | unchanged (now `Vec<Arc<_>>`, matches the new `transform_context` return type) |
| [`cyrup-ext/src/contract.rs:114`](../../../crates/cyrup-ext/src/contract.rs) patch `*messages = m` | — | unchanged (both sides `Vec<Arc<_>>`) |
| [`cyrup-ext/src/host/live.rs:2077`](../../../crates/cyrup-ext/src/host/live.rs) `serde_json::to_string(messages)` | — | **unchanged** (Arc transparent, §6.9) |

The guest-return path needs **no manual `Arc::new`**: `EventPatch::Context` is deserialized from
the guest's JSON, and with serde's `rc` feature on (§6.9) a JSON array deserializes straight into
`Vec<Arc<AgentMessage>>`, wrapping each element once — the exact analogue of "push sites wrap
once." So `contract.rs:114`'s `*messages = m` stays a one-line assignment.

**Why the half-fix bites here specifically.** With `transform_context` retyped to
`Vec<Arc<AgentMessage>>` (§6.7 step 1/3) but `HostEvent::Context` left `Vec<AgentMessage>`,
`hooks.rs:135` will not compile (`Vec<Arc<_>>` → `Vec<_>` mismatch). The path-of-least-resistance
"fix" is `messages: msgs.iter().map(|m| (**m).clone()).collect()`, which compiles, passes DoD
1–5, and silently restores the exact per-turn deep copy this task exists to remove. **Change the
field, not the clone.**

*(Complementary micro-opt, independent of the Arc change: add
`if self.dispatch.no_subscribers(EventKind::Context) { return Ok(msgs); }` before `hooks.rs:135`
to elide even the pointer-`Vec` allocation when nothing subscribes to `context`. Reasonable, but
NOT a substitute — it still deep-copies whenever a `context` subscriber IS present, so the Arc
change remains required for DoD #6. Note the transcript-carrying HostEvents are exactly two:
`Context` here and `AgentEnd` in §6.3; `MessageEnd` carries a single message, and the tool seams
carry args, not the transcript.)*

### 6.9 [AUG-2] Precondition CONFIRMED: serde's `rc` feature is enabled — the "Arc is transparent to serde" premise holds

The task leans on "`Arc<T>` serializes identically to `T`" at four seams (§1 [AUG], §6.3, §6.8,
and the `live.rs` sites). That is true **only** when serde's `rc` feature is enabled; without it
`Arc<AgentMessage>` has no `Serialize`/`Deserialize` impl and every serialized/deserialized seam
in this task fails to compile. Verified at
[`cyrup/Cargo.toml:145`](../../../Cargo.toml): `serde = { version = "1", features = ["derive",
"rc"] }`, and every crate on this path takes `serde.workspace = true` (`cyrup-core`,
`cyrup-agent`, `cyrup-ext`, `cyrup-session-svc`). So the `live.rs:2077`/`:2146` byte-identical
guest serialization, the guest-patch deserialization in §6.8, and the `--json`/RPC output in §6.3
are all safe. This is a load-bearing precondition — record it so a future `rc`-feature removal is
understood to break this task, not just this seam.
