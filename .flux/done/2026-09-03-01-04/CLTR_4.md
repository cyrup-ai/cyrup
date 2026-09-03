---
stage: qa
status: completed
updated: 2026-09-03 23:20
aug_against: branch 9509bff (CLTR_1..3B landed) — uncapped sweep of Finalized/source_index/immediate_error/Prep/Deferred/finalize across all 23 crates incl. tests; pi agent-loop.ts read for the finalize table
---

# CLTR_4 — Tool-call pipeline: constructor discipline + a pure finalize fold (F6)

OBJECTIVE: Make a `Finalized` unconstructible without its `source_index` (today `immediate_error`
writes `0` and two of three producers patch it afterwards), promote the parallel runtime's local
`Deferred` to a shared `PreparedCall`, and split `finalize` into an async hook call (shell) and a
pure `fold_tool_outcome` (core) so the replace-not-merge table is one total function. Internal to
`cyrup-agent/src/agent/run/tools/`; zero tendrils. Requires CLTR_1 (landed). Source: research
§3 F6 ([`.flux/research/CORE_LOOP_TYPE_REVIEW.md`](../research/CORE_LOOP_TYPE_REVIEW.md)).

## Aug findings that change the plan (read first)

**Sweep result — scope is exactly four files plus one caller.** The uncapped sweep
(`tmp/cltr4_sweep.txt`, 133 hits) finds `Finalized`, `Prep`, `source_index`, `immediate_error`,
`Deferred` (the tool one) and `.finalize(` for this pipeline ONLY in
[`tools/mod.rs`](../../crates/cyrup-agent/src/agent/run/tools/mod.rs),
[`tools/preflight.rs`](../../crates/cyrup-agent/src/agent/run/tools/preflight.rs),
[`tools/exec.rs`](../../crates/cyrup-agent/src/agent/run/tools/exec.rs),
[`tools/finalize.rs`](../../crates/cyrup-agent/src/agent/run/tools/finalize.rs); the one outside
caller is `turn.rs:71` `self.fail_truncated_tool_calls(&calls).await?` (signature unchanged).
Every other `Deferred`/`finalize` hit is `StopReason::Deferred` or a hasher — unrelated. The four
`tools/` files carry NO `#[cfg(test)]`; the test files name none of these internals
(`tool_result_model.rs:346-349` mentions `immediate_error` in a doc comment only). So no test
edits.

**Finding 1 — "fields stay private" is NOT enforceable where `Finalized` lives today.** Rust field
privacy is module-tree scoped: a struct declared in `tools/mod.rs` with private fields is
literal-constructible from `exec.rs`, `preflight.rs` and `finalize.rs`, because they are
`tools`'s children. That is exactly how `Finalized { source_index: 0, .. }` got written in
`preflight.rs:153` and patched in `exec.rs:70,278`. For `Finalized::new` to be the only
constructor **by the compiler**, `Finalized` must move to a leaf module `tools/finalized.rs`
with private fields and `pub(super)` accessors. Do that (SUBTASK1).

**Finding 2 — three of `Finalized`'s seven fields are duplicates.** `tool_call_id`, `tool_name`
and `is_error` are copies of `message.tool_call_id`, `message.tool_name`, `message.is_error`
(both producers set them pairwise: `preflight.rs:136-159`, `finalize.rs:123-148`), and
`result_value` is a pure function of `(message, terminate)` via `result_value_of`
([`agent/message.rs:75-100`](../../crates/cyrup-agent/src/agent/message.rs)). So the constructor
is `Finalized::new(source_index, message: ToolResultMessage, terminate: TerminateHint)` and it
DERIVES `result_value`; the struct keeps four fields. A `Finalized` then cannot disagree with
its own message.

