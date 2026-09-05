use super::*;

/// Which summarization an [`Entry::Warning`]-styled compaction-cost notice is reporting — pi's
/// `CompactionCostNotice.kind`, a synthetic render item carrying `entry.type` (`"compaction"` |
/// `"branch_summary"`) off the session entry that paid for the summary
/// (`interactive-mode.ts:3788-3794` @v0.83.0).
///
/// Presentation-only: it selects between the two label words pi prints at
/// `interactive-mode.ts:3811`, so it lives with the view rather than with the session types that
/// produce the usage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactionCostKind {
    /// A context compaction (`entry.type === "compaction"`) — label `Compaction`.
    Compaction,
    /// A branch summarization (`entry.type === "branch_summary"`) — label `Branch summary`.
    BranchSummary,
}

impl CompactionCostKind {
    /// pi's `notice.kind === "compaction" ? "Compaction" : "Branch summary"`
    /// (`interactive-mode.ts:3811`).
    pub(crate) const fn label(self) -> &'static str {
        match self {
            CompactionCostKind::Compaction => "Compaction",
            CompactionCostKind::BranchSummary => "Branch summary",
        }
    }
}

/// A committed transcript entry.
///
/// `Eq` is intentionally omitted: [`ToolRun`] carries the raw `serde_json::Value` call args / result
/// (so each tool can render its Pi-specific `renderCall`/`renderResult`), and `Value` is `PartialEq`
/// but not `Eq` (floats). Nothing in the crate needs a total `Eq` on entries.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    /// A user submission.
    ///
    /// `lead_spacer` freezes `interactive-mode.ts:3500`'s
    /// `if (this.chatContainer.children.length > 0)` at push time: the `Spacer(1)` that separates a
    /// user turn from what precedes it is **not** emitted for the very first child of a fresh
    /// session's chat. The trailing user message of a skill block (`:3513-3521`) is a different call
    /// site with no such gate and always carries it.
    User { text: String, lead_spacer: bool },
    /// A finalized assistant message.
    Assistant(String),
    /// A finalized run of assistant **thinking**/reasoning blocks (`assistant-message.ts:115-166`).
    ///
    /// Pi coalesces every consecutive `thinking` content block of a turn into ONE section joined by
    /// `\n\n` (`:116-127`) and renders it as italic `thinkingText` markdown (`:145-165`) — or, when
    /// `hideThinkingBlock` is set, as the single static `Thinking...` label (`:139-143`).
    ///
    /// `hidden` freezes that choice **at commit time**: Pi's `setHideThinkingBlock` (`:57-62`)
    /// re-renders every prior assistant message live, but cyrup's committed entries have already
    /// left the render tree for native scrollback (`App::flush_committed` → `insert_before`), so a
    /// runtime toggle can only affect entries committed after the flip (ADR-0001).
    Thinking { text: String, hidden: bool },
    /// A finished tool execution (`tool-execution.ts`): the tool name + the raw call args + the raw
    /// result value + error flag. Each built-in dispatches to its Pi-specific rich render
    /// (`core/tools/{read,write,edit,bash,grep,find,ls}.ts` `renderCall`/`renderResult`).
    Tool(ToolRun),
    /// A status / notification line (model change, compaction, queue, …).
    Status(String),
    /// pi's accent swap receipt: `Spacer(1)` + `Text(fg("accent", …), paddingX 1, paddingY 1)`
    /// (`interactive-mode.ts:6322-6324`, `handleClearCommand`). `Text.render` emits `paddingY`
    /// blanks above AND below (`packages/tui/src/components/text.ts:90-98`), which is the whole
    /// difference from [`Self::Status`] — rows `["", "", " ✓ New session started", ""]` versus
    /// `Status`'s `["", " msg"]`. `/new` alone uses this; every other session-swap caption stays
    /// `Status`.
    Receipt(String),
    /// An `error`-styled notice appended after an assistant turn that did not finish cleanly
    /// (`assistant-message.ts:175-201`): the max-output-token truncation notice, the abort wording,
    /// or `Error: {message}`. Rendered as a blank spacer + one error-coloured line, matching Pi's
    /// `Spacer(1)` + `Text(theme.fg("error", …), outputPad, 0)`.
    Error(String),
    /// A `warning`-styled notice — Pi `showWarning` (`interactive-mode.ts:3956-3960`): a `Spacer(1)`
    /// then `Text(theme.fg("warning", "Warning: …"), 1, 0)`. Structurally identical to [`Self::Error`]
    /// but in the warning colour; reached from an extension's `notify(msg, "warning")`.
    Warning(String),
    /// A bordered info block (`/hotkeys`, `/changelog`, `/session`, `/debug`): a top `DynamicBorder`,
    /// a bold-accent `title`, a blank, the `markdown` body, then a bottom `DynamicBorder`
    /// (interactive-mode.ts:5502-5507).
    Block { title: String, markdown: String },
    /// A finished `!`/`!!` bash execution (`bash-execution.ts`): the command header + output block,
    /// committed to scrollback when the process exits.
    Bash(BashExecution),
    /// A skill-invocation message (`skill-invocation-message.ts`): a bold `[skill]` label + the skill
    /// name, with the skill block content rendered as markdown below.
    ///
    /// `lead_spacer` carries the same `interactive-mode.ts:3500` gate as [`Self::User`] — the skill
    /// component is added at `:3506`, inside that `if (textContent)` / `children.length > 0` block,
    /// so it is the component the gated spacer actually precedes when a submission opens with a
    /// `<skill>` block.
    SkillInvocation {
        name: String,
        content: String,
        lead_spacer: bool,
    },
    /// A custom (extension) message (`custom-message.ts`): a bracketed type `label` + a markdown
    /// `body`, styled distinctly from a plain user message.
    ///
    /// `rendered` carries the text an extension's registered message renderer produced for this
    /// custom type (EXT-006); when present it REPLACES the label+markdown framing, because the
    /// renderer already owns the presentation (Pi hands the resolved renderer to
    /// `CustomMessageComponent` instead of the default, interactive-mode.ts:3324-3336).
    Custom {
        label: String,
        body: String,
        rendered: Rendered,
    },
    /// A branch-summary message (`branch-summary-message.ts`): a bold `[branch]` label + the
    /// `**Branch Summary**` markdown body produced when navigating away from a branch.
    ///
    /// X14 — the collapsed/expanded choice is NOT stored on the entry. Upstream's
    /// `BranchSummaryMessageComponent` keeps a live `expanded` field that BOTH
    /// `interactive-mode.ts:3493`'s seeding `component.setExpanded(this.toolOutputExpanded)` and
    /// `setToolsExpanded`'s re-broadcast to every `chatContainer` child (`:4032-4046`) write, so a
    /// later `Ctrl+O` reveals the body of a summary that was pushed while collapsed. Freezing the
    /// flag at push time made the expanded body unreachable; the render arm reads
    /// [`ImageOpts::tools_expanded`] — the LIVE flag — instead.
    BranchSummary { summary: String },
    /// A compaction-summary message (`compaction-summary-message.ts`): a bold `[compaction]` label
    /// noting the pre-compaction token count + the `**Compacted from N tokens**` summary markdown.
    /// Its expansion is `interactive-mode.ts:3486`'s, read live — see [`Self::BranchSummary`].
    CompactionSummary { tokens_before: u64, summary: String },
    /// The startup loaded-resources / diagnostics panel (`showLoadedResources`,
    /// interactive-mode.ts:1480-1690) — the `[Context]`/`[Skills]`/`[Prompts]`/`[Extensions]`/
    /// `[Themes]` inventory and the `[Skill conflicts]`/`[Prompt conflicts]`/`[Extension issues]`/
    /// `[Theme conflicts]` blocks. Pre-formatted by [`crate::startup::build_startup_lines`] because
    /// the expand/collapse choice cannot be revisited once committed (see that module's docs).
    LoadedResources(Vec<crate::startup::StartupLine>),
}

