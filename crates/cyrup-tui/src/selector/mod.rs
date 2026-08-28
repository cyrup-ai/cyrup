//! The editor-swap selector engine (spec/tui/05 §1.1, §3; port of Pi's `showSelector`
//! `interactive-mode.ts:3922-3933` + the `*-selector.ts` components).
//!
//! Pi's first-party selectors are **not** floating overlays: they *replace the input editor in place*
//! in the bottom inline region, full-width, delimited top and bottom by a `DynamicBorder`
//! (`dynamic-border.ts` — a full-width `─` rule, no box corners), and they push the message history up
//! (spec/tui/05 §1.1, §11). This module realizes that as the [`Selector`] trait (the input-slot
//! occupant) plus a shared [`ListSelector`] engine over [`SelectList`],
//! and the three dependency-free selectors Pi opens this way: thinking (`thinking-selector.ts`),
//! show-images (`show-images-selector.ts`), and theme with live preview (`theme-selector.ts`).
//!
//! The floating `OverlayManager` z-stack (spec/tui/05 §2) backs only extension-custom UI + the
//! hotkeys/help popup and is gated to the outer (L7) layer — the 13 first-party selectors are all
//! editor-swap, exactly as Pi (§1.2 "Decision for parity").

use cyrup_resources::theme::builtin_themes;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::keymap::{ModelsAction, ModelsKeymap, SelectAction, SelectKeymap};
use crate::select_list::{ColumnLayout, SelectItem, SelectList};
use crate::theme::UiTheme;

mod checkbox;
mod list;

pub use checkbox::{CheckboxSelector, SCOPED_MODELS_ALL};
pub use list::ListSelector;

/// `Input.render`'s prompt, verbatim: `input.ts:380` `const prompt = "> ";`.
///
/// **S31.** There is exactly ONE prompt definition upstream and every `Input` in every dialog gets
/// it, unstyled, at column 0 — `Input.render` never colours it (`input.ts:379-446`) and every
/// component that owns one adds it to its container as a bare child, with no `Text` wrapper to
/// inset it: `oauth-selector.ts:86`, `scoped-models-selector.ts:140`, `model-selector.ts:118`,
/// `session-selector.ts:418` (`lines.push(...this.searchInput.render(width))`),
/// `config-selector.ts:396`, `settings-list.ts:94`, `extension-input.ts:64`, `login-dialog.ts:
/// 140,160` and `tree-selector.ts:1302` (the one exception, which prefixes a literal two-space
/// `indent` of its own **before** the prompt — `"  " + "> " + value`).
///
/// cyrup had three separate inventions instead: `model_selector.rs`'s accent `" ▏"…"▏"` bars
/// (U+258F appears in no pi TUI source at all), and an accent `" > "` in `session_selector.rs` and
/// `login_dialog.rs`, all one column further right than upstream and all coloured.
pub const INPUT_PROMPT: &str = "> ";

/// The complete rendered `Input` line: [`INPUT_PROMPT`] followed by the value + block caret
/// ([`search_input_spans`]). The single composition point for every search box, so a dialog cannot
/// drift into a prompt of its own again (S31).
///
/// `width` is the **full** slot width; the value gets `width - prompt.length`, pi's
/// `availableWidth` (`input.ts:381`), and an `availableWidth <= 0` renders the bare prompt
/// (`:383-385`).
pub fn input_line_spans(value: &str, cursor: usize, width: u16, theme: &UiTheme) -> Vec<Span<'static>> {
    let mut spans = Vec::with_capacity(4);
    spans.push(Span::styled(INPUT_PROMPT, theme.base_style()));
    let available = usize::from(width).saturating_sub(INPUT_PROMPT.len());
    if available == 0 {
        return spans;
    }
    spans.extend(search_input_spans(value, cursor, available, theme));
    spans
}

/// The horizontally-scrolled window of an `Input` value — a statement-for-statement port of
/// `Input.render`'s scroll block (`pi/packages/tui/src/components/input.ts:387-422` @v0.83.0),
/// returning `(visible text, caret byte offset within that text)`.
///
/// Nothing is cached between frames: pi recomputes `startCol` from the cursor and the field width on
/// every render, so a resize can never desynchronise a stored offset (there is none to store).
///
/// * `totalWidth < availableWidth` — everything fits, verbatim value, caret where it was (`:391`;
///   the comparison is STRICT so the last column stays free for an end-of-value caret).
/// * otherwise `scrollWidth` is the field, minus one column reserved for a caret sitting at the very
///   end (`:397`), and `startCol` is one of three branches (`:404-413`): 0 when the caret is in the
///   first half-window, `totalWidth - scrollWidth` when it is in the last, else `cursorCol -
///   halfWidth` (caret centred).
fn input_window(value: &str, cursor: usize, available: usize) -> (String, usize) {
    let total = crate::text_width::str_width(value);
    // `if (totalWidth < availableWidth) { visibleText = this.value; }` (`:391-393`).
    if total < available {
        return (value.to_string(), cursor);
    }
    // `const scrollWidth = this.cursor === this.value.length ? availableWidth - 1 : availableWidth`
    // (`:397`).
    let scroll =
        if cursor >= value.len() { available.saturating_sub(1) } else { available };
    // The `else` of `if (scrollWidth > 0)` (`:418-421`): nothing fits, no caret.
    if scroll == 0 {
        return (String::new(), 0);
    }
    let cursor_col = crate::text_width::str_width(value.get(..cursor).unwrap_or(""));
    let half = scroll / 2;
    let start_col = if cursor_col < half {
        0
    } else if cursor_col > total.saturating_sub(half) {
        total.saturating_sub(scroll)
    } else {
        cursor_col.saturating_sub(half)
    };
    let visible = crate::text_width::slice_by_column(value, start_col, scroll, true);
    // `cursorDisplay = beforeCursor.length` (`:416-417`) — the caret's offset *inside the window*.
    let before =
        crate::text_width::slice_by_column(value, start_col, cursor_col.saturating_sub(start_col), true);
    let caret = before.len();
    (visible, caret)
}

