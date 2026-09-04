//! The host event enum (host -> guest direction), the `EventKind` discriminant, and the
//! `Subscriptions` bitset (arch-08 §3.4). `HostEvent` mirrors the func-08 §5 catalog; `EventKind`
//! indexes the 64-bit subscription bitset that gates dispatch (R-08-034 / R-ARCH-EXT-014).

use cyrup_agent::{AgentEvent, AgentMessage, ToolResultMessage};
use cyrup_core::{Content, Message, TerminateHint, ToolCallId};
use serde_json::Value;
use std::sync::Arc;

/// A C-like discriminant — one per `HostEvent` arm. Used to index the `Subscriptions` bitset and
/// (for serialization across the WIT boundary) as the `u8` the guest passes to `subscribe`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum EventKind {
    // mutating seams
    ToolCall = 0,
    ToolResult = 1,
    Context = 2,
    MessageEnd = 3,
    BeforeAgentStart = 4,
    ResourcesDiscover = 5,
    ProjectTrust = 6,
    // notify-only
    AgentStart = 7,
    AgentEnd = 8,
    TurnStart = 9,
    TurnEnd = 10,
    MessageStart = 11,
    MessageUpdate = 12, // HIGH-FREQ
    ToolExecStart = 13,
    ToolExecUpdate = 14, // HIGH-FREQ
    ToolExecEnd = 15,
    SessionStart = 16,
    SessionShutdown = 17,
    // input / provider / model — mutating + notify (pi's overload block,
    // extensions/types.ts:1190-1231 @v0.83.0)
    Input = 18,
    UserBash = 19,
    BeforeProviderRequest = 20,
    AfterProviderResponse = 21,
    ModelSelect = 22,
    ThinkingLevelSelect = 23,
    // session control lifecycle (pi's `on(event: "session_before_*")` overloads,
    // extensions/types.ts:1190-1231 @v0.83.0)
    SessionBeforeSwitch = 24,
    SessionBeforeFork = 25,
    SessionBeforeCompact = 26,
    SessionCompact = 27,
    SessionBeforeTree = 28,
    SessionTree = 29,
    /// `agent_settled` (pi `AgentSettledEvent`, extensions/types.ts:721-725 @v0.83.0; subscribed at
    /// `:1217` — EXT-073: the `:1225` this cited is `tool_execution_end`'s overload). Fired once an agent run has FULLY settled — no automatic retry, post-run
    /// compaction or queued continuation will follow. Pi emits it from the `finally` of
    /// `_runAgentPrompt` (agent-session.ts:1063-1072) via `_emitAgentSettled` (:581-588), which
    /// notifies the extension runner FIRST and the session subscribers second (SEAM-005).
    AgentSettled = 30,
    /// `before_provider_headers` (pi `BeforeProviderHeadersEvent`, extensions/types.ts:686-689
    /// @v0.83.0, subscribed at :1212, reduced by `emitBeforeProviderHeaders` at runner.ts:1045).
    /// EXT-009.
    BeforeProviderHeaders = 31,
    /// `session_info_changed` (pi `SessionInfoChangedEvent`, extensions/types.ts:571-575 @v0.83.0,
    /// subscribed at `:1193` — EXT-073: `:1203` is `session_compact`'s overload). EXT-011.
    SessionInfoChanged = 32,
}

impl EventKind {
    /// The number of distinct kinds (must stay <= 64 for the bitset).
    ///
    /// **33, and 1:1 with pi's catalog since EXT-009 and EXT-011 closed.** pi declares 33
    /// `on(event: "…")` overloads at `extensions/types.ts:1190-1231` @v0.83.0 (`:1203-1244`
    /// @v0.84.1), hand re-derived. EXT-036: the range this doc used to cite — `types.ts:1133-1171`
    /// — matches no upstream version and was fabricated, and the "1:1 with Pi's 31-event catalog"
    /// claim it carried was false while `before_provider_headers` and `session_info_changed` were
    /// missing. Enumerating pi's overload names against [`EventKind::name`] now leaves an empty
    /// set difference in both directions: no missing event and no cyrup-invented one.
    pub const COUNT: u8 = 33;

