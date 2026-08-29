---
stage: aug
status: done
updated: 2026-08-29 02:44
---

# Stop rebuilding the whole assistant message on every stream delta

> **Measured, not inferred.** On this host, `--release`: a 256 KB tool call costs
> **10.06 s** of CPU where one parse at the end costs **3.49 ms** — a **2,883×**
> overhead. The probe is reproduced in §6 so the number can be re-checked before and
> after. Nothing about this is theoretical.

---

## 0. READ THIS FIRST — three things that will send you the wrong way

**(a) This is not a `parse_streaming_json` bug.** The tolerant parser is a faithful,
correct port of pi's `partial-json` delegation
([`json-parse.ts:104-124`](../../../../pi/packages/ai/src/utils/json-parse.ts)). It is
*called wrongly*: on the entire accumulated buffer, once per delta. Do not "optimise the
parser" — fix the call pattern. A 10× faster parser still leaves a 288× overhead.

**(b) `snapshot()` cannot simply be deleted.** `partial` is pi's own contract — every
non-terminal `StreamEvent` carries the running `AssistantMessage`
([`agent-loop.ts:313-340`](../../../../pi/packages/agent/src/agent-loop.ts)), and
[`agent/run/stream.rs:184-187`](../../../crates/cyrup-agent/src/agent/run/stream.rs)
depends on it (`partial = p.clone()`). The work is to make producing it cheap, not to stop
producing it.

**(c) The cost is NOT confined to `anthropic_messages`.** Seven decoders define their own
`snapshot`, and there are **90** `snapshot(model, api)` call sites across them:

| decoder | call sites |
| --- | --- |
| [`bedrock_converse_stream/`](../../../crates/cyrup-provider/src/api/bedrock_converse_stream) | 23 (`driver.rs` 13, `events.rs` 10) |
| [`pi_messages.rs`](../../../crates/cyrup-provider/src/api/pi_messages.rs) | 12 |
| [`openai_responses/`](../../../crates/cyrup-provider/src/api/openai_responses) | 17 (`events.rs` 11, `slots.rs` 3, `decoder.rs` 2, `errors.rs` 1) |
| [`google_generative_ai/`](../../../crates/cyrup-provider/src/api/google_generative_ai) | 11 (`parts.rs` 7, `finish.rs` 2, `driver.rs` 2) |
| [`mistral_conversations/`](../../../crates/cyrup-provider/src/api/mistral_conversations) | 11 |
| [`openai_completions/`](../../../crates/cyrup-provider/src/api/openai_completions) | 8 |
| [`anthropic_messages/`](../../../crates/cyrup-provider/src/api/anthropic_messages) | 8 (`events.rs` 6, `driver.rs` 2) |

A fix applied only to the Anthropic path leaves the majority of providers unfixed.

**(d) Every citation in this file was re-verified against `761dc19` (branch
`david/performance`) during augmentation.** The 90-call-site table is exact — confirmed
per file: `bedrock` 23 (`driver.rs` 13 @ lines 64–305, `events.rs` 10 @ 67–302),
`pi_messages` 12 (@ 739–945), `openai_responses` 17 (`events.rs` 11, `slots.rs` 3,
`decoder.rs` 2, `errors.rs` 1), `google_generative_ai` 11 (`parts.rs` 7, `finish.rs` 2,
`driver.rs` 2), `mistral_conversations` 11, `openai_completions` 8, `anthropic_messages` 8
(`events.rs` 6 @ 147/160/176/207/251/266, `driver.rs` 2). `snapshot` and
`blocks_to_content` sit at [`blocks.rs:68`/`:94`](../../../crates/cyrup-provider/src/api/anthropic_messages/blocks.rs);
the `stop_reason`/`timestamp`/`apply_cost` lines are `:85`/`:89`/`:70`. Two line numbers
in the sections below drifted by one or two and are corrected inline: the `Fanout` sends
are at [`subscriber.rs:68,72`](../../../crates/cyrup-session-svc/src/subscriber.rs) (not
67,71), and the three tolerant passes are at
[`json_parse.rs:173/176/179`](../../../crates/cyrup-provider/src/utils/json_parse.rs).
Everything else — `parse_streaming_json_object` exported at
[`lib.rs:167`](../../../crates/cyrup-provider/src/lib.rs) (so the §6 probe compiles as
written), the `Vec<char>` materialisations at `json_parse.rs:48`/`:199`, and the
`events_fold.rs:502` doc comment — is accurate as filed.

---

## 1. What actually happens per delta