/// Render an embedded selector **search `Input`** with a visible block cursor at the byte offset
/// `cursor` (feature #9 "selector IME cursor"). Pi's selector search boxes render a reverse-video
/// cursor (an `Input` component) so the caret + any IME pre-edit is visible; cyrup's selectors tracked
/// the cursor offset but never drew it, leaving the search box caret-less. The character under the
/// caret (or a trailing space when the cursor is at the end) is drawn reversed over the base style;
/// text before/after keeps the base style. Shared by the model / session / scoped search boxes.
///
/// `available` is the column budget for the VALUE (the slot width less [`INPUT_PROMPT`]); a value
/// wider than that is windowed by [`input_window`] with the caret kept inside, rather than clipped
/// by the wrapping `Paragraph`.
pub fn search_input_spans(
    query: &str,
    cursor: usize,
    available: usize,
    theme: &UiTheme,
) -> Vec<Span<'static>> {
    let cursor = cursor.min(query.len());
    // Snap to a char boundary so slicing never panics on a multi-byte caret position.
    let cursor = (0..=cursor).rev().find(|i| query.is_char_boundary(*i)).unwrap_or(0);
    // Everything below draws the WINDOW, with the caret at its window-local offset.
    let (window, cursor) = input_window(query, cursor, available);
    let query = window.as_str();
    let cursor = cursor.min(query.len());
    let cursor = (0..=cursor).rev().find(|i| query.is_char_boundary(*i)).unwrap_or(0);
    let before = query.get(..cursor).unwrap_or("");
    let rest = query.get(cursor..).unwrap_or("");
    let mut chars = rest.chars();
    let (under, after) = match chars.next() {
        Some(c) => (c.to_string(), chars.as_str().to_string()),
        // Cursor at end of the query: draw the caret as a reversed space.
        None => (" ".to_string(), String::new()),
    };
    let cursor_style = theme.base_style().add_modifier(ratatui::style::Modifier::REVERSED);
    let mut spans = Vec::with_capacity(3);
    if !before.is_empty() {
        spans.push(Span::styled(before.to_string(), theme.base_style()));
    }
    spans.push(Span::styled(under, cursor_style));
    if !after.is_empty() {
        spans.push(Span::styled(after, theme.base_style()));
    }
    spans
}

/// Locate the caret inside an already-rendered selector slot: the first `REVERSED` cell in `area`,
/// scanning **rows bottom-up** and each row left-to-right, as `(x, y)` screen coordinates.
///
/// This is the port of Pi `TUI.extractCursorPosition` (`packages/tui/src/tui.ts:1189-1207`), which
/// walks the rendered lines from the last one upward looking for the `CURSOR_MARKER` that a focused
/// `Input` emits at its caret (`components/input.ts:434`: `const marker = this.focused ?
/// CURSOR_MARKER : ""`) and hands the position to `positionHardwareCursor`. Pi's marker is an
/// invisible APC string inside the text; cyrup has no text stream to hide a marker in — the frame is
/// a cell grid — so the marker is the caret CELL ITSELF, written by the single shared
/// [`search_input_spans`] (`cursor_style = base.add_modifier(REVERSED)`), which is the one and only
/// producer of a reversed cell inside a selector slot. **Anything new that reverses a cell inside
/// the input slot must be reconciled with this scan**, exactly as a second `CURSOR_MARKER` emitter
/// would have to be reconciled with Pi's.
///
/// Returns `None` when the slot holds a pure-list selector with no `Input` — the case Pi expresses
/// as "no component emitted a marker", where it hides the cursor rather than moving it.
pub(crate) fn caret_cell(buf: &ratatui::buffer::Buffer, area: Rect) -> Option<(u16, u16)> {
    let bottom = area.y.saturating_add(area.height);
    let right = area.x.saturating_add(area.width);
    for y in (area.y..bottom).rev() {
        for x in area.x..right {
            // `Buffer::cell` is bounds-checked and returns `None` outside the buffer, so a slot
            // partially off-screen (a terminal shrunk mid-frame) simply finds nothing.
            if buf
                .cell((x, y))
                .is_some_and(|c| c.modifier.contains(ratatui::style::Modifier::REVERSED))
            {
                return Some((x, y));
            }
        }
    }
    None
}

/// Split a dialog title/message string on literal `\n` (Pi's `${title}\n${message}` confirm join,
/// `interactive-mode.ts:2177`) into per-paragraph [`Line`]s, each carrying the same one-space left
/// pad the single-line title used to (`" {title}"`). Word-wrap of any resulting long paragraph is
/// applied separately, at render/measurement time, via ratatui's `Wrap`/`Paragraph::line_count`
/// (see [`title_wrapped_height`]) — this function only splits on EXPLICIT newlines.
pub(crate) fn title_lines(title: &str) -> Vec<Line<'static>> {
    title.split('\n').map(|l| Line::from(format!(" {l}"))).collect()
}