    /// Parse the `u8` a guest passes via `subscribe(event-kinds)`.
    pub fn from_u8(v: u8) -> Option<EventKind> {
        use EventKind::*;
        Some(match v {
            0 => ToolCall,
            1 => ToolResult,
            2 => Context,
            3 => MessageEnd,
            4 => BeforeAgentStart,
            5 => ResourcesDiscover,
            6 => ProjectTrust,
            7 => AgentStart,
            8 => AgentEnd,
            9 => TurnStart,
            10 => TurnEnd,
            11 => MessageStart,
            12 => MessageUpdate,
            13 => ToolExecStart,
            14 => ToolExecUpdate,
            15 => ToolExecEnd,
            16 => SessionStart,
            17 => SessionShutdown,
            18 => Input,
            19 => UserBash,
            20 => BeforeProviderRequest,
            21 => AfterProviderResponse,
            22 => ModelSelect,
            23 => ThinkingLevelSelect,
            24 => SessionBeforeSwitch,
            25 => SessionBeforeFork,
            26 => SessionBeforeCompact,
            27 => SessionCompact,
            28 => SessionBeforeTree,
            29 => SessionTree,
            30 => AgentSettled,
            31 => BeforeProviderHeaders,
            32 => SessionInfoChanged,
            _ => return None,
        })
    }

    /// The snake_case event name (pi's `on(event: "…")` overload block,
    /// `extensions/types.ts:1190-1231` @v0.83.0; used in `ExtensionError.event`).
    pub fn name(&self) -> &'static str {
        use EventKind::*;
        match self {
            ToolCall => "tool_call",
            ToolResult => "tool_result",
            Context => "context",
            MessageEnd => "message_end",
            BeforeAgentStart => "before_agent_start",
            ResourcesDiscover => "resources_discover",
            ProjectTrust => "project_trust",
            AgentStart => "agent_start",
            AgentEnd => "agent_end",
            TurnStart => "turn_start",
            TurnEnd => "turn_end",
            MessageStart => "message_start",
            MessageUpdate => "message_update",
            ToolExecStart => "tool_execution_start",
            ToolExecUpdate => "tool_execution_update",
            ToolExecEnd => "tool_execution_end",
            SessionStart => "session_start",
            SessionShutdown => "session_shutdown",
            Input => "input",
            UserBash => "user_bash",
            BeforeProviderRequest => "before_provider_request",
            AfterProviderResponse => "after_provider_response",
            ModelSelect => "model_select",
            ThinkingLevelSelect => "thinking_level_select",
            SessionBeforeSwitch => "session_before_switch",
            SessionBeforeFork => "session_before_fork",
            SessionBeforeCompact => "session_before_compact",
            SessionCompact => "session_compact",
            SessionBeforeTree => "session_before_tree",
            SessionTree => "session_tree",
            AgentSettled => "agent_settled",
            BeforeProviderHeaders => "before_provider_headers",
            SessionInfoChanged => "session_info_changed",
        }
    }

    /// Does a contained handler FAULT on this kind block the action (fail CLOSED), or degrade to
    /// no-mutation and let it proceed (fail OPEN)?
    ///
    /// This mirrors exactly which of Pi's runner emitters wrap each handler in a `try/catch`
    /// (`pi/packages/coding-agent/src/core/extensions/runner.ts`). Every emitter there catches and
    /// continues — `emit` (:810-819), `emitMessageEnd` (:845-861), `emitToolResult` (:887-907),
    /// `emitUserBash` (:963-968), `emitContext` (:993-1000), `emitBeforeProviderRequest`
    /// (:1025-1034), `emitBeforeAgentStart` (:1104-1124), `emitResourcesDiscover` (:1165-1179),
    /// `emitInput` (:1208-1222) — with ONE deliberate exception: `emitToolCall` (:932-953) has NO
    /// try/catch, so a throwing `tool_call` handler propagates out of the runner. Pi's
    /// `agent-session.ts:475-487` re-throws it as `Extension failed, blocking execution: …`, and
    /// `agent-loop.ts:616-662` turns that into an immediate error tool result — the tool is NEVER
    /// executed.
    ///
    /// `tool_call` is cyrup's permission seam (R-08-010): `cyrup-permission-system` subscribes
    /// exactly this kind, so a handler that traps, panics, OOMs, or blows the invocation budget must
    /// DENY rather than silently allow the call it was meant to gate (EXT-001).
    pub fn fails_closed(&self) -> bool {
        matches!(self, EventKind::ToolCall)
    }

    /// Map a notify-only `cyrup_agent::AgentEvent` to its kind (mutating kinds come via `Hooks`).
    ///
    /// `message_end` is deliberately absent (EXT-002). Like `tool_call`/`tool_result`/`context` it
    /// is a MUTATING seam: Pi gives it a dedicated emitter and excludes `MessageEndEvent` from the
    /// generic `emit()` union (`RunnerEmitEvent`, runner.ts:124-137), so
    /// `ExtensionRunner.emitMessageEnd` (runner.ts:835) — one implementation, one caller
    /// (agent-session.ts:752) — is the single dispatch point per finalized message. cyrup's
    /// counterpart is [`crate::ExtensionHost::emit_message_end`], driven by `SvcSubscriber`; routing
    /// it here as well would invoke every subscribed handler TWICE.
    pub fn from_agent(ev: &AgentEvent) -> Option<EventKind> {
        use EventKind::*;
        Some(match ev {
            AgentEvent::AgentStart => AgentStart,
            AgentEvent::TurnStart => TurnStart,
            AgentEvent::MessageStart { .. } => MessageStart,
            AgentEvent::MessageUpdate { .. } => MessageUpdate,
            AgentEvent::MessageEnd { .. } => return None,
            AgentEvent::ToolExecutionStart { .. } => ToolExecStart,
            AgentEvent::ToolExecutionUpdate { .. } => ToolExecUpdate,
            AgentEvent::ToolExecutionEnd { .. } => ToolExecEnd,
            AgentEvent::TurnEnd { .. } => TurnEnd,
            AgentEvent::AgentEnd { .. } => AgentEnd,
        })
    }
}

