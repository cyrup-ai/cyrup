//! The transcript / history view (R-10-028; arch-10 §2 `chrome/messages.rs`).
//!
//! Renders the conversation: user messages, assistant text (including the in-flight streaming
//! partial), and tool-execution start/end markers. The view owns a flat list of [`Entry`]s plus a
//! single mutable streaming buffer for the assistant turn currently being produced.
//!
//! ## Dependency note (driving from `AgentSessionEvent`)
//! `cyrup-tui` depends directly on `cyrup-provider` (Cargo.toml), so the streaming-delta variants of
//! `StreamEvent` *are* pattern-matched: [`crate::app::App::ingest_event`] folds
//! `MessageUpdate`'s `assistant_message_event` and appends `TextDelta { delta, .. }` here via
//! [`push_assistant_delta`](TranscriptView::push_assistant_delta), so the viewport grows
//! token-by-token like Pi's interactive stream. The terminal assistant message is recovered via
//! `StreamEvent::terminal_message()` (yielding a `&cyrup_core::AssistantMessage`) and replaces the
//! partial on `Done`/`Error`. The neutral [`TranscriptView`] API
//! ([`push_user`](TranscriptView::push_user),
//! [`push_assistant_delta`](TranscriptView::push_assistant_delta), …) remains the integration seam
//! an adapter feeds.

use cyrup_core::Content;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;
use serde_json::Value;
use unicode_segmentation::UnicodeSegmentation;

use crate::bash::BashExecution;
use crate::component::Component;
use crate::image::{image_fallback_text, ImageBlock};
use crate::theme::UiTheme;

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
    SkillInvocation { name: String, content: String, lead_spacer: bool },
    /// A custom (extension) message (`custom-message.ts`): a bracketed type `label` + a markdown
    /// `body`, styled distinctly from a plain user message.
    ///
    /// `rendered` carries the text an extension's registered message renderer produced for this
    /// custom type (EXT-006); when present it REPLACES the label+markdown framing, because the
    /// renderer already owns the presentation (Pi hands the resolved renderer to
    /// `CustomMessageComponent` instead of the default, interactive-mode.ts:3324-3336).
    Custom { label: String, body: String, rendered: Rendered },
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
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Rendered {
    /// No renderer is registered for this custom type, or the registered one drew nothing. A custom
    /// MESSAGE draws the default `[label]` + markdown box; a custom ENTRY is not pushed at all.
    #[default]
    None,
    /// The renderer's output, already flattened to display text. Emitted verbatim.
    Text(String),
    /// The renderer threw; the payload is `error.message`. Draws Pi's failure box
    /// (`components/custom-entry.ts:47-52`).
    ///
    /// Produced by [`crate::app::extension_render_entry`] from a
    /// [`cyrup_ext::RenderOutcome::Failed`] — a native renderer that panicked (contained by
    /// `catch_unwind`) or a guest renderer that trapped.
    Failed(String),
}