/// The WRAPPED row count `title` occupies at `width` columns — closes the fixed-0-or-1-row dialog
/// title/message truncation bug (L4 review §2.6): Pi's real `Text` component auto-sizes to its
/// wrapped content (`pi-tui/src/components/text.ts`), while cyrup's title area used to be hardcoded
/// to exactly one line (`u16::from(self.title.is_some())`) no matter how long the title/message
/// was, silently clipping anything past the first terminal row. Uses the SAME `wrapped_height`
/// (ratatui's own `Paragraph::line_count`, `transcript.rs`) the render call below applies via
/// `Wrap { trim: false }`, so the measured height can never disagree with what actually renders.
pub(crate) fn title_wrapped_height(title: &str, width: u16) -> u16 {
    let lines = title_lines(title);
    crate::transcript::wrapped_height(&lines, usize::from(width)).min(usize::from(u16::MAX)) as u16
}

/// Carve `area` top-to-bottom into `N` stacked full-width sub-rects of the given `heights`,
/// clamping at the bottom edge. **This is the dialog-envelope overflow model**; the doc below is
/// the anchor every envelope render site points at, including the three that build a `Vec<Line>`
/// and hand it to a `Paragraph` (`session_selector.rs`, `model_selector.rs`, `TrustSelector`) and
/// so get the identical clamp from ratatui instead of from here.
///
/// **Not** `Layout::vertical`. Ratatui's constraint solver is free to satisfy a set of
/// `Constraint::Length(1)` regions in any way that minimises its error terms, and when the area is
/// shorter than their sum it picks an arbitrary subset: probing the four-region `[1,1,1,1]` stack
/// `text_input.rs` used gives `[0,1,0,0]` at height 1 (the **title** row survives and the top rule
/// is dropped) and `[1,1,0,1]` at height 3 (the input FIELD is dropped while the rules stay). The
/// five-region `[1,1,Min(0),1,1]` stack `settings_selector.rs` used resolves to the HINT row alone
/// at height 1. That is exactly the "a blank row / a hint instead of its content" shape.
///
/// This helper is deterministic and strictly top-priority instead: each region gets `min(want, rows
/// still left)`. That is **exactly** what pi does to an over-tall dialog, read out of the source
/// rather than inferred:
///
/// * A pi dialog component is a plain `Container` (`packages/tui/src/tui.ts:211-245`) whose
///   `render(width)` concatenates its children's lines. It has no height input at all, so its
///   `Spacer(1)` children (`packages/tui/src/components/spacer.ts:21-27`, one `""` per line) are
///   emitted on **every** frame at every terminal size. Nothing in pi drops a `Spacer` because it
///   does not fit.
/// * The height decision happens one level up. Selectors are mounted into `editorContainer`
///   (`interactive-mode.ts:4370-4371`), one entry of the dock `VStack` (`interactive-mode.ts:
///   876-883`) with `shrink: 1, minSize: 3` — the direct analogue of cyrup's
///   `region_constraints` slot (`app.rs`, `desired_height(width).clamp(3, max_editor)`). A short
///   terminal shrinks that entry via `allocateStackSizes`
///   (`packages/tui/src/components/stack.ts:135-153`).
/// * `layoutComponent` then renders the component at its NATURAL height and allocates a shorter
///   box — `const allocatedHeight = height === undefined ? lines.length : Math.max(0,
///   Math.floor(height))` (`packages/tui/src/layout.ts:113`) — and `paintBox` paints
///   `box.lines[offset + row - box.rect.y]` for `row` in `[rect.y, rect.y + rect.height)`
///   (`layout.ts:307-310`), `offset` being 0 unless a `CURSOR_MARKER` sits below the window
///   (`layout.ts:114-118`).
///
/// So pi keeps the **first** `allocatedHeight` lines and drops the trailing ones. Which rows exist
/// is a strict PREFIX of the natural render, and it is stable across a resize: one row of shrink
/// costs exactly one row. Callers therefore pass each region's NATURAL height — the same numbers
/// their `desired_height` sums — never `area.height - fixed`, and never a "does it all fit?" gate
/// on the blank rows.
///
/// Two consequences look like bugs and are not:
///
/// * A short `/resume` or `cyrup config` slot leads with a **blank**, because
///   `session-selector.ts:737` and `config-selector.ts:901` put a `Spacer(1)` *above* the top
///   `DynamicBorder`. That blank is row 0 of the natural render, hence row 0 of every prefix of it.
///   Upstream shows the same blank.
/// * A dialog can be too short to show any list row (an `extension-selector.ts` envelope needs five
///   rows before `opt0` appears: `:44` `:45` `:47` `:49` come first). pi has no floor here either,
///   and forcing one would have to drop a `Spacer` — the behaviour this note exists to remove.
pub(crate) fn stack_rows<const N: usize>(area: Rect, heights: [u16; N]) -> [Rect; N] {
    let mut out = [Rect { x: area.x, y: area.y, width: area.width, height: 0 }; N];
    let mut y = area.y;
    let bottom = area.y.saturating_add(area.height);
    for (slot, want) in out.iter_mut().zip(heights) {
        let height = want.min(bottom.saturating_sub(y));
        *slot = Rect { x: area.x, y, width: area.width, height };
        y = y.saturating_add(height);
    }
    out
}