**Finding 3 — the research's `after_hook(..) -> Option<AfterOverride>` LOSES pi's third arm.**
pi `finalizeExecutedToolCall`
([`tmp/pi/packages/agent/src/agent-loop.ts:713-757`](../../tmp/pi/packages/agent/src/agent-loop.ts))
has three outcomes: override present (`afterResult.x ?? result.x` per field, `:738-745`), no
override (keep), and **hook throws** (`:747-750`: `createErrorToolResult(error.message)`,
`isError = true` — which also drops `usage` and `addedToolNames`). No `AfterOverride` value can
express the throw arm (`AfterOverride.usage: Option<Usage>` can replace but never CLEAR usage,
and `added_tool_names` is not on it at all —
[`hooks.rs:104-118`](../../crates/cyrup-agent/src/hooks.rs)). Collapsing `Err` to `None` would
silently turn a failing hook into "keep the tool's result" — a pi-parity regression of R-02-050.
So the shell passes the full `Result<Option<AfterOverride>, HookError>` through and the pure
fold's `match` has the three arms. (This is the exact shape CLTR_7's
`AfterOutcome { Keep, Override, Failed(HookError) }` will rename; nothing here pre-empts it.)

**Finding 4 — the hook reads the outcome BEFORE the fold, so the throw→error-result
normalisation is its own pure step.** `finalize.rs:39-61` converts `Result<ToolResult,
ToolError>` into base fields, and `AfterToolCall` at `:64-80` shows those base fields to the hook.
pi names this `ExecutedToolCallOutcome { result, isError }` (`agent-loop.ts:569-572`, built by
`executePreparedToolCall` `:670-707`). Introduce `Executed { result: ToolResult, is_error: bool }`
with `From<Result<ToolResult, ToolError>>` — pure, no await. `args` is NOT an input of the fold
(it only feeds the hook context), so it is dropped from `fold_tool_outcome`'s parameters versus
the research sketch.

**Finding 5 — `finalize` survives as a three-line composer.** Both runtimes call
`self.finalize(assistant, ctx_messages, call, idx, args, outcome)` today (`exec.rs:218`, `:347`).
Keep that entry point and make its body `Executed::from → after_hook → fold_tool_outcome`, so the
call sites change only where the index now comes from `PreparedCall`. DoD "both runtimes call the
pair" is satisfied through the composer; the pair itself is the only thing that does work.

**Finding 6 — the identical `ToolExecutionEnd` literal is written four times** (`mod.rs:104-110`,
`exec.rs:71-77`, `:219-225`, `:351-357`) from `Finalized`'s fields. Once fields are private this
becomes the accessor `Finalized::end_event(&self) -> AgentEvent`; the fourth site (`exec.rs:219`)
currently pulls `call_id`/`tool_name` from the channel message instead — same values, now one
source. Likewise `fin.message.clone()` + `fin.message` at three sites becomes
`fin.into_message()`.

## SUBTASK1 — `tools/finalized.rs`: the leaf module that owns the constructor

Create [`crates/cyrup-agent/src/agent/run/tools/finalized.rs`](../../crates/cyrup-agent/src/agent/run/tools/finalized.rs):

```rust
//! The one finalized tool-call record. It lives in its own leaf module so that [`Finalized::new`]
//! is the ONLY way to build one: Rust field privacy is module-tree scoped, and a struct declared
//! in `tools/mod.rs` with private fields is still literal-constructible from `exec.rs` and
//! `preflight.rs` (its children) — which is exactly how a `source_index: 0` placeholder got
//! written by one producer and patched by two of its three consumers.

use crate::agent::message::result_value_of;
use crate::event::{AgentEvent, ToolResultMessage};
use cyrup_core::TerminateHint;
use serde_json::Value;

/// A tool call's settled result: the transcript message the batch will return, the index of the
/// call it answers, and the `tool_execution_end.result` payload derived from both.
pub(super) struct Finalized {
    source_index: usize,
    /// `AgentToolResult.terminate?` (`packages/agent/src/types.ts:354-368`) —
    /// [`TerminateHint::Unspecified`] is pi's `undefined`, i.e. the key is absent from the emitted
    /// `result` and the call does not contribute a vote to `shouldTerminateToolBatch`
    /// (`agent-loop.ts:582-584`). AGENT-009. Runtime-only: not a field of the persisted message.
    terminate: TerminateHint,
    result_value: Value,
    message: ToolResultMessage,
}

impl Finalized {
    /// The only constructor. `source_index` is the position of the answered call in the assistant
    /// message's tool-call list; `result_value` is derived here so it can never disagree with
    /// `message` (Pi emits `result: finalized.result` verbatim, `emitToolExecutionEnd`,
    /// `agent-loop.ts:763-771`).
    pub(super) fn new(source_index: usize, message: ToolResultMessage, terminate: TerminateHint) -> Self {
        let result_value = result_value_of(
            &message.content,
            &message.details,
            message.usage.as_ref(),
            &message.added_tool_names,
            terminate,
        );
        Self { source_index, terminate, result_value, message }
    }

    pub(super) fn source_index(&self) -> usize { self.source_index }

    pub(super) fn terminate(&self) -> TerminateHint { self.terminate }

    /// The `tool_execution_end` event for this result — the one place the literal is written.
    pub(super) fn end_event(&self) -> AgentEvent {
        AgentEvent::ToolExecutionEnd {
            tool_call_id: self.message.tool_call_id.clone(),
            tool_name: self.message.tool_name.clone(),
            result: self.result_value.clone(),
            is_error: self.message.is_error,
        }
    }

    pub(super) fn into_message(self) -> ToolResultMessage { self.message }
}
```