impl Rendered {
    /// Collapse to the pre-X15 `Option<String>` shape — what the custom-MESSAGE and TOOL-row
    /// surfaces want, since `custom-message.ts:82-84` catches a throw and falls through to the
    /// default box. The ENTRY surface must NOT use this.
    pub fn into_text(self) -> Option<String> {
        match self {
            Self::Text(t) => Some(t),
            Self::None | Self::Failed(_) => None,
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
    started_at: Option<std::time::Instant>,
    /// Frozen run duration in milliseconds, set on [`TranscriptView::push_tool_end`]. Rendered as the
    /// bash `Took {d}s` footer once the command finishes.
    duration_ms: Option<u64>,
    /// The CALL text an extension's registered renderer produced for this tool (EXT-006; Pi
    /// `ToolDefinition.renderCall`, extensions/types.ts:472-473, preferred over the built-in by
    /// `tool-execution.ts:81-112`). `None` = no extension renders this tool, so the built-in
    /// per-tool dispatch draws it.
    pub rendered_call: Option<String>,
    /// The RESULT text an extension's registered renderer produced (Pi `renderResult`,
    /// extensions/types.ts:475-481). See [`ToolRun::rendered_call`].
    pub rendered_result: Option<String>,
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

/// One `image` content block of a tool result (`{type:"image", data, mimeType}`) — the wire mime type
/// plus the decoded raster, or `None` when the bytes were not a recognizable image. A block that
/// fails to decode still renders Pi's text stand-in ([`crate::image::image_fallback_text`]) so the
/// user is told an image came back.
#[derive(Clone, Debug, PartialEq)]
pub struct ResultImage {
    /// The declared `mimeType` (`image/png`, …), or `image/unknown` when the block omitted it —
    /// Pi's own default in `getTextOutput` (render-utils.ts:53).
    pub mime_type: String,
    /// The decoded raster, downscaled to [`MAX_RASTER_PX`], or `None` if the base64/format could
    /// not be decoded.
    pub block: Option<ImageBlock>,
    /// The **source** pixel dimensions, before any downscale — what Pi's `imageFallback` reports
    /// (`getImageDimensions(img.data, img.mimeType)`, render-utils.ts:55-56).
    pub dimensions: Option<(u32, u32)>,
}

/// The scrolling conversation history.
///
/// Per ADR-0001 / R-ARCH-TUI-003 the inline viewport holds only the **active** region (the
/// in-flight streaming turn). Entries are *committed* the moment they are pushed and are buffered in
/// `pending` until the shell drains them via [`drain_committed`](TranscriptView::drain_committed),
/// at which point they are emitted to the terminal's native scrollback with `Terminal::insert_before`
/// and never re-rendered inside the viewport.
#[derive(Default)]
pub struct TranscriptView {
    /// Committed entries awaiting a flush to native scrollback (drained, not re-rendered).
    pending: Vec<Entry>,
    /// Whether anything has already been drained into native scrollback. Together with the live
    /// buffers this answers `this.chatContainer.children.length > 0` (`interactive-mode.ts:3500`) —
    /// see [`TranscriptView::chat_has_children`]. `pending` alone cannot: it is emptied on every
    /// flush, so a session with a full screen of history would look like a fresh one.
    chat_flushed: bool,
    /// The assistant turn currently streaming, if any (the only entry the viewport renders).
    streaming: Option<String>,
    /// The assistant **reasoning** currently streaming (`StreamEvent::ThinkingDelta`), if any. Held
    /// separately from `streaming` because Pi renders thinking as its own block above the answer
    /// text (`assistant-message.ts:115-166`), in its own italic `thinkingText` colour.
    thinking: Option<String>,
    /// `hideThinkingBlock` (settings-manager.ts; Pi `AssistantMessageComponent.hideThinkingBlock`,
    /// assistant-message.ts:126): render one static `Thinking...` label instead of the reasoning
    /// body. Read when a thinking run is rendered live and frozen into [`Entry::Thinking::hidden`]
    /// when it commits.
    hide_thinking: bool,
    /// An extension's `setHiddenThinkingLabel(label?)` override (Pi `this.hiddenThinkingLabel`,
    /// `interactive-mode.ts:436` @v0.84.2 — as are `:435` and `:2118-2129` below); `None` is
    /// [`HIDDEN_THINKING_LABEL`], upstream's `defaultHiddenThinkingLabel` (`:435`).
    ///
    /// Read at PAINT time rather than frozen into [`Entry::Thinking`], because upstream's setter
    /// re-labels everything already on screen: it walks `chatContainer.children` and calls
    /// `setHiddenThinkingLabel` on every `AssistantMessageComponent`, plus the streaming one
    /// (`:2118-2129`). cyrup reproduces that for the live block and for every entry still inside the
    /// viewport. **[CYRUP-DELTA]** entries already flushed to the terminal's NATIVE scrollback via
    /// `insert_before` cannot be repainted at all (they are the terminal's cells now, not cyrup's),
    /// so an extension that relabels mid-session changes the live and pending rows and leaves
    /// scrolled-off history as it was written — the same architectural limit every other retroactive
    /// restyle on this path already has.
    hidden_thinking_label: Option<String>,
    /// Tool executions for the active turn, rendered live in the viewport until the turn ends, then
    /// committed (`tool-execution.ts` keeps tool components live in the message region). Honors the
    /// shared `tool_expanded` flag so `Ctrl+O` visibly expands/collapses in-flight tool output.
    active_tools: Vec<ToolRun>,
    /// Whether tool output renders expanded (full result) vs collapsed (`Ctrl+O`, `app.tools.expand`).
    pub tool_expanded: bool,
    /// `terminal.showImages` (Pi `ToolExecutionComponent.showImages`, tool-execution.ts:335). ON:
    /// a tool result's `image` blocks rasterize inline; OFF: they render as Pi's `[Image: …]` text
    /// stand-in (`getTextOutput`, render-utils.ts:49-59). Default `true`, matching
    /// `settings.terminal.showImages`.
    show_images: bool,
    /// Whether the terminal negotiated a real image protocol — Pi's `getCapabilities().images`
    /// (`components/tool-execution.ts:331`). TUI-N01: seeded from
    /// `AppState::image_renderer.is_graphical()` at boot and on session swap. Default `true` so a
    /// bare `TranscriptView::default()` in a test still exercises the inline raster path.
    graphical_images: bool,
    /// `terminal.imageWidthCells` (Pi `maxWidthCells`, tool-execution.ts:348; default 60): the cell
    /// width an inline tool-result image is clamped to.
    image_width_cells: u16,
    /// The live `!`/`!!` bash execution, if one is running/just-finished and not yet committed
    /// (`bash-execution.ts` — the component stays live in the message region, then scrolls up).
    bash: Option<BashExecution>,
    /// Live key labels (`Esc` / `Ctrl+O`) for the bash block's cancel + expand hints, set by the app
    /// from the keymap so rebinds reflect. `None` falls back to the Pi defaults.
    bash_cancel_hint: Option<String>,
    bash_expand_hint: Option<String>,
    /// X9 — the live `app.tools.expand` label for the TOOL/summary hints (`keyText`,
    /// `keybinding-hints.ts:34-36`). Distinct from [`Self::bash_expand_hint`], which is scoped to a
    /// single `!`/`!!` block's lifetime; this one outlives any one block. `None` = [`EXPAND_KEY`].
    expand_hint: Option<String>,
    /// X7 — Pi `ToolRenderContext.cwd` (`tool-execution.ts:126`): the SESSION's working directory,
    /// which `read`'s compact classification resolves relative paths against (`read.ts:336`).
    /// `None` falls back to the process cwd.
    cwd: Option<std::path::PathBuf>,
    /// How many visual lines the user has paged **up** from the tail of the active region
    /// (`PageUp`/`PageDown`, spec/tui/07). `0` keeps the newest text pinned to the bottom (the
    /// default auto-scroll); paging up reveals earlier streamed/tool/bash output before it commits to
    /// native scrollback. Reset to `0` whenever new content lands so live streaming stays visible.
    scroll_offset: usize,
    /// Horizontal padding (columns) applied to user/assistant/thinking output (Pi `outputPad`, `0`|`1`,
    /// default `1` — settings-manager.ts:1186; `Markdown(content, outputPad, 0)` /
    /// `Box(outputPad, 1)`, assistant-message.ts:103, user-message.ts:31). Toggled live from
    /// `/settings` → "Output padding" (F12). Left-indents each message line; `0` renders flush-left.
    output_pad: usize,
    /// Monotonic invalidation counter for the render cache (TUI-092 F2), bumped by EVERY public
    /// `&mut self` mutator that changes what `lines()` would emit — see the bump list below.
    /// `page_up`/`page_down` and `drain_committed` deliberately do NOT bump: scroll is applied at
    /// paint time, and a flush does not change the live region.
    render_generation: u64,
    /// The materialised active region, valid iff its key matches
    /// `(render_generation, width, theme.generation)` at request time. NOT an `Option`: the
    /// workspace no-panic lints (Cargo.toml [workspace.lints.clippy]: unwrap/expect/panic = deny)
    /// forbid the `expect("just populated")` an `Option` forces on the re-borrow, and the
    /// derived `Default` is provably what `lines()` emits for a default view (empty everything →
    /// empty lines, height 0), so the first call simply misses on `width` and populates.
    render_cache: RenderCache,
}

/// One materialisation of the active region: the styled lines plus their wrapped display height,
/// keyed by everything `lines()` + `wrapped_height()` read that is not `self` — the width and the
/// theme generation — plus the content generation that covers every `self` field.
#[derive(Default)]
struct RenderCache {
    generation: u64,
    width: usize,
    theme_generation: u64,
    lines: Vec<Line<'static>>,
    wrapped_height: usize,
}

impl TranscriptView {
    /// An empty transcript.
    pub fn new() -> Self {
        // Pi's `outputPad` defaults to 1 (settings-manager.ts:1186), `terminal.showImages` to
        // `true` and `terminal.imageWidthCells` to 60 (settings-manager.ts:1047-1066) — none of
        // which a derived `Default` would give, so seed them explicitly.
        TranscriptView {
            output_pad: 1,
            show_images: true,
            graphical_images: true,
            image_width_cells: DEFAULT_IMAGE_WIDTH_CELLS,
            ..TranscriptView::default()
        }
    }

    /// The horizontal output padding (`outputPad`, columns) applied to user/assistant messages — read
    /// by the shell to pass into [`entry_lines`] when flushing committed entries to scrollback.
    pub fn output_pad(&self) -> usize {
        self.output_pad
    }

    /// Set the horizontal output padding live (Pi `onOutputPadChange` → `this.outputPad = padding`,
    /// interactive-mode.ts:4127-4136). The `/settings` "Output padding" row drives this; the live region
    /// re-renders with the new indent on the next draw, and subsequently-committed messages flush with
    /// it (already-scrolled-off native scrollback is immutable, an accepted consequence of the
    /// content-sized-viewport architecture).
    pub fn set_output_pad(&mut self, pad: usize) {
        self.bump_render_generation();
        self.output_pad = pad;
    }

    /// Set `terminal.showImages` (Pi `ToolExecutionComponent.showImages`): rasterize a tool result's
    /// `image` blocks inline, or fall back to Pi's `[Image: …]` text stand-in.
    pub fn set_show_images(&mut self, show: bool) {
        self.bump_render_generation();
        self.show_images = show;
    }

    /// Whether inline tool-result images are on (read by the shell when flushing committed entries).
    pub fn show_images(&self) -> bool {
        self.show_images
    }

    /// Set whether the terminal has a real image protocol (TUI-N01; Pi's `getCapabilities().images`
    /// gate at `tool-execution.ts:331`). Off ⇒ a tool result's `image` blocks take the same
    /// `[Image: …]` text branch `showImages: false` takes, rather than rasterizing anyway.
    pub fn set_graphical_images(&mut self, graphical: bool) {
        self.bump_render_generation();
        self.graphical_images = graphical;
    }

    /// Whether the terminal has a real image protocol (read by the shell when flushing committed
    /// entries, so a committed block and the live one it scrolled up from agree).
    pub fn graphical_images(&self) -> bool {
        self.graphical_images
    }

    /// Set `terminal.imageWidthCells` (Pi `maxWidthCells`): the cell width an inline image is
    /// clamped to. `0` is coerced to 1 so a degenerate setting cannot produce a zero-width raster.
    pub fn set_image_width_cells(&mut self, cells: u16) {
        self.bump_render_generation();
        self.image_width_cells = cells.max(1);
    }

    /// The inline-image cell-width clamp (read by the shell when flushing committed entries).
    pub fn image_width_cells(&self) -> u16 {
        self.image_width_cells
    }

    /// Committed entries not yet flushed to scrollback (test/inspection access).
    pub fn pending(&self) -> &[Entry] {
        &self.pending
    }

    /// The current streaming partial, if a turn is in flight.
    pub fn streaming(&self) -> Option<&str> {
        self.streaming.as_deref()
    }

    /// True while an assistant turn is actively streaming **or** a tool/bash run is live in the viewport.
    pub fn has_active(&self) -> bool {
        self.streaming.is_some()
            || self.thinking.is_some()
            || !self.active_tools.is_empty()
            || self.bash.is_some()
    }

    /// Start a live `!`/`!!` bash execution block (replaces any prior uncommitted one). `cancel_hint`
    /// / `expand_hint` are the live key labels for the running + expand hints.
    pub fn start_bash(
        &mut self,
        command: impl Into<String>,
        excluded: bool,
        cancel_hint: Option<String>,
        expand_hint: Option<String>,
    ) {
        self.bump_render_generation();
        self.bash = Some(BashExecution::new(command, excluded));
        self.bash_cancel_hint = cancel_hint;
        self.bash_expand_hint = expand_hint;
    }

    /// Append a streamed chunk to the live bash block (`appendOutput`). No-op if none is live.
    pub fn bash_append(&mut self, chunk: &str) {
        self.bump_render_generation();
        if let Some(b) = self.bash.as_mut() {
            b.append_output(chunk);
        }
    }

    /// Mark the live bash block finished (`setComplete`). No-op if none is live.
    ///
    /// X13 — `truncated`/`full_output_path` are `setComplete`'s third and fourth arguments
    /// (`bash-execution.ts:98-103`), fed upstream from `result.truncated` / `result.fullOutputPath`
    /// (`interactive-mode.ts:6307-6312`). They drive the `Output truncated. Full output: …` status
    /// row. See [`Self::bash_complete_simple`] for the `!` path, which has no spool of its own.
    pub fn bash_complete(
        &mut self,
        exit_code: Option<i32>,
        cancelled: bool,
        truncated: bool,
        full_output_path: Option<String>,
    ) {
        self.bump_render_generation();
        if let Some(b) = self.bash.as_mut() {
            b.set_complete(exit_code, cancelled, truncated, full_output_path);
        }
    }

    /// [`Self::bash_complete`] with no truncation report — Pi's `catch` arm
    /// (`interactive-mode.ts:6357` `setComplete(undefined, false)`), and the shape the interactive
    /// `!` runner uses while it has no spool file to point at.
    pub fn bash_complete_simple(&mut self, exit_code: Option<i32>, cancelled: bool) {
        self.bump_render_generation();
        self.bash_complete(exit_code, cancelled, false, None);
    }

    /// Whether a bash block is live (running or finished-but-uncommitted).
    pub fn has_bash(&self) -> bool {
        self.bash.is_some()
    }

    /// Whether the live bash block is still running.
    pub fn bash_running(&self) -> bool {
        self.bash.as_ref().is_some_and(BashExecution::is_running)
    }

    /// The live bash block (test/inspection access).
    pub fn bash(&self) -> Option<&BashExecution> {
        self.bash.as_ref()
    }

    /// Toggle the live bash block's expansion (`Ctrl+O`); returns the new state if a block is live.
    pub fn toggle_bash_expanded(&mut self) -> Option<bool> {
        self.bump_render_generation();
        self.bash.as_mut().map(|b| {
            let next = !b.expanded();
            b.set_expanded(next);
            next
        })
    }

    /// Set the live bash block's expansion ABSOLUTELY — TUI-038. `setToolsExpanded` broadcasts one
    /// value to every `isExpandable` child of `loadedResourcesContainer` and `chatContainer`
    /// (`interactive-mode.ts:4040-4046` @v0.84.1), and the bash component is one of them
    /// (`components/bash-execution.ts:29` `private expanded = false`, `setExpanded` at `:70`). It is
    /// a fan-out upstream, not a choice between the bash block and the tool blocks. No-op when no
    /// block is live.
    pub fn set_bash_expanded(&mut self, expanded: bool) {
        self.bump_render_generation();
        if let Some(b) = self.bash.as_mut() {
            b.set_expanded(expanded);
        }
    }

    /// Commit the live bash block to scrollback (called once it has finished). A still-running block
    /// is committed as-is (e.g. on interrupt). No-op when none is live.
    pub fn commit_bash(&mut self) {
        self.bump_render_generation();
        if let Some(b) = self.bash.take() {
            self.pending.push(Entry::Bash(b));
        }
    }

    /// Commit an ALREADY-FINISHED `!`/`!!` execution straight to scrollback, without going through
    /// the live block. This is the replay path for a persisted `bashExecution` message (Pi
    /// `addMessageToChat`'s `bashExecution` arm — `new BashExecutionComponent(command, ui,
    /// excludeFromContext)` + `appendOutput(output)` + `setComplete(...)`, interactive-mode.ts:3310-3322),
    /// so a resumed session shows the user's own `!` commands as bash blocks instead of the
    /// ``Ran `cmd` ``  prose `convertToLlm` renders them to for the model.
    ///
    /// X13 — the persisted `bashExecution` message carries `truncated` and `fullOutputPath`, and
    /// upstream replays BOTH: `component.setComplete(message.exitCode, message.cancelled,
    /// message.truncated ? {truncated:true} : undefined, message.fullOutputPath)`
    /// (`interactive-mode.ts:3460-3465`). cyrup dropped them, so a resumed session lost the pointer
    /// to where the full output was spooled.
    #[allow(
        clippy::too_many_arguments,
        reason = "upstream's own arity: `new BashExecutionComponent(command, ui, excludeFromContext)`                   + `appendOutput(output)` + `setComplete(exitCode, cancelled, truncationResult,                   fullOutputPath)` (interactive-mode.ts:3454-3465), collapsed into one replay call"
    )]
    pub fn push_bash_execution(
        &mut self,
        command: impl Into<String>,
        excluded: bool,
        output: &str,
        exit_code: Option<i32>,
        cancelled: bool,
        truncated: bool,
        full_output_path: Option<String>,
    ) {
        self.bump_render_generation();
        let mut b = BashExecution::new(command, excluded);
        if !output.is_empty() {
            b.append_output(output);
        }
        b.set_complete(exit_code, cancelled, truncated, full_output_path);
        self.pending.push(Entry::Bash(b));
    }

    /// Take every committed entry, leaving the pending buffer empty. The shell renders the returned
    /// entries into native scrollback exactly once (R-ARCH-TUI-003), so they are not shown again in
    /// the inline viewport.
    pub fn drain_committed(&mut self) -> Vec<Entry> {
        self.chat_flushed |= !self.pending.is_empty();
        std::mem::take(&mut self.pending)
    }

    /// `this.chatContainer.children.length > 0` (`interactive-mode.ts:3500`).
    ///
    /// cyrup's analogue of the chat container is the whole entry stream: everything ever committed —
    /// still in `pending` or already flushed by [`drain_committed`](Self::drain_committed) — plus the
    /// live components the viewport owns while a turn runs. Upstream keeps those live components
    /// (`AssistantMessageComponent`, each `ToolExecutionComponent`, the `!` bash block) in
    /// `chatContainer` too, so they count.
    fn chat_has_children(&self) -> bool {
        self.chat_flushed
            || !self.pending.is_empty()
            || self.streaming.is_some()
            || self.thinking.is_some()
            || !self.active_tools.is_empty()
            || self.bash.is_some()
    }

    /// Append a user message. When `text` is a `<skill …>` block (a `/skill:name` expansion), it is
    /// split into a collapsible `[skill]` invocation message plus the trailing user message, exactly
    /// as Pi renders the `user` role (`parseSkillBlock` → `SkillInvocationMessageComponent` +
    /// `UserMessageComponent`, interactive-mode.ts:3112-3132). Plain text falls through to a single
    /// user entry.
    ///
    /// The leading `Spacer(1)` is gated on `this.chatContainer.children.length > 0` (`:3500`), so
    /// the first message of a fresh session gets none; the answer is frozen into the entry because
    /// the render happens later, after `drain_committed` has already emptied `pending`. The user
    /// message that trails a skill block is added at `:3513-3521` with its own **unconditional**
    /// spacer, so it always carries one.
    pub fn push_user(&mut self, text: impl Into<String>) {
        self.bump_render_generation();
        let text = text.into();
        let lead_spacer = self.chat_has_children();
        if let Some(block) = parse_skill_block(&text) {
            self.pending.push(Entry::SkillInvocation {
                name: block.name,
                content: block.content,
                lead_spacer,
            });
            if let Some(user_message) = block.user_message {
                self.pending.push(Entry::User { text: user_message, lead_spacer: true });
            }
        } else {
            self.pending.push(Entry::User { text, lead_spacer });
        }
        // A fresh prompt jumps the active region back to the tail (spec/tui/07 auto-scroll).
        self.scroll_offset = 0;
    }

    /// Page the active region up by `page` visual lines (`PageUp`): reveal earlier streamed/tool/bash
    /// output. Clamped against the content height at render time.
    pub fn page_up(&mut self, page: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(page.max(1));
    }

    /// Page the active region down by `page` visual lines (`PageDown`); `0` is the pinned tail.
    pub fn page_down(&mut self, page: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(page.max(1));
    }

    /// The current page-scroll offset from the tail (test/inspection access).
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Append a chunk of assistant text to the in-flight streaming buffer (R-10-028 streaming).
    pub fn push_assistant_delta(&mut self, delta: &str) {
        self.bump_render_generation();
        match &mut self.streaming {
            Some(buf) => buf.push_str(delta),
            None => self.streaming = Some(delta.to_string()),
        }
    }

    /// Finalize the assistant turn. If `text` is given it replaces the streaming buffer (e.g. the
    /// authoritative terminal message); otherwise the accumulated streaming buffer is committed.
    ///
    /// L3 — the gate is **whitespace-only**, not empty. Pi renders a text block at all only when
    /// `content.text.trim()` is truthy (`assistant-message.ts:107`) and counts it toward
    /// `hasVisibleContent` by the same trimmed test (`:96-98`), so a message of nothing but spaces
    /// produces no `Markdown` child and no leading `Spacer(1)`. Testing `!t.is_empty()` let `"   "`
    /// through and gave it a blank upstream never emits.
    pub fn commit_assistant(&mut self, text: Option<String>) {
        self.bump_render_generation();
        let final_text = text.or_else(|| self.streaming.take());
        self.streaming = None;
        if let Some(t) = final_text
            && !t.trim().is_empty()
        {
            self.pending.push(Entry::Assistant(t));
        }
    }

    /// Drop any in-flight streaming partial without committing (abort, R-10-030). Drops the live
    /// reasoning buffer too — an aborted turn shows neither its partial answer nor its partial
    /// thinking.
    pub fn discard_streaming(&mut self) {
        self.bump_render_generation();
        self.streaming = None;
        self.thinking = None;
    }

    /// Append a streamed chunk of assistant **reasoning** to the in-flight thinking buffer
    /// (`StreamEvent::ThinkingDelta`, provider `stream.rs:413`). Pi renders the thinking blocks of a
    /// turn as their own section (`assistant-message.ts:115-166`), so the buffer is kept apart from
    /// the answer text.
    pub fn push_thinking_delta(&mut self, delta: &str) {
        self.bump_render_generation();
        match &mut self.thinking {
            Some(buf) => buf.push_str(delta),
            None => self.thinking = Some(delta.to_string()),
        }
    }

    /// The current reasoning partial, if a turn is thinking (test/inspection access).
    pub fn thinking(&self) -> Option<&str> {
        self.thinking.as_deref()
    }

    /// Finalize the turn's reasoning. `text` (the authoritative `thinking` blocks of the terminal
    /// message, coalesced by [`thinking_text`]) replaces the streamed buffer when given; otherwise
    /// the accumulated buffer commits. Whitespace-only reasoning commits nothing, exactly as Pi
    /// skips a run whose trimmed blocks are all empty (`assistant-message.ts:128-130`).
    ///
    /// The `hideThinkingBlock` choice is frozen into the entry here — see [`Entry::Thinking`].
    pub fn commit_thinking(&mut self, text: Option<String>) {
        self.bump_render_generation();
        let final_text = text.or_else(|| self.thinking.take());
        self.thinking = None;
        if let Some(t) = final_text
            && !t.trim().is_empty()
        {
            self.pending.push(Entry::Thinking { text: t, hidden: self.hide_thinking });
        }
    }

    /// Set `hideThinkingBlock` live (Pi `setHideThinkingBlock`, assistant-message.ts:57-62). Affects
    /// the live reasoning block and every entry committed afterwards; already-flushed scrollback is
    /// immutable (see [`Entry::Thinking`]).
    pub fn set_hide_thinking_block(&mut self, hide: bool) {
        self.bump_render_generation();
        self.hide_thinking = hide;
    }

    /// Whether the reasoning body is collapsed to the `Thinking...` label (test/inspection access).
    pub fn hide_thinking_block(&self) -> bool {
        self.hide_thinking
    }

    /// Pi `setHiddenThinkingLabel(label?)` (`extensions/types.ts:167` @v0.83.0; the interactive body
    /// is `interactive-mode.ts:2118-2129` @v0.84.2, which assigns `label ?? this.defaultHiddenThinkingLabel`
    /// and re-broadcasts to every already-mounted assistant component). `None` restores
    /// [`HIDDEN_THINKING_LABEL`]. See [`Self::hidden_thinking_label`] for why this is paint-time
    /// state rather than a value frozen at commit.
    pub fn set_hidden_thinking_label(&mut self, label: Option<String>) {
        self.bump_render_generation();
        self.hidden_thinking_label = label;
    }

    /// The label a collapsed reasoning block currently renders — the extension's override, else
    /// [`HIDDEN_THINKING_LABEL`]. Read by the shell when flushing committed entries to scrollback,
    /// so a pending entry and the live block cannot disagree.
    pub fn hidden_thinking_label(&self) -> &str {
        self.hidden_thinking_label.as_deref().unwrap_or(HIDDEN_THINKING_LABEL)
    }

    /// Record a tool starting (live in the viewport): name + the raw call args (`ToolExecutionStart`).
    /// The args drive the per-tool `renderCall` header (path/command/pattern/range/…).
    ///
    /// Prefer [`Self::push_tool_start_rendered`] with the call's `toolCallId` wherever one is in
    /// hand — see [`ToolRun::call_id`]. This id-less form pairs its result by tool name alone, which
    /// cannot distinguish two concurrent calls to the same tool.
    pub fn push_tool_start(&mut self, name: impl Into<String>, args: Value) {
        self.bump_render_generation();
        self.push_tool_start_rendered(name, None, args, None);
    }

    /// [`Self::push_tool_start`] with the call's `toolCallId` and the CALL text an extension's
    /// registered renderer produced (EXT-006).
    ///
    /// `call_id` is the key the matching result is resolved by ([`ToolRun::call_id`]; Pi files each
    /// `ToolExecutionComponent` under `content.id`, interactive-mode.ts:3473). `rendered` replaces
    /// the built-in per-tool header for this run; `None` keeps the built-in dispatch (Pi prefers the
    /// extension's `renderCall` when the tool declares one, tool-execution.ts:81-112).
    pub fn push_tool_start_rendered(
        &mut self,
        name: impl Into<String>,
        call_id: Option<String>,
        args: Value,
        rendered: Option<String>,
    ) {
        self.bump_render_generation();
        self.active_tools.push(ToolRun {
            name: name.into(),
            call_id,
            args,
            result: None,
            is_error: false,
            done: false,
            started_at: Some(std::time::Instant::now()),
            duration_ms: None,
            rendered_call: rendered,
            rendered_result: None,
            preview: None,
            images: Vec::new(),
        });
    }

    /// Update a running tool's partial result (`ToolExecutionUpdate`): the raw partial result value,
    /// rendered by the tool's `renderResult` with `isPartial = true`.
    ///
    /// Routed to the run whose [`call_id`](ToolRun::call_id) matches, as Pi does
    /// (`this.pendingTools.get(event.toolCallId)`, interactive-mode.ts:3104); `None` falls back to
    /// the latest still-running tool.
    pub fn push_tool_update(&mut self, call_id: Option<&str>, partial: Option<Value>) {
        self.bump_render_generation();
        let run = match call_id {
            Some(id) => self.active_tools.iter_mut().find(|r| !r.done && r.call_id.as_deref() == Some(id)),
            None => self.active_tools.iter_mut().rev().find(|r| !r.done),
        };
        if let Some(run) = run
            && partial.is_some()
        {
            run.result = partial;
        }
    }

    /// Whether any live tool run is currently drawing a ticking `Elapsed …` figure, i.e. whether the
    /// frame goes stale on its own and must be repainted on a timer.
    ///
    /// This is Pi's `setInterval(() => context.invalidate(), 1000)` condition, verbatim: bash's
    /// `renderResult` arms that interval exactly when `state.startedAt !== undefined &&
    /// options.isPartial` and clears it on the final result (bash.ts:471-479). The `result.is_some()`
    /// term is upstream's `if (this.result)` gate on `renderResult` running at all
    /// (tool-execution.ts:281) — bash's initial empty update satisfies it immediately (bash.ts:384).
    ///
    /// Gates [`crate::App::run`]'s elapsed tick, so an idle session — or one running any tool but
    /// `bash` — never pays for a redraw.
    pub fn has_running_elapsed_tool(&self) -> bool {
        self.active_tools
            .iter()
            .any(|r| !r.done && r.name == "bash" && r.started_at.is_some() && r.result.is_some())
    }

    /// Attach `edit`'s pre-execution diff preview to a still-running call — Pi `setEditPreview`
    /// (edit.ts:263-280), the sink its `renderCall`'s `computeEditsDiff(...).then(...)` writes into
    /// (`:378-386`).
    ///
    /// `preview` is `Ok(diff)` or `Err(message)` (Pi's `EditDiffResult | EditDiffError`). Routed by
    /// `toolCallId` like every other per-run update ([`Self::push_tool_update`]); `None` falls back
    /// to the latest still-running tool. A run that has already finished is skipped — Pi drops a
    /// late preview by comparing `previewArgsKey` against the request key (`:381`), and once the
    /// result is in it is the result diff that renders (`formatEditResult`, `:220-226`).
    pub fn set_edit_preview(&mut self, call_id: Option<&str>, preview: Result<String, String>) {
        self.bump_render_generation();
        let run = match call_id {
            Some(id) => {
                self.active_tools.iter_mut().find(|r| !r.done && r.call_id.as_deref() == Some(id))
            }
            None => self.active_tools.iter_mut().rev().find(|r| !r.done),
        };
        if let Some(run) = run {
            run.preview = Some(preview);
        }
    }

    /// Record a tool finishing: attach the raw result/error to the matching live run, else a fresh
    /// done entry so a missed start never drops the result. Freezes the run duration for the bash
    /// `Took …` footer.
    ///
    /// Prefer [`Self::push_tool_end_rendered`] with the result's `toolCallId` — see
    /// [`ToolRun::call_id`] and [`Self::pending_run_mut`].
    pub fn push_tool_end(&mut self, name: impl Into<String>, is_error: bool, result: Option<Value>) {
        self.bump_render_generation();
        self.push_tool_end_rendered(name, None, is_error, result, None);
    }

    /// [`Self::push_tool_end`] with the result's `toolCallId` and the RESULT text an extension's
    /// registered renderer produced (EXT-006; Pi `renderResult`, extensions/types.ts:475-481).
    ///
    /// `call_id` selects the run this result belongs to — Pi's
    /// `renderedPendingTools.get(message.toolCallId)` (interactive-mode.ts:3483) / `pendingTools.get
    /// (event.toolCallId)` (`:3113`). `rendered = None` keeps the built-in body.
    pub fn push_tool_end_rendered(
        &mut self,
        name: impl Into<String>,
        call_id: Option<&str>,
        is_error: bool,
        result: Option<Value>,
        rendered: Option<String>,
    ) {
        self.bump_render_generation();
        let name = name.into();
        // Decode the result's `image` content blocks ONCE here (`tool-execution.ts:331-350`), not on
        // every frame — a screenshot-sized PNG must never be re-decoded per redraw.
        let images = result.as_ref().map(decode_result_images).unwrap_or_default();
        if let Some(run) = self.pending_run_mut(&name, call_id) {
            run.done = true;
            run.is_error = is_error;
            run.result = result;
            run.duration_ms = run.started_at.map(|s| s.elapsed().as_millis() as u64);
            run.rendered_result = rendered;
            run.images = images;
        } else {
            self.active_tools.push(ToolRun {
                name,
                call_id: call_id.map(str::to_string),
                args: Value::Null,
                result,
                is_error,
                done: true,
                started_at: None,
                duration_ms: None,
                rendered_call: None,
                rendered_result: rendered,
                preview: None,
                images,
            });
        }
    }

    /// Resolve the still-running tool run a result belongs to.
    ///
    /// Pi's rule, exactly: a result is looked up by its `toolCallId` and by nothing else
    /// (interactive-mode.ts:3483 on replay, `:3113` live), because one assistant turn routinely
    /// issues several calls to the SAME tool and only the id tells them apart.
    ///
    /// The two fallbacks below never fire for a real provider turn (every `ToolCall` carries an
    /// `id`); they exist so a caller with no id in hand — a test, or a `ToolExecutionEnd` whose
    /// start was dropped — still lands somewhere sensible rather than nowhere:
    ///
    /// * `call_id: Some(id)` matches that id; failing that, a same-name run that carries NO id at
    ///   all (an id-less start being completed by an id-carrying end). It never falls back to a run
    ///   bearing a *different* id — that is precisely the mispairing this exists to prevent.
    /// * `call_id: None` takes the latest still-running run of that name (the pre-id behavior).
    fn pending_run_mut(&mut self, name: &str, call_id: Option<&str>) -> Option<&mut ToolRun> {
        match call_id {
            Some(id) => {
                if let Some(idx) = self
                    .active_tools
                    .iter()
                    .position(|r| !r.done && r.call_id.as_deref() == Some(id))
                {
                    return self.active_tools.get_mut(idx);
                }
                self.active_tools
                    .iter_mut()
                    .rev()
                    .find(|r| !r.done && r.call_id.is_none() && r.name == name)
            }
            None => self.active_tools.iter_mut().rev().find(|r| !r.done && r.name == name),
        }
    }

    /// Commit the active turn's tool executions into scrollback (called when the turn ends). Each
    /// becomes an [`Entry::Tool`]; still-running tools are committed as-is (marked done).
    pub fn commit_tools(&mut self) {
        self.bump_render_generation();
        for mut run in self.active_tools.drain(..) {
            run.done = true;
            self.pending.push(Entry::Tool(run));
        }
    }

    /// Progressively commit the LEADING run of already-finished tool executions to scrollback WHILE
    /// the turn is still live, so the inline viewport keeps only the actively-running tail (the
    /// currently-executing tool + any tools queued behind it) instead of stacking every completed
    /// tool of a long multi-tool turn until `AgentEnd`. This is the faithful port of Pi's behavior:
    /// each `ToolExecutionComponent` (packages/coding-agent/src/modes/interactive/components/
    /// tool-execution.ts:13) is a persistent child that, as later tool/text lines append below it,
    /// scrolls up past the diff's viewport top and lives in native scrollback thereafter
    /// (packages/tui/src/tui.ts:1455 `if (firstChanged < prevViewportTop) fullRender`). cyrup's
    /// idiomatic-ratatui equivalent (ADR-0001) is `Terminal::insert_before` for each finished entry —
    /// driven here by moving the finished leading tools into `pending` for the next
    /// [`drain_committed`](Self::drain_committed) → `insert_before` flush.
    ///
    /// Only the LEADING contiguous run of `done` tools is drained (stopping at the first still-running
    /// tool), so scrollback order always equals call order even under hypothetical parallel/interleaved
    /// tools — a still-running earlier tool blocks committing a finished later one ahead of it.
    ///
    /// Guarded on `streaming.is_none()`: a tool is never committed ahead of still-uncommitted assistant
    /// text of the same step. The confirmed event ordering (the assistant stream's terminal
    /// `StreamEvent::Done` → `commit_assistant` fires BEFORE any `ToolExecutionStart` of that step)
    /// keeps `streaming` clear whenever a tool finishes, so this guard is a safety net that also holds
    /// under interleaving.
    pub fn commit_finished_leading_tools(&mut self) {
        self.bump_render_generation();
        if self.streaming.is_some() {
            return;
        }
        let split = self
            .active_tools
            .iter()
            .position(|run| !run.done)
            .unwrap_or(self.active_tools.len());
        for run in self.active_tools.drain(..split) {
            self.pending.push(Entry::Tool(run));
        }
    }

    /// The active (live) tool executions for the current turn (test/inspection access).
    pub fn active_tools(&self) -> &[ToolRun] {
        &self.active_tools
    }

    /// Toggle the tool-output expansion (`Ctrl+O`); returns the new state.
    pub fn toggle_tool_expanded(&mut self) -> bool {
        self.bump_render_generation();
        self.tool_expanded = !self.tool_expanded;
        self.tool_expanded
    }

    /// Set the tool-output expansion absolutely — Pi `setToolsExpanded(expanded)`
    /// (`interactive-mode.ts:3887-3903`), the extension-driven counterpart of the `Ctrl+O` toggle.
    /// Returns whether the value actually changed (Pi's `if (expanded === this.toolOutputExpanded)
    /// return` early-out, `:3888`), which the caller uses to decide whether to echo Pi's
    /// `Tool output: expanded|collapsed` status line.
    pub fn set_tool_expanded(&mut self, expanded: bool) -> bool {
        self.bump_render_generation();
        let changed = self.tool_expanded != expanded;
        self.tool_expanded = expanded;
        changed
    }

    /// The live tool-output expansion (Pi `this.toolOutputExpanded`, `interactive-mode.ts:442`).
    ///
    /// X14 — read by the shell when it builds the [`ImageOpts`] it flushes committed entries with,
    /// so a branch/compaction summary honours the flag in force when it is PAINTED rather than the
    /// one that happened to be set when it was pushed (`setToolsExpanded`'s re-broadcast to every
    /// `chatContainer` child, `:4032-4046`).
    pub fn tool_expanded(&self) -> bool {
        self.tool_expanded
    }

    /// Record a status / notification line.
    pub fn push_status(&mut self, text: impl Into<String>) {
        self.pending.push(Entry::Status(text.into()));
    }

    /// Push the startup loaded-resources / diagnostics panel (Pi `showLoadedResources`,
    /// interactive-mode.ts:1480-1690). No-op when there is nothing to show — a `quietStartup` boot
    /// with no problems prints nothing at all, exactly like Pi.
    pub fn push_loaded_resources(&mut self, lines: Vec<crate::startup::StartupLine>) {
        if lines.is_empty() {
            return;
        }
        self.pending.push(Entry::LoadedResources(lines));
    }

    /// Record an `error`-styled notice line — the incomplete/failed-turn footer Pi appends to an
    /// assistant message (`assistant-message.ts:177-201`). Distinct from
    /// [`push_status`](Self::push_status), which is dim and bulleted.
    pub fn push_error(&mut self, text: impl Into<String>) {
        self.pending.push(Entry::Error(text.into()));
    }

    /// Record a `warning`-styled notice line (Pi `showWarning`, `interactive-mode.ts:3956-3960`).
    pub fn push_warning(&mut self, text: impl Into<String>) {
        self.pending.push(Entry::Warning(text.into()));
    }

    /// Push a bordered info block (`/hotkeys`, `/changelog`, `/session`, `/debug`).
    pub fn push_block(&mut self, title: impl Into<String>, markdown: impl Into<String>) {
        self.pending.push(Entry::Block { title: title.into(), markdown: markdown.into() });
    }

    /// The startup "packages are out of date" notice — Pi `showPackageUpdateNotification`
    /// (`interactive-mode.ts:3920-3936`), pushed when the detached package-update check settles with
    /// a non-empty list (`:850-856`).
    ///
    /// Upstream's block is a `DynamicBorder`, a bold title, the instruction, `Packages:` and one
    /// `- name` line per package, then a closing border — structurally [`Entry::Block`], which is the
    /// same border/title/body sandwich (interactive-mode.ts:5502-5507). `[CYRUP-DELTA]`: upstream
    /// tints THIS block's border and title `warning` where the generic block is `accent`; cyrup
    /// reuses the generic block rather than forking the entry type for a colour.
    ///
    /// The action names cyrup's own command, `cyrup update --extensions` (`subcommands.rs`), which is
    /// upstream's `${APP_NAME} update --extensions` after the rebrand. A no-op on an empty list, so
    /// the caller never has to guard.
    pub fn push_package_updates(&mut self, packages: &[String]) {
        if packages.is_empty() {
            return;
        }
        let list = packages
            .iter()
            .map(|p| format!("- {p}"))
            .collect::<Vec<_>>()
            .join("\n");
        self.push_block(
            "Package Updates Available",
            format!(
                "Package updates are available. Run {} update --extensions\nPackages:\n{list}",
                crate::resume_hint::APP_NAME
            ),
        );
    }

    /// Push a skill-invocation message (`skill-invocation-message.ts`): a `[skill]` label + the skill
    /// name header, with the skill block content rendered as markdown.
    ///
    /// Upstream only ever builds this component inside `case "user"` (`interactive-mode.ts:3506`),
    /// so it takes the same `:3500` leading-spacer gate [`push_user`](Self::push_user) applies.
    pub fn push_skill_invocation(&mut self, name: impl Into<String>, content: impl Into<String>) {
        let lead_spacer = self.chat_has_children();
        self.pending.push(Entry::SkillInvocation {
            name: name.into(),
            content: content.into(),
            lead_spacer,
        });
    }

    /// Push a custom (extension) message (`custom-message.ts`): a bracketed type `label` + a markdown
    /// `body`.
    pub fn push_custom_message(&mut self, label: impl Into<String>, body: impl Into<String>) {
        self.pending.push(Entry::Custom {
            label: label.into(),
            body: body.into(),
            rendered: Rendered::None,
        });
    }

    /// [`Self::push_custom_message`] with the text an extension's registered message renderer
    /// produced for this custom type (EXT-006; Pi resolves the renderer at
    /// `interactive-mode.ts:3326` — `extensionRunner.getMessageRenderer(message.customType)` — and
    /// hands it to `CustomMessageComponent` INSTEAD of the default framing). When `rendered` is
    /// [`Rendered::Text`], the extension's lines are emitted verbatim: no `[label]` bracket, no
    /// markdown re-wrap, because the renderer already decided how the block looks;
    /// [`Rendered::Failed`] draws Pi's renderer-failure box (X15).
    pub fn push_custom_message_rendered(
        &mut self,
        label: impl Into<String>,
        body: impl Into<String>,
        rendered: Rendered,
    ) {
        self.pending.push(Entry::Custom {
            label: label.into(),
            body: body.into(),
            rendered,
        });
    }

    /// Push a branch-summary message (`branch-summary-message.ts`): the `**Branch Summary**` body
    /// produced when navigating away from / abandoning a branch.
    ///
    /// X14 — the collapsed/expanded choice is `component.setExpanded(this.toolOutputExpanded)`
    /// (`interactive-mode.ts:3493`) and is re-broadcast to every child on every toggle
    /// (`setToolsExpanded`, `:4032-4046`), so it is resolved at RENDER time from
    /// [`ImageOpts::tools_expanded`], never captured here.
    pub fn push_branch_summary(&mut self, summary: impl Into<String>) {
        self.pending.push(Entry::BranchSummary { summary: summary.into() });
    }

    /// Push a compaction-summary message (`compaction-summary-message.ts`): the pre-compaction token
    /// count + the `**Compacted from N tokens**` summary body.
    pub fn push_compaction_summary(&mut self, tokens_before: u64, summary: impl Into<String>) {
        // X14 — `interactive-mode.ts:3486`'s `setExpanded(this.toolOutputExpanded)`; like the branch
        // summary above, resolved at render time from the LIVE flag.
        self.pending.push(Entry::CompactionSummary { tokens_before, summary: summary.into() });
    }

    /// Set the live `app.tools.expand` key label every `… to expand` hint resolves through — Pi's
    /// `keyText("app.tools.expand")` (`keybinding-hints.ts:34-36`), which reads the keymap on every
    /// render. cyrup's transcript holds no keymap, so the app pushes the resolved label here
    /// whenever bindings change (X9). `None` restores cyrup's default binding label.
    pub fn set_expand_hint(&mut self, label: Option<String>) {
        self.bump_render_generation();
        self.expand_hint = label;
    }

    /// The label [`Self::set_expand_hint`] stored, or [`EXPAND_KEY`].
    pub fn expand_key(&self) -> &str {
        self.expand_hint.as_deref().unwrap_or(EXPAND_KEY)
    }

    /// Point the tool renderers at the SESSION's working directory — Pi `ToolRenderContext.cwd`
    /// (`tool-execution.ts:126`), which `read`'s compact classification resolves its path against
    /// (`read.ts:336`). `None` falls back to the process cwd.
    pub fn set_cwd(&mut self, cwd: Option<std::path::PathBuf>) {
        self.bump_render_generation();
        self.cwd = cwd;
    }

    /// The cwd [`Self::set_cwd`] stored.
    pub fn cwd(&self) -> Option<&std::path::Path> {
        self.cwd.as_deref()
    }

    /// Build the styled lines the inline viewport renders: **only** the active streaming partial,
    /// rendered as markdown (spec/tui/06 §8). Committed entries live in native scrollback (see
    /// [`drain_committed`](Self::drain_committed)).
    ///
    /// Pi renders the in-flight assistant message **inline** with no surrounding box/title
    /// (`assistant-message.ts:84-93`) and with **no** streaming caret — the only caret in the TUI is
    /// the editor's reverse-video cell (`editor.ts:545-564`), and `git grep "▌" v0.84.1 --
    /// packages/` finds a single hit, the pupil of an eye in
    /// `examples/extensions/custom-header.ts:22` (X1). The buffer is run through
    /// [`trim_partial_closing_fence`](crate::markdown::trim_partial_closing_fence) so a streaming code
    /// fence does not flicker open/closed (`markdown.ts:25-48`).
    /// Invalidate the render cache. Called by every mutator on the bump list below; the next
    /// [`cached_render`](Self::cached_render) misses on the generation key and recomputes once.
    /// `wrapping_add`: a plain `+ 1` is a debug-build overflow panic (denied lints aside) after
    /// 2^64 bumps; wrapping can only alias a cache entry built 2^64 generations ago, which no
    /// session survives to observe.
    fn bump_render_generation(&mut self) {
        self.render_generation = self.render_generation.wrapping_add(1);
    }

    /// Invalidate the render cache for a timer-driven repaint (TUI-092 F2): the live `!`/`!!`
    /// block's spinner glyph ([`BashExecution::render_lines`] → `started.elapsed()`,
    /// bash.rs:204) and a running bash tool's `Elapsed …` footer (`render_bash`,
    /// transcript.rs:2157) are computed from wall-clock time INSIDE `lines()`, so a frame that
    /// mutates nothing must still re-materialise while time-derived content is live. Called by
    /// the run loop's spinner tick (gated on `bash_running()`) and elapsed tick (gated on
    /// `has_running_elapsed_tool()`) — never on content-quiet frames, which stay free.
    pub fn bump_render_tick(&mut self) {
        self.bump_render_generation();
    }

    /// The materialised active region, recomputed only when (generation, width, theme) changed.
    /// The borrows are strictly sequential — check, build, assign, lend — so this is plain NLL,
    /// no Polonius case, and the final reborrow needs no `unwrap`/`expect` (no-panic policy):
    /// `render_cache` is a value, not an `Option`.
    fn cached_render(&mut self, width: usize, theme: &UiTheme) -> &RenderCache {
        let stale = self.render_cache.generation != self.render_generation
            || self.render_cache.width != width
            || self.render_cache.theme_generation != theme.generation;
        if stale {
            let lines = self.lines(width, theme); // the current body, unchanged
            // Measure WRAPPED display rows, not logical lines: `markdown::render` emits ONE
            // un-wrapped `Line` per prose paragraph, so counting `lines.len()` under-counts a long
            // streaming paragraph. `wrapped_height` measures with the SAME word-wrap `render`
            // applies (`Paragraph::line_count`). (Moved verbatim from the old `content_height`
            // body, transcript.rs:1138-1143.)
            let wrapped_height = wrapped_height(&lines, width);
            self.render_cache = RenderCache {
                generation: self.render_generation,
                width,
                theme_generation: theme.generation,
                lines,
                wrapped_height,
            };
        }
        &self.render_cache
    }

    /// The number of visual lines the active turn occupies at `width` — the message region's content
    /// height, used to **content-size** the inline viewport (ADR-0001 commitment #1, audit #1) so the
    /// empty turn never balloons into a void. `0` when nothing is streaming.
    pub fn content_height(&mut self, width: usize, theme: &UiTheme) -> usize {
        self.cached_render(width, theme).wrapped_height
    }

    fn lines(&self, width: usize, theme: &UiTheme) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        // The live reasoning block renders ABOVE the answer text, the order Pi's content walk
        // produces for a reasoning model (thinking blocks precede the text blocks of the turn) —
        // `assistant-message.ts:115-166`.
        // L3 — `assistant-message.ts:100-102`'s single `Spacer(1)`, emitted once for the whole
        // message when `hasVisibleContent`. The live view is the same component mid-stream, so the
        // blank belongs to whichever of the two blocks comes first.
        //
        // `hasVisibleContent` (`:96-98`) is
        // `content.some(c => (c.type === "text" && c.text.trim()) || (c.type === "thinking" &&
        // c.thinking.trim()))` — a **trimmed** test on both legs, and `:107` gates the text block's
        // `Markdown` child on the same `content.text.trim()`. So an assistant turn that has streamed
        // nothing but whitespace is not visible content: no blank, and no body either.
        let thinking_visible = self.thinking.as_ref().is_some_and(|t| !t.trim().is_empty());
        let stream_visible = self.streaming.as_ref().is_some_and(|s| !s.trim().is_empty());
        if thinking_visible || stream_visible {
            lines.push(Line::default());
        }
        // `:129-131` — `if (thinkingBlocks.length === 0) { continue; }` runs BEFORE the
        // `hideThinkingBlock` branch, and `:122-125` only collects blocks whose `.trim()` is
        // non-empty. So a reasoning run of nothing but whitespace renders nothing at all, not even
        // the `Thinking...` label. `commit_thinking` already applies that test; the live leg did
        // not, so a stray whitespace `ThinkingDelta` put a label (and its blank) on screen.
        if let Some(thinking) = self.thinking.as_ref().filter(|_| thinking_visible) {
            let mut td = thinking_lines(
                thinking,
                self.hide_thinking,
                width.saturating_sub(self.output_pad * 2),
                theme,
                self.hidden_thinking_label(),
            );
            if !td.is_empty() {
                pad_lines(&mut td, self.output_pad);
                lines.extend(td);
                // Pi's `hasVisibleContentAfter` spacer (`:134-137`): a blank only when more visible
                // assistant content follows — and "visible" is the same trimmed test (`:137`).
                if stream_visible {
                    lines.push(Line::default());
                }
            }
        }
        if let Some(partial) = self.streaming.as_ref().filter(|_| stream_visible) {
            // X1 — no role label and no `▌` caret. `assistant-message.ts:104-114` adds exactly one
            // child per text block, `new Markdown(content.text.trim(), this.outputPad, 0, …)`; the
            // only caret in the whole TUI is the editor's reverse-video cell (`editor.ts:545-564`).
            // `git grep "▌" v0.84.1 -- packages/` finds one hit, and it is the pupil of an eye in
            // `examples/extensions/custom-header.ts:22`.
            //
            // M9 — the width follows from that: `markdown.ts:284` `contentWidth = width -
            // paddingX * 2`, i.e. `width - outputPad * 2`, where the old `width - (11 + outputPad)`
            // was budgeting for the deleted `"assistant: "`.
            let body = crate::markdown::trim_partial_closing_fence(partial);
            let mut md = crate::markdown::render(
                &body,
                width.saturating_sub(self.output_pad * 2).max(1),
                theme,
            );
            if md.is_empty() {
                md.push(Line::default());
            }
            pad_lines(&mut md, self.output_pad);
            lines.extend(md);
        }
        // Live tool executions render below the streaming partial, honoring the expand flag so
        // `Ctrl+O` toggles their result body in the viewport before the turn commits.
        for run in &self.active_tools {
            lines.extend(tool_lines(
                run,
                self.tool_expanded,
                width,
                theme,
                ImageOpts {
                    show: self.show_images,
                    graphical: self.graphical_images,
                    width_cells: self.image_width_cells,
                    expand_key: self.expand_key(),
                    cwd: self.cwd.as_deref(),
                    tools_expanded: self.tool_expanded,
                    // A tool block draws no reasoning, so this is inert here — carried only so the
                    // bag has one construction shape.
                    hidden_thinking_label: None,
                },
            ));
        }
        // The live `!`/`!!` bash block renders last (`bash-execution.ts` sits in the message region).
        if let Some(b) = &self.bash {
            lines.extend(b.render_lines(
                width,
                theme,
                self.bash_cancel_hint.as_deref(),
                self.bash_expand_hint.as_deref(),
            ));
        }
        lines
    }
}

/// Pi's default read/write byte + line truncation limits (`truncate.ts:11-12`).
const DEFAULT_MAX_BYTES: u64 = 50 * 1024;
/// The **fallback** `app.tools.expand` label, used only when the caller supplied no live one —
/// cyrup's own default binding (`keymap.rs:378`, `Key::ctrl('o')`).
///
/// X9: this used to be the label itself, hard-coded at every hint site, so a user who rebound
/// `app.tools.expand` still read `ctrl+o` on screen. Pi resolves the binding at render time
/// (`keyText(keybinding)` → `getKeybindings().getKeys(...)`, `keybinding-hints.ts:34-36`); the live
/// label now rides in on [`ImageOpts::expand_key`].
const EXPAND_KEY: &str = "ctrl+o";

/// Pi's `terminal.imageWidthCells` default (settings-manager.ts:1060-1066) — the cell width an
/// inline tool-result image is clamped to (`maxWidthCells`, tool-execution.ts:348).
pub const DEFAULT_IMAGE_WIDTH_CELLS: u16 = 60;

