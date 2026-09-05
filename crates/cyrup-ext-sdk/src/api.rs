//! The ergonomic guest API (arch-08 §3.6) — the Rust analog of Pi's `ExtensionAPI` (the `pi` object
//! an extension factory receives, types.ts:1185-1420 @v0.83.0; EXT-072 corrected `:1128-1356`). An
//! author subscribes to any of the 33 events
//! with a typed handler `(event, &Ctx) -> Outcome`, registers tools/commands/shortcuts/flags/
//! providers/renderers/autocomplete, and the SDK lowers all of it onto the `cyrup:ext` WIT world.
//!
//! Handlers are stored uniformly as `Fn(&[&str], &Ctx) -> RawOutcome`; the typed `on_*` setters wrap
//! a typed closure into that uniform shape, and `subscription_kinds()` reports exactly the events
//! that have a handler (driving the host subscription bitset, R-ARCH-EXT-014).

use crate::autocomplete::{AutocompleteProvider, AutocompleteQuery, AutocompleteSuggestions};
use crate::ctx::{BashCommand, CommandCtx, Ctx, ToolCall};
use crate::descriptor::{CommandDescriptor, FlagSpec, ProviderConfig, ToolDescriptor};
use crate::events::*;
use crate::provider::{OAuthCallbacks, OAuthCredentials, ProviderHandlers, ProviderStream};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

// Event-kind discriminants. Three hand-maintained copies carry this numbering — the host's
// `EventKind` (cyrup-ext/src/event.rs), this table, and the literals `export_extension!` passes to
// `guest::hook`/`guest::notify` (src/macros.rs) — and the discriminant crosses the WIT boundary as
// a bare `u8`, so no compiler compares any pair of them. Two tests do, one per leg:
// `src/tests/world_import_coverage.rs` checks this table against `src/macros.rs`, and
// `cyrup-ext/src/tests/event_kind_lockstep.rs` checks it against `EventKind::from_u8`. Renumbering
// any one copy on its own turns at least one of them red.
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
    /// `agent_settled` (pi `AgentSettledEvent`, extensions/types.ts:721-725 @v0.83.0; subscribed at
    /// `:1217` — EXT-073: the `:1225` this cited is `tool_execution_end`'s overload) — the run has
    /// FULLY settled: no retry, post-run compaction or queued
    /// continuation will follow (SEAM-005).
    pub const AGENT_SETTLED: u8 = 30;
    /// `before_provider_headers` (pi `extensions/types.ts:686-689` @v0.83.0, subscribed at
    /// `:1212`) — EXT-009.
    pub const BEFORE_PROVIDER_HEADERS: u8 = 31;
    /// `session_info_changed` (pi `extensions/types.ts:571-575` @v0.83.0, subscribed at `:1193` —
    /// EXT-073: `:1203` is `session_compact`)
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
    /// The no-contribution outcome ([`Outcome::Noop`]): no block, no mutate, nothing handled.
    pub fn noop() -> Self {
        Outcome::Noop
    }
    /// Short-circuit the action with a reason (first block wins host-side), WITHOUT pi's
    /// `terminate` hint — for that, use [`Outcome::block_and_terminate`].
    pub fn block(reason: impl Into<String>) -> Self {
        Outcome::Block(Some(reason.into()), false)
    }
    /// Short-circuit the action with NO reason, so the host has no text to surface. Carries no
    /// `terminate` hint.
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
    ///
    /// **On an encode failure this returns [`Outcome::Noop`], NOT a mutate.** `v` is author-supplied
    /// and `serde_json` encoding is genuinely fallible for author types (a map with a non-string
    /// key, a `#[serde(flatten)]` over a non-map, a hand-written `Serialize` that returns `Err`).
    /// `Noop` is the only substitute that cannot corrupt the event: an encoded `null` would reach
    /// the host as a real mutate patch and, on `tool_call`, replace the tool's arguments with
    /// `null`. Use [`Outcome::try_mutate`] when the author wants to see the error.
    pub fn mutate(v: impl Serialize) -> Self {
        Outcome::try_mutate(v).unwrap_or(Outcome::Noop)
    }
    /// [`Outcome::mutate`], with the encode failure surfaced instead of degraded to
    /// [`Outcome::Noop`].
    pub fn try_mutate(v: impl Serialize) -> Result<Self, String> {
        match serde_json::to_value(v) {
            Ok(v) => Ok(Outcome::Mutate(v)),
            Err(e) => Err(format!("Outcome::mutate: {e}")),
        }
    }
    /// A fully-serviced result.
    ///
    /// **On an encode failure this returns [`Outcome::Noop`], NOT a handled result** — see
    /// [`Outcome::mutate`] for why the substitute is `Noop`. Use [`Outcome::try_handled`] when the
    /// author wants to see the error.
    pub fn handled(v: impl Serialize) -> Self {
        Outcome::try_handled(v).unwrap_or(Outcome::Noop)
    }
    /// [`Outcome::handled`], with the encode failure surfaced instead of degraded to
    /// [`Outcome::Noop`].
    pub fn try_handled(v: impl Serialize) -> Result<Self, String> {
        match serde_json::to_value(v) {
            Ok(v) => Ok(Outcome::Handled(v)),
            Err(e) => Err(format!("Outcome::handled: {e}")),
        }
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
    /// [`Outcome::Noop`] lowered: the handler changed nothing.
    Noop,
    /// `{reason, terminate}` — pi `ToolCallEventResult` (`extensions/types.ts:1072-1079`
    /// @v0.84.1). `terminate` is read only on `tool_call` (EXT-049).
    Block(Option<String>, bool),
    /// [`Outcome::Mutate`] lowered — the patch already serialized to JSON by `Outcome::into_raw`.
    Mutate(String),
    /// [`Outcome::Handled`] lowered — likewise already serialized.
    Handled(String),
}

// --- tool execution (Pi `ToolDefinition.execute`, types.ts:480; R-08-015) ---

/// A (text|image) content block in a tool result. Serializes 1:1 with `cyrup_core::Content`.
#[derive(Clone, Debug, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ContentBlock {
    /// A text block; built by [`Self::text`].
    Text {
        /// The block's text.
        text: String,
    },
    /// An image block.
    Image {
        /// The image bytes, base64-encoded (as in `cyrup_core::Content::Image`).
        data: String,
        /// The image's MIME type; `mimeType` on the wire.
        mime_type: String,
    },
}

impl ContentBlock {
    /// A `text` content block.
    pub fn text(t: impl Into<String>) -> Self {
        ContentBlock::Text { text: t.into() }
    }
}

