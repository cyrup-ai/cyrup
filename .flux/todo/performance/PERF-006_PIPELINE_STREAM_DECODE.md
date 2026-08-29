---
stage: aug
status: in-progress
updated: 2026-08-29 04:17
aug_against: cyrup HEAD 8f49433 (branch david/performance) · reqwest 0.13.4 · hyper 1.10.1 · hyper-util 0.1.20 · eventsource-stream 0.2.3 · tokio 1.52.3
---

# Pipeline the stream decode across cores

> One task does bytes → SSE frame → JSON → fold → snapshot → send, serially. Splitting it
> into a bounded 3-stage pipeline would make decode cost `max(stage)` instead of
> `sum(stages)`.
>
> **Augmentation finding: two of the three stages are not what this task assumed, and the
> arithmetic already predicts the answer.** Stage A's socket read is *already* on a separate
> tokio task — hyper spawns one per connection — so the read-ahead this task wants to build
> exists in the dependency. And stage C is currently ~10³–10⁴× stage B, which is why the
> pipeline buys nothing today and (after PERF-001 removes that ratio) buys nothing
> tomorrow either. **The predicted outcome is DoD (a): close as not worth it.** §4 is the
> probe that must produce the numbers to say so, and §7 is the contained work that is
> genuinely there.

---

## 0. READ THIS FIRST — five things that will send you the wrong way

**(a) `pi structurally cannot do this` is true but irrelevant.** The original framing leaned
on cyrup having threads where pi has one event loop. That is a real difference, and the
runtime does support it — [`crates/cyrup/src/main.rs:48`](../../../crates/cyrup/src/main.rs)
is `#[tokio::main(flavor = "multi_thread")]`, so spawned stages genuinely land on different
cores. It is irrelevant because *having* parallelism available does not mean there is
anything to overlap. See §3.

