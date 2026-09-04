//! The multi-line input editor (spec/tui/03; arch-10 §3.6; gaps 23-31, 36).
//!
//! A char-indexed `Vec<Vec<char>>` buffer (one inner vec per logical line) so all cursor math is on
//! `char` indices and never byte-indexes UTF-8 (no-panic policy, R-00-009). Beyond insert/motion the
//! editor ports Pi's editor surface (`pi-tui/src/components/editor.ts`): word navigation, the kill
//! ring (Ctrl+W/U/K, Alt+D feed it; Ctrl+Y yanks; Alt+Y yank-pops), an undo stack (Ctrl+-) with
//! typing coalescing, prompt history recall (Up/Down at the buffer edges, 100-entry, draft save),
//! char-jump (Ctrl+]), bash-mode detection (leading `!`/`!!`), and the slash/path autocomplete popup
//! (`autocomplete.rs`). Keys resolve through [`EditorKeymap`] — the editor never compares keys inline
//! (R-10-018). Horizontal motion + backspace/forward-delete step over whole **grapheme clusters**
//! (emoji, ZWJ sequences, combining marks) via [`unicode_segmentation`], matching Pi's editor, which
//! treats a user-perceived character as one cursor unit (`pi-tui/src/components/editor.ts`). Vertical
//! Up/Down motion is **wrap-aware**: logical lines are wrapped into a visual-line map
//! ([`InputEditor::visual_line_map`]) and the cursor moves by *visual* line, preserving a **sticky preferred
//! column** ([`InputEditor::preferred_visual_col`]) across short/long/rewrapped lines, falling through
//! to history at the first visual line and to line-end at the last (spec/tui/03 §4.1-§4.2). Large
//! pastes collapse to atomic `[paste #N …]` markers ([`InputEditor::handle_paste`]) that expand back
//! to content on submit ([`InputEditor::expanded_text`], spec/tui/03 §5.5). Those markers are atomic
//! to the CARET too, on every axis: horizontal motion and deletion step them whole through
//! [`InputEditor::marker_grapheme_boundaries`], and vertical motion snaps out of one it would
//! otherwise land inside ([`InputEditor::move_to_visual_line`]) — a caret parked mid-marker is what
//! turns the next Backspace into silent data loss.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;

use unicode_segmentation::UnicodeSegmentation;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style}; // editor.rs:26 — CMDHINT_01 needs the named type
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::autocomplete::{Autocomplete, CompletionContext};
use crate::commands::CommandRegistry;
use crate::component::Component;
use crate::keymap::{EditorAction, EditorKeymap};
use crate::theme::UiTheme;

/// History ring capacity (`editor.ts:381`).
const HISTORY_CAP: usize = 100;

mod completion;
mod config;
mod edit;
mod history;
mod keys;
pub(crate) mod kill_ring;
mod motion;
mod paste;
mod render;
pub(crate) mod undo;
pub(crate) mod word_nav;
mod wrap;

#[cfg(test)]
mod tests;

// The editor-internal helpers the submodules share. Re-bound here so every submodule reaches
// them through its own `use super::*;`, the same way `crate::app` and `crate::transcript` do.
use word_nav::{WordSeg, find_word_backward, find_word_forward, is_punctuation};
use wrap::{display_width, grapheme_boundaries};

/// Outcome of feeding a key to the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorOutcome {
    /// Enter pressed (or a slash-command accepted from the popup): the submitted text (buffer cleared).
    Submit(String),
    /// The buffer or cursor changed → request a re-render.
    Edited,
    /// The key was not handled by the editor.
    Ignored,
}

/// What the previous mutating op was, gating kill-ring accumulation and undo coalescing
/// (`editor.ts` `lastAction`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LastAction {
    None,
    Kill,
    Yank,
    Type,
}

/// Direction for char-jump mode (`editor.ts:307`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JumpDir {
    Forward,
    Backward,
}

