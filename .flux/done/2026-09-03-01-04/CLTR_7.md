---
stage: qa
status: completed
updated: 2026-09-03 23:20
aug_against: branch cbdd72a (CLTR_1..6 landed) — uncapped sweep of every `impl Hooks for`, every before/after_tool_call impl, BeforeOutcome/AfterOverride/HookError uses and `subscriber(` callers across all 23 crates incl. tests (614 hits, tmp/cltr7_sweep.txt)
---

# CLTR_7 — `Hooks` failure-mode map in the signatures (F7)

OBJECTIVE: Make the `Hooks` trait say what its comments say: the two per-call hooks
(`before_tool_call`, `after_tool_call`) return domain outcome enums whose `Failed` variant is an
expected per-call outcome, while the four run-aborting hooks keep `Result<_, HookError>`. Broadest
trait break in the plan, smallest guarantee — scheduled last. Requires CLTR_1 (`TerminateHint` on
`Block`, landed) and CLTR_4 (the pure `fold_tool_outcome`, landed). Source: research §3 F7
([`.flux/research/CORE_LOOP_TYPE_REVIEW.md`](../research/CORE_LOOP_TYPE_REVIEW.md)).

## Aug findings that change the plan (read first)

**Finding 1 — the break reaches 13 test-local impls in six files, not one.** The sweep finds 30
`impl Hooks for` blocks; the two per-call hooks are overridden in exactly: `cyrup-ext/src/hooks.rs`
(2), `cyrup-session-svc/src/hooks.rs` (2), the trait's own defaults (2), and **13 test impls** —
`agent_loop.rs` (4), `area02_backlog.rs` (1), `hook_failure_text.rs` (2), `model_boundary.rs` (1),
`round2_parity.rs` (1), `tool_result_model.rs` (4). A trait break cannot leave them unedited; the
edits are purely mechanical (return type + wrapping) and every assertion stays. SUBTASK5 lists
each one with its exact old→new text. `FailingConvert` (`hook_failure_text.rs:187`) overrides
`convert_to_llm`, which keeps `Result` — untouched. The other 15 test impls override only the four
run-aborting hooks — untouched.

**Finding 2 — the preflight match flattens to four arms in pi's order, with no `unreachable!`.**
Today [`preflight.rs:72-129`](../../crates/cyrup-agent/src/agent/run/tools/preflight.rs) is
`match before { Err(e) => …, Ok(outcome) => { abort check; match outcome { Block, Proceed } } }`.
pi's order (`agent-loop.ts:616-662` @v0.83.0): the hook throw returns FIRST with no abort check
(the `catch` at `:657-662`), then `if (signal?.aborted)` out-votes a block (`:629-635`), then the
block arm, then the proceed arm with its SECOND abort check (`:644-650`). With the three-variant
enum this is one flat `match` whose second arm is an or-pattern with a guard:
`BeforeOutcome::Block { .. } | BeforeOutcome::Proceed if self.cancel.is_cancelled() => aborted`.
An or-pattern with a guard is legal because `Block { .. }` binds nothing; the later `Block {
reason, terminate }` arm binds. Behaviour is byte-identical; the nested `Ok` scope goes.

**Finding 3 — `fold_tool_outcome` already has the three arms; only their spelling changes.**
CLTR_4 made the after-hook verdict a value (`Result<Option<AfterOverride>, HookError>`) folded by
a pure function with `Ok(Some)`/`Ok(None)`/`Err` arms
([`finalize.rs:101-142`](../../crates/cyrup-agent/src/agent/run/tools/finalize.rs)). `AfterOutcome`
is that Result's three cases with names — `Override`/`Keep`/`Failed` — so `after_hook` returns
it, the fold takes it, and the arms rename. Zero behaviour change (R-02-025/050).

**Finding 4 — `ExtHooks` never fails, so its `Failed` arm does not exist; `PolicyHooks` delegates.**
[`cyrup-ext/src/hooks.rs:33-68`](../../crates/cyrup-ext/src/hooks.rs) wraps every return in `Ok`;
the EXT-029 arm (`_ if cancel.is_cancelled() => Ok(BeforeOutcome::Proceed)`) and the
`Reduced::Blocked { reason, terminate, .. }` pass-through (already `TerminateHint`, CLTR_1) simply
drop the `Ok`. [`cyrup-session-svc/src/hooks.rs:190-224`](../../crates/cyrup-session-svc/src/hooks.rs)
`before_tool_call` ends with `self.inner.before_tool_call(ctx, cancel).await` — not a `?`; with the
enum return that tail call is already the right type, and its early returns drop their `Ok`.
`after_tool_call` is a one-line delegate. The `terminate: false` the task mentions was already
`TerminateHint::Unspecified` (CLTR_1).

