---
title: Performance backlog — streaming decode, transcript copies, tool parallelism
stage: new
status: pending
updated: 2026-08-29 02:33
---

# Performance backlog

Filed 2026-08-29 from a measured read of the inference-to-render hot path at
`761dc19` (branch `david/performance`). Six tasks, ranked by measured leverage — 001 has since landed, five remain open.

**Everything here is measured, not inferred.** Where a number appears it was produced on
this host in `--release` and the probe is reproduced inside the task file so anyone can
re-run it. Where something could *not* be measured it says so.

## The path these tasks sit on

```
reqwest bytes_stream ──> SSE frame ──> serde_json ──> Decoder::snapshot()   [cyrup-provider]
    ──> StreamEvent ──> partial.clone() ──> MessageUpdate                   [cyrup-agent]
    ──> Fanout::emit (awaited, bounded 1024) ──> ingest ──> draw            [cyrup-session-svc, cyrup-tui]
```

Two structural facts govern the whole list:

1. **`Fanout::emit` awaits every send**
   ([`subscriber.rs:63-72`](../../../crates/cyrup-session-svc/src/subscriber.rs)) —
   *"backpressure → slows the agent, never drops"*. The renderer is therefore on the
   throughput critical path, not merely the cosmetic one. A slow consumer stalls the
   provider stream.
2. **Five full copies of the accumulated assistant message are made per delta**, and the
   primary consumer discards all of them. See [PERF-001](../../done/2026-08-29-01-49/PERF-001_STREAM_SNAPSHOT_QUADRATIC.md) §1.

## Landed

**001 — [`PERF-001_STREAM_SNAPSHOT_QUADRATIC.md`](../../done/2026-08-29-01-49/PERF-001_STREAM_SNAPSHOT_QUADRATIC.md) shipped** in `04d6fa5`, merged as
PR #104. The per-delta re-parse and the per-delta payload copy are both gone: a snapshot is now
O(blocks), and the content axis of a 256 KB tool call in 40-byte deltas sits at 1.4x-1.9x a single
end-of-stream parse against a pre-change ~500x. It also fixed a live wrong-arguments bug on three
OpenAI-shaped wires. **It changed the cost model for 002, 005 and 006 — each of those files has a
revision pass recording what moved.**

## The tasks

| # | file | what | measured leverage |
| --- | --- | --- | --- |
| **002** | [`PERF-002_ARC_TRANSCRIPT_SNAPSHOTS.md`](PERF-002_ARC_TRANSCRIPT_SNAPSHOTS.md) | Whole transcript deep-cloned 4× per turn where pi passes a reference | O(history) → O(1); **cyrup is currently slower than pi** |
| **003** | [`PERF-003_PARALLEL_FILE_WALK.md`](PERF-003_PARALLEL_FILE_WALK.md) | `grep`/`find` walk single-threaded where pi shells out to multi-threaded `rg` | **2.8×** floor on this repo warm; **cyrup is currently slower than pi** |
| **004** | [`PERF-004_SESSION_PERSIST_FSYNC.md`](PERF-004_SESSION_PERSIST_FSYNC.md) | `fdatasync` per persisted entry, synchronously, on the caller's thread | **310×** per entry (3.3 µs → 1022 µs) on ext4 |
| **005** | [`PERF-005_DECOUPLE_RENDER_FROM_FOLD.md`](PERF-005_DECOUPLE_RENDER_FROM_FOLD.md) | Render runs on the event-fold task; draw cost can starve the input arm | fixes a **documented, in-code** input-death cliff |
| **006** | [`PERF-006_PIPELINE_STREAM_DECODE.md`](PERF-006_PIPELINE_STREAM_DECODE.md) | Frame/parse/fold run serially on one task | decode becomes `max(stage)` not `sum(stages)` |

## Suggested order

`005` → `003` → `006` → `004` → `002`.  (`001` is landed.)

`005` first because it fixes a failure the code itself documents as already reachable, and
because `Fanout::emit` awaits every send — a slow renderer throttles the provider stream, so this
is a throughput fix as well as a responsiveness one. `002` last because `001` already delivered
most of it: assistant messages, text, thinking and tool arguments are all refcounted now, so
re-measure before building. The three-stage pipeline `006` was opened for is closed; what survives
there is the SSE framer replacement.

## Two tasks are parity regressions, not just optimisations

`002` and `003` are places where **cyrup is slower than the thing it ports**. They are not
"make it faster" work; they are "stop being slower than pi" work, and they should be
described that way in any commit. `004` is a third of the same shape, but the extra cost
buys a durability guarantee pi does not offer — so it is a decision, not a defect. That
task states the decision rather than presuming it.

## What is deliberately NOT here

- **simd-json / sonic-rs.** The per-frame JSON is small. The quadratic is the problem;
  with `001` landed the parser choice is close to irrelevant. Reaching for a faster parser
  first would have delivered a few percent and hidden the 2,883×.
- **Removing the awaited fanout backpressure.** It is what bounds memory. `005` fixes the
  consumer instead.
- **Touching `ToolExecution::Sequential`**
  ([`tools/mod.rs:60`](../../../crates/cyrup-agent/src/agent/run/tools/mod.rs)). That is a
  correctness gate, not a performance knob.
- **Tool-call parallelism.** Already done and already better than pi:
  [`exec.rs:48`](../../../crates/cyrup-agent/src/agent/run/tools/exec.rs) uses a real
  `JoinSet` across cores, where pi's `Promise.all`
  ([`agent-loop.ts:549`](../../../../pi/packages/agent/src/agent-loop.ts)) is concurrency
  on one thread. No work needed.

## Open question for a human: there is no benchmark harness

`cyrup` has no `benches/`, no criterion dependency, and no per-turn timing surface. That
is the actual reason a 2,883× regression sat unnoticed — nothing in the repo could have
reported it. Every number in this backlog came from a throwaway probe that was deleted
afterwards.

This is **not** filed as a task because flux's stated principle is *"No tests, no
benchmarks in task files — `/flux/tests` owns tests, and is a separate step"*
([`SKILL.md`](../../../crates/cyrup-flux/resources/skills/flux/SKILL.md)). A standalone
task whose deliverable *is* a benchmark harness is arguably a different thing from a
feature task that specifies its own tests — but that call belongs to a human, not to the
agent that noticed the gap.

**The ask:** decide whether a `PERF-000` establishing a criterion harness over the decode
path belongs in this queue, in `/flux/tests`, or nowhere. Until it exists, every
Definition of Done in this backlog has to be verified by hand with the probe reproduced
in its own file.