/// Upper bound (px, either side) a tool-result image is downscaled to when it is decoded. A
/// half-block raster is at most a few dozen cells wide, so nothing above this is ever visible — and
/// the bound is what keeps the per-frame clone+resize of a screenshot-sized PNG off the render path.
const MAX_RASTER_PX: u32 = 1024;

/// Pi's `hiddenThinkingLabel` default (`assistant-message.ts:29`) — the single static line shown in
/// place of the reasoning body when `hideThinkingBlock` is on.
pub const HIDDEN_THINKING_LABEL: &str = "Thinking...";

/// Render one run of assistant reasoning (`assistant-message.ts:139-165`): the static
/// [`HIDDEN_THINKING_LABEL`] when `hidden`, otherwise the coalesced thinking body.
///
/// X5. The hidden form is a plain `new Text(theme.italic(theme.fg("thinkingText", label)),
/// outputPad, 0)` (`:141-143`) — one styled line. The **body** is a real
/// `new Markdown(thinkingBlocks.join("\n\n"), this.outputPad, 0, this.markdownTheme,
/// { color: (text) => theme.fg("thinkingText", text), italic: true }, …)` (`:146-164`).
///
/// The `{ color, italic }` pair reaches only `applyDefaultStyle` (`markdown.ts:377-404`), which
/// `renderToken` hands to the `paragraph`/`text` arms alone: a `heading` builds its own style
/// context (`:470-480`) and a fenced `code` block never consults one (`:520-539`). So `## Plan`
/// keeps `mdHeading` and a fence keeps its border + syntax colours — a thinking block is markdown,
/// not a flat grey wall. The doc comment that used to sit here claimed upstream "forces every span
/// to the one colour regardless of markdown structure"; that claim was false, and it was the
/// justification for splitting the body on `\n` and never calling the markdown renderer.
fn thinking_lines(
    text: &str,
    hidden: bool,
    width: usize,
    theme: &UiTheme,
    label: &str,
) -> Vec<Line<'static>> {
    let style = theme.thinking_text_style();
    if hidden {
        return vec![Line::styled(label.to_string(), style)];
    }
    let body = text.trim();
    if body.is_empty() {
        return Vec::new();
    }
    crate::markdown::render_with_default_style(body, width.max(1), theme, style.fg, true)
}

/// Render one tool execution into styled lines by dispatching on the tool name to its Pi-specific
/// `renderCall`/`renderResult` (`tool-execution.ts` composes each built-in's renderers, not a generic
/// one-liner): edit → a self-diff (`edit.ts:390`), bash → an output tail + truncation + `Took …`
/// (`bash.ts:440`), read → a line-range header + a hidden-until-expanded body (`read.ts:329/339`),
/// write → a content preview (`write.ts:227`), grep/find/ls → a match/entry list with limit notices
/// (`grep.ts:370`, `find.ts:359`, `ls.ts:210`). The whole block is tinted by execution state
/// (`toolPendingBg`/`toolSuccessBg`/`toolErrorBg`, tool-execution.ts:253-258) — the bg is the state
/// affordance (Pi has no gear/check glyph), preceded by an untinted blank (the component's `Spacer(1)`,
/// tool-execution.ts:63).
pub(crate) fn tool_lines(
    run: &ToolRun,
    expanded: bool,
    width: usize,
    theme: &UiTheme,
    images: ImageOpts,
) -> Vec<Line<'static>> {
    let mut block: Vec<Line<'static>> = Vec::new();
    // EXT-006: an extension that registered a renderer for THIS tool name owns the block (Pi
    // prefers the extension's `renderCall`/`renderResult` over the built-in's,
    // tool-execution.ts:81-112). Checked before the built-in dispatch so an extension can also
    // override how a BUILT-IN tool draws, exactly as Pi's definition-registry override does.
    if run.rendered_call.is_some() || run.rendered_result.is_some() {
        render_extension(run, expanded, theme, &mut block);
    } else {
        match run.name.as_str() {
            "read" => render_read(run, expanded, theme, images, &mut block),
            "write" => render_write(run, expanded, theme, images.expand_key, &mut block),
            "edit" => render_edit(run, theme, &mut block),
            "bash" => render_bash(run, expanded, theme, images.expand_key, &mut block),
            "grep" => render_grep(run, expanded, theme, images.expand_key, &mut block),
            "find" => render_find(run, expanded, theme, images.expand_key, &mut block),
            "ls" => render_ls(run, expanded, theme, images.expand_key, &mut block),
            _ => render_generic(run, theme, &mut block),
        }
    }
    // `image` content blocks (`tool-execution.ts:330-350`). Pi adds a real `Image` component per
    // block when `caps.images && showImages`, and otherwise `getTextOutput` appends the
    // `imageFallback` indicator to the text body (render-utils.ts:49-59). The two cases split around
    // `finalize_block` because a half-block raster must NOT get the tool block's background tint
    // patched over its cells — matching Pi, whose images are siblings of the tool box, not children.
    // TUI-N01 — the gate must consult the terminal's image CAPABILITY, not just `showImages` and
    // decodability. Upstream is `const caps = getCapabilities(); … if (caps.images && this.showImages
    // && img.data && img.mimeType)` (`components/tool-execution.ts:331-334` @v0.83.0): no protocol
    // means no `Image` child at all, and `getTextOutput` supplies the one-line `imageFallback`. On a
    // plain xterm, the Linux console, CI or a pipe, a `read` of a screenshot used to dump ~20-30 rows
    // of coloured `▀` into scrollback where pi prints one `[Image: …]` line.
    let inline = images.graphical
        && images.show
        && !run.images.is_empty()
        && run.images.iter().all(|i| i.block.is_some());
    if !inline {
        push_image_fallbacks(run, theme, &mut block);
    }
    // The block is state-tinted (bg-only); a leading untinted blank stands in for the component Spacer.
    //
    // X8 — `edit` is the one tool whose tint is NOT the shared `done`/`is_error` one. Pi gives it
    // `getEditHeaderBg(component.preview, component.settledError)` (`edit.ts:239-253`, applied at
    // `:262`), which tests the PREVIEW first and never looks at `done`: a preview diff computed from
    // the streamed arguments greens the block while the call is still pending, and a preview that
    // failed reds it.
    let bg = if run.name == "edit" && run.rendered_call.is_none() && run.rendered_result.is_none() {
        theme.edit_bg_style(Style::default(), edit_header_preview(run), run.is_error)
    } else {
        theme.tool_bg_style(Style::default(), run.done, run.is_error)
    };
    let mut out = vec![Line::default()];
    out.extend(finalize_block(block, width, bg));
    if inline {
        out.extend(image_raster_lines(run, width, images.width_cells));
    }
    out
}

/// The per-frame render inputs a tool block needs that are not on the [`ToolRun`] itself — Pi's
/// `ToolRenderContext` (`extensions/types.ts`, built at `tool-execution.ts:116-135`), narrowed to
/// the three fields cyrup's built-ins actually read.
///
/// `show`/`width_cells` are `terminal.showImages` / `terminal.imageWidthCells` (Pi's
/// `maxWidthCells`). `expand_key` and `cwd` are `context.expanded`'s companions: the live
/// `app.tools.expand` label every `… to expand` hint resolves through (`keyText`,
/// `keybinding-hints.ts:34-36`) and `context.cwd`, which `read`'s compact classification resolves
/// its path against (`read.ts:336`, `resolveToCwd(rawPath, cwd)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ImageOpts<'a> {
    pub show: bool,
    /// Whether the terminal negotiated a real image protocol — Pi's `getCapabilities().images`
    /// (`tool-execution.ts:331`). TUI-N01: fed from `AppState::image_renderer.is_graphical()`.
    /// Defaults to `true` so a test constructing `ImageOpts::default()` still exercises the inline
    /// path, which is the branch the raster tests are about.
    pub graphical: bool,
    pub width_cells: u16,
    /// The live `app.tools.expand` label; [`EXPAND_KEY`] when the caller has no keymap in hand.
    pub expand_key: &'a str,
    /// Pi `ToolRenderContext.cwd` — the SESSION's working directory, not necessarily the process's.
    /// `None` falls back to the process cwd.
    pub cwd: Option<&'a std::path::Path>,
    /// X14 — the LIVE `this.toolOutputExpanded` (`interactive-mode.ts:442`), the flag `Ctrl+O` /
    /// `setToolsExpanded` drive. Upstream never stores an expansion on a message: it seeds each
    /// component from this field at construction (`:3486`, `:3493`) and re-broadcasts to every
    /// `chatContainer` child on each toggle (`:4032-4046`), so the value in force at PAINT time is
    /// what renders. The branch/compaction summary arms of [`entry_lines`] read it here for exactly
    /// that reason. Defaults to `false`, Pi's own initial value.
    pub tools_expanded: bool,
    /// The LIVE `this.hiddenThinkingLabel` (`interactive-mode.ts:436`), for exactly the reason
    /// `tools_expanded` above is here: upstream never freezes it onto a message, it re-broadcasts to
    /// every mounted assistant component on each `setHiddenThinkingLabel` (`:2118-2129` @v0.84.2), so the
    /// value in force at PAINT time is what renders. `None` ⇒ [`HIDDEN_THINKING_LABEL`].
    ///
    /// (This struct's name has been narrower than its contents since `expand_key`/`cwd`/
    /// `tools_expanded` joined it; it is the per-paint bag for everything an [`Entry`] cannot carry
    /// on itself.)
    pub hidden_thinking_label: Option<&'a str>,
}

impl Default for ImageOpts<'_> {
    fn default() -> Self {
        ImageOpts {
            show: true,
            graphical: true,
            width_cells: DEFAULT_IMAGE_WIDTH_CELLS,
            expand_key: EXPAND_KEY,
            cwd: None,
            tools_expanded: false,
            hidden_thinking_label: None,
        }
    }
}

/// Append Pi's `[Image: …]` text stand-in for each `image` content block (`imageFallback`,
/// terminal-image.ts:546-558, reached from `getTextOutput`, render-utils.ts:49-59) — used when
/// `showImages` is off or a block could not be decoded.
///
/// Divergence worth naming: Pi splices this into the tool's TEXT output, so a collapsed `read` (whose
/// `renderResult` returns `""` unless expanded) shows nothing at all. cyrup appends it to the block
/// unconditionally, matching the inline-raster case — which Pi also renders regardless of `expanded`
/// — so an image result is never silently invisible.
fn push_image_fallbacks(run: &ToolRun, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    for img in &run.images {
        out.push(Line::styled(
            image_fallback_text(&img.mime_type, img.dimensions, None),
            theme.tool_output_style(),
        ));
    }
}

/// Rasterize each decoded `image` content block into half-block cell rows, each preceded by the
/// blank spacer Pi puts before every image component (`new Spacer(1)`, tool-execution.ts:342).
/// The raster is clamped to `width_cells` and to the content width. See
/// [`ImageBlock::halfblock_lines`] for why this is half-blocks rather than the negotiated
/// Kitty/iTerm2 protocol.
fn image_raster_lines(run: &ToolRun, width: usize, width_cells: u16) -> Vec<Line<'static>> {
    let cols = width_cells.min(width.min(u16::MAX as usize) as u16).max(1);
    let mut out = Vec::new();
    for img in run.images.iter().filter_map(|i| i.block.as_ref()) {
        let rows = img.halfblock_lines(cols);
        if rows.is_empty() {
            continue;
        }
        out.push(Line::default());
        out.extend(rows);
    }
    out
}

/// Decode the `image` content blocks of a raw tool result (`{content:[{type:"image", data, mimeType}]}`)
/// into [`ResultImage`]s — Pi's `result.content.filter((c) => c.type === "image")`
/// (tool-execution.ts:331). A block whose base64 or pixel format cannot be decoded is kept with
/// `block: None` so its text stand-in still renders.
fn decode_result_images(result: &Value) -> Vec<ResultImage> {
    use base64::Engine as _;
    let content = match result {
        Value::Object(o) => o.get("content"),
        Value::Array(_) => Some(result),
        _ => None,
    };
    let Some(Value::Array(items)) = content else { return Vec::new() };
    items
        .iter()
        .filter_map(Value::as_object)
        .filter(|o| o.get("type").and_then(Value::as_str) == Some("image"))
        .map(|o| {
            let mime_type = o
                .get("mimeType")
                .or_else(|| o.get("mime_type"))
                .and_then(Value::as_str)
                .unwrap_or("image/unknown")
                .to_string();
            let decoded = o
                .get("data")
                .and_then(Value::as_str)
                .and_then(|d| base64::engine::general_purpose::STANDARD.decode(d).ok())
                .and_then(|bytes| ImageBlock::decode(&bytes, mime_type.clone()));
            // Read the SOURCE dimensions (what Pi's `imageFallback` reports) before bounding the
            // raster the renderer will actually clone+resize each frame.
            let dimensions = decoded.as_ref().map(ImageBlock::dimensions);
            let block = decoded.map(|b| b.downscaled(MAX_RASTER_PX));
            ResultImage { mime_type, block, dimensions }
        })
        .collect()
}

/// Visible width of one **grapheme cluster** in terminal columns, via ratatui's
/// unicode-width-backed `Span::width` (Pi's `graphemeWidth`, `utils.ts:174-235`).
///
/// Deliberately the same measure [`apply_bg`] reaches through [`Line::width`]: L6's defect was that
/// the wrap point and the background pad disagreed, so the fix is only a fix if both halves count
/// the same way. `Span::width` sums `unicode_width` per `char`, so summing it per grapheme over a
/// span is identical to calling it once on the whole span — the two can never drift.
fn grapheme_cols(g: &str) -> usize {
    Span::raw(g).width()
}

/// Whether a grapheme cluster is whitespace (`token.trim() === ""`, `utils.ts:876`).
pub(crate) fn is_ws_grapheme(g: &str) -> bool {
    g.chars().all(char::is_whitespace)
}

/// Greedy word-wrap one styled line into `width`-column rows, preserving each span's style.
///
/// Port of `wrapSingleLine` (`tui/src/utils.ts:857-936`), which `Text`/`Markdown` run before a
/// `Box` re-pads them and which is therefore what makes `box.ts:85`'s `child.render(contentWidth)`
/// actually produce `contentWidth`-wide rows. Upstream's `AnsiCodeTracker` plumbing
/// (`:868`, `:914`, `:917`) is deliberately absent: a terminal line break clears the active SGR
/// state, so upstream must re-emit it at the head of every produced row, whereas ratatui carries
/// style per `Span` and the styles travel with the characters.
///
/// Semantics kept verbatim: an over-wide single token is hard-broken (`breakLongWord`, `:879-899`),
/// a wrap never starts a row with whitespace (`:912-915`), and every produced row is right-trimmed
/// (`:935`).
///
/// **The unit is the grapheme cluster, never the `char`.** Both of upstream's loops run over
/// `graphemeSegmenter.segment(...)`: `splitIntoTokensWithAnsi` builds its tokens grapheme by
/// grapheme (`utils.ts:775-798`) and `breakLongWord` segments the over-wide token the same way
/// before measuring it (`:979-980`, `:994-1012`). Wrapping per `char` instead tears a ZWJ emoji
/// sequence or a combining mark away from its base — a correctness bug, not a spacing one — and it
/// also measures differently from [`apply_bg`], which is exactly the disagreement L6 is about.
///
/// `pub(crate)` because [`crate::markdown`] is upstream's THIRD consumer of the same primitive —
/// `wrapTextWithAnsi` is called from `text.ts:67`, `box.ts` (transitively, through its child) AND
/// `markdown.ts:322`/`:594`/`:788`. There is exactly one wrapper here for the same reason there is
/// exactly one upstream; do not grow a fourth.
pub(crate) fn wrap_line(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    if line.width() <= width {
        return vec![line.clone()];
    }
    // Flatten to (grapheme, style) cells so a word may straddle a span boundary. Upstream segments
    // per non-ANSI run, i.e. per style run, which is exactly per `Span` here (`utils.ts:770-798`).
    let mut cells: Vec<(&str, Style)> = Vec::new();
    for span in &line.spans {
        let st = span.style;
        cells.extend(span.content.graphemes(true).map(|g| (g, st)));
    }
    // Tokenize into alternating whitespace / non-whitespace runs (`splitIntoTokensWithAnsi`).
    let mut tokens: Vec<Vec<(&str, Style)>> = Vec::new();
    for cell in cells {
        let ws = is_ws_grapheme(cell.0);
        match tokens.last_mut() {
            Some(tok) if tok.first().is_some_and(|f| is_ws_grapheme(f.0) == ws) => tok.push(cell),
            _ => tokens.push(vec![cell]),
        }
    }

    let mut rows: Vec<Vec<(&str, Style)>> = Vec::new();
    let mut cur: Vec<(&str, Style)> = Vec::new();
    let mut cur_w = 0usize;
    for tok in tokens {
        let tok_w: usize = tok.iter().map(|(g, _)| grapheme_cols(g)).sum();
        let is_ws = tok.first().is_some_and(|(g, _)| is_ws_grapheme(g));
        if tok_w > width && !is_ws {
            if !cur.is_empty() {
                rows.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            // `breakLongWord` (`:954-1015`) walks the token one GRAPHEME at a time, so a cluster is
            // never split across the break.
            for cell in tok {
                let w = grapheme_cols(cell.0);
                if cur_w + w > width && !cur.is_empty() {
                    rows.push(std::mem::take(&mut cur));
                    cur_w = 0;
                }
                cur_w += w;
                cur.push(cell);
            }
            continue;
        }
        if cur_w + tok_w > width && cur_w > 0 {
            rows.push(std::mem::take(&mut cur));
            cur_w = 0;
            if is_ws {
                continue;
            }
        }
        cur_w += tok_w;
        cur.extend(tok);
    }
    if !cur.is_empty() {
        rows.push(cur);
    }

    rows.into_iter()
        .map(|mut row| {
            while row.last().is_some_and(|(g, _)| is_ws_grapheme(g)) {
                row.pop();
            }
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (g, st) in row {
                match spans.last_mut() {
                    Some(last) if last.style == st => last.content.to_mut().push_str(g),
                    _ => spans.push(Span::styled(g.to_string(), st)),
                }
            }
            let mut out = Line::from(spans);
            out.style = line.style;
            out
        })
        .collect()
}

/// Port of `Box.render` (`tui/src/components/box.ts:74-136`), the shell every tinted transcript
/// block is built from.
///
/// Upstream renders its children at `contentWidth = max(1, width - paddingX * 2)` (`:79`), prefixes
/// `leftPad` to every produced row (`:80`, `:87`), then emits `paddingY` background-filled blank
/// rows, the content, and `paddingY` more blank rows (`:106-119`). `applyBg` (`:127-136`) pads each
/// row to the full `width` and paints the background across it. An empty child set renders nothing
/// at all (`:75-77`, `:91-93`) — no stray tinted blanks.
///
/// Callers that pass a non-zero `paddingY` upstream, i.e. every one of them: `tool-execution.ts:68`
/// `new Box(1, 1, toolPendingBg)` (and its `:69` `new Text("", 1, 1, …)` fallback), `edit.ts:158`
/// `new Box(1, 1, …)` (the self-shelled edit block), `user-message.ts:40`
/// `new Box(this.outputPad, 1, userMessageBg)`, `custom-message.ts:36`, `custom-entry.ts:49` and the
/// three `extends Box` message components — `skill-invocation-message.ts:17`,
/// `branch-summary-message.ts:16`, `compaction-summary-message.ts:16` — each `super(1, 1,
/// customMessageBg)`. `git grep "new Box(" v0.84.1 -- packages/` finds no other production site and
/// no caller passing `paddingY = 0`.
fn box_lines(
    children: Vec<Line<'static>>,
    width: usize,
    padding_x: usize,
    padding_y: usize,
    bg: Style,
) -> Vec<Line<'static>> {
    if children.is_empty() {
        return Vec::new();
    }
    let content_width = width.saturating_sub(padding_x * 2).max(1);
    let left = " ".repeat(padding_x);
    let mut content: Vec<Line<'static>> = Vec::new();
    for child in &children {
        for mut row in wrap_line(child, content_width) {
            if padding_x > 0 {
                row.spans.insert(0, Span::raw(left.clone()));
            }
            content.push(row);
        }
    }
    if content.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<Line<'static>> = Vec::with_capacity(content.len() + padding_y * 2);
    for _ in 0..padding_y {
        out.push(apply_bg(Line::default(), width, bg));
    }
    for row in content {
        out.push(apply_bg(row, width, bg));
    }
    for _ in 0..padding_y {
        out.push(apply_bg(Line::default(), width, bg));
    }
    out
}

/// `Box.applyBg` (`box.ts:127-136`): right-pad to `width` and paint the background over the whole
/// row.
///
/// The pad is measured with `Line::width()` — ratatui's unicode-width sum, the analogue of
/// upstream's `visibleWidth(line)` (`:128`). It used to be `chars().count()`, which **under**-counts
/// every CJK ideograph, emoji and box-drawing glyph, so the row was over-padded past the frame and
/// spilled into a spurious extra tinted row.
fn apply_bg(mut line: Line<'static>, width: usize, bg: Style) -> Line<'static> {
    let vis = line.width();
    if vis < width {
        line.spans.push(Span::raw(" ".repeat(width - vis)));
    }
    line.style = line.style.patch(bg);
    line
}

/// Port of `Text.render` (`tui/src/components/text.ts:60-87`) for one already-styled **[`Line`]**,
/// at `paddingY = 0` — the multi-span form, so a row built from several differently-styled pieces
/// (a `keyHint` pair's dim key + muted description, say) keeps every colour across the wrap.
///
/// Upstream has only this form: a `Text` is constructed from ONE string that already carries its
/// ANSI runs (`theme.fg("dim", …) + theme.fg("muted", …)`), and `wrapTextWithAnsi` carries those
/// runs through the wrap (`utils.ts:770-798`). [`wrap_line`] is that primitive; this adds `:64`'s
/// `contentWidth` and `:70-76`'s left margin. See [`text_lines`] for the single-style convenience
/// wrapper over it.
pub(crate) fn text_lines_of(
    src: &Line<'static>,
    width: usize,
    padding_x: usize,
) -> Vec<Line<'static>> {
    let content_width = width.saturating_sub(padding_x * 2).max(1);
    let left = " ".repeat(padding_x);
    let mut out: Vec<Line<'static>> = Vec::new();
    for mut row in wrap_line(src, content_width) {
        if padding_x > 0 {
            row.spans.insert(0, Span::raw(left.clone()));
        }
        out.push(row);
    }
    out
}

/// Port of `Text.render` (`tui/src/components/text.ts:60-87`) for one already-styled string, at
/// `paddingY = 0`.
///
/// Upstream wraps at `contentWidth = max(1, width - paddingX * 2)` (`:64`) and then emits
/// `leftMargin + line + rightMargin` per produced row (`:70-76`). The right margin and the pad to
/// `width` (`:83-85`) are only observable through a background, and a bare `Text` has none, so they
/// are not materialised here — a trailing run of blanks is invisible in a ratatui cell grid and
/// would only defeat the right-trim [`wrap_line`] performs.
fn text_lines(text: &str, width: usize, padding_x: usize, style: Style) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    // `wrapTextWithAnsi` splits on newlines first (`utils.ts:839`) and wraps each piece.
    for logical in text.split('\n') {
        // The style rides on the SPAN, not the `Line` — upstream's colour is baked into the string
        // the `Text` was constructed with (`theme.fg("dim", message)`), inside the margins, so it
        // survives being nested in a `Box` that later paints `Line::style` with a background.
        out.extend(text_lines_of(&Line::from(Span::styled(logical.to_string(), style)), width, padding_x));
    }
    out
}

/// The tool block's shell: `new Box(1, 1, <state bg>)` (tool-execution.ts:68).
fn finalize_block(lines: Vec<Line<'static>>, width: usize, bg: Style) -> Vec<Line<'static>> {
    box_lines(lines, width, 1, 1, bg)
}

// --- per-tool renderers ------------------------------------------------------------------------
//
// Each pushes fg-styled logical lines into `out`; `tool_lines` adds the leading spacer + state tint.

/// Port of `replaceTabs` (`core/tools/render-utils.ts:31-33`): `text.replace(/\t/g, "   ")`.
///
/// X6 — every `read`/`write` body row goes through this upstream, on BOTH sides of the
/// `lang ? … : …` ternary (`read.ts:185,190`, `write.ts:153,160`). cyrup pushed the raw string, so a
/// tab expanded at the terminal's own 8-column stops and the block's alignment drifted from Pi's
/// fixed three spaces. Exactly three spaces, not a tab-stop-aware expansion — upstream's regex is
/// positional-context-free.
fn replace_tabs(text: &str) -> String {
    text.replace('\t', "   ")
}

/// X6 — one already-`replaceTabs`'d body row, syntax-highlighted when the path resolved to a
/// language and flat `toolOutput` otherwise.
///
/// This is the whole of `read.ts:190`'s ternary
/// `lang ? replaceTabs(line) : theme.fg("toolOutput", replaceTabs(line))` — note the `lang` arm
/// carries NO `toolOutput` colour, because the highlighter already coloured it. Pi highlights the
/// joined body once (`highlightCode(replaceTabs(output), lang)`, `:185`) and splits it back into
/// lines; cyrup's [`crate::markdown::highlight_code_lines`] returns the same per-line vector, so the
/// rows are indexed rather than re-highlighted one at a time (syntect is stateful — highlighting a
/// line in isolation loses an open string/comment run).
fn body_line(
    raw: &str,
    highlighted: Option<&Vec<Line<'static>>>,
    idx: usize,
    theme: &UiTheme,
) -> Line<'static> {
    match highlighted.and_then(|h| h.get(idx)) {
        Some(l) => l.clone(),
        None => Line::styled(replace_tabs(raw), theme.tool_output_style()),
    }
}

/// `read` — header `read <path>:<range>` + (only when expanded/error) the file body (`read.ts:74-201`).
fn render_read(
    run: &ToolRun,
    expanded: bool,
    theme: &UiTheme,
    opts: ImageOpts<'_>,
    out: &mut Vec<Line<'static>>,
) {
    // X7 — `renderCall` picks between two headers (`read.ts:334-343`):
    // `const classification = !context.expanded ? getCompactReadClassification(args, context.cwd) : undefined;`
    // so the compact `[skill] name` / `read resource <label>` form is COLLAPSED-only; expanding a
    // skill read falls back to the plain `read <path>` header plus the body.
    let classification =
        if expanded { None } else { compact_read_classification(&run.args, opts.cwd) };
    match classification {
        Some(c) => out.push(compact_read_call(&c, &run.args, opts.expand_key, theme)),
        None => {
            let mut spans = vec![Span::styled("read ", theme.tool_title_style())];
            spans.push(tool_path_span(&run.args, &["file_path", "path"], None, theme));
            if let Some(range) = read_line_range(&run.args) {
                spans.push(Span::styled(range, theme.warning_style()));
            }
            out.push(Line::from(spans));
        }
    }
    // `formatReadResult`: nothing below the header when collapsed & not an error (read.ts:173-175).
    let Some(result) = &run.result else { return };
    if !expanded && !run.is_error {
        return;
    }
    let output = result_text(result);
    // `const lang = !isError && rawPath ? getLanguageFromPath(rawPath) : undefined` (`read.ts:184`).
    let raw_path = match str_arg(&run.args, &["file_path", "path"]) {
        StrArg::Value(p) => p,
        _ => String::new(),
    };
    let lang = if run.is_error || raw_path.is_empty() {
        None
    } else {
        crate::theme::language_from_path(&raw_path)
    };
    // `highlightCode(replaceTabs(output), lang)` — the tabs are replaced BEFORE the highlighter runs
    // on this side of the ternary (`read.ts:185`), so a leading tab is three highlighted spaces.
    let highlighted =
        lang.and_then(|l| crate::markdown::highlight_code_lines(&replace_tabs(&output), l, theme));
    let all = trim_trailing_empty(output.split('\n').collect());
    let total = all.len();
    let shown = if expanded { total } else { total.min(10) };
    out.push(Line::default());
    for (i, l) in all.iter().take(shown).enumerate() {
        out.push(body_line(l, highlighted.as_ref(), i, theme));
    }
    let remaining = total.saturating_sub(shown);
    if remaining > 0 {
        out.push(more_lines_hint(remaining, None, opts.expand_key, theme));
    }
    push_read_truncation(result, theme, out);
}

