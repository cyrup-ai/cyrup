---
title: Core Loop Type-Driven Design Review
stage: research
verified_at: c9da9fb (source tree identical to main 2cfff0f)
source_task: .flux/todo/CORE_LOOP_TYPE_REVIEW.md
---

# Core Loop Type-Driven Design Review

A type-driven design opportunity review of the agent "core artery" — the inference loop in
`cyrup-agent` above the provider abstraction — followed outward along every tendril into the
crates that consume it. This is a review with minimal design sketches, not a code change. Its
migration plan (§6) is shaped so `/split` can cut one implementation task per step.

**Every `file:symbol:lines` below was re-verified at HEAD.** The task file's Part B said its
citations were "as of `2cfff0f`"; at that exact commit `lifecycle.rs` was off by ~54 lines and
`loop_fn.rs` by ~23 (the research predates a later re-layout of those files). Nothing below
inherits a line number from the task — each was located by symbol and re-read.

All four of the task's open questions are resolved inline under their findings and collected in
§3's "Resolved open questions" box.

---

## 1. Executive Verdict

The core loop needs a **combination** of newtypes-with-parsing, explicit domain enums, and one
Functional Core / Imperative Shell extraction — and **no typestate**. That last point is a
conclusion, not an omission: the loop's phases are decided by a provider's event stream, a
cancellation token owned by another task, and hooks awaited between decisions, so the compiler
cannot know the phase at any program point where it would help. Every place typestate looked
attractive is either already correctly modelled by a runtime latch (`Agent`'s `watch<bool>`
CAS), or is genuinely dynamic (the tool `JoinSet`), and is rejected in §5 by name.

The highest-value opportunity is **F1: continuation as a parsed value**. The precondition
"transcript non-empty and not ending in an assistant message" is checked in three places, is
coupled to a separate `bool` that is the seventeenth argument to `RunCtx::new`, and — the part
the task under-sells — is validated against one snapshot and then run against a *second*
snapshot taken under a fresh lock. That is a real race in the agent loop, and `ResumePoint`
closes it almost for free. The session works around the missing guarantee by hand in three
places and then swallows the error the type should have prevented.

Behind it, three findings are textbook instances rather than pattern-hunting: a closed `AppRole`
enum replacing a `String` + const array + a documented two-representation overload (F2); a
three-valued `TerminateHint` replacing four encodings of one fact, where the code's own
`[CYRUP-DELTA]` already concedes information loss (F3); and `Option<ModelRef>` replacing an
empty-string sentinel whose only guard is a comment reading "unreachable" (F5). F4 extracts the
exactly-once `MessageStart` invariant from a mutable `bool` checked across three exit paths into
a pure accumulator.

The major tradeoff is scope: the task explicitly puts breaking changes to the public `Agent`,
`loop_fn` and `Hooks` surfaces on the table, and F1 and F7 use that licence. Each type is followed
outward to the boundary where it legitimately ends (§3 "Boundary" lines; the six immovable
boundaries B1–B6 are restated in §2). Nothing here changes the emitted `AgentEvent` sequence, any
message's wire bytes, or any error string — the parity tests remain the gate.

No domain information is missing for a confident conclusion on F1–F5. F7's payload choice and
F8's session-predicate audit are settled here rather than deferred (see the resolved questions).

---

## 2. Invariant and State Map

Boundaries first, because every row below is constrained by them:

| # | Boundary | Must not change | Why |
|---|---|---|---|
| B1 | `AgentMessage` serde shape (`event.rs:93` explains the hand-written impl) | tag key/values, `Assistant` flattening, `Custom{kind,payload}` keys | read by string projection with no type dependency in `cyrup-tui/src/app/event_extract.rs:185,196-198`; duck-typed by `cyrup-ext-subagents` |
| B2 | `AgentEvent` serde (`tag="type"`, camelCase fields) | attributes, variant + field names | `ndjson.rs:26-29`: a missing required key fails the **whole line silently** |
| B3 | `StreamFn::stream(&ModelRef,&Context,&StreamOptions)`; `ApiKeyResolver` | signatures | three impls incl. `cyrup-it` embedder seams |
| B4 | persisted transcript = `cyrup_session::agent_message::AgentMessage` (own enum) | JSONL bytes | bridge is `session-svc/src/event.rs:416-455` only |
| B5 | `HostEvent::Context{messages: Vec<Arc<AgentMessage>>}` | the `Arc` | `prompt_runtime.rs:1042,1055` uses `Arc::ptr_eq` as "was this rewritten?"; `cyrup-ext/src/hooks.rs:137` forbids unwrapping |
| B6 | `Reduced::Blocked{reason, terminate: bool, by}` (`contract.rs:196`) mirrors `BeforeOutcome::Block` | change both sides together | F3, F7 |

The invariants:

| Location | Domain fact or state | Current encoding | Failure mode | Best representation |
|---|---|---|---|---|
| `loop_fn.rs:197,200` · `:279,282` · `lifecycle.rs:185,209` | a run may resume only if the transcript is non-empty and does not end in an assistant message | the same two `if`s, three times; `EntryStart::Continue` (`run/mod.rs:23`) carries no proof | `continue_run` validates a snapshot at `:185/:209`, then `start_run` re-snapshots at `:270` under a fresh lock — `set_messages` in between admits a resumable-looking run on an unresumable transcript | **newtype proof** `ResumePoint`, one home (F1) |
| `run/mod.rs:107`, `lifecycle.rs:199,211,219` | `skip_initial_steering_poll` is `true` iff the entry is a steering-drain prompt | a separate `bool`, 17th argument to `RunCtx::new` | nothing ties the flag to the entry kind; `loop_fn` passes `false` unconditionally | derived from a **domain enum** `PromptSource` (F1) |
| `session-svc/…/retry.rs:141-147`, `auto_compaction.rs:395-410`, `bash.rs:279-282` | "pop the trailing assistant iff P" is an atomic transcript edit | `snapshot()` → `pop()` → `set_messages()` across two awaits, no lock | interleaves with the reducer; `run.rs:170` then swallows the resulting `ContinueFromAssistant` with `Err(_) => break` | `Agent::edit_transcript` under the state lock (F1) |
| `event.rs:71-76,85,179-186` | an `App` message's role is one of exactly three | `role: String` + `APP_MESSAGE_ROLES` + `.contains` | `App{role:"anything"}` constructs and serializes but can never deserialize | **closed enum** `AppRole` (F2) |
| `session-svc/src/hooks.rs:60-75`, `bash.rs:221-222`, `event.rs:435-436` | a bash execution is one domain message | live: `Custom{kind:"bashExecution"}`; resumed: `App{role:"bashExecution"}` | reconciled by a string compare whose documented failure renders raw JSON as a user turn | one variant, `App{role: AppRole::BashExecution}` (F2) |
| `tool.rs:42,56` · `hooks.rs:62,84,109` · `tools/mod.rs:35` · `contract.rs:196` | a tool's early-termination hint is tri-state (unspecified / terminate / explicit continue) | `bool`, `Option<bool>`, `bool`, `Option<bool>` — four encodings | `finalize.rs:51` does `if r.terminate {Some(true)} else {None}`; `:43-45` admits explicit `false` is unrepresentable | **domain enum** `TerminateHint` (F3) |
| `stream.rs:143,158,195,240` | `MessageStart` is emitted exactly once, before `MessageEnd`, on every exit path | `let mut started = false` guarded at two exits, set at a third | a fourth exit path that forgets the dance desyncs the TUI's turn interleaving | **functional core** accumulator that decides start-once on `settle` (F4) |
| `state.rs:90`, `builder.rs:1587-1591` | an agent may have no model | `ModelRef{provider:"",model:""}` sentinel, guarded by a comment | any path into `start_run` that skips the session preflight streams against `""/""` | `Option<ModelRef>` ending at the run boundary + `AgentError::NoModelSelected` (F5) |
| `tools/mod.rs:26-27`, `preflight.rs:128`, `exec.rs:70,278` | every `Finalized` carries its source index | `immediate_error` writes `source_index: 0`; two callers patch it after; `fail_truncated_tool_calls` never does | a finalized result silently attributed to slot 0 | index required by the constructor (F6) |
| `hooks.rs:212-279` | four hooks abort the run on `Err`; two degrade per call | all six return `Result<_, HookError>`; only comments say which is which | an implementor cannot tell a run-aborting `?` from a per-call one | per-call hooks return a **domain outcome enum** (F7) |
| `state.rs:94` vs `agent/mod.rs:70` | a run is in flight | `is_streaming: bool` **and** `running_tx: watch<bool>`, written at the same two sites | the session reads different ones for different decisions (`run.rs:75,325` vs `inject.rs:41,76,127`, `control.rs:236`) | one fact, one field (F8) |

