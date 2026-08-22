---
stage: new
status: done
updated: 2026-08-22 18:30
severity: high
effort: medium
category: test-structure
---

# Extract A Shared `src/tests/support` Module

## Description
[`src/tests/mod.rs`](../../crates/cyrup-agent/src/tests/mod.rs) is 16 lines: a two-line module doc plus 13 `mod` declarations. There is no shared fixture home, so every test file re-derives the same preamble. All counts below were re-run against the working tree.

- `fn model_ref()` is defined in **12** files (`grep -rn "fn model_ref()" src/tests/*.rs | wc -l` = 12). Eleven bodies are byte-identical: `ModelRef { provider: "faux".into(), api: Some("faux".into()), model: "faux-1".into() }`. [`proxy_live_turn.rs:24`](../../crates/cyrup-agent/src/tests/proxy_live_turn.rs) is the one deliberate `anthropic` variant.
- `struct Recorder` is declared in **8** files: [agent_loop.rs:46](../../crates/cyrup-agent/src/tests/agent_loop.rs), [area02_backlog.rs:94](../../crates/cyrup-agent/src/tests/area02_backlog.rs), [hook_failure_text.rs:60](../../crates/cyrup-agent/src/tests/hook_failure_text.rs), [model_boundary.rs:108](../../crates/cyrup-agent/src/tests/model_boundary.rs), [pending_containment.rs:47](../../crates/cyrup-agent/src/tests/pending_containment.rs), [tool_result_model.rs:95](../../crates/cyrup-agent/src/tests/tool_result_model.rs), [turn_tool_refresh.rs:37](../../crates/cyrup-agent/src/tests/turn_tool_refresh.rs), [untracked_misses.rs:80](../../crates/cyrup-agent/src/tests/untracked_misses.rs). Seven are the same `EventSubscriber` collector (`events: Mutex<Vec<AgentEvent>>` + `snapshot()`; area02_backlog adds `turn_starts()`). `turn_tool_refresh.rs:37` binds the **same name** to an unrelated `StreamFn` spy holding `inner` + `seen: Requests`. [round2_parity.rs:308](../../crates/cyrup-agent/src/tests/round2_parity.rs) is a ninth copy of the subscriber, named `AgentRec`.
- `fn obj_schema()` appears in **6** files and has **drifted**: area02_backlog.rs:43 returns `json!({ "type": "object", "properties": {}, "additionalProperties": true })`, while agent_loop.rs:104, model_boundary.rs:198, round2_parity.rs:112, tool_result_model.rs:44 and untracked_misses.rs:122 all return `json!({ "type": "object" })`.
- `fn faux_stream_fn` appears in **6** files under **two incompatible signatures**: `-> (Arc<FauxProvider>, Arc<dyn StreamFn>)` at agent_loop.rs:37 and [preflight_validation.rs:26](../../crates/cyrup-agent/src/tests/preflight_validation.rs), vs `-> Arc<dyn StreamFn>` at area02_backlog.rs:46, hook_failure_text.rs:52, pending_containment.rs:39, round2_parity.rs:58 — with the parameter spelled both `Vec<AssistantMessage>` and `Vec<cyrup_core::AssistantMessage>`.
- Further whole-item copies: `struct RecordingStreamFn` (agent_loop.rs:806, area02_backlog.rs:62, model_boundary.rs:63, proxy_live_turn.rs:47); `struct EchoTool` (agent_loop.rs:107, hook_failure_text.rs:114, round2_parity.rs:115, untracked_misses.rs:97); `PayloadRecordingStreamFn` + `type PayloadLog` + `fn payload_recording` duplicated verbatim between tool_result_model.rs:61-92 and untracked_misses.rs:46-76; `first_turn_results` (hook_failure_text.rs:78, tool_result_model.rs:113); `last_assistant` (hook_failure_text.rs:90, round2_parity.rs:348); `ev_name` (agent_loop.rs:63, round2_parity.rs:96); `struct PanicHook` (model_boundary.rs:597, untracked_misses.rs:308); `struct FailingTransform` (area02_backlog.rs:891, hook_failure_text.rs:244).

Why it matters: this is the largest duplication surface in the crate and it has already drifted in three ways that change meaning silently — two `obj_schema()` bodies feeding tool registration, two `faux_stream_fn` signatures under one name, and `Recorder` bound to two unrelated types. A reader moving between files cannot assume a familiar helper name means the familiar thing, and any change to the fixture surface (a new `AgentBuilder` argument, a new `EventSubscriber` method, a new `Tool` method) must be applied 6-12 times or the suite quietly tests different things in different files.