/// What an extension's registered renderer produced for an [`Entry::Custom`] (EXT-006, X15).
///
/// Upstream has TWO renderer components over custom types and they agree on three outcomes but not
/// on what to DRAW for them:
///
/// | outcome | `CustomMessageComponent` (`custom-message.ts:60-88`) | `CustomEntryComponent` (`custom-entry.ts:40-60`) |
/// |---|---|---|
/// | returned a component | the component, verbatim (`:76-80`) | `Spacer(1)` + the component (`:58-60`) |
/// | returned nothing / none registered | the default `[type] body` box (`:87-111`) | nothing at all (`:54-56`, `interactive-mode.ts:3433-3435`) |
/// | THREW | caught, falls through to the default box (`:82-84`) | `Spacer(1)` + a `customMessageBg` box holding `[type] renderer failed: {message}` (`:47-52`) |
///
/// So the throw is user-visible on exactly one surface. cyrup modelled the whole thing as
/// `Option<String>` and could not tell "the renderer threw" from "there is no renderer", which is
/// why [`Rendered::Failed`] had no producer; [`cyrup_ext::RenderOutcome`] now keeps them apart from
/// `ExtensionHost::render_via` outward, and [`crate::app::extension_render_entry`] is what turns a
/// fault into this variant. The message/tool surfaces still collapse `Failed` into
/// [`Rendered::None`] — that IS their upstream behaviour.
///
/// `Eq` is deliberately absent: [`Self::Live`] holds an `Arc<dyn RenderedComponent>`, which has no
/// meaningful total equality. `PartialEq` is hand-written below and compares `Live` by pointer.
#[derive(Clone, Debug, Default)]
pub enum Rendered {
    /// No renderer is registered for this custom type, or the registered one drew nothing. A custom
    /// MESSAGE draws the default `[label]` + markdown box; a custom ENTRY is not pushed at all.
    #[default]
    None,
    /// The renderer's output, already flattened to display text, together with the invocation that
    /// produced it ([`RenderedText`]). Emitted verbatim.
    Text(RenderedText),
    /// The renderer threw; the payload is `error.message`. Draws Pi's failure box
    /// (`components/custom-entry.ts:47-52`).
    ///
    /// Produced by [`crate::app::extension_render_entry`] from a
    /// [`cyrup_ext::RenderOutcome::Failed`] — a native renderer that panicked (contained by
    /// `catch_unwind`) or a guest renderer that trapped.
    Failed(String),
    /// The renderer handed back a LIVE component. Re-rendered by [`crate::transcript::entry_lines`]
    /// on EVERY frame at the live width, theme and expansion — the X14 rule `Entry::Tool` and
    /// `Entry::BranchSummary` already follow, and what makes a resize re-wrap and an expand toggle
    /// open a card that was pushed collapsed.
    Live(std::sync::Arc<dyn cyrup_ext::RenderedComponent>),
}

