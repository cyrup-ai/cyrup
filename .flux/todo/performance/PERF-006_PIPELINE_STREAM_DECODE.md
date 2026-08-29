---
stage: aug
status: done
updated: 2026-08-29 04:32
aug_against: cyrup HEAD 8f49433 (branch david/performance) · reqwest 0.13.4 · hyper 1.10.1 · hyper-util 0.1.20 · eventsource-stream 0.2.3 · tokio 1.52.3 · serde_json 1.0.150
measured_on: this host, `--release`, 5 runs, medians reported. Probe sources kept at
  [`crates/cyrup/tmp/perf006/`](../../../crates/cyrup/tmp/perf006/src/main.rs) (gitignored).
---

# Pipeline the stream decode across cores

> One task does bytes → SSE frame → JSON → fold → snapshot → send, serially. Splitting it
> into a bounded 3-stage pipeline would make decode cost `max(stage)` instead of
> `sum(stages)`.
>
> **VERDICT: DoD (a) — close as not worth it. The numbers are now measured, not predicted,
> and they are in §3.** After [PERF-001](PERF-001_STREAM_SNAPSHOT_QUADRATIC.md) the entire
> serial decode budget is **≈3.4 µs per
> frame**, i.e. ~294,000 frames/sec on one core, against a wire that delivers 10²–10³
> frames/sec. Decode runs at **~0.3 % of one core**. A pipeline that made it 1.9× faster
> would take that to 0.18 %.
>
> **Two things in this file were wrong and are corrected here.** The probe §4 specified
> would have inflated stage A by **113×** (measured) because it feeds `decode_sse_bytes`,
> and the decision metric §5 pre-registered (`sum/max`) is the wrong quantity — a 3×
> stage-balance ratio on a stage that is 99.7 % idle is still worth nothing. §7.2 is the
> real work and is now measured at **2.57×** on stage A.

---

## 0. READ THIS FIRST — six things that will send you the wrong way

**(a) THE PROBE THIS FILE USED TO SPECIFY WAS INVALID, AND IT WOULD HAVE PRODUCED THE RIGHT
VERDICT FOR A FABRICATED REASON.** §4 previously fed the recorded stream through
[`decode_sse_bytes`](../../../crates/cyrup-provider/src/stream/sse.rs) (`sse.rs:514`), which
wraps the **entire** buffer in one `futures::stream::once`:

```rust
pub fn decode_sse_bytes(bytes: impl Into<Bytes>) -> FrameStream {
    let bytes = bytes.into();
    let byte_stream = futures::stream::once(async move { Ok::<Bytes, std::io::Error>(bytes) });
    let es = byte_stream.eventsource();
```

`eventsource-stream`'s `parse_event` does `buffer.split_off(consumed)` **per line**
(`event_stream.rs:221-223`), which allocates and memcpy's the whole *remaining* buffer each
time. With one giant buffer that is O(n²) in the stream's byte length. Measured on a
1,544 KB / 9,540-frame stream:

| input shape | stage A total | per frame |
| --- | ---: | ---: |
| one `Bytes` (`decode_sse_bytes`) | **1.409 s** | **148.7 µs** |
| 8 KB chunks | 11.9 ms | 1.31 µs |
| one SSE event per chunk (what providers send) | 9.9 ms | **1.035 µs** |

**113×**, reproduced across five runs (108.9 / 110.2 / 112.8 / 113.2 / 115.1). A probe on the
replay path reports stage A as ~99.5 % of everything and yields `sum/max ≈ 1.005` — which
reads as "no stage dominates, close the task" and is pure artifact.

This contaminates **every existing in-crate helper**: `collect()`
([`tests/mod.rs:87-102`](../../../crates/cyrup-provider/src/api/anthropic_messages/tests)) and
`events()` ([`truncation_parity.rs:196-231`](../../../crates/cyrup-provider/src/api/truncation_parity.rs))
both call `decode_sse_bytes`. They are fine as **correctness** oracles and must not be
changed. They are unusable as **timing** oracles. Any future decode measurement must feed
`futures::stream::iter` over realistic chunks — see §4.

It is not a production bug: the shipping path is `resp.bytes_stream().eventsource()`
(`sse.rs:466`), and hyper delivers per-HTTP-chunk, so the buffer never grows past one chunk.
It *is* a live cost in the test suite, and it is why replay-based timing lies.

**(b) Stage A is ALREADY on another task, and the task file was originally wrong to imply
otherwise.** `resp.bytes_stream()` does **not** perform a `read(2)` on the decode task.
reqwest 0.13.4 sits on hyper-util 0.1.20's legacy client, which **spawns a background
dispatcher task per connection** — `executor.execute(...)` at `client.rs:552` (HTTP/2, logged
*"http2 handshake complete, spawning background dispatcher task"*) and `client.rs:584`
(HTTP/1, same message). The body stream the decoder polls is fed by that task over a channel.
**§6's "read-ahead" hop is therefore already implemented by the dependency graph**, and
building it again would add a buffer between hyper's channel and the framer, not between the
socket and the CPU.

**(c) …but stage A is not "near-zero CPU" either, and it is the LARGEST of the three.** The
old §3 table asserted `A ≈ B ≈ 10²–10³ ns`. Measured: **A = 1,035 ns, B = 591 ns —
A is 1.78× B**, and after PERF-001 it is the biggest single stage. What runs on the decode
task is SSE *framing*, and `eventsource-stream` 0.2.3 pays for it twice per chunk:

- [`utf8_stream.rs:59-62`] — `buffer.extend_from_slice(bytes)`, then `core::mem::take` +
  `String::from_utf8`: a full UTF-8 **validating pass plus a move, per chunk**. (The
  `split_off(valid_size)` at `:67` is the *rare* split-codepoint path, not the common one —
  the previous revision of this file cited it as if it were.)
