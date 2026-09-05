# ADR-0028 — `cyrup-acp` is enums and a functional core, not typestate

**Status** accepted (decided by default under the parity rule — overridable)
**Date** 2026-09-05
**Decides** the Rust type-design question for the unwritten `crates/cyrup-acp`, raised by area 15 (`docs/gap-analysis/15-cyrup-acp.md`)
**Blocks released** every area-15 unit whose `cyrup_mechanism` names a type this file defines — the `Turn` enum, the `ToolCallLedger`, `AbsCwd`/`SessionFile`/`AcpSessionId`, `AcpFailure::classify`, `SessionConfigKnob`, `TerminalAppender` and `DialogChoice`

## Numbering note

The next free ADR number is **not** 0012. `docs/gap-analysis/MCP-PORT-METHODOLOGY.md` claims
**0012 through 0027** for the MCP port and cites `ADR-0012` by name in four places, so this file
takes **0028**, the first number outside that block. `docs/adr/README.md`'s "the next is ADR-0012"
line predates that claim. Numbers are never reused and never renumbered.

## Context

This is an **opportunity review written before the crate exists**, not a refactor of shipped code.
Its subject is reconstructed from `svkozak/pi-acp` @ **v0.0.33** (`git -C tmp/pi-acp show
v0.0.33:<path>`) and from the cyrup types `cyrup-acp` will sit on. No code was modified to produce
it. It follows area 15's five surveys and their five adversary passes, and its invariant map is
built from the `invariants` each of those recorded.

Read it as an **opportunity review**: it recommends the smallest design change that captures a
meaningful invariant, and "no meaningful opportunity" is a conclusion it reaches deliberately in
§5, which is why that section is not optional. Every finding states both the guarantee gained
**and** the guarantee not gained, because the difference is where the runtime tests still have to
live.

---

## 1. Executive verdict

This crate needs **explicit domain enums and a functional core / imperative shell split first, boundary newtypes second, and typestate essentially not at all.** The reason is structural: an ACP agent is a JSON-RPC server driven by an editor that sends `initialize`, `session/new`, `session/prompt`, `session/cancel` and `session/set_config_option` in whatever order and interleaving it likes, and the ACP SDK's `Builder`/`ChainedHandler` (`tmp/agent-client-protocol-2.1.0/src/jsonrpc/handlers.rs`) registers every handler for the life of the connection — there is no ownership path along which a compiler could withhold a method. Every candidate "lifecycle" here is inspected dynamically, which is the decision rule's own signal for an enum.

The highest-value opportunity is **making the event-to-`SessionUpdate` translation a pure function over an explicit tool-call ledger, with the shell owning `ConnectionTo<Client>`**. In pi-acp that logic is `session.ts`'s `handlePiEvent` — a ~300-line switch that simultaneously decides, performs synchronous filesystem reads (`readFileSync` inside the event handler, in the `tool_execution_start` and `tool_execution_end` arms), mutates five parallel collections keyed by tool-call id (`currentToolCalls`, `fileSnapshots`, `fileMutationToolCallIds`, `bashToolCallIds`, `bashOutputSnapshots`), and sends notifications through a promise chain (`emit`/`lastEmit`). Those five collections *are* a sum type written as a product; collapsing them into one enum-valued map makes the two invariants that pi-acp enforces by hand — "the first emission for an id is `tool_call`, never `tool_call_update`" and "status never regresses to `pending`" — true by construction rather than by the `existingStatus ? … : …` ternaries repeated at four sites.

The second-highest is **`AcpFailure::classify(&SessionServiceError)` replacing `auth-required.ts`'s `maybeAuthRequiredError`**, which classifies errors by substring-matching a list containing `'permission denied'`, `'403'` and `'unauthorized'`. cyrup already distinguishes these typed: `SessionServiceError::{NoConfiguredAuth, AuthPreflightRefused, NoModelSelected}` (`crates/cyrup-session-svc/src/error.rs`). Porting the sniffer would make a routine `EACCES` from the bash tool present to the editor as an authentication failure.

**Typestate is not justified anywhere in this crate**, and I recommend against it in all four places it looks attractive (§5). The compiler-visible lifecycle of an ACP session ends at the handler boundary — the real lifecycle continues in a JSONL file on disk that a different `cyrup` process may also be writing.

The major tradeoff is that every recommendation here is a *module-private* guarantee, not a language-level one: the ACP schema types are `#[non_exhaustive]`, live in an external crate, and are deserialized by the SDK before any cyrup code runs, so **no cyrup newtype can have a validating `Deserialize` path on the request side**. Validation must happen as the first statement of each handler, and the invariant survives only because the validated type is what every downstream function accepts. I say this explicitly at each finding rather than pretending otherwise.

Missing domain information that prevents a fully confident conclusion, all labelled as open questions below: (a) what ACP semantics should be for a second `session/prompt` arriving mid-turn once cyrup's steer/follow-up queues replace pi-acp's `turnQueue` — cyrup emits one `AgentSettled` per *loop*, not per submission, so there is no per-prompt settle to respond to; (b) whether `cyrup-acp` gets a durable id→file map at all, or reads `cyrup_session::listing` as the single source of truth; (c) the wire method name TS SDK 0.26 binds `unstable_setSessionModel` to, carried over unresolved from the Architecture phase.

---

## 2. Invariant and state map

