---
stage: split
status: done
updated: 2026-09-03 07:50
---

# Core Loop Type-Driven Design Review

## Description

Map out a refactor of the agent "core artery" — the inference loop in `cyrup-agent` above the
provider abstraction — by performing a Rust type-driven design opportunity review, then extend
each accepted finding outward along its **tendrils**: every downstream crate that consumes the
`cyrup-agent` surface the finding reshapes. The output of THIS task is a **review + cross-crate
refactor map with minimal design sketches**, not code changes; the map is what `/split` decomposes
into per-crate implementation tasks. Deliverable: `.flux/research/CORE_LOOP_TYPE_REVIEW.md`.

**Scope decision (2026-09-03):** breaking changes to the public `Agent` / `loop_fn` / `Hooks`
surface ARE on the table. The goal is a refactor in which the whole call graph — `cyrup-agent`
internals AND every consumer — is improved by the same pattern. A finding that stops at
`cyrup-agent`'s edge and lets `cyrup-session-svc` immediately convert back to the primitive is
incomplete; each type is followed outward until it reaches a genuine system boundary, and the
review says where and why it ends there.

**Augmentation (2026-09-03):** the research below was done by reading every non-test file in
`cyrup-agent/src/` and sweeping all 67 downstream files that import `cyrup_agent`. The findings,
boundaries, tendril tables, migration order and rejects are PRE-COMPUTED. `/exec` writes the
review document from them — re-verifying each cited line still holds at HEAD, filling in the
twelve sub-fields per finding, and producing the before/after sketch — rather than re-doing the
survey. Where this file says "open question", `/exec` resolves it by reading the named code and
records the answer in the document.

---

## Part A — The review prompt (the spec the document must satisfy)

You are a senior Rust engineer performing a focused design review. Review the supplied Rust code
for concrete opportunities to encode domain invariants and legal workflow states in the type
system using: (1) Newtypes + "Parse, don't validate"; (2) Typestate; (3) Explicit domain enums
and exhaustive matching; (4) Functional Core, Imperative Shell. This is an opportunity review,
not a mandate. Prefer the smallest design change that captures a meaningful invariant. Do not
modify code; produce a review and minimal design sketches.

### Core decision rules
- Invalid value → newtype. Invalid sequence → typestate. Dynamically-inspected state → enum.
- Domain logic tangled with I/O or shared mutation → Functional Core / Imperative Shell.
- Expected business outcome → named enum variant. Aborting technical failure → `Result<T, E>`.
- Do not apply typestate when a newtype, enum, private constructor, or pure function suffices.

### Required output (sections, in order)
1. **Executive Verdict** (5–10 sentences).
2. **Invariant and State Map** — `| Location | Domain fact or state | Current encoding | Failure mode | Best representation |`.
3. **Findings** — each `#### [P1/P2/P3] Title` with the twelve sub-fields: Location · Current
   representation · Invariant or legal sequence · Concrete failure mode · Recommended pattern ·
   Why this pattern fits · Minimal proposed API · Guarantee gained · Guarantee not gained ·
   Migration and compatibility cost · Benefit versus ceremony · Confidence.
4. **Highest-Value Refactor Sketch** — before/after for the strongest finding.
5. **Deliberately Rejected Opportunities** (required, non-empty).
6. **Incremental Migration Plan** — workspace compiles after every step.
7. **Test Implications** — which runtime tests become parser tests / redundant / must remain;
   whether compile-fail tests earn their keep; which shell-level tests remain.

### Final review standards
Tie every finding to concrete code · do not infer domain rules silently · more types ≠ better ·
no typestate for complexity metrics alone · domain names over generic wrappers · newtypes at
boundaries · never unwrap a newtype right after constructing it · private constructors/fields
when they establish an invariant · account for serde · be explicit about migration cost ·
distinguish compile-time from runtime/distributed guarantees · few high-value findings over many
minor wrappers. Cap main findings at five unless extra ones are serious correctness risks
(this review has eight candidates; the document keeps the five strongest as main findings and
carries the rest under a "Secondary" heading with the same sub-field structure, so `/split` can
still cut them).

---

## Part B — The critical path (files under review)

All in `crates/cyrup-agent/src/`. Line numbers are as of `2cfff0f`; `/exec` re-verifies.

| Layer | File | Key symbols |
|---|---|---|
| 1. Run entry / latch | `agent/lifecycle.rs` | `Agent::prompt` (:191), `prompt_with_images` (:211), `continue_run` (:222-262), `start_run` (:264-330), `SettlementGuard` (:53-100), `RunHandle`, `emit_standalone` |
| 1b. Low-level twin | `loop_fn.rs` | `AgentContext`, `AgentLoopConfig`, `build_run_ctx` (:96-158), `run_agent_loop`, `run_agent_loop_continue` (:166-186), `agent_loop`, `agent_loop_continue` (:238-268) |
| 2. Run context | `agent/run/mod.rs` | `RunCtx` (:43-87), `RunCtx::new` 17 args (:91-138), `with_header_fn`, `emit` (:189-207), `emit_run_failure` (:227-255), `RunFailure(String)` (:31), `EntryStart` (:18-21), `run` (:277-289), `run_entry` (:291-307) |
| 3. Turn driver | `agent/run/turn.rs` | `RunCtx::run_loop` (:11-207) |
| 4. LLM boundary | `agent/run/stream.rs` | `RunCtx::stream_assistant` (:32-251); provider seam at :153 `self.stream_fn.stream(&model, &ctx, &opts)` |
| 4b. Transport seam | `stream_fn.rs` | `StreamFn`, `ApiKeyResolver`, `ProviderStreamFn` |
| 5. Tool batch | `agent/run/tools/mod.rs` | `execute_tool_calls` (:52-69), `fail_truncated_tool_calls` (:84-117), `ToolRuntimeMsg`, `Finalized` (:26-38), `Prep` (:40-45), `Batch` |
| 5a. Preflight | `agent/run/tools/preflight.rs` | `RunCtx::prepare` (:19-121), `immediate_error` (:128-164) |
| 5b. Execution | `agent/run/tools/exec.rs` | `execute_parallel` (:35-263), `execute_sequential` (:266-376); local `struct Deferred` (:52-58) |
| 5c. Finalize | `agent/run/tools/finalize.rs` | `RunCtx::finalize` (:16-154) |
| Supporting | `state.rs` | `StateInner` (:78-108), `GenerationConfig`, `AgentStateSnapshot`, `reduce` |
| Supporting | `hooks.rs` | `Hooks` (:210-283), `TurnUpdate` (:124-146), `BeforeOutcome` (:47-64), `AfterOverride`, `PostTurn`, `AgentContextView`, `default_convert_to_llm` |
| Supporting | `agent/mod.rs`, `agent/builder.rs`, `agent/facade.rs`, `agent/prompt.rs`, `agent/message.rs` | `Agent` struct (:58-82), `AgentBuilder`, setters/queues/latch, `PromptInput`, `errored_assistant`/`empty_assistant`/`result_value_of` |
| Supporting | `event.rs`, `queue.rs`, `error.rs` | `AgentEvent`, `AgentMessage` + hand-written serde (:86-190), `APP_MESSAGE_ROLES` (:85), `ToolResultMessage`, `PendingQueue`, `AgentError`/`BusyEntry`/`ContinueSurface` |

