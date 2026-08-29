---
stage: aug
status: done
updated: 2026-08-29 16:56
aug_against: cyrup HEAD f3bf9f0 · pi v0.84.2 (agent-loop.ts unchanged from ported baseline)
aug_reverified: cyrup HEAD 8f49433 — AUG-3 pass re-checked every cyrup file:line EXACT; serde `rc` + clone-before-gate preconditions re-confirmed; pi at v0.84.2-48 (see §7)
aug_revised: cyrup HEAD 7913760 — **PERF-001 LANDED AND CHANGED THIS TASK'S COST MODEL.** §0/§4/§6.6 restated, 8 line numbers corrected, 6 missed seam sites added, 1 new in-scope win found. **READ §8 BEFORE §0.**
---

> ## ⚠️ START AT [§8](#8-aug-4-revision-pass--head-7913760-after-perf-001-landed)
> PERF-001 merged (`04d6fa5` → `b8a53db` → HEAD `7913760`) and it rewrote the exact types this
> task is about: `AgentMessage::Assistant` is **already** `Arc<AssistantMessage>`, `Content::Text`
> is **already** a refcounted `SharedStr`, and `ToolCall::arguments` is **already** a refcounted
> `LazyArgs`. The headline claim below — "a deep copy of every message's content, all bytes" — is
> **no longer true for text**, which is the bulk of a transcript. The task is still worth doing and
> the plan in §6.7 is still correct, but **the justification, the sizing, and eight `file:line`
> citations changed**. §8 supersedes §0, §4, §6.6, DoD #1, and parts of §6.2/§6.7.

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