/// The visible window `[start, end)` of a scrolling list, centred on `selected` and clamped to the
/// ends — Pi's `SelectList.getVisibleRange` (`select-list.ts:86-90`): `start = clamp(selected -
/// maxVisible/2, 0, len - maxVisible)`, `end = min(start + maxVisible, len)`.
///
/// Every windowed body in the crate scrolls this way, and each one used to open with its own copy
/// of the three lines. The copies differed only cosmetically (an up-front `total <= visible` guard,
/// `saturating_sub` versus a redundant `.min(len)`); this is the one definition they all now call.
/// The `(i/N)` readout that usually follows a window is deliberately NOT folded in here — three
/// style/truncation variants exist, each carrying its own upstream citation.
pub(crate) fn centered_window(selected: usize, len: usize, max: usize) -> (usize, usize) {
    if len <= max {
        return (0, len);
    }
    let start = selected.saturating_sub(max / 2).min(len.saturating_sub(max));
    (start, start.saturating_add(max).min(len))
}

/// Which first-party selector occupies the input slot (spec/tui/05 §7 `SelectorKind`). The chrome
/// interprets a [`SelectorOutcome::Confirm`] / [`SelectorOutcome::Preview`] against this kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectorKind {
    /// Reasoning-level picker (`thinking-selector.ts`).
    Thinking,
    /// Step 1 of the `/settings` → "Default thinking level per model" submenu: pick the MODEL whose
    /// override to edit (Pi `SteppedSubmenuStep` `key: "model"`, `settings-selector.ts:580-608`).
    /// Confirming opens [`Self::ModelThinkingLevel`] for it.
    ModelThinking,
    /// Step 2 of that submenu: pick the LEVEL for the model chosen in [`Self::ModelThinking`]
    /// (`settings-selector.ts:610-645`), plus a `(clear override)` row when one is already set.
    ModelThinkingLevel,
    /// Inline-images yes/no (`show-images-selector.ts`).
    ShowImages,
    /// Theme picker with live preview (`theme-selector.ts`).
    Theme,
    /// Model picker (`/model`, `model-selector.ts`) — rows sourced from the model catalog (L5).
    Model,
    /// Settings menu (`/settings`, `settings-selector.ts`).
    Settings,
    /// Scoped-models enable/order picker (`/scoped-models`, `scoped-models-selector.ts`).
    ScopedModels,
    /// Resume-session picker (`/resume`, `session-selector.ts`).
    Session,
    /// Session-tree navigator (`/tree`, `tree-selector.ts`).
    Tree,
    /// The three-option "Summarize branch?" prompt shown after a `/tree` row is confirmed — Pi's
    /// `showExtensionSelector("Summarize branch?", ["No summary", "Summarize", "Summarize with
    /// custom prompt"])` (`interactive-mode.ts:4755-4760`). Escape re-shows the tree selector.
    BranchSummary,
    /// The custom-instructions editor opened by [`Self::BranchSummary`]'s third option — Pi's
    /// `showExtensionEditor("Custom summarization instructions")` (`interactive-mode.ts:4769`).
    /// Escape loops back to the [`Self::BranchSummary`] prompt (Pi's `continue`, `:4772`).
    BranchSummaryInstructions,
    /// Project-trust picker (`/trust`, `trust-selector.ts`).
    Trust,
    /// Fork-from-message picker (`/fork`, `user-message-selector.ts`).
    UserMessage,
    /// Provider login picker (`/login`, `oauth-selector.ts`). Confirming carries the chosen row's
    /// INDEX into [`crate::app::AppState::login_options`] (the resolved `AuthSelectorProvider[]`),
    /// not the provider id: one provider can offer two rows (oauth + api key) and the id alone
    /// cannot tell them apart (`getLoginProviderOptions`, `interactive-mode.ts:4938-4968`).
    Login,
    /// The auth-method choice shown when `/login` has not yet pinned one — Pi's
    /// `showLoginAuthTypeSelector` (`interactive-mode.ts:5028-5051`). Confirming carries
    /// `"oauth"` / `"api_key"`.
    LoginAuthType,
    /// The live login dialog (`LoginDialogComponent`, `components/login-dialog.ts`) occupying the
    /// input slot for the whole flow. Unlike every other kind this one is NOT a picker: it is
    /// driven by the spawned login task through [`crate::app::App::apply_login_msg`], and its
    /// `Confirm`/`Cancel` answer the flow's in-flight `AuthInteraction::prompt` rather than
    /// producing a selection.
    LoginDialog,
    /// Provider logout picker (`/logout`, `oauth-selector.ts`). Confirming carries the chosen row's
    /// INDEX into [`crate::app::AppState::logout_options`] (`getLogoutProviderOptions`,
    /// `interactive-mode.ts:4970-4979`).
    Logout,
    /// A loaded extension's `ui.confirm` dialog (L4 review §2.1): a Yes/No [`ListSelector`], exactly
    /// Pi's confirm-as-select (`interactive-mode.ts:2172-2179`). Resolved fully in-crate against the
    /// [`crate::app::AppState::pending_ui_reply`] one-shot — never becomes an `AppCommand`.
    ExtensionConfirm,
    /// A loaded extension's `ui.select` dialog (L4 review §2.1): a [`ListSelector`] over the guest's
    /// option strings. Resolved fully in-crate, same as [`Self::ExtensionConfirm`].
    ExtensionSelect,
    /// A loaded extension's `ui.input` dialog (L4 review §2.1): a [`crate::text_input::TextInputSelector`].
    /// Resolved fully in-crate, same as [`Self::ExtensionConfirm`].
    ExtensionInput,
    /// A loaded extension's `ui.editor` dialog rendered INLINE (Pi's DEFAULT `ExtensionEditorComponent`,
    /// `interactive/components/extension-editor.ts`, not a teardown to `$EDITOR`): a
    /// [`crate::extension_editor::ExtensionEditorSelector`]. Resolved fully in-crate, same as
    /// [`Self::ExtensionConfirm`]. `$VISUAL`/`$EDITOR` is reachable only via the explicit `Ctrl+G`
    /// (`app.editor.external`) escape hatch (`extension-editor.ts:107-111`), never the default.
    ExtensionEditor,
}

