---
stage: exec
status: done
updated: 2026-08-23 02:15
---

# Delete The Second NDJSON Parser — spawn::NdjsonEvent Is Dead And Wire-Incompatible

## Description

Two independent parsers read the same child stdout bytes, and the second one has never worked against a real child.

`src/exec/ndjson.rs` defines `SubagentEvent` with `rename_all_fields = "camelCase"`, matching the real producer (`crates/cyrup-agent/src/event.rs:216`, which is `#[serde(tag="type", rename_all="snake_case", rename_all_fields="camelCase")]` and emits `toolCallId`/`toolName`/`isError`). `src/spawn/mod.rs` defines `NdjsonEvent` with no `rename_all_fields`, so its `ToolExecutionStart`/`ToolExecutionEnd` variants require snake_case payload keys the producer never emits. `exec/ndjson.rs:105-112` documents the consequence: a known `type` tag with a missing required field fails the whole line deserialize, and `#[serde(other)]` only rescues unknown *tags*. So the spawn-side parser silently yields `None` for exactly the two events it was built to count; only `MessageEnd` survives.

Nothing in production reads the result. Every stdout line is nonetheless parsed twice — once by `spawn::SpawnedChild`, then again from the identical raw bytes by `exec::ndjson::parse_line` (exec/mod.rs:3236-3243 calls the two 'independent, tolerant views over the exact same wire bytes'). The only test pinning the spawn schema feeds a hand-written fixture the real emitter never produces, so CI cannot see the drift. Anyone adding a child event today has to guess which of two schemas is authoritative and will pick wrong half the time.

## Evidence

src/spawn/mod.rs:373-406 defines `NdjsonEvent` (4 variants: ToolExecutionStart, ToolExecutionEnd, MessageEnd, Unknown) with `#[serde(tag="type", rename_all="snake_case")]` and no `rename_all_fields`; src/exec/ndjson.rs:83-89 defines `SubagentEvent` (18 variants) with `rename_all_fields = "camelCase"`. Producer at crates/cyrup-agent/src/event.rs:216 carries both attributes; its doc at :213 names the payload fields `toolCallId`, `toolName`, `isError`. Parse sites: src/spawn/mod.rs:679 and :735 run `serde_json::from_str::<NdjsonEvent>(&line).ok()` on every stdout line; src/exec/mod.rs:3244 re-parses the same raw line via `crate::exec::ndjson::parse_line`, with the 'both are independent, tolerant views over the exact same wire bytes' comment at :3236-3243. `grep -rn '\.parsed' src/` returns exactly 5 hits — exec/ndjson.rs:438,923,925 and spawn/mod.rs:1495,1505 — and spawn/mod.rs's `#[cfg(test)] mod tests` begins at line 1081, so the latter two are test-only: zero production consumers. The only schema test, spawn/mod.rs:1265, uses the literal fixture `{"type":"tool_execution_start","call_id":"c1","name":"bash"}`. Two identically-named `NdjsonLine` structs at spawn/mod.rs:482 and exec/ndjson.rs:327 (confirmed by `grep -rhoE '^pub struct [A-Za-z0-9_]+' src/ | sort | uniq -d`). Sizes: exec/ndjson.rs is 989 lines (475 non-test) versus ~110 lines of parallel schema in spawn/mod.rs.

## Suggested approach

Delete `spawn::NdjsonEvent` and reduce `spawn::NdjsonLine` to carrying only `raw`; `SpawnedChild::next_event`/`next_event_or_exit` tee the raw line and `exec::ndjson::parse_line` becomes the crate's single parse. If a typed view is genuinely wanted at the spawn boundary, re-export `exec::ndjson::SubagentEvent` rather than redefining a second schema. The spawn-side doc at mod.rs:360-372 argues the narrow view is deliberate — it argues for a *working* narrow view and never contemplates that the shape does not parse, so it should be rewritten or removed rather than treated as a decision that excuses the duplication.

## Acceptance Criteria

- [ ] `grep -rn 'enum NdjsonEvent' src/` returns 0 — the spawn-side event enum is gone
- [ ] `grep -rhoE '^pub struct [A-Za-z0-9_]+' src/ | sort | uniq -d` no longer lists `NdjsonLine`
- [ ] Each child stdout line is deserialized at most once on the production path: `grep -rn 'from_str::<NdjsonEvent>' src/` returns 0
- [ ] The hand-written fixture at spawn/mod.rs:1265 is gone or replaced by a line captured from the real `cyrup --mode json` emitter (camelCase payload keys), and a test asserts that a real `tool_execution_start` line parses to the tool-start variant with a populated call id
- [ ] `cargo test -p cyrup-ext-subagents` passes with no new failures

## Source

- Identified by the `subagents-hygiene-survey` workflow (13 agents, 21 raw findings, 16 confirmed after adversarial verification).
- Effort: medium · survey priority: 4 of 6