Below the seam (boundary types only): `cyrup-provider/src/{provider.rs:154 Provider::stream, stream.rs:155 StreamOptions, :502 StreamEvent, context.rs:8 Context}`, `cyrup-core/src/tool.rs` (`Tool`, `ToolResult` :41 `terminate: bool`, `ToolUpdate` :52 `terminate: Option<bool>`), `cyrup-core/src/message/stop_reason.rs:75` (`StopReason::{Pending,Stop,Length,ToolUse,Error,Aborted,Deferred}`).

---

## Part C — Boundaries where a newtype legitimately ends (CONFIRMED by the sweep)

These are immovable for this pass. Every finding below either leaves them byte-identical or is
rejected.

| Boundary | Evidence | What must not change |
|---|---|---|
| **B1. `AgentMessage` serde shape** — self-tagged `role` ∈ `user`/`assistant`/`toolResult`/`custom`/`<App role>`, `Assistant` flattened (message's own fields + `role`), `Custom{kind,payload,details?,timestamp}` | Read by string projection with NO type dep in `cyrup-tui/src/app/event_extract.rs:182-259` and `extension_render.rs:63-76,136-144`; duck-typed in `cyrup-ext-subagents/src/exec/ndjson.rs:289-322` (`assistant_usage`, `is_error_or_aborted_message`) and `watchdog/turn_delta.rs:365-419,449`; `--json`/RPC stdout. A change compiles clean and breaks rendering / subagent usage accounting / watchdog review **silently**. `event.rs:86-102` documents why the serializer is hand-written (duplicate `role` key) — never revert to a derive. | Tag key, tag values, field names, `Assistant` flattening, `Custom` key names |
| **B2. `AgentEvent` serde** — `tag="type", rename_all="snake_case", rename_all_fields="camelCase"`, all 10 variants' field names/optionality | `ndjson.rs::SubagentEvent` is a dependency-free byte-compatible retype; a missing required key fails the WHOLE line silently (`ndjson.rs:25-30`). This exact bug already shipped once (`spawn/mod.rs:337-354`). `cyrup-session-svc/src/event.rs:286-333 from_agent` maps all 10 exhaustively; `cyrup-ext/src/event.rs:190-204,487-539` matches all 10 exhaustively (adding a variant is a compile error there — good). | Attributes, variant names, payload field names |
| **B3. Provider seam** — `StreamFn::stream(&ModelRef, &Context, &StreamOptions) -> EventStream<StreamEvent>` (sync, `&self`) | Implemented by `ProviderStreamFn`, `cyrup-session-svc/src/provider_swap.rs:83-95 ProviderSwap`, `cyrup-it/tests/bin/embedder_seams.rs:76-86 RecordingStreamFn`; re-exported by name from `cyrup-sdk/src/lib.rs:72` and `cyrup-session-svc/src/lib.rs:136`. `ApiKeyResolver::get_api_key(&ProviderId) -> Option<String>` (async_trait) implemented in `embedder_seams.rs:207-213`. | Signatures |
| **B4. Persisted transcript** — `cyrup_session::agent_message::AgentMessage` (its OWN enum, hand-written `SerializeMap` in Pi field order, `agent_message.rs:104-112, 267-364`); `cyrup-session` has NO `cyrup-agent` dependency | The bridge is entirely `cyrup-session-svc/src/event.rs:375-498` (`agent_message_to_core`, `raw_message_to_agent`, `core_message_to_agent`). Session's `Custom{custom_type,content}` ↔ agent's `Custom{kind,payload}` rename at `:423-431` is the only reconciliation and has no two-sided test. | The JSONL bytes; the bridge functions are the legitimate conversion site |
| **B5. WASM host event** — `cyrup-ext/src/event.rs::HostEvent`: `MessageStart`/`MessageUpdate` carry `AgentMessage` as `serde_json::Value` (:494-502), `TurnEnd{message: AgentMessage (by value), tool_results: Vec<ToolResultMessage>}` (:320,532-536), `Context{messages: Vec<Arc<AgentMessage>>}` (:301) | `transform_context`'s `Vec<Arc<AgentMessage>>` is load-bearing: `prompt_runtime.rs:1053-1056` uses `Arc::ptr_eq` as the "was this rewritten?" test and `hooks.rs:135-138` (PERF-002) forbids unwrapping the handles. | `Arc` on the context path; serde shape (B1) for the `Value` paths |
| **B6. Cross-crate duplicated contracts** | `cyrup-ext/src/contract.rs:190-199 Reduced::Blocked{reason, terminate: bool, by}` → copied field-for-field into `BeforeOutcome::Block` at `cyrup-ext/src/hooks.rs:56-58`; `contract.rs:52-57 EventPatch::ToolResult` mirrors `AfterOverride` minus `terminate`; `contract.rs:96-114 apply_patch` reimplements replace-not-merge. `HookOutcome::Block::terminate` doc (`contract.rs:20-31`) restates `BeforeOutcome::Block::terminate` verbatim. | Change both sides together (F3, F7) |

Per-run cancellation contracts that any refactor of `RunCtx` must preserve (they are relied on
across the crate boundary): `before_tool_call`'s `cancel` is a FRESH child of the run root minted
at `preflight.rs:83` — `cyrup-ext/src/hooks.rs:44-52` (EXT-029) returns `Proceed` on a cancelled
token and relies on the agent re-checking the root at `preflight.rs:94-97,111-118` before
executing. `EventSubscriber::on_event` is invoked sequentially and awaited — `cyrup-ext/src/subscriber.rs:19-21`'s
`Relaxed` turn counter depends on it.

---

## Part D — Tendrils (who consumes what; the refactor blast radius)

Non-test files only. "Free" = zero consumers outside `cyrup-agent`; reshape without accounting.

| Surface item | Consumers | Nature |
|---|---|---|
| `loop_fn::*` (`agent_loop`, `agent_loop_continue`, `run_agent_loop`, `run_agent_loop_continue`, `AgentContext`, `AgentLoopConfig`, `AgentEventSink`, `AgentLoopStream`) | **NONE** (repo-wide grep; only reachable via `cyrup_sdk::agent` module alias, unused) | **Free** |
| `Agent` methods | `cyrup-session-svc` only: `builder.rs:1603-1619,1737,1783-1790`; `session/run.rs:139,150-151,164-166,239,579,601-610,626,640,659`; `session/queue.rs:39,64,91,126-135`; `session/inject.rs:77-78,129`; `session/model.rs:319,333-335,485,503`; `session/mod.rs:466-471,480`; `session/tools.rs:37,50,130,132,143-144`; `session/thinking.rs:18,59-60,120`; `session/accessors.rs:46,177,311,315`; `session/bash.rs:280-282`; `session/forking.rs:328-329`; `session/retry.rs:142-145`; `session/auto_compaction.rs:341,399-410`; `session/compaction.rs:256`; `session/control.rs:148` | 22 distinct methods; `set_messages` at **5 re-seed sites + 1 read-modify-write** |
| `AgentBuilder` | `builder.rs:1603-1737` only (13 setters) | Single site |
| `Hooks` impls | `cyrup-session-svc/src/hooks.rs:177-332 PolicyHooks` (all 6; `convert_to_llm` deliberately does NOT delegate, :180-186); `cyrup-ext/src/hooks.rs:30-148 ExtHooks` (3 of 6: `before_tool_call`, `after_tool_call`, `transform_context`) | Two impls |
| `TurnUpdate` / `PostTurn` / `AgentContextView` | `session-svc/src/hooks.rs:299-322` sets `tools`, `system_prompt`, `model`, `thinking_level` every turn (4 of 5; `context` left to extensions); `session/tools.rs:98-144` is the source. `cyrup-ext` never names them. `AgentContextView` has **zero readers** anywhere. | One producer |
| `BeforeOutcome` | `session-svc/src/hooks.rs:231-257` constructs `Block{reason: Some, terminate: false}`; `cyrup-ext/src/hooks.rs:56-58` pass-through from `Reduced::Blocked` | Two constructors |
| `AfterOverride` / `AfterToolCall` | `cyrup-ext/src/hooks.rs:100-122` diff protocol (`details`/`usage` cannot be cleared; `terminate` never read/set); session-svc passthrough | One real consumer |
| `EventSubscriber` impls | `session-svc/src/subscriber.rs:126-232 SvcSubscriber` (matches `MessageStart`+User, `MessageEnd`, `AgentEnd`, else `from_agent`); `cyrup-ext/src/subscriber.rs:40-75 ExtSubscriber` (`AgentStart`, `TurnEnd`, else) | Two impls, registered `builder.rs:1783-1790` (`Subscription` discarded) |
| `AgentEvent` | `session-svc/src/event.rs:286-333` (10/10 mapped, `AgentEnd` gets `will_retry:false` placeholder); `cyrup-ext/src/event.rs` (10/10, `MessageEnd` deliberately dropped, EXT-002) | Exhaustive both sides |
| `AgentMessage` | 12 session-svc files (~60 sites; all 5 variants matched in `hooks.rs:35-98` and `event.rs`); `Custom` literals: `session/inject.rs:64-71,121-126`, `session/control.rs:237-245`, `session/bash.rs:221-227`, `event.rs:423-430`; `App` literal `event.rs:435-438`; `cyrup-ext-subagents/src/prompt_runtime.rs:1002-1075` matches `Custom`/`ToolResult`/`Assistant`, constructs `Assistant` (:1075); `cyrup-ext-subagents/src/watchdog/turn_delta.rs:442-443` params; `cyrup-test-support/src/messages.rs:46-68`; `cyrup-it/tests/bin/wasm_renderer_screen.rs:136-141` `Custom` literal; **public API leak** `session-svc/src/session/accessors.rs:315` and `cyrup-sdk/src/handle.rs:425` return `Vec<cyrup_agent::AgentMessage>` | Deepest coupling; wire = B1 |
| `ToolResultMessage` | `session-svc/src/event.rs:472-495` (8-field literal), `hooks.rs:45-55`; `cyrup-ext/src/event.rs:320`; `turn_delta.rs:443` | All 8 fields |
| `APP_MESSAGE_ROLES` | `session-svc/src/hooks.rs:100-123 app_role_payload` — load-bearing const, not docs | One reader |
| `QueueMode` | Re-exported `session-svc/src/lib.rs:132`; `command.rs:46-47` public enum; `builder.rs:1864-1868 parse_queue_mode` (`"all"` → All, **anything else** → OneAtATime silently); mirrored in `session/mod.rs:218-220` | Public |
| `AgentError` | `session-svc/src/error.rs:16-17` blanket `#[from]`; produced at ONE site `run.rs:139`; variants **never inspected**; `run.rs:164` `Err(_) => break` swallows `ContinueFromAssistant`; `RunActive` pre-empted by `StreamingNeedsBehavior` (`run.rs:75`) | Opaque to consumers |
| `StreamFn` / `ProxyStreamFn` / `ApiKeyResolver` | B3 | Boundary |
| `RunHandle` | `run.rs:151,166` `.finished()` only, value discarded | Trivial |
| `PromptInput` | implicit `From<Vec<AgentMessage>>` at `run.rs:139,150` | Trivial |
| `ProviderStreamFn` | `provider_swap.rs:15,93`; `embedder_seams.rs:71-104` (NOT re-exported by sdk) | B3 |

Incidental cleanups the sweep surfaced (fold into whichever task touches the file; not findings):
`cyrup-ext/src/facade.rs:578-585` says `ExtensionHost::subscriber(&self, _cancel)` should drop
its ignored param together with the one caller `session-svc/src/builder.rs:1373`; `builder.rs:1864-1868
parse_queue_mode` should become `impl FromStr for QueueMode` in `cyrup-agent/src/queue.rs` with an
`Err` for unknown text; `cyrup-sdk/src/lib.rs` re-exports no `AgentMessage` by name while
`handle.rs:425` returns it.

---

## Part E — Findings (pre-computed; `/exec` writes them up with all twelve sub-fields)

Ranking rationale: impact × likelihood × how much the compiler can close × migration cost.
F1–F5 are the document's five main findings; F6–F8 go under "Secondary".

### F1 [P1] Continuation as a parsed value: `RunEntry` + `Agent::edit_transcript`

**Pattern:** Newtype/parse-don't-validate + domain enum (replaces a `bool`). Not typestate.

**Location.** The precondition "transcript non-empty ∧ last message not assistant" is checked
three times: `loop_fn.rs:174-183` (`run_agent_loop_continue`), `:246-255` (`agent_loop_continue`),
`lifecycle.rs:226-235` (`continue_run`, which additionally drains steering → follow-up before
erroring). `EntryStart::{Prompt(Vec), Continue}` (`run/mod.rs:18-21`) carries no proof; the
coupled `skip_initial_steering_poll: bool` is a separate 17th argument to `RunCtx::new`
(`:107`) that must be `true` iff the entry is a steering-drain continuation (`lifecycle.rs:243`
passes `true`, `:255` passes `false`, `:261` passes `false`, `loop_fn.rs:154` passes `false`).

**Concrete failure modes (real, in HEAD).**
1. TOCTOU: `continue_run` snapshots `messages` at `lifecycle.rs:226`, validates, then `start_run`
   re-snapshots at `:270-281` under a fresh lock. `Agent::set_messages` (`facade.rs`) between the
   two lets a run enter `Continue` on a transcript ending in an assistant message.
2. The session works around the precondition by hand in three places, all as
   `snapshot().messages` → `pop()` → `set_messages()` across two awaits with no lock:
   `session-svc/src/session/retry.rs:140-147 drop_trailing_assistant` (any trailing assistant),
   `auto_compaction.rs:376-412` (narrow: `Error|Length` only, with a "Do not reuse that helper
   here" warning because the broad one would swallow a completed `Stop`/`ToolUse` turn), and
   `bash.rs:280-282` (push). `run.rs:164` then swallows `ContinueFromAssistant` with
   `Err(_) => break`, so the failure the type should prevent is currently invisible.

**Proposed API (agent side, all `pub(crate)` except the two `Agent` methods).**
```rust
// agent/run/mod.rs
pub(crate) enum PromptSource { Fresh, SteeringDrain, FollowUpDrain }
pub(crate) enum RunEntry {
    Prompt { messages: Vec<AgentMessage>, source: PromptSource },
    Continue(ResumePoint),
}
impl RunEntry {
    fn skip_initial_steering_poll(&self) -> bool {
        matches!(self, RunEntry::Prompt { source: PromptSource::SteeringDrain, .. })
    }
}
/// Proof that a transcript may be resumed without a new message. Constructed ONLY by
/// `ResumePoint::check`, which is the single home of the precondition.
pub(crate) struct ResumePoint(());
impl ResumePoint {
    pub(crate) fn check(messages: &[AgentMessage], surface: ContinueSurface)
        -> Result<Self, AgentError>
    { /* the two ifs, once */ }
}
```
`RunCtx::new` loses the `bool`; `run_entry` derives it from `RunEntry`. `continue_run` takes the
lock ONCE: snapshot, `ResumePoint::check`, and hand the same `Vec` into `start_run(entry, snapshot)`
so the validated transcript is the one the run uses (closes failure mode 1). The two `loop_fn`
duplicates collapse to `ResumePoint::check(&context.messages, ContinueSurface::Loop)?`.

**Public API (the tendril).**
```rust
impl Agent {
    /// Atomic transcript edit under the state lock — the replacement for every
    /// snapshot → mutate → set_messages triplet. Refused while a run is in flight
    /// (same latch as `reset`), so it can never race the reducer.
    pub fn edit_transcript<R>(&self, f: impl FnOnce(&mut Vec<AgentMessage>) -> R)
        -> Result<R, AgentError>;
    /// The named operation both session predicates need: pop the trailing assistant
    /// iff `pred` holds, returning it. Built on `edit_transcript`.
    pub fn pop_trailing_assistant_if(&self, pred: impl FnOnce(&AssistantMessage) -> bool)
        -> Result<Option<Arc<AssistantMessage>>, AgentError>;
}
```
Consumers: `retry.rs:140-147` → `pop_trailing_assistant_if(|_| true)`; `auto_compaction.rs:376-412`
→ `pop_trailing_assistant_if(|a| matches!(a.stop_reason, Error | Length))`; `bash.rs:280-282` →
`edit_transcript(|m| m.push(msg))`; `run.rs:164` stops swallowing — with the session no longer
able to hand the agent an unresumable transcript, an `Err` there is a bug and should surface as
`SessionServiceError::Agent`. `set_messages` stays for the wholesale re-seeds
(`auto_compaction.rs:341`, `compaction.rs:256`, `forking.rs:328`, `builder.rs:1532`).

**Guarantee gained.** `RunCtx` cannot be constructed in `Continue` mode without a `ResumePoint`;
the precondition has one home; the snapshot validated is the snapshot run; `skip_initial_steering_poll`
cannot disagree with the entry kind. **Not gained.** `set_messages` is still a wholesale
replacement callable between runs; correctness of the two session predicates; anything a hook's
`TurnUpdate::context` does mid-run.

**Open question for `/exec`.** Whether `edit_transcript` should also be refused during the
post-run gap that `session/run.rs:72-74` (AGENT-030) describes — the agent latch is released
between `agent_end` and the driver's `continue_run`; the session's `driver_tx` covers it. Decide
and document; recommendation: refuse only on the agent latch (the agent cannot see `driver_tx`),
and note the session must still gate on `is_run_active()`.

### F2 [P1] `AppRole` closed enum; `Custom` reserved for extension messages

**Pattern:** Explicit domain enum (replaces `String` + const array + string compares).

**Location.** `event.rs:66-77 AgentMessage::App { role: String, payload }`, `:85 APP_MESSAGE_ROLES`,
deserialize gate `:157-166`. Consumers: `session-svc/src/hooks.rs:100-123 app_role_payload`
(returns `None` iff `kind ∉ APP_MESSAGE_ROLES`), `:56-73` the **overload**: live `!` bash output
enters the transcript as `Custom { kind: "bashExecution", payload: <whole BashExecutionMessage> }`
(`session/bash.rs:221-227`) while a RESUMED bash message enters as `App { role: "bashExecution", .. }`
(`event.rs:435-438`). Two representations of one domain message, reconciled by a string compare
whose documented failure mode (`hooks.rs:60-73`) is rendering the raw JSON as a user turn and
ignoring `excludeFromContext`, so `!!` output reaches the model.

**Concrete failure mode.** `AgentMessage::App { role: "anything".into(), payload }` constructs,
serializes (payload verbatim, `event.rs:118`), and can never deserialize (`:83-85` rejects unknown
roles) — a transcript that round-trips through `--json` or the extension `session_before_compact`
payload breaks on read. Nothing prevents it today; `raw_role_tag` (`session-svc/src/event.rs:455-466`)
is the only producer and happens to be correct.

**Proposed API.**
```rust
// cyrup-agent/src/event.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppRole { BashExecution, BranchSummary, CompactionSummary }
impl AppRole {
    pub const fn as_str(self) -> &'static str { /* "bashExecution" | ... */ }
    pub fn parse(s: &str) -> Option<Self>;   // replaces APP_MESSAGE_ROLES.contains
}
pub enum AgentMessage { /* … */ App { role: AppRole, payload: serde_json::Map<String, Value> } }
```
`APP_MESSAGE_ROLES` is deleted (or kept as `AppRole::ALL` for the deserialize gate). Serde shape
unchanged (B1): `App` still serializes `payload` verbatim; deserialize routes via `AppRole::parse`.
Consumers: `session/bash.rs:221-227` constructs `App { role: AppRole::BashExecution, payload }`
directly; `hooks.rs:100-123 app_role_payload` is deleted and the `Custom` arm of
`coding_agent_convert_to_llm` (`:74-84`) loses its overload branch — `Custom` is now only ever
an extension `customType`; `session-svc/src/event.rs:455-466 raw_role_tag` returns `AppRole`;
`prompt_runtime.rs:1002-1007` unchanged (matches `Custom.kind` against its own allowlist).
TUI/subagents unchanged (wire identical).

**Open question for `/exec`.** How a live bash `App` message is persisted. `bash.rs` pushes via
`flush_pending_bash_messages` → `set_messages` (no `MessageEnd`, so `SvcSubscriber:148-190`'s
persist arm is not involved) — confirm by reading `bash.rs:200-290` and, if a `Custom`-only persist
path exists (`subscriber.rs:172 append_custom_message`), add the matching `App` arm.

**Guarantee gained.** An `App` with an unknown role is unrepresentable; the live/resumed bash
split disappears; `convert_to_llm` matches exhaustively on `AppRole`. **Not gained.** Payload
shape inside `App` remains an opaque map (by design — `cyrup-agent` must not know the
coding-agent message types, `event.rs:56-70`).

### F3 [P1] `TerminateHint` — one tri-state for `terminate`

**Pattern:** Explicit domain enum in `cyrup-core` replacing four encodings of one fact.

**Location.** `cyrup-core/src/tool.rs:41 ToolResult.terminate: bool` (doc: `false` = absent),
`:52 ToolUpdate.terminate: Option<bool>`; `cyrup-agent/src/hooks.rs:47-64 BeforeOutcome::Block.terminate: bool`,
`:98 AfterOverride.terminate: Option<bool>`, `:80 AfterToolCall.terminate: Option<bool>`;
`tools/mod.rs:30-37 Finalized.terminate: Option<bool>`; `message.rs:66-92 result_value_of(.., terminate: Option<bool>)`
emits the key iff `Some`; `finalize.rs:39-46` performs the lossy `if r.terminate {Some(true)} else {None}`
and carries a `[CYRUP-DELTA]` admitting "a tool that wants pi's explicit `terminate: false` cannot
express it". Cross-crate duplicate: `cyrup-ext/src/contract.rs:190-199 Reduced::Blocked.terminate: bool`,
`:20-31 HookOutcome::Block`.

**Concrete failure mode.** A tool returning `ToolResult { terminate: false, .. }` and one returning
`..Default::default()` produce identical wire (`terminate` absent) — pi's explicit `false` is
unrepresentable, and `after_tool_call`'s `Some(false)` override IS representable, so the two
paths disagree on what "false" means. `shouldTerminateToolBatch` = every finalized result has
`Some(true)`; a future reader of `Finalized.terminate == None` cannot tell "tool said nothing"
from "tool said false".

**Proposed API.**
```rust
// cyrup-core/src/tool.rs
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminateHint {
    /// pi `undefined` — key absent on the wire, does not contribute to the batch fold.
    #[default] Unspecified,
    Terminate,
    /// pi explicit `false` — key PRESENT as `false`.
    Continue,
}
impl TerminateHint {
    pub const fn requested(self) -> bool { matches!(self, Self::Terminate) }
    pub const fn wire(self) -> Option<bool> { /* Unspecified → None */ }
}
pub struct ToolResult { /* … */ pub terminate: TerminateHint }
pub struct ToolUpdate  { /* … */ pub terminate: TerminateHint }
```
`BeforeOutcome::Block.terminate`, `AfterOverride.terminate` (`Option<TerminateHint>` — `None` =
"hook has no opinion", distinct from `Some(Unspecified)` = "hook clears it"), `AfterToolCall.terminate`,
`Finalized.terminate` all become `TerminateHint`; `result_value_of`/`update_value` take
`TerminateHint` and call `.wire()`. The batch fold at `exec.rs:245-247, 341-343` becomes
`.all(|f| f.terminate.requested())`. `cyrup-ext/src/contract.rs` `Reduced::Blocked.terminate` and
`HookOutcome::Block.terminate` become `TerminateHint`; `EventPatch::ToolResult` gains `terminate:
Option<TerminateHint>` so `apply_patch` and `AfterOverride` agree.

**Tendrils (grep to confirm during `/exec`).** `cyrup-tools`: every `ToolResult { .. }` literal —
those using `..Default::default()` compile unchanged; any `terminate: true` becomes
`TerminateHint::Terminate`. `cyrup-ext` guest tool-result conversion (`host/` — locate the
`ToolResult` constructor fed from the WIT result). `cyrup-ext-subagents` (`ToolResult` literals, if
any). `cyrup-agent` tests: `tool_result_model.rs` (11 tests) assert the `terminate` key
presence/absence — they stay as-is and become the parser tests for `.wire()`.

**Guarantee gained.** The wire presence/absence of `terminate` is derived from a three-valued
type, not from a lossy `bool`; the batch fold reads one predicate. **Not gained.** The fold
rule itself (`every`, `finalizedCalls.length > 0`) — still a runtime rule with its existing tests.

### F4 [P2] `stream_assistant` as a functional core: `AssistantStream` accumulator

**Pattern:** Functional Core / Imperative Shell, with a two-phase private enum (NOT a generic
typestate — the shape is stable and small).

**Location.** `stream.rs:141-250`: a `select!` loop mixing (a) cancellation, (b) `StreamEvent`
consumption, (c) `partial` refresh, (d) emission of `MessageStart`/`MessageUpdate`/`MessageEnd`,
(e) the `started: bool` flag, (f) three synthesized-terminal paths (abort `:155-176`, `Done`/`Error`
`:200-208`, EOF `:230-238`). The "MessageStart exactly once, before MessageEnd, on every path"
invariant is enforced by three copies of `if !started { emit(MessageStart) }` (`:155-161`,
`:188-193`, `:239-244`).

**Concrete failure mode.** A fourth exit path (e.g. a per-turn deadline, or handling
`StopReason::Deferred` as a settled terminal) is added and omits the `!started` dance →
`MessageEnd` with no `MessageStart` → `SvcSubscriber`'s `streaming_message` state and the TUI's
turn interleaving (`cyrup-tui/src/tests/turn_interleaving.rs`) desync. Nothing in the types
prevents it; only the parity tests would notice, and only if they exercise that path.

**Proposed API (all private to `agent/run/`).**
```rust
// agent/run/assistant_stream.rs — pure, no tokio, no emit
pub(super) struct AssistantStream { partial: Arc<AssistantMessage>, phase: Phase }
enum Phase { Unstarted, Started }
pub(super) enum Step {
    /// Emit `MessageStart(partial)`.
    Start(Arc<AssistantMessage>),
    /// Emit `MessageUpdate { partial, event }`.
    Update { partial: Arc<AssistantMessage>, event: StreamEvent },
    /// Stop consuming; then call `settle`.
    Terminal(AssistantMessage),
    /// Pre-start block event, or a post-terminal stray: nothing to emit.
    Ignore,
}
/// What the shell must emit to close the message: `start` is `Some` iff no `Start` was
/// ever yielded, so exactly-once is decided here, not at three call sites.
pub(super) struct Settled { pub start: Option<Arc<AssistantMessage>>, pub end: AssistantMessage }
impl AssistantStream {
    pub(super) fn new(model: &ModelRef) -> Self;              // seeds `empty_assistant`
    pub(super) fn on_event(&mut self, ev: StreamEvent) -> Step;
    pub(super) fn settle(self, terminal: AssistantMessage) -> Settled;   // consumes
    pub(super) fn settle_aborted(self) -> Settled;                       // stamps Aborted + "Request was aborted"
    pub(super) fn settle_eof(self, model: &ModelRef) -> Settled;         // "stream ended without a terminal event"
}
```
The shell (`stream_assistant`) keeps: hook calls, `StreamOptions`/`Context` assembly, the
`select!`, and a single emission tail: `if let Some(p) = settled.start { emit(MessageStart) }
emit(MessageEnd(settled.end))`. `on_event`'s post-terminal handling makes the "Pi returns
immediately on the terminal" rule (`:196-199`) a property of the accumulator.

**Guarantee gained.** `Settled` can only be produced by consuming the stream, and it carries
the start decision; the three emission sites become one. Every `StreamEvent` sequence becomes a
pure unit test (no faux provider, no runtime). **Not gained.** The shell can still forget to emit
what `Settled` says (one site, reviewable); provider misbehaviour is still runtime.

**Rejected alternative.** Generic typestate `AssistantStream<Unstarted>` / `<Started>` —
the phase is decided by the provider's event stream at runtime, so the shell would need a boxed
enum anyway; the private `Phase` enum + consuming `settle` gives the same exactly-once guarantee
without the generics.

### F5 [P2] Modelless agent as a state, not a sentinel

**Pattern:** `Option` at the boundary (an invalid value made unrepresentable), with a new
`AgentError` variant.

**Location.** `state.rs:80 StateInner.model: ModelRef` (non-optional). `session-svc/src/builder.rs:1587-1602`
(SEAM-075) seeds `ModelRef { provider: "", api: None, model: "" }` for a credential-less session and
documents it as "unreachable while the session is modelless" because `prepare_and_assemble`
(`run.rs:438-440`) returns `NoModelSelected` first. The sentinel is readable through
`Agent::snapshot().model` (`thinking.rs:18,59`, `tools.rs:143`, `model.rs`) and is what
`emit_run_failure` (`run/mod.rs:230`) and the `catch_unwind` twin (`lifecycle.rs:298`) would stamp
onto an errored assistant message.

**Concrete failure mode.** Any future path into `start_run` that bypasses the session preflight
(a `loop_fn` embedder, `continue_run` from an extension control op, a test) streams against
`""/""` and produces an assistant message with `provider: ""` that then lands in the transcript.
The comment is the only guard.

**Proposed API.**
```rust
// state.rs
pub(crate) struct StateInner { pub model: Option<ModelRef>, /* … */ }
pub struct AgentStateSnapshot { pub model: Option<ModelRef>, /* … */ }
// builder.rs
impl AgentBuilder { pub fn new(stream_fn: Arc<dyn StreamFn>) -> Self; pub fn model(self, m: ModelRef) -> Self; }
// facade.rs
impl Agent { pub async fn set_model(&self, m: Option<ModelRef>); }
// error.rs
pub enum AgentError { /* … */ NoModelSelected }
```
`start_run` returns `Err(AgentError::NoModelSelected)` when `None` (checked under the same lock as
the snapshot, before the latch is claimed). `RunCtx.model: ModelRef` stays non-optional — the
`Option` ends at the run boundary. `TurnUpdate.model` unchanged. Consumers: `builder.rs:1587-1602`
sentinel + comment deleted, `.model(m)` applied only when `Some`; `session/tools.rs:135-144
next_turn_model_baseline` returns `Option` and `hooks.rs:319-321` sets `update.model` only when
`Some`; `thinking.rs:58-61` already clamps modelless to `Off`; `session/model.rs` passes `Some`;
`accessors.rs`/`sdk handle.rs:356 model()` already `Option`. `loop_fn::AgentLoopConfig.model`
stays `ModelRef` (an embedder always has one).

**Guarantee gained.** No assistant message can be produced under an empty provider/model id;
"no model" is an `AgentError` a caller can match, not a string compare. **Not gained.** The
session's richer `NoModelSelected` preflight stays (it fires before compaction/auth); a `ModelRef`
with `api: None` is still valid (`empty_assistant` falls back to `UNRESOLVED_API`) — see Rejected.

### F6 [Secondary, P2] Tool-call pipeline: constructor discipline + pure finalize fold

**Location.** `tools/mod.rs:26-45 Finalized{source_index,..}` and `Prep::{Immediate(Box<Finalized>), Ready{tool,args}}`;
`preflight.rs:128-164 immediate_error` sets `source_index: 0` and callers patch it after
(`exec.rs:70`, `:255`); `fail_truncated_tool_calls` (`mod.rs:84-117`) never patches it.
`exec.rs:52-58 struct Deferred` is the prepared-call record but is local to `execute_parallel`.
`finalize.rs:16-154` is async (awaits `after_tool_call`) and then performs the replace-not-merge
fold inline.

**Proposed.** Promote `Deferred` to `pub(super) struct PreparedCall { source_index, tool, args, call_id, tool_name }`
returned as `Prep::Ready(PreparedCall)`; `immediate_error(call, source_index, msg, terminate)`
requires the index; `Finalized` gets a private constructor. Split `finalize` into
`async fn after_hook(&self, …) -> Option<AfterOverride>` (shell) and
`fn fold_tool_outcome(call, source_index, args, outcome: Result<ToolResult, ToolError>, over: Option<AfterOverride>) -> Finalized`
(pure; the replace-not-merge table in one testable function). Both runtimes call the pair.
Internal only; zero tendrils. Typestate across the parallel runtime is rejected — completion
order is genuinely dynamic (`JoinSet` + channel).

### F7 [Secondary, P2] `Hooks` failure-mode map in the signatures

**Location.** `hooks.rs:210-283`. Four hooks abort the run on `Err` (`transform_context`,
`convert_to_llm` → `stream.rs:41-52`; `prepare_next_turn`, `should_stop_after_turn` → `turn.rs:145,183`),
two degrade per call (`before_tool_call` → `preflight.rs:88`; `after_tool_call` → `finalize.rs`).
Only comments say which is which.

**Proposed.** Per the review's own rule (expected per-item outcome → enum; aborting failure →
`Result`): `before_tool_call(..) -> BeforeOutcome` with `BeforeOutcome::{Proceed, Block{reason: Option<String>, terminate: TerminateHint}, Failed(String)}`;
`after_tool_call(..) -> AfterOutcome::{Keep, Override(AfterOverride), Failed(String)}`; the four
run-aborting hooks keep `Result<_, HookError>`. Consumers: `session-svc/src/hooks.rs:231-265`
(the `?` on the inner delegate becomes a match), `cyrup-ext/src/hooks.rs:33-67,72-125` (already
never returns `Err`), `cyrup-agent/src/tests/hook_failure_text.rs` (4 tests asserting the hook's
own text reaches the model — unchanged in intent, updated to construct `Failed`). Schedule LAST:
broadest trait break, smallest guarantee. Confidence medium — `HookError::Serde` via `?` inside
impls is lost for the two per-call hooks; `/exec` decides whether `Failed(String)` or
`Failed(HookError)` is the payload.

### F8 [Secondary, P3] One run-in-flight fact, two flags

**Location.** `state.rs:85 StateInner.is_streaming` (set `lifecycle.rs:273`, cleared
`SettlementGuard::drop :80`, and by `reset`) vs `Agent::running_tx` latch (`agent/mod.rs:68-73`,
claimed `lifecycle.rs:249-256`, released `:96`). Both are written in the same two places and mean
the same thing, but the session reads different ones for different decisions: `run.rs:75`
`is_run_active()` (latch + `driver_tx`) for the prompt gate, `inject.rs:41,76,127` and
`control.rs:236` `is_streaming()` (= `snapshot().is_streaming`) for queue-vs-run routing.
**Proposed.** Delete the field; `snapshot().is_streaming` reads `*running_rx.borrow()`. Then
audit the four `is_streaming()` call sites in session-svc against AGENT-030 (`run.rs:72-74`) and
either switch them to `is_run_active()` or document why the narrower predicate is correct there.
Internal + a four-site audit. P3 because the write sites are already co-located and the risk is
in the session's predicate choice, which this refactor exposes rather than fixes.

---

## Part F — Deliberately rejected (the document's section 5; expand each to a paragraph)

| Candidate | Why rejected |
|---|---|
| Typestate on `Agent` (`Agent<Idle>`/`Agent<Running>`) | `Agent` is `Arc`-shared across session, TUI, extensions; state is externally driven (abort from UI, `SettlementGuard` on another task). The `watch<bool>` CAS + `wait_for_idle` IS the right design (`lifecycle.rs:53-100`, `settlement_latch.rs` 3 multi-thread tests). |
| `PartialAssistant` vs `AssistantMessage` split (`StopReason::Pending` only valid on a partial, `stop_reason.rs:76-78`) | `StreamEvent::*.partial: Arc<AssistantMessage>` is the provider seam (B3) and the TUI renders partials as assistant messages; pi models it identically. Blast radius across provider/tui for a state that is genuinely dynamic. Keep the `Pending` seed in `empty_assistant` and F4's `Settled` as the guard. |
| `WorkingTranscript` newtype for `RunCtx::messages` vs `StateInner::messages` | Already distinguished by element type (`Vec<Arc<AgentMessage>>` vs `Vec<AgentMessage>`, PERF-001) — a direct assignment does not compile today. Make the distinction deliberate with a doc line on `RunCtx::messages`, not a wrapper. |
| `TurnOutcome` enum over `run_loop` (`turn.rs`) | The sequencing IS the spec (Pi `runLoop` line-for-line, with hooks awaited between decisions); an enum would restate the control flow without removing a check and would invite parity drift in the one place the 13 `agent_loop.rs` tests pin most tightly. |
| `BlockReason` newtype for the JS-falsy empty-string rule (`preflight.rs:104-107`) | Single consumer site; producer is cross-crate (`cyrup-ext` guest `block(some(""))`). The normalisation is at the only reader — correct placement. |
| `ApiKey` newtype (`stream.rs:65-70`, resolver result filtered for empty, static `gen_config.api_key` NOT filtered) | One site. Note the asymmetry as an **open question** (does pi's `\|\| config.apiKey` also fall through an empty static key?) and, if so, fix with a one-line `.filter(!empty)` on the static branch — not a type. |
| `RunFailure { source, message }` | Diagnostic value only; the message text is what reaches the wire and must stay the raw `e.to_string()`. |
| Collapsing `AgentContext` + `AgentLoopConfig` into `RunCtx`'s internal grouping as a public type | `loop_fn` has zero consumers so it is free, but the split mirrors Pi's `AgentContext`/`AgentLoopConfig` and the SDK re-exports the module wholesale; keep the public shapes, group internally (`RunShared` handles + `RunBaseline` snapshot) as part of F1's `RunCtx::new` cleanup. |
| `ModelRef.api: Option<ApiId>` → required | Two construction sites disagree (`session/model.rs:318 api: None` vs `:480-484 Some`) and `empty_assistant` papers over it with `UNRESOLVED_API`. Real, but a `cyrup-core`/`cyrup-provider` change outside this artery; record as a follow-up, not a finding. |
| Deriving serde on `AgentMessage` | Never — `event.rs:86-102` records the duplicate-`role`-key bug the hand-written impl fixes; B1. |

---

## Part G — Incremental migration plan (workspace compiles after every step)

Each step is one `/split` task; steps 1–3 and 5–7 have an agent-side commit followed by
consumer commits inside the same task. Verify with `cargo check --workspace` and
`cargo test -p cyrup-agent` after each.

1. **F3 `TerminateHint`** — `cyrup-core/src/tool.rs` → `cyrup-agent` (`hooks.rs`, `tools/*`, `message.rs`)
   → `cyrup-tools` literals → `cyrup-ext/src/contract.rs` + guest result conversion →
   `cyrup-ext-subagents` literals. Leaf-most; unlocks F6's pure fold.
2. **F2 `AppRole`** — `cyrup-agent/src/event.rs` → `session-svc/src/{hooks.rs, event.rs, session/bash.rs}`.
   Wire unchanged; TUI/subagents/it untouched.
3. **F1 `RunEntry` + `ResumePoint` + `Agent::edit_transcript`/`pop_trailing_assistant_if`** —
   `cyrup-agent/src/{agent/run/mod.rs, agent/lifecycle.rs, loop_fn.rs, agent/facade.rs, error.rs}`
   → `session-svc/src/session/{retry.rs, auto_compaction.rs, bash.rs, run.rs:164}`. Group
   `RunCtx::new`'s 17 args into `RunShared` + `RunBaseline` in the same step.
4. **F6 tool pipeline** — `cyrup-agent/src/agent/run/tools/*` only.
5. **F4 `AssistantStream`** — new `cyrup-agent/src/agent/run/assistant_stream.rs`; `stream.rs`
   shrinks to the shell.
6. **F5 `Option<ModelRef>`** — `cyrup-agent/src/{state.rs, agent/builder.rs, agent/facade.rs, agent/lifecycle.rs, error.rs}`
   → `session-svc/src/{builder.rs:1587-1602, session/tools.rs, hooks.rs:319-321, session/model.rs, session/thinking.rs}`.
7. **F7 `Hooks` outcome enums** — `cyrup-agent/src/hooks.rs` + `preflight.rs` + `finalize.rs`
   → `session-svc/src/hooks.rs` → `cyrup-ext/src/hooks.rs` (+ `contract.rs` `Reduced::Blocked`)
   → `cyrup-agent/src/tests/hook_failure_text.rs`.
8. **F8 `is_streaming` collapse** — `cyrup-agent/src/{state.rs, agent/lifecycle.rs, agent/facade.rs}`
   → audit `session-svc/src/session/{inject.rs, control.rs}` predicates.

Incidentals ride along: `QueueMode: FromStr` (step 3 or 6, whichever touches `builder.rs` first),
`ExtensionHost::subscriber` param drop (step 7), `cyrup-sdk` by-name `AgentMessage` re-export (step 2).

---

## Part H — Test implications (the document's section 7; analysis, not new work)

- **Become direct parser/pure tests:** `tool_result_model.rs` (11) → `TerminateHint::wire`;
  the continue-precondition assertions in `agent_loop.rs`/`round2_parity.rs`/`area02_backlog.rs`
  (grep `NoMessages|ContinueFromAssistant`) → `ResumePoint::check`; the abort/EOF/post-terminal
  cases in `model_boundary.rs` (18) → `AssistantStream::{on_event, settle_*}` sequences.
- **Remain necessary (behaviour):** all event-sequence parity tests (they pin B1/B2 and Pi
  ordering); `settlement_latch.rs` (3, multi-thread); `pending_containment.rs`; `hook_failure_text.rs`
  (text reaching the model); session-svc `round2.rs`/`round3.rs`/`mid_run_tool_anchoring.rs`;
  `cyrup-it` `embedder_seams.rs` (B3) and `wasm_renderer_screen.rs` (B1 via WASM).
- **Become redundant only once the type guarantee is complete:** the `skip_initial_steering_poll`
  coupling tests (after F1); `source_index` patching (after F6). Do not delete before.
- **Compile-fail (`trybuild`):** low value — every guarantee here is `pub(crate)` except
  `TerminateHint` (no `From<bool>`) and `AppRole` (no `From<String>`); a doc-test with
  `compile_fail` on each is sufficient.
- **Still shell-level:** anything crossing B1–B5 — the TUI serde projections, the NDJSON child
  protocol, the session JSONL round-trip, and the WASM `HostEvent` conversion — need a
  two-sided byte-pin per boundary (the sweep found none for the `Custom{kind,payload}` ↔
  `Custom{customType,content}` bridge at `session-svc/src/event.rs:423-431`).

---

## Instructions for `/exec`

1. Read Parts B–H; open each cited file and confirm the line ranges at HEAD (they were taken at
   `2cfff0f`). Correct any drift in the document, not here.
2. Resolve the four **open questions** (F1 post-run gap; F2 bash persistence; F7 `Failed`
   payload; `ApiKey` static-branch asymmetry) by reading the named code; record each answer under
   the finding.
3. Write `.flux/research/CORE_LOOP_TYPE_REVIEW.md` with the seven sections of Part A in order.
   Main findings F1–F5 with all twelve sub-fields each; F6–F8 under "Secondary findings" with the
   same sub-fields. Section 4's sketch is F1 (before: the three duplicated checks + the bool +
   the session's three pop/set triplets; after: `RunEntry`/`ResumePoint`/`edit_transcript` and the
   `continue_run` call site). Section 5 expands Part F. Section 6 is Part G with per-step file
   lists. Section 7 is Part H.
4. For every finding include a **Tendrils** table: `| Crate | File:lines | Change | Mechanical / Semantic |`
   built from Part D, and a **Boundary** line stating where the type ends and why (Part C).
5. Do not modify any source file. Do not create files other than the research document.

## Definition of done

- `.flux/research/CORE_LOOP_TYPE_REVIEW.md` exists with sections 1–7 in order.
- Five main findings (F1–F5) and three secondary (F6–F8), each with all twelve sub-fields, a
  tendrils table, and a boundary statement; every location cites `file:symbol:lines` verified at HEAD.
- Section 5 contains every row of Part F, expanded.
- Section 6 is an ordered list where each step names the crates/files it touches and asserts
  `cargo check --workspace` green at its end; the order matches Part G unless `/exec` finds a
  dependency that forces a swap (then say why).
- Each of the four open questions has a recorded answer.
- Every proposed change is checked against B1–B6; any that would alter a boundary is either
  reshaped to preserve it or moved to Part F.
- No source files modified.

## Acceptance Criteria

- [ ] Review document written to `.flux/research/CORE_LOOP_TYPE_REVIEW.md` containing all seven
      required output sections in order.
- [ ] Every finding cites `file:symbol:line-range` verified at HEAD.
- [ ] Five main findings + three secondary, each with all twelve sub-fields, a tendrils table
      and a boundary statement.
- [ ] "Deliberately Rejected Opportunities" section is non-empty and covers every row of Part F.
- [ ] Every proposed change is checked against the Pi-parity constraint (no change to the
      emitted `AgentEvent` sequence, message contents, error strings) and against boundaries B1–B6.
- [ ] Every newtype/typestate is followed outward to the boundary where it ends, with the reason.
- [ ] The migration plan keeps `cargo check --workspace` green after every step and is shaped so
      `/split` can cut one task per step.
- [ ] The four open questions are answered in the document.
- [ ] No source files are modified by this task.