`result_value_of` is `pub(super)` in `agent::message` (visible to `agent` and every descendant,
which includes `tools::finalized`); `TerminateHint` is `Copy` (both producers already move it
twice). `pub(super)` on the struct and its methods makes them visible throughout `tools` and its
children, which is every user.

In [`tools/mod.rs`](../../crates/cyrup-agent/src/agent/run/tools/mod.rs): add `mod finalized;`
(alphabetical, after `mod finalize;`) and `use finalized::Finalized;` (a private `use` is
importable by the children as `super::Finalized`, exactly as `Prep` is today). **Delete** the
`struct Finalized { .. }` block at `:27-39` and its comment. Remove `ToolCallId`, `ToolResult`,
`ToolError`, `Value`… only if they become unused — `ToolRuntimeMsg` (`:22-25`) still uses
`ToolCallId`, `ToolUpdate`, `ToolResult`, `ToolError`; `PreparedCall` (SUBTASK2) uses `Arc<dyn
Tool>`, `Value`, `ToolCallId`; `fail_truncated_tool_calls` uses `Value`. Let the compiler's
unused-import warnings be the guide; the DoD is zero warnings from these files.

## SUBTASK2 — `PreparedCall`, `Prep::Ready(PreparedCall)`, and the index at preflight

**`tools/mod.rs`.** Replace `enum Prep` (`:41-46`) with:

```rust
/// One prepared-but-not-yet-started call — pi's `PreparedToolCall` (`agent-loop.ts:556-561`:
/// `{ kind: "prepared", toolCall, tool, args }`) plus the index of the call it answers, captured
/// once at preflight instead of re-derived by each runtime. The Rust analogue of the deferred
/// `finalizedCalls.push(async () => …)` closure (`:522-533`).
pub(super) struct PreparedCall {
    source_index: usize,
    tool: Arc<dyn Tool>,
    args: Value,
    call_id: ToolCallId,
    tool_name: String,
}

enum Prep {
    /// Boxed: `Finalized` embeds a whole `ToolResultMessage` and dwarfs the `Ready` arm, so an
    /// unboxed variant makes every `Prep` (including the common prepared-call case) pay for it.
    Immediate(Box<Finalized>),
    Ready(PreparedCall),
}
```

Fields stay private: `exec.rs` and `preflight.rs` are children of `tools` and see them (that is
the intended sharing — the record is internal to the pipeline).

**`tools/preflight.rs`.**
- `use super::{Finalized, Prep};` → `use super::{Finalized, Prep, PreparedCall};`. Drop
  `result_value_of` from the `crate::agent::message` import (only `empty_details` remains used).
- `prepare` (`:19-24`) gains the index: `pub(super) async fn prepare(&self, assistant:
  &AssistantMessage, ctx_messages: &[Arc<AgentMessage>], call: &ToolCall, source_index: usize)
  -> Prep`.
- Every one of the SIX `self.immediate_error(call, …)` calls in `prepare` (`:32`, `:46`, `:77`,
  `:85`, `:90`, `:113`) becomes `self.immediate_error(call, source_index, …)` — same message and
  `terminate` arguments, index inserted second.
- `:116` `Prep::Ready { tool, args }` →
  `Prep::Ready(PreparedCall { source_index, tool, args, call_id: call.id.clone(), tool_name: call.name.clone() })`.