---

## 3. Findings

Ranking: impact × likelihood × how much the compiler can close × migration cost. F1–F5 are the
main findings; F6–F8 follow under "Secondary" with the same sub-field structure so `/split` can
cut them identically. Each finding ends with a **Tendrils** table (from the consumer sweep) and a
**Boundary** line naming where the type stops and why.

#### [P1] F1 — Continuation as a parsed value: `RunEntry` + `ResumePoint` + `Agent::edit_transcript`

**Location.**
`cyrup-agent/src/loop_fn.rs` — `run_agent_loop_continue` (`:184`, checks at `:197,200`),
`agent_loop_continue` (`:267`, checks at `:279,282`). `cyrup-agent/src/agent/lifecycle.rs` —
`continue_run` (`:168`, checks at `:185,209`; calls `start_run` at `:199` with `true`, `:211`
and `:219` with `false`), `start_run` (`:222`; the second snapshot at `:258-273`, `messages` at
`:270`). `cyrup-agent/src/agent/run/mod.rs` — `EntryStart` (`:21-24`), `RunCtx::new` (`:90`,
seventeen parameters, `skip_initial_steering_poll` at `:107`).
Consumers: `cyrup-session-svc/src/session/retry.rs:141-147` (`drop_trailing_assistant`),
`session/auto_compaction.rs:395-410` (the narrow pop, with its "Do not reuse that helper here"
warning at `:397`), `session/bash.rs:279-282` (`append_bash_message`), `session/run.rs:164-170`
(`Err(_) => break`).

**Current representation.**
The precondition is two `if`s — `is_empty()` → `NoMessages`, `last() is Assistant` →
`ContinueFromAssistant` — written out three times. `EntryStart::Continue` is a unit variant
carrying no evidence the check ran. Whether the run must skip its first steering poll is a
separate `bool` that the caller sets by hand to match the entry kind.

**Invariant or legal sequence.**
A run may enter `Continue` only on a transcript that is non-empty and does not end in an
assistant message, and *the transcript validated must be the transcript run*. The initial
steering poll is skipped exactly when the entry is a prompt synthesised by draining the steering
queue (pi `skipInitialSteeringPoll`, agent.ts:351,440-446).

**Concrete failure mode.**
Real, in HEAD: `continue_run` takes the state lock, snapshots `messages`, validates, and
releases; `start_run` then takes the lock again at `:258` and snapshots `messages` at `:270`. An
`Agent::set_messages` on another task between the two — the session's three pop/set triplets are
exactly such callers, and each spans two awaits without a lock — lets the run enter `Continue` on
a transcript that ends in an assistant message. The provider then rejects the request. The
session's `run.rs:170` matches `Err(_) => break`, so the failure the type should prevent is
currently invisible.

**Recommended pattern.**
Newtype-as-proof ("parse, don't validate") + a domain enum replacing the `bool`. **Not
typestate**: there is no sequence of operations on an object here, only a fact about a value at
one instant.

**Why this pattern fits.**
A private-constructor proof type gives the precondition one home and makes `Continue`
unconstructible without it; a runtime enum would still need the check repeated at each entry; a
builder would not close the TOCTOU because the race is between validation and use, not between
fields. The enum replaces the `bool` because the flag is a *derived* property of the entry kind
— computing it from the enum makes disagreement unrepresentable.

**Minimal proposed API.**
```rust
// cyrup-agent/src/agent/run/mod.rs  (pub(crate) unless noted)
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

/// Proof that a transcript may be resumed without a new message.
/// Constructed ONLY by `check` — the single home of the precondition.
pub(crate) struct ResumePoint(());
impl ResumePoint {
    pub(crate) fn check(messages: &[AgentMessage], surface: ContinueSurface)
        -> Result<Self, AgentError>
    {
        if messages.is_empty() { return Err(AgentError::NoMessages(surface)); }
        if matches!(messages.last(), Some(AgentMessage::Assistant(_))) {
            return Err(AgentError::ContinueFromAssistant);
        }
        Ok(ResumePoint(()))
    }
}
```
`RunCtx::new` loses the `bool` and takes `RunEntry`; `run_entry` derives the flag. `continue_run`
takes the state lock **once**: snapshot → `ResumePoint::check` → hand the *same* `Vec` into
`start_run(entry, snapshot)`. The two `loop_fn` duplicates collapse to one
`ResumePoint::check(&context.messages, ContinueSurface::Loop)?`.

```rust
// cyrup-agent/src/agent/facade.rs  (public — the tendril)
impl Agent {
    /// Atomic transcript edit under the state lock. Refused while a run is in flight
    /// (same latch as `reset`), so it can never race the reducer.
    pub fn edit_transcript<R>(&self, f: impl FnOnce(&mut Vec<AgentMessage>) -> R)
        -> Result<R, AgentError>;
    /// The named operation both session predicates need.
    pub fn pop_trailing_assistant_if(&self, pred: impl FnOnce(&AssistantMessage) -> bool)
        -> Result<Option<Arc<AssistantMessage>>, AgentError>;
}
```

**Guarantee gained.**
`RunCtx` cannot be built in `Continue` mode without a `ResumePoint`, and `ResumePoint` has one
constructor. The snapshot validated is the snapshot run. `skip_initial_steering_poll` cannot
disagree with the entry kind. The session's three hand-rolled triplets become one locked edit.

**Guarantee not gained.**
`set_messages` remains a wholesale replacement callable *between* runs. The correctness of the
two session predicates (which assistant to pop) is still theirs. Anything a hook's
`TurnUpdate::context` does mid-run is out of scope — it replaces the loop's copy by design.

**Migration and compatibility cost.**
`cyrup-agent`: `run/mod.rs` (`EntryStart`→`RunEntry`, `RunCtx::new`), `lifecycle.rs`
(`continue_run`, `start_run`), `loop_fn.rs` (two sites), `facade.rs` (two new methods),
`error.rs` (none — existing variants reused). `cyrup-session-svc`: `retry.rs:141-147` →
`pop_trailing_assistant_if(|_| true)`; `auto_compaction.rs:395-410` →
`pop_trailing_assistant_if(|a| matches!(a.stop_reason, Error | Length))`; `bash.rs:279-282` →
`edit_transcript(|m| m.push(msg))`; `run.rs:170` stops swallowing — an `Err` there is now a bug
and surfaces as `SessionServiceError::Agent`. No serde, no wire, no `Hooks` change. Tests: the
`NoMessages`/`ContinueFromAssistant` assertions in `agent_loop.rs`, `round2_parity.rs`,
`area02_backlog.rs` become direct `ResumePoint::check` tests; the `skip_initial_steering_poll`
coupling tests become redundant only after the `bool` is gone.

**Benefit versus ceremony.**
One zero-sized proof type and one three-variant enum remove two duplicated checks, one coupled
`bool`, a real race, three consumer workarounds and one swallowed error. Earns it comfortably.

**Confidence.** High — every site was read at HEAD; the TOCTOU is visible in the lock structure.

**Resolved open question (post-run gap).** Should `edit_transcript` also refuse during the gap
`session/run.rs:72-77` (AGENT-030) describes — after `agent_end` releases the agent latch but
before the driver's `continue_run`? **Decision: no.** `is_run_active()` is `!self.is_idle()` at
`session/mod.rs:499` — a *session*-level predicate over `driver_tx`, which the agent cannot see.
`edit_transcript` refuses on the agent latch only; the session must still gate its own callers on
`is_run_active()`, and its three pop/set sites already run inside that gate. Documented on the
method.

**Tendrils.**

| Crate | File:lines | Change | Mechanical / Semantic |
|---|---|---|---|
| cyrup-agent | `run/mod.rs:21-24,90-107` | `EntryStart`→`RunEntry`; drop the `bool` | Semantic |
| cyrup-agent | `lifecycle.rs:168-219,258-273` | one lock; `ResumePoint::check` | Semantic (closes TOCTOU) |
| cyrup-agent | `loop_fn.rs:197-200,279-282` | collapse to `check()?` | Mechanical |
| cyrup-agent | `facade.rs` | add `edit_transcript`, `pop_trailing_assistant_if` | New API |
| cyrup-session-svc | `retry.rs:141-147` | → `pop_trailing_assistant_if(|_| true)` | Mechanical |
| cyrup-session-svc | `auto_compaction.rs:395-410` | → `pop_trailing_assistant_if(pred)` | Mechanical |
| cyrup-session-svc | `bash.rs:279-282` | → `edit_transcript(push)` | Mechanical |
| cyrup-session-svc | `run.rs:170` | stop swallowing | Semantic |