The home must be crate-local: [`crates/cyrup-test-support/Cargo.toml:18`](../../crates/cyrup-test-support/Cargo.toml) lists `cyrup-agent` as a normal dependency, so cyrup-agent cannot take `cyrup-test-support` as a dev-dependency without a cycle.

## Scope
In scope: creating `crates/cyrup-agent/src/tests/support.rs`, moving the duplicated fixtures into it, deleting the per-file copies, and the one rename named below. Move-only refactor — assertions, test names, and test count stay unchanged.

Out of scope: adding new tests or new assertions; changing production code under `src/agent/`, `src/proxy.rs`, or elsewhere; splitting the large test files into smaller ones (that is a separate concern and this task is its prerequisite); touching `crates/cyrup-test-support`; any rustfmt or clippy campaign. Must not overlap with the queued `CARGO_DOC_WARNINGS`, `TEST_FAILURES`, `BUILD_FEATURE_COMBINATIONS`, or `CYRUP_IT_COMPILE_ERRORS` tasks.

## Approach
1. Add `mod support;` to `src/tests/mod.rs` (first entry, before `mod agent_loop;`) and create `src/tests/support.rs`. Keep it one file; split into `support/{model,stream,subscriber,tools}.rs` only if it exceeds ~400 lines.
2. Move in the canonical items: `model_ref()` (the `faux` body) plus an explicit `anthropic_model_ref()` for proxy_live_turn.rs; the event-collecting subscriber as `EventRecorder` with `snapshot()`; `first_turn_results`, `last_assistant`, `ev_name`; `EchoTool`; `PayloadRecordingStreamFn` / `PayloadLog` / `payload_recording`; the `RecordingStreamFn` wrapper shape — leave each file's `Captured` payload struct local, those genuinely differ.
3. `faux_stream_fn` moves in its widest form, `-> (Arc<FauxProvider>, Arc<dyn StreamFn>)`, taking `Vec<cyrup_core::AssistantMessage>`; the four callers that only want the stream fn destructure `.1`. Widest form chosen because it is a superset — the narrow form cannot express the two tests that assert on the provider.
4. `obj_schema()` moves as `json!({ "type": "object" })` — that is the 5-of-6 majority. Before deleting area02_backlog.rs:42, check its three call sites (lines 169, 734, 797, 1005) for any assertion that reads `properties` or `additionalProperties`; grep confirms none do, so the swap is safe. If one turns up, keep a separately named `permissive_obj_schema()` in support rather than forking `obj_schema`.
5. Rename `turn_tool_refresh.rs:37`'s `Recorder` to `ToolRequestSpy` so `Recorder`/`EventRecorder` has one meaning crate-wide; rename `round2_parity.rs:308`'s `AgentRec` usages to the shared `EventRecorder`.
6. Move `PanicHook` and `FailingTransform` into support as well — both pairs are identical unit structs with identical impls.
7. In each of the 12 touched files, delete the local copies and add `use super::support::*;` (or named imports if `*` collides). `agent_message_role_key.rs` and `settlement_latch.rs` share nothing beyond `model_ref` — touch settlement_latch.rs only for `model_ref`, leave agent_message_role_key.rs alone.

## Acceptance Criteria
- [ ] `crates/cyrup-agent/src/tests/support.rs` exists and `src/tests/mod.rs` declares `mod support;`.
- [ ] `grep -rn "fn model_ref()" crates/cyrup-agent/src/tests/*.rs | wc -l` returns 1 (in `support.rs`); `anthropic_model_ref` is defined once.
- [ ] `grep -rn "fn obj_schema()\|fn faux_stream_fn\|fn ev_name\|fn last_assistant\|fn first_turn_results\|fn payload_recording" crates/cyrup-agent/src/tests/*.rs` reports each exactly once, all in `support.rs`.
- [ ] `grep -rn "struct Recorder\|struct AgentRec\|struct EchoTool\|struct PayloadRecordingStreamFn\|struct PanicHook\|struct FailingTransform" crates/cyrup-agent/src/tests/*.rs` shows no name defined more than once, and no `struct Recorder` remains (the subscriber is `EventRecorder`; turn_tool_refresh's spy is `ToolRequestSpy`).
- [ ] `cargo test -p cyrup-agent` passes with the same 140 tests (compare the `test result:` line to the pre-change run).
- [ ] `cargo clippy -p cyrup-agent --all-targets` emits no more than the 3 diagnostics present before this change.
- [ ] `git diff --stat` shows no changes under `crates/cyrup-agent/src/agent/`, `src/proxy.rs`, or `crates/cyrup-test-support/`.