/// One extension render: the flattened rows, and everything it takes to ASK FOR THEM AGAIN
/// (EXT-006).
///
/// # Why the two travel together
/// Upstream re-invokes a renderer from the DRAW path — `MessageRenderer = (message, options, theme)
/// => Component` (`pi/packages/coding-agent/src/core/extensions/types.ts:1213-1217` @v0.84.4) is
/// called per paint — so `options.expanded` and the active `theme` are live inputs. cyrup's draw
/// path is sync and a guest renderer is an async wasm call, so the render happens once, off the
/// event path, and the text is written into the transcript. That makes the text a SNAPSHOT, and a
/// snapshot is only valid for the inputs it was taken under.
///
/// Rather than have every toggle remember to invalidate something, the snapshot carries its own
/// inputs: [`crate::App::refresh_extension_renders`] derives staleness by comparing
/// [`RenderSource::under`] against the live display inputs, so a row that would draw differently
/// re-renders and a row that would not costs one comparison. A new producer of a
/// [`Rendered::Text`] cannot forget to register itself, because the only thing that builds one is
/// [`crate::app::run_renderer`], which stamps the source from the call it just made.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderedText {
    /// The rows the renderer produced, flattened by `crate::app::rendered_text`.
    pub text: String,
    /// How to obtain them again — see [`Self::source`]. Private, and boxed: it is read only by the
    /// refresh pass, every [`Entry`] carries two of these, and `Entry`'s largest variant is already
    /// the one that would grow.
    source: Option<Box<RenderSource>>,
}

