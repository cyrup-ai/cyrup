# 02 — cyrup-agent (the turn loop)

This area covers `cyrup/crates/cyrup-agent` — the run loop, tool-call preparation/execution/finalization, the state reducer, hooks and the proxy — measured against `pi/packages/agent/src/{agent-loop,agent,proxy}.ts` at pi v0.83.0, with a small amount of spill into `cyrup-core`'s message/tool model and `cyrup-session-svc`'s per-turn refresh hook where the loop-side half is the thing under test. Headline: the five loop-level items closed since the last baseline (AGENT-001/-002/-004/-005/-008) all survive an adversarial re-read at HEAD, and what remains open is a consistent band of small parity divergences — error-string text, error-path event completeness, and abort/hook ordering — plus one newly-found asymmetry where the parallel and sequential batch modes behave incomparably on a faulting tool. Re-baselined against HEAD `1806375` on 2026-08-03; every line reference below was re-read at that commit.

## Status since the c8bd2ab baseline

| ID | Status | Note |
|---|---|---|
| AGENT-001 | **closed** | 4091c86. `agent.rs:479-483` branches on `StopReason::Length` into `fail_truncated_tool_calls` (`agent.rs:840-872`), emitting pi's exact four-event order and a byte-identical error string (`pi/packages/agent/src/agent-loop.ts:381-405`). Discriminating regression test at `crates/cyrup-agent/tests/agent_loop.rs:948-1027`. |
| AGENT-002 | **closed** | 8854601. `agent.rs:1087-1179` is genuinely two-phase — the prep loop (`:1108-1144`) contains no `joinset.spawn`; all bodies spawn at `:1151-1179`. Matches pi's lazy-closure + `Promise.all` (`agent-loop.ts:522,540-542`). Positional-log test at `tests/agent_loop.rs:1105-1152`. |
| AGENT-003 | open | Severity downgraded medium → low: after 8854601 the receiver drains within microseconds of the first spawn, so only a synchronous >64-update burst is lost. Still lossy vs pi. |
| AGENT-004 | **closed** | f777e44. `added_tool_names` on `cyrup_core::ToolResult` (`crates/cyrup-core/src/tool.rs:40`), through `finalize`, event payload, transcript (`event.rs:63`) and the hand-written omit-when-empty wire serializer (`message.rs:544-566`). Producer at `crates/cyrup-ext/src/wrapper.rs:123-145`. The doc at `tool.rs:33-39` corrects the original mis-description: it is a cache-**placement** record, not a change to the active tool set. |
| AGENT-005 | **closed** | f777e44. `usage` on `ToolResult` (`tool.rs:32`), readable by `AfterToolCall` (`hooks.rs:67`), patchable replace-not-merge by `AfterOverride` (`hooks.rs:92`, applied `agent.rs:1024-1026`), matching `agent-loop.ts:738`. |
| AGENT-006 | open | Four hook/transform failure sites still discard `HookError` and emit placeholders. |
| AGENT-007 | open | `prepare_next_turn` / `should_stop_after_turn` errors still end the run with a bare `agent_end`. |
| AGENT-008 | **closed** | 6d29542. `Max` added to `ThinkingLevel`/`ModelThinkingLevel` (`crates/cyrup-core/src/message.rs:38,55`), handled at `:69,:87`; provider ladder `EXTENDED_THINKING_LEVELS` is now `[ModelThinkingLevel; 7]` (`crates/cyrup-provider/src/collection.rs:396-404`). |
| AGENT-009 | open | `details: None` vs pi's `{}`, plus unconditional `details`/`terminate` keys on the `tool_execution_end.result` payload. Now also reached via `fail_truncated_tool_calls`. |
| AGENT-010 | open | Two loop-generated tool-error strings still differ from pi's. |
| AGENT-011 | open | `state.error_message` gated on stop reason with a synthetic fallback pi never produces. |
| AGENT-012 | open | Pre-hook abort check, and abort ordered below block. |
| AGENT-013 | open | Proxy HTTP failures do not produce pi's `Proxy error: …` message. |
| AGENT-014 | open | No `pending` stop reason for in-flight assistant messages. |
| AGENT-015 | open | Aborted parallel batch: never-prepared slots veto termination. 8854601 explicitly left this alone. |

No pre-existing item's status changed other than the five closures above, each of which was attacked and survived. Minor citation corrections folded in silently: AGENT-005's upstream catch is `agent-loop.ts:744-747` (not `:697-703`); AGENT-007's `handleRunFailure` is `agent.ts:496-511`; AGENT-013's `error_terminal` is `proxy.rs:509-524`; AGENT-015's call site is `agent-loop.ts:552`.

## Open items

> **⚠ THIS TABLE IS NOT THE COMPLETE OPEN SET.** 4 further items from the 2026-08-03
> surface-driven sweep live in their own table under `## Surface-sweep findings` (line ~264), with
> `-S` ids — **including 0 rated critical/high**. Enumerating only this table undercounts the
> area by 4 items, which is exactly how `SEAM-S01` (high) escaped a full audit pass on
> 2026-08-07. Count BOTH tables. See structural defect A in `00-residual-ledger.md`.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| AGENT-007 | high | parity-bug | S | `prepare_next_turn` / `should_stop_after_turn` error ends the run with a bare `agent_end` |
| AGENT-006 | medium | parity-bug | S | Hook failures reported with hardcoded placeholders instead of the hook's error |
| AGENT-009 | medium | parity-bug | M | Error tool results diverge in `details` and in `tool_execution_end.result` shape |
| AGENT-012 | medium | parity-bug | S | Abort checked before `before_tool_call`, and ordered below block |
| AGENT-014 | medium | not-ported | M | No `pending` stop reason for in-flight assistant messages |
| AGENT-015 | medium | parity-bug | S | Aborted parallel batch: unprepared calls veto termination |
| AGENT-016 | medium | cyrup-original | S | Panicking tool in a parallel batch vanishes (unwind builds only) |
| AGENT-017 | medium | stale-port | S | Per-turn refresh re-pushes only `tools`; mid-run model / thinking-level change never reaches the loop |
| AGENT-003 | low | parity-bug | S | `tool_execution_update` dropped when the bounded channel fills |
| AGENT-010 | low | parity-bug | S | Loop-generated tool-error strings do not match pi's |
| AGENT-011 | low | parity-bug | S | `state.error_message` gated wrongly and invents a message |
| AGENT-013 | low | parity-bug | S | Proxy HTTP failures lack pi's `Proxy error: …` message |
| AGENT-018 | low | parity-bug | S | Reducer diverges on non-assistant `message_start` and on when `pendingToolCalls` clears |
| AGENT-019 | low | test-defect | S | `a_02_2_parallel_completion_vs_source_order` asserts wall-clock latency and a sleep-derived order |