**Finding 5 — `subscriber(&self, _cancel)` has SIX callers, five of them tests.**
[`cyrup-ext/src/facade.rs:586`](../../crates/cyrup-ext/src/facade.rs) documents the parameter as
ignored (EXT-061) and names its one production caller,
[`cyrup-session-svc/src/builder.rs:1537`](../../crates/cyrup-session-svc/src/builder.rs)
(`session_cancel` stays in use afterwards — it is handed to `AgentSession::from_parts`). The five
test callers are all in `cyrup-ext/src/tests/native_dispatch.rs` (`:102`, `:164`, `:578`, `:952`,
`:1049`); at `:952` the local `cancel` is still used at `:962`, so only the argument goes.

**Finding 6 — the enums need the crate-root re-export.** `BeforeOutcome` is exported from
[`lib.rs:30-33`](../../crates/cyrup-agent/src/lib.rs); `AfterOutcome` joins it. Both in-tree impls
import from the crate root.

## SUBTASK1 — the outcome enums and the trait (`cyrup-agent/src/hooks.rs`)

Replace the `BeforeOutcome` enum (`:49-66`, doc line included) with:

```rust
/// Outcome of [`Hooks::before_tool_call`] (func-02 R-02-021). Every variant is an EXPECTED per-call
/// outcome: none of them aborts the run. pi's `beforeToolCall` returns `BeforeToolCallResult |
/// undefined` (`packages/agent/src/types.ts:61-69`, `:277`), and a throw is caught per call
/// (`agent-loop.ts:657-662` @v0.83.0) into an error tool result — which is why [`Self::Failed`]
/// is a variant here and not a `Result::Err`: the four run-aborting hooks keep `Result`, and the
/// signature now says which is which.
pub enum BeforeOutcome {
    Proceed,
    Block {
        reason: Option<String>,
        /// (AGENT-022 doc, verbatim from today's `:53-63`)
        terminate: TerminateHint,
    },
    /// The hook itself failed. The loop degrades exactly as for a pi `beforeToolCall` throw: an
    /// error tool result carrying the error's own text (`createErrorToolResult(error.message)`,
    /// `agent-loop.ts:657-662`), no `terminate`, no abort check first (func-02 R-02-050).
    Failed(HookError),
}
```

Add, directly after `AfterOverride` (`:104-117`):

```rust
/// Outcome of [`Hooks::after_tool_call`] (func-02 R-02-025). pi's `afterToolCall` returns
/// `AfterToolCallResult | undefined` (`types.ts:84-93`, `:292`) and a throw is caught per call
/// (`agent-loop.ts:747-750`) — three outcomes, all expected, none run-aborting.
pub enum AfterOutcome {
    /// `undefined` upstream: the tool's own result stands.
    Keep,
    /// Replace-not-merge per field (`afterResult.x ?? result.x`, `agent-loop.ts:738-745`).
    Override(AfterOverride),
    /// The hook itself failed: the WHOLE result becomes an error result carrying the error's own
    /// text, with `usage` and `added_tool_names` dropped and `terminate` cleared
    /// (`createErrorToolResult(error.message)`, `agent-loop.ts:747-750`; R-02-050).
    Failed(HookError),
}
```

Trait (`:235-250`): `before_tool_call` returns `BeforeOutcome` with default body
`BeforeOutcome::Proceed`; `after_tool_call` returns `AfterOutcome` with default body
`AfterOutcome::Keep`. Their doc comments gain one sentence each: "Cannot abort the run — every
outcome, [`BeforeOutcome::Failed`] included, is a per-call result (func-02 R-02-050)." The other
four hooks (`convert_to_llm` `:222`, `transform_context` `:227`, `prepare_next_turn` `:265`,
`should_stop_after_turn` `:279`) are byte-identical. `HookError` is already imported in `hooks.rs`
(the four `Result` hooks use it).

[`lib.rs:30-33`](../../crates/cyrup-agent/src/lib.rs): add `AfterOutcome` to the `hooks::{..}`
re-export list (alphabetical: `default_convert_to_llm, AfterOutcome, AfterOverride, …`).

## SUBTASK2 — the two loop call sites

**[`tools/preflight.rs`](../../crates/cyrup-agent/src/agent/run/tools/preflight.rs)** — replace
`:72-129` (`match before { … }` through the `Ok(outcome)` arm's closing braces; the function's
final `}` at `:130` stays) with:

```rust
        match before {
            // Pi's `prepareToolCall` wraps the `beforeToolCall` await in the same try that guards
            // argument preparation/validation, and its catch returns
            // `createErrorToolResult(error instanceof Error ? error.message : String(error))`
            // (agent-loop.ts:657-662) — the hook's OWN text reaches the model, exactly as the
            // validation failure two arms up already does. No abort check first: the catch
            // returns before `if (signal?.aborted)` is reached.
            BeforeOutcome::Failed(e) => Prep::Immediate(Box::new(
                self.immediate_error(call, source_index, e.to_string(), TerminateHint::Unspecified),
            )),
            // AGENT-012 — pi checks the signal the instant the hook returns and BEFORE it looks
            // at `beforeResult.block` (`agent-loop.ts:629-635` @v0.83.0), so an abort landing
            // during the hook OUT-VOTES a block and the transcript attributes the stop to the
            // user rather than to policy.
            BeforeOutcome::Block { .. } | BeforeOutcome::Proceed if self.cancel.is_cancelled() => {
                Prep::Immediate(Box::new(
                    self.immediate_error(call, source_index, "Operation aborted", TerminateHint::Unspecified),
                ))
            }
            BeforeOutcome::Block { reason, terminate } => Prep::Immediate(Box::new(self.immediate_error(
                call,
                source_index,
                // (AGENT-010 + AGENT-032(a) comment, verbatim from today's `:94-100`)
                reason
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "Tool execution was blocked".to_string()),
                // (AGENT-022 comment, verbatim from today's `:102-103`)
                terminate,
            ))),
            // Args mutated in place are executed as-is, WITHOUT re-validation (R-02-022).
            BeforeOutcome::Proceed => {
                // pi's SECOND abort check, outside the `if (config.beforeToolCall)` block
                // (`agent-loop.ts:644-650` @v0.83.0, `:648` @v0.84.1).
                if self.cancel.is_cancelled() {
                    Prep::Immediate(Box::new(
                        self.immediate_error(call, source_index, "Operation aborted", TerminateHint::Unspecified),
                    ))
                } else {
                    Prep::Ready(PreparedCall {
                        source_index,
                        tool,
                        args,
                        call_id: call.id.clone(),
                        tool_name: call.name.clone(),
                    })
                }
            }
        }