Take one `input_json_delta` frame at
[`anthropic_messages/events.rs:168-181`](../../../crates/cyrup-provider/src/api/anthropic_messages/events.rs):

```rust
        Some("input_json_delta") => {
            let text = delta.get("partial_json").and_then(Value::as_str).unwrap_or("");
            if let Some(Block::Tool { partial_json, .. }) = dec.blocks.get_mut(pos) {
                partial_json.push_str(text);          // append ~40 bytes
            }
            let partial = dec.snapshot(model, api);   // <-- rebuild EVERYTHING
```

`snapshot` ([`blocks.rs:68-91`](../../../crates/cyrup-provider/src/api/anthropic_messages/blocks.rs))
calls `blocks_to_content` (`:94-121`), which for **every** block in the message:

- clones the whole accumulated `text` / `thinking` `String`, and
- for a tool block, calls `parse_streaming_json_object(Some(partial_json))` on the
  **entire accumulated** buffer (`:121`).

That bottoms out in
[`utils/json_parse.rs`](../../../crates/cyrup-provider/src/utils/json_parse.rs), which is
where the constant factor comes from:

```rust
pub fn repair_json(json: &str) -> String {
    let chars: Vec<char> = json.chars().collect();   // :48 — 4 bytes/char, WHOLE buffer
```
```rust
fn parse_partial(input: &str) -> Option<Value> {
    let chars: Vec<char> = input.chars().collect();  // :199 — again
```

and `parse_streaming_json` (`:167-183`) makes **up to three passes**:

```rust
    if let Some(v) = parse_json_with_repair(trimmed) { return v; }   // pass 1 (repair → collect)
    if let Some(v) = parse_partial(trimmed) { return v; }            // pass 2 (collect)
    if let Some(v) = parse_partial(&repair_json(trimmed)) { return v; }  // pass 3 (collect ×2)
```

A *truncated* buffer — which is every mid-stream delta, i.e. the overwhelmingly common
case — **always fails pass 1** and therefore always pays at least two full `Vec<char>`
materialisations of everything received so far.

### The message is then copied four more times

`snapshot`'s output does not stop there. Per delta, downstream:

| # | site | copy |
| --- | --- | --- |
| 1 | `events.rs:176` | `snapshot()` — build (the expensive one) |
| 2 | [`agent/run/stream.rs:187`](../../../crates/cyrup-agent/src/agent/run/stream.rs) | `partial = p.clone()` |
| 3 | `agent/run/stream.rs:216` | `AgentMessage::Assistant(partial.clone())` into `MessageUpdate` |
| 4 | `agent/run/stream.rs:217` | `assistant_message_event: Box::new(e.clone())` — clones the event *including its own `partial`* |
| 5 | [`session-svc/subscriber.rs:68,72`](../../../crates/cyrup-session-svc/src/subscriber.rs) | `ev.clone()` per live subscriber |

**And the primary consumer throws all of it away.** The TUI's fold reads only `delta`,
and says so in its own doc comment
([`events_fold.rs:501-503`](../../../crates/cyrup-tui/src/app/events_fold.rs)):

> The remaining non-text frames (start/text-start/text-end/thinking-start/thinking-end/
> toolcall\*) carry only the running `partial`, whose content already reaches us via the
> deltas + the terminal, so nothing is rendered for them.

---

## 2. The measurements

`--release`, this host, 8 cores. Tool-call arguments are a realistic `write` payload;
chunk size 40 bytes, representative of Anthropic `input_json_delta`.

**Single call, by buffer size** — establishes that one parse is linear and cheap:

| buffer | per call |
| --- | --- |
| 1 KB | 15.0 µs |
| 4 KB | 51.2 µs |
| 16 KB | 194.2 µs |
| 64 KB | 764.8 µs |
| 256 KB | 3.131 ms |

**Streamed, re-parsing the accumulated buffer per delta** — what the code does today:

| tool call | deltas | **total** | one parse at end | overhead |
| --- | --- | --- | --- | --- |
| 1 KB | 26 | 225.7 µs | 17.5 µs | 13× |
| 4 KB | 103 | 2.715 ms | 56.3 µs | **48×** |
| 16 KB | 410 | 40.20 ms | 192.0 µs | **209×** |
| 64 KB | 1,639 | **625.7 ms** | 795.6 µs | **786×** |
| 256 KB | 6,554 | **10.058 s** | 3.488 ms | **2,883×** |

**Text/thinking path** — the same shape, cheaper because it is `memcpy` not parse, but it
runs on *every prose token of every response*:

| response | deltas | total | memcpy |
| --- | --- | --- | --- |
| 4 KB | 820 | 197.6 µs | 1.6 MB |
| 16 KB | 3,277 | 1.318 ms | 25.6 MB |
| 64 KB | 13,108 | 24.74 ms | 409.6 MB |
| 128 KB | 26,215 | **486.5 ms** | 1.64 GB |

Note the per-delta cost jumping 1.887 µs → 18.558 µs between 64 KB and 128 KB: that is
allocator pressure and cache-miss behaviour on top of the quadratic, so the curve is worse
than `O(N·k)` at realistic sizes.

---

## 3. What must not change

`snapshot()` is a **pure function of decoder state**. That is the property the whole fix
rests on, and it must survive: for identical `Decoder` contents, the returned
`AssistantMessage` must be byte-identical to today's, including

- `stop_reason: self.stop_reason.unwrap_or(StopReason::Pending)` — the in-flight `Pending`
  seed, which the terminal path deliberately never takes
  ([`blocks.rs:80-86`](../../../crates/cyrup-provider/src/api/anthropic_messages/blocks.rs));
- `timestamp: now_millis()` — **this one is NOT pure.** It is re-read per call. Any cache
  must still stamp a fresh timestamp on each emission, or `partial.timestamp` freezes for
  the duration of a block. Cache the *content*, not the whole message.
- `apply_cost(&model.cost, &mut usage)` — recomputed per call from `self.usage`, which a
  `message_delta` mutates mid-stream.