## AGENT-007 — `prepare_next_turn` / `should_stop_after_turn` error ends the run with a bare `agent_end`

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-agent/src/agent.rs:566-569`: `Err(_) => { self.emit(AgentEvent::AgentEnd { messages: self.new_messages.clone() }).await; return; }` for `prepare_next_turn`; an identical arm at `agent.rs:596-599` for `should_stop_after_turn`. No error assistant message, no `turn_end`, no `errorMessage` on state. The catch-all path at `agent.rs:1640-1690` already synthesizes `message_start`/`message_end`/`turn_end`/`agent_end` from an `errored_assistant`, so the loop knows how to close properly — these two arms just don't use it.

**upstream** — `pi/packages/agent/src/agent-loop.ts:229-256` calls `prepareNextTurn`/`shouldStopAfterTurn` with no try/catch, so a throw escapes `runLoop` into `pi/packages/agent/src/agent.ts:487-492` → `handleRunFailure` (`agent.ts:496-511`), which emits all four events at `:507-510` with `stopReason: aborted ? "aborted" : "error"` (`:503`) and `errorMessage` (`:504`).

**Impact** — A failing session hook (or any extension in that path) truncates the run with no error surfaced: the transcript has no failure message, the UI's turn never closes, and `state.error_message` stays unset, so the failure is invisible to the user and to any embedder polling state.

**Fix** — Factor `agent.rs:1656-1688` into `RunCtx::emit_run_failure(msg: String)` and call it from both `Err` arms after pushing the failure assistant message onto `self.new_messages`.

**Verify** — Test in `crates/cyrup-agent/tests/agent_loop.rs` with a hook whose `prepare_next_turn` returns `Err`; assert the event tail is `message_start` / `message_end` / `turn_end{stop_reason:"error"}` / `agent_end` and that the snapshot's `error_message` carries the hook's text.

## AGENT-006 — Hook failures reported with hardcoded placeholders instead of the hook's error

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — Four sites discard `HookError` via `Err(_)`: `crates/cyrup-agent/src/agent.rs:921` (`"beforeToolCall failed"`), `agent.rs:1038` (`"afterToolCall failed"`), `agent.rs:634` (`"transformContext failed"`), `agent.rs:638` (`"convertToLlm failed"`). cyrup already does this right for schema-validation failures at `agent.rs:899` (`self.immediate_error(call, e.to_string())`), so the placeholders are an oversight, not a policy.

**upstream** — `pi/packages/agent/src/agent-loop.ts:656-663`: the `catch (error)` closing `prepareToolCall` (opened `:600`) returns `createErrorToolResult(error instanceof Error ? error.message : String(error))` and wraps the whole prepare path including the `beforeToolCall` call. Same for `afterToolCall` at `agent-loop.ts:744-747`. A throwing `transformContext`/`convertToLlm` propagates to `pi/packages/agent/src/agent.ts:496-511`, whose `errorMessage` is the real text (`:504`).

**Impact** — The model receives a tool result saying only "beforeToolCall failed" and the user sees the same, so a permission-gate rejection, a config error and a fault inside a hook are indistinguishable. Debugging an extension failure requires attaching a debugger.

**Fix** — Change the four `Err(_)` arms to `Err(e)` and interpolate `e.to_string()`; confirm `HookError`'s `Display` in `crates/cyrup-agent/src/error.rs` carries the underlying text rather than a fixed label.

**Verify** — Hook returning `Err(HookError::from("distinctive-42"))` at each of the four points; assert the string reaches the emitted tool result / error assistant message. No existing test asserts any of the four placeholders (grepped workspace-wide), so nothing has to be rewritten.

## AGENT-009 — Error tool results diverge in `details` and in `tool_execution_end.result` shape

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `crates/cyrup-agent/src/agent.rs:942`: `immediate_error` builds `details: None`; the execute-error arm at `agent.rs:990` also passes `None`. `result_value_of` (`agent.rs:116-137`) unconditionally inserts `"details"` as `Value::Null` (`:125`) and unconditionally inserts `"terminate"` (`:135`). The update payload (`update_value`, `agent.rs:143-151`) already omits `terminate` correctly, so the two payloads are internally inconsistent. Since 4091c86, `fail_truncated_tool_calls` (`agent.rs:849-857`) routes through `immediate_error`, so every truncated-batch result inherits the same divergence.

**upstream** — `pi/packages/agent/src/agent-loop.ts:756-761`: `createErrorToolResult` returns `{ content: [{type:"text", text: message}], details: {} }` — `{}` literal. `emitToolExecutionEnd` (`agent-loop.ts:763-771`) emits `result: finalized.result` verbatim, so `JSON.stringify` drops `details`/`terminate` when `undefined`.

**Impact** — Any consumer of the event stream or the JSONL that distinguishes "no details" from "empty details" — extensions, golden snapshots, an SDK embedder — sees a shape pi never emits, and `terminate: null` appears on every result where pi emits nothing.

**Fix** — In `immediate_error` (`agent.rs:937-962`) emit an empty details map rather than `None`; in `result_value_of` (`agent.rs:116-137`) insert `details`/`terminate` only when present. The `terminate` half requires `cyrup_core::ToolResult.terminate` (`crates/cyrup-core/src/tool.rs:42`) to become `Option<bool>`, as `ToolUpdate.terminate` already is (`tool.rs:56`).

**Verify** — Assert the serialized `tool_execution_end.result` for a not-found tool has `details == {}` and no `terminate` key. `gap26_tool_execution_end_result_includes_terminate` (`crates/cyrup-agent/tests/model_boundary.rs:503-523`) asserts `terminate == true` for a tool that genuinely sets it — pi emits that too, so it survives the fix.

## AGENT-012 — Abort checked before `before_tool_call`, and ordered below block

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-agent/src/agent.rs:901-903`: `if self.cancel.is_cancelled() { return Prep::Immediate(… "Operation aborted") }` sits between validation (`:897-900`) and hook invocation (`:918`). Second divergence at `agent.rs:920-933`: the arms are ordered `Err(_)` (`:921`), `Block` (`:922-925`), then `Proceed` (`:927-933`) with `is_cancelled()` nested inside the `Proceed` arm at `:928`.