/// `write` — header `write <path>` + a content preview from the call args (`write.ts:131-179`).
fn render_write(
    run: &ToolRun,
    expanded: bool,
    theme: &UiTheme,
    expand_key: &str,
    out: &mut Vec<Line<'static>>,
) {
    let mut spans = vec![Span::styled("write ", theme.tool_title_style())];
    spans.push(tool_path_span(&run.args, &["file_path", "path"], None, theme));
    out.push(Line::from(spans));
    match str_arg(&run.args, &["content"]) {
        StrArg::Invalid => {
            out.push(Line::default());
            out.push(Line::styled(
                "[invalid content arg - expected string]".to_string(),
                theme.error_style(),
            ));
        }
        StrArg::Missing => {}
        StrArg::Value(content) => {
            let display = content.replace('\r', "");
            // X6 — `const lang = rawPath ? getLanguageFromPath(rawPath) : undefined` (`write.ts:151`).
            // Unlike `read` there is no `isError` leg: the preview comes from the ARGUMENTS, so it is
            // highlighted whether or not the write went on to fail.
            let raw_path = match str_arg(&run.args, &["file_path", "path"]) {
                StrArg::Value(p) => p,
                _ => String::new(),
            };
            let lang = if raw_path.is_empty() {
                None
            } else {
                crate::theme::language_from_path(&raw_path)
            };
            let highlighted = lang.and_then(|l| {
                crate::markdown::highlight_code_lines(&replace_tabs(&display), l, theme)
            });
            let all = trim_trailing_empty(display.split('\n').collect());
            let total = all.len();
            let shown = if expanded { total } else { total.min(10) };
            out.push(Line::default());
            for (i, l) in all.iter().take(shown).enumerate() {
                out.push(body_line(l, highlighted.as_ref(), i, theme));
            }
            let remaining = total.saturating_sub(shown);
            if remaining > 0 {
                out.push(more_lines_hint(remaining, Some(total), expand_key, theme));
            }
        }
    }
    // `formatWriteResult` shows output only on error (write.ts:164-179).
    if run.is_error && let Some(result) = &run.result {
        push_error_body(result, theme, out);
    }
}

/// `edit` — header `edit <path>` + the diff (`edit.ts:200-227/244-262/363-431`, rendered via
/// [`crate::diff::render_diff`], the port of `diff.ts`).
///
/// Two sources feed that diff, in Pi's order:
///
/// 1. the **pre-execution preview** ([`ToolRun::preview`], Pi `buildEditCallComponent`
///    edit.ts:244-262): a `Spacer(1)` then the diff `computeEditsDiff` produced from the arguments
///    alone, or the failure message in the error colour. This is on screen while the call is still
///    PENDING — including for the whole time a permission prompt is up — and before anything is
///    written.
/// 2. the settled result's `details.diff`, which **replaces** the preview rather than being appended
///    below it. That is Pi's own ordering, and it is easy to misread: `renderResult` calls
///    `setEditPreview(callComponent, { diff: result.details.diff, … })` (edit.ts:196-204) BEFORE
///    handing `callComponent.preview` to `formatEditResult`, so by the time `formatEditResult` tests
///    `resultDiff !== previewDiff` (`:220-223`) the two are the same object and the result body
///    renders nothing. The diff is therefore drawn exactly once, by the call component, and it is
///    the authoritative post-write one.
///
/// The same de-duplication applies to failures: an error result whose text merely restates the
/// preview error is dropped (`:212-218`), while a preview that succeeded stays on screen next to an
/// error the tool itself hit.
/// X8 — which of Pi's three `getEditHeaderBg` preview states this run is in
/// (`core/tools/edit.ts:239-253`).
///
/// `EditCallRenderComponent.preview` is a single slot that BOTH the pre-execution `computeEditsDiff`
/// (`renderCall`, `:385`) and the settled result (`renderResult`'s `setEditPreview` from
/// `details.diff`, `:400-411`) write, the result overwriting the preview. So the result diff is
/// tested first here, exactly as `renderResult` runs before `buildEditCallComponent` rebuilds the
/// component. The two are read with the same accessors [`render_edit`] uses, so the tint can never
/// disagree with the body drawn inside it.
fn edit_header_preview(run: &ToolRun) -> crate::theme::EditHeaderPreview {
    use crate::theme::EditHeaderPreview as P;
    let result_diff = run
        .result
        .as_ref()
        .filter(|_| !run.is_error)
        .and_then(|r| r.get("details"))
        .and_then(|d| d.get("diff"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    if result_diff.is_some() {
        return P::Computed;
    }
    match &run.preview {
        Some(Ok(d)) if !d.is_empty() => P::Computed,
        Some(Err(e)) if !e.trim().is_empty() => P::Failed,
        _ => P::Absent,
    }
}

fn render_edit(run: &ToolRun, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let mut spans = vec![Span::styled("edit ", theme.tool_title_style())];
    spans.push(tool_path_span(&run.args, &["file_path", "path"], None, theme));
    out.push(Line::from(spans));

    let preview_diff = match &run.preview {
        Some(Ok(d)) if !d.is_empty() => Some(d.as_str()),
        _ => None,
    };
    let preview_error = match &run.preview {
        Some(Err(e)) if !e.trim().is_empty() => Some(e.as_str()),
        _ => None,
    };
    // The settled diff supersedes the preview (`setEditPreview` from `renderResult`, edit.ts:196-204).
    let result_diff = run
        .result
        .as_ref()
        .filter(|_| !run.is_error)
        .and_then(|r| r.get("details"))
        .and_then(|d| d.get("diff"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());

    if let Some(diff) = result_diff.or(preview_diff) {
        out.push(Line::default());
        out.extend(crate::diff::render_diff(diff, theme));
    } else if let Some(err) = preview_error {
        out.push(Line::default());
        for l in err.split('\n') {
            out.push(Line::styled(l.to_string(), theme.error_style()));
        }
    }

    if run.is_error
        && let Some(result) = &run.result
    {
        // `if (!errorText || errorText === previewError) return undefined` (edit.ts:215-217).
        if preview_error.is_some_and(|e| result_text(result).trim() == e.trim()) {
            return;
        }
        push_error_body(result, theme, out);
    }
}

/// `bash` — header `$ <command> (timeout Ns)` + the output tail (collapsed = last 5 visual lines) +
/// truncation notices + a `Took {d}s` footer (`bash.ts:201-289/430-464`).
fn render_bash(
    run: &ToolRun,
    expanded: bool,
    theme: &UiTheme,
    expand_key: &str,
    out: &mut Vec<Line<'static>>,
) {
    // Header: `$ command`, bold, + a muted ` (timeout Ns)` suffix (`formatBashCall`).
    let title = theme.tool_title_style();
    let mut spans = Vec::new();
    match str_arg(&run.args, &["command"]) {
        StrArg::Invalid => {
            spans.push(Span::styled("$ ".to_string(), title));
            spans.push(Span::styled("[invalid arg]".to_string(), theme.error_style()));
        }
        StrArg::Missing => {
            spans.push(Span::styled("$ ".to_string(), title));
            spans.push(Span::styled("...".to_string(), theme.tool_output_style()));
        }
        StrArg::Value(cmd) => spans.push(Span::styled(format!("$ {cmd}"), title)),
    }
    if let Some(t) = run.args.get("timeout").and_then(Value::as_f64).filter(|t| *t != 0.0) {
        // `${timeout}s` (bash.ts:204): JS renders an integer number without a trailing `.0`.
        let disp = if t.fract() == 0.0 { format!("{}", t as i64) } else { format!("{t}") };
        spans.push(Span::styled(format!(" (timeout {disp}s)"), theme.muted_style()));
    }
    out.push(Line::from(spans));

    if let Some(result) = &run.result {
        let raw = result_text(result);
        let output = strip_bash_footer(raw.trim(), result, run.done);
        if !output.is_empty() {
            out.push(Line::default());
            let all: Vec<&str> = output.split('\n').collect();
            let total = all.len();
            if expanded {
                for l in &all {
                    out.push(Line::styled((*l).to_string(), theme.tool_output_style()));
                }
            } else {
                let shown = total.min(5);
                let skipped = total - shown;
                if skipped > 0 {
                    // X9 — same three-run shape as [`more_lines_hint`], with `bash.ts:281-284`'s own
                    // wording:
                    // `fg("muted", `... (${skipped} earlier lines,`) + ` ${keyHint("app.tools.expand", "to expand")}` + fg("muted", ")")`.
                    let mut spans = vec![
                        Span::styled(
                            format!("... ({skipped} earlier lines,"),
                            theme.muted_style(),
                        ),
                        Span::raw(" "),
                    ];
                    spans.extend(key_hint_spans(expand_key, "to expand", theme));
                    spans.push(Span::styled(")".to_string(), theme.muted_style()));
                    out.push(Line::from(spans));
                }
                for l in all.iter().skip(skipped) {
                    out.push(Line::styled((*l).to_string(), theme.tool_output_style()));
                }
            }
        }
        push_bash_warnings(result, theme, out);
        // The duration footer (bash.ts:309-313). Upstream is literally
        // `const label = options.isPartial ? "Elapsed" : "Took"` with
        // `formatDuration((endedAt ?? Date.now()) - startedAt)`, so a RUNNING command shows a live
        // `Elapsed 12.3s` that only becomes `Took 12.4s` when the call settles — the tool's
        // `renderResult` arms a 1 s `setInterval(() => context.invalidate())` (`:471-473`) precisely
        // to make it tick. It is gated on `startedAt`, which `renderCall` stamps the moment
        // execution begins (`:460-463`), NOT on the result being final; `run.result` is already
        // `Some` from the first frame because bash emits an initial empty update before it spawns
        // (bash.ts:384-385, ported at `cyrup-tools/src/tools/bash.rs:170`), which is what makes
        // upstream's `if (this.result)` renderResult gate (tool-execution.ts:281) pass too.
        //
        // Before this, cyrup keyed the line on `duration_ms`, which is written only on settle
        // (`push_tool_end_rendered`), so a long-running command rendered NO duration at all — the
        // one number that tells a user a 10-minute build is still alive.
        if let Some(started) = run.started_at {
            let (label, ms) = match run.duration_ms {
                Some(ms) => ("Took", ms),
                None => ("Elapsed", started.elapsed().as_millis() as u64),
            };
            // X10 — `bash.ts:317` is `new Text(`\n${theme.fg("muted", …)}`, 0, 0)`: the same
            // leading-`\n` blank row as the warnings block above.
            out.push(Line::default());
            out.push(Line::styled(
                format!("{label} {}", format_duration(ms)),
                theme.muted_style(),
            ));
        }
    }
}

/// `grep` — header `grep /<pattern>/ in <path> (glob) limit N` + matching lines (head-15) + a
/// `[Truncated: …]` notice (`grep.ts:68-121/370-379`).
fn render_grep(
    run: &ToolRun,
    expanded: bool,
    theme: &UiTheme,
    expand_key: &str,
    out: &mut Vec<Line<'static>>,
) {
    let title = theme.tool_title_style();
    let outp = theme.tool_output_style();
    let mut spans = vec![Span::styled("grep ".to_string(), title)];
    match str_arg(&run.args, &["pattern"]) {
        StrArg::Invalid => spans.push(Span::styled("[invalid arg]".to_string(), theme.error_style())),
        StrArg::Missing => spans.push(Span::styled("//".to_string(), theme.accent_style())),
        StrArg::Value(p) => spans.push(Span::styled(format!("/{p}/"), theme.accent_style())),
    }
    spans.push(Span::styled(" in ".to_string(), outp));
    push_search_path(&run.args, theme, &mut spans);
    if let StrArg::Value(glob) = str_arg(&run.args, &["glob"]) {
        spans.push(Span::styled(format!(" ({glob})"), outp));
    }
    if let Some(limit) = run.args.get("limit").and_then(Value::as_i64) {
        spans.push(Span::styled(format!(" limit {limit}"), outp));
    }
    out.push(Line::from(spans));
    push_list_output(run, expanded, 15, theme, expand_key, out);
    push_grep_warnings(run.result.as_ref(), theme, out);
}

/// `find` — header `find <pattern> in <path> (limit N)` + matching paths (head-20) + a `[Truncated: …]`
/// notice (`find.ts:59-107/359-368`).
fn render_find(
    run: &ToolRun,
    expanded: bool,
    theme: &UiTheme,
    expand_key: &str,
    out: &mut Vec<Line<'static>>,
) {
    let title = theme.tool_title_style();
    let outp = theme.tool_output_style();
    let mut spans = vec![Span::styled("find ".to_string(), title)];
    match str_arg(&run.args, &["pattern"]) {
        StrArg::Invalid => spans.push(Span::styled("[invalid arg]".to_string(), theme.error_style())),
        StrArg::Missing => {}
        StrArg::Value(p) => spans.push(Span::styled(p, theme.accent_style())),
    }
    spans.push(Span::styled(" in ".to_string(), outp));
    push_search_path(&run.args, theme, &mut spans);
    if let Some(limit) = run.args.get("limit").and_then(Value::as_i64) {
        spans.push(Span::styled(format!(" (limit {limit})"), outp));
    }
    out.push(Line::from(spans));
    push_list_output(run, expanded, 20, theme, expand_key, out);
    push_find_warnings(run.result.as_ref(), theme, out);
}

/// `ls` — header `ls <path> (limit N)` + entries (head-20) + a `[Truncated: …]` notice
/// (`ls.ts:52-93/210-219`).
fn render_ls(
    run: &ToolRun,
    expanded: bool,
    theme: &UiTheme,
    expand_key: &str,
    out: &mut Vec<Line<'static>>,
) {
    let mut spans = vec![Span::styled("ls ".to_string(), theme.tool_title_style())];
    spans.push(tool_path_span(&run.args, &["path"], Some("."), theme));
    if let Some(limit) = run.args.get("limit").and_then(Value::as_i64) {
        spans.push(Span::styled(format!(" (limit {limit})"), theme.tool_output_style()));
    }
    out.push(Line::from(spans));
    push_list_output(run, expanded, 20, theme, expand_key, out);
    push_ls_warnings(run.result.as_ref(), theme, out);
}

/// Non-built-in tools fall back to Pi's `formatToolExecution` (tool-execution.ts:365-376): the bold
/// tool name + pretty-printed args + any text output.
/// Draw a tool whose renderer an extension supplied (EXT-006). The extension's `renderCall` text
/// is the header; its `renderResult` text is the body, shown once the run finishes (collapsed runs
/// keep the header only, matching every built-in's collapsed form). A half-supplied renderer
/// degrades gracefully: a missing call text falls back to the tool NAME header, a missing result
/// text simply omits the body.
fn render_extension(run: &ToolRun, expanded: bool, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    match &run.rendered_call {
        Some(call) => {
            for l in call.split('\n') {
                out.push(Line::styled(l.to_string(), theme.tool_title_style()));
            }
        }
        None => out.push(Line::styled(run.name.clone(), theme.tool_title_style())),
    }
    if let Some(result) = &run.rendered_result
        && (run.done || expanded)
        && !result.trim().is_empty()
    {
        for l in result.split('\n') {
            out.push(Line::styled(l.to_string(), theme.tool_output_style()));
        }
    }
}

fn render_generic(run: &ToolRun, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    out.push(Line::styled(run.name.clone(), theme.tool_title_style()));
    if !run.args.is_null()
        && let Ok(pretty) = serde_json::to_string_pretty(&run.args)
    {
        out.push(Line::default());
        for l in pretty.split('\n') {
            out.push(Line::styled(l.to_string(), theme.tool_output_style()));
        }
    }
    if let Some(result) = &run.result {
        let output = result_text(result);
        if !output.trim().is_empty() {
            for l in output.split('\n') {
                out.push(Line::styled(l.to_string(), theme.tool_output_style()));
            }
        }
    }
}

// --- per-tool render helpers -------------------------------------------------------------------

/// A coalesced string argument (`args.file_path ?? args.path`, then Pi's `str()`): a present non-string
/// → [`StrArg::Invalid`] (`[invalid arg]`), absent/null/`""` → [`StrArg::Missing`], else the string.
enum StrArg {
    Invalid,
    Missing,
    Value(String),
}

/// `args[key0] ?? args[key1] ?? …` then `str()` (render-utils.ts:25-29): skip absent/JSON-null keys, a
/// non-string value is `Invalid`, an empty string is `Missing`.
fn str_arg(args: &Value, keys: &[&str]) -> StrArg {
    for k in keys {
        match args.get(k) {
            None | Some(Value::Null) => continue,
            Some(Value::String(s)) => {
                return if s.is_empty() { StrArg::Missing } else { StrArg::Value(s.clone()) };
            }
            Some(_) => return StrArg::Invalid,
        }
    }
    StrArg::Missing
}

/// `renderToolPath` (render-utils.ts:75-85): `[invalid arg]` for a non-string, the `emptyFallback`
/// (else `...`) for an empty/absent path, otherwise the `~`-shortened path in accent. Hyperlinks are a
/// terminal escape the cell grid does not carry (tracked residual).
fn tool_path_span(
    args: &Value,
    keys: &[&str],
    empty_fallback: Option<&str>,
    theme: &UiTheme,
) -> Span<'static> {
    match str_arg(args, keys) {
        StrArg::Invalid => Span::styled("[invalid arg]".to_string(), theme.error_style()),
        StrArg::Missing => match empty_fallback {
            Some(f) => Span::styled(shorten_path(f), theme.accent_style()),
            None => Span::styled("...".to_string(), theme.tool_output_style()),
        },
        StrArg::Value(p) => Span::styled(shorten_path(&p), theme.accent_style()),
    }
}

/// The `" in <path>"` tail shared by grep/find (`path = shortenPath(rawPath || ".")` in `toolOutput`, a
/// non-string → `[invalid arg]`). The caller has already pushed the `" in "` label span.
fn push_search_path(args: &Value, theme: &UiTheme, spans: &mut Vec<Span<'static>>) {
    match str_arg(args, &["path"]) {
        StrArg::Invalid => spans.push(Span::styled("[invalid arg]".to_string(), theme.error_style())),
        StrArg::Missing => {
            spans.push(Span::styled(shorten_path("."), theme.tool_output_style()));
        }
        StrArg::Value(p) => spans.push(Span::styled(shorten_path(&p), theme.tool_output_style())),
    }
}

/// `formatReadLineRange` (read.ts:67-72): `:<start>` or `:<start>-<end>` from `offset`/`limit`.
fn read_line_range(args: &Value) -> Option<String> {
    let offset = args.get("offset").and_then(Value::as_i64);
    let limit = args.get("limit").and_then(Value::as_i64);
    if offset.is_none() && limit.is_none() {
        return None;
    }
    let start = offset.unwrap_or(1);
    Some(match limit {
        Some(l) => format!(":{start}-{}", start + l - 1),
        None => format!(":{start}"),
    })
}

/// Port of `keyHint(keybinding, description)` (`keybinding-hints.ts:42-44`):
///
/// ```ts
/// return theme.fg("dim", keyText(keybinding)) + theme.fg("muted", ` ${description}`);
/// ```
///
/// TWO runs, not one — the key label alone is `dim` and the words after it are `muted`, and the
/// separating space belongs to the muted run. `bash.rs`'s X16 hint already renders exactly this
/// shape; X9 is the same primitive extracted so the transcript's hints stop disagreeing with it.
fn key_hint_spans(key: &str, description: &str, theme: &UiTheme) -> [Span<'static>; 2] {
    [
        Span::styled(key.to_string(), theme.dim_style()),
        Span::styled(format!(" {description}"), theme.muted_style()),
    ]
}

/// A `... (N more lines[, M total], <key> to expand)` hint (read/write/grep/find/ls collapsed tail).
///
/// X9 — upstream is one interpolation with THREE colour runs
/// (`read.ts:192` = `grep.ts:111` = `find.ts:108` = `ls.ts:85`, and `write.ts:162` with the extra
/// `N total,`):
///
/// ```ts
/// theme.fg("muted", `\n... (${remaining} more lines,`) + " " + keyHint("app.tools.expand", "to expand") + theme.fg("muted", ")")
/// ```
///
/// so the key label is `dim` against `muted` words — and the space between the count and the key is
/// OUTSIDE both `fg()` calls, i.e. unstyled. cyrup painted the whole sentence one flat `muted` and
/// spelled the key as the compile-time literal `ctrl+o`, so a rebound `app.tools.expand` still
/// printed `ctrl+o`; `key` is now the live `keyText` label.
fn more_lines_hint(
    remaining: usize,
    total: Option<usize>,
    key: &str,
    theme: &UiTheme,
) -> Line<'static> {
    let lead = match total {
        Some(t) => format!("... ({remaining} more lines, {t} total,"),
        None => format!("... ({remaining} more lines,"),
    };
    let mut spans = vec![Span::styled(lead, theme.muted_style()), Span::raw(" ")];
    spans.extend(key_hint_spans(key, "to expand", theme));
    spans.push(Span::styled(")".to_string(), theme.muted_style()));
    Line::from(spans)
}

/// The file names `getCompactReadClassification` treats as a "resource" read
/// (`core/tools/read.ts:42` `COMPACT_RESOURCE_FILE_NAMES`). Verbatim, including the two `.MD`
/// spellings — the set is matched case-SENSITIVELY upstream (`Set.has(basename(absolutePath))`), so
/// `agents.md` is deliberately not in it.
const COMPACT_RESOURCE_FILE_NAMES: [&str; 5] =
    ["AGENTS.override.md", "AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"];

/// One `CompactReadClassification` (`read.ts:37-40`) — `kind` is `"docs" | "resource" | "skill"`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CompactRead {
    kind: &'static str,
    label: String,
}

/// Port of `getCompactReadClassification` (`core/tools/read.ts:122-143`). **X7 = `G30b`** in
/// `docs/gap-analysis/PARITY-GAPS.md` §2.5 — the same unported function; porting it here closes both, and neither
/// backlog should re-land it.
///
/// ```ts
/// const absolutePath = resolveToCwd(rawPath, cwd);
/// const fileName = basename(absolutePath);
/// if (fileName === "SKILL.md") return { kind: "skill", label: basename(dirname(absolutePath)) || fileName };
/// const docsClassification = getPiDocsClassification(absolutePath);
/// if (docsClassification) return docsClassification;
/// if (COMPACT_RESOURCE_FILE_NAMES.has(fileName)) return { kind: "resource", label: formatPathRelativeToCwdOrAbsolute(absolutePath, cwd) };
/// return undefined;
/// ```
///
/// The `docs` arm is the one piece that cannot be ported here, and the missing seam is specific:
/// `getPiDocsClassification` (`:103-120`) resolves the read path against `dirname(getReadmePath())`
/// — the directory of the SHIPPED package's `README.md` (`coding-agent/src/config.ts`) — to label
/// `README.md`/`docs/…`/`examples/…` inside pi's own install. `getReadmePath` has no counterpart
/// anywhere in `crates/` (`grep -rn "readme_path\|getReadmePath" crates --include=*.rs` is empty),
/// and a Rust binary ships no such tree, so there is no path to compare against. `skill` and
/// `resource` are complete; `docs` needs a packaged-docs locator to exist first.
fn compact_read_classification(
    args: &Value,
    cwd: Option<&std::path::Path>,
) -> Option<CompactRead> {
    let raw_path = match str_arg(args, &["file_path", "path"]) {
        StrArg::Value(p) => p,
        // `if (!rawPath) return undefined` (`:127`) — covers both the empty and the non-string case,
        // since `str()` yields `""`/`null` and both are falsy.
        _ => return None,
    };
    // `resolveToCwd(rawPath, cwd)` — an absolute path is kept, a relative one is joined to the
    // session cwd. `Path::join` has exactly that semantic for an absolute right-hand side.
    let base = match cwd {
        Some(c) => c.to_path_buf(),
        None => std::env::current_dir().ok()?,
    };
    let absolute = base.join(&raw_path);
    let file_name = absolute.file_name()?.to_string_lossy().into_owned();
    if file_name == "SKILL.md" {
        // `basename(dirname(absolutePath)) || fileName` — the containing directory names the skill,
        // and a `SKILL.md` at the filesystem root falls back to the file name itself.
        let label = absolute
            .parent()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or(file_name);
        return Some(CompactRead { kind: "skill", label });
    }
    if COMPACT_RESOURCE_FILE_NAMES.contains(&file_name.as_str()) {
        // `formatPathRelativeToCwdOrAbsolute(absolutePath, cwd)`: the cwd-relative form when the file
        // is under it, else the absolute path.
        let label = absolute
            .strip_prefix(&base)
            .map(|r| r.to_string_lossy().into_owned())
            .unwrap_or_else(|_| absolute.to_string_lossy().into_owned());
        return Some(CompactRead { kind: "resource", label });
    }
    None
}

/// Port of `formatCompactReadCall` (`core/tools/read.ts:145-167`).
///
/// ```ts
/// const expandHint = theme.fg("dim", ` (${keyText("app.tools.expand")} to expand)`);
/// if (classification.kind === "skill")
///     return theme.fg("customMessageLabel", `\x1b[1m[skill]\x1b[22m `) +
///            theme.fg("customMessageText", classification.label) + formatReadLineRange(args, theme) + expandHint;
/// return theme.fg("toolTitle", theme.bold(`read ${classification.kind}`)) + " " +
///        theme.fg("accent", classification.label) + formatReadLineRange(args, theme) + expandHint;
/// ```
///
/// Note the expand hint here is **not** `keyHint`: it is one whole `dim` run including the words and
/// the parentheses (`:150`), unlike [`more_lines_hint`]'s dim-key/muted-words split. That asymmetry
/// is upstream's, and copying `keyHint`'s two-tone shape onto it would be the wrong fix.
fn compact_read_call(
    c: &CompactRead,
    args: &Value,
    expand_key: &str,
    theme: &UiTheme,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    if c.kind == "skill" {
        // The `\x1b[1m…\x1b[22m` pair inside the interpolation is bold-on/bold-off around the
        // bracket label only; `custom_message_label_style` already carries BOLD.
        spans.push(Span::styled("[skill] ".to_string(), theme.custom_message_label_style()));
        spans.push(Span::styled(c.label.clone(), theme.custom_message_text_style()));
    } else {
        spans.push(Span::styled(format!("read {}", c.kind), theme.tool_title_style()));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(c.label.clone(), theme.accent_style()));
    }
    if let Some(range) = read_line_range(args) {
        spans.push(Span::styled(range, theme.warning_style()));
    }
    spans.push(Span::styled(format!(" ({expand_key} to expand)"), theme.dim_style()));
    Line::from(spans)
}

/// Shared head-N list body for grep/find/ls (`\n` + first N output lines + a `… more` hint).
fn push_list_output(
    run: &ToolRun,
    expanded: bool,
    head: usize,
    theme: &UiTheme,
    expand_key: &str,
    out: &mut Vec<Line<'static>>,
) {
    let Some(result) = &run.result else { return };
    let output = result_text(result);
    let output = output.trim();
    if output.is_empty() {
        return;
    }
    let all: Vec<&str> = output.split('\n').collect();
    let total = all.len();
    let shown = if expanded { total } else { total.min(head) };
    out.push(Line::default());
    for l in all.iter().take(shown) {
        out.push(Line::styled((*l).to_string(), theme.tool_output_style()));
    }
    let remaining = total.saturating_sub(shown);
    if remaining > 0 {
        out.push(more_lines_hint(remaining, None, expand_key, theme));
    }
}

/// Push an error body (`\n` + the result text in the error color): edit/write on failure.
fn push_error_body(result: &Value, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let text = result_text(result);
    if text.trim().is_empty() {
        return;
    }
    out.push(Line::default());
    for l in text.split('\n') {
        out.push(Line::styled(l.to_string(), theme.error_style()));
    }
}

/// Extract the tool result's display text (`getTextOutput`, render-utils.ts:39-63): join the `text`
/// blocks of `{content:[…]}`, else a `text`/`output`/`stdout`/`message` string field, else a bare
/// string/array.
///
/// Every branch goes through [`crate::ansi::sanitize_display_text`] — the full
/// `sanitizeBinaryOutput(stripAnsi(text)).replace(/\r/g, "")` of `render-utils.ts:48`, not just the
/// `\r` drop. Only `bash` output arrives pre-sanitized (at capture, `cyrup-session-svc/src/bash.rs`
/// `sanitize_chunk`); `read`/`ls`/`find`/`grep` and every extension tool reach here raw, and the
/// transform is idempotent so the pre-sanitized path is unaffected.
///
/// `image` blocks are NOT represented here — they are rendered by [`tool_lines`], either as an
/// inline half-block raster or as Pi's `[Image: …]` stand-in ([`push_image_fallbacks`]) — so this is
/// the `showImages`-on half of Pi's `getTextOutput`, whose image-indicator half lives there.
fn result_text(result: &Value) -> String {
    match result {
        Value::String(s) => crate::ansi::sanitize_display_text(s),
        Value::Object(o) => {
            if let Some(content) = o.get("content") {
                return content_blocks_text(content);
            }
            for k in ["text", "output", "stdout", "message"] {
                if let Some(Value::String(s)) = o.get(k) {
                    return crate::ansi::sanitize_display_text(s);
                }
            }
            String::new()
        }
        Value::Array(_) => content_blocks_text(result),
        _ => String::new(),
    }
}