**Boundary.** Ends at `Agent`'s public surface. `RunEntry`/`ResumePoint` are `pub(crate)`; the
session never names them — it gains two methods and loses three workarounds. `set_messages` stays
for wholesale re-seeds (`auto_compaction.rs:341`, `compaction.rs`, `forking.rs`, `builder.rs`).

#### [P1] F2 — `AppRole` closed enum; `Custom` reserved for extension messages

**Location.**
`cyrup-agent/src/event.rs` — `AgentMessage::App { role: String, payload }` (`:71-76`),
`APP_MESSAGE_ROLES` (`:85`), the deserialize gate (`:179-186`).
`cyrup-session-svc/src/hooks.rs` — the `Custom` arm of `coding_agent_convert_to_llm` (`:74-75`),
`app_role_payload` (`:107`, the `.contains` at `:112`), the overload comment (`:60-75`).
`cyrup-session-svc/src/session/bash.rs:221-222` (live bash → `Custom{kind:"bashExecution"}`);
`cyrup-session-svc/src/event.rs:435-436` (resumed bash → `App{role: raw_role_tag(..)}`),
`raw_role_tag` (`:455`).

**Current representation.**
An open `String` validated on *deserialize* against a three-element const array, but freely
constructible. The same domain message — a bash execution — enters the transcript as `Custom`
when live and as `App` when resumed from the session file, and the LLM boundary reconciles the
two with a string compare on `kind`.

**Invariant or legal sequence.**
`App.role ∈ {bashExecution, branchSummary, compactionSummary}` — pi's three declaration-merged
coding-agent roles. `Custom` is an extension's `customType`, never one of those three.

**Concrete failure mode.**
`AgentMessage::App { role: "anything".into(), payload }` constructs and serializes (`:138`
writes `payload` verbatim) but can never deserialize (`:179` rejects it). A transcript that
round-trips through `--json` or the `session_before_compact` extension payload breaks on read.
Separately, the overload's documented failure (`hooks.rs:60-75`) is a live `!!` bash output
rendered as a raw-JSON user turn with `excludeFromContext` ignored — it reaches the model.

**Recommended pattern.** Explicit closed domain enum with exhaustive matching.