**upstream** — `pi/packages/agent/src/agent-loop.ts:600-663`: `prepareToolCall` has no pre-hook abort check. The only checks are `if (signal?.aborted)` at `:629`, immediately after the hook returns and **before** the block check, and `:644` after the block branch. pi therefore always invokes `beforeToolCall`, and abort out-votes a block.

**Impact** — On abort, extensions that rely on `beforeToolCall` firing for every call (audit logs, permission bookkeeping, ref-counted resources) silently miss calls. And a call the hook blocked during an aborted run reports the block reason where pi reports `"Operation aborted"`, so the transcript attributes the stop to policy instead of to the user.

**Fix** — Delete `agent.rs:901-903`; hoist the `is_cancelled()` check out of the `Proceed` arm so it runs on any `Ok(_)` before the `Block` branch is considered.

**Verify** — Test with a counting `before_tool_call` hook plus a token cancelled before prep: assert the hook count equals the call count, and that a hook returning `Block{reason}` under a cancelled token yields `"Operation aborted"`.

## AGENT-014 — No `pending` stop reason for in-flight assistant messages

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `crates/cyrup-core/src/message.rs:94-100`: `pub enum StopReason { Stop, Length, ToolUse, Error, Aborted }` — no `Pending`. Both in-flight constructors seed `Stop`: `crates/cyrup-agent/src/proxy.rs:284` (`empty_partial`, used at `proxy.rs:101`) and `crates/cyrup-agent/src/agent.rs:98` (`empty_assistant`, used as the streaming partial at `agent.rs:697`).

**upstream** — `pi/packages/agent/src/proxy.ts:123` `stopReason: "pending"`, with the variant declared in `pi/packages/ai/src/types.ts` and threaded through every provider partial (pi `f9a49869`, 2026-07-27).

**Impact** — A partial assistant message on the wire is indistinguishable from a completed one that stopped normally. Anything keying off `stopReason` — a front-end deciding whether to show a spinner, a session reader deciding whether a message is resumable — is misled, and a crash mid-stream leaves a JSONL entry that reads as a clean stop.

**Fix** — Add `Pending` with `#[serde(rename = "pending")]` to `StopReason`; seed it at `proxy.rs:284` and `agent.rs:98`; audit per-provider partial constructors under `crates/cyrup-provider/src/api/*`; confirm no exhaustive match treats it as terminal, specifically `agent.rs:462` (`Error | Aborted`) and `agent.rs:479` (`Length`).

**Verify** — Assert the streamed partial carries `stopReason: "pending"` until the terminal event rewrites it, and that a mid-stream abort produces `aborted`, not `pending`.

## AGENT-015 — Aborted parallel batch: unprepared calls veto termination

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-agent/src/agent.rs:1094` pre-sizes `finalized: Vec<Option<Finalized>>` to `n`; the prep loop breaks on cancel at `:1141-1143`; the fold at `:1232` starts from `let mut all_terminate = !finalized.is_empty();` — true whenever `calls` is non-empty, because the vec was pre-sized — and `:1244` `None => all_terminate = false` lets every never-prepared slot veto termination. The sequential path already has the right shape via the `produced` counter (`agent.rs:1259,1323,1329-1331`), so the two batch modes disagree with each other as well as with pi. 8854601's message records leaving this alone.

**upstream** — `pi/packages/agent/src/agent-loop.ts:535-537` breaks the same loop, but `finalizedCalls` holds only entries actually pushed; `orderedFinalizedCalls` (`:540-542`) is that shortened list; `shouldTerminateToolBatch` (`agent-loop.ts:582-584`, called at `:552`) is `finalizedCalls.length > 0 && finalizedCalls.every(f => f.result.terminate === true)` over it.

**Impact** — Abort a run mid-parallel-batch where the prepared tools all set `terminate` and cyrup runs another turn instead of ending — a further provider request (cost, latency, and a turn the user did not ask for) where pi terminates. Switching the same workload to sequential execution changes the outcome.

**Fix** — Fold over present slots only: `let present: Vec<_> = finalized.into_iter().flatten().collect(); let all_terminate = !present.is_empty() && present.iter().all(|f| f.terminate);`, keeping the message-emission loop over `present`.

**Verify** — Two-call parallel batch, both tools `terminate: true`, token cancelled after the first prep; assert `agent_end` follows with no second `turn_start`, and assert the same sequence under `ToolExecution::Sequential`.

## AGENT-016 — Panicking tool in a parallel batch vanishes (unwind builds only)

**Kind** cyrup-original · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-agent/src/agent.rs:1158-1178`: the spawned body awaits `tool.execute(...)` with no `catch_unwind`. On unwind the task dies before `ftx.send`, both its `utx`/`ftx` clones drop, the drain loop (`agent.rs:1182-1228`) never decrements `remaining` and exits via `None => break` at `:1184`, and `while joinset.join_next().await.is_some() {}` at `:1229` discards the `JoinError`. Consequences: `finalized[idx]` stays `None`, so no `ToolExecutionEnd` is emitted (and `crates/cyrup-agent/src/state.rs:122-124` is the only remover of `pending_tool_calls`, so the id stays pending), no tool-result `MessageStart`/`MessageEnd` pair (`agent.rs:1233-1246` skips `None`), nothing pushed at `agent.rs:489-494`, and `all_terminate` forced false at `:1244`. The **sequential** path has no such hole — it awaits inline on the run task (`agent.rs:1285-1303`), so the unwind reaches the catch-all at `agent.rs:1640` and produces pi's full closing sequence. No containment in the ext wrappers either: `crates/cyrup-ext/src/wrapper.rs:123-145` and `crates/cyrup-ext/src/host/live.rs:1341,1364` contain zero `catch_unwind`.

