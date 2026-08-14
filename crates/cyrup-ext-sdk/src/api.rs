//! The ergonomic guest API (arch-08 §3.6) — the Rust analog of Pi's `ExtensionAPI` (the `pi` object
//! an extension factory receives, types.ts:1128-1356). An author subscribes to any of the 30 events
//! with a typed handler `(event, &Ctx) -> Outcome`, registers tools/commands/shortcuts/flags/
//! providers/renderers/autocomplete, and the SDK lowers all of it onto the `cyrup:ext` WIT world.
//!
//! Handlers are stored uniformly as `Fn(&[&str], &Ctx) -> RawOutcome`; the typed `on_*` setters wrap
//! a typed closure into that uniform shape, and `subscription_kinds()` reports exactly the events
//! that have a handler (driving the host subscription bitset, R-ARCH-EXT-014).

use crate::autocomplete::{AutocompleteProvider, AutocompleteQuery, AutocompleteSuggestions};
use crate::ctx::{CommandCtx, Ctx, ToolCall};
use crate::descriptor::{CommandDescriptor, FlagSpec, ProviderConfig, ToolDescriptor};
use crate::events::*;
use crate::provider::{OAuthCallbacks, OAuthCredentials, ProviderHandlers, ProviderStream};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

// Event-kind discriminants — kept in lockstep with the host `EventKind` (cyrup-ext/src/event.rs).
mod kind {
    pub const TOOL_CALL: u8 = 0;
    pub const TOOL_RESULT: u8 = 1;
    pub const CONTEXT: u8 = 2;
    pub const MESSAGE_END: u8 = 3;
    pub const BEFORE_AGENT_START: u8 = 4;
    pub const RESOURCES_DISCOVER: u8 = 5;
    pub const PROJECT_TRUST: u8 = 6;
    pub const AGENT_START: u8 = 7;
    pub const AGENT_END: u8 = 8;
    pub const TURN_START: u8 = 9;
    pub const TURN_END: u8 = 10;
    pub const MESSAGE_START: u8 = 11;
    pub const MESSAGE_UPDATE: u8 = 12;
    pub const TOOL_EXEC_START: u8 = 13;
    pub const TOOL_EXEC_UPDATE: u8 = 14;
    pub const TOOL_EXEC_END: u8 = 15;
    pub const SESSION_START: u8 = 16;
    pub const SESSION_SHUTDOWN: u8 = 17;
    pub const INPUT: u8 = 18;
    pub const USER_BASH: u8 = 19;
    pub const BEFORE_PROVIDER_REQUEST: u8 = 20;
    pub const AFTER_PROVIDER_RESPONSE: u8 = 21;
    pub const MODEL_SELECT: u8 = 22;
    pub const THINKING_LEVEL_SELECT: u8 = 23;
    pub const SESSION_BEFORE_SWITCH: u8 = 24;
    pub const SESSION_BEFORE_FORK: u8 = 25;
    pub const SESSION_BEFORE_COMPACT: u8 = 26;
    pub const SESSION_COMPACT: u8 = 27;
    pub const SESSION_BEFORE_TREE: u8 = 28;
    pub const SESSION_TREE: u8 = 29;
    /// `agent_settled` (Pi `AgentSettledEvent`, extensions/types.ts:721-725; subscribed at
    /// types.ts:1225) — the run has FULLY settled: no retry, post-run compaction or queued
    /// continuation will follow (SEAM-005).
    pub const AGENT_SETTLED: u8 = 30;
    /// `before_provider_headers` (pi `extensions/types.ts:686-689` @v0.83.0, subscribed at
    /// `:1212`) — EXT-009.
    pub const BEFORE_PROVIDER_HEADERS: u8 = 31;
    /// `session_info_changed` (pi `extensions/types.ts:571-575` @v0.83.0, subscribed at `:1203`)
    /// — EXT-011.
    pub const SESSION_INFO_CHANGED: u8 = 32;
}

/// The block/mutate/notify contribution a handler returns (mirrors the host `HookOutcome`). The
/// open `mutate`/`handled` payloads are interpreted host-side per the event kind (the §3.3 reducer).
#[derive(Clone, Debug)]
pub enum Outcome {
    /// notify-only / no change.
    Noop,
    /// Short-circuit the action with an optional reason (first block wins host-side), plus pi's
    /// `terminate` hint (EXT-049; `extensions/types.ts:1072-1079` @v0.84.1: "Hint that the agent
    /// should stop after the current tool batch when this call is blocked. Early termination only
    /// happens when every finalized tool result in the batch sets this to true"). `terminate` is
    /// read only on `tool_call`; build it with [`Outcome::block_and_terminate`].
    Block(Option<String>, bool),
    /// Replace the in-flight value with this event-specific JSON patch.
    Mutate(Value),
    /// The extension fully serviced the action (`input`/`user_bash`/`resources_discover`).
    Handled(Value),
}

impl Outcome {
    pub fn noop() -> Self {
        Outcome::Noop
    }
    pub fn block(reason: impl Into<String>) -> Self {
        Outcome::Block(Some(reason.into()), false)
    }
    pub fn block_silent() -> Self {
        Outcome::Block(None, false)
    }
    /// Block AND hint that the agent should stop after this tool batch (EXT-049; pi
    /// `ToolCallEventResult.terminate`, `extensions/types.ts:1072-1079` @v0.84.1). The agent
    /// applies upstream's every()-rule — the run ends only if EVERY finalized result in the batch
    /// set it (`shouldTerminateToolBatch`, `packages/agent/src/agent-loop.ts:583`) — so one
    /// blocking handler setting this does not end the run on its own.
    pub fn block_and_terminate(reason: impl Into<String>) -> Self {
        Outcome::Block(Some(reason.into()), true)
    }
    /// A raw event-specific mutate patch.
    pub fn mutate(v: impl Serialize) -> Self {
        Outcome::Mutate(serde_json::to_value(v).unwrap_or(Value::Null))
    }
    /// A fully-serviced result.
    pub fn handled(v: impl Serialize) -> Self {
        Outcome::Handled(serde_json::to_value(v).unwrap_or(Value::Null))
    }

