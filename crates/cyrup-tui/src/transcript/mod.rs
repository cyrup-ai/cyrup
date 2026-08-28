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
//!
//! ## Historical paths: `transcript.rs:NNNN` citations elsewhere in this crate
//! This tree is what `crates/cyrup-tui/src/transcript.rs` became. Commit `205aa38`
//! (`Decompose cyrup-tui src/transcript.rs into submodules`) replaced that single file with the
//! modules listed below, and **`src/transcript.rs` no longer exists**. Any surviving
//! `transcript.rs:NNNN` citation in a comment anywhere in this crate — the six-row width table in
//! `markdown/`, the cache and output-pad notes, the event-fold comments — is therefore
//! **historical**: it names a line of the pre-split file, not of anything at HEAD.
//!
//! Those citations are annotated here rather than churned one by one, the same call the `app.rs`
//! split made (see `app/mod.rs`) and for the same reason
//! (`docs/gap-analysis/07-cyrup-tui.md:161-162`): a mechanical line-number remap produces pointers
//! that match TEXT but not MEANING. Re-point one only when it misdirects about a **symbol**, and
//! then only by reading the target.
//!
//! This does **not** apply to the crate's upstream citations (`assistant-message.ts:104-114`,
//! `markdown.ts:284`, …), which name a line in a pinned external tag of the TypeScript this crate
//! ports and stay verbatim.

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

mod bash_block;
mod cache;
mod content;
mod entry;
mod images;
mod layout;
mod message;
mod notices;
mod render;
mod stream;
mod tool_args;
mod tool_builtin;
mod tool_render;
mod tool_result;
mod tool_state;
mod view;

#[cfg(test)]
mod tests;

pub use content::{content_text, parse_skill_block, thinking_text, ParsedSkillBlock};
pub use entry::{Entry, Rendered, ToolRun};
pub use images::{ResultImage, DEFAULT_IMAGE_WIDTH_CELLS};
pub use message::HIDDEN_THINKING_LABEL;

pub(crate) use layout::{is_ws_grapheme, text_lines_of, wrap_line, wrapped_height};
pub(crate) use render::entry_lines;
pub(crate) use tool_render::{tool_lines, ImageOpts};

// The transcript-internal helpers the submodules share. Re-bound here so every submodule reaches
// them through its own `use super::*;`, the same way `crate::app`'s split modules do.
use images::{decode_result_images, image_raster_lines, push_image_fallbacks};
use layout::{
    body_line, box_lines, finalize_block, normalize_line, normalize_terminal_output, pad_lines,
    replace_tabs, text_lines,
};
use message::{collapsed_summary_lines, group_thousands, labeled_message_lines, thinking_lines};
use tool_args::{
    compact_read_call, compact_read_classification, js_number, key_hint_spans, more_lines_hint,
    push_search_path, read_line_range, str_arg, tool_path_span, StrArg,
};
use tool_builtin::{
    edit_header_preview, render_bash, render_edit, render_extension, render_find, render_generic,
    render_grep, render_ls, render_read, render_write,
};
use tool_render::EXPAND_KEY;
use tool_result::{
    format_duration, push_bash_warnings, push_error_body, push_find_warnings, push_grep_warnings,
    push_list_output, push_ls_warnings, push_read_truncation, result_text, shorten_path,
    strip_bash_footer, trim_trailing_empty,
};

/// The ceiling on [`TranscriptView::document()`], in entries — ADR-0005 §B-1's retention bound.
///
/// **The rule that enforces it:** the document grows in exactly one place,
/// [`TranscriptView::drain_committed`], and that call trims the FRONT back to this many entries
/// before it returns, bumping [`TranscriptView::retained_dropped()`] by however many it removed.
/// Nothing else appends, so there is no second site to keep in step and no unbounded-growth path.
///
/// Upstream has no counterpart because it has no bound: pi holds every message component alive in
/// `chatContainer` for the process's life, in BOTH modes, which is what lets `switchTuiMode` hand
/// the identical component set to the incoming renderer (`interactive-mode.ts:842-892`, the splice
/// at `:847` and the re-mount at `:880` @v0.84.3 — ADR-0005 cites `:808-822` against v0.84.1).
/// cyrup retains structured [`Entry`]s rather than mounted components, so an unbounded document is
/// an unbounded `Vec` on a session that may run for days; the cap is the cyrup-side answer.
///
/// The value is a count, not a rendered-line cap, because entries are what is retained and what
/// [`TranscriptView::retained_dropped()`] counts — a line cap would have to re-measure the whole
/// document at every width change to know whether it had been exceeded. At this ceiling a long
/// session's oldest turns scroll out of the alt-screen document exactly as they already scroll out
/// of the inline renderer's native scrollback; the renderer learns of it through
/// [`TranscriptView::retained_dropped()`] and shifts its cached row offsets by the delta.
pub(crate) const MAX_RETAINED_ENTRIES: usize = 5_000;