- `immediate_error` (`:128-162`): signature becomes
  `pub(super) fn immediate_error(&self, call: &ToolCall, source_index: usize, msg: impl Into<SharedStr>, terminate: TerminateHint) -> Finalized`.
  The `message` literal (`:136-150`) is unchanged. Replace the `Finalized { source_index: 0, … }`
  literal (`:153-161`) and the comment above it (`:151-152`, keep the comment) with
  `Finalized::new(source_index, message, terminate)`.

**`tools/mod.rs` `fail_truncated_tool_calls`** (`:85-118`) — the producer that never patched:
`for call in calls` → `for (idx, call) in calls.iter().enumerate()`; `self.immediate_error(call,
format!(…), TerminateHint::Unspecified)` → `self.immediate_error(call, idx, format!(…),
TerminateHint::Unspecified)`. The `ToolExecutionEnd` literal (`:104-110`) →
`self.emit(fin.end_event()).await?;`. Lines `:111-114` →

```rust
            let message = fin.into_message();
            let msg = AgentMessage::ToolResult(message.clone());
            self.emit(AgentEvent::MessageStart { message: msg.clone() }).await?;
            self.emit(AgentEvent::MessageEnd { message: msg }).await?;
            tool_results.push(message);
```

## SUBTASK3 — `tools/finalize.rs`: `Executed` (pure) → `after_hook` (shell) → `fold_tool_outcome` (pure)

Rewrite the file body to this structure (module doc: "Tool result finalization: normalise the
executed outcome, run `after_tool_call`, and fold the replace-not-merge table into a
`Finalized`"). Imports: `use super::Finalized; use crate::agent::message::empty_details; use
crate::agent::run::RunCtx; use crate::agent::util::now_millis; use crate::error::HookError; use
crate::event::{AgentMessage, ToolResultMessage}; use crate::hooks::{AfterOverride, AfterToolCall,
AgentContextView}; use cyrup_core::{AssistantMessage, Content, TerminateHint, ToolCall,
ToolError, ToolResult}; use serde_json::Value; use std::sync::Arc;`.

```rust
/// Pi `ExecutedToolCallOutcome { result, isError }` (`agent-loop.ts:569-572`): the tool's own
/// outcome after the throw→error-result conversion, BEFORE `after_tool_call` sees it.
pub(super) struct Executed {
    pub(super) result: ToolResult,
    pub(super) is_error: bool,
}

impl From<Result<ToolResult, ToolError>> for Executed {
    fn from(outcome: Result<ToolResult, ToolError>) -> Self {
        match outcome {
            // AGENT-009 — `terminate` is optional upstream (`AgentToolResult.terminate?`,
            // types.ts:354-368) and `TerminateHint` carries all three of its values through
            // unchanged: `Unspecified` puts no key on the wire, `Continue` puts an explicit `false`.
            Ok(result) => Self { result, is_error: false },
            // A throwing TOOL yields `createErrorToolResult(...)` (`agent-loop.ts:700-703`
            // @v0.83.0), i.e. `details: {}` and no `terminate`.
            Err(e) => Self {
                result: ToolResult {
                    content: vec![Content::text(e.to_string())],
                    details: Some(empty_details()),
                    usage: None,
                    added_tool_names: Vec::new(),
                    terminate: TerminateHint::Unspecified,
                },
                is_error: true,
            },
        }
    }
}

impl RunCtx {
    /// Finalize one executed call: normalise → `after_tool_call` → fold. The two runtimes call
    /// this; it composes the three steps and does nothing else.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn finalize(
        &self,
        assistant: &AssistantMessage,
        ctx_messages: &[Arc<AgentMessage>],
        call: &ToolCall,
        source_index: usize,
        args: Value,
        outcome: Result<ToolResult, ToolError>,
    ) -> Finalized {
        let executed = Executed::from(outcome);
        let hook = self.after_hook(assistant, ctx_messages, call, &args, &executed).await;
        fold_tool_outcome(call, source_index, executed, hook)
    }

    /// The shell: the one await in finalization. Builds the read-side view of the executed
    /// result (Pi `AfterToolCallContext`, types.ts:100-113) and returns the hook's verdict
    /// UNINTERPRETED — `Err` is a third outcome of the fold, not an absence (see
    /// [`fold_tool_outcome`]).
    pub(super) async fn after_hook(
        &self,
        assistant: &AssistantMessage,
        ctx_messages: &[Arc<AgentMessage>],
        call: &ToolCall,
        args: &Value,
        executed: &Executed,
    ) -> Result<Option<AfterOverride>, HookError> {
        let ctx = AfterToolCall {
            tool_name: &call.name,
            tool_call_id: &call.id,
            args,
            content: &executed.result.content,
            details: executed.result.details.as_ref(),
            usage: executed.result.usage.as_ref(),
            is_error: executed.is_error,
            terminate: executed.result.terminate,
            assistant_message: assistant,
            tool_call: call,
            context: AgentContextView {
                system_prompt: &self.system_prompt,
                messages: ctx_messages,
                tools: &self.tools,
            },
        };
        self.hooks.after_tool_call(ctx, self.cancel.child()).await
    }
}