- [`event_stream.rs:221-223`] — per **line**:
  ```rust
  let consumed = buffer.len() - rem.len();
  let rem = buffer.split_off(consumed);   // allocates a NEW String, memcpy's the remainder
  *buffer = rem;
  ```
  Note `parse_event` returns as soon as **one** event dispatches (`:279`), so a poll scans
  only the lines up to the next blank line — which is what bounds the damage on the wire path
  and why (a)'s single-buffer regime is so much worse.

  Vendored copies for reference:
  `~/.cargo/registry/src/index.crates.io-*/eventsource-stream-0.2.3/src/`.

**(d) The unbuildable DoD item.** An earlier revision said `cyrup-test-support`'s
`differential.rs` "should be what proves it". **`cyrup-provider` does not depend on
`cyrup-test-support`, and must not.** Verified at HEAD: `cyrup-provider`'s entire
`[dev-dependencies]` is `tokio` (with `net`) + `tokio-stream`
([`Cargo.toml`](../../../crates/cyrup-provider/Cargo.toml)). The edge runs the other way —
`cyrup-test-support` depends on `cyrup-provider` **with `features = ["faux"]` on a NORMAL
edge**, and the 39-line PROV-052 comment block at
[`cyrup-provider/Cargo.toml:14-53`](../../../crates/cyrup-provider/Cargo.toml) exists
specifically to reason about keeping `faux` off the shipped graph. Adding the reverse
dev-edge would unify `faux` into `cyrup-provider`'s own test build. Use the in-crate oracle
(§6.3).

**(e) `pi structurally cannot do this` is true but irrelevant.** cyrup does have real threads
where pi has one event loop —
[`crates/cyrup/src/main.rs:48`](../../../crates/cyrup/src/main.rs) is
`#[tokio::main(flavor = "multi_thread")]`, so spawned stages genuinely land on different
cores. It is irrelevant because *having* parallelism available does not mean there is
anything worth overlapping. See §3.

**(f) Every citation was re-verified against `8f49433`.** Corrections to the numbers as
previously filed:

| claim | filed as | actually |
| --- | --- | --- |
| idle-timeout rationale para | `sse.rs:28-37`, "wrong" sentence at `:36-37` | `sse.rs:27-36`, sentence at **`:35-36`** |
| `events()` helper | `truncation_parity.rs:196-215` / `:196-222` | `:196-231` |
| bedrock loop | `driver.rs:238-272` | `:240-247` (select), `:266-272` (decoder drain) |
| `Fanout::emit` | `subscriber.rs:63-72` | `:63-76`, sends at **`:68`** and **`:72`** |
| `decode_stream` call sites | "seven decoders" | **25 call sites across 17 files** |
| `channel()` capacity in production | probe used `1024` | **`STREAM_BUFFER = 64`** ([`collection.rs:28`](../../../crates/cyrup-provider/src/collection.rs), [`wire.rs:28`](../../../crates/cyrup-provider/src/wire.rs)) |
| `finish_blocks` as a snapshot site | implied | **no-op for Anthropic** ([`events.rs:257-261`](../../../crates/cyrup-provider/src/api/anthropic_messages/events.rs)) — not a site |

Accurate as filed: `sse.rs:466`, `sse.rs:295` (`FrameStream`), `sse.rs:169-175` (`SseFrame`),
`sse.rs:473`/`:477-478` (unfold + `biased`), `sse.rs:125-133`/`:130` (`with_idle_timeout` /
`read_timeout`), `driver.rs:49`, `subscriber.rs:23` (`CHANNEL_CAPACITY = 1024`),
`stream.rs:498` (`StreamEvent: PartialEq`), `golden.rs:16-34`, `json_parse.rs:152-161`, and
the six-SSE-decoder list.

---

## 1. Where it is — verified

### 1.1 The three stages, as the code actually splits them

| stage | what runs **on the decode task** | where | measured, per frame |
| --- | --- | --- | ---: |
| **A** | SSE framing only: UTF-8 validation + line split + per-line `String::split_off`. **Not** the socket read (0b). | [`sse.rs:466`](../../../crates/cyrup-provider/src/stream/sse.rs) `.eventsource()`, then the `stream::unfold` at `:473-507` | **1,035 ns** |
| **B** | `parse_json_with_repair(data)` → owned `serde_json::Value` | [`driver.rs:75`](../../../crates/cyrup-provider/src/api/anthropic_messages/driver.rs) → [`json_parse.rs:152-161`](../../../crates/cyrup-provider/src/utils/json_parse.rs) | **591 ns** |
| **C** | `process_event` → `Decoder` mutation → `snapshot()` → `sink.send().await` | [`driver.rs:86`](../../../crates/cyrup-provider/src/api/anthropic_messages/driver.rs) → [`events.rs`](../../../crates/cyrup-provider/src/api/anthropic_messages/events.rs) | 1.53 **ms** today; ≈534 ns after PERF-001 |

`parse_json_with_repair` is `serde_json::from_str::<Value>` on the happy path
(`json_parse.rs:153-154`), so B's 591 ns is exactly that call — the repair fallback
(`:156-159`) never runs on well-formed frames.

The `unfold` at `sse.rs:473-507` is worth reading before touching anything: it already
carries the `CancelToken` in a `tokio::select! { biased; _ = state.cancel.cancelled() => … }`
(`:477-478`) and yields exactly one `ProviderError::Aborted` before ending. **Cancellation
for stage A is solved and must not be re-solved** — a pipeline just has to not lose it.

### 1.2 Stage C's snapshot call sites

Confirmed in [`events.rs`](../../../crates/cyrup-provider/src/api/anthropic_messages/events.rs)
at `:147` (`text_delta`), `:160` (`thinking_delta`), `:176` (`input_json_delta`), `:207`
(`send_with_pos`, all four `*_start`), `:251` (`process_block_stop`), `:266` (`emit_error`),
plus [`driver.rs:42`](../../../crates/cyrup-provider/src/api/anthropic_messages/driver.rs)
(`Start`) and `:124` (`end_of_stream`). Eight sites. `finish_blocks` (`driver.rs:118`) is
**not** a ninth — it is an explicit no-op for Anthropic (`events.rs:257-261`), present only
for symmetry with the openai-completions decoder.