**Why this pattern fits.**
The value set is finite, stable (pi's, not ours) and inspected at runtime by the LLM boundary —
an enum is the direct representation. A newtype over `String` would still need the allowlist. A
private constructor would fix forgery but not the `Custom`/`App` split, which is the higher-value
half.

**Minimal proposed API.**
```rust
// cyrup-agent/src/event.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppRole { BashExecution, BranchSummary, CompactionSummary }
impl AppRole {
    pub const ALL: [AppRole; 3] = [/* … */];
    pub const fn as_str(self) -> &'static str { /* "bashExecution" | … */ }
    pub fn parse(s: &str) -> Option<Self>;        // replaces APP_MESSAGE_ROLES.contains
}
pub enum AgentMessage { /* … */ App { role: AppRole, payload: serde_json::Map<String, Value> } }
```
Serde shape unchanged (B1): serialize still writes `payload` verbatim; deserialize routes the
`role` string through `AppRole::parse`. `bash.rs:221` constructs `App { role:
AppRole::BashExecution, payload }` directly; `app_role_payload` is deleted and the `Custom` arm
of `coding_agent_convert_to_llm` loses its overload branch; `raw_role_tag` returns `AppRole`.

**Guarantee gained.**
An `App` with an unknown role is unrepresentable. The live/resumed bash split disappears — one
variant, one path through `convert_to_llm`, matched exhaustively.

**Guarantee not gained.**
The payload inside `App` stays an opaque map by design — `cyrup-agent` must not know the
coding-agent message types (`event.rs:56-70`). Extension `Custom` kinds remain open strings.

**Migration and compatibility cost.**
`cyrup-agent/src/event.rs` (variant, const, serde impl). `cyrup-session-svc`: `hooks.rs:74-123`
(delete `app_role_payload`, simplify the arm), `session/bash.rs:221-222`, `event.rs:435-436,455`.
TUI, subagents, `cyrup-it`: **untouched** — the wire is byte-identical. `prompt_runtime.rs:1002-1007`
matches `Custom.kind` against its own allowlist and is unaffected.

**Benefit versus ceremony.** One three-variant enum deletes a const, a helper function, a string
compare and a documented overload. Earns it.

**Confidence.** High.

**Resolved open question (bash persistence).** How is a live bash message persisted, and does an
`App`-shaped persist arm need adding? **No.** `append_bash_message` (`bash.rs:279-288`) pushes
via `set_messages` (no `MessageEnd`, so `SvcSubscriber`'s persist arm at `subscriber.rs:172-184`
is never involved) and persists **directly** at `:287` via
`manager.append_custom_message("bashExecution", payload, true, None)`. That call passes the role
string and payload without inspecting the `AgentMessage` variant, so it is byte-identical under
F2, and the session file already reloads that `customType` as the role (`hooks.rs:62-63`). The
store has no `App`-shaped append and does not need one.

**Tendrils.**

| Crate | File:lines | Change | Mechanical / Semantic |
|---|---|---|---|
| cyrup-agent | `event.rs:71-76,85,179-186` | `role: AppRole`; `parse` in the gate | Semantic |
| cyrup-session-svc | `hooks.rs:74-123` | delete `app_role_payload`; simplify `Custom` arm | Semantic |
| cyrup-session-svc | `session/bash.rs:221-222` | construct `App` directly | Mechanical |
| cyrup-session-svc | `event.rs:435-436,455` | `raw_role_tag -> AppRole` | Mechanical |
| cyrup-sdk | `lib.rs` | re-export `AgentMessage` by name (incidental; `handle.rs:425` returns it) | Mechanical |

**Boundary.** Ends at the serde impl (B1) and the session bridge (B4). `AppRole` never crosses
into `cyrup-session`, whose own `AgentMessage` keeps `custom_type: String` — the bridge at
`session-svc/src/event.rs:424` is the one legitimate conversion site.

#### [P1] F3 — `TerminateHint`: one tri-state for `terminate`

**Location.**
`cyrup-core/src/tool.rs` — `ToolResult.terminate: bool` (`:42`), `ToolUpdate.terminate:
Option<bool>` (`:56`). `cyrup-agent/src/hooks.rs` — `BeforeOutcome::Block.terminate: bool`
(`:62`), `AfterToolCall.terminate: Option<bool>` (`:84`), `AfterOverride.terminate: Option<bool>`
(`:109`). `cyrup-agent/src/agent/run/tools/mod.rs` — `Finalized.terminate: Option<bool>` (`:35`).
`tools/finalize.rs` — the lossy conversion (`:51`) and its `[CYRUP-DELTA]` (`:43-45`).
`tools/exec.rs:244-245` — the batch fold. Cross-crate: `cyrup-ext/src/contract.rs:196`
`Reduced::Blocked.terminate: bool`.

**Current representation.** Four encodings of one fact. `finalize.rs:51` reads
`if r.terminate { Some(true) } else { None }`, and `:43-45` says in so many words: "a tool that
wants pi's explicit `terminate: false` cannot express it".

**Invariant or legal sequence.** The hint is three-valued: unspecified (pi `undefined`, key
absent on the wire), terminate, or explicit continue (pi `false`, key present). The batch
terminates iff every finalized result says terminate (`exec.rs:237-245`).

**Concrete failure mode.** A tool returning `terminate: false` and one returning
`..Default::default()` produce identical wire — pi's explicit `false` is unrepresentable — yet
`after_tool_call`'s `Some(false)` override *is* representable, so the two paths disagree on what
"false" means. A reader of `Finalized.terminate == None` cannot tell "tool said nothing" from
"tool said false".

**Recommended pattern.** Explicit domain enum in `cyrup-core`, replacing all four encodings.

**Why this pattern fits.** Three named states, inspected at runtime by the fold — an enum, not a
newtype over `bool`. `Option<bool>` is the *shape* of the fact but not its *meaning*; the enum
names each state and makes `.wire()` the single place presence/absence is decided.

**Minimal proposed API.**
```rust
// cyrup-core/src/tool.rs
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminateHint {
    /// pi `undefined` — key absent on the wire; does not contribute to the batch fold.
    #[default] Unspecified,
    Terminate,
    /// pi explicit `false` — key PRESENT as `false`.
    Continue,
}
impl TerminateHint {
    pub const fn requested(self) -> bool { matches!(self, Self::Terminate) }
    pub const fn wire(self) -> Option<bool> {
        match self { Self::Unspecified => None, Self::Terminate => Some(true), Self::Continue => Some(false) }
    }
}
pub struct ToolResult { /* … */ pub terminate: TerminateHint }
pub struct ToolUpdate { /* … */ pub terminate: TerminateHint }
```
`BeforeOutcome::Block.terminate`, `AfterToolCall.terminate`, `Finalized.terminate` →
`TerminateHint`. `AfterOverride.terminate` → `Option<TerminateHint>` (`None` = hook has no
opinion; `Some(Unspecified)` = hook clears it — the distinction the current `Option<bool>`
collapses). `result_value_of`/`update_value` take `TerminateHint` and call `.wire()`. The fold
becomes `.all(|f| f.terminate.requested())`. `contract.rs:196` and `HookOutcome::Block` (B6)
become `TerminateHint` together.

**Guarantee gained.** The wire presence/absence of `terminate` derives from a three-valued type,
not a lossy `bool`. The fold reads one predicate. `false` is expressible.

**Guarantee not gained.** The fold rule itself (`every`, `len > 0`) stays a runtime rule with its
existing tests. Whether a given tool *should* say `Continue` is the tool's business.

**Migration and compatibility cost.** `cyrup-core/src/tool.rs` (two fields). `cyrup-agent`:
`hooks.rs:62,84,109`, `tools/mod.rs:35`, `finalize.rs:37-51`, `exec.rs:244-245`, `message.rs`
(`result_value_of`). `cyrup-tools`: every `ToolResult{..}` literal — `..Default::default()`
compiles unchanged; any `terminate: true` → `TerminateHint::Terminate`. `cyrup-ext`:
`contract.rs:196` + `HookOutcome::Block` + the guest result conversion; `EventPatch::ToolResult`
gains `terminate: Option<TerminateHint>` so `apply_patch` and `AfterOverride` agree.
`cyrup-ext-subagents`: any `ToolResult` literals. Tests: `tool_result_model.rs` (11) assert key
presence/absence and become the `.wire()` parser tests unchanged.

**Benefit versus ceremony.** One three-variant enum with two `const fn`s replaces four field
types and one lossy conversion the code already apologises for. Leaf-most change in the plan.
Earns it.

**Confidence.** High — the `[CYRUP-DELTA]` is the finding, written by the previous author.

**Tendrils.**

| Crate | File:lines | Change | Mechanical / Semantic |
|---|---|---|---|
| cyrup-core | `tool.rs:42,56` | `bool`/`Option<bool>` → `TerminateHint` | Semantic |
| cyrup-agent | `hooks.rs:62,84,109` · `tools/mod.rs:35` · `finalize.rs:37-51` · `exec.rs:244-245` · `message.rs` | field types; `.wire()`; `.requested()` | Mechanical |
| cyrup-tools | every `ToolResult {..}` literal | `true` → `Terminate`; defaults unchanged | Mechanical |
| cyrup-ext | `contract.rs:196`, `HookOutcome::Block`, guest result conversion, `EventPatch::ToolResult` | `TerminateHint`; add `terminate` to the patch | Semantic (B6) |
| cyrup-ext-subagents | `ToolResult` literals, if any | as cyrup-tools | Mechanical |

**Boundary.** Ends at the WIT/JSON guest boundary in `cyrup-ext`, where the guest's result is
converted into `ToolResult` — that conversion is the one place a raw `Option<bool>` from the wire
becomes a `TerminateHint`. `result_value_of`'s output (the `terminate` JSON key) is byte-identical.

#### [P2] F4 — `stream_assistant` as a functional core: the `AssistantStream` accumulator

**Location.**
`cyrup-agent/src/agent/run/stream.rs` — `stream_assistant` (`:29-252`); the provider seam
`self.stream_fn.stream(&model, &ctx, &opts)` (`:141`); `let mut started = false` (`:143`);
the exactly-once guards `if !started` (`:158`, abort path; `:240`, EOF path) and the set
`started = true` (`:195`, first-event path); the three synthesised terminals — abort
(`:169-176`, `"Request was aborted"`), `Done`/`Error` (`:205-209`), EOF (`:237`, `"stream ended
without a terminal event"`).

**Current representation.**
A `select!` loop that mixes cancellation, `StreamEvent` consumption, `partial` refresh, emission
of `MessageStart`/`MessageUpdate`/`MessageEnd`, a mutable `started` flag, and three synthesised
terminal messages. The invariant "`MessageStart` exactly once, before `MessageEnd`, on every
path" lives in that flag and the discipline of checking it at each exit.

**Invariant or legal sequence.**
Every assistant turn emits `MessageStart` exactly once and `MessageEnd` exactly once, in that
order, regardless of which of the four exits (first-event, abort, terminal, EOF) the stream takes
— and the provider's post-terminal strays are ignored (pi returns on the terminal, `:196-199`).

**Concrete failure mode.**
A fourth exit path is added — a per-turn deadline, or treating `StopReason::Deferred` as settled
— and omits the `if !started` dance. Result: `MessageEnd` with no `MessageStart`.
`SvcSubscriber`'s `streaming_message` state and the TUI's turn interleaving
(`cyrup-tui/src/tests/turn_interleaving.rs`, which exists) desync. Nothing in the types prevents
it; only a parity test that happens to exercise the new path would notice.

**Recommended pattern.**
Functional Core / Imperative Shell, with a two-phase *private* enum. Explicitly **not** a generic
typestate `AssistantStream<Unstarted>`/`<Started>`.

**Why this pattern fits.**
The phase is decided by the provider's event stream at runtime, so a generic typestate would
force the shell to hold a boxed enum anyway; a private `Phase` enum plus a *consuming* `settle`
gives the same exactly-once guarantee without the generics. What actually matters is moving the
*decision* ("does the shell still owe a `MessageStart`?") into one pure place and out of three
call sites — that is FC/IS, not typestate.

**Minimal proposed API.**
```rust
// cyrup-agent/src/agent/run/assistant_stream.rs — pure: no tokio, no emit, no hooks
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
/// What the shell must emit to close the message. `start` is `Some` iff no `Start` was ever
/// yielded — exactly-once is decided HERE, once, not at three call sites.
pub(super) struct Settled { pub start: Option<Arc<AssistantMessage>>, pub end: AssistantMessage }

impl AssistantStream {
    pub(super) fn new(model: &ModelRef) -> Self;                       // seeds `empty_assistant`
    pub(super) fn on_event(&mut self, ev: StreamEvent) -> Step;
    pub(super) fn settle(self, terminal: AssistantMessage) -> Settled;  // consumes
    pub(super) fn settle_aborted(self) -> Settled;    // stamps Aborted + "Request was aborted"
    pub(super) fn settle_eof(self, model: &ModelRef) -> Settled;  // "stream ended without a terminal event"
}
```
The shell keeps: hook calls, `StreamOptions`/`Context` assembly, the `select!`, and **one**
emission tail: `if let Some(p) = settled.start { emit(MessageStart(p)) } emit(MessageEnd(settled.end))`.
`on_event`'s post-terminal handling makes the "return on the terminal" rule a property of the
accumulator.

**Guarantee gained.** `Settled` can only be produced by consuming the stream, and it carries the
start decision — three emission sites become one. Every `StreamEvent` sequence becomes a pure
unit test with no faux provider and no runtime.

**Guarantee not gained.** The shell can still forget to emit what `Settled` says (one site,
reviewable). Provider misbehaviour — a `Done` with no prior content, two terminals — is still a
runtime matter handled inside `on_event`.

**Migration and compatibility cost.** One new private module; `stream.rs` shrinks to the shell.
No signature outside `agent/run/` changes; no consumer is touched. Tests: the abort/EOF/
post-terminal cases in `model_boundary.rs` (18) become `AssistantStream::{on_event, settle_*}`
sequence tests; the event-order parity tests stay as the shell's gate.

**Benefit versus ceremony.** Two small structs and one enum, private, in exchange for removing a
class of bug (missed `MessageStart`) from every future exit path and making the accumulator
testable without a runtime. Earns it; the ceremony is entirely internal.

**Confidence.** High on the failure mode and the shape; medium on the exact `Step` variant set,
which may need a `Deferred` arm once that stop reason is settled elsewhere.

**Tendrils.**

| Crate | File:lines | Change | Mechanical / Semantic |
|---|---|---|---|
| cyrup-agent | new `agent/run/assistant_stream.rs` | the accumulator | New |
| cyrup-agent | `stream.rs:143-252` | shell keeps `select!` + one emission tail | Semantic |
| — | — | no consumer changes | — |

**Boundary.** Ends inside `agent/run/`. The provider seam (B3) and the emitted `AgentEvent`
sequence (B2) are untouched — the parity tests are the proof.

#### [P2] F5 — Modelless agent as a state, not a sentinel

**Location.**
`cyrup-agent/src/state.rs` — `StateInner.model: ModelRef` (`:90`), `AgentStateSnapshot.model:
ModelRef` (`:138`). `cyrup-session-svc/src/builder.rs:1587-1591` (SEAM-075): seeds
`ModelRef { provider: "", api: None, model: "" }` for a credential-less session and calls it
"unreachable while the session is modelless" because `prepare_and_assemble` returns
`NoModelSelected` first. `cyrup-session-svc/src/error.rs:64` already has
`SessionServiceError::NoModelSelected`; `cyrup-agent/src/error.rs` has no such variant.

**Current representation.** A sentinel value whose only guard is a comment. It is readable
through `Agent::snapshot().model` and is what `emit_run_failure` (`run/mod.rs:208`) would stamp
onto an errored assistant message.

**Invariant or legal sequence.** A run cannot start without a model. Pi's agent holds `Model |
undefined` (agent-session.ts:866-868) — "no model" is a *state*, not a magic address.

**Concrete failure mode.** Any path into `start_run` that bypasses the session preflight — a
`loop_fn` embedder, a `continue_run` from an extension control op, a test — streams against
`""/""` and produces an assistant message with `provider: ""` that lands in the transcript.

**Recommended pattern.** `Option` at the boundary (an invalid value made unrepresentable) plus a
new `AgentError::NoModelSelected` variant. The `Option` ends at the run boundary: `RunCtx.model`
stays `ModelRef`.

**Why this pattern fits.** Two states, one of them "absent" — `Option` *is* the enum. A newtype
over `ModelRef` that forbids empty strings would fix the sentinel but not model the state; the
session already reasons about "modelless" as a state (`thinking.rs` clamps to `Off` for it).

**Minimal proposed API.**
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
`start_run` returns `Err(AgentError::NoModelSelected)` when `None`, checked under the same lock
as the snapshot and **before** the latch is claimed. `TurnUpdate.model` unchanged.
`loop_fn::AgentLoopConfig.model` stays `ModelRef` — an embedder always has one.

**Guarantee gained.** No assistant message can be produced under an empty provider/model id.
"No model" is an `AgentError` a caller can match, not a string compare.

**Guarantee not gained.** The session's richer `NoModelSelected` preflight stays — it fires
before compaction and auth, with pi's verbatim guidance text. A `ModelRef` with `api: None` is
still valid (`empty_assistant` falls back to `UNRESOLVED_API`) — see §5.

**Migration and compatibility cost.** `cyrup-agent`: `state.rs:90,138`, `agent/builder.rs`,
`agent/facade.rs`, `agent/lifecycle.rs` (the check), `error.rs`. `cyrup-session-svc`:
`builder.rs:1587-1602` (sentinel + comment deleted; `.model(m)` applied iff `Some`),
`session/tools.rs` (`next_turn_model_baseline` → `Option`), `hooks.rs:320` (set `update.model`
iff `Some`), `session/model.rs` (passes `Some`), `session/thinking.rs` (already clamps modelless).
`accessors.rs` and the SDK `handle.rs` `model()` already return `Option`.

**Benefit versus ceremony.** One `Option` and one error variant delete a sentinel and its
"unreachable" comment. Earns it; the ceremony is the `Some(..)` at four session sites.

**Confidence.** High.

**Tendrils.**

| Crate | File:lines | Change | Mechanical / Semantic |
|---|---|---|---|
| cyrup-agent | `state.rs:90,138` · `builder.rs` · `facade.rs` · `lifecycle.rs` · `error.rs` | `Option<ModelRef>`; `NoModelSelected` | Semantic |
| cyrup-session-svc | `builder.rs:1587-1602` | delete sentinel; `.model(m)` iff `Some` | Semantic |
| cyrup-session-svc | `session/tools.rs`, `hooks.rs:320`, `session/model.rs` | `Option` plumbing | Mechanical |
| cyrup-session-svc / cyrup-sdk | `accessors.rs`, `handle.rs` `model()` | already `Option` | None |

**Boundary.** Ends at `RunCtx::new` — the `Option` is resolved to a `ModelRef` or an error
before the run exists. `loop_fn`'s config (B3-adjacent, zero consumers) keeps `ModelRef`.

### Secondary findings

Same twelve sub-fields; carried separately so the five above stay the review's spine.

#### [P2] F6 — Tool-call pipeline: constructor discipline + a pure finalize fold

**Location.** `cyrup-agent/src/agent/run/tools/mod.rs` — `Finalized { source_index, .. }`
(`:26-35`), `Prep::{Immediate(Box<Finalized>), Ready{tool,args}}` (`:39-43`),
`fail_truncated_tool_calls` (`:83`). `tools/preflight.rs` — `immediate_error` (`:128`) writes
`source_index: 0`. `tools/exec.rs` — callers patch it afterwards (`:70`, `:278`); the local
`struct Deferred` (`:51`) is the prepared-call record but is private to `execute_parallel`.
`tools/finalize.rs` — `finalize` (`:18`) is async (awaits `after_tool_call` at `:84`) and performs
the replace-not-merge fold inline.

**Current representation.** A finalized result is built with a placeholder index that two of
three producers remember to overwrite. The prepared-call record exists but only inside one
runtime. The pure fold (which fields an override replaces) is interleaved with the hook await.

**Invariant or legal sequence.** Every `Finalized` carries the index of the call it answers.
The override fold is a total function of `(call, index, args, outcome, override)`.

**Concrete failure mode.** `fail_truncated_tool_calls` (`:83`) builds `Finalized`s and never
patches `source_index`; today that path happens to produce results in order, so the zeros are
harmless — until a caller reads `source_index` for anything but ordering.

**Recommended pattern.** Private constructor requiring the index (constructor discipline) +
FC/IS split of `finalize` into an async hook call and a pure fold. **Typestate across the
parallel runtime is rejected**: completion order is genuinely dynamic (`JoinSet` + channel).

**Why this pattern fits.** The index is a required datum, not a state — a constructor argument
closes it. The fold is a domain decision tangled with an await — FC/IS is the direct fix.

**Minimal proposed API.**
```rust
pub(super) struct PreparedCall { source_index: usize, tool: Arc<dyn Tool>, args: Value, call_id: ToolCallId, tool_name: String }
enum Prep { Immediate(Box<Finalized>), Ready(PreparedCall) }
impl RunCtx { fn immediate_error(&self, call: &ToolCall, source_index: usize, msg: String, terminate: TerminateHint) -> Finalized; }
impl Finalized { fn new(source_index: usize, /* … */) -> Self; }   // the only constructor
// finalize.rs
async fn after_hook(&self, …) -> Option<AfterOverride>;                                  // shell
fn fold_tool_outcome(call: &ToolCall, source_index: usize, args: &Value,
                     outcome: Result<ToolResult, ToolError>, over: Option<AfterOverride>) -> Finalized;  // pure
```
Both runtimes call the pair.

**Guarantee gained.** A `Finalized` cannot exist without its index. The replace-not-merge table
is one testable function.

**Guarantee not gained.** That the index is *correct* — the producer still supplies it.

**Migration and compatibility cost.** `tools/{mod,preflight,exec,finalize}.rs` only. Zero
tendrils. Tests: the `source_index` patching assertions become redundant only after the
constructor lands.

**Benefit versus ceremony.** Small; internal; unlocked by F3. Earns it as part of the same
touch of those files.

**Confidence.** High.

**Tendrils.** None outside `cyrup-agent/src/agent/run/tools/`.

**Boundary.** Entirely internal to the tool pipeline.

#### [P2] F7 — The `Hooks` failure-mode map in the signatures

**Location.** `cyrup-agent/src/hooks.rs` — the trait (`:212-279`): `convert_to_llm` (`:215`),
`transform_context` (`:220`), `before_tool_call` (`:229`), `after_tool_call` (`:238`),
`prepare_next_turn` (`:258`), `should_stop_after_turn` (`:272`). Abort sites:
`stream.rs:44` (transform → `RunFailure`), `turn.rs:120` (prepare), `turn.rs:181`
(should_stop). Degrade sites: `preflight.rs:69-77` (`before_tool_call` `Err` → immediate error
result), `finalize.rs:84` (`after_tool_call`). `cyrup-agent/src/error.rs:6-11` —
`HookError::{Failed(String), Serde(#[from] serde_json::Error)}`.

**Current representation.** All six return `Result<_, HookError>`. Four abort the run on `Err`;
two produce a per-call error result and continue. Only comments say which.

**Invariant or legal sequence.** Per the review's own rule: an expected per-item outcome is a
named enum variant; a failure that aborts the operation is `Result::Err`.

**Concrete failure mode.** An implementor of `before_tool_call` uses `?` expecting to abort the
run and instead produces a tool-error result the model reads as the tool's own output; or an
implementor of `prepare_next_turn` returns `Err` expecting a per-turn degrade and aborts the
whole run.

**Recommended pattern.** Domain outcome enums for the two per-call hooks; the four run-aborting
hooks keep `Result`.

**Why this pattern fits.** The two hooks' `Err` is not a failure of the operation — it is one of
the expected outcomes of a tool call, on par with `Block`. Naming it as a variant makes the
signature say what the comment says.

**Minimal proposed API.**
```rust
pub enum BeforeOutcome { Proceed, Block { reason: Option<String>, terminate: TerminateHint }, Failed(HookError) }
pub enum AfterOutcome  { Keep, Override(AfterOverride), Failed(HookError) }
async fn before_tool_call(&self, ctx: BeforeToolCall<'_>, cancel: CancelToken) -> BeforeOutcome;
async fn after_tool_call(&self, ctx: AfterToolCall<'_>, cancel: CancelToken) -> AfterOutcome;
// transform_context, convert_to_llm, prepare_next_turn, should_stop_after_turn: unchanged, Result<_, HookError>
```

**Resolved open question (`Failed` payload).** `Failed(String)` or `Failed(HookError)`?
**`Failed(HookError)`.** Neither in-tree impl of the two per-call hooks uses `?` on a
`serde_json` error today (`session-svc/src/hooks.rs`, `cyrup-ext/src/hooks.rs` — grep found
none), so nothing is lost either way; but `HookError` already carries `Serde(#[from])`, so
`Failed(HookError)` keeps `?` available inside a helper via `.map_err`, costs nothing, and the
`hook_failure_text.rs` tests only need `Display`, which `HookError` has. Zero information loss
beats a `String`.

**Guarantee gained.** The trait's signature states which hooks can abort a run. An implementor
cannot accidentally abort from a per-call hook.

**Guarantee not gained.** Correctness of any given hook; the abort semantics of the four
`Result` hooks are unchanged and still documented, not typed.

**Migration and compatibility cost.** Broadest trait break in the plan: `hooks.rs:229-257`,
`preflight.rs:69-77`, `finalize.rs:84`; `session-svc/src/hooks.rs` (two impls — the `?` on the
inner delegate becomes a match); `cyrup-ext/src/hooks.rs` (two impls — already never `Err`);
`cyrup-ext/src/contract.rs:196` (`Reduced::Blocked`, B6, with F3); `cyrup-agent/src/tests/hook_failure_text.rs`
(four tests construct `Failed`). **Schedule last** — smallest guarantee, widest surface.

**Benefit versus ceremony.** Two enums and a trait break for a documentation-grade guarantee.
Earns it only because the trait is being touched by F3 anyway and the two impls are small.

**Confidence.** Medium — the value is real but modest.

**Tendrils.**

| Crate | File:lines | Change | Mechanical / Semantic |
|---|---|---|---|
| cyrup-agent | `hooks.rs:229-257`, `preflight.rs:69-77`, `finalize.rs:84` | outcome enums; match instead of `?` | Semantic |
| cyrup-session-svc | `hooks.rs` `before_tool_call`/`after_tool_call` | match on the delegate | Mechanical |
| cyrup-ext | `hooks.rs` two impls; `contract.rs:196` | return enum; B6 with F3 | Mechanical |
| cyrup-agent tests | `hook_failure_text.rs` (4) | construct `Failed(..)` | Mechanical |

**Boundary.** Ends at the `Hooks` trait — both impls are in-tree. `cyrup-ext`'s guest-facing
`HookOutcome` (`contract.rs`) is the WASM boundary and keeps its own shape apart from the shared
`TerminateHint`.

#### [P3] F8 — One run-in-flight fact, two flags

**Location.** `cyrup-agent/src/state.rs:94` `StateInner.is_streaming` (set `lifecycle.rs:261`,
cleared `:84` in `SettlementGuard::drop` and `:120` in `reset`) vs `agent/mod.rs:70-71`
`running_tx: watch::Sender<bool>` (claimed `lifecycle.rs:235`, released `:98`). The doc at
`lifecycle.rs:58-59` already argues the latch must be `running_tx` itself, "deliberately not a
second bool beside it". Session reads: `run.rs:75,325` use `is_run_active()`; `inject.rs:41,76,127`
and `control.rs:236` use `is_streaming()`.

**Current representation.** Two fields, written at the same two sites, meaning the same thing;
consumers pick one per decision.

**Invariant or legal sequence.** One run-in-flight fact.

**Concrete failure mode.** Not a compiler-visible bug — the two are written together. The risk
is the session's *predicate choice*: `inject.rs` routes queue-vs-run on `is_streaming()`, which
is the agent's flag, while `prompt` gates on `is_run_active()`, which includes the session's
`driver_tx`. AGENT-030 (`run.rs:72-77`) documents that these differ during the post-`agent_end`
gap.

**Recommended pattern.** Delete the field; `snapshot().is_streaming` reads `*running_rx.borrow()`.
Then audit the four `is_streaming()` sites against AGENT-030.

**Why this pattern fits.** Not a new type — removing a duplicate. P3 because the write sites are
already co-located; the refactor *exposes* the predicate question rather than answering it.

**Minimal proposed API.** `AgentStateSnapshot.is_streaming` unchanged in shape; sourced from the
latch.

**Guarantee gained.** The two cannot disagree.

**Guarantee not gained.** Whether `inject.rs`/`control.rs` should read the session-level
predicate instead — that is the audit.

**Migration and compatibility cost.** `state.rs`, `lifecycle.rs:84,120,261`, `facade.rs`; a
four-site audit in `session-svc`. No signature change.

**Benefit versus ceremony.** Net negative code. Earns it trivially; the audit is the real work.

**Confidence.** High on the duplication; the audit outcome is open.

**Tendrils.** `session-svc/src/session/{inject.rs:41,76,127, control.rs:236}` — audit only.

**Boundary.** Internal; `AgentStateSnapshot`'s field survives with the same type.

---

### Resolved open questions (collected)

| # | Question (from the task) | Answer | Recorded under |
|---|---|---|---|
| 1 | Should `edit_transcript` refuse during the AGENT-030 post-run gap? | No — refuse on the agent latch only; `is_run_active()` is `!is_idle()` at `session/mod.rs:499`, a session predicate the agent cannot see; the session keeps gating its callers | F1 |
| 2 | How is a live bash message persisted; is an `App` persist arm needed? | Directly at `bash.rs:287` via `append_custom_message("bashExecution", …)`, bypassing the subscriber; byte-identical under F2; no `App` arm needed | F2 |
| 3 | `Failed(String)` or `Failed(HookError)`? | `Failed(HookError)` — zero information loss; neither in-tree impl uses `?` on serde today, so nothing changes in practice | F7 |
| 4 | Is the api-key static branch's lack of `.filter(!empty)` a divergence from pi? | **No.** `stream.rs:64-69` filters the resolver result then falls back to the static key, exactly pi's `(getApiKey?…) \|\| config.apiKey` — JS `\|\|` returns the last operand even when falsy, so pi also sends an empty static key. Cyrup is faithful on both branches. A `Some("")` static key is a builder-validation concern, not a loop one. No change | §5 |

---

## 4. Highest-Value Refactor Sketch — F1

**Before** (the shape at HEAD; three files, one precondition, no proof):

```rust
// loop_fn.rs:197-200  (and again at :279-282)
if context.messages.is_empty() { return Err(AgentError::NoMessages(ContinueSurface::Loop)); }
if matches!(context.messages.last(), Some(AgentMessage::Assistant(_))) {
    return Err(AgentError::ContinueFromAssistant);
}
// … build_run_ctx(…, /* skip_initial_steering_poll */ false)

// lifecycle.rs:168-219  continue_run
let snapshot = lock(&self.state).messages.clone();              // lock #1
if snapshot.is_empty() { return Err(AgentError::NoMessages(ContinueSurface::Agent)); }
/* drain steering → start_run(EntryStart::Prompt(steering), true) */
if matches!(snapshot.last(), Some(AgentMessage::Assistant(_))) {
    return Err(AgentError::ContinueFromAssistant);
}
/* drain follow-up → start_run(EntryStart::Prompt(follow), false) */
self.start_run(EntryStart::Continue, false).await          // ← proof discarded

// lifecycle.rs:222-299  start_run
let (…, messages, …) = { let st = lock(&self.state); (…, st.messages.clone(), …) };  // lock #2 — a DIFFERENT snapshot
RunCtx::new(/* 16 args */, messages, cancel, skip_initial_steering_poll)

// session-svc — three places, no lock, two awaits each
let mut msgs = self.agent.snapshot().await.messages;
if matches!(msgs.last(), Some(AgentMessage::Assistant(_))) { msgs.pop(); self.agent.set_messages(msgs).await; }
// run.rs:170
Err(_) => break,
```

**After** (pseudocode is marked; everything else is intended to compile as shown):

```rust
// ── cyrup-agent/src/agent/run/mod.rs ──────────────────────────────────────────────
pub(crate) enum PromptSource { Fresh, SteeringDrain, FollowUpDrain }

pub(crate) enum RunEntry {
    Prompt { messages: Vec<AgentMessage>, source: PromptSource },
    Continue(ResumePoint),
}
impl RunEntry {
    pub(crate) fn skip_initial_steering_poll(&self) -> bool {
        matches!(self, RunEntry::Prompt { source: PromptSource::SteeringDrain, .. })
    }
}

/// Proof that `messages` may be resumed. Zero-sized; private field; ONE constructor.
pub(crate) struct ResumePoint(());
impl ResumePoint {
    pub(crate) fn check(messages: &[AgentMessage], surface: ContinueSurface) -> Result<Self, AgentError> {
        if messages.is_empty() { return Err(AgentError::NoMessages(surface)); }
        if matches!(messages.last(), Some(AgentMessage::Assistant(_))) {
            return Err(AgentError::ContinueFromAssistant);
        }
        Ok(ResumePoint(()))
    }
}

/// Handles shared for the run's lifetime (formerly 9 of the 17 `new` args).
pub(crate) struct RunShared { state, subscribers, steering, follow_up, hooks, stream_fn, key_resolver, tool_execution, session_id }
/// The run-start `.slice()` baseline (formerly the other 8).
pub(crate) struct RunBaseline { system_prompt, model: ModelRef, thinking_level, gen_config, tools, messages: Vec<AgentMessage> }

impl RunCtx {
    pub(crate) fn new(shared: RunShared, baseline: RunBaseline, entry_cancel: RunCancel) -> Self;   // no bool
    pub(crate) async fn run(&mut self, entry: RunEntry) -> Vec<AgentMessage> {
        self.skip_initial_steering_poll = entry.skip_initial_steering_poll();   // derived, once
        /* … as today … */
    }
}

// ── cyrup-agent/src/agent/lifecycle.rs ────────────────────────────────────────────
impl Agent {
    pub async fn continue_run(&self) -> Result<RunHandle, AgentError> {
        // ONE lock: the transcript we validate is the transcript we run.
        let baseline = self.snapshot_baseline()?;                                // lock #1 — and only
        if baseline.messages.is_empty() { return Err(AgentError::NoMessages(ContinueSurface::Agent)); }
        if let Some(steering) = self.drain_steering() {
            return self.start_run(RunEntry::Prompt { messages: steering, source: PromptSource::SteeringDrain }, baseline).await;
        }
        let proof = ResumePoint::check(&baseline.messages, ContinueSurface::Agent);
        if let Some(follow) = self.drain_follow_up() {
            return self.start_run(RunEntry::Prompt { messages: follow, source: PromptSource::FollowUpDrain }, baseline).await;
        }
        self.start_run(RunEntry::Continue(proof?), baseline).await
    }

    /// `baseline` was taken by the caller under the state lock; `start_run` no longer re-snapshots.
    async fn start_run(&self, entry: RunEntry, baseline: RunBaseline) -> Result<RunHandle, AgentError> {
        /* claim latch (unchanged) … */
        let mut rc = RunCtx::new(self.shared(), baseline, cancel);
        /* spawn rc.run(entry) (unchanged) … */
    }
}

// ── cyrup-agent/src/agent/facade.rs ───────────────────────────────────────────────
impl Agent {
    pub fn edit_transcript<R>(&self, f: impl FnOnce(&mut Vec<AgentMessage>) -> R) -> Result<R, AgentError> {
        if *self.running_rx.borrow() { return Err(AgentError::RunActive(BusyEntry::Edit)); }
        let mut st = lock(&self.state);
        Ok(f(&mut st.messages))
    }
    pub fn pop_trailing_assistant_if(&self, pred: impl FnOnce(&AssistantMessage) -> bool)
        -> Result<Option<Arc<AssistantMessage>>, AgentError>
    {
        self.edit_transcript(|m| match m.last() {
            Some(AgentMessage::Assistant(a)) if pred(a) => {
                let a = Arc::clone(a); m.pop(); Some(a)
            }
            _ => None,
        })
    }
}

// ── cyrup-session-svc: the three triplets become one call each ────────────────────
// retry.rs
let _ = self.agent.pop_trailing_assistant_if(|_| true);
// auto_compaction.rs
let _ = self.agent.pop_trailing_assistant_if(|a| matches!(a.stop_reason, StopReason::Error | StopReason::Length));
// bash.rs
let _ = self.agent.edit_transcript(|m| m.push(msg));
// run.rs — the session can no longer hand the agent an unresumable transcript, so:
Err(e) => return Err(SessionServiceError::Agent(e)),
```

What the sketch establishes: `RunEntry::Continue` requires a `ResumePoint`; `ResumePoint` has one
constructor; `continue_run` locks once and passes its own snapshot forward; the poll flag is a
function of the entry; and the session's edits are atomic under the same lock the reducer uses.
`BusyEntry::Edit` is a new arm of the existing enum so the error text can name the operation.

---

## 5. Deliberately Rejected Opportunities

Required, and load-bearing: each of these would have looked like a finding to a reviewer
optimising for type count.

**Typestate on `Agent` (`Agent<Idle>` / `Agent<Running>`).** `Agent` is `Arc`-shared across the
session, the TUI and extensions, and its state is externally driven — an abort arrives from the
UI, and `SettlementGuard` (`lifecycle.rs:66`) releases the latch from the run's own task. No
program point can hold a `Agent<Running>` by value while another task owns the transition. The
`watch<bool>` compare-and-set at `:235` plus `wait_for_idle` **is** the correct design for a
dynamically-driven latch, and `settlement_latch.rs`'s three multi-thread tests pin it. Typestate
here would be a lie the compiler could not check.

**`PartialAssistant` vs `AssistantMessage` split.** `StopReason::Pending` is only valid on a
partial, which invites two types. But `StreamEvent::*.partial: Arc<AssistantMessage>` is the
provider seam (B3), the TUI renders partials *as* assistant messages, and pi models it with one
type. The blast radius crosses two crates for a state that is genuinely dynamic (decided
event-by-event). Keep the `Pending` seed in `empty_assistant` and let F4's `Settled` be the guard
that the terminal message is not `Pending`.

**`WorkingTranscript` newtype for `RunCtx::messages` vs `StateInner::messages`.** Already
distinguished by element type — `Vec<Arc<AgentMessage>>` vs `Vec<AgentMessage>` after PERF-002 —
so a direct assignment between them does not compile today. A wrapper would add ceremony to a
distinction the compiler already enforces. Make it *deliberate* with the doc line already on
`RunCtx::messages` (`run/mod.rs:63-75`), not a type.

**`TurnOutcome` enum over `run_loop` (`turn.rs:12-208`).** The sequencing *is* the spec — pi's
`runLoop` line for line, with hooks awaited between decisions. An enum would restate the control
flow without removing a single check, and would invite parity drift in exactly the place the
thirteen `agent_loop.rs` tests pin most tightly. Rejected on the review's own rule: no typestate
or enum solely to reduce a function's length.

**`BlockReason` newtype for the JS-falsy empty-string rule (`preflight.rs`).** One consumer
site normalises `Some("")` to `None`; the producer is cross-crate (`cyrup-ext` guest
`block(some(""))`). Normalisation at the only reader is the correct placement — a newtype would
move it to a boundary the producer does not control.

**`ApiKey` newtype (`stream.rs:64-69`).** One site. The task flagged an asymmetry — the resolver
result is `.filter(!empty)`'d, the static `gen_config.api_key` is not. Resolved above (open
question 4): pi's `(getApiKey?…) || config.apiKey` also passes an empty static key through,
because `||` yields its last operand even when falsy. Cyrup is faithful on both branches; there
is nothing to fix and nothing to type.

**`RunFailure { source, message }` (`run/mod.rs:35`).** Diagnostic value only. The message text
is what reaches the wire as `error_message` and must stay the raw `e.to_string()`. A structured
source would be read by nothing.

**Collapsing `AgentContext` + `AgentLoopConfig` into a public `RunCtx` grouping.** `loop_fn` has
zero consumers, so it is free to reshape — but the split mirrors pi's `AgentContext` /
`AgentLoopConfig` and the SDK re-exports the module wholesale. Keep the public shapes; group
*internally* as F1's `RunShared` + `RunBaseline` (which is what actually fixes the 17-argument
constructor).

**`ModelRef.api: Option<ApiId>` → required.** Real — two construction sites disagree
(`session/model.rs` `api: None` vs `Some`) and `empty_assistant` papers over it with
`UNRESOLVED_API`. But it is a `cyrup-core` / `cyrup-provider` change outside this artery.
Recorded as a follow-up, not a finding here.

**Deriving serde on `AgentMessage`.** Never. `event.rs:93` records the duplicate-`role`-key bug
the hand-written impl exists to prevent (B1). F2 edits the hand-written impl; it does not
replace it.

---

## 6. Incremental Migration Plan

Each step is one `/split` task. Each step's agent-side change lands first, then its consumer
changes, inside the same task; the workspace must pass `cargo check --workspace` and
`cargo test -p cyrup-agent` at the end of every step. The order is dependency-driven: leaf-most
first, broadest trait break last.

1. **F3 `TerminateHint`.** `cyrup-core/src/tool.rs:42,56` → `cyrup-agent/src/{hooks.rs:62,84,109,
   agent/run/tools/mod.rs:35, finalize.rs:37-51, exec.rs:244-245, agent/message.rs}` →
   `cyrup-tools` `ToolResult` literals → `cyrup-ext/src/contract.rs:196` + `HookOutcome::Block` +
   guest result conversion + `EventPatch::ToolResult` → `cyrup-ext-subagents` literals.
   *Green:* `cargo check --workspace`. Leaf-most; unlocks F6 and F7.
2. **F2 `AppRole`.** `cyrup-agent/src/event.rs:71-76,85,179-186` → `cyrup-session-svc/src/{hooks.rs:74-123,
   session/bash.rs:221-222, event.rs:435-436,455}`; incidental: `cyrup-sdk/src/lib.rs` re-exports
   `AgentMessage` by name. *Green:* wire byte-identical; TUI/subagents/it untouched.
3. **F1 `RunEntry` + `ResumePoint` + `Agent::edit_transcript`.** `cyrup-agent/src/{agent/run/mod.rs:21-24,90-107,
   agent/lifecycle.rs:168-299, loop_fn.rs:197-200,279-282, agent/facade.rs, error.rs (BusyEntry::Edit)}`
   → `cyrup-session-svc/src/session/{retry.rs:141-147, auto_compaction.rs:395-410, bash.rs:279-282,
   run.rs:170}`. Group `RunCtx::new`'s 17 args into `RunShared` + `RunBaseline` in this step.
   Incidental: `QueueMode: FromStr` in `cyrup-agent/src/queue.rs` replacing
   `builder.rs:1866-1868 parse_queue_mode` (which maps anything but `"all"` to `OneAtATime`
   silently). *Green:* the `NoMessages`/`ContinueFromAssistant` tests pass against
   `ResumePoint::check`.
4. **F6 tool pipeline.** `cyrup-agent/src/agent/run/tools/{mod,preflight,exec,finalize}.rs` only.
   *Green:* zero consumers.
5. **F4 `AssistantStream`.** New `cyrup-agent/src/agent/run/assistant_stream.rs`; `stream.rs:143-252`
   shrinks to the shell. *Green:* event-order parity tests unchanged.
6. **F5 `Option<ModelRef>`.** `cyrup-agent/src/{state.rs:90,138, agent/builder.rs, agent/facade.rs,
   agent/lifecycle.rs, error.rs}` → `cyrup-session-svc/src/{builder.rs:1587-1602, session/tools.rs,
   hooks.rs:320, session/model.rs, session/thinking.rs}`. *Green:* `accessors.rs`/SDK `model()`
   already `Option`.
7. **F7 `Hooks` outcome enums.** `cyrup-agent/src/{hooks.rs:229-257, agent/run/tools/preflight.rs:69-77,
   finalize.rs:84}` → `cyrup-session-svc/src/hooks.rs` → `cyrup-ext/src/hooks.rs` (+ `contract.rs:196`,
   already `TerminateHint` from step 1) → `cyrup-agent/src/tests/hook_failure_text.rs`. Incidental:
   drop `ExtensionHost::subscriber`'s ignored `_cancel` param with its one caller
   (`session-svc/src/builder.rs`). *Green:* broadest trait break, deliberately last.
8. **F8 `is_streaming` collapse.** `cyrup-agent/src/{state.rs:94, agent/lifecycle.rs:84,120,261,
   agent/facade.rs}` → audit `cyrup-session-svc/src/session/{inject.rs:41,76,127, control.rs:236}`
   against AGENT-030 and either switch to `is_run_active()` or document why the narrower predicate
   is right. *Green:* no signature change.

No step forces a swap of Part G's order; F3-before-F7 is the one hard dependency (B6 changes
`Reduced::Blocked.terminate` once, in step 1).