> **[AUG-4] The middle column is now WRONG — PERF-001 already fixed most of it.** At HEAD
> `7913760` a `Vec<AgentMessage>::clone()` costs `O(image bytes + JSON payload bytes)` plus one
> small allocation per content block — **not** `O(total transcript bytes)`. Assistant messages,
> text, thinking and tool-call arguments are all already refcounted. See **[§8.2](#82-the-new-cost-model-what-a-vecagentmessageclone-actually-costs-at-head-7913760)**
> for the corrected table and **[§8.3](#83-the-task-is-still-worth-doing--the-argument-just-moved)**
> for why the task survives that correction.

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
| [`agent/run/stream.rs:41`](../../../crates/cyrup-agent/src/agent/run/stream.rs) | `base_messages` | the loop's own working copy for the request (pi `context.messages`, `agent-loop.ts:283`) |
| [`agent/run/tools/mod.rs:63`](../../../crates/cyrup-agent/src/agent/run/tools/mod.rs) | `ctx_messages` | per-call hook context view (pi `currentContext.messages`, `agent-loop.ts:691`) |
| [`agent/run/turn.rs:100`](../../../crates/cyrup-agent/src/agent/run/turn.rs) | `ctx_messages` | `prepare_next_turn` context |
| [`agent/run/turn.rs:161`](../../../crates/cyrup-agent/src/agent/run/turn.rs) | `ctx_messages_after` | `should_stop_after_turn` re-snapshot |

Plus the accumulator and state copies on the same path:

| site | what |
| --- | --- |
| [`agent/run/turn.rs:53,178,198`](../../../crates/cyrup-agent/src/agent/run/turn.rs) | `self.new_messages.clone()` into `AgentEvent::AgentEnd` (three sites) |
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
[PERF-001](../../done/2026-08-29-01-49/PERF-001_STREAM_SNAPSHOT_QUADRATIC.md).** ~~The structural cost is
`4 × sizeof(transcript)` of `memcpy` plus one allocation per string per copy, per turn.
For a 400 KB transcript that is ~1.6 MB and a few hundred microseconds; for a 10 MB
transcript (a long session with large tool outputs) it is ~40 MB and single-digit
milliseconds per turn.~~

> **[AUG-4] THE STRUCK-THROUGH SIZING IS OBSOLETE.** PERF-001 landed and text no longer copies,
> so "4 × sizeof(transcript)" massively overstates the remaining cost for a text-only session and
> *understates* how sharply it is now concentrated in two places. The corrected figure is
> `4 × (image bytes + JSON payload bytes)` per turn, plus ~4 small allocations per content block.
> Concretely: a text-only 10 MB transcript now costs **~0** byte-copying (only allocator churn),
> while a transcript holding three pasted screenshots costs up to **54 MB of `memcpy` per turn**
> — because `Content::Image.data` is an owned base64 `String` capped at 4.5 MB each. Full
> derivation and the evidence in **[§8.2](#82-the-new-cost-model-what-a-vecagentmessageclone-actually-costs-at-head-7913760)**.

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

1. **A turn's cost no longer scales with transcript size.** ~~Prompting in a session with a
   10 MB transcript costs the same per-turn snapshot work as one with a 100 KB transcript.~~
   **[AUG-4] Restated for the post-PERF-001 world**, because the old phrasing is now satisfied
   *by accident* for a text-only session and so cannot discriminate a real fix from no fix:
   **prompting in a session holding several megabytes of IMAGE content, `Custom`/`App` JSON
   payloads, or `ToolResult.details` costs the same per-turn snapshot work as one holding none.**
   Image content is the sharp end (§8.2) — construct the check there or it proves nothing.
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

### 6.6 Cost model, confirmed ~~— SUPERSEDED by §8.2~~

> **[AUG-4] SUPERSEDED.** This paragraph described the pre-PERF-001 types and is kept only so the
> delta is legible. `AssistantMessage.content` is still `Vec<Content>`, but `Content` no longer
> owns its text: `Text.text`/`Thinking.thinking` are `SharedStr` and `ToolCall.arguments` is
> `LazyArgs`, both O(1) to clone. And `AgentMessage::Assistant` is now `Arc<AssistantMessage>`, so
> the entire assistant arm clones in constant time. **Read [§8.2](#82-the-new-cost-model-what-a-vecagentmessageclone-actually-costs-at-head-7913760) instead.**

~~`AssistantMessage`~~ ([`cyrup-core/src/message/assistant.rs:30`](../../../crates/cyrup-core/src/message/assistant.rs))
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

---

## 7. [AUG-3] Re-verification pass @ HEAD `8f49433` — every cyrup citation re-checked, exact

*This pass re-ran the greps behind §1–§6 against the working tree at the **current** HEAD
`8f49433` (identical to `f3bf9f0` in `cyrup-agent`/`session-svc`/`ext`/`core` — `git diff
f3bf9f0..HEAD` on those four crates is empty). The point of the pass is to let `exec` trust the
line numbers as-is: they were confirmed against the exact commit it will edit, not an ancestor.*

### 7.1 All cyrup-side line numbers verified EXACT (no edits needed)

Every `file:line` in §1–§6 resolves to the cited construct at HEAD `8f49433`. Spot-list of the
load-bearing ones, each re-read this pass:

- **§1 per-turn clones** — `stream.rs:40` (`base_messages = self.messages.clone()`),
  `tools/mod.rs:63`, `turn.rs:52/99/160/177/197`, field decls `run/mod.rs:62,69`, accumulator
  `run/mod.rs:245`, `lifecycle.rs:180,270,375`, `state.rs:122`, `loop_fn.rs:120,217` — all exact.
- **§6.3 `agent_end` trio** — the `AgentEnd { messages: vec![fm.clone()] }` failure constructor is
  at `run/mod.rs:221` **as cited** (line 222 is the separate `self.new_messages = vec![fm]`; do
  not “correct” 221→222). Session seam `event.rs:154/328-329`, `subscriber.rs:68/72/196-198`,
  and `retry.rs:75` (`will_retry_after_agent_end(&self, messages: &[AgentMessage])`) + its
  `AgentMessage::Assistant(a)` arm at `:83` — all exact.
- **§6.2 hook seam** — `hooks.rs:24` (`AgentContextView.messages: &'a [AgentMessage]`), `:114/117`
  (`PostTurn`), `:215` (`convert_to_llm(&[AgentMessage])`), `:220` (`transform_context`) — exact.
- **§6.7/§6.8 ext seams** — `ext/event.rs:300` (`Context`), `:314` (`AgentEnd`), `:536` (clone);
  `ext/contract.rs:58` (`Context`), `:114` (`*messages = m`); `ext/hooks.rs:130/135/138`;
  `ext/host/live.rs:2077` (Context `to_string`), `:2146` (AgentEnd `to_string`) — exact.
- **§0/§3.5/§6.7.6 doc comment** — the `.slice()` SNAPSHOT comment is at `run/mod.rs:63-68`
  exactly, above the `messages:` field at `:69`, citing `agent.ts:424-429; agent-loop.ts:104-107`.

### 7.2 The two load-bearing preconditions were RE-CONFIRMED independently, not taken on faith

- **serde `rc` (§6.9).** `Cargo.toml:145` reads
  `serde        = { version = "1",   features = ["derive", "rc"] }` — confirmed by eye this pass
  (note the column-aligned whitespace: a naive `grep 'serde = '` with single spaces MISSES it; use
  `grep '"rc"' Cargo.toml`). All four path crates (`cyrup-core/agent/ext/session-svc`) take
  `serde` via `workspace = true`. Without `rc`, every Arc-carrying seam in this task fails to
  compile — so this is the first thing to sanity-check if a build errors on a missing
  `Serialize`/`Deserialize` for `Arc<AgentMessage>`.
- **§6.8 clone-BEFORE-gate ordering.** Re-read `ext/hooks.rs:135-136`: `let ev =
  HostEvent::Context { messages: msgs.clone() };` is on the line **before**
  `dispatch_block_mutate(ev, ...)`, and the `no_subscribers` gate lives *inside*
  `dispatch_block_mutate` at `dispatch.rs:415`. So the `transform_context` transcript copy is
  genuinely UNCONDITIONAL (fires with zero `context` subscribers), unlike the `agent_end` copy at
  `ext/subscriber.rs:65` (`HostEvent::from_agent`) which sits AFTER that file's own
  `no_subscribers` gate at `:62`. The §6.8 asymmetry is real — this is exactly why DoD #6 names the
  `HostEvent::Context` build specifically.
- **§6.1 `no Arc::make_mut` (DoD #4).** Re-confirmed: writes to the three transcripts are all
  `push` of a freshly built message; no in-place index-assignment on this path. Structurally safe.

### 7.3 pi oracle: version + behavioural claims confirmed; two immaterial 1-line drifts

- **pi is at `v0.84.2-48-g59a71b2` here** (tags `v0.84.0/1/2` all present). The frontmatter’s
  `pi v0.84.2` is correct and is MORE current than the workspace `CLAUDE.md`’s “v0.84.1” — trust
  the frontmatter, not `CLAUDE.md`, for this file.
- **Behaviour confirmed** at `agent-loop.ts`: `let currentContext = initialContext` (`:163`),
  by-reference reassign `currentContext = nextTurnSnapshot.context ?? currentContext` (`:234`),
  accumulator `newMessages.push(…)` (`:187/:194/:220`), `{ type:"agent_end", messages:
  newMessages }` (`:198/:255/:274`), and the run-request copy `messages: [...context.messages,
  ...prompts]` (`:106`, a spread — the literal `.slice()` is the run-start snapshot in
  `agent.ts:424-429`, which is what the cyrup comment cites). Parity claim intact.
- **Do NOT “fix” the two 1-line pi drifts.** §2 cites the two `currentContext.messages.push`
  sites as `:186` and `:219`; at this checkout they are `:187` and `:220`. This is normal upstream
  line drift on a read-only oracle and changes NOTHING about the cyrup work — left as-is
  deliberately so a future reader doesn’t burn a turn “correcting” a pi line that will drift again.

### 7.4 Bottom line for `exec` ~~— superseded by §8.8~~

The plan in §6.7 (steps 1–7) is ready to execute verbatim; §6.8 is mandatory (DoD #6/#2); §6.4/§6.5
draw the scope boundary (do **not** touch `StateInner::messages` / `AgentStateSnapshot` / the public
`RunHandle::finished` return type in Phase 1). ~~No citation in this file needs updating before
starting.~~ If the compiler points at a `file:line` that disagrees with this document, HEAD moved
after `8f49433` — re-grep that one site before editing; everything else in the plan still holds.

> **[AUG-4] HEAD did move — §7's own escape clause fired.** §7 was written against `8f49433`;
> HEAD is now `7913760`. Eight citations drifted and six seam sites were missing. §7.1's claim
> that every line number is "EXACT" is **no longer true** — see §8.4. The §6.7 plan itself still
> holds, amended by §8.8.

---

## 8. [AUG-4] Revision pass @ HEAD `7913760` — after PERF-001 landed

*This pass re-ran every grep behind §1–§7 against the working tree at HEAD `7913760`. Unlike §7,
which confirmed a static tree, this one found **substantive movement**: the sibling task PERF-001
merged and changed the very types this task reasons about. Nothing below is a style note — each
item either corrects a false statement, adds a site whose omission is a compile error, or removes
a deep copy the earlier passes did not see.*

### 8.1 What changed under this task

`git log --oneline -3` at HEAD:

```
7913760 Merge remote-tracking branch 'origin/main' into david/performance
b8a53db Merge pull request #104 from cyrup-ai/claude/perf-stream-snapshot-quadratic
04d6fa5 perf(provider,core,agent): make the streamed `partial` linear, not quadratic
```

`04d6fa5` is **PERF-001**, the task §4 names as "much larger than this one." It introduced three
new `cyrup-core` types and rewired the message types onto them:

| introduced | file | what it does |
| --- | --- | --- |
| [`SharedStr`](../../../crates/cyrup-core/src/shared_str.rs) | `cyrup-core/src/shared_str.rs` | append-only string behind `Arc<RwLock<String>>` + cached `OnceLock<Arc<str>>`; **`Clone` is O(1) in every state** (`:156-175`) |
| [`LazyArgs`](../../../crates/cyrup-core/src/lazy_args.rs) | `cyrup-core/src/lazy_args.rs` | tool args parsed on first read; **`Clone` is O(1) in every state** (`:63-72`) |
| `parse_streaming_json_object` | `cyrup-core/src/json.rs` | the salvage parser `LazyArgs` defers to |

And it changed four field/variant types that this task's cost model depended on:

| site | before | **after (HEAD `7913760`)** |
| --- | --- | --- |
| [`cyrup-agent/src/event.rs:36`](../../../crates/cyrup-agent/src/event.rs) `AgentMessage::Assistant` | `AssistantMessage` | **`Arc<AssistantMessage>`** |
| [`cyrup-core/.../content.rs:19`](../../../crates/cyrup-core/src/message/content.rs) `Content::Text.text` | `String` | **`SharedStr`** |
| [`cyrup-core/.../content.rs:26`](../../../crates/cyrup-core/src/message/content.rs) `Content::Thinking.thinking` | `String` | **`SharedStr`** |
| [`cyrup-core/.../tool_call.rs:27`](../../../crates/cyrup-core/src/message/tool_call.rs) `ToolCall.arguments` | `serde_json::Map<String,Value>` | **`LazyArgs`** |

The `AgentMessage::Assistant` doc comment PERF-001 added says the quiet part out loud, and it is
the same argument this task makes: *"Owning it here meant a deep copy at each of those points, on
every stream delta. The wire is unchanged: serde's `rc` feature serializes an `Arc<T>`
transparently as `T`."* **PERF-001 independently validated this task's technique** — including the
`rc` precondition §6.9 identified — and shipped it one layer down.

### 8.2 The NEW cost model: what a `Vec<AgentMessage>::clone()` actually costs at HEAD `7913760`

Per-variant, verified against the current type definitions:

| `AgentMessage` variant | clone cost NOW | why |
| --- | --- | --- |
| `Assistant(Arc<AssistantMessage>)` | **O(1)** ✅ | refcount bump — already fixed by PERF-001 |
| `User { content: Vec<Content>, .. }` | O(blocks) + **image bytes** | see the `Content` table below |
| `ToolResult(ToolResultMessage)` | O(blocks) + **image bytes + `details` JSON** | `details: Option<Value>` is a deep `serde_json` clone; `added_tool_names: Vec<String>`, `tool_name: String` are small deep copies |
| `Custom { payload: Value, details: Option<Value>, .. }` | **O(payload bytes)** | two deep `serde_json::Value` clones |
| `App { payload: serde_json::Map<String,Value>, .. }` | **O(payload bytes)** | deep `Map` clone ([`event.rs:71-75`](../../../crates/cyrup-agent/src/event.rs)) |

And per `Content` block:

| `Content` variant | clone cost NOW |
| --- | --- |
| `Text { text: SharedStr, text_signature: Option<String> }` | **O(1)** for the text; one small alloc iff a signature is present |
| `Thinking { thinking: SharedStr, .. }` | **O(1)** for the text |
| `ToolCall(ToolCall)` | args **O(1)** (`LazyArgs`); `id` + `name` + `thought_signature` are small deep copies |
| `Image { data: String, mime_type: String }` | **O(base64 bytes) — THE residual** |

**`Content::Image.data` is the sharp end and it has a hard, documented ceiling.**
[`cyrup-tools/src/tools/read.rs:521`](../../../crates/cyrup-tools/src/tools/read.rs) sets
`MAX_B64_BYTES` to **4.5 MB of base64** — "Pi's headroom below Anthropic's 5MB limit
(image-resize-core.ts:22)" — and `read.rs:588`/`:675` build `Content::Image { data: base64_encode(…) }`
right at that budget. So **one** image block costs up to 4.5 MB per `Vec` clone, and this task's
four per-turn snapshots make that **18 MB per turn per image**. Three pasted screenshots in a
session → **~54 MB of `memcpy` on every single turn, for the rest of that session.**

**Corrected headline for §0's table, middle column:**

| | pre-PERF-001 (what §0 describes) | **HEAD `7913760` (actual)** | after this task |
| --- | --- | --- | --- |
| cost | O(total transcript bytes) | **O(image bytes + JSON payload bytes) + O(blocks) allocs** | O(n) pointer bumps |
| a 10 MB text-only transcript | ~10 MB copied | **~0 bytes copied**, ~N small allocs | 0 |
| a transcript with 3 images | ~13.5 MB copied | **~13.5 MB copied** — unchanged | 0 |

### 8.3 The task is still worth doing — the argument just moved

A fair reading of §8.2 is "PERF-001 took the general case; what's left is a special case." That is
true, and it should be stated honestly rather than papered over. Three reasons it is still work,
in descending strength:

1. **The image case is not exotic and it is the worst-shaped cost curve in the file.** Pasting a
   screenshot into a coding session is routine, and the cost is paid on *every subsequent turn* of
   that session, not once. 4.5 MB × 4 snapshots × every turn is a real, sustained, user-visible
   regression against pi, which copies a pointer. Nothing else in the loop behaves that way now.
2. **`Custom` / `App` / `ToolResult.details` are `serde_json::Value` and will never get a
   `SharedStr`-style fix** — §6.6's own note explains why (`Value` is foreign and has no shared
   variant; that is exactly the reasoning [`lazy_args.rs:9-11`](../../../crates/cyrup-core/src/lazy_args.rs)
   gives for deferring rather than sharing). `Arc<AgentMessage>` is the *only* lever that makes
   those blocks cheap to snapshot.
3. **It removes the whole class rather than the current instance.** PERF-001 fixed the types that
   were expensive *in July*. `Arc<AgentMessage>` makes the snapshot O(n) regardless of what any
   future field holds — which is precisely the property the port comment at `run/mod.rs:63-68`
   already claims. This is the difference between matching pi's *behaviour* and matching its
   *complexity class*.

**What NOT to claim.** Do not justify this task with "the transcript is deep-copied four times per
turn" any more — for a text session that is now false, and a reviewer who checks will discount the
whole file. Justify it with the image/JSON residual and the complexity-class argument above.

### 8.4 Line-number corrections — eight citations drifted (verified at `7913760`)

§7.1 asserted every cyrup `file:line` was "EXACT." Six were off by one at the time it was written
(they sit in files PERF-001 never touched, so this was an error in §7, not drift), and two moved
because PERF-001 inserted lines above them. **Corrected table — trust this one:**

| cited as | **actual at `7913760`** | content | cause |
| --- | --- | --- | --- |
| `stream.rs:40` | **`stream.rs:41`** | `let base_messages = self.messages.clone();` | §7 error (`:38-40` is the comment block) |
| `turn.rs:52` | **`turn.rs:53`** | `AgentEnd { messages: self.new_messages.clone() }` (error/abort path) | §7 error |
| `turn.rs:99` | **`turn.rs:100`** | `let ctx_messages = self.messages.clone();` | §7 error |
| `turn.rs:160` | **`turn.rs:161`** | `let ctx_messages_after = self.messages.clone();` | §7 error |
| `turn.rs:177` | **`turn.rs:178`** | `AgentEnd { … }` (`should_stop_after_turn` path) | §7 error |
| `turn.rs:197` | **`turn.rs:198`** | `AgentEnd { … }` (normal exit) | §7 error |
| `agent/event.rs:271` | **`agent/event.rs:280`** | `AgentEvent::AgentEnd { messages: Vec<AgentMessage> }` decl | PERF-001 (+9: `use std::sync::Arc` + Assistant doc) |
| `session-svc/event.rs:154` | **`session-svc/event.rs:155`** | `AgentSessionEvent::AgentEnd.messages` decl | PERF-001 (+1: `use std::sync::Arc`) |
| `agent/hooks.rs:114/117` | **`agent/hooks.rs:117`** only | `PostTurn.messages` (`:114` is the `struct` line) | §7 imprecision |

**Re-verified EXACT and needing no change:** `tools/mod.rs:63` · `run/mod.rs:62,69,221,245` ·
`run/mod.rs:63-68` (the `.slice()` doc comment) · `state.rs:122` (and `:93`/`:140` for the Phase-2
fields) · `loop_fn.rs:120,217` · `lifecycle.rs:180,270,375` · `hooks.rs:24,215,220,222,224` ·
`ext/event.rs:300,314,536` · `ext/contract.rs:58,114` · `ext/hooks.rs:130,135,138` ·
`ext/host/live.rs:2077,2146` · `ext/subscriber.rs:62,65` · `ext/dispatch.rs:415` ·
`session-svc/subscriber.rs:68,72,196,197,198` · `session-svc/retry.rs:75` · `Cargo.toml:145`.

### 8.5 SIX seam sites the §6.2/§6.7 inventory MISSED — each is a compile error or a silent re-copy

§6.2 lists four seam members (`AgentContextView.messages`, `PostTurn.messages`, `convert_to_llm`,
`transform_context`). A full `grep -rn '\[AgentMessage\]\|Vec<AgentMessage>'` over the three
crates' non-test sources finds **six more that carry the per-turn snapshot** and must flip with it:

| # | site | declaration | why it must change |
| --- | --- | --- | --- |
| 1 | [`agent/hooks.rs:39`](../../../crates/cyrup-agent/src/hooks.rs) | `BeforeToolCall.messages: &'a [AgentMessage]` | fed from `self.new_messages`; **compile error** if left |
| 2 | [`agent/hooks.rs:135`](../../../crates/cyrup-agent/src/hooks.rs) | `TurnUpdate.context: Option<Vec<AgentMessage>>` | the `prepare_next_turn` **override**, assigned straight into `self.messages` at [`turn.rs:134`](../../../crates/cyrup-agent/src/agent/run/turn.rs) (`self.messages = ctx;`) — **hard compile error** if left |
| 3 | [`agent/hooks.rs:178`](../../../crates/cyrup-agent/src/hooks.rs) | `default_convert_to_llm(msgs: &[AgentMessage])` | the free fn behind the trait default at `:215` |
| 4 | [`run/tools/exec.rs:36`](../../../crates/cyrup-agent/src/agent/run/tools/exec.rs) | `ctx_messages: &[AgentMessage]` | carries the `tools/mod.rs:63` snapshot into `execute_parallel` |
| 5 | [`run/tools/exec.rs:260`](../../../crates/cyrup-agent/src/agent/run/tools/exec.rs) | `ctx_messages: &[AgentMessage]` | … and into `execute_sequential` |
| 6 | [`run/tools/preflight.rs:21`](../../../crates/cyrup-agent/src/agent/run/tools/preflight.rs) + [`run/tools/finalize.rs:20`](../../../crates/cyrup-agent/src/agent/run/tools/finalize.rs) | `ctx_messages: &[AgentMessage]` | … and into the per-call preflight/finalize halves |

**#2 is the dangerous one.** `TurnUpdate.context` is `pub` on a `pub struct`, so leaving it as
`Vec<AgentMessage>` while `RunCtx::messages` becomes `Vec<Arc<AgentMessage>>` breaks `turn.rs:134`
outright — and the path-of-least-resistance repair is
`self.messages = ctx.into_iter().map(Arc::new).collect()`, which is *acceptable* (a per-override
cost, not per-turn, and overrides are rare) **but only if chosen deliberately**. The prescribed
path is to flip the field to `Option<Vec<Arc<AgentMessage>>>` so the override is a move, not a
re-wrap; a hook that builds a fresh context is constructing messages anyway, and wrapping at
construction is the same "push sites wrap once" rule from §6.1.

**Also note the full `Vec<AgentMessage>` surface that must NOT change** (§6.4/§6.5 scope boundary,
now enumerated so "which ones?" is not a judgement call during exec): `state.rs:93` (`StateInner`),
`state.rs:140` (`AgentStateSnapshot`), `loop_fn.rs:44,170,176,190,208,211,243`, `queue.rs:51,59,73`,
`run/mod.rs:22,99,225,229,241`, `agent/builder.rs:21,76`, `agent/prompt.rs:13,57,58`,
`agent/lifecycle.rs:21,26,70,71,75`, `agent/facade.rs:119,154`, and every
`cyrup-session-svc/src/session/*` occurrence. These are queue / builder / public-API surfaces, not
the per-turn path.

### 8.6 NEW in-scope win PERF-001 left on the table: `turn.rs` deep-copies the assistant message THREE times per turn

PERF-001 wrapped the push sites mechanically, and the mechanical wrap is `Arc::new(asst.clone())` —
which allocates a fresh `Arc` **and deep-copies the message into it**, defeating the sharing at the
exact moment it is created. Verified at [`turn.rs:40-83`](../../../crates/cyrup-agent/src/agent/run/turn.rs):

```rust
40:  let asst = self.stream_assistant().await?;             // AssistantMessage (owned)
44:  self.messages.push(AgentMessage::Assistant(Arc::new(asst.clone())));       // deep copy #1
45:  self.new_messages.push(AgentMessage::Assistant(Arc::new(asst.clone())));   // deep copy #2
49:  message: AgentMessage::Assistant(Arc::new(asst)),      // (error path — a move, fine)
83:  message: AgentMessage::Assistant(Arc::new(asst.clone())),                  // deep copy #3
```

So the two transcripts and the `turn_end` event each hold a **separate** `AssistantMessage`, and
`Arc::ptr_eq` between `self.messages.last()` and `self.new_messages.last()` is `false`. Three deep
copies per turn, every turn.

**Required change** — build the `Arc` once, clone the handle:

```rust
// The turn's assistant message is shared by the two working transcripts and the `turn_end`
// event; build the handle ONCE so the three carry the same allocation (PERF-001 wrapped these
// mechanically as `Arc::new(asst.clone())`, which deep-copied into each).
let asst = Arc::new(self.stream_assistant().await?);
self.messages.push(AgentMessage::Assistant(Arc::clone(&asst)));
self.new_messages.push(AgentMessage::Assistant(Arc::clone(&asst)));

if matches!(asst.stop_reason, StopReason::Error | StopReason::Aborted) {   // Deref — unchanged
    self.emit(AgentEvent::TurnEnd {
        message: AgentMessage::Assistant(Arc::clone(&asst)),
        …
// :83
    message: AgentMessage::Assistant(Arc::clone(&asst)),
```

The read sites need nothing: `tool_calls(&asst)` (`:57`), `asst.stop_reason` (`:47`, `:64`),
`self.execute_tool_calls(&asst, &calls)` (`:67`) and `message: &asst` (`:105`, `:166`, whose field
is `PostTurn.message: &'a AssistantMessage` at [`hooks.rs:120`](../../../crates/cyrup-agent/src/hooks.rs))
are all coercion sites, so `&Arc<T>` → `&T` applies. If the compiler disagrees at either
struct-literal field, spell it `&*asst` — do **not** reach for `.clone()`.

This is independent of the `Vec<Arc<_>>` change, is ~6 lines, and removes 3 whole-message deep
copies per turn on its own. **Do it first** (§8.8 step 0) so the rest of the work builds on a
correct push site, and so the win is real even if Phase 1 is deferred.

### 8.7 Explicitly OUT of scope: `convert_to_llm` still rebuilds the whole transcript every turn

Record this so exec does not believe DoD #1 covers it, and so a later reader does not file it as a
regression this task introduced. Every turn, [`stream.rs:52`](../../../crates/cyrup-agent/src/agent/run/stream.rs)
calls `convert_to_llm(&transformed)`, which builds a fresh `Vec<Message>` over the entire
transcript. Both live implementations deep-copy each assistant message out of its `Arc`:

- [`agent/hooks.rs:184`](../../../crates/cyrup-agent/src/hooks.rs) — `AgentMessage::Assistant(a) => Some(Message::Assistant((**a).clone()))`
- [`session-svc/hooks.rs:44`](../../../crates/cyrup-session-svc/src/hooks.rs) — the same `(**a).clone()`

The cause is that `cyrup_core::Message::Assistant` still holds `AssistantMessage` **by value**
([`conversation.rs:26`](../../../crates/cyrup-core/src/message/conversation.rs)). Post-PERF-001
this is O(blocks) rather than O(bytes) for text — but it still deep-copies image `data` and every
`details`/`payload` `Value`, on the same hot path, once per turn.

**Scope decision: NOT in this task.** Fixing it means `Message::Assistant(Arc<AssistantMessage>)`
in `cyrup-core`, which ripples into `cyrup-provider`'s request builders and all 10 wire APIs — a
strictly larger blast radius than everything in §6.7 combined, and it moves a type on the provider
boundary rather than inside the loop. **Open it as PERF-007** (PERF-003–006 are taken —
`PARALLEL_FILE_WALK`, `SESSION_PERSIST_FSYNC`, `DECOUPLE_RENDER_FROM_FOLD`,
`PIPELINE_STREAM_DECODE`) with this paragraph as its evidence. Do not half-start it here.

### 8.8 The revised ordered plan for `exec`

§6.7 remains the spine. Amended and re-ordered:

0. **[NEW — do first] Fix the triple deep copy at `turn.rs:40-83`** per §8.6. Self-contained,
   ~6 lines, no signature changes, no cross-crate ripple. Verify with `cargo check -p cyrup-agent`.
1. **`cyrup-agent` fields + seam types.** `RunCtx::{messages,new_messages}` →
   `Vec<Arc<AgentMessage>>` (`run/mod.rs:62,69`). Flip the seam: `AgentContextView.messages`
   (`hooks.rs:24`), **`BeforeToolCall.messages` (`:39`)**, `PostTurn.messages` (`:117`),
   **`TurnUpdate.context` (`:135`)**, `default_convert_to_llm` (`:178`), `convert_to_llm` (`:215`),
   `transform_context` (`:220-224`) — the bolded members are §8.5's misses.
2. **The four `ctx_messages` parameters** in `run/tools/{exec.rs:36, exec.rs:260, preflight.rs:21,
   finalize.rs:20}` (§8.5 #4–6). Purely `&[AgentMessage]` → `&[Arc<AgentMessage>]` plus
   `m.as_ref()` in match arms.
3. **`AgentEvent::AgentEnd.messages` → `Vec<Arc<AgentMessage>>`** at **`agent/event.rs:280`**
   (corrected from `:271`); update the constructors at `turn.rs:53,178,198`, `run/mod.rs:221`,
   `lifecycle.rs:375`, and the `loop_fn.rs:217` extractor (deref-map, per §6.5).
4. **Hook impls follow:** `session-svc/hooks.rs:35,179,225`, `ext/hooks.rs:130-138`.
5. **Session seam:** `AgentSessionEvent::AgentEnd.messages` at **`session-svc/event.rs:155`**
   (corrected from `:154`); `subscriber.rs:196-198`; `retry.rs:75` + its `:83` match arm.
6. **Extension seams — BOTH:** `HostEvent::AgentEnd` (`ext/event.rs:314`, clone at `:536`) **and**
   `HostEvent::Context` (`ext/event.rs:300`) + `EventPatch::Context` (`ext/contract.rs:58`), with
   the build at `ext/hooks.rs:135` becoming a pointer clone. `contract.rs:114` and
   `live.rs:2077,2146` stay byte-identical. **§6.8 is mandatory, not optional** — re-read its
   "change the field, not the clone" warning before touching `ext/hooks.rs:135`.
7. **Update the `.slice()` doc comment** at `run/mod.rs:63-68`.
8. **Do NOT touch** `state.rs:93,140`, `RunHandle::finished`, or any surface in §8.5's
   "must NOT change" list.

### 8.9 Preconditions re-confirmed at `7913760`

- **serde `rc`** — [`Cargo.toml:145`](../../../Cargo.toml) still enables it (grep for `"rc"`, not
  `serde = ` — the line is column-aligned with multiple spaces). PERF-001 now *depends* on this
  too, via `AgentMessage::Assistant(Arc<AssistantMessage>)`, so the feature is far better anchored
  than when §6.9 flagged it.
- **§6.8 clone-before-gate ordering** — still true: [`ext/hooks.rs:135`](../../../crates/cyrup-ext/src/hooks.rs)
  builds `HostEvent::Context { messages: msgs.clone() }` on the line *before*
  `dispatch_block_mutate`, whose `no_subscribers` gate is at
  [`ext/dispatch.rs:415`](../../../crates/cyrup-ext/src/dispatch.rs). Unconditional, every turn.
- **§6.1 no `Arc::make_mut` (DoD #4)** — still structurally satisfiable: every write to the three
  transcripts is a `push` of a freshly built message; the only index-assignment on the path is
  `turn.rs:134`'s wholesale `self.messages = ctx`, which replaces the vector, not an element.
- **pi oracle** — unchanged at `v0.84.2-48-g59a71b235`; §7.3's behavioural confirmations and its
  "do not fix the two 1-line pi drifts" instruction both still stand.

### 8.10 Definition of Done — delta

DoD #1 is restated in §5 (image/JSON-bearing transcript, not "10 MB transcript"). Items #2–#6 are
unchanged and still correct. Add one:

7. **[AUG-4] The turn's assistant message is one allocation, not four.** After §8.6,
   `self.messages.last()` and `self.new_messages.last()` hold the *same* `Arc` — `Arc::ptr_eq` is
   `true` — and the `turn_end` event carries a third handle to it rather than a third copy. This
   is the criterion that catches the mechanical-wrap pattern PERF-001 left behind, which compiles,
   passes every other DoD item, and looks correct.