/// The pure fold: pi `finalizeExecutedToolCall`'s three-way table (`agent-loop.ts:724-750`) as a
/// total function of its inputs, with no await and no `self`. Replace-not-merge per field
/// (R-02-025). `added_tool_names` rides through untouched on the override arm: pi spreads
/// `{...result}` before applying the hook's explicit fields (`:736-742`) and `addedToolNames` is
/// not one of them, so no hook can set or clear it — only the throw arm drops it.
pub(super) fn fold_tool_outcome(
    call: &ToolCall,
    source_index: usize,
    executed: Executed,
    hook: Result<Option<AfterOverride>, HookError>,
) -> Finalized {
    let Executed { result, mut is_error } = executed;
    // Exhaustive destructure: a field added to `ToolResult` must be placed in this table.
    let ToolResult { mut content, mut details, mut usage, mut added_tool_names, mut terminate } =
        result;
    match hook {
        Ok(Some(ov)) => {
            /* the five `if let Some(x) = ov.x { x = … }` arms and their comments, verbatim from
               today's `finalize.rs:85-104` */
        }
        Ok(None) => {}
        Err(e) => {
            /* today's `finalize.rs:108-119` verbatim: content = text(e), details = Some({}),
               usage = None, added_tool_names = Vec::new(), is_error = true,
               terminate = Unspecified — with its comments */
        }
    }
    let message = ToolResultMessage {
        tool_call_id: call.id.clone(),
        tool_name: call.name.clone(),
        content,
        details,
        usage,
        added_tool_names,
        is_error,
        // Pi `createToolResultMessage` stamps every tool result with `Date.now()`
        // (agent-loop.ts:741); this reaches the wire payload via `convert_to_llm`.
        timestamp: now_millis(),
    };
    Finalized::new(source_index, message, terminate)
}
```

The comment blocks marked "verbatim" are the ones at today's `finalize.rs:29-31`, `:40-42`,
`:51-52`, `:91-93`, `:100-101`, `:108-114` — move them, do not paraphrase them; they carry the
pi citations QA checks. `HookError` is `crate::error::HookError` (the hook trait's own error
type, [`hooks.rs:245-249`](../../crates/cyrup-agent/src/hooks.rs)).

## SUBTASK4 — the two runtimes consume the new shapes (`tools/exec.rs`)

- Imports: `use super::{Batch, Finalized, Prep, PreparedCall, ToolRuntimeMsg};`; the
  `cyrup_core` line loses `Tool` and `ToolCallId` (both only served `Deferred`) →
  `use cyrup_core::{AssistantMessage, ToolCall, ToolError, ToolUpdate, ToolUpdateSink};`.
- **Delete** `struct Deferred` and its doc (`:49-57`); `:58` → `let mut deferred: Vec<PreparedCall> = Vec::new();`.
- `:67-92` becomes:

```rust
            match self.prepare(assistant, ctx_messages, call, idx).await {
                Prep::Immediate(fin) => {
                    self.emit(fin.end_event()).await?;
                    if let Some(slot) = finalized.get_mut(idx) {
                        *slot = Some(*fin);
                    }
                }
                // Prepared only — the body is NOT started here. Pi defers it to the
                // post-loop `Promise.all` so a later call's `before_tool_call` cannot
                // still be open while this one runs.
                Prep::Ready(prepared) => deferred.push(prepared),
            }