| Location | Domain fact or state | Current encoding (pi-acp) | Failure mode | Best representation |
|---|---|---|---|---|
| `agent.ts` `newSession`, `loadSession` | `params.cwd` is absolute | `if (!isAbsolute(params.cwd)) throw` at two call sites; **not** checked on the `restoreSession(sessionId)` path used by `prompt`, which takes `cwd` from the session-map file | a relative cwd from `~/.pi/pi-acp/session-map.json` reaches `PiRpcProcess.spawn`, and every `resolvePath(cwd, …)` in `session.ts` resolves against the wrong root | `AbsCwd` newtype, parsed once per handler (F3) |
| `session-store.ts` `SessionStore.get`, `agent.ts` `deleteSession`/`cleanupFailedNewSession` | `sessionFile` names a session JSONL under the sessions root | `string` read from a JSON file, passed straight to `unlinkSync` | any path in that file is deleted; a corrupt or hand-edited map is an arbitrary-file-delete primitive | `SessionFile` newtype with a containment-checked constructor (F3) |
| `session.ts` `PiAcpSession` fields `pendingTurn`/`cancelRequested`/`inAgentLoop`/`turnQueue` | exactly one ACP turn is in flight, and its `stopReason` is produced exactly once | four independent fields; `inAgentLoop` is **assigned at five sites and read at none** (`session.ts:285,476,503,822,835,846`); `cancelRequested` is reset in `startTurn` but read from *outside* by `agent.ts` after the promise resolves | double-respond, drop-without-respond (an ACP request that never completes), or a stop reason re-derived from a flag the next turn has already reset | one `Turn` enum owning the `Responder<PromptResponse>` (F1) |
| `session.ts` `handlePiEvent`, `currentToolCalls` | for a given tool-call id, the first `session/update` is `tool_call`; later ones are `tool_call_update` | `existingStatus ? 'tool_call_update' : 'tool_call'` repeated at four sites, plus `includeTerminal: !existingStatus` | a `tool_call_update` for an id Zed never saw is dropped silently; a second `tool_call` re-renders the call; a second `terminal_info` `_meta` opens a second terminal | `ToolCallLedger` whose only entry constructor is `announce()` (F2) |
| `session.ts` `currentToolCalls: Map<id,'pending'\|'in_progress'>` | status is monotonic | hand-enforced (`const status = existingStatus ?? 'pending'`, with the comment "never downgrade status") | regressing to `pending` makes clients hide progress — pi-acp's own comment states this | `ToolStatus` with no backwards transition method (F2) |
| `translate/bash.ts` `isBashTool` vs `session.ts` `toToolKind` vs `tool_execution_start`'s `toolName === 'edit' \|\| toolName === 'write'` | one tool-name classification drives three decisions: terminal rendering, ACP `ToolKind`, snapshot-for-diff | three independent string comparisons that can disagree | cyrup's shell tool is `bash` **or** `powershell` (`crates/cyrup-tools/src/tools/powershell.rs`, `name: "powershell"`); a straight port classifies it `Other`, silently dropping terminal rendering, output deltas and exit code on Windows only. `find`/`grep`/`ls` also collapse to `Other` | one `ToolClass::of(&str)` feeding all three (F2) |
| `agent.ts` `newSession`/`loadSession`, the two `setTimeout(…, 0)` blocks | `available_commands_update` must reach the client *after* the response that creates the session | `setTimeout(…, 0)`, with the comment "some clients (e.g. Zed) will ignore notifications for an unknown sessionId" | in Rust, `ConnectionTo::send_notification` enqueues synchronously, so an update sent before `responder.respond(..)` is written first — the exact bug the `setTimeout` avoids, reintroduced with no timer to hide it | handlers return `(Response, Vec<SessionUpdate>)`; only the shell holds `cx` (F2) |
| `auth-required.ts` `maybeAuthRequiredError` | "this error means the user must authenticate" | substring match over `['api key','401','403','unauthorized','permission denied','forbidden',…]` on `String(err.message)` | a bash `EACCES` ("permission denied") mid-turn tears the session down and shows an Authenticate banner | exhaustive `match` on `SessionServiceError` (F4) |
| `agent.ts` `isThinkingLevel` + `buildConfigOptions` | the accepted `thought_level` values are exactly the advertised ones | a `x is ThinkingLevel` predicate listing six strings, and a separate `available: ThinkingLevel[]` literal listing the same six | cyrup's `ModelThinkingLevel` (`crates/cyrup-core/src/message/thinking.rs`) has a seventh rung, `Max`. Advertise from cyrup + validate from pi's copied list ⇒ the client's own dropdown entry is rejected with `invalidParams` | one `SessionConfigKnob` enum that both advertises and parses (F5) |
| `agent.ts` `setSessionModel` | the advertised model value id (`provider/id`) round-trips back to a provider+model pair | built by string concat in `getModelState`, torn apart by `split('/')` in `setSessionModel`, with a fallback that re-queries `getAvailableModels` | a provider or model id containing `/` splits wrong; the fallback silently picks a different provider's model of the same id | one `model_value_id(&ModelRef)` used by both directions; cyrup already has `AgentSession::set_model(pattern)` + `cyrup_config::ModelResolver` (F5) |
| `session.ts` `handleExtensionUiRequest` and its callers | every extension UI request is answered exactly once, on every path including throw | `sendExtensionUiResponse({id, cancelled:true})` repeated at six sites plus a `.catch` | in cyrup the analogue is `UiRequest.reply: oneshot::Sender<UiReply>` (`crates/cyrup-session-svc/src/host_services.rs`); dropping it is *fail-closed* — `LiveHostServices::ui_roundtrip` times out — so this invariant is already weaker here than upstream | reuse `cyrup_modes::rpc`'s `PendingUi` / `default_ui_reply` shape; do not re-derive (§5) |
| `agent.ts` `initialize` | `session/*` is only meaningful after `initialize` | not checked at all | a `session/new` before `initialize` is served, so `clientCapabilities` (including `auth.terminal` / `_meta["terminal-auth"]`) is unknown when auth methods are chosen | `OnceLock<ClientView>` read by each handler — **not** typestate (§5) |
| `session.ts` `startupInfo`/`startupInfoSent` | the startup prelude is emitted at most once | two fields; `setStartupInfo` resets `startupInfoSent = false` | re-calling `setStartupInfo` re-arms an already-sent prelude | `Option<StartupNotice>` taken by `Option::take` |
| ACP `SessionInfoUpdate` (`tmp/agent-client-protocol-schema-1.7.0/src/v1/client.rs:441`) | "title unchanged" and "clear the title" are different messages | pi-acp always sends a concrete `title` string (`agent.ts`'s `/name` arm) | cyrup's `AgentSessionEvent::SessionInfoChanged { name: Option<String> }` has **two** states; ACP's `MaybeUndefined<String>` has three. Mapping `None → Null` clears a title Zed should have kept | keep `MaybeUndefined`; never widen it to `Option` inside cyrup-acp |
| `agent.ts` `listSessions` | the pagination cursor is an offset into a *stable* ordering | `Number.parseInt(params.cursor, 10)`, "if cursor is invalid, treat as 0" | a session written between pages shifts the offset and a session is skipped or repeated | out of scope for types; note as a known ACP-level limitation |

---

## 3. Findings

#### [P1] Collapse the four turn fields into one `Turn` enum that owns the ACP responder

**Location.** Upstream: `session.ts`'s `PiAcpSession` fields `pendingTurn`, `turnQueue`, `cancelRequested`, `inAgentLoop`; its `prompt`, `cancel`, `startTurn`, `wasCancelRequested`, and the `agent_settled` arm of `handlePiEvent`; plus `agent.ts`'s `prompt`, which computes `stopReason` from `result === 'error' ? (session.wasCancelRequested() ? 'cancelled' : 'end_turn') : result`. cyrup side: `AgentSession::prompt_accepted` / `prompt_with` (`crates/cyrup-session-svc/src/session/run.rs`), `AgentSessionEvent::AgentSettled` (`crates/cyrup-session-svc/src/event.rs`), and the settle predicate already used by `cyrup_modes`' RPC loop (`crates/cyrup-modes/src/rpc/mod.rs`, the `in_flight = false` on `AgentSettled`).

**Current representation.** Four mutable fields plus a `PendingTurn { resolve, reject }` pair. `inAgentLoop` is written five times and never read — it is state that exists only to be assigned. `cancelRequested` is a `bool` that outlives the turn it describes: `cancel()` sets it, `startTurn()` clears it, and `agent.ts` reads it *after* awaiting the turn promise, i.e. after `startTurn` may already have cleared it for the queued successor.

**Invariant or legal sequence.** At most one ACP `session/prompt` responder is outstanding per session; it is answered exactly once; the answer is produced at `AgentSettled` (not `AgentEnd` — cyrup's own doc on `AgentSessionEvent::AgentSettled` states a turn that auto-retries emits two `AgentEnd`s and one `AgentSettled`, and pi-acp rediscovered the same rule in its `agent_end` arm); and the stop reason is computed at settle time from the state of *that* turn.

**Concrete failure mode.** In Rust the responder is a one-shot `Responder<PromptResponse>` that must be moved into the task spawned by `cx.spawn` (the Architecture phase established that awaiting the turn inside the handler blocks the dispatch loop and would make `session/cancel` undeliverable). With four loose fields, the realistic misuse is: an extension calls `ctx.newSession()` mid-turn, the runtime bumps `watch_generation`, the driver rebinds, and the *old* subscription's terminal `SessionReplaced` plus the *new* session's `AgentSettled` both reach the settle path. One of them responds; the other calls `respond` on a moved-from responder (does not compile) or, in the natural workaround where the responder sits in an `Option<Responder<_>>` behind a mutex, `take()`s `None` and the turn is answered once — but the symmetric bug, an early `return Err(..)` path that drops the `Option` without taking it, leaves the editor's `session/prompt` request permanently unanswered. There is no timeout on the ACP side; the user sees a spinner forever.

**Recommended pattern.** Explicit domain enum with private fields and a consuming settle. Not typestate.

**Why this pattern fits (versus simpler alternatives).** A newtype does not help — the problem is a *combination* of fields, not a value. A private constructor does not help — the problem is the transition, not construction. A smaller pure function does not help — the responder is an owned resource. Typestate (`Turn<Idle>` / `Turn<Running>`) would be strictly worse: the turn is stored in the per-session actor's state and mutated by events arriving on a channel, so the state is only knowable at runtime, and a generic marker would force the actor's own type to change on every transition. An enum inspected dynamically is exactly what the decision rules prescribe for this shape.

**Minimal proposed API.**

```rust
/// The ACP turn for one `session/prompt`, and the sole owner of its responder(s).
/// Ported from pi-acp v0.0.33 `session.ts`'s `PiAcpSession.pendingTurn`/`cancelRequested`
/// /`inAgentLoop`/`turnQueue`, collapsed into one value.
enum Turn {
    Idle,
    Running(RunningTurn),
}

struct RunningTurn {
    /// The `session/prompt` request(s) folded into this run. See the open question below.
    responders: Vec<Responder<PromptResponse>>,
    cancel: CancelState,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CancelState { None, Requested }

/// Why an ACP turn ended. The ONLY producer of an ACP `StopReason`.
enum TurnOutcome {
    Settled,
    Cancelled,
    /// A preflight refusal — the client gets an error response, not a stop reason.
    Refused(AcpFailure),
}

/// What the shell must do. `#[must_use]` so a settle that is computed and then dropped
/// is a warning, not a silent hang.
#[must_use]
struct SettleAction {
    responders: Vec<Responder<PromptResponse>>,
    result: Result<PromptResponse, agent_client_protocol::Error>,
}

impl Turn {
    /// Adopt the responder for an accepted prompt. `Err` hands it back so the caller
    /// must decide what to answer; it cannot be silently dropped.
    fn start(&mut self, r: Responder<PromptResponse>, accepted: PromptAccepted)
        -> Result<(), Responder<PromptResponse>>;

    /// Idempotent. Returns `false` when nothing is running.
    fn request_cancel(&mut self) -> bool;

    /// Settle once. A second call returns `None` — a late `AgentSettled` from a replaced
    /// session is a no-op, not a double-respond and not a panic.
    fn settle(&mut self, outcome: TurnOutcome) -> Option<SettleAction>;
}
```

**Guarantee gained.** Double-respond becomes unrepresentable: the responders are moved out of `Running` by `settle`, leaving `Idle`. The stop reason cannot be computed anywhere but `settle`, so `agent.ts`'s "ask the session whether cancel was requested, after the fact" pattern has no expressible form. `inAgentLoop`-style dead state cannot accumulate, because the enum has no field that is not read by a transition. `start` returning the responder on rejection makes "accepted a prompt and forgot the responder" a compile error at the call site rather than a hang.

**Guarantee not gained.** Rust cannot force `SettleAction` to actually be delivered — `#[must_use]` warns, it does not prove. Nothing here prevents the *session* from settling without ever emitting `AgentSettled` (a wedged tool, a panicking driver task); that still needs the shell's own timeout and still needs a test. And this says nothing about ordering relative to `session/update` notifications — that is F2.

**Migration and compatibility cost.** None; the crate does not exist. The cost is that `Turn` must be behind the same lock as whatever drains the event stream, which is a design constraint on the per-session actor, not a refactor.

**Benefit versus ceremony.** Four fields and one dead one become one enum with three transitions. Strongly positive.

**Confidence.** High for the enum. **Medium for the `responders: Vec<_>` shape** — it encodes an inference. pi-acp holds queued prompts in its own `turnQueue` and answers each separately; cyrup instead queues into the session's steer/follow-up queues (`AgentSession::steer`/`follow_up`, `crates/cyrup-session-svc/src/session/queue.rs`) and emits **one** `AgentSettled` for the whole loop, so there is no per-submission settle to respond to. Holding all responders and settling them together is the shape I would build, but **it is an open question** whether ACP clients tolerate N prompt responses arriving simultaneously, and it should be resolved against a real Zed build before the type is fixed. Do not port `turnQueue` itself — cyrup's queues plus `AgentSessionEvent::QueueUpdate` already carry the depth pi-acp publishes by hand in `_meta.piAcp.queueDepth`.

---

#### [P1] A pure `translate()` over an explicit `ToolCallLedger`; the shell owns `ConnectionTo<Client>`

**Location.** Upstream: `session.ts`'s `handlePiEvent` (the whole switch), `emit`/`flushEmits`/`lastEmit`, `emitBashToolCall`, `emitBashOutputUpdate`, `cleanupToolCall`, and the five collections declared at `session.ts:277-296`; `translate/bash.ts`'s `isBashTool`/`bashOutputDelta`/`bashTerminalInfoMeta`; `session.ts`'s `toToolKind`; and `agent.ts`'s two `setTimeout(() => …, 0)` blocks in `newSession` and `loadSession`. cyrup side: `AgentSessionEvent` (`crates/cyrup-session-svc/src/event.rs`) and `cyrup_provider::StreamEvent` (`crates/cyrup-provider/src/stream.rs`), which give `ToolCallStart/Delta/End` a typed `content_index` and `partial: Arc<AssistantMessage>` — the fields `session.ts` reaches for as `ame?.partial?.content?.[ame?.contentIndex ?? 0]`.

**Current representation.** One method that classifies, decides, reads files with `readFileSync`, mutates five maps/sets, and sends notifications. Tool-call state is spread across `currentToolCalls: Map<id, 'pending'|'in_progress'>`, `fileSnapshots: Map<id, {path, oldText}>`, `fileMutationToolCallIds: Set<id>`, `bashToolCallIds: Set<id>`, `bashOutputSnapshots: Map<id, string>` — five collections keyed by the same id whose *contents* imply which of three kinds of tool call this is.

**Invariant or legal sequence.** (a) For each tool-call id the first `session/update` is `tool_call`, all later ones `tool_call_update`; (b) status never regresses; (c) the bash `terminal_info` `_meta` and terminal content accompany the announce only; (d) bash output is emitted as a *delta* against what was already sent (`bashOutputDelta`); (e) a diff is emitted only when a pre-mutation snapshot exists and the content actually changed; (f) `available_commands_update` and the startup prelude must not be written before the response of the request that created the session.

**Concrete failure mode.** Two, both realistic:

1. *Classification drift.* `isBashTool` is `toolName.toLowerCase() === 'bash'`. cyrup's shell tool is `bash` on unix and `powershell` on Windows (`crates/cyrup-tools/src/tools/powershell.rs`). A faithful port classifies `powershell` as a generic tool, so on Windows the editor gets no terminal, no incremental output, and no exit code — and the bug is invisible on the developer's machine. The same three predicates also disagree about `find`/`grep`/`ls`, which land in `ToolKind::Other` when ACP has `Search` and `Read`.
2. *Ordering.* In pi-acp the `setTimeout(…, 0)` is what keeps `available_commands_update` behind the `session/new` response. `ConnectionTo::send_notification` is synchronous — it enqueues on an `mpsc` and returns `Result<(), Error>` with no `.await`. So the natural Rust port, "call `cx.send_notification(..)` at the end of the `session/new` handler and then `responder.respond(..)`", writes the notification **first**, and Zed drops updates for a `sessionId` it has not yet been told about. There is no timer left to accidentally save you.

**Recommended pattern.** Functional core / imperative shell, with the core's state expressed as one enum-valued map. No typestate.

**Why this pattern fits.** The decisions here are total functions of (event, prior ledger state) — exactly what a pure core is for — while the two things that make them hard to test are I/O: the pre-mutation file read and the notification send. Splitting them means the entire `handlePiEvent` switch becomes table-testable with no connection, no session, and no tempdir. A newtype cannot express "five collections are one sum type". Typestate cannot be applied because the values live in a `HashMap` keyed by a wire id: a heterogeneous map of `ToolCall<Announced>` and `ToolCall<Unannounced>` needs one concrete type per entry, so the state must be a runtime tag — an enum — regardless. A private constructor *is* part of the answer (see `announce` below) but is not sufficient on its own.

**Minimal proposed API.**

```rust
// --- core: no I/O, no `ConnectionTo`, no tokio -----------------------------------------

/// How a tool name maps onto ACP rendering. One classifier; three consumers.
/// Ported from pi-acp v0.0.33 `translate/bash.ts`'s `isBashTool` + `session.ts`'s
/// `toToolKind` + the `toolName === "edit" || toolName === "write"` test, which are
/// three independent string comparisons upstream.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToolClass { Terminal, Mutation, Read, Search, Other }

impl ToolClass {
    pub fn of(tool_name: &str) -> Self { /* bash|powershell => Terminal, edit|write => Mutation, … */ }
    pub fn acp_kind(self) -> ToolKind { /* … */ }
}

/// Live tool calls. The only way to create an entry is `announce`, so an update for an
/// unannounced id is unrepresentable inside this module.
pub struct ToolCallLedger { open: HashMap<acp::ToolCallId, ToolCallStream> }

struct ToolCallStream {          // all fields private
    status: ToolStatus,
    body: StreamBody,
}

enum StreamBody {
    Terminal { emitted: String },                       // was `bashOutputSnapshots`
    Mutation { path: PathBuf, before: Option<String> }, // was `fileSnapshots`
    Plain,
}

/// No transition back to `Pending` exists.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolStatus { Pending, InProgress }

/// What the shell must send, plus what it must do next. Named outcomes, not `Option`s.
pub struct Translated {
    pub updates: Vec<SessionUpdate>,
    pub turn: TurnSignal,
}

pub enum TurnSignal {
    Continue,
    /// `AgentSessionEvent::AgentSettled` — the only settle point (see F1).
    Settled,
    /// `AgentSessionEvent::SessionReplaced { generation }` — rebind, do not settle.
    Rebind { generation: u64 },
    /// A cyrup super-set event with no ACP representation; deliberately dropped,
    /// mirroring `cyrup_modes::is_upstream_wire_event`'s exclusions.
    Ignored,
}

/// The pure core. `snapshot` is supplied by the shell for `Mutation` starts/ends —
/// the core never touches the filesystem.
pub fn translate(
    ledger: &mut ToolCallLedger,
    ev: &AgentSessionEvent,
    snapshot: Option<FileSnapshot>,
) -> Translated;

/// Which reads the shell must perform BEFORE calling `translate` — itself pure.
pub fn snapshot_needed(ev: &AgentSessionEvent) -> Option<&Path>;

// --- shell -----------------------------------------------------------------------------

// per session-update pump:
//   let snap = snapshot_needed(&ev).map(read_to_string_lossy);   // I/O lives here
//   let Translated { updates, turn } = translate(&mut ledger, &ev, snap);
//   for u in updates { cx.send_notification(SessionNotification::new(sid.clone(), u))?; }
//   match turn { TurnSignal::Settled => { /* F1: turn.settle(..) */ } … }

/// Request handlers cannot send notifications: they never receive `cx`.
/// The shell responds first, then drains `follow_up` — the `setTimeout(…, 0)` invariant,
/// made structural.
pub struct HandlerOutcome<R> { pub response: R, pub follow_up: Vec<SessionUpdate> }

fn handle_new_session(/* no cx */ …) -> Result<HandlerOutcome<NewSessionResponse>, AcpFailure>;
```

**Guarantee gained.** A `tool_call_update` for an id that was never announced cannot be constructed inside the translator module, because `StreamBody` and `ToolCallStream` are private and the only entry constructor is `announce`. `ToolStatus` regression is unrepresentable — there is no `-> Pending` transition. The terminal `_meta` and terminal content are emitted by `announce` and by nothing else, so a duplicate terminal cannot occur. The `powershell` gap is closed at one site instead of three. And a request handler that has no `ConnectionTo<Client>` in scope *cannot* write a notification before its own response — the ordering invariant becomes a visibility rule rather than a timer. The whole switch becomes testable as `(ledger, event) -> Vec<SessionUpdate>` with no runtime.

**Guarantee not gained.** The `Vec<SessionUpdate>` the core returns can still be reordered, dropped, or sent against the wrong `SessionId` by the shell — that is exactly what remains to be integration-tested. Nothing here proves the *content* of a `Diff` is correct, only that it is emitted under the right precondition. `ToolClass::of` is still a string match against tool names that MCP servers and extensions can choose freely; it classifies unknown names as `Other`, which is right but is a runtime default, not a proof. And the ledger does not bound its own growth: a tool call whose `ToolExecutionEnd` never arrives leaks an entry, so the shell must clear the ledger on `AgentSettled` (pi-acp does not, which is a latent leak in a long-lived session).

**Migration and compatibility cost.** None — this is the initial shape. The one real constraint it imposes is that the event pump must be able to perform blocking-ish file reads between events; those should go through `tokio::task::spawn_blocking` or `tokio::fs`, not the synchronous `readFileSync` port (pi-acp does the read inline, which in Rust would block the pump and therefore delay every subsequent `session/update`).

**Benefit versus ceremony.** High benefit, low ceremony: one enum, one struct, two free functions. The `HandlerOutcome<R>` wrapper is the only added indirection and it buys the ordering invariant outright.

**Confidence.** High. Every element is directly evidenced: the five collections and the four `existingStatus` ternaries are in `session.ts`; the `setTimeout` comments state the ordering hazard in the upstream author's own words; `send_notification`'s synchronous-enqueue behaviour and the handler-blocks-the-loop rule are established facts from the Architecture phase's compiled probe; the `powershell` name is in `crates/cyrup-tools/src/tools/powershell.rs`.

---

#### [P1] `AbsCwd` and `SessionFile`: the two filesystem authorities that arrive as strings

**Location.** Upstream: `agent.ts`'s `newSession` and `loadSession` (`if (!isAbsolute(params.cwd)) throw RequestError.invalidParams(…)`), `agent.ts`'s `restoreSession` (`const cwd = opts?.cwd ?? stored.cwd` — no check), `session-store.ts`'s `StoredSession.sessionFile`, `agent.ts`'s `deleteSession` (`const sessionFile = stored?.sessionFile ?? piSession?.sessionFile; … unlinkSync(sessionFile)`) and `cleanupFailedNewSession` (same unlink). cyrup side: `cyrup_session::layout::{SessionLayout, SessionsRoot}`, `cyrup_session::listing::SessionInfo` (its `path: PathBuf`), and `cyrup_session_svc::delete_session_file_at` (`crates/cyrup-session-svc/src/session/files.rs`).

**Current representation.** Both are `string`. `cwd` is validated at two of the three entry points. `sessionFile` is never validated at all — it is read out of `~/.pi/pi-acp/session-map.json`, a file whose loader (`session-store.ts`'s `loadFile`) checks only `version === 1` and that `sessions` is an object.

**Invariant or legal sequence.** `cwd` is absolute at every point where it is joined against, resolved against, or handed to a session builder. `sessionFile` is a `.jsonl` under the configured sessions root before anything unlinks it.

**Concrete failure mode.** `deleteSession` is a delete primitive over an unvalidated path. The realistic route is not an attacker: it is a stale or half-written map file — `saveFile` is a non-atomic `writeFileSync`, so a crash mid-write leaves truncated JSON, and the recovery path returns `{version:1, sessions:{}}`, which merely loses entries. The sharper route is that `sessionFile` for a *new* session comes from `state?.sessionFile` returned by the child process (`session.ts`'s `SessionManager.create`), i.e. from a value the adapter did not compute. In-process, cyrup-acp gets this from `SessionLayout` and `listing::SessionInfo` and there is no excuse to keep it as a bare `PathBuf` after that. The `cwd` case is simpler and equally real: `prompt` → `restoreSession(sessionId)` uses `stored.cwd` with no absolute check, and `session.ts`'s `toToolCallLocations` does `isAbsolute(path) ? path : resolvePath(cwd, path)` — a relative stored cwd silently resolves tool-call locations against the adapter's process cwd, so Zed opens the wrong files.

**Recommended pattern.** Two parse-don't-validate newtypes, constructed at the boundary, inner representation private, no `Deserialize`, no `Default`, no `From<PathBuf>`.

**Why this pattern fits.** This is the textbook case: a validator (`isAbsolute`) returning a `bool` after which the raw primitive continues downstream, and a "must already be validated" fact carried by nothing. A domain enum is not the shape — there is one valid state. Typestate is absurd here. A private constructor alone is not enough because the *type* must differ, or `resolvePath(cwd, …)` will still accept a raw `&Path`.

**Minimal proposed API.**

```rust
/// An absolute working directory supplied by the ACP client.
/// Ported from pi-acp v0.0.33 `agent.ts`'s `isAbsolute(params.cwd)` guard, hoisted to
/// the type level so the `restoreSession` path cannot skip it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbsCwd(PathBuf);          // private field, no Default, no Deserialize

impl AbsCwd {
    pub fn parse(p: PathBuf) -> Result<Self, AcpFailure> {
        if p.is_absolute() { Ok(Self(p)) }
        else { Err(AcpFailure::InvalidParams {
            message: format!("cwd must be an absolute path: {}", p.display()) }) }
    }
    pub fn as_path(&self) -> &Path { &self.0 }
}

/// A session JSONL proven to live under the sessions root this process is configured for.
pub struct SessionFile(PathBuf);     // private field

impl SessionFile {
    /// The only constructor. `candidate` must normalise to a `.jsonl` under `root`.
    pub fn resolve(root: &SessionsRoot, candidate: &Path) -> Result<Self, SessionFileError>;
    /// Infallible: a listing entry is by construction under the root that produced it.
    pub fn from_listing(info: &cyrup_session::listing::SessionInfo) -> Self;
    pub fn path(&self) -> &Path { &self.0 }
}

// deletion takes the proof, not a path
fn delete(file: &SessionFile) -> Result<(), AcpFailure>;
```

**Guarantee gained.** No function that joins against a cwd can be called with a relative one, at any of the three entry points, because they take `&AbsCwd`. No delete can be reached with a path that did not come from either the listing layer or an explicit containment check. The "sometimes validated, sometimes forgotten" pattern in `agent.ts` has no expressible form.

**Guarantee not gained — and this is the part not to overstate.** (1) **Serde cannot carry this invariant.** `NewSessionRequest.cwd` is a `PathBuf` deserialized by `agent-client-protocol-schema`, in a `#[non_exhaustive]` struct cyrup does not own; the schema's fields also carry `#[serde_as(deserialize_as = "DefaultOnError")]`, so a malformed optional field silently becomes `None` rather than failing. `AbsCwd` therefore has *no* validating `Deserialize` path — it must be the first statement of each handler, and the guarantee is "everything downstream of the handler", not "everything". (2) `SessionFile::resolve`'s containment check is defeated by a symlink inside the root pointing outside it; `starts_with` after normalisation does not resolve symlinks, and `canonicalize` fails on a path that does not exist yet, so the check cannot be both total and symlink-proof. State the residual risk in the doc comment rather than implying the type proves more than it does. (3) Neither type says the path still exists, or that another `cyrup` process is not writing it — this is a filesystem, and TOCTOU is unaffected.

**Migration and compatibility cost.** Near zero at introduction; the cost is discipline — every helper signature takes `&AbsCwd`/`&SessionFile` rather than `&Path`, and `as_path()` is called only at the actual I/O call.

**Benefit versus ceremony.** Two small types, roughly thirty lines, removing a class of bug that includes a delete primitive. Clearly worth it.

**Confidence.** High for `AbsCwd`. **Medium for `SessionFile`**, because it depends on an unresolved design decision: if `cyrup-acp` reads `cyrup_session::listing` as the single source of truth and keeps **no** `session-map.json` mirror (which I recommend — see §5), then `from_listing` is the only constructor ever used and `resolve` exists only for defence in depth. That decision is an open question I could not settle from the code.

---

#### [P1] `AcpFailure::classify` by exhaustive match — never port `maybeAuthRequiredError`

**Location.** Upstream: `auth-required.ts`'s `maybeAuthRequiredError`, and its four call sites — `session.ts`'s `startTurn` catch, and `agent.ts`'s `newSession` (twice: the `availableModelsErr` branch and the `stateErr` branch). cyrup side: `SessionServiceError` (`crates/cyrup-session-svc/src/error.rs`), specifically `NoConfiguredAuth`, `AuthPreflightRefused`, `NoModelSelected`, `ModelNotFound`, `NoModelForSummarization`.

**Current representation.** `String(err.message).toLowerCase()` tested against `['api key','apikey','missing key','no key','not configured','unauthorized','authentication','permission denied','forbidden','401','403']`, plus a structural rule "zero models after spawn means unauthenticated".

**Invariant or legal sequence.** "The user must authenticate" is a distinguishable outcome, not a guess about a message.

**Concrete failure mode.** `startTurn`'s catch runs `maybeAuthRequiredError(err)` on *any* error that surfaces from the prompt path. cyrup's bash tool routinely produces messages containing `permission denied` (an `EACCES` on a script, a protected directory, a `sudo`-less command). A straight port therefore turns a failed bash call into `RequestError.authRequired`, which in pi-acp's `newSession` path is paired with `cleanupFailedNewSession` — which *unlinks the session file*. On the prompt path it "only" rejects the turn and shows the editor an Authenticate banner mid-conversation. Either way the user is told to reconfigure credentials that are fine. The `'403'` and `'401'` patterns are worse: any tool output or provider error text containing those three digits matches.

**Recommended pattern.** A domain enum for the client-visible failure, produced by an exhaustive `match` on cyrup's typed error, with technical aborts staying in `Result`.

**Why this pattern fits.** This is the decision rule verbatim — "expected business outcome ⇒ a named enum variant; technical failure that aborts ⇒ `Result<T, E>`". "Needs auth" is a business outcome the ACP protocol has a first-class representation for (`Error::auth_required()` plus `AuthMethod`s); "the JSONL write failed" is a technical abort. A newtype does not apply. Critically, per house rule 4 this is **not new machinery**: cyrup already draws the distinction, with `AuthPreflightRefused` carrying pi's verbatim `formatNoApiKeyFoundMessage` text and `NoModelSelected` carrying the `/login` → `/model` guidance. The port's job is to stop discarding that structure.

**Minimal proposed API.**

```rust
/// How a cyrup failure is presented to the ACP client. Replaces pi-acp v0.0.33
/// `auth-required.ts`'s `maybeAuthRequiredError` substring ladder.
pub enum AcpFailure {
    /// The client should offer `authenticate` / show the auth banner.
    AuthRequired { detail: String },
    InvalidParams { message: String },
    Internal { message: String },
}

impl AcpFailure {
    pub fn classify(err: &SessionServiceError) -> Self {
        use SessionServiceError as E;
        match err {
            E::NoConfiguredAuth(m) | E::AuthPreflightRefused(m) =>
                AcpFailure::AuthRequired { detail: m.clone() },
            E::NoModelSelected =>
                AcpFailure::AuthRequired { detail: err.to_string() },
            E::ModelNotFound(p) =>
                AcpFailure::InvalidParams { message: format!("Unknown modelId: {p}") },
            // …every other named variant decided explicitly…
            other => AcpFailure::Internal { message: other.to_string() },
        }
    }
}

impl From<AcpFailure> for agent_client_protocol::Error { /* byte-exact messages */ }
```

**Guarantee gained.** A tool's `permission denied` can never be classified as an auth failure, because classification no longer looks at message text. Adding an auth-bearing variant upstream is a decision someone makes at this `match` rather than a substring that happens to match.

**Guarantee not gained — say this plainly.** `SessionServiceError` is a large enum with several `#[from]` transparent variants (`Core`, `Agent`, `Session`, `Extension`, …), and a genuine auth failure originating inside `cyrup_provider` may arrive wrapped in one of those and land in the `other =>` arm as `Internal`. That is the *safe* direction — under-reporting auth rather than over-reporting it — but it is not a proof of correct classification, and it is the reason the catch-all must be `Internal` and never `AuthRequired`. The type also does not prove the *message strings* match pi byte-for-byte; that needs assertions. And the "zero models after spawn ⇒ unauthenticated" rule from `agent.ts`'s `newSession` is a separate, structural check that survives — in cyrup it becomes "the built session has no model", i.e. exactly the `NoModelSelected` condition, and should be asked as a question about session state rather than inferred from a failed call.

**Migration and compatibility cost.** Zero. This is a decision about what *not* to port.

**Benefit versus ceremony.** One enum, one `match`, replacing a heuristic that misfires on one of the most common strings in tool output.

**Confidence.** High. The substring list is verbatim in `auth-required.ts`; the typed cyrup variants and their doc comments are in `crates/cyrup-session-svc/src/error.rs`.

---

#### [P2] One `SessionConfigKnob` enum that both advertises and accepts

**Location.** Upstream: `agent.ts`'s `MODEL_CONFIG_ID`/`THOUGHT_LEVEL_CONFIG_ID` constants, `isThinkingLevel`, `getThinkingState` (whose `available: ThinkingLevel[]` literal re-lists the same six values), `buildConfigOptions`, `getModelState` (`modelId: \`${provider}/${id}\``), `setSessionModel` (`requestedModelId.split('/')`), `setSessionMode`, `setSessionConfigOption`, and `unstable_setSessionModel`. cyrup side: `cyrup_core::ModelThinkingLevel` and `ThinkingLevel` (`crates/cyrup-core/src/message/thinking.rs`), `cyrup_core::ModelRef`, `AgentSession::set_model` / `set_model_id` / `available_thinking_levels` / `set_thinking_level`.

**Current representation.** The advertised option list and the accepted-value validator are two independent literals over the same string space, and the model value id is built by concatenation in one function and destructured by `split('/')` in another. `setSessionMode` and `setSessionConfigOption('thought_level')` are two code paths for one operation with two different error strings.

**Invariant or legal sequence.** The set of values a client may send for a config option is exactly the set the agent advertised for it, and the value id round-trips.

**Concrete failure mode.** cyrup's `ModelThinkingLevel` has a seventh rung, `Max`, that pi does not have (`crates/cyrup-core/src/message/thinking.rs`: "Pi added `"max"` in fbdd4638"). Advertise from `AgentSession::available_thinking_levels()` — the natural in-process source — while porting `isThinkingLevel`'s six-string predicate verbatim, and the client renders a `max` entry in its dropdown that the agent then rejects with `invalidParams: Unknown thinking level: max`. The user sees a dropdown option that does not work. The model side has the mirror bug: `setSessionModel`'s fallback, when the requested id has no `/`, searches `getAvailableModels` for `String(m?.id) === modelId` and takes the **first** match — with two providers exposing the same model id, it silently selects the wrong provider's model.

**Recommended pattern.** A domain enum with a single advertise/parse pair over one id space.

**Why this pattern fits.** Two semantically different values (`model`, `thought_level`) share one primitive (`String` config id) and one value primitive (`String`), with repeated validation — the classic newtype/enum signal. But note what is *not* recommended: no newtype for the model id string, because `cyrup_core::ModelRef` plus `cyrup_config::ModelResolver` (used by `AgentSession::set_model`) already own that parsing, and `SessionConfigOptionValue` (`tmp/agent-client-protocol-schema-1.7.0/src/v1/agent.rs:2445`) is already a domain enum (`Boolean{value}` / untagged `ValueId{value}`) that replaces pi-acp's `typeof params.value !== 'string'` check. The finding is to *use* those rather than flatten them back to strings.

**Minimal proposed API.**

```rust
/// A settable session config option. Ported from pi-acp v0.0.33 `agent.ts`'s
/// MODEL_CONFIG_ID / THOUGHT_LEVEL_CONFIG_ID pair, with advertise and parse unified.
pub enum SessionConfigKnob {
    Model(ModelRef),
    Thinking(ModelThinkingLevel),
}

impl SessionConfigKnob {
    const MODEL_ID: &'static str = "model";
    const THINKING_ID: &'static str = "thought_level";

    /// The ONLY place option ids and value ids are minted.
    pub fn advertise(view: &SessionConfigView) -> Vec<SessionConfigOption>;

    /// The exact inverse of `advertise`, over the same id space.
    pub fn parse(config_id: &str, value: &SessionConfigOptionValue)
        -> Result<Self, AcpFailure>;

    /// `session/set_mode` is `Thinking` under another name — one validator, two entry points.
    pub fn parse_mode(mode_id: &SessionModeId) -> Result<Self, AcpFailure>;
}

/// Shared by both directions; a model value id is never built ad hoc.
fn model_value_id(m: &ModelRef) -> String;
fn model_from_value_id(id: &str, catalog: &[ModelRef]) -> Option<ModelRef>;
```

**Guarantee gained.** The advertised set and the accepted set cannot drift, because both derive from `ModelThinkingLevel`'s variants and one catalog. `set_mode` and `set_config_option` cannot disagree about what a thinking level is. The `SessionConfigOptionValue` enum's `Boolean` arm forces a decision instead of being silently rejected by a `typeof` test.

**Guarantee not gained.** The round-trip is only as good as `model_value_id`/`model_from_value_id` being mutual inverses, which is a *test* obligation, not a type-level one — a provider or model id containing `/` still needs an explicit decision (percent-encode, or use a `SessionConfigValueId` that is opaque and looked up rather than parsed; the latter is what I would build). Nothing here prevents the underlying catalog changing between the advertise and the set, so `parse` must still fail gracefully against a model that has since disappeared.

**Migration and compatibility cost.** None at introduction. One thing to note for parity: pi-acp's `LoadSessionResponse` returns `{configOptions, models, modes, _meta}` and the Rust `LoadSessionResponse` has no `models` field, so the `models` payload must move into `_meta` or be dropped — a deliberate `CYRUP-DELTA` decision that this enum's `advertise` should make in one place.

**Benefit versus ceremony.** One enum plus two free functions replacing four literals and two parsers. Positive but not dramatic, hence P2.

**Confidence.** Medium-high. The `Max` divergence is directly evidenced on both sides. The `split('/')` fragility is evidenced but I could not confirm from the corpus whether any real cyrup provider or model id contains a `/` — treat that half as an inference.

---

## 4. Highest-value refactor sketch — F2, the translator

### Before (`tmp/pi-acp` @ `v0.0.33`, `src/acp/session.ts`, the `tool_execution_end` arm)

One method that classifies, reads the filesystem, decides, mutates four collections, and sends — with the tool's identity recovered by asking a `Set` rather than by looking at a value:

```ts
case 'tool_execution_end': {
  const toolCallId = String((ev as any).toolCallId ?? '')
  if (!toolCallId) break
  const result = (ev as any).result
  const isError = Boolean((ev as any).isError)

  if (this.bashToolCallIds.has(toolCallId)) {           // identity from a Set
    this.emitBashOutputUpdate({ toolCallId, status: isError ? 'failed' : 'completed', result, isError })
    this.cleanupToolCall(toolCallId)                    // five deletes
    break
  }

  const text = toolResultToText(result)
  const snapshot = this.fileSnapshots.get(toolCallId)
  let content: ToolCallContent[] | undefined
  let hasStructuredDiff = false

  if (!isError && snapshot) {
    try {
      const abs = isAbsolute(snapshot.path) ? snapshot.path : resolvePath(this.cwd, snapshot.path)
      const newText = readFileSync(abs, 'utf8')          // blocking I/O, inside the decision
      if (snapshot.oldText === null || newText !== snapshot.oldText) {
        hasStructuredDiff = true
        content = [{ type: 'diff', path: snapshot.path, oldText: snapshot.oldText, newText }]
      }
    } catch { /* fall back to text only */ }
  }
  if (!content && !hasStructuredDiff && text) {
    content = [{ type: 'content', content: { type: 'text', text } }]
  }

  this.emit({ sessionUpdate: 'tool_call_update', toolCallId,      // sends, from inside the decision
    status: isError ? 'failed' : 'completed', content,
    ...(hasStructuredDiff ? {} : { rawOutput: result }) })

  this.cleanupToolCall(toolCallId)
  break
}
```

### After (proposed `crates/cyrup-acp/src/translate.rs` — real Rust, module-private internals)

```rust
//! Port of pi-acp v0.0.33 `src/acp/session.ts`'s `handlePiEvent`, `emitBashToolCall`,
//! `emitBashOutputUpdate` and `cleanupToolCall`. Pure: no `ConnectionTo`, no filesystem,
//! no tokio. The shell in `session_actor.rs` performs the reads and the sends.

use agent_client_protocol::schema::v1::{
    SessionUpdate, ToolCall, ToolCallContent, ToolCallId, ToolCallStatus, ToolCallUpdate,
    ToolCallUpdateFields, ToolKind,
};
use cyrup_session_svc::AgentSessionEvent;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One classifier; three consumers (announce shape, ledger variant, ACP kind).
/// Replaces `translate/bash.ts`'s `isBashTool`, `session.ts`'s `toToolKind`, and the
/// inline `toolName === "edit" || toolName === "write"` test, which upstream can disagree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolClass { Terminal, Mutation, Read, Search, Other }

impl ToolClass {
    pub fn of(tool_name: &str) -> Self {
        match tool_name {
            // CYRUP-DELTA vs pi-acp: `powershell` is cyrup's Windows shell tool
            // (`crates/cyrup-tools/src/tools/powershell.rs`); upstream's `isBashTool`
            // matches only "bash" and would drop terminal rendering on Windows.
            "bash" | "powershell" => ToolClass::Terminal,
            "edit" | "write" => ToolClass::Mutation,
            "read" => ToolClass::Read,
            // CYRUP-DELTA: upstream collapses these to `other`; ACP has `Search`.
            "grep" | "find" | "ls" => ToolClass::Search,
            _ => ToolClass::Other,
        }
    }

    pub fn acp_kind(self) -> ToolKind {
        match self {
            ToolClass::Terminal => ToolKind::Execute,
            ToolClass::Mutation => ToolKind::Edit,
            ToolClass::Read => ToolKind::Read,
            ToolClass::Search => ToolKind::Search,
            ToolClass::Other => ToolKind::Other,
        }
    }

    /// What the shell must read before the core can decide. Pure.
    pub fn needs_snapshot(self) -> bool { matches!(self, ToolClass::Mutation) }
}

/// Live tool calls. `announce` is the ONLY entry constructor, so an update for an
/// unannounced id cannot be produced inside this module.
#[derive(Default)]
pub struct ToolCallLedger { open: HashMap<ToolCallId, Stream> }

struct Stream { status: Status, body: Body }        // private

enum Body {
    Terminal { emitted: String },                   // was `bashOutputSnapshots`
    Mutation { path: PathBuf, before: Option<String> }, // was `fileSnapshots`
    Plain,
}

/// No transition back to `Pending` exists. This is `session.ts`'s "never downgrade
/// status" comment, made structural.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Status { Pending, InProgress }

impl Status { fn advance(&mut self) { *self = Status::InProgress; } }

/// Pre-mutation file contents, captured by the shell.
pub struct FileSnapshot { pub path: PathBuf, pub before: Option<String> }

/// What the shell must send, and what it must do next. Named outcomes, not `Option`s.
pub struct Translated { pub updates: Vec<SessionUpdate>, pub turn: TurnSignal }

pub enum TurnSignal {
    Continue,
    Settled,                       // AgentSettled — the only settle point (F1)
    Rebind { generation: u64 },    // SessionReplaced — rebind, do not settle
    Ignored,                       // cyrup super-set event with no ACP representation
}

impl ToolCallLedger {
    /// The only way an id enters the ledger. Returns the `tool_call` announce.
    fn announce(&mut self, id: ToolCallId, class: ToolClass, title: String,
                snapshot: Option<FileSnapshot>) -> SessionUpdate {
        let body = match (class, snapshot) {
            (ToolClass::Terminal, _) => Body::Terminal { emitted: String::new() },
            (ToolClass::Mutation, Some(s)) => Body::Mutation { path: s.path, before: s.before },
            _ => Body::Plain,
        };
        self.open.insert(id.clone(), Stream { status: Status::Pending, body });
        SessionUpdate::ToolCall(
            ToolCall::new(id, title).kind(class.acp_kind()).status(ToolCallStatus::Pending),
        )
    }

    /// An update for an id the ledger does not hold yields `None` — the "client never
    /// saw this tool call" case, which upstream can emit and Zed silently drops.
    fn update(&mut self, id: &ToolCallId) -> Option<(&mut Stream, ToolCallUpdateFields)> { /* … */ }

    /// Bounded: the shell calls this on `AgentSettled`. Upstream leaks entries whose
    /// `tool_execution_end` never arrives.
    pub fn clear(&mut self) { self.open.clear(); }
}

/// Which read the shell must perform before calling `translate`. Pure.
pub fn snapshot_needed(ev: &AgentSessionEvent) -> Option<(&ToolCallId, &Path)> { /* … */ }

/// The core. Total over `AgentSessionEvent`; every arm is exhaustively matched.
pub fn translate(
    ledger: &mut ToolCallLedger,
    ev: &AgentSessionEvent,
    snapshot: Option<FileSnapshot>,
) -> Translated { /* … */ }
```

Handlers, which must never write to the wire before their own response:

```rust
/// Request handlers never receive `ConnectionTo<Client>`, so the `setTimeout(…, 0)`
/// ordering hack in `agent.ts`'s `newSession`/`loadSession` becomes a visibility rule.
pub struct HandlerOutcome<R> { pub response: R, pub follow_up: Vec<SessionUpdate> }
```

And the orchestration call site (shell):

```rust
// session_actor.rs — the ONLY place that touches the connection or the filesystem.
loop {
    tokio::select! {
        Some(ev) = events.next() => {
            let snap = match snapshot_needed(&ev) {
                Some((_, path)) => Some(read_snapshot(path).await),   // I/O lives here
                None => None,
            };
            let Translated { updates, turn } = translate(&mut ledger, &ev, snap);
            for u in updates {
                cx.send_notification(SessionNotification::new(sid.clone(), u))?;
            }
            match turn {
                TurnSignal::Settled => {
                    ledger.clear();
                    if let Some(action) = self.turn.settle(TurnOutcome::Settled) {
                        // F1: the responder is moved out here and nowhere else.
                        deliver(action);
                    }
                }
                TurnSignal::Rebind { generation } => self.rebind(generation).await,
                TurnSignal::Continue | TurnSignal::Ignored => {}
            }
        }
        Some(req) = new_session_rx.recv() => {
            let HandlerOutcome { response, follow_up } = handle_new_session(&req)?;
            req.responder.respond(response)?;             // response FIRST
            for u in follow_up {                          // then the updates
                cx.send_notification(SessionNotification::new(sid.clone(), u))?;
            }
        }
    }
}
```

The whole `handlePiEvent` switch is now testable as `translate(&mut ledger, &event, snapshot) -> Translated` with no connection, no session, no tempdir — while the two things that were genuinely hard (the file read and the send ordering) are the only things left in the shell.

---

## 5. Deliberately rejected opportunities

**Typestate for the connection lifecycle (`Uninitialized → Initialized → Authenticated → SessionActive`).** This is the single most tempting application here and it should not be built. The ACP SDK's `Builder` registers every handler before `connect_to(Stdio::new())` is awaited, and `ChainedHandler` (`tmp/agent-client-protocol-2.1.0/src/jsonrpc/handlers.rs`) dispatches to whichever link claims a message for the connection's entire life — there is no ownership path along which the compiler could withhold `session/new` until `initialize` has been answered. Worse, the Architecture phase established that an *unregistered* session-scoped method returns `Handled::No { retry: … }` and is retained and retried rather than answered, so a "state-gated" handler that declines would manifest as a hung request. The right tool is a `OnceLock<ClientView>` (capabilities, protocol version, whether `auth.terminal` was advertised) set by the `initialize` handler and read by the others, returning `Error::new(-32002, …)` when empty — which is what `agent.ts`'s `initialize` effectively wants and never actually checks. Note also that the SDK *does* ship an initialize-ordering guard, in `src/jsonrpc/protocol_compat.rs`, but only under `unstable_protocol_v2`, which the Architecture phase ruled out because it changes v1 downgrade behaviour. A runtime check is what is on the table.

**Typestate for the session lifecycle (`New → Loaded → Prompting → Deleted`).** Rejected for the reason the brief flags as load-bearing: the compiler-visible lifecycle ends long before the real one does. A cyrup session is a JSONL file on disk that outlives the process, that `cyrup_session::listing::list_all` can enumerate later, and that another `cyrup` process may be appending to. A `Session<Prompting>` type would encode about four seconds of a lifecycle measured in days, and would need `Session<Loaded>` and `Session<New>` to coexist in the same map keyed by a wire `SessionId` — which forces one concrete type and therefore a runtime tag anyway. The `Turn` enum in F1 captures the only part of this that is finite, stable, and locally owned.

**Typestate on `ToolCallStream` (`Unannounced → Announced → Closed`).** Rejected in favour of the enum in F2 for a purely mechanical reason: the streams live in a `HashMap<ToolCallId, _>` and are selected by a value that arrives off the wire, so the map needs one concrete value type. The guarantee typestate would buy — "no update before an announce" — is fully obtained by making `Body`/`Stream` private and `announce` the only constructor, at a fraction of the cost.

**A `SessionStore` mirroring `~/.pi/pi-acp/session-map.json`.** Rejected outright. `session-store.ts` exists because pi-acp is a separate process that cannot see pi's session bookkeeping; `cyrup_session::listing::{list_all, list_in_dir}` plus `layout::{SessionLayout, SessionsRoot, encode_cwd}` give cyrup-acp the same facts in-process and typed. Keeping a second durable map guarantees the reconciliation code `agent.ts`'s `findStoredSession` already has to carry (try the store, fall back to `findPiSession`, then write the store back), plus a non-atomic `writeFileSync` that loses the whole map on a crash mid-write. One source of truth; no new file format.

**Newtyping the ACP ids.** `agent_client_protocol::schema::v1::SessionId` is already `pub struct SessionId(pub Arc<str>)`, and `ToolCallId`/`TerminalId` likewise; `cyrup_core` has its own `SessionId`/`ToolCallId` from the `str_id!` macro (`crates/cyrup-core/src/lib.rs`). Adding a third wrapper to "prevent mixing" buys nothing the two existing distinct types do not already buy — they are different types and will not unify. The one place to be careful is deliberate: pi-acp reuses the tool-call id *as* the terminal id (`translate/bash.ts`'s `bashTerminalContent(toolCallId)`), which is intentional and should carry a comment, not a conversion barrier.

**A newtype for the model value id.** Rejected in favour of `cyrup_core::ModelRef` plus `cyrup_config::ModelResolver`, which `AgentSession::set_model(pattern)` already uses. Wrapping the advertised string would duplicate a parser cyrup owns. F5's `model_value_id`/`model_from_value_id` are two small functions, not a type.

**Re-deriving the pending-dialog correlation.** `cyrup_modes::rpc`'s `PendingUi { kind, reply }`, `default_ui_reply(UiKind)` and `map_ui_response` (`crates/cyrup-modes/src/rpc/mod.rs`) are exactly the shape cyrup-acp needs for `session/request_permission`, including the pruning rule (`pending.retain(|_, p| !p.reply.is_closed())`) that exists because `LiveHostServices::ui_roundtrip` races the reply against `DialogOptions.timeout`. Per house rule 4 this is not a finding — it is code to lift into `cyrup-modes` and share, which the Architecture phase already recommends and which the TUI has already duplicated once.

**Porting `translate/bash.ts` and `translate/pi-tools.ts` at all.** `bashCommand` probes twelve key paths for one command string and `toolResultToText` has a four-deep stdout fallback ladder, because both read `Record<string, unknown>` off a wire. In-process, `AgentSessionEvent::ToolExecutionStart { args: Value, .. }` comes from cyrup's own tool and `StreamEvent::ToolCallDelta { content_index, partial, .. }` is typed. There is nothing to port; a newtype over the probe result would be a wrapper around a problem that does not exist here.

**A `Validated<T>` or `Checked<T>` generic wrapper.** Rejected on naming grounds alone — the brief's rule, and the right one. `AbsCwd` and `SessionFile` say what they are; a generic marker says only that someone once looked at it.

**Typestate to reduce the size of `handlePiEvent`.** Explicitly rejected: the switch is large because ACP has many update shapes, not because its states are badly ordered. F2 splits it on the I/O seam, which is a real seam; splitting it on a fabricated state machine would add types without removing a single reachable bad state.

---

## 6. Incremental migration plan

The crate does not exist, so this is the order in which types should *land*, each stage compiling and testable on its own.

1. **`AcpFailure` + `From<AcpFailure> for agent_client_protocol::Error` (F4).** No dependencies; needed by every handler's signature. Land it first so no handler is ever written with a `String` error, and so the `maybeAuthRequiredError` shape never appears in the tree at all. Ship with a table test asserting each named `SessionServiceError` variant's classification and the byte-exact ACP message.
2. **`AbsCwd` + `SessionFile` (F3).** Also dependency-free, and required before any handler signature is fixed — retrofitting a newtype into signatures that already take `&Path` is the expensive direction. Land `AbsCwd` with the crate's first handler; land `SessionFile` when `session/delete` and `session/list` land, together with the decision to read `cyrup_session::listing` as the sole source of truth.
3. **`ToolClass` + `ToolCallLedger` + `translate()` (F2, core half), with a stub shell.** This is the largest single body of logic and the one with the most upstream behaviour to match; landing it as a pure function means it can be brought up entirely against fixture `AgentSessionEvent`s before a connection exists. `TurnSignal` is defined here but its `Settled` arm does nothing yet.
4. **`Turn` (F1) and the shell that owns `ConnectionTo<Client>`.** Now `TurnSignal::Settled` gets its consumer. The `cx.spawn` + moved-`Responder` shape and the "never propagate `Err` out of a spawned task" rule land together, since both are properties of the same function. This is the point at which the crate first serves a real `session/prompt`.
5. **`HandlerOutcome<R>` and the `session/new` / `session/load` handlers (F2, shell half).** Introduced last among the structural types because it constrains handler signatures, and by this point the handler set is known. Landing it here rather than at step 3 avoids churning signatures twice.
6. **`SessionConfigKnob` (F5).** Genuinely independent of the others and the least urgent — `session/set_config_option` and `session/set_mode` are not on the critical path to a working turn. Land with the advertise/parse round-trip test in the same commit.
7. **Only then:** `session/list`, `session/delete`, `authenticate`, and the `elicitation/create` path for `UiKind::Input`, all of which sit on types already established.

At no stage does an earlier type need to change shape for a later one; the dependency order is 1 → 2 → 3 → 4 → 5, with 6 free-floating.

---

## 7. Test implications

**Become direct tests of a parser, replacing scattered behaviour tests.** `AbsCwd::parse` absorbs what would otherwise be two or three per-handler tests asserting the `cwd must be an absolute path: …` error (pi-acp checks it at two entry points and misses a third; a single parser test plus the fact that handlers take `&AbsCwd` covers all three). `SessionFile::resolve` absorbs containment testing — but note the symlink case is a *known gap*, so write the test that documents the gap rather than one that implies it is closed. `AcpFailure::classify` becomes one table test over `SessionServiceError` variants; the `permission denied`/`403` false-positive scenario should be an explicit negative case, because that is the regression this type exists to prevent, and a test named for it is the artefact that stops someone reintroducing the sniffer.

**Remain necessary as behaviour tests, unchanged.** The `AgentSettled`-not-`AgentEnd` settle rule: `Turn` guarantees the responder is consumed once, not that it is consumed at the *right* event, so pi-acp's own scenario — an auto-retrying turn producing two `AgentEnd`s and one `AgentSettled` — must still be asserted end to end. (Upstream has exactly this test intent in `test/component/session-events.test.ts` and the `agent_end` arm's comment.) The bash output *delta* logic, the diff emission precondition, `_meta` placement for `terminal_info`/`terminal_output`/`terminal_exit`, and every byte-exact wire string stay behaviour tests: the ledger proves an update is well-formed and well-ordered, never that its content matches upstream. The `session/prompt` + `session/cancel` interleaving test the Architecture phase recommends (assert the cancel is observed *before* the prompt response) is still mandatory — it closes both `cx.spawn` traps and no type addresses either.

**Become redundant because the invalid state no longer compiles or cannot be constructed.** Tests asserting "a `tool_call_update` is never emitted before a `tool_call` for the same id" and "status never regresses to `pending`" — inside the translator module those are unreachable given `announce` is the only entry constructor and `Status` has no backward transition. Keep *one* test of each as a canary on the module's privacy boundary rather than a matrix of them. Similarly, a test asserting "the prompt responder is answered exactly once" degenerates to a test that `Turn::settle` returns `None` on the second call.

**Compile-fail tests.** Worth it for exactly one guarantee: that a request handler cannot obtain `ConnectionTo<Client>`. That is the ordering invariant from F2, it is enforced only by a signature, and it is the kind of thing a well-meaning later change ("just pass `cx` in so we can send the update inline") would undo silently at runtime. A `trybuild` case asserting `handle_new_session(&req, &cx)` does not compile is cheap and pins the intent. I would not write compile-fail tests for the newtypes — their constructors are already private and a normal unit test covers them.

**Shell-level integration tests still needed.** Everything involving I/O or an external system: the stdio bootstrap and clean-EOF exit (including the `blocking`-pool caveat that a thread parked in `read(2)` on stdin is not cancellable); the notification-after-response ordering, observed on the actual wire rather than inferred from `HandlerOutcome`; `session/list` and `session/delete` against a real sessions root, including the idempotent-delete-of-a-missing-session case ACP requires and `agent.ts`'s `deleteSession` implements; the permission round trip through a real `UiSink` and `LiveHostServices::ui_roundtrip`, specifically the fail-closed path where the ACP client never answers and the dialog must land on the timeout rather than hanging; and a handler-set completeness test enumerating registrations against `AGENT_METHOD_NAMES` (`tmp/agent-client-protocol-schema-1.7.0/src/v1/agent.rs`), because a forgotten handler manifests as a hung request, not an error, and no type in this review would catch it.

**Do not delete runtime checks yet.** The type-level guarantees here are module-scoped, not language-scoped: `AbsCwd` has no validating `Deserialize` path, `SessionFile`'s containment check is symlink-permeable, `AcpFailure`'s catch-all can under-classify, and `Turn` cannot force delivery. Each of those residual gaps is a runtime test that stays.