/// 64-bit subscription bitset over `EventKind` (arch-08 §3.2). An event with zero subscribers
/// never serializes or crosses the boundary — a single `bitset & kind` test (R-ARCH-EXT-014).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Subscriptions(u64);

impl Subscriptions {
    pub const fn empty() -> Self {
        Subscriptions(0)
    }

    #[must_use]
    pub fn with(mut self, kind: EventKind) -> Self {
        self.0 |= 1u64 << (kind as u8);
        self
    }

    pub fn add(&mut self, kind: EventKind) {
        self.0 |= 1u64 << (kind as u8);
    }

    pub fn contains(&self, kind: EventKind) -> bool {
        self.0 & (1u64 << (kind as u8)) != 0
    }

    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    /// Union of two bitsets (used to build the dispatcher's aggregate gate).
    #[must_use]
    pub fn union(self, other: Subscriptions) -> Subscriptions {
        Subscriptions(self.0 | other.0)
    }

    /// Build from an iterator of kinds (e.g. the guest's `subscribe` list).
    pub fn from_kinds(kinds: impl IntoIterator<Item = EventKind>) -> Self {
        let mut s = Subscriptions::empty();
        for k in kinds {
            s.add(k);
        }
        s
    }
}

/// Where a user submission originated, as delivered to an `input` handler (Pi `InputSource`,
/// extensions/types.ts:789 — `"interactive" | "rpc" | "extension"`). The richer host-side
/// provenance is collapsed onto Pi's three handler-visible values at the dispatch boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InputEventSource {
    Interactive,
    Rpc,
    Extension,
}

impl Default for InputEventSource {
    /// Pi's default when no `source` is supplied (`options?.source ?? "interactive"`,
    /// agent-session.ts:1021).
    fn default() -> Self {
        Self::Interactive
    }
}

/// How a submission delivered while the agent is streaming will be queued (Pi
/// `streamingBehavior`, extensions/types.ts:801 — `"steer" | "followUp"`). `None` on the
/// [`HostEvent::Input`] event means the agent is idle (Pi passes `undefined` when not streaming,
/// agent-session.ts:1022).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InputStreamingBehavior {
    Steer,
    FollowUp,
}