/// The result of executing a guest-registered tool (Pi `AgentToolResult`, `packages/agent/src/types.ts:355-369` @v0.83.0 — a DIFFERENT package; EXT-036 corrected `extensions/types.ts:1043`, which is a member of the `ExtensionEvent` union).
#[derive(Clone, Debug, Default)]
pub struct ToolOutput {
    /// The result blocks the model sees. [`Self::text`] and [`Self::error`] build a single-text
    /// one.
    pub content: Vec<ContentBlock>,
    /// The per-tool structured details blob — app/extension metadata, NOT sent to the model
    /// (`cyrup_core::ToolResult`'s own rule). Set with [`Self::with_details`].
    pub details: Option<Value>,
    /// Whether this result is a failure (Pi `isError`); set by [`Self::error`].
    pub is_error: bool,
    /// End the agent loop after this result (Pi `terminate`, R-08-015).
    pub terminate: bool,
}

impl ToolOutput {
    /// A plain successful text result.
    pub fn text(t: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::text(t)],
            ..Self::default()
        }
    }
    /// An error result (surfaced to the model as `isError`).
    pub fn error(t: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::text(t)],
            is_error: true,
            ..Self::default()
        }
    }
    /// Attach the per-tool `details` blob. A value that fails to serialize leaves `details`
    /// unset rather than failing the tool.
    #[must_use]
    pub fn with_details(mut self, d: impl Serialize) -> Self {
        self.details = serde_json::to_value(d).ok();
        self
    }
    /// Set [`ToolOutput::terminate`]: end the agent loop after this result (Pi `terminate`,
    /// R-08-015).
    #[must_use]
    pub fn terminating(mut self) -> Self {
        self.terminate = true;
        self
    }
}

/// A tool implementation supplied by the guest author. Mirrors Pi's streaming `execute`: the
/// [`ToolCall`] carries the `toolCallId`, parsed `params`, and a `Ctx` with `emit_update` (onUpdate)
/// and the capability surface. Cancellation (Pi `signal`) is enforced host-side via the epoch.
pub trait ToolExec: 'static {
    /// Run the tool for one [`ToolCall`]. `Err` is the tool-level failure the host reports; a
    /// model-visible error is `Ok(`[`ToolOutput::error`]`)` instead.
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
    /// What crosses the seam to the host at registration.
    pub descriptor: ToolDescriptor,
    /// The executor, which stays guest-side and runs on `execute-tool`.
    pub exec: Box<dyn ToolExec>,
}

// --- command execution (pi `RegisteredCommand.handler`, types.ts:1167 @v0.83.0, the interface at
// `:1162-1168`; EXT-072 corrected `:1105-1111`; R-08-016) ---

/// A slash-command body supplied by the guest author. Runs at COMMAND tier (the [`CommandCtx`]
/// exposes the session-control ops); `args` is the raw argument string; returns optional text.
pub trait CommandExec: 'static {
    /// Run the command body against the raw argument string, returning the optional text described
    /// on the trait.
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

/// A dynamic argument completer (pi `getArgumentCompletions(prefix)`, types.ts:1166 @v0.83.0;
/// EXT-072: `:1108` is `cancel?: boolean`).
pub type ArgCompleter = Box<dyn Fn(&str) -> Vec<String> + 'static>;

/// A registered slash command: its descriptor, its handler, and an optional dynamic argument
/// completer.
pub struct RegisteredCommand {
    /// What crosses the seam to the host at registration.
    pub descriptor: CommandDescriptor,
    /// The command body, which stays guest-side.
    pub handler: Box<dyn CommandExec>,
    /// An optional dynamic completer, run through the `get-argument-completions` export; `None`
    /// leaves only [`CommandDescriptor::completions`]'s static list.
    pub completions: Option<ArgCompleter>,
}

// --- keyboard shortcuts (pi `registerShortcut`, types.ts:1250-1256 @v0.83.0; EXT-072: the
// `:1198-1205` this cited is inside the `on(event: …)` overload block; R-08-017) ---

/// A keyboard-shortcut body supplied by the guest author (pi `options.handler(ctx)`,
/// types.ts:1254 @v0.83.0; EXT-072: `:1203` is `session_compact`'s overload).
/// Invoked across the `execute-shortcut` export when the registered `KeyId` fires; receives the base
/// [`Ctx`] (Pi hands the shortcut handler the general `ExtensionContext`).
pub trait ShortcutExec: 'static {
    /// Run the shortcut body against the base [`Ctx`].
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
    /// The key that fires it (Pi `KeyId`); crosses the seam at registration.
    pub key: String,
    /// The description the host displays; crosses the seam alongside the key.
    pub description: String,
    /// The body, which stays guest-side and runs via `execute-shortcut`.
    pub handler: Box<dyn ShortcutExec>,
}

// --- message renderers (Pi `renderCall`/`renderResult`, types.ts:491-498 @v0.84.4; R-08-020) ---

/// The `(options, theme)` half of every upstream renderer signature (EXT-006) — the guest mirror of
/// `cyrup_ext::RenderOptions`, which is what the host serializes into the export's `opts-json`.
///
/// cyrup routes all three of pi's renderer surfaces through ONE export pair, so this is the union
/// of pi's three option bags (`pi/packages/coding-agent/src/core/extensions/types.ts` @v0.84.4):
/// `MessageRenderOptions { expanded, outputPad }` `:1195-1199`, `EntryRenderOptions { expanded }`
/// `:1209-1211`, `ToolRenderResultOptions { expanded, isPartial }` `:413-418`. A surface for which
/// a field has no meaning leaves it at its default.
///
/// Both halves are LIVE: the host re-invokes the renderer whenever they move, so a renderer that
/// branches on [`Self::expanded`] really does redraw when the user presses the expand key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RenderOptions {
    /// `options.expanded` — the live `app.tools.expand` flag.
    pub expanded: bool,
    /// `MessageRenderOptions.outputPad` — the horizontal padding the `outputPad` setting configures.
    pub output_pad: u32,
    /// `ToolRenderResultOptions.isPartial` — whether the result being drawn is still streaming.
    pub is_partial: bool,
    /// The NAME of the active theme. Pi passes the whole `Theme` object; an object cannot cross the
    /// component boundary, so a guest that needs the PALETTE calls `ui.theme_get_json()` (EXT-066).
    /// `None` when the host has no display (an RPC host, a test).
    pub theme: Option<String>,
}

impl RenderOptions {
    /// Parse the host's `opts-json`. Absent or malformed fields take their defaults rather than
    /// failing — a renderer must still draw when it cannot be told the options, which is exactly
    /// what upstream does for a renderer that never reads its `options` argument.
    pub fn from_json(v: &Value) -> Self {
        Self {
            expanded: v.get("expanded").and_then(Value::as_bool).unwrap_or(false),
            output_pad: v
                .get("outputPad")
                .and_then(Value::as_u64)
                .and_then(|w| u32::try_from(w).ok())
                .unwrap_or(0),
            is_partial: v.get("isPartial").and_then(Value::as_bool).unwrap_or(false),
            theme: v
                .get("theme")
                .and_then(Value::as_str)
                .map(ToString::to_string),
        }
    }
}