impl SelectorKind {
    /// Whether confirming this selector applies in-crate (theme/thinking/show-images/the extension-UI
    /// dialogs) or hands the chosen value to the run loop as an [`crate::app::AppCommand`] (the
    /// data-bound selectors, whose effect — set model, switch branch, login — lives at the session
    /// layer).
    pub fn is_data_bound(self) -> bool {
        !matches!(
            self,
            SelectorKind::Thinking
                | SelectorKind::ShowImages
                | SelectorKind::Theme
                | SelectorKind::ExtensionConfirm
                | SelectorKind::ExtensionSelect
                | SelectorKind::ExtensionInput
                | SelectorKind::ExtensionEditor
                // The login dialog answers the flow's in-flight prompt over a one-shot held on
                // `AppState::pending_login_prompt`; nothing reaches the run loop as a command.
                | SelectorKind::LoginDialog
        )
    }

    /// The selector's title shown above the list (`*-selector.ts` headers).
    pub fn title(self) -> &'static str {
        match self {
            SelectorKind::Thinking => "Thinking Level",
            // `title: "Per-Model Thinking Level"` on BOTH steps (`settings-selector.ts:583`,
            // `:612`) — the header does not change as the user walks the two pickers.
            SelectorKind::ModelThinking | SelectorKind::ModelThinkingLevel => {
                "Per-Model Thinking Level"
            }
            SelectorKind::ShowImages => "Show Images",
            SelectorKind::Theme => "Theme",
            SelectorKind::Model => "Select Model",
            SelectorKind::Settings => "Settings",
            SelectorKind::ScopedModels => "Scoped Models",
            SelectorKind::Session => "Resume Session",
            SelectorKind::Tree => "Session Tree",
            // Pi's exact prompt string (`interactive-mode.ts:4755`).
            SelectorKind::BranchSummary => "Summarize branch?",
            SelectorKind::BranchSummaryInstructions => "Custom summarization instructions",
            SelectorKind::Trust => "Project Trust",
            SelectorKind::UserMessage => "Fork from Message",
            // `OAuthSelectorComponent`'s own titles (`oauth-selector.ts:70`), verbatim.
            SelectorKind::Login => "Select provider to configure:",
            // `showLoginAuthTypeSelector`'s bare-`/login` title (`interactive-mode.ts:5058`); the
            // per-provider form (`Select authentication method for X:`) is set by
            // `resolve_auth_type_selector` and installed with `Selector::set_title`.
            SelectorKind::LoginAuthType => "Select authentication method:",
            // Overwritten with `` `Login to ${providerName}` `` (`login-dialog.ts:41`) at open time.
            SelectorKind::LoginDialog => "Login",
            SelectorKind::Logout => "Select provider to logout:",
            SelectorKind::ExtensionConfirm => "Confirm",
            SelectorKind::ExtensionSelect => "Select",
            SelectorKind::ExtensionInput => "Input",
            SelectorKind::ExtensionEditor => "Editor",
        }
    }

    /// Whether the pi component this kind stands in for builds a keyboard-hint row of its own.
    ///
    /// S3 (corrected): the hint row is **not** a property of pi's shared list engine — `SelectList`
    /// (`packages/tui/src/components/select-list.ts`) contains no hint code at all. It is built by
    /// individual components, and at v0.84.1 exactly two build the
    /// `rawKeyHint("↑↓","navigate") + keyHint(confirm,…) + keyHint(cancel,…)` row:
    ///
    /// * `ExtensionSelectorComponent` (`extension-selector.ts:63-73`) — "select"/"cancel". Four
    ///   cyrup kinds route through it: [`Self::ExtensionSelect`] and [`Self::BranchSummary`] via
    ///   `showExtensionSelector`, [`Self::ExtensionConfirm`] via `showExtensionConfirm`
    ///   (`interactive-mode.ts:2172-2179`), and [`Self::LoginAuthType`], which constructs one
    ///   directly (`interactive-mode.ts:5286-5289`).
    /// * `TrustSelectorComponent` (`trust-selector.ts:75-85`) — the same row but with **"save"**
    ///   rather than "select". cyrup's `/trust` is [`crate::settings_selector`]'s bespoke selector,
    ///   not a `ListSelector`, so this generic row never reaches it and porting that hint is left
    ///   as its own item (S40).
    ///
    /// Every other kind's component draws no such row: `ThinkingSelectorComponent`
    /// (`thinking-selector.ts:42-69`), `ShowImagesSelectorComponent`
    /// (`show-images-selector.ts:25-44`) and `ThemeSelectorComponent` (`theme-selector.ts:35-61`)
    /// are `DynamicBorder` + `SelectList` + `DynamicBorder` and nothing else;
    /// `OAuthSelectorComponent` (`/login`, `/logout`) and `UserMessageSelectorComponent` (`/fork`)
    /// contain no `keyHint` call at all.
    pub fn draws_hint_row(self) -> bool {
        matches!(
            self,
            SelectorKind::ExtensionSelect
                | SelectorKind::ExtensionConfirm
                | SelectorKind::BranchSummary
                | SelectorKind::LoginAuthType
        )
    }

    /// Whether the pi component this kind stands in for wraps its rows in a `paddingX = 1`
    /// `Text`/`TruncatedText`, i.e. insets them one column.
    ///
    /// S28 (corrected): same story as [`Self::draws_hint_row`] — the inset belongs to the
    /// *component*, not to `SelectList`. The components that inset are the four
    /// `ExtensionSelectorComponent` kinds (`extension-selector.ts:87` `new Text(text, 1, 0)`) and
    /// `OAuthSelectorComponent` (`oauth-selector.ts:144` `new TruncatedText(line, 1, 0)`, and
    /// `:149`/`:160` for its scroll indicator and empty state).
    ///
    /// The components that add a `SelectList` straight to the container — thinking, show-images,
    /// theme — pass it the container's full width, and `/fork`'s `UserMessageList`
    /// (`user-message-selector.ts:140`) is added unwrapped too. Those rows start at column 0.
    ///
    /// The one-column figure is `Text`'s own: `contentWidth = max(1, width - paddingX * 2)`
    /// (`packages/tui/src/components/text.ts:64`) with a matching left and right margin at
    /// `:70-76`, which is why the render below narrows the body by **2** and not by 1.
    pub fn insets_rows(self) -> bool {
        self.draws_hint_row() || matches!(self, SelectorKind::Login | SelectorKind::Logout)
    }

    /// Whether the pi component this kind stands in for separates its structural children with
    /// `Spacer(1)` rows (SYS-3 / L4), and therefore whether [`ListSelector`] should draw the blank
    /// rows of the envelope.
    ///
    /// Same discipline as [`Self::draws_hint_row`] and [`Self::insets_rows`]: this is a property of
    /// the individual pi COMPONENT, never of the shared list engine. `SelectList`
    /// (`packages/tui/src/components/select-list.ts`) emits no blank rows of its own, and the three
    /// components that add one straight to the container draw **zero** spacers — their whole
    /// constructor is border/list/border: `ThinkingSelectorComponent` (`thinking-selector.ts:42,66,69`),
    /// `ShowImagesSelectorComponent` (`show-images-selector.ts:25,41,44`) and `ThemeSelectorComponent`
    /// (`theme-selector.ts:35,58,61`). Putting the spacers in the engine would give those three a
    /// four-row envelope pi does not draw.
    ///
    /// The kinds that DO get them, with the constructor lines counted:
    ///
    /// * `ExtensionSelectorComponent` (`extension-selector.ts:44-75`) — `DynamicBorder`(:44),
    ///   `Spacer`(:45), title(:47), `Spacer`(:49), list(:61), `Spacer`(:62), hint(:63-73),
    ///   `Spacer`(:74), `DynamicBorder`(:75). **Four.** Reached by [`Self::ExtensionSelect`],
    ///   [`Self::ExtensionConfirm`], [`Self::BranchSummary`] and [`Self::LoginAuthType`].
    /// * `OAuthSelectorComponent` (`oauth-selector.ts:68-96`) — `DynamicBorder`(:68),
    ///   `Spacer`(:69), title(:73), `Spacer`(:74), search `Input`(:86), `Spacer`(:87), list(:91),
    ///   `Spacer`(:93), `DynamicBorder`(:96). **Four**, but one of them (`:87`) sits under the
    ///   search `Input` cyrup's `/login`+`/logout` list does not have yet (§6 "Search `Input` on
    ///   `/scoped-models`, `/login`, `/logout`, `/settings`"), so only three have a row here; the
    ///   fourth lands with the `Input`.
    ///
    /// [`Self::UserMessage`] (`/fork`) is deliberately NOT in this set even though
    /// `user-message-selector.ts:122-144` has four spacers, because its envelope is a different
    /// SHAPE — `Spacer`/title/subtitle/`Spacer`/`DynamicBorder`/`Spacer`/list/`Spacer`/
    /// `DynamicBorder`, i.e. the header sits **above** the top rule and there is a muted subtitle
    /// row. Bolting this flag onto it would put blank rows in places upstream does not have them;
    /// that component needs its own row order, not this one's.
    pub fn envelope_spacers(self) -> bool {
        matches!(
            self,
            SelectorKind::ExtensionSelect
                | SelectorKind::ExtensionConfirm
                | SelectorKind::BranchSummary
                | SelectorKind::LoginAuthType
                | SelectorKind::Login
                | SelectorKind::Logout
        )
    }
}