Each lands in `blocks_to_content`
([`blocks.rs:94-125`](../../../crates/cyrup-provider/src/api/anthropic_messages/blocks.rs)),
which re-parses the whole accumulated tool buffer (`:121`,
`parse_streaming_json_object(Some(partial_json))`). **That is PERF-001's target and it is the
entirety of why stage C is currently dominant.**

### 1.3 The other decoders — six SSE, one binary, 25 call sites

`while let Some(frame) = frames.next().await` appears in six places, all structurally
identical to Anthropic's:

| decoder | loop |
| --- | --- |
| [`anthropic_messages/driver.rs`](../../../crates/cyrup-provider/src/api/anthropic_messages/driver.rs) | `:49` |
| [`openai_responses/decoder.rs`](../../../crates/cyrup-provider/src/api/openai_responses/decoder.rs) | `:104` |
| [`openai_completions/decode.rs`](../../../crates/cyrup-provider/src/api/openai_completions/decode.rs) | `:42` |
| [`google_generative_ai/driver.rs`](../../../crates/cyrup-provider/src/api/google_generative_ai/driver.rs) | `:35` |
| [`mistral_conversations/driver.rs`](../../../crates/cyrup-provider/src/api/mistral_conversations/driver.rs) | `:36` |
| [`pi_messages.rs`](../../../crates/cyrup-provider/src/api/pi_messages.rs) | `:659` |

**But the blast radius is larger than six.** Four more `ApiImpl`s *reuse* another decoder's
`decode_stream` rather than defining their own — `azure_openai_responses.rs:170`,
`google_vertex.rs:230`, `openai_codex_responses/driver.rs:279` (which passes a **mapped**
stream, not raw frames), and `openai_completions/driver.rs:108`. Counting definitions plus
production plus test callers: **25 `decode_stream(` call sites across 17 files.**

**`bedrock_converse_stream` is NOT one of them** and an earlier revision was wrong to list it
as the same shape. It speaks AWS `vnd.amazon.eventstream` binary framing, not SSE: it polls
`response.bytes_stream()` directly under its own `tokio::select!` cancel arm (`:243-247`,
`biased` at `:244`) and drives a synchronous `EventStreamDecoder` over accumulated bytes
(`:240`, `:266-272`)
([`bedrock_converse_stream/driver.rs`](../../../crates/cyrup-provider/src/api/bedrock_converse_stream/driver.rs)).
Its stage A is CPU on the decode task with no `Stream` boundary to cut at.

---

## 2. The `'static` obstacle — real, but SMALLER than previously claimed

An earlier revision called this "the single largest cost of building the pipeline" and said
**"this changes the `decode_stream` signature in all seven decoders."** That is refuted by
code already in the tree.

`tokio::spawn` requires `Future: Send + 'static`, and
[`anthropic_messages/mod.rs:131`](../../../crates/cyrup-provider/src/api/anthropic_messages/mod.rs)
passes borrows:

```rust
decode_stream(frames, model, &self.api, &sink, is_oauth, &ctx.tools).await;
```

But **you do not need a signature change to spawn** — you need owned *locals inside the
spawned block*, then re-borrow them there. Both existing helpers already do exactly this:

```rust
// crates/cyrup-provider/src/api/anthropic_messages/tests/mod.rs:87-102
let m2 = m.clone();
let api2 = api.clone();
let task = tokio::spawn(async move {
    decode_stream(frames, &m2, &api2, &sink, false, &[]).await;   // borrows the MOVED locals
});
```

```rust
// crates/cyrup-provider/src/api/truncation_parity.rs:200-223 — same pattern, five decoders
let frames = decode_sse_bytes(raw.as_bytes().to_vec());
let task = tokio::spawn(async move { … decode_stream(frames, &model, &api, &sink, …) … });
```

So the actual obligation is: **the caller must own its arguments before spawning**, once per
stream, not per frame.

- `Model` is `Clone` ([`model.rs:52`](../../../crates/cyrup-provider/src/model.rs)) —
  carries `ModelCost` with an `Option<Vec<ModelCostTier>>` and several `String`s. Clone it
  once; never per frame.
- `ApiId` — `Clone`, cheap.
- `ToolDef` is `Clone` ([`context.rs:20`](../../../crates/cyrup-provider/src/context.rs)),
  and `tool_names` is already built per call as
  `tools.iter().map(|t| t.name.clone()).collect()`
  ([`driver.rs:35-38`](../../../crates/cyrup-provider/src/api/anthropic_messages/driver.rs)).
- `EventSink` is already `Clone` ([`api/mod.rs:66-69`](../../../crates/cyrup-provider/src/api/mod.rs)).

`FrameStream` is `Pin<Box<dyn Stream<Item = Result<SseFrame, ProviderError>> + Send>>`
([`sse.rs:295`](../../../crates/cyrup-provider/src/stream/sse.rs),
[`provider_plumbing.rs:92`](../../../crates/cyrup-provider/src/utils/provider_plumbing.rs)) —
the `+ Send` is there, so moving the stream into a spawned task compiles.

Note `ApiImpl::run` is `#[async_trait]`
([`api/mod.rs:93-106`](../../../crates/cyrup-provider/src/api/mod.rs)), so its future is
already `Pin<Box<dyn Future + Send + 'async_trait>>` and is *not* `'static`. That is why the
clone has to happen inside `run`/`decode_stream` and cannot be hoisted to the trait.

**Budget: a per-stream `Model` clone, not a 25-site signature refactor.** The refactor is only
needed if you want to eliminate that one clone, which is not worth 25 call sites.

---

## 3. The measured arithmetic — this is the answer

**All numbers below are medians of 5 `--release` runs on this host at `8f49433`.** The probe
that produced them is kept at
[`crates/cyrup/tmp/perf006/src/main.rs`](../../../crates/cyrup/tmp/perf006/src/main.rs)
(framing + parse) and
[`src/bin/hop.rs`](../../../crates/cyrup/tmp/perf006/src/bin/hop.rs) (the channel term).
Fixture: 128 KB prose + 256 KB tool call = 1,544 KB raw, **9,540 frames** — the shape DoD
(b).2 names.