/// A custom message renderer the guest registers for a `custom_type`. Each method returns a
/// serialized widget tree (`Value`), or `None` to fall back to the runtime's default renderer.
///
/// EXT-006 — `opts` is upstream's `(options, theme)` pair and is a LIVE input: the host re-invokes
/// the renderer when the expansion or the theme changes, so branching on it is how a renderer
/// draws a collapsed and an expanded form.
pub trait MessageRenderer: 'static {
    /// Render the CALL row as a widget tree, or `None` (the default) to leave it to the runtime.
    fn render_call(&self, _call: &Value, _opts: &RenderOptions, _ctx: &Ctx) -> Option<Value> {
        None
    }
    /// Render the RESULT row as a widget tree, or `None` (the default) to leave it to the runtime.
    fn render_result(&self, _result: &Value, _opts: &RenderOptions, _ctx: &Ctx) -> Option<Value> {
        None
    }
}

/// A registered renderer keyed by its `custom_type`.
pub struct RegisteredRenderer {
    /// The key this renderer is registered under, and the one the host routes `render-call` /
    /// `render-result` back by.
    pub custom_type: String,
    /// The renderer itself, which stays guest-side.
    pub renderer: Box<dyn MessageRenderer>,
}

// --- markdown transformer (EXT-019; Pi `MarkdownTransformer`, types.ts:1153 @v0.84.1) ---

/// The `MarkdownTransformContext` pi hands a transformer
/// (`pi/packages/coding-agent/src/core/extensions/types.ts:1147-1151` @v0.84.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkdownTransformContext {
    /// `"user" | "assistant" | "assistant-thinking"`.
    pub message_type: String,
    /// Whether the message is still streaming. `false` when the host's `isStreaming` is absent or
    /// not a bool ([`Self::from_json`]).
    pub is_streaming: bool,
    /// The host's `availableWidth`. `0` when it is absent, not a number, or does not fit a `u32`
    /// ([`Self::from_json`]).
    pub available_width: u32,
}

impl MarkdownTransformContext {
    /// Parse the host's `ctx-json`. Unknown/absent fields take conservative defaults rather than
    /// failing — a transformer must never be skipped because the host added a field.
    pub fn from_json(v: &Value) -> Self {
        Self {
            message_type: v
                .get("messageType")
                .and_then(Value::as_str)
                .unwrap_or("assistant")
                .to_string(),
            is_streaming: v
                .get("isStreaming")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            available_width: v
                .get("availableWidth")
                .and_then(Value::as_u64)
                .and_then(|w| u32::try_from(w).ok())
                .unwrap_or(0),
        }
    }
}

/// Transform transcript markdown before the host renders it (Pi
/// `type MarkdownTransformer = (markdown: string, context: MarkdownTransformContext) => string`,
/// `extensions/types.ts:1153` @v0.84.1 — a POST-BASELINE addition, absent at v0.83.0).
///
/// An extension has AT MOST ONE, because upstream stores it as `extension.markdownTransformer`
/// (`loader.ts:309-312`). The host folds every extension's transformer in load order, so this
/// receives whatever the previous extension produced.
pub trait MarkdownTransformer: 'static {
    /// Return the markdown to render. Because the host folds transformers in load order, the
    /// `markdown` given here is whatever the previous extension produced.
    fn transform(&self, markdown: &str, ctx: &MarkdownTransformContext) -> String;
}

/// One terminal-input handler's answer (EXT-021; pi `TerminalInputHandler`'s return,
/// `extensions/types.ts:113` @v0.83.0: `{ consume?: boolean; data?: string } | undefined`).
///
/// Both members stay `Option`: the host's fold — a port of `packages/tui/src/tui.ts:773-788` —
/// tests `consume` for truthiness and `data` for PRESENCE, so `{data: Some("")}` rewrites the
/// buffer to empty (and the keystroke is then dropped) while `TerminalInputResult::default()`
/// leaves it alone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalInputResult {
    /// Swallow the keystroke when truthy. Tested for TRUTHINESS by the fold, so `None` and
    /// `Some(false)` behave alike. Set by [`Self::consume`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consume: Option<bool>,
    /// Rewrite the keystroke's data. Tested for PRESENCE by the fold, so `Some("")` rewrites the
    /// buffer to empty while `None` leaves it alone. Set by [`Self::rewrite`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

impl TerminalInputResult {
    /// Swallow this keystroke — pi's `{consume: true}`.
    pub fn consume() -> Self {
        Self {
            consume: Some(true),
            data: None,
        }
    }

    /// Rewrite this keystroke and let the remaining handlers (and the editor) see the new value —
    /// pi's `{data}`.
    pub fn rewrite(data: impl Into<String>) -> Self {
        Self {
            consume: None,
            data: Some(data.into()),
        }
    }
}

/// A raw terminal-input handler (EXT-021; pi `TerminalInputHandler`,
/// `extensions/types.ts:113` @v0.83.0). `None` is upstream's `undefined` — "I looked at it and
/// did nothing".
///
/// Interactive mode only, exactly as upstream documents `onTerminalInput`
/// (`types.ts:144`): in RPC mode pi's own implementation is a no-op that returns an unsubscribe
/// doing nothing (`modes/rpc/rpc-mode.ts:162`).
pub trait TerminalInputHandler: 'static {
    /// Inspect one raw terminal input chunk. `None` is upstream's `undefined` — "I looked at it and
    /// did nothing".
    fn on_input(&self, data: &str) -> Option<TerminalInputResult>;
}

impl<F> TerminalInputHandler for F
where
    F: Fn(&str) -> Option<TerminalInputResult> + 'static,
{
    fn on_input(&self, data: &str) -> Option<TerminalInputResult> {
        self(data)
    }
}

impl<F> MarkdownTransformer for F
where
    F: Fn(&str, &MarkdownTransformContext) -> String + 'static,
{
    fn transform(&self, markdown: &str, ctx: &MarkdownTransformContext) -> String {
        self(markdown, ctx)
    }
}

/// A per-call command-execution backend this extension supplies for a `user_bash` command it
/// handled — pi `BashOperations` (`packages/coding-agent/src/core/tools/bash.ts:63-81` @v0.84.4:
/// *"Pluggable operations for the bash tool. Override these to delegate command execution to remote
/// systems (for example SSH)"*), returned as `UserBashEventResult.operations`
/// (`core/extensions/types.ts:1139`).
///
/// Register one with [`ExtensionApi::register_bash_operations`] and return `{"operations": true}`
/// from a `user_bash` handler (see that method's doc for why BOTH halves are needed). The host then
/// runs the extension's `!` command through [`Self::exec`] instead of the local shell — upstream's
/// `options?.operations ?? createLocalBashOperations({ shellPath })`
/// (`core/agent-session.ts:2782`).
///
/// The return is pi's `{ exitCode: number | null }`: `Ok(Some(code))` is an exit code, `Ok(None)`
/// is its `null` ("killed"), and `Err(message)` is its `throw` — which the host re-raises as a
/// failed command rather than a successful one with no output (`core/bash-executor.ts:154`).
pub trait BashOperations: 'static {
    /// Run one command, streaming its output with [`BashCommand::write`] and stopping when
    /// [`BashCommand::is_cancelled`] turns true.
    fn exec(&self, command: &BashCommand) -> Result<Option<i32>, String>;
}