**upstream** — `pi/packages/agent/src/agent-loop.ts:666-708`: `executePreparedToolCall` wraps the execute in try/catch/finally and converts any throw into `{ result: createErrorToolResult(…), isError: true }` (`:700-704`), which flows through `finalizeExecutedToolCall` → `emitToolExecutionEnd` (`:531`) → `createToolResultMessage` (`:773`) like a normal result. pi cannot lose a tool result to a faulting tool in either batch mode.

**Impact** — Scoped by profile. `cyrup/Cargo.toml:215` sets `[profile.release] panic = "abort"`, so in the shipped binary a panicking tool aborts the process and this control-flow hole is unreachable — as is every other `catch_unwind` in the workspace (`agent.rs:167`, `:387`, `:1640`, `cyrup-ext/src/native.rs:480`, `facade.rs:323` are all inert in release). Where it bites is `cargo test`, `cargo run`, and any embedder building with `panic = "unwind"`: the parallel path silently drops a result while the sequential path closes cleanly, so a suite exercising a faulting tool sees two incomparable behaviors, and a debug-run agent emits a transcript violating the tool_use/tool_result pairing invariant, which the next provider request rejects.

**Fix** — In the spawned body (`agent.rs:1158-1178`) wrap the await: `match AssertUnwindSafe(tool.execute(...)).catch_unwind().await { Ok(r) => r, Err(p) => Err(ToolError { message: panic_message(p.as_ref()) }) }`, reusing the existing `panic_message` helper (`agent.rs:175-183`); `FutureExt` is already in scope (`catch_unwind().await` at `agent.rs:167`) and `AssertUnwindSafe` is sound for the same reason as at `agent.rs:387`. Do the same in `execute_sequential` (`agent.rs:1285-1303`) so both modes agree with pi's single try/catch. Belt-and-braces: after the drain, synthesize an `immediate_error` for any still-`None` slot in a batch that was not cancelled. Whether release should also unwind is a separate profile-policy question, out of scope here.

**Verify** — New test in `crates/cyrup-agent/tests/agent_loop.rs`: two parallel calls, tool A panics `"boom-42"`, tool B returns normally. Assert (1) two `tool_execution_end`, A's with `is_error` and content containing `boom-42`; (2) two tool-result `message_end` in source order; (3) `turn_end.tool_results.len() == 2`; (4) `pending_tool_calls` empty before `agent_end`. Repeat under `ToolExecution::Sequential` and assert an identical sequence. All four fail today in the parallel case under the default (unwind) test profile.

## AGENT-017 — Per-turn refresh re-pushes only `tools`; mid-run model / thinking-level change never reaches the loop

