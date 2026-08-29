---
stage: qa
status: completed
updated: 2026-08-29 13:40
---

# PERF-001 — make the streamed `partial` linear, not just parse-free

## What was wrong

Every streaming decoder rebuilt the whole `AssistantMessage` for the `partial` on every delta, and
each rebuild copied the accumulated payload: an owned `String` per text block, a freshly parsed
`Map` per open tool call. That is O(N²) over a turn. A 256 KB tool call streamed in 40-byte deltas
cost **10.06 s** where a single end-of-stream parse of the same buffer costs 3.49 ms — a **2,883×**
overhead.

## What shipped

**Two types in `cyrup-core`**, both presenting today's API through `Deref` so the ~500 read sites
compile untouched:

- `SharedStr` — every snapshot of one block shares one append-only buffer and remembers only its own
  prefix length, so taking a snapshot is a refcount bump. The flat `&str` is built on first read and
  cached in an `Arc<str>`, so a snapshot nobody reads costs nothing and a clone is O(1) in every
  state — including a block whose payload was replaced wholesale at block end. `push_str` appends in
  place only while the handle is the buffer's tail; one that is not forks first, which is what
  preserves `String`'s value semantics.
- `LazyArgs` — `serde_json::Value` has no shared-string variant, so the tool path defers instead of
  sharing: a snapshot carries the raw argument buffer and recovers the `Map` — by exactly the
  whole-buffer `parse_streaming_json_object` the decoders used to run per delta — only if something
  reads it.

`json_parse.rs` moved to `cyrup-core::json` (a pure file move; it depends only on `serde_json`) and
is re-exported from `cyrup-provider`, so every internal path and the public API are unchanged.

**Wired through all seven decode paths.** `Content::Text.text` and `Content::Thinking.thinking`
became `SharedStr`, `ToolCall.arguments` became `LazyArgs`, and each decoder's scratch blocks
followed. All seven in-place `push_str` sites compile unchanged. `StreamingArgs` is gone from every
decode path — it existed to make a per-delta parse cheap, and there is no longer a per-delta parse.

**A shipped correctness bug went with it.** `response.function_call_arguments.done` replaced the
tool block's buffer wholesale while a second copy of the argument state — an incremental parser fed
only the deltas — kept describing the buffer that no longer existed, so the `partial` emitted
alongside that frame projected the PRE-`done` arguments on three wires. The block now holds the
buffer and nothing derived from it, so the two cannot diverge;
`toolcall_arguments_done_reprojects_the_replaced_buffer` keeps it nailed down.

## Results

| | |
| --- | --- |
| content axis, 256 KB tool call in 40-byte deltas | **1.4×–1.9×** a single end-of-stream parse (bar: 5×; was ~500×) |
| prose, 1 MB in 40-byte deltas | 12–60 copies of the buffer, bounded at 500 |
| per-snapshot projection | O(blocks), asserted structurally rather than timed |

The residual whole-drive cost (~60× one parse) is the per-event plumbing floor — one channel send
and one `AssistantMessage` construction per delta, already O(1) per event. That is
[PERF-002](./PERF-002_ARC_TRANSCRIPT_SNAPSHOTS.md)'s subject, not this one's.

## How it is guarded

Linearity is proved structurally, not by a stopwatch: a snapshot that materialises no payload is
O(blocks) by construction whatever the buffer holds, and that is asserted directly
(`no_snapshot_materialises_a_payload_nobody_read`). Timing corroborates it — the absolute bar is
asserted at the size the DoD names — but the scaling ratio is reported only, because at this cost a
difference-of-drives cannot resolve it at any affordable size and an assertion that passes on a
`0.00x` reading is worse than none.

Alongside: prefix freezing end to end for text and tool arguments; O(1) clone for both types in
every state; `LazyArgs` equivalence to the incremental scanner across 11 buffers × 7 chunkings;
serde byte-identity with `String`/`Map`; truncated salvage including a `\uXXXX` escape split across
two deltas.

## Gate

`cargo test --workspace` green, `cargo clippy --workspace --all-targets` 0, and
`cargo doc --workspace` 0 — the last of which also cleared 18 pre-existing broken intra-doc links
across `cyrup-intercom`, `cyrup-mcp` and `cyrup-tui`.
