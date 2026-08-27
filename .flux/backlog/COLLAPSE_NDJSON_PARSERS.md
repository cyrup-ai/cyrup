---
stage: aug
status: done
updated: 2026-08-27 04:47
---

# Delete The Second NDJSON Parser — spawn::NdjsonEvent Is Dead And Wire-Incompatible

## ALREADY DONE — this is a verification record, not an implementation plan

**Every acceptance criterion in this task already holds on `origin/main`.** The work landed in
commit `cb7afa5` ("cyrup-ext-subagents hygiene queue, partial remediation, and exec/mod.rs
decomposition", merged as PR #65), whose own message names this task as *complete*:

> COLLAPSE_NDJSON_PARSERS (complete): deleted `spawn::NdjsonEvent`, which wasn't merely a duplicate
> of `exec::ndjson` but a broken one — missing `rename_all_fields = "camelCase"` meant
> `tool_execution_start`/`end` lines from a real child parsed to `None`.
> `exec::ndjson::parse_line` is now the crate's single NDJSON schema.

Unlike its sibling `UNIFY_PATH_AND_CLOCK_HELPERS` (which that same commit explicitly flags as
*partial*, with named outstanding items), this task was carried to completion and the code was
subsequently re-verified through the `exec/mod.rs` decomposition and the intra-doc-link pass that
followed it. Nothing in `./src` needs to change. **The only outstanding action is filing this task
into `.flux/done/`.**

The line numbers cited in the original *Evidence* section below have all drifted — `src/exec/mod.rs`
went from 7,923 lines to 1,443 when it was decomposed into `agent_config.rs`, `run_result.rs`,
`progress.rs`, `spawn_plan.rs`, `attempt_runner.rs`, `drive_attempt.rs` and `testsupport.rs`, so the
"parsed twice at `exec/mod.rs:3236-3243`" site no longer exists at all, by either line number or
file. Every citation in the *Current state* table below was re-read from the working tree today.

---

## Current state

Crate root for all paths below: [`crates/cyrup-ext-subagents`](../../crates/cyrup-ext-subagents).
Working tree `src/` is level with `origin/main` (the only diff on this branch is under `.flux/`).

| # | Acceptance criterion | Holds? | Evidence |
|---|---|---|---|
| 1 | `grep -rn 'enum NdjsonEvent' src/` returns 0 | **YES** | grep exits 1, no matches. The only three surviving mentions of the identifier are prose in doc comments explaining why it is gone: [`src/exec/ndjson.rs:25`](../../crates/cyrup-ext-subagents/src/exec/ndjson.rs), [`src/spawn/mod.rs:319`](../../crates/cyrup-ext-subagents/src/spawn/mod.rs), [`src/spawn/mod.rs:1173`](../../crates/cyrup-ext-subagents/src/spawn/mod.rs). |
| 2 | `grep -rhoE '^pub struct [A-Za-z0-9_]+' src/ \| sort \| uniq -d` no longer lists `NdjsonLine` | **YES** | `NdjsonLine` is declared exactly once, at [`src/exec/ndjson.rs:328`](../../crates/cyrup-ext-subagents/src/exec/ndjson.rs). The spawn-side twin is gone. (The command today lists three *unrelated* duplicates — `AcceptanceLedger`, `NestedRunSummary`, `ProactiveSkillSubagentsConfig` — none of which is in this task's scope; the last was explicitly *rejected* by the same survey as a deliberate split.) |
| 3 | `grep -rn 'from_str::<NdjsonEvent>' src/` returns 0 | **YES** | grep exits 1. The crate's only `from_str` against an event schema is [`src/exec/ndjson.rs:349-351`](../../crates/cyrup-ext-subagents/src/exec/ndjson.rs): `pub fn parse_line(line: &str) -> Option<SubagentEvent> { serde_json::from_str::<SubagentEvent>(line).ok() }`. |
| 3b | Each child stdout line is deserialized **at most once** on the production path | **YES** | `SpawnedChild::next_event` ([`src/spawn/mod.rs:571`](../../crates/cyrup-ext-subagents/src/spawn/mod.rs)) and `next_event_or_exit` ([`:611`](../../crates/cyrup-ext-subagents/src/spawn/mod.rs)) are now pure line sources — they read, tee to the `.jsonl` artifact, and hand back `String`. `ChildStep::Line` ([`:347-350`](../../crates/cyrup-ext-subagents/src/spawn/mod.rs)) carries `Result<String, SubagentError>`, not a parsed event. The three production parse sites — [`src/exec/drive_attempt.rs:322`](../../crates/cyrup-ext-subagents/src/exec/drive_attempt.rs), [`src/background/runner_main.rs:1153`](../../crates/cyrup-ext-subagents/src/background/runner_main.rs), [`src/tui/events.rs:428`](../../crates/cyrup-ext-subagents/src/tui/events.rs) — each consume a *different* raw-line source (the live child, the runner's telemetry sink, the TUI's replay stream), so no byte sequence is parsed twice. |
| 4 | The hand-written snake_case fixture is gone, replaced by a real emitter line with camelCase payload keys, and a test asserts a real `tool_execution_start` parses to the tool-start variant with a populated call id | **YES** | The fixture `{"type":"tool_execution_start","call_id":"c1","name":"bash"}` no longer appears anywhere in the crate. Its replacement is [`src/spawn/mod.rs:1182-1201`](../../crates/cyrup-ext-subagents/src/spawn/mod.rs), `a_real_tool_execution_start_line_parses_with_a_populated_call_id` — quoted in full below. A companion, `ndjson_event_degrades_unknown_tags_rather_than_erroring` at [`:1203-1208`](../../crates/cyrup-ext-subagents/src/spawn/mod.rs), pins the `#[serde(other)]` half. |
| 5 | `cargo test -p cyrup-ext-subagents` passes with no new failures | **YES (as of `cb7afa5`)** | Not re-run in this augmentation pass (research-only mandate). `cb7afa5`'s own verification record: `cargo check --workspace --all-targets` clean, 2,483 lib tests + 1 doc-test pass, clippy on this crate reporting only two pre-existing findings. Re-confirm with the command in *Definition of Done*. |

### The replacement test, verbatim

[`src/spawn/mod.rs:1182-1201`](../../crates/cyrup-ext-subagents/src/spawn/mod.rs):

```rust
#[test]
fn a_real_tool_execution_start_line_parses_with_a_populated_call_id() {
    let line = r#"{"type":"tool_execution_start","toolCallId":"toolu_01ABC","toolName":"bash","args":{"command":"ls"}}"#;
    let ev = crate::exec::ndjson::parse_line(line)
        .expect("a real emitter line must parse, not degrade to None");
    let crate::exec::ndjson::SubagentEvent::ToolExecutionStart {
        tool_call_id,
        tool_name,
        ..
    } = ev
    else {
        panic!("a real `tool_execution_start` line must parse to the tool-start variant, got {ev:?}");
    };
    assert_eq!(
        tool_call_id.as_str(),
        "toolu_01ABC",
        "the call id must survive the parse, not arrive empty"
    );
    assert_eq!(tool_name, "bash");
}
```

That is the emitter's real shape: `toolCallId`/`toolName`, not `call_id`/`name`. It only parses
because the surviving schema carries `rename_all_fields` —
[`src/exec/ndjson.rs:85-91`](../../crates/cyrup-ext-subagents/src/exec/ndjson.rs):

```rust
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum SubagentEvent {
```

matching the producer at
[`crates/cyrup-agent/src/event.rs:216-217`](../../crates/cyrup-agent/src/event.rs):

```rust
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum AgentEvent {
```

The variant it lands in, [`src/exec/ndjson.rs:127-132`](../../crates/cyrup-ext-subagents/src/exec/ndjson.rs):

```rust
ToolExecutionStart {
    tool_call_id: ToolCallId,
    tool_name: String,
    #[serde(default)]
    args: serde_json::Value,
},
```

### The spawn boundary's replacement doc

The old doc at `spawn/mod.rs:360-372`, which the task correctly flagged as arguing for a *working*
narrow view it never had, was rewritten rather than deleted.
[`src/spawn/mod.rs:316-330`](../../crates/cyrup-ext-subagents/src/spawn/mod.rs) now opens:

> Where a child's stdout lines are typed.
>
> This module deliberately defines NO event schema of its own. It used to: a narrow `NdjsonEvent`
> with `#[serde(tag = "type", rename_all = "snake_case")]` and no `rename_all_fields` […] So the
> spawn boundary is now a pure line source: it reads, tees (R-SA-058), and hands back the raw line
> text. `crate::exec::ndjson::SubagentEvent` is the crate's single NDJSON schema and
> `crate::exec::ndjson::parse_line` its single parse — one shape to keep in step with the producer,
> so a newly added child event has exactly one place to be taught.

That is exactly the *Suggested approach*'s prescription, including its "rewritten or removed rather
than treated as a decision that excuses the duplication" clause. The single-parse invariant is
restated at the surviving consumer, [`src/exec/drive_attempt.rs:317-322`](../../crates/cyrup-ext-subagents/src/exec/drive_attempt.rs):

> `SpawnedChild::next_event_or_exit` tees and hands back the raw line without parsing it —
> `exec::ndjson::parse_line` is the crate's ONE NDJSON parse, so each child stdout line is
> deserialized exactly once, here, against the single `SubagentEvent` schema.

### Out-of-scope observation (do NOT action under this task)

`NdjsonLine` and `collect_ndjson` ([`src/exec/ndjson.rs:328`, `:421`](../../crates/cyrup-ext-subagents/src/exec/ndjson.rs)) now have no production callers — their only user is
`collect_ndjson_pairs_raw_text_with_its_parse_outcome` at [`:916`](../../crates/cyrup-ext-subagents/src/exec/ndjson.rs). That is a *dead-code* question about the surviving
module, not this task's *duplication* question, and this task's criteria do not cover it. Record it
as a fresh finding if it matters; do not fold it in here.

---

## Definition of Done

Everything below already passes. Run it to confirm, then move this file to `.flux/done/`.

```sh
cd /home/user/cyrup/crates/cyrup-ext-subagents

# AC1 — spawn-side event enum is gone (expect: no output, exit 1)
grep -rn 'enum NdjsonEvent' src/

# AC2 — NdjsonLine is not duplicated (expect: NdjsonLine absent from the output)
grep -rhoE '^pub struct [A-Za-z0-9_]+' src/ | sort | uniq -d

# AC3 — no second deserialize (expect: no output, exit 1)
grep -rn 'from_str::<NdjsonEvent>' src/

# AC3b — exactly one from_str against an event schema, in ndjson.rs (expect: 1 line)
grep -rn 'from_str::<SubagentEvent>' src/

# AC4 — the snake_case fixture is gone (expect: no output), the real-line test is present
grep -rn '"call_id":"c1"' src/
grep -n 'a_real_tool_execution_start_line_parses_with_a_populated_call_id' src/spawn/mod.rs

# AC5 — the suite
cd /home/user/cyrup && cargo test -p cyrup-ext-subagents
```

Then, and only then:

```sh
git mv .flux/todo/COLLAPSE_NDJSON_PARSERS.md .flux/done/
```

---

## Original task (retained for the record — line citations below are pre-`cb7afa5` and have drifted)

### Description

Two independent parsers read the same child stdout bytes, and the second one has never worked
against a real child.

`src/exec/ndjson.rs` defines `SubagentEvent` with `rename_all_fields = "camelCase"`, matching the
real producer (`crates/cyrup-agent/src/event.rs:216`, which is
`#[serde(tag="type", rename_all="snake_case", rename_all_fields="camelCase")]` and emits
`toolCallId`/`toolName`/`isError`). `src/spawn/mod.rs` defines `NdjsonEvent` with no
`rename_all_fields`, so its `ToolExecutionStart`/`ToolExecutionEnd` variants require snake_case
payload keys the producer never emits. `exec/ndjson.rs:105-112` documents the consequence: a known
`type` tag with a missing required field fails the whole line deserialize, and `#[serde(other)]`
only rescues unknown *tags*. So the spawn-side parser silently yields `None` for exactly the two
events it was built to count; only `MessageEnd` survives.

Nothing in production reads the result. Every stdout line is nonetheless parsed twice — once by
`spawn::SpawnedChild`, then again from the identical raw bytes by `exec::ndjson::parse_line`
(`exec/mod.rs:3236-3243` calls the two "independent, tolerant views over the exact same wire
bytes"). The only test pinning the spawn schema feeds a hand-written fixture the real emitter never
produces, so CI cannot see the drift.

### Suggested approach (as executed)

Delete `spawn::NdjsonEvent` and reduce `spawn::NdjsonLine` to carrying only `raw`;
`SpawnedChild::next_event`/`next_event_or_exit` tee the raw line and `exec::ndjson::parse_line`
becomes the crate's single parse.

### Source

- Identified by the `subagents-hygiene-survey` workflow (13 agents, 21 raw findings, 16 confirmed
  after adversarial verification).
- Effort: medium · survey priority: 4 of 6
- Remediated by commit `cb7afa5` / PR #65.