The salvage semantics of `parse_streaming_json` must also be preserved exactly: a truncated
`{"path":"a.` still yields `{"path": "a."}`, per the existing test at
[`proxy/builder.rs`'s `rebuilds_tool_call_args_from_streaming_json`](../../../crates/cyrup-agent/src/proxy/builder.rs)
and the salvage tests at
[`json_parse.rs:467-480`](../../../crates/cyrup-provider/src/utils/json_parse.rs).

---

## 4. Required implementation

Three layers. Land them in order; each is independently correct and independently
measurable, and **layer A alone captures most of the win**.

### 4a. Memoise the per-block content projection (do this first)

Give each decoder's `Decoder` a content cache keyed on what the blocks actually are, so a
`text_delta` stops re-parsing tool JSON and a second `input_json_delta` against block 0
stops re-cloning block 1's prose.

Add to each `Block` variant a monotonically-bumped `revision: u64`, incremented by the one
site that mutates it (`push_str` on `text` / `thinking` / `partial_json`, and the
signature/`redacted` setters). Then:

```rust
/// Per-block memo of `blocks_to_content`'s output. Keyed on `revision`, which the block's
/// only mutator bumps — so a delta against block *i* invalidates block *i* and nothing
/// else. Cuts the per-delta cost from "rebuild every block" to "rebuild one block".
struct ContentCache {
    entries: Vec<(u64, Content)>,
}
```

`blocks_to_content` becomes a `&mut self` method that walks `self.blocks`, reuses the
cached `Content` where `revision` matches, and recomputes only the misses. `snapshot`
still constructs a fresh `AssistantMessage` around it (fresh `timestamp`, fresh
`apply_cost`), so §3 holds.

**Exact mutation sites in `anthropic_messages` (the reference decoder).** The `revision`
bump must live at every point a `Block`'s content changes, and there are exactly four in
[`events.rs`](../../../crates/cyrup-provider/src/api/anthropic_messages/events.rs):
`text.push_str` (`text_delta`), `thinking.push_str` (`thinking_delta`),
`partial_json.push_str` (`input_json_delta`, `:174`), and `signature.push_str`
(`signature_delta`) — plus the `redacted` setter on the `Thinking` variant if a
`redacted_thinking` block sets it. Because
[`Block`](../../../crates/cyrup-provider/src/api/anthropic_messages/blocks.rs) is a bare
`enum` today, add the counter as a field on **each** variant and bump it in the same
`if let Some(Block::… { .. }) = dec.blocks.get_mut(pos)` arm that already does the
`push_str`; that arm is the *only* writer, which is what makes the memo sound. A new block
created by `content_block_start` gets `revision: 0` and no cache entry, so its first
`snapshot` is a guaranteed miss — correct.

**This alone removes the cross-block waste** — the dominant term whenever more than one
content block is live, which is every response with thinking plus text, or text plus a
tool call.

### 4b. Make the tool-argument parse incremental

The remaining cost is one block's own buffer being re-parsed from byte 0 on each of its
own deltas. Both `repair_json` and `parse_partial` are strictly **left-to-right**
single-pass scanners over `chars`, so they are naturally resumable.

Give the tool block a resumable parser state:

```rust
/// Resumable state for the tolerant tool-argument parse. `parse_partial` is a
/// left-to-right recursive-descent scanner with no lookbehind, so a delta can be fed as a
/// SUFFIX rather than re-scanning the whole buffer. `consumed` is the char index the last
/// feed stopped at; `stack` is the open-container path at that point.
struct StreamingArgs {
    consumed: usize,
    stack: Vec<PartialFrame>,
    /// The value recovered so far — what `parse_streaming_json_object` would have
    /// returned for `partial_json[..consumed]`.
    value: Map<String, Value>,
}
```

Two hard constraints:

- **`repair_json` is not trivially resumable across a chunk boundary.** It tracks
  `in_string` and can look one or four characters ahead (`hex4_at`,
  [`json_parse.rs:26-32`](../../../crates/cyrup-provider/src/utils/json_parse.rs)). A
  `\uXX` split across two deltas must not be mis-repaired. Carry the trailing incomplete
  escape (at most 5 chars) into the next feed rather than committing it.
- **The three-pass fallback (`:173-181`) must still be reachable.** The incremental path
  is an optimisation of passes 2/3; when it cannot make progress, fall back to today's
  whole-buffer call. Correctness first — a fallback that fires often is a perf bug, not a
  correctness bug, and it will show in the §6 numbers.

Also switch both scanners off `Vec<char>`. Neither needs random access — they are cursors.
`str::char_indices()` removes the 4-bytes-per-char materialisation outright, which is a
worthwhile change even in isolation and can be made *before* 4b as a standalone step. Two
secondary allocations go with it: `parse_bool_partial` and `parse_null_partial` each build
a fresh `rest: String` via `self.chars.iter().skip(self.pos).collect()` — replace those
with a `starts_with`/prefix check against the remaining `&str` so a `true`/`false`/`null`
literal costs no allocation either. They fire once per literal (not per delta), so this is
correctness-neutral cleanup, but it blocks the `Vec<char>` removal until converted.

### 4c. Stop the four downstream copies

- Make `StreamEvent`'s `partial` an `Arc<AssistantMessage>`. Sites 2–5 in §1 become
  refcount bumps. This touches every `partial:` construction site (all 90) and every
  consumer, but each change is mechanical.
- At [`stream.rs:187`](../../../crates/cyrup-agent/src/agent/run/stream.rs),
  `partial = p.clone()` becomes an `Arc` clone.
- At `stream.rs:217`, `Box::new(e.clone())` stops deep-copying the message.
- `AgentSessionEvent` in
  [`session-svc/event.rs`](../../../crates/cyrup-session-svc/src/event.rs) should carry
  the same `Arc`, so `Fanout::emit`'s per-subscriber `ev.clone()`
  ([`subscriber.rs:68,72`](../../../crates/cyrup-session-svc/src/subscriber.rs)) is a
  refcount bump too.

**The biggest unknown in this layer is already resolved: the wire format does not change
and the derives keep compiling.** [`StreamEvent`](../../../crates/cyrup-provider/src/stream.rs)
derives `Clone, Debug, PartialEq, Serialize, Deserialize`, and
[`AssistantMessage`](../../../crates/cyrup-core/src/message/assistant.rs) derives
`Clone, Debug, PartialEq` + `Deserialize` alongside a matching `Serialize`. The workspace
already enables serde's **`rc`** feature
([`Cargo.toml:145`](../../../Cargo.toml) — `serde = { … features = ["derive", "rc"] }`), so
`Arc<AssistantMessage>` gets a *transparent* `Serialize` (it emits the inner value
byte-for-byte, so the func-02 `assistantMessageEvent` RPC wire is unchanged) and a working
`Deserialize` (which allocates a fresh `Arc` per decode — pointer sharing is a send-side
win, which is exactly where the copies are). `Arc<T>: PartialEq/Clone/Debug` all hold, so
the `#[derive]` on `StreamEvent` compiles untouched. Without `rc` this layer would fail to
derive `Deserialize`; it is on, so it does not.

One extra copy site belongs here, not in the four above: `AgentSessionEvent::MessageUpdate`
([`session-svc/event.rs:127`](../../../crates/cyrup-session-svc/src/event.rs)) holds the
same message **twice** — once as `assistant_message_event: Box<StreamEvent>` (which now
carries the `Arc`) and once as `message: AgentMessage::Assistant(partial.clone())`
([`event.rs:23-28`](../../../crates/cyrup-agent/src/event.rs)). If `AgentMessage::Assistant`
keeps an owned `AssistantMessage`, the per-subscriber `ev.clone()` still deep-copies that
half. Carry the `Arc` through `AgentMessage::Assistant` too so the whole event is a refcount
bump; its custom `Serialize` (`event.rs:119`, `m.serialize(serializer)`) delegates to the
inner value and keeps the wire byte-1:1.

`Arc` is right rather than `Cow`/borrowing because the event outlives the decoder's borrow
scope and crosses an `mpsc` to an arbitrary number of subscribers.

---

## 5. Order of work

1. `char_indices()` in `repair_json` + `parse_partial` (§4b tail) — standalone, small,
   measurable on its own.
2. Per-block `revision` + `ContentCache` in `anthropic_messages` (§4a) — prove the shape on
   one decoder.
3. Roll §4a to the other six decoders. The `Decoder`/`Block`/`snapshot` shape is close
   enough across them that this is repetition, not redesign.
4. `Arc<AssistantMessage>` on `StreamEvent` and `AgentSessionEvent` (§4c).
5. Incremental tool-argument parse (§4b) — last, because after 1–3 it may no longer be
   worth its complexity. **Measure before building it.**

---

## 6. Definition of Done

Observable behaviour and measured cost.

1. **The quadratic is gone.** Streaming a 256 KB tool call in 40-byte deltas costs
   **within 5× of a single end-of-stream parse** of the same buffer, not 2,883×. Same for
   64 KB (currently 786×) and 16 KB (currently 209×). Reproduce with the probe below.
2. **Every decoder benefits, not just Anthropic.** The same measurement against the
   `openai_responses`, `openai_completions`, `bedrock_converse_stream`,
   `google_generative_ai`, `mistral_conversations` and `pi_messages` decoders shows the
   same shape.
3. **A prose response is linear.** A 128 KB text-only response costs O(N) total, not the
   current 486 ms / 1.64 GB of memcpy.
4. **`partial` is unchanged, byte for byte.** For any recorded stream, the
   `AssistantMessage` carried by every emitted event is identical to the pre-change one —
   content, `stop_reason` (`Pending` while in flight), `usage` after `apply_cost`, and a
   `timestamp` that still advances per event rather than freezing.
5. **Truncated salvage still works.** A tool call cut mid-string still yields the
   recovered prefix (`{"path":"a.` → `{"path": "a."}`), and a `\uXXXX` escape split across
   two deltas is recovered identically to a whole-buffer parse of the same bytes.
6. **No new allocation per delta in the steady state.** A text delta against a message
   with one live tool block performs **zero** tool-JSON parses.
7. **The suite is green under the real gate**, including the fixture-driven tests:
   `cargo test --workspace --features test-fixtures --no-fail-fast`, and
   `cargo clippy --workspace --all-targets --features test-fixtures` exits **0**.

### The probe

Reproduce the §2 numbers. Drop this at `crates/cyrup-provider/tests/perf_probe.rs`, run
`cargo test -p cyrup-provider --release --test perf_probe -- --nocapture`, and **delete it
afterwards** — it is a measuring stick, not a test, and it does not belong in the suite.

```rust
use cyrup_provider::parse_streaming_json_object;
use std::time::Instant;

fn tool_args(body_kb: usize) -> String {
    let body = "fn main() { println!(\"hello world\"); }\\n".repeat(body_kb * 1024 / 40);
    format!("{{\"path\":\"src/main.rs\",\"content\":\"{body}\"}}")
}

#[test]
fn probe() {
    for kb in [1usize, 4, 16, 64, 256] {
        let full = tool_args(kb);
        let chars: Vec<char> = full.chars().collect();
        let (mut acc, mut deltas, mut i) = (String::new(), 0u32, 0usize);
        let t = Instant::now();
        while i < chars.len() {
            let end = (i + 40).min(chars.len());
            acc.extend(chars[i..end].iter());
            let _ = parse_streaming_json_object(Some(&acc));   // the hot line
            deltas += 1;
            i = end;
        }
        let streamed = t.elapsed();
        let t2 = Instant::now();
        let _ = parse_streaming_json_object(Some(&full));
        let once = t2.elapsed();
        println!(
            "{kb:>4} KB / {deltas:>5} deltas -> {streamed:>10.3?}  (once: {once:>9.3?}, {:.0}x)",
            streamed.as_secs_f64() / once.as_secs_f64()
        );
    }
}
```
