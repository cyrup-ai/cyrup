---
stage: new
status: pending
updated: 2026-08-29 02:33
---

# Pipeline the stream decode across cores

> One task does bytes → SSE frame → JSON → fold → snapshot → send, serially. Splitting it
> into a bounded 3-stage pipeline makes decode cost `max(stage)` instead of `sum(stages)`.
> **pi structurally cannot do this** — one event-loop thread, no parallelism available to
> `Promise.all`.

---

## 0. READ THIS FIRST — this task is speculative and is deliberately ranked last

**Do not start this before [PERF-001](PERF-001_STREAM_SNAPSHOT_QUADRATIC.md) has landed and
been re-measured.** PERF-001 removes up to 2,883× of CPU from the *fold* stage. After it
lands, the pipeline's stages are:

| stage | what it does | cost after PERF-001 |
| --- | --- | --- |
| A | socket read + SSE framing | I/O bound; near-zero CPU |
| B | `serde_json` parse of one small frame | small, roughly constant |
| C | decoder fold + `snapshot` + `sink.send` | **the unknown** |

If C collapses to roughly B's size, `max(A,B,C) ≈ sum(A,B,C)` and this task buys close to
nothing while adding a thread boundary, two channels and an ordering obligation to the
hottest path in the program. **That is the likely outcome and it should be treated as the
default hypothesis.**

**The first deliverable of this task is therefore a measurement, not a pipeline.** Time each
stage separately on a recorded stream after PERF-001. If C is not the dominant term, close
this task as "not worth it" and record the numbers here. Closing it that way is a success,
not a failure.

---

## 1. Where it is

The decode loop is per-API but structurally identical everywhere. Anthropic, as the
reference:

- **Stage A** — [`stream/sse.rs:466`](../../../crates/cyrup-provider/src/stream/sse.rs):
  ```rust
  let es: EsInner = Box::pin(resp.bytes_stream().eventsource());
  ```
  reqwest's `bytes_stream` feeding `eventsource_stream`'s framer. Idle-timeout policy is set
  at [`sse.rs:125-131`](../../../crates/cyrup-provider/src/stream/sse.rs) via
  `ClientBuilder::read_timeout` — deliberately **not** a total-request `timeout`, which
  would kill a long generation (`sse.rs:28-35`). Do not disturb that.

- **Stages B and C** —
  [`api/anthropic_messages/driver.rs:49-…`](../../../crates/cyrup-provider/src/api/anthropic_messages/driver.rs):
  ```rust
  while let Some(frame) = frames.next().await {
      …
      if !is_message_event(&frame.event) { continue; }
      let data = frame.data.trim();
      // parse → dispatch into events.rs → dec mutation → snapshot → sink.send(...).await
  }
  ```
  Parse and fold are interleaved in one `await` chain, and `sink.send(...).await` is
  back-pressured all the way to the TUI (see
  [`subscriber.rs:63-72`](../../../crates/cyrup-session-svc/src/subscriber.rs)).

The same shape exists in the other six decoders: `bedrock_converse_stream/driver.rs`,
`openai_responses/`, `openai_completions/`, `google_generative_ai/driver.rs`,
`mistral_conversations/driver.rs`, `pi_messages.rs`.

---

## 2. What the pipeline would look like

Two bounded channels, three tasks, per stream:

```
reqwest bytes_stream ──[mpsc<SseFrame>, cap N]──> parse task ──[mpsc<(SseFrame, Value)>, cap N]──> fold task ──> sink
       (stage A)                                   (stage B)                                        (stage C)
```

Three properties that are **not negotiable**, because the current single-task design gets
each of them for free and a pipeline must re-establish them explicitly:

1. **Event order is exact.** `StreamEvent`s must reach the sink in the same order they would
   have today. Single-consumer bounded `mpsc` per hop preserves this; anything that fans out
   parse work across multiple workers does not, and must not be used.
2. **Back-pressure reaches the socket.** Today a slow sink stalls the `while let Some(frame)`
   loop, which stops reading the socket. With bounded channels this still holds — but the
   *bound* now sets how much unparsed data can accumulate in memory. Choose it deliberately
   and document it; `CHANNEL_CAPACITY = 1024` in
   [`session-svc/subscriber.rs:23`](../../../crates/cyrup-session-svc/src/subscriber.rs) is
   the existing precedent for that kind of constant.