```

The `let before = { … self.hooks.before_tool_call(ctx, self.cancel.child()).await };` block
(`:56-71`) is unchanged.

**[`tools/finalize.rs`](../../crates/cyrup-agent/src/agent/run/tools/finalize.rs)** —
- import: `use crate::hooks::{AfterOverride, AfterToolCall, AgentContextView};` →
  `use crate::hooks::{AfterOutcome, AfterToolCall, AgentContextView};` and drop
  `use crate::error::HookError;` (no longer named in this file).
- `after_hook` (`:67-93`): return type `-> AfterOutcome`; its doc's last sentence becomes
  "returns the hook's verdict UNINTERPRETED — [`AfterOutcome::Failed`] is a third outcome of the
  fold, not an absence (see [`fold_tool_outcome`])." Body unchanged (the trait call is the value).
- `fold_tool_outcome` (`:101-106`): parameter `hook: Result<Option<AfterOverride>, HookError>,` →
  `hook: AfterOutcome,`; arms `Ok(Some(ov)) =>` → `AfterOutcome::Override(ov) =>`,
  `Ok(None) => {}` → `AfterOutcome::Keep => {}`, `Err(e) =>` → `AfterOutcome::Failed(e) =>`. Arm
  bodies and comments unchanged.

## SUBTASK3 — the two in-tree impls

**[`cyrup-session-svc/src/hooks.rs`](../../crates/cyrup-session-svc/src/hooks.rs)** — import
(`:10-13`): add `AfterOutcome`, remove `AfterOverride` (no longer named) →
`use cyrup_agent::{AfterOutcome, AfterToolCall, AgentMessage, BeforeOutcome, BeforeToolCall,
HookError, Hooks, PostTurn, TurnUpdate};` (`HookError` stays: the four `Result` hooks use it).
`before_tool_call` (`:190-216`): return `-> BeforeOutcome`; the two
`return Ok(BeforeOutcome::Block { … });` (`:203`, `:208`) → `return BeforeOutcome::Block { … };`;
the tail `self.inner.before_tool_call(ctx, cancel).await` is unchanged. `after_tool_call`
(`:218-224`): return `-> AfterOutcome`; body unchanged.

**[`cyrup-ext/src/hooks.rs`](../../crates/cyrup-ext/src/hooks.rs)** — import (`:10-13`):
`use cyrup_agent::{AfterOutcome, AfterOverride, AfterToolCall, AgentMessage, BeforeOutcome,
BeforeToolCall, Hooks};` and delete `use cyrup_agent::HookError;` (`:13`) unless the compiler
says another hook in the file still names it. `before_tool_call` (`:33-68`): return
`-> BeforeOutcome`; the four `Ok(BeforeOutcome::…)` become bare (`:52`, `:57`, `:63`, `:67`);
comments unchanged except the EXT-029 quote of the agent's old code
(`` `Ok(BeforeOutcome::Proceed) => if self.cancel.is_cancelled()`, agent.rs:1026-1028 ``) →
`` `BeforeOutcome::Block { .. } | BeforeOutcome::Proceed if self.cancel.is_cancelled()`, tools/preflight.rs ``.
`after_tool_call` (`:72-136`): return `-> AfterOutcome`; `return Ok(None);` → `return AfterOutcome::Keep;`;
`Ok(if changed { Some(over) } else { None })` → `if changed { AfterOutcome::Override(over) } else { AfterOutcome::Keep }`;
`_ => Ok(None),` → `_ => AfterOutcome::Keep,`.

## SUBTASK4 — incidental: `ExtensionHost::subscriber` drops its ignored parameter

[`cyrup-ext/src/facade.rs:579-588`](../../crates/cyrup-ext/src/facade.rs): signature
`pub fn subscriber(&self) -> Arc<dyn EventSubscriber>`; rewrite the EXT-061 paragraph to: "EXT-061:
no cancel token is taken. pi passes the run's signal to each listener at the emit (`await
listener(event, signal)`, `packages/agent/src/agent.ts:574` @v0.83.0) and keeps no
subscriber-lifetime token; `ExtSubscriber` correspondingly holds none, and the per-event token
`EventSubscriber::on_event` receives is what every dispatched handler races against."
[`cyrup-session-svc/src/builder.rs:1537`](../../crates/cyrup-session-svc/src/builder.rs):
`ext_host.subscriber(session_cancel.clone())` → `ext_host.subscriber()` (`session_cancel` stays —
`from_parts` takes it). `cyrup-ext/src/tests/native_dispatch.rs` `:102`, `:164`, `:578`, `:1049`:
`host.subscriber(CancelToken::new())` → `host.subscriber()`; `:952`
`host.subscriber(cancel.clone())` → `host.subscriber()` (`cancel` is still used at `:962`). If
`CancelToken` becomes unused in that test file the compiler will say so; it will not — other
calls use it.

## SUBTASK5 — the 13 test impls (mechanical; assertions unchanged)

Return-type substitutions, applied to the listed impls only: `-> Result<BeforeOutcome, HookError>`
→ `-> BeforeOutcome`; `-> Result<Option<AfterOverride>, HookError>` → `-> AfterOutcome`
(`tool_result_model.rs:593` spells it `Result<crate::BeforeOutcome, HookError>` → `crate::BeforeOutcome`).
Body substitutions:

| File | Impl (line) | Old → new body line |
|---|---|---|
| `agent_loop.rs` | `BlockHook` `:468` | `Ok(BeforeOutcome::Block { … })` → `BeforeOutcome::Block { … }` |
| `agent_loop.rs` | `DetailsHook` `:518` | `Ok(Some(AfterOverride { … }))` → `AfterOutcome::Override(AfterOverride { … })` |
| `agent_loop.rs` | `TerminateHook` `:567` | `Ok(Some(AfterOverride { … }))` → `AfterOutcome::Override(AfterOverride { … })` |
| `agent_loop.rs` | `SlowGateHook` `:1222` | `Ok(BeforeOutcome::Proceed)` → `BeforeOutcome::Proceed` |
| `area02_backlog.rs` | `BlockWith` `:283` | `Ok(BeforeOutcome::Block { … })` → `BeforeOutcome::Block { … }` |
| `hook_failure_text.rs` | `FailingBefore` `:59` | `Err(HookError::new("policy store …"))` → `BeforeOutcome::Failed(HookError::new("policy store …"))` |
| `hook_failure_text.rs` | `FailingAfter` `:104` | `Err(HookError::new("redaction …"))` → `AfterOutcome::Failed(HookError::new("redaction …"))` |
| `model_boundary.rs` | `InspectHook` `:401` | `Ok(BeforeOutcome::Proceed)` → `BeforeOutcome::Proceed` |
| `round2_parity.rs` | `TerminateAndCount` `:142` | `Ok(Some(AfterOverride { … }))` → `AfterOutcome::Override(AfterOverride { … })` |
| `tool_result_model.rs` | `UsagePatchHook` `:217` | `Ok(Some(AfterOverride {` … `}))` → `AfterOutcome::Override(AfterOverride {` … `})` |
| `tool_result_model.rs` | `ContentOnlyHook` `:271` | same shape as above |
| `tool_result_model.rs` | `ThrowingHook` `:317` | `Err(HookError::new("boom"))` → `AfterOutcome::Failed(HookError::new("boom"))` |
| `tool_result_model.rs` | `GateLateToolHook` `:589` | both `Ok(crate::BeforeOutcome::…)` → bare |

Each file's `use` list gains `AfterOutcome` where an after-hook impl lives (`agent_loop.rs`,
`hook_failure_text.rs`, `round2_parity.rs`, `tool_result_model.rs`) and drops `HookError` only if
the compiler reports it unused (`hook_failure_text.rs` and `tool_result_model.rs` still construct
it; `agent_loop.rs`/`round2_parity.rs` have other `Result` hooks that may name it). No assertion,
fixture, or test name changes.

## Definition of done

- `grep -n "async fn before_tool_call\|async fn after_tool_call" -A4 crates/cyrup-agent/src/hooks.rs`
  shows `-> BeforeOutcome` and `-> AfterOutcome`; neither returns `Result`. The four other hooks'
  signatures are byte-identical (`convert_to_llm`, `transform_context`, `prepare_next_turn`,
  `should_stop_after_turn`).
- `BeforeOutcome::Failed(HookError)` and `AfterOutcome::Failed(HookError)` exist; `AfterOutcome`
  is re-exported from `cyrup_agent`.
- `grep -rn "Result<BeforeOutcome\|Result<Option<AfterOverride>" crates` matches nothing.
- `preflight.rs` has one flat four-arm `match before` in the order Failed → aborted-out-votes-block
  → Block → Proceed; no `unreachable!`; the strings `"Operation aborted"` and `"Tool execution was
  blocked"` are unchanged.