/// Join a `content` block array into text (`text` blocks concatenated with `\n`). `image` blocks are
/// skipped — [`tool_lines`] renders them (raster or `[Image: …]` stand-in).
fn content_blocks_text(content: &Value) -> String {
    match content {
        Value::Array(items) => {
            let mut parts = Vec::new();
            for it in items {
                if let Some(obj) = it.as_object() {
                    let ty = obj.get("type").and_then(Value::as_str);
                    if matches!(ty, Some("text") | None)
                        && let Some(Value::String(t)) = obj.get("text")
                    {
                        parts.push(crate::ansi::sanitize_display_text(t));
                        continue;
                    }
                } else if let Some(s) = it.as_str() {
                    parts.push(crate::ansi::sanitize_display_text(s));
                }
            }
            parts.join("\n")
        }
        Value::String(s) => crate::ansi::sanitize_display_text(s),
        _ => String::new(),
    }
}

/// Drop trailing empty lines (`trimTrailingEmptyLines`, read.ts:79-85 / write.ts:123-129).
fn trim_trailing_empty(mut lines: Vec<&str>) -> Vec<&str> {
    while lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

/// The truncation object from `result.details.truncation` when `truncated` is set.
fn truncation(result: &Value) -> Option<&Value> {
    let t = result.get("details")?.get("truncation")?;
    (t.get("truncated") == Some(&Value::Bool(true))).then_some(t)
}

fn tnum(t: &Value, key: &str) -> u64 {
    t.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// `formatSize` (truncate.ts:61-69): `B` / `KB` / `MB`.
fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// `formatDuration` (bash.ts:197-199): `{s}.{tenths}s`.
fn format_duration(ms: u64) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
}

/// read `renderResult` truncation footer (read.ts:190-199).
fn push_read_truncation(result: &Value, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let Some(t) = truncation(result) else { return };
    let max_bytes = t.get("maxBytes").and_then(Value::as_u64).unwrap_or(DEFAULT_MAX_BYTES);
    let msg = if t.get("firstLineExceedsLimit") == Some(&Value::Bool(true)) {
        format!("[First line exceeds {} limit]", format_size(max_bytes))
    } else if t.get("truncatedBy").and_then(Value::as_str) == Some("lines") {
        format!(
            "[Truncated: showing {} of {} lines ({} line limit)]",
            tnum(t, "outputLines"),
            tnum(t, "totalLines"),
            tnum(t, "maxLines"),
        )
    } else {
        format!("[Truncated: {} lines shown ({} limit)]", tnum(t, "outputLines"), format_size(max_bytes))
    };
    out.push(Line::styled(msg, theme.warning_style()));
}

/// Strip the `\n\n[Showing lines … Full output: <path>]` footer bash bakes into the text but re-renders
/// as a warning (bash.ts:226-231): only when finished + truncated + a `fullOutputPath` is present.
fn strip_bash_footer(output: &str, result: &Value, done: bool) -> String {
    let full = result.get("details").and_then(|d| d.get("fullOutputPath")).and_then(Value::as_str);
    if done
        && truncation(result).is_some()
        && let Some(path) = full
        && output.ends_with(']')
        && let Some(idx) = output.rfind("\n\n[")
        && output[idx..].contains(path)
    {
        return output[..idx].trim_end().to_string();
    }
    output.to_string()
}

/// bash `renderResult` truncation + full-output warnings (bash.ts:267-282).
fn push_bash_warnings(result: &Value, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let full = result.get("details").and_then(|d| d.get("fullOutputPath")).and_then(Value::as_str);
    let trunc = truncation(result);
    if trunc.is_none() && full.is_none() {
        return;
    }
    let mut warns = Vec::new();
    if let Some(p) = full {
        warns.push(format!("Full output: {p}"));
    }
    if let Some(t) = trunc {
        if t.get("truncatedBy").and_then(Value::as_str) == Some("lines") {
            warns.push(format!("Truncated: showing {} of {} lines", tnum(t, "outputLines"), tnum(t, "totalLines")));
        } else {
            let max_bytes = t.get("maxBytes").and_then(Value::as_u64).unwrap_or(DEFAULT_MAX_BYTES);
            warns.push(format!("Truncated: {} lines shown ({} limit)", tnum(t, "outputLines"), format_size(max_bytes)));
        }
    }
    // X10 — `bash.ts:311` is `new Text(`\n${theme.fg("warning", …)}`, 0, 0)`; the leading `\n` makes
    // `wrapTextWithAnsi` emit an empty first row (`utils.ts:839` splits on it), so the warning row
    // is always preceded by a blank.
    out.push(Line::default());
    out.push(Line::styled(format!("[{}]", warns.join(". ")), theme.warning_style()));
}

/// grep `renderResult` warnings (grep.ts:110-119).
fn push_grep_warnings(result: Option<&Value>, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let Some(result) = result else { return };
    let details = result.get("details");
    let match_limit = details.and_then(|d| d.get("matchLimitReached")).and_then(Value::as_u64);
    let lines_trunc = details.and_then(|d| d.get("linesTruncated")) == Some(&Value::Bool(true));
    let trunc = truncation(result);
    if match_limit.is_none() && trunc.is_none() && !lines_trunc {
        return;
    }
    let mut warns = Vec::new();
    if let Some(n) = match_limit {
        warns.push(format!("{n} matches limit"));
    }
    if let Some(t) = trunc {
        warns.push(format!("{} limit", format_size(t.get("maxBytes").and_then(Value::as_u64).unwrap_or(DEFAULT_MAX_BYTES))));
    }
    if lines_trunc {
        warns.push("some lines truncated".to_string());
    }
    out.push(Line::styled(format!("[Truncated: {}]", warns.join(", ")), theme.warning_style()));
}

/// find `renderResult` warnings (find.ts:98-105).
fn push_find_warnings(result: Option<&Value>, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let Some(result) = result else { return };
    let result_limit =
        result.get("details").and_then(|d| d.get("resultLimitReached")).and_then(Value::as_u64);
    let trunc = truncation(result);
    if result_limit.is_none() && trunc.is_none() {
        return;
    }
    let mut warns = Vec::new();
    if let Some(n) = result_limit {
        warns.push(format!("{n} results limit"));
    }
    if let Some(t) = trunc {
        warns.push(format!("{} limit", format_size(t.get("maxBytes").and_then(Value::as_u64).unwrap_or(DEFAULT_MAX_BYTES))));
    }
    out.push(Line::styled(format!("[Truncated: {}]", warns.join(", ")), theme.warning_style()));
}

/// ls `renderResult` warnings (ls.ts:84-91).
fn push_ls_warnings(result: Option<&Value>, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let Some(result) = result else { return };
    let entry_limit =
        result.get("details").and_then(|d| d.get("entryLimitReached")).and_then(Value::as_u64);
    let trunc = truncation(result);
    if entry_limit.is_none() && trunc.is_none() {
        return;
    }
    let mut warns = Vec::new();
    if let Some(n) = entry_limit {
        warns.push(format!("{n} entries limit"));
    }
    if let Some(t) = trunc {
        warns.push(format!("{} limit", format_size(t.get("maxBytes").and_then(Value::as_u64).unwrap_or(DEFAULT_MAX_BYTES))));
    }
    out.push(Line::styled(format!("[Truncated: {}]", warns.join(", ")), theme.warning_style()));
}

/// `shortenPath` (render-utils.ts:10-17): replace a leading `$HOME` with `~`.
fn shorten_path(path: &str) -> String {
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
        && let Some(rest) = path.strip_prefix(&home)
    {
        return format!("~{rest}");
    }
    path.to_string()
}

/// The number of WRAPPED display rows `lines` occupy at `width`, using the **same** word-wrap
/// `ratatui`'s `Paragraph::render` applies with `.wrap(Wrap { trim: false })`. ratatui 0.30's
/// `Paragraph::line_count(width)` runs the identical `WordWrapper` the renderer does, so the measured
/// height EXACTLY matches what render produces — the fix for the PROSE-WRAP truncation: a single
/// long paragraph (one logical [`Line`]) wraps to many display rows, and both the content-sized live
/// viewport ([`Transcript::content_height`]) and the scrollback flush must size to the wrapped count,
/// not the logical line count. `width == 0` degrades to the logical count (nothing renders anyway).
pub(crate) fn wrapped_height(lines: &[Line<'static>], width: usize) -> usize {
    if width == 0 {
        return lines.len();
    }
    Paragraph::new(lines.to_vec())
        .wrap(Wrap { trim: false })
        .line_count(width.min(u16::MAX as usize) as u16)
}

/// Render a single committed [`Entry`] into its styled scrollback line(s) at content `width`
/// (R-ARCH-TUI-003). Used by the shell to feed drained entries into `Terminal::insert_before`.
///
/// Assistant and user bodies render as **markdown** (spec/tui/06 §2) — multiple lines, no role
/// label: `assistant-message.ts:104-114` adds one `Markdown` child per text block and
/// `user-message.ts:38-58` one `Box`-wrapped `Markdown`, and neither contains a prefix (X1).
/// Prefix `leftMargin` to every ALREADY-WRAPPED row — the second half of `markdown.ts:334-340`
/// (`const lineWithMargins = leftMargin + line + rightMargin`), where `leftMargin =
/// " ".repeat(this.paddingX)` (`:329`) and `paddingX` is the caller's `outputPad`
/// (`assistant-message.ts:111` `new Markdown(text, this.outputPad, 0, …)`).
///
/// **Order matters and this is the back half of it.** Upstream wraps at `contentWidth` FIRST
/// (`:322`) and margins SECOND (`:340`); [`crate::markdown::render`] now does that wrap internally
/// (its `width` argument IS `contentWidth` — every call site passes `width - outputPad * 2`), so
/// every row handed here already fits and the indent lands on rows 1..N as well as on row 0. It
/// used to be inserted into the single unwrapped logical line and reflowed afterwards by the outer
/// `Paragraph::wrap` at FULL frame width, which is what put row 0 at column 1 and rows 1..N at
/// column 0 (L2) with no right gutter (M10).
///
/// `rightMargin` (`:330`) and the pad-to-`width` (`:346-348`) are not materialised, for the reason
/// [`text_lines`] gives: without a background they are an invisible trailing run of blanks in a
/// ratatui cell grid, and the gutter they buy comes from the narrower wrap instead. A no-op at
/// `pad == 0` (flush-left).
fn pad_lines(lines: &mut [Line<'static>], pad: usize) {
    if pad == 0 {
        return;
    }
    let indent = " ".repeat(pad);
    for line in lines.iter_mut() {
        line.spans.insert(0, Span::raw(indent.clone()));
    }
}

pub(crate) fn entry_lines(
    entry: &Entry,
    theme: &UiTheme,
    width: usize,
    output_pad: usize,
    images: ImageOpts<'_>,
) -> Vec<Line<'static>> {
    match entry {
        Entry::User { text, lead_spacer } => {
            // `UserMessageComponent` (`user-message.ts:38-58`) is exactly one child: a
            // `new Box(this.outputPad, 1, (c) => theme.bg("userMessageBg", c))` wrapping
            // `new Markdown(text, 0, 0, …, { color: userMessageText })`. The body is the block's
            // only content — there is **no** role label anywhere in the component (X1), and the
            // `userMessageBg` fill plus the `Box`'s `paddingY = 1` tinted blank above and below are
            // what identify a user turn (L1). The OSC-133 shell-zone markers at `:66-67` are
            // terminal shell-integration escapes the ratatui cell grid cannot carry.
            //
            // Content width is the `Box`'s `contentWidth = width - outputPad * 2` (`box.ts:79`),
            // which the inner `Markdown(…, 0, 0)` then passes through unchanged (`markdown.ts:284`
            // with `paddingX = 0`) — not `width - 5` (M9: that 5 was the width of the deleted
            // `"you: "` label).
            let role = theme.user_message_bg_style();
            let md = crate::markdown::render_with_text_color(
                text,
                width.saturating_sub(output_pad * 2).max(1),
                theme,
                role.fg,
            );
            // `applyBackgroundToLine` paints the BACKGROUND only (`box.ts:132-134`).
            let fill = match role.bg {
                Some(bg) => Style::default().bg(bg),
                None => Style::default(),
            };
            let mut out = box_lines(md, width, output_pad, 1, fill);
            // `chatContainer.addChild(new Spacer(1))` before the component — but `:3500` GATES it on
            // `this.chatContainer.children.length > 0`, so the very first thing in a fresh session's
            // chat gets no leading blank (`lead_spacer`, frozen at push time by
            // [`TranscriptView::chat_has_children`]). `:3514`, the user message that trails a skill
            // block, is UNgated and always passes `true`.
            //
            // The blank is also skipped when the component itself rendered nothing: `box_lines`
            // returns `[]` for an empty child set (`box.ts:75-77`/`:91-93`), and upstream never
            // reaches either the spacer or the component in that case because `:3499`'s
            // `if (textContent)` has already skipped the whole `case "user"`.
            if *lead_spacer && !out.is_empty() {
                out.insert(0, Line::default());
            }
            out
        }
        Entry::Assistant(text) => {
            // L3 — `assistant-message.ts:107` renders a text block only when `content.text.trim()`
            // is truthy, and `:96-98` counts it toward `hasVisibleContent` by the same trimmed test.
            // A whitespace-only message is therefore neither body nor blank.
            if text.trim().is_empty() {
                return Vec::new();
            }
            // `assistant-message.ts:104-114`: the body is `new Markdown(content.text.trim(),
            // this.outputPad, 0, …)` and nothing else — no role label (X1). `contentWidth =
            // width - outputPad * 2` (`markdown.ts:284`), not `width - 11` (M9: the 11 was
            // `"assistant: "`).
            let mut md = crate::markdown::render(
                text,
                width.saturating_sub(output_pad * 2).max(1),
                theme,
            );
            if md.is_empty() {
                md.push(Line::default());
            }
            // The `outputPad` horizontal padding (Pi `Markdown(content, outputPad, 0)`).
            pad_lines(&mut md, output_pad);
            // L3 — `assistant-message.ts:100-102`:
            // `if (hasVisibleContent) { this.contentContainer.addChild(new Spacer(1)); }`.
            // Gated above by the same trimmed predicate `hasVisibleContent` uses (`:96-98`), so the
            // blank cannot outlive its content. [`TranscriptView::commit_assistant`] refuses to
            // commit a whitespace-only turn for the same reason; the check is repeated here because
            // `Entry::Assistant` is public and reachable without going through it.
            md.insert(0, Line::default());
            md
        }
        Entry::Thinking { text, hidden } => {
            // The reasoning section (`assistant-message.ts:139-165`), padded like every other
            // assistant-side block. `hidden` was frozen at commit time (see [`Entry::Thinking`]).
            let mut out = thinking_lines(
                text,
                *hidden,
                width.saturating_sub(output_pad * 2),
                theme,
                images.hidden_thinking_label.unwrap_or(HIDDEN_THINKING_LABEL),
            );
            if out.is_empty() {
                return out;
            }
            pad_lines(&mut out, output_pad);
            // The same single `Spacer(1)` as the assistant arm (`:100-102`). Upstream renders one
            // component per assistant message, so the spacer lands on whichever visible block comes
            // first; the reasoning run always precedes the answer text in the content walk, and the
            // blank BETWEEN them is upstream's `hasVisibleContentAfter` spacer (`:166-168`). Two
            // cyrup entries each carrying one leading blank therefore reproduce upstream's
            // `[blank] thinking [blank] text` exactly.
            out.insert(0, Line::default());
            out
        }
        Entry::Tool(run) => {
            // X14 — a committed tool renders at the LIVE `this.toolOutputExpanded`, exactly like a
            // live one. Upstream has ONE `ToolExecutionComponent` per call and never swaps its
            // expansion when it scrolls: the component is seeded `setExpanded(this.toolOutputExpanded)`
            // at every construction site (`interactive-mode.ts:3165`, `:3239`, `:3437`, `:3486`,
            // `:3602`) and re-broadcast on every toggle (`setToolsExpanded`, `:4032-4046`), with
            // `toolOutputExpanded` defaulting to **false** (`:442`).
            //
            // This used to pass a hardcoded `true`, "so finalized scrollback keeps the complete
            // record". That is the GREEN-SLAB defect: a collapsed `read`'s `renderResult` returns
            // `""` upstream (`read.ts:178-180`) and `bash`/`grep`/`ls` cap at 10 rows, so upstream's
            // committed block is a 3-row header. Forcing `true` dumped the WHOLE file — every line
            // of it — inside the full-width `toolSuccessBg` box, so one `read` of a 500-line file
            // painted 500 rows of solid tool tint (Indexed(22), a vivid `#005f00`, once a
            // 256-colour terminal quantises `#283228`) straight over the conversation.
            tool_lines(run, images.tools_expanded, width, theme, images)
        }
        Entry::Bash(b) => {
            // Same rule for the `!`/`!!` block: `BashExecutionComponent` is `isExpandable` and takes
            // the same broadcast (`setToolsExpanded`, `:4032-4046`), so a committed one renders at
            // the live flag rather than force-expanded.
            let mut full = b.clone();
            full.set_expanded(images.tools_expanded);
            full.render_lines(width, theme, None, None)
        }
        Entry::SkillInvocation { name, content, lead_spacer } => {
            // `[skill]` label + bold name header, full content as markdown (the committed/expanded
            // form — `skill-invocation-message.ts` expanded branch). The leading spacer is the gated
            // `interactive-mode.ts:3500` one (see [`Entry::SkillInvocation`]).
            labeled_message_lines(
                "skill",
                &format!("**{name}**"),
                content,
                false,
                *lead_spacer,
                theme,
                width,
            )
        }
        Entry::Custom { label, body, rendered } => match rendered {
            // X15 — the renderer THREW. Pi does not silently drop the entry: `CustomEntryComponent`
            // catches and draws a failure box in its place (`components/custom-entry.ts:47-52`):
            //
            // ```ts
            // } catch (error) {
            //     const message = error instanceof Error ? error.message : String(error);
            //     const box = new Box(1, 1, (text) => theme.bg("customMessageBg", text));
            //     box.addChild(new Text(theme.fg("error", `[${this.entry.customType}] renderer failed: ${message}`), 0, 0));
            //     component = box;
            // }
            // ```
            //
            // — a `customMessageBg` box holding ONE `error`-coloured line, and then `:59-60`'s
            // `Spacer(1)` + the box, the same leading blank the success arm gets.
            Rendered::Failed(message) => {
                let block = theme.custom_message_bg_style();
                let fill = match block.bg {
                    Some(bg) => Style::default().bg(bg),
                    None => Style::default(),
                };
                let text = format!("[{label}] renderer failed: {message}");
                // `new Text(…, 0, 0)` inside a `Box(1, 1)`: paddingX 0, so the row wraps at the
                // box's own content width (`box.ts:79`) with no further margin.
                let children = text_lines(&text, width.saturating_sub(2).max(1), 0, theme.error_style());
                let mut out = box_lines(children, width, 1, 1, fill);
                if !out.is_empty() {
                    out.insert(0, Line::default());
                }
                out
            }
            // EXT-006: an extension registered a renderer for this custom type, so ITS output is
            // the block (Pi hands the resolved renderer to `CustomMessageComponent` in place of the
            // default framing, interactive-mode.ts:3324-3336). Emitted verbatim — the renderer
            // already owns the presentation, so no `[label]` bracket is added.
            Rendered::Text(text) => {
                // `CustomMessageComponent` adds its `Spacer(1)` in the CONSTRUCTOR
                // (`custom-message.ts:33`), before `rebuild()` chooses between the custom renderer
                // (`:79`) and the default box (`:88`), so both arms carry the leading blank.
                let mut out = vec![Line::default()];
                // X11 — `custom-message.ts:76-81` is `this.customComponent = component;
                // this.addChild(component); return;`: the component is added AS-IS and the host
                // applies no colour of its own. cyrup re-styled every row `dim`, which overrode
                // whatever the extension had chosen — the one thing `renderShell: "self"`/a custom
                // renderer exists to prevent. Rows go out unstyled so the terminal default (and any
                // styling the renderer expressed) survives.
                out.extend(text.split('\n').map(|l| Line::raw(l.to_string())));
                out
            }
            // A bracketed extension-type label + the markdown body (`custom-message.ts`).
            // `custom-message.ts:33`'s constructor `Spacer(1)` — unconditional.
            Rendered::None => labeled_message_lines(label, "", body, true, true, theme, width),
        },
        Entry::BranchSummary { summary } => {
            // X14 — `BranchSummaryMessageComponent` is a `Box(1, 1, customMessageBg)` whose body
            // depends on `expanded`, which `interactive-mode.ts:3493` seeds from
            // `this.toolOutputExpanded` and `setToolsExpanded` re-broadcasts on every toggle
            // (`:4032-4046` walks `chatContainer.children` calling `setExpanded`), so the LIVE flag
            // is read here (`branch-summary-message.ts:11,22-25,32-56`). COLLAPSED it is one
            // row, not the whole summary:
            //
            // ```ts
            // theme.fg("customMessageText", "Branch summary (") +
            //     theme.fg("dim", keyText("app.tools.expand")) +
            //     theme.fg("customMessageText", " to expand)")
            // ```
            //
            // Note the two-tone split is `customMessageText`/`dim`, NOT `keyHint`'s `muted`/`dim`.
            // `interactive-mode.ts:3491` is UNgated — the branch summary always gets its blank.
            if images.tools_expanded {
                labeled_message_lines(
                    "branch",
                    "**Branch Summary**",
                    summary,
                    true,
                    true,
                    theme,
                    width,
                )
            } else {
                collapsed_summary_lines("branch", "Branch summary (", images.expand_key, theme, width)
            }
        }
        Entry::CompactionSummary { tokens_before, summary } => {
            // X14 — the same collapsed form (and the same LIVE `toolOutputExpanded` read), with the
            // token count in the lead (`compaction-summary-message.ts:48-56`):
            // `fg("customMessageText", `Compacted from ${tokenStr} tokens (`) + fg("dim", keyText(…)) + fg("customMessageText", " to expand)")`.
            if !images.tools_expanded {
                let lead =
                    format!("Compacted from {} tokens (", group_thousands(*tokens_before));
                return collapsed_summary_lines(
                    "compaction",
                    &lead,
                    images.expand_key,
                    theme,
                    width,
                );
            }
            let header = format!("**Compacted from {} tokens**", group_thousands(*tokens_before));
            // `interactive-mode.ts:3484` is UNgated too.
            labeled_message_lines("compaction", &header, summary, true, true, theme, width)
        }
        Entry::Status(text) => {
            // X18 — `showStatus` (`interactive-mode.ts:3411-3429`) is two chat children and nothing
            // more:
            //
            // ```ts
            // const spacer = new Spacer(1);
            // const text = new Text(theme.fg("dim", message), 1, 0);
            // this.chatContainer.addChild(spacer);
            // this.chatContainer.addChild(text);
            // ```
            //
            // so a status row is a leading blank plus a `dim` `Text` at **paddingX 1** — the same
            // one-column inset every other chat child sits at (`text.ts:64`, `:70-76`). There is no
            // bullet: `git grep "•" v0.84.1 -- packages/coding-agent/src/modes/interactive` finds no
            // status glyph, and `showStatus` interpolates nothing before `message`. The `• ` prefix
            // and the flush-left placement were both cyrup inventions.
            //
            // The spacer is UNgated here — unlike `:3500`, `:3424` has no
            // `chatContainer.children.length` test.
            let mut out = vec![Line::default()];
            out.extend(text_lines(text, width, 1, theme.dim_style()));
            out
        }
        Entry::Warning(text) => {
            // Pi `showWarning` (`interactive-mode.ts:3884-3888` @v0.83.0): `Spacer(1)` then
            // `Text(theme.fg("warning", …), 1, 0)` — the `Error` shape in the warning colour.
            //
            // TUI-062(a) — the cite used to read `:3956-3960`, which is `getAllQueuedMessages` /
            // `clearAllQueues` at that tag, not `showWarning`. Re-read at v0.83.0: `showError` is
            // `:3878-3882` and `showWarning` immediately follows at `:3884-3888`. **The backlog's
            // own proposed correction (`:3885-3889`) is also off by one** — `:3885` is the `Spacer`,
            // i.e. the first line of the BODY, and `:3889` is the blank line after the closing
            // brace.
            //
            // TUI-062(b), the design half, is unchanged and deliberate: pi builds
            // `Warning: ${warningMessage}` INSIDE `showWarning` (`:3886`), while this arm renders
            // `text` verbatim, so the prefix stays a per-caller obligation. Two callers that are
            // ports of `showWarning` supply it (`app.rs:3626`, `crates/cyrup/src/main.rs`'s
            // `modelFallbackMessage` push); the project-trust banner (`app.rs`'s
            // `render_project_trust_warning_if_needed`) correctly does NOT, because pi's banner is a
            // raw warning-coloured `Text` (`:3505`) and never goes through `showWarning`. Moving the
            // prefix in here would therefore have to be conditional, which is why it has not been.
            let mut out = vec![Line::default()];
            out.extend(text_lines(text, width, output_pad, theme.warning_style()));
            out
        }
        Entry::Error(text) => {
            // Pi: `Spacer(1)` then `Text(theme.fg("error", text), outputPad, 0)`
            // (assistant-message.ts:180, :189, :193). A `Text` WRAPS at
            // `contentWidth = width - paddingX * 2` (`text.ts:64`) and margins each produced row
            // (`:70-76`) — it does not hand one long logical line to an outer reflow. cyrup did the
            // latter, so a long error printed row 0 at column `outputPad` and every continuation row
            // at column 0, the same L2 defect the markdown body had.
            let mut out = vec![Line::default()];
            out.extend(text_lines(text, width, output_pad, theme.error_style()));
            out
        }
        Entry::Block { title, markdown } => {
            // Both upstream instances of this stack are identical — `/changelog`
            // (interactive-mode.ts:6067-6072) and `/hotkeys` (:6197-6203); `git grep -n
            // "new DynamicBorder()" v0.84.1 -- .../interactive-mode.ts` finds exactly six sites and
            // four of them are those two pairs. Each is:
            //
            //   Spacer(1) / DynamicBorder() / Text(bold(accent(title)), 1, 0) / Spacer(1) /
            //   Markdown(body, 1, 1, theme) / DynamicBorder()
            //
            // The last two constructor arguments are `(paddingX, paddingY)` — **not**
            // `(paddingX, leftMargin)`: `markdown.ts:250-260` binds the third parameter to
            // `this.paddingY`, and the left margin is derived from paddingX alone
            // (`markdown.ts:329` `leftMargin = " ".repeat(this.paddingX)`). So the body is inset by
            // ONE column on both sides (content width `width - 2`, `markdown.ts:284`) and carries one
            // blank row above AND below it (`markdown.ts:352-361`), and the title is inset by one
            // column too (`Text`'s own `paddingX`, `text.ts:60-87`). Only the two `─` rules run
            // edge to edge.
            let w = width.max(1);
            let rule = "─".repeat(w);
            let bold = theme.accent_style().add_modifier(ratatui::style::Modifier::BOLD);
            let mut out: Vec<Line<'static>> = vec![
                Line::default(),
                Line::styled(rule.clone(), theme.border_style()),
            ];
            out.extend(text_lines_of(&Line::styled(title.clone(), bold), w, 1));
            out.push(Line::default());
            // `markdown.ts:288-296` returns EARLY on blank text, before the paddingY block, so an
            // empty body contributes no rows at all — not two blanks.
            if !markdown.trim().is_empty() {
                let mut md = crate::markdown::render(markdown, w.saturating_sub(2).max(1), theme);
                pad_lines(&mut md, 1);
                out.push(Line::default());
                out.extend(md);
                out.push(Line::default());
            }
            out.push(Line::styled(rule, theme.border_style()));
            out
        }
        Entry::LoadedResources(lines) => {
            crate::startup::startup_lines(lines, theme, width.max(1), output_pad)
        }
    }
}

/// Render a labeled extension/system message (`skill`/`custom`/`branch`/`compaction` variants),
/// then the optional bold `header` + the `body` rendered as markdown. The committed scrollback form
/// is the *expanded* render (the complete record), like committed tools.
///
/// T9 (TUI-FIDELITY §2): the `[label]` bracket is Pi's `customMessageLabel` token, not `accent`.
/// All four upstream components build it identically — `theme.fg("customMessageLabel",
/// "\x1b[1m[<name>]\x1b[22m")` — at v0.84.1
/// `coding-agent/src/modes/interactive/components/skill-invocation-message.ts:38`,
/// `custom-message.ts:92`, `branch-summary-message.ts:35` and `compaction-summary-message.ts:36`.
/// The `\x1b[1m…\x1b[22m` pair is SGR bold, so the bold stays; only the colour role changes
/// (`dark.json:41` `#9575cd`, `light.json:40` `#7e57c2` — purple, where cyrup was painting the teal
/// accent).
///
/// T9 continued — `customMessageBg` + `customMessageText`. All four upstream components are (or
/// wrap) a `Box` whose fill is `theme.bg("customMessageBg", …)` and hand their body to
/// `new Markdown(…, { color: (text) => theme.fg("customMessageText", text) })`:
/// `custom-message.ts:36,107-111`, `skill-invocation-message.ts:17,42-44`,
/// `branch-summary-message.ts:16,42-44`, `compaction-summary-message.ts:16,43-45`. Both tokens were
/// dead on screen — [`UiTheme::custom_message_bg_style`] had zero callers — so the block drew no
/// fill and the body took the plain `text` role. The fill goes on `Line::style` (the same mechanism
/// the `userMessageBg` block uses) and the body colour goes through
/// [`crate::markdown::render_with_text_color`], because a span-level `fg` set by the markdown
/// renderer would otherwise mask a line-level one.
///
/// X2 — the block shell. All four components are (or extend) `new Box(1, 1, (t) =>
/// theme.bg("customMessageBg", t))`, so the body sits in a **1-column inset** and the box emits a
/// tinted blank row above and below it (`box.ts:79-88`, `:106-119`). Both were missing: the label
/// started at column 0 and the purple band was exactly as tall as its content.
///
/// X2 — the `Spacer(1)` after the label. `custom-message.ts:94`, `branch-summary-message.ts:37` and
/// `compaction-summary-message.ts:38` each `addChild(new Spacer(1))` immediately after the label
/// `Text`; `skill-invocation-message.ts` does **not** (`:36-45` is label then `Markdown`, `:47-53`
/// is one collapsed line). Hence `spacer_after_label` rather than an unconditional blank — the row
/// is a property of three of the four components, not of the shared shell.
///
/// `lead_spacer` is the `chatContainer.addChild(new Spacer(1))` that precedes the component. It is
/// **not** uniform across the call sites, so each one passes its own answer:
/// `interactive-mode.ts:3484` (compaction) and `:3491` (branch) are unconditional, whereas `:3500`
/// — which covers the skill component, since `:3506` sits inside it — is gated on
/// `this.chatContainer.children.length > 0`. A custom message supplies its own in the constructor
/// (`custom-message.ts:33`), also unconditional.
/// X14 — the COLLAPSED branch/compaction summary: the same `Box(1, 1, customMessageBg)` +
/// `[label]` + `Spacer(1)` shell [`labeled_message_lines`] builds, but with one `Text` row in place
/// of the markdown body (`branch-summary-message.ts:46-56`, `compaction-summary-message.ts:47-56`).
///
/// ```ts
/// this.addChild(new Text(
///     theme.fg("customMessageText", "Branch summary (") +
///         theme.fg("dim", keyText("app.tools.expand")) +
///         theme.fg("customMessageText", " to expand)"),
///     0, 0));
/// ```
///
/// Three runs, and the outer two are `customMessageText` — NOT `muted`. This is not `keyHint`; the
/// two components spell the pair out by hand and only the key label shares `dim` with it. `lead`
/// carries the trailing `(` so the compaction variant can interpolate its token count
/// (`Compacted from 12,345 tokens (`).
fn collapsed_summary_lines(
    label: &str,
    lead: &str,
    expand_key: &str,
    theme: &UiTheme,
    width: usize,
) -> Vec<Line<'static>> {
    let block = theme.custom_message_bg_style();
    let text = theme.custom_message_text_style();
    let content_width = width.saturating_sub(2).max(1);
    let row = Line::from(vec![
        Span::styled(lead.to_string(), text),
        Span::styled(expand_key.to_string(), theme.dim_style()),
        Span::styled(" to expand)".to_string(), text),
    ]);
    let mut children =
        vec![Line::styled(format!("[{label}]"), theme.custom_message_label_style())];
    children.push(Line::default());
    // `new Text(…, 0, 0)` — paddingX 0 inside the `Box`, so the row wraps at the box's own content
    // width with no extra margin.
    children.extend(text_lines_of(&row, content_width, 0));
    let fill = match block.bg {
        Some(bg) => Style::default().bg(bg),
        None => Style::default(),
    };
    let mut out = box_lines(children, width, 1, 1, fill);
    // `interactive-mode.ts:3484`/`:3491` — the leading `Spacer(1)` is unconditional for both.
    if !out.is_empty() {
        out.insert(0, Line::default());
    }
    out
}