impl RenderedText {
    /// A render that can be asked for again — what every extension renderer produces.
    pub fn new(text: impl Into<String>, source: RenderSource) -> Self {
        Self {
            text: text.into(),
            source: Some(Box::new(source)),
        }
    }

    /// A render with NO re-invocation path: it never refreshes, whatever the display inputs do.
    /// The literal a test builds, and the shape-preserving value for a producer that is not an
    /// extension renderer.
    pub fn frozen(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            source: None,
        }
    }

    /// The invocation that produced [`Self::text`], if it can be repeated.
    pub fn source(&self) -> Option<&RenderSource> {
        self.source.as_deref()
    }
}

/// The renderer invocation a [`RenderedText`] came from, and the display inputs it ran under.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderSource {
    /// Which host renderer surface answered — the one to ask again.
    pub surface: RenderSurface,
    /// The key the renderer is registered under: a custom type for a message/entry, the TOOL NAME
    /// for a tool row (`crate::app::extension_render`).
    pub key: String,
    /// The payload the renderer was handed. Kept because a re-invocation needs it and the entry it
    /// is attached to does not always still hold it (a custom message keeps only its rendered body).
    pub payload: serde_json::Value,
    /// The display inputs [`RenderedText::text`] was produced under. Comparing this against the
    /// live ones IS the staleness test.
    pub under: cyrup_ext::RenderOptions,
}

/// Where in the view a [`RenderedText`] lives, so [`crate::App::refresh_extension_renders`] can
/// write a fresh one back into the row it re-rendered.
///
/// A live tool row is in `active_tools` and a finished-and-flushed one is in `pending`, so both
/// have to be addressable; a custom message/entry is only ever in `pending`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RenderSlot {
    /// `pending[i]`, an [`Entry::Custom`]'s `rendered`.
    PendingCustom(usize),
    /// `pending[i]`, an [`Entry::Tool`]'s CALL side.
    PendingToolCall(usize),
    /// `pending[i]`, an [`Entry::Tool`]'s RESULT side.
    PendingToolResult(usize),
    /// `active_tools[i]`'s CALL side.
    ActiveToolCall(usize),
    /// `active_tools[i]`'s RESULT side.
    ActiveToolResult(usize),
}

/// One row whose extension render was produced under display inputs that no longer hold.
pub(crate) struct StaleRender {
    /// The row to write the new text into.
    pub slot: RenderSlot,
    /// The invocation to repeat, with [`RenderSource::under`] already advanced to the LIVE inputs —
    /// so the caller hands it straight to `crate::app::run_renderer` and the result records the
    /// options it was actually produced under.
    pub next: RenderSource,
}

/// Which of the host's four renderer entry points produced a [`RenderedText`].
///
/// Upstream keeps `messageRenderers` and `entryRenderers` as disjoint maps
/// (`extensions/types.ts:1766-1768` @v0.84.4) and resolves a tool row's `renderCall`/`renderResult`
/// off the tool DEFINITION (`components/tool-execution.ts:84-92`); cyrup's host keeps the same
/// tables, so re-asking has to name the surface rather than just the key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderSurface {
    /// `ExtensionHost::render_message_call_outcome` — a custom MESSAGE.
    Message,
    /// `ExtensionHost::render_tool_call_outcome` — the CALL side of a tool row.
    ToolCall,
    /// `ExtensionHost::render_tool_result_outcome` — the RESULT side of a tool row.
    ToolResult,
    /// `ExtensionHost::render_entry` — a custom ENTRY.
    Entry,
}