- `fold_tool_outcome` matches `Override`/`Keep`/`Failed`; arm bodies unchanged.
- `ExtensionHost::subscriber` takes no parameter; six callers updated.
- The 13 test impls change only in return type and wrapping; every assertion and fixture is
  unchanged (`hook_failure_text.rs`'s four tests still assert the hook's own text reaches the
  model; EXT-029 behaviour in `cyrup-ext/src/hooks.rs:44-52` unchanged).
- `cargo check --workspace --all-targets --features test-fixtures` clean;
  `cargo test --workspace --features test-fixtures --no-fail-fast` all green (8466 baseline);
  `cargo clippy --workspace --all-targets --features test-fixtures` adds no warning (the one
  pre-existing `cyrup-tui` `question_mark` warning is not this task's);
  `cargo doc -p cyrup-agent -p cyrup-ext -p cyrup-session-svc --no-deps` exits 0 (intra-doc links
  `[`Self::Failed`]`, `[`Hooks::after_tool_call`]`, `[`AfterOutcome::Failed`]`,
  `[`fold_tool_outcome`]`, `[`BeforeOutcome::Failed`]` resolve as written).

## Research notes

Research §3 F7, §2 boundary B6. The WASM-facing `HookOutcome` in `cyrup-ext/src/contract.rs` is
the guest boundary and keeps its own shape apart from the shared `TerminateHint`; `Reduced::Blocked`
(`contract.rs:208`) already carries `TerminateHint`. pi anchors:
[`types.ts:61-69`](../../tmp/pi/packages/agent/src/types.ts) `BeforeToolCallResult`, `:84-93`
`AfterToolCallResult`, `:277`/`:292` the optional-result hook signatures;
[`agent-loop.ts:616-662`](../../tmp/pi/packages/agent/src/agent-loop.ts) `prepareToolCall` and
`:713-757` `finalizeExecutedToolCall`.

No tests to be written — another team owns tests. No benchmarks to be written.