**(b) Stage A is ALREADY on another task, and the task file was wrong to imply otherwise.**
`resp.bytes_stream()` ([`sse.rs:466`](../../../crates/cyrup-provider/src/stream/sse.rs)) does
**not** perform a `read(2)` on the decode task. reqwest 0.13.4 sits on hyper-util 0.1.20's
legacy client, which **spawns a background dispatcher task per connection** —
`executor.execute(...)` at `client.rs:552` (HTTP/2, logged *"http2 handshake complete,
spawning background dispatcher task"*) and `client.rs:584` (HTTP/1, same message). The body
stream the decoder polls is fed by that task over a channel. **§3b ("read-ahead only") is
therefore already implemented by the dependency graph**, and building it again would add a
buffer between hyper's channel and the framer, not between the socket and the CPU.

**(c) …but stage A is not "near-zero CPU" either. The original table was wrong in both
directions.** What still runs on the decode task is the SSE *framing*, and
`eventsource-stream` 0.2.3 does two allocating copies per frame:

- [`utf8_stream.rs:64-67`] — `buffer.extend_from_slice(bytes)`, then `core::mem::take` +
  `String::from_utf8`: a full UTF-8 validating pass plus a move, per chunk.
- [`event_stream.rs:220-223`] — per **line**:
  ```rust
  let consumed = buffer.len() - rem.len();
  let rem = buffer.split_off(consumed);   // allocates a NEW String, memcpy's the remainder
  *buffer = rem;
  ```
  `String::split_off` is O(remaining), so parsing *n* lines out of one chunk is O(n²) in the
  chunk's bytes. Small in absolute terms, but it is real allocator traffic per frame and it
  must be **measured**, not assumed away. (Vendored copies for reference:
  `~/.cargo/registry/src/*/eventsource-stream-0.2.3/src/`.)

**(d) DoD item 3 as originally written is not buildable.** It says
`cyrup-test-support`'s `differential.rs` "is the existing machinery … and should be what
proves it". **`cyrup-provider` does not depend on `cyrup-test-support`, and must not.**
Verified at HEAD: `cyrup-provider`'s entire `[dev-dependencies]` is `tokio` + `tokio-stream`
([`Cargo.toml`](../../../crates/cyrup-provider/Cargo.toml), final section). The edge runs the
other way — `cyrup-test-support` depends on `cyrup-provider` **with `features = ["faux"]` on
a NORMAL edge**, and the 28-line comment block at
[`cyrup-provider/Cargo.toml:25-52`](../../../crates/cyrup-provider/Cargo.toml) exists
specifically to keep `faux` off the shipped graph. Adding the reverse dev-edge would unify
`faux` into `cyrup-provider`'s own test build and undo that. **Use the in-crate oracle
instead — it already exists and covers five decoders**: `truncation_parity.rs:196`,
`async fn events(wire: Wire, raw: &str) -> Vec<StreamEvent>`
([`api/truncation_parity.rs:196-215`](../../../crates/cyrup-provider/src/api/truncation_parity.rs)),
which drives each decoder over recorded SSE via `decode_sse_bytes` and returns every emitted
event in order, with zero extra dependencies. §6.3 specifies exactly how to use it.

**(e) Every citation below was re-verified against `8f49433`.** Corrections to the numbers
as originally filed: the idle-timeout application is `with_idle_timeout` at
**`sse.rs:124-133`** with `read_timeout` at **`:130`** (filed as `:125-131`); its rationale
para runs **`sse.rs:28-37`** (filed as `:28-35`) — `:36-37` is the "a total-request deadline
would be *wrong*" sentence, which is the load-bearing half. `Fanout::emit` is
**`subscriber.rs:64-76`** with the two `ev.clone()` sends at **`:68`** and **`:72`** (filed as
`:63-72`). The transport-`Err` arm is **`driver.rs:50-56`** and the `event: error` arm
**`driver.rs:58-67`** (filed as `:51-57` / `:51-66`). Accurate as filed: `sse.rs:466`,
`driver.rs:49`, `subscriber.rs:23` (`CHANNEL_CAPACITY = 1024`), and the six-other-decoders
list — with the caveat in §1.3 that one of them is not an SSE decoder at all.

---

## 1. Where it is — verified

### 1.1 The three stages, as the code actually splits them

| stage | what runs **on the decode task** | where |
| --- | --- | --- |
| **A** | SSE framing only: UTF-8 validation + line split + per-line `String::split_off`. **Not** the socket read (0b). | [`sse.rs:466`](../../../crates/cyrup-provider/src/stream/sse.rs) `.eventsource()`, then the `stream::unfold` at `:467-503` |
| **B** | `parse_json_with_repair(data)` → owned `serde_json::Value` | [`driver.rs:75`](../../../crates/cyrup-provider/src/api/anthropic_messages/driver.rs) → [`json_parse.rs:152-162`](../../../crates/cyrup-provider/src/utils/json_parse.rs) |
| **C** | `process_event` → `Decoder` mutation → `snapshot()` → `sink.send().await` | [`driver.rs:86`](../../../crates/cyrup-provider/src/api/anthropic_messages/driver.rs) → [`events.rs`](../../../crates/cyrup-provider/src/api/anthropic_messages/events.rs) |

The `unfold` at `sse.rs:473-507` is worth reading before touching anything: it already
carries the `CancelToken` in a `tokio::select! { biased; _ = state.cancel.cancelled() => … }`
and yields exactly one `ProviderError::Aborted` before ending. **Cancellation for stage A is
solved and must not be re-solved** — a pipeline just has to not lose it.

### 1.2 Stage C's six `snapshot` call sites

Confirmed in [`events.rs`](../../../crates/cyrup-provider/src/api/anthropic_messages/events.rs)
at `:147` (`text_delta`), `:160` (`thinking_delta`), `:176` (`input_json_delta`), `:207`
(`send_with_pos`, all four `*_start`), `:251` (`process_block_stop`), `:266` (`emit_error`),
plus [`driver.rs:42`](../../../crates/cyrup-provider/src/api/anthropic_messages/driver.rs)
(`Start`) and `:124` (`end_of_stream`). Each lands in `blocks_to_content`
([`blocks.rs:94-121`](../../../crates/cyrup-provider/src/api/anthropic_messages/blocks.rs)),
which re-parses the whole accumulated tool buffer. **That is PERF-001's target and it is the
entirety of why stage C is currently dominant.**

### 1.3 The other decoders — six SSE, one binary

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

**`bedrock_converse_stream` is NOT one of them** and the original list was wrong to include
it as the same shape. It speaks AWS `vnd.amazon.eventstream` binary framing, not SSE: it
polls `response.bytes_stream()` directly under its own `tokio::select!` cancel arm and drives
a synchronous `EventStreamDecoder` over accumulated bytes
([`bedrock_converse_stream/driver.rs:238-272`](../../../crates/cyrup-provider/src/api/bedrock_converse_stream/driver.rs),
[`framing.rs:29-43`](../../../crates/cyrup-provider/src/api/bedrock_converse_stream/framing.rs)).
Its stage A is CPU on the decode task with no `Stream` boundary to cut at, so any pipeline
design must treat it as a seventh, different case — or exclude it and say so.

---

## 2. The obstacle the original task did not name: nothing here is `'static`

This is the single largest cost of building the pipeline and it is not mentioned anywhere in
the task as filed.

[`anthropic_messages/mod.rs:131`](../../../crates/cyrup-provider/src/api/anthropic_messages/mod.rs):

```rust
decode_stream(frames, model, &self.api, &sink, is_oauth, &ctx.tools).await;
```

Every argument but `frames` is a **borrow** from `ApiImpl::run`'s `&self` / `&Model` /
`&Context` ([`api/mod.rs:92-101`](../../../crates/cyrup-provider/src/api/mod.rs)).
`tokio::spawn` requires `Future: Send + 'static` (the trait is declared at
[`api/mod.rs:94-106`](../../../crates/cyrup-provider/src/api/mod.rs)). So each spawned stage
needs owned state:

- `Arc<Model>` — `Model` is `Clone` ([`model.rs:52-54`](../../../crates/cyrup-provider/src/model.rs))
  but carries `ModelCost` with an `Option<Vec<ModelCostTier>>` and several `String`s; clone it
  once into an `Arc`, never per frame.
- `ApiId` — `Clone`, cheap.
- `Arc<[String]>` for `tool_names` — today built per call as
  `tools.iter().map(|t| t.name.clone()).collect()`
  ([`driver.rs:35-38`](../../../crates/cyrup-provider/src/api/anthropic_messages/driver.rs)).
- `EventSink` — already `Clone` ([`api/mod.rs:66-70`](../../../crates/cyrup-provider/src/api/mod.rs)).

**This changes the `decode_stream` signature in all seven decoders**, plus their in-crate test
callers (`anthropic_messages/tests/mod.rs:88-101`, `truncation_parity.rs:196-215`,
`openai_completions/tests/mod.rs`, `openai_responses/tests/`,
`google_generative_ai/tests/mod.rs`, `mistral_conversations/tests/mod.rs`,
`pi_messages.rs:1161/1513/1556/1586`, `tests/anthropic_sensitive_stop.rs:181`). Budget for it
before deciding the pipeline is cheap.

`FrameStream` is `Pin<Box<dyn Stream<Item = Result<SseFrame, ProviderError>> + Send>>`
([`sse.rs:295`](../../../crates/cyrup-provider/src/stream/sse.rs),
[`provider_plumbing.rs:92`](../../../crates/cyrup-provider/src/utils/provider_plumbing.rs)) —
the `+ Send` is there, so moving the stream into a spawned task does compile. That part is fine.

---

## 3. The arithmetic that already predicts the answer

**Do this reasoning before writing any code. It takes five minutes and it is why §4 is a
probe rather than a pipeline.**

From [PERF-001 §2](PERF-001_STREAM_SNAPSHOT_QUADRATIC.md) — measured, `--release`, this host:

> 256 KB tool call, 6,554 deltas, streamed re-parse: **10.058 s**

That is stage C's parse alone, and it works out to **≈ 1.53 ms per delta**.

Stage B on the same stream is one `serde_json::from_str::<Value>` over a single
`content_block_delta` frame — on the order of **10² ns** for a ~100-byte frame. Stage A is
one UTF-8 pass plus one `split_off` over a similarly small buffer, same order.

```
today:   A ≈ B ≈ 10²–10³ ns      C ≈ 1.53 × 10⁶ ns
         sum/max = (A+B+C)/C  ≈  1.000_6×
```

**A perfect three-stage pipeline today is worth six parts in ten thousand.** The entire
observable cost is stage C, and PERF-001 removes it *in place* — without a thread boundary,
without channels, without an ordering obligation.

Now the case this task actually asks about — after PERF-001 lands. PERF-001's §4a memo makes a
delta cost "rebuild one block" instead of "rebuild every block", and §4c makes the
downstream copies refcount bumps. Stage C's remaining work per delta is one
`Content::text(text.clone())` for the changed block plus the `AssistantMessage` construction —
hundreds of ns, i.e. **the same order as A and B**. Then:

```
after PERF-001:  A ≈ B ≈ C   ⇒   sum/max ≈ 3×   (theoretical ceiling, zero overhead)
```

and the ceiling is not reachable, because of a term that is *in series with all three stages
and cannot be pipelined*: `sink.send(...).await`. It is bounded
([`api/mod.rs:85-88`](../../../crates/cyrup-provider/src/api/mod.rs)) and back-pressured
through `Fanout::emit`, which awaits **every** subscriber
([`subscriber.rs:64-76`](../../../crates/cyrup-session-svc/src/subscriber.rs) —
*"backpressure → slows the agent, never drops"*) all the way to the TUI draw. Whenever the
renderer is the slow party, decode throughput is the renderer's number and `max(A,B,C)` is
noise. **That is PERF-005's problem, and PERF-005 is the task that fixes it.**

The DoD below demands **≥1.5× on decode tokens/sec** to justify shipping. Against a 3×
theoretical ceiling on a term that is not the bottleneck once PERF-001 and PERF-005 land, the
honest prediction is that the measurement will not clear the bar.

**Record the numbers anyway.** An unmeasured "probably not worth it" is a guess, and this file
exists so nobody has to guess again.

---

## 4. The required first deliverable: the probe

**Prerequisite: [PERF-001](PERF-001_STREAM_SNAPSHOT_QUADRATIC.md) must have landed.** Running
this before PERF-001 measures the quadratic, not the pipeline, and will report a meaningless
`C/B ≈ 10⁴`. If PERF-001 has not landed, **stop and do PERF-001 first** — that is the
correct next action, not a workaround.

### 4.1 Where it goes

`decode_stream` is `pub(crate)`
([`anthropic_messages/mod.rs:32`](../../../crates/cyrup-provider/src/api/anthropic_messages/mod.rs)),
so an external `crates/cyrup-provider/tests/*.rs` **cannot reach it**. The probe must be an
in-crate `#[cfg(test)]` module. Add it as
`crates/cyrup-provider/src/api/anthropic_messages/tests/stage_probe.rs`, declared from
[`tests/mod.rs`](../../../crates/cyrup-provider/src/api/anthropic_messages/tests), and
**delete it once the numbers are in this file** — it is a measuring stick, not a test.

Run with:

```bash
cargo test -p cyrup-provider --release \
  anthropic_messages::tests::stage_probe -- --nocapture --test-threads=1
```

`--release` is mandatory; a debug-profile ratio between three small functions is noise.

### 4.2 The probe

Times each stage over the **same** recorded stream, in isolation, by materialising the
intermediate between each pair. Reuse the existing model/context fixtures already in
[`tests/mod.rs`](../../../crates/cyrup-provider/src/api/anthropic_messages/tests) (`model()`,
and the `collect()` helper at `:88-101` for the shape).

```rust
//! PERF-006 stage probe. NOT a test — a measuring stick. Delete once §5 is filled in.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::model;
use crate::api::{channel, anthropic_messages::decode_stream};
use crate::stream::sse::SseFrame;
use crate::utils::json_parse::parse_json_with_repair;
use crate::{decode_sse_bytes, StreamEvent};
use cyrup_core::ApiId;
use futures::StreamExt;
use serde_json::Value;
use std::time::Instant;

/// A realistic stream: a long prose block, then a large tool call in 40-byte
/// `input_json_delta`s — the two shapes DoD (b).2 names.
fn recorded_stream(prose_kb: usize, tool_kb: usize) -> Vec<u8> {
    let mut s = String::new();
    s.push_str("event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":10}}}\n\n");

    s.push_str("event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n");
    let chunk = "the quick brown fox jumps over the lazy dog ";
    for _ in 0..(prose_kb * 1024 / chunk.len()) {
        s.push_str(&format!(
            "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"{chunk}\"}}}}\n\n"
        ));
    }
    s.push_str("event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n");

    s.push_str("event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"write\"}}\n\n");
    let body = "fn main() { println!(\\\\\"hi\\\\\"); }\\\\n".repeat(tool_kb * 1024 / 32);
    let args = format!("{{\\\"path\\\":\\\"src/main.rs\\\",\\\"content\\\":\\\"{body}\\\"}}");
    let bytes: Vec<char> = args.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        let end = (i + 40).min(bytes.len());
        let piece: String = bytes[i..end].iter().collect();
        s.push_str(&format!(
            "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":1,\"delta\":{{\"type\":\"input_json_delta\",\"partial_json\":\"{piece}\"}}}}\n\n"
        ));
        i = end;
    }
    s.push_str("event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n");
    s.push_str("event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":900}}\n\n");
    s.push_str("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");
    s.into_bytes()
}

#[tokio::test(flavor = "multi_thread")]
async fn stage_probe() {
    for (prose_kb, tool_kb) in [(4usize, 4usize), (16, 16), (64, 64), (128, 256)] {
        let raw = recorded_stream(prose_kb, tool_kb);
        let m = model();
        let api = ApiId::from(super::super::API_ID);

        // ---- stage A: framing only (UTF-8 + line split + split_off) ----
        let t = Instant::now();
        let mut frames_out: Vec<SseFrame> = Vec::new();
        {
            let mut fs = decode_sse_bytes(raw.clone());
            while let Some(f) = fs.next().await {
                frames_out.push(f.expect("frame"));
            }
        }
        let a = t.elapsed();

        // ---- stage B: JSON parse of each frame, nothing else ----
        let t = Instant::now();
        let mut values: Vec<Value> = Vec::with_capacity(frames_out.len());
        for f in &frames_out {
            if let Some(v) = parse_json_with_repair(f.data.trim()) {
                values.push(v);
            }
        }
        let b = t.elapsed();
        std::hint::black_box(&values);

        // ---- stage A+B+C: the whole loop as it ships, with a drain consumer ----
        // The consumer drains as fast as it can, so this isolates decode from the
        // renderer's back-pressure (which §3 explains is in series regardless).
        let (sink, mut rx) = channel(1024);
        let m2 = m.clone();
        let api2 = api.clone();
        let raw2 = raw.clone();
        let t = Instant::now();
        let task = tokio::spawn(async move {
            let fs = decode_sse_bytes(raw2);
            decode_stream(fs, &m2, &api2, &sink, false, &[]).await;
        });
        let mut n = 0usize;
        while let Some(ev) = rx.recv().await {
            std::hint::black_box(&ev);
            n += 1;
        }
        task.await.expect("decode task");
        let total = t.elapsed();

        // C is the whole loop minus the two stages measured in isolation.
        let c = total.saturating_sub(a).saturating_sub(b);
        let sum = a + b + c;
        let max = a.max(b).max(c);
        println!(
            "prose {prose_kb:>3} KB / tool {tool_kb:>3} KB | frames {:>6} events {n:>6} | \
             A {a:>10.3?}  B {b:>10.3?}  C {c:>10.3?} | total {total:>10.3?} \
             sum/max {:.3}x",
            frames_out.len(),
            sum.as_secs_f64() / max.as_secs_f64().max(f64::MIN_POSITIVE),
        );
    }
}
```

Two things about this probe that are deliberate and must not be "simplified":

- **`flavor = "multi_thread"`.** The default `#[tokio::test]` is `current_thread`, on which
  the spawned decode task and the receiving loop interleave on one core — which would make
  the `total` measurement structurally unable to show overlap and would bias the answer
  toward "no benefit". Do not let the harness pre-decide the result.
- **`C = total − A − B` rather than a third direct measurement.** Timing `process_event` +
  `snapshot` + `send` in isolation would require reconstructing decoder state outside the
  driver, which is exactly the kind of hand-built replica that drifts from the code it claims
  to measure. Subtraction reuses the shipping loop verbatim. It slightly *over*-attributes to
  C (the loop's own `await` overhead lands there), which is the safe direction: it biases
  toward "pipeline looks worthwhile", so a "not worth it" verdict taken from it is
  conservative.

### 4.3 Also measure the framing allocations (§0c)

While the probe is in place, take one extra number: run stage A alone against a stream fed in
**realistic chunk sizes** rather than one `Bytes`. `decode_sse_bytes`
([`sse.rs:512-526`](../../../crates/cyrup-provider/src/stream/sse.rs)) wraps the whole buffer
in a single `stream::once`, so it exercises the `split_off` path at its *worst* (one huge
buffer, every line paying O(remaining)) — which over-states stage A versus the wire, where
hyper delivers ~8–16 KB frames. Note which regime the reported A came from; if A turns out to
matter at all, re-run it with `futures::stream::iter` over 8 KB slices before drawing any
conclusion.

---

## 5. Pre-registered decision rule — fill this in, then act on it

Paste the probe's output here, verbatim, then take the branch it selects. **Deciding the rule
after seeing the numbers is how a speculative task talks itself into shipping.**

```
(paste probe output here — cyrup HEAD, date, `--release`)
```

| observed | verdict |
| --- | --- |
| `sum/max` < **1.2×** at every size | **Close as DoD (a).** No stage dominates; there is nothing to overlap. Record the table, set `status: done`, and do §7 instead. |
| `sum/max` ≥ 1.2× but the dominant stage is **C** | PERF-001 did not fully land, or missed a decoder. **Go back to PERF-001**, do not build a pipeline around a quadratic. |
| `sum/max` ≥ 1.2× and the dominant stage is **A** | §7.2 (framing allocations), not a pipeline. A single-hop read-ahead cannot beat fixing an O(n²) `split_off`. |
| `sum/max` ≥ 1.2× and the dominant stage is **B** | §7.1 (borrowed deserialization), not a pipeline. Contained to one decoder, no ordering risk. |
| `sum/max` ≥ **2.0×** and no single stage is fixable in place | **Only then** build §6. |

---

## 6. If — and only if — §5 selects it: the required implementation

Do not skim this into existence. Every property below is one the current single-task loop
gets **for free**, and a pipeline must re-establish each one explicitly.

### 6.1 Shape

```
hyper conn task ──> bytes_stream ──> [framer task] ──mpsc<SseFrame>──> [parse task]
                                                    ──mpsc<(SseFrame, Value)>──> [fold task] ──> sink
       (already spawned, §0b)          stage A            cap N          stage B      cap N       stage C
```

Single-producer/single-consumer bounded `tokio::sync::mpsc` per hop. **Nothing fans out.**

### 6.2 The four non-negotiables

1. **Exact event order.** One consumer per hop preserves it. Any design that spreads parse
   work over multiple workers does not, and is forbidden (§8).

2. **Back-pressure still reaches the socket.** A bounded `mpsc` propagates the stall backward
   through all three hops to `bytes_stream`, and from there to hyper's connection task, which
   stops reading. The *bound* becomes the declared memory ceiling for unparsed data and must
   be documented next to the constant, in the style of
   [`subscriber.rs:23`](../../../crates/cyrup-session-svc/src/subscriber.rs)
   (`const CHANNEL_CAPACITY: usize = 1024;`). Note the ceiling is
   `cap × sizeof(SseFrame + Value)`, and a `Value` for a large `content_block_start` is not
   small — pick the number from that product, not by copying 1024.

3. **Prompt cancellation.** Stage A's `CancelToken` arm already exists at
   [`sse.rs:473-507`](../../../crates/cyrup-provider/src/stream/sse.rs) (`tokio::select!` with
   `biased;` at `:477-478`) and must be preserved as-is. Stages B and C are spawned tasks and each needs its
   own observation of the token — the same `select!` pattern used throughout
   [`cyrup-ext/src/host/live.rs`](../../../crates/cyrup-ext/src/host/live.rs) (`:1446`,
   `:1506`, `:1526`, `:1559`, `:1579`, `:2034`), whose doc comment at `:1277-1280` states the
   governing rule: *state "must be torn down on EVERY exit path"*, including the one where the
   future is simply dropped at an await point. Dropping the sender is the natural teardown
   signal; **verify it, do not assume it** — DoD (b).5 is a test, not a claim.
   [`cyrup-core/src/keyed_lock.rs`](../../../crates/cyrup-core/src/keyed_lock.rs) is the other
   in-tree reference for cancel-aware waiting.

4. **Errors terminate the *whole* pipeline, not one stage.** Two paths, both of which today
   `return` out of the single loop and must keep producing **exactly one** error event at the
   sink:
   - transport `Err` → `sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))`,
     [`driver.rs:50-56`](../../../crates/cyrup-provider/src/api/anthropic_messages/driver.rs);
   - `event: error` frame → `emit_error(&dec, model, api, sink, msg)`, `driver.rs:58-67`
     (pi `anthropic-messages.ts:439-441`).

   Both need `dec` — decoder state that lives in **stage C**, while the transport error is
   detected in **stage A**. So an error must travel forward down the pipeline as a *value* and
   be emitted by stage C, not emitted at the point of detection. Sending it sideways to the
   sink from stage A would race the frames still in flight and can emit the error *before*
   events that logically precede it. **This is the subtlest correctness requirement in the
   whole task.** The channel item type must therefore be something like
   `Result<(SseFrame, Value), ProviderError>`, with `Err` short-circuiting the fold and closing
   the pipeline.

   The same applies to the three other terminal paths that need `dec`: the unparseable-frame
   error (`driver.rs:75-84`), the `stop_reason == Error` check (`:89-100`), and the
   `saw_message_start && !saw_message_stop` truncation error (`:104-115`).

### 6.3 The ordering oracle (replaces the unbuildable DoD item 3)

Use the in-crate helper, per §0d:

1. Before the change, capture `Vec<StreamEvent>` for a set of recorded streams via
   [`truncation_parity.rs:196-222`](../../../crates/cyrup-provider/src/api/truncation_parity.rs)'s
   `events(wire, raw)` — it already covers Anthropic, OpenAI-completions, OpenAI-responses,
   Google and Mistral from one call site.
2. **Normalize timestamps before comparing.** `AssistantMessage.timestamp` is
   `now_millis()` re-read on *every* `snapshot`
   ([`blocks.rs:89`](../../../crates/cyrup-provider/src/api/anthropic_messages/blocks.rs) →
   [`provider_plumbing.rs:84-89`](../../../crates/cyrup-provider/src/utils/provider_plumbing.rs)),
   so a raw `assert_eq!` on two `Vec<StreamEvent>` fails nondeterministically. Serialize to
   JSON and zero `timestamp` / null `responseId` — the identical rule
   [`golden.rs:16-34`](../../../crates/cyrup-test-support/src/golden.rs) applies; **reimplement
   those ~18 lines locally rather than taking the dependency** (§0d).
3. `StreamEvent` derives `PartialEq`
   ([`stream.rs:498`](../../../crates/cyrup-provider/src/stream.rs)), so once timestamps are
   normalized the comparison is a plain equality on the normalized JSON — types, ordering
   **and** payloads, which is strictly stronger than `differential.rs`'s type-sequence diff.

### 6.4 Order of work

1. Make `decode_stream` take owned/`Arc` state (§2) — mechanical, compiles and passes on its
   own, and is a prerequisite for anything spawned.
2. Split **stage C only** onto a spawned task, with `Result<…, ProviderError>` as the channel
   item (§6.2.4). One hop, one ordering obligation. Measure.
3. Only if step 2 clears its share of the bar, add the second hop.
4. Roll to the other five SSE decoders. Bedrock last and separately (§1.3).

---

## 7. What to do instead — the contained work that is actually there

These are worth doing **regardless** of §5's verdict, and if §5 closes the task they are the
deliverable. Each is smaller than the pipeline, carries no ordering or cancellation risk, and
is independently measurable with the §4 probe.

### 7.1 Stop building an owned `Value` per frame (stage B)

[`driver.rs:75`](../../../crates/cyrup-provider/src/api/anthropic_messages/driver.rs) calls
`parse_json_with_repair(data) -> Option<Value>`, and everything downstream then does map
lookups against it — `event.get("type")`, `delta.get("text")`, `cb.get("signature")`, … — at
roughly a dozen sites across
[`events.rs`](../../../crates/cyrup-provider/src/api/anthropic_messages/events.rs). A
`serde_json::Value` for a `content_block_delta` allocates a `Map` plus a `String` per key and
per value.

Replace it with borrowed deserialization into a typed frame enum over `&'a str`:

```rust
/// One Anthropic stream event, borrowed from the frame's own `data` buffer.
/// `#[serde(borrow)]` makes every `&str` field a slice into `frame.data`, so a
/// `content_block_delta` costs zero allocations instead of a `Map` + two `String`s.
#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicFrame<'a> {
    MessageStart { message: MessageStart<'a> },
    ContentBlockStart { index: i64, content_block: ContentBlock<'a> },
    ContentBlockDelta { index: i64, delta: BlockDelta<'a> },
    ContentBlockStop { index: i64 },
    MessageDelta { #[serde(default)] delta: MessageDelta<'a>, #[serde(default)] usage: Option<UsageWire> },
    MessageStop,
}

#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BlockDelta<'a> {
    TextDelta { #[serde(borrow)] text: &'a str },
    ThinkingDelta { #[serde(borrow)] thinking: &'a str },
    InputJsonDelta { #[serde(borrow)] partial_json: &'a str },
    SignatureDelta { #[serde(borrow)] signature: &'a str },
}
```

Two constraints that make this a real port change rather than a refactor:

- **The repair fallback must survive.** `parse_json_with_repair`
  ([`json_parse.rs:152-162`](../../../crates/cyrup-provider/src/utils/json_parse.rs)) tries
  strict parse, then `repair_json`, then a strict parse of the repaired string. `repair_json`
  returns an owned `String`, so the borrowed path cannot borrow from it — keep the typed
  borrowed parse as the fast path and fall back to today's `Value` path when it fails. That
  fallback is also what preserves the `#[serde(other)]`-style tolerance for unknown event
  types that `driver.rs:197` (`_ => true`) currently gives for free — an unrecognised `type`
  must stay a silent no-op, never an error.
- **`#[serde(borrow)]` needs the input to outlive the parse.** `frame.data` is an owned
  `String` from `eventsource_stream` and lives for the loop iteration, which is sufficient
  today — but it is exactly what a pipeline would break by moving the frame across a channel.
  If §5 ever selects §6, this and that must be reconciled (send the `String` with the parsed
  view, or parse in the fold stage).

### 7.2 The framing allocations (stage A, §0c)

`event_stream.rs:220-223`'s `buffer.split_off(consumed)` allocates and memcpy's per line.
This is a **dependency-level** cost, so the options are (in order of preference):

1. Confirm it matters first, using §4.3's chunked-input measurement. On realistic 8–16 KB
   chunks it may be negligible; on `decode_sse_bytes`'s single-buffer path it is not, and
   `decode_sse_bytes` is a **test/replay** path, so a bad number there is not automatically a
   production problem. Establish which regime before spending anything.
2. If it does matter on the wire path: replace `eventsource-stream` with an in-tree framer.
   SSE framing is ~80 lines (field split on `:`, `data:` accumulation with `\n` joins,
   dispatch on blank line) and cyrup already owns `SseFrame`
   ([`sse.rs:170-175`](../../../crates/cyrup-provider/src/stream/sse.rs)) and the
   `stream::unfold` around it (`SseFrame` derives `Clone, Debug, PartialEq, Eq` at
   [`sse.rs:169-175`](../../../crates/cyrup-provider/src/stream/sse.rs), so it is already
   comparable in a golden test). A `BytesMut`-based framer with `advance()` instead of
   `split_off` removes the copy entirely. This also removes a `nom` dependency edge.
   **Do not start here** — it is only justified by a number from step 1.

---

## 8. Explicitly out of scope

- **simd-json / sonic-rs.** Per-frame JSON is ~100 bytes; §3 shows parse is not the dominant
  term. See [INDEX](INDEX.md). Revisit only if §5 selects the "dominant stage is B" row, and
  even then §7.1 (removing the allocation) beats swapping the parser.
- **Parallelising across frames.** Breaks ordering (§6.2.1). The decoder is a state machine
  over an ordered event sequence; there is no correct way to fold two frames concurrently.
- **Touching the idle-timeout policy** at
  [`sse.rs:124-133`](../../../crates/cyrup-provider/src/stream/sse.rs). It is correct, and the
  reasoning — including *why* a total-request `timeout` would be wrong — is documented at
  `sse.rs:28-37`.
- **Re-implementing read-ahead.** hyper already does it (§0b).
- **Adding `cyrup-test-support` to `cyrup-provider`.** (§0d.)

---

## 9. Definition of Done

**Either** of these closes the task.

### (a) Closed as not worth it — the predicted outcome, and a success

1. §5's table is filled in with real `--release` probe output taken **after PERF-001 landed**,
   at all four sizes, naming the cyrup commit.
2. Those numbers show `sum/max` within ~20% of 1.0 — no stage dominates.
3. §7's items are either done or filed as their own tasks with their own numbers.
4. The probe module is **deleted**; nothing from §4 remains in the tree.
5. Frontmatter → `status: done`, with the numbers as the justification.

### (b) Shipped

All of:

1. **Per-stage timings recorded in §5**, before and after, same recorded stream, `--release`.
2. **Decode throughput improves ≥1.5×** on tokens/sec, measured on a stream with both a long
   prose body and a large tool call (the §4.2 generator produces exactly that), with the
   consumer draining freely so the number is decode's and not the renderer's.
3. **Event order and payloads are identical**, proven by §6.3's in-crate oracle over every
   decoder touched — normalized-JSON equality, not just a type sequence.
4. **Back-pressure still reaches the socket.** A stalled consumer stops the read; peak memory
   during a stalled stream is bounded by the declared capacities, and that bound is documented
   next to the constants as `cap × sizeof(item)` (§6.2.2).
5. **Cancellation is prompt.** An aborted run tears down all stages and leaks no detached
   task; a dropped stream leaks neither a parse nor a fold task. Tested, per §6.2.3.
6. **Errors terminate the whole pipeline.** A transport `Err`, an `event: error` frame, an
   unparseable frame, a `stop_reason == Error`, and a `message_start` with no `message_stop`
   each still produce **exactly one** error event at the sink, in the correct position
   relative to preceding events, and end the stream (§6.2.4).
7. **The suite is green under the real gate:**
   ```bash
   cargo test --workspace --features test-fixtures --no-fail-fast
   cargo clippy --workspace --all-targets --features test-fixtures; echo "exit=$?"   # MUST be 0
   ```
   Check `df -h /` and `df -h /tmp` first — `target/` is ~65–83 GB on a 96 GB volume, and a
   full `/tmp` surfaces as `ld terminated with signal 7 [Bus error]`, not as a disk error.