impl PartialEq for Rendered {
    /// Structural for the value arms; pointer identity for [`Self::Live`], which is the only
    /// equality a trait object can honestly offer.
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::None, Self::None) => true,
            (Self::Text(a), Self::Text(b)) => a == b,
            (Self::Failed(a), Self::Failed(b)) => a == b,
            (Self::Live(a), Self::Live(b)) => std::sync::Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl Rendered {
    /// Collapse to the pre-X15 single-render shape — what the custom-MESSAGE and TOOL-row
    /// surfaces want, since `custom-message.ts:82-84` catches a throw and falls through to the
    /// default box. The ENTRY surface must NOT use this.
    pub fn into_text(self) -> Option<RenderedText> {
        match self {
            Self::Text(t) => Some(t),
            // A live component has no flattened text: it is drawn per frame, not folded once. The
            // message surface must carry the `Rendered` through instead of collapsing it here.
            Self::None | Self::Failed(_) | Self::Live(_) => None,
        }
    }

    /// Whether anything at all should be drawn for a custom ENTRY carrying this outcome
    /// (`CustomEntryComponent.hasContent()`, `custom-entry.ts:24-26`, checked at
    /// `interactive-mode.ts:3438-3440`). A failure box counts as content — upstream assigns it to
    /// `customComponent` (`:51`) before the check.
    pub fn has_content(&self) -> bool {
        !matches!(self, Self::None)
    }
}