    // --- typed mutate helpers (per-event shapes) ---

    /// `tool_call`: rewrite the tool input (R-08-010).
    pub fn replace_tool_input(input: impl Serialize) -> Self {
        Outcome::mutate(input)
    }
    /// `tool_result`: replace-not-merge override of result fields (R-08-011).
    pub fn patch_tool_result(patch: ToolResultPatch) -> Self {
        Outcome::mutate(patch)
    }
    /// `context`: filter/replace the message list.
    pub fn replace_messages(messages: impl Serialize) -> Self {
        Outcome::mutate(messages)
    }
    /// `message_end`: replace the message (same role enforced host-side).
    pub fn replace_message(message: impl Serialize) -> Self {
        Outcome::mutate(message)
    }
    /// `before_agent_start`: inject a message and/or replace the system prompt.
    pub fn before_agent_start(result: BeforeAgentStartResult) -> Self {
        Outcome::mutate(result)
    }
    /// `session_before_compact`: supply a compaction override (Pi `SessionBeforeCompactResult.compaction`).
    /// The override's summary/details land in the appended compaction entry (`fromExtension`).
    pub fn compaction_override(result: SessionBeforeCompactResult) -> Self {
        Outcome::mutate(result)
    }
    /// `session_before_tree`: supply a summary/customInstructions/label override (Pi
    /// `SessionBeforeTreeResult`).
    pub fn tree_override(result: SessionBeforeTreeResult) -> Self {
        Outcome::mutate(result)
    }

    pub(crate) fn into_raw(self) -> RawOutcome {
        match self {
            Outcome::Noop => RawOutcome::Noop,
            Outcome::Block(r, t) => RawOutcome::Block(r, t),
            Outcome::Mutate(v) => RawOutcome::Mutate(v.to_string()),
            Outcome::Handled(v) => RawOutcome::Handled(v.to_string()),
        }
    }
}

/// The lowered outcome the guest export glue converts into the WIT `hook-outcome`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RawOutcome {
    Noop,
    /// `{reason, terminate}` — pi `ToolCallEventResult` (`extensions/types.ts:1072-1079`
    /// @v0.84.1). `terminate` is read only on `tool_call` (EXT-049).
    Block(Option<String>, bool),
    Mutate(String),
    Handled(String),
}

// --- tool execution (Pi `ToolDefinition.execute`, types.ts:464; R-08-015) ---

/// A (text|image) content block in a tool result. Serializes 1:1 with `cyrup_core::Content`.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ContentBlock {
    Text { text: String },
    Image { data: String, mime_type: String },
}

impl ContentBlock {
    pub fn text(t: impl Into<String>) -> Self {
        ContentBlock::Text { text: t.into() }
    }
}

/// The result of executing a guest-registered tool (Pi `AgentToolResult`, types.ts:1043).
#[derive(Clone, Debug, Default)]
pub struct ToolOutput {
    pub content: Vec<ContentBlock>,
    pub details: Option<Value>,
    pub is_error: bool,
    /// End the agent loop after this result (Pi `terminate`, R-08-015).
    pub terminate: bool,
}

impl ToolOutput {
    /// A plain successful text result.
    pub fn text(t: impl Into<String>) -> Self {
        Self { content: vec![ContentBlock::text(t)], ..Self::default() }
    }
    /// An error result (surfaced to the model as `isError`).
    pub fn error(t: impl Into<String>) -> Self {
        Self { content: vec![ContentBlock::text(t)], is_error: true, ..Self::default() }
    }
    pub fn with_details(mut self, d: impl Serialize) -> Self {
        self.details = serde_json::to_value(d).ok();
        self
    }
    pub fn terminating(mut self) -> Self {
        self.terminate = true;
        self
    }
}

/// A tool implementation supplied by the guest author. Mirrors Pi's streaming `execute`: the
/// [`ToolCall`] carries the `toolCallId`, parsed `params`, and a `Ctx` with `emit_update` (onUpdate)
/// and the capability surface. Cancellation (Pi `signal`) is enforced host-side via the epoch.
pub trait ToolExec: 'static {
    fn execute(&self, call: ToolCall) -> Result<ToolOutput, String>;

    /// Coerce raw tool-call arguments BEFORE schema validation — pi
    /// `ToolDefinition.prepareArguments?: (args: unknown) => Static<TParams>`
    /// (`extensions/types.ts:468` @v0.83.0, run before `validateToolArguments` in
    /// `packages/agent/src/agent-loop.ts`). EXT-023.
    ///
    /// Returning `None` (the default) is upstream's ABSENT `prepareArguments` — the identity. A
    /// tool that overrides this must ALSO set `ToolDescriptor::prepare_arguments`, because that
    /// flag is what tells the host to call the export at all; the descriptor field used to be
    /// accepted, documented and silently discarded at the boundary.
    fn prepare_arguments(&self, _args: &Value) -> Option<Value> {
        None
    }
}

impl<F> ToolExec for F
where
    F: Fn(ToolCall) -> Result<ToolOutput, String> + 'static,
{
    fn execute(&self, call: ToolCall) -> Result<ToolOutput, String> {
        (self)(call)
    }
}

/// A registered tool: its descriptor + its executor.
pub struct RegisteredTool {
    pub descriptor: ToolDescriptor,
    pub exec: Box<dyn ToolExec>,
}

// --- command execution (Pi `RegisteredCommand.handler`, types.ts:1105-1111; R-08-016) ---

/// A slash-command body supplied by the guest author. Runs at COMMAND tier (the [`CommandCtx`]
/// exposes the session-control ops); `args` is the raw argument string; returns optional text.
pub trait CommandExec: 'static {
    fn execute(&self, args: &str, ctx: &CommandCtx) -> Result<Option<String>, String>;
}

impl<F> CommandExec for F
where
    F: Fn(&str, &CommandCtx) -> Result<Option<String>, String> + 'static,
{
    fn execute(&self, args: &str, ctx: &CommandCtx) -> Result<Option<String>, String> {
        (self)(args, ctx)
    }
}