```

- `:107` `for Deferred { source_index, tool, args, call_id, tool_name } in deferred {` →
  `for PreparedCall { source_index, tool, args, call_id, tool_name } in deferred {` (body
  unchanged).
- `:217-228` becomes:

```rust
                    let fin =
                        self.finalize(assistant, ctx_messages, &call, source_index, args, outcome).await;
                    self.emit(fin.end_event()).await?;
                    if let Some(slot) = finalized.get_mut(fin.source_index()) {
                        *slot = Some(fin);
                    }
```

  (`call_id` and `tool_name` from the channel message are still used by the `find` and the
  defensive stand-in at `:204-216`; nothing becomes unused.)
- `:244-245` → `present.iter().all(|f| f.terminate().requested())`; `:247-252` →

```rust
        for fin in present {
            let message = fin.into_message();
            let msg = AgentMessage::ToolResult(message.clone());
            self.emit(AgentEvent::MessageStart { message: msg.clone() }).await?;
            self.emit(AgentEvent::MessageEnd { message: msg }).await?;
            tool_results.push(message);
        }
```

- Sequential, `:275-281`:

```rust
            let fin = match self.prepare(assistant, ctx_messages, call, idx).await {
                Prep::Immediate(fin) => *fin,
                Prep::Ready(PreparedCall { source_index, tool, args, .. }) => {
```

  and `:347` → `self.finalize(assistant, ctx_messages, call, source_index, args, outcome).await`
  (`call.id.clone()` at `:300` and the `call.*` uses inside the update loop stay — they are the
  same call; `call_id`/`tool_name` from the record are the parallel runtime's, hence the `..`).
- `:351-357` → `self.emit(fin.end_event()).await?;`; `:358` → `if !fin.terminate().requested()`;
  `:361-364` → the same `into_message()` five-liner as above (with `produced += 1` after it).

## Definition of done

- `Finalized` is declared in `tools/finalized.rs` with four private fields; `Finalized::new` is
  its only constructor: `grep -rn "Finalized {" crates/cyrup-agent/src/agent/run/tools/` matches
  nothing outside `finalized.rs`; `grep -rn "source_index: 0\|\.source_index = "` in that
  directory matches nothing.
- `AgentEvent::ToolExecutionEnd {` appears in `tools/` only inside `Finalized::end_event`.
- `struct Deferred` is gone; `PreparedCall` is matched by name in both `execute_parallel` and
  `execute_sequential`; `prepare` and `immediate_error` take `source_index`;
  `fail_truncated_tool_calls` passes its enumerate index.
- `fold_tool_outcome` is a free non-async `pub(super) fn` taking
  `Result<Option<AfterOverride>, HookError>` with a three-arm `match`; `Executed: From<Result<ToolResult, ToolError>>`
  is non-async; the only `self.hooks.after_tool_call(..).await` in `finalize.rs` is inside
  `after_hook`; `finalize` is the three-line composer both runtimes call.
- Typestate across the parallel runtime is **not** introduced — completion order is genuinely
  dynamic (`JoinSet` + channel).
- No file outside `crates/cyrup-agent/src/agent/run/tools/` changes; no test file changes.
- `cargo check --workspace --all-targets --features test-fixtures` clean;
  `cargo test --workspace --features test-fixtures --no-fail-fast` all green (8466 baseline; the
  27 `agent_loop.rs` and 15 `tool_result_model.rs` tests unedited);
  `cargo clippy --workspace --all-targets --features test-fixtures` adds no warning (the one
  pre-existing `cyrup-tui` `question_mark` warning is not this task's);
  `cargo doc -p cyrup-agent --no-deps` exits 0 (the workspace denies broken intra-doc links —
  `[`Finalized::new`]` and `[`fold_tool_outcome`]` in the docs above resolve within their
  modules).

## Research notes

Research §3 F6. The batch fold at `exec.rs:235-245` (`all_terminate` over the FILLED slots,
AGENT-015) is untouched by this step except for the accessor spelling. `ctx_state_and_abort.rs:161`
quotes an old `immediate_error(call, "Operation aborted")` in a comment describing a fixed bug —
a test-file comment, left alone. pi parity anchors: `PreparedToolCall` `:556-561`,
`ExecutedToolCallOutcome` `:569-572`, `executePreparedToolCall` `:670-707`,
`finalizeExecutedToolCall` `:713-757`, `createErrorToolResult` `:760-765`, all at
[`tmp/pi/packages/agent/src/agent-loop.ts`](../../tmp/pi/packages/agent/src/agent-loop.ts).

No tests to be written — another team owns tests. No benchmarks to be written.