impl<F> BashOperations for F
where
    F: Fn(&BashCommand) -> Result<Option<i32>, String> + 'static,
{
    fn exec(&self, command: &BashCommand) -> Result<Option<i32>, String> {
        self(command)
    }
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
    /// Custom-ENTRY renderers (pi `registerEntryRenderer`, types.ts:1295 @v0.84.1 / `:1279`
    /// @v0.83.0 — cite the tag, EXT-036's version-lag class). A SEPARATE list from
    /// [`Self::renderers`], mirroring upstream's disjoint `messageRenderers`/`entryRenderers` maps
    /// (types.ts:1702 and `:1704` @v0.84.1 — the two maps are NOT adjacent, `:1703` between them is
    /// `markdownTransformer`); on the wire an entry still travels over `render-call`.
    pub(crate) entry_renderers: Vec<RegisteredRenderer>,
    /// EXT-019: at most one per extension (pi `extension.markdownTransformer`, types.ts:1703 @v0.84.1
    /// @v0.84.1).
    pub(crate) markdown_transformer: Option<Box<dyn MarkdownTransformer>>,
    /// DRIFT-004: at most one per extension — upstream reads `operations` off the SINGLE
    /// `UserBashEventResult` whose handler won the reduction (`extensions/runner.ts:1005-1032`).
    pub(crate) bash_operations: Option<Box<dyn BashOperations>>,
    /// EXT-021: this extension's raw terminal-input handler, if it subscribed. AT MOST ONE —
    /// upstream allows several `onTerminalInput` calls per extension, but each returns its own
    /// unsubscribe and the host's subscriber table is keyed by EXTENSION, so a guest with two
    /// handlers would be folded once. Modelling it as one handler makes that explicit instead of
    /// silently dropping the second.
    pub(crate) terminal_input_handler: Option<Box<dyn TerminalInputHandler>>,
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
    if s.is_empty() { None } else { Some(json(s)) }
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
    /// An empty API surface — no handlers, no registrations. Identical to `Default::default()`.
    pub fn new() -> Self {
        Self::default()
    }

    // --- registration ---

    /// Register a tool (overrides a built-in of the same name host-side, R-08-012).
    pub fn register_tool(&mut self, descriptor: ToolDescriptor, exec: impl ToolExec) {
        self.tools.push(RegisteredTool {
            descriptor,
            exec: Box::new(exec),
        });
    }

    /// Register a pre-built tool (Pi `defineTool` output / `customTools` array entry, sdk gap #6).
    pub fn register_tool_def(&mut self, tool: RegisteredTool) {
        self.tools.push(tool);
    }

    /// Register a slash command (R-08-016). The handler runs at command tier (session ops allowed).
    /// Mirrors pi's `registerCommand(name, {description, handler})` (types.ts:1247 @v0.83.0;
    /// EXT-072 corrected `:1105`).
    pub fn register_command(
        &mut self,
        name: impl Into<String>,
        desc: CommandDescriptor,
        handler: impl CommandExec,
    ) {
        self.commands.push((
            name.into(),
            RegisteredCommand {
                descriptor: desc,
                handler: Box::new(handler),
                completions: None,
            },
        ));
    }

    /// Register a slash command with a dynamic argument completer (Pi `getArgumentCompletions`,
    /// types.ts:1166 @v0.83.0; EXT-072 corrected `:1108`): the completer is called with the current
    /// argument prefix.
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
    /// `registerShortcut(key, {description, handler})` (types.ts:1250-1256 @v0.83.0; EXT-072
    /// corrected `:1198-1205`): the `key`+`description`
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
        // EXT-076: the host dispatches the NORMALIZED (lowercased) key —
        // `ExtensionRegistry::resolve_shortcuts_inner` emits `key.to_lowercase()` — while
        // `register_shortcut` stores the author's key verbatim, so an exact compare here never
        // finds a handler registered as e.g. `"Ctrl+G"`. Upstream is structurally immune because
        // `setupExtensionShortcuts` captures the handler off the normalized map rather than
        // re-looking it up by the pressed key; matching case-insensitively is the same guarantee
        // at this seam. Shortcut keys are ASCII (`ctrl+alt+f`, `Ctrl+G`), so ASCII folding is the
        // right comparison and avoids Unicode-case surprises in a lookup key.
        match self
            .shortcuts
            .iter()
            .find(|s| s.key.eq_ignore_ascii_case(key))
        {
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
    /// `registerProvider({oauth, streamSimple})`, types.ts:1401 @v0.83.0, the `ProviderConfig` bag at
    /// `:1427-1464`; EXT-072 corrected `:1337`; sdk gap #1). The static `config`
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
    /// are invoked across the boundary when a tool of `custom_type` is displayed (Pi types.ts:489).
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

    /// Register this extension's markdown transformer (EXT-019; Pi
    /// `registerMarkdownTransformer(transformer)`, `extensions/types.ts:1292` @v0.84.1, impl
    /// `loader.ts:309-312`). AT MOST ONE per extension — a second call REPLACES the first, exactly
    /// as upstream's field assignment does.
    pub fn register_markdown_transformer(&mut self, transformer: impl MarkdownTransformer) {
        self.markdown_transformer = Some(Box::new(transformer));
    }

    /// Run this extension's markdown transformer, if it registered one (the `transform-markdown`
    /// export body). Identity when it did not.
    pub fn transform_markdown(&self, markdown: &str, ctx: &MarkdownTransformContext) -> String {
        match &self.markdown_transformer {
            Some(t) => t.transform(markdown, ctx),
            None => markdown.to_string(),
        }
    }

    /// Whether this extension registered a markdown transformer (drives the
    /// `register-markdown-transformer` import at init).
    pub fn has_markdown_transformer(&self) -> bool {
        self.markdown_transformer.is_some()
    }

    /// Register this extension's bash backend (DRIFT-004; pi `UserBashEventResult.operations`,
    /// `core/extensions/types.ts:1139` @v0.84.4). AT MOST ONE — a second call REPLACES the first,
    /// as upstream's per-result field does.
    ///
    /// BOTH halves are required for the backend to be used, and they say different things:
    ///
    /// * this call declares that the guest HAS a backend (it drives the
    ///   `registration.register-bash-operations` import at `init`);
    /// * a `user_bash` handler returning [`Outcome::handled`] with `{"operations": true}` is what
    ///   says *this particular command* should run through it — upstream's handler returns
    ///   `{ operations }` for exactly the commands it wants to redirect and `undefined` for the
    ///   rest (`examples/extensions/ssh.ts:203-206`), and a handler that returns a `result`
    ///   instead short-circuits execution entirely, so the backend is never consulted
    ///   (`modes/rpc/rpc-mode.ts:571-576`).
    pub fn register_bash_operations(&mut self, operations: impl BashOperations) {
        self.bash_operations = Some(Box::new(operations));
    }

    /// Run this extension's bash backend (the `bash-operations-exec` export body). `Err` when it
    /// registered none — an unexpected call must not look like a command that ran and produced
    /// nothing.
    pub fn exec_bash_operations(&self, command: &BashCommand) -> Result<Option<i32>, String> {
        match &self.bash_operations {
            Some(ops) => ops.exec(command),
            None => Err("this extension registered no bash operations".to_string()),
        }
    }

    /// Whether this extension registered a bash backend (drives the
    /// `registration.register-bash-operations` import at init).
    pub fn has_bash_operations(&self) -> bool {
        self.bash_operations.is_some()
    }

    /// Listen to raw terminal input (EXT-021; pi `ctx.ui.onTerminalInput(handler)`,
    /// `extensions/types.ts:145` @v0.83.0 — "Listen to raw terminal input (interactive mode
    /// only)"). A second call REPLACES the first; see the `terminal_input_handler` field's doc
    /// for why.
    pub fn on_terminal_input(&mut self, handler: impl TerminalInputHandler) {
        self.terminal_input_handler = Some(Box::new(handler));
    }

    /// Run this extension's terminal-input handler, if it registered one (the
    /// `on-terminal-input` export body). `None` — upstream's `undefined` — when it did not.
    pub fn handle_terminal_input(&self, data: &str) -> Option<TerminalInputResult> {
        self.terminal_input_handler
            .as_ref()
            .and_then(|h| h.on_input(data))
    }

    /// Whether this extension subscribed to raw terminal input (drives the
    /// `ui.subscribe-terminal-input` import at init).
    pub fn has_terminal_input_handler(&self) -> bool {
        self.terminal_input_handler.is_some()
    }

    /// Register a custom ENTRY renderer (Pi `pi.registerEntryRenderer(customType, renderer)`,
    /// types.ts:1295 @v0.84.1 / `:1279` @v0.83.0) — the TUI-only surface for entries appended with
    /// `append_entry`, which do NOT
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

    /// Declare that `command` supplies argument completions — the host then calls this guest's
    /// `argument-completions` export for it.
    ///
    /// [CYRUP-DELTA] EXT-062: upstream this is not a call but a FIELD on the options bag passed to
    /// `registerCommand` — `getArgumentCompletions?: (argumentPrefix: string) => AutocompleteItem[]
    /// | null | Promise<…>` (`extensions/types.ts:1166` @v0.83.0). A closure cannot cross the
    /// component boundary, so the declaration and the callback separate: this flag, and the export.
    /// Same inversion as `prepare_arguments` / `has_renderer` on a tool descriptor. `R-08-021` is a
    /// cyrup requirement id, not a pi citation.
    pub fn add_autocomplete(&mut self, command: impl Into<String>) {
        self.autocomplete.push(command.into());
    }

    /// Stack a global autocomplete provider on top of the current one (Pi `addAutocompleteProvider`,
    /// `extensions/types.ts:225` @v0.83.0 — the `:218` this used to cite is `getEditorText`'s doc
    /// line, the declaration being `:219`).
    /// Providers are folded in registration order: each sees the wrapped ("current") provider's
    /// suggestions and may augment or replace them.
    ///
    /// EXT-065: the declaring import is `ui.add-autocomplete-provider`, not
    /// `registration.add-autocomplete-provider` — upstream declares this inside `ExtensionUIContext`,
    /// and on cyrup that placement is what puts it behind the manifest's `capabilities.ui` grant. A
    /// guest without that grant has its providers refused host-side.
    pub fn add_autocomplete_provider(&mut self, provider: impl AutocompleteProvider) {
        self.autocomplete_providers.push(Box::new(provider));
    }

    /// Subscribe to an inter-extension event-bus topic (Pi `pi.events.on(channel, handler)`,
    /// event-bus.ts:18; gap-08 §5.3). The `handler` runs whenever ANY loaded extension — this one
    /// included, matching Pi's EventEmitter — emits `topic` via [`Ctx::emit`]; it receives the topic
    /// and the emitted JSON payload. The topic is declared to the host (the `bus.subscribe` import)
    /// so the host knows to fan a matching emit out to this guest's `bus-deliver` export.
    pub fn on_bus(
        &mut self,
        topic: impl Into<String>,
        handler: impl Fn(&str, Value, &Ctx) + 'static,
    ) {
        self.bus_subscriptions
            .push((topic.into(), Box::new(handler)));
    }

    // --- the 33 event subscriptions ---

    /// `tool_call` — VETOABLE (returns [`Outcome`]): block the call with [`Outcome::block`]
    /// (first block wins host-side) or rewrite its arguments with
    /// [`Outcome::replace_tool_input`]. Payload [`ToolCallEvent`].
    ///
    /// The first of this type's 33 event subscribers (pi `pi.on`, types.ts:1190-1231 @v0.83.0;
    /// EXT-072 corrected the count AND the range, which cited the message-rendering block).
    /// Each subscriber below names its pi event and says whether it is vetoable or
    /// notify-only. `tool_call` is the one kind that fails CLOSED: it is cyrup's permission
    /// seam (R-08-010), so a handler that traps, panics or exhausts its budget DENIES the call
    /// instead of silently allowing it (EXT-001).
    pub fn on_tool_call(&mut self, f: impl Fn(ToolCallEvent, &Ctx) -> Outcome + 'static) {
        self.handlers.insert(
            kind::TOOL_CALL,
            Box::new(move |a, c| {
                let ev = ToolCallEvent {
                    call_id: arg(a, 0).into(),
                    name: arg(a, 1).into(),
                    input: json(arg(a, 2)),
                };
                f(ev, c).into_raw()
            }),
        );
    }
    /// `tool_result` — VETOABLE (returns [`Outcome`]): override result fields with
    /// [`Outcome::patch_tool_result`] (replace-not-merge, R-08-011). Payload [`ToolResultEvent`].
    pub fn on_tool_result(&mut self, f: impl Fn(ToolResultEvent, &Ctx) -> Outcome + 'static) {
        self.handlers.insert(
            kind::TOOL_RESULT,
            Box::new(move |a, c| {
                let ev = ToolResultEvent {
                    call_id: arg(a, 0).into(),
                    name: arg(a, 1).into(),
                    input: json(arg(a, 2)),
                    content: json(arg(a, 3)),
                    is_error: arg(a, 4) == "true",
                    details: opt_json(arg(a, 5)),
                    // pi `ToolResultEventBase.usage` (types.ts:920-921 @v0.83.0; `:919` is `isError`);
                    // empty arg = pi `undefined`.
                    usage: opt_json(arg(a, 6)),
                };
                f(ev, c).into_raw()
            }),
        );
    }
    /// `context` — VETOABLE (returns [`Outcome`]): filter or replace the LLM message list with
    /// [`Outcome::replace_messages`]. Payload [`ContextEvent`].
    pub fn on_context(&mut self, f: impl Fn(ContextEvent, &Ctx) -> Outcome + 'static) {
        self.handlers.insert(
            kind::CONTEXT,
            Box::new(move |a, c| {
                f(
                    ContextEvent {
                        messages: json(arg(a, 0)),
                    },
                    c,
                )
                .into_raw()
            }),
        );
    }
    /// `message_end` — VETOABLE (returns [`Outcome`]): replace the just-finished message with
    /// [`Outcome::replace_message`] (same role enforced host-side). Payload [`MessageEndEvent`].
    pub fn on_message_end(&mut self, f: impl Fn(MessageEndEvent, &Ctx) -> Outcome + 'static) {
        self.handlers.insert(
            kind::MESSAGE_END,
            Box::new(move |a, c| {
                f(
                    MessageEndEvent {
                        message: json(arg(a, 0)),
                    },
                    c,
                )
                .into_raw()
            }),
        );
    }
    /// `before_agent_start` — VETOABLE (returns [`Outcome`]): inject a message and/or replace the
    /// system prompt with [`Outcome::before_agent_start`]. Payload [`BeforeAgentStartEvent`].
    pub fn on_before_agent_start(
        &mut self,
        f: impl Fn(BeforeAgentStartEvent, &Ctx) -> Outcome + 'static,
    ) {
        self.handlers.insert(
            kind::BEFORE_AGENT_START,
            Box::new(move |a, c| {
                let ev = BeforeAgentStartEvent {
                    prompt: arg(a, 0).into(),
                    images: json(arg(a, 1)),
                    system_prompt: arg(a, 2).into(),
                    options: json(arg(a, 3)),
                };
                f(ev, c).into_raw()
            }),
        );
    }
    /// `input` — VETOABLE (returns [`Outcome`]): transform the submission, or service it outright
    /// with [`Outcome::handled`]. Payload [`InputEvent`].
    pub fn on_input(&mut self, f: impl Fn(InputEvent, &Ctx) -> Outcome + 'static) {
        self.handlers.insert(
            kind::INPUT,
            Box::new(move |a, c| {
                let ev = InputEvent {
                    text: arg(a, 0).into(),
                    images: opt_images(arg(a, 1)),
                    source: arg(a, 2).into(),
                    streaming_behavior: opt_str(arg(a, 3)),
                };
                f(ev, c).into_raw()
            }),
        );
    }
    /// `user_bash` — VETOABLE (returns [`Outcome`]): block, transform or fully service a `!`/`!!`
    /// bash invocation ([`Outcome::handled`]). Payload [`UserBashEvent`].
    pub fn on_user_bash(&mut self, f: impl Fn(UserBashEvent, &Ctx) -> Outcome + 'static) {
        self.handlers.insert(
            kind::USER_BASH,
            Box::new(move |a, c| {
                let ev = UserBashEvent {
                    command: arg(a, 0).into(),
                    exclude_from_context: arg(a, 1) == "true",
                    cwd: arg(a, 2).into(),
                };
                f(ev, c).into_raw()
            }),
        );
    }
    /// `before_provider_request` — VETOABLE (returns [`Outcome`]): mutate the outbound provider
    /// payload. Payload [`BeforeProviderRequestEvent`].
    pub fn on_before_provider_request(
        &mut self,
        f: impl Fn(BeforeProviderRequestEvent, &Ctx) -> Outcome + 'static,
    ) {
        self.handlers.insert(
            kind::BEFORE_PROVIDER_REQUEST,
            Box::new(move |a, c| {
                f(
                    BeforeProviderRequestEvent {
                        payload: json(arg(a, 0)),
                    },
                    c,
                )
                .into_raw()
            }),
        );
    }
    /// `before_provider_headers` (EXT-009) — VETOABLE-shaped: it returns [`Outcome`], not `()`.
    /// The reading the host gives that outcome is the header patch — return it via
    /// [`Outcome::mutate`], where a key mapped to `null` DELETES that header (pi
    /// `extensions/types.ts:681-685` @v0.83.0: handlers "mutate `headers` in place … the
    /// return value is ignored"). Payload [`BeforeProviderHeadersEvent`].
    pub fn on_before_provider_headers(
        &mut self,
        f: impl Fn(BeforeProviderHeadersEvent, &Ctx) -> Outcome + 'static,
    ) {
        self.handlers.insert(
            kind::BEFORE_PROVIDER_HEADERS,
            Box::new(move |a, c| {
                f(
                    BeforeProviderHeadersEvent {
                        headers: json(arg(a, 0)),
                    },
                    c,
                )
                .into_raw()
            }),
        );
    }
    /// `session_info_changed` (EXT-011) — notify-only (returns `()`). Payload
    /// [`SessionInfoChangedEvent`].
    pub fn on_session_info_changed(&mut self, f: impl Fn(SessionInfoChangedEvent, &Ctx) + 'static) {
        self.handlers.insert(
            kind::SESSION_INFO_CHANGED,
            notify(move |a, c| {
                f(
                    SessionInfoChangedEvent {
                        name: opt_str(arg(a, 0)),
                    },
                    c,
                )
            }),
        );
    }
    /// `resources_discover` — VETOABLE (returns [`Outcome`]): contribute skill/prompt/theme paths
    /// with [`Outcome::handled`] ([`ResourcesResult`]). Payload [`ResourcesDiscoverEvent`].
    pub fn on_resources_discover(
        &mut self,
        f: impl Fn(ResourcesDiscoverEvent, &Ctx) -> Outcome + 'static,
    ) {
        // EXT-016: `cwd` + `reason` (pi extensions/types.ts:544-548 @v0.83.0) — a
        // resource-contributing extension could not tell which directory it was discovering for,
        // nor startup from `/reload`, so it could not scope or cache its contribution.
        self.handlers.insert(
            kind::RESOURCES_DISCOVER,
            Box::new(move |a, c| {
                f(
                    ResourcesDiscoverEvent {
                        cwd: arg(a, 0).into(),
                        reason: arg(a, 1).into(),
                    },
                    c,
                )
                .into_raw()
            }),
        );
    }
    /// `project_trust` — VETOABLE (returns [`Outcome`]): decide whether `cwd` is trusted. The
    /// answer shape is [`ProjectTrustResult`], whose `trusted` is pi's TRI-STATE — `undecided`
    /// falls through to the next handler, which a bool would collapse. Payload
    /// [`ProjectTrustEvent`].
    pub fn on_project_trust(&mut self, f: impl Fn(ProjectTrustEvent, &Ctx) -> Outcome + 'static) {
        // EXT-043: `cwd` (pi extensions/types.ts:519-522 @v0.83.0) — the key the trust store is
        // keyed by, so `remember` has a well-defined meaning from the handler's point of view.
        self.handlers.insert(
            kind::PROJECT_TRUST,
            Box::new(move |a, c| {
                f(
                    ProjectTrustEvent {
                        cwd: arg(a, 0).into(),
                    },
                    c,
                )
                .into_raw()
            }),
        );
    }
    /// `session_before_switch` — VETOABLE (returns [`Outcome`]): [`Outcome::block`] refuses the
    /// switch. Payload [`SessionBeforeSwitchEvent`].
    pub fn on_session_before_switch(
        &mut self,
        f: impl Fn(SessionBeforeSwitchEvent, &Ctx) -> Outcome + 'static,
    ) {
        self.handlers.insert(
            kind::SESSION_BEFORE_SWITCH,
            Box::new(move |a, c| {
                f(
                    SessionBeforeSwitchEvent {
                        reason: arg(a, 0).into(),
                        target_session_file: opt_str(arg(a, 1)),
                    },
                    c,
                )
                .into_raw()
            }),
        );
    }
    /// `session_before_fork` — VETOABLE (returns [`Outcome`]): [`Outcome::block`] refuses the fork.
    /// Payload [`SessionBeforeForkEvent`].
    pub fn on_session_before_fork(
        &mut self,
        f: impl Fn(SessionBeforeForkEvent, &Ctx) -> Outcome + 'static,
    ) {
        self.handlers.insert(
            kind::SESSION_BEFORE_FORK,
            Box::new(move |a, c| {
                f(
                    SessionBeforeForkEvent {
                        entry_id: arg(a, 0).into(),
                        position: arg(a, 1).into(),
                    },
                    c,
                )
                .into_raw()
            }),
        );
    }
    /// `session_before_compact` — VETOABLE (returns [`Outcome`]): [`Outcome::block`] refuses the
    /// compaction, or [`Outcome::compaction_override`] supplies the summary instead of the model.
    /// Payload [`SessionBeforeCompactEvent`].
    pub fn on_session_before_compact(
        &mut self,
        f: impl Fn(SessionBeforeCompactEvent, &Ctx) -> Outcome + 'static,
    ) {
        self.handlers.insert(
            kind::SESSION_BEFORE_COMPACT,
            Box::new(move |a, c| {
                let ev = SessionBeforeCompactEvent {
                    preparation: json(arg(a, 0)),
                    branch_entries: json(arg(a, 1)),
                    custom_instructions: opt_str(arg(a, 2)),
                    reason: arg(a, 3).into(),
                    will_retry: arg(a, 4) == "true",
                };
                f(ev, c).into_raw()
            }),
        );
    }
    /// `session_before_tree` — VETOABLE (returns [`Outcome`]): [`Outcome::block`] refuses the
    /// branch summarization, or [`Outcome::tree_override`] overrides its
    /// summary/instructions/label.
    /// Payload [`SessionBeforeTreeEvent`].
    pub fn on_session_before_tree(
        &mut self,
        f: impl Fn(SessionBeforeTreeEvent, &Ctx) -> Outcome + 'static,
    ) {
        self.handlers.insert(
            kind::SESSION_BEFORE_TREE,
            Box::new(move |a, c| {
                f(
                    SessionBeforeTreeEvent {
                        preparation: json(arg(a, 0)),
                    },
                    c,
                )
                .into_raw()
            }),
        );
    }

    // --- notify-only subscriptions (return ignored) ---

    /// `agent_start` — notify-only: the handler returns `()`, which the SDK lowers to
    /// [`RawOutcome::Noop`]. Carries no payload, so the handler receives only the [`Ctx`].
    pub fn on_agent_start(&mut self, f: impl Fn(&Ctx) + 'static) {
        self.handlers
            .insert(kind::AGENT_START, notify(move |_a, c| f(c)));
    }
    /// `agent_end` — notify-only (the handler returns `()`; the SDK reports [`RawOutcome::Noop`]).
    /// Payload [`AgentEndEvent`], the full final message list.
    pub fn on_agent_end(&mut self, f: impl Fn(AgentEndEvent, &Ctx) + 'static) {
        self.handlers.insert(
            kind::AGENT_END,
            notify(move |a, c| {
                f(
                    AgentEndEvent {
                        messages: json(arg(a, 0)),
                    },
                    c,
                )
            }),
        );
    }
    /// `agent_settled` — notify-only (returns `()`), and payload-free like
    /// [`Self::on_agent_start`]: the handler receives only the [`Ctx`].
    ///
    /// pi `on("agent_settled", handler)` (extensions/types.ts:1217 @v0.83.0; EXT-073: `:1225` is
    /// `tool_execution_end`). Fires ONCE per run, after every
    /// automatic retry / post-run compaction / queued continuation has finished — unlike
    /// [`Self::on_agent_end`], which fires once per `agent.prompt`/`agent.continue`.
    pub fn on_agent_settled(&mut self, f: impl Fn(&Ctx) + 'static) {
        self.handlers
            .insert(kind::AGENT_SETTLED, notify(move |_a, c| f(c)));
    }
    /// `turn_start` — notify-only (returns `()`). Payload [`TurnStartEvent`].
    pub fn on_turn_start(&mut self, f: impl Fn(TurnStartEvent, &Ctx) + 'static) {
        self.handlers.insert(
            kind::TURN_START,
            notify(move |a, c| {
                f(
                    TurnStartEvent {
                        turn_index: arg(a, 0).parse().unwrap_or(0),
                        timestamp: arg(a, 1).parse().unwrap_or(0),
                    },
                    c,
                )
            }),
        );
    }
    /// `turn_end` — notify-only (returns `()`). Payload [`TurnEndEvent`]: the finalized assistant
    /// message AND the tool results produced this turn.
    pub fn on_turn_end(&mut self, f: impl Fn(TurnEndEvent, &Ctx) + 'static) {
        self.handlers.insert(
            kind::TURN_END,
            notify(move |a, c| {
                f(
                    TurnEndEvent {
                        turn_index: arg(a, 0).parse().unwrap_or(0),
                        message: json(arg(a, 1)),
                        tool_results: json(arg(a, 2)),
                    },
                    c,
                )
            }),
        );
    }
    /// `message_start` — notify-only (returns `()`). Payload [`MessageStartEvent`].
    pub fn on_message_start(&mut self, f: impl Fn(MessageStartEvent, &Ctx) + 'static) {
        self.handlers.insert(
            kind::MESSAGE_START,
            notify(move |a, c| {
                f(
                    MessageStartEvent {
                        message: json(arg(a, 0)),
                    },
                    c,
                )
            }),
        );
    }
    /// `message_update` — notify-only (returns `()`) and HIGH-FREQ. Payload
    /// [`MessageUpdateEvent`]: the full in-flight message AND the provider delta.
    pub fn on_message_update(&mut self, f: impl Fn(MessageUpdateEvent, &Ctx) + 'static) {
        self.handlers.insert(
            kind::MESSAGE_UPDATE,
            notify(move |a, c| {
                f(
                    MessageUpdateEvent {
                        message: json(arg(a, 0)),
                        assistant_message_event: json(arg(a, 1)),
                    },
                    c,
                )
            }),
        );
    }
    /// `tool_execution_start` — notify-only (returns `()`). Payload [`ToolExecStartEvent`]; the
    /// vetoable seam for a tool invocation is [`Self::on_tool_call`].
    pub fn on_tool_exec_start(&mut self, f: impl Fn(ToolExecStartEvent, &Ctx) + 'static) {
        self.handlers.insert(
            kind::TOOL_EXEC_START,
            notify(move |a, c| {
                f(
                    ToolExecStartEvent {
                        call_id: arg(a, 0).into(),
                        name: arg(a, 1).into(),
                        args: json(arg(a, 2)),
                    },
                    c,
                )
            }),
        );
    }
    /// `tool_execution_update` — notify-only (returns `()`) and HIGH-FREQ. Payload
    /// [`ToolExecUpdateEvent`], which carries the streamed `chunk`.
    pub fn on_tool_exec_update(&mut self, f: impl Fn(ToolExecUpdateEvent, &Ctx) + 'static) {
        self.handlers.insert(
            kind::TOOL_EXEC_UPDATE,
            notify(move |a, c| {
                f(
                    ToolExecUpdateEvent {
                        call_id: arg(a, 0).into(),
                        name: arg(a, 1).into(),
                        args: json(arg(a, 2)),
                        chunk: json(arg(a, 3)),
                    },
                    c,
                )
            }),
        );
    }
    /// `tool_execution_end` — notify-only (returns `()`). Payload [`ToolExecEndEvent`]; the
    /// vetoable seam for the finished result is [`Self::on_tool_result`].
    pub fn on_tool_exec_end(&mut self, f: impl Fn(ToolExecEndEvent, &Ctx) + 'static) {
        self.handlers.insert(
            kind::TOOL_EXEC_END,
            notify(move |a, c| {
                f(
                    ToolExecEndEvent {
                        call_id: arg(a, 0).into(),
                        name: arg(a, 1).into(),
                        result: json(arg(a, 2)),
                        is_error: arg(a, 3) == "true",
                    },
                    c,
                )
            }),
        );
    }
    /// `session_start` — notify-only (returns `()`). Payload [`SessionLifecycleEvent`], whose
    /// `reason` includes `"reload"` and whose `session_file` is pi's `previousSessionFile`.
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
    /// `session_shutdown` — notify-only (returns `()`). Payload [`SessionLifecycleEvent`], whose
    /// `session_file` is pi's `targetSessionFile` — the destination of a session replacement.
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
    /// `after_provider_response` — notify-only (returns `()`). Payload
    /// [`AfterProviderResponseEvent`]: the HTTP status + response headers.
    pub fn on_after_provider_response(
        &mut self,
        f: impl Fn(AfterProviderResponseEvent, &Ctx) + 'static,
    ) {
        self.handlers.insert(
            kind::AFTER_PROVIDER_RESPONSE,
            notify(move |a, c| {
                f(
                    AfterProviderResponseEvent {
                        status: arg(a, 0).parse().unwrap_or(0),
                        headers: json(arg(a, 1)),
                    },
                    c,
                )
            }),
        );
    }
    /// `model_select` — notify-only (returns `()`). Payload [`ModelSelectEvent`]: the new model,
    /// the previous one, and the `source` of the change.
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
    /// `thinking_level_select` — notify-only (returns `()`). Payload [`ThinkingLevelSelectEvent`].
    pub fn on_thinking_level_select(
        &mut self,
        f: impl Fn(ThinkingLevelSelectEvent, &Ctx) + 'static,
    ) {
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
    /// `session_compact` — notify-only (returns `()`): the compaction entry has already been
    /// produced. Payload [`SessionCompactEvent`]; the vetoable seam is
    /// [`Self::on_session_before_compact`].
    pub fn on_session_compact(&mut self, f: impl Fn(SessionCompactEvent, &Ctx) + 'static) {
        // The host seam supplies the full Pi shape: the produced compaction entry, whether an
        // extension drove it, the trigger reason, and the retry flag (L4 gap #5, wired through the
        // cyrup-session-svc producer).
        self.handlers.insert(
            kind::SESSION_COMPACT,
            notify(move |a, c| {
                let ev = SessionCompactEvent {
                    compaction_entry: json(arg(a, 0)),
                    from_extension: arg(a, 1) == "true",
                    reason: arg(a, 2).into(),
                    will_retry: arg(a, 3) == "true",
                };
                f(ev, c)
            }),
        );
    }
    /// `session_tree` — notify-only (returns `()`). Payload [`SessionTreeEvent`]; the vetoable seam
    /// is [`Self::on_session_before_tree`].
    pub fn on_session_tree(&mut self, f: impl Fn(SessionTreeEvent, &Ctx) + 'static) {
        self.handlers.insert(
            kind::SESSION_TREE,
            notify(move |a, c| {
                f(
                    SessionTreeEvent {
                        tree: json(arg(a, 0)),
                    },
                    c,
                )
            }),
        );
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
    pub fn render_call(
        &self,
        custom_type: &str,
        call: &Value,
        opts: &RenderOptions,
    ) -> Option<Value> {
        self.renderers
            .iter()
            .chain(self.entry_renderers.iter())
            .find(|r| r.custom_type == custom_type)
            .and_then(|r| r.renderer.render_call(call, opts, &Ctx::new()))
    }

    /// Render a tool result via a registered renderer for `custom_type` (Pi `renderResult`).
    pub fn render_result(
        &self,
        custom_type: &str,
        result: &Value,
        opts: &RenderOptions,
    ) -> Option<Value> {
        self.renderers
            .iter()
            .find(|r| r.custom_type == custom_type)
            .and_then(|r| r.renderer.render_result(result, opts, &Ctx::new()))
    }

    // --- provider OAuth + streamSimple callbacks (pi `ProviderConfig`, types.ts:1427-1464 @v0.83.0
    // — `streamSimple` `:1437`, `oauth` `:1450-1463`; EXT-072: `:1380-1392` is `@example` JSDoc;
    // sdk gap #1) ---

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
    /// `AutocompleteProviderFactory` chain, types.ts:124 @v0.83.0; EXT-072: `:117` is a
    /// `WorkingIndicatorOptions` doc line; sdk gap #2). Each provider sees the previous
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

    /// The tools this extension registered, in registration order (descriptor + executor).
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