/// A dynamic argument completer (Pi `getArgumentCompletions(prefix)`, types.ts:1108).
pub type ArgCompleter = Box<dyn Fn(&str) -> Vec<String> + 'static>;

/// A registered slash command: its descriptor, its handler, and an optional dynamic argument
/// completer.
pub struct RegisteredCommand {
    pub descriptor: CommandDescriptor,
    pub handler: Box<dyn CommandExec>,
    pub completions: Option<ArgCompleter>,
}

// --- keyboard shortcuts (Pi `registerShortcut`, types.ts:1198-1205; R-08-017) ---

/// A keyboard-shortcut body supplied by the guest author (Pi `options.handler(ctx)`, types.ts:1203).
/// Invoked across the `execute-shortcut` export when the registered `KeyId` fires; receives the base
/// [`Ctx`] (Pi hands the shortcut handler the general `ExtensionContext`).
pub trait ShortcutExec: 'static {
    fn execute(&self, ctx: &Ctx) -> Result<(), String>;
}

impl<F> ShortcutExec for F
where
    F: Fn(&Ctx) -> Result<(), String> + 'static,
{
    fn execute(&self, ctx: &Ctx) -> Result<(), String> {
        (self)(ctx)
    }
}

/// A registered keyboard shortcut: its `key` (Pi `KeyId`), optional `description` (surfaced to the
/// host for display), and its `handler`. The key+description cross the seam via the
/// `registration.register-shortcut` import; the handler stays guest-side and runs via
/// `execute-shortcut` (so a registered shortcut is no longer structurally inert).
pub struct RegisteredShortcut {
    pub key: String,
    pub description: String,
    pub handler: Box<dyn ShortcutExec>,
}

// --- message renderers (Pi `renderCall`/`renderResult`, types.ts:472-481; R-08-020) ---

/// A custom message renderer the guest registers for a `custom_type`. Each method returns a
/// serialized widget tree (`Value`), or `None` to fall back to the runtime's default renderer.
pub trait MessageRenderer: 'static {
    fn render_call(&self, _call: &Value, _ctx: &Ctx) -> Option<Value> {
        None
    }
    fn render_result(&self, _result: &Value, _ctx: &Ctx) -> Option<Value> {
        None
    }
}

/// A registered renderer keyed by its `custom_type`.
pub struct RegisteredRenderer {
    pub custom_type: String,
    pub renderer: Box<dyn MessageRenderer>,
}

/// A uniform handler: parses the ordered string args the host passes and returns a [`RawOutcome`].
type Handler = Box<dyn Fn(&[&str], &Ctx) -> RawOutcome + 'static>;

/// The collected registrations + handlers an extension declares in its factory (arch-08 §3.6).
#[derive(Default)]
pub struct ExtensionApi {
    handlers: HashMap<u8, Handler>,
    pub(crate) tools: Vec<RegisteredTool>,
    pub(crate) commands: Vec<(String, RegisteredCommand)>,
    pub(crate) shortcuts: Vec<RegisteredShortcut>,
    pub(crate) flags: Vec<(String, FlagSpec)>,
    pub(crate) providers: Vec<(String, ProviderConfig)>,
    /// Dynamic provider callbacks (OAuth + `streamSimple`) keyed by provider id — the non-serializable
    /// half of `registerProvider` (sdk gap #1). Invoked across the `provider-*` exports.
    pub(crate) provider_handlers: HashMap<String, ProviderHandlers>,
    pub(crate) renderers: Vec<RegisteredRenderer>,
    /// Custom-ENTRY renderers (Pi `registerEntryRenderer`, types.ts:1295). A SEPARATE list from
    /// [`Self::renderers`], mirroring upstream's disjoint `messageRenderers`/`entryRenderers` maps
    /// (types.ts:1703-1704); on the wire an entry still travels over `render-call`.
    pub(crate) entry_renderers: Vec<RegisteredRenderer>,
    pub(crate) autocomplete: Vec<String>,
    /// Stacked global autocomplete providers (Pi `addAutocompleteProvider`, sdk gap #2). Folded in
    /// registration order over the host's built-in suggestions by [`Self::autocomplete_suggest`].
    pub(crate) autocomplete_providers: Vec<Box<dyn AutocompleteProvider>>,
    /// Inter-extension event-bus subscriptions (Pi `pi.events.on(channel, handler)`,
    /// event-bus.ts:18; gap-08 §5.3). Each is a `(topic, handler)`; the topic is declared to the
    /// host via the `bus.subscribe` import so a matching `bus.emit` from ANY extension is fanned out
    /// to this guest's `bus-deliver` export, which routes to [`Self::dispatch_bus`].
    pub(crate) bus_subscriptions: Vec<(String, BusHandler)>,
}

/// A bus subscription handler: receives the emitted topic + its JSON payload + a [`Ctx`] (Pi
/// `handler(data)`, event-bus.ts:18). The topic is passed too so one handler can serve several
/// closely-related channels if the author registers it under each.
type BusHandler = Box<dyn Fn(&str, Value, &Ctx) + 'static>;