---

## 7. Test Implications

**Become direct parser / pure-function tests.**
`tool_result_model.rs` (11 tests asserting the `terminate` key's presence/absence) → tests of
`TerminateHint::wire()`, unchanged in assertion. The `NoMessages` / `ContinueFromAssistant`
assertions in `agent_loop.rs`, `round2_parity.rs`, `area02_backlog.rs` → tests of
`ResumePoint::check`. The abort / EOF / post-terminal cases in `model_boundary.rs` (18) →
`AssistantStream::{on_event, settle, settle_aborted, settle_eof}` sequence tests with no faux
provider and no runtime.

**Remain necessary as behaviour tests.** Every event-sequence parity test — they pin B1/B2 and
pi's ordering, which no type here guarantees. `settlement_latch.rs` (3, multi-thread — the latch
is runtime by design, §5). `pending_containment.rs`. `hook_failure_text.rs` (the hook's own text
reaching the model; updated to construct `Failed(..)`, unchanged in intent). Session-svc
`round2.rs` / `round3.rs` / `mid_run_tool_anchoring.rs`. `cyrup-it` `embedder_seams.rs` (B3) and
`wasm_renderer_screen.rs` (B1 through WASM).

**Become redundant only once the type guarantee is complete — do not delete before.** The
`skip_initial_steering_poll` coupling tests, after F1 removes the `bool`. The `source_index`
patching assertions, after F6's constructor lands. Deleting either earlier removes the only
check on an invariant the type does not yet hold.

**Compile-fail tests.** Low value here — every guarantee is `pub(crate)` except `TerminateHint`
(no `From<bool>`) and `AppRole` (no `From<String>`). A `compile_fail` doc-test on each of those
two constructors is sufficient; a `trybuild` harness would be ceremony.

**Still shell-level.** Anything crossing B1–B5: the TUI's serde string projections
(`event_extract.rs:185,196-198`), the NDJSON child protocol (`ndjson.rs:26-29`), the session JSONL
round-trip, and the WASM `HostEvent` conversion. Each needs a two-sided byte-pin per boundary. The
sweep found **none** for the `Custom{kind,payload}` ↔ `Custom{custom_type,content}` bridge at
`session-svc/src/event.rs:424` — that is the one boundary test this review would add before F2
lands, independent of any type change.