/// The host event (host -> guest). One arm per func-08 §5 event; payload = minimum-spec record.
/// Open-shaped fields carry `serde_json::Value`; fixed-shape ids/roles are typed (arch-08 §3.4).
#[derive(Clone, Debug)]
pub enum HostEvent {
    // 5.4 tools — mutating seams
    ToolCall {
        call_id: ToolCallId,
        name: String,
        input: Value,
    },
    ToolResult {
        call_id: ToolCallId,
        name: String,
        /// The executed tool's arguments (Pi `ToolResultEventBase.input`, types.ts:886).
        input: Value,
        content: Vec<Content>,
        details: Option<Value>,
        is_error: bool,
        /// Usage the tool execution itself reported (Pi `ToolResultEventBase.usage`,
        /// types.ts:919-921, upstream `2fd38684`). `None` = absent, which is every ordinary tool.
        /// Observable by a handler and patchable via [`crate::EventPatch::ToolResult::usage`].
        usage: Option<cyrup_core::Usage>,
        /// The tool's early-termination hint (pi `AgentToolResult.terminate?`). Host-side only:
        /// the WIT `on-tool-result` call has a fixed signature and does not carry it, so a guest
        /// cannot OBSERVE it — but a guest CAN set it through
        /// [`crate::EventPatch::ToolResult::terminate`], and this field is what that patch lands
        /// on, exactly as pi's `afterResult.terminate ?? result.terminate` does not require the
        /// hook to have seen the original.
        terminate: TerminateHint,
    },
    // 5.3 agent & turn — mutating + notify
    Context {
        messages: Vec<Arc<AgentMessage>>,
    },
    MessageEnd {
        message: Message,
    },
    /// `before_agent_start` (Pi types.ts:665): the user prompt + images + the (chainable)
    /// system prompt and its options. A handler may inject a message and/or replace the prompt;
    /// injected messages ACCUMULATE across handlers (Pi `runner.ts:980` `messages.push`).
    BeforeAgentStart {
        prompt: String,
        images: Value,
        system_prompt: String,
        options: Value,
        /// Messages injected by handlers so far (folded across the chain, R-08-011).
        injected: Vec<Message>,
    },
    AgentStart,
    AgentEnd {
        messages: Vec<Arc<AgentMessage>>,
    },
    /// `turn_start` (Pi `TurnStartEvent`, types.ts:688-693): the turn index AND a wall-clock
    /// `timestamp` (Pi `Date.now()`, agent-session.ts:624). `turn_index` is derived in the
    /// `ExtSubscriber` fan-out layer (mirroring Pi's `AgentSession._turnIndex`), not on the raw event.
    TurnStart {
        turn_index: u32,
        timestamp: u64,
    },
    TurnEnd {
        turn_index: u32,
        message: AgentMessage,
        tool_results: Vec<ToolResultMessage>,
    },
    /// `message_start` (Pi `MessageStartEvent`, types.ts:711-715): the full message (user|assistant|
    /// toolResult), serialized — not just its role.
    MessageStart {
        message: Value,
    },
    /// `message_update` (Pi `MessageUpdateEvent`, types.ts:717-722): the in-flight `message` AND the
    /// `assistantMessageEvent` provider delta (carried as `delta`).
    MessageUpdate {
        message: Value,
        delta: Value,
    },
    ToolExecStart {
        call_id: ToolCallId,
        name: String,
        args: Value,
    },
    /// `tool_execution_update` (pi `ToolExecutionUpdateEvent`, extensions/types.ts:770-776
    /// @v0.83.0): `{type, toolCallId, toolName, args, partialResult}`. `name` and `args` were
    /// dropped here while the arm directly above kept both, so an observer that missed
    /// `tool_execution_start` — late registration, reload, or a run already in flight — could not
    /// filter by tool at all (EXT-014).
    ToolExecUpdate {
        call_id: ToolCallId,
        name: String,
        args: Value,
        chunk: Value,
    },
    /// `tool_execution_end` (pi `ToolExecutionEndEvent`, extensions/types.ts:779-785 @v0.83.0):
    /// `{type, toolCallId, toolName, result, isError}` (EXT-014).
    ToolExecEnd {
        call_id: ToolCallId,
        name: String,
        result: Value,
        is_error: bool,
    },
    // 5.1/5.2 startup & session
    /// `session_start` (pi `SessionStartEvent`, extensions/types.ts:562-569 @v0.83.0): `reason`
    /// (`"startup"|"reload"|"new"|"resume"|"fork"`) and `previousSessionFile?`, "Present for
    /// \"new\", \"resume\", and \"fork\"" (EXT-015).
    SessionStart {
        reason: String,
        previous_session_file: Option<String>,
    },
    /// `session_shutdown` (pi `SessionShutdownEvent`, extensions/types.ts:616-621 @v0.83.0):
    /// `reason` and `targetSessionFile?`, "Destination session file when shutting down due to
    /// session replacement" (EXT-015).
    SessionShutdown {
        reason: String,
        target_session_file: Option<String>,
    },
    /// `session_info_changed` (pi `SessionInfoChangedEvent`, extensions/types.ts:571-575
    /// @v0.83.0): "Current normalized session name. Undefined when the name is cleared" — so
    /// `None` is upstream's `undefined`, not an empty name (EXT-011).
    SessionInfoChanged {
        name: Option<String>,
    },
    /// `resources_discover` (pi `ResourcesDiscoverEvent`, extensions/types.ts:544-548 @v0.83.0):
    /// `{type, cwd, reason: "startup" | "reload"}` (EXT-016).
    ResourcesDiscover {
        cwd: String,
        reason: String,
    },
    /// `project_trust` (pi `ProjectTrustEvent`, extensions/types.ts:519-522 @v0.83.0):
    /// `{type, cwd}`. The verdict is per-DIRECTORY upstream — the store is keyed by cwd
    /// (`options.trustStore.set(options.cwd, trusted)`, core/project-trust.ts:63-65) — so without
    /// this a trust-policy extension cannot key an allowlist or honour `remember` (EXT-043).
    ProjectTrust {
        cwd: String,
    },
    // 5.5 input / 5.6 provider / model (pi's overload block, extensions/types.ts:1190-1231 @v0.83.0)
    // Carries the submission text AND the attached images (Pi `InputEvent.text`/`.images`,
    // types.ts:792-802) so an `input` handler can `transform` either (Pi runner.ts:1116-1119),
    // PLUS the `source` (Pi `InputEvent.source`, types.ts:799) and the in-flight `streaming_behavior`
    // (Pi `InputEvent.streamingBehavior`, types.ts:801, `undefined` when idle) so a handler can
    // branch on interactive-vs-queued / steer-vs-follow-up before deciding (Pi runner.ts:1108-1114).
    Input {
        text: String,
        images: Vec<Content>,
        source: InputEventSource,
        streaming_behavior: Option<InputStreamingBehavior>,
    },
    /// `user_bash` (Pi `UserBashEvent`, types.ts:782-790): the `command`, the `exclude_from_context`
    /// flag (true for the `!!` prefix), and the `cwd`. The `operations`/`result` override is returned
    /// via the `handled` outcome (Pi `UserBashEventResult`), not carried inbound.
    UserBash {
        command: String,
        exclude_from_context: bool,
        cwd: String,
    },
    BeforeProviderRequest {
        payload: Value,
    },
    /// `before_provider_headers` (pi `BeforeProviderHeadersEvent`, extensions/types.ts:686-689
    /// @v0.83.0). `headers` is the assembled header bag. Upstream's doc (:681-685) is exact:
    /// "Handlers mutate `headers` in place … A `null` value deletes that header", so the patch is
    /// an object whose values are `string | null` and a `null` DELETES rather than blanks
    /// (EXT-009).
    BeforeProviderHeaders {
        headers: Value,
    },
    AfterProviderResponse {
        status: u32,
        headers: Value,
    },
    /// `model_select` (pi `ModelSelectEvent`, extensions/types.ts:794-799 @v0.83.0): `model`,
    /// `previousModel` and `source` are THREE SIBLING fields. cyrup used to nest the latter two
    /// inside `model`, so a ported handler read `event.previousModel` and got `undefined` while
    /// `event.model` was not a `Model` shape either (EXT-042).
    ModelSelect {
        model: Value,
        previous_model: Option<Value>,
        source: String,
    },
    /// `thinking_level_select` (pi `ThinkingLevelSelectEvent`, extensions/types.ts:802-806
    /// @v0.83.0): `{level, previousLevel}`. `previousLevel` is not optional upstream; `None` here
    /// is the first event of a session, where there is genuinely no prior level (EXT-042).
    ThinkingLevelSelect {
        level: String,
        previous_level: Option<String>,
    },
    // session control lifecycle (pi's overload block, extensions/types.ts:1190-1231 @v0.83.0)
    /// `session_before_switch` (pi `SessionBeforeSwitchEvent`, extensions/types.ts:578-582
    /// @v0.83.0): `reason: "new" | "resume"` and `targetSessionFile?`. cyrup carried a bare
    /// `target_id` and dropped `reason` — the field that distinguishes the two cases a handler
    /// most needs to tell apart (EXT-015).
    SessionBeforeSwitch {
        reason: String,
        target_session_file: Option<String>,
    },
    /// `session_before_fork` (pi `SessionBeforeForkEvent`, extensions/types.ts:585-589 @v0.83.0):
    /// `entryId` and `position: "before" | "at"` (EXT-015).
    SessionBeforeFork {
        entry_id: String,
        position: String,
    },
    /// `session_before_compact` (Pi `SessionBeforeCompactEvent`, types.ts:577-587): the computed
    /// `preparation` (`CompactionPreparation`, whose `messagesToSummarize`/`turnPrefixMessages`
    /// carry RAW `AgentMessage`s with their roles intact), the `branch_entries` in scope, optional
    /// `custom_instructions`, the trigger `reason` (`"manual"|"threshold"|"overflow"`), and
    /// `will_retry`. A handler may veto (`block`) or return a compaction override via `mutate` — the
    /// folded override lands in `override_result` (Pi `SessionBeforeCompactResult.compaction`).
    SessionBeforeCompact {
        preparation: Value,
        branch_entries: Value,
        custom_instructions: Option<String>,
        reason: String,
        will_retry: bool,
        /// The guest's compaction override, folded from a `mutate` outcome (`None` = no override).
        override_result: Option<Value>,
    },
    /// `session_compact` (Pi `SessionCompactEvent`, types.ts:589-598): the produced compaction entry
    /// (its `summary` carries the text), whether an extension drove it, the trigger reason, retry flag.
    SessionCompact {
        compaction_entry: Value,
        from_extension: bool,
        reason: String,
        will_retry: bool,
    },
    /// `session_before_tree` (Pi `SessionBeforeTreeEvent`, types.ts:623-628): the computed
    /// `preparation` (`TreePreparation`). A handler may veto (`block`) or return a
    /// summary/customInstructions/label override via `mutate` (folded into `override_result`).
    SessionBeforeTree {
        preparation: Value,
        /// The guest's tree override, folded from a `mutate` outcome (`None` = no override).
        override_result: Option<Value>,
    },
    SessionTree {
        tree: Value,
    },
    /// `agent_settled` (Pi `AgentSettledEvent`, extensions/types.ts:721-725) — a payload-free
    /// notification that the whole run, including every automatic continuation, has settled.
    ///
    /// Deliberately absent from [`HostEvent::from_agent`]: it has NO `AgentEvent` source. It is
    /// SYNTHESISED by `cyrup-session-svc` at the post-run driver's tail (the point that corresponds
    /// to Pi's `_runAgentPrompt` `finally`), which is the only place that knows the retry /
    /// compaction / queued-continuation loop is done. Routing it through the `ExtSubscriber` seam
    /// would fire it once per `agent_end` instead of once per run.
    AgentSettled,
}

