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
use layout::{body_line, box_lines, finalize_block, pad_lines, replace_tabs, text_lines};
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