**Kind** stale-port · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-agent/src/agent.rs:290-324`: `RunCtx` snapshots `model` (`:303`) and `thinking_level` (`:306`) at run start; `stream_assistant` reads only those (`agent.rs:624-625`); the only in-run writer is a `TurnUpdate` folded at `agent.rs:536-541`. The port of pi's per-turn refresh is `SessionHooks::prepare_next_turn` at `crates/cyrup-session-svc/src/hooks.rs:170-181`, which awaits the inner hook (`:174`) and then sets exactly one field — `update.tools = Some(session.next_turn_tools().await);` (`:179`) — never `update.model` or `update.thinking_level`. The mechanism exists and the loop honors it stickily (`TurnUpdate::model` / `::thinking_level`, `crates/cyrup-agent/src/hooks.rs:119-120`); only the caller omits it. Meanwhile `AgentSession::set_model_resolved` (`crates/cyrup-session-svc/src/session.rs:2254`) and `set_thinking_level` (`session.rs:2750-2775`) write straight through `Agent::set_model` / `set_thinking_level` (`agent.rs:1410,1413` — plain state writes, no idle gate), so the write lands where the in-flight run will never read it. Second artifact of the same omission: the doc at `hooks.rs:154-156` claims "Ordering matches Pi exactly … an extension may still replace the transcript, model or thinking level; it may not out-vote the session on which tools exist" — false about pi.

**upstream** — `pi/packages/coding-agent/src/core/agent-session.ts:520-541`: `_installAgentNextTurnRefresh` returns on every turn `{ ...previousSnapshot, context: {…, tools: this.agent.state.tools.slice()}, model: this.agent.state.model, thinkingLevel: this.agent.state.thinkingLevel }` — `model` at `:537` and `thinkingLevel` at `:538`, both **after** the spread, so the session deliberately out-votes any extension override. `pi/packages/agent/src/agent-loop.ts:229-241` folds them into the running config (`model: nextTurnSnapshot.model ?? config.model`; `reasoning` derived from `thinkingLevel`, `"off"` → `undefined`).

**Impact** — Switching model (`/model`, the picker) or cycling the thinking level while a run is in flight has no effect until the next prompt: for an agentic tool loop that is dozens of turns and many minutes still streaming against the old model, the old reasoning tier and the old price, where pi switches at the next turn boundary. Worse, `set_thinking_level` persists a `thinking_level_change` entry to the JSONL and emits `ThinkingLevelChanged` plus the `thinking_level_select` ext event (`session.rs:2762-2772`), so the transcript and the UI both claim a switch that did not happen.

**Fix** — In `crates/cyrup-session-svc/src/hooks.rs:178-180`, alongside `update.tools`, set `update.model` and `update.thinking_level` from the live agent snapshot (`Agent::snapshot()` exposes both — `crates/cyrup-agent/src/state.rs:87-88`), after the inner hook's result so session precedence matches `agent-session.ts:537-538`. Correct the doc at `hooks.rs:154-156` in the same change. Leave the deliberate, documented `systemPrompt` exception at `hooks.rs:158-160` alone.

**Verify** — Test in `crates/cyrup-session-svc/tests/` with a recording `StreamFn` capturing `ModelRef` and `StreamOptions.reasoning` per request; drive a two-turn run and call `set_model_resolved(other)` / `set_thinking_level(Xhigh)` from a subscriber on the first `tool_execution_end`; assert request #2 carries the new values. Today it carries the originals. `crates/cyrup-agent/tests/turn_tool_refresh.rs:132-175` is the working template for the tools half.

## AGENT-003 — `tool_execution_update` dropped when the bounded channel fills

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — Parallel path: `crates/cyrup-agent/src/agent.rs:1095` `mpsc::channel::<ToolRuntimeMsg>(64)` with the sink at `:1160-1167` doing `let _ = utx.try_send(…)`, discarding the `Result`. Sequential path: `agent.rs:1276` `mpsc::channel::<ToolUpdate>(64)` and `:1281` `let _ = utx.try_send(u);`. The `accepting` AtomicBool (`agent.rs:1152,1161,1169`; `:1277,1280,1304`) correctly mirrors pi's `acceptingUpdates`.

**upstream** — `pi/packages/agent/src/agent-loop.ts:671` `const updateEvents: Promise<void>[] = []`; `:681-691` pushes every emission; `:695`/`:699` `await Promise.all(updateEvents)` on both the success and throw paths, plus the `finally` at `:705`. The only upstream drop rule is `acceptingUpdates` (`:672,680,694,698,705`).

**Impact** — Progress output from a chatty tool can be silently truncated in the UI and in the transcript. Downgraded from medium: 8854601's two-phase rewrite means the receiver starts draining within microseconds of the first spawn (`agent.rs:1180-1182`), so only a synchronous burst of >64 updates outruns it, and the built-in bash tool throttles at 100ms leading+trailing (`crates/cyrup-tools/src/tools/bash.rs:188-212`). Caveat: third-party and extension tools have no such throttle, so `low` is a statement about the built-ins.

**Fix** — Switch both channels to `mpsc::unbounded_channel` and replace `try_send` with `send` in both paths, keeping the `accepting` gate as the sole drop rule.

**Verify** — Tool emitting 500 updates synchronously with no await between them; assert 500 `tool_execution_update` events reach a subscriber.

## AGENT-010 — Loop-generated tool-error strings do not match pi's

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-agent/src/agent.rs:886` `format!("Tool '{}' not found", call.name)` (single quotes); `agent.rs:924` `reason.unwrap_or_else(|| "Tool call blocked by beforeToolCall".to_string())`.

**upstream** — `pi/packages/agent/src/agent-loop.ts:611` `` createErrorToolResult(`Tool ${toolCall.name} not found`) `` (no quotes); `agent-loop.ts:639` `createErrorToolResult(beforeResult.reason || "Tool execution was blocked")`. The abort string already matches (`agent.rs:902,929` vs `agent-loop.ts:631,646`).

**Impact** — These strings go into the transcript sent back to the model, so cyrup and pi feed different text on identical inputs — a divergence in what the model conditions on, and a mismatch for any golden/differential fixture recorded from pi.

**Fix** — `agent.rs:886` → `format!("Tool {} not found", call.name)`; `agent.rs:924` → `"Tool execution was blocked"`.

**Verify** — Assert the exact strings in the emitted tool result. Confirmed at HEAD: `crates/cyrup-session-svc/tests/mid_run_tool_anchoring.rs:235` is a **comment** quoting the current wrong string; the assertion below it does not match on text, so only the comment needs updating.

## AGENT-011 — `state.error_message` gated wrongly and invents a message

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-agent/src/state.rs:125-131`: the `TurnEnd` arm gates on `matches!(a.stop_reason, StopReason::Error | StopReason::Aborted)` and falls back to `Some("turn ended with error".to_string())` when the message carries no `error_message`.

**upstream** — `pi/packages/agent/src/agent.ts:558-561`: `case "turn_end": if (event.message.role === "assistant" && event.message.errorMessage) { this._state.errorMessage = event.message.errorMessage; } break;` — gated purely on presence, no stop-reason gate, no synthetic fallback.

**Impact** — Two divergences: an aborted turn with no `errorMessage` gets a fabricated string in cyrup and nothing in pi (a user-visible "turn ended with error" on a deliberate cancel); and a `turn_end` carrying an `errorMessage` with a non-error stop reason updates pi's state but not cyrup's, so a recoverable-error annotation is lost.

**Fix** — Rewrite the arm to drop both the stop-reason gate and the fallback.

**Verify** — Reducer unit tests for both directions. No test asserts `"turn ended with error"` anywhere in the workspace (grepped).

## AGENT-013 — Proxy HTTP failures lack pi's `Proxy error: …` message

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-agent/src/proxy.rs:458-466` — the comment cites `proxy.ts:166-177` but the arm is `Err(e) => { let _ = tx.send(error_terminal(&builder, &cancel, e.to_string())).await; return; }`. `error_terminal` (`proxy.rs:509-524`) assigns `error.error_message = Some(message)` at `:522`. `open_sse` turns a non-2xx into `ProviderError::Http { status, message: <raw body> }`, so the terminal `errorMessage` is the raw body under cyrup's `Display` — never the literal `Proxy error: …`, and the server's JSON `error` field is never extracted.