impl HostEvent {
    pub fn kind(&self) -> EventKind {
        use EventKind as K;
        match self {
            HostEvent::ToolCall { .. } => K::ToolCall,
            HostEvent::ToolResult { .. } => K::ToolResult,
            HostEvent::Context { .. } => K::Context,
            HostEvent::MessageEnd { .. } => K::MessageEnd,
            HostEvent::BeforeAgentStart { .. } => K::BeforeAgentStart,
            HostEvent::AgentStart => K::AgentStart,
            HostEvent::AgentEnd { .. } => K::AgentEnd,
            HostEvent::TurnStart { .. } => K::TurnStart,
            HostEvent::TurnEnd { .. } => K::TurnEnd,
            HostEvent::MessageStart { .. } => K::MessageStart,
            HostEvent::MessageUpdate { .. } => K::MessageUpdate,
            HostEvent::ToolExecStart { .. } => K::ToolExecStart,
            HostEvent::ToolExecUpdate { .. } => K::ToolExecUpdate,
            HostEvent::ToolExecEnd { .. } => K::ToolExecEnd,
            HostEvent::SessionStart { .. } => K::SessionStart,
            HostEvent::SessionShutdown { .. } => K::SessionShutdown,
            HostEvent::SessionInfoChanged { .. } => K::SessionInfoChanged,
            HostEvent::ResourcesDiscover { .. } => K::ResourcesDiscover,
            HostEvent::ProjectTrust { .. } => K::ProjectTrust,
            HostEvent::Input { .. } => K::Input,
            HostEvent::UserBash { .. } => K::UserBash,
            HostEvent::BeforeProviderRequest { .. } => K::BeforeProviderRequest,
            HostEvent::BeforeProviderHeaders { .. } => K::BeforeProviderHeaders,
            HostEvent::AfterProviderResponse { .. } => K::AfterProviderResponse,
            HostEvent::ModelSelect { .. } => K::ModelSelect,
            HostEvent::ThinkingLevelSelect { .. } => K::ThinkingLevelSelect,
            HostEvent::SessionBeforeSwitch { .. } => K::SessionBeforeSwitch,
            HostEvent::SessionBeforeFork { .. } => K::SessionBeforeFork,
            HostEvent::SessionBeforeCompact { .. } => K::SessionBeforeCompact,
            HostEvent::SessionCompact { .. } => K::SessionCompact,
            HostEvent::SessionBeforeTree { .. } => K::SessionBeforeTree,
            HostEvent::SessionTree { .. } => K::SessionTree,
            HostEvent::AgentSettled => K::AgentSettled,
        }
    }