/// The routing outcome of feeding one key to the active selector (spec/tui/05 §3.1
/// `SelectorOutcome`). The chrome closes the slot and restores the editor on `Confirm`/`Cancel`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectorOutcome {
    /// The key was not a selector binding — nothing changed.
    Ignored,
    /// Internal state changed (e.g. selection moved); redraw, stay open.
    Redraw,
    /// Selection moved on a live-preview selector; carries the now-highlighted value
    /// (`theme-selector.ts:54-56` `onSelectionChange → onPreview`).
    Preview(String),
    /// The highlighted row was confirmed (`tui.select.confirm`); carries its value.
    Confirm(String),
    /// The highlighted row was confirmed **and asked to become the persisted default** (`Ctrl+S`,
    /// Pi's second confirm key inside the model and thinking pickers — `model-selector.ts:401-408`
    /// `onSelectAsDefaultCallback`, `thinking-selector.ts:121-125` `onSelectAsDefault`). Carries the
    /// same value [`Self::Confirm`] would.
    ///
    /// Pi keeps the two keys distinct because plain `Enter` is deliberately session-only:
    /// `selectModel(m, false)` / `selectLevel(l, false)` pass `{ persist: false }`
    /// (`interactive-mode.ts:4993`, `:4801`), and only the `Ctrl+S` sibling passes `true`
    /// (`:4999`, `:4813`). Emitting a separate outcome — rather than a flag on `Confirm` — keeps
    /// every existing `Confirm` consumer untouched and non-persisting by construction.
    ///
    /// Pi binds `Ctrl+S` here with a LITERAL `matchesKey(keyData, "ctrl+s")`
    /// (`model-selector.ts:401`, `thinking-selector.ts:122`), not a keybindings id — unlike the
    /// scoped-models `app.models.save` — so the cyrup binding is likewise non-configurable, and is
    /// only live on the selectors that opted in (Pi guards on the callback being wired at all, so
    /// an un-wired picker leaves `Ctrl+S` to its search input, `model-selector.ts:409-412`).
    ConfirmDefault(String),
    /// A setting was changed **in place** without closing the slot (Pi's settings/config selectors
    /// apply each cycle live via `onChange` and stay open — settings-list.ts `cycleValue`). Carries an
    /// `"id\u{1f}value"` payload the chrome persists; the selector remains open and redraws.
    Apply(String),
    /// The selector was dismissed (`tui.select.cancel` — `Esc`/`Ctrl+C`).
    Cancel,
    /// A `/settings` row that opens a nested picker was activated (Pi's `SettingItem.submenu`,
    /// `settings-selector.ts:603-610` — the "Theme" row opens `ThemeSubmenu`). Carries the submenu id
    /// (`"theme"`); the chrome replaces the settings selector with the matching picker (spec/tui/05 §6).
    OpenSubmenu(String),
    /// `Ctrl+G` (`app.editor.external`) pressed inside [`crate::extension_editor::
    /// ExtensionEditorSelector`] (Pi `ExtensionEditorComponent.openExternalEditor`,
    /// `extension-editor.ts:119-157`): the chrome tears the terminal down for `$VISUAL`/`$EDITOR`,
    /// seeded via [`Selector::external_edit_text`], and on success feeds the result back via
    /// [`Selector::apply_external_edit`] WITHOUT closing the slot — only every other selector kind's
    /// `handle` ever returns this (it is the default no-op there).
    OpenExternalEditor,
}