### 3.1 Per-frame cost of each stage

```
                                                   per frame     range (5 runs)
  A   framing, eventsource-stream, 1 event/chunk     1.035 µs     1.022 – 1.064 µs
  A   framing, eventsource-stream, 8 KB chunks       1.310 µs
  A   framing, eventsource-stream, single buffer   148.700 µs     <- decode_sse_bytes ONLY
  A'  framing, in-tree advance-based framer (§7.2)    408 ns        401 –   523 ns
  B   serde_json -> owned Value                       591 ns        572 –   611 ns
  B'  borrowed typed deserialize (§7.1)               512 ns        501 –   515 ns
  C   fold + snapshot, TODAY                         1.53 ms      (PERF-001 §2)
  C   fold + snapshot, after PERF-001                 534 ns      (3.5 ms / 6554 deltas)
  S   sink.send().await, empty payload                211 ns        199 –   212 ns
  S   sink.send().await, alloc + send 200 B           794 ns        719 –   861 ns
  S   sink.send().await, alloc + send 4 KB           1.200 µs     1.137 – 1.262 µs
```

`S` is measured against a **freely-draining** consumer — the floor, not the real value. See
§3.3.

### 3.2 The verdict: decode is idle, not serial-bound

Per frame, wire path, after PERF-001, free-draining consumer, 4 KB partial:

```
  serial today   =  A 1035 + B 591 + C 534 + S 1200  =  3360 ns/frame
                 =  297,600 frames/sec on ONE core
```

An Anthropic stream delivers on the order of **10²–10³ frames/sec** (one
`content_block_delta` per token-ish; PERF-001's 6,554-delta fixture over a ~10 s tool call is
~650/s). At 10³ frames/sec the decoder consumes **3.4 ms of CPU per wall-clock second —
0.34 % of one core.** At an aggressive 10⁴ frames/sec it is 3.4 %.

**That is the whole answer.** `max(stage)` vs `sum(stages)` is a question about a resource
that is 99.7 % idle. A perfect pipeline:

```
  pipelined      =  max(A+hop, B+hop, C+S)
                 =  max(1035+211, 591+211, 534+1200)
                 =  1734 ns/frame   ->  1.94x   ->  0.17 % of one core
```

1.94× *does* clear the DoD's 1.5× bar. It buys 1.6 ms of CPU per second, on a core that had
996 ms spare, at the cost of two channels, an ordering obligation across 25 call sites, three
cancellation observations and a forward-travelling error protocol (§6.2.4).

### 3.3 …and the 1.94× evaporates the moment the consumer is real

`sink.send` is **in series with all three stages and cannot be pipelined**. It is bounded
([`api/mod.rs:85-88`](../../../crates/cyrup-provider/src/api/mod.rs), production capacity
`STREAM_BUFFER = 64`) and back-pressured through `Fanout::emit`, which awaits **every**
subscriber ([`subscriber.rs:63-76`](../../../crates/cyrup-session-svc/src/subscriber.rs) —
*"backpressure → slows the agent, never drops"*) all the way to the TUI draw.

Measured, `hop.rs`, consumer given a synthetic per-event stall:

```
  consumer stalls  0 µs/event  ->  producer sees   0.215 µs/event
  consumer stalls  1 µs/event  ->  producer sees   1.98  µs/event
  consumer stalls  5 µs/event  ->  producer sees   7.64  µs/event
  consumer stalls 20 µs/event  ->  producer sees  22.75  µs/event
```

The consumer's cost passes through to the producer essentially 1:1, plus ~1 µs of scheduling.
Recomputing §3.2 with a realistic consumer:

| consumer cost/event | serial | pipelined | speedup |
| ---: | ---: | ---: | ---: |
| 0 µs (drains freely) | 3.36 µs | 1.73 µs | **1.94×** |
| 1 µs | 4.14 µs | 2.51 µs | 1.65× |
| 5 µs | 9.80 µs | 8.17 µs | **1.20×** |
| 20 µs | 24.9 µs | 23.3 µs | **1.07×** |

A TUI frame draw is far more than 5 µs. **PERF-005 exists precisely because the renderer's
cost is on this critical path**, so the 5–20 µs rows are the operating regime, and the
pipeline is worth 7–20 % there — below the DoD bar, and against a stage that is already idle.

### 3.4 Two capacity traps the numbers expose

- **`cap = 1` is catastrophic**: 17.3 µs/item versus 794 ns at `cap = 1024` — a 22× penalty
  from lock-step wakeups. Any pipeline hop must be generously sized. `cap = 16` already costs
  993 ns (1.25× the `cap = 1024` figure), so the useful floor is O(100), not O(10).
- **The payload dominates the hop**: 211 ns empty → 794 ns at 200 B → 1.20 µs at 4 KB. The
  channel primitive is nearly free; **what costs is allocating the item.** See §8.

---

## 4. If you re-measure: how to do it validly

You should not need to — §3 has the numbers. If the upstreams or the code move and you do,
these are the constraints, and (a) is the one that matters.

### 4.1 Never feed `decode_sse_bytes` to a timing probe

Per §0a it is 113× off. Feed `futures::stream::iter` over realistic chunks:

```rust
use eventsource_stream::Eventsource;   // already a direct dep of cyrup-provider

/// One SSE event per chunk — what providers actually send over HTTP/2 DATA frames.
fn per_event_chunks(raw: &str) -> Vec<bytes::Bytes> {
    raw.split_inclusive("\n\n")
        .map(|s| bytes::Bytes::from(s.as_bytes().to_vec()))
        .collect()
}

let st = futures::stream::iter(per_event_chunks(&raw).into_iter().map(Ok::<_, std::io::Error>));
let mut frames = st.eventsource();
```

Cross-check both regimes (8 KB chunks **and** one-event-per-chunk); they agree to within 27 %
(1.31 µs vs 1.035 µs) and both are linear in stream size, which is the signal that you are
out of the quadratic.

### 4.2 Where a probe would live, if one is needed