**upstream** — `pi/packages/agent/src/proxy.ts:167` `` let errorMessage = `Proxy error: ${response.status} ${response.statusText}`; `` then `:171` `` errorMessage = `Proxy error: ${errorData.error}`; `` when the body parses as `{error?: string}`.

**Impact** — Proxy-mode failures surface an unstructured raw body instead of pi's two-tier message, so a proxy's own JSON error string is buried and the failure is not attributable to the proxy at a glance.

**Fix** — Match `ProviderError::Http { status, message }` specifically in `run_proxy` and reproduce pi's two-tier construction (status + statusText, upgraded to `errorData.error` when the body parses); fall back to `e.to_string()` for non-HTTP variants.

**Verify** — Stub proxy returning 502 with `{"error":"upstream down"}`; assert the terminal `errorMessage == "Proxy error: upstream down"`, and `Proxy error: 502 Bad Gateway` when the body is not JSON.

## AGENT-018 — Reducer diverges on non-assistant `message_start` and on when `pendingToolCalls` clears

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** medium

**cyrup** — `crates/cyrup-agent/src/state.rs:107-111`: `MessageStart` sets `streaming_message` only `if message.is_assistant()`, while the matching `MessageEnd` (`:115-118`) unconditionally clears it. `state.rs:132-135`: the `AgentEnd` arm does `st.streaming_message = None; st.pending_tool_calls.clear();`, and `RunCtx::emit` (`agent.rs:374-389`) reduces (`:375-378`) **before** awaiting subscribers (`:380-388`), so a subscriber handling `agent_end` already sees an empty pending set.

**upstream** — `pi/packages/agent/src/agent.ts:531-532`: `case "message_start": this._state.streamingMessage = event.message; break;` — no role check. `agent.ts:564-565`: the `agent_end` case clears only `streamingMessage`; `pendingToolCalls` is reset in `finishRun()` (`agent.ts:514-519`, the clear at `:517`), called from the `finally` at `agent.ts:491-492`. The doc at `agent.ts:521-527` states the guarantee outright: the run is idle only after all awaited `agent_end` listeners finish and `finishRun()` clears runtime-owned state.

**Impact** — Two observability divergences for anything reading `AgentStateSnapshot` from inside a subscriber: (a) a front-end or extension rendering `streaming_message` shows nothing for user and tool-result messages where pi shows them; (b) a subscriber reading `pending_tool_calls` on `agent_end` to detect calls abandoned by an aborted run sees an empty set under cyrup and the real set under pi — exactly the diagnostic that would surface AGENT-016. No effect on transcript or provider payloads.

**Fix** — Drop the `is_assistant()` guard at `state.rs:108`. Move `pending_tool_calls.clear()` out of the `AgentEnd` arm into `SettlementGuard::drop` (`agent.rs:1356-1368`), mirroring pi's `finishRun`.

**Verify** — Reducer unit test: `reduce(MessageStart{ AgentMessage::user_text("hi") })` → `streaming_message.is_some()`. Integration test in `crates/cyrup-agent/tests/agent_loop.rs` with a subscriber snapshotting `pending_tool_calls` inside its `agent_end` handler on a run aborted mid-batch, asserting the set is non-empty.

**Caveat** — The assistant-only rule at `state.rs:100-111` cites R-02-040 in its own doc comment. `spec/` is not in this workspace, so whether that requirement sanctions the divergence cannot be adjudicated here; filed at `low` with the uncertainty explicit. The `pending_tool_calls` half carries no such citation and is unambiguous.