/// One tool execution, shown live in the viewport while it runs (`tool-execution.ts` pending box) and
/// committed to scrollback when the turn ends. The block is tinted by execution state
/// (`toolPendingBg`/`toolSuccessBg`/`toolErrorBg`), and each built-in renders its own Pi-specific
/// `renderCall` header + `renderResult` body. `expanded` (`Ctrl+O`, `app.tools.expand`) shows the full
/// result; the collapsed form shows each tool's preview (bash tail-5, grep head-15, find/ls head-20, a
/// hidden read/write body, …).
#[derive(Clone, Debug, PartialEq)]
pub struct ToolRun {
    /// Tool name (`read`, `bash`, `edit`, …).
    pub name: String,
    /// The provider-assigned `toolCallId` of the call this run renders (`ToolCall::id`,
    /// message.rs:150 / `AgentSessionEvent::Tool*::tool_call_id`). This is the IDENTITY Pi pairs a
    /// result to its call by: every rendered call component is filed under `content.id`
    /// (interactive-mode.ts:3473, and `pendingTools.set(event.toolCallId, …)` at `:3096` on the live
    /// path) and every result resolves with `get(message.toolCallId)` (`:3483`, `:3113`). Matching by
    /// [`name`](ToolRun::name) instead swaps the bodies of two calls to the SAME tool in one turn —
    /// the batched shape parallel tool execution produces routinely.
    ///
    /// `None` only when the caller had no id in hand (a synthesized run from a result whose start
    /// was missed, or a test/legacy `push_tool_start`), in which case the name fallback applies.
    pub call_id: Option<String>,
    /// The raw tool-call arguments (`renderCall(args)`) — the path/command/pattern/offset/limit/…
    /// each tool's header is built from. `Value::Null` when a start was missed.
    pub args: Value,
    /// The raw tool result (`{content, details, terminate}`; `renderResult(result)`) — carries the
    /// per-tool `details` (edit `diff`, bash/read/grep/find/ls `truncation`, …). `None` while running.
    pub result: Option<Value>,
    /// Whether the tool failed.
    pub is_error: bool,
    /// Whether the execution has finished (drives the pending→success/error background tint).
    pub done: bool,
    /// Wall-clock start of the run, set on [`TranscriptView::push_tool_start`] — the basis for the
    /// bash `Took …` duration line (`formatDuration`, bash.ts:197/284-288).
    pub(super) started_at: Option<std::time::Instant>,
    /// Frozen run duration in milliseconds, set on [`TranscriptView::push_tool_end`]. Rendered as the
    /// bash `Took {d}s` footer once the command finishes.
    pub(super) duration_ms: Option<u64>,
    /// The CALL text an extension's registered renderer produced for this tool (EXT-006; Pi
    /// `ToolDefinition.renderCall`, extensions/types.ts:491 @v0.84.4, preferred over the built-in by
    /// `tool-execution.ts:81-112`). `None` = no extension renders this tool, so the built-in
    /// per-tool dispatch draws it.
    ///
    /// A [`RenderedText`] rather than a bare `String` since EXT-006: it carries the display inputs
    /// the text was produced under, which is what lets `Ctrl+O` and a theme switch re-invoke the
    /// renderer instead of leaving a frozen row among live ones.
    pub rendered_call: Option<RenderedText>,
    /// The RESULT text an extension's registered renderer produced (Pi `renderResult`,
    /// extensions/types.ts:493-498 @v0.84.4). See [`ToolRun::rendered_call`].
    pub rendered_result: Option<RenderedText>,
    /// What the session's `getToolDefinition(name)` registry (agent-session.ts:806) answered for
    /// [`name`](ToolRun::name) when the run started: `None` = no definition, `Some(shell)` = a
    /// definition declaring that `renderShell`. Two of Pi's `ToolExecutionComponent` questions
    /// read off it:
    ///
    /// * `hasRendererDefinition()`, i.e. `builtInToolDefinition !== undefined || toolDefinition
    ///   !== undefined` (tool-execution.ts:103-105) — `is_some()` here, and so true for an
    ///   extension-registered, SDK-registered or MCP-proxied tool as well as a built-in. It is the
    ///   branch upstream picks the whole block SHAPE by, and it is **not** "an extension
    ///   registered a renderer" — that is [`rendered_call`](ToolRun::rendered_call) /
    ///   [`rendered_result`](ToolRun::rendered_result), and a definition with no renderer is the
    ///   normal case for an MCP tool. A defined tool draws through its renderers, falling back
    ///   per-side to a bold name (`createCallFallback`, `:137-139`) and a ten-line output preview
    ///   (`createResultFallback`, `:141-155`); only a tool with NO definition reaches the
    ///   unbounded `formatToolExecution` (`:330-333`) that dumps the full argument JSON.
    /// * `getRenderShell()`, i.e. `toolDefinition.renderShell ?? builtInToolDefinition.renderShell
    ///   ?? "default"` (`:108-116`) — the payload. cyrup keeps ONE definition per name (the
    ///   session registry merges built-ins, custom and extension tools), so the two-tier `??` is
    ///   a single read of [`cyrup_core::Tool::render_kind`]; `None` is upstream's final
    ///   `"default"`. [`ToolRenderKind::SelfRendered`] drops the tinted `Box(1, 1)` shell in
    ///   favour of the tool's own framing (EXT-024; `:76`, `:237-259`, `:275-277`).
    ///
    /// `None` on the id-less/legacy constructors — the shape-preserving value, since every
    /// built-in name is answered by the built-in table before the first question is consulted, so
    /// it decides only how an entirely UNKNOWN name draws — and the value under which every
    /// built-in but `edit` keeps its shell.
    pub definition: Option<ToolRenderKind>,
    /// `edit`'s **pre-execution** diff preview — Pi `EditCallRenderComponent.preview`
    /// (edit.ts:145-153), set by `setEditPreview` (`:263-280`) from the `computeEditsDiff` its
    /// `renderCall` fires the moment the streamed arguments are complete (`:377-386`).
    ///
    /// It is what puts the diff on screen while the call is still PENDING — through the permission
    /// prompt (cyrup emits `ToolExecutionStart` before `prepare`, i.e. before the `before_tool_call`
    /// gate, `cyrup-agent/src/agent.rs:1181/1334`) and before anything is written. `Ok` is the diff
    /// text, `Err` the `EditDiffError.error` message; `None` is "no preview" (a non-`edit` tool, a
    /// replayed history entry, or a file too large to preview synchronously).
    ///
    /// Populated only through [`TranscriptView::set_edit_preview`].
    pub preview: Option<Result<String, String>>,
    /// The `image` content blocks of the result, decoded once when the run finishes
    /// (`tool-execution.ts:331-350` filters `content` for `type === "image"` on every display
    /// update). Decoding here rather than per frame keeps a screenshot-sized PNG off the redraw path.
    pub images: Vec<ResultImage>,
}