`decode_stream` is `pub(crate)`
([`anthropic_messages/mod.rs:32`](../../../crates/cyrup-provider/src/api/anthropic_messages/mod.rs)),
so an external `crates/cyrup-provider/tests/*.rs` **cannot reach it**. It must be an in-crate
`#[cfg(test)]` module, and it must be **deleted once the numbers are recorded** — a measuring
stick, not a test. Standing rule from
[`SKILL.md`](../../../crates/cyrup-flux/resources/skills/flux/SKILL.md): benchmarks are not
task deliverables.

```bash
cargo test -p cyrup-provider --release \
  anthropic_messages::tests::stage_probe -- --nocapture --test-threads=1
```

Two properties that must not be "simplified" away:

- **`#[tokio::test(flavor = "multi_thread")]`.** The default is `current_thread`, on which a
  spawned decode task and the receiving loop interleave on one core — structurally unable to
  show overlap, biasing the answer toward "no benefit". Do not let the harness pre-decide.
- **Derive C by subtraction (`total − A − B`), not by a third direct measurement.** Timing
  `process_event` + `snapshot` + `send` in isolation needs decoder state reconstructed
  outside the driver — a hand-built replica that drifts from the code it claims to measure.
  Subtraction reuses the shipping loop verbatim, and it slightly *over*-attributes to C
  (the loop's own await overhead lands there), which biases toward "pipeline looks
  worthwhile" — so a "not worth it" verdict taken from it is conservative.

### 4.3 Use `STREAM_BUFFER`, not 1024

Production is `channel(STREAM_BUFFER)` with `STREAM_BUFFER = 64`
([`collection.rs:28`](../../../crates/cyrup-provider/src/collection.rs),
[`wire.rs:28`](../../../crates/cyrup-provider/src/wire.rs)). A probe at 1024 measures a
channel the product does not use, and per §3.4 the capacity is worth ~25 %.

---

## 5. Decision — resolved

The pre-registered rule in the previous revision selected on `sum/max`. **That was the wrong
metric and it is retired here**: a 3× stage-balance ratio on a stage running at 0.3 %
utilisation is still worth nothing. The correct question is *absolute headroom against the
wire rate*, and §3.2 answers it.

```
cyrup 8f49433 · 2026-08-29 · --release · 5 runs · 128 KB prose + 256 KB tool = 9,540 frames

     A framing (1 event/chunk)   1.035 µs/frame
     B serde_json -> Value         591 ns/frame
     C fold+snapshot (post-001)    534 ns/frame   [derived from PERF-001 §2: 3.5 ms / 6554]
     S sink.send (free consumer) 1.200 µs/frame

     serial      3.36 µs/frame  =  297,600 frames/sec  =  0.34 % of one core @ 10³ frames/s
     pipelined   1.73 µs/frame  =  1.94x, free consumer
                                =  1.20x at a 5 µs consumer
                                =  1.07x at a 20 µs consumer
```

| observed | verdict |
| --- | --- |
| decode < 5 % of one core at realistic frame rates | **Close as DoD (a).** ✅ **This is the case: 0.34 %.** Do §7.2, skip §7.1, and read §8. |
| decode saturates a core **and** stage C dominates | [PERF-001](PERF-001_STREAM_SNAPSHOT_QUADRATIC.md) did not land, or missed a decoder. Go back to it. |
| decode saturates a core **and** stage A dominates | §7.2 (in-tree framer). Measured 2.57× on A. |
| decode saturates a core **and** stage B dominates | §7.1 (borrowed deserialization). Measured 1.16× — probably still not worth it. |
| decode saturates a core and no stage is fixable in place | **Only then** build §6. |

---

## 6. If a future measurement ever selects it: the required implementation

Retained because the constraints are real and hard-won. **Do not build this on today's
numbers.** Every property below is one the current single-task loop gets **for free**, and a
pipeline must re-establish each one explicitly.

### 6.1 Shape

```
hyper conn task ──> bytes_stream ──> [framer task] ──mpsc<SseFrame>──> [parse task]
                                                    ──mpsc<Result<(SseFrame,Value),Err>>──> [fold task] ──> sink
       (already spawned, §0b)          stage A          cap >= 100          stage B         cap >= 100      stage C
```

Single-producer/single-consumer bounded `tokio::sync::mpsc` per hop. **Nothing fans out.**
Capacity per §3.4: O(100) minimum; `cap = 1` costs 22×, `cap = 16` costs 1.25×.

### 6.2 The four non-negotiables

1. **Exact event order.** One consumer per hop preserves it. Any design that spreads parse
   work over multiple workers does not, and is forbidden (§9).

2. **Back-pressure still reaches the socket.** A bounded `mpsc` propagates the stall backward
   through all three hops to `bytes_stream`, and from there to hyper's connection task, which
   stops reading. The *bound* becomes the declared memory ceiling for unparsed data and must
   be documented next to the constant, in the style of
   [`subscriber.rs:23`](../../../crates/cyrup-session-svc/src/subscriber.rs)
   (`const CHANNEL_CAPACITY: usize = 1024;`). Note the ceiling is
   `cap × sizeof(SseFrame + Value)`, and a `Value` for a large `content_block_start` is not
   small — pick the number from that product, not by copying 1024 or 64.

3. **Prompt cancellation.** Stage A's `CancelToken` arm already exists at
   [`sse.rs:473-507`](../../../crates/cyrup-provider/src/stream/sse.rs) (`tokio::select!` with
   `biased;` at `:477-478`) and must be preserved as-is. Stages B and C are spawned tasks and
   each needs its own observation of the token — the same `select!` pattern used throughout
   [`cyrup-ext/src/host/live.rs`](../../../crates/cyrup-ext/src/host/live.rs) (`:1446`,
   `:1506`, `:1526`, `:1559`, `:1579`, `:2034`), whose doc comment at `:1277-1280` states the
   governing rule: state *"must be torn down on EVERY exit path"*, including the one where the
   future is simply dropped at an await point. Dropping the sender is the natural teardown
   signal; **verify it, do not assume it** — DoD (b).5 is a test, not a claim.
   [`cyrup-core/src/keyed_lock.rs`](../../../crates/cyrup-core/src/keyed_lock.rs) is the other
   in-tree reference for cancel-aware waiting.

4. **Errors terminate the *whole* pipeline, not one stage.** Five terminal paths, all of which
   today `return` out of the single loop and must keep producing **exactly one** error event
   at the sink:

   | path | site | needs |
   | --- | --- | --- |
   | transport `Err` | [`driver.rs:52-56`](../../../crates/cyrup-provider/src/api/anthropic_messages/driver.rs) | `provider`, `model_id` |
   | `event: error` frame | `driver.rs:58-67` (pi `anthropic-messages.ts:439-441`) | **`dec`** |
   | unparseable frame | `driver.rs:75-85` | **`dec`** |
   | `stop_reason == Error` | `driver.rs:89-101` | **`dec`** |
   | `message_start` without `message_stop` | `driver.rs:104-116` (pi `:463-465`) | **`dec`** |

   Four of the five need `dec` — decoder state that lives in **stage C** — while the transport
   error is detected in **stage A**. So an error must travel forward down the pipeline as a
   *value* and be emitted by stage C, not emitted at the point of detection. Sending it
   sideways to the sink from stage A would race the frames still in flight and can emit the
   error *before* events that logically precede it. **This is the subtlest correctness
   requirement in the whole task.** The channel item type must therefore be
   `Result<(SseFrame, Value), ProviderError>`, with `Err` short-circuiting the fold and
   closing the pipeline.

   Note also `driver.rs:197`'s `_ => true`: an unrecognised event `type` is a **silent
   no-op**, never an error. Whatever forwards frames must preserve that.

### 6.3 The ordering oracle

Use the in-crate helper, per §0d:

1. Before the change, capture `Vec<StreamEvent>` for a set of recorded streams via
   [`truncation_parity.rs:196-231`](../../../crates/cyrup-provider/src/api/truncation_parity.rs)'s
   `events(wire, raw)` — it already covers Anthropic, OpenAI-completions, OpenAI-responses,
   Google and Mistral from one call site. (It uses `decode_sse_bytes`; that is **correct for
   a correctness oracle** and only wrong for timing — §0a.)
2. **Normalize timestamps before comparing.** `AssistantMessage.timestamp` is
   `now_millis()` re-read on *every* `snapshot`
   ([`blocks.rs:89`](../../../crates/cyrup-provider/src/api/anthropic_messages/blocks.rs) →
   [`provider_plumbing.rs:84`](../../../crates/cyrup-provider/src/utils/provider_plumbing.rs)),
   so a raw `assert_eq!` on two `Vec<StreamEvent>` fails nondeterministically. Serialize to
   JSON and zero `timestamp` / null `responseId` — the identical rule
   [`golden.rs:16-34`](../../../crates/cyrup-test-support/src/golden.rs) applies;
   **reimplement those 19 lines locally rather than taking the dependency** (§0d).
3. `StreamEvent` derives `PartialEq`
   ([`stream.rs:498`](../../../crates/cyrup-provider/src/stream.rs)), so once timestamps are
   normalized the comparison is plain equality on the normalized JSON — types, ordering
   **and** payloads, which is strictly stronger than `differential.rs`'s type-sequence diff.

### 6.4 Order of work

1. Own the state at the caller (§2) — a per-stream `Model`/`ApiId`/tool-name clone inside
   `decode_stream`. Mechanical, compiles and passes on its own. **No signature change.**
2. Split **stage C only** onto a spawned task, with `Result<…, ProviderError>` as the channel
   item (§6.2.4). One hop, one ordering obligation. Measure with §4's methodology.
3. Only if step 2 clears its share of the bar, add the second hop.
4. Roll to the other five SSE decoders. Bedrock last and separately (§1.3).

---

## 7. What to do instead — the contained work, now priced

Both were previously filed as equal-weight alternatives. **They are not.** §7.2 is worth
2.57× on its stage; §7.1 is worth 1.16× and is not worth the port risk.

### 7.1 Borrowed deserialization (stage B) — **MEASURED, AND NOT WORTH IT**

The proposal was to replace `parse_json_with_repair(data) -> Option<Value>`
([`driver.rs:75`](../../../crates/cyrup-provider/src/api/anthropic_messages/driver.rs)) with
borrowed typed deserialization over `&'a str`, on the theory that a `serde_json::Value` for a
`content_block_delta` allocates a `Map` plus a `String` per key and per value.

It does. **It is worth 79 ns/frame.** Measured over the same 9,540 frames, with a faithful
`#[serde(tag = "type")]` enum carrying `#[serde(borrow)] &'a str` for every delta payload:

```
  B   serde_json -> owned Value    591 ns/frame
  B'  borrowed typed               512 ns/frame     ->  1.16x  (range 1.12 - 1.19)
```

**1.16× on stage B is 1.02× end-to-end** against §3.2's 3,360 ns budget. serde_json's scan
and unescape dominate; the allocations do not. (Downstream map-lookup savings —
`event.get("type")`, `delta.get("text")`, ~a dozen sites in
[`events.rs`](../../../crates/cyrup-provider/src/api/anthropic_messages/events.rs) — are
additional and unmeasured, but they are lookups into an already-built `Map`, i.e. the small
end of a small term.)

Against that, the port cost is real and the constraints are sharp:

- **The repair fallback must survive.** `parse_json_with_repair`
  ([`json_parse.rs:152-161`](../../../crates/cyrup-provider/src/utils/json_parse.rs)) tries
  strict parse, then `repair_json`, then a strict parse of the repaired string. `repair_json`
  returns an owned `String`, so the borrowed path cannot borrow from it — you would keep the
  typed borrowed parse as a fast path and fall back to today's `Value` path, i.e. **both**
  code paths forever.
- **`#[serde(other)]`-style tolerance is currently free.** `driver.rs:197`'s `_ => true`
  silently ignores unknown event types. A `#[serde(tag = "type")]` enum **fails** on an
  unknown tag, so every future Anthropic event type becomes a parse error routed to the
  fallback — a behaviour change disguised as an optimisation.
- **`#[serde(borrow)]` needs the input to outlive the parse.** `frame.data` is an owned
  `String` from `eventsource_stream` and lives for the loop iteration, which suffices today —
  and is exactly what §6 would break by moving the frame across a channel.

**Recommendation: do not do §7.1.** If it is ever revisited, it must be measured first with
§4's methodology, and the number to beat is 79 ns.

### 7.2 Replace the SSE framer (stage A) — **MEASURED, 2.57×, AND THE ACTUAL WORK**

`eventsource-stream` 0.2.3 costs **1,035 ns/frame** on the wire path. A minimal in-tree framer
that keeps a `Vec<u8>` and *advances a cursor* instead of `String::split_off`-ing per line
costs **408 ns/frame** — a **2.57× reduction on the largest of the three stages**, worth
**1.23× end-to-end** with zero concurrency, zero ordering risk and no new failure mode.

```
  A   eventsource-stream, 1 event/chunk    1.035 µs/frame     (1.022 - 1.064)
  A   eventsource-stream, 8 KB chunks      1.310 µs/frame
  A'  in-tree framer,     1 event/chunk      408 ns/frame     (401 - 523)   -> 2.57x
  A'  in-tree framer,     8 KB chunks        406 ns/frame                   -> 3.22x
```

Note A' is **flat across chunk regimes** — the quadratic is gone by construction, which also
removes §0a's 113× replay artifact and speeds up every SSE test in the crate.

The working shape (validated in
[`crates/cyrup/tmp/perf006/src/main.rs`](../../../crates/cyrup/tmp/perf006/src/main.rs), which
frames the same 9,540-frame fixture and asserts frame-for-frame agreement with
`eventsource-stream` in both chunk regimes):

```rust
/// SSE framing over a byte buffer with an advancing cursor — no per-line `String::split_off`,
/// no `String::from_utf8` of the whole chunk. Reclaims the consumed prefix only when the
/// cursor passes a threshold, so steady-state cost is O(bytes), not O(bytes x lines).
#[derive(Default)]
struct Framer {
    buf: Vec<u8>,
    start: usize,
    event: String,
    data: String,
    saw_field: bool,
}

impl Framer {
    fn push(&mut self, chunk: &[u8]) {
        if self.start > 0 && self.start == self.buf.len() {
            self.buf.clear();
            self.start = 0;
        } else if self.start > 16 * 1024 {
            self.buf.drain(..self.start);
            self.start = 0;
        }
        self.buf.extend_from_slice(chunk);
    }

    fn next_frame(&mut self) -> Option<SseFrame> {
        loop {
            let rest = self.buf.get(self.start..)?;
            let nl = rest.iter().position(|b| *b == b'\n')?;
            let line = &rest[..nl];
            let line = if line.last() == Some(&b'\r') { &line[..line.len() - 1] } else { line };
            self.start += nl + 1;
            if line.is_empty() {
                if self.saw_field {
                    // SSE spec: absent `event:` means "message".
                    let event = if self.event.is_empty() {
                        "message".to_string()
                    } else {
                        std::mem::take(&mut self.event)
                    };
                    self.event.clear();
                    self.saw_field = false;
                    return Some(SseFrame { event, data: std::mem::take(&mut self.data) });
                }
                continue;
            }
            let (field, value) = match line.iter().position(|b| *b == b':') {
                Some(i) => (&line[..i], line.get(i + 1..).unwrap_or(&[])),
                None => (line, &[][..]),
            };
            let value = value.strip_prefix(b" ").unwrap_or(value);
            match field {
                b"event" => { self.event = String::from_utf8_lossy(value).into_owned(); self.saw_field = true; }
                // Multiple `data:` lines join with '\n', per spec.
                b"data"  => {
                    if !self.data.is_empty() { self.data.push('\n'); }
                    self.data.push_str(&String::from_utf8_lossy(value));
                    self.saw_field = true;
                }
                b""      => {}                       // `: comment` — ignored, does not arm dispatch
                _        => self.saw_field = true,   // `id:` / `retry:` / unknown — arms dispatch
            }
        }
    }
}
```

What this must additionally handle before it ships, none of which the probe covers:

- **UTF-8 across chunk boundaries.** The probe uses `from_utf8_lossy` per field value, which
  is correct only because each field lies wholly inside the buffer by the time a `\n` is
  found — a line is never split. That holds because framing is line-oriented and `push`
  appends. Keep it; do not "optimise" to `from_utf8_unchecked`
  (`#![forbid(unsafe_code)]` is crate-level on `cyrup-provider` anyway).
- **The BOM.** `eventsource-stream` strips a leading BOM once
  (`event_stream.rs:266-275`). Reproduce it.
- **`retry:` and `id:`.** Currently discarded by cyrup's `SseFrame`
  ([`sse.rs:169-175`](../../../crates/cyrup-provider/src/stream/sse.rs) is `{event, data}`
  only), so parity is preserved by ignoring them — but they must still *arm dispatch*, which
  is what the `_ => self.saw_field = true` arm does.
- **Error surface.** `sse.rs:485-503` unwraps `EventStreamError::Transport` by hand to reach
  the reason (see `flatten_source_chain`) and caps the message with `normalize_error_body`
  because `EventStreamError::Parser` embeds provider-controlled input. An in-tree framer has
  no `Parser` variant at all — every byte sequence frames — so the transport error is the
  only one left and that unwrapping simplifies. **Do not change the resulting
  `ProviderError::Decode` text**; it lands in `AssistantMessage.error_message` verbatim and
  `truncation_parity.rs` asserts on terminals.
- **It removes a dependency edge.** `eventsource-stream` pulls `nom`. Dropping it needs a
  `[workspace.dependencies]` removal with a justifying note, per the workspace convention.

**Definition of done for §7.2**: `SseFrame` streams are byte-identical to
`eventsource-stream`'s over every fixture in
[`src/api/*/tests/`](../../../crates/cyrup-provider/src/api) and
[`truncation_parity.rs`](../../../crates/cyrup-provider/src/api/truncation_parity.rs), in both
the single-buffer (`decode_sse_bytes`) and chunked regimes. `SseFrame` already derives
`PartialEq, Eq` (`sse.rs:169`), so that assertion is a one-liner.

---

## 8. The finding this measurement handed to another task

`hop.rs` priced the send term by payload size, and the result is the largest single lever left
in the decode path — **and it belongs to PERF-002, not here**:

```
  sink.send, empty payload            211 ns
  sink.send, alloc + send   200 B     794 ns      (+583 ns)
  sink.send, alloc + send     4 KB   1.200 µs     (+989 ns)
```

The channel primitive is nearly free; **what costs is allocating the item**. Every
`StreamEvent` cyrup sends carries a freshly-built `AssistantMessage` from
`dec.snapshot(model, api)` — `blocks_to_content`
([`blocks.rs:94-125`](../../../crates/cyrup-provider/src/api/anthropic_messages/blocks.rs))
clones every `String` and rebuilds every `Content` on all eight snapshot sites (§1.2).

At a 4 KB accumulated message that is **~1 µs per frame, larger than stages A and B
combined**, and it grows with the message. [PERF-002](PERF-002_ARC_TRANSCRIPT_SNAPSHOTS.md)'s
`Arc` change collapses it toward the 211 ns floor. **Record this in PERF-002**: it is worth
more than anything PERF-006 could deliver, and unlike a pipeline it also shrinks the term that
§3.3 shows is in series with everything.

---

## 9. Explicitly out of scope

- **A three-stage pipeline, on today's numbers.** §3.2/§3.3. Revisit only if §5's table
  selects it.
- **simd-json / sonic-rs.** Per-frame JSON is ~160 bytes and stage B is 591 ns of a 3,360 ns
  budget. §7.1 shows that even removing *all* of B's allocations is worth 79 ns; a faster
  scanner is bounded above by ~591 ns end-to-end, i.e. <1.2×. See [INDEX](INDEX.md).
- **Parallelising across frames.** Breaks ordering (§6.2.1). The decoder is a state machine
  over an ordered event sequence; there is no correct way to fold two frames concurrently.
- **Touching the idle-timeout policy** at
  [`sse.rs:125-133`](../../../crates/cyrup-provider/src/stream/sse.rs). It is correct, and the
  reasoning — including *why* a total-request `timeout` would be wrong — is documented at
  `sse.rs:27-36`.
- **Re-implementing read-ahead.** hyper already does it (§0b).
- **Adding `cyrup-test-support` to `cyrup-provider`.** (§0d.)
- **"Fixing" `decode_sse_bytes`'s quadratic by changing `decode_sse_bytes`.** It is a replay
  helper; the quadratic is in the dependency, and §7.2 removes it as a side effect of the work
  that is actually justified. Do not add chunking to `decode_sse_bytes` as a band-aid — it
  would mask the artifact rather than remove it, and the fixtures' single-buffer shape is what
  makes them readable.

---

## 10. Definition of Done

### (a) Closed as not worth it — **this is the outcome, and it is a success**

1. ✅ §3 carries real `--release` per-stage numbers at `8f49433`, 5 runs, with the probe
   sources retained at
   [`crates/cyrup/tmp/perf006/`](../../../crates/cyrup/tmp/perf006/src/main.rs).
2. ✅ §3.2 shows the serial decode budget is 3.36 µs/frame = **0.34 % of one core** at
   realistic frame rates, and §3.3 shows the pipeline's 1.94× collapses to 1.07–1.20× once
   the consumer is real.
3. ✅ §0a documents that the previously-specified probe was invalid by **113×**, so nobody
   re-derives the verdict from the artifact.
4. §7's items are re-ranked with measurements: **§7.2 is the work** (2.57× on stage A,
   1.23× end-to-end); **§7.1 is declined** (1.16×, plus a behaviour change on unknown event
   types). §8 hands the send-payload finding to PERF-002.