fn labeled_message_lines(
    label: &str,
    header: &str,
    body: &str,
    spacer_after_label: bool,
    lead_spacer: bool,
    theme: &UiTheme,
    width: usize,
) -> Vec<Line<'static>> {
    let block = theme.custom_message_bg_style();
    // `Box(1, 1)` renders its children at `contentWidth = width - 2` (`box.ts:79`).
    let content_width = width.saturating_sub(2).max(1);
    let mut children =
        vec![Line::styled(format!("[{label}]"), theme.custom_message_label_style())];
    if spacer_after_label {
        children.push(Line::default());
    }
    let md_src = if header.is_empty() {
        body.to_string()
    } else if body.is_empty() {
        header.to_string()
    } else {
        format!("{header}\n\n{body}")
    };
    if !md_src.is_empty() {
        children.extend(crate::markdown::render_with_text_color(
            &md_src,
            content_width,
            theme,
            block.fg,
        ));
    }
    // The `customMessageBg` fill covers the whole box — padding rows and label row included.
    // A theme that omits the token leaves `bg` `None` and the terminal default shows through.
    // `applyBackgroundToLine` paints the BACKGROUND only (`box.ts:132-134`); the foreground comes
    // from the content, which already carries `customMessageText` via `render_with_text_color`.
    let fill = match block.bg {
        Some(bg) => Style::default().bg(bg),
        None => Style::default(),
    };
    let mut out = box_lines(children, width, 1, 1, fill);
    // The leading `Spacer(1)` — see `lead_spacer` above. Skipped when the `Box` produced no rows at
    // all (`box.ts:75-77`/`:91-93`), so a contentless block cannot leave an orphan blank behind.
    if lead_spacer && !out.is_empty() {
        out.insert(0, Line::default());
    }
    out
}

/// Group an integer with `,` thousands separators (Pi `Number.toLocaleString()` for the compaction
/// token count). Pure ASCII; never allocates beyond the result.
fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    let len = bytes.len();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

impl Component for TranscriptView {
    /// Render the active turn **inline** (no box/title — `assistant-message.ts:84-93`, spec/tui/01 §3):
    /// the streaming partial is a wrapped `Paragraph` filling the region, tail-anchored so the newest
    /// text stays visible as it grows (spec/tui/01 §3 overflow).
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let width = area.width as usize;
        let (total, lines) = {
            let cache = self.cached_render(width, theme);
            (cache.lines.len(), cache.lines.clone())
        };
        // Auto-scroll: keep the tail (newest text) visible when content exceeds the region height,
        // minus any user page-up offset (clamped so it can never scroll past the top).
        let inner_h = area.height as usize;
        let max_scroll = total.saturating_sub(inner_h);
        self.scroll_offset = self.scroll_offset.min(max_scroll);
        let scroll = max_scroll.saturating_sub(self.scroll_offset).min(u16::MAX as usize) as u16;
        let para = Paragraph::new(lines)
            .style(theme.base_style())
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0));
        frame.render_widget(para, area);
    }
}

/// Join the `Text` content blocks of a message body into a single string (drops thinking/tool/image
/// blocks). Operates on `cyrup_core::Content`, which is in the dependency set.
pub fn content_text(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// Coalesce the `Thinking` content blocks of a message into one section, joined by `\n\n` — Pi's
/// inner run-collecting loop (`assistant-message.ts:116-127`), which trims each block and skips the
/// empty ones. `redacted` blocks carry no readable text and are dropped with the rest of the empties.
///
/// Pi keeps *runs* of adjacent thinking blocks separate (a text block between two runs starts a new
/// section); cyrup's transcript carries a single reasoning block per turn, so every run of a message
/// folds into one — the difference is only visible when a model interleaves text and thinking.
pub fn thinking_text(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Thinking { thinking, .. } => Some(thinking.trim()),
            _ => None,
        })
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// A skill block parsed out of a submitted/replayed user message (Pi `ParsedSkillBlock`,
/// agent-session.ts:103).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedSkillBlock {
    /// The skill name (`<skill name="…">`).
    pub name: String,
    /// The skill location (`location="…">`) — the on-disk path the skill expanded from.
    pub location: String,
    /// The skill block body (markdown between the open/close tags).
    pub content: String,
    /// The trailing user message after the block, if any (`\n\n{message}`).
    pub user_message: Option<String>,
}