/// An undo snapshot — "editor text state plus the paste registry"
/// (`pi/packages/tui/src/components/editor.ts:216-220` @v0.83.0):
///
/// ```text
/// interface EditorSnapshot { state: EditorState; pastes: Map<number, string>; pasteCounter: number }
/// ```
///
/// [`lines`](Self::lines)/[`row`](Self::row)/[`col`](Self::col) are pi's `EditorState`
/// (`editor.ts:209-213`, `{ lines, cursorLine, cursorCol }`); `pastes`/`paste_counter` are the
/// registry, without which an undone marker deletion puts the marker TEXT back on screen while
/// [`InputEditor::expanded_text`] can no longer resolve it — the model then receives the literal
/// `[paste #N …]` string (TUI-042). The deep copy `structuredClone` gives pi for free
/// (`undo-stack.ts:11-13`) is the `Clone` on this struct.
#[derive(Clone, Debug)]
struct Snapshot {
    lines: Vec<Vec<char>>,
    row: usize,
    col: usize,
    /// The paste registry as of the snapshot (`EditorSnapshot.pastes`, `editor.ts:218`).
    pastes: BTreeMap<u32, String>,
    /// The paste id counter as of the snapshot (`EditorSnapshot.pasteCounter`, `editor.ts:219`).
    paste_counter: u32,
}