/// Read an ordered arg without panicking on a short slice (clippy no-indexing).
fn arg<'a>(args: &'a [&'a str], i: usize) -> &'a str {
    args.get(i).copied().unwrap_or("")
}

/// Parse a JSON arg, degrading to `Null` (never a panic).
fn json(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or(Value::Null)
}

/// Parse an OPTIONAL JSON arg: an empty string is Pi `undefined` (`None`); anything else parses
/// (degrading to `Null`). Used for `option<string>` seam params (e.g. `tool_result.details`).
fn opt_json(s: &str) -> Option<Value> {
    if s.is_empty() {
        None
    } else {
        Some(json(s))
    }
}

/// Parse an OPTIONAL string arg: empty = Pi `undefined` (`None`). Used for `streamingBehavior`.
fn opt_str(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// Parse the `images` arg into Pi's optional shape: an empty string or an empty array `[]` is Pi
/// `undefined` (`None`); a non-empty array parses to `Some(array)` (Pi `InputEvent.images?`).
fn opt_images(s: &str) -> Option<Value> {
    match opt_json(s) {
        Some(Value::Array(a)) if a.is_empty() => None,
        other => other,
    }
}

impl ExtensionApi {
    pub fn new() -> Self {
        Self::default()
    }

    // --- registration ---

    /// Register a tool (overrides a built-in of the same name host-side, R-08-012).
    pub fn register_tool(&mut self, descriptor: ToolDescriptor, exec: impl ToolExec) {
        self.tools.push(RegisteredTool { descriptor, exec: Box::new(exec) });
    }

    /// Register a pre-built tool (Pi `defineTool` output / `customTools` array entry, sdk gap #6).
    pub fn register_tool_def(&mut self, tool: RegisteredTool) {
        self.tools.push(tool);
    }

    /// Register a slash command (R-08-016). The handler runs at command tier (session ops allowed).
    /// Mirrors Pi's `registerCommand(name, {description, handler})` (types.ts:1105).
    pub fn register_command(
        &mut self,
        name: impl Into<String>,
        desc: CommandDescriptor,
        handler: impl CommandExec,
    ) {
        self.commands.push((
            name.into(),
            RegisteredCommand { descriptor: desc, handler: Box::new(handler), completions: None },
        ));
    }

    /// Register a slash command with a dynamic argument completer (Pi `getArgumentCompletions`,
    /// types.ts:1108): the completer is called with the current argument prefix.
    pub fn register_command_with_completions(
        &mut self,
        name: impl Into<String>,
        desc: CommandDescriptor,
        handler: impl CommandExec,
        completions: impl Fn(&str) -> Vec<String> + 'static,
    ) {
        self.commands.push((
            name.into(),
            RegisteredCommand {
                descriptor: desc,
                handler: Box::new(handler),
                completions: Some(Box::new(completions)),
            },
        ));
    }

    /// Register a keyboard shortcut with a handler (R-08-017). Mirrors Pi's
    /// `registerShortcut(key, {description, handler})` (types.ts:1198-1205): the `key`+`description`
    /// cross the seam for display, the `handler` is stored guest-side and runs via the
    /// `execute-shortcut` export when the `KeyId` fires.
    pub fn register_shortcut(
        &mut self,
        key: impl Into<String>,
        description: impl Into<String>,
        handler: impl ShortcutExec,
    ) {
        self.shortcuts.push(RegisteredShortcut {
            key: key.into(),
            description: description.into(),
            handler: Box::new(handler),
        });
    }

    /// Execute a registered shortcut's handler by key (R-08-017). The `execute-shortcut` export
    /// routes here when the host reports the matching `KeyId` fired. Returns an error for an unknown
    /// key (never a panic).
    pub fn execute_shortcut(&self, key: &str, ctx: &Ctx) -> Result<(), String> {
        match self.shortcuts.iter().find(|s| s.key == key) {
            Some(s) => s.handler.execute(ctx),
            None => Err(format!("no such shortcut: {key}")),
        }
    }

    /// Register a CLI flag (R-08-018).
    pub fn register_flag(&mut self, name: impl Into<String>, spec: FlagSpec) {
        self.flags.push((name.into(), spec));
    }

    /// Register a custom LLM provider with a static config (R-08-019).
    pub fn register_provider(&mut self, id: impl Into<String>, config: ProviderConfig) {
        self.providers.push((id.into(), config));
    }

    /// Register a custom LLM provider with dynamic OAuth + `streamSimple` callbacks (Pi
    /// `registerProvider({oauth, streamSimple})`, types.ts:1337; sdk gap #1). The static `config`
    /// crosses the seam to register models; the [`ProviderHandlers`] stay guest-side and are invoked
    /// across the `provider-*` exports. The config's `oauth`/`hasStreamSimple` markers are auto-filled
    /// from the handlers so the host knows which callbacks exist.
    pub fn register_provider_with_handlers(
        &mut self,
        id: impl Into<String>,
        mut config: ProviderConfig,
        handlers: ProviderHandlers,
    ) {
        let id = id.into();
        if let Some(oauth) = &handlers.oauth {
            config.oauth = Some(serde_json::json!({ "name": oauth.name }));
        }
        config.has_stream_simple = handlers.has_stream_simple();
        self.providers.push((id.clone(), config));
        self.provider_handlers.insert(id, handlers);
    }

    /// Register a custom message renderer (R-08-020). The renderer's `render_call`/`render_result`
    /// are invoked across the boundary when a tool of `custom_type` is displayed (Pi types.ts:472).
    pub fn register_message_renderer(
        &mut self,
        custom_type: impl Into<String>,
        renderer: impl MessageRenderer,
    ) {
        self.renderers.push(RegisteredRenderer {
            custom_type: custom_type.into(),
            renderer: Box::new(renderer),
        });
    }

    /// Register a custom ENTRY renderer (Pi `pi.registerEntryRenderer(customType, renderer)`,
    /// types.ts:1295) — the TUI-only surface for entries appended with `append_entry`, which do NOT
    /// participate in LLM context.
    ///
    /// The renderer is invoked through [`MessageRenderer::render_call`], since an entry crosses the
    /// boundary on the world's `render-call` export (there is no `render-entry`; see the world's
    /// `register-entry-renderer` comment). A renderer that PANICS here draws upstream's
    /// `[type] renderer failed: …` box (`custom-entry.ts:47-52`) rather than being silently
    /// dropped — the one surface where a renderer fault is user-visible.
    pub fn register_entry_renderer(
        &mut self,
        custom_type: impl Into<String>,
        renderer: impl MessageRenderer,
    ) {
        self.entry_renderers.push(RegisteredRenderer {
            custom_type: custom_type.into(),
            renderer: Box::new(renderer),
        });
    }

    /// Add an autocomplete provider for a command (R-08-021).
    pub fn add_autocomplete(&mut self, command: impl Into<String>) {
        self.autocomplete.push(command.into());
    }

    /// Stack a global autocomplete provider on top of the current one (Pi `addAutocompleteProvider`,
    /// types.ts:218; sdk gap #2). Providers are folded in registration order: each sees the wrapped
    /// ("current") provider's suggestions and may augment or replace them.
    pub fn add_autocomplete_provider(&mut self, provider: impl AutocompleteProvider) {
        self.autocomplete_providers.push(Box::new(provider));
    }

    /// Subscribe to an inter-extension event-bus topic (Pi `pi.events.on(channel, handler)`,
    /// event-bus.ts:18; gap-08 §5.3). The `handler` runs whenever ANY loaded extension — this one
    /// included, matching Pi's EventEmitter — emits `topic` via [`Ctx::emit`]; it receives the topic
    /// and the emitted JSON payload. The topic is declared to the host (the `bus.subscribe` import)
    /// so the host knows to fan a matching emit out to this guest's `bus-deliver` export.
    pub fn on_bus(&mut self, topic: impl Into<String>, handler: impl Fn(&str, Value, &Ctx) + 'static) {
        self.bus_subscriptions.push((topic.into(), Box::new(handler)));
    }

    // --- the 30 event subscriptions (Pi `pi.on`, types.ts:1133-1171) ---

    pub fn on_tool_call(&mut self, f: impl Fn(ToolCallEvent, &Ctx) -> Outcome + 'static) {
        self.handlers.insert(kind::TOOL_CALL, Box::new(move |a, c| {
            let ev = ToolCallEvent { call_id: arg(a, 0).into(), name: arg(a, 1).into(), input: json(arg(a, 2)) };
            f(ev, c).into_raw()
        }));
    }
    pub fn on_tool_result(&mut self, f: impl Fn(ToolResultEvent, &Ctx) -> Outcome + 'static) {
        self.handlers.insert(kind::TOOL_RESULT, Box::new(move |a, c| {
            let ev = ToolResultEvent {
                call_id: arg(a, 0).into(),
                name: arg(a, 1).into(),
                input: json(arg(a, 2)),
                content: json(arg(a, 3)),
                is_error: arg(a, 4) == "true",
                details: opt_json(arg(a, 5)),
                // Pi `ToolResultEventBase.usage` (types.ts:919-921); empty arg = Pi `undefined`.
                usage: opt_json(arg(a, 6)),
            };
            f(ev, c).into_raw()
        }));
    }
    pub fn on_context(&mut self, f: impl Fn(ContextEvent, &Ctx) -> Outcome + 'static) {
        self.handlers.insert(kind::CONTEXT, Box::new(move |a, c| {
            f(ContextEvent { messages: json(arg(a, 0)) }, c).into_raw()
        }));
    }
    pub fn on_message_end(&mut self, f: impl Fn(MessageEndEvent, &Ctx) -> Outcome + 'static) {
        self.handlers.insert(kind::MESSAGE_END, Box::new(move |a, c| {
            f(MessageEndEvent { message: json(arg(a, 0)) }, c).into_raw()
        }));
    }
    pub fn on_before_agent_start(&mut self, f: impl Fn(BeforeAgentStartEvent, &Ctx) -> Outcome + 'static) {
        self.handlers.insert(kind::BEFORE_AGENT_START, Box::new(move |a, c| {
            let ev = BeforeAgentStartEvent {
                prompt: arg(a, 0).into(),
                images: json(arg(a, 1)),
                system_prompt: arg(a, 2).into(),
                options: json(arg(a, 3)),
            };
            f(ev, c).into_raw()
        }));
    }
    pub fn on_input(&mut self, f: impl Fn(InputEvent, &Ctx) -> Outcome + 'static) {
        self.handlers.insert(kind::INPUT, Box::new(move |a, c| {
            let ev = InputEvent {
                text: arg(a, 0).into(),
                images: opt_images(arg(a, 1)),
                source: arg(a, 2).into(),
                streaming_behavior: opt_str(arg(a, 3)),
            };
            f(ev, c).into_raw()
        }));
    }
    pub fn on_user_bash(&mut self, f: impl Fn(UserBashEvent, &Ctx) -> Outcome + 'static) {
        self.handlers.insert(kind::USER_BASH, Box::new(move |a, c| {
            let ev = UserBashEvent {
                command: arg(a, 0).into(),
                exclude_from_context: arg(a, 1) == "true",
                cwd: arg(a, 2).into(),
            };
            f(ev, c).into_raw()
        }));
    }
    pub fn on_before_provider_request(&mut self, f: impl Fn(BeforeProviderRequestEvent, &Ctx) -> Outcome + 'static) {
        self.handlers.insert(kind::BEFORE_PROVIDER_REQUEST, Box::new(move |a, c| {
            f(BeforeProviderRequestEvent { payload: json(arg(a, 0)) }, c).into_raw()
        }));
    }
    /// `before_provider_headers` (EXT-009). Return the header patch via [`Outcome::mutate`]; a key
    /// mapped to `null` DELETES that header (pi `extensions/types.ts:681-685` @v0.83.0).
    pub fn on_before_provider_headers(
        &mut self,
        f: impl Fn(BeforeProviderHeadersEvent, &Ctx) -> Outcome + 'static,
    ) {
        self.handlers.insert(kind::BEFORE_PROVIDER_HEADERS, Box::new(move |a, c| {
            f(BeforeProviderHeadersEvent { headers: json(arg(a, 0)) }, c).into_raw()
        }));
    }
    /// `session_info_changed` (EXT-011) — notify-only.
    pub fn on_session_info_changed(&mut self, f: impl Fn(SessionInfoChangedEvent, &Ctx) + 'static) {
        self.handlers.insert(
            kind::SESSION_INFO_CHANGED,
            notify(move |a, c| f(SessionInfoChangedEvent { name: opt_str(arg(a, 0)) }, c)),
        );
    }
    pub fn on_resources_discover(
        &mut self,
        f: impl Fn(ResourcesDiscoverEvent, &Ctx) -> Outcome + 'static,
    ) {
        // EXT-016: `cwd` + `reason` (pi extensions/types.ts:544-548 @v0.83.0) — a
        // resource-contributing extension could not tell which directory it was discovering for,
        // nor startup from `/reload`, so it could not scope or cache its contribution.
        self.handlers.insert(kind::RESOURCES_DISCOVER, Box::new(move |a, c| {
            f(ResourcesDiscoverEvent { cwd: arg(a, 0).into(), reason: arg(a, 1).into() }, c)
                .into_raw()
        }));
    }
    pub fn on_project_trust(&mut self, f: impl Fn(ProjectTrustEvent, &Ctx) -> Outcome + 'static) {
        // EXT-043: `cwd` (pi extensions/types.ts:519-522 @v0.83.0) — the key the trust store is
        // keyed by, so `remember` has a well-defined meaning from the handler's point of view.
        self.handlers.insert(kind::PROJECT_TRUST, Box::new(move |a, c| {
            f(ProjectTrustEvent { cwd: arg(a, 0).into() }, c).into_raw()
        }));
    }
    pub fn on_session_before_switch(&mut self, f: impl Fn(SessionBeforeSwitchEvent, &Ctx) -> Outcome + 'static) {
        self.handlers.insert(kind::SESSION_BEFORE_SWITCH, Box::new(move |a, c| {
            f(
                SessionBeforeSwitchEvent {
                    reason: arg(a, 0).into(),
                    target_session_file: opt_str(arg(a, 1)),
                },
                c,
            )
            .into_raw()
        }));
    }
    pub fn on_session_before_fork(&mut self, f: impl Fn(SessionBeforeForkEvent, &Ctx) -> Outcome + 'static) {
        self.handlers.insert(kind::SESSION_BEFORE_FORK, Box::new(move |a, c| {
            f(
                SessionBeforeForkEvent {
                    entry_id: arg(a, 0).into(),
                    position: arg(a, 1).into(),
                },
                c,
            )
            .into_raw()
        }));
    }
    pub fn on_session_before_compact(&mut self, f: impl Fn(SessionBeforeCompactEvent, &Ctx) -> Outcome + 'static) {
        self.handlers.insert(kind::SESSION_BEFORE_COMPACT, Box::new(move |a, c| {
            let ev = SessionBeforeCompactEvent {
                preparation: json(arg(a, 0)),
                branch_entries: json(arg(a, 1)),
                custom_instructions: opt_str(arg(a, 2)),
                reason: arg(a, 3).into(),
                will_retry: arg(a, 4) == "true",
            };
            f(ev, c).into_raw()
        }));
    }
    pub fn on_session_before_tree(&mut self, f: impl Fn(SessionBeforeTreeEvent, &Ctx) -> Outcome + 'static) {
        self.handlers.insert(kind::SESSION_BEFORE_TREE, Box::new(move |a, c| {
            f(SessionBeforeTreeEvent { preparation: json(arg(a, 0)) }, c).into_raw()
        }));
    }

    // --- notify-only subscriptions (return ignored) ---

    pub fn on_agent_start(&mut self, f: impl Fn(&Ctx) + 'static) {
        self.handlers.insert(kind::AGENT_START, notify(move |_a, c| f(c)));
    }
    pub fn on_agent_end(&mut self, f: impl Fn(AgentEndEvent, &Ctx) + 'static) {
        self.handlers.insert(kind::AGENT_END, notify(move |a, c| f(AgentEndEvent { messages: json(arg(a, 0)) }, c)));
    }
    /// Pi `on("agent_settled", handler)` (extensions/types.ts:1225). Fires ONCE per run, after every
    /// automatic retry / post-run compaction / queued continuation has finished — unlike
    /// [`Self::on_agent_end`], which fires once per `agent.prompt`/`agent.continue`.
    pub fn on_agent_settled(&mut self, f: impl Fn(&Ctx) + 'static) {
        self.handlers.insert(kind::AGENT_SETTLED, notify(move |_a, c| f(c)));
    }
    pub fn on_turn_start(&mut self, f: impl Fn(TurnStartEvent, &Ctx) + 'static) {
        self.handlers.insert(kind::TURN_START, notify(move |a, c| {
            f(TurnStartEvent { turn_index: arg(a, 0).parse().unwrap_or(0), timestamp: arg(a, 1).parse().unwrap_or(0) }, c)
        }));
    }
    pub fn on_turn_end(&mut self, f: impl Fn(TurnEndEvent, &Ctx) + 'static) {
        self.handlers.insert(kind::TURN_END, notify(move |a, c| {
            f(
                TurnEndEvent {
                    turn_index: arg(a, 0).parse().unwrap_or(0),
                    message: json(arg(a, 1)),
                    tool_results: json(arg(a, 2)),
                },
                c,
            )
        }));
    }
    pub fn on_message_start(&mut self, f: impl Fn(MessageStartEvent, &Ctx) + 'static) {
        self.handlers.insert(kind::MESSAGE_START, notify(move |a, c| f(MessageStartEvent { message: json(arg(a, 0)) }, c)));
    }
    pub fn on_message_update(&mut self, f: impl Fn(MessageUpdateEvent, &Ctx) + 'static) {
        self.handlers.insert(kind::MESSAGE_UPDATE, notify(move |a, c| {
            f(MessageUpdateEvent { message: json(arg(a, 0)), assistant_message_event: json(arg(a, 1)) }, c)
        }));
    }
    pub fn on_tool_exec_start(&mut self, f: impl Fn(ToolExecStartEvent, &Ctx) + 'static) {
        self.handlers.insert(kind::TOOL_EXEC_START, notify(move |a, c| {
            f(ToolExecStartEvent { call_id: arg(a, 0).into(), name: arg(a, 1).into(), args: json(arg(a, 2)) }, c)
        }));
    }
    pub fn on_tool_exec_update(&mut self, f: impl Fn(ToolExecUpdateEvent, &Ctx) + 'static) {
        self.handlers.insert(kind::TOOL_EXEC_UPDATE, notify(move |a, c| {
            f(
                ToolExecUpdateEvent {
                    call_id: arg(a, 0).into(),
                    name: arg(a, 1).into(),
                    args: json(arg(a, 2)),
                    chunk: json(arg(a, 3)),
                },
                c,
            )
        }));
    }
    pub fn on_tool_exec_end(&mut self, f: impl Fn(ToolExecEndEvent, &Ctx) + 'static) {
        self.handlers.insert(kind::TOOL_EXEC_END, notify(move |a, c| {
            f(
                ToolExecEndEvent {
                    call_id: arg(a, 0).into(),
                    name: arg(a, 1).into(),
                    result: json(arg(a, 2)),
                    is_error: arg(a, 3) == "true",
                },
                c,
            )
        }));
    }
    pub fn on_session_start(&mut self, f: impl Fn(SessionLifecycleEvent, &Ctx) + 'static) {
        self.handlers.insert(
            kind::SESSION_START,
            notify(move |a, c| {
                f(
                    SessionLifecycleEvent {
                        reason: arg(a, 0).into(),
                        session_file: opt_str(arg(a, 1)),
                    },
                    c,
                )
            }),
        );
    }
    pub fn on_session_shutdown(&mut self, f: impl Fn(SessionLifecycleEvent, &Ctx) + 'static) {
        self.handlers.insert(
            kind::SESSION_SHUTDOWN,
            notify(move |a, c| {
                f(
                    SessionLifecycleEvent {
                        reason: arg(a, 0).into(),
                        session_file: opt_str(arg(a, 1)),
                    },
                    c,
                )
            }),
        );
    }
    pub fn on_after_provider_response(&mut self, f: impl Fn(AfterProviderResponseEvent, &Ctx) + 'static) {
        self.handlers.insert(kind::AFTER_PROVIDER_RESPONSE, notify(move |a, c| {
            f(AfterProviderResponseEvent { status: arg(a, 0).parse().unwrap_or(0), headers: json(arg(a, 1)) }, c)
        }));
    }
    pub fn on_model_select(&mut self, f: impl Fn(ModelSelectEvent, &Ctx) + 'static) {
        self.handlers.insert(
            kind::MODEL_SELECT,
            notify(move |a, c| {
                f(
                    ModelSelectEvent {
                        model: json(arg(a, 0)),
                        previous_model: opt_json(arg(a, 1)),
                        source: arg(a, 2).into(),
                    },
                    c,
                )
            }),
        );
    }
    pub fn on_thinking_level_select(&mut self, f: impl Fn(ThinkingLevelSelectEvent, &Ctx) + 'static) {
        self.handlers.insert(
            kind::THINKING_LEVEL_SELECT,
            notify(move |a, c| {
                f(
                    ThinkingLevelSelectEvent {
                        level: arg(a, 0).into(),
                        previous_level: opt_str(arg(a, 1)),
                    },
                    c,
                )
            }),
        );
    }
    pub fn on_session_compact(&mut self, f: impl Fn(SessionCompactEvent, &Ctx) + 'static) {
        // The host seam supplies the full Pi shape: the produced compaction entry, whether an
        // extension drove it, the trigger reason, and the retry flag (L4 gap #5, wired through the
        // cyrup-session-svc producer).
        self.handlers.insert(kind::SESSION_COMPACT, notify(move |a, c| {
            let ev = SessionCompactEvent {
                compaction_entry: json(arg(a, 0)),
                from_extension: arg(a, 1) == "true",
                reason: arg(a, 2).into(),
                will_retry: arg(a, 3) == "true",
            };
            f(ev, c)
        }));
    }
    pub fn on_session_tree(&mut self, f: impl Fn(SessionTreeEvent, &Ctx) + 'static) {
        self.handlers.insert(kind::SESSION_TREE, notify(move |a, c| f(SessionTreeEvent { tree: json(arg(a, 0)) }, c)));
    }

    // --- dispatch + subscription bitset ---

    /// Run the handler for `kind` (if any) with the ordered string `args` and `ctx`.
    pub fn dispatch(&self, kind: u8, args: &[&str], ctx: &Ctx) -> RawOutcome {
        match self.handlers.get(&kind) {
            Some(h) => h(args, ctx),
            None => RawOutcome::Noop,
        }
    }

    /// Execute a guest-registered tool by name (R-08-015).
    pub fn execute_tool(&self, name: &str, call: ToolCall) -> Result<ToolOutput, String> {
        match self.tools.iter().find(|t| t.descriptor.name == name) {
            Some(t) => t.exec.execute(call),
            None => Err(format!("no such tool: {name}")),
        }
    }

    /// Run a registered tool's `prepareArguments` shim (EXT-023). `None` when the tool does not
    /// exist or declares no shim — either way the host leaves the arguments untouched, which is
    /// upstream's identity default.
    pub fn prepare_tool_arguments(&self, name: &str, args: &Value) -> Option<Value> {
        self.tools
            .iter()
            .find(|t| t.descriptor.name == name)
            .and_then(|t| t.exec.prepare_arguments(args))
    }

    /// Execute a guest-registered slash command by name (R-08-016). Runs the handler with a
    /// command-tier [`CommandCtx`] and the raw `args` string; returns its optional text output.
    pub fn execute_command(&self, name: &str, args: &str) -> Result<Option<String>, String> {
        match self.commands.iter().find(|(n, _)| n == name) {
            Some((_, cmd)) => cmd.handler.execute(args, &CommandCtx::new()),
            None => Err(format!("no such command: {name}")),
        }
    }

    /// Dynamic argument completions for a command (Pi `getArgumentCompletions(prefix)`). Falls back
    /// to the descriptor's static `completions` filtered by `prefix` when no dynamic completer is set.
    pub fn argument_completions(&self, name: &str, prefix: &str) -> Vec<String> {
        match self.commands.iter().find(|(n, _)| n == name) {
            Some((_, cmd)) => match &cmd.completions {
                Some(f) => f(prefix),
                None => cmd
                    .descriptor
                    .completions
                    .iter()
                    .filter(|c| c.starts_with(prefix))
                    .cloned()
                    .collect(),
            },
            None => Vec::new(),
        }
    }

    /// Render a tool call via a registered renderer for `custom_type` (Pi `renderCall`). Returns the
    /// serialized widget tree (`None` = default renderer).
    /// A custom-ENTRY renderer is searched too, and LAST: the message table is the one the host
    /// routes tool rows and custom messages through, and it must keep winning a key it already
    /// claims. An entry-only type falls through to the entry table.
    pub fn render_call(&self, custom_type: &str, call: &Value) -> Option<Value> {
        self.renderers
            .iter()
            .chain(self.entry_renderers.iter())
            .find(|r| r.custom_type == custom_type)
            .and_then(|r| r.renderer.render_call(call, &Ctx::new()))
    }

    /// Render a tool result via a registered renderer for `custom_type` (Pi `renderResult`).
    pub fn render_result(&self, custom_type: &str, result: &Value) -> Option<Value> {
        self.renderers
            .iter()
            .find(|r| r.custom_type == custom_type)
            .and_then(|r| r.renderer.render_result(result, &Ctx::new()))
    }

    // --- provider OAuth + streamSimple callbacks (Pi types.ts:1380-1392; sdk gap #1) ---

    fn oauth_of(&self, id: &str) -> Result<&crate::provider::OAuthProvider, String> {
        self.provider_handlers
            .get(id)
            .and_then(|h| h.oauth.as_ref())
            .ok_or_else(|| format!("provider `{id}` has no OAuth handler"))
    }

    /// Run a provider's `login(callbacks)` flow; returns the credentials JSON to persist.
    pub fn provider_login(&self, id: &str) -> Result<OAuthCredentials, String> {
        (self.oauth_of(id)?.login)(&OAuthCallbacks::new())
    }

    /// Refresh a provider's expired credentials (Pi `refreshToken`).
    pub fn provider_refresh_token(
        &self,
        id: &str,
        credentials: OAuthCredentials,
    ) -> Result<OAuthCredentials, String> {
        (self.oauth_of(id)?.refresh_token)(credentials)
    }

    /// Derive the API key string from a provider's credentials (Pi `getApiKey`).
    pub fn provider_get_api_key(
        &self,
        id: &str,
        credentials: &OAuthCredentials,
    ) -> Result<String, String> {
        (self.oauth_of(id)?.get_api_key)(credentials)
    }

    /// Rewrite a provider's models given its credentials (Pi optional `modifyModels`). Returns the
    /// models unchanged when the provider supplies no `modifyModels` closure.
    pub fn provider_modify_models(
        &self,
        id: &str,
        models: Value,
        credentials: &OAuthCredentials,
    ) -> Result<Value, String> {
        match &self.oauth_of(id)?.modify_models {
            Some(f) => f(models, credentials),
            None => Ok(models),
        }
    }

    /// Run a provider's custom `streamSimple`: it pushes events into `stream` then returns (sdk #1).
    pub fn provider_stream_simple(
        &self,
        id: &str,
        stream: &ProviderStream,
        model: Value,
        context: Value,
        options: Value,
    ) -> Result<(), String> {
        let handler = self
            .provider_handlers
            .get(id)
            .and_then(|h| h.stream_simple.as_ref())
            .ok_or_else(|| format!("provider `{id}` has no streamSimple handler"))?;
        handler.stream(model, context, options, stream)
    }

    /// Fold the stacked autocomplete providers over the host's built-in `base` suggestions (Pi the
    /// `AutocompleteProviderFactory` chain, types.ts:117; sdk gap #2). Each provider sees the previous
    /// ("current") result; `None` defers to it.
    pub fn autocomplete_suggest(
        &self,
        base: Option<AutocompleteSuggestions>,
        query: &AutocompleteQuery,
    ) -> Option<AutocompleteSuggestions> {
        let mut current = base;
        for provider in &self.autocomplete_providers {
            if let Some(next) = provider.suggest(query, current.as_ref()) {
                current = Some(next);
            }
        }
        current
    }

    /// The number of stacked autocomplete providers (drives the host `add-autocomplete-provider`).
    pub fn autocomplete_provider_count(&self) -> usize {
        self.autocomplete_providers.len()
    }

    /// The `u8` event-kind list this extension subscribed to, for the host `subscribe` import
    /// (R-ARCH-EXT-014). Exactly the kinds with a registered handler.
    pub fn subscription_kinds(&self) -> Vec<u8> {
        let mut v: Vec<u8> = self.handlers.keys().copied().collect();
        v.sort_unstable();
        v
    }

    /// The DISTINCT inter-extension bus topics this extension subscribed to, for the host
    /// `bus.subscribe` import (gap-08 §5.3). Order-preserving, deduplicated.
    pub fn bus_topics(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for (t, _) in &self.bus_subscriptions {
            if !v.iter().any(|x| x == t) {
                v.push(t.clone());
            }
        }
        v
    }

    /// Deliver a bus event to every matching subscription handler (Pi EventEmitter fan-out to all
    /// listeners registered on the channel, event-bus.ts). Called by the `bus-deliver` export when
    /// the host routes an emit from any extension to this guest.
    pub fn dispatch_bus(&self, topic: &str, payload: Value, ctx: &Ctx) {
        for (t, h) in &self.bus_subscriptions {
            if t == topic {
                h(topic, payload.clone(), ctx);
            }
        }
    }

    pub fn tools(&self) -> &[RegisteredTool] {
        &self.tools
    }

    /// The registered providers' `(id, static config)` pairs (the serializable half that crosses the
    /// seam; OAuth/`streamSimple` closures live in `provider_handlers`).
    pub fn providers(&self) -> &[(String, ProviderConfig)] {
        &self.providers
    }
}

/// Wrap a notify (`-> ()`) closure into the uniform handler shape (returns `Noop`).
fn notify(f: impl Fn(&[&str], &Ctx) + 'static) -> Handler {
    Box::new(move |a, c| {
        f(a, c);
        RawOutcome::Noop
    })
}