/// Parse a `<skill name="…" location="…">\n…\n</skill>(\n\n{userMessage})?` block out of message text
/// (Pi `parseSkillBlock`, agent-session.ts:114, a hand-port of its anchored regex — no regex dep).
/// Returns `None` for any text that is not exactly such a block.
pub fn parse_skill_block(text: &str) -> Option<ParsedSkillBlock> {
    let rest = text.strip_prefix("<skill name=\"")?;
    let (name, rest) = rest.split_once('"')?;
    if name.is_empty() {
        return None;
    }
    let rest = rest.strip_prefix(" location=\"")?;
    let (location, rest) = rest.split_once('"')?;
    if location.is_empty() {
        return None;
    }
    let rest = rest.strip_prefix(">\n")?;
    // Non-greedy: the body runs to the FIRST `\n</skill>` (`[\s\S]*?`).
    let close = rest.find("\n</skill>")?;
    let content = rest[..close].to_string();
    let after = &rest[close + "\n</skill>".len()..];
    let user_message = if after.is_empty() {
        None
    } else {
        // Must be `\n\n{message}` to end (the regex's optional `(?:\n\n([\s\S]+))?$`).
        let um = after.strip_prefix("\n\n")?.trim();
        (!um.is_empty()).then(|| um.to_string())
    };
    Some(ParsedSkillBlock { name: name.to_string(), location: location.to_string(), content, user_message })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod output_pad_tests {
    use super::*;

    fn line_text(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// A user entry whose `interactive-mode.ts:3500` gate is already decided — this module is about
    /// the pad, not about where the leading `Spacer(1)` comes from.
    fn user(text: &str, lead_spacer: bool) -> Entry {
        Entry::User { text: text.to_string(), lead_spacer }
    }

    /// F12: a fresh transcript defaults to Pi's `outputPad = 1` and `set_output_pad` drives it.
    #[test]
    fn output_pad_defaults_to_one_and_is_settable() {
        let mut view = TranscriptView::new();
        assert_eq!(view.output_pad(), 1, "Pi's default outputPad is 1");
        view.set_output_pad(0);
        assert_eq!(view.output_pad(), 0);
    }

    /// `outputPad` left-indents the message BODY; `0` renders flush-left, `1` prepends a single
    /// leading column.
    ///
    /// X1 — there is no `you: ` / `assistant: ` label to indent past. `user-message.ts:38-58` adds
    /// exactly one child (a `Box` wrapping a `Markdown`) and `assistant-message.ts:104-114` adds one
    /// `Markdown` per text block; neither component contains a role prefix.
    ///
    /// L1/L3 — the user block's first two rows are the leading `Spacer(1)`
    /// (`interactive-mode.ts:3501`) and the `Box`'s top `paddingY` row (`box.ts:107-109`); the
    /// assistant block's first row is `assistant-message.ts:100-102`'s `Spacer(1)`.
    #[test]
    fn output_pad_left_indents_committed_messages() {
        let theme = UiTheme::dark();
        // pad = 1 → the body starts one column in.
        let u1 = entry_lines(&user("hello", true), &theme, 80, 1, ImageOpts::default());
        assert_eq!(line_text(&u1[0]), "", "user leading Spacer(1): {:?}", line_text(&u1[0]));
        assert_eq!(line_text(&u1[1]).trim(), "", "user top paddingY row: {:?}", line_text(&u1[1]));
        assert!(line_text(&u1[2]).starts_with(" hello"), "pad=1 user: {:?}", line_text(&u1[2]));
        let a1 = entry_lines(&Entry::Assistant("hi".into()), &theme, 80, 1, ImageOpts::default());
        assert_eq!(line_text(&a1[0]), "", "assistant leading Spacer(1)");
        assert_eq!(line_text(&a1[1]), " hi", "pad=1 assistant: {:?}", line_text(&a1[1]));
        // pad = 0 → flush-left (no leading space).
        let u0 = entry_lines(&user("hello", true), &theme, 80, 0, ImageOpts::default());
        assert!(line_text(&u0[2]).starts_with("hello"), "pad=0 user: {:?}", line_text(&u0[2]));
        let a0 = entry_lines(&Entry::Assistant("hi".into()), &theme, 80, 0, ImageOpts::default());
        assert_eq!(line_text(&a0[1]), "hi", "pad=0 assistant: {:?}", line_text(&a0[1]));

        // MIRROR (X1): no role label at any pad, in either arm.
        for pad in [0usize, 1] {
            for e in [user("hello", true), Entry::Assistant("hi".into())] {
                let joined: String = entry_lines(&e, &theme, 80, pad, ImageOpts::default())
                    .iter()
                    .map(line_text)
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(!joined.contains("you:"), "pad={pad}: {joined:?}");
                assert!(!joined.contains("assistant:"), "pad={pad}: {joined:?}");
            }
        }
    }

    /// The live streaming partial honors the pad too (Pi keeps the outputPad on the in-flight
    /// `AssistantMessageComponent`). Rendering the active region with pad=1 vs pad=0 shifts the line.
    ///
    /// Row 0 is L3's `Spacer(1)` (`assistant-message.ts:100-102`), which the live view emits for the
    /// same reason the committed arm does — it is one component either side of the commit.
    #[test]
    fn output_pad_indents_the_live_streaming_partial() {
        let theme = UiTheme::dark();
        let mut view = TranscriptView::new();
        view.push_assistant_delta("streaming answer");
        let padded = view.lines(80, &theme);
        assert_eq!(line_text(&padded[0]), "", "leading Spacer(1) missing");
        assert_eq!(line_text(&padded[1]), " streaming answer", "pad=1 live");
        view.set_output_pad(0);
        let flush = view.lines(80, &theme);
        assert_eq!(line_text(&flush[1]), "streaming answer", "pad=0 live");

        // MIRROR (X1): no `assistant: ` label and no `▌` caret in the live region either.
        // `git grep "▌" v0.84.1 -- packages/` finds only `examples/extensions/custom-header.ts:22`.
        let joined: String = flush.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(!joined.contains("assistant:"), "live label: {joined:?}");
        assert!(!joined.contains('\u{258c}'), "live caret: {joined:?}");
    }

    /// A sentence long enough to wrap several times at any of the widths this module tests.
    const LONG: &str = "The quick brown fox jumps over the lazy dog and then keeps running for \
                        quite a long while indeed before it finally stops.";

    /// **L2 + M10** — EVERY row of a multi-row message carries the `outputPad` margin, and no row
    /// reaches the last column.
    ///
    /// `markdown.ts:316-326` wraps at `contentWidth = width - paddingX * 2` (`:284`) and only then
    /// does `:334-340` emit `leftMargin + line + rightMargin` for **each** produced row. cyrup used
    /// to insert the margin into the single unwrapped logical line and let the outer
    /// `Paragraph::wrap` reflow it at full frame width, so row 0 started at column 1 and rows 1..N
    /// at column 0 — a ragged left edge on nearly every turn — with nothing holding a right gutter.
    #[test]
    fn l2_every_wrapped_row_of_a_message_carries_the_margin_and_a_right_gutter() {
        let theme = UiTheme::dark();
        for width in [20usize, 40, 80] {
            let rows = entry_lines(&Entry::Assistant(LONG.into()), &theme, width, 1, ImageOpts::default());
            // Row 0 is `assistant-message.ts:100-102`'s `Spacer(1)`; the body follows.
            let body = &rows[1..];
            assert!(body.len() > 1, "width={width}: expected a wrapped body, got {body:?}");
            for row in body {
                let t = line_text(row);
                assert!(t.starts_with(' '), "width={width}: row lost its leftMargin: {t:?}");
                assert!(!t.starts_with("  "), "width={width}: over-indented row: {t:?}");
                // `contentWidth = width - paddingX*2` plus one column of `leftMargin` — the last
                // column stays empty, which is the `rightMargin` (`markdown.ts:330`/`:340`).
                assert!(row.width() < width, "width={width}: no right gutter: {t:?} ({})", row.width());
            }
            // MIRROR: at `outputPad = 0` there is no margin, and the wrap uses the full width.
            let flush = entry_lines(&Entry::Assistant(LONG.into()), &theme, width, 0, ImageOpts::default());
            for row in &flush[1..] {
                assert!(row.width() <= width, "pad=0 width={width}: {:?}", line_text(row));
            }
            assert!(!line_text(&flush[1]).starts_with(' '), "pad=0 must be flush-left");
        }

        // MIRROR: a short message still occupies exactly one body row, and an empty turn none.
        let short = entry_lines(&Entry::Assistant("hi".into()), &theme, 80, 1, ImageOpts::default());
        assert_eq!(short.len(), 2, "spacer + one row: {short:?}");
        assert!(entry_lines(&Entry::Assistant("   ".into()), &theme, 80, 1, ImageOpts::default())
            .is_empty());
    }

    /// The same for the LIVE streaming partial (`transcript.rs:1000`'s call site) — the row a user
    /// watches for the whole turn.
    #[test]
    fn l2_live_streaming_partial_wraps_inside_its_own_padding() {
        let theme = UiTheme::dark();
        let mut view = TranscriptView::new();
        view.push_assistant_delta(LONG);
        let rows = view.lines(40, &theme);
        assert!(rows.len() > 2, "expected a wrapped live body: {rows:?}");
        for row in &rows[1..] {
            let t = line_text(row);
            assert!(t.starts_with(' '), "live row lost its leftMargin: {t:?}");
            assert!(row.width() <= 39, "live row has no right gutter: {t:?}");
        }
    }

    /// **Edit 6** — a long `Entry::Error` / `Entry::Warning` is a `Text`, and a `Text` WRAPS at
    /// `contentWidth = width - paddingX * 2` (`text.ts:64`) before prefixing `leftMargin` to each
    /// produced row (`:70-76`).
    ///
    /// `assistant-message.ts:180`/`:189`/`:193` construct them as `new Text(theme.fg("error", …),
    /// this.outputPad, 0)`; `interactive-mode.ts:3956-3960` does the same in the warning colour.
    /// cyrup pushed ONE unwrapped logical line and `pad_lines`'d it, i.e. the L2 defect again.
    #[test]
    fn error_and_warning_rows_wrap_inside_the_output_pad() {
        let theme = UiTheme::dark();
        for entry in [Entry::Error(LONG.into()), Entry::Warning(LONG.into())] {
            let rows = entry_lines(&entry, &theme, 40, 1, ImageOpts::default());
            assert_eq!(line_text(&rows[0]), "", "leading Spacer(1)");
            assert!(rows.len() > 2, "expected a wrapped body: {rows:?}");
            for row in &rows[1..] {
                let t = line_text(row);
                assert!(t.starts_with(' '), "row lost its leftMargin: {t:?}");
                assert!(row.width() <= 39, "row has no right gutter: {t:?}");
            }
            // The colour rides on the span, inside the margins (`theme.fg("error", text)`).
            assert!(rows[1].spans.iter().any(|s| s.style.fg.is_some()), "colour lost: {rows:?}");
        }
    }

    /// **CFG-051** — the migrated-credential notice must RENDER, verbatim, and BEFORE the
    /// model-fallback warning.
    ///
    /// pi shows the line inside the running UI — `if (migratedProviders && migratedProviders.length
    /// > 0) { this.showWarning(\`Migrated credentials to auth.json: ${migratedProviders.join(", ")}\`); }`
    /// (`interactive-mode.ts:874-876` @v0.83.0) — ahead of the `modelFallbackMessage` warning
    /// (`:883-885`). cyrup pushes both from `run_interactive` in that order
    /// (`crates/cyrup/src/main.rs:1940` then `:1946`), and the STRING is pinned on that side by
    /// `the_migrated_credential_notice_is_pis_line_and_is_absent_when_nothing_moved`.
    ///
    /// What no test pinned — the residual REPRO-LOG carried for this row — is the RENDER: a string
    /// pushed into `pending` is only a notice if `entry_lines` (the production path, `app.rs:1851`)
    /// actually emits it. `Entry::Warning` renders its text VERBATIM, which is why `Warning: ` is a
    /// per-caller obligation here (TUI-062) — so a renderer that re-prefixed, truncated or dropped
    /// the line would leave the string test green and the user with nothing on screen.
    #[test]
    fn the_migrated_credential_notice_renders_first_and_verbatim_in_the_transcript() {
        // The two production lines, in `run_interactive` order. Deliberately DISTINCT values (two
        // providers, a comma join) so a renderer that emitted the wrong entry cannot pass.
        const MIGRATED: &str = "Warning: Migrated credentials to auth.json: anthropic, openai";
        const FALLBACK: &str = "Warning: No models available.";
        let theme = UiTheme::dark();
        let mut view = TranscriptView::new();
        view.push_warning(MIGRATED);
        view.push_warning(FALLBACK);
        // PRESENCE before absence: an empty queue would make every row assertion below vacuous.
        assert_eq!(view.pending().len(), 2, "both warnings queued: {:?}", view.pending());

        // The production render path: `app.rs:1851` maps every entry through `entry_lines` at the
        // transcript's own `output_pad`. Width 100 is wider than either line, so a row that does
        // not match exactly is a render defect, not a wrap.
        let rows: Vec<Line<'static>> = view
            .pending()
            .iter()
            .flat_map(|e| entry_lines(e, &theme, 100, view.output_pad(), ImageOpts::default()))
            .collect();
        let text: Vec<String> = rows.iter().map(line_text).collect();

        let migrated_at = text
            .iter()
            .position(|r| r.trim() == MIGRATED)
            .unwrap_or_else(|| panic!("the migrated-credential notice never rendered: {text:?}"));
        let fallback_at = text
            .iter()
            .position(|r| r.trim() == FALLBACK)
            .unwrap_or_else(|| panic!("the model-fallback warning never rendered: {text:?}"));
        assert!(
            migrated_at < fallback_at,
            "pi renders the migrated-credential notice (`:874-876`) BEFORE the modelFallbackMessage \
             warning (`:883-885`); got {text:?}"
        );
        // Verbatim: exactly one `Warning: `, no second prefix from the renderer.
        assert_eq!(
            text[migrated_at].matches("Warning: ").count(),
            1,
            "the renderer must not re-prefix a verbatim `Entry::Warning`: {:?}",
            text[migrated_at]
        );
        // …and in the warning colour, not the default foreground.
        assert_eq!(
            rows[migrated_at].spans.iter().find_map(|s| s.style.fg),
            theme.warning_style().fg,
            "the notice must render in the warning colour (`theme.fg(\"warning\", …)`)"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod skill_tests {
    use super::*;

    #[test]
    fn parses_a_skill_block_with_trailing_user_message() {
        let text = "<skill name=\"deploy\" location=\"/skills/deploy.md\">\nRun the deploy steps.\n</skill>\n\nplease deploy prod";
        let block = parse_skill_block(text).expect("should parse");
        assert_eq!(block.name, "deploy");
        assert_eq!(block.location, "/skills/deploy.md");
        assert_eq!(block.content, "Run the deploy steps.");
        assert_eq!(block.user_message.as_deref(), Some("please deploy prod"));
    }

    #[test]
    fn parses_a_skill_block_without_user_message() {
        let text = "<skill name=\"lint\" location=\"/s/lint.md\">\nlint body\nmore\n</skill>";
        let block = parse_skill_block(text).expect("should parse");
        assert_eq!(block.name, "lint");
        assert_eq!(block.content, "lint body\nmore");
        assert_eq!(block.user_message, None);
    }

    #[test]
    fn plain_text_is_not_a_skill_block() {
        assert_eq!(parse_skill_block("just a normal message"), None);
        // A single newline after `</skill>` (not `\n\n`) is not a valid trailer.
        assert_eq!(
            parse_skill_block("<skill name=\"x\" location=\"y\">\nz\n</skill>\noops"),
            None
        );
    }

    #[test]
    fn push_user_splits_a_skill_block_into_two_entries() {
        let mut view = TranscriptView::new();
        view.push_user(
            "<skill name=\"deploy\" location=\"/s/d.md\">\nbody\n</skill>\n\nrun it",
        );
        let entries = view.pending();
        assert!(matches!(entries.first(), Some(Entry::SkillInvocation { name, .. }) if name == "deploy"));
        assert!(matches!(entries.get(1), Some(Entry::User { text, .. }) if text == "run it"));
    }

    #[test]
    fn push_user_keeps_plain_text_as_one_entry() {
        let mut view = TranscriptView::new();
        view.push_user("hello world");
        let entries = view.pending();
        assert_eq!(entries.len(), 1);
        assert!(matches!(entries.first(), Some(Entry::User { text, .. }) if text == "hello world"));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod progressive_commit_tests {
    use super::*;

    fn tool_names(entries: &[Entry]) -> Vec<&str> {
        entries
            .iter()
            .filter_map(|e| match e {
                Entry::Tool(run) => Some(run.name.as_str()),
                _ => None,
            })
            .collect()
    }

    /// The primary SCREEN-FILL fix: finished tools commit progressively (leaving `active_tools`), so a
    /// long multi-tool turn never stacks the whole turn in the live viewport. `content_height` (which
    /// sizes the viewport) stays bounded to the running tail.
    #[test]
    fn finished_tools_commit_progressively_and_content_height_stays_bounded() {
        let theme = UiTheme::dark();
        let mut view = TranscriptView::new();
        let tall_before = {
            // Simulate 20 finished tool calls arriving one at a time (the reported storm).
            for i in 0..20u32 {
                let name = format!("read_{i}");
                view.push_tool_start(name.clone(), serde_json::json!({ "path": format!("file_{i}.md") }));
                view.push_tool_end(
                    name,
                    false,
                    Some(format!("body of file {i}\nsecond line\nthird").into()),
                );
                // The app drains finished-leading tools after every ToolExecutionEnd.
                view.commit_finished_leading_tools();
            }
            view.content_height(80, &theme)
        };
        // All 20 committed to `pending` (headed for native scrollback), none left live.
        assert_eq!(view.active_tools().len(), 0, "finished tools must not accumulate live");
        assert_eq!(tool_names(view.pending()).len(), 20, "all finished tools should be committed");
        // The live region measured near-empty (no tail): bounded, not full-screen.
        assert!(tall_before <= 1, "content_height ballooned to {tall_before}; must stay bounded");
        // Commit order equals call order in scrollback.
        assert_eq!(tool_names(view.pending()).first().copied(), Some("read_0"));
        assert_eq!(tool_names(view.pending()).last().copied(), Some("read_19"));
    }

    /// Only the LEADING run of finished tools commits: a still-running earlier tool blocks committing a
    /// finished later one ahead of it, so scrollback order = call order even when tools interleave.
    #[test]
    fn only_leading_finished_run_commits_running_tool_blocks() {
        let mut view = TranscriptView::new();
        view.push_tool_start("a", Value::Null); // will stay running
        view.push_tool_start("b", Value::Null);
        view.push_tool_start("c", Value::Null);
        // `b` finishes first, but `a` is still running ahead of it.
        view.push_tool_end("b", false, Some("b-result".into()));
        view.commit_finished_leading_tools();
        assert!(view.pending().is_empty(), "nothing commits while the leading tool `a` runs");
        assert_eq!(view.active_tools().len(), 3, "all three stay live until `a` finishes");

        // `a` finishes → the leading run `a`, `b` commits (in order), `c` stays live.
        view.push_tool_end("a", false, Some("a-result".into()));
        view.commit_finished_leading_tools();
        assert_eq!(tool_names(view.pending()), vec!["a", "b"], "leading finished run commits in order");
        assert_eq!(view.active_tools().len(), 1, "still-running `c` stays live");
        assert_eq!(view.active_tools()[0].name, "c");
    }

    /// The `streaming.is_none()` guard: a finished tool never commits ahead of uncommitted assistant
    /// text of the same step (SCROLLBACK-ORDER safety).
    #[test]
    fn streaming_partial_blocks_tool_commit() {
        let mut view = TranscriptView::new();
        view.push_assistant_delta("thinking about the next step");
        view.push_tool_start("read", Value::Null);
        view.push_tool_end("read", false, Some("result".into()));
        view.commit_finished_leading_tools();
        assert!(view.pending().is_empty(), "tool must not commit while assistant text is streaming");
        assert_eq!(view.active_tools().len(), 1, "the finished tool stays live behind the stream");

        // Once the assistant text commits (streaming cleared), the tool is free to commit after it.
        view.commit_assistant(None);
        view.commit_finished_leading_tools();
        assert!(
            matches!(view.pending().first(), Some(Entry::Assistant(_))),
            "assistant text commits before the tool row"
        );
        assert_eq!(tool_names(view.pending()), vec!["read"], "the tool commits after the stream");
    }
}


/// Batch 5 — transcript vertical rhythm (TUI-FIDELITY L1, L3, L5, L6, X1, X2, X5, X10).
///
/// Every assertion here is anchored to a quoted pi v0.84.1 line; each test is paired with a MIRROR
/// assertion covering the shape that must NOT change.
#[cfg(test)]
mod vertical_rhythm_tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use ratatui::style::{Color, Modifier};

    use super::*;

    fn txt(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }
    fn texts(lines: &[Line<'static>]) -> Vec<String> {
        lines.iter().map(txt).collect()
    }

    fn committed_tool(result: Value) -> Entry {
        let mut view = TranscriptView::new();
        view.push_tool_start("bash", serde_json::json!({ "command": "cat wide.txt" }));
        view.push_tool_end("bash", false, Some(result));
        view.commit_tools();
        view.drain_committed().into_iter().next().unwrap()
    }

    /// L1 — `box.ts:106-119`: `for (i < paddingY) result.push(this.applyBg("", width))` before the
    /// content and again after it, with every caller passing `paddingY = 1`
    /// (`tool-execution.ts:68`). Both rows are `applyBg`-filled, so they carry the state tint across
    /// the whole width — they are the block's visible top and bottom edge.
    ///
    /// L5 — `box.ts:79-88`: the child renders at `contentWidth = width - paddingX * 2` and is then
    /// shifted right by `leftPad`, leaving a one-column tinted gutter on BOTH sides.
    #[test]
    fn tool_block_has_tinted_padding_rows_and_a_gutter_on_both_sides() {
        let theme = UiTheme::dark();
        let entry = committed_tool(serde_json::json!({
            "content": [{ "type": "text", "text": "abc" }],
        }));
        let lines = entry_lines(&entry, &theme, 30, 1, ImageOpts::default());
        let tint = theme.tool_bg_style(Style::default(), true, false).bg;
        assert!(tint.is_some(), "the dark theme defines a tool tint");

        // Row 0 is the component's own untinted `Spacer(1)` (`tool-execution.ts:63`).
        assert_eq!(txt(&lines[0]), "", "leading Spacer(1)");
        assert_eq!(lines[0].style.bg, None, "the Spacer is OUTSIDE the Box, so it is untinted");
        // Rows 1 and N-1 are the Box's paddingY rows: blank, full width, tinted.
        let last = lines.len() - 1;
        for i in [1usize, last] {
            assert_eq!(txt(&lines[i]).trim(), "", "row {i} is blank: {:?}", txt(&lines[i]));
            assert_eq!(lines[i].width(), 30, "row {i} fills the width");
            assert_eq!(lines[i].style.bg, tint, "row {i} carries the tint");
        }
        // Content rows sit at column 1 and are padded to the full width, so the last column is a
        // tinted blank — the right gutter.
        let header = &lines[2];
        assert!(txt(header).starts_with(" $ "), "1-column inset: {:?}", txt(header));
        assert_eq!(header.width(), 30, "content row fills the width");

        // MIRROR: an EMPTY block renders nothing at all — `box.ts:75-77` / `:91-93` return `[]`
        // before any padding row is pushed, so a contentless Box never leaves two stray tinted rows.
        assert!(box_lines(Vec::new(), 30, 1, 1, Style::default().bg(Color::Red)).is_empty());
    }

    /// L5, the load-bearing half — `box.ts:85` renders the child at `contentWidth`, so a line longer
    /// than the frame is broken at `width - paddingX * 2` and the last column of every row stays a
    /// tinted blank. Sizing the child at the full `width` instead (what cyrup did) lets output run
    /// flush into column N-1 with no gutter on the right.
    #[test]
    fn tool_block_content_is_sized_to_width_minus_both_paddings() {
        let theme = UiTheme::dark();
        // A single unbroken 25-column token: it is hard-broken at `contentWidth`, so the break
        // point is exactly what `contentWidth` is. At `width - 2` the first row's ink is 1 + 18 = 19
        // and column 19 stays a tinted blank; sized at the full `width` it would be 1 + 20 = 21 and
        // overflow the frame entirely.
        let long = "abcdefghijklmnopqrstuvwxy 0123456789";
        let entry = committed_tool(serde_json::json!({
            "content": [{ "type": "text", "text": long }],
        }));
        let lines = entry_lines(&entry, &theme, 20, 1, ImageOpts::default());
        let body: Vec<&Line<'static>> = lines
            .iter()
            .filter(|l| {
                let t = txt(l);
                t.contains("abcdefghijklmnopqr") || t.contains("stuvwxy")
            })
            .collect();
        assert!(body.len() > 1, "the long line must wrap inside the Box: {:?}", texts(&lines));
        for row in &body {
            assert_eq!(row.width(), 20, "every row still fills the frame");
            // The INK — everything before the right pad — must stop at or before column
            // `paddingX + contentWidth` = 19, leaving column 19 (0-indexed) as the tinted gutter.
            let ink = Line::raw(txt(row).trim_end().to_string()).width();
            assert!(ink <= 19, "row ran into the right gutter ({ink} cols): {:?}", txt(row));
            assert!(txt(row).starts_with(' '), "row lost its left inset: {:?}", txt(row));
        }

        // MIRROR: a SHORT line is not broken and is not indented twice.
        let short = committed_tool(serde_json::json!({
            "content": [{ "type": "text", "text": "zz" }],
        }));
        let sl = entry_lines(&short, &theme, 20, 1, ImageOpts::default());
        assert_eq!(sl.iter().filter(|l| txt(l).contains("zz")).count(), 1);
    }

    /// L6 — `box.ts:127-131` measures with `visibleWidth(line)`, which counts terminal COLUMNS. The
    /// old `chars().count()` under-counts every wide glyph, so the pad was too long and the tinted
    /// row overflowed the frame into a spurious extra row.
    #[test]
    fn tool_block_background_is_measured_in_columns_not_chars() {
        let theme = UiTheme::dark();
        // Eight CJK ideographs: 8 chars, 16 columns.
        let wide = committed_tool(serde_json::json!({
            "content": [{ "type": "text", "text": "日本語のテキスト" }],
        }));
        let lines = entry_lines(&wide, &theme, 30, 1, ImageOpts::default());
        for (i, line) in lines.iter().enumerate() {
            assert!(line.width() <= 30, "row {i} overflows the frame: {} cols", line.width());
        }
        let body = lines.iter().find(|l| txt(l).contains('日')).unwrap();
        assert_eq!(body.width(), 30, "the wide row is padded to exactly the width, not past it");

        // MIRROR: the same number of NARROW characters still lands on exactly the width — the fix is
        // a change of measure, not a change of target.
        let narrow = committed_tool(serde_json::json!({
            "content": [{ "type": "text", "text": "abcdefgh" }],
        }));
        let nlines = entry_lines(&narrow, &theme, 30, 1, ImageOpts::default());
        let nbody = nlines.iter().find(|l| txt(l).contains("abcdefgh")).unwrap();
        assert_eq!(nbody.width(), 30);
    }

    /// L3 — `assistant-message.ts:100-102`. The blank is gated on `hasVisibleContent` (`:96-98`),
    /// which is exactly the condition `commit_assistant` / `commit_thinking` already gate the entry
    /// on, so an empty turn emits neither entry nor blank.
    #[test]
    fn assistant_and_thinking_each_lead_with_the_spacer() {
        let theme = UiTheme::dark();
        let a = entry_lines(&Entry::Assistant("hi".into()), &theme, 40, 1, ImageOpts::default());
        assert_eq!(txt(&a[0]), "", "assistant leading Spacer(1)");
        assert_eq!(txt(&a[1]), " hi");

        let t = Entry::Thinking { text: "musing".into(), hidden: true };
        let tl = entry_lines(&t, &theme, 40, 1, ImageOpts::default());
        assert_eq!(txt(&tl[0]), "", "thinking leading Spacer(1)");
        assert_eq!(txt(&tl[1]), format!(" {HIDDEN_THINKING_LABEL}"));

        // A thinking run followed by the answer reproduces upstream's
        // `[Spacer] thinking [Spacer] text` (`:100-102` + `:166-168`).
        let seq: Vec<String> = tl.iter().chain(a.iter()).map(txt).collect();
        assert_eq!(seq, vec!["", " Thinking...", "", " hi"]);

        // MIRROR: an empty turn commits no entry, so no orphan blank can reach scrollback.
        let mut view = TranscriptView::new();
        view.commit_assistant(Some(String::new()));
        view.commit_thinking(Some("   ".into()));
        assert!(view.pending().is_empty(), "empty content must not commit");
    }

    /// X2 — `custom-message.ts:94`, `branch-summary-message.ts:37` and
    /// `compaction-summary-message.ts:38` each `addChild(new Spacer(1))` right after the label
    /// `Text`. `skill-invocation-message.ts` does NOT: `:36-45` is label then `Markdown`, with no
    /// spacer between them. The label itself is `theme.fg("customMessageLabel", "\x1b[1m[…]\x1b[22m")`
    /// inside a `Box(1, 1, customMessageBg)`, so it is inset one column and banded.
    #[test]
    fn label_blocks_space_after_the_label_except_skill() {
        let theme = UiTheme::dark();
        let branch = entry_lines(
            // `tools_expanded: true` — this test is about the EXPANDED body's spacer
            // (`branch-summary-message.ts:37` then `:39-45`); X14's collapsed arm is covered by
            // `x14_collapsed_branch_summary_is_one_hint_row`.
            &Entry::BranchSummary { summary: "we merged".into() },
            &theme,
            40,
            1,
            ImageOpts { tools_expanded: true, ..ImageOpts::default() },
        );
        let b = texts(&branch);
        assert_eq!(b[0], "", "leading Spacer(1) (interactive-mode.ts:3491)");
        assert_eq!(b[1].trim(), "", "Box top paddingY");
        assert_eq!(b[2].trim_end(), " [branch]", "label, inset 1");
        assert_eq!(b[3].trim(), "", "Spacer(1) after the label (branch-summary-message.ts:37)");
        assert_eq!(b[b.len() - 1].trim(), "", "Box bottom paddingY");

        // MIRROR: `[skill]` has NO spacer after its label — the body follows immediately.
        let skill = entry_lines(
            &Entry::SkillInvocation {
                name: "deploy".into(),
                content: "run it".into(),
                lead_spacer: true,
            },
            &theme,
            40,
            1,
            ImageOpts::default(),
        );
        let s = texts(&skill);
        assert_eq!(s[2].trim_end(), " [skill]", "label, inset 1");
        assert_eq!(s[3].trim_end(), " deploy", "the body starts on the very next row");

        // Every row of either block carries the `customMessageBg` band, padding rows included.
        let band = theme.custom_message_bg_style().bg;
        assert!(band.is_some());
        for (i, line) in branch.iter().enumerate().skip(1) {
            assert_eq!(line.style.bg, band, "branch row {i} is unbanded");
            assert_eq!(line.width(), 40, "branch row {i} does not fill the width");
        }
    }

    /// X5 — `assistant-message.ts:146-164` renders the reasoning body through a real `Markdown`
    /// with `{ color: thinkingText, italic: true }`. Because that pair only reaches
    /// `applyDefaultStyle` (`markdown.ts:377-404`), a heading keeps `mdHeading` (`:470-480`) instead
    /// of being flattened into the thinking colour.
    #[test]
    fn thinking_body_is_markdown_not_flat_text() {
        let theme = UiTheme::dark();
        let e = Entry::Thinking { text: "## Plan\n\nthen do it".into(), hidden: false };
        let lines = entry_lines(&e, &theme, 40, 0, ImageOpts::default());
        let heading = lines.iter().find(|l| txt(l).contains("Plan")).unwrap();
        // The literal `## ` is consumed by the renderer (level < 3 prints no prefix).
        assert_eq!(txt(heading), "Plan", "markdown was not parsed: {:?}", txt(heading));
        let hs = heading.spans[0].style;
        assert_eq!(hs.fg, theme.md_heading_style().fg, "heading kept its own colour");

        let prose = lines.iter().find(|l| txt(l).contains("then do it")).unwrap();
        let ps = prose.spans[0].style;
        assert_eq!(ps.fg, theme.thinking_text_style().fg, "prose takes the thinkingText colour");
        assert!(ps.add_modifier.contains(Modifier::ITALIC), "prose takes `italic: true`");

        // MIRROR: the HIDDEN form is still one plain `Text` line (`:141-143`), not markdown.
        let hidden = Entry::Thinking { text: "## Plan".into(), hidden: true };
        let hl = entry_lines(&hidden, &theme, 40, 0, ImageOpts::default());
        assert_eq!(texts(&hl), vec!["".to_string(), HIDDEN_THINKING_LABEL.to_string()]);
    }

    /// X10 — `bash.ts:311` and `:317` both build their row as `new Text(`\n${…}`, 0, 0)`. The
    /// leading `\n` makes `wrapTextWithAnsi` (`utils.ts:839`) emit an empty first row, so the
    /// truncation warning and the `Took Ns` footer are each preceded by a blank.
    #[test]
    fn bash_tool_warnings_and_duration_each_get_a_leading_blank() {
        let theme = UiTheme::dark();
        let entry = committed_tool(serde_json::json!({
            "content": [{ "type": "text", "text": "out" }],
            "details": { "fullOutputPath": "/tmp/full.txt" },
        }));
        let lines = entry_lines(&entry, &theme, 40, 1, ImageOpts::default());
        let rows = texts(&lines);
        let warn = rows.iter().position(|r| r.contains("Full output")).unwrap();
        assert_eq!(rows[warn - 1].trim(), "", "blank before the warning row");
        let took = rows.iter().position(|r| r.contains("Took ")).unwrap();
        assert_eq!(rows[took - 1].trim(), "", "blank before the duration row");

        // MIRROR: the blanks belong to the warning/duration rows, not to the output — a result with
        // neither still ends on the Box's single bottom padding row.
        let plain = committed_tool(serde_json::json!({
            "content": [{ "type": "text", "text": "out" }],
        }));
        let pl = texts(&entry_lines(&plain, &theme, 40, 1, ImageOpts::default()));
        assert!(!pl.iter().any(|r| r.contains("Full output")));
        assert_eq!(pl[pl.len() - 1].trim(), "");
        assert_ne!(pl[pl.len() - 2].trim(), "", "no doubled trailing blank");
    }
}

/// Batch 6 — the adversarial-review follow-ups on the batch-5 rhythm work.
///
/// Same rules as [`vertical_rhythm_tests`]: every assertion is anchored to a quoted pi v0.84.1 line
/// and paired with a MIRROR covering the shape that must NOT change.
#[cfg(test)]
mod rhythm_followup_tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    fn txt(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }
    fn texts(lines: &[Line<'static>]) -> Vec<String> {
        lines.iter().map(txt).collect()
    }
    /// The grapheme clusters of a rendered row set, whitespace dropped — whitespace is exactly what
    /// wrapping is allowed to remove (`utils.ts:906`, `:935`), and nothing else is.
    fn clusters(rows: &[Line<'static>]) -> Vec<String> {
        rows.iter()
            .flat_map(|r| {
                txt(r).graphemes(true).map(str::to_string).collect::<Vec<_>>()
            })
            .filter(|g| !g.trim().is_empty())
            .collect()
    }

    /// L6, the half the batch left undone — `wrap_line` must measure and break on GRAPHEME CLUSTERS.
    ///
    /// Upstream never sees a `char`: `splitIntoTokensWithAnsi` builds its tokens from
    /// `graphemeSegmenter.segment(...)` (`tui/src/utils.ts:775-798`) and `breakLongWord` re-segments
    /// the over-wide token the same way before measuring each piece (`:977-980`, `:994-1012`).
    /// Breaking per `char` severs a ZWJ emoji sequence from its joiner and a combining mark from its
    /// base, which is a correctness bug rather than a spacing one — and it measures differently from
    /// [`apply_bg`], which is the very disagreement L6 is about.
    #[test]
    fn wrap_line_breaks_on_graphemes_not_chars() {
        // ONE unbroken token (no spaces) so it takes the `breakLongWord` path, mixing a ZWJ family
        // emoji, a combining-mark sequence and wide CJK.
        let src = "AAAA\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}BBe\u{301}\u{65e5}\u{672c}\u{8a9e}";
        let rows = wrap_line(&Line::raw(src.to_string()), 8);
        assert!(rows.len() > 1, "the token must hard-break at width 8: {:?}", texts(&rows));
        for (i, r) in rows.iter().enumerate() {
            assert!(r.width() <= 8, "row {i} overflows: {} cols {:?}", r.width(), txt(r));
        }
        // No cluster was split: re-segmenting the produced rows yields the source's clusters, in
        // order. A per-`char` break emits `"\u{1f468}"` then a bare `"\u{200d}"`, which does not.
        let want: Vec<String> =
            src.graphemes(true).filter(|g| !g.trim().is_empty()).map(str::to_string).collect();
        assert_eq!(clusters(&rows), want, "a grapheme cluster was torn: {:?}", texts(&rows));
        assert!(
            rows.iter().any(|r| txt(r).contains('\u{1f468}')
                && txt(r).contains('\u{1f466}')),
            "the ZWJ family landed on one row whole: {:?}",
            texts(&rows)
        );
        assert!(
            !rows.iter().any(|r| txt(r).starts_with('\u{200d}') || txt(r).starts_with('\u{301}')),
            "a row began with an orphaned joiner/combining mark: {:?}",
            texts(&rows)
        );

        // A long unbroken run of WIDE characters: 26 CJK ideographs, 52 columns, at width 10.
        let cjk = "\u{8a9e}".repeat(26);
        let wide = wrap_line(&Line::raw(cjk.clone()), 10);
        for (i, r) in wide.iter().enumerate() {
            assert!(r.width() <= 10, "wide row {i} overflows: {} cols", r.width());
        }
        assert_eq!(clusters(&wide).len(), 26, "characters were lost or duplicated");

        // MIRROR: the token-width SUM leg is grapheme-measured too, so a space-separated wide string
        // wraps on its spaces and every row still fits.
        let words = "\u{65e5}\u{672c}\u{8a9e} \u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466} abc \u{65e5}\u{672c}\u{8a9e}";
        let ws = wrap_line(&Line::raw(words.to_string()), 9);
        for (i, r) in ws.iter().enumerate() {
            assert!(r.width() <= 9, "row {i} overflows: {} cols {:?}", r.width(), txt(r));
        }
        assert_eq!(
            clusters(&ws),
            words
                .graphemes(true)
                .filter(|g| !g.trim().is_empty())
                .map(str::to_string)
                .collect::<Vec<String>>()
        );

        // MIRROR: the fast path is untouched — a line that already fits comes back byte-identical.
        let fits = Line::raw("short".to_string());
        assert_eq!(texts(&wrap_line(&fits, 20)), vec!["short".to_string()]);
    }

    /// L3 — the gate is `hasVisibleContent` (`assistant-message.ts:96-98`):
    ///
    /// ```ts
    /// const hasVisibleContent = message.content.some(
    ///   (c) => (c.type === "text" && c.text.trim()) || (c.type === "thinking" && c.thinking.trim()),
    /// );
    /// if (hasVisibleContent) { this.contentContainer.addChild(new Spacer(1)); }
    /// ```
    ///
    /// Both legs are **trimmed**, and `:107` gates the text block's `Markdown` child on the same
    /// `content.text.trim()`. A turn of nothing but spaces therefore gets neither a body nor a
    /// blank. `commit_assistant` tested `!t.is_empty()`, which let `"   "` through.
    #[test]
    fn whitespace_only_assistant_turn_is_not_visible_content() {
        let theme = UiTheme::dark();

        // Committed leg: a whitespace-only turn never becomes an entry.
        let mut view = TranscriptView::new();
        view.commit_assistant(Some("   \n\t ".to_string()));
        assert!(view.pending().is_empty(), "whitespace-only turn committed: {:?}", view.pending());

        // …and the render arm refuses it independently, because `Entry::Assistant` is public.
        assert!(entry_lines(&Entry::Assistant("  ".into()), &theme, 40, 1, ImageOpts::default())
            .is_empty());

        // Streaming leg: the live region shows nothing at all until real text arrives.
        let mut live = TranscriptView::new();
        live.push_assistant_delta("  ");
        assert!(live.lines(40, &theme).is_empty(), "{:?}", texts(&live.lines(40, &theme)));

        // MIRROR: real content still gets exactly one leading blank, on both legs.
        let mut ok = TranscriptView::new();
        ok.commit_assistant(Some("hi".to_string()));
        let committed = entry_lines(&ok.pending()[0], &theme, 40, 1, ImageOpts::default());
        assert_eq!(texts(&committed), vec!["".to_string(), " hi".to_string()]);
        live.push_assistant_delta("hi");
        assert_eq!(texts(&live.lines(40, &theme)), vec!["".to_string(), " hi".to_string()]);
    }

    /// The `hasVisibleContent` fix must not disarm the reasoning leg: a whitespace-only THINKING
    /// buffer is invisible for the same reason (`:97`'s `c.thinking.trim()`), but a real one still
    /// carries the blank, and the `hasVisibleContentAfter` blank between the two (`:135-137`,
    /// `:166-168`) is likewise gated on the trimmed answer text.
    #[test]
    fn live_thinking_and_answer_spacers_use_the_trimmed_predicate() {
        let theme = UiTheme::dark();
        let mut view = TranscriptView::new();
        view.set_hide_thinking_block(true);

        view.push_thinking_delta("  ");
        assert!(view.lines(40, &theme).is_empty(), "whitespace-only reasoning is not visible");

        view.push_thinking_delta("musing");
        assert_eq!(
            texts(&view.lines(40, &theme)),
            vec!["".to_string(), format!(" {HIDDEN_THINKING_LABEL}")],
            "one leading blank, no trailing one — nothing visible follows"
        );

        // A whitespace-only answer must NOT open the `hasVisibleContentAfter` gap.
        view.push_assistant_delta("   ");
        assert_eq!(
            texts(&view.lines(40, &theme)),
            vec!["".to_string(), format!(" {HIDDEN_THINKING_LABEL}")]
        );

        // MIRROR: a real answer does. (The buffer is now `"   done"`; `Markdown` drops the leading
        // whitespace, so the row is the `outputPad` column plus the text.)
        view.push_assistant_delta("done");
        assert_eq!(
            texts(&view.lines(40, &theme)),
            vec![
                "".to_string(),
                format!(" {HIDDEN_THINKING_LABEL}"),
                "".to_string(),
                " done".to_string(),
            ]
        );
    }

    /// The leading `Spacer(1)` may not outlive the component it introduces. `box.ts:75-77` and
    /// `:91-93` both `return []` for an empty child set, and upstream never reaches the spacer in
    /// that case either — `interactive-mode.ts:3499`'s `if (textContent)` skips the whole
    /// `case "user"`, spacer included. Prepending it unconditionally left a bare blank row.
    #[test]
    fn a_component_that_renders_nothing_gets_no_leading_spacer() {
        let theme = UiTheme::dark();
        let empty = Entry::User { text: String::new(), lead_spacer: true };
        assert!(
            entry_lines(&empty, &theme, 40, 1, ImageOpts::default()).is_empty(),
            "orphan blank ahead of an empty user box"
        );
        // The same for the labeled shell, exercised through `box_lines` directly.
        assert!(box_lines(Vec::new(), 40, 1, 1, Style::default()).is_empty());

        // MIRROR: real text still gets the blank, the tinted paddingY row and the inset body.
        let real = Entry::User { text: "hello".into(), lead_spacer: true };
        let rows = entry_lines(&real, &theme, 40, 1, ImageOpts::default());
        assert_eq!(txt(&rows[0]), "", "leading Spacer(1)");
        assert_eq!(rows[0].width(), 0, "the Spacer is outside the Box, so it is not filled");
        assert_eq!(rows[1].width(), 40, "the Box's top paddingY row IS filled");
        assert!(txt(&rows[2]).starts_with(" hello"));
    }

    /// X18 — `showStatus` (`interactive-mode.ts:3411-3429`) puts a status line in the chat container
    /// like any other child:
    ///
    /// ```ts
    /// const spacer = new Spacer(1);
    /// const text = new Text(theme.fg("dim", message), 1, 0);
    /// this.chatContainer.addChild(spacer);
    /// this.chatContainer.addChild(text);
    /// ```
    ///
    /// So: a leading blank, then a `dim` `Text` at **paddingX 1** — `Text.render` emits
    /// `leftMargin + line + rightMargin` (`text.ts:70-76`) and wraps at `width - paddingX * 2`
    /// (`:64`). No bullet is interpolated anywhere; the `• ` prefix and the flush-left placement
    /// were cyrup inventions.
    #[test]
    fn status_row_is_a_spacer_plus_a_one_column_inset_dim_text() {
        let theme = UiTheme::dark();
        let rows = entry_lines(&Entry::Status("Model: opus".into()), &theme, 40, 1, ImageOpts::default());
        assert_eq!(texts(&rows), vec!["".to_string(), " Model: opus".to_string()]);
        assert!(!txt(&rows[1]).contains('\u{2022}'), "invented bullet: {:?}", txt(&rows[1]));
        assert_eq!(rows[1].spans[1].style, theme.dim_style(), "`theme.fg(\"dim\", message)`");

        // The inset does not depend on `outputPad` — `new Text(…, 1, 0)` hard-codes paddingX 1.
        let flush = entry_lines(&Entry::Status("Model: opus".into()), &theme, 40, 0, ImageOpts::default());
        assert_eq!(texts(&flush), vec!["".to_string(), " Model: opus".to_string()]);

        // MIRROR: a long status wraps at `contentWidth = width - 2` and every row keeps the inset.
        let long = entry_lines(
            &Entry::Status("aaaa bbbb cccc dddd eeee ffff gggg".into()),
            &theme,
            16,
            1,
            ImageOpts::default(),
        );
        assert!(long.len() > 2, "a long status must wrap: {:?}", texts(&long));
        for row in long.iter().skip(1) {
            assert!(txt(row).starts_with(' '), "row lost its inset: {:?}", txt(row));
            assert!(row.width() <= 16, "row overflows: {:?}", txt(row));
        }
    }

    /// The first child of a fresh session's chat gets NO leading blank:
    /// `interactive-mode.ts:3500` is `if (this.chatContainer.children.length > 0) { …
    /// addChild(new Spacer(1)) }`. The neighbouring call sites are deliberately different — `:3484`
    /// (compaction), `:3491` (branch) and `:3514` (the user message trailing a skill block) are all
    /// UNgated — so the gate is per call site, not a global rule.
    #[test]
    fn the_first_chat_child_gets_no_leading_spacer() {
        let theme = UiTheme::dark();
        let mut view = TranscriptView::new();
        view.push_user("first");
        let first = view.drain_committed();
        assert!(matches!(first[0], Entry::User { lead_spacer: false, .. }), "{:?}", first[0]);
        let rows = entry_lines(&first[0], &theme, 40, 1, ImageOpts::default());
        assert_eq!(rows[0].width(), 40, "row 0 is the Box's tinted paddingY row, not a Spacer");
        assert!(txt(&rows[1]).starts_with(" first"), "{:?}", texts(&rows));

        // MIRROR: the SECOND message gets one — and it still does after the first was flushed to
        // native scrollback, which is why the answer cannot be read off `pending` at render time.
        view.push_user("second");
        let second = view.drain_committed();
        assert!(matches!(second[0], Entry::User { lead_spacer: true, .. }), "{:?}", second[0]);
        let srows = entry_lines(&second[0], &theme, 40, 1, ImageOpts::default());
        assert_eq!(txt(&srows[0]), "");
        assert_eq!(srows[0].width(), 0, "the Spacer is untinted and unpadded");

        // A live streaming turn is a chat child too (`AssistantMessageComponent` is in
        // `chatContainer`), so a user message that follows one is never "first".
        let mut streamed = TranscriptView::new();
        streamed.push_assistant_delta("hi");
        streamed.push_user("after a stream");
        assert!(matches!(streamed.pending()[0], Entry::User { lead_spacer: true, .. }));

        // `:3500` covers the SKILL component too (`:3506` sits inside it), while the user message
        // that trails the skill block (`:3513-3521`) has its own unconditional spacer.
        let mut skill = TranscriptView::new();
        skill.push_user("<skill name=\"deploy\" location=\"a\">\nrun it\n</skill>\n\nand then?");
        let entries = skill.drain_committed();
        assert!(
            matches!(entries[0], Entry::SkillInvocation { lead_spacer: false, .. }),
            "{:?}",
            entries[0]
        );
        assert!(matches!(entries[1], Entry::User { lead_spacer: true, .. }), "{:?}", entries[1]);
        assert_eq!(
            txt(&entry_lines(&entries[1], &theme, 40, 1, ImageOpts::default())[0]),
            ""
        );

        // MIRROR: the ungated call sites are unaffected — a branch summary opening a fresh session
        // still leads with its blank (`:3491`).
        let branch =
            entry_lines(
                &Entry::BranchSummary { summary: "merged".into() },
                &theme,
                40,
                1,
                ImageOpts { tools_expanded: true, ..ImageOpts::default() },
            );
        assert_eq!(txt(&branch[0]), "");
        assert_eq!(branch[0].width(), 0, "`:3491`'s Spacer is outside the Box");
    }

    /// **`Entry::Block` — the body is rendered at `width - 2`, not at `width`.**
    ///
    /// The stack is `Markdown(body, 1, 1, theme)` (`interactive-mode.ts:6201`, and the identical
    /// `/changelog` site at `:6071`), and `Markdown.render` opens with
    /// `const contentWidth = Math.max(1, width - this.paddingX * 2)` (`markdown.ts:284`) — paddingX
    /// is 1, so the body wraps at two columns narrower than the rule above it and is then inset by
    /// `leftMargin = " ".repeat(this.paddingX)` (`:328`). Rendering the body at the full `width` put
    /// a row of body text one column wider than the block it sits in, and the inset then pushed it
    /// past the right edge.
    #[test]
    fn block_body_wraps_at_width_minus_two() {
        let theme = UiTheme::dark();
        // 19 columns of body inside a 20-column block: fits at `width`, must NOT fit at `width - 2`.
        let body = "aaaaaaaaa bbbbbbbbb";
        assert_eq!(Line::raw(body).width(), 19);
        let rows = entry_lines(
            &Entry::Block { title: "T".into(), markdown: body.into() },
            &theme,
            20,
            1,
            ImageOpts::default(),
        );
        let text: Vec<String> = texts(&rows);
        assert!(
            text.iter().any(|r| r.trim() == "aaaaaaaaa") && text.iter().any(|r| r.trim() == "bbbbbbbbb"),
            "the body did not wrap at `width - 2` — it was rendered at the full width: {text:?}"
        );
        for row in &rows {
            assert!(row.width() <= 20, "a row overflowed the block: {:?}", txt(row));
        }
        // `leftMargin` — every body row carries the one-column inset (`markdown.ts:328-340`).
        for row in rows.iter().filter(|r| txt(r).contains('a') || txt(r).contains('b')) {
            assert!(txt(row).starts_with(' '), "body row lost `leftMargin`: {:?}", txt(row));
        }

        // MIRROR — the two `─` rules are the one thing that DOES run edge to edge (`DynamicBorder`
        // is a chat child with no padding at all), so the block is 20 wide even though its body is 18.
        assert_eq!(txt(&rows[1]), "─".repeat(20), "the opening rule is full width");
        assert_eq!(
            txt(rows.last().unwrap()),
            "─".repeat(20),
            "the closing rule is full width"
        );
    }

    /// **`Entry::Block` — an EMPTY body contributes no rows, not two blank ones.**
    ///
    /// `Markdown.render` returns `[]` on blank text at `markdown.ts:288-296`:
    ///
    /// ```ts
    /// if (!text || text.trim() === "") {
    ///     const result: string[] = [];
    ///     …
    ///     return result;
    /// }
    /// ```
    ///
    /// That early return is BEFORE the `paddingY` block at `:352-361`, so the component's own
    /// blank rows above and below the body never materialize. Emitting them anyway left a
    /// bodyless block (a `/changelog` with no entries is the live case) four rows tall with a
    /// hollow gap the upstream never draws.
    #[test]
    fn block_with_an_empty_body_emits_no_padding_rows() {
        let theme = UiTheme::dark();
        let rule = "─".repeat(24);
        let empty = entry_lines(
            &Entry::Block { title: "What's New".into(), markdown: String::new() },
            &theme,
            24,
            1,
            ImageOpts::default(),
        );
        assert_eq!(
            texts(&empty),
            vec!["".to_string(), rule.clone(), " What's New".to_string(), String::new(), rule.clone()],
            "an empty body must add nothing between the title's trailing blank and the closing rule"
        );

        // Whitespace-only is the same case — the guard is `text.trim() === ""`, not `!text`.
        let blank = entry_lines(
            &Entry::Block { title: "What's New".into(), markdown: "  \n\n \t".into() },
            &theme,
            24,
            1,
            ImageOpts::default(),
        );
        assert_eq!(texts(&blank), texts(&empty), "a whitespace-only body is a blank body");

        // MIRROR — a real body DOES bring the `paddingY` pair with it (`:352-361`), so the two
        // shapes differ by exactly the body plus its two blanks.
        let full = entry_lines(
            &Entry::Block { title: "What's New".into(), markdown: "hello".into() },
            &theme,
            24,
            1,
            ImageOpts::default(),
        );
        assert_eq!(full.len(), empty.len() + 3, "body + one blank above + one below: {:?}", texts(&full));
        assert_eq!(txt(&full[4]), "", "paddingY row above the body");
        assert_eq!(txt(&full[5]).trim(), "hello");
        assert_eq!(txt(&full[6]), "", "paddingY row below the body");
    }
}

/// Batch-11 group X — the eight transcript items scheduled by every plan and delivered by none:
/// X6 (`read`/`write` syntax highlighting + `replaceTabs`), X7 (compact `read` classification, =
/// `G30b`), X8 (`edit` preview-state tint), X9 (dim-key/muted-word expand hints resolved from the
/// live keymap), X11 (extension-rendered custom message keeps its own colour), X14 (collapsed
/// branch/compaction summaries), X15 (renderer-failure box).
#[cfg(test)]
mod x_group_tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]
    use serde_json::json;

    use super::*;

    fn txt(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }
    fn texts(lines: &[Line<'static>]) -> Vec<String> {
        lines.iter().map(txt).collect()
    }
    fn joined(lines: &[Line<'static>]) -> String {
        texts(lines).join("\n")
    }
    /// The one rendered row whose text contains `needle`.
    fn row<'a>(lines: &'a [Line<'static>], needle: &str) -> &'a Line<'static> {
        lines
            .iter()
            .find(|l| txt(l).contains(needle))
            .unwrap_or_else(|| panic!("no row containing {needle:?} in:\n{}", joined(lines)))
    }
    fn text_result(text: &str, details: Value) -> Value {
        json!({ "content": [{ "type": "text", "text": text }], "details": details })
    }
    /// One settled tool run, rendered through the real [`tool_lines`] dispatch.
    fn run_lines(
        name: &str,
        args: Value,
        result: Option<Value>,
        expanded: bool,
        opts: ImageOpts<'_>,
    ) -> Vec<Line<'static>> {
        let theme = UiTheme::dark();
        let mut view = TranscriptView::new();
        view.push_tool_start(name, args);
        if let Some(r) = result {
            view.push_tool_end(name, false, Some(r));
        }
        let run = view.active_tools()[0].clone();
        let mut out = Vec::new();
        out.extend(tool_lines(&run, expanded, 100, &theme, opts));
        out
    }

    // --- X6 -------------------------------------------------------------------------------------

    /// **X6 — an expanded `read` of a source file is SYNTAX HIGHLIGHTED, not one flat grey wall.**
    ///
    /// `read.ts:184-190`:
    /// ```ts
    /// const lang = !isError && rawPath ? getLanguageFromPath(rawPath) : undefined;
    /// const renderedLines = lang ? highlightCode(replaceTabs(output), lang) : output.split("\n");
    /// …displayLines.map((line) => (lang ? replaceTabs(line) : theme.fg("toolOutput", replaceTabs(line))))
    /// ```
    /// so with a language in hand the body carries the highlighter's colours and NOT `toolOutput`.
    #[test]
    fn x6_expanded_read_of_a_rust_file_is_highlighted() {
        let theme = UiTheme::dark();
        let lines = run_lines(
            "read",
            json!({ "path": "src/main.rs" }),
            Some(text_result("// a comment\nfn main() {}", json!(null))),
            true,
            ImageOpts::default(),
        );
        let comment = row(&lines, "a comment");
        let comment_span = comment
            .spans
            .iter()
            .find(|s| s.content.contains("comment"))
            .expect("the comment text is on the row");
        assert_ne!(
            comment_span.style,
            theme.tool_output_style(),
            "a highlighted row is NOT painted `toolOutput`:\n{}",
            joined(&lines)
        );
        assert_eq!(
            comment_span.style,
            theme.syntax_style_for_scope("comment.line").unwrap(),
            "`// a comment` takes the syntaxComment role"
        );
        // The `fn` keyword on the next row proves the highlighter ran over the whole body, not just
        // the first line.
        let decl = row(&lines, "fn main");
        assert!(
            decl.spans.iter().any(|s| s.style == theme.syntax_style_for_scope("keyword").unwrap()),
            "`fn` takes the syntaxKeyword role:\n{}",
            joined(&lines)
        );

        // MIRROR: a path whose extension `getLanguageFromPath` does not know has `lang === undefined`,
        // so every row stays flat `theme.fg("toolOutput", …)` — the `: ` arm of the same ternary.
        let flat = run_lines(
            "read",
            json!({ "path": "notes.unknownext" }),
            Some(text_result("// a comment\nfn main() {}", json!(null))),
            true,
            ImageOpts::default(),
        );
        // The flat arm is one `Line::styled(replaceTabs(line), toolOutput)` — the colour rides on
        // the row, not on per-token spans, because there are no tokens.
        assert_eq!(
            row(&flat, "a comment").style.fg,
            theme.tool_output_style().fg,
            "unknown extension ⇒ flat toolOutput:\n{}",
            joined(&flat)
        );
    }

    /// **X6 — tabs become exactly three spaces (`replaceTabs`, `render-utils.ts:31-33`).**
    #[test]
    fn x6_tabs_are_replaced_with_three_spaces() {
        let lines = run_lines(
            "read",
            json!({ "path": "notes.unknownext" }),
            Some(text_result("a\tb", json!(null))),
            true,
            ImageOpts::default(),
        );
        assert!(
            joined(&lines).contains("a   b"),
            "tab ⇒ three spaces:\n{}",
            joined(&lines)
        );
        assert!(!joined(&lines).contains('\t'), "no raw tab survives");

        // MIRROR: `write`'s content preview runs through the same `replaceTabs` (`write.ts:160`).
        let w = run_lines(
            "write",
            json!({ "path": "notes.unknownext", "content": "a\tb" }),
            None,
            false,
            ImageOpts::default(),
        );
        assert!(joined(&w).contains("a   b"), "write preview too:\n{}", joined(&w));
    }

    // --- X7 -------------------------------------------------------------------------------------

    /// **X7 (= `G30b`) — a collapsed `read` of a `SKILL.md` is `[skill] <dir> (key to expand)`.**
    ///
    /// `read.ts:336` picks `formatCompactReadCall` only when NOT expanded, and `:130-133` labels a
    /// `SKILL.md` with `basename(dirname(absolutePath))`.
    #[test]
    fn x7_collapsed_read_of_a_skill_md_uses_the_compact_header() {
        let theme = UiTheme::dark();
        let cwd = std::path::Path::new("/home/u/.cyrup");
        let opts = ImageOpts { cwd: Some(cwd), ..ImageOpts::default() };
        let lines = run_lines("read", json!({ "path": "skills/commit-helper/SKILL.md" }), None, false, opts);
        let header = row(&lines, "[skill]");
        assert_eq!(
            txt(header).trim_end(),
            " [skill] commit-helper (ctrl+o to expand)",
            "compact skill header (the leading space is the Box's paddingX)"
        );
        assert_eq!(
            header.spans[1].style,
            theme.custom_message_label_style(),
            "`theme.fg(\"customMessageLabel\", …)` on the bracket (read.ts:153)"
        );
        assert_eq!(
            header.spans[2].style,
            theme.custom_message_text_style(),
            "`theme.fg(\"customMessageText\", label)` (read.ts:154)"
        );
        assert!(!joined(&lines).contains("SKILL.md"), "the raw path is gone:\n{}", joined(&lines));

        // MIRROR 1: EXPANDING the same read falls back to the plain `read <path>` header —
        // `!context.expanded ? getCompactReadClassification(...) : undefined` (read.ts:336).
        let expanded = run_lines("read", json!({ "path": "skills/commit-helper/SKILL.md" }), None, true, opts);
        assert!(
            joined(&expanded).contains("read skills/commit-helper/SKILL.md"),
            "expanded ⇒ plain header:\n{}",
            joined(&expanded)
        );
        assert!(!joined(&expanded).contains("[skill]"));

        // MIRROR 2: an ordinary source file classifies as nothing and keeps the plain header.
        let plain = run_lines("read", json!({ "path": "src/main.rs" }), None, false, opts);
        assert!(joined(&plain).contains("read src/main.rs"), "{}", joined(&plain));
        assert!(!joined(&plain).contains("to expand"), "no compact hint on a plain read");
    }

    /// **X7 — `AGENTS.md`/`CLAUDE.md` classify as `resource`, labelled relative to the cwd.**
    ///
    /// `read.ts:42` `COMPACT_RESOURCE_FILE_NAMES` + `:138-140`, rendered by `:160-165` as
    /// `fg("toolTitle", bold("read resource")) + " " + fg("accent", label)`.
    #[test]
    fn x7_agents_md_is_a_compact_resource_read() {
        let theme = UiTheme::dark();
        let cwd = std::path::Path::new("/w/project");
        let opts = ImageOpts { cwd: Some(cwd), ..ImageOpts::default() };
        let lines = run_lines("read", json!({ "path": "docs/AGENTS.md" }), None, false, opts);
        let header = row(&lines, "read resource");
        assert_eq!(txt(header).trim_end(), " read resource docs/AGENTS.md (ctrl+o to expand)");
        assert_eq!(header.spans[1].style, theme.tool_title_style());
        assert_eq!(header.spans[3].style, theme.accent_style(), "`fg(\"accent\", label)`");

        // MIRROR: the set is matched case-sensitively on the BASENAME, so `agents.md` is not in it.
        let lower = run_lines("read", json!({ "path": "docs/agents.md" }), None, false, opts);
        assert!(!joined(&lower).contains("read resource"), "{}", joined(&lower));
        assert!(joined(&lower).contains("read docs/agents.md"), "{}", joined(&lower));
    }

    // --- X8 -------------------------------------------------------------------------------------

    /// **X8 — a PENDING `edit` with a computed preview is tinted `toolSuccessBg`, not `toolPendingBg`.**
    ///
    /// `getEditHeaderBg` (`edit.ts:239-253`) tests the preview FIRST and never looks at `done`.
    #[test]
    fn x8_edit_tint_follows_the_preview_not_done() {
        let theme = UiTheme::dark();
        let mut view = TranscriptView::new();
        view.push_tool_start_rendered(
            "edit".to_string(),
            Some("call-1".to_string()),
            json!({ "path": "a.rs" }),
            None,
        );
        view.set_edit_preview(Some("call-1"), Ok("@@\n-old\n+new".to_string()));
        let run = view.active_tools()[0].clone();
        assert!(!run.done, "still pending — a permission prompt is up");
        let lines = tool_lines(&run, false, 60, &theme, ImageOpts::default());
        let success = theme.tool_bg_style(Style::default(), true, false);
        let pending = theme.tool_bg_style(Style::default(), false, false);
        assert_ne!(success, pending, "the dark theme distinguishes the two tints");
        assert_eq!(
            lines[1].style, success,
            "a computed preview greens the pending block (edit.ts:244-248)"
        );

        // MIRROR 1: a preview that FAILED reds the same pending block (`"error" in preview`).
        let mut v2 = TranscriptView::new();
        v2.push_tool_start_rendered(
            "edit".to_string(),
            Some("c".to_string()),
            json!({ "path": "a.rs" }),
            None,
        );
        v2.set_edit_preview(Some("c"), Err("no match for oldText".to_string()));
        let r2 = v2.active_tools()[0].clone();
        assert_eq!(
            tool_lines(&r2, false, 60, &theme, ImageOpts::default())[1].style,
            theme.tool_bg_style(Style::default(), false, true),
            "a failed preview reds it (edit.ts:245-246)"
        );

        // MIRROR 2: no preview at all still means `toolPendingBg` — the fix must not green
        // everything (`edit.ts:253`).
        let mut v3 = TranscriptView::new();
        v3.push_tool_start("edit", json!({ "path": "a.rs" }));
        let r3 = v3.active_tools()[0].clone();
        assert_eq!(tool_lines(&r3, false, 60, &theme, ImageOpts::default())[1].style, pending);

        // MIRROR 3: every OTHER tool keeps the `done`/`is_error` keying — `getEditHeaderBg` is
        // `edit`-only, and a pending `read` must stay neutral.
        let r4 = run_lines("read", json!({ "path": "a.rs" }), None, false, ImageOpts::default());
        assert_eq!(r4[1].style, pending, "pending read is untouched by X8");
    }

    // --- X9 -------------------------------------------------------------------------------------

    /// **X9 — the `… to expand` hint is dim-key + muted-words, and the key is the LIVE binding.**
    ///
    /// `read.ts:192` + `keybinding-hints.ts:42-43`.
    #[test]
    fn x9_more_lines_hint_splits_dim_key_from_muted_words() {
        let theme = UiTheme::dark();
        let body: String =
            (0..30).map(|i| format!("line {i}\n")).collect::<String>().trim_end().to_string();
        // A collapsed `read` renders no body at all (`read.ts:178-180`), so the hint is exercised
        // through `grep`, whose head-15 collapse uses the very same `more_lines_hint` (`grep.ts:111`
        // is byte-identical to `read.ts:192`).
        let g = run_lines(
            "grep",
            json!({ "pattern": "x" }),
            Some(text_result(&body, json!(null))),
            false,
            ImageOpts::default(),
        );
        let hint = row(&g, "more lines");
        let spans: Vec<(&str, Style)> =
            hint.spans.iter().map(|s| (s.content.as_ref(), s.style)).collect();
        // [0] is the Box's paddingX margin.
        assert_eq!(spans[1].0, "... (15 more lines,");
        assert_eq!(spans[1].1, theme.muted_style());
        assert_eq!(spans[3].0, "ctrl+o", "the key label is its own span");
        assert_eq!(spans[3].1, theme.dim_style(), "`theme.fg(\"dim\", keyText(...))`");
        assert_eq!(spans[4].0, " to expand");
        assert_eq!(spans[4].1, theme.muted_style(), "the description run is `muted`");
        assert_ne!(theme.dim_style(), theme.muted_style(), "the two roles differ in this theme");

        // MIRROR 1: a REBOUND `app.tools.expand` reaches the hint — the whole point of `keyText`.
        let rebound = run_lines(
            "grep",
            json!({ "pattern": "x" }),
            Some(text_result(&body, json!(null))),
            false,
            ImageOpts { expand_key: "ctrl+e/f4", ..ImageOpts::default() },
        );
        let h2 = row(&rebound, "more lines");
        assert_eq!(h2.spans[3].content.as_ref(), "ctrl+e/f4");
        assert!(!txt(h2).contains("ctrl+o"), "the literal is gone: {:?}", txt(h2));

        // MIRROR 2: the same two-tone shape on the bash tool's `… earlier lines` hint
        // (`bash.ts:281-284`), which had the identical defect.
        let b = run_lines(
            "bash",
            json!({ "command": "ls" }),
            Some(text_result(&body, json!(null))),
            false,
            ImageOpts { expand_key: "ctrl+e", ..ImageOpts::default() },
        );
        let hb = row(&b, "earlier lines");
        assert_eq!(hb.spans[1].content.as_ref(), "... (25 earlier lines,");
        assert_eq!(hb.spans[1].style, theme.muted_style());
        assert_eq!(hb.spans[3].content.as_ref(), "ctrl+e", "resolved, not the `ctrl+o` literal");
        assert_eq!(hb.spans[3].style, theme.dim_style());
        assert_eq!(hb.spans[4].content.as_ref(), " to expand");
        assert_eq!(hb.spans[4].style, theme.muted_style());
    }

    // --- X11 ------------------------------------------------------------------------------------

    /// **X11 — an extension-rendered custom message keeps its own colour; the host adds none.**
    ///
    /// `custom-message.ts:76-81` is `this.addChild(component); return;` — the component goes in
    /// as-is. cyrup restyled every row `dim`.
    #[test]
    fn x11_extension_rendered_message_is_not_forced_dim() {
        let theme = UiTheme::dark();
        let entry = Entry::Custom {
            label: "demo".to_string(),
            body: "ignored".to_string(),
            rendered: Rendered::Text("Hello from the extension".to_string()),
        };
        let lines = entry_lines(&entry, &theme, 60, 1, ImageOpts::default());
        let r = row(&lines, "Hello from the extension");
        // The old code was `Line::styled(l, theme.dim_style())`, which parks the colour on the ROW,
        // so both the row style and every span style have to be checked — asserting only on
        // `spans[0]` would pass against the defect.
        assert_ne!(r.style, theme.dim_style(), "the host must not repaint the renderer's output");
        assert_eq!(r.style, Style::default(), "added as-is ⇒ no row-level host styling");
        assert!(
            r.spans.iter().all(|s| s.style == Style::default()),
            "…and none on the spans either: {:?}",
            r.spans.iter().map(|s| s.style).collect::<Vec<_>>()
        );

        // MIRROR: the DEFAULT (no renderer) framing is unchanged — still the `[demo]` box whose body
        // is `customMessageText` (`custom-message.ts:92,107-111`).
        let default_entry = Entry::Custom {
            label: "demo".to_string(),
            body: "body text".to_string(),
            rendered: Rendered::None,
        };
        let d = entry_lines(&default_entry, &theme, 60, 1, ImageOpts::default());
        assert!(joined(&d).contains("[demo]"), "{}", joined(&d));
        assert!(joined(&d).contains("body text"), "{}", joined(&d));
    }

    // --- X15 ------------------------------------------------------------------------------------

    /// **X15 — a THROWING renderer draws Pi's failure box, not nothing.**
    ///
    /// `custom-entry.ts:47-52`: a `Box(1, 1, customMessageBg)` holding
    /// `theme.fg("error", "[type] renderer failed: <message>")`, then `:59-60`'s `Spacer(1)`.
    #[test]
    fn x15_a_throwing_renderer_draws_the_failure_box() {
        let theme = UiTheme::dark();
        let entry = Entry::Custom {
            label: "demo".to_string(),
            body: "unused".to_string(),
            rendered: Rendered::Failed("boom".to_string()),
        };
        let lines = entry_lines(&entry, &theme, 60, 1, ImageOpts::default());
        assert!(!lines.is_empty(), "the entry must not vanish");
        assert_eq!(txt(&lines[0]), "", "`custom-entry.ts:59`'s Spacer(1)");
        let r = row(&lines, "renderer failed");
        assert_eq!(txt(r).trim_end(), " [demo] renderer failed: boom");
        assert_eq!(r.spans[1].style.fg, theme.error_style().fg, "`theme.fg(\"error\", …)`");
        assert_eq!(
            r.style.bg,
            theme.custom_message_bg_style().bg,
            "inside a `Box(1, 1, customMessageBg)`"
        );
        assert!(!joined(&lines).contains("unused"), "the default body is not also drawn");
    }

    // --- X14 ------------------------------------------------------------------------------------

    /// **X14 — a collapsed branch summary is ONE row, and the expand key is the live one.**
    ///
    /// `branch-summary-message.ts:46-56`.
    #[test]
    fn x14_collapsed_branch_summary_is_one_hint_row() {
        let theme = UiTheme::dark();
        let entry =
            Entry::BranchSummary { summary: "tried the async rewrite, abandoned it".to_string() };
        let lines = entry_lines(&entry, &theme, 60, 1, ImageOpts::default());
        assert!(joined(&lines).contains("[branch]"), "{}", joined(&lines));
        let hint = row(&lines, "Branch summary");
        assert_eq!(txt(hint).trim_end(), " Branch summary (ctrl+o to expand)");
        assert_eq!(
            hint.spans[1].style,
            theme.custom_message_text_style(),
            "`fg(\"customMessageText\", \"Branch summary (\")` — NOT muted (`:49`)"
        );
        assert_eq!(hint.spans[2].style, theme.dim_style(), "`fg(\"dim\", keyText(...))` (`:50`)");
        assert!(
            !joined(&lines).contains("async rewrite"),
            "the body is withheld:\n{}",
            joined(&lines)
        );

        // MIRROR 1: the live keymap label reaches it.
        let rebound = entry_lines(
            &entry,
            &theme,
            60,
            1,
            ImageOpts { expand_key: "f2", ..ImageOpts::default() },
        );
        assert!(joined(&rebound).contains("Branch summary (f2 to expand)"), "{}", joined(&rebound));

        // MIRROR 2: expanded still renders the full markdown body + `**Branch Summary**` header.
        let open = entry_lines(
            &Entry::BranchSummary { summary: "tried the async rewrite, abandoned it".to_string() },
            &theme,
            60,
            1,
            ImageOpts { tools_expanded: true, ..ImageOpts::default() },
        );
        assert!(joined(&open).contains("async rewrite"), "{}", joined(&open));
        assert!(joined(&open).contains("Branch Summary"), "{}", joined(&open));

        // MIRROR 3: the compaction variant keeps its grouped token count in the collapsed lead
        // (`compaction-summary-message.ts:50`).
        let comp = entry_lines(
            &Entry::CompactionSummary {
                tokens_before: 123_456,
                summary: "condensed".to_string(),
            },
            &theme,
            60,
            1,
            ImageOpts::default(),
        );
        assert!(
            joined(&comp).contains("Compacted from 123,456 tokens (ctrl+o to expand)"),
            "{}",
            joined(&comp)
        );
        assert!(!joined(&comp).contains("condensed"));
    }

    /// **X14 — the collapse state is Pi's LIVE `toolOutputExpanded`, read at RENDER time.**
    ///
    /// `setToolsExpanded` does not merely store the flag; it walks `chatContainer.children` and
    /// calls `setExpanded(expanded)` on every expandable child (`interactive-mode.ts:4032-4046`),
    /// and `BranchSummaryMessageComponent.setExpanded` re-runs `updateDisplay()`
    /// (`branch-summary-message.ts:22-25`). So a summary pushed while collapsed — the default,
    /// `interactive-mode.ts:442` `private toolOutputExpanded = false` — MUST open when the flag is
    /// toggled afterwards.
    ///
    /// This replaces `x14_push_freezes_the_live_tools_expanded_flag`, which asserted
    /// `Entry::BranchSummary { expanded: false, .. }` after a `set_tool_expanded(true)` and so
    /// pinned the defect: with the flag frozen at push there was NO ordering in which the body
    /// could ever be rendered.
    #[test]
    fn x14_toggling_tools_expanded_reveals_an_already_pushed_summary_body() {
        let theme = UiTheme::dark();
        let mut view = TranscriptView::new();
        view.push_branch_summary("we merged the spike");
        view.push_compaction_summary(1234, "condensed history");

        // Collapsed (Pi's initial `toolOutputExpanded = false`): one hint row each, no body.
        let entries = view.drain_committed();
        let render = |view: &TranscriptView, entries: &[Entry]| -> String {
            entries
                .iter()
                .flat_map(|e| {
                    entry_lines(
                        e,
                        &theme,
                        60,
                        1,
                        ImageOpts { tools_expanded: view.tool_expanded(), ..ImageOpts::default() },
                    )
                })
                .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        };
        let collapsed = render(&view, &entries);
        assert!(collapsed.contains("Branch summary (ctrl+o to expand)"), "{collapsed}");
        assert!(collapsed.contains("Compacted from 1,234 tokens (ctrl+o to expand)"), "{collapsed}");
        assert!(!collapsed.contains("we merged the spike"), "{collapsed}");
        assert!(!collapsed.contains("condensed history"), "{collapsed}");

        // `Ctrl+O` AFTER the push. The SAME entries must now paint their bodies.
        assert!(view.set_tool_expanded(true), "the flag actually changed");
        let expanded = render(&view, &entries);
        assert!(
            expanded.contains("we merged the spike"),
            "the branch body is reachable after the toggle:\n{expanded}"
        );
        assert!(
            expanded.contains("condensed history"),
            "the compaction body is reachable after the toggle:\n{expanded}"
        );
        assert!(!expanded.contains("to expand"), "and the collapsed hints are gone:\n{expanded}");

        // MIRROR: toggling back re-collapses the same entries — the flag is read, not latched.
        assert!(view.set_tool_expanded(false));
        let recollapsed = render(&view, &entries);
        assert!(!recollapsed.contains("we merged the spike"), "{recollapsed}");
        assert!(recollapsed.contains("Branch summary (ctrl+o to expand)"), "{recollapsed}");
    }

    /// **X7 — `language_from_path` is the `getLanguageFromPath` table verbatim
    /// (`theme.ts:1184-1250`).**
    #[test]
    fn x6_language_from_path_matches_pis_table() {
        use crate::theme::language_from_path as lang;
        assert_eq!(lang("a.rs"), Some("rust"));
        assert_eq!(lang("a.TSX"), Some("typescript"), "the extension is lower-cased");
        assert_eq!(lang("a.zsh"), Some("bash"));
        assert_eq!(lang("a.hpp"), Some("cpp"));
        assert_eq!(lang("a.yml"), Some("yaml"));
        assert_eq!(lang("nodots"), None, "`split(\".\").pop()` yields the whole name ⇒ no match");
        assert_eq!(lang("a.nope"), None);
    }
}