/// The scrolling conversation history.
///
/// Entries are *committed* the moment they are pushed and are buffered in `pending` until the shell
/// drains them via [`drain_committed`](TranscriptView::drain_committed). What becomes of a drained
/// entry is the **renderer's** strategy, not a property of this type:
///
/// - **Inline (`regular`, the default renderer).** Per ADR-0001 / R-ARCH-TUI-003 the inline viewport
///   holds only the **active** region (the in-flight streaming turn), so drained entries are emitted
///   to the terminal's native scrollback with `Terminal::insert_before` and are not re-rendered
///   inside the viewport. That describes how the inline renderer spends its entries; it is not a
///   crate-wide impossibility claim, and ADR-0001 explicitly deferred the fullscreen question.
/// - **Alternate screen (`fullscreen`, ADR-0005 §Decision B-1).** With
///   [`set_retain_document`](TranscriptView::set_retain_document) on, a drain still returns the
///   entries to the caller **and** retains a clone of each in
///   [`document()`](TranscriptView::document()), in commit order — the scrollable document the
///   alt-screen renderer paints. Upstream needs no such flag: pi keeps every message component
///   alive in `chatContainer` in both modes and simply wraps `documentContainer` in a `ScrollView`
///   (`interactive-mode.ts:918`, mounted as the fullscreen layout root at `:933-936` @v0.84.3).
///
/// Retention is bounded — see [`MAX_RETAINED_ENTRIES`] for the ceiling and the rule that enforces it.
#[derive(Default)]
pub struct TranscriptView {
    /// Committed entries awaiting a flush to native scrollback (drained, not re-rendered).
    pending: Vec<Entry>,
    /// Whether anything has already been drained into native scrollback. Together with the live
    /// buffers this answers `this.chatContainer.children.length > 0` (`interactive-mode.ts:3500`) —
    /// see [`TranscriptView::chat_has_children`]. `pending` alone cannot: it is emptied on every
    /// flush, so a session with a full screen of history would look like a fresh one.
    chat_flushed: bool,
    /// ADR-0005 §B-1 — whether a drain RETAINS its entries in [`TranscriptView::document()`] instead
    /// of letting them go. `false` (the default, and every regular-mode session) is the inline
    /// mode's R-ARCH-TUI-003 strategy, byte-identical to the behaviour before this ADR; `true` is
    /// what gives the alternate-screen renderer a document to scroll. Upstream needs no such flag:
    /// it keeps every message component alive in `chatContainer` in BOTH modes, which is why
    /// `switchTuiMode` can hand the identical component set to the incoming renderer
    /// (`interactive-mode.ts:842-892` @v0.84.3).
    ///
    /// **Set once, at the composition root, for the session's life.** Because cyrup's flag is a
    /// filter and upstream's container is not, turning retention OFF and back ON splices two
    /// non-adjacent runs of history together with no gap marker and no
    /// [`TranscriptView::retained_dropped()`] movement — every row index a renderer holds would then
    /// be silently wrong. ADR-0005 §B-14's live mode switch therefore does NOT touch this flag; it
    /// is enabled whenever `tuiMode` is switchable at all, and the cost of retaining in a session
    /// that never enters fullscreen is bounded by [`MAX_RETAINED_ENTRIES`].
    retain_document: bool,
    /// ADR-0005 §B-1 — the retained document: every committed [`Entry`] this view has drained while
    /// [`TranscriptView::retain_document()`] was on, in commit order, trimmed at the front to
    /// [`MAX_RETAINED_ENTRIES`]. cyrup's stand-in for pi's `documentContainer`, which the alt screen
    /// wraps in a `ScrollView` (`interactive-mode.ts:918`, `:933-936` @v0.84.3).
    ///
    /// Empty in every regular-mode session, and the inline path never reads it: the inline renderer
    /// consumes the `Vec` [`TranscriptView::drain_committed`] returns and flushes it to native
    /// scrollback (R-ARCH-TUI-003), which is unchanged by this field's existence.
    document: Vec<Entry>,
    /// ADR-0005 §B-1 — how many entries have been removed from the FRONT of
    /// [`TranscriptView::document()`] over the session's life, by the [`MAX_RETAINED_ENTRIES`] bound
    /// or by [`TranscriptView::clear_document`]. Monotonic; never reset.
    ///
    /// This is the ONLY signal a renderer has that the row its cached offsets point at moved. A
    /// renderer records the value it last rebuilt against and shifts its scroll position by the
    /// delta, so a trim scrolls history off the top instead of silently re-aiming the viewport at
    /// unrelated rows. Upstream has no counterpart because it never drops (see
    /// [`MAX_RETAINED_ENTRIES`]).
    retained_dropped: u64,
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
    /// `markdown.mermaid` (Pi `getMermaidRenderingMode`, settings-manager.ts:1251-1254), the mode
    /// the mermaid markdown transformer's `getMode()` closure returns (`mermaid.ts:62`,
    /// `interactive-mode.ts:484-486`).
    ///
    /// Read at PAINT time — like [`Self::hidden_thinking_label`] and for the same reason: upstream
    /// re-reads the closure on every render, so cycling the `/settings` row re-renders everything
    /// still in the live region and, under the alternate-screen renderer, the retained document
    /// too. Entries the INLINE renderer has already flushed to native scrollback keep the form they
    /// committed with, the same accepted limit `outputPad`/`hideThinkingBlock` carry.
    ///
    /// The derived `Default` is [`cyrup_config::MermaidRenderingMode::Streaming`], which is
    /// already Pi's documented default (`settings-manager.ts:61`), so nothing seeds it in
    /// [`TranscriptView::new`].
    mermaid_mode: cyrup_config::MermaidRenderingMode,
    /// An extension's `setHiddenThinkingLabel(label?)` override (Pi `this.hiddenThinkingLabel`,
    /// `interactive-mode.ts:436` @v0.84.2 — as are `:435` and `:2118-2129` below); `None` is
    /// [`HIDDEN_THINKING_LABEL`], upstream's `defaultHiddenThinkingLabel` (`:435`).
    ///
    /// Read at PAINT time rather than frozen into [`Entry::Thinking`], because upstream's setter
    /// re-labels everything already on screen: it walks `chatContainer.children` and calls
    /// `setHiddenThinkingLabel` on every `AssistantMessageComponent`, plus the streaming one
    /// (`:2118-2129`). cyrup reproduces that for the live block and for every entry still inside the
    /// viewport. **[CYRUP-DELTA], and only under the INLINE renderer:** entries that renderer has
    /// flushed to the terminal's NATIVE scrollback via `insert_before` cannot be repainted at all
    /// (they are the terminal's cells now, not cyrup's), so an extension that relabels mid-session
    /// changes the live and pending rows and leaves scrolled-off history as it was written — the
    /// same architectural limit every other retroactive restyle on the inline path already has.
    ///
    /// The limit is the inline flush's, not the transcript's. Under the alternate-screen renderer
    /// (ADR-0005 §Decision B-1) retention keeps the committed [`Entry`]s in
    /// [`Self::document()`] and the renderer repaints them from there every frame, so a
    /// mid-session relabel reaches retained history too — which is exactly why the label is read at
    /// paint time rather than frozen in.
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
    /// `getCapabilities().hyperlinks` (`terminal-image.ts:130-143`). Boot default **false**, pi's
    /// own conservative value; refined once by `App::detect_image_support`. Held here rather than
    /// read from [`crate::image::hyperlinks_supported`] at paint time because that getter falls
    /// back to an env sniff — the TUI-N11 hermeticity hole — which would make every header
    /// assertion depend on the developer's `TERM_PROGRAM`.
    hyperlinks: bool,
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
    /// The hrefs `lines` was built with (TUI-020). Cached alongside because the ids in the spans'
    /// marker bits index THIS table; a cache hit that reused stale hrefs would link the right text
    /// to the wrong file.
    links: crate::osc::LinkSink,
}