/// A multi-line text editor with a block cursor, kill ring, undo, history, and autocomplete.
pub struct InputEditor {
    /// Lines as char vectors. Invariant: always at least one line.
    lines: Vec<Vec<char>>,
    row: usize,
    col: usize,
    focused: bool,
    keymap: EditorKeymap,
    kill_ring: Vec<String>,
    last_action: LastAction,
    undo: Vec<Snapshot>,
    history: VecDeque<String>,
    /// `-1` ⇒ not browsing history; otherwise an index into `history`.
    history_index: isize,
    history_draft: Option<Snapshot>,
    jump: Option<JumpDir>,
    registry: CommandRegistry,
    autocomplete: Option<Autocomplete>,
    /// Max visible rows in the autocomplete dropdown (Pi `autocompleteMaxVisible`, default 5, clamped
    /// 3–20; item #6). Plumbed from `settings.autocompleteMaxVisible` by the binary; applied to every
    /// opened popup's [`crate::SelectList`].
    autocomplete_max_visible: u16,
    /// The configurable autocomplete-popup key table (item #6; `tui.autocomplete.*`). The popup's
    /// navigate/accept/cancel keys are no longer hardcoded — a `keybindings.json` rebind flows through.
    autocomplete_keymap: crate::keymap::AutocompleteKeymap,
    cwd: PathBuf,
    /// Cached whole-tree candidate list for `@`-mention search — files AND directories, the latter
    /// marked by a trailing `/` (`autocomplete.ts` populates once, then fuzzy-filters in-process per
    /// keystroke). Lazily built on the first `@`-mention, invalidated on `set_cwd`. `None` until
    /// first needed.
    mention_files: Option<Vec<String>>,
    /// The live `/model` / `/login` / `/thinking` candidate sets the argument completers rank
    /// (`interactive-mode.ts:685-736` @v0.84.3), plus the answers extension commands' own
    /// completers gave (`:753`). Push-fed like `registry` and `mention_files`, because
    /// [`Autocomplete::compute`] is synchronous and the editor holds no session: the app snapshots
    /// the builtin three on boot, session swap, credential change and scope save
    /// ([`crate::App::refresh_argument_sources`]) and refreshes the extension entries per keystroke
    /// ([`crate::App::refresh_extension_completions`]). Empty until the first push.
    arg_sources: crate::autocomplete::ArgumentSources,
    /// The layout width (in columns) used to wrap logical lines into **visual** lines for vertical
    /// motion (`editor.ts:1690` `build_visual_line_map(width)`). Updated every render; `80` until the
    /// first render. Vertical Up/Down resolve against the visual map computed at this width.
    view_width: usize,
    /// First **visual** line of the render window (`editor.ts:288` `scrollOffset`). The editor shows
    /// at most `maxVisibleLines` rows; anything above/below is scrolled out and announced by the
    /// `─── ↑ N more ` / `─── ↓ N more ` rules ([`crate::editor::render::scroll_border`], `editor.ts:259-268`). Kept in range
    /// and re-pointed at the caret every render (`editor.ts:507-516`) and reset to `0` whenever the
    /// buffer is replaced wholesale (`editor.ts:471`, `:449`).
    scroll_offset: usize,
    /// The host TERMINAL's row count, from which the editor derives its own visible-row budget
    /// (`editor.ts:499-501`):
    ///
    /// ```text
    /// const terminalRows = this.tui.terminal.rows;
    /// const maxVisibleLines = Math.max(5, Math.floor(terminalRows * 0.3));
    /// ```
    ///
    /// The cap is INSIDE the component upstream, read from `this.tui` inside a `render(width)` that
    /// takes no height at all (`:499-501`) — the editor is never told how tall its slot is and never
    /// trusts a container to have windowed it. cyrup derived the same number solely from
    /// `area.height - 2`, i.e. from whatever rect the caller happened to hand it, so an
    /// [`InputEditor`] rendered anywhere other than [`crate::app::region_constraints`]'s slot — a
    /// bare `Component::render` into a taller rect, an embedder's own layout — silently lost the cap
    /// and drew as many rows as it was given.
    ///
    /// `24` until [`Self::set_terminal_height`] lands, pi's own `?? 24` fallback for a missing
    /// terminal height (`config-selector.ts:264-266`). The rect still participates: the render
    /// window is `min(area.height - 2, max(5, floor(rows * 0.3)))`, so a slot CLIPPED shorter than
    /// the editor asked for still degrades correctly — something pi's height-free `render(width)`
    /// cannot express.
    term_rows: u16,
    /// The **sticky preferred column** for vertical motion (`editor.ts:66`
    /// `preferred_visual_col`): the intended visual column Up/Down try to land on across short/long/
    /// rewrapped visual lines. `Some` while a vertical run is in progress; cleared by any horizontal
    /// motion, edit, or paste so the next vertical move re-seeds from the live cursor (spec/tui/03 §4.2).
    preferred_visual_col: Option<usize>,
    /// The **pre-snap column** stashed when a vertical move snapped the caret back to the start of
    /// an atomic segment — pi's `snappedFromCursorCol` (`editor.ts:331-336`):
    ///
    /// ```text
    /// // When the cursor is snapped to the start of an atomic segment, e.g. a
    /// // paste marker, cursorCol no longer reflects where the cursor would have
    /// // landed. This field stores the pre-snap cursorCol so that the next
    /// // vertical move can resolve it to a visual column on whatever VL it belongs
    /// // to.
    /// ```
    ///
    /// Set by [`InputEditor::move_to_visual_line`] (`editor.ts:1455-1461`), consumed by the next
    /// vertical move (`:1396-1404`) and cleared both when a vertical move lands outside every atomic
    /// segment (`:1465`) and by [`InputEditor::reset_preferred_col`], cyrup's `setCursorCol`
    /// (`:1377-1381`).
    snapped_from_col: Option<usize>,
    /// Large-paste store (`editor.ts:81` `pastes: id -> expanded content`): each entry is the full
    /// pasted text the buffer shows collapsed to a `[paste #N …]` marker. [`InputEditor::expanded_text`] substitutes
    /// markers back to content on submit (`expandPasteMarkers`, spec/tui/03 §5.5).
    pastes: BTreeMap<u32, String>,
    /// Monotonic id for the next large paste (`editor.ts:82` `paste_counter`).
    paste_counter: u32,
    /// The current reasoning level (`off`/`minimal`/`low`/`medium`/`high`/`xhigh`/`max`) — the editor's
    /// top/bottom rule color is the primary always-visible thinking-level signal
    /// (`interactive-mode.ts:3533-3541`, spec/tui/03 §3.3). Recolored green in bash mode. Updated by
    /// the app on `ThinkingLevelChanged`; `"medium"` until set.
    thinking_level: String,
    /// Whether this editor's rule is owned by a reasoning level at all (T9, TUI-FIDELITY §2).
    ///
    /// Pi's shared `Editor` takes its rule colour from `getEditorTheme().borderColor` =
    /// `theme.fg("borderMuted", …)` (v0.84.1 `theme.ts:1301-1304`, `tui/src/components/editor.ts:348`).
    /// Only the *chat* editor is then reassigned per thinking level / bash mode
    /// (`interactive-mode.ts:3990-3993`). An `ExtensionEditorComponent` — `new Editor(tui,
    /// getEditorTheme(), options)`, `components/extension-editor.ts:70` — never is, so it keeps
    /// `borderMuted`. `true` (the chat editor) by default; the extension-editor dialog clears it via
    /// [`InputEditor::use_muted_border`].
    thinking_level_owns_border: bool,
    /// Horizontal padding, in columns, applied INSIDE the top/bottom rules (Pi `editorPaddingX` →
    /// `CustomEditor({paddingX})`, `tui/src/components/editor.ts:349,484-489`). `0` (Pi's default)
    /// keeps the historical flush layout. The rules themselves still span the full width — Pi pads
    /// only the text rows (`editor.ts:522` left/right pad vs `:530` `horizontal.repeat(width)`).
    padding_x: u16,
    /// Whether the terminal's real (hardware) cursor is placed on the caret each frame (Pi
    /// `showHardwareCursor` → `TUI.setShowHardwareCursor`, `tui/src/tui.ts:346-352,1659-1663`).
    /// **Off** by default, as Pi's (`tui.ts:312`, `settings-manager.ts:1182` — only an explicit
    /// setting or `PI_HARDWARE_CURSOR=1` turns it on): the always-drawn reverse-video soft cursor is
    /// the caret the user sees, and Pi calls `terminal.hideCursor()` on every frame while this is
    /// false. Ratatui couples position and visibility (`Terminal::draw` hides the cursor whenever
    /// `Frame::set_cursor_position` was not called), so this flag gates that call.
    show_hardware_cursor: bool,
}