/// The input-slot occupant contract (spec/tui/05 §3.1). Object-safe so the chrome can hold a
/// `Box<dyn Selector>` in place of the editor.
pub trait Selector: Send {
    /// Lines this selector wants this frame, driving the live-region height (top rule + body + bottom
    /// rule). The chrome clamps this to the available rows.
    fn desired_height(&self, width: u16) -> u16;
    /// Render into `area` (the editor slot's `Rect`).
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme);
    /// Route one key through the [`SelectKeymap`], returning the outcome.
    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome;
    // NOTE: there is deliberately no `fn cursor(&self) -> Option<(u16, u16)>` here. It existed as a
    // defaulted accessor returning `None` that all 13 implementors took and no caller ever read, so
    // while a selector owned the input slot the terminal's hardware cursor sat wherever the last
    // frame left it. The hardware cursor is now placed by [`caret_cell`] from the chrome, which
    // finds the caret in the RENDERED CELLS — Pi's own mechanism (`TUI.extractCursorPosition`,
    // `tui.ts:1189-1207`, scans the rendered lines for the marker an `Input` emits, `input.ts:434`)
    // and the reason no per-selector accessor is needed: every selector already draws its caret
    // through the one shared [`search_input_spans`].
    /// Overwrite the selector's rendered title, if it has one (a no-op default for the selectors
    /// that don't). Used by the extension-UI countdown (Pi's `CountdownTimer`, `countdown-timer.ts:
    /// 7-38`) to live-update an open `ui.{confirm,select,input}` dialog's title with its remaining
    /// `(Ns)` once per second, exactly like `ExtensionSelectorComponent`/`ExtensionInputComponent`'s
    /// `titleText.setText` — see [`App::tick_extension_dialog_countdown`](crate::app::App).
    fn set_title(&mut self, _title: String) {}
    /// Tell the selector how many rows the host terminal has, so a body that windows can size its
    /// window from it. A no-op default, because only one pi component takes this input:
    /// `ConfigSelectorComponent`, whose `terminalHeight` parameter (`config-selector.ts:888`) is
    /// fed `ui.terminal.rows` at the construction site (`cli/config-selector.ts:47`) and becomes
    /// `this.maxVisible = Math.max(5, (terminalHeight ?? 24) - chrome)` (`config-selector.ts:
    /// 264-266`). Called before [`Self::desired_height`] on every frame, so a resize re-sizes the
    /// window the way a pi restart would.
    fn set_terminal_height(&mut self, _rows: u16) {}
    /// The current buffer text for a `Ctrl+G` external-editor round trip (Pi `app.editor.external`);
    /// `None` (the default) for every selector kind except
    /// [`crate::extension_editor::ExtensionEditorSelector`], which is the only one whose `handle` can
    /// ever return [`SelectorOutcome::OpenExternalEditor`] in the first place.
    fn external_edit_text(&self) -> Option<String> {
        None
    }
    /// Feed the external editor's result back into the buffer (Pi `this.editor.setText(newContent)`,
    /// `extension-editor.ts:152`) — a no-op default; only [`crate::extension_editor::
    /// ExtensionEditorSelector`] overrides it. The chrome calls this ONLY on a clean exit (Pi's
    /// `status === 0` gate), never on a cancelled/failed edit.
    fn apply_external_edit(&mut self, _text: &str) {}
    /// Downcast to the `/login` dialog, if that is what occupies the slot — `None` (the default) for
    /// every other selector.
    ///
    /// A targeted accessor rather than an `Any` downcast, in the same spirit as
    /// [`Self::external_edit_text`]/[`Self::apply_external_edit`] above (also overridden by exactly
    /// one implementor). The `/login` dialog is the only selector whose content is mutated by
    /// something *other* than a key press: the spawned login task pushes `AuthEvent`s and prompts at
    /// it through [`crate::app::App::apply_login_msg`], which needs `&mut LoginDialog` out of the
    /// `Box<dyn Selector>` the slot holds. Pi has the same need and solves it by keeping a typed
    /// `dialog` local in scope across the `await` (`interactive-mode.ts:5379-5403`).
    fn as_login_dialog(&mut self) -> Option<&mut crate::login_dialog::LoginDialog> {
        None
    }
    /// Downcast to the `/settings` list, if that is what this selector is — `None` (the default)
    /// for every other selector, in the same targeted-accessor spirit as
    /// [`Self::as_login_dialog`] above.
    ///
    /// The one caller is the submenu return path: pi's `SettingsList` owns its submenu and can
    /// therefore write the chosen value straight into the parent row's `currentValue` before
    /// closing it (`settings-list.ts:216-225`). cyrup's submenus are separate frames stacked in
    /// the input slot, so the chrome ([`crate::app::App::set_settings_row_value`]) needs the
    /// concrete list back out of the `Box<dyn Selector>` to do the same write.
    fn as_settings_mut(&mut self) -> Option<&mut crate::settings_selector::SettingsSelector> {
        None
    }
    /// Adopt the live `tui.editor.*` table so an embedded [`crate::text_input::Input`] resolves word
    /// motion / kill ring / undo through the user's own bindings, exactly as pi's `Input` calls
    /// `getKeybindings()` on every key (`input.ts:86`). A no-op for pure-list selectors.
    fn set_editor_keymap(&mut self, _keymap: &crate::keymap::EditorKeymap) {}
    /// Offer a bracketed paste to whatever [`crate::text_input::Input`] this selector owns (pi
    /// `Input.handlePaste`, `input.ts:362-372`: newlines stripped, tabs expanded, inserted at the
    /// caret). [`SelectorOutcome::Ignored`] — the default — means the selector owns no input and the
    /// chrome drops the paste.
    fn handle_paste(&mut self, _text: &str) -> SelectorOutcome {
        SelectorOutcome::Ignored
    }
}