    /// Build a notify-only `HostEvent` from an `AgentEvent` (the `ExtSubscriber` seam, arch-08 §5.4).
    /// Mutating events (`ToolCall`/`ToolResult`/`Context`) are produced by the `Hooks` seam instead,
    /// and `MessageEnd` by [`crate::ExtensionHost::emit_message_end`] — see
    /// [`EventKind::from_agent`] (EXT-002).
    pub fn from_agent(ev: &AgentEvent) -> Option<HostEvent> {
        Some(match ev {
            AgentEvent::AgentStart => HostEvent::AgentStart,
            // `turn_index` is a placeholder here; the `ExtSubscriber` fan-out layer overwrites it
            // with the derived counter value (Pi `AgentSession._turnIndex`). `timestamp` is the
            // wall-clock at emit (Pi `Date.now()`, agent-session.ts:624).
            AgentEvent::TurnStart => HostEvent::TurnStart {
                turn_index: 0,
                timestamp: now_millis(),
            },
            AgentEvent::MessageStart { message } => HostEvent::MessageStart {
                message: serde_json::to_value(message).unwrap_or(Value::Null),
            },
            AgentEvent::MessageUpdate {
                message,
                assistant_message_event,
            } => HostEvent::MessageUpdate {
                message: serde_json::to_value(message).unwrap_or(Value::Null),
                delta: serde_json::to_value(assistant_message_event).unwrap_or(Value::Null),
            },
            // Mutating seam — dispatched exactly once by `ExtensionHost::emit_message_end`
            // (EXT-002). Never routed through the notify subscriber.
            AgentEvent::MessageEnd { .. } => return None,
            AgentEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => HostEvent::ToolExecStart {
                call_id: tool_call_id.clone(),
                name: tool_name.clone(),
                args: args.clone(),
            },
            // EXT-014: `tool_name` and `args` are on the `AgentEvent` and were being discarded
            // by the `..` — pi carries both through to the handler
            // (`ToolExecutionUpdateEvent`, extensions/types.ts:770-776 @v0.83.0).
            AgentEvent::ToolExecutionUpdate {
                tool_call_id,
                tool_name,
                args,
                partial_result,
            } => HostEvent::ToolExecUpdate {
                call_id: tool_call_id.clone(),
                name: tool_name.clone(),
                args: args.clone(),
                chunk: partial_result.clone(),
            },
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error,
            } => HostEvent::ToolExecEnd {
                call_id: tool_call_id.clone(),
                name: tool_name.clone(),
                result: result.clone(),
                is_error: *is_error,
            },
            AgentEvent::TurnEnd {
                message,
                tool_results,
            } => HostEvent::TurnEnd {
                turn_index: 0,
                message: message.clone(),
                tool_results: tool_results.clone(),
            },
            AgentEvent::AgentEnd { messages } => HostEvent::AgentEnd {
                messages: messages.clone(),
            },
        })
    }
}

/// Wall-clock milliseconds since the Unix epoch (Pi `Date.now()`). A clock before the epoch
/// degrades to `0` (never a panic).
pub(crate) fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// NOTE (EXT-002): the former `to_llm_message` helper lived here solely to build the notify-path
// `HostEvent::MessageEnd`. That path is gone — `message_end` reaches extensions exactly once, via
// `ExtensionHost::emit_message_end`, whose `cyrup_core::Message` is converted by
// `cyrup-session-svc`'s `agent_message_to_core`.