3. **Cancellation is prompt.** A run abort must stop all three stages. Today dropping the
   stream is enough. With spawned tasks, each must observe the cancel token — the pattern
   [`cyrup-core/src/keyed_lock.rs`](../../../crates/cyrup-core/src/keyed_lock.rs) and the
   `select!` arms in `cyrup-ext/src/host/live.rs` already use.

Error propagation must also be preserved exactly: an `Err` frame today short-circuits to
`sink.send(e.into_error_event(...))` and **returns**
([`driver.rs:51-57`](../../../crates/cyrup-provider/src/api/anthropic_messages/driver.rs)),
and an `event: error` frame surfaces its data as an error (`:58-…`, pi
`anthropic-messages.ts:439-441`). Both must still terminate the whole pipeline, not just the
stage that saw them.

---

## 3. Cheaper things to try first, in this order

Each is smaller than the pipeline and may remove the reason for it.

**3a. Stop allocating per frame in stage B.** `frame.data` is a `String` that
`eventsource_stream` already allocated; the parse then builds a fully-owned
`serde_json::Value`. Borrowed deserialization (`#[serde(borrow)]` into a typed frame struct
over `&'a str`, rather than `Value`) removes one allocation per delta and the map lookups
that follow it (`delta.get("type")`, `.get("text")`, …). This is a contained change to one
decoder that can be measured on its own.

**3b. Read-ahead only.** Split *just* stage A onto its own task with a bounded channel, so
the decode task is never the thing waiting on `read()`. One channel, no ordering risk, and
it captures most of the overlap benefit if the socket is the latency source. Try this before
the full three-stage split.

**3c. Then, and only then, the full pipeline** — if measurement shows stage C is still the
dominant term after PERF-001.

---

## 4. Explicitly out of scope

- **simd-json / sonic-rs.** Per-frame JSON is small; see
  [INDEX](INDEX.md) — swapping the parser is a few percent on a term that is not dominant.
  Revisit only if §3a's measurement shows parse is the biggest stage, which would be
  surprising.
- **Parallelising across frames.** Breaks ordering (§2.1). The decoder is a state machine
  over an ordered event sequence; there is no correct way to fold two frames concurrently.
- **Touching the idle-timeout policy** at
  [`sse.rs:117-131`](../../../crates/cyrup-provider/src/stream/sse.rs). It is correct and
  the reasoning is documented at `sse.rs:28-35`.

---

## 5. Definition of Done

**Either** of these closes the task:

**(a) Closed as not worth it.** Per-stage timings taken on a recorded stream after PERF-001
are recorded in this file, showing `max(A,B,C)` is within ~20% of `sum(A,B,C)` — i.e. no
stage dominates and there is nothing to overlap. Frontmatter goes to `status: done` with the
numbers as the justification. **This is a legitimate and likely outcome.**

**(b) Shipped.** All of:

1. **Per-stage timings are recorded here**, before and after, on the same recorded stream.
2. **Decode throughput improves measurably** on a stream with a large tool call and a long
   prose body — at least 1.5× on tokens/sec of decode, or the change is not worth its risk.
3. **Event order is byte-identical.** For a recorded stream, the emitted `StreamEvent`
   sequence — types, ordering, and payloads — matches the pre-change sequence exactly.
   [`cyrup-test-support`'s `differential.rs`](../../../crates/cyrup-test-support/src/differential.rs)
   is the existing machinery for this (R-00-012) and should be what proves it.
4. **Back-pressure still reaches the socket.** A stalled consumer stops the socket read
   rather than buffering without bound; peak memory during a stalled stream is bounded by
   the declared channel capacities and that bound is documented next to the constants.
5. **Cancellation is prompt.** An aborted run tears down all stages promptly and leaves no
   detached tasks; a dropped stream does not leak a parse or fold task.
6. **Errors terminate the whole pipeline.** A transport `Err` and an `event: error` frame
   each still produce exactly one error event at the sink and end the stream, as they do at
   [`driver.rs:51-66`](../../../crates/cyrup-provider/src/api/anthropic_messages/driver.rs).
7. **The suite is green under the real gate:**
   `cargo test --workspace --features test-fixtures --no-fail-fast`, and
   `cargo clippy --workspace --all-targets --features test-fixtures` exits **0**.
