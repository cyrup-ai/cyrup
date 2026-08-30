---
stage: qa
status: completed
updated: 2026-08-30 02:55
aug_against: cyrup HEAD 7913760 (branch david/performance) · reqwest 0.13.4 · hyper 1.10.1 · hyper-util 0.1.20 · eventsource-stream 0.2.3 · nom 7.1.3 · tokio 1.52.3 · serde_json 1.0.150
aug_reverified: cyrup main **b7a564d** (AUG-5, after PERF-003/004/005 landed) — every citation re-checked
  on a FRESH CLONE. All cyrup-side line numbers EXACT; 2 upstream line numbers corrected; 2 call-site
  counts corrected; the probe tree is GONE. **READ §9 FIRST.**
measured_on: a PREVIOUS host and branch, `--release`, 4 runs, medians. **The probes that produced
  these numbers are GONE — §9.1.** Upstream is re-vendored for citation at
  [`tmp/eventsource-stream-0.2.3/`](../../../tmp/eventsource-stream-0.2.3/src/event_stream.rs);
  `tmp/` is gitignored, so re-create it with the one-liner in §9.1 if missing.
---

# Replace the SSE framer

> **The three-stage pipeline this file was opened for is CLOSED as not worth it — §1.**
> The work that survives, and that `/flux/exec` must ship, is a single contained change:
> **replace `eventsource-stream` 0.2.3 with an in-tree incremental SSE framer in
> [`cyrup-provider/src/stream/sse.rs`](../../../crates/cyrup-provider/src/stream/sse.rs).**
>
> It is worth **2.46×** on the wire path, **3.42×** at 8 KB chunks, and **383×** on the replay
> path every `decode_sse_bytes` call site takes. It also removes a **live panic** the dependency
> can inflict on a running turn, removes an `unsafe` block from the dependency graph, drops
> `nom` + `eventsource-stream` from the tree entirely, and retires the design contortion
> [PERF-001's test module](../../../crates/cyrup-provider/src/api/anthropic_messages/tests/perf001.rs)
> had to adopt to work around the dependency's quadratic.

---

> ## ⚠️ START AT [§9](#9-aug-5-re-verification--main-b7a564d-on-a-fresh-clone--read-before-0)
> AUG-5 re-verified this file against **`main` at `b7a564d`** on a **fresh clone**. The §4 plan is
> unchanged and ready to type in — but **the probe tree every §3 number cites is GONE** (`tmp/` is
> gitignored at any depth), two upstream line numbers were wrong, and two call-site counts were
> wrong. §9 supersedes §3's "reproduce with" instruction and corrects §4.1's `parser.rs` citation.

## 0. What changed since the previous augmentation — read this first

The previous revision was written at `8f49433`. Four things have moved, and three of them
invalidate something it said.

**(a) PERF-001 LANDED (`04d6fa5`).** Stage C is no longer a projection; it is the code at HEAD.
`Decoder::snapshot` now returns `Arc<AssistantMessage>`
([`blocks.rs:116`](../../../crates/cyrup-provider/src/api/anthropic_messages/blocks.rs)) and
**every non-terminal `StreamEvent` variant carries `Arc<AssistantMessage>`**
([`stream.rs:501-575`](../../../crates/cyrup-provider/src/stream.rs)). Consequence for this
file: §8's old "the payload dominates the hop, hand it to PERF-002" finding is **already
delivered** for the provider→sink hop — the item being sent is now a refcount bump, not a 4 KB
rebuild. The finding still applies downstream of `Fanout::emit`, which is PERF-002's own
territory, but it is no longer a PERF-006 hand-off.

**(b) THE §7.2 FRAMER SKETCH IN THE PREVIOUS REVISION WAS WRONG — 11 divergences out of 32
cases.** It was re-derived from the SSE spec rather than read off
[`EventBuilder`](../../../tmp/eventsource-stream-0.2.3/src/event_stream.rs), and it
got the dispatch rule backwards. Measured, not reasoned — see §3.2 for the full table. The
central error: the sketch armed dispatch on *"a field was seen"* (`saw_field`), where the actual
rule is *"a blank line always attempts dispatch; the attempt EMITS only if the data buffer is
non-empty; and it resets the event/data buffers either way."* Under the sketch, `event: ping\n\n`
and `id: 7\n\n` and `retry: 100\n\n` each produce a spurious frame that upstream suppresses, and
a suppressed dispatch leaks its event-type buffer into the next frame. **§4 carries the corrected
algorithm, verified 32/34 identical against upstream.**

**(c) `eventsource-stream` 0.2.3 PANICS on a leading BOM, and the panic is reachable from
`open_sse`.** [`event_stream.rs:266-275`](../../../tmp/eventsource-stream-0.2.3/src/event_stream.rs)
strips the BOM with `&string[1..]` after matching `is_bom` on a `char`. U+FEFF is **three** bytes
in UTF-8, so byte index 1 is not a char boundary:

```
thread 'main' panicked at eventsource-stream-0.2.3/src/event_stream.rs:271:36:
start byte index 1 is not a char boundary; it is inside '\u{feff}' (bytes 0..3 of string)
```

Reproduced by `framer.rs` *(gone — §9.1)*, case
*"BOM at stream start"*. A provider, proxy or CDN that prepends a UTF-8 BOM to
`text/event-stream` kills the turn. The workspace's no-panic policy cannot see it — it is
`clippy::indexing_slicing` **inside a dependency** — and neither can `#![forbid(unsafe_code)]`,
which the same dependency violates in
[`utf8_stream.rs:69`](../../../tmp/eventsource-stream-0.2.3/src/utf8_stream.rs)
(`unsafe { String::from_utf8_unchecked(bytes) }`). The corrected framer handles the BOM at the
byte level and returns the right frames.

**(d) THE QUADRATIC IS ALREADY DISTORTING THE CRATE'S TEST DESIGN — independently rediscovered
by PERF-001.** [`tests/perf001.rs:7-15`](../../../crates/cyrup-provider/src/api/anthropic_messages/tests/perf001.rs)
opens with:

> *"**The decoder is fed `SseFrame`s directly rather than an SSE transcript.** PERF-001's own
> measurements subtract `decode_sse_bytes` measured alone, because handing it a whole transcript
> as one blob is quadratic in the FRAME count … at 512 KB in 40-byte deltas that layer accounts
> for **2.8 s of a 3.1 s drive**"*

That file hand-builds its frames and keeps a separate
`hand_built_frames_match_the_sse_decoder` check purely to pin the hand-built shape back to the
real one. **The workaround exists only because of the dependency.** With the in-tree framer the
whole-buffer regime costs 388 ns/frame instead of 148,700 ns/frame, and the transcript can be fed
directly.

---

## 1. The pipeline: closed, and why — the short version

The original proposal was to split bytes → frame → JSON → fold → snapshot → send into a bounded
3-stage pipeline so decode cost becomes `max(stage)` instead of `sum(stages)`. It is not worth
building. Two facts settle it and neither has moved since they were measured.

**Decode is idle.** Per frame, on the wire path, at HEAD:

```
  A  SSE framing (eventsource-stream, 1 event/chunk)      860 ns
  B  parse_json_with_repair -> owned Value                591 ns
  C  process_event + snapshot (post-PERF-001)             534 ns
  S  sink.send, Arc payload                            ~  211 ns
                                                       ---------
     serial                                            ~ 2196 ns/frame  =  455,000 frames/sec
```

An Anthropic stream delivers 10²–10³ frames/sec. The decode task consumes **~0.2 % of one core**
at 10³ frames/sec. `max(stage)` versus `sum(stages)` is a question about a resource that is
99.8 % idle.

**And the speedup evaporates against a real consumer.** `sink.send` is in series with all three
stages and cannot be pipelined: it is bounded (`STREAM_BUFFER = 64`,
[`collection.rs:28`](../../../crates/cyrup-provider/src/collection.rs),
[`wire.rs:28`](../../../crates/cyrup-provider/src/wire.rs)) and back-pressured through
`Fanout::emit`, which awaits **every** subscriber
([`subscriber.rs:63-76`](../../../crates/cyrup-session-svc/src/subscriber.rs) — *"backpressure →
slows the agent, never drops"*) all the way to the TUI draw. Measured with
`hop.rs` *(gone — §9.1)*, a consumer's per-event cost passes
through to the producer ~1:1:

| consumer cost/event | serial | pipelined | speedup |
| ---: | ---: | ---: | ---: |
| 0 µs (drains freely) | 2.20 µs | 1.07 µs | 2.05× |
| 5 µs | 8.6 µs | 7.5 µs | **1.15×** |
| 20 µs | 23.7 µs | 22.6 µs | **1.05×** |

A TUI frame draw is far more than 5 µs — [PERF-005](PERF-005_DECOUPLE_RENDER_FROM_FOLD.md) exists
precisely because the renderer sits on this path — so the 5–20 µs rows are the operating regime.
Against that, the pipeline costs two channels, an ordering obligation across
**29 `decode_stream(` call sites in 19 files**, three separate cancellation observations, and a
forward-travelling error protocol (four of the five terminal paths in
[`driver.rs`](../../../crates/cyrup-provider/src/api/anthropic_messages/driver.rs) need decoder
state that lives in stage C, while the transport error is detected in stage A, so an error must
travel *down* the pipeline as a value or it races the frames still in flight).

Also note the read-ahead hop §6 of the previous revision proposed **already exists**: reqwest
0.13.4 sits on hyper-util 0.1.20's legacy client, which spawns a background dispatcher task per
connection (`client.rs:552` HTTP/2, `:584` HTTP/1), so `resp.bytes_stream()` is fed over a channel
and performs no `read(2)` on the decode task.

**Nothing in §1 is work.** It is the recorded reason §4 is.

---

## 2. Where it is — citations refreshed against `7913760`

### 2.1 The dependency's entire footprint is ONE `use` line

```
crates/cyrup-provider/src/stream/sse.rs:46
    use eventsource_stream::{Event as EsEvent, EventStreamError, Eventsource};
```

That is the **only** occurrence of `eventsource_stream` in `crates/`. Everything downstream of it
is `SseFrame`, which is cyrup's own type
([`sse.rs:168-175`](../../../crates/cyrup-provider/src/stream/sse.rs), `{event, data}`, deriving
`Clone, Debug, PartialEq, Eq`). Three sites consume the dependency:

| site | what it does |
| --- | --- |
| [`sse.rs:297-298`](../../../crates/cyrup-provider/src/stream/sse.rs) | `type EsInner = Pin<Box<dyn Stream<Item = Result<EsEvent, EventStreamError<reqwest::Error>>> + Send>>` |
| `sse.rs:466` | `let es: EsInner = Box::pin(resp.bytes_stream().eventsource());` |
| `sse.rs:514-526` | `decode_sse_bytes` — `stream::once(one Bytes).eventsource()`, the replay helper |

And two places unwrap its error type:

- `sse.rs:485-503` — the `unfold` arm. `EventStreamError::Transport(inner)` (`:495`) is unwrapped
  by hand because `EventStreamError` has an empty `Error::source`, then passed through
  [`flatten_source_chain`](../../../crates/cyrup-provider/src/stream/sse.rs) (`:317`) and
  `normalize_error_body`.
- `sse.rs:523` — `decode_sse_bytes`'s `ProviderError::Decode(e.to_string())`.

### 2.2 The cancel arm that must survive untouched

[`sse.rs:473-507`](../../../crates/cyrup-provider/src/stream/sse.rs) is a `futures::stream::unfold`
carrying the `CancelToken` in a `tokio::select! { biased; _ = state.cancel.cancelled() => … }`
(`:477-478`), yielding exactly one `ProviderError::Aborted` and then ending. **Cancellation is
solved. §4 replaces what `state.es` *is*, and changes nothing about this loop's shape.**

### 2.3 Who consumes frames

Six SSE decoders, all structurally identical, plus one binary decoder that is not in scope:

| decoder | frame loop |
| --- | --- |
| [`anthropic_messages/driver.rs`](../../../crates/cyrup-provider/src/api/anthropic_messages/driver.rs) | `:45` |
| [`openai_responses/decoder.rs`](../../../crates/cyrup-provider/src/api/openai_responses/decoder.rs) | `:140` |
| [`openai_completions/decode.rs`](../../../crates/cyrup-provider/src/api/openai_completions/decode.rs) | `:42` |
| [`google_generative_ai/driver.rs`](../../../crates/cyrup-provider/src/api/google_generative_ai/driver.rs) | `:35` |
| [`mistral_conversations/driver.rs`](../../../crates/cyrup-provider/src/api/mistral_conversations/driver.rs) | `:36` |
| [`pi_messages.rs`](../../../crates/cyrup-provider/src/api/pi_messages.rs) | `:673` |

`bedrock_converse_stream` speaks AWS `vnd.amazon.eventstream` binary framing, not SSE, and is
untouched by this work.

**None of these change.** They are generic over `Stream<Item = Result<SseFrame, ProviderError>>`
([`driver.rs:22-31`](../../../crates/cyrup-provider/src/api/anthropic_messages/driver.rs)), and
`SseFrame` is unchanged. **The blast radius of §4 is one file plus two manifests.**

### 2.4 The in-tree precedent to copy

[`bedrock_converse_stream/framing.rs:22-45`](../../../crates/cyrup-provider/src/api/bedrock_converse_stream/framing.rs)
is already a hand-rolled incremental wire framer in this crate, with exactly the API shape §4
adopts:

```rust
#[derive(Default)]
pub(super) struct EventStreamDecoder { buffer: Vec<u8> }

impl EventStreamDecoder {
    pub(super) fn push(&mut self, chunk: &[u8]) { … }
    /// Pop the next complete frame, or `Ok(None)` when more bytes are needed.
    pub(super) fn next_frame(&mut self) -> Result<Option<EventFrame>, String> { … }
}
```

Note its no-panic style: `self.buffer.get(..n).ok_or(…)?` everywhere, never `self.buffer[..n]`.
**Match it.** `#![forbid(unsafe_code)]` and the `clippy::indexing_slicing` deny apply.

---

## 3. Measurements at `7913760`

`--release`, 4 runs, medians. Fixture: 128 KB prose + 256 KB tool call = **1,544 KB raw,
9,540 frames**. Reproduce with
`perf006/src/bin/framer.rs` *(gone — §9.1)*.

### 3.1 Framing cost

| input shape | `eventsource-stream` 0.2.3 | in-tree framer (§4) | ratio |
| --- | ---: | ---: | ---: |
| one SSE event per chunk *(what providers send)* | 860 ns/frame | **349 ns** | **2.46×** |
| 8 KB chunks | 1,274 ns/frame | **373 ns** | **3.42×** |
| one whole buffer *(every `decode_sse_bytes` site)* | **148,700 ns/frame** | **388 ns** | **383×** |

Run-to-run spread: upstream 856–875 / 1243–1276 / 147,324–149,418; framer 344–425 / 367–444 /
374–440 (the high end of each is the first, cold run).

**The framer is FLAT across chunk regimes — 349/373/388 — and upstream is not.** That is the
signal that the quadratic is gone by construction, and it is what makes §0d's workaround
unnecessary.

The quadratic's mechanism, for the record: `parse_event` does `buffer.split_off(consumed)` **per
line** ([`event_stream.rs:221-223`](../../../tmp/eventsource-stream-0.2.3/src/event_stream.rs)),
allocating a new `String` and memcpy'ing the whole *remaining* buffer each time; and `Utf8Stream`
re-validates and moves the entire accumulated buffer per chunk
([`utf8_stream.rs:59-70`](../../../tmp/eventsource-stream-0.2.3/src/utf8_stream.rs)).
With one giant buffer both are O(n²) in the stream's byte length.

It is not a production bug — the shipping path is `resp.bytes_stream().eventsource()` and hyper
delivers per-HTTP-chunk — but it is a live cost across the crate's whole offline test surface:
**26 `decode_sse_bytes` call sites in 14 files**, including
[`truncation_parity.rs:196-231`](../../../crates/cyrup-provider/src/api/truncation_parity.rs)'s
`events(wire, raw)`, which drives five wire APIs from one helper.

### 3.2 Behavioural equivalence — 34 cases

`framer.rs` runs every case below through both implementations and compares. **The corrected
framer of §4 is identical on 32; the two remaining differences are both strict improvements.**

<details><summary>The 11 cases where the PREVIOUS revision's sketch diverged (all fixed in §4)</summary>

| case | `eventsource-stream` | old §7.2 sketch |
| --- | --- | --- |
| `event: ping\n\n` | *(no frame)* | `("ping","")` ✗ |
| `id: 7\n\n` | *(no frame)* | `("message","")` ✗ |
| `retry: 100\n\n` | *(no frame)* | `("message","")` ✗ |
| `foo: bar\n\n` | *(no frame)* | `("message","")` ✗ |
| `event: a\ndata:\ndata: x\n\n` | `("a","\nx")` | `("a","x")` ✗ |
| `data\ndata: x\n\n` | `("message","\nx")` | `("message","x")` ✗ |
| `event: a\n\nevent: b\ndata: x\n\n` | `[("b","x")]` | `[("a",""),("b","x")]` ✗ |
| lone-CR terminators, 2 events | `[("a","x"),("b","y")]` | `[]` ✗ |
| CR-only stream, 2 events | 2 frames | `[]` ✗ |
| invalid UTF-8 mid-stream | `Err(UTF8 error)` | `("message","\u{fffd}\u{fffd}")` ✗ |
| BOM mid-stream (2nd chunk) | `[("message","x")]` | `[("message","x"),("message","")]` ✗ |

</details>

The two remaining §4-vs-upstream differences:

| case | `eventsource-stream` 0.2.3 | §4 framer | assessment |
| --- | --- | --- | --- |
| BOM at stream start | **PANIC** (`event_stream.rs:271`) | `[("a","x")]` | **fix** — §0c |
| invalid UTF-8 mid-stream | `Err(UTF8 error: … from index 0)` | `Err(UTF8 error: … from index 6)` | same class, same terminal behaviour; the byte offset differs because §4 validates per line and upstream validates per chunk. Nothing in `crates/` asserts on this text. |

---

## 4. THE WORK — the in-tree SSE framer

Four independent justifications, any one of which would carry it:

1. **2.46× on the wire path, 3.42× at 8 KB, 383× on the replay path** (§3.1). Zero concurrency,
   zero ordering risk, no new failure mode, no signature change anywhere.
2. **It removes a live panic** a provider can trigger (§0c).
3. **It removes `nom` 7.1.3 and `eventsource-stream` 0.2.3 from the dependency graph.** Verified:
   `cargo tree --workspace -i nom` reports `nom → eventsource-stream → cyrup-provider` and nothing
   else. It also removes the graph's `unsafe { String::from_utf8_unchecked }`.
4. **It retires PERF-001's test workaround** (§0d) — the transcript can be fed to the decoder
   directly again.

### 4.1 The algorithm — read off `EventBuilder`, not re-derived

This is the WHATWG *event stream interpretation* algorithm as `eventsource-stream` implements it
([`event_stream.rs:19-113`](../../../tmp/eventsource-stream-0.2.3/src/event_stream.rs)).
Implement exactly these rules. Every one of them is load-bearing for a case in §3.2.

**Line terminators** (`parser.rs:79-85`, `end_of_line`): `\r\n`, lone `\r`, or `\n`.
**A trailing `\r` that is the last byte of the buffer is NOT yet a terminator** — the `\n` may be
in the next chunk. Hold it. At EOF an unterminated line is silently dropped (upstream returns
`Incomplete` forever and the stream just ends).

**Per line:**

| line | action |
| --- | --- |
| empty | **Dispatch.** Take *both* the event-type and data buffers, resetting them **unconditionally**. If the data buffer is empty, emit **nothing** and continue. Otherwise pop **one** trailing `\n`, substitute `"message"` for an empty event type, and yield the frame. |
| starts with `:` | comment — ignore |
| `name: value` | strip **one** leading space from `value`; split on the **first** colon |
| `name` with no colon | `value` is `""` |

**Per field name:**

| name | action |
| --- | --- |
| `event` | **replace** the event-type buffer with `value` |
| `data` | append `value` **then one `\n`**, unconditionally |
| `id`, `retry`, anything else | ignore — `SseFrame` carries neither, and **neither arms dispatch** |

**BOM**: strip a leading U+FEFF (`EF BB BF`) **once, at stream start only**, at the byte level.

**UTF-8**: validate each line; an invalid sequence is a terminal error, not a lossy substitution.
`from_utf8_lossy` is wrong here — it would turn transport corruption into a confusing downstream
JSON parse failure.

The three rules the previous sketch got wrong, restated because they are the whole difference:
*only a blank line dispatches*; *a dispatch emits only if the data buffer is non-empty*; *a
dispatch resets the buffers whether or not it emitted*.

### 4.2 The framer — new file `crates/cyrup-provider/src/stream/framer.rs`

Verified 32/34 against upstream by
`framer.rs` *(gone — §9.1)*. Ship this shape.

```rust
//! Incremental SSE framing — the WHATWG "event stream interpretation" algorithm over a byte
//! buffer with an advancing cursor.
//!
//! Replaces `eventsource-stream` 0.2.3 (see [`super::sse`]). Same observable frames on every
//! shape the crate's fixtures and the six wire APIs produce, minus two upstream defects: a
//! `&string[1..]` BOM strip that panics on a non-char-boundary, and a per-line
//! `String::split_off` + per-chunk whole-buffer `String::from_utf8` that together make framing
//! quadratic in the byte length of a single buffer.

use super::sse::SseFrame;

/// UTF-8 byte order mark, stripped once at stream start (`event_stream.rs:266-275` upstream).
const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

/// Consumed-prefix bytes tolerated before the buffer is compacted. The cursor makes framing
/// O(bytes); this bounds the buffer's *residency* without paying a memmove per line.
const RECLAIM_THRESHOLD: usize = 16 * 1024;

#[derive(Default)]
pub(crate) struct SseFramer {
    buf: Vec<u8>,
    /// Cursor into `buf`; everything before it is consumed.
    start: usize,
    bom_checked: bool,
    /// The spec's "event type buffer".
    event: String,
    /// The spec's "data buffer" — every `data:` line appends its value plus one LF.
    data: String,
}

impl SseFramer {
    pub(crate) fn push(&mut self, chunk: &[u8]) {
        if self.start == self.buf.len() {
            self.buf.clear();
            self.start = 0;
        } else if self.start >= RECLAIM_THRESHOLD {
            self.buf.drain(..self.start);
            self.start = 0;
        }
        self.buf.extend_from_slice(chunk);
    }

    /// The next complete frame, or `Ok(None)` when more bytes are needed.
    pub(crate) fn next_frame(&mut self) -> Result<Option<SseFrame>, std::str::Utf8Error> {
        if !self.bom_checked {
            if self.buf.len() >= BOM.len() {
                self.bom_checked = true;
                if self.buf.starts_with(&BOM) {
                    self.start = BOM.len();
                }
            } else if self.buf.iter().any(|b| *b == b'\n' || *b == b'\r') {
                // A complete line shorter than the BOM cannot begin with one.
                self.bom_checked = true;
            } else {
                return Ok(None);
            }
        }
        loop {
            let rest = self.buf.get(self.start..).unwrap_or(&[]);
            let Some(pos) = rest.iter().position(|b| *b == b'\n' || *b == b'\r') else {
                return Ok(None);
            };
            let term_len = match rest.get(pos) {
                Some(b'\n') => 1,
                Some(b'\r') => match rest.get(pos + 1) {
                    Some(b'\n') => 2,
                    Some(_) => 1,
                    // A trailing CR may still become CRLF; upstream reports `Incomplete` here.
                    None => return Ok(None),
                },
                _ => return Ok(None),
            };
            let line_at = self.start;
            self.start += pos + term_len;
            let line =
                std::str::from_utf8(self.buf.get(line_at..line_at + pos).unwrap_or(&[]))?;

            if line.is_empty() {
                // Dispatch. Both buffers reset EVEN IF nothing is emitted (upstream's
                // `core::mem::take(self)` in `EventBuilder::dispatch`).
                let mut data = std::mem::take(&mut self.data);
                let event = std::mem::take(&mut self.event);
                if data.is_empty() {
                    continue;
                }
                if data.ends_with('\n') {
                    data.pop();
                }
                return Ok(Some(SseFrame {
                    event: if event.is_empty() { "message".to_string() } else { event },
                    data,
                }));
            }
            if line.starts_with(':') {
                continue; // comment
            }
            let (name, value) = match line.split_once(':') {
                Some((n, v)) => (n, v.strip_prefix(' ').unwrap_or(v)),
                None => (line, ""),
            };
            match name {
                "event" => {
                    self.event.clear();
                    self.event.push_str(value);
                }
                "data" => {
                    self.data.push_str(value);
                    self.data.push('\n');
                }
                // `id:` and `retry:` are spec fields that `SseFrame` does not carry. Neither
                // arms dispatch: only a blank line dispatches, and only non-empty data emits.
                _ => {}
            }
        }
    }
}
```

Notes for whoever types it in:

- **No indexing, no `unwrap`, no `expect`, no `panic`.** `self.buf.get(..)`, `rest.get(..)`,
  `.unwrap_or(&[])`, `.strip_prefix(' ').unwrap_or(v)` — `Option::unwrap_or` is not
  `clippy::unwrap_used`. This compiles clean under the workspace lints with no `#![allow]`.
- **`data.ends_with('\n')` before `pop()`** rather than upstream's
  `is_lf(data.chars().next_back().unwrap())`. Same behaviour, no panic path. Do **not** re-check
  emptiness after the pop — `event: ping\ndata:\n\n` must still yield `("ping","")`.
- **The borrows are field-disjoint.** `line` borrows `self.buf`; the mutations touch
  `self.event`/`self.data`/`self.start`. This is fine *as long as no `&mut self` method is
  called while `line` is alive* — keep the body inline and do not factor the per-line work into a
  helper method.
- `std::str::from_utf8` on the whole line (not just the value) is deliberate: it matches
  upstream's "any invalid byte is an error" and is simpler than validating field and value
  separately. It costs nothing measurable — the framer is still 349 ns/frame.

### 4.3 The stream adapter — same file

```rust
/// A framing error: either the byte source failed, or the stream was not UTF-8.
pub(crate) enum FrameError<E> {
    Transport(E),
    Utf8(std::str::Utf8Error),
}

/// Frame a byte stream. `Send`-safe and cancellation-agnostic — the caller keeps its own
/// cancel arm (see [`super::sse::open_sse`]).
pub(crate) fn frame_bytes<S, B, E>(
    inner: S,
) -> impl futures::Stream<Item = Result<SseFrame, FrameError<E>>> + Send
where
    S: futures::Stream<Item = Result<B, E>> + Send + 'static,
    B: AsRef<[u8]> + Send,
    E: Send,
{
    struct St<S> {
        inner: std::pin::Pin<Box<S>>,
        framer: SseFramer,
        done: bool,
    }
    let st = St { inner: Box::pin(inner), framer: SseFramer::default(), done: false };
    futures::stream::unfold(st, |mut st| async move {
        loop {
            if st.done {
                return None;
            }
            match st.framer.next_frame() {
                Ok(Some(frame)) => return Some((Ok(frame), st)),
                Err(e) => {
                    st.done = true;
                    return Some((Err(FrameError::Utf8(e)), st));
                }
                Ok(None) => {}
            }
            match futures::StreamExt::next(&mut st.inner).await {
                Some(Ok(bytes)) => st.framer.push(bytes.as_ref()),
                Some(Err(e)) => {
                    st.done = true;
                    return Some((Err(FrameError::Transport(e)), st));
                }
                // EOF. An unterminated trailing line is dropped, as upstream drops it.
                None => return None,
            }
        }
    })
}
```

### 4.4 The wiring in `sse.rs` — a five-line diff

Register the module (`crates/cyrup-provider/src/stream.rs:10` currently reads `pub mod sse;`):

```rust
mod framer;
pub mod sse;
```

Then in [`sse.rs`](../../../crates/cyrup-provider/src/stream/sse.rs):

1. **`:46`** — delete `use eventsource_stream::{Event as EsEvent, EventStreamError, Eventsource};`
   and add `use super::framer::{FrameError, frame_bytes};`.
2. **`:297-298`** — retype `EsInner`. It now already carries `SseFrame`, so the unfold's `Ok`
   arm loses its field-by-field rebuild:
   ```rust
   type EsInner =
       Pin<Box<dyn Stream<Item = Result<SseFrame, FrameError<reqwest::Error>>> + Send>>;
   ```
3. **`:466`** — `let es: EsInner = Box::pin(frame_bytes(resp.bytes_stream()));`
4. **`:481-503`** — inside the existing `unfold`, keep the `biased` cancel arm **byte-for-byte**;
   change only the two data arms:
   ```rust
   Some(Ok(frame)) => Some((Ok(frame), state)),
   Some(Err(e)) => {
       state.done = true;
       let text = match &e {
           // `flatten_source_chain` is still required: reqwest's `Display` is terse and the
           // real reason ("operation timed out") lives only in `Error::source`. That string is
           // the sole input to `is_retryable_assistant_error`.
           FrameError::Transport(inner) => {
               format!("Transport error: {}", flatten_source_chain(inner))
           }
           FrameError::Utf8(inner) => format!("UTF8 error: {inner}"),
       };
       Some((Err(ProviderError::Decode(normalize_error_body(&text))), state))
   }
   ```
   The `"Transport error: "` prefix and the `flatten_source_chain` call **must** survive —
   `a_stall_mid_stream_ends_the_stream_with_an_error` (`sse.rs:775`) asserts the resulting message
   contains `"timed out"`. `normalize_error_body` also stays: a `FrameError::Utf8` message is
   short, but the cap is what keeps provider-controlled bytes out of
   `AssistantMessage.error_message`.
5. **`:514-526`** — `decode_sse_bytes` keeps its **exact** signature and return type (26 call
   sites depend on it):
   ```rust
   pub fn decode_sse_bytes(bytes: impl Into<Bytes>) -> FrameStream {
       let bytes = bytes.into();
       let byte_stream = futures::stream::once(async move { Ok::<Bytes, std::io::Error>(bytes) });
       Box::pin(frame_bytes(byte_stream).map(|r| {
           r.map_err(|e| match e {
               FrameError::Transport(io) => ProviderError::Decode(io.to_string()),
               FrameError::Utf8(inner) => ProviderError::Decode(format!("UTF8 error: {inner}")),
           })
       }))
   }
   ```

Nothing else in the crate changes. The six decoders, all 29 `decode_stream(` call sites, and
`SseFrame` itself are untouched.

### 4.5 Drop the dependency

- [`Cargo.toml:205`](../../../Cargo.toml) — remove `eventsource-stream = { version = "0.2.3" }`
  from `[workspace.dependencies]`. Per the workspace convention (justifying comments on every
  entry), replace it with a one-line note recording that SSE framing moved in-tree and why, in
  the style of the neighbouring `reqwest` entry.
- [`crates/cyrup-provider/Cargo.toml:67`](../../../crates/cyrup-provider/Cargo.toml) — remove
  `eventsource-stream = { workspace = true }`.
- `Cargo.lock` will drop `eventsource-stream`, `nom`, and `minimal-lexical`. `memchr`,
  `futures-core` and `pin-project-lite` stay — they have other consumers.

### 4.6 Provenance — one real obligation and one thing to ASK about

`sse.rs:1-2` opens:

```rust
//! Direct-wire HTTP + SSE transport (arch-01 §7.1: `reqwest` + `rustls`, no native-tls, +
//! `eventsource-stream`).
```

**`spec/` is not in this workspace** (see `CLAUDE.md`), so *"arch-01 §7.1 names
`eventsource-stream`"* cannot be verified here and must not be treated as settled either way.
Two things follow:

- **Do**: rewrite that line to name the in-tree framer and state the delta with its reason —
  "framing moved in-tree; upstream's BOM strip panics and its per-line `split_off` is quadratic;
  frames are equivalent on 32/34 probed shapes" — the same way the `[CYRUP-DELTA]` paragraph at
  `sse.rs:27-36` states the `read_timeout` substitution. Doc comments carry port provenance in
  this repo; extend, never drop.
- **Ask, do not decide**: whether swapping a spec-named dependency needs the spec amended. Surface
  it; do not fabricate spec text to justify it, and do not let its absence block the change.

---

## 5. Explicitly out of scope

- **The three-stage pipeline.** §1. Closed.
- **Borrowed typed deserialization for stage B.** Measured at 1.16× on B = **1.02× end-to-end**,
  and it would break `driver.rs:190`'s `_ => true` — an unrecognised event `type` is currently a
  silent no-op, but a `#[serde(tag = "type")]` enum *fails* on an unknown tag, turning every future
  Anthropic event type into a parse error. A behaviour change disguised as an optimisation.
  Declined.
- **simd-json / sonic-rs.** Per-frame JSON is ~160 bytes; stage B is 591 ns of a ~2,200 ns budget.
  Bounded above by <1.4× end-to-end even if parsing were free.
- **Parallelising across frames.** The decoder is a state machine over an ordered event sequence.
- **Touching the idle-timeout policy** at
  [`sse.rs:125-133`](../../../crates/cyrup-provider/src/stream/sse.rs). Correct, and the reasoning
  — including why a total-request `timeout` would be wrong — is at `sse.rs:27-36`.
- **Re-implementing read-ahead.** hyper already does it (§1).
- **Changing `SseFrame`** to carry `id`/`retry`. Upstream parses them; cyrup discards them; no
  consumer wants them. Ignoring them is the parity-preserving choice.
- **"Fixing" the quadratic by chunking inside `decode_sse_bytes`.** That masks the artifact instead
  of removing it, and §4 removes it as a side effect.
- **`bedrock_converse_stream`.** Binary framing, not SSE.
- **Reworking `perf001.rs` to stop hand-building frames.** §4 makes it unnecessary, not wrong. A
  separate call.

---

## 6. Definition of done

1. `crates/cyrup-provider/src/stream/framer.rs` exists, carrying `SseFramer` (§4.2), `FrameError`
   and `frame_bytes` (§4.3), and is registered from `crates/cyrup-provider/src/stream.rs`.
2. `crates/cyrup-provider/src/stream/sse.rs` contains **no** reference to `eventsource_stream`.
   `EsInner`, `open_sse`'s `es` binding, the `unfold`'s two data arms and `decode_sse_bytes` are
   rewired per §4.4. `decode_sse_bytes`'s signature and return type are byte-identical to today's.
3. The `biased` `CancelToken` arm at `sse.rs:477-478` is unchanged, and the transport error path
   still goes through `flatten_source_chain` + `normalize_error_body` and still starts with
   `"Transport error: "`.
4. `eventsource-stream` is gone from `Cargo.toml` and `crates/cyrup-provider/Cargo.toml`, and
   `cargo tree --workspace -i nom` reports nothing.
5. The module doc comment at `sse.rs:1-2` names the in-tree framer and states the delta with its
   reason (§4.6). The spec question is raised with the user, not answered by the agent.
6. The workspace gate is green — check `df -h /` and `df -h /tmp` first, and confirm no other
   session is building:
   ```bash
   cargo test --workspace --features test-fixtures --no-fail-fast
   cargo clippy --workspace --all-targets --features test-fixtures; echo "exit=$?"   # MUST be 0
   ```
   `truncation_parity.rs`, `sse.rs`'s own module, and all six decoders' fixture suites replay
   through `decode_sse_bytes`, so they are the existing behavioural check on §4 and must pass
   unchanged — no fixture, no expected value and no assertion may be edited to accommodate the new
   framer. If one needs editing, the framer is wrong.
7. Nothing new is added under `crates/` beyond `framer.rs`. `tmp/` at any level is gitignored
   ([`.gitignore:7`](../../../.gitignore) — the rule is `tmp/`, not `/tmp/`), so the re-vendored
   `tmp/eventsource-stream-0.2.3/` cannot enter the commit; confirm with `git status --short`
   that it does not appear.

---

## 9. [AUG-5] Re-verification @ main `b7a564d`, on a fresh clone — READ BEFORE §0

The previous augmentation was written at `7913760` on branch **`david/performance`**. This pass
re-checked **every** citation against **`main` at `b7a564d`** (PERF-003, PERF-004 and PERF-005 have
landed since) in a **freshly cloned container**. The headline: **the plan in §4 is sound and its
cyrup-side citations are exact — but the evidence base in §3 is not reproducible here, and two
upstream line numbers were wrong.**

### 9.1 🔴 The probe tree is GONE — §3's numbers cannot be re-run as written

`.gitignore:7` is `tmp/` — **unanchored**, so it ignores a `tmp` directory *at any depth*, including
`crates/cyrup/tmp/`. That tree was scratch on a branch, never committed, and this container is a
fresh clone. All of it is absent:

| cited artifact | status |
| --- | --- |
| `perf006/src/bin/framer.rs` — the 34-case equivalence harness **and** the BOM-panic reproducer | **gone** |
| `perf006/src/bin/hop.rs` — the §1 consumer-cost table | **gone** |
| `crates/cyrup/tmp/eventsource-stream-0.2.3/` — vendored upstream, target of 7 citations | **gone** |

Upstream has been **re-vendored** during this pass, so every `event_stream.rs` / `utf8_stream.rs` /
`parser.rs` citation in this file is live again and was re-verified below. Re-create it with:

```bash
cp -r ~/.cargo/registry/src/index.crates.io-*/eventsource-stream-0.2.3 tmp/
```

**What this means for `/exec` — do NOT rebuild the probe harness.** §3's measurements stand as
*historical evidence for why this work was chosen*; they are not a deliverable and re-deriving them
is benchmark work this task does not want. The **equivalence gate is the one already named in DoD
item 6**: `truncation_parity.rs`, `sse.rs`'s own module tests, and all six decoders' fixture suites
replay through `decode_sse_bytes`, and they must pass **unedited**. That is the real check on §4,
and it is stronger than a throwaway probe because it runs in CI forever.

The one thing genuinely lost is the **BOM-panic reproducer** (§0c). The panic is still provable by
reading upstream — re-verified verbatim at `event_stream.rs:266-275` in §9.3 below — so §0c's claim
stands on the source, not on a probe.

### 9.2 Cyrup-side citations: ALL EXACT at `b7a564d`, no edits needed

Re-checked individually, not spot-checked:

| citation | verified |
| --- | --- |
| `sse.rs:46` — the single `use eventsource_stream::…` line | ✅ exact; still the **only** occurrence in `crates/` |
| `sse.rs:168-175` — `SseFrame { event: String, data: String }`, `Clone/Debug/PartialEq/Eq` | ✅ exact — confirms §4.2's struct literal compiles |
| `sse.rs:295` — `type FrameStream = Pin<Box<dyn Stream<Item = Result<SseFrame, ProviderError>> + Send>>` | ✅ exact — §4.4 item 5's return type is right |
| `sse.rs:297` — `type EsInner` | ✅ exact |
| `sse.rs:317` — `fn flatten_source_chain` | ✅ exact |
| `sse.rs:466` — `Box::pin(resp.bytes_stream().eventsource())` | ✅ exact |
| `sse.rs:478` — `biased;` in the cancel arm | ✅ exact (cited as `:477-478`; the `tokio::select!` opens at `:477`) |
| `sse.rs:514` — `pub fn decode_sse_bytes(bytes: impl Into<Bytes>) -> FrameStream` | ✅ exact |
| `sse.rs:775` — `a_stall_mid_stream_ends_the_stream_with_an_error` | ✅ exact; its `contains("timed out")` assertion is at `:808` |
| `stream.rs:10` — `pub mod sse;` | ✅ exact — §4.4's two-line registration applies cleanly |
| all six decoder frame loops (§2.3) | ✅ all six are `while let Some(frame) = frames.next().await {` at the cited lines |
| `bedrock_converse_stream/framing.rs:22-45` | ✅ exact — `#[derive(Default)] pub(super) struct EventStreamDecoder { buffer: Vec<u8> }` at `:28-31` |
| `Cargo.toml:205`, `crates/cyrup-provider/Cargo.toml:67` | ✅ both exact |
| `tests/perf001.rs:7-15` | ✅ exact, quoted text matches verbatim |

**`cargo tree --workspace -i nom` re-run at `b7a564d`:**

```
nom v7.1.3
└── eventsource-stream v0.2.3
    └── cyrup-provider v0.0.0
```

§4's claim 3 is **confirmed**: `eventsource-stream` is `nom`'s only path into the tree.

### 9.3 Upstream citations: 2 corrected, and the load-bearing algorithm re-confirmed

Re-read against the re-vendored copy. **Corrected in place above:**

| was | is | note |
| --- | --- | --- |
| `parser.rs:216` (`end_of_line`) | **`parser.rs:79-85`** | `parser.rs` is only **118 lines**; `:216` cannot exist. The *rule* was right: `alt((crlf, take_while_m_n(1,1,is_cr), take_while_m_n(1,1,is_lf)))` |
| `event_stream.rs:24-110` (`EventBuilder`) | **`:19-113`** | `struct` opens at `:19`, `impl` at `:24`, `add` at `:45`, `dispatch` at `:95-113` |

**The three rules §0b says the previous sketch got backwards were re-verified against the source,
because they are the entire difference between a correct framer and an 11/32-wrong one:**

1. *Only a blank line dispatches* — `RawEventLine::Empty => self.is_complete = true`
   (`event_stream.rs:71`). `id` (`:57`) and `retry` (`:62`) parse their values and set
   `is_complete` **never**. ✅
2. *A dispatch resets both buffers even when it emits nothing* — `core::mem::take(self)` at `:96`
   runs **before** the emptiness check at `:100-102`. ✅
3. *A dispatch emits only if the data buffer is non-empty* — `if event.data.is_empty() { return
   None; }` at `:100-102`. ✅

Also re-confirmed, each backing a specific §4.1 row: `event` **replaces** the type buffer
(`:51`, `self.event.event = val.to_string()`); `data` appends value **plus one `\u{000A}`**
unconditionally (`:53-55`); one trailing LF is popped (`:104-106`); an empty type becomes
`"message"` (`:108-110`).

§4.2's deliberate divergence — `data.ends_with('\n')` instead of upstream's
`is_lf(data.chars().next_back().unwrap())` (`:104`) — is **correct and strictly better**: identical
behaviour, one fewer panic path. Upstream's `unwrap` is guarded by `:100`, so it is not itself a
live bug; §4.2's version simply does not need the guard. And §4.2's warning not to re-check
emptiness after the pop is **right** — traced through upstream, `event: ping\ndata:\n\n` gives
`data = "\n"` → passes `:100` → pops to `""` → emits `("ping","")`.

**The two upstream defects are re-confirmed verbatim:**

- **The BOM panic** (`event_stream.rs:266-275`): `if is_bom(string.chars().next().unwrap()) {
  &string[1..] }` at `:270-271` — `is_bom` matches a **`char`**, then the slice indexes by
  **byte**. U+FEFF is 3 bytes in UTF-8, so `&string[1..]` splits it and panics.
- **The quadratic** (`event_stream.rs:221-223`): `let rem = buffer.split_off(consumed); *buffer =
  rem;` **per line** — a fresh `String` plus a memcpy of the whole remainder each time. Compounded
  by `utf8_stream.rs:59-70`, which `core::mem::take`s and re-validates the entire accumulated
  buffer **per chunk**, and whose `:69` is the graph's
  `unsafe { String::from_utf8_unchecked(bytes) }`.

### 9.4 Two call-site counts were wrong — corrected in place

Re-counted at `b7a564d`:

| claim | was | is |
| --- | --- | --- |
| `decode_sse_bytes` call sites | 30 in 14 files | **26 in 14 files** |
| `decode_stream(` call sites | 27 in 18 files | **29 in 19 files** |

Neither changes any conclusion — none of these sites are edited, which is precisely §2.3's point —
but DoD item 2 leans on the first as the reason `decode_sse_bytes`'s signature is frozen, so it
should be right.

### 9.5 Net assessment for `/exec`

**The §4 plan is unchanged and ready to type in.** Nothing in this pass moved the design; §4.1's
algorithm, §4.2's framer, §4.3's adapter and §4.4's five-line rewiring all survive re-verification
against both the current tree and the re-vendored upstream.

Three things to carry into exec:

1. **Do not chase §3's numbers.** The probes are gone (§9.1); the fixture suites are the gate.
2. **`SseFramer::next_frame` is not sticky on a UTF-8 error** — it advances `self.start` past the
   bad line *before* the `?`. That is safe **only** because `frame_bytes` sets `done = true` on
   `Err` (§4.3) and never polls again. Keep that arm exactly as §4.3 writes it; if the framer is
   ever driven directly, a second call would silently resume past the corruption.
3. **§4.6's spec question still needs a human.** `spec/` is not in this workspace, so whether
   swapping a spec-named dependency requires an arch-01 amendment cannot be settled here. Surface
   it; do not fabricate spec text, and do not let it block the change.