5. ✅ Nothing was added to `crates/`. The measurement lived and died in `tmp/` (gitignored at
   every level).
6. Frontmatter → `status: done`, with §3 as the justification.

**The remaining action is to file §7.2 as its own task** (suggested: `PERF-007 IN-TREE SSE
FRAMER`) and add the §8 note to PERF-002. Those are the deliverables that survive this file.

### (b) Shipped — retained for a future re-selection only

All of:

1. **Per-stage timings recorded in §3**, before and after, same recorded stream, `--release`,
   measured per §4 (chunked input, `STREAM_BUFFER` capacity, `multi_thread` flavor).
2. **Decode throughput improves ≥1.5×** on tokens/sec **with a consumer stalling ≥5 µs/event**
   — the regime §3.3 shows is the operating one. A number taken against a freely-draining
   consumer does not count.
3. **Event order and payloads are identical**, proven by §6.3's in-crate oracle over every
   decoder touched — normalized-JSON equality, not just a type sequence.
4. **Back-pressure still reaches the socket.** A stalled consumer stops the read; peak memory
   during a stalled stream is bounded by the declared capacities, and that bound is documented
   next to the constants as `cap × sizeof(item)` (§6.2.2). Capacity ≥100 per §3.4.
5. **Cancellation is prompt.** An aborted run tears down all stages and leaks no detached
   task; a dropped stream leaks neither a parse nor a fold task. Tested, per §6.2.3.
6. **Errors terminate the whole pipeline.** All five paths in §6.2.4's table still produce
   **exactly one** error event at the sink, in the correct position relative to preceding
   events, and end the stream — and `driver.rs:197`'s unknown-event no-op is preserved.
7. **The suite is green under the real gate:**
   ```bash
   cargo test --workspace --features test-fixtures --no-fail-fast
   cargo clippy --workspace --all-targets --features test-fixtures; echo "exit=$?"   # MUST be 0
   ```
   Check `df -h /` and `df -h /tmp` first — `target/` is ~65–83 GB on a 96 GB volume, and a
   full `/tmp` surfaces as `ld terminated with signal 7 [Bus error]`, not as a disk error.