/// A full-width `─` rule [`Line`] in an arbitrary `style` — the primitive behind [`border_rule`] and
/// [`border_rule_line`], and the one place the repeat-a-box-drawing-character idiom lives.
///
/// Exposed because one caller needs the rule in a colour other than `border`: `/resume`'s selected
/// separator draws it `accent` (S13, `session-selector.ts:738,746`).
pub(crate) fn rule_line(width: u16, style: Style) -> Line<'static> {
    Line::from(Span::styled("─".repeat(usize::from(width.max(1))), style))
}

/// A full-width `─` rule [`Line`] styled `border` — the [`Line`]-shaped twin of [`border_rule`], for
/// the envelopes that assemble a `Vec<Line>` instead of carving rects.
pub(crate) fn border_rule_line(width: u16, theme: &UiTheme) -> Line<'static> {
    rule_line(width, theme.border_style())
}

/// A full-width `─` rule styled `border`, matching Pi's `DynamicBorder`
/// (`dynamic-border.ts:23` `color("─".repeat(max(1,width)))`) — **not** a ratatui `Block` border, so
/// it spans the whole inline width with no corners (spec/tui/05 §11).
pub(crate) fn border_rule(width: u16, theme: &UiTheme) -> Paragraph<'static> {
    Paragraph::new(border_rule_line(width, theme))
}