/// One **visual** line of the wrapped editor: a contiguous slice of a logical line that fits the
/// layout width (`editor.ts:1690-1715` `VisualLine { logical_line, start_col, length }`). An empty
/// logical line yields exactly one zero-length visual line so the cursor still has a row to sit on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VisualLine {
    /// The logical line (`lines` index) this visual line is a slice of.
    pub logical: usize,
    /// The char column in the logical line where this visual line begins.
    pub start: usize,
    /// The number of chars on this visual line (0 for an empty logical line / trailing wrap).
    pub len: usize,
}

/// The persistent command-token highlight and argument-hint ghost, computed fresh from the buffer
/// on every render (CMDHINT_01 — cyrup-original; pi renders neither. Its `argumentHint` reaches
/// exactly one site upstream, the popup description at `tui/src/autocomplete.ts:315`).
///
/// Ranges are **char** indices into `lines[0]`, never bytes: the buffer is `Vec<Vec<char>>`
/// (`editor.rs:98`) and the wrap map ([`VisualLine`]) is char-based, so a byte range would
/// mis-slice the instant a command name or its preceding text contains a non-ASCII char.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandHighlight {
    /// Char range within `lines[0]` (always starting at 0, including the leading `/`) to render in
    /// [`crate::UiTheme::accent_style`].
    pub token: std::ops::Range<usize>,
    /// The command's unmodified, **unsplit** `argument_hint` (`SlashCommand::argument_hint` is a
    /// `Option<Cow<'static, str>>`, `commands.rs:50`; owned `String` here so the highlight borrows
    /// nothing from the registry), to draw as dim ghost text after the buffer's last visual line.
    /// `None` when there is no exact match, no hint, or the argument zone already holds a
    /// non-whitespace char.
    pub ghost: Option<String>,
}