## AGENT-019 — `a_02_2_parallel_completion_vs_source_order` asserts wall-clock latency and a sleep-derived order

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-agent/tests/agent_loop.rs:327`: `assert!(elapsed < Duration::from_millis(115), …)` where `elapsed` (`:277` → `:281`) spans `prompt` → `handle.finished()` → `wait_for_idle()` — faux-provider streaming and every subscriber await, not just the tool bodies — while the two `SpanTool`s sleep 80ms and 50ms (`:264-265`), leaving ~35ms for everything else. `agent_loop.rs:301-302`: `assert_eq!(ends, vec!["fast", "slow"], …)` plus `assert_ne!(ends, starts, …)` derive the expected completion order purely from the 80-vs-50ms gap, i.e. from the scheduler. The test collects real `(name, start, end)` intervals into `spans` (`:208-214`) and never reads them; the sibling `a_02_3_one_sequential_forces_batch_sequential` does exactly that at `:352-354` (`assert!(s[0].2 <= s[1].1)`).

**upstream** — The property under test is `pi/packages/agent/src/agent-loop.ts:540-542` (`await Promise.all(...)`), i.e. "the batch is concurrent" — a structural claim about overlapping intervals. pi's suite is not an oracle for a wall-clock bound.

**Impact** — Third instance of the shape this project keeps finding (`providers/anthropic.rs`, `round9_l5res.rs`, `caps/proc.rs`). Under load or a debug-profile runner `elapsed` crosses 115ms and the suite fails for reasons unrelated to the code; the reflex remedy is to raise the constant until the assertion proves nothing. The completion-order assertion fails the same way in the opposite direction — one 30ms hiccup on `fast` inverts it. These are the only assertions covering the concurrency half of AGENT-002's fix, so loosening them silently stops guarding what 8854601 landed.

**Fix** — Replace `agent_loop.rs:327` with an interval-overlap assertion over the `spans` already collected: sort by start and assert `s[0].2 > s[1].1` — the dual of `a_02_3`'s non-overlap check at `:354`, true by construction whenever the bodies are spawned together. For the completion-order half, release the two tools with a test-driven `Notify`/oneshot the way `agent_002_parallel_defers_execution_until_whole_batch_is_prepared` (`agent_loop.rs:1106-1152`) uses a `Barrier`, so `ends` vs `starts` becomes a fact rather than a race.

**Verify** — After the rewrite, run `cargo test -p cyrup-agent a_02_2` under artificial load (`taskset -c 0` alongside a busy loop) and confirm it still passes; today the wall-clock assertion fails under that condition while the code is correct.

## Coverage

Read-only; no `cargo`/`npm` was run. Tree verified clean at HEAD `1806375` before starting. Every `closed` verdict was treated as a claim to refute: for each, the cyrup file at HEAD, the corresponding pi v0.83.0 source, and the accompanying test were opened.

Read at HEAD: `crates/cyrup-agent/src/agent.rs` at 100-300, 299-398, 440-620, 615-735, 813-1082, 1076-1345, 1348-1400, 1630-1700; `state.rs` 85-138 (whole reducer); `event.rs` 40-80; `hooks.rs` 55-140; `proxy.rs` 275-295, 445-475, 500-530; `crates/cyrup-core/src/tool.rs` 1-80 and `message.rs` 25-105, 490-566; `crates/cyrup-session-svc/src/hooks.rs` 140-190 and `session.rs` 2238-2270, 2740-2775; `crates/cyrup-ext/src/wrapper.rs` 85-145; `crates/cyrup-provider/src/collection.rs` 395-436; `crates/cyrup-tools/src/tools/bash.rs` (update cadence); `crates/cyrup-agent/tests/agent_loop.rs` 160-360, 940-1035, 1090-1152; `tests/turn_tool_refresh.rs`; `crates/cyrup-session-svc/tests/mid_run_tool_anchoring.rs` 225-300; plus `cyrup/Cargo.toml` profiles.

Read upstream: `pi/packages/agent/src/agent-loop.ts` 190-260, 370-410, 520-560, 575-615, 625-712, 730-800; `agent.ts` 470-575; `proxy.ts` (targeted greps for `Proxy error` and `stopReason: "pending"`); `pi/packages/coding-agent/src/core/agent-session.ts` 505-545.

One material correction to the prior reassessment: `cyrup/Cargo.toml:215` sets `[profile.release] panic = "abort"`, which makes every `catch_unwind` in the workspace inert in the shipped binary. That does not kill AGENT-016 (the parallel/sequential asymmetry and the divergence from pi's uniform try/catch are real, and dev/test builds unwind) but it bounds the user-visible consequence, so the item is medium, not high. Other areas whose items lean on cyrup's panic-containment layer should note that layer is dev-only.

Test-defect hunt, both shapes. Shape 1 (a test pinning current-but-wrong behavior): grepped `crates/*/tests/` and `crates/*/src/` for every string this area's open items call wrong — `beforeToolCall failed`, `afterToolCall failed`, `Tool '…' not found`, `blocked by beforeToolCall`, `turn ended with error`, `transformContext failed`, `convertToLlm failed` — and no test asserts any of them; the only artifact is the comment at `mid_run_tool_anchoring.rs:235`. `gap26_tool_execution_end_result_includes_terminate` (`model_boundary.rs:503-523`) was re-checked and does not pin AGENT-009's defect. Shape 2 (uncontrollable scheduling outcome): grepped all eight files under `crates/cyrup-agent/tests/` for `sleep|elapsed|Instant::now|timeout(`. One clear instance, filed as AGENT-019. Examined and deliberately not filed: `a_02_7` (`agent_loop.rs:656`) and `miss4_abort_carries_streamed_partial_content` (`untracked_misses.rs:385`), which sleep to drain an instant stream and then assert on message content, not ordering; `proxy_live_turn.rs:122`, a server-side hold-open asserting nothing; and `agent_loop.rs:1073`'s 5s rendezvous timeout, whose margin is enormous. Noted but not filed: the AGENT-002 regression test's discriminating power rests on a 250ms gate sleep, so extreme load degrades it to a false PASS — the harmless failure direction.

Blind spots and things taken on trust: (a) every verdict rests on reading code and the shape of its test, not on executing either; (b) the spec tree is absent, so where cyrup cites a requirement to justify a divergence it cannot be adjudicated — this bears on exactly one item, AGENT-018's first half (R-02-040), kept at low with the uncertainty stated inline; (c) `pi/packages/agent/test/*.test.ts` was not read, which would independently pin the expected event sequences for AGENT-007 and AGENT-015; (d) AGENT-016's provider-side symptom is inference from the tool_use/tool_result pairing invariant, though the cyrup-side consequences follow directly from the code.

Scope note for the parent: AGENT-017's fix site is `crates/cyrup-session-svc/src/hooks.rs` (area 08); it is filed here because the agent-side half is verified present and honored and the whole gap is one caller-side omission plus a wrong doc comment — deduplicate against area 08. f777e44's deliberate WIT ABI break (on-tool-result gained usage-json in both copies; components built against the old world fail to instantiate) belongs to area 06, and its deferred pi `getUsageCostBreakdown` "Tools/summaries" bucket to areas 03/07; neither is filed here.



---

## Surface-sweep findings (2026-08-03, HEAD `9219dcd`)

Found by a **surface-driven** sweep that walked pi asking what has NO cyrup counterpart at
all, rather than checking a list of known items. That inversion exists because the
item-driven method missed pi's stray-OSC-reply swallow (`pi/packages/tui/src/tui.ts:788-794`)
— a real, user-reported bug — and by construction cannot see behaviour nobody wrote an item
for. IDs use an `-SNN` suffix to mark their provenance.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| AGENT-S01 | medium | not-ported | S | Provider attribution + opencode session-affinity headers are computed once at session build and never recomputed after a cross-provider `/model` switch |
| AGENT-S02 | low | not-ported | M | `Agent::subscribe` returns nothing (no detach handle) and `EventSubscriber::on_event` receives no abort signal |
| AGENT-S03 | low | not-ported | S | `StreamOptions.metadata` is unreachable from the agent loop — `GenerationConfig` has no field for it, so neither `Agent` nor the low-level `loop_fn` API can set it |
| AGENT-S04 | low | not-ported | S | The `transport` setting is parsed, migrated, offered in the settings dialog and never wired into the agent — a dead settings row |

## AGENT-S01 — Provider attribution + opencode session-affinity headers are computed once at session build and never recomputed after a cross-provider `/model` switch

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed

**upstream** — pi/packages/coding-agent/src/core/sdk.ts:318-327 calls `mergeProviderAttributionHeaders(model, settingsManager, options?.sessionId, requestHeaders)` INSIDE the `streamFn` closure — i.e. once per provider request, dispatched on the model that request is actually going to. `getSessionHeaders` / `getDefaultAttributionHeaders` (pi/packages/coding-agent/src/core/provider-attribution.ts:40-108) are keyed on the model's provider id and base-url host.

**cyrup** — ABSENT. 

**Impact** — Start on an opencode model, `/model` to Anthropic: `x-opencode-session: <session-uuid>` and `x-opencode-client: pi` keep going to api.anthropic.com for the rest of the session — an opaque session identifier sent to an unrelated vendor. Switch the other way and the affinity headers never appear, so opencode's session-keyed prompt-cache routing degrades to cache misses (real cost/latency, no error). Same for OpenRouter/NVIDIA/Cloudflare/Vercel attribution: switching to one of those mid-session drops `HTTP-Referer`/`X-OpenRouter-Title`/`X-OpenRouter-Categories`; switching away keeps sending them to whoever is next. Silent in both directions.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## AGENT-S02 — `Agent::subscribe` returns nothing (no detach handle) and `EventSubscriber::on_event` receives no abort signal

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed

**upstream** — pi/packages/agent/src/agent.ts:243-246 `subscribe(listener: (event, signal: AbortSignal) => …): () => void { this.listeners.add(listener); return () => this.listeners.delete(listener); }`; the contract is documented at :233-242 and the signal is passed at agent.ts:573 `await listener(event, signal)`. Both halves are load-bearing upstream: pi/packages/coding-agent/src/core/agent-session.ts:311/393/818-820/829-830 store and invoke `_unsubscribeAgent` to detach the session handler for the duration of a manual `compact()`; pi/packages/coding-agent/src/modes/rpc/rpc-mode.ts:56-57,352-360,731-732 unsubscribes and re-subscribes a stdout-backpressure listener on every rebind and on shutdown.

**cyrup** — ABSENT. 

**Impact** — Two capabilities absent rather than degraded. (a) Nothing can detach from an agent's event stream; an embedder attaching a per-operation observer leaks it for the process lifetime. cyrup's own `compact()` (crates/cyrup-session-svc/src/session.rs:1305-1315) substitutes `abort_and_settle()` for pi's `_disconnectFromAgent()` and documents it, but the primitive it substituted for still does not exist for anyone else. (b) A subscriber doing expensive work (streaming to a remote client, rendering, persisting) cannot observe that the run it is servicing was aborted, so it runs to completion and the abort's latency benefit is lost for exactly the listeners that make abort worth having. No current in-tree consumer is harmed — both registered subscribers are permanent — so this is an embedder/SDK-surface gap.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## AGENT-S03 — `StreamOptions.metadata` is unreachable from the agent loop — `GenerationConfig` has no field for it, so neither `Agent` nor the low-level `loop_fn` API can set it

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — pi/packages/ai/src/types.ts:183-191 declares `metadata?: Record<string, unknown>` on `StreamOptions` ("For example, Anthropic uses `user_id` for abuse tracking and rate limiting"). `AgentLoopConfig extends SimpleStreamOptions` (pi/packages/agent/src/types.ts:144), so any low-level `agentLoop`/`runAgentLoop` caller can set it and it is spread into the provider call at agent-loop.ts:308-312.

**cyrup** — ABSENT. 

**Impact** — An SDK embedder cannot set Anthropic `metadata.user_id` (abuse tracking / rate-limit bucketing) or any other provider metadata through either `Agent` or the low-level `loop_fn` API, where pi's low-level caller can set it by construction. No effect on the shipped binary — no pi built-in populates it either — so this is an API-surface hole, not a runtime bug.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## AGENT-S04 — The `transport` setting is parsed, migrated, offered in the settings dialog and never wired into the agent — a dead settings row

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — pi/packages/coding-agent/src/core/sdk.ts:357 `transport: settingsManager.getTransport()` into `new Agent({...})`; folded into the per-turn loop config at pi/packages/agent/src/agent.ts:442 and reaching the provider at agent-loop.ts:308. Also mutated live at pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:4286-4289 (`onTransportChange` sets both the setting and `this.session.agent.transport`).

**cyrup** — ABSENT. 

**Impact** — SEVERITY CORRECTED DOWN from the claim's `medium`, and its stated impact is wrong for cyrup. I checked the consumer side: no cyrup wire API reads `StreamOptions.transport` at all (`grep -n 'transport' crates/cyrup-provider/src/api/*.rs crates/cyrup-provider/src/stream/sse.rs` finds only prose and error strings; the only `Transport::` uses in cyrup-provider are in a `simple_options.rs` unit test). Upstream, `transport` is consumed only by `packages/ai/src/api/openai-codex-responses.ts:300,1480` and `bedrock-converse-stream.ts` — BOTH documented-unported wire APIs. So `Auto` in cyrup never prefers websocket; every request is SSE regardless. The real, live consequence is narrower: the TUI settings dialog (crates/cyrup-tui/src/app.rs:3666-3671) presents a `Transport: auto/websocket/sse` choice that can never affect anything, and the `websockets` migration writes a key nothing consumes. It becomes a genuine behavioural gap only if `openai-codex-responses`/`bedrock-converse-stream` are ported.

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

